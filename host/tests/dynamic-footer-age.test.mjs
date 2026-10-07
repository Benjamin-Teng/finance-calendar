// host/tests/dynamic-footer-age.test.mjs
//
// 重現並鎖住 Codex adversarial review（.superpowers/sdd/tasks/reviews/task-4.4-codex.md
// [medium]）指出的問題：dynamic.js 的週期計時器（60 秒）與 visibilitychange 只在
// `todayIso` 改變時才呼叫 `render()` 重算頁腳；「已 N 天未更新」門檻（`data.fetched` 超過
// 3 天）是用 `Date.now()` 算的時分秒粒度，同一天之內完全可能跨過 3 天門檻——這種情況下
// todayIso 沒變，頁腳會一直停在「未過期」，直到下一次 todayIso 真的改變（隔天）才會補上
// 警示，最多延遲近一天才出現。
//
// 做法：把 host/ui/common.js（`export ` 前綴拿掉）與 host/ui/widgets/dynamic.js
// （`export function mount` 拿掉 `export`）的原始碼串起來，在 Node vm context 內執行——
// 兩個檔案本身都不 import 任何東西（dynamic.js 透過 ctx.common 拿 common.js 的函式，不是
// 用 import），可以直接逐行照跑，不需要像 widget.html 那樣做文字替換。用可控制的假
// `Date`（覆寫 `now()`，其餘建構行為原樣委派給真正的 Date）與可以手動觸發的假
// `setInterval` 回呼，模擬「今天的日期沒變、但牆上時鐘往前走跨過 3 天門檻」。
//
// 執行：node host/tests/dynamic-footer-age.test.mjs
// 通過條件：todayIso 不變、`fetched` 已超過 3 天時，下一次計時器 tick（或 visibilitychange）
// 必須讓頁腳出現「已 N 天未更新」警示；缺這個行為就 FAIL、exit 1。

import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const commonJsPath = path.join(__dirname, '..', 'ui', 'common.js');
const dynamicJsPath = path.join(__dirname, '..', 'ui', 'widgets', 'dynamic.js');

function stripExports(src) {
  return src.replace(/^export (const|function) /gm, '$1 ');
}

function makeElementStub(tag) {
  const el = {
    tagName: tag,
    className: '',
    id: '',
    textContent: '',
    style: {},
    _innerHTML: '',
    _children: [],
    classList: { add() {}, remove() {}, toggle() {} },
    appendChild(child) {
      this._children.push(child);
    },
    append(...children) {
      this._children.push(...children);
    },
    insertAdjacentHTML(_position, html) {
      this._innerHTML += html;
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

/** 可控制「現在時間」的假 Date：`now()` 回傳測試設定的固定值，其餘建構行為原樣委派給真正
 * 的 Date（`new Date('YYYY-MM-DDTHH:mm:ss')` 這種帶參數呼叫不受影響，只有無參數 `new Date()`
 * 與 `Date.now()` 被接管）。 */
function makeFakeDateClass(getNow) {
  return class FakeDate extends Date {
    constructor(...args) {
      if (args.length === 0) {
        super(getNow());
      } else {
        super(...args);
      }
    }
    static now() {
      return getNow();
    }
  };
}

async function main() {
  const commonSrc = stripExports(readFileSync(commonJsPath, 'utf8'));
  const dynamicSrc = readFileSync(dynamicJsPath, 'utf8').replace(
    'export function mount(container, ctx) {',
    'function mount(container, ctx) {',
  );

  // task 5.2：applyAppearance 併入清單，讓 mount() 開頭的 common.applyAppearance(ctx.settings)
  // 呼叫得到真正的（vm 內定義的）函式，而不是 undefined——它會讀 `document.documentElement
  // .style`，下面的 documentStub 補上這個節點供它呼叫，不驗證寫入結果（本測試關心的是頁腳
  // 過期警示，不是外觀套用，外觀套用已有 host/tests/apply-appearance.test.mjs 專門驗證）。
  const combinedSrc = `${commonSrc}\n${dynamicSrc}\nglobalThis.__test.mount = mount;\nglobalThis.__test.common = { DAY, WD, strip, addDays, md, mdw, weekStartOf, esc, pad2, tzOffsetLabel, nthWeekday, isoDate, todayBase, isTradingDay, targetDay, applyAppearance };`;

  // 固定「今天」＝2026-09-28（週一，非假日、非週末，todayBase()/targetDay() 都會落在當天，
  // isToday=true，不影響本測試關心的頁腳邏輯）；fetched＝2026-09-25 02:00，滿 3 天的門檻是
  // 2026-09-28 02:00——起始「現在」設在門檻之前（00:01），之後把「現在」推到門檻之後
  // （04:01），todayIso 全程都是 2026-09-28，不會觸發「跨日整份重建」那條路徑。
  const today = '2026-09-28';
  const beforeThreshold = new Date('2026-09-28T00:01:00').getTime();
  const afterThreshold = new Date('2026-09-28T04:01:00').getTime();
  let fakeNow = beforeThreshold;

  let intervalCb = null;
  const visibilityHandlers = [];
  const documentStub = {
    hidden: false,
    createElement: (tag) => makeElementStub(tag),
    addEventListener: (name, fn) => {
      if (name === 'visibilitychange') visibilityHandlers.push(fn);
    },
    removeEventListener: () => {},
    // task 5.2：applyAppearance 的預設參數讀 document.documentElement.style；本測試不驗證
    // 外觀套用本身（見 apply-appearance.test.mjs），只要 setProperty 存在、不丟例外即可。
    documentElement: { style: { setProperty: () => {} } },
  };
  const windowStub = { __TEST_TODAY: today };

  const sandbox = {
    document: documentStub,
    window: windowStub,
    Date: makeFakeDateClass(() => fakeNow),
    setInterval: (cb) => {
      intervalCb = cb;
      return 1;
    },
    clearInterval: () => {},
    console,
    __test: {},
  };
  vm.createContext(sandbox);
  vm.runInContext(combinedSrc, sandbox, { filename: 'common.js + dynamic.js (extracted)' });

  const container = makeElementStub('div');
  const snapshot = {
    channel: 'tw-events',
    status: 'ok',
    data: {
      events: [],
      punish: [],
      holidays: [],
      fetched: '2026-09-25 02:00',
    },
  };
  const ctx = {
    common: sandbox.__test.common,
    config: { maxHeight: 640 },
    snapshot,
    settings: { show_dividend: false, data_dir: '/data' },
    onData: () => {},
    onSettings: () => {},
  };

  sandbox.__test.mount(container, ctx);

  // container 的子元素依 mount() 內 `container.append(header, list, foot)` 的順序建立；
  // foot 是最後一個。
  const foot = container._children[container._children.length - 1];

  const footBeforeTick = foot.innerHTML;
  if (/已 \d+ 天未更新/.test(footBeforeTick)) {
    console.error(
      '[dynamic-footer-age] 前提不成立：掛載當下（現在時間未過 3 天門檻）頁腳已經有過期警示，' +
        `本測試對「跨門檻才出現警示」沒有鑑別力：${footBeforeTick}`,
    );
    process.exit(2);
  }

  // 牆上時鐘往前走，跨過 3 天門檻，但「今天」的日期（todayIso）不變——這正是 Codex 指出的
  // 情境：同一天之內跨過門檻，計時器 tick 若只看 todayIso 有沒有變，會誤判「不需要重算」。
  fakeNow = afterThreshold;
  if (typeof intervalCb !== 'function') {
    console.error('[dynamic-footer-age] mount() 沒有呼叫 setInterval() 註冊週期計時器，測試前提不成立。');
    process.exit(2);
  }
  intervalCb();

  const footAfterTick = foot.innerHTML;

  const failures = [];
  if (!/已 3 天未更新/.test(footAfterTick)) {
    failures.push(
      '計時器 tick（today 沒變、現在時間已跨過 3 天門檻）後頁腳仍沒有過期警示——' +
        `tick 前=${JSON.stringify(footBeforeTick)}，tick 後=${JSON.stringify(footAfterTick)}。`,
    );
  }

  // visibilitychange（頁面從背景切回前景）也要有同樣的效果——用另一次 mount 單獨驗證，避免
  // 跟上面的計時器狀態互相污染。
  fakeNow = beforeThreshold;
  const container2 = makeElementStub('div');
  const ctx2 = { ...ctx, snapshot: { ...snapshot, data: { ...snapshot.data } } };
  sandbox.__test.mount(container2, ctx2);
  const foot2 = container2._children[container2._children.length - 1];
  const foot2Before = foot2.innerHTML;
  fakeNow = afterThreshold;
  documentStub.hidden = false;
  for (const fn of visibilityHandlers.slice(-1)) fn(); // 只觸發這次 mount 新註冊的 handler
  const foot2After = foot2.innerHTML;
  if (!/已 3 天未更新/.test(foot2After)) {
    failures.push(
      'visibilitychange（today 沒變、現在時間已跨過 3 天門檻）後頁腳仍沒有過期警示——' +
        `觸發前=${JSON.stringify(foot2Before)}，觸發後=${JSON.stringify(foot2After)}。`,
    );
  }

  if (failures.length > 0) {
    console.error('[dynamic-footer-age] FAIL');
    for (const f of failures) console.error('  - ' + f);
    process.exit(1);
  }
  console.log(
    '[dynamic-footer-age] PASS：同日跨過 3 天未更新門檻時，計時器與 visibilitychange 都會補上頁腳警示。',
  );
}

main();
