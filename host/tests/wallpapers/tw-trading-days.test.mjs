// host/tests/wallpapers/tw-trading-days.test.mjs
//
// 台股交易日與「資料落後幾個交易日」的共用判定（task 3.5；3.6 天際線也用）。
// spec「資料過期標示」：落後 ≥ 3 個交易日才在角落顯示「資料停在 M/D」，剛好落後 2 個不顯示。
// spec「脊線與等高線呈現盤中走勢」Scenario「週一早上」：週一 06:00 的最近一個已收盤交易日是上週五。
// 執行：`node --test "host/tests/wallpapers/*.test.mjs"`

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { parseInstant } from '../../ui/wallpapers/lib/core.mjs';
import {
  STALE_MIN_BEHIND,
  addDays,
  isTpeTradingDay,
  lastClosedTradingDay,
  mdText,
  staleMarker,
  tpeClosed,
  tradingDaysBehind,
} from '../../ui/wallpapers/lib/tw-trading-days.mjs';
import * as tearoffModel from '../../ui/wallpapers/lib/tearoff-model.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const REPO = path.join(HERE, '..', '..', '..');
const CFG = JSON.parse(readFileSync(path.join(REPO, 'host', 'ui', 'wallpapers', 'config', 'wallpaper-config.default.json'), 'utf8'));
const SAMPLE = JSON.parse(readFileSync(path.join(REPO, 'tests', 'fixtures', 'tw_events_sample.json'), 'utf8'));

/** 台北當地時間（不帶時區的 ISO）→ UTC 毫秒；與機器時區無關。 */
const tpe = (iso) => parseInstant(iso, 'Asia/Taipei');
/** 只有 config 休市表、資料層休市日曆為空。 */
const NO_DATA_HOLIDAYS = { holidays: [] };

test('共用模組：撕日曆仍匯出同一個 tpeClosed／addDays（3.4 的實作抽到 lib/tw-trading-days.mjs）', () => {
  assert.equal(tearoffModel.tpeClosed, tpeClosed);
  assert.equal(tearoffModel.addDays, addDays);
});

test('交易日：週末、config 休市、data.holidays 三者聯集', () => {
  assert.equal(isTpeTradingDay('2026-10-02', CFG, NO_DATA_HOLIDAYS), true); // 週五
  assert.equal(isTpeTradingDay('2026-10-03', CFG, NO_DATA_HOLIDAYS), false); // 週六
  assert.equal(isTpeTradingDay('2026-10-09', CFG, NO_DATA_HOLIDAYS), false); // config：國慶日補假
  assert.equal(isTpeTradingDay('2026-10-07', CFG, { holidays: ['2026-10-07'] }), false); // 只有 data 有
  assert.equal(isTpeTradingDay('2026-10-07', CFG, null), true);
});

test('最近一個已收盤交易日：13:30 為界（13:29 是前一個交易日，13:30 是當天）', () => {
  assert.equal(lastClosedTradingDay(tpe('2026-10-06T13:29'), CFG, NO_DATA_HOLIDAYS), '2026-10-05');
  assert.equal(lastClosedTradingDay(tpe('2026-10-06T13:29:59'), CFG, NO_DATA_HOLIDAYS), '2026-10-05');
  assert.equal(lastClosedTradingDay(tpe('2026-10-06T13:30'), CFG, NO_DATA_HOLIDAYS), '2026-10-06');
  assert.equal(lastClosedTradingDay(tpe('2026-10-06T23:59'), CFG, NO_DATA_HOLIDAYS), '2026-10-06');
  assert.equal(lastClosedTradingDay(tpe('2026-10-06T00:00'), CFG, NO_DATA_HOLIDAYS), '2026-10-05');
});

test('最近一個已收盤交易日：以台北時間判定，與「現在」用哪個時區表示無關', () => {
  // 台北 10/06 13:30 ＝ UTC 05:30 ＝ 東京 14:30 ＝ 紐約 10/06 01:30
  const ms = Date.UTC(2026, 9, 6, 5, 30);
  assert.equal(lastClosedTradingDay(ms, CFG, NO_DATA_HOLIDAYS), '2026-10-06');
  assert.equal(lastClosedTradingDay(ms - 1, CFG, NO_DATA_HOLIDAYS), '2026-10-05');
});

test('spec Scenario 週一早上：週一 06:00 的最近一個已收盤交易日是上週五，週五的資料落後 0', () => {
  const mon6 = tpe('2026-10-05T06:00');
  assert.equal(lastClosedTradingDay(mon6, CFG, NO_DATA_HOLIDAYS), '2026-10-02');
  assert.equal(tradingDaysBehind('2026-10-02', mon6, CFG, NO_DATA_HOLIDAYS), 0);
  const m = staleMarker('2026-10-02', mon6, CFG, NO_DATA_HOLIDAYS);
  assert.deepEqual(m, { dataDate: '2026-10-02', lastClosed: '2026-10-02', behind: 0, show: false, text: null });
});

test('落後 0：資料日＝最近一個已收盤交易日（收盤後、隔天開盤前）', () => {
  assert.equal(tradingDaysBehind('2026-10-01', tpe('2026-10-01T14:00'), CFG, NO_DATA_HOLIDAYS), 0);
  assert.equal(tradingDaysBehind('2026-10-01', tpe('2026-10-02T12:00'), CFG, NO_DATA_HOLIDAYS), 0);
  assert.equal(tradingDaysBehind('2026-10-01', tpe('2026-10-02T13:30'), CFG, NO_DATA_HOLIDAYS), 1);
});

test('spec Scenario 剛好落後 2 個交易日：不顯示；13:30 一過變落後 3，顯示「資料停在 10/1」', () => {
  // 10/01（四）的資料；之後的交易日：10/02（五）、10/05（一）、10/06（二）
  const before = staleMarker('2026-10-01', tpe('2026-10-06T13:29'), CFG, NO_DATA_HOLIDAYS);
  assert.equal(before.behind, 2);
  assert.equal(before.show, false);
  assert.equal(before.text, null);
  const after = staleMarker('2026-10-01', tpe('2026-10-06T13:30'), CFG, NO_DATA_HOLIDAYS);
  assert.equal(after.behind, 3);
  assert.equal(after.show, true);
  assert.equal(after.text, '資料停在 10/1');
  assert.equal(STALE_MIN_BEHIND, 3);
});

test('跨週末：週五的資料到下週二收盤後落後 2、週三收盤後落後 3（週六日不算）', () => {
  assert.equal(tradingDaysBehind('2026-10-02', tpe('2026-10-05T13:30'), CFG, NO_DATA_HOLIDAYS), 1);
  assert.equal(tradingDaysBehind('2026-10-02', tpe('2026-10-06T14:00'), CFG, NO_DATA_HOLIDAYS), 2);
  assert.equal(tradingDaysBehind('2026-10-02', tpe('2026-10-07T13:30'), CFG, NO_DATA_HOLIDAYS), 3);
  // 週末當天（週六、週日任何時刻）最近一個已收盤交易日都是週五
  assert.equal(lastClosedTradingDay(tpe('2026-10-03T15:00'), CFG, NO_DATA_HOLIDAYS), '2026-10-02');
  assert.equal(lastClosedTradingDay(tpe('2026-10-04T23:00'), CFG, NO_DATA_HOLIDAYS), '2026-10-02');
});

test('跨休市日（config）：10/08 的資料，10/09 國慶補假＋週末不算，到 10/14 收盤後才落後 3', () => {
  assert.equal(lastClosedTradingDay(tpe('2026-10-12T10:00'), CFG, NO_DATA_HOLIDAYS), '2026-10-08');
  assert.equal(tradingDaysBehind('2026-10-08', tpe('2026-10-12T10:00'), CFG, NO_DATA_HOLIDAYS), 0);
  assert.equal(tradingDaysBehind('2026-10-08', tpe('2026-10-13T14:00'), CFG, NO_DATA_HOLIDAYS), 2);
  const m = staleMarker('2026-10-08', tpe('2026-10-14T13:30'), CFG, NO_DATA_HOLIDAYS);
  assert.equal(m.behind, 3);
  assert.equal(m.text, '資料停在 10/8');
});

test('跨休市日（真實樣本）：9/24 的資料，9/25（五）與 9/28（一）休市 → 週一、週二早上都落後 0', () => {
  // 樣本 tw_events_sample.json 的 holidays 含 2026-09-25、2026-09-28（config 也有）
  assert.ok(SAMPLE.holidays.includes('2026-09-25') && SAMPLE.holidays.includes('2026-09-28'));
  assert.equal(lastClosedTradingDay(tpe('2026-09-28T06:00'), CFG, SAMPLE), '2026-09-24');
  assert.equal(tradingDaysBehind('2026-09-24', tpe('2026-09-28T06:00'), CFG, SAMPLE), 0);
  assert.equal(tradingDaysBehind('2026-09-24', tpe('2026-09-29T06:00'), CFG, SAMPLE), 0);
  assert.equal(tradingDaysBehind('2026-09-24', tpe('2026-09-29T13:30'), CFG, SAMPLE), 1);
});

test('跨休市日（只有 data.holidays 有）：資料層休市日也不算交易日', () => {
  const now = tpe('2026-10-07T13:30');
  assert.equal(tradingDaysBehind('2026-10-01', now, CFG, NO_DATA_HOLIDAYS), 4);
  assert.equal(tradingDaysBehind('2026-10-01', now, CFG, { holidays: ['2026-10-05'] }), 3);
  // data.holidays 型別錯 → 忽略，只看週末與 config
  assert.equal(tradingDaysBehind('2026-10-01', now, CFG, { holidays: 'oops' }), 4);
});

test('跨年：config 未收 2027、data.holidays 有 2027-01-01 → 12/30 的資料到 1/4 收盤後落後 2', () => {
  const d = { holidays: ['2027-01-01'] };
  assert.equal(tradingDaysBehind('2026-12-30', tpe('2027-01-04T14:00'), CFG, d), 2);
  assert.equal(lastClosedTradingDay(tpe('2027-01-02T09:00'), CFG, d), '2026-12-31');
});

test('資料日期晚於最近一個已收盤交易日（時鐘偏差）→ 落後 0、不顯示', () => {
  assert.equal(tradingDaysBehind('2026-10-07', tpe('2026-10-06T14:00'), CFG, NO_DATA_HOLIDAYS), 0);
  assert.equal(staleMarker('2026-10-07', tpe('2026-10-06T14:00'), CFG, NO_DATA_HOLIDAYS).show, false);
});

test('落後很多：半年前的資料照算，顯示標示', () => {
  const m = staleMarker('2026-04-01', tpe('2026-10-06T14:00'), CFG, NO_DATA_HOLIDAYS);
  assert.ok(m.behind > 100, String(m.behind));
  assert.equal(m.show, true);
  assert.equal(m.text, '資料停在 4/1');
});

test('資料日期不合法 → behind null、不顯示（不拋錯）', () => {
  for (const bad of ['2026-02-30', '2026/10/01', '', null, undefined, 20261001]) {
    const m = staleMarker(bad, tpe('2026-10-06T14:00'), CFG, NO_DATA_HOLIDAYS);
    assert.equal(m.behind, null, String(bad));
    assert.equal(m.show, false);
    assert.equal(m.text, null);
  }
});

test('M/D 不補零', () => {
  assert.equal(mdText('2026-10-02'), '10/2');
  assert.equal(mdText('2027-01-05'), '1/5');
  assert.equal(mdText('2026-12-31'), '12/31');
});
