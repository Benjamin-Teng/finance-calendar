//! fc-host：桌面小工具宿主，進入點只做組裝。
//!
//! Goals（design.md）：核心與小工具頁面只透過通道快照查詢＋事件推送溝通；子專案 2 換資料
//! 來源、日後擴充插槽接新資料源時，核心、視窗管理與設定結構都不需修改。本檔因此保持精簡，
//! 邏輯依 design.md 的分工搬到對應模組：
//!
//! - [`settings`]：設定模型（D12，task 2.3 填入）。
//! - [`data`]：`DataSource` trait 與通道註冊表（D5，task 2.6 填入）。
//! - [`desktop`]：Win32 桌面層行為集中於此（D8），其餘模組不得直接呼叫 Win32
//!   （task 2.4、2.5、2.8、3.1–3.5 填入）。
//! - [`widgets`]：小工具視窗管理與註冊表（D1、D6、D7）；核心↔頁面 IPC 介面（D4）
//!   （task 2.7 填入；視窗工廠本身留給 task 3.1–3.2）。
//! - [`tray`]：系統匣選單與單一執行個體（D12，task 5.1）；圖示本身由 `tauri.conf.json` 的
//!   `app.trayIcon`＋`tray-icon` feature 在啟動時自動建立，不需額外程式碼。
//! - [`app_icon`]：系統匣與設定視窗圖示隨桌布主題切換（dynamic-wallpaper task 5.2，design.md
//!   D9）；唯一入口 `app_icon::request_refresh`。
//! - [`recovery`]：WebView2 故障復原（D12，task 5.6）——`ProcessFailed` 的處理決策、browser
//!   行程結束去重、卡死看門狗；實際重建走 [`widgets`] 的視窗工廠。
//! - [`logging`]：本機記錄檔（D12，task 5.8 填入）——`main()` 最開頭安裝為 `log` crate 的
//!   全域 logger，全 crate 之後一律用 `log::info!`／`warn!`／`error!` 巨集，不再 `eprintln!`。
//!
//! 本 task（2.7）把設定（2.3）與通道註冊表（2.6）接進 Tauri managed state
//! （[`widgets::AppState`]），註冊五個 IPC 指令，並啟動排程執行緒每 30 秒輪詢一次通道、有
//! 更新的通道推播資料（design.md D5）。task 3.1 起 setup 依設定建立小工具視窗
//! （`widgets::sync_widget_windows`），頁面以 `subscribe_data` 成為推播收件人；IPC 本身的
//! 驗收仍以 [`self_test_ipc`]（`self-test-ipc` cargo feature）自建測試視窗進行。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app_icon;
mod data;
mod desktop;
mod fetch;
mod layout;
mod logging;
#[cfg(feature = "probe-render")]
mod probe_render;
mod recovery;
#[cfg(feature = "self-test-ipc")]
mod self_test_ipc;
#[cfg(feature = "self-test-ipc")]
mod self_test_render;
mod settings;
mod tray;
mod updater;
mod wallpaper;
mod wallpaper_cli;
mod wallpaper_config;
mod wallpaper_coordinator;
mod wallpaper_dnd;
mod wallpaper_render;
mod wallpaper_settings;
mod wallpaper_state;
mod webview_env;
mod widgets;

#[cfg(feature = "self-test-ipc")]
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use tauri::Manager;

use widgets::AppState;

/// 排程輪詢間隔（design.md D5：「每 30 秒比對」）。
const POLL_INTERVAL: Duration = Duration::from_secs(30);

/// `--self-test-ipc` 旗標與其參數（`self_test_ipc.rs` 模組文件「用法」）。只在
/// `self-test-ipc` cargo feature 開啟時才解析與生效，正式 release build（不加 `--features`）
/// 這段程式碼根本不會被編譯進去。
#[cfg(feature = "self-test-ipc")]
struct SelfTestIpcArgs {
    log: PathBuf,
    timeout_secs: u64,
}

#[cfg(feature = "self-test-ipc")]
fn parse_self_test_ipc_args() -> Option<SelfTestIpcArgs> {
    let mut present = false;
    let mut log = PathBuf::from("self_test_ipc.log");
    let mut timeout_secs: u64 = 10;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--self-test-ipc" => present = true,
            "--log" => {
                if let Some(v) = args.next() {
                    log = PathBuf::from(v);
                }
            }
            "--timeout-secs" => {
                if let Some(v) = args.next() {
                    timeout_secs = v.parse().unwrap_or(timeout_secs).max(1);
                }
            }
            _ => {}
        }
    }

    present.then_some(SelfTestIpcArgs { log, timeout_secs })
}

/// IPC 指令清單（design.md D4）。task 5.6 fix round 1 加 `get_pause`；fix F1（review 7.3 M3）加
/// `get_edit_mode`；widget-font-scale-per-widget task 2.2 加 `adjust_widget_font_scale`、
/// `get_widget_font_state`（design.md D3）。本 app 沒有宣告 app manifest，自訂指令不受
/// capability 的逐指令 ACL 管制（`render_done_command` 也由不在 `capabilities/default.json`
/// 視窗清單內的桌布渲染視窗呼叫），任何本機 webview 都能呼叫——故兩個字級指令一律以呼叫端
/// label 判斷身分、非小工具回錯誤。
///
/// `self-test-ipc` feature 另外多註冊三個驗收用指令：
/// [`self_test_ipc::self_test_set_manual_pause`]（讓 `host/tools/verify-5.6.ps1` 不靠系統匣點擊
/// ＝不注入輸入就能把宿主切進手動暫停）與
/// [`self_test_ipc::self_test_quit`]（task 5.7，讓 `host/tools/rm-restart-test.ps1` 不靠系統匣
/// 點擊就能觸發與「結束」完全相同的路徑）、[`self_test_ipc::self_test_unregister_restart`]
/// （fix F2，review 5.7：只取消重新啟動註冊、不結束行程）；正式 release build（不加
/// `--features`）不含這些指令。
fn invoke_handler() -> impl Fn(tauri::ipc::Invoke) -> bool + Send + Sync + 'static {
    #[cfg(not(feature = "self-test-ipc"))]
    {
        tauri::generate_handler![
            widgets::get_snapshot,
            widgets::subscribe_data,
            widgets::get_settings,
            widgets::get_pause,
            widgets::get_edit_mode,
            widgets::update_settings,
            widgets::set_edit_mode,
            widgets::report_content,
            widgets::adjust_widget_font_scale,
            widgets::get_widget_font_state,
            wallpaper_render::render_done_command,
            wallpaper_render::render_failed_command,
            wallpaper_settings::get_wallpaper_status,
            wallpaper_settings::get_wallpaper_config,
            wallpaper_settings::select_wallpaper_theme,
            wallpaper_settings::respond_spotlight_confirmation,
            fetch::status::get_fetch_status,
        ]
    }
    #[cfg(feature = "self-test-ipc")]
    {
        tauri::generate_handler![
            widgets::get_snapshot,
            widgets::subscribe_data,
            widgets::get_settings,
            widgets::get_pause,
            widgets::get_edit_mode,
            widgets::update_settings,
            widgets::set_edit_mode,
            widgets::report_content,
            widgets::adjust_widget_font_scale,
            widgets::get_widget_font_state,
            wallpaper_render::render_done_command,
            wallpaper_render::render_failed_command,
            wallpaper_settings::get_wallpaper_status,
            wallpaper_settings::get_wallpaper_config,
            wallpaper_settings::select_wallpaper_theme,
            wallpaper_settings::respond_spotlight_confirmation,
            fetch::status::get_fetch_status,
            self_test_ipc::self_test_set_manual_pause,
            self_test_ipc::self_test_quit,
            self_test_ipc::self_test_unregister_restart,
        ]
    }
}

/// 建立小工具與桌布協調所需的啟動參數（`build_ui` 只吃它，方便包進 `updater::UiBuilder`）。
struct UiParams {
    /// 桌布協調迴圈只在 Primary 跑；`--self-test-render`（4.5 量測）不跑。
    run_wallpaper_coordinator: bool,
    #[cfg(feature = "self-test-ipc")]
    self_test_render_args: Option<self_test_render::Args>,
}

/// 建立小工具視窗、桌布協調與其周邊（置底守門、資料輪詢、抓取排程）——原本 `setup` 內「更新器啟動之後」的
/// 全部內容，原樣搬出，邏輯不變。必須在主執行緒呼叫：無舊標記時由 `setup` 直接呼叫；讀到舊標記時由
/// `updater` 過渡期狀態機經 `run_on_main_thread` 呼叫（installer-auto-update task 3.1，design.md D3）。
fn build_ui(handle: &tauri::AppHandle, params: UiParams) {
    let UiParams {
        run_wallpaper_coordinator,
        #[cfg(feature = "self-test-ipc")]
        self_test_render_args,
    } = params;
    // installer-auto-update 4.x：已在「因更新結束」就一個都不建。最終審查 M2 起自動安裝不會在 UI 建好之前開始
    // （建立中符合條件的更新延後到建好後在背景重新評估），原本「`--autostart` 後 0.2 秒即安裝」的實機情況不再
    // 發生；這個檢查與其後各啟動點（小工具視窗、協調迴圈、抓取排程）的檢查留作防線，涵蓋「這裡通過之後才開始
    // 收尾」的交錯。
    if updater::is_exiting_for_update() {
        log::info!("更新過渡期：已在因更新結束，不建立小工具與桌布協調");
        return;
    }
    // task 5.3：編輯版面拖曳結束的回呼（design.md D9），必須在
    // `widgets::sync_widget_windows` 建立任何小工具視窗、進而安裝
    // `desktop::widget_subclass_proc` 之前登記——理論上子類別化的視窗要等到使用者
    // 進入編輯模式並實際拖曳才會觸發 `WM_EXITSIZEMOVE`，順序其實不影響正確性，但先登記
    // 較不容易在日後改動時不小心留下「視窗已建立、回呼還沒登記」的空窗。
    // task 7.5：擴充為三個時機點（開始／拖曳中／結束），拖曳中以 `edit-preview` 預告
    // 「若此刻放開」是否合法，放開時不合法就彈回（design.md D7「編輯版面」）。
    let drag_enter_handle = handle.clone();
    let drag_moving_handle = handle.clone();
    let drag_sizing_handle = handle.clone();
    let drag_end_handle = handle.clone();
    let drag_settle_handle = handle.clone();
    let drag_destroy_handle = handle.clone();
    desktop::register_widget_drag_hooks(desktop::WidgetDragHooks {
        on_enter: Box::new(move |hwnd, cursor| {
            widgets::begin_widget_drag(&drag_enter_handle, hwnd, cursor);
        }),
        // fix drag-dpi：回傳視窗此刻應有的矩形（跨縮放比例時即時換成目標顯示器上的
        // 實際大小），`desktop` 改寫提議矩形並套用。
        on_moving: Box::new(move |hwnd, proposed, cursor| {
            widgets::update_widget_drag(&drag_moving_handle, hwnd, proposed, cursor)
        }),
        // task 7.6：編輯版面調整大小（`WM_SIZING`）的紅框預告，放開時同一個
        // `on_exit` 依收到過的被拖邊改走調整大小的判斷。
        on_sizing: Box::new(move |hwnd, edges, proposed| {
            widgets::update_widget_resize(&drag_sizing_handle, hwnd, edges, proposed);
        }),
        // fix F6b：結束時只記下待判定，`desktop` 投遞自訂訊息後才以當時的視窗矩形
        // 判定（屆時系統放開後的定位都已完成，見 design.md D7「取消」）。
        on_exit: Box::new(move |hwnd| widgets::finish_widget_drag(&drag_end_handle, hwnd)),
        on_settle: Box::new(move |hwnd, seq| {
            widgets::settle_widget_drag(&drag_settle_handle, hwnd, seq);
        }),
        on_destroyed: Box::new(move |hwnd| {
            widgets::forget_widget_drag(&drag_destroy_handle, hwnd);
        }),
    });

    // task 3.1：依設定建立所有開啟的小工具視窗（首次啟動＝五個財經小工具）。setup 在
    // 主執行緒、事件迴圈已就緒，Tauri 允許在此建立視窗。
    widgets::sync_widget_windows(handle);

    // installer-auto-update 複審 M-A：建立小工具視窗時 WebView2 以巢狀訊息泵等待，`--restore-wallpaper` 的交接
    // （跨行程 SendMessage）可能在這段期間被派送進來，把過渡期轉到「結束」並要求結束（M2a）。結束要求要等這裡
    // 返回才處理，所以已在「結束」就不再啟動協調迴圈、看門狗、守門視窗與抓取排程；已建的小工具隨行程結束。
    if updater::transition_quitting(handle) {
        log::info!(
            "更新過渡期：建立小工具途中已轉到「結束」，不啟動桌布協調、看門狗、守門視窗與抓取排程"
        );
        return;
    }

    // dynamic-wallpaper task 4.5：桌布渲染管線（design.md D2）。只登記 managed state，
    // 不建立視窗；排程呼叫屬 4.7。
    wallpaper_render::install(handle);
    #[cfg(feature = "self-test-ipc")]
    if let Some(args) = self_test_render_args {
        self_test_render::spawn(handle.clone(), args);
    }

    // dynamic-wallpaper task 4.7a：桌布協調迴圈（專屬執行緒；排程、渲染、設定桌布、還原）。
    // 只投遞喚醒給它的地方：資料輪詢（下方）、`update_settings`、`set_pause_reason`、守門
    // 視窗（顯示變更、explorer 重新啟動、系統時間變更）、縮放變更（`.run()`）。
    if run_wallpaper_coordinator {
        wallpaper_coordinator::spawn_for_app(handle);
    }

    // task 5.6：WebView2 卡死偵測看門狗（design.md D12；`ProcessFailed` 的復原則在
    // 視窗工廠建立每個小工具時訂閱，見 recovery.rs 模組文件）。
    recovery::spawn_watchdog(handle);

    // task 3.3：置底守門——隱藏頂層視窗接收 explorer 重啟廣播＋30 秒保險檢查
    // （design.md D8）。緊接在 sync_widget_windows 之後，讓守門的重排動作一開始就能
    // 操作到已建立的小工具視窗；兩個回呼各自即時查詢目前狀態（見 widgets.rs），不依賴
    // 呼叫當下的快照。task 3.4：同一個函式也在主執行緒安裝 Win+D 偵測的 WinEvent
    // 掛鉤（顯示桌面時小工具浮在桌面之上，見 desktop.rs「D8：Win+D 顯示桌面」）。task
    // 5.5：同一個函式也安裝自動暫停偵測（鎖定、忙碌輪詢、顯示器、電源來源、省電），
    // `on_pause_signal` 把 desktop 模組偵測到的原始訊號（[`desktop::PauseSignal`]，
    // 不認得暫停原因集合或使用者設定）轉成對應的 [`widgets::PauseReason`]——電池／省電
    // 是否真的觸發暫停取決於使用者設定，交給 [`widgets::set_battery_raw`]／
    // [`widgets::set_power_saver_raw`] 內部判斷（見 widgets.rs「電池／省電：業務判斷」
    // 一節）；其餘三種直接呼叫 [`widgets::set_pause_reason`]。
    let gatekeeper_handle_a = handle.clone();
    let gatekeeper_handle_b = handle.clone();
    let gatekeeper_handle_c = handle.clone();
    let gatekeeper_handle_d = handle.clone();
    let gatekeeper_handle_e = handle.clone();
    if let Err(err) = desktop::spawn_gatekeeper(
        handle,
        &settings::default_gatekeeper_log_path(),
        desktop::GatekeeperCallbacks {
            visible_widget_hwnds: Box::new(move || {
                widgets::visible_widget_hwnds(&gatekeeper_handle_a)
            }),
            rebottom_all: Box::new(move || {
                widgets::rebottom_all_visible_widgets(&gatekeeper_handle_b)
            }),
            // task 3.5：WM_DISPLAYCHANGE／WM_SETTINGCHANGE(SPI_SETWORKAREA)／
            // WM_DPICHANGED（下面 `.run()` 的 ScaleFactorChanged 分支）都經
            // `desktop::request_relayout` 合併、延後到守門視窗的計時器才呼叫這個回呼
            // （fix F2，review 3.5）。
            relayout_all: Box::new(move || {
                let count = widgets::relayout_all_widgets(&gatekeeper_handle_d);
                log::info!("多螢幕重新定位：已重排 {count} 個小工具");
                // dynamic-wallpaper task 4.7a：顯示組態變了，桌布也要重新列舉螢幕。
                wallpaper_coordinator::notify_app(
                    &gatekeeper_handle_d,
                    wallpaper_coordinator::Wake::DisplayChanged,
                );
                // dynamic-wallpaper task 5.2：主要顯示器的縮放可能變了，系統匣圖示重挑尺寸
                // （尺寸沒變時 app_icon 去重、不重送）。
                app_icon::request_refresh(&gatekeeper_handle_d);
                count
            }),
            on_pause_signal: Box::new(move |signal| match signal {
                desktop::PauseSignal::Locked(active) => {
                    widgets::set_pause_reason(
                        &gatekeeper_handle_c,
                        widgets::PauseReason::Locked,
                        active,
                    );
                }
                desktop::PauseSignal::DisplayOff(active) => {
                    widgets::set_pause_reason(
                        &gatekeeper_handle_c,
                        widgets::PauseReason::DisplayOff,
                        active,
                    );
                }
                desktop::PauseSignal::SystemBusy(active) => {
                    widgets::set_pause_reason(
                        &gatekeeper_handle_c,
                        widgets::PauseReason::SystemBusy,
                        active,
                    );
                }
                desktop::PauseSignal::OnBattery(on_battery) => {
                    widgets::set_battery_raw(&gatekeeper_handle_c, on_battery);
                }
                desktop::PauseSignal::PowerSaver(active) => {
                    widgets::set_power_saver_raw(&gatekeeper_handle_c, active);
                }
            }),
            // dynamic-wallpaper task 4.7a：同步廣播的處理常式內只投遞通知。
            on_system_event: Box::new(move |event| {
                let wake = match event {
                    desktop::SystemEvent::ExplorerRestarted => {
                        // dynamic-wallpaper task 5.2 修正輪 1：tray-icon 以它存的舊圖示重新
                        // 登記，重啟期間換的主題可能沒套上；作廢去重並刷新（立即返回）。
                        app_icon::on_explorer_restarted(&gatekeeper_handle_e);
                        wallpaper_coordinator::Wake::ExplorerRestarted
                    }
                    desktop::SystemEvent::TimeChanged => wallpaper_coordinator::Wake::TimeChanged,
                    // dynamic-wallpaper task 4.7b：工作階段結束不還原，只停止新的渲染。
                    desktop::SystemEvent::SessionEnding => {
                        // bug-leaked-renderer 修正輪 1（L3）：工作階段結束時，事件迴圈結束點的
                        // 渲染 browser 處置交給系統。
                        wallpaper_render::note_session_ending(true);
                        // installer-auto-update task 3.1：工作階段結束是正常結束路徑，刪除當機標記
                        // （QUERYENDSESSION 可能被取消，見 `updater::on_session_end_cancelled`）。
                        updater::on_session_ending();
                        wallpaper_coordinator::Wake::SessionEnding
                    }
                    desktop::SystemEvent::SessionEndCancelled => {
                        wallpaper_render::note_session_ending(false);
                        updater::on_session_end_cancelled();
                        wallpaper_coordinator::Wake::SessionEndCancelled
                    }
                };
                wallpaper_coordinator::notify_app(&gatekeeper_handle_e, wake);
            }),
        },
    ) {
        log::error!("建立置底守門視窗失敗：{err}");
    }

    let poll_handle = handle.clone();
    // 排程執行緒：啟動時立即做第一次輪詢（避免小工具乾等最多 30 秒才看到第一份
    // 資料），之後每 POLL_INTERVAL 重複。`thread::spawn` 而非 `tauri::async_runtime`
    // ——`ChannelRegistry::poll_all_changed` 本身是同步阻塞的檔案 I/O（`data.rs`：
    // `fs::metadata`／`fs::read_to_string`），丟進獨立系統執行緒比包一層 async 簡單、
    // 也不會佔用 Tauri 的 async runtime worker。
    thread::spawn(move || loop {
        {
            let state = poll_handle.state::<AppState>();
            // dynamic-wallpaper task 4.6 修正輪 3：主題設定檔讀到損壞時的延後重讀在鎖外等待
            // （`poll_and_notify_settled`），不阻塞主執行緒的 get_snapshot／update_settings。
            let changed = widgets::poll_and_notify_settled(&state, thread::sleep);
            // dynamic-wallpaper task 4.7a：資料或主題設定有更新就喚醒桌布協調迴圈。
            if changed
                .iter()
                .any(|c| c == data::TW_EVENTS_CHANNEL || c == data::WALLPAPER_CHANNEL)
            {
                wallpaper_coordinator::notify_app(
                    &poll_handle,
                    wallpaper_coordinator::Wake::DataChanged,
                );
            }
        }
        thread::sleep(POLL_INTERVAL);
    });

    // data-layer-rust task 5.2／5.3：市場資料抓取排程（專用執行緒；台北時點、補抓、60 分鐘下限、
    // 失敗重試；`data_fetch` 與隔離偵測決定要不要抓，見 `fetch::scheduler` 模組文件）。放在資料輪詢
    // 執行緒之後：抓取寫出的 `tw_events.json` 由上面的輪詢在 30 秒內撿起。控制把手進 managed
    // state，供系統匣「結束」停止它（`tray::quit`）。
    fetch::scheduler::spawn_for_app(handle);
}

/// 處理一個手動重複啟動的 single-instance 交接（開啟設定視窗）。UI 已建立（或更新器停用）時由回呼直接呼叫；
/// UI 建好之前（過渡期，或沒有舊標記時 setup 同步建立途中）排隊的請求在小工具建立完成後由 `updater` 依序重放。
fn replay_manual_handoff(app: &tauri::AppHandle, args: Vec<String>) {
    log::info!("single-instance：手動重複啟動（args={args:?}），開啟設定視窗");
    tray::open_settings_window(app);
}

fn main() {
    // dynamic-wallpaper task 1.4（只在 probe-render 建置）：渲染成本探針接管整個行程。必須是
    // main 的第一件事——早於記錄檔初始化（寫 %LOCALAPPDATA%）、重新啟動註冊、單一執行個體仲裁、
    // 開機自啟與設定載入（`load_for_arbitrated_startup` 會寫預設設定），這些都會在真實系統留下
    // 副作用；探針一律不經過它們（見 probe_render 模組文件「隔離」一節）。
    #[cfg(feature = "probe-render")]
    if let Some(parsed) = probe_render::parse_args(std::env::args().skip(1)) {
        std::process::exit(probe_render::run(parsed));
    }

    // dynamic-wallpaper task 4.5（design.md D2）：WebView2 的 `WEBVIEW2_USER_DATA_FOLDER` 會取代
    // 呼叫端指定的使用者資料夾，留在環境裡會讓桌布渲染視窗併進小工具那組的 browser 行程。建立
    // 任何執行緒與視窗之前讀出並移除，改由各建立點明確指定（見 webview_env 模組文件）。
    webview_env::capture_user_data_override();

    // task 5.8：全行程最開頭安裝統一記錄檔（design.md D12「本機檔案、每日輪替、保留 7
    // 天」），早於任何可能記錄事件的程式碼——含下面的 `--self-test-ipc` 分支，該模式仍會
    // 呼叫到 `desktop`／`widgets` 裡已換成 `log` 巨集的程式碼路徑。
    logging::init();
    // installer-auto-update task 3.3：記下主執行緒 id，更新前收尾（`updater::prepare_exit_for_update`）據此拒絕
    // 在主執行緒執行（會與 `run_on_main_thread` 死鎖）。
    updater::mark_main_thread();
    // 2026-10-01 soak：宿主無聲消失。GUI 子系統沒有主控台，panic 訊息原本只到看不到的
    // stderr；之後任何 panic 都在記錄檔留下訊息、位置與 backtrace（見 logging 模組）。
    logging::install_panic_hook();
    log::info!(
        "fc-host 啟動 version={} pid={}",
        env!("CARGO_PKG_VERSION"),
        std::process::id()
    );
    // 同一事件：從開發工具殼層啟動時會繼承 SEM_NOGPFAULTERRORBOX，原生當機完全不經 WER（無事件、
    // 無 dump）。清掉它，讓原生當機至少留下 WER 報告（見 desktop::ensure_crashes_reach_wer）。
    desktop::ensure_crashes_reach_wer();

    // dynamic-wallpaper task 4.7b：`--restore-wallpaper`（安裝檔呼叫）接管整個行程——不建立小工具
    // 視窗、系統匣、協調迴圈的排程，也不登記重新啟動；有宿主在跑就經 single-instance 通道交給它，
    // 否則在本行程直接還原，以結束碼回報（見 wallpaper_cli 模組文件）。放在啟動仲裁之前：它自己
    // 仲裁，且不得成為帶小工具的 Primary。
    if wallpaper_cli::requested(&std::env::args().collect::<Vec<_>>()) {
        std::process::exit(wallpaper_cli::run());
    }

    // data-layer-rust task 5.1：`--fetch-once <目錄>` 單次抓取接管整個行程——抓一輪、寫
    // `<目錄>/tw_events.json` 後以結束碼回報；不經單一執行個體、不看 `data_fetch`、不讀寫排程
    // 紀錄、不建立視窗與系統匣（見 fetch::cli 模組文件）。與 `--restore-wallpaper` 同位置，且在其後。
    if fetch::cli::requested(&std::env::args().collect::<Vec<_>>()) {
        std::process::exit(fetch::cli::run());
    }

    // 驗收用（只在 self-test-ipc 建置）：主執行緒存取違規，確認從錯誤模式 0x8003 的殼層啟動時，
    // 原生當機現在會留下「Application Error」事件。
    #[cfg(feature = "self-test-ipc")]
    if std::env::args().any(|a| a == "--self-test-native-crash") {
        // SAFETY: 刻意寫入空指標以觸發存取違規（0xC0000005），只存在於驗收建置。
        unsafe { std::ptr::null_mut::<i32>().write_volatile(1) };
    }

    // 驗收用（只在 self-test-ipc 建置）：主執行緒直接 panic，確認正式二進位檔的 panic 訊息
    // 會落進記錄檔（以暫存 %LOCALAPPDATA% 執行 `fc-host.exe --self-test-panic`，查
    // `logs\fc-host.<今天>.log` 有 `fc_host::panic` 一行、結束碼 101）。
    #[cfg(feature = "self-test-ipc")]
    if std::env::args().any(|a| a == "--self-test-panic") {
        panic!("self-test panic（驗證 panic hook 寫入記錄檔）");
    }

    // task 4.5（只在 self-test-ipc 建置）：`--self-test-render` 走正常啟動（小工具照常建立），
    // setup 完成後另開執行緒以正式渲染管線出圖並量測，見 self_test_render 模組文件。
    #[cfg(feature = "self-test-ipc")]
    let self_test_render_args = match self_test_render::parse_args(std::env::args().skip(1)) {
        None => None,
        Some(Ok(args)) => Some(args),
        Some(Err(e)) => {
            log::error!("--self-test-render 參數錯誤：{e}");
            std::process::exit(2);
        }
    };

    #[cfg(feature = "self-test-ipc")]
    if let Some(args) = parse_self_test_ipc_args() {
        // 接手整個行程：驗證模式開自己的測試視窗，不啟動下面正常的產品視窗與排程執行緒。
        self_test_ipc::run(args.log, args.timeout_secs);
        return;
    }

    // installer-auto-update task 3.3（design.md D4）：`--wait-exit <pid>`——安裝檔已啟動卻在 `install()` 之後失敗時，
    // 舊宿主以 `--autostart --wait-exit <本 PID>` 啟動新的自己再立刻結束。新行程必須先等舊行程結束才進入仲裁，否則
    // 取不到執行個體鎖就會當 Secondary 退出，最後沒有宿主在跑。放在仲裁之前；PID 被重用時不等（驗映像路徑與建立時間）。
    desktop::wait_exit::run_from_args(&std::env::args().collect::<Vec<_>>());

    // task 5.1 fix round 1（Codex task-5.1-codex.md [high]）：在 `tauri::Builder` 之前以自有的
    // 兩把具名 mutex 仲裁「誰是唯一執行個體」，補 single-instance plugin「mutex 已建、交接視窗
    // 未建」之間的同時啟動競態（見 `desktop/instance.rs` 模組文件）。放在讀設定之前：落敗的
    // 行程不需要、也不該寫設定檔（fix F3，review 5.1 low：Secondary 只唯讀載入，見下方
    // `load_for_arbitrated_startup`）。`arbitration` 必須活到行程結束——它持有執行個體鎖。
    let Some(arbitration) = desktop::arbitrate_startup() else {
        // 等啟動鎖逾時：已有執行個體卡在啟動中，再開一組只會變兩組（安全退出，見模組文件）。
        return;
    };
    let startup_role = arbitration.role();

    // installer-auto-update task 3.1（design.md D3）：更新器啟用判定與當機迴圈保護標記。放在仲裁成功之後（只有
    // 取得執行個體鎖的 Primary 才讀寫標記；Secondary、`--restore-wallpaper`、`--fetch-once` 完全不碰——後兩者在
    // 仲裁之前就 exit 了），且早於 settings 載入與 `tauri::Builder`，讓 Builder／WebView2 初始化當機也被標記抓到。
    // `generate_context!()` 提前展開：判定只需要設定檔，不需要任何 Tauri 執行期物件。
    let context = tauri::generate_context!();
    let updater_gate = updater::evaluate_gate(context.config());
    let updater_startup = updater::prepare(
        startup_role,
        &updater_gate,
        &std::env::args().collect::<Vec<_>>(),
    );

    // task 2.3：讀取設定檔（缺檔／缺欄位／損壞皆有預設值可退回，見 settings.rs 文件）。
    let settings_path = settings::default_settings_path();
    // 首次啟動把預設值落地；損壞／版本不符的檔案備份後也立刻存回預設值（fix F2b）。
    // fix F3（review 5.1 low）：以上寫入只在仲裁沒有判定落敗時做；Secondary 唯讀載入、不建立
    // 不備份不覆寫。`Unarbitrated`（仲裁機制本身故障、退回只靠 plugin）照舊走寫入路徑——它可能
    // 就是唯一執行個體，不能讓首次啟動永遠不落地。
    // 使用者決定（2026-10-01）：開機自啟登錄由安裝檔寫入與移除，啟動流程（含設定檔首次建立）
    // 一律不碰登錄；只有使用者在設定視窗切換開關時才寫（`widgets::update_settings`）。
    let settings::StartupSettings {
        settings,
        save_error,
    } = settings::load_for_arbitrated_startup(
        &settings_path,
        startup_role.may_write_startup_state(),
    );
    if let Some(err) = save_error {
        log::error!("啟動：寫入設定檔失敗（{}）：{err}", settings_path.display());
    }
    // task 2.7：設定＋通道註冊表（依 settings.data_dir 建立，design.md D5）進 Tauri managed
    // state，供下面的 IPC 指令與排程執行緒共用。fix F2（review 5.4）：正式宿主接上真正的
    // 開機自啟登錄（只讀寫 Run 值，見 `widgets::AutostartRegistry`）。
    // dynamic-wallpaper task 4.7a：主題設定檔依啟動角色載入——Primary 缺檔時寫出預設檔，Secondary
    // 只讀不寫（見 `wallpaper_coordinator::startup_plan`）；結果交給 `wallpaper` 通道的監看，第一輪
    // 輪詢不重讀、不重記警告。
    let wallpaper_plan = wallpaper_coordinator::startup_plan(startup_role);
    let theme_config = wallpaper_coordinator::load_theme_config(
        wallpaper_plan,
        &settings_path.with_file_name(wallpaper_config::CONFIG_FILE_NAME),
    );
    // 桌布協調迴圈只在 Primary 跑；`--self-test-render`（4.5 量測）不跑，免得兩邊同時渲染。
    let run_wallpaper_coordinator = wallpaper_plan.run_coordinator;
    #[cfg(feature = "self-test-ipc")]
    let run_wallpaper_coordinator = run_wallpaper_coordinator && self_test_render_args.is_none();
    let state = AppState::new(settings, settings_path)
        .with_autostart_registry(Box::new(desktop::HkcuRunRegistry))
        .with_wallpaper_config(&theme_config);

    let builder = tauri::Builder::default()
        // task 5.1／design.md D12：單一執行個體。crate 文件要求註冊在所有 plugin 最前面——
        // 第二個執行個體的偵測（具名 mutex）與交接必須先於其餘啟動流程完成，見
        // `tray.rs` 模組文件「單一執行個體」一節與 crate 原始碼
        // `platform_impl/windows.rs`（`Builder::build` 的 doc："Register it first among your
        // app's plugins"）。回呼收到的是第二個行程的命令列參數：帶 `--autostart`／
        // `--restarted`（開機自啟或系統重新啟動觸發的重複啟動）靜默忽略；其餘（手動重複啟動）
        // 開啟既有執行個體的設定視窗（specs/widget-host-lifecycle「單一執行個體」兩個
        // Scenario）。
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // dynamic-wallpaper task 4.7b：`--restore-wallpaper` 的交接——只投遞給桌布協調迴圈、
            // 立即返回（這裡是交接視窗的同步 WndProc），結果以具名事件回報給命令列行程。
            if let Some(reply_pid) = wallpaper_cli::handoff_request(&args) {
                // installer-auto-update 最終審查 M2a：更新過渡期（協調迴圈尚未建立）中交給宿主只會「交接失敗」，而
                // 宿主不結束，命令列等不到它結束就放棄還原。改為：回報「交接失敗」並以過渡期結束收掉宿主（不還原、
                // 不存主題、刪標記），命令列等到宿主結束後在自己的行程還原。先回報、再結束；兩者都不等待。
                if updater::quit_for_restore_handoff(app) {
                    wallpaper_cli::refuse_handoff_during_update_transition(reply_pid);
                    updater::quit_during_transition(app);
                    return;
                }
                wallpaper_cli::accept_handoff(app, reply_pid);
                return;
            }
            if tray::is_automated_relaunch(&args) {
                log::info!("single-instance：自動化的重複啟動（args={args:?}），靜默忽略");
                return;
            }
            // installer-auto-update task 3.1：小工具尚未建好時（更新過渡期，或沒有舊標記時 setup 同步建立途中的
            // 巢狀訊息泵——最終審查 M2）的交接請求先排隊，建完後由 updater 重放；其餘情況（含更新器停用）即時處理。
            match updater::route_handoff(app, &args) {
                updater::HandoffDisposition::ProcessNow => replay_manual_handoff(app, args),
                updater::HandoffDisposition::Queued | updater::HandoffDisposition::Dropped => {}
            }
        }));
    // installer-auto-update task 3.1（design.md D1、D3）：更新外掛只在判定啟用時註冊——外掛在 `build()` 時就解析
    // `plugins.updater`，設定有問題會讓整個 Builder 失敗；更新器是附屬功能，不得因此讓宿主無法啟動。
    // capability 不開給任何 webview，更新完全由後端主導。
    let builder = if updater_startup.enabled() {
        builder.plugin(updater::plugin())
    } else {
        builder
    };
    builder
        // D12／task 2.2、5.4：開機自啟。fix F2（review 5.4）起不再註冊 `tauri-plugin-autostart`：
        // 它底下 `auto-launch` 的 `enable()` 每次都把 `...\Explorer\StartupApproved\Run` 改回「啟用」
        // （撤銷使用者在工作管理員的停用）、Run 值的路徑也沒加引號。改由
        // `widgets::sync_autostart`＋`desktop::HkcuRunRegistry` 只寫 Run 值
        // （`"<exe>" --autostart`，值名沿用 `fc-host`），且只在使用者切換設定開關時寫（初次
        // 登錄與移除歸安裝檔），`--autostart` 讓上面的 single-instance 回呼與
        // `tray::is_automated_relaunch` 認出開機自啟。
        // task 5.1：系統匣選單事件的全域分派（`tray::handle_menu_event`）。
        .on_menu_event(tray::handle_menu_event)
        .manage(state)
        // 這五個指令**不需要**額外的 capability 宣告，見 `widgets.rs` 模組文件「Command 權限」
        // 一節的查證（本 crate 沒有 `host/permissions/`，app 自訂指令的 ACL 檢查對本機來源
        // 整段略過）。
        .invoke_handler(invoke_handler())
        .setup(move |app| {
            // task 5.1 fix round 1：single-instance plugin 已在 `build()` 內建好 mutex 與交接
            // 視窗，啟動臨界區結束，放開啟動鎖讓排隊中的行程進來交接（見
            // `desktop/instance.rs`）。
            desktop::release_startup_lock();
            if startup_role == desktop::StartupRole::Secondary {
                // 已有執行個體（持有執行個體鎖），plugin 卻沒找到交接視窗而放行到這裡——首個
                // 執行個體正在結束，或其交接視窗建立失敗。在建立任何視窗之前安全退出，不成為
                // 第二組小工具；`cleanup_before_exit` 同 plugin 交接退出的做法，移除
                // `build()` 時已建立的系統匣圖示。
                log::warn!(
                    "啟動仲裁：已有執行個體但 single-instance 未交接（交接視窗不存在），本行程退出"
                );
                app.handle().cleanup_before_exit();
                std::process::exit(0);
            }

            // task 5.7：向系統註冊本行程可重新啟動（design.md D12）。放在 setup 最前面——
            // `.setup()` 閉包只有勝出 single-instance 判定的那個行程會跑到（見 desktop.rs
            // 模組文件「task 5.7 補上」一節），越早註冊越好（Microsoft Learn：初始註冊必須在
            // 行程遇到例外或無回應之前完成）。對應的 `UnregisterApplicationRestart` 呼叫在
            // `tray::quit`。
            desktop::register_for_restart();

            // task 5.1：系統匣選單掛到啟動時已由 tray-icon feature 自動建立的圖示上（見
            // tray.rs 模組文件）。放在 setup 最前面，之後任何流程需要用選單反映狀態
            // （目前沒有）都能確定選單已存在。
            tray::register(app.handle());
            // dynamic-wallpaper task 5.2：系統匣（設定檔 `trayIcon` 建立時是統一圖示 16px，
            // `icons/png/icon-16.png`）換成目前主題、主要顯示器縮放對應尺寸的圖示（立即返回，套用在
            // 主執行緒，見 app_icon 模組文件）。
            app_icon::request_refresh(app.handle());

            // installer-auto-update task 3.1（design.md D3）：更新任務先於小工具與桌布協調啟動。建立 UI 那一大段
            // 包成 `build_ui`，無舊標記時 `updater::start` 立即呼叫它、建完才把狀態轉到「UI 已建立」（最終審查 M2）；
            // 讀到舊標記時由過渡期狀態機決定何時（在主執行緒）呼叫。
            let ui_params = UiParams {
                run_wallpaper_coordinator,
                #[cfg(feature = "self-test-ipc")]
                self_test_render_args,
            };
            updater::start(
                app.handle(),
                &updater_startup,
                Box::new(move |handle: &tauri::AppHandle| build_ui(handle, ui_params)),
                replay_manual_handoff,
            );
            Ok(())
        })
        .build(context)
        .expect("fc-host 啟動失敗")
        .run(|app, event| {
            // task 3.1：小工具視窗可由設定全部關閉；最後一個視窗關閉時 Tauri 會送
            // `ExitRequested { code: None }`，宿主（常駐系統匣）不能因此結束。程式主動結束
            // （之後系統匣「結束」呼叫 `app.exit(0)`，code 為 `Some`）照常放行（design.md D12）。
            if let tauri::RunEvent::ExitRequested {
                code: None, api, ..
            } = &event
            {
                api.prevent_exit();
            }

            // 2026-10-01 soak：宿主無聲消失。正常結束路徑也要留一行，之後才分得出「事件迴圈
            // 正常結束」「panic」（logging::install_panic_hook）與「兩者皆無＝原生當機或被外部
            // 終止」。
            match &event {
                tauri::RunEvent::ExitRequested {
                    code: Some(code), ..
                } => log::info!("事件迴圈：收到結束要求 code={code}"),
                tauri::RunEvent::Exit => {
                    log::info!("事件迴圈結束（RunEvent::Exit），行程即將結束");
                    // installer-auto-update task 3.1：任何走到這裡的結束都是正常結束（panic、abort、原生當機
                    // 不會）：刪除當機標記、停止更新任務。
                    updater::on_exit(app);
                    // data-layer-rust task 5.2：總保險——任何結束路徑（含日後 installer-auto-update 的「因更新
                    // 結束」）都至少設一次抓取排程的停止旗標（非阻塞）；要等它停下的路徑自己呼叫
                    // `fetch::scheduler::stop_app`。
                    fetch::scheduler::request_stop_app(app);
                    // bug-leaked-renderer 修正輪 1（L3）：系統匣「結束」以外的 app.exit 路徑也處置前一個
                    // 渲染 browser 行程；主執行緒上不等待，工作階段結束中則交給系統。
                    wallpaper_render::settle_on_event_loop_exit(app);
                }
                _ => {}
            }

            // fix round 1（Codex 3.1）：`destroy()` 是非同步的；小工具視窗真正銷毀完成（Tauri
            // 此時已把它移出視窗表）後，若這次銷毀是視窗工廠發起的，再同步一次開關——銷毀
            // 期間使用者又把同一個小工具開回來時，由這裡補建（見 `widgets::on_window_destroyed`）。
            if let tauri::RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::Destroyed,
                ..
            } = &event
            {
                widgets::on_window_destroyed(app, label);
            }

            // task 3.5（design.md D9「多螢幕與 DPI 定位」）：縮放比例改變（`WM_DPICHANGED`）
            // 只送給縮放改變的那台顯示器上的視窗；tao 把它轉譯成 Tauri
            // `WindowEvent::ScaleFactorChanged`。哪一扇小工具視窗收到就代表哪一台顯示器的縮放
            // 變了，但重排本身處理**全部**可見小工具（見 `widgets::relayout_all_widgets`
            // 文件），故這裡不需要用 `label` 做任何篩選以外的事——只用它排除非小工具視窗
            // （例如 `settings`、守門視窗）觸發的縮放事件，避免無意義的重排。
            if let tauri::RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::ScaleFactorChanged { .. },
                ..
            } = &event
            {
                // fix F2（review 3.5 high）：不可在這裡同步重排——回呼返回後 tao 會再對這扇
                // 視窗 `SetWindowPos(Windows 建議矩形)`，蓋掉格線矩形。改經守門視窗延後、合併
                // （`desktop::request_relayout`，見 `desktop::RelayoutCoalescer`）。守門視窗不存在
                // 時才退回立即重排（至少其他視窗正確，最後一扇等下一次觸發）。
                if widgets::widget_id_from_label(label).is_some()
                    && !desktop::request_relayout(desktop::RelayoutReason::DpiChanged)
                {
                    let count = widgets::relayout_all_widgets(app);
                    log::warn!(
                        "多螢幕重新定位（縮放變更，{label}，守門視窗不可用、立即執行）：已重排 \
                         {count} 個小工具"
                    );
                }
                // dynamic-wallpaper task 4.7a：桌布依螢幕的 DPI 出圖，縮放變了要重新評估。
                wallpaper_coordinator::notify_app(app, wallpaper_coordinator::Wake::DpiChanged);
                // dynamic-wallpaper task 5.2：設定視窗換到不同縮放的顯示器，圖示重挑尺寸（立即
                // 返回，套用延後到主執行緒佇列，不受 tao 隨後 SetWindowPos 影響）。
                if label == tray::SETTINGS_WINDOW_LABEL {
                    app_icon::request_refresh(app);
                }
            }
        });
}
