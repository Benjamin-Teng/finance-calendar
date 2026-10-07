// host/ui/wallpapers/lib/tw-trading-days.mjs
//
// 台股交易日與「資料落後幾個交易日」的共用判定（task 3.5 由撕日曆 3.4 的實作抽出；脊線、等高線、
// 3.6 天際線共用）。ES module、不依賴 DOM，可在 Node 測試（host/tests/wallpapers/tw-trading-days.test.mjs）。
//
// ## 規則
//   休市日   週末、`config.holidays.TPE[年].days`（status 'closed'；半日市不算）、`data.holidays`（TWSE 休市日曆）
//            三者之一（與撕日曆相同，`tpeClosed`）。日期是 YYYY-MM-DD 的台股當地（台北）日期。
//   已收盤   交易日台北時間 13:30（含）起算「已收盤」；13:30 前最近一個已收盤交易日是前一個交易日。
//            一律以台北時間判定，與顯示時區、機器時區無關。
//   落後數   從資料日期（不含）到最近一個已收盤交易日（含）之間的交易日數；資料日期不早於它＝0。
//   過期標示 落後 ≥ STALE_MIN_BEHIND（3）才顯示「資料停在 M/D」（M/D 不補零）；剛好落後 2 不顯示
//            （spec「資料過期標示」）。
//
// ## 函式
//   addDays(date, n)                               純日曆加減天數
//   tpeClosed(date, config, data)                  → { closed, reason: 'weekend'|'config'|'data'|null, name? }
//   isTpeTradingDay(date, config, data)            → boolean
//   lastClosedTradingDay(nowMs, config, data)      → 'YYYY-MM-DD'｜null（往回找 LOOKBACK_DAYS 天都找不到）
//   tradingDaysBehind(dataDate, nowMs, config, data) → 整數｜null（資料日期不合法或找不到已收盤交易日）
//   staleMarker(dataDate, nowMs, config, data)     → { dataDate, lastClosed, behind, show, text }
//   mdText(date)                                   → 'M/D'

import { tzParts } from './core.mjs';
import { holidayOn, isRealDate } from './config-holidays.mjs';

/** 落後幾個交易日起顯示過期標示（spec：落後 2 不顯示、3 才顯示）。 */
export const STALE_MIN_BEHIND = 3;
/** 台股收盤（台北時間，分鐘）。 */
export const TPE_CLOSE_MIN = 13 * 60 + 30;
/** 找「最近一個已收盤交易日」最多往回找幾天（防設定把每天都標成休市時無窮迴圈）。 */
export const LOOKBACK_DAYS = 400;

const isPlain = (v) => v !== null && typeof v === 'object' && !Array.isArray(v);
const toUtc = (date) => Date.parse(`${date}T00:00:00Z`);
const pad = (n) => String(n).padStart(2, '0');

/** 日期加減天數（純日曆運算，不涉時區）。 */
export function addDays(date, n) {
  return new Date(toUtc(date) + n * 86400000).toISOString().slice(0, 10);
}

/** 0＝週日 … 6＝週六。 */
export function dow(date) {
  return new Date(toUtc(date)).getUTCDay();
}

/**
 * 台股是否休市（台股當地日期）。回傳 `{ closed, reason }`，reason＝'weekend'｜'config'｜'data'；
 * config 命中時另帶假日名 `name`。半日市（config `half: true`）不算休市。
 */
export function tpeClosed(date, config, data) {
  const w = dow(date);
  if (w === 0 || w === 6) return { closed: true, reason: 'weekend' };
  const h = holidayOn(config, 'TPE', date);
  if (h.status === 'closed') return { closed: true, reason: 'config', name: h.name };
  const list = isPlain(data) && Array.isArray(data.holidays) ? data.holidays : [];
  if (list.includes(date)) return { closed: true, reason: 'data' };
  return { closed: false, reason: null };
}

/** 是否為台股交易日。 */
export function isTpeTradingDay(date, config, data) {
  return !tpeClosed(date, config, data).closed;
}

/** 最近一個已收盤交易日（台北時間 13:30 為界）。往回 LOOKBACK_DAYS 天都找不到回傳 null。 */
export function lastClosedTradingDay(nowMs, config, data) {
  const p = tzParts('Asia/Taipei', nowMs);
  const today = `${p.y}-${pad(p.mo)}-${pad(p.d)}`;
  let d = p.h * 60 + p.mi >= TPE_CLOSE_MIN ? today : addDays(today, -1);
  for (let i = 0; i < LOOKBACK_DAYS; i++, d = addDays(d, -1)) {
    if (isTpeTradingDay(d, config, data)) return d;
  }
  return null;
}

/**
 * 資料落後幾個交易日：資料日期（不含）到最近一個已收盤交易日（含）之間的交易日數。
 * 資料日期不合法、或找不到已收盤交易日 → null。資料日期不早於最近一個已收盤交易日 → 0。
 */
export function tradingDaysBehind(dataDate, nowMs, config, data) {
  if (!isRealDate(dataDate)) return null;
  const last = lastClosedTradingDay(nowMs, config, data);
  if (last === null) return null;
  let n = 0;
  for (let d = addDays(dataDate, 1); d <= last; d = addDays(d, 1)) {
    if (isTpeTradingDay(d, config, data)) n++;
  }
  return n;
}

/** 'YYYY-MM-DD' → 'M/D'（不補零）。 */
export function mdText(date) {
  return `${Number(date.slice(5, 7))}/${Number(date.slice(8, 10))}`;
}

/**
 * 過期標示（spec「資料過期標示」）。`show`＝落後 ≥ STALE_MIN_BEHIND；`text`＝「資料停在 M/D」或 null。
 * 資料日期不合法時 behind 為 null、不顯示（不拋錯；呼叫端自己決定要不要失敗）。
 */
export function staleMarker(dataDate, nowMs, config, data) {
  const lastClosed = lastClosedTradingDay(nowMs, config, data);
  const behind = tradingDaysBehind(dataDate, nowMs, config, data);
  const show = behind !== null && behind >= STALE_MIN_BEHIND;
  return { dataDate, lastClosed, behind, show, text: show ? `資料停在 ${mdText(dataDate)}` : null };
}
