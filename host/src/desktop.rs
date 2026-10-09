//! Win32 桌面層行為集中於此（design.md D8）；其餘模組不得直接呼叫 Win32。
//!
//! 本 task（2.5）交付：
//! - [`source_name_to_device_path`]：以 `QueryDisplayConfig`＋`DisplayConfigGetDeviceInfo`
//!   取得「GDI 來源裝置名稱（`\\.\DISPLAYn`）→ 穩定的 `monitorDevicePath`」對照表（design.md
//!   D9）。這是本檔唯一直接呼叫 Win32 的地方。
//! - [`monitors_from_tauri`]：純對應邏輯，把螢幕清單（[`MonitorSnapshot`]，見其文件：
//!   `tauri::Monitor` 沒有公開建構子，核心邏輯改吃這個可在測試裡組字面量的中介型別）轉成
//!   [`crate::layout::MonitorInfo`] 清單，供格線推導（task 7.2 起為
//!   [`crate::layout::resolve_grid_placements`]）使用。此函式不呼叫 Win32，只做資料轉換，故可
//!   單元測試。
//! - [`monitor_infos_from_tauri_monitors`]：實機呼叫端（task 3.1／3.5）用的入口，直接接受
//!   `tauri::Monitor` 清單，內部轉成 `MonitorSnapshot` 後委派給 `monitors_from_tauri`。
//!
//! 尚未有呼叫端接上（視窗工廠是 task 3.1／3.5），公開項目在 `cargo clippy` 下會是
//! `dead_code`；以模組層級 `allow` 抑制，避免之後每個公開項目各自標註（比照 layout.rs 的
//! 慣例）。
//!
//! 為何直接採用 Tauri `Monitor.work_area`／`Monitor.scale_factor`，不再另外呼叫 Win32
//! （design.md D9「取得工作區與 DPI 縮放」的裁決）：查過 `tauri-runtime-wry-2.12.0` 原始碼
//! （`src/monitor/windows.rs`）——`Monitor.work_area` 在 Windows 上正是
//! `GetMonitorInfoW().rcWork`（實體像素），`Monitor.name` 正是 `GetMonitorInfoW().szDevice`
//! （即 `\\.\DISPLAYn`，與 `DisplayConfigGetDeviceInfo` 用來比對來源名稱的字串同一來源）；
//! `Monitor.scale_factor` 則由 tao 的 `get_monitor_dpi`（`GetDpiForMonitor`）換算而來。三者都
//! 已是本模組需要的形式，重新呼叫一次 Win32 只會是重複勞動、還多一個「兩份資料何時會不同步」
//! 的風險。
//!
//! task 2.8 補上：
//! - [`resolve_appearance`]：毛玻璃啟用判定純函式（D8）。輸入 [`AcrylicConditions`]
//!   （build 號、透明效果開關、省電模式、探針 1.2 結論、使用者選擇），輸出
//!   [`crate::settings::AppearanceMode`]；任一條件不滿足即純色，純函式、不呼叫 Win32，可
//!   單元測試（見 `acrylic_tests` 模組）。探針 1.2 已定案（小工具失焦時毛玻璃不呈現，毛玻璃
//!   停用，design.md D8），[`ACRYLIC_WORKS_UNFOCUSED_DEFAULT`] 固定為 `false`，呼叫端傳這個常數。
//! - [`power_saver_active`]／[`transparency_effects_enabled`]／[`os_build_number`]：讀系統
//!   狀態組出 [`AcrylicConditions`] 的三個輸入（brief 標示「可選」，一併做掉），皆回傳
//!   `Option`（`None`＝讀取失敗、狀態未知）；[`AcrylicConditions::from_readings`] 把任一未知
//!   當成條件不滿足→純色（fix round 1，Codex 2.8：未知狀態不得被當成允許毛玻璃）。
//!
//! task 3.1 補上（視窗工廠的桌面層部分，見「D8：小工具視窗的樣式、擺放與顯示」一節）：
//! - [`present_widget`]：隱藏狀態下套樣式（[`apply_widget_ex_style`]：加
//!   `WS_EX_TOOLWINDOW`、去 `WS_EX_APPWINDOW`）→ 設實體像素矩形（[`set_window_rect`]）→
//!   `SetWindowPos(HWND_BOTTOM, SWP_SHOWWINDOW|SWP_NOACTIVATE|…)` 一次完成顯示與置底
//!   （[`show_at_bottom`]）。
//! - [`current_appearance`]／[`apply_appearance`]：外觀判定與套用的單一入口（探針 1.2 已定案
//!   毛玻璃停用，故一律純色）。
//!
//! task 3.2 補上（小工具尺寸跟隨內容，design.md D7）：
//! - [`hide_widget`]：`SetWindowPos(SWP_HIDEWINDOW|…)` 隱藏視窗（內容高度為 0 時）——**不得**
//!   改用 Tauri `hide()`／`set_visible(false)`，理由同本節開頭的不變量說明。
//! - [`is_widget_visible`]：`IsWindowVisible` 讀目前是否可見，供呼叫端（task 7.3 起是
//!   [`crate::widgets::apply_report_content`]）判斷「從無內容變回有內容」時要不要走
//!   [`show_at_bottom`] 重新顯示。
//! - 尺寸變更（非隱藏／重新顯示）沿用既有的 [`set_window_rect`]，不需要新函式。
//! - 決定「要不要隱藏、新矩形是多少」屬於認得小工具註冊表與設定的業務邏輯，不屬於本模組
//!   （D8「Win32 相關行為集中在單一模組」指的是**呼叫** Win32 API 的動作本身，不是決策），
//!   實作在 [`crate::widgets::apply_report_content`]，本模組只提供上述兩個 Win32 原語。
//!
//! task 3.3 補上（置底守門，design.md D8「explorer 重啟」「保險檢查」）：
//! - [`needs_rebottom`]：純函式——輸入一次 z-order 列舉快照（[`ZOrderWindowState`]），判定
//!   「是否有可見的一般應用程式視窗位於任一小工具之下」。等價變換見其文件：只需檢查
//!   z-order 中排在**最上層那個小工具**之後有沒有一般視窗，不必對每個小工具個別檢查，因此
//!   「小工具之間的順序」天然不影響結果（design.md D8）。
//! - [`zorder_snapshot`]（3.3 時名為 `snapshot_for_safety_check`，3.4 改名共用）：對應的
//!   Win32 列舉（`GetTopWindow`／`GetWindow`
//!   (`GW_HWNDNEXT`)，「一般視窗」判準與 `host/tools/watch-zorder.ps1` 一致），把哪些 HWND
//!   算作「小工具」交由呼叫端傳入（本模組不認得小工具註冊表）。
//! - [`spawn_gatekeeper`]：建立隱藏頂層守門視窗（`tauri::window::WindowBuilder`——**不是**
//!   webview 視窗，也不是 message-only／`HWND_MESSAGE` 視窗，滿足 D8「不可用 message-only
//!   視窗」；不呼叫 [`show_at_bottom`]，永遠保持隱藏，只作為訊息接收端）。以
//!   `SetWindowSubclass` 掛上處理常式：收到 `RegisterWindowMessageW("TaskbarCreated")` 廣播
//!   即呼叫呼叫端提供的 `rebottom_all`；`SetTimer` 每 30 秒觸發一次保險檢查，判定
//!   [`needs_rebottom`] 為真才呼叫 `rebottom_all`（design.md「小工具之間不因此反覆重排」：
//!   平時沒有一般視窗蓋住小工具時，判定恆為假，不會無謂重排）。呼叫端（`widgets` 模組）提供
//!   「目前可見的小工具 HWND」與「全部重新置底」兩個回呼——決策與小工具註冊表仍不進本模組。
//! - 每次實際重排都寫一行到 `settings::default_gatekeeper_log_path()`（task-3.3-brief：
//!   宿主端加重排次數的記錄，供驗收讀取「10 分鐘內無任何重排」／「30 秒內恢復」）。
//! - 顯示桌面期間略過保險檢查：3.3 原本留下 `SAFETY_CHECK_SUSPENDED` 旗標當介面，task 3.4
//!   改由 [`SHOW_DESKTOP_ACTIVE`] 單一旗標同時承擔「顯示桌面中」與「保險檢查暫停」兩個語意
//!   （兩者永遠同值，分成兩個旗標只會多一個不同步的機會）。`TaskbarCreated` 的處理**不**受此
//!   旗標影響——design.md 該條目沒有「顯示桌面期間略過」的但書，維持無條件全部重新置底。
//!
//! task 3.4 補上（Win+D 顯示桌面，design.md D8 Win+D 條，探針 1.1 定案；見「D8：Win+D 顯示
//! 桌面」一節）：
//! - 純函式：[`classify_top`]（z-order 頂端第一個有意義的視窗是桌面還是一般視窗）、
//!   [`decide_show_desktop`]（進入／重插／離開／不動）、[`insert_target`]（插入位置）、
//!   [`is_relevant_win_event`]（哪些 WinEvent 需要重算）、[`is_shell_desktop_window`]。
//! - Win32：[`install_show_desktop_subclass`]（小工具視窗子類別化 `WM_WINDOWPOSCHANGING`，
//!   顯示桌面中把 tao 改寫的 `HWND_BOTTOM` 還原成原請求值；由 [`present_widget`] 安裝）、
//!   守門視窗建立時一併以 out-of-context `SetWinEventHook` 監聽事件、每次事件重算判準；
//!   30 秒計時器先重算判準（兜底漏接的事件）再做保險檢查。
//! - 每次進入／重插／離開都寫一行到守門記錄檔（`SHOWDESKTOP ENTER|REASSERT|EXIT …`），供
//!   `host/tools/verify-3.4.ps1` 判讀。
//!
//! task 3.5 補上（多螢幕重新定位，design.md D9「多螢幕與 DPI 定位」）：
//! - [`GatekeeperCallbacks::relayout_all`]：新增第三個回呼，本模組不認得小工具註冊表與設定，
//!   「重新列舉顯示器、重算全部（可見）小工具矩形」交給呼叫端
//!   （[`crate::widgets::relayout_all_widgets`]）。
//! - [`gatekeeper_subclass_proc`] 新增攔截 `WM_DISPLAYCHANGE`（解析度／色彩深度改變、拔接
//!   螢幕，送給所有頂層視窗）與 `WM_SETTINGCHANGE`（`wParam == SPI_SETWORKAREA`，工作區改變，
//!   同樣廣播給所有頂層視窗），兩者都經 [`request_relayout`] 延後呼叫 `relayout_all`。
//! - `WM_DPICHANGED`（縮放比例改變）**不**在這裡處理：這則訊息只送給縮放改變的那台顯示器上
//!   的視窗，隱藏在固定位置的守門視窗不保證收得到；改由 `main.rs` 的 `App::run` 事件迴圈攔截
//!   Tauri 轉譯後的 `WindowEvent::ScaleFactorChanged`（任一小工具視窗收到都代表某台顯示器的
//!   縮放變了），見其「task 3.5」註解與 [`GatekeeperCallbacks::relayout_all`] 文件。
//! - fix F2（review 3.5）：三個來源都不在收到當下同步重排——`WM_DPICHANGED` 的同步重排會被
//!   tao 隨後的 `SetWindowPos(建議矩形)` 蓋掉；同步廣播的 `WM_SETTINGCHANGE` 內做慢工作會拖住
//!   explorer。一律 [`request_relayout`]：以 [`RelayoutCoalescer`] 合併同一波、`PostMessageW`
//!   通知守門視窗、排 [`RELAYOUT_DEBOUNCE_MS`] 計時器，到期時重排一次並記錄
//!   `RELAYOUT reason=<原因,…> applied=<套用數>`。
//! - 純函式部分（`monitors`＋記錄位置 → 矩形）task 7.2 起為
//!   [`crate::layout::resolve_grid_placements`]（格線兩階段推導），顯示器清單沿用
//!   [`monitor_infos_from_tauri_monitors`]，本模組不重複。
//!
//! task 5.3 補上（編輯版面拖曳結束，design.md D9「編輯版面」）：
//! - [`WIDGET_DRAG_HOOKS`]／[`register_widget_drag_hooks`]（task 7.5 由單一的拖曳結束回呼
//!   擴充為三個時機點：`WM_ENTERSIZEMOVE` 開始、`WM_MOVING` 提議矩形、`WM_EXITSIZEMOVE`
//!   結束）：`widget_subclass_proc`
//!   （task 3.4 既有的每個小工具視窗都會裝的子類別）新增攔截 `WM_EXITSIZEMOVE`，轉交
//!   `DefSubclassProc`（讓 tao 自己的 `dragging` 旗標歸零）後呼叫這個回呼——實際的顯示器解析、
//!   移動對齊（task 7.2 起為 `layout::placement_after_move`）、存檔與重新推導都在呼叫端
//!   （`widgets::finish_widget_drag`），本模組只提供「拖曳結束了」這個時機點與觸發它的 HWND。
//!   fix F6b：結束時不立即判定，改 `PostMessageW(WM_APP_DRAG_SETTLE)` 給同一扇視窗，處理時才
//!   呼叫 `on_settle`（`widgets::settle_widget_drag`）以當時的視窗矩形判定；`WM_NCDESTROY` 時
//!   呼叫 `on_destroyed` 丟棄該視窗的拖曳狀態。
//! - fix drag-dpi（design.md D7「跨縮放比例拖曳」）：`WM_MOVING` 的回呼可回傳目標矩形，本模組
//!   改寫提議矩形並在尺寸不同時 `SetWindowPos`；另攔截小工具視窗自己的 `WM_GETDPISCALEDSIZE`
//!   與 `WM_DPICHANGED`——fix F6 起**只在宿主自己的 `apply_drag_rect` 進行中**（重入旗標
//!   `APPLYING_DRAG_RECT`）回答它正在套用的大小／改寫建議矩形，其餘情況（含使用者改縮放比例、
//!   系統移動迴圈自己跨過 DPI 邊界）原樣轉交，上面「縮放改變走 `ScaleFactorChanged` 延後重排」
//!   的路徑不受影響。
//! - **拖曳期間置底不需要額外處理**：小工具視窗建立時已帶 tao 的 `always_on_bottom(true)`
//!   （`widgets::create_widget_window`），tao 對帶這個旗標的視窗，在**每一次**
//!   `WM_WINDOWPOSCHANGING`（查證 tao 0.37.1 `platform_impl/windows/event_loop.rs` 約
//!   1140–1143 行：`if window_flags.contains(ALWAYS_ON_BOTTOM) { window_pos.hwndInsertAfter =
//!   HWND_BOTTOM }`，沒有排除純位置變更的條件）都無條件把 `hwndInsertAfter` 覆寫成
//!   `HWND_BOTTOM`——`DefWindowProc` 內建拖曳迴圈搬動視窗正是靠連續送這則訊息，因此拖曳中的
//!   每一次位置更新都被迫同時重新置底，不需要另外暫停或恢復任何 3.3／3.4 的機制。這個判斷是
//!   由 crate 原始碼直接查證得出，非執行期觀察；3.3 的 30 秒保險檢查若剛好在一次異常久的拖曳
//!   中觸發，最多是對已經在底層的小工具再呼叫一次 `show_at_bottom`（`SetWindowPos` 帶
//!   `SWP_NOMOVE`，不影響拖曳中的位置），非本 task 需要處理的干擾。
//! - **不搶焦點**：`startDragging()`（前端用 `data-tauri-drag-region`，見
//!   `host/ui/widget.html`）與一般點擊共用同一條「`WS_EX_NOACTIVATE` 視窗不因滑鼠輸入被啟用」
//!   路徑（`widgets::create_widget_window` 的 `focusable(false)`）。與 task 1.3
//!   （`examples/probe_focus_touch.rs`）尚待補驗的點擊-不啟用結論同一個假設，但這裡多一層
//!   差異：`startDragging()` 底層是 `PostMessageW` 直接注入合成的 `WM_NCLBUTTONDOWN`
//!   （tao `window.rs::handle_os_dragging`），略過真實滑鼠點擊會先觸發的
//!   `WM_MOUSEACTIVATE`／命中測試那段管線；Microsoft Learn 對 `WS_EX_NOACTIVATE` 的描述是
//!   「使用者點擊時不會成為前景視窗」，屬視窗管理器層級的啟用判斷（`SetActiveWindow`／
//!   `SetForegroundWindow` 才能真正啟用），不是只綁在 `WM_MOUSEACTIVATE` 這個訊息的回覆值上，
//!   故推論拖曳與一般點擊應該落到同一個判斷、結果相同；但這仍只是查證文件得出的推論，尚未有
//!   `SafeInput` 真實拖曳的第一手觀察，與 1.3 一併列入 `human-checklist.md`「待解鎖自動驗收」。
//!
//! task 5.5：「省電模式變化時重新判定外觀」已完成——`desktop` 模組只負責偵測並透過
//! [`PauseSignal::PowerSaver`] 通知；實際重判與套用（`desktop::current_appearance`／
//! `desktop::apply_appearance` 的呼叫點）在 [`crate::widgets::refresh_all_widget_appearance`]
//! （呼叫端不認得小工具註冊表的既有分工原則），見該函式文件與下方「D12：自動暫停偵測」一節。
//!
//! task 5.8（本機記錄檔，見 `crate::logging`）：`gatekeeper.log`（`REBOTTOM …`／
//! `SHOWDESKTOP …`）**刻意不併入**統一記錄檔——`host/tools/verify-3.3.ps1`／`verify-3.4.ps1`
//! 已經對它逐行 `Select-String`／正則解析（例如 `^(\S+) SHOWDESKTOP (ENTER|REASSERT|EXIT)\b`
//! 要求時間戳後緊接關鍵字，中間不能插入統一格式會加的 `[target][level]`），這兩支驗收腳本
//! 目前仍是「待解鎖驗收」中、尚未跑過的證據來源；改動它們的解析格式風險大於好處，故
//! `gatekeeper_log` 的格式與檔案維持原樣不動。[`now_string`] 改為 `pub(crate)` 只是把既有的
//! `GetLocalTime` 時間戳邏輯讓 `logging.rs` 共用，不代表兩份記錄檔合併。散落在本模組、
//! `main.rs`、`widgets.rs` 其餘的 `eprintln!`（不是這兩支腳本解析目標、純粹是失敗診斷訊息）
//! 才換成 `log::warn!`／`log::error!`，寫進統一記錄檔。
//!
//! task 5.8 另外補上 [`install_process_failed_logger`]：design.md D12「WebView2 故障」條列出
//! 的 `ProcessFailed` 事件種類**只記錄、不處理**——`Reload()`／browser 行程重建等復原動作是
//! task 5.6 的範圍（design.md 已依探針 1.4 的實測結論定案復原策略）。task 5.8 的 brief 明講
//! 「故障（5.6）…都要寫入」記錄檔，且驗收要求「終止 renderer 一次，確認記錄檔出現」；當時
//! `src/` 內完全沒有訂閱 `ProcessFailed` 的程式碼（只有 `examples/probe_webview2.rs` 這個
//! task 1.4 的獨立探針），沒有東西會在 renderer 被殺掉時寫記錄，故補這個最小訂閱點——事件
//! 處理常式的訂閱與型別對照直接沿用探針已驗證過的寫法（`examples/probe_webview2.rs` 的
//! `attach`／`on_process_failed`/`failed_kind_name`/`reason_name`），只是拿掉探針才需要的
//! 心跳／行程對照／`recreate` 機制，且不呼叫 `Reload()` 等復原動作。task 5.6 之後若要接上
//! 復原，直接在 [`log_process_failed`] 的 `match kind` 分支裡加對應動作即可，不需要換掉
//! 訂閱機制本身。
//!
//! task 5.6 據此把 `install_process_failed_logger` 原地擴充成
//! [`install_process_failed_handler`]：記錄格式不變，另把故障種類翻成
//! [`crate::recovery::WebviewFailureKind`] 交給呼叫端回呼（復原決策在 `crate::recovery`），
//! 並提供對事件來源 webview 的 `Reload()`；仍是唯一的 `add_ProcessFailed` 訂閱點。
//!
//! task 5.7 補上（重新啟動註冊，design.md D12「重新啟動註冊」）：
//! - [`register_for_restart`]：啟動時（`main.rs` 的 `.setup()` 閉包，只有勝出的單一執行個體
//!   會跑到這裡——輸掉 single-instance 判定的第二個行程在 `tauri::Builder::build()`
//!   內部就 `std::process::exit(0)`，不會執行到 `.setup()`，見 `tray.rs` 模組文件「單一執行
//!   個體」一節）呼叫 `RegisterApplicationRestart("--restarted", …)`，重新啟動時帶的命令列
//!   參數與開機自啟／`is_automated_relaunch` 判斷的旗標同一個字串。
//! - [`unregister_restart`]：系統匣「結束」（`tray::quit`）時呼叫
//!   `UnregisterApplicationRestart`，確保使用者主動結束不會被系統重新啟動（specs
//!   「Restart Manager 關閉後重新啟動」Requirement 的 MUST NOT 那句）。
//! - `dwFlags` 選擇 `RESTART_NO_CRASH | RESTART_NO_HANG`（查證 Microsoft Learn
//!   〈RegisterApplicationRestart function〉：<https://learn.microsoft.com/windows/win32/api/winbase/nf-winbase-registerapplicationrestart>）：
//!   文件原話——「如果應用程式遇到未處理的例外狀況或無回應，使用者會被詢問是否要重新啟動；
//!   應用程式不會在未經使用者同意下自動重新啟動；但如果應用程式正在被更新且需要重新啟動，
//!   則會自動重新啟動」。也就是說：只要不設 `RESTART_NO_PATCH`／`RESTART_NO_REBOOT`，「因
//!   Restart Manager 關閉以完成更新」這條路徑本來就是**自動**重新啟動（無需使用者同意）—
//!   —design.md 這句「自動重啟只發生在經 Restart Manager 關閉並由其要求重啟時」描述的正是
//!   這個預設行為，不需要額外旗標去啟用。反過來，crash／hang 兩種情境預設會跳出「要不要
//!   重新啟動」的系統對話框；本宿主是常駐系統匣的背景程式、崩潰或卡死時彈出這種對話框不是
//!   期望行為（且 WebView2 的故障已由 task 5.6 的核心自行復原，不靠 OS 層重啟），故明確設
//!   `RESTART_NO_CRASH`／`RESTART_NO_HANG` 排除這兩種情境，只保留「被更新／Restart Manager
//!   關閉」這條唯一預期會重新啟動的路徑，與本 task 的驗收腳本（RM 關閉後應重啟、系統匣結束
//!   不應重啟）語意一致。

#![allow(dead_code)]

// task 5.1 fix round 1：啟動仲裁（補 single-instance plugin 同時啟動的競態），Win32 具名 mutex
// 同樣屬於本模組的職責範圍，獨立成子模組以免本檔再膨脹，見 `desktop/instance.rs` 模組文件。
mod instance;
pub use instance::{
    arbitrate_startup, arbitrate_startup_within, release_startup_lock, try_acquire_instance_lock,
    InstanceLock, StartupArbitration, StartupRole,
};

// dynamic-wallpaper task 4.2：桌布 COM 執行緒（design.md D3）。`IDesktopWallpaper` 的 Win32 呼叫在
// `desktop/wallpaper/com_backend.rs`，桌布相關登錄（不經 COM 執行緒）在 `desktop/wallpaper/registry.rs`，
// 見 `desktop/wallpaper.rs` 模組文件。
pub mod wallpaper;

// dynamic-wallpaper task 4.7a：本機時區（排程器的 `UtcOffsetSource` Windows 實作），見
// `desktop/timezone.rs` 模組文件。
pub mod timezone;

// dynamic-wallpaper task 4.7b：`--restore-wallpaper` 交給執行中的宿主（single-instance 交接視窗
// 的 WM_COPYDATA＋具名事件回覆），見 `desktop/handoff.rs` 模組文件。
pub mod handoff;

// dynamic-wallpaper task 4.9：explorer 資源安全閥的讀值（殼層擁有者 PID＋GetGuiResources），見
// `desktop/explorer_gdi.rs` 模組文件。
pub mod explorer_gdi;

// dynamic-wallpaper task 4.8：系統匣通知（對宿主既有的系統匣圖示發 `NIF_INFO` 通知），見
// `desktop/tray_balloon.rs` 模組文件。
pub mod tray_balloon;

// dynamic-wallpaper task 4.7c：勿打擾的兩個系統來源（WinRT FocusSessionManager＋非官方 WNF），只讀不切換，
// 見 `desktop/dnd.rs` 模組文件；判讀與輪詢在 `crate::wallpaper_dnd`。
pub mod dnd;

// bug-leaked-renderer：桌布渲染視窗那組 WebView2 browser 行程的把手（等它結束、核對身分、必要時結束它），
// 見 `desktop/browser_process.rs` 模組文件。
pub mod browser_process;

// installer-auto-update task 3.3（design.md D4）：`--wait-exit <pid>`——啟動仲裁之前驗證並等舊宿主行程結束，
// 見 `desktop/wait_exit.rs` 模組文件。
pub mod wait_exit;

// widget-adaptive-zoom-and-grid task 5.1（widget-adaptive-zoom-and-grid design.md D4）：編輯版面的格線疊加
// 視窗（每台顯示器一個滑鼠穿透的分層視窗），見 `desktop/grid_overlay.rs` 模組文件。
pub mod grid_overlay;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write as _;
use std::mem::size_of;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

use tauri::{AppHandle, Monitor, Runtime, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2ProcessFailedEventArgs, ICoreWebView2ProcessFailedEventArgs2,
    COREWEBVIEW2_PROCESS_FAILED_KIND, COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED,
    COREWEBVIEW2_PROCESS_FAILED_KIND_FRAME_RENDER_PROCESS_EXITED,
    COREWEBVIEW2_PROCESS_FAILED_KIND_GPU_PROCESS_EXITED,
    COREWEBVIEW2_PROCESS_FAILED_KIND_PPAPI_BROKER_PROCESS_EXITED,
    COREWEBVIEW2_PROCESS_FAILED_KIND_PPAPI_PLUGIN_PROCESS_EXITED,
    COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED,
    COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE,
    COREWEBVIEW2_PROCESS_FAILED_KIND_SANDBOX_HELPER_PROCESS_EXITED,
    COREWEBVIEW2_PROCESS_FAILED_KIND_UTILITY_PROCESS_EXITED, COREWEBVIEW2_PROCESS_FAILED_REASON,
    COREWEBVIEW2_PROCESS_FAILED_REASON_CRASHED, COREWEBVIEW2_PROCESS_FAILED_REASON_LAUNCH_FAILED,
    COREWEBVIEW2_PROCESS_FAILED_REASON_OUT_OF_MEMORY,
    COREWEBVIEW2_PROCESS_FAILED_REASON_PROFILE_DELETED,
    COREWEBVIEW2_PROCESS_FAILED_REASON_TERMINATED, COREWEBVIEW2_PROCESS_FAILED_REASON_UNEXPECTED,
    COREWEBVIEW2_PROCESS_FAILED_REASON_UNRESPONSIVE,
};
use webview2_com::{take_pwstr, ProcessFailedEventHandler};
use windows::core::{w, Interface, PCWSTR, PWSTR};
use windows::Win32::Devices::Display::{
    DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig,
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
    DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME, QDC_ONLY_ACTIVE_PATHS,
};
use windows::Win32::Foundation::{
    LocalFree, ERROR_FILE_NOT_FOUND, ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, HANDLE, HLOCAL,
    HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM,
};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::Storage::FileSystem::{GetFullPathNameW, GetLongPathNameW};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Diagnostics::Debug::{
    GetErrorMode, SetErrorMode, SEM_NOGPFAULTERRORBOX, THREAD_ERROR_MODE,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Power::{
    GetSystemPowerStatus, RegisterPowerSettingNotification, POWERBROADCAST_SETTING,
    SYSTEM_POWER_STATUS,
};
use windows::Win32::System::Recovery::{
    RegisterApplicationRestart, UnregisterApplicationRestart, RESTART_NO_CRASH, RESTART_NO_HANG,
};
use windows::Win32::System::Registry::{
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE,
    REG_SZ, RRF_NOEXPAND, RRF_RT_ANY, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
};
use windows::Win32::System::RemoteDesktop::{
    WTSFreeMemory, WTSQuerySessionInformationW, WTSRegisterSessionNotification, WTSSessionInfoEx,
    NOTIFY_FOR_THIS_SESSION, WTSINFOEXW, WTS_CURRENT_SESSION, WTS_SESSIONSTATE_LOCK,
    WTS_SESSIONSTATE_UNLOCK,
};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::SystemServices::{
    GUID_ACDC_POWER_SOURCE, GUID_CONSOLE_DISPLAY_STATE, GUID_POWER_SAVING_STATUS,
};
use windows::Win32::System::Threading::{
    GetCurrentThread, GetCurrentThreadId, GetThreadDescription,
};
use windows::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
use windows::Win32::UI::Shell::{
    DefSubclassProc, FOLDERID_LocalAppData, RemoveWindowSubclass, SHGetKnownFolderPath,
    SHQueryUserNotificationState, SetWindowSubclass, KNOWN_FOLDER_FLAG,
    QUERY_USER_NOTIFICATION_STATE, QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetClassNameW, GetCursorPos, GetDesktopWindow, GetShellWindow, GetTopWindow,
    GetWindow, GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindow,
    IsWindowVisible, KillTimer, PostMessageW, RegisterWindowMessageW, SetTimer, SetWindowLongPtrW,
    SetWindowPos, CHILDID_SELF, DEVICE_NOTIFY_WINDOW_HANDLE, ENDSESSION_CLOSEAPP,
    ENDSESSION_CRITICAL, ENDSESSION_LOGOFF, EVENT_OBJECT_CLOAKED, EVENT_OBJECT_HIDE,
    EVENT_OBJECT_REORDER, EVENT_OBJECT_SHOW, EVENT_OBJECT_UNCLOAKED, EVENT_SYSTEM_FOREGROUND,
    EVENT_SYSTEM_MINIMIZEEND, EVENT_SYSTEM_MINIMIZESTART, GA_ROOT, GWL_EXSTYLE, GWL_STYLE,
    GW_HWNDNEXT, GW_HWNDPREV, HWND_BOTTOM, HWND_TOP, OBJID_WINDOW, PBT_POWERSETTINGCHANGE,
    SPI_SETWORKAREA, SWP_FRAMECHANGED, SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, WINDOWPOS, WINEVENT_OUTOFCONTEXT,
    WMSZ_BOTTOM, WMSZ_BOTTOMLEFT, WMSZ_BOTTOMRIGHT, WMSZ_LEFT, WMSZ_RIGHT, WMSZ_TOP, WMSZ_TOPLEFT,
    WMSZ_TOPRIGHT, WM_APP, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_ENDSESSION, WM_ENTERSIZEMOVE,
    WM_EXITSIZEMOVE, WM_GETDPISCALEDSIZE, WM_MOVING, WM_NCDESTROY, WM_POWERBROADCAST,
    WM_QUERYENDSESSION, WM_SETTINGCHANGE, WM_SIZING, WM_TIMER, WM_WINDOWPOSCHANGING,
    WM_WTSSESSION_CHANGE, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_MAXIMIZEBOX,
    WS_SIZEBOX, WTS_SESSION_LOCK, WTS_SESSION_UNLOCK,
};

use crate::layout::{MonitorInfo, PhysicalRect, ResizeEdges};
use crate::recovery::WebviewFailureKind;
use crate::settings::{AppearanceMode, MonitorId};

/// 嘗試重新查詢顯示組態的最大次數（見下方 `source_name_to_device_path` 內的重試迴圈說明）。
const MAX_QUERY_ATTEMPTS: u32 = 8;

/// dynamic-wallpaper task 4.7a 修正輪 1（審查 low 4）：以螢幕矩形（實體像素、虛擬桌面座標；
/// `IDesktopWallpaper::GetMonitorRECT` 的值）直接查那台螢幕的有效 DPI——`MonitorFromRect`
/// （`MONITOR_DEFAULTTONULL`：矩形不在任何螢幕上時不硬配最近的一台）＋`GetDpiForMonitor`
/// （`MDT_EFFECTIVE_DPI`，與縮放比例對應：96＝100%）。不依賴另一份顯示器清單的矩形逐像素相等。
/// 查不到回 `None`（呼叫端記警告並退回 96）。本行程為每螢幕 DPI 感知，座標即實體像素。
pub fn dpi_for_rect(rect: crate::layout::PhysicalRect) -> Option<u32> {
    use windows::Win32::Graphics::Gdi::{MonitorFromRect, MONITOR_DEFAULTTONULL};
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    let r = windows::Win32::Foundation::RECT {
        left: rect.x,
        top: rect.y,
        right: rect.x.checked_add(rect.width)?,
        bottom: rect.y.checked_add(rect.height)?,
    };
    // SAFETY: `r` 是區域變數，呼叫期間有效；MonitorFromRect 只讀取它。
    let monitor = unsafe { MonitorFromRect(&r, MONITOR_DEFAULTTONULL) };
    if monitor.is_invalid() {
        return None;
    }
    let (mut x, mut y) = (0u32, 0u32);
    // SAFETY: `monitor` 剛由 MonitorFromRect 取得；兩個輸出指標指向區域變數。
    unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y) }.ok()?;
    (x > 0).then_some(x)
}

/// 以 `QueryDisplayConfig`＋`DisplayConfigGetDeviceInfo` 取得「GDI 來源裝置名稱
/// （`\\.\DISPLAYn`，與 Tauri `Monitor.name` 同一來源）→ 穩定的 `monitorDevicePath`」對照表
/// （design.md D9）。
///
/// 重試：`GetDisplayConfigBufferSizes` 查得的陣列大小與實際 `QueryDisplayConfig` 呼叫之間，
/// 顯示器組態可能剛好變動（例如拔插），此時 `QueryDisplayConfig` 回傳
/// `ERROR_INSUFFICIENT_BUFFER`——Microsoft Learn 文件建議的處理方式是重新查詢大小後再試一次；
/// 這裡設上限 [`MAX_QUERY_ATTEMPTS`] 次，避免理論上的無窮迴圈。
///
/// 任何一步失敗（含重試耗盡）只回傳空／不完整的 map，不 panic——呼叫端
/// [`monitors_from_tauri`] 對查不到的顯示器有退回策略（見其文件），[`crate::layout`] 的
/// `resolve_grid_placements` 對「顯示器不在清單」也有退回策略（退回主螢幕）。
pub fn source_name_to_device_path() -> HashMap<String, String> {
    let mut map = HashMap::new();

    for _ in 0..MAX_QUERY_ATTEMPTS {
        let mut path_count: u32 = 0;
        let mut mode_count: u32 = 0;
        // SAFETY: 只寫入兩個局部 u32；flags 用文件建議值 QDC_ONLY_ACTIVE_PATHS（只列出目前
        // 作用中的路徑，數量才會與 Tauri 列舉到的螢幕數一致）。
        let sizes_status = unsafe {
            GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count)
        };
        if sizes_status != ERROR_SUCCESS {
            return map;
        }

        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];
        // SAFETY: `paths`／`modes` 的長度剛好等於上一步查得的大小，`QueryDisplayConfig` 依
        // `numpatharrayelements`／`nummodeinfoarrayelements` 指標指向的數量寫入、不會超界；
        // 兩個輸出參數同時也是輸入（告知緩衝區大小）。`currenttopologyid` 傳 `None`——本函式
        // 不需要目前的拓樸 id。
        let query_status = unsafe {
            QueryDisplayConfig(
                QDC_ONLY_ACTIVE_PATHS,
                &mut path_count,
                paths.as_mut_ptr(),
                &mut mode_count,
                modes.as_mut_ptr(),
                None,
            )
        };

        if query_status == ERROR_SUCCESS {
            paths.truncate(path_count as usize);
            for path in &paths {
                if let (Some(source_name), Some(device_path)) =
                    (query_source_name(path), query_target_device_path(path))
                {
                    map.insert(source_name, device_path);
                }
            }
            return map;
        }
        if query_status != ERROR_INSUFFICIENT_BUFFER {
            return map;
        }
        // 緩衝區不足＝兩次呼叫之間顯示器數量變了，迴圈重新查一次大小。
    }

    map
}

/// 查詢單一路徑的來源（GPU 輸出埠）GDI 裝置名稱，即 `\\.\DISPLAYn`（與 tao／Tauri
/// `Monitor.name` 同一來源：`GetMonitorInfoW().szDevice`）。
fn query_source_name(path: &DISPLAYCONFIG_PATH_INFO) -> Option<String> {
    let mut request = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
            size: size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
            adapterId: path.sourceInfo.adapterId,
            id: path.sourceInfo.id,
        },
        ..Default::default()
    };
    // SAFETY: `DISPLAYCONFIG_SOURCE_DEVICE_NAME` 是 `#[repr(C)]` 且 `header` 為第一個欄位，
    // 傳入 `header` 的位址等同傳入整個結構的位址——這是 Microsoft Learn 文件記載的標準用法。
    // `header.size` 已設為本結構的實際大小，`DisplayConfigGetDeviceInfo` 只會依此大小讀寫，
    // 不會寫超界；`header.type`／`adapterId`／`id` 皆取自剛由 `QueryDisplayConfig` 填好的
    // `path`，是合法的查詢目標。
    let status = unsafe { DisplayConfigGetDeviceInfo(&mut request.header) };
    (status == 0).then(|| utf16_buf_to_string(&request.viewGdiDeviceName))
}

/// 查詢單一路徑的目標（顯示器）穩定裝置路徑，即 `DISPLAYCONFIG_TARGET_DEVICE_NAME`
/// 的 `monitorDevicePath`（design.md D9：不含解析度、不用 `\\.\DISPLAYn`，拔插重新編號後
/// 不變）。
fn query_target_device_path(path: &DISPLAYCONFIG_PATH_INFO) -> Option<String> {
    let mut request = DISPLAYCONFIG_TARGET_DEVICE_NAME {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
            size: size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
            adapterId: path.targetInfo.adapterId,
            id: path.targetInfo.id,
        },
        ..Default::default()
    };
    // SAFETY: 同 query_source_name——header 是第一個欄位、size 對應實際結構大小，
    // adapterId／id 取自合法的 path。
    let status = unsafe { DisplayConfigGetDeviceInfo(&mut request.header) };
    (status == 0).then(|| utf16_buf_to_string(&request.monitorDevicePath))
}

/// 把定長 UTF-16 緩衝區（如 `viewGdiDeviceName`／`monitorDevicePath`，Win32 以 NUL 結尾字串
/// 填入固定大小陣列）轉成 `String`，在第一個 `0` 處截斷；找不到 `0`（理論上不會，緩衝區夠大）
/// 則使用整個緩衝區。
fn utf16_buf_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// 從 Tauri [`Monitor`] 擷取本模組需要的欄位。
///
/// 存在的原因：`tauri::Monitor`（`tauri-2.12.0/src/window/mod.rs`）的欄位是
/// `pub(crate)`、只能透過 `.name()`／`.work_area()`／`.scale_factor()` 這些 accessor
/// 讀取，**沒有公開建構子**——外部 crate（含本檔的單元測試）無法直接組出 `tauri::Monitor`
/// 字面量。本結構是純資料、任何地方都能組字面量，讓 [`monitors_from_tauri`] 的核心對應邏輯
/// 可以脫離「測試裡生不生得出一個 tauri::Monitor」而獨立單元測試（design.md D9「純對應邏輯要
/// 可單元測試」）。實機呼叫端（task 3.1／3.5）改用 [`monitor_infos_from_tauri_monitors`]，
/// 那裡才會接觸到真正的 `tauri::Monitor`。
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorSnapshot {
    /// tao／Tauri 在 Windows 上填入 `GetMonitorInfoW().szDevice`，即 `\\.\DISPLAYn`；顯示器在
    /// 查詢當下已消失時為 `None`（tao 文件）。
    pub name: Option<String>,
    /// 實體像素，工作區（扣除工作列後的可用範圍）。
    pub work_area: PhysicalRect,
    pub scale_factor: f64,
}

impl From<&Monitor> for MonitorSnapshot {
    fn from(monitor: &Monitor) -> Self {
        let work_area = monitor.work_area();
        MonitorSnapshot {
            name: monitor.name().cloned(),
            work_area: PhysicalRect {
                x: work_area.position.x,
                y: work_area.position.y,
                // 螢幕工作區寬高不會接近 i32::MAX，`as i32` 不會實際溢位；比照 layout.rs
                // 既有風格（該檔的像素矩形換算也用 `as i32`）。
                width: work_area.size.width as i32,
                height: work_area.size.height as i32,
            },
            scale_factor: monitor.scale_factor(),
        }
    }
}

// ── D8：小工具視窗的樣式、擺放與顯示（task 3.1）──────────────────────────────────────
//
// 視窗工廠（`widgets::create_widget_window`）以 Tauri `visible(false)` 建好視窗後，把 HWND
// 交給 [`present_widget`]，由本節一次完成「樣式 → 位置 → 顯示並置底」。順序的理由：
//
// 1. 樣式先套：`WS_EX_TOOLWINDOW` 要在視窗第一次顯示前就位，工作列／Alt+Tab 才不會在顯示
//    瞬間登記它（Microsoft Learn「Managing Taskbar Buttons」：顯示時依延伸樣式決定是否建
//    按鈕）。
// 2. 位置在隱藏狀態下設好：顯示那一刻就在正確位置，不會先在預設位置閃一下。
// 3. 最後以單一 `SetWindowPos(HWND_BOTTOM, SWP_SHOWWINDOW|SWP_NOACTIVATE|…)` 同時顯示與置底
//    （design.md D8）。tao 的 `always_on_bottom` 只在「有人要求改 z-order」時改寫成
//    `HWND_BOTTOM`，新視窗建立當下在最上層（專案 memory「置底視窗建立當下不會置底」），
//    因此顯示與置底必須是同一個呼叫，中間不能有「已顯示、還在上層」的瞬間。
//
// ## 重要不變量：小工具視窗**不得**再呼叫 tao／Tauri 的旗標類 API
//
// 顯示是本模組直接呼叫 Win32 完成的，tao 內部的 `WindowFlags::VISIBLE` 仍是 `false`
// （tao 0.37.1 只有 `set_visible` 會改它）。tao 的 `apply_diff`（`window_state.rs`）在**任何**
// 旗標有差異時都會先做 `if !new.contains(VISIBLE) { ShowWindow(SW_HIDE) }`，並以
// `to_window_styles()` 整組重寫 `GWL_STYLE`／`GWL_EXSTYLE`（抹掉 `WS_EX_TOOLWINDOW`）。
// 所以對小工具視窗呼叫 `show()`／`hide()`／`set_always_on_bottom()`／`set_focusable()`／
// `set_resizable()`／`set_decorations()` 等會觸發旗標差異的 API，都會把視窗藏起來或洗掉
// 樣式。反過來用 Tauri `show()` 同步旗標也不行：建立完成後 tao 已清掉
// `MARKER_DONT_FOCUS`，`show()` 走 `ShowWindow(SW_SHOW)`（會啟用視窗、搶焦點）。
//
// 因此：顯示／隱藏／z-order 一律經本模組的 Win32 函式（[`show_at_bottom`]；task 3.2 的隱藏
// 也應走 `SetWindowPos(SWP_HIDEWINDOW)` 或 `ShowWindow(SW_HIDE)`），Win+D 的暫停置底依探針
// 1.1 定案以子類別化 `WM_WINDOWPOSCHANGING` 處理、不切 tao 旗標。若日後真的必須切 tao 旗標，
// 切完要立刻以本模組重新套樣式並重新顯示（[`present_widget`] 的後兩步）。
// `set_position`／`set_size` 不是旗標變更（tao 內部雖呼叫 `set_window_flags` 清
// `MAXIMIZED`，但值沒變、diff 為空、`apply_diff` 直接返回），不受此限。

/// 小工具視窗延伸樣式的純計算：加 `WS_EX_TOOLWINDOW`（不進工作列與 Alt+Tab）、去
/// `WS_EX_APPWINDOW`（tao 對無 owner 的視窗一律加上它，見 `window_state.rs`
/// `ON_TASKBAR`；它的語意是「強制出現在工作列」，與 `WS_EX_TOOLWINDOW` 並存時行為沒有
/// 一手文件保證，故明確移除）。其餘位元（含 tao 由 `focusable(false)` 產生的
/// `WS_EX_NOACTIVATE`）原樣保留。
pub fn widget_ex_style(current: u32) -> u32 {
    (current | WS_EX_TOOLWINDOW.0) & !WS_EX_APPWINDOW.0
}

/// 對 `hwnd` 套用 [`widget_ex_style`]，並讀回確認。已是目標值時不寫入。
///
/// `GetWindowLongPtrW`／`SetWindowLongPtrW` 在 windows crate 只為 64 位元目標
/// （`x86_64`、`aarch64`、`arm64ec`）提供，涵蓋本專案要支援的 x64 與 ARM64。
pub fn apply_widget_ex_style(hwnd: HWND) -> Result<(), String> {
    // SAFETY: hwnd 是本行程剛建立、仍存活的頂層視窗；讀取 GWL_EXSTYLE 不涉及記憶體所有權。
    let current = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
    let desired = widget_ex_style(current);
    if current == desired {
        return Ok(());
    }
    // SAFETY: 同上；寫入的是延伸樣式位元，不是指標，不影響視窗程序或使用者資料。
    // `u32 as isize` 在 64 位元目標上是零延伸，延伸樣式只用到低 32 位元。
    unsafe { SetWindowLongPtrW(hwnd, GWL_EXSTYLE, desired as isize) };
    // SAFETY: 同上。
    let after = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
    if after == desired {
        Ok(())
    } else {
        Err(format!(
            "套用延伸樣式失敗：期望 0x{desired:08X}，讀回 0x{after:08X}"
        ))
    }
}

/// task 7.6（design.md D7「調整大小的機制」退路）：小工具視窗 `GWL_STYLE` 的純計算——切換
/// `WS_SIZEBOX`（＝`WS_THICKFRAME`），並一律拿掉 `WS_MAXIMIZEBOX`，其餘位元原樣保留。
///
/// task 7.7（Aero Snap 實測，`host/tools/verify-7.7-aero-snap.ps1`）：`WS_THICKFRAME` 與
/// `WS_MAXIMIZEBOX` 同時存在時，系統的視窗吸附會作用在小工具上——移動到螢幕頂端放開被最大化
/// （`IsZoomed`、`WS_MAXIMIZE`，記錄被寫成最大化後的矩形），上下緣拖到螢幕邊放開被「垂直
/// 最大化」（`IsWindowArranged`，拖曳結束時讀到的是吸附後的矩形，超出工作區的放開反而被判合法；
/// 之後 `SetWindowPos` 回推導矩形，`rcNormalPosition` 仍停在吸附前）。拿掉 `WS_MAXIMIZEBOX`
/// 後三種情況都不再發生（探針實測）。建立視窗時另以 `maximizable(false)` 讓 tao 的旗標一致
/// （`crate::widgets::create_widget_window`），這裡是切換樣式時的保險。
pub fn widget_style_with_sizebox(current: u32, enabled: bool) -> u32 {
    let base = current & !WS_MAXIMIZEBOX.0;
    if enabled {
        base | WS_SIZEBOX.0
    } else {
        base & !WS_SIZEBOX.0
    }
}

/// task 7.6（design.md D7「調整大小的機制」）：進出編輯版面時切換小工具視窗的 `WS_SIZEBOX`。
///
/// 探針（`host/tools/probe-7.6-resize.ps1`）實測：沒有 `WS_THICKFRAME` 的視窗上
/// `startResizeDragging` 不會進入系統尺寸迴圈，所以編輯版面期間要有這個樣式。**不可**用 tao 的
/// `set_resizable`（本模組「重要不變量」一節：旗標差異會 `ShowWindow(SW_HIDE)` 並整組重寫樣式）；
/// 這裡直接改 `GWL_STYLE`，再以 `SWP_FRAMECHANGED` 讓系統重算非用戶區（tao 對無邊框視窗的
/// `WM_NCCALCSIZE` 仍回 0＝整個視窗都是用戶區，外觀不變），不帶 `SWP_SHOWWINDOW`／
/// `SWP_HIDEWINDOW`——顯示狀態不變，隱藏中的視窗維持隱藏，不需要「重新顯示」。之後重套一次
/// 延伸樣式（[`apply_widget_ex_style`]，冪等）。已是目標值時不寫入，回傳 `Ok(false)`。
///
/// 只能在擁有者（主）執行緒呼叫：`SetWindowLongPtrW(GWL_STYLE)` 會同步送
/// `WM_STYLECHANGING`／`WM_STYLECHANGED`；工作執行緒請用 [`set_widget_resizable_on_main_thread`]。
pub fn set_widget_resizable(hwnd: HWND, enabled: bool) -> Result<bool, String> {
    // SAFETY: hwnd 是本行程仍存活的頂層視窗；讀取 GWL_STYLE 不涉及記憶體所有權。
    let current = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32;
    let desired = widget_style_with_sizebox(current, enabled);
    if current == desired {
        return Ok(false);
    }
    // SAFETY: 同上；寫入的是樣式位元，不是指標。`u32 as isize` 在 64 位元目標上零延伸。
    unsafe { SetWindowLongPtrW(hwnd, GWL_STYLE, desired as isize) };
    // SAFETY: 只要求系統重算非用戶區；不移動、不改大小、不改 z-order、不啟用、不改顯示狀態。
    unsafe {
        SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_NOMOVE
                | SWP_NOSIZE
                | SWP_NOZORDER
                | SWP_NOOWNERZORDER
                | SWP_NOACTIVATE
                | SWP_FRAMECHANGED,
        )
    }
    .map_err(|e| format!("SetWindowPos(SWP_FRAMECHANGED) 失敗：{e}"))?;
    apply_widget_ex_style(hwnd)?;
    // SAFETY: 同上。
    let after = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32;
    let checked = WS_SIZEBOX.0 | WS_MAXIMIZEBOX.0;
    if after & checked == desired & checked {
        Ok(true)
    } else {
        Err(format!(
            "切換 WS_SIZEBOX／WS_MAXIMIZEBOX 失敗：期望 0x{desired:08X}，讀回 0x{after:08X}"
        ))
    }
}

/// [`set_widget_resizable`] 排進主執行緒執行（在主執行緒上呼叫時同步執行，理由同
/// [`install_show_desktop_subclass_on_main_thread`]）。排隊期間視窗可能已被銷毀，先 `IsWindow`。
///
/// `enabled` 是在主執行緒**真正執行時**才呼叫的判斷（fix F1，review 7.6 L1）：工作執行緒建立
/// 視窗後排隊，排隊期間使用者可能已離開編輯版面；若在排隊當下就決定值，之後才執行的
/// `set_widget_resizable(hwnd, true)` 會在鎖定狀態下留下 `WS_SIZEBOX`。值已確定的呼叫端傳
/// `move || value` 即可。
pub fn set_widget_resizable_on_main_thread(
    app: &AppHandle,
    hwnd: HWND,
    enabled: impl FnOnce() -> bool + Send + 'static,
) {
    let raw = hwnd.0 as isize;
    let scheduled = app.run_on_main_thread(move || {
        let hwnd = hwnd_from(raw);
        // SAFETY: IsWindow 對任意值都安全。
        if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
            return;
        }
        let enabled = enabled();
        match set_widget_resizable(hwnd, enabled) {
            Ok(true) => log::info!("小工具視窗 WS_SIZEBOX 已切換為 {enabled}（hwnd=0x{raw:X}）"),
            Ok(false) => {}
            Err(err) => log::warn!("小工具視窗切換 WS_SIZEBOX 失敗（enabled={enabled}）：{err}"),
        }
    });
    if let Err(err) = scheduled {
        log::warn!("小工具視窗切換 WS_SIZEBOX 無法排進主執行緒：{err}");
    }
}

/// `WM_SIZING` 的 `wParam`（Microsoft Learn「WM_SIZING message」：`WMSZ_*`）→ 被拖邊
/// （task 7.6；design.md D7「調整大小只對齊被拖曳的邊」）。文件以外的值回傳 `None`。純函式。
pub fn resize_edges_from_wmsz(wmsz: u32) -> Option<ResizeEdges> {
    Some(match wmsz {
        WMSZ_LEFT => ResizeEdges::LEFT,
        WMSZ_RIGHT => ResizeEdges::RIGHT,
        WMSZ_TOP => ResizeEdges::TOP,
        WMSZ_BOTTOM => ResizeEdges::BOTTOM,
        WMSZ_TOPLEFT => ResizeEdges::TOP_LEFT,
        WMSZ_TOPRIGHT => ResizeEdges::TOP_RIGHT,
        WMSZ_BOTTOMLEFT => ResizeEdges::BOTTOM_LEFT,
        WMSZ_BOTTOMRIGHT => ResizeEdges::BOTTOM_RIGHT,
        _ => return None,
    })
}

/// 讀取視窗目前的外框矩形（實體像素，虛擬桌面座標）。
pub fn window_rect(hwnd: HWND) -> Result<PhysicalRect, String> {
    let mut rect = RECT::default();
    // SAFETY: `rect` 是局部變數，`GetWindowRect` 只寫入這個結構。
    unsafe { GetWindowRect(hwnd, &mut rect) }.map_err(|e| format!("GetWindowRect 失敗：{e}"))?;
    Ok(PhysicalRect {
        x: rect.left,
        y: rect.top,
        width: rect.right - rect.left,
        height: rect.bottom - rect.top,
    })
}

/// 設定視窗外框矩形（實體像素），不改 z-order、不啟用視窗；回傳讀回的實際矩形。
///
/// 最多設兩次：移到另一台 DPI 不同的顯示器時，系統會送 `WM_DPICHANGED`，tao 依「邏輯尺寸
/// 不變」改寫成新 DPI 下的大小（`event_loop.rs` 的 `WM_DPICHANGED` 分支），第一次呼叫後的
/// 尺寸因此可能不是我們要的；此時視窗已在目標顯示器上，第二次呼叫不會再跨 DPI，必定精確。
/// `SetWindowPos` 跨執行緒呼叫時以同步訊息送到擁有者執行緒處理，回傳時變更已完成，讀回
/// 的矩形可信。
pub fn set_window_rect(hwnd: HWND, rect: PhysicalRect) -> Result<PhysicalRect, String> {
    let mut actual = window_rect(hwnd)?;
    for _ in 0..2 {
        if actual == rect {
            break;
        }
        // SAFETY: hwnd 是本行程仍存活的頂層視窗；SWP_NOZORDER 保證不動 z-order、
        // SWP_NOACTIVATE 保證不啟用。
        unsafe {
            SetWindowPos(
                hwnd,
                None,
                rect.x,
                rect.y,
                rect.width,
                rect.height,
                SWP_NOZORDER | SWP_NOACTIVATE,
            )
        }
        .map_err(|e| format!("SetWindowPos（設定矩形）失敗：{e}"))?;
        actual = window_rect(hwnd)?;
    }
    Ok(actual)
}

/// 一次完成「顯示＋置底」（design.md D8）：
/// `SetWindowPos(HWND_BOTTOM, SWP_SHOWWINDOW|SWP_NOACTIVATE|SWP_NOMOVE|SWP_NOSIZE)`。
///
/// 另加 `SWP_FRAMECHANGED`：`SetWindowLongPtr` 改過的延伸樣式在下一次 `SetWindowPos`
/// 才會完整生效（Microsoft Learn `SetWindowLongPtrW`：「Certain window data is cached, so
/// changes you make using SetWindowLongPtr will not take effect until you call the
/// SetWindowPos function」並建議搭配 `SWP_FRAMECHANGED`），合在同一個呼叫裡不多一次視窗
/// 變動。
pub fn show_at_bottom(hwnd: HWND) -> Result<(), String> {
    // SAFETY: hwnd 是本行程仍存活的頂層視窗；位置與尺寸由 NOMOVE／NOSIZE 保持不變。
    unsafe {
        SetWindowPos(
            hwnd,
            Some(HWND_BOTTOM),
            0,
            0,
            0,
            0,
            SWP_SHOWWINDOW | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_FRAMECHANGED,
        )
    }
    .map_err(|e| format!("SetWindowPos（顯示並置底）失敗：{e}"))
}

/// 隱藏小工具視窗（design.md D7：內容高度為 0 時隱藏）：`SetWindowPos(SWP_HIDEWINDOW)`。
///
/// **不得**改用 Tauri `hide()`／`set_visible(false)`——本節開頭的不變量：tao 內部
/// `WindowFlags::VISIBLE` 已經因為顯示走 Win32 而與實際狀態不同步，呼叫 `hide()` 會觸發
/// `apply_diff` 整組重寫樣式，抹掉 `WS_EX_TOOLWINDOW`。不改位置、尺寸、z-order、不啟用視窗
/// （`SWP_NOACTIVATE`／`SWP_NOMOVE`／`SWP_NOSIZE`／`SWP_NOZORDER`）——之後 [`show_at_bottom`]
/// 重新顯示時才一併決定 z-order。
pub fn hide_widget(hwnd: HWND) -> Result<(), String> {
    // SAFETY: hwnd 是本行程仍存活的頂層視窗；四個 NO* 旗標保證除了可見性以外什麼都不動。
    unsafe {
        SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_HIDEWINDOW | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER,
        )
    }
    .map_err(|e| format!("SetWindowPos（隱藏）失敗：{e}"))
}

/// 讀取視窗目前是否可見（`IsWindowVisible`）。供
/// [`crate::widgets::apply_report_content`] 判斷「從無內容變回有內容」（或編輯版面切換）時要
/// 不要走 [`show_at_bottom`] 重新顯示（已經可見時只需要 [`set_window_rect`] 調整尺寸，重複
/// 顯示是多餘的 z-order 動作）。
pub fn is_widget_visible(hwnd: HWND) -> bool {
    // SAFETY: hwnd 是本行程仍存活的頂層視窗；只讀取狀態，不涉及記憶體所有權。
    unsafe { IsWindowVisible(hwnd) }.as_bool()
}

/// 視窗工廠的桌面層入口：對仍隱藏的小工具視窗依序套樣式、設矩形、顯示並置底（順序理由見
/// 本節開頭）。回傳實際矩形。任何一步失敗即回傳 `Err`，視窗維持隱藏（樣式或矩形失敗時
/// 還沒顯示），由呼叫端決定是否銷毀。
///
/// task 3.4：顯示前另外安裝 [`install_show_desktop_subclass`]（Win+D 期間暫停 tao 的強制
/// 置底）。安裝失敗**不**中止——小工具照常顯示並置底，只是 Win+D 時會被 tao 壓回桌面之下
/// （退化成沒有 Win+D 功能），比整個小工具建立失敗更能接受；失敗原因記到記錄檔。
///
/// task 5.6 修正：`SetWindowSubclass` 只能在視窗的**擁有者執行緒**呼叫（Microsoft Learn
/// `SetWindowSubclass`：「You cannot use the subclassing helper functions to subclass a
/// window across threads」），而 tao 的視窗一律建在主（事件迴圈）執行緒上。視窗工廠在啟動
/// 時由主執行緒呼叫，但 `update_settings` 開啟小工具與 5.6 故障復原重建都在工作執行緒呼叫
/// ——原本直接呼叫會失敗（5.6 實機記錄：每個重建視窗都記一行「SetWindowSubclass 失敗」），
/// 那些視窗在 Win+D 時被壓回桌面之下。改經 `AppHandle::run_on_main_thread` 安裝：在主執行緒
/// 上呼叫時 tauri-runtime-wry 直接同步執行（`send_user_message` 比對 `main_thread_id`），
/// 行為與原本相同；在工作執行緒上呼叫時排進事件迴圈、稍後在主執行緒執行。
pub fn present_widget(
    app: &AppHandle,
    hwnd: HWND,
    rect: PhysicalRect,
) -> Result<PhysicalRect, String> {
    apply_widget_ex_style(hwnd)?;
    let actual = set_window_rect(hwnd, rect)?;
    install_show_desktop_subclass_on_main_thread(app, hwnd);
    show_at_bottom(hwnd)?;
    Ok(actual)
}

/// task 7.2（design.md D9「空間不足，暫時隱藏」）：建立當下推導結果為空間不足的小工具——
/// 套好與 [`present_widget`] 相同的樣式與 Win+D 子類別化，但**不**設矩形、不顯示。之後
/// 重新推導得到空位時，呼叫端以 [`set_window_rect`]＋[`show_at_bottom`] 顯示（與 D7「無內容」
/// 隱藏後重新顯示同一條路徑），此時樣式已就緒、不會出現在工作列或 Alt+Tab。
pub fn prepare_widget_hidden(app: &AppHandle, hwnd: HWND) -> Result<(), String> {
    apply_widget_ex_style(hwnd)?;
    install_show_desktop_subclass_on_main_thread(app, hwnd);
    Ok(())
}

/// 見 [`present_widget`] 的 task 5.6 修正說明。`HWND` 不是 `Send`，以整數值跨執行緒傳遞，
/// 在主執行緒上先以 `IsWindow` 確認視窗仍存在（排隊期間可能已被銷毀）。
fn install_show_desktop_subclass_on_main_thread(app: &AppHandle, hwnd: HWND) {
    let raw = hwnd.0 as isize;
    let scheduled = app.run_on_main_thread(move || {
        let hwnd = hwnd_from(raw);
        // SAFETY: IsWindow 對任意值都安全。
        if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
            return;
        }
        if let Err(err) = install_show_desktop_subclass(hwnd) {
            log::warn!("小工具視窗 Win+D 子類別化失敗（顯示桌面時不會浮在桌面之上）：{err}");
        }
    });
    if let Err(err) = scheduled {
        log::warn!("小工具視窗 Win+D 子類別化無法排進主執行緒：{err}");
    }
}

/// [`install_process_failed_handler`] 交給回呼的一次故障。
pub struct WebviewFailure<'a> {
    /// 收到事件的小工具視窗 label。
    pub label: String,
    pub kind: WebviewFailureKind,
    /// 訂閱當下讀到的 browser 行程 PID（`ICoreWebView2::BrowserProcessId`；讀不到為
    /// `None`）。browser 結束後舊 webview 已查不到，所以在訂閱時先記下，供去重用。
    pub browser_pid: Option<u32>,
    /// 對事件來源 webview 呼叫 `ICoreWebView2::Reload()`；只在回呼執行期間有效。
    pub reload: &'a dyn Fn() -> Result<(), String>,
}

/// task 5.8＋5.6：在小工具的 WebView2 上訂閱 `ProcessFailed`（每個視窗一次，由
/// [`crate::widgets::create_widget_window`] 在 [`present_widget`] 成功之後呼叫）。事件發生時：
///
/// 1. 寫一行記錄（[`log_process_failed`]，task 5.8 的格式 `WebView2 故障 widget=… kind=…`）；
/// 2. 把種類翻成 [`WebviewFailureKind`] 交給 `on_failure`（復原決策在
///    [`crate::recovery::on_webview_failure`]，本模組只提供 WebView2 呼叫本身）。
///
/// task 5.8 當時的 `install_process_failed_logger` 只記錄；5.6 把它**原地擴充**成本函式，
/// 仍然是同一個、也是唯一一個 `add_ProcessFailed` 訂閱點（不另建第二套訂閱，避免同一事件被
/// 處理兩次）。`Reload()` 沿用同一個 `ICoreWebView2`，訂閱不會因重新載入而失效（探針 1.4：
/// 重建後 R2 階段照常收到事件），故不需要、也不可以在重新載入後重訂。
///
/// 訂閱失敗（`with_webview`／`CoreWebView2()`／`add_ProcessFailed` 任一步）只記錄，不影響
/// 小工具視窗本身——此時該小工具只剩看門狗（[`crate::recovery::spawn_watchdog`]）兜底。
pub fn install_process_failed_handler<F>(window: &WebviewWindow, label: &str, on_failure: F)
where
    F: Fn(&WebviewFailure) + Send + 'static,
{
    let sub_label = label.to_string();
    let attach_result = window.with_webview(move |platform_webview| {
        // SAFETY: `with_webview` 的回呼與下面掛上的事件處理常式都由 WebView2 在 UI 執行緒
        // 派送（同 `examples/probe_webview2.rs::attach` 既有查證）；`token`／`core`／`pid`
        // 皆為本閉包內的區域變數，事件處理常式只捕捉 `String`、`Option<u32>` 與 `on_failure`
        // 的所有權；`sender` 由 WebView2 傳入、只在事件處理期間使用。
        unsafe {
            let core = match platform_webview.controller().CoreWebView2() {
                Ok(core) => core,
                Err(err) => {
                    log::warn!(
                        "小工具（{sub_label}）取得 CoreWebView2 失敗，無法訂閱 ProcessFailed：{err}"
                    );
                    return;
                }
            };
            let mut pid = 0u32;
            let browser_pid = core
                .BrowserProcessId(&mut pid)
                .ok()
                .and_then(|()| (pid != 0).then_some(pid));
            let mut token = 0i64;
            let handler_label = sub_label.clone();
            let handler = ProcessFailedEventHandler::create(Box::new(move |sender, args| {
                let Some(kind) = log_process_failed(&handler_label, args) else {
                    return Ok(());
                };
                let reload = || -> Result<(), String> {
                    match &sender {
                        // 仍在外層 unsafe 區塊內：在 UI 執行緒上對事件來源 webview 呼叫
                        // Reload（探針 1.4 同）。
                        Some(core) => core.Reload().map_err(|e| e.to_string()),
                        None => Err("事件來源 ICoreWebView2 為 None".to_string()),
                    }
                };
                on_failure(&WebviewFailure {
                    label: handler_label.clone(),
                    kind,
                    browser_pid,
                    reload: &reload,
                });
                Ok(())
            }));
            match core.add_ProcessFailed(&handler, &mut token) {
                Ok(()) => {
                    log::info!("已訂閱 ProcessFailed widget={sub_label} browserPid={browser_pid:?}")
                }
                Err(err) => log::warn!("小工具（{sub_label}）訂閱 ProcessFailed 失敗：{err}"),
            }
        }
    });
    if let Err(err) = attach_result {
        log::warn!("小工具（{label}）with_webview 失敗，無法訂閱 ProcessFailed：{err}");
    }
}

/// bug-leaked-renderer：讀 `window` 的 WebView2 browser 行程 PID（`ICoreWebView2::BrowserProcessId`），
/// 並在同一個 UI 執行緒回呼裡立刻開把手——webview 還在，這個 PID 一定是它的 browser 行程，之後持有
/// 把手就不怕 PID 被重用。呼叫端在**非主執行緒**等結果（最多 `timeout`；`with_webview` 要靠主執行緒
/// 派送，在主執行緒呼叫會等到逾時）。
pub fn webview_browser_process(
    window: &WebviewWindow,
    timeout: std::time::Duration,
) -> Result<browser_process::ProcessHandle, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    window
        .with_webview(move |platform_webview| {
            // SAFETY: `with_webview` 的回呼在 UI 執行緒執行（同 `install_process_failed_handler`）；
            // `pid` 是本閉包的局部變數，COM 物件只在回呼內使用。
            let result = unsafe {
                platform_webview
                    .controller()
                    .CoreWebView2()
                    .map_err(|e| format!("取得 CoreWebView2 失敗：{e}"))
                    .and_then(|core| {
                        let mut pid = 0u32;
                        core.BrowserProcessId(&mut pid)
                            .map_err(|e| format!("BrowserProcessId 失敗：{e}"))
                            .map(|()| pid)
                    })
            }
            .and_then(browser_process::ProcessHandle::open);
            let _ = tx.send(result);
        })
        .map_err(|e| format!("with_webview 失敗：{e}"))?;
    rx.recv_timeout(timeout)
        .map_err(|_| format!("{} 秒內沒有取得 browser 行程 PID", timeout.as_secs_f64()))?
}

/// `COREWEBVIEW2_PROCESS_FAILED_KIND` → [`WebviewFailureKind`]。
fn failure_kind(kind: COREWEBVIEW2_PROCESS_FAILED_KIND) -> WebviewFailureKind {
    match kind {
        COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED => {
            WebviewFailureKind::RenderProcessExited
        }
        COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE => {
            WebviewFailureKind::RenderProcessUnresponsive
        }
        COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED => {
            WebviewFailureKind::BrowserProcessExited
        }
        COREWEBVIEW2_PROCESS_FAILED_KIND_GPU_PROCESS_EXITED => WebviewFailureKind::GpuProcessExited,
        _ => WebviewFailureKind::Other,
    }
}

/// [`install_process_failed_handler`] 的記錄部分：組出一行記錄並依故障種類分級
/// （`error`：`RENDER_PROCESS_EXITED`／`BROWSER_PROCESS_EXITED`／`RENDER_PROCESS_UNRESPONSIVE`
/// ——小工具內容直接受影響、會觸發復原；其餘種類記 `warn`，例如 `GPU_PROCESS_EXITED`——
/// design.md D12 記錄 browser 約 2 秒內會自動重啟 GPU process，心跳不中斷，只記錄）。回傳
/// 故障種類；`args` 為 `None`（無從判斷）時回傳 `None`、不做復原。
fn log_process_failed(
    label: &str,
    args: Option<ICoreWebView2ProcessFailedEventArgs>,
) -> Option<WebviewFailureKind> {
    let Some(args) = args else {
        log::warn!("小工具（{label}）收到 ProcessFailed 但 args=None");
        return None;
    };
    let mut kind = COREWEBVIEW2_PROCESS_FAILED_KIND::default();
    // SAFETY: kind 為本函式內的區域輸出變數。
    let _ = unsafe { args.ProcessFailedKind(&mut kind) };
    let kind_name = process_failed_kind_name(kind);

    let mut detail = String::new();
    if let Ok(args2) = args.cast::<ICoreWebView2ProcessFailedEventArgs2>() {
        let mut reason = COREWEBVIEW2_PROCESS_FAILED_REASON::default();
        let mut exit_code = 0i32;
        let mut desc = PWSTR::null();
        // SAFETY: 三個輸出指標皆為本函式內的區域變數；`desc` 由 `take_pwstr` 接手釋放
        // （同 `examples/probe_webview2.rs::on_process_failed` 既有查證）。
        unsafe {
            let _ = args2.Reason(&mut reason);
            let _ = args2.ExitCode(&mut exit_code);
            let desc_s = if args2.ProcessDescription(&mut desc).is_ok() {
                take_pwstr(desc)
            } else {
                String::new()
            };
            detail = format!(
                " reason={} exitCode={exit_code} desc={desc_s:?}",
                process_failed_reason_name(reason)
            );
        }
    }

    let failure = failure_kind(kind);
    match failure {
        WebviewFailureKind::RenderProcessExited
        | WebviewFailureKind::RenderProcessUnresponsive
        | WebviewFailureKind::BrowserProcessExited => {
            log::error!("WebView2 故障 widget={label} kind={kind_name}{detail}");
        }
        WebviewFailureKind::GpuProcessExited | WebviewFailureKind::Other => {
            log::warn!("WebView2 故障 widget={label} kind={kind_name}{detail}（只記錄）");
        }
    }
    Some(failure)
}

fn process_failed_kind_name(kind: COREWEBVIEW2_PROCESS_FAILED_KIND) -> &'static str {
    match kind {
        COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED => "BROWSER_PROCESS_EXITED",
        COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED => "RENDER_PROCESS_EXITED",
        COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE => {
            "RENDER_PROCESS_UNRESPONSIVE"
        }
        COREWEBVIEW2_PROCESS_FAILED_KIND_FRAME_RENDER_PROCESS_EXITED => {
            "FRAME_RENDER_PROCESS_EXITED"
        }
        COREWEBVIEW2_PROCESS_FAILED_KIND_UTILITY_PROCESS_EXITED => "UTILITY_PROCESS_EXITED",
        COREWEBVIEW2_PROCESS_FAILED_KIND_SANDBOX_HELPER_PROCESS_EXITED => {
            "SANDBOX_HELPER_PROCESS_EXITED"
        }
        COREWEBVIEW2_PROCESS_FAILED_KIND_GPU_PROCESS_EXITED => "GPU_PROCESS_EXITED",
        COREWEBVIEW2_PROCESS_FAILED_KIND_PPAPI_PLUGIN_PROCESS_EXITED => {
            "PPAPI_PLUGIN_PROCESS_EXITED"
        }
        COREWEBVIEW2_PROCESS_FAILED_KIND_PPAPI_BROKER_PROCESS_EXITED => {
            "PPAPI_BROKER_PROCESS_EXITED"
        }
        _ => "UNKNOWN_PROCESS_EXITED",
    }
}

fn process_failed_reason_name(reason: COREWEBVIEW2_PROCESS_FAILED_REASON) -> &'static str {
    match reason {
        COREWEBVIEW2_PROCESS_FAILED_REASON_UNEXPECTED => "UNEXPECTED",
        COREWEBVIEW2_PROCESS_FAILED_REASON_UNRESPONSIVE => "UNRESPONSIVE",
        COREWEBVIEW2_PROCESS_FAILED_REASON_TERMINATED => "TERMINATED",
        COREWEBVIEW2_PROCESS_FAILED_REASON_CRASHED => "CRASHED",
        COREWEBVIEW2_PROCESS_FAILED_REASON_LAUNCH_FAILED => "LAUNCH_FAILED",
        COREWEBVIEW2_PROCESS_FAILED_REASON_OUT_OF_MEMORY => "OUT_OF_MEMORY",
        COREWEBVIEW2_PROCESS_FAILED_REASON_PROFILE_DELETED => "PROFILE_DELETED",
        _ => "UNKNOWN",
    }
}

/// 依使用者設定與目前系統狀態決定外觀（design.md D8）：讀三個系統狀態後交給
/// [`resolve_appearance`]；探針 1.2 結論（失焦不呈現）以 [`ACRYLIC_WORKS_UNFOCUSED_DEFAULT`]
/// （`false`）傳入——故必為 [`AppearanceMode::Solid`]。
pub fn current_appearance(user_mode: AppearanceMode) -> AppearanceMode {
    resolve_appearance(&AcrylicConditions::from_readings(
        os_build_number(),
        transparency_effects_enabled(),
        power_saver_active(),
        ACRYLIC_WORKS_UNFOCUSED_DEFAULT,
        user_mode,
    ))
}

/// 把外觀套到小工具視窗的**唯一**入口（brief：透明／圓角的套用點做成單一函式）。回傳實際
/// 套用的模式。
///
/// 現行樣式（探針 1.2 已定案毛玻璃停用，design.md D8）：
/// - 純色：視窗本身由工廠以 Tauri `transparent(true)`＋`decorations(false)`＋`shadow(false)`
///   建立，頁面 `widget.css` 的 `.panel` 自己畫半透明深色底與 16 px 圓角，視窗層不需要再做
///   任何事。
/// - 毛玻璃：探針 1.2 實測 DWM 系統背景材質在視窗未取得焦點時不呈現（只剩攤平的純色塊），
///   故停用——即使呼叫端傳入 `Acrylic` 也退回純色並回傳 `Solid`，不呼叫
///   `set_effects`／`DwmSetWindowAttribute`。
pub fn apply_appearance<R: Runtime>(
    window: &WebviewWindow<R>,
    mode: AppearanceMode,
) -> AppearanceMode {
    let _ = window;
    match mode {
        AppearanceMode::Solid => AppearanceMode::Solid,
        // 探針 1.2 定案：失焦時毛玻璃不呈現，停用（design.md D8），退回純色。
        AppearanceMode::Acrylic => AppearanceMode::Solid,
    }
}

// ── D8：置底守門（task 3.3）──────────────────────────────────────────────────────
//
// 兩個觸發來源都會呼叫呼叫端提供的「全部重新置底」回呼（[`GatekeeperCallbacks::rebottom_all`],
// 內部只是對目前每一個仍可見的小工具視窗呼叫既有的 [`show_at_bottom`]，不是新機制）：
//
// 1. explorer 重啟：隱藏頂層守門視窗（[`spawn_gatekeeper`]）收到
//    `RegisterWindowMessageW("TaskbarCreated")` 廣播即無條件全部重新置底（design.md D8）。
// 2. 30 秒保險檢查：[`needs_rebottom`] 判定為真才重新置底，平時不做無謂動作（design.md
//    「小工具之間不因此反覆重排」）。

/// 一次 z-order 列舉中，單一頂層視窗與 [`needs_rebottom`]／[`decide_show_desktop`] 判定
/// 相關的欄位。只含布林旗標與 `hwnd` 的位址值——字面量可直接在測試中組出（不需要真正的
/// Win32 物件），呼叫端（[`zorder_snapshot`]）另外用一個獨立的 `is_widget` 集合比對來決定
/// 每一筆的 `is_widget`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZOrderWindowState {
    /// `HWND.0` 的位址值（task 3.4：插入位置與要重插哪些小工具需要指名視窗；3.3 的
    /// [`needs_rebottom`] 不看這個欄位）。
    pub hwnd: isize,
    /// 這個 HWND 是否為目前已知（且可見）的小工具視窗；由呼叫端依 hwnd 是否在
    /// [`GatekeeperCallbacks::visible_widget_hwnds`] 回傳的集合中標記，本模組不認得小工具
    /// 註冊表（同 `apply_report_content` 的分工原則：Win32 呼叫本身集中在這裡，業務判斷不放
    /// 這裡）。
    pub is_widget: bool,
    pub visible: bool,
    pub minimized: bool,
    pub cloaked: bool,
    pub tool_window: bool,
    /// `GetWindowRect` 寬高皆 > 0。
    pub has_size: bool,
    /// `WS_EX_TOPMOST`（task 3.4：Win+D 判準略過 topmost 視窗，插入位置遇到 topmost 改用
    /// `HWND_TOP`）。
    pub topmost: bool,
    /// 殼層行程（`GetShellWindow` 的 PID）擁有的 `Progman`／`WorkerW`，見
    /// [`is_shell_desktop_window`]（task 3.4）。
    pub shell_desktop: bool,
}

/// 「一般應用程式視窗」判準，與 `host/tools/watch-zorder.ps1` 的 `below` 判準（README「一般
/// 視窗」判準一節）一致：可見、非最小化、非 cloaked、非 tool window、非零尺寸。
fn is_normal_window(w: &ZOrderWindowState) -> bool {
    w.visible && !w.minimized && !w.cloaked && !w.tool_window && w.has_size
}

/// 30 秒保險檢查的判定純函式（design.md D8）：`snapshot` 是一次 z-order 列舉、由上到下
/// （topmost 在前，與 `host/tools/watch-zorder.ps1` 的列舉方向一致）；回傳「是否有可見的
/// 一般應用程式視窗，在 z-order 中排在任一小工具之後（也就是視覺上被那個小工具蓋住）」。
///
/// ## 化簡為「只看最上層那個小工具」
///
/// 判定原文是「存在小工具 W、一般視窗 N，使得 N 在 `snapshot` 中排在 W 之後」。這等價於
/// 「排在 `snapshot` 中**索引最小**（z-order 最上層）的那個小工具之後，存在一般視窗」——
/// 因為若某個一般視窗 N 排在某個小工具 W 之後，N 的索引必然也大於索引最小的那個小工具（後者
/// 的索引是所有小工具中最小的，也就是 N 的索引唯一可能大於或小於其中的最小值），可以直接拿
/// 最小索引的小工具當 W 本身。因此只需要找出最上層小工具的位置、檢查它之後有沒有一般視窗，
/// 不必對每個小工具個別檢查——這也是「小工具之間的順序不列入判斷」自然成立的原因：兩個小工具
/// 之間夾了第三個小工具，或小工具彼此的相對前後順序如何，都不影響「最上層小工具的位置」這個
/// 唯一用得到的量。
///
/// 快照中沒有任何小工具（例如使用者把十個小工具全部關閉）時視為不需要重排。
pub fn needs_rebottom(snapshot: &[ZOrderWindowState]) -> bool {
    let Some(topmost_widget) = snapshot.iter().position(|w| w.is_widget) else {
        return false;
    };
    snapshot[topmost_widget + 1..].iter().any(is_normal_window)
}

/// 讀取視窗是否被 DWM cloak（`DWMWA_CLOAKED`；同 `host/examples/probe_wind.rs` 的
/// `is_cloaked` 寫法）。查詢失敗視為未 cloak（沒有一手文件保證失敗時的語意，選擇不誤判為
/// cloaked——cloaked 只會讓視窗被排除在「一般視窗」判準之外，保守起見查詢失敗時仍讓它有機會
/// 被算進去，而不是靜默略過一個實際上正在遮擋小工具的視窗）。
fn window_is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked: u32 = 0;
    // SAFETY: `cloaked` 是本函式的局部變數，大小與傳入的 `cbattribute` 一致；
    // `DwmGetWindowAttribute` 只會寫入這個結構、不會寫超界。
    let status = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            (&mut cloaked as *mut u32).cast(),
            size_of::<u32>() as u32,
        )
    };
    status.is_ok() && cloaked != 0
}

/// [`needs_rebottom`]／[`decide_show_desktop`] 共用的 Win32 列舉來源：由上到下走訪目前所有
/// 頂層視窗（`GetTopWindow`／`GetWindow(GW_HWNDNEXT)`，走訪方式同
/// `host/tools/watch-zorder.ps1`／`host/examples/probe_wind.rs`），逐一讀取判定需要的旗標。
/// `widget_hwnds`：目前已知且可見的小工具視窗 HWND（以 `HWND.0` 的位址值比對），由呼叫端
/// （`widgets` 模組）提供——本模組不認得小工具註冊表。
///
/// （task 3.3 原名 `snapshot_for_safety_check`；task 3.4 起 Win+D 判準也用同一份快照，改名並
/// 多讀 `hwnd`／`topmost`／`shell_desktop` 三個欄位。）
pub fn zorder_snapshot(widget_hwnds: &HashSet<isize>) -> Vec<ZOrderWindowState> {
    // SAFETY: 純查詢；殼層沒在跑（explorer 重啟中）時回傳空 HWND，PID 記為 0，
    // `is_shell_desktop_window` 對 0 一律回 false。
    let shell = unsafe { GetShellWindow() };
    let shell_pid = if shell.is_invalid() {
        0
    } else {
        window_pid(shell)
    };

    let mut list = Vec::new();
    // SAFETY: 純查詢；`GetTopWindow(None)` 取得 z-order 最上層的頂層視窗，`Err` 代表目前沒有
    // 任何頂層視窗（理論上不會發生，桌面殼層一定存在）。
    let mut cur = unsafe { GetTopWindow(None) }.ok();
    while let Some(h) = cur {
        // SAFETY: `h` 是剛從 `GetTopWindow`／`GetWindow` 取得、仍在 z-order 中的有效 HWND，
        // 以下皆為純查詢。
        let visible = unsafe { IsWindowVisible(h) }.as_bool();
        let is_widget = widget_hwnds.contains(&(h.0 as isize));
        // 不可見的視窗（頂層視窗的大多數）其餘欄位不影響任何判定——`is_normal_window`、
        // `classify_top`、`widgets_below` 都先要求 `visible`——故略過其餘查詢（尤其跨行程的
        // `DwmGetWindowAttribute`／`GetClassNameW`）。task 3.4 起每個相關 WinEvent 都會列舉
        // 一次，這能省下大部分成本。
        let state = if visible {
            let ex_style = unsafe { GetWindowLongPtrW(h, GWL_EXSTYLE) } as u32;
            ZOrderWindowState {
                hwnd: h.0 as isize,
                is_widget,
                visible,
                minimized: unsafe { IsIconic(h) }.as_bool(),
                cloaked: window_is_cloaked(h),
                tool_window: ex_style & WS_EX_TOOLWINDOW.0 != 0,
                has_size: window_rect(h)
                    .map(|r| r.width > 0 && r.height > 0)
                    .unwrap_or(false),
                topmost: ex_style & WS_EX_TOPMOST.0 != 0,
                shell_desktop: is_shell_desktop_window(&window_class(h), window_pid(h), shell_pid),
            }
        } else {
            ZOrderWindowState {
                hwnd: h.0 as isize,
                is_widget,
                visible,
                minimized: false,
                cloaked: false,
                tool_window: false,
                has_size: false,
                topmost: false,
                shell_desktop: false,
            }
        };
        list.push(state);

        // SAFETY: 純查詢，走訪下一個（更靠下）的頂層視窗；`Err` 代表已到最底。
        cur = unsafe { GetWindow(h, GW_HWNDNEXT) }.ok();
    }
    list
}

/// 視窗類別名稱（`GetClassNameW`）；失敗回傳空字串。
fn window_class(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    // SAFETY: `buf` 是本函式的局部可寫緩衝區，windows crate 以切片長度當 `nMaxCount`，
    // `GetClassNameW` 不會寫超界；`hwnd` 失效時 API 回傳 0。
    let len = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..len.max(0) as usize])
}

/// 擁有該視窗的行程 PID（`GetWindowThreadProcessId`）；失敗時為 0。
fn window_pid(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    // SAFETY: `pid` 是本函式的局部變數，API 只寫入這個 u32。
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

/// 記錄用：`0x1234(Class)`。
fn describe_window(hwnd: HWND) -> String {
    format!("0x{:X}({})", hwnd.0 as usize, window_class(hwnd))
}

/// [`spawn_gatekeeper`] 需要的兩個回呼（design.md D8）：本模組不認得小工具註冊表與設定，
/// 「哪些 HWND 目前是可見的小工具」「怎麼把它們全部重新置底」都由呼叫端（`widgets` 模組）
/// 提供。兩者都必須是 `Send + Sync`——存進 `'static` 的 [`GATEKEEPER`]，其內部只捕捉
/// `AppHandle`（Tauri 保證 `Send + Sync`，本 crate 既有的 `thread::spawn(move || …)` 已多處
/// 依賴這一點）。
pub struct GatekeeperCallbacks {
    /// 目前所有「可見」小工具視窗的 HWND（design.md D7：內容為 0 而被 [`hide_widget`] 隱藏的
    /// 小工具不算——保險檢查／`TaskbarCreated` 都不該把它們強制重新顯示，這正是它們必須是
    /// **可見**小工具而非「所有已開啟」小工具的原因）。
    pub visible_widget_hwnds: Box<dyn Fn() -> Vec<HWND> + Send + Sync + 'static>,
    /// 把目前每一個可見小工具視窗重新 [`show_at_bottom`]；回傳實際重排的數量，供記錄
    /// （task-3.3-brief：宿主端加重排次數的記錄）。
    pub rebottom_all: Box<dyn Fn() -> usize + Send + Sync + 'static>,
    /// task 5.5：暫停訊號變化（design.md D12「自動暫停」）。如何轉成
    /// [`crate::widgets::PauseReason`]（含電池／省電是否真的觸發暫停，取決於使用者設定）屬於
    /// 業務判斷，本模組不認得暫停原因集合，交給呼叫端（見「D12：自動暫停偵測」一節）。
    pub on_pause_signal: Box<dyn Fn(PauseSignal) + Send + Sync + 'static>,
    /// task 3.5（design.md D9「多螢幕與 DPI 定位」）：顯示設定變更——重新列舉顯示器並重算
    /// 全部（目前可見）小工具的矩形。哪些小工具算「可見」、怎麼把邏輯內容尺寸換算回矩形，
    /// 都屬於業務判斷，本模組不認得小工具註冊表與設定，交給呼叫端
    /// （[`crate::widgets::relayout_all_widgets`]）。由 [`gatekeeper_subclass_proc`] 在
    /// `WM_DISPLAYCHANGE`／`WM_SETTINGCHANGE(SPI_SETWORKAREA)` 時呼叫；`WM_DPICHANGED`
    /// （縮放改變）走另一條路徑——tao 把它轉譯成 Tauri `WindowEvent::ScaleFactorChanged`
    /// 送到「所在顯示器縮放改變的那扇小工具視窗」，守門視窗（隱藏、位置固定）本身不一定收得到
    /// （`WM_DPICHANGED` 是送給該顯示器上的視窗，不是像 `WM_DISPLAYCHANGE`／
    /// `WM_SETTINGCHANGE` 那樣送給所有頂層視窗），故改在 `main.rs` 的 `App::run` 事件迴圈攔截，
    /// 見其「task 3.5」註解。
    ///
    /// fix F2（review 3.5）：三個來源都不在收到當下呼叫，而是經 [`request_relayout`] 合併、
    /// 延後到守門視窗的 [`RELAYOUT_TIMER_ID`] 計時器到期時才呼叫一次。回傳實際套用矩形的
    /// 小工具數，寫進 `RELAYOUT` 記錄行（驗收據此確認重排真的有作用）。
    pub relayout_all: Box<dyn Fn() -> usize + Send + Sync + 'static>,
    /// dynamic-wallpaper task 4.7a：桌布協調迴圈要知道的系統事件（[`SystemEvent`]）。在守門視窗的
    /// 處理常式裡**同步**呼叫，回呼只能投遞通知、立即返回（`WM_TIMECHANGE` 是同步廣播，見 memory
    /// 「同步 WndProc 裡做慢工作，會卡住發送端」）。不影響本模組其他行為。
    pub on_system_event: Box<dyn Fn(SystemEvent) + Send + Sync + 'static>,
}

/// 守門視窗收到、與小工具無關但桌布協調迴圈需要的系統事件（dynamic-wallpaper task 4.7a，
/// [`GatekeeperCallbacks::on_system_event`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemEvent {
    /// `TaskbarCreated` 廣播：explorer 重新啟動（桌布介面要重建，見 `desktop::wallpaper`）。
    ExplorerRestarted,
    /// `WM_TIMECHANGE`：系統時間被改變。時區改變是否也送這則訊息尚待實機驗證；協調迴圈另以時鐘
    /// 跳動與偏移比對自行偵測，不只依賴它。
    TimeChanged,
    /// dynamic-wallpaper task 4.7b：`WM_QUERYENDSESSION`，或 `WM_ENDSESSION` 且 `wParam` 非 0——
    /// 登出、關機、重新開機、Restart Manager 關閉。桌布協調迴圈據此不還原、停止新的渲染。
    SessionEnding,
    /// dynamic-wallpaper task 4.7b：`WM_ENDSESSION` 且 `wParam` 為 0（有程式否決，工作階段不結束）。
    SessionEndCancelled,
}

/// dynamic-wallpaper task 4.7b：工作階段結束訊息 → 桌布協調迴圈的系統事件（[`end_session_record`]
/// 同一組訊息；其他訊息回傳 `None`）。純函式。
pub fn session_end_event(msg: u32, wparam: usize) -> Option<SystemEvent> {
    match msg {
        WM_QUERYENDSESSION => Some(SystemEvent::SessionEnding),
        WM_ENDSESSION if wparam != 0 => Some(SystemEvent::SessionEnding),
        WM_ENDSESSION => Some(SystemEvent::SessionEndCancelled),
        _ => None,
    }
}

/// 目前是否處於「顯示桌面」（design.md D8 Win+D 條；task 3.4 由 [`evaluate_show_desktop`]
/// 在進入／離開時切換）。三個讀取點：
/// - [`run_safety_check`]：為真時略過保險檢查（design.md D8「顯示桌面期間，保險檢查不得把
///   小工具重新置底」；3.3 的 `SAFETY_CHECK_SUSPENDED` 介面併入此旗標）。
/// - [`widget_subclass_proc`]：為真時把 tao 改寫的 `HWND_BOTTOM` 還原成原請求值。
/// - [`evaluate_show_desktop`]：判定進入／重插／離開。
///
/// **不影響** `TaskbarCreated` 的處理——design.md 該條目沒有「顯示桌面期間略過」的但書，
/// 維持無條件全部重新置底（旗標不變；下一個事件或 30 秒計時器重算判準時，若仍是顯示桌面就
/// 重插、否則離開）。全部存取都在主執行緒（見「D8：Win+D 顯示桌面」一節），`Relaxed` 足夠；
/// 用 atomic 只是為了能放在 `static`。
pub static SHOW_DESKTOP_ACTIVE: AtomicBool = AtomicBool::new(false);

/// fix F2（review 5.5 medium）：`WTSRegisterSessionNotification` 失敗後的重試計時器 id。
const SESSION_NOTIFY_RETRY_TIMER_ID: usize = 4;
/// 重試間隔。登入初期 Remote Desktop Services 尚未就緒時會回 `RPC_S_INVALID_BINDING`
/// （Microsoft Learn `WTSRegisterSessionNotification` 備註：要等 `GlobalTermSrvReadyEvent`），
/// 通常幾秒到幾十秒內就緒。
const SESSION_NOTIFY_RETRY_INTERVAL_MS: u32 = 5_000;
/// 重試上限（含啟動當下那一次）：60 次 × 5 秒＝約 5 分鐘，之後放棄並記錄。
const SESSION_NOTIFY_RETRY_MAX: u32 = 60;

/// 鎖定通知註冊的重試決策（[`session_registration_retry_step`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionRegistrationRetry {
    /// 已註冊成功，停止重試。
    Registered,
    /// 失敗，計時器到期再試。
    RetryLater,
    /// 已達上限仍失敗，放棄（鎖定暫停失效，其餘暫停來源照常）。
    GiveUp,
}

/// `attempts`＝包含這一次在內已嘗試的次數；`succeeded`＝這一次是否成功。
pub fn session_registration_retry_step(
    attempts: u32,
    max: u32,
    succeeded: bool,
) -> SessionRegistrationRetry {
    if succeeded {
        SessionRegistrationRetry::Registered
    } else if attempts >= max {
        SessionRegistrationRetry::GiveUp
    } else {
        SessionRegistrationRetry::RetryLater
    }
}

/// 嘗試註冊工作階段鎖定／解鎖通知一次（`attempts` 含這一次），依結果寫記錄並排定／取消重試
/// 計時器。成功且不是第一次嘗試時主動查詢一次目前的鎖定狀態（重試期間可能漏接鎖定事件）。
fn try_register_session_notification(hwnd: HWND, attempts: u32) {
    // SAFETY: `hwnd` 是本行程的守門視窗（主執行緒擁有、存活到行程結束）；不主動
    // `WTSUnRegisterSessionNotification`（同守門視窗其餘掛鉤：存活到行程結束才由系統回收）。
    let result = unsafe { WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION) };
    match session_registration_retry_step(attempts, SESSION_NOTIFY_RETRY_MAX, result.is_ok()) {
        SessionRegistrationRetry::Registered => {
            // SAFETY: 同一視窗、同一執行緒；計時器不存在時 KillTimer 失敗無害。
            let _ = unsafe { KillTimer(Some(hwnd), SESSION_NOTIFY_RETRY_TIMER_ID) };
            if attempts > 1 {
                gatekeeper_log(&format!(
                    "WTSRegisterSessionNotification 重試成功（第 {attempts} 次）"
                ));
                log::info!("鎖定通知註冊：第 {attempts} 次重試成功");
                if let (Some(state), Some(locked)) = (GATEKEEPER.get(), query_session_locked()) {
                    gatekeeper_log(&format!("RETRY locked={locked}"));
                    (state.callbacks.on_pause_signal)(PauseSignal::Locked(locked));
                }
            }
        }
        SessionRegistrationRetry::RetryLater => {
            let err = result.err().map(|e| e.to_string()).unwrap_or_default();
            gatekeeper_log(&format!(
                "WTSRegisterSessionNotification 失敗（第 {attempts} 次）：{err}；\
                 {SESSION_NOTIFY_RETRY_INTERVAL_MS} ms 後重試"
            ));
            log::warn!("鎖定通知註冊失敗（第 {attempts} 次）：{err}；稍後重試");
            // SAFETY: 同上；同一 id 重設計時器只會重設到期時間。
            if unsafe {
                SetTimer(
                    Some(hwnd),
                    SESSION_NOTIFY_RETRY_TIMER_ID,
                    SESSION_NOTIFY_RETRY_INTERVAL_MS,
                    None,
                )
            } == 0
            {
                log::error!("鎖定通知註冊：重試計時器排不上，放棄重試（鎖定時不會自動暫停）");
            }
        }
        SessionRegistrationRetry::GiveUp => {
            // SAFETY: 同上。
            let _ = unsafe { KillTimer(Some(hwnd), SESSION_NOTIFY_RETRY_TIMER_ID) };
            let err = result.err().map(|e| e.to_string()).unwrap_or_default();
            gatekeeper_log(&format!(
                "WTSRegisterSessionNotification 重試 {attempts} 次仍失敗，放棄：{err}"
            ));
            log::error!(
                "鎖定通知註冊：重試 {attempts} 次仍失敗，放棄（鎖定時不會自動暫停；其餘暫停來源照常）：{err}"
            );
        }
    }
}

/// fix F2（review 3.5）：通知守門視窗「有待執行的重排」的自訂訊息（[`request_relayout`]
/// 以 `PostMessageW` 送出，可從任何執行緒呼叫）。
const WM_APP_RELAYOUT: u32 = WM_APP + 0x35;
/// 延後重排的計時器 id（與 [`SAFETY_CHECK_TIMER_ID`]／[`BUSY_POLL_TIMER_ID`] 不同）。
const RELAYOUT_TIMER_ID: usize = 3;
/// 延後重排的等待時間：一次顯示變更的連串訊息（`WM_DISPLAYCHANGE`、每台顯示器的
/// `SPI_SETWORKAREA`、每扇小工具的 `WM_DPICHANGED`）通常在數十毫秒內送完；250 ms 讓同一波
/// 盡量合併成一次重排，使用者又感覺不到延遲。在這之後才到的訊息會開啟新的一波，正確性不靠
/// 這個數字（見 [`RelayoutCoalescer`]）。
const RELAYOUT_DEBOUNCE_MS: u32 = 250;

/// 守門視窗的固定 label（不得與 `w-<id>`／`settings` 衝突；`widgets::window_label` 的既有
/// 規則只產生 `w-` 開頭的 label，這裡刻意不用該前綴避免任何混淆）。守門視窗以
/// `tauri::window::WindowBuilder`（不掛 WebView）建立，不會出現在 `app.webview_windows()`，
/// 因此也不會被 `widgets::plan_window_changes`／`sync_widget_windows` 誤認成小工具。
const GATEKEEPER_LABEL: &str = "gatekeeper";
/// `SetWindowSubclass` 的子類別 id：同一個 hwnd 只安裝一次，用固定常數即可。
const GATEKEEPER_SUBCLASS_ID: usize = 1;
/// `SetTimer` 的計時器 id。
const SAFETY_CHECK_TIMER_ID: usize = 1;
/// 保險檢查間隔：design.md D8「每 30 秒檢查」。
const SAFETY_CHECK_INTERVAL_MS: u32 = 30_000;
/// task 5.5：`SHQueryUserNotificationState` 輪詢計時器 id（與保險檢查的
/// [`SAFETY_CHECK_TIMER_ID`] 不同，兩者裝在同一個守門視窗上）。
const BUSY_POLL_TIMER_ID: usize = 2;
/// design.md D12：「每 5 秒輪詢」。
const BUSY_POLL_INTERVAL_MS: u32 = 5_000;

struct GatekeeperState {
    /// `RegisterWindowMessageW("TaskbarCreated")` 的回傳值；系統範圍共享（同一字串在所有
    /// 行程得到同一個訊息 id，Microsoft Learn），啟動時查一次即可。
    taskbar_created_message: u32,
    callbacks: GatekeeperCallbacks,
    /// 重排事件記錄檔；開檔失敗（例如路徑不可寫）時為 `None`，只是不記錄，不影響守門邏輯本身
    /// 運作（同 `settings::load_or_default`「不會失敗」的精神：記錄遺失比宿主功能停擺更能
    /// 接受）。
    log: Option<Mutex<fs::File>>,
    /// task 5.5：`SHQueryUserNotificationState` 5 秒輪詢上一次判定的忙碌狀態，只有變化時才
    /// 呼叫 [`GatekeeperCallbacks::on_pause_signal`]（避免每 5 秒都送一次沒有變化的
    /// `pause` 事件）。全部存取都在守門視窗所在的主執行緒（`WM_TIMER` 由訊息迴圈派送），
    /// `Relaxed` 足夠；用 atomic 只是為了能放進 `'static` 的 [`GATEKEEPER`]。
    last_busy: AtomicBool,
    /// 守門視窗本身（[`request_relayout`] 的 `PostMessageW` 目標；`HWND` 不是 `Send`，存成
    /// `isize`）。
    hwnd: isize,
    /// fix F2（review 3.5）：待執行重排的合併旗標，見 [`RelayoutCoalescer`]。
    relayout: Mutex<RelayoutCoalescer>,
    /// fix F2（review 5.5 medium）：`WTSRegisterSessionNotification` 已嘗試的次數（啟動當下算
    /// 第 1 次），重試計時器每次到期遞增。
    session_notify_attempts: AtomicU32,
}

/// 觸發重排的來源（fix F2，review 3.5）：只用於記錄與合併。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayoutReason {
    /// `WM_DISPLAYCHANGE`（解析度、拔接螢幕）。
    DisplayChange,
    /// `WM_SETTINGCHANGE(SPI_SETWORKAREA)`（工作區改變）。
    WorkArea,
    /// `WM_DPICHANGED`（tao `ScaleFactorChanged`，縮放比例改變）。
    DpiChanged,
    /// `TaskbarCreated`（explorer 重新啟動）：工作區可能改變，且建立時取不到殼層桌面視窗、以
    /// `HWND_BOTTOM` 退路顯示的格線要在這裡插回桌面正上方（重排結尾的 `sync_grid_overlay`）。
    ExplorerRestarted,
}

impl RelayoutReason {
    /// 記錄檔用的固定字串（`RELAYOUT reason=…`，驗收腳本據此比對）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DisplayChange => "display-change",
            Self::WorkArea => "workarea-change",
            Self::DpiChanged => "dpi-change",
            Self::ExplorerRestarted => "explorer-restart",
        }
    }
}

/// 重排請求的合併旗標（fix F2，review 3.5 high＋low）。
///
/// 一次顯示變更會連續送來 `WM_DISPLAYCHANGE`、每台顯示器的 `SPI_SETWORKAREA`、每扇小工具的
/// `WM_DPICHANGED`；這些都**不**在收到當下同步重排：
/// - `WM_DPICHANGED`：tao 在 `ScaleFactorChanged` 回呼返回後才 `SetWindowPos(建議矩形)`，同步
///   重排的結果會被它蓋掉。
/// - `WM_SETTINGCHANGE`／`WM_DISPLAYCHANGE`：explorer 以同步廣播送來，在 WndProc 裡重排會拖住
///   發送端（memory「同步 WndProc 裡做慢工作，會卡住發送端」）。
///
/// 改為：第一個請求（[`Self::request`] 回傳 `true`）由呼叫端 `PostMessageW` 給守門視窗；
/// 守門視窗收到後排一個短計時器，到期時 [`Self::take`] 一次取出整波原因、只重排一次。之後才到
/// 的請求開啟新的一波。posted message 與 `WM_TIMER` 都要等目前這則（被 send 進來的）訊息處理
/// 完、回到訊息迴圈才會派送，所以重排一定發生在 tao 套用建議矩形之後。
#[derive(Debug, Default)]
pub struct RelayoutCoalescer {
    pending: Vec<RelayoutReason>,
}

impl RelayoutCoalescer {
    /// 記下一次重排請求。回傳 `true`＝這是新一波的第一個請求，呼叫端要通知守門視窗；
    /// `false`＝已有待執行的重排，合併即可。
    pub fn request(&mut self, reason: RelayoutReason) -> bool {
        let first = self.pending.is_empty();
        if !self.pending.contains(&reason) {
            self.pending.push(reason);
        }
        first
    }

    /// 取出並清空這一波的原因（依首次出現順序、去重）。空＝沒有待執行的重排。通知守門視窗
    /// 失敗時呼叫端也以這個撤回，讓下一個請求重新通知。
    pub fn take(&mut self) -> Vec<RelayoutReason> {
        std::mem::take(&mut self.pending)
    }
}

/// 單例：整個行程只會呼叫一次 [`spawn_gatekeeper`]（`main.rs` 的 `setup`），子類別處理常式
/// 從這裡讀取狀態，不使用 `SetWindowSubclass` 的 `dwrefdata`（避免手動管理裸指標生命週期）。
static GATEKEEPER: OnceLock<GatekeeperState> = OnceLock::new();

/// 記錄檔時間戳（同 `self_test_ipc.rs`／`probe_wind.rs` 既有的 `GetLocalTime` 寫法，取本機
/// 時間，格式一致方便 grep）。`pub(crate)`：task 5.8 的 `logging.rs`（統一記錄檔）也用它組
/// 每行的時間戳，本模組是「其餘模組不得直接呼叫 Win32」規則下唯一能呼叫 `GetLocalTime` 的
/// 地方，故由此對外提供，而不是讓 `logging.rs` 自己再呼叫一次。
pub(crate) fn now_string() -> String {
    let t = local_time();
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, t.wMilliseconds
    )
}

/// 今天的行事曆日（本機時區，`(year, month, day)`）；供 `logging.rs` 判斷「該不該換一份新的
/// 每日輪替記錄檔」（task 5.8）。與 [`now_string`] 共用同一次 `GetLocalTime` 呼叫的邏輯來源
/// （`local_time`），只是只取日期部分。
pub(crate) fn today() -> (i32, u32, u32) {
    let t = local_time();
    (t.wYear as i32, t.wMonth as u32, t.wDay as u32)
}

fn local_time() -> windows::Win32::Foundation::SYSTEMTIME {
    // SAFETY: GetLocalTime 只寫回傳值，無前置條件。
    unsafe { GetLocalTime() }
}

fn gatekeeper_log(msg: &str) {
    let Some(state) = GATEKEEPER.get() else {
        return;
    };
    let Some(log) = &state.log else {
        return;
    };
    if let Ok(mut f) = log.lock() {
        let _ = writeln!(f, "{} {}", now_string(), msg);
    }
}

/// 開啟重排記錄檔（附加寫入，跨重啟保留歷史；驗收腳本需要看到「10 分鐘內無任何重排」，覆寫
/// 會抹掉前一次驗收的證據）。目錄不存在就建立；任何一步失敗回傳 `None`，由
/// [`GatekeeperState::log`] 靜默不記錄。
fn open_gatekeeper_log(path: &Path) -> Option<fs::File> {
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()
}

/// 建立隱藏頂層守門視窗並掛上處理常式（design.md D8）。呼叫時機：`main.rs` 的 `setup` 內，
/// 在 `widgets::sync_widget_windows` 之後（守門的重排動作要能立刻操作到已建立的小工具視窗；
/// 呼叫端傳入的 `callbacks` 每次觸發都即時查詢目前狀態，不依賴呼叫當下的快照）。只能呼叫一次
/// （`GATEKEEPER` 是 `OnceLock`），重複呼叫回傳 `Err`。
///
/// 視窗以 `WebviewWindowBuilder` 建立（`tauri::window::WindowBuilder` 這個不掛 WebView 的
/// 版本，查證 `tauri-2.12.0/src/window/mod.rs` 的 `unstable_struct!` 巨集後確認只在啟用
/// `unstable` cargo feature 時才公開，本專案未啟用該 feature，不引入這個依賴面——沿用既有
/// widget 視窗的建立方式反而更省事，見鐵則「KISS 與不重造輪子」）；載入
/// `host/ui/index.html`（既有的 `frontendDist` 佔位頁，其原始註解已寫明「本檔不會顯示給
/// 使用者」，本來就是為這種用途準備的，不必另建新檔）。屬性：`visible(false)`（永遠不顯示，
/// 本函式之後也不會對它呼叫 [`show_at_bottom`]——它只是訊息接收端，不是小工具，沒有 D8 的
/// 置底不變量要維護）、`focusable(false)`＋`focused(false)`（不搶焦點）、
/// `decorations(false)`（無邊框，雖然永遠不顯示，維持最小意外表面）、`skip_taskbar(true)`
/// （防禦性；反正也不會顯示）。這是**頂層**視窗（無 `parent`／`owner`），不是
/// message-only／`HWND_MESSAGE` 視窗——`TaskbarCreated` 是以 `HWND_BROADCAST` 廣播，
/// message-only 視窗收不到廣播（design.md D8 明文排除）。label 不用 `w-` 前綴（不與
/// `widgets::window_label` 的規則衝突），`widgets::plan_window_changes` 只認得
/// `WIDGET_SPECS` 裡的 id，不會被這個額外的 label 影響（`plan_ignores_non_widget_labels_and_
/// treats_missing_config_as_disabled` 已涵蓋「清單裡混入不認得的 label」這個情境）。
pub fn spawn_gatekeeper(
    app: &AppHandle,
    log_path: &Path,
    callbacks: GatekeeperCallbacks,
) -> Result<(), String> {
    if GATEKEEPER.get().is_some() {
        return Err("gatekeeper 已建立，不可重複呼叫".to_string());
    }

    let window = crate::webview_env::with_widget_data_dir(WebviewWindowBuilder::new(
        app,
        GATEKEEPER_LABEL,
        WebviewUrl::App("index.html".into()),
    ))
    .visible(false)
    .focused(false)
    .focusable(false)
    .decorations(false)
    .skip_taskbar(true)
    .build()
    .map_err(|e| format!("建立守門視窗失敗：{e}"))?;
    let hwnd = window
        .hwnd()
        .map_err(|e| format!("取得守門視窗 HWND 失敗：{e}"))?;

    // SAFETY: 傳入的是靜態字串常量的寬字元字面值（`w!` 巨集），無其餘前置條件；回傳值是
    // 系統範圍共享的訊息 id，非指標，無生命週期疑慮。
    let taskbar_created_message = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };

    let state = GatekeeperState {
        taskbar_created_message,
        callbacks,
        log: open_gatekeeper_log(log_path).map(Mutex::new),
        last_busy: AtomicBool::new(false),
        hwnd: hwnd.0 as isize,
        relayout: Mutex::new(RelayoutCoalescer::default()),
        session_notify_attempts: AtomicU32::new(1),
    };
    // 先把狀態放進 GATEKEEPER，再安裝子類別／計時器——避免訊息在極端情況下搶先於狀態就緒之前
    // 送達（子類別處理常式讀到 `GATEKEEPER.get() == None` 時只是安靜略過，不會 panic，但這樣
    // 順序更沒有僥倖成分）。
    if GATEKEEPER.set(state).is_err() {
        return Err("gatekeeper 狀態重複初始化".to_string());
    }

    // SAFETY: `hwnd` 是本行程剛建立、仍存活、尚未安裝過子類別的頂層視窗；
    // `gatekeeper_subclass_proc` 只讀取上面已經初始化完成的 `'static` 的 `GATEKEEPER`，不使用
    // `dwrefdata`（傳 0）。
    let installed = unsafe {
        SetWindowSubclass(
            hwnd,
            Some(gatekeeper_subclass_proc),
            GATEKEEPER_SUBCLASS_ID,
            0,
        )
    };
    if !installed.as_bool() {
        return Err("SetWindowSubclass 失敗".to_string());
    }

    // SAFETY: `hwnd` 為本行程仍存活的頂層視窗；`lptimerfunc` 傳 `None`，計時器到期時系統以
    // `WM_TIMER` 訊息（而非回呼函式指標）送進這個視窗的訊息佇列，交由上面裝好的子類別處理常式
    // 接手（Microsoft Learn `SetTimer`：`lpTimerFunc` 為 `NULL` 時系統改用 `WM_TIMER` 訊息）。
    if unsafe {
        SetTimer(
            Some(hwnd),
            SAFETY_CHECK_TIMER_ID,
            SAFETY_CHECK_INTERVAL_MS,
            None,
        )
    } == 0
    {
        return Err("SetTimer 失敗".to_string());
    }

    // task 5.5：design.md D12「每 5 秒輪詢」SHQueryUserNotificationState（見「D12：自動暫停
    // 偵測」一節）——同一個計時器機制，另一個 id，裝在同一個守門視窗上。
    if unsafe { SetTimer(Some(hwnd), BUSY_POLL_TIMER_ID, BUSY_POLL_INTERVAL_MS, None) } == 0 {
        return Err("SetTimer（忙碌輪詢）失敗".to_string());
    }

    // task 5.5：工作階段鎖定／解鎖通知（design.md D12）。fix F2（review 5.5 medium）：失敗不再
    // 提早 return（否則下面的電源通知、Win+D 掛鉤、啟動查詢全部不裝）——記錄後繼續，並以
    // [`SESSION_NOTIFY_RETRY_TIMER_ID`] 計時器重試到成功或達上限。
    try_register_session_notification(hwnd, 1);

    // task 5.5：顯示器、電源來源、省電模式通知（design.md D12）。單一 GUID 註冊失敗只記錄、
    // 不中止整個 spawn_gatekeeper——其餘暫停來源（鎖定、忙碌輪詢）仍應正常運作。
    let recipient = HANDLE::from(hwnd);
    for guid in [
        &GUID_CONSOLE_DISPLAY_STATE,
        &GUID_ACDC_POWER_SOURCE,
        &GUID_POWER_SAVING_STATUS,
    ] {
        // SAFETY: `recipient` 是本行程剛建立、仍存活的頂層視窗的 HANDLE（互動式應用程式，
        // `Flags=DEVICE_NOTIFY_WINDOW_HANDLE`）；不主動 UnregisterPowerSettingNotification
        // （同上，存活到行程結束）。
        if let Err(err) = unsafe {
            RegisterPowerSettingNotification(recipient, guid, DEVICE_NOTIFY_WINDOW_HANDLE)
        } {
            gatekeeper_log(&format!(
                "RegisterPowerSettingNotification({guid:?}) 失敗：{err}"
            ));
        }
    }

    // task 3.4：Win+D 偵測。與守門視窗同一個（主）執行緒安裝 out-of-context 掛鉤，回呼由這個
    // 執行緒的訊息迴圈派送，與上面的 WM_TIMER／TaskbarCreated 處理不會互相搶跑。啟動當下先算
    // 一次（例如開機自啟時所有視窗都還是最小化＝已經是「顯示桌面」）。
    if let Err(err) = install_show_desktop_hooks() {
        gatekeeper_log(&format!("SHOWDESKTOP hooks-failed err={err}"));
        return Err(err);
    }
    gatekeeper_log("SHOWDESKTOP hooks-installed");
    if let Some(state) = GATEKEEPER.get() {
        evaluate_show_desktop(state, "startup");

        // task 5.5：啟動當下主動查詢一次（task-5.5-brief：不只靠之後的通知，剛好在鎖定狀態
        // 下啟動時，暫停原因要立刻含 Locked）。查不到（`None`）維持沉默——不覆寫成錯誤的已知
        // 值，等之後的通知或下一次啟動再補。
        if let Some(locked) = query_session_locked() {
            gatekeeper_log(&format!("STARTUP locked={locked}"));
            (state.callbacks.on_pause_signal)(PauseSignal::Locked(locked));
        }
        if let Some(on_battery) = query_on_battery() {
            gatekeeper_log(&format!("STARTUP on_battery={on_battery}"));
            (state.callbacks.on_pause_signal)(PauseSignal::OnBattery(on_battery));
        }
        if let Some(power_saver) = power_saver_active() {
            gatekeeper_log(&format!("STARTUP power_saver={power_saver}"));
            (state.callbacks.on_pause_signal)(PauseSignal::PowerSaver(power_saver));
        }
        if let Some(busy) = poll_user_busy() {
            state.last_busy.store(busy, Ordering::Relaxed);
            gatekeeper_log(&format!("STARTUP busy={busy}"));
            (state.callbacks.on_pause_signal)(PauseSignal::SystemBusy(busy));
        }
    }

    Ok(())
}

/// 守門視窗的子類別處理常式（design.md D8）：處理 `TaskbarCreated` 廣播與 30 秒保險檢查
/// 計時器，其餘訊息一律交回 `DefSubclassProc`。全部在建立守門視窗的那個執行緒（tao 的訊息
/// 迴圈派送 `WM_TIMER`／廣播訊息時都在同一個訊息佇列上），單執行緒存取 `GATEKEEPER`，不需要
/// 額外同步。
///
/// # Safety
/// 由 Win32 訊息迴圈呼叫，簽章型別固定為 `SUBCLASSPROC`；`hwnd`／`msg`／`wparam`／`lparam`
/// 皆為系統依標準訊息派送機制傳入的合法值。本函式不持有任何需要手動管理生命週期的資料
/// （`GATEKEEPER` 是行程壽命的 `'static` `OnceLock`），`_uidsubclass`／`_dwrefdata` 未使用。
unsafe extern "system" fn gatekeeper_subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _uidsubclass: usize,
    _dwrefdata: usize,
) -> LRESULT {
    if let Some(state) = GATEKEEPER.get() {
        if msg == state.taskbar_created_message {
            gatekeeper_log("EVENT TaskbarCreated");
            let count = (state.callbacks.rebottom_all)();
            gatekeeper_log(&format!("REBOTTOM reason=explorer-restart count={count}"));
            // 經重排收斂點（延後到守門視窗計時器、合併同一波），重排結尾會同步格線：以退路定位的
            // 格線在殼層恢復後插回桌面正上方（`GridOverlay::update`）。
            request_relayout(RelayoutReason::ExplorerRestarted);
            (state.callbacks.on_system_event)(SystemEvent::ExplorerRestarted);
        } else if msg == windows::Win32::UI::WindowsAndMessaging::WM_TIMECHANGE {
            // dynamic-wallpaper task 4.7a：只投遞通知（同步廣播，不在這裡做事）。
            gatekeeper_log("EVENT WM_TIMECHANGE");
            (state.callbacks.on_system_event)(SystemEvent::TimeChanged);
        } else if msg == WM_TIMER && wparam.0 == SAFETY_CHECK_TIMER_ID {
            // task 3.4：先重算 Win+D 判準（design.md D8「由 30 秒保險檢查兜底」：萬一漏接
            // 事件，至多 30 秒內進入／離開），再依結果決定是否略過保險檢查。
            evaluate_show_desktop(state, "timer");
            run_safety_check(state);
        } else if msg == WM_TIMER && wparam.0 == BUSY_POLL_TIMER_ID {
            // task 5.5：design.md D12「每 5 秒輪詢」；只有變化才通知（見
            // `GatekeeperState::last_busy` 文件）。
            if let Some(busy) = poll_user_busy() {
                let previous = state.last_busy.swap(busy, Ordering::Relaxed);
                if previous != busy {
                    gatekeeper_log(&format!("EVENT quns-busy-changed busy={busy}"));
                    (state.callbacks.on_pause_signal)(PauseSignal::SystemBusy(busy));
                }
            }
        } else if msg == WM_WTSSESSION_CHANGE {
            // task 5.5：design.md D12 鎖定／解鎖；其餘 wParam（登入、登出、遠端連線等）由
            // `session_change_pause_signal` 回傳 `None`、略過。
            if let Some(locked) = session_change_pause_signal(wparam.0) {
                gatekeeper_log(&format!("EVENT WTSSESSION_CHANGE locked={locked}"));
                (state.callbacks.on_pause_signal)(PauseSignal::Locked(locked));
            }
        } else if msg == WM_POWERBROADCAST && wparam.0 as u32 == PBT_POWERSETTINGCHANGE {
            // SAFETY: 剛確認 `msg==WM_POWERBROADCAST` 且 `wParam==PBT_POWERSETTINGCHANGE`，
            // 滿足 `handle_power_setting_change` 的前置條件。
            unsafe { handle_power_setting_change(state, lparam) };
        } else if msg == WM_DISPLAYCHANGE {
            // task 3.5（design.md D9）：解析度／色彩深度改變、拔接外接螢幕都會送這則訊息給
            // 所有頂層視窗（Microsoft Learn `WM_DISPLAYCHANGE`：「This message is sent to all
            // top-level windows after the display mode has changed」），守門視窗（隱藏頂層
            // 視窗）收得到。
            // fix F2（review 3.5 low）：同步廣播，不在這裡重排，見 [`RelayoutCoalescer`]。
            gatekeeper_log("EVENT WM_DISPLAYCHANGE");
            request_relayout(RelayoutReason::DisplayChange);
        } else if msg == WM_SETTINGCHANGE && wparam.0 as u32 == SPI_SETWORKAREA.0 {
            // task 3.5（design.md D9）：工作區改變（工作列顯示/隱藏、移動、拔接螢幕造成的
            // 工作區重新計算）。`WM_SETTINGCHANGE` 由 `SystemParametersInfoW` 以
            // `SPIF_SENDCHANGE` 廣播給所有頂層視窗；`wParam` 帶的正是該次呼叫的 `uiAction`
            // （此處只關心 `SPI_SETWORKAREA`，其餘系統設定變更略過）。
            gatekeeper_log("EVENT WM_SETTINGCHANGE SPI_SETWORKAREA");
            request_relayout(RelayoutReason::WorkArea);
        } else if msg == WM_APP_RELAYOUT {
            // fix F2（review 3.5）：（重新）排定延後重排；同一 id 的 SetTimer 會重設到期時間。
            // SAFETY: `hwnd` 是收到這則訊息的守門視窗本身（本執行緒擁有）；`lpTimerFunc` 為
            // `None`，到期時以 `WM_TIMER` 送回本處理常式。
            if unsafe { SetTimer(Some(hwnd), RELAYOUT_TIMER_ID, RELAYOUT_DEBOUNCE_MS, None) } == 0 {
                // 計時器排不上：退回立即執行（仍在 posted message 裡，不在同步廣播的呼叫堆疊內）。
                gatekeeper_log("RELAYOUT SetTimer 失敗，立即執行");
                run_pending_relayout(state);
            }
        } else if msg == WM_TIMER && wparam.0 == SESSION_NOTIFY_RETRY_TIMER_ID {
            // fix F2（review 5.5 medium）：鎖定通知註冊重試。
            let attempts = state
                .session_notify_attempts
                .fetch_add(1, Ordering::Relaxed)
                + 1;
            try_register_session_notification(hwnd, attempts);
        } else if let Some(line) = end_session_record(msg, wparam.0, lparam.0) {
            // fix F5（review fix-soak low）：登出／關機／Restart Manager 關閉。系統同步送來、
            // 之後可能直接結束行程，這裡只寫一行記錄（快速、不做其他工作，見 memory「同步
            // WndProc 裡做慢工作，會卡住發送端」），回應照舊交給 DefSubclassProc。
            log::warn!("工作階段結束通知：{line}");
            log::logger().flush();
            // dynamic-wallpaper task 4.7b：通知桌布協調迴圈（回呼只投遞、立即返回；不做任何 COM
            // 呼叫，不拖延關機）。
            if let Some(event) = session_end_event(msg, wparam.0) {
                (state.callbacks.on_system_event)(event);
            }
        } else if msg == WM_TIMER && wparam.0 == RELAYOUT_TIMER_ID {
            // SAFETY: 同上；計時器由本視窗、本執行緒排定。失敗只代表計時器已不存在，無害。
            let _ = unsafe { KillTimer(Some(hwnd), RELAYOUT_TIMER_ID) };
            run_pending_relayout(state);
        }
    }
    // SAFETY: 交回系統預設處理，`hwnd`／`msg`／`wparam`／`lparam` 原樣傳遞——標準子類別處理
    // 常式寫法，與既有 `host/examples/probe_wind.rs` 的 `subclass_proc` 相同。
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

/// fix F2（review 3.5）：要求一次延後、合併的重排（[`RelayoutCoalescer`]）。可從任何執行緒
/// 呼叫：新一波的第一個請求以 `PostMessageW` 通知守門視窗，其餘只合併。回傳 `false`＝守門視窗
/// 不存在或通知失敗（請求已撤回），呼叫端自行決定是否改為立即重排。
///
/// 注意：不可改用 `AppHandle::run_on_main_thread`——在主執行緒上呼叫時 tauri 會**同步**執行，
/// 沒有延後效果（`ScaleFactorChanged` 正是在主執行緒的 run 回呼裡）。
pub fn request_relayout(reason: RelayoutReason) -> bool {
    let Some(state) = GATEKEEPER.get() else {
        return false;
    };
    let mut relayout = state
        .relayout
        .lock()
        .expect("relayout coalescer mutex poisoned");
    if !relayout.request(reason) {
        return true;
    }
    // SAFETY: `state.hwnd` 是行程存活期間不銷毀的守門視窗；`PostMessageW` 可跨執行緒呼叫，
    // 只把訊息排進該視窗執行緒的佇列，不等待處理。
    let posted = unsafe {
        PostMessageW(
            Some(hwnd_from(state.hwnd)),
            WM_APP_RELAYOUT,
            WPARAM(0),
            LPARAM(0),
        )
    };
    if let Err(err) = posted {
        relayout.take();
        drop(relayout);
        gatekeeper_log(&format!(
            "RELAYOUT 通知守門視窗失敗 reason={}：{err}",
            reason.as_str()
        ));
        return false;
    }
    true
}

/// 守門視窗的延後重排到期：取出整波原因、重排一次並記錄套用數。沒有待執行的重排就什麼都不做。
fn run_pending_relayout(state: &GatekeeperState) {
    let reasons = state
        .relayout
        .lock()
        .expect("relayout coalescer mutex poisoned")
        .take();
    if reasons.is_empty() {
        return;
    }
    let count = (state.callbacks.relayout_all)();
    let names: Vec<&str> = reasons.iter().map(|r| r.as_str()).collect();
    gatekeeper_log(&format!(
        "RELAYOUT reason={} applied={count}",
        names.join(",")
    ));
}

/// 30 秒保險檢查的一次執行：查詢目前可見小工具 → 沒有小工具就直接返回（沒什麼好保護的）→
/// 顯示桌面期間（[`SHOW_DESKTOP_ACTIVE`]）略過 → 列舉 z-order →
/// [`needs_rebottom`] 判定為真才重新置底並記錄。
fn run_safety_check(state: &GatekeeperState) {
    let visible = (state.callbacks.visible_widget_hwnds)();
    if visible.is_empty() {
        return;
    }
    if SHOW_DESKTOP_ACTIVE.load(Ordering::Relaxed) {
        return;
    }

    let snapshot = zorder_snapshot(&hwnd_set(&visible));
    if needs_rebottom(&snapshot) {
        gatekeeper_log("EVENT safety-check-triggered");
        let count = (state.callbacks.rebottom_all)();
        gatekeeper_log(&format!("REBOTTOM reason=safety-check count={count}"));
    }
}

fn hwnd_set(hwnds: &[HWND]) -> HashSet<isize> {
    hwnds.iter().map(|h| h.0 as isize).collect()
}

// ── D8：Win+D 顯示桌面（task 3.4）──────────────────────────────────────────────────
//
// 依探針 1.1 定案（design.md D8 Win+D 條、`host/examples/probe_wind.rs`、
// `.superpowers/sdd/tasks/task-1.1-report.md`）：
//
// - 判準：z-order 由上往下，略過小工具、不可見／最小化／cloaked、`WS_EX_TOPMOST` 視窗後，
//   第一個遇到的若是殼層的 `Progman`／`WorkerW`＝顯示桌面（桌面位於所有一般視窗之上）；若是
//   一般視窗＝不是。「前景視窗是桌面」**不是**判準——單純點擊桌面時前景也會變成 `Progman`，
//   但 z-order 不變（1.1 情境 C）。所有一般視窗都最小化時，往下走一定先遇到桌面，自然也判為
//   顯示桌面（design.md D8 明文要求）。
// - 偵測時機：out-of-context `SetWinEventHook`（見 [`is_relevant_win_event`]）每個相關事件
//   都重算；不另設輪詢，只有守門視窗的 30 秒計時器兜底。
// - 進入：先設 [`SHOW_DESKTOP_ACTIVE`]（小工具的 `WM_WINDOWPOSCHANGING` 子類別因此改為保留
//   原請求值，等同暫停 tao 的強制置底；**不**呼叫 `set_always_on_bottom(false)`——其
//   `HWND_NOTOPMOST` 會把小工具推到所有一般視窗之上，且會觸發 tao `apply_diff` 洗掉樣式／藏
//   視窗，見上方 task 3.1 的不變量說明），再把每個位於桌面之下的可見小工具以 `SetWindowPos`
//   插到 `GetWindow(桌面, GW_HWNDPREV)` 之後（[`insert_target`]）。
// - 重插：explorer 進入後 20～60 ms 內會再抬一次 `Progman`；顯示桌面期間只要又有小工具落在
//   桌面之下，就再插一次（1.1 實測之後 5 秒以上 z-order 無變化）。
// - 離開：任何一般視窗回到桌面之上時清旗標，並以 [`GatekeeperCallbacks::rebottom_all`]
//   （即 [`show_at_bottom`]）恢復置底。
//
// 執行緒：掛鉤回呼、守門視窗的計時器與 `TaskbarCreated`、小工具的子類別處理常式都在主執行緒
// （Tauri 的事件迴圈執行緒；小工具視窗由它擁有，`SetWindowSubclass` 也必須在擁有者執行緒
// 呼叫），沒有跨執行緒互等。

/// 小工具視窗 `WM_WINDOWPOSCHANGING` 子類別的 id（每個小工具視窗各裝一次，用固定常數即可；
/// 與守門視窗的 [`GATEKEEPER_SUBCLASS_ID`] 裝在不同視窗上，不會衝突）。
const WIDGET_SUBCLASS_ID: usize = 0x34;

/// task 5.3（design.md D8／D9「編輯版面」）：小工具拖曳結束的回呼；task 7.5 擴充為
/// [`WidgetDragHooks`] 的三個時機點。`main.rs` 的 `setup` 呼叫一次
/// [`register_widget_drag_hooks`] 登記；[`widget_subclass_proc`] 在 `WM_EXITSIZEMOVE` 時以
/// 這扇視窗自己的 `HWND` 呼叫，交給呼叫端（`widgets::finish_widget_drag`）依目前所在顯示器與
/// 矩形重算並保存位置——本模組不認得設定、小工具註冊表與 `AppHandle`。
///
/// 為什麼在這裡偵測拖曳結束，而不是讓前端在 `mouseup` 呼叫另一個指令：`startDragging()`
/// （前端用 Tauri 內建的 `data-tauri-drag-region`，見 `host/ui/widget.html`）底層是
/// `PostMessageW(WM_NCLBUTTONDOWN, HTCAPTION)` 觸發 `DefWindowProc` 內建的非用戶端拖曳迴圈
/// （tao 0.37.1 `platform_impl/windows/window.rs::handle_os_dragging`）；這個迴圈期間滑鼠已被
/// `SetCapture` 到頂層 HWND，WebView2（獨立的瀏覽器行程）收不到 `mouseup`，前端無法可靠得知
/// 拖曳何時結束。`WM_EXITSIZEMOVE` 是 Win32 對這個迴圈結束的標準通知（tao 自己也在這則訊息裡
/// 把內部的 `dragging` 旗標歸零），比「監聽 `Moved` 事件＋去抖」精確，也不會誤判「使用者拖到一半
/// 暫停手沒放開」為拖曳結束。
///
/// 與 [`GATEKEEPER`] 同樣的理由用 `OnceLock`（見其文件「task 3.3 補上」一節）：回呼只捕捉
/// `AppHandle`（`Send + Sync`），避免用 `SetWindowSubclass` 的 `dwrefdata` 手動管理裸指標
/// 生命週期（`dwrefdata` 在本模組其餘子類別化——[`install_show_desktop_subclass`]、守門視窗的
/// `SetWindowSubclass` 呼叫（[`spawn_gatekeeper`]）——也都刻意傳 0、不使用，見各自文件）。
static WIDGET_DRAG_HOOKS: OnceLock<WidgetDragHooks> = OnceLock::new();

/// 游標位置（實體像素、虛擬桌面座標）。
pub type CursorPos = (i32, i32);
/// [`WidgetDragHooks::on_enter`] 的型別。
pub type DragEnterHook = dyn Fn(HWND, Option<CursorPos>) + Send + Sync + 'static;
/// [`WidgetDragHooks::on_moving`] 的型別。
pub type DragMovingHook =
    dyn Fn(HWND, PhysicalRect, Option<CursorPos>) -> Option<PhysicalRect> + Send + Sync + 'static;
/// [`WidgetDragHooks::on_exit`] 的型別。
pub type DragExitHook = dyn Fn(HWND) -> usize + Send + Sync + 'static;
/// [`WidgetDragHooks::on_settle`] 的型別。
pub type DragSettleHook = dyn Fn(HWND, usize) + Send + Sync + 'static;

/// 小工具拖曳（系統移動迴圈）各時機點的回呼（task 7.5；design.md D7「編輯版面」）。全部都在
/// 擁有該視窗的主執行緒、於子類別處理常式內被同步呼叫：呼叫端只能做輕量的純計算與非同步投遞
/// （例如 `emit`），不得在這裡同步建立視窗或等待其他執行緒（memory：
/// sync-wndproc-slow-work-blocks-sender、win32-window-apis-have-thread-affinity）。
pub struct WidgetDragHooks {
    /// `WM_ENTERSIZEMOVE`：移動迴圈開始（`DefSubclassProc` 之後呼叫）。第二個參數＝此刻的
    /// 游標位置（[`cursor_pos`]；讀不到為 `None`），fix drag-dpi 用來記下抓點。
    pub on_enter: Box<DragEnterHook>,
    /// `WM_MOVING`：提議矩形（實體像素、螢幕座標；本行程為 Per-Monitor-V2，視窗座標即實體
    /// 像素）與此刻的游標位置。不做格線吸附。fix drag-dpi：回傳 `Some(矩形)` 時本模組把提議
    /// 矩形改寫成它，尺寸與視窗目前不同時另以 `SetWindowPos` 套用（見 [`widget_subclass_proc`]
    /// 的 `WM_MOVING` 分支）；`None`＝不介入。
    pub on_moving: Box<DragMovingHook>,
    /// `WM_SIZING`（task 7.6）：調整大小迴圈中的被拖邊（`wParam` 的 `WMSZ_*`，見
    /// [`resize_edges_from_wmsz`]）與提議矩形。唯讀——不改寫提議矩形，
    /// 對齊只在放開時做。
    pub on_sizing: Box<dyn Fn(HWND, ResizeEdges, PhysicalRect) + Send + Sync + 'static>,
    /// `WM_EXITSIZEMOVE`：移動／調整大小迴圈結束（`DefSubclassProc` 之後呼叫，tao 已歸零
    /// `dragging`）。fix F6b：呼叫端在這裡**不判定**放開位置，只回傳序號；本模組把它當
    /// `wParam` 以 `PostMessageW(WM_APP_DRAG_SETTLE)` 送回同一扇視窗，處理時呼叫
    /// [`Self::on_settle`]。投遞失敗時立即呼叫 `on_settle`。
    pub on_exit: Box<DragExitHook>,
    /// fix F6b：`WM_APP_DRAG_SETTLE` 到達（系統的移動迴圈與放開後的定位都已完成）：以此刻的
    /// 視窗矩形判定放開位置。第二個參數＝`on_exit` 回傳的序號。
    pub on_settle: Box<DragSettleHook>,
    /// fix F6b：小工具視窗 `WM_NCDESTROY`：丟棄它的進行中拖曳與待判定（posted message 不會送到
    /// 已銷毀的視窗）。
    pub on_destroyed: Box<dyn Fn(HWND) + Send + Sync + 'static>,
}

/// fix F6b：小工具視窗「拖曳結束、延後判定」的自訂訊息（`wParam`＝[`WidgetDragHooks::on_exit`]
/// 回傳的序號）。只投遞給本行程自己的小工具視窗，由 [`widget_subclass_proc`] 攔下、不轉交
/// （tao 0.37.1／wry 0.57.0 不使用 `WM_APP` 範圍；與守門視窗的 [`WM_APP_RELAYOUT`] 不同值）。
const WM_APP_DRAG_SETTLE: u32 = WM_APP + 0x36;

/// 登記 [`WIDGET_DRAG_HOOKS`]；`main.rs` 的 `setup` 只呼叫一次。重複呼叫只有第一次生效
/// （`OnceLock::set` 對之後的呼叫回傳 `Err`，這裡靜默忽略——沒有任何呼叫端需要知道「登記晚了」，
/// 唯一呼叫端本身就只呼叫一次）。
pub fn register_widget_drag_hooks(hooks: WidgetDragHooks) {
    let _ = WIDGET_DRAG_HOOKS.set(hooks);
}

/// 目前的游標位置（實體像素、虛擬桌面座標；本行程為 Per-Monitor-V2）。讀不到回傳 `None`。
fn cursor_pos() -> Option<(i32, i32)> {
    let mut p = POINT::default();
    // SAFETY: `p` 是局部變數，`GetCursorPos` 只寫入這個結構。
    unsafe { GetCursorPos(&mut p) }.ok().map(|()| (p.x, p.y))
}

thread_local! {
    /// fix F6（review dragdpi medium）：[`apply_drag_rect`] 正在套用的矩形（重入旗標）。
    /// 只有它不是 `None` 時，[`widget_subclass_proc`] 才回答 `WM_GETDPISCALEDSIZE`、改寫
    /// `WM_DPICHANGED` 的建議矩形——也就是只介入「宿主自己改尺寸」同步觸發的 DPI 變更。
    /// 系統自行觸發的 DPI 變更（例如使用者改縮放比例、系統移動迴圈自己跨過 DPI 邊界）
    /// 此時不在套用中，照系統預設處理，不會被改寫成游標處。小工具視窗都由主執行緒
    /// 擁有，訊息都在主執行緒派送，thread-local 即可。
    static APPLYING_DRAG_RECT: std::cell::Cell<Option<PhysicalRect>> =
        const { std::cell::Cell::new(None) };
}

/// 目前 [`apply_drag_rect`] 正在套用的矩形；沒有在套用時為 `None`。
fn applying_drag_rect() -> Option<PhysicalRect> {
    APPLYING_DRAG_RECT.with(std::cell::Cell::get)
}

/// 在 `f` 執行期間把 [`APPLYING_DRAG_RECT`] 設成 `rect`，結束（含 panic 展開）後還原成進入
/// 前的值（巢狀呼叫時還原外層）。
fn with_applying_drag_rect<R>(rect: PhysicalRect, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<PhysicalRect>);
    impl Drop for Restore {
        fn drop(&mut self) {
            APPLYING_DRAG_RECT.with(|cell| cell.set(self.0));
        }
    }
    let _restore = Restore(APPLYING_DRAG_RECT.with(|cell| cell.replace(Some(rect))));
    f()
}

/// fix drag-dpi：拖曳中把視窗設成 `rect`（尺寸與目前不同時才呼叫 `SetWindowPos`，不動
/// z-order、不啟用）。在 `WM_MOVING`／`WM_DPICHANGED` 內同步呼叫；跨 DPI 時會同步觸發
/// `WM_GETDPISCALEDSIZE`／`WM_DPICHANGED`，由同一個子類別以同一個矩形回答（fix F6：經
/// [`APPLYING_DRAG_RECT`]，只有這段期間會回答），不會遞迴改尺寸。
fn apply_drag_rect(hwnd: HWND, rect: PhysicalRect) {
    let Ok(current) = window_rect(hwnd) else {
        return;
    };
    if current == rect {
        return;
    }
    // SAFETY: hwnd 是本行程仍存活的小工具頂層視窗（正在處理它的訊息）；SWP_NOZORDER／
    // SWP_NOACTIVATE 保證不動 z-order、不啟用。
    let applied = with_applying_drag_rect(rect, || unsafe {
        SetWindowPos(
            hwnd,
            None,
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            SWP_NOZORDER | SWP_NOACTIVATE,
        )
    });
    if let Err(err) = applied {
        log::warn!("拖曳中套用目標尺寸失敗：{err}");
    }
}

/// [`PhysicalRect`] → Win32 `RECT`（[`physical_rect_from_ltrb`] 的反向）。純函式。
fn rect_from_physical(r: PhysicalRect) -> RECT {
    RECT {
        left: r.x,
        top: r.y,
        right: r.x.saturating_add(r.width),
        bottom: r.y.saturating_add(r.height),
    }
}

/// `WM_MOVING` 的 `RECT`（left/top/right/bottom）→ [`PhysicalRect`]。寬高為負（不合法輸入）時
/// 夾成 0。純函式。
pub fn physical_rect_from_ltrb(left: i32, top: i32, right: i32, bottom: i32) -> PhysicalRect {
    PhysicalRect {
        x: left,
        y: top,
        width: right.saturating_sub(left).max(0),
        height: bottom.saturating_sub(top).max(0),
    }
}

/// [`classify_top`] 的結果：z-order 頂端第一個「有意義」的視窗是什麼（索引指向快照）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZOrderTop {
    /// 殼層的 `Progman`／`WorkerW` 位於所有一般視窗之上＝顯示桌面。
    Desktop(usize),
    /// 一般應用程式視窗位於桌面之上。
    App(usize),
    /// 兩者都沒遇到（理論上不會：殼層在跑時桌面視窗一定在；explorer 重啟中可能發生）。
    Nothing,
}

/// [`decide_show_desktop`] 的結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShowDesktopAction {
    /// 狀態不變、不需要任何動作。
    None,
    /// 進入顯示桌面：設旗標，把 `insert` 裡的小工具（位於桌面之下的可見小工具，可能為空——
    /// 例如全部視窗最小化時小工具本來就在桌面正上方）插到桌面之上。
    Enter { desk: usize, insert: Vec<isize> },
    /// 已在顯示桌面中，但又有小工具落到桌面之下（explorer 再抬一次 `Progman`、或
    /// `TaskbarCreated`／重新顯示把它置底了）：再插一次。
    Reassert { desk: usize, insert: Vec<isize> },
    /// 一般視窗回到桌面之上：清旗標、全部恢復置底。
    Exit { app: usize },
}

/// 插入位置（[`insert_target`] 的結果）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertTarget {
    /// 桌面視窗正上方已經是這個小工具自己，不必動。
    AlreadyInPlace,
    /// `hwndInsertAfter`＝桌面視窗的前一個視窗（插在它之後＝插在桌面正上方）。
    After(isize),
    /// 前一個視窗是 topmost 或不存在：用 `HWND_TOP`（非 topmost 頂端＝桌面正上方，因為此時
    /// 桌面就是非 topmost 視窗中的第一個）。
    Top,
}

/// 殼層桌面視窗判準：類別為 `Progman` 或 `WorkerW`，且擁有者行程是殼層
/// （`GetShellWindow` 的 PID）。`shell_pid` 為 0（殼層沒在跑）時一律不是——其他行程也可能
/// 註冊同名類別（例如桌布程式自己開的 `WorkerW`），不能只看類別名稱。
pub fn is_shell_desktop_window(class: &str, pid: u32, shell_pid: u32) -> bool {
    shell_pid != 0 && pid == shell_pid && (class == "Progman" || class == "WorkerW")
}

/// 畫面上看得到（可見、非最小化、非 cloaked）。
fn is_shown(w: &ZOrderWindowState) -> bool {
    w.visible && !w.minimized && !w.cloaked
}

/// Win+D 判準的純函式（design.md D8；探針 1.1 `scan_top` 的同一套規則）：`snapshot` 由上到下，
/// 略過小工具、topmost、看不到的視窗；第一個遇到的若是殼層桌面視窗（`Progman` 帶
/// `WS_EX_TOOLWINDOW`，所以要先於「一般視窗」判準檢查）回傳 [`ZOrderTop::Desktop`]，若是
/// 一般視窗（[`is_normal_window`]：非 tool window、非零尺寸）回傳 [`ZOrderTop::App`]；其餘
/// （例如看得到的 tool window、零尺寸視窗）略過繼續往下。
pub fn classify_top(snapshot: &[ZOrderWindowState]) -> ZOrderTop {
    for (i, w) in snapshot.iter().enumerate() {
        if w.is_widget || w.topmost || !is_shown(w) {
            continue;
        }
        if w.shell_desktop {
            return ZOrderTop::Desktop(i);
        }
        if is_normal_window(w) {
            return ZOrderTop::App(i);
        }
    }
    ZOrderTop::Nothing
}

/// 位於 `desk` 之下（快照索引較大）的可見小工具 hwnd，由上到下。
fn widgets_below(snapshot: &[ZOrderWindowState], desk: usize) -> Vec<isize> {
    snapshot
        .iter()
        .skip(desk + 1)
        .filter(|w| w.is_widget && w.visible)
        .map(|w| w.hwnd)
        .collect()
}

/// 由目前狀態（`active`＝[`SHOW_DESKTOP_ACTIVE`]）與一次 z-order 快照決定要做什麼。純函式。
///
/// - 頂端是桌面：未進入→[`ShowDesktopAction::Enter`]；已進入且有小工具落在桌面之下→
///   [`ShowDesktopAction::Reassert`]；否則不動（已經穩定在桌面之上，**不**重複插入——
///   避免自己觸發的 `EVENT_OBJECT_REORDER` 又引發下一次插入，形成來回切換）。
/// - 頂端是一般視窗：已進入→[`ShowDesktopAction::Exit`]；否則不動（平常狀態；小工具是否
///   在一般視窗之下由 30 秒保險檢查負責，不歸這裡）。
/// - 都沒有：不動（explorer 重啟中等過渡狀態，不據此進出）。
pub fn decide_show_desktop(active: bool, snapshot: &[ZOrderWindowState]) -> ShowDesktopAction {
    match classify_top(snapshot) {
        ZOrderTop::Desktop(desk) => {
            let insert = widgets_below(snapshot, desk);
            if !active {
                ShowDesktopAction::Enter { desk, insert }
            } else if !insert.is_empty() {
                ShowDesktopAction::Reassert { desk, insert }
            } else {
                ShowDesktopAction::None
            }
        }
        ZOrderTop::App(app) if active => ShowDesktopAction::Exit { app },
        ZOrderTop::App(_) | ZOrderTop::Nothing => ShowDesktopAction::None,
    }
}

/// 插入位置純函式（design.md D8：「`hwndInsertAfter` 設為 `GetWindow(桌面視窗,
/// GW_HWNDPREV)`；前一個視窗是 topmost 或不存在時改用 `HWND_TOP`」）。`prev`＝桌面視窗的
/// 前一個視窗 `(hwnd, 是否 topmost)`，`None`＝桌面已是最上層。前一個就是 `own` 自己時不必動
/// （探針 1.1：「ENTER 當下若前一個視窗已經是小工具自己，就直接略過」）。前一個是**另一個**
/// 小工具時照常插在它之後——等於插到它與桌面之間，同樣在桌面之上；小工具彼此的順序不重要。
pub fn insert_target(own: isize, prev: Option<(isize, bool)>) -> InsertTarget {
    match prev {
        Some((hwnd, _)) if hwnd == own => InsertTarget::AlreadyInPlace,
        Some((hwnd, false)) => InsertTarget::After(hwnd),
        Some((_, true)) | None => InsertTarget::Top,
    }
}

/// 哪些 WinEvent 需要重算 Win+D 判準（design.md D8 定案的事件清單）。純函式；呼叫端先查好
/// 兩個 Win32 事實再傳入：
/// - `from_desktop_window`：事件來源是桌面視窗（`GetDesktopWindow`，類別 `#32769`）。頂層
///   視窗的 z-order 變化以它為容器送出 `EVENT_OBJECT_REORDER`（探針 1.1 的觸發來源全部是
///   `REORDER:#32769`）；其他容器的 REORDER（視窗內子控制項重排）與判準無關。
/// - `top_level`：事件的 hwnd 是頂層視窗（`GetAncestor(GA_ROOT)` 是自己）。
///
/// `EVENT_SYSTEM_FOREGROUND`、`EVENT_SYSTEM_MINIMIZESTART`／`END` 一律重算；
/// `EVENT_OBJECT_SHOW`／`HIDE`／`CLOAKED`／`UNCLOAKED` 只看頂層視窗本身
/// （`OBJID_WINDOW`＋`CHILDID_SELF`）——這三組對子控制項、游標、插入點也會觸發，過濾掉可以
/// 省下大量無謂的 z-order 列舉。
pub fn is_relevant_win_event(
    event: u32,
    id_object: i32,
    id_child: i32,
    from_desktop_window: bool,
    top_level: bool,
) -> bool {
    match event {
        EVENT_OBJECT_REORDER => from_desktop_window,
        EVENT_SYSTEM_FOREGROUND | EVENT_SYSTEM_MINIMIZESTART | EVENT_SYSTEM_MINIMIZEEND => true,
        EVENT_OBJECT_SHOW | EVENT_OBJECT_HIDE | EVENT_OBJECT_CLOAKED | EVENT_OBJECT_UNCLOAKED => {
            id_object == OBJID_WINDOW.0 && id_child == CHILDID_SELF as i32 && top_level
        }
        _ => false,
    }
}

fn win_event_name(event: u32) -> &'static str {
    match event {
        EVENT_SYSTEM_FOREGROUND => "FOREGROUND",
        EVENT_SYSTEM_MINIMIZESTART => "MINIMIZESTART",
        EVENT_SYSTEM_MINIMIZEEND => "MINIMIZEEND",
        EVENT_OBJECT_SHOW => "SHOW",
        EVENT_OBJECT_HIDE => "HIDE",
        EVENT_OBJECT_REORDER => "REORDER",
        EVENT_OBJECT_CLOAKED => "CLOAKED",
        EVENT_OBJECT_UNCLOAKED => "UNCLOAKED",
        _ => "OTHER",
    }
}

/// 對小工具視窗安裝 `WM_WINDOWPOSCHANGING` 子類別（[`widget_subclass_proc`]）。必須在擁有
/// 該視窗的執行緒（主執行緒）呼叫——`SetWindowSubclass` 不能跨執行緒；目前唯一呼叫端
/// [`present_widget`] 在 `widgets::create_widget_window` 裡，兩條路徑（`setup`、同步的
/// `update_settings` 指令）都在主執行緒。重複安裝同一 id 只會更新參考資料，無副作用。
pub fn install_show_desktop_subclass(hwnd: HWND) -> Result<(), String> {
    // SAFETY: `hwnd` 是本行程仍存活的小工具頂層視窗；`widget_subclass_proc` 是 `'static`
    // 函式、只讀取 `'static` 的 `SHOW_DESKTOP_ACTIVE`，不使用 `dwrefdata`（傳 0）；視窗銷毀時
    // 由處理常式自己在 `WM_NCDESTROY` 移除子類別。
    let ok = unsafe { SetWindowSubclass(hwnd, Some(widget_subclass_proc), WIDGET_SUBCLASS_ID, 0) };
    if ok.as_bool() {
        Ok(())
    } else {
        Err("SetWindowSubclass 失敗（是否不在視窗的擁有者執行緒？）".to_string())
    }
}

/// 小工具視窗的子類別處理常式（design.md D8「暫停置底：子類別化 `WM_WINDOWPOSCHANGING`，
/// 在 tao 改寫成 `HWND_BOTTOM` 之後，把 `hwndInsertAfter` 還原為原本的請求值」）。
///
/// tao 0.37.1（`platform_impl/windows/event_loop.rs` 的 `WM_WINDOWPOSCHANGING` 分支）對帶
/// `ALWAYS_ON_BOTTOM` 旗標的視窗一律把 `hwndInsertAfter` 改成 `HWND_BOTTOM`。本處理常式比
/// tao 晚安裝＝先被呼叫：先記下原請求、交給 `DefSubclassProc`（tao 在裡面改寫），回來後若
/// 處於顯示桌面且這次確實要改 z-order（無 `SWP_NOZORDER`），就把原請求寫回。不在顯示桌面時
/// 完全不介入，置底行為與 3.1–3.3 相同。
///
/// # Safety
/// 由 Win32 訊息迴圈呼叫，簽章型別固定為 `SUBCLASSPROC`。`WM_WINDOWPOSCHANGING` 的 `lparam`
/// 依文件保證指向有效的 `WINDOWPOS`（仍做空指標防禦）；其餘訊息原樣轉交。
unsafe extern "system" fn widget_subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _uidsubclass: usize,
    _dwrefdata: usize,
) -> LRESULT {
    if msg == WM_NCDESTROY {
        // fix F6b：丟棄這扇視窗的拖曳狀態（延後判定的訊息不會送到已銷毀的視窗）。
        if let Some(hooks) = WIDGET_DRAG_HOOKS.get() {
            (hooks.on_destroyed)(hwnd);
        }
        // SAFETY: Microsoft Learn「Subclassing Controls」：子類別須在視窗銷毀前移除，
        // `WM_NCDESTROY` 是最後一則訊息；參數與安裝時相同。
        let _ =
            unsafe { RemoveWindowSubclass(hwnd, Some(widget_subclass_proc), WIDGET_SUBCLASS_ID) };
        // SAFETY: 轉交下一層處理常式（tao 的子類別／原視窗程序）。
        return unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
    }
    if msg == WM_ENTERSIZEMOVE {
        // task 7.5：移動迴圈開始（`startDragging()` 觸發）。先轉交下一層再通知。
        // SAFETY: 原樣轉交下一層處理常式。
        let result = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
        if let Some(hooks) = WIDGET_DRAG_HOOKS.get() {
            (hooks.on_enter)(hwnd, cursor_pos());
        }
        return result;
    }
    if msg == WM_MOVING {
        // task 7.5：拖曳中的提議矩形（Microsoft Learn「WM_MOVING message」：`lParam` 指向
        // 視窗目前位置的 `RECT`，螢幕座標；「To change the position of the drag rectangle,
        // an application must change the members of this structure」）。先轉交下一層（若有人
        // 調整提議矩形，讀到的是調整後的值），再讀出交給回呼。
        //
        // fix drag-dpi：回呼回傳目標矩形（目標顯示器上保留格數的實際大小、游標維持同一比例
        // 位置）時改寫提議矩形。文件只承諾採用改寫後的**位置**，所以尺寸不同時另以
        // `SetWindowPos` 套用（[`apply_drag_rect`]）；之後系統移動迴圈的 `SetWindowPos`
        // 只動位置（與我們寫回的位置相同），尺寸維持。跨 DPI 時套用會同步觸發
        // `WM_GETDPISCALEDSIZE`／`WM_DPICHANGED`，下面兩個分支以同一個矩形回答。
        // SAFETY: 原樣轉交下一層處理常式。
        let result = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
        let rect = lparam.0 as *mut RECT;
        if !rect.is_null() {
            if let Some(hooks) = WIDGET_DRAG_HOOKS.get() {
                // SAFETY: WM_MOVING 的 lParam 依文件指向有效 RECT（已做空指標防禦），在本次
                // 訊息處理期間可讀寫（文件明定應用程式以改寫它來調整拖曳矩形）。
                let r = unsafe { *rect };
                let proposed = physical_rect_from_ltrb(r.left, r.top, r.right, r.bottom);
                if let Some(desired) = (hooks.on_moving)(hwnd, proposed, cursor_pos()) {
                    if desired != proposed {
                        // SAFETY: 同上。
                        unsafe { *rect = rect_from_physical(desired) };
                        apply_drag_rect(hwnd, desired);
                        return LRESULT(1);
                    }
                }
            }
        }
        return result;
    }
    if msg == WM_GETDPISCALEDSIZE {
        // fix drag-dpi：Microsoft Learn「WM_GETDPISCALEDSIZE message」：Per-Monitor-V2 頂層
        // 視窗在 `WM_DPICHANGED` 之前收到；`lParam` 是 in/out 的 `SIZE*`，回 TRUE 表示已算好
        // 新尺寸，它會成為 `WM_DPICHANGED` 的候選矩形大小；回 FALSE 則套用預設的線性換算。
        // tao 0.37.1 不處理這則訊息（`event_loop.rs` 無此分支，落到 DefWindowProc 回 0）。
        // fix F6：只在宿主自己的 [`apply_drag_rect`] 進行中回答它正在套用的大小（見
        // [`APPLYING_DRAG_RECT`]）；系統自行觸發的 DPI 變更（改縮放比例等）照原樣轉交。
        let size = lparam.0 as *mut SIZE;
        if let (false, Some(desired)) = (size.is_null(), applying_drag_rect()) {
            // SAFETY: 依文件 lParam 指向有效 SIZE（已做空指標防禦），本次訊息處理期間可寫。
            unsafe {
                *size = SIZE {
                    cx: desired.width,
                    cy: desired.height,
                }
            };
            return LRESULT(1);
        }
        // SAFETY: 原樣轉交下一層處理常式。
        return unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
    }
    if msg == WM_DPICHANGED {
        // fix drag-dpi：tao 0.37.1（`event_loop.rs` 的 `WM_DPICHANGED` 分支）先同步送出
        // `ScaleFactorChanged`，回呼返回後在 Windows 11（build ≥ 22000）直接
        // `SetWindowPos(lParam 的建議矩形)`；Windows 10 則以自己算的「邏輯尺寸不變」大小擺放。
        // 本子類別比 tao 先被呼叫：拖曳中先把建議矩形改寫成宿主要的矩形再轉交（Windows 11
        // 下 tao 直接採用），轉交回來後若矩形仍不同（Windows 10 路徑）再自行套用一次。
        // fix F6：只在宿主自己的 [`apply_drag_rect`] 進行中介入（[`APPLYING_DRAG_RECT`]）。
        // 其餘 DPI 變更——使用者改縮放比例、系統移動迴圈自己跨過
        // DPI 邊界——完全不介入；最後一種在下一則 `WM_MOVING` 由宿主改回目標大小。
        let suggested = lparam.0 as *mut RECT;
        let desired = if suggested.is_null() {
            None
        } else {
            applying_drag_rect()
        };
        if let Some(desired) = desired {
            // SAFETY: 依文件 lParam 指向有效 RECT（已做空指標防禦），本次訊息處理期間可寫；
            // tao 在轉交後讀取它。
            unsafe { *suggested = rect_from_physical(desired) };
        }
        // SAFETY: 轉交下一層處理常式（tao 在這裡更新縮放、送 ScaleFactorChanged、套用矩形）。
        let result = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
        if let Some(desired) = desired {
            apply_drag_rect(hwnd, desired);
        }
        return result;
    }
    if msg == WM_SIZING {
        // task 7.6：調整大小迴圈中的提議矩形（Microsoft Learn「WM_SIZING message」：`wParam`
        // 是被拖邊 `WMSZ_*`，`lParam` 指向螢幕座標的 `RECT`）。同 WM_MOVING：先轉交、唯讀讀出、
        // 不改寫。
        // SAFETY: 原樣轉交下一層處理常式。
        let result = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
        let rect = lparam.0 as *const RECT;
        let edges = u32::try_from(wparam.0)
            .ok()
            .and_then(resize_edges_from_wmsz);
        if let (false, Some(edges)) = (rect.is_null(), edges) {
            if let Some(hooks) = WIDGET_DRAG_HOOKS.get() {
                // SAFETY: WM_SIZING 的 lParam 依文件指向有效 RECT（已做空指標防禦），在本次
                // 訊息處理期間可讀；只讀取、不寫入。
                let r = unsafe { *rect };
                (hooks.on_sizing)(
                    hwnd,
                    edges,
                    physical_rect_from_ltrb(r.left, r.top, r.right, r.bottom),
                );
            }
        }
        return result;
    }
    if msg == WM_EXITSIZEMOVE {
        // task 5.3：拖曳結束；task 7.6 起也可能是編輯版面調整大小結束（`WS_SIZEBOX` 只在
        // 編輯版面期間存在，見 [`set_widget_resizable`]）。
        // SAFETY: 先轉交下一層（tao 在這裡把內部 `dragging` 旗標歸零、必要時補送
        // `WM_LBUTTONUP`），確保 tao 自身狀態一致後才讀取最終矩形。
        let result = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
        if let Some(hooks) = WIDGET_DRAG_HOOKS.get() {
            // fix F6b：不在這裡判定（這則訊息不保證視窗已在最終位置，例如關閉「拖曳時顯示視窗
            // 內容」時系統在它之後才定位視窗），延後到 posted message：它要等系統的移動迴圈結束、
            // 回到訊息迴圈才派送，屆時視窗已在實際最終位置（design.md D7「取消」）。
            let seq = (hooks.on_exit)(hwnd);
            // SAFETY: `hwnd` 是收到這則訊息、仍存活的本行程視窗；`PostMessageW` 只把訊息排進
            // 佇列、不等待處理。
            let posted =
                unsafe { PostMessageW(Some(hwnd), WM_APP_DRAG_SETTLE, WPARAM(seq), LPARAM(0)) };
            if let Err(err) = posted {
                log::warn!("拖曳結束：延後判定訊息投遞失敗（{err}），立即判定");
                (hooks.on_settle)(hwnd, seq);
            }
        }
        return result;
    }
    if msg == WM_APP_DRAG_SETTLE {
        // fix F6b：延後判定（見 WM_EXITSIZEMOVE 分支）。自訂訊息，不轉交下一層。
        if let Some(hooks) = WIDGET_DRAG_HOOKS.get() {
            (hooks.on_settle)(hwnd, wparam.0);
        }
        return LRESULT(0);
    }
    let wp = lparam.0 as *mut WINDOWPOS;
    if msg != WM_WINDOWPOSCHANGING || wp.is_null() {
        // SAFETY: 同上，原樣轉交。
        return unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
    }
    // SAFETY: 上面已確認是 WM_WINDOWPOSCHANGING 且指標非空；文件保證指向有效 WINDOWPOS，
    // 在本次訊息處理期間可讀寫。
    let (requested, flags) = unsafe { ((*wp).hwndInsertAfter, (*wp).flags) };
    // SAFETY: 同上，原樣轉交（tao 在這裡把 hwndInsertAfter 改寫成 HWND_BOTTOM）。
    let result = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
    if flags.0 & SWP_NOZORDER.0 == 0 && SHOW_DESKTOP_ACTIVE.load(Ordering::Relaxed) {
        // SAFETY: 同上；只寫回原本的請求值。
        unsafe { (*wp).hwndInsertAfter = requested };
    }
    result
}

/// 以 out-of-context `SetWinEventHook` 監聽 design.md D8 定案的事件（範圍與探針 1.1 相同）。
/// 必須在有訊息迴圈的主執行緒呼叫（回呼由呼叫執行緒的訊息迴圈派送）。掛鉤存活到行程結束，
/// 不保存 handle、不解除。
fn install_show_desktop_hooks() -> Result<(), String> {
    let ranges = [
        (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
        (EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND),
        // SHOW(0x8002)、HIDE(0x8003)、REORDER(0x8004) 連號，一個範圍涵蓋。
        (EVENT_OBJECT_SHOW, EVENT_OBJECT_REORDER),
        (EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED),
    ];
    for (lo, hi) in ranges {
        // SAFETY: out-of-context 掛鉤（`hmodwineventproc` 為 None、`WINEVENT_OUTOFCONTEXT`），
        // 監聽所有行程／執行緒（0, 0）；回呼 `show_desktop_win_event` 是 `'static` 函式。
        let hook = unsafe {
            SetWinEventHook(
                lo,
                hi,
                None,
                Some(show_desktop_win_event),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            )
        };
        if hook.is_invalid() {
            return Err(format!("SetWinEventHook({lo:#x}..{hi:#x}) 失敗"));
        }
    }
    Ok(())
}

/// WinEvent 回呼：過濾（[`is_relevant_win_event`]）後重算判準。
///
/// # Safety
/// 由系統以 `WINEVENTPROC` 簽章在主執行緒的訊息迴圈中呼叫；`hwnd` 可能已失效，以下 API 對
/// 失效 hwnd 皆只回傳失敗值、不會存取無效記憶體。
unsafe extern "system" fn show_desktop_win_event(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _event_thread: u32,
    _event_time: u32,
) {
    // SAFETY: 純查詢。
    let from_desktop_window = !hwnd.is_invalid() && hwnd == unsafe { GetDesktopWindow() };
    // SAFETY: 純查詢；hwnd 失效時回傳空 HWND，比較結果為 false。
    let top_level = !hwnd.is_invalid() && unsafe { GetAncestor(hwnd, GA_ROOT) } == hwnd;
    if !is_relevant_win_event(event, id_object, id_child, from_desktop_window, top_level) {
        return;
    }
    if let Some(state) = GATEKEEPER.get() {
        evaluate_show_desktop(state, &format!("event:{}", win_event_name(event)));
    }
}

/// 重算判準並執行 [`decide_show_desktop`] 決定的動作，進入／重插／離開各寫一行記錄：
/// `SHOWDESKTOP ENTER|REASSERT|EXIT source=<來源> …`（供 `host/tools/verify-3.4.ps1` 判讀）。
fn evaluate_show_desktop(state: &GatekeeperState, source: &str) {
    let visible = (state.callbacks.visible_widget_hwnds)();
    let snapshot = zorder_snapshot(&hwnd_set(&visible));
    let active = SHOW_DESKTOP_ACTIVE.load(Ordering::Relaxed);
    match decide_show_desktop(active, &snapshot) {
        ShowDesktopAction::None => {}
        ShowDesktopAction::Enter { desk, insert } => {
            // 先設旗標再插入：插入時 tao 的改寫要被子類別還原。
            SHOW_DESKTOP_ACTIVE.store(true, Ordering::Relaxed);
            let desk = hwnd_from(snapshot[desk].hwnd);
            let moved = insert_widgets_above_desktop(desk, &insert);
            gatekeeper_log(&format!(
                "SHOWDESKTOP ENTER source={source} desk={} below={} inserted={moved}",
                describe_window(desk),
                insert.len()
            ));
        }
        ShowDesktopAction::Reassert { desk, insert } => {
            let desk = hwnd_from(snapshot[desk].hwnd);
            let moved = insert_widgets_above_desktop(desk, &insert);
            gatekeeper_log(&format!(
                "SHOWDESKTOP REASSERT source={source} desk={} below={} inserted={moved}",
                describe_window(desk),
                insert.len()
            ));
        }
        ShowDesktopAction::Exit { app } => {
            // 先清旗標再置底：置底的 HWND_BOTTOM 請求本來就不會被還原，這裡只是讓之後 tao 的
            // 強制置底立即恢復作用。
            SHOW_DESKTOP_ACTIVE.store(false, Ordering::Relaxed);
            let count = (state.callbacks.rebottom_all)();
            gatekeeper_log(&format!(
                "SHOWDESKTOP EXIT source={source} top-app={} rebottom={count}",
                describe_window(hwnd_from(snapshot[app].hwnd))
            ));
        }
    }
}

fn hwnd_from(value: isize) -> HWND {
    HWND(value as *mut core::ffi::c_void)
}

/// 把 `widgets` 逐一插到桌面視窗正上方（每次都重新查 `GetWindow(desk, GW_HWNDPREV)`——前
/// 一個小工具插進去後，桌面的前一個視窗就變成它）。回傳實際呼叫 `SetWindowPos` 成功的數量。
fn insert_widgets_above_desktop(desk: HWND, widgets: &[isize]) -> usize {
    let mut moved = 0;
    for &own in widgets {
        let after = match insert_target(own, desktop_prev(desk)) {
            InsertTarget::AlreadyInPlace => continue,
            InsertTarget::After(h) => hwnd_from(h),
            InsertTarget::Top => HWND_TOP,
        };
        // SAFETY: `own` 是本行程仍存活的小工具視窗（剛由回呼列出且可見）；不啟用、不移動、
        // 不改尺寸，只改 z-order。
        let result = unsafe {
            SetWindowPos(
                hwnd_from(own),
                Some(after),
                0,
                0,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
            )
        };
        match result {
            Ok(()) => moved += 1,
            Err(err) => gatekeeper_log(&format!(
                "SHOWDESKTOP insert-failed widget=0x{:X} err={err}",
                own as usize
            )),
        }
    }
    moved
}

/// 桌面視窗 `desk` 在 z-order 中的前一個視窗 `(hwnd, 是否 topmost)`，給 [`insert_target`]；
/// `None`＝桌面已是最上層（或已失效）。
fn desktop_prev(desk: HWND) -> Option<(isize, bool)> {
    // SAFETY: 純查詢；`Err` 代表桌面視窗已是最上層（或已失效）。
    unsafe { GetWindow(desk, GW_HWNDPREV) }.ok().map(|p| {
        // SAFETY: 純查詢，`p` 剛由 GetWindow 取得。
        let ex = unsafe { GetWindowLongPtrW(p, GWL_EXSTYLE) } as u32;
        (p.0 as isize, ex & WS_EX_TOPMOST.0 != 0)
    })
}

/// 把 `own` 放到殼層桌面視窗（`GetShellWindow`）正上方要用的插入位置，規則同 Win+D 的
/// [`insert_target`]（`GetWindow(桌面, GW_HWNDPREV)` 之後；前一個是 topmost 或不存在時
/// `HWND_TOP`）。取不到殼層桌面視窗（explorer 沒在跑、重啟中）回 `None`，由呼叫端決定退路。
///
/// 用途：格線疊加視窗建立時一次 `SetWindowPos(SWP_SHOWWINDOW)` 完成顯示與定位
/// （`grid_overlay::GridOverlay::create`），不先顯示在 z-order 最上層、也不靠 `HWND_BOTTOM`
/// 之後由系統或 explorer 調整。
pub fn insert_target_above_shell_desktop(own: HWND) -> Option<InsertTarget> {
    // SAFETY: 純查詢；殼層沒在跑時回空 HWND。
    let desk = unsafe { GetShellWindow() };
    if desk.is_invalid() {
        return None;
    }
    Some(insert_target(own.0 as isize, desktop_prev(desk)))
}

// ── D12：自動暫停偵測（task 5.5）────────────────────────────────────────────────────
//
// design.md D12「暫停偵測」三種機制、五個系統來源，統一經由 [`PauseSignal`]／
// [`GatekeeperCallbacks::on_pause_signal`] 通知呼叫端；desktop 模組不認得暫停原因集合或使用者
// 設定（「電池／省電依設定」屬於業務判斷，留給呼叫端——同本檔其餘 D8 回呼「本模組不認得小
// 工具註冊表」的分工原則，見 [`GatekeeperCallbacks`] 文件）：
//
// 1. `SHQueryUserNotificationState`：守門視窗的第二個計時器（[`BUSY_POLL_TIMER_ID`]）每 5 秒
//    輪詢一次（design.md D12：「每 5 秒輪詢」）。`QUNS_BUSY`／`QUNS_RUNNING_D3D_FULL_SCREEN`／
//    `QUNS_PRESENTATION_MODE` 互斥（同一次查詢只回傳其中一個值），視為同一個「忙碌」訊號
//    （[`quns_indicates_busy`]）；其餘 `QUNS_*` 值不暫停。只有值真的變化時才通知
//    （`GatekeeperState::last_busy`），避免每 5 秒都送一次沒有變化的 `pause` 事件。
// 2. `WTSRegisterSessionNotification`（工作階段鎖定／解鎖）：`WM_WTSSESSION_CHANGE`，`wParam`
//    為 `WTS_SESSION_LOCK`／`WTS_SESSION_UNLOCK` 時觸發（[`session_change_pause_signal`]），
//    其餘 `wParam`（登入、登出、遠端連線等）忽略。
// 3. `RegisterPowerSettingNotification` × 3（顯示器、電源來源、省電）：`WM_POWERBROADCAST`
//    帶 `PBT_POWERSETTINGCHANGE`，`lParam` 指向 `POWERBROADCAST_SETTING`，依 `PowerSetting`
//    這個 GUID 分派到對應的解讀純函式（[`handle_power_setting_change`]）。
//
// 啟動時主動查詢一次目前狀態（task-5.5-brief：「初始狀態要主動查詢，不只靠通知」——上述三種
// 機制都只在**之後的變化**時通知，啟動當下（例如剛好在鎖定狀態下啟動）必須另外查一次，見
// [`spawn_gatekeeper`] 內緊接在掛鉤安裝之後的一段）：鎖定用
// `WTSQuerySessionInformationW(WTSSessionInfoEx)`（[`query_session_locked`]）；電源來源用
// `GetSystemPowerStatus`（[`query_on_battery`]）；省電模式重用既有 [`power_saver_active`]（
// 同一顆 `GetSystemPowerStatus`，2.8 已有）；忙碌狀態直接呼叫一次 [`poll_user_busy`]。顯示器
// 目前開關沒有對應的一次性查詢 API——`GUID_CONSOLE_DISPLAY_STATE`／`GUID_MONITOR_POWER_ON`
// 在 Microsoft Learn「Power Setting GUIDs」都只是**通知**，沒有配對的「立即查詢目前狀態」
// 函式——顯示器關閉這一項因此只能靠之後的通知，啟動當下維持預設（未關閉，宿主啟動時螢幕多半
// 是開著的：使用者剛登入或排程剛把桌布叫醒）。
//
// 一手來源：
// - `SHQueryUserNotificationState`／`QUERY_USER_NOTIFICATION_STATE`（`QUNS_BUSY` 涵蓋一般
//   全螢幕應用程式或已開啟簡報設定，`QUNS_RUNNING_D3D_FULL_SCREEN` 專指獨占模式 Direct3D
//   全螢幕，`QUNS_PRESENTATION_MODE` 專指使用者手動開啟的簡報設定）：
//   <https://learn.microsoft.com/en-us/windows/win32/api/shellapi/ne-shellapi-query_user_notification_state>
// - `WTSRegisterSessionNotification`／`NOTIFY_FOR_THIS_SESSION`：
//   <https://learn.microsoft.com/en-us/windows/win32/api/wtsapi32/nf-wtsapi32-wtsregistersessionnotification>
// - `WM_WTSSESSION_CHANGE`／`WTS_SESSION_LOCK`(0x7)／`WTS_SESSION_UNLOCK`(0x8)：
//   <https://learn.microsoft.com/en-us/windows/win32/termserv/wm-wtssession-change>
// - `WTSQuerySessionInformationW`／`WTS_INFO_CLASS`（`WTSSessionInfoEx` 是這個列舉的正式
//   成員，非未公開 API）：
//   <https://learn.microsoft.com/en-us/windows/win32/api/wtsapi32/nf-wtsapi32-wtsquerysessioninformationw>
//   <https://learn.microsoft.com/en-us/windows/win32/api/wtsapi32/ne-wtsapi32-wts_info_class>
// - `WTSINFOEX`／`WTSINFOEX_LEVEL1`／`SessionFlags`（`WTS_SESSIONSTATE_LOCK`＝0、
//   `WTS_SESSIONSTATE_UNLOCK`＝1、`WTS_SESSIONSTATE_UNKNOWN`＝0xFFFFFFFF；該頁另記載
//   Windows 7／Server 2008 R2 上兩個旗標語意相反的已知瑕疵，不影響本專案目標的 Windows 11）：
//   <https://learn.microsoft.com/en-us/windows/win32/api/wtsapi32/ns-wtsapi32-wtsinfoex_level1_a>
// - `RegisterPowerSettingNotification`（`DEVICE_NOTIFY_WINDOW_HANDLE`＝互動式應用程式用視窗
//   handle 接收 `WM_POWERBROADCAST`）：
//   <https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-registerpowersettingnotification>
// - `POWERBROADCAST_SETTING`（`Data` 為變動長度陣列，長度見 `DataLength`）：
//   <https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-powerbroadcast_setting>
// - `GUID_CONSOLE_DISPLAY_STATE`／`GUID_ACDC_POWER_SOURCE`／`GUID_POWER_SAVING_STATUS`（`Data`
//   語意；`GUID_ENERGY_SAVER_STATUS` 該頁明文標示「relates to a prerelease product which may
//   be substantially modified」且未被 windows crate 收錄，故依 design.md 的備選寫法查證後
//   採用穩定的 `GUID_POWER_SAVING_STATUS`）：
//   <https://learn.microsoft.com/en-us/windows/win32/power/power-setting-guids>

/// [`GatekeeperCallbacks::on_pause_signal`] 通知的原始訊號種類（design.md D12「自動暫停」）。
/// 本模組只負責偵測，如何轉成 [`crate::widgets::PauseReason`]（含電池／省電要不要真的觸發
/// 暫停，取決於使用者設定 `pause_rules`）由呼叫端決定——同 [`GatekeeperCallbacks`] 其餘欄位
/// 「本模組不認得小工具註冊表／暫停原因集合」的分工原則。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseSignal {
    /// 工作階段鎖定（`true`）／解鎖（`false`）。
    Locked(bool),
    /// 顯示器關閉（`true`）／開啟或變暗（`false`）。
    DisplayOff(bool),
    /// `SHQueryUserNotificationState` 判定為忙碌（全螢幕應用程式／簡報設定／獨占全螢幕
    /// Direct3D 三者之一）。
    SystemBusy(bool),
    /// 目前使用電池（`true`）／使用 AC 或 UPS 短期電源（`false`）。
    OnBattery(bool),
    /// 省電模式（Battery Saver）開啟（`true`）／關閉（`false`）。
    PowerSaver(bool),
}

/// `QUERY_USER_NOTIFICATION_STATE` → 是否視為「忙碌」而暫停（design.md D12：「QUNS_BUSY、
/// QUNS_RUNNING_D3D_FULL_SCREEN、QUNS_PRESENTATION_MODE 視為暫停，其餘不暫停」）。純函式。
pub fn quns_indicates_busy(state: QUERY_USER_NOTIFICATION_STATE) -> bool {
    matches!(
        state,
        QUNS_BUSY | QUNS_RUNNING_D3D_FULL_SCREEN | QUNS_PRESENTATION_MODE
    )
}

/// `GUID_CONSOLE_DISPLAY_STATE` 通知的 `Data`（`MONITOR_DISPLAY_STATE`）→ 是否視為「顯示器
/// 關閉」。只有完全關閉（`PowerMonitorOff`＝0）才算；變暗（`PowerMonitorDim`＝2）畫面仍可見，
/// 不視為關閉（Microsoft Learn「Power Setting GUIDs」）。純函式。
pub fn display_off_from_value(value: u32) -> bool {
    value == 0
}

/// `GUID_ACDC_POWER_SOURCE` 通知的 `Data`（`SYSTEM_POWER_CONDITION`）→ 是否視為「使用電池」。
/// 只有 `PoDc`（1，機身電池）算；`PoAc`（0，市電或等效）與 `PoHot`（2，UPS 等短期備援電源）
/// 都不是一般理解的「使用電池」，不觸發（Microsoft Learn「Power Setting GUIDs」）。純函式。
pub fn on_battery_from_power_condition(value: u32) -> bool {
    value == 1
}

/// `GetSystemPowerStatus` 的 `ACLineStatus` → 是否使用電池（`None`＝未知，呼叫端不據此動作）。
/// `0`＝離線（電池）、`1`＝上線（AC）、其餘（含 `255`＝未知）視為未知。純函式。
pub fn on_battery_from_ac_line_status(status: u8) -> Option<bool> {
    match status {
        0 => Some(true),
        1 => Some(false),
        _ => None,
    }
}

/// `GUID_POWER_SAVING_STATUS` 通知的 `Data` → 是否開啟省電模式（`0`＝關閉、非 0＝開啟）。
/// 純函式。
pub fn power_saving_from_value(value: u32) -> bool {
    value != 0
}

/// `WTSINFOEX_LEVEL1` 的 `SessionFlags` → 是否鎖定（`None`＝未知，呼叫端不據此動作）。
/// `WTS_SESSIONSTATE_LOCK`＝鎖定、`WTS_SESSIONSTATE_UNLOCK`＝解鎖，其餘（含
/// `WTS_SESSIONSTATE_UNKNOWN`）未知。純函式。
pub fn session_locked_from_flags(flags: i32) -> Option<bool> {
    match flags as u32 {
        x if x == WTS_SESSIONSTATE_LOCK => Some(true),
        x if x == WTS_SESSIONSTATE_UNLOCK => Some(false),
        _ => None,
    }
}

/// `WM_WTSSESSION_CHANGE` 的 `wParam` → 鎖定狀態變化（`None`＝與鎖定無關的通知，如登入／
/// 登出／遠端連線，忽略）。純函式。
pub fn session_change_pause_signal(wparam: usize) -> Option<bool> {
    match wparam as u32 {
        WTS_SESSION_LOCK => Some(true),
        WTS_SESSION_UNLOCK => Some(false),
        _ => None,
    }
}

/// fix F5（review fix-soak low）：`WM_QUERYENDSESSION`／`WM_ENDSESSION` → 記錄檔的一行；其他
/// 訊息回傳 `None`。純函式。
///
/// 登出、關機與 Restart Manager 的 `RmShutdown` 都先送這兩則訊息，系統隨後可能直接結束行程、
/// 不經事件迴圈的 `RunEvent::Exit`。記下這一行，宿主消失後才能和 taskkill 之類的外部終止
/// 區分。`lParam` 旗標（Microsoft Learn「WM_QUERYENDSESSION message」）：
/// `ENDSESSION_CLOSEAPP`＝Restart Manager 要求關閉（安裝程式要更新使用中的檔案）、
/// `ENDSESSION_CRITICAL`＝強制結束、`ENDSESSION_LOGOFF`＝登出；全 0＝關機或重新開機。
/// `WM_ENDSESSION` 的 `wParam` 非 0＝工作階段確定要結束（0＝有程式否決、取消）。
pub fn end_session_record(msg: u32, wparam: usize, lparam: isize) -> Option<String> {
    let name = match msg {
        WM_QUERYENDSESSION => "WM_QUERYENDSESSION",
        WM_ENDSESSION => "WM_ENDSESSION",
        _ => return None,
    };
    // lParam 只用低 32 位元（旗標都是 DWORD）；ENDSESSION_LOGOFF 是最高位元，轉 i32 會變負數。
    let flags = lparam as u32;
    let mut names: Vec<&str> = [
        (ENDSESSION_CLOSEAPP, "ENDSESSION_CLOSEAPP"),
        (ENDSESSION_CRITICAL, "ENDSESSION_CRITICAL"),
        (ENDSESSION_LOGOFF, "ENDSESSION_LOGOFF"),
    ]
    .iter()
    .filter(|(bit, _)| flags & bit != 0)
    .map(|(_, name)| *name)
    .collect();
    if flags == 0 {
        names.push("0（關機或重新開機）");
    }
    let ending = if msg == WM_ENDSESSION {
        format!(" ending={}", wparam != 0)
    } else {
        String::new()
    };
    Some(format!(
        "{name}{ending} lParam=0x{flags:08X} flags={}",
        names.join("|")
    ))
}

/// `POWERBROADCAST_SETTING::Data` 的前 4 位元組解讀成小端序 `DWORD`（`DataLength` 不足 4 時
/// 回傳 `None`；`GUID_CONSOLE_DISPLAY_STATE`／`GUID_ACDC_POWER_SOURCE`／
/// `GUID_POWER_SAVING_STATUS` 三者的 `Data` 依 Microsoft Learn 皆為 `DWORD`）。
fn power_setting_data_u32(setting: &POWERBROADCAST_SETTING) -> Option<u32> {
    if setting.DataLength < 4 {
        return None;
    }
    // SAFETY: `Data` 在 windows crate 宣告為 1 位元組的彈性陣列（`POWERBROADCAST_SETTING`
    // 文件：「Data[1]：The new value…」，實際資料延伸到 `DataLength` 位元組），上面已確認
    // `DataLength >= 4`；`setting` 來自系統經 `WM_POWERBROADCAST` 傳入、對這次訊息處理期間
    // 有效的緩衝區，讀取緊接在 `Data` 起始位址之後的 4 個位元組落在該緩衝區界內。
    let bytes: [u8; 4] = unsafe { std::slice::from_raw_parts(setting.Data.as_ptr(), 4) }
        .try_into()
        .ok()?;
    Some(u32::from_le_bytes(bytes))
}

/// `WM_POWERBROADCAST`／`PBT_POWERSETTINGCHANGE` 的 `lParam`：依 `PowerSetting` 這個 GUID
/// 分派到對應的解讀純函式，通知呼叫端（design.md D12）。不認得的 GUID（例如系統送出本模組未
/// 註冊的其他電源設定通知）略過。
///
/// # Safety
/// 由呼叫端（[`gatekeeper_subclass_proc`]）保證 `lparam` 來自
/// `WM_POWERBROADCAST`／`PBT_POWERSETTINGCHANGE`——Microsoft Learn 保證此時指向有效的
/// `POWERBROADCAST_SETTING`，在本次訊息處理期間可讀。
unsafe fn handle_power_setting_change(state: &GatekeeperState, lparam: LPARAM) {
    let ptr = lparam.0 as *const POWERBROADCAST_SETTING;
    if ptr.is_null() {
        return;
    }
    // SAFETY: 見本函式文件；純讀取，不寫入。
    let setting = unsafe { &*ptr };
    let Some(value) = power_setting_data_u32(setting) else {
        return;
    };
    let guid = setting.PowerSetting;
    let signal = if guid == GUID_CONSOLE_DISPLAY_STATE {
        Some(PauseSignal::DisplayOff(display_off_from_value(value)))
    } else if guid == GUID_ACDC_POWER_SOURCE {
        Some(PauseSignal::OnBattery(on_battery_from_power_condition(
            value,
        )))
    } else if guid == GUID_POWER_SAVING_STATUS {
        Some(PauseSignal::PowerSaver(power_saving_from_value(value)))
    } else {
        None
    };
    if let Some(signal) = signal {
        gatekeeper_log(&format!("EVENT power-setting-change signal={signal:?}"));
        (state.callbacks.on_pause_signal)(signal);
    }
}

/// 啟動當下查詢工作階段是否已鎖定（task-5.5-brief：初始狀態要主動查詢，不只靠之後的通知）。
/// 查詢失敗或回傳未知值一律回傳 `None`。
fn query_session_locked() -> Option<bool> {
    let mut buffer = PWSTR::null();
    let mut bytes_returned: u32 = 0;
    // SAFETY: `hserver=None`（等同 `WTS_CURRENT_SERVER_HANDLE`）＋`WTS_CURRENT_SESSION` 查詢
    // 本機當前工作階段（Microsoft Learn `WTSQuerySessionInformationW`：「Only specify
    // WTS_CURRENT_SESSION when obtaining session information on the local server」）；
    // `buffer`／`bytes_returned` 是本函式的局部變數，成功時 `buffer` 指向系統配置的緩衝區，
    // 用完須以 `WTSFreeMemory` 釋放（下方已釋放）。
    let ok = unsafe {
        WTSQuerySessionInformationW(
            None,
            WTS_CURRENT_SESSION,
            WTSSessionInfoEx,
            &mut buffer,
            &mut bytes_returned,
        )
    };
    if ok.is_err() || buffer.is_null() {
        return None;
    }
    // SAFETY: 呼叫成功，`buffer` 指向系統配置、大小至少 `bytes_returned` 位元組的緩衝區；
    // `WTSSessionInfoEx` 這個 info class 的回傳型別固定是 `WTSINFOEXW`（Microsoft Learn
    // 「WTSINFOEXA」：「This structure is returned … when you specify "WTSSessionInfoEx"」）。
    let info = unsafe { (buffer.0 as *const WTSINFOEXW).read_unaligned() };
    // SAFETY: `info.Level` 剛從系統回傳的緩衝區讀出；union 的每個成員都是純資料（`Copy`、
    // 無指標），任何 bit pattern 都是合法值，讀取本身不會有記憶體安全問題——只有「當
    // `Level != 1` 時這個值語意上沒有意義」，故用 `then_some` 讓值一律算出、僅在 `Level == 1`
    // 時保留（Microsoft Learn「WTSINFOEXA」：「1: The Data member is a WTSINFOEX_LEVEL1
    // structure」）。
    let flags = (info.Level == 1).then_some(unsafe { info.Data.WTSInfoExLevel1 }.SessionFlags);
    // SAFETY: `buffer` 是本函式獨占取得、上面已讀完的緩衝區，釋放恰好一次。
    unsafe { WTSFreeMemory(buffer.0 as *mut _) };
    flags.and_then(session_locked_from_flags)
}

/// 啟動當下查詢目前是否使用電池（`GetSystemPowerStatus` 的 `ACLineStatus`）。與既有
/// [`power_saver_active`] 各自獨立呼叫同一顆 API——成本低，且兩者的失敗判定各自獨立，拆開
/// 比共用一次查詢結果再拆兩個函式更簡單。
fn query_on_battery() -> Option<bool> {
    let mut status = SYSTEM_POWER_STATUS::default();
    // SAFETY: 同 `power_saver_active`：`status` 是局部變數，大小與 `GetSystemPowerStatus`
    // 期待的結構相符，只會被寫入、不會寫超界。
    let ok = unsafe { GetSystemPowerStatus(&mut status) };
    ok.is_ok()
        .then_some(status.ACLineStatus)
        .and_then(on_battery_from_ac_line_status)
}

/// `SHQueryUserNotificationState` 的一次查詢（design.md D12：每 5 秒輪詢，見
/// [`BUSY_POLL_TIMER_ID`]）。
fn poll_user_busy() -> Option<bool> {
    // SAFETY: 無前置條件的純查詢（同探針 `host/examples/probe_wind.rs` 既有用法）。
    unsafe { SHQueryUserNotificationState() }
        .ok()
        .map(quns_indicates_busy)
}

// ── D8：毛玻璃（Acrylic）啟用判定 ──────────────────────────────────────────────

/// `DWMWA_SYSTEMBACKDROP_TYPE`（系統背景材質，Acrylic／Mica 的統一開關）支援的最低
/// Windows 11 build 號。Microsoft Learn 兩處均記載「Windows 11 Build 22621」：
/// - <https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/ne-dwmapi-dwm_systembackdrop_type>
///   （`DWM_SYSTEMBACKDROP_TYPE` 列舉頁，Requirements 表「Minimum supported client」）
/// - <https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/ne-dwmapi-dwmwindowattribute>
///   （`DWMWINDOWATTRIBUTE` 列舉頁，`DWMWA_SYSTEMBACKDROP_TYPE` 條目「This value is supported
///   starting with Windows 11 Build 22621」）
pub const MIN_BUILD_FOR_SYSTEM_BACKDROP: u32 = 22_621;

/// 探針 1.2 的結論（DWM Acrylic 在小工具未取得焦點時是否仍呈現）：**不呈現**，故為
/// `false`。小工具設計上永不取得焦點（`.focusable(false)`），實測（`host/tools/evidence/1.2-*`，
/// 筆電 2026-09-28、4K 2026-10-06）失焦時系統背景材質被攤平成不透明純色塊，毛玻璃因此停用
/// （design.md D8）。若日後改變小工具的焦點模型或補驗 `WM_NCACTIVATE` 強制使用中渲染有效，
/// 這個常數是唯一要改的地方。
pub const ACRYLIC_WORKS_UNFOCUSED_DEFAULT: bool = false;

/// [`resolve_appearance`] 的輸入條件（design.md D8）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcrylicConditions {
    /// 目前作業系統的 build 號（例如登錄檔 `CurrentBuildNumber`，見 [`os_build_number`]）。
    pub build_number: u32,
    /// Windows 設定「個人化 → 色彩 → 透明效果」是否開啟。
    pub transparency_enabled: bool,
    /// 是否處於省電模式（battery saver）。
    pub power_saver_active: bool,
    /// 探針 1.2 的結論：毛玻璃在視窗未取得焦點時是否仍會呈現（實測為不呈現，正式呼叫端傳
    /// [`ACRYLIC_WORKS_UNFOCUSED_DEFAULT`]＝`false`）。
    pub acrylic_works_unfocused: bool,
    /// 使用者在外觀設定中的選擇（[`Settings::appearance_mode`](crate::settings::Settings)）。
    pub user_mode: AppearanceMode,
}

/// D8 毛玻璃啟用判定純函式：依 [`AcrylicConditions`] 決定小工具最終要用哪一種外觀
/// （specs/widget-host-windows「小工具外觀模式」）。
///
/// 判定順序——任一項不滿足即回傳 [`AppearanceMode::Solid`]，全部滿足才回傳
/// [`AppearanceMode::Acrylic`]：
/// 1. 使用者在設定中選純色（使用者意願最優先，即使其餘條件全數滿足）。
/// 2. 毛玻璃在未取得焦點時無法呈現（探針 1.2 結論）——小工具設計上永不取得焦點，此條件
///    一旦成立就沒有其他情況能讓毛玻璃有意義地呈現。
/// 3. build 號未達 [`MIN_BUILD_FOR_SYSTEM_BACKDROP`]（作業系統不支援系統背景材質）。
/// 4. Windows「透明效果」關閉。
/// 5. 處於省電模式。
pub fn resolve_appearance(conditions: &AcrylicConditions) -> AppearanceMode {
    if conditions.user_mode == AppearanceMode::Solid {
        return AppearanceMode::Solid;
    }
    if !conditions.acrylic_works_unfocused {
        return AppearanceMode::Solid;
    }
    if conditions.build_number < MIN_BUILD_FOR_SYSTEM_BACKDROP {
        return AppearanceMode::Solid;
    }
    if !conditions.transparency_enabled {
        return AppearanceMode::Solid;
    }
    if conditions.power_saver_active {
        return AppearanceMode::Solid;
    }
    AppearanceMode::Acrylic
}

/// [`AcrylicConditions`] 由系統讀值組出（fix round 1，Codex 2.8）：三個系統狀態任一讀取失敗
/// （`None`＝未知）一律當成「不滿足」——build 未知→視為不支援（0）、透明效果未知→視為關閉、
/// 省電模式未知→視為開啟——使 [`resolve_appearance`] 退回純色（specs/widget-host-windows
/// 「小工具外觀模式」：僅在條件滿足時使用毛玻璃，其餘退回純色）。呼叫端（task 3.x／5.5）在
/// 下一次系統通知或重試時重新讀取、重新判定即可恢復。
impl AcrylicConditions {
    pub fn from_readings(
        build_number: Option<u32>,
        transparency_enabled: Option<bool>,
        power_saver_active: Option<bool>,
        acrylic_works_unfocused: bool,
        user_mode: AppearanceMode,
    ) -> Self {
        AcrylicConditions {
            build_number: build_number.unwrap_or(0),
            transparency_enabled: transparency_enabled.unwrap_or(false),
            power_saver_active: power_saver_active.unwrap_or(true),
            acrylic_works_unfocused,
            user_mode,
        }
    }
}

/// 讀取目前是否處於省電模式（battery saver）：`GetSystemPowerStatus` 的
/// `SystemStatusFlag`——0＝關閉、1＝開啟（Microsoft Learn：
/// <https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-system_power_status>，
/// 「Note This flag and the GUID_POWER_SAVING_STATUS GUID were introduced in Windows 10.」）。
/// 呼叫失敗回傳 `None`（未知），由 [`AcrylicConditions::from_readings`] 保守處理為純色。
pub fn power_saver_active() -> Option<bool> {
    let mut status = SYSTEM_POWER_STATUS::default();
    // SAFETY: `status` 是本函式的局部變數，大小與 `SYSTEM_POWER_STATUS` 完全匹配；
    // `GetSystemPowerStatus` 只會寫入這個結構、不會寫超界（Microsoft Learn 標準用法：
    // 傳入結構的可變參照）。
    let ok = unsafe { GetSystemPowerStatus(&mut status) };
    power_saver_from_status(ok.is_ok().then_some(status.SystemStatusFlag))
}

/// 解讀 `GetSystemPowerStatus` 的結果（`None`＝呼叫失敗→未知）。
fn power_saver_from_status(flag: Option<u8>) -> Option<bool> {
    flag.map(|flag| flag != 0)
}

/// 讀取 Windows「透明效果」設定：`HKEY_CURRENT_USER`\Software\Microsoft\Windows\
/// CurrentVersion\Themes\Personalize 的 `EnableTransparency`（`REG_DWORD`，brief 指定的來源；
/// Windows 沒有為此值另外提供公開 Win32／WinRT 函式，讀登錄檔是官方「個人化」設定頁本身的
/// 落地位置）。缺該值或讀取失敗回傳 `None`（未知），由 [`AcrylicConditions::from_readings`]
/// 保守處理為純色。
///
/// 待查證：「該值缺席時 Windows 視為透明效果開啟」沒有一手文件可查，故不據此推定開啟；本機
/// （2026-09-28）實測該值存在（=1）。
pub fn transparency_effects_enabled() -> Option<bool> {
    transparency_from_registry(read_registry_dword(
        HKEY_CURRENT_USER,
        w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
        w!("EnableTransparency"),
    ))
}

/// 解讀 `EnableTransparency` 登錄值（`None`＝讀取失敗或值不存在→未知）。
fn transparency_from_registry(value: Option<u32>) -> Option<bool> {
    value.map(|value| value != 0)
}

/// 讀取目前作業系統的 build 號：`HKEY_LOCAL_MACHINE`\SOFTWARE\Microsoft\Windows NT\
/// CurrentVersion 的 `CurrentBuildNumber`（`REG_SZ`）。讀不到或無法解析為整數時回傳
/// `None`——由 [`AcrylicConditions::from_readings`] 保守處理為「不支援」，不讓讀取失敗誤判
/// 為支援。
///
/// 待查證：Microsoft Learn 沒有為這個登錄值本身開設公開 API 參考頁（它是作業系統安裝時寫入
/// 的資料，不是函式呼叫的回傳值）；task brief 建議的替代方案 `RtlGetVersion` 同樣沒有
/// Microsoft Learn 官方文件頁（未公開的 ntdll 匯出函式），與 AGENTS.md 全域約束「不使用未公開
/// Win32 API」牴觸，故改採此登錄值——Windows「設定 → 系統 → 關於」頁顯示的組建版本即讀自
/// 此值，`winver.exe` 亦同，是廣泛已知且穩定多年的位置，但仍非一手 API 文件可查證的字串。
pub fn os_build_number() -> Option<u32> {
    let text = read_registry_string(
        HKEY_LOCAL_MACHINE,
        w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion"),
        w!("CurrentBuildNumber"),
    )?;
    text.trim().parse().ok()
}

/// 從登錄檔讀出一個 `REG_DWORD` 值。找不到機碼／值、型別不符、或 API 回傳非
/// `ERROR_SUCCESS` 一律回傳 `None`，交由呼叫端決定保守退回值。
fn read_registry_dword(hkey: HKEY, subkey: PCWSTR, value: PCWSTR) -> Option<u32> {
    let mut data: u32 = 0;
    let mut data_len: u32 = size_of::<u32>() as u32;
    // SAFETY: `data` 的大小與傳入的 `data_len` 一致，`RRF_RT_REG_DWORD` 限制只接受 4
    // 位元組的 DWORD 值，`RegGetValueW` 不會寫超過 `data_len` 指定的位元組數。
    let status = unsafe {
        RegGetValueW(
            hkey,
            subkey,
            value,
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut _),
            Some(&mut data_len),
        )
    };
    (status == ERROR_SUCCESS).then_some(data)
}

/// 從登錄檔讀出一個 `REG_SZ` 字串值。緩衝區容量 [`REGISTRY_STRING_BUF_LEN`] 個 UTF-16
/// 字元，對本檔用到的值（組建號，最長 5 位數字）綽綽有餘；找不到、型別不符、緩衝區不足或
/// 讀取失敗一律回傳 `None`。
fn read_registry_string(hkey: HKEY, subkey: PCWSTR, value: PCWSTR) -> Option<String> {
    const REGISTRY_STRING_BUF_LEN: usize = 64;
    let mut buf = [0u16; REGISTRY_STRING_BUF_LEN];
    let mut data_len: u32 = (buf.len() * size_of::<u16>()) as u32;
    // SAFETY: `buf` 的位元組長度與傳入的 `data_len` 一致，`RegGetValueW` 不會寫超過此長度。
    let status = unsafe {
        RegGetValueW(
            hkey,
            subkey,
            value,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut data_len),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    Some(utf16_buf_to_string(&buf))
}

/// 把 Tauri 的螢幕清單轉成 [`MonitorInfo`] 清單（design.md D9）的核心對應邏輯。純資料轉換，
/// 不呼叫 Win32；輸入是 [`MonitorSnapshot`]（見其文件：`tauri::Monitor` 沒有公開建構子，故
/// 核心邏輯改吃這個可在測試裡組字面量的中介型別），實機呼叫端用
/// [`monitor_infos_from_tauri_monitors`]。
///
/// - 識別：以 `monitor.name` 在 `device_paths`（[`source_name_to_device_path`] 的輸出）查出
///   穩定的 `monitorDevicePath`。查不到時（兩次列舉之間組態變動的競態，或上游任一步查詢失敗
///   回傳空表）`id` 為 `None`＝未解析：這台顯示器仍留在清單、可被暫時定位（主螢幕退回），但
///   **不以 `\\.\DISPLAYn` 暫代**——DISPLAYn 會隨拔插重新編號，一旦經拖曳被存進設定，日後
///   可能對到另一台螢幕（design.md D9；fix round 1，Codex 2.5）。未解析者的保存規則見
///   [`crate::layout::MonitorInfo::id`]。
/// - `monitor.name` 為 `None`（顯示器在查詢當下已消失）的項目直接略過，不產生無法識別身分的
///   `MonitorInfo`。
/// - `is_primary`：與 `primary`（呼叫端另外以 Tauri `primary_monitor()` 查得）的 `name`
///   相同即為主螢幕；`primary` 為 `None` 或其 `name` 為 `None` 時沒有任何一台會被標記為主螢幕
///   （呼叫端／`resolve_grid_placements` 的退回鏈需能處理這種極端情況，不可假設一定有）。
/// - 工作區與縮放：直接採用 `monitor.work_area`（實體像素）與 `monitor.scale_factor`，理由見
///   本檔模組文件。
pub fn monitors_from_tauri(
    monitors: &[MonitorSnapshot],
    primary: Option<&MonitorSnapshot>,
    device_paths: &HashMap<String, String>,
) -> Vec<MonitorInfo> {
    let primary_name = primary.and_then(|m| m.name.as_deref());

    monitors
        .iter()
        .filter_map(|monitor| {
            let name = monitor.name.as_deref()?;

            Some(MonitorInfo {
                id: device_paths.get(name).cloned().map(MonitorId::Device),
                work_area: monitor.work_area,
                scale_factor: monitor.scale_factor,
                is_primary: primary_name == Some(name),
            })
        })
        .collect()
}

/// [`monitors_from_tauri`] 的實機入口：直接接受 Tauri 的 `Monitor` 清單（`app.available_
/// monitors()`／`window.available_monitors()` 的回傳值），內部轉成 [`MonitorSnapshot`] 後
/// 委派給核心對應邏輯。視窗工廠（task 3.1）與多螢幕重新定位（task 3.5）呼叫這個函式，不直接
/// 呼叫 `monitors_from_tauri`。
pub fn monitor_infos_from_tauri_monitors(
    monitors: &[Monitor],
    primary: Option<&Monitor>,
    device_paths: &HashMap<String, String>,
) -> Vec<MonitorInfo> {
    let snapshots: Vec<MonitorSnapshot> = monitors.iter().map(MonitorSnapshot::from).collect();
    let primary_snapshot = primary.map(MonitorSnapshot::from);
    // fix monitor-id：查不到穩定識別時，這台顯示器上的放開一律不合法、`Device` 記錄也對不到它；
    // 寫出 GDI 名稱與對照表內容，日後不必再猜是哪一台、對照表當下長什麼樣子。
    for name in unresolved_monitor_names(&snapshots, device_paths) {
        let mut known: Vec<&str> = device_paths.keys().map(String::as_str).collect();
        known.sort_unstable();
        log::warn!(
            "顯示器無穩定識別：GDI 名稱 {name} 不在 QueryDisplayConfig 對照表內\
             （對照表的 GDI 名稱：{known:?}）"
        );
    }
    monitors_from_tauri(&snapshots, primary_snapshot.as_ref(), device_paths)
}

/// fix monitor-id：`monitors` 中有 GDI 名稱、但在 `device_paths`（[`source_name_to_device_path`]
/// 的輸出）查不到穩定識別者的 GDI 名稱，順序同輸入——即 [`monitors_from_tauri`] 會給
/// `id: None` 的那幾台。`name` 為 `None`（查詢當下已消失）者本來就會被略過，不列入。純函式。
pub fn unresolved_monitor_names<'a>(
    monitors: &'a [MonitorSnapshot],
    device_paths: &HashMap<String, String>,
) -> Vec<&'a str> {
    monitors
        .iter()
        .filter_map(|m| m.name.as_deref())
        .filter(|name| !device_paths.contains_key(*name))
        .collect()
}

/// task 5.7／design.md D12：向系統註冊本行程可重新啟動。只在勝出 single-instance 判定的那個
/// 行程呼叫一次（`main.rs` 的 `.setup()` 閉包），失敗（例如作業系統版本過舊，但本專案不支援
/// Windows 10 以前版本，理論上不會發生；仍照 Microsoft Learn 文件的錯誤碼防禦）只記錄、不視
/// 為致命錯誤——就算註冊失敗，宿主仍能正常運作，只是不會在 Restart Manager 關閉後被自動重新
/// 啟動。命令列參數 `--restarted` 與 `tray::is_automated_relaunch` 判斷的字串同一個（`Register
/// ApplicationRestart` 文件明講「不要在命令列裡包含執行檔本身的名稱，函式會自動補上」，故只放
/// 旗標本身）；`dwFlags` 的選擇理由見本檔模組文件「task 5.7 補上」一節。
pub fn register_for_restart() {
    // SAFETY: `w!("--restarted")` 展開為指向以 null 結尾之 UTF-16 靜態資料的 `PCWSTR`，生命
    // 週期為 `'static`，涵蓋整個呼叫；`RegisterApplicationRestart` 只讀取這個字串，不保留
    // 指標、不寫入呼叫端記憶體（Microsoft Learn 標準用法，與本檔其餘 `w!(...)` 呼叫點一致）。
    if let Err(err) =
        unsafe { RegisterApplicationRestart(w!("--restarted"), RESTART_NO_CRASH | RESTART_NO_HANG) }
    {
        log::warn!("RegisterApplicationRestart 失敗：{err}");
    } else {
        log::info!("已向系統註冊重新啟動（RegisterApplicationRestart，命令列 --restarted）");
    }
}

// ── panic 記錄的執行緒／模組資訊（fix F5，review fix-soak low×2）──────────────────────

// panic hook（`logging::panic_record`）要記錄「哪個執行緒」，但 hook 裡不能再呼叫可能 panic
// 的 `std::thread::current()`：在執行緒 TLS 解構期間 panic 時，部分 std 版本取 current 會再
// panic，panic 中再 panic 直接 abort，原本的訊息就寫不進記錄檔。以下兩個 Win32 呼叫都不碰
// Rust 的 TLS，任何時機呼叫都不會 panic。

/// 目前執行緒的 Win32 執行緒 id（與 WER 報告、ProcMon、除錯器顯示的 TID 相同）。
pub(crate) fn current_thread_id() -> u32 {
    // SAFETY: GetCurrentThreadId 無參數、無前置條件。
    unsafe { GetCurrentThreadId() }
}

/// 目前執行緒的描述：std 在 Windows 上為具名執行緒（`thread::Builder::name`）呼叫
/// `SetThreadDescription`，這裡讀回同一個字串。未命名或讀取失敗回傳 `None`（主執行緒 std
/// 不設描述，也是 `None`）。
pub(crate) fn current_thread_description() -> Option<String> {
    // SAFETY: GetCurrentThread 回傳的偽控制代碼不需關閉；GetThreadDescription 成功時回傳以
    // LocalAlloc 配置的字串，讀完後必須以 LocalFree 釋放（Microsoft Learn
    // GetThreadDescription）。
    unsafe {
        let pwstr = GetThreadDescription(GetCurrentThread()).ok()?;
        if pwstr.is_null() {
            return None;
        }
        let text = pwstr.to_string().ok();
        let _ = LocalFree(Some(HLOCAL(pwstr.0.cast())));
        text.filter(|s| !s.is_empty())
    }
}

/// 本行程執行檔（fc-host.exe）的模組基底位址；取不到回傳 0。panic hook 把它和 backtrace 一起
/// 記下：正式安裝沒附 PDB 時 backtrace 只剩原始位址，有 ASLR 時必須「位址−基底」換算成 RVA，
/// 事後才能對著同一版建置的 PDB 解析回函式與行號。
pub(crate) fn exe_module_base() -> usize {
    // SAFETY: GetModuleHandleW(None) 取呼叫行程的 exe 模組，不增加參考計數、不需釋放。
    unsafe { GetModuleHandleW(PCWSTR::null()) }
        .map(|module| module.0 as usize)
        .unwrap_or(0)
}

// ── 錯誤模式：讓原生當機留下 WER 報告（2026-10-01 soak 宿主無聲消失）────────────────

/// `mode` 去掉 `SEM_NOGPFAULTERRORBOX`、其餘位元原樣保留（純函式，見
/// [`ensure_crashes_reach_wer`]）。
pub fn error_mode_reporting_crashes(mode: u32) -> u32 {
    mode & !SEM_NOGPFAULTERRORBOX.0
}

/// 錯誤模式會由子行程**繼承**（Microsoft Learn `SetErrorMode`：「a child process inherits the
/// error mode of its parent process」）。從 Claude Code 這類開發工具的殼層啟動時，宿主會繼承到
/// `0x8003`（含 `SEM_NOGPFAULTERRORBOX`），原生當機就**完全不經 WER**：沒有「Application
/// Error」事件、沒有 WER 報告、LocalDumps 也不會寫 dump（2026-10-01 以刻意存取違規的探針
/// 實測：繼承 0x8003 時事件記錄一筆都沒有，錯誤模式為 0 時立刻出現事件 1000／1001）。
/// soak 宿主就是在這種情況下無聲消失，事後無從判斷是原生當機還是其他原因。
///
/// 這裡只清掉 `SEM_NOGPFAULTERRORBOX`，`SEM_FAILCRITICALERRORS`／`SEM_NOOPENFILEERRORBOX`
/// 原樣保留（它們只抑制「磁碟未就緒」類對話框，對常駐的系統匣程式是合理的）。從檔案總管或
/// 開機自啟啟動時繼承到的本來就是 0，呼叫後不變。`main()` 在安裝記錄器之後立即呼叫。
///
/// 注意：本宿主的 `RegisterApplicationRestart` 帶 `RESTART_NO_CRASH`（見
/// [`register_for_restart`]），所以讓當機經過 WER 只會留下報告與 dump，**不會**觸發自動重啟。
pub fn ensure_crashes_reach_wer() {
    // SAFETY: 兩個函式都只讀寫本行程的錯誤模式旗標，不涉及呼叫端記憶體。
    let before = unsafe { GetErrorMode() };
    let after = error_mode_reporting_crashes(before);
    if after != before {
        unsafe { SetErrorMode(THREAD_ERROR_MODE(after)) };
        log::warn!(
            "錯誤模式繼承自啟動者 0x{before:X}（含 SEM_NOGPFAULTERRORBOX，原生當機不會留 WER \
             報告），已改為 0x{after:X}"
        );
    } else {
        log::info!("錯誤模式 0x{before:X}（原生當機會交給 WER）");
    }
}

#[cfg(test)]
mod panic_context_tests {
    use super::{current_thread_description, current_thread_id, exe_module_base};

    #[test]
    fn exe_module_base_is_below_code_in_this_executable() {
        let base = exe_module_base();
        assert_ne!(base, 0);
        // 本函式的程式碼位於執行檔映像內，位址必在基底之上、且不會離基底太遠（< 256 MiB）。
        let code = exe_module_base as fn() -> usize as usize;
        assert!(code > base, "code=0x{code:X} base=0x{base:X}");
        assert!(code - base < 256 << 20, "code=0x{code:X} base=0x{base:X}");
    }

    #[test]
    fn thread_id_is_nonzero_and_differs_between_threads() {
        let here = current_thread_id();
        let there = std::thread::spawn(current_thread_id)
            .join()
            .expect("執行緒 panic");
        assert_ne!(here, 0);
        assert_ne!(there, 0);
        assert_ne!(here, there);
    }

    #[test]
    fn thread_description_reports_std_thread_name() {
        let named = std::thread::Builder::new()
            .name("fc-desc-test".into())
            .spawn(current_thread_description)
            .expect("建立執行緒失敗")
            .join()
            .expect("執行緒 panic");
        assert_eq!(named.as_deref(), Some("fc-desc-test"));
        let unnamed = std::thread::spawn(current_thread_description)
            .join()
            .expect("執行緒 panic");
        assert_eq!(unnamed, None);
    }
}

#[cfg(test)]
mod error_mode_tests {
    use super::error_mode_reporting_crashes;

    #[test]
    fn clears_only_the_no_gpfault_box_bit() {
        // 0x8003：從 Claude Code／Node 類工具的殼層啟動時繼承到的值（2026-10-01 實測）。
        assert_eq!(error_mode_reporting_crashes(0x8003), 0x8001);
        assert_eq!(error_mode_reporting_crashes(0x0002), 0);
        assert_eq!(error_mode_reporting_crashes(0), 0);
        assert_eq!(error_mode_reporting_crashes(0x8001), 0x8001);
    }
}

/// task 5.7／design.md D12：使用者從系統匣「結束」時呼叫，取消上面的註冊——確保這種主動結束
/// 不會被 Restart Manager 之後的重新啟動要求誤判為需要恢復（specs「Restart Manager 關閉後
/// 重新啟動」Requirement 的 MUST NOT 那句）。失敗只記錄：行程即將 `app.exit(0)` 結束，取消
/// 註冊失敗不影響結束本身，且行程結束後這個註冊本來就會隨行程消失。
pub fn unregister_restart() {
    // SAFETY: 無參數、不涉及呼叫端記憶體，`UnregisterApplicationRestart` 只是清除行程內部
    // 已由 `RegisterApplicationRestart` 建立的 WER 註冊記錄。
    if let Err(err) = unsafe { UnregisterApplicationRestart() } {
        log::warn!("UnregisterApplicationRestart 失敗：{err}");
    } else {
        log::info!("已取消重新啟動註冊（UnregisterApplicationRestart）");
    }
}

// ── 隔離環境偵測（data-layer-rust tasks.md 5.3、design.md D10）─────────────────────────

/// 環境變數 `LOCALAPPDATA` 與系統登記的本機應用程式資料夾不一致的證據（兩邊都已展開成長路徑）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalAppDataMismatch {
    /// 環境變數 `LOCALAPPDATA`（展開後）。
    pub env_value: String,
    /// `SHGetKnownFolderPath(FOLDERID_LocalAppData)`（展開後）。
    pub registered: String,
}

/// 兩個**已展開**的目錄路徑是否是同一個目錄：不分大小寫、`/` 與 `\` 視為相同、忽略結尾斜線。
/// 純字串比較——短檔名（8.3）展開與 `..` 正規化由呼叫端先做（[`expand_dir_path`]）。
/// 磁碟機根目錄（`C:\`）與 `C:` 視為相同。
pub fn same_directory(a: &str, b: &str) -> bool {
    normalize_dir_for_compare(a) == normalize_dir_for_compare(b)
}

fn normalize_dir_for_compare(path: &str) -> String {
    let unified: String = path
        .trim()
        .chars()
        .map(|c| if c == '/' { '\\' } else { c })
        .collect();
    let trimmed = unified.trim_end_matches('\\');
    // `C:` 與 `C:\` 都是磁碟機根目錄：統一補回結尾反斜線。
    let base = if trimmed.len() == 2 && trimmed.ends_with(':') {
        format!("{trimmed}\\")
    } else {
        trimmed.to_string()
    };
    base.to_lowercase()
}

/// 判定核心（純函式）：兩邊都取得且不同才算不一致；任一邊取不到（環境變數不存在、系統呼叫失敗）＝無法判定
/// ＝回傳 `None`（呼叫端視為非隔離：誤判成隔離會讓一般使用者靜默停在舊資料，比誤判成非隔離嚴重）。
pub fn local_app_data_mismatch(
    env_value: Option<&str>,
    registered: Option<&str>,
) -> Option<LocalAppDataMismatch> {
    let (env_value, registered) = (env_value?, registered?);
    if same_directory(env_value, registered) {
        None
    } else {
        Some(LocalAppDataMismatch {
            env_value: env_value.to_string(),
            registered: registered.to_string(),
        })
    }
}

/// 把路徑以 `GetFullPathNameW`（絕對化、消去 `.`／`..`）再 `GetLongPathNameW`（8.3 短名展開、還原大小寫）
/// 展開；任一步失敗就退回上一步的結果（例如路徑不存在時 `GetLongPathNameW` 會失敗）。
pub fn expand_dir_path(path: &str) -> String {
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let full = call_path_api(|buf| {
        // SAFETY: `wide` 以 NUL 結尾且在呼叫期間存活；`buf` 是呼叫端配置的可寫緩衝區。
        unsafe { GetFullPathNameW(PCWSTR(wide.as_ptr()), Some(buf), None) }
    })
    .unwrap_or_else(|| path.to_string());
    let full_wide: Vec<u16> = full.encode_utf16().chain(std::iter::once(0)).collect();
    call_path_api(|buf| {
        // SAFETY: 同上。
        unsafe { GetLongPathNameW(PCWSTR(full_wide.as_ptr()), Some(buf)) }
    })
    .unwrap_or(full)
}

/// 兩段式呼叫「回傳寫入字元數；緩衝區不足時回傳所需大小（含結尾 NUL）」的路徑 API。0＝失敗。
fn call_path_api(mut call: impl FnMut(&mut [u16]) -> u32) -> Option<String> {
    let mut buf = vec![0u16; 512];
    for _ in 0..2 {
        let n = call(&mut buf) as usize;
        if n == 0 {
            return None;
        }
        if n < buf.len() {
            return Some(String::from_utf16_lossy(&buf[..n]));
        }
        buf = vec![0u16; n + 1];
    }
    None
}

/// 系統登記的本機應用程式資料夾（`SHGetKnownFolderPath(FOLDERID_LocalAppData)`）；失敗回 `None`。
fn registered_local_app_data() -> Option<String> {
    // SAFETY: `FOLDERID_LocalAppData` 是常數 GUID；成功時回傳的字串由 COM 配置，下面複製後
    // 以 `CoTaskMemFree` 釋放（MSDN：呼叫端負責釋放）。
    unsafe {
        let p = SHGetKnownFolderPath(&FOLDERID_LocalAppData, KNOWN_FOLDER_FLAG(0), None).ok()?;
        let text = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const std::ffi::c_void));
        text
    }
}

/// 隔離偵測：本行程的環境變數 `LOCALAPPDATA` 與系統登記的本機應用程式資料夾不同＝驗收腳本以暫存資料夾
/// 隔離宿主（design.md D10）。回傳不一致的證據（供記錄檔寫明原因）；一致或無法判定回 `None`。
pub fn detect_isolated_local_app_data() -> Option<LocalAppDataMismatch> {
    let env_value = std::env::var("LOCALAPPDATA")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(|v| expand_dir_path(&v));
    let registered = registered_local_app_data().map(|v| expand_dir_path(&v));
    local_app_data_mismatch(env_value.as_deref(), registered.as_deref())
}

// ── 開機自啟登錄（fix F2，review 5.4）────────────────────────────────────────────────

/// `HKCU` 底下的開機自啟機碼。
const RUN_SUBKEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
/// Run 值的名稱：沿用 task 5.4 `tauri-plugin-autostart` 預設的 `productName`，既有安裝的值會被
/// 同一名稱覆寫／刪除，不留下孤兒。
const RUN_VALUE_NAME: PCWSTR = w!("fc-host");

/// 真正的開機自啟登錄（`crate::widgets::AutostartRegistry` 的正式實作）：**只**讀寫
/// `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\fc-host`，永不碰
/// `...\Explorer\StartupApproved\Run`（使用者在工作管理員的啟用／停用由系統自己管）。
pub struct HkcuRunRegistry;

impl crate::widgets::AutostartRegistry for HkcuRunRegistry {
    fn set_run_value(&self, command: &str) -> Result<(), String> {
        let wide: Vec<u16> = command.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: `wide` 是以 NUL 結尾的 UTF-16 緩衝區，`cbdata` 為其位元組長度（含結尾 NUL，
        // REG_SZ 的要求）；機碼與值名都是靜態寬字串常數。
        let status = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_SUBKEY,
                RUN_VALUE_NAME,
                REG_SZ.0,
                Some(wide.as_ptr() as *const core::ffi::c_void),
                (wide.len() * size_of::<u16>()) as u32,
            )
        };
        if status == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(format!("RegSetKeyValueW 失敗：{status:?}"))
        }
    }

    fn delete_run_value(&self) -> Result<(), String> {
        // SAFETY: 機碼與值名都是靜態寬字串常數，無其他前置條件。
        let status = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_SUBKEY, RUN_VALUE_NAME) };
        if status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(format!("RegDeleteKeyValueW 失敗：{status:?}"))
        }
    }

    fn run_value_registered(&self) -> Result<bool, String> {
        // 只問存在與否：不給緩衝區（`pvdata`／`pcbdata` 皆 None），`RRF_RT_ANY` 不限型別——
        // 安裝檔寫成 REG_SZ 或 REG_EXPAND_SZ 都算已登錄。加 `RRF_NOEXPAND`：不展開
        // REG_EXPAND_SZ 的環境變數，「只問存在」不必展開，也避免沒有緩衝區時展開路徑回傳
        // 非 ERROR_SUCCESS 而被誤當成讀取失敗（review autostart-installer L2）。
        // SAFETY: 機碼與值名都是靜態寬字串常數；不傳任何輸出緩衝區，`RegGetValueW` 不寫入
        // 呼叫端記憶體。
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                RUN_SUBKEY,
                RUN_VALUE_NAME,
                RRF_RT_ANY | RRF_NOEXPAND,
                None,
                None,
                None,
            )
        };
        if status == ERROR_SUCCESS {
            Ok(true)
        } else if status == ERROR_FILE_NOT_FOUND {
            Ok(false)
        } else {
            Err(format!("RegGetValueW 失敗：{status:?}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moving_rect_ltrb_converts_to_physical_rect_including_negative_origin() {
        // task 7.5：WM_MOVING 的 RECT → PhysicalRect；左側副螢幕座標為負。
        assert_eq!(
            physical_rect_from_ltrb(-1500, -20, -700, 280),
            PhysicalRect {
                x: -1500,
                y: -20,
                width: 800,
                height: 300
            }
        );
        // 不合法（right < left）夾成 0，不產生負尺寸。
        assert_eq!(physical_rect_from_ltrb(10, 10, 5, 5).width, 0);
        assert_eq!(physical_rect_from_ltrb(10, 10, 5, 5).height, 0);
    }

    #[test]
    fn physical_rect_round_trips_through_win32_rect() {
        // fix drag-dpi：改寫 WM_MOVING／WM_DPICHANGED 的 RECT 時用的反向轉換。
        let r = PhysicalRect {
            x: -1500,
            y: -20,
            width: 1280,
            height: 391,
        };
        let w = rect_from_physical(r);
        assert_eq!((w.left, w.top, w.right, w.bottom), (-1500, -20, -220, 371));
        assert_eq!(physical_rect_from_ltrb(w.left, w.top, w.right, w.bottom), r);
    }

    #[test]
    fn dpi_answer_only_while_applying_own_drag_rect() {
        // fix F6（review dragdpi medium）：只有宿主自己的 apply_drag_rect 進行中才回答
        // WM_GETDPISCALEDSIZE／改寫 WM_DPICHANGED；系統自行觸發的 DPI 變更（例如使用者改
        // 縮放比例）此時不在套用中，必須回 None、照系統預設。
        let r = PhysicalRect {
            x: 100,
            y: 200,
            width: 1280,
            height: 435,
        };
        assert_eq!(applying_drag_rect(), None, "沒有套用時不介入");
        let inside = with_applying_drag_rect(r, applying_drag_rect);
        assert_eq!(inside, Some(r), "套用中回答正在套用的矩形");
        assert_eq!(applying_drag_rect(), None, "套用結束後旗標清掉");
    }

    #[test]
    fn nested_apply_restores_outer_drag_rect() {
        // WM_DPICHANGED 轉交後（Windows 10 路徑）會在外層套用中再呼叫一次 apply_drag_rect；
        // 內層結束要還原外層的矩形，不能提早清成 None。
        let outer = PhysicalRect {
            x: 0,
            y: 0,
            width: 800,
            height: 300,
        };
        let inner = PhysicalRect {
            x: 10,
            y: 10,
            width: 900,
            height: 330,
        };
        let (in_inner, after_inner) = with_applying_drag_rect(outer, || {
            let in_inner = with_applying_drag_rect(inner, applying_drag_rect);
            (in_inner, applying_drag_rect())
        });
        assert_eq!(in_inner, Some(inner));
        assert_eq!(after_inner, Some(outer));
        assert_eq!(applying_drag_rect(), None);
    }

    #[test]
    fn wmsz_maps_to_dragged_edges() {
        // task 7.6：WM_SIZING 的 wParam（Microsoft Learn「WM_SIZING」：WMSZ_*）→ 被拖邊。
        use crate::layout::ResizeEdges;
        let cases = [
            (WMSZ_LEFT, ResizeEdges::LEFT),
            (WMSZ_RIGHT, ResizeEdges::RIGHT),
            (WMSZ_TOP, ResizeEdges::TOP),
            (WMSZ_BOTTOM, ResizeEdges::BOTTOM),
            (WMSZ_TOPLEFT, ResizeEdges::TOP_LEFT),
            (WMSZ_TOPRIGHT, ResizeEdges::TOP_RIGHT),
            (WMSZ_BOTTOMLEFT, ResizeEdges::BOTTOM_LEFT),
            (WMSZ_BOTTOMRIGHT, ResizeEdges::BOTTOM_RIGHT),
        ];
        for (wmsz, expected) in cases {
            assert_eq!(resize_edges_from_wmsz(wmsz), Some(expected), "WMSZ {wmsz}");
        }
        // 文件以外的值（0、9 以上）不猜，回傳 None。
        assert_eq!(resize_edges_from_wmsz(0), None);
        assert_eq!(resize_edges_from_wmsz(9), None);
    }

    #[test]
    fn widget_style_sizebox_toggles_only_that_bit() {
        // task 7.6（design.md D7 退路）：只切 WS_SIZEBOX（＝WS_THICKFRAME），其餘位元不動；
        // 已是目標值時結果不變（呼叫端據此略過寫入）。task 7.7 起 WS_MAXIMIZEBOX 一律拿掉
        // （見下一個測試），這裡的基準樣式先去掉它，只看 SIZEBOX 這一位。
        let base = 0x16CF_0000_u32 & !WS_SIZEBOX.0 & !WS_MAXIMIZEBOX.0;
        let on = widget_style_with_sizebox(base, true);
        assert_eq!(on, base | WS_SIZEBOX.0);
        assert_eq!(widget_style_with_sizebox(on, true), on);
        assert_eq!(widget_style_with_sizebox(on, false), base);
        assert_eq!(widget_style_with_sizebox(base, false), base);
    }

    #[test]
    fn widget_style_never_keeps_maximize_box() {
        // task 7.7（Aero Snap 實測，design.md D7「調整大小的機制」）：WS_THICKFRAME＋
        // WS_MAXIMIZEBOX 時，拖到螢幕頂端會被系統最大化、上下緣拖到螢幕邊會被垂直吸附
        // （IsWindowArranged）。切換樣式時一律拿掉 WS_MAXIMIZEBOX，進出編輯版面都一樣。
        let tao_default = 0x14CB_0000_u32; // tao 小工具視窗實測樣式：含 WS_MAXIMIZEBOX、無 SIZEBOX
        assert_ne!(
            tao_default & WS_MAXIMIZEBOX.0,
            0,
            "前提：實測樣式含 WS_MAXIMIZEBOX"
        );
        let on = widget_style_with_sizebox(tao_default, true);
        assert_eq!(on, (tao_default | WS_SIZEBOX.0) & !WS_MAXIMIZEBOX.0);
        let off = widget_style_with_sizebox(on, false);
        assert_eq!(off, tao_default & !WS_MAXIMIZEBOX.0);
        assert_eq!(
            widget_style_with_sizebox(tao_default, false) & WS_MAXIMIZEBOX.0,
            0
        );
    }

    fn monitor(name: &str, work_area: (i32, i32, i32, i32), scale_factor: f64) -> MonitorSnapshot {
        MonitorSnapshot {
            name: Some(name.to_string()),
            work_area: PhysicalRect {
                x: work_area.0,
                y: work_area.1,
                width: work_area.2,
                height: work_area.3,
            },
            scale_factor,
        }
    }

    fn nameless_monitor() -> MonitorSnapshot {
        MonitorSnapshot {
            name: None,
            work_area: PhysicalRect {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            },
            scale_factor: 1.0,
        }
    }

    // ── 正常情況：查表命中，主螢幕依 name 比對標記 ─────────────────────────────

    #[test]
    fn maps_name_to_device_path_and_marks_primary() {
        let primary = monitor("\\\\.\\DISPLAY1", (0, 0, 1920, 1040), 1.0);
        let secondary = monitor("\\\\.\\DISPLAY2", (-3840, -60, 3840, 2140), 1.25);
        let monitors = [primary.clone(), secondary.clone()];

        let mut device_paths = HashMap::new();
        device_paths.insert(
            "\\\\.\\DISPLAY1".to_string(),
            "\\\\?\\DISPLAY#PRIMARY#{guid-1}".to_string(),
        );
        device_paths.insert(
            "\\\\.\\DISPLAY2".to_string(),
            "\\\\?\\DISPLAY#SECONDARY#{guid-2}".to_string(),
        );

        let result = monitors_from_tauri(&monitors, Some(&primary), &device_paths);

        assert_eq!(result.len(), 2);

        let first = &result[0];
        assert_eq!(
            first.id,
            Some(MonitorId::Device(
                "\\\\?\\DISPLAY#PRIMARY#{guid-1}".to_string()
            ))
        );
        assert!(first.is_primary);
        assert_eq!(first.work_area.width, 1920);
        assert_eq!(first.work_area.height, 1040);
        assert_eq!(first.scale_factor, 1.0);

        let second = &result[1];
        assert_eq!(
            second.id,
            Some(MonitorId::Device(
                "\\\\?\\DISPLAY#SECONDARY#{guid-2}".to_string()
            ))
        );
        assert!(!second.is_primary, "非主螢幕不應被標記");
        assert_eq!(second.work_area.x, -3840);
        assert_eq!(second.work_area.y, -60);
        assert_eq!(second.scale_factor, 1.25);
    }

    // ── 查表失敗：用 name 本身暫代，不讓顯示器從清單消失 ─────────────────────────

    #[test]
    fn lookup_miss_keeps_monitor_but_leaves_id_unresolved() {
        let m = monitor("\\\\.\\DISPLAY3", (0, 0, 2560, 1600), 1.0);
        let device_paths = HashMap::new(); // 刻意空表：模擬查表失敗／競態下查不到。

        let result = monitors_from_tauri(std::slice::from_ref(&m), Some(&m), &device_paths);

        assert_eq!(result.len(), 1, "查不到 device path 仍要保留這台顯示器");
        assert_eq!(
            result[0].id, None,
            "查不到時為未解析，不得以 DISPLAYn 暫代（design.md D9）"
        );
        assert!(result[0].is_primary, "未解析者仍可作為主螢幕退回目標");
    }

    // ── fix monitor-id：查不到穩定識別的顯示器要能點名（供 warn 記錄寫出 GDI 名稱）──────

    #[test]
    fn unresolved_monitor_names_lists_only_lookup_misses_with_their_gdi_name() {
        let hit = monitor("\\\\.\\DISPLAY1", (3840, 4, 2560, 1516), 1.75);
        let miss = monitor("\\\\.\\DISPLAY5", (0, 0, 3840, 2088), 1.5);
        let gone = MonitorSnapshot {
            name: None,
            ..hit.clone()
        };
        let mut device_paths = HashMap::new();
        device_paths.insert(
            "\\\\.\\DISPLAY1".to_string(),
            "\\\\?\\DISPLAY#BOE0CDF#{guid}".to_string(),
        );

        let monitors = [hit, miss, gone];
        let names = unresolved_monitor_names(&monitors, &device_paths);

        assert_eq!(names, vec!["\\\\.\\DISPLAY5"]);
    }

    #[test]
    fn unresolved_monitor_names_is_empty_when_every_monitor_resolves() {
        let a = monitor("\\\\.\\DISPLAY5", (0, 0, 3840, 2088), 1.5);
        let mut device_paths = HashMap::new();
        device_paths.insert(
            "\\\\.\\DISPLAY5".to_string(),
            "\\\\?\\DISPLAY#AUSAA34#{guid}".to_string(),
        );
        assert!(unresolved_monitor_names(std::slice::from_ref(&a), &device_paths).is_empty());
    }

    // ── fix round 1（Codex 2.5）：查表失敗 → 拖曳保存 → 查詢恢復 → 重新編號 的流程 ───────

    #[test]
    fn lookup_miss_must_not_yield_display_n_as_saved_monitor() {
        let m = monitor("\\\\.\\DISPLAY3", (0, 0, 2560, 1600), 1.0);
        let infos = monitors_from_tauri(std::slice::from_ref(&m), Some(&m), &HashMap::new());
        let rect = PhysicalRect {
            x: 16,
            y: 16,
            width: 100,
            height: 50,
        };
        assert_eq!(
            crate::layout::placement_after_move(&infos, rect, 16, 9),
            None,
            "查表失敗時拖曳結束不產生可保存的 placement（呼叫端保留原設定）"
        );
    }

    /// task 7.2：同一流程改用格線模型（`placement_after_move` 寫回 → `resolve_grid_placements`
    /// 推導）。
    #[test]
    fn saved_device_placement_survives_lookup_failure_recovery_and_renumbering() {
        use crate::layout::{
            placement_after_move, resolve_grid_placements, GridWidgetInput, ResolvedWidgetPlacement,
        };
        use crate::settings::WidgetPlacement;

        let resolve_one = |infos: &[crate::layout::MonitorInfo], saved: &WidgetPlacement| {
            let input = GridWidgetInput {
                id: "clock",
                monitor: saved.monitor.clone(),
                record_rect: saved.grid_rect(),
                zoom_box: crate::widgets::widget_spec("clock")
                    .expect("時鐘規格")
                    .zoom_box,
                font_scale: 1.0,
            };
            match resolve_grid_placements(infos, &[input])[0] {
                ResolvedWidgetPlacement::Placed { physical_rect, .. } => physical_rect,
                ResolvedWidgetPlacement::HiddenNoSpace => panic!("單一小工具不應空間不足"),
            }
        };
        let laptop_path = "\\\\?\\DISPLAY#LAPTOP#{guid-a}".to_string();
        let external_path = "\\\\?\\DISPLAY#EXTERNAL#{guid-b}".to_string();

        // ① 正常：筆電面板＝DISPLAY1（主）、外接＝DISPLAY2。使用者把小工具拖到外接螢幕。
        let laptop1 = monitor("\\\\.\\DISPLAY1", (0, 0, 2560, 1560), 1.5);
        let external2 = monitor("\\\\.\\DISPLAY2", (2560, 0, 3840, 2100), 1.5);
        let table1: HashMap<_, _> = [
            ("\\\\.\\DISPLAY1".to_string(), laptop_path.clone()),
            ("\\\\.\\DISPLAY2".to_string(), external_path.clone()),
        ]
        .into();
        let infos1 = monitors_from_tauri(
            &[laptop1.clone(), external2.clone()],
            Some(&laptop1),
            &table1,
        );
        let dragged = PhysicalRect {
            x: 2560 + 240,
            y: 88,
            width: 1280,
            height: 394,
        };
        let saved = placement_after_move(&infos1, dragged, 16, 9).expect("識別已解析");
        assert_eq!(saved.monitor, MonitorId::Device(external_path.clone()));
        let rect_before = resolve_one(&infos1, &saved);
        assert!(rect_before.x >= 2560, "放在外接螢幕上：{rect_before:?}");

        // ② 查詢失敗（空表）：外接螢幕仍在清單但未解析 → 小工具暫時退回主螢幕，saved 不變。
        let infos_fail = monitors_from_tauri(
            &[laptop1.clone(), external2.clone()],
            Some(&laptop1),
            &HashMap::new(),
        );
        assert!(infos_fail.iter().all(|m| m.id.is_none()));
        let rect_fail = resolve_one(&infos_fail, &saved);
        assert!(
            rect_fail.x < 2560,
            "未解析時暫時顯示在主螢幕：{rect_fail:?}"
        );

        // ③ 查詢恢復：回到原位。
        let infos_ok = monitors_from_tauri(&[laptop1.clone(), external2], Some(&laptop1), &table1);
        assert_eq!(resolve_one(&infos_ok, &saved), rect_before);

        // ④ Windows 重新編號：外接變 DISPLAY1、筆電變 DISPLAY2（座標不變）。saved 仍對到外接。
        let external1 = monitor("\\\\.\\DISPLAY1", (2560, 0, 3840, 2100), 1.5);
        let laptop2 = monitor("\\\\.\\DISPLAY2", (0, 0, 2560, 1560), 1.5);
        let table2: HashMap<_, _> = [
            ("\\\\.\\DISPLAY1".to_string(), external_path.clone()),
            ("\\\\.\\DISPLAY2".to_string(), laptop_path),
        ]
        .into();
        let infos2 = monitors_from_tauri(&[external1, laptop2.clone()], Some(&laptop2), &table2);
        assert_eq!(
            resolve_one(&infos2, &saved),
            rect_before,
            "重新編號後 placement 仍對到同一台外接螢幕"
        );
    }

    // ── 沒有 name 的項目（顯示器已消失）直接略過 ────────────────────────────────

    #[test]
    fn skips_monitors_without_a_name() {
        let named = monitor("\\\\.\\DISPLAY1", (0, 0, 1920, 1040), 1.0);
        let monitors = [nameless_monitor(), named];

        let result = monitors_from_tauri(&monitors, None, &HashMap::new());

        assert_eq!(result.len(), 1, "無 name 的項目應被略過");
        // 空表＝未解析；此處只驗證「有 name 的那台留下、無 name 的略過」。
        assert_eq!(result[0].id, None);
        assert_eq!(result[0].work_area.width, 1920);
    }

    // ── 沒有主螢幕資訊時，沒有任何一台被標記為主螢幕 ──────────────────────────────

    #[test]
    fn no_primary_marked_when_primary_monitor_is_none() {
        let m = monitor("\\\\.\\DISPLAY1", (0, 0, 1920, 1040), 1.0);
        let result = monitors_from_tauri(&[m], None, &HashMap::new());
        assert!(!result[0].is_primary);
    }

    // ── 主螢幕的 name 對不到清單中任何一台時，同樣沒有任何一台被標記 ───────────────

    #[test]
    fn no_primary_marked_when_primary_name_does_not_match_any_monitor() {
        let m = monitor("\\\\.\\DISPLAY1", (0, 0, 1920, 1040), 1.0);
        let stale_primary = monitor("\\\\.\\DISPLAY9", (0, 0, 1, 1), 1.0);

        let result = monitors_from_tauri(&[m], Some(&stale_primary), &HashMap::new());
        assert!(!result[0].is_primary);
    }
}

/// task 5.5：自動暫停偵測（design.md D12）的判定純函式。
#[cfg(test)]
mod pause_signal_tests {
    use super::{
        display_off_from_value, end_session_record, on_battery_from_ac_line_status,
        on_battery_from_power_condition, power_saving_from_value, quns_indicates_busy,
        session_change_pause_signal, session_locked_from_flags,
    };
    use windows::Win32::UI::Shell::{
        QUNS_ACCEPTS_NOTIFICATIONS, QUNS_APP, QUNS_BUSY, QUNS_NOT_PRESENT, QUNS_PRESENTATION_MODE,
        QUNS_QUIET_TIME, QUNS_RUNNING_D3D_FULL_SCREEN,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        ENDSESSION_CLOSEAPP, ENDSESSION_LOGOFF, WM_ENDSESSION, WM_QUERYENDSESSION, WM_TIMER,
        WTS_SESSION_LOCK, WTS_SESSION_UNLOCK,
    };

    // ── quns_indicates_busy：design.md D12 明列的三個值 ─────────────────────────

    #[test]
    fn quns_busy_full_screen_and_presentation_pause() {
        assert!(quns_indicates_busy(QUNS_BUSY));
        assert!(quns_indicates_busy(QUNS_RUNNING_D3D_FULL_SCREEN));
        assert!(quns_indicates_busy(QUNS_PRESENTATION_MODE));
    }

    #[test]
    fn other_quns_values_do_not_pause() {
        for state in [
            QUNS_NOT_PRESENT,
            QUNS_ACCEPTS_NOTIFICATIONS,
            QUNS_QUIET_TIME,
            QUNS_APP,
        ] {
            assert!(!quns_indicates_busy(state), "{state:?} 不應觸發暫停");
        }
    }

    // ── display_off_from_value：MONITOR_DISPLAY_STATE ──────────────────────────

    #[test]
    fn display_off_only_when_fully_off() {
        assert!(display_off_from_value(0), "PowerMonitorOff");
        assert!(!display_off_from_value(1), "PowerMonitorOn 不算關閉");
        assert!(
            !display_off_from_value(2),
            "PowerMonitorDim 畫面仍可見，不算關閉"
        );
    }

    // ── on_battery_from_power_condition：SYSTEM_POWER_CONDITION（通知路徑）────────

    #[test]
    fn on_battery_only_for_podc() {
        assert!(!on_battery_from_power_condition(0), "PoAc");
        assert!(on_battery_from_power_condition(1), "PoDc");
        assert!(
            !on_battery_from_power_condition(2),
            "PoHot（UPS 短期備援）不算使用電池"
        );
    }

    // ── on_battery_from_ac_line_status：GetSystemPowerStatus（啟動查詢路徑）───────

    #[test]
    fn ac_line_status_maps_to_battery_state() {
        assert_eq!(on_battery_from_ac_line_status(0), Some(true), "離線＝電池");
        assert_eq!(on_battery_from_ac_line_status(1), Some(false), "上線＝AC");
        assert_eq!(on_battery_from_ac_line_status(255), None, "未知");
        assert_eq!(
            on_battery_from_ac_line_status(200),
            None,
            "文件未定義的其他值視為未知"
        );
    }

    // ── power_saving_from_value ──────────────────────────────────────────────

    #[test]
    fn power_saving_is_boolean_flag() {
        assert!(!power_saving_from_value(0));
        assert!(power_saving_from_value(1));
        // 只要非 0 都算開啟（Microsoft Learn 只定義 0／1，但保守起見不把其餘值當成關閉）。
        assert!(power_saving_from_value(2));
    }

    // ── session_locked_from_flags：WTSINFOEX_LEVEL1.SessionFlags（啟動查詢路徑）───

    #[test]
    fn session_flags_lock_and_unlock() {
        assert_eq!(
            session_locked_from_flags(0),
            Some(true),
            "WTS_SESSIONSTATE_LOCK"
        );
        assert_eq!(
            session_locked_from_flags(1),
            Some(false),
            "WTS_SESSIONSTATE_UNLOCK"
        );
    }

    #[test]
    fn session_flags_unknown_is_none() {
        assert_eq!(
            session_locked_from_flags(-1),
            None,
            "WTS_SESSIONSTATE_UNKNOWN(0xFFFFFFFF，作為 i32 是 -1)"
        );
        assert_eq!(session_locked_from_flags(7), None, "未定義的其他值");
    }

    // ── session_change_pause_signal：WM_WTSSESSION_CHANGE wParam（通知路徑）──────

    #[test]
    fn session_change_lock_and_unlock() {
        assert_eq!(
            session_change_pause_signal(WTS_SESSION_LOCK as usize),
            Some(true)
        );
        assert_eq!(
            session_change_pause_signal(WTS_SESSION_UNLOCK as usize),
            Some(false)
        );
    }

    #[test]
    fn session_change_ignores_unrelated_events() {
        // WTS_CONSOLE_CONNECT(1)／WTS_SESSION_LOGON(5)／WTS_SESSION_LOGOFF(6) 等——與鎖定無關。
        for wparam in [1usize, 2, 3, 4, 5, 6, 9, 10, 11, 15] {
            assert_eq!(
                session_change_pause_signal(wparam),
                None,
                "wParam={wparam} 應與鎖定狀態無關"
            );
        }
    }

    // ── end_session_record：WM_QUERYENDSESSION／WM_ENDSESSION（fix F5）──────────────

    #[test]
    fn end_session_query_from_restart_manager_names_closeapp() {
        let line = end_session_record(WM_QUERYENDSESSION, 0, ENDSESSION_CLOSEAPP as isize)
            .expect("WM_QUERYENDSESSION 應記錄");
        assert!(line.contains("WM_QUERYENDSESSION"), "{line}");
        assert!(line.contains("lParam=0x00000001"), "{line}");
        assert!(line.contains("ENDSESSION_CLOSEAPP"), "{line}");
    }

    #[test]
    fn end_session_records_ending_flag_and_logoff() {
        let line = end_session_record(WM_ENDSESSION, 1, ENDSESSION_LOGOFF as i32 as isize)
            .expect("WM_ENDSESSION 應記錄");
        assert!(line.contains("WM_ENDSESSION"), "{line}");
        assert!(line.contains("ending=true"), "{line}");
        assert!(line.contains("lParam=0x80000000"), "{line}");
        assert!(line.contains("ENDSESSION_LOGOFF"), "{line}");
        assert!(!line.contains("ENDSESSION_CLOSEAPP"), "{line}");

        let cancelled = end_session_record(WM_ENDSESSION, 0, 0).expect("WM_ENDSESSION 應記錄");
        assert!(cancelled.contains("ending=false"), "{cancelled}");
    }

    #[test]
    fn end_session_zero_flags_means_shutdown_or_restart() {
        let line = end_session_record(WM_QUERYENDSESSION, 0, 0).expect("應記錄");
        assert!(line.contains("關機或重新開機"), "{line}");
    }

    #[test]
    fn end_session_record_ignores_other_messages() {
        assert_eq!(end_session_record(WM_TIMER, 0, 1), None);
    }

    // ── session_end_event：桌布協調迴圈的工作階段結束通知（dynamic-wallpaper 4.7b）──────

    #[test]
    fn session_end_messages_map_to_ending_and_cancelled() {
        use super::{session_end_event, SystemEvent};
        assert_eq!(
            session_end_event(WM_QUERYENDSESSION, 0),
            Some(SystemEvent::SessionEnding)
        );
        assert_eq!(
            session_end_event(WM_ENDSESSION, 1),
            Some(SystemEvent::SessionEnding)
        );
        assert_eq!(
            session_end_event(WM_ENDSESSION, 0),
            Some(SystemEvent::SessionEndCancelled),
            "wParam 0＝有程式否決，工作階段不結束"
        );
        assert_eq!(session_end_event(WM_TIMER, 1), None);
    }
}

/// task 2.8：毛玻璃啟用判定純函式（design.md D8）的先寫測試——此時 `resolve_appearance`／
/// `AcrylicConditions`／`MIN_BUILD_FOR_SYSTEM_BACKDROP` 尚未實作，本模組應編譯失敗（RED），
/// 待下一步加入實作後轉綠（GREEN），報告會附兩者的 `cargo test` 輸出。
#[cfg(test)]
mod acrylic_tests {
    use super::{resolve_appearance, AcrylicConditions, MIN_BUILD_FOR_SYSTEM_BACKDROP};
    use crate::settings::AppearanceMode;

    /// 五個條件全部滿足的基準值：`build_number` 取最低支援值本身（邊界值，見
    /// `build_number_at_minimum_is_sufficient`）、透明效果開、非省電模式、假設毛玻璃在未取得
    /// 焦點時仍呈現（探針 1.2 實測為不呈現、正式值是 `false`；此處設 `true` 是為了讓其餘條件
    /// 能逐一單獨翻轉驗證）、使用者選毛玻璃。
    fn all_satisfied() -> AcrylicConditions {
        AcrylicConditions {
            build_number: MIN_BUILD_FOR_SYSTEM_BACKDROP,
            transparency_enabled: true,
            power_saver_active: false,
            acrylic_works_unfocused: true,
            user_mode: AppearanceMode::Acrylic,
        }
    }

    #[test]
    fn all_conditions_satisfied_yields_acrylic() {
        assert_eq!(
            resolve_appearance(&all_satisfied()),
            AppearanceMode::Acrylic
        );
    }

    #[test]
    fn build_number_at_minimum_is_sufficient() {
        // 邊界值本身（design.md D8／Microsoft Learn 記載的最低 build）要視為「支援」，不是
        // 差一號才支援——見下一個測試 build_below_minimum_yields_solid 驗證邊界另一側。
        let conditions = AcrylicConditions {
            build_number: MIN_BUILD_FOR_SYSTEM_BACKDROP,
            ..all_satisfied()
        };
        assert_eq!(resolve_appearance(&conditions), AppearanceMode::Acrylic);
    }

    #[test]
    fn build_below_minimum_yields_solid() {
        let conditions = AcrylicConditions {
            build_number: MIN_BUILD_FOR_SYSTEM_BACKDROP - 1,
            ..all_satisfied()
        };
        assert_eq!(resolve_appearance(&conditions), AppearanceMode::Solid);
    }

    #[test]
    fn transparency_disabled_yields_solid() {
        let conditions = AcrylicConditions {
            transparency_enabled: false,
            ..all_satisfied()
        };
        assert_eq!(resolve_appearance(&conditions), AppearanceMode::Solid);
    }

    #[test]
    fn power_saver_active_yields_solid() {
        let conditions = AcrylicConditions {
            power_saver_active: true,
            ..all_satisfied()
        };
        assert_eq!(resolve_appearance(&conditions), AppearanceMode::Solid);
    }

    #[test]
    fn acrylic_not_working_unfocused_yields_solid_even_if_everything_else_satisfied() {
        let conditions = AcrylicConditions {
            acrylic_works_unfocused: false,
            ..all_satisfied()
        };
        assert_eq!(resolve_appearance(&conditions), AppearanceMode::Solid);
    }

    #[test]
    fn user_picks_solid_overrides_everything_else_satisfied() {
        let conditions = AcrylicConditions {
            user_mode: AppearanceMode::Solid,
            ..all_satisfied()
        };
        assert_eq!(resolve_appearance(&conditions), AppearanceMode::Solid);
    }

    #[test]
    fn user_picks_solid_even_when_nothing_else_satisfied() {
        // 使用者選純色時，其餘條件即使全部不滿足也不影響結果（本就該是純色）——確保判定
        // 順序沒有把使用者選擇放在某個「先被其他條件擋下」的分支之後才短路。
        let conditions = AcrylicConditions {
            build_number: 10240,
            transparency_enabled: false,
            power_saver_active: true,
            acrylic_works_unfocused: false,
            user_mode: AppearanceMode::Solid,
        };
        assert_eq!(resolve_appearance(&conditions), AppearanceMode::Solid);
    }

    #[test]
    fn multiple_conditions_unsatisfied_still_yields_solid() {
        // 非「恰好一項」不滿足的組合，確認函式不是靠計數門檻而是任何一項不滿足即純色。
        let conditions = AcrylicConditions {
            transparency_enabled: false,
            power_saver_active: true,
            ..all_satisfied()
        };
        assert_eq!(resolve_appearance(&conditions), AppearanceMode::Solid);
    }
}

/// fix round 1（Codex 2.8）：系統狀態讀取失敗不得被當成「允許毛玻璃」。
#[cfg(test)]
mod system_reading_tests {
    use super::{
        power_saver_from_status, resolve_appearance, transparency_from_registry, AcrylicConditions,
        MIN_BUILD_FOR_SYSTEM_BACKDROP,
    };
    use crate::settings::AppearanceMode;

    #[test]
    fn transparency_read_failure_is_unknown() {
        assert_eq!(transparency_from_registry(None), None);
        assert_eq!(transparency_from_registry(Some(1)), Some(true));
        assert_eq!(transparency_from_registry(Some(0)), Some(false));
    }

    #[test]
    fn power_status_failure_is_unknown() {
        assert_eq!(power_saver_from_status(None), None);
        assert_eq!(power_saver_from_status(Some(1)), Some(true));
        assert_eq!(power_saver_from_status(Some(0)), Some(false));
    }

    /// 三個系統讀值皆成功且條件滿足時的組合（其餘兩項輸入固定為「允許」）。
    fn readings(
        build: Option<u32>,
        transparency: Option<bool>,
        power_saver: Option<bool>,
    ) -> AppearanceMode {
        resolve_appearance(&AcrylicConditions::from_readings(
            build,
            transparency,
            power_saver,
            true,
            AppearanceMode::Acrylic,
        ))
    }

    #[test]
    fn all_readings_known_and_satisfied_yields_acrylic() {
        assert_eq!(
            readings(Some(MIN_BUILD_FOR_SYSTEM_BACKDROP), Some(true), Some(false)),
            AppearanceMode::Acrylic
        );
    }

    #[test]
    fn any_unknown_reading_yields_solid() {
        let ok_build = Some(MIN_BUILD_FOR_SYSTEM_BACKDROP);
        assert_eq!(
            readings(None, Some(true), Some(false)),
            AppearanceMode::Solid,
            "build 未知"
        );
        assert_eq!(
            readings(ok_build, None, Some(false)),
            AppearanceMode::Solid,
            "透明效果未知"
        );
        assert_eq!(
            readings(ok_build, Some(true), None),
            AppearanceMode::Solid,
            "省電模式未知"
        );
        assert_eq!(
            readings(None, None, None),
            AppearanceMode::Solid,
            "全部未知"
        );
    }

    #[test]
    fn from_readings_passes_through_probe_and_user_choice() {
        let c = AcrylicConditions::from_readings(
            Some(MIN_BUILD_FOR_SYSTEM_BACKDROP),
            Some(true),
            Some(false),
            false,
            AppearanceMode::Solid,
        );
        assert!(!c.acrylic_works_unfocused);
        assert_eq!(c.user_mode, AppearanceMode::Solid);
    }
}

#[cfg(test)]
mod window_style_tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::{
        WS_EX_ACCEPTFILES, WS_EX_NOACTIVATE, WS_EX_WINDOWEDGE,
    };

    /// tao 對 `focusable(false)`、無 owner 的視窗產生的延伸樣式（`window_state.rs`
    /// `to_window_styles`：WINDOWEDGE｜ACCEPTFILES｜APPWINDOW（ON_TASKBAR）｜NOACTIVATE）。
    fn tao_widget_ex_style() -> u32 {
        WS_EX_WINDOWEDGE.0 | WS_EX_ACCEPTFILES.0 | WS_EX_APPWINDOW.0 | WS_EX_NOACTIVATE.0
    }

    #[test]
    fn adds_toolwindow_and_removes_appwindow() {
        let style = widget_ex_style(tao_widget_ex_style());
        assert_ne!(style & WS_EX_TOOLWINDOW.0, 0, "須含 WS_EX_TOOLWINDOW");
        assert_eq!(style & WS_EX_APPWINDOW.0, 0, "不得含 WS_EX_APPWINDOW");
    }

    #[test]
    fn keeps_noactivate_and_other_bits() {
        let style = widget_ex_style(tao_widget_ex_style());
        assert_ne!(
            style & WS_EX_NOACTIVATE.0,
            0,
            "tao 產生的 WS_EX_NOACTIVATE 必須保留"
        );
        assert_ne!(style & WS_EX_WINDOWEDGE.0, 0);
        assert_ne!(style & WS_EX_ACCEPTFILES.0, 0);
    }

    #[test]
    fn is_idempotent() {
        let once = widget_ex_style(tao_widget_ex_style());
        assert_eq!(widget_ex_style(once), once);
    }
}

/// fix F2（review 3.5 high＋low）：重排請求的合併／延後純邏輯。
#[cfg(test)]
mod relayout_coalescer_tests {
    use super::{RelayoutCoalescer, RelayoutReason};

    #[test]
    fn first_request_of_a_wave_asks_caller_to_post_and_later_ones_merge() {
        let mut c = RelayoutCoalescer::default();
        assert!(
            c.request(RelayoutReason::DpiChanged),
            "第一個請求：呼叫端要 PostMessage 給守門視窗"
        );
        assert!(
            !c.request(RelayoutReason::DpiChanged),
            "同一波後續請求只合併，不再 Post"
        );
        assert!(!c.request(RelayoutReason::WorkArea));
        assert!(!c.request(RelayoutReason::DisplayChange));
        assert_eq!(
            c.take(),
            vec![
                RelayoutReason::DpiChanged,
                RelayoutReason::WorkArea,
                RelayoutReason::DisplayChange
            ],
            "延後執行時一次取出整波原因（去重、保留首次出現順序）"
        );
    }

    #[test]
    fn nothing_pending_means_nothing_to_run() {
        let mut c = RelayoutCoalescer::default();
        assert!(c.take().is_empty(), "沒有請求時延後回呼什麼都不做");
        c.request(RelayoutReason::WorkArea);
        c.take();
        assert!(c.take().is_empty(), "同一波只執行一次");
    }

    #[test]
    fn request_after_take_starts_a_new_wave() {
        let mut c = RelayoutCoalescer::default();
        assert!(c.request(RelayoutReason::DpiChanged));
        assert_eq!(c.take(), vec![RelayoutReason::DpiChanged]);
        assert!(
            c.request(RelayoutReason::DpiChanged),
            "執行後才到的觸發（例如 tao 套建議矩形之後的下一扇 WM_DPICHANGED）要再排一次"
        );
    }

    #[test]
    fn failed_post_can_be_withdrawn_so_the_next_request_posts_again() {
        let mut c = RelayoutCoalescer::default();
        assert!(c.request(RelayoutReason::WorkArea));
        c.take(); // PostMessage 失敗：呼叫端撤回
        assert!(c.request(RelayoutReason::WorkArea));
    }

    #[test]
    fn reason_names_are_stable_log_tokens() {
        assert_eq!(RelayoutReason::DisplayChange.as_str(), "display-change");
        assert_eq!(RelayoutReason::WorkArea.as_str(), "workarea-change");
        assert_eq!(RelayoutReason::DpiChanged.as_str(), "dpi-change");
        assert_eq!(
            RelayoutReason::ExplorerRestarted.as_str(),
            "explorer-restart"
        );
    }

    /// Codex 審 5c46c74 medium：explorer 重啟（`TaskbarCreated`）也要經重排收斂點，讓
    /// `relayout_all_widgets` → `sync_grid_overlay` 把以退路定位的格線插回桌面正上方。
    #[test]
    fn taskbar_created_requests_a_relayout() {
        let src = include_str!("desktop.rs").replace("\r\n", "\n");
        let start = src
            .find("if msg == state.taskbar_created_message {")
            .expect("TaskbarCreated 分支");
        let branch = &src[start..start + src[start..].find("} else if").expect("分支結尾")];
        assert!(
            branch.contains("request_relayout(RelayoutReason::ExplorerRestarted)"),
            "{branch}"
        );
    }
}

/// fix F2（review 5.5 medium）：`WTSRegisterSessionNotification` 失敗後的重試決策。
#[cfg(test)]
mod session_notification_retry_tests {
    use super::{session_registration_retry_step, SessionRegistrationRetry};

    #[test]
    fn success_stops_retrying() {
        assert_eq!(
            session_registration_retry_step(3, 60, true),
            SessionRegistrationRetry::Registered
        );
    }

    #[test]
    fn failure_keeps_retrying_until_the_limit() {
        assert_eq!(
            session_registration_retry_step(1, 60, false),
            SessionRegistrationRetry::RetryLater
        );
        assert_eq!(
            session_registration_retry_step(59, 60, false),
            SessionRegistrationRetry::RetryLater
        );
        assert_eq!(
            session_registration_retry_step(60, 60, false),
            SessionRegistrationRetry::GiveUp,
            "第 60 次仍失敗 → 放棄並記錄"
        );
    }
}

/// task 3.3：置底守門的判定純函式（design.md D8「保險檢查」）。
#[cfg(test)]
mod gatekeeper_tests {
    use super::{needs_rebottom, ZOrderWindowState};

    /// 一個「正常」的一般應用程式視窗（可見、非最小化、非 cloaked、非 tool window、非零
    /// 尺寸）；測試以 `..normal_app()` 覆寫個別欄位製造反例。
    fn normal_app() -> ZOrderWindowState {
        ZOrderWindowState {
            hwnd: 0,
            is_widget: false,
            visible: true,
            minimized: false,
            cloaked: false,
            tool_window: false,
            has_size: true,
            topmost: false,
            shell_desktop: false,
        }
    }

    /// 小工具視窗的真實旗標組合：`is_widget=true` 之外，實機的小工具一律帶
    /// `WS_EX_TOOLWINDOW`（task 3.1 `apply_widget_ex_style`），所以 `tool_window` 也要是
    /// `true`——否則測試字面量會把小工具自己誤判成「一般視窗」，測不出真正的判定邏輯
    /// （`is_normal_window` 不看 `is_widget`，只看其餘四個旗標＋尺寸）。
    fn widget() -> ZOrderWindowState {
        ZOrderWindowState {
            is_widget: true,
            tool_window: true,
            ..normal_app()
        }
    }

    #[test]
    fn no_widgets_in_snapshot_never_needs_rebottom() {
        // 使用者把十個小工具全部關閉：快照裡完全沒有 is_widget=true 的項目。
        assert!(!needs_rebottom(&[normal_app(), normal_app()]));
        assert!(!needs_rebottom(&[]));
    }

    #[test]
    fn widget_topmost_with_nothing_below_does_not_need_rebottom() {
        // 由上到下：一般視窗、小工具——小工具已經在最下面，符合 D8「小工具維持在一般視窗
        // 之下」。
        assert!(!needs_rebottom(&[normal_app(), widget()]));
    }

    #[test]
    fn normal_window_below_widget_needs_rebottom() {
        // 由上到下：小工具、一般視窗——一般視窗排在小工具之後，代表小工具蓋住了它。
        assert!(needs_rebottom(&[widget(), normal_app()]));
    }

    #[test]
    fn normal_window_above_widget_only_does_not_need_rebottom() {
        // 一般視窗全部排在小工具之上（z-order 索引更小），小工具之後沒有任何一般視窗。
        assert!(!needs_rebottom(&[normal_app(), normal_app(), widget()]));
    }

    #[test]
    fn normal_window_between_two_widgets_needs_rebottom() {
        // 由上到下：小工具 A、一般視窗、小工具 B——一般視窗排在 A 之後（被 A 蓋住），即使它
        // 排在 B 之前也算違規；「小工具之間的順序」不影響這個判定。
        assert!(needs_rebottom(&[widget(), normal_app(), widget()]));
    }

    #[test]
    fn multiple_adjacent_widgets_with_nothing_below_do_not_need_rebottom() {
        // 三個小工具相鄰、沒有任何一般視窗夾在中間或排在它們之後：不需要重排。結合
        // `normal_window_between_two_widgets_needs_rebottom`（一般視窗夾在小工具之間仍算
        // 違規）可知，真正影響結果的只有「最上層小工具之後有沒有一般視窗」，小工具彼此的
        // 相對順序／數量本身不影響判定（design.md「小工具之間的順序不列入判斷」）。
        assert!(!needs_rebottom(&[
            normal_app(),
            widget(),
            widget(),
            widget()
        ]));
    }

    #[test]
    fn minimized_window_below_widget_is_not_a_violation() {
        let minimized = ZOrderWindowState {
            minimized: true,
            ..normal_app()
        };
        assert!(!needs_rebottom(&[widget(), minimized]));
    }

    #[test]
    fn cloaked_window_below_widget_is_not_a_violation() {
        let cloaked = ZOrderWindowState {
            cloaked: true,
            ..normal_app()
        };
        assert!(!needs_rebottom(&[widget(), cloaked]));
    }

    #[test]
    fn invisible_window_below_widget_is_not_a_violation() {
        let invisible = ZOrderWindowState {
            visible: false,
            ..normal_app()
        };
        assert!(!needs_rebottom(&[widget(), invisible]));
    }

    #[test]
    fn tool_window_below_widget_is_not_a_violation() {
        // 例如另一個置底程式或浮動面板：不是「一般應用程式視窗」，不算違規
        // （host/tools/watch-zorder.ps1 的「一般視窗」判準第 4 條）。
        let tool = ZOrderWindowState {
            tool_window: true,
            ..normal_app()
        };
        assert!(!needs_rebottom(&[widget(), tool]));
    }

    #[test]
    fn zero_size_window_below_widget_is_not_a_violation() {
        // 隱藏用的 0×0 訊息代理視窗，不算「一般視窗」。
        let zero_size = ZOrderWindowState {
            has_size: false,
            ..normal_app()
        };
        assert!(!needs_rebottom(&[widget(), zero_size]));
    }

    #[test]
    fn multiple_normal_windows_only_need_one_violation_to_trigger() {
        assert!(needs_rebottom(&[
            widget(),
            normal_app(),
            normal_app(),
            normal_app()
        ]));
    }
}

#[cfg(test)]
mod show_desktop_tests {
    //! task 3.4：Win+D 判準、動作決策、插入位置、事件過濾的純函式測試。快照情境取自探針 1.1
    //! 的實測 z-order（`.superpowers/sdd/tasks/task-1.1-report.md`）。
    use super::{
        classify_top, decide_show_desktop, insert_target, is_relevant_win_event,
        is_shell_desktop_window, InsertTarget, ShowDesktopAction, ZOrderTop, ZOrderWindowState,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EVENT_OBJECT_CLOAKED, EVENT_OBJECT_FOCUS, EVENT_OBJECT_HIDE, EVENT_OBJECT_REORDER,
        EVENT_OBJECT_SHOW, EVENT_OBJECT_UNCLOAKED, EVENT_SYSTEM_FOREGROUND,
        EVENT_SYSTEM_MINIMIZEEND, EVENT_SYSTEM_MINIMIZESTART, OBJID_CARET, OBJID_WINDOW,
    };

    fn app(hwnd: isize) -> ZOrderWindowState {
        ZOrderWindowState {
            hwnd,
            is_widget: false,
            visible: true,
            minimized: false,
            cloaked: false,
            tool_window: false,
            has_size: true,
            topmost: false,
            shell_desktop: false,
        }
    }

    fn minimized(hwnd: isize) -> ZOrderWindowState {
        ZOrderWindowState {
            minimized: true,
            ..app(hwnd)
        }
    }

    /// 小工具：實機一律帶 `WS_EX_TOOLWINDOW`（task 3.1）。
    fn widget(hwnd: isize) -> ZOrderWindowState {
        ZOrderWindowState {
            is_widget: true,
            tool_window: true,
            ..app(hwnd)
        }
    }

    /// 殼層的 `Progman`：本機實測帶 `WS_EX_TOOLWINDOW`（task-1.1-report 疑慮 1）。
    fn progman(hwnd: isize) -> ZOrderWindowState {
        ZOrderWindowState {
            tool_window: true,
            shell_desktop: true,
            ..app(hwnd)
        }
    }

    /// 工作列：topmost、可見。
    fn taskbar(hwnd: isize) -> ZOrderWindowState {
        ZOrderWindowState {
            topmost: true,
            tool_window: true,
            ..app(hwnd)
        }
    }

    /// 探針 1.1 實測 `GetWindow(Progman, GW_HWNDPREV)` 得到的不可見 `IME` 視窗。
    fn ime(hwnd: isize) -> ZOrderWindowState {
        ZOrderWindowState {
            visible: false,
            ..app(hwnd)
        }
    }

    const TASKBAR: isize = 0x10;
    const NOTEPAD: isize = 0x20;
    const EDGE: isize = 0x21;
    const IME: isize = 0x10C26;
    const PROGMAN: isize = 0x1028C;
    const W1: isize = 0x90CD2;
    const W2: isize = 0x90CD4;

    /// 平常狀態：一般視窗在上，小工具緊貼在 `Progman` 之上（1.1 冒煙測試：
    /// Tauri Window 之後就是 Progman）。
    fn normal_state() -> Vec<ZOrderWindowState> {
        vec![
            taskbar(TASKBAR),
            app(NOTEPAD),
            app(EDGE),
            ime(IME),
            widget(W1),
            progman(PROGMAN),
        ]
    }

    /// 按下 Win+D 之後、小工具還沒插入時：一般視窗被最小化，`Progman` 被抬到非 topmost 頂端
    /// （1.1 run2 11:12:29.774：Progman above 11→3）。
    fn after_win_d() -> Vec<ZOrderWindowState> {
        vec![
            taskbar(TASKBAR),
            ime(IME),
            progman(PROGMAN),
            minimized(NOTEPAD),
            minimized(EDGE),
            widget(W1),
        ]
    }

    /// 小工具已插在 `Progman` 正上方。
    fn inserted() -> Vec<ZOrderWindowState> {
        vec![
            taskbar(TASKBAR),
            ime(IME),
            widget(W1),
            progman(PROGMAN),
            minimized(NOTEPAD),
            minimized(EDGE),
        ]
    }

    // ── 判準：是否處於顯示桌面 ─────────────────────────────────────────────────

    #[test]
    fn normal_state_top_is_app() {
        assert_eq!(classify_top(&normal_state()), ZOrderTop::App(1));
    }

    #[test]
    fn after_win_d_top_is_desktop() {
        assert_eq!(classify_top(&after_win_d()), ZOrderTop::Desktop(2));
    }

    #[test]
    fn clicking_desktop_does_not_count_as_show_desktop() {
        // 1.1 情境 C：點擊桌面空白處只讓 Progman 成為前景、z-order 不變。判準不看前景，快照
        // 與平常狀態相同，因此不進入。
        assert_eq!(
            decide_show_desktop(false, &normal_state()),
            ShowDesktopAction::None
        );
    }

    #[test]
    fn all_normal_windows_minimized_counts_as_show_desktop() {
        // design.md D8：所有一般視窗都最小化時同樣判為顯示桌面。小工具本來就在 Progman 正上方，
        // 不需要插入任何視窗。
        let snapshot = vec![
            taskbar(TASKBAR),
            minimized(NOTEPAD),
            minimized(EDGE),
            widget(W1),
            progman(PROGMAN),
        ];
        assert_eq!(classify_top(&snapshot), ZOrderTop::Desktop(4));
        assert_eq!(
            decide_show_desktop(false, &snapshot),
            ShowDesktopAction::Enter {
                desk: 4,
                insert: vec![]
            }
        );
    }

    #[test]
    fn topmost_invisible_cloaked_and_tool_windows_above_desktop_are_skipped() {
        let topmost_app = ZOrderWindowState {
            topmost: true,
            ..app(0x1)
        };
        let cloaked_app = ZOrderWindowState {
            cloaked: true,
            ..app(0x2)
        };
        let floating_tool = ZOrderWindowState {
            tool_window: true,
            ..app(0x3)
        };
        let zero_size = ZOrderWindowState {
            has_size: false,
            ..app(0x4)
        };
        let snapshot = vec![
            topmost_app,
            ime(IME),
            cloaked_app,
            floating_tool,
            zero_size,
            progman(PROGMAN),
            app(NOTEPAD),
        ];
        assert_eq!(classify_top(&snapshot), ZOrderTop::Desktop(5));
    }

    #[test]
    fn widget_above_desktop_does_not_hide_the_desktop_from_the_criterion() {
        // 顯示桌面中小工具在 Progman 之上，判準要略過小工具本身，否則會被當成「一般視窗在
        // 桌面之上」而立刻離開。
        assert_eq!(classify_top(&inserted()), ZOrderTop::Desktop(3));
    }

    #[test]
    fn empty_snapshot_is_nothing() {
        assert_eq!(classify_top(&[]), ZOrderTop::Nothing);
        assert_eq!(decide_show_desktop(true, &[]), ShowDesktopAction::None);
    }

    // ── 動作決策：進入／重插／離開 ─────────────────────────────────────────────

    #[test]
    fn enter_lists_visible_widgets_below_the_desktop() {
        let hidden_widget = ZOrderWindowState {
            visible: false,
            ..widget(W2)
        };
        let mut snapshot = after_win_d();
        snapshot.push(hidden_widget);
        assert_eq!(
            decide_show_desktop(false, &snapshot),
            ShowDesktopAction::Enter {
                desk: 2,
                insert: vec![W1]
            }
        );
    }

    #[test]
    fn stable_after_insert_does_nothing() {
        // 插入後自己觸發的 REORDER 再算一次：不得重複插入（避免來回切換）。
        assert_eq!(
            decide_show_desktop(true, &inserted()),
            ShowDesktopAction::None
        );
    }

    #[test]
    fn explorer_raising_desktop_again_triggers_reassert() {
        // 1.1：進入後 20～60 ms 內 explorer 再抬一次 Progman，小工具又落到它之下。
        assert_eq!(
            decide_show_desktop(true, &after_win_d()),
            ShowDesktopAction::Reassert {
                desk: 2,
                insert: vec![W1]
            }
        );
    }

    #[test]
    fn app_back_above_desktop_exits() {
        // 再按一次 Win+D 或切換到應用程式：一般視窗回到 Progman 之上。
        let snapshot = vec![
            taskbar(TASKBAR),
            app(NOTEPAD),
            ime(IME),
            widget(W1),
            progman(PROGMAN),
        ];
        assert_eq!(
            decide_show_desktop(true, &snapshot),
            ShowDesktopAction::Exit { app: 1 }
        );
        assert_eq!(
            decide_show_desktop(false, &snapshot),
            ShowDesktopAction::None
        );
    }

    #[test]
    fn full_win_d_cycle() {
        // 進入 → 穩定 → explorer 再抬 → 重插 → 穩定 → 離開 → 平常，逐步套用決策後的狀態。
        let mut active = false;
        let steps = [
            (after_win_d(), "enter"),
            (inserted(), "none"),
            (after_win_d(), "reassert"),
            (inserted(), "none"),
            (normal_state(), "exit"),
            (normal_state(), "none"),
        ];
        for (snapshot, expected) in steps {
            let got = match decide_show_desktop(active, &snapshot) {
                ShowDesktopAction::None => "none",
                ShowDesktopAction::Enter { .. } => {
                    active = true;
                    "enter"
                }
                ShowDesktopAction::Reassert { .. } => "reassert",
                ShowDesktopAction::Exit { .. } => {
                    active = false;
                    "exit"
                }
            };
            assert_eq!(got, expected);
        }
        assert!(!active);
    }

    // ── 插入位置 ─────────────────────────────────────────────────────────────

    #[test]
    fn insert_after_the_window_just_above_the_desktop() {
        // 1.1：GetWindow(Progman, GW_HWNDPREV) 是不可見的 IME 視窗，插在它之後＝Progman 正上方。
        let snapshot = after_win_d();
        let ZOrderTop::Desktop(desk) = classify_top(&snapshot) else {
            panic!("應判為顯示桌面");
        };
        let prev = &snapshot[desk - 1];
        assert_eq!(
            insert_target(W1, Some((prev.hwnd, prev.topmost))),
            InsertTarget::After(IME)
        );
    }

    #[test]
    fn insert_uses_hwnd_top_when_prev_is_topmost_or_missing() {
        assert_eq!(insert_target(W1, Some((TASKBAR, true))), InsertTarget::Top);
        assert_eq!(insert_target(W1, None), InsertTarget::Top);
    }

    #[test]
    fn insert_skips_when_widget_is_already_directly_above_desktop() {
        assert_eq!(
            insert_target(W1, Some((W1, false))),
            InsertTarget::AlreadyInPlace
        );
    }

    #[test]
    fn second_widget_goes_between_first_widget_and_desktop() {
        // 第一個小工具插好後，桌面的前一個視窗就是它；第二個插在它之後，同樣位於桌面之上。
        assert_eq!(
            insert_target(W2, Some((W1, false))),
            InsertTarget::After(W1)
        );
    }

    // ── 事件過濾 ─────────────────────────────────────────────────────────────

    const WIN: i32 = OBJID_WINDOW.0;

    #[test]
    fn reorder_only_counts_when_sent_by_the_desktop_window() {
        assert!(is_relevant_win_event(
            EVENT_OBJECT_REORDER,
            WIN,
            0,
            true,
            false
        ));
        assert!(!is_relevant_win_event(
            EVENT_OBJECT_REORDER,
            WIN,
            0,
            false,
            true
        ));
    }

    #[test]
    fn foreground_and_minimize_events_always_count() {
        for event in [
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_MINIMIZESTART,
            EVENT_SYSTEM_MINIMIZEEND,
        ] {
            assert!(is_relevant_win_event(event, WIN, 0, false, false));
        }
    }

    #[test]
    fn show_hide_cloak_only_count_for_top_level_windows_themselves() {
        for event in [
            EVENT_OBJECT_SHOW,
            EVENT_OBJECT_HIDE,
            EVENT_OBJECT_CLOAKED,
            EVENT_OBJECT_UNCLOAKED,
        ] {
            assert!(is_relevant_win_event(event, WIN, 0, false, true));
            // 子視窗
            assert!(!is_relevant_win_event(event, WIN, 0, false, false));
            // 非視窗物件（插入點）
            assert!(!is_relevant_win_event(event, OBJID_CARET.0, 0, false, true));
            // 子元素
            assert!(!is_relevant_win_event(event, WIN, 3, false, true));
        }
    }

    #[test]
    fn unrelated_events_are_ignored() {
        assert!(!is_relevant_win_event(
            EVENT_OBJECT_FOCUS,
            WIN,
            0,
            true,
            true
        ));
    }

    // ── 殼層桌面視窗 ─────────────────────────────────────────────────────────

    #[test]
    fn shell_desktop_requires_shell_pid_and_desktop_class() {
        assert!(is_shell_desktop_window("Progman", 5500, 5500));
        assert!(is_shell_desktop_window("WorkerW", 5500, 5500));
        // 別的行程開的同名類別（例如桌布程式）
        assert!(!is_shell_desktop_window("WorkerW", 1234, 5500));
        // 殼層沒在跑
        assert!(!is_shell_desktop_window("Progman", 0, 0));
        assert!(!is_shell_desktop_window("Shell_TrayWnd", 5500, 5500));
    }
}

/// dynamic-wallpaper task 4.7a 修正輪 1：`dpi_for_rect` 的唯讀實測（只查詢、不改任何設定）。
#[cfg(test)]
mod dpi_for_rect_tests {
    use super::dpi_for_rect;
    use crate::layout::PhysicalRect;

    #[test]
    fn rect_on_the_primary_monitor_has_a_dpi_and_offscreen_rect_has_none() {
        let on_primary = PhysicalRect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        };
        let dpi = dpi_for_rect(on_primary).expect("主螢幕原點應查得到 DPI");
        assert!((96..=96 * 5).contains(&dpi), "{dpi}");
        let far = PhysicalRect {
            x: -1_000_000,
            y: -1_000_000,
            width: 10,
            height: 10,
        };
        assert_eq!(dpi_for_rect(far), None);
        let overflow = PhysicalRect {
            x: i32::MAX,
            y: 0,
            width: 10,
            height: 10,
        };
        assert_eq!(dpi_for_rect(overflow), None);
    }
}

#[cfg(test)]
mod isolation_tests {
    use super::{
        expand_dir_path, local_app_data_mismatch, registered_local_app_data, same_directory,
        LocalAppDataMismatch,
    };

    #[test]
    fn same_directory_ignores_case_and_trailing_slashes() {
        let base = r"C:\Users\Ben\AppData\Local";
        for other in [
            r"c:\users\ben\appdata\local",
            r"C:\USERS\BEN\APPDATA\LOCAL",
            r"C:\Users\Ben\AppData\Local\",
            r"C:\Users\Ben\AppData\Local\\",
            "C:/Users/Ben/AppData/Local",
            "C:/Users/Ben/AppData/Local/",
            r"  C:\Users\Ben\AppData\Local  ",
        ] {
            assert!(same_directory(base, other), "{other:?}");
            assert!(same_directory(other, base), "{other:?}");
        }
    }

    #[test]
    fn same_directory_distinguishes_different_directories() {
        let base = r"C:\Users\Ben\AppData\Local";
        for other in [
            r"C:\Users\Ben\AppData\Local\Temp",
            r"C:\Users\Ben\AppData\LocalLow",
            r"C:\Users\Ben\AppData",
            r"D:\Users\Ben\AppData\Local",
            r"C:\Users\Bob\AppData\Local",
            "",
        ] {
            assert!(!same_directory(base, other), "{other:?}");
        }
    }

    #[test]
    fn drive_roots_compare_with_and_without_the_backslash() {
        assert!(same_directory(r"C:\", r"c:\"));
        assert!(same_directory(r"C:\", "C:"));
        assert!(!same_directory(r"C:\", r"D:\"));
    }

    #[test]
    fn mismatch_needs_both_sides_and_a_real_difference() {
        let reg = r"C:\Users\Ben\AppData\Local";
        assert_eq!(
            local_app_data_mismatch(Some(r"c:\users\ben\appdata\local\"), Some(reg)),
            None
        );
        assert_eq!(
            local_app_data_mismatch(Some(r"C:\Temp\fc-test\local"), Some(reg)),
            Some(LocalAppDataMismatch {
                env_value: r"C:\Temp\fc-test\local".to_string(),
                registered: reg.to_string(),
            })
        );
        // 無法判定（任一邊取不到）＝不算隔離。
        assert_eq!(local_app_data_mismatch(None, Some(reg)), None);
        assert_eq!(local_app_data_mismatch(Some(reg), None), None);
        assert_eq!(local_app_data_mismatch(None, None), None);
    }

    #[test]
    fn expand_dir_path_normalises_dot_dot_and_restores_case() {
        let expanded = expand_dir_path(r"c:\windows\system32\..\system32\");
        assert!(
            same_directory(&expanded, r"C:\Windows\System32"),
            "{expanded}"
        );
        assert!(!expanded.contains(".."), "{expanded}");
    }

    #[test]
    fn expand_dir_path_keeps_a_nonexistent_path_instead_of_failing() {
        let expanded = expand_dir_path(r"C:\no-such-dir-fc-host-test\x\..\y");
        assert!(
            same_directory(&expanded, r"C:\no-such-dir-fc-host-test\y"),
            "{expanded}"
        );
    }

    #[test]
    fn expand_dir_path_is_idempotent_on_the_temp_directory() {
        let temp = std::env::temp_dir();
        let once = expand_dir_path(&temp.to_string_lossy());
        assert_eq!(expand_dir_path(&once), once);
        assert!(same_directory(&once, &once));
    }

    /// 真正觸發 8.3 短檔名展開（審查 m6）：建一個名稱超過 8 字元的資料夾，用 `GetShortPathNameW` 取得它的
    /// 短名，斷言 `expand_dir_path(短名)` 還原成長名。磁碟停用 8.3 時（短名等於長名）無從觸發，跳過並說明。
    #[test]
    fn expand_dir_path_turns_a_real_8_3_short_name_back_into_the_long_name() {
        use windows::core::PCWSTR;
        use windows::Win32::Storage::FileSystem::GetShortPathNameW;

        let long_dir = std::env::temp_dir().join(format!(
            "fc-host-isolation-long-directory-name-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&long_dir).expect("建立測試資料夾");
        let long = long_dir.to_string_lossy().into_owned();
        let wide: Vec<u16> = long.encode_utf16().chain(std::iter::once(0)).collect();
        let mut buf = vec![0u16; 1024];
        // SAFETY: `wide` 以 NUL 結尾且存活；`buf` 是可寫緩衝區，API 回傳寫入的字元數（0＝失敗）。
        let n = unsafe { GetShortPathNameW(PCWSTR(wide.as_ptr()), Some(&mut buf)) } as usize;
        let short = String::from_utf16_lossy(&buf[..n.min(buf.len())]);
        if n == 0 || same_directory(&short, &long) {
            eprintln!("略過：此磁碟停用 8.3 短檔名（短名與長名相同：{short}），無法觸發展開");
        } else {
            assert!(
                !same_directory(&short, &long),
                "前提：短名 {short} 與長名 {long} 不同"
            );
            let expanded = expand_dir_path(&short);
            assert!(
                same_directory(&expanded, &long),
                "短名 {short} 應展開成長名 {long}，實際 {expanded}"
            );
            // 展開後與環境變數的長名寫法比較＝同一個目錄（隔離偵測不會把短名誤判成不同）。
            assert!(local_app_data_mismatch(Some(&expanded), Some(&long)).is_none());
        }
        let _ = std::fs::remove_dir_all(&long_dir);
    }

    #[test]
    fn registered_local_app_data_is_available_on_a_normal_session() {
        // 系統登記值取得成功（失敗＝None 會讓隔離偵測靜默失效，這裡確認 API 呼叫本身沒壞）。
        let registered = registered_local_app_data().expect("SHGetKnownFolderPath 應成功");
        assert!(
            registered.contains(std::path::MAIN_SEPARATOR),
            "{registered}"
        );
    }
}
