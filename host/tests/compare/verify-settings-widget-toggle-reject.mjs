#!/usr/bin/env node
// host/tests/compare/verify-settings-widget-toggle-reject.mjs
//
// task 7.4（design.md D7「記錄位置的寫回時機」第 2 點；specs/widget-host-windows「小工具互不
// 重疊」「空間不足」Scenario）：設定視窗 settings.html 把小工具開關打開時，若後端
// `update_settings` 因為「開啟小工具找空位找不到空位」回傳 `Err("空間不足，請先調整版面")`，
// 畫面應顯示這則錯誤訊息並把該開關還原為關閉——不留下「看起來已經開啟、實際上後端設定完全
// 沒變」的畫面。
//
// 這個情境需要 Rust 端真的判斷空間不足，但 fixture 模式（無 Tauri）沒有 Rust 後端可以送出
// 真正的 `Err`；三種結果本身（相交→找到並寫回、相交→找不到拒絕、顯示器不存在→不寫回）已由
// `host/src/widgets.rs` 的 `place_newly_enabled_widgets` 單元測試涵蓋（`cargo test`）。本腳本
// 只驗證「後端回絕時，settings.html 這一層的顯示與還原邏輯對不對」，用 bridge.js 的測試掛鉤
// `window.__bridgeTest.forceUpdateSettingsError`（task 7.4 新增，僅 `!hasTauri` 時存在）讓下
// 一次 `updateSettings()` 拋出指定訊息，模擬真正的後端拒絕，不繞過 settings.html 本身任何
// 邏輯（brief 明文允許「以 CDP 或實機驗證」）。
//
// 不重造 CDP／伺服器骨架，直接沿用 `serve.mjs`／`cdp.mjs`（比照 `verify-quotes-pause.mjs`
// 的 `withPage` 模式）。
//
// 用法：node host/tests/compare/verify-settings-widget-toggle-reject.mjs

import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { startServer } from './serve.mjs';
import { launchEdge, newTarget, closeTarget, CDPTarget, waitForPageCondition, evaluate } from './cdp.mjs';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(__dirname, '..', '..', '..');
const DEFAULT_FIXTURE = path.join(REPO_ROOT, 'host', 'ui', 'fixtures', 'tw-events.json');
const FORCED_MESSAGE = '空間不足，請先調整版面';

async function withSettingsPage(edge, serverUrl, fn) {
  const target = await newTarget(edge.port, 'about:blank');
  const session = new CDPTarget(target.webSocketDebuggerUrl);
  try {
    await session.connect();
    await session.send('Page.enable');
    await session.send('Page.navigate', { url: `${serverUrl}/new/settings.html` });
    // custom1 在 fixtures/settings.json 裡預設 enabled:false，main() 的 renderForm() 掛載後
    // 才會把它的 checkbox 建出來並回填 checked 狀態；用它的存在＋未勾選當「表單已可操作」的
    // 判斷依據。
    await waitForPageCondition(
      session,
      "!!document.getElementById('widget-custom1-enabled') && " +
        "document.getElementById('widget-custom1-enabled').checked === false",
    );
    await new Promise((r) => setTimeout(r, 100)); // 給事件監聽器掛載留緩衝
    return await fn(session);
  } finally {
    session.close();
    await closeTarget(edge.port, target.id);
  }
}

/** 驗證①：後端拒絕時顯示訊息、開關還原為關閉；②同一顆開關之後正常開啟仍會成功（確認還原
 * 動作沒有把控制項弄壞，且 `forceUpdateSettingsError` 用後即清除、不會卡住後續操作）。 */
async function verifyRejectThenNormalSucceeds(edge, serverUrl) {
  const failures = [];
  await withSettingsPage(edge, serverUrl, async (session) => {
    await evaluate(
      session,
      `window.__bridgeTest.forceUpdateSettingsError = ${JSON.stringify(FORCED_MESSAGE)}`,
    );

    // 模擬使用者點擊 custom1 的開關（頁面內部 DOM 事件，非系統層級輸入注入）。
    await evaluate(session, "document.getElementById('widget-custom1-enabled').click()");
    // onchange 內的 patch() 是 async，等狀態文字出現「套用失敗」再檢查。
    await waitForPageCondition(
      session,
      "document.getElementById('status').textContent.includes('套用失敗')",
    );

    const statusText = await evaluate(session, "document.getElementById('status').textContent");
    const statusClass = await evaluate(session, "document.getElementById('status').className");
    const checkedAfterReject = await evaluate(
      session,
      "document.getElementById('widget-custom1-enabled').checked",
    );
    console.log(
      `[verify-settings-widget-toggle-reject] 拒絕後：status="${statusText}" class="${statusClass}" checked=${checkedAfterReject}`,
    );

    if (!statusText.includes(FORCED_MESSAGE)) {
      failures.push(`狀態文字應包含後端錯誤訊息「${FORCED_MESSAGE}」，實際="${statusText}"`);
    }
    if (!statusClass.includes('err')) {
      failures.push(`狀態列應標示為錯誤樣式（class 含 "err"），實際="${statusClass}"`);
    }
    if (checkedAfterReject !== false) {
      failures.push('後端拒絕後開關應還原為關閉（false），實際仍是 true');
    }

    // 確認掛鉤用後即清除：不必再手動清掉，直接再點一次應該正常成功（fixture 模式的
    // updateSettings 正常路徑：合併進記憶體並回傳，不再拋錯）。
    await evaluate(session, "document.getElementById('widget-custom1-enabled').click()");
    await waitForPageCondition(
      session,
      "document.getElementById('status').textContent.includes('已套用')",
    );
    const checkedAfterSuccess = await evaluate(
      session,
      "document.getElementById('widget-custom1-enabled').checked",
    );
    console.log(
      `[verify-settings-widget-toggle-reject] 正常開啟：checked=${checkedAfterSuccess}`,
    );
    if (checkedAfterSuccess !== true) {
      failures.push('掛鉤清除後再次點擊應正常開啟（checked=true），實際仍是 false');
    }
  });
  return failures;
}

/** 驗證③（fix F1，review 7.4 L2）：「關閉」方向失敗也要還原——clock 在 fixture 預設開啟，
 * 點擊（取消勾選）時後端拒絕（例如存檔 I/O 錯誤），開關應回到勾選，不可停在關閉而後端仍開啟。 */
async function verifyCloseRejectRestoresChecked(edge, serverUrl) {
  const failures = [];
  await withSettingsPage(edge, serverUrl, async (session) => {
    const before = await evaluate(session, "document.getElementById('widget-clock-enabled').checked");
    if (before !== true) {
      failures.push(`前提：clock 在 fixture 應預設開啟，實際 checked=${before}`);
      return;
    }
    await evaluate(session, "window.__bridgeTest.forceUpdateSettingsError = '存檔失敗（模擬）'");
    await evaluate(session, "document.getElementById('widget-clock-enabled').click()");
    await waitForPageCondition(
      session,
      "document.getElementById('status').textContent.includes('套用失敗')",
    );
    const after = await evaluate(session, "document.getElementById('widget-clock-enabled').checked");
    console.log(`[verify-settings-widget-toggle-reject] 關閉被拒絕後：checked=${after}`);
    if (after !== true) {
      failures.push(`關閉被後端拒絕後開關應還原為開啟（true），實際 checked=${after}`);
    }
  });
  return failures;
}

async function run() {
  const server = await startServer(DEFAULT_FIXTURE);
  console.log(`[verify-settings-widget-toggle-reject] 本機伺服器：${server.url}（暫存目錄：${server.tmpRootDisplay}）`);
  const edge = await launchEdge();
  console.log(`[verify-settings-widget-toggle-reject] headless Edge 已啟動，devtools port=${edge.port}`);

  let pass;
  try {
    const failures = [
      ...(await verifyRejectThenNormalSucceeds(edge, server.url)),
      ...(await verifyCloseRejectRestoresChecked(edge, server.url)),
    ];
    pass = failures.length === 0;
    console.log(`\n── settings.html：開關被後端拒絕時顯示訊息並還原（開啟與關閉兩個方向）──`);
    if (pass) {
      console.log('  PASS');
    } else {
      console.log(`  FAIL（${failures.length} 項）`);
      for (const f of failures) console.log(`    - ${f}`);
    }
  } finally {
    await edge.close();
    await server.close({});
  }

  if (!pass) {
    process.exitCode = 1;
  }
}

run().catch((err) => {
  console.error('[verify-settings-widget-toggle-reject] 執行失敗：', err);
  process.exitCode = 1;
});
