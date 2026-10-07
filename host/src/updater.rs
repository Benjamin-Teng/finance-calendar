//! 宿主端更新器（installer-auto-update task 3.1／3.2／3.3；design.md D1、D3、D4、D7）：檢查、下載、驗簽、
//! 當機迴圈保護、安裝觸發（系統匣選單項與通知、`--autostart` 後 10 分鐘內自動安裝、`update-state.json`
//! 防循環）、更新前結束流程與失敗退路。
//!
//! ## 模組分工
//!
//! | 檔案 | 內容 | 能否不靠 Tauri 單元測試 |
//! |---|---|---|
//! | [`gate`] | 更新器啟用判定（占位公鑰、`dangerous*` 後門旗標、壞設定） | 是 |
//! | [`schedule`] | 啟動方式 → 檢查排程（60–180 秒／`--autostart` 網路就緒後／舊標記立即） | 是 |
//! | [`marker`] | `startup-marker` 檔的生命週期（只由主行程讀寫） | 是（暫存資料夾） |
//! | [`transition`] | 過渡期原子狀態機（等待→安裝／建立 UI／結束只轉移一次、交接排隊） | 是 |
//! | [`engine`] | 決策核心：週期、結果處置、`catch_unwind` 隔離；後端／副作用／計時器皆注入 | 是（假物件） |
//! | [`exit`] | 「因更新結束」的收尾（D4 時間預算表）與 `install()` 失敗後的重新啟動（task 3.3） | 是（假步驟） |
//! | [`install`] | 安裝觸發規則（手動／自動／過渡期）、10 分鐘窗口、24 小時退避、嘗試紀錄、兩種失敗點的處置、重新啟動參數（task 3.2） | 是（假的 `InstallSystem`） |
//! | [`temp_cleanup`] | 清掉外掛留在 `%TEMP%` 的過期安裝檔暫存資料夾（背景、命名樣式整串比對、失敗只記錄） | 是（暫存資料夾） |
//! | [`state_file`] | `update-state.json` 格式與讀寫 | 是 |
//! | [`version`] | SemVer 比較（深度防禦：不降版） | 是 |
//! | `plugin_backend` | 把 `tauri-plugin-updater` 包成後端與 `InstallSystem`（逾時、錯誤歸類、`on_before_exit` 掛鉤、失敗點判別） | 掛鉤與失敗點判別是（純邏輯），其餘否 |
//! | 本檔 | 組裝：啟動前準備（標記）、setup 內啟動、給 `main.rs`／`tray.rs` 的薄接點 | 否 |
//!
//! ## 啟動順序（`main.rs`）
//!
//! ```text
//! main()
//!   ├─ --restore-wallpaper / --fetch-once ──▶ 在仲裁之前就 exit：完全不會走到這裡（StartMode 再表達一次）
//!   ├─ arbitrate_startup()  ──▶ role = Primary / Secondary / Unarbitrated
//!   ├─ evaluate_gate()      ──▶ 設定與建置旗標決定要不要註冊外掛（失敗也不得拖垮宿主）
//!   ├─ prepare()            ──▶ 只有 Primary：讀舊標記、寫新標記、啟動 5 分鐘到期計時
//!   ├─ tauri::Builder … .plugin(updater)（只在啟用時）
//!   └─ setup: start()       ──▶ 更新任務先啟動；無舊標記→立即（同步）建立 UI，建完才轉到「UI 已建立」；
//!                              有舊標記→過渡期（不阻塞 setup），由狀態機決定何時建立 UI
//! ```
//!
//! 外掛的 capability **不開給任何 webview**（`capabilities/default.json` 沒有 `updater:*`）：更新完全由
//! 後端主導，頁面不能呼叫 `plugin:updater|*`。

mod cancel;
mod engine;
mod exit;
mod gate;
mod install;
mod marker;
mod plugin_backend;
mod schedule;
mod state_file;
mod temp_cleanup;
mod transition;
mod version;

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::plugin::TauriPlugin;
use tauri::{AppHandle, Manager, Wry};

use crate::desktop::StartupRole;
use cancel::RealTimer;
use engine::{BuildTicket, Effects, InstallReady, UpdaterCore};
use gate::Gate;
use marker::{PreviousMarker, StartupMarker, MARKER_LIFETIME};
use schedule::{plan_first_check, StartMode};
use transition::Transition;
pub use transition::{HandoffDisposition, QuitDisposition};
// `mark_main_thread` 在 `main()` 開頭呼叫；`is_exiting_for_update` 給 `tray`／`widgets`／`recovery` 的入口檢查。
// `prepare_exit_for_update`／`relaunch_after_failed_install` 只由 `plugin_backend` 使用，不對外。
pub use exit::{claim_tray_quit, is_exiting_for_update, mark_main_thread, try_begin_start};
pub use install::update_menu_label;

/// 更新器的記錄 target（spec：失敗只寫記錄）。
pub(crate) const LOG_TARGET: &str = "fc_host::updater";

/// 本行程的標記把手（只有 Primary 才有）。全行程唯一，供系統匣結束、事件迴圈結束、工作階段結束
/// 這些散在各處的正常結束路徑清除（[`clear_startup_marker`]）。
static MARKER: OnceLock<Arc<StartupMarker>> = OnceLock::new();

/// 建立小工具與桌布協調的函式（`main.rs` 把 setup 內「建立 UI」那一大段包成它）。
pub type UiBuilder = Box<dyn FnOnce(&AppHandle) + Send>;
/// 處理一個 single-instance 交接請求（`main.rs` 的回呼本體）。
pub type HandoffFn = fn(&AppHandle, Vec<String>);

/// `main()` 在 `tauri::Builder` 之前決定好的更新器啟動資訊。
pub struct Startup {
    enabled: bool,
    mode: StartMode,
    /// 讀到舊標記**且**更新器啟用＝進入過渡期。
    stale_marker: bool,
}

impl Startup {
    /// 是否要註冊外掛並啟動更新任務。
    pub fn enabled(&self) -> bool {
        self.enabled
    }
}

/// 依設定與建置旗標判定並記錄（`main()` 在 `tauri::Builder` 之前呼叫，`context.config()` 即可，
/// 不需要 `AppHandle`）。
pub fn evaluate_gate(config: &tauri::Config) -> Gate {
    // 隔離偵測與資料抓取共用 `desktop::detect_isolated_local_app_data`（design.md D3）。
    let isolation = crate::desktop::detect_isolated_local_app_data();
    let gate = gate::decide(
        config.plugins.0.get("updater"),
        cfg!(feature = "update-e2e"),
        isolation.as_ref(),
    );
    match &gate {
        Gate::Enabled => log::info!(target: LOG_TARGET, "更新器：啟用"),
        Gate::Disabled(reason @ gate::DisabledReason::Isolated(_)) => {
            log::info!(target: LOG_TARGET, "更新器：停用——{reason}")
        }
        Gate::Disabled(reason) if reason.is_error() => {
            log::error!(target: LOG_TARGET, "更新器：停用——{reason}")
        }
        Gate::Disabled(reason) => log::warn!(target: LOG_TARGET, "更新器：停用——{reason}"),
    }
    gate
}

/// 啟動前準備（仲裁成功之後、`tauri::Builder` 之前）：只有 Primary 讀寫標記，並啟動 5 分鐘到期計時。
pub fn prepare(role: StartupRole, gate: &Gate, args: &[String]) -> Startup {
    let mode = StartMode::from_args(args);
    let begin = StartupMarker::begin(&data_dir(), role, env!("CARGO_PKG_VERSION"), unix_now());
    if let Some(marker) = begin.marker {
        match marker.spawn_expiry(Arc::new(RealTimer), MARKER_LIFETIME) {
            Ok(_) => {}
            Err(e) => log::warn!(target: LOG_TARGET, "啟動標記：無法建立到期計時執行緒（{e}）"),
        }
        let _ = MARKER.set(marker);
    }
    make_startup(gate, mode, &begin.previous)
}

/// 純判定（可測）：這次啟動要不要啟用更新器、要不要進入過渡期。
fn make_startup(gate: &Gate, mode: StartMode, previous: &PreviousMarker) -> Startup {
    let enabled = gate.is_enabled() && mode.checks_for_updates();
    if let PreviousMarker::Present(info) = previous {
        log::warn!(
            target: LOG_TARGET,
            "啟動標記：偵測到上次非正常結束（舊標記版本={:?} 時間={:?}）",
            info.version,
            info.unix_secs
        );
        if !enabled {
            log::info!(target: LOG_TARGET, "更新器停用，不進入更新過渡期，直接建立小工具");
        }
    }
    Startup {
        enabled,
        mode,
        stale_marker: enabled && previous.is_stale(),
    }
}

/// 外掛本體（`main.rs` 只在 [`Startup::enabled`] 時註冊）。
pub fn plugin() -> TauriPlugin<Wry, tauri_plugin_updater::Config> {
    tauri_plugin_updater::Builder::new().build()
}

/// managed state：核心把手，供系統匣等處查詢過渡期狀態。
struct UpdaterRuntime {
    core: Arc<UpdaterCore>,
}

/// `setup` 內、**建立小工具與桌布協調之前**呼叫。更新器停用時直接建立 UI。
pub fn start(app: &AppHandle, startup: &Startup, ui_builder: UiBuilder, handoff: HandoffFn) {
    // 4.x 實機發現 A4：外掛寫在 %TEMP% 的安裝檔不會被刪；背景清掉過期的（不阻塞、失敗只記錄）。
    // 更新器停用時也做——舊版本留下的殘留與現在是否啟用無關。
    temp_cleanup::spawn(app.package_info().name.clone());
    if !startup.enabled {
        ui_builder(app);
        return;
    }
    // 沒有舊標記也從「建立 UI」起始（最終審查 M2）：`build_ui` 在 setup 內同步執行，建完才轉到「UI 已建立」，
    // 期間下載完成的自動安裝延後到建好後在背景重新評估，不在 UI 建好之前收尾。
    let transition = Arc::new(if startup.stale_marker {
        Transition::waiting()
    } else {
        Transition::building_ui()
    });
    let effects = Arc::new(AppEffects {
        app: app.clone(),
        ui_builder: Mutex::new(Some(ui_builder)),
        handoff,
    });
    // 外掛 `install()` 與 `on_before_exit` 掛鉤共用的狀態（掛鉤在 `PluginBackend::check` 建立 `UpdaterBuilder` 時綁定）。
    let probe = Arc::new(plugin_backend::InstallProbe::default());
    let core = UpdaterCore::new(
        env!("CARGO_PKG_VERSION"),
        Arc::new(plugin_backend::PluginBackend::new(
            app.clone(),
            probe.clone(),
        )),
        effects.clone(),
        Arc::new(RealTimer),
        transition,
        Arc::new(plugin_backend::PluginInstaller::new(probe, data_dir())),
        startup.mode,
    );
    app.manage(UpdaterRuntime { core: core.clone() });

    let plan = plan_first_check(
        startup.mode,
        startup.stale_marker,
        random_u64(),
        crate::fetch::net::MAX_WAIT,
    );
    let task_started = match plan {
        Some(plan) => match core.spawn(plan) {
            Ok(_) => true,
            Err(e) => {
                log::error!(target: LOG_TARGET, "更新任務：無法建立背景執行緒（{e}）");
                false
            }
        },
        None => false,
    };

    if startup.stale_marker {
        // 過渡期：setup 不阻塞、先不建立小工具。看門狗與更新任務任何一邊轉移到「建立 UI」都會建立。
        if !task_started || core.spawn_watchdog().is_err() {
            log::error!(target: LOG_TARGET, "更新過渡期：無法啟動任務或看門狗，直接建立小工具");
            core.fail_open();
        }
    } else {
        // 沒有舊標記：更新任務已在背景啟動，UI 立即在 setup 內建立；建完才轉到「UI 已建立」並重放建立途中
        // 排隊的交接（最終審查 M2）。
        effects.run_ui_now();
        core.finish_inline_build_ui();
    }
}

struct AppEffects {
    app: AppHandle,
    ui_builder: Mutex<Option<UiBuilder>>,
    handoff: HandoffFn,
}

impl AppEffects {
    fn take_builder(&self) -> Option<UiBuilder> {
        self.ui_builder
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }

    /// 沒有過渡期：在目前（setup、主）執行緒建立 UI。呼叫端建完要呼叫 `UpdaterCore::finish_inline_build_ui`。
    fn run_ui_now(&self) {
        if let Some(builder) = self.take_builder() {
            builder(&self.app);
        }
    }
}

impl Effects for AppEffects {
    fn build_ui(&self, ticket: BuildTicket) {
        let Some(builder) = self.take_builder() else {
            log::warn!(target: LOG_TARGET, "更新過渡期：小工具建立函式已被取走，略過");
            ticket.finish();
            return;
        };
        // 小工具與桌布協調必須在主執行緒建立（視窗、WinEvent 掛鉤等有執行緒親和性）。
        let app = self.app.clone();
        let posted = self.app.run_on_main_thread(move || {
            // 投遞到執行之間使用者可能已按系統匣「結束」（`Quit`）：已在結束就不建立。
            if !ticket.still_wanted() {
                log::info!(target: LOG_TARGET, "更新過渡期：已在結束，略過建立小工具與桌布協調");
                return;
            }
            builder(&app);
            ticket.finish();
        });
        // 只在事件迴圈已結束（行程正在結束）時失敗；此時 phase 停在 BuildingUi、佇列不重放都無妨。
        if let Err(e) = posted {
            log::error!(target: LOG_TARGET, "更新過渡期：無法把建立小工具的工作送到主執行緒：{e}");
        }
    }

    fn replay_handoff(&self, args: Vec<String>) {
        (self.handoff)(&self.app, args);
    }

    fn notify_ready(&self, ready: &InstallReady) {
        // 系統匣通知（同一版本只會呼叫一次）＋重建選單讓「更新到 vX.Y.Z 並重新啟動」出現。
        log::info!(
            target: LOG_TARGET,
            "新版 v{} 已下載並驗簽通過，系統匣通知並加上選單項",
            ready.version
        );
        crate::tray::show_notification(&self.app, install::ready_notification(&ready.version));
        crate::tray::refresh(&self.app);
    }

    fn refresh_menu(&self) {
        crate::tray::refresh(&self.app);
    }

    fn notify_install_failed(&self, version: &str) {
        crate::tray::show_notification(&self.app, install::install_failed_notification(version));
    }

    fn notify_install_refused(&self, why: &str) {
        crate::tray::show_notification(&self.app, install::install_refused_notification(why));
    }

    fn spawn_background(&self, job: Box<dyn FnOnce() + Send>) -> std::io::Result<()> {
        // 呼叫端在主執行緒（UI 剛建完）：延後的自動安裝要等主執行緒收尾，必須在自己的執行緒跑。
        std::thread::Builder::new()
            .name("fc-updater-deferred-install".to_owned())
            .spawn(job)
            .map(|_| ())
    }
}

// ── 給 main.rs／tray.rs 的薄接點 ──────────────────────────────────────────────────

/// single-instance 交接請求（非 `--restore-wallpaper`、非自動化重複啟動）的處置。更新器停用時一律即時處理；
/// 啟用時 UI 建好之前（過渡期，或沒有舊標記時 setup 同步建立途中的巢狀訊息泵）排隊、建完重放，之後即時處理。
pub fn route_handoff(app: &AppHandle, args: &[String]) -> HandoffDisposition {
    match app.try_state::<UpdaterRuntime>() {
        Some(rt) => rt.core.transition().offer_handoff(args.to_vec()),
        None => HandoffDisposition::ProcessNow,
    }
}

/// single-instance 收到 `--restore-wallpaper` 交接時先問這裡（最終審查 M2a）：過渡期（「等待」／「建立 UI」，後者
/// 含沒有舊標記時 setup 同步建立 UI 的途中——此時協調迴圈同樣尚未建立）中
/// 回 `true` 並已轉到「結束」、取消更新任務——呼叫端要回報「交接失敗」並呼叫 [`quit_during_transition`]，讓命令列
/// 等宿主結束後在自己的行程還原。其餘情況（含更新器未啟動）回 `false`，照原本交給協調迴圈。
pub fn quit_for_restore_handoff(app: &AppHandle) -> bool {
    app.try_state::<UpdaterRuntime>()
        .is_some_and(|rt| rt.core.on_restore_handoff())
}

/// 過渡期是否已轉到「結束」（複審 M-A）：`main.rs` 的 `build_ui` 建完小工具視窗後問一次——建立途中的巢狀訊息泵
/// 可能派送 `--restore-wallpaper` 交接而轉到「結束」，此時不得再啟動協調迴圈等背景元件。更新器未啟動時回 `false`。
pub fn transition_quitting(app: &AppHandle) -> bool {
    app.try_state::<UpdaterRuntime>()
        .is_some_and(|rt| rt.core.is_quitting())
}

/// 系統匣「結束」該怎麼處理（過渡期中結束時順便取消更新任務）。
pub fn on_tray_quit(app: &AppHandle) -> QuitDisposition {
    // task 3.3（審查 I1）：正在因更新結束時，「結束」一律忽略（不還原桌布、不存主題）。
    exit::quit_gate(|| match app.try_state::<UpdaterRuntime>() {
        Some(rt) => rt.core.on_tray_quit(),
        None => QuitDisposition::Normal,
    })
}

/// 已下載並驗簽、等待安裝的更新版本（`tray::refresh` 據此決定要不要顯示「更新到 vX.Y.Z 並重新啟動」）。
pub fn available_update(app: &AppHandle) -> Option<String> {
    app.try_state::<UpdaterRuntime>()
        .and_then(|rt| rt.core.ready_version())
}

/// 系統匣選單項「更新到 vX.Y.Z 並重新啟動」被點選（在 tao 事件迴圈執行緒上呼叫）：立即返回，安裝在
/// 專屬的背景執行緒進行——`install()` 要等收尾最多約 20 秒，`prepare_exit_for_update` 也不得在主執行緒執行
/// （會與 `run_on_main_thread` 死鎖，D4）。結果只記錄（成功時行程已結束）。
pub fn install_from_menu(app: &AppHandle) {
    let Some(rt) = app.try_state::<UpdaterRuntime>() else {
        log::warn!(target: LOG_TARGET, "手動安裝：更新器未啟動，忽略選單點選");
        return;
    };
    let core = rt.core.clone();
    let spawned = std::thread::Builder::new()
        .name("fc-update-install".to_owned())
        .spawn(move || {
            let outcome = core.install_manual();
            log::info!(target: LOG_TARGET, "手動安裝結束：{outcome:?}");
        });
    if let Err(e) = spawned {
        log::error!(target: LOG_TARGET, "手動安裝：無法建立背景執行緒（{e}），本次點選無效");
    }
}

/// 過渡期中結束（design.md D3）：不還原桌布、不存主題（協調迴圈尚未建立，語意同工作階段結束），
/// 取消重新啟動註冊、刪除當機標記後結束。觸發：系統匣「結束」，或過渡期收到 `--restore-wallpaper` 交接
/// （[`quit_for_restore_handoff`]）。
pub fn quit_during_transition(app: &AppHandle) {
    log::info!(
        target: LOG_TARGET,
        "更新過渡期中結束（系統匣「結束」或 --restore-wallpaper 交接）：不還原桌布、不存主題，刪除標記後結束"
    );
    crate::desktop::unregister_restart();
    clear_startup_marker();
    app.exit(0);
}

/// 正常結束路徑清除當機標記（系統匣「結束」、事件迴圈結束、過渡期結束；3.3 的「因更新結束」也呼叫它）。
/// 沒有標記（Secondary、寫入失敗）時什麼都不做。
pub fn clear_startup_marker() {
    if let Some(marker) = MARKER.get() {
        marker.clear();
    }
}

/// 事件迴圈結束（`RunEvent::Exit`）：清除標記並停止更新任務。
pub fn on_exit(app: &AppHandle) {
    clear_startup_marker();
    if let Some(rt) = app.try_state::<UpdaterRuntime>() {
        rt.core.stop();
    }
}

/// 工作階段結束（登出、關機）：刪除標記，但保留被取消時恢復的可能。
pub fn on_session_ending() {
    if let Some(marker) = MARKER.get() {
        marker.suspend_for_session_end();
    }
}

/// 工作階段結束被取消：標記還在 5 分鐘壽命內就恢復。
pub fn on_session_end_cancelled() {
    if let Some(marker) = MARKER.get() {
        marker.resume_after_session_end_cancelled(unix_now());
    }
}

// ── 小工具 ────────────────────────────────────────────────────────────────────────

/// `%LOCALAPPDATA%\tw.fintools.fc-host\`（標記與 `update-state.json` 所在；與記錄檔目錄同一個父層）。
fn data_dir() -> PathBuf {
    crate::logging::default_log_dir()
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// 不引入亂數 crate：標準函式庫的 `RandomState` 每個實例都有隨機種子。
fn random_u64() -> u64 {
    RandomState::new().build_hasher().finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gate::DisabledReason;
    use marker::MarkerInfo;

    fn stale() -> PreviousMarker {
        PreviousMarker::Present(MarkerInfo {
            version: Some("0.1.0".into()),
            unix_secs: Some(1),
        })
    }

    #[test]
    fn stale_marker_with_enabled_updater_enters_transition() {
        let s = make_startup(&Gate::Enabled, StartMode::Normal, &stale());
        assert!(s.enabled() && s.stale_marker);
        let s = make_startup(&Gate::Enabled, StartMode::Autostart, &stale());
        assert!(s.enabled() && s.stale_marker);
    }

    #[test]
    fn stale_marker_with_disabled_updater_builds_ui_immediately() {
        // 占位公鑰等停用情況：沒有更新可等，不得無謂延後小工具。
        let off = Gate::Disabled(DisabledReason::PlaceholderPubkey);
        let s = make_startup(&off, StartMode::Normal, &stale());
        assert!(!s.enabled() && !s.stale_marker);
        // 隔離環境停用同樣照既有「停用」處理（不進過渡期、不註冊外掛）。
        let isolated = Gate::Disabled(DisabledReason::Isolated(
            crate::desktop::LocalAppDataMismatch {
                env_value: r"C:\Temp\iso\Local".to_string(),
                registered: r"C:\Users\someone\AppData\Local".to_string(),
            },
        ));
        for mode in [StartMode::Normal, StartMode::Autostart] {
            let s = make_startup(&isolated, mode, &stale());
            assert!(!s.enabled() && !s.stale_marker, "{mode:?}");
        }
    }

    #[test]
    fn no_marker_or_unchecked_marker_never_enters_transition() {
        for previous in [PreviousMarker::Absent, PreviousMarker::NotChecked] {
            let s = make_startup(&Gate::Enabled, StartMode::Normal, &previous);
            assert!(s.enabled() && !s.stale_marker, "{previous:?}");
        }
    }

    #[test]
    fn command_line_modes_never_enable_the_updater() {
        for mode in [StartMode::FetchOnce, StartMode::RestoreWallpaper] {
            let s = make_startup(&Gate::Enabled, mode, &stale());
            assert!(!s.enabled() && !s.stale_marker, "{mode:?} 不得檢查更新");
        }
    }

    #[test]
    fn evaluate_gate_wires_isolation_detection_and_the_e2e_cfg_into_decide() {
        // `evaluate_gate` 要 `tauri::Config`、讀真實環境，無法在單元測試直接驅動；以原始碼斷言接線存在。
        // 只取非測試區段（本測試自己的字串常值不得算數），再切出 `evaluate_gate` 本體。
        // 若有人把隔離偵測拿掉（例如 isolation 改傳 None）或把 e2e 旗標寫死，這裡會失敗。
        let src = include_str!("updater.rs");
        let production = src.split("#[cfg(test)]").next().expect("非測試區段");
        let body = production
            .split("pub fn evaluate_gate(")
            .nth(1)
            .expect("找得到 evaluate_gate")
            .split("\n}\n")
            .next()
            .expect("evaluate_gate 本體");
        assert!(
            body.contains("let isolation = crate::desktop::detect_isolated_local_app_data();"),
            "要以共用的隔離偵測取得 isolation"
        );
        assert!(
            body.contains("gate::decide(")
                && body.contains("cfg!(feature = \"update-e2e\")")
                && body.contains("isolation.as_ref()"),
            "decide 要吃偵測結果與 update-e2e cfg"
        );
    }

    #[test]
    fn capabilities_do_not_grant_the_updater_plugin_to_any_webview() {
        // design.md D3：更新由後端主導。外掛的 `plugin:updater|*` 指令只有在 capability 授權時頁面才呼叫得到；
        // 任何 capability 檔出現 `updater:` 權限（或 `*` 萬用）就是把更新能力開給了 webview。
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("capabilities");
        let mut checked = 0;
        for entry in std::fs::read_dir(&dir).expect("capabilities 資料夾") {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "json") {
                let text = std::fs::read_to_string(&path).unwrap();
                assert!(
                    !text.contains("updater"),
                    "{} 不得授權 updater 外掛給 webview",
                    path.display()
                );
                checked += 1;
            }
        }
        assert!(checked >= 1, "至少要檢查到一個 capability 檔");
    }

    /// 最終審查 M2a：single-instance 回呼收到 `--restore-wallpaper` 交接時，先問過渡期（`quit_for_restore_handoff`）；
    /// 過渡期中先回報「交接失敗」、再以過渡期結束收掉宿主、然後返回，不交給協調迴圈。回呼需要 `AppHandle`，以原始碼
    /// 斷言接線順序；行為面由 `engine` 的 `restore_handoff_*` 與 `wallpaper_cli` 的過渡期交接測試涵蓋。
    #[test]
    fn restore_handoff_during_the_transition_is_refused_then_the_host_quits() {
        let main = include_str!("main.rs").replace("\r\n", "\n");
        let start = main
            .find("if let Some(reply_pid) = wallpaper_cli::handoff_request(&args) {")
            .expect("找得到交接分支");
        let body = &main[start..];
        let pos = |needle: &str| {
            body.find(needle)
                .unwrap_or_else(|| panic!("交接分支缺少 {needle}"))
        };
        let ask = pos("updater::quit_for_restore_handoff(app)");
        let refuse = pos("wallpaper_cli::refuse_handoff_during_update_transition(reply_pid)");
        let quit = pos("updater::quit_during_transition(app)");
        let accept = pos("wallpaper_cli::accept_handoff(app, reply_pid)");
        assert!(ask < refuse && refuse < quit && quit < accept);
        assert!(
            body[quit..accept].contains("return;"),
            "過渡期結束後要直接返回"
        );
    }

    /// 複審 M-A：`build_ui` 建完小工具視窗（巢狀訊息泵可能派送交接）之後、啟動任何背景元件（渲染管線、協調迴圈、
    /// 看門狗、守門視窗、抓取排程）之前，再問一次過渡期是否已「結束」，是就返回。行為面見 `engine` 的
    /// `restore_handoff_nested_inside_the_ui_build_stops_before_the_coordinator`。
    #[test]
    fn build_ui_rechecks_the_transition_after_creating_widget_windows() {
        let main = include_str!("main.rs").replace("\r\n", "\n");
        let start = main
            .find("fn build_ui(handle: &tauri::AppHandle, params: UiParams) {")
            .expect("找得到 build_ui");
        let body = &main[start..];
        let pos = |needle: &str| {
            body.find(needle)
                .unwrap_or_else(|| panic!("build_ui 缺少 {needle}"))
        };
        let widgets = pos("widgets::sync_widget_windows(handle);");
        let check = pos("updater::transition_quitting(handle)");
        let render = pos("wallpaper_render::install(handle);");
        let rest = [
            pos("wallpaper_coordinator::spawn_for_app(handle);"),
            pos("recovery::spawn_watchdog(handle);"),
            pos("desktop::spawn_gatekeeper("),
            pos("fetch::scheduler::spawn_for_app(handle);"),
        ];
        assert!(
            widgets < check && check < render,
            "檢查要在建完視窗之後、渲染管線之前"
        );
        assert!(rest.iter().all(|&p| check < p), "背景元件都要在檢查之後");
        assert!(body[check..render].contains("return;"), "已結束要直接返回");
    }

    /// 最終審查 M2：沒有舊標記時狀態從「建立 UI」起始，setup 同步建完 UI 之後才轉到「UI 已建立」。`start` 需要
    /// `AppHandle`，以原始碼斷言接線；行為面見 `engine` 的 `no_marker_start_*`。
    #[test]
    fn no_marker_start_marks_the_ui_ready_only_after_building_it() {
        let src = include_str!("updater.rs").replace("\r\n", "\n");
        let production = src.split("#[cfg(test)]").next().expect("非測試區段");
        let body = production
            .split("pub fn start(")
            .nth(1)
            .expect("找得到 start")
            .split("\n}\n")
            .next()
            .expect("start 本體");
        assert!(
            body.contains("Transition::building_ui()") && !body.contains("Transition::ui_ready()"),
            "沒有舊標記也要從「建立 UI」起始"
        );
        let build = body.find("effects.run_ui_now();").expect("同步建立 UI");
        let finish = body
            .find("core.finish_inline_build_ui();")
            .expect("建完轉到 UI 已建立");
        assert!(build < finish, "建完才轉到「UI 已建立」");
    }

    #[test]
    fn data_dir_is_the_parent_of_the_log_dir() {
        let dir = data_dir();
        assert!(
            dir.ends_with("tw.fintools.fc-host"),
            "標記與 update-state.json 放在 %LOCALAPPDATA%\\tw.fintools.fc-host：{}",
            dir.display()
        );
    }
}
