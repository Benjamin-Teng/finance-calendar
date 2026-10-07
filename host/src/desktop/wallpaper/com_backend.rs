//! 真實桌布後端：`IDesktopWallpaper`（COM）（dynamic-wallpaper task 4.2）。
//!
//! 只在 [`super::WallpaperService`] 的工作執行緒上建立與使用（介面不是 `Send`）。呼叫方式照搬
//! 探針 `examples/probe_wallpaper.rs`（tasks 1.1／1.2 在 STA 下實機驗證列舉、讀取、設定與介面
//! 重建；本後端改用 MTA，MTA 下目前只驗證過唯讀路徑，寫入路徑待 4.4／6.1 實機驗收）。投影片的
//! 讀取與還原是探針沒有涵蓋的部分，見 [`ComBackend::restore_slideshow`]。登錄與焦點狀態不在這裡，
//! 見 [`super::registry`]。
//!
//! 單元測試不使用本型別（見 `tests.rs`）；唯讀的人工驗收見其中的 `#[ignore]` 測試。

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::core::{Interface, BOOL, PCWSTR, PWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, RECT};
#[cfg(test)]
use windows::Win32::System::Com::COINIT_APARTMENTTHREADED;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED,
};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    DesktopWallpaper, IDesktopWallpaper, ILCreateFromPathW, ILFree, IShellItemArray,
    SHCreateShellItemArrayFromIDLists, DESKTOP_SLIDESHOW_OPTIONS, DESKTOP_WALLPAPER_POSITION,
    SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumThreadWindows, FindWindowExW, GetAncestor, GetClassNameW, GetDesktopWindow, GetShellWindow,
    GetSystemMetrics, GetWindowThreadProcessId, GA_PARENT, HWND_MESSAGE, SM_CMONITORS,
};

use super::{
    assemble_monitor_list, decode_wide, slideshow_applicable, BackendError, MonitorEntry,
    MonitorProbe, MonitorWallpaper, SlideshowInfo, WallpaperBackend, WallpaperPosition,
    WallpaperSnapshot,
};

/// 真實桌布後端。建立時 `CoInitializeEx(COINIT_MULTITHREADED)` 一次（為何是 MTA 而非探針的
/// STA，見 `wallpaper.rs` 模組文件「COM apartment」一節）；介面延後到第一個請求前由工作
/// 執行緒呼叫 [`WallpaperBackend::recreate`] 建立。丟棄時先放掉介面再 `CoUninitialize`。
pub struct ComBackend {
    /// `CoInitializeEx` 是否成功（含 `S_FALSE`）；失敗時一律回 [`BackendError::Unavailable`]。
    com_initialized: bool,
    dw: Option<IDesktopWallpaper>,
}

impl ComBackend {
    /// 在**目前執行緒**初始化 COM。只應在工作執行緒上呼叫（[`super::WallpaperService::spawn_com`]
    /// 的工廠閉包）。
    pub fn new() -> Self {
        // SAFETY: 無指標參數；本執行緒是剛建立的工作執行緒，尚未以其他模式初始化 COM。成功
        // （含 S_FALSE）時由 Drop 對稱呼叫 CoUninitialize。
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr.is_err() {
            log::warn!("桌布 COM 執行緒 CoInitializeEx 失敗：{hr:?}");
        }
        Self {
            com_initialized: hr.is_ok(),
            dw: None,
        }
    }

    fn dw(&self) -> Result<&IDesktopWallpaper, BackendError> {
        self.dw.as_ref().ok_or_else(|| {
            BackendError::Unavailable("沒有可用的 IDesktopWallpaper 介面".to_owned())
        })
    }
}

impl Default for ComBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ComBackend {
    fn drop(&mut self) {
        // 介面必須在 CoUninitialize 之前釋放。
        self.dw = None;
        if self.com_initialized {
            // SAFETY: 與 new() 中成功的 CoInitializeEx 對稱，且在同一執行緒（後端不是 Send，
            // 只會在建立它的工作執行緒上被丟棄）。
            unsafe { CoUninitialize() };
        }
    }
}

fn com_err(what: &str, e: windows::core::Error) -> BackendError {
    BackendError::Com(format!("{what} 失敗：{e}"))
}

/// 以 NUL 結尾的 UTF-16。
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 路徑轉成以 NUL 結尾的 UTF-16，**不經** `to_string_lossy`（無法以 UTF-8 表示的檔名照樣忠實
/// 傳給 Windows）。
fn wide_path(p: &Path) -> Vec<u16> {
    p.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// 取出 COM 配置的字串並以 `CoTaskMemFree` 釋放。無效 UTF-16 回 [`BackendError::Invalid`]
/// （絕不變成空字串，見 [`decode_wide`]）；空指標回空字串。記憶體無論成敗都會釋放。
fn pwstr_take(what: &str, p: PWSTR) -> Result<String, BackendError> {
    if p.is_null() {
        return Ok(String::new());
    }
    // SAFETY: p 非空、由 COM 方法以 CoTaskMemAlloc 配置且以 NUL 結尾；as_wide 借用的切片在
    // 下方釋放前就已解碼完畢。
    let decoded = decode_wide(what, unsafe { p.as_wide() });
    // SAFETY: 同上，p 由 CoTaskMemAlloc 配置，此後不再使用。
    unsafe { CoTaskMemFree(Some(p.0 as *const c_void)) };
    decoded
}

/// 列舉螢幕（含已拔除者）。`GetMonitorRECT` 直接呼叫 vtable 取原始 HRESULT（windows-rs 的包裝會
/// 把 `S_FALSE` 當成功吞掉；探針做法），逐台收集後連同系統作用中的顯示器數
/// （`GetSystemMetrics(SM_CMONITORS)`）交給 [`assemble_monitor_list`]：某台不在線（`S_FALSE` 或失敗；
/// 實機拔除回 `E_FAIL`）、而在線台數已達作用中顯示器數時只讓那台算離線，不讓整批失敗；否則（作用中
/// 螢幕的暫時錯誤、插回後 explorer 落後）、介面斷線或全部失敗，整個回 [`BackendError::Com`]（觸發重建，
/// 呼叫端退避重試）。
fn list_monitors(dw: &IDesktopWallpaper) -> Result<Vec<MonitorEntry>, BackendError> {
    // SAFETY: dw 是有效介面；方法無指標參數。
    let count = unsafe { dw.GetMonitorDevicePathCount() }
        .map_err(|e| com_err("GetMonitorDevicePathCount", e))?;
    let mut probes = Vec::with_capacity(count as usize);
    for i in 0..count {
        // SAFETY: i < count；回傳的 PWSTR 由 pwstr_take 釋放。
        let raw = unsafe { dw.GetMonitorDevicePathAt(i) }
            .map_err(|e| com_err(&format!("GetMonitorDevicePathAt({i})"), e))?;
        let path = pwstr_take("螢幕裝置路徑", raw)?;
        let path_w = wide(&path);
        let mut rect = RECT::default();
        // SAFETY: vtable 取自有效介面；path_w 以 NUL 結尾且在呼叫期間存活；rect 為可寫的區域
        // 變數。
        let hr = unsafe {
            (Interface::vtable(dw).GetMonitorRECT)(
                Interface::as_raw(dw),
                PCWSTR(path_w.as_ptr()),
                &mut rect,
            )
        };
        probes.push(MonitorProbe {
            device_path: path,
            hr: hr.0,
            ltrb: [rect.left, rect.top, rect.right, rect.bottom],
        });
    }
    // 系統作用中的顯示器數：在逐台查詢之後讀，取最新的拓樸（拔除時系統先更新、explorer 之後才回
    // E_FAIL；插回時系統先更新、explorer 落後）。0＝讀不到。
    // SAFETY: 無指標參數，任何執行緒都可呼叫。
    let active = unsafe { GetSystemMetrics(SM_CMONITORS) };
    assemble_monitor_list(probes, u32::try_from(active).ok().filter(|n| *n > 0))
}

/// 讀投影片設定（只在 [`slideshow_applicable`] 時呼叫）。任何 COM 失敗回 `Com`；沒有項目、
/// 或項目不是檔案系統路徑（無法忠實還原）回 `Invalid`——兩者都代表「記錄失敗」。
fn read_slideshow(dw: &IDesktopWallpaper) -> Result<SlideshowInfo, BackendError> {
    let mut options = DESKTOP_SLIDESHOW_OPTIONS::default();
    let mut tick: u32 = 0;
    // SAFETY: 兩個輸出指標指向可寫的區域變數。
    unsafe { dw.GetSlideshowOptions(&mut options, &mut tick) }
        .map_err(|e| com_err("GetSlideshowOptions", e))?;
    // SAFETY: dw 是有效介面。
    let arr: IShellItemArray =
        unsafe { dw.GetSlideshow() }.map_err(|e| com_err("GetSlideshow", e))?;
    // SAFETY: arr 是有效介面。
    let count = unsafe { arr.GetCount() }.map_err(|e| com_err("IShellItemArray::GetCount", e))?;
    if count == 0 {
        return Err(BackendError::Invalid(
            "狀態為投影片，但 GetSlideshow 沒有任何項目".to_owned(),
        ));
    }
    let mut items = Vec::with_capacity(count as usize);
    for i in 0..count {
        // SAFETY: i < count。
        let item = unsafe { arr.GetItemAt(i) }
            .map_err(|e| com_err(&format!("IShellItemArray::GetItemAt({i})"), e))?;
        // SAFETY: item 是有效介面；回傳的 PWSTR 由 pwstr_take 釋放。
        let raw = unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) }.map_err(|e| {
            BackendError::Invalid(format!(
                "投影片項目 {i} 不是檔案系統路徑（GetDisplayName 失敗：{e}），無法忠實記錄"
            ))
        })?;
        items.push(pwstr_take("投影片項目路徑", raw)?);
    }
    Ok(SlideshowInfo {
        items,
        options: options.0,
        tick_ms: tick,
    })
}

/// 從檔案系統路徑建立 `IShellItemArray`（`ILCreateFromPathW`＋`SHCreateShellItemArrayFromIDLists`）。
/// 建立的物件留在本執行緒的 MTA（不是經 `CoCreateInstance` 建立的 apartment 類別，不會被搬進
/// 系統代管的 STA）；explorer 之後對它的回呼由本行程的 RPC 執行緒處理。
fn shell_item_array(paths: &[String]) -> Result<IShellItemArray, BackendError> {
    if paths.is_empty() {
        return Err(BackendError::Invalid("投影片沒有任何項目".to_owned()));
    }
    let mut pidls: Vec<*mut ITEMIDLIST> = Vec::with_capacity(paths.len());
    let mut bad = None;
    for p in paths {
        let w = wide(p);
        // SAFETY: w 以 NUL 結尾且在呼叫期間存活；回傳的 PIDL 由下方 ILFree 釋放。
        let pidl = unsafe { ILCreateFromPathW(PCWSTR(w.as_ptr())) };
        if pidl.is_null() {
            bad = Some(p.clone());
            break;
        }
        pidls.push(pidl);
    }
    let result = match bad {
        Some(p) => Err(BackendError::Invalid(format!("無法解析投影片路徑：{p}"))),
        None => {
            let consts: Vec<*const ITEMIDLIST> = pidls.iter().map(|p| p.cast_const()).collect();
            // SAFETY: consts 內每個 PIDL 都非空且在呼叫期間有效（ILFree 在之後）。
            unsafe { SHCreateShellItemArrayFromIDLists(&consts) }.map_err(|e| {
                BackendError::Invalid(format!("SHCreateShellItemArrayFromIDLists 失敗：{e}"))
            })
        }
    };
    for p in pidls {
        // SAFETY: p 由 ILCreateFromPathW 配置、非空，只釋放一次；IShellItemArray 已自行複製。
        unsafe { ILFree(Some(p.cast_const())) };
    }
    result
}

impl WallpaperBackend for ComBackend {
    fn is_ready(&self) -> bool {
        self.com_initialized && self.dw.is_some()
    }

    fn recreate(&mut self) -> Result<(), BackendError> {
        self.dw = None;
        if !self.com_initialized {
            return Err(BackendError::Unavailable(
                "本執行緒 CoInitializeEx 失敗".to_owned(),
            ));
        }
        // SAFETY: 本執行緒已初始化 COM；CLSID 為系統定義的常數。
        let dw: IDesktopWallpaper =
            unsafe { CoCreateInstance(&DesktopWallpaper, None, CLSCTX_ALL) }.map_err(|e| {
                BackendError::Unavailable(format!("CoCreateInstance(DesktopWallpaper) 失敗：{e}"))
            })?;
        self.dw = Some(dw);
        Ok(())
    }

    fn explorer_pid(&self) -> Option<u32> {
        // SAFETY: 無參數；explorer 重啟中沒有殼層視窗時回空 HWND。不等待 explorer 回應。
        let hwnd = unsafe { GetShellWindow() };
        if hwnd.0.is_null() {
            return None;
        }
        let mut pid = 0u32;
        // SAFETY: hwnd 可能剛好失效，此時函式回 0、pid 不變；pid 為可寫區域變數。
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        (pid != 0).then_some(pid)
    }

    fn list_monitors(&mut self) -> Result<Vec<MonitorEntry>, BackendError> {
        list_monitors(self.dw()?)
    }

    /// 嚴格讀取：在線螢幕的 `GetWallpaper`、`GetPosition`、`GetBackgroundColor`、`GetStatus`，
    /// 以及（投影片適用時）投影片設定，任一失敗即整個回錯。離線螢幕的 `GetWallpaper` 失敗只記為
    /// `None`（離線螢幕本來就不設定）。
    fn read(&mut self) -> Result<WallpaperSnapshot, BackendError> {
        let dw = self.dw()?;
        let mut monitors = Vec::new();
        for m in list_monitors(dw)? {
            let path_w = wide(&m.device_path);
            // SAFETY: path_w 以 NUL 結尾且在呼叫期間存活；回傳的 PWSTR 由 pwstr_take 釋放。
            let raw = unsafe { dw.GetWallpaper(PCWSTR(path_w.as_ptr())) };
            let wallpaper = if m.is_online() {
                let raw = raw.map_err(|e| com_err("GetWallpaper（在線螢幕）", e))?;
                Some(pwstr_take("桌布路徑", raw)?)
            } else {
                raw.ok().and_then(|p| pwstr_take("桌布路徑", p).ok())
            };
            monitors.push(MonitorWallpaper {
                monitor: m,
                wallpaper,
            });
        }
        // SAFETY（以下三個呼叫）：dw 是有效介面，方法沒有指標參數。
        let position = unsafe { dw.GetPosition() }.map_err(|e| com_err("GetPosition", e))?;
        let background_color =
            unsafe { dw.GetBackgroundColor() }.map_err(|e| com_err("GetBackgroundColor", e))?;
        let status = unsafe { dw.GetStatus() }.map_err(|e| com_err("GetStatus", e))?;
        let slideshow = if slideshow_applicable(status.0) {
            Some(read_slideshow(dw)?)
        } else {
            None
        };
        Ok(WallpaperSnapshot {
            monitors,
            position: WallpaperPosition(position.0),
            background_color: background_color.0,
            slideshow_status: status.0,
            slideshow,
        })
    }

    fn set_wallpaper(&mut self, device_path: &str, image: &Path) -> Result<(), BackendError> {
        let dw = self.dw()?;
        let id_w = wide(device_path);
        let image_w = wide_path(image);
        // SAFETY: 兩個寬字串以 NUL 結尾且在呼叫期間存活。
        unsafe { dw.SetWallpaper(PCWSTR(id_w.as_ptr()), PCWSTR(image_w.as_ptr())) }
            .map_err(|e| com_err("SetWallpaper", e))
    }

    /// `SetWallpaper(NULL, 圖)`：monitorID 傳空指標＝「全部螢幕」設定（不產生逐螢幕轉存檔、登錄寫成原圖路徑）。
    fn set_wallpaper_all(&mut self, image: &Path) -> Result<(), BackendError> {
        let dw = self.dw()?;
        let image_w = wide_path(image);
        // SAFETY: monitorID 為空指標（API 定義為「所有螢幕」）；image_w 以 NUL 結尾且在呼叫期間存活。
        unsafe { dw.SetWallpaper(PCWSTR::null(), PCWSTR(image_w.as_ptr())) }
            .map_err(|e| com_err("SetWallpaper(NULL, 圖)", e))
    }

    /// `SetWallpaper(NULL, "")`：monitorID 傳空指標＝所有螢幕（6.1 實機：只有這樣才會回到純色）。
    fn set_solid_color(&mut self) -> Result<(), BackendError> {
        let dw = self.dw()?;
        let empty_w = wide("");
        // SAFETY: monitorID 為空指標（API 定義為「所有螢幕」）；empty_w 以 NUL 結尾且在呼叫期間存活。
        unsafe { dw.SetWallpaper(PCWSTR::null(), PCWSTR(empty_w.as_ptr())) }
            .map_err(|e| com_err("SetWallpaper(NULL, \"\")", e))
    }

    fn set_position(&mut self, position: WallpaperPosition) -> Result<(), BackendError> {
        // SAFETY: 無指標參數。
        unsafe {
            self.dw()?
                .SetPosition(DESKTOP_WALLPAPER_POSITION(position.0))
        }
        .map_err(|e| com_err("SetPosition", e))
    }

    fn set_background_color(&mut self, colorref: u32) -> Result<(), BackendError> {
        // SAFETY: 無指標參數。
        unsafe { self.dw()?.SetBackgroundColor(COLORREF(colorref)) }
            .map_err(|e| com_err("SetBackgroundColor", e))
    }

    /// 還原投影片：`SetSlideshow`（項目路徑）→ `SetSlideshowOptions`（旗標與間隔）。
    ///
    /// 探針沒有實測投影片還原（本機原桌布不是投影片），MTA 下的寫入也尚無實機證據；呼叫端應以
    /// [`Self::read`] 讀回 `slideshow_status`／`slideshow` 確認。路徑無法解析時回
    /// [`BackendError::Invalid`]（不觸發重建）。
    fn restore_slideshow(&mut self, slideshow: &SlideshowInfo) -> Result<(), BackendError> {
        let dw = self.dw()?;
        let arr = shell_item_array(&slideshow.items)?;
        // SAFETY: arr 是有效介面。
        unsafe { dw.SetSlideshow(&arr) }.map_err(|e| com_err("SetSlideshow", e))?;
        // SAFETY: 無指標參數。
        unsafe {
            dw.SetSlideshowOptions(
                DESKTOP_SLIDESHOW_OPTIONS(slideshow.options),
                slideshow.tick_ms,
            )
        }
        .map_err(|e| com_err("SetSlideshowOptions", e))
    }
}

// ---------------------------------------------------------------------------------------------
// 診斷：執行緒擁有的視窗（fix round 1：驗證工作執行緒不擁有任何視窗）
// ---------------------------------------------------------------------------------------------

/// 某執行緒擁有的一扇視窗。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadWindow {
    /// 視窗類別名稱（例如 STA 的 `OleMainThreadWndClass`）。
    pub class: String,
    /// 是否為 message-only 視窗（父視窗是 `HWND_MESSAGE` 根，類別 `Message`）。
    pub message_only: bool,
}

fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    // SAFETY: buf 為可寫緩衝區，長度即傳入的切片長度。
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn describe(hwnd: HWND) -> ThreadWindow {
    // SAFETY: hwnd 可能已失效，此時 GetAncestor 回空 HWND；無其他前置條件。
    let parent = unsafe { GetAncestor(hwnd, GA_PARENT) };
    // SAFETY: 無參數。
    let desktop = unsafe { GetDesktopWindow() };
    ThreadWindow {
        class: class_name(hwnd),
        message_only: !parent.0.is_null() && parent != desktop && class_name(parent) == "Message",
    }
}

unsafe extern "system" fn collect_hwnd(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam 是 thread_windows 內 `&mut Vec<HWND>` 的位址，在 EnumThreadWindows 期間有效。
    let out = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
    out.push(hwnd);
    BOOL(1)
}

/// 列出 `thread_id` 擁有的全部視窗：`EnumThreadWindows`（非子視窗），再以
/// `FindWindowExW(HWND_MESSAGE, …)` 逐一走 message-only 視窗、比對擁有者執行緒補上（兩者聯集，
/// 不確定 `EnumThreadWindows` 是否涵蓋 message-only 視窗）。
pub fn thread_windows(thread_id: u32) -> Vec<ThreadWindow> {
    let mut hwnds: Vec<HWND> = Vec::new();
    // SAFETY: 回呼只把 HWND 推進 hwnds；lparam 指向的 Vec 在呼叫期間存活且不被其他人存取。
    unsafe {
        let _ = EnumThreadWindows(
            thread_id,
            Some(collect_hwnd),
            LPARAM(&mut hwnds as *mut Vec<HWND> as isize),
        );
    }
    let mut after: Option<HWND> = None;
    // 上限防護：message-only 視窗全系統數量有限，避免異常時無窮迴圈。
    for _ in 0..100_000 {
        // SAFETY: 父視窗為 HWND_MESSAGE 常數，類別與標題傳 NULL＝不篩選。
        let Ok(next) = (unsafe { FindWindowExW(Some(HWND_MESSAGE), after, None, None) }) else {
            break;
        };
        if next.0.is_null() {
            break;
        }
        let mut pid = 0u32;
        // SAFETY: pid 為可寫區域變數；next 若剛好失效函式回 0。
        let owner = unsafe { GetWindowThreadProcessId(next, Some(&mut pid)) };
        if owner == thread_id && !hwnds.contains(&next) {
            hwnds.push(next);
        }
        after = Some(next);
    }
    hwnds.into_iter().map(describe).collect()
}

/// 對照組（只給 `#[ignore]` 的唯讀驗收用）：在一條**新的 STA** 執行緒上建立 `IDesktopWallpaper`、
/// 做一次唯讀呼叫，列出該執行緒擁有的視窗，證明 [`thread_windows`] 偵測得到 STA 的 OLE 隱藏視窗。
#[cfg(test)]
pub(super) fn sta_control_thread_windows() -> Result<Vec<ThreadWindow>, String> {
    std::thread::spawn(|| {
        // SAFETY: 新執行緒、尚未初始化 COM；成功時下方對稱 CoUninitialize。
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if hr.is_err() {
            return Err(format!("CoInitializeEx(STA) 失敗：{hr:?}"));
        }
        let result = (|| {
            // SAFETY: 本執行緒已初始化 COM。
            let dw: IDesktopWallpaper =
                unsafe { CoCreateInstance(&DesktopWallpaper, None, CLSCTX_ALL) }
                    .map_err(|e| format!("CoCreateInstance 失敗：{e}"))?;
            // SAFETY: dw 是有效介面；唯讀呼叫。
            unsafe { dw.GetMonitorDevicePathCount() }
                .map_err(|e| format!("GetMonitorDevicePathCount 失敗：{e}"))?;
            Ok(thread_windows(crate::desktop::current_thread_id()))
        })();
        // SAFETY: 與上方成功的 CoInitializeEx 對稱，同一執行緒；介面已在閉包內釋放。
        unsafe { CoUninitialize() };
        result
    })
    .join()
    .map_err(|_| "對照組執行緒 panic".to_owned())?
}
