//! explorer 資源安全閥的讀值（dynamic-wallpaper task 4.9；design.md D11）。
//!
//! [`read_explorer_gdi`]：殼層視窗（`GetShellWindow`）的擁有者 PID（`GetWindowThreadProcessId`）→
//! `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` → `GetGuiResources(GR_GDIOBJECTS)` →
//! `CloseHandle`。四個都是核心／win32k 的查詢，**不送任何訊息給 explorer**，explorer 卡住時也不會
//! 阻塞，所以協調迴圈可以在每次設定桌布前直接呼叫（不必經桌布 COM 執行緒）。
//!
//! windows crate 0.62.2 的簽名查證於 `~/.cargo/registry/src/*/windows-0.62.2/src/Windows/Win32/`：
//! - `System/Threading/mod.rs`：`OpenProcess(PROCESS_ACCESS_RIGHTS, bool, u32) -> Result<HANDLE>`
//!   （無效把手轉成 `Error::from_thread`）、`GetGuiResources(HANDLE, GET_GUI_RESOURCES_FLAGS) -> u32`、
//!   `GR_GDIOBJECTS`、`PROCESS_QUERY_LIMITED_INFORMATION`——都在已啟用的 `Win32_System_Threading`；
//! - `Foundation/mod.rs`：`CloseHandle(HANDLE) -> Result<()>`；
//! - `UI/WindowsAndMessaging/mod.rs`：`GetShellWindow() -> HWND`、
//!   `GetWindowThreadProcessId(HWND, Option<*mut u32>) -> u32`。
//!
//! `GetGuiResources` 失敗時回 0（Microsoft Learn〈GetGuiResources function〉：「If the function
//! fails, the return value is zero」），explorer 的 GDI 物件數不會真的是 0，所以 0 一律當失敗。

use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Threading::{
    GetGuiResources, OpenProcess, GR_GDIOBJECTS, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{GetShellWindow, GetWindowThreadProcessId};

/// 一次讀值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExplorerGdiReading {
    /// 殼層視窗擁有者（explorer）的 PID。
    pub pid: u32,
    /// `GetGuiResources(GR_GDIOBJECTS)`。
    pub gdi: u32,
}

/// 讀 explorer（殼層視窗擁有者）目前的 GDI 物件數。失敗回傳一行中文說明（記錄用）：殼層視窗不
/// 存在（explorer 重新啟動中）、取不到 PID、`OpenProcess` 或 `GetGuiResources` 失敗。
pub fn read_explorer_gdi() -> Result<ExplorerGdiReading, String> {
    // SAFETY: 無參數；explorer 重新啟動中沒有殼層視窗時回空 HWND。不等待 explorer 回應。
    let hwnd = unsafe { GetShellWindow() };
    if hwnd.0.is_null() {
        return Err("沒有殼層視窗（explorer 可能正在重新啟動）".to_owned());
    }
    let mut pid = 0u32;
    // SAFETY: `pid` 是本函式的局部可寫變數，API 只寫入這個 u32；hwnd 剛好失效時回 0、pid 不變。
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == 0 {
        return Err("取不到殼層視窗的擁有者 PID".to_owned());
    }
    // SAFETY: 只要求「有限查詢」權限、不繼承；失敗時 windows crate 回 Err（不會給無效把手）。
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .map_err(|e| format!("OpenProcess(PID {pid}) 失敗：{e}"))?;
    // SAFETY: `handle` 是上一行成功開啟、帶 PROCESS_QUERY_LIMITED_INFORMATION 的行程把手。
    let gdi = unsafe { GetGuiResources(handle, GR_GDIOBJECTS) };
    // 失敗時先取錯誤碼，再關把手（CloseHandle 會改寫執行緒的最後錯誤）。
    let error = (gdi == 0).then(windows::core::Error::from_thread);
    // SAFETY: `handle` 由本函式開啟、只關一次；之後不再使用。
    if let Err(e) = unsafe { CloseHandle(handle) } {
        log::warn!("關閉 explorer（PID {pid}）的行程把手失敗：{e}");
    }
    match error {
        Some(e) => Err(format!(
            "GetGuiResources(PID {pid}, GR_GDIOBJECTS) 回 0：{e}"
        )),
        None => Ok(ExplorerGdiReading { pid, gdi }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 實機唯讀查詢一次（不改任何東西）：有殼層時讀值應大於 0，沒有殼層（例如服務工作階段）時是
    /// 可讀的錯誤而不是 panic。
    #[test]
    fn reading_explorer_gdi_returns_a_positive_count_or_a_readable_error() {
        match read_explorer_gdi() {
            Ok(r) => {
                assert!(r.pid != 0);
                assert!(r.gdi > 0, "{r:?}");
            }
            Err(e) => assert!(!e.is_empty()),
        }
    }
}
