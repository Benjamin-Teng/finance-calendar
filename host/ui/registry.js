// host/ui/registry.js
//
// 十個小工具的固定清單（design.md D6）：`id`、訂閱通道、預設開關。**這是「小工具內容如何
// 顯示」的權威清單**——頁面只負責填滿核心給的視窗，不需要知道任何尺寸。
//
// 版面欄位只放 Rust（design.md D6，task 7.2 起）：
//   - host/src/widgets.rs 的 `WIDGET_SPECS`：id → 訂閱通道、設計寬度（倍率 1 時的邏輯寬度）、
//     設計最小高度。
//   - host/src/settings.rs 的 `WIDGET_IDS`／`DEFAULT_GRID_RECTS`：id → 預設開關與預設格座標
//     `{ col, row, w, h }`（48×48 格線）。
// 三份清單的 `id` 集合與「id → 通道」必須互相一致；Rust 單元測試
// `widgets::tests::specs_match_frontend_registry_js` 直接解析本檔核對 id 與通道，並斷言本檔
// 不再出現寬度／高度上限／預設位置等版面欄位。

export const WIDGETS = [
  { id: 'clock', channel: 'tw-events', defaultEnabled: true },
  { id: 'macro', channel: 'tw-events', defaultEnabled: true },
  { id: 'fixed', channel: 'tw-events', defaultEnabled: true },
  { id: 'dynamic', channel: 'tw-events', defaultEnabled: true },
  { id: 'quotes', channel: 'tw-events', defaultEnabled: true },
  ...['custom1', 'custom2', 'custom3', 'custom4', 'custom5'].map((id) => ({
    id,
    channel: id,
    defaultEnabled: false,
  })),
];

/** 依 id 查小工具設定；找不到回傳 `null`（widget.html 據此顯示錯誤，不當掉）。 */
export function getWidget(id) {
  return WIDGETS.find((w) => w.id === id) ?? null;
}

/** 十個 id 的清單（沿用清單原本的順序），供需要列舉全部小工具的地方使用
 * （例如設定視窗）。 */
export const WIDGET_IDS = WIDGETS.map((w) => w.id);
