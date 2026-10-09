// host/tests/widget-edit-mode.test.mjs
//
// task 5.3（design.md D9；specs/widget-host-windows「編輯版面」）：鎖住 widget.html 內嵌腳本
// 「編輯版面外框／游標＋是否掛 `data-tauri-drag-region`」這段邏輯（見該檔 main() 內
// `applyEditAffordance`）——真正的拖曳（`startDragging()`／`WM_EXITSIZEMOVE`）需要 Win32，不在
// 這支測試涵蓋範圍，那部分的證據見 host/tools/evidence/5.3-*.log；這裡只測「能不能開始拖」這個
// 純前端狀態機：
//
//   draggable = editModeActive && !layoutLocked
//
// 其中 `layoutLocked` 讀的是 `Settings.layout_locked`（權威來源，見 host/src/widgets.rs
// `apply_layout_lock` 對它與 `edit_mode` 旗標耦合的說明），不是 `edit_mode` 本身——本測試特別
// 涵蓋「進入編輯模式但版面仍鎖定」這個防禦性分支，即使目前唯一的解鎖入口（系統匣「編輯版面」）
// 理論上不會產生這個組合。
//
// 做法與 host/tests/widget-init-race.test.mjs 相同：把 widget.html 內嵌的
// <script type="module"> 抽出來，只把靜態／動態 import 換成測試樁，在 Node vm context 內
// 逐行照跑原始邏輯。
//
// 執行：node host/tests/widget-edit-mode.test.mjs [widget.html 路徑，預設 host/ui/widget.html]

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

/** 極簡 `classList`：只支援本測試用得到的 `toggle`／`contains`。 */
function makeClassList() {
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
  const handlers = new Map();
  const el = {
    tagName: tag,
    className: '',
    textContent: '',
    _children: [],
    _parent: null,
    _innerHTML: '',
    classList: makeClassList(),
    // task 7.6：調整大小把手會 addEventListener('mousedown')、並在離開編輯版面時 remove()。
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
    // task 7.3：widget.html 新增的 `placeholder.style.display = ...`（佔位外框顯示切換）在
    // main() 掛載後的路徑上無條件執行，DOM 樁需要一個可寫的 `style` 物件撐過去。
    style: {},
    appendChild(child) {
      child._parent = this;
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
    hasAttribute(name) {
      return attrs.has(name);
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

/**
 * 跑一次情境：載入 `settings.layout_locked`＝`initialLocked`，`beforeMountEditMode`＝真時在
 * 動態 import 完成前先送一筆 `edit-mode:true`（模擬「頁面在編輯模式中途重新載入」），掛載完成
 * 後再依 `postMountEvents` 逐筆送事件（`{type:'edit-mode', enabled}` 或
 * `{type:'settings', layout_locked}`）。回傳掛載當下與每次事件之後 `container` 的
 * `data-tauri-drag-region` 屬性值與 `document.body` 的 class 狀態序列，供各情境個別斷言。
 */
async function runScenario(
  scriptSrc,
  {
    initialLocked,
    beforeMountEditMode,
    postMountEvents = [],
    noContent = false,
    editModeQuery = false,
    editModeEventDuringQuery,
  },
) {
  const rootEl = makeElementStub('div');
  const bodyEl = makeElementStub('body');
  const documentStub = {
    documentElement: { dataset: {} }, // widget.html 寫入 data-widget（清單收合，design.md D5）
    body: bodyEl,
    getElementById: (id) => (id === 'widget-root' ? rootEl : null),
    createElement: (tag) => makeElementStub(tag),
    title: '',
  };
  // widget-font-scale-per-widget：字級控制在可拖時掛 window resize 監聽、移除時解除。
  const windowStub = {
    location: { search: '?w=clock' },
    innerHeight: 400,
    addEventListener() {},
    removeEventListener() {},
  };
  // fix F1（review 7.3 H1）：`noContent` 情境要能在掛載後送一次「內容高度 0」的回呼。
  const resizeObservers = [];
  class ResizeObserverStub {
    constructor(cb) {
      this.cb = cb;
      resizeObservers.push(this);
    }
    observe() {}
    disconnect() {}
  }

  const listeners = { data: [], settings: [], 'edit-mode': [], 'edit-preview': [], pause: [], 'widget-font': [] };
  const dispatch = (name, payload) => {
    for (const fn of listeners[name]) fn({ payload });
  };

  let importStartedResolve;
  const importStarted = new Promise((res) => {
    importStartedResolve = res;
  });
  let releaseImport;
  const importGate = new Promise((res) => {
    releaseImport = res;
  });

  let mountedContainer = null;
  const resizeCalls = [];

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
      getSettings: async () => ({ widgets: {}, layout_locked: initialLocked }),
      getPause: async () => ({ paused: false, reason: null }),
      // fix F1（review 7.3 M3）：`get_edit_mode` 查詢；`editModeEventDuringQuery` 在查詢仍在途
      // 時先送一筆 edit-mode 事件（事件優先，規則比照 get_pause）。
      getEditMode: async () => {
        if (editModeEventDuringQuery !== undefined) {
          dispatch('edit-mode', editModeEventDuringQuery);
        }
        return editModeQuery;
      },
      // widget-font-scale-per-widget task 4.1：字級控制的初始查詢（行為由
      // widget-font-controls.test.mjs 鎖住，這裡只要撐過呼叫）。
      getWidgetFontState: async () => ({ font_scale: 1, at_cap: false }),
      adjustWidgetFontScale: async () => ({ font_scale: 1, at_cap: false }),
      updateSettings: async () => {},
      setEditMode: async () => {},
      reportContent: () => {},
      startResizeDragging: async (dir) => {
        resizeCalls.push(dir);
      },
    },
    common: {},
    dynamicImport: async () => {
      importStartedResolve();
      await importGate;
      return {
        mount(container, _ctx) {
          mountedContainer = container;
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

  await importStarted;
  if (beforeMountEditMode) {
    dispatch('edit-mode', true);
  }
  releaseImport();
  await sandbox.__test.mainPromise;

  if (noContent) {
    for (const ro of resizeObservers) {
      ro.cb([{ borderBoxSize: [{ blockSize: 0, inlineSize: 0 }] }]);
    }
  }

  for (const ev of postMountEvents) {
    if (ev.type === 'edit-mode') {
      dispatch('edit-mode', ev.enabled);
    } else if (ev.type === 'settings') {
      dispatch('settings', { widgets: {}, layout_locked: ev.layout_locked });
    } else if (ev.type === 'edit-preview') {
      dispatch('edit-preview', { id: ev.id, valid: ev.valid });
    }
  }

  const handles = bodyEl._children.filter((c) => c.getAttribute('data-resize-dir') !== null);
  // 按下第一個把手（模擬使用者按住把手），記錄 bridge.startResizeDragging 收到的方向與事件
  // 是否被攔下（不可冒泡到 Tauri 的 data-tauri-drag-region 監聽器，否則變成移動）。
  let pressed = null;
  if (handles.length > 0) {
    const ev = {
      button: 0,
      defaultPrevented: false,
      propagationStopped: false,
      preventDefault() {
        this.defaultPrevented = true;
      },
      stopPropagation() {
        this.propagationStopped = true;
      },
    };
    handles[0]._dispatch('mousedown', ev);
    await Promise.resolve();
    pressed = {
      dir: handles[0].getAttribute('data-resize-dir'),
      calls: [...resizeCalls],
      stopped: ev.propagationStopped,
    };
  }

  const placeholderEl = rootEl._children.find((c) => c.className === 'edit-placeholder') || null;

  return {
    placeholderDisplay: placeholderEl ? placeholderEl.style.display : undefined,
    placeholderDragRegion: placeholderEl
      ? placeholderEl.getAttribute('data-tauri-drag-region')
      : undefined,
    handles: handles.map((h) => ({
      dir: h.getAttribute('data-resize-dir'),
      dragRegion: h.getAttribute('data-tauri-drag-region'),
    })),
    pressed,
    dragRegion: mountedContainer.getAttribute('data-tauri-drag-region'),
    bodyEditMode: bodyEl.classList.contains('edit-mode'),
    bodyEditLocked: bodyEl.classList.contains('edit-locked'),
    bodyEditInvalid: bodyEl.classList.contains('edit-invalid'),
  };
}

async function main() {
  const overridePath = process.argv[2];
  const rawHtml = readFileSync(overridePath || widgetHtmlPath, 'utf8');
  const scriptSrc = transformForHarness(extractModuleScript(rawHtml));

  const failures = [];

  // 1. 預設：locked=true、從未進入編輯模式——不可拖、無外框 class。
  {
    const r = await runScenario(scriptSrc, { initialLocked: true });
    if (r.dragRegion !== null) {
      failures.push(`預設狀態不應掛 data-tauri-drag-region，實際="${r.dragRegion}"`);
    }
    if (r.bodyEditMode) {
      failures.push('預設狀態不應有 body.edit-mode');
    }
  }

  // 2. 解鎖後進入編輯模式：locked=false、掛載後收到 edit-mode:true——應可拖、外框顯示、
  //    不疊 edit-locked。
  {
    const r = await runScenario(scriptSrc, {
      initialLocked: false,
      postMountEvents: [{ type: 'edit-mode', enabled: true }],
    });
    if (r.dragRegion !== 'deep') {
      failures.push(`解鎖＋編輯模式應掛 data-tauri-drag-region="deep"，實際="${r.dragRegion}"`);
    }
    if (!r.bodyEditMode) {
      failures.push('解鎖＋編輯模式應有 body.edit-mode');
    }
    if (r.bodyEditLocked) {
      failures.push('解鎖＋編輯模式不應有 body.edit-locked');
    }
  }

  // 3. 防禦性分支：locked=true 但仍收到 edit-mode:true（理論上不會發生，見檔頭說明）——
  //    不可拖，但仍顯示「編輯模式中、鎖定」的外框樣式（edit-mode + edit-locked 都在）。
  {
    const r = await runScenario(scriptSrc, {
      initialLocked: true,
      postMountEvents: [{ type: 'edit-mode', enabled: true }],
    });
    if (r.dragRegion !== null) {
      failures.push(`鎖定中即使進入編輯模式也不應掛 data-tauri-drag-region，實際="${r.dragRegion}"`);
    }
    if (!r.bodyEditMode || !r.bodyEditLocked) {
      failures.push(
        `鎖定中進入編輯模式應同時有 body.edit-mode 與 body.edit-locked，實際 edit-mode=${r.bodyEditMode} edit-locked=${r.bodyEditLocked}`,
      );
    }
  }

  // 4. 掛載前已在編輯模式（頁面重新載入時，宿主已處於編輯模式）＋解鎖：掛載當下（不需要額外
  //    的 postMountEvents）就應該可拖——驗證 widget.html 對 edit-mode 的「掛載前緩衝」補送
  //    邏輯也會餵到本 task 新增的 handler（不只是舊有的 widget 模組自己的 handler）。
  {
    const r = await runScenario(scriptSrc, { initialLocked: false, beforeMountEditMode: true });
    if (r.dragRegion !== 'deep') {
      failures.push(
        `掛載前已在編輯模式＋解鎖，掛載當下就應可拖，實際 data-tauri-drag-region="${r.dragRegion}"`,
      );
    }
  }

  // 5. 動態解鎖：先在編輯模式＋鎖定（不可拖），之後收到 settings 事件把 layout_locked 改成
  //    false——應該立刻變成可拖，不需要重新載入頁面或重送 edit-mode 事件。
  {
    const r = await runScenario(scriptSrc, {
      initialLocked: true,
      postMountEvents: [
        { type: 'edit-mode', enabled: true },
        { type: 'settings', layout_locked: false },
      ],
    });
    if (r.dragRegion !== 'deep') {
      failures.push(
        `settings 事件解鎖後應立刻可拖，實際 data-tauri-drag-region="${r.dragRegion}"`,
      );
    }
  }

  // 6. 離開編輯模式：解鎖中先進編輯模式（可拖），之後收到 edit-mode:false——應該立刻不可拖、
  //    外框 class 移除。
  {
    const r = await runScenario(scriptSrc, {
      initialLocked: false,
      postMountEvents: [
        { type: 'edit-mode', enabled: true },
        { type: 'edit-mode', enabled: false },
      ],
    });
    if (r.dragRegion !== null) {
      failures.push(`離開編輯模式後不應再掛 data-tauri-drag-region，實際="${r.dragRegion}"`);
    }
    if (r.bodyEditMode) {
      failures.push('離開編輯模式後不應再有 body.edit-mode');
    }
  }

  // ── task 7.5：edit-preview 紅框（design.md D4 `edit-preview { id, valid }`）──────────
  const editOn = { type: 'edit-mode', enabled: true };

  // 7. 編輯版面中收到自己的 valid:false → body.edit-invalid。
  {
    const r = await runScenario(scriptSrc, {
      initialLocked: false,
      postMountEvents: [editOn, { type: 'edit-preview', id: 'clock', valid: false }],
    });
    if (!r.bodyEditInvalid) {
      failures.push('收到自己的 edit-preview valid:false 應加上 body.edit-invalid（紅框）');
    }
  }

  // 8. 之後收到自己的 valid:true → 紅框消失。
  {
    const r = await runScenario(scriptSrc, {
      initialLocked: false,
      postMountEvents: [
        editOn,
        { type: 'edit-preview', id: 'clock', valid: false },
        { type: 'edit-preview', id: 'clock', valid: true },
      ],
    });
    if (r.bodyEditInvalid) {
      failures.push('收到自己的 edit-preview valid:true 後紅框應消失');
    }
  }

  // 9. 別的小工具的 valid:false 不理會。
  {
    const r = await runScenario(scriptSrc, {
      initialLocked: false,
      postMountEvents: [editOn, { type: 'edit-preview', id: 'macro', valid: false }],
    });
    if (r.bodyEditInvalid) {
      failures.push('id 不是自己的 edit-preview 不應影響紅框');
    }
  }

  // 10. 紅框中離開編輯版面 → 清掉；再進入編輯版面也不殘留。
  {
    const r = await runScenario(scriptSrc, {
      initialLocked: false,
      postMountEvents: [
        editOn,
        { type: 'edit-preview', id: 'clock', valid: false },
        { type: 'edit-mode', enabled: false },
      ],
    });
    if (r.bodyEditInvalid) {
      failures.push('離開編輯版面時應清掉紅框');
    }
    const r2 = await runScenario(scriptSrc, {
      initialLocked: false,
      postMountEvents: [
        editOn,
        { type: 'edit-preview', id: 'clock', valid: false },
        { type: 'edit-mode', enabled: false },
        editOn,
      ],
    });
    if (r2.bodyEditInvalid) {
      failures.push('離開再進入編輯版面，不應殘留上一次拖曳的紅框');
    }
  }

  // 11. 不在編輯版面時收到 valid:false（理論上不會發生）不顯示紅框。
  {
    const r = await runScenario(scriptSrc, {
      initialLocked: false,
      postMountEvents: [{ type: 'edit-preview', id: 'clock', valid: false }],
    });
    if (r.bodyEditInvalid) {
      failures.push('不在編輯版面時不應顯示紅框');
    }
  }

  // ── task 7.6：調整大小把手（design.md D7「調整大小的機制」）────────────────────────
  const ALL_DIRS = [
    'East',
    'North',
    'NorthEast',
    'NorthWest',
    'South',
    'SouthEast',
    'SouthWest',
    'West',
  ];

  // 12. 解鎖＋編輯版面：body 下有八個把手（四邊四角），全部標 data-tauri-drag-region="false"；
  //     按下把手呼叫 startResizeDragging(該方向)，且事件不再往外冒泡（不觸發移動）。
  {
    const r = await runScenario(scriptSrc, { initialLocked: false, postMountEvents: [editOn] });
    const dirs = r.handles.map((h) => h.dir).sort();
    if (JSON.stringify(dirs) !== JSON.stringify(ALL_DIRS)) {
      failures.push(`編輯版面應有八個把手 ${ALL_DIRS.join(',')}，實際 ${dirs.join(',')}`);
    }
    if (r.handles.some((h) => h.dragRegion !== 'false')) {
      failures.push('每個把手都應標 data-tauri-drag-region="false"（避免被攔成移動）');
    }
    if (!r.pressed || JSON.stringify(r.pressed.calls) !== JSON.stringify([r.pressed.dir])) {
      failures.push(
        `按下把手應呼叫 startResizeDragging(該方向) 一次，實際 ${JSON.stringify(r.pressed)}`,
      );
    }
    if (!r.pressed || !r.pressed.stopped) {
      failures.push('按下把手的 mousedown 應 stopPropagation，不可冒泡到拖曳區');
    }
  }

  // 13. 鎖定（預設）、鎖定中的編輯模式、離開編輯版面：都沒有把手（非編輯模式拉不動）。
  {
    const cases = [
      ['預設鎖定', { initialLocked: true }],
      ['鎖定中進入編輯模式', { initialLocked: true, postMountEvents: [editOn] }],
      [
        '離開編輯版面',
        {
          initialLocked: false,
          postMountEvents: [editOn, { type: 'edit-mode', enabled: false }],
        },
      ],
    ];
    for (const [name, opts] of cases) {
      const r = await runScenario(scriptSrc, opts);
      if (r.handles.length !== 0) {
        failures.push(`${name}時不應有調整大小把手，實際 ${r.handles.length} 個`);
      }
    }
  }

  // 14. 進出編輯版面兩次：把手不重複累積（仍是八個）。
  {
    const r = await runScenario(scriptSrc, {
      initialLocked: false,
      postMountEvents: [editOn, { type: 'edit-mode', enabled: false }, editOn, editOn],
    });
    if (r.handles.length !== 8) {
      failures.push(`重複進入編輯版面後把手應仍為 8 個，實際 ${r.handles.length}`);
    }
  }

  // ── fix F1（review 7.3 H1）：無內容的小工具在編輯版面畫的佔位外框也要能拖 ──────────
  // container 被小工具模組設成 display:none 時沒有可點面積，拖曳區只掛在它身上等於拖不動；
  // 佔位外框是 container 的 sibling，要自己帶 data-tauri-drag-region（鎖定時一樣不帶）。

  // 15. 無內容＋編輯版面＋未鎖定：佔位外框顯示且帶拖曳屬性。
  {
    const r = await runScenario(scriptSrc, {
      initialLocked: false,
      noContent: true,
      postMountEvents: [editOn],
    });
    if (r.placeholderDisplay !== 'flex') {
      failures.push(`無內容＋編輯版面應顯示佔位外框，實際 display="${r.placeholderDisplay}"`);
    }
    if (r.placeholderDragRegion !== 'deep') {
      failures.push(
        `無內容＋編輯版面＋未鎖定，佔位外框應帶 data-tauri-drag-region="deep"，實際="${r.placeholderDragRegion}"`,
      );
    }
  }

  // ── fix F1（review 7.3 M3）：get_edit_mode 初始查詢 ───────────────────────────────
  // edit-mode 事件只在切換時廣播一次；編輯版面期間頁面 Reload／故障重建，新頁面沒收到過事件，
  // 要靠查詢知道目前在編輯版面，否則核心顯示了視窗、頁面卻不畫佔位外框也不掛拖曳區。

  // 18. 查詢回報編輯版面中、掛載前沒有任何 edit-mode 事件：掛載後就是編輯版面（可拖、外框）。
  {
    const r = await runScenario(scriptSrc, {
      initialLocked: false,
      noContent: true,
      editModeQuery: true,
    });
    if (r.dragRegion !== 'deep' || !r.bodyEditMode) {
      failures.push(
        `get_edit_mode 回 true 時掛載後應在編輯版面，實際 drag="${r.dragRegion}" edit-mode=${r.bodyEditMode}`,
      );
    }
    if (r.placeholderDisplay !== 'flex' || r.placeholderDragRegion !== 'deep') {
      failures.push(
        `get_edit_mode 回 true＋無內容應畫佔位外框且可拖，實際 display="${r.placeholderDisplay}" drag="${r.placeholderDragRegion}"`,
      );
    }
  }

  // 19. 查詢仍在途時收到 edit-mode:false，查詢晚一步回舊值 true：以事件為準（不在編輯版面）。
  {
    const r = await runScenario(scriptSrc, {
      initialLocked: false,
      editModeQuery: true,
      editModeEventDuringQuery: false,
    });
    if (r.dragRegion !== null || r.bodyEditMode) {
      failures.push(
        `查詢期間收到的 edit-mode 事件應優先於查詢回覆，實際 drag="${r.dragRegion}" edit-mode=${r.bodyEditMode}`,
      );
    }
  }

  // 16. 無內容＋編輯版面＋鎖定：佔位外框仍顯示，但不可拖。
  {
    const r = await runScenario(scriptSrc, {
      initialLocked: true,
      noContent: true,
      postMountEvents: [editOn],
    });
    if (r.placeholderDragRegion !== null) {
      failures.push(`鎖定時佔位外框不應帶拖曳屬性，實際="${r.placeholderDragRegion}"`);
    }
  }

  // 17. 無內容：離開編輯版面後佔位外框隱藏、拖曳屬性移除。
  {
    const r = await runScenario(scriptSrc, {
      initialLocked: false,
      noContent: true,
      postMountEvents: [editOn, { type: 'edit-mode', enabled: false }],
    });
    if (r.placeholderDisplay !== 'none' || r.placeholderDragRegion !== null) {
      failures.push(
        `離開編輯版面後佔位外框應隱藏且不帶拖曳屬性，實際 display="${r.placeholderDisplay}" drag="${r.placeholderDragRegion}"`,
      );
    }
  }

  if (failures.length > 0) {
    console.error('[widget-edit-mode] FAIL');
    for (const f of failures) console.error('  - ' + f);
    process.exit(1);
  }
  console.log(
    '[widget-edit-mode] PASS：data-tauri-drag-region 與 body.edit-mode/edit-locked 皆正確反映' +
      'editModeActive×layoutLocked 的六種情境（預設鎖定、解鎖後可拖、鎖定時防禦性阻擋、掛載前已' +
      '在編輯模式、settings 事件動態解鎖、離開編輯模式立刻停止），edit-preview 紅框五種情境' +
      '（出現、消失、他人事件不理會、離開編輯版面清除且不殘留、非編輯模式不顯示）亦正確；' +
      '調整大小把手三種情境（編輯版面八個把手且不觸發移動、鎖定／離開時沒有把手、重複進入不累積）亦正確；' +
      '無內容佔位外框三種情境（編輯版面未鎖定可拖、鎖定不可拖、離開後隱藏）亦正確；' +
      'get_edit_mode 初始查詢兩種情境（查詢回編輯版面即生效、查詢期間事件優先）亦正確。',
  );
}

main().catch((err) => {
  console.error('[widget-edit-mode] 測試本身丟出例外：', err);
  process.exit(1);
});
