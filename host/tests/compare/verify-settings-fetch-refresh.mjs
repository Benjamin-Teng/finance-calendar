#!/usr/bin/env node
// host/tests/compare/verify-settings-fetch-refresh.mjs
//
// fetch-progress-and-manual-refresh task 4.2：設定視窗「立即更新資料」按鈕與抓取狀態自動更新的 DOM 檢查
// （fixture 模式，無 Tauri；不啟動宿主、不碰桌面）。宿主那一端以 bridge.js 的測試掛鉤代替：
// `__bridgeTestInit.fetchStatus`／`fetchActivity`／`fetchActivityError`／`requestFetchNowOutcome`／
// `requestFetchNowError` 在載入前注入，`__bridgeTest.emit('fetch-activity', …)` 模擬事件，改
// `__bridgeTest.fetchStatus` 模擬一輪寫完後的新排程紀錄。這裡驗的是頁面接線（訂閱、重查、回覆處理）；
// 真的宿主送出的事件與完成時間要在實機另外確認（見 task-4-report.md）。
//
// 截圖寫到 host/tools/evidence/fetch-progress-4.2-fixture-<名稱>.png（只截「資料來源」區塊）。
//
// 用法：node host/tests/compare/verify-settings-fetch-refresh.mjs
// headless Edge 由 cdp.mjs 以獨立 --user-data-dir 啟動，只關閉自己啟動的那一個。

import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { startServer } from './serve.mjs';
import { launchEdge, newTarget, closeTarget, CDPTarget, waitForPageCondition, evaluate } from './cdp.mjs';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(__dirname, '..', '..', '..');
const DEFAULT_FIXTURE = path.join(REPO_ROOT, 'host', 'ui', 'fixtures', 'tw-events.json');
const EVIDENCE_DIR = path.join(REPO_ROOT, 'host', 'tools', 'evidence');
const TAG = '[verify-settings-fetch-refresh]';

/** 設定視窗的預設大小（tray.rs `create_settings_window`：560×760）。 */
const WIDTH = 560;
const HEIGHT = 760;

const ACTIVE = {
  kind: 'active',
  last_start: '2026-10-05 15:02',
  last_finish: '2026-10-05 15:03',
  source_failed: false,
  isolation: null,
};
const FINISHED = { ...ACTIVE, last_start: '2026-10-05 16:09', last_finish: '2026-10-05 16:10' };
const OFF = { ...ACTIVE, kind: 'off_by_setting' };
const ISOLATED = {
  ...ACTIVE,
  kind: 'off_isolated',
  settings_path: 'C:\\Users\\example\\AppData\\Roaming\\tw.fintools.fc-host\\settings.json',
  isolation: { env_value: 'C:\\Temp\\iso\\Local', registered: 'C:\\Users\\example\\AppData\\Local' },
};
const act = (running, enabled = true, last = 'ok') => ({ running, enabled, last });

const ISOLATED_TEXT =
  '未抓取：偵測到隔離環境（LOCALAPPDATA 與系統登記不同），可在設定檔將 data_fetch 設為 "on"，改完後重新啟動宿主';

/**
 * 模擬「get_fetch_activity 在途時收到事件」：攔下 bridge.js 建立 `window.__bridgeTest` 的那一刻，把
 * `fetchActivity` 換成 getter——頁面查詢讀它時先送出 `event`（此時監聽器已掛好），再回傳查詢當下的舊值。
 * fixture 的查詢沒有真正的在途期間，不這樣做就測不到仲裁規則。
 */
function raceScript(event) {
  return `(() => {
    let bt;
    Object.defineProperty(window, '__bridgeTest', {
      configurable: true,
      get() { return bt; },
      set(v) {
        bt = v;
        let stale = v.fetchActivity;
        let fired = false;
        Object.defineProperty(v, 'fetchActivity', {
          configurable: true,
          get() {
            if (!fired) { fired = true; v.emit('fetch-activity', ${JSON.stringify(event)}); }
            return stale;
          },
          set(x) { stale = x; },
        });
      },
    });
  })();`;
}

async function withPage(edge, serverUrl, init, fn, extraScript = '') {
  const target = await newTarget(edge.port, 'about:blank');
  const session = new CDPTarget(target.webSocketDebuggerUrl);
  try {
    await session.connect();
    await session.send('Page.enable');
    await session.send('Emulation.setDeviceMetricsOverride', {
      width: WIDTH,
      height: HEIGHT,
      deviceScaleFactor: 1,
      mobile: false,
    });
    await session.send('Page.addScriptToEvaluateOnNewDocument', {
      source: `window.__bridgeTestInit = ${JSON.stringify(init)};${extraScript}`,
    });
    await session.send('Page.navigate', { url: `${serverUrl}/new/settings.html` });
    await waitForPageCondition(session, 'window.__bridgeTest && window.__settingsFetchReady === true');
    return await fn(session);
  } finally {
    session.close();
    await closeTarget(edge.port, target.id);
  }
}

/** 按鈕、狀態文字、說明行與 request_fetch_now 呼叫次數。 */
async function readState(session) {
  return JSON.parse(
    await evaluate(
      session,
      `JSON.stringify({
        button: document.getElementById('fetch-refresh')?.textContent ?? null,
        disabled: document.getElementById('fetch-refresh')?.disabled ?? null,
        text: document.getElementById('fetch-status-text')?.textContent ?? null,
        note: document.getElementById('fetch-refresh-note')?.textContent ?? null,
        box: document.getElementById('fetch-status')?.textContent ?? null,
        calls: window.__bridgeTest.requestFetchNowCalls,
      })`,
    ),
  );
}

/** 等到狀態符合 `want` 的每個欄位；逾時把最後讀到的狀態寫進失敗訊息（不拋錯，繼續下一步）。 */
async function expectState(session, failures, label, want) {
  const cond = Object.entries(want)
    .map(([k, v]) => {
      const expr = {
        button: "document.getElementById('fetch-refresh')?.textContent",
        disabled: "document.getElementById('fetch-refresh')?.disabled",
        text: "document.getElementById('fetch-status-text')?.textContent",
        note: "(document.getElementById('fetch-refresh-note')?.textContent ?? null)",
        // 整個狀態框的文字：確認沒有多出的節點（例如沒有說明時誤畫出「null」）。
        box: "document.getElementById('fetch-status')?.textContent",
        calls: 'window.__bridgeTest.requestFetchNowCalls',
      }[k];
      return `(${expr}) === ${JSON.stringify(v)}`;
    })
    .join(' && ');
  try {
    await waitForPageCondition(session, cond, { timeoutMs: 3000 });
  } catch {
    failures.push(`${label}：預期 ${JSON.stringify(want)}，實際 ${JSON.stringify(await readState(session))}`);
  }
}

const emit = (session, payload) =>
  evaluate(session, `window.__bridgeTest.emit('fetch-activity', ${JSON.stringify(payload)})`);
const setStatus = (session, status) =>
  evaluate(session, `window.__bridgeTest.fetchStatus = ${JSON.stringify(status)}`);
const click = (session) => evaluate(session, "document.getElementById('fetch-refresh').click()");

async function screenshot(session, name) {
  await evaluate(session, "document.getElementById('sources-section').scrollIntoView({ block: 'end' })");
  const r = JSON.parse(
    await evaluate(
      session,
      "JSON.stringify((({x, y, width, height}) => ({x: x + window.scrollX, y: y + window.scrollY, width, height}))(document.getElementById('sources-section').getBoundingClientRect()))",
    ),
  );
  const clip = { x: Math.max(0, r.x - 8), y: Math.max(0, r.y - 8), width: r.width + 16, height: r.height + 16, scale: 1 };
  const { data } = await session.send('Page.captureScreenshot', { format: 'png', clip });
  const file = path.join(EVIDENCE_DIR, `fetch-progress-4.2-fixture-${name}.png`);
  await writeFile(file, Buffer.from(data, 'base64'));
  console.log(`${TAG} 截圖 ${path.relative(REPO_ROOT, file)}`);
}

const scenarios = [
  {
    // spec「視窗開著時完成一輪」：按下 → 抓取期間停用並標示更新中 → 結束後恢復、完成時間更新。
    name: 'manual-round',
    init: { fetchStatus: ACTIVE, fetchActivity: act(false) },
    async run(s, f) {
      await expectState(s, f, '開頁', {
        button: '立即更新資料', disabled: false, text: '上次更新：2026-10-05 15:03', note: null, calls: 0,
        box: '上次更新：2026-10-05 15:03',
      });
      await screenshot(s, 'idle');
      await click(s);
      await expectState(s, f, '按下', { calls: 1, note: null });
      await emit(s, act(true));
      await expectState(s, f, '開始抓取', {
        button: '更新中…', disabled: true, text: '更新中…上次更新：2026-10-05 15:03',
        box: '更新中…上次更新：2026-10-05 15:03',
      });
      await screenshot(s, 'running');
      // 停用中的按鈕點不下去，不會再送要求。
      await click(s);
      await setStatus(s, FINISHED);
      await emit(s, act(false));
      await expectState(s, f, '一輪結束', {
        button: '立即更新資料', disabled: false, text: '上次更新：2026-10-05 16:10', calls: 1,
      });
    },
  },
  {
    // spec「剛安裝就開啟設定視窗」：首抓進行中開頁，結束後從「等待第一輪抓取」變成完成時間。
    name: 'first-round',
    init: {
      fetchStatus: { ...ACTIVE, last_start: null, last_finish: null },
      fetchActivity: act(true, true, 'none'),
    },
    async run(s, f) {
      await expectState(s, f, '開頁', {
        button: '更新中…', disabled: true, text: '更新中…上次更新：尚無紀錄（等待第一輪抓取）',
      });
      await setStatus(s, FINISHED);
      await emit(s, act(false));
      await expectState(s, f, '首抓結束', {
        button: '立即更新資料', disabled: false, text: '上次更新：2026-10-05 16:10',
      });
    },
  },
  {
    // spec「被判為隔離環境」：未抓取原因照舊、按鈕停用。
    name: 'isolated',
    init: { fetchStatus: ISOLATED, fetchActivity: act(false, false) },
    async run(s, f) {
      await expectState(s, f, '開頁', {
        button: '立即更新資料', disabled: true, text: ISOLATED_TEXT,
        box:
          `${ISOLATED_TEXT}最後一次更新：2026-10-05 15:03LOCALAPPDATA：C:\\Temp\\iso\\Local` +
          '系統登記：C:\\Users\\example\\AppData\\Local' +
          '設定檔：C:\\Users\\example\\AppData\\Roaming\\tw.fintools.fc-host\\settings.json',
      });
      await screenshot(s, 'isolated');
      await click(s);
      await expectState(s, f, '停用時按下', { calls: 0 });
    },
  },
  {
    name: 'off-by-setting',
    init: { fetchStatus: OFF, fetchActivity: act(false, false) },
    async run(s, f) {
      await expectState(s, f, '開頁', { button: '立即更新資料', disabled: true, text: '未抓取：已在設定關閉' });
    },
  },
  {
    // 宿主回 disabled（設定在按下前一刻改了）：顯示說明，並重查狀態換成「未抓取」原因。
    name: 'reply-disabled',
    init: { fetchStatus: ACTIVE, fetchActivity: act(false), requestFetchNowOutcome: 'disabled' },
    async run(s, f) {
      await setStatus(s, OFF);
      await click(s);
      await expectState(s, f, '回 disabled', {
        calls: 1, note: '抓取未啟用，無法立即更新', text: '未抓取：已在設定關閉', disabled: true,
      });
    },
  },
  {
    // 宿主拒絕（沒有排程、正在因更新結束）：顯示說明，按鈕恢復；之後開始一輪時說明消失。
    name: 'reply-error',
    init: { fetchStatus: ACTIVE, fetchActivity: act(false), requestFetchNowError: '抓取排程沒有啟動，無法立即更新' },
    async run(s, f) {
      await click(s);
      await expectState(s, f, '被拒', {
        calls: 1, note: '無法立即更新：抓取排程沒有啟動，無法立即更新', disabled: false,
        text: '上次更新：2026-10-05 15:03',
        box: '上次更新：2026-10-05 15:03無法立即更新：抓取排程沒有啟動，無法立即更新',
      });
      await emit(s, act(true));
      await expectState(s, f, '開始抓取後說明消失', { note: null, button: '更新中…' });
    },
  },
  {
    // 先 listen 再查詢，查詢在途期間收到的事件優先：查詢回覆是舊的「閒置」，事件說「進行中」。
    name: 'query-race',
    init: { fetchStatus: ACTIVE, fetchActivity: act(false) },
    script: raceScript(act(true)),
    async run(s, f) {
      const hooked = await evaluate(
        s,
        "typeof Object.getOwnPropertyDescriptor(window.__bridgeTest, 'fetchActivity')?.get === 'function'",
      );
      if (!hooked) f.push('攔截腳本沒有生效（fetchActivity 不是 getter）');
      await expectState(s, f, '開頁', { button: '更新中…', disabled: true, text: '更新中…上次更新：2026-10-05 15:03' });
    },
  },
  {
    // 活動狀態查不到（＝未知）：不顯示更新中、按鈕依抓取狀態可按；第一筆事件校正。
    name: 'activity-unknown',
    init: { fetchStatus: ACTIVE, fetchActivityError: true },
    async run(s, f) {
      await expectState(s, f, '開頁', { button: '立即更新資料', disabled: false, text: '上次更新：2026-10-05 15:03' });
      await emit(s, act(true));
      await expectState(s, f, '事件校正', { button: '更新中…', disabled: true });
    },
  },
  {
    // 閒置時抓取被關閉（沒有一輪結束）：enabled 改變也重查，原因與按鈕跟著換。
    name: 'enabled-change',
    init: { fetchStatus: ACTIVE, fetchActivity: act(false) },
    async run(s, f) {
      await setStatus(s, OFF);
      await emit(s, act(false, false));
      await expectState(s, f, '關閉抓取', { text: '未抓取：已在設定關閉', disabled: true });
    },
  },
  {
    // 形狀不對的事件被 bridge 濾掉，不會把按鈕卡在某個狀態。
    name: 'malformed-event',
    init: { fetchStatus: ACTIVE, fetchActivity: act(false) },
    async run(s, f) {
      await emit(s, { running: 'yes', enabled: true, last: 'ok' });
      await emit(s, { running: true, enabled: true });
      await expectState(s, f, '壞事件', { button: '立即更新資料', disabled: false, text: '上次更新：2026-10-05 15:03' });
    },
  },
];

/** D6：資料目錄提示不再指向 update_tw_events.py，改述用途並提醒先關閉抓取。 */
async function checkDataDirHint(session, failures) {
  const hint = await evaluate(
    session,
    "document.getElementById('data-dir').closest('section').querySelector('.hint').textContent",
  );
  if (hint.includes('update_tw_events.py')) failures.push(`資料目錄提示仍提到 update_tw_events.py（「${hint}」）`);
  if (!hint.includes('data_fetch') || !hint.includes('"off"')) {
    failures.push(`資料目錄提示沒寫先把 data_fetch 設為 "off"（「${hint}」）`);
  }
}

async function run() {
  await mkdir(EVIDENCE_DIR, { recursive: true });
  const server = await startServer(DEFAULT_FIXTURE);
  console.log(`${TAG} 本機伺服器：${server.url}（暫存目錄：${server.tmpRootDisplay}）`);
  const edge = await launchEdge();
  console.log(`${TAG} headless Edge 已啟動，devtools port=${edge.port}`);
  const failures = [];
  try {
    for (const sc of scenarios) {
      const before = failures.length;
      await withPage(
        edge,
        server.url,
        sc.init,
        async (session) => {
          const local = [];
          await sc.run(session, local);
          if (sc.name === 'manual-round') await checkDataDirHint(session, local);
          failures.push(...local.map((m) => `${sc.name}：${m}`));
        },
        sc.script,
      );
      console.log(`${TAG} ${sc.name}：${failures.length === before ? 'PASS' : 'FAIL'}`);
    }
  } finally {
    await edge.close();
    await server.close({});
  }
  for (const f of failures) console.log(`${TAG} FAIL ${f}`);
  console.log(`${TAG} VERDICT ${failures.length === 0 ? 'PASS' : 'FAIL'}（${scenarios.length} 個情境）`);
  if (failures.length) process.exitCode = 1;
}

run().catch((err) => {
  console.error(`${TAG} 執行失敗：`, err);
  process.exitCode = 1;
});
