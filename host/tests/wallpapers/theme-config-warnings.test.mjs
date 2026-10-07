// host/tests/wallpapers/theme-config-warnings.test.mjs
//
// task 4.1 修正輪 1（審查 medium）：頁面合併主題設定後一律跑 `validateThemeConfig`，巢狀設定錯誤
// （例：`days:[null]`、不存在的日期）要出現在頁面的 warnings（即宿主收到的 meta.warnings）。
// 宿主 Rust 只做頂層鍵合併，巢狀驗證的唯一呼叫點是 `wallpaper.mjs` 的 `env.withDefaults`。
//
// 頁面層測試直接在 Node 跑 `runWallpaper`：不帶 `window.__TAURI__`（bridge.js 可安全 import）、
// 字型清單給空陣列、canvas 用假物件、fixture 用 data: URL、宿主回報用替身收下 meta。
// 執行：`node --test "host/tests/wallpapers/*.test.mjs"`。

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { mergeThemeConfig } from '../../ui/wallpapers/lib/config-holidays.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const DEFAULTS = JSON.parse(
  readFileSync(path.join(HERE, '..', '..', 'ui', 'wallpapers', 'config', 'wallpaper-config.default.json'), 'utf8'),
);

/** 一個最小的頁面環境（只提供 runWallpaper 會碰到的 window／document 成員）。 */
function installFakePage() {
  globalThis.window = {
    location: { search: '', href: 'http://localhost/wallpapers/test.html', origin: 'http://localhost' },
    dispatchEvent() {
      return true;
    },
  };
  globalThis.document = { documentElement: { dataset: {} } };
}

/** 以 `{data, config}` 信封跑一次完整頁面流程，回傳 `{ state, meta }`。 */
async function runPageWithConfig(config) {
  installFakePage();
  const { runWallpaper } = await import('../../ui/wallpapers/lib/wallpaper.mjs');
  const envelope = { data: { updated: '2026-10-02T08:00:00+08:00' }, config };
  const fixture = `data:application/json,${encodeURIComponent(JSON.stringify(envelope))}`;
  let reported = null;
  const canvas = { width: 0, height: 0, getContext: () => ({}) };
  const state = await runWallpaper({
    canvas,
    fonts: [],
    search: `?w=320&h=180&fixture=${encodeURIComponent(fixture)}`,
    draw(env) {
      env.withDefaults(DEFAULTS);
    },
    report: async (outcome) => {
      reported = outcome;
      return { sent: null, error: null };
    },
  });
  return { state, meta: reported?.meta };
}

test('mergeThemeConfig：合併後再驗證巢狀內容，內建預設本身零警告', () => {
  assert.deepEqual(mergeThemeConfig(DEFAULTS, {}).warnings, []);
  assert.deepEqual(mergeThemeConfig(DEFAULTS, undefined).warnings, []);

  const bad = { holidays: { TPE: { 2026: { through: '2026-12-31', days: [null, { date: '2026-02-30', name: '不存在' }] } } } };
  const { value, warnings } = mergeThemeConfig(DEFAULTS, bad);
  const text = warnings.join('\n');
  assert.match(text, /config\.holidays\.TPE\.2026\.days\[0\] 型別應為物件/);
  assert.match(text, /config\.holidays\.TPE\.2026\.days\[1\]\.date（2026-02-30）不是 YYYY-MM-DD 的真實日期/);
  assert.deepEqual(value.holidays.TPE['2026'].days, bad.holidays.TPE['2026'].days, '合併結果照 mergeConfig（陣列整份取代）');
  assert.ok(value.holidays.TPE['2027'] === undefined || value.holidays.TPE['2027'], '其他年度不受影響');

  // mergeConfig 本身的型別警告仍在、且排在驗證警告之前
  const typed = mergeThemeConfig(DEFAULTS, { thresholds: 'x', holidays: bad.holidays });
  assert.match(typed.warnings[0], /thresholds/);
});

test('頁面：巢狀設定錯誤（days:[null]、不存在的日期）出現在頁面 warnings 與宿主 meta.warnings', async () => {
  const config = { holidays: { TPE: { 2026: { through: '2026-12-31', days: [null, { date: '2026-02-30', name: '不存在' }] } } } };
  const { state, meta } = await runPageWithConfig(config);
  assert.equal(state.phase, 'done', state.error);
  const text = state.warnings.join('\n');
  assert.match(text, /config\.holidays\.TPE\.2026\.days\[0\] 型別應為物件/);
  assert.match(text, /days\[1\]\.date（2026-02-30）不是 YYYY-MM-DD 的真實日期/);
  assert.deepEqual(meta.warnings, state.warnings, '警告隨 meta.warnings 回報宿主');
  assert.equal(new Set(state.warnings).size, state.warnings.length, '同一則警告不重複');
});

test('頁面：內建預設（config 為空物件）零警告，不影響截圖稿「有警告就失敗」的規則', async () => {
  const { state, meta } = await runPageWithConfig({});
  assert.equal(state.phase, 'done', state.error);
  assert.deepEqual(state.warnings, []);
  assert.deepEqual(meta.warnings, []);
});
