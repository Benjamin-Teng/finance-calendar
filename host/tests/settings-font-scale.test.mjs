// host/tests/settings-font-scale.test.mjs
//
// widget-adaptive-zoom-and-grid task 3.2（design.md D2／D5）：設定視窗外觀區的字級滑桿。
// - 滑桿規格：70–150%、間距 5%；旁註「受小工具框大小限制，時鐘與行情條預設已填滿框，只會縮小」。
// - 拖動滑桿 → `patch({ font_scale })`（以 0..1 倍率送出，例如 85% → 0.85）。
// - 收到 `settings` 事件（renderForm 回填）→ 滑桿與百分比文字同步；欄位缺漏或非數字退回 100%。
//
// 做法比照 settings-autostart-revert.test.mjs：從 settings.html 抽出函式原始碼，在 Node vm
// context 內以測試樁 `el`／`patch`／`document` 執行。
//
// 執行：node --test host/tests/settings-font-scale.test.mjs

import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const settingsHtmlPath = path.join(__dirname, '..', 'ui', 'settings.html');
// 統一成 LF：repo 設 core.autocrlf=true，新 checkout 是 CRLF 而 Edit／Write 寫出的檔是 LF；
// 下面以 '\n      }\n' 切函式結尾，不正規化會在 CRLF 檔上找不到。
const html = readFileSync(settingsHtmlPath, 'utf8').replace(/\r\n/g, '\n');

function extractFunction(name) {
  const start = html.indexOf(`function ${name}(`);
  if (start < 0) throw new Error(`settings.html 找不到 ${name}`);
  const end = html.indexOf('\n      }\n', start);
  if (end < 0) throw new Error(`settings.html 找不到 ${name} 的結尾`);
  return html.slice(start, end + '\n      }\n'.length);
}

/** 測試樁節點：記錄屬性、子節點與事件處理器，可讀寫 value／textContent。 */
function makeEl(tag, props, ...children) {
  const node = {
    tag,
    props: props ?? {},
    children: children.flat(),
    value: props?.value ?? '',
    textContent: '',
    listeners: {},
    addEventListener(type, fn) {
      (this.listeners[type] ??= []).push(fn);
    },
  };
  return node;
}

function findById(node, id) {
  if (!node || typeof node !== 'object') return null;
  if (node.props?.id === id) return node;
  for (const child of node.children ?? []) {
    const hit = findById(child, id);
    if (hit) return hit;
  }
  return null;
}

function allText(node) {
  if (typeof node === 'string') return node;
  if (!node || typeof node !== 'object') return '';
  return [node.textContent ?? '', ...(node.children ?? []).map(allText)].join('');
}

function buildAppearance() {
  const patches = [];
  const context = vm.createContext({
    el: makeEl,
    patch: async (obj, label) => {
      patches.push({ obj, label });
      return true;
    },
    // 測試不驗 debounce 時序，直接同步呼叫。
    debounce: (fn) => fn,
  });
  vm.runInContext(
    `${extractFunction('buildAppearanceSection')}\nglobalThis.__section = buildAppearanceSection();`,
    context,
  );
  return { section: context.__section, patches };
}

test('字級滑桿：70–150%、間距 5%，旁註照 design.md D2', () => {
  const { section } = buildAppearance();
  const range = findById(section, 'font-scale-range');
  assert.ok(range, '外觀區要有 id=font-scale-range 的滑桿');
  assert.equal(range.tag, 'input');
  assert.equal(range.props.type, 'range');
  assert.equal(range.props.min, '70');
  assert.equal(range.props.max, '150');
  assert.equal(range.props.step, '5');
  assert.ok(findById(section, 'font-scale-val'), '要有百分比文字 id=font-scale-val');
  assert.ok(
    allText(section).includes('受小工具框大小限制，時鐘與行情條預設已填滿框，只會縮小'),
    '旁註文字',
  );
});

test('拖動滑桿：送出 patch({ font_scale })，倍率為百分比 ÷ 100，並更新百分比文字', () => {
  const { section, patches } = buildAppearance();
  const range = findById(section, 'font-scale-range');
  const val = findById(section, 'font-scale-val');
  for (const [pct, expected] of [
    [85, 0.85],
    [150, 1.5],
    [70, 0.7],
    [100, 1],
  ]) {
    patches.length = 0;
    range.value = String(pct);
    for (const fn of range.listeners.input ?? []) fn();
    assert.equal(val.textContent, `${pct}%`);
    assert.equal(patches.length, 1, `拖到 ${pct}% 送出一次 patch`);
    // 物件來自 vm context（原型不同），不能直接 deepStrictEqual 整個物件。
    assert.deepEqual(Object.keys(patches[0].obj), ['font_scale']);
    assert.equal(patches[0].obj.font_scale, expected);
  }
});

test('renderFontScale：settings 事件回填滑桿與百分比文字；缺漏或非數字退回 100%', () => {
  const range = { value: '' };
  const val = { textContent: '' };
  const context = vm.createContext({
    document: {
      getElementById: (id) => ({ 'font-scale-range': range, 'font-scale-val': val })[id] ?? null,
    },
  });
  vm.runInContext(`${extractFunction('renderFontScale')}\nglobalThis.__fn = renderFontScale;`, context);
  const render = context.__fn;

  render({ font_scale: 1.2 });
  assert.equal(range.value, '120');
  assert.equal(val.textContent, '120%');

  render({ font_scale: 0.85 });
  assert.equal(range.value, '85');
  assert.equal(val.textContent, '85%');

  // 浮點誤差不外洩成 '114.99999999999999'。
  render({ font_scale: 1.15 });
  assert.equal(range.value, '115');

  for (const bad of [{}, { font_scale: null }, { font_scale: 'x' }, { font_scale: NaN }]) {
    render(bad);
    assert.equal(range.value, '100', JSON.stringify(bad));
    assert.equal(val.textContent, '100%');
  }
});

test('renderForm 回填時呼叫 renderFontScale，且使用者正在拖的滑桿不被蓋掉', () => {
  const body = extractFunction('renderForm');
  assert.match(body, /if \(!skip\('font-scale-range'\)\)\s*\{?\s*renderFontScale\(settings\)/);
});
