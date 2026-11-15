// host/tests/bridge-fetch-activity.test.mjs
//
// fetch-progress-and-manual-refresh：host/ui/bridge.js 的抓取活動狀態與立即更新封裝。直接 import 真的
// bridge.js，不做樁：
//
// - fixture 分支（沒有 `window.__TAURI__`）：`normalizeFetchActivity` 的邊界、`getFetchActivity` 的
//   預設值／覆寫／`fetchActivityError`、`listen('fetch-activity')` 濾掉形狀不對的事件、`requestFetchNow`
//   的掛鉤。
// - Tauri 分支（以假的 `window.__TAURI__` 另外載入一份模組；`hasTauri` 在模組載入時決定，所以用不同的
//   查詢字串取得新的模組實例）：`get_fetch_activity` 被拒時 `getFetchActivity` 回 null 而不是 reject。
//   小工具外殼把它放在初始化的 `Promise.all` 裡，這裡一旦 reject，整個小工具就掛不上去（design.md D8）。
//   `request_fetch_now` 被拒時則**要**往上丟，設定視窗才能顯示說明。
//
// 執行：node --test host/tests/bridge-fetch-activity.test.mjs

import test from 'node:test';
import assert from 'node:assert/strict';

globalThis.window = {};
const BRIDGE_URL = new URL('../ui/bridge.js', import.meta.url);
const fixture = await import(BRIDGE_URL.href);

// Tauri 分支：invoke 的行為由各測試設定。
let invokeImpl = async () => undefined;
const invokeCalls = [];
let windowListener = null;
window.__TAURI__ = {
  core: {
    invoke: (cmd, args) => {
      invokeCalls.push({ cmd, args });
      return invokeImpl(cmd, args);
    },
  },
  webviewWindow: {
    getCurrentWebviewWindow: () => ({
      listen: async (event, handler) => {
        windowListener = { event, handler };
        return () => {};
      },
    }),
  },
};
const tauri = await import(`${BRIDGE_URL.href}?tauri`);

const VALID = { running: true, enabled: false, last: 'failed' };

test('兩份模組分別走 fixture 與 Tauri 分支', () => {
  assert.equal(fixture.hasTauri, false);
  assert.equal(tauri.hasTauri, true);
});

test('normalizeFetchActivity：三種 last 與兩個布林欄位都對才收，只留三個欄位', () => {
  for (const last of ['none', 'ok', 'failed']) {
    for (const running of [true, false]) {
      for (const enabled of [true, false]) {
        assert.deepEqual(fixture.normalizeFetchActivity({ running, enabled, last }), { running, enabled, last });
      }
    }
  }
  assert.deepEqual(fixture.normalizeFetchActivity({ ...VALID, data_dir: 'C:\\x', extra: 1 }), VALID);
});

test('normalizeFetchActivity：缺欄位、型別不對、未知的 last、非物件一律回 null', () => {
  const bad = [
    null,
    undefined,
    'running',
    0,
    true,
    [],
    {},
    { enabled: true, last: 'ok' }, // 缺 running
    { running: false, last: 'ok' }, // 缺 enabled
    { running: false, enabled: true }, // 缺 last
    { running: 'true', enabled: true, last: 'ok' },
    { running: 1, enabled: true, last: 'ok' },
    { running: null, enabled: true, last: 'ok' },
    { running: false, enabled: 'false', last: 'ok' },
    { running: false, enabled: 0, last: 'ok' },
    { running: false, enabled: true, last: 'running' },
    { running: false, enabled: true, last: 'OK' },
    { running: false, enabled: true, last: '' },
    { running: false, enabled: true, last: null },
    { running: false, enabled: true, last: 1 },
  ];
  for (const raw of bad) {
    assert.equal(fixture.normalizeFetchActivity(raw), null, JSON.stringify(raw));
  }
});

test('fixture：getFetchActivity 預設為閒置、啟用、上一輪成功；覆寫照正規化；fetchActivityError 時回 null', async () => {
  const t = window.__bridgeTest;
  assert.deepEqual(await fixture.getFetchActivity(), { running: false, enabled: true, last: 'ok' });
  t.fetchActivity = { ...VALID, extra: 1 };
  assert.deepEqual(await fixture.getFetchActivity(), VALID);
  t.fetchActivity = { running: true };
  assert.equal(await fixture.getFetchActivity(), null);
  t.fetchActivity = VALID;
  t.fetchActivityError = true;
  assert.equal(await fixture.getFetchActivity(), null);
  t.fetchActivity = null;
  t.fetchActivityError = null;
});

test('fixture：listen("fetch-activity") 只轉交形狀正確的事件，payload 已正規化', async () => {
  const got = [];
  const unlisten = await fixture.listen('fetch-activity', (e) => got.push(e));
  const t = window.__bridgeTest;
  t.emit('fetch-activity', { running: 'yes', enabled: true, last: 'ok' });
  t.emit('fetch-activity', { running: true, enabled: true, last: 'later' });
  t.emit('fetch-activity', null);
  t.emit('fetch-activity', { ...VALID, path: 'C:\\x' });
  assert.deepEqual(got, [{ event: 'fetch-activity', payload: VALID }]);
  unlisten();
  t.emit('fetch-activity', VALID);
  assert.equal(got.length, 1);
});

test('fixture：其他事件不經正規化，原樣轉交', async () => {
  const got = [];
  await fixture.listen('pause', (e) => got.push(e));
  window.__bridgeTest.emit('pause', { paused: true, reason: 'x' });
  assert.deepEqual(got, [{ event: 'pause', payload: { paused: true, reason: 'x' } }]);
});

test('fixture：requestFetchNow 預設回 started 並計次；可覆寫 outcome；錯誤掛鉤讓它 reject', async () => {
  const t = window.__bridgeTest;
  const before = t.requestFetchNowCalls;
  assert.deepEqual(await fixture.requestFetchNow(), { outcome: 'started' });
  t.requestFetchNowOutcome = 'already_running';
  assert.deepEqual(await fixture.requestFetchNow(), { outcome: 'already_running' });
  t.requestFetchNowError = '抓取排程沒有啟動，無法立即更新';
  await assert.rejects(fixture.requestFetchNow(), (e) => e === '抓取排程沒有啟動，無法立即更新');
  assert.equal(t.requestFetchNowCalls, before + 3);
  t.requestFetchNowOutcome = null;
  t.requestFetchNowError = null;
});

test('Tauri：get_fetch_activity 被拒時 getFetchActivity 回 null，不 reject', async () => {
  invokeImpl = async () => {
    throw '抓取活動狀態尚未就緒';
  };
  invokeCalls.length = 0;
  assert.equal(await tauri.getFetchActivity(), null);
  assert.deepEqual(invokeCalls, [{ cmd: 'get_fetch_activity', args: undefined }]);

  // 同步拋出（例如 IPC 物件壞掉）也一樣。
  invokeImpl = () => {
    throw new Error('ipc gone');
  };
  assert.equal(await tauri.getFetchActivity(), null);
});

test('Tauri：get_fetch_activity 回覆形狀不對時回 null，對的照正規化', async () => {
  invokeImpl = async () => ({ running: true, enabled: true });
  assert.equal(await tauri.getFetchActivity(), null);
  invokeImpl = async () => ({ ...VALID, extra: 1 });
  assert.deepEqual(await tauri.getFetchActivity(), VALID);
});

test('Tauri：listen("fetch-activity") 經視窗的 listen 掛上，形狀不對的事件不轉交', async () => {
  const got = [];
  await tauri.listen('fetch-activity', (e) => got.push(e));
  assert.equal(windowListener.event, 'fetch-activity');
  windowListener.handler({ event: 'fetch-activity', id: 7, payload: { running: true } });
  windowListener.handler({ event: 'fetch-activity', id: 8, payload: { ...VALID, extra: 1 } });
  assert.deepEqual(got, [{ event: 'fetch-activity', id: 8, payload: VALID }]);
});

test('Tauri：requestFetchNow 呼叫 request_fetch_now（不帶參數），被拒時往上丟', async () => {
  invokeCalls.length = 0;
  invokeImpl = async () => ({ outcome: 'started' });
  assert.deepEqual(await tauri.requestFetchNow(), { outcome: 'started' });
  assert.deepEqual(invokeCalls, [{ cmd: 'request_fetch_now', args: undefined }]);
  invokeImpl = async () => {
    throw '抓取排程沒有啟動，無法立即更新';
  };
  await assert.rejects(tauri.requestFetchNow(), (e) => e === '抓取排程沒有啟動，無法立即更新');
});
