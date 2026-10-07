// host/tests/wallpapers/astrolabe-sessions.test.mjs
//
// 星盤（task 3.3）時段展開、狀態判定、「接下來」清單的單元測試。純函式在
// host/ui/wallpapers/lib/astrolabe-sessions.mjs，不依賴 DOM。
// 執行：`node --test "host/tests/wallpapers/*.test.mjs"`。
//
// 一律明確帶時區（`tpe()` 以 Asia/Taipei 解讀不帶時區的時刻；computeAstrolabe 的顯示時區明寫），
// 結果不受執行機器的系統時區影響。時段表與休市表一律取自內建預設檔（與頁面相同的來源）。

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { parseInstant } from '../../ui/wallpapers/lib/core.mjs';
import {
  FUTURE_HOURS,
  PAST_HOURS,
  STATUS_TEXT,
  UPCOMING_MAX,
  clipToWindow,
  computeAstrolabe,
  expandMarket,
  formatUtcOffset,
  parseDays,
} from '../../ui/wallpapers/lib/astrolabe-sessions.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const CFG = JSON.parse(readFileSync(path.join(HERE, '..', '..', 'ui', 'wallpapers', 'config', 'wallpaper-config.default.json'), 'utf8'));
const clone = (v) => JSON.parse(JSON.stringify(v));

const TPE_TZ = 'Asia/Taipei';
/** 不帶時區的時刻一律以台北當地時間解讀。 */
const tpe = (s) => {
  const v = parseInstant(s, TPE_TZ);
  assert.ok(Number.isFinite(v), `測試時刻不合法：${s}`);
  return v;
};
const model = (s, tz = TPE_TZ, cfg = CFG) => computeAstrolabe(cfg, tpe(s), tz);
const mk = (m, id) => m.markets.find((x) => x.id === id);
const upc = (m) => m.upcoming.map((e) => `${e.timeText} ${e.name}${e.what}`);
const market = (id, cfg = CFG) => cfg.markets.find((x) => x.id === id);
const iso = (ms) => new Date(ms).toISOString().slice(0, 16) + 'Z';
/** 某市場展開後、起點落在某交易所當地日期的時段（kind:label）。 */
const segsOnDay = (m, id, day) =>
  mk(m, id)
    .segments.filter((s) => s.day === day)
    .map((s) => `${s.kind}:${s.label}`);

// ── spec「星盤呈現六市場交易時段」────────────────────────────────────────────────────

test('spec 美股開盤時刻：10 月某週一 21:30 台北 → 紐約交易中，接下來含倫敦收盤 23:35', () => {
  for (const day of ['2026-10-05', '2026-10-12', '2026-10-19']) {
    const m = model(`${day}T21:30`);
    assert.equal(mk(m, 'NYC').statusText, '交易中', day);
    assert.ok(upc(m).includes('23:35 倫敦收盤'), `${day}：${upc(m).join('、')}`);
  }
  // 完整清單（週一 10/05，首爾當天休市不影響隔天開盤）
  assert.deepEqual(upc(model('2026-10-05T21:30')), ['23:35 倫敦收盤', '04:00 紐約收盤', '08:00 東京開盤', '08:00 首爾開盤']);
});

test('spec 美國標準時間：1 月某週一 21:30 台北 → 紐約盤前，接下來第一筆為紐約開盤 22:30', () => {
  for (const day of ['2027-01-04', '2027-01-11', '2027-01-25']) {
    const m = model(`${day}T21:30`);
    assert.equal(mk(m, 'NYC').statusText, '盤前', day);
    assert.equal(upc(m)[0], '22:30 紐約開盤', day);
  }
});

test('spec 盤後與夜盤重疊：交易日 17:30 台北 → 首爾 NXT 盤後與 KOSPI200 夜盤兩段都存在，狀態取「盤後」', () => {
  const m = model('2026-10-06T17:30');
  const sel = mk(m, 'SEL');
  const active = sel.segments.filter((s) => s.active).map((s) => s.kind).sort();
  assert.deepEqual(active, ['night', 'post']);
  assert.equal(sel.status, 'post');
  assert.equal(sel.statusText, '盤後');
  // 兩段在可見窗口內都有長度（畫得出來）
  for (const s of sel.segments.filter((x) => x.active)) assert.ok(clipToWindow(s.h0, s.h1), `${s.label} 不在可見窗口`);
});

test('spec 台指期結算日：當月第三個週三，台北當晚沒有夜盤時段', () => {
  // 2026-10-21、2026-11-18、2026-07-15 是第三個週三；前一天、隔天、第四個週三照常有夜盤
  for (const [wed, before, after] of [
    ['2026-10-21', '2026-10-20', '2026-10-22'],
    ['2026-11-18', '2026-11-17', '2026-11-19'],
    ['2026-07-15', '2026-07-14', '2026-07-16'],
  ]) {
    const m = model(`${wed}T16:00`);
    assert.deepEqual(segsOnDay(m, 'TPE', wed).filter((s) => s.startsWith('night')), [], `${wed} 不該有夜盤`);
    assert.ok(segsOnDay(m, 'TPE', before).some((s) => s.startsWith('night')), `${before} 應有夜盤`);
    assert.ok(segsOnDay(m, 'TPE', after).some((s) => s.startsWith('night')), `${after} 應有夜盤`);
    assert.equal(mk(m, 'TPE').statusText, '休息', `${wed} 16:00 無夜盤 → 休息`);
  }
  const m4 = model('2026-10-28T16:00'); // 第四個週三
  assert.equal(mk(m4, 'TPE').statusText, '夜盤');
});

test('「接下來」最多四筆，且午休中斷不算收盤（東京前場 11:30 結束不列）', () => {
  assert.equal(UPCOMING_MAX, 4);
  const m = model('2026-10-06T10:00'); // 東京 11:00，前場進行中
  assert.ok(m.upcoming.length <= 4);
  assert.equal(m.upcoming.length, 4);
  const tyo = m.upcoming.filter((e) => e.marketId === 'TYO');
  assert.deepEqual(tyo.map((e) => `${e.timeText}${e.what}`), ['14:30收盤'], '東京下一筆是 15:30 JST 收盤（台北 14:30），不是 11:30 午休');
  assert.deepEqual(upc(m), ['13:30 台北收盤', '14:30 東京收盤', '14:30 首爾收盤', '15:00 倫敦開盤']);
  // 東京午休當下（台北 11:00 ＝東京 12:00）：狀態是「休息」，展開的時段沒有在 11:30 標為收盤
  const lunch = model('2026-10-06T11:00');
  assert.equal(mk(lunch, 'TYO').statusText, '休息');
  const closes = mk(lunch, 'TYO').segments.filter((s) => s.day === '2026-10-06' && s.isClose);
  assert.deepEqual(closes.map((s) => s.label), ['收盤集合競價']);
  // 開盤只算第一段正規交易（後場 12:30 不是開盤）
  const opens = mk(lunch, 'TYO').segments.filter((s) => s.day === '2026-10-06' && s.isOpen);
  assert.deepEqual(opens.map((s) => s.label), ['前場']);
});

test('「接下來」只看 24 小時內、嚴格晚於現在；正在開盤的那一刻不列為「接下來」', () => {
  const m = model('2026-10-05T21:30'); // 紐約 09:30 正好開盤
  assert.ok(!upc(m).includes('21:30 紐約開盤'));
  for (const e of m.upcoming) {
    assert.ok(e.t > m.nowMs && e.t - m.nowMs < 24 * 3600000);
  }
  // 週六中午：24 小時內沒有任何開收盤 → 空清單
  assert.deepEqual(model('2026-10-03T12:00').upcoming, []);
});

test('狀態文字涵蓋交易中、集合競價、盤前、盤後、夜盤、休息', () => {
  assert.deepEqual(STATUS_TEXT, { regular: '交易中', auction: '集合競價', pre: '盤前', post: '盤後', night: '夜盤', idle: '休息' });
  // 精度 15 分鐘：起點落在 15 分鐘格點上的競價段才會顯示「集合競價」——香港開市前 09:00–09:30、
  // 香港收市競價 16:00–16:10、倫敦收盤競價 16:30–16:35；起點不在格點上的（倫敦開盤 07:50、東京 15:25、
  // 首爾 15:20、台北 13:25）取整後碰不到，例如倫敦 14:55（台北）會取整成 14:45（期貨現貨前）
  assert.equal(mk(model('2026-10-06T09:15'), 'HKG').statusText, '集合競價');
  assert.equal(mk(model('2026-10-06T16:00'), 'HKG').statusText, '集合競價');
  assert.equal(mk(model('2026-10-06T13:25'), 'TPE').statusText, '交易中', '台北 13:25 取整成 13:15，仍在盤中');
  assert.equal(mk(model('2026-10-06T14:55'), 'LON').statusText, '夜盤');
  // 台北 23:35 取整成 23:30＝倫敦 16:30 收盤競價（倫敦 CPX 16:35–16:40 同樣碰不到格點）
  assert.equal(mk(model('2026-10-06T23:35'), 'LON').statusText, '集合競價');
  // 紐約 16:15 EDT（台北 04:15）：盤後與 E-mini 16:00–17:00 重疊 → 盤後
  assert.equal(mk(model('2026-10-07T04:15'), 'NYC').statusText, '盤後');
  // 週六中午：各市場皆休息
  const sat = model('2026-10-03T12:00');
  for (const x of sat.markets) assert.equal(x.statusText, '休息', x.id);
});

test('精度 15 分鐘：「現在」向下取整；時間以顯示時區呈現', () => {
  const m = model('2026-10-05T21:44');
  assert.equal(m.nowMs, tpe('2026-10-05T21:30'));
  assert.equal(m.nowText, '21:30');
  assert.equal(m.tzText, 'UTC+8');
  // 同一刻以東京時區顯示：現在 22:30、倫敦收盤 00:35
  const tokyo = computeAstrolabe(CFG, tpe('2026-10-05T21:44'), 'Asia/Tokyo');
  assert.equal(tokyo.nowText, '22:30');
  assert.equal(tokyo.tzText, 'UTC+9');
  assert.equal(tokyo.upcoming[0].timeText, '00:35');
  // 狀態與顯示時區無關
  assert.deepEqual(tokyo.markets.map((x) => x.statusText), m.markets.map((x) => x.statusText));
});

test('可見窗口：現在之前 10 小時到之後 12 小時；clipToWindow 只保留窗口內部分', () => {
  assert.equal(PAST_HOURS, 10);
  assert.equal(FUTURE_HOURS, 12);
  assert.deepEqual(clipToWindow(-15, -9), [-10, -9]);
  assert.deepEqual(clipToWindow(11, 20), [11, 12]);
  assert.equal(clipToWindow(-15, -10), null);
  assert.equal(clipToWindow(12, 14), null);
});

test('formatUtcOffset：整點與非整點時區', () => {
  assert.equal(formatUtcOffset(480), 'UTC+8');
  assert.equal(formatUtcOffset(-300), 'UTC−5');
  assert.equal(formatUtcOffset(330), 'UTC+5:30');
  assert.equal(formatUtcOffset(-570), 'UTC−9:30');
  assert.equal(formatUtcOffset(0), 'UTC+0');
});

// ── 美歐夏令切換週 ───────────────────────────────────────────────────────────────────

test('夏令切換：秋季歐洲先回標準時間（10/25）、美國後回（11/1），錯開的那週各自換算', () => {
  // 10/19 週一：兩地都是夏令 → 倫敦收盤 23:35、紐約 21:30 開盤
  let m = model('2026-10-19T21:30');
  assert.equal(mk(m, 'NYC').statusText, '交易中');
  assert.equal(upc(m)[0], '23:35 倫敦收盤');
  // 10/26–10/30：倫敦已是 GMT（收盤 00:35）、紐約仍是 EDT（21:30 開盤）
  for (const day of ['2026-10-26', '2026-10-27', '2026-10-28', '2026-10-29', '2026-10-30']) {
    m = model(`${day}T21:30`);
    assert.equal(mk(m, 'NYC').statusText, '交易中', day);
    assert.equal(upc(m)[0], '00:35 倫敦收盤', day);
  }
  // 11/2 週一：兩地都是標準時間 → 紐約 21:30 只是盤前、22:30 開盤
  m = model('2026-11-02T21:30');
  assert.equal(mk(m, 'NYC').statusText, '盤前');
  assert.deepEqual(upc(m).slice(0, 2), ['22:30 紐約開盤', '00:35 倫敦收盤']);
});

test('夏令切換：春季美國先進夏令（2027/3/14）、歐洲後進（3/28），錯開的那兩週各自換算', () => {
  // 3/8 週一：兩地標準時間
  let m = model('2027-03-08T21:30');
  assert.equal(mk(m, 'NYC').statusText, '盤前');
  // 3/15–3/26：紐約 EDT（21:30 開盤）、倫敦 GMT（收盤 00:35）
  for (const day of ['2027-03-15', '2027-03-19', '2027-03-22', '2027-03-25']) {
    m = model(`${day}T21:30`);
    assert.equal(mk(m, 'NYC').statusText, '交易中', day);
    assert.equal(upc(m)[0], '00:35 倫敦收盤', day);
  }
  // 3/29 週一：倫敦復活節星期一休市；3/30 週二兩地都是夏令 → 倫敦收盤 23:35
  m = model('2027-03-30T21:30');
  assert.equal(upc(m)[0], '23:35 倫敦收盤');
});

test('夏令切換當天：時段以交易所當地時刻定義，UTC 隨夏令移動', () => {
  // E-mini 週日 18:00 ET：10/25 是 EDT（22:00Z）、11/1 已是 EST（23:00Z）
  const es = (day, now) =>
    expandMarket(market('NYC'), tpe(now), CFG).find((s) => s.day === day && s.kind === 'night' && s.label.startsWith('E-mini') && s.t1 - s.t0 > 3 * 3600000);
  assert.equal(iso(es('2026-10-25', '2026-10-26T10:00').t0), '2026-10-25T22:00Z');
  assert.equal(iso(es('2026-11-01', '2026-11-02T10:00').t0), '2026-11-01T23:00Z');
  // 該段跨午夜的終點（次日 09:30 ET）同樣依當地時刻
  assert.equal(iso(es('2026-11-01', '2026-11-02T10:00').t1), '2026-11-02T14:30Z');
  // 倫敦期貨（現貨前）01:00：10/23 週五 BST（00:00Z）、10/26 週一 GMT（01:00Z）
  const lon = (day, now) => expandMarket(market('LON'), tpe(now), CFG).find((s) => s.day === day && s.start === '01:00');
  assert.equal(iso(lon('2026-10-23', '2026-10-23T12:00').t0), '2026-10-23T00:00Z');
  assert.equal(iso(lon('2026-10-26', '2026-10-26T12:00').t0), '2026-10-26T01:00Z');
});

// ── Overnight 生效日（2026-12-06）前後 ──────────────────────────────────────────────

test('Overnight 生效日前後：12/3（週四）不畫、12/6（週日）起畫；狀態跟著改', () => {
  const ovn = (m) => mk(m, 'NYC').segments.filter((s) => s.label.startsWith('Overnight'));
  // 台北 12/4 10:00 ＝紐約 12/3 21:00：尚未生效 → 只有 E-mini（夜盤）
  let m = model('2026-12-04T10:00');
  assert.deepEqual(ovn(m), []);
  assert.equal(mk(m, 'NYC').statusText, '夜盤');
  // 台北 12/6 10:00 ＝紐約 12/5 週六 21:00：週六無任何時段
  m = model('2026-12-06T10:00');
  assert.equal(mk(m, 'NYC').statusText, '休息');
  // 台北 12/7 10:00 ＝紐約 12/6 週日 21:00：Overnight 生效當天 → 盤前（與 E-mini 重疊，取盤前）
  m = model('2026-12-07T10:00');
  const o = ovn(m).filter((s) => s.day === '2026-12-06');
  assert.equal(o.length, 1);
  assert.equal(iso(o[0].t0), '2026-12-07T02:00Z'); // 21:00 EST
  assert.equal(iso(o[0].t1), '2026-12-07T09:00Z'); // 隔天 04:00 EST
  assert.equal(mk(m, 'NYC').statusText, '盤前');
});

test('spec 尚未生效的時段：生效日是下個月時，本月的星盤不顯示該時段', () => {
  // 11 月中（紐約週日晚上）：Overnight 生效日 12/6 是下個月 → 不顯示
  const m = model('2026-11-16T10:00');
  assert.deepEqual(mk(m, 'NYC').segments.filter((s) => s.label.startsWith('Overnight')), []);
  // 自訂設定：把台北盤後定價的生效日設為下個月 1 日 → 本月不畫、下個月畫
  const cfg = clone(CFG);
  market('TPE', cfg).segments.find((s) => s.kind === 'post').effective = '2026-11-01';
  const oct = computeAstrolabe(cfg, tpe('2026-10-06T14:15'), TPE_TZ);
  assert.equal(mk(oct, 'TPE').segments.filter((s) => s.kind === 'post').length, 0);
  assert.equal(mk(oct, 'TPE').statusText, '休息');
  const nov = computeAstrolabe(cfg, tpe('2026-11-03T14:15'), TPE_TZ);
  assert.equal(mk(nov, 'TPE').statusText, '盤後');
});

// ── 跨日夜盤 ─────────────────────────────────────────────────────────────────────────

test('跨日夜盤：起點日的時段延伸到隔天，凌晨仍判定為夜盤', () => {
  // 台指期夜盤 週一 15:00 → 週二 05:00
  let m = model('2026-10-06T02:00');
  const tpeNight = mk(m, 'TPE').segments.find((s) => s.active);
  assert.equal(tpeNight.kind, 'night');
  assert.equal(tpeNight.day, '2026-10-05');
  assert.equal(iso(tpeNight.t0), '2026-10-05T07:00Z');
  assert.equal(iso(tpeNight.t1), '2026-10-05T21:00Z');
  assert.equal(mk(m, 'TPE').statusText, '夜盤');
  // 週五夜盤延伸到週六 05:00；週六 15:00 沒有新的夜盤
  m = model('2026-10-03T02:00');
  assert.equal(mk(m, 'TPE').statusText, '夜盤');
  assert.equal(mk(model('2026-10-03T15:30'), 'TPE').statusText, '休息');
  // 恆指期貨 17:00 → 03:00
  assert.equal(mk(model('2026-10-07T02:30'), 'HKG').statusText, '夜盤');
  assert.equal(mk(model('2026-10-07T03:00'), 'HKG').statusText, '休息');
  // 紐約 E-mini 週日 18:00 ET 開始（台北週一 06:00，標準時間 07:00）
  assert.equal(mk(model('2026-11-09T07:00'), 'NYC').statusText, '夜盤');
});

// ── spec「星盤的休市日與生效日」與休市規則（controller rulings）──────────────────────────

test('spec 美國感恩節：紐約環變暗並標示「休市（感恩節）」', () => {
  const m = model('2026-11-26T21:30'); // 紐約 11/26 08:30
  const nyc = mk(m, 'NYC');
  assert.equal(nyc.status, 'closed');
  assert.equal(nyc.statusText, '休市（感恩節）');
  assert.equal(nyc.holiday.name, '感恩節');
  assert.equal(nyc.dimmed, true);
  // 當天（紐約日期）的時段一律不畫，接下來不含紐約當天的開收盤
  assert.deepEqual(segsOnDay(m, 'NYC', '2026-11-26'), []);
  assert.ok(!m.upcoming.some((e) => e.marketId === 'NYC'), upc(m).join('、'));
  // 台北 11/27 07:00 ＝紐約 11/26 18:00：紐約當地仍是感恩節
  assert.equal(mk(model('2026-11-27T07:00'), 'NYC').statusText, '休市（感恩節）');
  // 其他市場不受影響、不變暗
  for (const x of m.markets.filter((y) => y.id !== 'NYC')) assert.equal(x.dimmed, false, x.id);
});

test('休市 ruling：跨午夜時段以起點的當地日期查休市表（前一晚開的整段照畫、休市當晚不開）', () => {
  // 紐約：11/25（週三）18:00 開始的 E-mini 跨進感恩節 → 整段照畫到 11/26 09:30
  const m = model('2026-11-26T21:30');
  const es = mk(m, 'NYC').segments.filter((s) => s.day === '2026-11-25' && s.kind === 'night' && s.start === '18:00');
  assert.equal(es.length, 1);
  assert.equal(iso(es[0].t1), '2026-11-26T14:30Z');
  // 台北：10/9 國慶日（補假）。10/8 15:00 的夜盤照畫到 10/9 05:00；10/9 當晚不開
  const t = model('2026-10-09T02:00');
  const tpeSegs = mk(t, 'TPE').segments;
  assert.ok(tpeSegs.some((s) => s.day === '2026-10-08' && s.kind === 'night'));
  assert.deepEqual(segsOnDay(t, 'TPE', '2026-10-09'), []);
  // 休市當天標示優先：即使前一晚的夜盤還在進行，狀態仍是「休市（假日名）」
  assert.ok(tpeSegs.some((s) => s.active && s.kind === 'night'));
  assert.equal(mk(t, 'TPE').statusText, '休市（國慶日（補假））');
  assert.equal(mk(t, 'TPE').dimmed, true);
});

test('休市表 unknown（through 之後）照一般交易日畫', () => {
  // 台北 2027 尚未公布（無年度鍵）→ 2027-01-04 週一照常
  const m = model('2027-01-04T10:00');
  assert.equal(mk(m, 'TPE').statusText, '交易中');
  assert.equal(mk(m, 'TPE').dimmed, false);
  // 香港 2027 只到 10/08：之後的平日照常
  const h = model('2027-11-15T10:00');
  assert.equal(mk(h, 'HKG').statusText, '交易中');
});

test('半日市 ruling：非夜盤時段截到 close、起點 ≥ close 的非夜盤時段不畫、夜盤照畫', () => {
  // 紐約 11/27 感恩節隔天 13:00 收
  let m = model('2026-11-27T22:00');
  const nyc = mk(m, 'NYC').segments.filter((s) => s.day === '2026-11-27');
  const by = (label) => nyc.find((s) => s.label === label);
  assert.equal(iso(by('正規交易').t1), '2026-11-27T18:00Z'); // 13:00 EST
  assert.equal(by('盤後'), undefined, '盤後 16:00 起點 ≥ 13:00，不畫');
  assert.ok(by('盤前'), '盤前 04:00–09:30 照畫');
  assert.ok(nyc.some((s) => s.kind === 'night' && s.start === '16:00'), 'E-mini 16:00–17:00 是 night，照畫');
  assert.ok(nyc.find((s) => s.isClose).label === '正規交易');
  assert.ok(upc(m).includes('02:00 紐約收盤'), upc(m).join('、'));
  // 香港 12/24 平安夜 12:00 收：開市前競價保留、收市競價不畫、期貨夜盤照畫
  m = model('2026-12-24T10:00');
  const hkg = mk(m, 'HKG').segments.filter((s) => s.day === '2026-12-24');
  assert.deepEqual(hkg.map((s) => `${s.kind}:${s.start}`).sort(), ['auction:09:00', 'night:17:00', 'regular:09:30'].sort());
  assert.equal(iso(hkg.find((s) => s.kind === 'regular').t1), '2026-12-24T04:00Z');
  assert.ok(upc(m).includes('12:00 香港收盤'), upc(m).join('、'));
  // 倫敦 12/24 12:30 收：期貨（現貨前）、開盤競價保留；連續交易截到 12:30；收盤競價、CPX 不畫；期貨（現貨後）照畫
  m = model('2026-12-24T19:00');
  const lon = mk(m, 'LON').segments.filter((s) => s.day === '2026-12-24');
  assert.deepEqual(lon.map((s) => `${s.kind}:${s.start}`).sort(), ['auction:07:50', 'night:01:00', 'night:16:35', 'regular:08:00'].sort());
  assert.equal(iso(lon.find((s) => s.kind === 'regular').t1), '2026-12-24T12:30Z');
  assert.equal(lon.find((s) => s.isClose).kind, 'regular');
  // 半日市不是休市：不變暗、狀態照常
  assert.equal(mk(model('2026-11-27T23:00'), 'NYC').statusText, '交易中');
  assert.equal(mk(model('2026-11-27T23:00'), 'NYC').dimmed, false);
});

// ── 設定檔防呆 ───────────────────────────────────────────────────────────────────────

test('parseDays：區間、逗號、跨週末；不認得的星期回錯誤而不是無窮迴圈', () => {
  assert.deepEqual(parseDays('Mon-Fri').days, [1, 2, 3, 4, 5]);
  assert.deepEqual(parseDays('Sun-Thu').days, [0, 1, 2, 3, 4]);
  assert.deepEqual(parseDays('Fri-Mon').days, [5, 6, 0, 1]);
  assert.deepEqual(parseDays('Mon,Wed').days, [1, 3]);
  assert.equal(parseDays('Mon-Fri').error, null);
  assert.ok(parseDays('Mon-Fry').error);
  assert.deepEqual(parseDays('Mon-Fry').days, []);
  assert.ok(parseDays(42).error);
});

test('設定錯誤（星期拼錯、時間格式錯、skip 格式錯）：該時段略過並回警告，不拋錯、不卡住', () => {
  const cfg = clone(CFG);
  const tpeSegs = market('TPE', cfg).segments;
  tpeSegs[0].days = 'Mon-Fry';
  tpeSegs[1].start = '25:00';
  tpeSegs[3].skip = 'third-wednesday';
  const m = computeAstrolabe(cfg, tpe('2026-10-06T10:00'), TPE_TZ);
  assert.ok(m.warnings.some((w) => w.includes('Mon-Fry')), m.warnings.join('；'));
  assert.ok(m.warnings.some((w) => w.includes('25:00')), m.warnings.join('；'));
  assert.ok(m.warnings.some((w) => w.includes('third-wednesday')), m.warnings.join('；'));
  assert.equal(mk(m, 'TPE').segments.filter((s) => s.label === '盤中').length, 0);
  // 內建預設檔沒有任何警告
  assert.deepEqual(model('2026-10-06T10:00').warnings, []);
});

test('effective 格式錯誤（2026-12-6、不存在的日期、非字串）：該時段略過並回警告，不會靜默晚一年才生效', () => {
  for (const bad of ['2026-12-6', '2026-02-30', '20261206', 20261206]) {
    const cfg = clone(CFG);
    market('NYC', cfg).segments.find((s) => s.label.startsWith('Overnight')).effective = bad;
    const m = computeAstrolabe(cfg, tpe('2027-01-05T10:00'), TPE_TZ); // 紐約 1/4 週一 21:00，Overnight 應已生效
    assert.ok(m.warnings.some((w) => w.includes('effective') && w.includes(String(bad))), `${bad}：${m.warnings.join('；')}`);
    assert.deepEqual(mk(m, 'NYC').segments.filter((s) => s.label.startsWith('Overnight')), [], `${bad}：格式錯的時段不畫`);
  }
  // 格式正確則照常：同一時刻 Overnight 已生效 → 盤前
  const ok = model('2027-01-05T10:00');
  assert.equal(mk(ok, 'NYC').statusText, '盤前');
  assert.deepEqual(ok.warnings, []);
});

test('時段表順序與內容取自設定（不寫死市場）：自訂設定只有兩個市場時只畫兩條環', () => {
  const cfg = clone(CFG);
  cfg.markets = cfg.markets.filter((x) => x.id === 'TPE' || x.id === 'NYC');
  const m = computeAstrolabe(cfg, tpe('2026-10-05T21:30'), TPE_TZ);
  assert.deepEqual(m.markets.map((x) => x.id), ['TPE', 'NYC']);
  // 自訂休市日（使用者新增）也會生效
  cfg.holidays.NYC['2026'].days.push({ date: '2026-10-05', name: '測試假日' });
  const h = computeAstrolabe(cfg, tpe('2026-10-05T21:30'), TPE_TZ);
  assert.equal(mk(h, 'NYC').statusText, '休市（測試假日）');
});

// ── 4.1（3.2 延後項 N3）：壞的市場元素不讓整頁繪圖中斷 ───────────────────────────────────

test('computeAstrolabe：markets 含 null、缺 id、tz 打錯的元素略過並記警告，其餘市場照畫', () => {
  const cfg = JSON.parse(JSON.stringify(CFG));
  const tpeMarket = cfg.markets.find((m) => m.id === 'TPE');
  const badTz = { ...cfg.markets.find((m) => m.id === 'TYO'), tz: 'Asia/Tokio' };
  cfg.markets = [null, { name: '沒有 id', tz: 'Asia/Tokyo', segments: [] }, badTz, tpeMarket];
  cfg.holidays.TPE['2026'].days.unshift(null);
  let m;
  assert.doesNotThrow(() => {
    m = computeAstrolabe(cfg, tpe('2026-10-05T10:00'), TPE_TZ);
  });
  assert.deepEqual(
    m.markets.map((x) => x.id),
    ['TPE'],
  );
  const w = m.warnings.join('\n');
  assert.match(w, /markets\[0\]/);
  assert.match(w, /markets\[1\]/);
  assert.match(w, /Asia\/Tokio/);
  assert.match(w, /holidays\.TPE\.2026\.days\[0\]/);
});
