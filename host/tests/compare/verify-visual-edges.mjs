#!/usr/bin/env node
// host/tests/compare/verify-visual-edges.mjs
//
// 小工具視窗邊緣視覺檢查（visual fix：陰影被視窗邊緣裁成方框、行情條面板不填滿格子）。
//
// 每個小工具在宿主裡是一扇透明無邊框視窗、大小＝格子。本腳本以 headless Edge 透明背景
// （CDP `Emulation.setDefaultBackgroundColorOverride` alpha=0）、用實機格子尺寸
// （CDP `Emulation.setDeviceMetricsOverride`，device scale factor 依宿主 ZoomFactor 換算，見
// 下方 WIDGETS；避開 headless viewport 比 --window-size 小的問題）開 `widget.html?w=<id>`（fixture 模式），截圖後以程式
// 讀 PNG：
//   - 四條邊最外 1px 的 alpha 最大值：> 0 代表面板或陰影碰到視窗邊緣，會被裁成硬邊。
//   - 面板（alpha ≥ PANEL_ALPHA_MIN 的區域）沿中線到視窗上下左右緣的距離（實體 px）。
// 另截兩張編輯版面圖（行情條有資料／無資料佔位外框），確認虛線外框仍畫在視窗內側：只看四邊
// 中段的外框像素（`frameSideVisible`，四角把手不算），並以「拿掉外框、只留把手」的負向對照
// 確認這項檢查不是空真（fix F6）。尺寸由 Rust 預設格座標推導（`widgetGeometry`）。
//
// 用法：
//   node host/tests/compare/verify-visual-edges.mjs --label before
//   node host/tests/compare/verify-visual-edges.mjs --label after --assert
// `--assert`：任一小工具邊緣 alpha 非 0、或任一小工具（含行情條夾在 3.0）的面板邊距不等於
// round(gap × 縮放比例 × 該小工具的 content_zoom) ±1 實體 px 時 exit 1（`expectedPanelInsetPx`／`panelInsetProblems`）。
// 截圖存 host/tools/evidence/visual-fix-<id>-<label>.png。

import path from 'node:path';
import { readFileSync } from 'node:fs';
import { writeFile } from 'node:fs/promises';
import { inflateSync } from 'node:zlib';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { startServer } from './serve.mjs';
import { launchEdge, newTarget, closeTarget, CDPTarget, waitForPageCondition, evaluate } from './cdp.mjs';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(__dirname, '..', '..', '..');
const DEFAULT_FIXTURE = path.join(REPO_ROOT, 'host', 'ui', 'fixtures', 'tw-events.json');
const EMPTY_QUOTES_FIXTURE = path.join(__dirname, 'fixtures', 'tw-events-empty-quotes.json');
const EVIDENCE_DIR = path.join(REPO_ROOT, 'host', 'tools', 'evidence');

// 實機格子尺寸（實體 px，4K@150%，工作區 3840×2088）。fix F6：從 Rust 的預設格座標
// （host/src/settings.rs `DEFAULT_GRID_RECTS`，順序同 `WIDGET_IDS`）與設計寬度
// （host/src/widgets.rs `WIDGET_SPECS`）推導，不寫死——預設格改了，量的就是新尺寸。
// 格線像素與 host/src/layout.rs `edge`／`grid_rect_to_physical` 相同：floor(i × 長度 ÷ 48)，
// 寬高＝兩條格線的差。
// 宿主對頁面套 WebView2 ZoomFactor＝`layout::content_zoom`（最小框＋舒適框＋字級，
// widget-adaptive-zoom-and-grid design.md D1，取代舊的「邏輯寬 ÷ 設計寬度」`grid_zoom`）。
// 頁面實際的 CSS viewport＝邏輯尺寸 ÷ 倍率，即「實體 px ÷ (縮放比例 × 倍率)」，所以這裡以
// device scale factor＝縮放比例 × 倍率模擬，CSS 寬高＝實體寬高 ÷ 該比例（取整；CDP 要整數）。
// 倍率設計框（`ZoomBox`）由 `widgetZoomBoxes` 從 widgets.rs 的 `WIDGET_SPECS` 讀出，不寫死。
const WORK_AREA_4K = { width: 3840, height: 2088 };
const SCALE_4K_150 = 1.5; // 上面實機尺寸的螢幕縮放比例
const GRID = 48;
const FINANCE_IDS = ['clock', 'macro', 'fixed', 'dynamic', 'quotes'];

function rustSource(rel) {
  return readFileSync(path.join(REPO_ROOT, 'host', 'src', rel), 'utf8');
}

/** 取 `text` 中 `name(` 之後到對應右括號之間的內容（括號配對），回傳頂層以逗號切開的引數。 */
function callArgs(text, name) {
  const open = text.indexOf(`${name}(`);
  if (open < 0) return null;
  let depth = 0;
  let start = open + name.length + 1;
  const args = [];
  for (let i = start; i < text.length; i++) {
    const c = text[i];
    if (c === '(') depth++;
    else if (c === ')') {
      if (depth === 0) {
        args.push(text.slice(start, i).trim());
        return args;
      }
      depth--;
    } else if (c === ',' && depth === 0) {
      args.push(text.slice(start, i).trim());
      start = i + 1;
    }
  }
  throw new Error(`${name}( 沒有對應的右括號`);
}

/** host/src/widgets.rs 的 `WIDGET_GAP_CSS_PX`：`#widget-root` 四周的透明邊距（CSS 像素，同 widget.css `--widget-gap`）。 */
export function widgetGapCssPx() {
  const gapMatch = rustSource('widgets.rs').match(/pub const WIDGET_GAP_CSS_PX: f64 = ([\d.]+);/);
  if (!gapMatch) throw new Error('widgets.rs 找不到 WIDGET_GAP_CSS_PX');
  return Number(gapMatch[1]);
}

/**
 * 面板邊距的預期值（實體 px）＝round(gap × device scale factor)，device scale factor＝螢幕縮放比例 ×
 * 該小工具的 `content_zoom`。widget-adaptive-zoom-and-grid design.md D1 起各小工具倍率各自決定，邊距的
 * 實體像素因此彼此不同（舊判準「與 clock 相差 ±1 px」只在倍率都約 1.7 的舊模型成立）；CSS 上仍一律是 gap。
 */
export function expectedPanelInsetPx(dsf, gap = widgetGapCssPx()) {
  return Math.round(gap * dsf);
}

/**
 * 四邊實測面板邊距（`measure().panelInsetPx`）與預期值相差超過 `tol` 實體 px 的描述清單（空＝通過）。
 * 找不到面板（null）也算問題。抓的缺陷：面板沒填滿格子（邊距變大）、gap 被吃掉（邊距 0 或變小）、四邊不一致。
 */
export function panelInsetProblems(inset, expected, tol = 1) {
  const problems = [];
  for (const side of ['top', 'bottom', 'left', 'right']) {
    const v = inset[side];
    if (v === null || v === undefined || Math.abs(v - expected) > tol) {
      problems.push(`面板 ${side} 邊距 ${v}px，預期 ${expected}±${tol}px`);
    }
  }
  return problems;
}

/** 求值 Rust 常數運算式：只支援數字、`WIDGET_GAP_CSS_PX`、`+`、`*`（widgets.rs 的寫法）。 */
function evalLength(expr, gap) {
  const src = expr.replace(/WIDGET_GAP_CSS_PX/g, String(gap));
  if (!/^[\d.\s+*]+$/.test(src)) throw new Error(`看不懂的長度運算式：${expr}`);
  return src.split('+').reduce((sum, term) => sum + term.split('*').reduce((p, f) => p * Number(f), 1), 0);
}

/**
 * 從 host/src/widgets.rs 的 `WIDGET_SPECS` 讀出五個財經小工具的倍率設計框（`ZoomBox`）。
 * 支援 `list_box(min寬, 面板最小高, comfort寬)`（min 高＝面板最小高＋2×`WIDGET_GAP_CSS_PX`、
 * comfort 高＝1.5×min 高，同 widgets.rs `list_box`）與 `zoom_box(min寬, min高, Some(寬)|None, 高)`。
 * 長度單位為邏輯（CSS）像素；`comfortWidth` 為 `null` 表示寬度不限制倍率（行情條）。
 */
export function widgetZoomBoxes() {
  const widgetsRs = rustSource('widgets.rs');
  const gap = widgetGapCssPx();
  const boxes = {};
  for (const id of FINANCE_IDS) {
    const head = widgetsRs.match(new RegExp(`finance_spec\\(\\s*"${id}",\\s*"[^"]*",\\s*`));
    if (!head) throw new Error(`WIDGET_SPECS 找不到 ${id}`);
    const rest = widgetsRs.slice(head.index + head[0].length);
    if (rest.startsWith('list_box(')) {
      const [minWidth, panelMinHeight, comfortWidth] = callArgs(rest, 'list_box').map((a) => evalLength(a, gap));
      const minHeight = panelMinHeight + 2 * gap;
      boxes[id] = { minWidth, minHeight, comfortWidth, comfortHeight: minHeight * 1.5 };
    } else if (rest.startsWith('zoom_box(')) {
      const [minWidth, minHeight, comfort, comfortHeight] = callArgs(rest, 'zoom_box');
      const some = comfort.match(/^Some\((.*)\)$/s);
      if (!some && comfort !== 'None') throw new Error(`${id} 的 comfort_width 看不懂：${comfort}`);
      boxes[id] = {
        minWidth: evalLength(minWidth, gap),
        minHeight: evalLength(minHeight, gap),
        comfortWidth: some ? evalLength(some[1], gap) : null,
        comfortHeight: evalLength(comfortHeight, gap),
      };
    } else {
      throw new Error(`${id} 的倍率框不是 list_box／zoom_box：${rest.slice(0, 40)}`);
    }
  }
  return boxes;
}

/** 正數長度防護：非有限值或 ≤ 0 視為 1.0（同 layout.rs `sane_positive_len`／`sane_scale`）。 */
function saneLength(v) {
  return Number.isFinite(v) && v > 0 ? v : 1;
}

/**
 * 與 Rust `layout::content_zoom` 相同的公式（widget-adaptive-zoom-and-grid design.md D1）：
 * auto＝min(邏輯寬÷comfort 寬（有才算）, 邏輯高÷comfort 高)、cap＝min(邏輯寬÷min 寬（comfort 寬
 * 有才算）, 邏輯高÷min 高)，zoom＝clamp(min(auto×字級, cap), 0.5, 3.0)。
 */
export function contentZoom(physW, physH, scale, box, fontScale = 1) {
  const s = saneLength(scale);
  const logicalW = physW / s;
  const logicalH = physH / s;
  const heightAuto = logicalH / saneLength(box.comfortHeight);
  const heightCap = logicalH / saneLength(box.minHeight);
  const [auto, cap] =
    box.comfortWidth !== null
      ? [
          Math.min(logicalW / saneLength(box.comfortWidth), heightAuto),
          Math.min(logicalW / saneLength(box.minWidth), heightCap),
        ]
      : [heightAuto, heightCap];
  return Math.min(Math.max(Math.min(auto * saneLength(fontScale), cap), 0.5), 3);
}

/** 實體矩形 → 倍率與 CDP 模擬參數（device scale factor＝縮放比例 × 倍率；CSS 寬高＝實體 ÷ 它）。 */
export function rectGeometry(box, physW, physH, scale = SCALE_4K_150, fontScale = 1) {
  const zoom = contentZoom(physW, physH, scale, box, fontScale);
  const dsf = saneLength(scale) * zoom;
  return { zoom, dsf, w: Math.round(physW / dsf), h: Math.round(physH / dsf) };
}

/** 由 Rust 原始碼推導五個財經小工具在 4K@150% 的實機尺寸、倍率與 device scale factor。 */
export function widgetGeometry(workArea = WORK_AREA_4K, scale = SCALE_4K_150, fontScale = 1) {
  const settingsRs = rustSource('settings.rs');
  const boxes = widgetZoomBoxes();
  const idsBlock = settingsRs.match(/pub const WIDGET_IDS: \[&str; \d+\] = \[([^\]]*)\]/);
  if (!idsBlock) throw new Error('settings.rs 找不到 WIDGET_IDS');
  const ids = [...idsBlock[1].matchAll(/"([a-z0-9]+)"/g)].map((m) => m[1]);
  const rectsBlock = settingsRs.match(/pub const DEFAULT_GRID_RECTS: \[GridRect; \d+\] = \[([\s\S]*?)\n\];/);
  if (!rectsBlock) throw new Error('settings.rs 找不到 DEFAULT_GRID_RECTS');
  const rects = [...rectsBlock[1].matchAll(/grid\((\d+),\s*(\d+),\s*(\d+),\s*(\d+)\)/g)].map((m) =>
    m.slice(1, 5).map(Number),
  );
  if (rects.length !== ids.length) throw new Error(`預設格數量 ${rects.length} 與 id 數量 ${ids.length} 不符`);
  const edge = (i, extent) => Math.floor((i * extent) / GRID);
  return FINANCE_IDS.map((id) => {
    const index = ids.indexOf(id);
    if (index < 0) throw new Error(`WIDGET_IDS 沒有 ${id}`);
    const [col, row, w, h] = rects[index];
    const physW = edge(col + w, workArea.width) - edge(col, workArea.width);
    const physH = edge(row + h, workArea.height) - edge(row, workArea.height);
    return { id, grid: [col, row, w, h], physW, physH, ...rectGeometry(boxes[id], physW, physH, scale, fontScale) };
  });
}
// 面板背景 alpha 預設 0.55（截圖實測面板區 alpha≈149）；修正前的陰影在視窗邊緣量到約 119，
// 用 130 分界面板與陰影。
const PANEL_ALPHA_MIN = 130;

function parseArgs(argv) {
  const args = { label: 'run', assert: false };
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === '--label') args.label = argv[++i];
    else if (argv[i] === '--assert') args.assert = true;
    else throw new Error(`未知參數：${argv[i]}`);
  }
  if (!/^[a-z0-9-]+$/.test(args.label)) throw new Error(`--label 只能用小寫英數與 -：${args.label}`);
  return args;
}

/** 最小 PNG 解碼器：只支援 8-bit RGBA（color type 6）、非交錯——CDP 透明背景截圖即此格式。 */
export function decodePng(buf) {
  const sig = '89504e470d0a1a0a';
  if (buf.subarray(0, 8).toString('hex') !== sig) throw new Error('不是 PNG');
  let off = 8;
  let width = 0;
  let height = 0;
  const idat = [];
  while (off < buf.length) {
    const len = buf.readUInt32BE(off);
    const type = buf.toString('latin1', off + 4, off + 8);
    const data = buf.subarray(off + 8, off + 8 + len);
    if (type === 'IHDR') {
      width = data.readUInt32BE(0);
      height = data.readUInt32BE(4);
      const bitDepth = data[8];
      const colorType = data[9];
      const interlace = data[12];
      if (bitDepth !== 8 || colorType !== 6 || interlace !== 0) {
        throw new Error(`不支援的 PNG 格式：bitDepth=${bitDepth} colorType=${colorType} interlace=${interlace}`);
      }
    } else if (type === 'IDAT') {
      idat.push(data);
    } else if (type === 'IEND') {
      break;
    }
    off += 12 + len;
  }
  const raw = inflateSync(Buffer.concat(idat));
  const bpp = 4;
  const stride = width * bpp;
  const out = Buffer.alloc(stride * height);
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)];
    const line = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
    for (let x = 0; x < stride; x++) {
      const a = x >= bpp ? out[y * stride + x - bpp] : 0;
      const b = y > 0 ? out[(y - 1) * stride + x] : 0;
      const c = x >= bpp && y > 0 ? out[(y - 1) * stride + x - bpp] : 0;
      let v = line[x];
      if (filter === 1) v += a;
      else if (filter === 2) v += b;
      else if (filter === 3) v += (a + b) >> 1;
      else if (filter === 4) {
        const p = a + b - c;
        const pa = Math.abs(p - a);
        const pb = Math.abs(p - b);
        const pc = Math.abs(p - c);
        v += pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
      } else if (filter !== 0) throw new Error(`未知 PNG filter ${filter}`);
      out[y * stride + x] = v & 0xff;
    }
  }
  return { width, height, alpha: (x, y) => out[(y * width + x) * 4 + 3] };
}

function measure(img) {
  const { width: W, height: H, alpha } = img;
  let top = 0;
  let bottom = 0;
  let left = 0;
  let right = 0;
  for (let x = 0; x < W; x++) {
    top = Math.max(top, alpha(x, 0));
    bottom = Math.max(bottom, alpha(x, H - 1));
  }
  for (let y = 0; y < H; y++) {
    left = Math.max(left, alpha(0, y));
    right = Math.max(right, alpha(W - 1, y));
  }
  const cx = Math.floor(W / 2);
  const cy = Math.floor(H / 2);
  const firstFrom = (n, get) => {
    for (let i = 0; i < n; i++) if (get(i) >= PANEL_ALPHA_MIN) return i;
    return null;
  };
  const inset = {
    top: firstFrom(H, (i) => alpha(cx, i)),
    bottom: firstFrom(H, (i) => alpha(cx, H - 1 - i)),
    left: firstFrom(W, (i) => alpha(i, cy)),
    right: firstFrom(W, (i) => alpha(W - 1 - i, cy)),
  };
  return { size: `${W}x${H}`, edgeAlphaMax: { top, bottom, left, right }, panelInsetPx: inset };
}

// 編輯外框可見性（fix F6）：只取每條邊中段 25%–75%（避開四角 14 CSS px 的調整大小角標與
// 16 px 圓角），看最外 FRAME_BAND_PX 列／行；該位置任一像素 alpha ≥ FRAME_ALPHA_MIN 就算畫到。
// 2px 虛線約一半長度有實線，畫到的比例 ≥ FRAME_MIN_RATIO 才算看得到外框本身。
const SIDES = ['top', 'bottom', 'left', 'right'];
const FRAME_BAND_PX = 2;
const FRAME_ALPHA_MIN = 100;
const FRAME_MIN_RATIO = 0.2;

/** 編輯版面虛線外框在 `side` 邊中段是否可見（只看外框本身，四角把手不算）。 */
export function frameSideVisible(img, side) {
  const { width: W, height: H, alpha } = img;
  const horizontal = side === 'top' || side === 'bottom';
  const length = horizontal ? W : H;
  const from = Math.floor(length * 0.25);
  const to = Math.ceil(length * 0.75);
  let painted = 0;
  for (let i = from; i < to; i++) {
    let hit = false;
    for (let d = 0; d < FRAME_BAND_PX && !hit; d++) {
      const [x, y] =
        side === 'top' ? [i, d] : side === 'bottom' ? [i, H - 1 - d] : side === 'left' ? [d, i] : [W - 1 - d, i];
      hit = alpha(x, y) >= FRAME_ALPHA_MIN;
    }
    if (hit) painted++;
  }
  return to > from && painted / (to - from) >= FRAME_MIN_RATIO;
}

async function capture(edge, server, { id, w, h, dsf }, { fixturesPath = '/fixtures/', setup = null } = {}) {
  const target = await newTarget(edge.port, 'about:blank');
  const session = new CDPTarget(target.webSocketDebuggerUrl);
  try {
    await session.connect();
    await session.send('Page.enable');
    await session.send('Emulation.setDeviceMetricsOverride', {
      width: w,
      height: h,
      deviceScaleFactor: dsf,
      mobile: false,
    });
    await session.send('Emulation.setDefaultBackgroundColorOverride', { color: { r: 0, g: 0, b: 0, a: 0 } });
    await session.send('Page.navigate', { url: `${server.url}/new/widget.html?w=${id}&fixtures=${fixturesPath}` });
    await waitForPageCondition(
      session,
      "!!document.getElementById('widget-root') && document.getElementById('widget-root').children.length > 0",
    );
    await new Promise((r) => setTimeout(r, 400));
    if (setup) {
      await evaluate(session, setup);
      await new Promise((r) => setTimeout(r, 300));
    }
    // 行情條跑馬燈會動，暫停後再截圖，避免兩次截圖間內容位移（不影響邊緣量測）。
    await evaluate(session, "window.__bridgeTest && window.__bridgeTest.emit('pause', {paused:true, reason:'shot'})");
    const viewport = await evaluate(session, '[innerWidth, innerHeight]');
    const shot = await session.send('Page.captureScreenshot', { format: 'png' });
    return { png: Buffer.from(shot.data, 'base64'), viewport };
  } finally {
    session.close();
    await closeTarget(edge.port, target.id);
  }
}

// 編輯版面＋解鎖（可拖、有調整大小把手）：沿用 bridge.js fixture 模式的 `__bridgeTest.emit`。
const EDIT_SETUP = `(async () => {
  const bridge = await import('/new/bridge.js');
  const s = await bridge.getSettings();
  window.__bridgeTest.emit('settings', Object.assign({}, s, { layout_locked: false }));
  window.__bridgeTest.emit('edit-mode', true);
  return true;
})()`;

// 負向對照（fix F6）：同樣進編輯版面，但把虛線外框（`body::before` 與佔位外框）改成透明；
// 四角角標仍在。此時外框檢查必須判為不可見，否則檢查本身是空真。
const EDIT_SETUP_NO_FRAME = `(async () => {
  await ${EDIT_SETUP};
  const style = document.createElement('style');
  style.textContent = 'body.edit-mode::before, .edit-placeholder { border-color: transparent !important; }';
  document.head.appendChild(style);
  return true;
})()`;

async function run() {
  const args = parseArgs(process.argv.slice(2));
  const WIDGETS = widgetGeometry();
  const edge = await launchEdge();
  const server = await startServer(DEFAULT_FIXTURE);
  const emptyServer = await startServer(EMPTY_QUOTES_FIXTURE);
  const results = {};
  const failures = [];
  try {
    for (const spec of WIDGETS) {
      const { png, viewport } = await capture(edge, server, spec);
      const file = path.join(EVIDENCE_DIR, `visual-fix-${spec.id}-${args.label}.png`);
      await writeFile(file, png);
      const m = measure(decodePng(png));
      results[spec.id] = m;
      console.log(
        `[visual] ${spec.id} grid=${spec.grid.join(',')} phys=${spec.physW}x${spec.physH} ` +
          `viewport=${viewport.join('x')} png=${m.size} ` +
          `edgeAlphaMax=${JSON.stringify(m.edgeAlphaMax)} panelInsetPx=${JSON.stringify(m.panelInsetPx)}`,
      );
    }

    const editShots = [
      { name: 'quotes-edit', server, spec: WIDGETS.find((s) => s.id === 'quotes') },
      { name: 'quotes-empty-edit', server: emptyServer, spec: WIDGETS.find((s) => s.id === 'quotes') },
    ];
    for (const shotSpec of editShots) {
      const { png } = await capture(edge, shotSpec.server, shotSpec.spec, { setup: EDIT_SETUP });
      await writeFile(path.join(EVIDENCE_DIR, `visual-fix-${shotSpec.name}-${args.label}.png`), png);
      const img = decodePng(png);
      const m = measure(img);
      results[shotSpec.name] = m;
      const frame = Object.fromEntries(SIDES.map((side) => [side, frameSideVisible(img, side)]));
      console.log(
        `[visual] ${shotSpec.name} edgeAlphaMax=${JSON.stringify(m.edgeAlphaMax)} 外框中段可見=${JSON.stringify(frame)}`,
      );
      // 虛線外框（金色、畫在視窗內側）必須看得到：四邊中段（避開四角把手）都要有外框像素。
      for (const side of SIDES) {
        if (!frame[side]) failures.push(`${shotSpec.name}：編輯版面外框在 ${side} 邊中段不可見`);
      }
    }

    // 行情條 zoom 被夾在上限 3.0（fix F6，review monitorid-visual low；widget-adaptive-zoom-and-grid
    // task 4.2 依 `content_zoom` 重算）：行情條倍率只看高度（寬度不限制），4K＠100% 拉滿 48 欄寬、
    // 高度約 5 格（實體 217 px）＝邏輯高 217，217÷60＝3.6 超過 3.0 被夾住，頁面 CSS viewport
    // 寬＝3840÷3＝1280，比最小框寬 992 寬。面板仍要填滿（左右內距相同），不可停在 992 靠左。
    {
      const quotes = WIDGETS.find((s) => s.id === 'quotes');
      const clamped = {
        ...quotes,
        physW: 3840,
        physH: 217,
        ...rectGeometry(widgetZoomBoxes().quotes, 3840, 217, 1, 1),
      };
      const { png, viewport } = await capture(edge, server, clamped);
      await writeFile(path.join(EVIDENCE_DIR, `visual-fix-quotes-clamped-${args.label}.png`), png);
      const m = measure(decodePng(png));
      results['quotes-clamped'] = m;
      console.log(
        `[visual] quotes-clamped（zoom 夾在 3.0）viewport=${viewport.join('x')} png=${m.size} ` +
          `edgeAlphaMax=${JSON.stringify(m.edgeAlphaMax)} panelInsetPx=${JSON.stringify(m.panelInsetPx)}`,
      );
      const { left, right } = m.panelInsetPx;
      if (left === null || right === null || Math.abs(left - right) > 1) {
        failures.push(`quotes-clamped：面板左右內距 ${left}／${right} px 不一致（面板沒有隨視窗寬度填滿）`);
      }
    }

    // 負向對照：拿掉虛線外框只留四角把手，檢查必須判為不可見（證明上面的檢查不是空真）。
    {
      const quotes = WIDGETS.find((s) => s.id === 'quotes');
      const { png } = await capture(edge, server, quotes, { setup: EDIT_SETUP_NO_FRAME });
      const img = decodePng(png);
      const frame = Object.fromEntries(SIDES.map((side) => [side, frameSideVisible(img, side)]));
      console.log(`[visual] negative-control（無外框、只有把手）外框中段可見=${JSON.stringify(frame)}（預期全 false）`);
      for (const side of SIDES) {
        if (frame[side]) failures.push(`負向對照：拿掉外框後 ${side} 邊仍判為可見，外框檢查是空真`);
      }
    }

    if (args.assert) {
      for (const spec of WIDGETS) {
        const m = results[spec.id];
        for (const [side, a] of Object.entries(m.edgeAlphaMax)) {
          if (a !== 0) failures.push(`${spec.id}：${side} 邊最外 1px alpha=${a}（應為 0）`);
        }
      }
      // 面板邊距＝round(gap × 縮放比例 × 該小工具的 content_zoom) ±1 實體 px（含行情條夾在 3.0 的情境）。
      const gap = widgetGapCssPx();
      const insetCases = [
        ...WIDGETS.map((spec) => ({ name: spec.id, dsf: spec.dsf })),
        { name: 'quotes-clamped', dsf: rectGeometry(widgetZoomBoxes().quotes, 3840, 217, 1, 1).dsf },
      ];
      for (const c of insetCases) {
        const expected = expectedPanelInsetPx(c.dsf, gap);
        const problems = panelInsetProblems(results[c.name].panelInsetPx, expected);
        console.log(
          `[visual] ${c.name} 面板邊距 ${JSON.stringify(results[c.name].panelInsetPx)} 預期 ${expected}±1px` +
            `（gap ${gap} × dsf ${c.dsf.toFixed(4)}）${problems.length ? ' ✗' : ' ✓'}`,
        );
        for (const p of problems) failures.push(`${c.name}：${p}`);
      }
    }
  } finally {
    await server.close();
    await emptyServer.close();
    await edge.close();
  }
  if (failures.length) {
    console.log(`[visual] FAIL\n  - ${failures.join('\n  - ')}`);
    process.exit(1);
  }
  console.log('[visual] PASS');
}

// 只在直接執行時跑（host/tests/visual-edges-geometry.test.mjs 會 import 本檔的純函式）。
if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  run().catch((e) => {
    console.error(e);
    process.exit(1);
  });
}
