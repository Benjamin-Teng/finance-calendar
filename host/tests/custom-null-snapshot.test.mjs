// host/tests/custom-null-snapshot.test.mjs
//
// 鎖住 Codex adversarial review（.superpowers/sdd/tasks/reviews/task-4.6-codex.md
// [medium]）指出的問題：host/ui/widgets/custom.js 的 render() 除了看 `snapshot.status`，
// 還多判斷了 `snapshot.data == null`，把「已成功載入、但檔案內容合法地就是 JSON `null`」的
// 快照也當成「尚未設定」——customN.json 允許任意合法 JSON
// （openspec/changes/archive/2026-10-08-desktop-widget-host/specs/widget-data-feed/spec.md
// 「擴充通道讀取任意 JSON」），`null` 是合法值；此時 `snapshot.status` 是 `"ok"`（資料層
// `JsonFileSource` 已經解析成功並推播），不該被判成「尚無資料」
// （同 spec「尚無資料」Requirement：只有「從未取得有效快照」才是尚無資料）。
//
// 做法：直接 `import { mount } from '../ui/widgets/custom.js'`（本模組是純 DOM 操作，
// 不像 widget.html 骨架那樣需要抽取內嵌 script），用最小的假 `document`／`container`／
// `ctx.common` 餵給 `mount()`，檢查 render 後 `sub`（更新時間）與 `body`（內容摘要）的
// 文字內容。`ctx.common` 傳假物件（只實作 `applyAppearance`／`esc`）避免依賴真的
// `host/ui/common.js`（其 `applyAppearance` 預設參數會touch 真的 `document.documentElement`）。
//
// 執行：node host/tests/custom-null-snapshot.test.mjs [custom.js 路徑，預設
// host/ui/widgets/custom.js]。選用路徑參數讓這支測試也能對「修正前」的版本
// （例如 `git show <commit>:host/ui/widgets/custom.js > 暫存檔` 出來的內容）重跑取得 RED
// 證據——global-constraints.md 禁止 git checkout/reset 等動到整個工作樹的指令。
//
// 通過條件：四個情境（首次載入 status=ok data=null／從物件更新為 status=ok data=null／
// status=empty 顯示尚未設定／status=ok 帶物件資料正常摘要）全部成立，缺一個就 FAIL。

import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const defaultModPath = path.join(__dirname, '..', 'ui', 'widgets', 'custom.js');

/** 極簡元素樁：`mount()` 只用到 classList.add／style 屬性賦值／append／innerHTML／
 * textContent／className，不需要真的 DOM。 */
function makeElementStub(tag) {
  const el = {
    tagName: tag,
    style: {},
    className: '',
    classList: { add() {} },
    _children: [],
    append(...kids) {
      this._children.push(...kids);
    },
  };
  let text = '';
  let html = '';
  Object.defineProperty(el, 'textContent', {
    get() {
      return text;
    },
    set(v) {
      text = v;
    },
  });
  Object.defineProperty(el, 'innerHTML', {
    get() {
      return html;
    },
    set(v) {
      html = v;
    },
  });
  return el;
}

/** 假 `document`：`mount()` 建立 header／ttl／sub／body 四個元素。 */
function makeDocumentStub() {
  return { createElement: (tag) => makeElementStub(tag) };
}

/** 假 `ctx.common`：只提供 `mount()` 實際呼叫到的兩個函式，`esc` 用真正的規則（簡化版，
 * 本測試不餵含特殊字元的內容，直接原樣輸出即可）。 */
function makeFakeCommon() {
  return {
    applyAppearance() {},
    esc: (s) => String(s ?? ''),
  };
}

async function mountWith(mountFn, snapshot) {
  const container = makeElementStub('div');
  const settingsHandlers = [];
  const dataHandlers = [];
  const ctx = {
    common: makeFakeCommon(),
    config: { id: 'custom1', channel: 'custom1', width: 360, maxHeight: 480 },
    snapshot,
    settings: {},
    onData: (fn) => dataHandlers.push(fn),
    onSettings: (fn) => settingsHandlers.push(fn),
  };
  mountFn(container, ctx);
  // header 是第一個 append 進 container 的元素，body 是第二個。
  const [header, body] = container._children;
  const [, sub] = header._children;
  return {
    push: (snap) => dataHandlers.forEach((fn) => fn(snap)),
    subText: () => sub.textContent,
    bodyHtml: () => body.innerHTML,
  };
}

async function main() {
  const overridePath = process.argv[2];
  const modUrl = overridePath
    ? pathToFileURL(path.resolve(overridePath)).href
    : pathToFileURL(defaultModPath).href;
  const { mount } = await import(modUrl);

  global.document = makeDocumentStub();

  const failures = [];

  // 1. 首次載入：status='ok'、data=null（customN.json 內容合法地就是 JSON null）——
  //    應顯示更新時間與內容摘要，不是「尚未設定」。
  {
    const h = await mountWith(mount, {
      channel: 'custom1',
      status: 'ok',
      data: null,
      meta: { loadedAt: 1700000000000 },
    });
    if (h.bodyHtml().includes('尚未設定')) {
      failures.push('情境1：status=ok 的 null 快照不應顯示「尚未設定」，實際=' + h.bodyHtml());
    }
    if (h.subText() === '') {
      failures.push('情境1：status=ok 的 null 快照應顯示更新時間，實際為空字串');
    }
    if (!h.bodyHtml().includes('null')) {
      failures.push('情境1：內容摘要應如實呈現 null，實際=' + h.bodyHtml());
    }
  }

  // 2. 先有物件快照（正常顯示摘要），之後透過 onData 收到 status='ok'、data=null 的更新——
  //    應該跟著更新成 null 的摘要與新的 loadedAt，不能被判成「尚未設定」或卡在舊摘要。
  {
    const h = await mountWith(mount, {
      channel: 'custom1',
      status: 'ok',
      data: { items: [1, 2, 3] },
      meta: { loadedAt: 1700000000000 },
    });
    if (h.bodyHtml().includes('尚未設定')) {
      failures.push('情境2初始：物件快照不應顯示「尚未設定」');
    }
    h.push({ channel: 'custom1', status: 'ok', data: null, meta: { loadedAt: 1700000100000 } });
    if (h.bodyHtml().includes('尚未設定')) {
      failures.push('情境2更新後：status=ok 的 null 更新不應顯示「尚未設定」，實際=' + h.bodyHtml());
    }
    if (!h.bodyHtml().includes('null')) {
      failures.push('情境2更新後：內容摘要應更新為 null，實際=' + h.bodyHtml());
    }
    if (h.subText() === '') {
      failures.push('情境2更新後：更新時間不應被清空');
    }
  }

  // 3. 對照組：status='empty'（從未取得有效快照）——仍應顯示「尚未設定」，確保修法沒有
  //    連帶破壞真正「尚無資料」的情境。
  {
    const h = await mountWith(mount, { channel: 'custom2', status: 'empty', data: null, meta: null });
    if (!h.bodyHtml().includes('尚未設定')) {
      failures.push('情境3：status=empty 應顯示「尚未設定」，實際=' + h.bodyHtml());
    }
    if (h.subText() !== '') {
      failures.push('情境3：status=empty 時更新時間應為空字串');
    }
  }

  // 4. 對照組：status='ok' 帶物件資料——既有摘要邏輯（頂層鍵與筆數）不受本次修法影響。
  {
    const h = await mountWith(mount, {
      channel: 'custom1',
      status: 'ok',
      data: { items: [1, 2, 3], updated: '2026-09-28' },
      meta: { loadedAt: 1700000000000 },
    });
    if (!h.bodyHtml().includes('2 個頂層鍵')) {
      failures.push('情境4：物件快照應正常顯示頂層鍵摘要，實際=' + h.bodyHtml());
    }
  }

  if (failures.length > 0) {
    console.error('[custom-null-snapshot] FAIL');
    for (const f of failures) console.error('  - ' + f);
    process.exit(1);
  }
  console.log(
    '[custom-null-snapshot] PASS：status=ok 的 null 快照（首次載入與從物件更新為 null）皆' +
      '正確顯示更新時間與內容摘要，status=empty 與物件快照兩個對照情境不受影響。',
  );
}

main().catch((err) => {
  console.error('[custom-null-snapshot] 測試本身丟出例外：', err);
  process.exit(1);
});
