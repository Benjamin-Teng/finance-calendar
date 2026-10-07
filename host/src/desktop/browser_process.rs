//! 桌布渲染視窗那組 WebView2 browser 行程的把手（bug-leaked-renderer：渲染 browser 行程偶發殘留）。
//!
//! 渲染視窗每次用同一個獨立的 WebView2 使用者資料夾，關窗後它的 browser 行程才開始結束（重現量測：
//! 返回後約 0.18 秒）；6.2 長跑有一次它卡在結束流程、永不退出。宿主在下一次渲染前等它結束、卡住就
//! 結束它（見 `crate::wallpaper_render` 模組文件「前一個渲染 browser 行程」一節）。本模組只提供 Win32 呼叫：
//!
//! - [`ProcessHandle::open`]：渲染視窗建好、還在的時候，以 `ICoreWebView2::BrowserProcessId` 讀到
//!   的 PID 立刻開把手。之後持有把手，PID 被重用也不會認錯行程；
//! - [`ProcessHandle::wait_exit`]：`WaitForSingleObject`（行程物件結束時為 signaled）；
//! - [`ProcessHandle::identity`]：父 PID（Toolhelp 快照）與命令列
//!   （`NtQueryInformationProcess(ProcessCommandLineInformation)`），給呼叫端核對「確實是宿主自己
//!   的渲染 browser 行程」後才結束它；
//! - [`ProcessHandle::terminate`]：`TerminateProcess`。
//!
//! windows crate 0.62.2 的簽名查證於 `~/.cargo/registry/src/*/windows-0.62.2/src/Windows/`：
//! - `Win32/System/Threading/mod.rs`：`OpenProcess(PROCESS_ACCESS_RIGHTS, bool, u32) -> Result<HANDLE>`、
//!   `WaitForSingleObject(HANDLE, u32) -> WAIT_EVENT`、`TerminateProcess(HANDLE, u32) -> Result<()>`、
//!   `PROCESS_SYNCHRONIZE`、`PROCESS_TERMINATE`、`PROCESS_QUERY_LIMITED_INFORMATION`（已啟用的
//!   `Win32_System_Threading`）；
//! - `Wdk/System/Threading/mod.rs`：`NtQueryInformationProcess(HANDLE, PROCESSINFOCLASS, *mut c_void,
//!   u32, *mut u32) -> NTSTATUS`、`ProcessCommandLineInformation`（＝60；feature `Wdk_System_Threading`）。
//!   這個資訊類別只需要 `PROCESS_QUERY_LIMITED_INFORMATION`，回傳 `UNICODE_STRING` 緊接字元資料；
//! - `Win32/System/Diagnostics/ToolHelp/mod.rs`：`CreateToolhelp32Snapshot`、`Process32FirstW`／
//!   `Process32NextW`、`PROCESSENTRY32W.th32ParentProcessID`（已啟用的
//!   `Win32_System_Diagnostics_ToolHelp`）。

use std::time::Duration;

use windows::Wdk::System::Threading::{NtQueryInformationProcess, ProcessCommandLineInformation};
use windows::Win32::Foundation::{
    CloseHandle, HANDLE, UNICODE_STRING, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
};

/// 一個行程的把手（`SYNCHRONIZE | QUERY_LIMITED_INFORMATION`，可以的話加 `TERMINATE`）。
pub struct ProcessHandle {
    handle: HANDLE,
    pid: u32,
    can_terminate: bool,
}

// SAFETY: 行程把手是核心物件把手，可在任何執行緒使用與關閉；本型別只在 Drop 關一次。
unsafe impl Send for ProcessHandle {}
// SAFETY: 所有方法都只以把手呼叫執行緒安全的核心 API（Wait／Query／Terminate），不改內部狀態。
unsafe impl Sync for ProcessHandle {}

/// 行程的身分（給「是否可以結束它」的核對用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessIdentity {
    /// 父行程 PID（Toolhelp 快照；找不到為 `None`）。
    pub parent_pid: Option<u32>,
    /// 完整命令列。
    pub command_line: String,
}

impl ProcessHandle {
    /// 開 `pid` 的把手。先要求含 `PROCESS_TERMINATE`，被拒時退回只有等待與查詢（之後
    /// [`ProcessHandle::terminate`] 回錯誤）。
    pub fn open(pid: u32) -> Result<Self, String> {
        if pid == 0 {
            return Err("PID 為 0".to_owned());
        }
        let full = PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE;
        // SAFETY: 不繼承；失敗時 windows crate 回 Err（不會給無效把手）。
        match unsafe { OpenProcess(full, false, pid) } {
            Ok(handle) => Ok(ProcessHandle {
                handle,
                pid,
                can_terminate: true,
            }),
            Err(_) => {
                let limited = PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION;
                // SAFETY: 同上。
                let handle = unsafe { OpenProcess(limited, false, pid) }
                    .map_err(|e| format!("OpenProcess(PID {pid}) 失敗：{e}"))?;
                Ok(ProcessHandle {
                    handle,
                    pid,
                    can_terminate: false,
                })
            }
        }
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// 等行程結束，最多 `timeout`；已結束（含在呼叫前就結束）回 `true`。等待本身失敗視為未結束。
    pub fn wait_exit(&self, timeout: Duration) -> bool {
        let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX - 1);
        // SAFETY: `self.handle` 是本物件持有、帶 SYNCHRONIZE 的有效行程把手。
        let r = unsafe { WaitForSingleObject(self.handle, ms) };
        if r == WAIT_OBJECT_0 {
            return true;
        }
        if r != WAIT_TIMEOUT {
            log::warn!("等待行程 {} 結束失敗（WAIT_EVENT {:#x}）", self.pid, r.0);
        }
        false
    }

    /// 父 PID 與命令列。
    pub fn identity(&self) -> Result<ProcessIdentity, String> {
        Ok(ProcessIdentity {
            parent_pid: parent_pid_of(self.pid)?,
            command_line: self.command_line()?,
        })
    }

    fn command_line(&self) -> Result<String, String> {
        let mut needed = 0u32;
        // 先問大小（STATUS_INFO_LENGTH_MISMATCH 時 needed 為所需位元組數）。
        // SAFETY: 長度 0、緩衝區為 null 是合法的探詢；`needed` 是本函式的局部變數。
        let _ = unsafe {
            NtQueryInformationProcess(
                self.handle,
                ProcessCommandLineInformation,
                std::ptr::null_mut(),
                0,
                &mut needed,
            )
        };
        if needed == 0 {
            return Err(format!("讀不到行程 {} 的命令列長度", self.pid));
        }
        // 以 u64 配置確保對齊足夠 UNICODE_STRING（含指標）。
        let words = (needed as usize).div_ceil(8);
        let mut buf = vec![0u64; words];
        let len = u32::try_from(words * 8).unwrap_or(u32::MAX);
        // SAFETY: `buf` 至少 `len` 位元組、可寫，生命期涵蓋本呼叫；`needed` 為局部變數。
        let status = unsafe {
            NtQueryInformationProcess(
                self.handle,
                ProcessCommandLineInformation,
                buf.as_mut_ptr().cast(),
                len,
                &mut needed,
            )
        };
        if status.0 < 0 {
            return Err(format!(
                "NtQueryInformationProcess(PID {}, ProcessCommandLineInformation) 失敗：{:#x}",
                self.pid, status.0
            ));
        }
        // SAFETY: 成功時緩衝區開頭是 UNICODE_STRING，Buffer 指向同一緩衝區內、長度 Length 位元組
        // 的 UTF-16 資料（核心寫入）；`buf` 仍存活且對齊 8。
        let text = unsafe {
            let us = &*(buf.as_ptr().cast::<UNICODE_STRING>());
            if us.Buffer.is_null() || us.Length == 0 {
                String::new()
            } else {
                let chars = std::slice::from_raw_parts(us.Buffer.0, usize::from(us.Length) / 2);
                String::from_utf16_lossy(chars)
            }
        };
        Ok(text)
    }

    /// 結束行程（結束碼 1）。開把手時沒拿到 `PROCESS_TERMINATE` 回錯誤。
    pub fn terminate(&self) -> Result<(), String> {
        if !self.can_terminate {
            return Err(format!(
                "行程 {} 的把手沒有 PROCESS_TERMINATE 權限",
                self.pid
            ));
        }
        // SAFETY: `self.handle` 是本物件持有、帶 PROCESS_TERMINATE 的有效行程把手。
        unsafe { TerminateProcess(self.handle, 1) }
            .map_err(|e| format!("TerminateProcess(PID {}) 失敗：{e}", self.pid))
    }
}

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        // SAFETY: 本物件持有的把手，只在這裡關一次。
        if let Err(e) = unsafe { CloseHandle(self.handle) } {
            log::warn!("關閉行程 {} 的把手失敗：{e}", self.pid);
        }
    }
}

/// 以 Toolhelp 快照找 `pid` 的父 PID；快照裡沒有這個行程（已結束）回 `Ok(None)`。
fn parent_pid_of(pid: u32) -> Result<Option<u32>, String> {
    // SAFETY: 快照全部行程；失敗時 windows crate 回 Err。
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
        .map_err(|e| format!("CreateToolhelp32Snapshot 失敗：{e}"))?;
    let mut entry = PROCESSENTRY32W {
        dwSize: u32::try_from(std::mem::size_of::<PROCESSENTRY32W>()).unwrap_or(0),
        ..Default::default()
    };
    let mut found = None;
    // SAFETY: `snap` 是上面成功建立的快照把手；`entry` 為局部變數且 dwSize 已設。
    let mut ok = unsafe { Process32FirstW(snap, &mut entry) }.is_ok();
    while ok {
        if entry.th32ProcessID == pid {
            found = Some(entry.th32ParentProcessID);
            break;
        }
        // SAFETY: 同上。
        ok = unsafe { Process32NextW(snap, &mut entry) }.is_ok();
    }
    // SAFETY: 本函式建立的快照把手，只關一次。
    let _ = unsafe { CloseHandle(snap) };
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    /// 實機：對自己啟動的子行程（`ping -n 30`，約 30 秒後自行結束）開把手——讀得到父 PID＝本行程、
    /// 命令列含參數；未結束時 `wait_exit(0)` 為 false；`terminate` 後 `wait_exit` 為 true。
    #[test]
    fn handle_reads_identity_waits_and_terminates_own_child() {
        let mut child = Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("啟動 ping");
        let h = ProcessHandle::open(child.id()).expect("開把手");
        assert_eq!(h.pid(), child.id());
        assert!(!h.wait_exit(Duration::ZERO), "剛啟動不應已結束");
        let id = h.identity().expect("身分");
        assert_eq!(id.parent_pid, Some(std::process::id()));
        assert!(id.command_line.contains("127.0.0.1"), "{id:?}");
        h.terminate().expect("結束");
        assert!(h.wait_exit(Duration::from_secs(5)));
        let _ = child.wait();
    }

    #[test]
    fn open_rejects_pid_zero() {
        assert!(ProcessHandle::open(0).is_err());
    }
}
