// host/tests/widget-fetch-activity.test.mjs
//
// fetch-progress-and-manual-refresh task 3.3／3.5（design.md D4／D8；specs/finance-widgets「抓取進行
// 中的顯示」、specs/widget-data-feed「尚無資料」「抓取完成後立即更新快照」）：鎖住 widget.html
// 內嵌腳本的「抓取進行中的顯示」：
//   - 訂閱：財經小工具先 listen('fetch-activity') 再查 get_fetch_activity，查詢在途時收到的事件
//     優先；擴充插槽不訂閱也不查。
//   - 整塊載入：needsData 的小工具沒有有效快照、且抓取啟用並（進行中或沒跑過）時，隱藏內容容器、
//     顯示載入區塊並回報有內容；解除時依當下大小重報（行情條仍無資料 → 回報無內容）。
//   - 角落圖示：一輪進行中且不是整塊載入時，body 下多一個圖示；內容容器不變；結束時移除。
//   - 沒有抓取進行、且有資料時，頁面不多出任何節點。
//   - D8：結束事件先到、快照後到——期間維持載入（不出現無資料提示），主動查詢快照的回覆到了才
//     離開；回覆確定沒有資料才顯示無資料文字；查詢逾時（2 秒）照當下狀態顯示；活動狀態查詢被拒
//     （未知）不顯示載入，之後收到事件能恢復。
//
// 做法同 widget-font-controls.test.mjs：抽出 widget.html 的 <script type="module">，把 import 換成
// 測試樁（common 用真的 host/ui/common.js），在 Node vm context 內照跑原始邏輯。小工具模組用一個
// 假模組：有資料時畫「content:<data>」，沒有時畫 `common.fetchActivityView` 的文字（行情條型的
// 假模組則改成整個隱藏，同 quotes.js）。
//
// 執行：node --test host/tests/widget-fetch-activity.test.mjs

import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';

import * as common from '../ui/common.js';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const widgetHtmlPath = path.join(__dirname, '..', 'ui', 'widget.html');
const cssPath = path.join(__dirname, '..', 'ui', 'widget.css');

function extractModuleScript(html) {
  const matches = [...html.matchAll(/<script type="module">([\s\S]*?)<\/script>/g)];
  if (matches.length !== 1) {
    throw new Error(`widget.html 預期恰好一個 <script type="module"> 區塊，實際 ${matches.length} 個`);
  }
  return matches[0][1];
}

function transformForHarness(code) {
  const replacements = [
    ["import { getWidget } from './registry.js';", 'const { getWidget } = globalThis.__test.registry;'],
    ["import * as bridge from './bridge.js';", 'const bridge = globalThis.__test.bridge;'],
    ["import * as common from './common.js';", 'const common = globalThis.__test.common;'],
    ['mod = await import(`./widgets/${id}.js`);', 'mod = await globalThis.__test.dynamicImport(id);'],
    ['main();', 'globalThis.__test.mainPromise = main();'],
  ];
  let out = code;
  for (const [from, to] of replacements) {
    if (!out.includes(from)) {
      throw new Error(`widget.html 抽出的腳本找不到這段原文，測試的替換假設過期：\n${from}`);
    }
    out = out.replace(from, to);
  }
  return out;
}

const scriptSrc = transformForHarness(extractModuleScript(readFileSync(widgetHtmlPath, 'utf8')));

function makeClassList(el) {
  const set = new Set();
  return {
    toggle(name, force) {
      const on = force === undefined ? !set.has(name) : !!force;
      if (on) set.add(name);
      else set.delete(name);
      el._mutations++;
    },
    add(...names) {
      for (const n of names) set.add(n);
      el._mutations++;
    },
    remove(...names) {
      for (const n of names) set.delete(n);
      el._mutations++;
    },
    contains(name) {
      return set.has(name);
    },
    get size() {
      return set.size;
    },
  };
}

function makeElementStub(tag) {
  const attrs = new Map();
  const el = {
    tagName: String(tag).toUpperCase(),
    className: '',
    textContent: '',
    title: '',
    _children: [],
    _parent: null,
    _mutations: 0,
    style: {},
    addEventListener() {},
    remove() {
      if (this._parent) {
        this._parent._children = this._parent._children.filter((c) => c !== this);
        this._parent = null;
      }
    },
    appendChild(child) {
      child._parent = this;
      this._children.push(child);
      return child;
    },
    append(...children) {
      for (const c of children) this.appendChild(c);
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
    get children() {
      return this._children;
    },
    querySelectorAll() {
      return [];
    },
    // 內容容器的高度：被外殼隱藏（fetch-hidden）或被模組設成 display:none 時為 0，否則 100。
    getBoundingClientRect() {
      const hidden = this.classList.contains('fetch-hidden') || this.style.display === 'none';
      const h = this._isContainer && !hidden ? 100 : 0;
      return { left: 0, top: 0, right: h ? 300 : 0, bottom: h, width: h ? 300 : 0, height: h };
    },
  };
  el.classList = makeClassList(el);
  return el;
}

function hasClass(el, cls) {
  return el.className.split(/\s+/).includes(cls) || el.classList.contains(cls);
}

function findAll(root, cls) {
  const out = [];
  const walk = (n) => {
    if (hasClass(n, cls)) out.push(n);
    for (const c of n._children) walk(c);
  };
  walk(root);
  return out;
}

const flush = async () => {
  for (let i = 0; i < 10; i++) await Promise.resolve();
};

const WIDGETS = {
  clock: { id: 'clock', channel: 'tw-events' },
  fixed: { id: 'fixed', channel: 'tw-events' },
  macro: { id: 'macro', channel: 'tw-events', needsData: true },
  dynamic: { id: 'dynamic', channel: 'tw-events', needsData: true },
  quotes: { id: 'quotes', channel: 'tw-events', needsData: true },
  custom1: { id: 'custom1', channel: 'custom1' },
};

const EMPTY = { channel: 'tw-events', status: 'empty', data: null, meta: null, generation: 0 };
const ok = (data, loadedAt = 1000, generation = 0) => ({
  channel: 'tw-events',
  status: 'ok',
  data,
  meta: { loadedAt },
  generation,
});
const act = (running, enabled, last) => ({ running, enabled, last });

/**
 * 掛載一個小工具：
 * - `id`：registry 項目（WIDGETS）；行情條型（quotes）的假模組無資料時整個隱藏。
 * - `initialSnapshot`：掛載時 get_snapshot 的回覆。
 * - `activityQuery`：get_fetch_activity 的回覆（`null`＝bridge 已把拒絕轉成 null，即未知）。
 * - `activityEventsDuringQuery`：查詢在途時先送出的 fetch-activity 事件 payload。
 * - `snapshotQuery`：掛載後（D8）主動查詢時的回覆；函式＝自己控制（可回傳懸而未決的 Promise）。
 */
async function mountWidget({
  id = 'macro',
  initialSnapshot = EMPTY,
  activityQuery = act(false, true, 'ok'),
  activityEventsDuringQuery = [],
  snapshotQuery = async () => EMPTY,
  editMode = false,
  locked = false,
} = {}) {
  const rootEl = makeElementStub('div');
  const bodyEl = makeElementStub('body');
  const documentStub = {
    documentElement: { dataset: {} },
    body: bodyEl,
    getElementById: (i) => (i === 'widget-root' ? rootEl : null),
    createElement: (t) => makeElementStub(t),
    title: '',
  };
  const resizeObservers = [];
  class ResizeObserverStub {
    constructor(cb) {
      this.cb = cb;
      this.last = undefined;
      resizeObservers.push(this);
    }
    observe(target) {
      this.target = target;
    }
    disconnect() {}
  }
  const listeners = {};
  const dispatch = (name, payload) => {
    for (const fn of listeners[name] || []) fn({ payload });
  };
  const timers = new Map();
  let timerSeq = 0;
  const calls = { getSnapshot: 0, getFetchActivity: 0, listen: [] };
  const reports = [];
  const windowStub = {
    location: { search: `?w=${id}` },
    innerHeight: 400,
    addEventListener() {},
    removeEventListener() {},
  };

  const testGlobals = {
    registry: { getWidget: (i) => WIDGETS[i] ?? null },
    bridge: {
      hasTauri: true,
      listen: async (event, handler) => {
        calls.listen.push(event);
        (listeners[event] ||= []).push(handler);
        return () => {};
      },
      getSnapshot: async () => {
        calls.getSnapshot++;
        return calls.getSnapshot === 1 ? initialSnapshot : snapshotQuery();
      },
      getSettings: async () => ({ widgets: {}, layout_locked: locked }),
      getPause: async () => ({ paused: false, reason: null }),
      getEditMode: async () => editMode,
      getWidgetFontState: async () => ({ font_scale: 1, at_cap: false }),
      getFetchActivity: async () => {
        calls.getFetchActivity++;
        for (const p of activityEventsDuringQuery) dispatch('fetch-activity', p);
        return activityQuery;
      },
      adjustWidgetFontScale: async () => ({ font_scale: 1, at_cap: false }),
      updateSettings: async () => {},
      setEditMode: async () => {},
      reportContent: (v) => reports.push(v),
      startResizeDragging: async () => {},
    },
    common,
    dynamicImport: async () => ({
      mount(container, ctx) {
        container._isContainer = true;
        let snap = ctx.snapshot;
        const render = () => {
          if (snap && snap.status === 'ok') {
            container.textContent = `content:${snap.data}`;
            container.style.display = '';
            return;
          }
          if (id === 'quotes') {
            container.textContent = '';
            container.style.display = 'none';
            return;
          }
          container.textContent = common.fetchActivityView({
            status: 'empty',
            needsData: true,
            activity: ctx.fetchActivity(),
            dataDir: 'D:/data',
          }).text;
        };
        render();
        ctx.onData((s) => {
          snap = s;
          render();
        });
        ctx.onFetchActivity(() => render());
      },
    }),
    mainPromise: null,
  };
  const sandbox = {
    document: documentStub,
    window: windowStub,
    URLSearchParams,
    ResizeObserver: ResizeObserverStub,
    console: { ...console, warn() {} },
    setTimeout: (fn, ms) => {
      const handle = ++timerSeq;
      timers.set(handle, { fn, ms });
      return handle;
    },
    clearTimeout: (handle) => {
      timers.delete(handle);
    },
    __test: testGlobals,
  };
  vm.createContext(sandbox);
  vm.runInContext(scriptSrc, sandbox, { filename: 'widget.html (extracted, fetch activity)' });
  await sandbox.__test.mainPromise;

  const container = () => rootEl._children.find((c) => c._isContainer);
  const loading = () => findAll(rootEl, 'fetch-loading')[0];
  const corner = () => findAll(bodyEl, 'fetch-corner')[0];
  // 模擬 ResizeObserver：內容容器高度與上次回呼時不同才回呼（第一次一定回呼）。
  const settle = () => {
    for (const ro of resizeObservers) {
      const h = ro.target.getBoundingClientRect().height;
      if (h !== ro.last) {
        ro.last = h;
        ro.cb([{ borderBoxSize: [{ blockSize: h, inlineSize: h ? 300 : 0 }] }]);
      }
    }
  };
  settle();
  // 畫面上「看得到」的文字：整塊載入時是載入文字，內容容器被隱藏時是空字串。
  const visibleText = () => {
    const l = loading();
    if (l) return l._children.map((c) => c.textContent).join('');
    const c = container();
    if (!c || c.classList.contains('fetch-hidden') || c.style.display === 'none') return '';
    return c.textContent;
  };
  const runTimers = () => {
    for (const [handle, t] of [...timers]) {
      timers.delete(handle);
      t.fn();
    }
  };
  return {
    rootEl,
    bodyEl,
    calls,
    reports,
    timers,
    dispatch,
    settle,
    container,
    loading,
    corner,
    visibleText,
    runTimers,
    activity: (p) => {
      dispatch('fetch-activity', p);
      settle();
    },
    data: (snapshot) => {
      dispatch('data', { channel: 'tw-events', generation: snapshot.generation ?? 0, snapshot: snapshot.status === 'ok' ? { channel: 'tw-events', data: snapshot.data, meta: snapshot.meta } : null });
      settle();
    },
  };
}

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

// ── 3.3 整塊載入 ──────────────────────────────────────────────────────────────

test('首次安裝、首抓還沒開始：needsData 的小工具整塊載入、隱藏內容、回報有內容', async () => {
  const w = await mountWidget({ activityQuery: act(false, true, 'none') });
  assert.ok(w.loading(), '要有載入區塊');
  assert.equal(w.visibleText(), '資料抓取中…');
  assert.ok(w.container().classList.contains('fetch-hidden'));
  assert.equal(w.reports.at(-1), true);
  assert.ok(!w.corner(), '整塊載入時不顯示角落圖示');
});

test('載入 → 收到資料：回到內容、載入區塊移除、仍回報有內容', async () => {
  const w = await mountWidget({ activityQuery: act(true, true, 'none') });
  assert.ok(w.loading());
  assert.ok(!w.corner(), '一輪進行中但整塊載入：不另外顯示角落圖示');
  w.data(ok('v1'));
  assert.ok(!w.loading(), '有資料後載入區塊移除');
  assert.equal(w.visibleText(), 'content:v1');
  assert.ok(!w.container().classList.contains('fetch-hidden'));
  assert.equal(w.reports.at(-1), true);
  assert.ok(w.corner(), '抓取仍在進行，改顯示角落圖示');
  w.activity(act(false, true, 'ok'));
  assert.ok(!w.corner(), '結束後角落圖示消失');
});

test('行情條：載入期間保持可見（回報有內容）；載入 → 失敗仍無資料 → 回報無內容', async () => {
  const w = await mountWidget({ id: 'quotes', activityQuery: act(true, true, 'none') });
  assert.ok(w.loading());
  assert.deepEqual(w.reports, [true], '載入期間只回報有內容，不因內容容器量到 0 而回報無內容');
  w.activity(act(false, true, 'failed'));
  await flush();
  w.settle();
  assert.ok(!w.loading(), '快照查詢確定沒有資料後離開載入');
  assert.equal(w.reports.at(-1), false, '解除時依當下大小重報：行情條仍無資料 → 無內容');
});

test('總經日曆：載入 → 失敗後顯示無資料文字，整段期間不曾回報無內容（視窗不閃一下隱藏）', async () => {
  const w = await mountWidget({ activityQuery: act(true, true, 'none') });
  w.activity(act(false, true, 'failed'));
  await flush();
  w.settle();
  assert.ok(!w.loading());
  assert.match(w.visibleText(), /產生 log\.txt/);
  assert.deepEqual(w.reports, [true], '解除載入時依當下大小（有文字）重報，不先送一次 false');
});

test('抓取已關閉或上一輪失敗：不整塊載入，顯示無資料文字', async () => {
  const off = await mountWidget({ activityQuery: act(false, false, 'none') });
  assert.ok(!off.loading());
  assert.match(off.visibleText(), /抓取已關閉/);
  const failed = await mountWidget({ activityQuery: act(false, true, 'failed') });
  assert.ok(!failed.loading());
  assert.match(failed.visibleText(), /產生 log\.txt/);
});

test('時鐘、台股固定事件（不需要資料）：抓取中只出現角落圖示，不整塊載入', async () => {
  for (const id of ['clock', 'fixed']) {
    const w = await mountWidget({ id, activityQuery: act(true, true, 'none') });
    assert.ok(!w.loading(), id);
    assert.ok(w.corner(), id);
  }
});

// ── 3.3 角落圖示與「沒有抓取時不多節點」────────────────────────────────────────

test('已有資料時開始一輪：只在 body 下多一個角落圖示，內容容器與回報不變；結束後移除', async () => {
  const w = await mountWidget({ initialSnapshot: ok('v1'), activityQuery: act(false, true, 'ok') });
  const c = w.container();
  const before = { mutations: c._mutations, text: c.textContent, children: w.rootEl._children.length, reports: [...w.reports] };
  w.activity(act(true, true, 'ok'));
  const icon = w.corner();
  assert.ok(icon, '要有角落圖示');
  assert.equal(icon._parent, w.bodyEl, '圖示掛在 body 下（#widget-root 之外）');
  assert.ok(!w.rootEl._children.includes(icon));
  assert.equal(icon.getAttribute('aria-hidden'), 'true');
  assert.equal(c._mutations, before.mutations, '內容容器的 class 不被動到');
  assert.equal(c.textContent, before.text);
  assert.equal(w.rootEl._children.length, before.children, '#widget-root 下不多節點');
  assert.deepEqual(w.reports, before.reports, '不重報內容');
  assert.ok(!w.loading());
  w.activity(act(false, true, 'ok'));
  assert.ok(!w.corner());
  assert.equal(w.calls.getSnapshot, 1, '已有資料的小工具在一輪結束時不主動查快照');
});

test('沒有抓取進行、且有資料：body 與 #widget-root 下沒有任何抓取提示節點', async () => {
  for (const id of ['clock', 'macro', 'quotes']) {
    const w = await mountWidget({ id, initialSnapshot: ok('v1'), activityQuery: act(false, true, 'ok') });
    assert.equal(findAll(w.bodyEl, 'fetch-spinner').length, 0, id);
    assert.equal(w.bodyEl._children.length, 0, `${id}：body 下沒有外殼新增的節點`);
    assert.equal(w.rootEl._children.length, 2, `${id}：#widget-root 只有內容容器與佔位外框`);
    assert.ok(!w.container().classList.contains('fetch-hidden'));
  }
});

test('擴充插槽：不訂閱 fetch-activity、不查詢活動狀態、不顯示提示', async () => {
  const w = await mountWidget({ id: 'custom1', activityQuery: act(true, true, 'none') });
  assert.ok(!w.calls.listen.includes('fetch-activity'));
  assert.equal(w.calls.getFetchActivity, 0);
  w.dispatch('fetch-activity', act(true, true, 'none'));
  assert.ok(!w.corner());
  assert.ok(!w.loading());
});

test('訂閱順序：先 listen("fetch-activity") 再查詢；查詢在途時收到的事件優先於查詢回覆', async () => {
  const w = await mountWidget({
    initialSnapshot: ok('v1'),
    activityQuery: act(false, true, 'ok'),
    activityEventsDuringQuery: [act(true, true, 'ok')],
  });
  assert.ok(w.calls.listen.includes('fetch-activity'));
  assert.ok(w.corner(), '查詢在途時收到「進行中」事件：以事件為準');
});

test('編輯版面（解鎖）時整塊載入區塊也是拖曳區；鎖定時不是', async () => {
  const unlocked = await mountWidget({ activityQuery: act(true, true, 'none'), editMode: true });
  assert.equal(unlocked.loading().getAttribute('data-tauri-drag-region'), 'deep');
  unlocked.dispatch('edit-mode', false);
  assert.equal(unlocked.loading().getAttribute('data-tauri-drag-region'), null);
  const locked = await mountWidget({ activityQuery: act(true, true, 'none'), editMode: true, locked: true });
  assert.equal(locked.loading().getAttribute('data-tauri-drag-region'), null);
});

test('暫停事件：body 加上 motion-paused（動畫靜止），恢復時移除', async () => {
  const w = await mountWidget({ initialSnapshot: ok('v1'), activityQuery: act(true, true, 'ok') });
  w.dispatch('pause', { paused: true, reason: 'manual' });
  assert.ok(w.bodyEl.classList.contains('motion-paused'));
  w.dispatch('pause', { paused: false, reason: null });
  assert.ok(!w.bodyEl.classList.contains('motion-paused'));
});

// ── 3.5 結束通知與快照的送達順序（design.md D8）────────────────────────────────

test('結束事件先到、快照後到：期間維持載入、不出現無資料提示；快照到了直接變成內容', async () => {
  const reply = deferred();
  const w = await mountWidget({
    activityQuery: act(true, true, 'none'),
    snapshotQuery: () => reply.promise,
  });
  assert.ok(w.loading());
  w.activity(act(false, true, 'ok'));
  await flush();
  assert.ok(w.loading(), '結束事件先到：維持載入');
  assert.equal(w.visibleText(), '資料抓取中…', '不出現無資料提示');
  assert.equal(w.calls.getSnapshot, 2, '結束時主動查一次快照');
  reply.resolve(ok('fresh', 2000));
  await flush();
  w.settle();
  assert.ok(!w.loading());
  assert.equal(w.visibleText(), 'content:fresh');
  assert.equal(w.reports.at(-1), true);
  assert.equal(w.timers.size, 0, '回覆到了就取消逾時計時');
});

test('快照查詢回覆確定沒有資料：才顯示無資料文字（上一輪失敗）', async () => {
  const reply = deferred();
  const w = await mountWidget({ activityQuery: act(true, true, 'none'), snapshotQuery: () => reply.promise });
  w.activity(act(false, true, 'failed'));
  await flush();
  assert.equal(w.visibleText(), '資料抓取中…');
  reply.resolve(EMPTY);
  await flush();
  assert.ok(!w.loading());
  assert.match(w.visibleText(), /^尚無資料 —— 最近一次資料抓取失敗.*產生 log\.txt/);
});

test('快照查詢逾時（2 秒）：照當下狀態顯示；晚到的有效快照仍會採用', async () => {
  const reply = deferred();
  const w = await mountWidget({ activityQuery: act(true, true, 'none'), snapshotQuery: () => reply.promise });
  w.activity(act(false, true, 'failed'));
  await flush();
  assert.ok(w.loading());
  const timeouts = [...w.timers.values()].map((t) => t.ms);
  assert.deepEqual(timeouts, [2000], '逾時 2 秒');
  w.runTimers();
  w.settle();
  assert.ok(!w.loading(), '逾時後離開載入');
  assert.match(w.visibleText(), /最近一次資料抓取失敗/);
  reply.resolve(ok('late', 3000));
  await flush();
  w.settle();
  assert.equal(w.visibleText(), 'content:late', '晚到的有效快照照樣採用');
});

test('快照查詢被拒：立即照當下狀態顯示', async () => {
  const w = await mountWidget({
    activityQuery: act(true, true, 'none'),
    snapshotQuery: async () => {
      throw new Error('ipc');
    },
  });
  w.activity(act(false, true, 'failed'));
  await flush();
  assert.ok(!w.loading());
  assert.match(w.visibleText(), /最近一次資料抓取失敗/);
});

test('快照經 data 事件先到（早於查詢回覆）：立即離開載入；之後同版本的查詢回覆不重派送', async () => {
  const reply = deferred();
  const w = await mountWidget({ activityQuery: act(true, true, 'none'), snapshotQuery: () => reply.promise });
  const seen = [];
  w.activity(act(false, true, 'ok'));
  await flush();
  w.data(ok('fresh', 2000));
  assert.ok(!w.loading());
  assert.equal(w.visibleText(), 'content:fresh');
  const c = w.container();
  const textBefore = c.textContent;
  c.textContent = 'sentinel';
  reply.resolve(ok('fresh', 2000));
  await flush();
  seen.push(c.textContent);
  assert.equal(seen[0], 'sentinel', '同版本的查詢回覆不得再派送給模組');
  assert.equal(textBefore, 'content:fresh');
});

test('查詢回覆是舊世代的快照：丟棄（不蓋掉換資料目錄後的新世代）', async () => {
  const reply = deferred();
  const w = await mountWidget({
    initialSnapshot: { ...EMPTY, generation: 2 },
    activityQuery: act(true, true, 'none'),
    snapshotQuery: () => reply.promise,
  });
  w.activity(act(false, true, 'ok'));
  await flush();
  reply.resolve(ok('old-dir', 5000, 1));
  await flush();
  assert.ok(!w.loading());
  assert.doesNotMatch(w.visibleText(), /old-dir/);
});

test('活動狀態查詢被拒（未知）：不顯示載入、只顯示一般無資料文字；收到事件後恢復', async () => {
  const w = await mountWidget({ activityQuery: null });
  assert.ok(!w.loading(), '未知時不顯示載入');
  assert.equal(w.visibleText(), '尚無資料 —— 資料目錄：D:/data');
  assert.doesNotMatch(w.visibleText(), /失敗/);
  w.activity(act(true, true, 'none'));
  assert.ok(w.loading(), '收到事件後恢復整塊載入');
  assert.equal(w.reports.at(-1), true);
});

test('未知 → 第一筆事件就是「沒有進行中」：不觸發 D8 查詢，照狀態顯示', async () => {
  const w = await mountWidget({ activityQuery: null });
  w.activity(act(false, true, 'failed'));
  await flush();
  assert.equal(w.calls.getSnapshot, 1);
  assert.match(w.visibleText(), /最近一次資料抓取失敗/);
});

// ── widget.css：動畫只動 transform、減少動態效果與暫停時靜止、字級控制顯示時隱藏角落圖示 ─────

const css = readFileSync(cssPath, 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
function rulesMatching(selectorPart) {
  return [...css.matchAll(/([^{}]+)\{([^{}]*)\}/g)]
    .map((m) => ({ sel: m[1].trim().replace(/\s+/g, ' '), body: m[2] }))
    .filter((r) => r.sel.includes(selectorPart));
}

test('widget.css：減少動態效果時關掉轉圈動畫', () => {
  const at = css.indexOf('@media (prefers-reduced-motion: reduce)');
  assert.ok(at >= 0, '要有 prefers-reduced-motion 區塊');
  const block = css.slice(at, css.indexOf('}', css.indexOf('}', at) + 1) + 1);
  assert.match(block, /\.fetch-spinner\s*\{[^}]*animation:\s*none/);
});

test('widget.css：暫停時動畫停住、字級控制顯示時角落圖示隱藏、角落圖示不攔滑鼠', () => {
  assert.ok(rulesMatching('body.motion-paused .fetch-spinner').some((r) => /animation-play-state:\s*paused/.test(r.body)));
  assert.ok(rulesMatching('body.font-controls-shown .fetch-corner').some((r) => /display:\s*none/.test(r.body)));
  const corner = rulesMatching('.fetch-corner').find((r) => r.sel === '.fetch-corner');
  assert.match(corner.body, /pointer-events:\s*none/);
  assert.match(corner.body, /position:\s*fixed/);
});

test('widget.css：轉圈的關鍵影格只動 transform', () => {
  const at = css.indexOf('@keyframes fetch-spin');
  assert.ok(at >= 0);
  const block = css.slice(at, css.indexOf('}', css.indexOf('}', at) + 1) + 1);
  const props = [...block.matchAll(/([a-z-]+)\s*:/g)].map((m) => m[1]);
  assert.deepEqual([...new Set(props)], ['transform']);
});

test('widget.css：整塊載入期間隱藏內容容器的規則壓得過模組的 inline display', () => {
  assert.ok(rulesMatching('#widget-root > .fetch-hidden').some((r) => /display:\s*none\s*!important/.test(r.body)));
});
