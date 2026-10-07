//! WebView2 故障復原（task 5.6；design.md D12「WebView2 故障」條，依探針 1.4 定案）。
//!
//! ## 分工
//!
//! - `crate::desktop::install_process_failed_handler`：在每個小工具 webview 的
//!   `ICoreWebView2` 上訂閱 `ProcessFailed`（每個視窗只訂閱一次，由視窗工廠
//!   `crate::widgets::create_widget_window` 呼叫），把 WebView2 的故障種類翻成
//!   [`WebviewFailureKind`]、寫一行記錄，然後呼叫本模組的 [`on_webview_failure`]。
//! - 本模組：決定怎麼復原（純函式 [`failure_response`]）、browser 行程結束的去重
//!   （[`BrowserExitDedup`]）、卡死偵測的看門狗（[`Watchdog`]）、自動重建次數上限
//!   （[`RebuildBudget`]），以及把這些接到實際動作的協調程式碼。
//! - `crate::widgets::rebuild_widget_windows`：實際「先建新視窗、再拆舊視窗」——一律走
//!   3.1 的視窗工廠（`create_widget_window`），因此置底、`WS_EX_TOOLWINDOW`、
//!   `WS_EX_NOACTIVATE`、tao VISIBLE 旗標不變量與 `ProcessFailed` 訂閱都和一般建立的視窗
//!   完全相同。
//!
//! ## 處理方式（design.md D12，探針 1.4 實測）
//!
//! | 故障 | 處理 |
//! |---|---|
//! | `RENDER_PROCESS_EXITED` | 對事件來源的 webview `Reload()`；失敗改重建該視窗 |
//! | `RENDER_PROCESS_UNRESPONSIVE` | 重建該視窗（官方範例對無回應建議重建而非 `Reload()`） |
//! | `BROWSER_PROCESS_EXITED` | 每個 webview 各收到一次，去重後只處理一次：先建新一代全部小工具視窗、再拆舊的 |
//! | `GPU_PROCESS_EXITED` 等 | 只記錄（browser 會自行重啟 GPU process） |
//! | 頁面卡死（無事件） | 看門狗連續多次逾時 → 重建該視窗 |
//!
//! 頁面重新載入或重建後會自己重新 `subscribe_data`＋`get_snapshot`（`host/ui/widget.html`
//! 的啟動流程），核心不需要替它補訂閱；重建時只把舊 label 的訂閱移除。
//!
//! ## 看門狗：為什麼是「核心探測」而不是頁面自己的計時器心跳
//!
//! 探針 1.4 用的是頁面每秒 `fetch` 一次的計時器心跳。正式宿主改成**核心每
//! [`WATCHDOG_INTERVAL`] 對每個小工具送一段探測腳本**（`WebviewWindow::eval_with_callback`，
//! 底層是 WebView2 `ExecuteScript`，wry `webview2/mod.rs::execute_script`），腳本在頁面的 JS
//! 主執行緒上執行完、完成回呼帶回 [`PROBE_TOKEN`] 才算「活著」。理由：
//!
//! - 被完全遮住或隱藏的 WebView2 頁面，瀏覽器會節流頁面計時器（design.md Risks 已記；
//!   Chromium 對隱藏頁面的計時器最慢可到每分鐘一次），頁面自己的 `setInterval` 心跳在小工具
//!   被一般視窗蓋住時就會「看起來卡死」。`ExecuteScript` 是瀏覽器→renderer 的直接請求，
//!   不是頁面計時器，不受這種節流影響；頁面 JS 真的卡死時它排不上主執行緒，回呼永遠不來，
//!   正好就是要偵測的狀態。
//! - 不需要改頁面（不新增 IPC 指令、不動 `host/ui/`）。
//!
//! controller 裁決（task-5.6-brief）另外要求兩件事，都在 [`Watchdog::tick`]：
//!
//! 1. **只在未暫停時計時**：暫停原因集合（5.5，`AppState::pause_reasons`）非空時不送探測、
//!    並把所有逾時計數歸零——鎖定、顯示器關閉、全螢幕等情境下系統本來就可能延後或凍結
//!    webview，這段時間的「沒回應」不能算數；恢復後從零開始重新累計。
//! 2. **需連續多次逾時**：[`WATCHDOG_MISS_LIMIT`] 次連續探測都沒回應才判定卡死。門檻理由見
//!    該常數文件。

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};

use crate::widgets::{self, AppState};

/// 看門狗探測間隔。
///
/// 5 秒：比頁面正常情況下處理一段 `ExecuteScript` 的時間（毫秒級）大好幾個數量級，偶發的
/// 主執行緒忙碌（大量重繪、GC、宿主主執行緒正在建立其他視窗）不會讓一次探測誤判；又夠短，
/// 卡死後在 [`WATCHDOG_MISS_LIMIT`] × 5 秒內（15–20 秒）就能重建。
pub const WATCHDOG_INTERVAL: Duration = Duration::from_secs(5);

/// 連續幾次探測沒有回應才判定卡死。
///
/// 3 次＝同一個頁面**連續 15 秒以上**完全沒有執行過任何一段腳本。小工具頁面沒有任何合理
/// 情境會讓 JS 主執行緒連續佔用這麼久（渲染一次是毫秒級），單次或兩次逾時只可能是暫時性
/// 延遲（例如剛從睡眠恢復、頁面剛好在重新載入、宿主主執行緒正忙著建立視窗，探測腳本排在
/// 後面）；要求連續 3 次把這些情況都排除掉，代價只是卡死後多等 10 秒才重建。
pub const WATCHDOG_MISS_LIMIT: u32 = 3;

/// 探測腳本的回傳值（JSON 字串化後出現在完成回呼的結果裡）。回呼帶回的字串必須含這個記號
/// 才算活著——WebView2 在執行失敗時也可能呼叫完成回呼（wry 會把錯誤碼丟掉、只傳結果字串），
/// 用記號區分「腳本真的在頁面上跑完」與「失敗回報」。
pub const PROBE_TOKEN: &str = "fc-host-alive";

/// 探測腳本本體：只回傳記號，不讀寫頁面任何狀態。
pub const PROBE_SCRIPT: &str = "'fc-host-alive'";

/// browser 行程結束後，等多久才開始重建。每個 webview 的 `BROWSER_PROCESS_EXITED` 在數毫秒
/// 內陸續送達（探針 1.4：3 個視窗 3 ms 內），稍等一下讓舊環境完全收掉再以同一個 user data
/// folder 建立新 browser process（探針用 1 秒；這裡取 0.5 秒，重建總耗時仍遠低於規格的
/// 10 秒）。
pub const BROWSER_REBUILD_DELAY: Duration = Duration::from_millis(500);

/// 重建失敗時的重試間隔（依序使用；用完就放棄並記錄錯誤）。
pub const REBUILD_RETRY_DELAYS: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
];

/// 同一個小工具在 [`REBUILD_BUDGET_WINDOW`] 內最多自動重建幾次（看門狗／無回應／`Reload`
/// 失敗三條路徑共用）。頁面若因程式錯誤一載入就卡死，沒有上限會每 15 秒重建一次、無止盡
/// 地循環；超過上限就暫停自動重建並記錄錯誤，等額度釋出（[`RebuildBudget::retry_after`]）
/// 再交還看門狗監看（fix F2，review 5.6 low）。
pub const REBUILD_BUDGET_PER_WIDGET: usize = 3;

/// 「browser 行程結束 → 全部重建」在 [`REBUILD_BUDGET_WINDOW`] 內最多幾次。WebView2 執行階段
/// 本身壞掉時新 browser 會一啟動就結束，沒有上限會變成無窮重建迴圈。
pub const REBUILD_BUDGET_ALL: usize = 5;

/// 自動重建次數上限的計算區間。
pub const REBUILD_BUDGET_WINDOW: Duration = Duration::from_secs(600);

/// [`REBUILD_BUDGET_ALL`] 在 [`RebuildBudget`] 裡用的鍵（小工具 id 不會是這個字串）。
const ALL_WIDGETS_KEY: &str = "*";

// ── 純邏輯 ──────────────────────────────────────────────────────────────────────

/// WebView2 故障種類（`crate::desktop` 從 `COREWEBVIEW2_PROCESS_FAILED_KIND` 翻譯而來，本模組
/// 不直接認得 WebView2 的常數）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebviewFailureKind {
    RenderProcessExited,
    RenderProcessUnresponsive,
    BrowserProcessExited,
    GpuProcessExited,
    /// 其餘種類（utility、sandbox helper、iframe renderer 等）：小工具沒有 iframe，這些行程
    /// 結束不影響頁面內容，只記錄。
    Other,
}

/// 對一次故障要採取的動作（[`failure_response`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureResponse {
    /// 對事件來源 webview `Reload()`。
    Reload,
    /// 重建事件來源那一個小工具視窗。
    RebuildWindow,
    /// 重建全部小工具視窗（先建後拆）。
    RebuildAll,
    /// 只記錄。
    LogOnly,
}

/// 故障種類 → 動作（design.md D12，探針 1.4 定案；本模組文件的對照表）。
pub fn failure_response(kind: WebviewFailureKind) -> FailureResponse {
    match kind {
        WebviewFailureKind::RenderProcessExited => FailureResponse::Reload,
        WebviewFailureKind::RenderProcessUnresponsive => FailureResponse::RebuildWindow,
        WebviewFailureKind::BrowserProcessExited => FailureResponse::RebuildAll,
        WebviewFailureKind::GpuProcessExited | WebviewFailureKind::Other => {
            FailureResponse::LogOnly
        }
    }
}

/// `BROWSER_PROCESS_EXITED` 的去重：同一個 browser 行程結束時，**每個** webview 都會各收到
/// 一次事件（探針 1.4），只能處理一次。
///
/// - 以事件來源 webview 訂閱當下讀到的 browser 行程 PID 為鍵：最近一次處理過的 PID 不再處理
///   （重建後新 browser 的 PID 不同，下一次故障照常處理；舊 webview 晚到的事件因 PID 已處理而
///   略過）。只記最近一次，見 [`Self::finish`]。
/// - 讀不到 PID（`None`）時退回「重建進行中就略過」：[`Self::begin`] 到 [`Self::finish`]
///   之間的所有事件一律略過。
#[derive(Debug, Default)]
pub struct BrowserExitDedup {
    handled: HashSet<u32>,
    pending: bool,
    /// 進行中這一次重建所處理的 PID（[`Self::finish`] 時只留它）。
    current: Option<u32>,
}

impl BrowserExitDedup {
    /// 這一筆事件是否應該啟動重建。回傳 `true` 時呼叫端負責在重建結束後呼叫
    /// [`Self::finish`]。
    pub fn begin(&mut self, browser_pid: Option<u32>) -> bool {
        if self.pending {
            return false;
        }
        if let Some(pid) = browser_pid {
            if !self.handled.insert(pid) {
                return false;
            }
        }
        self.pending = true;
        self.current = browser_pid;
        true
    }

    /// 重建結束（不論成敗）。
    ///
    /// fix F2（review 5.6 low）：只保留**這一次**處理的 PID 擋舊 webview 晚到的事件，更早的 PID
    /// 全部忘掉——Windows 會回收 PID，長時間常駐下新一代 browser 可能拿到先前處理過的 PID，
    /// 永久記住會讓它結束時被當成重複事件略過。
    pub fn finish(&mut self) {
        self.pending = false;
        self.handled.clear();
        if let Some(pid) = self.current.take() {
            self.handled.insert(pid);
        }
    }
}

/// 看門狗單一視窗的狀態。
#[derive(Debug, Default, Clone, Copy)]
struct ProbeState {
    /// 已送出、尚未收到回應的最新一次探測序號。
    outstanding: Option<u64>,
    /// 連續幾次 tick 時上一次探測仍未回應。
    misses: u32,
}

/// [`Watchdog::tick`] 的結果。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct TickPlan {
    /// 這一輪要送出的探測：(視窗 label, 序號)。
    pub probes: Vec<(String, u64)>,
    /// 判定卡死、要重建的視窗 label（已自動 [`Watchdog::retire`]，之後不再探測）。
    pub hung: Vec<String>,
}

/// 卡死偵測看門狗（純狀態機；時間由呼叫端每 [`WATCHDOG_INTERVAL`] 呼叫一次
/// [`Self::tick`] 推進，見本模組文件「看門狗」一節）。
#[derive(Debug, Default)]
pub struct Watchdog {
    probes: HashMap<String, ProbeState>,
    /// 已判定卡死或正在被重建的視窗：不再探測、回應也不再理會（舊視窗等著被銷毀）。
    retired: HashSet<String>,
    next_seq: u64,
}

impl Watchdog {
    /// 推進一輪。
    ///
    /// - `live`：目前存在的小工具視窗 label。不在清單裡的視窗（已關閉）其狀態一併丟棄。
    /// - `paused`：暫停原因集合非空。暫停中不送探測、所有計數歸零（controller 裁決：只在
    ///   未暫停時計時）。
    /// - 未暫停時，對每個未 retire 的視窗：上一次探測還沒回應 → 逾時計數＋1；達到
    ///   [`WATCHDOG_MISS_LIMIT`] → 列入 `hung` 並 retire，不再送探測；否則送一次新的探測。
    pub fn tick(&mut self, live: &[String], paused: bool) -> TickPlan {
        // 已關閉的視窗：丟掉狀態（同 label 日後重新開啟時從零開始）。
        self.probes.retain(|label, _| live.contains(label));
        self.retired.retain(|label| live.contains(label));

        let mut plan = TickPlan::default();
        if paused {
            for state in self.probes.values_mut() {
                *state = ProbeState::default();
            }
            return plan;
        }

        for label in live {
            if self.retired.contains(label) {
                continue;
            }
            let state = self.probes.entry(label.clone()).or_default();
            if state.outstanding.is_some() {
                state.misses += 1;
            }
            if state.misses >= WATCHDOG_MISS_LIMIT {
                self.probes.remove(label);
                self.retired.insert(label.clone());
                plan.hung.push(label.clone());
                continue;
            }
            self.next_seq += 1;
            state.outstanding = Some(self.next_seq);
            plan.probes.push((label.clone(), self.next_seq));
        }
        plan
    }

    /// 探測回應（探測腳本在頁面上執行完）：該視窗活著，逾時計數歸零。任何序號的回應都算——
    /// 回應本身就證明頁面主執行緒在回應送出的當下是空閒的。已 retire 的視窗不理會。
    pub fn on_alive(&mut self, label: &str, seq: u64) {
        let _ = seq;
        if self.retired.contains(label) {
            return;
        }
        if let Some(state) = self.probes.get_mut(label) {
            *state = ProbeState::default();
        }
    }

    /// 停止探測這個視窗（它即將被重建／銷毀）。回傳是否為**新**retire——已經 retire 過（已判定
    /// 卡死或已在重建中）回傳 `false`，[`request_window_rebuild`] 據此避免對同一個舊視窗重複
    /// 重建（例如 `RENDER_PROCESS_UNRESPONSIVE` 每幾秒就再送一次）。
    pub fn retire(&mut self, label: &str) -> bool {
        self.probes.remove(label);
        self.retired.insert(label.to_string())
    }

    /// 重建失敗、舊視窗仍留著時恢復探測（下次卡死判定時再試一次，受 [`RebuildBudget`]
    /// 限制）。計數從零開始。
    pub fn unretire(&mut self, label: &str) {
        self.retired.remove(label);
        self.probes.remove(label);
    }
}

/// 自動重建次數上限（滑動時間窗）：同一個鍵在 `window` 內最多 `max` 次。
#[derive(Debug)]
pub struct RebuildBudget {
    max: usize,
    window: Duration,
    history: HashMap<String, VecDeque<Instant>>,
}

impl RebuildBudget {
    pub fn new(max: usize, window: Duration) -> Self {
        Self {
            max,
            window,
            history: HashMap::new(),
        }
    }

    /// 還有額度就記一筆並回傳 `true`；在 `window` 內已用滿 `max` 次則回傳 `false`（不記）。
    pub fn try_take(&mut self, key: &str, now: Instant) -> bool {
        let history = self.history.entry(key.to_string()).or_default();
        while history
            .front()
            .is_some_and(|t| now.saturating_duration_since(*t) >= self.window)
        {
            history.pop_front();
        }
        if history.len() >= self.max {
            return false;
        }
        history.push_back(now);
        true
    }

    /// fix F2（review 5.6 low）：`key` 目前額度已滿時，還要多久才會釋出一格（最舊一筆滿
    /// `window` 的時刻）；還有額度時回傳 `None`。被額度擋下的視窗據此在額度恢復後重新交給
    /// 看門狗監看，而不是永久 retire。
    pub fn retry_after(&self, key: &str, now: Instant) -> Option<Duration> {
        let history = self.history.get(key)?;
        let live: Vec<&Instant> = history
            .iter()
            .filter(|t| now.saturating_duration_since(**t) < self.window)
            .collect();
        if live.len() < self.max {
            return None;
        }
        let oldest = live.first()?;
        Some(self.window - now.saturating_duration_since(**oldest))
    }
}

/// 已被重建取代、等著銷毀的舊視窗 label（`RecoveryState::replaced`）。
///
/// fix F2（review 5.6 high）：label 會被重新使用——小工具關掉再開、或任何路徑以 `w-<id>`
/// 重新建立視窗——所以「已被取代」只對**那一扇舊視窗**成立：同名視窗被建立
/// （[`Self::on_window_created`]）或舊視窗銷毀完成（[`Self::on_window_destroyed`]）時一律清掉，
/// 否則新視窗之後故障會被當成「已被取代」而略過、永遠不再自動復原。
#[derive(Debug, Default)]
pub struct ReplacedLabels {
    labels: HashSet<String>,
}

impl ReplacedLabels {
    /// 新視窗已建好、舊視窗即將 `destroy()`。
    pub fn mark(&mut self, label: &str) {
        self.labels.insert(label.to_string());
    }

    pub fn contains(&self, label: &str) -> bool {
        self.labels.contains(label)
    }

    /// 以 `label` 建立了一扇新視窗（視窗工廠成功時呼叫）：這個 label 現在指向新視窗。
    pub fn on_window_created(&mut self, label: &str) {
        self.labels.remove(label);
    }

    /// `label` 的視窗已銷毀（`WindowEvent::Destroyed`）。
    pub fn on_window_destroyed(&mut self, label: &str) {
        self.labels.remove(label);
    }
}

/// `crate::widgets::rebuild_widget_windows` 的結果。不在 `failed`／`skipped` 的目標＝已重建（或
/// 小工具在設定中已關閉、只拆了舊視窗）。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RebuildOutcome {
    /// 新視窗建立失敗、舊視窗保留不拆的 label（呼叫端決定是否重試）。
    pub failed: Vec<String>,
    /// 沒有動作的 label：已被另一條重建路徑取代、或舊視窗已不存在。**不算成功**——記錄不得
    /// 寫成「重建完成」（review 5.6 high）。
    pub skipped: Vec<String>,
}

/// 單一視窗重建的結果分類（[`single_rebuild_result`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SingleRebuildResult {
    Rebuilt,
    Failed,
    Skipped,
}

/// 只重建 `label` 一扇時，從 [`RebuildOutcome`] 判定它的結果。
pub fn single_rebuild_result(outcome: &RebuildOutcome, label: &str) -> SingleRebuildResult {
    if outcome.failed.iter().any(|l| l == label) {
        SingleRebuildResult::Failed
    } else if outcome.skipped.iter().any(|l| l == label) {
        SingleRebuildResult::Skipped
    } else {
        SingleRebuildResult::Rebuilt
    }
}

/// 重建後新視窗的 label：`w-<id>-r<n>`。舊視窗在新視窗建好之前仍存在（先建後拆），Tauri 的
/// label 必須唯一，故不能沿用 `w-<id>`。`crate::widgets::widget_id_from_label` 是它的反向
/// 解析；`capabilities/default.json` 的 `w-*` 萬用字元同樣涵蓋這個形式。
pub fn rebuilt_window_label(widget_id: &str, n: u64) -> String {
    format!("w-{widget_id}-r{n}")
}

// ── 狀態與協調（非純函式，實機驗收見 host/tools/verify-5.6.ps1）──────────────────────

/// 復原相關的共用狀態（掛在 [`AppState::recovery`]）。
pub struct RecoveryState {
    pub dedup: Mutex<BrowserExitDedup>,
    pub watchdog: Mutex<Watchdog>,
    pub widget_budget: Mutex<RebuildBudget>,
    pub all_budget: Mutex<RebuildBudget>,
    /// 已被重建取代（新視窗已建立、舊視窗已送出 `destroy()`）的舊 label。`destroy()` 是非同步
    /// 的，舊視窗可能還會在 `app.webview_windows()` 裡出現一小段時間；
    /// `crate::widgets::rebuild_widget_windows` 據此略過，避免兩條重建路徑（例如單窗重建剛做完、
    /// 全部重建緊接著開始）對同一個舊視窗各建一個替代品。
    pub replaced: Mutex<ReplacedLabels>,
    label_seq: AtomicU64,
}

impl Default for RecoveryState {
    fn default() -> Self {
        Self {
            dedup: Mutex::new(BrowserExitDedup::default()),
            watchdog: Mutex::new(Watchdog::default()),
            widget_budget: Mutex::new(RebuildBudget::new(
                REBUILD_BUDGET_PER_WIDGET,
                REBUILD_BUDGET_WINDOW,
            )),
            all_budget: Mutex::new(RebuildBudget::new(
                REBUILD_BUDGET_ALL,
                REBUILD_BUDGET_WINDOW,
            )),
            replaced: Mutex::new(ReplacedLabels::default()),
            label_seq: AtomicU64::new(1),
        }
    }
}

impl RecoveryState {
    /// 下一個重建視窗 label（[`rebuilt_window_label`]，序號全宿主遞增、不重複）。
    pub fn next_label(&self, widget_id: &str) -> String {
        rebuilt_window_label(widget_id, self.label_seq.fetch_add(1, Ordering::Relaxed))
    }
}

/// `crate::desktop::install_process_failed_handler` 的回呼：已經記錄過事件本身，這裡只負責
/// 復原動作。在 UI（主）執行緒上被呼叫——**不可**在這裡同步建立視窗（`build()` 在主執行緒
/// 的事件處理中會死鎖，同 `widgets::update_settings` 的說明），重建一律丟到獨立執行緒。
///
/// `reload`：對事件來源 webview 呼叫 `Reload()`（由 desktop 模組提供，只在這個回呼執行期間
/// 有效）。
pub fn on_webview_failure(
    app: &AppHandle,
    label: &str,
    kind: WebviewFailureKind,
    browser_pid: Option<u32>,
    reload: &dyn Fn() -> Result<(), String>,
) {
    // installer-auto-update task 3.3（審查 I1）：正在因更新結束時，視窗本來就要被銷毀，不復原（也不重建）。
    if crate::updater::is_exiting_for_update() {
        log::info!("WebView2 復原：正在因更新結束，略過 widget={label} 的故障（{kind:?}）");
        return;
    }
    match failure_response(kind) {
        FailureResponse::LogOnly => {}
        FailureResponse::Reload => match reload() {
            Ok(()) => log::info!("WebView2 復原：已重新載入 widget={label}（renderer 結束）"),
            Err(err) => {
                log::error!("WebView2 復原：重新載入失敗 widget={label}：{err}；改為重建視窗");
                request_window_rebuild(app, label, "重新載入失敗");
            }
        },
        FailureResponse::RebuildWindow => request_window_rebuild(app, label, "renderer 無回應"),
        FailureResponse::RebuildAll => request_rebuild_all(app, label, browser_pid),
    }
}

/// 重建單一小工具視窗（無回應／`Reload()` 失敗）。先 retire 讓看門狗不再對舊視窗動作；已經
/// retire 過（已在重建中、或已判定卡死）就略過，避免同一個舊視窗被重建兩次。
pub fn request_window_rebuild(app: &AppHandle, label: &str, reason: &str) {
    if crate::updater::is_exiting_for_update() {
        log::info!("WebView2 復原：正在因更新結束，不重建 widget={label}（原因：{reason}）");
        return;
    }
    let newly_retired = app
        .state::<AppState>()
        .recovery
        .watchdog
        .lock()
        .expect("watchdog mutex poisoned")
        .retire(label);
    if !newly_retired {
        log::debug!("WebView2 復原：widget={label} 已在重建中，略過（原因：{reason}）");
        return;
    }
    spawn_window_rebuild(app, label, reason);
}

/// 已 retire 的舊視窗：檢查 [`REBUILD_BUDGET_PER_WIDGET`]，在獨立執行緒重建。看門狗判定卡死時
/// [`Watchdog::tick`] 已經 retire 過，直接走這裡。
fn spawn_window_rebuild(app: &AppHandle, label: &str, reason: &str) {
    if crate::updater::is_exiting_for_update() {
        log::info!("WebView2 復原：正在因更新結束，不重建 widget={label}（原因：{reason}）");
        return;
    }
    let Some(id) = widgets::widget_id_from_label(label) else {
        log::warn!("WebView2 復原：{label} 不是小工具視窗，不重建");
        return;
    };
    let state = app.state::<AppState>();
    let allowed = state
        .recovery
        .widget_budget
        .lock()
        .expect("widget_budget mutex poisoned")
        .try_take(id, Instant::now());
    if !allowed {
        // fix F2（review 5.6 low）：不永久 retire——額度釋出後交還看門狗，仍卡死就再試一次。
        let wait = state
            .recovery
            .widget_budget
            .lock()
            .expect("widget_budget mutex poisoned")
            .retry_after(id, Instant::now())
            .unwrap_or(Duration::ZERO);
        log::error!(
            "WebView2 復原：widget={label} 在 {} 秒內已自動重建 {REBUILD_BUDGET_PER_WIDGET} 次，\
             暫停自動重建 {} 秒後恢復監看（原因：{reason}）",
            REBUILD_BUDGET_WINDOW.as_secs(),
            wait.as_secs()
        );
        unretire_after(app, label.to_string(), wait);
        return;
    }
    log::warn!("WebView2 復原：重建視窗 widget={label}（原因：{reason}）");
    let handle = app.clone();
    let label = label.to_string();
    thread::spawn(move || {
        let started = Instant::now();
        let outcome = widgets::rebuild_widget_windows(&handle, std::slice::from_ref(&label));
        match single_rebuild_result(&outcome, &label) {
            SingleRebuildResult::Rebuilt => log::info!(
                "WebView2 復原：widget={label} 重建完成，耗時 {} ms",
                started.elapsed().as_millis()
            ),
            // 已被另一條重建路徑取代或已不存在：舊視窗正在消失，沒有東西要監看。
            SingleRebuildResult::Skipped => log::warn!(
                "WebView2 復原：widget={label} 未重建（已被取代或已不存在），不視為重建完成"
            ),
            SingleRebuildResult::Failed => {
                // 舊視窗還在（建立失敗時不拆舊的）：恢復探測，下次判定卡死時再試一次。
                unretire_after(&handle, label, Duration::ZERO);
            }
        }
    });
}

/// `delay` 之後把 `label` 交還看門狗監看（[`Watchdog::unretire`]）。`delay` 為零時立即執行。
/// 視窗在這段期間消失也無妨：[`Watchdog::tick`] 只保留存在的視窗。
fn unretire_after(app: &AppHandle, label: String, delay: Duration) {
    let unretire = |app: &AppHandle, label: &str| {
        app.state::<AppState>()
            .recovery
            .watchdog
            .lock()
            .expect("watchdog mutex poisoned")
            .unretire(label);
    };
    if delay.is_zero() {
        unretire(app, &label);
        return;
    }
    let handle = app.clone();
    thread::spawn(move || {
        thread::sleep(delay);
        log::info!("WebView2 復原：widget={label} 重建額度已恢復，重新交給看門狗監看");
        unretire(&handle, &label);
    });
}

/// browser 行程結束：去重、檢查 [`REBUILD_BUDGET_ALL`]，在獨立執行緒「先建新一代全部小工具
/// 視窗、再拆舊的」，失敗者依 [`REBUILD_RETRY_DELAYS`] 重試。設定視窗若開著也一併重開（它和
/// 小工具共用同一個 browser 行程，內容同樣已經停擺）。
fn request_rebuild_all(app: &AppHandle, label: &str, browser_pid: Option<u32>) {
    if crate::updater::is_exiting_for_update() {
        log::info!("WebView2 復原：正在因更新結束，不重建全部視窗（觸發：{label}）");
        return;
    }
    let state = app.state::<AppState>();
    let first = state
        .recovery
        .dedup
        .lock()
        .expect("dedup mutex poisoned")
        .begin(browser_pid);
    if !first {
        log::info!(
            "WebView2 復原：browser 行程結束（browserPid={browser_pid:?}）已在處理，略過 \
             widget={label} 的重複事件"
        );
        return;
    }
    let allowed = state
        .recovery
        .all_budget
        .lock()
        .expect("all_budget mutex poisoned")
        .try_take(ALL_WIDGETS_KEY, Instant::now());
    if !allowed {
        log::error!(
            "WebView2 復原：{} 秒內 browser 行程已結束並重建 {REBUILD_BUDGET_ALL} 次，停止自動\
             重建（WebView2 執行階段可能異常）",
            REBUILD_BUDGET_WINDOW.as_secs()
        );
        state
            .recovery
            .dedup
            .lock()
            .expect("dedup mutex poisoned")
            .finish();
        return;
    }
    log::warn!(
        "WebView2 復原：browser 行程結束（browserPid={browser_pid:?}，首先回報 widget={label}），\
         {} ms 後重建全部小工具視窗",
        BROWSER_REBUILD_DELAY.as_millis()
    );
    let handle = app.clone();
    thread::spawn(move || {
        let started = Instant::now();
        thread::sleep(BROWSER_REBUILD_DELAY);
        let state = handle.state::<AppState>();
        let mut targets = widgets::widget_window_labels(&handle);
        {
            let mut watchdog = state
                .recovery
                .watchdog
                .lock()
                .expect("watchdog mutex poisoned");
            for target in &targets {
                watchdog.retire(target);
            }
        }
        let mut delays = REBUILD_RETRY_DELAYS.iter();
        let mut skipped: Vec<String> = Vec::new();
        loop {
            let outcome = widgets::rebuild_widget_windows(&handle, &targets);
            skipped.extend(outcome.skipped);
            targets = outcome.failed;
            if targets.is_empty() {
                break;
            }
            match delays.next() {
                Some(delay) => {
                    log::warn!(
                        "WebView2 復原：{} 個視窗重建失敗，{} ms 後重試：{targets:?}",
                        targets.len(),
                        delay.as_millis()
                    );
                    thread::sleep(*delay);
                }
                None => {
                    // fix F2（review 5.6 low）：舊視窗還在（建立失敗不拆舊的），交還看門狗監看
                    // ——仍卡死／空白時由逐窗重建（受每窗額度限制）接手，不永久 retire。
                    log::error!(
                        "WebView2 復原：重試用盡，仍有視窗重建失敗（交還看門狗監看）：{targets:?}"
                    );
                    for target in &targets {
                        unretire_after(&handle, target.clone(), Duration::ZERO);
                    }
                    break;
                }
            }
        }
        reopen_settings_window_if_open(&handle);
        if targets.is_empty() && skipped.is_empty() {
            log::info!(
                "WebView2 復原：browser 行程結束後的全部重建完成，自事件起 {} ms",
                started.elapsed().as_millis()
            );
        } else {
            log::warn!(
                "WebView2 復原：browser 行程結束後的重建結束（未全部完成），自事件起 {} ms；\
                 失敗 {targets:?}、略過（已被取代或已不存在）{skipped:?}",
                started.elapsed().as_millis()
            );
        }
        state
            .recovery
            .dedup
            .lock()
            .expect("dedup mutex poisoned")
            .finish();
    });
}

/// 設定視窗（`crate::tray::SETTINGS_WINDOW_LABEL`）若開著：銷毀後重新開啟。label 固定，必須
/// 等舊視窗真的從 Tauri 的視窗表移除（`destroy()` 是非同步的）才能以同名重建；最多等 3 秒，
/// 等不到就只記錄、不重開（使用者可從系統匣重新開啟）。
fn reopen_settings_window_if_open(app: &AppHandle) {
    let Some(window) = app.get_webview_window(crate::tray::SETTINGS_WINDOW_LABEL) else {
        return;
    };
    if let Err(err) = window.destroy() {
        log::warn!("WebView2 復原：關閉舊設定視窗失敗：{err}");
        return;
    }
    for _ in 0..60 {
        if app
            .get_webview_window(crate::tray::SETTINGS_WINDOW_LABEL)
            .is_none()
        {
            log::info!("WebView2 復原：重新開啟設定視窗");
            crate::tray::open_settings_window(app);
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    log::warn!("WebView2 復原：舊設定視窗 3 秒內未移除，不重新開啟（可從系統匣開啟）");
}

/// 啟動看門狗執行緒（`main.rs` setup 呼叫一次）。每 [`WATCHDOG_INTERVAL`]：讀暫停狀態與目前
/// 小工具視窗 → [`Watchdog::tick`] → 卡死者 [`request_window_rebuild`]、其餘送探測。
pub fn spawn_watchdog(app: &AppHandle) {
    let app = app.clone();
    thread::spawn(move || loop {
        thread::sleep(WATCHDOG_INTERVAL);
        watchdog_round(&app);
    });
}

fn watchdog_round(app: &AppHandle) {
    let state = app.state::<AppState>();
    let paused = !state
        .pause_reasons
        .lock()
        .expect("pause_reasons mutex poisoned")
        .is_empty();
    let live = widgets::widget_window_labels(app);
    let plan = state
        .recovery
        .watchdog
        .lock()
        .expect("watchdog mutex poisoned")
        .tick(&live, paused);

    for label in plan.hung {
        log::error!(
            "WebView2 故障 widget={label} kind=WATCHDOG_TIMEOUT（連續 {WATCHDOG_MISS_LIMIT} 次、\
             每次 {} 秒探測無回應，判定頁面卡死）",
            WATCHDOG_INTERVAL.as_secs()
        );
        spawn_window_rebuild(app, &label, "看門狗逾時");
    }

    for (label, seq) in plan.probes {
        let Some(window) = app.get_webview_window(&label) else {
            continue;
        };
        let handle = app.clone();
        let probe_label = label.clone();
        let sent = window.eval_with_callback(PROBE_SCRIPT, move |result| {
            if result.contains(PROBE_TOKEN) {
                handle
                    .state::<AppState>()
                    .recovery
                    .watchdog
                    .lock()
                    .expect("watchdog mutex poisoned")
                    .on_alive(&probe_label, seq);
            }
        });
        if let Err(err) = sent {
            // 送不出去（視窗正在關閉等）：這一輪就當沒回應，由逾時計數處理。
            log::debug!("看門狗探測送出失敗 widget={label}：{err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    // ── failure_response ─────────────────────────────────────────────────────

    #[test]
    fn render_exit_reloads_unresponsive_rebuilds_window_browser_exit_rebuilds_all() {
        assert_eq!(
            failure_response(WebviewFailureKind::RenderProcessExited),
            FailureResponse::Reload
        );
        assert_eq!(
            failure_response(WebviewFailureKind::RenderProcessUnresponsive),
            FailureResponse::RebuildWindow
        );
        assert_eq!(
            failure_response(WebviewFailureKind::BrowserProcessExited),
            FailureResponse::RebuildAll
        );
    }

    #[test]
    fn gpu_and_other_process_exits_are_log_only() {
        assert_eq!(
            failure_response(WebviewFailureKind::GpuProcessExited),
            FailureResponse::LogOnly
        );
        assert_eq!(
            failure_response(WebviewFailureKind::Other),
            FailureResponse::LogOnly
        );
    }

    // ── BrowserExitDedup ─────────────────────────────────────────────────────

    #[test]
    fn dedup_handles_each_browser_pid_only_once_even_after_finish() {
        let mut dedup = BrowserExitDedup::default();
        assert!(dedup.begin(Some(100)), "第一筆事件啟動重建");
        assert!(
            !dedup.begin(Some(100)),
            "同一 browser 的其他 webview 事件略過"
        );
        dedup.finish();
        assert!(
            !dedup.begin(Some(100)),
            "重建完成後舊 webview 晚到的事件（同一個已處理的 PID）仍略過"
        );
    }

    #[test]
    fn dedup_handles_new_browser_pid_after_previous_rebuild_finished() {
        let mut dedup = BrowserExitDedup::default();
        assert!(dedup.begin(Some(100)));
        dedup.finish();
        assert!(
            dedup.begin(Some(200)),
            "重建後的新 browser 再次結束要照常處理"
        );
    }

    #[test]
    fn dedup_skips_everything_while_a_rebuild_is_pending() {
        let mut dedup = BrowserExitDedup::default();
        assert!(dedup.begin(Some(100)));
        assert!(!dedup.begin(Some(200)), "重建進行中，不論 PID 都不重複啟動");
        assert!(!dedup.begin(None));
        dedup.finish();
        assert!(dedup.begin(Some(200)));
    }

    #[test]
    fn dedup_without_pid_falls_back_to_pending_flag() {
        let mut dedup = BrowserExitDedup::default();
        assert!(dedup.begin(None));
        assert!(
            !dedup.begin(None),
            "讀不到 PID 時，進行中的重建期間一律略過"
        );
        dedup.finish();
        assert!(
            dedup.begin(None),
            "讀不到 PID、且沒有進行中的重建 → 視為新事件"
        );
    }

    // ── Watchdog ─────────────────────────────────────────────────────────────

    #[test]
    fn watchdog_probes_every_live_window_on_first_tick() {
        let mut dog = Watchdog::default();
        let plan = dog.tick(&labels(&["w-clock", "w-macro"]), false);
        let probed: Vec<&str> = plan.probes.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(probed, vec!["w-clock", "w-macro"]);
        assert!(plan.hung.is_empty());
        assert_ne!(plan.probes[0].1, plan.probes[1].1, "序號不重複");
    }

    #[test]
    fn watchdog_declares_hung_only_after_miss_limit_consecutive_misses() {
        let mut dog = Watchdog::default();
        let live = labels(&["w-clock"]);
        dog.tick(&live, false); // 送出第一次探測
        for round in 1..WATCHDOG_MISS_LIMIT {
            let plan = dog.tick(&live, false);
            assert!(plan.hung.is_empty(), "第 {round} 次逾時還不能判定卡死");
            assert_eq!(plan.probes.len(), 1, "未判定前每輪照常再探測");
        }
        let plan = dog.tick(&live, false);
        assert_eq!(plan.hung, vec!["w-clock".to_string()]);
        assert!(plan.probes.is_empty(), "判定卡死的視窗不再探測");
        let plan = dog.tick(&live, false);
        assert!(plan.hung.is_empty(), "已 retire，不重複回報");
        assert!(plan.probes.is_empty());
    }

    #[test]
    fn watchdog_response_resets_the_miss_count() {
        let mut dog = Watchdog::default();
        let live = labels(&["w-clock"]);
        let mut last = dog.tick(&live, false).probes[0].1;
        // 逾時到只差一次就判定，然後回應 → 計數歸零，要再連續 MISS_LIMIT 次才判定。
        for _ in 1..WATCHDOG_MISS_LIMIT {
            last = dog.tick(&live, false).probes[0].1;
        }
        dog.on_alive("w-clock", last);
        for _ in 0..WATCHDOG_MISS_LIMIT {
            assert!(dog.tick(&live, false).hung.is_empty());
        }
        assert_eq!(dog.tick(&live, false).hung, vec!["w-clock".to_string()]);
    }

    #[test]
    fn watchdog_accepts_a_late_response_to_an_older_probe() {
        let mut dog = Watchdog::default();
        let live = labels(&["w-clock"]);
        let first = dog.tick(&live, false).probes[0].1;
        for _ in 1..WATCHDOG_MISS_LIMIT {
            dog.tick(&live, false);
        }
        dog.on_alive("w-clock", first);
        assert!(
            dog.tick(&live, false).hung.is_empty(),
            "舊探測晚到的回應也證明頁面活著"
        );
    }

    #[test]
    fn watchdog_never_counts_while_paused_and_resets_counts_on_pause() {
        let mut dog = Watchdog::default();
        let live = labels(&["w-clock"]);
        dog.tick(&live, false);
        for _ in 1..WATCHDOG_MISS_LIMIT {
            dog.tick(&live, false);
        }
        // 已差一次就判定卡死；此時進入暫停。
        for _ in 0..(WATCHDOG_MISS_LIMIT * 4) {
            let plan = dog.tick(&live, true);
            assert!(plan.probes.is_empty(), "暫停中不送探測");
            assert!(plan.hung.is_empty(), "暫停中不判定");
        }
        // 恢復後從零開始：第一輪只送探測，還要再連續 MISS_LIMIT 次逾時才判定。
        let plan = dog.tick(&live, false);
        assert!(plan.hung.is_empty());
        assert_eq!(plan.probes.len(), 1);
        for _ in 1..WATCHDOG_MISS_LIMIT {
            assert!(dog.tick(&live, false).hung.is_empty());
        }
        assert_eq!(dog.tick(&live, false).hung, vec!["w-clock".to_string()]);
    }

    #[test]
    fn watchdog_forgets_closed_windows() {
        let mut dog = Watchdog::default();
        dog.tick(&labels(&["w-clock"]), false);
        for _ in 1..WATCHDOG_MISS_LIMIT {
            dog.tick(&labels(&["w-clock"]), false);
        }
        // 視窗被關掉一輪後又以同一 label 重新開啟：不承接關閉前的逾時計數。
        dog.tick(&[], false);
        let plan = dog.tick(&labels(&["w-clock"]), false);
        assert!(plan.hung.is_empty());
        assert_eq!(plan.probes.len(), 1);
    }

    #[test]
    fn watchdog_ignores_retired_windows_and_their_responses() {
        let mut dog = Watchdog::default();
        let live = labels(&["w-clock", "w-macro"]);
        let plan = dog.tick(&live, false);
        dog.retire("w-clock");
        dog.on_alive("w-clock", plan.probes[0].1);
        let plan = dog.tick(&live, false);
        let probed: Vec<&str> = plan.probes.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(probed, vec!["w-macro"], "retire 的視窗不再探測");
        for _ in 0..(WATCHDOG_MISS_LIMIT * 2) {
            assert!(
                !dog.tick(&live, false).hung.contains(&"w-clock".to_string()),
                "retire 的視窗不會被判定卡死"
            );
        }
    }

    #[test]
    fn retire_reports_whether_the_window_was_newly_retired() {
        let mut dog = Watchdog::default();
        assert!(dog.retire("w-clock"), "第一次 retire");
        assert!(!dog.retire("w-clock"), "重複 retire（已在重建中）");
        dog.unretire("w-clock");
        assert!(dog.retire("w-clock"), "unretire 後可再次 retire");
    }

    #[test]
    fn hung_window_is_already_retired_when_reported() {
        let mut dog = Watchdog::default();
        let live = labels(&["w-clock"]);
        for _ in 0..WATCHDOG_MISS_LIMIT {
            dog.tick(&live, false);
        }
        assert_eq!(dog.tick(&live, false).hung, vec!["w-clock".to_string()]);
        assert!(
            !dog.retire("w-clock"),
            "卡死回報時已 retire：同時到來的 UNRESPONSIVE 事件不會再觸發第二次重建"
        );
    }

    #[test]
    fn watchdog_unretire_restarts_monitoring_from_zero() {
        let mut dog = Watchdog::default();
        let live = labels(&["w-clock"]);
        dog.tick(&live, false);
        dog.retire("w-clock");
        dog.unretire("w-clock");
        let plan = dog.tick(&live, false);
        assert_eq!(plan.probes.len(), 1, "恢復後重新探測");
        assert!(plan.hung.is_empty());
        for _ in 1..WATCHDOG_MISS_LIMIT {
            assert!(dog.tick(&live, false).hung.is_empty());
        }
        assert_eq!(dog.tick(&live, false).hung, vec!["w-clock".to_string()]);
    }

    // ── RebuildBudget ────────────────────────────────────────────────────────

    #[test]
    fn budget_allows_up_to_max_within_window_per_key() {
        let mut budget = RebuildBudget::new(3, Duration::from_secs(600));
        let t0 = Instant::now();
        assert!(budget.try_take("clock", t0));
        assert!(budget.try_take("clock", t0 + Duration::from_secs(10)));
        assert!(budget.try_take("clock", t0 + Duration::from_secs(20)));
        assert!(!budget.try_take("clock", t0 + Duration::from_secs(30)));
        assert!(
            budget.try_take("macro", t0 + Duration::from_secs(30)),
            "鍵彼此獨立"
        );
    }

    #[test]
    fn budget_frees_slots_once_they_leave_the_window() {
        let mut budget = RebuildBudget::new(2, Duration::from_secs(600));
        let t0 = Instant::now();
        assert!(budget.try_take("clock", t0));
        assert!(budget.try_take("clock", t0 + Duration::from_secs(100)));
        assert!(!budget.try_take("clock", t0 + Duration::from_secs(599)));
        assert!(
            budget.try_take("clock", t0 + Duration::from_secs(600)),
            "第一筆剛好滿 600 秒即移出時間窗"
        );
        assert!(!budget.try_take("clock", t0 + Duration::from_secs(650)));
    }

    #[test]
    fn denied_attempt_does_not_consume_budget() {
        let mut budget = RebuildBudget::new(1, Duration::from_secs(600));
        let t0 = Instant::now();
        assert!(budget.try_take("clock", t0));
        for s in 1..50 {
            assert!(!budget.try_take("clock", t0 + Duration::from_secs(s)));
        }
        assert!(
            budget.try_take("clock", t0 + Duration::from_secs(600)),
            "被拒的嘗試不記錄，不會把可用時間往後推"
        );
    }

    // ── fix F2（review 5.6）────────────────────────────────────────────────────

    #[test]
    fn dedup_handles_a_reused_pid_after_another_browser_was_handled() {
        let mut dedup = BrowserExitDedup::default();
        assert!(dedup.begin(Some(100)));
        dedup.finish();
        assert!(dedup.begin(Some(200)));
        dedup.finish();
        assert!(
            dedup.begin(Some(100)),
            "PID 100 已被系統重用給新一代 browser：它結束時要照常重建"
        );
    }

    #[test]
    fn replaced_label_is_forgotten_when_the_same_label_is_created_again() {
        let mut replaced = ReplacedLabels::default();
        replaced.mark("w-clock");
        assert!(replaced.contains("w-clock"));
        replaced.on_window_created("w-clock");
        assert!(
            !replaced.contains("w-clock"),
            "同名 w-<id> 被重新建立（開關小工具）後，新視窗不是「已被取代」的舊視窗"
        );
    }

    #[test]
    fn replaced_label_is_forgotten_when_the_old_window_is_destroyed() {
        let mut replaced = ReplacedLabels::default();
        replaced.mark("w-clock");
        replaced.mark("w-macro");
        replaced.on_window_destroyed("w-clock");
        assert!(!replaced.contains("w-clock"));
        assert!(replaced.contains("w-macro"), "只清掉銷毀的那一個");
    }

    #[test]
    fn skipped_target_is_not_reported_as_rebuilt() {
        let outcome = RebuildOutcome {
            failed: vec![],
            skipped: vec!["w-clock".to_string()],
        };
        assert_eq!(
            single_rebuild_result(&outcome, "w-clock"),
            SingleRebuildResult::Skipped
        );
        let outcome = RebuildOutcome {
            failed: vec!["w-clock".to_string()],
            skipped: vec![],
        };
        assert_eq!(
            single_rebuild_result(&outcome, "w-clock"),
            SingleRebuildResult::Failed
        );
        assert_eq!(
            single_rebuild_result(&RebuildOutcome::default(), "w-clock"),
            SingleRebuildResult::Rebuilt
        );
    }

    #[test]
    fn budget_reports_when_the_next_slot_frees_up() {
        let mut budget = RebuildBudget::new(3, Duration::from_secs(600));
        let t0 = Instant::now();
        assert_eq!(budget.retry_after("clock", t0), None, "還有額度");
        for s in [0, 10, 20] {
            assert!(budget.try_take("clock", t0 + Duration::from_secs(s)));
        }
        assert_eq!(
            budget.retry_after("clock", t0 + Duration::from_secs(30)),
            Some(Duration::from_secs(570)),
            "最舊一筆（t0）滿 600 秒時釋出"
        );
        assert_eq!(
            budget.retry_after("clock", t0 + Duration::from_secs(600)),
            None
        );
    }

    // ── label ────────────────────────────────────────────────────────────────

    #[test]
    fn rebuilt_label_has_w_dash_prefix_and_generation_suffix() {
        assert_eq!(rebuilt_window_label("clock", 7), "w-clock-r7");
        assert_eq!(rebuilt_window_label("custom5", 12), "w-custom5-r12");
    }

    #[test]
    fn recovery_state_hands_out_distinct_labels() {
        let state = RecoveryState::default();
        let a = state.next_label("clock");
        let b = state.next_label("clock");
        assert_ne!(a, b);
        assert!(a.starts_with("w-clock-r") && b.starts_with("w-clock-r"));
    }
}
