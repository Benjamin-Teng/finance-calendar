#!/usr/bin/env node
// host/tests/compare/verify-settings-fetch-status.mjs
//
// data-layer-rust task 5.5：設定視窗底部「資料來源」區塊（來源清單、授權條款網址、抓取狀態）的
// headless 截圖與 DOM 檢查（fixture 模式，無 Tauri；不啟動宿主、不碰桌面）。抓取狀態由 bridge.js 的
// 測試掛鉤注入（`window.__bridgeTestInit.fetchStatus`／`fetchStatusError`，以 CDP
// `Page.addScriptToEvaluateOnNewDocument` 在頁面載入前設定）。每個情境一張截圖寫到
// host/tools/evidence/data-layer-5.5-<名稱>.png：
//
//   active-ok        啟用、上次更新時間、沒有失敗（另截整頁，確認不擠壓既有區塊）
//   active-failed    啟用、上次更新時間＋「（部分來源失敗）」
//   active-round-failed 啟用、整輪沒有成功（無來源成功或寫檔失敗）→ 「（本輪未成功更新）」
//   off-by-setting   「未抓取：已在設定關閉」＋最後一次更新
//   off-isolated     「未抓取：偵測到隔離環境…」＋最後一次更新、兩個路徑、設定檔路徑
//   query-failed     查詢指令失敗 → 顯示「查詢失敗」、其餘區塊照常
//
// 用法：node host/tests/compare/verify-settings-fetch-status.mjs
// headless Edge 由 cdp.mjs 以獨立 --user-data-dir 啟動，只關閉自己啟動的那一個。

import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { startServer } from './serve.mjs';
import { launchEdge, newTarget, closeTarget, CDPTarget, waitForPageCondition, evaluate } from './cdp.mjs';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(__dirname, '..', '..', '..');
const DEFAULT_FIXTURE = path.join(REPO_ROOT, 'host', 'ui', 'fixtures', 'tw-events.json');
const EVIDENCE_DIR = path.join(REPO_ROOT, 'host', 'tools', 'evidence');
const TAG = '[verify-settings-fetch-status]';

/** 設定視窗的預設大小（tray.rs `create_settings_window`：560×760）。 */
const WIDTH = 560;
const HEIGHT = 760;

const SOURCES = '臺灣證券交易所（依政府資料開放授權條款第 1 版）|證券櫃檯買賣中心（依政府資料開放授權條款第 1 版）|公開資訊觀測站|ForexFactory|Yahoo Finance|紐約聯邦準備銀行';
const LICENSE_URL = 'https://data.gov.tw/license';
const SECTIONS_BEFORE = '動態桌布|外觀|內容|資料|十個小工具|暫停規則|啟動';

async function withPage(edge, serverUrl, init, fn) {
  const target = await newTarget(edge.port, 'about:blank');
  const session = new CDPTarget(target.webSocketDebuggerUrl);
  try {
    await session.connect();
    await session.send('Page.enable');
    await session.send('Emulation.setDeviceMetricsOverride', {
      width: WIDTH,
      height: HEIGHT,
      deviceScaleFactor: 1,
      mobile: false,
    });
    await session.send('Page.addScriptToEvaluateOnNewDocument', {
      source: `window.__bridgeTestInit = ${JSON.stringify(init)};`,
    });
    await session.send('Page.navigate', { url: `${serverUrl}/new/settings.html` });
    await waitForPageCondition(session, 'window.__bridgeTest && window.__settingsFetchReady === true');
    return await fn(session);
  } finally {
    session.close();
    await closeTarget(edge.port, target.id);
  }
}

async function screenshot(session, name, clip, beyond = false) {
  const params = { format: 'png' };
  if (clip) params.clip = { ...clip, scale: 1 };
  if (beyond) params.captureBeyondViewport = true;
  const { data } = await session.send('Page.captureScreenshot', params);
  const file = path.join(EVIDENCE_DIR, `data-layer-5.5-${name}.png`);
  await writeFile(file, Buffer.from(data, 'base64'));
  console.log(`${TAG} 截圖 ${path.relative(REPO_ROOT, file)}`);
}

const text = (session, sel) =>
  evaluate(session, `(document.querySelector(${JSON.stringify(sel)})?.textContent ?? '')`);

/** 捲到最底，回傳「資料來源」區塊的文件座標矩形（含邊距；captureScreenshot 的 clip 是文件座標），供截圖。 */
async function scrollToSources(session) {
  await evaluate(session, "document.getElementById('sources-section').scrollIntoView({ block: 'end' })");
  const r = JSON.parse(
    await evaluate(
      session,
      "JSON.stringify((({x, y, width, height}) => ({x: x + window.scrollX, y: y + window.scrollY, width, height}))(document.getElementById('sources-section').getBoundingClientRect()))",
    ),
  );
  return { x: Math.max(0, r.x - 8), y: Math.max(0, r.y - 8), width: r.width + 16, height: r.height + 16 };
}

/** 每個情境都要成立的共通檢查：區塊在最底、來源清單與授權網址照 spec、版面不擠壓。 */
async function common(session, failures, label) {
  const headings = await evaluate(
    session,
    "[...document.querySelectorAll('main section h2')].map((h) => h.textContent).join('|')",
  );
  if (headings !== `${SECTIONS_BEFORE}|資料來源`) failures.push(`${label}：區塊清單不對（${headings}）`);

  const sources = await evaluate(
    session,
    "[...document.querySelectorAll('#source-list li')].map((li) => li.textContent).join('|')",
  );
  if (sources !== SOURCES) failures.push(`${label}：資料來源清單不對（${sources}）`);

  const license = await text(session, '#license-line');
  if (!license.includes(LICENSE_URL)) failures.push(`${label}：缺授權條款網址（「${license}」）`);

  // 區塊完整在文件內、沒有橫向溢出、其他區塊的相對順序沒變（資料來源是最後一個 section）。
  const geom = JSON.parse(
    await evaluate(
      session,
      `JSON.stringify({
        overflowX: document.documentElement.scrollWidth - document.documentElement.clientWidth,
        last: document.querySelector('main section:last-of-type').id,
        sourcesBottom: document.getElementById('sources-section').getBoundingClientRect().bottom + window.scrollY,
        docHeight: document.documentElement.scrollHeight,
      })`,
    ),
  );
  if (geom.overflowX > 0) failures.push(`${label}：出現橫向溢出 ${geom.overflowX}px`);
  if (geom.last !== 'sources-section') failures.push(`${label}：資料來源不在最底部（最後是 ${geom.last}）`);
  if (geom.sourcesBottom > geom.docHeight) failures.push(`${label}：區塊超出文件高度`);
}

const ACTIVE = {
  kind: 'active',
  last_start: '2026-10-05 15:02',
  last_finish: '2026-10-05 15:03',
  source_failed: false,
  isolation: null,
};

const scenarios = [
  {
    name: 'active-ok',
    init: { fetchStatus: ACTIVE },
    async check(session, f) {
      const t = await text(session, '#fetch-status-text');
      if (t !== '上次更新：2026-10-05 15:03') f.push(`active-ok：狀態文字不對（「${t}」）`);
      const cls = await evaluate(session, "document.getElementById('fetch-status').className");
      if (!cls.includes('ok')) f.push(`active-ok：class 應含 ok（${cls}）`);
      // 整頁截圖（確認不擠壓既有區塊）。
      const h = await evaluate(session, 'document.documentElement.scrollHeight');
      await screenshot(session, 'full-page', { x: 0, y: 0, width: WIDTH, height: h }, true);
      await screenshot(session, 'active-ok', await scrollToSources(session));
    },
  },
  {
    name: 'active-failed',
    init: { fetchStatus: { ...ACTIVE, source_failed: true } },
    async check(session, f) {
      const t = await text(session, '#fetch-status-text');
      if (t !== '上次更新：2026-10-05 15:03（部分來源失敗）') f.push(`active-failed：狀態文字不對（「${t}」）`);
      await screenshot(session, 'active-failed', await scrollToSources(session));
    },
  },
  {
    name: 'active-round-failed',
    init: { fetchStatus: { ...ACTIVE, source_failed: true, round_failed: true } },
    async check(session, f) {
      const t = await text(session, '#fetch-status-text');
      if (t !== '上次更新：2026-10-05 15:03（本輪未成功更新）') f.push(`active-round-failed：狀態文字不對（「${t}」）`);
      await screenshot(session, 'active-round-failed', await scrollToSources(session));
    },
  },
  {
    name: 'off-by-setting',
    init: { fetchStatus: { ...ACTIVE, kind: 'off_by_setting' } },
    async check(session, f) {
      const t = await text(session, '#fetch-status-text');
      if (t !== '未抓取：已在設定關閉') f.push(`off-by-setting：狀態文字不對（「${t}」）`);
      const detail = await text(session, '#fetch-status .fetch-detail');
      if (detail !== '最後一次更新：2026-10-05 15:03') f.push(`off-by-setting：應補充最後一次更新（「${detail}」）`);
      if ((await text(session, '#fetch-status-text')).includes('上次更新')) f.push('off-by-setting：主文字不應是上次更新');
      await screenshot(session, 'off-by-setting', await scrollToSources(session));
    },
  },
  {
    name: 'off-isolated',
    init: {
      fetchStatus: {
        kind: 'off_isolated',
        last_start: '2026-10-05 15:02',
        last_finish: '2026-10-05 15:03',
        source_failed: false,
        settings_path: 'C:\\Users\\Ben\\AppData\\Roaming\\tw.fintools.fc-host\\settings.json',
        isolation: { env_value: 'C:\\Temp\\iso\\Local', registered: 'C:\\Users\\Ben\\AppData\\Local' },
      },
    },
    async check(session, f) {
      const t = await text(session, '#fetch-status-text');
      const want = '未抓取：偵測到隔離環境（LOCALAPPDATA 與系統登記不同），可在設定檔將 data_fetch 設為 "on"，改完後重新啟動宿主';
      if (t !== want) f.push(`off-isolated：狀態文字不對（「${t}」）`);
      const detail = await evaluate(
        session,
        "[...document.querySelectorAll('#fetch-status .fetch-detail')].map((d) => d.textContent).join('|')",
      );
      const wantDetail =
        '最後一次更新：2026-10-05 15:03|LOCALAPPDATA：C:\\Temp\\iso\\Local|系統登記：C:\\Users\\Ben\\AppData\\Local|' +
        '設定檔：C:\\Users\\Ben\\AppData\\Roaming\\tw.fintools.fc-host\\settings.json';
      if (detail !== wantDetail) {
        f.push(`off-isolated：路徑補充行不對（「${detail}」）`);
      }
      await screenshot(session, 'off-isolated', await scrollToSources(session));
    },
  },
  {
    name: 'query-failed',
    init: { fetchStatusError: 'boom' },
    async check(session, f) {
      const t = await text(session, '#fetch-status-text');
      if (!t.startsWith('抓取狀態：查詢失敗')) f.push(`query-failed：應顯示查詢失敗（「${t}」）`);
      await screenshot(session, 'query-failed', await scrollToSources(session));
    },
  },
];

async function run() {
  await mkdir(EVIDENCE_DIR, { recursive: true });
  const server = await startServer(DEFAULT_FIXTURE);
  console.log(`${TAG} 本機伺服器：${server.url}（暫存目錄：${server.tmpRootDisplay}）`);
  const edge = await launchEdge();
  console.log(`${TAG} headless Edge 已啟動，devtools port=${edge.port}`);
  const failures = [];
  try {
    for (const s of scenarios) {
      const before = failures.length;
      await withPage(edge, server.url, s.init, async (session) => {
        await common(session, failures, s.name);
        await s.check(session, failures);
      });
      console.log(`${TAG} ${s.name}：${failures.length === before ? 'PASS' : 'FAIL'}`);
    }
  } finally {
    await edge.close();
    await server.close({});
  }
  for (const f of failures) console.log(`${TAG} FAIL ${f}`);
  console.log(`${TAG} VERDICT ${failures.length === 0 ? 'PASS' : 'FAIL'}（${scenarios.length} 個情境）`);
  if (failures.length) process.exitCode = 1;
}

run().catch((err) => {
  console.error(`${TAG} 執行失敗：`, err);
  process.exitCode = 1;
});
