// host/ui/wallpapers/lib/tearoff-model.mjs
//
// 撕日曆（task 3.4）的純邏輯：本機日期、星期、中文月份、干支年、「宜」「忌」規則、日期雜湊選句、
// 單行縮字、旋轉後的邊界框。ES module、不依賴 DOM，可在 Node 測試
// （host/tests/wallpapers/tearoff-model.test.mjs）。繪圖在 `tearoff-draw.mjs`。
//
// ## 規則（spec「撕日曆的日期與『宜』」「撕日曆的『忌』」＋ task 3.4 controller rulings）
//   日期    一律用「本機日期」（顯示時區 env.tz 的當地日期，`localDate`），休市判定也用它。
//   休市日  週末、`config.holidays.TPE[年].days`（status 'closed'；半日市不算）、`data.holidays`（TWSE 休市日曆）三者之一。
//   宜      休市日：只有 `phrases.closedDay.yi`。
//           交易日：依序 台積電財報（data.events 中 code 2330、type earnings／conference、date＝當天）
//           → 財報密集週（earningsWeek：截止日 −(leadDays−1) … 截止日）→ 旺季（orderSeasons 每筆各算一項），
//           最多 MAX_YI 項；都沒有時從 `phrases.yiPool` 依日期取一句。
//   忌      休市日：`phrases.closedDay.ji`（每元素一行）。
//           交易日：data.margin 的 short_margin_ratio < thresholds.shortRatio，或 maintenance_ratio < thresholds.maintenanceRatio
//           （嚴格小於；值不是有限數就不比）→ `phrases.marginWarning.ji`；否則從 `phrases.jiPool` 依日期取一句。
//           融資資料過期（`data.margin.date` 落後最近一個已收盤交易日 ≥ STALE_MIN_BEHIND 個交易日，與「資料過期
//           標示」同一規則；task 6.4，審查 R3-low）視同缺資料：不據以產生「忌」（本來會觸發警示時記警告）。日期缺漏或不合法時
//           判斷不了，照舊使用。「現在」＝`opts.nowMs`（頁面傳 env.nowMs），省略時取 `date` 當天台北 23:59。
//   同日同句  `pickPhrase(pool, date, salt)`＝`pool[dateHash(salt|date) % pool.length]`；宜與忌用不同 salt。
//           詞庫變動（增刪句子）後選句可能跟著變，屬預期。
//   干支年  以農曆正月初一為界：設定檔 `lunarNewYear.dates` 有該年時用它（官方日期，中央氣象署日曆資料表），
//           否則用 `Intl.DateTimeFormat('zh-TW-u-ca-chinese')`；干支名稱一律取 Intl 的 yearName，不寫死對照表。
//           Node 與 headless Edge 的輸出由 tearoff-model.test.mjs 與測試頁 tearoff-ganzhi-edge.html 各自驗證。
//   縮字    每項單行：超過可用寬度就逐 1px 縮到放得下，沒有硬下限（不截斷、不換行、不超出頁面）；
//           低於可讀下限（呼叫端給，撕日曆＝可用寬度 ÷ 24）時回報 belowReadable，呼叫端記警告。

import { tzParts } from './core.mjs';
import { isRealDate, lunarNewYearOf } from './config-holidays.mjs';
import { addDays, dow, STALE_MIN_BEHIND, tpeClosed, tradingDaysBehind } from './tw-trading-days.mjs';

// 休市判定與日期加減在 task 3.5 抽到共用模組 tw-trading-days.mjs（脊線、等高線、天際線也用），這裡原樣轉出。
export { addDays, tpeClosed };

export const YI_SALT = 'yi';
export const JI_SALT = 'ji';
/** 交易日「宜」最多幾項。 */
export const MAX_YI = 2;

const pad = (n) => String(n).padStart(2, '0');
const isPlain = (v) => v !== null && typeof v === 'object' && !Array.isArray(v);
const toUtc = (date) => Date.parse(`${date}T00:00:00Z`);

/** UTC 毫秒 → 該時區的當地日期 YYYY-MM-DD。 */
export function localDate(tz, nowMs) {
  const p = tzParts(tz, nowMs);
  return `${p.y}-${pad(p.mo)}-${pad(p.d)}`;
}

/** 「週二」。 */
export function weekdayText(date) {
  return `週${'日一二三四五六'[dow(date)]}`;
}

const MONTH_NAMES = ['一', '二', '三', '四', '五', '六', '七', '八', '九', '十', '十一', '十二'];

/** 「七月」「十一月」。 */
export function monthText(date) {
  return `${MONTH_NAMES[Number(date.slice(5, 7)) - 1]}月`;
}

/** 日（不補零）：「28」「5」。 */
export function dayText(date) {
  return String(Number(date.slice(8, 10)));
}

let ganzhiFmt = null;
const GANZHI_RE = /[甲乙丙丁戊己庚辛壬癸][子丑寅卯辰巳午未申酉戌亥]/u;

/** Intl 中國曆在 `date`（YYYY-MM-DD，取 UTC 正午）的干支＋「年」；取不到回傳 null。 */
function intlGanzhi(date) {
  ganzhiFmt ??= new Intl.DateTimeFormat('zh-TW-u-ca-chinese', { year: 'numeric', timeZone: 'UTC' });
  const at = new Date(`${date}T12:00:00Z`);
  const part = ganzhiFmt.formatToParts(at).find((p) => p.type === 'yearName')?.value ?? '';
  const m = GANZHI_RE.exec(part) ?? GANZHI_RE.exec(ganzhiFmt.format(at));
  return m ? `${m[0]}年` : null;
}

/**
 * 農曆年的干支＋「年」（例「丙午年」），以農曆正月初一為界。
 * - 設定檔 `lunarNewYear.dates[西元年]` 有合理日期（`lunarNewYearOf`）時，以它判定換年：當天 ≥ 初一 → 該西元年的農曆年，
 *   否則 → 前一年；干支名稱仍由 Intl 取（取該農曆年 6 月 1 日，離換年日夠遠、ICU 不會算錯），不寫死干支對照表。
 * - 沒列的年度退回 Intl 中國曆（ICU）。ICU 的天文近似在朔接近午夜的年份會差一天（2027 初一算成 2/7，官方 2/6），
 *   所以已公布的年份應填入設定檔（task 3.4 修正輪 1）。
 * 都取不到回傳 null（呼叫端畫成無干支並記警告）。
 */
export function ganzhiYear(date, config) {
  const y = Number(date.slice(0, 4));
  const lny = lunarNewYearOf(config, y);
  if (lny) return intlGanzhi(`${date >= lny ? y : y - 1}-06-01`);
  return intlGanzhi(date);
}

/** FNV-1a 32 位元雜湊（無號整數）。 */
export function dateHash(str) {
  let h = 0x811c9dc5;
  for (const ch of String(str)) {
    const cp = ch.codePointAt(0);
    // 逐位元組（UTF-16 碼元拆兩個位元組也行；這裡用碼位的低高位元組，決定性即可）
    h ^= cp & 0xff;
    h = Math.imul(h, 0x01000193);
    h ^= (cp >>> 8) & 0xff;
    h = Math.imul(h, 0x01000193);
  }
  return h >>> 0;
}

/** 依日期從詞庫取一句（同一天同一句）；詞庫為空回傳 null。 */
export function pickPhrase(pool, date, salt) {
  if (!Array.isArray(pool) || pool.length === 0) return null;
  return pool[dateHash(`${salt}|${date}`) % pool.length];
}

/** 是否在財報密集週：任一截止日 D 使 D−(leadDays−1) ≤ date ≤ D（含截止日；跨年的截止日也算）。 */
export function inEarningsWeek(date, earningsWeek) {
  const lead = Number(earningsWeek?.leadDays);
  const deadlines = Array.isArray(earningsWeek?.deadlines) ? earningsWeek.deadlines : [];
  if (!Number.isFinite(lead) || lead < 1) return false;
  const y = Number(date.slice(0, 4));
  const t = toUtc(date);
  for (const md of deadlines) {
    for (const yy of [y, y + 1]) {
      const d = `${yy}-${md}`;
      if (!isRealDate(d)) continue;
      const diff = Math.round((toUtc(d) - t) / 86400000);
      if (diff >= 0 && diff <= lead - 1) return true;
    }
  }
  return false;
}

/** 當月命中的旺季標籤（依設定順序，全部回傳）。 */
export function orderSeasonLabels(date, orderSeasons) {
  const m = Number(date.slice(5, 7));
  return (Array.isArray(orderSeasons) ? orderSeasons : [])
    .filter((s) => isPlain(s) && Array.isArray(s.months) && s.months.includes(m) && typeof s.label === 'string' && s.label.trim() !== '')
    .map((s) => s.label);
}

/** 當天是否有台積電法說會（財報）：events 中 code 2330、type earnings／conference、date 為當天。缺漏一律 false。 */
export function tsmcEarningsOn(date, data) {
  const events = isPlain(data) && Array.isArray(data.events) ? data.events : [];
  return events.some(
    (e) => isPlain(e) && String(e.code) === '2330' && (e.type === 'earnings' || e.type === 'conference') && e.date === date,
  );
}

/** 融資警示：最新（輸出中那一份）券資比或維持率嚴格低於門檻。值不是有限數的那一項不比。 */
export function marginAlert(data, thresholds) {
  const m = isPlain(data) && isPlain(data.margin) ? data.margin : null;
  if (!m) return false;
  const below = (v, th) => typeof v === 'number' && Number.isFinite(v) && typeof th === 'number' && Number.isFinite(th) && v < th;
  return below(m.short_margin_ratio, thresholds?.shortRatio) || below(m.maintenance_ratio, thresholds?.maintenanceRatio);
}

/**
 * 融資資料是否過期（task 6.4，審查 R3-low）：`data.margin.date` 落後最近一個已收盤交易日 ≥ STALE_MIN_BEHIND
 * 個交易日（與「資料過期標示」`staleMarker` 同一規則）。過期回傳 `{ date, behind }`，否則（含沒有 margin、
 * 日期缺漏或不合法而判斷不了）回傳 null。
 */
export function marginStale(data, nowMs, config) {
  const m = isPlain(data) && isPlain(data.margin) ? data.margin : null;
  if (!m || typeof m.date !== 'string') return null;
  const behind = tradingDaysBehind(m.date, nowMs, config, data);
  return behind !== null && behind >= STALE_MIN_BEHIND ? { date: m.date, behind } : null;
}

const usable = (s) => typeof s === 'string' && s.trim() !== '';

/** 詞庫只留非空字串；清空時退回 fallback 的同名詞庫並記警告。 */
function poolOf(cfg, fallback, key, warnings) {
  const pool = (Array.isArray(cfg?.phrases?.[key]) ? cfg.phrases[key] : []).filter(usable);
  if (pool.length > 0) return pool;
  const fb = (Array.isArray(fallback?.phrases?.[key]) ? fallback.phrases[key] : []).filter(usable);
  warnings.push(`config.phrases.${key} 詞庫沒有可用的字句，改用內建預設詞庫`);
  return fb;
}

/**
 * 撕日曆的資料模型。`cfg`＝已與內建預設合併的主題設定；`data`＝tw_events 鍵（可為 null）；
 * `date`＝本機日期 YYYY-MM-DD。`opts.fallback`＝內建預設設定（詞庫被清空時退回用）；`opts.nowMs`＝現在
 * （UTC 毫秒；判斷融資資料是否過期用，省略時取 `date` 當天台北 23:59）。
 * @returns {{ date: string, ganzhi: string|null, weekday: string, month: string, day: string,
 *             closed: boolean, closedReason: string|null, yi: string[], ji: string[],
 *             yiFrom: 'closed'|'events'|'pool', jiFrom: 'closed'|'margin'|'pool', warnings: string[] }}
 */
export function computeTearoff(cfg, data, date, opts = {}) {
  const warnings = [];
  const fallback = opts.fallback ?? cfg;
  const ph = cfg?.phrases ?? {};
  const st = tpeClosed(date, cfg, data);
  const base = {
    date,
    ganzhi: ganzhiYear(date, cfg),
    weekday: weekdayText(date),
    month: monthText(date),
    day: dayText(date),
    closed: st.closed,
    closedReason: st.reason,
    warnings,
  };
  if (base.ganzhi === null) warnings.push(`無法取得 ${date} 的干支年（Intl 不支援農曆曆法），年欄只顯示星期`);

  if (st.closed) {
    const ji = (Array.isArray(ph.closedDay?.ji) ? ph.closedDay.ji : []).filter(usable);
    return { ...base, yi: [ph.closedDay?.yi].filter(usable), ji, yiFrom: 'closed', jiFrom: 'closed' };
  }

  const items = [];
  if (tsmcEarningsOn(date, data) && usable(ph.events?.tsmcEarnings)) items.push(ph.events.tsmcEarnings);
  if (inEarningsWeek(date, cfg?.earningsWeek) && usable(ph.events?.earningsWeek)) items.push(ph.events.earningsWeek);
  items.push(...orderSeasonLabels(date, cfg?.orderSeasons));

  let yi;
  let yiFrom;
  if (items.length > 0) {
    yi = items.slice(0, MAX_YI);
    yiFrom = 'events';
  } else {
    yi = [pickPhrase(poolOf(cfg, fallback, 'yiPool', warnings), date, YI_SALT)].filter(usable);
    yiFrom = 'pool';
  }

  const nowMs = Number.isFinite(opts.nowMs) ? opts.nowMs : Date.parse(`${date}T23:59:00+08:00`);
  const stale = marginStale(data, nowMs, cfg);
  const alert = marginAlert(data, cfg?.thresholds);
  if (stale && alert) {
    // 只在過期的數字本來會觸發警示時記（改變了結果）；沒觸發時忽略它與照用沒有差別。
    warnings.push(`融資資料停在 ${stale.date}（落後 ${stale.behind} 個交易日），視同缺資料，不據以產生「忌」`);
  }
  let ji;
  let jiFrom;
  if (!stale && alert && usable(ph.marginWarning?.ji)) {
    ji = [ph.marginWarning.ji];
    jiFrom = 'margin';
  } else {
    ji = [pickPhrase(poolOf(cfg, fallback, 'jiPool', warnings), date, JI_SALT)].filter(usable);
    jiFrom = 'pool';
  }
  return { ...base, yi, ji, yiFrom, jiFrom };
}

/**
 * 單行縮字：字級 `basePx` 量起來超過 `maxW` 時，逐次縮 1px 直到放得下（不截斷、不換行、不超出可用寬度）。
 * 沒有硬下限（task 3.4 修正輪 1）：縮到 1px 仍放不下時改用依寬度等比算出的小數字級。
 * 結果低於可讀下限 `readablePx` 時 `belowReadable: true`，呼叫端記警告（字仍在頁面內，只是可能難以閱讀）。
 * `measure(px)` 回傳該字級下的寬度。
 * @returns {{ px: number, fits: boolean, belowReadable: boolean }}
 */
export function fitFontPx({ measure, basePx, maxW, readablePx }) {
  const done = (px) => ({ px, fits: measure(px) <= maxW, belowReadable: px < Math.min(basePx, readablePx) });
  if (measure(basePx) <= maxW) return { px: basePx, fits: true, belowReadable: false };
  let px = Math.floor(basePx);
  if (px === basePx) px -= 1;
  for (; px >= 1; px--) {
    if (measure(px) <= maxW) return done(px);
  }
  // 1px 都放不下（極端長句）：寬度近似與字級成正比，依比例算小數字級，再保守地往下修到放得下
  let f = Math.floor((maxW / measure(1)) * 1000) / 1000;
  while (f > 0.001 && measure(f) > maxW) f = Math.floor(f * 0.95 * 1000) / 1000;
  return done(Math.max(f, 0.001));
}

/** 以 2D 變換矩陣（DOMMatrix 的 a–f）轉換邊界框四角，回傳軸對齊外接框（版面檢查用）。 */
export function transformedAabb(box, m) {
  const pts = [
    [box.x, box.y],
    [box.x + box.w, box.y],
    [box.x, box.y + box.h],
    [box.x + box.w, box.y + box.h],
  ].map(([x, y]) => [m.a * x + m.c * y + m.e, m.b * x + m.d * y + m.f]);
  const xs = pts.map((p) => p[0]);
  const ys = pts.map((p) => p[1]);
  const x = Math.min(...xs);
  const y = Math.min(...ys);
  return { label: box.label, x, y, w: Math.max(...xs) - x, h: Math.max(...ys) - y };
}
