// host/tests/widget-font-controls.test.mjs
//
// widget-font-scale-per-widget task 4.1（design.md D3／D4；specs「編輯版面調整個別字級」）：鎖住
// widget.html 內嵌腳本的「編輯版面字級控制」（A−／百分比／A+）：
//   - 顯示條件：編輯版面且未鎖定才有控制（含無內容的佔位外框狀態）；鎖定、離開編輯版面時不存在。
//   - 停用條件：字級 0.5 時 A− 停用；字級 3.0 或 at_cap 時 A+ 停用，at_cap 另以 title 與控制下方
//     一行小字提示「已達框大小上限，請把小工具拉大」。
//   - `widget-font` 事件只處理自己 id 的那筆。
//   - 按鈕呼叫 adjust_widget_font_scale(±1)，回傳後立即更新；失敗保留原值並短暫顯示錯誤。
//   - 不觸發拖曳：控制與其子元素都標 data-tauri-drag-region="false"，pointerdown／mousedown 不冒泡。
//   - 初始化競態：查詢在途時先到的事件優先，不被較舊的查詢回覆覆蓋（比照 widget-init-race）。
//
// 做法同 widget-edit-mode.test.mjs：抽出 widget.html 的 <script type="module">，把 import 換成
// 測試樁，在 Node vm context 內照跑原始邏輯。
//
// 執行：node --test host/tests/widget-font-controls.test.mjs

import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const widgetHtmlPath = path.join(__dirname, '..', 'ui', 'widget.html');

const AT_CAP_HINT = '已達框大小上限，請把小工具拉大';

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

function makeClassList() {
  const set = new Set();
  return {
    toggle(name, force) {
      const on = force === undefined ? !set.has(name) : !!force;
      if (on) set.add(name);
      else set.delete(name);
    },
    add(...names) {
      for (const n of names) set.add(n);
    },
    remove(...names) {
      for (const n of names) set.delete(n);
    },
    contains(name) {
      return set.has(name);
    },
  };
}

// 幾何樁：物件（className 或 '#id' → [left, top, right, bottom]），或函式——每次量測時以
// `{ controlsShown }`（量測「當下」body 是否已有 font-controls-shown）呼叫、回傳同形物件，用來模擬
// 「為控制保留空間的 CSS 規則生效前後版面不同」（修正輪 4：量測必須在 class 同步之後）。
let currentRects = null;
let currentBody = null;
function rectsNow() {
  if (typeof currentRects !== 'function') {
    return currentRects;
  }
  return currentRects({
    controlsShown: !!currentBody && currentBody.classList.contains('font-controls-shown'),
  });
}

function makeElementStub(tag) {
  const attrs = new Map();
  const handlers = new Map();
  const el = {
    tagName: String(tag).toUpperCase(),
    className: '',
    textContent: '',
    title: '',
    disabled: false,
    type: '',
    _children: [],
    _parent: null,
    _innerHTML: '',
    classList: makeClassList(),
    style: {},
    addEventListener(type, fn) {
      if (!handlers.has(type)) handlers.set(type, []);
      handlers.get(type).push(fn);
    },
    _dispatch(type, event) {
      for (const fn of handlers.get(type) || []) fn(event);
    },
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
    hasAttribute(name) {
      return attrs.has(name);
    },
    // 幾何：測試以 globalThis.__rects（className 或 id → rect）指定，未指定＝零尺寸。
    id: '',
    get children() {
      return this._children;
    },
    querySelectorAll(sel) {
      const cls = sel.startsWith('.') ? sel.slice(1) : null;
      const out = [];
      const visit = (n) => {
        for (const c of n._children) {
          if (cls && c.className.split(/\s+/).includes(cls)) out.push(c);
          visit(c);
        }
      };
      visit(this);
      return out;
    },
    getBoundingClientRect() {
      const rects = rectsNow();
      const r = (rects && (rects[this.className] || rects['#' + this.id])) || null;
      const [left, top, right, bottom] = r || [0, 0, 0, 0];
      return { left, top, right, bottom, width: right - left, height: bottom - top };
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

function walk(node, fn) {
  fn(node);
  for (const c of node._children) walk(c, fn);
}

function hasClass(el, cls) {
  return el.className.split(/\s+/).includes(cls) || el.classList.contains(cls);
}

function findAll(root, cls) {
  const out = [];
  walk(root, (n) => {
    if (hasClass(n, cls)) out.push(n);
  });
  return out;
}

function makeEvent(extra = {}) {
  return {
    button: 0,
    propagationStopped: false,
    defaultPrevented: false,
    stopPropagation() {
      this.propagationStopped = true;
    },
    preventDefault() {
      this.defaultPrevented = true;
    },
    ...extra,
  };
}

const flush = async () => {
  for (let i = 0; i < 5; i++) await Promise.resolve();
};

/**
 * 跑一次掛載：
 * - `locked`：settings.layout_locked；`editMode`：get_edit_mode 查詢回覆。
 * - `fontQuery`：get_widget_font_state 回覆（`null`＝查詢失敗，bridge 已把 reject 轉成 null）。
 * - `fontEventsDuringQuery`：查詢在途時（回覆之前）送出的 widget-font 事件 payload。
 * - `noContent`：掛載後送一次內容高度 0（佔位外框狀態）。
 * - `adjust(step)`：adjust_widget_font_scale 的實作（回傳值或丟錯）。
 */
async function mountWidget({
  locked = false,
  editMode = true,
  fontQuery = { font_scale: 1, at_cap: false },
  fontEventsDuringQuery = [],
  noContent = false,
  adjust = async () => ({ font_scale: 1, at_cap: false }),
  innerHeight = 400,
  rects = null,
  panel = null,
  onModuleData = null,
  onModuleSettings = null,
} = {}) {
  currentRects = rects;
  const rootEl = makeElementStub('div');
  const bodyEl = makeElementStub('body');
  currentBody = bodyEl;
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
      resizeObservers.push(this);
    }
    observe() {}
    disconnect() {}
  }
  const listeners = {};
  const dispatch = (name, payload) => {
    for (const fn of listeners[name] || []) fn({ payload });
  };
  const adjustCalls = [];
  const timers = [];
  const windowListeners = {};
  const windowStub = {
    location: { search: '?w=clock' },
    innerHeight,
    addEventListener(type, fn) {
      (windowListeners[type] ||= []).push(fn);
    },
    removeEventListener(type, fn) {
      windowListeners[type] = (windowListeners[type] || []).filter((f) => f !== fn);
    },
  };
  const resizeWindow = (h) => {
    windowStub.innerHeight = h;
    for (const fn of windowListeners.resize || []) fn({});
  };

  const testGlobals = {
    registry: { getWidget: (i) => (i === 'clock' ? { id: 'clock', channel: 'tw-events' } : null) },
    bridge: {
      hasTauri: true,
      listen: async (event, handler) => {
        (listeners[event] ||= []).push(handler);
        return () => {};
      },
      getSnapshot: async () => ({ channel: 'tw-events', status: 'empty', data: null, meta: null }),
      getSettings: async () => ({ widgets: {}, layout_locked: locked }),
      getPause: async () => ({ paused: false, reason: null }),
      getEditMode: async () => editMode,
      getWidgetFontState: async () => {
        for (const p of fontEventsDuringQuery) dispatch('widget-font', p);
        return fontQuery;
      },
      adjustWidgetFontScale: async (step) => {
        adjustCalls.push(step);
        return adjust(step);
      },
      updateSettings: async () => {},
      setEditMode: async () => {},
      reportContent: () => {},
      startResizeDragging: async () => {},
    },
    common: {},
    // `panel`：掛載時在 container 下建出的子元素（[tag, className, id, [子元素…]]），供幾何測試。
    dynamicImport: async () => ({
      mount(container, ctx) {
        // 小工具模組自己的 data／settings 處理（重畫內容可能改變版面）；在 widget.html 的處理之前登記。
        if (onModuleData) ctx.onData(onModuleData);
        if (onModuleSettings) ctx.onSettings(onModuleSettings);
        const build = ([tag, className, id, kids = []]) => {
          const el = makeElementStub(tag);
          el.className = className;
          el.id = id || '';
          for (const k of kids) el.appendChild(build(k));
          return el;
        };
        for (const spec of panel || []) container.appendChild(build(spec));
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
    setTimeout: (fn, ms) => {
      timers.push({ fn, ms });
      return timers.length;
    },
    clearTimeout: () => {},
    __test: testGlobals,
  };
  vm.createContext(sandbox);
  vm.runInContext(scriptSrc, sandbox, { filename: 'widget.html (extracted, font controls)' });
  await sandbox.__test.mainPromise;
  if (noContent) {
    for (const ro of resizeObservers) ro.cb([{ borderBoxSize: [{ blockSize: 0, inlineSize: 0 }] }]);
  }

  const controls = () => findAll(bodyEl, 'font-controls');
  const one = (cls) => {
    const [c] = controls();
    return c ? findAll(c, cls)[0] : undefined;
  };
  return {
    bodyEl,
    rootEl,
    dispatch,
    resizeWindow,
    resizeObservers,
    windowListeners,
    adjustCalls,
    timers,
    controls,
    dec: () => one('font-dec'),
    inc: () => one('font-inc'),
    pct: () => one('font-pct'),
    hint: () => one('font-hint'),
    error: () => one('font-error'),
  };
}

function visible(el) {
  return !!el && el.style.display !== 'none' && el.textContent !== '';
}

// ── 顯示條件 ──────────────────────────────────────────────────────────────────

test('編輯版面＋未鎖定：body 下有一組字級控制，A−／百分比／A+ 齊全', async () => {
  const w = await mountWidget();
  assert.equal(w.controls().length, 1);
  assert.equal(w.dec().tagName, 'BUTTON');
  assert.equal(w.inc().tagName, 'BUTTON');
  assert.equal(w.dec().textContent, 'A−');
  assert.equal(w.inc().textContent, 'A+');
  assert.equal(w.pct().textContent, '100%');
});

test('不在編輯版面、或鎖定中：沒有字級控制', async () => {
  assert.equal((await mountWidget({ editMode: false })).controls().length, 0);
  assert.equal((await mountWidget({ locked: true })).controls().length, 0);
});

test('無內容（佔位外框）時編輯版面照樣顯示控制', async () => {
  const w = await mountWidget({ noContent: true });
  assert.equal(w.controls().length, 1);
});

test('離開編輯版面移除控制；再進入不重複累積；鎖定時移除', async () => {
  const w = await mountWidget();
  w.dispatch('edit-mode', false);
  assert.equal(w.controls().length, 0);
  w.dispatch('edit-mode', true);
  w.dispatch('edit-mode', true);
  assert.equal(w.controls().length, 1);
  w.dispatch('settings', { widgets: {}, layout_locked: true });
  assert.equal(w.controls().length, 0);
});

// ── 停用條件 ──────────────────────────────────────────────────────────────────

test('字級 0.5：A− 停用、A+ 可用', async () => {
  const w = await mountWidget({ fontQuery: { font_scale: 0.5, at_cap: false } });
  assert.equal(w.pct().textContent, '50%');
  assert.equal(w.dec().disabled, true);
  assert.equal(w.inc().disabled, false);
});

test('字級 3.0：A+ 停用、A− 可用', async () => {
  const w = await mountWidget({ fontQuery: { font_scale: 3, at_cap: false } });
  assert.equal(w.pct().textContent, '300%');
  assert.equal(w.inc().disabled, true);
  assert.equal(w.dec().disabled, false);
});

test('at_cap：A+ 停用，title 與下方一行小字提示；解除後提示消失、按鈕恢復', async () => {
  const w = await mountWidget({ fontQuery: { font_scale: 1.2, at_cap: true } });
  assert.equal(w.inc().disabled, true);
  assert.equal(w.inc().title, AT_CAP_HINT);
  assert.ok(visible(w.hint()), 'at_cap 時下方小字要顯示');
  assert.equal(w.hint().textContent, AT_CAP_HINT);
  w.dispatch('widget-font', { id: 'clock', font_scale: 1.2, at_cap: false });
  assert.equal(w.inc().disabled, false);
  assert.equal(w.inc().title, '');
  assert.ok(!visible(w.hint()), '解除上限後小字要隱藏');
});

test('浮點誤差不外洩：1.1 顯示 110%、0.7000000000000001 顯示 70%', async () => {
  const w = await mountWidget({ fontQuery: { font_scale: 1.1, at_cap: false } });
  assert.equal(w.pct().textContent, '110%');
  w.dispatch('widget-font', { id: 'clock', font_scale: 0.7000000000000001, at_cap: false });
  assert.equal(w.pct().textContent, '70%');
});

test('查詢失敗且沒收到事件：顯示佔位符號、兩鈕停用；之後事件補上', async () => {
  const w = await mountWidget({ fontQuery: null });
  assert.equal(w.pct().textContent, '—');
  assert.equal(w.dec().disabled, true);
  assert.equal(w.inc().disabled, true);
  w.dispatch('widget-font', { id: 'clock', font_scale: 1.3, at_cap: false });
  assert.equal(w.pct().textContent, '130%');
  assert.equal(w.dec().disabled, false);
  assert.equal(w.inc().disabled, false);
});

// ── widget-font 事件只處理自己 id ─────────────────────────────────────────────

test('widget-font：自己 id 的更新顯示，別人的不理會', async () => {
  const w = await mountWidget();
  w.dispatch('widget-font', { id: 'macro', font_scale: 2, at_cap: true });
  assert.equal(w.pct().textContent, '100%');
  assert.equal(w.inc().disabled, false);
  w.dispatch('widget-font', { id: 'clock', font_scale: 1.4, at_cap: false });
  assert.equal(w.pct().textContent, '140%');
});

test('不在編輯版面時收到的 widget-font 也記住，進入編輯版面時顯示最新值', async () => {
  const w = await mountWidget({ editMode: false });
  w.dispatch('widget-font', { id: 'clock', font_scale: 1.6, at_cap: false });
  w.dispatch('edit-mode', true);
  assert.equal(w.pct().textContent, '160%');
});

// ── 按鈕動作 ──────────────────────────────────────────────────────────────────

test('按 A+／A−：呼叫 adjustWidgetFontScale(+1／−1)，回傳後立即更新顯示', async () => {
  const w = await mountWidget({
    adjust: async (step) => ({ font_scale: step > 0 ? 1.1 : 0.9, at_cap: step > 0 }),
  });
  w.inc()._dispatch('click', makeEvent());
  await flush();
  assert.deepEqual([...w.adjustCalls], [1]);
  assert.equal(w.pct().textContent, '110%');
  assert.equal(w.inc().disabled, true, '回傳 at_cap 時 A+ 立即停用');
  w.dec()._dispatch('click', makeEvent());
  await flush();
  assert.deepEqual([...w.adjustCalls], [1, -1]);
  assert.equal(w.pct().textContent, '90%');
});

test('停用中的按鈕被點：不呼叫指令', async () => {
  const w = await mountWidget({ fontQuery: { font_scale: 0.5, at_cap: false } });
  w.dec()._dispatch('click', makeEvent());
  await flush();
  assert.deepEqual([...w.adjustCalls], []);
});

test('指令失敗：保留原值，控制旁短暫顯示錯誤，計時後清掉', async () => {
  const w = await mountWidget({
    adjust: async () => {
      throw '版面已鎖定';
    },
  });
  w.inc()._dispatch('click', makeEvent());
  await flush();
  assert.equal(w.pct().textContent, '100%');
  assert.ok(visible(w.error()), '失敗時要顯示錯誤');
  assert.match(w.error().textContent, /版面已鎖定/);
  assert.ok(w.timers.length > 0, '錯誤要排定計時清除（短暫顯示）');
  for (const t of w.timers.splice(0)) t.fn();
  assert.ok(!visible(w.error()), '計時到後錯誤消失');
});

// ── 不觸發拖曳 ────────────────────────────────────────────────────────────────

test('控制與其所有子元素都標 data-tauri-drag-region="false"', async () => {
  const w = await mountWidget();
  const [c] = w.controls();
  walk(c, (n) => {
    assert.equal(
      n.getAttribute('data-tauri-drag-region'),
      'false',
      `${n.tagName}.${n.className} 缺少 data-tauri-drag-region="false"`,
    );
  });
});

test('控制掛在 body 下（不在帶 deep 拖曳區的 container／佔位外框子樹內）', async () => {
  const w = await mountWidget();
  const [c] = w.controls();
  assert.equal(c._parent, w.bodyEl);
});

test('在控制上 pointerdown／mousedown：stopPropagation，不冒泡到拖曳區', async () => {
  const w = await mountWidget();
  const [c] = w.controls();
  for (const type of ['pointerdown', 'mousedown']) {
    const ev = makeEvent();
    c._dispatch(type, ev);
    assert.ok(ev.propagationStopped, `${type} 要 stopPropagation`);
  }
});

// ── 初始化競態（D3：查詢在途時收到的事件優先）────────────────────────────────

test('查詢在途時先到自己的事件：以事件為準，不被較舊的查詢回覆覆蓋', async () => {
  const w = await mountWidget({
    fontQuery: { font_scale: 1, at_cap: false },
    fontEventsDuringQuery: [{ id: 'clock', font_scale: 1.3, at_cap: true }],
  });
  assert.equal(w.pct().textContent, '130%');
  assert.equal(w.inc().disabled, true);
});

test('查詢在途時只收到別人的事件：仍採用查詢回覆', async () => {
  const w = await mountWidget({
    fontQuery: { font_scale: 1.2, at_cap: false },
    fontEventsDuringQuery: [{ id: 'macro', font_scale: 2.5, at_cap: true }],
  });
  assert.equal(w.pct().textContent, '120%');
  assert.equal(w.inc().disabled, false);
});

test('查詢失敗但查詢在途時收到過事件：用事件值', async () => {
  const w = await mountWidget({
    fontQuery: null,
    fontEventsDuringQuery: [{ id: 'clock', font_scale: 0.8, at_cap: false }],
  });
  assert.equal(w.pct().textContent, '80%');
});

// ── 協調者裁示（實機截圖遮擋）：矮視窗不顯示內嵌提示；編輯版面隱藏副標題 ─────────────

test('矮視窗（行情條約 60 CSS px）at_cap：不顯示內嵌提示，只保留 A+ 停用與 title', async () => {
  const w = await mountWidget({ fontQuery: { font_scale: 1, at_cap: true }, innerHeight: 60 });
  assert.equal(w.inc().disabled, true);
  assert.equal(w.inc().title, AT_CAP_HINT);
  assert.ok(!visible(w.hint()), '矮視窗不顯示內嵌提示');
});

test('高視窗 at_cap：內嵌提示照常顯示在控制下方', async () => {
  const w = await mountWidget({ fontQuery: { font_scale: 1, at_cap: true }, innerHeight: 300 });
  assert.ok(visible(w.hint()));
  assert.equal(w.hint().textContent, AT_CAP_HINT);
});

test('視窗高度改變（resize）時重新判斷是否顯示內嵌提示；離開編輯版面解除 resize 監聽', async () => {
  const w = await mountWidget({ fontQuery: { font_scale: 1, at_cap: true }, innerHeight: 60 });
  assert.ok(!visible(w.hint()));
  w.resizeWindow(300);
  assert.ok(visible(w.hint()), '拉高後顯示');
  w.resizeWindow(60);
  assert.ok(!visible(w.hint()), '壓矮後隱藏');
  w.dispatch('edit-mode', false);
  assert.equal((w.windowListeners.resize || []).length, 0, '控制移除時要解除 resize 監聽');
  w.dispatch('edit-mode', true);
  assert.equal((w.windowListeners.resize || []).length, 1, '再進入不重複累積監聽');
});

// 副標題（`.panel > header > .sub`：macro／fixed／dynamic／custom 的標題列右側）與標題／時鐘為控制
// 保留的空間，一律依 body 的 `font-controls-shown`（只在字級控制實際存在時才有，見下方 vm 測試）
// 生效，不依 `body.edit-mode`——鎖定中的編輯版面也有 edit-mode，但沒有控制（Codex 審查 8d24acc）。
// 用 visibility:hidden 而非 display:none，標題列高度與標題位置不變。
const css = readFileSync(path.join(__dirname, '..', 'ui', 'widget.css'), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
function cssRules() {
  return [...css.matchAll(/([^{}]+)\{([^{}]*)\}/g)].map((m) => ({
    selectors: m[1].split(',').map((x) => x.trim().replace(/\s+/g, ' ')),
    body: m[2],
  }));
}

test('widget.css：控制顯示時（body.font-controls-shown）隱藏 .panel > header > .sub（visibility:hidden）', () => {
  const hit = cssRules().find(
    (r) =>
      r.selectors.includes('body.font-controls-shown .panel > header > .sub') &&
      /visibility:\s*hidden/.test(r.body),
  );
  assert.ok(hit, '要有 `body.font-controls-shown .panel > header > .sub { visibility: hidden }`');
});

test('widget.css：副標題、標題、時鐘的編輯版面規則都不依 body.edit-mode（鎖定時沒有控制）', () => {
  for (const r of cssRules()) {
    for (const sel of r.selectors) {
      if (/body\.edit-mode/.test(sel) && /\.sub|\.ttl|\.clockcard|> header/.test(sel)) {
        assert.fail(`${sel} 應改依 body.font-controls-shown`);
      }
    }
  }
  const reserving = cssRules().filter((r) =>
    r.selectors.some((sel) => sel.startsWith('body.font-controls-shown')),
  );
  const covered = reserving.flatMap((r) => r.selectors).join(' ');
  for (const part of ['.sub', '.ttl', '.clockcard']) {
    assert.ok(covered.includes(part), `body.font-controls-shown 規則要涵蓋 ${part}`);
  }
});

test('widget.css：沒有控制時沒有規則隱藏副標題', () => {
  for (const r of cssRules()) {
    const plain = r.selectors.filter((s) => /\.sub$/.test(s) && !s.includes('font-controls-shown'));
    if (plain.length > 0) {
      assert.doesNotMatch(r.body, /visibility:\s*hidden|display:\s*none/, plain.join(','));
    }
  }
});

test('body.font-controls-shown 與控制是否存在同步：解鎖編輯有、鎖定編輯沒有、切換跟著變', async () => {
  const shown = (w) => w.bodyEl.classList.contains('font-controls-shown');
  const unlocked = await mountWidget();
  assert.equal(unlocked.controls().length, 1);
  assert.ok(shown(unlocked), '解鎖編輯版面：控制存在、body 有 class');

  const locked = await mountWidget({ locked: true });
  assert.ok(locked.bodyEl.classList.contains('edit-mode'), '前提：鎖定中仍是 edit-mode');
  assert.equal(locked.controls().length, 0);
  assert.ok(!shown(locked), '鎖定編輯版面：沒有控制、body 不可有 class');

  // 鎖定 → 解鎖 → 鎖定 → 離開編輯版面
  locked.dispatch('settings', { widgets: {}, layout_locked: false });
  assert.ok(shown(locked) && locked.controls().length === 1, '解鎖後出現');
  locked.dispatch('settings', { widgets: {}, layout_locked: true });
  assert.ok(!shown(locked) && locked.controls().length === 0, '再鎖定後消失');
  locked.dispatch('settings', { widgets: {}, layout_locked: false });
  locked.dispatch('edit-mode', false);
  assert.ok(!shown(locked) && locked.controls().length === 0, '離開編輯版面後消失');

  const off = await mountWidget({ editMode: false });
  assert.ok(!shown(off), '不在編輯版面：沒有 class');
});

test('各小工具的副標題確實是 .panel > header > .sub（選擇器以實際 DOM 為準）', () => {
  for (const f of ['macro', 'fixed', 'dynamic', 'custom']) {
    const src = readFileSync(path.join(__dirname, '..', 'ui', 'widgets', `${f}.js`), 'utf8');
    assert.match(src, /sub\.className = 'sub';/, `${f}.js 的副標題 class`);
    assert.match(src, /header\.append\(ttl, sub\)/, `${f}.js 把副標題放在 header 直接子層`);
    assert.match(src, /classList\.add\('panel'\)/,`${f}.js 的容器是 .panel`);
  }
});

test('時鐘最小框高（160 CSS px）at_cap：內嵌提示會壓到置中的時間數字，不顯示', async () => {
  const w = await mountWidget({ fontQuery: { font_scale: 1, at_cap: true }, innerHeight: 160 });
  assert.ok(!visible(w.hint()));
  assert.equal(w.inc().title, AT_CAP_HINT);
});

// ── Codex 審查修正輪：內嵌提示不得壓到標題或面板內容（幾何防護）───────────────────
// 幾何本身由 host/tests/compare/verify-font-controls-geometry.mjs 在 headless Edge 量；這裡鎖住
// widget.html 的判斷邏輯：提示的 rect 與 `.ttl`、或面板內 header 以外的子元素相交就不顯示。
const LIST_PANEL = [
  ['header', '', '', [['div', 'ttl'], ['div', 'sub']]],
  ['div', 'scroll-area'],
];

test('夠高的視窗 at_cap，但提示與 .ttl 相交（窄框）：不顯示內嵌提示', async () => {
  const w = await mountWidget({
    fontQuery: { font_scale: 1, at_cap: true },
    innerHeight: 320,
    panel: LIST_PANEL,
    rects: { 'font-hint': [34, 43, 211, 61], ttl: [27, 24, 121, 50], 'scroll-area': [9, 63, 220, 300] },
  });
  assert.ok(!visible(w.hint()), '壓到標題時不顯示');
  assert.equal(w.inc().title, AT_CAP_HINT, 'title 提示仍在');
});

test('夠高的視窗 at_cap，提示壓到標題列以外的面板內容（清單區）：不顯示', async () => {
  const w = await mountWidget({
    fontQuery: { font_scale: 1, at_cap: true },
    innerHeight: 320,
    panel: LIST_PANEL,
    rects: { 'font-hint': [200, 43, 452, 76], ttl: [27, 24, 159, 50], 'scroll-area': [9, 63, 461, 300] },
  });
  assert.ok(!visible(w.hint()));
});

test('夠高的視窗 at_cap，提示落在標題列右側空白：顯示', async () => {
  const w = await mountWidget({
    fontQuery: { font_scale: 1, at_cap: true },
    innerHeight: 320,
    panel: LIST_PANEL,
    rects: { 'font-hint': [275, 43, 452, 61], ttl: [27, 24, 159, 50], 'scroll-area': [9, 63, 461, 300] },
  });
  assert.ok(visible(w.hint()));
});

test('時鐘 at_cap：提示壓到時間文字時不顯示（面板子元素都受保護）', async () => {
  const w = await mountWidget({
    fontQuery: { font_scale: 1, at_cap: true },
    innerHeight: 300,
    panel: [['div', '', 'clockTime'], ['div', '', 'clockDate']],
    rects: { 'font-hint': [18, 43, 194, 76], '#clockTime': [9, 70, 203, 127], '#clockDate': [9, 130, 203, 152] },
  });
  assert.ok(!visible(w.hint()));
});

// ── 修正輪 4（Codex 審查 4ec15c7）：內嵌提示的碰撞判斷必須量「控制已顯示」的版面 ──────────────
// 為控制保留空間的 CSS 規則（`.ttl` 讓位、時鐘上方內距）依 body.font-controls-shown 生效；量測若在
// class 加上之前做，量到的是平時的版面——原本有空間的提示被誤藏，或原本會壓到內容的提示被放出來。
// 之後內容自己改變版面（有無內容切換、資料或設定重畫）也要重新判斷，不能停在舊的量測結果。
// 平時版面：標題沒讓位、延伸到提示下方；控制顯示後：標題讓位，提示落在右側空白。
const TTL_YIELDS = ({ controlsShown }) => ({
  'font-hint': [275, 41, 452, 59],
  ttl: controlsShown ? [19, 16, 159, 58] : [27, 24, 300, 50],
  'scroll-area': [9, 66, 461, 300],
});
// 反向：平時不相交，控制顯示後標題框變大（內距）而壓到提示。
const TTL_GROWS = ({ controlsShown }) => ({
  'font-hint': [275, 41, 452, 59],
  ttl: controlsShown ? [19, 16, 290, 58] : [27, 24, 159, 50],
  'scroll-area': [9, 66, 461, 300],
});

test('首次進入解鎖編輯版面（掛載時已在編輯版面）：以控制顯示後的版面判斷，有空間的提示要顯示', async () => {
  const w = await mountWidget({
    fontQuery: { font_scale: 1, at_cap: true },
    innerHeight: 320,
    panel: LIST_PANEL,
    rects: TTL_YIELDS,
  });
  assert.ok(w.bodyEl.classList.contains('font-controls-shown'));
  assert.ok(visible(w.hint()), '只有平時版面才相交，控制顯示後有空間：要顯示');
});

test('進入編輯版面（edit-mode 事件）與解鎖（settings 事件）建立控制時，同樣以控制顯示後的版面判斷', async () => {
  const viaEdit = await mountWidget({
    editMode: false,
    fontQuery: { font_scale: 1, at_cap: true },
    innerHeight: 320,
    panel: LIST_PANEL,
    rects: TTL_YIELDS,
  });
  viaEdit.dispatch('edit-mode', true);
  assert.ok(visible(viaEdit.hint()), 'edit-mode 進入');

  const viaUnlock = await mountWidget({
    locked: true,
    fontQuery: { font_scale: 1, at_cap: true },
    innerHeight: 320,
    panel: LIST_PANEL,
    rects: TTL_YIELDS,
  });
  viaUnlock.dispatch('settings', { widgets: {}, layout_locked: false });
  assert.ok(visible(viaUnlock.hint()), 'settings 解鎖');
});

test('首次進入解鎖編輯版面：平時不相交、控制顯示後才相交的提示不顯示', async () => {
  const w = await mountWidget({
    fontQuery: { font_scale: 1, at_cap: true },
    innerHeight: 320,
    panel: LIST_PANEL,
    rects: TTL_GROWS,
  });
  assert.ok(!visible(w.hint()));
  assert.equal(w.inc().title, AT_CAP_HINT);
});

test('控制建立後內容由無變有（ResizeObserver）：重新判斷提示，壓到新內容就收回', async () => {
  let hasRows = false;
  const w = await mountWidget({
    fontQuery: { font_scale: 1, at_cap: true },
    innerHeight: 320,
    panel: LIST_PANEL,
    noContent: true,
    rects: () => ({
      'font-hint': [275, 41, 452, 59],
      ttl: [19, 16, 159, 58],
      'scroll-area': hasRows ? [9, 40, 461, 300] : [0, 0, 0, 0],
    }),
  });
  assert.ok(visible(w.hint()), '無內容時有空間');
  hasRows = true;
  for (const ro of w.resizeObservers) ro.cb([{ borderBoxSize: [{ blockSize: 200, inlineSize: 400 }] }]);
  assert.ok(!visible(w.hint()), '內容出現、壓到提示：收回');
});

test('控制建立後小工具因資料重畫改變版面：重新判斷提示', async () => {
  let tall = false;
  const w = await mountWidget({
    fontQuery: { font_scale: 1, at_cap: true },
    innerHeight: 320,
    panel: LIST_PANEL,
    onModuleData: () => {
      tall = true; // 模組重畫後標題列變矮、清單區往上頂到提示
    },
    rects: () => ({
      'font-hint': [275, 41, 452, 59],
      ttl: [19, 16, 159, 58],
      'scroll-area': tall ? [9, 50, 461, 300] : [9, 66, 461, 300],
    }),
  });
  assert.ok(visible(w.hint()));
  w.dispatch('data', { channel: 'tw-events', generation: 0, snapshot: { channel: 'tw-events', data: {}, meta: { loadedAt: 1 } } });
  assert.ok(!visible(w.hint()), '資料重畫後壓到清單區：收回');
});

test('控制建立後小工具因設定重畫改變版面：重新判斷提示', async () => {
  let squeezed = false;
  const w = await mountWidget({
    fontQuery: { font_scale: 1, at_cap: true },
    innerHeight: 320,
    panel: LIST_PANEL,
    onModuleSettings: () => {
      squeezed = true;
    },
    rects: () => ({
      'font-hint': [275, 41, 452, 59],
      ttl: [19, 16, 159, 58],
      'scroll-area': squeezed ? [9, 50, 461, 300] : [9, 66, 461, 300],
    }),
  });
  assert.ok(visible(w.hint()));
  w.dispatch('settings', { widgets: {}, layout_locked: false });
  assert.ok(!visible(w.hint()), '設定重畫後壓到清單區：收回');
});
