// host/tests/wallpapers/skyline-model.test.mjs
//
// 天際線（task 3.6）的純函式：讀日 K、20MA、取最後 20 根、資料不足、價格正規化（含平盤）、K 線幾何、seed、標示文字。
// spec「天際線呈現 20 日 K 線與 20MA」「資料過期標示」。
// 20MA 的期望值一律用**本檔自己的最簡單迴圈**計算（與實作分開），並挑 2026-09-02 那根 K 手算後寫死（controller ruling 2）。
// 執行：`node --test "host/tests/wallpapers/*.test.mjs"`

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import {
  CANDLE_COUNT,
  MA_PERIOD,
  NEED_ROWS,
  buildSkylineModel,
  candleDirection,
  candleGeom,
  candleSlots,
  movingAverage,
  priceRange,
  priceToV,
  readDaily,
  skylineCaption,
  skylineSeed,
  skylineWindow,
} from '../../ui/wallpapers/lib/skyline-model.mjs';
import { isTpeTradingDay } from '../../ui/wallpapers/lib/tw-trading-days.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const REPO = path.join(HERE, '..', '..', '..');
const SAMPLE = JSON.parse(readFileSync(path.join(REPO, 'tests', 'fixtures', 'tw_events_sample.json'), 'utf8'));
const DAILY = SAMPLE.twii_daily; // 63 筆真實日 K（2026-07-02..2026-10-01）
const DAILY40 = DAILY.slice(-40); // tasks.md：「以 40 筆樣本驗證第一根 K 的 20MA」
const fixture = (name) => JSON.parse(readFileSync(path.join(HERE, 'fixtures', name), 'utf8'));
const tpe = (s) => Date.parse(`${s}+08:00`);
const EPS = 1e-9;

/** 獨立計算：第 i 根的 20MA＝含它在內往前 20 筆 close 的算術平均；不足 20 筆＝null。刻意寫成最笨的迴圈。 */
function independentMa(rows, i) {
  if (i < 19) return null;
  let sum = 0;
  for (let k = i - 19; k <= i; k++) sum = sum + rows[k].close;
  return sum / 20;
}

/** 2026-09-02（40 筆樣本的第 21 筆＝畫面最左一根）往前 20 個交易日的收盤價，逐筆抄自 tw_events_sample.json。 */
const HAND_0902 = [
  ['2026-08-06', 44396.7],
  ['2026-08-07', 44225.91],
  ['2026-08-10', 44928.76],
  ['2026-08-11', 45120.72],
  ['2026-08-12', 45518.07],
  ['2026-08-13', 46021.48],
  ['2026-08-14', 45811.01],
  ['2026-08-17', 45857.27],
  ['2026-08-18', 45308.68],
  ['2026-08-19', 44719.35],
  ['2026-08-20', 44933.74],
  ['2026-08-21', 45224.29],
  ['2026-08-24', 44762.32],
  ['2026-08-25', 45169.46],
  ['2026-08-26', 45832.62],
  ['2026-08-27', 45975.22],
  ['2026-08-28', 46331.45],
  ['2026-08-31', 46128.47],
  ['2026-09-01', 46948.72],
  ['2026-09-02', 46164.72],
];
/** 手算（十進位逐筆相加）：合計 909,378.96 → ÷20 ＝ 45,468.948。 */
const HAND_0902_SUM = 909378.96;
const HAND_0902_MA = 45468.948;

const row = (date, o, h, l, c) => ({ date, open: o, high: h, low: l, close: c });

// ── readDaily ───────────────────────────────────────────────────────────────────────

test('readDaily：真實樣本 63 筆、依日期由舊到新、無警告', () => {
  const r = readDaily(SAMPLE);
  assert.equal(r.rows.length, 63);
  assert.equal(r.rows[0].date, '2026-07-02');
  assert.equal(r.rows.at(-1).date, '2026-10-01');
  assert.deepEqual(r.warnings, []);
  for (let i = 1; i < r.rows.length; i++) assert.ok(r.rows[i - 1].date < r.rows[i].date);
});

test('readDaily：twii_daily 不存在、不是陣列、為空、data 為 null → 拋錯（不畫假資料）', () => {
  assert.throws(() => readDaily({ holidays: [] }), /缺少日 K 資料 twii_daily/);
  assert.throws(() => readDaily(null), /缺少日 K 資料 twii_daily/);
  assert.throws(() => readDaily({ twii_daily: { date: '2026-10-01' } }), /twii_daily 不是陣列/);
  assert.throws(() => readDaily({ twii_daily: 'x' }), /twii_daily 不是陣列/);
  assert.throws(() => readDaily({ twii_daily: [] }), /twii_daily 為空/);
});

test('readDaily：不合法的列（日期錯、價格非數字或 ≤ 0、高低與開收矛盾）剔除並警告；全壞拋錯', () => {
  const good = DAILY.slice(-3);
  const r = readDaily({
    twii_daily: [
      ...good,
      row('2026-13-01', 1, 1, 1, 1),
      row('2026-10-02', 1, null, 1, 1),
      row('2026-10-05', 100, 99, 98, 100), // high < open
      row('2026-10-06', 100, 105, 101, 100), // low > close
      row('2026-10-07', 0, 0, 0, 0),
      'x',
    ],
  });
  assert.equal(r.rows.length, 3);
  assert.equal(r.warnings.length, 1);
  assert.match(r.warnings[0], /6 筆不合法的日 K 已剔除/);
  assert.throws(() => readDaily({ twii_daily: [row('2026-10-02', 1, null, 1, 1)] }), /沒有可用的日 K/);
});

test('readDaily：亂序依日期排序；同日重複取後者並警告', () => {
  const [a, b, c] = DAILY.slice(-3);
  const dup = { ...b, close: b.close + 1, high: b.high + 1 };
  const r = readDaily({ twii_daily: [c, a, b, dup] });
  assert.deepEqual(
    r.rows.map((x) => x.date),
    [a.date, b.date, c.date],
  );
  assert.equal(r.rows[1].close, b.close + 1);
  assert.equal(r.warnings.length, 1);
  assert.match(r.warnings[0], /同日重複 1 筆/);
});

// ── 20MA ────────────────────────────────────────────────────────────────────────────

test('20MA：手算的 2026-09-02 那根——20 個收盤價與樣本逐筆相同、合計與平均正確（誤差 < 1e-9）', () => {
  const i = DAILY40.findIndex((r) => r.date === '2026-09-02');
  assert.equal(i, 20, '40 筆樣本的第 21 筆＝畫面最左一根');
  assert.deepEqual(
    DAILY40.slice(i - 19, i + 1).map((r) => [r.date, r.close]),
    HAND_0902,
  );
  // 手算值本身自洽（防抄錯）：平均 × 20 ＝ 合計
  assert.ok(Math.abs(HAND_0902_MA * 20 - HAND_0902_SUM) < 1e-6);
  const ma = movingAverage(DAILY40.map((r) => r.close));
  assert.ok(Math.abs(ma[i] - HAND_0902_MA) < EPS, `實作 ${ma[i]}，手算 ${HAND_0902_MA}`);
});

test('20MA：40 筆樣本每一根都與獨立迴圈相同（誤差 < 1e-9）；前 19 筆為 null（不用不足 20 筆的平均冒充）', () => {
  const ma = movingAverage(DAILY40.map((r) => r.close));
  assert.equal(ma.length, 40);
  for (let i = 0; i < 40; i++) {
    const want = independentMa(DAILY40, i);
    if (want === null) assert.equal(ma[i], null, `#${i}`);
    else assert.ok(Math.abs(ma[i] - want) < EPS, `#${i}：${ma[i]} vs ${want}`);
  }
});

test('20MA：週期參數與邊界（剛好 20 筆只有最後一筆有值；空陣列）', () => {
  assert.equal(MA_PERIOD, 20);
  const c = Array.from({ length: 20 }, (_, k) => k + 1); // 1..20 → 平均 10.5
  const ma = movingAverage(c);
  assert.deepEqual(ma.slice(0, 19), Array(19).fill(null));
  assert.equal(ma[19], 10.5);
  assert.deepEqual(movingAverage([]), []);
  assert.deepEqual(movingAverage([1, 2, 3], 2), [null, 1.5, 2.5]);
});

// ── 取最後 20 根 ─────────────────────────────────────────────────────────────────────

test('skylineWindow（40 筆樣本）：20 根、最左 2026-09-02 最右 2026-10-01、每根都有 20MA 且等於獨立計算', () => {
  assert.equal(CANDLE_COUNT, 20);
  assert.equal(NEED_ROWS, 39);
  const w = skylineWindow(DAILY40);
  assert.equal(w.candles.length, 20);
  assert.equal(w.candles[0].date, '2026-09-02');
  assert.equal(w.candles.at(-1).date, '2026-10-01');
  assert.equal(w.maCount, 20);
  assert.deepEqual(w.warnings, []);
  assert.ok(Math.abs(w.candles[0].ma - HAND_0902_MA) < EPS);
  w.candles.forEach((c, k) => {
    const want = independentMa(DAILY40, 20 + k);
    assert.ok(Math.abs(c.ma - want) < EPS, c.date);
    // K 線四價原樣來自資料
    assert.deepEqual([c.open, c.high, c.low, c.close], [DAILY40[20 + k].open, DAILY40[20 + k].high, DAILY40[20 + k].low, DAILY40[20 + k].close]);
  });
});

test('skylineWindow：63 筆與 40 筆取到同一組 20 根與同樣的 20MA（多出的舊資料不影響）', () => {
  const a = skylineWindow(DAILY).candles;
  const b = skylineWindow(DAILY40).candles;
  assert.deepEqual(a, b);
});

test('spec「收盤後更新」：最右一根是當日、最左一根是 19 個交易日前；當日資料進來後整窗前移一個交易日', () => {
  const holidaysData = { holidays: SAMPLE.holidays };
  const tradingDaysBetween = (from, to) => {
    let n = 0;
    for (let d = new Date(`${from}T00:00:00Z`); ; ) {
      d = new Date(d.getTime() + 86400000);
      const s = d.toISOString().slice(0, 10);
      if (s > to) return n;
      if (isTpeTradingDay(s, {}, holidaysData)) n++;
    }
  };
  const w = skylineWindow(DAILY);
  assert.equal(w.candles.at(-1).date, '2026-10-01');
  assert.equal(tradingDaysBetween(w.candles[0].date, w.candles.at(-1).date), 19);
  // 10/02（週五）收盤後資料層更新為當日
  const today = row('2026-10-02', 48400, 48700, 48300, 48650);
  const w2 = skylineWindow([...DAILY, today]);
  assert.equal(w2.candles.at(-1).date, '2026-10-02');
  assert.equal(w2.candles[0].date, '2026-09-03');
  assert.equal(tradingDaysBetween(w2.candles[0].date, w2.candles.at(-1).date), 19);
  assert.ok(Math.abs(w2.candles.at(-1).ma - independentMa([...DAILY, today], DAILY.length)) < EPS);
});

// ── 資料不足 ─────────────────────────────────────────────────────────────────────────

test('資料剛好 39 筆：20 根都有完整 20MA、無警告', () => {
  const w = skylineWindow(DAILY.slice(-39));
  assert.equal(w.candles.length, 20);
  assert.equal(w.maCount, 20);
  assert.deepEqual(w.warnings, []);
  assert.ok(Math.abs(w.candles[0].ma - HAND_0902_MA) < EPS);
});

test('資料 38 筆：K 照畫 20 根，最左 1 根沒有 20MA（null，不以 19 筆平均冒充）並警告', () => {
  const rows = DAILY.slice(-38);
  const w = skylineWindow(rows);
  assert.equal(w.candles.length, 20);
  assert.equal(w.candles[0].ma, null);
  assert.equal(w.maCount, 19);
  for (let k = 1; k < 20; k++) assert.ok(Math.abs(w.candles[k].ma - independentMa(rows, 18 + k)) < EPS);
  assert.equal(w.warnings.length, 1);
  assert.match(w.warnings[0], /只有 38 筆（需要 39 筆）.*最左 1 根 K 沒有完整 20MA/);
});

test('資料 20 筆：只有最右一根有 20MA；資料 19 筆或更少 → 拋錯（一根完整的都沒有）', () => {
  const w = skylineWindow(DAILY.slice(-20));
  assert.equal(w.candles.length, 20);
  assert.equal(w.maCount, 1);
  assert.deepEqual(
    w.candles.slice(0, 19).map((c) => c.ma),
    Array(19).fill(null),
  );
  assert.ok(Math.abs(w.candles[19].ma - independentMa(DAILY.slice(-20), 19)) < EPS);
  assert.throws(() => skylineWindow(DAILY.slice(-19)), /只有 19 筆.*連一根 K 的 20MA 都算不出來/);
  assert.throws(() => skylineWindow(DAILY.slice(-1)), /只有 1 筆/);
});

// ── 正規化 ───────────────────────────────────────────────────────────────────────────

test('priceRange：範圍涵蓋 20 根的最高、最低與所有 20MA；v(lo)=0、v(hi)=1、單調', () => {
  const { candles } = skylineWindow(DAILY);
  const r = priceRange(candles);
  assert.equal(r.flat, false);
  const all = candles.flatMap((c) => [c.high, c.low, c.ma]);
  assert.equal(r.lo, Math.min(...all));
  assert.equal(r.hi, Math.max(...all));
  assert.equal(r.lo, 45398.43);
  assert.equal(r.hi, 48601.53);
  assert.equal(priceToV(r.lo, r), 0);
  assert.equal(priceToV(r.hi, r), 1);
  assert.ok(priceToV(46000, r) < priceToV(47000, r));
});

test('priceRange：MA 為 null 的根不參與範圍（不會變成 0 把範圍拉到 0）', () => {
  const { candles } = skylineWindow(DAILY.slice(-20));
  const r = priceRange(candles);
  assert.ok(r.lo > 40000, `lo=${r.lo}`);
});

test('平盤：20 根開高低收全相同 → 不除以零、全部 v=0.5；幾乎平盤（差 0.01）壓在 0.5±0.001', () => {
  const flatRows = DAILY.slice(-39).map((r) => row(r.date, 48000, 48000, 48000, 48000));
  const { candles } = skylineWindow(flatRows);
  const r = priceRange(candles);
  assert.equal(r.flat, true);
  for (const c of candles) {
    for (const p of [c.open, c.high, c.low, c.close, c.ma]) assert.equal(priceToV(p, r), 0.5);
  }
  const near = DAILY.slice(-39).map((x, k) => row(x.date, 48000, 48000.01, 48000, k % 2 ? 48000.01 : 48000));
  const rn = priceRange(skylineWindow(near).candles);
  assert.equal(rn.flat, true);
  for (const c of skylineWindow(near).candles) {
    for (const p of [c.high, c.low, c.ma]) {
      const v = priceToV(p, rn);
      assert.ok(Number.isFinite(v) && Math.abs(v - 0.5) <= 0.001, `${p} → ${v}`);
    }
  }
});

test('平盤門檻：高低差剛好等於中價 × 0.1% 時，兩種算法結果相同（連續）', () => {
  const mid = 48000;
  const half = (mid * 0.001) / 2;
  const rows = DAILY.slice(-39).map((x) => row(x.date, mid, mid + half, mid - half, mid));
  const r = priceRange(skylineWindow(rows).candles);
  // 浮點下 48000 × 0.001 可能比 48 多一點點，落在哪一支都可以；重點是兩支在門檻上給同樣的 v
  assert.ok(Math.abs(priceToV(mid + half, r) - 1) < EPS, `flat=${r.flat}`);
  assert.ok(Math.abs(priceToV(mid - half, r) - 0) < EPS, `flat=${r.flat}`);
  assert.ok(Math.abs(priceToV(mid, r) - 0.5) < EPS);
  // 兩支的公式在門檻上相等
  const flatV = (p) => 0.5 + (p - mid) / (mid * 0.001);
  const rangeV = (p) => (p - (mid - half)) / (2 * half);
  for (const p of [mid - half, mid, mid + half]) assert.ok(Math.abs(flatV(p) - rangeV(p)) < EPS);
});

// ── K 線幾何 ─────────────────────────────────────────────────────────────────────────

test('candleGeom：開高低收與 MA 依同一價格比例對應到 y（高價在上）；實體上下緣＝max／min(開, 收)', () => {
  const { candles } = skylineWindow(DAILY);
  const r = priceRange(candles);
  const box = { floorY: 900, span: 600 };
  const y = (p) => 900 - priceToV(p, r) * 600;
  for (const c of candles) {
    const g = candleGeom(c, r, box);
    assert.equal(g.yHigh, y(c.high));
    assert.equal(g.yLow, y(c.low));
    assert.equal(g.yBodyTop, y(Math.max(c.open, c.close)));
    assert.equal(g.yBodyBot, y(Math.min(c.open, c.close)));
    assert.equal(g.yMa, y(c.ma));
    assert.ok(g.yHigh <= g.yBodyTop && g.yBodyTop <= g.yBodyBot && g.yBodyBot <= g.yLow);
  }
  const g0 = candleGeom({ ...candles[0], ma: null }, r, box);
  assert.equal(g0.yMa, null);
});

test('candleDirection：收 > 開＝up（紅）、收 < 開＝down（綠）、相等＝flat', () => {
  assert.equal(candleDirection(row('2026-10-01', 100, 110, 90, 105)), 'up');
  assert.equal(candleDirection(row('2026-10-01', 100, 110, 90, 95)), 'down');
  assert.equal(candleDirection(row('2026-10-01', 100, 110, 90, 100)), 'flat');
  assert.equal(candleDirection(DAILY.at(-1)), 'up'); // 10/01 開 47961.98 收 48353.49
});

test('截圖情境前提：大漲 fixture 20 根的 MA 全在 K 下方（低於最低價）；大跌 fixture 全在 K 上方（高於最高價）', () => {
  const up = skylineWindow(readDaily(fixture('skyline-rally.json').data).rows);
  assert.equal(up.maCount, 20);
  for (const c of up.candles) assert.ok(c.ma < c.low, `${c.date} ma ${c.ma} low ${c.low}`);
  const dn = skylineWindow(readDaily(fixture('skyline-crash.json').data).rows);
  assert.equal(dn.maCount, 20);
  for (const c of dn.candles) assert.ok(c.ma > c.high, `${c.date} ma ${c.ma} high ${c.high}`);
});

test('截圖情境前提：剛好 39 筆 fixture 是樣本最後 39 筆原值；缺鍵／空陣列 fixture 拋錯；19 筆 fixture 拋錯；30 筆 fixture 警告', () => {
  const r39 = readDaily(fixture('skyline-39.json').data);
  assert.deepEqual(r39.rows, DAILY.slice(-39));
  assert.equal(skylineWindow(r39.rows).maCount, 20);
  assert.throws(() => readDaily(fixture('skyline-missing.json').data), /缺少日 K 資料/);
  assert.throws(() => readDaily(fixture('skyline-empty.json').data), /為空/);
  assert.throws(() => skylineWindow(readDaily(fixture('skyline-19.json').data).rows), /只有 19 筆/);
  const w30 = skylineWindow(readDaily(fixture('skyline-30.json').data).rows);
  assert.equal(w30.maCount, 11);
  assert.equal(w30.warnings.length, 1);
});

// ── 版面槽位 ─────────────────────────────────────────────────────────────────────────

test('candleSlots：20 個等寬槽位、中心遞增、全在 [xL, xR] 內、相鄰有間隔；最右一槽給最新一根', () => {
  const s = candleSlots(20, { xL: 40, xR: 1880, fill: 0.7 });
  assert.equal(s.length, 20);
  for (let k = 0; k < 20; k++) {
    assert.ok(s[k].x >= 40 && s[k].x + s[k].w <= 1880 + EPS, `#${k}`);
    assert.ok(Math.abs(s[k].cx - (s[k].x + s[k].w / 2)) < EPS);
    assert.ok(Math.abs(s[k].w - s[0].w) < EPS);
    if (k > 0) assert.ok(s[k].x > s[k - 1].x + s[k - 1].w, `#${k} 與前一根相接或重疊`);
  }
  assert.ok(Math.abs(s[0].w - (1840 / 20) * 0.7) < EPS);
});

// ── seed 與標示文字 ──────────────────────────────────────────────────────────────────

test('skylineSeed：最右一根 K 的日期 YYYYMMDD；同資料同 seed；改舊資料不影響、換最新日期才變', () => {
  const { candles } = skylineWindow(DAILY);
  assert.equal(skylineSeed(candles), 20261001);
  assert.equal(skylineSeed(skylineWindow(DAILY).candles), 20261001);
  const edited = DAILY.map((r, k) => (k === 50 ? { ...r, close: r.close + 1 } : r));
  assert.equal(skylineSeed(skylineWindow(edited).candles), 20261001);
  assert.equal(skylineSeed(skylineWindow([...DAILY, row('2026-10-02', 1, 2, 1, 2)]).candles), 20261002);
});

test('skylineCaption：日期範圍＋最右一根的收盤價（twii_daily.close）與 20MA，數字皆為資料原值', () => {
  const { candles } = skylineWindow(DAILY);
  const cap = skylineCaption(candles);
  assert.equal(cap.range, 'TAIEX 20D  2026-09-02 – 2026-10-01');
  assert.equal(cap.values, 'CLOSE 48,353.49   MA20 46,981.62');
});

// ── 組裝（過期標示） ─────────────────────────────────────────────────────────────────

test('buildSkylineModel：一般（10/02 12:00）落後 0、不顯示；seed、標示、警告', () => {
  const m = buildSkylineModel(SAMPLE, {}, tpe('2026-10-02T12:00'));
  assert.equal(m.candles.length, 20);
  assert.equal(m.stale.dataDate, '2026-10-01');
  assert.equal(m.stale.behind, 0);
  assert.equal(m.stale.show, false);
  assert.equal(m.seed, 20261001);
  assert.deepEqual(m.warnings, []);
  assert.equal(m.range.flat, false);
});

test('spec「剛好落後 2 個交易日」：資料 10/01，10/06 13:29 落後 2 不顯示；13:30 落後 3 顯示「資料停在 10/1」', () => {
  const a = buildSkylineModel(SAMPLE, {}, tpe('2026-10-06T13:29'));
  assert.equal(a.stale.behind, 2);
  assert.equal(a.stale.show, false);
  assert.equal(a.stale.text, null);
  const b = buildSkylineModel(SAMPLE, {}, tpe('2026-10-06T13:30'));
  assert.equal(b.stale.behind, 3);
  assert.equal(b.stale.show, true);
  assert.equal(b.stale.text, '資料停在 10/1');
});

test('spec「斷網三天」：資料停在 10/01、三個交易日（10/2、10/5、10/6）抓取失敗 → 照常 20 根 K＋20MA，並顯示資料日期', () => {
  const m = buildSkylineModel(SAMPLE, {}, tpe('2026-10-06T18:00'));
  assert.equal(m.candles.length, 20);
  assert.equal(m.maCount, 20);
  assert.equal(m.candles.at(-1).date, '2026-10-01');
  assert.equal(m.stale.text, '資料停在 10/1');
});

test('buildSkylineModel：讀檔與資料不足的警告往上傳；缺資料拋錯', () => {
  const m = buildSkylineModel({ twii_daily: DAILY.slice(-30), holidays: SAMPLE.holidays }, {}, tpe('2026-10-02T12:00'));
  assert.equal(m.warnings.length, 1);
  assert.equal(m.maCount, 11);
  assert.throws(() => buildSkylineModel({ holidays: [] }, {}, tpe('2026-10-02T12:00')), /缺少日 K 資料/);
});
