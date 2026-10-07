//! 正式後端：把 `tauri-plugin-updater` 包成 [`UpdateBackend`]（檢查、下載）與 [`InstallSystem`]（安裝）。
//!
//! 這一層不在單元測試範圍（要真的連網與驗簽、真的 `Update`／`AppHandle`）；核心邏輯的測試走假後端，這裡只做：
//! 呼叫外掛、把外掛的非同步 API 在更新任務自己的執行緒上同步化（`block_on`）、把外掛的錯誤歸類、
//! 把外掛 `install()` 的失敗依「`on_before_exit` 是否已跑完」分成兩種（[`InstallProbe`]）。
//! 例外：[`run_before_exit_hook`] 與 [`InstallProbe`] 是純邏輯，有單元測試。
//!
//! ## 逾時（design.md D3 的更正）
//!
//! D3 寫「`UpdaterBuilder::timeout` 設 10 分鐘」，但外掛 `updater.rs` 的 `check()` 產生 `Update` 時把
//! `timeout` 設成 `None`——`UpdaterBuilder::timeout` 只作用於 `latest.json` 的請求，**下載不會繼承**。
//! 所以這裡兩處分開設：清單請求 60 秒（`UpdaterBuilder::timeout`）；下載 10 分鐘（`Update::timeout`，
//! 公開欄位，`reqwest` 的整體逾時含 body）。再加一層 `tokio::time::timeout` 當保險。
//!
//! ## 安裝（task 3.2；design.md D3、D4）
//!
//! `UpdaterBuilder` 在 `check()` 時設定三件事（都綁進產生的 `Update`）：
//! `restart_after_install(false)`＋`installer_args(["/R", "/ARGS", "--autostart"])`（重新啟動一律帶 `--autostart`，
//! 引用見 [`INSTALLER_ARGS`]），以及 `on_before_exit`（[`run_before_exit_hook`]，裡面呼叫
//! `exit::prepare_exit_for_update`）。`Update::install` 在呼叫端的**背景執行緒**執行（`UpdaterCore` 的更新任務
//! 執行緒或選單點選時另開的執行緒），不在主執行緒。

use std::io;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tauri::AppHandle;
use tauri_plugin_updater::{Error as PluginError, Update, UpdaterExt};

use super::cancel::CancelToken;
use super::engine::{BackendError, Candidate, Downloaded, UpdateBackend};
use super::exit::{self, ExitOwner, ExitOwnerKind};
use super::install::{InstallFailure, InstallSystem, INSTALLER_ARGS};
use super::schedule::{CHECK_REQUEST_TIMEOUT, DOWNLOAD_TIMEOUT};
use super::state_file::{self, UpdateState};
use super::LOG_TARGET;
use crate::fetch::net;

/// 外層保險逾時比內層多給一點，讓內層 `reqwest` 逾時先觸發、錯誤訊息比較清楚。
const OUTER_GRACE: Duration = Duration::from_secs(15);

pub struct PluginBackend {
    app: AppHandle,
    probe: Arc<InstallProbe>,
}

impl PluginBackend {
    pub fn new(app: AppHandle, probe: Arc<InstallProbe>) -> Self {
        Self { app, probe }
    }
}

// ── 安裝：`on_before_exit` 掛鉤與失敗點判別 ────────────────────────────────────────────

/// 外掛 `install()` 與我們的 `on_before_exit` 掛鉤之間的共享狀態（task 3.2）。
///
/// 外掛 `install()` 的順序是 `extract`（寫暫存檔）→ `on_before_exit` → `ShellExecuteW` → `exit(0)`
/// （`tauri-plugin-updater-2.13.1/src/updater.rs:938-979`）。`install()` 回 `Err` 時只有「掛鉤是否已跑完」
/// 能分辨兩種完全不同的處置（見 [`InstallFailure`]），所以掛鉤跑完後記一筆旗標；`ui_built` 則是安裝發起前
/// 由呼叫端告訴掛鉤（掛鉤在 `UpdaterBuilder` 建立時就綁死，當下還不知道這次安裝有沒有 UI）。
#[derive(Default)]
pub struct InstallProbe {
    ui_built: AtomicBool,
    before_exit_done: AtomicBool,
}

impl InstallProbe {
    /// 一次安裝開始前：記下有沒有 UI、清掉上一次的「掛鉤已跑完」。
    pub fn begin(&self, ui_built: bool) {
        self.ui_built.store(ui_built, Ordering::SeqCst);
        self.before_exit_done.store(false, Ordering::SeqCst);
    }

    pub fn ui_built(&self) -> bool {
        self.ui_built.load(Ordering::SeqCst)
    }

    pub fn mark_before_exit_done(&self) {
        self.before_exit_done.store(true, Ordering::SeqCst);
    }

    /// 把 `install()` 的錯誤依「收尾是否已開始」歸類：掛鉤已跑完，**或**收尾已經開始（`exiting_started`＝
    /// `exit::is_exiting_for_update()`，`prepare_exit_for_update` 一開始就設）都算 `AfterExit`。
    /// 後者涵蓋收尾期間 panic（`unregister_restart`、`summarize` 等在掛鉤執行緒上直接跑、不在 `catch_unwind`
    /// 裡）：旗標已永久為真、部分收尾可能已做，不能當成「宿主照常運作」，要走重新啟動、不建 UI（審查 M2）。
    /// 掛鉤中止（搶不到結束擁有權、`OnMainThread`）時旗標未設，仍是 `BeforeExit`。
    pub fn failure(&self, message: String, exiting_started: bool) -> InstallFailure {
        if exiting_started || self.before_exit_done.load(Ordering::SeqCst) {
            InstallFailure::AfterExit(message)
        } else {
            InstallFailure::BeforeExit(message)
        }
    }
}

/// `on_before_exit` 掛鉤本體（`Fn()`，沒有回傳值可以表達「中止」，所以中止一律以 `resume_unwind` 展開
/// 離開外掛的 `install()`，由 [`PluginInstaller::install`] 的 `catch_unwind` 接住；展開發生在掛鉤**內**、
/// `ShellExecuteW` 之前，所以外掛尚未啟動安裝檔）：
///
/// - 系統匣「結束」已在進行／已搶到結束擁有權／已在因更新結束：中止（競態收口——安裝發起時檢查過一次，但
///   解壓縮要一段時間，這段時間使用者可能按了「結束」；兩邊各自收尾會互相破壞）。
/// - 以 [`ExitOwner`] 的 CAS 搶 `Update`（審查 M1）：搶輸（系統匣「結束」剛好搶先）就中止。反方向（掛鉤先搶、
///   `tray::quit` 後到）由 `tray::quit` 搶 `Tray` 失敗而忽略這次「結束」。所以不論先後，桌布都不會在更新時被還原。
/// - `prepare` 回 `Err`（`ExitError::OnMainThread` 等）：什麼都沒收尾、旗標未設；**歸還**結束擁有權（否則系統匣
///   「結束」會被永久擋住）後中止。
/// - `prepare` 成功：記下「掛鉤已跑完」，之後的失敗一律是 `AfterExit`。`prepare` panic 時不歸還（旗標已設，見
///   [`InstallProbe::failure`]），行程會重新啟動自己後結束。
///
/// 中止視同 `BeforeExit` 失敗（掛鉤旗標沒記），宿主照常運作。`resume_unwind` 不經 panic hook（不會在記錄檔
/// 留 backtrace 噪音），原因由這裡自己記錄。
pub fn run_before_exit_hook(
    probe: &InstallProbe,
    owner: &ExitOwner,
    quit_in_progress: impl FnOnce() -> bool,
    prepare: impl FnOnce(bool) -> Result<(), String>,
) {
    if quit_in_progress() {
        abort_install("系統匣結束或更新結束已在進行，中止安裝");
    }
    if !owner.try_claim(ExitOwnerKind::Update) {
        abort_install("系統匣「結束」已搶先取得結束擁有權，中止安裝");
    }
    match prepare(probe.ui_built()) {
        Ok(()) => probe.mark_before_exit_done(),
        Err(e) => {
            owner.release(ExitOwnerKind::Update);
            abort_install(&format!("更新前收尾無法執行（{e}），中止安裝"))
        }
    }
}

// 掛鉤中止（`abort_install`）靠 `resume_unwind` 展開離開外掛的 `install()`，由 `PluginInstaller::install` 的
// `catch_unwind` 接住；`panic = "abort"` 下展開不存在，中止會變成整個行程 abort（審查 M3）。日後有人改 profile
// 時在編譯期就失敗，不要等到執行期。
#[cfg(panic = "abort")]
compile_error!(
    "更新器的 on_before_exit 中止依賴 panic=unwind（resume_unwind）；不得以 panic=\"abort\" 建置"
);

fn abort_install(why: &str) -> ! {
    log::error!(target: LOG_TARGET, "on_before_exit：{why}");
    resume_unwind(Box::new(why.to_owned()))
}

/// 系統匣「結束」已搶到結束擁有權（`quit` 一開始就搶，早於 `QUIT_STARTED`）或已在進行（`tray::quit_started`，
/// 等桌布還原中），或已經在因更新結束。
fn quit_in_progress_now() -> bool {
    exit::tray_owns_exit() || crate::tray::quit_started() || exit::is_exiting_for_update()
}

/// 正式的 [`InstallSystem`]：外掛 `install()`、`%LOCALAPPDATA%\tw.fintools.fc-host\update-state.json`、
/// 重新啟動退路。不在單元測試範圍（要真的 `Update` 與 `AppHandle`）；每個分支的邏輯在
/// [`super::install`] 與 [`run_before_exit_hook`]／[`InstallProbe`] 的測試裡以假物件涵蓋。
pub struct PluginInstaller {
    probe: Arc<InstallProbe>,
    data_dir: PathBuf,
    started: Instant,
}

impl PluginInstaller {
    pub fn new(probe: Arc<InstallProbe>, data_dir: PathBuf) -> Self {
        Self {
            probe,
            data_dir,
            started: Instant::now(),
        }
    }
}

impl InstallSystem for PluginInstaller {
    fn now_unix(&self) -> i64 {
        super::unix_now()
    }

    fn uptime(&self) -> Duration {
        self.started.elapsed()
    }

    fn quit_in_progress(&self) -> bool {
        quit_in_progress_now()
    }

    fn read_state(&self) -> Option<UpdateState> {
        state_file::read(&self.data_dir)
    }

    fn write_state(&self, state: &UpdateState) -> io::Result<()> {
        state_file::write(&self.data_dir, state)
    }

    fn install(&self, update: &Downloaded, ui_built: bool) -> Result<(), InstallFailure> {
        // `prepare_exit_for_update` 要經 `run_on_main_thread` 關視窗，在主執行緒會死鎖（D4）；呼叫端本應是背景
        // 執行緒，這裡再擋一次（還沒有任何收尾，屬於 BeforeExit）。
        if exit::current_is_main_thread() {
            return Err(InstallFailure::BeforeExit(
                "不得在主執行緒呼叫 install()".to_owned(),
            ));
        }
        let Some(plugin_update) = update.handle.downcast_ref::<Update>() else {
            return Err(InstallFailure::BeforeExit(
                "更新物件型別不符（內部錯誤）".to_owned(),
            ));
        };
        self.probe.begin(ui_built);
        let result = catch_unwind(AssertUnwindSafe(|| plugin_update.install(&update.bytes)));
        let message = match result {
            // 外掛成功時在 `install()` 內 `exit(0)`，不會回來。
            Ok(Ok(())) => return Ok(()),
            Ok(Err(e)) => e.to_string(),
            Err(payload) => super::engine::panic_message(payload.as_ref()),
        };
        Err(self.probe.failure(message, exit::is_exiting_for_update()))
    }

    fn after_exit_failure(&self, error: &str) {
        exit::relaunch_after_failed_install(error)
    }
}

impl UpdateBackend for PluginBackend {
    fn wait_network(&self, max_wait: Duration, cancel: &CancelToken) -> bool {
        // 與 `fetch::net::wait_for_network` 同一套探測與間隔；被取消時視為「就緒」好讓呼叫端立刻往下
        // 檢查 `cancel`（語意同 `wait_for_network_unless_stopped`）。
        net::wait_for_network_with(
            || {
                if cancel.is_cancelled() {
                    Ok(())
                } else {
                    net::tcp_probe(net::TIMEOUT_EACH)
                }
            },
            |interval| {
                cancel.wait(interval);
            },
            net::INTERVAL,
            max_wait,
        )
    }

    fn check(&self) -> Result<Option<Candidate>, BackendError> {
        let probe = Arc::clone(&self.probe);
        let hook_app = self.app.clone();
        let updater = self
            .app
            .updater_builder()
            .timeout(CHECK_REQUEST_TIMEOUT)
            // 重新啟動參數（design.md D3）：外掛不自己加 `/R /ARGS <目前行程參數>`，改由 `INSTALLER_ARGS`
            // 帶 `/R /ARGS --autostart`（引用見該常數的文件）。
            .restart_after_install(false)
            .installer_args(INSTALLER_ARGS)
            .on_before_exit(move || {
                run_before_exit_hook(
                    &probe,
                    &exit::EXIT_OWNER,
                    quit_in_progress_now,
                    |ui_built| {
                        exit::prepare_exit_for_update(&hook_app, ui_built)
                            .map(|_| ())
                            .map_err(|e| e.to_string())
                    },
                )
            })
            .build()
            .map_err(classify)?;
        let checked = tauri::async_runtime::block_on(async {
            tokio::time::timeout(CHECK_REQUEST_TIMEOUT + OUTER_GRACE, updater.check()).await
        });
        match checked {
            Err(_) => Err(BackendError::Network("檢查更新逾時".into())),
            Ok(Err(e)) => Err(classify(e)),
            Ok(Ok(None)) => Ok(None),
            Ok(Ok(Some(update))) => Ok(Some(Candidate {
                version: update.version.clone(),
                handle: Box::new(update),
            })),
        }
    }

    fn download(&self, candidate: Candidate) -> Result<Downloaded, BackendError> {
        let mut update = *candidate
            .handle
            .downcast::<Update>()
            .map_err(|_| BackendError::Other("更新物件型別不符（內部錯誤）".into()))?;
        // 下載逾時要設在 `Update` 上（見模組文件）。
        update.timeout = Some(DOWNLOAD_TIMEOUT);
        let downloaded = tauri::async_runtime::block_on(async {
            tokio::time::timeout(
                DOWNLOAD_TIMEOUT + OUTER_GRACE,
                update.download(|_, _| {}, || {}),
            )
            .await
        });
        match downloaded {
            Err(_) => Err(BackendError::Network("下載更新逾時".into())),
            // `Update::download` 在回傳位元組之前已完成簽章與版本驗證（`verify_signature`）。
            Ok(Err(e)) => Err(classify(e)),
            Ok(Ok(bytes)) => Ok(Downloaded {
                version: update.version.clone(),
                bytes,
                handle: Box::new(update),
            }),
        }
    }
}

/// 外掛錯誤歸類：簽章／版本驗證失敗要單獨標出來（該檔案不得執行）。
fn classify(err: PluginError) -> BackendError {
    match err {
        PluginError::Minisign(_)
        | PluginError::Base64(_)
        | PluginError::SignatureUtf8(_)
        | PluginError::SignedVersionMismatch { .. }
        | PluginError::MissingSignedVersion => BackendError::Verify(err.to_string()),
        PluginError::Reqwest(_) | PluginError::Network(_) | PluginError::Io(_) => {
            BackendError::Network(err.to_string())
        }
        other => BackendError::Other(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn abort_message(f: impl FnOnce()) -> Option<String> {
        catch_unwind(AssertUnwindSafe(f))
            .err()
            .map(|p| super::super::engine::panic_message(p.as_ref()))
    }

    #[test]
    fn failure_is_classified_by_whether_the_hook_finished() {
        let probe = InstallProbe::default();
        probe.begin(true);
        assert_eq!(
            probe.failure("寫暫存檔失敗".into(), false),
            InstallFailure::BeforeExit("寫暫存檔失敗".into()),
            "掛鉤還沒跑完＝宿主原狀"
        );
        probe.mark_before_exit_done();
        assert_eq!(
            probe.failure("ShellExecuteW 失敗".into(), true),
            InstallFailure::AfterExit("ShellExecuteW 失敗".into()),
            "掛鉤跑完後才失敗＝要重新啟動，不得回頭建 UI"
        );
        // 下一次安裝開始時旗標清掉（上一次失敗的殘留不得影響這一次）。
        probe.begin(false);
        assert!(!probe.ui_built());
        assert!(matches!(
            probe.failure("x".into(), false),
            InstallFailure::BeforeExit(_)
        ));
    }

    #[test]
    fn a_panic_during_the_exit_steps_is_after_exit_even_though_the_hook_never_finished() {
        // 審查 M2：`prepare_exit_for_update` 設了旗標之後才 panic（例如 unregister_restart 在掛鉤執行緒直接跑），
        // 掛鉤沒有走到「記下跑完」；旗標已永久為真、部分收尾可能已做，不能當成「宿主照常運作」。
        let owner = ExitOwner::new();
        let probe = InstallProbe::default();
        probe.begin(true);
        let msg = abort_message(|| {
            run_before_exit_hook(&probe, &owner, || false, |_| panic!("收尾期間的 panic"))
        });
        assert!(
            msg.is_some_and(|m| m.contains("收尾期間")),
            "panic 原樣展開離開 install()"
        );
        // 此時 `is_exiting_for_update()` 為真（prepare 一開始就設）：歸類為 AfterExit → 重新啟動、不建 UI。
        assert!(matches!(
            probe.failure("panic".into(), true),
            InstallFailure::AfterExit(_)
        ));
        // 反例：旗標沒設（掛鉤中止、OnMainThread）仍是 BeforeExit。
        assert!(matches!(
            probe.failure("abort".into(), false),
            InstallFailure::BeforeExit(_)
        ));
        assert!(
            owner.is_owned_by(ExitOwnerKind::Update),
            "prepare panic 時不歸還擁有權（行程會重新啟動自己後結束）"
        );
    }

    #[test]
    fn hook_runs_the_exit_steps_with_the_ui_flag_and_records_completion() {
        for ui_built in [true, false] {
            let owner = ExitOwner::new();
            let probe = InstallProbe::default();
            probe.begin(ui_built);
            let seen = Cell::new(None);
            run_before_exit_hook(
                &probe,
                &owner,
                || false,
                |ui| {
                    seen.set(Some(ui));
                    Ok(())
                },
            );
            assert_eq!(
                seen.get(),
                Some(ui_built),
                "未建 UI 的同步安裝要傳 false（略過 3、6 步，步驟 5 只移除系統匣圖示）"
            );
            assert!(matches!(
                probe.failure("x".into(), false),
                InstallFailure::AfterExit(_)
            ));
            assert!(owner.is_owned_by(ExitOwnerKind::Update));
        }
    }

    #[test]
    fn hook_aborts_without_running_the_exit_steps_when_quit_is_in_progress() {
        // 競態收口：解壓縮期間使用者按了系統匣「結束」→ 不收尾、不啟動安裝檔，宿主交給結束流程。
        let owner = ExitOwner::new();
        let probe = InstallProbe::default();
        probe.begin(true);
        let ran = Cell::new(false);
        let msg = abort_message(|| {
            run_before_exit_hook(
                &probe,
                &owner,
                || true,
                |_| {
                    ran.set(true);
                    Ok(())
                },
            )
        });
        assert!(
            msg.is_some_and(|m| m.contains("中止")),
            "展開離開 install()"
        );
        assert!(!ran.get(), "不得呼叫 prepare_exit_for_update");
        assert!(
            matches!(
                probe.failure("x".into(), false),
                InstallFailure::BeforeExit(_)
            ),
            "中止視同 on_before_exit 之前失敗"
        );
    }

    #[test]
    fn hook_aborts_when_the_tray_quit_claimed_the_exit_first() {
        // 審查 M1，方向一：`tray::quit` 已搶到結束擁有權（但 `QUIT_STARTED` 還沒設、`quit_in_progress` 為假）。
        // 掛鉤搶不到 → 中止，不得收尾（否則桌布被還原的同時又因更新結束）。
        let owner = ExitOwner::new();
        assert!(owner.try_claim(ExitOwnerKind::Tray));
        let probe = InstallProbe::default();
        probe.begin(true);
        let ran = Cell::new(false);
        let msg = abort_message(|| {
            run_before_exit_hook(
                &probe,
                &owner,
                || false,
                |_| {
                    ran.set(true);
                    Ok(())
                },
            )
        });
        assert!(msg.is_some_and(|m| m.contains("搶先")));
        assert!(!ran.get());
        assert!(owner.is_owned_by(ExitOwnerKind::Tray), "擁有權不被奪走");
        assert!(matches!(
            probe.failure("x".into(), false),
            InstallFailure::BeforeExit(_)
        ));
    }

    #[test]
    fn once_the_hook_owns_the_exit_the_tray_quit_is_refused() {
        // 審查 M1，方向二：掛鉤先搶到（`quit_gate` 的旗標此時還沒設），之後 `tray::quit` 搶 Tray 失敗 → 忽略，
        // 桌布不被還原。
        let owner = ExitOwner::new();
        let probe = InstallProbe::default();
        probe.begin(true);
        run_before_exit_hook(&probe, &owner, || false, |_| Ok(()));
        assert!(!owner.try_claim(ExitOwnerKind::Tray));
    }

    #[test]
    fn hook_aborts_when_the_exit_steps_cannot_run_for_example_on_the_main_thread() {
        // `ExitError::OnMainThread`：什麼都沒收尾（旗標也沒設），不得繼續啟動安裝檔；擁有權歸還，系統匣「結束」
        // 不會被永久擋住。
        let owner = ExitOwner::new();
        let probe = InstallProbe::default();
        probe.begin(true);
        let msg = abort_message(|| {
            run_before_exit_hook(
                &probe,
                &owner,
                || false,
                |_| Err(exit::ExitError::OnMainThread.to_string()),
            )
        });
        assert!(msg.is_some_and(|m| m.contains("主執行緒")));
        assert!(matches!(
            probe.failure("x".into(), false),
            InstallFailure::BeforeExit(_)
        ));
        assert!(
            owner.try_claim(ExitOwnerKind::Tray),
            "擁有權已歸還：系統匣「結束」仍可用"
        );
    }

    #[test]
    fn quit_in_progress_covers_the_tray_claim_the_quit_flag_and_an_update_exit() {
        // 系統匣「結束」搶到擁有權／`QUIT_STARTED`（等桌布還原中）／「因更新結束」旗標任一為真都不得啟動安裝。
        let src = include_str!("plugin_backend.rs");
        let body: String = src
            .split("fn quit_in_progress_now() -> bool {")
            .nth(1)
            .expect("找得到 quit_in_progress_now")
            .lines()
            .take(3)
            .collect::<Vec<_>>()
            .join(" ");
        assert!(body.contains("exit::tray_owns_exit()"), "{body}");
        assert!(body.contains("crate::tray::quit_started()"), "{body}");
        assert!(body.contains("exit::is_exiting_for_update()"), "{body}");
    }

    /// 只取正式碼（`mod tests` 之前）並去掉所有空白。兩個原因（3.2 複審 N2）：整檔 `contains` 會比對到
    /// 測試自己的斷言字串而永遠成立；rustfmt 又會把呼叫拆成多行，單行字串比對不到正式碼。
    fn production_source_without_whitespace(src: &str) -> String {
        let src = src.replace("\r\n", "\n");
        let (production, _) = src
            .split_once("\n#[cfg(test)]\nmod tests {")
            .expect("找得到測試模組起點");
        production.chars().filter(|c| !c.is_whitespace()).collect()
    }

    #[test]
    fn production_hook_uses_the_global_exit_owner_and_install_classifies_with_the_exiting_flag() {
        let production = production_source_without_whitespace(include_str!("plugin_backend.rs"));
        assert!(production.contains("run_before_exit_hook(&probe,&exit::EXIT_OWNER,"));
        assert!(production.contains("self.probe.failure(message,exit::is_exiting_for_update())"));
    }

    #[test]
    fn building_with_panic_abort_is_rejected_at_compile_time() {
        // 審查 M3：掛鉤中止依賴 panic=unwind。
        let src = include_str!("plugin_backend.rs");
        assert!(src.contains("#[cfg(panic = \"abort\")]\ncompile_error!("));
    }
}
