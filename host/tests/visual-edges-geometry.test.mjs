// host/tests/visual-edges-geometry.test.mjs
//
// 回歸測試：host/tests/compare/verify-visual-edges.mjs 的兩個純函式（fix F6，review
// monitorid-visual low／dragdpi low）。
//   1. `widgetGeometry`：小工具實機尺寸要從 Rust 的預設格座標（host/src/settings.rs
//      `DEFAULT_GRID_RECTS`）與設計寬度（host/src/widgets.rs `WIDGET_SPECS`）推導，不可寫死
//      ——預設格改了（fix min-height：時鐘 16×10、總經日曆 15,12,16,30），腳本要跟著量新尺寸。
//   2. `frameSideVisible`：編輯版面虛線外框要驗「外框本身」的像素（四邊中段，避開四角的調整
//      大小角標），只有四角有不透明像素時必須判為不可見——原本只看整條邊的 alpha 最大值，
//      四角把手就能讓檢查空真通過。
//
// 執行：node host/tests/visual-edges-geometry.test.mjs
// 通過條件：全部斷言成立，缺一個就 FAIL、exit 1。

import { widgetGeometry, frameSideVisible } from './compare/verify-visual-edges.mjs';

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
assertEqual(
  geo.map((g) => g.designW),
  [500, 500, 470, 470, 992],
  '設計寬度取自 WIDGET_SPECS',
);

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
