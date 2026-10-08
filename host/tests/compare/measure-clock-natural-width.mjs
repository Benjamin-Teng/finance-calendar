#!/usr/bin/env node
// host/tests/compare/measure-clock-natural-width.mjs
//
// 量測時鐘小工具「內容自然寬度」（change widget-adaptive-zoom-and-grid，task 2.1，design.md D1）。
//
// 作法：headless Edge 開 `widget.html?w=clock`（fixture 模式，裝置像素比 1、無 ZoomFactor＝倍率 1），
// 把 `#widget-root > .panel` 改成 `width:max-content`（面板依內容收縮），讀 border box 寬度：
//   1. 時間：枚舉 00:00–23:59 全部 1440 個字串，各自量面板寬（其餘兩行清空，只看這一行）。
//   2. 日期：枚舉 2026-01-01 至 2031-12-31 每一天 `clock.js` 的 `Y/M/D　週X` 字串。
//   3. 週別：同一區間每一天 `本週 md(ws) – md(we)` 字串（呼叫 common.js 真實函式，去重後量）。
//   4. 取三行各自最寬者組合，量整個面板寬；加 `#widget-root` 左右 `--widget-gap` 即整扇視窗所需
//      邏輯寬度（實測值）。
//   5. 高度：同一最寬內容下的自然高度（border box）＋上下 gap；再把視窗設成「寬 = 建議 min_width、
//      高 = 156」走正式版面（面板 flex 填滿），驗證 scrollHeight ≤ clientHeight 且文字沒被裁。
//   6. 另以 Date 覆寫成 2026-12-28 23:58 走真實 `tick()` 路徑量一次，與步驟 4 交叉核對。
//   7. 以 CDP `CSS.getPlatformFontsForNode` 記錄三行實際生效的字型。
//
// 用法：node host/tests/compare/measure-clock-natural-width.mjs [--json]
// 不改任何產品程式碼；暫存目錄與 profile 用完即刪。

import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { startServer } from './serve.mjs';
import { buildOverrideScript } from './capture-utils.mjs';
import { launchEdge, newTarget, closeTarget, CDPTarget, waitForPageCondition, evaluate } from './cdp.mjs';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(__dirname, '..', '..', '..');
const FIXTURE = path.join(REPO_ROOT, 'host', 'ui', 'fixtures', 'tw-events.json');

const WIDGET_GAP = 8; // widget.css :root --widget-gap；host/src/widgets.rs WIDGET_GAP_CSS_PX
const SAFETY_PX = 4; // design.md Risks 要求的字型差異餘裕
const CURRENT_MIN_HEIGHT = 140 + 2 * WIDGET_GAP; // widgets.rs 現行 design_min_height

const asJson = process.argv.includes('--json');

// 在頁面內執行：全部枚舉與量測都在頁面端做（一次 evaluate 同步完成，不會被 1 秒 tick 打斷）。
const MEASURE_SCRIPT = `(async () => {
  const common = await import('/new/common.js');
  const root = document.getElementById('widget-root');
  const panel = root.querySelector('.panel');
  const timeEl = document.getElementById('clockTime');
  const dateEl = document.getElementById('clockDate');
  const weekEl = document.getElementById('weekRange');
  const style = document.createElement('style');
  style.textContent = '#widget-root > .panel { flex: none !important; width: max-content !important; align-self: flex-start !important; height: auto !important; }';
  document.head.appendChild(style);
  const W = () => panel.getBoundingClientRect().width;
  const H = () => panel.getBoundingClientRect().height;
  const rootPad = parseFloat(getComputedStyle(root).paddingLeft);
  const rootPadR = parseFloat(getComputedStyle(root).paddingRight);
  const rootPadT = parseFloat(getComputedStyle(root).paddingTop);
  const rootPadB = parseFloat(getComputedStyle(root).paddingBottom);

  function setOnly(which, text) {
    timeEl.textContent = which === 'time' ? text : '';
    dateEl.textContent = which === 'date' ? text : '';
    weekEl.textContent = which === 'week' ? text : '';
  }
  const pad2 = common.pad2;

  // 1. 時間
  const times = [];
  for (let h = 0; h < 24; h++) for (let m = 0; m < 60; m++) times.push(pad2(h) + ':' + pad2(m));
  const timeW = new Map();
  for (const t of times) { setOnly('time', t); timeW.set(t, W()); }

  // 2./3. 日期與週別（2026-01-01 至 2031-12-31）
  const dates = new Set();
  const weeks = new Set();
  for (let d = new Date(2026, 0, 1); d.getFullYear() <= 2031; d = new Date(d.getFullYear(), d.getMonth(), d.getDate() + 1)) {
    dates.add(d.getFullYear() + '/' + (d.getMonth() + 1) + '/' + d.getDate() + '\\u3000週' + common.WD[d.getDay()]);
    const ws = common.weekStartOf(d);
    const we = common.addDays(ws, 6);
    weeks.add('本週 ' + common.md(ws) + ' – ' + common.md(we));
  }
  const dateW = new Map();
  for (const t of dates) { setOnly('date', t); dateW.set(t, W()); }
  const weekW = new Map();
  for (const t of weeks) { setOnly('week', t); weekW.set(t, W()); }

  const top = (map, n) => [...map.entries()].sort((a, b) => b[1] - a[1]).slice(0, n);
  const minOf = (map) => Math.min(...map.values());
  const maxOf = (map) => Math.max(...map.values());
  const worstTime = top(timeW, 1)[0][0];
  const worstDate = top(dateW, 1)[0][0];
  const worstWeek = top(weekW, 1)[0][0];

  // 4. 三行合併的最寬組合
  timeEl.textContent = worstTime; dateEl.textContent = worstDate; weekEl.textContent = worstWeek;
  const combinedPanelW = W();
  const combinedPanelH = H();
  const windowW = combinedPanelW + rootPad + rootPadR;
  const windowH = combinedPanelH + rootPadT + rootPadB;

  // 各行自然高度（border box，單看一行不含 padding 時的行盒高度）
  const lineBox = (el) => { const r = el.getBoundingClientRect(); return { top: r.top, bottom: r.bottom, height: r.height, width: r.width }; };
  const panelRect = panel.getBoundingClientRect();
  const cs = getComputedStyle(panel);

  // 候選字串寬度表（供報告）
  const sample = {
    time: [...top(timeW, 5), ['(最窄)', minOf(timeW)], ['00:00', timeW.get('00:00')], ['23:58', timeW.get('23:58')], ['11:11', timeW.get('11:11')], ['12:30', timeW.get('12:30')]],
    date: [...top(dateW, 5), ['(最窄)', minOf(dateW)], ['2026/12/28　週一', dateW.get('2026/12/28\\u3000週一')]],
    week: [...top(weekW, 5), ['(最窄)', minOf(weekW)], ['本週 12/27 – 1/2', weekW.get('本週 12/27 – 1/2')]],
  };
  const lines = { time: lineBox(timeEl), date: lineBox(dateEl), week: lineBox(weekEl) };

  return {
    counts: { times: times.length, dates: dates.size, weeks: weeks.size },
    chrome: {
      panelPadding: cs.padding, panelBorder: cs.borderLeftWidth, boxSizing: cs.boxSizing,
      rootPadding: [rootPadT, rootPadR, rootPadB, rootPad],
    },
    worst: { time: [worstTime, timeW.get(worstTime)], date: [worstDate, dateW.get(worstDate)], week: [worstWeek, weekW.get(worstWeek)] },
    ranges: { time: [minOf(timeW), maxOf(timeW)], date: [minOf(dateW), maxOf(dateW)], week: [minOf(weekW), maxOf(weekW)] },
    sample,
    combined: { panelW: combinedPanelW, panelH: combinedPanelH, windowW, windowH, lines, panelRect: { w: panelRect.width, h: panelRect.height } },
  };
})()`;

// 正式版面核對：視窗（viewport）= 建議寬 × 156，不改 CSS，面板 flex 填滿；用最寬字串。
const layoutCheckScript = (worst) => `(() => {
  const root = document.getElementById('widget-root');
  const panel = root.querySelector('.panel');
  document.getElementById('clockTime').textContent = ${JSON.stringify(worst.time)};
  document.getElementById('clockDate').textContent = ${JSON.stringify(worst.date)};
  document.getElementById('weekRange').textContent = ${JSON.stringify(worst.week)};
  const r = panel.getBoundingClientRect();
  const kids = [...panel.children].map((el) => {
    const b = el.getBoundingClientRect();
    return { id: el.id, top: b.top - r.top, bottom: r.bottom - b.bottom, scrollW: el.scrollWidth, clientW: el.clientWidth };
  });
  return {
    inner: [innerWidth, innerHeight],
    panel: { w: r.width, h: r.height, clientH: panel.clientHeight, scrollH: panel.scrollHeight, clientW: panel.clientWidth, scrollW: panel.scrollWidth },
    kids,
    clippedV: panel.scrollHeight > panel.clientHeight,
    clippedH: panel.scrollWidth > panel.clientWidth,
  };
})()`;

async function openPage(edge, server, { w, h, init }) {
  const target = await newTarget(edge.port, 'about:blank');
  const session = new CDPTarget(target.webSocketDebuggerUrl);
  await session.connect();
  await session.send('Page.enable');
  await session.send('Emulation.setDeviceMetricsOverride', { width: w, height: h, deviceScaleFactor: 1, mobile: false });
  if (init) await session.send('Page.addScriptToEvaluateOnNewDocument', { source: init });
  await session.send('Page.navigate', { url: `${server.url}/new/widget.html?w=clock&fixtures=/fixtures/` });
  await waitForPageCondition(session, "!!document.getElementById('clockTime') && document.getElementById('clockTime').textContent !== '--:--'");
  await new Promise((r) => setTimeout(r, 600));
  return { session, target };
}

async function fontsOf(session) {
  await session.send('DOM.enable');
  await session.send('CSS.enable');
  const doc = await session.send('DOM.getDocument', { depth: 0 });
  const out = {};
  for (const id of ['clockTime', 'clockDate', 'weekRange']) {
    const q = await session.send('DOM.querySelector', { nodeId: doc.root.nodeId, selector: '#' + id });
    const f = await session.send('CSS.getPlatformFontsForNode', { nodeId: q.nodeId });
    out[id] = f.fonts.map((x) => `${x.familyName}${x.isCustomFont ? ' (web font)' : ''} x${x.glyphCount}`);
  }
  const specified = await evaluate(session, `(() => {
    const out = {};
    for (const id of ['clockTime', 'clockDate', 'weekRange']) {
      const cs = getComputedStyle(document.getElementById(id));
      out[id] = { fontFamily: cs.fontFamily, fontSize: cs.fontSize, fontWeight: cs.fontWeight, letterSpacing: cs.letterSpacing, fontVariantNumeric: cs.fontVariantNumeric };
    }
    return out;
  })()`);
  return { platform: out, computed: specified };
}

const server = await startServer(FIXTURE);
const edge = await launchEdge();
let result;
try {
  // A. 枚舉量測（寬 viewport，避免窄視窗讓換行污染量測）
  const a = await openPage(edge, server, { w: 1400, h: 600 });
  const fonts = await fontsOf(a.session);
  const measured = await evaluate(a.session, MEASURE_SCRIPT);
  a.session.close();
  await closeTarget(edge.port, a.target.id);

  const m = measured.combined;
  const measuredWindowW = m.windowW;
  const ceilW = Math.ceil(measuredWindowW - 1e-6);
  const suggested = ceilW + SAFETY_PX;
  const worstStrings = { time: measured.worst.time[0], date: measured.worst.date[0], week: measured.worst.week[0] };

  // B. 真實 tick 路徑：Date 固定在 2026-12-28 23:58（台北時區字面值，頁面以本機時區解讀）
  const fixedMs = new Date(2026, 11, 28, 23, 58, 0).getTime();
  const b = await openPage(edge, server, { w: 1400, h: 600, init: buildOverrideScript(fixedMs, '2026-12-28') });
  const real = await evaluate(b.session, `(() => {
    const root = document.getElementById('widget-root');
    const panel = root.querySelector('.panel');
    const style = document.createElement('style');
    style.textContent = '#widget-root > .panel { flex: none !important; width: max-content !important; align-self: flex-start !important; height: auto !important; }';
    document.head.appendChild(style);
    const r = panel.getBoundingClientRect();
    return { text: [clockTime.textContent, clockDate.textContent, weekRange.textContent], panelW: r.width, panelH: r.height };
  })()`);
  b.session.close();
  await closeTarget(edge.port, b.target.id);

  // C. 正式版面：視窗 = 建議寬 × 156（現行 design_min_height）；也量「實測寬 ceil」與「再窄 1px」作對照
  const layouts = {};
  for (const [label, w] of [['suggested', suggested], ['measuredCeil', ceilW], ['ceilMinus1', ceilW - 1]]) {
    const c = await openPage(edge, server, { w, h: CURRENT_MIN_HEIGHT });
    layouts[label] = await evaluate(c.session, layoutCheckScript(worstStrings));
    c.session.close();
    await closeTarget(edge.port, c.target.id);
  }

  // D. 字型敏感度：Noto Sans TC 不在使用者機器上時會退到下一順位（Microsoft JhengHei、system-ui）。
  //    強制 body 改用各候選字型，量同一組最寬字串（time/date/week 各自單獨量，取最大）。
  const fontVariants = {};
  for (const fam of ['"Noto Sans TC"', '"Microsoft JhengHei"', 'system-ui', 'sans-serif']) {
    const d = await openPage(edge, server, { w: 1400, h: 600 });
    fontVariants[fam] = await evaluate(d.session, `(() => {
      document.body.style.fontFamily = ${JSON.stringify(fam)};
      const panel = document.querySelector('#widget-root > .panel');
      const st = document.createElement('style');
      st.textContent = '#widget-root > .panel { flex: none !important; width: max-content !important; align-self: flex-start !important; height: auto !important; }';
      document.head.appendChild(st);
      const out = {};
      const t = clockTime, d = clockDate, w = weekRange;
      t.textContent = ${JSON.stringify(worstStrings.time)}; d.textContent = ${JSON.stringify(worstStrings.date)}; w.textContent = ${JSON.stringify(worstStrings.week)};
      const r = panel.getBoundingClientRect();
      out.combined = { panelW: r.width, panelH: r.height, windowW: r.width + 16, windowH: r.height + 16 };
      for (const [k, el] of [['time', t], ['date', d], ['week', w]]) {
        const keep = [t.textContent, d.textContent, w.textContent];
        t.textContent = k === 'time' ? keep[0] : ''; d.textContent = k === 'date' ? keep[1] : ''; w.textContent = k === 'week' ? keep[2] : '';
        out[k + 'PanelW'] = panel.getBoundingClientRect().width;
        t.textContent = keep[0]; d.textContent = keep[1]; w.textContent = keep[2];
      }
      return out;
    })()`);
    const fontNames = await fontsOf(d.session);
    fontVariants[fam].platformFont = fontNames.platform.clockTime;
    d.session.close();
    await closeTarget(edge.port, d.target.id);
  }

  result = {
    fontVariants,
    method: 'headless Edge, deviceScaleFactor=1, 無 ZoomFactor；#widget-root>.panel 設 width:max-content',
    fonts,
    measured,
    real,
    suggested: { measuredWindowW, ceilW, safetyPx: SAFETY_PX, minWidth: suggested },
    height: {
      naturalPanelH: m.panelH,
      naturalWindowH: m.windowH,
      currentDesignMinHeight: CURRENT_MIN_HEIGHT,
      withinCurrent: m.windowH <= CURRENT_MIN_HEIGHT,
    },
    layouts,
  };
} finally {
  await edge.close();
  await server.close();
}

if (asJson) {
  console.log(JSON.stringify(result, null, 2));
} else {
  const r = result;
  const f = (n) => (typeof n === 'number' ? n.toFixed(3) : n);
  console.log('== 方法 ==');
  console.log(r.method);
  console.log('\n== 實際生效字型 ==');
  console.log(JSON.stringify(r.fonts, null, 2));
  console.log('\n== 枚舉數量 ==');
  console.log(JSON.stringify(r.measured.counts));
  console.log('\n== 面板 chrome ==');
  console.log(JSON.stringify(r.measured.chrome));
  for (const k of ['time', 'date', 'week']) {
    console.log(`\n== ${k} 候選（面板 border box 寬，其餘兩行清空）==`);
    for (const [s, w] of r.measured.sample[k]) console.log(`  ${JSON.stringify(s)}\t${f(w)}`);
    console.log(`  範圍 min=${f(r.measured.ranges[k][0])} max=${f(r.measured.ranges[k][1])}`);
  }
  console.log('\n== 最寬組合 ==');
  console.log(JSON.stringify({ worst: r.measured.worst, combined: { panelW: m2(r.measured.combined.panelW), panelH: m2(r.measured.combined.panelH), windowW: m2(r.measured.combined.windowW), windowH: m2(r.measured.combined.windowH) } }));
  console.log('各行 box：', JSON.stringify(r.measured.combined.lines));
  console.log('\n== 真實 tick 路徑（Date 固定 2026-12-28 23:58）==');
  console.log(JSON.stringify(r.real));
  console.log('\n== 字型敏感度（強制 body font-family，最寬字串）==');
  for (const [k, v] of Object.entries(r.fontVariants)) console.log(k, JSON.stringify(v));
  console.log('\n== 結論 ==');
  console.log(JSON.stringify(r.suggested), '\n高度：', JSON.stringify(r.height));
  console.log('\n== 正式版面核對（視窗 W x 156）==');
  for (const [k, v] of Object.entries(r.layouts)) console.log(k, JSON.stringify(v));
}
function m2(n) {
  return Math.round(n * 1000) / 1000;
}
