// host/tests/settings-fetch.test.mjs
//
// data-layer-rust task 5.5：設定視窗「資料來源與抓取狀態」區塊的純邏輯（host/ui/settings-fetch.mjs）。
// 文字照 spec market-data-fetch「標示資料來源與抓取狀態」與 task brief 的逐字內容。
// fetch-progress-and-manual-refresh task 4.1：「立即更新資料」按鈕與狀態文字（design.md D5 的對照表）。
//
// 執行：node --test host/tests/settings-fetch.test.mjs

import test from 'node:test';
import assert from 'node:assert/strict';

import {
  LICENSE_URL,
  DATA_SOURCES,
  NOT_FETCHED_OFF,
  NOT_FETCHED_ISOLATED,
  STATUS_QUERY_FAILED,
  fetchStatusDisplay,
  REFRESH_LABEL,
  REFRESH_RUNNING_LABEL,
  REFRESH_DISABLED_NOTE,
  fetchControlsDisplay,
  refreshReplyNote,
  refreshErrorNote,
} from '../ui/settings-fetch.mjs';

test('資料來源清單與授權條款連結照 spec', () => {
  assert.deepEqual(
    DATA_SOURCES.map((s) => s.name),
    ['臺灣證券交易所', '證券櫃檯買賣中心', '公開資訊觀測站', 'ForexFactory', 'Yahoo Finance', '紐約聯邦準備銀行'],
  );
  // 兩個機關都掛授權標註（spec：括號修飾前面兩個機關）。
  assert.equal(DATA_SOURCES[0].license, '依政府資料開放授權條款第 1 版');
  assert.equal(DATA_SOURCES[1].license, '依政府資料開放授權條款第 1 版');
  assert.deepEqual(
    DATA_SOURCES.filter((s) => s.license).map((s) => s.name),
    ['臺灣證券交易所', '證券櫃檯買賣中心'],
  );
  assert.equal(LICENSE_URL, 'https://data.gov.tw/license');
});

test('啟用：上次更新時間，部分來源失敗時加註', () => {
  const ok = fetchStatusDisplay({
    kind: 'active',
    last_start: '2026-10-05 15:02',
    last_finish: '2026-10-05 15:03',
    source_failed: false,
    isolation: null,
  });
  assert.equal(ok.text, '上次更新：2026-10-05 15:03');
  assert.equal(ok.tone, 'ok');

  const failed = fetchStatusDisplay({
    kind: 'active',
    last_start: '2026-10-05 15:02',
    last_finish: '2026-10-05 15:03',
    source_failed: true,
    isolation: null,
  });
  assert.equal(failed.text, '上次更新：2026-10-05 15:03（部分來源失敗）');
  assert.equal(failed.tone, 'warn');
});

test('整輪沒有成功（無來源成功或寫檔失敗）：寫中性的「本輪未成功更新」，不寫「部分」也不寫「所有來源失敗」', () => {
  const d = fetchStatusDisplay({
    kind: 'active',
    last_start: '2026-10-05 15:02',
    last_finish: '2026-10-05 15:03',
    source_failed: true,
    round_failed: true,
    isolation: null,
  });
  assert.equal(d.text, '上次更新：2026-10-05 15:03（本輪未成功更新）');
  assert.equal(d.tone, 'warn');
});

test('啟用但沒有完成過的一輪：不顯示假時間', () => {
  const never = fetchStatusDisplay({
    kind: 'active', last_start: null, last_finish: null, source_failed: false, isolation: null,
  });
  assert.equal(never.text, '上次更新：尚無紀錄（等待第一輪抓取）');
  const unfinished = fetchStatusDisplay({
    kind: 'active', last_start: '2026-10-05 15:02', last_finish: null, source_failed: false, isolation: null,
  });
  assert.equal(unfinished.text, '上次更新：尚未完成（上一輪開始於 2026-10-05 15:02）');
});

test('在設定關閉：主文字是未抓取與原因，補充行顯示最後一次更新（有的話）', () => {
  const d = fetchStatusDisplay({
    kind: 'off_by_setting', last_start: '2026-10-05 15:02', last_finish: '2026-10-05 15:03',
    source_failed: false, isolation: null,
  });
  assert.equal(d.text, '未抓取：已在設定關閉');
  assert.equal(d.text, NOT_FETCHED_OFF);
  assert.equal(d.tone, 'off');
  assert.ok(!d.text.includes('上次更新'));
  // 但仍在補充行顯示最後一次更新時間（有的話）。
  assert.deepEqual(d.detail, ['最後一次更新：2026-10-05 15:03']);
  const none = fetchStatusDisplay({
    kind: 'off_by_setting', last_start: null, last_finish: null, source_failed: false, isolation: null,
  });
  assert.deepEqual(none.detail, []);
});

test('隔離環境：未抓取、如何改成 on、並列出兩個路徑', () => {
  const d = fetchStatusDisplay({
    kind: 'off_isolated', last_start: '2026-10-05 15:02', last_finish: '2026-10-05 15:03', source_failed: false,
    settings_path: 'C:\\Users\\Ben\\AppData\\Roaming\\tw.fintools.fc-host\\settings.json',
    isolation: { env_value: 'C:\\Temp\\iso\\Local', registered: 'C:\\Users\\Ben\\AppData\\Local' },
  });
  assert.equal(
    d.text,
    '未抓取：偵測到隔離環境（LOCALAPPDATA 與系統登記不同），可在設定檔將 data_fetch 設為 "on"，改完後重新啟動宿主',
  );
  assert.equal(d.text, NOT_FETCHED_ISOLATED);
  assert.deepEqual(d.detail, [
    '最後一次更新：2026-10-05 15:03',
    'LOCALAPPDATA：C:\\Temp\\iso\\Local',
    '系統登記：C:\\Users\\Ben\\AppData\\Local',
    '設定檔：C:\\Users\\Ben\\AppData\\Roaming\\tw.fintools.fc-host\\settings.json',
  ]);
});

test('查詢失敗或回應不認得：顯示查詢失敗而不是拋錯', () => {
  for (const bad of [null, undefined, 'x', {}, { kind: 'weird' }]) {
    assert.equal(fetchStatusDisplay(bad).text, STATUS_QUERY_FAILED);
  }
});

// ── 立即更新資料（fetch-progress-and-manual-refresh task 4.1，design.md D5 的對照表） ──────────────

const ACTIVE = {
  kind: 'active', last_start: '2026-10-05 15:02', last_finish: '2026-10-05 15:03', source_failed: false, isolation: null,
};
const OFF = { ...ACTIVE, kind: 'off_by_setting' };
const ISOLATED = {
  ...ACTIVE,
  kind: 'off_isolated',
  settings_path: 'C:\\s.json',
  isolation: { env_value: 'C:\\Temp\\iso\\Local', registered: 'C:\\Users\\Ben\\AppData\\Local' },
};
const act = (running, enabled = true, last = 'ok') => ({ running, enabled, last });

test('啟用且未在抓：按鈕「立即更新資料」可按，狀態文字照舊', () => {
  const d = fetchControlsDisplay({ status: ACTIVE, activity: act(false) });
  assert.deepEqual(d.button, { text: '立即更新資料', disabled: false });
  assert.equal(d.button.text, REFRESH_LABEL);
  assert.equal(d.text, '上次更新：2026-10-05 15:03');
  assert.equal(d.tone, 'ok');
});

test('抓取進行中：按鈕「更新中…」停用，狀態文字前加「更新中…」，補充行與顏色照舊', () => {
  const d = fetchControlsDisplay({ status: ACTIVE, activity: act(true) });
  assert.deepEqual(d.button, { text: '更新中…', disabled: true });
  assert.equal(d.button.text, REFRESH_RUNNING_LABEL);
  assert.equal(d.text, '更新中…上次更新：2026-10-05 15:03');
  assert.equal(d.tone, 'ok');

  // 前綴加在任何既有文字前面：部分來源失敗、等待第一輪、查詢失敗。
  const failed = fetchControlsDisplay({ status: { ...ACTIVE, source_failed: true }, activity: act(true) });
  assert.equal(failed.text, '更新中…上次更新：2026-10-05 15:03（部分來源失敗）');
  assert.equal(failed.tone, 'warn');
  const first = fetchControlsDisplay({
    status: { ...ACTIVE, last_start: null, last_finish: null }, activity: act(true, true, 'none'),
  });
  assert.equal(first.text, '更新中…上次更新：尚無紀錄（等待第一輪抓取）');
  const queryFailed = fetchControlsDisplay({ status: null, statusError: 'boom', activity: act(true) });
  assert.equal(queryFailed.text, `更新中…${STATUS_QUERY_FAILED}：boom`);
  assert.equal(queryFailed.button.disabled, true);
});

test('未啟用（設定關閉／隔離環境）：按鈕「立即更新資料」停用，原因就是既有的「未抓取」文字', () => {
  const off = fetchControlsDisplay({ status: OFF, activity: act(false, false) });
  assert.deepEqual(off.button, { text: '立即更新資料', disabled: true });
  assert.equal(off.text, NOT_FETCHED_OFF);
  assert.deepEqual(off.detail, ['最後一次更新：2026-10-05 15:03']);
  assert.equal(off.tone, 'off');

  const iso = fetchControlsDisplay({ status: ISOLATED, activity: act(false, false) });
  assert.deepEqual(iso.button, { text: '立即更新資料', disabled: true });
  assert.equal(iso.text, NOT_FETCHED_ISOLATED);
  assert.equal(iso.detail.length, 4);
});

test('啟用與否以抓取狀態為準（與原因文字同源）；活動狀態的 enabled 落後時不影響按鈕', () => {
  // 剛把設定改成 on、排程還沒醒來：活動狀態仍是 enabled:false，抓取狀態已是 active。
  assert.equal(fetchControlsDisplay({ status: ACTIVE, activity: act(false, false) }).button.disabled, false);
  // 反過來：剛改成 off，活動狀態仍是 enabled:true。
  assert.equal(fetchControlsDisplay({ status: OFF, activity: act(false, true) }).button.disabled, true);
});

test('抓取狀態查不到時退回活動狀態的 enabled；兩者都未知時可按（由宿主回覆決定）', () => {
  const noStatus = { status: null, statusError: 'boom' };
  assert.equal(fetchControlsDisplay({ ...noStatus, activity: act(false, false) }).button.disabled, true);
  assert.equal(fetchControlsDisplay({ ...noStatus, activity: act(false, true) }).button.disabled, false);
  assert.equal(fetchControlsDisplay({ ...noStatus, activity: null }).button.disabled, false);
  // 回覆形狀不認得（kind 不對）也算查不到。
  assert.equal(fetchControlsDisplay({ status: { kind: 'weird' }, activity: act(false, false) }).button.disabled, true);
  // 查詢失敗時即使 status 帶著舊值也不採用它的 kind。
  assert.equal(
    fetchControlsDisplay({ status: ACTIVE, statusError: 'boom', activity: act(false, false) }).button.disabled,
    true,
  );
});

test('活動狀態未知（null）：不當成抓取中，按鈕與文字同未在抓', () => {
  const d = fetchControlsDisplay({ status: ACTIVE, activity: null });
  assert.deepEqual(d.button, { text: '立即更新資料', disabled: false });
  assert.equal(d.text, '上次更新：2026-10-05 15:03');
});

test('要求在途：按鈕停用但文字不變（還沒收到開始抓取的通知）', () => {
  const d = fetchControlsDisplay({ status: ACTIVE, activity: act(false), requesting: true });
  assert.deepEqual(d.button, { text: '立即更新資料', disabled: true });
  assert.equal(d.text, '上次更新：2026-10-05 15:03');
});

test('request_fetch_now 的回覆與錯誤：started／already_running 不加說明，disabled 與錯誤顯示一行', () => {
  assert.equal(refreshReplyNote({ outcome: 'started' }), null);
  assert.equal(refreshReplyNote({ outcome: 'already_running' }), null);
  assert.equal(refreshReplyNote({ outcome: 'disabled' }), REFRESH_DISABLED_NOTE);
  assert.equal(REFRESH_DISABLED_NOTE, '抓取未啟用，無法立即更新');
  for (const weird of [null, undefined, {}, { outcome: 'later' }]) {
    assert.equal(refreshReplyNote(weird), '無法立即更新：宿主回覆無法辨識');
  }
  // Tauri 以字串 reject；也接受 Error。
  assert.equal(refreshErrorNote('抓取排程沒有啟動，無法立即更新'), '無法立即更新：抓取排程沒有啟動，無法立即更新');
  assert.equal(refreshErrorNote(new Error('x')), '無法立即更新：x');
});
