// host/tests/fixed-data-generation.test.mjs
//
// change fixed-events-holiday-shift 的 Codex adversarial review [medium]：`data_dir` 切換後
// widget.html 會推一筆新世代的 `status:'empty'`（舊世代與同世代多餘的 empty 已在 widget.html
// 濾掉），台股固定事件必須丟掉舊目錄的休市日、改回只依週末判斷（spec「台股固定事件遇休市順延」：
// 資料尚未取得時只以週末判斷）。
//
// 做法同 custom-null-snapshot.test.mjs：極簡假 `document`／元素樁，直接 `mount()`
// host/ui/widgets/fixed.js，`ctx.common` 用真的 host/ui/common.js；`Date` 固定在 2026-10-18（週日，
// 本週 10/18–10/24，台指期結算 10/21 週三）。
//
// 執行：node --test host/tests/fixed-data-generation.test.mjs

import test from 'node:test';
import assert from 'node:assert/strict';

const RealDate = Date;
const FIXED_MS = new RealDate(2026, 9, 18, 10, 0, 0).getTime();
class FixedDate extends RealDate {
  constructor(...args) {
    if (args.length === 0) super(FIXED_MS);
    else super(...args);
  }
  static now() {
    return FIXED_MS;
  }
}

function makeElement(tag) {
  return {
    tagName: tag,
    className: '',
    id: '',
    textContent: '',
    html: '',
    classList: { add() {} },
    append() {},
    set innerHTML(v) {
      this.html = v;
    },
    get innerHTML() {
      return this.html;
    },
    insertAdjacentHTML(_pos, s) {
      this.html += s;
    },
    querySelectorAll() {
      return [];
    },
  };
}

test('新世代 empty 快照清掉舊休市日，改回只依週末', async () => {
  const created = [];
  globalThis.Date = FixedDate;
  globalThis.document = {
    hidden: false,
    documentElement: { style: { setProperty() {} } },
    createElement(tag) {
      const el = makeElement(tag);
      created.push(el);
      return el;
    },
    addEventListener() {},
    removeEventListener() {},
  };
  let unmount = null;
  try {
    const common = await import('../ui/common.js');
    const { mount } = await import('../ui/widgets/fixed.js');
    let push = null;
    unmount = mount(makeElement('div'), {
      common,
      config: {},
      settings: {},
      snapshot: { status: 'ok', generation: 1, data: { holidays: ['2026-10-21'] } },
      onData: (fn) => {
        push = fn;
      },
      onSettings: () => {},
    });
    const list = created.find((el) => el.id === 'fixedList');
    assert.ok(list, '應建立 #fixedList');
    assert.match(list.html, /10\/22 \(四\)<\/span><b>台指期／選擇權結算<span class="badge shifted"[^>]*>原 10\/21/);

    push({ status: 'empty', generation: 2, data: null, meta: null });
    assert.match(list.html, /10\/21 \(三\)<\/span><b>台指期／選擇權結算<\/b>/);
    assert.doesNotMatch(list.html, /原 10\/21/);
  } finally {
    unmount?.(); // 清掉 60 秒計時器，否則斷言失敗時行程不會結束
    globalThis.Date = RealDate;
    delete globalThis.document;
  }
});
