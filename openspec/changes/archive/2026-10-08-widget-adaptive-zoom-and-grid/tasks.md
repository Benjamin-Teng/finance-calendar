# Tasks：widget-adaptive-zoom-and-grid

> 執行路徑：SDD｜理由：跨 layout／widgets／settings／desktop Win32／前端多個模組，有新的 Win32 視窗與數值公式（倍率、最小格數）的高風險面，後面的 task 建在倍率模型之上，需要逐 task review。

## 1. 倍率與格線的純函式（`host/src/layout.rs`）

- [x] 1.1 新增 `ZoomBox` 型別與 `content_zoom(實體寬, 實體高, 縮放比例, &ZoomBox, font_scale)`（design.md D1 公式），刪除 `grid_zoom`；單元與屬性測試涵蓋：結果恆在 0.5–3、不超過上限（矩形不小於最小格數且未被夾到 3 時）、`comfort_width=None` 只看高、`font_scale` 放大被上限截住、非法輸入不 panic；`cargo test layout` 通過
- [x] 1.2 `min_grid_size`／`meets_min_grid_size` 改為 design.md D3 公式（參數改吃 `ZoomBox`），保留「`min_grid_size` 的結果自己判定合法」屬性測試，並新增「新條件不比舊條件嚴」的屬性測試（舊公式以測試內的參考實作表示）；`cargo test layout` 通過
- [x] 1.3 新增 `grid_line_offsets(extent) -> [i32; 47]`，測試與 `edge()` 及 `grid_rect_to_physical` 的邊逐一相等（含奇數寬度、極小工作區）；`cargo test layout` 通過

## 2. 小工具規格與倍率呼叫點（`host/src/widgets.rs`）

- [x] 2.1 以 headless Edge 量測時鐘 `.panel` 的自然寬度（最寬日期／週別組合、`width:max-content` 的 border box），把量測方法與數值寫入 `openspec/changes/widget-adaptive-zoom-and-grid/task-2.1-report.md`；報告附量測指令輸出
- [x] 2.2 `WIDGET_SPECS` 改用 `ZoomBox`（design.md D1 表格，時鐘寬取 2.1 實測值），所有倍率計算點（`resolve_grid_placements`、`DragSession` 移動換算、建立視窗、`report_content` 重套、`relayout_all_widgets`）改呼叫 `content_zoom` 並傳入當下 `font_scale`（拖曳開始時快照）；新增測試守住 `comfort ≥ min` 與 `min_height` 含上下 gap；既有預設格座標屬性測試與跨 DPI 拖曳測試改用新公式後通過；`cargo test` 通過

## 3. 字級設定

- [x] 3.1 `settings.rs` 新增 `font_scale`（預設 1.0、sanitize：缺漏或非數字→1.0、夾 0.7–1.5、四捨五入到 0.05；在 `parse_with_defaults` 中於 typed 反序列化**之前**對原始 JSON 處理，比照 `sanitize_data_fetch`），`merge_user_patch` 接受並驗證；測試涵蓋：舊設定檔無此欄位時照常載入且不產生 `.bad-*` 備份；磁碟上 `font_scale` 為字串／null／布林時，只有字級退回 1.0、非預設的版面與其他欄位完整保留、不產生 `.bad-*` 備份；超範圍夾值；非數字 patch 被拒；`cargo test settings` 通過
- [x] 3.2 `host/ui/settings.html` 外觀區加字級滑桿（70–150%、間距 5%、附 design.md D2 的旁註），`patch({font_scale})`，收到 `settings` 事件同步滑桿；新增 `host/tests/settings-font-scale.test.mjs`（比照既有 settings 測試的作法）並通過

## 4. 前端修正

- [x] 4.1 `host/ui/widgets/custom.js` 移除寫死寬度 360，改為填滿小工具寬度；以 `node host/tests/custom-null-snapshot.test.mjs` 及相關既有測試通過驗證
- [x] 4.2 `host/tests/compare/verify-visual-edges.mjs` 的倍率模擬改為與 `content_zoom` 相同的公式（含高度），`node --test host/tests/visual-edges-geometry.test.mjs` 通過

## 5. 編輯版面格線

- [x] 5.1 新增 `host/src/desktop/grid_overlay.rs`（design.md D4）：建立／重畫／銷毀分層視窗、premultiplied ARGB 畫線、`WM_NCHITTEST`→`HTTRANSPARENT`、置底；可純函式化的部分（線條像素緩衝的產生、alpha 選擇）有單元測試；`cargo test` 與 `cargo clippy --all-targets` 通過
- [x] 5.2 `widgets.rs` 新增 `sync_grid_overlay` 收斂點，掛在 `set_edit_mode`、`relayout_all_widgets` 結尾、`update_settings` 主題色變更後；非主執行緒經 `run_on_main_thread` 投遞；`is_exiting_for_update()` 為真不建立；「目標狀態」判定（編輯中→每台顯示器一個、矩形＝工作區）抽成純函式並有單元測試；`cargo test` 通過

## 6. 驗收

- [x] 6.1 品質 gate 全跑並附輸出：`cargo fmt --check`、`cargo clippy --all-targets`（另跑 `--features self-test-ipc`、`--features update-e2e`）、`cargo test`、`cargo test --features self-test-ipc`、`cargo test --features update-e2e`、`node --test` 跑 `host/tests/*.test.mjs`、改過的 `.md` 跑 `npx markdownlint-cli2`
- [x] 6.2 實機（release 建置、隔離資料夾、先確認工作階段未鎖定）：進入編輯版面後列舉格線視窗——每台顯示器一個、延伸樣式含 LAYERED／TRANSPARENT／NOACTIVATE／TOOLWINDOW、矩形＝工作區；`WindowFromPoint` 在格線上的空白處不回傳格線視窗；前景視窗不變；`watch-zorder.ps1` 記錄格線在一般視窗之下；離開編輯版面後格線視窗消失；截圖目視線條位置。證據存 `host/tools/evidence/`（經去識別）
- [x] 6.3 實機目視：時鐘大框、行情條加高、清單又寬又矮、字級 70%／150% 四種情境截圖，確認倍率與 design.md D1 預期一致、無裁切；證據存 `host/tools/evidence/`

## 7. 文件

- [x] 7.1 `AGENTS.md`：「現況」與「桌面小工具宿主」的倍率描述改為寬高自適應＋字級，待辦移除「字型大小可由使用者設定」「編輯版面時顯示格線」兩項；`host/README.md` 若描述倍率或設定欄位一併更新；`npx markdownlint-cli2` 0 error
