//! 系統匣通知（dynamic-wallpaper task 4.8；specs/desktop-wallpaper「使用者自行更換桌布時讓位」
//! 「explorer 資源安全閥」的系統匣通知）。
//!
//! ## 做法：對宿主既有的系統匣圖示發 `NIF_INFO` 通知（零新依賴）
//!
//! 宿主的系統匣圖示由 Tauri 2.12 經 `tray-icon` 0.25.1 建立（`tray.rs` 模組文件）。`tray-icon`
//! 為每個圖示建一個隱藏視窗，並以 `Shell_NotifyIconW(NIM_ADD)` 登記 `(hWnd, uID)`；
//! `hWnd` 可由 `tray_icon::TrayIcon::window_handle()` 取得（Tauri 的
//! `TrayIcon::with_inner_tray_icon`），`uID` 是 crate 內部計數器給的值、沒有公開 API
//! （`tray-icon-0.25.1/src/platform_impl/windows/mod.rs`：`internal_id = COUNTER.next()`，
//! `src/counter.rs` 從 1 起算）。這個計數器也被 `TrayIconId::new_unique()` 共用：Tauri 2.12 依設定檔
//! 建立圖示時走 `TrayIconBuilder::with_id("main")`（`tauri-2.12.0/src/app.rs`），它先呼叫
//! `Self::new()`→`tray_icon::TrayIconBuilder::new()`（`tray-icon-0.25.1/src/lib.rs`），以
//! `TrayIconId::new_unique()` 用掉 1，圖示本身的 `internal_id` 依原始碼推導為 **2**（修正輪 1 依審查
//! 改正；先前誤寫為 1）。推導依賴 crate 內部實作、升級就可能變，所以**以探測為準、不寫死**：
//! [`show_balloon`] 依序以 `uID`＝1…[`MAX_PROBE_ID`] 送 `NIM_MODIFY`，第一個回 `TRUE` 的就是該視窗的
//! 圖示（同一個 `hWnd` 只登記了一個圖示），之後快取。
//! `uID` 不對時 `NIM_MODIFY` 只回 `FALSE`、不影響任何圖示。Tauri 設定沒有指定圖示 GUID
//! （`tauri.conf.json` 的 `app.trayIcon` 只有 `iconPath`），所以殼層以 `(hWnd, uID)` 識別。
//!
//! 一手來源（Microsoft Learn）：
//! - 〈NOTIFYICONDATAW structure〉：`NIF_INFO`＝顯示通知，`szInfo` 最多 256 字元（含結尾 null）、
//!   `szInfoTitle` 最多 64 字元（含結尾 null）；`dwInfoFlags` 的 `NIIF_INFO`／`NIIF_WARNING` 是標題
//!   左側的圖示；「No more than one balloon notification at a time can be displayed for the
//!   taskbar」，後到的排隊。
//! - 〈Shell_NotifyIconW function〉：`NIM_MODIFY` 以 `NIM_ADD` 時的識別碼找圖示，成功回 `TRUE`；
//!   「On Windows 10, the balloon messages are shown as banner notifications」（Windows 11 為短暫
//!   顯示、不留在通知中心）。
//!
//! windows crate 0.62.2 的簽名查證於 `~/.cargo/registry/src/*/windows-0.62.2/src/Windows/Win32/UI/Shell/mod.rs`：
//! `Shell_NotifyIconW(NOTIFY_ICON_MESSAGE, *const NOTIFYICONDATAW) -> BOOL`、`NOTIFYICONDATAW`
//! （`szInfo: [u16; 256]`、`szInfoTitle: [u16; 64]`、`Default`＝全 0）、`NIF_INFO`、`NIM_MODIFY`、
//! `NIIF_INFO`、`NIIF_WARNING`——都在已啟用的 `Win32_UI_Shell`。
//!
//! ## 執行緒
//!
//! `Shell_NotifyIconW` 是送給 explorer（工作列）的同步跨行程呼叫；安全閥正是 explorer 出狀況的
//! 時候，所以呼叫端（`tray::show_notification`）一律在**用完即丟的工作執行緒**呼叫本模組，不在主
//! 執行緒（小工具、設定視窗會被卡住）、也不在協調執行緒（下一次評估會被延後）。這個 API 只以
//! `(hWnd, uID)` 識別圖示，不是對視窗本身的操作，沒有視窗執行緒親和性的問題。

use std::sync::atomic::{AtomicU32, Ordering};

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_INFO, NIIF_INFO, NIIF_WARNING, NIM_MODIFY, NOTIFYICONDATAW,
    NOTIFY_ICON_INFOTIP_FLAGS,
};

/// 探測 `uID` 的上限（`tray-icon` 的計數器從 1 起算，宿主只建立一個系統匣圖示）。
pub const MAX_PROBE_ID: u32 = 32;

/// 上次成功的 `uID`（0＝還沒找到）。
static FOUND_ID: AtomicU32 = AtomicU32::new(0);

/// 通知的圖示種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BalloonKind {
    Info,
    Warning,
}

impl BalloonKind {
    fn flags(self) -> NOTIFY_ICON_INFOTIP_FLAGS {
        match self {
            Self::Info => NIIF_INFO,
            Self::Warning => NIIF_WARNING,
        }
    }
}

/// 把 `s` 編成 UTF-16 填進固定長度的陣列，**一定**留結尾 null；太長時截斷，且不切開代理對
/// （截斷點落在高代理之後時連它一起丟掉）。回傳是否截斷。
pub fn fill_wide<const N: usize>(dst: &mut [u16; N], s: &str) -> bool {
    dst.fill(0);
    if N == 0 {
        return !s.is_empty();
    }
    let units: Vec<u16> = s.encode_utf16().collect();
    let max = N - 1;
    let mut len = units.len().min(max);
    let truncated = units.len() > max;
    if truncated && len > 0 && (0xD800..=0xDBFF).contains(&units[len - 1]) {
        len -= 1;
    }
    dst[..len].copy_from_slice(&units[..len]);
    truncated
}

/// 組出 `NIM_MODIFY`＋`NIF_INFO` 用的結構（`hWnd` 以整數保存，方便跨執行緒傳遞）。
fn balloon_data(
    hwnd: isize,
    id: u32,
    title: &str,
    body: &str,
    kind: BalloonKind,
) -> NOTIFYICONDATAW {
    let mut nid = NOTIFYICONDATAW {
        cbSize: u32::try_from(std::mem::size_of::<NOTIFYICONDATAW>()).unwrap_or(u32::MAX),
        hWnd: HWND(hwnd as *mut core::ffi::c_void),
        uID: id,
        uFlags: NIF_INFO,
        dwInfoFlags: kind.flags(),
        ..Default::default()
    };
    if fill_wide(&mut nid.szInfoTitle, title) {
        log::warn!("系統匣通知標題過長，已截斷：{title}");
    }
    // `szInfo` 空字串＝移除通知；內文一定要有字。
    let body = if body.is_empty() { " " } else { body };
    if fill_wide(&mut nid.szInfo, body) {
        log::warn!("系統匣通知內文過長，已截斷：{body}");
    }
    nid
}

/// 依序嘗試的 `uID`：先試上次成功的，再試 1…[`MAX_PROBE_ID`]（不重複）。
pub fn probe_order(cached: u32) -> Vec<u32> {
    let mut ids = Vec::with_capacity(MAX_PROBE_ID as usize + 1);
    if cached != 0 {
        ids.push(cached);
    }
    ids.extend((1..=MAX_PROBE_ID).filter(|&i| i != cached));
    ids
}

/// 對 `tray_hwnd` 這個系統匣圖示視窗顯示一則通知。成功回傳使用的 `uID`；找不到圖示（explorer
/// 重新啟動中、圖示已移除）時回錯誤說明。**會阻塞到 explorer 回應**——只在工作執行緒呼叫（見
/// 模組文件「執行緒」）。
pub fn show_balloon(
    tray_hwnd: isize,
    title: &str,
    body: &str,
    kind: BalloonKind,
) -> Result<u32, String> {
    if tray_hwnd == 0 {
        return Err("沒有系統匣圖示視窗".to_owned());
    }
    for id in probe_order(FOUND_ID.load(Ordering::Relaxed)) {
        let nid = balloon_data(tray_hwnd, id, title, body, kind);
        // SAFETY: `nid` 是本函式擁有、`cbSize` 正確、字串欄位都以 null 結尾的結構，指標只在呼叫
        // 期間有效；`NIM_MODIFY` 不保存指標。`hWnd` 不存在或 `uID` 不對時 API 回 FALSE，不影響
        // 任何圖示。
        let ok = unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) }.as_bool();
        if ok {
            FOUND_ID.store(id, Ordering::Relaxed);
            return Ok(id);
        }
    }
    Err(format!(
        "Shell_NotifyIconW(NIM_MODIFY, NIF_INFO) 對 uID 1–{MAX_PROBE_ID} 都失敗（系統匣圖示可能尚未登記，例如 explorer 重新啟動中）"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_wide_keeps_terminating_null_and_reports_truncation() {
        let mut buf = [0xFFFFu16; 8];
        assert!(!fill_wide(&mut buf, "財經桌布"));
        let want: Vec<u16> = "財經桌布".encode_utf16().collect();
        assert_eq!(&buf[..4], want.as_slice());
        assert!(buf[4..].iter().all(|&u| u == 0), "其餘位置清成 0");

        let mut small = [0u16; 4];
        assert!(fill_wide(&mut small, "abcdef"), "超過 N-1 要回報截斷");
        assert_eq!(small, [b'a' as u16, b'b' as u16, b'c' as u16, 0]);

        let mut exact = [0u16; 4];
        assert!(!fill_wide(&mut exact, "abc"), "剛好 N-1 不算截斷");
        assert_eq!(exact[3], 0);
    }

    #[test]
    fn fill_wide_never_splits_a_surrogate_pair() {
        // 「𠀀」（U+20000）是代理對；截斷點落在高代理之後時整個字元丟掉。
        let mut buf = [0u16; 3];
        assert!(fill_wide(&mut buf, "a𠀀b"));
        assert_eq!(buf, [b'a' as u16, 0, 0]);
    }

    #[test]
    fn notification_text_fits_shell_limits() {
        // Microsoft Learn：szInfoTitle 64、szInfo 256（皆含結尾 null）。
        let nid = balloon_data(
            1,
            1,
            &"標".repeat(100),
            &"文".repeat(400),
            BalloonKind::Warning,
        );
        assert_eq!(nid.szInfoTitle[63], 0);
        assert_eq!(nid.szInfo[255], 0);
        assert_eq!(nid.uFlags, NIF_INFO);
        assert_eq!(nid.dwInfoFlags, NIIF_WARNING);
        let empty = balloon_data(1, 1, "t", "", BalloonKind::Info);
        assert_ne!(empty.szInfo[0], 0, "空內文會移除通知，改成一個空白");
    }

    #[test]
    fn probe_order_tries_cached_id_first_without_duplicates() {
        assert_eq!(probe_order(0), (1..=MAX_PROBE_ID).collect::<Vec<_>>());
        let ids = probe_order(3);
        assert_eq!(ids[0], 3);
        assert_eq!(ids.len(), MAX_PROBE_ID as usize);
        assert_eq!(ids.iter().filter(|&&i| i == 3).count(), 1);
    }

    #[test]
    fn show_balloon_without_window_fails_without_calling_the_shell() {
        assert!(show_balloon(0, "t", "b", BalloonKind::Info).is_err());
    }

    /// 實機：自建一個隱藏視窗＋系統匣圖示（`uID`＝7，驗證探測找得到非 1 的識別碼），以
    /// [`show_balloon`] 發一則真的通知，6 秒後移除圖示。**會在桌面右下角跳出通知、短暫多一個系統匣
    /// 圖示**，不改桌布、不碰設定；只在手動執行時跑：
    /// `cargo test --bin fc-host -- --ignored tray_balloon::tests::real_balloon_on_own_icon --nocapture`
    #[test]
    #[ignore = "實機：會顯示一則真的系統匣通知"]
    fn real_balloon_on_own_icon() {
        use windows::core::w;
        use windows::Win32::UI::Shell::{NIF_ICON, NIF_TIP, NIM_ADD, NIM_DELETE};
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, LoadIconW, IDI_APPLICATION, WINDOW_EX_STYLE,
            WINDOW_STYLE,
        };

        const ID: u32 = 7;
        // SAFETY: 系統類別 STATIC、不可見、沒有父視窗；失敗時 windows crate 回 Err。
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("fc-host 4.8 tray balloon test"),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                None,
                None,
                None,
                None,
            )
        }
        .expect("建立測試視窗");
        // SAFETY: 系統預設圖示，不需釋放。
        let icon = unsafe { LoadIconW(None, IDI_APPLICATION) }.expect("載入系統圖示");
        let mut add = NOTIFYICONDATAW {
            cbSize: u32::try_from(std::mem::size_of::<NOTIFYICONDATAW>()).unwrap(),
            hWnd: hwnd,
            uID: ID,
            uFlags: NIF_ICON | NIF_TIP,
            hIcon: icon,
            ..Default::default()
        };
        fill_wide(&mut add.szTip, "fc-host 4.8 通知測試");
        // SAFETY: 結構由本函式擁有、cbSize 正確；NIM_ADD 不保存指標。
        let added = unsafe { Shell_NotifyIconW(NIM_ADD, &add) }.as_bool();
        println!("NIM_ADD(uID {ID}) = {added}");
        assert!(added, "登記測試圖示失敗");

        let text = crate::wallpaper_settings::notice_text(
            &crate::wallpaper_state::TakeoverNotice::Yielded {
                device_paths: Vec::new(),
            },
        );
        let result = show_balloon(hwnd.0 as isize, &text.title, &text.body, text.kind);
        println!(
            "show_balloon = {result:?}；標題「{}」；內文「{}」",
            text.title, text.body
        );
        std::thread::sleep(std::time::Duration::from_secs(6));

        let del = NOTIFYICONDATAW {
            cbSize: add.cbSize,
            hWnd: hwnd,
            uID: ID,
            ..Default::default()
        };
        // SAFETY: 同上；移除本測試登記的圖示、銷毀本測試建立的視窗。
        let deleted = unsafe { Shell_NotifyIconW(NIM_DELETE, &del) }.as_bool();
        let destroyed = unsafe { DestroyWindow(hwnd) };
        println!("NIM_DELETE = {deleted}；DestroyWindow = {destroyed:?}");
        assert_eq!(result, Ok(ID), "探測應找到 uID {ID}");
        assert!(deleted);
    }
}
