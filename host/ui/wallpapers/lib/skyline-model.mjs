// host/ui/wallpapers/lib/skyline-model.mjs
//
// 天際線（task 3.6）的純函式：日 K → 20 根 K 線建築＋20MA 電線。ES module、不依賴 DOM，可在 Node 測試
// （host/tests/wallpapers/skyline-model.test.mjs）。資料形狀見 AGENTS.md「現況」資料層一節：
//   data.twii_daily ＝ [{ date, open, high, low, close }, …]，依日期由舊到新、只收已收盤日，資料層保證 ≥ 40 筆。
//
// ## 規則（task 3.6 controller rulings）
//   20MA     第 i 根＝含它在內往前 MA_PERIOD（20）筆收盤價（close）的算術平均。不足 20 筆＝null，**不用不足 20 筆的平均冒充**。
//   取窗     畫最後 CANDLE_COUNT（20）根；最右一根＝最新一筆（收盤後更新為當日），最左一根＝往前 19 個交易日。
//            20 根都要有完整 20MA 需要 NEED_ROWS（39）筆。
//   資料不足 20..38 筆：K 照畫 20 根，沒有完整 20MA 的那幾根 ma＝null（不畫），並回警告。
//            < 20 筆（連一根完整的 20MA 都沒有）→ 拋錯，頁面走 3.1 契約的錯誤路徑。
//   缺資料   twii_daily 不存在、不是陣列、為空、沒有任何可用的列 → 拋錯（不畫假資料）。
//            不合法的列（日期錯、價格不是正數、高低與開收矛盾）剔除並警告；亂序排序；同日重複取後者並警告。
//   正規化   價格範圍＝20 根的最高價、最低價與所有非 null 的 20MA。高低差小於中價 × MIN_RANGE_FRAC（0.1%，與盤中走勢
//            同一門檻）時視為平盤：改用這個最小範圍、以中價置中，不除以零；剛好等於門檻時兩種算法結果相同。
//   seed     裝飾層（窗燈、星點…）的亂數 seed＝最右一根 K 的日期（YYYYMMDD 整數，tradeDateSeed）。
//   過期     資料日期＝最後一筆的日期，用 tw-trading-days.mjs 的 staleMarker（與 3.5 相同）。
//   標示文字 數字只取資料原值：收盤價取 twii_daily 的 close（不是盤中序列；盤中序列停在 13:25，不等於收盤價）。
//
// ## 函式
//   readDaily(data)                       → { rows, warnings }
//   movingAverage(closes, period=20)      → (number|null)[]
//   skylineWindow(rows, {count, period})  → { candles:[{date,open,high,low,close,ma}], maCount, warnings }
//   priceRange(candles)                   → { lo, hi, mid, den, flat }；priceToV(p, range) → 0..1（平盤時在 0.5 附近）
//   candleGeom(candle, range, {floorY, span}) → { yHigh, yBodyTop, yBodyBot, yLow, yOpen, yClose, yMa }
//   candleDirection(candle)               → 'up'｜'down'｜'flat'
//   candleSlots(count, {xL, xR, fill})    → [{ x, w, cx }]（等寬、左到右＝舊到新）
//   skylineSeed(candles)、skylineCaption(candles)
//   buildSkylineModel(data, cfg, nowMs)   → 上面全部組起來（頁面與測試共用）

import { isRealDate } from './config-holidays.mjs';
import { MIN_RANGE_FRAC, formatPrice, tradeDateSeed } from './intraday.mjs';
import { staleMarker } from './tw-trading-days.mjs';

export const MA_PERIOD = 20;
export const CANDLE_COUNT = 20;
/** 20 根 K 每根都有完整 20MA 所需的筆數。 */
export const NEED_ROWS = CANDLE_COUNT + MA_PERIOD - 1;

const isPlain = (v) => v !== null && typeof v === 'object' && !Array.isArray(v);
const isPrice = (p) => typeof p === 'number' && Number.isFinite(p) && p > 0;

function isValidRow(r) {
  if (!isPlain(r) || !isRealDate(r.date)) return false;
  const { open, high, low, close } = r;
  if (![open, high, low, close].every(isPrice)) return false;
  return high >= Math.max(open, close, low) && low <= Math.min(open, close);
}

/**
 * 讀 `data.twii_daily`。回傳 `{ rows, warnings }`：rows 依日期由舊到新、只留 date／open／high／low／close。
 * 缺鍵、不是陣列、為空、沒有任何可用的列 → 拋錯。
 */
export function readDaily(data) {
  const raw = isPlain(data) ? data.twii_daily : undefined;
  if (raw === undefined || raw === null) throw new Error('缺少日 K 資料 twii_daily（資料層尚未產生或格式錯誤），不畫假資料');
  if (!Array.isArray(raw)) throw new Error('twii_daily 不是陣列，不畫假資料');
  if (raw.length === 0) throw new Error('twii_daily 為空，不畫假資料');
  const good = raw.filter(isValidRow).map((r) => ({ date: r.date, open: r.open, high: r.high, low: r.low, close: r.close }));
  if (good.length === 0) throw new Error(`twii_daily 沒有可用的日 K（${raw.length} 筆全不合法），不畫假資料`);
  const warnings = [];
  const bad = raw.length - good.length;
  if (bad > 0) warnings.push(`twii_daily 有 ${bad} 筆不合法的日 K 已剔除`);
  // 依日期穩定排序；同日重複保留原陣列中較後的一筆（資料層「同日重複取後者」）
  const byDate = new Map();
  let dup = 0;
  for (const r of good) {
    if (byDate.has(r.date)) dup++;
    byDate.set(r.date, r);
  }
  if (dup > 0) warnings.push(`twii_daily 同日重複 ${dup} 筆，取後者`);
  const rows = [...byDate.values()].sort((a, b) => (a.date < b.date ? -1 : a.date > b.date ? 1 : 0));
  return { rows, warnings };
}

/** 簡單移動平均：第 i 筆＝closes[i−period+1..i] 的算術平均；不足 period 筆為 null。 */
export function movingAverage(closes, period = MA_PERIOD) {
  const out = new Array(closes.length).fill(null);
  for (let i = period - 1; i < closes.length; i++) {
    let sum = 0;
    for (let k = i - period + 1; k <= i; k++) sum += closes[k];
    out[i] = sum / period;
  }
  return out;
}

/**
 * 取最後 count 根並附上各自的 20MA。rows 少於 period（一根完整的 20MA 都沒有）→ 拋錯；
 * 少於 count＋period−1 → 照畫、沒有完整 20MA 的根 ma＝null，並回警告。
 */
export function skylineWindow(rows, { count = CANDLE_COUNT, period = MA_PERIOD } = {}) {
  const n = rows.length;
  if (n < period) {
    throw new Error(`twii_daily 只有 ${n} 筆，連一根 K 的 ${period}MA 都算不出來（需要 ≥ ${period} 筆），不畫假資料`);
  }
  const ma = movingAverage(
    rows.map((r) => r.close),
    period,
  );
  const from = Math.max(0, n - count);
  const candles = rows.slice(from).map((r, k) => ({ ...r, ma: ma[from + k] }));
  const maCount = candles.filter((c) => c.ma !== null).length;
  const need = count + period - 1;
  const warnings = [];
  if (maCount < candles.length) {
    warnings.push(
      `twii_daily 只有 ${n} 筆（需要 ${need} 筆），最左 ${candles.length - maCount} 根 K 沒有完整 ${period}MA，不畫該段 MA`,
    );
  }
  return { candles, maCount, warnings };
}

/** 價格範圍（20 根的高、低與非 null 的 MA）；平盤時改用中價 × minRangeFrac 的最小範圍、以中價置中。 */
export function priceRange(candles, { minRangeFrac = MIN_RANGE_FRAC } = {}) {
  const all = [];
  for (const c of candles) {
    all.push(c.high, c.low);
    if (c.ma !== null && c.ma !== undefined) all.push(c.ma);
  }
  const lo = Math.min(...all);
  const hi = Math.max(...all);
  const raw = hi - lo;
  const mid = (hi + lo) / 2;
  const floor = Math.abs(mid) * minRangeFrac;
  const flat = !(raw > 0 && raw >= floor);
  const den = flat ? (floor > 0 ? floor : 1) : raw;
  return { lo, hi, mid, den, flat };
}

/** 價格 → 0..1（lo＝0、hi＝1；平盤時 0.5＋(p−mid)/den）。 */
export function priceToV(p, range) {
  return range.flat ? 0.5 + (p - range.mid) / range.den : (p - range.lo) / range.den;
}

/** K 線的畫面 y（高價在上）：y＝floorY − v × span。MA 為 null 時 yMa＝null。 */
export function candleGeom(c, range, { floorY, span }) {
  const y = (p) => floorY - priceToV(p, range) * span;
  return {
    yHigh: y(c.high),
    yLow: y(c.low),
    yOpen: y(c.open),
    yClose: y(c.close),
    yBodyTop: y(Math.max(c.open, c.close)),
    yBodyBot: y(Math.min(c.open, c.close)),
    yMa: c.ma === null || c.ma === undefined ? null : y(c.ma),
  };
}

/** 收 > 開＝'up'（台股紅）、收 < 開＝'down'（綠）、相等＝'flat'。 */
export function candleDirection(c) {
  return c.close > c.open ? 'up' : c.close < c.open ? 'down' : 'flat';
}

/** count 個等寬槽位，[xL, xR] 均分，建築寬＝槽寬 × fill、置中。左到右＝舊到新。 */
export function candleSlots(count, { xL, xR, fill }) {
  const slot = (xR - xL) / count;
  const w = slot * fill;
  return Array.from({ length: count }, (_, k) => {
    const cx = xL + (k + 0.5) * slot;
    return { x: cx - w / 2, w, cx };
  });
}

/** 裝飾層亂數 seed＝最右一根 K 的日期（YYYYMMDD）。 */
export function skylineSeed(candles) {
  return tradeDateSeed(candles[candles.length - 1].date);
}

/** 標示文字兩行：日期範圍、最右一根的收盤價（twii_daily.close）與 20MA。 */
export function skylineCaption(candles) {
  const first = candles[0];
  const last = candles[candles.length - 1];
  const ma = last.ma === null ? '—' : formatPrice(last.ma);
  return {
    range: `TAIEX ${candles.length}D  ${first.date} – ${last.date}`,
    values: `CLOSE ${formatPrice(last.close)}   MA${MA_PERIOD} ${ma}`,
  };
}

/**
 * 組裝：讀日 K → 取窗＋20MA → 價格範圍 → 過期標示 → seed → 標示文字。缺資料或不足 20 筆 → 拋錯。
 * @param {object} data   env.data（裸 tw_events 的鍵）
 * @param {object} cfg    合併內建預設後的設定（交易日判定用 config.holidays.TPE）
 * @param {number} nowMs  「現在」（UTC 毫秒）
 */
export function buildSkylineModel(data, cfg, nowMs) {
  const read = readDaily(data);
  const win = skylineWindow(read.rows);
  const last = win.candles[win.candles.length - 1];
  return {
    rowCount: read.rows.length,
    candles: win.candles,
    maCount: win.maCount,
    range: priceRange(win.candles),
    stale: staleMarker(last.date, nowMs, cfg, data),
    seed: skylineSeed(win.candles),
    caption: skylineCaption(win.candles),
    warnings: [...read.warnings, ...win.warnings],
  };
}
