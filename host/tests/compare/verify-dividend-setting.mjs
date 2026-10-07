#!/usr/bin/env node
// host/tests/compare/verify-dividend-setting.mjs
//
// 驗證「顯示除權息」設定（specs/finance-widgets「顯示除權息」Scenario；task-4.4-brief.md）：
// 使用者在設定中勾選後，台股動態事件小工具的預告清單應加入除權息場次，且與 Lively 版把
// `CONFIG.dynTypes` 加入 `'dividend'` 後的行為一致。
//
// compare.mjs 的標準跨版本比對（`--widget dynamic`）只驗證「兩邊預設都不顯示除權息」時一致
// （task-4.4-brief.md：「對照測試時新版設定為不顯示即應一致」，這是主要對照路徑，沿用
// `host/ui/fixtures/settings.json` 的 `show_dividend:false` 預設值）；不支援臨時切換新版
// fixture 的 `settings.json` 內容，也沒有辦法在 Lively 版注入 `CONFIG.dynTypes` 的修改
// （`CONFIG` 是 `finance-calendar.html` 自己那個 `<script>` 內宣告的變數，`compare.mjs`
// 的標準流程不會去動它）。本腳本專門驗證「打開之後」的行為，不重複造一套 CDP／伺服器骨架，
// 直接沿用 `capture-utils.mjs`（`buildOverrideScript`／`buildExtractScript`／
// `taipeiDateStr`，task 4.4 從 `compare.mjs` 抽出的共用邏輯，理由見該檔頭註解——不能直接
// `import` `compare.mjs` 本體，會觸發它自己的 `run()` 並提早 `process.exit()`）、
// `cdp.mjs`、`serve.mjs`、`panels.mjs` 既有的匯出函式。
//
// 做法：
//   1. 用 `startServer()` 起一份標準暫存伺服器（`/fixtures/settings.json` 預設
//      `show_dividend:false`），拿到 `tmpRoot` 後直接覆寫該檔為 `show_dividend:true`
//      （伺服器每次請求都重新 `readFile`，覆寫檔案不需要重啟伺服器）。
//   2. 新版：`Page.navigate` 到 `/new/widget.html?w=dynamic&fixtures=/fixtures/`
//      （此時讀到的就是 `show_dividend:true` 的設定）。
//   3. Lively 版：`Page.navigate` 到 `/lively/finance-calendar.html`，等
//      `window.TW_EVENTS` 就緒後，用 `Runtime.evaluate` 把 `CONFIG.dynTypes` 直接設成與
//      本檔 `dynamic.js` 的 `dynTypesFor({show_dividend:true})` **相同的陣列**（而不是
//      `push('dividend')` 到陣列尾端）——`dynamic.js` 檔頭已記錄「除權息排序是本 task 的
//      判斷，Lively 版原始碼未規定順序」，本腳本要驗證的是「給定同一份 dynTypes 排序，
//      兩邊的顯示邏輯結果一致」，不是「兩種不同排序恰好結果相同」；真實 fixture 有多筆
//      除權息與財報／股東會／法說會同一天（見下方 `--today` 選擇），排序不同會讓同一天
//      內的項目次序不同、逐行比對抓得到，因此排序必須對齊才是有意義的比較。
//      再重新呼叫 `renderDyn(window.TW_EVENTS)`（`finance-calendar.html` 的
//      `renderDyn` 是最上層 `<script>`（非 module）宣告的函式，等同掛在 `window` 下）。
//   4. 用 `panels.mjs` 的 `dynamic` 選取器／排除清單擷取兩邊文字，逐行比對。
//
// 用法：node host/tests/compare/verify-dividend-setting.mjs [--today <ISO8601>] [--fixture <path>]

import path from 'node:path';
import { readFile, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { startServer } from './serve.mjs';
import { launchEdge, newTarget, closeTarget, CDPTarget, waitForPageCondition, evaluate } from './cdp.mjs';
import { getPanel } from './panels.mjs';
import { buildOverrideScript, buildExtractScript, taipeiDateStr } from './capture-utils.mjs';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(__dirname, '..', '..', '..');
const DEFAULT_FIXTURE = path.join(REPO_ROOT, 'host', 'ui', 'fixtures', 'tw-events.json');
// 2026-09-28 週一下午：窗口內涵蓋多筆「除權息 vs 財報／股東會／法說會同一天」的真實 fixture
// 資料（見 task-4.4-report.md 的資料核對，例如 2026-10-01／10-05／10-08／10-12），足以
// 驗證除權息加入後的排序 tie-break 邏輯，不只是「多一種類型的行」這種淺層差異。
const DEFAULT_TODAY = '2026-09-28T14:05:00+08:00';
// 與 `dynamic.js` 的 `dynTypesFor({show_dividend:true})` 逐字相同（見該檔案檔頭說明）。
const DYN_TYPES_WITH_DIVIDEND = ['earnings', 'dividend', 'conference', 'meeting', 'punish'];

function parseArgs(argv) {
  const opts = { today: DEFAULT_TODAY, fixture: DEFAULT_FIXTURE };
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === '--today') opts.today = argv[++i];
    else if (argv[i] === '--fixture') opts.fixture = path.resolve(argv[++i]);
    else throw new Error(`未知參數：${argv[i]}`);
  }
  return opts;
}

function diffLines(a, b) {
  const max = Math.max(a.length, b.length);
  const diffs = [];
  for (let i = 0; i < max; i++) {
    if (a[i] !== b[i]) diffs.push({ index: i, a: a[i] ?? '(無此行)', b: b[i] ?? '(無此行)' });
  }
  return { pass: diffs.length === 0, diffs };
}

async function captureLivelyWithDividend(port, baseUrl, todayMs, todayStr) {
  const panel = getPanel('dynamic');
  const target = await newTarget(port, 'about:blank');
  const session = new CDPTarget(target.webSocketDebuggerUrl);
  try {
    await session.connect();
    await session.send('Page.enable');
    await session.send('Emulation.setTimezoneOverride', { timezoneId: 'Asia/Taipei' });
    await session.send('Page.addScriptToEvaluateOnNewDocument', {
      source: buildOverrideScript(todayMs, todayStr),
    });
    await session.send('Page.navigate', { url: `${baseUrl}/lively/finance-calendar.html` });
    await waitForPageCondition(session, "typeof window.TW_EVENTS !== 'undefined'");
    await new Promise((r) => setTimeout(r, 200));

    const mutateResult = await evaluate(
      session,
      `(() => {
        if (typeof renderDyn !== 'function') return { ok:false, error:'renderDyn 不是全域函式' };
        CONFIG.dynTypes = ${JSON.stringify(DYN_TYPES_WITH_DIVIDEND)};
        renderDyn(window.TW_EVENTS);
        return { ok:true };
      })()`,
    );
    if (!mutateResult.ok) throw new Error(`注入 CONFIG.dynTypes 失敗：${mutateResult.error}`);
    await new Promise((r) => setTimeout(r, 200));

    const result = await evaluate(session, buildExtractScript(panel.livelySelector, panel.exclude));
    if (!result.ok) throw new Error(result.error);
    return result.lines;
  } finally {
    session.close();
    await closeTarget(port, target.id);
  }
}

async function captureNewWithDividend(port, baseUrl, todayMs, todayStr) {
  const panel = getPanel('dynamic');
  const target = await newTarget(port, 'about:blank');
  const session = new CDPTarget(target.webSocketDebuggerUrl);
  try {
    await session.connect();
    await session.send('Page.enable');
    await session.send('Emulation.setTimezoneOverride', { timezoneId: 'Asia/Taipei' });
    await session.send('Page.addScriptToEvaluateOnNewDocument', {
      source: buildOverrideScript(todayMs, todayStr),
    });
    await session.send('Page.navigate', {
      url: `${baseUrl}/new/widget.html?w=dynamic&fixtures=/fixtures/`,
    });
    await waitForPageCondition(
      session,
      "!!document.getElementById('widget-root') && document.getElementById('widget-root').children.length > 0",
    );
    await new Promise((r) => setTimeout(r, 200));

    const result = await evaluate(session, buildExtractScript(panel.newSelector, panel.exclude));
    if (!result.ok) throw new Error(result.error);
    return result.lines;
  } finally {
    session.close();
    await closeTarget(port, target.id);
  }
}

async function run() {
  const opts = parseArgs(process.argv.slice(2));
  const todayMs = Date.parse(opts.today);
  if (Number.isNaN(todayMs)) throw new Error(`--today 不是合法的日期時間字串：${opts.today}`);
  const todayStr = taipeiDateStr(todayMs);

  console.log(`[verify-dividend] --today=${opts.today}（Asia/Taipei=${todayStr}）`);
  console.log(`[verify-dividend] --fixture=${opts.fixture}`);

  const server = await startServer(opts.fixture);
  console.log(`[verify-dividend] 本機伺服器：${server.url}（暫存目錄：${server.tmpRootDisplay}）`);

  // 覆寫新版 fixture 的 settings.json：只改 show_dividend，其餘欄位沿用
  // host/ui/fixtures/settings.json（startServer 已複製到 tmpRoot/fixtures/）。
  const settingsPath = path.join(server.tmpRoot, 'fixtures', 'settings.json');
  const settings = JSON.parse(await readFile(settingsPath, 'utf8'));
  if (settings.show_dividend !== false) {
    throw new Error(
      `host/ui/fixtures/settings.json 的 show_dividend 預期為 false（原始檔預設值），` +
        `實際讀到 ${settings.show_dividend}——請確認覆寫邏輯還對得上該檔案`,
    );
  }
  settings.show_dividend = true;
  await writeFile(settingsPath, JSON.stringify(settings, null, 2));
  console.log('[verify-dividend] 已覆寫新版 fixture settings.json：show_dividend=true');

  const edge = await launchEdge();
  console.log(`[verify-dividend] headless Edge 已啟動，devtools port=${edge.port}`);

  let pass;
  try {
    const [livelyLines, newLines] = await Promise.all([
      captureLivelyWithDividend(edge.port, server.url, todayMs, todayStr),
      captureNewWithDividend(edge.port, server.url, todayMs, todayStr),
    ]);
    const diff = diffLines(livelyLines, newLines);
    pass = diff.pass;

    console.log(`\n── 顯示除權息（show_dividend=true）widget=dynamic ──`);
    console.log(`  lively（CONFIG.dynTypes 含 dividend）：${livelyLines.length} 行／new:dynamic：${newLines.length} 行`);
    // 額外斷言：確認測試真的有測到東西，不是兩邊剛好都空——至少要出現「除權息」chip 文字，
    // 否則代表 fixture／日期挑選讓上節窗口內剛好沒有除權息事件，測試等於沒測到功能。
    const hasDividendChip = newLines.some((l) => l.includes('除權息'));
    if (!hasDividendChip) {
      pass = false;
      console.log('  FAIL：新版輸出裡完全沒有「除權息」字樣——測試沒有實際涵蓋到這個功能');
    }
    if (diff.pass) {
      console.log('  PASS（逐行完全相同）');
    } else {
      console.log(`  FAIL（${diff.diffs.length} 處不同）`);
      for (const d of diff.diffs.slice(0, 30)) {
        console.log(`    [${d.index}] - ${d.a}`);
        console.log(`    [${d.index}] + ${d.b}`);
      }
    }
  } finally {
    await edge.close();
    await server.close({});
  }

  console.log(pass ? '\n[verify-dividend] 總結：PASS' : '\n[verify-dividend] 總結：FAIL');
  return pass ? 0 : 1;
}

run()
  .then((code) => process.exit(code))
  .catch((err) => {
    console.error(`[verify-dividend] 發生錯誤：${err.stack || err.message}`);
    process.exit(2);
  });
