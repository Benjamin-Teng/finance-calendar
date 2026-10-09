## Context

動機見 proposal.md。現況（`openspec/changes/archive/2026-10-08-widget-adaptive-zoom-and-grid/design.md` D1–D4）：

- 倍率在 Rust：`layout::content_zoom(實體寬, 實體高, 縮放比例, &ZoomBox, font_scale)`；`resolve_grid_placements(monitors, widgets, font_scale)` 收單一全域字級；`DragSession` 拖曳開始時快照字級；`WidgetRuntime::zoom` 是重套快取（拖曳旁路也要同步寫，memory `applied-value-cache-has-second-write-path.md`）。
- 設定：`Settings.font_scale`（0.7–1.5／0.05），`parse_with_defaults` 在 typed 反序列化前以 `sanitize_font_scale` 處理 raw JSON；`merge_user_patch` 接受 `font_scale`。`WidgetConfig { enabled, placement }`。
- 前端：`settings.html` 外觀區有全域滑桿；小工具頁在編輯版面以 `data-tauri-drag-region="deep"` 讓整頁可拖曳、四邊四角有調整大小把手（`widget.html`、`bridge.js`）。
- 清單的 `ZoomBox`：最小框寬＝0.75 × 設計寬、舒適框寬＝設計寬；清單列（`.ev`、`.ev.macro`）是單行 flex，右側「預／前」數值靠右。

## Goals / Non-Goals

**Goals:**

- 字級只有一層（每個小工具），升級不遺失 v0.2.0 使用者設定的全域值。
- 調字級的操作就在小工具上，所見即所得。
- 窄框清單可以把字放大到實用大小，而且不重疊、不裁切。

**Non-Goals:**

- 時鐘、行情條的版面收合（它們已由高度／寬度填滿框，字級只能縮小，維持現狀）。
- 鍵盤或滾輪調字級（小工具不可聚焦）。
- 動態桌布主題頁。

## Decisions

### D1 設定格式與遷移

- `WidgetConfig` 新增 `font_scale: f64`（`#[serde(default)]` 1.0）。`Settings.font_scale` 刪除，不升 `SETTINGS_VERSION`。
- `normalize_font_scale`：非有限值 → 1.0；夾 0.5–3.0；四捨五入到 0.1 的倍數（以「整數十分位 ÷ 10」產生乾淨值，序列化為 `1.2` 而非 `1.2000000000000002`）。
- `parse_with_defaults` 在 typed 反序列化**之前**處理 raw JSON（沿用 v0.2.0 的作法，理由同前：錯型值會讓整份設定被當成損壞）：
  1. 頂層 `font_scale` 若是有限數字，正規化後作為「遷移值」；不論是否有效都移除頂層鍵。
  2. 對 `widgets` 下每個物件：`font_scale` 是有限數字 → 正規化寫回；缺漏 → 有遷移值就寫入遷移值；其他型別（字串、null、布林）→ 移除（交給 serde 預設 1.0）並記警告。
  3. typed 反序列化與既有的小工具補齊之後，若有遷移值，對 `WIDGET_IDS` 中每個「raw JSON 裡沒有有效自己字級」的小工具（含原檔未列出、由預設補齊者）設為遷移值。實作上在第 2 步記下哪些 id 有有效值，補齊後再套用。
- 反序列化後的 `Settings` 對每個小工具再套一次 `normalize_font_scale`（型別層保險）。
- `merge_user_patch` 拒收頂層 `font_scale` 與任何 `widgets.<id>.font_scale`：字級唯一的寫入入口是 D3 的指令（需編輯版面）。

### D2 倍率計算改用各自字級

- `GridWidgetInput` 新增 `font_scale`；`resolve_grid_placements(monitors, widgets)` 移除全域參數。所有倍率計算點（建立視窗、`relayout_all_widgets`、`DragSession` 移動換算、`report_content` 重套）改取該小工具自己的字級；`DragSession` 照舊在拖曳開始時快照（只快照被拖曳的那個小工具）。
- 新增 `layout::content_zoom_detail(...) -> ZoomDetail { zoom, at_cap }`，`content_zoom` 改為其薄包裝。`at_cap`＝「再放大一級（字級 +0.1）倍率也不會變大」：`auto × font_scale ≥ cap`（容許 1e-9 誤差）或 `zoom ≥ 3.0`；字級已達 3.0 時也為真。`WidgetRuntime` 與 `zoom` 一起快取 `at_cap`，兩者在同一處寫入。

### D3 調字級的指令與狀態推送

- 指令 `adjust_widget_font_scale(step: i32) -> Result<WidgetFontState, String>`：小工具 id 一律由**呼叫端 webview label** 決定（`widget_id_from_label`），頁面無法改別人的字級。`step` 只接受 +1／−1（＝±0.1），其他值回錯誤。條件同拖曳存檔：在編輯版面且未鎖定（`drag_end_may_save` 同義），否則回錯誤、不改設定。成功時更新記憶體設定、存檔（失敗則回錯、記憶體不改）、廣播 `settings`、重排（`relayout_all_widgets` 已會重算倍率與 at_cap）。
- 指令 `get_widget_font_state() -> Result<WidgetFontState, String>`（同樣以呼叫端 label 決定 id），`WidgetFontState { font_scale, at_cap }`。兩個指令對非小工具 label（設定視窗、桌布渲染視窗、未知或畸形 label）一律回錯誤、不 panic、不回傳虛構的預設值（本機自訂指令沒有逐指令 ACL，任何本機 webview 都能呼叫）。
- 狀態推送：重排或拖曳旁路改變某小工具的 `at_cap` 或字級時，廣播事件 `widget-font`，payload `{ id, font_scale, at_cap }`；頁面只處理自己 id 的那筆。**為何不用 Channel**：此資料不敏感（各小工具字級與是否達上限）且 settings 視窗也可能想讀；AGENTS.md 規定的 Channel 用於資料本體，`widget-font` 與 `edit-mode` 同屬 UI 狀態廣播。頁面啟動或重新載入時先 `listen('widget-font')` 再呼叫 `get_widget_font_state`，並採 `widget.html` 對 `settings`／`edit-mode` 既有的仲裁規則：查詢在途期間收到的事件優先於查詢回覆（事件較新），查詢回覆只在期間沒收到事件時採用。

### D4 編輯版面的字級按鈕

- `widget.html` 在編輯版面且未鎖定時，於右上角顯示一組控制：`A−`、百分比、`A+`（佔位外框狀態也顯示）。控制本身與其子元素標 `data-tauri-drag-region="false"`，且 `pointerdown` 時 `stopPropagation`，確保不進入拖曳；控制不得落在四邊四角的調整大小把手範圍內。
- 字級 0.5 時 `A−` 停用；`at_cap` 為真時 `A+` 呈停用樣式並以 `title` 與控制下方一行小字顯示「已達框大小上限，請把小工具拉大」；字級 3.0 時同樣停用。按下時呼叫指令，回傳後立即更新顯示；失敗時保留原值並在控制旁短暫顯示錯誤。
- 控制的尺寸以 CSS px 寫死，會隨 ZoomFactor 縮放；倍率 0.5 時仍需可點（最小點擊區 24×24 CSS px，倍率 0.5 時 12 邏輯 px——可接受，因 0.5 是極端情況）。

### D5 清單收合版面與最小框

- 收合以 CSS media query 依視窗 CSS 寬度切換（每個小工具是獨立 viewport）：寬度 < 斷點時 `.ev` 列改為兩行 grid，第一行「時間、標記、點、標題」，第二行右側數值（`.ev .n` 等），標題允許換行（`overflow-wrap:anywhere`）。斷點各小工具依實測決定（單行版面開始重疊的寬度＋餘裕）。
- 清單 `ZoomBox.min_width` 改為實測值：以 headless Edge 在收合版面下找最窄、仍不重疊不裁切的 CSS 寬度（用最長的實際字串組合：長標題、帶「預／前」的列、長日期列），取整加 4 餘裕。目標 ≤ 設計寬 × 0.55（spec「窄框的清單可以放大」要求總經日曆在 0.64 倍設計寬時能到 1.2 倍，即 min_width ≤ 266）；實測若大於此值，回報而不是硬壓數值。`comfort` 不變。最小格數隨之變寬鬆（只放寬，既有記錄不受影響）。

### D6 設定視窗

- 移除全域字級滑桿與其測試，外觀區加一行說明「字級在『編輯版面』中於各小工具右上角調整」。

## Risks / Trade-offs

- [降版到 v0.2.0 會讀不到各自字級（全域欄位已不寫出）] → 更新器只升不降；手動降版時字級回 100%，不影響版面。
- [清單 min 寬變窄，極窄框可能出現很多換行] → 只在使用者主動放大字級或把框拉很窄時發生；最小格數仍保證倍率 0.5 時版面完整。
- [按鈕佔用右上角] → 只在編輯版面出現；收合尺寸以不擋到面板標題文字為準，實機截圖目視。
- [`widget-font` 廣播] → 非敏感 UI 狀態，負向測試確認頁面只處理自己的 id。

## Migration Plan

- 載入時自動遷移，無需使用者動作；首次因任何原因存檔後檔案即為新格式。
- 回退：不規劃降版。
