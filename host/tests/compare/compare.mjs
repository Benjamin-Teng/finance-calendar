#!/usr/bin/env node
// host/tests/compare/compare.mjs
//
// 小工具畫面文字的基準檔比對：在固定時區、固定時間、固定資料下開 `widget.html?w=<id>`，把畫面上的
// 文字逐行擷取出來，與 `golden/<情境>.txt` 逐行比較。情境清單在 scenarios.mjs。
//
// 基準檔的來歷：2026-11-16 Lively 版退場（change `retire-lively`）前，macro／dynamic／quotes 的每個
// 情境都先以當時的跨版本比對確認與 Lively 版逐行相同，再存成基準檔；例外是 `dynamic-stale-3d` 的頁尾
// 警示文案（v0.3.6 刻意改寫）。clock 與 fixed 在更早的 change 就刻意與 Lively 版不同，基準檔是當時的
// 新版輸出。之後基準檔就是「目前認可的顯示結果」：刻意改顯示時用 `--update` 重產，並在 diff 裡讀過。
//
// 零安裝：全部用 Node 22 內建能力（全域 `fetch`／`WebSocket` 驅動 CDP，`node:http` 起靜態伺服器）。
// 用法見同目錄 README.md。
//
// 本檔檔尾會直接執行 `run()` 並 `process.exit()`，不能被其他腳本 import；共用邏輯在
// capture-utils.mjs、evidence.mjs、scenarios.mjs。

import path from 'node:path';
import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { startServer } from './serve.mjs';
import { launchEdge, newTarget, closeTarget, CDPTarget, waitForPageCondition, evaluate } from './cdp.mjs';
import {
  SCENARIOS,
  WIDGET_IDS,
  DEFAULT_TODAY,
  ROOT_SELECTOR,
  EXCLUDE_SELECTORS,
  getScenario,
} from './scenarios.mjs';
import { taipeiDateStr, buildOverrideScript, buildExtractScript } from './capture-utils.mjs';
import { evidenceName, writeEvidence } from './evidence.mjs';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(__dirname, '..', '..', '..');
const EVIDENCE_DIR = path.join(REPO_ROOT, 'host', 'tools', 'evidence');
const GOLDEN_DIR = path.join(__dirname, 'golden');
const DEFAULT_FIXTURE = path.join(REPO_ROOT, 'host', 'ui', 'fixtures', 'tw-events.json');
const DEFAULT_INJECT = '__INJECTED_DIFF__';

function printHelp() {
  console.log(`用法：node host/tests/compare/compare.mjs [選項]

預設跑 scenarios.mjs 的全部情境，逐一與 golden/<情境>.txt 逐行比較。

  --widget <id>        只跑某個小工具的情境（${WIDGET_IDS.join('｜')}）
  --scenario <name>    只跑一個情境（名稱見 --list）
  --list               列出所有情境後結束
  --update             不比較，把這次擷取到的內容寫成基準檔（刻意改顯示時用；改完要讀過 diff）
  --inject-diff [文字] 擷取前在畫面上多塞一行文字，證明比對抓得到差異（預期 FAIL、結束碼 1）
  --keep-temp          不清除暫存伺服器目錄（除錯用）
  --evidence [名稱]    另把本次輸出寫成 host/tools/evidence/compare-<名稱>.log（去識別）
  -h, --help           顯示本說明

結束碼：0＝全部相同（或 --update 完成）；1＝有差異或缺基準檔；2＝執行錯誤。

範例：
  node host/tests/compare/compare.mjs
  node host/tests/compare/compare.mjs --widget macro
  node host/tests/compare/compare.mjs --scenario dynamic-dividend --update
  node host/tests/compare/compare.mjs --scenario clock --inject-diff
`);
}

function parseArgs(argv) {
  const opts = {
    widget: null,
    scenario: null,
    list: false,
    update: false,
    injectDiff: null,
    keepTemp: false,
    evidence: null,
    help: false,
  };
  const optionalValue = (i) => {
    const next = argv[i + 1];
    return next && !next.startsWith('-') ? next : null;
  };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    switch (a) {
      case '--widget':
      case '--scenario': {
        const v = optionalValue(i);
        if (!v) throw new Error(`${a} 需要一個值（--help 看用法）`);
        opts[a.slice(2)] = v;
        i++;
        break;
      }
      case '--list':
        opts.list = true;
        break;
      case '--update':
        opts.update = true;
        break;
      case '--inject-diff': {
        const v = optionalValue(i);
        if (v) i++;
        opts.injectDiff = v || DEFAULT_INJECT;
        break;
      }
      case '--keep-temp':
        opts.keepTemp = true;
        break;
      case '--evidence': {
        const v = optionalValue(i);
        if (v) i++;
        opts.evidence = v || true;
        break;
      }
      case '-h':
      case '--help':
        opts.help = true;
        break;
      default:
        throw new Error(`未知參數：${a}（--help 看用法）`);
    }
  }
  if (opts.update && opts.injectDiff) {
    throw new Error('--update 與 --inject-diff 不能同時使用（會把注入的文字寫進基準檔）');
  }
  return opts;
}

function selectScenarios(opts) {
  if (opts.scenario) return [getScenario(opts.scenario)];
  if (opts.widget) {
    if (!WIDGET_IDS.includes(opts.widget)) {
      throw new Error(`未知的小工具 id：「${opts.widget}」（可用：${WIDGET_IDS.join('、')}）`);
    }
    return SCENARIOS.filter((s) => s.widget === opts.widget);
  }
  return SCENARIOS;
}

function parseTime(label, iso) {
  const ms = Date.parse(iso);
  if (Number.isNaN(ms)) throw new Error(`${label} 不是合法的日期時間字串：${iso}`);
  return { ms, str: taipeiDateStr(ms) };
}

/** 擷取一個情境的畫面文字（回傳字串陣列，一個文字節點一行）。 */
async function captureScenario(port, baseUrl, scenario, injectDiff) {
  const from = parseTime('today', scenario.today || DEFAULT_TODAY);
  const to = scenario.advanceTo ? parseTime('advanceTo', scenario.advanceTo) : null;
  const url = `${baseUrl}/new/widget.html?w=${encodeURIComponent(scenario.widget)}&fixtures=/fixtures/`;

  const target = await newTarget(port, 'about:blank');
  const session = new CDPTarget(target.webSocketDebuggerUrl);
  try {
    await session.connect();
    await session.send('Page.enable');
    await session.send('Emulation.setTimezoneOverride', { timezoneId: 'Asia/Taipei' });
    await session.send('Page.addScriptToEvaluateOnNewDocument', {
      source: buildOverrideScript(from.ms, from.str),
    });
    await session.send('Page.navigate', { url });
    await waitForPageCondition(
      session,
      "!!document.getElementById('widget-root') && document.getElementById('widget-root').children.length > 0",
    );
    // 資料就緒後到模組實際寫入 DOM 之間留一點緩衝（本機實測綽綽有餘，見 README）。
    await new Promise((r) => setTimeout(r, 200));

    if (to) {
      // 不重新導覽：直接把 window.Date 換成新的固定時間，再送 visibilitychange（模組的 handler 只檢查
      // 「現在不是 hidden」，dispatch 這個事件本身就會觸發重算）。
      await evaluate(
        session,
        `(() => {
          const FIXED_MS = ${to.ms};
          const RealDate = window.Date;
          class FixedDate extends RealDate {
            constructor(...args) { if (args.length === 0) { super(FIXED_MS); } else { super(...args); } }
            static now() { return FIXED_MS; }
          }
          window.Date = FixedDate;
          window.__TEST_TODAY = ${JSON.stringify(to.str)};
          document.dispatchEvent(new Event('visibilitychange'));
          return { ok:true };
        })()`,
      );
      await new Promise((r) => setTimeout(r, 200));
    }

    if (injectDiff) {
      await evaluate(
        session,
        `(() => { const el = document.createElement('div');
          el.textContent = ${JSON.stringify(injectDiff)};
          document.querySelector(${JSON.stringify(ROOT_SELECTOR)}).prepend(el);
          return { ok:true }; })()`,
      );
    }

    const result = await evaluate(session, buildExtractScript(ROOT_SELECTOR, EXCLUDE_SELECTORS));
    if (!result.ok) throw new Error(result.error);
    return result.lines;
  } finally {
    session.close();
    await closeTarget(port, target.id);
  }
}

function goldenPath(name) {
  return path.join(GOLDEN_DIR, `${name}.txt`);
}

async function readGolden(name) {
  const file = goldenPath(name);
  if (!existsSync(file)) return null;
  const text = await readFile(file, 'utf8');
  return text.split(/\r?\n/).filter((l) => l !== '');
}

async function writeGolden(name, lines) {
  await mkdir(GOLDEN_DIR, { recursive: true });
  await writeFile(goldenPath(name), lines.length ? `${lines.join('\n')}\n` : '', 'utf8');
}

function diffLines(a, b) {
  const max = Math.max(a.length, b.length);
  const diffs = [];
  for (let i = 0; i < max; i++) {
    if (a[i] !== b[i]) diffs.push({ index: i, a: a[i] ?? '(無此行)', b: b[i] ?? '(無此行)' });
  }
  return diffs;
}

/** 跑一個情境；回傳是否通過。 */
async function runScenario(edge, scenario, opts) {
  const fixture = scenario.fixture ? path.join(__dirname, scenario.fixture) : DEFAULT_FIXTURE;
  const server = await startServer(fixture);
  let lines;
  try {
    if (scenario.settings) {
      const settingsPath = path.join(server.tmpRoot, 'fixtures', 'settings.json');
      const settings = JSON.parse(await readFile(settingsPath, 'utf8'));
      await writeFile(settingsPath, JSON.stringify({ ...settings, ...scenario.settings }, null, 1));
    }
    lines = await captureScenario(edge.port, server.url, scenario, opts.injectDiff);
  } finally {
    await server.close({ keepFiles: opts.keepTemp });
    if (opts.keepTemp) console.log(`[compare] --keep-temp：已保留暫存目錄 ${server.tmpRoot}（勿貼進證據）`);
  }

  console.log(`\n── ${scenario.name}（${scenario.widget}）：${scenario.note} ──`);
  if (opts.update) {
    await writeGolden(scenario.name, lines);
    console.log(`  已寫入基準檔 golden/${scenario.name}.txt（${lines.length} 行）`);
    return true;
  }
  const golden = await readGolden(scenario.name);
  if (golden === null) {
    console.log(`  FAIL（沒有基準檔 golden/${scenario.name}.txt；確認內容正確後用 --update 產生）`);
    return false;
  }
  const diffs = diffLines(golden, lines);
  console.log(`  基準檔：${golden.length} 行／這次：${lines.length} 行`);
  if (diffs.length === 0) {
    console.log('  PASS（逐行完全相同）');
    return true;
  }
  console.log(`  FAIL（${diffs.length} 處不同；- 基準檔、+ 這次）`);
  for (const d of diffs.slice(0, 30)) {
    console.log(`    [${d.index}] - ${d.a}`);
    console.log(`    [${d.index}] + ${d.b}`);
  }
  if (diffs.length > 30) console.log(`    ...（其餘 ${diffs.length - 30} 處省略）`);
  return false;
}

async function runCompare(opts) {
  const scenarios = selectScenarios(opts);
  const edge = await launchEdge();
  console.log(`[compare] headless Edge 已啟動，devtools port=${edge.port}；情境 ${scenarios.length} 個`);
  let failed = 0;
  try {
    for (const scenario of scenarios) {
      if (!(await runScenario(edge, scenario, opts))) failed++;
    }
  } finally {
    await edge.close();
  }
  if (opts.update) {
    console.log(`\n[compare] 總結：UPDATED（${scenarios.length} 個基準檔）`);
    return 0;
  }
  console.log(failed === 0 ? '\n[compare] 總結：PASS' : `\n[compare] 總結：FAIL（${failed}／${scenarios.length} 個情境）`);
  return failed === 0 ? 0 : 1;
}

// `--evidence [名稱]`：把本次所有 console.log 輸出同步收集起來，結束（含例外）時去識別寫成
// host/tools/evidence/compare-<名稱>.log（見 evidence.mjs）。
async function run() {
  const opts = parseArgs(process.argv.slice(2));
  if (opts.help) {
    printHelp();
    return 0;
  }
  if (opts.list) {
    for (const s of SCENARIOS) console.log(`${s.name}\t${s.widget}\t${s.note}`);
    return 0;
  }
  if (!opts.evidence) return runCompare(opts);

  const captured = [];
  const origLog = console.log;
  console.log = (...args) => {
    captured.push(args.map((a) => (typeof a === 'string' ? a : String(a))).join(' '));
    origLog(...args);
  };
  let code = 2;
  try {
    code = await runCompare(opts);
    return code;
  } catch (err) {
    captured.push(`[compare] 發生錯誤：${err.message}`);
    throw err;
  } finally {
    console.log = origLog;
    const name = evidenceName(opts);
    captured.unshift(`# compare.mjs ${new Date().toISOString()} 參數：${process.argv.slice(2).join(' ')}`);
    captured.push(`# 結束碼 ${code}`);
    const file = writeEvidence(EVIDENCE_DIR, name, captured);
    console.log(`[compare] 證據檔：host/tools/evidence/${path.basename(file)}`);
  }
}

run()
  .then((code) => process.exit(code))
  .catch((err) => {
    console.error(`[compare] 發生錯誤：${err.stack || err.message}`);
    process.exit(2);
  });
