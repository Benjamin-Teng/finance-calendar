// host/tests/quotes-pause-idempotent.test.mjs
//
// 重現並鎖住 Codex adversarial review（.superpowers/sdd/tasks/reviews/task-4.5-codex.md
// [medium]）指出的問題：quotes.js 的 `ctx.onPause` handler 每次收到 `{paused:false}` 都
// 無條件呼叫 `scheduleStep()`，即使跑馬燈已經在動也一樣——`scheduleStep()` 會排一筆新的
// `requestAnimationFrame`／`setTimeout` 並覆寫 `state.raf`／`state.tm`，先前那一筆若還沒
// 觸發，參照被蓋掉、`stopAnimation()` 就再也取消不到它，留下無法取消的殘留動畫鏈。之後
// 收到 `{paused:true}` 只能取消「最後一筆」，較舊的殘留鏈仍會繼續前進並不斷重新排程自己，
// 使「暫停時 SHALL 停止捲動」失效。
//
// 做法：把 host/ui/widgets/quotes.js 的 `export function mount` 拿掉 `export`，在 Node vm
// context 內執行（本檔本身不 import 任何東西，只靠 ctx.common／全域的
// requestAnimationFrame／cancelAnimationFrame／setTimeout／clearTimeout／document，可以
// 逐行照跑）。排程器換成測試可控制的假實作（用 Map 記錄「排定中、尚未觸發」的
// callback，可以手動 flush、也可以直接讀取目前排定中的筆數），不依賴真的計時器與真的
// requestAnimationFrame，讓「殘留幾條鏈」這件事變成可以精確斷言的整數，而不是猜時間。
//
// 執行：node host/tests/quotes-pause-idempotent.test.mjs
// 通過條件：重複收到 `{paused:false}`（跑馬燈本來就沒暫停）不得新增排程；之後收到
// `{paused:true}` 必須讓排定中的筆數歸零、且位置在那之後不再變化，缺一個就 FAIL、exit 1。

import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const quotesJsPath = path.join(__dirname, '..', 'ui', 'widgets', 'quotes.js');

/** 可控制、可手動 flush 的假 requestAnimationFrame／setTimeout：不依賴真的時間流逝，
 * 「目前排定中還沒觸發的筆數」可以直接讀出來斷言。 */
function makeScheduler() {
  let nextId = 1;
  const rafQueue = new Map(); // id -> cb
  const timeoutQueue = new Map(); // id -> cb

  return {
    requestAnimationFrame(cb) {
      const id = nextId++;
      rafQueue.set(id, cb);
      return id;
    },
    cancelAnimationFrame(id) {
      rafQueue.delete(id);
    },
    setTimeout(cb) {
      const id = nextId++;
      timeoutQueue.set(id, cb);
      return id;
    },
    clearTimeout(id) {
      timeoutQueue.delete(id);
    },
    pendingCount() {
      return rafQueue.size + timeoutQueue.size;
    },
    /** 觸發目前排定中的全部 raf／timeout 回呼一次（回呼執行時可能又排入新的一筆，不在這次
     * flush 內處理，呼叫端要自己決定要不要再 flush 一次）。 */
    flush(t) {
      const rafEntries = [...rafQueue.entries()];
      rafQueue.clear();
      for (const [, cb] of rafEntries) cb(t);
      const timeoutEntries = [...timeoutQueue.entries()];
      timeoutQueue.clear();
      for (const [, cb] of timeoutEntries) cb();
    },
  };
}

/** ul 元素的假實作：`scrollWidth`／`clientWidth` 固定回傳「內容超寬」，讓 render() 每次都
 * 走「頭尾相接」跑馬燈那條路徑（不需要真的解析 HTML 排版）；`children`／`offsetLeft` 用
 * 一列固定寬度模擬，只是為了讓 `loopAt` 算出一個正數，實際數值不影響本測試關心的事。 */
function makeUlStub() {
  const ROW_WIDTH = 120;
  const ul = {
    className: '',
    scrollLeft: 0,
    scrollWidth: 2000,
    clientWidth: 500,
    children: [],
    _innerHTML: '',
    appendChild() {},
    get innerHTML() {
      return ul._innerHTML;
    },
    set innerHTML(v) {
      ul._innerHTML = v;
      const rowCount = (v.match(/<li>/g) || []).length;
      ul.children = Array.from({ length: rowCount }, (_, i) => ({ offsetLeft: i * ROW_WIDTH }));
    },
    insertAdjacentHTML(_position, html) {
      const startIndex = ul.children.length;
      const rowCount = (html.match(/<li>/g) || []).length;
      for (let i = 0; i < rowCount; i++) {
        ul.children.push({ offsetLeft: (startIndex + i) * ROW_WIDTH });
      }
    },
  };
  return ul;
}

function makeContainerStub() {
  const container = {
    style: {},
    className: '',
    _innerHTML: '',
    appendChild() {},
    get innerHTML() {
      return container._innerHTML;
    },
    set innerHTML(v) {
      container._innerHTML = v;
    },
  };
  return container;
}

async function main() {
  const src = readFileSync(quotesJsPath, 'utf8').replace(
    'export function mount(container, ctx) {',
    'function mount(container, ctx) {',
  );
  const combinedSrc = `${src}\nglobalThis.__test.mount = mount;`;

  const scheduler = makeScheduler();
  const ul = makeUlStub();
  const container = makeContainerStub();

  const sandbox = {
    document: { createElement: () => ul },
    requestAnimationFrame: scheduler.requestAnimationFrame,
    cancelAnimationFrame: scheduler.cancelAnimationFrame,
    setTimeout: scheduler.setTimeout,
    clearTimeout: scheduler.clearTimeout,
    console,
    __test: {},
  };
  vm.createContext(sandbox);
  vm.runInContext(combinedSrc, sandbox, { filename: 'quotes.js (extracted)' });

  const snapshot = {
    channel: 'tw-events',
    status: 'ok',
    data: {
      quotes: [
        { kind: 'index', name: '加權', price: 17000, chg_pct: 1.23 },
        { kind: 'fx', name: 'USD/TWD', price: 31.2, chg_pct: -0.1 },
        { kind: 'yield', name: '美債10Y', price: 4.1, chg_abs: 0.02 },
      ],
    },
  };
  const ctx = {
    // task 5.2：mount() 現在會呼叫 common.applyAppearance／ctx.onSettings（透明度／主題色
    // 即時套用），與本測試要驗證的「暫停/恢復排程冪等」無關，給無 op stub 讓 mount() 能跑
    // 完即可，不用引入整份 common.js 原始碼。
    common: { esc: (s) => String(s), applyAppearance: () => {} },
    config: { width: 992 },
    snapshot,
    settings: null,
    onData: () => {},
    onSettings: () => {},
    onPause: (fn) => {
      ctx.__pauseHandler = fn;
    },
  };

  sandbox.__test.mount(container, ctx);

  const failures = [];

  const pendingAfterMount = scheduler.pendingCount();
  if (pendingAfterMount !== 1) {
    console.error(
      `[quotes-pause-idempotent] 前提不成立：掛載後應該剛好有 1 筆排定中的動畫，實際 ${pendingAfterMount} 筆——` +
        '本測試對「重複排程」沒有鑑別力，可能是 fixture 內容沒有觸發跑馬燈（ul.scrollWidth/clientWidth 邏輯需要檢查）。',
    );
    process.exit(2);
  }

  // 讓跑馬燈先跑一段，取得一個「正在移動中」的基準位置。
  scheduler.flush(16);
  const posRunning = ul.scrollLeft;

  // Codex 指出的情境：重複收到 {paused:false}（本來就沒暫停），不應該新增排程。
  ctx.__pauseHandler({ paused: false });
  ctx.__pauseHandler({ paused: false });
  const pendingAfterDuplicateResume = scheduler.pendingCount();
  if (pendingAfterDuplicateResume !== 1) {
    failures.push(
      `重複收到 {paused:false} 後排定中的筆數變成 ${pendingAfterDuplicateResume}（預期仍是 1）——` +
        '每次都無條件呼叫 scheduleStep()，建立了無法取消的第二條動畫鏈。',
    );
  }

  // 真的暫停：排定中的筆數必須歸零，不能只取消「最後一筆」、留下較舊的殘留鏈。
  ctx.__pauseHandler({ paused: true });
  const pendingAfterPause = scheduler.pendingCount();
  if (pendingAfterPause !== 0) {
    failures.push(
      `暫停後排定中的筆數是 ${pendingAfterPause}（預期 0）——stopAnimation() 只取消了最後一筆，` +
        '較舊的殘留鏈仍在排程佇列裡，之後還會繼續觸發 step() 讓畫面移動。',
    );
  }

  const posAtPause = ul.scrollLeft;
  // 暫停後即使佇列裡還有殘留的排程，也 flush 幾次看看位置會不會動——這是最終使用者會看到
  // 的症狀（「暫停時 SHALL 停止捲動」）。
  scheduler.flush(1016);
  scheduler.flush(2016);
  const posAfterPauseFlushes = ul.scrollLeft;
  if (posAfterPauseFlushes !== posAtPause) {
    failures.push(
      `暫停後位置仍在變化：暫停當下=${posAtPause}，之後兩次 flush 後=${posAfterPauseFlushes}——` +
        '殘留的動畫鏈仍在推進捲動位置。',
    );
  }

  if (failures.length > 0) {
    console.error('[quotes-pause-idempotent] FAIL');
    console.error(`  （參考：posRunning=${posRunning}）`);
    for (const f of failures) console.error('  - ' + f);
    process.exit(1);
  }
  console.log(
    '[quotes-pause-idempotent] PASS：重複恢復不會新增排程，暫停後排程歸零、位置不再變化。' +
      `（posRunning=${posRunning}，posAtPause=${posAtPause}）`,
  );
}

main();
