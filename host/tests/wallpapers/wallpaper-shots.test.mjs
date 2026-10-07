// host/tests/wallpapers/wallpaper-shots.test.mjs
//
// 截圖稿 host/tools/wallpaper-shots.mjs 的必要字型查表（task 3.4 順修 3.3 審查 low）：
// `--page` 的各種寫法（./x.html、反斜線、多餘的 ../）都要查到同一份 PAGE_REQUIRED_FONTS，
// 不能因為字串不同就靜默跳過必要字型檢查。
// 執行：`node --test "host/tests/wallpapers/*.test.mjs"`（import 截圖稿不會啟動 Edge，main 只在直接執行時跑）。

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { PAGE_REQUIRED_FONTS, normalizePagePath, requiredFontsFor } from '../../tools/wallpaper-shots.mjs';

test('normalizePagePath：./、反斜線、開頭斜線、a/../ 都正規化成同一個鍵', () => {
  const bs = String.fromCharCode(92); // 反斜線（避免原始碼裡的跳脫字元被誤改）
  for (const p of ['tearoff.html', './tearoff.html', `.${bs}tearoff.html`, '/tearoff.html', 'x/../tearoff.html', './/tearoff.html']) {
    assert.equal(normalizePagePath(p), 'tearoff.html', JSON.stringify(p));
  }
  assert.equal(normalizePagePath(`..${bs}test-fixtures${bs}tearoff-layout-negative.html`), '../test-fixtures/tearoff-layout-negative.html');
  assert.equal(normalizePagePath('./../test-fixtures/astrolabe-layout-negative.html'), '../test-fixtures/astrolabe-layout-negative.html');
});

test('requiredFontsFor：變體寫法查得到撕日曆的三種必要字型；未列出的頁面回傳 null', () => {
  const want = ['notoserif-900', 'wenkai-400', 'wenkai-700'];
  assert.deepEqual(requiredFontsFor('./tearoff.html'), want);
  assert.deepEqual(requiredFontsFor('tearoff.html'), want);
  assert.deepEqual(requiredFontsFor('./astrolabe.html'), PAGE_REQUIRED_FONTS['astrolabe.html']);
  assert.equal(requiredFontsFor('not-listed.html'), null);
});

test('PAGE_REQUIRED_FONTS 的鍵本身都是正規化後的寫法（否則永遠查不到）', () => {
  for (const k of Object.keys(PAGE_REQUIRED_FONTS)) assert.equal(normalizePagePath(k), k);
});

test('脊線與等高線（task 3.5）：兩頁與反例頁都要求 IBM Plex Mono 600 與 Noto Sans TC 500', () => {
  const want = ['plexmono-600', 'notosans-500'];
  for (const p of ['ridgeline.html', './contour.html', '../test-fixtures/ridgeline-layout-negative.html', '../test-fixtures/contour-layout-negative.html']) {
    assert.deepEqual(requiredFontsFor(p), want, p);
  }
});

test('天際線（task 3.6）：頁面與反例頁都要求 IBM Plex Mono 600 與 Noto Sans TC 500', () => {
  const want = ['plexmono-600', 'notosans-500'];
  for (const p of ['skyline.html', './skyline.html', '../test-fixtures/skyline-layout-negative.html', '../test-fixtures/skyline-stale-oldpos-negative.html']) {
    assert.deepEqual(requiredFontsFor(p), want, p);
  }
});

test('parseArgs（task 3.5 修正輪 1）：--contract-fixture／--contract-envelope 讓契約檢查 A、B 改用頁面適用的 fixture；預設不帶＝原行為', async () => {
  const { parseArgs } = await import('../../tools/wallpaper-shots.mjs');
  const d = parseArgs([]);
  assert.equal(d.contractFixture, null);
  assert.equal(d.contractEnvelope, null);
  const a = parseArgs(['--contract-checks', '--contract-fixture', '/test-fixtures/tw_events_sample.json', '--contract-envelope', '/test-fixtures/intraday-envelope.json']);
  assert.equal(a.contractChecks, true);
  assert.equal(a.contractFixture, '/test-fixtures/tw_events_sample.json');
  assert.equal(a.contractEnvelope, '/test-fixtures/intraday-envelope.json');
});
