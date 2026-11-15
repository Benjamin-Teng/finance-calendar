// host/tests/compare/capture-utils.mjs
//
// `taipeiDateStr`／`buildOverrideScript`／`buildExtractScript`：「固定時區／固定時間／DOM 文字擷取」的
// 建構函式。零副作用模組，可以安全 import（`compare.mjs` 檔尾會直接執行 `run()` 並 `process.exit()`，
// 不能被 import）。

/** `YYYY-MM-DD`（Asia/Taipei，不受執行本腳本的機器本機時區影響）。 */
export function taipeiDateStr(epochMs) {
  // en-CA 的 DateTimeFormat 輸出剛好是 YYYY-MM-DD。
  const fmt = new Intl.DateTimeFormat('en-CA', {
    timeZone: 'Asia/Taipei',
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
  });
  return fmt.format(new Date(epochMs));
}

/** 注入頁面的「固定時間」腳本：零參數 `new Date()`／`Date.now()` 一律回傳固定時刻，
 * 帶參數呼叫（如 `new Date(window.__TEST_TODAY + 'T00:00:00')`）原樣放行——`host/ui/common.js` 的
 * `todayBase()` 靠帶參數呼叫＋`__TEST_TODAY` 字串自己算「今天」，不能把所有 Date 呼叫都夾死在同一個值，
 * 否則台股動態事件的日期計算會被破壞。 */
export function buildOverrideScript(fixedEpochMs, todayStr) {
  return `(() => {
  const FIXED_MS = ${fixedEpochMs};
  const RealDate = Date;
  class FixedDate extends RealDate {
    constructor(...args) {
      if (args.length === 0) { super(FIXED_MS); } else { super(...args); }
    }
    static now() { return FIXED_MS; }
  }
  window.Date = FixedDate;
  window.__TEST_TODAY = ${JSON.stringify(todayStr)};
})();`;
}

/** 擷取＋正規化：clone 選取到的面板節點、移除排除清單命中的子節點，再依 DOM 順序走訪所有
 * 文字節點，空白正規化＋trim，回傳非空字串陣列（一個文字節點一行，逐行比對用）。 */
export function buildExtractScript(selector, excludeSelectors) {
  return `(() => {
  const root = document.querySelector(${JSON.stringify(selector)});
  if (!root) return { ok:false, error:'selector 找不到：' + ${JSON.stringify(selector)} };
  const clone = root.cloneNode(true);
  (${JSON.stringify(excludeSelectors)}).forEach((sel) => {
    clone.querySelectorAll(sel).forEach((el) => el.remove());
  });
  const lines = [];
  (function walk(node) {
    if (node.nodeType === Node.TEXT_NODE) {
      const t = node.textContent.replace(/\\s+/g, ' ').trim();
      if (t) lines.push(t);
      return;
    }
    if (node.nodeType !== Node.ELEMENT_NODE) return;
    for (const child of node.childNodes) walk(child);
  })(clone);
  return { ok:true, lines };
})();`;
}
