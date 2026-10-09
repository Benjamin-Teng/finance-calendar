#!/usr/bin/env node
// host/tests/compare/measure-list-collapse.mjs
//
// 量測清單小工具的「單行版面斷點」與「收合版面最窄可用寬度」（change
// widget-font-scale-per-widget，task 3.1，design.md D5；結論見該 change 的 task-3.1-report.md）。
//
// 作法（比照 measure-clock-natural-width.mjs）：headless Edge 開 `widget.html?w=<id>`（fixture
// 模式、deviceScaleFactor 1、無 ZoomFactor＝倍率 1），資料換成「最差情況」fixture（最長的實際
// 標題、帶「預／前」的列、最寬的日期、今天徽章、最長的備註，見下方 worstFixture／
// FIXED_WORST_ROWS），再以 CDP `Emulation.setDeviceMetricsOverride` 逐 1 px 改變 viewport 寬，
// 每個寬度在頁面內檢查：
//   原始判準（spec「清單在窄寬度收合」）：
//     A. 每列（`.ev`）子元素兩兩不重疊、都在列的內容框內；
//     B. 文字不被裁切：葉節點（`.ev` 子元素、標題列、`.dayhead`、`.divider`、`.empty`、`.hint`、
//        `.summary`）的文字行盒都在自己的框內，捲動區與面板 scrollWidth ≤ clientWidth；
//     C. 標題列 `.ttl`／`.sub` 不重疊、都在 header 內。
//   可讀性下限（本 task 另加，只會讓結果更保守；報告說明）：
//     R1. 清單標題（`.ev b`）換行時寬 ≥ 4 個全形字（4em）——否則合法但會一字一行；
//     R2. 右側數值（`.ev .n`）不換行；
//     R3. 小工具標題 `.ttl` 不換行。
// 掃描（每個寬度都跑上面的檢查；「X≥N」＝從起點往窄、每個寬度都合格的最窄寬度 N）：
//   - single：根元素 data-widget 改名（收合 media query 不生效）＝清單列一直維持單行。從設計寬
//     往窄掃，原始判準的 N＝「單行開始重疊」；斷點＝N 取整＋4（design.md D5「單行版面開始重疊
//     的寬度＋餘裕」），不大於設計寬（spec：寬度 ≥ 設計寬時與現行版面相同）。
//   - collapsed：widget.css 原樣、從目前斷點下方往窄掃＝收合版面本身的最窄寬度；含可讀性下限的
//     N 取整＋4＝建議 min_width（design.md D5）。
//   - actual：widget.css 原樣、從設計寬往窄掃＝整段寬度（單行＋收合）的核對。
// 另以四種字型（Noto Sans TC、Microsoft JhengHei、system-ui、sans-serif；使用者機器未必裝
// Noto Sans TC）重量，取最大值；並核對：設計寬下 actual 與 legacy（再撤銷本 task 不限寬度規則
// ＝改動前版面）逐元素相同、斷點兩側確實是單行／兩行、目前 min_width 下各字型都合格、spec 情境
// 「總經 0.64 倍設計寬＋字級 200%」的 CSS 寬可用。
//
// 用法：node host/tests/compare/measure-list-collapse.mjs [--json]
// 不改任何產品程式碼；暫存 fixture、伺服器暫存目錄與 Edge profile 用完即刪。

import path from 'node:path';
import { readFileSync } from 'node:fs';
import { mkdtemp, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { startServer } from './serve.mjs';
import { buildOverrideScript } from './capture-utils.mjs';
import { launchEdge, newTarget, closeTarget, CDPTarget, waitForPageCondition, evaluate } from './cdp.mjs';
import { widgetZoomBoxes, contentZoom } from './verify-visual-edges.mjs';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(__dirname, '..', '..', '..');
const BASE_FIXTURE = path.join(REPO_ROOT, 'host', 'ui', 'fixtures', 'tw-events.json');
const CSS_PATH = path.join(REPO_ROOT, 'host', 'ui', 'widget.css');

const SAFETY_PX = 4; // design.md D5：取整加 4
const FLOOR_EM = 4; // R1
const SCAN_MIN = 100;
const VIEW_H = 2400;
const FONTS = [null, '"Noto Sans TC"', '"Microsoft JhengHei"', 'system-ui', 'sans-serif'];
const asJson = process.argv.includes('--json');

// 設計寬＝WIDGET_SPECS 的 comfort 寬（擴充插槽同台股事件，widgets.rs custom_spec）。
const boxes = widgetZoomBoxes();
const WIDGETS = [
  { id: 'macro', designWidth: boxes.macro.comfortWidth, today: '2026-12-28', ready: '.ev' },
  { id: 'fixed', designWidth: boxes.fixed.comfortWidth, today: '2026-12-18', ready: '.ev' },
  { id: 'dynamic', designWidth: boxes.dynamic.comfortWidth, today: '2026-12-28', ready: '.ev' },
  { id: 'custom1', designWidth: boxes.fixed.comfortWidth, today: '2026-12-28', ready: '.summary' },
];

/** widget.css 的收合斷點（`@media (width < Npx)` 區塊內含 `[data-widget='<id>']`）；沒有＝null。 */
function cssBreakpoints() {
  const css = readFileSync(CSS_PATH, 'utf8');
  const out = {};
  const re = /@media \(width < ([\d.]+)px\)\s*\{/g;
  let m;
  while ((m = re.exec(css))) {
    const body = css.slice(re.lastIndex, css.indexOf('\n}', re.lastIndex));
    for (const id of body.matchAll(/\[data-widget='([\w-]+)'\]/g)) out[id[1]] = Number(m[1]);
  }
  return out;
}

// ── 最差情況資料 ─────────────────────────────────────────────────────────────────
/** update_tw_events.py 的指標中譯字典 `T` 的全部譯名（凍結檔，只讀）。 */
function macroDictionaryTitles() {
  const py = readFileSync(path.join(REPO_ROOT, 'update_tw_events.py'), 'utf8');
  const start = py.indexOf('\nT = {');
  const body = py.slice(start, py.indexOf('\n}\n', start));
  return [...body.matchAll(/"[^"]*":\s*"([^"]*)"/g)].map((m) => m[1]);
}
// 字典查不到時原樣保留英文（只換 m/m 等字尾，zh_title）：fixture 裡的實例＋ForexFactory 常見
// 英文標題（含最長的不可斷單字 Manufacturers／Expectations／Productivity／Supervision）。
const UNTRANSLATED = [
  'Final GDP Price Index (季增)',
  'FOMC Member Waller 談話',
  'Prelim UoM Inflation Expectations',
  'Revised UoM Inflation Expectations',
  'Tankan Large Manufacturers Index',
  'Prelim Nonfarm Productivity (季增)',
  'Fed Vice Chair for Supervision Barr 談話',
  'Final Wholesale Inventories (月增)',
  'Tertiary Industry Activity (月增)',
];
const WORST_VALUE = '-102.3B'; // 「預／前」數值：ForexFactory 常見最長格式（7 字元）

function worstFixture(base) {
  const titles = [...macroDictionaryTitles(), ...UNTRANSLATED];
  const countries = [
    ['US', '美'],
    ['EU', '歐'],
    ['JP', '日'],
  ];
  const macro = titles.map((title, i) => {
    const d = new Date(2026, 11, 28 + Math.floor(i / 48), (i % 48) >> 1, 59);
    const [country, flag] = countries[i % 3];
    const iso = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
    const time = `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
    // 每 7 筆一筆沒有數值（.n 為空、收合時不多佔一行）；其餘帶最長的「預／前」。
    const vals = i % 7 === 6 ? { forecast: '', previous: '' } : { forecast: WORST_VALUE, previous: WORST_VALUE };
    return { dt: `${iso}T${time}`, date: iso, time, ts: Math.floor(d.getTime() / 1000), country, flag, impact: 'high', title, title_en: title, ...vals };
  });
  // 台股動態事件：最長的代號＋名稱（fixture 實例 00985B 群益ESG投等債0-5）、最長的備註、今天徽章。
  const names = [
    ['00985B', '群益ESG投等債0-5'],
    ['7631', '聚賢研發-創'],
    ['2330', '台積電'],
  ];
  const notes = {
    earnings: ['Q2 財報（線上）', 'Q4 財報'],
    conference: ['法說會（線上）', '法說會'],
    meeting: ['股東臨時會・改選董監', '股東常會'],
  };
  const events = [];
  for (const date of ['2026-12-28', '2026-12-29', '2026-12-30', '2026-12-31']) {
    for (const type of ['earnings', 'conference', 'meeting']) {
      for (const [code, name] of names) {
        for (const note of notes[type]) events.push({ date, type, code, name, note });
      }
    }
  }
  const punish = names.flatMap(([code, name]) => [
    { code, name, start: '2026-12-15', end: '2026-12-31', times: 2, market: '上櫃' },
    { code: `${code}`, name, start: '2026-12-20', end: '2026-12-31', times: 1, market: '上市' },
  ]);
  return { ...base, macro, events, punish, updated: '2026-12-28 23:58', fetched: '2026-12-28 23:58' };
}

// 台股固定事件的列由日期計算、不吃資料：渲染後再附加最差組合（最寬日期、各種最長標籤、今天徽章；
// 標記與 fixed.js 相同）。
const FIXED_LABELS = ['台股・那指・道瓊期貨季度結算', '12月營收公布截止（10日前）', 'Q4＋年報 財報公布截止', '台指期／選擇權結算'];
const FIXED_WORST_ROWS = FIXED_LABELS.flatMap((label) => [
  `<li class="ev week"><span class="d">12/28 (一)</span><b>${label}</b><span class="badge">今天</span></li>`,
  `<li class="ev past"><span class="d">12/28 (一)</span><b>${label}</b></li>`,
]).join('');
// 擴充插槽：摘要再附一段長英數鍵名（customN.json 可放任意 JSON）。
const CUSTOM_WORST_SUMMARY = '3 個頂層鍵：updated、fetched、very_long_top_level_key_name_from_custom_json（128 筆）';

// ── 頁面端：檢查函式（以 Function.toString 注入）────────────────────────────────────
function pageCheck(floorEm) {
  const EPS = 0.5;
  const problems = [];
  const raw = [];
  const readable = [];
  const vis = (el) => {
    const cs = getComputedStyle(el);
    return cs.display !== 'none' && cs.visibility !== 'hidden' && el.getClientRects().length > 0;
  };
  const r = (el) => el.getBoundingClientRect();
  const name = (el) => (el.className ? `${el.tagName.toLowerCase()}.${String(el.className).split(' ').join('.')}` : el.tagName.toLowerCase());
  const overlaps = (a, b) =>
    Math.min(a.right, b.right) - Math.max(a.left, b.left) > EPS && Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top) > EPS;
  const inside = (a, box) => a.left >= box.left - EPS && a.right <= box.right + EPS;
  const contentBox = (el) => {
    const b = r(el);
    const cs = getComputedStyle(el);
    return {
      left: b.left + parseFloat(cs.borderLeftWidth) + parseFloat(cs.paddingLeft),
      right: b.right - parseFloat(cs.borderRightWidth) - parseFloat(cs.paddingRight),
    };
  };
  const textLines = (el) => {
    const range = document.createRange();
    range.selectNodeContents(el);
    const rects = [...range.getClientRects()].filter((x) => x.width > 0);
    return rects;
  };
  const textClipped = (el) => {
    const box = contentBox(el);
    return textLines(el).some((x) => x.left < box.left - EPS || x.right > box.right + EPS);
  };
  const lineCount = (el) => {
    const tops = new Set(textLines(el).map((x) => Math.round(x.top)));
    return tops.size;
  };
  const text = (el) => el.textContent.replace(/\s+/g, ' ').trim().slice(0, 24);

  const panel = document.querySelector('#widget-root > .panel');
  // B：捲動區與面板沒有水平溢出
  for (const el of [panel, ...panel.querySelectorAll('.scroll-area, header')]) {
    if (vis(el) && el.scrollWidth > el.clientWidth) raw.push(`${name(el)} 水平溢出 ${el.scrollWidth}>${el.clientWidth}`);
  }
  // A／B：清單列
  for (const row of panel.querySelectorAll('.ev')) {
    if (!vis(row)) continue;
    const kids = [...row.children].filter(vis);
    const box = contentBox(row);
    for (let i = 0; i < kids.length; i++) {
      const ki = r(kids[i]);
      if (!inside(ki, box)) raw.push(`列「${text(row)}」的 ${name(kids[i])} 超出列`);
      if (kids[i].textContent.trim() && textClipped(kids[i])) raw.push(`列「${text(row)}」的 ${name(kids[i])} 文字被裁切`);
      for (let j = i + 1; j < kids.length; j++) {
        if (overlaps(ki, r(kids[j]))) raw.push(`列「${text(row)}」的 ${name(kids[i])} 與 ${name(kids[j])} 重疊`);
      }
    }
    const b = row.querySelector(':scope > b');
    if (b && vis(b)) {
      const fs = parseFloat(getComputedStyle(b).fontSize);
      // 標題本身就短於 4em（例如「失業率」）而一行放得下時不算。
      if (r(b).width + EPS < floorEm * fs && lineCount(b) > 1) readable.push(`列「${text(row)}」標題寬 ${r(b).width.toFixed(1)} < ${floorEm}em`);
    }
    const n = row.querySelector(':scope > .n');
    if (n && vis(n) && n.textContent.trim() && lineCount(n) > 1) readable.push(`列「${text(row)}」數值換行`);
  }
  // B：其他文字區塊
  for (const el of panel.querySelectorAll('.dayhead, .divider, .empty, .hint, .summary')) {
    if (vis(el) && textClipped(el)) raw.push(`${name(el)}「${text(el)}」文字被裁切`);
  }
  // C：標題列
  const header = panel.querySelector(':scope > header');
  if (header) {
    const hb = contentBox(header);
    const parts = [...header.children].filter(vis);
    for (let i = 0; i < parts.length; i++) {
      if (!inside(r(parts[i]), hb)) raw.push(`標題列 ${name(parts[i])} 超出 header`);
      if (parts[i].textContent.trim() && textClipped(parts[i])) raw.push(`標題列 ${name(parts[i])} 文字被裁切`);
      for (let j = i + 1; j < parts.length; j++) if (overlaps(r(parts[i]), r(parts[j]))) raw.push(`標題列 ${name(parts[i])} 與 ${name(parts[j])} 重疊`);
    }
    const ttl = header.querySelector('.ttl');
    if (ttl && lineCount(ttl) > 1) readable.push('小工具標題 .ttl 換行');
  }
  problems.push(...raw, ...readable);
  return { raw, readable };
}

/** 頁面端：每個 `.ev`（有數值者）目前是否為兩行（`.n` 在標題下方）。 */
function pageRowShape() {
  let single = 0;
  let double = 0;
  for (const row of document.querySelectorAll('.ev')) {
    const b = row.querySelector(':scope > b');
    const n = row.querySelector(':scope > .n');
    if (!b || !n || !n.textContent.trim()) continue;
    if (n.getBoundingClientRect().top >= b.getBoundingClientRect().bottom - 0.5) double++;
    else single++;
  }
  return { single, double };
}

/** 頁面端：面板內所有元素的矩形（比較 actual 與 legacy 版面是否逐元素相同）。 */
function pageRects() {
  return [...document.querySelectorAll('#widget-root *')].map((el) => {
    const b = el.getBoundingClientRect();
    return [Math.round(b.left * 64) / 64, Math.round(b.top * 64) / 64, Math.round(b.width * 64) / 64, Math.round(b.height * 64) / 64];
  });
}

/**
 * 頁面端切換版面：actual＝widget.css 原樣；single＝只停用收合 media query（data-widget 改名），
 * 不限寬度的標題列換行等規則照舊＝「一直維持單行的清單列」；legacy＝single 再撤銷本 task 加的
 * 不限寬度規則＝改動前的版面。
 */
function pageSetMode(mode, id, font) {
  document.documentElement.dataset.widget = mode === 'actual' ? id : `${mode}-${id}`;
  let st = document.getElementById('__measure_legacy');
  if (mode === 'legacy' && !st) {
    st = document.createElement('style');
    st.id = '__measure_legacy';
    st.textContent =
      '.panel > header { flex-wrap: nowrap !important; } .empty, .hint, .dayhead, .divider, .summary { overflow-wrap: normal !important; }';
    document.head.appendChild(st);
  } else if (mode !== 'legacy' && st) {
    st.remove();
  }
  document.body.style.fontFamily = font || '';
  return document.documentElement.dataset.widget;
}

// ── 驅動 ─────────────────────────────────────────────────────────────────────────
const call = (fn, ...args) => `(${fn.toString()})(${args.map((a) => JSON.stringify(a)).join(', ')})`;

async function openWidget(edge, server, w) {
  const target = await newTarget(edge.port, 'about:blank');
  const session = new CDPTarget(target.webSocketDebuggerUrl);
  await session.connect();
  await session.send('Page.enable');
  await session.send('Emulation.setDeviceMetricsOverride', { width: w.designWidth, height: VIEW_H, deviceScaleFactor: 1, mobile: false });
  const [y, mo, d] = w.today.split('-').map(Number);
  await session.send('Page.addScriptToEvaluateOnNewDocument', {
    source: buildOverrideScript(new Date(y, mo - 1, d, 23, 58, 58).getTime(), w.today),
  });
  await session.send('Page.navigate', { url: `${server.url}/new/widget.html?w=${w.id}&fixtures=/fixtures/` });
  await waitForPageCondition(session, `!!document.querySelector(${JSON.stringify(w.ready)})`);
  await new Promise((res) => setTimeout(res, 400));
  if (w.id === 'fixed') {
    await evaluate(session, `document.querySelector('.evlist').insertAdjacentHTML('beforeend', ${JSON.stringify(FIXED_WORST_ROWS)})`);
  }
  if (w.id === 'custom1') {
    await evaluate(session, `document.querySelector('.summary').textContent = ${JSON.stringify(CUSTOM_WORST_SUMMARY)}`);
  }
  return { session, target };
}

async function setWidth(session, width) {
  await session.send('Emulation.setDeviceMetricsOverride', { width, height: VIEW_H, deviceScaleFactor: 1, mobile: false });
}

async function checkAt(session, width) {
  await setWidth(session, width);
  return evaluate(session, call(pageCheck, FLOOR_EM));
}

/** 由 `from` 往窄掃到 SCAN_MIN：回傳「從 from 到它都合格」的最窄寬度（raw／含可讀性下限）與第一個失敗原因。 */
async function scan(session, from) {
  let rawOk = null;
  let readOk = null;
  let rawFail = null;
  let readFail = null;
  for (let w = from; w >= SCAN_MIN; w--) {
    const res = await checkAt(session, w);
    if (rawFail === null) {
      if (res.raw.length) rawFail = { width: w, first: res.raw[0], count: res.raw.length };
      else rawOk = w;
    }
    if (readFail === null) {
      if (res.raw.length || res.readable.length) readFail = { width: w, first: [...res.raw, ...res.readable][0], count: res.raw.length + res.readable.length };
      else readOk = w;
    }
    if (rawFail && readFail) break;
  }
  return { rawOk, readOk, rawFail, readFail };
}

const tmp = await mkdtemp(path.join(tmpdir(), 'fc-measure-list-'));
const fixturePath = path.join(tmp, 'tw-events.worst.json');
await writeFile(fixturePath, JSON.stringify(worstFixture(JSON.parse(readFileSync(BASE_FIXTURE, 'utf8')))));
const server = await startServer(fixturePath);
const edge = await launchEdge();
const breakpoints = cssBreakpoints();
// 擴充插槽不在 widgetZoomBoxes（只解析財經五個）：min 寬取 widgets.rs `custom_spec` 的 list_box 第一個引數。
const customMin = readFileSync(path.join(REPO_ROOT, 'host', 'src', 'widgets.rs'), 'utf8').match(
  /const fn custom_spec[\s\S]*?zoom_box: list_box\(([\d.]+),/,
);
if (!customMin) throw new Error('widgets.rs 找不到 custom_spec 的 list_box');
const zoomBoxes = { ...boxes, custom1: { ...boxes.fixed, minWidth: Number(customMin[1]) } };
const result = { method: {}, widgets: {} };
try {
  for (const w of WIDGETS) {
    const { session, target } = await openWidget(edge, server, w);
    const rowCount = await evaluate(session, `document.querySelectorAll('.ev').length`);
    const bp = breakpoints[w.id] ?? null;
    const perFont = {};
    for (const font of FONTS) {
      const label = font || '(widget.css 字型堆疊)';
      // single：清單列一直維持單行，由設計寬往窄掃＝「單行開始重疊」的寬度。
      await evaluate(session, call(pageSetMode, 'single', w.id, font));
      const single = await scan(session, w.designWidth);
      await evaluate(session, call(pageSetMode, 'actual', w.id, font));
      // collapsed：從斷點下方開始掃＝收合版面本身的最窄可用寬度（與目前斷點取值無關）；
      // actual：從設計寬掃＝整段寬度（單行＋收合）都可用的最窄寬度，用來核對目前斷點。
      const collapsed = await scan(session, bp === null ? w.designWidth : Math.min(Math.ceil(bp) - 1, w.designWidth));
      const actual = await scan(session, w.designWidth);
      perFont[label] = { single, collapsed, actual };
    }
    // 結論：各字型取最大
    const maxOf = (pick) => Math.max(...Object.values(perFont).map(pick));
    const singleReadOk = maxOf((f) => f.single.readOk ?? Infinity);
    const singleRawOk = maxOf((f) => f.single.rawOk ?? Infinity);
    const collapsedReadOk = maxOf((f) => f.collapsed.readOk ?? Infinity);
    const collapsedRawOk = maxOf((f) => f.collapsed.rawOk ?? Infinity);
    const actualReadOk = maxOf((f) => f.actual.readOk ?? Infinity);
    const actualRawOk = maxOf((f) => f.actual.rawOk ?? Infinity);
    // 斷點＝單行開始重疊（原始判準）的寬度取整＋4（design.md D5），不大於設計寬。
    const suggestedBreakpoint = Math.min(Math.ceil(singleRawOk - 1e-6) + SAFETY_PX, w.designWidth);
    const suggestedMinWidth = Math.ceil(collapsedReadOk - 1e-6) + SAFETY_PX;

    // 核對（目前 widget.css 與 widgets.rs 的數值）
    await evaluate(session, call(pageSetMode, 'actual', w.id, null));
    const minW = Math.floor(zoomBoxes[w.id].minWidth);
    const verify = {};
    // 設計寬：actual 與 legacy 逐元素相同
    await setWidth(session, w.designWidth);
    const actualRects = await evaluate(session, call(pageRects));
    await evaluate(session, call(pageSetMode, 'legacy', w.id, null));
    const legacyRects = await evaluate(session, call(pageRects));
    await evaluate(session, call(pageSetMode, 'actual', w.id, null));
    verify.designWidthSameAsLegacy = JSON.stringify(actualRects) === JSON.stringify(legacyRects);
    verify.designWidthShape = await evaluate(session, call(pageRowShape));
    if (bp !== null) {
      await setWidth(session, bp);
      verify.atBreakpointShape = await evaluate(session, call(pageRowShape));
      await setWidth(session, bp - 1);
      verify.belowBreakpointShape = await evaluate(session, call(pageRowShape));
    }
    for (const font of FONTS) {
      await evaluate(session, call(pageSetMode, 'actual', w.id, font));
      const res = await checkAt(session, minW);
      verify[`atMinWidth ${font || '(堆疊)'}`] = { width: minW, raw: res.raw.length, readable: res.readable.length, first: [...res.raw, ...res.readable][0] ?? null };
    }
    await evaluate(session, call(pageSetMode, 'actual', w.id, null));
    await setWidth(session, minW);
    verify.atMinWidthShape = await evaluate(session, call(pageRowShape));
    // 整段寬度（設計寬 → min_width）原始判準都合格；收合範圍（斷點下方 → min_width）含可讀性下限都合格。
    verify.fullRangeRawOk = actualRawOk <= minW;
    verify.collapsedRangeReadableOk = collapsedReadOk <= minW;
    if (w.id === 'macro') {
      // spec「窄框的清單可以放大」：邏輯寬 0.64 × 設計寬、高度充足、字級 200%。
      const logicalW = 0.64 * w.designWidth;
      const zoom = contentZoom(logicalW, 4000, 1, zoomBoxes.macro, 2);
      const cssW = Math.round(logicalW / zoom);
      const res = await checkAt(session, cssW);
      verify.specNarrowScaleUp = { logicalW, zoom, cssWidth: cssW, raw: res.raw.length, readable: res.readable.length, shape: await evaluate(session, call(pageRowShape)) };
    }
    session.close();
    await closeTarget(edge.port, target.id);
    result.widgets[w.id] = {
      designWidth: w.designWidth,
      rowCount,
      perFont,
      conclusion: { singleRawOk, singleReadOk, collapsedRawOk, collapsedReadOk, actualRawOk, actualReadOk, suggestedBreakpoint, suggestedMinWidth },
      current: { cssBreakpoint: bp, minWidth: minW },
      verify,
    };
  }
  result.method = {
    engine: 'headless Edge（CDP），deviceScaleFactor 1，無 ZoomFactor',
    floorEm: FLOOR_EM,
    safetyPx: SAFETY_PX,
    fonts: FONTS.map((f) => f || '(widget.css 字型堆疊)'),
    worstValue: WORST_VALUE,
  };
} finally {
  await edge.close();
  await server.close();
  await rm(tmp, { recursive: true, force: true });
}

if (asJson) {
  console.log(JSON.stringify(result, null, 2));
} else {
  console.log('== 方法 ==');
  console.log(JSON.stringify(result.method));
  for (const [id, w] of Object.entries(result.widgets)) {
    console.log(`\n== ${id}（設計寬 ${w.designWidth}，清單列 ${w.rowCount}）==`);
    for (const [font, f] of Object.entries(w.perFont)) {
      const fmt = (s) => `raw≥${s.rawOk} 可讀≥${s.readOk}｜raw 失敗@${s.rawFail?.width}：${s.rawFail?.first ?? '-'}｜可讀失敗@${s.readFail?.width}：${s.readFail?.first ?? '-'}`;
      console.log(`  ${font}`);
      console.log(`    single 單行：${fmt(f.single)}`);
      console.log(`    collapsed 收合（斷點下方起）：${fmt(f.collapsed)}`);
      console.log(`    actual 整段（設計寬起）：${fmt(f.actual)}`);
    }
    console.log('  結論：', JSON.stringify(w.conclusion));
    console.log('  目前數值：', JSON.stringify(w.current));
    console.log('  核對：', JSON.stringify(w.verify));
  }
}
