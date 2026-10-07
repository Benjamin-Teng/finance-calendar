// host/tests/wallpapers/render-id.test.mjs
//
// task 4.5（design.md D2）：宿主每次渲染配一個 render id，以 query `rid` 帶給頁面；頁面在回報宿主的
// meta 原樣帶回（成功與失敗都要），宿主據此丟棄逾時後才晚到的 PNG。`rid` 是已知參數，不得產生
// 「未知參數」警告（否則每一張圖都帶警告）。
// 執行：`node --test "host/tests/wallpapers/*.test.mjs"`。

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { KNOWN_PARAMS, parseQuery } from '../../ui/wallpapers/lib/core.mjs';

const opts = { nowMs: Date.UTC(2026, 9, 2, 6, 0), localTz: 'Asia/Taipei' };

test('parseQuery：rid 是已知參數、原樣取出、不產生警告', () => {
  assert.ok(KNOWN_PARAMS.includes('rid'));
  const q = parseQuery('?w=1920&h=1080&t=2026-10-05T00%3A30%3A00Z&tz=Asia%2FTaipei&rid=17', opts);
  assert.equal(q.rid, '17');
  assert.deepEqual(q.warnings, []);
  assert.equal(parseQuery('', opts).rid, null, '沒帶＝null（獨立模式）');
});

function installFakePage(search) {
  globalThis.window = {
    location: { search, href: `http://localhost/wallpapers/test.html${search}`, origin: 'http://localhost' },
    dispatchEvent() {
      return true;
    },
  };
  globalThis.document = { documentElement: { dataset: {} } };
}

async function run(search, draw) {
  installFakePage(search);
  const { runWallpaper } = await import('../../ui/wallpapers/lib/wallpaper.mjs');
  const fixture = `data:application/json,${encodeURIComponent(JSON.stringify({ data: {}, config: {} }))}`;
  let reported = null;
  const state = await runWallpaper({
    canvas: { width: 0, height: 0, getContext: () => ({}) },
    fonts: [],
    search: `${search}&fixture=${encodeURIComponent(fixture)}`,
    draw,
    report: async (outcome) => {
      reported = outcome;
      return { sent: null, error: null };
    },
  });
  return { state, reported };
}

test('頁面：成功回報的 meta 帶回 rid', async () => {
  const { state, reported } = await run('?w=320&h=180&rid=42', () => {});
  assert.equal(state.phase, 'done', state.error);
  assert.equal(reported.ok, true);
  assert.equal(reported.meta.rid, '42');
  assert.deepEqual(reported.meta.warnings, []);
});

test('頁面：失敗回報的 meta 也帶回 rid', async () => {
  const { state, reported } = await run('?w=320&h=180&rid=43', () => {
    throw new Error('draw 拋錯');
  });
  assert.equal(state.phase, 'error');
  assert.equal(reported.ok, false);
  assert.equal(reported.meta.rid, '43');
});

test('頁面：沒帶 rid 時 meta.rid 為 null', async () => {
  const { reported } = await run('?w=320&h=180', () => {});
  assert.equal(reported.meta.rid, null);
});
