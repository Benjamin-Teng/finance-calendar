// host/tests/widget-init-race.test.mjs
//
// 重現並鎖住 Codex adversarial review（.superpowers/sdd/tasks/reviews/task-4.1-codex.md
// [high]）指出的問題：widget.html 的骨架在「監聽器已註冊、但 get_snapshot/get_settings
// 查詢回覆或動態 import(`./widgets/<id>.js`) 尚未完成」這段期間收到的 data／settings／
// pause／edit-mode 事件會被丟棄（callback 陣列這時還是空的，forEach 等於什麼都沒做），
// 掛載後仍停在舊版本，而來源檔未變時不會再通知第二次。
//
// round 2（.superpowers/sdd/tasks/reviews/task-4.1-codex-r1.md [high]）另外鎖住一個更窄的
// 競態：round 1 的修法用「跨事件共用遞增序號、查詢回覆在 resolve 那一刻蓋序號」判斷查詢與
// 緩衝事件誰新，但 JS 單執行緒只保證回呼「執行」順序，不保證 IPC 層「送達」順序——若 v2
// 事件先送達（seq 較小），v1 查詢的 promise 較晚才 resolve（seq 較大），比較 seq 會誤判
// v1 比較新，用它蓋掉已緩衝的 v2。`runQueryRaceScenario` 專門重現這個「查詢仍在途、事件
// 先抵達，查詢晚一步才用舊資料 resolve」的情境（`runScenario` 測的是動態 import 期間，
// 屬於查詢已經 resolve 之後的窗口，兩者是不同的競態，都要蓋住）。
//
// 做法：把 host/ui/widget.html 內嵌的 <script type="module"> 原始碼抽出來，只把三個
// 靜態 import 與一個動態 import 的「模組從哪裡來」換成測試可控制的樁（其餘邏輯——事件
// 監聽器註冊順序、Promise.all 查詢、動態 import、掛載——原封不動、逐行照跑），在 Node vm
// context 內執行，用可控制的 resolve 時機模擬「事件恰好在動態 import 完成前抵達」
// （`runScenario`）或「事件恰好在查詢仍在途時抵達」（`runQueryRaceScenario`）。
//
// 這不是在測 bridge.js／真的 Tauri 行為（bridge.js 由另一個 task 負責、本檔不碰也不需要
// 真的載入它），純粹是在鎖住 widget.html 自己的時序邏輯。
//
// 執行：node host/tests/widget-init-race.test.mjs [widget.html 路徑，預設 host/ui/widget.html]
// 選用路徑參數是為了在不動工作樹既有檔案的情況下，能對「修正前」的版本（例如
// `git show <commit>:host/ui/widget.html > 暫存檔` 出來的內容）重跑同一支測試取得 RED
// 證據——global-constraints.md 禁止 git checkout/reset 等動到整個工作樹的指令。
//
// 通過條件：
//   - runScenario：data／settings／edit-mode／pause 四種「掛載前事件」都必須在掛載時或
//     掛載後立刻被小工具看到，缺一個就 FAIL。
//   - runQueryRaceScenario：查詢仍在途時抵達的 v2 事件，必須贏過查詢晚一步 resolve 回來的
//     舊版本 v1（data／settings 皆是），缺一個就 FAIL。
//   - runPauseScenario（task 5.6 fix round 1）：頁面啟動／重新載入時以 `get_pause` 查詢目前
//     暫停狀態（宿主暫停中重建的頁面不能從「未暫停」開始）；查詢仍在途時收到的 `pause` 事件
//     以事件為準（pause 沒有版本欄位，比照 settings 的規則）。
// 任一情境有失敗就 exit 1。

import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const widgetHtmlPath = path.join(__dirname, '..', 'ui', 'widget.html');

function extractModuleScript(html) {
  const matches = [...html.matchAll(/<script type="module">([\s\S]*?)<\/script>/g)];
  if (matches.length !== 1) {
    throw new Error(
      `widget.html 預期恰好一個 <script type="module"> 區塊，實際找到 ${matches.length} 個——` +
        '本測試依賴這個假設抽取骨架邏輯，結構變了要一併檢查測試是否還有效。',
    );
  }
  return matches[0][1];
}

function transformForHarness(code) {
  const replacements = [
    [
      "import { getWidget } from './registry.js';",
      'const { getWidget } = globalThis.__test.registry;',
    ],
    ["import * as bridge from './bridge.js';", 'const bridge = globalThis.__test.bridge;'],
    ["import * as common from './common.js';", 'const common = globalThis.__test.common;'],
    [
      'mod = await import(`./widgets/${id}.js`);',
      'mod = await globalThis.__test.dynamicImport(id);',
    ],
    ['main();', 'globalThis.__test.mainPromise = main();'],
  ];
  let out = code;
  for (const [from, to] of replacements) {
    if (!out.includes(from)) {
      throw new Error(
        '預期能在 widget.html 抽出的腳本中找到這段原文，找不到表示骨架已經改了、測試的' +
          `文字替換假設過期，需要一併更新本測試：\n${from}`,
      );
    }
    out = out.replace(from, to);
  }
  return out;
}

// task 5.3：widget.html 的 `applyEditAffordance`（編輯版面外框／游標＋
// `data-tauri-drag-region`）不是本測試要鎖住的行為（見 host/tests/widget-edit-mode.test.mjs），
// 但它在 main() 掛載後無條件執行一次，本測試的 DOM 樁因此也要能撐過那幾行呼叫
// （`document.body.classList`／`container.setAttribute`／`removeAttribute`），否則會在
// `runScenario`／`runQueryRaceScenario`／`runPauseScenario` 裡丟出無關的 TypeError。
function makeClassListStub() {
  const set = new Set();
  return {
    toggle(name, force) {
      const on = force === undefined ? !set.has(name) : !!force;
      if (on) {
        set.add(name);
      } else {
        set.delete(name);
      }
    },
    contains(name) {
      return set.has(name);
    },
  };
}

function makeElementStub(tag) {
  const attrs = new Map();
  const el = {
    tagName: tag,
    className: '',
    textContent: '',
    _children: [],
    _innerHTML: '',
    classList: makeClassListStub(),
    // task 7.3：widget.html 新增的 `placeholder.style.display = ...`（佔位外框顯示切換）也在
    // main() 掛載後的路徑上無條件執行，DOM 樁需要一個可寫的 `style` 物件撐過去（同上方註解
    // 「DOM 樁也要能撐過那幾行呼叫」的道理）。
    style: {},
    appendChild(child) {
      this._children.push(child);
    },
    setAttribute(name, value) {
      attrs.set(name, String(value));
    },
    removeAttribute(name) {
      attrs.delete(name);
    },
    getAttribute(name) {
      return attrs.has(name) ? attrs.get(name) : null;
    },
  };
  Object.defineProperty(el, 'innerHTML', {
    get() {
      return this._innerHTML;
    },
    set(v) {
      this._innerHTML = v;
      this._children = [];
    },
  });
  return el;
}

/** 跑一次「掛載前事件競態」情境；回傳掛載當下與掛載後觀察到的狀態，供斷言。 */
async function runScenario(scriptSrc) {
  const rootEl = makeElementStub('div');
  const documentStub = {
    documentElement: { dataset: {} }, // widget.html 寫入 data-widget（清單收合，design.md D5）
    body: makeElementStub('body'),
    getElementById: (id) => (id === 'widget-root' ? rootEl : null),
    createElement: (tag) => makeElementStub(tag),
    title: '',
  };
  const windowStub = { location: { search: '?w=clock' } };
  class ResizeObserverStub {
    observe() {}
    disconnect() {}
  }

  const listeners = { data: [], settings: [], 'edit-mode': [], 'edit-preview': [], pause: [], 'widget-font': [] };
  const dispatch = (name, payload) => {
    for (const fn of listeners[name]) fn({ payload });
  };

  const snapshotV1 = { channel: 'tw-events', status: 'ok', data: 'v1', meta: { v: 1 } };
  const settingsV1 = { widgets: {}, v: 1 };

  let importStartedResolve;
  const importStarted = new Promise((res) => {
    importStartedResolve = res;
  });
  let releaseImport;
  const importGate = new Promise((res) => {
    releaseImport = res;
  });

  const mountObservations = { snapshot: undefined, settings: undefined };
  const lateEvents = { data: [], settings: [], editMode: [], pause: [] };

  const testGlobals = {
    registry: {
      getWidget: (id) => (id === 'clock' ? { id: 'clock', channel: 'tw-events' } : null),
    },
    bridge: {
      hasTauri: true,
      listen: async (event, handler) => {
        listeners[event].push(handler);
        return () => {};
      },
      getSnapshot: async () => snapshotV1,
      getSettings: async () => settingsV1,
      getPause: async () => ({ paused: false, reason: null }),
      getEditMode: async () => false,
      getWidgetFontState: async () => null,
      updateSettings: async () => {},
      setEditMode: async () => {},
      reportContent: () => {},
    },
    common: {},
    dynamicImport: async () => {
      importStartedResolve();
      await importGate; // 卡住，直到測試主動放行——模擬「動態 import 需要一段時間」。
      return {
        mount(container, ctx) {
          mountObservations.snapshot = ctx.snapshot;
          mountObservations.settings = ctx.settings;
          ctx.onData((v) => lateEvents.data.push(v));
          ctx.onSettings((v) => lateEvents.settings.push(v));
          ctx.onEditMode((v) => lateEvents.editMode.push(v));
          ctx.onPause((v) => lateEvents.pause.push(v));
        },
      };
    },
    mainPromise: null,
  };

  const sandbox = {
    document: documentStub,
    window: windowStub,
    URLSearchParams,
    ResizeObserver: ResizeObserverStub,
    console,
    __test: testGlobals,
  };
  vm.createContext(sandbox);
  vm.runInContext(scriptSrc, sandbox, { filename: 'widget.html (extracted)' });

  // main() 此時已經同步跑到 `await bridge.listen(...)` 那串（listen 立刻 resolve，但仍要
  // 過幾個 microtask）。用「動態 import 真的被呼叫了」這個確定性訊號，取代用猜的 tick 數，
  // 確定 main() 已經走過 listen 與 Promise.all（get_snapshot／get_settings），正卡在
  // 動態 import 上。
  await importStarted;

  // 在動態 import 完成前，模擬四種事件各送一筆「版本 2」——這正是 Codex 指出的空窗期。
  dispatch('data', {
    channel: 'tw-events',
    snapshot: { channel: 'tw-events', data: 'v2', meta: { v: 2 } },
  });
  dispatch('settings', { widgets: {}, v: 2 });
  dispatch('edit-mode', { enabled: true });
  dispatch('pause', { paused: true, reason: 'test' });

  releaseImport();
  await sandbox.__test.mainPromise;

  return { mountObservations, lateEvents };
}

/**
 * round 2 重現：查詢仍在途時（`bridge.getSnapshot`／`getSettings` 都已呼叫、都還沒
 * resolve）v2 事件先送達，查詢晚一步才用「舊版本」v1 resolve——鎖住 task-4.1-codex-r1.md
 * [high] 指出的競態（seq 判新舊在這個時序下會誤判）。
 */
async function runQueryRaceScenario(scriptSrc) {
  const rootEl = makeElementStub('div');
  const documentStub = {
    documentElement: { dataset: {} }, // widget.html 寫入 data-widget（清單收合，design.md D5）
    body: makeElementStub('body'),
    getElementById: (id) => (id === 'widget-root' ? rootEl : null),
    createElement: (tag) => makeElementStub(tag),
    title: '',
  };
  const windowStub = { location: { search: '?w=clock' } };
  class ResizeObserverStub {
    observe() {}
    disconnect() {}
  }

  const listeners = { data: [], settings: [], 'edit-mode': [], 'edit-preview': [], pause: [], 'widget-font': [] };
  const dispatch = (name, payload) => {
    for (const fn of listeners[name]) fn({ payload });
  };

  // data 通道用 meta.loadedAt 分版本（design.md D4）；settings 沒有版本欄位，用內容本身
  // 區分「查詢回覆」與「事件」，這裡刻意用比較舊的 loadedAt／v 值代表「v1」。
  const snapshotV1 = { channel: 'tw-events', status: 'ok', data: 'v1', meta: { loadedAt: 1000 } };
  const settingsV1 = { widgets: {}, v: 1 };

  let releaseSnapshot;
  const snapshotGate = new Promise((res) => {
    releaseSnapshot = res;
  });
  let releaseSettings;
  const settingsGate = new Promise((res) => {
    releaseSettings = res;
  });
  let snapshotStartedResolve;
  const snapshotStarted = new Promise((res) => {
    snapshotStartedResolve = res;
  });
  let settingsStartedResolve;
  const settingsStarted = new Promise((res) => {
    settingsStartedResolve = res;
  });

  const mountObservations = { snapshot: undefined, settings: undefined };

  const testGlobals = {
    registry: {
      getWidget: (id) => (id === 'clock' ? { id: 'clock', channel: 'tw-events' } : null),
    },
    bridge: {
      hasTauri: true,
      listen: async (event, handler) => {
        listeners[event].push(handler);
        return () => {};
      },
      getSnapshot: async () => {
        snapshotStartedResolve();
        await snapshotGate; // 卡住，直到測試放行——模擬「查詢仍在途」。
        return snapshotV1; // 放行後才 resolve，且回傳的是「舊版本」。
      },
      getSettings: async () => {
        settingsStartedResolve();
        await settingsGate;
        return settingsV1;
      },
      getPause: async () => ({ paused: false, reason: null }),
      getEditMode: async () => false,
      getWidgetFontState: async () => null,
      updateSettings: async () => {},
      setEditMode: async () => {},
      reportContent: () => {},
    },
    common: {},
    dynamicImport: async () => ({
      mount(container, ctx) {
        mountObservations.snapshot = ctx.snapshot;
        mountObservations.settings = ctx.settings;
      },
    }),
    mainPromise: null,
  };

  const sandbox = {
    document: documentStub,
    window: windowStub,
    URLSearchParams,
    ResizeObserver: ResizeObserverStub,
    console,
    __test: testGlobals,
  };
  vm.createContext(sandbox);
  vm.runInContext(scriptSrc, sandbox, { filename: 'widget.html (extracted, query race)' });

  // main() 此時應該正卡在 Promise.all([getSnapshot(), getSettings()])——兩個查詢都已送出、
  // 都還沒 resolve。用「兩個查詢都真的被呼叫了」這個確定性訊號，取代用猜的 tick 數。
  await Promise.all([snapshotStarted, settingsStarted]);

  // Codex 指出的競態：查詢仍在途時，v2 事件先送達前端。
  dispatch('data', {
    channel: 'tw-events',
    snapshot: { channel: 'tw-events', data: 'v2', meta: { loadedAt: 2000 } },
  });
  dispatch('settings', { widgets: {}, v: 2 });

  // 查詢晚一步才用「舊版本」resolve——重現「v1 查詢在 v2 事件之後才完成」。
  releaseSnapshot();
  releaseSettings();

  await sandbox.__test.mainPromise;

  return { mountObservations };
}

/**
 * task 4.1 fix round 3（.superpowers/sdd/tasks/reviews/task-4.1-fix-codex-r1.md [high]）：
 * 「舊目錄事件 → registry 重建 → empty 查詢」。`update_settings` 改 `data_dir` 會整份重建通道
 * 註冊表（`host/src/widgets.rs`），重建後 `get_snapshot` 回 `status:'empty'`（meta=null）——
 * 這個 empty 代表「新目錄還沒有資料」，比切換前緩衝到的舊目錄事件**新**，不能因為 meta=null
 * 就判定它較舊、讓舊目錄資料復活。宿主在快照／empty 回覆與 data 推送都帶 registry 世代
 * `generation`（目錄切換／註冊表重建時遞增），前端先比世代、同世代才比 `meta.loadedAt`。
 *
 * 時序：查詢在途 → 舊世代（generation 0）事件抵達並被緩衝 → 查詢以新世代（generation 1）
 * 的 empty resolve → 掛載。掛載後再送一筆晚到的舊世代事件（排程執行緒在重建前取得的快照，
 * 重建後才投遞）與一筆新世代事件，回傳掛載當下的快照與掛載後 onData 收到的清單。
 */
async function runGenerationScenario(scriptSrc) {
  const rootEl = makeElementStub('div');
  const documentStub = {
    documentElement: { dataset: {} }, // widget.html 寫入 data-widget（清單收合，design.md D5）
    body: makeElementStub('body'),
    getElementById: (id) => (id === 'widget-root' ? rootEl : null),
    createElement: (tag) => makeElementStub(tag),
    title: '',
  };
  const windowStub = { location: { search: '?w=clock' } };
  class ResizeObserverStub {
    observe() {}
    disconnect() {}
  }
  const listeners = { data: [], settings: [], 'edit-mode': [], 'edit-preview': [], pause: [], 'widget-font': [] };
  const dispatch = (name, payload) => {
    for (const fn of listeners[name]) fn({ payload });
  };

  let releaseSnapshot;
  const snapshotGate = new Promise((res) => {
    releaseSnapshot = res;
  });
  let snapshotStartedResolve;
  const snapshotStarted = new Promise((res) => {
    snapshotStartedResolve = res;
  });
  const mountObservations = { snapshot: undefined };
  const late = [];

  const testGlobals = {
    registry: {
      getWidget: (id) => (id === 'clock' ? { id: 'clock', channel: 'tw-events' } : null),
    },
    bridge: {
      hasTauri: true,
      listen: async (event, handler) => {
        listeners[event].push(handler);
        return () => {};
      },
      getSnapshot: async () => {
        snapshotStartedResolve();
        await snapshotGate;
        // registry 已重建（新目錄尚無檔案）：empty、新世代。
        return { channel: 'tw-events', status: 'empty', data: null, meta: null, generation: 1 };
      },
      getSettings: async () => ({ widgets: {} }),
      getPause: async () => ({ paused: false, reason: null }),
      getEditMode: async () => false,
      getWidgetFontState: async () => null,
      updateSettings: async () => {},
      setEditMode: async () => {},
      reportContent: () => {},
    },
    common: {},
    dynamicImport: async () => ({
      mount(container, ctx) {
        mountObservations.snapshot = ctx.snapshot;
        ctx.onData((v) => late.push(v));
      },
    }),
    mainPromise: null,
  };
  const sandbox = {
    document: documentStub,
    window: windowStub,
    URLSearchParams,
    ResizeObserver: ResizeObserverStub,
    console,
    __test: testGlobals,
  };
  vm.createContext(sandbox);
  vm.runInContext(scriptSrc, sandbox, { filename: 'widget.html (extracted, generation)' });

  await snapshotStarted;
  // 舊目錄（generation 0）的資料在查詢仍在途時抵達。
  dispatch('data', {
    channel: 'tw-events',
    generation: 0,
    snapshot: { channel: 'tw-events', data: 'old-dir', meta: { loadedAt: 2000 } },
  });
  releaseSnapshot();
  await sandbox.__test.mainPromise;

  // 掛載後：晚到的舊世代事件（應丟棄）與新世代事件（應派送）。
  dispatch('data', {
    channel: 'tw-events',
    generation: 0,
    snapshot: { channel: 'tw-events', data: 'old-dir-late', meta: { loadedAt: 3000 } },
  });
  dispatch('data', {
    channel: 'tw-events',
    generation: 1,
    snapshot: { channel: 'tw-events', data: 'new-dir', meta: { loadedAt: 2500 } },
  });

  return { mountObservations, late };
}

/**
 * task 2.7 follow-up（task-4.1-report.md fix round 3「疑慮」／host/src/widgets.rs
 * `push_empty_snapshots_after_rebuild`）：`update_settings` 改 `data_dir` 重建通道註冊表後，
 * 對已訂閱通道推送一筆新世代的 empty——payload 的 `snapshot` 欄位是 `null`（不是像
 * `runGenerationScenario` 那樣的「empty **查詢**回覆」，是「empty **推送**」，兩條路徑各自
 * 獨立、都要驗）。本情境驗證 widget.html 的 `data` 監聽器把 `snapshot:null` 正規化成
 * `status:'empty'`（`data`／`meta` 皆為 `null`），掛載後世代遞增的推送要照常派送給 onData
 * handler，不能因為「以前只看過 `payload.snapshot.xxx`」而在讀取 `null` 的屬性時丟例外、
 * 或被誤判成一筆該丟棄的資料。
 */
async function runEmptyPushScenario(scriptSrc) {
  const rootEl = makeElementStub('div');
  const documentStub = {
    documentElement: { dataset: {} }, // widget.html 寫入 data-widget（清單收合，design.md D5）
    body: makeElementStub('body'),
    getElementById: (id) => (id === 'widget-root' ? rootEl : null),
    createElement: (tag) => makeElementStub(tag),
    title: '',
  };
  const windowStub = { location: { search: '?w=clock' } };
  class ResizeObserverStub {
    observe() {}
    disconnect() {}
  }
  const listeners = { data: [], settings: [], 'edit-mode': [], 'edit-preview': [], pause: [], 'widget-font': [] };
  const dispatch = (name, payload) => {
    for (const fn of listeners[name]) fn({ payload });
  };

  const received = [];
  const testGlobals = {
    registry: {
      getWidget: (id) => (id === 'clock' ? { id: 'clock', channel: 'tw-events' } : null),
    },
    bridge: {
      hasTauri: true,
      listen: async (event, handler) => {
        listeners[event].push(handler);
        return () => {};
      },
      // 掛載時通道已有資料（世代 0）——用來確認「切換前有資料」這個前提，之後的 empty
      // 推送才有意義區分「本來就空」與「切換後才空」。
      getSnapshot: async () => ({
        channel: 'tw-events',
        status: 'ok',
        data: 'has-data',
        meta: { loadedAt: 1000 },
        generation: 0,
      }),
      getSettings: async () => ({ widgets: {} }),
      getPause: async () => ({ paused: false, reason: null }),
      getEditMode: async () => false,
      getWidgetFontState: async () => null,
      updateSettings: async () => {},
      setEditMode: async () => {},
      reportContent: () => {},
    },
    common: {},
    dynamicImport: async () => ({
      mount(container, ctx) {
        ctx.onData((v) => received.push(v));
      },
    }),
    mainPromise: null,
  };
  const sandbox = {
    document: documentStub,
    window: windowStub,
    URLSearchParams,
    ResizeObserver: ResizeObserverStub,
    console,
    __test: testGlobals,
  };
  vm.createContext(sandbox);
  vm.runInContext(scriptSrc, sandbox, { filename: 'widget.html (extracted, empty-push)' });
  await sandbox.__test.mainPromise;

  // 資料目錄切換：核心對已訂閱通道推送新世代（1）的 empty（snapshot: null）。
  dispatch('data', { channel: 'tw-events', generation: 1, snapshot: null });

  return { received };
}

/**
 * fix F3（.superpowers/sdd/tasks/reviews/task-2.7-datadir-opus.md [medium]）：「同世代 ok 先到、
 * empty 後到」。改 `data_dir` 重建註冊表後，排程執行緒可能先輪詢到新目錄的檔案、送出新世代的
 * ok，重建後補推的 empty 才抵達（核心已改在 registry 鎖內推送，這裡是前端的第二道防線）。
 * 同一世代內核心不會在 ok 之後送出真正代表「變空」的訊息，所以同世代的 empty 不得蓋掉 ok：
 *   - 掛載前（緩衝）：查詢在途時 ok(1) 與 empty(1) 依序抵達，查詢以舊世代（0）的 ok resolve
 *     ——掛載時必須是 ok(1)，不是 empty(1)，也不是舊世代的資料。
 *   - 掛載後：ok(2) 與 empty(2) 依序抵達——onData 不得收到 empty(2)。
 */
async function runSameGenerationEmptyAfterOkScenario(scriptSrc) {
  const rootEl = makeElementStub('div');
  const documentStub = {
    documentElement: { dataset: {} }, // widget.html 寫入 data-widget（清單收合，design.md D5）
    body: makeElementStub('body'),
    getElementById: (id) => (id === 'widget-root' ? rootEl : null),
    createElement: (tag) => makeElementStub(tag),
    title: '',
  };
  const windowStub = { location: { search: '?w=clock' } };
  class ResizeObserverStub {
    observe() {}
    disconnect() {}
  }
  const listeners = { data: [], settings: [], 'edit-mode': [], 'edit-preview': [], pause: [], 'widget-font': [] };
  const dispatch = (name, payload) => {
    for (const fn of listeners[name]) fn({ payload });
  };

  let releaseSnapshot;
  const snapshotGate = new Promise((res) => {
    releaseSnapshot = res;
  });
  let snapshotStartedResolve;
  const snapshotStarted = new Promise((res) => {
    snapshotStartedResolve = res;
  });
  const mountObservations = { snapshot: undefined };
  const received = [];

  const testGlobals = {
    registry: {
      getWidget: (id) => (id === 'clock' ? { id: 'clock', channel: 'tw-events' } : null),
    },
    bridge: {
      hasTauri: true,
      listen: async (event, handler) => {
        listeners[event].push(handler);
        return () => {};
      },
      getSnapshot: async () => {
        snapshotStartedResolve();
        await snapshotGate;
        // 查詢在重建前就已讀出：舊世代的 ok。
        return {
          channel: 'tw-events',
          status: 'ok',
          data: 'old-dir',
          meta: { loadedAt: 1000 },
          generation: 0,
        };
      },
      getSettings: async () => ({ widgets: {} }),
      getPause: async () => ({ paused: false, reason: null }),
      getEditMode: async () => false,
      getWidgetFontState: async () => null,
      updateSettings: async () => {},
      setEditMode: async () => {},
      reportContent: () => {},
    },
    common: {},
    dynamicImport: async () => ({
      mount(container, ctx) {
        mountObservations.snapshot = ctx.snapshot;
        ctx.onData((v) => received.push(v));
      },
    }),
    mainPromise: null,
  };
  const sandbox = {
    document: documentStub,
    window: windowStub,
    URLSearchParams,
    ResizeObserver: ResizeObserverStub,
    console,
    __test: testGlobals,
  };
  vm.createContext(sandbox);
  vm.runInContext(scriptSrc, sandbox, {
    filename: 'widget.html (extracted, same-generation empty after ok)',
  });

  await snapshotStarted;
  dispatch('data', {
    channel: 'tw-events',
    generation: 1,
    snapshot: { channel: 'tw-events', data: 'new-dir-g1', meta: { loadedAt: 2000 } },
  });
  dispatch('data', { channel: 'tw-events', generation: 1, snapshot: null });
  releaseSnapshot();
  await sandbox.__test.mainPromise;

  dispatch('data', {
    channel: 'tw-events',
    generation: 2,
    snapshot: { channel: 'tw-events', data: 'new-dir-g2', meta: { loadedAt: 3000 } },
  });
  dispatch('data', { channel: 'tw-events', generation: 2, snapshot: null });

  return { mountObservations, received };
}

/**
 * task 5.6 fix round 1：`get_pause` 初始查詢。`queryResult` 是查詢回覆；`eventDuringQuery`
 * 不為 undefined 時，在查詢仍在途（已呼叫、未 resolve）時送出這筆 pause 事件，之後查詢才
 * resolve。回傳掛載後 onPause handler 依序收到的 payload。
 */
async function runPauseScenario(scriptSrc, { queryResult, eventDuringQuery }) {
  const rootEl = makeElementStub('div');
  const documentStub = {
    documentElement: { dataset: {} }, // widget.html 寫入 data-widget（清單收合，design.md D5）
    body: makeElementStub('body'),
    getElementById: (id) => (id === 'widget-root' ? rootEl : null),
    createElement: (tag) => makeElementStub(tag),
    title: '',
  };
  const windowStub = { location: { search: '?w=clock' } };
  class ResizeObserverStub {
    observe() {}
    disconnect() {}
  }
  const listeners = { data: [], settings: [], 'edit-mode': [], 'edit-preview': [], pause: [], 'widget-font': [] };
  const dispatch = (name, payload) => {
    for (const fn of listeners[name]) fn({ payload });
  };

  let releasePause;
  const pauseGate = new Promise((res) => {
    releasePause = res;
  });
  let pauseStartedResolve;
  const pauseStarted = new Promise((res) => {
    pauseStartedResolve = res;
  });
  const received = [];

  const testGlobals = {
    registry: {
      getWidget: (id) => (id === 'clock' ? { id: 'clock', channel: 'tw-events' } : null),
    },
    bridge: {
      hasTauri: true,
      listen: async (event, handler) => {
        listeners[event].push(handler);
        return () => {};
      },
      getSnapshot: async () => ({ channel: 'tw-events', status: 'empty', data: null, meta: null }),
      getSettings: async () => ({ widgets: {} }),
      getPause: async () => {
        pauseStartedResolve();
        await pauseGate;
        return queryResult;
      },
      getEditMode: async () => false,
      getWidgetFontState: async () => null,
      updateSettings: async () => {},
      setEditMode: async () => {},
      reportContent: () => {},
    },
    common: {},
    dynamicImport: async () => ({
      mount(container, ctx) {
        ctx.onPause((v) => received.push(v));
      },
    }),
    mainPromise: null,
  };
  const sandbox = {
    document: documentStub,
    window: windowStub,
    URLSearchParams,
    ResizeObserver: ResizeObserverStub,
    console,
    __test: testGlobals,
  };
  vm.createContext(sandbox);
  vm.runInContext(scriptSrc, sandbox, { filename: 'widget.html (extracted, pause query)' });

  // 舊版 widget.html 根本不呼叫 getPause：等不到就直接放行，讓 main() 照舊跑完再斷言。
  await Promise.race([pauseStarted, new Promise((res) => setTimeout(res, 200))]);
  if (eventDuringQuery !== undefined) {
    dispatch('pause', eventDuringQuery);
  }
  releasePause();
  await sandbox.__test.mainPromise;
  return received;
}

async function main() {
  const overridePath = process.argv[2];
  const rawHtml = readFileSync(overridePath || widgetHtmlPath, 'utf8');
  const scriptSrc = transformForHarness(extractModuleScript(rawHtml));

  const { mountObservations, lateEvents } = await runScenario(scriptSrc);
  const queryRace = await runQueryRaceScenario(scriptSrc);
  const generation = await runGenerationScenario(scriptSrc);
  const emptyPush = await runEmptyPushScenario(scriptSrc);
  const sameGenEmpty = await runSameGenerationEmptyAfterOkScenario(scriptSrc);
  const pauseFromQuery = await runPauseScenario(scriptSrc, {
    queryResult: { paused: true, reason: 'manual' },
  });
  const pauseEventWins = await runPauseScenario(scriptSrc, {
    queryResult: { paused: true, reason: 'manual' },
    eventDuringQuery: { paused: false, reason: null },
  });

  const failures = [];

  // data／settings：掛載時拿到的初始值應該是「動態 import 期間才抵達、比查詢回覆更新」的
  // 版本 2；退而求其次，掛載後立刻補送給 onData／onSettings handler 也算數。兩者都沒有
  // 才是真的遺失。
  const gotV2Data =
    mountObservations.snapshot?.data === 'v2' || lateEvents.data.some((v) => v.data === 'v2');
  if (!gotV2Data) {
    failures.push(
      'data：掛載時仍是舊版本（ctx.snapshot.data=' +
        JSON.stringify(mountObservations.snapshot?.data) +
        '，掛載後補送清單=' +
        JSON.stringify(lateEvents.data) +
        '）——動態 import 期間收到的事件被丟棄。',
    );
  }
  const gotV2Settings =
    mountObservations.settings?.v === 2 || lateEvents.settings.some((v) => v.v === 2);
  if (!gotV2Settings) {
    failures.push(
      'settings：掛載時仍是舊版本（ctx.settings.v=' +
        JSON.stringify(mountObservations.settings?.v) +
        '，掛載後補送清單=' +
        JSON.stringify(lateEvents.settings) +
        '）——動態 import 期間收到的事件被丟棄。',
    );
  }
  if (!lateEvents.editMode.some((v) => v?.enabled === true)) {
    failures.push('edit-mode：動態 import 期間收到的事件沒有在掛載後補送給 onEditMode handler。');
  }
  if (!lateEvents.pause.some((v) => v?.paused === true)) {
    failures.push('pause：動態 import 期間收到的事件沒有在掛載後補送給 onPause handler。');
  }

  // round 2：查詢仍在途時 v2 事件先抵達，查詢晚一步才用舊版本 v1 resolve——掛載時必須是
  // v2，不能被「較晚 resolve」的 v1 蓋過去（task-4.1-codex-r1.md [high]）。
  if (queryRace.mountObservations.snapshot?.data !== 'v2') {
    failures.push(
      'data（查詢仍在途時事件先抵達）：掛載時是舊版本（ctx.snapshot.data=' +
        JSON.stringify(queryRace.mountObservations.snapshot?.data) +
        '）——查詢完成順序蓋過了資料本身的新舊，v1 查詢晚一步 resolve 蓋掉了已抵達的 v2 事件。',
    );
  }
  if (queryRace.mountObservations.settings?.v !== 2) {
    failures.push(
      'settings（查詢仍在途時事件先抵達）：掛載時是舊版本（ctx.settings.v=' +
        JSON.stringify(queryRace.mountObservations.settings?.v) +
        '）——查詢完成順序蓋過了資料本身的新舊，v1 查詢晚一步 resolve 蓋掉了已抵達的 v2 事件。',
    );
  }

  // task 4.1 fix round 3：registry 重建後的 empty（新世代）必須勝過緩衝的舊世代事件；掛載後
  // 晚到的舊世代事件要丟棄，新世代事件照常派送。
  const genMount = generation.mountObservations.snapshot;
  if (genMount?.status !== 'empty' || genMount?.data !== null) {
    failures.push(
      'data（舊目錄事件 → registry 重建 → empty 查詢）：掛載時應為新世代的 empty，實際=' +
        JSON.stringify(genMount) +
        '——切換資料目錄前的舊資料被復活。',
    );
  }
  const lateData = generation.late.map((v) => v?.data);
  if (lateData.includes('old-dir-late') || !lateData.includes('new-dir')) {
    failures.push(
      'data（掛載後的世代過濾）：舊世代事件應丟棄、新世代事件應派送，onData 實際收到=' +
        JSON.stringify(lateData),
    );
  }

  // task 2.7 follow-up：重建後的 empty 推送（snapshot:null）要正規化成 status=empty，且
  // data/meta 皆為 null、世代照實帶，掛載後照常派送（世代遞增，不是該丟棄的舊推送）。
  const pushed = emptyPush.received.at(-1);
  if (
    !pushed ||
    pushed.status !== 'empty' ||
    pushed.data !== null ||
    pushed.meta !== null ||
    pushed.generation !== 1
  ) {
    failures.push(
      'data（重建後的 empty 推送，snapshot:null）：onData 應收到 ' +
        '{status:"empty",data:null,meta:null,generation:1}，實際=' +
        JSON.stringify(pushed),
    );
  }

  // fix F3（review 2.7 medium）：同世代 empty 晚於 ok 抵達時不得蓋掉 ok（緩衝與掛載後兩處）。
  const sameGenMount = sameGenEmpty.mountObservations.snapshot;
  if (sameGenMount?.status !== 'ok' || sameGenMount?.data !== 'new-dir-g1') {
    failures.push(
      'data（掛載前：同世代 ok 先到、empty 後到）：掛載時應為 ok(1) new-dir-g1，實際=' +
        JSON.stringify(sameGenMount) +
        '——同世代的 empty 蓋掉了已緩衝的 ok。',
    );
  }
  const sameGenAfter = sameGenEmpty.received;
  if (
    sameGenAfter.some((v) => v?.status === 'empty') ||
    sameGenAfter.at(-1)?.data !== 'new-dir-g2'
  ) {
    failures.push(
      'data（掛載後：同世代 ok 先到、empty 後到）：onData 不得收到同世代的 empty，實際=' +
        JSON.stringify(sameGenAfter),
    );
  }

  // task 5.6 fix round 1：宿主暫停中重新載入／重建的頁面，掛載後必須立刻知道「目前暫停中」。
  if (pauseFromQuery.at(-1)?.paused !== true) {
    failures.push(
      'pause（get_pause 初始查詢）：宿主回報暫停中、掛載前沒有任何 pause 事件，onPause 收到的' +
        '最後一筆應為 paused=true，實際=' +
        JSON.stringify(pauseFromQuery) +
        '——頁面重新載入後會在暫停中恢復動畫。',
    );
  }
  // 查詢仍在途時收到的 pause 事件以事件為準（pause 沒有版本欄位，比照 settings 規則）。
  if (pauseEventWins.length === 0 || pauseEventWins.some((v) => v?.paused === true)) {
    failures.push(
      'pause（查詢仍在途時事件先抵達）：事件 paused=false 應勝過晚一步 resolve 的查詢 paused=true，' +
        'onPause 實際收到=' +
        JSON.stringify(pauseEventWins),
    );
  }

  if (failures.length > 0) {
    console.error('[widget-init-race] FAIL');
    for (const f of failures) console.error('  - ' + f);
    process.exit(1);
  }
  console.log(
    '[widget-init-race] PASS：動態 import 期間、以及查詢仍在途時收到的 data/settings/pause/' +
      'edit-mode 事件都沒有遺失，也沒有被較晚 resolve 的舊查詢蓋過去；get_pause 初始查詢生效；' +
      '重建後的 empty 推送（snapshot:null）正規化並派送正確。',
  );
}

main();
