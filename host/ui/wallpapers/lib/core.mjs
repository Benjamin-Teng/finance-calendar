// host/ui/wallpapers/lib/core.mjs
//
// 繪圖頁共用模組的「純邏輯」部分（task 3.1，design.md D2／D8）。ES module、不依賴 DOM，
// 可直接在 Node 內建 test runner 中 import（host/tests/wallpapers/core.test.mjs）。
// DOM 相關（字型載入、通道／fixture 資料、畫完訊號）在同目錄 `wallpaper.mjs`。
//
// ## 契約（3.3–3.6 主題頁與 4.5 宿主渲染管線都建立在這上面，改動要同步它們）
//
// 1. 尺寸基準：`S = sizeScale(W, H) = min(W, H) / 2160`。一切線寬、字級、間距都乘 S；
//    只看畫布像素，不看 devicePixelRatio、不看 Windows 縮放比例（規格「縮放比例不影響構圖」）。
// 2. query 參數（`parseQuery`）：
//      w, h      輸出像素，整數 16..16384；缺漏或不合法 → 3840×2160，並記入 warnings
//      t         模擬「現在」，ISO 8601。帶時區（Z 或 ±hh:mm）＝絕對時刻；
//                不帶時區（例 2026-10-02T21:00）＝解讀成 `tz` 當地時間（與機器時區無關，可重現）
//      tz        IANA 時區（顯示用）；缺漏或不合法 → 系統時區
//      fixture   資料 JSON 的 URL（形狀見第 7 點），相對頁面網址。宿主內缺漏 → 走宿主通道
//                `wallpaper`；獨立模式（無 Tauri）缺漏 → 讀 host/ui/fixtures/tw-events.json。
//                讀不到、不是 JSON、不是物件 → 整張失敗（phase 'error'），不會悄悄當成「沒資料」。
//      rid       宿主的 render id（task 4.5）：原樣放進回報宿主的 meta.rid，宿主據此丟棄逾時後才晚到的
//                PNG。頁面不解讀它；缺漏（獨立模式）＝null。
//    只認這六個參數；其他參數（例如 `now=`、`fixtures=`）記入 warnings，不會被默默忽略。
//    宿主（4.5）載入頁面時帶 w、h、t（絕對 ISO 含 Z）、tz、rid（沒有 `now`）。
//    注意：bridge.js 的 `fixtures=`（複數、目錄）是小工具頁的參數，桌布頁不使用。
// 3. 決定性亂數：`mulberry32(seed)` 回傳 () => [0,1)；同 seed 同序列。
// 4. 時區：`tzParts`／`tzOffsetMin`／`zonedToUtc` 全用 `Intl`，夏令時間自動處理，Rust 端不管時區。
// 5. 字型清單 `FONT_REQUIREMENTS`：頁面繪製前必須全部載入成功（見 wallpaper.mjs `loadFonts`）。
// 7. 通道 payload 形狀（design.md D7）：`{ data, config }`
//      data     從 tw_events 擷取的鍵（存在才帶）：twii_intraday、twii_daily、margin、holidays、events、
//               wallpaper_errors（宿主 `data.rs` 的 `WALLPAPER_DATA_KEYS`，task 4.6；不含 quotes 等其他鍵）
//      config   主題設定（wallpaper-config.json 的內容：時段表、休市表、詞庫、門檻…）
//    頁面以 `env.data`、`env.config` 取用；`config` 缺鍵時頁面用自己的內建預設，合併用
//    `mergeConfig(defaults, config)`（缺鍵補預設、型別錯補預設並回傳警告；不認得的鍵原樣保留）。
//    config 的鍵與內容由 task 3.2 的預設檔定義，本模組只定義信封與合併掛點。
//    為了方便，fixture 也可以是「裸的 tw_events JSON」，視為 `{ data: <它>, config: {} }`
//    （`normalizePayload`）。判別規則：頂層有物件型的 `data` 鍵＝信封，否則＝裸 tw_events。
// 6. 版面檢查 `checkLayout(boxes, W, H)`：規格要求「所有文字元素邊界框落在畫面內且兩兩不相交」，
//    主題頁把每個文字元素的邊界框 push 進 `env.boxes`，截圖腳本會用這個函式斷言。

/** 預設輸出尺寸（樣稿預設值）。 */
export const DEFAULT_W = 3840;
export const DEFAULT_H = 2160;
/** 單邊像素的合理範圍；超出視為不合法（防 `w=1e9` 這類輸入把渲染視窗撐爆）。 */
export const MIN_DIM = 16;
export const MAX_DIM = 16384;

/** 尺寸基準：一切線寬字級以短邊等比例（短邊 2160 → 1）。 */
export function sizeScale(w, h) {
  return Math.min(w, h) / 2160;
}

/** mulberry32 決定性亂數（樣稿 `bg-sessions.html` 同款）。回傳 () => [0,1) 的函式。 */
export function mulberry32(seed) {
  let a = seed | 0;
  return function () {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

// ── 時區 ─────────────────────────────────────────────────────────────────────────────

const fmtCache = new Map();

/** `tz` 是否為 `Intl` 認得的 IANA 時區。 */
export function isValidTz(tz) {
  if (typeof tz !== 'string' || tz === '') {
    return false;
  }
  try {
    new Intl.DateTimeFormat('en-US', { timeZone: tz });
    return true;
  } catch {
    return false;
  }
}

/** 系統時區（IANA 名稱）。 */
export function systemTz() {
  return Intl.DateTimeFormat().resolvedOptions().timeZone;
}

/** UTC 毫秒 → 某時區的當地年月日時分秒（`mo` 為 1–12）。 */
export function tzParts(tz, ms) {
  let f = fmtCache.get(tz);
  if (!f) {
    f = new Intl.DateTimeFormat('en-US', {
      timeZone: tz,
      hourCycle: 'h23',
      year: 'numeric',
      month: '2-digit',
      day: '2-digit',
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
    });
    fmtCache.set(tz, f);
  }
  const o = {};
  for (const p of f.formatToParts(new Date(ms))) {
    o[p.type] = p.value;
  }
  return { y: +o.year, mo: +o.month, d: +o.day, h: +o.hour, mi: +o.minute, s: +o.second };
}

/** 某時刻該時區相對 UTC 的偏移（分鐘，東為正；台北 +480）。 */
export function tzOffsetMin(tz, ms) {
  const p = tzParts(tz, ms);
  const asUTC = Date.UTC(p.y, p.mo - 1, p.d, p.h, p.mi, p.s);
  return Math.round((asUTC - Math.floor(ms / 1000) * 1000) / 60000);
}

/** 某時區的當地日期＋時刻 → UTC 毫秒（夏令時間由 Intl 處理；`ss` 預設 0）。 */
export function zonedToUtc(tz, y, mo, d, hh, mi, ss = 0) {
  const guess = Date.UTC(y, mo - 1, d, hh, mi, ss);
  const off = tzOffsetMin(tz, guess);
  let t = guess - off * 60000;
  const off2 = tzOffsetMin(tz, t);
  if (off2 !== off) {
    t = guess - off2 * 60000;
  }
  return t;
}

/** 毫秒無條件捨去到 `stepMs` 的整數倍（星盤每 15 分鐘一格＝`floorToStep(ms, 900000)`）。 */
export function floorToStep(ms, stepMs) {
  return Math.floor(ms / stepMs) * stepMs;
}

// ── query 解析 ───────────────────────────────────────────────────────────────────────

// 帶時區的完整 ISO 時刻（日期＋T＋時分[秒]＋Z 或 ±hh[:mm]）；純日期「2026-10-02」結尾的 -02
// 不能被當成時區偏移，所以一定要有時間部分。
/** 頁面認得的 query 參數；其他一律警告。 */
export const KNOWN_PARAMS = ['w', 'h', 't', 'tz', 'fixture', 'rid'];

/** query 的 render id（`rid`）；沒帶或解析失敗＝null。不拋錯：回報宿主時一定要拿得到它。 */
export function readRenderId(search) {
  try {
    return new URLSearchParams(search || '').get('rid') || null;
  } catch {
    return null;
  }
}

const HAS_OFFSET = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}(?::\d{2}(?:\.\d+)?)?(?:Z|[+-]\d{2}:\d{2})$/i;
const NAIVE_ISO = /^(\d{4})-(\d{2})-(\d{2})(?:[T ](\d{2}):(\d{2})(?::(\d{2})(?:\.\d+)?)?)?$/;

function parseDim(raw, fallback, name, warnings) {
  if (raw == null || raw === '') {
    return fallback;
  }
  if (!/^\d+$/.test(raw)) {
    warnings.push(`${name}=${raw} 不是整數，改用 ${fallback}`);
    return fallback;
  }
  const v = Number(raw);
  if (v < MIN_DIM || v > MAX_DIM) {
    warnings.push(`${name}=${raw} 超出 ${MIN_DIM}..${MAX_DIM}，改用 ${fallback}`);
    return fallback;
  }
  return v;
}

/** `t` 字串 → UTC 毫秒；不合法回傳 NaN。不帶時區者解讀成 `tz` 當地時間。 */
export function parseInstant(raw, tz) {
  if (HAS_OFFSET.test(raw)) {
    return Date.parse(raw);
  }
  const m = NAIVE_ISO.exec(raw);
  if (!m) {
    return NaN;
  }
  const [y, mo, d, hh, mi, ss] = [m[1], m[2], m[3], m[4] ?? 0, m[5] ?? 0, m[6] ?? 0].map(Number);
  // 欄位合法性：用 UTC 往返確認（擋掉 2026-13-45、25:00 這類 Date.UTC 會自動進位的輸入）
  const chk = new Date(Date.UTC(y, mo - 1, d, hh, mi, ss));
  if (
    chk.getUTCFullYear() !== y ||
    chk.getUTCMonth() !== mo - 1 ||
    chk.getUTCDate() !== d ||
    chk.getUTCHours() !== hh ||
    chk.getUTCMinutes() !== mi ||
    chk.getUTCSeconds() !== ss
  ) {
    return NaN;
  }
  return zonedToUtc(tz, y, mo, d, hh, mi, ss);
}

/**
 * 解析頁面 query（`location.search`）。不合法的值不拋錯，改用預設並記入 `warnings`
 * （宿主渲染不能因為一個壞參數就整張圖失敗；測試與截圖稿會檢查 warnings 為空）。
 *
 * @param {string} search `?w=1920&h=1080&t=...`
 * @param {{ nowMs?: number, localTz?: string }} [opts] 注入「現在」與系統時區（測試用）
 * @returns {{ w: number, h: number, nowMs: number, tz: string, fixture: string|null,
 *             rid: string|null, hasT: boolean, warnings: string[] }}
 */
export function parseQuery(search, opts = {}) {
  const nowMs = opts.nowMs ?? Date.now();
  const localTz = opts.localTz ?? systemTz();
  const qs = new URLSearchParams(search || '');
  const warnings = [];

  const w = parseDim(qs.get('w'), DEFAULT_W, 'w', warnings);
  const h = parseDim(qs.get('h'), DEFAULT_H, 'h', warnings);

  let tz = localTz;
  const rawTz = qs.get('tz');
  if (rawTz) {
    if (isValidTz(rawTz)) {
      tz = rawTz;
    } else {
      warnings.push(`tz=${rawTz} 不是有效的 IANA 時區，改用 ${localTz}`);
    }
  }

  let now = nowMs;
  let hasT = false;
  const rawT = qs.get('t');
  if (rawT) {
    const v = parseInstant(rawT, tz);
    if (Number.isFinite(v)) {
      now = v;
      hasT = true;
    } else {
      warnings.push(`t=${rawT} 不是有效的 ISO 時間，改用現在`);
    }
  }

  const fixture = qs.get('fixture') || null;
  const rid = readRenderId(search);

  for (const key of new Set(qs.keys())) {
    if (KNOWN_PARAMS.includes(key)) {
      continue;
    }
    let hint = `支援的參數：${KNOWN_PARAMS.join('、')}`;
    if (key === 'now') {
      hint = '「現在」請用 t（ISO 8601，絕對時刻帶 Z）';
    } else if (key === 'fixtures') {
      hint = '是否要用 fixture（單一資料檔網址）？桌布頁不使用 bridge.js 的 fixtures（目錄）';
    }
    warnings.push(`未知參數 ${key}=${qs.get(key)}，已忽略；${hint}`);
  }
  return { w, h, nowMs: now, tz, fixture, rid, hasT, warnings };
}

// ── 字型清單 ─────────────────────────────────────────────────────────────────────────

/**
 * 頁面繪製前必須載入成功的字型（樣稿實際用到的字重；全部 OFL，檔案在 `../fonts/`，
 * `@font-face` 在 `../fonts/fonts.css`）。變動字型一個檔涵蓋多個字重。
 * 頁面可只載入自己用到的子集（`runWallpaper({ fonts })`，例：星盤的 `ASTROLABE_FONTS`）；
 * Noto Sans TC 500／600 是星盤的中文字（樣稿字型堆疊裡的系統字型微軟正黑體不是內建字型，task 3.3 改用堆疊中的 Noto Sans TC）。
 * `file` 供測試核對檔案存在；`weight` 是 canvas `ctx.font` 實際會用的字重。
 */
export const FONT_REQUIREMENTS = [
  { id: 'cinzel-500', family: 'Cinzel', weight: 500, file: 'Cinzel-VF.ttf' },
  { id: 'cinzel-600', family: 'Cinzel', weight: 600, file: 'Cinzel-VF.ttf' },
  { id: 'cinzel-700', family: 'Cinzel', weight: 700, file: 'Cinzel-VF.ttf' },
  { id: 'wenkai-400', family: 'LXGW WenKai TC', weight: 400, file: 'LXGWWenKaiTC-Regular.ttf' },
  { id: 'wenkai-700', family: 'LXGW WenKai TC', weight: 700, file: 'LXGWWenKaiTC-Bold.ttf' },
  { id: 'notoserif-900', family: 'Noto Serif TC', weight: 900, file: 'NotoSerifTC-VF.ttf' },
  { id: 'notosans-500', family: 'Noto Sans TC', weight: 500, file: 'NotoSansTC-VF.ttf' },
  { id: 'notosans-600', family: 'Noto Sans TC', weight: 600, file: 'NotoSansTC-VF.ttf' },
  { id: 'plexmono-600', family: 'IBM Plex Mono', weight: 600, file: 'IBMPlexMono-SemiBold.ttf' },
];

/** `document.fonts.load()`／canvas `ctx.font` 用的 CSS font 字串。 */
export function cssFont(req, px = 32) {
  return `${req.weight} ${px}px "${req.family}"`;
}

// ── 版面檢查 ─────────────────────────────────────────────────────────────────────────

/**
 * 量測一段文字在目前 `ctx.font`／`textAlign`／`textBaseline` 下畫在 (x, y) 的實際邊界框，
 * 回傳 `{ label, x, y, w, h }`，可直接 push 進 `env.boxes`。用 `actualBoundingBox*`
 * （字形實際墨跡範圍，不是字級估計值），所以對齊方式與基線設定都已反映在結果裡。
 * 呼叫端仍須自己用同一組 x、y 去 `fillText`。
 */
export function textBox(ctx, label, text, x, y) {
  const m = ctx.measureText(text);
  return {
    label,
    x: x - m.actualBoundingBoxLeft,
    y: y - m.actualBoundingBoxAscent,
    w: m.actualBoundingBoxLeft + m.actualBoundingBoxRight,
    h: m.actualBoundingBoxAscent + m.actualBoundingBoxDescent,
  };
}

/**
 * 規格「任意解析度與縮放正確等比」：所有文字元素邊界框落在畫面內且兩兩不相交。
 * 邊界框 `{ label, x, y, w, h }`（左上角＋寬高，像素）；只共用邊或角（面積為 0）不算相交。
 *
 * @returns {{ ok: boolean, outside: string[], overlaps: Array<[string, string]> }}
 */
export function checkLayout(boxes, W, H) {
  const outside = [];
  const overlaps = [];
  for (const b of boxes) {
    if (b.x < 0 || b.y < 0 || b.x + b.w > W || b.y + b.h > H) {
      outside.push(b.label);
    }
  }
  for (let i = 0; i < boxes.length; i++) {
    for (let j = i + 1; j < boxes.length; j++) {
      const a = boxes[i];
      const b = boxes[j];
      if (a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h) {
        overlaps.push([a.label, b.label]);
      }
    }
  }
  return { ok: outside.length === 0 && overlaps.length === 0, outside, overlaps };
}

// ── 通道 payload 與主題設定 ──────────────────────────────────────────────────────────────

const isPlain = (v) => v !== null && typeof v === 'object' && !Array.isArray(v);

/**
 * 通道 payload → `{ data, config, shape, warnings }`。信封 `{ data, config }` 原樣取出；
 * 裸 tw_events JSON 視為 `{ data: <它>, config: {} }`。不是物件（null、陣列、字串…）→ 拋錯
 * （明確失敗，不退化成空資料）。`config` 不是物件時改用 `{}` 並記入警告。
 */
export function normalizePayload(raw) {
  if (!isPlain(raw)) {
    throw new Error(`資料 payload 必須是 JSON 物件，實際為 ${raw === null ? 'null' : Array.isArray(raw) ? '陣列' : typeof raw}`);
  }
  const warnings = [];
  if (isPlain(raw.data)) {
    let config = {};
    if (raw.config !== undefined) {
      if (isPlain(raw.config)) {
        config = raw.config;
      } else {
        warnings.push('payload.config 不是物件，改用空設定');
      }
    }
    return { data: raw.data, config, shape: 'envelope', warnings };
  }
  return { data: raw, config: {}, shape: 'bare', warnings };
}

function cloneJson(v) {
  return v === undefined ? undefined : JSON.parse(JSON.stringify(v));
}

function mergeNode(def, cfg, path, warnings) {
  const at = path ? `config.${path}` : 'config';
  if (cfg === undefined) {
    return cloneJson(def);
  }
  if (isPlain(def)) {
    if (!isPlain(cfg)) {
      warnings.push(`${at} 型別應為物件，改用預設`);
      return cloneJson(def);
    }
    const out = {};
    for (const k of Object.keys(def)) {
      out[k] = mergeNode(def[k], cfg[k], path ? `${path}.${k}` : k, warnings);
    }
    for (const k of Object.keys(cfg)) {
      if (!(k in def)) {
        out[k] = cloneJson(cfg[k]);
      }
    }
    return out;
  }
  const kind = (v) => (Array.isArray(v) ? 'array' : v === null ? 'null' : typeof v);
  if (kind(def) !== kind(cfg)) {
    warnings.push(`${at} 型別應為 ${kind(def)}，實際為 ${kind(cfg)}，改用預設`);
    return cloneJson(def);
  }
  return cloneJson(cfg);
}

/**
 * 主題設定合併掛點：頁面把自己的內建預設與通道送來的 `config` 合併。逐鍵處理：
 * 缺鍵 → 預設；型別不符 → 預設＋警告；物件遞迴；陣列與純量整個取代（不逐項合併）；
 * 預設裡沒有的鍵原樣保留。回傳 `{ value, warnings }`（深拷貝，不改動輸入）。
 */
export function mergeConfig(defaults, config) {
  const warnings = [];
  const value = mergeNode(defaults, config ?? {}, '', warnings);
  return { value, warnings };
}
