// host/tests/settings-font-scale.test.mjs
//
// widget-font-scale-per-widget task 4.2（design.md D6）：設定視窗不再有全域字級滑桿——字級改在
// 「編輯版面」中於各小工具右上角調整（宿主也拒收 `font_scale` patch）。本測試鎖住：
// - 外觀區沒有字級滑桿（id=font-scale-range／font-scale-val），也不會送出 `font_scale` patch。
// - 外觀區有一行說明「字級在『編輯版面』中於各小工具右上角調整」。
// - 表單回填（renderForm）不再碰字級欄位。
//
// 做法比照 settings-autostart-revert.test.mjs：從 settings.html 抽出函式原始碼，在 Node vm
// context 內以測試樁 `el`／`patch` 執行。第一個參數可指定別的 settings.html（例如舊版，用來確認
// 測試有鑑別力）。
//
// 執行：node --test host/tests/settings-font-scale.test.mjs

import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const settingsHtmlPath =
  process.env.SETTINGS_HTML_OVERRIDE || path.join(__dirname, '..', 'ui', 'settings.html');
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

/** 測試樁節點：記錄屬性、子節點與事件處理器。 */
function makeEl(tag, props, ...children) {
  return {
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
}

function walk(node, fn) {
  if (!node || typeof node !== 'object') return;
  fn(node);
  for (const child of node.children ?? []) walk(child, fn);
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
    debounce: (fn) => fn,
  });
  vm.runInContext(
    `${extractFunction('buildAppearanceSection')}\nglobalThis.__section = buildAppearanceSection();`,
    context,
  );
  return { section: context.__section, patches };
}

test('外觀區沒有字級滑桿', () => {
  const { section } = buildAppearance();
  const ids = [];
  walk(section, (n) => {
    if (n.props?.id) ids.push(n.props.id);
  });
  assert.ok(!ids.includes('font-scale-range'), '不應再有 id=font-scale-range 的滑桿');
  assert.ok(!ids.includes('font-scale-val'), '不應再有 id=font-scale-val 的百分比文字');
});

test('外觀區所有控制都不會送出 font_scale patch', () => {
  const { section, patches } = buildAppearance();
  walk(section, (n) => {
    for (const fns of Object.values(n.listeners ?? {})) {
      for (const fn of fns) fn();
    }
    if (typeof n.props?.onchange === 'function') n.props.onchange();
  });
  for (const { obj } of patches) {
    assert.ok(!Object.keys(obj).includes('font_scale'), `不應送出 font_scale：${JSON.stringify(obj)}`);
  }
});

test('外觀區有說明：字級在「編輯版面」中於各小工具右上角調整', () => {
  const { section } = buildAppearance();
  assert.ok(allText(section).includes('字級在「編輯版面」中於各小工具右上角調整'));
});

test('表單回填不再碰字級欄位', () => {
  const body = extractFunction('renderForm');
  assert.doesNotMatch(body, /font-scale|renderFontScale|font_scale/);
  assert.ok(!html.includes('function renderFontScale('), 'renderFontScale 應已移除');
});
