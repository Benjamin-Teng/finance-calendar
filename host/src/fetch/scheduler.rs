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

/// 停止旗標＋可被打斷的睡眠。
#[derive(Debug, Default)]
pub struct StopSignal {
    stopped: AtomicBool,
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

    /// 睡 `timeout`，被 [`StopSignal::request`] 喚醒就提早返回。回傳是否已要求停止。
    pub fn wait(&self, timeout: StdDuration) -> bool {
        let guard = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = self
            .wake
            .wait_timeout_while(guard, timeout, |_| !self.is_stopped())
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
    if gate != Gate::Enabled {
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

    match ls.sched.step(now, record.as_ref(), exists, rng) {
        Step::Sleep(d) => Tick::Sleep(d.unsigned_abs()),
        Step::Start(trigger) => {
            // 一輪抓取另包一層：panic 算該輪失敗（記進排程紀錄走退避），否則決定性的 panic 會讓每次醒來都重來。
            let end = catch_unwind(AssertUnwindSafe(|| env.run_round(trigger, &dir, stop)))
                .unwrap_or_else(|payload| {
                    env.log(
                        Level::Error,
                        &format!(
                            "抓取排程：一輪抓取 panic（{}），記為失敗",
                            panic_message(payload.as_ref())
                        ),
                    );
                    RoundEnd::Done { ok: false }
                });
            let finished = env.now();
            match end {
                RoundEnd::Aborted => {
                    // 中止的一輪沒有「跑完」：不設首抓抑制（審查 N2），也不更新紀錄。
                    ls.sched.idle_wake(finished);
                    if stop.is_stopped() {
                        Tick::Stop
                    } else {
                        // 抓取在輪中被關掉：下一次醒來照常（會看到閘門已關）。
                        Tick::Sleep(StdDuration::ZERO)
                    }
                }
                RoundEnd::Done { ok } => {
                    // 跑完後資料檔還不存在＝寫檔失敗：之後不首抓、走重試退避（熱迴圈防護，見 `sched` 模組文件）。
                    ls.sched
                        .round_finished(finished, env.data_file_exists(&dir));
                    let new_state = FetchState::after_round(record.as_ref(), now, finished, ok);
                    ls.memory = Some(new_state);
                    save_record(env, ls, &new_state);
                    Tick::Sleep(StdDuration::ZERO)
                }
            }
        }
    }
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
            if self.fail_saves.get() {
                return Err(io::Error::other("模擬磁碟已滿"));
            }
            self.saved.borrow_mut().push(*record);
            *self.state.borrow_mut() = Some(*record);
            Ok(())
        }
        fn run_round(&self, trigger: Trigger, _dir: &Path, _stop: &StopSignal) -> RoundEnd {
            self.rounds.borrow_mut().push((self.now.get(), trigger));
            assert!(!self.panic_in_round.get(), "一輪炸了");
            if self.abort_by_turning_off.replace(false) {
                self.mode.set(DataFetch::Off);
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

    // ───────────── StopSignal ─────────────

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
