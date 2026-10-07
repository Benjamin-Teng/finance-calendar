// host/ui/wallpapers/lib/intraday.mjs
//
// 脊線與等高線（task 3.5）共用的盤中走勢純函式。ES module、不依賴 DOM，可在 Node 測試
// （host/tests/wallpapers/intraday.test.mjs）。資料形狀見 AGENTS.md「現況」資料層一節：
//   data.twii_intraday ＝ { date: 'YYYY-MM-DD', points: [[epoch 秒, 價], …] }，最後一個完整交易日、5 分鐘一點。
//
// ## 原則（task 3.5 controller ruling 1）
//   - 畫面主形狀是真實走勢：時間→橫軸（首末點正規化到 0..1）、價格→高度（依當日高低點正規化到 0..1）。
//   - 高低差小於中價 × MIN_RANGE_FRAC（0.1%）時視為平盤：改用這個最小範圍、以中價置中，
//     不把 0.01 點的跳動放大成整幅山形（也不會除以零）。高低差剛好等於門檻時兩種算法結果相同。
//   - 裝飾層的亂數 seed 由資料交易日決定（tradeDateSeed）：同一份資料必定產生同一張圖。
//   - 畫面上的數字只取資料原值（局部高點的價位、首末點時間），不捏造刻度。
//   - 缺資料（缺鍵、序列為空、日期不合法、沒有可用的點）→ readIntraday 拋錯，頁面走 3.1 契約的錯誤路徑。

import { tzParts } from './core.mjs';
import { isRealDate } from './config-holidays.mjs';
import { dow } from './tw-trading-days.mjs';

/** 平盤門檻：高低差小於中價的這個比例時，改用它當正規化範圍（0.1%，加權指數約 48 點）。 */
export const MIN_RANGE_FRAC = 0.001;

const isPlain = (v) => v !== null && typeof v === 'object' && !Array.isArray(v);
const pad = (n) => String(n).padStart(2, '0');
const isPoint = (p) => Array.isArray(p) && p.length >= 2 && Number.isFinite(p[0]) && typeof p[1] === 'number' && Number.isFinite(p[1]) && p[1] > 0;

/**
 * 讀 `data.twii_intraday`。回傳 `{ date, points, warnings }`（points 依時間排序、只留 [秒, 價]）。
 * 缺鍵、日期不合法、points 不是陣列、序列為空、沒有任何可用的點 → 拋錯。部分壞點剔除並回警告。
 */
export function readIntraday(data) {
  const ti = isPlain(data) ? data.twii_intraday : undefined;
  if (!isPlain(ti)) throw new Error('缺少盤中走勢資料 twii_intraday（資料層尚未產生或格式錯誤），不畫假資料');
  if (!isRealDate(ti.date)) throw new Error(`twii_intraday 的日期不合法：${String(ti.date)}`);
  if (!Array.isArray(ti.points)) throw new Error(`twii_intraday（${ti.date}）的 points 不是陣列`);
  if (ti.points.length === 0) throw new Error(`twii_intraday（${ti.date}）序列為空，不畫假資料`);
  const good = ti.points.filter(isPoint).map((p) => [p[0], p[1]]);
  if (good.length === 0) throw new Error(`twii_intraday（${ti.date}）沒有可用的點（${ti.points.length} 個全不合法）`);
  good.sort((a, b) => a[0] - b[0]);
  const bad = ti.points.length - good.length;
  const warnings = bad > 0 ? [`twii_intraday（${ti.date}）有 ${bad} 個不合法的點已剔除`] : [];
  return { date: ti.date, points: good, warnings };
}

/**
 * 正規化：`t`＝時間（首點 0、末點 1；單點 0.5），`v`＝價格（當日低點 0、高點 1；平盤以中價置中在 0.5 附近）。
 * @returns {{ t: number[], v: number[], lo: number, hi: number, open: number, close: number,
 *             hiIdx: number, loIdx: number, flat: boolean, prices: number[] }}
 */
export function normalizeIntraday(points, { minRangeFrac = MIN_RANGE_FRAC } = {}) {
  const n = points.length;
  const ts = points.map((p) => p[0]);
  const prices = points.map((p) => p[1]);
  const span = ts[n - 1] - ts[0];
  const t = ts.map((x) => (span > 0 ? (x - ts[0]) / span : 0.5));
  let hiIdx = 0;
  let loIdx = 0;
  for (let i = 1; i < n; i++) {
    if (prices[i] > prices[hiIdx]) hiIdx = i;
    if (prices[i] < prices[loIdx]) loIdx = i;
  }
  const hi = prices[hiIdx];
  const lo = prices[loIdx];
  const raw = hi - lo;
  const mid = (hi + lo) / 2;
  const floor = Math.abs(mid) * minRangeFrac;
  const flat = !(raw > 0 && raw >= floor);
  let v;
  if (!flat) {
    v = prices.map((p) => (p - lo) / raw);
  } else {
    const den = floor > 0 ? floor : 1;
    v = prices.map((p) => 0.5 + (p - mid) / den);
  }
  return { t, v, lo, hi, open: prices[0], close: prices[n - 1], hiIdx, loIdx, flat, prices };
}

/** 序列 → 畫面座標：x＝x0 + t×(x1−x0)，y＝baseY − v×amp（高點在 baseY−amp、低點在 baseY）。 */
export function seriesToXY(norm, { x0, x1, baseY, amp }) {
  return norm.t.map((t, i) => [x0 + t * (x1 - x0), baseY - norm.v[i] * amp]);
}

/**
 * 脊線的主脊（金線）完整路徑：畫面左緣 → 第一個資料點以「開盤價位」水平延伸，資料段（seriesToXY），
 * 最後一個資料點 → 畫面右緣以「收盤價位」水平延伸（task 3.5 修正輪 1：資料外不得出現任何漲跌形狀）。
 * 延伸段只有一個端點、必定水平；資料起點已在左緣（或終點已在右緣）時不加延伸點。
 * 呼叫端把延伸段畫成與資料段明顯不同的「無資料」樣式。
 * @returns {{ pts: Array<[number, number]>, dataFrom: number, dataTo: number }}
 */
export function goldRidgePath(norm, { W, x0, x1, baseY, amp }) {
  const data = seriesToXY(norm, { x0, x1, baseY, amp });
  const first = data[0];
  const last = data[data.length - 1];
  const pts = [];
  if (first[0] > 0) pts.push([0, first[1]]);
  const dataFrom = pts.length;
  pts.push(...data);
  const dataTo = pts.length - 1;
  if (last[0] < W) pts.push([W, last[1]]);
  return { pts, dataFrom, dataTo };
}

/** 交易日 → 決定性亂數 seed（YYYYMMDD 整數，與樣稿 20260726 這類 seed 同形）。日期不合法拋錯。 */
export function tradeDateSeed(date) {
  if (!isRealDate(date)) throw new Error(`交易日不合法：${String(date)}`);
  return Number(date.replaceAll('-', ''));
}

/**
 * 序列的局部高點（等高線的標高位置）：在 ±k 點內最高、且比 ±w 點內最低處高出 minProm（正規化單位）。
 * 第一個一定是當日最高點；之後依高度挑，彼此時間相隔 ≥ minSepT，最多 max 個。平盤時只有最高點。
 * @returns {number[]} 序列索引
 */
export function seriesPeaks(norm, { max = 3, minSepT = 0.2, minProm = 0.1, k = 3, w = 6 } = {}) {
  const { v, t } = norm;
  const n = v.length;
  const cands = [];
  for (let i = 0; i < n; i++) {
    let isMax = true;
    for (let j = Math.max(0, i - k); j <= Math.min(n - 1, i + k) && isMax; j++) {
      if (v[j] > v[i]) isMax = false;
    }
    if (!isMax) continue;
    let low = v[i];
    for (let j = Math.max(0, i - w); j <= Math.min(n - 1, i + w); j++) low = Math.min(low, v[j]);
    if (v[i] - low >= minProm) cands.push(i);
  }
  cands.sort((a, b) => v[b] - v[a] || a - b);
  const picked = [norm.hiIdx];
  for (const c of cands) {
    if (picked.length >= max) break;
    if (picked.every((p) => Math.abs(t[c] - t[p]) >= minSepT)) picked.push(c);
  }
  return picked;
}

const WEEKDAYS = ['SUN', 'MON', 'TUE', 'WED', 'THU', 'FRI', 'SAT'];

/** epoch 秒 → 台北 HH:MM。 */
function tpeHm(sec) {
  const p = tzParts('Asia/Taipei', sec * 1000);
  return `${pad(p.h)}:${pad(p.mi)}`;
}

/** 交易日標示：「TAIEX 2026-10-01 THU 09:00–13:25」（首末點時間；單點只有一個時間）。全部來自資料。 */
export function dayCaption(date, points) {
  const a = tpeHm(points[0][0]);
  const b = tpeHm(points[points.length - 1][0]);
  return `TAIEX ${date} ${WEEKDAYS[dow(date)]} ${a === b ? a : `${a}–${b}`}`;
}

const PRICE_FMT = new Intl.NumberFormat('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 });

/** 價位：千分位、兩位小數（資料原值）。 */
export function formatPrice(p) {
  return PRICE_FMT.format(p);
}
