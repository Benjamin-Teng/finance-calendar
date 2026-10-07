//! `fc-host.exe --restore-wallpaper`（dynamic-wallpaper task 4.7b 裁決 5；spec「還原原桌布」Scenario
//! 「安裝檔呼叫還原指令」）：還原原桌布後結束，不建立任何小工具視窗、不啟動協調迴圈的排程。
//!
//! `main()` 在記錄檔初始化之後、啟動仲裁與 `tauri::Builder` 之前攔下這個旗標，整個行程交給
//! [`run`]，以結束碼回報結果（[`RestoreCode::exit_code`]）。
//!
//! ## 給安裝檔：結束碼、耗時與執行身分
//!
//! | 碼 | 意義 |
//! |---|---|
//! | 0 | 全部還原，或沒有需要還原的桌布（含找不到狀態檔——記錄會寫出實際讀的路徑與使用者） |
//! | 1 | 還原失敗或狀態檔暫時讀不到（狀態檔保留，可重試）；或桌布已還原但主題設定存檔失敗 |
//! | 2 | 狀態檔讀不懂（封鎖），沒有動任何東西 |
//! | 3 | 交給執行中的宿主後時限內沒有回報（宿主可能之後仍會完成） |
//! | 4 | 有宿主在執行卻交接不了，時限內也沒有結束 |
//! | 5 | 本行程內部錯誤（無法建立桌布 COM 執行緒或還原執行緒；還原工作執行緒 panic——記錄檔有 `fc_host::panic` 一行） |
//! | 6 | 已還原所有在線螢幕、仍有離線（或讀不到）的螢幕待還原；**安裝檔可視為完成並提示**（宿主之後再執行、螢幕接上時才會補做） |
//! | 7 | 超過命令列總時限 [`CLI_TOTAL_LIMIT`]（本行程還原做到一半時，狀態檔已有「還原進行中」標記） |
//!
//! **上表以外的任何值都代表失敗**：例如 101＝還原工作執行緒以外的 panic（記錄檔有 `fc_host::panic` 一行；
//! 還原本身在工作執行緒上做，那裡的 panic 以 5 回報）、被外部終止的碼。
//!
//! 耗時：整個命令列只有一個總時限 [`CLI_TOTAL_LIMIT`]（90 秒，硬上限）。每一段的時限都是「該段上限」與
//! 「剩下的預算」取小：
//!
//! - 等啟動鎖：[`STARTUP_LOCK_WAIT`] 30 秒（只有宿主卡在啟動中才會等滿）；
//! - 送出交接：[`HANDOFF_SEND_TIMEOUT`] 5 秒；
//! - 等宿主回報：[`HANDOFF_REPLY_WAIT`] 30 秒；
//! - 等宿主結束：[`PRIMARY_EXIT_WAIT`] 10 秒（每次探測都是不等待的試取執行個體鎖）；
//! - 本行程還原：在工作執行緒上做，只等剩下的預算（各段上限相加 75 秒，所以至少還有 15 秒），等不到就
//!   以 7 結束；工作執行緒沒回報就結束（panic）以 5 結束，不算逾時。
//!
//! 正常情況（沒有宿主，或宿主正常回應）是數秒。
//!
//! ### 安裝檔應該怎麼做
//!
//! 1. **等待上限設 [`INSTALLER_WAIT_RECOMMENDED`]（120 秒）以上**：總時限 90 秒，再加行程啟動與記錄落盤的
//!    餘裕。
//! 2. **逾時不要強制終止行程**（`TerminateProcess`／`taskkill /F`）：它一定會在總時限內自己結束。強制終止
//!    若發生在本行程還原中途，只能靠狀態檔的標記等宿主下次執行才續做，而解除安裝情境不會再有下一次。
//! 3. 依結束碼處置：
//!    - **0**：完成。
//!    - **3**（宿主沒有回報）：宿主仍在處理或卡住。等 `fc-host.exe` 結束（或請使用者從系統匣「結束」）
//!      後重跑一次。
//!    - **4**（交接不了）：宿主在執行卻收不到要求，或它還沒準備好（剛啟動的頭一兩秒也會這樣）。先結束
//!      `fc-host.exe` 再重跑；剛啟動的情況等幾秒重跑即可。
//!    - **6**（部分還原）：在線的螢幕都已還原，**可視為完成**，並提示使用者：離線的螢幕要宿主再執行、且
//!      螢幕接上時才會補做——解除安裝前可接上那些螢幕後重跑；接不上的螢幕（例如出差時接過、之後不再
//!      接的投影機）會停在宿主的圖，要使用者自己在 Windows 設定換掉。程式不替待還原紀錄設時限：之後每次
//!      執行都會再回 6（task 6.4 裁定：不改程式，安裝檔依此表處理）。
//!    - **7**（超過總時限）：重跑一次。
//!    - **1、2、5、其他**：記錄檔（`%LOCALAPPDATA%\tw.fintools.fc-host\logs`，target `fc_host::wallpaper_cli`）
//!      寫了原因；可重跑一次，仍失敗就提示使用者自行換桌布。
//!
//! 執行身分：狀態檔、設定檔在 `%APPDATA%`、桌布與登錄在目前使用者——要以**接管過桌布的那個使用者
//! 本人、不提權**執行。以其他帳號或提權執行會讀到別的設定檔，通常得到 0（找不到狀態檔），記錄會寫出
//! 實際讀的路徑與使用者名稱。命令列在本行程還原期間持有執行個體鎖，這時啟動的宿主會安全退出：
//! 命令列結束後再啟動宿主。
//!
//! ## 流程（[`run_with`]，可注入的決策；正式的 Win32／檔案操作在 [`RealOps`]）
//!
//! 1. 啟動仲裁（`desktop::arbitrate_startup_within`，與宿主同一組具名 mutex）：
//!    - 取得執行個體鎖＝沒有宿主在跑 → 在本行程直接還原
//!      （`wallpaper_coordinator::restore_for_command_line`：主題改「不接管」並存檔、讀狀態檔、
//!      以 `CommandLine` 還原）。持有執行個體鎖到行程結束：這段期間啟動的宿主會是 Secondary、安全退出，
//!      不會兩邊同時動狀態檔。
//!    - 已有宿主 → 經既有的 single-instance 通道交接（`desktop::handoff`），宿主的桌布協調迴圈把主題
//!      改「不接管」並存檔、還原，以具名事件回報；等 [`HANDOFF_REPLY_WAIT`]。沒有回報＝結束碼 3
//!      （不自己再還原：宿主可能還在動狀態檔）。找不到交接視窗、送不出去（宿主正在結束），或宿主回報
//!      「交接失敗」（它沒有協調迴圈、迴圈已關閉——含系統匣結束已還原完的情況）→ 等宿主的執行個體鎖
//!      （[`PRIMARY_EXIT_WAIT`]），等到就在本行程還原（主題存成「不接管」；已還原過就是「沒有需要還原」
//!      ＝0），等不到＝結束碼 4。
//!    - 仲裁機制故障（`Unarbitrated`）→ 先試交接；找不到交接視窗才在本行程還原；宿主回報「交接失敗」時
//!      同上（等它結束後在本行程還原）。
//!    - 等啟動鎖逾時（有宿主卡在啟動中）→ 先試交接；找不到交接視窗或送不出去＝4；宿主回報「交接失敗」時
//!      同上（等它結束後在本行程還原）。
//! 2. 每一步都記一行（target [`LOG_TARGET`]，寫進宿主同一個每日記錄檔）。
//!
//! 宿主端（`main.rs` 的 single-instance 回呼）以 [`handoff_request`] 認出交接、以 [`accept_handoff`]
//! 投遞給協調迴圈；回覆碼與事件序號的對應見 [`REPLY_CODES`]。

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use log::Level;
use tauri::{AppHandle, Manager};

use crate::desktop;
use crate::wallpaper_coordinator::{
    restore_for_command_line, CommandLineParts, CoordinatorHandle, Request, RequestProgress,
    RestoreCode, RestoreReport,
};

/// 命令列旗標。
pub const RESTORE_FLAG: &str = "--restore-wallpaper";
/// 交接參數：命令列行程的 PID（宿主據此找回覆事件）。
pub const REPLY_PID_FLAG: &str = "--reply-pid";
/// 記錄的 target。
pub const LOG_TARGET: &str = "fc_host::wallpaper_cli";
/// 交接後等宿主回報的上限（裁決 5：如 30 秒）。
pub const HANDOFF_REPLY_WAIT: Duration = Duration::from_secs(30);
/// 送出交接訊息的上限（宿主主執行緒卡住時不永遠等）。
pub const HANDOFF_SEND_TIMEOUT: Duration = Duration::from_secs(5);
/// 宿主在但交接不了時，等它結束（取得執行個體鎖）的上限。
pub const PRIMARY_EXIT_WAIT: Duration = Duration::from_secs(10);
/// 等啟動鎖的上限（與宿主的 `desktop::instance::STARTUP_WAIT` 相同）。
pub const STARTUP_LOCK_WAIT: Duration = Duration::from_secs(30);
/// 整個命令列的總時限（複審 N3）：各段上限相加 75 秒，本行程還原至少還有 15 秒。
pub const CLI_TOTAL_LIMIT: Duration = Duration::from_secs(90);
/// 給安裝檔的建議等待上限（總時限加上行程啟動、記錄落盤的餘裕）。
pub const INSTALLER_WAIT_RECOMMENDED: Duration = Duration::from_secs(120);

/// 宿主能回報的結果碼；序號＝回覆事件的序號。
pub const REPLY_CODES: [RestoreCode; 5] = [
    RestoreCode::Restored,
    RestoreCode::Failed,
    RestoreCode::Blocked,
    RestoreCode::HandoffFailed,
    RestoreCode::PartiallyRestored,
];
/// 宿主端：寫「還原進行中」標記時取共用鎖的上限（審查 F6）。
pub const HANDOFF_LOCK_WAIT: Duration = Duration::from_secs(5);
/// 等宿主結束時兩次探測之間的間隔。
pub const PRIMARY_EXIT_POLL: Duration = Duration::from_millis(200);

/// 每隔 `poll` 探測一次、最多等 `cap`（含）：`probe` 為真回 `true`。探測本身必須不等待，總時間
/// 才是硬上限（審查 F5）。時鐘與睡眠可注入（測試）。
pub fn wait_until(
    cap: Duration,
    poll: Duration,
    mut probe: impl FnMut() -> bool,
    now: impl Fn() -> Duration,
    mut sleep: impl FnMut(Duration),
) -> bool {
    let start = now();
    loop {
        if probe() {
            return true;
        }
        let elapsed = now().saturating_sub(start);
        if elapsed >= cap {
            return false;
        }
        sleep(poll.min(cap - elapsed));
    }
}

/// 結果碼 → 回覆事件序號（命令列端才會產生的碼沒有序號）。
pub fn reply_slot(code: RestoreCode) -> Option<usize> {
    REPLY_CODES.iter().position(|c| *c == code)
}

/// 本行程的命令列（第一個是執行檔路徑）是否要求 `--restore-wallpaper`。
pub fn requested(args: &[String]) -> bool {
    args.iter().skip(1).any(|a| a == RESTORE_FLAG)
}

/// 交接給宿主的參數（第一個是執行檔路徑，與 plugin 送的 `std::env::args()` 同形）。
pub fn handoff_args(exe: &str, reply_pid: u32) -> Vec<String> {
    vec![
        exe.to_owned(),
        RESTORE_FLAG.to_owned(),
        REPLY_PID_FLAG.to_owned(),
        reply_pid.to_string(),
    ]
}

/// 宿主端：single-instance 回呼收到的參數是不是命令列還原的交接。`Some(Some(pid))`＝要回報給
/// `pid`；`Some(None)`＝是還原要求但沒有回覆對象；`None`＝不是。
pub fn handoff_request(args: &[String]) -> Option<Option<u32>> {
    if !requested(args) {
        return None;
    }
    let pid = args
        .iter()
        .skip_while(|a| *a != REPLY_PID_FLAG)
        .nth(1)
        .and_then(|v| v.parse().ok());
    Some(pid)
}

/// 啟動仲裁的結果（命令列的觀點）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arbitration {
    /// 取得執行個體鎖：沒有宿主在跑。
    NoInstance,
    /// 已有宿主。
    InstanceRunning,
    /// 仲裁機制故障。
    Unarbitrated,
    /// 等啟動鎖逾時（有宿主卡在啟動中）。
    StartupLockTimeout,
}

/// 交接的結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandoffOutcome {
    /// 宿主回報了結果。
    Replied(RestoreCode),
    /// 送出了，但宿主在時限內沒有回報。
    NoReply,
    /// 找不到交接視窗。
    NoWindow,
    /// 送不出去（宿主主執行緒卡住、或其他錯誤）。
    SendFailed(String),
}

/// [`run_with`] 的依賴。每段都帶自己的時限（該段上限與剩下的總預算取小；複審 N3）。
pub trait CliOps {
    /// 命令列開始至今的時間（總時限的時鐘）。
    fn elapsed(&self) -> Duration;
    /// 啟動仲裁，等啟動鎖最多 `wait`。
    fn arbitrate(&mut self, wait: Duration) -> Arbitration;
    /// 交接：送出最多 `send`，等回報最多 `reply`。
    fn hand_off(&mut self, send: Duration, reply: Duration) -> HandoffOutcome;
    /// 等宿主結束（取得執行個體鎖），最多 `cap`；取得回 `true`。
    fn wait_for_primary_exit(&mut self, cap: Duration) -> bool;
    /// 本行程還原，最多 `cap`。
    fn restore_locally(&mut self, cap: Duration) -> LocalRestore;
}

/// 本行程還原的結果（[`CliOps::restore_locally`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalRestore {
    /// 做完了（含失敗的結果分類）。
    Finished(RestoreReport),
    /// 時限內沒做完（結束碼 7）。
    TimedOut,
    /// 還原工作執行緒沒有回報就結束了（panic；結束碼 5）。
    WorkerDied,
}

/// 等還原工作執行緒的回報，最多 `cap`：逾時與「執行緒已結束卻沒回報」（送出端隨 panic 展開被丟掉）
/// 要分開——後者不是逾時（4.7b 複審 L1）。
fn await_restore_worker(
    rx: &std::sync::mpsc::Receiver<RestoreReport>,
    cap: Duration,
) -> LocalRestore {
    match rx.recv_timeout(cap) {
        Ok(report) => LocalRestore::Finished(report),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => LocalRestore::TimedOut,
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => LocalRestore::WorkerDied,
    }
}

/// 剩下的總預算。
fn budget_left(ops: &dyn CliOps) -> Duration {
    CLI_TOTAL_LIMIT.saturating_sub(ops.elapsed())
}

/// 本行程還原（只拿剩下的總預算）。
fn restore_here(ops: &mut dyn CliOps, log: &dyn Fn(Level, &str), why: &str) -> RestoreCode {
    let cap = budget_left(ops);
    if cap.is_zero() {
        log(
            Level::Error,
            "--restore-wallpaper：總時限已用完，不在本行程還原（結束碼 7）",
        );
        return RestoreCode::TimedOut;
    }
    log(
        Level::Info,
        &format!(
            "--restore-wallpaper：{why}，在本行程直接還原（剩下的總時限 {} 秒）",
            cap.as_secs()
        ),
    );
    match ops.restore_locally(cap) {
        LocalRestore::Finished(report) => report.code,
        LocalRestore::WorkerDied => {
            log(
                Level::Error,
                "--restore-wallpaper：本行程還原的工作執行緒異常結束、沒有回報結果（panic，原因見記錄檔 fc_host::panic 一行；結束碼 5）；寫到一半的檔案都是原子寫入，可再執行一次",
            );
            RestoreCode::Internal
        }
        LocalRestore::TimedOut => {
            log(
                Level::Error,
                &format!(
                    "--restore-wallpaper：本行程還原在總時限 {} 秒內沒做完（結束碼 7）；狀態檔已有「還原進行中」標記，可再執行一次",
                    CLI_TOTAL_LIMIT.as_secs()
                ),
            );
            RestoreCode::TimedOut
        }
    }
}

/// 命令列的決策（見模組文件「流程」）。整體不超過 [`CLI_TOTAL_LIMIT`]：每段時限＝該段上限與剩下的
/// 預算取小，本行程還原只拿剩下的預算。
pub fn run_with(ops: &mut dyn CliOps, log: &dyn Fn(Level, &str)) -> RestoreCode {
    let arbitration = ops.arbitrate(STARTUP_LOCK_WAIT.min(budget_left(ops)));
    log(
        Level::Info,
        &format!("--restore-wallpaper：啟動仲裁 → {arbitration:?}"),
    );
    if arbitration == Arbitration::NoInstance {
        return restore_here(ops, log, "沒有執行中的宿主");
    }
    let budget = budget_left(ops);
    if budget.is_zero() {
        log(
            Level::Error,
            "--restore-wallpaper：總時限已用完，不交接（結束碼 7）",
        );
        return RestoreCode::TimedOut;
    }
    let send = HANDOFF_SEND_TIMEOUT.min(budget);
    let reply = HANDOFF_REPLY_WAIT.min(budget - send);
    log(
        Level::Info,
        &format!(
            "--restore-wallpaper：交給執行中的宿主（single-instance 通道），等它回報（最多 {} 秒）",
            reply.as_secs()
        ),
    );
    match ops.hand_off(send, reply) {
        HandoffOutcome::Replied(RestoreCode::HandoffFailed) => {
            // 審查 F3：宿主收到了卻沒有協調迴圈（啟動失敗、還沒建立、或迴圈已關閉）。
            log(
                Level::Warn,
                "--restore-wallpaper：執行中的宿主回報「交接失敗」（它沒有可用的桌布協調迴圈），等它結束後在本行程還原",
            );
            wait_then_local(ops, log)
        }
        HandoffOutcome::Replied(code) => {
            log(
                if code == RestoreCode::Restored {
                    Level::Info
                } else {
                    Level::Warn
                },
                &format!(
                    "--restore-wallpaper：執行中的宿主回報 {}（結束碼 {}）",
                    code.label(),
                    code.exit_code()
                ),
            );
            code
        }
        HandoffOutcome::NoReply => {
            log(
                Level::Error,
                &format!(
                    "--restore-wallpaper：執行中的宿主 {} 秒內沒有回報（結束碼 {}）；不在本行程還原（宿主可能仍在處理）",
                    reply.as_secs(),
                    RestoreCode::NoReply.exit_code()
                ),
            );
            RestoreCode::NoReply
        }
        failed @ (HandoffOutcome::NoWindow | HandoffOutcome::SendFailed(_)) => {
            log(
                Level::Warn,
                &format!("--restore-wallpaper：交接失敗（{failed:?}）"),
            );
            match arbitration {
                Arbitration::Unarbitrated if failed == HandoffOutcome::NoWindow => restore_here(
                    ops,
                    log,
                    "仲裁機制故障且找不到交接視窗（視為沒有執行中的宿主）",
                ),
                Arbitration::InstanceRunning => wait_then_local(ops, log),
                _ => {
                    log(
                        Level::Error,
                        &format!(
                            "--restore-wallpaper：無法交接、也無法確定沒有宿主在執行；不還原（結束碼 {}）",
                            RestoreCode::HandoffFailed.exit_code()
                        ),
                    );
                    RestoreCode::HandoffFailed
                }
            }
        }
    }
}

/// 等宿主結束（硬上限：[`PRIMARY_EXIT_WAIT`] 與剩下的預算取小），等到就在本行程還原，等不到＝4。
fn wait_then_local(ops: &mut dyn CliOps, log: &dyn Fn(Level, &str)) -> RestoreCode {
    let cap = PRIMARY_EXIT_WAIT.min(budget_left(ops));
    if ops.wait_for_primary_exit(cap) {
        restore_here(ops, log, "宿主已結束（取得執行個體鎖）")
    } else {
        log(
            Level::Error,
            &format!(
                "--restore-wallpaper：宿主仍在執行卻無法交接，{} 秒內也沒有結束；不在本行程還原（結束碼 {}）",
                cap.as_secs(),
                RestoreCode::HandoffFailed.exit_code()
            ),
        );
        RestoreCode::HandoffFailed
    }
}

/// 正式的命令列：回傳行程結束碼。
pub fn run() -> i32 {
    log::info!(
        target: LOG_TARGET,
        "--restore-wallpaper：開始（pid={}；總時限 {} 秒，安裝檔建議等待 {} 秒）",
        std::process::id(),
        CLI_TOTAL_LIMIT.as_secs(),
        INSTALLER_WAIT_RECOMMENDED.as_secs()
    );
    let mut ops = RealOps {
        started: Instant::now(),
        arbitration: None,
        instance: None,
    };
    let code = run_with(&mut ops, &|level, msg| {
        log::log!(target: LOG_TARGET, level, "{msg}");
    });
    log::info!(
        target: LOG_TARGET,
        "--restore-wallpaper：結束（{}，結束碼 {}）",
        code.label(),
        code.exit_code()
    );
    log::logger().flush();
    code.exit_code()
}

/// 正式的依賴：Win32 具名 mutex／交接視窗／具名事件、真正的 COM 服務與 HKCU。
struct RealOps {
    /// 命令列開始的時刻（總時限的起點）。
    started: Instant,
    /// 持有到行程結束（執行個體鎖）。
    arbitration: Option<desktop::StartupArbitration>,
    /// 等宿主結束時取得的執行個體鎖（持有到行程結束）。
    instance: Option<desktop::InstanceLock>,
}

impl CliOps for RealOps {
    fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    fn arbitrate(&mut self, wait: Duration) -> Arbitration {
        let Some(arb) = desktop::arbitrate_startup_within(wait) else {
            return Arbitration::StartupLockTimeout;
        };
        // 命令列不需要啟動臨界區：立刻放開啟動鎖，不擋住同時啟動的宿主。
        desktop::release_startup_lock();
        let role = arb.role();
        self.arbitration = Some(arb);
        match role {
            desktop::StartupRole::Primary => Arbitration::NoInstance,
            desktop::StartupRole::Secondary => Arbitration::InstanceRunning,
            desktop::StartupRole::Unarbitrated => Arbitration::Unarbitrated,
        }
    }

    fn hand_off(&mut self, send: Duration, reply: Duration) -> HandoffOutcome {
        let pid = std::process::id();
        let events = match desktop::handoff::ReplyEvents::create(pid, REPLY_CODES.len()) {
            Ok(e) => e,
            Err(e) => return HandoffOutcome::SendFailed(e),
        };
        let exe = std::env::args().next().unwrap_or_default();
        match desktop::handoff::send_handoff(&handoff_args(&exe, pid), send) {
            Ok(()) => {}
            Err(desktop::handoff::SendError::NoWindow) => return HandoffOutcome::NoWindow,
            Err(desktop::handoff::SendError::Failed(e)) => return HandoffOutcome::SendFailed(e),
        }
        match events.wait(reply) {
            Some(slot) => HandoffOutcome::Replied(REPLY_CODES[slot]),
            None => HandoffOutcome::NoReply,
        }
    }

    fn wait_for_primary_exit(&mut self, cap: Duration) -> bool {
        // 審查 F5：只試取執行個體鎖（不等待、不碰啟動鎖），總時間是硬上限。
        let origin = Instant::now();
        let mut acquired = None;
        let exited = wait_until(
            cap,
            PRIMARY_EXIT_POLL,
            || {
                acquired = desktop::try_acquire_instance_lock();
                acquired.is_some()
            },
            || origin.elapsed(),
            std::thread::sleep,
        );
        self.instance = acquired;
        exited
    }

    fn restore_locally(&mut self, cap: Duration) -> LocalRestore {
        // 複審 N3：還原在工作執行緒上做，本執行緒最多等 `cap`。逾時時行程隨即結束（工作執行緒隨之
        // 消失）：狀態機在第一個桌布 API 呼叫之前就寫了「還原進行中」標記，與被終止相同，可再執行。
        // 工作執行緒 panic 時送出端隨展開被丟掉，等待立即以 WorkerDied 返回（4.7b 複審 L1）。
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("fc-restore-cli".to_owned())
            .spawn(move || {
                let _ = tx.send(restore_in_this_process());
            });
        if let Err(e) = spawned {
            return LocalRestore::Finished(RestoreReport::new(
                RestoreCode::Internal,
                format!("無法建立還原執行緒：{e}"),
            ));
        }
        await_restore_worker(&rx, cap)
    }
}

/// 本行程還原（正式依賴）。
fn restore_in_this_process() -> RestoreReport {
    let service = match crate::desktop::wallpaper::WallpaperService::spawn_com(
        crate::desktop::wallpaper::WallpaperServiceConfig::default(),
    ) {
        Ok(s) => s,
        Err(e) => {
            return RestoreReport::new(
                RestoreCode::Internal,
                format!("無法建立桌布 COM 執行緒：{e}"),
            )
        }
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
    restore_for_command_line(CommandLineParts {
        service,
        registry: Box::new(crate::desktop::wallpaper::registry::HkcuRegistry),
        files: Box::new(crate::desktop::wallpaper::file_identity::Win32FileIdentity),
        paths: crate::wallpaper_state::default_paths(),
        settings_path: crate::settings::default_settings_path(),
        takeover: crate::wallpaper_state::TakeoverConfig::default(),
        now,
        log: std::sync::Arc::new(|level, msg: &str| {
            log::log!(target: LOG_TARGET, level, "{msg}");
        }),
    })
}

/// 宿主端：single-instance 回呼收到命令列的交接（在交接視窗的同步 WndProc 裡呼叫——只投遞、
/// 立即返回）。協調迴圈處理完以回覆事件回報；協調迴圈不在（`--self-test-render`、啟動失敗、還沒建立）
/// 或要求沒被處理就被丟棄時，回報「交接失敗」，命令列改等宿主結束後自己還原。
pub fn accept_handoff(app: &AppHandle, reply_pid: Option<u32>) {
    log::info!(
        target: LOG_TARGET,
        "single-instance：收到 --restore-wallpaper 交接（回覆對象 pid={reply_pid:?}），交給桌布協調迴圈"
    );
    route_handoff(
        app.try_state::<CoordinatorHandle>()
            .map(|h| h.inner().clone()),
        ReplyTo::new(reply_pid),
    );
}

/// 宿主端：更新過渡期（讀到舊標記、小工具與協調迴圈尚未建立）中收到交接（installer-auto-update 最終審查 M2a）。
/// 不交給協調迴圈（它還不存在），立即回報「交接失敗」；呼叫端接著以 `updater::quit_during_transition` 結束宿主
/// （不還原、不存主題、刪標記）。命令列收到「交接失敗」後走「等宿主結束（[`PRIMARY_EXIT_WAIT`]）→ 在本行程還原」，
/// 整段仍受 [`CLI_TOTAL_LIMIT`] 約束。只做回報（設定具名事件），在同步 WndProc 裡呼叫也不會卡住發送端。
pub fn refuse_handoff_during_update_transition(reply_pid: Option<u32>) {
    log::info!(
        target: LOG_TARGET,
        "single-instance：更新過渡期中收到 --restore-wallpaper 交接（回覆對象 pid={reply_pid:?}）：回報「交接失敗」並結束宿主，由命令列在自己的行程還原"
    );
    refuse_handoff(ReplyTo::new(reply_pid));
}

/// [`refuse_handoff_during_update_transition`] 的本體（可測試）。
fn refuse_handoff(mut reply: ReplyTo) {
    reply.send(RestoreCode::HandoffFailed);
}

/// [`accept_handoff`] 的本體（可測試）：沒有協調迴圈就立即回報「交接失敗」；有就開執行緒先寫
/// 「還原進行中」標記（一次小檔讀寫，不在同步 WndProc 裡做；修正輪 1）再投遞。
fn route_handoff(handle: Option<CoordinatorHandle>, mut reply: ReplyTo) {
    let Some(handle) = handle else {
        log::warn!(
            target: LOG_TARGET,
            "桌布協調迴圈不在，無法處理 --restore-wallpaper 交接"
        );
        reply.send(RestoreCode::HandoffFailed);
        return;
    };
    let spawned = std::thread::Builder::new()
        .name("fc-restore-handoff".to_owned())
        .spawn(move || {
            handle.request_restore_within(
                Request::CommandLineRestore(Box::new(move |p| {
                    if let RequestProgress::Finished(report) = p {
                        reply.send(report.code);
                    }
                })),
                HANDOFF_LOCK_WAIT,
            );
        });
    if let Err(e) = spawned {
        // 閉包連同 `reply` 一起被丟棄：`ReplyTo` 的 Drop 回報「交接失敗」。
        log::error!(
            target: LOG_TARGET,
            "無法建立交接執行緒（{e}），--restore-wallpaper 交接失敗"
        );
    }
}

/// 設定命令列行程的回覆事件（正式：`desktop::handoff::signal_reply`；測試：記錄）。
type SignalFn = Box<dyn Fn(u32, usize) -> Result<(), String> + Send>;

/// 回覆命令列；沒回覆就被丟棄（要求沒被處理）時回報「交接失敗」。只回報一次。
struct ReplyTo {
    pid: Option<u32>,
    sent: bool,
    signal: SignalFn,
}

impl ReplyTo {
    fn new(pid: Option<u32>) -> Self {
        Self::with_signal(pid, Box::new(desktop::handoff::signal_reply))
    }

    fn with_signal(pid: Option<u32>, signal: SignalFn) -> Self {
        Self {
            pid,
            sent: false,
            signal,
        }
    }

    fn send(&mut self, code: RestoreCode) {
        if self.sent {
            return;
        }
        self.sent = true;
        let Some(pid) = self.pid else {
            return;
        };
        let slot = reply_slot(code).unwrap_or(1);
        match (self.signal)(pid, slot) {
            Ok(()) => log::info!(
                target: LOG_TARGET,
                "已回報 --restore-wallpaper 結果給 pid={pid}：{}（結束碼 {}）",
                code.label(),
                code.exit_code()
            ),
            Err(e) => log::warn!(
                target: LOG_TARGET,
                "回報 --restore-wallpaper 結果給 pid={pid} 失敗（命令列可能已逾時結束）：{e}"
            ),
        }
    }
}

impl Drop for ReplyTo {
    fn drop(&mut self) {
        if !self.sent {
            self.send(RestoreCode::HandoffFailed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::cell::RefCell;

    use crate::wallpaper_coordinator::{RestoreCode, RestoreReport};

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn restore_flag_is_recognised_only_after_the_executable() {
        assert!(requested(&args(&["fc-host.exe", "--restore-wallpaper"])));
        assert!(!requested(&args(&["fc-host.exe"])));
        assert!(
            !requested(&args(&["--restore-wallpaper"])),
            "第一個是執行檔路徑"
        );
        assert!(!requested(&args(&["fc-host.exe", "--autostart"])));
    }

    #[test]
    fn handoff_arguments_round_trip_through_the_primary_callback() {
        let sent = handoff_args("C:\\fc\\fc-host.exe", 4242);
        assert_eq!(handoff_request(&sent), Some(Some(4242)));
        assert_eq!(
            handoff_request(&args(&["fc-host.exe", "--restore-wallpaper"])),
            Some(None),
            "沒帶回覆對象仍是還原要求"
        );
        assert_eq!(
            handoff_request(&args(&["fc-host.exe", "--autostart"])),
            None
        );
        assert_eq!(handoff_request(&args(&["fc-host.exe"])), None);
        // 交接參數不得被認成自動化的重複啟動（那條路會被靜默忽略）。
        assert!(!crate::tray::is_automated_relaunch(&sent));
    }

    #[test]
    fn reply_slots_cover_every_code_the_primary_can_send() {
        for code in [
            RestoreCode::Restored,
            RestoreCode::Failed,
            RestoreCode::Blocked,
            RestoreCode::HandoffFailed,
            RestoreCode::PartiallyRestored,
        ] {
            let slot = reply_slot(code).expect("每個回覆碼都有自己的事件");
            assert_eq!(REPLY_CODES[slot], code);
        }
        assert_eq!(
            reply_slot(RestoreCode::NoReply),
            None,
            "NoReply 只在命令列端產生"
        );
    }

    /// 假的命令列環境：記下呼叫順序、每段拿到的時限，並以假時鐘模擬每段耗時。
    struct FakeOps {
        arbitration: Arbitration,
        handoff: Vec<HandoffOutcome>,
        primary_exits: bool,
        local: RestoreCode,
        calls: RefCell<Vec<&'static str>>,
        /// 假時鐘（自命令列開始）。
        clock: std::cell::Cell<Duration>,
        /// 仲裁、交接各自耗費的時間。
        arbitrate_takes: Duration,
        hand_off_takes: Duration,
        /// 本行程還原需要的時間（超過拿到的時限＝逾時）。
        local_takes: Duration,
        /// 本行程還原的工作執行緒 panic（沒有回報就結束）。
        local_dies: bool,
        /// 每段拿到的時限。
        caps: RefCell<Vec<(&'static str, Duration)>>,
    }

    impl FakeOps {
        fn new(arbitration: Arbitration) -> Self {
            Self {
                arbitration,
                handoff: Vec::new(),
                primary_exits: false,
                local: RestoreCode::Restored,
                calls: RefCell::new(Vec::new()),
                clock: std::cell::Cell::new(Duration::ZERO),
                arbitrate_takes: Duration::ZERO,
                hand_off_takes: Duration::ZERO,
                local_takes: Duration::ZERO,
                local_dies: false,
                caps: RefCell::new(Vec::new()),
            }
        }

        fn advance(&self, d: Duration) {
            self.clock.set(self.clock.get() + d);
        }
    }

    impl CliOps for FakeOps {
        fn elapsed(&self) -> Duration {
            self.clock.get()
        }
        fn arbitrate(&mut self, wait: Duration) -> Arbitration {
            self.calls.borrow_mut().push("arbitrate");
            self.caps.borrow_mut().push(("arbitrate", wait));
            self.advance(self.arbitrate_takes.min(wait));
            self.arbitration
        }
        fn hand_off(&mut self, send: Duration, reply: Duration) -> HandoffOutcome {
            self.calls.borrow_mut().push("hand_off");
            self.caps.borrow_mut().push(("send", send));
            self.caps.borrow_mut().push(("reply", reply));
            self.advance(self.hand_off_takes.min(send + reply));
            self.handoff.remove(0)
        }
        fn wait_for_primary_exit(&mut self, cap: Duration) -> bool {
            self.calls.borrow_mut().push("wait_for_primary_exit");
            self.caps.borrow_mut().push(("exit_wait", cap));
            if !self.primary_exits {
                self.advance(cap);
            }
            self.primary_exits
        }
        fn restore_locally(&mut self, cap: Duration) -> LocalRestore {
            self.calls.borrow_mut().push("restore_locally");
            self.caps.borrow_mut().push(("local", cap));
            if self.local_dies {
                return LocalRestore::WorkerDied;
            }
            if self.local_takes > cap {
                self.advance(cap);
                return LocalRestore::TimedOut;
            }
            self.advance(self.local_takes);
            LocalRestore::Finished(RestoreReport {
                code: self.local,
                summary: "假".to_owned(),
            })
        }
    }

    /// [複審 N3] 整個命令列只有一個總時限：每段拿到的時限是「該段上限」與「剩下的預算」取小，本行程
    /// 還原也只拿剩下的預算；用完就以 7 結束，總耗時不超過 [`CLI_TOTAL_LIMIT`]。
    #[test]
    fn the_whole_command_line_is_bounded_by_one_deadline() {
        let mut ops = FakeOps::new(Arbitration::InstanceRunning);
        ops.arbitrate_takes = Duration::from_secs(30);
        ops.hand_off_takes = Duration::from_secs(35);
        ops.handoff = vec![HandoffOutcome::NoWindow];
        ops.primary_exits = true;
        ops.local_takes = Duration::from_secs(100);
        let (code, lines) = run(&mut ops);
        assert_eq!(code, RestoreCode::TimedOut);
        assert_ne!(code.exit_code(), 0);
        assert!(ops.clock.get() <= CLI_TOTAL_LIMIT, "{:?}", ops.clock.get());
        let caps = ops.caps.borrow().clone();
        assert_eq!(caps[0], ("arbitrate", Duration::from_secs(30)));
        assert_eq!(caps[1], ("send", HANDOFF_SEND_TIMEOUT));
        assert_eq!(caps[2], ("reply", HANDOFF_REPLY_WAIT));
        assert_eq!(
            caps.last().copied(),
            Some(("local", CLI_TOTAL_LIMIT - Duration::from_secs(65))),
            "本行程還原只拿剩下的預算：{caps:?}"
        );
        assert!(lines.iter().any(|l| l.contains("總時限")), "{lines:?}");

        // 沒有宿主：本行程還原拿到幾乎整個預算，做得完就照常回報。
        let mut ops = FakeOps::new(Arbitration::NoInstance);
        ops.local_takes = Duration::from_secs(3);
        assert_eq!(run(&mut ops).0, RestoreCode::Restored);
        assert_eq!(ops.caps.borrow()[1], ("local", CLI_TOTAL_LIMIT));
    }

    /// [4.7b 複審 L1] 還原工作執行緒 panic（沒有回報就結束）不是逾時：回 5（內部錯誤），記錄寫明
    /// 工作執行緒異常結束，不寫「總時限內沒做完、已有標記」。
    #[test]
    fn restore_worker_panic_is_internal_error_not_timeout() {
        let mut ops = FakeOps::new(Arbitration::NoInstance);
        ops.local_dies = true;
        let (code, lines) = run(&mut ops);
        assert_eq!(code, RestoreCode::Internal);
        assert_eq!(code.exit_code(), 5);
        assert!(
            lines.iter().any(|l| l.contains("工作執行緒異常結束")),
            "{lines:?}"
        );
        assert!(
            !lines
                .iter()
                .any(|l| l.contains("結束碼 7") || l.contains("還原進行中")),
            "panic 不得寫成逾時：{lines:?}"
        );

        // 對照：真的逾時仍是 7。
        let mut ops = FakeOps::new(Arbitration::NoInstance);
        ops.local_takes = CLI_TOTAL_LIMIT + Duration::from_secs(1);
        assert_eq!(run(&mut ops).0, RestoreCode::TimedOut);
    }

    /// 正式的等待（[`await_restore_worker`]）以真的執行緒區分三種結果：回報、逾時、panic。
    #[test]
    fn awaiting_the_restore_worker_tells_panic_from_timeout() {
        use std::sync::mpsc;

        let (tx, rx) = mpsc::channel::<RestoreReport>();
        let worker = std::thread::spawn(move || {
            let _keep = tx;
            panic!("還原工作執行緒 panic（測試）");
        });
        assert!(worker.join().is_err());
        assert_eq!(
            await_restore_worker(&rx, Duration::from_secs(5)),
            LocalRestore::WorkerDied
        );

        let (tx, rx) = mpsc::channel::<RestoreReport>();
        assert_eq!(
            await_restore_worker(&rx, Duration::from_millis(20)),
            LocalRestore::TimedOut
        );
        drop(tx);

        let (tx, rx) = mpsc::channel();
        let report = RestoreReport {
            code: RestoreCode::Restored,
            summary: "假".to_owned(),
        };
        tx.send(report.clone()).unwrap();
        assert_eq!(
            await_restore_worker(&rx, Duration::from_millis(20)),
            LocalRestore::Finished(report)
        );
    }

    fn run(ops: &mut FakeOps) -> (RestoreCode, Vec<String>) {
        let lines = RefCell::new(Vec::new());
        let code = run_with(ops, &|_, msg: &str| lines.borrow_mut().push(msg.to_owned()));
        (code, lines.into_inner())
    }

    #[test]
    fn without_a_running_host_restores_in_this_process() {
        let mut ops = FakeOps::new(Arbitration::NoInstance);
        let (code, lines) = run(&mut ops);
        assert_eq!(code, RestoreCode::Restored);
        assert_eq!(*ops.calls.borrow(), ["arbitrate", "restore_locally"]);
        assert!(
            lines.iter().any(|l| l.contains("沒有執行中的宿主")),
            "{lines:?}"
        );

        let mut ops = FakeOps::new(Arbitration::NoInstance);
        ops.local = RestoreCode::Blocked;
        assert_eq!(run(&mut ops).0, RestoreCode::Blocked);
        assert_eq!(RestoreCode::Blocked.exit_code(), 2);
    }

    #[test]
    fn with_a_running_host_hands_off_and_returns_its_reply() {
        for reply in [RestoreCode::Restored, RestoreCode::Failed] {
            let mut ops = FakeOps::new(Arbitration::InstanceRunning);
            ops.handoff = vec![HandoffOutcome::Replied(reply)];
            let (code, lines) = run(&mut ops);
            assert_eq!(code, reply);
            assert_eq!(*ops.calls.borrow(), ["arbitrate", "hand_off"]);
            assert!(
                lines.iter().any(|l| l.contains("執行中的宿主")),
                "{lines:?}"
            );
        }
    }

    #[test]
    fn no_reply_within_the_limit_is_a_distinct_failure() {
        let mut ops = FakeOps::new(Arbitration::InstanceRunning);
        ops.handoff = vec![HandoffOutcome::NoReply];
        let (code, _) = run(&mut ops);
        assert_eq!(code, RestoreCode::NoReply);
        assert_ne!(code.exit_code(), 0);
        assert_eq!(
            *ops.calls.borrow(),
            ["arbitrate", "hand_off"],
            "不得自己再還原一次"
        );
    }

    #[test]
    fn host_without_handoff_window_is_waited_for_then_restored_locally() {
        let mut ops = FakeOps::new(Arbitration::InstanceRunning);
        ops.handoff = vec![HandoffOutcome::NoWindow];
        ops.primary_exits = true;
        assert_eq!(run(&mut ops).0, RestoreCode::Restored);
        assert_eq!(
            *ops.calls.borrow(),
            [
                "arbitrate",
                "hand_off",
                "wait_for_primary_exit",
                "restore_locally"
            ]
        );

        let mut ops = FakeOps::new(Arbitration::InstanceRunning);
        ops.handoff = vec![HandoffOutcome::SendFailed("逾時".to_owned())];
        ops.primary_exits = false;
        let (code, _) = run(&mut ops);
        assert_eq!(code, RestoreCode::HandoffFailed);
        assert_eq!(
            *ops.calls.borrow(),
            ["arbitrate", "hand_off", "wait_for_primary_exit"],
            "宿主還在、又交接不了：不得兩邊同時動狀態檔"
        );
    }

    #[test]
    fn broken_arbitration_tries_handoff_first() {
        let mut ops = FakeOps::new(Arbitration::Unarbitrated);
        ops.handoff = vec![HandoffOutcome::Replied(RestoreCode::Restored)];
        assert_eq!(run(&mut ops).0, RestoreCode::Restored);
        assert_eq!(*ops.calls.borrow(), ["arbitrate", "hand_off"]);

        let mut ops = FakeOps::new(Arbitration::Unarbitrated);
        ops.handoff = vec![HandoffOutcome::NoWindow];
        assert_eq!(run(&mut ops).0, RestoreCode::Restored);
        assert_eq!(
            *ops.calls.borrow(),
            ["arbitrate", "hand_off", "restore_locally"]
        );

        let mut ops = FakeOps::new(Arbitration::StartupLockTimeout);
        ops.handoff = vec![HandoffOutcome::NoWindow];
        assert_eq!(run(&mut ops).0, RestoreCode::HandoffFailed);
        assert_eq!(*ops.calls.borrow(), ["arbitrate", "hand_off"]);
    }

    #[test]
    fn every_code_has_a_distinct_exit_code_and_only_restored_is_zero() {
        let all = [
            RestoreCode::Restored,
            RestoreCode::Failed,
            RestoreCode::Blocked,
            RestoreCode::NoReply,
            RestoreCode::HandoffFailed,
            RestoreCode::Internal,
            RestoreCode::PartiallyRestored,
            RestoreCode::TimedOut,
        ];
        let codes: std::collections::HashSet<i32> = all.iter().map(|c| c.exit_code()).collect();
        assert_eq!(codes.len(), all.len());
        for c in all {
            assert_eq!(c.exit_code() == 0, c == RestoreCode::Restored, "{c:?}");
        }
    }

    // ── 修正輪 2（審查 F3／F5／F7）──────────────────────────────────────────────────

    /// [F3] 宿主回「交接失敗」（沒有協調迴圈、迴圈已關閉）：與找不到交接視窗相同——等宿主結束，
    /// 等到就在本行程還原，等不到才是 4。
    #[test]
    fn handoff_failed_reply_waits_for_the_host_then_restores_locally() {
        for arbitration in [
            Arbitration::InstanceRunning,
            Arbitration::Unarbitrated,
            Arbitration::StartupLockTimeout,
        ] {
            let mut ops = FakeOps::new(arbitration);
            ops.handoff = vec![HandoffOutcome::Replied(RestoreCode::HandoffFailed)];
            ops.primary_exits = true;
            assert_eq!(run(&mut ops).0, RestoreCode::Restored, "{arbitration:?}");
            assert_eq!(
                *ops.calls.borrow(),
                [
                    "arbitrate",
                    "hand_off",
                    "wait_for_primary_exit",
                    "restore_locally"
                ],
                "{arbitration:?}"
            );

            let mut ops = FakeOps::new(arbitration);
            ops.handoff = vec![HandoffOutcome::Replied(RestoreCode::HandoffFailed)];
            ops.primary_exits = false;
            assert_eq!(
                run(&mut ops).0,
                RestoreCode::HandoffFailed,
                "{arbitration:?}"
            );
            assert_eq!(
                *ops.calls.borrow(),
                ["arbitrate", "hand_off", "wait_for_primary_exit"],
                "{arbitration:?}"
            );
        }
    }

    /// [F5／F7] 等宿主結束的上限是硬上限：探測是非阻塞的，總時間不超過上限加一次輪詢間隔。
    #[test]
    fn waiting_for_the_host_to_exit_has_a_hard_upper_bound() {
        use std::cell::Cell;
        let now = Cell::new(Duration::ZERO);
        let probes = Cell::new(0u32);
        let exited = wait_until(
            Duration::from_secs(10),
            Duration::from_millis(500),
            || {
                probes.set(probes.get() + 1);
                false
            },
            || now.get(),
            |d| now.set(now.get() + d),
        );
        assert!(!exited);
        assert!(now.get() <= Duration::from_secs(10), "{:?}", now.get());
        assert!(now.get() >= Duration::from_millis(9_500), "{:?}", now.get());
        assert_eq!(probes.get(), 21, "0、0.5、…、10 秒各探測一次");

        let now = Cell::new(Duration::ZERO);
        let exited = wait_until(
            Duration::from_secs(10),
            Duration::from_millis(500),
            || now.get() >= Duration::from_secs(2),
            || now.get(),
            |d| now.set(now.get() + d),
        );
        assert!(exited);
        assert_eq!(now.get(), Duration::from_secs(2));
    }

    /// [F7] 宿主端：回覆物件沒回覆就被丟棄＝回報「交接失敗」；只回報一次。
    #[test]
    fn dropped_reply_reports_handoff_failed_once() {
        let (reply, sent) = recording_reply(42);
        drop(reply);
        assert_eq!(
            *sent.lock().unwrap(),
            [(42, reply_slot(RestoreCode::HandoffFailed).unwrap())]
        );

        let (mut reply, sent) = recording_reply(43);
        reply.send(RestoreCode::Restored);
        drop(reply);
        assert_eq!(
            *sent.lock().unwrap(),
            [(43, reply_slot(RestoreCode::Restored).unwrap())]
        );
    }

    /// [F3／F7] 宿主端的三種情況：還沒有協調迴圈、協調迴圈啟動失敗、系統匣結束已還原完。
    #[test]
    fn primary_side_routes_the_three_no_loop_cases() {
        // 1. 還沒有（或沒有）協調迴圈：立即回報「交接失敗」。
        let (reply, sent) = recording_reply(1);
        route_handoff(None, reply);
        wait_for_reply(&sent);
        assert_eq!(
            *sent.lock().unwrap(),
            [(1, reply_slot(RestoreCode::HandoffFailed).unwrap())]
        );

        // 2. 協調迴圈啟動失敗（信箱已關閉、沒有結果）：要求被丟棄 →「交接失敗」。
        let (handle, join) = crate::wallpaper_coordinator::spawn_coordinator::<
            crate::wallpaper_coordinator::TauriPorts,
            _,
        >(|| Err("假的啟動失敗".to_owned()))
        .unwrap();
        join.join().unwrap();
        let (reply, sent) = recording_reply(2);
        route_handoff(Some(handle), reply);
        wait_for_reply(&sent);
        assert_eq!(
            *sent.lock().unwrap(),
            [(2, reply_slot(RestoreCode::HandoffFailed).unwrap())]
        );
    }

    /// installer-auto-update 最終審查 M2a，宿主端：過渡期中收到交接＝立即（同步）回報「交接失敗」一次。
    #[test]
    fn primary_side_refuses_the_handoff_during_the_update_transition() {
        let (reply, sent) = recording_reply(7);
        refuse_handoff(reply);
        assert_eq!(
            *sent.lock().unwrap(),
            [(7, reply_slot(RestoreCode::HandoffFailed).unwrap())],
            "同步回報、只回報一次（Drop 不重複）"
        );
    }

    /// installer-auto-update 最終審查 M2a，命令列端：過渡期中的宿主回「交接失敗」後隨即結束——命令列等到它結束，
    /// 在本行程還原，總時長在 [`CLI_TOTAL_LIMIT`] 內，本行程還原拿到剩下的預算。另斷言最壞情況（仲裁與送出各用滿
    /// 上限、等宿主結束用滿 [`PRIMARY_EXIT_WAIT`]）之後仍留有還原的時間。
    #[test]
    fn handoff_refused_during_the_update_transition_restores_here_within_the_total_limit() {
        let mut ops = FakeOps::new(Arbitration::InstanceRunning);
        ops.arbitrate_takes = STARTUP_LOCK_WAIT;
        ops.hand_off_takes = HANDOFF_SEND_TIMEOUT;
        ops.handoff = vec![HandoffOutcome::Replied(RestoreCode::HandoffFailed)];
        ops.primary_exits = true;
        ops.local_takes = Duration::from_secs(5);
        let (code, lines) = run(&mut ops);
        assert_eq!(code, RestoreCode::Restored);
        assert_eq!(
            *ops.calls.borrow(),
            [
                "arbitrate",
                "hand_off",
                "wait_for_primary_exit",
                "restore_locally"
            ]
        );
        assert!(ops.clock.get() <= CLI_TOTAL_LIMIT, "{:?}", ops.clock.get());
        let caps = ops.caps.borrow().clone();
        let exit_wait = caps.iter().find(|c| c.0 == "exit_wait").unwrap().1;
        assert_eq!(exit_wait, PRIMARY_EXIT_WAIT);
        let local = caps.iter().find(|c| c.0 == "local").unwrap().1;
        assert_eq!(
            local,
            CLI_TOTAL_LIMIT - STARTUP_LOCK_WAIT - HANDOFF_SEND_TIMEOUT,
            "宿主立即結束：本行程還原拿到剩下的全部預算：{caps:?}"
        );
        let worst_case_before_restore =
            STARTUP_LOCK_WAIT + HANDOFF_SEND_TIMEOUT + PRIMARY_EXIT_WAIT;
        assert!(
            worst_case_before_restore + Duration::from_secs(30) <= CLI_TOTAL_LIMIT,
            "最壞情況下仍要留至少 30 秒給本行程還原：{worst_case_before_restore:?}"
        );
        assert!(lines.iter().any(|l| l.contains("交接失敗")), "{lines:?}");
    }

    type Sent = std::sync::Arc<std::sync::Mutex<Vec<(u32, usize)>>>;

    fn recording_reply(pid: u32) -> (ReplyTo, Sent) {
        let sent: Sent = std::sync::Arc::default();
        let sink = std::sync::Arc::clone(&sent);
        (
            ReplyTo::with_signal(
                Some(pid),
                Box::new(move |pid, slot| {
                    sink.lock().unwrap().push((pid, slot));
                    Ok(())
                }),
            ),
            sent,
        )
    }

    fn wait_for_reply(sent: &Sent) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while sent.lock().unwrap().is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
