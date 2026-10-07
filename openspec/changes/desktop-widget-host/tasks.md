# Tasks

> 執行路徑：SDD｜理由：跨 Rust 核心／Win32／前端三個模組、task 超過 8 個、含視窗生命週期與故障復原等高風險面，且後段 task 建立在前段探針結論之上，需要逐 task review。
>
> 2026-09-30 追加第 7 節（格線版面，design.md D7／D9 改寫）：**第 7 節須在第 6 節之前完成**。
> 編號放在最後，是為了不和 SDD ledger 與審查佇列裡既有的 task 編號衝突。被第 7 節取代或縮減的
> 既有 task 已在該行註明。未完成項的執行順序：7.1 → 7.2 → 7.3 → 3.2 → 7.4 → 5.2 → 7.5 → 5.3
> → 7.6 → 2.5 → 3.5 → 7.7 → 第 6 節（其餘未勾項已實作、只差審查，不受此順序影響）。

所有 Rust task 先載入 `rust-engineer` skill；每個 task 完成前在 `host/` 內跑
`cargo fmt --check`、`cargo clippy --all-targets`、`cargo test` 並附輸出。桌面行為的驗收
一律附 `host/tools/` 監控腳本的記錄檔，視覺驗收前先確認工作階段未鎖定。

## 1. 探針（決定 design D8 做法，結論寫回 design.md）

- [x] 1.0 把 z-order／視窗狀態監控腳本收進 `host/tools/`（輸入 PID 與 HWND，每 0.5 秒記錄一次有變化的狀態：可見、最小化、上方可見視窗數、下方可見視窗類別、前景視窗類別、explorer PID），並附一份範例輸出
- [x] 1.1 Win+D 探針：以「桌面視窗實際位於頂端」為判準實作進出偵測與插入位置；監控記錄須同時證明四件事：Win+D 時小工具可見、進入後 5 秒內無來回切換、離開後回到一般視窗之下、開著應用程式時點擊桌面空白處小工具**不會**浮上來；結論（採用方案或退路）寫回 design.md D8
- [x] 1.2 Acrylic 探針：以 `focusable(false)` 建立視窗、套系統背景材質＋圓角＋透明 WebView2，**先點擊其他應用程式使小工具失焦後**再於 4K 與筆電螢幕截圖，確認有模糊、無黑底與殘影；所用屬性名附 Microsoft Learn 連結；失焦不呈現則記錄「毛玻璃停用」並寫回 design.md
- [x] 1.3 焦點與觸控探針：`focusable(false)`＋`WS_EX_TOOLWINDOW`，驗證點擊後前景視窗不變、切換置底旗標後兩項樣式仍在、Alt+Tab 無小工具、觸控板兩指捲動與觸控螢幕拖曳可捲動清單；不行則改 `WM_MOUSEACTIVATE` 方案並記錄（觸控板兩指捲動：使用者 10/01、10/04 實測可捲，2026-10-06 確認；觸控螢幕本機 SM_MAXIMUMTOUCHES=0，N/A；其餘項 44bbb3f）
- [x] 1.4 WebView2 故障探針：以 `controller()`＋`webview2-com` 訂閱 `ProcessFailed`，用工作管理員分別終止 renderer 與 browser process，記錄收到的事件種類，並記錄多個同源小工具是否共用 renderer

## 2. 核心骨架

- [x] 2.1 重整 `host/`：移除 spike 的 `fc` scheme 與命令列參數，建立 `settings`／`data`／`desktop`／`widgets`／`tray` 模組骨架；`frontendDist` 指向 `host/ui/`；啟用 `tray-icon` feature、加入 `webview2-com`（版本與 tauri 所用者一致）；`cargo build --release` 成功且 `dumpbin /dependents` 仍無 `VCRUNTIME140.dll`
- [x] 2.2 權限：新增 `host/capabilities/default.json`（`core:default`＋`core:window:allow-start-dragging`＋autostart 必要權限）並設 `app.withGlobalTauri: true`；release build 下測試頁面能 `listen` 事件並呼叫 `startDragging`
- [x] 2.3 設定模型：`Settings`（`version`、外觀模式、透明度、主題色、顯示除權息、資料目錄、十個小工具的開關／placement／縮放、暫停規則、鎖定、自動啟動）的預設值、缺欄位補齊、損壞檔改名備份；單元測試涵蓋三種情況（placement 由 7.2 改為格座標、縮放移除）
- [x] 2.4 位置計算純函式（D9）：單元測試涵蓋四個錨點角落、以工作區為基準、縮放改變、解析度改變、所屬顯示器消失退回主螢幕且不覆寫原設定、顯示器回來後復位、拖曳結束時依最近角落重算並對齊 8 px（**已由 7.1 取代**：錨點模型整組移除）
- [x] 2.5 顯示器識別：以 `DisplayConfigGetDeviceInfo` 取得穩定識別並對應到 Tauri 的螢幕清單；在本機記錄兩個螢幕的識別，並驗證 Windows 重新編號（拔插外接螢幕）後識別不變
- [x] 2.6 `DataSource` trait、通道註冊表與 `JsonFileSource`（D5）：通道 `tw-events`（含全空拒收）與 `custom1`–`custom5`（任意合法 JSON）；修改時間＋大小變動才讀、解析失敗保留舊快照、從未成功時狀態為 `empty`；單元測試以暫存目錄涵蓋新檔、未變、損壞、全空、尚無資料五種情況
- [x] 2.7 IPC 介面（D4）：`get_snapshot(channel)`、`get_settings`、`update_settings`、`set_edit_mode`、`report_size`（由 7.3 改為 `report_content`）與 `data`（只送訂閱者）／`settings`／`pause`／`edit-mode` 事件；以測試頁面驗證「先 listen 再 get」順序下能收到快照與後續事件，且未訂閱的通道不會收到
- [x] 2.8 毛玻璃啟用判定純函式（D8）：輸入 build 號、透明效果開關、省電模式、探針 1.2 結論、使用者選擇，輸出毛玻璃或純色；單元測試涵蓋各組合

## 3. 桌面層行為

- [x] 3.1 視窗工廠：依設定建立／關閉小工具視窗（`visible(false)` 建立後一次完成顯示與置底），套用 1.2、1.3 定案的外觀與樣式；監控記錄顯示從第一筆起即無一般視窗位於小工具之下
- [x] 3.2 尺寸：高度跟隨內容已由格線模型取代（實作在 7.3）；7.3 完成後以行情條無資料 fixture 驗證視窗隱藏、有資料後在原格子出現、編輯版面期間以佔位外框顯示
- [x] 3.3 置底守門：隱藏頂層視窗接收 `TaskbarCreated` 重新置底，加上 30 秒保險檢查（只在有一般視窗位於小工具之下時才重排）；手動重啟 explorer 與人為 `SetWindowPos(HWND_TOP)` 後，監控記錄 30 秒內恢復，且平常 10 分鐘內無任何重排
- [x] 3.4 Win+D：依 1.1 定案方案實作；監控記錄 Win+D 進出兩輪、點桌面三次，符合 1.1 的四項條件
- [x] 3.5 多螢幕：監聽顯示設定變更，依 7.1 的實際位置推導重排；手動驗證改縮放比例、改解析度、拔除與接回外接螢幕、主螢幕沒有空位、跨螢幕拖放五種情況

## 4. 小工具前端

- [x] 4.1 `common.js`、`bridge.js`（含無 Tauri 時讀 fixture 的模式）、`registry.js`、`widget.css`、`widget.html` 骨架；以本機 HTTP 伺服器開啟 `widget.html?w=clock` 能以 fixture 顯示
- [x] 4.2 對照測試腳本（D11）：本機 HTTP 伺服器＋CDP 固定時區與日期時間，擷取 Lively 版與新版 DOM 文字比較；先以 Lively 版自比對通過，再以故意注入差異的反例證明腳本會失敗
- [x] 4.3 搬移時鐘與總經日曆小工具（改用最大高度、不用 `vh`、呼叫 `report_size`；由 7.3 改為填滿格子＋`report_content`）；對照測試兩者通過
- [x] 4.4 搬移台股固定事件與台股動態事件小工具（含處置股非交易日順延、「顯示除權息」設定、「尚無資料」提示）；對照測試通過，並以休市日驗證順延
- [x] 4.5 搬移行情條（跑馬燈、無資料時內容高度為 0）；對照測試通過；以測試用命令送出 `pause`／恢復事件，驗證停止與從停止處繼續
- [x] 4.6 擴充插槽 `custom1`–`custom5` 空白模組；在宿主中開啟 `custom1`（此時設定視窗尚未完成，直接編輯 `settings.json`），放入範例 `custom1.json` 後 60 秒內顯示內容摘要，刪除後仍保留上一份，`custom2` 無檔時顯示「尚未設定」
- [x] 4.7 原生捲動：移除 ▴▾ 按鈕、捲軸 hover 顯示；在宿主中以滾輪、觸控板、觸控螢幕實測三種捲動，並記錄關閉「捲動非使用中視窗」系統設定時的結果（滾輪 9/9、路由 11/11 於整合版 6f727bd；觸控板使用者實測，2026-10-06 確認；觸控螢幕 N/A）

## 5. 生命週期

- [x] 5.1 系統匣選單（編輯版面、設定、暫停／繼續、版本號、結束）與 single-instance（手動重複啟動開設定視窗；帶 `--autostart`／`--restarted` 時靜默結束）；手動驗證兩種重複啟動
- [x] 5.2 設定視窗 `settings.html`：外觀模式、透明度、主題色、顯示除權息、資料目錄、十個小工具開關（個別縮放滑桿由 7.4 移除，開啟時空間不足的錯誤顯示見 7.4）、暫停規則、自動啟動；改值後所有小工具即時套用、重啟後保留
- [x] 5.3 編輯版面：系統匣切換、進出編輯版面時的鎖定旗標與佔位外框（移動、調整大小、對齊格線、彈回與保存改由 7.5／7.6 實作）；手動驗證切換後狀態正確、鎖定時拖不動
- [x] 5.4 autostart（`--autostart` 參數）；初次登錄與移除歸安裝檔（子專案 3），宿主首次啟動不寫登錄、只在使用者切換設定開關時寫入／刪除 Run 值、開關顯示登錄實況（design.md D12）；以 `--autostart` 啟動時小工具出現且未跳出設定視窗（`verify-5.4.ps1`）；實機登出時宿主完成工作階段結束收尾（證據 `5.4-logoff-endsession.log`）。登入後由 Run 值自啟的實機路徑不另驗（使用者 2026-10-06 決定：Run 值由安裝檔寫入、已在 installer 4.x 驗過，`--autostart` 的啟動行為由 `verify-5.4.ps1` 涵蓋）
- [x] 5.5 暫停偵測（QUNS 狀態對照、鎖定、顯示器關閉、電池、省電、手動）並推送 `pause`；手動驗證鎖定後解鎖，期間跑馬燈停止、解鎖後時鐘正確（使用者 2026-10-04 裁定鎖定 1 分鐘取代 10 分鐘）。證據：QUNS／手動／跑馬燈停止與接續＝`verify-5.5.ps1` 12/12（最終建置）；鎖定＝Manual C 10/04；電池＝在場第一段 10/06；顯示器關閉＝`B10-monitorpower-log.log` 10/04。顯示器關閉的最終建置重驗與省電模式實機（插電時 Windows 回報節能停用）依使用者 2026-10-06 決定發版後補
- [x] 5.6 WebView2 故障復原（依 1.4 結論）；終止 renderer 與 browser process 各一次，所有受影響小工具 10 秒內恢復，記錄檔有對應紀錄
- [x] 5.7 `RegisterApplicationRestart`（啟動時註冊、帶 `--restarted`）與系統匣結束時取消；以自製 Restart Manager 測試腳本（`RmStartSession`→`RmRegisterResources`（宿主 exe）→`RmShutdown`→`RmRestart`）驗證關閉後重新啟動，從系統匣結束則不重啟
- [x] 5.8 本機記錄檔（每日輪替、保留 7 天）；故障、復原、資料載入錯誤出現在記錄中

## 6. 整合驗收

- [x] 6.1 對照 `specs/` 每個 Scenario 逐條實測並記錄結果（含 explorer 重啟、Win+D 與點桌面、焦點、Alt+Tab、多螢幕、尺寸、暫停、故障復原、擴充插槽、透明效果關閉、04:00 翻日）
- [x] 6.2 連續執行 24 小時（含睡眠喚醒與鎖定），記錄宿主與 WebView2 行程的記憶體與 CPU：記憶體增幅低於 20%，期間小工具未消失（使用者 2026-10-06 裁定：run3 13.6 小時、記憶體 +13.8%、宿主未中斷即視為通過；未涵蓋睡眠喚醒，鎖定由 5.5 單獨驗過；證據 bda4e46）
- [x] 6.3 品質 gate：`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`（預設／`--features self-test-ipc`／`--features update-e2e`）、`cargo test`（同三組）、`markdownlint-cli2` 全部通過並附輸出；整合 adversarial review 通過（使用者 2026-10-01 決定以 Opus 取代 Codex）
- [x] 6.4 更新 AGENTS.md（新增宿主架構、擴充插槽接法與驗證方式）與 `host/README`，並以 `openspec validate desktop-widget-host --strict` 通過（4cc172a：AGENTS.md 改以 v0.1.0 宿主為現行，新增「現況」「資料來源細節」「安裝」「驗證」，Lively 段落移入「舊版（Lively，已凍結）」；host/README.md 更正 prerelease 時機；markdownlint-cli2 0 issues；`openspec validate desktop-widget-host --strict`＝valid，2026-10-06）

## 7. 格線版面（design.md D7／D9，2026-09-30 追加，須在第 6 節之前完成）

- [x] 7.1 格線純函式（`host/src/layout.rs`，**只新增、不刪舊函式**，自有 `GridRect` 型別）：`GRID = 48`、格線像素 `edge()`、格座標→實體矩形、倍率（夾 0.5–3）、最小格數、碰撞判斷、移動對齊（保留寬高）與調整大小對齊（只對齊被拖邊）、找空位（右上優先，記錄格數→最小格數）、放開合法判斷（實際位置＋記錄就在該顯示器上的其他記錄位置，見 D7）、實際位置推導（對應實體顯示器→第一階段放記錄不相交者→第二階段替衝突者找空位→暫時隱藏；`Primary` 與 `Device` 撞同一台；接回復位；不改寫記錄位置）。單元測試含屬性測試：多種工作區實體尺寸 × 縮放比例（100／125／150／175／200%，含本機筆電 2560×1600＠175%、4K＠150%）套用十個預設格座標時，相鄰小工具共用邊界、矩形兩兩不相交、高寬比 ≥ 0.53 時不小於最小格數；另涵蓋四捨五入對齊、移動不改格數、超界／重疊／過小判不合法、找空位順序、主螢幕空間不足時隱藏且不佔格、衝突者找空位不擠走記錄合法者、放開成功後重新推導位置不變
- [x] 7.2 設定模型 v2 並切換 Rust 呼叫端：`WidgetPlacement { monitor, col, row, w, h }`、`SETTINGS_VERSION = 2`；在 `merge_json` 前對原始 JSON 判斷版本，缺 `version` 或 `< 2` 改名備份後以預設值啟動；載入後驗證格座標範圍、不合法者退回預設；`update_settings` 拒收 `placement`。版面欄位只放 Rust：`widgets.rs` 規格表改列設計寬度與設計最小高度（時鐘的設計最小高度以 headless 量測時鐘頁面 `.panel` 的 `scrollHeight` 定案，不得小於實測值，估計約 140；其餘初值：總經日曆 500／200、台股固定事件 470／160、台股動態事件 470／160、行情條 992／44、擴充插槽 470／120），`settings.rs` 預設格座標（初值：時鐘 15,1,16,9；總經日曆 15,11,16,31；台股固定事件 32,1,15,17；台股動態事件 32,19,15,23；行情條 15,43,32,4；五個擴充插槽放左側、互不重疊）；`host/ui/registry.js` 刪除 width／maxHeight／defaultPosition，一致性測試改為只比對 id 與通道並斷言預設格座標兩兩不相交。`widgets.rs` 的視窗建立、多螢幕重排、拖曳結束改用 7.1（拖曳結束此時只做移動對齊與寫回，驗證與彈回在 7.5），之後刪除錨點模型的 `resolve_placement_rect`／`plan_relayout`／`placement_from_drag_end`／`snapped_placement_and_rect` 與相關測試；單元測試涵蓋舊版與缺版本設定檔備份（實作 b5ee609..9241f3d；審查 M1／low 由 F1 f0d6b3d、M2 由 F2 eb637e2＋F2b 7c1693f 修正並 approve；縮放實測 7.7-grid-layout-manual-scale175／150-back；合併後 gate 1779/0）
- [x] 7.3 頁面填滿格子：`report_content(has_content)` 取代 `report_size`（處理時重套倍率），刪除高度跟隨內容；無內容時隱藏但保留格子，編輯版面期間核心顯示所有開啟中的小工具、頁面對無內容者畫佔位外框；`host/ui/widget.css` 與各小工具模組改為填滿視窗高度、清單區捲動；4.x 對照測試仍全部通過
- [x] 7.4 開啟小工具找空位：`update_settings` 開啟小工具時，若所屬顯示器存在且記錄格子與記錄就在該顯示器上的其他開啟中小工具的記錄位置相交，就找空位並寫回，找不到回傳錯誤；所屬顯示器不存在時不寫回；`settings.html` 顯示「空間不足，請先調整版面」並把開關還原、移除個別縮放滑桿；因空間不足暫時隱藏者以系統匣選單的停用項目標示；單元測試涵蓋三種結果，設定視窗以 CDP 或實機驗證
- [x] 7.5 編輯版面移動的驗證與彈回：`desktop.rs` 處理 `WM_MOVING`／`WM_EXITSIZEMOVE`，拖曳中以 `edit-preview` 廣播切換紅色外框，放開後合法就寫回並重新推導、不合法就回到推導出的原矩形；實機經 `SafeInput.psm1` 驗證：移動後對齊且格數不變、重疊彈回、拖曳中紅框出現與消失（裁切截圖或頁面記錄）、跨螢幕拖放歸屬中心所在螢幕（無第二螢幕時標記待補）、拖曳期間前景視窗與焦點不變、鎖定時拖不動、重啟後保留；合成輸入無效時回報 ENV-BLOCKED，不繞過
- [x] 7.6 編輯版面調整大小：先做探針——在 `resizable(false)` 的小工具視窗上呼叫 `startResizeDragging`，確認能否進入尺寸迴圈（收到 `WM_SIZING`）且前景與焦點不變，結論寫回 design.md D7；可行則只加編輯模式的 JS 把手（標 `data-tauri-drag-region="false"`），不可行則依 D7 退路由 `desktop.rs` 切換 `WS_SIZEBOX`；capability 加 `core:window:allow-start-resize-dragging`；`WM_SIZING` 只對齊被拖邊、紅框預告與過小彈回沿用 7.5。實機驗證：加寬兩格後內容放大、縮到小於最小格數彈回、調整期間前景與焦點不變、鎖定時無法調整
- [x] 7.7 驗收腳本與預設值定稿：新增 `host/tools/verify-grid-layout.ps1`（列舉所有小工具視窗矩形，斷言兩兩不相交、每條邊落在該顯示器的格線像素上）；本機筆電與 4K 外接螢幕（接上時；未接則標記待補）實機截圖（只裁小工具區域）確認排列接近 Lively 版、時鐘不被裁切、台股動態事件不蓋行情條，必要時調整預設格座標與設計最小高度；更新受影響的 `verify-*.ps1`、`host/tools/README.md` 與 AGENTS.md「桌面小工具宿主」一節（註冊表欄位與「三份清單」描述）
