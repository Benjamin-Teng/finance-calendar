# Tasks：widget-font-scale-per-widget

> 執行路徑：SDD｜理由：跨 settings／layout／widgets／前端與驗收腳本，含設定檔格式遷移與新的 IPC 指令權限判定，後面的 task 建在設定格式與倍率細節之上。

## 1. 設定格式（`host/src/settings.rs`）

- [x] 1.1 `WidgetConfig.font_scale`、刪除 `Settings.font_scale`、`normalize_font_scale` 改 0.5–3.0／0.1、`parse_with_defaults` 依 design.md D1 遷移與 sanitize、`merge_user_patch` 拒收兩種 `font_scale`；測試涵蓋：無字級舊檔→全 1.0 且無 `.bad-*`；帶頂層 1.2 的 v0.2.0 檔→各小工具 1.2、存檔後不再有頂層鍵；帶頂層 1.2 但只列出部分小工具的檔→列出與未列出者皆 1.2（含 round-trip 存讀）；單一小工具字級為字串／null→只有它回 1.0、其餘保留、無 `.bad-*`；超範圍夾值；序列化輸出乾淨小數；patch 帶任一形式 `font_scale` 被拒。呼叫端（layout／widgets）暫以每個小工具的值或 1.0 編譯通過即可，倍率行為在 2.1 完成；`cargo test` 通過

## 2. 倍率與指令（`host/src/layout.rs`、`host/src/widgets.rs`）

- [x] 2.1 依 design.md D2：`GridWidgetInput.font_scale`、`resolve_grid_placements` 移除全域參數、`content_zoom_detail`／`at_cap`、所有倍率計算點改用各自字級、`WidgetRuntime` 同處快取 `zoom` 與 `at_cap`（拖曳旁路亦同）；測試涵蓋 at_cap 的三種成立條件與不成立、兩個小工具不同字級互不影響、拖曳快照只取被拖曳者；`cargo test` 通過
- [x] 2.2 依 design.md D3：`adjust_widget_font_scale`、`get_widget_font_state`、`widget-font` 廣播；id 由呼叫端 label 決定、step 只收 ±1、鎖定或不在編輯版面時拒絕、存檔失敗記憶體不變；純邏輯抽出並單元測試（兩個指令各自對 settings、桌布渲染、未知與畸形 label 回錯誤且不 panic；端點 0.5／3.0 不越界）；指令註冊於 `main.rs` 並確認 capability 允許小工具 webview 呼叫；`cargo test` 與 `--features self-test-ipc` 通過

## 3. 清單收合與最小框

- [x] 3.1 依 design.md D5：清單列收合 CSS（總經日曆、台股固定／動態事件、擴充插槽），以 headless Edge 實測各清單在收合版面下的最窄可用寬度與單行版面斷點，寫入 `openspec/changes/widget-font-scale-per-widget/task-3.1-report.md`（含可重跑的量測腳本，放 `host/tests/compare/`），`WIDGET_SPECS` 清單 `min_width` 改為實測值（總經日曆須 ≤ 266，否則回報）；`verify-visual-edges.mjs` 與相關 node／Rust 測試（含最小格數屬性測試）通過

## 4. 前端

- [x] 4.1 依 design.md D4：`widget.html`／`widget.css`／`bridge.js` 編輯版面字級控制（A−／百分比／A+、停用條件、達上限提示、不觸發拖曳、啟動時 listen 再查詢），新增 `host/tests/widget-font-controls.test.mjs` 涵蓋顯示條件、停用條件、只處理自己 id 的 `widget-font`、按鈕事件不冒泡到拖曳區、初始化競態（查詢在途時先到的事件不被較舊的查詢回覆覆蓋，比照 `widget-init-race.test.mjs`）；`node --test host/tests/*.test.mjs` 通過
- [x] 4.2 依 design.md D6：`settings.html` 移除全域字級滑桿、加說明文字，刪除或改寫 `host/tests/settings-font-scale.test.mjs`；node 測試通過

## 5. 驗收

- [x] 5.1 品質 gate 全跑並附輸出：`cargo fmt --check`、`cargo clippy --all-targets`（另跑 `--features self-test-ipc`、`--features update-e2e`）、`cargo test`（三組 feature）、`node --test host/tests/*.test.mjs`、`host/tools/tests/*.ps1`、改過的 `.md` 跑 `npx markdownlint-cli2`
- [x] 5.2 實機（release 建置、隔離、未鎖定、正式版未執行）：更新 `host/tools/verify-adaptive-zoom.ps1`——D 情境改為各小工具字級；新增情境 E：總經日曆邏輯寬約 0.64 × 設計寬、高度充足、字級 200% → 倍率 ≥ 1.2、收合版面無重疊無裁切；新增情境 F：v0.2.0 格式（頂層 `font_scale` 1.2）設定檔載入後各小工具 1.2；以 CDP 在小工具頁呼叫 `adjust_widget_font_scale` 驗證只改自己、鎖定時被拒；截圖目視字級控制外觀。`verify-grid-overlay.ps1` 重跑一次。全部 0 FAIL

## 6. 文件

- [x] 6.1 `AGENTS.md`（現況「版面」與桌面小工具宿主節的字級描述改為逐小工具＋編輯版面按鈕、清單收合）、`host/README.md`（設定欄位 `widgets.<id>.font_scale`、範圍、遷移）、`host/tools/README.md`（腳本新情境）；`npx markdownlint-cli2` 0 error
