## Context

動機見 proposal.md「Why」。現況（`desktop-widget-host` 歸檔 design.md D7）：

- 倍率只在 Rust 算：`layout::grid_zoom(實體寬, 縮放比例, 設計寬度)`＝clamp(邏輯寬 ÷ 設計寬度, 0.5, 3)，經 `widgets::apply_content_zoom` 以 WebView2 `SetZoomFactor` 套到整頁；頁面字級全是寫死的 px，靠 ZoomFactor 等比縮放。呼叫點：`resolve_grid_placements`、`DragSession::moving`（跨顯示器拖曳即時換算）、建立視窗、`report_content(true)` 重套、`relayout_all_widgets`。
- `WIDGET_SPECS` 每個小工具只有 `design_width`、`design_min_height` 兩個版面欄位。
- 最小格數：`min_w`＝邏輯寬 ≥ 設計寬 × 0.5；`min_h`＝邏輯高 ≥ 設計最小高 × zoom（zoom 取自寬度）。`meets_min_grid_size` 與 `min_grid_size` 同公式。
- `update_settings` 每次都會 `relayout_all_widgets`，設定新增欄位不必另接重排。
- `set_edit_mode` 是同步 Tauri 指令（主執行緒），切換後依序廣播 `settings`、`edit-mode`，再同步小工具顯示與 `WS_SIZEBOX`。暫停不隱藏小工具視窗（只讓頁面停更）。
- Win32 一律集中在 `host/src/desktop.rs` 與 `host/src/desktop/` 子模組。目前沒有任何可見的全工作區覆蓋視窗，也沒有用到 `WS_EX_LAYERED`／`WS_EX_TRANSPARENT`。

## Goals / Non-Goals

**Goals:**

- 倍率模型保持「Rust 單一來源、純函式、可屬性測試」，不在頁面量測內容。
- 最小格數只放寬、不收緊：既有合法的記錄位置在新規則下仍合法。
- 格線的每條線與吸附用的格線座標逐像素一致（同一個 `edge()`）。

**Non-Goals:**

- 每個小工具各自的字級（使用者已選全域微調）。
- 調整大小拖曳中即時重算倍率（現行 `WM_SIZING` 只更新紅框，維持；放開後由 relayout 套用）。
- 調整大小時加亮最小範圍（紅框預告已提示「會彈回」）。
- 動態桌布主題頁、Lively 版頁面。
- 提高倍率上限 3。

## Decisions

### D1 倍率模型：最小框＋舒適框

`WIDGET_SPECS` 的 `design_width`／`design_min_height` 改為一組 `ZoomBox`：

| 欄位 | 意義 |
|---|---|
| `min_width`、`min_height` | 最小框：倍率 1 時內容剛好塞得下的邏輯寬高（CSS 像素，高度含上下 `WIDGET_GAP_CSS_PX`）。決定上限與最小格數。 |
| `comfort_width: Option<f64>` | 舒適框寬；`None`＝寬度不限制倍率（跑馬燈），此時上限也不看寬。 |
| `comfort_height` | 舒適框高。 |

純函式 `layout::content_zoom(實體寬, 實體高, 縮放比例, &ZoomBox, font_scale) -> f64`：

```text
邏輯寬 = 實體寬 ÷ 縮放比例；邏輯高同理
auto = min(邏輯寬 ÷ comfort_width（有才算）, 邏輯高 ÷ comfort_height)
cap  = min(邏輯寬 ÷ min_width（comfort_width 有才算）, 邏輯高 ÷ min_height)
zoom = clamp(min(auto × font_scale, cap), 0.5, 3.0)
```

非法輸入（非有限值、≤ 0）沿用 `sane_scale`／`sane_positive_len` 的退回慣例，不 panic；`font_scale` 非法時視為 1.0。`grid_zoom` 刪除，所有呼叫點改用 `content_zoom`。

各小工具的數值（`comfort ≥ min` 為不變式，測試守住）：

| 小工具 | min（寬×高） | comfort（寬×高） | 理由 |
|---|---|---|---|
| 時鐘 | 212 × 160 | 同 min | 文字要填滿框；面板內距本身就是留白。task 2.1 以 headless Edge 實測最寬內容的視窗需求 207.625 × 155.1875（含左右、上下 gap），寬高都取整後加 4 作字型差異餘裕（`task-2.1-report.md`；重跑 `node host/tests/compare/measure-clock-natural-width.mjs`）。 |
| 行情條 | 992 × 60 | 不限 × 60 | 跑馬燈寬度不受限，字級由高度決定；min 寬只用於最小格數（維持現行 496 邏輯像素的最小寬）。 |
| 總經日曆 | 375 × 216 | 500 × 324 | comfort 寬＝原設計寬，框夠高時倍率與現行相同；comfort 高＝1.5 × min 高；min 寬＝0.75 × 設計寬，讓字級微調有放大空間（標題以省略號截斷）。 |
| 台股固定／動態事件 | 352.5 × 176 | 470 × 264 | 同上。 |
| 擴充插槽 | 352.5 × 136 | 470 × 204 | 同上。 |

**為何不在頁面量測內容（替代方案）**：量測要等頁面渲染完、資料變了要重算，倍率會隨內容跳動，且最小格數無法在 Rust 端預先推導（放開判定在 Rust）。靜態設計框結果固定、可屬性測試，代價是時鐘寬度要手動量一次。

**對預設版面的影響**（1920×1040＠100% 估算）：總經日曆、台股固定／動態事件倍率不變（寬度決定）；時鐘由 1.28 變約 1.35（高度決定）；行情條由 1.29 變約 1.44（高度決定）。

### D2 `font_scale` 設定

- `Settings.font_scale: f64`，`#[serde(default = ...)]` 預設 1.0，不升 `SETTINGS_VERSION`。載入時 sanitize：非數字或缺漏 → 1.0；超出範圍 → 夾到 0.7／1.5；四捨五入到 0.05 的倍數（滑桿間距，避免設定檔寫出 1.2000000000000002 之類的值）。**sanitize 必須在 typed 反序列化之前、對原始 JSON 做**（比照 `parse_with_defaults` 裡的 `sanitize_wallpaper_theme`／`sanitize_data_fetch`）：非數字或非有限值直接移除該鍵（交給 serde 預設），數字就夾值、取整後寫回；否則字串之類的錯型值會讓 `from_value::<Settings>` 整份失敗，被當成損壞而重設使用者的版面。反序列化後的 `Settings` 再套一次同一個夾值函式，作為型別層的保險。
- `merge_user_patch` 接受 `font_scale`：非數字 → 拒絕（回錯誤）；數字 → 套同一個 sanitize。
- 設定視窗外觀區加滑桿（70–150%，間距 5%），旁註「受小工具框大小限制，時鐘與行情條預設已填滿框，只會縮小」。`update_settings` 本來就會 relayout，新值立即套用。
- 倍率計算點取 `font_scale` 的方式：`relayout_all_widgets`、`create_widget_window`、`report_content` 重套時讀當下設定；`DragSession` 在拖曳開始時快照，拖曳中不變。

### D3 最小格數：最小框的一半

```text
min_w = 第一個 w 使 邏輯寬(w) ≥ min_width × 0.5
min_h = 第一個 h 使 邏輯高(h) ≥ min_height × 0.5
meets_min_grid_size：邏輯寬 ≥ min_width × 0.5 且 邏輯高 ≥ min_height × 0.5
```

舊規則的高度條件是「設計最小高 × 依寬度算出的 zoom」，寬框要求更高；新模型的倍率已被高度上限壓住，不需要這個耦合。新條件對 `min_height ≤ 舊 design_min_height` 的小工具（時鐘以外的九個）都不比舊條件嚴（清單 min 寬變為 0.75 倍、高度條件取消 zoom 乘數）。時鐘的 min_height 由 156 提高到 160（task 2.1 字型餘裕），在「舊倍率約 0.5–0.513」的窄帶內（例如 2256×1504＠150% 的 8×4 格）舊合法、新不合法。影響有限：推導第一階段只檢查範圍與碰撞、不檢查最小格數，這類記錄不會被移動或隱藏；倍率夾在 0.5 時內容需求 155.19 × 0.5 ≈ 77.6 仍小於矩形高，不會裁切；唯一差別是之後調整大小不能再放回這個尺寸。與 `font_scale` 無關（規格明定）。

### D4 格線疊加視窗：原生分層視窗

- **新模組 `host/src/desktop/grid_overlay.rs`**（Win32 集中原則）：每台顯示器一個 `WS_POPUP` 視窗，延伸樣式 `WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW`，矩形＝該台工作區（實體像素）。以 32 位元 premultiplied ARGB DIB 畫線，`UpdateLayeredWindow(ULW_ALPHA)` 一次更新；`WM_NCHITTEST` 回 `HTTRANSPARENT` 作第二道保險。`ShowWindow(SW_SHOWNOACTIVATE)` 後 `SetWindowPos(HWND_BOTTOM, SWP_NOACTIVATE|SWP_NOMOVE|SWP_NOSIZE)`。
- **線條**：內部 47 條垂直＋47 條水平，位置由新的純函式 `layout::grid_line_offsets(extent) -> [i32; 47]`（＝`edge(0, extent, i)`，i＝1..=47）算出，與吸附同源。1 實體像素寬；顏色取主題色，一般線 alpha 約 0x30、每 6 格一條較明顯（alpha 約 0x70），讓 48 格可以 8×6 分塊目測。工作區外框不畫（小工具貼邊時框線會被誤認為小工具邊框）。
- **執行緒**：建立、重畫、銷毀都在主執行緒（視窗的訊息由 tao 事件迴圈的 `GetMessage` 一併派送）。`set_edit_mode` 本來就在主執行緒；`relayout_all_widgets` 的呼叫端有主執行緒也有守門視窗，格線更新一律經 `run_on_main_thread` 投遞（非同步，不等待），避免執行緒親和性問題（memory：`win32-window-apis-have-thread-affinity.md`）。
- **生命週期**（`widgets.rs` 一個 `sync_grid_overlay(app)` 收斂點）：目標狀態＝「在編輯版面中」時，每台顯示器各一個且矩形與工作區相同；否則全部銷毀。呼叫時機：`set_edit_mode` 之後、`relayout_all_widgets` 結尾（顯示器、DPI、工作區變更）、`update_settings` 主題色變更後（重畫顏色）。`updater::is_exiting_for_update()` 為真時不建立（建立入口不變式）。重畫採「比對顯示器清單與工作區，不同就重建該台」的冪等作法。
- **z-order 不硬搶**：小工具的 `always_on_bottom` 會在 z-order 變更時把自己壓到最底，拖曳中的小工具可能暫時跑到格線下方。格線是滑鼠穿透、面板本來就半透明，上下差異肉眼幾乎看不出；不在每次 `WM_WINDOWPOSCHANGING` 重新排序，避免兩組置底視窗互相搶位造成閃爍或訊息風暴。

**替代方案**：(a) 每台一個透明 WebView 畫格線——要多一組 browser 行程、改 capability、還要另設資料通道；(b) 每個小工具頁在自己矩形內畫格線——空白處看不到格線，失去「看得出能放哪」的用途。兩者皆不採用。

### D5 前端

- `host/ui/settings.html`：外觀區新增字級滑桿，`patch({font_scale})`；收到 `settings` 事件時同步滑桿。
- `host/ui/widgets/custom.js`：移除寫死的 `WIDTH_PX = 360` 與 `container.style.width`，改為填滿小工具寬度（與其他小工具一致）。
- `host/tests/compare/verify-visual-edges.mjs`：模擬 ZoomFactor 的公式改為與 `content_zoom` 相同（含高度），既有「行情條夾在 3.0」的情境依新公式重算。

## Risks / Trade-offs

- [時鐘最小框寬度依字型與內容而變（例如日期、週別字串長度）] → 量測取最長的實際字串（兩位數月日、`本週 10/26 – 11/1` 這類最寬組合），取整後再加少量餘裕；超出時只是左右內距被吃掉，不會裁切文字中心區。
- [清單 min 寬改為 0.75 × 設計寬，字級放大時標題被截斷較多] → 屬使用者自選（放大字＝顯示較少），最小格數仍保證倍率 0.5 時版面完整。
- [拖曳中小工具可能被格線疊在上面] → 見 D4，滑鼠穿透確保可操作；實機以 `watch-zorder.ps1` 記錄並目視確認。
- [Win+D（顯示桌面）時格線可能被系統隱藏] → 編輯版面期間按 Win+D 屬邊緣情境，不處理；離開再進入編輯版面即恢復。
- [預設版面的時鐘、行情條字變大] → 這正是使用者要的方向（截圖兩例）；不提供退回舊公式的開關。
- [主執行緒以外呼叫的 relayout 經 `run_on_main_thread` 投遞，格線更新略晚於小工具] → 只影響視覺數十毫秒，冪等重畫保證最終一致。

## Migration Plan

- 設定檔：`font_scale` 以 serde 預設補齊，舊檔照常載入、不另存備份；不升版本號。
- 版面：最小格數只放寬，既有記錄位置不受影響；倍率改變只影響外觀。
- 回退：更新器只升不降，不規劃降版；若新公式在實機上有問題，以下一版修正。
