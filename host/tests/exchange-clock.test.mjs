// host/tests/exchange-clock.test.mjs
//
// change clock-six-exchanges（design.md D1）：時鐘小工具六個交易所的當地時間、日期與開收盤。
// 直接讀內建的 wallpaper-config.default.json（與時鐘執行期讀的是同一份）。
//
// 執行：node --test host/tests/exchange-clock.test.mjs

import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { EXCHANGES, exchangeRows } from '../ui/exchange-clock.mjs';
import { zonedToUtc } from '../ui/wallpapers/lib/core.mjs';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const CONFIG = JSON.parse(
  readFileSync(path.join(__dirname, '..', 'ui', 'wallpapers', 'config', 'wallpaper-config.default.json'), 'utf8'),
);

const at = (tz, y, mo, d, h, mi) => zonedToUtc(tz, y, mo, d, h, mi);
const row = (nowMs, code, config = CONFIG) => exchangeRows(config, nowMs).find((r) => r.code === code);

test('六列順序與簡寫：由東到西', () => {
  const rows = exchangeRows(CONFIG, Date.UTC(2026, 10, 16, 2, 0));
  assert.deepEqual(rows.map((r) => r.code), ['JP', 'KR', 'TW', 'HK', 'UK', 'US']);
  assert.deepEqual(EXCHANGES.map((e) => e.id), ['TYO', 'SEL', 'TPE', 'HKG', 'LON', 'NYC']);
});

test('台北盤中、紐約收盤：時間與日期各依當地時區', () => {
  const now = at('Asia/Taipei', 2026, 11, 16, 10, 0);
  assert.deepEqual(row(now, 'TW'), { code: 'TW', time: '10:00', date: '26-11-16', open: true });
  assert.deepEqual(row(now, 'US'), { code: 'US', time: '21:00', date: '26-11-15', open: false });
  assert.deepEqual(row(now, 'JP'), { code: 'JP', time: '11:00', date: '26-11-16', open: true });
});

test('台北開收盤的邊界分鐘', () => {
  const tw = (h, mi) => row(at('Asia/Taipei', 2026, 11, 16, h, mi), 'TW').open;
  assert.equal(tw(8, 59), false);
  assert.equal(tw(9, 0), true);
  assert.equal(tw(13, 24), true);
  assert.equal(tw(13, 29), true, '收盤集合競價算開盤');
  assert.equal(tw(13, 30), false);
  assert.equal(tw(14, 15), false, '盤後定價不算開盤');
  assert.equal(tw(16, 0), false, '台指期夜盤不算開盤');
});

test('東京午休為收盤，前後場為開盤', () => {
  const jp = (h, mi) => row(at('Asia/Tokyo', 2026, 11, 16, h, mi), 'JP').open;
  assert.equal(jp(11, 29), true);
  assert.equal(jp(11, 30), false);
  assert.equal(jp(12, 0), false);
  assert.equal(jp(12, 30), true);
  assert.equal(jp(18, 0), false, '日經期貨夜盤不算開盤');
});

test('盤前與盤後不算開盤', () => {
  const us = (h, mi) => row(at('America/New_York', 2026, 11, 16, h, mi), 'US').open;
  assert.equal(us(8, 0), false);
  assert.equal(us(9, 30), true);
  assert.equal(us(15, 59), true);
  assert.equal(us(16, 0), false);
  assert.equal(row(at('Asia/Seoul', 2026, 11, 16, 8, 30), 'KR').open, false);
  assert.equal(row(at('Asia/Seoul', 2026, 11, 16, 16, 0), 'KR').open, false);
});

test('休市日：感恩節紐約全天收盤', () => {
  const r = row(at('America/New_York', 2026, 11, 26, 11, 0), 'US');
  assert.deepEqual(r, { code: 'US', time: '11:00', date: '26-11-26', open: false });
});

test('半日市：感恩節隔天紐約 13:00 收盤', () => {
  const us = (h, mi) => row(at('America/New_York', 2026, 11, 27, h, mi), 'US').open;
  assert.equal(us(12, 59), true);
  assert.equal(us(13, 0), false);
  assert.equal(us(15, 0), false);
});

test('週末全部收盤', () => {
  const rows = exchangeRows(CONFIG, at('Asia/Taipei', 2026, 11, 14, 10, 0)); // 週六
  assert.deepEqual(rows.map((r) => r.open), [false, false, false, false, false, false]);
});

test('日光節約時間：夏令時的紐約與倫敦', () => {
  const now = Date.UTC(2026, 6, 1, 13, 30);
  assert.deepEqual(row(now, 'US'), { code: 'US', time: '09:30', date: '26-07-01', open: true });
  assert.deepEqual(row(now, 'UK'), { code: 'UK', time: '14:30', date: '26-07-01', open: true });
});

test('沒有設定檔：時間日期照常、全部收盤', () => {
  const now = at('Asia/Taipei', 2026, 11, 16, 10, 0);
  const rows = exchangeRows(null, now);
  assert.deepEqual(rows.map((r) => r.open), [false, false, false, false, false, false]);
  assert.deepEqual(rows[2], { code: 'TW', time: '10:00', date: '26-11-16', open: false });
  assert.deepEqual(rows[5], { code: 'US', time: '21:00', date: '26-11-15', open: false });
});

test('設定檔缺某個市場或內容壞掉：該列仍有時間、為收盤，其他列不受影響', () => {
  const now = at('Asia/Taipei', 2026, 11, 16, 10, 0);
  const partial = { ...CONFIG, markets: CONFIG.markets.filter((m) => m.id !== 'TPE') };
  assert.deepEqual(row(now, 'TW', partial), { code: 'TW', time: '10:00', date: '26-11-16', open: false });
  assert.equal(row(now, 'JP', partial).open, true);
  for (const broken of [{}, { markets: null }, { markets: [null, 'x'] }]) {
    const rows = exchangeRows(broken, now);
    assert.equal(rows.length, 6);
    assert.equal(rows[2].time, '10:00');
    assert.equal(rows.some((r) => r.open), false);
  }
});

test('台北休市日與倫敦開盤競價', () => {
  assert.equal(row(at('Asia/Taipei', 2027, 1, 1, 10, 0), 'TW').open, false, '2027 元旦');
  assert.equal(row(at('Asia/Taipei', 2027, 2, 3, 10, 0), 'TW').open, false, '春節前只辦結算交割');
  assert.equal(row(at('Asia/Taipei', 2027, 1, 4, 10, 0), 'TW').open, true, '新年開始交易日');
  assert.equal(row(at('Europe/London', 2026, 11, 16, 7, 49), 'UK').open, false);
  assert.equal(row(at('Europe/London', 2026, 11, 16, 7, 50), 'UK').open, true);
});

test('時區以本模組為準：設定檔的 tz 打錯時開收盤仍依正確時區', () => {
  const now = at('Asia/Taipei', 2026, 11, 16, 10, 0);
  const wrong = { ...CONFIG, markets: CONFIG.markets.map((m) => (m.id === 'TPE' ? { ...m, tz: 'America/New_York' } : m)) };
  assert.equal(row(now, 'TW', wrong).open, true);
});

test('跨年：日期的年份取兩位數', () => {
  const r = row(at('Asia/Tokyo', 2027, 1, 4, 9, 5), 'JP');
  assert.equal(r.date, '27-01-04');
  assert.equal(r.time, '09:05');
});
