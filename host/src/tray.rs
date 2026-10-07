//! 系統匣選單與單一執行個體（design.md D12；task 5.1）。
//!
//! 圖示本身由 `tauri.conf.json` 的 `app.trayIcon`＋`Cargo.toml` 的 `tray-icon` feature
//! 在 `App::build` 時自動建立（`tauri-2.12.0/src/app.rs` 的
//! `#[cfg(all(desktop, feature = "tray-icon"))]` 區塊），id 未在設定檔指定時預設為 `"main"`
//! （task 2.1 註記；同一段程式碼 `tray_config.id.clone().unwrap_or_else(|| "main".into())`）；
//! 本模組不重建圖示，只在 `main.rs` 的 `setup` 內以 [`register`] 取得該圖示並掛上選單。
//!
//! ## 選單（specs/widget-host-lifecycle「系統匣常駐」）
//!
//! - [`MenuEntry`]／[`build_menu_entries`]：選單內容的純函式表示（id、文字、啟用／勾選狀態），
//!   不依賴任何 Tauri 執行期物件，單元測試直接呼叫（brief 要求的「選單建構函式」驗證方式）。
//! - [`materialize_menu`]：把 [`MenuEntry`] 轉成真正的 `tauri::menu::Menu`，需要
//!   `AppHandle`／`Manager`，不在單元測試涵蓋範圍（同 `widgets::sync_widget_windows` 與其
//!   `plan_window_changes` 的分工方式）。
//! - [`refresh`]：狀態變更後重新整理選單（`編輯版面` 的勾選、`暫停／繼續` 的文字）；每次選單
//!   事件處理完都呼叫一次，成本低（七個項目）不值得為此另存個別項目的控制代碼。
//! - [`handle_menu_event`]：五個項目的事件分派，在 `main.rs` 以
//!   `tauri::Builder::on_menu_event` 註冊為全域監聽器。
//!
//! ## 單一執行個體（specs/widget-host-lifecycle「單一執行個體」）
//!
//! - [`is_automated_relaunch`]：純函式，判斷第二個執行個體的命令列參數是否帶
//!   `--autostart`／`--restarted`（`tauri-plugin-single-instance` 的回呼收到的是**第二個**
//!   行程的 `std::env::args()`，第一個元素是執行檔路徑——見 crate 原始碼
//!   `platform_impl/windows.rs` 的 `run_callback` 與其文件註解）。
//! - `main.rs` 以 `tauri_plugin_single_instance::init` 註冊回呼：偵測到自動化的重複啟動
//!   （開機自啟或系統重新啟動）時靜默忽略；否則呼叫 [`open_settings_window`]。第二個執行個體
//!   本身無論哪種情況都在 crate 內部 `std::process::exit(0)`，不會執行到 `main.rs` 下半段
//!   （crate 文件："The second instance never reaches `tauri::Builder::run`"）——退出碼固定
//!   為 0，行為差異只在「既有執行個體要不要開設定視窗」，符合
//!   specs/widget-host-lifecycle 兩個 Scenario 的要求。

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::thread;

use tauri::menu::{
    CheckMenuItemBuilder, Menu, MenuBuilder, MenuEvent, MenuItemBuilder, PredefinedMenuItem,
};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder, Wry};

use crate::widgets::{self, reason_label, AppState, PauseReason};

/// 設定視窗的固定 label（`capabilities/default.json` 的 `windows` 清單已含 `"settings"`）。
pub const SETTINGS_WINDOW_LABEL: &str = "settings";

/// 系統匣中「main」圖示的 id（`tauri.conf.json` 未設 `trayIcon.id`，Tauri 預設值，見本檔模組
/// 文件開頭）。
pub(crate) const TRAY_ID: &str = "main";

const MENU_ID_EDIT_MODE: &str = "edit_mode";
const MENU_ID_OPEN_SETTINGS: &str = "open_settings";
const MENU_ID_PAUSE_TOGGLE: &str = "pause_toggle";
const MENU_ID_INSTALL_UPDATE: &str = "install_update";
const MENU_ID_VERSION: &str = "version";
const MENU_ID_QUIT: &str = "quit";

// ── 選單內容（純函式）────────────────────────────────────────────────────────────────

/// 選單中的一個項目（[`build_menu_entries`] 的輸出型別），與實際 Tauri 選單物件無關，供單元
/// 測試直接檢查。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuEntry {
    /// 有勾選狀態的項目（本 task 只有「編輯版面」）。
    Check {
        id: &'static str,
        label: String,
        checked: bool,
    },
    /// 一般可點項目；`enabled=false` 用於「版本號」（不可點，只顯示）。
    Item {
        id: &'static str,
        label: String,
        enabled: bool,
    },
    Separator,
}

/// design.md D12／task-5.1-brief：系統匣選單，依序為「編輯版面、設定、暫停／繼續、（空間不足
/// 暫時隱藏項目，若有）、版本號、結束」，版本號前後各加一條分隔線把它與可操作項目隔開。純函式：
/// 同樣的 `(edit_mode, manual_active, other_reason, no_space_hidden, version)` 一定產生同樣的
/// 選單內容，供單元測試核對（見 `tray_tests`）。
///
/// task 5.5：`paused: bool` 參數改成 `manual_active`／`other_reason` 兩個——原本的
/// `is_paused(app)`（任一原因即為 `true`）沒辦法反映「點『暫停／繼續』這個按鈕實際上在切換
/// 什麼」：它只切 [`PauseReason::Manual`] 這一個原因的成員資格（見
/// [`toggle_manual_pause`]／`widgets.rs` 模組文件「task 5.1 補上暫停原因集合」），若其他自動
/// 原因仍在，點「繼續」之後整體仍會維持暫停——這是正確語意（見 [`pause_toggle_label`]），但
/// 選單文字要能反映，故改吃這兩個更精確的輸入。
///
/// task 7.4（design.md D9；specs/widget-host-windows「多螢幕與 DPI 定位」「小工具互不重疊」）：
/// `no_space_hidden` 是 `crate::widgets::no_space_hidden_menu_entries` 算出的
/// `(小工具 id, 選單文字)` 清單，依序插在「暫停／繼續」之後、版本號分隔線之前，各自一條停用
/// （不可點）項目；為空時（沒有任何小工具因空間不足暫時隱藏）完全不插入任何東西，選單結構與
/// task 5.1 原本的六項＋兩條分隔線一致。
pub fn build_menu_entries(
    edit_mode: bool,
    manual_active: bool,
    other_reason: Option<PauseReason>,
    no_space_hidden: &[(&'static str, String)],
    version: &str,
    update_version: Option<&str>,
) -> Vec<MenuEntry> {
    let mut entries = vec![
        MenuEntry::Check {
            id: MENU_ID_EDIT_MODE,
            label: "編輯版面".to_string(),
            checked: edit_mode,
        },
        MenuEntry::Item {
            id: MENU_ID_OPEN_SETTINGS,
            label: "設定".to_string(),
            enabled: true,
        },
        MenuEntry::Item {
            id: MENU_ID_PAUSE_TOGGLE,
            label: pause_toggle_label(manual_active, other_reason),
            enabled: true,
        },
    ];
    // installer-auto-update task 3.2：有已下載並驗簽的新版時，在「暫停／繼續」之後加「更新到 vX.Y.Z 並重新啟動」。
    if let Some(update) = update_version {
        entries.push(MenuEntry::Item {
            id: MENU_ID_INSTALL_UPDATE,
            label: crate::updater::update_menu_label(update),
            enabled: true,
        });
    }
    if !no_space_hidden.is_empty() {
        entries.push(MenuEntry::Separator);
        for (id, label) in no_space_hidden {
            entries.push(MenuEntry::Item {
                id,
                label: label.clone(),
                enabled: false,
            });
        }
    }
    entries.push(MenuEntry::Separator);
    entries.push(MenuEntry::Item {
        id: MENU_ID_VERSION,
        label: format!("版本 {version}"),
        enabled: false,
    });
    entries.push(MenuEntry::Separator);
    entries.push(MenuEntry::Item {
        id: MENU_ID_QUIT,
        label: "結束".to_string(),
        enabled: true,
    });
    entries
}

/// 「暫停／繼續」項目的文字（task 5.5）。點下去只切換 [`PauseReason::Manual`]，故文字依
/// `manual_active` 決定基本動詞（「暫停」或「繼續」），`other_reason`（排除 `Manual` 後目前
/// 最優先的其他暫停原因，見 [`widgets::top_other_reason`]）非 `None` 時附註目前實際仍在暫停的
/// 原因，讓使用者知道點下去不一定會真的恢復動畫：
/// - 未手動暫停、也沒有其他原因 → 「暫停」。
/// - 未手動暫停、但已因其他原因暫停中 → 「暫停（目前已因……暫停中）」（點下去只是多加一個
///   手動原因，畫面不會有變化，但語意仍正確——集合聯集，插入已存在的成員是無害的）。
/// - 已手動暫停、沒有其他原因 → 「繼續」。
/// - 已手動暫停、但還有其他原因 → 「繼續（仍因……暫停中）」。
fn pause_toggle_label(manual_active: bool, other_reason: Option<PauseReason>) -> String {
    match (manual_active, other_reason) {
        (true, Some(r)) => format!("繼續（仍因{}暫停中）", reason_label(r)),
        (true, None) => "繼續".to_string(),
        (false, Some(r)) => format!("暫停（目前已因{}暫停中）", reason_label(r)),
        (false, None) => "暫停".to_string(),
    }
}

// ── 選單內容 → 真正的 Tauri 選單（需要 AppHandle，不在單元測試範圍）───────────────────

/// 把 [`MenuEntry`] 清單轉成 `tauri::menu::Menu`。呼叫順序即顯示順序。
fn materialize_menu(app: &AppHandle, entries: &[MenuEntry]) -> tauri::Result<Menu<Wry>> {
    let mut builder = MenuBuilder::new(app);
    for entry in entries {
        match entry {
            MenuEntry::Check { id, label, checked } => {
                let item = CheckMenuItemBuilder::with_id(*id, label)
                    .checked(*checked)
                    .build(app)?;
                builder = builder.item(&item);
            }
            MenuEntry::Item { id, label, enabled } => {
                let item = MenuItemBuilder::with_id(*id, label)
                    .enabled(*enabled)
                    .build(app)?;
                builder = builder.item(&item);
            }
            MenuEntry::Separator => {
                let separator = PredefinedMenuItem::separator(app)?;
                builder = builder.item(&separator);
            }
        }
    }
    builder.build()
}

/// 依目前狀態（編輯版面旗標、暫停與否）重建選單並套用到系統匣圖示。啟動時（`main.rs` setup）
/// 與每次選單事件改變狀態後都呼叫一次；七個項目重建成本低，不值得只更新變化的那一項。
pub fn refresh(app: &AppHandle) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        log::warn!("系統匣：找不到 id 為 \"{TRAY_ID}\" 的圖示，無法更新選單");
        return;
    };
    let edit_mode = *app
        .state::<AppState>()
        .edit_mode
        .lock()
        .expect("edit_mode mutex poisoned");
    let (manual_active, other_reason) = widgets::manual_pause_status(app);
    let no_space_hidden = widgets::current_no_space_hidden(app);
    let entries = build_menu_entries(
        edit_mode,
        manual_active,
        other_reason,
        &no_space_hidden,
        env!("CARGO_PKG_VERSION"),
        crate::updater::available_update(app).as_deref(),
    );
    match materialize_menu(app, &entries) {
        Ok(menu) => {
            if let Err(err) = tray.set_menu(Some(menu)) {
                log::error!("系統匣：套用選單失敗：{err}");
            }
        }
        Err(err) => log::error!("系統匣：建立選單失敗：{err}"),
    }
}

/// `main.rs` setup 呼叫一次：把選單掛上啟動時已存在的系統匣圖示，並記下圖示視窗（系統匣通知用，
/// 見 [`show_notification`]）。
pub fn register(app: &AppHandle) {
    refresh(app);
    cache_tray_hwnd(app);
}

// ── 系統匣通知（dynamic-wallpaper task 4.8）────────────────────────────────────────────

/// 系統匣圖示的隱藏視窗（`tray-icon` 為每個圖示建立；圖示存在期間不變，explorer 重新啟動時
/// `tray-icon` 以同一個視窗重新登記）。0＝還沒取得。
static TRAY_HWND: AtomicIsize = AtomicIsize::new(0);

/// 取得並記下系統匣圖示的視窗。`with_inner_tray_icon` 在主執行緒執行並等待結果——`register` 在
/// setup（主執行緒）呼叫時直接執行；工作執行緒呼叫時等主執行緒處理。
fn cache_tray_hwnd(app: &AppHandle) -> isize {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        log::warn!("系統匣：找不到 id 為 \"{TRAY_ID}\" 的圖示，無法發系統匣通知");
        return 0;
    };
    match tray.with_inner_tray_icon(|inner| inner.window_handle() as isize) {
        Ok(hwnd) => {
            TRAY_HWND.store(hwnd, Ordering::Relaxed);
            hwnd
        }
        Err(err) => {
            log::warn!("系統匣：取得圖示視窗失敗，無法發系統匣通知：{err}");
            0
        }
    }
}

/// 以系統匣通知顯示 `text`（讓位、安全閥、等焦點確認）。**立即返回**：通知在用完即丟的工作執行緒
/// 送出（`Shell_NotifyIconW` 是送給 explorer 的同步呼叫，explorer 卡住時不能拖住呼叫端；見
/// `desktop::tray_balloon` 模組文件「執行緒」）。失敗只記錄（通知盡力而為，狀態也可在設定視窗看到）。
pub fn show_notification(app: &AppHandle, text: crate::wallpaper_settings::NotificationText) {
    let handle = app.clone();
    let spawned = thread::Builder::new()
        .name("fc-tray-notify".to_owned())
        .spawn(move || {
            let mut hwnd = TRAY_HWND.load(Ordering::Relaxed);
            if hwnd == 0 {
                hwnd = cache_tray_hwnd(&handle);
            }
            match crate::desktop::tray_balloon::show_balloon(
                hwnd,
                &text.title,
                &text.body,
                text.kind,
            ) {
                Ok(id) => log::info!(
                    "系統匣通知已送出（uID {id}）：{}｜{}",
                    text.title,
                    text.body
                ),
                Err(err) => {
                    log::warn!("系統匣通知送出失敗（{err}）：{}｜{}", text.title, text.body)
                }
            }
        });
    if let Err(err) = spawned {
        log::error!("系統匣通知：建立工作執行緒失敗：{err}");
    }
}

// ── 選單事件分派 ────────────────────────────────────────────────────────────────────

/// `tauri::Builder::on_menu_event` 的全域處理常式（`main.rs` 註冊）。
pub fn handle_menu_event(app: &AppHandle, event: MenuEvent) {
    match event.id().0.as_str() {
        MENU_ID_EDIT_MODE => toggle_edit_mode(app),
        MENU_ID_OPEN_SETTINGS => open_settings_window(app),
        MENU_ID_PAUSE_TOGGLE => toggle_manual_pause(app),
        MENU_ID_INSTALL_UPDATE => crate::updater::install_from_menu(app),
        MENU_ID_QUIT => quit(app),
        // MENU_ID_VERSION：enabled=false，正常情況下不會收到事件；防禦性忽略而非 panic。
        other => log::debug!("系統匣：忽略未知或不可點的選單項目 id={other}"),
    }
}

/// 「編輯版面」：切換旗標（沿用 [`widgets::set_edit_mode`]，與小工具頁面觸發的路徑相同、廣播
/// 邏輯不重複一份），再重整選單讓勾選狀態跟上。
fn toggle_edit_mode(app: &AppHandle) {
    let state = app.state::<AppState>();
    let next = {
        let current = *state.edit_mode.lock().expect("edit_mode mutex poisoned");
        !current
    };
    // fix F2（review 5.3 low）：鎖定旗標存檔失敗時不切換（錯誤已在 set_edit_mode 記錄），選單
    // 勾選狀態依實際旗標重整。
    match widgets::set_edit_mode(next, state, app.clone()) {
        Ok(()) => log::info!("系統匣：編輯版面切換為 {next}"),
        Err(err) => log::warn!("系統匣：編輯版面切換為 {next} 失敗，維持原狀：{err}"),
    }
    refresh(app);
}

/// 「設定」：開啟（或聚焦既有的）設定視窗（`host/ui/settings.html`，task 5.2）。
///
/// ## 為什麼建立視窗那段要丟到獨立執行緒（task 5.2 驗收時發現）
///
/// 本函式有兩個呼叫端：`tray.rs` 選單事件（`handle_menu_event`，在 tao 事件迴圈上，呼叫
/// `.build()` 本身安全）與 `main.rs` 的 `tauri_plugin_single_instance::init` 回呼。後者由
/// **第二個行程**送 `WM_COPYDATA` 觸發——`tauri-plugin-single-instance-2.5.0/src/
/// platform_impl/windows.rs` 的 `UserData::run_callback` 是第一個執行個體那扇隱藏視窗的
/// `WndProc` 直接呼叫使用者回呼，而 `SendMessageW`（第二個行程用來送 `WM_COPYDATA`）是**
/// 跨行程同步呼叫**：發送端會一直卡在 `SendMessageW`，直到接收端的 `WndProc` 處理完那則
/// 訊息才返回。若在這個回呼裡直接同步呼叫 `WebviewWindowBuilder::build()`，就是在「處理
/// 另一個視窗訊息的當下」又建立一整個新的 WebView2 視窗——這與
/// `widgets.rs::update_settings` 文件記載的「同步指令內呼叫 `build()` 在 Windows 上死鎖
/// （wry#583）」屬於同一類「在不該同步呼叫 `build()` 的情境下同步呼叫」問題，只是觸發路徑
/// 從「Tauri 指令」換成「WM_COPYDATA 的 WndProc」。
///
/// 驗收（`host/tools/verify-5.2.ps1`）實測到的具體症狀：手動重複啟動的第二個行程遲遲不結束
/// （`SendMessageW` 卡住，逾時後才強制關閉）、既有執行個體的設定視窗遲遲不出現在 CDP 除錯埠
/// 的 `/json/list`——耗時從幾乎瞬間到超過 90 秒不等（不是穩定重現，符合「同步呼叫剛好卡進
/// 一個不該卡的訊息處理路徑」這種時機敏感的問題特徵，而不是單純的「WebView2 在鎖定工作階段
/// 下比較慢」；`.focused(false)`＝不在建立當下搶焦點——這件事本身仍值得保留，鎖定時搶前景會
/// 被 Windows 拒絕——但單獨這個改動並未穩定解決卡住的問題，改成獨立執行緒才是對症的修法）。
///
/// 修法與 `update_settings` 完全同構：把「建立視窗」整段搬到獨立執行緒——該執行緒呼叫
/// `.build()` 只是把「建立視窗」的請求送進 tao 事件迴圈並等待完成，本函式（也就是
/// `WndProc`／`SendMessageW` 呼叫端）立刻返回，不佔著訊息處理的呼叫堆疊等一整個 WebView2
/// 視窗建好。`app: &AppHandle` 需要 `.clone()` 給執行緒（`AppHandle` 本身設計成可以跨執行緒
/// 使用）。
pub fn open_settings_window(app: &AppHandle) {
    // installer-auto-update task 3.3（審查 I1）：正在因更新結束時不開（會在視窗已銷毀後又建出視窗）。
    if crate::updater::is_exiting_for_update() {
        log::info!("設定視窗：正在因更新結束，不開啟");
        return;
    }
    if let Some(window) = app.get_webview_window(SETTINGS_WINDOW_LABEL) {
        log::info!("設定視窗：已存在，改為顯示並取得焦點");
        if let Err(err) = window.unminimize() {
            log::warn!("設定視窗：取消最小化失敗：{err}");
        }
        if let Err(err) = window.show() {
            log::warn!("設定視窗：顯示失敗：{err}");
        }
        if let Err(err) = window.set_focus() {
            log::warn!("設定視窗：取得焦點失敗：{err}");
        }
        return;
    }

    let handle = app.clone();
    thread::spawn(move || create_settings_window(&handle));
}

/// [`open_settings_window`] 實際建立新視窗那段（見該函式文件：獨立執行緒呼叫，避免在
/// single-instance 的 `WM_COPYDATA` `WndProc` 內同步呼叫 `.build()`）。
///
/// task 5.2：settings.html 從空白佔位頁換成完整表單（外觀、資料目錄、十個小工具、暫停規則、
/// 自動啟動），480x400 放不下，改成可捲動的較大預設尺寸；仍保留 `resizable(true)` 讓使用者
/// 自行調整，`min_inner_size` 避免縮到表單擠壞。design.md D10：一般視窗（可取得焦點、非
/// 置底），不套用小工具的 `focusable(false)`／`always_on_bottom(true)` 等桌面層屬性——
/// `focusable` 維持預設（true），使用者仍可點擊取得焦點。
///
/// `.focused(false)`：只影響「建立當下是否試圖搶焦點」（tao `with_focused` 文件："Whether
/// the window will be **initially** focused"，不是永久禁用焦點）；工作階段鎖定中 Windows 會
/// 拒絕背景行程取得前景/焦點，建立當下不搶焦點可避免這個請求卡住或被拒後的不確定行為，建立
/// 完成後另外補一次 `set_focus()`（見下方）讓一般情況下使用者仍會看到視窗取得焦點。
fn create_settings_window(app: &AppHandle) {
    // task 4.5：小工具那組的 WebView2 使用者資料夾（見 crate::webview_env）。
    let result = crate::webview_env::with_widget_data_dir(WebviewWindowBuilder::new(
        app,
        SETTINGS_WINDOW_LABEL,
        WebviewUrl::App("settings.html".into()),
    ))
    .title("財經桌布設定")
    .inner_size(560.0, 760.0)
    .min_inner_size(420.0, 480.0)
    .resizable(true)
    .focused(false)
    .visible(true)
    .build();
    match result {
        Ok(window) => {
            log::info!("設定視窗：已開啟");
            // dynamic-wallpaper task 5.2：建立時是預設（統一）圖示，換成目前主題的圖示。
            crate::app_icon::request_refresh(app);
            if let Err(err) = window.set_focus() {
                log::warn!("設定視窗：取得焦點失敗（可能工作階段已鎖定）：{err}");
            }
        }
        Err(err) => log::error!("設定視窗：建立失敗：{err}"),
    }
}

/// 「暫停／繼續」：只切換 [`PauseReason::Manual`] 這一個原因的啟用狀態（design.md D12「暫停
/// 原因集合」；`widgets.rs` 模組文件「task 5.1 補上暫停原因集合」一節）——若使用者手動繼續時
/// 其他原因仍在，整體仍會維持暫停（原因集合的聯集語意），這是正確語意（task 5.5 controller
/// 裁決，見 `task-5.5-brief.md`）；選單文字（[`pause_toggle_label`]）改為附註仍在的原因，
/// 不再只顯示可能誤導使用者的「繼續」。[`widgets::set_pause_reason`] 內部已呼叫 [`refresh`]，
/// 這裡不重複呼叫。
fn toggle_manual_pause(app: &AppHandle) {
    let currently_manual = {
        let state = app.state::<AppState>();
        let reasons = state
            .pause_reasons
            .lock()
            .expect("pause_reasons mutex poisoned");
        reasons.contains(&PauseReason::Manual)
    };
    let paused = widgets::set_pause_reason(app, PauseReason::Manual, !currently_manual);
    log::info!(
        "系統匣：手動暫停切換為 {}（整體 paused={paused}）",
        !currently_manual
    );
}

/// 「結束」：關閉所有小工具視窗並結束行程，且不被自動重新啟動
/// （specs/widget-host-lifecycle「系統匣常駐」Scenario「結束宿主」）。`app.exit(0)` 送出
/// `RunEvent::ExitRequested { code: Some(0), .. }`，`main.rs` 的 `run` 回呼只對 `code: None`
/// 呼叫 `prevent_exit()`，故本呼叫照常放行、行程結束。
///
/// `pub(crate)`（而非私有）：task 5.7 的 `self_test_ipc::self_test_quit`（僅
/// `self-test-ipc` feature）需要呼叫與系統匣選單完全相同的這一條路徑，才能驗證
/// `host/tools/rm-restart-test.ps1` 的「從系統匣結束後 RM 不重啟」——以 CDP `invoke` 觸發，
/// 不模擬點擊選單（不注入輸入）。
pub(crate) fn quit(app: &AppHandle) {
    log::info!("系統匣：使用者選擇「結束」");
    // installer-auto-update task 3.1（design.md D3）：UI 建好之前（舊標記的過渡期，或無標記時 setup 同步建立中——最終審查
    // M2）「結束」轉到狀態機的「結束」：取消更新任務、不還原桌布、不存主題，刪除標記後結束；同步安裝進行中則忽略
    // （安裝完會自行結束）。UI 已建立（一般情況）走下面原本的流程。
    match quit_action(crate::updater::on_tray_quit(app)) {
        QuitAction::Proceed => {}
        QuitAction::ExitWithoutRestore => {
            crate::updater::quit_during_transition(app);
            return;
        }
        QuitAction::Ignore => {
            log::info!("系統匣：更新安裝或結束已在進行中，忽略這次「結束」");
            return;
        }
    }
    // installer-auto-update 3.2（審查 M1）：與「因更新結束」搶同一個結束擁有權（CAS）。更新的 `on_before_exit` 掛鉤
    // 若已先搶到，這次「結束」就忽略（桌布不得在更新時被還原）；反過來我們先搶到，掛鉤搶不到就中止安裝。必須在
    // 任何收尾動作（解除重啟註冊、刪標記、還原桌布）之前。
    if !crate::updater::claim_tray_quit() {
        log::info!("系統匣：因更新結束已搶先取得結束擁有權，忽略這次「結束」（不還原桌布）");
        return;
    }
    // task 5.7：先取消重新啟動註冊（design.md D12：「使用者從系統匣結束時先
    // UnregisterApplicationRestart」），確保這種主動結束不會被 Restart Manager 之後的重新啟動
    // 要求誤判為需要恢復（specs「Restart Manager 關閉後重新啟動」Requirement 的 MUST NOT
    // 那句）。`tauri-plugin-single-instance` 釋放具名 mutex／事件視窗的清理已掛在
    // `RunEvent::Exit`（crate 內部自動處理，見本檔模組文件），不需要在這裡另外呼叫
    // `tauri_plugin_single_instance::destroy`。
    crate::desktop::unregister_restart();
    // installer-auto-update task 3.1：使用者主動結束是正常結束路徑，刪除當機迴圈保護標記。
    crate::updater::clear_startup_marker();

    // data-layer-rust task 5.2：先通知抓取排程停止（只設旗標、不等；進行中的一輪在下一個來源邊界停下、
    // 不寫半份檔）。等它收尾放在下面的結束執行緒，主執行緒不等；沒有結束執行緒的路徑照舊立即結束。
    crate::fetch::scheduler::request_stop_app(app);

    // dynamic-wallpaper task 4.7b（spec「還原原桌布」Scenario「系統匣結束」）：有桌布協調迴圈時，
    // 結束前先請它停止渲染並還原原桌布（含全域填滿方式），等它回報（上限見 `ExitLimits`）後才結束。
    // 等待在獨立執行緒：還原要列舉顯示器（需要主執行緒往返），主執行緒不能卡著等。沒有協調迴圈
    // （Secondary 不會走到這裡；`--self-test-render`、協調迴圈啟動失敗）時照舊立即結束。
    let Some(handle) = app
        .try_state::<crate::wallpaper_coordinator::CoordinatorHandle>()
        .map(|h| h.inner().clone())
    else {
        // 沒有協調迴圈（`--self-test-render`、協調迴圈啟動失敗）：主題不是「不接管」才改並存檔。
        if let Err(e) = crate::widgets::set_wallpaper_theme_none(app) {
            log::error!("系統匣「結束」：桌布主題存成「不接管」失敗：{e}");
        }
        app.exit(0);
        return;
    };
    if QUIT_STARTED.swap(true, Ordering::SeqCst) {
        log::info!("系統匣：結束已在進行中（等桌布還原），忽略重複的「結束」");
        return;
    }
    // 4.5 審查建議：等還原的這幾秒先把圖示藏起來，使用者不會以為沒反應而再點。
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        if let Err(err) = tray.set_visible(false) {
            log::warn!("系統匣：隱藏圖示失敗：{err}");
        }
    }
    let exit_app = app.clone();
    let spawned = thread::Builder::new()
        .name("fc-tray-exit".to_owned())
        .spawn(move || {
            crate::wallpaper_coordinator::exit_and_restore(
                &handle,
                crate::wallpaper_coordinator::ExitLimits::default(),
            );
            save_theme_none_on_exit(&exit_app);
            settle_renderer_browser_on_exit(&exit_app);
            // data-layer-rust task 5.2：等抓取排程停下（上限 `QUIT_STOP_TIMEOUT`，超時放棄等待照常結束）。
            crate::fetch::scheduler::stop_app(
                &exit_app,
                crate::fetch::scheduler::QUIT_STOP_TIMEOUT,
            );
            exit_app.exit(0);
        });
    if let Err(err) = spawned {
        log::error!("系統匣：無法建立結束執行緒（{err}），不還原桌布、立即結束");
        save_theme_none_on_exit(app);
        app.exit(0);
    }
}

/// 系統匣「結束」對更新器處置的反應（installer-auto-update task 3.1）。只有 `Proceed` 會走下面原本的流程——
/// 那條流程在找不到協調迴圈時會 `set_wallpaper_theme_none`（把桌布主題永久存成「不接管」），所以 UI 尚未
/// 存在（過渡期、建立 UI 尚未執行）的結束必須走 `ExitWithoutRestore`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuitAction {
    /// 原本的結束流程（還原桌布、存主題）。
    Proceed,
    /// 不還原桌布、不存主題、刪標記後結束。
    ExitWithoutRestore,
    /// 忽略這次按下。
    Ignore,
}

fn quit_action(disposition: crate::updater::QuitDisposition) -> QuitAction {
    use crate::updater::QuitDisposition as D;
    match disposition {
        D::Normal => QuitAction::Proceed,
        D::QuitDuringTransition => QuitAction::ExitWithoutRestore,
        D::IgnoreInstalling | D::AlreadyQuitting => QuitAction::Ignore,
    }
}

/// dynamic-wallpaper task 6.4（審查 R1 low）：系統匣「結束」一律把桌布主題存成「不接管」——不論還原
/// 是否在時限內完成（協調迴圈處理要求時已改並存檔；這裡補上它沒在時限內處理、或存檔失敗的情況）。
/// 下次啟動因此不接管；還原沒做完時，狀態檔的「還原進行中」標記讓下次啟動續做。
fn save_theme_none_on_exit(app: &AppHandle) {
    // installer-auto-update task 3.3（審查 I1）：正在因更新結束時不得把主題存成「不接管」（新版啟動後要照常接管）。
    if crate::updater::is_exiting_for_update() {
        log::warn!("系統匣「結束」：正在因更新結束，不把桌布主題存成「不接管」");
        return;
    }
    match crate::widgets::ensure_wallpaper_theme_none_saved(app) {
        Ok(()) => log::info!("系統匣「結束」：桌布主題已存成「不接管」"),
        Err(e) => log::error!("系統匣「結束」：桌布主題存成「不接管」失敗：{e}"),
    }
}

/// bug-leaked-renderer：結束前確保最後一次桌布渲染的 browser 行程已結束（卡住就由宿主結束它，
/// `Renderer::shutdown`）；6.2 長跑的殘留行程在宿主結束後仍存活數小時。只在結束執行緒呼叫（最多等
/// `BROWSER_EXIT_WAIT`＋`BROWSER_KILL_WAIT`）。沒有協調迴圈的那條路不需要：渲染只由協調迴圈發起
/// （`--self-test-render` 在自己的執行緒收尾時處置）。
fn settle_renderer_browser_on_exit(app: &AppHandle) {
    if let Some(renderer) = app.try_state::<crate::wallpaper_render::WallpaperRenderer>() {
        let settled = renderer.shutdown();
        log::info!("系統匣「結束」：最後一個桌布渲染 browser 行程 {settled:?}");
    }
}

/// 系統匣「結束」已開始（等桌布還原中）：重複的「結束」忽略。
static QUIT_STARTED: AtomicBool = AtomicBool::new(false);

/// 系統匣「結束」是否已開始（等桌布還原中）。安裝更新前檢查：已在結束就不得啟動安裝
/// （installer-auto-update task 3.2；兩邊各自收尾會互相破壞）。
pub(crate) fn quit_started() -> bool {
    QUIT_STARTED.load(Ordering::SeqCst)
}

// ── 單一執行個體（純函式部分）────────────────────────────────────────────────────────

/// design.md D12／specs/widget-host-lifecycle「單一執行個體」：第二個執行個體的命令列參數
/// （`tauri_plugin_single_instance` 回呼收到的 `args`，第一個元素是執行檔路徑）帶
/// `--autostart` 或 `--restarted` 之一，即視為自動化的重複啟動（開機自啟或系統重新啟動觸發），
/// 須靜默忽略；其餘（含使用者手動再次執行）視為手動重複啟動。純函式，見 `tray_tests`。
pub fn is_automated_relaunch(args: &[String]) -> bool {
    args.iter()
        .any(|a| a == "--autostart" || a == "--restarted")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── build_menu_entries：內容、順序、狀態反映 ────────────────────────────────

    #[test]
    fn menu_entries_are_in_brief_order_with_expected_ids() {
        let entries = build_menu_entries(false, false, None, &[], "0.1.0", None);
        let ids: Vec<Option<&str>> = entries
            .iter()
            .map(|e| match e {
                MenuEntry::Check { id, .. } | MenuEntry::Item { id, .. } => Some(*id),
                MenuEntry::Separator => None,
            })
            .collect();
        assert_eq!(
            ids,
            vec![
                Some(MENU_ID_EDIT_MODE),
                Some(MENU_ID_OPEN_SETTINGS),
                Some(MENU_ID_PAUSE_TOGGLE),
                None,
                Some(MENU_ID_VERSION),
                None,
                Some(MENU_ID_QUIT),
            ],
            "順序須符合 tasks.md 5.1：編輯版面、設定、暫停／繼續、版本號、結束"
        );
    }

    #[test]
    fn edit_mode_entry_reflects_checked_state() {
        let unchecked = build_menu_entries(false, false, None, &[], "0.1.0", None);
        assert_eq!(
            unchecked[0],
            MenuEntry::Check {
                id: MENU_ID_EDIT_MODE,
                label: "編輯版面".to_string(),
                checked: false,
            }
        );

        let checked = build_menu_entries(true, false, None, &[], "0.1.0", None);
        assert_eq!(
            checked[0],
            MenuEntry::Check {
                id: MENU_ID_EDIT_MODE,
                label: "編輯版面".to_string(),
                checked: true,
            }
        );
    }

    // ── 暫停／繼續文字（task 5.5：manual_active × other_reason 四種組合）────────────

    #[test]
    fn pause_entry_shows_plain_pause_when_nothing_active() {
        let entries = build_menu_entries(false, false, None, &[], "0.1.0", None);
        assert_eq!(
            entries[2],
            MenuEntry::Item {
                id: MENU_ID_PAUSE_TOGGLE,
                label: "暫停".to_string(),
                enabled: true,
            }
        );
    }

    #[test]
    fn pause_entry_shows_plain_resume_when_only_manual_active() {
        let entries = build_menu_entries(false, true, None, &[], "0.1.0", None);
        assert_eq!(
            entries[2],
            MenuEntry::Item {
                id: MENU_ID_PAUSE_TOGGLE,
                label: "繼續".to_string(),
                enabled: true,
            }
        );
    }

    #[test]
    fn pause_entry_annotates_other_reason_when_manual_not_active() {
        // 使用者從未點過暫停，但系統因為鎖定已經自動暫停中：文字要能讓使用者知道點「暫停」
        // 不會有任何可見變化（已經是暫停狀態）。
        let entries =
            build_menu_entries(false, false, Some(PauseReason::Locked), &[], "0.1.0", None);
        assert_eq!(
            entries[2],
            MenuEntry::Item {
                id: MENU_ID_PAUSE_TOGGLE,
                label: "暫停（目前已因鎖定暫停中）".to_string(),
                enabled: true,
            }
        );
    }

    #[test]
    fn pause_entry_annotates_other_reason_when_manual_active() {
        // 使用者點過暫停，但同時系統也因全螢幕自動暫停：點「繼續」只會撤掉手動這一個原因，
        // 整體仍會維持暫停，文字要如實反映。
        let entries = build_menu_entries(
            false,
            true,
            Some(PauseReason::SystemBusy),
            &[],
            "0.1.0",
            None,
        );
        assert_eq!(
            entries[2],
            MenuEntry::Item {
                id: MENU_ID_PAUSE_TOGGLE,
                label: "繼續（仍因全螢幕或忙碌暫停中）".to_string(),
                enabled: true,
            }
        );
    }

    #[test]
    fn version_entry_shows_version_and_is_not_clickable() {
        let entries = build_menu_entries(false, false, None, &[], "1.2.3", None);
        assert_eq!(
            entries[4],
            MenuEntry::Item {
                id: MENU_ID_VERSION,
                label: "版本 1.2.3".to_string(),
                enabled: false,
            },
            "版本號只顯示、不可點（enabled=false）"
        );
    }

    #[test]
    fn quit_entry_is_present_and_clickable() {
        let entries = build_menu_entries(false, false, None, &[], "0.1.0", None);
        assert_eq!(
            entries[6],
            MenuEntry::Item {
                id: MENU_ID_QUIT,
                label: "結束".to_string(),
                enabled: true,
            }
        );
    }

    #[test]
    fn separators_flank_the_version_entry() {
        let entries = build_menu_entries(false, false, None, &[], "0.1.0", None);
        assert_eq!(entries[3], MenuEntry::Separator);
        assert_eq!(entries[5], MenuEntry::Separator);
    }

    // ── 空間不足暫時隱藏（task 7.4）──────────────────────────────────────────────

    #[test]
    fn no_space_hidden_entries_appear_disabled_between_pause_and_version_separator() {
        let hidden = [
            ("custom3", "空間不足，暫時隱藏：擴充插槽 3".to_string()),
            ("quotes", "空間不足，暫時隱藏：行情條".to_string()),
        ];
        let entries = build_menu_entries(false, false, None, &hidden, "0.1.0", None);
        assert_eq!(
            entries,
            vec![
                MenuEntry::Check {
                    id: MENU_ID_EDIT_MODE,
                    label: "編輯版面".to_string(),
                    checked: false,
                },
                MenuEntry::Item {
                    id: MENU_ID_OPEN_SETTINGS,
                    label: "設定".to_string(),
                    enabled: true,
                },
                MenuEntry::Item {
                    id: MENU_ID_PAUSE_TOGGLE,
                    label: "暫停".to_string(),
                    enabled: true,
                },
                MenuEntry::Separator,
                MenuEntry::Item {
                    id: "custom3",
                    label: "空間不足，暫時隱藏：擴充插槽 3".to_string(),
                    enabled: false,
                },
                MenuEntry::Item {
                    id: "quotes",
                    label: "空間不足，暫時隱藏：行情條".to_string(),
                    enabled: false,
                },
                MenuEntry::Separator,
                MenuEntry::Item {
                    id: MENU_ID_VERSION,
                    label: "版本 0.1.0".to_string(),
                    enabled: false,
                },
                MenuEntry::Separator,
                MenuEntry::Item {
                    id: MENU_ID_QUIT,
                    label: "結束".to_string(),
                    enabled: true,
                },
            ],
            "順序：編輯版面、設定、暫停／繼續、分隔線、每個隱藏項目、分隔線、版本號、分隔線、結束"
        );
    }

    #[test]
    fn no_space_hidden_empty_keeps_original_six_entry_structure() {
        // 沒有任何小工具因空間不足暫時隱藏時，選單結構應與 task 5.1 原本完全一致（不多出
        // 空的分隔線）。
        let entries = build_menu_entries(false, false, None, &[], "0.1.0", None);
        assert_eq!(entries.len(), 7, "六個項目＋兩條分隔線");
        assert_eq!(entries[3], MenuEntry::Separator);
        assert_eq!(
            entries[4],
            MenuEntry::Item {
                id: MENU_ID_VERSION,
                label: "版本 0.1.0".to_string(),
                enabled: false,
            }
        );
    }

    // ── 系統匣「結束」對更新器處置的反應（installer-auto-update 3.1 審查 I2）──────────

    #[test]
    fn only_normal_disposition_reaches_the_theme_saving_quit_path() {
        use crate::updater::QuitDisposition as D;
        // `quit()` 裡只有 `Proceed` 會往下走到 `set_wallpaper_theme_none`／`exit_and_restore`；
        // UI 尚未存在（過渡期等待中、建立 UI 已決定但尚未執行）與重複按下都不得走到。
        assert_eq!(quit_action(D::Normal), QuitAction::Proceed);
        assert_eq!(
            quit_action(D::QuitDuringTransition),
            QuitAction::ExitWithoutRestore,
            "建立 UI 尚未執行時的結束也是這一條（transition::quit_disposition_by_phase 驗證對應）"
        );
        assert_eq!(quit_action(D::AlreadyQuitting), QuitAction::Ignore);
        assert_eq!(quit_action(D::IgnoreInstalling), QuitAction::Ignore);
    }

    // ── 更新選單項（installer-auto-update task 3.2）──────────────────────────────

    #[test]
    fn update_entry_appears_after_pause_only_when_an_update_is_ready() {
        let none = build_menu_entries(false, false, None, &[], "0.1.0", None);
        assert!(
            !none
                .iter()
                .any(|e| matches!(e, MenuEntry::Item { id, .. } if *id == MENU_ID_INSTALL_UPDATE)),
            "沒有已下載的新版：不顯示更新項"
        );
        let some = build_menu_entries(false, false, None, &[], "0.1.0", Some("0.1.1"));
        assert_eq!(
            some[3],
            MenuEntry::Item {
                id: MENU_ID_INSTALL_UPDATE,
                label: "更新到 v0.1.1 並重新啟動".to_string(),
                enabled: true,
            },
            "緊接在「暫停／繼續」之後、可點"
        );
        // 其餘項目與沒有更新時一致（只多一項）。
        assert_eq!(some.len(), none.len() + 1);
        assert_eq!(&some[..3], &none[..3]);
        assert_eq!(&some[4..], &none[3..]);
    }

    #[test]
    fn menu_event_dispatch_routes_the_update_item_to_the_updater() {
        // `handle_menu_event` 需要 `MenuEvent`（無法在單元測試建構），以原始碼斷言分派存在且不經主執行緒同步安裝。
        let src = include_str!("tray.rs");
        assert!(src.contains("MENU_ID_INSTALL_UPDATE => crate::updater::install_from_menu(app)"));
    }

    #[test]
    fn quit_claims_the_exit_owner_before_any_teardown_step() {
        // 審查 M1：`quit` 在 `quit_action` 之後、解除重啟註冊／刪標記／還原之前搶結束擁有權；搶輸就返回。
        let src = include_str!("tray.rs");
        let quit_fn = src
            .split("pub(crate) fn quit(app: &AppHandle) {")
            .nth(1)
            .expect("找得到 quit");
        let claim = quit_fn
            .find("crate::updater::claim_tray_quit()")
            .expect("quit 要搶結束擁有權");
        let action = quit_fn.find("quit_action(").expect("quit_action");
        let unregister = quit_fn
            .find("crate::desktop::unregister_restart()")
            .expect("unregister_restart");
        let marker = quit_fn
            .find("crate::updater::clear_startup_marker()")
            .expect("clear_startup_marker");
        assert!(action < claim, "先問過渡期狀態機");
        assert!(
            claim < unregister && claim < marker,
            "搶擁有權要早於任何收尾動作"
        );
        let after_claim = &quit_fn[claim..unregister];
        assert!(after_claim.contains("return;"), "搶輸就返回、什麼都不做");
    }

    #[test]
    fn quit_started_reflects_the_quit_flag() {
        // 只讀不寫全域旗標（其他測試並行）；預設為假＝可以安裝。
        assert!(!quit_started());
    }

    // ── is_automated_relaunch：single-instance 回呼的參數判斷 ──────────────────────

    #[test]
    fn manual_relaunch_with_only_exe_path_is_not_automated() {
        assert!(!is_automated_relaunch(&["fc-host.exe".to_string()]));
    }

    #[test]
    fn manual_relaunch_with_unrelated_args_is_not_automated() {
        assert!(!is_automated_relaunch(&[
            "fc-host.exe".to_string(),
            "--self-test-ipc".to_string()
        ]));
    }

    #[test]
    fn empty_args_is_not_automated() {
        assert!(!is_automated_relaunch(&[]));
    }

    #[test]
    fn autostart_flag_is_automated() {
        assert!(is_automated_relaunch(&[
            "fc-host.exe".to_string(),
            "--autostart".to_string()
        ]));
    }

    #[test]
    fn restarted_flag_is_automated() {
        assert!(is_automated_relaunch(&[
            "fc-host.exe".to_string(),
            "--restarted".to_string()
        ]));
    }

    #[test]
    fn flag_position_does_not_matter() {
        assert!(is_automated_relaunch(&[
            "--autostart".to_string(),
            "fc-host.exe".to_string(),
        ]));
    }
}
