//! `--restore-wallpaper` 交給執行中的宿主（dynamic-wallpaper task 4.7b 裁決 5）。
//!
//! ## 去程：既有的 single-instance 通道
//!
//! 宿主（Primary）的 `tauri-plugin-single-instance` 2.5.0 建了一扇隱藏的交接視窗，類別名稱
//! `<identifier>-sic`、視窗名稱 `<identifier>-siw`（plugin 原始碼 `platform_impl/windows.rs`；本專案
//! 不開 `semver` feature，名稱不帶版本）。第二個行程以 `WM_COPYDATA`（`dwData`＝1542、內容
//! `"<工作目錄>|<參數1>|<參數2>…\0"`）送參數，Primary 的視窗程序把參數交給 `main.rs` 註冊的回呼。
//! 命令列行程**不經過** `tauri::Builder`（plugin 在 `build()` 內送完就 `process::exit(0)`，拿不到
//! 結果），改由本模組照同一格式自己送：[`send_copydata_to`] 以 `SendMessageTimeoutW`
//! （`SMTO_ABORTIFHUNG`）送出，宿主主執行緒卡住時不會永遠等。Primary 的回呼只投遞要求給桌布協調
//! 迴圈、立即返回（同步 WndProc 不做慢工作）。
//!
//! 依賴 plugin 的私有命名是取捨：`desktop/instance.rs` 的啟動仲裁刻意不依賴它，但「交給執行中的
//! 宿主」必須打到那扇視窗。[`APP_IDENTIFIER`] 有測試對 `tauri.conf.json`；plugin 升版改了命名時
//! 會落到「找不到交接視窗」（命令列結束碼 4），不會誤動作。
//!
//! ## 回程：具名事件
//!
//! 命令列行程先建立 N 個手動重設的具名事件 `Local\tw.fintools.fc-host.restore-reply.<命令列 PID>.<n>`
//! （[`ReplyEvents`]），交接參數帶 `--reply-pid <PID>`；Primary 還原完成後開啟並設定結果對應的那一個
//! （[`signal_reply`]），命令列以 `WaitForMultipleObjects` 等（上限由呼叫端決定）。`Local\`＝工作階段
//! 範圍，與單一執行個體一致。事件不帶資料，結果碼就是事件的序號。

use std::time::Duration;

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, HANDLE, LPARAM, WAIT_OBJECT_0, WAIT_TIMEOUT, WPARAM,
};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::Threading::{
    CreateEventW, OpenEventW, SetEvent, WaitForMultipleObjects, EVENT_MODIFY_STATE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, SendMessageTimeoutW, SMTO_ABORTIFHUNG, SMTO_BLOCK, WM_COPYDATA,
};

/// `tauri.conf.json` 的 `identifier`（plugin 以它命名交接視窗）。
pub const APP_IDENTIFIER: &str = "tw.fintools.fc-host";

/// plugin 的 `WMCOPYDATA_SINGLE_INSTANCE_DATA`。
const SINGLE_INSTANCE_COPYDATA_ID: usize = 1542;

/// 交接視窗的（類別名稱, 視窗名稱）。
pub fn plugin_window_names() -> (String, String) {
    (
        format!("{APP_IDENTIFIER}-sic"),
        format!("{APP_IDENTIFIER}-siw"),
    )
}

/// `WM_COPYDATA` 的內容（plugin 的格式：工作目錄與參數以 `|` 串接、結尾 NUL）。
pub fn copydata_payload(cwd: &str, args: &[String]) -> String {
    format!("{cwd}|{}\0", args.join("|"))
}

/// 回覆事件的名稱。
pub fn reply_event_name(pid: u32, slot: usize) -> String {
    format!("Local\\{APP_IDENTIFIER}.restore-reply.{pid}.{slot}")
}

/// 送出失敗的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendError {
    /// 找不到交接視窗（宿主正在結束、或交接視窗建立失敗）。
    NoWindow,
    /// `SendMessageTimeoutW` 逾時或失敗（宿主主執行緒卡住）。
    Failed(String),
}

/// 把參數送給執行中的宿主（以目前的工作目錄、plugin 的格式）。
pub fn send_handoff(args: &[String], timeout: Duration) -> Result<(), SendError> {
    let cwd = std::env::current_dir().unwrap_or_default();
    let payload = copydata_payload(&cwd.to_string_lossy(), args);
    let (class, window) = plugin_window_names();
    send_copydata_to(&class, &window, &payload, timeout)
}

/// [`send_handoff`] 的本體（視窗名稱可注入，測試用不存在的名稱）。
pub fn send_copydata_to(
    class: &str,
    window: &str,
    payload: &str,
    timeout: Duration,
) -> Result<(), SendError> {
    let class = HSTRING::from(class);
    let window = HSTRING::from(window);
    // SAFETY: 兩個 HSTRING 在呼叫期間有效、以 NUL 結尾；FindWindowW 只讀取它們。
    let hwnd = match unsafe { FindWindowW(PCWSTR(class.as_ptr()), PCWSTR(window.as_ptr())) } {
        Ok(h) if !h.is_invalid() => h,
        _ => return Err(SendError::NoWindow),
    };
    let bytes = payload.as_bytes();
    let cds = COPYDATASTRUCT {
        dwData: SINGLE_INSTANCE_COPYDATA_ID,
        cbData: u32::try_from(bytes.len()).map_err(|_| SendError::Failed("內容過長".into()))?,
        lpData: bytes.as_ptr() as *mut core::ffi::c_void,
    };
    let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
    let mut result = 0usize;
    // SAFETY: `cds` 與它指向的 `bytes` 在這次同步呼叫期間都有效；WM_COPYDATA 由系統把資料複製到
    // 接收端，接收端只在處理期間讀取。`result` 是本函式的區域變數。
    let ok = unsafe {
        SendMessageTimeoutW(
            hwnd,
            WM_COPYDATA,
            WPARAM(0),
            LPARAM(&cds as *const COPYDATASTRUCT as isize),
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            ms,
            Some(&mut result),
        )
    };
    if ok.0 == 0 {
        return Err(SendError::Failed(format!(
            "SendMessageTimeoutW 失敗或逾時（{} ms）：{}",
            ms,
            windows::core::Error::from_thread()
        )));
    }
    Ok(())
}

/// 命令列端的回覆事件組（drop 時關閉）。
pub struct ReplyEvents {
    handles: Vec<HANDLE>,
}

impl ReplyEvents {
    /// 建立 `slots` 個手動重設、初始未設定的具名事件。
    pub fn create(pid: u32, slots: usize) -> Result<Self, String> {
        let mut events = Self {
            handles: Vec::with_capacity(slots),
        };
        for slot in 0..slots {
            let name = HSTRING::from(reply_event_name(pid, slot));
            // SAFETY: 名稱在呼叫期間有效；預設安全屬性。
            let handle = unsafe { CreateEventW(None, true, false, PCWSTR(name.as_ptr())) }
                .map_err(|e| format!("建立回覆事件 {} 失敗：{e}", reply_event_name(pid, slot)))?;
            events.handles.push(handle);
        }
        Ok(events)
    }

    /// 等任一事件被設定，回傳它的序號；逾時或失敗回 `None`。
    pub fn wait(&self, timeout: Duration) -> Option<usize> {
        let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX - 1);
        // SAFETY: handles 都是本結構持有、尚未關閉的事件。
        let r = unsafe { WaitForMultipleObjects(&self.handles, false, ms) };
        if r == WAIT_TIMEOUT {
            return None;
        }
        let index = r.0.checked_sub(WAIT_OBJECT_0.0)? as usize;
        (index < self.handles.len()).then_some(index)
    }
}

impl Drop for ReplyEvents {
    fn drop(&mut self) {
        for h in self.handles.drain(..) {
            // SAFETY: 由 CreateEventW 取得，只在這裡關閉一次。
            let _ = unsafe { CloseHandle(h) };
        }
    }
}

/// Primary 端：設定命令列行程 `pid` 的第 `slot` 個回覆事件。事件不存在（命令列已逾時結束）時回錯誤。
pub fn signal_reply(pid: u32, slot: usize) -> Result<(), String> {
    let name = reply_event_name(pid, slot);
    let wide = HSTRING::from(name.as_str());
    // SAFETY: 名稱在呼叫期間有效；只要求設定狀態的權限。
    let handle = unsafe { OpenEventW(EVENT_MODIFY_STATE, false, PCWSTR(wide.as_ptr())) }
        .map_err(|e| format!("開啟回覆事件 {name} 失敗：{e}"))?;
    // SAFETY: handle 有效；設定後立即關閉。
    let set = unsafe { SetEvent(handle) };
    // SAFETY: 同上，只關閉一次。
    let _ = unsafe { CloseHandle(handle) };
    set.map_err(|e| format!("設定回覆事件 {name} 失敗：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::{Duration, Instant};

    /// 視窗名稱由 Tauri 設定的 identifier 決定：與 `tauri.conf.json` 不一致就找不到宿主。
    #[test]
    fn identifier_matches_tauri_conf() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
        assert_eq!(conf["identifier"], APP_IDENTIFIER);
        let (class, window) = plugin_window_names();
        assert_eq!(class, "tw.fintools.fc-host-sic");
        assert_eq!(window, "tw.fintools.fc-host-siw");
    }

    /// [審查 F4] 交接依賴 plugin 的私有協定（視窗名稱、`dwData`、格式）：版本必須精確釘住，升版要
    /// 重看 `platform_impl/windows.rs` 再改這裡；`semver` feature 會讓視窗名稱帶版本，不得開啟。
    #[test]
    fn single_instance_plugin_version_is_pinned() {
        let manifest = include_str!("../../Cargo.toml");
        let dep: Vec<&str> = manifest
            .lines()
            .filter(|l| l.trim_start().starts_with("tauri-plugin-single-instance"))
            .collect();
        assert_eq!(
            dep,
            ["tauri-plugin-single-instance = \"=2.5.0\""],
            "Cargo.toml 必須精確釘住 =2.5.0、不帶任何 feature（semver 會讓視窗名稱帶版本）"
        );
        let lock = include_str!("../../Cargo.lock");
        let entry = lock
            .split("[[package]]")
            .find(|p| p.contains("name = \"tauri-plugin-single-instance\""))
            .expect("Cargo.lock 有這個套件");
        assert!(
            entry.contains("version = \"2.5.0\""),
            "Cargo.lock 的 tauri-plugin-single-instance 不是 2.5.0：升版前先重看交接協定\n{entry}"
        );
    }

    /// 送出的內容與 plugin 的解析方式（`split('|')`：第一段是工作目錄，其餘是參數）對得上。
    #[test]
    fn copydata_payload_matches_the_plugin_parser() {
        let args = vec![
            "C:\\fc\\fc-host.exe".to_owned(),
            "--restore-wallpaper".to_owned(),
            "--reply-pid".to_owned(),
            "77".to_owned(),
        ];
        let payload = copydata_payload("C:\\Users\\u", &args);
        assert!(payload.ends_with('\0'));
        // plugin：CStr → split('|') → 第一段 cwd、其餘 args。
        let text = payload.trim_end_matches('\0');
        let mut parts = text.split('|');
        assert_eq!(parts.next(), Some("C:\\Users\\u"));
        assert_eq!(parts.map(str::to_owned).collect::<Vec<_>>(), args);
    }

    #[test]
    fn reply_event_names_are_per_process_and_slot() {
        assert_eq!(
            reply_event_name(1234, 2),
            "Local\\tw.fintools.fc-host.restore-reply.1234.2"
        );
        assert_ne!(reply_event_name(1, 0), reply_event_name(2, 0));
    }

    /// 真的具名事件：另一條執行緒以 `signal_reply` 設定第 2 個，等待端拿到 2；沒有人設定時逾時。
    #[test]
    fn reply_events_round_trip_and_time_out() {
        let pid = 0x5A00_0000 | std::process::id();
        let events = ReplyEvents::create(pid, 4).expect("建立事件");
        let t = Instant::now();
        assert_eq!(events.wait(Duration::from_millis(50)), None);
        assert!(t.elapsed() >= Duration::from_millis(40));

        let signaller = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            signal_reply(pid, 2)
        });
        assert_eq!(events.wait(Duration::from_secs(5)), Some(2));
        signaller.join().unwrap().expect("設定事件");
        assert!(signal_reply(pid, 9).is_err(), "不存在的事件回錯誤");
    }

    /// 宿主沒在跑時找不到交接視窗（測試用不存在的名稱，不碰真正執行中的宿主）。
    #[test]
    fn missing_window_is_reported_as_no_window() {
        let r = send_copydata_to(
            "fc-host-test-no-such-class",
            "fc-host-test-no-such-window",
            "x\0",
            Duration::from_millis(100),
        );
        assert_eq!(r, Err(SendError::NoWindow));
    }
}
