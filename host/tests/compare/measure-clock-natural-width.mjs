#!/usr/bin/env node
// host/tests/compare/measure-clock-natural-width.mjs
//
// 量測時鐘小工具「內容自然尺寸」，確認放得進最小框（`widgets.rs` 的 `WIDGET_SPECS`，時鐘 212×160）。
// 原為 change widget-adaptive-zoom-and-grid task 2.1 的量測；change clock-six-exchanges 改量六列版面。
//
// 作法：headless Edge 開 `widget.html?w=clock`（fixture 模式，裝置像素比 1、無 ZoomFactor＝倍率 1）：
//   1. 欄位溢出：時間枚舉 00:00–23:59、日期枚舉 2026–2031 每一天的 `YY-MM-DD`，逐一填進每一欄，
//      檢查 scrollWidth 不超過固定欄寬（六列共用同一組欄寬，量第一列即可）。
//   2. 自然尺寸：把 `#widget-root > .panel` 改成依內容收縮，讀 border box 寬高，加上 `#widget-root` 的
//      邊距＝整扇視窗所需的邏輯尺寸；再加 4 px 字型差異餘裕後不得超過最小框。
//   3. 正式版面：視窗設成最小框，不改 CSS，確認面板沒有被裁（scroll 尺寸不超過 client 尺寸）。
//   4. 字型敏感度：強制 body 改用各候選字型重做 1–2。
//
// 用法：node host/tests/compare/measure-clock-natural-width.mjs [--json]
// 結束碼：0＝放得下；1＝超出最小框或欄位溢出。不改任何產品程式碼；暫存目錄與 profile 用完即刪。

import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { startServer } from './serve.mjs';
import { launchEdge, newTarget, closeTarget, CDPTarget, waitForPageCondition, evaluate } from './cdp.mjs';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(__dirname, '..', '..', '..');
const FIXTURE = path.join(REPO_ROOT, 'host', 'ui', 'fixtures', 'tw-events.json');

const MIN_BOX = { w: 212, h: 160 }; // host/src/widgets.rs WIDGET_SPECS 的時鐘最小框
const SAFETY_PX = 4; // 字型差異餘裕
const FONTS = [null, '"Noto Sans TC"', '"Microsoft JhengHei"', 'system-ui', 'sans-serif'];

const asJson = process.argv.includes('--json');

const measureScript = (fontFamily) => `(() => {
  if (${JSON.stringify(fontFamily)}) document.body.style.fontFamily = ${JSON.stringify(fontFamily)};
  const root = document.getElementById('widget-root');
  const panel = root.querySelector('.panel');
  const row = panel.querySelector('.xrow');
  const timeEl = row.querySelector('.xtime');
  const dateEl = row.querySelector('.xdate');
  const codeEls = [...panel.querySelectorAll('.xcode')];
  const pad2 = (n) => String(n).padStart(2, '0');

  // 1. 欄位溢出
  const overflow = [];
  const check = (el, text) => {
    el.textContent = text;
    if (el.scrollWidth > el.clientWidth) overflow.push({ text, scrollW: el.scrollWidth, clientW: el.clientWidth });
  };
  let times = 0;
  for (let h = 0; h < 24; h++) for (let m = 0; m < 60; m++) { check(timeEl, pad2(h) + ':' + pad2(m)); times++; }
  let dates = 0;
  for (let d = new Date(2026, 0, 1); d.getFullYear() <= 2031; d = new Date(d.getFullYear(), d.getMonth(), d.getDate() + 1)) {
    check(dateEl, pad2(d.getFullYear() % 100) + '-' + pad2(d.getMonth() + 1) + '-' + pad2(d.getDate()));
    dates++;
  }
  for (const el of codeEls) if (el.scrollWidth > el.clientWidth) overflow.push({ text: el.textContent, scrollW: el.scrollWidth, clientW: el.clientWidth });

  // 2. 自然尺寸
  const style = document.createElement('style');
  style.textContent = '#widget-root > .panel { flex: none !important; width: max-content !important; align-self: flex-start !important; height: auto !important; }';
  document.head.appendChild(style);
  const r = panel.getBoundingClientRect();
  const cs = getComputedStyle(root);
  const padX = parseFloat(cs.paddingLeft) + parseFloat(cs.paddingRight);
  const padY = parseFloat(cs.paddingTop) + parseFloat(cs.paddingBottom);
  return {
    counts: { times, dates, rows: panel.querySelectorAll('.xrow').length },
    overflow: overflow.slice(0, 10),
    overflowCount: overflow.length,
    panel: { w: r.width, h: r.height },
    window: { w: r.width + padX, h: r.height + padY },
  };
})()`;

const LAYOUT_SCRIPT = `(() => {
  const panel = document.querySelector('#widget-root > .panel');
  const pr = panel.getBoundingClientRect();
  const rows = [...panel.querySelectorAll('.xrow')].map((el) => el.getBoundingClientRect());
  return {
    inner: [innerWidth, innerHeight],
    panel: { clientW: panel.clientWidth, scrollW: panel.scrollWidth, clientH: panel.clientHeight, scrollH: panel.scrollHeight },
    clipped: panel.scrollHeight > panel.clientHeight || panel.scrollWidth > panel.clientWidth,
    rowsInside: rows.every((b) => b.left >= pr.left && b.right <= pr.right && b.top >= pr.top && b.bottom <= pr.bottom),
  };
})()`;

async function openPage(edge, server, { w, h }) {
  const target = await newTarget(edge.port, 'about:blank');
  const session = new CDPTarget(target.webSocketDebuggerUrl);
  await session.connect();
  await session.send('Page.enable');
  await session.send('Emulation.setDeviceMetricsOverride', { width: w, height: h, deviceScaleFactor: 1, mobile: false });
  await session.send('Page.navigate', { url: `${server.url}/new/widget.html?w=clock&fixtures=/fixtures/` });
  await waitForPageCondition(session, "document.querySelectorAll('.xrow').length === 6 && document.querySelector('.xtime').textContent !== '--:--'");
  await new Promise((r) => setTimeout(r, 600));
  return { session, target };
}

async function withPage(edge, server, size, script) {
  const page = await openPage(edge, server, size);
  try {
    return await evaluate(page.session, script);
  } finally {
    page.session.close();
    await closeTarget(edge.port, page.target.id);
  }
}

const server = await startServer(FIXTURE);
const edge = await launchEdge();
const result = { minBox: MIN_BOX, safetyPx: SAFETY_PX, fonts: {} };
try {
  for (const fam of FONTS) {
    result.fonts[fam ?? '(預設)'] = await withPage(edge, server, { w: 1400, h: 600 }, measureScript(fam));
  }
  result.layoutAtMinBox = await withPage(edge, server, MIN_BOX, LAYOUT_SCRIPT);
} finally {
  await edge.close();
  await server.close();
}

const problems = [];
for (const [fam, m] of Object.entries(result.fonts)) {
  if (m.counts.rows !== 6) problems.push(`${fam}：列數 ${m.counts.rows}，應為 6`);
  if (m.overflowCount) problems.push(`${fam}：${m.overflowCount} 個字串超出欄寬，例 ${JSON.stringify(m.overflow[0])}`);
  if (m.window.w + SAFETY_PX > MIN_BOX.w) problems.push(`${fam}：自然寬 ${m.window.w}＋${SAFETY_PX} 超過 ${MIN_BOX.w}`);
  if (m.window.h + SAFETY_PX > MIN_BOX.h) problems.push(`${fam}：自然高 ${m.window.h}＋${SAFETY_PX} 超過 ${MIN_BOX.h}`);
}
if (result.layoutAtMinBox.clipped || !result.layoutAtMinBox.rowsInside) problems.push(`最小框版面被裁：${JSON.stringify(result.layoutAtMinBox)}`);
result.problems = problems;

if (asJson) {
  console.log(JSON.stringify(result, null, 2));
} else {
  console.log(`最小框 ${MIN_BOX.w}×${MIN_BOX.h}，餘裕 ${SAFETY_PX}px（headless Edge，deviceScaleFactor=1）`);
  for (const [fam, m] of Object.entries(result.fonts)) {
    console.log(`${fam}\t面板 ${m.panel.w}×${m.panel.h}\t視窗 ${m.window.w}×${m.window.h}\t枚舉 ${m.counts.times} 時間／${m.counts.dates} 日期\t溢出 ${m.overflowCount}`);
  }
  console.log('最小框版面：', JSON.stringify(result.layoutAtMinBox));
  console.log(problems.length ? `FAIL\n${problems.join('\n')}` : 'PASS');
}
process.exit(problems.length ? 1 : 0);
