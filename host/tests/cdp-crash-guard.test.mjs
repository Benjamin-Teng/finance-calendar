// host/tests/cdp-crash-guard.test.mjs
//
// fix F5（review fix-soak high）：cdp-crash.mjs 送出當機指令前，/json/list 的 page 目標必須
// 全部是宿主的頁面（tauri.localhost）；否則代表連到的是別的程式的 WebView2／Edge，必須中止、
// 一個指令都不送。這是 repro-soak-exit.ps1「埠的 Listen 行程是測試宿主的子孫」之外的第二道
// 檢查（其他 Tauri 程式也用 tauri.localhost，所以只靠這一道不夠）。
//
// 執行：node host/tests/cdp-crash-guard.test.mjs（結束碼 0＝全部通過）

import assert from 'node:assert/strict';
import { checkHostTargets, selectWidgetPage } from '../tools/cdp-crash.mjs';

let failed = 0;
function test(name, fn) {
  try {
    fn();
    console.log(`PASS  ${name}`);
  } catch (e) {
    failed++;
    console.log(`FAIL  ${name}\n      ${e.message}`);
  }
}

const hostPage = (path) => ({ type: 'page', url: `http://tauri.localhost/${path}` });

test('全部是宿主頁面 → ok，回傳這些 page', () => {
  const list = [hostPage('widget.html?id=clock'), hostPage('widget.html?id=quotes'), { type: 'service_worker', url: 'x' }];
  const r = checkHostTargets(list);
  assert.equal(r.ok, true, r.reason);
  assert.equal(r.pages.length, 2);
});

test('https 的 tauri.localhost 也算宿主頁面', () => {
  assert.equal(checkHostTargets([{ type: 'page', url: 'https://tauri.localhost/settings.html' }]).ok, true);
});

test('混入外部網頁 → 拒絕', () => {
  const r = checkHostTargets([hostPage('widget.html'), { type: 'page', url: 'https://www.example.com/' }]);
  assert.equal(r.ok, false);
  assert.match(r.reason, /example\.com/);
});

test('看似 tauri.localhost 的其他主機 → 拒絕', () => {
  assert.equal(checkHostTargets([{ type: 'page', url: 'http://tauri.localhost.evil.test/' }]).ok, false);
});

test('沒有任何 page → 拒絕', () => {
  assert.equal(checkHostTargets([]).ok, false);
  assert.equal(checkHostTargets([{ type: 'service_worker', url: 'http://tauri.localhost/sw.js' }]).ok, false);
});

test('不是陣列 → 拒絕', () => {
  assert.equal(checkHostTargets({ error: 'x' }).ok, false);
});

// verify-7.3（審查 low）：只讓「指定小工具」那一頁的 renderer 當機，不得波及 clock／macro 等其他頁。
const wpage = (id) => ({ type: 'page', url: `http://tauri.localhost/widget.html?w=${id}`, webSocketDebuggerUrl: `ws://x/${id}` });

test('selectWidgetPage：以 ?w= 完全相等挑出唯一一頁', () => {
  const r = selectWidgetPage([wpage('clock'), wpage('quotes'), wpage('macro')], 'quotes');
  assert.equal(r.ok, true, r.reason);
  assert.equal(r.page.webSocketDebuggerUrl, 'ws://x/quotes');
});

test('selectWidgetPage：不靠子字串（w=custom1 不會配到 w=custom10）', () => {
  const r = selectWidgetPage([wpage('custom10'), wpage('clock')], 'custom1');
  assert.equal(r.ok, false);
});

test('selectWidgetPage：同 id 有兩頁 → 拒絕（無法確定是哪一扇）', () => {
  assert.equal(selectWidgetPage([wpage('quotes'), wpage('quotes')], 'quotes').ok, false);
});

test('selectWidgetPage：非宿主頁面不列入', () => {
  const foreign = { type: 'page', url: 'https://example.com/widget.html?w=quotes', webSocketDebuggerUrl: 'ws://evil' };
  assert.equal(selectWidgetPage([foreign], 'quotes').ok, false);
});

if (failed) {
  console.log(`${failed} 項失敗`);
  process.exit(1);
}
console.log('全部通過');
