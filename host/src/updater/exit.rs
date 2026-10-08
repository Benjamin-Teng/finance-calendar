//! 「因更新結束」的收尾基礎（installer-auto-update task 3.3；design.md D3「過渡期」、D4「更新前的結束流程」）。
//!
//! 外掛 `install()` 的順序是：寫暫存檔 → `on_before_exit` → `ShellExecuteW` 啟動安裝檔 → `exit(0)`，不經過 Tauri 的
//! 結束事件，所以 `ExitRequested`／`RunEvent::Exit` 的清理都不會跑。本模組提供 `on_before_exit` 要做的事
//! （[`prepare_exit_for_update`]）與 `install()` 在 `on_before_exit` **之後**才失敗時的退路
//! （[`relaunch_after_failed_install`]）。**task 3.3 建基礎；task 3.2 已把它們接到真正的 `install()`（`plugin_backend` 的 `on_before_exit` 掛鉤與 `after_exit_failure`）。**
//!
//! ## 時間預算（D4 表，逐步執行）
//!
//! | 順序 | 步驟 | 上限 | 性質 | 實作 |
//! |---|---|---|---|---|
//! | 1 | 解除 `RegisterApplicationRestart` | 即時 | 必做 | [`ExitSteps::unregister_restart`]，呼叫端執行緒直接做 |
//! | 2 | 寫出尚未存檔的設定 | — | — | **刪除**：宿主沒有延後存檔（見下） |
//! | 3 | 桌布協調迴圈「因更新結束」 | 5 秒 | 可跳過 | [`ExitSteps::stop_coordinator`] |
//! | 4 | 停止資料抓取 | 2 秒 | 可跳過 | [`ExitSteps::stop_fetch`] |
//! | 5 | 經 `run_on_main_thread` 關閉所有 WebView2 視窗與編輯版面格線、移除系統匣圖示 | 3 秒 | 可跳過 | [`ExitSteps::close_ui`]；尚未建立 UI 時只移除系統匣圖示：[`ExitSteps::remove_tray`] |
//! | 6 | 等渲染 browser 行程結束 | 5 秒 | 可跳過 | [`ExitSteps::settle_render_browser`] |
//! | 7 | 刪當機標記、記錄「因更新結束」、flush | 1 秒 | 必做 | [`ExitSteps::finalize`] |
//!
//! 各步上限加總 16 秒（D4 原表含步驟 2 的 2 秒為 18 秒），整段另有 [`ExitBudget::total`]＝20 秒的總預算：可跳過的步驟
//! 實際可用時間＝`min(自己的上限, 總預算 − 已用 − 步驟 7 的保留 − 寬限)`，預算用盡就略過（記為逾時）。步驟 7 的 1 秒永遠保留，
//! 所以「必做」的步驟本身有界、一定執行。
//!
//! 可跳過的步驟與步驟 7 都在**專屬執行緒**上跑、由呼叫端以 `recv_timeout` 等待：逾時就放棄等待（執行緒留在背景，
//! 行程馬上要 `exit`）、繼續下一步，不讓任何一步卡死整段。
//!
//! ### 尚未建立 UI（過渡期同步安裝，`ui_built=false`）
//!
//! 略過 3、6（沒有協調迴圈、沒有渲染器）；步驟 5 不略過，改成只移除系統匣圖示（[`ExitSteps::remove_tray`]，上限同為
//! 3 秒）。系統匣圖示由 `tauri.conf.json` 的 `app.trayIcon` 在 setup 之前就建立，過渡期也在；外掛的 `on_before_exit`
//! 已換成我們的掛鉤（不再呼叫 `cleanup_before_exit`），不移除就 `exit(0)` 會在系統匣留下殘影，新版啟動後出現兩個圖示
//! （最終審查 M1）。
//!
//! 這一步同樣經 `run_on_main_thread`，但不會死鎖：design.md D3 規定過渡期主執行緒全程不等待任何背景執行緒（建立 UI
//! 是投遞到主執行緒、`quit_during_transition` 與交接排隊都不等待），所以主執行緒閒置時閉包立即執行；主執行緒忙碌
//! （例如卡在某個同步呼叫）時，工作執行緒以 `recv_timeout` 在上限內放棄，記為逾時、繼續步驟 7。
//!
//! ### 步驟 2 為什麼刪除（先查證的結論）
//!
//! 設計稿（審查 R2）要求先確認是否存在「延後存檔」。查證 `host/src` 所有 `settings::save` 呼叫點（`widgets.rs`：
//! `update_settings`、`set_wallpaper_theme_none`、`resave_wallpaper_theme_none`、`ensure_wallpaper_theme_none_saved`、
//! `set_edit_mode`、拖曳結束 `judge_pending_drop`；`wallpaper_coordinator.rs`：主題改不接管的存檔）——全都是**在呼叫
//! 當下同步寫檔**（`settings::write_atomic`：寫 `.tmp` 再 `rename`，不會留下半份檔），沒有去抖動計時器、沒有背景
//! 寫檔執行緒、沒有「記憶體已改、稍後才落盤」的設定快取。因此「強制寫出延後存檔的設定」**無事可做，整步刪除**，
//! 也就不需要 `Mutex<Settings>` 的 `try_lock` 輪詢。兩個邊界記錄在此，不是缺口：
//!
//! - 拖曳放開時的判定經 `PostMessageW` 延到同一扇視窗的訊息佇列（`widgets::finish_widget_drag`／`settle_widget_drag`），
//!   視窗在這幾毫秒內被關閉時該次移動不寫回（`forget_widget_drag`）。這不是延後存檔——使用者的拖曳在更新前結束流程的
//!   20 秒內才放開，等同「更新時剛好在拖曳」，且寫回的只是版面位置，下次啟動照推導矩形顯示。
//! - 協調迴圈的 `theme_save_retry`（主題改不接管、存檔失敗後的退避重試）是真正的延後存檔，但「因更新結束」明定
//!   **不動主題**（語意同工作階段結束），重試隨迴圈結束而作廢；新版啟動後狀態檔仍是接管中、主題照舊。
//!
//! ## 不得在主執行緒呼叫
//!
//! 步驟 5 經 `run_on_main_thread` 在主執行緒關視窗；呼叫端若就是主執行緒，等待期間主執行緒不跑訊息迴圈，只會白白耗滿
//! 3 秒。`main()` 開頭呼叫 [`mark_main_thread`] 記下主執行緒 id，[`prepare_exit_for_update`] 發現自己在主執行緒就直接回
//! [`ExitError::OnMainThread`]、什麼都不做。（Rust 沒有辦法用型別標記「非主執行緒」，這是能做到最接近的保證。）
//!
//! ## 失敗退路
//!
//! `install()` 在 `on_before_exit` 之後失敗（例如 `ShellExecuteW` 失敗）時，宿主已經沒有小工具、沒有桌布協調：
//! [`relaunch_after_failed_install`] 以 `--autostart --wait-exit <本 PID>` 啟動新的自己，記錄後以 `std::process::exit`
//! 結束（不用 `app.exit`：它走 `ExitRequested`，可能誤入系統匣「結束」的還原桌布路徑）。新行程的 `--wait-exit`
//! 處理在 `desktop::wait_exit`。

use std::ffi::OsString;
use std::io;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};

use super::LOG_TARGET;

/// 各步時間上限與整段總預算（D4 表）。測試用較短的值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitBudget {
    /// 整段上限（含步驟 7）：D4 要求不超過 20 秒。
    pub total: Duration,
    /// 步驟 3：協調迴圈「因更新結束」。
    pub coordinator: Duration,
    /// 步驟 4：停止資料抓取。
    pub fetch: Duration,
    /// 步驟 5：關閉 WebView2 視窗與移除系統匣圖示。
    pub ui: Duration,
    /// 步驟 6：等渲染 browser 行程結束。
    pub browser: Duration,
    /// 步驟 7：刪標記、記錄、flush（總預算永遠為它保留這段）。
    pub finalize: Duration,
    /// 等工作執行緒回報時，在步驟上限之外多給的寬限（讓步驟自己的內部逾時判定有機會先回報，而不是被外層搶先放棄）。
    /// 總預算的扣除一併計入（每個被等待的步驟各一次），所以整段耗時不會超過 `total`。
    pub grace: Duration,
}

impl Default for ExitBudget {
    fn default() -> Self {
        Self {
            total: Duration::from_secs(20),
            coordinator: Duration::from_secs(5),
            fetch: Duration::from_secs(2),
            ui: Duration::from_secs(3),
            browser: Duration::from_secs(5),
            finalize: Duration::from_secs(1),
            grace: Duration::from_millis(100),
        }
    }
}

/// 收尾的步驟（編號與 D4 表一致；步驟 2 已刪除，故沒有 2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitStep {
    UnregisterRestart,
    Coordinator,
    StopFetch,
    CloseUi,
    SettleBrowser,
    Finalize,
}

impl ExitStep {
    /// D4 表的順序編號。
    pub fn number(self) -> u8 {
        match self {
            Self::UnregisterRestart => 1,
            Self::Coordinator => 3,
            Self::StopFetch => 4,
            Self::CloseUi => 5,
            Self::SettleBrowser => 6,
            Self::Finalize => 7,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::UnregisterRestart => "解除重新啟動註冊",
            Self::Coordinator => "協調迴圈結束",
            Self::StopFetch => "停止資料抓取",
            Self::CloseUi => "關閉視窗與系統匣圖示",
            Self::SettleBrowser => "渲染 browser 行程",
            Self::Finalize => "刪標記與記錄",
        }
    }
}

/// 一步的結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepStatus {
    /// 完成。
    Done,
    /// 不適用（沒有該元件，或尚未建立 UI 而略過）。
    NotApplicable,
    /// 逾時（上限內沒完成，已放棄等待；含總預算用盡而略過）。
    TimedOut,
    /// 失敗（錯誤已記在 `detail`）。
    Failed,
}

/// 步驟實作回傳的結果與一行說明。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepOutput {
    pub status: StepStatus,
    pub detail: String,
}

impl StepOutput {
    pub fn done(detail: impl Into<String>) -> Self {
        Self {
            status: StepStatus::Done,
            detail: detail.into(),
        }
    }
    pub fn not_applicable(detail: impl Into<String>) -> Self {
        Self {
            status: StepStatus::NotApplicable,
            detail: detail.into(),
        }
    }
    pub fn timed_out(detail: impl Into<String>) -> Self {
        Self {
            status: StepStatus::TimedOut,
            detail: detail.into(),
        }
    }
    pub fn failed(detail: impl Into<String>) -> Self {
        Self {
            status: StepStatus::Failed,
            detail: detail.into(),
        }
    }
}

/// 一步的報告：耗時、結果、說明。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepReport {
    pub step: ExitStep,
    pub status: StepStatus,
    pub elapsed: Duration,
    pub detail: String,
}

/// 整段收尾的報告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitReport {
    pub steps: Vec<StepReport>,
    /// 從開始到最後一步結束的總耗時。
    pub total: Duration,
    /// 本次使用的總預算（20 秒）。
    pub budget: Duration,
    /// 是否在 UI 已建立的情況下執行（`false`＝略過 3、6，步驟 5 只移除系統匣圖示）。
    pub ui_built: bool,
}

impl ExitReport {
    /// 總耗時是否在總預算內。
    pub fn within_budget(&self) -> bool {
        self.total <= self.budget
    }

    /// 逾時的步驟。
    pub fn timed_out_steps(&self) -> Vec<ExitStep> {
        self.steps
            .iter()
            .filter(|s| s.status == StepStatus::TimedOut)
            .map(|s| s.step)
            .collect()
    }

    /// 取得某一步的報告。
    #[cfg_attr(not(test), allow(dead_code))] // 測試用的查詢；正式路徑只記 `finalize` 寫出的摘要。
    pub fn step(&self, step: ExitStep) -> Option<&StepReport> {
        self.steps.iter().find(|s| s.step == step)
    }

    /// 一行摘要（每步的編號、結果、耗時毫秒）。
    #[cfg_attr(not(test), allow(dead_code))] // 同上：正式路徑在 `run_exit_for_update` 內以 `summarize` 直接取用。
    pub fn summary(&self) -> String {
        summarize(&self.steps, self.total, self.budget, self.ui_built)
    }
}

fn summarize(steps: &[StepReport], total: Duration, budget: Duration, ui_built: bool) -> String {
    let parts: Vec<String> = steps
        .iter()
        .map(|s| {
            // 說明一併寫進記錄（例如尚未建立 UI 時步驟 5 的系統匣圖示是否移除成功，最終審查 M1）。
            let detail = if s.detail.is_empty() {
                String::new()
            } else {
                format!("，{}", s.detail)
            };
            format!(
                "{}{}={}（{} ms{detail}）",
                s.step.number(),
                s.step.label(),
                match s.status {
                    StepStatus::Done => "完成",
                    StepStatus::NotApplicable => "略過",
                    StepStatus::TimedOut => "逾時",
                    StepStatus::Failed => "失敗",
                },
                s.elapsed.as_millis()
            )
        })
        .collect();
    format!(
        "因更新結束：收尾（{}）共 {} ms／預算 {} ms；{}",
        if ui_built {
            "UI 已建立"
        } else {
            "尚未建立 UI"
        },
        total.as_millis(),
        budget.as_millis(),
        parts.join("；")
    )
}

/// 收尾步驟的實作（可注入：正式＝[`AppExitSteps`]，測試用假物件）。
///
/// 可跳過的步驟與 `finalize` 會在**工作執行緒**上呼叫（所以要 `Send + Sync + 'static`）；回傳的 `limit` 是
/// 這一步實際可用的時間，實作應自己在 `limit` 內放棄（外層另有 [`ExitBudget::grace`] 寬限後強制放棄等待）。
pub trait ExitSteps: Send + Sync + 'static {
    /// 步驟 1。
    fn unregister_restart(&self);
    /// 步驟 3：要求協調迴圈「因更新結束」並等它（尚未建立 UI 時不會被呼叫）。
    fn stop_coordinator(&self, limit: Duration) -> StepOutput;
    /// 步驟 4：停止資料抓取（下一個來源邊界停）。
    fn stop_fetch(&self, limit: Duration) -> StepOutput;
    /// 步驟 5：經 `run_on_main_thread` 關閉所有 WebView2 視窗並移除系統匣圖示（尚未建立 UI 時不會被呼叫）。
    fn close_ui(&self, limit: Duration) -> StepOutput;
    /// 步驟 5（尚未建立 UI 時）：只移除系統匣圖示（經 `run_on_main_thread`，`limit` 內放棄；UI 已建立時不會被呼叫，
    /// 由 [`close_ui`](Self::close_ui) 一併處理）。
    fn remove_tray(&self, limit: Duration) -> StepOutput;
    /// 步驟 6：等渲染 browser 行程結束（尚未建立 UI 時不會被呼叫）。
    fn settle_render_browser(&self, limit: Duration) -> StepOutput;
    /// 步驟 7：刪當機標記、把 `summary`（步驟 1–6 的摘要）記成「因更新結束」並 flush 記錄檔。
    fn finalize(&self, summary: &str, limit: Duration) -> StepOutput;
}

/// 可跳過步驟的工作內容（在工作執行緒上以該步實際可用的時間呼叫）。
type StepFn<S> = Box<dyn FnOnce(&S, Duration) -> StepOutput + Send>;

/// 在專屬執行緒跑 `f`，最多等 `limit + grace`。逾時放棄等待、執行緒留在背景。
fn run_bounded<S, F>(
    steps: &Arc<S>,
    step: ExitStep,
    limit: Duration,
    grace: Duration,
    f: F,
) -> StepReport
where
    S: ExitSteps,
    F: FnOnce(&S, Duration) -> StepOutput + Send + 'static,
{
    let started = Instant::now();
    let (tx, rx) = mpsc::channel();
    let worker_steps = Arc::clone(steps);
    let spawned = thread::Builder::new()
        .name(format!("fc-exit-{}", step.number()))
        .spawn(move || {
            let out = catch_unwind(AssertUnwindSafe(|| f(&worker_steps, limit)));
            let _ = tx.send(out);
        });
    let output = match spawned {
        Err(e) => StepOutput::failed(format!("無法建立工作執行緒：{e}")),
        Ok(_) => match rx.recv_timeout(limit + grace) {
            Ok(Ok(out)) => out,
            Ok(Err(_)) => StepOutput::failed("步驟 panic（已隔離）"),
            Err(RecvTimeoutError::Timeout) => StepOutput::timed_out(format!(
                "{:.1} 秒內沒有完成，放棄等待（工作執行緒留在背景）",
                limit.as_secs_f32()
            )),
            Err(RecvTimeoutError::Disconnected) => StepOutput::failed("工作執行緒意外結束"),
        },
    };
    StepReport {
        step,
        status: output.status,
        elapsed: started.elapsed(),
        detail: output.detail,
    }
}

/// 依 D4 表執行收尾（純邏輯；不檢查主執行緒，見 [`prepare_exit_for_update`]）。
///
/// `ui_built=false`（過渡期：當機迴圈保護的同步安裝）時略過 3、6（不呼叫），步驟 5 改呼叫
/// [`ExitSteps::remove_tray`]（只移除系統匣圖示，上限同步驟 5；不死鎖的理由見模組文件）。
pub fn run_exit_for_update<S: ExitSteps>(
    steps: &Arc<S>,
    ui_built: bool,
    budget: ExitBudget,
) -> ExitReport {
    let start = Instant::now();
    let mut reports: Vec<StepReport> = Vec::with_capacity(6);

    // 步驟 1：必做、即時。
    let t = Instant::now();
    steps.unregister_restart();
    reports.push(StepReport {
        step: ExitStep::UnregisterRestart,
        status: StepStatus::Done,
        elapsed: t.elapsed(),
        detail: String::new(),
    });

    // 可跳過的步驟：可用時間＝min(自己的上限, 總預算 − 已用 − 步驟 7 的保留)。
    let mut skippable = |step: ExitStep, limit: Duration, needs_ui: bool, f: StepFn<S>| {
        if needs_ui && !ui_built {
            reports.push(StepReport {
                step,
                status: StepStatus::NotApplicable,
                elapsed: Duration::ZERO,
                detail: "尚未建立 UI，略過".to_owned(),
            });
            return;
        }
        // 步驟 7 的保留（它自己的上限加寬限）與這一步的寬限都先扣掉，才能保證整段不超過總預算。
        let remaining = budget
            .total
            .saturating_sub(start.elapsed())
            .saturating_sub(budget.finalize + budget.grace)
            .saturating_sub(budget.grace);
        let effective = limit.min(remaining);
        if effective.is_zero() {
            reports.push(StepReport {
                step,
                status: StepStatus::TimedOut,
                elapsed: Duration::ZERO,
                detail: "總預算已用盡，略過".to_owned(),
            });
            return;
        }
        reports.push(run_bounded(steps, step, effective, budget.grace, f));
    };
    skippable(
        ExitStep::Coordinator,
        budget.coordinator,
        true,
        Box::new(|s, limit| s.stop_coordinator(limit)),
    );
    skippable(
        ExitStep::StopFetch,
        budget.fetch,
        false,
        Box::new(|s, limit| s.stop_fetch(limit)),
    );
    // 步驟 5：UI 已建立＝關視窗＋移除系統匣圖示；尚未建立 UI＝只移除系統匣圖示（最終審查 M1：系統匣圖示在 setup
    // 之前就已建立，不移除會在 `exit(0)` 後留下殘影）。
    skippable(
        ExitStep::CloseUi,
        budget.ui,
        false,
        if ui_built {
            Box::new(|s, limit| s.close_ui(limit))
        } else {
            Box::new(|s, limit| s.remove_tray(limit))
        },
    );
    skippable(
        ExitStep::SettleBrowser,
        budget.browser,
        true,
        Box::new(|s, limit| s.settle_render_browser(limit)),
    );

    // 步驟 7：必做，永遠有自己保留的時間；摘要涵蓋步驟 1–6。
    let summary = summarize(&reports, start.elapsed(), budget.total, ui_built);
    reports.push(run_bounded(
        steps,
        ExitStep::Finalize,
        budget.finalize,
        budget.grace,
        move |s, limit| s.finalize(&summary, limit),
    ));

    ExitReport {
        steps: reports,
        total: start.elapsed(),
        budget: budget.total,
        ui_built,
    }
}

// ---------------------------------------------------------------------------------------------
// 主執行緒保護
// ---------------------------------------------------------------------------------------------

/// 主執行緒的 id（`main()` 開頭記下）。
static MAIN_THREAD: OnceLock<ThreadId> = OnceLock::new();

/// `main()` 最開頭呼叫一次：記下主執行緒 id，[`prepare_exit_for_update`] 靠它拒絕在主執行緒執行。
pub fn mark_main_thread() {
    let _ = MAIN_THREAD.set(thread::current().id());
}

/// 目前執行緒是不是 `main()` 記下的主執行緒（`install()` 的呼叫端自查用；還沒記下時一律回 `false`）。
pub fn current_is_main_thread() -> bool {
    is_marked_main_thread(MAIN_THREAD.get().copied(), thread::current().id())
}

/// 純判定：已記下主執行緒、且目前就是它。
fn is_marked_main_thread(main: Option<ThreadId>, current: ThreadId) -> bool {
    main == Some(current)
}

// ---------------------------------------------------------------------------------------------
// 「正在因更新結束」全域旗標（審查 I1）
// ---------------------------------------------------------------------------------------------

/// 一旦 [`prepare_exit_for_update`] 開始就永遠為真（行程即將被 `install()` 的 `exit(0)` 結束，或 `relaunch_after_failed_install`
/// 的 `process::exit` 結束，不需要也不應該清除）。
///
/// 收尾期間行程仍有系統匣圖示（步驟 5 才移除）與小工具視窗，其他路徑若在這段時間動作會破壞「更新不還原桌布」：
///
/// - 系統匣「結束」→ 還原桌布並把主題存成「不接管」（[`quit_gate`] 對 `updater::on_tray_quit` 回 `IgnoreInstalling`，
///   `tray::quit` 因而直接返回；`tray::save_theme_none_on_exit` 另有保險）；
/// - `widgets::sync_widget_windows`／`rebuild_widget_windows`、`recovery` 的故障重建、`tray::open_settings_window`
///   → 在步驟 5 銷毀視窗之後又把視窗建回來。
///
/// 這些入口開頭都檢查 [`is_exiting_for_update`]，為真就記錄並直接返回。
///
/// **3.2 的用法**：旗標只由 `install()` 的 `on_before_exit` 裡呼叫的 [`prepare_exit_for_update`] 設置，3.2 不需要、也
/// **不應**在呼叫 `install()` 之前自己設——`install()` 在 `on_before_exit` 之前失敗（寫暫存檔失敗）時宿主照常運作，
/// 旗標一旦為真就不會復原，會讓系統匣「結束」與視窗同步永久失效。`on_before_exit` 之後失敗走
/// [`relaunch_after_failed_install`] 時旗標維持為真（行程立刻結束，沒有人需要它變回假）。尚未建立 UI 的同步安裝
/// （`ui_built=false`）同樣由 `prepare_exit_for_update` 設旗標，此時系統匣「結束」本就已被過渡期狀態機擋成
/// `IgnoreInstalling`，旗標只是補上收尾期間的重疊。
#[cfg_attr(test, allow(dead_code))] // 測試改用執行緒區域旗標（見 `TEST_EXITING`）。
static EXITING_FOR_UPDATE: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
thread_local! {
    /// 測試改讀寫這個執行緒區域旗標：全域旗標一旦設成真就不會復原，會污染同一個測試行程裡並行的其他測試。
    static TEST_EXITING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// 是否已經開始因更新結束（見 [`EXITING_FOR_UPDATE`]）。
pub fn is_exiting_for_update() -> bool {
    #[cfg(test)]
    {
        TEST_EXITING.with(std::cell::Cell::get)
    }
    #[cfg(not(test))]
    {
        EXITING_FOR_UPDATE.load(Ordering::SeqCst)
    }
}

/// 設旗標（冪等）。
// 只由 `enter_exiting_state`（`prepare_exit_for_update`）呼叫；3.2 的安裝流程不得提前呼叫（見 [`EXITING_FOR_UPDATE`]）。
pub fn begin_exiting_for_update() {
    #[cfg(test)]
    TEST_EXITING.with(|f| f.set(true));
    #[cfg(not(test))]
    EXITING_FOR_UPDATE.store(true, Ordering::SeqCst);
    // 同時關閉「啟動閘門」：此後尚未啟動的協調迴圈、抓取排程都不得再啟動（見 [`StartGate`]）。
    START_GATE.close();
}

// ---------------------------------------------------------------------------------------------
// 啟動閘門：收尾與「建立 UI」的競態（installer-auto-update 4.x 實機發現 A3）
// ---------------------------------------------------------------------------------------------

/// 「元件啟動」與「收尾」之間的閘門。
///
/// **競態**：`--autostart` 後很快就自動安裝時，更新任務（背景執行緒）在 `setup` 的 `build_ui`（主執行緒）還沒跑完前就進入
/// `prepare_exit_for_update(ui_built=true)`。收尾的步驟 3（協調迴圈）、4（抓取）看到「沒有 managed state」就判為不適用，
/// 之後 `build_ui` 才啟動它們——收尾期間協調迴圈照常 `SetWallpaper`／渲染、抓取寫檔寫到一半被 `exit(0)` 終止。
///
/// **解法**：旗標與「啟動中」計數放在同一把鎖裡，使「啟動」與「收尾開始」互相排序——
///
/// - 啟動端在啟動點先 [`try_begin`](Self::try_begin)：閘門已關就回 `None`（不得啟動）；否則取得 [`StartPermit`]，
///   **啟動完成（managed state 已登記）後才放掉**；
/// - 收尾端先 [`close`](Self::close)（`begin_exiting_for_update`），步驟 3／4 再 [`wait_idle`](Self::wait_idle)
///   等所有「啟動中」的許可放掉，之後 managed state 不是已登記（照常停止它）就是永遠不會登記（不適用）。
///
/// 不論哪一邊先拿到鎖，結果只有兩種：啟動端看見閘門已關而不啟動，或收尾端看見有人在啟動而等它。
pub struct StartGate {
    state: Mutex<GateState>,
    idle: Condvar,
}

struct GateState {
    closed: bool,
    starting: usize,
}

/// 啟動中的許可；放掉（`Drop`）時通知等待者。
#[must_use = "許可要持有到元件啟動完成（managed state 已登記）才放掉"]
pub struct StartPermit<'a> {
    gate: &'a StartGate,
}

impl Drop for StartPermit<'_> {
    fn drop(&mut self) {
        let mut st = self.gate.lock();
        st.starting = st.starting.saturating_sub(1);
        if st.starting == 0 {
            self.gate.idle.notify_all();
        }
    }
}

impl StartGate {
    pub const fn new() -> Self {
        Self {
            state: Mutex::new(GateState {
                closed: false,
                starting: 0,
            }),
            idle: Condvar::new(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, GateState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 啟動端：閘門已關回 `None`（不得啟動），否則回許可。
    pub fn try_begin(&self) -> Option<StartPermit<'_>> {
        let mut st = self.lock();
        if st.closed {
            return None;
        }
        st.starting += 1;
        Some(StartPermit { gate: self })
    }

    /// 收尾端：關閉閘門（冪等）。已取得的許可不受影響，由 [`wait_idle`](Self::wait_idle) 等它們。
    pub fn close(&self) {
        self.lock().closed = true;
    }

    /// 等到沒有任何「啟動中」的許可；`limit` 內等到回 `true`，逾時回 `false`。
    pub fn wait_idle(&self, limit: Duration) -> bool {
        let guard = self.lock();
        let (guard, _) = self
            .idle
            .wait_timeout_while(guard, limit, |st| st.starting > 0)
            .unwrap_or_else(|e| e.into_inner());
        guard.starting == 0
    }
}

impl Default for StartGate {
    fn default() -> Self {
        Self::new()
    }
}

/// 全行程唯一的啟動閘門（單元測試各自建立 [`StartGate`] 實例；`begin_exiting_for_update` 會關這個）。
pub static START_GATE: StartGate = StartGate::new();

/// 啟動點用：`what` 只用於記錄。閘門已關（正在因更新結束）回 `None`，呼叫端必須放棄啟動。
pub fn try_begin_start(what: &str) -> Option<StartPermit<'static>> {
    let permit = START_GATE.try_begin();
    if permit.is_none() {
        log::info!(target: LOG_TARGET, "因更新結束：不啟動{what}");
    }
    permit
}

/// 收尾步驟 3／4 的共同骨架：先等「啟動中」的元件啟動完成（`limit` 內），再用剩餘時間執行 `stop`。
/// 等不到就回逾時（元件還在啟動、不知道它之後會不會註冊，不能假裝沒事）。
fn stop_after_start(
    gate: &StartGate,
    limit: Duration,
    what: &str,
    stop: impl FnOnce(Duration) -> StepOutput,
) -> StepOutput {
    let started = Instant::now();
    if !gate.wait_idle(limit) {
        return StepOutput::timed_out(format!(
            "{what}仍在啟動中，{:.1} 秒內沒有完成",
            limit.as_secs_f32()
        ));
    }
    stop(limit.saturating_sub(started.elapsed()))
}

/// 系統匣「結束」的處置閘門：正在因更新結束時一律 `IgnoreInstalling`（不呼叫 `normal`）；否則照 `normal`。
pub fn quit_gate(normal: impl FnOnce() -> super::QuitDisposition) -> super::QuitDisposition {
    if is_exiting_for_update() {
        log::info!(target: LOG_TARGET, "系統匣：正在因更新結束，忽略「結束」（不還原桌布、不存主題）");
        return super::QuitDisposition::IgnoreInstalling;
    }
    normal()
}

// ---------------------------------------------------------------------------------------------
// 「結束擁有者」：系統匣「結束」與「因更新結束」的互斥（審查 M1）
// ---------------------------------------------------------------------------------------------

/// 誰擁有「這次結束」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitOwnerKind {
    /// 系統匣「結束」：還原桌布、存主題、`app.exit(0)`。
    Tray,
    /// 因更新結束：不還原桌布、不存主題，由外掛 `install()` 結束行程。
    Update,
}

impl ExitOwnerKind {
    fn code(self) -> u8 {
        match self {
            Self::Tray => 1,
            Self::Update => 2,
        }
    }
}

/// 單一原子值的「結束擁有者」（0＝沒有）。兩條結束流程在**做任何事之前**都以 CAS 搶：搶輸的一方放棄，
/// 所以不管兩邊的先後順序如何，不會出現「桌布被還原（系統匣結束）又同時因更新結束」——spec「更新不還原桌布」。
///
/// - 系統匣「結束」：`tray::quit` 在 `quit_action` 之後、解除重啟註冊／刪標記／還原之前搶 [`ExitOwnerKind::Tray`]，
///   搶輸（更新已搶先）就忽略這次按下。
/// - 更新：`on_before_exit` 掛鉤在呼叫 `prepare_exit_for_update` 之前搶 [`ExitOwnerKind::Update`]，搶輸（系統匣
///   結束已開始）就中止安裝；之後若 `prepare` 本身無法執行（`OnMainThread`），掛鉤要 [`release`](Self::release)
///   歸還，否則系統匣「結束」會被永久擋住。
///
/// 同一方重複搶視為成功（系統匣「結束」可以被連按）。
pub struct ExitOwner(AtomicU8);

impl ExitOwner {
    pub const fn new() -> Self {
        Self(AtomicU8::new(0))
    }

    /// 嘗試成為擁有者；已經是同一方也回 `true`。
    pub fn try_claim(&self, who: ExitOwnerKind) -> bool {
        match self
            .0
            .compare_exchange(0, who.code(), Ordering::SeqCst, Ordering::SeqCst)
        {
            Ok(_) => true,
            Err(current) => current == who.code(),
        }
    }

    /// 歸還（只有目前確實是 `who` 才會清掉）。
    pub fn release(&self, who: ExitOwnerKind) {
        let _ = self
            .0
            .compare_exchange(who.code(), 0, Ordering::SeqCst, Ordering::SeqCst);
    }

    pub fn is_owned_by(&self, who: ExitOwnerKind) -> bool {
        self.0.load(Ordering::SeqCst) == who.code()
    }
}

impl Default for ExitOwner {
    fn default() -> Self {
        Self::new()
    }
}

/// 全行程唯一的結束擁有者（單元測試各自建立 [`ExitOwner`] 實例，不碰這個全域值）。
pub static EXIT_OWNER: ExitOwner = ExitOwner::new();

/// 系統匣「結束」搶擁有權（`tray::quit` 用）。
pub fn claim_tray_quit() -> bool {
    EXIT_OWNER.try_claim(ExitOwnerKind::Tray)
}

/// 系統匣「結束」已搶到擁有權（早於 `QUIT_STARTED`，後者要等到還原流程開始才設）。
pub fn tray_owns_exit() -> bool {
    EXIT_OWNER.is_owned_by(ExitOwnerKind::Tray)
}

/// 入口檢查：不在主執行緒才設旗標。主執行緒呼叫時回 `Err` 且**不設旗標**（什麼都不做），並記一筆 error。
fn enter_exiting_state(main: Option<ThreadId>, current: ThreadId) -> Result<(), ExitError> {
    if is_marked_main_thread(main, current) {
        log::error!(target: LOG_TARGET, "因更新結束：{}", ExitError::OnMainThread);
        return Err(ExitError::OnMainThread);
    }
    begin_exiting_for_update();
    Ok(())
}

/// [`prepare_exit_for_update`] 的錯誤。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitError {
    /// 在主執行緒呼叫：步驟 5 的 `run_on_main_thread` 會等不到主執行緒，什麼都沒做。
    OnMainThread,
}

impl std::fmt::Display for ExitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OnMainThread => {
                f.write_str("不得在主執行緒執行更新前收尾（會與 run_on_main_thread 死鎖）")
            }
        }
    }
}

impl std::error::Error for ExitError {}

/// `on_before_exit` 要做的事（`plugin_backend::run_before_exit_hook` 呼叫它）：依 D4 表收尾並回傳每步耗時與逾時報告，同時記一行摘要。
///
/// 只能在背景執行緒呼叫；在主執行緒呼叫回 [`ExitError::OnMainThread`]。`ui_built` 由呼叫端依過渡期狀態機決定
/// （`false`＝「安裝」狀態的未建 UI 收尾）。
pub fn prepare_exit_for_update(app: &AppHandle, ui_built: bool) -> Result<ExitReport, ExitError> {
    // 先設旗標再做任何事（見 [`EXITING_FOR_UPDATE`]）；主執行緒呼叫時回 Err、不設旗標、不做事。
    enter_exiting_state(MAIN_THREAD.get().copied(), thread::current().id())?;
    let steps = Arc::new(AppExitSteps { app: app.clone() });
    let report = run_exit_for_update(&steps, ui_built, ExitBudget::default());
    if !report.within_budget() {
        log::warn!(
            target: LOG_TARGET,
            "因更新結束：收尾總耗時 {} ms 超過預算 {} ms（逾時步驟：{:?}）",
            report.total.as_millis(),
            report.budget.as_millis(),
            report.timed_out_steps()
        );
        log::logger().flush();
    }
    Ok(report)
}

// ---------------------------------------------------------------------------------------------
// 正式實作
// ---------------------------------------------------------------------------------------------

/// 以 `AppHandle` 實作各步驟。
struct AppExitSteps {
    app: AppHandle,
}

impl ExitSteps for AppExitSteps {
    fn unregister_restart(&self) {
        crate::desktop::unregister_restart();
    }

    fn stop_coordinator(&self, limit: Duration) -> StepOutput {
        use crate::wallpaper_coordinator::{exit_for_update, CoordinatorHandle, ExitForUpdateWait};
        // 協調迴圈可能正在 `build_ui`（主執行緒）裡啟動：先等它啟動完成，再判斷有沒有把手（見 [`StartGate`]）。
        stop_after_start(&START_GATE, limit, "桌布協調迴圈", |limit| {
            let Some(handle) = self.app.try_state::<CoordinatorHandle>() else {
                return StepOutput::not_applicable("沒有桌布協調迴圈");
            };
            match exit_for_update(&handle, limit) {
                ExitForUpdateWait::Exited => {
                    StepOutput::done("協調迴圈已結束（未還原、未動狀態檔與主題）")
                }
                ExitForUpdateWait::Disconnected => StepOutput::done("協調迴圈已不在"),
                ExitForUpdateWait::TimedOut => StepOutput::timed_out("協調迴圈沒有在時限內處理"),
            }
        })
    }

    fn stop_fetch(&self, limit: Duration) -> StepOutput {
        use crate::fetch::scheduler::{stop_app, StopOutcome};
        stop_after_start(&START_GATE, limit, "抓取排程", |limit| {
            match stop_app(&self.app, limit) {
                None => StepOutput::not_applicable("沒有抓取排程"),
                Some(StopOutcome::Stopped) => StepOutput::done("抓取排程已停止"),
                Some(StopOutcome::TimedOut) => StepOutput::timed_out("抓取排程沒有在時限內停下"),
            }
        })
    }

    fn close_ui(&self, limit: Duration) -> StepOutput {
        let deadline = Instant::now() + limit;
        let (tx, rx) = mpsc::channel();
        let app = self.app.clone();
        // 在主執行緒：強制銷毀所有 WebView2 視窗（`destroy` 不經 `CloseRequested`）、藏起並移除系統匣圖示
        // （`NIM_DELETE`；`exit(0)` 不會替我們做）。`Shell_NotifyIconW` 是送 explorer 的同步呼叫，explorer 卡住時
        // 主執行緒會停在這裡——外層逾時照樣放棄。
        // widget-adaptive-zoom-and-grid task 5.2 修正第 1 輪：編輯版面的格線疊加視窗是原生視窗、不在
        // `webview_windows()` 裡，經 `widgets::clear_grid_overlays` 在同一個主執行緒閉包內一併銷毀（格線集合在
        // 主執行緒的 thread-local）。之後的建立入口都已被 `is_exiting_for_update()` 擋住。
        let queued = self.app.run_on_main_thread(move || {
            let mut destroyed = 0usize;
            for (_, window) in app.webview_windows() {
                if window.destroy().is_ok() {
                    destroyed += 1;
                }
            }
            let overlays = crate::widgets::clear_grid_overlays();
            let tray_removed = remove_tray_icon(&app);
            let _ = tx.send((destroyed, overlays, tray_removed));
        });
        if let Err(e) = queued {
            return StepOutput::failed(format!("run_on_main_thread 失敗：{e}"));
        }
        let (destroyed, overlays, tray_removed) = match rx.recv_timeout(limit) {
            Ok(v) => v,
            Err(_) => return StepOutput::timed_out("主執行緒沒有在時限內處理（卡住或忙碌）"),
        };
        // `destroy` 的實際銷毀是主執行緒事件迴圈稍後處理的：輪詢視窗表清空。
        while !self.app.webview_windows().is_empty() {
            if Instant::now() >= deadline {
                return StepOutput::timed_out(format!(
                    "已要求銷毀 {destroyed} 個視窗，但時限內仍有視窗殘留"
                ));
            }
            thread::sleep(Duration::from_millis(20));
        }
        StepOutput::done(format!(
            "已銷毀 {destroyed} 個視窗、{overlays} 個格線；系統匣圖示{}",
            if tray_removed {
                "已移除"
            } else {
                "不存在"
            }
        ))
    }

    fn remove_tray(&self, limit: Duration) -> StepOutput {
        let (tx, rx) = mpsc::channel();
        let app = self.app.clone();
        // 過渡期主執行緒不等待任何背景執行緒（design.md D3），所以閉包在主執行緒閒置時立即執行；主執行緒忙碌或卡在
        // `Shell_NotifyIconW`（explorer 卡住）時，這裡在 `limit` 內放棄（外層另有寬限後強制放棄）。
        let queued = self.app.run_on_main_thread(move || {
            let _ = tx.send(remove_tray_icon(&app));
        });
        // 結果說明會出現在步驟 7 記下的收尾摘要裡（記錄檔寫明這一步的結果）。
        match queued {
            Err(e) => StepOutput::failed(format!(
                "尚未建立 UI：run_on_main_thread 失敗，系統匣圖示未移除（{e}）"
            )),
            Ok(()) => match rx.recv_timeout(limit) {
                Ok(true) => StepOutput::done("尚未建立 UI：只移除系統匣圖示，已移除"),
                Ok(false) => StepOutput::done("尚未建立 UI：只移除系統匣圖示，圖示不存在"),
                Err(_) => StepOutput::timed_out(format!(
                    "尚未建立 UI：主執行緒 {:.1} 秒內沒有處理系統匣圖示移除（卡住或忙碌），放棄等待",
                    limit.as_secs_f32()
                )),
            },
        }
    }

    fn settle_render_browser(&self, limit: Duration) -> StepOutput {
        use crate::wallpaper_render::{BrowserSettle, WallpaperRenderer};
        let Some(renderer) = self.app.try_state::<WallpaperRenderer>() else {
            return StepOutput::not_applicable("沒有桌布渲染器");
        };
        // 沿用渲染處置函式（等、仍在就核對身分後結束它），但時間切在本步分到的 `limit` 內（`shutdown_within`：
        // 等 `limit − 結束後確認`，再 `TerminateProcess` 並確認），不會超過預算；外層另有工作執行緒逾時保險。
        match renderer.shutdown_within(limit) {
            BrowserSettle::None => StepOutput::done("沒有殘留的渲染 browser 行程"),
            BrowserSettle::Exited { pid, .. } => StepOutput::done(format!("PID {pid} 已結束")),
            BrowserSettle::Terminated { pid, gone: true } => {
                StepOutput::done(format!("PID {pid} 逾時後由宿主結束"))
            }
            BrowserSettle::Terminated { pid, gone: false } => StepOutput::failed(format!(
                "PID {pid} 已要求結束但仍未退出（可能卡在核心拆除）"
            )),
            BrowserSettle::TerminateRequested { pid } => {
                StepOutput::done(format!("PID {pid} 已要求結束（不等待確認）"))
            }
            BrowserSettle::Left { pid } => {
                StepOutput::failed(format!("PID {pid} 仍在且核對不符或結束失敗，未處置"))
            }
        }
    }

    fn finalize(&self, summary: &str, _limit: Duration) -> StepOutput {
        super::clear_startup_marker();
        log::info!(target: LOG_TARGET, "{summary}");
        log::logger().flush();
        StepOutput::done("已刪當機標記並 flush 記錄檔")
    }
}

/// 在主執行緒上：藏起並移除系統匣圖示（`NIM_DELETE`；`exit(0)` 不會替我們做）。回傳圖示原本是否存在。
/// `Shell_NotifyIconW` 是送 explorer 的同步呼叫，explorer 卡住時會停在這裡——呼叫端都有逾時。
fn remove_tray_icon(app: &AppHandle) -> bool {
    if let Some(tray) = app.tray_by_id(crate::tray::TRAY_ID) {
        let _ = tray.set_visible(false);
    }
    app.remove_tray_by_id(crate::tray::TRAY_ID).is_some()
}

// ---------------------------------------------------------------------------------------------
// 失敗退路：以 `--autostart --wait-exit <pid>` 重新啟動自己
// ---------------------------------------------------------------------------------------------

/// `install()` 在 `on_before_exit` 之後失敗、舊行程結束時的結束碼（非 0：安裝失敗）。
pub const FAILED_INSTALL_EXIT_CODE: i32 = 1;

/// 重新啟動用的命令：`<exe> --autostart --wait-exit <old_pid>`，stdin／stdout／stderr 一律 `Stdio::null()`
/// （不繼承呼叫端管線，避免啟動它的工具卡到子行程結束；見 memory `start-process-redirect-inherits-caller-pipe`）。
pub fn relaunch_command(exe: &Path, old_pid: u32) -> Command {
    let mut cmd = Command::new(exe);
    cmd.arg("--autostart")
        .arg("--wait-exit")
        .arg(old_pid.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd
}

/// [`relaunch_with`] 的結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelaunchOutcome {
    /// 已啟動新行程（PID）。
    Spawned(u32),
    /// 取不到自己的 exe 路徑。
    NoExePath(String),
    /// 啟動失敗。
    SpawnFailed(String),
}

/// 可測的本體：`exe`＝目前 exe 路徑（取得失敗帶錯誤字串），`spawn` 負責真正啟動並回新 PID。
pub fn relaunch_with(
    exe: Result<PathBuf, String>,
    old_pid: u32,
    spawn: impl FnOnce(Command) -> io::Result<u32>,
) -> RelaunchOutcome {
    let exe = match exe {
        Ok(exe) => exe,
        Err(e) => return RelaunchOutcome::NoExePath(e),
    };
    match spawn(relaunch_command(&exe, old_pid)) {
        Ok(pid) => RelaunchOutcome::Spawned(pid),
        Err(e) => RelaunchOutcome::SpawnFailed(e.to_string()),
    }
}

/// 命令列顯示用（記錄）。
fn describe(cmd: &Command) -> String {
    let args: Vec<OsString> = cmd.get_args().map(OsString::from).collect();
    format!("{:?} {:?}", cmd.get_program(), args)
}

/// `install()` 在 `on_before_exit` 之後失敗時呼叫：以 `--autostart --wait-exit <本 PID>` 啟動一個新的自己、記錄
/// 失敗原因與結果，然後**立刻** `std::process::exit`（不用 `app.exit`，D4）。不回傳。
///
/// 注意：舊行程結束前新行程會卡在 `--wait-exit`（上限 30 秒），所以這裡一定要立刻結束，不要再做任何等待。
pub fn relaunch_after_failed_install(install_error: &str) -> ! {
    log::error!(
        target: LOG_TARGET,
        "安裝檔啟動失敗（{install_error}）：收尾已做完、沒有小工具也沒有桌布協調，以 --autostart --wait-exit 重新啟動自己後結束"
    );
    let outcome = relaunch_with(
        std::env::current_exe().map_err(|e| e.to_string()),
        std::process::id(),
        |mut cmd| {
            log::info!(target: LOG_TARGET, "重新啟動命令：{}", describe(&cmd));
            cmd.spawn().map(|child| child.id())
        },
    );
    match &outcome {
        RelaunchOutcome::Spawned(pid) => {
            log::info!(target: LOG_TARGET, "已啟動新行程 PID {pid}，舊行程立即結束（結束碼 {FAILED_INSTALL_EXIT_CODE}）")
        }
        RelaunchOutcome::NoExePath(e) => log::error!(
            target: LOG_TARGET,
            "取不到自己的 exe 路徑（{e}），無法重新啟動；宿主要等下次登入才會回來"
        ),
        RelaunchOutcome::SpawnFailed(e) => log::error!(
            target: LOG_TARGET,
            "重新啟動失敗（{e}）；宿主要等下次登入才會回來"
        ),
    }
    log::logger().flush();
    std::process::exit(FAILED_INSTALL_EXIT_CODE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// 假步驟：記錄呼叫順序，各步可指定睡眠時間與結果。
    #[derive(Default)]
    struct Fake {
        calls: Mutex<Vec<&'static str>>,
        delays: Mutex<std::collections::HashMap<&'static str, Duration>>,
        summaries: Mutex<Vec<String>>,
        limits: Mutex<std::collections::HashMap<&'static str, Duration>>,
        panic_in: Mutex<Option<&'static str>>,
    }

    impl Fake {
        fn delay(&self, name: &'static str, d: Duration) {
            self.delays.lock().unwrap().insert(name, d);
        }
        fn enter(&self, name: &'static str, limit: Option<Duration>) {
            self.calls.lock().unwrap().push(name);
            if let Some(l) = limit {
                self.limits.lock().unwrap().insert(name, l);
            }
            let d = self.delays.lock().unwrap().get(name).copied();
            if let Some(d) = d {
                thread::sleep(d);
            }
            if *self.panic_in.lock().unwrap() == Some(name) {
                panic!("測試用 panic：{name}");
            }
        }
        fn calls(&self) -> Vec<&'static str> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl ExitSteps for Fake {
        fn unregister_restart(&self) {
            self.enter("unregister", None);
        }
        fn stop_coordinator(&self, limit: Duration) -> StepOutput {
            self.enter("coordinator", Some(limit));
            StepOutput::done("ok")
        }
        fn stop_fetch(&self, limit: Duration) -> StepOutput {
            self.enter("fetch", Some(limit));
            StepOutput::done("ok")
        }
        fn close_ui(&self, limit: Duration) -> StepOutput {
            self.enter("ui", Some(limit));
            StepOutput::done("ok")
        }
        fn remove_tray(&self, limit: Duration) -> StepOutput {
            self.enter("tray", Some(limit));
            StepOutput::done("系統匣圖示已移除")
        }
        fn settle_render_browser(&self, limit: Duration) -> StepOutput {
            self.enter("browser", Some(limit));
            StepOutput::done("ok")
        }
        fn finalize(&self, summary: &str, limit: Duration) -> StepOutput {
            self.summaries.lock().unwrap().push(summary.to_owned());
            self.enter("finalize", Some(limit));
            StepOutput::done("ok")
        }
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// 縮小十倍的預算，保留 D4 表的比例（限制夠寬，平行測試負載高時「正常完成」的步驟也不會誤判逾時）。
    fn small() -> ExitBudget {
        ExitBudget {
            total: ms(2000),
            coordinator: ms(500),
            fetch: ms(200),
            ui: ms(300),
            browser: ms(500),
            finalize: ms(100),
            grace: ms(50),
        }
    }

    #[test]
    fn default_budget_matches_the_d4_table() {
        let b = ExitBudget::default();
        assert_eq!(b.total, Duration::from_secs(20));
        assert_eq!(b.coordinator, Duration::from_secs(5));
        assert_eq!(b.fetch, Duration::from_secs(2));
        assert_eq!(b.ui, Duration::from_secs(3));
        assert_eq!(b.browser, Duration::from_secs(5));
        assert_eq!(b.finalize, Duration::from_secs(1));
        assert_eq!(b.grace, Duration::from_millis(100));
        // 各步上限加總（不含已刪除的步驟 2）：16 秒，在 20 秒總預算內，且不超過 D4 原表的 18 秒。
        let sum = b.coordinator + b.fetch + b.ui + b.browser + b.finalize;
        assert_eq!(sum, Duration::from_secs(16));
        assert!(sum <= b.total);
    }

    #[test]
    fn step_numbers_follow_the_design_table_without_step_two() {
        let order = [
            ExitStep::UnregisterRestart,
            ExitStep::Coordinator,
            ExitStep::StopFetch,
            ExitStep::CloseUi,
            ExitStep::SettleBrowser,
            ExitStep::Finalize,
        ];
        let numbers: Vec<u8> = order.iter().map(|s| s.number()).collect();
        assert_eq!(numbers, vec![1, 3, 4, 5, 6, 7]);
    }

    /// 全部正常：依表順序執行、每步都有耗時報告、步驟 7 收到涵蓋 1–6 的摘要。
    #[test]
    fn runs_all_steps_in_table_order_and_reports_each() {
        let fake = Arc::new(Fake::default());
        let report = run_exit_for_update(&fake, true, small());
        assert_eq!(
            fake.calls(),
            vec![
                "unregister",
                "coordinator",
                "fetch",
                "ui",
                "browser",
                "finalize"
            ]
        );
        let order: Vec<ExitStep> = report.steps.iter().map(|s| s.step).collect();
        assert_eq!(
            order,
            vec![
                ExitStep::UnregisterRestart,
                ExitStep::Coordinator,
                ExitStep::StopFetch,
                ExitStep::CloseUi,
                ExitStep::SettleBrowser,
                ExitStep::Finalize
            ]
        );
        assert!(report.steps.iter().all(|s| s.status == StepStatus::Done));
        assert!(report.within_budget());
        assert!(report.timed_out_steps().is_empty());
        let summaries = fake.summaries.lock().unwrap().clone();
        assert_eq!(summaries.len(), 1);
        for needle in [
            "因更新結束",
            "1解除重新啟動註冊",
            "3協調迴圈結束",
            "6渲染 browser 行程",
        ] {
            assert!(summaries[0].contains(needle), "{needle}：{}", summaries[0]);
        }
        assert!(report.summary().contains("7刪標記與記錄"));
    }

    /// 尚未建立 UI（過渡期同步安裝）：略過 3、6，且完全不呼叫它們；步驟 5 不呼叫 `close_ui`，改呼叫
    /// `remove_tray`（最終審查 M1：系統匣圖示在過渡期也在，不移除會留下殘影），上限同步驟 5；1、4、7 照做。
    /// 步驟 5 的結果寫進步驟 7 記下的摘要。
    #[test]
    fn without_ui_steps_3_6_are_skipped_and_step_5_only_removes_the_tray_icon() {
        let fake = Arc::new(Fake::default());
        let budget = small();
        let report = run_exit_for_update(&fake, false, budget);
        assert_eq!(
            fake.calls(),
            vec!["unregister", "fetch", "tray", "finalize"],
            "步驟計畫：1、4、5（只移除系統匣圖示）、7"
        );
        for step in [ExitStep::Coordinator, ExitStep::SettleBrowser] {
            let r = report.step(step).expect("有報告");
            assert_eq!(r.status, StepStatus::NotApplicable, "{step:?}");
            assert_eq!(r.elapsed, Duration::ZERO);
        }
        let tray = report.step(ExitStep::CloseUi).expect("步驟 5 有報告");
        assert_eq!(tray.status, StepStatus::Done);
        assert_eq!(
            fake.limits.lock().unwrap().get("tray").copied(),
            Some(budget.ui),
            "系統匣移除的上限比照步驟 5"
        );
        assert!(!report.ui_built);
        let summaries = fake.summaries.lock().unwrap().clone();
        assert_eq!(summaries.len(), 1);
        for needle in [
            "尚未建立 UI",
            "5關閉視窗與系統匣圖示=完成",
            "系統匣圖示已移除",
        ] {
            assert!(summaries[0].contains(needle), "{needle}：{}", summaries[0]);
        }
    }

    /// 尚未建立 UI 時系統匣移除卡住（主執行緒忙碌、explorer 卡住）：在步驟 5 的上限內放棄、記為逾時，步驟 7 照做，
    /// 整段在總預算內。
    #[test]
    fn without_ui_a_hanging_tray_removal_is_abandoned_within_the_step_limit() {
        let fake = Arc::new(Fake::default());
        fake.delay("tray", Duration::from_secs(5));
        let budget = small();
        let report = run_exit_for_update(&fake, false, budget);
        let tray = report.step(ExitStep::CloseUi).unwrap();
        assert_eq!(tray.status, StepStatus::TimedOut, "{tray:?}");
        assert!(
            tray.elapsed >= budget.ui && tray.elapsed < budget.ui + ms(1000),
            "{:?}",
            tray.elapsed
        );
        assert_eq!(
            report.step(ExitStep::Finalize).unwrap().status,
            StepStatus::Done
        );
        assert!(report.within_budget(), "{:?}", report.total);
        assert!(report.summary().contains("5關閉視窗與系統匣圖示=逾時"));
    }

    /// 可跳過的步驟逾時就放棄、繼續下一步；必做的步驟 1、7 仍執行；整段不超過總預算。
    #[test]
    fn a_hanging_skippable_step_is_abandoned_and_later_steps_still_run() {
        let fake = Arc::new(Fake::default());
        fake.delay("coordinator", Duration::from_secs(5)); // 卡住（上限 500 ms）
        let started = Instant::now();
        let report = run_exit_for_update(&fake, true, small());
        let elapsed = started.elapsed();
        assert!(elapsed < ms(2000), "不得等卡住的步驟：{elapsed:?}");
        let c = report.step(ExitStep::Coordinator).unwrap();
        assert_eq!(c.status, StepStatus::TimedOut, "{c:?}");
        assert!(
            c.elapsed >= ms(500) && c.elapsed < ms(1500),
            "{:?}",
            c.elapsed
        );
        assert_eq!(report.timed_out_steps(), vec![ExitStep::Coordinator]);
        for step in [
            ExitStep::UnregisterRestart,
            ExitStep::StopFetch,
            ExitStep::CloseUi,
            ExitStep::SettleBrowser,
            ExitStep::Finalize,
        ] {
            assert_eq!(
                report.step(step).unwrap().status,
                StepStatus::Done,
                "{step:?}"
            );
        }
        assert!(report.within_budget(), "{:?}", report.total);
    }

    /// 每一個可跳過步驟都卡住：整段仍在總預算內，步驟 7 照做（必做＋保留時間）。
    #[test]
    fn all_skippable_steps_hanging_still_finishes_within_the_total_budget() {
        let fake = Arc::new(Fake::default());
        for name in ["coordinator", "fetch", "ui", "browser"] {
            fake.delay(name, Duration::from_secs(5));
        }
        let report = run_exit_for_update(&fake, true, small());
        assert_eq!(
            report.timed_out_steps(),
            vec![
                ExitStep::Coordinator,
                ExitStep::StopFetch,
                ExitStep::CloseUi,
                ExitStep::SettleBrowser
            ]
        );
        assert_eq!(
            report.step(ExitStep::Finalize).unwrap().status,
            StepStatus::Done
        );
        // 總預算扣掉保留與寬限後，最壞情況也不超過總預算。
        assert!(
            report.within_budget(),
            "{:?} > {:?}",
            report.total,
            small().total
        );
    }

    /// 總預算用盡：後面的可跳過步驟只拿到剩餘時間；剩餘為 0 就略過（記為逾時、不呼叫）；步驟 7 照做且整段不超過總預算。
    #[test]
    fn total_budget_caps_later_steps_and_reserves_time_for_the_final_step() {
        let fake = Arc::new(Fake::default());
        let budget = ExitBudget {
            total: ms(1000),
            coordinator: ms(800),
            fetch: ms(800),
            ui: ms(800),
            browser: ms(800),
            finalize: ms(100),
            grace: ms(50),
        };
        fake.delay("coordinator", ms(700)); // 完成（上限 800 ms），但已吃掉大半預算
        fake.delay("fetch", ms(400)); // 只剩 ≤100 ms 可用 → 逾時
        let report = run_exit_for_update(&fake, true, budget);
        if let Some(l) = fake.limits.lock().unwrap().get("fetch") {
            assert!(*l <= ms(100), "步驟 4 只剩總預算的餘額：{l:?}");
        }
        let budget_skipped = report
            .steps
            .iter()
            .filter(|s| s.detail == "總預算已用盡，略過")
            .count();
        assert!(budget_skipped >= 1, "{:#?}", report.steps);
        for step in &report.steps {
            if step.detail == "總預算已用盡，略過" {
                assert_eq!(step.status, StepStatus::TimedOut);
                assert_eq!(step.elapsed, Duration::ZERO);
            }
        }
        assert_eq!(fake.limits.lock().unwrap().get("finalize"), Some(&ms(100)));
        assert_eq!(
            report.step(ExitStep::Finalize).unwrap().status,
            StepStatus::Done
        );
        assert!(
            report.within_budget(),
            "{:?} > {:?}",
            report.total,
            budget.total
        );
    }

    /// 步驟 panic 被隔離：記為失敗、後面照做。
    #[test]
    fn a_panicking_step_is_isolated() {
        let fake = Arc::new(Fake::default());
        *fake.panic_in.lock().unwrap() = Some("fetch");
        // 上限放很寬：panic 本身會很快回報，但測試行程裡別的測試可能裝了會抓 backtrace 的 panic hook，
        // 第一次 panic 的符號解析可能慢到超過一般上限。
        let budget = ExitBudget {
            total: Duration::from_secs(120),
            fetch: Duration::from_secs(60),
            ..small()
        };
        let report = run_exit_for_update(&fake, true, budget);
        assert_eq!(
            report.step(ExitStep::StopFetch).unwrap().status,
            StepStatus::Failed
        );
        assert_eq!(
            report.step(ExitStep::CloseUi).unwrap().status,
            StepStatus::Done
        );
        assert_eq!(
            report.step(ExitStep::Finalize).unwrap().status,
            StepStatus::Done
        );
    }

    /// 各步拿到的上限＝表上的上限（總預算充裕時）。
    #[test]
    fn steps_receive_their_table_limits_when_budget_is_ample() {
        let fake = Arc::new(Fake::default());
        let budget = ExitBudget {
            total: Duration::from_secs(20),
            ..small()
        };
        run_exit_for_update(&fake, true, budget);
        let limits = fake.limits.lock().unwrap().clone();
        assert_eq!(limits["coordinator"], budget.coordinator);
        assert_eq!(limits["fetch"], budget.fetch);
        assert_eq!(limits["ui"], budget.ui);
        assert_eq!(limits["browser"], budget.browser);
        assert_eq!(limits["finalize"], budget.finalize);
    }

    #[test]
    fn main_thread_detection_only_matches_the_marked_thread() {
        let here = thread::current().id();
        let other = thread::spawn(|| thread::current().id()).join().unwrap();
        assert!(is_marked_main_thread(Some(here), here));
        assert!(!is_marked_main_thread(Some(here), other));
        assert!(
            !is_marked_main_thread(None, here),
            "未標記時不阻擋（測試環境）"
        );
    }

    /// 「因更新結束」不得走系統匣「結束」的還原與存主題路徑：收尾程式碼（不含註解與測試）不引用任何那條路徑的函式。
    /// 行為面的斷言在 `wallpaper_coordinator::tests::exit_for_update_*`（狀態檔位元組、主題、桌布 API 呼叫都不變）。
    #[test]
    fn exit_for_update_code_never_references_the_tray_quit_restore_or_theme_save_paths() {
        let src = include_str!("exit.rs").replace("\r\n", "\n");
        let production = src.split("#[cfg(test)]").next().unwrap();
        let code: String = production
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        for forbidden in [
            "ensure_wallpaper_theme_none_saved",
            "set_wallpaper_theme_none",
            "resave_wallpaper_theme_none",
            "exit_and_restore",
            "ExitAndRestore",
            "restore_now",
            "tray::quit",
            "app.exit(",
            "ExitLimits",
        ] {
            assert!(!code.contains(forbidden), "收尾程式碼不得引用 {forbidden}");
        }
        // 對照：它確實走「因更新結束」的要求。
        assert!(code.contains("exit_for_update"));
    }

    // ── 「正在因更新結束」旗標（審查 I1）──────────────────────────────────────────────

    /// 旗標預設為假：一般狀態下系統匣「結束」照 `normal` 的處置（各執行緒的測試旗標彼此獨立）。
    #[test]
    fn quit_gate_delegates_when_not_exiting() {
        use super::super::QuitDisposition as D;
        assert!(!is_exiting_for_update());
        let mut called = false;
        let d = quit_gate(|| {
            called = true;
            D::Normal
        });
        assert!(called);
        assert_eq!(d, D::Normal);
    }

    /// 旗標設定後按系統匣「結束」：處置為 `IgnoreInstalling`（`tray::quit_action` 對應 `Ignore`，`quit()` 在那裡就
    /// 返回——不還原桌布、不存主題、不 `app.exit`），而且連 `normal` 都不會被問。
    #[test]
    fn quit_gate_ignores_the_tray_quit_once_exiting_for_update() {
        use super::super::QuitDisposition as D;
        begin_exiting_for_update();
        assert!(is_exiting_for_update());
        let d = quit_gate(|| panic!("正在因更新結束時不得再問一般處置"));
        assert_eq!(d, D::IgnoreInstalling);
        begin_exiting_for_update(); // 冪等
        assert!(is_exiting_for_update());
    }

    /// 複審 N-3：正式的 `remove_tray`（需要 `AppHandle`，單元測試只驗得到假物件）以原始碼斷言：本體經
    /// `run_on_main_thread` 呼叫 `remove_tray_icon(&app)`，並以 `recv_timeout(limit)` 在上限內放棄；`close_ui` 也共用
    /// 同一個 helper。實際效果（系統匣只剩一個圖示）由 tasks 4.3 的實機情境驗證。
    #[test]
    fn production_remove_tray_goes_through_the_main_thread_with_a_timeout() {
        let src = include_str!("exit.rs").replace("\r\n", "\n");
        let (production, _) = src
            .split_once("\n#[cfg(test)]\nmod tests {")
            .expect("找得到測試模組起點");
        let body_of = |signature: &str| {
            let start = production
                .find(signature)
                .unwrap_or_else(|| panic!("找不到 {signature}"));
            let rest = &production[start..];
            // 本體到下一個同層 `fn` 為止。
            let end = rest[signature.len()..]
                .find("\n    fn ")
                .map_or(rest.len(), |e| e + signature.len());
            rest[..end].to_owned()
        };
        let remove = body_of("    fn remove_tray(&self, limit: Duration) -> StepOutput {");
        let main = remove
            .find("run_on_main_thread(")
            .expect("remove_tray 要經 run_on_main_thread");
        let helper = remove
            .find("remove_tray_icon(&app)")
            .expect("remove_tray 要呼叫 remove_tray_icon");
        assert!(main < helper, "remove_tray_icon 在投遞到主執行緒的閉包內");
        assert!(remove.contains("recv_timeout(limit)"), "{remove}");
        let close = body_of("    fn close_ui(&self, limit: Duration) -> StepOutput {");
        assert!(close.contains("remove_tray_icon(&app)"), "{close}");
        let helper_fn = body_of("fn remove_tray_icon(app: &AppHandle) -> bool {");
        assert!(helper_fn.contains("remove_tray_by_id(crate::tray::TRAY_ID)"));
    }

    /// widget-adaptive-zoom-and-grid task 5.2 修正第 1 輪：編輯版面的格線疊加視窗是原生視窗、不在
    /// `webview_windows()` 裡，`close_ui` 要在投遞到主執行緒的閉包內（格線集合在主執行緒的 thread-local）
    /// 經 `widgets` 的公開函式一併清掉；`updater` 不直接碰 `desktop::grid_overlay`。
    #[test]
    fn close_ui_clears_grid_overlays_on_the_main_thread_through_widgets() {
        let src = include_str!("exit.rs").replace("\r\n", "\n");
        let (production, _) = src
            .split_once("\n#[cfg(test)]\nmod tests {")
            .expect("找得到測試模組起點");
        let signature = "    fn close_ui(&self, limit: Duration) -> StepOutput {";
        let start = production.find(signature).expect("找得到 close_ui");
        let rest = &production[start..];
        let end = rest[signature.len()..]
            .find("\n    fn ")
            .map_or(rest.len(), |e| e + signature.len());
        let close = &rest[..end];
        let main = close
            .find("run_on_main_thread(")
            .expect("close_ui 經 run_on_main_thread");
        let clear = close
            .find("crate::widgets::clear_grid_overlays()")
            .expect("close_ui 要清掉格線");
        assert!(main < clear, "清除格線要在投遞到主執行緒的閉包內");
        assert!(
            !production.contains("grid_overlay::"),
            "updater 不得直接碰 desktop::grid_overlay"
        );
    }

    // ── 結束擁有者（審查 M1）：兩個順序都只有一方成功 ─────────────────────────────────

    #[test]
    fn exit_owner_tray_first_blocks_the_update_claim() {
        let o = ExitOwner::new();
        assert!(o.try_claim(ExitOwnerKind::Tray), "系統匣先搶到");
        assert!(
            !o.try_claim(ExitOwnerKind::Update),
            "更新搶輸＝中止安裝，不收尾、不啟動安裝檔"
        );
        assert!(o.is_owned_by(ExitOwnerKind::Tray));
    }

    #[test]
    fn exit_owner_update_first_blocks_the_tray_claim() {
        // 另一個方向（掛鉤先搶、`quit` 後到）：桌布不得被還原。
        let o = ExitOwner::new();
        assert!(o.try_claim(ExitOwnerKind::Update));
        assert!(!o.try_claim(ExitOwnerKind::Tray), "系統匣「結束」被忽略");
    }

    #[test]
    fn exit_owner_same_party_may_claim_again_and_release_hands_it_back() {
        let o = ExitOwner::new();
        assert!(o.try_claim(ExitOwnerKind::Tray));
        assert!(o.try_claim(ExitOwnerKind::Tray), "系統匣「結束」可被連按");
        // 不是擁有者的歸還無效。
        o.release(ExitOwnerKind::Update);
        assert!(o.is_owned_by(ExitOwnerKind::Tray));
        o.release(ExitOwnerKind::Tray);
        assert!(o.try_claim(ExitOwnerKind::Update), "歸還後另一方可以搶");
    }

    #[test]
    fn exit_owner_race_has_exactly_one_winner() {
        for _ in 0..200 {
            let o = Arc::new(ExitOwner::new());
            let (a, b) = (Arc::clone(&o), Arc::clone(&o));
            let t = thread::spawn(move || a.try_claim(ExitOwnerKind::Tray));
            let u = thread::spawn(move || b.try_claim(ExitOwnerKind::Update));
            let (tray, update) = (t.join().unwrap(), u.join().unwrap());
            assert!(tray ^ update, "恰好一方成功：tray={tray} update={update}");
        }
    }

    #[test]
    fn global_owner_helpers_are_wired_to_the_tray_side() {
        // 不碰全域值（其他測試並行）：只斷言對外函式存在且語意為「系統匣」那一側。
        // 只掃正式碼（測試模組之前）：整檔 `contains` 會比對到下面這兩行斷言字串本身而永遠成立（3.2 複審 N2）。
        // 正式碼前面還有零星的 `#[cfg(test)]` 單項，所以以「測試模組」的起點為界，不是第一個 `#[cfg(test)]`。
        let src = include_str!("exit.rs").replace("\r\n", "\n");
        let (production, _) = src
            .split_once("\n#[cfg(test)]\nmod tests {")
            .expect("找得到測試模組起點");
        assert!(production.contains("EXIT_OWNER.try_claim(ExitOwnerKind::Tray)"));
        assert!(production.contains("EXIT_OWNER.is_owned_by(ExitOwnerKind::Tray)"));
    }

    /// 入口：不在主執行緒才設旗標；主執行緒呼叫回 Err、不設旗標（什麼都不做）。
    #[test]
    fn entering_the_exiting_state_sets_the_flag_except_on_the_main_thread() {
        let here = thread::current().id();
        let other = thread::spawn(|| thread::current().id()).join().unwrap();
        assert_eq!(
            enter_exiting_state(Some(here), here),
            Err(ExitError::OnMainThread)
        );
        assert!(!is_exiting_for_update(), "主執行緒呼叫不得設旗標");
        assert_eq!(enter_exiting_state(Some(other), here), Ok(()));
        assert!(is_exiting_for_update());
    }

    /// 取原始碼中某個函式簽章起算的前 `lines` 行（統一換行）。
    fn head_of(src: &str, signature: &str, lines: usize) -> String {
        let src = src.replace("\r\n", "\n");
        let at = src
            .find(signature)
            .unwrap_or_else(|| panic!("找不到 {signature}"));
        src[at..].lines().take(lines).collect::<Vec<_>>().join("\n")
    }

    /// 旗標為真時，會「建立／重建視窗」或「存主題」的入口一律在開頭直接返回（這些函式需要 `AppHandle`，無法在單元測試裡
    /// 實際呼叫，改以原始碼斷言每個入口的開頭都檢查旗標；行為面由 `quit_gate`／`enter_exiting_state` 測試與 3.2 的 e2e 涵蓋）。
    #[test]
    fn window_building_and_theme_saving_entry_points_check_the_flag_first() {
        let widgets = include_str!("../widgets.rs");
        let tray = include_str!("../tray.rs");
        let recovery = include_str!("../recovery.rs");
        let updater = include_str!("../updater.rs");
        let cases: [(&str, &str, usize); 8] = [
            (widgets, "pub fn sync_widget_windows(app: &AppHandle) {", 8),
            (widgets, "pub fn rebuild_widget_windows(", 8),
            (tray, "pub fn open_settings_window(app: &AppHandle) {", 8),
            (tray, "fn save_theme_none_on_exit(app: &AppHandle) {", 8),
            (recovery, "pub fn on_webview_failure(", 14),
            (recovery, "pub fn request_window_rebuild(", 6),
            (recovery, "fn spawn_window_rebuild(", 6),
            (recovery, "fn request_rebuild_all(", 6),
        ];
        for (src, signature, lines) in cases {
            let head = head_of(src, signature, lines);
            assert!(
                head.contains("is_exiting_for_update()"),
                "{signature} 開頭沒有檢查旗標：\n{head}"
            );
            assert!(head.contains("return"), "{signature} 旗標為真時要直接返回");
        }
        // 系統匣「結束」的處置一律先過閘門。
        let head = head_of(updater, "pub fn on_tray_quit(app: &AppHandle)", 8);
        assert!(head.contains("exit::quit_gate"), "{head}");
        // `quit()` 對 `Ignore` 在任何還原／存主題動作之前就返回。
        let quit = head_of(tray, "pub(crate) fn quit(app: &AppHandle) {", 30);
        let ignore = quit.find("QuitAction::Ignore").expect("quit 處理 Ignore");
        let restore = quit
            .find("unregister_restart()")
            .expect("quit 之後呼叫 unregister_restart");
        assert!(
            ignore < restore,
            "Ignore 必須在 unregister_restart 之前返回"
        );
    }

    // ── 啟動閘門（4.x 實機發現 A3：收尾與建立 UI 的競態） ──────────────────────────────────

    use std::sync::atomic::AtomicUsize;

    /// 假「元件」：模擬 `spawn_for_app`——先取許可、啟動（可在中途暫停）、登記，才放掉許可。
    /// `registered` 是「managed state 已登記」。
    fn fake_start(
        gate: &StartGate,
        registered: &AtomicBool,
        inside: Option<(&mpsc::Sender<()>, &mpsc::Receiver<()>)>,
    ) -> bool {
        let Some(_permit) = gate.try_begin() else {
            return false;
        };
        if let Some((entered, resume)) = inside {
            entered.send(()).unwrap();
            resume.recv().unwrap();
        }
        registered.store(true, Ordering::SeqCst);
        true
    }

    /// 假「停止」：對應 `AppExitSteps::stop_coordinator`——等啟動完成後，已登記就停止、否則不適用。
    fn fake_stop(gate: &StartGate, registered: &AtomicBool, limit: Duration) -> StepOutput {
        stop_after_start(gate, limit, "假元件", |_| {
            if registered.load(Ordering::SeqCst) {
                StepOutput::done("已停止")
            } else {
                StepOutput::not_applicable("沒有假元件")
            }
        })
    }

    /// 情境一「收尾早於啟動」：收尾先關閘門，之後的啟動被拒絕、永遠不會登記；步驟判為不適用（真的沒有東西要停）。
    #[test]
    fn exit_before_start_refuses_the_start_and_the_step_is_not_applicable() {
        let gate = StartGate::new();
        let registered = AtomicBool::new(false);
        gate.close();
        let out = fake_stop(&gate, &registered, ms(500));
        assert_eq!(out.status, StepStatus::NotApplicable);
        assert!(!fake_start(&gate, &registered, None), "閘門已關不得啟動");
        assert!(
            !registered.load(Ordering::SeqCst),
            "收尾開始後不得再有東西登記"
        );
        assert!(gate.try_begin().is_none());
    }

    /// 情境二「收尾與啟動交錯」：啟動端已取得許可、尚未登記時收尾開始——步驟必須等到登記完成，
    /// 然後看見它並照常停止（而不是判成「沒有」）。
    #[test]
    fn exit_during_start_waits_for_the_registration_then_stops_it() {
        let gate = Arc::new(StartGate::new());
        let registered = Arc::new(AtomicBool::new(false));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let starter = {
            let (gate, registered) = (Arc::clone(&gate), Arc::clone(&registered));
            thread::spawn(move || fake_start(&gate, &registered, Some((&entered_tx, &resume_rx))))
        };
        entered_rx.recv().unwrap(); // 啟動端已持有許可、尚未登記
        gate.close(); // 收尾開始（`begin_exiting_for_update`）
        let stopper = {
            let (gate, registered) = (Arc::clone(&gate), Arc::clone(&registered));
            thread::spawn(move || fake_stop(&gate, &registered, Duration::from_secs(5)))
        };
        // 收尾端確實在等：給它一點時間，確認它沒有搶先判成「不適用」。
        thread::sleep(ms(100));
        assert!(!stopper.is_finished(), "啟動中時，步驟必須等待");
        resume_tx.send(()).unwrap();
        assert!(
            starter.join().unwrap(),
            "許可在閘門關閉前取得，啟動照常完成"
        );
        let out = stopper.join().unwrap();
        assert_eq!(out.status, StepStatus::Done, "{out:?}");
        assert!(registered.load(Ordering::SeqCst));
    }

    /// 啟動端一直不放許可：步驟在自己的時限內放棄並回逾時（不假裝沒事、也不卡死整段收尾）。
    #[test]
    fn a_start_that_never_finishes_makes_the_step_time_out_within_its_limit() {
        let gate = StartGate::new();
        let permit = gate.try_begin().expect("閘門未關");
        gate.close();
        let t = Instant::now();
        let out = stop_after_start(&gate, ms(150), "假元件", |_| {
            panic!("啟動未完成，不得執行停止")
        });
        assert_eq!(out.status, StepStatus::TimedOut, "{out:?}");
        assert!(out.detail.contains("仍在啟動中"), "{}", out.detail);
        assert!(t.elapsed() < ms(2000));
        drop(permit);
        assert!(gate.wait_idle(ms(10)), "許可放掉後恢復閒置");
    }

    /// 等待啟動所花的時間要從該步時限扣掉：交給 `stop` 的是剩餘時間。
    #[test]
    fn the_stop_gets_only_the_remaining_limit_after_waiting_for_a_start() {
        let gate = StartGate::new();
        let permit = gate.try_begin().expect("閘門未關");
        gate.close();
        let seen = AtomicUsize::new(0);
        let out = thread::scope(|scope| {
            scope.spawn(move || {
                thread::sleep(ms(200));
                drop(permit);
            });
            stop_after_start(&gate, Duration::from_secs(5), "假元件", |left| {
                seen.store(usize::try_from(left.as_millis()).unwrap(), Ordering::SeqCst);
                StepOutput::done("ok")
            })
        });
        assert_eq!(out.status, StepStatus::Done);
        let left = seen.load(Ordering::SeqCst);
        assert!(
            left < 5000 && left > 3000,
            "剩餘時間應為上限扣掉等待：{left} ms"
        );
    }

    /// 沒有任何啟動中的元件：步驟不多等、直接用完整時限執行。
    #[test]
    fn with_nothing_starting_the_stop_runs_immediately_with_the_full_limit() {
        let gate = StartGate::new();
        gate.close();
        let out = stop_after_start(&gate, ms(500), "假元件", |left| {
            assert!(left > ms(400), "{left:?}");
            StepOutput::done("ok")
        });
        assert_eq!(out.status, StepStatus::Done);
    }

    /// 多個啟動中的元件：全部放掉才算閒置。
    #[test]
    fn idle_only_after_every_permit_is_released() {
        let gate = StartGate::new();
        let a = gate.try_begin().unwrap();
        let b = gate.try_begin().unwrap();
        assert!(!gate.wait_idle(ms(20)));
        drop(a);
        assert!(!gate.wait_idle(ms(20)), "還有一個許可");
        drop(b);
        assert!(gate.wait_idle(ms(20)));
    }

    /// 正式啟動點都要先過閘門（這些函式需要 `AppHandle`，改以原始碼斷言；時序行為由上面的測試涵蓋）：
    /// 兩個 `spawn_for_app` 在開頭取許可、取不到就返回；`build_ui` 開頭先看旗標；步驟 3／4 都經 `stop_after_start`。
    #[test]
    fn production_start_points_go_through_the_gate() {
        let coordinator = include_str!("../wallpaper_coordinator.rs");
        let scheduler = include_str!("../fetch/scheduler.rs");
        let main = include_str!("../main.rs");
        for (src, signature) in [
            (coordinator, "pub fn spawn_for_app(app: &AppHandle) {"),
            (scheduler, "pub fn spawn_for_app(app: &AppHandle) {"),
        ] {
            let head = head_of(src, signature, 8);
            assert!(head.contains("try_begin_start("), "{signature}：\n{head}");
            assert!(head.contains("return"), "{signature} 取不到許可要返回");
        }
        let head = head_of(main, "fn build_ui(handle: &tauri::AppHandle", 18);
        assert!(head.contains("is_exiting_for_update()"), "{head}");
        assert!(head.contains("return"), "{head}");
        let src = include_str!("exit.rs").replace("\r\n", "\n");
        let production = &src[..src.find("#[cfg(test)]\nmod tests").expect("測試模組起點")];
        assert_eq!(
            production.matches("stop_after_start(&START_GATE").count(),
            2,
            "步驟 3 與 4 都要先等啟動中的元件"
        );
        assert!(
            production.contains("START_GATE.close();"),
            "begin_exiting_for_update 要關閘門"
        );
    }

    /// 全域旗標與閘門連動：`begin_exiting_for_update` 之後全域閘門就拒絕啟動。
    /// （全域閘門一旦關上不會復原；本行程內其他測試都用自己的 `StartGate` 實例，不碰全域，所以不互相污染。）
    #[test]
    fn begin_exiting_closes_the_global_gate() {
        begin_exiting_for_update();
        assert!(try_begin_start("測試元件").is_none());
    }

    /// 審查 M2：視窗工廠 `create_widget_window` 在建立前與 `build()` 回來後各檢查一次旗標，後者為真就在顯示前
    /// 銷毀剛建好的視窗（故障重建在工作執行緒上通過入口檢查之後才開始收尾的殘餘時間窗）。需要 `AppHandle`，
    /// 以原始碼斷言；只掃 `widgets.rs` 的正式碼（切在測試模組之前），不會比對到測試字串本身。
    #[test]
    fn widget_window_factory_rechecks_the_flag_after_build_and_destroys_the_window() {
        let src = include_str!("../widgets.rs").replace("\r\n", "\n");
        let production = &src[..src
            .find("#[cfg(test)]\nmod tests")
            .expect("widgets.rs 測試模組起點")];
        let start = production
            .find("pub fn create_widget_window(")
            .expect("找得到視窗工廠");
        let body = &production[start..];
        let build = body.find(".build()").expect("工廠呼叫 build()");
        let flag = "crate::updater::is_exiting_for_update()";
        let before = body[..build].find(flag).expect("建立前要檢查旗標");
        let after_rel = body[build..].find(flag).expect("build() 之後要再檢查旗標");
        let after = build + after_rel;
        assert!(before < build && build < after);
        // 第二次檢查之後緊接著銷毀視窗並回 Err（在登記任何狀態、顯示視窗之前）。
        let tail: String = body[after..].lines().take(6).collect::<Vec<_>>().join("\n");
        assert!(tail.contains("window.destroy()"), "{tail}");
        assert!(tail.contains("return Err("), "{tail}");
        assert!(
            body[after..].find("claim_label_for_new_window").unwrap() > tail.len(),
            "銷毀要早於登記標記"
        );
    }

    // ── 重新啟動 ────────────────────────────────────────────────────────────────────────

    #[test]
    fn relaunch_command_is_autostart_wait_exit_with_the_old_pid() {
        let cmd = relaunch_command(Path::new(r"C:\fc\fc-host.exe"), 4321);
        assert_eq!(cmd.get_program(), r"C:\fc\fc-host.exe");
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args, vec!["--autostart", "--wait-exit", "4321"]);
    }

    #[test]
    fn relaunch_with_reports_spawn_success_and_failures() {
        let mut seen = None;
        let out = relaunch_with(Ok(PathBuf::from(r"C:\fc\fc-host.exe")), 77, |cmd| {
            seen = Some(describe(&cmd));
            Ok(999)
        });
        assert_eq!(out, RelaunchOutcome::Spawned(999));
        let seen = seen.unwrap();
        assert!(
            seen.contains("--autostart") && seen.contains("--wait-exit") && seen.contains("77"),
            "{seen}"
        );

        let out = relaunch_with(Ok(PathBuf::from("x.exe")), 1, |_| {
            Err(io::Error::new(io::ErrorKind::NotFound, "找不到"))
        });
        assert!(
            matches!(out, RelaunchOutcome::SpawnFailed(ref m) if m.contains("找不到")),
            "{out:?}"
        );

        let mut called = false;
        let out = relaunch_with(Err("沒有路徑".into()), 1, |_| {
            called = true;
            Ok(1)
        });
        assert!(matches!(out, RelaunchOutcome::NoExePath(_)));
        assert!(!called, "沒有 exe 路徑就不得啟動");
    }
}
