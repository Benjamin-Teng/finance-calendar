#!/usr/bin/env node
// host/tests/compare/compare.mjs
//
// Lively 版 vs 新版小工具對照測試腳本（design.md D11、task-4.2-brief.md）。
//
// 兩種模式：
//   1. 自比對（預設，不帶 --widget）：對 Lively 版同一個面板連續擷取兩次（各自獨立的
//      headless Edge 分頁、獨立導覽），逐行比對文字。固定時區＋固定時間下應該完全一致——
//      這是本 task 的第一項驗收（「先以 Lively 版自比對通過」）。加 --inject-diff 時，
//      第二次擷取前會故意竄改一個 DOM 節點的文字，證明比對邏輯真的抓得到差異（第二項驗收
//      「故意注入差異的反例證明腳本會失敗」）。
//   2. 跨版本比對（--widget <id>）：Lively 版對應面板 vs 新版 `widget.html?w=<id>`，
//      供 4.3–4.5 搬移小工具時直接加這個參數驗收（brief「腳本介面要讓 4.3–4.5 直接加
//      --widget clock 等參數比對新舊版」）。本 task 只有 clock 小工具已搬移（task 4.1），
//      其餘九個會被 widget.html 顯示「尚未實作」佔位訊息，因此 --widget macro 這類指令
//      現在跑起來一定是 FAIL（預期中的 FAIL，不是腳本本身的 bug）——見 README「已知限制」。
//
// 用法見同目錄 README.md。
//
// 零安裝方案的選擇：全部用 Node 22 內建能力（全域 `fetch`／`WebSocket` 驅動 CDP，
// `node:http` 起靜態伺服器），不裝任何 npm 套件、不呼叫 Python，理由見 cdp.mjs／serve.mjs
// 檔頭註解；brief 允許 Python stdlib／Node 內建 WebSocket／PowerShell ClientWebSocket 三選一
// 「零安裝優先」，選 Node 是因為 CDP 驅動與靜態伺服器可以用同一個語言、同一支腳本完成，
// 比「Node 開瀏覽器＋Python 開伺服器」兩個語言互相等待、共用暫存路徑來得單純。
//
// `taipeiDateStr`／`buildOverrideScript`／`buildExtractScript` 抽到 `capture-utils.mjs`
// （task 4.4）：`verify-dividend-setting.mjs` 需要同一套「固定時區／固定時間／DOM 文字
// 擷取」邏輯，但不能直接 `import` 本檔——本檔檔尾 `run().then((code) =>
// process.exit(code))` 是 CLI 腳本慣例，只要被 `import` 就會在載入當下觸發自己的
// `run()`（預設跑五面板自比對）並在跑完時直接 `process.exit()`，提早結束呼叫端的行程。
// 抽成獨立、零副作用的模組給兩邊共用，本檔的 CLI 行為未變。

import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { startServer } from './serve.mjs';
import { launchEdge, newTarget, closeTarget, CDPTarget, waitForPageCondition, evaluate } from './cdp.mjs';
import { PANEL_IDS, getPanel } from './panels.mjs';
import { taipeiDateStr, buildOverrideScript, buildExtractScript } from './capture-utils.mjs';
import { evidenceName, writeEvidence } from './evidence.mjs';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(__dirname, '..', '..', '..');
const EVIDENCE_DIR = path.join(REPO_ROOT, 'host', 'tools', 'evidence');
const DEFAULT_FIXTURE = path.join(REPO_ROOT, 'host', 'ui', 'fixtures', 'tw-events.json');
// 週一、平日下午、非跨日邊界。task 4.4 核對 host/ui/fixtures/tw-events.json 的
// `holidays` 才發現這天實際上**是**休市日之一——不影響 clock／macro／fixed（不吃休市日），
// 對 `dynamic` 反而是意外之喜：預設 `--today` 不改就順帶測到「今天休市，順延下一交易日」
// 這條 spec Scenario（見 task-4.4-report.md「休市日驗證」一節的實測輸出：`休市，順延
// 9/29 (二)`）。
const DEFAULT_TODAY = '2026-09-28T14:05:00+08:00';

function printHelp() {
  console.log(`用法：node host/tests/compare/compare.mjs [選項]

模式（互斥，預設為自比對）：
  （不帶 --widget）        Lively 版自比對：同一面板擷取兩次，逐行比較
  --widget <id>            跨版本比對：Lively 版 vs 新版 widget.html?w=<id>
                            （id 為 clock｜macro｜fixed｜dynamic｜quotes）

其他選項：
  --today <ISO8601>        固定「現在」時間，含時區偏移，例如
                            2026-09-28T14:05:00+08:00（預設同此值）
  --fixture <path>         tw-events fixture JSON（預設 host/ui/fixtures/tw-events.json）
  --panel <id>             自比對模式限定只跑一個面板（預設五個全跑）
  --inject-diff [target]   自比對模式：第二次擷取前竄改一個 DOM 節點文字，證明比對邏輯
                            抓得到差異（反例驗證）。target 格式 <elementId>[:<追加文字>]，
                            預設 clockTime:__INJECTED_DIFF__
  --advance-to <ISO8601>   須搭配 --widget：只導覽新版頁面一次（固定在 --today），不重新
                            載入頁面的情況下把時間換成這個值並觸發 visibilitychange，
                            驗證「節流計時器恢復後立即重算」（design.md Risks 最後一條）
                            取代 Lively 版整頁重載（scheduleDailyReload／04:00）的效果；
                            比對對象是 Lively 版在這個時間點「重新導覽」的結果
                            （spec「跨日更新」）
  --keep-temp               不清除暫存伺服器目錄（除錯用，會印出路徑）
  --evidence [名稱]         另把本次輸出寫成 host/tools/evidence/compare-<名稱>.log（去識別：
                            使用者路徑改成 %TEMP% 等字樣）。省略名稱時依模式命名：self、
                            self-<panel>、widget-<id>、widget-<id>-advance
  -h, --help                顯示本說明

範例：
  node host/tests/compare/compare.mjs
  node host/tests/compare/compare.mjs --inject-diff
  node host/tests/compare/compare.mjs --widget clock
  node host/tests/compare/compare.mjs --widget clock --evidence
  node host/tests/compare/compare.mjs --panel dynamic --today 2026-10-03T09:00:00+08:00
  node host/tests/compare/compare.mjs --widget macro --today 2026-09-28T03:59:00+08:00 \\
    --advance-to 2026-09-28T04:01:00+08:00
`);
}

function parseArgs(argv) {
  const opts = {
    today: DEFAULT_TODAY,
    fixture: DEFAULT_FIXTURE,
    panel: null,
    widget: null,
    injectDiff: null,
    advanceTo: null,
    keepTemp: false,
    evidence: null,
    help: false,
  };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    switch (a) {
      case '--today':
        opts.today = argv[++i];
        break;
      case '--fixture':
        opts.fixture = path.resolve(argv[++i]);
        break;
      case '--panel':
        opts.panel = argv[++i];
        break;
      case '--widget':
        opts.widget = argv[++i];
        break;
      case '--advance-to':
        opts.advanceTo = argv[++i];
        break;
      case '--inject-diff': {
        const next = argv[i + 1];
        if (next && !next.startsWith('--')) {
          opts.injectDiff = next;
          i++;
        } else {
          opts.injectDiff = 'clockTime:__INJECTED_DIFF__';
        }
        break;
      }
      case '--keep-temp':
        opts.keepTemp = true;
        break;
      case '--evidence': {
        const next = argv[i + 1];
        if (next && !next.startsWith('--')) {
          opts.evidence = next;
          i++;
        } else {
          opts.evidence = true;
        }
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
  return opts;
}

// `taipeiDateStr`／`buildOverrideScript`／`buildExtractScript`：見 capture-utils.mjs
// （task 4.4 抽出，理由見上方檔頭註解）。

async function captureOnce({ port, url, selector, excludeSelectors, todayMs, todayStr, injectDiff, kind }) {
  const target = await newTarget(port, 'about:blank');
  const session = new CDPTarget(target.webSocketDebuggerUrl);
  try {
    await session.connect();
    await session.send('Page.enable');
    await session.send('Emulation.setTimezoneOverride', { timezoneId: 'Asia/Taipei' });
    await session.send('Page.addScriptToEvaluateOnNewDocument', {
      source: buildOverrideScript(todayMs, todayStr),
    });
    await session.send('Page.navigate', { url });

    if (kind === 'lively') {
      // 全頁只有一次資料載入入口（loadData() → 同源 <script src="tw_events.js">），
      // 不論比對哪個面板都等它跑完，比等特定面板的內容更新可靠。
      await waitForPageCondition(session, "typeof window.TW_EVENTS !== 'undefined'");
    } else {
      await waitForPageCondition(
        session,
        "!!document.getElementById('widget-root') && document.getElementById('widget-root').children.length > 0",
      );
    }
    // 資料就緒後到 renderXxx() 實際寫入 DOM 之間留一點緩衝（本機實測綽綽有餘，見 README）。
    await new Promise((r) => setTimeout(r, 200));

    if (injectDiff) {
      const [elId, ...rest] = injectDiff.split(':');
      const suffix = rest.length ? rest.join(':') : '__INJECTED_DIFF__';
      const mutateResult = await evaluate(
        session,
        `(() => { const el = document.getElementById(${JSON.stringify(elId)});
          if (!el) return { ok:false, error:'找不到元素 #' + ${JSON.stringify(elId)} };
          el.textContent = el.textContent + ${JSON.stringify(' ' + suffix)};
          return { ok:true }; })()`,
      );
      if (!mutateResult.ok) throw new Error(`--inject-diff 失敗：${mutateResult.error}`);
    }

    const result = await evaluate(session, buildExtractScript(selector, excludeSelectors));
    if (!result.ok) throw new Error(result.error);
    return result.lines;
  } finally {
    session.close();
    await closeTarget(port, target.id);
  }
}

function diffLines(a, b) {
  const max = Math.max(a.length, b.length);
  const diffs = [];
  for (let i = 0; i < max; i++) {
    if (a[i] !== b[i]) diffs.push({ index: i, a: a[i] ?? '(無此行)', b: b[i] ?? '(無此行)' });
  }
  return { pass: diffs.length === 0, diffs };
}

async function compareLivelySelf(port, baseUrl, panelId, todayMs, todayStr, injectDiff) {
  const panel = getPanel(panelId);
  const url = `${baseUrl}/lively/finance-calendar.html`;
  const common = { port, url, selector: panel.livelySelector, excludeSelectors: panel.exclude, todayMs, todayStr, kind: 'lively' };
  const a = await captureOnce({ ...common, injectDiff: null });
  const b = await captureOnce({ ...common, injectDiff });
  const diff = diffLines(a, b);
  return { ...diff, labelA: 'lively#1', labelB: injectDiff ? 'lively#2(已注入差異)' : 'lively#2', linesA: a, linesB: b };
}

async function compareWidget(port, baseUrl, widgetId, todayMs, todayStr) {
  const panel = getPanel(widgetId);
  const livelyUrl = `${baseUrl}/lively/finance-calendar.html`;
  const newUrl = `${baseUrl}/new/widget.html?w=${encodeURIComponent(widgetId)}&fixtures=/fixtures/`;
  const a = await captureOnce({
    port, url: livelyUrl, selector: panel.livelySelector, excludeSelectors: panel.exclude,
    todayMs, todayStr, injectDiff: null, kind: 'lively',
  });
  const b = await captureOnce({
    port, url: newUrl, selector: panel.newSelector, excludeSelectors: panel.exclude,
    todayMs, todayStr, injectDiff: null, kind: 'new',
  });
  const diff = diffLines(a, b);
  return { ...diff, labelA: 'lively', labelB: `new:${widgetId}`, linesA: a, linesB: b };
}

/**
 * 跨日／節流恢復情境（spec「跨日更新」；design.md Risks 最後一條；task-4.3-brief.md
 * pre-flight 裁決「03:59 → 04:01」）：新版頁面只導覽一次（固定在 `fromMs`），不重新載入
 * 頁面的情況下把 `Date` 換成 `toMs` 並觸發 `visibilitychange`，驗證小工具「節流計時器恢復後
 * 立即重算」（clock.js／macro.js 的 visibilitychange handler）足以取代 Lively 版
 * `scheduleDailyReload()`（04:00 整頁重載）的效果——比對基準是 Lively 版在 `toMs`
 * **重新導覽**的結果，不是同一份頁面的兩次擷取，理由是 Lively 版本來就是靠整頁重載達成
 * 「翻日後重算」，兩種機制（重載 vs 事件驅動重算）只要結果一致就算通過。
 */
async function compareWidgetLiveAdvance(port, baseUrl, widgetId, fromMs, fromStr, toMs, toStr) {
  const panel = getPanel(widgetId);
  const newUrl = `${baseUrl}/new/widget.html?w=${encodeURIComponent(widgetId)}&fixtures=/fixtures/`;
  const livelyUrl = `${baseUrl}/lively/finance-calendar.html`;

  const livelyAfter = await captureOnce({
    port,
    url: livelyUrl,
    selector: panel.livelySelector,
    excludeSelectors: panel.exclude,
    todayMs: toMs,
    todayStr: toStr,
    injectDiff: null,
    kind: 'lively',
  });

  const target = await newTarget(port, 'about:blank');
  const session = new CDPTarget(target.webSocketDebuggerUrl);
  let newAfter;
  try {
    await session.connect();
    await session.send('Page.enable');
    await session.send('Emulation.setTimezoneOverride', { timezoneId: 'Asia/Taipei' });
    await session.send('Page.addScriptToEvaluateOnNewDocument', {
      source: buildOverrideScript(fromMs, fromStr),
    });
    await session.send('Page.navigate', { url: newUrl });
    await waitForPageCondition(
      session,
      "!!document.getElementById('widget-root') && document.getElementById('widget-root').children.length > 0",
    );
    await new Promise((r) => setTimeout(r, 200));

    // 不重新導覽：直接在既有頁面把 window.Date 換成新的固定時間，再送一個
    // visibilitychange 事件（不需要真的把分頁切到背景再切回來——clock.js／macro.js 的
    // handler 只檢查「現在不是 hidden」，dispatch 這個事件本身就會呼叫到 handler）。
    const advanceResult = await evaluate(
      session,
      `(() => {
        const FIXED_MS = ${toMs};
        const RealDate = window.Date;
        class FixedDate extends RealDate {
          constructor(...args) { if (args.length === 0) { super(FIXED_MS); } else { super(...args); } }
          static now() { return FIXED_MS; }
        }
        window.Date = FixedDate;
        window.__TEST_TODAY = ${JSON.stringify(toStr)};
        document.dispatchEvent(new Event('visibilitychange'));
        return { ok:true };
      })()`,
    );
    if (!advanceResult.ok) throw new Error('推進時間失敗');
    await new Promise((r) => setTimeout(r, 200));

    const result = await evaluate(session, buildExtractScript(panel.newSelector, panel.exclude));
    if (!result.ok) throw new Error(result.error);
    newAfter = result.lines;
  } finally {
    session.close();
    await closeTarget(port, target.id);
  }

  const diff = diffLines(livelyAfter, newAfter);
  return {
    ...diff,
    labelA: `lively@${toStr}（重新導覽）`,
    labelB: `new:${widgetId}@live-advance（未重新導覽）`,
    linesA: livelyAfter,
    linesB: newAfter,
  };
}

function printResult(label, result) {
  console.log(`\n── ${label} ──`);
  console.log(`  ${result.labelA}：${result.linesA.length} 行／${result.labelB}：${result.linesB.length} 行`);
  if (result.pass) {
    console.log('  PASS（逐行完全相同）');
    return;
  }
  console.log(`  FAIL（${result.diffs.length} 處不同）`);
  for (const d of result.diffs.slice(0, 30)) {
    console.log(`    [${d.index}] - ${d.a}`);
    console.log(`    [${d.index}] + ${d.b}`);
  }
  if (result.diffs.length > 30) {
    console.log(`    ...（其餘 ${result.diffs.length - 30} 處省略）`);
  }
}

// fix F7：`--evidence [名稱]` 時把本次所有 console.log 輸出同步收集起來，結束（含例外）時去識別寫成
// host/tools/evidence/compare-<名稱>.log（見 evidence.mjs）。
async function run() {
  const opts = parseArgs(process.argv.slice(2));
  if (opts.help) {
    printHelp();
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

async function runCompare(opts) {
  const todayMs = Date.parse(opts.today);
  if (Number.isNaN(todayMs)) {
    throw new Error(`--today 不是合法的日期時間字串：${opts.today}`);
  }
  const todayStr = taipeiDateStr(todayMs);

  console.log(`[compare] --today=${opts.today}（epoch=${todayMs}，Asia/Taipei 今天=${todayStr}）`);
  console.log(`[compare] --fixture=${opts.fixture}`);

  const server = await startServer(opts.fixture);
  console.log(`[compare] 本機伺服器：${server.url}（暫存目錄：${server.tmpRootDisplay}）`);

  const edge = await launchEdge();
  console.log(`[compare] headless Edge 已啟動，devtools port=${edge.port}`);

  let overallPass = true;
  try {
    if (opts.widget && opts.advanceTo) {
      const advanceToMs = Date.parse(opts.advanceTo);
      if (Number.isNaN(advanceToMs)) {
        throw new Error(`--advance-to 不是合法的日期時間字串：${opts.advanceTo}`);
      }
      const advanceToStr = taipeiDateStr(advanceToMs);
      console.log(`[compare] --advance-to=${opts.advanceTo}（Asia/Taipei=${advanceToStr}）`);
      const result = await compareWidgetLiveAdvance(
        edge.port,
        server.url,
        opts.widget,
        todayMs,
        todayStr,
        advanceToMs,
        advanceToStr,
      );
      printResult(`跨日／節流恢復 widget=${opts.widget}（${opts.today} → ${opts.advanceTo}）`, result);
      overallPass = result.pass;
    } else if (opts.widget) {
      const result = await compareWidget(edge.port, server.url, opts.widget, todayMs, todayStr);
      printResult(`跨版本比對 widget=${opts.widget}`, result);
      overallPass = result.pass;
    } else {
      const panelIds = opts.panel ? [opts.panel] : PANEL_IDS;
      for (const id of panelIds) {
        const result = await compareLivelySelf(edge.port, server.url, id, todayMs, todayStr, opts.injectDiff);
        printResult(`自比對 panel=${id}`, result);
        if (!result.pass) overallPass = false;
      }
    }
  } finally {
    await edge.close();
    await server.close({ keepFiles: opts.keepTemp });
    if (opts.keepTemp) {
      console.log(`[compare] --keep-temp：已保留暫存目錄 ${server.tmpRoot}（除錯用實際路徑，勿貼進證據）`);
    }
  }

  console.log(overallPass ? '\n[compare] 總結：PASS' : '\n[compare] 總結：FAIL');
  return overallPass ? 0 : 1;
}

run()
  .then((code) => process.exit(code))
  .catch((err) => {
    console.error(`[compare] 發生錯誤：${err.stack || err.message}`);
    process.exit(2);
  });
