//! `--wait-exit <pid>`：等舊宿主行程結束後才進入啟動仲裁（installer-auto-update task 3.3、design.md D4）。
//!
//! 使用情境：更新安裝檔已啟動、卻在 `install()` 之後失敗（例如 `ShellExecuteW` 失敗），舊宿主此時沒有小工具也沒有
//! 桌布協調，改以 `--autostart --wait-exit <本行程 PID>` 啟動一個新的自己再立刻 `std::process::exit`
//! （[`crate::updater::relaunch_after_failed_install`]）。新行程若不等舊行程結束，`desktop/instance.rs` 取不到
//! 執行個體鎖就會當 Secondary 轉交參數後退出，而舊行程收到 `--autostart` 會被 `tray.rs` 靜默忽略——最後沒有任何
//! 宿主在跑。本模組讓新行程在 `main()` 的仲裁**之前**等舊行程結束。
//!
//! ## 驗證流程（PID 可能已被別的行程重用，不能盲等）
//!
//! 1. `OpenProcess(SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION)` 失敗＝行程不存在（或無權限）→ 視為已結束，直接仲裁。
//! 2. 拿到把手後確認它是舊宿主：`QueryFullProcessImageNameW` 的映像路徑與自己相同（不分大小寫），且 `GetProcessTimes`
//!    的建立時間**早於**自己；不符（PID 被重用、或根本是自己）→ 不等，直接仲裁。讀不到身分也不等（無法證明是舊宿主，
//!    寧可不等也不要被一個無關的長壽行程卡 30 秒）。
//! 3. 符合就 `WaitForSingleObject`，上限 [`WAIT_EXIT_LIMIT`]（30 秒），之後進入仲裁。逾時只記錄、照常仲裁（新行程
//!    會成為 Secondary 退出，宿主要等下次登入才回來——design.md D4 已知風險）。
//!
//! 判定邏輯對 [`ProcessOps`] 泛型，單元測試以假物件涵蓋「PID 被重用、開不了、路徑不同、建立時間較晚、逾時」；
//! [`SystemOps`] 是真正的 Win32 實作（windows crate 0.62.2，簽名查證於 `~/.cargo/registry/src/*/windows-0.62.2/
//! src/Windows/Win32/System/Threading/mod.rs`：`OpenProcess(PROCESS_ACCESS_RIGHTS, bool, u32) -> Result<HANDLE>`、
//! `QueryFullProcessImageNameW(HANDLE, PROCESS_NAME_FORMAT, PWSTR, *mut u32) -> Result<()>`、
//! `GetProcessTimes(HANDLE, *mut FILETIME ×4) -> Result<()>`、`WaitForSingleObject(HANDLE, u32) -> WAIT_EVENT`，
//! 皆在已啟用的 `Win32_System_Threading`）。

use std::time::{Duration, Instant};

use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetProcessTimes, OpenProcess, QueryFullProcessImageNameW,
    WaitForSingleObject, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE,
};

/// 等舊行程結束的上限（design.md D4）。
pub const WAIT_EXIT_LIMIT: Duration = Duration::from_secs(30);

/// 命令列旗標。
pub const WAIT_EXIT_FLAG: &str = "--wait-exit";

/// 一個行程的身分（判斷「是不是舊宿主」用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    /// `QueryFullProcessImageNameW`（Win32 路徑格式）。
    pub image_path: String,
    /// `GetProcessTimes` 的建立時間（FILETIME，100 ns 計數）。
    pub created: u64,
}

/// 行程操作（可注入）：真實作 [`SystemOps`]，測試用假物件。
pub trait ProcessOps {
    /// 開啟後的把手（Drop 時關閉）。
    type Handle;
    /// 開啟行程（同步＋有限查詢權限）；失敗＝視為已結束。
    fn open(&self, pid: u32) -> Result<Self::Handle, String>;
    /// 讀映像路徑與建立時間。
    fn info(&self, handle: &Self::Handle) -> Result<ProcessInfo, String>;
    /// 等行程結束最多 `timeout`；已結束回 `true`。
    fn wait_exit(&self, handle: &Self::Handle, timeout: Duration) -> bool;
}

/// 為什麼判定它不是舊宿主。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotPredecessor {
    /// 映像路徑不同（PID 被別的程式重用）。
    DifferentImage { theirs: String, ours: String },
    /// 對方的建立時間不早於自己（PID 被新行程重用，或就是自己）。
    NotOlderThanUs { theirs: u64, ours: u64 },
}

/// [`wait_for_predecessor`] 的結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaitOutcome {
    /// 開不了行程（已結束或不存在）→ 直接仲裁。
    NotRunning(String),
    /// 開得了、但不是舊宿主 → 不等，直接仲裁。
    NotPredecessor(NotPredecessor),
    /// 無法讀取身分（自己或對方）→ 不等，直接仲裁。
    CannotVerify(String),
    /// 舊宿主已結束（等了 `waited`）。
    Exited { waited: Duration },
    /// 上限內舊宿主仍未結束 → 照常仲裁（新行程多半會成為 Secondary）。
    TimedOut,
}

/// 驗證 `pid` 是舊宿主後等它結束（純邏輯；`own` 是自己的身分，讀不到時傳 `Err`）。
pub fn wait_for_predecessor<O: ProcessOps>(
    ops: &O,
    pid: u32,
    own: &Result<ProcessInfo, String>,
    limit: Duration,
) -> WaitOutcome {
    let handle = match ops.open(pid) {
        Ok(h) => h,
        Err(e) => return WaitOutcome::NotRunning(e),
    };
    let own = match own {
        Ok(own) => own,
        Err(e) => return WaitOutcome::CannotVerify(format!("讀不到自己的身分：{e}")),
    };
    let theirs = match ops.info(&handle) {
        Ok(theirs) => theirs,
        Err(e) => return WaitOutcome::CannotVerify(format!("讀不到 PID {pid} 的身分：{e}")),
    };
    if !same_image(&theirs.image_path, &own.image_path) {
        return WaitOutcome::NotPredecessor(NotPredecessor::DifferentImage {
            theirs: theirs.image_path,
            ours: own.image_path.clone(),
        });
    }
    if theirs.created >= own.created {
        return WaitOutcome::NotPredecessor(NotPredecessor::NotOlderThanUs {
            theirs: theirs.created,
            ours: own.created,
        });
    }
    let started = Instant::now();
    if ops.wait_exit(&handle, limit) {
        WaitOutcome::Exited {
            waited: started.elapsed(),
        }
    } else {
        WaitOutcome::TimedOut
    }
}

/// Windows 路徑不分大小寫。
fn same_image(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// 解析 `--wait-exit <pid>`／`--wait-exit=<pid>`（`args[0]` 是 exe 路徑，略過）。沒有旗標回 `None`；有旗標但缺值、
/// 不是正整數或為 0 回 `Some(Err)`（呼叫端記錄後照常啟動，不等）。
pub fn parse_wait_exit_pid(args: &[String]) -> Option<Result<u32, String>> {
    let mut iter = args.iter().skip(1);
    while let Some(arg) = iter.next() {
        let value = if arg == WAIT_EXIT_FLAG {
            match iter.next() {
                Some(v) => v.as_str(),
                None => return Some(Err(format!("{WAIT_EXIT_FLAG} 缺少 PID"))),
            }
        } else if let Some(v) = arg.strip_prefix("--wait-exit=") {
            v
        } else {
            continue;
        };
        return Some(match value.parse::<u32>() {
            Ok(0) => Err(format!("{WAIT_EXIT_FLAG} 的 PID 不可為 0")),
            Ok(pid) => Ok(pid),
            Err(_) => Err(format!("{WAIT_EXIT_FLAG} 的 PID 不是正整數：{value:?}")),
        });
    }
    None
}

/// 行程把手（Drop 關閉）。
pub struct OwnedProcess(HANDLE);

impl Drop for OwnedProcess {
    fn drop(&mut self) {
        // SAFETY: 把手由 `OpenProcess` 取得、只在這裡關一次。
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// 真正的 Win32 實作。
pub struct SystemOps;

impl ProcessOps for SystemOps {
    type Handle = OwnedProcess;

    fn open(&self, pid: u32) -> Result<OwnedProcess, String> {
        // SAFETY: 不繼承；失敗時 windows crate 回 Err（不會給無效把手）。
        unsafe {
            OpenProcess(
                PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                false,
                pid,
            )
        }
        .map(OwnedProcess)
        .map_err(|e| format!("OpenProcess(PID {pid}) 失敗：{e}"))
    }

    fn info(&self, handle: &OwnedProcess) -> Result<ProcessInfo, String> {
        process_info(handle.0)
    }

    fn wait_exit(&self, handle: &OwnedProcess, timeout: Duration) -> bool {
        // `u32::MAX` 是 INFINITE，上限再小一點。
        let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX - 1);
        // SAFETY: `handle.0` 是本物件持有、帶 SYNCHRONIZE 的有效行程把手。
        let r = unsafe { WaitForSingleObject(handle.0, ms) };
        if r == WAIT_OBJECT_0 {
            return true;
        }
        if r != WAIT_TIMEOUT {
            log::warn!("等待行程結束失敗（WAIT_EVENT {:#x}），視為未結束", r.0);
        }
        false
    }
}

/// 以把手讀映像路徑與建立時間（`handle` 需要 `PROCESS_QUERY_LIMITED_INFORMATION`）。
fn process_info(handle: HANDLE) -> Result<ProcessInfo, String> {
    // 32768 個 UTF-16 單位＝Windows 路徑長度上限。
    let mut buf = vec![0u16; 32_768];
    let mut len = u32::try_from(buf.len()).unwrap_or(u32::MAX);
    // SAFETY: `buf` 可寫且至少 `len` 個單位、生命期涵蓋本呼叫；`len` 是局部變數，回傳時為實際長度（不含結尾 0）。
    unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
    }
    .map_err(|e| format!("QueryFullProcessImageNameW 失敗：{e}"))?;
    let image_path = String::from_utf16_lossy(&buf[..(len as usize).min(buf.len())]);

    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: 四個輸出參數都是本函式的區域變數。
    unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) }
        .map_err(|e| format!("GetProcessTimes 失敗：{e}"))?;
    Ok(ProcessInfo {
        image_path,
        created: (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime),
    })
}

/// 本行程的身分（偽把手 `GetCurrentProcess` 具備完整存取權）。
pub fn own_process_info() -> Result<ProcessInfo, String> {
    // SAFETY: `GetCurrentProcess` 回傳不需關閉的偽把手。
    process_info(unsafe { GetCurrentProcess() })
}

/// `main()` 在啟動仲裁之前呼叫：命令列有 `--wait-exit <pid>` 就等舊宿主結束（上限 [`WAIT_EXIT_LIMIT`]），
/// 每種結果記一行；沒有旗標什麼都不做。回傳實際執行的結果（沒有旗標或參數錯誤回 `None`）。
pub fn run_from_args(args: &[String]) -> Option<WaitOutcome> {
    let pid = match parse_wait_exit_pid(args)? {
        Ok(pid) => pid,
        Err(e) => {
            log::warn!("{e}，不等待、照常啟動");
            return None;
        }
    };
    log::info!(
        "{WAIT_EXIT_FLAG} {pid}：驗證後等舊宿主結束（上限 {} 秒）",
        WAIT_EXIT_LIMIT.as_secs()
    );
    let outcome = wait_for_predecessor(&SystemOps, pid, &own_process_info(), WAIT_EXIT_LIMIT);
    match &outcome {
        WaitOutcome::NotRunning(why) => {
            log::info!("{WAIT_EXIT_FLAG}：PID {pid} 已不存在（{why}），直接進入啟動仲裁")
        }
        WaitOutcome::NotPredecessor(why) => log::warn!(
            "{WAIT_EXIT_FLAG}：PID {pid} 不是舊宿主（{why:?}；PID 被重用），不等待、直接進入啟動仲裁"
        ),
        WaitOutcome::CannotVerify(why) => {
            log::warn!("{WAIT_EXIT_FLAG}：無法驗證 PID {pid} 是舊宿主（{why}），不等待、直接進入啟動仲裁")
        }
        WaitOutcome::Exited { waited } => log::info!(
            "{WAIT_EXIT_FLAG}：舊宿主（PID {pid}）已結束（等了 {:.2} 秒），進入啟動仲裁",
            waited.as_secs_f32()
        ),
        WaitOutcome::TimedOut => log::error!(
            "{WAIT_EXIT_FLAG}：舊宿主（PID {pid}）{} 秒內沒有結束，照常進入啟動仲裁（多半會成為 Secondary 退出，宿主要等下次登入才回來）",
            WAIT_EXIT_LIMIT.as_secs()
        ),
    }
    Some(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    fn info(path: &str, created: u64) -> ProcessInfo {
        ProcessInfo {
            image_path: path.to_owned(),
            created,
        }
    }

    const EXE: &str = r"C:\Users\u\AppData\Local\fc-host\fc-host.exe";

    /// 假行程操作：`open`／`info` 的結果可指定；`exits_after` 為 `None` 表示永遠不結束。
    struct Fake {
        open: Result<(), String>,
        info: Result<ProcessInfo, String>,
        exits_after: Option<Duration>,
        info_calls: Cell<u32>,
        wait_limits: RefCell<Vec<Duration>>,
    }

    impl Fake {
        fn predecessor() -> Self {
            Fake {
                open: Ok(()),
                info: Ok(info(EXE, 100)),
                exits_after: Some(Duration::ZERO),
                info_calls: Cell::new(0),
                wait_limits: RefCell::new(Vec::new()),
            }
        }
    }

    impl ProcessOps for Fake {
        type Handle = ();
        fn open(&self, _pid: u32) -> Result<(), String> {
            self.open.clone()
        }
        fn info(&self, _h: &()) -> Result<ProcessInfo, String> {
            self.info_calls.set(self.info_calls.get() + 1);
            self.info.clone()
        }
        fn wait_exit(&self, _h: &(), timeout: Duration) -> bool {
            self.wait_limits.borrow_mut().push(timeout);
            matches!(self.exits_after, Some(d) if d <= timeout)
        }
    }

    fn own() -> Result<ProcessInfo, String> {
        Ok(info(EXE, 200))
    }

    #[test]
    fn limit_is_thirty_seconds() {
        assert_eq!(WAIT_EXIT_LIMIT, Duration::from_secs(30));
    }

    /// 開不了＝視為已結束，不查身分、不等。
    #[test]
    fn unopenable_pid_counts_as_already_exited() {
        let mut f = Fake::predecessor();
        f.open = Err("拒絕存取或不存在".into());
        let out = wait_for_predecessor(&f, 4242, &own(), WAIT_EXIT_LIMIT);
        assert!(matches!(out, WaitOutcome::NotRunning(_)), "{out:?}");
        assert_eq!(f.info_calls.get(), 0);
        assert!(f.wait_limits.borrow().is_empty());
    }

    /// PID 被重用（一）：路徑不同的行程，不等。
    #[test]
    fn reused_pid_with_a_different_image_is_not_waited_for() {
        let mut f = Fake::predecessor();
        f.info = Ok(info(r"C:\Windows\System32\notepad.exe", 100));
        f.exits_after = None;
        let out = wait_for_predecessor(&f, 4242, &own(), WAIT_EXIT_LIMIT);
        assert!(
            matches!(
                out,
                WaitOutcome::NotPredecessor(NotPredecessor::DifferentImage { .. })
            ),
            "{out:?}"
        );
        assert!(f.wait_limits.borrow().is_empty(), "不得等待");
    }

    /// PID 被重用（二）：同一個 exe 但建立時間晚於自己（例如使用者另外開的新宿主），不等。
    #[test]
    fn reused_pid_created_after_us_is_not_waited_for() {
        let mut f = Fake::predecessor();
        f.info = Ok(info(EXE, 300));
        f.exits_after = None;
        let out = wait_for_predecessor(&f, 4242, &own(), WAIT_EXIT_LIMIT);
        assert_eq!(
            out,
            WaitOutcome::NotPredecessor(NotPredecessor::NotOlderThanUs {
                theirs: 300,
                ours: 200
            })
        );
        assert!(f.wait_limits.borrow().is_empty());
    }

    /// 建立時間相同（含 `--wait-exit` 傳了自己的 PID）也不等：必須嚴格早於自己。
    #[test]
    fn equal_creation_time_or_own_pid_is_not_waited_for() {
        let mut f = Fake::predecessor();
        f.info = Ok(info(EXE, 200));
        let out = wait_for_predecessor(&f, std::process::id(), &own(), WAIT_EXIT_LIMIT);
        assert!(matches!(out, WaitOutcome::NotPredecessor(_)), "{out:?}");
        assert!(f.wait_limits.borrow().is_empty());
    }

    /// 路徑比對不分大小寫（Windows）。
    #[test]
    fn image_path_comparison_ignores_case() {
        let mut f = Fake::predecessor();
        f.info = Ok(info(&EXE.to_uppercase(), 100));
        let out = wait_for_predecessor(&f, 4242, &own(), WAIT_EXIT_LIMIT);
        assert!(matches!(out, WaitOutcome::Exited { .. }), "{out:?}");
    }

    /// 身分讀不到（對方或自己）＝無法證明是舊宿主，不等。
    #[test]
    fn unreadable_identity_is_not_waited_for() {
        let mut f = Fake::predecessor();
        f.info = Err("拒絕存取".into());
        f.exits_after = None;
        let out = wait_for_predecessor(&f, 4242, &own(), WAIT_EXIT_LIMIT);
        assert!(matches!(out, WaitOutcome::CannotVerify(_)), "{out:?}");
        assert!(f.wait_limits.borrow().is_empty());

        let f = Fake::predecessor();
        let out = wait_for_predecessor(&f, 4242, &Err("無法查詢".into()), WAIT_EXIT_LIMIT);
        assert!(matches!(out, WaitOutcome::CannotVerify(_)), "{out:?}");
        assert!(f.wait_limits.borrow().is_empty());
    }

    /// 符合條件：以 30 秒上限等待，結束了回 `Exited`。
    #[test]
    fn verified_predecessor_is_waited_for_with_the_limit() {
        let f = Fake::predecessor();
        let out = wait_for_predecessor(&f, 4242, &own(), WAIT_EXIT_LIMIT);
        assert!(matches!(out, WaitOutcome::Exited { .. }), "{out:?}");
        assert_eq!(*f.wait_limits.borrow(), vec![WAIT_EXIT_LIMIT]);
    }

    /// 上限內沒結束＝`TimedOut`（呼叫端記錄後照常仲裁）。
    #[test]
    fn predecessor_that_outlives_the_limit_times_out() {
        let mut f = Fake::predecessor();
        f.exits_after = Some(Duration::from_secs(31));
        let out = wait_for_predecessor(&f, 4242, &own(), WAIT_EXIT_LIMIT);
        assert_eq!(out, WaitOutcome::TimedOut);
        f.exits_after = None;
        assert_eq!(
            wait_for_predecessor(&f, 4242, &own(), Duration::from_millis(5)),
            WaitOutcome::TimedOut
        );
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn parse_accepts_both_spellings_anywhere_after_the_exe() {
        let exe = r"C:\fc-host.exe";
        assert_eq!(
            parse_wait_exit_pid(&args(&[exe, "--autostart", "--wait-exit", "1234"])),
            Some(Ok(1234))
        );
        assert_eq!(
            parse_wait_exit_pid(&args(&[exe, "--wait-exit=77", "--autostart"])),
            Some(Ok(77))
        );
        assert_eq!(parse_wait_exit_pid(&args(&[exe, "--autostart"])), None);
        assert_eq!(parse_wait_exit_pid(&args(&[exe])), None);
        // 第一個引數是 exe 路徑，不當旗標。
        assert_eq!(parse_wait_exit_pid(&args(&["--wait-exit", "5"])), None);
    }

    #[test]
    fn parse_rejects_missing_zero_and_non_numeric_values() {
        let exe = r"C:\fc-host.exe";
        for bad in [
            vec![exe, "--wait-exit"],
            vec![exe, "--wait-exit", "0"],
            vec![exe, "--wait-exit", "abc"],
            vec![exe, "--wait-exit", "-3"],
            vec![exe, "--wait-exit="],
        ] {
            assert!(
                matches!(parse_wait_exit_pid(&args(&bad)), Some(Err(_))),
                "{bad:?}"
            );
        }
    }

    // ── 真正的 Win32：以暫時啟動的 cmd.exe 當「舊宿主」，全程不碰真實宿主、桌面與網路 ──────────

    /// 啟動一個阻塞在 stdin 的 `cmd.exe`（`set /p`）；關掉 stdin 它就結束。
    fn spawn_blocked_cmd() -> (std::process::Child, std::process::ChildStdin) {
        use std::process::{Command, Stdio};
        let comspec = std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".to_owned());
        let mut child = Command::new(comspec)
            .args(["/d", "/c", "set /p x="])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("啟動 cmd.exe");
        let stdin = child.stdin.take().expect("stdin");
        (child, stdin)
    }

    #[test]
    fn real_win32_reads_own_identity() {
        let me = own_process_info().expect("自己的身分");
        let exe = std::env::current_exe().unwrap();
        assert!(
            same_image(&me.image_path, &exe.to_string_lossy())
                || me
                    .image_path
                    .to_lowercase()
                    .ends_with(&exe.file_name().unwrap().to_string_lossy().to_lowercase()),
            "{} vs {}",
            me.image_path,
            exe.display()
        );
        assert!(me.created > 0);
    }

    #[test]
    fn real_win32_waits_for_a_verified_process_until_it_exits() {
        let (mut child, stdin) = spawn_blocked_cmd();
        let pid = child.id();
        let handle = SystemOps.open(pid).expect("開得了");
        let theirs = SystemOps.info(&handle).expect("讀得到身分");
        assert!(
            theirs.image_path.to_lowercase().ends_with("cmd.exe"),
            "{theirs:?}"
        );
        // 讓「自己」假裝是比它晚建立、同映像的新行程。
        let me = Ok(ProcessInfo {
            image_path: theirs.image_path.clone(),
            created: theirs.created + 1,
        });
        // 還在跑：短上限內等不到。
        assert_eq!(
            wait_for_predecessor(&SystemOps, pid, &me, Duration::from_millis(150)),
            WaitOutcome::TimedOut
        );
        // 關掉 stdin → cmd 結束 → 等得到。
        drop(stdin);
        let out = wait_for_predecessor(&SystemOps, pid, &me, Duration::from_secs(10));
        assert!(matches!(out, WaitOutcome::Exited { .. }), "{out:?}");
        let _ = child.wait();
    }

    #[test]
    fn real_win32_does_not_wait_when_the_image_differs_from_ours() {
        let (mut child, stdin) = spawn_blocked_cmd();
        let pid = child.id();
        // 自己（測試行程）的映像不是 cmd.exe → 視為 PID 被重用，不等。
        let me = own_process_info();
        let started = Instant::now();
        let out = wait_for_predecessor(&SystemOps, pid, &me, Duration::from_secs(10));
        assert!(
            matches!(
                out,
                WaitOutcome::NotPredecessor(NotPredecessor::DifferentImage { .. })
            ),
            "{out:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(5), "不得等待");
        drop(stdin);
        let _ = child.wait();
    }

    #[test]
    fn real_win32_treats_a_dead_pid_as_not_running() {
        let (mut child, stdin) = spawn_blocked_cmd();
        let pid = child.id();
        drop(stdin);
        child.wait().unwrap();
        drop(child);
        // 回收後立刻查：行程物件已被我們的 Child 把手之外釋放，OpenProcess 通常失敗；
        // 若系統剛好把同一 PID 配給別的行程，結果會是 NotPredecessor（或讀不到映像時的 CannotVerify）——三者都是「不等」。
        let out =
            wait_for_predecessor(&SystemOps, pid, &own_process_info(), Duration::from_secs(1));
        assert!(
            matches!(
                out,
                WaitOutcome::NotRunning(_)
                    | WaitOutcome::NotPredecessor(_)
                    | WaitOutcome::CannotVerify(_)
            ),
            "{out:?}"
        );
    }
}
