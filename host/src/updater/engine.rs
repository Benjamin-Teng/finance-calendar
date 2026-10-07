//! 更新器決策核心：檢查排程、下載與驗簽結果的處置、過渡期狀態機的驅動（design.md D3）。
//!
//! 與 Tauri／外掛隔離：網路與外掛由 [`UpdateBackend`] 注入、對外副作用（建立 UI、重放交接、同步安裝、
//! 通知）由 [`Effects`] 注入、睡眠由 [`Timer`] 注入，單元測試全部以假物件驅動、不碰網路、不真的睡。
//!
//! ## 一個週期
//!
//! ```text
//! backend.check()
//!   ├─ Err／沒有新版／版本不高於目前 ─▶ 「沒有新版」結果
//!   └─ 有新版 ─▶ （已持有同版本則略過）backend.download()（內含驗簽）
//!        ├─ Err（含簽章不符）─▶ 「沒有新版」結果（不產生 InstallReady，不執行該檔）
//!        └─ Ok ─▶ 「有新版」結果
//!
//! 「沒有新版」結果：過渡期等待中 → 轉移到「建立 UI」；其餘狀態不做事。
//! 「有新版」結果：
//!   等待中   → （24 小時退避未擋）轉移到「安裝」，install::run_install(Transition, ui_built=false)；
//!              失敗（on_before_exit 之前）→ 退路：建立 UI，丟掉更新（下個週期重新下載）；
//!              on_before_exit 之後失敗 → 已重新啟動自己（不返回），不建 UI
//!   其餘     → 一般規則：位元組留在記憶體，同一版本只通知一次（effects.notify_ready）；
//!              `--autostart` 啟動後 10 分鐘內、UI 已建立、未被退避 → 自動安裝（Auto）；
//!              符合條件但 UI 還在建立 → 延後項，UI 建好後在背景執行緒重新評估（不裝才保留並通知）
//! 手動：系統匣選單 → install_manual()（背景執行緒），位元組留在記憶體直到成功（成功＝行程結束）
//! ```
//!
//! 每個週期都包在 `catch_unwind` 裡：週期內任何 panic 只記錄、視同失敗，不影響下一個週期，也不會讓
//! 過渡期卡在「等待」。

use std::any::Any;
use std::io;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::cancel::{CancelToken, Timer};
use super::install::{self, AutoDecision, InstallError, InstallKind, InstallSystem};
use super::schedule::{
    check_due, CheckDue, FirstCheck, StartMode, CHECK_INTERVAL, CHECK_POLL_INTERVAL,
    TRANSITION_TIMEOUT,
};
use super::transition::{Phase, QuitDisposition, Transition};
use super::version;
use super::LOG_TARGET;

/// 後端（外掛）錯誤。分類只影響記錄文字。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendError {
    /// 連線、逾時、HTTP 狀態等。
    Network(String),
    /// 簽章或版本驗證失敗：該檔案不得執行。
    Verify(String),
    /// 其他（設定、平台鍵找不到等）。
    Other(String),
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Network(e) => write!(f, "網路錯誤：{e}"),
            Self::Verify(e) => write!(f, "簽章驗證失敗：{e}"),
            Self::Other(e) => write!(f, "{e}"),
        }
    }
}

/// 檢查到的新版（尚未下載）。`handle` 是後端自己的東西（正式＝外掛的 `Update`）。
pub struct Candidate {
    pub version: String,
    pub handle: Box<dyn Any + Send + Sync>,
}

/// 已下載且驗簽通過的更新（位元組在記憶體）。
pub struct Downloaded {
    pub version: String,
    /// 安裝檔位元組（留在記憶體），交給外掛的 `Update::install`。
    pub bytes: Vec<u8>,
    /// 後端自己的東西（正式＝外掛的 `Update`，`install` 時取出來呼叫）。
    pub handle: Box<dyn Any + Send + Sync>,
}

/// 外掛抽象。契約：`check` 只在遠端版本高於目前版本時回 `Some`（核心仍會再比一次）；`download`
/// 回傳的位元組**已通過簽章驗證**（驗證失敗回 [`BackendError::Verify`]）。
pub trait UpdateBackend: Send + Sync {
    /// 等網路就緒，最多 `max_wait`；`cancel` 被取消就提早返回。回傳是否就緒。
    fn wait_network(&self, max_wait: Duration, cancel: &CancelToken) -> bool;
    fn check(&self) -> Result<Option<Candidate>, BackendError>;
    fn download(&self, candidate: Candidate) -> Result<Downloaded, BackendError>;
}

/// 「已下載並驗簽通過、可以安裝」事件（通知與系統匣選單項據此顯示）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallReady {
    pub version: String,
}

/// 「建立 UI」的執行憑證：建立 UI 的閉包在主執行緒執行時用它向狀態機確認。
pub struct BuildTicket {
    core: Arc<UpdaterCore>,
}

impl BuildTicket {
    /// 狀態機仍停在「建立 UI」＝還需要建。已轉到「結束」（投遞期間使用者按了系統匣結束）就不該建。
    pub fn still_wanted(&self) -> bool {
        self.core.transition.phase() == Phase::BuildingUi
    }

    /// UI 建完：轉到「UI 已建立」、依序重放排隊的交接，有延後的自動安裝就交給背景執行緒重新評估。
    pub fn finish(self) {
        self.core.finish_build_ui();
    }
}

/// 核心對外的副作用。
pub trait Effects: Send + Sync {
    /// 請在**主執行緒**建立小工具與桌布協調（呼叫端保證只呼叫一次）。實際執行的閉包開頭必須先問
    /// [`BuildTicket::still_wanted`]（投遞到執行之間使用者可能已按「結束」，此時要略過），建完呼叫
    /// [`BuildTicket::finish`]。
    fn build_ui(&self, ticket: BuildTicket);
    /// 處理一個過渡期排隊的 single-instance 交接（UI 已建立）。
    fn replay_handoff(&self, args: Vec<String>);
    /// 一般規則：新版已下載並驗簽，通知使用者並更新系統匣選單（同一版本只會呼叫一次）。
    /// 呼叫時 [`UpdaterCore::ready_version`] 已是這個版本。
    fn notify_ready(&self, ready: &InstallReady);
    /// 重建系統匣選單（更新被丟掉、或重新下載到已通知過的同一版本時，讓「更新到 vX.Y.Z 並重新啟動」跟著消失／回來）。
    fn refresh_menu(&self);
    /// 手動安裝在 `on_before_exit` 之前失敗（或 panic）：發 Warning 通知「更新失敗，稍後會再試」（審查 I2）。
    fn notify_install_failed(&self, version: &str);
    /// 使用者點了更新但被拒絕（小工具尚未就緒、已有安裝進行中、沒有可裝的更新）：通知原因（審查 M5）。
    fn notify_install_refused(&self, why: &str);
    /// 在**背景執行緒**執行 `job`（延後的自動安裝，見 `UpdaterCore::resume_deferred`）。呼叫端在
    /// 主執行緒（UI 剛建完）：`install()` 的 `on_before_exit` 要等主執行緒，所以絕不能在呼叫端執行緒同步執行。
    /// 回 `Err`＝無法建立執行緒（`job` 已丟掉）。
    fn spawn_background(&self, job: Box<dyn FnOnce() + Send>) -> io::Result<()>;
}

/// 一般規則下自動安裝的結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AutoOutcome {
    /// 沒有嘗試（條件不符、已有安裝進行中、結束進行中）：照常保留並通知。
    NotAttempted,
    /// 符合自動安裝條件、但 UI 尚未建好（最終審查 M2）：已記為延後項，UI 建好後在背景重新評估；先不保留、不通知。
    Deferred,
    /// `on_before_exit` 之後失敗：已交給重新啟動退路，行程即將結束。
    Ending,
    /// 在 `on_before_exit` 之前失敗（或 panic）：更新已丟掉，不保留、不通知。
    Failed,
}

pub struct UpdaterCore {
    current_version: String,
    backend: Arc<dyn UpdateBackend>,
    effects: Arc<dyn Effects>,
    timer: Arc<dyn Timer>,
    transition: Arc<Transition>,
    /// 取消更新任務（過渡期使用者結束、行程結束）。
    stop: CancelToken,
    /// 已下載並驗簽的更新（一般規則下留給使用者點選安裝）。`Arc`：手動安裝不取走它（失敗時選單項與
    /// 位元組都還在），也避免複製數十 MB。
    ready: Mutex<Option<Arc<Downloaded>>>,
    /// 已通知過的版本。
    notified: Mutex<Option<String>>,
    /// 安裝的外部世界（時間、狀態檔、外掛 `install()`、失敗退路）。
    installer: Arc<dyn InstallSystem>,
    /// 本行程的啟動方式（`--autostart` 才可能自動安裝）。
    mode: StartMode,
    /// 有一個安裝正在進行（同一時間只允許一個：選單連點、手動與自動並發都擋）。
    installing: AtomicBool,
    /// 延後的自動安裝（最終審查 M2）：符合自動安裝條件時 UI 還在建立，UI 建好後由
    /// `finish_build_ui` 取走、在背景執行緒重新評估。
    deferred_auto: Mutex<Option<DeferredAuto>>,
    /// 測試掛鉤：在 `try_auto_install` 記下延後項的前／後模擬 `finish_build_ui` 插進來（複審 Minor 2）。
    #[cfg(test)]
    race_hook: Mutex<Option<RaceHook>>,
}

/// 延後的自動安裝：更新本體＋第一次評估（下載完成）時的 uptime。重新評估時 10 分鐘窗口用這個值（spec：啟動後
/// 10 分鐘內找到並下載完），退避、phase、安裝互斥則重新查（複審 Minor 1）。
struct DeferredAuto {
    downloaded: Arc<Downloaded>,
    first_uptime: Duration,
}

/// 測試掛鉤的觸發點。
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RaceHook {
    /// 第一次讀 phase 之後、記下延後項之前。
    BeforeRecord,
    /// 記下延後項之後、再讀 phase 之前。
    AfterRecord,
}

/// 安裝進行中的旗標（離開作用域一定釋放；成功安裝時行程直接結束，不需要釋放）。
struct InstallGuard<'a>(&'a AtomicBool);

impl<'a> InstallGuard<'a> {
    fn acquire(flag: &'a AtomicBool) -> Option<Self> {
        flag.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| Self(flag))
    }
}

impl Drop for InstallGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// 手動安裝（系統匣選單）的結果，供呼叫端記錄與測試；成功安裝時行程已結束，沒有「成功」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManualOutcome {
    /// 沒有開始（狀態不對、沒有可裝的更新、已有安裝進行中）。
    Refused(String),
    /// `install()` 失敗但宿主還在原狀（`on_before_exit` 之前失敗時更新已丟掉，下個週期重新下載；結束進行中沒開始時更新保留）。
    Failed(InstallError),
    /// `on_before_exit` 之後失敗：已重新啟動自己（正式環境不會返回）。
    Relaunched(InstallError),
}

impl UpdaterCore {
    pub fn new(
        current_version: &str,
        backend: Arc<dyn UpdateBackend>,
        effects: Arc<dyn Effects>,
        timer: Arc<dyn Timer>,
        transition: Arc<Transition>,
        installer: Arc<dyn InstallSystem>,
        mode: StartMode,
    ) -> Arc<Self> {
        Arc::new(Self {
            current_version: current_version.to_owned(),
            backend,
            effects,
            timer,
            transition,
            stop: CancelToken::new(),
            ready: Mutex::new(None),
            notified: Mutex::new(None),
            installer,
            mode,
            installing: AtomicBool::new(false),
            deferred_auto: Mutex::new(None),
            #[cfg(test)]
            race_hook: Mutex::new(None),
        })
    }

    pub fn transition(&self) -> &Arc<Transition> {
        &self.transition
    }

    /// 啟動更新任務執行緒（自己的背景任務，與其他功能隔離）。
    pub fn spawn(self: &Arc<Self>, plan: FirstCheck) -> io::Result<JoinHandle<()>> {
        let me = Arc::clone(self);
        thread::Builder::new()
            .name("fc-updater".to_owned())
            .spawn(move || me.run(plan))
    }

    /// 啟動過渡期逾時看門狗：離開「等待」時醒來退出；睡滿 60 秒仍在「等待」就轉移到「建立 UI」。
    ///
    /// 60 秒以**清醒時間**計（相對逾時，系統睡眠不計入）是正確語意：它是「等網路＋檢查＋下載」的工作預算，
    /// 睡眠期間這些工作也沒有進展；醒來後繼續倒數剩下的部分。
    pub fn spawn_watchdog(self: &Arc<Self>) -> io::Result<JoinHandle<()>> {
        let me = Arc::clone(self);
        thread::Builder::new()
            .name("fc-updater-watchdog".to_owned())
            .spawn(move || {
                let left_waiting = me.transition.left_waiting_token();
                if !me.timer.sleep(TRANSITION_TIMEOUT, &left_waiting) {
                    me.on_transition_timeout();
                }
            })
    }

    /// 過渡期 60 秒逾時：仍在「等待」才轉移到「建立 UI」（晚到的更新結果之後照一般規則處理）。
    pub fn on_transition_timeout(self: &Arc<Self>) {
        if self.transition.phase() == Phase::Waiting {
            log::warn!(
                target: LOG_TARGET,
                "更新過渡期：{} 秒內沒有結果，改為建立小工具（更新任務仍在背景進行）",
                TRANSITION_TIMEOUT.as_secs()
            );
            self.enter_build_ui();
        }
    }

    /// 系統匣「結束」的處置；過渡期中結束時順便取消更新任務。
    pub fn on_tray_quit(&self) -> QuitDisposition {
        let disposition = self.transition.quit_disposition();
        if disposition == QuitDisposition::QuitDuringTransition {
            log::info!(target: LOG_TARGET, "更新過渡期：使用者按了「結束」，取消更新任務");
            self.stop.cancel();
        }
        disposition
    }

    /// 過渡期（「等待」／「建立 UI」）收到 `--restore-wallpaper` 交接（最終審查 M2a）：轉到「結束」並取消更新任務，
    /// 回 `true`——呼叫端回報「交接失敗」並走 `quit_during_transition`（不還原、不存主題、刪標記、結束），命令列
    /// 等宿主結束後在自己的行程還原。過渡期沒有協調迴圈，交給宿主只會「交接失敗」而宿主不結束，命令列等不到它
    /// 結束就回結束碼 4、桌布停在宿主的圖。其餘 phase 不動、回 `false`（照原本的交接處理）。
    pub fn on_restore_handoff(&self) -> bool {
        if !self.transition.begin_quit() {
            return false;
        }
        log::info!(
            target: LOG_TARGET,
            "更新過渡期：收到 --restore-wallpaper 交接，取消更新任務、結束宿主，讓命令列在自己的行程還原"
        );
        self.stop.cancel();
        true
    }

    /// 過渡期是否已轉到「結束」（複審 M-A：建立 UI 途中的巢狀訊息泵可能派送 `--restore-wallpaper` 交接而轉到
    /// 「結束」，建立工作在啟動背景元件之前再問一次）。
    pub fn is_quitting(&self) -> bool {
        self.transition.phase() == Phase::Quit
    }

    /// 取走已下載並驗簽的更新（測試用；手動安裝不取走，見 [`Self::install_manual`]）。
    #[cfg(test)]
    pub fn take_ready(&self) -> Option<Arc<Downloaded>> {
        self.ready.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    /// 目前持有（已下載並驗簽通過、等待安裝）的更新版本；系統匣選單據此顯示「更新到 vX.Y.Z 並重新啟動」。
    pub fn ready_version(&self) -> Option<String> {
        self.ready
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|d| d.version.clone())
    }

    /// 沒有過渡期時 setup 同步建完 UI（phase 從 [`Transition::building_ui`] 起始，最終審查 M2）：轉到「UI 已建立」、
    /// 依序重放建立途中排隊的交接，有延後的自動安裝就交給背景執行緒重新評估。建立途中已轉到「結束」時不轉移、
    /// 不重放，延後項在背景被丟棄。
    pub fn finish_inline_build_ui(self: &Arc<Self>) {
        self.finish_build_ui();
    }

    /// 無法啟動更新任務或看門狗時的安全閥：直接轉移到「建立 UI」，不讓宿主卡在沒有小工具的過渡期。
    pub fn fail_open(self: &Arc<Self>) {
        self.enter_build_ui();
    }

    /// 通知停止更新任務（行程結束路徑）。
    pub fn stop(&self) {
        self.stop.cancel();
    }

    // ── 任務本體 ────────────────────────────────────────────────────────────────

    fn run(self: &Arc<Self>, plan: FirstCheck) {
        let proceed = match plan {
            // 首檢延遲以清醒時間計（相對逾時）：量的是「啟動後跑了多久」，見 `schedule` 模組文件「兩種時間」。
            FirstCheck::AfterDelay(delay) => {
                log::info!(
                    target: LOG_TARGET,
                    "更新檢查：{} 秒後進行第一次檢查",
                    delay.as_secs()
                );
                !self.timer.sleep(delay, &self.stop)
            }
            FirstCheck::AfterNetwork { max_wait } => {
                let online = self.backend.wait_network(max_wait, &self.stop);
                if !self.stop.is_cancelled() && !online {
                    log::warn!(
                        target: LOG_TARGET,
                        "更新檢查：等網路逾時（{} 秒），仍嘗試檢查",
                        max_wait.as_secs()
                    );
                }
                !self.stop.is_cancelled()
            }
        };
        if !proceed {
            return;
        }
        loop {
            self.run_cycle_guarded();
            if self.wait_for_next_check() {
                break;
            }
        }
    }

    /// 等到下一次週期檢查；回傳 `true`＝被取消。
    ///
    /// 檢查週期以**牆上時鐘**計（含睡眠，design.md D3）：相對逾時在 Windows 8 以後不計入系統睡眠時間，直接睡
    /// 6 小時會讓常闔蓋的筆電拉長成好幾天。改為每 [`CHECK_POLL_INTERVAL`]（清醒時間）醒來，以
    /// [`check_due`] 比對 `SystemTime` 距上次檢查是否已滿 [`CHECK_INTERVAL`]——睡眠醒來後最晚一個醒來間隔內檢查。
    /// 時鐘倒退時把起點重設為現在；前跳只觸發一次檢查（檢查完起點改為新的「現在」）。取消會立即喚醒睡眠。
    fn wait_for_next_check(&self) -> bool {
        let mut last_check = self.timer.wall_now();
        loop {
            if self.timer.sleep(CHECK_POLL_INTERVAL, &self.stop) {
                return true;
            }
            let now = self.timer.wall_now();
            match check_due(last_check, now, CHECK_INTERVAL) {
                CheckDue::Due => return false,
                CheckDue::NotYet => {}
                CheckDue::ClockWentBack => {
                    log::info!(
                        target: LOG_TARGET,
                        "更新檢查：系統時鐘比上次檢查早（被調整過），從現在起重新計算 {} 小時",
                        CHECK_INTERVAL.as_secs() / 3600
                    );
                    last_check = now;
                }
            }
        }
    }

    /// 一個週期，`catch_unwind` 隔離：panic 只記錄、視同失敗。
    fn run_cycle_guarded(self: &Arc<Self>) {
        let outcome = catch_unwind(AssertUnwindSafe(|| self.cycle()));
        if let Err(payload) = outcome {
            log::error!(
                target: LOG_TARGET,
                "更新週期發生 panic（已隔離，下個週期再試）：{}",
                panic_message(payload.as_ref())
            );
            self.on_no_update();
        }
    }

    fn cycle(self: &Arc<Self>) {
        let candidate = match self.backend.check() {
            Ok(Some(c)) => c,
            Ok(None) => {
                log::info!(target: LOG_TARGET, "更新檢查：目前已是最新版本");
                self.on_no_update();
                return;
            }
            Err(e) => {
                log::warn!(target: LOG_TARGET, "更新檢查失敗：{e}");
                self.on_no_update();
                return;
            }
        };
        if !version::is_newer(&self.current_version, &candidate.version) {
            log::warn!(
                target: LOG_TARGET,
                "更新檢查：遠端版本 {} 不高於目前 {}，忽略（不降版）",
                candidate.version,
                self.current_version
            );
            self.on_no_update();
            return;
        }
        if self.ready_version().as_deref() == Some(candidate.version.as_str()) {
            log::info!(
                target: LOG_TARGET,
                "更新檢查：v{} 已下載並驗簽，不重複下載",
                candidate.version
            );
            return;
        }
        log::info!(target: LOG_TARGET, "更新檢查：發現新版 v{}，開始下載", candidate.version);
        match self.backend.download(candidate) {
            Ok(downloaded) => self.on_update_downloaded(downloaded),
            Err(e @ BackendError::Verify(_)) => {
                log::error!(target: LOG_TARGET, "更新檔未通過驗證，不安裝：{e}");
                self.on_no_update();
            }
            Err(e) => {
                log::warn!(target: LOG_TARGET, "更新下載失敗：{e}");
                self.on_no_update();
            }
        }
    }

    // ── 結果處置 ────────────────────────────────────────────────────────────────

    /// 「沒有新版／失敗」：過渡期等待中才有意義——轉移到建立 UI。
    fn on_no_update(self: &Arc<Self>) {
        self.enter_build_ui();
    }

    /// 「有新版」：已下載並驗簽通過。
    fn on_update_downloaded(self: &Arc<Self>, downloaded: Downloaded) {
        let downloaded = Arc::new(downloaded);
        let version = downloaded.version.clone();
        if self.transition.phase() == Phase::Waiting {
            // 過渡期（當機迴圈保護）：同步安裝同樣受 24 小時退避限制（design.md D3）。
            let decision = self.decide_auto(InstallKind::Transition, &version);
            install::log_decision(InstallKind::Transition, &version, &decision);
            match decision {
                AutoDecision::NotifyOnly(_) => {
                    // 退避：只通知；小工具照常建立（不能因為不裝就卡在過渡期）。
                    self.keep_and_notify(downloaded);
                    self.enter_build_ui();
                    return;
                }
                AutoDecision::Install => {
                    if self.transition.begin_install() {
                        self.install_in_transition(downloaded);
                        return;
                    }
                    // 轉移被別條路徑搶先（例如使用者按了「結束」）：照下面的一般規則處置。
                }
            }
        }
        // 一般規則（含「逾時後下載才完成」）。
        match self.transition.phase() {
            Phase::Installing | Phase::Quit => {
                log::info!(
                    target: LOG_TARGET,
                    "更新結果 v{version} 晚到，但行程已在 {:?}，捨棄",
                    self.transition.phase()
                );
            }
            Phase::Waiting | Phase::BuildingUi | Phase::UiReady => {
                match self.try_auto_install(&downloaded, None) {
                    // on_before_exit 之後失敗：已重新啟動自己，行程即將結束。
                    // 自動安裝在 on_before_exit 之前失敗：更新已丟掉、不通知（通知會叫使用者去選單，但選單項已消失）；
                    // 下個週期重新下載後，依一般規則通知。
                    // 延後：UI 建好後在背景重新評估，屆時不裝才保留並通知。
                    AutoOutcome::Ending | AutoOutcome::Failed | AutoOutcome::Deferred => {}
                    AutoOutcome::NotAttempted => self.keep_and_notify(downloaded),
                }
            }
        }
    }

    /// 取走延後的自動安裝（沒有就回 `None`）。
    fn take_deferred(&self) -> Option<DeferredAuto> {
        self.deferred_auto
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }

    /// 取走並重新評估延後的自動安裝（沒有就什麼都不做；測試入口）。**不得在主執行緒呼叫**，見
    /// [`Self::resume_deferred`]；正式路徑由 [`Self::finish_build_ui`] 取走後直接把延後項交給背景執行緒。
    #[cfg(test)]
    pub fn resume_deferred_auto_install(self: &Arc<Self>) {
        if let Some(deferred) = self.take_deferred() {
            self.resume_deferred(deferred);
        }
    }

    /// UI 建好後重新評估延後的自動安裝（最終審查 M2）。**不得在主執行緒呼叫**（`install()` 的 `on_before_exit`
    /// 要等主執行緒）。重新經 `try_auto_install`：10 分鐘窗口用第一次評估時的 uptime，24 小時退避、`UiReady`
    /// 檢查、安裝互斥重新查；不裝就照一般規則保留並通知。UI 沒建成（建立途中轉到「結束」）就丟棄、不安裝也不通知。
    fn resume_deferred(self: &Arc<Self>, deferred: DeferredAuto) {
        debug_assert!(
            !super::exit::current_is_main_thread(),
            "延後的自動安裝不得在主執行緒重新評估"
        );
        let DeferredAuto {
            downloaded,
            first_uptime,
        } = deferred;
        let version = downloaded.version.clone();
        let phase = self.transition.phase();
        if phase != Phase::UiReady {
            log::info!(
                target: LOG_TARGET,
                "延後的自動安裝 v{version}：目前狀態 {phase:?}（UI 沒有建成），丟棄、不安裝"
            );
            return;
        }
        log::info!(target: LOG_TARGET, "延後的自動安裝 v{version}：UI 已建好，重新評估");
        match self.try_auto_install(&downloaded, Some(first_uptime)) {
            AutoOutcome::Ending | AutoOutcome::Failed | AutoOutcome::Deferred => {}
            AutoOutcome::NotAttempted => self.keep_and_notify(downloaded),
        }
    }

    /// UI 建好之後（主執行緒上）：有延後的自動安裝就取走、交給背景執行緒重新評估，只投遞、不等待。建立不了執行緒
    /// 就退回保留＋重建選單（選單項可用；氣球通知盡力而為）。
    fn spawn_deferred_auto_install(self: &Arc<Self>) {
        let Some(deferred) = self.take_deferred() else {
            return;
        };
        let fallback = Arc::clone(&deferred.downloaded);
        let me = Arc::clone(self);
        let job: Box<dyn FnOnce() + Send> = Box::new(move || {
            if let Err(payload) = catch_unwind(AssertUnwindSafe(|| me.resume_deferred(deferred))) {
                log::error!(
                    target: LOG_TARGET,
                    "延後的自動安裝發生 panic（已隔離）：{}",
                    panic_message(payload.as_ref())
                );
            }
        });
        if let Err(e) = self.effects.spawn_background(job) {
            log::error!(
                target: LOG_TARGET,
                "延後的自動安裝：無法建立背景執行緒（{e}），改為只通知"
            );
            self.keep_and_notify(fallback);
        }
    }

    /// 測試掛鉤（複審 Minor 2）：設定的觸發點到了就模擬 `finish_build_ui` 在此插進來。
    #[cfg(test)]
    fn run_race_hook(self: &Arc<Self>, at: RaceHook) {
        let fire = {
            let mut hook = self.race_hook.lock().unwrap_or_else(|e| e.into_inner());
            if *hook == Some(at) {
                *hook = None;
                true
            } else {
                false
            }
        };
        if fire {
            self.finish_inline_build_ui();
        }
    }

    /// 過渡期的同步安裝（沒有 UI）。成功時行程在 `install()` 內結束、不會返回；返回只有：
    /// - `on_before_exit` 之後失敗（正式環境已重新啟動自己並結束，不會返回；測試只記錄）→ **不得**建 UI；
    /// - 其餘（沒有開始、`install()` 在結束流程之前失敗、panic、回 Ok 卻沒結束）→ 行程還在，一律走退路建立 UI
    ///   ——phase 不得停在 Installing（否則沒有小工具、系統匣「結束」也被忽略）。更新沒裝成時丟掉（下個週期重新
    ///   檢查、重新下載，審查 I1）；只有「結束進行中」沒開始時才照一般規則保留並通知。
    fn install_in_transition(self: &Arc<Self>, downloaded: Arc<Downloaded>) {
        log::info!(
            target: LOG_TARGET,
            "更新過渡期：v{} 已下載並驗簽，轉入同步安裝（不建立小工具）",
            downloaded.version
        );
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            install::run_install(
                &*self.installer,
                &downloaded,
                InstallKind::Transition,
                false,
            )
        }));
        let (why, discard) = match outcome {
            Ok(e) if !e.host_is_intact() => {
                log::error!(
                    target: LOG_TARGET,
                    "更新過渡期：同步安裝在結束流程之後失敗（{e}），已交給重新啟動退路，不建立小工具"
                );
                return;
            }
            Ok(other) => (other.to_string(), other.should_discard_update()),
            Err(payload) => (
                format!("panic（{}）", panic_message(payload.as_ref())),
                true,
            ),
        };
        log::warn!(target: LOG_TARGET, "更新過渡期：同步安裝未成功：{why}；改為建立小工具");
        if self.transition.install_failed_retreat() {
            self.run_build_ui();
        }
        if discard {
            self.discard_ready_after_failed_install(&downloaded.version);
        } else {
            self.keep_and_notify(downloaded);
        }
    }

    /// 一般規則下是否自動安裝：只有 `--autostart` 啟動後 10 分鐘內、UI 已建立、未被 24 小時退避擋下才裝。
    ///
    /// 符合條件但 UI 還在建立（沒有舊標記時 setup 同步建立中、或過渡期逾時後撞上建立中）：不在建立中途收尾，記為
    /// 延後項、回 [`AutoOutcome::Deferred`]，UI 建好後由 [`Self::resume_deferred`] 重新評估（最終審查 M2）。
    /// 記錄只在真的開始時才寫「符合條件，開始安裝」，不會先寫開始再寫改為只通知（審查 Nit 4）。
    ///
    /// `first_uptime`：重新評估延後項時傳第一次評估的 uptime（10 分鐘窗口用它）；`None`＝用現在的 uptime。
    fn try_auto_install(
        self: &Arc<Self>,
        downloaded: &Arc<Downloaded>,
        first_uptime: Option<Duration>,
    ) -> AutoOutcome {
        debug_assert!(
            !super::exit::current_is_main_thread(),
            "自動安裝不得在主執行緒評估"
        );
        let version = &downloaded.version;
        let uptime = first_uptime.unwrap_or_else(|| self.installer.uptime());
        let decision = self.decide_auto_at(InstallKind::Auto, version, uptime);
        if decision != AutoDecision::Install {
            install::log_decision(InstallKind::Auto, version, &decision);
            return AutoOutcome::NotAttempted;
        }
        match self.transition.phase() {
            Phase::UiReady => {}
            phase @ (Phase::Waiting | Phase::BuildingUi) => {
                #[cfg(test)]
                self.run_race_hook(RaceHook::BeforeRecord);
                *self.deferred_auto.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some(DeferredAuto {
                        downloaded: Arc::clone(downloaded),
                        first_uptime: uptime,
                    });
                #[cfg(test)]
                self.run_race_hook(RaceHook::AfterRecord);
                // 記下之後再看一次：`finish_build_ui` 若在記下之前就轉到 UiReady，它看不到這個延後項。兩邊各自「先寫
                // 自己的、再讀對方的」（各經一把鎖），至少一邊看得到對方；`take` 保證只有一邊取得。
                if self.transition.phase() != Phase::UiReady {
                    log::info!(
                        target: LOG_TARGET,
                        "自動安裝 v{version}：UI 建立中（{phase:?}），延後到建好後再評估"
                    );
                    return AutoOutcome::Deferred;
                }
                if self.take_deferred().is_none() {
                    // `finish_build_ui` 已取走，交給它的背景執行緒。
                    return AutoOutcome::Deferred;
                }
            }
            phase @ (Phase::Installing | Phase::Quit) => {
                log::info!(target: LOG_TARGET, "自動安裝 v{version}：目前狀態 {phase:?}，不安裝");
                return AutoOutcome::NotAttempted;
            }
        }
        let Some(_guard) = InstallGuard::acquire(&self.installing) else {
            log::info!(target: LOG_TARGET, "自動安裝 v{version}：已有安裝進行中，只通知");
            return AutoOutcome::NotAttempted;
        };
        install::log_decision(InstallKind::Auto, version, &decision);
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            install::run_install(&*self.installer, downloaded, InstallKind::Auto, true)
        }));
        match outcome {
            Ok(e) if !e.host_is_intact() => AutoOutcome::Ending,
            Ok(e) if e.should_discard_update() => {
                log::warn!(
                    target: LOG_TARGET,
                    "自動安裝 v{version} 未成功：{e}；丟掉這份更新，下個週期重新檢查並下載"
                );
                self.discard_ready_after_failed_install(version);
                AutoOutcome::Failed
            }
            Ok(e) => {
                log::warn!(target: LOG_TARGET, "自動安裝 v{version} 未成功：{e}；改為只通知");
                AutoOutcome::NotAttempted
            }
            Err(payload) => {
                log::error!(
                    target: LOG_TARGET,
                    "自動安裝 v{version} 發生 panic（已隔離）：{}；丟掉這份更新，下個週期重新檢查並下載",
                    panic_message(payload.as_ref())
                );
                self.discard_ready_after_failed_install(version);
                AutoOutcome::Failed
            }
        }
    }

    /// 系統匣選單「更新到 vX.Y.Z 並重新啟動」（手動安裝）。**必須在背景執行緒呼叫**（`install()` 要等
    /// 收尾，且 `prepare_exit_for_update` 不得在主執行緒執行）。安裝中不取走更新；`on_before_exit` 之前失敗（或
    /// panic）時丟掉它（下個週期重新下載，審查 I1）並發「更新失敗，稍後會再試」通知（審查 I2）；被拒絕時也發通知
    /// 說明原因（審查 M5）。「結束進行中」沒開始、`on_before_exit` 之後失敗（已重新啟動）不丟、不通知。
    /// 手動安裝不受 24 小時退避與 10 分鐘窗口限制，但同樣寫入嘗試紀錄（`install::run_install`）。
    pub fn install_manual(self: &Arc<Self>) -> ManualOutcome {
        let phase = self.transition.phase();
        if phase != Phase::UiReady {
            log::warn!(target: LOG_TARGET, "手動安裝：目前狀態 {phase:?}，小工具尚未就緒或正在結束");
            return self.refuse_manual("小工具尚未就緒或正在結束");
        }
        let Some(_guard) = InstallGuard::acquire(&self.installing) else {
            log::warn!(target: LOG_TARGET, "手動安裝：已有安裝進行中");
            return self.refuse_manual("已有安裝進行中");
        };
        let Some(update) = self.ready.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
            log::warn!(target: LOG_TARGET, "手動安裝：沒有已下載並驗簽的更新");
            // 選單項可能是過期的（更新剛被丟掉）：一併重建選單。
            self.effects.refresh_menu();
            return self.refuse_manual("沒有已下載並驗簽的更新");
        };
        log::info!(target: LOG_TARGET, "手動安裝：使用者點選更新到 v{}", update.version);
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            install::run_install(&*self.installer, &update, InstallKind::Manual, true)
        }));
        let error = match outcome {
            Ok(e @ InstallError::FailedAfterExit(_)) => return ManualOutcome::Relaunched(e),
            Ok(e) => e,
            Err(payload) => {
                let why = format!("panic（{}）", panic_message(payload.as_ref()));
                log::error!(target: LOG_TARGET, "手動安裝 v{} 發生 {why}（已隔離）", update.version);
                InstallError::FailedBeforeExit(why)
            }
        };
        if error.should_discard_update() {
            log::warn!(
                target: LOG_TARGET,
                "手動安裝 v{} 未成功：{error}；宿主照常運作，丟掉這份更新，下個週期重新檢查並下載",
                update.version
            );
            self.discard_ready_after_failed_install(&update.version);
            self.effects.notify_install_failed(&update.version);
        } else {
            log::warn!(target: LOG_TARGET, "手動安裝 v{} 沒有開始：{error}", update.version);
        }
        ManualOutcome::Failed(error)
    }

    /// 手動安裝被拒絕：通知使用者原因（審查 M5），回 `Refused`。
    fn refuse_manual(&self, why: &str) -> ManualOutcome {
        self.effects.notify_install_refused(why);
        ManualOutcome::Refused(why.to_owned())
    }

    /// 宿主還在、更新卻沒裝成：丟掉手上持有的更新並重建選單（讓選單項消失），使下個檢查週期因「沒有持有」而
    /// 重新下載（D4「下個週期再試」；寫暫存檔失敗可能是位元組本身有問題，重試同一份永遠失敗）。只丟同一版本，
    /// 不誤丟之後下載到的新版。「已通知過的版本」不清：重新下載同一版本不再發通知，但會重建選單（見
    /// [`Self::keep_and_notify`]）。
    fn discard_ready_after_failed_install(&self, version: &str) {
        {
            let mut ready = self.ready.lock().unwrap_or_else(|e| e.into_inner());
            if ready.as_ref().is_some_and(|d| d.version == version) {
                *ready = None;
            }
        }
        log::info!(target: LOG_TARGET, "已丟掉 v{version} 的更新，下個檢查週期會重新下載");
        self.effects.refresh_menu();
    }

    /// 自動種類的決策（讀嘗試紀錄、時間、啟動方式）。
    fn decide_auto(&self, kind: InstallKind, candidate_version: &str) -> AutoDecision {
        self.decide_auto_at(kind, candidate_version, self.installer.uptime())
    }

    /// 同 [`Self::decide_auto`]，但 10 分鐘窗口用指定的 uptime（延後項重新評估時用第一次評估的值）。
    fn decide_auto_at(
        &self,
        kind: InstallKind,
        candidate_version: &str,
        uptime: Duration,
    ) -> AutoDecision {
        install::decide_auto(
            kind,
            self.mode,
            uptime,
            self.installer.read_state().as_ref(),
            &self.current_version,
            candidate_version,
            self.installer.now_unix(),
        )
    }

    /// 保留更新（供手動安裝）並照「同一版本只通知一次」通知。已通知過的版本（失敗後丟掉、重新下載到同一版本）
    /// 不再通知，但要重建選單讓選單項回來。
    fn keep_and_notify(&self, downloaded: Arc<Downloaded>) {
        let version = downloaded.version.clone();
        match self.transition.phase() {
            Phase::Installing | Phase::Quit => {
                log::info!(
                    target: LOG_TARGET,
                    "更新 v{version}：行程已在 {:?}，不保留也不通知",
                    self.transition.phase()
                );
            }
            Phase::Waiting | Phase::BuildingUi | Phase::UiReady => {
                *self.ready.lock().unwrap_or_else(|e| e.into_inner()) = Some(downloaded);
                if self.mark_notified(&version) {
                    log::info!(target: LOG_TARGET, "新版 v{version} 已下載並驗簽，通知使用者");
                    self.effects.notify_ready(&InstallReady { version });
                } else {
                    log::info!(target: LOG_TARGET, "新版 v{version} 已通知過，只重建選單");
                    self.effects.refresh_menu();
                }
            }
        }
    }

    /// 同一版本只通知一次：第一次回 `true`。
    fn mark_notified(&self, version: &str) -> bool {
        let mut notified = self.notified.lock().unwrap_or_else(|e| e.into_inner());
        if notified.as_deref() == Some(version) {
            return false;
        }
        *notified = Some(version.to_owned());
        true
    }

    // ── 建立 UI ────────────────────────────────────────────────────────────────

    /// 「等待」→「建立 UI」，成功才通知呼叫端建立 UI。
    fn enter_build_ui(self: &Arc<Self>) {
        if self.transition.begin_build_ui() {
            log::info!(target: LOG_TARGET, "更新過渡期：結束，開始建立小工具與桌布協調");
            self.run_build_ui();
        }
    }

    /// 已轉移到 `BuildingUi` 之後：要求主執行緒建立 UI，建完處理排隊的交接。
    fn run_build_ui(self: &Arc<Self>) {
        self.effects.build_ui(BuildTicket {
            core: Arc::clone(self),
        });
    }

    /// UI 建完（過渡期與沒有舊標記兩條路徑共用，在主執行緒上）：轉到「UI 已建立」、重放排隊的交接，之後把延後的
    /// 自動安裝交給背景執行緒——**不得**在這裡同步安裝（`on_before_exit` 要等主執行緒，會死鎖）。
    fn finish_build_ui(self: &Arc<Self>) {
        for args in self.transition.finish_build_ui() {
            self.effects.replay_handoff(args);
        }
        self.spawn_deferred_auto_install();
    }
}

pub(super) fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "（非字串的 panic 內容）".to_owned()
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::updater::install::tests::{FakeInstall, InstallScript};
    use crate::updater::schedule::FirstCheck;
    use crate::updater::transition::HandoffDisposition;
    use std::collections::VecDeque;

    // ── 假物件 ──────────────────────────────────────────────────────────────────

    /// 腳本化的後端：`check`／`download` 依序吐出預先排好的結果，並記錄呼叫。
    #[derive(Default)]
    pub struct FakeBackend {
        pub checks: Mutex<VecDeque<Result<Option<String>, BackendError>>>,
        pub downloads: Mutex<VecDeque<Result<(), BackendError>>>,
        pub check_calls: Mutex<usize>,
        pub download_calls: Mutex<Vec<String>>,
        pub network_waits: Mutex<Vec<Duration>>,
        pub panic_on_check: Mutex<bool>,
    }

    impl FakeBackend {
        pub fn push_check(&self, r: Result<Option<&str>, BackendError>) {
            self.checks
                .lock()
                .unwrap()
                .push_back(r.map(|o| o.map(str::to_owned)));
        }
        pub fn push_download(&self, r: Result<(), BackendError>) {
            self.downloads.lock().unwrap().push_back(r);
        }
    }

    impl UpdateBackend for FakeBackend {
        fn wait_network(&self, max_wait: Duration, _cancel: &CancelToken) -> bool {
            self.network_waits.lock().unwrap().push(max_wait);
            true
        }
        fn check(&self) -> Result<Option<Candidate>, BackendError> {
            *self.check_calls.lock().unwrap() += 1;
            if *self.panic_on_check.lock().unwrap() {
                panic!("假後端：check 內部 panic");
            }
            match self.checks.lock().unwrap().pop_front() {
                None => Ok(None),
                Some(Err(e)) => Err(e),
                Some(Ok(None)) => Ok(None),
                Some(Ok(Some(version))) => Ok(Some(Candidate {
                    version,
                    handle: Box::new(()),
                })),
            }
        }
        fn download(&self, candidate: Candidate) -> Result<Downloaded, BackendError> {
            self.download_calls
                .lock()
                .unwrap()
                .push(candidate.version.clone());
            match self.downloads.lock().unwrap().pop_front() {
                None | Some(Ok(())) => Ok(Downloaded {
                    version: candidate.version,
                    bytes: vec![1, 2, 3],
                    handle: Box::new(()),
                }),
                Some(Err(e)) => Err(e),
            }
        }
    }

    #[derive(Default)]
    pub struct FakeEffects {
        /// 與 `FakeInstall` 共用同一份事件記錄，才看得出安裝與建立 UI、通知的先後。
        pub events: Arc<Mutex<Vec<String>>>,
        /// `true`＝`build_ui` 只把憑證收起來（模擬「已投遞到主執行緒但還沒執行」），之後由 `run_pending` 執行。
        pub defer_build: Mutex<bool>,
        pub pending_build: Mutex<Option<BuildTicket>>,
        /// 複審 M-A：設了就模擬 `main.rs` 的 `build_ui`——建立小工具視窗途中（WebView2 的巢狀訊息泵）派送進來一個
        /// `--restore-wallpaper` 交接，之後在啟動協調迴圈等背景元件之前以 `is_quitting` 再問一次。用 `Weak`
        /// 避免核心與假副作用互相持有。
        pub nested_restore_handoff: Mutex<Option<std::sync::Weak<UpdaterCore>>>,
        /// `spawn_background` 收到的工作（不立即執行，模擬「交給背景執行緒」；測試以 `run_spawned` 執行）。
        pub spawned: Mutex<Vec<Box<dyn FnOnce() + Send>>>,
        /// `true`＝`spawn_background` 回錯誤（模擬無法建立執行緒）。
        pub spawn_fails: Mutex<bool>,
    }

    impl FakeEffects {
        pub fn events(&self) -> Vec<String> {
            self.events.lock().unwrap().clone()
        }

        /// 執行 `spawn_background` 收到的工作（在測試執行緒上，代表背景執行緒）。
        pub fn run_spawned(&self) {
            let jobs = std::mem::take(&mut *self.spawned.lock().unwrap());
            for job in jobs {
                job();
            }
        }

        /// 與 `AppEffects::build_ui` 的閉包同樣的邏輯：還需要才建，否則略過。
        fn run_ticket(&self, ticket: BuildTicket) {
            let nested = self.nested_restore_handoff.lock().unwrap().take();
            if let Some(core) = nested.and_then(|w| w.upgrade()) {
                if !ticket.still_wanted() {
                    self.events.lock().unwrap().push("build_ui_skipped".into());
                    return;
                }
                // `build_ui`：先建小工具視窗（期間巢狀派送交接），再於啟動背景元件前檢查。
                self.events.lock().unwrap().push("widgets".into());
                let quit = core.on_restore_handoff();
                self.events
                    .lock()
                    .unwrap()
                    .push(format!("nested_restore_handoff:{quit}"));
                if core.is_quitting() {
                    self.events
                        .lock()
                        .unwrap()
                        .push("coordinator_not_started".into());
                } else {
                    self.events
                        .lock()
                        .unwrap()
                        .push("coordinator_started".into());
                }
                // `AppEffects::build_ui` 的閉包在 `builder` 返回後一律呼叫 `finish`。
                ticket.finish();
                return;
            }
            if ticket.still_wanted() {
                self.events.lock().unwrap().push("build_ui".into());
                ticket.finish();
            } else {
                self.events.lock().unwrap().push("build_ui_skipped".into());
            }
        }

        pub fn run_pending(&self) {
            let ticket = self.pending_build.lock().unwrap().take();
            if let Some(ticket) = ticket {
                self.run_ticket(ticket);
            }
        }
    }

    impl Effects for FakeEffects {
        fn build_ui(&self, ticket: BuildTicket) {
            if *self.defer_build.lock().unwrap() {
                *self.pending_build.lock().unwrap() = Some(ticket);
            } else {
                self.run_ticket(ticket);
            }
        }
        fn replay_handoff(&self, args: Vec<String>) {
            self.events
                .lock()
                .unwrap()
                .push(format!("replay:{}", args.join(" ")));
        }
        fn notify_ready(&self, ready: &InstallReady) {
            self.events
                .lock()
                .unwrap()
                .push(format!("notify:{}", ready.version));
        }
        fn refresh_menu(&self) {
            self.events.lock().unwrap().push("refresh_menu".into());
        }
        fn notify_install_failed(&self, version: &str) {
            self.events
                .lock()
                .unwrap()
                .push(format!("install_failed:{version}"));
        }
        fn notify_install_refused(&self, why: &str) {
            self.events
                .lock()
                .unwrap()
                .push(format!("install_refused:{why}"));
        }
        fn spawn_background(&self, job: Box<dyn FnOnce() + Send>) -> io::Result<()> {
            if *self.spawn_fails.lock().unwrap() {
                self.events.lock().unwrap().push("spawn_failed".into());
                return Err(io::Error::other("假的：無法建立執行緒"));
            }
            self.events.lock().unwrap().push("spawn_background".into());
            self.spawned.lock().unwrap().push(job);
            Ok(())
        }
    }

    /// 立即返回的計時器：依腳本回報「被取消」與否，預設取消（讓迴圈結束）；記錄要求的時間。
    ///
    /// 假牆上時鐘：每次睡眠前進要求的時間（清醒時間＝牆上時間），之後再套用 `wall_jumps` 排好的一次跳動
    /// （秒，正＝睡眠或前跳、負＝時鐘倒退）。
    pub struct FakeTimer {
        pub script: Mutex<VecDeque<bool>>,
        pub requested: Mutex<Vec<Duration>>,
        pub wall: Mutex<std::time::SystemTime>,
        pub wall_jumps: Mutex<VecDeque<i64>>,
    }

    impl Default for FakeTimer {
        fn default() -> Self {
            Self {
                script: Mutex::default(),
                requested: Mutex::default(),
                wall: Mutex::new(std::time::UNIX_EPOCH + Duration::from_secs(1_800_000_000)),
                wall_jumps: Mutex::default(),
            }
        }
    }

    impl Timer for FakeTimer {
        fn sleep(&self, duration: Duration, _cancel: &CancelToken) -> bool {
            self.requested.lock().unwrap().push(duration);
            let mut wall = self.wall.lock().unwrap();
            *wall += duration;
            if let Some(jump) = self.wall_jumps.lock().unwrap().pop_front() {
                let by = Duration::from_secs(jump.unsigned_abs());
                *wall = if jump >= 0 { *wall + by } else { *wall - by };
            }
            self.script.lock().unwrap().pop_front().unwrap_or(true)
        }

        fn wall_now(&self) -> std::time::SystemTime {
            *self.wall.lock().unwrap()
        }
    }

    struct Rig {
        core: Arc<UpdaterCore>,
        backend: Arc<FakeBackend>,
        effects: Arc<FakeEffects>,
        timer: Arc<FakeTimer>,
        install: Arc<FakeInstall>,
    }

    fn rig(transition: Transition) -> Rig {
        rig_with(transition, StartMode::Normal)
    }

    fn rig_with(transition: Transition, mode: StartMode) -> Rig {
        let backend = Arc::new(FakeBackend::default());
        let effects = Arc::new(FakeEffects::default());
        let timer = Arc::new(FakeTimer::default());
        let install = Arc::new(FakeInstall::new(effects.events.clone()));
        let core = UpdaterCore::new(
            "0.1.0",
            backend.clone(),
            effects.clone(),
            timer.clone(),
            Arc::new(transition),
            install.clone(),
            mode,
        );
        Rig {
            core,
            backend,
            effects,
            timer,
            install,
        }
    }

    fn verify_err() -> BackendError {
        BackendError::Verify("簽章不符".into())
    }

    // ── spec「自動檢查更新」：排程 ─────────────────────────────────────────────────

    /// 6 小時＝72 個 5 分鐘醒來間隔。
    const POLLS_PER_INTERVAL: usize = 72;

    #[test]
    fn normal_start_sleeps_then_checks_then_repeats_every_six_hours() {
        let r = rig(Transition::ui_ready());
        {
            let mut script = r.timer.script.lock().unwrap();
            script.push_back(false); // 首檢延遲睡滿
            script.extend(std::iter::repeat_n(false, POLLS_PER_INTERVAL)); // 6 小時的醒來間隔都睡滿
            script.push_back(true); // 第二次檢查後的第一個醒來間隔被取消
        }
        r.core.run(FirstCheck::AfterDelay(Duration::from_secs(90)));
        assert_eq!(
            *r.backend.check_calls.lock().unwrap(),
            2,
            "延遲後檢查一次、6 小時後再一次"
        );
        let mut want = vec![Duration::from_secs(90)];
        want.extend(std::iter::repeat_n(
            Duration::from_secs(300),
            POLLS_PER_INTERVAL + 1,
        ));
        assert_eq!(*r.timer.requested.lock().unwrap(), want);
    }

    #[test]
    fn periodic_check_is_not_due_before_six_wall_clock_hours() {
        let r = rig(Transition::ui_ready());
        r.timer
            .script
            .lock()
            .unwrap()
            .extend(std::iter::repeat_n(false, POLLS_PER_INTERVAL - 1)); // 5 小時 55 分後被取消
        r.core.run(FirstCheck::AfterNetwork {
            max_wait: Duration::from_secs(300),
        });
        assert_eq!(
            *r.backend.check_calls.lock().unwrap(),
            1,
            "未滿 6 小時不檢查"
        );
    }

    #[test]
    fn periodic_check_runs_on_first_wake_after_two_days_of_sleep() {
        // M1：系統睡眠不計入清醒時間，但牆上時鐘照走——醒來後第一個醒來間隔就檢查，不再等 6 小時清醒時間。
        let r = rig(Transition::ui_ready());
        r.timer.wall_jumps.lock().unwrap().push_back(2 * 24 * 3600);
        r.timer.script.lock().unwrap().push_back(false); // 這個醒來間隔裡睡了兩天
        r.core.run(FirstCheck::AfterNetwork {
            max_wait: Duration::from_secs(300),
        });
        assert_eq!(*r.backend.check_calls.lock().unwrap(), 2);
        assert_eq!(
            *r.timer.requested.lock().unwrap(),
            vec![Duration::from_secs(300), Duration::from_secs(300)],
            "醒來後立即檢查，之後回到 5 分鐘醒來間隔（第二個被取消）"
        );
    }

    #[test]
    fn clock_going_back_restarts_the_interval_without_rapid_checks() {
        // 時鐘倒退一天：起點重設為現在，再滿 6 小時才檢查（不會永不檢查，也不會立即或連續檢查）。
        let r = rig(Transition::ui_ready());
        r.timer.wall_jumps.lock().unwrap().push_back(-24 * 3600);
        r.timer
            .script
            .lock()
            .unwrap()
            .extend(std::iter::repeat_n(false, POLLS_PER_INTERVAL)); // 倒退那次＋其後 71 次
        r.core.run(FirstCheck::AfterNetwork {
            max_wait: Duration::from_secs(300),
        });
        assert_eq!(
            *r.backend.check_calls.lock().unwrap(),
            1,
            "倒退後才過 5 小時 55 分：還沒到"
        );

        let r = rig(Transition::ui_ready());
        r.timer.wall_jumps.lock().unwrap().push_back(-24 * 3600);
        r.timer
            .script
            .lock()
            .unwrap()
            .extend(std::iter::repeat_n(false, POLLS_PER_INTERVAL + 1));
        r.core.run(FirstCheck::AfterNetwork {
            max_wait: Duration::from_secs(300),
        });
        assert_eq!(
            *r.backend.check_calls.lock().unwrap(),
            2,
            "倒退後滿 6 小時：照常檢查"
        );
    }

    #[test]
    fn large_forward_clock_jump_checks_once_then_waits_six_hours_again() {
        let r = rig(Transition::ui_ready());
        r.timer
            .wall_jumps
            .lock()
            .unwrap()
            .push_back(365 * 24 * 3600);
        {
            let mut script = r.timer.script.lock().unwrap();
            script.push_back(false); // 前跳：到期
            script.extend(std::iter::repeat_n(false, POLLS_PER_INTERVAL - 1)); // 之後 5 小時 55 分
        }
        r.core.run(FirstCheck::AfterNetwork {
            max_wait: Duration::from_secs(300),
        });
        assert_eq!(
            *r.backend.check_calls.lock().unwrap(),
            2,
            "前跳只觸發一次檢查，不會連續狂檢查"
        );
    }

    #[test]
    fn cancel_during_a_poll_ends_the_task() {
        let r = rig(Transition::ui_ready());
        r.timer.script.lock().unwrap().extend([false, false, true]);
        r.core.run(FirstCheck::AfterNetwork {
            max_wait: Duration::from_secs(300),
        });
        assert_eq!(*r.backend.check_calls.lock().unwrap(), 1);
        assert_eq!(
            r.timer.requested.lock().unwrap().len(),
            3,
            "第三個醒來間隔被取消就結束，不再睡"
        );
    }

    #[test]
    fn autostart_waits_for_network_then_checks_immediately() {
        let r = rig(Transition::ui_ready());
        r.core.run(FirstCheck::AfterNetwork {
            max_wait: Duration::from_secs(300),
        });
        assert_eq!(
            *r.backend.network_waits.lock().unwrap(),
            vec![Duration::from_secs(300)]
        );
        assert_eq!(
            *r.backend.check_calls.lock().unwrap(),
            1,
            "網路就緒後立即檢查，沒有 60–180 秒延遲"
        );
        assert_eq!(
            *r.timer.requested.lock().unwrap(),
            vec![Duration::from_secs(300)],
            "第一次檢查前沒有任何睡眠（唯一一次是檢查後的第一個醒來間隔）"
        );
    }

    #[test]
    fn cancelled_before_first_check_never_checks() {
        let r = rig(Transition::ui_ready());
        r.core.stop();
        r.core.run(FirstCheck::AfterNetwork {
            max_wait: Duration::from_secs(60),
        });
        assert_eq!(*r.backend.check_calls.lock().unwrap(), 0);
        let r = rig(Transition::ui_ready());
        r.timer.script.lock().unwrap().push_back(true); // 延遲期間被取消
        r.core.run(FirstCheck::AfterDelay(Duration::from_secs(60)));
        assert_eq!(*r.backend.check_calls.lock().unwrap(), 0);
    }

    // ── spec「自動檢查更新」：離線／失敗只記錄 ──────────────────────────────────────

    #[test]
    fn offline_check_only_logs_and_changes_nothing_visible() {
        // 一般啟動（沒有過渡期）：檢查失敗不建 UI、不通知、不安裝。
        let r = rig(Transition::ui_ready());
        r.backend
            .push_check(Err(BackendError::Network("無法連線".into())));
        r.core.run_cycle_guarded();
        assert!(r.effects.events().is_empty(), "離線時不得有任何對外動作");
        assert_eq!(r.core.transition().phase(), Phase::UiReady);
        // 下個週期再試：這次成功。
        r.backend.push_check(Ok(Some("0.1.1")));
        r.core.run_cycle_guarded();
        assert_eq!(r.effects.events(), vec!["notify:0.1.1"]);
    }

    #[test]
    fn download_failure_only_logs() {
        let r = rig(Transition::ui_ready());
        r.backend.push_check(Ok(Some("0.1.1")));
        r.backend
            .push_download(Err(BackendError::Network("逾時".into())));
        r.core.run_cycle_guarded();
        assert!(r.effects.events().is_empty());
        assert!(r.core.take_ready().is_none());
    }

    // ── spec「更新必須通過簽章驗證」／版本規則 ──────────────────────────────────────

    #[test]
    fn tampered_package_never_becomes_install_ready() {
        for phase in [Transition::ui_ready(), Transition::waiting()] {
            let waiting = phase.phase() == Phase::Waiting;
            let r = rig(phase);
            r.backend.push_check(Ok(Some("0.1.1")));
            r.backend.push_download(Err(verify_err()));
            r.core.run_cycle_guarded();
            let events = r.effects.events();
            assert!(
                !events
                    .iter()
                    .any(|e| e.starts_with("notify:") || e.starts_with("install_now:")),
                "驗簽失敗不得產生 InstallReady，也不得安裝：{events:?}"
            );
            assert!(r.core.take_ready().is_none(), "不得保留未通過驗證的位元組");
            if waiting {
                assert_eq!(
                    events,
                    vec!["build_ui"],
                    "過渡期遇到驗簽失敗等同沒有新版：建立 UI"
                );
            }
        }
    }

    #[test]
    fn version_not_higher_than_current_is_ignored() {
        for remote in ["0.1.0", "0.0.9", "garbage"] {
            let r = rig(Transition::ui_ready());
            r.backend.push_check(Ok(Some(remote)));
            r.core.run_cycle_guarded();
            assert!(
                r.backend.download_calls.lock().unwrap().is_empty(),
                "遠端 {remote} 不高於目前 0.1.0：不得下載（不降版）"
            );
            assert!(r.effects.events().is_empty());
        }
    }

    // ── spec「更新的安裝時機」：同一版本只通知一次 ──────────────────────────────────

    #[test]
    fn same_version_is_notified_once_and_not_redownloaded() {
        let r = rig(Transition::ui_ready());
        r.backend.push_check(Ok(Some("0.1.1")));
        r.core.run_cycle_guarded();
        r.backend.push_check(Ok(Some("0.1.1")));
        r.core.run_cycle_guarded();
        assert_eq!(
            r.effects.events(),
            vec!["notify:0.1.1"],
            "同一版本只通知一次"
        );
        assert_eq!(
            *r.backend.download_calls.lock().unwrap(),
            vec!["0.1.1"],
            "已持有同版本，6 小時後的檢查不重新下載數十 MB"
        );
        assert_eq!(
            r.core.take_ready().map(|d| d.version.clone()).as_deref(),
            Some("0.1.1")
        );
    }

    #[test]
    fn newer_version_after_notification_notifies_again() {
        let r = rig(Transition::ui_ready());
        r.backend.push_check(Ok(Some("0.1.1")));
        r.core.run_cycle_guarded();
        r.backend.push_check(Ok(Some("0.1.2")));
        r.core.run_cycle_guarded();
        assert_eq!(r.effects.events(), vec!["notify:0.1.1", "notify:0.1.2"]);
        assert_eq!(
            r.core.take_ready().map(|d| d.version.clone()).as_deref(),
            Some("0.1.2")
        );
    }

    // ── 隔離：panic 不影響任務 ──────────────────────────────────────────────────────

    #[test]
    fn panic_inside_a_cycle_is_contained_and_next_cycle_runs() {
        let r = rig(Transition::ui_ready());
        *r.backend.panic_on_check.lock().unwrap() = true;
        r.core.run_cycle_guarded(); // 不得向外傳播
        *r.backend.panic_on_check.lock().unwrap() = false;
        r.backend.push_check(Ok(Some("0.1.1")));
        r.core.run_cycle_guarded();
        assert_eq!(
            r.effects.events(),
            vec!["notify:0.1.1"],
            "panic 之後下一個週期照常運作"
        );
    }

    #[test]
    fn panic_during_transition_still_releases_the_wait() {
        // 過渡期中更新任務 panic：視同失敗，仍要轉移到建立 UI，不能讓宿主卡在沒有小工具的狀態。
        let r = rig(Transition::waiting());
        *r.backend.panic_on_check.lock().unwrap() = true;
        r.core.run_cycle_guarded();
        assert_eq!(r.effects.events(), vec!["build_ui"]);
        assert_eq!(r.core.transition().phase(), Phase::UiReady);
    }

    #[test]
    fn task_runs_on_its_own_thread_and_survives_a_panicking_run() {
        // 實際走 spawn：任務在獨立執行緒、panic 被隔離，join 不得回傳 Err。
        let r = rig(Transition::ui_ready());
        *r.backend.panic_on_check.lock().unwrap() = true;
        let handle = r
            .core
            .spawn(FirstCheck::AfterNetwork {
                max_wait: Duration::from_secs(1),
            })
            .expect("啟動更新任務執行緒");
        handle.join().expect("panic 已隔離，執行緒正常結束");
    }

    // ── spec「新版啟動即當機」＋過渡期三種轉移 ───────────────────────────────────────

    #[test]
    fn crash_loop_with_new_version_installs_before_building_ui() {
        // 舊標記仍在 → 過渡期；檢查到新版 → 轉入安裝，先於建立小工具。假的 install_now 若成功，行程
        // 會結束而不返回（無法在單元測試表達），所以這裡驗證順序：install_now 在 build_ui 之前被呼叫、
        // 且期間沒有任何 UI；返回之後的處置見 install_*_retreats_* 三個測試。
        let r = rig(Transition::waiting());
        *r.effects.defer_build.lock().unwrap() = true;
        r.backend.push_check(Ok(Some("0.1.1")));
        r.core.run_cycle_guarded();
        assert_eq!(
            r.effects.events(),
            vec!["state:0.1.1", "install_now:0.1.1", "refresh_menu"],
            "先寫嘗試紀錄、再呼叫 install（呼叫時沒有 build_ui：在建立小工具之前安裝）；假的 install 回 Ok＝沒成功，退路的建立 UI 只是投遞（defer_build、尚未執行），更新丟掉並重建選單"
        );
    }

    /// I1：同步安裝返回（回 Err、panic、或回 Ok 卻沒結束行程）時，一律走退路建立 UI，phase 不得停在
    /// Installing，之後系統匣「結束」要能走一般流程結束。
    fn assert_install_return_retreats(script: InstallScript) {
        let r = rig(Transition::waiting());
        *r.install.install_result.lock().unwrap() = script;
        r.backend.push_check(Ok(Some("0.1.1")));
        r.core.run_cycle_guarded();
        assert_eq!(
            r.effects.events(),
            vec![
                "state:0.1.1",
                "install_now:0.1.1",
                "build_ui",
                "refresh_menu"
            ],
            "退路建立 UI；更新沒裝成就丟掉並重建選單（不通知），下個週期重新下載"
        );
        assert_eq!(
            r.core.transition().phase(),
            Phase::UiReady,
            "不得停在 Installing"
        );
        assert_eq!(r.core.ready_version(), None, "更新已丟掉（審查 I1）");
        assert_eq!(
            r.core.on_tray_quit(),
            QuitDisposition::Normal,
            "系統匣「結束」要能正常結束，不是被 IgnoreInstalling 吞掉"
        );
    }

    #[test]
    fn install_returning_ok_without_exiting_retreats_to_build_ui() {
        assert_install_return_retreats(InstallScript::ReturnsOk);
    }

    #[test]
    fn install_panic_retreats_to_build_ui() {
        assert_install_return_retreats(InstallScript::Panics);
    }

    #[test]
    fn install_error_retreats_to_build_ui() {
        assert_install_return_retreats(InstallScript::BeforeExit);
    }

    #[test]
    fn transition_to_install_drops_queued_handoffs() {
        let r = rig(Transition::waiting());
        assert_eq!(
            r.core
                .transition()
                .offer_handoff(vec!["fc-host.exe".into()]),
            HandoffDisposition::Queued
        );
        r.backend.push_check(Ok(Some("0.1.1")));
        r.core.run_cycle_guarded();
        assert!(
            !r.effects.events().iter().any(|e| e.starts_with("replay:")),
            "轉到安裝：排隊的交接被丟棄，不得重放"
        );
    }

    #[test]
    fn install_failure_retreat_drops_pre_install_handoffs_and_handles_later_ones_immediately() {
        let r = rig(Transition::waiting());
        *r.install.install_result.lock().unwrap() = InstallScript::BeforeExit;
        // 在安裝開始之前排隊的交接會被丟棄；退路建立 UI 後新進的交接即時處理。
        r.core
            .transition()
            .offer_handoff(vec!["fc-host.exe".into(), "--early".into()]);
        r.backend.push_check(Ok(Some("0.1.1")));
        r.core.run_cycle_guarded();
        assert_eq!(
            r.effects.events(),
            vec![
                "state:0.1.1",
                "install_now:0.1.1",
                "build_ui",
                "refresh_menu"
            ]
        );
        assert_eq!(
            r.core
                .transition()
                .offer_handoff(vec!["fc-host.exe".into()]),
            HandoffDisposition::ProcessNow
        );
    }

    /// I2：建立 UI 已決定、但投遞到主執行緒的閉包還沒跑時按系統匣「結束」——比照過渡期結束（不還原、
    /// 不存主題、刪標記），閉包發現正在結束就略過建立；重複按「結束」忽略。
    #[test]
    fn quit_between_build_ui_decision_and_execution_skips_the_build() {
        let r = rig(Transition::waiting());
        *r.effects.defer_build.lock().unwrap() = true;
        r.backend.push_check(Ok(None));
        r.core.run_cycle_guarded();
        assert_eq!(r.core.transition().phase(), Phase::BuildingUi);
        assert!(r.effects.events().is_empty(), "閉包已投遞但尚未執行");

        assert_eq!(r.core.on_tray_quit(), QuitDisposition::QuitDuringTransition);
        assert!(r.core.stop.is_cancelled(), "取消更新任務");
        assert_eq!(r.core.on_tray_quit(), QuitDisposition::AlreadyQuitting);

        r.effects.run_pending();
        assert_eq!(
            r.effects.events(),
            vec!["build_ui_skipped"],
            "已在結束：不得建立小工具與桌布協調"
        );
        assert_eq!(r.core.transition().phase(), Phase::Quit);
    }

    #[test]
    fn repeated_quit_during_transition_is_ignored() {
        let r = rig(Transition::waiting());
        assert_eq!(r.core.on_tray_quit(), QuitDisposition::QuitDuringTransition);
        assert_eq!(r.core.on_tray_quit(), QuitDisposition::AlreadyQuitting);
        assert_eq!(r.core.on_tray_quit(), QuitDisposition::AlreadyQuitting);
    }

    #[test]
    fn no_update_during_transition_builds_ui_and_replays_queue_in_order() {
        let r = rig(Transition::waiting());
        for flag in ["--a", "--b"] {
            r.core
                .transition()
                .offer_handoff(vec!["fc-host.exe".into(), flag.into()]);
        }
        r.backend.push_check(Ok(None));
        r.core.run_cycle_guarded();
        assert_eq!(
            r.effects.events(),
            vec![
                "build_ui",
                "replay:fc-host.exe --a",
                "replay:fc-host.exe --b"
            ],
            "轉到建立 UI 後依序處理排隊的交接"
        );
        assert_eq!(r.core.transition().phase(), Phase::UiReady);
    }

    #[test]
    fn check_failure_during_transition_builds_ui() {
        let r = rig(Transition::waiting());
        r.backend
            .push_check(Err(BackendError::Network("離線".into())));
        r.core.run_cycle_guarded();
        assert_eq!(r.effects.events(), vec!["build_ui"]);
    }

    #[test]
    fn user_quit_during_transition_cancels_task_and_blocks_late_results() {
        let r = rig(Transition::waiting());
        assert_eq!(r.core.on_tray_quit(), QuitDisposition::QuitDuringTransition);
        assert!(r.core.stop.is_cancelled(), "取消更新任務");
        // 晚到的結果：不得建 UI、不得安裝、不得通知。
        r.backend.push_check(Ok(Some("0.1.1")));
        r.core.run_cycle_guarded();
        assert!(
            !r.effects
                .events()
                .iter()
                .any(|e| e == "build_ui" || e.starts_with("install_now")),
            "結束後晚到的結果不得再建立 UI 或安裝：{:?}",
            r.effects.events()
        );
        // 任務迴圈也不會再啟動檢查。
        let before = *r.backend.check_calls.lock().unwrap();
        r.core.run(FirstCheck::AfterNetwork {
            max_wait: Duration::from_secs(60),
        });
        assert_eq!(*r.backend.check_calls.lock().unwrap(), before);
    }

    /// 最終審查 M2a：過渡期（「等待」）收到 `--restore-wallpaper` 交接——轉到「結束」、取消更新任務；之後晚到的結果
    /// 不建 UI、不安裝，系統匣「結束」與重複的交接都視為已在結束。
    #[test]
    fn restore_handoff_while_waiting_quits_the_transition() {
        let r = rig(Transition::waiting());
        assert!(r.core.on_restore_handoff(), "等待中：轉到結束");
        assert_eq!(r.core.transition().phase(), Phase::Quit);
        assert!(r.core.stop.is_cancelled(), "取消更新任務");
        assert!(!r.core.on_restore_handoff(), "已在結束：不重複轉移");
        assert_eq!(r.core.on_tray_quit(), QuitDisposition::AlreadyQuitting);
        r.backend.push_check(Ok(Some("0.1.1")));
        r.core.run_cycle_guarded();
        assert!(
            !r.effects
                .events()
                .iter()
                .any(|e| e == "build_ui" || e.starts_with("install_now")),
            "{:?}",
            r.effects.events()
        );
    }

    /// 「建立 UI」已決定、閉包還沒跑時收到交接：同樣轉到結束，閉包發現已在結束就略過建立。
    #[test]
    fn restore_handoff_while_building_ui_quits_and_skips_the_build() {
        let r = rig(Transition::waiting());
        *r.effects.defer_build.lock().unwrap() = true;
        r.backend.push_check(Ok(None));
        r.core.run_cycle_guarded();
        assert_eq!(r.core.transition().phase(), Phase::BuildingUi);
        assert!(r.core.on_restore_handoff());
        assert!(r.core.stop.is_cancelled());
        r.effects.run_pending();
        assert_eq!(r.effects.events(), vec!["build_ui_skipped"]);
        assert_eq!(r.core.transition().phase(), Phase::Quit);
    }

    /// 複審 M-A：交接在「建立 UI」的閉包**執行途中**（建立小工具視窗的巢狀訊息泵）被派送——轉到「結束」，建立工作
    /// 在啟動背景元件前以 `is_quitting` 察覺，不啟動協調迴圈；`finish` 不吐出任何排隊的交接。
    #[test]
    fn restore_handoff_nested_inside_the_ui_build_stops_before_the_coordinator() {
        let r = rig(Transition::waiting());
        r.core
            .transition()
            .offer_handoff(vec!["fc-host.exe".into(), "--a".into()]);
        *r.effects.nested_restore_handoff.lock().unwrap() = Some(Arc::downgrade(&r.core));
        r.backend.push_check(Ok(None));
        r.core.run_cycle_guarded();
        assert_eq!(
            r.effects.events(),
            vec![
                "widgets",
                "nested_restore_handoff:true",
                "coordinator_not_started"
            ],
            "建立途中已轉到結束：不得啟動協調迴圈，排隊的交接也不重放"
        );
        assert_eq!(r.core.transition().phase(), Phase::Quit);
        assert!(r.core.is_quitting());
        assert!(r.core.stop.is_cancelled(), "取消更新任務");
    }

    /// `is_quitting` 只在「結束」為真。
    #[test]
    fn is_quitting_is_true_only_in_the_quit_phase() {
        let r = rig(Transition::waiting());
        assert!(!r.core.is_quitting());
        assert!(r.core.transition().begin_build_ui());
        assert!(!r.core.is_quitting());
        assert!(r.core.on_restore_handoff());
        assert!(r.core.is_quitting());
        assert!(!rig(Transition::ui_ready()).core.is_quitting());
    }

    /// 過渡期以外不動：UI 已建立（交給協調迴圈）、同步安裝中（不打斷安裝）都回 `false`、phase 與任務不變。
    #[test]
    fn restore_handoff_outside_the_transition_is_left_to_the_normal_path() {
        let ready = rig(Transition::ui_ready());
        assert!(!ready.core.on_restore_handoff());
        assert_eq!(ready.core.transition().phase(), Phase::UiReady);
        assert!(!ready.core.stop.is_cancelled());

        let installing = rig(Transition::waiting());
        assert!(installing.core.transition().begin_install());
        assert!(!installing.core.on_restore_handoff());
        assert_eq!(installing.core.transition().phase(), Phase::Installing);
        assert!(!installing.core.stop.is_cancelled());
    }

    #[test]
    fn tray_quit_without_transition_is_normal() {
        let r = rig(Transition::ui_ready());
        assert_eq!(r.core.on_tray_quit(), QuitDisposition::Normal);
        assert!(!r.core.stop.is_cancelled());
    }

    // ── 逾時後下載才完成 ────────────────────────────────────────────────────────────

    #[test]
    fn download_finishing_after_timeout_follows_normal_rules() {
        let r = rig(Transition::waiting());
        // 60 秒逾時：轉到建立 UI。
        r.core.on_transition_timeout();
        assert_eq!(r.effects.events(), vec!["build_ui"]);
        // 之後下載才完成：不走同步安裝，一般規則（儲存＋通知一次）。
        r.backend.push_check(Ok(Some("0.1.1")));
        r.core.run_cycle_guarded();
        assert_eq!(
            r.effects.events(),
            vec!["build_ui", "notify:0.1.1"],
            "逾時後才下載完成：不安裝、不再建 UI，只通知"
        );
        assert_eq!(r.core.transition().phase(), Phase::UiReady);
        assert_eq!(
            r.core.take_ready().map(|d| d.version.clone()).as_deref(),
            Some("0.1.1")
        );
    }

    #[test]
    fn timeout_does_nothing_once_the_transition_has_moved() {
        let r = rig(Transition::waiting());
        assert!(r.core.transition().begin_install()); // 轉到安裝（安裝進行中）
        r.core.on_transition_timeout();
        assert!(r.effects.events().is_empty(), "已轉到安裝，逾時不得再建 UI");
        assert_eq!(r.core.transition().phase(), Phase::Installing);
    }

    #[test]
    fn watchdog_thread_fires_timeout_only_when_the_sleep_elapses() {
        // 睡滿（回報未取消）→ 觸發逾時、建立 UI。
        let r = rig(Transition::waiting());
        r.timer.script.lock().unwrap().push_back(false);
        r.core.spawn_watchdog().unwrap().join().unwrap();
        assert_eq!(
            *r.timer.requested.lock().unwrap(),
            vec![Duration::from_secs(60)]
        );
        assert_eq!(r.effects.events(), vec!["build_ui"]);
        // 提前被喚醒（轉移已發生）→ 不動作。
        let r = rig(Transition::waiting());
        r.timer.script.lock().unwrap().push_back(true);
        r.core.spawn_watchdog().unwrap().join().unwrap();
        assert!(r.effects.events().is_empty());
    }

    #[test]
    fn late_result_after_user_quit_is_discarded() {
        let r = rig(Transition::waiting());
        r.core.on_tray_quit();
        // 假設 check 在使用者結束之前就已發出，之後才回來：直接餵給處置函式。
        r.core.on_update_downloaded(Downloaded {
            version: "0.1.1".into(),
            bytes: vec![],
            handle: Box::new(()),
        });
        assert!(r.effects.events().is_empty());
        assert!(r.core.take_ready().is_none());
    }

    // ── task 3.2：安裝觸發（自動／手動／過渡期）、防循環、失敗分支 ──────────────────────

    use crate::updater::state_file::UpdateState;

    /// 假時鐘的「現在」是 1_000_000；剛嘗試過（一分鐘前）安裝 v0.1.1 的紀錄。
    fn recent_attempt(version: &str) -> UpdateState {
        UpdateState::new(version, 1_000_000 - 60)
    }

    fn fresh_ready(r: &Rig, version: &str) {
        r.backend.push_check(Ok(Some(version)));
        r.core.run_cycle_guarded();
    }

    #[test]
    fn autostart_within_ten_minutes_installs_after_recording_the_attempt() {
        // spec「使用者沒有理會」：登入後數分鐘內自動完成更新。UI 已建立（ui_built=true）、先寫紀錄再安裝；
        // 假的 install 回 Ok（外掛成功時行程已結束，無法表達）＝沒成功，之後落到「丟掉並重建選單」的保險路徑。
        let r = rig_with(Transition::ui_ready(), StartMode::Autostart);
        *r.install.uptime.lock().unwrap() = Duration::from_secs(90);
        fresh_ready(&r, "0.1.1");
        assert_eq!(
            r.effects.events(),
            vec!["state:0.1.1", "install_now:0.1.1", "refresh_menu"]
        );
        assert_eq!(*r.install.last_ui_built.lock().unwrap(), Some(true));
    }

    #[test]
    fn autostart_after_ten_minutes_only_notifies() {
        let r = rig_with(Transition::ui_ready(), StartMode::Autostart);
        *r.install.uptime.lock().unwrap() = Duration::from_secs(11 * 60);
        fresh_ready(&r, "0.1.1");
        assert_eq!(r.effects.events(), vec!["notify:0.1.1"]);
        assert!(r.install.read_state_none(), "沒安裝就不寫嘗試紀錄");
    }

    #[test]
    fn normal_launch_never_installs_on_its_own() {
        // 一般執行期間（沒有 --autostart）不自行開始安裝，即使剛啟動。
        let r = rig_with(Transition::ui_ready(), StartMode::Normal);
        *r.install.uptime.lock().unwrap() = Duration::from_secs(5);
        fresh_ready(&r, "0.1.1");
        assert_eq!(r.effects.events(), vec!["notify:0.1.1"]);
    }

    #[test]
    fn auto_install_that_did_not_take_effect_backs_off_for_24_hours() {
        // spec「自動安裝沒有生效」：上次（一分鐘前）自動安裝 v0.1.1，但重新啟動的仍是 v0.1.0。
        let r = rig_with(Transition::ui_ready(), StartMode::Autostart);
        *r.install.state.lock().unwrap() = Some(recent_attempt("0.1.1"));
        fresh_ready(&r, "0.1.1");
        assert_eq!(
            r.effects.events(),
            vec!["notify:0.1.1"],
            "只通知、不安裝（記錄檔另有一筆錯誤，見 install::log_decision）"
        );
        // 24 小時後可再試。
        let r = rig_with(Transition::ui_ready(), StartMode::Autostart);
        *r.install.state.lock().unwrap() = Some(UpdateState::new("0.1.1", 1_000_000 - 24 * 3600));
        fresh_ready(&r, "0.1.1");
        assert!(r.effects.events().contains(&"install_now:0.1.1".to_owned()));
    }

    #[test]
    fn auto_install_failing_after_on_before_exit_relaunches_and_never_builds_ui_or_notifies() {
        // ShellExecuteW 失敗／使用者取消 UAC：收尾已做完，不回頭建 UI、不通知（行程馬上結束）。
        let r = rig_with(Transition::ui_ready(), StartMode::Autostart);
        *r.install.install_result.lock().unwrap() = InstallScript::AfterExit;
        fresh_ready(&r, "0.1.1");
        assert_eq!(
            r.effects.events(),
            vec![
                "state:0.1.1",
                "install_now:0.1.1",
                "relaunch:ShellExecuteW 失敗"
            ]
        );
        assert_eq!(r.core.ready_version(), None);
    }

    #[test]
    fn auto_install_failing_before_on_before_exit_keeps_running_and_discards_the_update() {
        // 寫暫存檔失敗：on_before_exit 尚未執行，宿主照常運作、記錄；更新丟掉（不通知，通知會指向已消失的選單項），
        // 下個週期重新下載（審查 I1，見 `a_discarded_update_is_downloaded_again_and_the_menu_item_returns`）。
        let r = rig_with(Transition::ui_ready(), StartMode::Autostart);
        *r.install.install_result.lock().unwrap() = InstallScript::BeforeExit;
        fresh_ready(&r, "0.1.1");
        assert_eq!(
            r.effects.events(),
            vec!["state:0.1.1", "install_now:0.1.1", "refresh_menu"]
        );
        assert_eq!(r.core.ready_version(), None);
        assert_eq!(r.core.transition().phase(), Phase::UiReady);
        assert_eq!(r.core.on_tray_quit(), QuitDisposition::Normal);
    }

    #[test]
    fn auto_install_panic_is_contained_and_discards_the_update() {
        let r = rig_with(Transition::ui_ready(), StartMode::Autostart);
        *r.install.install_result.lock().unwrap() = InstallScript::Panics;
        fresh_ready(&r, "0.1.1");
        assert_eq!(
            r.effects.events().last().map(String::as_str),
            Some("refresh_menu")
        );
        assert_eq!(r.core.ready_version(), None);
        // 保險旗標已釋放：之後手動安裝不會被「已有安裝進行中」擋住。
        assert!(!r.core.installing.load(Ordering::SeqCst));
    }

    #[test]
    fn quit_started_blocks_the_auto_install() {
        // 系統匣「結束」已在進行（QUIT_STARTED）：不得啟動安裝；更新只通知。
        let r = rig_with(Transition::ui_ready(), StartMode::Autostart);
        *r.install.quit.lock().unwrap() = true;
        fresh_ready(&r, "0.1.1");
        assert_eq!(r.effects.events(), vec!["notify:0.1.1"]);
        assert!(r.install.read_state_none());
    }

    #[test]
    fn transition_build_defers_the_auto_install_until_the_ui_is_built() {
        // 過渡期逾時後才下載完成、剛好撞上「建立 UI 中」：不在建立中途收尾，記為延後項（先不通知）；UI 建好後在
        // 背景執行緒重新評估，仍在 10 分鐘窗內就照常自動安裝（最終審查 M2 修法 A）。
        let r = rig_with(Transition::waiting(), StartMode::Autostart);
        *r.effects.defer_build.lock().unwrap() = true;
        r.core.on_transition_timeout();
        assert_eq!(r.core.transition().phase(), Phase::BuildingUi);
        fresh_ready(&r, "0.1.1");
        assert!(r.effects.events().is_empty(), "延後：不安裝也先不通知");
        assert!(r.install.read_state_none());
        assert_eq!(r.core.ready_version(), None);
        r.effects.run_pending(); // 主執行緒建完 UI
        assert_eq!(
            r.effects.events(),
            vec!["build_ui", "spawn_background"],
            "建完只投遞到背景執行緒，不在主執行緒同步安裝"
        );
        assert!(r.install.read_state_none(), "投遞當下沒有開始安裝");
        r.effects.run_spawned(); // 背景執行緒
        assert_eq!(
            r.effects.events(),
            vec![
                "build_ui",
                "spawn_background",
                "state:0.1.1",
                "install_now:0.1.1",
                "refresh_menu"
            ]
        );
        assert_eq!(*r.install.last_ui_built.lock().unwrap(), Some(true));
    }

    #[test]
    fn no_marker_start_defers_the_auto_install_until_setup_has_built_the_ui() {
        // 最終審查 M2：沒有舊標記、`--autostart` 開機且網路很快——下載在 setup 同步建完 UI 之前完成（實機記錄如此）。
        // 建立中不安裝、不通知；建完後在背景執行緒照常自動安裝。
        let r = rig_with(Transition::building_ui(), StartMode::Autostart);
        *r.install.uptime.lock().unwrap() = Duration::from_secs(20);
        fresh_ready(&r, "0.1.1");
        assert!(r.effects.events().is_empty(), "{:?}", r.effects.events());
        assert!(r.install.read_state_none(), "建立中沒有嘗試安裝");
        r.core.finish_inline_build_ui();
        assert_eq!(r.core.transition().phase(), Phase::UiReady);
        assert_eq!(r.effects.events(), vec!["spawn_background"]);
        r.effects.run_spawned();
        assert_eq!(
            r.effects.events(),
            vec![
                "spawn_background",
                "state:0.1.1",
                "install_now:0.1.1",
                "refresh_menu"
            ]
        );
        // 背景重新評估只做一次：再呼叫不會重複安裝。
        r.core.resume_deferred_auto_install();
        assert_eq!(r.effects.events().len(), 4);
    }

    #[test]
    fn deferred_auto_install_uses_the_uptime_at_download_for_the_ten_minute_window() {
        // 複審 Minor 1：下載在啟動後 30 秒完成、UI 到 11 分鐘才建好——spec「10 分鐘內找到並下載完」已成立，照常安裝。
        let r = rig_with(Transition::building_ui(), StartMode::Autostart);
        *r.install.uptime.lock().unwrap() = Duration::from_secs(30);
        fresh_ready(&r, "0.1.1");
        *r.install.uptime.lock().unwrap() = Duration::from_secs(11 * 60);
        r.core.finish_inline_build_ui();
        r.effects.run_spawned();
        assert_eq!(
            r.effects.events(),
            vec![
                "spawn_background",
                "state:0.1.1",
                "install_now:0.1.1",
                "refresh_menu"
            ]
        );
    }

    #[test]
    fn download_after_the_window_while_building_only_notifies_without_deferring() {
        // 下載完成時已超出 10 分鐘窗：第一次評估就只通知，不記延後項（建完也不會再裝）。
        let r = rig_with(Transition::building_ui(), StartMode::Autostart);
        *r.install.uptime.lock().unwrap() = Duration::from_secs(11 * 60);
        fresh_ready(&r, "0.1.1");
        assert_eq!(r.effects.events(), vec!["notify:0.1.1"]);
        r.core.finish_inline_build_ui();
        assert_eq!(r.effects.events(), vec!["notify:0.1.1"], "沒有延後項可投遞");
        assert!(r.install.read_state_none(), "沒安裝就不寫嘗試紀錄");
        assert_eq!(r.core.ready_version().as_deref(), Some("0.1.1"));
    }

    #[test]
    fn ui_ready_just_before_recording_installs_on_the_updater_thread() {
        // 複審 Minor 2 掛鉤 A：第一次讀到 BuildingUi 之後、記下延後項之前 UI 建好（finish 看不到延後項）。記下後再讀
        // phase 已是 UiReady，自己取回、在更新執行緒上照常安裝，不投遞背景執行緒。
        let r = rig_with(Transition::building_ui(), StartMode::Autostart);
        *r.install.uptime.lock().unwrap() = Duration::from_secs(20);
        *r.core.race_hook.lock().unwrap() = Some(RaceHook::BeforeRecord);
        fresh_ready(&r, "0.1.1");
        assert_eq!(
            r.effects.events(),
            vec!["state:0.1.1", "install_now:0.1.1", "refresh_menu"],
            "安裝在呼叫端（更新執行緒）上發生，沒有 spawn_background"
        );
        assert!(r.core.take_deferred().is_none());
    }

    #[test]
    fn ui_ready_just_after_recording_hands_off_to_the_background_exactly_once() {
        // 複審 Minor 2 掛鉤 B：記下延後項之後、再讀 phase 之前 UI 建好（finish 取走並投遞）。更新執行緒回 Deferred，
        // 背景執行緒只安裝一次。
        let r = rig_with(Transition::building_ui(), StartMode::Autostart);
        *r.install.uptime.lock().unwrap() = Duration::from_secs(20);
        *r.core.race_hook.lock().unwrap() = Some(RaceHook::AfterRecord);
        r.backend.push_check(Ok(Some("0.1.1")));
        r.core.run_cycle_guarded();
        assert_eq!(r.effects.events(), vec!["spawn_background"]);
        assert!(r.install.read_state_none(), "更新執行緒沒有安裝");
        r.effects.run_spawned();
        let installs = r
            .effects
            .events()
            .iter()
            .filter(|e| e.starts_with("install_now"))
            .count();
        assert_eq!(installs, 1, "{:?}", r.effects.events());
    }

    #[test]
    fn deferred_outcome_is_returned_when_finish_takes_the_item() {
        // 掛鉤 B 的直接斷言：`try_auto_install` 回 Deferred。
        let r = rig_with(Transition::building_ui(), StartMode::Autostart);
        *r.install.uptime.lock().unwrap() = Duration::from_secs(20);
        *r.core.race_hook.lock().unwrap() = Some(RaceHook::AfterRecord);
        let downloaded = Arc::new(Downloaded {
            version: "0.1.1".into(),
            bytes: Vec::new(),
            handle: Box::new(()),
        });
        assert_eq!(
            r.core.try_auto_install(&downloaded, None),
            AutoOutcome::Deferred
        );
        r.effects.run_spawned();
        assert_eq!(
            r.effects
                .events()
                .iter()
                .filter(|e| e.starts_with("install_now"))
                .count(),
            1
        );
    }

    #[test]
    fn deferred_auto_install_is_dropped_when_the_build_turns_into_quit() {
        // 建立途中收到 `--restore-wallpaper` 交接而轉到「結束」：延後項丟棄，不安裝、不通知。
        let r = rig_with(Transition::building_ui(), StartMode::Autostart);
        *r.install.uptime.lock().unwrap() = Duration::from_secs(20);
        fresh_ready(&r, "0.1.1");
        assert!(r.core.on_restore_handoff());
        r.core.finish_inline_build_ui();
        r.effects.run_spawned();
        assert!(
            !r.effects
                .events()
                .iter()
                .any(|e| e.starts_with("install_now") || e.starts_with("notify")),
            "{:?}",
            r.effects.events()
        );
        assert!(r.install.read_state_none());
        assert_eq!(r.core.transition().phase(), Phase::Quit);
        assert!(r.core.take_deferred().is_none(), "延後項已丟棄");
        // 直接呼叫也一樣（沒有延後項可做）。
        r.core.resume_deferred_auto_install();
        assert!(r.install.read_state_none());
    }

    #[test]
    fn deferred_auto_install_falls_back_to_notify_when_no_thread_can_be_spawned() {
        let r = rig_with(Transition::building_ui(), StartMode::Autostart);
        *r.install.uptime.lock().unwrap() = Duration::from_secs(20);
        fresh_ready(&r, "0.1.1");
        *r.effects.spawn_fails.lock().unwrap() = true;
        r.core.finish_inline_build_ui();
        assert_eq!(r.effects.events(), vec!["spawn_failed", "notify:0.1.1"]);
        assert!(r.install.read_state_none(), "主執行緒上絕不同步安裝");
    }

    #[test]
    fn finishing_the_build_without_a_deferred_install_spawns_nothing() {
        let r = rig_with(Transition::building_ui(), StartMode::Autostart);
        r.core.finish_inline_build_ui();
        assert!(r.effects.events().is_empty());
    }

    #[test]
    fn no_marker_start_auto_installs_once_setup_has_built_the_ui() {
        // 最終審查 M2：setup 建完 UI 之後才下載完成（仍在 `--autostart` 的 10 分鐘窗內）→ 照常自動安裝。
        let r = rig_with(Transition::building_ui(), StartMode::Autostart);
        r.core.finish_inline_build_ui();
        *r.install.uptime.lock().unwrap() = Duration::from_secs(90);
        fresh_ready(&r, "0.1.1");
        assert_eq!(
            r.effects.events(),
            vec!["state:0.1.1", "install_now:0.1.1", "refresh_menu"]
        );
        assert_eq!(*r.install.last_ui_built.lock().unwrap(), Some(true));
    }

    #[test]
    fn no_marker_start_replays_handoffs_queued_during_the_build() {
        // 建立途中（WebView2 巢狀訊息泵）派送進來的一般交接排隊，setup 建完後依序重放；之後即時處理。
        let r = rig(Transition::building_ui());
        let args = |s: &str| vec!["fc-host.exe".to_owned(), s.to_owned()];
        assert_eq!(
            r.core.transition().offer_handoff(args("--a")),
            HandoffDisposition::Queued
        );
        r.core.finish_inline_build_ui();
        assert_eq!(r.effects.events(), vec!["replay:fc-host.exe --a"]);
        assert_eq!(
            r.core.transition().offer_handoff(args("--b")),
            HandoffDisposition::ProcessNow
        );
        // 建立途中收到 `--restore-wallpaper` 交接：轉到「結束」（協調迴圈尚未建立，比照過渡期 M2a）；建完的收尾什麼都不做。
        let r = rig(Transition::building_ui());
        assert!(r.core.on_restore_handoff());
        r.core.finish_inline_build_ui();
        assert_eq!(r.core.transition().phase(), Phase::Quit);
        assert!(r.core.stop.is_cancelled());
        assert!(r.effects.events().is_empty());
    }

    #[test]
    fn tampered_package_in_the_auto_install_window_never_installs() {
        // spec「更新檔被竄改」：驗簽失敗＝不產生 InstallReady、不執行、不寫嘗試紀錄，目前版本照常運作。
        let r = rig_with(Transition::ui_ready(), StartMode::Autostart);
        r.backend.push_check(Ok(Some("0.1.1")));
        r.backend.push_download(Err(verify_err()));
        r.core.run_cycle_guarded();
        assert!(r.effects.events().is_empty(), "{:?}", r.effects.events());
        assert_eq!(r.core.ready_version(), None);
        assert!(r.install.read_state_none());
        assert_eq!(
            r.core.install_manual(),
            ManualOutcome::Refused("沒有已下載並驗簽的更新".into()),
            "選單也沒有東西可裝"
        );
    }

    // ── 手動安裝（系統匣選單）────────────────────────────────────────────────────

    #[test]
    fn manual_install_records_the_attempt_and_runs_with_the_ui_built() {
        // spec「使用者點選更新」。一般啟動（不是 --autostart）也能手動裝；紀錄先於 install。
        let r = rig(Transition::ui_ready());
        fresh_ready(&r, "0.1.1");
        assert_eq!(r.effects.events(), vec!["notify:0.1.1"]);
        let outcome = r.core.install_manual();
        assert_eq!(
            outcome,
            ManualOutcome::Failed(InstallError::ReturnedWithoutExit),
            "假的 install 回 Ok＝違反契約（真外掛成功時行程已結束）"
        );
        assert_eq!(
            r.effects.events(),
            vec![
                "notify:0.1.1",
                "state:0.1.1",
                "install_now:0.1.1",
                "refresh_menu",
                "install_failed:0.1.1"
            ],
            "失敗：丟掉更新並重建選單，再發「更新失敗，稍後會再試」通知（審查 I1、I2）"
        );
        assert_eq!(*r.install.last_ui_built.lock().unwrap(), Some(true));
        assert_eq!(r.core.ready_version(), None);
    }

    #[test]
    fn manual_install_ignores_backoff_but_still_records() {
        // 手動不受 24 小時退避限制，且同樣寫入（失敗後下次登入才會正確退避）。
        let r = rig(Transition::ui_ready());
        *r.install.state.lock().unwrap() = Some(recent_attempt("0.1.1"));
        fresh_ready(&r, "0.1.1");
        let _ = r.core.install_manual();
        assert!(r.effects.events().contains(&"install_now:0.1.1".to_owned()));
        assert_eq!(
            r.install.read_state().unwrap().attempted_at_unix,
            1_000_000,
            "嘗試時間更新為這次手動安裝"
        );
    }

    #[test]
    fn manual_install_failing_before_on_before_exit_keeps_the_host_running() {
        let r = rig(Transition::ui_ready());
        *r.install.install_result.lock().unwrap() = InstallScript::BeforeExit;
        fresh_ready(&r, "0.1.1");
        assert_eq!(
            r.core.install_manual(),
            ManualOutcome::Failed(InstallError::FailedBeforeExit("寫暫存檔失敗".into()))
        );
        assert!(!r
            .effects
            .events()
            .iter()
            .any(|e| e.starts_with("relaunch:")));
        assert_eq!(r.core.ready_version(), None, "丟掉，等下個週期重新下載");
        // 旗標已釋放；沒有更新可裝時被拒絕（並通知原因），不是「已有安裝進行中」。
        assert_eq!(
            r.core.install_manual(),
            ManualOutcome::Refused("沒有已下載並驗簽的更新".into())
        );
    }

    #[test]
    fn manual_install_failing_after_on_before_exit_relaunches() {
        let r = rig(Transition::ui_ready());
        *r.install.install_result.lock().unwrap() = InstallScript::AfterExit;
        fresh_ready(&r, "0.1.1");
        let outcome = r.core.install_manual();
        assert_eq!(
            outcome,
            ManualOutcome::Relaunched(InstallError::FailedAfterExit("ShellExecuteW 失敗".into()))
        );
        assert_eq!(
            r.effects.events().last().map(String::as_str),
            Some("relaunch:ShellExecuteW 失敗")
        );
    }

    #[test]
    fn manual_install_panic_is_contained() {
        let r = rig(Transition::ui_ready());
        *r.install.install_result.lock().unwrap() = InstallScript::Panics;
        fresh_ready(&r, "0.1.1");
        assert!(matches!(
            r.core.install_manual(),
            ManualOutcome::Failed(InstallError::FailedBeforeExit(_))
        ));
        assert!(!r.core.installing.load(Ordering::SeqCst));
    }

    #[test]
    fn manual_install_is_refused_when_quit_is_in_progress() {
        // 系統匣「結束」已在進行（QUIT_STARTED）或已在因更新結束：不得啟動安裝、不寫紀錄。
        let r = rig(Transition::ui_ready());
        fresh_ready(&r, "0.1.1");
        *r.install.quit.lock().unwrap() = true;
        assert!(matches!(
            r.core.install_manual(),
            ManualOutcome::Failed(InstallError::QuitInProgress)
        ));
        assert_eq!(
            r.effects.events(),
            vec!["notify:0.1.1"],
            "正在結束：不丟更新、不重建選單、不發失敗通知"
        );
        assert_eq!(r.core.ready_version().as_deref(), Some("0.1.1"));
        assert!(r.install.read_state_none());
    }

    #[test]
    fn manual_install_is_refused_until_the_ui_is_ready() {
        let r = rig(Transition::waiting());
        *r.effects.defer_build.lock().unwrap() = true;
        r.core.on_transition_timeout(); // BuildingUi（閉包已投遞、尚未執行）
        fresh_ready(&r, "0.1.1");
        assert!(matches!(r.core.install_manual(), ManualOutcome::Refused(_)));
        assert!(!r
            .effects
            .events()
            .iter()
            .any(|e| e.starts_with("install_now")));
        r.effects.run_pending();
        assert!(matches!(r.core.install_manual(), ManualOutcome::Failed(_)));
    }

    #[test]
    fn only_one_install_may_run_at_a_time() {
        let r = rig(Transition::ui_ready());
        fresh_ready(&r, "0.1.1");
        let guard = InstallGuard::acquire(&r.core.installing).expect("第一個取得");
        assert!(InstallGuard::acquire(&r.core.installing).is_none());
        assert_eq!(
            r.core.install_manual(),
            ManualOutcome::Refused("已有安裝進行中".into())
        );
        // 自動安裝也被擋，改為只通知。
        let ra = rig_with(Transition::ui_ready(), StartMode::Autostart);
        let _held = InstallGuard::acquire(&ra.core.installing).unwrap();
        fresh_ready(&ra, "0.1.1");
        assert_eq!(ra.effects.events(), vec!["notify:0.1.1"]);
        drop(guard);
        assert!(InstallGuard::acquire(&r.core.installing).is_some());
    }

    // ── 過渡期同步安裝：退避與兩種失敗點 ────────────────────────────────────────

    #[test]
    fn crash_loop_install_backs_off_too_and_still_builds_the_ui() {
        // D3：「當機迴圈保護的同步安裝同樣受此限」。退避＝只通知＋照常建立小工具（不卡在過渡期）。
        let r = rig(Transition::waiting());
        *r.install.state.lock().unwrap() = Some(recent_attempt("0.1.1"));
        fresh_ready(&r, "0.1.1");
        assert_eq!(r.effects.events(), vec!["notify:0.1.1", "build_ui"]);
        assert_eq!(r.core.transition().phase(), Phase::UiReady);
    }

    #[test]
    fn crash_loop_install_failing_after_on_before_exit_does_not_build_ui() {
        // 沒有 UI 的同步安裝在 on_before_exit 之後失敗：走重新啟動退路，不回頭建 UI（3.1 複審 N1）。
        let r = rig(Transition::waiting());
        *r.install.install_result.lock().unwrap() = InstallScript::AfterExit;
        fresh_ready(&r, "0.1.1");
        assert_eq!(
            r.effects.events(),
            vec![
                "state:0.1.1",
                "install_now:0.1.1",
                "relaunch:ShellExecuteW 失敗"
            ]
        );
        assert_eq!(*r.install.last_ui_built.lock().unwrap(), Some(false));
        assert_eq!(
            r.core.transition().phase(),
            Phase::Installing,
            "行程即將結束；不建 UI、不退路"
        );
    }

    #[test]
    fn crash_loop_install_not_started_retreats_to_build_ui() {
        // 系統匣「結束」進行中／寫不進防循環紀錄：沒有開始安裝，走退路建立 UI。
        for setup in 0..2 {
            let r = rig(Transition::waiting());
            if setup == 0 {
                *r.install.quit.lock().unwrap() = true;
            } else {
                *r.install.write_fails.lock().unwrap() = true;
            }
            *r.install.install_result.lock().unwrap() = InstallScript::AfterExit;
            fresh_ready(&r, "0.1.1");
            let expected = if setup == 0 {
                // 結束進行中：沒開始、不丟更新（照一般規則保留並通知）。
                vec!["build_ui", "notify:0.1.1"]
            } else {
                // 寫不進防循環紀錄：沒開始，但更新丟掉、下個週期重新下載。
                vec!["build_ui", "refresh_menu"]
            };
            assert_eq!(r.effects.events(), expected, "setup={setup}");
        }
    }

    // ── 規則：不得提前設「因更新結束」旗標 ───────────────────────────────────────

    #[test]
    fn install_wiring_never_sets_the_exiting_flag_itself() {
        // 旗標只由 `prepare_exit_for_update`（on_before_exit 之內）設；3.2 在呼叫 `install()` 之前自己設，
        // 寫暫存檔失敗後宿主照常運作時，旗標不會復原、系統匣「結束」與視窗同步會永久失效。
        let call = concat!("begin_exiting_for_update", "(");
        for (name, src) in [
            ("engine.rs", include_str!("engine.rs")),
            ("install.rs", include_str!("install.rs")),
            ("plugin_backend.rs", include_str!("plugin_backend.rs")),
            ("../updater.rs", include_str!("../updater.rs")),
            ("../tray.rs", include_str!("../tray.rs")),
        ] {
            // 本測試檔自己的文字也含這串字：只看「呼叫」位置前面不是 `fn ` 或 `concat!` 的行。
            let offending = src.lines().filter(|l| {
                l.contains(call)
                    && !l.contains("concat!")
                    && !l.trim_start().starts_with("//")
                    && !l.contains("fn ")
            });
            assert_eq!(
                offending.count(),
                0,
                "{name} 不得呼叫 begin_exiting_for_update"
            );
        }
    }

    // ── 審查第 1 輪：I1 丟掉更新並重新下載、I2／M5 使用者回饋、M4 退避只擋同一版本 ─────────

    fn event_count(r: &Rig, line: &str) -> usize {
        r.effects.events().iter().filter(|e| *e == line).count()
    }

    #[test]
    fn a_discarded_update_is_downloaded_again_and_the_menu_item_returns() {
        // I1：on_before_exit 之前失敗 → 丟掉；下個週期（同一個候選版本）重新下載，選單項回來，不重複通知。
        for kind in ["auto", "manual", "transition"] {
            let r = match kind {
                "auto" => rig_with(Transition::ui_ready(), StartMode::Autostart),
                "manual" => rig(Transition::ui_ready()),
                _ => rig(Transition::waiting()),
            };
            *r.install.install_result.lock().unwrap() = InstallScript::BeforeExit;
            fresh_ready(&r, "0.1.1");
            if kind == "manual" {
                assert_eq!(r.core.ready_version().as_deref(), Some("0.1.1"));
                assert!(matches!(r.core.install_manual(), ManualOutcome::Failed(_)));
            }
            assert_eq!(r.core.ready_version(), None, "{kind}：丟掉");
            assert!(
                event_count(&r, "refresh_menu") >= 1,
                "{kind}：選單項跟著消失：{:?}",
                r.effects.events()
            );
            assert_eq!(r.backend.download_calls.lock().unwrap().len(), 1, "{kind}");

            // 下個週期：因為沒有持有，會再下載一次。這次不再讓安裝發生（非 autostart 窗口內／UI 已就緒的一般規則）。
            *r.install.uptime.lock().unwrap() = Duration::from_secs(3600);
            let before_refresh = event_count(&r, "refresh_menu");
            fresh_ready(&r, "0.1.1");
            assert_eq!(
                r.backend.download_calls.lock().unwrap().len(),
                2,
                "{kind}：重跑週期會再下載"
            );
            assert_eq!(r.core.ready_version().as_deref(), Some("0.1.1"), "{kind}");
            if kind != "manual" {
                // 自動／過渡期安裝失敗時沒通知過：重新下載時才第一次通知（notify_ready 本身會重建選單）。
                assert_eq!(event_count(&r, "notify:0.1.1"), 1, "{kind}");
            } else {
                assert_eq!(
                    event_count(&r, "refresh_menu"),
                    before_refresh + 1,
                    "{kind}：重新下載到已通知過的同一版本：重建選單讓選單項回來"
                );
            }
            assert!(
                event_count(&r, "notify:0.1.1") <= 1,
                "{kind}：同一版本最多通知一次（auto 失敗時沒通知過，重新下載時才第一次通知；其餘本來就通知過）"
            );
        }
    }

    #[test]
    fn a_failed_auto_install_notifies_only_once_the_update_is_downloaded_again() {
        // 自動安裝失敗當下不通知（通知會叫使用者去選單，但選單項已消失）；重新下載後（超過 10 分鐘窗口）才通知一次。
        let r = rig_with(Transition::ui_ready(), StartMode::Autostart);
        *r.install.install_result.lock().unwrap() = InstallScript::BeforeExit;
        fresh_ready(&r, "0.1.1");
        assert_eq!(event_count(&r, "notify:0.1.1"), 0);
        *r.install.uptime.lock().unwrap() = Duration::from_secs(11 * 60);
        fresh_ready(&r, "0.1.1");
        assert_eq!(event_count(&r, "notify:0.1.1"), 1);
        fresh_ready(&r, "0.1.1"); // 已持有：略過，不重複下載也不重複通知
        assert_eq!(event_count(&r, "notify:0.1.1"), 1);
        assert_eq!(r.backend.download_calls.lock().unwrap().len(), 2);
    }

    #[test]
    fn after_exit_failure_and_quit_in_progress_do_not_discard_the_update() {
        // 不丟的情況：on_before_exit 之後才失敗（行程即將結束、已重新啟動自己）、結束進行中沒開始。
        let r = rig(Transition::ui_ready());
        *r.install.install_result.lock().unwrap() = InstallScript::AfterExit;
        fresh_ready(&r, "0.1.1");
        let menu_events = event_count(&r, "refresh_menu");
        assert!(matches!(
            r.core.install_manual(),
            ManualOutcome::Relaunched(_)
        ));
        assert_eq!(r.core.ready_version().as_deref(), Some("0.1.1"));
        assert_eq!(event_count(&r, "refresh_menu"), menu_events);
        assert_eq!(
            event_count(&r, "install_failed:0.1.1"),
            0,
            "已重新啟動：不發失敗通知"
        );

        let r = rig(Transition::ui_ready());
        fresh_ready(&r, "0.1.1");
        *r.install.quit.lock().unwrap() = true;
        let _ = r.core.install_manual();
        assert_eq!(r.core.ready_version().as_deref(), Some("0.1.1"));
        assert_eq!(event_count(&r, "refresh_menu"), 0);
        assert_eq!(
            event_count(&r, "install_failed:0.1.1"),
            0,
            "結束進行中：不發失敗通知"
        );
    }

    #[test]
    fn manual_install_failure_notifies_for_before_exit_failures_and_panics_only() {
        // I2：on_before_exit 之前失敗與 panic 發「更新失敗」通知；AfterExit、結束進行中不發（見上一個測試）。
        for script in [
            InstallScript::BeforeExit,
            InstallScript::Panics,
            InstallScript::ReturnsOk,
        ] {
            let r = rig(Transition::ui_ready());
            *r.install.install_result.lock().unwrap() = script;
            fresh_ready(&r, "0.1.1");
            let _ = r.core.install_manual();
            assert_eq!(event_count(&r, "install_failed:0.1.1"), 1);
        }
        // 通知文字（Warning、帶版本、說明稍後會再試）。
        let n = install::install_failed_notification("0.1.1");
        assert_eq!(n.kind, crate::desktop::tray_balloon::BalloonKind::Warning);
        assert!(n.body.contains("0.1.1") && n.body.contains("稍後會再試"));
    }

    #[test]
    fn a_refused_click_tells_the_user_why() {
        // M5：UI 尚未就緒／已有安裝進行中／沒有可裝的更新。
        let r = rig(Transition::waiting());
        *r.effects.defer_build.lock().unwrap() = true;
        r.core.on_transition_timeout();
        fresh_ready(&r, "0.1.1");
        assert!(matches!(r.core.install_manual(), ManualOutcome::Refused(_)));
        assert_eq!(
            event_count(&r, "install_refused:小工具尚未就緒或正在結束"),
            1
        );

        let r = rig(Transition::ui_ready());
        fresh_ready(&r, "0.1.1");
        let _held = InstallGuard::acquire(&r.core.installing).unwrap();
        assert!(matches!(r.core.install_manual(), ManualOutcome::Refused(_)));
        assert_eq!(event_count(&r, "install_refused:已有安裝進行中"), 1);

        let r = rig(Transition::ui_ready());
        assert!(matches!(r.core.install_manual(), ManualOutcome::Refused(_)));
        assert_eq!(event_count(&r, "install_refused:沒有已下載並驗簽的更新"), 1);
        assert_eq!(event_count(&r, "refresh_menu"), 1, "過期的選單項一併重建");
        let n = install::install_refused_notification("已有安裝進行中");
        assert!(n.body.contains("已有安裝進行中") && n.body.contains("請稍後再試"));
    }

    #[test]
    fn backoff_only_blocks_the_same_version_a_newer_one_installs_as_usual() {
        // M4：上次嘗試 0.1.1（一分鐘前、目前版本 0.1.0）。同版本只通知；更新的 0.1.2 照常自動安裝。
        let r = rig_with(Transition::ui_ready(), StartMode::Autostart);
        *r.install.state.lock().unwrap() = Some(recent_attempt("0.1.1"));
        fresh_ready(&r, "0.1.2");
        assert!(
            r.effects.events().contains(&"install_now:0.1.2".to_owned()),
            "更新的版本不受 0.1.1 的退避影響：{:?}",
            r.effects.events()
        );
        // 過渡期同樣只擋同一版本。
        let r = rig(Transition::waiting());
        *r.install.state.lock().unwrap() = Some(recent_attempt("0.1.1"));
        fresh_ready(&r, "0.1.2");
        assert!(r.effects.events().contains(&"install_now:0.1.2".to_owned()));
    }
}
