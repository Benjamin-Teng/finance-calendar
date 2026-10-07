//! 桌布協調迴圈（dynamic-wallpaper task 4.7a；design.md D3／D5／D6；tasks.md 4.7 的排程核心）。
//!
//! 把 4.1 主題設定、4.2 桌布 COM 服務、4.3 排程器、4.4 接管狀態機、4.5 渲染管線、4.6 資料通道接成
//! 一條**專屬執行緒**上的迴圈：依排程器的 `next_wake` 設**單一**計時器，事件到來就喚醒再評估。
//! 各模組文件「呼叫端契約」就是本模組的規格；4.7a 報告逐條對照。
//!
//! ## 執行緒與鎖
//!
//! - 迴圈跑在 `fc-wallpaper-coord` 執行緒（[`spawn_coordinator`]；任何結束方式——含 panic——都關閉
//!   信箱，之後的還原要求立即斷線，task 6.4，審查 R2a-L2）。它獨佔 [`SchedulerState`]、
//!   [`WallpaperTakeover`]、桌布 COM 服務把手與登錄介面——這些都不需要鎖。
//! - 其他執行緒（主執行緒的事件迴圈、Tauri 指令、資料輪詢執行緒、守門視窗）只經
//!   [`CoordinatorHandle::notify`] 投遞喚醒：取一把只保護佇列的小鎖、推入、`notify_one`，**從不等待**
//!   協調迴圈。狀態查詢（[`CoordinatorHandle::status`]）讀另一把只在發佈時短暫持有的鎖。
//! - 宿主輸入（設定的主題、暫停原因、`tw-events` 的資料日期、主題設定版本）由
//!   [`CoordinatorPorts::inputs`] 短暫上鎖複製出來；**呼叫 `render()` 時不持有任何宿主的鎖**
//!   （`window_sync`、設定、暫停原因、通道註冊表都不持有），渲染要靠主執行緒建立視窗，持鎖會死結。
//!
//! ## 一次 step
//!
//! [`Coordinator::step`]：
//!
//! 1. 處理這次的喚醒（[`Wake`]，去重、保留先後；記一行「喚醒」）：檢查時鐘與時區（倒退或時區改變
//!    就清除排程器的時點紀錄）、主題變更（切到「不接管」→ 還原原桌布）、焦點確認、顯示變更（處理
//!    待還原螢幕、標記要重新列舉與清理）、COM 忙碌解除或 explorer 重新啟動（「設定路徑恢復」）、
//!    主題設定檔變更（清除時點紀錄＝目前的主題重畫）、到期的還原重試。
//! 2. 評估（[`next_action`]）：狀態檔封鎖時不評估、不渲染。主題不是「不接管」時才列舉螢幕（不接管時
//!    完全不碰 COM，維持 desktop-widget-host 原行為）。記一行「決策」（純計時器喚醒且決策沒變時不記，
//!    避免洗版）。
//! 3. `Redraw` 就執行（見下），結束後把執行期間累積在信箱裡的喚醒**合併**成一次，回到第 1 步——這同時
//!    滿足「Redraw 之後立即再評估一次、以那一次的 `next_wake` 為準」與「期間的喚醒只標記、結束後合併
//!    成一次評估」。
//! 4. 回傳下一次喚醒時刻（排程器的 `next_wake` 與還原重試時刻取早）；迴圈外殼據此等待，最長
//!    [`CoordinatorConfig::max_wait`]（兜底偵測時鐘跳動、顯示器變化），COM 忙碌時最長
//!    [`CoordinatorConfig::busy_poll`]（偵測忙碌解除）。
//!
//! ## 執行 `Redraw`
//!
//! 逐台：先查中止條件（宿主結束、設定的主題已不是這份計畫的主題、狀態機已停止、進入暫停或勿打擾——
//! task 6.4，審查 R2b-L3）→ COM 忙碌就記
//! `Busy`、寫一行「預檢跳過」、不渲染 → 渲染（失敗記 `Render`）→ 再查一次中止條件（渲染最長 30 秒，
//! 期間使用者可能切到「不接管」；**已渲染但還沒送出的不再送**）→ 經狀態機設定（`Applied`→`record_drawn`；
//! `SetFailed`／`ReadbackFailed`→`Apply`，錯誤是 `Busy` 時記 `Busy`；`Refused`→`Apply`；`Yielded`→
//! 套用事件並中止；`BackupFailed`→套用事件、已接管時排「不接管」還原並中止；等焦點確認→其餘各台記
//! `Apply` 並中止）。輸出圖寫 a／b 哪一格，以**讀回確認**目前顯示的路徑為準（第一台要渲染時讀一次；
//! task 6.4，審查 R2a-L1——狀態檔的 `last_set` 是「準備設定」的路徑，設定失敗時螢幕仍是另一格），讀不到
//! 才退回 `last_set`。中止的螢幕不記錄，解除後的評估照既有規則補畫。全部記錄完之後，若有 `Applied`，等
//! [`CoordinatorConfig::confirm_delay`]（約 1 秒）讀回一次並逐台 `confirm_applied`。最後
//! `redraw_finished`，有排隊的還原就立刻執行。
//!
//! 還原（含切到「不接管」）一律在 `Redraw` 結束之後才執行：它是信箱裡的事件，`Redraw` 期間只會讓
//! 進行中的計畫在下一個檢查點中止。
//!
//! ## 啟停路徑（4.7b）
//!
//! - **系統匣「結束」**（[`Request::ExitAndRestore`]／[`exit_and_restore`]）：系統匣的結束執行緒投遞
//!   要求後等待；進行中的 `Redraw` 在下一個檢查點中止（[`Inbox::restore_requested`]），迴圈外殼在
//!   `step` 之間先把主題存成「不接管」（task 6.4：一律如此，下次啟動不再接管），再以
//!   [`RestoreReason::TrayExit`] 還原（含全域填滿方式、背景色、登錄，依 4.4 規則），回報後迴圈結束。
//!   上限見 [`ExitLimits`]：逾時照常結束，狀態檔保留「還原進行中」標記；系統匣的結束執行緒等完後再強制
//!   存一次「不接管」（`widgets::ensure_wallpaper_theme_none_saved`），迴圈沒在時限內處理要求時也一樣。
//! - **工作階段結束**（[`Wake::SessionEnding`]／[`Wake::SessionEndCancelled`]）：不還原、不呼叫桌布
//!   API、不渲染，狀態維持原樣（狀態檔每次變更都已同步寫入，沒有延後的落盤）；取消時恢復排程。
//! - **因更新結束**（[`Request::ExitForUpdate`]／[`exit_for_update`]，installer-auto-update task 3.3）：語意同
//!   工作階段結束——不還原、不呼叫桌布 API、不動狀態檔與主題（不寫「還原進行中」標記、不存主題、不重試）——
//!   但處理完讓迴圈結束並回報；進行中的 `Redraw` 同樣在下一個檢查點中止。時限由呼叫端決定（D4 表 5 秒）。
//!   它**不走**系統匣「結束」的還原與存主題路徑（那條由 [`Request::ExitAndRestore`] 與 `tray::quit` 負責）。
//! - **還原中斷續做**：啟動時狀態檔為接管中且有「還原進行中」標記（`wallpaper_state` 的
//!   `restore_in_progress`）→ 先以標記的原因續做還原；完成前不接管、不渲染、不做讓位判定（失敗依
//!   退避重試）；完成後主題改「不接管」並存檔（維持不接管直到使用者重新選主題），續做期間使用者
//!   已改選主題則照常接管。「不接管」的還原沒做完、使用者又選回主題時清除標記（放棄那次還原）。
//! - **當機後重啟**：沒有標記時照常接管並重畫（排程器從空狀態開始），啟動記一行接管狀態與啟動方式
//!   （[`launch_kind`]：`--restarted`／`--autostart`／一般）。
//! - **命令列**：交給執行中的宿主時是 [`Request::CommandLineRestore`]（主題改「不接管」並存檔、以
//!   [`RestoreReason::CommandLine`] 還原、回報，迴圈繼續）；沒有宿主時 `wallpaper_cli` 在本行程呼叫
//!   [`restore_for_command_line`]。結果碼與結束碼見 [`RestoreCode`]。
//! - **「主題改不接管」的存檔重試**（task 6.4，審查 R2b-L1）：讓位、安全閥、焦點取消、備份失敗、續做完成、
//!   命令列、系統匣結束把主題改「不接管」時存檔失敗（記憶體已改），依 `retry_delay` 以
//!   [`CoordinatorPorts::resave_theme_none`] 重試到成功；使用者這段期間選回某個主題就停止。重試時刻併入
//!   下一次喚醒。
//! - **還原重試限頻**：設定路徑恢復（COM 忙碌解除、`ComRecovered`、explorer 重新啟動）觸發的還原
//!   重試，每個退避窗至多一次；恢復觸發的重試失敗後新的退避窗延續「額度已用掉」，只有退避時刻到的
//!   重試才開啟新的額度（比照 4.3 M1）。
//!
//! ## explorer 資源安全閥（4.9；design.md D11）
//!
//! - **讀取時點**：評估結果是 `Redraw`、而且確定會渲染（不在勿打擾、不在等焦點確認或停止接管）時，
//!   [`CoordinatorPorts::explorer_gdi`] 讀一次（核心查詢、不送訊息給 explorer），以讀值再評估一次；
//!   主題為「不接管」、暫停、不需重畫時都不讀。每次讀值記一行（讀值、基準、門檻）。
//! - **基準**：排程器帶出 `Decision::rebaseline` 時寫進狀態檔（接管開始、PID 改變）；主題從「不接管」
//!   改回某個主題（使用者重新開啟接管）時清除；宿主重啟沿用狀態檔的基準，PID 相同就照舊比較。
//!   接管前讀取失敗時清掉未接管時留下的舊基準，避免它被帶進新的接管。
//! - **觸發**：`Action::StopTakeover` → 記一行 warn（讀值、基準、PID、門檻、決策）→
//!   [`Coordinator::restore_now`]（`RestoreReason::SafetyValve`）：還原原桌布與填滿方式、主題改
//!   「不接管」、通知、排程器 `reset` 都在既有的「不接管」還原路徑內。還原失敗依 `retry_delay`
//!   重試；同一次觸發的事件（主題改「不接管」、通知）只套用一次（重試不再套用）。
//! - **讀取失敗**：照常設定、不停止；同一段連續失敗只記一次 warn，恢復時記一行。
//! - **self-test 注入**（只在 `self-test-ipc` 建置）：見 `explorer_gdi_injection` 子模組。
//!
//! ## 給 4.7c／4.8 的接口
//!
//! - 4.7c（勿打擾，`wallpaper_dnd` 模組）：[`CoordinatorPorts::do_not_disturb`]（正式＝
//!   `wallpaper_dnd::DndMonitor::query`；只在本來要重畫時呼叫，主題為「不接管」時從不呼叫）與
//!   [`Wake::DoNotDisturbChanged`]（輪詢執行緒在兩個來源的合併結果改變時送出）；為真時本應重畫的評估改為
//!   [`CoordinatorState::DoNotDisturb`]、不渲染、不記錄時點，解除後的評估由排程器補畫最近錯過的時點一次。
//! - 4.8（設定視窗，`wallpaper_settings` 模組）：[`CoordinatorHandle::status`]（等待資料中、封鎖、等焦點
//!   確認、續做還原中、通知；每次 step 結束發佈 `taken_over`／`spotlight_confirmed`／
//!   `awaiting_spotlight_confirmation`，後者以狀態機階段為準）。設定視窗在選主題時就先問：確認時以
//!   [`SpotlightPreconfirm`] 登記**那個主題**（**事先確認**），迴圈在看到設定的主題正是它時才套用
//!   （未接管時 `WallpaperTakeover::confirm_spotlight` 也接受；不會落在還沒被換掉的舊主題上，修正輪 1）。
//!   迴圈自己在等確認時，設定視窗對話的答案送 [`Wake::SpotlightConfirmed`]／[`Wake::SpotlightCancelled`]
//!   （後備路徑；另有其他主題的事先確認在等時，確認喚醒不套用到目前這個主題）。迴圈自己
//!   發現要等確認時呼叫一次 [`CoordinatorPorts::spotlight_confirmation_needed`]（系統匣提示開設定）。
//!   讓位與安全閥的 [`CoordinatorPorts::notify_user`] 正式實作以系統匣通知顯示
//!   （`wallpaper_settings::notice_text`；安全閥的 `detail` 含讀值、基準、PID、門檻，`restore` 是那次
//!   還原的結果，未接管時不寫「已還原」）。
//!
//! ## 記錄
//!
//! 每個喚醒、排程決策、桌布 API 呼叫（列舉、經狀態機的設定、確認讀回、還原、待還原處理）與結果都有
//! 一行，target 固定 [`LOG_TARGET`]（宿主記錄檔照常每日輪替；實機驗收時依 target 把每條路徑的行
//! 擷取成各自的證據檔）。

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use log::Level;
use serde_json::Value;
use tauri::{AppHandle, Manager};

use crate::desktop::wallpaper::file_identity::FileIdentity;
use crate::desktop::wallpaper::pathcmp::path_eq;
use crate::desktop::wallpaper::registry::RegistryStore;
use crate::desktop::wallpaper::{MonitorEntry, WallpaperError, WallpaperService};
use crate::desktop::StartupRole;
use crate::layout::PhysicalRect;
use crate::settings;
use crate::settings::WallpaperTheme;
use crate::wallpaper::{
    next_action, retry_delay, Action, CivilDate, DataDates, DataKey, Decision, ExplorerRebaseline,
    ExplorerResources, ExplorerSample, FailureKind, MonitorGeometry, MonitorSnapshot,
    RebaselineCause, RedrawPlan, SafetyValveTrip, SchedulerInput, SchedulerState, UnixSeconds,
    UtcOffsetSource, BUSY_SKIP_WARN_SLOTS, EXPLORER_GDI_ABSOLUTE_LIMIT, EXPLORER_GDI_DELTA_LIMIT,
};
use crate::wallpaper_config::{self, WallpaperConfig};
use crate::wallpaper_render::{
    is_valid_monitor_key, iso_utc, OutputTarget, RenderFailure, RenderFailureReason, RenderRequest,
    RenderSuccess,
};
use crate::wallpaper_state::{
    monitor_key, Phase, RefuseReason, RestoreReason, RestoreResult, SetOutcome, StableDisplay,
    StatePaths, TakeoverConfig, TakeoverEvent, TakeoverIo, TakeoverNotice, TakeoverStatus,
    ThemeNoneCause, WallpaperTakeover,
};
pub use crate::wallpaper_state::{MarkOutcome, RestoreRequestMarker};
use crate::widgets::PauseReason;

/// 記錄的 target（宿主記錄檔每行都帶 target，驗收時據此擷取）。
pub const LOG_TARGET: &str = "fc_host::wallpaper_coordinator";

/// 協調執行緒名稱。
const THREAD_NAME: &str = "fc-wallpaper-coord";

/// 沒有 DPI 資訊時的預設（100%）。
const DEFAULT_DPI: u32 = 96;

// ---------------------------------------------------------------------------------------------
// 喚醒與宿主輸入
// ---------------------------------------------------------------------------------------------

/// 喚醒原因。同一次 step 內重複的原因只算一次（[`dedupe`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Wake {
    /// 協調迴圈剛啟動。
    Startup,
    /// 計時器到點（或等待上限到期）。
    Timer,
    /// 資料通道有更新（`tw-events` 或 `wallpaper`）。
    DataChanged,
    /// 設定變更（`update_settings`）。
    SettingsChanged,
    /// 顯示組態變更（解析度、拔接螢幕、工作區）。
    DisplayChanged,
    /// 縮放比例變更。
    DpiChanged,
    /// 暫停原因變化。
    PauseChanged,
    /// 系統時間或時區變更（`WM_TIMECHANGE`）。
    TimeChanged,
    /// 桌布 COM 執行緒恢復（逾時後忙碌解除；迴圈本身也會以忙碌旗標偵測）。
    ComRecovered,
    /// explorer 重新啟動（`TaskbarCreated`）。
    ExplorerRestarted,
    /// 使用者在焦點提示中按「確認」（4.8）。
    SpotlightConfirmed,
    /// 使用者在焦點提示中按「取消」（4.8）。
    SpotlightCancelled,
    /// 勿打擾狀態變化（4.7c：`wallpaper_dnd` 的輪詢執行緒在合併結果改變時送出）。
    DoNotDisturbChanged,
    /// 工作階段即將結束（`WM_QUERYENDSESSION`／`WM_ENDSESSION` 確定結束；登出、關機、重新開機、
    /// Restart Manager 關閉）：不還原、停止新的渲染與桌布 API 呼叫（4.7b）。
    SessionEnding,
    /// 工作階段結束被取消（`WM_ENDSESSION` 的 `wParam` 為 0）：恢復排程（4.7b）。
    SessionEndCancelled,
}

/// 去重、保留第一次出現的先後。
fn dedupe(wakes: Vec<Wake>) -> Vec<Wake> {
    let mut seen = HashSet::new();
    wakes.into_iter().filter(|w| seen.insert(*w)).collect()
}

/// 設定桌布（或設定前讀回）失敗的種類：
/// - `Busy`（前一個逾時的請求還沒返回，沒有送出）→ [`FailureKind::Busy`]；
/// - `Timeout`（explorer 卡住）與送不出去的錯誤（`WrongThread`、`WorkerGone`）→ [`FailureKind::Apply`]：
///   等下一個時點，逾時的請求返回時另有「設定路徑恢復」事件提早重試；
/// - `Backend`（explorer 有回應但回錯）、`Aborted`（後端中途中止）→ [`FailureKind::ApplyError`]：
///   不會有恢復事件，由排程器依 `retry_delay` 退避重試（2026-10-06 在場驗收）。
fn apply_failure_kind(e: &WallpaperError) -> FailureKind {
    match e {
        WallpaperError::Busy { .. } => FailureKind::Busy,
        WallpaperError::Timeout { .. }
        | WallpaperError::WrongThread { .. }
        | WallpaperError::WorkerGone { .. } => FailureKind::Apply,
        WallpaperError::Backend { .. } | WallpaperError::Aborted { .. } => FailureKind::ApplyError,
    }
}

/// 「等待重試」記錄用的失敗種類名稱。
fn retry_kind_label(kind: FailureKind) -> &'static str {
    match kind {
        FailureKind::Render => "渲染失敗",
        FailureKind::ApplyError => "套用失敗",
        FailureKind::Apply => "設定逾時後恢復",
        FailureKind::Busy => "忙碌跳過後恢復",
    }
}

fn wake_list(wakes: &[Wake]) -> String {
    wakes
        .iter()
        .map(|w| format!("{w:?}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// 一次評估需要的宿主輸入（[`CoordinatorPorts::inputs`] 短暫上鎖複製出來）。
#[derive(Debug, Clone, PartialEq)]
pub struct HostInputs {
    /// 設定中的桌布主題。
    pub theme: WallpaperTheme,
    /// 目前成立的暫停原因（`AppState::pause_reasons`；電池／省電已依 `PauseRules` 過濾）。
    pub pause: HashSet<PauseReason>,
    /// `tw-events` 快照的資料日期（[`data_dates`]）。
    pub data: DataDates,
    /// 主題設定檔內容改變的次數（`ChannelRegistry::wallpaper_config_version`）。
    pub config_version: Option<u64>,
}

/// 一台顯示器（Tauri 列舉＋穩定識別），用來對 `IDesktopWallpaper` 的矩形配 DPI 與螢幕鍵。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayInfo {
    /// 穩定的 `monitorDevicePath`（`desktop::source_name_to_device_path`；查不到為 `None`）。
    pub device_path: Option<String>,
    /// 整台螢幕的矩形（實體像素、虛擬桌面座標）。
    pub rect: PhysicalRect,
    /// 縮放比例對應的 DPI（96＝100%）。
    pub dpi: u32,
}

/// 從 `tw-events` 擷取排程器要的資料日期：`twii_intraday.date`、`twii_daily` 最後一筆有效的
/// `date`、`margin.date`；缺漏或無法解析為 `None`。
pub fn data_dates(tw_events: &Value) -> DataDates {
    fn date_of(v: Option<&Value>) -> Option<CivilDate> {
        v?.get("date")?.as_str().and_then(CivilDate::parse_iso)
    }
    DataDates {
        twii_intraday: date_of(tw_events.get("twii_intraday")),
        twii_daily: tw_events
            .get("twii_daily")
            .and_then(Value::as_array)
            .and_then(|rows| rows.iter().rev().find_map(|row| date_of(Some(row)))),
        margin: date_of(tw_events.get("margin")),
    }
}

// ---------------------------------------------------------------------------------------------
// 依賴注入
// ---------------------------------------------------------------------------------------------

/// 協調迴圈對宿主的全部依賴（正式：[`TauriPorts`]；測試：假時鐘、假渲染器……）。
pub trait CoordinatorPorts: Send {
    /// 現在（UTC，UNIX 秒）。
    fn now(&self) -> UnixSeconds;
    /// 單調時鐘（任意起點）：與 [`Self::now`] 比對，偵測系統時間被調整。
    fn monotonic(&self) -> Duration;
    /// 本機時區。
    fn tz(&self) -> &dyn UtcOffsetSource;
    /// 宿主輸入（短暫上鎖複製；見模組文件「執行緒與鎖」）。
    fn inputs(&self) -> HostInputs;
    /// 目前的顯示器（實體矩形、DPI、穩定識別）。
    fn displays(&self) -> Vec<DisplayInfo>;
    /// 以螢幕矩形查有效 DPI（正式：`desktop::dpi_for_rect`）；查不到為 `None`。
    fn dpi_for_rect(&self, rect: PhysicalRect) -> Option<u32>;
    /// 渲染一台螢幕（阻塞，最長約 30 秒；呼叫時不持有任何宿主的鎖）。
    fn render(
        &self,
        req: &RenderRequest,
        out: &OutputTarget<'_>,
    ) -> Result<RenderSuccess, RenderFailure>;
    /// 狀態機要求把主題改成「不接管」：改設定、存檔、廣播。存檔失敗回 `Err`（記憶體中已改）：協調迴圈
    /// 依 `retry_delay` 以 [`Self::resave_theme_none`] 重試存檔（task 6.4，審查 R2b-L1）。
    fn set_theme_none(&self, cause: ThemeNoneCause) -> Result<(), String>;
    /// 重試把「不接管」存檔：記憶體中的主題仍是「不接管」才存（`Ok(true)`）；使用者已選回某個主題時
    /// 不動（`Ok(false)`）。
    fn resave_theme_none(&self) -> Result<bool, String>;
    /// 系統匣通知（讓位、安全閥；4.8 正式實作以系統匣通知顯示）。
    fn notify_user(&self, notice: &TakeoverNotice);
    /// 原桌布是 Windows 焦點、開始等使用者確認（4.8）：設定視窗可能沒開，正式實作以系統匣通知請
    /// 使用者開設定確認。同一段等待只呼叫一次。預設什麼都不做。
    fn spotlight_confirmation_needed(&self) {}
    /// 等待（設定後讀回確認前的延遲）。
    fn sleep(&self, d: Duration);
    /// 勿打擾是否成立（4.7c；正式：`wallpaper_dnd::DndMonitor::query`）。只在評估結果是 `Redraw` 時
    /// 呼叫。預設 `false`。
    fn do_not_disturb(&self) -> bool {
        false
    }
    /// explorer（殼層視窗擁有者）目前的 GDI 物件數（4.9 安全閥；正式：
    /// `desktop::explorer_gdi::read_explorer_gdi`，`self-test-ipc` 建置可注入假讀值）。只做核心查詢、
    /// 不送訊息給 explorer，不會阻塞。失敗回一行說明。
    fn explorer_gdi(&self) -> Result<ExplorerSample, String>;
}

/// 協調迴圈的信箱（正式：[`Mailbox`]；測試：可在渲染中途推入喚醒的假信箱）。
pub trait Inbox {
    /// 取出（並清空）目前累積的喚醒。
    fn drain(&self) -> Vec<Wake>;
    /// 宿主是否要求結束（進行中的 `Redraw` 在下一個檢查點中止）。
    fn shutdown_requested(&self) -> bool;
    /// 是否有待處理的還原要求（系統匣結束、命令列；4.7b）：進行中的 `Redraw` 同樣在下一個檢查點
    /// 中止，要求在 `Redraw` 結束後由迴圈外殼處理（[`Coordinator::handle_request`]）。
    fn restore_requested(&self) -> bool {
        false
    }
    /// 信箱裡是否有尚未處理的「工作階段結束」（之後沒有被「取消」蓋過；審查 F2）：工作階段結束的
    /// 處理常式一投遞就看得到，進行中的 `Redraw` 在下一個檢查點中止，不必等 `handle_wakes`。
    fn session_end_pending(&self) -> bool {
        false
    }
}

/// 一串喚醒裡，最後一個工作階段相關的是不是「結束」（[`Inbox::session_end_pending`] 的本體）。
pub(crate) fn session_end_pending_in(wakes: impl Iterator<Item = Wake>) -> bool {
    wakes
        .filter(|w| matches!(w, Wake::SessionEnding | Wake::SessionEndCancelled))
        .last()
        == Some(Wake::SessionEnding)
}

/// 記錄接口（等級＋訊息）；正式寫進 `log`（target [`LOG_TARGET`]），測試收集。
pub type LogSink = Arc<dyn Fn(Level, &str) + Send + Sync>;

/// 時間參數。
#[derive(Debug, Clone, Copy)]
pub struct CoordinatorConfig {
    /// `Applied` 之後等多久再讀回確認（4.4 契約：約 1 秒）。
    pub confirm_delay: Duration,
    /// 沒有計時器時最長等多久再評估一次（兜底偵測時鐘跳動、時區與顯示器變化）。
    pub max_wait: Duration,
    /// COM 執行緒忙碌時最長等多久再查一次忙碌旗標（偵測「逾時後忙碌解除」）。
    pub busy_poll: Duration,
    /// 系統時間與單調時鐘的差距超過這麼多秒才算「時間被調整」。
    pub clock_jump_tolerance_secs: i64,
    /// 同一次 step 內最多執行幾次 `Redraw`（防禦：正常情況每次執行都會記錄，不會連續回傳 Redraw）。
    pub max_redraws_per_step: u32,
    /// 接管狀態機的等待時間。
    pub takeover: TakeoverConfig,
    /// 工作階段結束通知之後行程仍在執行超過這麼久＝錯過了取消通知（`WM_ENDSESSION` 為 0），恢復
    /// 排程（4.7b）。真的結束時行程早就被系統結束了。
    pub session_end_stale: Duration,
}

impl Default for CoordinatorConfig {
    fn default() -> Self {
        Self {
            confirm_delay: Duration::from_secs(1),
            max_wait: Duration::from_secs(60),
            busy_poll: Duration::from_secs(2),
            clock_jump_tolerance_secs: 60,
            max_redraws_per_step: 8,
            takeover: TakeoverConfig::default(),
            session_end_stale: Duration::from_secs(10 * 60),
        }
    }
}

/// [`Coordinator::new`] 的參數。
pub struct CoordinatorParts<P> {
    pub ports: P,
    pub service: WallpaperService,
    pub registry: Box<dyn RegistryStore + Send>,
    pub files: Box<dyn FileIdentity + Send>,
    pub paths: StatePaths,
    pub config: CoordinatorConfig,
    pub log: LogSink,
}

// ---------------------------------------------------------------------------------------------
// 需要回覆的要求（4.7b：系統匣結束、命令列還原）
// ---------------------------------------------------------------------------------------------

/// 還原的結果分類，也是 `--restore-wallpaper` 的結束碼（[`RestoreCode::exit_code`]）：0＝已還原或
/// 無需還原，其餘都是失敗。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RestoreCode {
    /// 已還原，或沒有需要還原的桌布（結束碼 0）。
    Restored,
    /// 還原失敗或狀態檔暫時讀不到；狀態檔保留，可重試（結束碼 1）。
    Failed,
    /// 狀態檔讀不懂（封鎖），無法還原（結束碼 2）。
    Blocked,
    /// 命令列：執行中的宿主在時限內沒有回報（結束碼 3）。
    NoReply,
    /// 命令列：有宿主在執行卻交接不了，或宿主收到了卻無法處理（結束碼 4）。
    HandoffFailed,
    /// 命令列：本行程內部錯誤（例如無法建立桌布 COM 執行緒、還原工作執行緒 panic；結束碼 5）。
    Internal,
    /// 在線的螢幕都還原了，但有螢幕離線或讀不到而標為待還原——要宿主之後再執行才會補做（結束碼 6；
    /// 審查 F5）。
    PartiallyRestored,
    /// 命令列：整體時限（`wallpaper_cli::CLI_TOTAL_LIMIT`）內沒做完（結束碼 7；複審 N3）。本行程還原
    /// 做到一半時，狀態檔已有「還原進行中」標記，可再執行一次。
    TimedOut,
}

impl RestoreCode {
    /// 行程結束碼（供安裝檔判斷）。
    pub fn exit_code(self) -> i32 {
        match self {
            Self::Restored => 0,
            Self::Failed => 1,
            Self::Blocked => 2,
            Self::NoReply => 3,
            Self::HandoffFailed => 4,
            Self::Internal => 5,
            Self::PartiallyRestored => 6,
            Self::TimedOut => 7,
        }
    }

    /// 記錄用的中文名稱。
    pub fn label(self) -> &'static str {
        match self {
            Self::Restored => "已還原（或無需還原）",
            Self::Failed => "失敗",
            Self::Blocked => "狀態檔封鎖",
            Self::NoReply => "宿主沒有回報",
            Self::HandoffFailed => "交接失敗",
            Self::Internal => "內部錯誤",
            Self::PartiallyRestored => "部分還原（有待還原的螢幕）",
            Self::TimedOut => "超過命令列總時限",
        }
    }
}

/// 一次還原的回報（結果分類＋一行摘要）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreReport {
    pub code: RestoreCode,
    pub summary: String,
}

impl RestoreReport {
    pub fn new(code: RestoreCode, summary: impl Into<String>) -> Self {
        Self {
            code,
            summary: summary.into(),
        }
    }

    /// 狀態機的還原結果 → 回報。
    pub fn from_result(result: &RestoreResult) -> Self {
        match result {
            RestoreResult::Restored {
                offline_unverified,
                user_choice_kept,
                warnings,
                state_saved,
            } => Self::new(
                if offline_unverified.is_empty() {
                    RestoreCode::Restored
                } else {
                    RestoreCode::PartiallyRestored
                },
                format!(
                    "已還原；離線待還原 {offline_unverified:?}、保留使用者自選 {user_choice_kept:?}、警告 {} 則、狀態已存檔 {state_saved}",
                    warnings.len()
                ),
            ),
            RestoreResult::NothingToRestore => {
                Self::new(RestoreCode::Restored, "沒有需要還原的桌布（未接管）")
            }
            RestoreResult::Blocked => Self::new(RestoreCode::Blocked, "狀態檔讀不懂（封鎖），無法還原"),
            RestoreResult::StateUnavailable => {
                Self::new(RestoreCode::Failed, "狀態檔暫時讀不到，沒有還原")
            }
            RestoreResult::Failed { problems } => {
                Self::new(RestoreCode::Failed, format!("還原失敗：{problems:?}"))
            }
            RestoreResult::Queued => Self::new(RestoreCode::Failed, "還原排隊中（不應發生）"),
        }
    }
}

/// 要求的進度回報。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestProgress {
    /// 協調迴圈已開始處理（進行中的 Redraw 已結束或中止）。
    Started,
    /// 還原完成（成功或失敗）。
    Finished(RestoreReport),
    /// 「因更新結束」已處理完（沒有還原、沒有動任何狀態；迴圈隨即結束）。
    ExitedForUpdate,
}

/// 回覆接口（在協調執行緒上呼叫，必須立即返回）。
pub type ReplyFn = Box<dyn FnMut(RequestProgress) + Send>;

/// 需要回覆的要求：還原類經 [`CoordinatorHandle::request_restore_within`]、「因更新結束」經
/// [`CoordinatorHandle::request_exit_for_update`] 投遞，迴圈外殼在 `step` 之間依序處理。
pub enum Request {
    /// 系統匣「結束」：還原後協調迴圈結束（宿主隨後結束）。
    ExitAndRestore(ReplyFn),
    /// `--restore-wallpaper` 交給執行中的宿主：主題改「不接管」並存檔、還原；迴圈繼續。
    CommandLineRestore(ReplyFn),
    /// 「因更新結束」（installer-auto-update task 3.3、design.md D4）：語意同工作階段結束——**不還原**、
    /// 不呼叫任何桌布 API、不動狀態檔與主題設定（含不寫「還原進行中」標記、不重試存主題）——但處理完
    /// 讓協調迴圈結束並回報 [`RequestProgress::ExitedForUpdate`]。新版安裝完成後重新啟動，狀態檔仍是
    /// 接管中，新行程照常接管並重畫。
    ExitForUpdate(ReplyFn),
}

impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::ExitAndRestore(_) => "ExitAndRestore",
            Self::CommandLineRestore(_) => "CommandLineRestore",
            Self::ExitForUpdate(_) => "ExitForUpdate",
        })
    }
}

/// [`Coordinator::handle_request`] 之後迴圈要不要繼續。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopControl {
    Continue,
    Exit,
}

// ---------------------------------------------------------------------------------------------
// 可查詢的狀態（4.8）
// ---------------------------------------------------------------------------------------------

/// 協調迴圈目前的狀態（設定視窗顯示用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoordinatorState {
    /// 還沒評估過。
    Starting,
    /// 主題為「不接管」。
    Idle,
    /// 首次無資料：所選主題需要的資料還不存在，不接管（設定視窗顯示「等待資料中」）。
    WaitingForData(DataKey),
    /// 已接管但資料鍵不見了：保留目前的桌布。
    HoldWithoutData(DataKey),
    /// 暫停中。
    Paused,
    /// 勿打擾中（4.7c）。
    DoNotDisturb,
    /// 原桌布是 Windows 焦點，等使用者確認（4.8）；確認前不渲染、不設定。
    AwaitingSpotlightConfirmation,
    /// 不需要重畫。
    UpToDate,
    /// 這次沒有要重畫的螢幕，但有螢幕上次失敗、等短間隔重試（排程器的
    /// [`crate::wallpaper::Decision::pending_retries`]；計時器已含重試時刻）。不是「已是最新」。
    RetryPending,
    /// 正在重畫。
    Redrawing,
    /// 狀態檔讀不懂：不接管、不還原（附原因）。
    Blocked(String),
    /// 上次的還原沒做完（狀態檔有「還原進行中」標記）：先續做還原，完成前不接管、不渲染（4.7b）。
    ResumingRestore,
    /// 啟動時狀態檔暫時讀不到：啟動判定（含「還原進行中」標記）延到讀到為止，期間不接管、不渲染
    /// （4.7b 審查 F1）。
    WaitingForStateFile,
    /// 工作階段結束中：不還原、不渲染、不呼叫桌布 API（4.7b）。
    SessionEnding,
}

/// [`CoordinatorHandle::status`] 的內容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoordinatorStatus {
    pub state: CoordinatorState,
    /// 原桌布是 Windows 焦點，等使用者確認（4.8 開提示）。
    pub awaiting_spotlight_confirmation: bool,
    /// 桌布 COM 執行緒忙碌中（explorer 無回應）。
    pub com_busy: bool,
    /// 「不接管」的還原因狀態檔暫時讀不到而排定的重試時刻。
    pub restore_retry_at: Option<UnixSeconds>,
    /// 下一次計時器時刻。
    pub next_wake: Option<UnixSeconds>,
    /// 這次執行最近發出的通知（讓位、安全閥；最多 [`MAX_STATUS_NOTICES`] 筆，最舊的先丟；4.8 在發出
    /// 當下已以系統匣通知顯示，這裡只供查詢）。
    pub notices: Vec<TakeoverNotice>,
    /// 狀態檔為接管中（4.8：設定視窗據此判斷選主題時要不要先問 Windows 焦點；封鎖或暫時讀不到時
    /// 保守為真，同 `WallpaperTakeover::already_taken_over`）。
    pub taken_over: bool,
    /// 使用者這次執行已確認接管（原桌布為 Windows 焦點；4.8：設定視窗不再重問）。
    pub spotlight_confirmed: bool,
}

impl Default for CoordinatorStatus {
    fn default() -> Self {
        Self {
            state: CoordinatorState::Starting,
            awaiting_spotlight_confirmation: false,
            com_busy: false,
            restore_retry_at: None,
            next_wake: None,
            notices: Vec::new(),
            taken_over: false,
            spotlight_confirmed: false,
        }
    }
}

/// 設定視窗的事先確認（4.8 修正輪 1）：使用者在選主題時已確認焦點提示的**那個主題**。設定視窗在存檔前
/// 登記、存檔失敗或取消時撤回；協調迴圈在看到設定的主題正是它時才套用（`WallpaperTakeover::confirm_spotlight`）
/// 並清除，所以確認不會套用到還沒被換掉的舊主題上（不會先用舊主題畫一次）。只取一把小鎖、不等待。
#[derive(Debug, Clone, Default)]
pub struct SpotlightPreconfirm(Arc<Mutex<Option<WallpaperTheme>>>);

impl SpotlightPreconfirm {
    /// 登記（`Some`）或撤回（`None`）。
    pub fn set(&self, theme: Option<WallpaperTheme>) {
        *lock(&self.0) = theme;
    }

    /// 目前登記的主題。
    pub fn get(&self) -> Option<WallpaperTheme> {
        *lock(&self.0)
    }

    /// 登記的正是 `theme`（且不是「不接管」）時取出並清除。
    fn take_if(&self, theme: WallpaperTheme) -> bool {
        let mut g = lock(&self.0);
        if theme != WallpaperTheme::None && *g == Some(theme) {
            *g = None;
            true
        } else {
            false
        }
    }
}

/// [`CoordinatorStatus::notices`] 保留的筆數上限（只供查詢；通知在發生當下已顯示）。
pub const MAX_STATUS_NOTICES: usize = 16;

/// 一次 step 的結果（迴圈外殼據此決定等多久）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepOutcome {
    /// 下一次時間觸發（`None`＝只等事件；外殼仍以 `max_wait` 兜底）。
    pub next_wake: Option<UnixSeconds>,
    /// COM 執行緒忙碌（外殼以 `busy_poll` 縮短等待）。
    pub busy: bool,
}

// ---------------------------------------------------------------------------------------------
// 協調迴圈本體
// ---------------------------------------------------------------------------------------------

/// 還原失敗後排定的重試：狀態檔暫時讀不到（4.4 契約第 4 點、修正輪 3 裁決 6），或還原本身失敗
/// （COM 忙碌、逾時、讀回驗證失敗；4.7a 修正輪 1）。依 `retry_delay` 退避；設定路徑恢復時立即重試。
#[derive(Debug, Clone)]
struct RestoreRetry {
    reason: RestoreReason,
    failures: u32,
    at: UnixSeconds,
    /// 這個退避窗（到 `at` 為止）的「設定路徑恢復」重試額度已用掉（4.7b，比照 4.3 M1）：恢復觸發的
    /// 重試失敗後，新的退避窗延續已用掉的狀態；只有退避時刻到的重試才開啟新的額度。
    recovery_used: bool,
}

/// 啟動時發現的「還原進行中」標記（4.7b）：續做完成前不接管。
#[derive(Debug, Clone)]
struct ResumeState {
    reason: RestoreReason,
    started_at: UnixSeconds,
    /// 續做期間使用者改選了某個主題：完成後照常接管，不強制改回「不接管」。
    reselected: bool,
}

/// 狀態檔為接管中且有「還原進行中」標記＝上次的還原沒做完（未接管卻有標記不該發生，在下一次還原時
/// 清掉）。封鎖或暫時讀不到時 `None`。
fn resume_from(takeover: &WallpaperTakeover, reselected: bool) -> Option<ResumeState> {
    takeover
        .state()
        .filter(|s| s.status == TakeoverStatus::TakenOver)
        .and_then(|s| s.restore_in_progress.as_ref())
        .map(|m| ResumeState {
            reason: m.restore_reason(),
            started_at: m.started_at,
            reselected,
        })
}

/// 上一次檢查時鐘時的讀值。
#[derive(Debug, Clone, Copy)]
struct ClockAnchor {
    wall: UnixSeconds,
    mono: Duration,
    /// 兩個固定參考時刻（啟動當年 1/15 與 7/15）的偏移：時區設定改變才會變，夏令切換不會。
    fingerprint: (i32, i32),
}

/// 桌布協調迴圈（見模組文件）。
pub struct Coordinator<P> {
    ports: P,
    service: WallpaperService,
    registry: Box<dyn RegistryStore + Send>,
    files: Box<dyn FileIdentity + Send>,
    paths: StatePaths,
    config: CoordinatorConfig,
    log: LogSink,
    takeover: WallpaperTakeover,
    scheduler: SchedulerState,
    status: Arc<Mutex<CoordinatorStatus>>,

    /// 上次看到的主題（`None`＝還沒看過，啟動時一定當成「改變」）。
    last_theme: Option<WallpaperTheme>,
    /// 上次看到的主題設定版本。
    last_config_version: Option<u64>,
    /// 上次的顯示器清單（Tauri）。
    displays: Vec<DisplayInfo>,
    /// 上次成功的 `IDesktopWallpaper` 螢幕列舉。
    com_monitors: Option<Vec<MonitorEntry>>,
    /// 下一次評估前要重新列舉螢幕。
    monitors_dirty: bool,
    /// 下一次成功列舉後清理已拔除螢幕的殘留輸出檔。
    cleanup_pending: bool,
    /// 上次觀察到的 COM 忙碌旗標與開始時刻。
    last_busy: bool,
    busy_since: Option<UnixSeconds>,
    /// 這一段連續的 busy 跳過是否已記過 warn。
    busy_warned: bool,
    clock: Option<ClockAnchor>,
    /// 時區指紋的兩個參考時刻（第一次檢查時決定）。
    tz_probes: Option<(UnixSeconds, UnixSeconds)>,
    restore_retry: Option<RestoreRetry>,
    /// 上一次記錄的決策摘要（純計時器喚醒且摘要沒變時不記）。
    last_decision: String,
    /// 是否已記過「接管中缺資料」的警告（同一段只記一次）。
    hold_warned: bool,
    /// COM 忙碌、延後列舉螢幕的記錄是否已寫過（同一段忙碌只記一次）。
    list_deferred_logged: bool,
    /// 上一次列舉螢幕失敗的錯誤（相同錯誤不重記）。
    last_list_error: Option<String>,
    /// 列舉螢幕失敗後的重試（連續失敗次數、重試時刻）：依 [`crate::wallpaper::retry_delay`] 退避，
    /// 成功列舉後清除。沒有要重畫的螢幕時也要短間隔再列舉，不能等下一個時點才發現清單已變
    /// （2026-10-06 在場驗收：失敗不得被當成已是最新）。
    list_retry: Option<(u32, UnixSeconds)>,
    /// 處理待還原螢幕失敗（讀不到目前桌布等）後的重試（連續失敗次數、重試時刻）：依
    /// [`crate::wallpaper::retry_delay`] 退避，處理成功（含仍離線）後清除。插回時 explorer 可能比顯示
    /// 變更事件慢，那一次讀取失敗後不會再有事件（審查 High-1）。
    pending_retry: Option<(u32, UnixSeconds)>,
    /// 查不到 DPI 而已記過警告的螢幕矩形（查得到後移除，之後再失敗會再記）。
    dpi_warned: HashSet<(i32, i32, i32, i32)>,
    next_wake: Option<UnixSeconds>,
    /// 上次的還原沒做完、要先續做（4.7b）。
    resume: Option<ResumeState>,
    /// 外部要求的「還原進行中」標記（4.7b 修正輪 1；與把手共用）。
    restore_request: Arc<RestoreRequestMarker>,
    /// 啟動判定（標記、接管狀態）已做（審查 F1）：啟動時狀態檔暫時讀不到就延到讀到為止。
    startup_decided: bool,
    /// 延後的啟動判定下次重讀狀態檔的時刻與失敗次數。
    state_retry: Option<(u32, UnixSeconds)>,
    /// 啟動判定之前使用者改選了某個主題：判定出續做時記為「重新選了主題」。
    pending_reselect: bool,
    /// 啟動判定之前使用者重新開啟了接管（主題從「不接管」改回某個主題；4.9 修正輪 1）：讀到狀態檔時
    /// 清除安全閥基準。只換主題（A → B）不設。
    pending_reopen: bool,
    /// 工作階段結束中、延後的啟動判定被擋下的記錄是否已寫過（複審 N1）。
    decision_session_logged: bool,
    /// 工作階段結束中（4.7b）與開始的時刻。
    session_ending: bool,
    session_ending_since: Option<UnixSeconds>,
    /// 工作階段結束期間跳過還原重試的記錄是否已寫過。
    session_retry_logged: bool,
    /// 目前處於一段連續的 explorer GDI 讀取失敗中（4.9：同一段只記一次 warn，恢復時記一行）。
    explorer_read_failing: bool,
    /// 設定視窗的事先確認（4.8 修正輪 1；與把手共用）。
    preconfirm: SpotlightPreconfirm,
    /// 「主題改不接管」存檔失敗後的重試（task 6.4，審查 R2b-L1）：（連續失敗次數, 下次重試時刻）。
    theme_save_retry: Option<(u32, UnixSeconds)>,
}

impl<P: CoordinatorPorts> Coordinator<P> {
    /// 讀狀態檔、建立排程器（空狀態）。不呼叫任何桌布 API。
    pub fn new(parts: CoordinatorParts<P>) -> Self {
        let CoordinatorParts {
            ports,
            service,
            registry,
            files,
            paths,
            config,
            log,
        } = parts;
        let now = ports.now();
        let mut takeover = WallpaperTakeover::load(paths.clone(), config.takeover, now);
        let restore_request = Arc::new(RestoreRequestMarker::default());
        restore_request.set_paths(paths.clone());
        takeover.attach_restore_request(Arc::clone(&restore_request));
        // 只有接管中的標記才是「還原沒做完」；未接管卻有標記（不該發生）在下一次還原時清掉。
        let startup_decided = takeover.unavailable().is_none();
        let resume = resume_from(&takeover, false);
        let c = Self {
            ports,
            service,
            registry,
            files,
            paths,
            config,
            log,
            takeover,
            scheduler: SchedulerState::default(),
            status: Arc::new(Mutex::new(CoordinatorStatus::default())),
            last_theme: None,
            last_config_version: None,
            displays: Vec::new(),
            com_monitors: None,
            monitors_dirty: true,
            cleanup_pending: true,
            last_busy: false,
            busy_since: None,
            busy_warned: false,
            clock: None,
            tz_probes: None,
            restore_retry: None,
            last_decision: String::new(),
            hold_warned: false,
            list_deferred_logged: false,
            last_list_error: None,
            list_retry: None,
            pending_retry: None,
            dpi_warned: HashSet::new(),
            next_wake: None,
            resume,
            restore_request,
            startup_decided,
            state_retry: None,
            pending_reselect: false,
            pending_reopen: false,
            decision_session_logged: false,
            session_ending: false,
            session_ending_since: None,
            session_retry_logged: false,
            explorer_read_failing: false,
            preconfirm: SpotlightPreconfirm::default(),
            theme_save_retry: None,
        };
        if let Some(reason) = c.takeover.blocked() {
            c.log_at(
                Level::Error,
                format!(
                    "狀態檔 {} 無法使用（封鎖：{reason:?}）：不排程渲染、不設定、不還原桌布",
                    c.paths.state_file.display()
                ),
            );
        } else if let Some(e) = c.takeover.unavailable() {
            c.log_at(
                Level::Warn,
                format!(
                    "狀態檔目前讀不到（{e}）：啟動判定（含「還原進行中」標記）延到讀到為止，期間不接管、不渲染"
                ),
            );
        }
        c
    }

    /// 換成外部提供的狀態儲存格（[`spawn_coordinator`] 用：把手先建立，迴圈後建立）。
    fn attach_status(&mut self, cell: Arc<Mutex<CoordinatorStatus>>) {
        let current = self.status();
        *lock(&cell) = current;
        self.status = cell;
    }

    /// 換成把手共用的事先確認登記（[`spawn_coordinator`] 用）。
    fn attach_preconfirm(&mut self, cell: SpotlightPreconfirm) {
        self.preconfirm = cell;
    }

    /// 事先確認的登記（測試以它模擬設定視窗）。
    #[cfg(test)]
    pub(crate) fn spotlight_preconfirm(&self) -> SpotlightPreconfirm {
        self.preconfirm.clone()
    }

    /// 換成把手共用的外部標記要求（[`spawn_coordinator`] 用）。
    fn attach_restore_request(&mut self, request: Arc<RestoreRequestMarker>) {
        request.set_paths(self.paths.clone());
        self.takeover.attach_restore_request(Arc::clone(&request));
        self.restore_request = request;
    }

    /// 目前的狀態（複本）。
    pub fn status(&self) -> CoordinatorStatus {
        lock(&self.status).clone()
    }

    /// 桌布 COM 執行緒是否忙碌。
    #[cfg(test)]
    pub(crate) fn service_busy(&self) -> bool {
        self.service.is_busy()
    }

    #[cfg(test)]
    pub(crate) fn scheduler_generation(&self) -> u64 {
        self.scheduler.generation()
    }

    fn log_at(&self, level: Level, msg: impl AsRef<str>) {
        (self.log)(level, msg.as_ref());
    }

    fn info(&self, msg: impl AsRef<str>) {
        self.log_at(Level::Info, msg);
    }

    fn warn(&self, msg: impl AsRef<str>) {
        self.log_at(Level::Warn, msg);
    }

    fn publish(&self, f: impl FnOnce(&mut CoordinatorStatus)) {
        f(&mut lock(&self.status));
    }

    /// 本機時刻的記錄格式：`2026-10-05 10:15:00 (UTC+08:00)`。
    fn fmt_time(&self, t: UnixSeconds) -> String {
        let offset = self.ports.tz().offset_at(t);
        let local = iso_utc(t + i64::from(offset));
        let sign = if offset < 0 { '-' } else { '+' };
        let abs = offset.unsigned_abs();
        format!(
            "{} (UTC{sign}{:02}:{:02})",
            local.trim_end_matches('Z').replace('T', " "),
            abs / 3600,
            abs / 60 % 60
        )
    }

    fn fmt_wake(&self, t: Option<UnixSeconds>) -> String {
        t.map_or_else(|| "無（只等事件）".to_owned(), |t| self.fmt_time(t))
    }

    // ── step ─────────────────────────────────────────────────────────────────────────────

    /// 處理一批喚醒、評估、必要時執行 `Redraw`（見模組文件「一次 step」）。
    pub fn step(&mut self, wakes: Vec<Wake>, inbox: &dyn Inbox) -> StepOutcome {
        let mut wakes = dedupe(wakes);
        let mut after_redraw = false;
        let mut redraws = 0;
        loop {
            self.handle_wakes(&wakes, after_redraw, inbox.restore_requested());
            let quiet = !after_redraw && wakes.iter().all(|w| *w == Wake::Timer);
            let Some(plan) = self.evaluate(quiet) else {
                break;
            };
            if inbox.shutdown_requested() {
                self.info("宿主結束中，不執行 Redraw");
                break;
            }
            if inbox.restore_requested() {
                self.info("有待處理的還原要求（系統匣結束或命令列），不執行 Redraw");
                break;
            }
            if inbox.session_end_pending() {
                self.info("工作階段結束通知已到，不執行 Redraw");
                break;
            }
            self.execute(&plan, inbox);
            redraws += 1;
            wakes = dedupe(inbox.drain());
            if wakes.is_empty() {
                self.info("Redraw 結束，立即再評估一次");
            } else {
                self.info(format!(
                    "Redraw 期間的喚醒合併為一次評估：{}",
                    wake_list(&wakes)
                ));
            }
            after_redraw = true;
            if redraws >= self.config.max_redraws_per_step {
                self.warn(format!(
                    "同一次喚醒內已執行 {redraws} 次 Redraw，先停下，{} 秒後再評估",
                    self.config.max_wait.as_secs()
                ));
                let now = self.ports.now();
                let cap = now + i64::try_from(self.config.max_wait.as_secs()).unwrap_or(60);
                self.next_wake = Some(self.next_wake.map_or(cap, |t| t.min(cap)));
                break;
            }
        }
        let busy = self.service.is_busy() || self.last_busy;
        let next_wake = self.next_wake;
        let retry_at = self.restore_retry.as_ref().map(|r| r.at);
        let taken_over = self.takeover.already_taken_over();
        let spotlight_confirmed = self.takeover.phase() == Phase::SpotlightConfirmed;
        // 以狀態機的階段為準（4.8）：等確認期間使用者在設定改選「不接管」也會結束等待。
        let awaiting = self.takeover.phase() == Phase::AwaitingSpotlightConfirmation;
        self.publish(|s| {
            s.awaiting_spotlight_confirmation = awaiting;
            s.com_busy = busy;
            s.next_wake = next_wake;
            s.restore_retry_at = retry_at;
            s.taken_over = taken_over;
            s.spotlight_confirmed = spotlight_confirmed;
        });
        StepOutcome { next_wake, busy }
    }

    /// 迴圈外殼這次最多等多久（見模組文件「一次 step」第 4 點）。
    pub fn wait_timeout(&self, out: &StepOutcome) -> Duration {
        let mut wait = self.config.max_wait;
        if let Some(t) = out.next_wake {
            let secs = u64::try_from((t - self.ports.now()).max(0)).unwrap_or(0);
            // 計時器以秒為單位：多等 200 ms，避免在時點前一刻醒來又多睡一輪。
            wait = wait.min(Duration::from_secs(secs) + Duration::from_millis(200));
        }
        if out.busy {
            wait = wait.min(self.config.busy_poll);
        }
        wait
    }

    // ── 喚醒處理 ─────────────────────────────────────────────────────────────────────────

    /// `request_pending`＝信箱裡有尚未處理的要求（[`Inbox::restore_requested`]：系統匣結束、命令列
    /// 還原、因更新結束）。要求可能在迴圈外殼 `take_requests` 之後才到，或讓進行中的 Redraw 中止後
    /// 回到這裡再評估；這時不處理待還原螢幕（會呼叫桌布 COM），留給要求處理完之後（v0.1.0 最終審查
    /// Minor 2：因更新結束的收尾期間不得呼叫桌布 API）。
    fn handle_wakes(&mut self, wakes: &[Wake], after_redraw: bool, request_pending: bool) {
        let has = |w: Wake| wakes.contains(&w);
        let pure_timer = wakes.iter().all(|w| *w == Wake::Timer);
        if !after_redraw && !pure_timer {
            self.info(format!("喚醒：{}", wake_list(wakes)));
        }
        if has(Wake::Startup) {
            self.info(format!(
                "協調迴圈啟動：狀態檔 {}、輸出資料夾 {}",
                self.paths.state_file.display(),
                self.paths.output_dir.display()
            ));
        }

        // 0a. 工作階段結束／取消（4.7b），依到達順序；通知後太久仍在執行＝錯過取消通知。在啟動判定之前
        //     （複審 N1：同一批喚醒帶著「結束」時，延後的判定不得先還原）。
        for wake in wakes {
            match wake {
                Wake::SessionEnding => self.on_session_ending(),
                Wake::SessionEndCancelled => self.on_session_end_cancelled(),
                _ => {}
            }
        }
        self.check_session_end_stale();

        // 0b. 啟動判定（審查 F1）：狀態檔讀得到才做；讀不到就依退避重讀，第一次讀到時照同一套規則判定
        //     （有標記→先續做還原），在主題同步之前。工作階段結束中不做延後的判定（複審 N1）。
        self.ensure_startup_decision(has(Wake::Startup));

        // 1. 時鐘與時區。
        self.check_clock(has(Wake::TimeChanged));

        // 2. 主題：每次都比對（不只看 SettingsChanged——設定可能已改、通知還沒到），啟動時一定處理。
        let theme = self.ports.inputs().theme;
        self.sync_theme(theme, has(Wake::Startup) || has(Wake::SettingsChanged));

        // 3. 焦點確認（4.8）。
        //   a. 設定視窗的事先確認（修正輪 1）：登記的主題正是現在設定的主題才套用——確認不會落在還沒被
        //      換掉的舊主題上（主題變更晚一步才被看到時，留到看到為止）。不重設排程器：主題變更本身就會
        //      觸發評估；從「等確認」確認時才要重設（之前記了 Apply）。
        if self.preconfirm.take_if(theme) {
            let was_awaiting = self.takeover.phase() == Phase::AwaitingSpotlightConfirmation;
            if self.takeover.confirm_spotlight() {
                if was_awaiting {
                    self.scheduler.reset();
                }
                self.info(format!(
                    "使用者在設定視窗事先確認接管主題 {}（原桌布為 Windows 焦點）：第一次設定時不再等確認",
                    theme.as_str()
                ));
            } else {
                self.info(format!(
                    "設定視窗的事先確認（主題 {}）：目前階段 {:?}、接管中 {}，不需要確認，忽略",
                    theme.as_str(),
                    self.takeover.phase(),
                    self.takeover.already_taken_over()
                ));
            }
        }
        //   b. 後備路徑：迴圈自己在等確認時，設定視窗對話的答案。設定視窗另有一個不同主題的事先確認在等
        //      （使用者已改選別的主題）時不套用到目前這個主題。
        if has(Wake::SpotlightConfirmed) {
            let pending_other = self.preconfirm.get().is_some_and(|t| t != theme);
            if self.takeover.phase() == Phase::AwaitingSpotlightConfirmation && !pending_other {
                self.takeover.confirm_spotlight();
                self.scheduler.reset();
                self.info("使用者確認接管（原桌布為 Windows 焦點）：排程器重設，每台重畫");
            } else {
                self.info(format!(
                    "收到焦點確認喚醒，但目前階段 {:?}、接管中 {}、另有其他主題的事先確認 {}：忽略",
                    self.takeover.phase(),
                    self.takeover.already_taken_over(),
                    pending_other
                ));
            }
        }
        if has(Wake::SpotlightCancelled) {
            let events = self.takeover.cancel_spotlight();
            self.publish(|s| s.awaiting_spotlight_confirmation = false);
            self.info("使用者取消接管（原桌布為 Windows 焦點）");
            self.apply_events(events);
        }

        // 4. 顯示變更。
        let display_event = has(Wake::DisplayChanged) || has(Wake::DpiChanged);
        if has(Wake::Startup)
            || display_event
            || has(Wake::ExplorerRestarted)
            || has(Wake::ComRecovered)
        {
            self.monitors_dirty = true;
        }
        if display_event && request_pending {
            // 有待處理的要求：先不處理待還原，改排一個立即到期的待還原重試（連續失敗次數不變）——顯示變更
            // 事件不會再來，直接略過會遺失；要求處理完後的下一次 step 由下面的重試分支補做。
            self.cleanup_pending = true;
            let failures = self.pending_retry.map_or(0, |(n, _)| n);
            self.pending_retry = Some((failures, self.ports.now()));
            self.info("顯示變更：有待處理的要求（系統匣結束、命令列或因更新結束），待還原螢幕等要求處理完再做");
        } else if display_event {
            self.cleanup_pending = true;
            self.process_pending_monitors("顯示變更");
        } else if let Some((failures, at)) = self.pending_retry {
            // 待還原重試（審查 High-1）：時刻已到才做；工作階段結束中不做（工作階段結束不還原）；有待處理
            // 的要求時也不做（要求優先，處理完後的下一次 step 再做）。
            if self.ports.now() >= at && !self.session_ending && !request_pending {
                self.process_pending_monitors(&format!("待還原重試（第 {failures} 次失敗後）"));
            }
        }

        // 5. 設定路徑恢復（COM 忙碌解除、explorer 重新啟動）。此時上一次 Redraw 的每台都已記錄。
        let now = self.ports.now();
        let busy = self.service.is_busy();
        let busy_cleared = self.last_busy && !busy;
        if busy && !self.last_busy {
            self.busy_since.get_or_insert(now);
        }
        self.last_busy = busy;
        if !busy {
            self.busy_since = None;
        }
        let mut causes = Vec::new();
        if busy_cleared {
            causes.push("COM 執行緒忙碌解除");
        }
        if has(Wake::ComRecovered) {
            causes.push("COM 恢復通知");
        }
        if has(Wake::ExplorerRestarted) {
            causes.push("explorer 重新啟動");
        }
        let recovered = !causes.is_empty();
        if recovered {
            let n = self.scheduler.apply_path_recovered(now);
            self.info(format!(
                "設定路徑恢復（{}）：重新排入 {n} 台上次設定失敗或預檢跳過的螢幕",
                causes.join("、")
            ));
        }

        // 6. 主題設定檔。
        let inputs = self.ports.inputs();
        if let Some(version) = inputs.config_version {
            let changed = self.last_config_version.is_some_and(|old| old != version);
            self.last_config_version = Some(version);
            if changed && inputs.theme != WallpaperTheme::None {
                self.scheduler.clear_records();
                self.info(format!(
                    "主題設定檔已變更（第 {version} 版）：清除時點紀錄，目前的主題 {} 重畫",
                    inputs.theme.as_str()
                ));
            }
        }

        // 7. 還原重試：時刻已到，或設定路徑剛恢復（COM 忙碌解除、explorer 重新啟動）。設定路徑恢復
        //    觸發的重試每個退避窗至多一次（4.7b，比照 4.3 M1）。續做中的還原不因主題而取消；工作階段
        //    結束期間不還原。
        if let Some(retry) = self.restore_retry.clone() {
            let due = now >= retry.at;
            if inputs.theme != WallpaperTheme::None && self.resume.is_none() {
                self.restore_retry = None;
                self.info("主題已不是「不接管」，取消排定的還原重試");
            } else if self.session_ending {
                if (due || recovered) && !self.session_retry_logged {
                    self.session_retry_logged = true;
                    self.info("工作階段結束中：不執行還原重試（工作階段結束不還原）");
                }
            } else if due {
                self.info(format!(
                    "還原重試（第 {} 次失敗後；重試時刻已到）",
                    retry.failures
                ));
                self.restore_with(retry.reason, false);
            } else if recovered && retry.recovery_used {
                self.info(format!(
                    "設定路徑恢復，但這個退避窗已用過一次恢復重試：限頻，等到 {} 再試還原",
                    self.fmt_time(retry.at)
                ));
            } else if recovered {
                self.info(format!(
                    "還原重試（第 {} 次失敗後；設定路徑恢復，用掉這個退避窗的恢復額度）",
                    retry.failures
                ));
                self.restore_with(retry.reason, true);
            }
        }

        // 8. 「主題改不接管」的存檔重試（task 6.4，審查 R2b-L1）：只寫設定檔，工作階段結束中也照做。
        self.retry_theme_save(self.ports.now());
    }

    /// 啟動判定（審查 F1）：啟動時狀態檔讀得到就立即做；讀不到就依 `retry_delay` 重讀（其他操作的
    /// 重讀讀到了也算），第一次讀到時做——有「還原進行中」標記就先續做還原。判定之前 `evaluate`
    /// 不接管、不渲染。
    fn ensure_startup_decision(&mut self, startup: bool) {
        if self.startup_decided {
            if startup {
                self.decide_startup(false);
            }
            return;
        }
        if self.session_ending {
            // 複審 N1：工作階段結束 MUST NOT 還原。延後的判定（可能續做還原）等結束被取消後再做；
            // 工作階段真的結束時，標記留在狀態檔給下次啟動。
            if !self.decision_session_logged {
                self.decision_session_logged = true;
                self.info("工作階段結束中：延後的啟動判定（含續做還原）等工作階段結束取消後再做；標記留在狀態檔");
            }
            return;
        }
        self.decision_session_logged = false;
        let now = self.ports.now();
        let readable = match self.state_retry {
            None => false,
            Some((_, at)) if now >= at => self.takeover.reload_if_unavailable(now),
            Some(_) => self.takeover.unavailable().is_none(),
        };
        if readable {
            self.decide_startup(true);
            return;
        }
        let failures = self.state_retry.map_or(0, |(n, _)| n) + 1;
        if self.state_retry.is_none_or(|(_, at)| now >= at) {
            let at = now + retry_delay(failures);
            self.state_retry = Some((failures, at));
            self.warn(format!(
                "狀態檔暫時讀不到（第 {failures} 次）：延後啟動判定，{} 再讀；期間不接管、不渲染",
                self.fmt_time(at)
            ));
        }
    }

    /// 做啟動判定（狀態檔已讀到）：記錄接管狀態；有「還原進行中」標記就續做還原（4.7b 裁決 3）。
    /// `delayed`＝啟動時讀不到、現在才讀到：主題為「不接管」時補做啟動時本來就會做的還原。
    fn decide_startup(&mut self, delayed: bool) {
        self.startup_decided = true;
        self.state_retry = None;
        if delayed {
            self.resume = resume_from(&self.takeover, self.pending_reselect);
            self.info("狀態檔已讀到：做延後的啟動判定");
            if std::mem::take(&mut self.pending_reopen) {
                // 狀態檔讀不到期間使用者重新開啟了接管（主題從「不接管」改回某個主題；4.9）：讀到時補做
                // 基準重設。只是換了主題（A → B）不算，基準沿用（審查 L1）。
                self.reset_valve_baseline_on_reopen();
            }
        }
        self.log_startup_takeover_state();
        if let Some(resume) = self.resume.clone() {
            self.info(format!(
                "還原中斷續做：狀態檔有「還原進行中」標記（原因 {:?}、開始於 {}）→ 先續做還原；完成前不接管、不渲染、不做讓位判定",
                resume.reason,
                self.fmt_time(resume.started_at)
            ));
            self.restore_now(resume.reason);
        } else if delayed && self.last_theme == Some(WallpaperTheme::None) {
            self.restore_now(RestoreReason::NotTakeover);
        }
    }

    /// 啟動時的接管狀態（4.7b 當機後重啟的記錄）：接管中、沒有標記＝上次沒有經過還原就結束了
    /// （當機、工作階段結束、被終止），照常接管並重畫。
    fn log_startup_takeover_state(&self) {
        if self.resume.is_some() {
            return; // 續做另記一行。
        }
        let line = match self.takeover.state() {
            Some(s) if s.status == TakeoverStatus::TakenOver => {
                "啟動：狀態檔為接管中、沒有「還原進行中」標記（上次結束時沒有還原：當機、工作階段結束或被終止）→ 照常接管並重畫"
            }
            Some(_) => "啟動：狀態檔為未接管",
            None => return, // 封鎖、暫時讀不到：new() 已記。
        };
        self.info(line);
    }

    /// 工作階段結束（4.7b，spec「還原原桌布」：工作階段結束不還原）：只記錄並停止新的渲染。狀態檔
    /// 每次變更都已同步寫入（`write_durable`），沒有延後的落盤要做；這裡不呼叫任何桌布 API。
    fn on_session_ending(&mut self) {
        if self.session_ending {
            return;
        }
        self.session_ending = true;
        self.session_ending_since = Some(self.ports.now());
        self.session_retry_logged = false;
        let status = match self.takeover.state() {
            Some(s) if s.status == TakeoverStatus::TakenOver => "接管中",
            Some(_) => "未接管",
            None => "不可用（封鎖或暫時讀不到）",
        };
        self.info(format!(
            "工作階段結束（登出／關機／重新開機／Restart Manager）：不還原原桌布，狀態檔維持{status}（每次變更都已同步寫入，沒有延後的落盤）；停止新的渲染與桌布 API 呼叫"
        ));
    }

    fn on_session_end_cancelled(&mut self) {
        if !self.session_ending {
            return;
        }
        self.session_ending = false;
        self.session_ending_since = None;
        self.info("工作階段結束已取消：恢復排程");
    }

    fn check_session_end_stale(&mut self) {
        let Some(since) = self.session_ending_since else {
            return;
        };
        let limit = i64::try_from(self.config.session_end_stale.as_secs()).unwrap_or(i64::MAX);
        let elapsed = self.ports.now() - since;
        if self.session_ending && elapsed >= limit {
            self.session_ending = false;
            self.session_ending_since = None;
            self.warn(format!(
                "工作階段結束通知後 {} 秒行程仍在執行（可能錯過取消通知）：恢復排程",
                elapsed
            ));
        }
    }

    /// 系統時間與時區（契約 5）：倒退或時區改變時清除排程器的時點紀錄。
    fn check_clock(&mut self, event: bool) {
        let wall = self.ports.now();
        let mono = self.ports.monotonic();
        let (p1, p2) = *self.tz_probes.get_or_insert_with(|| {
            let year = CivilDate::from_days(wall.div_euclid(86_400)).year;
            let probe = |month| {
                CivilDate {
                    year,
                    month,
                    day: 15,
                }
                .to_days()
                    * 86_400
                    + 12 * 3600
            };
            (probe(1), probe(7))
        });
        let tz = self.ports.tz();
        let fingerprint = (tz.offset_at(p1), tz.offset_at(p2));
        if let Some(prev) = self.clock {
            let elapsed = i64::try_from(mono.saturating_sub(prev.mono).as_secs()).unwrap_or(0);
            let drift = wall - (prev.wall + elapsed);
            let tol = self.config.clock_jump_tolerance_secs;
            if fingerprint != prev.fingerprint {
                self.scheduler.clear_records();
                self.info(format!(
                    "時區改變（參考偏移 {:?} → {:?} 秒）：清除時點紀錄，每台以新時區重畫",
                    prev.fingerprint, fingerprint
                ));
            } else if drift < -tol {
                self.scheduler.clear_records();
                self.info(format!(
                    "系統時間往回調了 {} 秒：清除時點紀錄（時點以嚴格較新比較，不清會延後重畫）",
                    -drift
                ));
            } else if drift > tol {
                self.info(format!(
                    "系統時間往前跳了 {drift} 秒（睡眠喚醒或校時），照常評估"
                ));
            } else if event {
                self.info(format!(
                    "系統時間變更通知：沒有倒退或時區改變（差 {drift} 秒），不清紀錄"
                ));
            }
        }
        self.clock = Some(ClockAnchor {
            wall,
            mono,
            fingerprint,
        });
    }

    /// 主題變更（契約 6）：通知狀態機；切到「不接管」就還原原桌布（此時沒有進行中的 Redraw）。
    /// 每次處理喚醒與每次評估前都呼叫（修正輪 1，審查 low 5）：設定已改而 `SettingsChanged` 還沒到時，
    /// 主題變更仍先交給狀態機，再做排程決策。`announce`＝主題沒變時也記一行（設定變更事件）。
    fn sync_theme(&mut self, theme: WallpaperTheme, announce: bool) {
        if self.last_theme == Some(theme) {
            if announce {
                self.info(format!("設定變更：主題未變（{}）", theme.as_str()));
            }
            return;
        }
        self.info(format!(
            "主題：{} → {}",
            self.last_theme.map_or("（啟動）", WallpaperTheme::as_str),
            theme.as_str()
        ));
        let startup = self.last_theme.is_none();
        let reopened =
            self.last_theme == Some(WallpaperTheme::None) && theme != WallpaperTheme::None;
        self.last_theme = Some(theme);
        self.takeover.theme_selected(theme);
        if reopened && self.startup_decided {
            self.reset_valve_baseline_on_reopen();
        }
        if !self.startup_decided {
            // 狀態檔還讀不到（審查 F1）：不還原、不放棄任何標記；改選主題排在啟動判定（與可能的續做）
            // 之後才生效。
            if !startup {
                self.pending_reselect = theme != WallpaperTheme::None;
                // 4.9 修正輪 1（審查 L1）：只有「不接管 → 某個主題」才是重新開啟接管；A → B 不算。
                // 期間又改回「不接管」就作廢（之後再選主題時重新判定）。
                self.pending_reopen =
                    theme != WallpaperTheme::None && (self.pending_reopen || reopened);
                self.info("狀態檔還讀不到：主題變更排在啟動判定之後（有未完成的還原時先續做）");
            }
            return;
        }
        if let Some(resume) = &mut self.resume {
            // 續做中（4.7b）：續做本身就是還原，不另外還原、不清標記。啟動後改選某個主題＝使用者
            // 重新選了主題，續做完成後照常接管。
            if !startup && theme != WallpaperTheme::None {
                resume.reselected = true;
                self.info(format!(
                    "續做還原期間使用者選了主題 {}：續做完成後照常接管",
                    theme.as_str()
                ));
            }
            return;
        }
        if theme == WallpaperTheme::None && self.session_ending {
            // 工作階段結束中不還原（4.7b）：排成立即到期的重試，結束被取消時才執行；工作階段真的
            // 結束時，下次啟動主題為「不接管」照常還原。
            self.info("工作階段結束中：切到「不接管」的還原延到工作階段結束取消後");
            self.restore_retry = Some(RestoreRetry {
                reason: RestoreReason::NotTakeover,
                failures: 0,
                at: self.ports.now(),
                recovery_used: false,
            });
        } else if theme == WallpaperTheme::None {
            self.restore_retry = None;
            self.restore_now(RestoreReason::NotTakeover);
        } else if self.takeover.restore_marker().is_some() {
            // 「不接管」的還原沒做完（重試中）使用者又選回主題：放棄那次還原（4.7b），否則重啟時會
            // 被當成還原中斷而續做。
            //
            // 不變式（複審 1：原本的 `restore_attempted` 閘是多餘的，已拿掉）：走到這裡時，記憶體中的
            // 標記只可能是本次執行的 `mark_restore_started` 寫的——啟動時讀到的「接管中＋標記」一定成為
            // `resume`（上面已 return，續做完成前不會放棄）；啟動判定之前也已 return；判定之後狀態不會
            // 再從磁碟重讀（`Ready`／`Blocked` 不會變回 `Unavailable`）；外部要求的標記只寫磁碟、不進
            // 記憶體，放棄時也會合併保留（`persist_marker_only`）。
            self.restore_retry = None;
            match self.takeover.abandon_restore_marker() {
                Ok(_) => {
                    self.info("使用者選回主題：放棄未完成的還原，清除「還原進行中」標記，照常接管")
                }
                Err(e) => self.warn(format!(
                    "使用者選回主題：清除「還原進行中」標記時寫入狀態檔失敗（{e}）；記憶體中已清除"
                )),
            }
        }
    }

    /// 使用者重新開啟接管（主題從「不接管」改回某個主題；4.9、design.md D11）：清掉安全閥基準，
    /// 下一次設定前的讀值成為新基準。
    fn reset_valve_baseline_on_reopen(&mut self) {
        match self.takeover.clear_explorer_baseline() {
            Ok(true) => {
                self.info("使用者重新開啟接管：清除安全閥基準，下一次設定桌布前的讀值成為新基準")
            }
            Ok(false) => self
                .info("使用者重新開啟接管：沒有要清除的安全閥基準，下一次設定桌布前的讀值成為基準"),
            Err(e) => self.warn(format!(
                "使用者重新開啟接管：清除安全閥基準時寫入狀態檔失敗（{e}）；記憶體中已清除"
            )),
        }
    }

    /// 狀態機要求的副作用（主題改不接管、通知）。
    fn apply_events(&mut self, events: Vec<TakeoverEvent>) {
        for event in events {
            match event {
                TakeoverEvent::ThemeSetToNone { cause } => {
                    self.warn(format!("狀態機要求主題改為「不接管」（{cause:?}）"));
                    let saved = self.ports.set_theme_none(cause);
                    self.note_theme_saved(saved);
                    self.takeover.theme_selected(WallpaperTheme::None);
                    self.last_theme = Some(WallpaperTheme::None);
                    self.restore_retry = None;
                }
                TakeoverEvent::Notify(notice) => {
                    self.warn(format!("通知使用者：{notice:?}"));
                    self.ports.notify_user(&notice);
                    self.publish(|s| {
                        s.notices.push(notice);
                        // 只保留最近幾筆（修正輪 2）：通知在發生當下已顯示，這裡只供查詢。
                        let excess = s.notices.len().saturating_sub(MAX_STATUS_NOTICES);
                        s.notices.drain(..excess);
                    });
                }
            }
        }
    }

    // ── 螢幕 ─────────────────────────────────────────────────────────────────────────────

    fn stable_displays(&self) -> Vec<StableDisplay> {
        self.displays
            .iter()
            .filter_map(|d| {
                d.device_path.as_ref().map(|p| StableDisplay {
                    device_path: p.clone(),
                    rect: d.rect,
                })
            })
            .collect()
    }

    /// 需要穩定識別（螢幕鍵）的操作前呼叫：顯示器清單未知就先取一次。
    fn ensure_displays(&mut self) {
        if self.displays.is_empty() {
            self.displays = self.ports.displays();
        }
    }

    /// 顯示變更（契約 4）或待還原重試到期：處理待還原的螢幕（未接管時才有作用；沒有待還原者不碰
    /// COM）。有螢幕處理失敗（例如插回後 explorer 還沒追上、讀不到目前桌布）時依退避排
    /// `pending_retry`——之後不一定還有顯示變更事件（審查 High-1）；仍離線者等顯示變更，不排重試。
    fn process_pending_monitors(&mut self, cause: &str) {
        let pending = self
            .takeover
            .state()
            .is_some_and(|s| s.monitors.values().any(|r| r.pending_restore));
        if !pending {
            // 狀態檔讀不到只會發生在啟動判定之前（讀到之後狀態機不會再回到「讀不到」）：這時不知道有沒有
            // 待還原，延後的啟動判定讀到狀態檔時會處理（主題「不接管」時走還原），不另排重試。
            if self.takeover.unavailable().is_some() {
                self.info(format!(
                    "{cause}：狀態檔暫時讀不到，待還原螢幕等狀態檔讀到後處理"
                ));
            } else {
                self.info(format!("{cause}：沒有待還原的螢幕"));
            }
            self.pending_retry = None;
            return;
        }
        self.displays = self.ports.displays();
        let stable = self.stable_displays();
        let mut io = TakeoverIo {
            wallpaper: &self.service,
            registry: &mut *self.registry,
            stable_displays: &stable,
            files: &*self.files,
            now: self.ports.now(),
        };
        let report = self.takeover.restore_pending_monitors(&mut io);
        self.info(format!(
            "{cause}：處理待還原螢幕 → 還原 {:?}、清除標記（已不是宿主的圖） {:?}、仍顯示宿主的圖（無法還原、不再自動處理） {:?}、仍離線 {:?}、失敗 {:?}、已存檔 {}",
            report.restored,
            report.cleared,
            report.left_showing_host,
            report.still_offline,
            report.failed,
            report.state_saved
        ));
        if report.failed.is_empty() {
            self.pending_retry = None;
        } else {
            let failures = self.pending_retry.map_or(0, |(n, _)| n).saturating_add(1);
            let at = self.ports.now() + retry_delay(failures);
            self.pending_retry = Some((failures, at));
            self.warn(format!(
                "待還原螢幕處理失敗（第 {failures} 次）：{} 再試（插回後 explorer 可能還沒追上）",
                self.fmt_time(at)
            ));
        }
    }

    /// 主題不是「不接管」時，評估前確認螢幕清單是新的：顯示器清單（Tauri）改變、或被標記要重新
    /// 列舉時，經 COM 列舉一次（失敗沿用上次的清單、但不以它的座標出圖設定，並以 `list_retry`
    /// 退避再列舉）。
    fn refresh_monitors(&mut self) {
        // COM 忙碌時列舉一定回 Busy：不列舉、不查顯示器、同一段忙碌只記一次（修正輪 1，審查 low 3）。
        // 忙碌解除後的評估再列舉（標記保留）。
        if self.service.is_busy() {
            if !self.list_deferred_logged {
                self.list_deferred_logged = true;
                self.info("桌布 COM 執行緒忙碌，延後列舉螢幕（忙碌解除後再列舉）");
            }
            return;
        }
        self.list_deferred_logged = false;
        let displays = self.ports.displays();
        if displays != self.displays {
            self.info(format!(
                "顯示器清單改變：{}",
                displays
                    .iter()
                    .map(|d| format!(
                        "{}×{}@({},{}) DPI {}",
                        d.rect.width, d.rect.height, d.rect.x, d.rect.y, d.dpi
                    ))
                    .collect::<Vec<_>>()
                    .join("；")
            ));
            self.displays = displays;
            self.monitors_dirty = true;
        }
        if !self.monitors_dirty && self.com_monitors.is_some() {
            return;
        }
        match self.service.list_monitors() {
            Ok(list) => {
                self.info(format!(
                    "列舉螢幕（IDesktopWallpaper）：{} 台，在線 {} 台：{}",
                    list.len(),
                    list.iter().filter(|m| m.is_online()).count(),
                    list.iter()
                        .map(|m| match m.rect {
                            Some(r) => format!(
                                "{} {}×{}@({},{})",
                                m.device_path, r.width, r.height, r.x, r.y
                            ),
                            None => format!("{}（離線）", m.device_path),
                        })
                        .collect::<Vec<_>>()
                        .join("；")
                ));
                self.com_monitors = Some(list);
                self.monitors_dirty = false;
                self.last_list_error = None;
                self.list_retry = None;
                if self.cleanup_pending {
                    self.cleanup_pending = false;
                    self.cleanup_outputs();
                }
            }
            Err(e) => {
                if matches!(
                    e,
                    WallpaperError::Timeout { .. } | WallpaperError::Busy { .. }
                ) {
                    self.note_busy();
                }
                // 退避重試列舉：沒有要重畫的螢幕時也不能等到下一個時點（清單可能已變）。
                let failures = self.list_retry.map_or(0, |(n, _)| n).saturating_add(1);
                let at = self.ports.now() + retry_delay(failures);
                self.list_retry = Some((failures, at));
                let msg = e.to_string();
                if self.last_list_error.as_deref() != Some(msg.as_str()) {
                    self.warn(format!(
                        "列舉螢幕失敗：{msg}；沿用上次的清單（不以舊座標出圖），第 {failures} 次，{} 再列舉（相同錯誤不再重記）",
                        self.fmt_time(at)
                    ));
                    self.last_list_error = Some(msg);
                }
            }
        }
    }

    /// 在線螢幕（排程器輸入）：`IDesktopWallpaper` 的裝置路徑與矩形，DPI 依矩形對到顯示器清單。
    ///
    /// DPI 以 COM 的矩形直接查（[`CoordinatorPorts::dpi_for_rect`]；修正輪 1，審查 low 4），查不到才
    /// 退回顯示器清單裡矩形相同者，再查不到記一次警告並以 96 計。
    fn online_snapshots(&mut self) -> Vec<MonitorSnapshot> {
        let Some(list) = self.com_monitors.clone() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for m in list {
            let Some(r) = m.rect else {
                continue;
            };
            let (Some(width), Some(height)) = (
                u32::try_from(r.width).ok().filter(|w| *w > 0),
                u32::try_from(r.height).ok().filter(|h| *h > 0),
            ) else {
                continue;
            };
            let rect_key = (r.x, r.y, r.width, r.height);
            let dpi = self
                .ports
                .dpi_for_rect(r)
                .or_else(|| self.displays.iter().find(|d| d.rect == r).map(|d| d.dpi));
            let dpi = match dpi {
                Some(dpi) => {
                    self.dpi_warned.remove(&rect_key);
                    dpi
                }
                None => {
                    if self.dpi_warned.insert(rect_key) {
                        self.warn(format!(
                            "無法取得 DPI：螢幕 {} 矩形 {}×{}@({},{})；以 {DEFAULT_DPI} 計（縮放變更將無法觸發這台重畫）",
                            m.device_path, r.width, r.height, r.x, r.y
                        ));
                    }
                    DEFAULT_DPI
                }
            };
            out.push(MonitorSnapshot {
                device_path: m.device_path,
                geometry: MonitorGeometry { width, height, dpi },
            });
        }
        out
    }

    fn monitor_rect(&self, device_path: &str) -> Option<PhysicalRect> {
        self.com_monitors
            .as_ref()?
            .iter()
            .find(|m| path_eq(&m.device_path, device_path))?
            .rect
    }

    /// 清除已拔除螢幕的殘留輸出檔（契約 4；4.5 只清每台自己的殘檔）。保留：在線螢幕的鍵、狀態檔
    /// 裡任何紀錄的鍵（含離線螢幕——它可能仍顯示那張圖）、任何紀錄引用的路徑。狀態檔不可用時不清。
    fn cleanup_outputs(&mut self) {
        let Some(list) = &self.com_monitors else {
            return;
        };
        let Some(state) = self.takeover.state() else {
            self.info("狀態檔不可用，略過殘留輸出檔清理");
            return;
        };
        let stable = self.stable_displays();
        let mut keep_keys: HashSet<String> = list
            .iter()
            .filter(|m| m.is_online())
            .map(|m| monitor_key(&m.device_path, m.rect, &stable))
            .collect();
        let mut keep_paths = Vec::new();
        for (key, rec) in &state.monitors {
            keep_keys.insert(key.clone());
            keep_paths.extend(rec.last_set.clone());
            keep_paths.push(rec.original_wallpaper.clone());
        }
        for path in stale_output_files(&self.paths.output_dir, &keep_keys, &keep_paths) {
            match fs::remove_file(&path) {
                Ok(()) => self.info(format!("清除已拔除螢幕的殘留輸出檔 {}", path.display())),
                Err(e) => self.warn(format!("清除殘留輸出檔 {} 失敗：{e}", path.display())),
            }
        }
    }

    // ── 評估 ─────────────────────────────────────────────────────────────────────────────

    /// 評估一次；要重畫時回傳計畫。`quiet`＝純計時器喚醒（決策沒變就不記）。
    fn evaluate(&mut self, quiet: bool) -> Option<RedrawPlan> {
        if let Some(reason) = self.takeover.blocked() {
            let reason = format!("{reason:?}");
            if !quiet {
                self.log_at(
                    Level::Error,
                    format!("狀態檔封鎖中（{reason}）：不排程渲染"),
                );
            }
            self.next_wake = None;
            self.publish(|s| s.state = CoordinatorState::Blocked(reason));
            return None;
        }
        let now = self.ports.now();
        let inputs = self.ports.inputs();
        let theme = inputs.theme;
        // 決策用的主題一定已交給狀態機（修正輪 1，審查 low 5）。
        self.sync_theme(theme, false);
        // 工作階段結束中、續做還原中（4.7b）：不列舉螢幕、不渲染。
        let hold = if self.session_ending {
            Some(CoordinatorState::SessionEnding)
        } else if !self.startup_decided {
            Some(CoordinatorState::WaitingForStateFile)
        } else if self.resume.is_some() {
            Some(CoordinatorState::ResumingRestore)
        } else {
            None
        };
        if let Some(state) = hold {
            let retry_at = if self.startup_decided {
                self.restore_retry.as_ref().map(|r| r.at)
            } else {
                self.state_retry.map(|(_, at)| at)
            };
            let save_at = self.theme_save_retry.map(|(_, at)| at);
            self.next_wake = match (if self.session_ending { None } else { retry_at }, save_at) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            let line = format!("{state:?}；下次喚醒 {}", self.fmt_wake(self.next_wake));
            if !quiet || line != self.last_decision {
                self.info(format!("決策：{line}"));
            }
            self.last_decision = line;
            self.publish(|s| s.state = state);
            return None;
        }
        if theme != WallpaperTheme::None {
            self.refresh_monitors();
        }
        let monitors = if theme == WallpaperTheme::None {
            Vec::new()
        } else {
            self.online_snapshots()
        };
        let mut decision = self.decide(now, &inputs, &monitors, ExplorerResources::NotSampled);
        let dnd = matches!(decision.action, Action::Redraw(_)) && self.ports.do_not_disturb();
        // 狀態機一定會拒絕設定的階段：不渲染（修正輪 1，審查 medium 2）。等焦點確認時只等
        // SpotlightConfirmed／SpotlightCancelled；停止接管（讓位、安全閥）在主題改回某個主題時才解除。
        let refusing_phase = match self.takeover.phase() {
            Phase::AwaitingSpotlightConfirmation if theme != WallpaperTheme::None => {
                Some(CoordinatorState::AwaitingSpotlightConfirmation)
            }
            Phase::Stopped if theme != WallpaperTheme::None => Some(CoordinatorState::Idle),
            _ => None,
        };
        // 4.9 安全閥（排程器契約 7）：確定要渲染、設定桌布時才讀 explorer，再評估一次（純函式）。
        if matches!(decision.action, Action::Redraw(_)) && !dnd && refusing_phase.is_none() {
            let explorer = self.sample_explorer();
            decision = self.decide(now, &inputs, &monitors, explorer);
            if let Some(rebaseline) = decision.rebaseline {
                self.record_valve_baseline(rebaseline, now);
            } else if explorer == ExplorerResources::ReadFailed
                && !self.takeover.already_taken_over()
            {
                // 接管前讀不到：清掉未接管時留下的舊基準（例如上一次嘗試接管時寫入），否則接管開始時
                // 會被帶進新的接管狀態。接管後第一次讀到的值才是基準。
                if let Err(e) = self.takeover.clear_explorer_baseline() {
                    self.warn(format!(
                        "接管前清除舊的安全閥基準時寫入狀態檔失敗（記憶體中已清除）：{e}"
                    ));
                }
            }
        }
        self.scheduler.apply_forget(&decision);
        if !decision.forget.is_empty() {
            self.info(format!("排程器移除紀錄：{:?}", decision.forget));
        }
        let mut trip: Option<SafetyValveTrip> = None;
        let (state, plan, wake) = match (refusing_phase, decision.action) {
            (Some(refused), _) => (refused, None, None),
            (None, action) => match action {
                Action::Idle => (CoordinatorState::Idle, None, None),
                Action::WaitingForData { missing } => {
                    (CoordinatorState::WaitingForData(missing), None, None)
                }
                Action::HoldWithoutData { missing } => {
                    if !self.hold_warned {
                        self.hold_warned = true;
                        self.warn(format!(
                        "已接管但主題 {} 需要的資料 {missing:?} 不見了：保留目前的桌布，不重畫、不還原",
                        theme.as_str()
                    ));
                    }
                    (CoordinatorState::HoldWithoutData(missing), None, None)
                }
                Action::Paused => (CoordinatorState::Paused, None, None),
                Action::UpToDate if !decision.pending_retries.is_empty() => {
                    (CoordinatorState::RetryPending, None, decision.next_wake)
                }
                Action::UpToDate => (CoordinatorState::UpToDate, None, decision.next_wake),
                Action::Redraw(_) if dnd => (CoordinatorState::DoNotDisturb, None, None),
                Action::Redraw(plan) => {
                    (CoordinatorState::Redrawing, Some(plan), decision.next_wake)
                }
                Action::StopTakeover(t) => {
                    trip = Some(t);
                    (CoordinatorState::Idle, None, None)
                }
            },
        };
        if !matches!(state, CoordinatorState::HoldWithoutData(_)) {
            self.hold_warned = false;
        }
        if let Some(t) = &trip {
            self.trip_safety_valve(t);
        }
        // 螢幕清單沿用上次（列舉失敗）：到期再列舉一次。只取未來的時刻——忙碌而延後列舉時重試時刻
        // 可能已過，那時由忙碌解除事件觸發評估，不排過去的喚醒（否則會空轉）。
        let list_retry_at = self.list_retry.map(|(_, at)| at).filter(|at| {
            theme != WallpaperTheme::None
                && self.monitors_dirty
                && *at > now
                && matches!(
                    state,
                    CoordinatorState::UpToDate
                        | CoordinatorState::RetryPending
                        | CoordinatorState::Redrawing
                )
        });
        let state = match state {
            CoordinatorState::UpToDate if list_retry_at.is_some() => CoordinatorState::RetryPending,
            other => other,
        };
        let wake = match (wake, list_retry_at) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        let retry_at = self.retry_wake();
        self.next_wake = match (wake, retry_at) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        let summary = match &plan {
            None if trip.is_some() => "安全閥觸發：停止接管（已走「不接管」的還原路徑）".to_owned(),
            Some(p) => format!(
                "Redraw 主題 {} 時點 {:?} 呈現 {}：{}",
                p.theme.as_str(),
                p.stamp,
                self.fmt_time(p.as_of),
                p.targets
                    .iter()
                    .map(|t| format!("{}（{:?}）", t.monitor.device_path, t.reason))
                    .collect::<Vec<_>>()
                    .join("、")
            ),
            None => format!("{state:?}"),
        };
        let mut summary = summary;
        if !decision.pending_retries.is_empty() {
            summary.push_str(&format!(
                "；等待重試：{}",
                decision
                    .pending_retries
                    .iter()
                    .map(|r| format!(
                        "{}（{}，第 {} 次）於 {}",
                        r.device_path,
                        retry_kind_label(r.kind),
                        r.failures,
                        self.fmt_time(r.at)
                    ))
                    .collect::<Vec<_>>()
                    .join("、")
            ));
        }
        if theme != WallpaperTheme::None && self.monitors_dirty && self.com_monitors.is_some() {
            summary.push_str("；螢幕清單沿用上次（列舉失敗或延後），不以舊座標出圖");
        }
        if let Some(at) = list_retry_at {
            summary.push_str(&format!("；{} 再列舉螢幕", self.fmt_time(at)));
        }
        let line = format!("{summary}；下次喚醒 {}", self.fmt_wake(self.next_wake));
        if !quiet || plan.is_some() || line != self.last_decision {
            self.info(format!("決策：{line}"));
        }
        self.last_decision = line;
        self.publish(|s| s.state = state);
        plan
    }

    /// 一次排程評估（[`Self::evaluate`] 以不同的 explorer 讀值呼叫兩次；純函式）。
    fn decide(
        &self,
        now: UnixSeconds,
        inputs: &HostInputs,
        monitors: &[MonitorSnapshot],
        explorer: ExplorerResources,
    ) -> Decision {
        next_action(
            &self.scheduler,
            &SchedulerInput {
                now,
                tz: self.ports.tz(),
                theme: inputs.theme,
                pause: &inputs.pause,
                data: inputs.data,
                monitors,
                already_taken_over: self.takeover.already_taken_over(),
                explorer,
            },
        )
    }

    // ── explorer 資源安全閥（4.9）─────────────────────────────────────────────────────────

    /// 讀一次 explorer 的 GDI 物件數（即將設定桌布之前）。讀到就記一行（讀值、基準、門檻）；讀取
    /// 失敗照常設定，同一段連續失敗只記一次 warn（比照 busy 跳過的作法），恢復時記一行。
    fn sample_explorer(&mut self) -> ExplorerResources {
        match self.ports.explorer_gdi() {
            Ok(reading) => {
                if self.explorer_read_failing {
                    self.explorer_read_failing = false;
                    self.info("讀取 explorer GDI 恢復");
                }
                let baseline = self.takeover.valve_baseline();
                let base = match baseline {
                    Some(b) => format!("{}（PID {}）", b.gdi, b.pid),
                    None => "無".to_owned(),
                };
                self.info(format!(
                    "explorer GDI：PID {} 讀值 {}；基準 {base}；門檻 增量超過 {EXPLORER_GDI_DELTA_LIMIT} 或絕對值超過 {EXPLORER_GDI_ABSOLUTE_LIMIT}",
                    reading.pid, reading.gdi
                ));
                ExplorerResources::Sampled { reading, baseline }
            }
            Err(e) => {
                if !self.explorer_read_failing {
                    self.explorer_read_failing = true;
                    self.warn(format!(
                        "讀取 explorer GDI 失敗（{e}）：照常設定桌布、不停止接管（同一段連續失敗只記這一次）"
                    ));
                }
                ExplorerResources::ReadFailed
            }
        }
    }

    /// 排程器決定的新基準寫進狀態檔（接管開始、explorer 重新啟動、重新開啟接管後的第一次讀值）。
    fn record_valve_baseline(&mut self, rebaseline: ExplorerRebaseline, now: UnixSeconds) {
        let b = rebaseline.baseline;
        let why = match rebaseline.cause {
            RebaselineCause::NoBaseline => {
                "沒有基準（接管開始、重新開啟接管或舊狀態檔）".to_owned()
            }
            RebaselineCause::PidChanged { previous } => format!(
                "explorer PID 改變（{} → {}，舊基準 {}）",
                previous.pid, b.pid, previous.gdi
            ),
        };
        match self.takeover.set_explorer_baseline(b, now) {
            Ok(true) => self.info(format!("安全閥基準設為 PID {} GDI {}：{why}", b.pid, b.gdi)),
            Ok(false) => self.warn(format!(
                "安全閥基準 PID {} GDI {}（{why}）未記錄：狀態檔不可用",
                b.pid, b.gdi
            )),
            Err(e) => self.warn(format!(
                "安全閥基準 PID {} GDI {}（{why}）寫入狀態檔失敗（記憶體中已更新）：{e}",
                b.pid, b.gdi
            )),
        }
    }

    /// 安全閥觸發（design.md D11）：記一行含讀值、基準、PID、門檻與決策，走「不接管」的還原路徑
    /// （`RestoreReason::SafetyValve`：還原原桌布與全域填滿方式、主題改「不接管」、通知、排程器
    /// `reset`）。此時沒有進行中的 Redraw（評估在兩次 Redraw 之間）。
    fn trip_safety_valve(&mut self, trip: &SafetyValveTrip) {
        let base = match trip.baseline {
            Some(b) => format!(
                "基準 {}（PID {}）、增量 {}",
                b.gdi,
                b.pid,
                trip.delta().unwrap_or(0)
            ),
            None => "尚無基準".to_owned(),
        };
        self.warn(format!(
            "安全閥觸發：explorer PID {} GDI 讀值 {}、{base}；門檻 增量超過 {EXPLORER_GDI_DELTA_LIMIT} 或絕對值超過 {EXPLORER_GDI_ABSOLUTE_LIMIT}（增量超過 {}、絕對值超過 {}）→ 決策：停止接管——不設定桌布，還原原桌布與填滿方式、主題改「不接管」並通知",
            trip.reading.pid,
            trip.reading.gdi,
            trip.delta_exceeded,
            trip.absolute_exceeded
        ));
        self.restore_now(RestoreReason::SafetyValve {
            detail: trip.detail(),
        });
    }

    // ── 執行 Redraw ──────────────────────────────────────────────────────────────────────

    /// 這份計畫要不要在下一台之前中止。
    fn abort_reason(&self, plan: &RedrawPlan, inbox: &dyn Inbox) -> Option<String> {
        if inbox.shutdown_requested() {
            return Some("宿主結束".to_owned());
        }
        if inbox.restore_requested() {
            return Some("收到還原要求（系統匣結束或命令列）".to_owned());
        }
        if self.session_ending || inbox.session_end_pending() {
            return Some("工作階段結束".to_owned());
        }
        let inputs = self.ports.inputs();
        let theme = inputs.theme;
        if theme != plan.theme {
            return Some(format!("設定的主題已改為 {}", theme.as_str()));
        }
        match self.takeover.phase() {
            Phase::Stopped => return Some("狀態機已停止接管（讓位或安全閥）".to_owned()),
            Phase::AwaitingSpotlightConfirmation => {
                return Some("等使用者確認焦點（確認前不渲染）".to_owned())
            }
            Phase::Idle | Phase::SpotlightConfirmed => {}
        }
        // task 6.4（審查 R2b-L3）：重畫途中進入暫停（鎖定、全螢幕、簡報……）或勿打擾：其餘螢幕中止（已設定
        // 的保留；中止的不記錄，解除後的評估依既有規則補畫）。勿打擾在本來就要重畫時才查，這裡沿用同一個
        // 查詢（正常情況回快取、不等待）。
        if let Some(reason) = inputs
            .pause
            .iter()
            .find(|r| crate::wallpaper::pauses_wallpaper(**r))
        {
            return Some(format!("進入暫停（{reason:?}）"));
        }
        if self.ports.do_not_disturb() {
            return Some("進入勿打擾".to_owned());
        }
        None
    }

    fn note_busy(&mut self) {
        let now = self.ports.now();
        self.busy_since.get_or_insert(now);
        self.last_busy = true;
    }

    /// 連續 busy 跳過跨滿 [`BUSY_SKIP_WARN_SLOTS`] 個時點時記一次 warn（契約 3）。
    fn check_busy_warn(&mut self) {
        let n = self.scheduler.consecutive_busy_skip_slots();
        if n < BUSY_SKIP_WARN_SLOTS {
            self.busy_warned = false;
            return;
        }
        if !self.busy_warned {
            self.busy_warned = true;
            let since = self.fmt_wake(self.busy_since);
            self.warn(format!(
                "桌布 COM 執行緒已連續 {n} 個時點忙碌（自 {since} 起），桌布停止更新：explorer 可能長期卡住"
            ));
        }
    }

    fn execute(&mut self, plan: &RedrawPlan, inbox: &dyn Inbox) {
        self.info(format!(
            "Redraw 開始：主題 {}、時點 {:?}、呈現 {}、{} 台",
            plan.theme.as_str(),
            plan.stamp,
            self.fmt_time(plan.as_of),
            plan.targets.len()
        ));
        self.takeover.redraw_started();
        let stable = self.stable_displays();
        let output_dir = self.paths.output_dir.clone();
        let mut stop: Option<(String, usize)> = None;
        // a/b 選格要避開「螢幕正在顯示的那一格」（task 6.4，審查 R2a-L1）：以讀回確認的路徑為準，不用狀態檔
        // 的 `last_set`（那是「準備設定」的路徑——設定失敗時螢幕仍是另一格）。第一台要渲染時讀一次。
        let mut shown: Option<Vec<(String, Option<String>)>> = None;

        for (index, target) in plan.targets.iter().enumerate() {
            if let Some(why) = self.abort_reason(plan, inbox) {
                stop = Some((why, index));
                break;
            }
            let dev = target.monitor.device_path.clone();
            let geometry = target.monitor.geometry;

            // 預檢：COM 執行緒忙碌就不渲染、不呼叫桌布 API（契約 2、3）。
            if self.service.is_busy() {
                self.note_busy();
                let now = self.ports.now();
                let recorded = self
                    .scheduler
                    .record_failed(plan, target, FailureKind::Busy, now);
                let since = self.fmt_wake(self.busy_since);
                self.info(format!(
                    "預檢跳過：桌布 COM 執行緒忙碌（自 {since} 起），不渲染、不呼叫桌布 API；螢幕 {dev}、時點 {:?}（記錄 Busy：{recorded}）",
                    plan.stamp
                ));
                self.check_busy_warn();
                continue;
            }

            // 預檢：螢幕清單不是最新（這次評估的列舉失敗，或 COM 忙碌而延後、評估後才解除）就不渲染、
            // 不設定——沿用的上次清單座標可能已不對（2026-10-06 在場驗收：拔掉一台後以舊座標
            // @(3840,4) 查 DPI、出圖再設定）。記 ApplyError：依退避短間隔重試，重試前的評估會再列舉一次。
            if self.monitors_dirty {
                let now = self.ports.now();
                let recorded =
                    self.scheduler
                        .record_failed(plan, target, FailureKind::ApplyError, now);
                self.warn(format!(
                    "預檢跳過：螢幕清單不是最新（列舉失敗或因 COM 忙碌延後，沿用的是上次的清單），不以舊座標渲染與設定；螢幕 {dev}、時點 {:?}（記錄 ApplyError：依退避短間隔重試；記錄：{recorded}）",
                    plan.stamp
                ));
                continue;
            }

            let key = monitor_key(&dev, self.monitor_rect(&dev), &stable);
            let shown = shown.get_or_insert_with(|| self.read_displayed());
            let displayed = match shown.iter().find(|(d, _)| path_eq(d, &dev)) {
                Some((_, Some(current))) => Some(current.clone()),
                _ => self.takeover.state().and_then(|s| {
                    s.monitors
                        .values()
                        .find(|r| path_eq(&r.device_path, &dev))
                        .and_then(|r| r.last_set.clone())
                }),
            };
            let req = RenderRequest {
                monitor_key: key.clone(),
                width: geometry.width,
                height: geometry.height,
                theme: plan.theme,
                as_of: plan.as_of,
                tz: String::new(),
                fixture: None,
            };
            self.info(format!(
                "渲染：螢幕 {dev}（{:?}）{}×{} DPI {} 鍵 {key}",
                target.reason, geometry.width, geometry.height, geometry.dpi
            ));
            let rendered = self.ports.render(
                &req,
                &OutputTarget {
                    dir: &output_dir,
                    displayed: displayed.as_deref().map(Path::new),
                },
            );
            let now = self.ports.now();
            let success = match rendered {
                Ok(s) => s,
                Err(f) => {
                    let recorded = self.scheduler.record_failed(plan, target, f.kind(), now);
                    self.warn(format!(
                        "渲染失敗：螢幕 {dev}：{f}；保留舊圖，記錄 {:?}（依退避重試；記錄：{recorded}）",
                        f.kind()
                    ));
                    self.check_busy_warn();
                    continue;
                }
            };
            // 渲染最長 30 秒：期間使用者可能切到「不接管」或宿主要結束——已渲染但還沒送出的不送。
            if let Some(why) = self.abort_reason(plan, inbox) {
                stop = Some((why, index));
                break;
            }

            let mut io = TakeoverIo {
                wallpaper: &self.service,
                registry: &mut *self.registry,
                stable_displays: &stable,
                files: &*self.files,
                now,
            };
            let outcome = self.takeover.set_monitor_wallpaper(
                &mut io,
                &mut self.scheduler,
                &dev,
                &success.path,
            );
            let now = self.ports.now();
            self.info(format!(
                "SetWallpaper（經狀態機）：螢幕 {dev}、圖 {} → {outcome:?}",
                success.path.display()
            ));
            match outcome {
                SetOutcome::Applied => {
                    let recorded = self.scheduler.record_drawn(plan, &target.monitor);
                    self.check_busy_warn();
                    if !recorded {
                        self.info(format!("螢幕 {dev} 的結果屬於過時的計畫，排程器忽略"));
                    }
                    // 每台在自己設定成功後約 confirm_delay 就讀回確認，不等整批（修正輪 1，審查 low 8）。
                    self.confirm_applied(&dev, &stable);
                }
                SetOutcome::SetFailed(e) | SetOutcome::ReadbackFailed(e) => {
                    let kind = apply_failure_kind(&e);
                    if matches!(
                        e,
                        WallpaperError::Timeout { .. } | WallpaperError::Busy { .. }
                    ) {
                        self.note_busy();
                    }
                    let recorded = self.scheduler.record_failed(plan, target, kind, now);
                    let when = if kind == FailureKind::ApplyError {
                        "explorer 有回應但回錯：依退避短間隔重試"
                    } else {
                        "下一個時點或設定路徑恢復時再試"
                    };
                    self.warn(format!(
                        "設定桌布失敗：螢幕 {dev}：{e}；記錄 {kind:?}（{when}；記錄：{recorded}）"
                    ));
                    self.check_busy_warn();
                }
                SetOutcome::Refused(reason) => {
                    let recorded =
                        self.scheduler
                            .record_failed(plan, target, FailureKind::Apply, now);
                    self.warn(format!(
                        "狀態機拒絕設定：螢幕 {dev}：{reason:?}；記錄 Apply（記錄：{recorded}）"
                    ));
                    self.check_busy_warn();
                    if reason == RefuseReason::MonitorNotOnline {
                        // 設定前讀回說這台不在線，與這次的螢幕清單不符：下一次評估前重新列舉（審查 Low-3）。
                        self.monitors_dirty = true;
                    }
                    if matches!(
                        reason,
                        RefuseReason::Blocked
                            | RefuseReason::Stopped
                            | RefuseReason::RestorePending
                    ) {
                        stop = Some((format!("狀態機拒絕（{reason:?}）"), index + 1));
                        break;
                    }
                }
                SetOutcome::Yielded { events } => {
                    self.apply_events(events);
                    stop = Some(("使用者自行更換桌布，已讓位".to_owned(), index + 1));
                    break;
                }
                SetOutcome::BackupFailed { events } => {
                    // task 6.4（審查 R1-M3a）：原桌布無法備份 → 不接管。套用「主題改不接管」不會觸發還原，
                    // 已接管（接管中新接上的螢幕備份不了）時自己排還原：Redraw 進行中＝排隊到它結束後執行。
                    self.apply_events(events);
                    if self.takeover.already_taken_over() {
                        self.restore_now(RestoreReason::NotTakeover);
                    }
                    stop = Some(("原桌布無法備份，不接管".to_owned(), index + 1));
                    break;
                }
                SetOutcome::AwaitingSpotlightConfirmation => {
                    // 這台與其餘各台記 Apply：等使用者確認（確認時排程器重設），不在每次喚醒時重畫。
                    for t in &plan.targets[index..] {
                        self.scheduler
                            .record_failed(plan, t, FailureKind::Apply, now);
                    }
                    let newly = !self.status().awaiting_spotlight_confirmation;
                    self.publish(|s| s.awaiting_spotlight_confirmation = true);
                    self.info("原桌布為 Windows 焦點：等使用者確認後才接管，其餘螢幕這次不渲染");
                    if newly {
                        // 設定視窗可能沒開：請使用者開設定確認（同一段等待只提示一次）。
                        self.ports.spotlight_confirmation_needed();
                    }
                    stop = Some(("等使用者確認焦點".to_owned(), index + 1));
                    break;
                }
            }
        }

        if let Some((why, from)) = &stop {
            let rest: Vec<&str> = plan.targets[(*from).min(plan.targets.len())..]
                .iter()
                .map(|t| t.monitor.device_path.as_str())
                .collect();
            self.info(format!(
                "Redraw 中止（{why}）：其餘 {} 台不渲染、不設定：{rest:?}",
                rest.len()
            ));
        }

        let queued = self.takeover.redraw_finished();
        self.info("Redraw 結束");
        if let Some(reason) = queued {
            self.info(format!("執行 Redraw 期間排隊的還原：{reason:?}"));
            self.restore_now(reason);
        }
    }

    /// 讀回各台目前顯示的桌布（[`Self::execute`] 選 a/b 格用；task 6.4，審查 R2a-L1）。讀不到時回空清單
    /// （各台退回狀態檔的 `last_set`）；逾時或忙碌照常記成忙碌，接下來的預檢會跳過。
    fn read_displayed(&mut self) -> Vec<(String, Option<String>)> {
        match self.service.read() {
            Ok(snap) => snap
                .monitors
                .into_iter()
                .filter(|m| m.monitor.is_online())
                .map(|m| (m.monitor.device_path, m.wallpaper))
                .collect(),
            Err(e) => {
                if matches!(
                    e,
                    WallpaperError::Timeout { .. } | WallpaperError::Busy { .. }
                ) {
                    self.note_busy();
                }
                self.warn(format!(
                    "讀回目前顯示的桌布失敗（{e}）：選 a/b 格改用狀態檔記錄的上次設定"
                ));
                Vec::new()
            }
        }
    }

    /// 一台螢幕 `Applied` 之後等 `confirm_delay`（約 1 秒，可注入）讀回該台，呼叫 `confirm_applied`
    /// （4.4 契約第 2 點；修正輪 1 起每台在自己設定後就確認，不等整批）。
    fn confirm_applied(&mut self, dev: &str, stable: &[StableDisplay]) {
        self.ports.sleep(self.config.confirm_delay);
        let snapshot = match self.service.read() {
            Ok(s) => s,
            Err(e) => {
                if matches!(
                    e,
                    WallpaperError::Timeout { .. } | WallpaperError::Busy { .. }
                ) {
                    self.note_busy();
                }
                self.warn(format!(
                    "確認已套用：螢幕 {dev} 讀回失敗（{e}）；下一次設定前的讀回仍會判定"
                ));
                return;
            }
        };
        let readback = snapshot
            .monitors
            .iter()
            .find(|m| path_eq(&m.monitor.device_path, dev))
            .and_then(|m| m.wallpaper.clone());
        let line = match readback {
            Some(rb) => {
                let io = TakeoverIo {
                    wallpaper: &self.service,
                    registry: &mut *self.registry,
                    stable_displays: stable,
                    files: &*self.files,
                    now: self.ports.now(),
                };
                let ok = self.takeover.confirm_applied(&io, dev, &rb);
                format!("確認已套用：螢幕 {dev} 讀回 {rb} → {ok}")
            }
            None => format!("確認已套用：螢幕 {dev} 讀不到目前的桌布（可能已離線）"),
        };
        self.info(line);
    }

    // ── 還原 ─────────────────────────────────────────────────────────────────────────────

    /// 立即還原原桌布（沒有進行中的 Redraw 時呼叫）。`StateUnavailable` 依 `retry_delay` 排定重試
    /// （契約 8），`Blocked` 停止重試。4.7b 的結束還原、命令列還原、還原中斷續做也走這裡。
    pub fn restore_now(&mut self, reason: RestoreReason) -> RestoreResult {
        self.restore_with(reason, false)
    }

    /// [`Self::restore_now`] 的本體。`by_recovery`＝這次是「設定路徑恢復」觸發的重試：失敗時新的
    /// 退避窗延續「恢復額度已用掉」（4.7b 限頻）。
    fn restore_with(&mut self, reason: RestoreReason, by_recovery: bool) -> RestoreResult {
        let now = self.ports.now();
        let needs_displays = self.takeover.state().is_none_or(|s| {
            s.status == TakeoverStatus::TakenOver || s.monitors.values().any(|r| r.pending_restore)
        });
        if needs_displays {
            self.ensure_displays();
        }
        let stable = self.stable_displays();
        self.info(format!("還原原桌布：原因 {reason:?}"));
        let mut io = TakeoverIo {
            wallpaper: &self.service,
            registry: &mut *self.registry,
            stable_displays: &stable,
            files: &*self.files,
            now,
        };
        // 這是同一個原因的重試（4.9 ledger）：事件（安全閥的「主題改不接管」與通知）第一次已套用過。
        let is_retry = self
            .restore_retry
            .as_ref()
            .is_some_and(|r| r.reason == reason);
        let outcome = self
            .takeover
            .restore(&mut io, &mut self.scheduler, reason.clone());
        if self.service.is_busy() {
            self.note_busy();
        }
        let failures = self.restore_retry.as_ref().map_or(0, |r| r.failures) + 1;
        let at = now + retry_delay(failures);
        // 事件先套用、再依結果排定重試（4.9 ledger）：`ThemeSetToNone` 會清掉 `restore_retry`，放在後面
        // 會把剛排定的重試清掉，安全閥的還原失敗後就不再重試。同一次觸發的事件只套用一次。
        if is_retry && !outcome.events.is_empty() {
            self.info("還原重試：這次觸發的事件（主題改「不接管」、通知）第一次已套用，不再重複");
        } else {
            self.apply_events(outcome.events);
        }
        match &outcome.result {
            RestoreResult::StateUnavailable => {
                self.warn(format!(
                    "還原結果：狀態檔暫時讀不到；第 {failures} 次失敗，{} 再試",
                    self.fmt_time(at)
                ));
                self.restore_retry = Some(RestoreRetry {
                    reason,
                    failures,
                    at,
                    recovery_used: by_recovery,
                });
            }
            RestoreResult::Failed { problems } => {
                // 修正輪 1（審查 medium 1）：COM 忙碌、逾時、讀回驗證失敗——宿主的圖還在桌面上，主題已是
                // 「不接管」、排程不會再評估到它，協調迴圈自己負責重試：退避時刻到、或設定路徑恢復時。
                self.warn(format!(
                    "還原失敗（{problems:?}）：第 {failures} 次，{} 再試；COM 恢復或 explorer 重新啟動時立即重試",
                    self.fmt_time(at)
                ));
                self.restore_retry = Some(RestoreRetry {
                    reason,
                    failures,
                    at,
                    recovery_used: by_recovery,
                });
            }
            RestoreResult::Blocked => {
                self.restore_retry = None;
                self.log_at(
                    Level::Error,
                    "還原結果：狀態檔封鎖，無法還原（不重試；可修復狀態檔後以 --restore-wallpaper 處理）",
                );
            }
            RestoreResult::Queued => {
                self.info("還原結果：已排隊到進行中的 Redraw 結束之後");
            }
            other => {
                self.restore_retry = None;
                self.info(format!("還原結果：{other:?}"));
            }
        }
        self.finish_resume_if_done(&outcome.result);
        outcome.result
    }

    /// 續做的還原完成（標記已清除）後依標記原因決定後續（4.7b 裁決 3）：系統匣結束、不接管、命令列、
    /// 安全閥一律維持不接管直到使用者重新選主題——主題不是「不接管」時改成「不接管」並存檔；續做
    /// 期間使用者已改選主題則照常接管。
    fn finish_resume_if_done(&mut self, result: &RestoreResult) {
        if self.resume.is_none()
            || self.takeover.restore_marker().is_some()
            || !matches!(
                result,
                RestoreResult::Restored { .. } | RestoreResult::NothingToRestore
            )
        {
            return;
        }
        let Some(resume) = self.resume.take() else {
            return;
        };
        self.info(format!(
            "還原中斷續做完成（原因 {:?}）：「還原進行中」標記已清除",
            resume.reason
        ));
        let theme = self.ports.inputs().theme;
        if resume.reselected {
            self.info(format!(
                "續做期間使用者已重新選了主題 {}：照常接管",
                theme.as_str()
            ));
        } else if theme != WallpaperTheme::None {
            self.info(format!(
                "續做的還原原因為 {:?}：維持不接管直到使用者重新選主題——主題 {} 改為「不接管」並存檔",
                resume.reason,
                theme.as_str()
            ));
            self.set_theme_none_here(ThemeNoneCause::RestoreResumed);
        }
    }

    /// 「主題改不接管」的存檔結果（task 6.4，審查 R2b-L1）：失敗時依 `retry_delay` 排定重試存檔——讓位、
    /// 安全閥、焦點取消、備份失敗、系統匣結束之後，重啟前沒有存進設定檔的話，下次啟動會照舊主題重新
    /// 接管、蓋掉使用者的桌布。成功（含使用者已選回主題）就清掉重試。
    fn note_theme_saved(&mut self, saved: Result<(), String>) {
        match saved {
            Ok(()) => self.theme_save_retry = None,
            Err(e) => self.schedule_theme_save_retry(&e),
        }
    }

    fn schedule_theme_save_retry(&mut self, error: &str) {
        let failures = self.theme_save_retry.map_or(0, |(n, _)| n) + 1;
        let at = self.ports.now() + retry_delay(failures);
        self.theme_save_retry = Some((failures, at));
        self.warn(format!(
            "主題「不接管」存檔失敗（第 {failures} 次：{error}）：記憶體中已是「不接管」，{} 重試存檔",
            self.fmt_time(at)
        ));
    }

    /// 到期的「主題改不接管」存檔重試。
    fn retry_theme_save(&mut self, now: UnixSeconds) {
        let Some((failures, at)) = self.theme_save_retry else {
            return;
        };
        if now < at {
            return;
        }
        match self.ports.resave_theme_none() {
            Ok(true) => {
                self.theme_save_retry = None;
                self.info(format!(
                    "主題「不接管」重新存檔成功（先前失敗 {failures} 次）"
                ));
            }
            Ok(false) => {
                self.theme_save_retry = None;
                self.info("主題「不接管」存檔重試：使用者已選回主題（那次選擇已存檔），停止重試");
            }
            Err(e) => self.schedule_theme_save_retry(&e),
        }
    }

    /// 還原重試與存檔重試的下一個時刻（取早）。
    fn retry_wake(&self) -> Option<UnixSeconds> {
        let restore = self.restore_retry.as_ref().map(|r| r.at);
        let save = self.theme_save_retry.map(|(_, at)| at);
        let pending = self.pending_retry.map(|(_, at)| at);
        [restore, save, pending].into_iter().flatten().min()
    }

    /// 協調迴圈自己決定把主題改成「不接管」（續做完成、命令列）：改設定並存檔、通知狀態機。
    fn set_theme_none_here(&mut self, cause: ThemeNoneCause) {
        if self.ports.inputs().theme != WallpaperTheme::None {
            let saved = self.ports.set_theme_none(cause);
            self.note_theme_saved(saved);
        }
        self.takeover.theme_selected(WallpaperTheme::None);
        self.last_theme = Some(WallpaperTheme::None);
    }

    // ── 需要回覆的要求（4.7b）──────────────────────────────────────────────────────────

    /// 處理一個要求（迴圈外殼在 `step` 之間呼叫：此時沒有進行中的 Redraw——進行中的已在檢查點
    /// 中止）。回傳迴圈要不要結束。
    pub fn handle_request(&mut self, request: Request) -> LoopControl {
        let control = self.handle_request_inner(request);
        // 標記已由這次還原自己的寫入接手（成功時清除、失敗時留著），外部要求清掉，之後的寫入不再補它。
        self.restore_request.clear();
        control
    }

    fn handle_request_inner(&mut self, request: Request) -> LoopControl {
        match request {
            Request::ExitAndRestore(mut reply) => {
                self.info(
                    "系統匣「結束」：停止新的渲染（進行中的 Redraw 已在檢查點中止），開始還原原桌布（含全域填滿方式、背景色、登錄）",
                );
                reply(RequestProgress::Started);
                self.restore_retry = None;
                // task 6.4（審查 R1 low）：系統匣結束一律存成「不接管」——不論還原是否在時限內完成，下次
                // 啟動都不再接管（還原沒做完時續做，完成後同樣維持不接管）。系統匣的結束執行緒在等待後
                // 會再強制存一次（這裡存檔失敗、或迴圈沒在時限內處理要求時）。
                self.set_theme_none_here(ThemeNoneCause::TrayExit);
                let result = self.restore_now(RestoreReason::TrayExit);
                let report = RestoreReport::from_result(&result);
                let ok = matches!(
                    report.code,
                    RestoreCode::Restored | RestoreCode::PartiallyRestored
                );
                self.log_at(
                    if ok { Level::Info } else { Level::Warn },
                    format!(
                        "系統匣「結束」：還原結果 {}（{}）→ 協調迴圈結束{}",
                        report.code.label(),
                        report.summary,
                        if ok {
                            ""
                        } else {
                            "；狀態檔保留「還原進行中」標記，下次啟動續做"
                        }
                    ),
                );
                reply(RequestProgress::Finished(report));
                LoopControl::Exit
            }
            Request::CommandLineRestore(mut reply) => {
                self.info(
                    "命令列 --restore-wallpaper（交給執行中的宿主）：主題改為「不接管」並存檔，還原原桌布",
                );
                reply(RequestProgress::Started);
                self.restore_retry = None;
                self.set_theme_none_here(ThemeNoneCause::CommandLine);
                let result = self.restore_now(RestoreReason::CommandLine);
                let report = RestoreReport::from_result(&result);
                self.log_at(
                    if report.code == RestoreCode::Restored {
                        Level::Info
                    } else {
                        Level::Warn
                    },
                    format!(
                        "命令列 --restore-wallpaper：還原結果 {}（{}），回報給命令列（結束碼 {}）",
                        report.code.label(),
                        report.summary,
                        report.code.exit_code()
                    ),
                );
                reply(RequestProgress::Finished(report));
                LoopControl::Continue
            }
            Request::ExitForUpdate(mut reply) => {
                // 刻意什麼都不做：不還原、不存主題（不呼叫 `set_theme_none_here`）、不清還原重試與存檔重試
                // （迴圈即將結束，它們隨行程消失；狀態檔與主題設定檔維持原樣）、不碰桌布 API。
                let status = match self.takeover.state() {
                    Some(s) if s.status == TakeoverStatus::TakenOver => "接管中",
                    Some(_) => "未接管",
                    None => "不可用（封鎖或暫時讀不到）",
                };
                self.info(format!(
                    "因更新結束：不還原原桌布，狀態檔維持{status}、主題不變（新版啟動後照常接管）；停止新的渲染與桌布 API 呼叫，協調迴圈結束"
                ));
                reply(RequestProgress::ExitedForUpdate);
                LoopControl::Exit
            }
        }
    }
}

/// 輸出資料夾內不該留下的輸出檔（[`Coordinator`] 的清理）：檔名是 `<螢幕鍵>-a.png`／`-b.png`（含
/// `.tmp`），鍵不在 `keep_keys`、路徑不在 `keep_paths`。只看資料夾本身的檔案（不進子資料夾，
/// 備份資料夾不碰）。
pub fn stale_output_files(
    dir: &Path,
    keep_keys: &HashSet<String>,
    keep_paths: &[String],
) -> Vec<PathBuf> {
    const SUFFIXES: [&str; 4] = ["-a.png", "-b.png", "-a.png.tmp", "-b.png.tmp"];
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_owned();
            let key = SUFFIXES.iter().find_map(|s| name.strip_suffix(s))?;
            if !is_valid_monitor_key(key) || keep_keys.contains(key) {
                return None;
            }
            let path = e.path();
            let shown = path.to_string_lossy().into_owned();
            (!keep_paths.iter().any(|k| path_eq(k, &shown))).then_some(path)
        })
        .collect();
    out.sort();
    out
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------------------------------------
// 執行緒外殼與把手
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Default)]
struct MailboxInner {
    pending: Vec<Wake>,
    shutdown: bool,
    /// 待處理的要求（4.7b）。
    requests: Vec<Request>,
    /// 協調迴圈已結束（或從未啟動）：之後的要求直接丟棄＝回覆端立即知道沒有人會處理（命令列端收到
    /// 「交接失敗」，等宿主結束後在本行程還原——系統匣結束已還原完的情況也一樣，主題因此存成「不接管」，
    /// 結果與另外兩條路相同；複審 N2）。
    closed: bool,
}

/// 協調迴圈的信箱：一把只保護佇列的小鎖＋條件變數。
#[derive(Debug, Default)]
pub struct Mailbox {
    inner: Mutex<MailboxInner>,
    cv: Condvar,
}

impl Mailbox {
    fn push(&self, wake: Wake) {
        lock(&self.inner).pending.push(wake);
        self.cv.notify_one();
    }

    #[allow(dead_code)] // 測試與日後的關閉路徑用，見 `CoordinatorHandle::shutdown`。
    fn request_shutdown(&self) {
        lock(&self.inner).shutdown = true;
        self.cv.notify_one();
    }

    /// 投遞一個要求；迴圈已結束時直接丟棄（回覆接口隨之釋放）。
    fn push_request(&self, request: Request) {
        let mut inner = lock(&self.inner);
        if inner.closed {
            drop(inner);
            drop(request);
            return;
        }
        inner.requests.push(request);
        drop(inner);
        self.cv.notify_one();
    }

    fn take_requests(&self) -> Vec<Request> {
        std::mem::take(&mut lock(&self.inner).requests)
    }

    /// 協調迴圈結束：丟棄未處理的要求，之後的要求也直接丟棄（見 [`MailboxInner::closed`]）。
    fn close(&self) {
        let dropped = {
            let mut inner = lock(&self.inner);
            inner.closed = true;
            std::mem::take(&mut inner.requests)
        };
        drop(dropped);
    }

    /// 等到有喚醒、要求、要求結束或逾時；逾時回傳 `[Timer]`。
    fn wait(&self, timeout: Duration) -> Vec<Wake> {
        let guard = lock(&self.inner);
        let (mut guard, _) = self
            .cv
            .wait_timeout_while(guard, timeout, |m| {
                m.pending.is_empty() && !m.shutdown && m.requests.is_empty()
            })
            .unwrap_or_else(|e| e.into_inner());
        let wakes = std::mem::take(&mut guard.pending);
        if wakes.is_empty() {
            vec![Wake::Timer]
        } else {
            wakes
        }
    }
}

impl Inbox for Mailbox {
    fn drain(&self) -> Vec<Wake> {
        std::mem::take(&mut lock(&self.inner).pending)
    }

    fn shutdown_requested(&self) -> bool {
        lock(&self.inner).shutdown
    }

    fn restore_requested(&self) -> bool {
        !lock(&self.inner).requests.is_empty()
    }

    fn session_end_pending(&self) -> bool {
        session_end_pending_in(lock(&self.inner).pending.iter().copied())
    }
}

/// 協調迴圈的把手（Tauri managed state）。`notify`／`status`／`shutdown` 都只取一把小鎖、立即返回，
/// 可在主執行緒、指令處理緒、視窗程序內呼叫。
#[derive(Clone)]
pub struct CoordinatorHandle {
    mailbox: Arc<Mailbox>,
    /// 外部要求的「還原進行中」標記（4.7b 修正輪 1）。
    restore_request: Arc<RestoreRequestMarker>,
    status: Arc<Mutex<CoordinatorStatus>>,
    /// 設定視窗的事先確認（4.8 修正輪 1）。
    preconfirm: SpotlightPreconfirm,
}

impl CoordinatorHandle {
    /// 設定視窗的事先確認登記（4.8 修正輪 1；見 [`SpotlightPreconfirm`]）。
    pub fn spotlight_preconfirm(&self) -> SpotlightPreconfirm {
        self.preconfirm.clone()
    }

    /// 投遞一個喚醒（不等待）。
    pub fn notify(&self, wake: Wake) {
        self.mailbox.push(wake);
    }

    /// 目前狀態（4.8 設定視窗的接口：`wallpaper_settings` 的指令讀它，不等協調迴圈）。
    pub fn status(&self) -> CoordinatorStatus {
        lock(&self.status).clone()
    }

    /// 要求迴圈結束、不還原（進行中的 Redraw 在下一個檢查點中止）。不等待。系統匣結束改用
    /// [`Request::ExitAndRestore`]（4.7b）；這個只剩測試使用。
    #[allow(dead_code)]
    pub fn shutdown(&self) {
        self.mailbox.request_shutdown();
    }

    /// 投遞「因更新結束」要求（installer-auto-update task 3.3；不等待）。**不**寫「還原進行中」標記、不碰狀態檔：
    /// 寫了標記，新版啟動時會走續做還原而不接管。迴圈已結束時要求直接丟棄（回覆端因此立即斷線）。
    pub fn request_exit_for_update(&self, reply: ReplyFn) {
        let request = Request::ExitForUpdate(reply);
        log::info!(target: LOG_TARGET, "{request:?}：投遞（不寫還原標記）");
        self.mailbox.push_request(request);
    }

    /// 測試用：[`Self::request_restore_within`] 以 5 秒為取鎖上限（正式呼叫端都自己指定上限）。
    #[cfg(test)]
    pub fn request_restore(&self, request: Request) -> MarkOutcome {
        self.request_restore_within(request, Duration::from_secs(5))
    }

    /// 投遞還原要求（系統匣結束、命令列交接；4.7b，不等待；迴圈已結束時：系統匣結束已還原完成就以
    /// 那次的結果回報，否則丟棄要求、回覆接口隨之釋放）。修正輪 1：**先**把「還原進行中」標記寫進
    /// 狀態檔（只寫狀態檔、不清孤兒備份；未接管時不寫），再投遞——進行中的渲染拖過結束的等待上限、
    /// 宿主沒還原就結束時，下次啟動會續做還原而不是照常接管。修正輪 2（審查 F6）：取共用鎖最多等
    /// `lock_wait`，等不到就不寫標記、照常投遞。做一次小檔讀寫，不要在同步 WndProc 裡呼叫。
    pub fn request_restore_within(&self, request: Request, lock_wait: Duration) -> MarkOutcome {
        let reason = match &request {
            Request::ExitAndRestore(_) => RestoreReason::TrayExit,
            Request::CommandLineRestore(_) => RestoreReason::CommandLine,
            // 「因更新結束」不還原，沒有「還原進行中」標記可寫；專用入口是 `request_exit_for_update`。
            Request::ExitForUpdate(_) => {
                unreachable!("ExitForUpdate 不是還原要求，請用 request_exit_for_update")
            }
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
        let outcome = self.restore_request.request(&reason, now, lock_wait);
        match &outcome {
            MarkOutcome::Written => log::info!(
                target: LOG_TARGET,
                "{request:?}：在等待之前寫入「還原進行中」標記（{reason:?}）"
            ),
            MarkOutcome::NotNeeded => log::info!(
                target: LOG_TARGET,
                "{request:?}：狀態檔不是接管中（或已有標記），不需要先寫標記"
            ),
            MarkOutcome::NotAttached => log::warn!(
                target: LOG_TARGET,
                "{request:?}：協調迴圈還沒接上狀態檔，無法先寫標記"
            ),
            MarkOutcome::LockTimeout => log::warn!(
                target: LOG_TARGET,
                "{request:?}：{} 秒內取不到狀態檔的寫入鎖（協調迴圈卡在寫檔），沒有先寫標記（照常要求還原）",
                lock_wait.as_secs_f32()
            ),
            MarkOutcome::Failed(e) => log::error!(
                target: LOG_TARGET,
                "{request:?}：先寫「還原進行中」標記失敗：{e}（照常要求還原）"
            ),
        }
        self.mailbox.push_request(request);
        outcome
    }
}

/// 系統匣結束的等待上限（4.7b 裁決 1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitLimits {
    /// 等進行中的 Redraw 結束或中止、協調迴圈開始還原的上限（渲染一台最長約 30 秒，但每台之間都會
    /// 檢查中止條件；超過就不還原、照常結束）。
    pub redraw_wait: Duration,
    /// 還原本身的上限：逾時照常結束，狀態檔保留「還原進行中」標記，下次啟動續做。
    pub restore_limit: Duration,
}

impl Default for ExitLimits {
    fn default() -> Self {
        Self {
            redraw_wait: Duration::from_secs(5),
            restore_limit: Duration::from_secs(10),
        }
    }
}

/// 系統匣結束等待的結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitWait {
    /// 還原完成（成功或失敗）。
    Finished(RestoreReport),
    /// 協調迴圈在 `redraw_wait` 內沒有開始還原（卡在渲染或 explorer 裡）。
    NotStarted,
    /// 開始還原後超過 `restore_limit`。
    TimedOut,
    /// 協調迴圈不在（已結束或從未啟動）。
    Disconnected,
}

/// 等系統匣結束的還原（純等待邏輯，可單元測試）。
pub fn wait_exit_restore(rx: &Receiver<RequestProgress>, limits: ExitLimits) -> ExitWait {
    let finished = |p| match p {
        RequestProgress::Finished(r) => Some(r),
        RequestProgress::Started | RequestProgress::ExitedForUpdate => None,
    };
    match rx.recv_timeout(limits.redraw_wait) {
        Ok(p) => {
            if let Some(r) = finished(p) {
                return ExitWait::Finished(r);
            }
        }
        Err(RecvTimeoutError::Timeout) => return ExitWait::NotStarted,
        Err(RecvTimeoutError::Disconnected) => return ExitWait::Disconnected,
    }
    let deadline = Instant::now() + limits.restore_limit;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(p) => {
                if let Some(r) = finished(p) {
                    return ExitWait::Finished(r);
                }
            }
            Err(RecvTimeoutError::Timeout) => return ExitWait::TimedOut,
            Err(RecvTimeoutError::Disconnected) => return ExitWait::Disconnected,
        }
    }
}

/// 系統匣「結束」（4.7b）：請協調迴圈停止渲染並還原，等它回報（上限見 [`ExitLimits`]），每種結果
/// 記一行。由系統匣的結束執行緒呼叫（**不可**在主執行緒：還原要列舉顯示器，需要主執行緒往返）。
pub fn exit_and_restore(handle: &CoordinatorHandle, limits: ExitLimits) -> ExitWait {
    let started = Instant::now();
    let (tx, rx) = std::sync::mpsc::channel();
    // 上限從這裡就開始算（審查 F6）：先寫標記取共用鎖的時間也算在 `redraw_wait` 裡。
    handle.request_restore_within(
        Request::ExitAndRestore(Box::new(move |p| {
            let _ = tx.send(p);
        })),
        limits.redraw_wait,
    );
    let wait = wait_exit_restore(
        &rx,
        ExitLimits {
            redraw_wait: limits.redraw_wait.saturating_sub(started.elapsed()),
            restore_limit: limits.restore_limit,
        },
    );
    match &wait {
        ExitWait::Finished(r)
            if matches!(
                r.code,
                RestoreCode::Restored | RestoreCode::PartiallyRestored
            ) =>
        {
            log::info!(target: LOG_TARGET, "系統匣「結束」：還原完成（{}），宿主結束", r.summary);
        }
        ExitWait::Finished(r) => log::warn!(
            target: LOG_TARGET,
            "系統匣「結束」：還原{}（{}），照常結束；狀態檔保留「還原進行中」標記，下次啟動續做",
            r.code.label(),
            r.summary
        ),
        ExitWait::NotStarted => log::warn!(
            target: LOG_TARGET,
            "系統匣「結束」：{} 秒內協調迴圈沒有開始還原（Redraw 或 explorer 卡住），照常結束；狀態檔已先寫「還原進行中」標記，下次啟動續做還原、不接管",
            limits.redraw_wait.as_secs()
        ),
        ExitWait::TimedOut => log::warn!(
            target: LOG_TARGET,
            "系統匣「結束」：還原超過 {} 秒上限，照常結束；狀態檔保留「還原進行中」標記，下次啟動續做",
            limits.restore_limit.as_secs()
        ),
        ExitWait::Disconnected => log::info!(
            target: LOG_TARGET,
            "系統匣「結束」：協調迴圈不在（已結束或未啟動），不還原、照常結束"
        ),
    }
    wait
}

/// 「因更新結束」等待的結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitForUpdateWait {
    /// 協調迴圈已處理完並結束。
    Exited,
    /// 時限內沒有處理（迴圈卡在渲染或 explorer 呼叫裡）；要求仍留在信箱，宿主隨後結束。
    TimedOut,
    /// 協調迴圈不在（已結束或從未啟動）。
    Disconnected,
}

/// 等「因更新結束」的回報（純等待邏輯，可單元測試）。
pub fn wait_exit_for_update(rx: &Receiver<RequestProgress>, limit: Duration) -> ExitForUpdateWait {
    let deadline = Instant::now() + limit;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(RequestProgress::ExitedForUpdate) => return ExitForUpdateWait::Exited,
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => return ExitForUpdateWait::TimedOut,
            Err(RecvTimeoutError::Disconnected) => return ExitForUpdateWait::Disconnected,
        }
    }
}

/// 「因更新結束」（installer-auto-update task 3.3、design.md D4 步驟 3）：請協調迴圈停止渲染並結束，
/// **不還原、不動狀態檔與主題**，等它回報（上限 `limit`，D4 表為 5 秒），每種結果記一行。不得在主執行緒
/// 呼叫（等待期間主執行緒不能卡住：協調迴圈的 Redraw 可能需要主執行緒往返）。
pub fn exit_for_update(handle: &CoordinatorHandle, limit: Duration) -> ExitForUpdateWait {
    let (tx, rx) = std::sync::mpsc::channel();
    handle.request_exit_for_update(Box::new(move |p| {
        let _ = tx.send(p);
    }));
    let wait = wait_exit_for_update(&rx, limit);
    match wait {
        ExitForUpdateWait::Exited => {
            log::info!(target: LOG_TARGET, "因更新結束：協調迴圈已結束（未還原、未動狀態檔與主題）");
        }
        ExitForUpdateWait::TimedOut => log::warn!(
            target: LOG_TARGET,
            "因更新結束：{:.1} 秒內協調迴圈沒有處理要求（Redraw 或 explorer 卡住），放棄等待、照常結束；狀態檔與主題未動",
            limit.as_secs_f32()
        ),
        ExitForUpdateWait::Disconnected => log::info!(
            target: LOG_TARGET,
            "因更新結束：協調迴圈不在（已結束或未啟動），略過"
        ),
    }
    wait
}

/// 在專屬執行緒上建立並執行協調迴圈。`make` 在該執行緒上執行（讀狀態檔不在主執行緒）；回 `Err`
/// 時記錯誤、執行緒結束（宿主其他部分照常）。
pub fn spawn_coordinator<P, F>(make: F) -> io::Result<(CoordinatorHandle, JoinHandle<()>)>
where
    P: CoordinatorPorts + 'static,
    F: FnOnce() -> Result<Coordinator<P>, String> + Send + 'static,
{
    let mailbox = Arc::new(Mailbox::default());
    let status = Arc::new(Mutex::new(CoordinatorStatus::default()));
    let restore_request = Arc::new(RestoreRequestMarker::default());
    let preconfirm = SpotlightPreconfirm::default();
    let handle = CoordinatorHandle {
        mailbox: Arc::clone(&mailbox),
        status: Arc::clone(&status),
        restore_request: Arc::clone(&restore_request),
        preconfirm: preconfirm.clone(),
    };
    let join = thread::Builder::new()
        .name(THREAD_NAME.to_owned())
        .spawn(move || {
            // task 6.4（審查 R2a-L2）：任何結束方式（含 panic 展開）都關閉信箱——之後的要求立即被丟棄，
            // 系統匣結束立即得到「斷線」、命令列交接立即知道交接失敗，而不是等滿上限。
            let _close = CloseOnDrop(Arc::clone(&mailbox));
            let mut coordinator = match make() {
                Ok(c) => c,
                Err(e) => {
                    log::error!(target: LOG_TARGET, "桌布協調迴圈無法啟動：{e}");
                    return;
                }
            };
            coordinator.attach_status(status);
            coordinator.attach_restore_request(restore_request);
            coordinator.attach_preconfirm(preconfirm);
            run_loop(&mut coordinator, &mailbox);
        })?;
    Ok((handle, join))
}

/// 協調執行緒結束時關閉信箱（[`Mailbox::close`]；panic 展開時也會執行）。
struct CloseOnDrop(Arc<Mailbox>);

impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        if thread::panicking() {
            log::error!(
                target: LOG_TARGET,
                "桌布協調迴圈 panic：關閉信箱（之後的還原要求立即回報交接失敗；桌布停止更新，狀態檔的標記讓下次啟動續做）"
            );
        }
        self.0.close();
    }
}

/// 協調迴圈外殼：要求（4.7b）在 `step` 之間依序處理——進行中的 Redraw 會先在檢查點中止。
fn run_loop<P: CoordinatorPorts>(coordinator: &mut Coordinator<P>, mailbox: &Mailbox) {
    let mut wakes = vec![Wake::Startup];
    loop {
        if mailbox.shutdown_requested() {
            coordinator.info("協調迴圈結束（宿主結束）");
            return;
        }
        for request in mailbox.take_requests() {
            if coordinator.handle_request(request) == LoopControl::Exit {
                coordinator.info("協調迴圈結束（結束要求：系統匣結束或因更新結束）");
                return;
            }
        }
        let out = coordinator.step(std::mem::take(&mut wakes), mailbox);
        if mailbox.shutdown_requested() {
            coordinator.info("協調迴圈結束（宿主結束）");
            return;
        }
        if mailbox.restore_requested() {
            continue;
        }
        let timeout = coordinator.wait_timeout(&out);
        wakes = mailbox.wait(timeout);
    }
}

/// 投遞喚醒給宿主的協調迴圈（沒有協調迴圈時——Secondary、啟動前、self-test——什麼都不做）。
pub fn notify_app(app: &AppHandle, wake: Wake) {
    if let Some(handle) = app.try_state::<CoordinatorHandle>() {
        handle.notify(wake);
    }
}

// ---------------------------------------------------------------------------------------------
// 啟動接線（main.rs）
// ---------------------------------------------------------------------------------------------

/// 依啟動仲裁的角色決定桌布相關的啟動動作（契約 9）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartupPlan {
    /// 主題設定檔缺檔時寫出預設檔（`load_for_startup`）；否則只讀（`reload`）。
    pub write_config: bool,
    /// 啟動協調迴圈（讀寫狀態檔、設定桌布）。
    pub run_coordinator: bool,
}

/// Secondary（仲裁落敗）只讀設定、不寫任何檔、不跑協調迴圈；Primary 與 `Unarbitrated`（仲裁故障，
/// 可能就是唯一執行個體，比照 `settings::load_for_arbitrated_startup`）照常。
pub fn startup_plan(role: StartupRole) -> StartupPlan {
    let primary = role.may_write_startup_state();
    StartupPlan {
        write_config: primary,
        run_coordinator: primary,
    }
}

/// 依 [`StartupPlan`] 載入主題設定檔（結果交給 `AppState::with_wallpaper_config`）。
pub fn load_theme_config(plan: StartupPlan, path: &Path) -> WallpaperConfig {
    if plan.write_config {
        wallpaper_config::load_for_startup(path)
    } else {
        wallpaper_config::reload(path)
    }
}

/// 正式的宿主依賴（Tauri managed state、Windows 時區、渲染管線、勿打擾監看）。
pub struct TauriPorts {
    app: AppHandle,
    tz: crate::desktop::timezone::WindowsLocalTime,
    origin: Instant,
    /// 勿打擾監看（4.7c）：第一次需要重畫時才建立輪詢執行緒；隨協調迴圈結束丟棄。
    dnd: crate::wallpaper_dnd::DndMonitor,
}

impl TauriPorts {
    pub fn new(app: AppHandle) -> Self {
        let dnd = crate::wallpaper_dnd::monitor_for_app(&app);
        Self {
            app,
            tz: crate::desktop::timezone::WindowsLocalTime::default(),
            origin: Instant::now(),
            dnd,
        }
    }
}

impl CoordinatorPorts for TauriPorts {
    fn now(&self) -> UnixSeconds {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
    }

    fn monotonic(&self) -> Duration {
        self.origin.elapsed()
    }

    fn tz(&self) -> &dyn UtcOffsetSource {
        &self.tz
    }

    fn inputs(&self) -> HostInputs {
        inputs_from_state(&self.app.state::<crate::widgets::AppState>())
    }

    fn dpi_for_rect(&self, rect: PhysicalRect) -> Option<u32> {
        crate::desktop::dpi_for_rect(rect)
    }

    fn displays(&self) -> Vec<DisplayInfo> {
        let monitors = match self.app.available_monitors() {
            Ok(m) => m,
            Err(e) => {
                log::warn!(target: LOG_TARGET, "列舉顯示器失敗：{e}");
                return Vec::new();
            }
        };
        let paths = crate::desktop::source_name_to_device_path();
        monitors
            .iter()
            .map(|m| {
                let pos = m.position();
                let size = m.size();
                DisplayInfo {
                    device_path: m.name().and_then(|n| paths.get(n).cloned()),
                    rect: PhysicalRect {
                        x: pos.x,
                        y: pos.y,
                        width: i32::try_from(size.width).unwrap_or(i32::MAX),
                        height: i32::try_from(size.height).unwrap_or(i32::MAX),
                    },
                    dpi: dpi_from_scale(m.scale_factor()),
                }
            })
            .collect()
    }

    fn render(
        &self,
        req: &RenderRequest,
        out: &OutputTarget<'_>,
    ) -> Result<RenderSuccess, RenderFailure> {
        match self
            .app
            .try_state::<crate::wallpaper_render::WallpaperRenderer>()
        {
            Some(renderer) => renderer.render(req, out),
            None => Err(RenderFailure {
                render_id: None,
                reason: RenderFailureReason::OpenFailed(
                    "桌布渲染管線未就緒（渲染視窗的使用者資料夾無法決定）".to_owned(),
                ),
                window_closed: None,
            }),
        }
    }

    fn set_theme_none(&self, cause: ThemeNoneCause) -> Result<(), String> {
        crate::widgets::set_wallpaper_theme_none(&self.app).inspect_err(|e| {
            log::error!(
                target: LOG_TARGET,
                "主題改為「不接管」（{cause:?}）後存檔失敗：{e}（記憶體中已改，協調迴圈稍後重試存檔）"
            );
        })
    }

    fn resave_theme_none(&self) -> Result<bool, String> {
        crate::widgets::resave_wallpaper_theme_none(&self.app)
    }

    fn notify_user(&self, notice: &TakeoverNotice) {
        // 4.8：系統匣通知（工作執行緒送出，不阻塞協調迴圈；文字見 wallpaper_settings::notice_text）。
        log::info!(target: LOG_TARGET, "系統匣通知：{notice:?}");
        crate::tray::show_notification(&self.app, crate::wallpaper_settings::notice_text(notice));
    }

    fn spotlight_confirmation_needed(&self) {
        log::info!(target: LOG_TARGET, "系統匣通知：原桌布為 Windows 焦點，請使用者開設定確認");
        crate::tray::show_notification(
            &self.app,
            crate::wallpaper_settings::spotlight_pending_text(),
        );
    }

    fn sleep(&self, d: Duration) {
        thread::sleep(d);
    }

    fn do_not_disturb(&self) -> bool {
        self.dnd.query()
    }

    fn explorer_gdi(&self) -> Result<ExplorerSample, String> {
        #[cfg(feature = "self-test-ipc")]
        if let Some(injected) =
            explorer_gdi_injection::read(crate::desktop::explorer_gdi::read_explorer_gdi)
        {
            return injected;
        }
        crate::desktop::explorer_gdi::read_explorer_gdi().map(|r| ExplorerSample {
            pid: r.pid,
            gdi: r.gdi,
        })
    }
}

/// 從 `AppState` 讀宿主輸入（`TauriPorts::inputs` 的本體）：依序**短暫**鎖設定、暫停原因、通道
/// 註冊表，各自複製後立即放開，不巢狀、不取 `window_sync`；回傳擁有權的值，呼叫端手上沒有任何守衛。
pub fn inputs_from_state(state: &crate::widgets::AppState) -> HostInputs {
    let theme = lock(&state.settings).wallpaper_theme;
    let pause = lock(&state.pause_reasons).clone();
    let (data, config_version) = {
        let registry = lock(&state.registry);
        (
            registry
                .snapshot(crate::data::TW_EVENTS_CHANNEL)
                .map(|s| data_dates(&s.data))
                .unwrap_or_default(),
            registry.wallpaper_config_version(),
        )
    };
    HostInputs {
        theme,
        pause,
        data,
        config_version,
    }
}

/// Tauri 的縮放係數 → DPI（1.0＝96、1.5＝144、1.75＝168）。
pub fn dpi_from_scale(scale: f64) -> u32 {
    let dpi = (scale * 96.0).round();
    if dpi.is_finite() && dpi >= 1.0 && dpi <= f64::from(u32::MAX) {
        dpi as u32
    } else {
        DEFAULT_DPI
    }
}

/// 正式宿主：在 `setup` 內（Primary）呼叫，啟動協調迴圈並登記把手（managed state）。真正的 COM
/// 服務、狀態檔讀取都在協調執行緒上進行。
pub fn spawn_for_app(app: &AppHandle) {
    // installer-auto-update 4.x：正在因更新結束就不啟動；許可持有到把手登記完（managed state）才放掉，
    // 收尾步驟 3 才不會在「啟動中」誤判成沒有協調迴圈（`updater::exit::StartGate`）。
    let Some(_start_permit) = crate::updater::try_begin_start("桌布協調迴圈") else {
        return;
    };
    let app_for_thread = app.clone();
    let spawned = spawn_coordinator(move || {
        let service = WallpaperService::spawn_com(
            crate::desktop::wallpaper::WallpaperServiceConfig::default(),
        )
        .map_err(|e| format!("無法建立桌布 COM 執行緒：{e}"))?;
        Ok(Coordinator::new(CoordinatorParts {
            ports: TauriPorts::new(app_for_thread),
            service,
            registry: Box::new(crate::desktop::wallpaper::registry::HkcuRegistry),
            files: Box::new(crate::desktop::wallpaper::file_identity::Win32FileIdentity),
            paths: crate::wallpaper_state::default_paths(),
            config: CoordinatorConfig::default(),
            log: Arc::new(|level, msg: &str| log::log!(target: LOG_TARGET, level, "{msg}")),
        }))
    });
    match spawned {
        Ok((handle, _join)) => {
            // 不保留 JoinHandle：協調執行緒可能卡在渲染或 explorer 裡，任何人都不該 join 它。
            app.manage(handle);
            let args: Vec<String> = std::env::args().collect();
            log::info!(
                target: LOG_TARGET,
                "桌布協調迴圈已啟動（啟動方式：{}）",
                launch_kind(&args)
            );
        }
        Err(e) => log::error!(target: LOG_TARGET, "無法建立桌布協調執行緒：{e}"),
    }
}

/// 啟動方式（記錄用）：`--restarted`＝`RegisterApplicationRestart` 重新啟動（Restart Manager）、
/// `--autostart`＝開機自啟，其餘＝一般啟動（含當機後使用者重開）。
pub fn launch_kind(args: &[String]) -> String {
    let has = |flag: &str| args.iter().skip(1).any(|a| a == flag);
    if has("--restarted") {
        "--restarted（RegisterApplicationRestart／Restart Manager 重新啟動）".to_owned()
    } else if has("--autostart") {
        "--autostart（開機自啟）".to_owned()
    } else {
        "一般啟動".to_owned()
    }
}

// ---------------------------------------------------------------------------------------------
// `--restore-wallpaper`：沒有執行中的宿主時在本行程直接還原（4.7b 裁決 5）
// ---------------------------------------------------------------------------------------------

/// [`restore_for_command_line`] 的依賴（正式：真正的 COM 服務、HKCU、`%APPDATA%`；測試：假的）。
pub struct CommandLineParts {
    pub service: WallpaperService,
    pub registry: Box<dyn RegistryStore + Send>,
    pub files: Box<dyn FileIdentity + Send>,
    pub paths: StatePaths,
    /// 宿主的 `settings.json`（主題改「不接管」）。
    pub settings_path: PathBuf,
    pub takeover: TakeoverConfig,
    pub now: UnixSeconds,
    pub log: LogSink,
}

/// 沒有執行中的宿主時，`--restore-wallpaper` 在本行程直接還原：不建立任何視窗、不啟動排程。
/// 先把主題設定改為「不接管」並存檔（解除安裝情境下次啟動不再接管；中途中斷時，下次啟動的續做
/// 也會維持不接管），再讀狀態檔、以 [`RestoreReason::CommandLine`] 還原（狀態機會先寫「還原進行中」
/// 標記）。每一步都經 `log` 記一行。
pub fn restore_for_command_line(parts: CommandLineParts) -> RestoreReport {
    let CommandLineParts {
        service,
        mut registry,
        files,
        paths,
        settings_path,
        takeover,
        now,
        log,
    } = parts;
    let say = |level: Level, msg: String| log(level, &msg);
    let user = std::env::var("USERNAME").unwrap_or_else(|_| "（未知）".to_owned());
    say(
        Level::Info,
        format!(
            "--restore-wallpaper：沒有執行中的宿主，在本行程直接還原（不建立小工具視窗、不啟動排程）；使用者 {user}、設定檔 {}、狀態檔 {}",
            settings_path.display(),
            paths.state_file.display()
        ),
    );
    let state_file = paths.state_file.clone();
    let state_existed = state_file.exists();
    let settings_problem = match theme_none_in_settings_file(&settings_path) {
        Ok(true) => {
            say(
                Level::Info,
                "--restore-wallpaper：主題設定已改為「不接管」並存檔".to_owned(),
            );
            None
        }
        Ok(false) => {
            say(
                Level::Info,
                "--restore-wallpaper：主題設定已是「不接管」（或設定檔不存在／讀不懂），不寫入"
                    .to_owned(),
            );
            None
        }
        Err(e) => {
            say(
                Level::Error,
                format!("--restore-wallpaper：主題設定改為「不接管」時存檔失敗：{e}（照常還原）"),
            );
            Some(e)
        }
    };
    let mut state = WallpaperTakeover::load(paths, takeover, now);
    if let Some(reason) = state.blocked() {
        let report = RestoreReport::new(
            RestoreCode::Blocked,
            format!("狀態檔讀不懂（{reason:?}），無法還原"),
        );
        say(
            Level::Error,
            format!(
                "--restore-wallpaper：{}（結束碼 {}）",
                report.summary,
                report.code.exit_code()
            ),
        );
        return report;
    }
    let mut scheduler = SchedulerState::default();
    let mut io = TakeoverIo {
        wallpaper: &service,
        registry: &mut *registry,
        stable_displays: &[],
        files: &*files,
        now,
    };
    say(
        Level::Info,
        "--restore-wallpaper：還原原桌布：原因 CommandLine".to_owned(),
    );
    let outcome = state.restore(&mut io, &mut scheduler, RestoreReason::CommandLine);
    let mut report = RestoreReport::from_result(&outcome.result);
    if !state_existed && outcome.result == RestoreResult::NothingToRestore {
        // 審查 F5：找不到狀態檔仍是 0，但記下實際讀的位置與使用者——以其他帳號或提權執行時，
        // %APPDATA% 會指向別的設定檔。
        say(
            Level::Warn,
            format!(
                "--restore-wallpaper：找不到狀態檔 {}（使用者 {user}）：沒有需要還原的桌布；若宿主曾在其他帳號接管，要以那個使用者本人、未提權執行",
                state_file.display()
            ),
        );
    }
    if let (Some(problem), RestoreCode::Restored | RestoreCode::PartiallyRestored) =
        (settings_problem, report.code)
    {
        report = RestoreReport::new(
            RestoreCode::Failed,
            format!(
                "{}；但主題設定無法改為「不接管」（{problem}），下次啟動會再接管",
                report.summary
            ),
        );
    }
    say(
        if report.code == RestoreCode::Restored {
            Level::Info
        } else {
            Level::Warn
        },
        format!(
            "--restore-wallpaper：結果 {}（{}），結束碼 {}（本行程）",
            report.code.label(),
            report.summary,
            report.code.exit_code()
        ),
    );
    report
}

/// 把 `settings.json` 的主題改成「不接管」並存檔。檔案不存在、讀不懂（預設值就是不接管）或已是
/// 不接管時不寫（回 `Ok(false)`）。
fn theme_none_in_settings_file(path: &Path) -> Result<bool, String> {
    if !path.exists() {
        return Ok(false);
    }
    let mut current = settings::load_for_arbitrated_startup(path, false).settings;
    if current.wallpaper_theme == WallpaperTheme::None {
        return Ok(false);
    }
    current.wallpaper_theme = WallpaperTheme::None;
    settings::save(path, &current)
        .map(|()| true)
        .map_err(|e| e.to_string())
}

// task 4.9：self-test 建置注入 explorer GDI 讀值（正式建置不含）。
#[cfg(feature = "self-test-ipc")]
mod explorer_gdi_injection;

#[cfg(test)]
mod tests;
