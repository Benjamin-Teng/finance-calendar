// host/tests/settings-autostart-revert.test.mjs
//
// review autostart-installer L1（design.md D12）：`update_settings` 寫開機自啟登錄失敗時回傳錯誤
// （`widgets::AUTOSTART_WRITE_FAILED`）。settings.html 的「登入時自動啟動」開關必須比照小工具
// 開關，在 `patch()` 回報失敗時把勾選還原——否則剛點過、仍有焦點的 checkbox 會停在使用者點的
// 值（`renderForm` 以 skipFocused 重繪，不會蓋掉它），看起來已開啟、實際上沒有登錄。
//
// 做法：從 settings.html 抽出 `buildStartupSection` 的原始碼，在 Node vm context 內以測試樁
// `el`／`patch` 執行，模擬使用者勾選後送出失敗與成功兩種情況。
//
// 執行：node host/tests/settings-autostart-revert.test.mjs

import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const settingsHtmlPath = path.join(__dirname, '..', 'ui', 'settings.html');
// 統一成 LF：repo 設 core.autocrlf=true 且無 .gitattributes，新 checkout（例如另一個 worktree）是 CRLF，
// 而 Edit／Write 寫出的檔是 LF；下面以 '\n      }\n' 切函式結尾，不正規化會在 CRLF 檔上找不到而丟例外。
const html = readFileSync(settingsHtmlPath, 'utf8').replace(/\r\n/g, '\n');

const start = html.indexOf('function buildStartupSection()');
if (start < 0) throw new Error('settings.html 找不到 buildStartupSection');
const end = html.indexOf('\n      }\n', start);
if (end < 0) throw new Error('settings.html 找不到 buildStartupSection 的結尾');
const source = html.slice(start, end + '\n      }\n'.length);

function findById(node, id) {
  if (!node || typeof node !== 'object') return null;
  if (node.props?.id === id) return node;
  for (const child of node.children ?? []) {
    const hit = findById(child, id);
    if (hit) return hit;
  }
  return null;
}

async function run(patchResult) {
  const patches = [];
  const context = vm.createContext({
    el: (tag, props, ...children) => ({ tag, props, children, checked: false }),
    patch: async (obj, label) => {
      patches.push({ obj, label });
      return patchResult;
    },
  });
  vm.runInContext(`${source}\nglobalThis.__section = buildStartupSection();`, context);
  const input = findById(context.__section, 'autostart');
  if (!input) throw new Error('找不到 id=autostart 的 checkbox');
  input.checked = true; // 使用者勾選（瀏覽器先改 checked 再觸發 change）
  await input.props.onchange();
  return { input, patches };
}

let failures = 0;
function check(cond, msg) {
  if (cond) {
    console.log(`ok   ${msg}`);
  } else {
    failures += 1;
    console.error(`FAIL ${msg}`);
  }
}

{
  const { input, patches } = await run(false);
  check(patches.length === 1 && patches[0].obj.autostart === true, '送出 { autostart: true }');
  check(input.checked === false, '寫入失敗 → 開關還原成未勾選');
}
{
  const { input } = await run(true);
  check(input.checked === true, '寫入成功 → 開關維持勾選');
}

if (failures > 0) {
  console.error(`${failures} 項失敗`);
  process.exit(1);
}
console.log('settings-autostart-revert：全部通過');
