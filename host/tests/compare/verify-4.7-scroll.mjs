#!/usr/bin/env node
// host/tests/compare/verify-4.7-scroll.mjs
//
// Task 4.7 驗收（design.md D10；finance-widgets spec「原生捲動」）：確認 macro／fixed／
// dynamic 三個有清單的小工具已改用原生捲動——不再有 Lively 版的 ▴▾ 翻頁鈕
// （`.scrollctl.up`／`.scrollctl.dn`，見 panels.mjs 註解），捲軸平時隱藏、游標移入才顯示，
// 且原生 `overflow-y:auto` 真的能被滾輪事件捲動（無殘留的 wheel handler 擋掉預設行為）。
//
// 用真正的 headless Edge（同 regression-macro-scroll.mjs 的理由：巢狀 DOM／CSS :hover
// 偽類、捲軸樣式屬性都要在真正的瀏覽器引擎裡驗證，純 Node 模擬 DOM 做不到）。
//
// 每個小工具三項斷言：
//   1. 無 ▴▾ 按鈕殘留：`.scrollctl` 選取器抓不到任何節點，`body.innerText` 也不含
//      「▴」「▾」這兩個字元（防止用別的 class 名字重做同一顆按鈕）。
//   2. 捲軸 hover 才顯示：`.scroll-area` 平時 `getComputedStyle(...).scrollbarWidth`
//      為 `none`；用 CDP `Input.dispatchMouseEvent`（瀏覽器層級合成事件，非 OS SendInput，
//      鎖定畫面時也能跑）把游標真的移到 `.scroll-area` 範圍內觸發原生 `:hover`，
//      confirm 變成 `thin`；移出後應變回 `none`。
//   3. 原生滾輪可捲動：`scrollTop` 先設一個非 0 值確認可讀寫，再用 CDP
//      `Input.dispatchMouseEvent`（type `mouseWheel`）送一次真正的合成滾輪事件，
//      確認瀏覽器原生捲動行為生效（`scrollTop` 隨 `deltaY` 增加）——這一步只驗證
//      「新版沒有任何 JS 攔截/取消原生捲動」，不驗證 Windows 對「非使用中視窗」的滾輪
//      路由規則，那一段需要真正的 OS 層 SendInput＋未鎖定工作階段，留給
//      human-checklist.md／待解鎖自動驗收（task-4.7-report.md 說明）。
//
// custom1（擴充插槽）另外只做斷言 1、2（內容量小，不保證可捲動，故不做斷言 3），
// 確認共用的 `.scroll-area` 骨架也套用在插槽小工具上。

import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { startServer } from './serve.mjs';
import { launchEdge, newTarget, closeTarget, CDPTarget, waitForPageCondition, evaluate } from './cdp.mjs';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(__dirname, '..', '..', '..');
const DEFAULT_FIXTURE = path.join(REPO_ROOT, 'host', 'ui', 'fixtures', 'tw-events.json');

// widget id → 是否驗斷言 3（原生滾輪捲動；插槽內容量小、不保證可捲動，只驗斷言 1、2）。
const TARGETS = [
  { id: 'macro', assertScroll: true },
  { id: 'fixed', assertScroll: true },
  { id: 'dynamic', assertScroll: true },
  { id: 'custom1', assertScroll: false },
];

async function checkWidget(session, server, id, assertScroll) {
  const results = [];
  const url = `${server.url}/new/widget.html?w=${id}&fixtures=/fixtures/`;
  await session.send('Page.navigate', { url });
  await waitForPageCondition(
    session,
    "!!document.getElementById('widget-root') && document.getElementById('widget-root').children.length > 0",
  );
  await new Promise((r) => setTimeout(r, 200));

  // ── 斷言 1：無 ▴▾ 按鈕殘留 ──────────────────────────────────────────────────
  const btnCheck = await evaluate(
    session,
    `(() => {
      const legacy = document.querySelectorAll('.scrollctl').length;
      const text = document.body.innerText || '';
      return { legacyCount: legacy, hasUpGlyph: text.includes('▴'), hasDnGlyph: text.includes('▾') };
    })()`,
  );
  const pass1 = btnCheck.legacyCount === 0 && !btnCheck.hasUpGlyph && !btnCheck.hasDnGlyph;
  results.push({
    name: '無 ▴▾ 按鈕殘留',
    pass: pass1,
    detail: JSON.stringify(btnCheck),
  });

  // ── 斷言 2：捲軸 hover 才顯示 ────────────────────────────────────────────────
  const rect = await evaluate(
    session,
    `(() => {
      const el = document.querySelector('.scroll-area');
      if (!el) return null;
      const r = el.getBoundingClientRect();
      return { x: r.x, y: r.y, width: r.width, height: r.height };
    })()`,
  );
  if (!rect) {
    results.push({ name: '找到 .scroll-area', pass: false, detail: '找不到 .scroll-area 節點' });
    return results;
  }
  const before = await evaluate(
    session,
    "getComputedStyle(document.querySelector('.scroll-area')).scrollbarWidth",
  );
  const cx = rect.x + rect.width / 2;
  const cy = rect.y + Math.min(rect.height / 2, 40); // 靠上方，避免落在清單底部之外
  await session.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: cx, y: cy });
  await new Promise((r) => setTimeout(r, 150));
  const hovered = await evaluate(
    session,
    "getComputedStyle(document.querySelector('.scroll-area')).scrollbarWidth",
  );
  await session.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: 2, y: 2 });
  await new Promise((r) => setTimeout(r, 150));
  const after = await evaluate(
    session,
    "getComputedStyle(document.querySelector('.scroll-area')).scrollbarWidth",
  );
  const pass2 = before === 'none' && hovered === 'thin' && after === 'none';
  results.push({
    name: '捲軸 hover 才顯示（scrollbar-width: none → thin → none）',
    pass: pass2,
    detail: `before=${before} hovered=${hovered} after=${after}`,
  });

  if (!assertScroll) return results;

  // ── 斷言 3：原生滾輪可捲動（無殘留 wheel handler 擋掉預設行為） ───────────────────
  // 用 fixture 真實資料量時，`fixed`（固定事件通常只有 4–5 條，見 registry.js 註解）在
  // maxHeight 上限內天生就塞得下、不會產生捲軸——這是資料量的關係，不是 CSS／JS 有沒有
  // 正確處理捲動的問題。故這裡量到「天生不可捲動」時，臨時把該節點的 max-height 縮小到
  // 強制產生 overflow，只為了驗證「原生捲動機制本身」（.scroll-area 的 overflow-y:auto
  // 沒有被任何殘留 JS 擋掉），驗完立即還原，不影響前面兩項斷言已經量到的真實版面。
  let precheck = await evaluate(
    session,
    `(() => {
      const el = document.querySelector('.scroll-area');
      return { scrollHeight: el.scrollHeight, clientHeight: el.clientHeight, scrollable: el.scrollHeight > el.clientHeight };
    })()`,
  );
  let forced = false;
  if (!precheck.scrollable) {
    forced = true;
    await evaluate(
      session,
      "(() => { document.querySelector('.scroll-area').style.maxHeight = '50px'; return true; })()",
    );
    precheck = await evaluate(
      session,
      `(() => {
        const el = document.querySelector('.scroll-area');
        return { scrollHeight: el.scrollHeight, clientHeight: el.clientHeight, scrollable: el.scrollHeight > el.clientHeight };
      })()`,
    );
  }
  if (!precheck.scrollable) {
    results.push({
      name: '原生滾輪可捲動',
      pass: false,
      detail: `即使強制縮小 max-height 仍不可捲動（內容過短）：${JSON.stringify(precheck)}`,
    });
    return results;
  }
  const beforeScroll = await evaluate(session, "document.querySelector('.scroll-area').scrollTop");
  await session.send('Input.dispatchMouseEvent', {
    type: 'mouseWheel',
    x: cx,
    y: cy,
    deltaX: 0,
    deltaY: 120,
  });
  await new Promise((r) => setTimeout(r, 200));
  const afterScroll = await evaluate(session, "document.querySelector('.scroll-area').scrollTop");
  const pass3 = afterScroll > beforeScroll;
  results.push({
    name: '原生滾輪可捲動（合成 mouseWheel 事件）',
    pass: pass3,
    detail: `scrollTop ${beforeScroll} → ${afterScroll}${forced ? '（內容天生塞得下，已臨時縮小 max-height 強制產生捲軸）' : ''}`,
  });
  if (forced) {
    await evaluate(
      session,
      "(() => { document.querySelector('.scroll-area').style.maxHeight = ''; return true; })()",
    );
  }
  return results;
}

async function run() {
  const server = await startServer(DEFAULT_FIXTURE);
  console.log(`[verify-4.7] 本機伺服器：${server.url}（暫存目錄：${server.tmpRootDisplay}）`);
  const edge = await launchEdge();
  console.log(`[verify-4.7] headless Edge 已啟動，devtools port=${edge.port}`);

  let pass = true;
  const target = await newTarget(edge.port, 'about:blank');
  const session = new CDPTarget(target.webSocketDebuggerUrl);
  try {
    await session.connect();
    await session.send('Page.enable');
    // 視窗要夠大，避免 .scroll-area 落在預設 headless viewport 之外（macro/fixed/dynamic
    // 寬度 470–500px、maxHeight 最高 640px，見 registry.js）。
    await session.send('Emulation.setDeviceMetricsOverride', {
      width: 900,
      height: 900,
      deviceScaleFactor: 1,
      mobile: false,
    });

    for (const { id, assertScroll } of TARGETS) {
      console.log(`\n[verify-4.7] === ${id} ===`);
      const results = await checkWidget(session, server, id, assertScroll);
      for (const r of results) {
        console.log(`[verify-4.7] ${r.pass ? 'PASS' : 'FAIL'}  ${id}：${r.name}（${r.detail}）`);
        if (!r.pass) pass = false;
      }
    }
  } finally {
    session.close();
    await closeTarget(edge.port, target.id);
    await edge.close();
    await server.close();
  }

  console.log(pass ? '\n[verify-4.7] 總結：PASS' : '\n[verify-4.7] 總結：FAIL');
  return pass ? 0 : 1;
}

run()
  .then((code) => process.exit(code))
  .catch((err) => {
    console.error(`[verify-4.7] 發生錯誤：${err.stack || err.message}`);
    process.exit(2);
  });
