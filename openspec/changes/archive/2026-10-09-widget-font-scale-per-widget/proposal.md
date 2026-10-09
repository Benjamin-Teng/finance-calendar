## Why

v0.2.0 上線後，使用者把總經日曆放在又窄又高的框（150% 縮放下邏輯 320×624），字級由寬度決定只有 0.64 倍、下方大片空白；全域字級即使開到 150% 也被清單的「塞得下」寬度上限（最小框寬＝0.75 × 設計寬）卡在約 0.85 倍。使用者（2026-10-09）要求每個小工具各自調整字級，且在編輯版面時直接在小工具上調。

## What Changes

- **BREAKING（設定檔格式）**：全域 `font_scale` 改為每個小工具各自的 `widgets.<id>.font_scale`；載入 v0.2.0 設定檔時把全域值帶入各小工具，之後不再寫出全域欄位。範圍由 70–150%／0.05 改為 50–300%／0.1。
- 設定視窗移除全域字級滑桿，改為提示「在編輯版面中調整各小工具字級」。
- 編輯版面時每個小工具顯示 A−／目前百分比／A+ 按鈕；按下立即重排並保存；倍率已達上限時 A+ 停用並提示拉大框。新增後端指令，只在編輯版面且未鎖定時接受。
- 清單類小工具（總經日曆、台股固定／動態事件、擴充插槽）的列在窄寬度改為收合版面（數值移到標題下一行、標題可換行），清單最小框寬度依實測降到約設計寬的一半，讓窄框也能放大字級。舒適框不變，字級 100% 時外觀不變。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `widget-host-windows`：倍率公式中的字級改為每個小工具各自設定；新增「編輯版面調整個別字級」。
- `widget-host-lifecycle`：「設定持久化」的字級改為逐小工具、範圍改變、v0.2.0 全域字級遷移。
- `finance-widgets`：新增「清單在窄寬度收合」。

## Impact

- Rust：`settings.rs`（欄位搬移、sanitize、遷移、移除全域欄位與 patch 入口）、`layout.rs`（`GridWidgetInput` 帶各自字級、`resolve_grid_placements` 簽名）、`widgets.rs`（`WIDGET_SPECS` 清單最小框寬、新指令、所有倍率計算點改取各自字級、回報是否達上限）、`self_test_ipc.rs` 若用到。
- 前端：`host/ui/settings.html`（移除滑桿、加提示）、`host/ui/widget.html`／`widget.css`／`bridge.js`（編輯版面字級按鈕）、清單小工具的 CSS（收合版面）。
- 測試：上述模組單元測試、`host/tests/` 前端測試、`verify-visual-edges.mjs` 與 `verify-adaptive-zoom.ps1` 的數值與情境。
- 文件：`AGENTS.md`、`host/README.md`、`host/tools/README.md`。
