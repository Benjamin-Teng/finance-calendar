//! 勿打擾偵測（dynamic-wallpaper task 4.7c；dynamic-wallpaper design.md D12；spec desktop-wallpaper
//! 「暫停更新」）。
//!
//! ## 兩個來源、OR 合併
//!
//! - **官方**：WinRT `FocusSessionManager`（`IsSupported` 為真時讀 `GetDefault().IsFocusActive`）。
//! - **非官方**：WNF `WNF_SHEL_QUIETHOURS_ACTIVE_PROFILE_CHANGED` 的 4 位元組整數，**> 0** 視為勿打擾。
//!
//! 兩者的系統呼叫在 `crate::desktop::dnd`；本模組把原始讀值判讀成 [`Tri`]（[`interpret_focus`]／
//! [`interpret_wnf`]），以 [`combine`] 合併：**任一為「是」就暫停**，讀不到（未知）一律**不**暫停——兩者
//! 都讀不到時照常重畫（最壞情況是勿打擾時仍照常重畫）。輪詢執行緒卡住或消失時，最後讀值只在
//! [`DndConfig::stale_after`]（預設 [`STALE_AFTER`]＝3 個輪詢週期）內有效；過期後先立即重讀一次，
//! 重讀也等不到才視為未知（契約 7），所以偵測故障不會讓桌布永遠不畫。
//!
//! ## 呼叫端契約
//!
//! 1. [`DndMonitor::query`] 是協調迴圈 `CoordinatorPorts::do_not_disturb` 的本體，只在「本來要重畫」
//!    時呼叫（主題為「不接管」時從不呼叫），所以主題為「不接管」時**不建立**輪詢執行緒、不做任何讀取，
//!    行為與 4.7c 之前完全相同。
//! 2. 第一次查詢（以及停止輪詢之後的下一次查詢）啟用輪詢：喚醒輪詢執行緒（第一次時建立
//!    `fc-wallpaper-dnd` 執行緒）立刻讀一次，並在呼叫端**最多等** [`DndConfig::first_read_wait`]
//!    （預設 [`FIRST_READ_WAIT`]）等這次讀值，避免啟動或剛選主題時在勿打擾中先畫一張。等不到＝回目前的
//!    快取（未讀過＝不暫停），記一次 warn；讀值之後才到時，若合併結果改變照樣送喚醒。其餘查詢只讀快取、
//!    立即返回，不會阻塞協調迴圈（例外：讀值過期那一段的第一次查詢會再等一次重讀，見契約 7）。
//! 3. 輪詢執行緒每 [`DndConfig::poll_interval`]（預設 [`POLL_INTERVAL`]＝60 秒）先問主題是否接管
//!    （[`DndHooks::takes_over`]）：不接管就停止輪詢（不再讀，直到下一次查詢）；接管就讀兩個來源。
//!    **合併結果改變**才呼叫 [`DndHooks::notify`]（正式＝送 `Wake::DoNotDisturbChanged`）；只有其中
//!    一個來源的讀值變了（例如 WNF 從 1 變 2）不送。輪詢執行緒自己做全部讀取，從不取主執行緒或協調
//!    迴圈的鎖（`takes_over` 只短暫鎖設定）。
//! 4. 協調迴圈收到 `DoNotDisturbChanged` 就再評估：勿打擾解除時，排程器（4.3 契約：錯過的時點只補一次、
//!    取最近的時點）立即補畫；沒有錯過時點就不提前畫。
//! 5. 記錄（target＝協調迴圈的 `LOG_TARGET`，實機證據依 target 擷取）：任一來源的讀值或合併結果改變
//!    時記一行（兩個來源各自的讀值與合併結果）；某來源讀取失敗時，同一段連續失敗只記一次 warn，恢復時
//!    記一行（比照 Busy 的 warn 作法）。
//! 6. COM／WinRT：輪詢執行緒建立來源時在**自己的執行緒**上 `CoInitializeEx(COINIT_MULTITHREADED)`
//!    （[`SystemSources::new`]），不影響桌布 COM 執行緒與主執行緒的公寓。
//! 7. 讀值時效（修正輪 1、2）：每次讀值完成時以 [`DndHooks::clock`] 記下時刻。查詢時若最後一次讀值已
//!    超過 [`DndConfig::stale_after`]，**同一段過期的第一次判定**先要求輪詢執行緒立即重讀、最多等
//!    [`DndConfig::first_read_wait`]：
//!    - 等到（典型情境：正式時鐘是 `Instant`＝QPC，**含睡眠時間**，睡眠超過門檻喚醒後輪詢器只是還沒
//!      輪到）→ 照新值判斷，記一行 info；
//!    - 等不到（讀取卡在沒有回應的跨行程呼叫裡、或輪詢執行緒 panic 後消失）→ 回「不暫停」並記一次 warn；
//!      同一段之後的評估不再等、直接回「不暫停」。
//!
//!    下一次讀值完成時記一行「恢復更新」並回到照讀值判斷。不另排計時器：協調迴圈本來就有 60 秒的等待
//!    上限，門檻過後最慢 60 秒內再評估一次就會生效。
//!
//! ## 為什麼兩半都輪詢
//!
//! `FocusSessionManager` 有官方的 `IsFocusActiveChanged` 事件，但 WNF 半邊不論如何都要輪詢（訂閱 WNF
//! 是另一組未公開 API，brief 裁決不用）；同一條執行緒每 60 秒順手讀官方半邊，比另外維護一個事件註冊
//! （回呼在任意 MTA 執行緒、要保存 token 並在結束時移除）簡單，延遲上限一樣是 60 秒。
//!
//! ## self-test 注入（只在 `self-test-ipc` 建置）
//!
//! 見 `injection` 子模組：以檔案注入假讀值，並可縮短輪詢間隔，供 tasks 6.1 驗收。

use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use log::Level;

use crate::desktop::dnd::WnfRaw;
use crate::wallpaper_coordinator::LogSink;

/// 輪詢間隔（brief 裁決 2：每 60 秒）。
pub const POLL_INTERVAL: Duration = Duration::from_secs(60);

/// 啟用輪詢時，查詢端最多等第一次讀值多久（見模組文件契約 2）。
pub const FIRST_READ_WAIT: Duration = Duration::from_secs(2);

/// 讀值的有效期限（修正輪 1）：3 個輪詢週期沒有新讀值時先立即重讀，重讀也等不到才視為未知、不暫停
/// （見模組文件契約 7）。
pub const STALE_AFTER: Duration = Duration::from_secs(3 * 60);

/// 輪詢執行緒名稱。
const THREAD_NAME: &str = "fc-wallpaper-dnd";

/// 官方來源在記錄裡的名稱。
const FOCUS_NAME: &str = "專注工作階段（FocusSessionManager）";
/// 非官方來源在記錄裡的名稱。
const WNF_NAME: &str = "WNF 勿打擾（非官方）";

// ---------------------------------------------------------------------------------------------
// 判讀與合併（純函式）
// ---------------------------------------------------------------------------------------------

/// 一個來源的判讀結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tri {
    /// 勿打擾中。
    On,
    /// 不在勿打擾。
    Off,
    /// 讀不到（不支援、呼叫失敗、資料不合）：不暫停。
    Unknown,
}

/// 一個來源的讀值：判讀結果＋記錄用的說明（讀值本身或失敗原因）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSample {
    pub tri: Tri,
    pub detail: String,
}

impl SourceSample {
    pub fn on(detail: impl Into<String>) -> Self {
        Self {
            tri: Tri::On,
            detail: detail.into(),
        }
    }

    pub fn off(detail: impl Into<String>) -> Self {
        Self {
            tri: Tri::Off,
            detail: detail.into(),
        }
    }

    pub fn unknown(detail: impl Into<String>) -> Self {
        Self {
            tri: Tri::Unknown,
            detail: detail.into(),
        }
    }

    /// 變化偵測用的鍵：讀得到時是讀值說明，讀不到時一律 `None`（失敗訊息不同不算改變）。
    fn key(&self) -> Option<String> {
        (self.tri != Tri::Unknown).then(|| self.detail.clone())
    }

    fn describe(&self) -> String {
        match self.tri {
            Tri::Unknown => format!("未知（{}）", self.detail),
            Tri::On | Tri::Off => self.detail.clone(),
        }
    }
}

/// 兩個來源合併：任一為 [`Tri::On`] 就暫停；未知不暫停。
pub fn combine(focus: Tri, wnf: Tri) -> bool {
    focus == Tri::On || wnf == Tri::On
}

/// WNF 原始讀值的判讀：`Err`（函式不存在）、非 0 的 NTSTATUS、資料不足 4 位元組＝未知；否則小端序
/// `i32`，**> 0** 為勿打擾（0 與負值都不是）。
pub fn interpret_wnf(raw: Result<WnfRaw, String>) -> SourceSample {
    let raw = match raw {
        Ok(raw) => raw,
        Err(e) => return SourceSample::unknown(e),
    };
    if raw.status != 0 {
        return SourceSample::unknown(format!(
            "NtQueryWnfStateData 回傳 NTSTATUS 0x{:08X}",
            raw.status as u32
        ));
    }
    if raw.size < 4 {
        return SourceSample::unknown(format!("WNF 資料只有 {} 位元組（需要 4）", raw.size));
    }
    let value = i32::from_le_bytes(raw.data);
    let detail = format!("WNF={value}");
    if value > 0 {
        SourceSample::on(detail)
    } else {
        SourceSample::off(detail)
    }
}

/// 官方來源的判讀：`Ok(Some(b))`＝`IsFocusActive`；`Ok(None)`＝`IsSupported` 為假（未知）；`Err`＝未知。
pub fn interpret_focus(result: Result<Option<bool>, String>) -> SourceSample {
    match result {
        Ok(Some(true)) => SourceSample::on("IsFocusActive=true"),
        Ok(Some(false)) => SourceSample::off("IsFocusActive=false"),
        Ok(None) => SourceSample::unknown("IsSupported=false（此系統不支援）"),
        Err(e) => SourceSample::unknown(e),
    }
}

// ---------------------------------------------------------------------------------------------
// 狀態追蹤（純邏輯：變化偵測與記錄去重）
// ---------------------------------------------------------------------------------------------

/// 一次讀值之後的結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    /// 合併結果（是否暫停）。
    pub combined: bool,
    /// 合併結果與上一次不同（要送喚醒）。初始視為「不暫停」。
    pub changed: bool,
    /// 要記的行（等級、訊息）。
    pub lines: Vec<(Level, String)>,
}

/// 兩個來源的上一次讀值與連續失敗狀態。
#[derive(Debug, Default)]
pub struct DndTracker {
    /// 上一次的變化偵測鍵（`None`＝還沒讀過）。
    last_key: Option<(Option<String>, Option<String>)>,
    /// 各來源（官方、WNF）是否在一段連續失敗中。
    failing: [bool; 2],
    /// 上一次的合併結果。
    combined: bool,
}

impl DndTracker {
    /// 記一次讀值，回傳合併結果、是否改變與要記的行。
    pub fn observe(&mut self, focus: &SourceSample, wnf: &SourceSample) -> Observation {
        let mut lines = Vec::new();
        for (i, (name, sample)) in [(FOCUS_NAME, focus), (WNF_NAME, wnf)].iter().enumerate() {
            let failed = sample.tri == Tri::Unknown;
            if failed && !self.failing[i] {
                lines.push((
                    Level::Warn,
                    format!(
                        "勿打擾：{name} 讀取失敗，視為未知、不因此暫停（同一段連續失敗只記這一次）：{}",
                        sample.detail
                    ),
                ));
            } else if !failed && self.failing[i] {
                lines.push((
                    Level::Info,
                    format!("勿打擾：{name} 恢復讀取：{}", sample.detail),
                ));
            }
            self.failing[i] = failed;
        }
        let combined = combine(focus.tri, wnf.tri);
        let key = (focus.key(), wnf.key());
        if self.last_key.as_ref() != Some(&key) || combined != self.combined {
            lines.push((
                Level::Info,
                format!(
                    "勿打擾狀態：{FOCUS_NAME} {}、{WNF_NAME} {} → 合併：{}",
                    focus.describe(),
                    wnf.describe(),
                    if combined {
                        "勿打擾中，暫停重畫"
                    } else {
                        "不暫停"
                    }
                ),
            ));
        }
        self.last_key = Some(key);
        let changed = combined != self.combined;
        self.combined = combined;
        Observation {
            combined,
            changed,
            lines,
        }
    }

    /// 忘掉之前的讀值（停止輪詢時）：之後第一次讀值重新記一行，合併結果從「不暫停」起算。
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

// ---------------------------------------------------------------------------------------------
// 來源
// ---------------------------------------------------------------------------------------------

/// 兩個來源的讀取（正式：[`SystemSources`]；測試：假的；self-test：注入）。輪詢器每次先讀官方、
/// 再讀 WNF。
pub trait DndSources {
    fn read_focus(&mut self) -> SourceSample;
    fn read_wnf(&mut self) -> SourceSample;
}

/// 真正的系統來源（`crate::desktop::dnd`）。必須在輪詢執行緒上建立（MTA 初始化屬於建立它的執行緒），
/// 不是 `Send`。
pub struct SystemSources {
    mta: Result<crate::desktop::dnd::MtaGuard, String>,
}

impl SystemSources {
    /// 在呼叫端執行緒上初始化 MTA（失敗時官方半邊恆為未知，WNF 半邊照讀）。
    pub fn new() -> Self {
        Self {
            mta: crate::desktop::dnd::init_mta_for_this_thread(),
        }
    }
}

impl DndSources for SystemSources {
    fn read_focus(&mut self) -> SourceSample {
        match &self.mta {
            Ok(_) => interpret_focus(crate::desktop::dnd::read_focus_active()),
            Err(e) => SourceSample::unknown(e.clone()),
        }
    }

    fn read_wnf(&mut self) -> SourceSample {
        interpret_wnf(crate::desktop::dnd::query_quiet_hours_wnf())
    }
}

// ---------------------------------------------------------------------------------------------
// 輪詢器
// ---------------------------------------------------------------------------------------------

/// 輪詢器的時間參數。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DndConfig {
    pub poll_interval: Duration,
    pub first_read_wait: Duration,
    /// 最後一次讀值超過這麼久就先立即重讀、等不到才視為未知（契約 7）。
    pub stale_after: Duration,
}

impl Default for DndConfig {
    fn default() -> Self {
        Self {
            poll_interval: POLL_INTERVAL,
            first_read_wait: FIRST_READ_WAIT,
            stale_after: STALE_AFTER,
        }
    }
}

/// 時效判定用的時鐘（正式：[`system_clock`]；測試：假時鐘）。
pub type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

/// 系統的單調時鐘。
pub fn system_clock() -> Clock {
    Arc::new(Instant::now)
}

/// 在輪詢執行緒上建立來源（MTA 初始化在那條執行緒上做）。
pub type SourcesFactory = Box<dyn FnOnce() -> Box<dyn DndSources> + Send>;

/// 輪詢器對宿主的依賴。
pub struct DndHooks {
    /// 建立來源（在輪詢執行緒上呼叫一次）。
    pub sources: SourcesFactory,
    /// 主題是否接管（不是「不接管」）；只短暫鎖設定。
    pub takes_over: Box<dyn Fn() -> bool + Send>,
    /// 合併結果改變（正式：送 `Wake::DoNotDisturbChanged`；不得等待）。
    pub notify: Box<dyn Fn() + Send>,
    /// 記錄。
    pub log: LogSink,
    /// 讀值時效的時鐘（契約 7；輪詢排程照真實時間走）。
    pub clock: Clock,
}

/// 輪詢執行緒才需要的部分（第一次啟用時交給執行緒）。
struct PollerParts {
    sources: SourcesFactory,
    takes_over: Box<dyn Fn() -> bool + Send>,
    notify: Box<dyn Fn() + Send>,
}

struct MonitorState {
    /// 輪詢中（查詢啟用、主題不接管時停止）。
    enabled: bool,
    /// 最近一次讀值的合併結果。
    active: bool,
    /// 已完成的讀值次數（查詢端以它判斷「有沒有新的讀值」）。
    completed: u64,
    /// 查詢端要求立刻讀一次。
    read_requested: bool,
    shutdown: bool,
    /// 還沒交給輪詢執行緒的依賴（`None`＝執行緒已建立或建立失敗）。
    pending: Option<PollerParts>,
    /// 執行緒建立失敗（之後恆為不暫停）。
    spawn_failed: bool,
    /// 這一段「等不到第一次讀值」已記過 warn。
    wait_warned: bool,
    /// 最近一次讀值完成的時刻（[`Inner::clock`]；`None`＝停止輪詢後還沒讀過）。
    last_read_at: Option<Instant>,
    /// 這一段「讀值過期」已記過 warn。
    stale_warned: bool,
}

struct Inner {
    state: Mutex<MonitorState>,
    cv: Condvar,
    config: DndConfig,
    log: LogSink,
    clock: Clock,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 勿打擾監看（見模組文件「呼叫端契約」）。丟棄時結束輪詢執行緒（不等待它）。
pub struct DndMonitor {
    inner: Arc<Inner>,
}

impl DndMonitor {
    /// 建立監看。**不**建立執行緒、不讀任何來源：第一次 [`Self::query`] 才開始。
    pub fn new(config: DndConfig, hooks: DndHooks) -> Self {
        let DndHooks {
            sources,
            takes_over,
            notify,
            log,
            clock,
        } = hooks;
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(MonitorState {
                    enabled: false,
                    active: false,
                    completed: 0,
                    read_requested: false,
                    shutdown: false,
                    pending: Some(PollerParts {
                        sources,
                        takes_over,
                        notify,
                    }),
                    spawn_failed: false,
                    wait_warned: false,
                    last_read_at: None,
                    stale_warned: false,
                }),
                cv: Condvar::new(),
                config,
                log,
                clock,
            }),
        }
    }

    fn say(&self, level: Level, msg: &str) {
        (self.inner.log)(level, msg);
    }

    /// 最後一次讀值是否已超過 [`DndConfig::stale_after`]（契約 7）。
    fn is_stale(&self, st: &MonitorState) -> bool {
        st.last_read_at.is_some_and(|at| {
            (self.inner.clock)().saturating_duration_since(at) > self.inner.config.stale_after
        })
    }

    /// 要求輪詢執行緒立刻讀一次，最多等 [`DndConfig::first_read_wait`]；回傳上鎖的狀態與是否等到。
    fn request_read_and_wait<'a>(
        &self,
        mut st: MutexGuard<'a, MonitorState>,
    ) -> (MutexGuard<'a, MonitorState>, bool) {
        st.read_requested = true;
        let target = st.completed + 1;
        self.inner.cv.notify_all();
        let deadline = Instant::now() + self.inner.config.first_read_wait;
        while st.completed < target {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return (st, false);
            }
            st = self
                .inner
                .cv
                .wait_timeout(st, left)
                .map(|(g, _)| g)
                .unwrap_or_else(|e| e.into_inner().0);
        }
        (st, true)
    }

    /// 快取的判定，套用讀值時效（契約 7）：最後一次讀值太舊＝未知＝不暫停。
    fn judged(&self, st: &mut MonitorState) -> bool {
        if let Some(at) = st.last_read_at {
            let age = (self.inner.clock)().saturating_duration_since(at);
            if age > self.inner.config.stale_after {
                if !st.stale_warned {
                    st.stale_warned = true;
                    self.say(
                        Level::Warn,
                        &format!(
                            "勿打擾：讀值已 {} 秒沒有更新（上限 {} 秒），立即重讀也等不到（輪詢執行緒可能卡在讀取裡），視為未知、不暫停",
                            age.as_secs(),
                            self.inner.config.stale_after.as_secs()
                        ),
                    );
                }
                return false;
            }
        }
        st.active
    }

    /// 勿打擾是否成立（協調迴圈只在本來要重畫時呼叫）。停止輪詢中時先啟用並等第一次讀值（上限
    /// [`DndConfig::first_read_wait`]），其餘情況回快取、立即返回；讀值過期那一段的第一次查詢另會等
    /// 一次重讀（同一上限，見模組文件契約 7）。
    pub fn query(&self) -> bool {
        let inner = &self.inner;
        let mut st = lock(&inner.state);
        if st.spawn_failed {
            return self.judged(&mut st);
        }
        if st.enabled {
            // 契約 7（修正輪 2）：同一段過期的第一次判定先要求立即重讀（睡眠喚醒後輪詢器只是還沒輪到），
            // 等到就照新值判斷；等不到才由 `judged` 判成未知並記 warn——之後同一段不再等。
            if !st.stale_warned && self.is_stale(&st) {
                let (guard, fresh) = self.request_read_and_wait(st);
                st = guard;
                if fresh {
                    let active = st.active;
                    drop(st);
                    self.say(
                        Level::Info,
                        &format!(
                            "勿打擾：讀值已過期（例如剛從睡眠喚醒），立即重讀拿到新值（{}）",
                            if active { "勿打擾中" } else { "不暫停" }
                        ),
                    );
                    return active;
                }
            }
            return self.judged(&mut st);
        }
        st.enabled = true;
        if let Some(parts) = st.pending.take() {
            let for_thread = Arc::clone(inner);
            let spawned = thread::Builder::new()
                .name(THREAD_NAME.to_owned())
                .spawn(move || run_poller(&for_thread, parts));
            match spawned {
                Ok(_) => self.say(
                    Level::Info,
                    &format!(
                        "勿打擾：啟動輪詢執行緒（每 {} 秒讀一次；主題為「不接管」時停止）",
                        inner.config.poll_interval.as_secs_f32()
                    ),
                ),
                Err(e) => {
                    st.spawn_failed = true;
                    st.active = false;
                    self.say(
                        Level::Error,
                        &format!("勿打擾：無法建立輪詢執行緒，之後一律視為未知、不暫停：{e}"),
                    );
                    return false;
                }
            }
        } else {
            self.say(Level::Info, "勿打擾：恢復輪詢（需要重畫時重新讀取）");
        }
        let (guard, fresh) = self.request_read_and_wait(st);
        st = guard;
        if !fresh {
            if !st.wait_warned {
                st.wait_warned = true;
                let active = self.judged(&mut st);
                drop(st);
                self.say(
                    Level::Warn,
                    &format!(
                        "勿打擾：{} 秒內等不到第一次讀值，先以目前的快取（{}）判定；讀值到了若改變會再喚醒協調迴圈",
                        inner.config.first_read_wait.as_secs_f32(),
                        if active { "勿打擾中" } else { "不暫停" }
                    ),
                );
                return active;
            }
            return self.judged(&mut st);
        }
        st.wait_warned = false;
        self.judged(&mut st)
    }
}

impl Drop for DndMonitor {
    fn drop(&mut self) {
        lock(&self.inner.state).shutdown = true;
        self.inner.cv.notify_all();
    }
}

/// 輪詢執行緒本體（見模組文件契約 3）。
fn run_poller(inner: &Inner, parts: PollerParts) {
    let PollerParts {
        sources,
        takes_over,
        notify,
    } = parts;
    let mut sources = sources();
    let mut tracker = DndTracker::default();
    let mut next_poll: Option<Instant> = None;
    loop {
        let requested = {
            let mut st = lock(&inner.state);
            loop {
                if st.shutdown {
                    return;
                }
                if st.read_requested {
                    st.read_requested = false;
                    break true;
                }
                if !st.enabled {
                    st = inner.cv.wait(st).unwrap_or_else(|e| e.into_inner());
                    continue;
                }
                let now = Instant::now();
                match next_poll {
                    Some(at) if at > now => {
                        st = inner
                            .cv
                            .wait_timeout(st, at - now)
                            .map(|(g, _)| g)
                            .unwrap_or_else(|e| e.into_inner().0);
                    }
                    _ => break false,
                }
            }
        };
        if !requested && !takes_over() {
            let mut st = lock(&inner.state);
            if st.read_requested || st.shutdown {
                continue;
            }
            st.enabled = false;
            st.active = false;
            st.last_read_at = None;
            drop(st);
            tracker.reset();
            next_poll = None;
            (inner.log)(
                Level::Info,
                "勿打擾：主題為「不接管」，停止輪詢（下次需要重畫時再讀）",
            );
            continue;
        }
        let focus = sources.read_focus();
        let wnf = sources.read_wnf();
        let observation = tracker.observe(&focus, &wnf);
        for (level, line) in &observation.lines {
            (inner.log)(*level, line);
        }
        let recovered = {
            let mut st = lock(&inner.state);
            st.active = observation.combined;
            st.completed += 1;
            st.last_read_at = Some((inner.clock)());
            std::mem::replace(&mut st.stale_warned, false)
        };
        if recovered {
            (inner.log)(
                Level::Info,
                &format!(
                    "勿打擾：讀值恢復更新（{}）",
                    if observation.combined {
                        "勿打擾中"
                    } else {
                        "不暫停"
                    }
                ),
            );
        }
        inner.cv.notify_all();
        if observation.changed {
            notify();
        }
        next_poll = Some(Instant::now() + inner.config.poll_interval);
    }
}

// ---------------------------------------------------------------------------------------------
// 正式接線
// ---------------------------------------------------------------------------------------------

/// 正式接線的兩個 hook（修正輪 1，審查 low3：抽出來才測得到）：`takes_over`＝主題不是「不接管」，
/// `notify`＝送 `Wake::DoNotDisturbChanged`。`read_theme` 讀設定中的主題（只短暫上鎖），`send` 投遞
/// 喚醒（不得等待）。
#[allow(clippy::type_complexity)]
pub fn host_hooks(
    read_theme: impl Fn() -> crate::settings::WallpaperTheme + Send + 'static,
    send: impl Fn(crate::wallpaper_coordinator::Wake) + Send + 'static,
) -> (Box<dyn Fn() -> bool + Send>, Box<dyn Fn() + Send>) {
    (
        Box::new(move || read_theme() != crate::settings::WallpaperTheme::None),
        Box::new(move || send(crate::wallpaper_coordinator::Wake::DoNotDisturbChanged)),
    )
}

/// 正式宿主的勿打擾監看：主題由 `AppState` 的設定判斷、改變時送 `Wake::DoNotDisturbChanged`、記錄
/// 寫進協調迴圈的 target。`self-test-ipc` 建置另可注入讀值與輪詢間隔（`injection` 子模組）。
pub fn monitor_for_app(app: &tauri::AppHandle) -> DndMonitor {
    use tauri::Manager;

    let for_theme = app.clone();
    let for_notify = app.clone();
    let log: LogSink = Arc::new(|level, msg: &str| {
        log::log!(target: crate::wallpaper_coordinator::LOG_TARGET, level, "{msg}");
    });
    #[allow(unused_mut)]
    let mut config = DndConfig::default();
    #[cfg(feature = "self-test-ipc")]
    if let Some(interval) = injection::poll_interval_override() {
        config.poll_interval = interval;
        config.stale_after = interval * 3;
    }
    #[cfg(feature = "self-test-ipc")]
    let sources: SourcesFactory = {
        let log = Arc::clone(&log);
        Box::new(move || {
            Box::new(injection::InjectingSources::new(SystemSources::new(), log))
                as Box<dyn DndSources>
        })
    };
    #[cfg(not(feature = "self-test-ipc"))]
    let sources: SourcesFactory =
        Box::new(|| Box::new(SystemSources::new()) as Box<dyn DndSources>);
    let (takes_over, notify) = host_hooks(
        move || {
            let state = for_theme.state::<crate::widgets::AppState>();
            let theme = lock(&state.settings).wallpaper_theme;
            theme
        },
        move |wake| crate::wallpaper_coordinator::notify_app(&for_notify, wake),
    );
    DndMonitor::new(
        config,
        DndHooks {
            sources,
            takes_over,
            notify,
            log,
            clock: system_clock(),
        },
    )
}

// task 4.7c：self-test 建置注入勿打擾讀值（正式建置不含）。
#[cfg(feature = "self-test-ipc")]
mod injection;

#[cfg(test)]
mod tests;
