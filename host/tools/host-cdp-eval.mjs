// host/tools/host-cdp-eval.mjs
//
// 對「執行中的 fc-host」某個小工具頁面執行一段 JS（task 3.1 驗收：從頁面呼叫
// `update_settings` 觸發視窗工廠即時建立／關閉視窗）。前提：宿主以環境變數
// `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=<port>` 啟動（WebView2 會把
// 這個變數附加到瀏覽器程序參數；驅動腳本 verify-3.1.ps1 負責設定，正式執行不帶）。
//
// 用法：
//   node host/tools/host-cdp-eval.mjs <port> <url 子字串> <JS 運算式>
// 例：
//   node host/tools/host-cdp-eval.mjs 9333 "w=clock" "window.__TAURI__.core.invoke('get_settings')"
//
// 運算式以 `awaitPromise: true` 執行，結果以 JSON 印到 stdout；頁面端例外或找不到分頁時
// 以非 0 結束碼離開。CDP 客戶端沿用 host/tests/compare/cdp.mjs（零安裝，Node 內建 WebSocket）。
//
// 只在「已提交」的文件執行（evalOnCommittedPage）：/json/list 的 url 是 WebView2 的可見 URL，
// 導覽一開始就換成目標網址，真正提交要再晚 20–70 ms（宿主主執行緒忙時更久）；這段期間連上去
// 執行的運算式其實跑在初始的 about:blank 文件裡——同步讀 DOM 得到「元素不存在」，輪詢型運算式
// 在提交那一刻拿到「Execution context was destroyed」（2026-10-06 verify-5.2／5.4 回歸，CDP
// 事件實測）。故先讀 location.href 確認文件已是目標網址才執行；運算式只執行一次，已提交之後
// 才發生的 context destroyed（真的重新載入）原樣往上丟，不重跑有副作用的運算式。

import { pathToFileURL } from 'node:url';
import { CDPTarget, evaluate } from '../tests/compare/cdp.mjs';

const CONTEXT_GONE = /Execution context was destroyed|Cannot find context with specified id/;

/**
 * 找出 url 含 `urlPart` 的分頁，等它的文件提交到目標網址後執行 `expression` 一次並回傳結果。
 * 依賴注入（測試用）：listTargets() → /json/list 陣列；connect(page) → { evaluate(expr), close() }。
 */
export async function evalOnCommittedPage({
  listTargets,
  connect,
  urlPart,
  expression,
  timeoutMs = 10000,
  pollMs = 100,
  sleep = (ms) => new Promise((r) => setTimeout(r, ms)),
  now = () => Date.now(),
}) {
  const deadline = now() + timeoutMs;
  let lastHref = '';
  for (;;) {
    const list = await listTargets();
    const page = list.find((t) => t.type === 'page' && t.url.includes(urlPart));
    if (!page) {
      throw new Error(`找不到 url 含「${urlPart}」的分頁；現有：${list.map((t) => t.url).join(', ')}`);
    }
    const target = await connect(page);
    try {
      let committed = false;
      try {
        lastHref = String(await target.evaluate('location.href'));
        committed = lastHref.includes(urlPart);
      } catch (e) {
        if (!CONTEXT_GONE.test(e.message)) throw e;
      }
      if (committed) return await target.evaluate(expression);
    } finally {
      target.close();
    }
    if (now() >= deadline) {
      throw new Error(`url 含「${urlPart}」的分頁在 ${timeoutMs} ms 內尚未提交（文件仍是 ${lastHref || '未知'}）`);
    }
    await sleep(pollMs);
  }
}

async function main() {
  const [portArg, urlPart, expression] = process.argv.slice(2);
  if (!portArg || !urlPart || !expression) {
    console.error('用法：node host-cdp-eval.mjs <port> <url 子字串> <JS 運算式>');
    process.exit(2);
  }
  try {
    const value = await evalOnCommittedPage({
      listTargets: async () => (await fetch(`http://127.0.0.1:${portArg}/json/list`)).json(),
      connect: async (page) => {
        const target = new CDPTarget(page.webSocketDebuggerUrl);
        await target.connect();
        return { evaluate: (expr) => evaluate(target, expr), close: () => target.close() };
      },
      urlPart,
      expression,
    });
    console.log(JSON.stringify(value));
  } catch (e) {
    console.error(e.message);
    process.exit(1);
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  await main();
}
