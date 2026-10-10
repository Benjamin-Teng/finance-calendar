// host/tests/list-collapse.test.mjs
//
// widget-font-scale-per-widget task 3.1（design.md D5）：清單收合版面與最小框。
// - widget.html 把 `?w=<id>` 寫到 `<html data-widget>`，widget.css 才能依小工具各自的斷點收合。
// - widget.css 對有右側數值（`.n`）的清單——總經日曆、台股動態事件——各有一個
//   `@media (width < Npx)` 收合區塊：`.ev` 換行、標題可斷字、`.n` 獨占第二行、空 `.n` 不佔行。
//   台股固定事件與擴充插槽沒有右側數值，單行版面本身（標題在列內換行）就是最窄版面，不設斷點。
// - 斷點與最小框寬（host/src/widgets.rs `WIDGET_SPECS`）＝task-3.1-report.md 的實測結論；
//   不變式：min 寬 < 斷點（最小框下一定是收合版面）、斷點 < 設計寬（寬度足夠時維持單行，
//   且倍率由寬度決定時 CSS 寬剛好等於設計寬，不能落在斷點上）、總經日曆 min 寬 ≤ 266（spec
//   「窄框的清單可以放大」）。
// 量測腳本：host/tests/compare/measure-list-collapse.mjs（數值改動時重跑並同步更新本檔與報告）。
//
// 執行：node --test host/tests/list-collapse.test.mjs

import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { widgetZoomBoxes } from './compare/verify-visual-edges.mjs';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const UI = path.join(__dirname, '..', 'ui');
const read = (p) => readFileSync(p, 'utf8').replace(/\r\n/g, '\n');
const css = read(path.join(UI, 'widget.css'));
const html = read(path.join(UI, 'widget.html'));
const widgetsRs = read(path.join(__dirname, '..', 'src', 'widgets.rs'));

// task-3.1-report.md「結論」表（改這裡要同步改報告與 widgets.rs／widget.css）；台股固定事件的
// min 寬是 change fixed-events-holiday-shift task-3.2-report.md 的重量值（加了順延徽章）。
const COLLAPSIBLE = {
  macro: { minWidth: 229, breakpoint: 423 },
  dynamic: { minWidth: 333, breakpoint: 467 },
};
const FIXED_MIN_WIDTH = 285;
const CUSTOM_MIN_WIDTH = 239;

/** widget.css 中每個 `@media (width < Npx) { … }` 區塊：{ breakpoint, body }（以大括號配對切出）。 */
function mediaBlocks(text) {
  const out = [];
  const re = /@media \(width < ([\d.]+)px\)\s*\{/g;
  let m;
  while ((m = re.exec(text))) {
    let depth = 1;
    let i = re.lastIndex;
    for (; i < text.length && depth > 0; i++) {
      if (text[i] === '{') depth++;
      else if (text[i] === '}') depth--;
    }
    out.push({ breakpoint: Number(m[1]), body: text.slice(re.lastIndex, i - 1) });
  }
  return out;
}

const blocksFor = (id) => mediaBlocks(css).filter((b) => b.body.includes(`[data-widget='${id}']`));

/** widgets.rs `custom_spec` 的 `list_box(min寬, …)` 第一個引數。 */
function customMinWidth() {
  const m = widgetsRs.match(/const fn custom_spec[\s\S]*?zoom_box: list_box\(([\d.]+),/);
  assert.ok(m, 'widgets.rs 找不到 custom_spec 的 list_box');
  return Number(m[1]);
}

test('widget.html 把小工具 id 寫到 <html data-widget>', () => {
  assert.match(html, /document\.documentElement\.dataset\.widget = id;/);
});

for (const [id, expected] of Object.entries(COLLAPSIBLE)) {
  test(`${id}：恰有一個收合區塊，含兩行版面規則`, () => {
    const blocks = blocksFor(id);
    assert.equal(blocks.length, 1, `${id} 應恰有一個收合 media 區塊`);
    const sel = `:root[data-widget='${id}']`;
    const rule = (selector) => {
      const at = blocks[0].body.indexOf(`${selector} {`);
      assert.ok(at >= 0, `${id} 收合區塊缺少「${selector}」`);
      return blocks[0].body.slice(at, blocks[0].body.indexOf('}', at));
    };
    assert.match(rule(`${sel} .ev`), /flex-wrap: wrap;/);
    assert.match(rule(`${sel} .ev b`), /flex: 1 1 0;[\s\S]*min-width: 0;[\s\S]*overflow-wrap: anywhere;/);
    assert.match(rule(`${sel} .ev .n`), /flex: 1 0 100%;[\s\S]*margin-left: 0;/);
    assert.match(rule(`${sel} .ev .n:empty`), /display: none;/);
  });

  test(`${id}：斷點與 min 寬＝實測值，且 min 寬 < 斷點 < 設計寬`, () => {
    const box = widgetZoomBoxes()[id];
    const [{ breakpoint }] = blocksFor(id);
    assert.deepEqual({ minWidth: box.minWidth, breakpoint }, expected);
    assert.ok(box.minWidth < breakpoint, `${id}：min 寬 ${box.minWidth} 應小於斷點 ${breakpoint}`);
    assert.ok(breakpoint < box.comfortWidth, `${id}：斷點 ${breakpoint} 應小於設計寬 ${box.comfortWidth}`);
  });
}

test('總經日曆 min 寬 ≤ 266（0.64 倍設計寬、字級 200% 時倍率至少 1.2）', () => {
  assert.ok(widgetZoomBoxes().macro.minWidth <= 266);
});

test('台股固定事件：min 寬＝實測值，沒有收合斷點（無右側數值）', () => {
  assert.equal(widgetZoomBoxes().fixed.minWidth, FIXED_MIN_WIDTH);
  assert.equal(blocksFor('fixed').length, 0);
});

test('擴充插槽：min 寬＝實測值，沒有收合斷點（無清單列）', () => {
  assert.equal(customMinWidth(), CUSTOM_MIN_WIDTH);
  assert.equal(mediaBlocks(css).filter((b) => b.body.includes("[data-widget='custom")).length, 0);
});

test('清單 min 寬只放寬（不大於改動前的 0.75 × 設計寬）', () => {
  const boxes = widgetZoomBoxes();
  for (const id of ['macro', 'fixed', 'dynamic']) {
    assert.ok(boxes[id].minWidth <= 0.75 * boxes[id].comfortWidth, id);
  }
  assert.ok(customMinWidth() <= 352.5);
});
