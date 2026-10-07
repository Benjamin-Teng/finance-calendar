// host/ui/wallpapers/lib/astrolabe-sessions.mjs
//
// 星盤（task 3.3）的純邏輯：把設定檔的時段表展開成絕對時間區間、判定各市場目前狀態、列出
// 「接下來」的開收盤。ES module、不依賴 DOM，可在 Node 測試
// （host/tests/wallpapers/astrolabe-sessions.test.mjs）。繪圖在同目錄 `astrolabe-draw.mjs`。
//
// ## 規則（spec「星盤呈現六市場交易時段」「星盤的休市日與生效日」＋ controller rulings）
//
// 1. 時段表與休市表一律來自設定（`config.markets`、`config.holidays`），本模組不寫死任何市場。
//    時段以「交易所當地時刻」定義，經 `zonedToUtc` 換成 UTC，夏令時間自動反映。
// 2. 展開：以「現在」在該市場時區的當地日期為準，前 2 天到後 1 天，每天依 `days`（星期）、
//    `effective`（生效日，含當天）、`skip`（`nth:N:W`＝當月第 N 個星期 W 不開）決定是否有該時段。
//    `end <= start` 視為跨午夜，終點落在隔天。
// 3. 休市表語意見 config/README.md。**每個時段以「起點」的交易所當地日期查表**：
//    - 起點那天休市（closed）→ 整段不畫；起點那天不休市 → 整段照畫，即使跨進休市日（已知簡化）。
//    - 半日市（half）：非 night 時段一律截到 `close`；起點 ≥ `close` 的非 night 時段不畫
//      （自然排除收盤競價與盤後、保留開盤競價）；night 照畫。
//    - unknown（through 之後）照一般交易日。
// 4. 開盤＝當天第一段實際畫出的 regular 的起點；收盤＝當天實際畫出的 regular／auction 中最晚的終點。
//    午休（如東京前場 11:30）不是收盤。
// 5. 狀態：市場的「當地今天」是休市日 → `closed`，文字「休市（假日名）」，整條環變暗，
//    優先於任何進行中的時段（例：前一晚開始、跨進休市日的夜盤）。否則取進行中時段的最高優先序
//    （regular > auction > pre = post > night），都沒有 → `idle`（休息）。
// 6. 「現在」向下取整到 15 分鐘；可見窗口為現在之前 10 小時到之後 12 小時；
//    「接下來」＝嚴格晚於現在、24 小時內的開收盤，依時刻排序取前 4 筆（同時刻依設定中的市場順序）。
// 7. 設定錯誤（星期拼錯、時間格式錯、effective 不是 YYYY-MM-DD 真實日期、skip 格式錯）不拋錯：該時段略過
//    （skip 錯則忽略 skip），警告字串放在回傳的 `warnings`（同一則只出現一次）。

import { floorToStep, isValidTz, tzOffsetMin, tzParts, zonedToUtc } from './core.mjs';
import { holidayOn, isRealDate } from './config-holidays.mjs';

/** 時間精度：15 分鐘。 */
export const STEP_MS = 15 * 60000;
/** 可見窗口：現在之前 10 小時、之後 12 小時。 */
export const PAST_HOURS = 10;
export const FUTURE_HOURS = 12;
/** 「接下來」最多幾筆、往後看幾小時。 */
export const UPCOMING_MAX = 4;
export const UPCOMING_HORIZON_HOURS = 24;

export const STATUS_TEXT = { regular: '交易中', auction: '集合競價', pre: '盤前', post: '盤後', night: '夜盤', idle: '休息' };
export const STATUS_RANK = { regular: 5, auction: 4, pre: 3, post: 3, night: 2 };
export const SEGMENT_KINDS = Object.keys(STATUS_RANK);

const DOW = { Sun: 0, Mon: 1, Tue: 2, Wed: 3, Thu: 4, Fri: 5, Sat: 6 };
const HM_RE = /^([01]\d|2[0-3]):([0-5]\d)$/;
const SKIP_RE = /^nth:([1-5]):([0-6])$/;
const pad2 = (n) => String(n).padStart(2, '0');

/**
 * `"Mon-Fri"`、`"Sun-Thu"`、`"Fri-Mon"`、`"Mon,Wed"` → `{ days: [星期數字（0＝週日）], error }`。
 * 不認得的星期名或非字串 → `days: []` 與錯誤說明（不會無窮迴圈）。
 */
export function parseDays(spec) {
  if (typeof spec !== 'string' || spec.trim() === '') {
    return { days: [], error: `days（${String(spec)}）應為 "Mon-Fri" 這類字串` };
  }
  const out = [];
  for (const part of spec.split(',')) {
    const ab = part.trim().split('-');
    const a = DOW[ab[0]];
    const b = ab.length > 1 ? DOW[ab[1]] : a;
    if (ab.length > 2 || a === undefined || b === undefined) {
      return { days: [], error: `days（${spec}）有不認得的星期，應為 Sun Mon Tue Wed Thu Fri Sat` };
    }
    for (let i = a; ; i = (i + 1) % 7) {
      if (!out.includes(i)) out.push(i);
      if (i === b) break;
    }
  }
  return { days: out, error: null };
}

/** `"HH:MM"` → `{ h, m }`；格式錯回 null。 */
function parseHm(s) {
  const m = typeof s === 'string' ? HM_RE.exec(s) : null;
  return m ? { h: +m[1], m: +m[2] } : null;
}

/** 可見窗口裁切：`[h0, h1]`（相對現在的小時）與 `[-PAST_HOURS, FUTURE_HOURS]` 的交集；無交集回 null。 */
export function clipToWindow(h0, h1) {
  const c0 = Math.max(h0, -PAST_HOURS);
  const c1 = Math.min(h1, FUTURE_HOURS);
  return c1 > c0 ? [c0, c1] : null;
}

/** UTC 偏移（分鐘，東為正）→ `UTC+8`、`UTC−5`、`UTC+5:30`（負號用 U+2212，與樣稿相同）。 */
export function formatUtcOffset(offMin) {
  const sign = offMin < 0 ? '−' : '+';
  const a = Math.abs(offMin);
  const h = Math.floor(a / 60);
  const m = a % 60;
  return `UTC${sign}${h}${m ? `:${pad2(m)}` : ''}`;
}

/** UTC 毫秒 → 某時區的 `HH:MM`。 */
export function formatHm(ms, tz) {
  const p = tzParts(tz, ms);
  return `${pad2(p.h)}:${pad2(p.mi)}`;
}

/**
 * 展開一個市場在「現在」前後的時段（規則 2–4）。回傳依起點排序的陣列，每筆：
 * `{ kind, label, start, end, day, t0, t1, isOpen, isClose }`；`day`＝起點的交易所當地日期，
 * `t0`／`t1`＝UTC 毫秒（半日市已截短）。設定錯誤的訊息加進 `warnings`（Set）。
 */
export function expandMarket(mk, nowMs, config, warnings = new Set()) {
  const at = `markets.${mk?.id ?? '?'}`;
  const segments = Array.isArray(mk?.segments) ? mk.segments : [];
  const out = [];
  const base = tzParts(mk.tz, nowMs);

  for (let off = -2; off <= 1; off++) {
    const dt = new Date(Date.UTC(base.y, base.mo - 1, base.d + off));
    const y = dt.getUTCFullYear();
    const mo = dt.getUTCMonth() + 1;
    const d = dt.getUTCDate();
    const wd = dt.getUTCDay();
    const day = `${y}-${pad2(mo)}-${pad2(d)}`;

    const hol = holidayOn(config, mk.id, day, warnings);
    if (hol.status === 'closed') continue;
    let halfClose = null;
    if (hol.status === 'half') {
      const c = parseHm(hol.close);
      if (c) halfClose = zonedToUtc(mk.tz, y, mo, d, c.h, c.m);
      else warnings.add(`holidays.${mk.id}：${day} 半日市的 close（${String(hol.close)}）不是 HH:MM，當天照一般交易日畫`);
    }

    const dayOut = [];
    segments.forEach((sg, i) => {
      const where = `${at}.segments[${i}]（${sg?.label ?? ''}）`;
      if (!SEGMENT_KINDS.includes(sg?.kind)) {
        warnings.add(`${where}：kind（${String(sg?.kind)}）應為 ${SEGMENT_KINDS.join('／')}，略過`);
        return;
      }
      const pd = parseDays(sg.days);
      if (pd.error) {
        warnings.add(`${where}：${pd.error}，略過`);
        return;
      }
      if (!pd.days.includes(wd)) return;
      if (sg.effective !== undefined) {
        // 字串比較只對 YYYY-MM-DD 成立：'2026-12-6' 會被當成比 '2026-12-10' 晚，靜默延到隔年才生效
        if (!isRealDate(sg.effective)) {
          warnings.add(`${where}：effective（${String(sg.effective)}）應為 YYYY-MM-DD 的真實日期，略過`);
          return;
        }
        if (day < sg.effective) return; // 已公告未生效
      }
      if (sg.skip !== undefined) {
        const k = SKIP_RE.exec(String(sg.skip));
        if (!k) warnings.add(`${where}：skip（${String(sg.skip)}）應為 "nth:週次:星期"（例 nth:3:3＝第三個週三），已忽略`);
        else if (wd === +k[2] && Math.ceil(d / 7) === +k[1]) return;
      }
      const a = parseHm(sg.start);
      const b = parseHm(sg.end);
      if (!a || !b) {
        warnings.add(`${where}：start／end（${String(sg.start)}–${String(sg.end)}）應為 HH:MM，略過`);
        return;
      }
      const t0 = zonedToUtc(mk.tz, y, mo, d, a.h, a.m);
      const crosses = b.h * 60 + b.m <= a.h * 60 + a.m;
      let t1 = zonedToUtc(mk.tz, y, mo, d + (crosses ? 1 : 0), b.h, b.m);
      if (halfClose !== null && sg.kind !== 'night') {
        if (t0 >= halfClose) return;
        t1 = Math.min(t1, halfClose);
      }
      dayOut.push({ kind: sg.kind, label: sg.label ?? '', start: sg.start, end: sg.end, day, t0, t1, isOpen: false, isClose: false });
    });

    const regular = dayOut.filter((s) => s.kind === 'regular').sort((p, q) => p.t0 - q.t0);
    if (regular.length) regular[0].isOpen = true;
    const cash = dayOut.filter((s) => s.kind === 'regular' || s.kind === 'auction');
    if (cash.length) cash.reduce((p, q) => (q.t1 > p.t1 ? q : p)).isClose = true;
    out.push(...dayOut);
  }
  return out.sort((p, q) => p.t0 - q.t0);
}

/**
 * 一個市場在「現在」的完整狀態（規則 5）：展開的時段（加上相對現在的小時 `h0`／`h1`、
 * `active`、`past`）、狀態、狀態文字、是否變暗。
 */
export function marketState(mk, nowMs, config, warnings = new Set()) {
  const segments = expandMarket(mk, nowMs, config, warnings).map((s) => ({
    ...s,
    h0: (s.t0 - nowMs) / 3600000,
    h1: (s.t1 - nowMs) / 3600000,
    active: s.t0 <= nowMs && s.t1 > nowMs,
    past: s.t1 <= nowMs,
  }));
  const p = tzParts(mk.tz, nowMs);
  const today = holidayOn(config, mk.id, `${p.y}-${pad2(p.mo)}-${pad2(p.d)}`, warnings);
  let status = 'idle';
  let holiday = null;
  if (today.status === 'closed') {
    status = 'closed';
    holiday = { name: today.name };
  } else {
    for (const s of segments) {
      if (s.active && (status === 'idle' || STATUS_RANK[s.kind] > STATUS_RANK[status])) status = s.kind;
    }
  }
  return {
    id: mk.id,
    name: mk.name,
    color: mk.color,
    tz: mk.tz,
    segments,
    status,
    statusText: status === 'closed' ? `休市（${holiday.name}）` : STATUS_TEXT[status],
    holiday,
    dimmed: status === 'closed',
  };
}

/** 「接下來」：嚴格晚於現在、`horizonHours` 內的開收盤，依時刻排序取前 `max` 筆（同時刻依市場順序）。 */
export function upcomingEvents(markets, nowMs, { max = UPCOMING_MAX, horizonHours = UPCOMING_HORIZON_HOURS } = {}) {
  const horizon = nowMs + horizonHours * 3600000;
  const events = [];
  markets.forEach((m, order) => {
    for (const s of m.segments) {
      if (s.isOpen && s.t0 > nowMs && s.t0 < horizon) events.push({ t: s.t0, order, marketId: m.id, name: m.name, color: m.color, what: '開盤' });
      if (s.isClose && s.t1 > nowMs && s.t1 < horizon) events.push({ t: s.t1, order, marketId: m.id, name: m.name, color: m.color, what: '收盤' });
    }
  });
  events.sort((a, b) => a.t - b.t || a.order - b.order);
  return events.slice(0, max);
}

/**
 * 星盤的完整資料模型（頁面只負責畫）。`rawNowMs` 向下取整到 15 分鐘；`tz`＝顯示時區。
 * @returns {{ nowMs: number, nowText: string, tz: string, tzText: string,
 *             markets: ReturnType<typeof marketState>[],
 *             upcoming: Array<{ t: number, marketId: string, name: string, color: string, what: string, timeText: string }>,
 *             warnings: string[] }}
 */
export function computeAstrolabe(config, rawNowMs, tz) {
  const nowMs = floorToStep(rawNowMs, STEP_MS);
  const warnings = new Set();
  const list = Array.isArray(config?.markets) ? config.markets : [];
  if (!Array.isArray(config?.markets)) warnings.add('config.markets 不是陣列，星盤沒有任何市場');
  // 壞的市場元素（null、缺 id、tz 不是 Intl 認得的時區）略過並記警告，其餘市場照畫（task 4.1，N3：
  // 宿主只做頂層合併、mergeConfig 對陣列整份取代，這些元素可能原樣到這裡，tzParts 會拋錯）。
  const usable = list.filter((mk, i) => {
    if (!mk || typeof mk !== 'object' || typeof mk.id !== 'string' || mk.id === '') {
      warnings.add(`config.markets[${i}] 不是含 id 的物件，星盤略過`);
      return false;
    }
    if (!isValidTz(mk.tz)) {
      warnings.add(`config.markets[${i}]（${mk.id}）的 tz（${String(mk.tz)}）不是 Intl 認得的 IANA 時區，星盤略過`);
      return false;
    }
    return true;
  });
  const markets = usable.map((mk) => marketState(mk, nowMs, config, warnings));
  const upcoming = upcomingEvents(markets, nowMs).map((e) => ({ ...e, timeText: formatHm(e.t, tz) }));
  return {
    nowMs,
    nowText: formatHm(nowMs, tz),
    tz,
    tzText: formatUtcOffset(tzOffsetMin(tz, nowMs)),
    markets,
    upcoming,
    warnings: [...warnings],
  };
}
