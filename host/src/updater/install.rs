//! 安裝觸發規則與安裝流程（installer-auto-update task 3.2；design.md D3「安裝時機」「防止自動安裝循環」、
//! D4「失敗處理」）。與 Tauri／外掛隔離：時間、狀態檔、外掛的 `install()`、失敗退路都經 [`InstallSystem`] 注入，
//! 單元測試以假物件驅動（每個失敗分支都有測試，見本檔與 `engine` 的測試）。
//!
//! ## 三種安裝
//!
//! | 種類 | 觸發 | 受哪些規則限制 | `ui_built` |
//! |---|---|---|---|
//! | [`InstallKind::Manual`] | 系統匣選單「更新到 vX.Y.Z 並重新啟動」 | 只受「正在結束」「已有安裝進行中」限制（**不受 24 小時退避限制**），但**同樣寫入**嘗試紀錄 | `true`（UI 已建立） |
//! | [`InstallKind::Auto`] | `--autostart` 啟動後 10 分鐘內下載完成 | 必須 `--autostart`、未滿 10 分鐘、24 小時退避 | `true` |
//! | [`InstallKind::Transition`] | 當機迴圈保護的過渡期同步安裝（3.1） | 24 小時退避（D3：「當機迴圈保護的同步安裝同樣受此限」）；不限 10 分鐘、不要求 `--autostart` | `false`（沒有 UI） |
//!
//! 一般執行期間（沒有 `--autostart`、或超過 10 分鐘）**不自行開始安裝**，只通知。
//!
//! ## 一次安裝的順序（[`run_install`]）
//!
//! ```text
//! 1. 系統匣「結束」已在進行（QUIT_STARTED）或已在因更新結束 ─▶ QuitInProgress（沒寫紀錄、沒動任何東西）
//! 2. 寫 update-state.json（嘗試的版本與時間）。自動種類寫不進去 ─▶ NotStarted（沒有防循環紀錄就不自動安裝）；
//!    手動寫不進去只記錄、照常安裝（使用者明確要求）
//! 3. InstallSystem::install（外掛 install()；on_before_exit 掛 prepare_exit_for_update）
//!      ├─ BeforeExit 失敗（主執行緒呼叫、型別不符、寫暫存檔失敗、結束已開始而中止）─▶ FailedBeforeExit
//!      │     宿主照常運作；**丟掉手上持有的更新**（[`InstallError::should_discard_update`]），下個週期重新
//!      │     檢查、重新下載（D4「下個週期再試」；位元組本身壞掉時重試同一份永遠失敗）
//!      ├─ AfterExit 失敗（on_before_exit 已跑完才失敗，例如 ShellExecuteW 失敗、使用者取消 UAC）
//!      │     ─▶ after_exit_failure（正式：relaunch_after_failed_install，不返回）；**不得**回頭建 UI
//!      └─ 回 Ok（外掛成功時 exit(0)，不會回來；回來了就是違反契約）─▶ ReturnedWithoutExit
//! ```
//!
//! 24 小時退避只對**同一版本**（候選版本不高於上次嘗試的版本）；出現更新的版本時照常自動安裝
//! （spec「24 小時內不再自動安裝同一版本」；每個版本最多各被擋一次，不會循環）。
//!
//! 嘗試紀錄在步驟 2 寫入、**失敗時不清除**：`FailedBeforeExit` 後行程仍活著，不會循環；保留紀錄只讓之後 24 小時內
//! 的自動安裝退避（保守側），手動不受影響。寫入發生在 `install()` **之前**，所以即使 `install()` 中途行程消失
//! （安裝檔已啟動、舊行程被結束），下次啟動仍讀得到。

use std::io;
use std::time::Duration;

use super::engine::Downloaded;
use super::schedule::StartMode;
use super::state_file::UpdateState;
use super::LOG_TARGET;
use crate::desktop::tray_balloon::BalloonKind;
use crate::wallpaper_settings::NotificationText;

/// `--autostart` 啟動後多久內找到並下載完新版才自動安裝（design.md D3）。
pub const AUTO_INSTALL_WINDOW: Duration = Duration::from_secs(10 * 60);

/// 交給外掛 `installer_args` 的參數（design.md D3「重新啟動參數」）。外掛的 Windows NSIS 命令列是
/// `<install_mode 參數> /UPDATE [<restart 參數>] <installer_args…>`，這裡搭配 `restart_after_install(false)`
/// （外掛不加自己的 `/R /ARGS <目前行程參數>`）後的完整命令列為 `/P /UPDATE /R /ARGS --autostart`：
///
/// - `/P`＝`windows.installMode: passive`（`tauri-plugin-updater-2.13.1/src/config.rs` `nsis_args`）；
/// - `/UPDATE`＝外掛固定加（`updater.rs:990`）；
/// - `/R /ARGS --autostart`＝本常數。NSIS 範本（`tauri-cli-v2.12.1`＝design D6 釘選的版本所產出的 `installer.nsi`；
///   行號取自本機建置產物 `host/target/release/nsis/x64/installer.nsi`，該檔不在 git 內，重現請以同版 tauri-cli
///   重新打包）在 `.onInstSuccess`
///   （第 715–726 行；passive／silent 條件在第 718–719 行）只在 passive／silent 模式下以 `${GetOptions} $CMDLINE "/R"` 判斷要不要重新啟動
///   （第 720 行），再以 `${GetOptions} $CMDLINE "/ARGS"`（第 722 行）取值、`nsis_tauri_utils::RunAsUser
///   "$INSTDIR\${MAINBINARYNAME}.exe" "$R0"`（第 723 行）重新啟動。`GetOptions` 的值取到下一個以 `/` 開頭的
///   字元為止，`--autostart` 不含 `/`，所以可行；`/ARGS` 排在最後，後面不會再有被吃進去的參數。
///   `/R` 與 `/ARGS` 在 `.onInit` 沒有任何解析（`.onInit` 只讀 `/P`、`/NS`、`/UPDATE`，第 469–483 行），不會與
///   外掛參數重複或衝突。
pub const INSTALLER_ARGS: [&str; 3] = ["/R", "/ARGS", "--autostart"];

/// 選單項目「更新到 vX.Y.Z 並重新啟動」的文字（系統匣選單與通知內文共用）。
pub fn update_menu_label(version: &str) -> String {
    format!("更新到 v{version} 並重新啟動")
}

/// 「新版已下載並驗簽」的系統匣通知文字（同一版本只發一次，見 `UpdaterCore::mark_notified`）。
pub fn ready_notification(version: &str) -> NotificationText {
    NotificationText {
        title: format!("財經日曆：有新版 v{version}"),
        body: format!(
            "新版已下載並通過驗證。要更新時，請從系統匣選單選「{}」。",
            update_menu_label(version)
        ),
        kind: BalloonKind::Info,
    }
}

/// 手動安裝在 `on_before_exit` 之前失敗（或 panic）的通知（Warning）。文案「稍後會再試」成立的前提：失敗後
/// 已丟掉手上的更新，下個檢查週期會重新下載（見 `UpdaterCore::discard_ready_after_failed_install`）。
pub fn install_failed_notification(version: &str) -> NotificationText {
    NotificationText {
        title: "財經日曆：更新失敗".to_owned(),
        body: format!("更新到 v{version} 失敗，稍後會再試。"),
        kind: BalloonKind::Warning,
    }
}

/// 使用者點了更新但被拒絕（小工具尚未就緒、已有安裝進行中、沒有可裝的更新）時的通知；`why` 是拒絕原因。
pub fn install_refused_notification(why: &str) -> NotificationText {
    NotificationText {
        title: "財經日曆：暫時無法更新".to_owned(),
        body: format!("{why}，請稍後再試。"),
        kind: BalloonKind::Info,
    }
}

/// 安裝的種類（見模組文件表）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallKind {
    Manual,
    Auto,
    Transition,
}

impl InstallKind {
    /// 自動種類（不是使用者按的）：寫不進嘗試紀錄就不安裝。
    pub fn is_automatic(self) -> bool {
        !matches!(self, Self::Manual)
    }

    fn label(self) -> &'static str {
        match self {
            Self::Manual => "手動",
            Self::Auto => "自動",
            Self::Transition => "過渡期同步",
        }
    }
}

/// `InstallSystem::install` 的失敗：依「`on_before_exit` 是否已經跑完」分兩種（3.1 複審 N1）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallFailure {
    /// `on_before_exit` 還沒跑（或被中止）：什麼都沒收尾，宿主照常運作。
    BeforeExit(String),
    /// `on_before_exit` 已跑完（視窗已關、系統匣圖示已移除、標記已刪）：不得回頭建 UI。
    AfterExit(String),
}

/// 安裝需要的外部世界。
pub trait InstallSystem: Send + Sync {
    fn now_unix(&self) -> i64;
    /// 自核心建立（約等於行程啟動）起經過的時間（10 分鐘自動安裝窗口用）。
    fn uptime(&self) -> Duration;
    /// 系統匣「結束」已在進行，或已經在因更新結束：不得啟動安裝。
    fn quit_in_progress(&self) -> bool;
    fn read_state(&self) -> Option<UpdateState>;
    fn write_state(&self, state: &UpdateState) -> io::Result<()>;
    /// 呼叫外掛的 `install()`（呼叫端保證在背景執行緒）。成功時行程在其中結束、不會返回。
    fn install(&self, update: &Downloaded, ui_built: bool) -> Result<(), InstallFailure>;
    /// `on_before_exit` 之後失敗的退路。正式實作＝`exit::relaunch_after_failed_install`（重新啟動自己後
    /// `process::exit`，不返回）；測試實作只記錄。
    fn after_exit_failure(&self, error: &str);
}

/// 自動種類要不要安裝。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutoDecision {
    Install,
    NotifyOnly(SkipReason),
}

/// 為什麼只通知、不自動安裝。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    /// 不是 `--autostart` 啟動：一般執行期間不自行開始安裝。
    NotAutostart,
    /// `--autostart` 但已超過 10 分鐘。
    WindowElapsed(Duration),
    /// 上次嘗試過同一版本、目前版本仍低於它、且未滿 24 小時（防循環）。
    BackedOff {
        attempted_version: String,
        attempted_at_unix: i64,
    },
}

impl std::fmt::Display for SkipReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAutostart => write!(f, "不是 --autostart 啟動，一般執行期間不自行安裝"),
            Self::WindowElapsed(up) => write!(
                f,
                "啟動已 {} 秒，超過 {} 秒的自動安裝窗口",
                up.as_secs(),
                AUTO_INSTALL_WINDOW.as_secs()
            ),
            Self::BackedOff {
                attempted_version,
                attempted_at_unix,
            } => write!(
                f,
                "上次已嘗試安裝 v{attempted_version}（Unix 時間 {attempted_at_unix}）但目前版本仍較舊，24 小時內不再自動安裝"
            ),
        }
    }
}

/// 純判定：自動種類（`Auto`／`Transition`）要不要安裝。`Manual` 不經過這裡（不受任何限制）。
///
/// - `Transition`：只看 24 小時退避（過渡期不分啟動方式、與 10 分鐘窗口無關）。
/// - `Auto`：必須 `--autostart`、未滿 10 分鐘（含剛好 10 分鐘）、且不退避。
/// - 退避只對**同一版本**：`candidate_version` 高於上次嘗試的版本時照常安裝。
pub fn decide_auto(
    kind: InstallKind,
    mode: StartMode,
    uptime: Duration,
    state: Option<&UpdateState>,
    current_version: &str,
    candidate_version: &str,
    now_unix: i64,
) -> AutoDecision {
    if kind == InstallKind::Auto {
        if mode != StartMode::Autostart {
            return AutoDecision::NotifyOnly(SkipReason::NotAutostart);
        }
        if uptime > AUTO_INSTALL_WINDOW {
            return AutoDecision::NotifyOnly(SkipReason::WindowElapsed(uptime));
        }
    }
    if let Some(state) = state {
        if state.backs_off_auto_install(current_version, candidate_version, now_unix) {
            return AutoDecision::NotifyOnly(SkipReason::BackedOff {
                attempted_version: state.attempted_version.clone(),
                attempted_at_unix: state.attempted_at_unix,
            });
        }
    }
    AutoDecision::Install
}

/// 把決策結果記到記錄檔：退避是錯誤（spec「自動安裝沒有生效」：記錄檔有一筆錯誤），其餘是一般資訊。
pub fn log_decision(kind: InstallKind, version: &str, decision: &AutoDecision) {
    match decision {
        AutoDecision::Install => {
            log::info!(target: LOG_TARGET, "{}安裝 v{version}：符合條件，開始安裝", kind.label())
        }
        AutoDecision::NotifyOnly(reason @ SkipReason::BackedOff { .. }) => {
            log::error!(target: LOG_TARGET, "{}安裝 v{version}：{reason}；只通知", kind.label())
        }
        AutoDecision::NotifyOnly(reason) => {
            log::info!(target: LOG_TARGET, "{}安裝 v{version}：{reason}；只通知", kind.label())
        }
    }
}

/// 一次安裝沒有成功結束行程的結果（成功時行程已經結束，沒有「成功」這個結果）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallError {
    /// 沒有開始：系統匣結束或更新結束已在進行。什麼都沒動；更新**不丟**（正在結束，沒有下個週期）。
    QuitInProgress,
    /// 沒有開始：自動種類寫不進嘗試紀錄（沒有防循環紀錄就不自動安裝）。什麼都沒動。
    NotStarted(String),
    /// `install()` 在 `on_before_exit` 之前失敗：宿主照常運作（過渡期走退路建立 UI）。
    FailedBeforeExit(String),
    /// `install()` 回 Ok 卻沒結束行程（違反契約）：視同 `FailedBeforeExit`。
    ReturnedWithoutExit,
    /// `on_before_exit` 之後失敗：已呼叫 [`InstallSystem::after_exit_failure`]（正式環境不會返回）。
    /// 呼叫端**不得**回頭建 UI。
    FailedAfterExit(String),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::QuitInProgress => write!(f, "沒有開始安裝（系統匣結束或更新結束已在進行）"),
            Self::NotStarted(why) => write!(f, "沒有開始安裝（{why}）"),
            Self::FailedBeforeExit(e) => write!(f, "install() 在結束流程之前失敗（{e}）"),
            Self::ReturnedWithoutExit => write!(f, "install() 返回了但行程沒有結束（違反契約）"),
            Self::FailedAfterExit(e) => write!(f, "install() 在結束流程之後失敗（{e}）"),
        }
    }
}

impl InstallError {
    /// 宿主還在原狀（沒有收尾過）：可以照常運作或走建立 UI 的退路。
    pub fn host_is_intact(&self) -> bool {
        !matches!(self, Self::FailedAfterExit(_))
    }

    /// 宿主還在、更新卻沒裝成：丟掉手上持有的更新，讓下個週期重新檢查並重新下載（D4「下個週期再試」）。
    /// 不丟的：`QuitInProgress`（正在結束）、`FailedAfterExit`（行程即將結束、已重新啟動自己）。
    pub fn should_discard_update(&self) -> bool {
        matches!(
            self,
            Self::NotStarted(_) | Self::FailedBeforeExit(_) | Self::ReturnedWithoutExit
        )
    }
}

/// 執行一次安裝（見模組文件的順序）。**只會回傳失敗**：成功時行程在 `install()` 內結束。
pub fn run_install(
    sys: &dyn InstallSystem,
    update: &Downloaded,
    kind: InstallKind,
    ui_built: bool,
) -> InstallError {
    if sys.quit_in_progress() {
        let why = "系統匣結束或更新結束已在進行";
        log::warn!(target: LOG_TARGET, "{}安裝 v{}：{why}，不啟動安裝", kind.label(), update.version);
        return InstallError::QuitInProgress;
    }
    let record = UpdateState::new(&update.version, sys.now_unix());
    if let Err(e) = sys.write_state(&record) {
        if kind.is_automatic() {
            let why = format!("無法寫入 update-state.json（{e}），沒有防循環紀錄就不自動安裝");
            log::error!(target: LOG_TARGET, "{}安裝 v{}：{why}", kind.label(), update.version);
            return InstallError::NotStarted(why);
        }
        log::warn!(
            target: LOG_TARGET,
            "手動安裝 v{}：無法寫入 update-state.json（{e}），仍照使用者要求安裝",
            update.version
        );
    }
    log::info!(
        target: LOG_TARGET,
        "{}安裝 v{}：呼叫外掛 install()（ui_built={ui_built}）",
        kind.label(),
        update.version
    );
    match sys.install(update, ui_built) {
        Ok(()) => InstallError::ReturnedWithoutExit,
        Err(InstallFailure::BeforeExit(e)) => InstallError::FailedBeforeExit(e),
        Err(InstallFailure::AfterExit(e)) => {
            sys.after_exit_failure(&e);
            InstallError::FailedAfterExit(e)
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// 假的安裝環境：記錄呼叫順序到共用的事件記錄，`install` 的結果由腳本決定。
    pub struct FakeInstall {
        pub events: Arc<Mutex<Vec<String>>>,
        pub now: Mutex<i64>,
        pub uptime: Mutex<Duration>,
        pub quit: Mutex<bool>,
        pub state: Mutex<Option<UpdateState>>,
        pub write_fails: Mutex<bool>,
        pub install_result: Mutex<InstallScript>,
        /// 最近一次 `install` 收到的 `ui_built`。
        pub last_ui_built: Mutex<Option<bool>>,
    }

    #[derive(Clone, Copy, Default)]
    pub enum InstallScript {
        /// 違反契約：回 Ok 卻沒結束。
        #[default]
        ReturnsOk,
        BeforeExit,
        AfterExit,
        Panics,
    }

    impl FakeInstall {
        pub fn new(events: Arc<Mutex<Vec<String>>>) -> Self {
            Self {
                events,
                now: Mutex::new(1_000_000),
                uptime: Mutex::new(Duration::from_secs(30)),
                quit: Mutex::new(false),
                state: Mutex::new(None),
                write_fails: Mutex::new(false),
                install_result: Mutex::new(InstallScript::default()),
                last_ui_built: Mutex::new(None),
            }
        }
        fn push(&self, line: String) {
            self.events.lock().unwrap().push(line);
        }
        /// 還沒有任何嘗試紀錄。
        pub fn read_state_none(&self) -> bool {
            self.state.lock().unwrap().is_none()
        }
    }

    impl InstallSystem for FakeInstall {
        fn now_unix(&self) -> i64 {
            *self.now.lock().unwrap()
        }
        fn uptime(&self) -> Duration {
            *self.uptime.lock().unwrap()
        }
        fn quit_in_progress(&self) -> bool {
            *self.quit.lock().unwrap()
        }
        fn read_state(&self) -> Option<UpdateState> {
            self.state.lock().unwrap().clone()
        }
        fn write_state(&self, state: &UpdateState) -> io::Result<()> {
            if *self.write_fails.lock().unwrap() {
                return Err(io::Error::other("磁碟已滿"));
            }
            self.push(format!("state:{}", state.attempted_version));
            *self.state.lock().unwrap() = Some(state.clone());
            Ok(())
        }
        fn install(&self, update: &Downloaded, ui_built: bool) -> Result<(), InstallFailure> {
            self.push(format!("install_now:{}", update.version));
            *self.last_ui_built.lock().unwrap() = Some(ui_built);
            match *self.install_result.lock().unwrap() {
                InstallScript::ReturnsOk => Ok(()),
                InstallScript::BeforeExit => Err(InstallFailure::BeforeExit("寫暫存檔失敗".into())),
                InstallScript::AfterExit => {
                    Err(InstallFailure::AfterExit("ShellExecuteW 失敗".into()))
                }
                InstallScript::Panics => panic!("假的 install 內部 panic"),
            }
        }
        fn after_exit_failure(&self, error: &str) {
            self.push(format!("relaunch:{error}"));
        }
    }

    fn downloaded(version: &str) -> Downloaded {
        Downloaded {
            version: version.to_owned(),
            bytes: vec![1, 2, 3],
            handle: Box::new(()),
        }
    }

    fn fake(script: InstallScript) -> FakeInstall {
        let f = FakeInstall::new(Arc::default());
        *f.install_result.lock().unwrap() = script;
        f
    }

    fn events(f: &FakeInstall) -> Vec<String> {
        f.events.lock().unwrap().clone()
    }

    // ── 重新啟動參數 ────────────────────────────────────────────────────────────

    #[test]
    fn installer_args_complete_the_nsis_command_line_without_duplicates() {
        // 外掛（restart_after_install=false）組出 `<nsis_args> /UPDATE <installer_args>`；passive 的 nsis_args 是 `/P`。
        let mut line = vec!["/P", "/UPDATE"];
        line.extend(INSTALLER_ARGS);
        assert_eq!(line.join(" "), "/P /UPDATE /R /ARGS --autostart");
        for flag in ["/P", "/UPDATE", "/R", "/ARGS"] {
            assert_eq!(
                line.iter().filter(|t| **t == flag).count(),
                1,
                "{flag} 不得重複"
            );
        }
        // `/ARGS` 的值取到下一個 `/` 為止：值本身不含 `/`、且 `/ARGS` 是最後一個旗標。
        let args_pos = line.iter().position(|t| *t == "/ARGS").unwrap();
        assert_eq!(&line[args_pos + 1..], ["--autostart"]);
        assert!(!"--autostart".contains('/'));
        // 與 `StartMode` 的判定一致：新版以這個參數啟動就是 Autostart。
        assert_eq!(
            StartMode::from_args(&["fc-host.exe", "--autostart"]),
            StartMode::Autostart
        );
    }

    #[test]
    fn menu_label_and_notification_mention_the_same_text() {
        assert_eq!(update_menu_label("0.1.1"), "更新到 v0.1.1 並重新啟動");
        let n = ready_notification("0.1.1");
        assert!(n.title.contains("v0.1.1"));
        assert!(n.body.contains(&update_menu_label("0.1.1")));
    }

    // ── decide_auto：spec「登入後 10 分鐘內自動安裝」「超過 10 分鐘只通知」「24 小時退避」────

    #[test]
    fn auto_install_inside_ten_minutes_of_an_autostart_launch() {
        for secs in [0, 30, 599, 600] {
            assert_eq!(
                decide_auto(
                    InstallKind::Auto,
                    StartMode::Autostart,
                    Duration::from_secs(secs),
                    None,
                    "0.1.0",
                    "0.1.1",
                    1_000
                ),
                AutoDecision::Install,
                "啟動後 {secs} 秒（含剛好 10 分鐘）"
            );
        }
    }

    #[test]
    fn after_ten_minutes_an_autostart_launch_only_notifies() {
        let d = decide_auto(
            InstallKind::Auto,
            StartMode::Autostart,
            Duration::from_secs(601),
            None,
            "0.1.0",
            "0.1.1",
            1_000,
        );
        assert!(matches!(
            d,
            AutoDecision::NotifyOnly(SkipReason::WindowElapsed(_))
        ));
    }

    #[test]
    fn a_normal_launch_never_auto_installs() {
        // 一般執行期間不自行開始安裝（包含剛啟動的前 10 分鐘）。
        let d = decide_auto(
            InstallKind::Auto,
            StartMode::Normal,
            Duration::from_secs(5),
            None,
            "0.1.0",
            "0.1.1",
            1_000,
        );
        assert_eq!(d, AutoDecision::NotifyOnly(SkipReason::NotAutostart));
    }

    #[test]
    fn backoff_blocks_auto_and_transition_installs_for_24_hours() {
        // spec「自動安裝沒有生效」：登入後自動安裝 v0.1.2，但重新啟動的仍是 v0.1.1。
        let state = UpdateState::new("0.1.2", 1_000);
        for kind in [InstallKind::Auto, InstallKind::Transition] {
            let d = decide_auto(
                kind,
                StartMode::Autostart,
                Duration::from_secs(10),
                Some(&state),
                "0.1.1",
                "0.1.2",
                1_000 + 3600,
            );
            assert!(
                matches!(d, AutoDecision::NotifyOnly(SkipReason::BackedOff { .. })),
                "{kind:?}"
            );
            // 滿 24 小時可再試。
            assert_eq!(
                decide_auto(
                    kind,
                    StartMode::Autostart,
                    Duration::from_secs(10),
                    Some(&state),
                    "0.1.1",
                    "0.1.2",
                    1_000 + 24 * 3600
                ),
                AutoDecision::Install,
                "{kind:?}"
            );
            // 目前版本已追上：不退避。
            assert_eq!(
                decide_auto(
                    kind,
                    StartMode::Autostart,
                    Duration::from_secs(10),
                    Some(&state),
                    "0.1.2",
                    "0.1.2",
                    1_000 + 3600
                ),
                AutoDecision::Install,
                "{kind:?}"
            );
        }
    }

    #[test]
    fn transition_ignores_the_ten_minute_window_and_start_mode() {
        // 當機迴圈保護：一般啟動、啟動已久都照樣同步安裝（只受退避限制）。
        assert_eq!(
            decide_auto(
                InstallKind::Transition,
                StartMode::Normal,
                Duration::from_secs(3600),
                None,
                "0.1.0",
                "0.1.1",
                1_000
            ),
            AutoDecision::Install
        );
    }

    // ── run_install：嘗試紀錄（含手動）、失敗分支 ───────────────────────────────

    #[test]
    fn every_kind_records_the_attempt_before_calling_install() {
        for kind in [
            InstallKind::Manual,
            InstallKind::Auto,
            InstallKind::Transition,
        ] {
            let f = fake(InstallScript::BeforeExit);
            *f.now.lock().unwrap() = 4242;
            let _ = run_install(&f, &downloaded("0.1.1"), kind, true);
            assert_eq!(
                events(&f),
                vec!["state:0.1.1", "install_now:0.1.1"],
                "{kind:?}：先寫嘗試紀錄、再呼叫 install（手動安裝同樣寫入）"
            );
            let s = f.read_state().unwrap();
            assert_eq!(
                (s.attempted_version.as_str(), s.attempted_at_unix),
                ("0.1.1", 4242)
            );
        }
    }

    #[test]
    fn manual_install_is_not_blocked_by_backoff_but_a_manual_record_makes_the_next_auto_back_off() {
        // 手動安裝失敗後下次登入也要正確退避（D3）。
        let f = fake(InstallScript::BeforeExit);
        *f.state.lock().unwrap() = Some(UpdateState::new("0.1.2", 999_999)); // 剛嘗試過
        let e = run_install(&f, &downloaded("0.1.2"), InstallKind::Manual, true);
        assert_eq!(e, InstallError::FailedBeforeExit("寫暫存檔失敗".into()));
        assert!(
            events(&f).contains(&"install_now:0.1.2".to_owned()),
            "手動不受 24 小時退避限制"
        );
        let state = f.read_state().unwrap();
        let d = decide_auto(
            InstallKind::Auto,
            StartMode::Autostart,
            Duration::from_secs(10),
            Some(&state),
            "0.1.1",
            "0.1.2",
            f.now_unix() + 60,
        );
        assert!(matches!(
            d,
            AutoDecision::NotifyOnly(SkipReason::BackedOff { .. })
        ));
    }

    #[test]
    fn install_failing_before_on_before_exit_keeps_the_host_running() {
        // 寫暫存檔失敗：on_before_exit 尚未執行，宿主照常運作；不呼叫退路、不重新啟動。
        let f = fake(InstallScript::BeforeExit);
        let e = run_install(&f, &downloaded("0.1.1"), InstallKind::Manual, true);
        assert_eq!(e, InstallError::FailedBeforeExit("寫暫存檔失敗".into()));
        assert!(e.host_is_intact());
        assert!(
            !events(&f).iter().any(|l| l.starts_with("relaunch:")),
            "不得走重新啟動退路：{:?}",
            events(&f)
        );
    }

    #[test]
    fn install_failing_after_on_before_exit_relaunches_instead_of_building_ui() {
        // ShellExecuteW 失敗／使用者取消 UAC：收尾已做完，走 relaunch_after_failed_install（3.1 複審 N1）。
        for kind in [
            InstallKind::Manual,
            InstallKind::Auto,
            InstallKind::Transition,
        ] {
            let f = fake(InstallScript::AfterExit);
            let e = run_install(
                &f,
                &downloaded("0.1.1"),
                kind,
                kind != InstallKind::Transition,
            );
            assert_eq!(
                e,
                InstallError::FailedAfterExit("ShellExecuteW 失敗".into())
            );
            assert!(!e.host_is_intact(), "呼叫端不得回頭建 UI");
            assert_eq!(
                events(&f).last().map(String::as_str),
                Some("relaunch:ShellExecuteW 失敗"),
                "{kind:?}：失敗訊息交給重新啟動退路"
            );
        }
    }

    #[test]
    fn install_returning_ok_is_a_contract_violation_not_a_success() {
        let f = fake(InstallScript::ReturnsOk);
        let e = run_install(&f, &downloaded("0.1.1"), InstallKind::Auto, true);
        assert_eq!(e, InstallError::ReturnedWithoutExit);
        assert!(e.host_is_intact());
    }

    #[test]
    fn quit_in_progress_blocks_every_kind_without_touching_anything() {
        // 系統匣「結束」進行中（QUIT_STARTED）或已在因更新結束：不啟動安裝、不寫紀錄、不呼叫 install。
        for kind in [
            InstallKind::Manual,
            InstallKind::Auto,
            InstallKind::Transition,
        ] {
            let f = fake(InstallScript::AfterExit);
            *f.quit.lock().unwrap() = true;
            let e = run_install(&f, &downloaded("0.1.1"), kind, true);
            assert_eq!(e, InstallError::QuitInProgress, "{kind:?}");
            assert!(events(&f).is_empty(), "{kind:?}：{:?}", events(&f));
            assert!(f.read_state().is_none());
        }
    }

    #[test]
    fn automatic_kinds_refuse_to_install_without_a_loop_guard_record() {
        for kind in [InstallKind::Auto, InstallKind::Transition] {
            let f = fake(InstallScript::AfterExit);
            *f.write_fails.lock().unwrap() = true;
            let e = run_install(&f, &downloaded("0.1.1"), kind, true);
            assert!(matches!(e, InstallError::NotStarted(_)), "{kind:?}");
            assert!(
                !events(&f).iter().any(|l| l.starts_with("install_now")),
                "沒有防循環紀錄就不得自動安裝"
            );
        }
    }

    #[test]
    fn manual_install_proceeds_even_if_the_record_cannot_be_written() {
        let f = fake(InstallScript::BeforeExit);
        *f.write_fails.lock().unwrap() = true;
        let e = run_install(&f, &downloaded("0.1.1"), InstallKind::Manual, true);
        assert!(matches!(e, InstallError::FailedBeforeExit(_)));
        assert_eq!(events(&f), vec!["install_now:0.1.1"]);
    }

    #[test]
    fn ui_built_is_passed_through_to_the_installer() {
        let f = fake(InstallScript::BeforeExit);
        let _ = run_install(&f, &downloaded("0.1.1"), InstallKind::Transition, false);
        assert_eq!(*f.last_ui_built.lock().unwrap(), Some(false));
        let _ = run_install(&f, &downloaded("0.1.1"), InstallKind::Manual, true);
        assert_eq!(*f.last_ui_built.lock().unwrap(), Some(true));
    }
}
