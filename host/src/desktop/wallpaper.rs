//! 桌布 COM 執行緒（dynamic-wallpaper task 4.2；design.md D3／D4）。
//!
//! ## 為什麼要專屬執行緒
//!
//! `IDesktopWallpaper` 由 explorer 承載（跨行程 COM）。explorer 卡住時，任何呼叫都可能無限期
//! 阻塞。本模組把介面關在**一條專屬執行緒**裡：該執行緒 `CoInitializeEx` 一次、持有介面，以
//! channel 佇列接收請求，結果再經 channel 回傳。呼叫端（排程器 4.3、狀態機 4.4）只透過
//! [`WallpaperService`] 送請求，並以逾時等待（預設 [`DEFAULT_REQUEST_TIMEOUT`]＝10 秒）。
//!
//! 登錄（`HKCU\Control Panel\Desktop` 三個值）與 Windows 焦點狀態**不經過**這條執行緒：它們不碰
//! explorer，放在子模組 [`registry`] 的一般函式，任何執行緒都能直接呼叫。這樣 COM 執行緒忙碌或
//! 已死時，仍能把登錄寫回原值（探針驗證過的後備：下次登入時 explorer 依登錄載入原圖）。
//!
//! ## 逾時與「忙碌」
//!
//! - 逾時：該次回傳 [`WallpaperError::Timeout`]，並把執行緒標為「忙碌」。逾時當下若結果其實已
//!   送達（與逾時同時發生），先取回結果、不宣告逾時。
//! - 忙碌期間（前一個逾時的請求還沒返回），新請求**立即**回傳 [`WallpaperError::Busy`]，
//!   不送進佇列——不排隊堆積、不阻塞呼叫端。
//! - 前一個請求最終返回時，工作執行緒自動解除忙碌（記一行 info）。
//! - 逾時的請求若還在佇列裡沒開始（多個呼叫端並行時才會發生），工作執行緒輪到它時直接略過，
//!   不會在 explorer 恢復後才執行一個呼叫端早已放棄的舊請求。
//! - 每個請求在工作執行緒上的耗時（含必要的介面重建）超過 [`DEFAULT_SLOW_THRESHOLD`]（1 秒）
//!   記警告，作為 explorer 異常的早期訊號。
//!
//! ## 不得在主執行緒呼叫
//!
//! 請求方法最長會等滿逾時。主執行緒（Tauri 事件迴圈、同步指令）呼叫會凍結整個宿主，故
//! [`WallpaperService`] 的請求方法在主執行緒（std 命名為 `main` 的執行緒）上：debug 建置
//! `debug_assert!` 直接失敗，release 建置回 [`WallpaperError::WrongThread`]、不送出請求。
//! Tauri 的 async 指令跑在 async runtime 的工作執行緒上，被阻塞同樣不好；本防護不涵蓋它，
//! 4.3／4.7 的驗收要確認呼叫都發生在排程執行緒（或 `spawn_blocking`）上。
//!
//! ## 介面重建（依探針 `examples/probe_wallpaper.rs`）
//!
//! explorer 重啟後，手上的 `IDesktopWallpaper` 會變成斷線的 proxy，要重新 `CoCreateInstance`。
//! 每個請求開始前，工作執行緒依 [`RecreateTracker`] 判定是否先重建：
//!
//! - 還沒有介面（第一次使用，或上次建立失敗）；
//! - 上一個請求回報介面失效（[`BackendError::invalidates_interface`]）或 panic；
//! - explorer 的 PID（殼層視窗擁有者）與建立介面時不同。
//!
//! 重建失敗時該次請求照樣交給後端，由後端回 [`BackendError::Unavailable`]；下個請求再試。
//! 失敗的請求本身**不自動重試**（`SetWallpaper` 是否生效本來就要靠讀回確認，交給呼叫端在下一個
//! 時點重來）。
//!
//! ## COM apartment：MTA
//!
//! 工作執行緒以 `COINIT_MULTITHREADED` 初始化，不沿用探針的 STA。理由是 explorer **回呼**我們：
//! 還原投影片時把我們建立的 `IShellItemArray` 交給 explorer（`SetSlideshow`），explorer 之後對它
//! 的任何呼叫（含最後的 `Release`）都會進到本行程。在 STA 下這些呼叫要排進本執行緒的訊息佇列，
//! 而本執行緒閒置時阻塞在請求佇列上、不抽訊息——explorer 就會卡在等我們，正是專案離開 Lively 的
//! 失效鏈（跨行程同步呼叫打進一條不回應的執行緒；memory：`lively-explorer-crash-chain`、
//! `sync-wndproc-slow-work-blocks-sender`）。MTA 下這些回呼由 COM 的 RPC 執行緒直接處理，不需要
//! 本執行緒回應。`SHCreateShellItemArrayFromIDLists` 由 shell32 直接建立物件（不是經
//! `CoCreateInstance` 建立的 apartment 類別），物件就留在本 MTA，不會被搬進系統代管的 STA；它被
//! RPC 執行緒回呼時必須可多執行緒存取——這點與 MTA 下的寫入路徑一樣尚無實機證據，待 4.4／6.1
//! 驗收。
//!
//! 附帶結果：MTA 不建 OLE 隱藏視窗，工作執行緒擁有 0 扇視窗（STA 時會有一扇
//! `OleMainThreadWndClass`；它是 message-only、收不到廣播，所以「廣播被卡住」不是改用 MTA 的
//! 理由）。驗收：`tests.rs` 的 `#[ignore]` 唯讀測試以 `EnumThreadWindows`＋message-only 視窗
//! 列舉確認 0 扇，並以 STA 對照組證明這個檢查偵測得到 OLE 視窗。
//!
//! ## 給 4.4：「接管前記錄原桌布」的成敗判定
//!
//! - [`WallpaperService::read`] 回 `Ok` ＝ COM 部分記錄完整：在線螢幕的
//!   [`MonitorWallpaper::wallpaper`] 必為 `Some`（空字串＝純色）；`position`／`background_color`／
//!   `slideshow_status` 都是實際讀到的值；[`WallpaperSnapshot::slideshow`] 為 `Some` **若且唯若**
//!   `slideshow_status` 含 [`SLIDESHOW_STATE_SLIDESHOW`]。
//! - `read` 回**任何** `Err`（逾時、忙碌、COM 失敗、路徑含無效 UTF-16、投影片項目不是檔案系統
//!   路徑……）＝記錄失敗，**不得接管**，下個時點重試。
//! - 另外 [`registry::snapshot_desktop_registry`] 也必須成功；失敗同樣不得接管。
//! - 離線螢幕的 `wallpaper` 可能是 `None`（讀不到），離線螢幕本來就不設定，不影響判定。某台
//!   `GetMonitorRECT` 回 `S_FALSE` 或失敗、而在線台數已達系統作用中的顯示器數時，只讓那台算離線
//!   （[`assemble_monitor_list`]），`read` 不因此回錯（台數不足＝作用中螢幕的暫時錯誤或 explorer 落後，
//!   整個回錯）；離線螢幕的
//!   `GetWallpaper` 只是盡力讀（失敗記為 `None`、不影響成敗），讀到的值只用來保護正在顯示的備份檔。
//!
//! ## 注入假實作
//!
//! 所有 COM 呼叫都在 [`WallpaperBackend`] 的實作裡：真實後端是 [`ComBackend`]（子模組
//! `wallpaper/com_backend.rs`），單元測試與 4.3／4.4 以 [`WallpaperService::spawn_with`] 注入
//! 自己的後端。後端由工廠閉包**在工作執行緒上**建立（COM 介面不是 `Send`），故 trait 不要求
//! `Send`。登錄則以 [`registry::RegistryStore`] 注入假實作。
//!
//! 本模組只提供能力：宿主啟動時**不**建立這個服務、不做任何桌布動作（接線屬 4.3／4.4）。

use std::fmt;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use crate::layout::PhysicalRect;

mod com_backend;
pub mod file_identity;
pub mod pathcmp;
pub mod registry;
pub use com_backend::ComBackend;

/// 呼叫端等待單一請求的預設逾時（design.md D3）。
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// 工作執行緒上單一請求耗時超過此值即記警告（design.md D3）。
pub const DEFAULT_SLOW_THRESHOLD: Duration = Duration::from_secs(1);

/// 工作執行緒名稱（出現在 panic 記錄與除錯器）。
const THREAD_NAME: &str = "fc-wallpaper-com";

// ---------------------------------------------------------------------------------------------
// 資料型別（與平台無關，可在測試裡組字面量）
// ---------------------------------------------------------------------------------------------

/// 全域填滿方式（`DESKTOP_WALLPAPER_POSITION` 的原始值）。保留原始整數，讀到未知值也能原樣
/// 寫回。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WallpaperPosition(pub i32);

impl WallpaperPosition {
    pub const CENTER: Self = Self(0);
    pub const TILE: Self = Self(1);
    pub const STRETCH: Self = Self(2);
    pub const FIT: Self = Self(3);
    /// design.md D4：首次接管時設成這個（圖與螢幕同尺寸時為 1:1）。
    pub const FILL: Self = Self(4);
    pub const SPAN: Self = Self(5);
}

/// `IDesktopWallpaper` 列舉到的一台螢幕。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorEntry {
    /// `GetMonitorDevicePathAt` 的裝置路徑（也是 `SetWallpaper` 的 monitorID）。
    pub device_path: String,
    /// `GetMonitorRECT` 的矩形；`None`＝離線：回 `S_FALSE`（文件記載的已拔除仍被列舉）或失敗
    /// HRESULT（實機的拔除回 `E_FAIL`），且在線台數已達系統作用中的顯示器數。離線判定以 RECT 為準，
    /// 不以列舉數量。台數不足（作用中螢幕的暫時錯誤、插回後 explorer 落後）或介面斷線時不會出現在
    /// 這裡，整個列舉改回 [`BackendError::Com`]（見 [`assemble_monitor_list`]）。
    pub rect: Option<PhysicalRect>,
}

impl MonitorEntry {
    /// 是否在線（design.md D4：離線者每次重畫都略過）。
    pub fn is_online(&self) -> bool {
        self.rect.is_some()
    }
}

/// 一台螢幕與它目前的桌布。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorWallpaper {
    pub monitor: MonitorEntry,
    /// 逐螢幕 `GetWallpaper` 的讀回值（原路徑，不是 `TranscodedWallpaper`）；空字串＝沒有圖片
    /// （純色；純色是全域狀態，6.1 實機所有螢幕會一起讀回空字串，見
    /// [`WallpaperBackend::set_solid_color`]）。**在線螢幕必為 `Some`**（讀取失敗時整個 `read` 回錯）；離線螢幕讀不到時為
    /// `None`。
    pub wallpaper: Option<String>,
}

/// `GetStatus`（`DESKTOP_SLIDESHOW_STATE`）的旗標：投影片功能已啟用。
pub const SLIDESHOW_STATE_ENABLED: i32 = 0x1;
/// `GetStatus` 的旗標：目前設定為投影片。
pub const SLIDESHOW_STATE_SLIDESHOW: i32 = 0x2;
/// `GetStatus` 的旗標：遠端工作階段暫時停用投影片。
pub const SLIDESHOW_STATE_DISABLED_BY_REMOTE_SESSION: i32 = 0x4;
/// `DESKTOP_SLIDESHOW_OPTIONS` 的旗標：隨機播放。
pub const SLIDESHOW_OPTION_SHUFFLE: i32 = 0x1;

/// `GetStatus` 的旗標是否表示「目前設定為投影片」，也就是投影片設定必須記錄。
pub fn slideshow_applicable(status: i32) -> bool {
    status & SLIDESHOW_STATE_SLIDESHOW != 0
}

/// 投影片設定（還原用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlideshowInfo {
    /// `GetSlideshow` 各項目的檔案系統路徑（通常是單一資料夾）。
    pub items: Vec<String>,
    /// `GetSlideshowOptions` 的旗標（[`SLIDESHOW_OPTION_SHUFFLE`]）。
    pub options: i32,
    /// `GetSlideshowOptions` 的切換間隔（毫秒）。
    pub tick_ms: u32,
}

/// 一次「讀取」請求的結果（spec「接管前記錄原桌布」的 COM 部分）。回 `Ok` 即代表記錄完整，
/// 見模組文件「給 4.4」一節。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WallpaperSnapshot {
    /// 全部列舉到的螢幕（含離線者，見 [`MonitorEntry::is_online`]），依 `GetMonitorDevicePathAt`
    /// 的索引排列：第 i 個就是索引 i（task 6.1 修正：explorer 的逐螢幕轉存檔 `Transcoded_<索引三位數>`
    /// 以這個索引編號，見 `wallpaper_state::StatePaths::monitor_transcoded`）。
    pub monitors: Vec<MonitorWallpaper>,
    /// `GetPosition`。
    pub position: WallpaperPosition,
    /// `GetBackgroundColor`（`COLORREF`＝`0x00BBGGRR`）。
    pub background_color: u32,
    /// `GetStatus` 的原始旗標（[`SLIDESHOW_STATE_ENABLED`] 等）。
    pub slideshow_status: i32,
    /// `GetSlideshow`＋`GetSlideshowOptions`。`Some` 若且唯若 [`slideshow_applicable`]
    /// (`slideshow_status`)；不適用時不讀取、為 `None`。
    pub slideshow: Option<SlideshowInfo>,
}

// ---------------------------------------------------------------------------------------------
// 與平台無關的轉換（純函式，可單元測試）
// ---------------------------------------------------------------------------------------------

/// 一台螢幕 `GetMonitorRECT` 的結果（[`classify_monitor_rect`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonitorRect {
    /// `S_OK`：在線，附矩形。
    Online(PhysicalRect),
    /// `S_FALSE`：文件記載的「已拔除但仍被列舉」。插回後 explorer 落後時也可能這樣回，所以同樣要以
    /// 系統作用中的顯示器數佐證才算離線（[`assemble_monitor_list`]）。
    Detached,
    /// 失敗 HRESULT（介面斷線者除外，見 [`classify_monitor_rect`]）：可能離線，也可能是作用中螢幕的
    /// 暫時錯誤——要以系統作用中的顯示器數佐證才算離線（[`assemble_monitor_list`]）。2026-10-06 在場
    /// 驗收實機：拔掉的螢幕仍在 `GetMonitorDevicePathCount`／`GetMonitorDevicePathAt` 裡，
    /// `GetMonitorRECT` 回 `E_FAIL`（0x80004005）而不是 `S_FALSE`。
    Failed(i32),
}

/// 介面本身已斷線（explorer 重啟或結束）的 HRESULT：不是「這台離線」，整個列舉回錯、觸發重建。
const INTERFACE_LOST_HRESULTS: [windows::core::HRESULT; 9] = [
    windows::Win32::Foundation::RPC_E_DISCONNECTED,
    windows::Win32::Foundation::RPC_E_SERVER_DIED,
    windows::Win32::Foundation::RPC_E_SERVER_DIED_DNE,
    windows::Win32::Foundation::RPC_E_INVALID_OBJECT,
    windows::Win32::Foundation::CO_E_OBJNOTCONNECTED,
    windows::Win32::Foundation::CO_E_SERVER_STOPPING,
    // RPC_S_SERVER_UNAVAILABLE（1722）、RPC_S_CALL_FAILED（1726）、RPC_S_CALL_FAILED_DNE（1727）的
    // HRESULT 形式。
    windows::core::HRESULT::from_win32(1722),
    windows::core::HRESULT::from_win32(1726),
    windows::core::HRESULT::from_win32(1727),
];

/// `GetMonitorRECT` 原始 HRESULT 的意義：`S_OK`（0）＝在線（回矩形）；`S_FALSE`（1）＝已拔除
/// 但仍被列舉；其他失敗 HRESULT＝[`MonitorRect::Failed`]（實機的拔除就是這樣；是否當成離線由
/// [`assemble_monitor_list`] 以系統作用中的顯示器數判定）。
/// 例外（回 [`BackendError::Com`]，觸發重建）：介面斷線類 HRESULT（[`INTERFACE_LOST_HRESULTS`]），
/// 以及非預期的成功碼（不當成離線）。
pub fn classify_monitor_rect(hr: i32, ltrb: [i32; 4]) -> Result<MonitorRect, BackendError> {
    const S_OK: i32 = 0;
    const S_FALSE: i32 = 1;
    let com_err = |what: &str| {
        BackendError::Com(format!(
            "GetMonitorRECT {what}：HRESULT 0x{:08X}",
            hr as u32
        ))
    };
    match hr {
        S_OK => Ok(MonitorRect::Online(
            crate::desktop::physical_rect_from_ltrb(ltrb[0], ltrb[1], ltrb[2], ltrb[3]),
        )),
        S_FALSE => Ok(MonitorRect::Detached),
        h if INTERFACE_LOST_HRESULTS.iter().any(|lost| lost.0 == h) => {
            Err(com_err("失敗（介面已斷線）"))
        }
        h if h < 0 => Ok(MonitorRect::Failed(h)),
        _ => Err(com_err("回非預期的成功碼")),
    }
}

/// 列舉時逐台取得的原始結果：裝置路徑、`GetMonitorRECT` 的 HRESULT 與 RECT（left, top, right, bottom）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorProbe {
    pub device_path: String,
    pub hr: i32,
    pub ltrb: [i32; 4],
}

/// 把逐台的 `GetMonitorRECT` 結果組成列舉清單（順序＝`GetMonitorDevicePathAt` 索引）。
///
/// `active_monitors`＝列舉同時讀到的系統作用中顯示器數（`GetSystemMetrics(SM_CMONITORS)`；讀不到為
/// `None`）。
///
/// 逐台處理：有不在線的項目（`GetMonitorRECT` 回 `S_FALSE` 或失敗 HRESULT）時，**只有在線（`S_OK`）
/// 的台數已達系統作用中的顯示器數**，才把它們標為離線（`rect: None`）、其餘照常回傳——作用中的顯示器
/// 都已在線，不在線的那幾台必然不是作用中的（2026-10-06 實機：拔掉的螢幕仍被列舉、回 `E_FAIL`；一台
/// 離線不得讓整批失敗，否則在線的筆電也補畫不了）。否則不在線者裡至少有一台是作用中的——暫時錯誤
/// （例如 `RPC_E_CALL_REJECTED`、拓樸切換途中），或插回後 explorer 還沒追上系統（回 `E_FAIL` 或
/// 文件記載的 `S_FALSE` 都可能）——整個列舉回錯，呼叫端退避重試；不得當成離線（還原時會被標成待還原、
/// 命令列回「仍有離線」，解除安裝後那台永久停在宿主的圖；插回時會被當成已列舉完畢而不再補畫）。
///
/// 對應依據只用台數、不對應「哪一台」：系統的顯示器（`HMONITOR`）與 `IDesktopWallpaper` 的裝置路徑沒有
/// 可靠的一對一鍵（矩形在拓樸切換途中會變），而「在線台數 ≥ 作用中台數」已足以證明不在線者都不是作用中
/// 的。沒有任何不在線的項目時不看系統台數：`IDesktopWallpaper` 若對某台作用中的顯示器根本不列舉（未見過），
/// 列舉照常回傳、不會永久失敗；但此時只要另有一台不在線就會整批回錯。讀不到系統台數時，只有失敗 HRESULT
/// 整批回錯，`S_FALSE` 照舊視為離線。
///
/// 其他整批回錯：任一台是介面斷線類 HRESULT 或非預期成功碼（[`classify_monitor_rect`]）；沒有任何一台
/// 在線、且至少一台是失敗 HRESULT。
pub fn assemble_monitor_list(
    probes: Vec<MonitorProbe>,
    active_monitors: Option<u32>,
) -> Result<Vec<MonitorEntry>, BackendError> {
    let mut out = Vec::with_capacity(probes.len());
    let mut failed = Vec::new();
    for p in probes {
        let rect = match classify_monitor_rect(p.hr, p.ltrb)? {
            MonitorRect::Online(r) => Some(r),
            MonitorRect::Detached => None,
            MonitorRect::Failed(hr) => {
                failed.push(format!("{}：HRESULT 0x{:08X}", p.device_path, hr as u32));
                None
            }
        };
        out.push(MonitorEntry {
            device_path: p.device_path,
            rect,
        });
    }
    let online = out.iter().filter(|m| m.is_online()).count();
    let not_online: Vec<&str> = out
        .iter()
        .filter(|m| !m.is_online())
        .map(|m| m.device_path.as_str())
        .collect();
    if not_online.is_empty() {
        // 沒有不在線的項目：不看系統台數（explorer 漏列某台時也照常回傳，不得永久失敗）。
        return Ok(out);
    }
    if online == 0 && !failed.is_empty() {
        return Err(BackendError::Com(format!(
            "GetMonitorRECT 對所有螢幕都失敗（沒有在線螢幕）：{}",
            failed.join("；")
        )));
    }
    let failed_text = if failed.is_empty() {
        "無（皆為 S_FALSE）".to_owned()
    } else {
        failed.join("；")
    };
    match active_monitors {
        Some(active) if online >= active as usize => Ok(out),
        Some(active) => Err(BackendError::Com(format!(
            "在線 {online} 台少於系統作用中的顯示器 {active} 台（不在線者至少一台是作用中的，不當成離線）：不在線 {}；失敗 HRESULT {failed_text}",
            not_online.join("、")
        ))),
        // 讀不到系統台數：S_FALSE（文件記載的已拔除）照舊視為離線，失敗 HRESULT 無法佐證。
        None if failed.is_empty() => Ok(out),
        None => Err(BackendError::Com(format!(
            "GetMonitorRECT 失敗，且讀不到系統作用中的顯示器數（無法證明失敗者已離線）：{failed_text}"
        ))),
    }
}

/// UTF-16 → `String`，無效 UTF-16（落單的 surrogate）回錯而不是空字串——空字串在本模組代表
/// 「純色、沒有圖片」，把無法表示的路徑記成空字串會讓 4.4 誤記原桌布。
pub fn decode_wide(what: &str, wide: &[u16]) -> Result<String, BackendError> {
    String::from_utf16(wide)
        .map_err(|_| BackendError::Invalid(format!("{what} 含無效的 UTF-16，無法忠實記錄")))
}

// ---------------------------------------------------------------------------------------------
// 請求種類與錯誤
// ---------------------------------------------------------------------------------------------

/// 請求種類（記錄與錯誤訊息用）。全部都經由 `IDesktopWallpaper`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RequestKind {
    ListMonitors,
    Read,
    SetWallpaper,
    SetWallpaperAll,
    SetSolidColor,
    SetPosition,
    SetBackgroundColor,
    RestoreSlideshow,
}

impl RequestKind {
    /// 記錄用名稱。
    pub fn name(self) -> &'static str {
        match self {
            Self::ListMonitors => "list_monitors",
            Self::Read => "read",
            Self::SetWallpaper => "set_wallpaper",
            Self::SetWallpaperAll => "set_wallpaper_all",
            Self::SetSolidColor => "set_solid_color",
            Self::SetPosition => "set_position",
            Self::SetBackgroundColor => "set_background_color",
            Self::RestoreSlideshow => "restore_slideshow",
        }
    }
}

impl fmt::Display for RequestKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// 後端（工作執行緒內）回報的錯誤。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendError {
    /// COM 呼叫失敗：介面可能已失效（explorer 重啟），下一個請求前重建。
    Com(String),
    /// 沒有可用的介面（`CoInitializeEx` 或 `CoCreateInstance` 失敗）。
    Unavailable(String),
    /// 資料無法使用或無法忠實表示（無效 UTF-16、投影片路徑無法解析……），不觸發重建。
    Invalid(String),
}

impl BackendError {
    /// 這個錯誤是否代表介面可能已失效、下一個請求前要重建。
    pub fn invalidates_interface(&self) -> bool {
        matches!(self, Self::Com(_) | Self::Unavailable(_))
    }
}

impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Com(m) => write!(f, "COM 呼叫失敗：{m}"),
            Self::Unavailable(m) => write!(f, "桌布介面不可用：{m}"),
            Self::Invalid(m) => write!(f, "資料無法使用：{m}"),
        }
    }
}

impl std::error::Error for BackendError {}

/// 呼叫端看到的錯誤。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WallpaperError {
    /// 等滿逾時仍未返回；服務隨即進入忙碌，直到這個請求返回。
    Timeout {
        request: RequestKind,
        timeout: Duration,
    },
    /// 前一個逾時的請求還沒返回；本請求沒有送出。
    Busy { request: RequestKind },
    /// 在主執行緒上呼叫（release 建置的防護；debug 建置會先 `debug_assert!` 失敗）；本請求沒有
    /// 送出。
    WrongThread { request: RequestKind },
    /// 工作執行緒已不存在（無法送出請求）。
    WorkerGone { request: RequestKind },
    /// 請求在處理中途中止（後端 panic），沒有結果。
    Aborted { request: RequestKind },
    /// 後端回報失敗。
    Backend {
        request: RequestKind,
        error: BackendError,
    },
}

impl WallpaperError {
    /// 哪一種請求失敗。
    pub fn request(&self) -> RequestKind {
        match self {
            Self::Timeout { request, .. }
            | Self::Busy { request }
            | Self::WrongThread { request }
            | Self::WorkerGone { request }
            | Self::Aborted { request }
            | Self::Backend { request, .. } => *request,
        }
    }
}

impl fmt::Display for WallpaperError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout { request, timeout } => {
                write!(f, "桌布請求 {request} 逾時（{} ms）", timeout.as_millis())
            }
            Self::Busy { request } => {
                write!(f, "桌布請求 {request} 未送出：前一個逾時的請求尚未返回")
            }
            Self::WrongThread { request } => {
                write!(f, "桌布請求 {request} 未送出：不得在主執行緒上呼叫")
            }
            Self::WorkerGone { request } => {
                write!(f, "桌布請求 {request} 未送出：工作執行緒已結束")
            }
            Self::Aborted { request } => write!(f, "桌布請求 {request} 處理中途中止"),
            Self::Backend { request, error } => write!(f, "桌布請求 {request} 失敗：{error}"),
        }
    }
}

impl std::error::Error for WallpaperError {}

// ---------------------------------------------------------------------------------------------
// 後端 trait 與記錄接口
// ---------------------------------------------------------------------------------------------

/// 「桌布後端」：工作執行緒上實際執行請求的物件。真實實作 [`ComBackend`]；測試與 4.3／4.4
/// 注入假實作。所有方法都在同一條工作執行緒上被呼叫，可以放心阻塞（呼叫端有逾時）。
pub trait WallpaperBackend {
    /// 手上是否有可用的介面（`false` 時下一個請求前會先 [`Self::recreate`]）。
    fn is_ready(&self) -> bool;
    /// 丟掉舊介面、重新建立（`CoCreateInstance`）。
    fn recreate(&mut self) -> Result<(), BackendError>;
    /// 目前 explorer 的 PID（殼層視窗擁有者）；查不到（explorer 重啟中）為 `None`。
    fn explorer_pid(&self) -> Option<u32>;

    /// 列舉螢幕（含離線者）。
    fn list_monitors(&mut self) -> Result<Vec<MonitorEntry>, BackendError>;
    /// 讀取逐螢幕桌布、填滿方式、背景色與投影片狀態；成敗語意見模組文件「給 4.4」。
    fn read(&mut self) -> Result<WallpaperSnapshot, BackendError>;
    /// 逐螢幕 `SetWallpaper`（非同步：`Ok` 只代表請求被接受，是否生效以讀回為準）。
    fn set_wallpaper(&mut self, device_path: &str, image: &Path) -> Result<(), BackendError>;
    /// 「全部螢幕」設定：`SetWallpaper(NULL, 圖)`（installer-auto-update 4.x）。monitorID 為空指標＝所有螢幕；
    /// 與逐螢幕設定的差別（6.1 實機）：不產生逐螢幕轉存檔 `Transcoded_<索引>`、登錄 `Wallpaper` 寫成這張圖的路徑，
    /// 之後 `GetWallpaper(NULL)` 才讀得到值。用於還原「原本就是以全部螢幕設定」的桌布。非同步，是否生效以讀回為準。
    fn set_wallpaper_all(&mut self, image: &Path) -> Result<(), BackendError>;
    /// 全域純色：`SetWallpaper(NULL, "")`（task 6.1 修正）。純色是**全域**狀態——逐螢幕
    /// `SetWallpaper(<螢幕>, "")` 不會變純色，而是換回 explorer 記憶中的逐螢幕圖片（仍回 `S_OK`），
    /// 只有 monitorID 為 NULL 才會讓所有螢幕回到純色（此時 `GetStatus` 回 0）。非同步，是否生效以讀回
    /// 為準。
    fn set_solid_color(&mut self) -> Result<(), BackendError>;
    /// 全域 `SetPosition`。
    fn set_position(&mut self, position: WallpaperPosition) -> Result<(), BackendError>;
    /// 全域 `SetBackgroundColor`（`COLORREF`＝`0x00BBGGRR`）。
    fn set_background_color(&mut self, colorref: u32) -> Result<(), BackendError>;
    /// 還原投影片（`SetSlideshow`＋`SetSlideshowOptions`）。
    fn restore_slideshow(&mut self, slideshow: &SlideshowInfo) -> Result<(), BackendError>;
}

/// 服務的記錄接口（測試注入以攔截警告；預設 [`StdLog`] 寫進 `log` facade＝宿主記錄檔）。
pub trait WallpaperLog: Send + Sync {
    fn warn(&self, msg: &str);
    fn info(&self, msg: &str);
}

/// 預設記錄接口：轉給 `log` crate。
#[derive(Debug, Default, Clone, Copy)]
pub struct StdLog;

impl WallpaperLog for StdLog {
    fn warn(&self, msg: &str) {
        log::warn!("{msg}");
    }
    fn info(&self, msg: &str) {
        log::info!("{msg}");
    }
}

/// 服務設定。
#[derive(Clone)]
pub struct WallpaperServiceConfig {
    /// 呼叫端等待單一請求的逾時（預設 [`DEFAULT_REQUEST_TIMEOUT`]）。
    pub timeout: Duration,
    /// 工作執行緒上單一請求耗時超過此值記警告（預設 [`DEFAULT_SLOW_THRESHOLD`]）。
    pub slow_threshold: Duration,
    /// 記錄接口。
    pub log: Arc<dyn WallpaperLog>,
}

impl Default for WallpaperServiceConfig {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_REQUEST_TIMEOUT,
            slow_threshold: DEFAULT_SLOW_THRESHOLD,
            log: Arc::new(StdLog),
        }
    }
}

impl fmt::Debug for WallpaperServiceConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WallpaperServiceConfig")
            .field("timeout", &self.timeout)
            .field("slow_threshold", &self.slow_threshold)
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------------------------
// 介面重建判定（純邏輯）
// ---------------------------------------------------------------------------------------------

/// 為什麼要重建介面（記錄用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecreateReason {
    /// 還沒有介面（第一次使用或上次建立失敗）。
    NotReady,
    /// 上一個請求回報介面失效。
    PreviousFailure,
    /// explorer 的 PID 改變（重啟過）。
    ExplorerRestarted { old: u32, new: u32 },
}

impl fmt::Display for RecreateReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotReady => f.write_str("尚無介面"),
            Self::PreviousFailure => f.write_str("上一個 COM 請求失敗"),
            Self::ExplorerRestarted { old, new } => {
                write!(f, "explorer PID 改變 {old} -> {new}")
            }
        }
    }
}

/// 介面重建判定的狀態（工作執行緒私有）。
#[derive(Debug, Default)]
struct RecreateTracker {
    /// 上一個請求回報介面失效。
    stale: bool,
    /// 建立目前介面時（或之後第一次查到時）的 explorer PID。
    bound_pid: Option<u32>,
}

impl RecreateTracker {
    /// 請求開始前：要不要先重建？`ready`＝後端手上有介面；`pid`＝目前 explorer PID
    /// （`None`＝查不到，例如 explorer 重啟中，不算改變）。
    fn decide(&self, ready: bool, pid: Option<u32>) -> Option<RecreateReason> {
        if !ready {
            return Some(RecreateReason::NotReady);
        }
        if self.stale {
            return Some(RecreateReason::PreviousFailure);
        }
        match (self.bound_pid, pid) {
            (Some(old), Some(new)) if old != new => {
                Some(RecreateReason::ExplorerRestarted { old, new })
            }
            _ => None,
        }
    }

    /// 重建結束：成功才清掉待重建、綁定當時的 PID；失敗維持原狀（下個請求再試）。
    fn on_recreated(&mut self, ok: bool, pid: Option<u32>) {
        if ok {
            self.stale = false;
            self.bound_pid = pid;
        }
    }

    /// 建立介面時 PID 未知：之後第一次查到就綁定。
    fn observe_pid(&mut self, pid: Option<u32>) {
        if self.bound_pid.is_none() {
            self.bound_pid = pid;
        }
    }

    /// 請求結束：回報介面可能失效（或 panic）就標記待重建。
    fn on_job_done(&mut self, invalidated: bool) {
        if invalidated {
            self.stale = true;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 服務（呼叫端 API）
// ---------------------------------------------------------------------------------------------

/// 執行緒名稱是否為禁止呼叫阻塞式請求的執行緒：std 把行程主執行緒命名為 `main`（Tauri 的
/// 事件迴圈與同步指令都在這條執行緒上）。
fn is_forbidden_caller(thread_name: Option<&str>) -> bool {
    thread_name == Some("main")
}

/// 桌布 COM 執行緒的呼叫端把手。可 `clone` 給多個呼叫端共用同一條工作執行緒；最後一個把手
/// 丟棄後，工作執行緒處理完佇列即結束（丟棄**不**等待工作執行緒，卡住時也不會拖住丟棄者）。
///
/// 請求方法會阻塞到結果或逾時，**不得在主執行緒呼叫**（見模組文件）。
#[derive(Clone)]
pub struct WallpaperService {
    inner: Arc<Inner>,
}

struct Inner {
    tx: mpsc::Sender<Job>,
    gate: Arc<Mutex<Gate>>,
    config: WallpaperServiceConfig,
}

impl WallpaperService {
    /// 以真實 COM 後端（[`ComBackend`]）啟動工作執行緒。
    pub fn spawn_com(config: WallpaperServiceConfig) -> std::io::Result<Self> {
        Self::spawn_with(config, ComBackend::new)
    }

    /// 以注入的後端啟動工作執行緒。`factory` 在工作執行緒上執行（後端不需要 `Send`）。
    pub fn spawn_with<B, F>(config: WallpaperServiceConfig, factory: F) -> std::io::Result<Self>
    where
        B: WallpaperBackend + 'static,
        F: FnOnce() -> B + Send + 'static,
    {
        let (tx, rx) = mpsc::channel::<Job>();
        let gate = Arc::new(Mutex::new(Gate::default()));
        let worker_gate = Arc::clone(&gate);
        let log = Arc::clone(&config.log);
        let slow_threshold = config.slow_threshold;
        // 不保留 JoinHandle：工作執行緒可能卡在 explorer 裡，任何人都不該 join 它。佇列的最後一個
        // Sender 丟棄後，它處理完手上的請求即結束。
        thread::Builder::new()
            .name(THREAD_NAME.to_owned())
            .spawn(move || {
                let mut backend = factory();
                worker_loop(&mut backend, rx, &worker_gate, slow_threshold, &*log);
            })?;
        Ok(Self {
            inner: Arc::new(Inner { tx, gate, config }),
        })
    }

    /// 目前是否忙碌（有逾時的請求尚未返回）。
    pub fn is_busy(&self) -> bool {
        lock_gate(&self.inner.gate).hung.is_some()
    }

    /// 本服務的請求逾時。
    pub fn timeout(&self) -> Duration {
        self.inner.config.timeout
    }

    /// 列舉螢幕（含離線者）。
    pub fn list_monitors(&self) -> Result<Vec<MonitorEntry>, WallpaperError> {
        self.submit(RequestKind::ListMonitors, |b| b.list_monitors())
    }

    /// 讀取逐螢幕桌布、填滿方式、背景色與投影片狀態。成敗語意見模組文件「給 4.4」。
    pub fn read(&self) -> Result<WallpaperSnapshot, WallpaperError> {
        self.submit(RequestKind::Read, |b| b.read())
    }

    /// 逐螢幕設定桌布（非同步：`Ok` 只代表 explorer 接受請求，是否生效以 [`Self::read`] 讀回為準）。
    pub fn set_wallpaper(&self, device_path: &str, image: &Path) -> Result<(), WallpaperError> {
        let device_path = device_path.to_owned();
        let image: PathBuf = image.to_owned();
        self.submit(RequestKind::SetWallpaper, move |b| {
            b.set_wallpaper(&device_path, &image)
        })
    }

    /// 以「全部螢幕」設定桌布（`SetWallpaper(NULL, 圖)`；見 [`WallpaperBackend::set_wallpaper_all`]）。
    pub fn set_wallpaper_all(&self, image: &Path) -> Result<(), WallpaperError> {
        let image: PathBuf = image.to_owned();
        self.submit(RequestKind::SetWallpaperAll, move |b| {
            b.set_wallpaper_all(&image)
        })
    }

    /// 所有螢幕改回純色（`SetWallpaper(NULL, "")`；見 [`WallpaperBackend::set_solid_color`]）。
    pub fn set_solid_color(&self) -> Result<(), WallpaperError> {
        self.submit(RequestKind::SetSolidColor, |b| b.set_solid_color())
    }

    /// 設定全域填滿方式。
    pub fn set_position(&self, position: WallpaperPosition) -> Result<(), WallpaperError> {
        self.submit(RequestKind::SetPosition, move |b| b.set_position(position))
    }

    /// 設定背景色（`COLORREF`＝`0x00BBGGRR`）。
    pub fn set_background_color(&self, colorref: u32) -> Result<(), WallpaperError> {
        self.submit(RequestKind::SetBackgroundColor, move |b| {
            b.set_background_color(colorref)
        })
    }

    /// 還原投影片。
    pub fn restore_slideshow(&self, slideshow: &SlideshowInfo) -> Result<(), WallpaperError> {
        let slideshow = slideshow.clone();
        self.submit(RequestKind::RestoreSlideshow, move |b| {
            b.restore_slideshow(&slideshow)
        })
    }

    fn submit<T, Op>(&self, request: RequestKind, op: Op) -> Result<T, WallpaperError>
    where
        T: Send + 'static,
        Op: FnOnce(&mut dyn WallpaperBackend) -> Result<T, BackendError> + Send + 'static,
    {
        let forbidden = is_forbidden_caller(thread::current().name());
        debug_assert!(
            !forbidden,
            "桌布請求 {request} 不得在主執行緒上呼叫（最長阻塞到逾時）"
        );
        if forbidden {
            self.inner.config.log.warn(&format!(
                "桌布請求 {request} 在主執行緒上被呼叫，已拒絕（呼叫端應改在排程執行緒上呼叫）"
            ));
            return Err(WallpaperError::WrongThread { request });
        }

        let (reply_tx, reply_rx) = mpsc::sync_channel::<Result<T, BackendError>>(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        let id = {
            let mut gate = lock_gate(&self.inner.gate);
            if gate.hung.is_some() {
                return Err(WallpaperError::Busy { request });
            }
            gate.next_id += 1;
            let id = gate.next_id;
            let job = Job {
                id,
                request,
                cancelled: Arc::clone(&cancelled),
                run: Box::new(move |backend| {
                    let result = op(backend);
                    let invalidated = result
                        .as_ref()
                        .err()
                        .is_some_and(BackendError::invalidates_interface);
                    // 呼叫端可能已逾時離開，送不到就算了。
                    let _ = reply_tx.send(result);
                    invalidated
                }),
            };
            // 在閘門鎖內送出：與工作執行緒的「處理完」更新互斥，編號與佇列順序一致。send 不阻塞。
            if self.inner.tx.send(job).is_err() {
                return Err(WallpaperError::WorkerGone { request });
            }
            id
        };

        match reply_rx.recv_timeout(self.inner.config.timeout) {
            Ok(result) => wrap_result(request, result),
            Err(RecvTimeoutError::Disconnected) => Err(WallpaperError::Aborted { request }),
            Err(RecvTimeoutError::Timeout) => on_timeout(
                &reply_rx,
                &self.inner.gate,
                &cancelled,
                id,
                request,
                self.inner.config.timeout,
                &*self.inner.config.log,
            ),
        }
    }
}

fn wrap_result<T>(
    request: RequestKind,
    result: Result<T, BackendError>,
) -> Result<T, WallpaperError> {
    result.map_err(|error| WallpaperError::Backend { request, error })
}

/// 呼叫端等滿逾時之後的處理：
///
/// 1. 先 `try_recv` 一次——結果若已在 channel 裡（與逾時同時送達），直接回傳，不宣告逾時、
///    不設忙碌、不記警告。
/// 2. 上鎖後若工作執行緒已處理完這個請求（`last_finished >= id`），表示請求沒有結果
///    （panic 中止）→ `Aborted`。
/// 3. 否則標記取消、設忙碌、記警告，回 `Timeout`。
fn on_timeout<T>(
    reply_rx: &Receiver<Result<T, BackendError>>,
    gate: &Mutex<Gate>,
    cancelled: &AtomicBool,
    id: u64,
    request: RequestKind,
    timeout: Duration,
    log: &dyn WallpaperLog,
) -> Result<T, WallpaperError> {
    if let Ok(result) = reply_rx.try_recv() {
        return wrap_result(request, result);
    }
    let mut g = lock_gate(gate);
    if g.last_finished >= id {
        // 上鎖前的瞬間完成：結果（若有）一定已在 channel 裡（送出發生在標記完成之前）。
        drop(g);
        return match reply_rx.try_recv() {
            Ok(result) => wrap_result(request, result),
            Err(_) => Err(WallpaperError::Aborted { request }),
        };
    }
    cancelled.store(true, Ordering::SeqCst);
    if g.hung.is_none_or(|h| h.id < id) {
        g.hung = Some(HungJob {
            id,
            request,
            since: Instant::now(),
        });
    }
    drop(g);
    log.warn(&format!(
        "桌布請求 {request} 逾時（{} ms），標為忙碌：前一個請求返回前不再送新請求",
        timeout.as_millis()
    ));
    Err(WallpaperError::Timeout { request, timeout })
}

/// 佇列裡的一個請求。`run` 執行後端呼叫、把結果送回呼叫端，回傳「介面是否可能已失效」。
struct Job {
    id: u64,
    request: RequestKind,
    /// 呼叫端已逾時放棄；工作執行緒輪到它時若尚未開始就略過。
    cancelled: Arc<AtomicBool>,
    run: JobFn,
}

type JobFn = Box<dyn FnOnce(&mut dyn WallpaperBackend) -> bool + Send>;

/// 呼叫端與工作執行緒共用的忙碌閘門。
#[derive(Debug, Default)]
struct Gate {
    /// 上一個發出的請求編號（從 1 起算）。
    next_id: u64,
    /// 工作執行緒最後處理完（含略過）的請求編號；佇列依序處理，故編號單調遞增。
    last_finished: u64,
    /// 逾時未返回的請求；`Some` 時＝忙碌。不變量：`hung.id > last_finished`。
    hung: Option<HungJob>,
}

#[derive(Debug, Clone, Copy)]
struct HungJob {
    id: u64,
    request: RequestKind,
    since: Instant,
}

fn lock_gate(gate: &Mutex<Gate>) -> MutexGuard<'_, Gate> {
    // 閘門內只有整數與 Option，panic 中斷也不會留下不一致的狀態，毒化時沿用內容即可。
    gate.lock().unwrap_or_else(|e| e.into_inner())
}

/// 工作執行緒主迴圈：依序處理佇列，所有 Sender 丟棄後結束。
fn worker_loop(
    backend: &mut dyn WallpaperBackend,
    rx: mpsc::Receiver<Job>,
    gate: &Mutex<Gate>,
    slow_threshold: Duration,
    log: &dyn WallpaperLog,
) {
    let mut tracker = RecreateTracker::default();
    for job in rx {
        let Job {
            id,
            request,
            cancelled,
            run,
        } = job;
        if cancelled.load(Ordering::SeqCst) {
            log.info(&format!(
                "桌布請求 {request} 的呼叫端已逾時放棄，略過不執行"
            ));
            finish_job(gate, id, log);
            continue;
        }
        let started = Instant::now();
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            prepare_interface(&mut *backend, &mut tracker, log);
            run(&mut *backend)
        }));
        let invalidated = outcome.unwrap_or_else(|_| {
            log.warn(&format!(
                "桌布請求 {request} 處理中 panic；下一個請求前重建介面"
            ));
            true
        });
        tracker.on_job_done(invalidated);
        let elapsed = started.elapsed();
        if elapsed > slow_threshold {
            log.warn(&format!(
                "桌布請求 {request} 耗時 {} ms（超過 {} ms），explorer 可能異常",
                elapsed.as_millis(),
                slow_threshold.as_millis()
            ));
        }
        finish_job(gate, id, log);
    }
}

/// 請求前：依 [`RecreateTracker`] 判定要不要（重新）建立介面。失敗只記警告，請求照樣交給
/// 後端（後端會回 [`BackendError::Unavailable`]）。
fn prepare_interface(
    backend: &mut dyn WallpaperBackend,
    tracker: &mut RecreateTracker,
    log: &dyn WallpaperLog,
) {
    let pid = backend.explorer_pid();
    match tracker.decide(backend.is_ready(), pid) {
        None => tracker.observe_pid(pid),
        Some(reason) => match backend.recreate() {
            Ok(()) => {
                tracker.on_recreated(true, pid);
                log.info(&format!("已重建 IDesktopWallpaper（{reason}）"));
            }
            Err(e) => {
                tracker.on_recreated(false, pid);
                log.warn(&format!("重建 IDesktopWallpaper 失敗（{reason}）：{e}"));
            }
        },
    }
}

/// 標記請求 `id` 處理完（含略過）；若它涵蓋了逾時未返回的請求，解除忙碌。
fn finish_job(gate: &Mutex<Gate>, id: u64, log: &dyn WallpaperLog) {
    let mut g = lock_gate(gate);
    g.last_finished = id;
    if let Some(h) = g.hung.filter(|h| h.id <= id) {
        g.hung = None;
        drop(g);
        log.info(&format!(
            "先前逾時的桌布請求 {} 已返回（逾時後又過 {} ms），解除忙碌",
            h.request,
            h.since.elapsed().as_millis()
        ));
    }
}

#[cfg(test)]
mod tests;
