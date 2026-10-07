#!/usr/bin/env node
// host/tests/compare/regression-macro-scroll.mjs
//
// 回歸測試（task 4.3 fix round 1，Codex finding：host/ui/widgets/macro.js:147-148）：
// 週期重算（setInterval 60 秒）與 visibilitychange 立即重算，SHALL NOT 打斷使用者正在
// 閱讀的捲動位置。修好之前（原始 f36c52b）兩者都會無條件呼叫 render(lastData)，而
// render() 開頭 `list.innerHTML = ''` 清空整份清單、結尾又把 scrollTop 拉回第一筆未發生
// 事件——Codex 用 Node 模擬 DOM 重現：scrollTop=400 後觸發計時回呼變 86。
//
// 用真正的 headless Edge（不是手刻 DOM／HTML parser）：macro.js 產生的 HTML 有 <li>／
// <div class="dayhead">等巢狀結構，若要在純 Node 環境正確模擬 querySelector／
// insertAdjacentHTML／scrollTop 這些行為，等於要重寫一個小型瀏覽器引擎——不如直接用
// 這個 repo 既有的零安裝 CDP 骨架（cdp.mjs／serve.mjs，同 host/tests/compare/README.md
// 「零安裝方案的選擇」的理由），跑真正的 DOM，測到的才是「真的會不會在瀏覽器裡發生」。
//
// 三個斷言：
//   1. `visibilitychange` 觸發後，捲動位置不變（Codex finding 明確要求測這條路徑）。
//   2. 週期計時器（setInterval 60000ms）觸發後，捲動位置不變（同上，且不必真的等 60
//      秒——見下方「如何不等待真實 60 秒」）。
//   3. 「不是無事發生的假修復」：把 `Date` 推進到某筆事件之後，重新觸發週期重算，
//      確認該筆事件確實從「未發生」變成「已過去」（`.past` class）——證明週期重算仍然
//      持續更新今天標記／過期狀態，不是單純把整個機制關掉。
//
// ## 如何不等待真實 60 秒
// macro.js 的 setInterval 呼叫是頁面自己的 JS 呼叫瀏覽器原生 `setInterval`，沒有對外
// 公開的測試鉤子可以直接呼叫。用 `Page.addScriptToEvaluateOnNewDocument` 在
// widget.html 的任何模組腳本執行之前，先把 `window.setInterval` 包一層：呼叫原生版本
// （讓其餘行為不變）之餘，把 `(fn, delay)` 記錄到 `window.__capturedTimers`。測試腳本
// 之後直接呼叫 `window.__capturedTimers.find(t => t.delay === 60000).fn()`，等同「模擬
// 這一輪 60 秒到了」，但是在真正的頁面環境、真正的 DOM 上執行——比等待虛擬時間
// （`Emulation.setVirtualTimePolicy`）或真的等 60 秒更直接可靠，且沒有引入新的
// 跨檔案依賴（不動 cdp.mjs／compare.mjs）。

import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { startServer } from './serve.mjs';
import { launchEdge, newTarget, closeTarget, CDPTarget, waitForPageCondition, evaluate } from './cdp.mjs';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(__dirname, '..', '..', '..');
const DEFAULT_FIXTURE = path.join(REPO_ROOT, 'host', 'ui', 'fixtures', 'tw-events.json');

const CAPTURE_TIMERS_SCRIPT = `(() => {
  window.__capturedTimers = [];
  const realSetInterval = window.setInterval.bind(window);
  window.setInterval = (fn, delay, ...rest) => {
    window.__capturedTimers.push({ fn, delay });
    return realSetInterval(fn, delay, ...rest);
  };
})();`;

async function run() {
  const server = await startServer(DEFAULT_FIXTURE);
  console.log(`[regression] 本機伺服器：${server.url}（暫存目錄：${server.tmpRootDisplay}）`);
  const edge = await launchEdge();
  console.log(`[regression] headless Edge 已啟動，devtools port=${edge.port}`);

  let pass = true;
  const target = await newTarget(edge.port, 'about:blank');
  const session = new CDPTarget(target.webSocketDebuggerUrl);
  try {
    await session.connect();
    await session.send('Page.enable');
    await session.send('Page.addScriptToEvaluateOnNewDocument', { source: CAPTURE_TIMERS_SCRIPT });
    await session.send('Page.navigate', {
      url: `${server.url}/new/widget.html?w=macro&fixtures=/fixtures/`,
    });
    await waitForPageCondition(
      session,
      "!!document.getElementById('widget-root') && document.getElementById('widget-root').children.length > 0",
    );
    await new Promise((r) => setTimeout(r, 200));

    // 前提檢查：清單真的可捲動（否則 scrollTop 測試沒有意義）。
    const precheck = await evaluate(
      session,
      `(() => {
        const list = document.querySelector('.evlist');
        return {
          evCount: document.querySelectorAll('.ev').length,
          scrollable: list.scrollHeight > list.clientHeight,
          scrollHeight: list.scrollHeight,
          clientHeight: list.clientHeight,
        };
      })()`,
    );
    console.log(`[regression] 前提檢查：${JSON.stringify(precheck)}`);
    if (!precheck.scrollable || precheck.evCount < 5) {
      throw new Error('fixture 內容不足以測試捲動（清單不可捲動或事件數太少），檢查 fixture');
    }

    // ── 斷言 1：visibilitychange 觸發後，捲動位置不變 ──────────────────────────────
    const beforeVisibility = await evaluate(
      session,
      `(() => { document.querySelector('.evlist').scrollTop = 400; return document.querySelector('.evlist').scrollTop; })()`,
    );
    const afterVisibility = await evaluate(
      session,
      `(() => { document.dispatchEvent(new Event('visibilitychange')); return document.querySelector('.evlist').scrollTop; })()`,
    );
    const visibilityPass = afterVisibility === beforeVisibility;
    console.log(
      `[regression] 斷言 1（visibilitychange 保留捲動位置）：before=${beforeVisibility} after=${afterVisibility} → ${visibilityPass ? 'PASS' : 'FAIL'}`,
    );
    if (!visibilityPass) pass = false;

    // ── 斷言 2：週期計時器（setInterval 60000ms）觸發後，捲動位置不變 ──────────────
    const beforeInterval = await evaluate(
      session,
      `(() => { document.querySelector('.evlist').scrollTop = 400; return document.querySelector('.evlist').scrollTop; })()`,
    );
    const intervalResult = await evaluate(
      session,
      `(() => {
        const t = window.__capturedTimers.find((x) => x.delay === 60000);
        if (!t) return { ok:false, error:'沒有捕捉到 60000ms 的 setInterval（macro.js 是否還在用這個週期？）' };
        t.fn();
        return { ok:true, scrollTop: document.querySelector('.evlist').scrollTop };
      })()`,
    );
    if (!intervalResult.ok) throw new Error(intervalResult.error);
    const intervalPass = intervalResult.scrollTop === beforeInterval;
    console.log(
      `[regression] 斷言 2（週期計時器保留捲動位置）：before=${beforeInterval} after=${intervalResult.scrollTop} → ${intervalPass ? 'PASS' : 'FAIL'}`,
    );
    if (!intervalPass) pass = false;

    // ── 斷言 3：週期重算不是空殼——把 Date 推到某筆未發生事件之後，重新觸發後
    //    該筆事件應變成「已過去」（.past），且捲動位置仍不變 ───────────────────────
    const beforeAdvance = await evaluate(
      session,
      `(() => { document.querySelector('.evlist').scrollTop = 400; return document.querySelector('.evlist').scrollTop; })()`,
    );
    const advanceResult = await evaluate(
      session,
      `(() => {
        const target = document.querySelector('.ev:not(.past)[data-past-at]');
        if (!target) return { ok:false, error:'找不到尚未過去的事件（data-past-at 屬性缺失？或全部都已過去）' };
        const thresholdMs = Number(target.dataset.pastAt);
        const FIXED_MS = thresholdMs + 60 * 1000; // 推到該事件之後 1 分鐘
        const RealDate = window.Date;
        class FixedDate extends RealDate {
          constructor(...args) { if (args.length === 0) { super(FIXED_MS); } else { super(...args); } }
          static now() { return FIXED_MS; }
        }
        window.Date = FixedDate;
        window.__regressionTargetIndex = Array.prototype.indexOf.call(
          document.querySelectorAll('.ev[data-past-at]'), target,
        );
        const t = window.__capturedTimers.find((x) => x.delay === 60000);
        t.fn();
        const nodes = document.querySelectorAll('.ev[data-past-at]');
        const same = nodes[window.__regressionTargetIndex];
        return {
          ok: true,
          becamePast: same.classList.contains('past'),
          scrollTop: document.querySelector('.evlist').scrollTop,
        };
      })()`,
    );
    if (!advanceResult.ok) throw new Error(advanceResult.error);
    const advancePass = advanceResult.becamePast && advanceResult.scrollTop === beforeAdvance;
    console.log(
      `[regression] 斷言 3（週期重算持續更新過期狀態＋不動捲動位置）：` +
        `becamePast=${advanceResult.becamePast} scrollTop ${beforeAdvance}→${advanceResult.scrollTop} → ${advancePass ? 'PASS' : 'FAIL'}`,
    );
    if (!advancePass) pass = false;
  } finally {
    session.close();
    await closeTarget(edge.port, target.id);
    await edge.close();
    await server.close();
  }

  console.log(pass ? '\n[regression] 總結：PASS' : '\n[regression] 總結：FAIL');
  return pass ? 0 : 1;
}

run()
  .then((code) => process.exit(code))
  .catch((err) => {
    console.error(`[regression] 發生錯誤：${err.stack || err.message}`);
    process.exit(2);
  });
