// host/tools/cdp-widgets.mjs
//
// task 5.6 驗收輔助：透過 WebView2 CDP 除錯埠一次讀取「執行中的 fc-host」所有小工具頁面
// （以及設定視窗）的狀態，或在指定小工具頁面注入 JS 無窮迴圈（只對本宿主的頁面，埠號由驅動
// 腳本 verify-5.6.ps1 以 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=<port>`
// 指定給本次啟動的宿主）。
//
// 用法：
//   node host/tools/cdp-widgets.mjs <port> state [marker]
//     → stdout 印一行 JSON：[{ kind, id, targetId, url, ok, len, timeOrigin, text, err }]
//       kind＝'widget'（url 含 widget.html?w=）或 'settings'（url 含 settings.html）。
//       ok＝頁面已載入並掛上小工具內容（#widget-root 有子元素、沒有佔位訊息；設定視窗則是
//       readyState complete 且 body 有文字）。卡死或已失效的頁面在逾時後 ok=false、err 有值。
//       給了 marker 時另加 hasMarker＝頁面文字是否含該字串（驗證重建後資料推播仍送達）。
//   node host/tools/cdp-widgets.mjs <port> hang <id>
//     → 在 url 含 `w=<id>` 的頁面排一個 50 ms 後開始的同步無窮迴圈（評估本身立即返回），
//       模擬「頁面 JS 卡死、沒有任何 ProcessFailed 事件」（探針 1.4 的 U／H 階段）。
//
// CDP 客戶端沿用 host/tests/compare/cdp.mjs（零安裝）。每個頁面的連線／指令都有逾時，卡死的
// 頁面不會讓整支腳本卡住。

import { CDPTarget, evaluate } from '../tests/compare/cdp.mjs';

const [portArg, cmd, arg] = process.argv.slice(2);
if (!portArg || !cmd) {
  console.error('用法：node cdp-widgets.mjs <port> state | hang <id>');
  process.exit(2);
}

const PAGE_TIMEOUT_MS = 2500;

async function listPages() {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), 2000);
  try {
    const res = await fetch(`http://127.0.0.1:${portArg}/json/list`, { signal: controller.signal });
    return (await res.json()).filter((t) => t.type === 'page');
  } finally {
    clearTimeout(timer);
  }
}

function classify(url) {
  const m = /widget\.html\?w=([a-z0-9]+)/.exec(url);
  if (m) return { kind: 'widget', id: m[1] };
  if (url.includes('settings.html')) return { kind: 'settings', id: 'settings' };
  return null;
}

const PROBE = `(() => {
  const marker = ${JSON.stringify(cmd === 'state' && arg ? arg : '')};
  const all = document.body ? document.body.innerText : '';
  const hasMarker = marker ? all.includes(marker) : null;
  const root = document.getElementById('widget-root');
  if (root) {
    return {
      ok: document.readyState === 'complete' && root.children.length > 0 &&
          !root.querySelector('.widget-placeholder'),
      len: root.innerText.trim().length,
      text: root.innerText.replace(/\\s+/g, ' ').trim().slice(0, 160),
      timeOrigin: performance.timeOrigin,
      hasMarker,
    };
  }
  const text = document.body ? document.body.innerText.replace(/\\s+/g, ' ').trim() : '';
  return {
    ok: document.readyState === 'complete' && text.length > 0,
    len: text.length,
    text: text.slice(0, 80),
    timeOrigin: performance.timeOrigin,
    hasMarker,
  };
})()`;

async function withTarget(page, fn) {
  const target = new CDPTarget(page.webSocketDebuggerUrl, {
    commandTimeoutMs: PAGE_TIMEOUT_MS,
    connectTimeoutMs: PAGE_TIMEOUT_MS,
  });
  try {
    await target.connect();
    return await fn(target);
  } finally {
    target.close();
  }
}

async function state() {
  let pages;
  try {
    pages = await listPages();
  } catch (e) {
    console.log(JSON.stringify({ error: `CDP 無法連線：${e.message}` }));
    return;
  }
  const rows = await Promise.all(
    pages
      .map((p) => ({ p, c: classify(p.url) }))
      .filter((x) => x.c)
      .map(async ({ p, c }) => {
        const base = { kind: c.kind, id: c.id, targetId: p.id, url: p.url };
        try {
          const v = await withTarget(p, (t) => evaluate(t, PROBE));
          return { ...base, ...v, err: null };
        } catch (e) {
          return { ...base, ok: false, len: 0, text: '', timeOrigin: null, err: e.message };
        }
      }),
  );
  console.log(JSON.stringify(rows));
}

async function hang(id) {
  const pages = await listPages();
  const page = pages.find((p) => p.url.includes(`widget.html?w=${id}`));
  if (!page) {
    console.error(`找不到 w=${id} 的頁面`);
    process.exit(1);
  }
  const v = await withTarget(page, (t) =>
    evaluate(t, "setTimeout(() => { for (;;) {} }, 50); 'armed'"),
  );
  console.log(JSON.stringify({ id, targetId: page.id, result: v }));
}

if (cmd === 'state') {
  await state();
} else if (cmd === 'hang' && arg) {
  await hang(arg);
} else {
  console.error(`未知指令：${cmd}`);
  process.exit(2);
}
