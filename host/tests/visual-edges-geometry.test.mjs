// host/tests/visual-edges-geometry.test.mjs
//
// 回歸測試：host/tests/compare/verify-visual-edges.mjs 的兩個純函式（fix F6，review
// monitorid-visual low／dragdpi low）。
//   1. `widgetGeometry`：小工具實機尺寸要從 Rust 的預設格座標（host/src/settings.rs
//      `DEFAULT_GRID_RECTS`）與倍率設計框（host/src/widgets.rs `WIDGET_SPECS` 的 `ZoomBox`）
//      推導，不可寫死——預設格改了（fix min-height：時鐘 16×10、總經日曆 15,12,16,30），
//      腳本要跟著量新尺寸。倍率公式與 Rust `layout::content_zoom` 相同（含高度、字級；
//      widget-adaptive-zoom-and-grid task 4.2），`contentZoom` 的案例數值同 layout.rs 單元測試。
//   2. `frameSideVisible`：編輯版面虛線外框要驗「外框本身」的像素（四邊中段，避開四角的調整
//      大小角標），只有四角有不透明像素時必須判為不可見——原本只看整條邊的 alpha 最大值，
//      四角把手就能讓檢查空真通過。
//
// 執行：node host/tests/visual-edges-geometry.test.mjs
// 通過條件：全部斷言成立，缺一個就 FAIL、exit 1。

import {
  widgetGeometry,
  widgetZoomBoxes,
  contentZoom,
  rectGeometry,
  frameSideVisible,
  widgetGapCssPx,
  expectedPanelInsetPx,
  panelInsetProblems,
} from './compare/verify-visual-edges.mjs';

let failures = 0;

function assertEqual(actual, expected, label) {
  const ok = JSON.stringify(actual) === JSON.stringify(expected);
  console.log(`${ok ? 'PASS' : 'FAIL'}  ${label}（實際=${JSON.stringify(actual)} 期望=${JSON.stringify(expected)}）`);
  if (!ok) failures++;
}

// ── 1. 4K＠150%（工作區 3840×2088）上的實機尺寸 ─────────────────────────────────────
const geo = widgetGeometry();
const byId = Object.fromEntries(geo.map((g) => [g.id, g]));
assertEqual(geo.map((g) => g.id), ['clock', 'macro', 'fixed', 'dynamic', 'quotes'], '只取五個財經小工具、順序同 WIDGET_IDS');
assertEqual([byId.clock.physW, byId.clock.physH], [1280, 435], 'clock 預設 (15,1,16,10) → 1280×435');
assertEqual([byId.macro.physW, byId.macro.physH], [1280, 1305], 'macro 預設 (15,12,16,30) → 1280×1305');
assertEqual([byId.fixed.physW, byId.fixed.physH], [1200, 740], 'fixed 預設 (32,1,15,17) → 1200×740');
assertEqual([byId.dynamic.physW, byId.dynamic.physH], [1200, 1001], 'dynamic 預設 (32,19,15,23) → 1200×1001');
assertEqual([byId.quotes.physW, byId.quotes.physH], [2560, 174], 'quotes 預設 (15,43,32,4) → 2560×174');

// 倍率設計框取自 WIDGET_SPECS（task 4.2；widget-adaptive-zoom-and-grid design.md D1 表格，清單
// min 寬為 widget-font-scale-per-widget task 3.1 實測值）。
assertEqual(
  widgetZoomBoxes(),
  {
    clock: { minWidth: 212, minHeight: 160, comfortWidth: 212, comfortHeight: 160 },
    macro: { minWidth: 229, minHeight: 216, comfortWidth: 500, comfortHeight: 324 },
    fixed: { minWidth: 285, minHeight: 176, comfortWidth: 470, comfortHeight: 264 },
    dynamic: { minWidth: 333, minHeight: 176, comfortWidth: 470, comfortHeight: 264 },
    quotes: { minWidth: 992, minHeight: 60, comfortWidth: null, comfortHeight: 60 },
  },
  '倍率設計框取自 WIDGET_SPECS（含 list_box／運算式／None）',
);

// 倍率＝Rust `layout::content_zoom`（同一組數值與 layout.rs 單元測試）。
const approx = (a, b) => Math.abs(a - b) < 1e-9;
function assertApprox(actual, expected, label) {
  const ok = approx(actual, expected);
  console.log(`${ok ? 'PASS' : 'FAIL'}  ${label}（實際=${actual} 期望=${expected}）`);
  if (!ok) failures++;
}
const LIST = { minWidth: 375, minHeight: 216, comfortWidth: 500, comfortHeight: 324 };
const CLOCK = { minWidth: 212, minHeight: 160, comfortWidth: 212, comfortHeight: 160 };
const TICKER = { minWidth: 992, minHeight: 60, comfortWidth: null, comfortHeight: 60 };
assertApprox(contentZoom(1000, 324, 1, LIST, 1), 1, '清單又寬又矮：被高度限制');
assertApprox(contentZoom(600, 400, 1, LIST, 1), 1.2, '清單 auto');
assertApprox(contentZoom(600, 400, 1, LIST, 1.2), 1.44, '字級 1.2 放大 auto');
assertApprox(contentZoom(600, 400, 1, LIST, 1.5), 1.6, '字級 1.5 被 cap 截到 1.6');
assertApprox(contentZoom(424, 320, 1, CLOCK, 1), 2, '時鐘寬高皆為最小框 2 倍');
assertApprox(contentZoom(530, 330, 1, CLOCK, 1), 330 / 160, '時鐘取較受限的一軸');
assertApprox(contentZoom(5000, 60, 1, TICKER, 1), 1, '行情條只看高度');
assertApprox(contentZoom(300, 120, 1, TICKER, 1), 2, '行情條寬度遠小於 min 寬仍只看高');
assertApprox(contentZoom(750, 486, 1.5, LIST, 1), 1, '先以螢幕縮放比例換成邏輯長度');
assertApprox(contentZoom(10, 10, 1, LIST, 1), 0.5, '夾下限 0.5');
assertApprox(contentZoom(5000, 5000, 1, CLOCK, 1), 3, '夾上限 3.0');
assertApprox(contentZoom(600, 400, 1, LIST, Number.NaN), 1.2, 'font_scale 非法視為 1.0');
assertApprox(contentZoom(600, 400, Number.NaN, LIST, 1), 1.2, '縮放比例非法視為 1.0');

// 4K＠150% 預設格上各小工具的倍率與 CDP 模擬參數（device scale factor＝縮放比例 × 倍率）。
const expectedZoom = {
  clock: 290 / 160, // 邏輯 853.3×290，高度決定
  macro: 1280 / 1.5 / 500, // 寬度決定
  fixed: 1200 / 1.5 / 470,
  dynamic: 1200 / 1.5 / 470,
  quotes: 116 / 60, // 邏輯高 116，寬度不限
};
for (const [id, z] of Object.entries(expectedZoom)) {
  assertApprox(byId[id].zoom, z, `${id} 倍率`);
  assertApprox(byId[id].dsf, 1.5 * z, `${id} device scale factor＝1.5 × 倍率`);
  assertEqual(
    [byId[id].w, byId[id].h],
    [Math.round(byId[id].physW / (1.5 * z)), Math.round(byId[id].physH / (1.5 * z))],
    `${id} CSS viewport＝實體尺寸 ÷ device scale factor`,
  );
}

// 行情條倍率夾在 3.0：4K＠100%、整個工作區寬、約 5 格高（實體 217）→ 高度算出 3.6，夾 3.0；
// 與寬度無關（舊模型看寬度，新模型只看高度）。
const clamped = rectGeometry(widgetZoomBoxes().quotes, 3840, 217, 1, 1);
assertEqual([clamped.zoom, clamped.dsf, clamped.w, clamped.h], [3, 3, 1280, 72], '行情條倍率夾在 3.0 的 viewport');

// ── 1b. 面板邊距判準（task 6.3 修正）：每個小工具＝round(gap × 縮放比例 × 自己的倍率) ±1 實體 px ──
assertEqual(widgetGapCssPx(), 8, 'gap 取自 widgets.rs WIDGET_GAP_CSS_PX');
assertEqual(expectedPanelInsetPx(byId.clock.dsf), 22, 'clock 預期邊距 round(8 × 1.5 × 1.8125)＝22');
assertEqual(expectedPanelInsetPx(byId.macro.dsf), 20, 'macro 預期邊距 round(8 × 1.5 × 1.7067)＝20');
assertEqual(expectedPanelInsetPx(byId.quotes.dsf), 23, 'quotes 預期邊距 round(8 × 1.5 × 1.9333)＝23');
assertEqual(expectedPanelInsetPx(clamped.dsf), 24, 'quotes 夾在 3.0 時預期邊距 8 × 3＝24');
const sides = (t, b, l, r) => ({ top: t, bottom: b, left: l, right: r });
// 2026-10-08 實跑量到的邊距（倍率各自不同，實體 px 彼此不同）都要通過——舊判準「與 clock 相差 ±1」會誤報。
assertEqual(panelInsetProblems(sides(22, 22, 22, 23), 22), [], 'clock 實測 22/22/22/23 通過');
assertEqual(panelInsetProblems(sides(21, 21, 21, 21), 20), [], 'macro 實測 21 對預期 20 在 ±1 內');
assertEqual(panelInsetProblems(sides(24, 24, 24, 24), 23), [], 'quotes 實測 24 對預期 23 在 ±1 內');
// 鑑別力：原本想抓的缺陷仍然抓得到。
assertEqual(panelInsetProblems(sides(0, 0, 0, 0), 23).length, 4, 'gap 被吃掉（面板貼邊）四邊都報');
assertEqual(panelInsetProblems(sides(23, 23, 23, 60), 23).length, 1, '面板沒填滿（右邊距變大）只報右邊');
assertEqual(panelInsetProblems(sides(23, 26, 23, 23), 23).length, 1, '上下邊距不一致（差 3 px）報下邊');
assertEqual(panelInsetProblems(sides(23, null, 23, 23), 23).length, 1, '找不到面板（null）算問題');
assertEqual(panelInsetProblems(sides(22, 22, 22, 22), 24).length, 4, '倍率沒套用（以 clock 的 22 量 quotes-clamped 預期 24）四邊都報');

// ── 2. 編輯外框可見性：只看四邊中段 ──────────────────────────────────────────────────
// 合成 200×100 的 alpha 圖：`paint(x, y)` 回傳該點 alpha。
function image(paint) {
  return { width: 200, height: 100, alpha: (x, y) => paint(x, y) };
}
const cornersOnly = image((x, y) => ((x < 20 || x >= 180) && (y < 20 || y >= 80) ? 255 : 0));
const dashedFrame = image((x, y) => {
  const onEdge = x < 2 || x >= 198 || y < 2 || y >= 98;
  const along = x < 2 || x >= 198 ? y : x;
  return onEdge && along % 8 < 4 ? 220 : 0; // 4 px 實線、4 px 空隙
});
for (const side of ['top', 'bottom', 'left', 'right']) {
  assertEqual(frameSideVisible(cornersOnly, side), false, `只有四角角標時 ${side} 邊判為不可見`);
  assertEqual(frameSideVisible(dashedFrame, side), true, `虛線外框 ${side} 邊判為可見`);
}
assertEqual(frameSideVisible(image(() => 0), 'top'), false, '全透明判為不可見');

if (failures > 0) {
  console.log(`\n${failures} 個斷言失敗`);
  process.exit(1);
}
console.log('\n全部通過');
