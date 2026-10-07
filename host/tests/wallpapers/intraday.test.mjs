// host/tests/wallpapers/intraday.test.mjs
//
// 脊線與等高線（task 3.5）共用的盤中走勢純函式：讀資料、正規化、序列→座標、交易日 seed、局部高點、標示文字。
// spec「脊線與等高線呈現盤中走勢」：以加權指數最後一個完整交易日的盤中走勢為資料，並標示該交易日日期。
// 執行：`node --test "host/tests/wallpapers/*.test.mjs"`

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import {
  MIN_RANGE_FRAC,
  dayCaption,
  formatPrice,
  goldRidgePath,
  normalizeIntraday,
  readIntraday,
  seriesPeaks,
  seriesToXY,
  tradeDateSeed,
} from '../../ui/wallpapers/lib/intraday.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const REPO = path.join(HERE, '..', '..', '..');
const SAMPLE = JSON.parse(readFileSync(path.join(REPO, 'tests', 'fixtures', 'tw_events_sample.json'), 'utf8'));
const PTS = SAMPLE.twii_intraday.points;

const finite = (arr) => arr.every((x) => Number.isFinite(x));
/** 台北 2026-10-01 的 epoch 秒：09:00 + 5 分鐘 × i。 */
const at = (i) => 1790816400 + i * 300;

// ── 讀資料 ──────────────────────────────────────────────────────────────────────────

test('readIntraday：真實樣本 2026-10-01，54 點、依時間排序', () => {
  const s = readIntraday(SAMPLE);
  assert.equal(s.date, '2026-10-01');
  assert.equal(s.points.length, 54);
  assert.deepEqual(s.points[0], [1790816400, 48036.41]);
  assert.deepEqual(s.warnings, []);
});

test('readIntraday：缺鍵、序列為空、data 為 null → 拋錯（頁面走錯誤路徑，不畫假資料）', () => {
  assert.throws(() => readIntraday(null), /twii_intraday/);
  assert.throws(() => readIntraday({}), /twii_intraday/);
  assert.throws(() => readIntraday({ twii_intraday: 'x' }), /twii_intraday/);
  assert.throws(() => readIntraday({ twii_intraday: { date: '2026-10-01', points: [] } }), /空/);
  assert.throws(() => readIntraday({ twii_intraday: { date: '2026-10-01' } }), /points/);
});

test('readIntraday：日期不合法 → 拋錯', () => {
  for (const date of ['2026-02-30', '2026/10/01', '', null, 20261001]) {
    assert.throws(() => readIntraday({ twii_intraday: { date, points: [[at(0), 100]] } }), /日期/, String(date));
  }
});

test('readIntraday：壞點剔除並警告；全部都壞 → 拋錯；亂序會排序', () => {
  const s = readIntraday({ twii_intraday: { date: '2026-10-01', points: [[at(2), 102], [at(0), 100], null, [at(1), 'x'], [at(3)], [at(4), -1], [at(5), NaN], [at(1), 101]] } });
  assert.deepEqual(s.points, [[at(0), 100], [at(1), 101], [at(2), 102]]);
  assert.equal(s.warnings.length, 1);
  assert.match(s.warnings[0], /5 個/);
  assert.throws(() => readIntraday({ twii_intraday: { date: '2026-10-01', points: [null, [at(0), 0]] } }), /沒有可用/);
});

// ── 正規化 ──────────────────────────────────────────────────────────────────────────

test('normalizeIntraday：時間以首末點正規化到 0..1、價格以當日高低點正規化到 0..1', () => {
  const n = normalizeIntraday(PTS);
  assert.equal(n.t[0], 0);
  assert.equal(n.t.at(-1), 1);
  assert.equal(n.v[n.hiIdx], 1);
  assert.equal(n.v[n.loIdx], 0);
  assert.equal(n.hi, Math.max(...PTS.map((p) => p[1])));
  assert.equal(n.lo, Math.min(...PTS.map((p) => p[1])));
  assert.equal(n.open, 48036.41);
  assert.equal(n.close, 48281.21);
  assert.equal(n.flat, false);
  assert.ok(n.v.every((v) => v >= 0 && v <= 1));
  // 單調：價格高者 v 高
  for (let i = 1; i < PTS.length; i++) assert.equal(Math.sign(n.v[i] - n.v[0]), Math.sign(PTS[i][1] - PTS[0][1]));
});

test('normalizeIntraday：平盤（高低相同）不除以零，整條在中線 0.5', () => {
  const n = normalizeIntraday([0, 1, 2, 3].map((i) => [at(i), 48000]));
  assert.ok(finite(n.v) && finite(n.t));
  assert.deepEqual(n.v, [0.5, 0.5, 0.5, 0.5]);
  assert.equal(n.flat, true);
});

test('normalizeIntraday：高低差小於中價 × MIN_RANGE_FRAC（幾乎平盤）不放大雜訊，壓在中線附近', () => {
  const n = normalizeIntraday([0, 1, 2, 3, 4].map((i) => [at(i), 48000 + (i % 2) * 0.01]));
  assert.equal(n.flat, true);
  assert.ok(n.v.every((v) => Math.abs(v - 0.5) < 0.001), JSON.stringify(n.v));
  assert.equal(MIN_RANGE_FRAC, 0.001);
  // 剛好等於門檻時與一般正規化一致（連續，不跳）
  const r = 48000 * MIN_RANGE_FRAC;
  const edge = normalizeIntraday([[at(0), 48000 - r / 2], [at(1), 48000 + r / 2]]);
  assert.ok(Math.abs(edge.v[0]) < 1e-9 && Math.abs(edge.v[1] - 1) < 1e-9, JSON.stringify(edge.v));
});

test('normalizeIntraday：單點不除以零（t、v 都是 0.5）', () => {
  const n = normalizeIntraday([[at(0), 48000]]);
  assert.deepEqual(n.t, [0.5]);
  assert.deepEqual(n.v, [0.5]);
  assert.equal(n.hiIdx, 0);
  assert.equal(n.loIdx, 0);
});

// ── 序列 → 座標 ────────────────────────────────────────────────────────────────────

test('seriesToXY：時間→橫軸（x0..x1）、價格→高度（低點在 baseY、高點在 baseY−amp）', () => {
  const n = normalizeIntraday(PTS);
  const xy = seriesToXY(n, { x0: 100, x1: 900, baseY: 800, amp: 200 });
  assert.equal(xy.length, PTS.length);
  assert.deepEqual(xy[0].map((v) => +v.toFixed(6)), [100, +(800 - 200 * n.v[0]).toFixed(6)]);
  assert.equal(xy.at(-1)[0], 900);
  assert.equal(xy[n.hiIdx][1], 600);
  assert.equal(xy[n.loIdx][1], 800);
  for (let i = 1; i < xy.length; i++) assert.ok(xy[i][0] > xy[i - 1][0]);
});

test('seriesToXY：單點與平盤都得到有限座標', () => {
  for (const pts of [[[at(0), 48000]], [0, 1, 2].map((i) => [at(i), 48000])]) {
    const xy = seriesToXY(normalizeIntraday(pts), { x0: 100, x1: 900, baseY: 800, amp: 200 });
    assert.ok(xy.every(([x, y]) => Number.isFinite(x) && Number.isFinite(y)));
    assert.ok(xy.every(([, y]) => y === 700));
  }
});

test('goldRidgePath（修正輪 1）：資料段＝seriesToXY；資料外以開盤／收盤價位水平延伸到畫面左右緣（不畫任何漲跌）', () => {
  const n = normalizeIntraday(PTS);
  const o = { W: 1920, x0: 200, x1: 1100, baseY: 860, amp: 230 };
  const p = goldRidgePath(n, o);
  const data = seriesToXY(n, o);
  assert.deepEqual(p.pts.slice(p.dataFrom, p.dataTo + 1), data);
  const yOpen = data[0][1];
  const yClose = data.at(-1)[1];
  // 左段：從畫面左緣到第一個資料點，y 全等於開盤高度
  assert.deepEqual(p.pts[0], [0, yOpen]);
  assert.ok(p.pts.slice(0, p.dataFrom).every(([, y]) => y === yOpen), '左段水平');
  // 右段：從最後一個資料點到畫面右緣，y 全等於收盤高度
  assert.deepEqual(p.pts.at(-1), [1920, yClose]);
  assert.ok(p.pts.slice(p.dataTo + 1).every(([, y]) => y === yClose), '右段水平');
  for (let i = 1; i < p.pts.length; i++) assert.ok(p.pts[i][0] >= p.pts[i - 1][0], `x 遞增 @${i}`);
  // 資料外只有兩個端點，沒有中間點（不可能出現坡度）
  assert.equal(p.dataFrom, 1);
  assert.equal(p.pts.length - 1 - p.dataTo, 1);
});

test('goldRidgePath：資料起點就在左緣、終點就在右緣時不加延伸點', () => {
  const n = normalizeIntraday(PTS);
  const p = goldRidgePath(n, { W: 1000, x0: 0, x1: 1000, baseY: 500, amp: 100 });
  assert.equal(p.dataFrom, 0);
  assert.equal(p.dataTo, p.pts.length - 1);
});

test('goldRidgePath：單點序列也能組出完整路徑（整條水平）', () => {
  const p = goldRidgePath(normalizeIntraday([[at(0), 48000]]), { W: 1000, x0: 100, x1: 600, baseY: 500, amp: 100 });
  assert.equal(p.dataFrom, p.dataTo);
  assert.deepEqual(p.pts, [[0, 450], [350, 450], [1000, 450]]);
});

// ── 交易日 seed ────────────────────────────────────────────────────────────────────

test('tradeDateSeed：同一交易日同一 seed、不同交易日不同 seed；格式錯拋錯', () => {
  assert.equal(tradeDateSeed('2026-10-01'), 20261001);
  assert.equal(tradeDateSeed('2026-10-01'), tradeDateSeed('2026-10-01'));
  assert.notEqual(tradeDateSeed('2026-10-01'), tradeDateSeed('2026-10-02'));
  assert.notEqual(tradeDateSeed('2026-09-24'), tradeDateSeed('2026-10-01'));
  assert.throws(() => tradeDateSeed('2026/10/01'));
  assert.throws(() => tradeDateSeed('2026-02-30'));
});

// ── 局部高點（等高線的標高） ───────────────────────────────────────────────────────

test('seriesPeaks：第一個一定是當日最高點；最多 max 個；彼此時間相隔 ≥ minSepT', () => {
  const n = normalizeIntraday(PTS);
  const pk = seriesPeaks(n, { max: 3, minSepT: 0.2, minProm: 0.1 });
  assert.equal(pk[0], n.hiIdx);
  assert.ok(pk.length >= 1 && pk.length <= 3);
  for (let i = 0; i < pk.length; i++) {
    for (let j = i + 1; j < pk.length; j++) assert.ok(Math.abs(n.t[pk[i]] - n.t[pk[j]]) >= 0.2);
  }
});

test('seriesPeaks：兩座明顯的山 → 兩個標高；平盤 → 只有最高點一個', () => {
  const mk = (vals) => normalizeIntraday(vals.map((p, i) => [at(i), p]));
  const two = mk([100, 104, 108, 104, 100, 100, 100, 100, 103, 106, 103, 100]);
  assert.deepEqual(seriesPeaks(two, { max: 3, minSepT: 0.2, minProm: 0.1 }), [2, 9]);
  const flat = mk([48000, 48000.01, 48000, 48000.01, 48000, 48000.01, 48000, 48000.01]);
  assert.deepEqual(seriesPeaks(flat, { max: 3, minSepT: 0.2, minProm: 0.1 }), [flat.hiIdx]);
  const single = mk([48000]);
  assert.deepEqual(seriesPeaks(single, { max: 3, minSepT: 0.2, minProm: 0.1 }), [0]);
});

// ── 標示文字 ───────────────────────────────────────────────────────────────────────

test('dayCaption：指數、交易日、星期、首末點的台北時間（全部來自資料）', () => {
  assert.equal(dayCaption('2026-10-01', PTS), 'TAIEX 2026-10-01 THU 09:00–13:25');
  assert.equal(dayCaption('2026-09-24', [[1790211600, 1]]), 'TAIEX 2026-09-24 THU 09:00');
});

test('formatPrice：千分位、兩位小數（資料原值，不捏造位數以外的數）', () => {
  assert.equal(formatPrice(48281.21), '48,281.21');
  assert.equal(formatPrice(48000), '48,000.00');
  assert.equal(formatPrice(9876.5), '9,876.50');
});

// ── 截圖情境 fixture 的前提 ─────────────────────────────────────────────────────────

const FIX = (name) => JSON.parse(readFileSync(path.join(HERE, 'fixtures', name), 'utf8'));
const CFG = JSON.parse(readFileSync(path.join(REPO, 'host', 'ui', 'wallpapers', 'config', 'wallpaper-config.default.json'), 'utf8'));

test('fixture 前提：週一早上（9/24 真實序列）在 2026-09-28 06:00 落後 0、不顯示過期標示', async () => {
  const { staleMarker } = await import('../../ui/wallpapers/lib/tw-trading-days.mjs');
  const { parseInstant } = await import('../../ui/wallpapers/lib/core.mjs');
  const d = FIX('intraday-monday-0924.json').data;
  const s = readIntraday(d);
  assert.equal(s.date, '2026-09-24');
  assert.equal(s.points.length, 54);
  const m = staleMarker(s.date, parseInstant('2026-09-28T06:00', 'Asia/Taipei'), CFG, d);
  assert.equal(m.lastClosed, '2026-09-24');
  assert.equal(m.behind, 0);
  assert.equal(dayCaption(s.date, s.points), 'TAIEX 2026-09-24 THU 09:00–13:25');
});

test('fixture 前提：平盤序列 flat=true、標高只有最高點；缺鍵與空序列都拋錯', () => {
  const n = normalizeIntraday(readIntraday(FIX('intraday-flat.json').data).points);
  assert.equal(n.flat, true);
  assert.ok(Math.abs(n.hi - n.lo - 0.01) < 1e-6);
  assert.deepEqual(seriesPeaks(n, { max: 3, minSepT: 0.2, minProm: 0.1 }), [n.hiIdx]);
  assert.throws(() => readIntraday(FIX('intraday-missing.json').data), /twii_intraday/);
  assert.throws(() => readIntraday(FIX('intraday-empty.json').data), /空/);
});
