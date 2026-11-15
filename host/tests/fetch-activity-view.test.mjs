// host/tests/fetch-activity-view.test.mjs
//
// fetch-progress-and-manual-refresh task 3.2（design.md D4；specs/finance-widgets「抓取進行中的
// 顯示」、specs/widget-data-feed「尚無資料」）：鎖住 host/ui/common.js 的純函式
// `fetchActivityView`／`noDataText`／`isFetchPending`——由（快照狀態、needsData、活動狀態）決定
// 小工具顯示內容、整塊載入或無資料提示，以及無資料提示依抓取狀態區分的文字。
//
// 執行：node --test host/tests/fetch-activity-view.test.mjs

import test from 'node:test';
import assert from 'node:assert/strict';

import {
  FETCHING_TEXT,
  fetchActivityView,
  isFetchPending,
  noDataText,
} from '../ui/common.js';

const DIR = 'D:\\data\\fc';
const act = (running, enabled, last) => ({ running, enabled, last });
const view = (status, activity, needsData = true) =>
  fetchActivityView({ status, needsData, activity, dataDir: DIR });

// ── 規格的五種組合 ─────────────────────────────────────────────────────────────

test('首次安裝、首抓還沒開始（啟用、沒跑過、未進行）：整塊載入「資料抓取中…」', () => {
  assert.deepEqual(view('empty', act(false, true, 'none')), { mode: 'loading', text: FETCHING_TEXT });
  assert.equal(FETCHING_TEXT, '資料抓取中…');
});

test('一輪進行中：整塊載入（不論上一輪結果）', () => {
  for (const last of ['none', 'ok', 'failed']) {
    assert.equal(view('empty', act(true, true, last)).mode, 'loading', `last=${last}`);
  }
});

test('抓取已關閉：尚無資料、資料目錄路徑與「抓取已關閉」，不顯示載入', () => {
  for (const last of ['none', 'ok', 'failed']) {
    const v = view('empty', act(false, false, last));
    assert.equal(v.mode, 'empty', `last=${last}`);
    assert.match(v.text, /^尚無資料/);
    assert.ok(v.text.includes(DIR), '要有資料目錄路徑');
    assert.match(v.text, /抓取已關閉/);
    assert.doesNotMatch(v.text, /log\.txt/, '關閉時不叫使用者看記錄');
  }
});

test('上一輪失敗（啟用、未進行）：尚無資料與「產生 log.txt」', () => {
  const v = view('empty', act(false, true, 'failed'));
  assert.equal(v.mode, 'empty');
  assert.match(v.text, /^尚無資料/);
  assert.match(v.text, /系統匣「產生 log\.txt」/);
});

test('已有資料：照常顯示內容（不論活動狀態）', () => {
  for (const a of [act(true, true, 'none'), act(false, false, 'failed'), act(false, true, 'ok'), null]) {
    assert.deepEqual(view('ok', a), { mode: 'content', text: '' });
  }
});

// ── 其餘狀態與邊界 ─────────────────────────────────────────────────────────────

test('不需要資料的小工具（時鐘、台股固定事件）：一律顯示內容，抓取中也不整塊載入', () => {
  assert.equal(view('empty', act(true, true, 'none'), false).mode, 'content');
  assert.equal(view(null, act(false, true, 'none'), false).mode, 'content');
});

test('活動狀態未知（查詢失敗、還沒收到事件）：不顯示載入，只給一般的尚無資料提示', () => {
  const v = view('empty', null);
  assert.equal(v.mode, 'empty');
  assert.equal(v.text, `尚無資料 —— 資料目錄：${DIR}`);
});

test('成功過但目前資料目錄沒有資料（例如剛換資料目錄）：一般的尚無資料提示', () => {
  assert.equal(view('empty', act(false, true, 'ok')).text, `尚無資料 —— 資料目錄：${DIR}`);
});

test('快照狀態缺漏（null／undefined）視同沒有有效快照', () => {
  assert.equal(view(null, act(true, true, 'ok')).mode, 'loading');
  assert.equal(view(undefined, act(false, true, 'failed')).mode, 'empty');
});

test('資料目錄未知：以「（尚未取得設定）」代替', () => {
  assert.match(noDataText(act(false, false, 'ok'), null), /資料目錄：（尚未取得設定）/);
  assert.match(noDataText(null, ''), /資料目錄：（尚未取得設定）/);
});

test('未失敗時文字不含連線失敗或失敗字樣（規格：MUST NOT 在抓取尚未失敗時宣稱連線失敗）', () => {
  const notFailed = [null, act(false, true, 'ok'), act(false, true, 'none'), act(true, true, 'none'),
    act(true, true, 'ok'), act(false, false, 'none'), act(false, false, 'ok')];
  for (const a of notFailed) {
    const v = view('empty', a);
    assert.doesNotMatch(v.text, /連線失敗|失敗/, JSON.stringify(a));
    assert.doesNotMatch(noDataText(a, DIR), /連線失敗|失敗/, `noDataText ${JSON.stringify(a)}`);
  }
});

test('isFetchPending：啟用且（進行中或沒跑過）才成立；未知為否', () => {
  assert.equal(isFetchPending(null), false);
  assert.equal(isFetchPending(act(false, true, 'none')), true);
  assert.equal(isFetchPending(act(true, true, 'failed')), true);
  assert.equal(isFetchPending(act(false, true, 'ok')), false);
  assert.equal(isFetchPending(act(false, true, 'failed')), false);
  assert.equal(isFetchPending(act(true, false, 'none')), false, '關閉時不顯示載入');
});
