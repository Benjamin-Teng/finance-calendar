//! 勿打擾的兩個系統來源（dynamic-wallpaper task 4.7c；design.md D12）。只做讀取，**不切換**任何
//! 勿打擾／專注設定。判讀（真／假／未知）與合併在 `crate::wallpaper_dnd`，本模組只回原始讀值。
//!
//! ## 官方：`Windows.UI.Shell.FocusSessionManager`（WinRT）
//!
//! [`read_focus_active`]：`FocusSessionManager::IsSupported()` 為真才 `GetDefault()?.IsFocusActive()`。
//! windows crate 0.62.2 的宣告查證於 `~/.cargo/registry/src/*/windows-0.62.2/src/Windows/UI/Shell/mod.rs`：
//! `FocusSessionManager::IsSupported() -> Result<bool>`、`GetDefault() -> Result<FocusSessionManager>`、
//! `IsFocusActive(&self) -> Result<bool>`（另有 `IsFocusActiveChanged` 事件，本 task 不用，見
//! `wallpaper_dnd` 模組文件），三者都沒有額外的 `cfg(feature)` 把關，只需要 crate feature
//! `UI_Shell`（`Cargo.toml`：`UI_Shell = ["UI"]`、`UI = ["Foundation"]`）。
//!
//! WinRT 啟用需要呼叫端執行緒已初始化 COM／WinRT 公寓：輪詢執行緒建立時呼叫一次
//! [`init_mta_for_this_thread`]（`CoInitializeEx(COINIT_MULTITHREADED)`，效果等同
//! `RoInitialize(RO_INIT_MULTITHREADED)`，且沿用已啟用的 `Win32_System_Com`，不必另開
//! `Win32_System_WinRT`）。只影響輪詢執行緒自己，不碰桌布 COM 執行緒與主執行緒的公寓狀態。
//!
//! ## 非官方：WNF `WNF_SHEL_QUIETHOURS_ACTIVE_PROFILE_CHANGED`
//!
//! [`query_quiet_hours_wnf`]：`ntdll!NtQueryWnfStateData` 讀 state name [`WNF_QUIETHOURS_STATE_NAME`]
//! 的 4 位元組資料。**這是未公開的 API**：windows crate 0.62.2 與 windows-sys 0.61.2 都沒有這個函式的
//! binding（`grep -rn NtQueryWnfStateData` 於兩個 crate 的 `src/` 皆無結果），簽名照公開原始碼
//! （Workrave、YASB 等開源專案；本機參考 `%LOCALAPPDATA%\fc-dw-dnd\read-dnd.ps1` 的 P/Invoke 宣告）：
//!
//! ```text
//! NTSTATUS NtQueryWnfStateData(
//!     const WNF_STATE_NAME* StateName,   // 64 位元整數
//!     const WNF_TYPE_ID*    TypeId,      // 可為 NULL
//!     const VOID*           ExplicitScope, // 可為 NULL
//!     WNF_CHANGE_STAMP*     ChangeStamp, // ULONG
//!     VOID*                 Buffer,
//!     ULONG*                BufferSize); // 進：緩衝區大小；出：資料大小
//! ```
//!
//! 以 `GetModuleHandleW("ntdll.dll")`＋`GetProcAddress` **動態**取得（不靜態連結）：未來的 Windows
//! 若移除這個匯出，靜態連結會讓整個執行檔載入失敗；動態取得時只是這半邊變成「未知」。
//! `GetModuleHandleW`／`GetProcAddress` 在已啟用的 `Win32_System_LibraryLoader`。

use std::ffi::c_void;

use windows::core::{s, w};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::UI::Shell::FocusSessionManager;

/// `WNF_SHEL_QUIETHOURS_ACTIVE_PROFILE_CHANGED` 的 state name（非官方；Workrave／YASB 等公開原始碼
/// 與本機 `read-dnd.ps1` 使用同一個值）。資料是 4 位元組整數，> 0 表示勿打擾中。
pub const WNF_QUIETHOURS_STATE_NAME: u64 = 0x0D83_063E_A3BF_1C75;

/// `NtQueryWnfStateData` 的一次原始結果（判讀見 `wallpaper_dnd::interpret_wnf`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WnfRaw {
    /// 回傳的 NTSTATUS（0＝STATUS_SUCCESS）。
    pub status: i32,
    /// 呼叫後的 `BufferSize`（實際資料大小）。
    pub size: u32,
    /// 緩衝區內容（小端序）。
    pub data: [u8; 4],
}

/// `NtQueryWnfStateData` 的函式型別（非官方簽名，見模組文件）。
type NtQueryWnfStateDataFn = unsafe extern "system" fn(
    state_name: *const u64,
    type_id: *const c_void,
    explicit_scope: *const c_void,
    change_stamp: *mut u32,
    buffer: *mut c_void,
    buffer_size: *mut u32,
) -> i32;

/// 讀 WNF 勿打擾狀態一次。`Err`＝ntdll 或函式取不到（呼叫端視為未知）；呼叫成功但 NTSTATUS 非 0、
/// 大小不足一樣回 `Ok`，由呼叫端判讀。核心查詢，不送訊息給任何視窗，不會阻塞。
pub fn query_quiet_hours_wnf() -> Result<WnfRaw, String> {
    // SAFETY: 取已載入模組的把手（不增加參考計數）；ntdll 在每個 Win32 行程裡一定已載入。
    let ntdll = unsafe { GetModuleHandleW(w!("ntdll.dll")) }
        .map_err(|e| format!("取不到 ntdll.dll 的模組把手：{e}"))?;
    // SAFETY: `ntdll` 是上一行取得的有效模組把手，程序名稱是以 NUL 結尾的 ASCII 常值。
    let proc = unsafe { GetProcAddress(ntdll, s!("NtQueryWnfStateData")) }.ok_or_else(|| {
        "ntdll.dll 沒有 NtQueryWnfStateData 匯出（Windows 版本可能已改變）".to_owned()
    })?;
    // SAFETY: 依公開原始碼的簽名把匯出轉成正確的函式型別（見模組文件）；兩者都是 "system" 呼叫慣例的
    // 函式指標，大小相同。
    let query: NtQueryWnfStateDataFn = unsafe { std::mem::transmute(proc) };
    let state_name = WNF_QUIETHOURS_STATE_NAME;
    let mut change_stamp = 0u32;
    let mut data = [0u8; 4];
    let mut size = u32::try_from(data.len()).unwrap_or(4);
    // SAFETY: 所有指標都指向本函式的局部變數，在呼叫期間有效：state name 唯讀、change stamp 與大小
    // 可寫、緩衝區 4 位元組且 `size` 如實告知容量；TypeId／ExplicitScope 依公開用法傳 NULL。資料比
    // 緩衝區大時 API 回 STATUS_BUFFER_TOO_SMALL、不寫超出範圍。
    let status = unsafe {
        query(
            &state_name,
            std::ptr::null(),
            std::ptr::null(),
            &mut change_stamp,
            data.as_mut_ptr().cast(),
            &mut size,
        )
    };
    Ok(WnfRaw { status, size, data })
}

/// 讀官方專注工作階段狀態：`Ok(Some(b))`＝`IsFocusActive`；`Ok(None)`＝`IsSupported` 為假；`Err`＝
/// WinRT 啟用或呼叫失敗（一行說明）。呼叫端執行緒須先 [`init_mta_for_this_thread`]。
pub fn read_focus_active() -> Result<Option<bool>, String> {
    let supported = FocusSessionManager::IsSupported()
        .map_err(|e| format!("FocusSessionManager.IsSupported 失敗：{e}"))?;
    if !supported {
        return Ok(None);
    }
    let manager = FocusSessionManager::GetDefault()
        .map_err(|e| format!("FocusSessionManager.GetDefault 失敗：{e}"))?;
    manager
        .IsFocusActive()
        .map(Some)
        .map_err(|e| format!("FocusSessionManager.IsFocusActive 失敗：{e}"))
}

/// 本執行緒的 MTA 初始化（成功時丟棄會對稱 `CoUninitialize`）。不是 `Send`：必須在同一條執行緒上
/// 建立與丟棄。
pub struct MtaGuard {
    _not_send: std::marker::PhantomData<*const ()>,
}

impl Drop for MtaGuard {
    fn drop(&mut self) {
        // SAFETY: 只有 `CoInitializeEx` 成功（含 S_FALSE）才會建立 `MtaGuard`，且它不是 Send，丟棄時仍
        // 在同一條執行緒上，與那次初始化對稱。
        unsafe { CoUninitialize() };
    }
}

/// 在**呼叫端執行緒**上以 MTA 初始化 COM／WinRT（`CoInitializeEx(COINIT_MULTITHREADED)`）。失敗回
/// 一行說明（呼叫端把官方來源視為未知，WNF 來源不受影響）。
pub fn init_mta_for_this_thread() -> Result<MtaGuard, String> {
    // SAFETY: 保留參數為 None；只影響呼叫端執行緒。成功（含 S_FALSE）由 MtaGuard 的 Drop 對稱解除。
    let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if hr.is_ok() {
        Ok(MtaGuard {
            _not_send: std::marker::PhantomData,
        })
    } else {
        Err(format!("CoInitializeEx(COINIT_MULTITHREADED) 失敗：{hr:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wallpaper_dnd::{interpret_focus, interpret_wnf};

    // 修正輪 1（審查 low2）：兩個本機實讀測試**照常執行、不斷言讀值或成敗**——讀值與成敗隨 Windows 版本
    // 與使用者設定檔而變（舊版 Windows 10 沒有 FocusSessionManager 類別；從未發佈過這個 WNF 的設定檔
    // 回 size 0），那些情況正式程式本來就判成「未知」，不是錯誤。保留執行（而非 #[ignore]）是因為
    // 呼叫本身仍值得每台機器都跑：GetProcAddress＋transmute 的簽名或指標寫錯會讓測試行程當掉，
    // 這是與環境無關的回歸訊號。只斷言與環境無關的不變式：不 panic、結果可交給 `interpret_*` 判讀成三態之一、成功時資料
    // 大小不超過緩衝區、新執行緒的 MTA 初始化成功。讀值以 `--nocapture` 印出（task 4.7c 報告記錄了
    // 勿打擾關閉時的讀值）。

    /// 本機唯讀實讀 WNF（不改任何設定）。
    #[test]
    fn wnf_quiet_hours_query_never_panics_and_is_interpretable() {
        let raw = query_quiet_hours_wnf();
        if let Ok(r) = &raw {
            println!(
                "WNF_SHEL_QUIETHOURS_ACTIVE_PROFILE_CHANGED：status=0x{:08X} size={} value={}",
                r.status as u32,
                r.size,
                i32::from_le_bytes(r.data)
            );
            if r.status == 0 {
                assert!(r.size <= 4, "成功時資料大小不得超過緩衝區：{}", r.size);
            }
        }
        let sample = interpret_wnf(raw);
        println!("判讀：{:?} {}", sample.tri, sample.detail);
    }

    /// 本機唯讀實讀官方來源（在專屬執行緒上初始化 MTA，不影響測試執行緒）。
    #[test]
    fn focus_session_manager_read_never_panics_and_is_interpretable() {
        let result = std::thread::spawn(|| {
            let _mta = init_mta_for_this_thread().expect("新執行緒的 MTA 初始化應成功");
            read_focus_active()
        })
        .join()
        .expect("讀取執行緒 panic");
        println!("FocusSessionManager：{result:?}");
        let sample = interpret_focus(result);
        println!("判讀：{:?} {}", sample.tri, sample.detail);
    }
}
