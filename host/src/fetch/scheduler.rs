//! 宿主內建的抓取排程（data-layer-rust tasks.md 5.2、5.3；design.md D5、D10；spec「定時抓取」「啟動與恢復時補抓」
//! 「抓取間隔下限」「等待網路」「抓取開關與隔離環境」）。
//!
//! 判斷規則全在純函式 [`super::sched`]；這裡是把它接上時鐘、設定、紀錄檔、一輪抓取與停止介面的迴圈。
//!
//! # 執行緒：專用的 `std::thread`
//!
//! 不用 `tauri::async_runtime::spawn`：排程要呼叫的 [`super::net::wait_for_network_unless_stopped`] 與
//! 檔案讀寫都是**阻塞**呼叫（最長等網路 300 秒以上），丟進 Tauri 的 async worker 會佔住它；一輪抓取本身需要
//! 的 tokio runtime 由 [`super::exec::execute_round`] 每輪自建（`ReqwestFetch` 的連線池與 runtime 綁在一起，
//! 每輪一組最單純）。專用執行緒也讓「排程 panic 不影響宿主」更直接：迴圈每次醒來的處理都包
//! `catch_unwind`（[`run_loop`]），一輪抓取另有自己的一層（[`tick`]），panic 只記錄、算該輪失敗。
//! 與宿主既有的資料輪詢執行緒（`main.rs`）同一種做法。
//!
//! # 迴圈（[`run_loop`]）
//!
//! 每次醒來（約每 60 秒；有待啟動的一輪時睡到它的開始時刻）：
//!
//! 1. 讀目前設定的 `data_dir` 與 `data_fetch`（**即時生效**，不需要重新啟動）；以 [`decide_gate`] 判定
//!    啟用／關閉／隔離環境，狀態改變時記一行（隔離環境記「隔離環境，不抓取（LOCALAPPDATA=…，系統登記=…）」）。
//!    不是啟用就只記醒來時間（[`super::sched::Scheduler::idle_wake`]）、不碰網路、不寫任何檔案；
//! 2. 讀排程紀錄（讀不到或壞掉＝沒有）、處理時鐘倒退（改寫紀錄）、看資料檔在不在；
//! 3. [`super::sched::Scheduler::step`] 決定要不要開始一輪；要的話執行（[`run_scheduled_round`]：開始行 → 建目錄 →
//!    等網路 → [`super::exec::execute_round`]），結束後更新排程紀錄。
//!
//! 「一輪成功」＝寫檔成功**且**有任何來源成功（[`super::round::RoundResult::any_source_ok`]）；寫檔失敗、整輪
//! panic、等同全部來源失敗都記為失敗，走加倍退避重試（[`super::sched`] 模組文件）。被停止中止的一輪不更新紀錄。
//!
//! # 活動狀態（[`SchedEnv::activity`]）
//!
//! 迴圈在三處回報 [`FetchActivity`]（fetch-progress-and-manual-refresh design.md D2；語意見 [`super::activity`]）：
//!
//! - 每次醒來開頭：「沒有抓取進行中」時應有的值（`enabled` 依這次的閘門、`last` 依迴圈記住的結果）與
//!   `LoopState::sent`（上次成功送出的值）不同就補送——涵蓋閘門改變、第一次醒來，以及上次醒來送出 `running: true`
//!   之後 panic、沒送到結束的情況（否則圖示會一直轉）；
//! - 決定開始一輪之前：`running: true`；
//! - 一輪結束後（跑完、被中止、panic 被接住都算），排程紀錄寫完之後：`running: false`，`enabled` 重新判定一次、
//!   `last` 依結局更新（被中止的一輪不改）。
//!
//! `last` 在第一次醒來時由排程紀錄初始化（沒有紀錄＝`none`）。正式環境寫 managed state 並廣播 `fetch-activity`，
//! 結束時先同步輪詢一次資料再廣播（[`super::activity::publish`]；只保證呼叫順序、不保證頁面的送達順序，見該模組文件）。
//! managed state 由 [`register_activity`] 在建立小工具之前登記。
//!
//! # 手動更新（fetch-progress-and-manual-refresh design.md D3）
//!
//! 設定視窗的「立即更新資料」（指令在 [`super::manual`]）經 [`SchedulerHandle::request_manual`] 設 [`StopSignal`] 的手動旗標
//! 並喚醒這條執行緒，不另開會寫資料檔的執行緒。[`tick`] 在閘門啟用時取走旗標，以 [`Trigger::Manual`] 直接開始一輪
//! （不經時點與 60 分鐘下限），與排程的一輪走同一個函式（`run_round_and_report`）；閘門未啟用時取走並丟棄。
//! 一輪結束時（送出 `running: false` 之前）再清一次旗標，進行中送來的要求由這一輪吸收。
//!
//! # 停止介面（[`SchedulerHandle`]）
//!
//! - [`SchedulerHandle::request_stop`]：只設旗標、不等（非阻塞）；
//! - [`SchedulerHandle::stop`]`(timeout)`：設旗標並等執行緒結束，最多等 `timeout`，超時就放棄等待
//!   （[`StopOutcome::TimedOut`]；執行緒會在下一個檢查點自行結束，宿主不必等它）。
//!
//! 進行中的一輪在**下一個來源邊界**停下（[`super::round::RoundHooks::should_stop`]），**不寫半份檔**（寫檔是
//! 一輪最後的原子動作）；等網路與睡眠都分成小片檢查停止旗標。已送出的單一 HTTP 請求不中斷（各請求有自己的
//! 逾時），所以 `timeout` 要大於單一請求的逾時才等得到「在邊界停下」。`data_fetch` 被改成 off 或環境判為
//! 隔離時，進行中的一輪同樣在下一個來源邊界停下（spec：`off` 時 MUST NOT 再發抓取請求）。
//!
//! 呼叫點：系統匣「結束」（[`stop_app`]，`tray.rs`）與日後 installer-auto-update 的「因更新結束」路徑；
//! `RunEvent::Exit`（`main.rs`）另外呼叫非阻塞的 [`request_stop_app`] 當總保險。

use std::io;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration as StdDuration;

use log::Level;
use tauri::{AppHandle, Manager};
use time::OffsetDateTime;

use crate::desktop::{self, LocalAppDataMismatch};
use crate::settings::DataFetch;
use crate::widgets::AppState;

use super::activity::{self, FetchActivity, FetchActivityStore, LastRound};
use super::clock::Clock;
use super::exec::{self, ExecStatus};
use super::http::{Fetch, ReqwestFetch};
use super::net;
use super::outcome::panic_message;
use super::output::OUTPUT_FILE;
use super::report::{self, Line};
use super::sched::{FetchState, Rng, Scheduler, Step, SystemRng, Trigger, WAKE_INTERVAL};
use super::state;

/// 抓取目前能不能跑（由 `data_fetch` 與隔離偵測決定）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    /// 抓取啟用。
    Enabled,
    /// `data_fetch="off"`。
    Off,
    /// `data_fetch="auto"` 且環境變數 `LOCALAPPDATA` 與系統登記的不同（驗收腳本以暫存資料夾隔離宿主）。
    Isolated(LocalAppDataMismatch),
}

/// `data_fetch` ＋ 隔離偵測 → 抓取開關（design.md D10）：`on` 一律啟用（含隔離環境——使用者明確要求）、`off`
/// 一律關閉、`auto` 在隔離環境關閉、否則啟用。
pub fn decide_gate(mode: DataFetch, isolation: Option<&LocalAppDataMismatch>) -> Gate {
    match mode {
        DataFetch::On => Gate::Enabled,
        DataFetch::Off => Gate::Off,
        DataFetch::Auto => match isolation {
            Some(m) => Gate::Isolated(m.clone()),
            None => Gate::Enabled,
        },
    }
}

/// 排程執行緒的喚醒訊號：停止旗標＋「手動要求」旗標（設定視窗的「立即更新資料」，fetch-progress-and-manual-refresh
/// design.md D3），共用一個 condvar 的可打斷睡眠。
///
/// 手動旗標只由迴圈取走（[`StopSignal::take_manual`]）：閘門啟用時開始一輪 [`Trigger::Manual`]，未啟用時丟棄；一輪結束時
/// 再清一次（進行中送來的要求由這一輪吸收）。旗標成立期間 [`StopSignal::wait`] 不睡，所以迴圈每條路都要取走它，否則會空轉。
#[derive(Debug, Default)]
pub struct StopSignal {
    stopped: AtomicBool,
    manual: AtomicBool,
    lock: Mutex<()>,
    wake: Condvar,
}

impl StopSignal {
    pub fn new() -> Self {
        StopSignal::default()
    }

    /// 要求停止（冪等），並喚醒正在 [`StopSignal::wait`] 的執行緒。
    pub fn request(&self) {
        let _guard = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        self.stopped.store(true, Ordering::SeqCst);
        self.wake.notify_all();
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    /// 要求立即抓一輪（冪等：連送多次只算一次），並喚醒正在 [`StopSignal::wait`] 的執行緒。在鎖內設旗標，
    /// 與 `wait` 的條件檢查互斥，不會漏掉喚醒。
    pub fn request_manual(&self) {
        let _guard = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        self.manual.store(true, Ordering::SeqCst);
        self.wake.notify_all();
    }

    /// 取走手動要求：回傳取走前是否成立，之後旗標為假。
    pub fn take_manual(&self) -> bool {
        self.manual.swap(false, Ordering::SeqCst)
    }

    fn manual_pending(&self) -> bool {
        self.manual.load(Ordering::SeqCst)
    }

    /// 睡 `timeout`，被停止要求或手動要求喚醒就提早返回。回傳值**只表示是否已要求停止**；手動要求留給迴圈下一次醒來取走。
    pub fn wait(&self, timeout: StdDuration) -> bool {
        let guard = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = self
            .wake
            .wait_timeout_while(guard, timeout, |_| {
                !self.is_stopped() && !self.manual_pending()
            })
            .unwrap_or_else(PoisonError::into_inner);
        self.is_stopped()
    }
}

/// 一輪（含開始前的準備）的結局，給迴圈更新排程紀錄用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundEnd {
    /// 跑完了：`ok`＝寫檔成功且有任何來源成功。
    Done { ok: bool },
    /// 被停止要求（或抓取被關掉）中止：不算失敗、不更新排程紀錄。
    Aborted,
}

/// 迴圈需要的外部世界（注入點：測試給假的時鐘、設定與一輪）。
pub trait SchedEnv {
    /// 現在（UTC 瞬間，系統時鐘）。
    fn now(&self) -> OffsetDateTime;
    /// 目前設定的資料目錄與抓取開關。
    fn settings(&self) -> (PathBuf, DataFetch);
    /// 隔離偵測的結果（行程生命週期內不變）。
    fn isolation(&self) -> Option<LocalAppDataMismatch>;
    /// `dir` 下的 `tw_events.json` 是否存在。
    fn data_file_exists(&self, dir: &Path) -> bool {
        dir.join(OUTPUT_FILE).is_file()
    }
    fn load_state(&self) -> Option<FetchState>;
    fn save_state(&self, state: &FetchState) -> io::Result<()>;
    /// 執行一輪（含開始行、建目錄、等網路、抓、寫檔），回傳結局。
    fn run_round(&self, trigger: Trigger, dir: &Path, stop: &StopSignal) -> RoundEnd;
    fn log(&self, level: Level, text: &str);
    /// 回報抓取活動狀態（見模組文件「活動狀態」）。預設什麼都不做：只關心排程判斷的假環境不必實作。
    fn activity(&self, _activity: FetchActivity) {}
}

/// 一次醒來的結果。
enum Tick {
    /// 要求停止，離開迴圈。
    Stop,
    /// 睡這麼久再醒來。
    Sleep(StdDuration),
}

/// 迴圈在醒來之間記住的東西。
#[derive(Default)]
struct LoopState {
    sched: Scheduler,
    last_gate: Option<Gate>,
    /// 最後一筆排程紀錄（記憶體備援）。紀錄檔持續寫不進去時，60 分鐘下限與失敗退避靠它在同一個行程內
    /// 繼續有效（審查 I1）；有值時**優先於**磁碟上的紀錄（只有本行程會寫這個檔，磁碟紀錄只在行程啟動、記憶體還是空的時候有意義；
    /// 否則時鐘倒退又寫不進去時，磁碟上那筆「未來」的紀錄會讓每分鐘都重新修正、重新警告，審查 N1）。
    memory: Option<FetchState>,
    /// 上一次寫排程紀錄是否失敗（警告只在狀態改變時記一次，避免每輪洗版）。
    save_failing: bool,
    /// 行程內最近一輪的結果；`None`＝還沒初始化（第一次醒來時由排程紀錄初始化）。
    last: Option<LastRound>,
    /// 上一次**成功送出**的活動狀態（`SchedEnv::activity` 正常返回才記）。每次醒來開頭與「應有值」比對、不同就補送，
    /// 涵蓋閘門改變，以及送出 `running: true` 之後這次醒來 panic、沒送到 `running: false` 的情況。
    sent: Option<FetchActivity>,
}

/// 沒有抓取進行中時應有的活動狀態。迴圈每次醒來開頭（[`tick`]）與 managed state 的初始值（[`register_activity`]）
/// 共用這一個算法，兩者才不會分歧。
fn idle_activity(gate: &Gate, last: LastRound) -> FetchActivity {
    FetchActivity {
        running: false,
        enabled: *gate == Gate::Enabled,
        last,
    }
}

/// 行程啟動時的活動狀態：`last` 由排程紀錄初始化（[`LastRound::from_record`]），其餘同 [`idle_activity`]。
fn initial_activity(
    mode: DataFetch,
    isolation: Option<&LocalAppDataMismatch>,
    record: Option<&FetchState>,
) -> FetchActivity {
    idle_activity(
        &decide_gate(mode, isolation),
        LastRound::from_record(record),
    )
}

/// 送出活動狀態並記下（送出途中 panic 就不記，下一次醒來補送）。
fn report_activity<E: SchedEnv>(env: &E, ls: &mut LoopState, activity: FetchActivity) {
    env.activity(activity);
    ls.sent = Some(activity);
}

/// 寫排程紀錄；成功與失敗的記錄都只在狀態改變時送出一次。
fn save_record<E: SchedEnv>(env: &E, state: &mut LoopState, record: &FetchState) {
    match env.save_state(record) {
        Ok(()) => {
            if state.save_failing {
                env.log(Level::Info, "抓取排程：排程紀錄恢復可寫入");
                state.save_failing = false;
            }
        }
        Err(e) => {
            if !state.save_failing {
                env.log(
                    Level::Warn,
                    &format!(
                        "抓取排程：寫入排程紀錄失敗（{e}）；本行程改用記憶體中的紀錄維持 60 分鐘下限與退避，重啟後會遺失"
                    ),
                );
                state.save_failing = true;
            }
        }
    }
}

/// 排程迴圈：每次醒來呼叫 [`tick`]，睡眠交給 `sleep`（回傳是否要停止）。正式的 `sleep` 是
/// [`StopSignal::wait`]，測試給推進假時鐘的函式。每次醒來的處理都包 `catch_unwind`：panic 只記錄、下一次
/// 醒來照常，不會讓迴圈死掉。
pub fn run_loop<E: SchedEnv>(
    env: &E,
    stop: &StopSignal,
    rng: &mut dyn Rng,
    sleep: &mut dyn FnMut(StdDuration) -> bool,
) {
    let mut state = LoopState::default();
    loop {
        if stop.is_stopped() {
            return;
        }
        let tick_result = catch_unwind(AssertUnwindSafe(|| tick(env, stop, rng, &mut state)));
        let nap = match tick_result {
            Ok(Tick::Stop) => return,
            Ok(Tick::Sleep(d)) => d,
            Err(payload) => {
                env.log(
                    Level::Error,
                    &format!(
                        "抓取排程：這次醒來的處理 panic（{}），下一次醒來照常",
                        panic_message(payload.as_ref())
                    ),
                );
                // 取走手動要求之前就 panic 時旗標還在，`wait` 會立刻返回；panic 若是決定性的就變成空轉。丟掉這次要求，
                // 照常睡一個醒來間隔。
                stop.take_manual();
                WAKE_INTERVAL.unsigned_abs()
            }
        };
        if sleep(nap) {
            return;
        }
    }
}

/// 一次醒來（見模組文件「迴圈」）。
fn tick<E: SchedEnv>(env: &E, stop: &StopSignal, rng: &mut dyn Rng, ls: &mut LoopState) -> Tick {
    let now = env.now();
    let (dir, mode) = env.settings();
    let gate = decide_gate(mode, env.isolation().as_ref());
    if ls.last_gate.as_ref() != Some(&gate) {
        log_gate(env, &gate, mode, &dir);
        ls.last_gate = Some(gate.clone());
    }
    // 活動狀態：沒有抓取進行中時應有的值，與上次送出的不同就補送（閘門改變、上次醒來 panic 沒送到結束）。
    // `last` 第一次醒來時由排程紀錄初始化——閘門關閉時也要有值，所以不等到下面讀紀錄的地方。
    // 與 `initial_activity` 同一個來源（`LastRound::from_record`）。
    let last = *ls
        .last
        .get_or_insert_with(|| LastRound::from_record(env.load_state().as_ref()));
    let idle = idle_activity(&gate, last);
    if ls.sent != Some(idle) {
        report_activity(env, ls, idle);
    }
    if gate != Gate::Enabled {
        // 手動要求在閘門未啟用時丟棄（指令那一端已擋；這裡收掉「指令檢查之後才被關掉」的情況，也避免旗標留著讓
        // `wait` 不睡）。
        if stop.take_manual() {
            env.log(
                Level::Info,
                "抓取排程：收到立即更新要求，但抓取未啟用，不抓取",
            );
        }
        ls.sched.idle_wake(now);
        return Tick::Sleep(WAKE_INTERVAL.unsigned_abs());
    }

    // 記憶體紀錄優先（紀錄檔寫不進去時它才是真相）；記憶體還是空的（行程剛啟動）才讀磁碟。
    let mut record = ls.memory.or_else(|| env.load_state());
    if let Some(s) = record {
        let (clamped, changed) = s.clamped_to(now);
        if changed {
            env.log(
                Level::Warn,
                "抓取排程：偵測到系統時鐘倒退，排程紀錄裡晚於現在的時間改為現在",
            );
            save_record(env, ls, &clamped);
            ls.memory = Some(clamped);
            record = Some(clamped);
        }
    }
    let exists = env.data_file_exists(&dir);
    ls.sched.set_data_dir(&dir);

    // 手動要求（設定視窗「立即更新資料」）：直接開始，不經 `Scheduler::step` 的時點與 60 分鐘下限（design.md D3）。
    // 這一輪照常寫排程紀錄，之後的時點與補抓以它的開始時間計算下限；失敗同樣走重試退避。`step` 這次沒有呼叫，
    // 上次醒來時間與待啟動的一輪由一輪結束後的 `round_finished`／`idle_wake` 一併重設。
    let trigger = if stop.take_manual() {
        Trigger::Manual
    } else {
        match ls.sched.step(now, record.as_ref(), exists, rng) {
            Step::Sleep(d) => return Tick::Sleep(d.unsigned_abs()),
            Step::Start(trigger) => trigger,
        }
    };
    run_round_and_report(env, stop, ls, trigger, now, &dir, record)
}

/// 執行一輪並回報開始與結束（排程與手動共用同一條路：同一個執行緒、同一條寫檔路徑、同一組活動狀態回報）。
/// `started`＝這次醒來的時間（＝排程紀錄的開始時間），`record`＝開始前的排程紀錄。
fn run_round_and_report<E: SchedEnv>(
    env: &E,
    stop: &StopSignal,
    ls: &mut LoopState,
    trigger: Trigger,
    started: OffsetDateTime,
    dir: &Path,
    record: Option<FetchState>,
) -> Tick {
    // 開始前的最近一輪結果；`tick` 開頭已初始化，`Never` 只是型別上的退路。
    let last = ls.last.unwrap_or(LastRound::Never);
    report_activity(
        env,
        ls,
        FetchActivity {
            running: true,
            enabled: true,
            last,
        },
    );
    // 一輪抓取另包一層：panic 算該輪失敗（記進排程紀錄走退避），否則決定性的 panic 會讓每次醒來都重來。
    let end = catch_unwind(AssertUnwindSafe(|| env.run_round(trigger, dir, stop))).unwrap_or_else(
        |payload| {
            env.log(
                Level::Error,
                &format!(
                    "抓取排程：一輪抓取 panic（{}），記為失敗",
                    panic_message(payload.as_ref())
                ),
            );
            RoundEnd::Done { ok: false }
        },
    );
    // 抓取進行中送來的手動要求由這一輪吸收，不緊接著再抓一輪（design.md D3）。必須在下面送出 `running: false`
    // **之前**清：指令以活動狀態的 `running` 擋重複要求，清旗標到送出結束通知之間送來的要求仍會看到「進行中」而不設
    // 旗標；反過來先送結束通知再清，看到「沒有進行中」而設下的旗標會被這裡清掉，指令回了 `started` 卻沒有抓。
    stop.take_manual();
    let finished = env.now();
    let next = match end {
        RoundEnd::Aborted => {
            // 中止的一輪沒有「跑完」：不設首抓抑制（審查 N2），也不更新紀錄、不改 `last`。
            ls.sched.idle_wake(finished);
            if stop.is_stopped() {
                Tick::Stop
            } else {
                // 抓取在輪中被關掉：下一次醒來照常（會看到閘門已關）。
                Tick::Sleep(StdDuration::ZERO)
            }
        }
        RoundEnd::Done { ok } => {
            ls.last = Some(if ok {
                LastRound::Succeeded
            } else {
                LastRound::Failed
            });
            // 跑完後資料檔還不存在＝寫檔失敗：之後不首抓、走重試退避（熱迴圈防護，見 `sched` 模組文件）。
            ls.sched.round_finished(finished, env.data_file_exists(dir));
            let new_state = FetchState::after_round(record.as_ref(), started, finished, ok);
            ls.memory = Some(new_state);
            save_record(env, ls, &new_state);
            Tick::Sleep(StdDuration::ZERO)
        }
    };
    // 結束通知放在寫完排程紀錄之後：設定視窗收到「沒有抓取進行中」就重讀紀錄，要讀到這一輪。閘門重新判定一次：
    // 輪中被改成 off 而中止時，要一併告訴頁面「抓取未啟用」（spec「被中止的一輪」）。
    let (_, mode_after) = env.settings();
    let enabled = decide_gate(mode_after, env.isolation().as_ref()) == Gate::Enabled;
    let last = ls.last.unwrap_or(last);
    report_activity(
        env,
        ls,
        FetchActivity {
            running: false,
            enabled,
            last,
        },
    );
    next
}

fn log_gate<E: SchedEnv>(env: &E, gate: &Gate, mode: DataFetch, dir: &Path) {
    match gate {
        Gate::Enabled => env.log(
            Level::Info,
            &format!(
                "抓取排程：啟用（data_fetch={}，資料目錄={}）",
                mode.as_str(),
                dir.display()
            ),
        ),
        Gate::Off => env.log(
            Level::Info,
            "抓取排程：data_fetch=off，不抓取（只讀取資料目錄中已有的檔案）",
        ),
        Gate::Isolated(m) => env.log(
            Level::Info,
            &format!(
                "隔離環境，不抓取（LOCALAPPDATA={}，系統登記={}）；要在此環境抓取請結束宿主、把設定檔的 data_fetch 改成 on 後重新啟動",
                m.env_value, m.registered
            ),
        ),
    }
}

/// 執行一輪（見模組文件「迴圈」第 3 點）。全部外部依賴由參數注入：`make_fetch` 建 HTTP client、`wait_network`
/// 等網路（收到 `should_stop`，回傳是否就緒；因停止而回傳時值不重要，這裡會再檢查一次）、`now` 取時鐘。
/// 記錄行依 B-FLOW-18 的順序交給 `sink`：開始行（含觸發原因）→ 建目錄／等網路 → 各來源 → 已輸出 → 完成摘要。
pub fn run_scheduled_round<F: Fetch, E: std::fmt::Display>(
    trigger: Trigger,
    dir: &Path,
    make_fetch: impl FnOnce() -> Result<F, E>,
    wait_network: impl FnOnce(&(dyn Fn() -> bool + Sync)) -> bool,
    now: impl FnOnce() -> Clock,
    should_stop: &(dyn Fn() -> bool + Sync),
    sink: &mut (dyn FnMut(Line) + Send),
) -> RoundEnd {
    sink(report::start_line(trigger.as_str(), dir));
    if let Err(e) = std::fs::create_dir_all(dir) {
        sink((
            Level::Error,
            format!("輸出目錄 {} 不存在且建立失敗：{e}", dir.display()),
        ));
        return RoundEnd::Done { ok: false };
    }
    let fetch = match make_fetch() {
        Ok(f) => f,
        Err(e) => {
            sink((Level::Error, format!("建立 HTTP client 失敗：{e}")));
            return RoundEnd::Done { ok: false };
        }
    };
    let net_ok = wait_network(should_stop);
    if should_stop() {
        sink((
            Level::Info,
            "收到停止要求或抓取已停用，本輪在開始抓取之前中止".to_string(),
        ));
        return RoundEnd::Aborted;
    }
    let executed = exec::execute_round(&fetch, &now(), dir, !net_ok, should_stop, sink);
    match executed.status {
        ExecStatus::Aborted => RoundEnd::Aborted,
        ExecStatus::Written => RoundEnd::Done {
            ok: executed.any_source_ok,
        },
        ExecStatus::WriteFailed | ExecStatus::RoundPanicked | ExecStatus::SetupFailed => {
            RoundEnd::Done { ok: false }
        }
    }
}

// ───────────── 執行緒與停止介面 ─────────────

/// 系統匣「結束」等排程停下的上限。停止點在**來源邊界**而不是請求邊界：行情 12 檔每檔至少間隔 2 秒，
/// 一個來源可以跑 20 秒以上，所以結束時若正在抓行情，通常會 `TimedOut`——這是預期的：超時就放棄等待、照常結束，
/// 原子寫檔保證不會留下半份檔（殘留的暫存檔由 `STALE_TMP_AGE` 清除）；`request_stop` 在 `quit` 一開始就送出，
/// 與還原桌布重疊，實際增加的等待通常接近 0。
pub const QUIT_STOP_TIMEOUT: StdDuration = StdDuration::from_secs(5);

/// [`SchedulerHandle::stop`] 的結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopOutcome {
    /// 排程執行緒已結束（進行中的一輪已在來源邊界停下）。
    Stopped,
    /// 等到 `timeout` 執行緒仍在跑（例如卡在一個慢請求）：已放棄等待，它會在下一個檢查點自行結束。
    TimedOut,
}

/// 排程執行緒的控制把手（管理在 Tauri managed state，見 [`spawn_for_app`]）。
pub struct SchedulerHandle {
    stop: Arc<StopSignal>,
    finished: Arc<(Mutex<bool>, Condvar)>,
    join: Mutex<Option<JoinHandle<()>>>,
}

impl SchedulerHandle {
    /// 只設停止旗標、不等（非阻塞，主執行緒也可安全呼叫）。
    pub fn request_stop(&self) {
        self.stop.request();
    }

    /// 要求立即抓一輪（非阻塞：只設旗標並喚醒排程執行緒；見模組文件「手動更新」）。
    pub fn request_manual(&self) {
        self.stop.request_manual();
    }

    /// 設停止旗標並等執行緒結束，最多等 `timeout`（見模組文件「停止介面」）。可重複呼叫。
    pub fn stop(&self, timeout: StdDuration) -> StopOutcome {
        self.stop.request();
        let (done, cv) = &*self.finished;
        let guard = done.lock().unwrap_or_else(PoisonError::into_inner);
        let (guard, _) = cv
            .wait_timeout_while(guard, timeout, |finished| !*finished)
            .unwrap_or_else(PoisonError::into_inner);
        let finished = *guard;
        drop(guard);
        if !finished {
            return StopOutcome::TimedOut;
        }
        if let Some(join) = self
            .join
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            let _ = join.join();
        }
        StopOutcome::Stopped
    }

    /// 執行緒是否已結束（測試用）。
    #[cfg(test)]
    pub fn is_finished(&self) -> bool {
        *self
            .finished
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// 在專用執行緒啟動排程迴圈（睡眠用 [`StopSignal::wait`]，可被停止介面打斷）。
pub fn spawn<E: SchedEnv + Send + 'static>(
    env: E,
    mut rng: Box<dyn Rng + Send>,
) -> io::Result<SchedulerHandle> {
    let stop = Arc::new(StopSignal::new());
    let finished = Arc::new((Mutex::new(false), Condvar::new()));
    let thread_stop = Arc::clone(&stop);
    let thread_finished = Arc::clone(&finished);
    let join = thread::Builder::new()
        .name("fc-fetch-scheduler".to_owned())
        .spawn(move || {
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                run_loop(&env, &thread_stop, rng.as_mut(), &mut |d| {
                    thread_stop.wait(d)
                });
            }));
            if let Err(payload) = outcome {
                env.log(
                    Level::Error,
                    &format!(
                        "抓取排程執行緒 panic 後結束：{}",
                        panic_message(payload.as_ref())
                    ),
                );
            }
            env.log(Level::Info, "抓取排程已停止");
            let (done, cv) = &*thread_finished;
            *done.lock().unwrap_or_else(PoisonError::into_inner) = true;
            cv.notify_all();
        })?;
    Ok(SchedulerHandle {
        stop,
        finished,
        join: Mutex::new(Some(join)),
    })
}

// ───────────── 宿主接線 ─────────────

/// 正式環境：系統時鐘、`AppState` 裡的設定、`fetch-state.json`、真實網路。
struct HostEnv {
    app: AppHandle,
    isolation: Option<LocalAppDataMismatch>,
    state_path: PathBuf,
}

impl HostEnv {
    fn gate_enabled(&self) -> bool {
        let (_, mode) = self.settings();
        decide_gate(mode, self.isolation.as_ref()) == Gate::Enabled
    }
}

impl SchedEnv for HostEnv {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }

    fn settings(&self) -> (PathBuf, DataFetch) {
        let state = self.app.state::<AppState>();
        let settings = state
            .settings
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        (settings.data_dir.clone(), settings.data_fetch)
    }

    fn isolation(&self) -> Option<LocalAppDataMismatch> {
        self.isolation.clone()
    }

    fn load_state(&self) -> Option<FetchState> {
        state::load(&self.state_path)
    }

    fn save_state(&self, record: &FetchState) -> io::Result<()> {
        state::save(&self.state_path, record)
    }

    fn run_round(&self, trigger: Trigger, dir: &Path, stop: &StopSignal) -> RoundEnd {
        // 停止要求、或抓取在輪中被關掉（data_fetch 改成 off）都在下一個來源邊界停下。
        let should_stop = || stop.is_stopped() || !self.gate_enabled();
        run_scheduled_round(
            trigger,
            dir,
            ReqwestFetch::new,
            |stop_now| net::wait_for_network_unless_stopped(stop_now),
            Clock::now,
            &should_stop,
            &mut |(level, text): Line| report::write_log(level, &text),
        )
    }

    fn log(&self, level: Level, text: &str) {
        report::write_log(level, text);
    }

    fn activity(&self, next: FetchActivity) {
        activity::publish_to_app(&self.app, next);
    }
}

/// 把抓取活動狀態（[`FetchActivityStore`]）登記進 Tauri managed state，初始值＝[`initial_activity`]（沒有抓取進行中、
/// 閘門依目前設定、`last` 依排程紀錄，讀一個小檔）。`build_ui` 在建立小工具視窗**之前**呼叫，讓小工具一建好就查得到
/// （否則 `get_fetch_activity` 回「尚未就緒」，頁面要等第一筆事件）；[`spawn_for_app`] 啟動執行緒前再呼叫一次確認。
/// 已登記就不動（不覆蓋排程執行緒寫入的值）。
///
/// 只登記一個 `Mutex`，不啟動執行緒、不寫檔、不呼叫桌布 API，本身不是「背景元件」；仍比照背景元件先取 `StartGate`
/// 許可（正在因更新結束就不登記，查詢回「尚未就緒」），讓收尾期間不新增任何 managed state。
pub fn register_activity(app: &AppHandle) {
    let Some(_start_permit) = crate::updater::try_begin_start("抓取活動狀態") else {
        return;
    };
    if app.try_state::<FetchActivityStore>().is_some() {
        return;
    }
    let mode = app
        .state::<AppState>()
        .settings
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .data_fetch;
    let initial = initial_activity(
        mode,
        desktop::detect_isolated_local_app_data().as_ref(),
        state::load(&state::default_state_path()).as_ref(),
    );
    app.manage(FetchActivityStore::new(initial));
}

/// 啟動抓取排程並把控制把手放進 Tauri managed state（`setup` 完成後呼叫一次）。失敗只記錄、不影響宿主。
pub fn spawn_for_app(app: &AppHandle) {
    // installer-auto-update 4.x：正在因更新結束就不啟動；許可持有到把手登記完才放掉
    // （`updater::exit::StartGate`，收尾步驟 4 據此分辨「啟動中」與「沒有排程」）。
    let Some(_start_permit) = crate::updater::try_begin_start("抓取排程") else {
        return;
    };
    let env = HostEnv {
        app: app.clone(),
        isolation: desktop::detect_isolated_local_app_data(),
        state_path: state::default_state_path(),
    };
    // 活動狀態必須在執行緒啟動之前就在（執行緒第一次醒來就會寫入）。正常已由 `build_ui` 在建立小工具前登記，這裡只是
    // 確認（已登記就不動）。
    register_activity(app);
    match spawn(env, Box::new(SystemRng)) {
        Ok(handle) => {
            log::info!(target: report::LOG_TARGET, "抓取排程已啟動");
            app.manage(handle);
        }
        Err(e) => log::error!(target: report::LOG_TARGET, "抓取排程無法啟動：{e}"),
    }
}

/// 要求排程停止並等它（最多 `timeout`）。沒有排程（沒啟動、啟動失敗）回 `None`。系統匣「結束」與日後
/// 「因更新結束」用；**會阻塞**，不要在主執行緒呼叫（主執行緒用 [`request_stop_app`]）。
pub fn stop_app(app: &AppHandle, timeout: StdDuration) -> Option<StopOutcome> {
    let handle = app.try_state::<SchedulerHandle>()?;
    let outcome = handle.stop(timeout);
    // 「抓取排程已停止」由執行緒結束時自己記（只記一次）；這裡只在放棄等待時補一行。
    if outcome == StopOutcome::TimedOut {
        log::warn!(
            target: report::LOG_TARGET,
            "抓取排程在 {} 秒內沒有停下，放棄等待（它會在下一個來源邊界自行結束）",
            timeout.as_secs()
        );
    }
    Some(outcome)
}

/// 非阻塞版：只設停止旗標。沒有排程時什麼都不做。
pub fn request_stop_app(app: &AppHandle) {
    if let Some(handle) = app.try_state::<SchedulerHandle>() {
        handle.request_stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::fixture::Scenario;
    use crate::fetch::sched::tests::{Fixed, HI, LO};
    use crate::fetch::test_util::TempDir;
    use std::cell::{Cell, RefCell};
    use std::sync::atomic::AtomicUsize;
    use time::macros::datetime;
    use time::Duration;

    // ───────────── 假環境（單執行緒，推進假時鐘） ─────────────

    struct FakeEnv {
        now: Cell<OffsetDateTime>,
        mode: Cell<DataFetch>,
        isolation: Option<LocalAppDataMismatch>,
        file_exists: Cell<bool>,
        state: RefCell<Option<FetchState>>,
        /// 每輪的結果；用完後一律 `Done { ok: true }`。
        script: RefCell<Vec<RoundEnd>>,
        round_secs: i64,
        rounds: RefCell<Vec<(OffsetDateTime, Trigger)>>,
        logs: RefCell<Vec<(Level, String)>>,
        panic_in_round: Cell<bool>,
        panic_in_settings: Cell<bool>,
        saved: RefCell<Vec<FetchState>>,
        /// 為真時 `save_state` 一律失敗（模擬磁碟滿、權限、防毒鎖檔）。
        fail_saves: Cell<bool>,
        /// 為真時，下一輪在中途被中止：抓取開關被改成 off、這輪沒有寫出檔案（只觸發一次）。
        abort_by_turning_off: Cell<bool>,
        /// 為真時，下一輪在中途收到停止要求而中止（只觸發一次）。
        abort_by_stop: Cell<bool>,
        /// 為真時，下一次 `save_state` panic（只觸發一次；模擬「送出 running: true 之後、送出結束之前」這次醒來炸掉）。
        panic_in_save: Cell<bool>,
        /// 為真時，下一輪進行中送來一次手動要求（只觸發一次；模擬抓取中按「立即更新資料」）。
        manual_in_round: Cell<bool>,
        /// 收到的活動狀態，依序。
        activities: RefCell<Vec<FetchActivity>>,
        /// 每次收到活動狀態時已寫入的排程紀錄筆數（驗證結束通知在寫完紀錄之後）。
        saved_at_activity: RefCell<Vec<usize>>,
    }

    impl FakeEnv {
        fn new(now: OffsetDateTime) -> FakeEnv {
            FakeEnv {
                now: Cell::new(now),
                mode: Cell::new(DataFetch::On),
                isolation: None,
                file_exists: Cell::new(true),
                state: RefCell::new(None),
                script: RefCell::new(Vec::new()),
                round_secs: 40,
                rounds: RefCell::new(Vec::new()),
                logs: RefCell::new(Vec::new()),
                panic_in_round: Cell::new(false),
                panic_in_settings: Cell::new(false),
                saved: RefCell::new(Vec::new()),
                fail_saves: Cell::new(false),
                abort_by_turning_off: Cell::new(false),
                abort_by_stop: Cell::new(false),
                panic_in_save: Cell::new(false),
                manual_in_round: Cell::new(false),
                activities: RefCell::new(Vec::new()),
                saved_at_activity: RefCell::new(Vec::new()),
            }
        }

        fn log_texts(&self) -> Vec<String> {
            self.logs.borrow().iter().map(|(_, t)| t.clone()).collect()
        }
    }

    impl SchedEnv for FakeEnv {
        fn now(&self) -> OffsetDateTime {
            self.now.get()
        }
        fn settings(&self) -> (PathBuf, DataFetch) {
            assert!(!self.panic_in_settings.replace(false), "設定讀取炸了");
            (PathBuf::from("D:\\fake-data"), self.mode.get())
        }
        fn isolation(&self) -> Option<LocalAppDataMismatch> {
            self.isolation.clone()
        }
        fn data_file_exists(&self, _dir: &Path) -> bool {
            self.file_exists.get()
        }
        fn load_state(&self) -> Option<FetchState> {
            *self.state.borrow()
        }
        fn save_state(&self, record: &FetchState) -> io::Result<()> {
            assert!(!self.panic_in_save.replace(false), "寫紀錄炸了");
            if self.fail_saves.get() {
                return Err(io::Error::other("模擬磁碟已滿"));
            }
            self.saved.borrow_mut().push(*record);
            *self.state.borrow_mut() = Some(*record);
            Ok(())
        }
        fn run_round(&self, trigger: Trigger, _dir: &Path, stop: &StopSignal) -> RoundEnd {
            self.rounds.borrow_mut().push((self.now.get(), trigger));
            assert!(!self.panic_in_round.get(), "一輪炸了");
            if self.manual_in_round.replace(false) {
                stop.request_manual();
            }
            if self.abort_by_turning_off.replace(false) {
                self.mode.set(DataFetch::Off);
                return RoundEnd::Aborted;
            }
            if self.abort_by_stop.replace(false) {
                stop.request();
                return RoundEnd::Aborted;
            }
            self.now
                .set(self.now.get() + Duration::seconds(self.round_secs));
            self.file_exists.set(true);
            let mut script = self.script.borrow_mut();
            if script.is_empty() {
                RoundEnd::Done { ok: true }
            } else {
                script.remove(0)
            }
        }
        fn log(&self, level: Level, text: &str) {
            self.logs.borrow_mut().push((level, text.to_string()));
        }
        fn activity(&self, activity: FetchActivity) {
            self.activities.borrow_mut().push(activity);
            self.saved_at_activity
                .borrow_mut()
                .push(self.saved.borrow().len());
        }
    }

    /// 假時鐘測試的醒來次數上限：回歸讓 tick 一直回 Sleep(ZERO)、假時鐘不前進時，要 panic 失敗而不是無限迴圈。
    /// 最長的合法測試（模擬數小時、每 60 秒醒一次）也遠低於此數。
    const MAX_FAKE_WAKES: u32 = 100_000;

    #[derive(Default)]
    struct WakeCap(u32);

    impl WakeCap {
        fn wake(&mut self) {
            self.0 += 1;
            assert!(
                self.0 < MAX_FAKE_WAKES,
                "模擬沒有推進（醒來 {} 次）",
                self.0
            );
        }
    }

    /// 跑迴圈到假時鐘超過 `until`；睡眠把假時鐘推進相同長度。
    fn drive(env: &FakeEnv, rng: &mut dyn Rng, until: OffsetDateTime) {
        let stop = StopSignal::new();
        let mut cap = WakeCap::default();
        run_loop(env, &stop, rng, &mut |d| {
            cap.wake();
            env.now.set(env.now.get() + Duration::try_from(d).unwrap());
            env.now.get() > until
        });
    }

    fn ok_state(start: OffsetDateTime) -> FetchState {
        FetchState {
            last_start: start,
            last_finish: Some(start + Duration::seconds(40)),
            last_any_success: true,
            consecutive_failures: 0,
        }
    }

    // ───────────── 閘門（5.3） ─────────────

    fn mismatch() -> LocalAppDataMismatch {
        LocalAppDataMismatch {
            env_value: "C:\\Temp\\iso\\local".to_string(),
            registered: "C:\\Users\\Ben\\AppData\\Local".to_string(),
        }
    }

    #[test]
    fn gate_follows_data_fetch_and_isolation() {
        let m = mismatch();
        let table = [
            (DataFetch::Auto, None, Gate::Enabled),
            (DataFetch::Auto, Some(&m), Gate::Isolated(m.clone())),
            (DataFetch::On, None, Gate::Enabled),
            (DataFetch::On, Some(&m), Gate::Enabled),
            (DataFetch::Off, None, Gate::Off),
            (DataFetch::Off, Some(&m), Gate::Off),
        ];
        for (mode, iso, want) in table {
            assert_eq!(
                decide_gate(mode, iso),
                want,
                "{mode:?} iso={}",
                iso.is_some()
            );
        }
    }

    #[test]
    fn an_isolated_environment_never_fetches_and_logs_the_reason_once() {
        let mut env = FakeEnv::new(datetime!(2026-10-05 01:00:00 UTC));
        env.mode.set(DataFetch::Auto);
        env.isolation = Some(mismatch());
        env.file_exists.set(false); // 即使資料檔不存在、紀錄也沒有
        drive(&env, &mut Fixed(LO), datetime!(2026-10-05 13:00:00 UTC));
        assert!(env.rounds.borrow().is_empty(), "隔離環境不得開始任何一輪");
        assert!(env.saved.borrow().is_empty(), "也不得寫排程紀錄");
        let reasons: Vec<String> = env
            .log_texts()
            .into_iter()
            .filter(|t| t.starts_with("隔離環境，不抓取"))
            .collect();
        assert_eq!(reasons.len(), 1, "原因只記一次：{:?}", env.log_texts());
        assert!(
            reasons[0].contains("LOCALAPPDATA=C:\\Temp\\iso\\local"),
            "{}",
            reasons[0]
        );
        assert!(
            reasons[0].contains("系統登記=C:\\Users\\Ben\\AppData\\Local"),
            "{}",
            reasons[0]
        );
    }

    #[test]
    fn data_fetch_off_never_fetches_and_on_starts_without_a_restart() {
        let env = FakeEnv::new(datetime!(2026-10-05 01:00:00 UTC));
        env.mode.set(DataFetch::Off);
        env.file_exists.set(false);
        drive(&env, &mut Fixed(LO), datetime!(2026-10-05 03:00:00 UTC));
        assert!(env.rounds.borrow().is_empty(), "off：不抓");
        // 即時生效：同一個迴圈、不重啟，設定改成 on 之後開始抓（資料檔不存在 → 首抓）。
        env.mode.set(DataFetch::On);
        let t = env.now.get();
        drive(&env, &mut Fixed(LO), t + Duration::minutes(5));
        let rounds = env.rounds.borrow();
        assert_eq!(rounds.len(), 1, "{rounds:?}");
        assert_eq!(rounds[0].1, Trigger::FirstFetch);
        assert!(rounds[0].0 - t <= Duration::seconds(15) + Duration::minutes(1));
    }

    #[test]
    fn switching_to_off_stops_new_rounds_and_back_to_on_catches_up() {
        let env = FakeEnv::new(datetime!(2026-10-05 04:00:00 UTC)); // 台北 12:00
        *env.state.borrow_mut() = Some(ok_state(datetime!(2026-10-05 03:30:00 UTC)));
        env.mode.set(DataFetch::Off);
        // off 期間跨過 15:00 時點（UTC 07:00）：不抓。
        drive(&env, &mut Fixed(LO), datetime!(2026-10-05 08:00:00 UTC));
        assert!(env.rounds.borrow().is_empty());
        env.mode.set(DataFetch::On);
        let t = env.now.get();
        drive(&env, &mut Fixed(LO), t + Duration::minutes(10));
        let rounds = env.rounds.borrow();
        assert_eq!(rounds.len(), 1, "重新啟用後補抓：{rounds:?}");
        assert_eq!(rounds[0].1, Trigger::Missed);
    }

    // ───────────── 迴圈：紀錄、失敗、防線 ─────────────

    #[test]
    fn a_finished_round_updates_the_record() {
        let env = FakeEnv::new(datetime!(2026-10-05 02:00:00 UTC));
        env.file_exists.set(false);
        drive(&env, &mut Fixed(LO), datetime!(2026-10-05 02:03:00 UTC));
        let saved = env.saved.borrow();
        assert_eq!(saved.len(), 1, "{saved:?}");
        let s = saved[0];
        assert_eq!(
            s.last_start,
            datetime!(2026-10-05 02:00:05 UTC),
            "首抓 5 秒後（亂數下界）"
        );
        assert_eq!(s.last_finish, Some(s.last_start + Duration::seconds(40)));
        assert!(s.last_any_success);
        assert_eq!(s.consecutive_failures, 0);
    }

    #[test]
    fn failed_rounds_back_off_and_a_success_resets() {
        let env = FakeEnv::new(datetime!(2026-10-05 02:00:00 UTC));
        env.file_exists.set(false);
        *env.script.borrow_mut() = vec![
            RoundEnd::Done { ok: false },
            RoundEnd::Done { ok: false },
            RoundEnd::Done { ok: true },
        ];
        drive(&env, &mut Fixed(LO), datetime!(2026-10-05 03:50:00 UTC));
        let rounds = env.rounds.borrow();
        assert_eq!(rounds[0].1, Trigger::FirstFetch);
        assert_eq!((rounds[1].1, rounds[2].1), (Trigger::Retry, Trigger::Retry));
        let gap1 = rounds[1].0 - (rounds[0].0 + Duration::seconds(40));
        let gap2 = rounds[2].0 - (rounds[1].0 + Duration::seconds(40));
        assert_eq!(
            (gap1, gap2),
            (Duration::minutes(10), Duration::minutes(20)),
            "10、20 分鐘加倍"
        );
        let saved = env.saved.borrow();
        let failures: Vec<u32> = saved
            .iter()
            .take(3)
            .map(|s| s.consecutive_failures)
            .collect();
        assert_eq!(failures, vec![1, 2, 0]);
        assert_eq!(rounds.len(), 3, "成功後回到時點節奏：{rounds:?}");
    }

    #[test]
    fn a_panicking_round_counts_as_a_failure_and_the_loop_survives() {
        let env = FakeEnv::new(datetime!(2026-10-05 02:00:00 UTC));
        env.file_exists.set(false);
        env.panic_in_round.set(true);
        drive(&env, &mut Fixed(LO), datetime!(2026-10-05 02:05:00 UTC));
        assert_eq!(env.rounds.borrow().len(), 1, "panic 後退避、不是每分鐘重來");
        {
            let saved = env.saved.borrow();
            assert_eq!(saved.len(), 1);
            assert!(!saved[0].last_any_success);
            assert_eq!(saved[0].consecutive_failures, 1);
        }
        assert!(env
            .log_texts()
            .iter()
            .any(|t| t.contains("一輪抓取 panic") && t.contains("一輪炸了")));
        // 之後恢復正常：下一次重試照常執行。
        env.panic_in_round.set(false);
        let t = env.now.get();
        drive(&env, &mut Fixed(LO), t + Duration::minutes(30));
        assert_eq!(env.rounds.borrow().len(), 2);
        // 第二次 `drive` 等於宿主重啟（迴圈狀態重來）：資料檔仍不存在 → 一律首抓（舊的失敗紀錄不延後首抓）。
        assert_eq!(env.rounds.borrow()[1].1, Trigger::FirstFetch);
    }

    #[test]
    fn a_panic_outside_the_round_is_logged_and_the_next_wake_is_normal() {
        let env = FakeEnv::new(datetime!(2026-10-05 02:00:00 UTC));
        env.file_exists.set(false);
        env.panic_in_settings.set(true);
        drive(&env, &mut Fixed(LO), datetime!(2026-10-05 02:03:00 UTC));
        assert!(env
            .log_texts()
            .iter()
            .any(|t| t.contains("這次醒來的處理 panic") && t.contains("設定讀取炸了")));
        assert_eq!(env.rounds.borrow().len(), 1, "下一次醒來照常執行首抓");
    }

    #[test]
    fn a_clock_set_back_is_corrected_in_the_record_and_does_not_fetch() {
        let env = FakeEnv::new(datetime!(2026-10-05 04:00:00 UTC)); // 台北 12:00
        *env.state.borrow_mut() = Some(ok_state(datetime!(2026-10-05 07:30:00 UTC))); // 3.5 小時後
        drive(&env, &mut Fixed(LO), datetime!(2026-10-05 04:10:00 UTC));
        assert!(env.rounds.borrow().is_empty());
        let saved = env.saved.borrow();
        assert_eq!(saved.len(), 1, "只改寫一次：{saved:?}");
        assert_eq!(
            saved[0].last_start,
            datetime!(2026-10-05 04:00:00 UTC),
            "視為現在"
        );
        assert!(env.log_texts().iter().any(|t| t.contains("時鐘倒退")));
    }

    /// 審查 I1：排程紀錄持續寫不進去時，60 分鐘下限仍然有效（記憶體備援）。沒有備援時，磁碟上永遠讀不到
    /// 紀錄，每輪結束後 30–120 秒就會「錯過」補抓，變成每 1–3 分鐘一整輪的熱迴圈。
    #[test]
    fn the_minimum_interval_survives_a_record_file_that_can_never_be_written() {
        let env = FakeEnv::new(datetime!(2026-10-05 00:00:00 UTC)); // 台北 08:00
        env.fail_saves.set(true);
        drive(&env, &mut Fixed(LO), datetime!(2026-10-05 12:00:00 UTC)); // 台北 20:00
        let rounds = env.rounds.borrow();
        assert!(env.saved.borrow().is_empty(), "測試前提：一次都寫不進去");
        assert!(
            rounds.len() <= 4,
            "12 小時內時點 12／15／18 共 3 個，加一次啟動補抓：{rounds:?}"
        );
        assert!(rounds.len() >= 3, "下限不是丟棄：時點輪仍要抓：{rounds:?}");
        for w in rounds.windows(2) {
            assert!(
                w[1].0 - w[0].0 >= Duration::minutes(60),
                "任兩輪開始至少相隔 60 分鐘：{rounds:?}"
            );
        }
        let warns = env
            .logs
            .borrow()
            .iter()
            .filter(|(l, t)| *l == Level::Warn && t.contains("寫入排程紀錄失敗"))
            .count();
        assert_eq!(
            warns,
            1,
            "寫入失敗只在狀態改變時記一次：{:?}",
            env.log_texts()
        );
    }

    #[test]
    fn failure_backoff_survives_a_record_file_that_can_never_be_written() {
        let env = FakeEnv::new(datetime!(2026-10-05 00:00:00 UTC));
        env.fail_saves.set(true);
        *env.script.borrow_mut() = vec![RoundEnd::Done { ok: false }; 8];
        drive(&env, &mut Fixed(LO), datetime!(2026-10-05 03:30:00 UTC));
        let rounds = env.rounds.borrow();
        assert_eq!(rounds[0].1, Trigger::Missed);
        let gaps: Vec<i64> = rounds
            .windows(2)
            .map(|w| (w[1].0 - (w[0].0 + Duration::seconds(env.round_secs))).whole_minutes())
            .collect();
        assert_eq!(gaps[..3], [10, 20, 40], "{rounds:?}");
        assert!(rounds[1..].iter().all(|(_, t)| *t == Trigger::Retry));
    }

    #[test]
    fn a_record_file_that_becomes_writable_again_is_noted_once() {
        let env = FakeEnv::new(datetime!(2026-10-05 00:00:00 UTC));
        env.fail_saves.set(true);
        let healed = Cell::new(false);
        let stop = StopSignal::new();
        // 同一個迴圈：00:10 之後紀錄檔恢復可寫，且資料檔被刪（再來一輪首抓，這次寫得進去）。
        let mut cap = WakeCap::default();
        run_loop(&env, &stop, &mut Fixed(LO), &mut |d| {
            cap.wake();
            env.now.set(env.now.get() + Duration::try_from(d).unwrap());
            if !healed.get() && env.now.get() > datetime!(2026-10-05 00:10:00 UTC) {
                healed.set(true);
                env.fail_saves.set(false);
                env.file_exists.set(false);
            }
            env.now.get() > datetime!(2026-10-05 00:20:00 UTC)
        });
        assert!(!env.saved.borrow().is_empty());
        let texts = env.log_texts();
        assert_eq!(
            texts.iter().filter(|t| t.contains("恢復可寫入")).count(),
            1,
            "{texts:?}"
        );
        assert_eq!(
            texts
                .iter()
                .filter(|t| t.contains("寫入排程紀錄失敗"))
                .count(),
            1,
            "{texts:?}"
        );
    }

    /// 審查第 2 輪結論：有鑑別力的 N2 測試。中止後**第一次零長度睡眠**（迴圈在 Aborted 分支之後立刻回傳
    /// `Sleep(0)`）就把抓取開關設回 on——中間沒有任何「閘門關閉」的醒來，所以閘門關閉時的 `idle_wake` 不會
    /// 幫忙解除抑制；解除抑制只能靠 Aborted 分支本身不設抑制。若 Aborted 分支誤呼叫 `round_finished(…, 檔案不存在)`，
    /// 下一輪會被抑制、變成 30–120 秒的 `missed`（沒有紀錄時），此測試就會失敗。
    #[test]
    fn an_abort_followed_at_once_by_on_again_first_fetches_within_fifteen_seconds_of_the_abort() {
        let env = FakeEnv::new(datetime!(2026-10-05 02:00:00 UTC));
        env.file_exists.set(false);
        env.abort_by_turning_off.set(true);
        let stop = StopSignal::new();
        let mut cap = WakeCap::default();
        run_loop(&env, &stop, &mut Fixed(HI), &mut |d| {
            cap.wake();
            env.now.set(env.now.get() + Duration::try_from(d).unwrap());
            if d.is_zero() && env.mode.get() == DataFetch::Off {
                // Aborted 分支之後的第一次零長度睡眠：立刻打開。
                env.mode.set(DataFetch::On);
            }
            env.now.get() > datetime!(2026-10-05 02:05:00 UTC)
        });
        let rounds = env.rounds.borrow();
        assert_eq!(rounds.len(), 2, "{rounds:?}");
        let aborted_at = rounds[0].0;
        assert_eq!(
            rounds[1].1,
            Trigger::FirstFetch,
            "不是被抑制後的 missed：{rounds:?}"
        );
        assert!(
            rounds[1].0 - aborted_at <= Duration::seconds(15),
            "距離中止不得超過 15 秒（首抓延遲上界）：{rounds:?}"
        );
    }

    /// 審查 N2：首抓途中被中止（抓取被關掉）沒有「跑完一輪」，不可設首抓抑制；之後重新打開仍要在 5–15 秒內
    /// 首抓（沒有紀錄、資料檔不存在時，被抑制的話會變成 30–120 秒的「錯過」）。
    #[test]
    fn an_aborted_first_fetch_then_off_then_on_still_first_fetches_within_fifteen_seconds() {
        let env = FakeEnv::new(datetime!(2026-10-05 02:00:00 UTC));
        env.file_exists.set(false);
        env.abort_by_turning_off.set(true);
        let reenabled_at = Cell::new(None::<OffsetDateTime>);
        let stop = StopSignal::new();
        let mut cap = WakeCap::default();
        run_loop(&env, &stop, &mut Fixed(HI), &mut |d| {
            cap.wake();
            env.now.set(env.now.get() + Duration::try_from(d).unwrap());
            // 關閉三分鐘之後重新打開。
            if reenabled_at.get().is_none()
                && env.mode.get() == DataFetch::Off
                && env.now.get() > datetime!(2026-10-05 02:04:00 UTC)
            {
                env.mode.set(DataFetch::On);
                reenabled_at.set(Some(env.now.get()));
            }
            env.now.get() > datetime!(2026-10-05 02:10:00 UTC)
        });
        let rounds = env.rounds.borrow();
        assert_eq!(rounds.len(), 2, "{rounds:?}");
        assert_eq!(rounds[0].1, Trigger::FirstFetch);
        assert_eq!(rounds[1].1, Trigger::FirstFetch, "不是 30–120 秒的 Missed");
        let flip = reenabled_at.get().expect("有重新打開");
        // 重新打開後第一次醒來最晚 60 秒、再加首抓延遲上界 15 秒。
        assert!(rounds[1].0 >= flip);
        assert!(
            rounds[1].0 - flip <= Duration::seconds(75),
            "{rounds:?} flip={flip}"
        );
        assert!(
            env.saved.borrow().len() == 1,
            "中止的一輪不寫紀錄，第二輪寫一筆"
        );
    }

    /// 審查 N1：時鐘倒退又寫不進紀錄檔時，磁碟上那筆「未來」的紀錄只在啟動時修正一次；之後用記憶體紀錄，
    /// 不會每分鐘重複修正、重複警告，牆上時鐘追上之前就能依時點恢復抓取（design D5「時鐘倒退」的定義）。
    #[test]
    fn a_clock_set_back_with_an_unwritable_record_file_warns_once_and_still_recovers() {
        let env = FakeEnv::new(datetime!(2026-10-05 04:00:00 UTC)); // 台北 12:00
        env.fail_saves.set(true);
        *env.state.borrow_mut() = Some(ok_state(datetime!(2026-10-05 07:30:00 UTC))); // 磁碟上的「未來」
        drive(&env, &mut Fixed(LO), datetime!(2026-10-05 08:00:00 UTC));
        let texts = env.log_texts();
        assert_eq!(
            texts.iter().filter(|t| t.contains("時鐘倒退")).count(),
            1,
            "只修正一次：{texts:?}"
        );
        assert_eq!(
            texts
                .iter()
                .filter(|t| t.contains("寫入排程紀錄失敗"))
                .count(),
            1,
            "{texts:?}"
        );
        let rounds = env.rounds.borrow();
        assert_eq!(rounds.len(), 1, "{rounds:?}");
        assert_eq!(
            rounds[0].1,
            Trigger::Slot,
            "15:00 時點照常抓（牆上時鐘遠早於磁碟上的未來時間）"
        );
        assert!(
            rounds[0].0 >= datetime!(2026-10-05 07:00:00 UTC)
                && rounds[0].0 <= datetime!(2026-10-05 07:06:00 UTC),
            "{rounds:?}"
        );
    }

    #[test]
    fn missing_record_with_an_existing_data_file_catches_up_as_missed() {
        let env = FakeEnv::new(datetime!(2026-10-05 02:00:00 UTC));
        drive(&env, &mut Fixed(HI), datetime!(2026-10-05 02:10:00 UTC));
        let rounds = env.rounds.borrow();
        assert_eq!(rounds.len(), 1);
        assert_eq!(
            rounds[0],
            (datetime!(2026-10-05 02:02:00 UTC), Trigger::Missed),
            "120 秒（亂數上界）"
        );
    }

    #[test]
    fn stopping_ends_the_loop_without_touching_the_record() {
        let env = FakeEnv::new(datetime!(2026-10-05 02:00:00 UTC));
        env.file_exists.set(false);
        let stop = StopSignal::new();
        stop.request();
        let mut rng = Fixed(LO);
        run_loop(&env, &stop, &mut rng, &mut |_| false);
        assert!(
            env.rounds.borrow().is_empty(),
            "已要求停止就不再開始新的一輪"
        );
        // 輪中被停止：Aborted 不更新紀錄、迴圈結束。
        let stop = StopSignal::new();
        let env2 = FakeEnv::new(datetime!(2026-10-05 02:00:00 UTC));
        env2.file_exists.set(false);
        *env2.script.borrow_mut() = vec![RoundEnd::Aborted];
        let mut calls = 0;
        // 第一輪被標為 Aborted；模擬「停止要求在輪中到來」：在 run_round 之後 stop 已成立。
        struct StopAfter<'a>(&'a FakeEnv, &'a StopSignal);
        impl SchedEnv for StopAfter<'_> {
            fn now(&self) -> OffsetDateTime {
                self.0.now()
            }
            fn settings(&self) -> (PathBuf, DataFetch) {
                self.0.settings()
            }
            fn isolation(&self) -> Option<LocalAppDataMismatch> {
                None
            }
            fn data_file_exists(&self, d: &Path) -> bool {
                self.0.data_file_exists(d)
            }
            fn load_state(&self) -> Option<FetchState> {
                self.0.load_state()
            }
            fn save_state(&self, s: &FetchState) -> io::Result<()> {
                self.0.save_state(s)
            }
            fn run_round(&self, t: Trigger, d: &Path, s: &StopSignal) -> RoundEnd {
                let end = self.0.run_round(t, d, s);
                self.1.request();
                end
            }
            fn log(&self, l: Level, t: &str) {
                self.0.log(l, t)
            }
        }
        let wrapper = StopAfter(&env2, &stop);
        run_loop(&wrapper, &stop, &mut Fixed(LO), &mut |d| {
            calls += 1;
            env2.now
                .set(env2.now.get() + Duration::try_from(d).unwrap());
            false
        });
        assert_eq!(env2.rounds.borrow().len(), 1);
        assert!(env2.saved.borrow().is_empty(), "被中止的一輪不更新排程紀錄");
        assert!(
            calls <= 1,
            "停止後迴圈立即結束，只可能睡過一次（首抓前的 5 秒）：{calls}"
        );
    }

    // ───────────── 活動狀態（fetch-progress-and-manual-refresh 1.2） ─────────────

    fn act(running: bool, enabled: bool, last: LastRound) -> FetchActivity {
        FetchActivity {
            running,
            enabled,
            last,
        }
    }

    use LastRound::{Failed, Never, Succeeded};

    #[test]
    fn a_normal_round_reports_idle_then_running_then_ok() {
        let env = FakeEnv::new(datetime!(2026-10-05 02:00:00 UTC));
        env.file_exists.set(false);
        drive(&env, &mut Fixed(LO), datetime!(2026-10-05 02:30:00 UTC));
        assert_eq!(env.rounds.borrow().len(), 1);
        assert_eq!(
            *env.activities.borrow(),
            [
                act(false, true, Never),
                act(true, true, Never),
                act(false, true, Succeeded)
            ],
            "之後每分鐘醒來不重送相同的值"
        );
        assert_eq!(
            *env.saved_at_activity.borrow(),
            [0, 0, 1],
            "結束通知在寫完排程紀錄之後（設定視窗收到後重讀紀錄要讀到這一輪）"
        );
    }

    #[test]
    fn a_round_where_every_source_fails_reports_last_failed() {
        let env = FakeEnv::new(datetime!(2026-10-05 02:00:00 UTC));
        env.file_exists.set(false);
        *env.script.borrow_mut() = vec![RoundEnd::Done { ok: false }];
        drive(&env, &mut Fixed(LO), datetime!(2026-10-05 02:05:00 UTC));
        assert_eq!(
            *env.activities.borrow(),
            [
                act(false, true, Never),
                act(true, true, Never),
                act(false, true, Failed)
            ]
        );
    }

    #[test]
    fn a_round_aborted_by_a_stop_request_reports_not_running_and_keeps_last() {
        let env = FakeEnv::new(datetime!(2026-10-05 02:00:00 UTC));
        env.file_exists.set(false);
        env.abort_by_stop.set(true);
        drive(&env, &mut Fixed(LO), datetime!(2026-10-05 02:30:00 UTC));
        assert_eq!(env.rounds.borrow().len(), 1);
        assert!(env.saved.borrow().is_empty());
        assert_eq!(
            *env.activities.borrow(),
            [
                act(false, true, Never),
                act(true, true, Never),
                act(false, true, Never)
            ],
            "停止前送出 running: false；被中止的一輪不改 last"
        );
    }

    #[test]
    fn a_gate_closed_mid_round_reports_not_running_and_disabled() {
        let env = FakeEnv::new(datetime!(2026-10-05 02:00:00 UTC));
        *env.state.borrow_mut() = Some(FetchState {
            last_any_success: false,
            consecutive_failures: 1,
            ..ok_state(datetime!(2026-10-05 01:00:00 UTC))
        });
        env.abort_by_turning_off.set(true);
        drive(&env, &mut Fixed(LO), datetime!(2026-10-05 02:30:00 UTC));
        assert_eq!(env.rounds.borrow().len(), 1);
        assert_eq!(
            *env.activities.borrow(),
            [
                act(false, true, Failed),
                act(true, true, Failed),
                act(false, false, Failed)
            ],
            "結束通知就帶 enabled: false，之後閘門關閉的醒來不重送"
        );
    }

    #[test]
    fn a_panicking_round_reports_not_running_and_failed() {
        let env = FakeEnv::new(datetime!(2026-10-05 02:00:00 UTC));
        env.file_exists.set(false);
        env.panic_in_round.set(true);
        drive(&env, &mut Fixed(LO), datetime!(2026-10-05 02:05:00 UTC));
        assert_eq!(
            *env.activities.borrow(),
            [
                act(false, true, Never),
                act(true, true, Never),
                act(false, true, Failed)
            ]
        );
    }

    /// 送出 `running: true` 之後這次醒來在別處 panic（這裡是寫排程紀錄）：結束通知沒送到，下一次醒來開頭補送。
    #[test]
    fn a_wake_that_panics_after_reporting_running_is_corrected_on_the_next_wake() {
        let env = FakeEnv::new(datetime!(2026-10-05 02:00:00 UTC));
        env.file_exists.set(false);
        env.panic_in_save.set(true);
        drive(&env, &mut Fixed(LO), datetime!(2026-10-05 02:05:00 UTC));
        assert!(env
            .log_texts()
            .iter()
            .any(|t| t.contains("這次醒來的處理 panic") && t.contains("寫紀錄炸了")));
        assert_eq!(env.rounds.borrow().len(), 1);
        assert_eq!(
            *env.activities.borrow(),
            [
                act(false, true, Never),
                act(true, true, Never),
                act(false, true, Succeeded)
            ]
        );
    }

    #[test]
    fn last_is_initialized_from_the_schedule_record_at_startup_even_when_disabled() {
        let recent = datetime!(2026-10-05 01:55:00 UTC);
        for (any_success, mode, want) in [
            (true, DataFetch::On, act(false, true, Succeeded)),
            (false, DataFetch::On, act(false, true, Failed)),
            (true, DataFetch::Off, act(false, false, Succeeded)),
        ] {
            let env = FakeEnv::new(datetime!(2026-10-05 02:00:00 UTC));
            env.mode.set(mode);
            *env.state.borrow_mut() = Some(FetchState {
                last_any_success: any_success,
                consecutive_failures: u32::from(!any_success),
                ..ok_state(recent)
            });
            // 失敗紀錄的重試在 10 分鐘後；只看第一次醒來的值。
            drive(&env, &mut Fixed(LO), datetime!(2026-10-05 02:03:00 UTC));
            assert!(env.rounds.borrow().is_empty());
            assert_eq!(*env.activities.borrow(), [want], "{any_success} {mode:?}");
        }
    }

    /// managed state 的初始值（`initial_activity`）與迴圈第一次醒來送出的值一致：頁面先查到的與第一筆事件不會互相矛盾。
    #[test]
    fn the_initial_store_value_equals_the_first_value_the_loop_reports() {
        let m = mismatch();
        let records = [
            None,
            Some(ok_state(datetime!(2026-10-05 01:55:00 UTC))),
            Some(FetchState {
                last_any_success: false,
                consecutive_failures: 1,
                ..ok_state(datetime!(2026-10-05 01:55:00 UTC))
            }),
        ];
        for mode in [DataFetch::Auto, DataFetch::On, DataFetch::Off] {
            for iso in [None, Some(m.clone())] {
                for record in records {
                    let mut env = FakeEnv::new(datetime!(2026-10-05 02:00:00 UTC));
                    env.mode.set(mode);
                    env.isolation = iso.clone();
                    *env.state.borrow_mut() = record;
                    drive(&env, &mut Fixed(LO), datetime!(2026-10-05 02:00:01 UTC));
                    let want = initial_activity(mode, iso.as_ref(), record.as_ref());
                    assert_eq!(
                        env.activities.borrow().first(),
                        Some(&want),
                        "{mode:?} iso={} record={record:?}",
                        iso.is_some()
                    );
                }
            }
        }
    }

    /// 同一個迴圈內閘門 off → on（不經重啟）只補送一次。
    #[test]
    fn a_gate_change_within_one_loop_is_reported_once() {
        let env = FakeEnv::new(datetime!(2026-10-05 02:00:00 UTC));
        env.mode.set(DataFetch::Off);
        *env.state.borrow_mut() = Some(ok_state(datetime!(2026-10-05 01:50:00 UTC)));
        let stop = StopSignal::new();
        let mut cap = WakeCap::default();
        run_loop(&env, &stop, &mut Fixed(LO), &mut |d| {
            cap.wake();
            env.now.set(env.now.get() + Duration::try_from(d).unwrap());
            if env.now.get() > datetime!(2026-10-05 02:05:00 UTC) {
                env.mode.set(DataFetch::On);
            }
            env.now.get() > datetime!(2026-10-05 02:20:00 UTC)
        });
        assert!(env.rounds.borrow().is_empty());
        assert_eq!(
            *env.activities.borrow(),
            [act(false, false, Succeeded), act(false, true, Succeeded)]
        );
    }

    // ───────────── 手動更新（fetch-progress-and-manual-refresh 2.2） ─────────────

    /// 跑迴圈到假時鐘超過 `until`，並在 `requests` 的各時刻送出手動要求。睡眠模擬 `StopSignal::wait`：要求落在這次睡眠
    /// 之內就在那一刻醒來；旗標還沒被取走就不睡（真的 `wait` 會立刻返回），所以迴圈若漏取旗標，會在 `WakeCap` 撞上限。
    fn drive_with_requests(
        env: &FakeEnv,
        rng: &mut dyn Rng,
        until: OffsetDateTime,
        requests: &[OffsetDateTime],
    ) {
        let stop = StopSignal::new();
        let mut cap = WakeCap::default();
        let mut pending: Vec<OffsetDateTime> = requests.to_vec();
        run_loop(env, &stop, rng, &mut |d| {
            cap.wake();
            if stop.manual_pending() {
                return false;
            }
            let wake_at = env.now.get() + Duration::try_from(d).unwrap();
            match pending.first().copied() {
                Some(t) if t <= wake_at => {
                    pending.remove(0);
                    env.now.set(t.max(env.now.get()));
                    stop.request_manual();
                }
                _ => env.now.set(wake_at),
            }
            env.now.get() > until
        });
    }

    /// 上一輪 08:30（台北）開始、成功：12:00 時點之前沒有該抓的。
    fn morning_record() -> FetchState {
        ok_state(datetime!(2026-10-05 00:30:00 UTC))
    }

    /// spec「排程剛抓完就手動更新」：12:00 時點那輪 12:00:40 抓完，12:03:40 按下 → 立刻開始，不等到 13:00。
    #[test]
    fn a_manual_request_three_minutes_after_a_scheduled_round_starts_at_once() {
        let env = FakeEnv::new(datetime!(2026-10-05 03:58:00 UTC)); // 台北 11:58
        *env.state.borrow_mut() = Some(morning_record());
        let pressed = datetime!(2026-10-05 04:03:40 UTC);
        drive_with_requests(
            &env,
            &mut Fixed(LO),
            datetime!(2026-10-05 06:30:00 UTC),
            &[pressed],
        );
        assert_eq!(
            *env.rounds.borrow(),
            [
                (datetime!(2026-10-05 04:00:00 UTC), Trigger::Slot),
                (pressed, Trigger::Manual),
            ],
            "手動不受 60 分鐘下限；之後到 15:00 時點前不再抓"
        );
        let saved = env.saved.borrow();
        assert_eq!(saved.len(), 2);
        assert_eq!(saved[1].last_start, pressed, "手動那一輪照常寫排程紀錄");
        assert!(saved[1].last_any_success);
    }

    /// spec「手動更新後緊接排程時點」：11:40 手動 → 12:00 時點延後到 12:40（下限以手動那一輪的開始時間計算）。
    #[test]
    fn a_slot_twenty_minutes_after_a_manual_round_waits_for_the_minimum_interval() {
        let env = FakeEnv::new(datetime!(2026-10-05 03:39:00 UTC)); // 台北 11:39
        *env.state.borrow_mut() = Some(morning_record());
        let pressed = datetime!(2026-10-05 03:40:00 UTC);
        drive_with_requests(
            &env,
            &mut Fixed(LO),
            datetime!(2026-10-05 05:30:00 UTC),
            &[pressed],
        );
        assert_eq!(
            *env.rounds.borrow(),
            [
                (pressed, Trigger::Manual),
                (datetime!(2026-10-05 04:40:00 UTC), Trigger::Slot),
            ],
            "12:00 時點延後、不丟棄"
        );
        assert_eq!(
            *env.activities.borrow(),
            [
                act(false, true, Succeeded),
                act(true, true, Succeeded),
                act(false, true, Succeeded),
                act(true, true, Succeeded),
                act(false, true, Succeeded),
            ],
            "手動輪與排程輪走同一組活動狀態回報"
        );
    }

    /// spec「抓取進行中再按一次」：進行中送來的要求由這一輪吸收，結束後不緊接著再抓一輪——排程輪與手動輪都一樣。
    #[test]
    fn a_request_during_a_round_does_not_cause_a_second_round() {
        // 排程輪（首抓）進行中按下。
        let env = FakeEnv::new(datetime!(2026-10-05 02:00:00 UTC));
        env.file_exists.set(false);
        env.manual_in_round.set(true);
        drive_with_requests(
            &env,
            &mut Fixed(LO),
            datetime!(2026-10-05 03:00:00 UTC),
            &[],
        );
        assert_eq!(
            *env.rounds.borrow(),
            [(datetime!(2026-10-05 02:00:05 UTC), Trigger::FirstFetch)]
        );

        // 手動輪進行中再按一次。
        let env = FakeEnv::new(datetime!(2026-10-05 03:39:00 UTC));
        *env.state.borrow_mut() = Some(morning_record());
        env.manual_in_round.set(true);
        let pressed = datetime!(2026-10-05 03:40:00 UTC);
        drive_with_requests(
            &env,
            &mut Fixed(LO),
            datetime!(2026-10-05 04:30:00 UTC),
            &[pressed],
        );
        assert_eq!(*env.rounds.borrow(), [(pressed, Trigger::Manual)]);
    }

    /// spec「抓取已關閉」：閘門未啟用時的要求丟棄、不抓（旗標沒取走的話 `drive_with_requests` 會因不睡而撞上限）。
    #[test]
    fn a_manual_request_while_disabled_is_dropped() {
        for (mode, iso) in [(DataFetch::Off, None), (DataFetch::Auto, Some(mismatch()))] {
            let mut env = FakeEnv::new(datetime!(2026-10-05 03:39:00 UTC));
            env.mode.set(mode);
            env.isolation = iso;
            *env.state.borrow_mut() = Some(morning_record());
            drive_with_requests(
                &env,
                &mut Fixed(LO),
                datetime!(2026-10-05 03:50:00 UTC),
                &[datetime!(2026-10-05 03:40:00 UTC)],
            );
            assert!(env.rounds.borrow().is_empty(), "{mode:?}");
            assert!(
                env.log_texts()
                    .iter()
                    .any(|t| t.contains("收到立即更新要求，但抓取未啟用")),
                "{:?}",
                env.log_texts()
            );
        }
    }

    /// 閘門在要求送出之後、迴圈醒來之前被關掉，之後在同一個迴圈內重新啟用：要求已丟棄，重新啟用後不冒出遲來的手動輪。
    #[test]
    fn a_request_followed_by_turning_fetch_off_is_dropped_within_the_same_loop() {
        let env = FakeEnv::new(datetime!(2026-10-05 03:39:00 UTC));
        *env.state.borrow_mut() = Some(morning_record());
        let stop = StopSignal::new();
        let mut cap = WakeCap::default();
        let mut wakes = 0;
        run_loop(&env, &stop, &mut Fixed(LO), &mut |d| {
            cap.wake();
            wakes += 1;
            if stop.manual_pending() {
                return false;
            }
            match wakes {
                1 => {
                    stop.request_manual();
                    env.mode.set(DataFetch::Off);
                }
                3 => env.mode.set(DataFetch::On),
                _ => env.now.set(env.now.get() + Duration::try_from(d).unwrap()),
            }
            env.now.get() > datetime!(2026-10-05 03:55:00 UTC)
        });
        assert!(env.rounds.borrow().is_empty(), "{:?}", env.rounds.borrow());
    }

    /// 取走手動要求之前就 panic：丟掉這次要求、照常睡（旗標留著的話 `wait` 立刻返回，決定性的 panic 會變成空轉）。
    #[test]
    fn a_panic_before_the_request_is_taken_drops_it_instead_of_spinning() {
        let env = FakeEnv::new(datetime!(2026-10-05 03:39:00 UTC));
        *env.state.borrow_mut() = Some(morning_record());
        let stop = StopSignal::new();
        let mut cap = WakeCap::default();
        let mut wakes = 0;
        run_loop(&env, &stop, &mut Fixed(LO), &mut |d| {
            cap.wake();
            wakes += 1;
            if stop.manual_pending() {
                return false;
            }
            if wakes == 1 {
                stop.request_manual();
                env.panic_in_settings.set(true);
            } else {
                env.now.set(env.now.get() + Duration::try_from(d).unwrap());
            }
            env.now.get() > datetime!(2026-10-05 03:55:00 UTC)
        });
        assert!(env
            .log_texts()
            .iter()
            .any(|t| t.contains("這次醒來的處理 panic")));
        assert!(env.rounds.borrow().is_empty(), "{:?}", env.rounds.borrow());
    }

    /// 手動輪失敗同樣走既有的重試退避（10–15 分鐘，下界 10 分鐘）。
    #[test]
    fn a_failed_manual_round_backs_off_like_any_failure() {
        let env = FakeEnv::new(datetime!(2026-10-05 03:39:00 UTC));
        *env.state.borrow_mut() = Some(morning_record());
        *env.script.borrow_mut() = vec![RoundEnd::Done { ok: false }];
        let pressed = datetime!(2026-10-05 03:40:00 UTC);
        drive_with_requests(
            &env,
            &mut Fixed(LO),
            datetime!(2026-10-05 03:59:00 UTC),
            &[pressed],
        );
        assert_eq!(
            *env.rounds.borrow(),
            [
                (pressed, Trigger::Manual),
                (
                    pressed + Duration::seconds(40) + Duration::minutes(10),
                    Trigger::Retry
                ),
            ]
        );
        let saved = env.saved.borrow();
        assert_eq!(
            (saved[0].last_any_success, saved[0].consecutive_failures),
            (false, 1)
        );
    }

    // ───────────── StopSignal ─────────────

    #[test]
    fn a_manual_request_wakes_a_sleeper_in_another_thread_without_meaning_stop() {
        let s = Arc::new(StopSignal::new());
        let s2 = Arc::clone(&s);
        let t = std::time::Instant::now();
        let sleeper = thread::spawn(move || s2.wait(StdDuration::from_secs(30)));
        thread::sleep(StdDuration::from_millis(50));
        s.request_manual();
        assert!(!sleeper.join().unwrap(), "回傳值只表示停止");
        assert!(t.elapsed() < StdDuration::from_secs(5));
        assert!(!s.is_stopped());
        // 旗標還在（留給迴圈取走）：取走一次為真、之後為假，取走後 `wait` 照常睡滿。
        assert!(s.take_manual());
        assert!(!s.take_manual());
        let t = std::time::Instant::now();
        assert!(!s.wait(StdDuration::from_millis(50)));
        assert!(t.elapsed() >= StdDuration::from_millis(40));
    }

    #[test]
    fn stop_signal_wait_times_out_or_wakes_early() {
        let s = StopSignal::new();
        let t = std::time::Instant::now();
        assert!(!s.wait(StdDuration::from_millis(50)));
        assert!(t.elapsed() >= StdDuration::from_millis(40));
        s.request();
        let t = std::time::Instant::now();
        assert!(s.wait(StdDuration::from_secs(30)), "已要求停止：立即返回");
        assert!(t.elapsed() < StdDuration::from_secs(2));
    }

    #[test]
    fn stop_signal_wakes_a_sleeper_in_another_thread() {
        let s = Arc::new(StopSignal::new());
        let s2 = Arc::clone(&s);
        let t = std::time::Instant::now();
        let sleeper = thread::spawn(move || s2.wait(StdDuration::from_secs(30)));
        thread::sleep(StdDuration::from_millis(50));
        s.request();
        assert!(sleeper.join().unwrap());
        assert!(t.elapsed() < StdDuration::from_secs(5));
    }

    // ───────────── 執行緒與停止介面（真實執行緒、真實睡眠） ─────────────

    /// 可跨執行緒的假環境：閘門關閉（`data_fetch=off`）時迴圈只睡覺；`round` 決定一輪做什麼。
    struct ThreadEnv {
        mode: DataFetch,
        round_started: Arc<AtomicBool>,
        round_behavior: Behavior,
    }

    #[derive(Clone, Copy)]
    enum Behavior {
        /// 不會被呼叫（閘門關閉）。
        Never,
        /// 一直忙到停止旗標成立，才在「邊界」回傳 Aborted。
        UntilStopped,
        /// 忽略停止旗標，固定睡這麼久（模擬卡在慢請求）。
        IgnoreStopFor(StdDuration),
    }

    impl SchedEnv for ThreadEnv {
        fn now(&self) -> OffsetDateTime {
            OffsetDateTime::now_utc()
        }
        fn settings(&self) -> (PathBuf, DataFetch) {
            (PathBuf::from("D:\\fake-data"), self.mode)
        }
        fn isolation(&self) -> Option<LocalAppDataMismatch> {
            None
        }
        fn data_file_exists(&self, _dir: &Path) -> bool {
            false
        }
        fn load_state(&self) -> Option<FetchState> {
            None
        }
        fn save_state(&self, _record: &FetchState) -> io::Result<()> {
            Ok(())
        }
        fn run_round(&self, _t: Trigger, _d: &Path, stop: &StopSignal) -> RoundEnd {
            self.round_started.store(true, Ordering::SeqCst);
            match self.round_behavior {
                Behavior::Never => unreachable!("閘門關閉時不該開始一輪"),
                Behavior::UntilStopped => {
                    while !stop.is_stopped() {
                        thread::sleep(StdDuration::from_millis(5));
                    }
                    RoundEnd::Aborted
                }
                Behavior::IgnoreStopFor(d) => {
                    thread::sleep(d);
                    RoundEnd::Aborted
                }
            }
        }
        fn log(&self, _level: Level, _text: &str) {}
    }

    #[test]
    fn stop_interrupts_the_idle_sleep_immediately() {
        let started = Arc::new(AtomicBool::new(false));
        let handle = spawn(
            ThreadEnv {
                mode: DataFetch::Off,
                round_started: Arc::clone(&started),
                round_behavior: Behavior::Never,
            },
            Box::new(Fixed(LO)),
        )
        .unwrap();
        thread::sleep(StdDuration::from_millis(100));
        assert!(!handle.is_finished());
        let t = std::time::Instant::now();
        assert_eq!(
            handle.stop(StdDuration::from_secs(10)),
            StopOutcome::Stopped
        );
        assert!(
            t.elapsed() < StdDuration::from_secs(3),
            "60 秒睡眠要被打斷：{:?}",
            t.elapsed()
        );
        assert!(handle.is_finished());
        assert!(!started.load(Ordering::SeqCst));
        // 可重複呼叫。
        assert_eq!(handle.stop(StdDuration::from_secs(1)), StopOutcome::Stopped);
    }

    #[test]
    fn stop_reaches_a_round_in_progress() {
        let started = Arc::new(AtomicBool::new(false));
        let handle = spawn(
            ThreadEnv {
                mode: DataFetch::On,
                round_started: Arc::clone(&started),
                round_behavior: Behavior::UntilStopped,
            },
            // 資料檔不存在、紀錄沒有 → 首抓，亂數下界 5 秒。
            Box::new(Fixed(LO)),
        )
        .unwrap();
        let wait_start = std::time::Instant::now();
        while !started.load(Ordering::SeqCst) {
            assert!(
                wait_start.elapsed() < StdDuration::from_secs(15),
                "5 秒首抓沒有開始"
            );
            thread::sleep(StdDuration::from_millis(20));
        }
        assert_eq!(handle.stop(StdDuration::from_secs(5)), StopOutcome::Stopped);
    }

    #[test]
    fn stop_gives_up_after_the_timeout_when_a_round_will_not_end() {
        let started = Arc::new(AtomicBool::new(false));
        let handle = spawn(
            ThreadEnv {
                mode: DataFetch::On,
                round_started: Arc::clone(&started),
                round_behavior: Behavior::IgnoreStopFor(StdDuration::from_millis(1500)),
            },
            Box::new(Fixed(LO)),
        )
        .unwrap();
        let wait_start = std::time::Instant::now();
        while !started.load(Ordering::SeqCst) {
            assert!(wait_start.elapsed() < StdDuration::from_secs(15));
            thread::sleep(StdDuration::from_millis(20));
        }
        let t = std::time::Instant::now();
        assert_eq!(
            handle.stop(StdDuration::from_millis(100)),
            StopOutcome::TimedOut
        );
        assert!(
            t.elapsed() < StdDuration::from_secs(1),
            "超時就放棄等待：{:?}",
            t.elapsed()
        );
        // 放棄等待之後，執行緒仍會在一輪結束後自行結束。
        assert_eq!(
            handle.stop(StdDuration::from_secs(10)),
            StopOutcome::Stopped
        );
    }

    /// 排程剛抓完、沒有待啟動的一輪：只有手動要求會讓它抓。記下每輪的觸發原因。
    struct ManualThreadEnv {
        record: FetchState,
        triggers: Arc<Mutex<Vec<Trigger>>>,
    }

    impl SchedEnv for ManualThreadEnv {
        fn now(&self) -> OffsetDateTime {
            OffsetDateTime::now_utc()
        }
        fn settings(&self) -> (PathBuf, DataFetch) {
            (PathBuf::from("D:\\fake-data"), DataFetch::On)
        }
        fn isolation(&self) -> Option<LocalAppDataMismatch> {
            None
        }
        fn data_file_exists(&self, _dir: &Path) -> bool {
            true
        }
        fn load_state(&self) -> Option<FetchState> {
            Some(self.record)
        }
        fn save_state(&self, _record: &FetchState) -> io::Result<()> {
            Ok(())
        }
        fn run_round(&self, t: Trigger, _d: &Path, _stop: &StopSignal) -> RoundEnd {
            self.triggers.lock().unwrap().push(t);
            RoundEnd::Done { ok: true }
        }
        fn log(&self, _level: Level, _text: &str) {}
    }

    #[test]
    fn request_manual_on_the_handle_starts_a_manual_round_on_the_scheduler_thread() {
        let triggers = Arc::new(Mutex::new(Vec::new()));
        let handle = spawn(
            ManualThreadEnv {
                record: ok_state(OffsetDateTime::now_utc()),
                triggers: Arc::clone(&triggers),
            },
            Box::new(Fixed(LO)),
        )
        .unwrap();
        thread::sleep(StdDuration::from_millis(100));
        assert!(triggers.lock().unwrap().is_empty(), "剛抓完：沒有排程輪");
        handle.request_manual();
        let wait_start = std::time::Instant::now();
        while triggers.lock().unwrap().is_empty() {
            assert!(
                wait_start.elapsed() < StdDuration::from_secs(5),
                "手動要求要打斷 60 秒的睡眠"
            );
            thread::sleep(StdDuration::from_millis(10));
        }
        thread::sleep(StdDuration::from_millis(100));
        assert_eq!(*triggers.lock().unwrap(), [Trigger::Manual], "只抓一輪");
        assert_eq!(handle.stop(StdDuration::from_secs(5)), StopOutcome::Stopped);
    }

    // ───────────── 一輪的真實流程（FixtureFetch，不連網） ─────────────

    fn collect_round(
        sc: Scenario,
        dir: &Path,
        trigger: Trigger,
        should_stop: &(dyn Fn() -> bool + Sync),
        events: &Mutex<Vec<String>>,
    ) -> RoundEnd {
        let mut sink = |(_, text): Line| events.lock().unwrap().push(text);
        let clock = sc.clock;
        run_scheduled_round(
            trigger,
            dir,
            move || Ok::<_, String>(sc.fetch),
            |_| {
                events.lock().unwrap().push("<等網路>".to_string());
                true
            },
            move || clock,
            should_stop,
            &mut sink,
        )
    }

    #[test]
    fn a_scheduled_round_logs_start_wait_sources_write_summary_in_order() {
        let sc = Scenario::named("recorded-20261005").unwrap();
        let dir = TempDir::new("sched-round-order");
        let events = Mutex::new(Vec::new());
        let end = collect_round(sc, dir.path(), Trigger::Slot, &|| false, &events);
        assert_eq!(end, RoundEnd::Done { ok: true });
        let events = events.lock().unwrap();
        assert!(
            events[0].starts_with("開始抓取一輪 觸發=slot"),
            "{}",
            events[0]
        );
        assert_eq!(events[1], "<等網路>");
        assert!(events[2].starts_with("抓取"));
        let n = events.len();
        assert!(events[n - 2].starts_with("已輸出 → "), "{}", events[n - 2]);
        assert!(events[n - 1].starts_with("完成："), "{}", events[n - 1]);
        assert!(dir.path().join(OUTPUT_FILE).is_file());
    }

    #[test]
    fn every_trigger_name_reaches_the_start_line() {
        for (trigger, name) in [
            (Trigger::FirstFetch, "first-fetch"),
            (Trigger::Slot, "slot"),
            (Trigger::Missed, "missed"),
            (Trigger::Retry, "retry"),
            (Trigger::Manual, "manual"),
        ] {
            let dir = TempDir::new("sched-round-trigger");
            let events = Mutex::new(Vec::new());
            collect_round(
                Scenario::named("recorded-20261005").unwrap(),
                dir.path(),
                trigger,
                &|| false,
                &events,
            );
            assert!(
                events.lock().unwrap()[0].contains(&format!("觸發={name}")),
                "{name}"
            );
        }
    }

    #[test]
    fn a_stop_request_halts_at_a_source_boundary_and_writes_no_file() {
        let sc = Scenario::named("recorded-20261005").unwrap();
        let dir = TempDir::new("sched-round-stop");
        let events = Mutex::new(Vec::new());
        let asked = AtomicUsize::new(0);
        // 開始前（等網路後）問一次，之後每個來源邊界問一次；第 4 次（＝第 3 個來源後）回 true。
        let stop = || asked.fetch_add(1, Ordering::SeqCst) + 1 >= 4;
        let end = collect_round(sc, dir.path(), Trigger::Missed, &stop, &events);
        assert_eq!(end, RoundEnd::Aborted);
        let leftovers: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            leftovers.is_empty(),
            "不得寫半份檔、也不留暫存檔：{leftovers:?}"
        );
        let events = events.lock().unwrap();
        assert!(
            events
                .iter()
                .any(|e| e.contains("已在來源邊界中止本輪，不寫檔")),
            "{events:?}"
        );
        assert!(!events.iter().any(|e| e.starts_with("完成：")));
    }

    #[test]
    fn a_stop_while_waiting_for_the_network_aborts_before_any_request() {
        let sc = Scenario::named("recorded-20261005").unwrap();
        let dir = TempDir::new("sched-round-stop-net");
        let events = Mutex::new(Vec::new());
        let end = collect_round(sc, dir.path(), Trigger::Missed, &|| true, &events);
        assert_eq!(end, RoundEnd::Aborted);
        assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());
    }

    #[test]
    fn a_failing_http_client_or_directory_is_a_failed_round_not_a_panic() {
        let sc = Scenario::named("recorded-20261005").unwrap();
        let dir = TempDir::new("sched-round-setup");
        let mut lines = Vec::new();
        let end = run_scheduled_round(
            Trigger::Missed,
            dir.path(),
            || Err::<crate::fetch::fixture::FixtureFetch, _>("TLS 初始化失敗".to_string()),
            |_| panic!("client 建不起來不該等網路"),
            || sc.clock,
            &|| false,
            &mut |l| lines.push(l),
        );
        assert_eq!(end, RoundEnd::Done { ok: false });
        assert!(lines
            .last()
            .unwrap()
            .1
            .contains("建立 HTTP client 失敗：TLS 初始化失敗"));

        // 「目錄」其實是普通檔案：建不起來。
        let file = dir.path().join("im-a-file");
        std::fs::write(&file, "x").unwrap();
        let mut lines = Vec::new();
        let sc2 = Scenario::named("recorded-20261005").unwrap();
        let end = run_scheduled_round(
            Trigger::Missed,
            &file,
            move || Ok::<_, String>(sc2.fetch),
            |_| panic!("目錄建不起來不該等網路"),
            || sc.clock,
            &|| false,
            &mut |l| lines.push(l),
        );
        assert_eq!(end, RoundEnd::Done { ok: false });
        assert!(lines.last().unwrap().1.contains("建立失敗"));
    }

    #[test]
    fn a_round_where_every_source_fails_is_a_failed_round_even_though_a_file_is_written() {
        let sc = Scenario::named("all-sources-fail-with-old").unwrap();
        let dir = TempDir::new("sched-round-allfail");
        sc.install_old_into(dir.path()).unwrap();
        let events = Mutex::new(Vec::new());
        let end = collect_round(sc, dir.path(), Trigger::Slot, &|| false, &events);
        assert_eq!(
            end,
            RoundEnd::Done { ok: false },
            "沿用舊資料的一輪不算成功（要退避重試）"
        );
        assert!(
            dir.path().join(OUTPUT_FILE).is_file(),
            "仍寫出沿用的舊資料（K-15）"
        );
    }
}
