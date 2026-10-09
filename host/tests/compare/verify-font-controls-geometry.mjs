#!/usr/bin/env node
// host/tests/compare/verify-font-controls-geometry.mjs
//
// 編輯版面字級控制的幾何檢查（widget-font-scale-per-widget task 4.1 修正輪；design.md Risks「控制
// 不擋面板標題文字」）。headless Edge 開 `widget.html?w=<id>`（fixture 模式、deviceScaleFactor 1、
// 無 ZoomFactor：CSS viewport＝下列寬高），進入編輯版面（解鎖），在頁面內量：
//   - 清單類（macro／fixed／dynamic／custom1）：`.font-row`（按鈕列）與 `.panel > header > .ttl`
//     的 bounding rect 不相交；寬度取各自的最小寬（WIDGET_SPECS 的 min_width）與設計寬
//     （comfort_width），高度取最小框高。
//   - 時鐘：`.font-row` 與 #clockTime／#clockDate／#weekRange 不相交。三者是整列寬的 block，
//     量元素框＝行盒（水平方向比文字本身嚴格）；不用 Range rect——它取字型的 ascent＋descent
//     （Noto Sans TC 約 1.45em），比 line-height 1.1 的行盒高出一截，那段是空白、不是字形。
//   - 達上限（at_cap）且視窗夠高時另量內嵌提示 `.font-hint`：顯示時不得與 `.ttl`、面板內標題列
//     以外的內容、時鐘文字相交；不顯示時必須是真的放不下（Codex 審查 4ec15c7）——在目前版面把提示
//     暫時顯示出來量，不相交卻沒顯示＝誤藏。時鐘 212×320 與清單設計寬×320 的 at_cap 明確要求顯示。
//   - 提示的碰撞判斷必須量「控制已顯示」的版面（Codex 審查 4ec15c7）：頁面載入前包住
//     `getBoundingClientRect`，記下每次量 `.font-hint` 當下 body 有沒有 `font-controls-shown`；
//     首次進入解鎖編輯版面時若在 class 生效前量測（量到標題沒讓位、時鐘沒保留空間的平時版面）即失敗。
//   - 鎖定狀態（Codex 審查 8d24acc）：同一頁依序「平時→鎖定編輯→解鎖編輯→再鎖定」，鎖定編輯
//     （有 body.edit-mode＋edit-locked、沒有控制）時標題框、標題列、副標題可見性、時鐘內距／對齊／
//     時間框都必須與平時完全相同，且沒有 `.font-row`／`body.font-controls-shown`；解鎖時兩者都出現、
//     上面的不相交檢查成立。
// 行情條只有一行、控制避不開，已裁示接受，不在檢查範圍。
// 倍率框尺寸從 host/src/widgets.rs 讀（`widgetZoomBoxes`，擴充插槽另解析 `custom_spec`），不寫死。
//
// 用法：node host/tests/compare/verify-font-controls-geometry.mjs [--assert]
// `--assert`：任一情境相交時 exit 1。

import path from 'node:path';
import { readFileSync } from 'node:fs';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { startServer } from './serve.mjs';
import { launchEdge, newTarget, closeTarget, CDPTarget, waitForPageCondition, evaluate } from './cdp.mjs';
import { widgetZoomBoxes, widgetGapCssPx } from './verify-visual-edges.mjs';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(__dirname, '..', '..', '..');
const FIXTURE = path.join(REPO_ROOT, 'host', 'ui', 'fixtures', 'tw-events.json');

/** 擴充插槽的倍率框：`custom_spec` 內的 `list_box(min_width, panel_min_height, comfort_width)`。 */
export function customZoomBox() {
  const rs = readFileSync(path.join(REPO_ROOT, 'host', 'src', 'widgets.rs'), 'utf8');
  const fn = rs.slice(rs.indexOf('const fn custom_spec('));
  const m = fn.match(/list_box\(\s*([\d.]+),\s*([\d.]+),\s*([\d.]+)\s*\)/);
  if (!m) throw new Error('widgets.rs 的 custom_spec 找不到 list_box(…)');
  const gap = widgetGapCssPx();
  const minHeight = Number(m[2]) + 2 * gap;
  return { minWidth: Number(m[1]), minHeight, comfortWidth: Number(m[3]), comfortHeight: minHeight * 1.5 };
}

export function cases() {
  const boxes = { ...widgetZoomBoxes(), custom1: customZoomBox() };
  const out = [];
  for (const id of ['macro', 'fixed', 'dynamic', 'custom1']) {
    const b = boxes[id];
    for (const [label, w] of [
      ['min', b.minWidth],
      ['design', b.comfortWidth],
    ]) {
      const width = Math.ceil(w);
      out.push({ id, label, w: width, h: Math.ceil(b.minHeight), font: { font_scale: 1, at_cap: false } });
      // 夠高才會顯示內嵌提示（widget.html FONT_HINT_MIN_HEIGHT），另量一次 at_cap。
      // 設計寬時標題列右側有空間：提示必須顯示（expectHint）。
      out.push({
        id,
        label: `${label}-atcap-tall`,
        w: width,
        h: 320,
        font: { font_scale: 1, at_cap: true },
        expectHint: label === 'design',
      });
    }
  }
  const c = boxes.clock;
  out.push({ id: 'clock', label: 'min', w: Math.ceil(c.minWidth), h: Math.ceil(c.minHeight), font: { font_scale: 1, at_cap: false } });
  out.push({ id: 'clock', label: 'min-atcap', w: Math.ceil(c.minWidth), h: Math.ceil(c.minHeight), font: { font_scale: 1, at_cap: true } });
  out.push({ id: 'clock', label: 'atcap-tall', w: Math.ceil(c.minWidth), h: 320, font: { font_scale: 1, at_cap: true }, expectHint: true });
  return out;
}

/** widget.html 的 `FONT_HINT_MIN_HEIGHT`：視窗低於此高度時提示依設計不顯示（不算誤藏）。 */
export function hintMinHeight() {
  const html = readFileSync(path.join(REPO_ROOT, 'host', 'ui', 'widget.html'), 'utf8');
  const m = html.match(/const FONT_HINT_MIN_HEIGHT = (\d+);/);
  if (!m) throw new Error('widget.html 找不到 FONT_HINT_MIN_HEIGHT');
  return Number(m[1]);
}

// 頁面載入前注入：記錄每次量 `.font-hint` 當下 body 是否已有 font-controls-shown（見檔頭）。
const HINT_MEASURE_PROBE = `(() => {
  window.__hintMeasures = [];
  const orig = Element.prototype.getBoundingClientRect;
  Element.prototype.getBoundingClientRect = function () {
    if (this.classList && this.classList.contains('font-hint')) {
      window.__hintMeasures.push(document.body.classList.contains('font-controls-shown'));
    }
    return orig.call(this);
  };
})();`;

const EDIT = `(async () => {
  const bridge = await import('/new/bridge.js');
  const s = await bridge.getSettings();
  window.__bridgeTest.emit('settings', Object.assign({}, s, { layout_locked: false }));
  window.__bridgeTest.emit('edit-mode', true);
  return true;
})()`;

// 頁面內量測：回傳 { problems: [...], detail }。`opts`：`atCap`／`minHeight`（提示「該不該顯示」的
// 檢查只在 at_cap 且視窗不低於 FONT_HINT_MIN_HEIGHT 時做）、`expectHint`（此情境必須顯示提示）。
const measureExpr = (opts = {}) => `((opts) => {
  const problems = [];
  // 先讀探針（之後本函式自己量提示也會記錄，那些都在 class 生效後）。
  const early = (window.__hintMeasures || []).filter((shown) => !shown).length;
  if (early > 0) problems.push('.font-hint 在 body.font-controls-shown 生效前被量測 ' + early + ' 次（量到平時版面）');
  const box = (el) => { const r = el.getBoundingClientRect(); return { l: r.left, t: r.top, r: r.right, b: r.bottom }; };
  const hit = (a, b) => a.l < b.r && b.l < a.r && a.t < b.b && b.t < a.b && a.r > a.l && b.r > b.l;
  const fmt = (x) => '[' + [x.l, x.t, x.r, x.b].map((v) => Math.round(v * 10) / 10).join(',') + ']';
  const row = document.querySelector('.font-row');
  if (!row) return { problems: ['找不到 .font-row（未進入編輯版面？）'], detail: {} };
  const rowBox = box(row);
  const hintEl = document.querySelector('.font-hint');
  const hintShown = !!hintEl && getComputedStyle(hintEl).display !== 'none' && hintEl.textContent !== '';
  const hintBox = hintShown ? box(hintEl) : null;
  const targets = [];
  const ttl = document.querySelector('.panel > header > .ttl');
  if (ttl) targets.push(['.ttl', box(ttl), true]);
  for (const sel of ['#clockTime', '#clockDate', '#weekRange']) {
    const el = document.querySelector(sel);
    if (el) targets.push([sel, box(el), true]);
  }
  // 提示另外不得壓到面板內標題列以外的內容（清單捲動區、頁尾等）。
  const panel = document.querySelector('#widget-root > .panel');
  if (panel && ttl) {
    for (const child of panel.children) {
      if (child.tagName !== 'HEADER') targets.push(['panel>' + (child.className || child.tagName), box(child), false]);
    }
  }
  for (const [name, t, vsRow] of targets) {
    if (vsRow && hit(rowBox, t)) problems.push('.font-row ' + fmt(rowBox) + ' 與 ' + name + ' ' + fmt(t) + ' 相交');
    if (hintBox && hit(hintBox, t)) problems.push('.font-hint ' + fmt(hintBox) + ' 與 ' + name + ' ' + fmt(t) + ' 相交');
  }
  const detail = { viewport: [innerWidth, innerHeight], row: fmt(rowBox), hintShown, hint: hintBox && fmt(hintBox) };
  // 沒顯示提示時，在目前版面把它暫時顯示出來量：不與任何受保護區域相交＝有空間卻被藏起來。
  if (!hintShown && hintEl && opts.atCap && innerHeight >= opts.minHeight) {
    const inc = document.querySelector('.font-inc');
    const saved = [hintEl.style.display, hintEl.textContent];
    hintEl.style.display = '';
    hintEl.textContent = inc ? inc.title : '';
    const forced = box(hintEl);
    [hintEl.style.display, hintEl.textContent] = saved;
    const blocked = targets.filter(([, t]) => hit(forced, t)).map(([name]) => name);
    detail.hiddenBecause = blocked;
    if (blocked.length === 0) problems.push('at_cap 提示有空間 ' + fmt(forced) + ' 卻沒有顯示');
  }
  if (opts.expectHint && !hintShown) problems.push('此情境（空間足夠）at_cap 提示必須顯示');
  if (ttl) {
    detail.ttl = fmt(box(ttl));
    detail.ttlTruncated = ttl.scrollWidth > ttl.clientWidth;
  }
  return { problems, detail };
})(${JSON.stringify(opts)})`;

async function measureCase(edge, server, c) {
  const target = await newTarget(edge.port, 'about:blank');
  const session = new CDPTarget(target.webSocketDebuggerUrl);
  try {
    await session.connect();
    await session.send('Page.enable');
    await session.send('Emulation.setDeviceMetricsOverride', { width: c.w, height: c.h, deviceScaleFactor: 1, mobile: false });
    await session.send('Page.addScriptToEvaluateOnNewDocument', {
      source: `window.__bridgeTestInit = { widgetFontState: ${JSON.stringify(c.font)} };\n${HINT_MEASURE_PROBE}`,
    });
    await session.send('Page.navigate', { url: `${server.url}/new/widget.html?w=${c.id}&fixtures=/fixtures/` });
    await waitForPageCondition(session, "document.getElementById('widget-root').children.length > 0");
    await evaluate(session, 'document.fonts ? document.fonts.ready.then(() => true) : true');
    await new Promise((r) => setTimeout(r, 300));
    await evaluate(session, EDIT);
    await waitForPageCondition(session, "!!document.querySelector('.font-row')");
    await new Promise((r) => setTimeout(r, 150));
    return await evaluate(
      session,
      measureExpr({ atCap: c.font.at_cap, minHeight: hintMinHeight(), expectHint: !!c.expectHint }),
    );
  } finally {
    session.close();
    await closeTarget(edge.port, target.id);
  }
}

// ── 鎖定狀態（Codex 審查 8d24acc）：鎖定中的編輯版面有 body.edit-mode 但沒有控制，標題、副標題、
// 時鐘版面必須與平時完全相同；鎖定→解鎖→鎖定切換時跟著控制出現／消失。 ─────────────────────
const SET_STATE = (locked, editMode) => `(async () => {
  const bridge = await import('/new/bridge.js');
  const s = await bridge.getSettings();
  window.__bridgeTest.emit('settings', Object.assign({}, s, { layout_locked: ${locked} }));
  window.__bridgeTest.emit('edit-mode', ${editMode});
  return true;
})()`;

// 與「控制是否存在」相關的版面快照：標題框、標題列高、副標題可見性、時鐘內距與對齊。
const SNAPSHOT = `JSON.stringify((() => {
  const r = (el) => { if (!el) return null; const b = el.getBoundingClientRect(); return [b.left, b.top, b.right, b.bottom].map((v) => Math.round(v * 10) / 10); };
  const cs = (el, k) => (el ? getComputedStyle(el)[k] : null);
  const ttl = document.querySelector('.panel > header > .ttl');
  const sub = document.querySelector('.panel > header > .sub');
  const clock = document.querySelector('.clockcard');
  return {
    ttl: r(ttl),
    ttlTruncated: ttl ? ttl.scrollWidth > ttl.clientWidth : null,
    header: r(document.querySelector('.panel > header')),
    subVisibility: cs(sub, 'visibility'),
    clockPadding: cs(clock, 'padding'),
    clockJustify: cs(clock, 'justifyContent'),
    clockTime: r(document.getElementById('clockTime')),
  };
})())`;

const CONTROL_STATE = `JSON.stringify({
  row: !!document.querySelector('.font-row'),
  shownClass: document.body.classList.contains('font-controls-shown'),
  editMode: document.body.classList.contains('edit-mode'),
  locked: document.body.classList.contains('edit-locked'),
})`;

export function lockCases() {
  return cases().filter((c) => !c.label.includes('atcap'));
}

async function measureLockToggle(edge, server, c) {
  const target = await newTarget(edge.port, 'about:blank');
  const session = new CDPTarget(target.webSocketDebuggerUrl);
  const problems = [];
  const step = async (label, locked, editMode) => {
    await evaluate(session, SET_STATE(locked, editMode));
    await new Promise((r) => setTimeout(r, 150));
    return {
      snap: JSON.parse(await evaluate(session, SNAPSHOT)),
      ctl: JSON.parse(await evaluate(session, CONTROL_STATE)),
      label,
    };
  };
  try {
    await session.connect();
    await session.send('Page.enable');
    await session.send('Emulation.setDeviceMetricsOverride', { width: c.w, height: c.h, deviceScaleFactor: 1, mobile: false });
    await session.send('Page.addScriptToEvaluateOnNewDocument', {
      source: `window.__bridgeTestInit = { widgetFontState: ${JSON.stringify(c.font)} };`,
    });
    await session.send('Page.navigate', { url: `${server.url}/new/widget.html?w=${c.id}&fixtures=/fixtures/` });
    await waitForPageCondition(session, "document.getElementById('widget-root').children.length > 0");
    await evaluate(session, 'document.fonts ? document.fonts.ready.then(() => true) : true');
    await new Promise((r) => setTimeout(r, 300));

    const normal = await step('平時', true, false);
    const locked1 = await step('鎖定編輯', true, true);
    const unlocked = await step('解鎖編輯', false, true);
    const unlockedGeom = await evaluate(session, measureExpr());
    const locked2 = await step('再鎖定', true, true);
    const base = JSON.stringify(normal.snap);
    for (const s of [locked1, locked2]) {
      if (!s.ctl.editMode || !s.ctl.locked) problems.push(`${s.label}：前提不成立（edit-mode／edit-locked）${JSON.stringify(s.ctl)}`);
      if (s.ctl.row || s.ctl.shownClass) problems.push(`${s.label}：不應有字級控制或 font-controls-shown ${JSON.stringify(s.ctl)}`);
      if (JSON.stringify(s.snap) !== base) {
        problems.push(`${s.label}：版面與平時不同 ${JSON.stringify(s.snap)} ≠ 平時 ${base}`);
      }
    }
    if (!unlocked.ctl.row || !unlocked.ctl.shownClass) {
      problems.push(`解鎖編輯：應出現控制與 font-controls-shown ${JSON.stringify(unlocked.ctl)}`);
    }
    for (const p of unlockedGeom.problems) problems.push(`解鎖編輯：${p}`);
    return { problems, detail: { normal: normal.snap, unlocked: unlocked.snap } };
  } finally {
    session.close();
    await closeTarget(edge.port, target.id);
  }
}

async function run() {
  const assertMode = process.argv.includes('--assert');
  const edge = await launchEdge();
  const server = await startServer(FIXTURE);
  const failures = [];
  try {
    for (const c of cases()) {
      const { problems, detail } = await measureCase(edge, server, c);
      const tag = `${c.id} ${c.label} ${c.w}x${c.h}`;
      console.log(`[font-geom] ${tag} ${problems.length ? 'FAIL' : 'ok'} ${JSON.stringify(detail)}`);
      for (const p of problems) {
        console.log(`    - ${p}`);
        failures.push(`${tag}：${p}`);
      }
    }
    for (const c of lockCases()) {
      const { problems, detail } = await measureLockToggle(edge, server, c);
      const tag = `${c.id} ${c.label} ${c.w}x${c.h} 鎖定→解鎖→鎖定`;
      console.log(`[font-geom] ${tag} ${problems.length ? 'FAIL' : 'ok'} ${JSON.stringify(detail)}`);
      for (const p of problems) {
        console.log(`    - ${p}`);
        failures.push(`${tag}：${p}`);
      }
    }
  } finally {
    await server.close();
    await edge.close();
  }
  if (failures.length > 0) {
    console.log(`[font-geom] FAIL（${failures.length} 項）`);
    if (assertMode) process.exit(1);
    return;
  }
  console.log('[font-geom] PASS');
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  run().catch((e) => {
    console.error(e);
    process.exit(1);
  });
}
