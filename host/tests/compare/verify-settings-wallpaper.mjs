#!/usr/bin/env node
// host/tests/compare/verify-settings-wallpaper.mjs
//
// dynamic-wallpaper task 4.8：設定視窗「動態桌布」區塊各狀態的截圖與檢查（fixture 模式，無 Tauri）。
// 狀態由 bridge.js 的測試掛鉤注入（`window.__bridgeTestInit`，以 CDP
// `Page.addScriptToEvaluateOnNewDocument` 在頁面載入前設定）：協調迴圈狀態快照、主題設定檔、本機
// 日期。每個狀態一張截圖寫到 host/tools/evidence/dw-4.8-<名稱>.png，並以 DOM 文字檢查：
//
//   normal               主題「不接管」、無提示：只有主題選單與備份提示（spec「主題為不接管且無提示時
//                        只多出主題選單與備份提示」）
//   waiting-for-data     「等待資料中」（spec「首次安裝無資料時不接管」）
//   spotlight-dialog     點選某主題 → 焦點確認對話（spec「原桌布為 Windows 焦點時先提醒」）
//   spotlight-cancelled  對話按取消 → 主題維持「不接管」、送出的答案是 false
//   spotlight-awaiting   協調迴圈自己在等確認 → 開頁即顯示對話
//   holiday-expiring     本機日期 12/05、休市表缺明年 → 「請更新明年的休市表」
//   holiday-not-yet      本機日期 12/01 → 不提示
//   backup-hint          常駐備份提示（各狀態都檢查；另截主題選單＋提示的特寫）
//   theme-<id>           各主題選中
//
// 用法：node host/tests/compare/verify-settings-wallpaper.mjs
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
const TAG = '[verify-settings-wallpaper]';

/** 設定視窗的預設大小（tray.rs `create_settings_window`：560×760）。 */
const WIDTH = 560;
const HEIGHT = 760;

const THEMES = ['none', 'ridgeline', 'tearoff', 'astrolabe', 'contour', 'skyline'];
const BACKUP_TEXT = '記住我的喜好設定';
const SPOTLIGHT_TEXT = '停止接管時無法自動切回 Windows 焦點，需要到 Windows 設定手動切回';

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
    await waitForPageCondition(
      session,
      "!!document.getElementById('wallpaper-section') && !!document.getElementById('autostart') && " +
        "document.querySelector('input[name=wallpaper_theme]:checked') !== null",
    );
    // 第一次 refreshWallpaper（含載入內建預設設定檔）完成。
    await waitForPageCondition(session, 'window.__bridgeTest && window.__settingsWallpaperReady === true');
    return await fn(session);
  } finally {
    session.close();
    await closeTarget(edge.port, target.id);
  }
}

async function screenshot(session, name, clip) {
  const params = { format: 'png' };
  if (clip) params.clip = { ...clip, scale: 1 };
  const { data } = await session.send('Page.captureScreenshot', params);
  const file = path.join(EVIDENCE_DIR, `dw-4.8-${name}.png`);
  await writeFile(file, Buffer.from(data, 'base64'));
  console.log(`${TAG} 截圖 ${path.relative(REPO_ROOT, file)}`);
  return file;
}

const text = (session, sel) =>
  evaluate(session, `(document.querySelector(${JSON.stringify(sel)})?.textContent ?? '')`);

async function common(session, failures, label) {
  const backup = await text(session, '#backup-hint');
  if (!backup.includes(BACKUP_TEXT)) failures.push(`${label}：備份提示不見了（「${backup}」）`);
  const themes = await evaluate(
    session,
    "[...document.querySelectorAll('input[name=wallpaper_theme]')].map((r) => r.value).join(',')",
  );
  if (themes !== THEMES.join(',')) failures.push(`${label}：主題選單順序不對（${themes}）`);
}

const scenarios = [
  {
    name: 'normal',
    init: {},
    async check(session, f) {
      const notices = await evaluate(session, "document.getElementById('wallpaper-notices').children.length");
      if (notices !== 0) f.push(`normal：不應有提示，實際 ${notices} 則`);
      const sections = await evaluate(session, "[...document.querySelectorAll('main section h2')].map((h) => h.textContent).join('|')");
      if (sections !== '動態桌布|外觀|內容|資料|十個小工具|暫停規則|啟動|資料來源') f.push(`normal：區塊清單不對（${sections}）`);
      const checked = await evaluate(session, "document.querySelector('input[name=wallpaper_theme]:checked').value");
      if (checked !== 'none') f.push(`normal：預設主題應為 none，實際 ${checked}`);
      await screenshot(session, 'normal');
      await screenshot(session, 'backup-hint', await sectionClip(session));
    },
  },
  {
    name: 'waiting-for-data',
    init: {
      wallpaperStatus: { state: 'waiting_for_data', waiting_for: '加權指數日 K' },
      settingsOverride: { wallpaper_theme: 'skyline' },
    },
    async check(session, f) {
      const t = await text(session, '#wallpaper-notices');
      if (!t.startsWith('等待資料中') || !t.includes('加權指數日 K')) f.push(`waiting-for-data：提示不對（「${t}」）`);
      await screenshot(session, 'waiting-for-data');
    },
  },
  {
    name: 'spotlight-dialog',
    init: { wallpaperStatus: { spotlight_check_needed: true } },
    async check(session, f) {
      await evaluate(session, "document.getElementById('wallpaper-theme-astrolabe').click()");
      await waitForPageCondition(session, "document.getElementById('spotlight-dialog').open === true");
      const t = await text(session, '#spotlight-dialog');
      if (!t.includes(SPOTLIGHT_TEXT)) f.push(`spotlight-dialog：對話缺 spec 文字（「${t}」）`);
      const sel = await evaluate(session, 'JSON.stringify(window.__bridgeTest.lastThemeSelection)');
      if (sel !== JSON.stringify({ theme: 'astrolabe', spotlightAnswer: null })) {
        f.push(`spotlight-dialog：第一次送出應不帶答案（${sel}）`);
      }
      await screenshot(session, 'spotlight-dialog');
      // 取消：主題維持「不接管」，第二次送出帶 false。
      await evaluate(session, "document.getElementById('spotlight-cancel').click()");
      await waitForPageCondition(session, "document.getElementById('status').textContent.includes('已取消')");
      const after = await evaluate(session, "document.querySelector('input[name=wallpaper_theme]:checked').value");
      if (after !== 'none') f.push(`spotlight-cancelled：取消後主題應維持 none，實際 ${after}`);
      const sel2 = await evaluate(session, 'JSON.stringify(window.__bridgeTest.lastThemeSelection)');
      if (sel2 !== JSON.stringify({ theme: 'astrolabe', spotlightAnswer: false })) {
        f.push(`spotlight-cancelled：取消應送 false（${sel2}）`);
      }
      await screenshot(session, 'spotlight-cancelled');
    },
  },
  {
    // 修正輪 1（審查 medium）：主題已是天際線（尚未接管）時改選等高線並在提示中取消 → 存成「不接管」；
    // 之後協調迴圈的「等確認」旗標晚一步才更新，輪詢也不得再開一次後備對話。
    name: 'spotlight-cancel-from-theme',
    init: {
      wallpaperStatus: { spotlight_check_needed: true },
      settingsOverride: { wallpaper_theme: 'skyline' },
    },
    async check(session, f) {
      await evaluate(session, "document.getElementById('wallpaper-theme-contour').click()");
      await waitForPageCondition(session, "document.getElementById('spotlight-dialog').open === true");
      // 協調迴圈此時發佈了舊主題的「等確認」（快照落後於使用者的取消）。
      await evaluate(session, 'window.__bridgeTest.wallpaperStatus = { awaiting_spotlight_confirmation: true }');
      await evaluate(session, "document.getElementById('spotlight-cancel').click()");
      await waitForPageCondition(session, "document.getElementById('status').textContent.includes('已取消')");
      const after = await evaluate(session, "document.querySelector('input[name=wallpaper_theme]:checked').value");
      if (after !== 'none') f.push(`spotlight-cancel-from-theme：取消後主題應改為 none，實際 ${after}`);
      const statusText = await text(session, '#status');
      if (!statusText.includes('不接管')) f.push(`spotlight-cancel-from-theme：狀態列應寫「不接管」（「${statusText}」）`);
      await screenshot(session, 'spotlight-cancel-from-theme');
      // 旗標還沒更新的這次輪詢（3 秒）期間不得再開對話。
      await new Promise((r) => setTimeout(r, 3600));
      const reopened = await evaluate(session, "document.getElementById('spotlight-dialog').open");
      if (reopened) f.push('spotlight-cancel-from-theme：剛取消就又開了後備對話');
      if (await evaluate(session, 'window.__bridgeTest.lastSpotlightResponse !== null')) {
        f.push('spotlight-cancel-from-theme：不應送出後備路徑的答案');
      }
    },
  },
  {
    name: 'spotlight-confirmed',
    init: { wallpaperStatus: { spotlight_check_needed: true } },
    async check(session, f) {
      await evaluate(session, "document.getElementById('wallpaper-theme-contour').click()");
      await waitForPageCondition(session, "document.getElementById('spotlight-dialog').open === true");
      await evaluate(session, "document.getElementById('spotlight-confirm').click()");
      await waitForPageCondition(session, "document.getElementById('status').textContent.includes('已套用')");
      const after = await evaluate(session, "document.querySelector('input[name=wallpaper_theme]:checked').value");
      if (after !== 'contour') f.push(`spotlight-confirmed：確認後主題應為 contour，實際 ${after}`);
      const sel = await evaluate(session, 'JSON.stringify(window.__bridgeTest.lastThemeSelection)');
      if (sel !== JSON.stringify({ theme: 'contour', spotlightAnswer: true })) f.push(`spotlight-confirmed：應送 true（${sel}）`);
    },
  },
  {
    name: 'spotlight-awaiting',
    init: {
      wallpaperStatus: { state: 'awaiting_spotlight_confirmation', awaiting_spotlight_confirmation: true },
      settingsOverride: { wallpaper_theme: 'astrolabe' },
    },
    async check(session, f) {
      await waitForPageCondition(session, "document.getElementById('spotlight-dialog').open === true");
      await screenshot(session, 'spotlight-awaiting');
      await evaluate(session, "document.getElementById('spotlight-confirm').click()");
      await waitForPageCondition(session, 'window.__bridgeTest.lastSpotlightResponse === true');
      const open = await evaluate(session, "document.getElementById('spotlight-dialog').open");
      if (open) f.push('spotlight-awaiting：回答後對話應關閉');
    },
  },
  {
    name: 'holiday-expiring',
    init: { today: '2026-12-05' },
    async check(session, f) {
      const t = await text(session, '#wallpaper-notices');
      if (!t.includes('請更新明年的休市表') || !t.includes('wallpaper-config.json')) {
        f.push(`holiday-expiring：提示不對（「${t}」）`);
      }
      await screenshot(session, 'holiday-expiring');
    },
  },
  {
    name: 'holiday-not-yet',
    init: { today: '2026-12-01' },
    async check(session, f) {
      const t = await text(session, '#wallpaper-notices');
      if (t.includes('休市表')) f.push(`holiday-not-yet：12/01 不應提示（「${t}」）`);
    },
  },
  ...THEMES.map((id) => ({
    name: `theme-${id}`,
    init: { settingsOverride: { wallpaper_theme: id } },
    async check(session, f) {
      const checked = await evaluate(session, "document.querySelector('input[name=wallpaper_theme]:checked').value");
      if (checked !== id) f.push(`theme-${id}：選中的是 ${checked}`);
      await screenshot(session, `theme-${id}`, await sectionClip(session));
    },
  })),
];

/** 「動態桌布」區塊的截圖範圍。 */
async function sectionClip(session) {
  const r = JSON.parse(
    await evaluate(
      session,
      "JSON.stringify((({x, y, width, height}) => ({x, y, width, height}))(document.getElementById('wallpaper-section').getBoundingClientRect()))",
    ),
  );
  return { x: Math.max(0, r.x - 8), y: Math.max(0, r.y - 8), width: r.width + 16, height: r.height + 16 };
}

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
