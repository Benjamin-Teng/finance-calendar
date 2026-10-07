// host/tools/cdp-crash.mjs
//
// 讓測試宿主的 WebView2 子行程「真的當機」（不是 TerminateProcess）：soak 事件重現用
// （repro-soak-exit.ps1 的 -RealCrash）。前提同 host-cdp-eval.mjs：宿主以
// `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=<port>` 啟動。
//
// 用法：
//   node host/tools/cdp-crash.mjs <port> renderers [gapMs]   每個 page 依序送 Page.crash
//   node host/tools/cdp-crash.mjs <port> gpu                  browser 端點送 Browser.crashGpuProcess
//   node host/tools/cdp-crash.mjs <port> browser              browser 端點送 Browser.crash
//   node host/tools/cdp-crash.mjs <port> page <小工具 id>      只對 widget.html?w=<id> 那一頁送 Page.crash
//                                                             （verify-7.3：精準觸發該小工具的 Reload）
//
// 安全（fix F5，review fix-soak high）：埠被別的 Chromium 系程式佔用時，連到的就是那個程式，
// 當機指令會打到它身上。兩道檢查：
//   1. 主要檢查在呼叫端：repro-soak-exit.ps1 每次呼叫本工具前，確認在該埠 Listen 的行程是
//      本次測試宿主的子孫，不符就不呼叫。
//   2. 本工具自己再檢查一次 [`checkHostTargets`]：/json/list 的 page 目標必須至少一個、而且
//      全部是宿主頁面（tauri.localhost），否則結束碼 3、一個指令都不送。其他 Tauri 程式也用
//      tauri.localhost，所以這一道只是補強，不能取代第 1 道。
// CDP 指令名稱查證：Chrome DevTools Protocol（Page.crash、Browser.crashGpuProcess、
// Browser.crash，皆標 Experimental）。

import { pathToFileURL } from 'node:url';
import { CDPTarget } from '../tests/compare/cdp.mjs';

/** 宿主頁面的來源：Tauri 在 Windows 上以 http(s)://tauri.localhost 提供前端。 */
function isHostUrl(url) {
  try {
    const u = new URL(String(url));
    return (u.protocol === 'http:' || u.protocol === 'https:') && u.hostname === 'tauri.localhost';
  } catch {
    return false;
  }
}

/**
 * /json/list 的回傳是否全是宿主頁面。回傳 `{ ok, reason, pages }`：`pages` 是 type=page 的目標；
 * 沒有任何 page、或有任何 page 不是宿主頁面，`ok` 為 false。
 */
export function checkHostTargets(list) {
  if (!Array.isArray(list)) return { ok: false, reason: '/json/list 回傳不是陣列', pages: [] };
  const pages = list.filter((t) => t && t.type === 'page');
  if (pages.length === 0) return { ok: false, reason: '沒有任何 page 目標，無法確認是測試宿主', pages };
  const foreign = pages.filter((p) => !isHostUrl(p.url));
  if (foreign.length > 0) {
    return { ok: false, reason: `有非宿主頁面：${foreign.map((p) => p.url).join(', ')}`, pages };
  }
  return { ok: true, reason: '', pages };
}

/**
 * 從 /json/list 挑出 `widget.html?w=<id>` 那一頁（宿主頁面、`w` 參數完全相等、恰好一頁）。
 * 回傳 `{ ok, reason, page }`；找不到或有多頁都不 ok（不猜）。
 */
export function selectWidgetPage(list, id) {
  const pages = (Array.isArray(list) ? list : []).filter((t) => {
    if (!t || t.type !== 'page' || !isHostUrl(t.url)) return false;
    try {
      return new URL(String(t.url)).searchParams.get('w') === id;
    } catch {
      return false;
    }
  });
  if (pages.length !== 1) return { ok: false, reason: `w=${id} 的宿主頁面有 ${pages.length} 頁（必須恰好 1 頁）`, page: null };
  return { ok: true, reason: '', page: pages[0] };
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function fire(wsUrl, method) {
  const t = new CDPTarget(wsUrl);
  await t.connect();
  // 當機指令通常不會回應（行程直接結束），只等一小段時間就放手。
  await t.send(method, {}, { timeoutMs: 1500 }).catch((e) => console.log(`${method}: ${e.message}`));
  t.close();
}

async function main() {
  const [portArg, mode, gapArg] = process.argv.slice(2);
  if (!portArg || !mode) {
    console.error('用法：node cdp-crash.mjs <port> renderers|gpu|browser [gapMs]｜page <小工具 id>');
    process.exit(2);
  }
  if (!['renderers', 'gpu', 'browser', 'page'].includes(mode)) {
    console.error(`未知模式：${mode}`);
    process.exit(2);
  }
  const gapMs = mode === 'page' ? 0 : Number(gapArg ?? 450);

  const list = await (await fetch(`http://127.0.0.1:${portArg}/json/list`)).json();
  const check = checkHostTargets(list);
  if (!check.ok) {
    console.error(`中止：埠 ${portArg} 上的目標不像測試宿主（${check.reason}），未送出任何指令`);
    process.exit(3);
  }

  if (mode === 'page') {
    const sel = selectWidgetPage(list, gapArg);
    if (!sel.ok) {
      console.error(`中止：${sel.reason}，未送出任何指令`);
      process.exit(3);
    }
    console.log(`Page.crash ${sel.page.url}`);
    await fire(sel.page.webSocketDebuggerUrl, 'Page.crash');
  } else if (mode === 'renderers') {
    for (const p of check.pages) {
      console.log(`Page.crash ${p.url}`);
      await fire(p.webSocketDebuggerUrl, 'Page.crash');
      await sleep(gapMs);
    }
  } else {
    const ver = await (await fetch(`http://127.0.0.1:${portArg}/json/version`)).json();
    const method = mode === 'gpu' ? 'Browser.crashGpuProcess' : 'Browser.crash';
    console.log(method);
    await fire(ver.webSocketDebuggerUrl, method);
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  await main();
}
