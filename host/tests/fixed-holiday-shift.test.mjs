// host/tests/fixed-holiday-shift.test.mjs
//
// change fixed-events-holiday-shift（design.md D1／D2）：台股固定事件遇非交易日順延。
// - `nextTradingDay(d, holidays)`：交易日回傳原日期，否則往後第一個交易日（上限 30 天，
//   走到就回傳原日期）。
// - `fixedOccurrences(today, holidays, monthsAhead)`：台指期結算、財報截止、月營收截止順延，
//   季結算（第三個週五，美股商品）不依台灣休市日調整；從上個月起算（月底財報可能順延進本月）。
// 休市日集合是 `isoDate` 字串（同 `tw_events.json` 的 `holidays`）。
//
// 執行：node --test host/tests/fixed-holiday-shift.test.mjs

import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { fixedOccurrences, isoDate, nextTradingDay } from '../ui/common.js';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

const d = (y, m, day) => new Date(y, m - 1, day);
const iso = (x) => isoDate(x);

// 2026 春節（TWSE holidaySchedule：2/12、2/13 為「市場無交易，僅辦理結算交割」，2/16–2/20 春節）。
const SPRING_2026 = new Set([
  '2026-02-12', '2026-02-13', '2026-02-16', '2026-02-17', '2026-02-18', '2026-02-19', '2026-02-20',
]);

const find = (occ, tag, origIso) => occ.find((o) => o.tag === tag && iso(o.orig) === origIso);

test('nextTradingDay：交易日回傳同一天', () => {
  const wed = d(2026, 10, 21);
  assert.equal(nextTradingDay(wed, new Set()).getTime(), wed.getTime());
});

test('nextTradingDay：週六、週日順延到週一', () => {
  assert.equal(iso(nextTradingDay(d(2026, 10, 10), new Set())), '2026-10-12');
  assert.equal(iso(nextTradingDay(d(2026, 10, 11), new Set())), '2026-10-12');
});

test('nextTradingDay：連假接週末一路順延', () => {
  assert.equal(iso(nextTradingDay(d(2026, 2, 12), SPRING_2026)), '2026-02-23');
});

test('nextTradingDay：30 天內找不到交易日就回傳原日期', () => {
  const all = new Set();
  for (let i = 0; i <= 40; i++) all.add(iso(new Date(2026, 4, 1 + i)));
  const start = d(2026, 5, 1);
  assert.equal(nextTradingDay(start, all).getTime(), start.getTime());
});

test('交易日的事件不動、不帶說明', () => {
  const occ = fixedOccurrences(d(2026, 10, 18), new Set(), 0);
  const tx = find(occ, '期權', '2026-10-21');
  assert.ok(tx, '應有 10/21 台指期結算');
  assert.equal(tx.date.getTime(), tx.orig.getTime());
  assert.equal(tx.note, null);
});

test('台指期結算遇春節：2/18 → 2/23，說明為期交所規則', () => {
  const occ = fixedOccurrences(d(2026, 2, 22), SPRING_2026, 0);
  const tx = find(occ, '期權', '2026-02-18');
  assert.ok(tx);
  assert.equal(iso(tx.date), '2026-02-23');
  assert.match(tx.note, /期交所/);
});

test('月營收截止 10/10（週六）→ 10/12，說明為推定', () => {
  const occ = fixedOccurrences(d(2026, 10, 11), new Set(['2026-10-09']), 0);
  const rev = find(occ, '營收', '2026-10-10');
  assert.ok(rev);
  assert.equal(rev.label, '9月營收公布截止（10日前）');
  assert.equal(iso(rev.date), '2026-10-12');
  assert.match(rev.note, /推定/);
});

test('Q3 財報截止 11/14（週六）→ 11/16，說明為推定', () => {
  const occ = fixedOccurrences(d(2026, 11, 15), new Set(), 0);
  const fin = find(occ, '財報', '2026-11-14');
  assert.ok(fin);
  assert.equal(fin.label, 'Q3 財報公布截止');
  assert.equal(iso(fin.date), '2026-11-16');
  assert.match(fin.note, /推定/);
});

test('季結算不依台灣休市日調整，標籤不含台股', () => {
  // 2026-03 第三個週五＝3/20；故意放進休市日集合。
  const occ = fixedOccurrences(d(2026, 3, 15), new Set(['2026-03-20']), 0);
  const q = find(occ, '季結算', '2026-03-20');
  assert.ok(q);
  assert.equal(q.label, '那指・道瓊期貨季度結算');
  assert.equal(iso(q.date), '2026-03-20');
  assert.equal(q.note, null);
});

test('上個月月底的財報截止順延進本月（3/31 週日 → 4/1）', () => {
  const occ = fixedOccurrences(d(2024, 4, 1), new Set(), 0);
  const fin = find(occ, '財報', '2024-03-31');
  assert.ok(fin, '應從上個月起算');
  assert.equal(fin.label, 'Q4＋年報 財報公布截止');
  assert.equal(iso(fin.date), '2024-04-01');
});

test('休市日清單不涵蓋明年時只依週末判斷', () => {
  const occ = fixedOccurrences(d(2026, 12, 20), SPRING_2026, 1);
  // 2027-01-10 是週日 → 1/11；2027-01-20（週三）是交易日 → 不動。
  assert.equal(iso(find(occ, '營收', '2027-01-10').date), '2027-01-11');
  const tx = find(occ, '期權', '2027-01-20');
  assert.equal(iso(tx.date), '2027-01-20');
  assert.equal(tx.note, null);
});

test('結果依順延後日期排序', () => {
  const occ = fixedOccurrences(d(2026, 2, 1), SPRING_2026, 2);
  for (let i = 1; i < occ.length; i++) {
    assert.ok(occ[i - 1].date <= occ[i].date, `第 ${i} 筆排序錯誤`);
  }
});

test('fixed.js 以 .badge.today 判斷今天徽章，不用裸 .badge', () => {
  const src = readFileSync(path.join(__dirname, '..', 'ui', 'widgets', 'fixed.js'), 'utf8');
  assert.match(src, /\.badge\.today/);
  assert.doesNotMatch(src, /querySelector\('\.badge'\)/);
});
