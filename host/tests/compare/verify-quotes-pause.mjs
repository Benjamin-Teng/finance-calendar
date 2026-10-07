#!/usr/bin/env node
// host/tests/compare/verify-quotes-pause.mjs
//
// 驗證行情條跑馬燈的暫停/恢復（task-4.5-brief.md：「以測試用命令送出 pause／恢復事件，驗證
// 停止與從停止處繼續」）與無資料時內容高度為 0（design.md D7）。
//
// 這兩件事都不適合塞進 `compare.mjs` 既有的兩種模式（自比對／跨版本比對）：
//   - Lively 版完全沒有「暫停」這個概念（見 quotes.js 檔頭「暫停/恢復」一節），沒有對應的
//     Lively 版行為可以比對，只能單獨對新版頁面驗證「這個新行為本身做得對不對」。
//   - 「無資料時視窗隱藏」需要讀 `window.__bridgeTest.lastReportContent`（bridge.js fixture
//     模式的測試掛鉤，task 4.5 新增；task 7.3 起改記布林值本身，取代原本的 `{width,height}`），
//     不是逐行文字比對能表達的東西。
// 因此另開一支腳本，比照 `verify-dividend-setting.mjs`（task 4.4）的模式：不重造 CDP／伺服器
// 骨架，直接 import `serve.mjs`／`cdp.mjs`。
//
// 用法：node host/tests/compare/verify-quotes-pause.mjs

import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { startServer } from './serve.mjs';
import { launchEdge, newTarget, closeTarget, CDPTarget, waitForPageCondition, evaluate } from './cdp.mjs';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(__dirname, '..', '..', '..');
const DEFAULT_FIXTURE = path.join(REPO_ROOT, 'host', 'ui', 'fixtures', 'tw-events.json');
const EMPTY_QUOTES_FIXTURE = path.join(__dirname, 'fixtures', 'tw-events-empty-quotes.json');

async function withPage(edge, serverUrl, urlPath, fn) {
  const target = await newTarget(edge.port, 'about:blank');
  const session = new CDPTarget(target.webSocketDebuggerUrl);
  try {
    await session.connect();
    await session.send('Page.enable');
    await session.send('Page.navigate', { url: `${serverUrl}${urlPath}` });
    await waitForPageCondition(
      session,
      "!!document.getElementById('widget-root') && document.getElementById('widget-root').children.length > 0",
    );
    await new Promise((r) => setTimeout(r, 200)); // 給 mount()/render() 的首次渲染留緩衝
    return await fn(session);
  } finally {
    session.close();
    await closeTarget(edge.port, target.id);
  }
}

/** 驗證①：跑馬燈真的在動、②暫停後停止、③恢復後從暫停處（不是從頭）繼續前進。 */
async function verifyPauseResume(edge, serverUrl) {
  const failures = [];
  await withPage(edge, serverUrl, '/new/widget.html?w=quotes&fixtures=/fixtures/', async (session) => {
    const scrollLeft = () => evaluate(session, "document.querySelector('.tlist').scrollLeft");

    // 前提：內容確實超寬、跑馬燈確實在跑（否則後面「暫停後不動」這件事沒有鑑別力——
    // 靜止的頁面本來就不會動，通過測試沒有意義）。
    const before = await scrollLeft();
    await new Promise((r) => setTimeout(r, 600));
    const running = await scrollLeft();
    if (before === running) {
      failures.push(`前提不成立：跑馬燈啟動後 600ms scrollLeft 未變化（before=${before} running=${running}），` + '後續暫停測試無鑑別力');
    }

    // 暫停：送 {paused:true}，確認之後不再前進。
    await evaluate(session, "window.__bridgeTest.emit('pause', {paused:true, reason:'test'})");
    const pausedAt = await scrollLeft();
    await new Promise((r) => setTimeout(r, 600));
    const stillPaused = await scrollLeft();
    if (pausedAt !== stillPaused) {
      failures.push(`暫停後仍在移動：pausedAt=${pausedAt} → 600ms 後=${stillPaused}`);
    }

    // 恢復：送 {paused:false}，確認從 stillPaused 繼續前進（不是歸零重新開始）。
    await evaluate(session, "window.__bridgeTest.emit('pause', {paused:false})");
    await new Promise((r) => setTimeout(r, 600));
    const resumed = await scrollLeft();
    if (resumed === stillPaused) {
      failures.push(`恢復後未繼續前進：暫停時=${stillPaused} → 恢復 600ms 後=${resumed}`);
    }
    // 「從停止處繼續」的關鍵不是「resumed 比 stillPaused 大」（頭尾相接模式會在
    // loopAt 折返成更小的數字，屬正常 wraparound，不是回到頭），而是「resumed 不等於
    // stillPaused 且不是從 0 附近重新起跑」——若 resumed 剛好落在 0 附近且
    // stillPaused 明顯不是 0，才需要進一步懷疑是不是被重置；用 loopAt 一併印出來
        // 供人工複核。
    const loopAt = await evaluate(session, '(() => { const el = document.querySelector(".tlist"); return el ? el.scrollWidth / 2 : null; })()');
    console.log(
      `[verify-quotes-pause] before=${before} running=${running} pausedAt=${pausedAt} ` +
        `stillPaused=${stillPaused} resumed=${resumed}（loopAt≈${loopAt}）`,
    );
    if (stillPaused > 5 && resumed < 2 && stillPaused < loopAt - 5) {
      failures.push(`恢復後的位置（${resumed}）疑似被重置為接近 0，而非從暫停處（${stillPaused}）繼續——loopAt≈${loopAt}`);
    }
  });
  return failures;
}

/** 驗證：quotes fixture 為空陣列時，report_content 回報 false（design.md D7；宿主依此隱藏
 * 視窗）。task 7.3：`lastReportContent` 的合法值本身可能是 `false`，用 `typeof === 'boolean'`
 * 判斷「有沒有回報過」，不能用 truthy。 */
async function verifyEmptyHeightZero(edge, serverUrl) {
  const failures = [];
  await withPage(edge, serverUrl, '/new/widget.html?w=quotes&fixtures=/fixtures/', async (session) => {
    await waitForPageCondition(
      session,
      "!!window.__bridgeTest && typeof window.__bridgeTest.lastReportContent === 'boolean'",
    );
    const hasContent = await evaluate(session, 'window.__bridgeTest.lastReportContent');
    const displayed = await evaluate(session, "document.getElementById('widget-root').firstElementChild.style.display");
    console.log(`[verify-quotes-pause] 空 quotes：report_content=${JSON.stringify(hasContent)}，container.style.display=${JSON.stringify(displayed)}`);
    if (hasContent !== false) {
      failures.push(`空 quotes 應回報 report_content(false)，實際=${JSON.stringify(hasContent)}`);
    }
    if (displayed !== 'none') {
      failures.push(`空 quotes 應 display:none，實際=${displayed}`);
    }
  });
  return failures;
}

async function run() {
  const edge = await launchEdge();
  console.log(`[verify-quotes-pause] headless Edge 已啟動，devtools port=${edge.port}`);
  let failures = [];
  try {
    const serverA = await startServer(DEFAULT_FIXTURE);
    console.log(`[verify-quotes-pause] 伺服器（真實 fixture，13 檔行情）：${serverA.url}`);
    try {
      failures = failures.concat(await verifyPauseResume(edge, serverA.url));
    } finally {
      await serverA.close();
    }

    const serverB = await startServer(EMPTY_QUOTES_FIXTURE);
    console.log(`[verify-quotes-pause] 伺服器（quotes:[] fixture）：${serverB.url}`);
    try {
      failures = failures.concat(await verifyEmptyHeightZero(edge, serverB.url));
    } finally {
      await serverB.close();
    }
  } finally {
    await edge.close();
  }

  if (failures.length) {
    console.log(`\n[verify-quotes-pause] FAIL（${failures.length} 項）`);
    for (const f of failures) console.log(`  - ${f}`);
    return 1;
  }
  console.log('\n[verify-quotes-pause] PASS：跑馬燈暫停/恢復、空 quotes 高度 0 皆符合預期');
  return 0;
}

run()
  .then((code) => process.exit(code))
  .catch((err) => {
    console.error(`[verify-quotes-pause] 發生錯誤：${err.stack || err.message}`);
    process.exit(2);
  });
