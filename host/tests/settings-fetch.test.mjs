// host/tests/settings-fetch.test.mjs
//
// data-layer-rust task 5.5：設定視窗「資料來源與抓取狀態」區塊的純邏輯（host/ui/settings-fetch.mjs）。
// 文字照 spec market-data-fetch「標示資料來源與抓取狀態」與 task brief 的逐字內容。
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
