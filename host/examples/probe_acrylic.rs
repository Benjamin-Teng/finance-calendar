//! Task 1.2 Acrylic（毛玻璃）探針。
//!
//! 驗證 design.md D8／Risks 段的假設：DWM 系統背景材質（`DWMWA_SYSTEMBACKDROP_TYPE`）在小工具
//! `.focusable(false)`＋永不取得前景焦點的狀態下，是否仍會呈現模糊；不成立則毛玻璃一律停用、
//! 改純色（`host/src/desktop.rs` 的 `ACRYLIC_WORKS_UNFOCUSED_DEFAULT`）。
//!
//! ## 建立方式（比照 D8／`desktop.rs` 未來的正式呼叫端）
//! - `.focusable(false)`／`.focused(false)`：tao 產生 `WS_EX_NOACTIVATE`，本探針視窗設計上
//!   永遠不會取得鍵盤焦點或成為前景視窗。
//! - `.transparent(true)`＋頁面 `html,body{background:transparent}`：DWM 系統背景材質只在
//!   視窗背景真正透明（而非只是視覺上半透明的純色）時才會顯示——`window-vibrancy`
//!   （tauri `.effects()` 實際呼叫的 crate，見下方「為何不用 `.effects()`」）文件與原始碼皆要求
//!   視窗背景透明。
//! - 建立後以 `WS_EX_TOOLWINDOW`＋`SetWindowPos(HWND_BOTTOM, SWP_SHOWWINDOW|SWP_NOACTIVATE|
//!   SWP_NOMOVE|SWP_NOSIZE)` 完成顯示與置底，與 design.md D8「建立與顯示」一致，確保探針量到的
//!   是小工具實際會用的視窗樣式（而非一般可聚焦視窗）。
//!
//! ## 所用 DWM 屬性（Microsoft Learn 一手來源）
//! - `DWMWA_SYSTEMBACKDROP_TYPE`（`DWMWINDOWATTRIBUTE` 列舉之一）：
//!   <https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/ne-dwmapi-dwmwindowattribute>
//!   條目載明「This value is supported starting with Windows 11 Build 22621」。
//! - `DWM_SYSTEMBACKDROP_TYPE` 列舉值：
//!   <https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/ne-dwmapi-dwm_systembackdrop_type>
//!   本探針用 `DWMSBT_TRANSIENTWINDOW`（列舉頁「Transient window backdrop material」，即
//!   Windows 11 設計語言中的 Acrylic；`DWMSBT_MAINWINDOW`／`DWMSBT_TABBEDWINDOW` 分別對應
//!   Mica／Mica Alt，非本次驗證對象）。
//! - `DWMWA_WINDOW_CORNER_PREFERENCE`／`DWM_WINDOW_CORNER_PREFERENCE`（`DWMWCP_ROUND`）：
//!   <https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/ne-dwmapi-dwm_window_corner_preference>
//! - `DwmSetWindowAttribute`：
//!   <https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/nf-dwmapi-dwmsetwindowattribute>
//!
//! ## 為何不用 Tauri 內建 `.effects()`
//! 查過 `tauri-2.12.0/src/vibrancy/windows.rs` 原始碼：`.effects()` 在 Windows 上實際呼叫
//! `window_vibrancy::apply_acrylic()`（crate `window-vibrancy-0.8.1`）。查其
//! `src/windows.rs`：build 號 ≥ 22621（`is_backdroptype_supported()`）時確實呼叫
//! `DwmSetWindowAttribute(DWMWA_SYSTEMBACKDROP_TYPE, DWMSBT_TRANSIENTWINDOW)`（與本探針同一條
//! 路徑）；但 build 號更舊時會**退回呼叫未公開的 `SetWindowCompositionAttribute`**
//! （`ACCENT_ENABLE_ACRYLICBLURBEHIND`）。AGENTS.md／global-constraints 明文禁止未公開 API，
//! 故本專案改為自己直接呼叫 `DwmSetWindowAttribute`＋自行以
//! [`host/src/desktop.rs`]`::MIN_BUILD_FOR_SYSTEM_BACKDROP` 判斷 build 號是否支援，
//! 不支援就整個略過（不呼叫任何 API、含未公開的），而不是讓 `.effects()` 悄悄退回未公開 API。
//!
//! ## 探針設計：A／B 雙視窗對照
//! 同時建立兩個外觀完全相同（同尺寸、同 CSS、同置底方式）的視窗：
//! - **A（測試組）**：套用 `--backdrop-a`（預設 `transientwindow`）。
//! - **B（控制組）**：套用 `--backdrop-b`（預設 `none`＝顯式呼叫
//!   `DwmSetWindowAttribute(DWMWA_SYSTEMBACKDROP_TYPE, DWMSBT_NONE)`，不是「不呼叫」）。
//!
//! 兩者並排截圖，若 A 呈現模糊而 B 呈現黑底／穿透未模糊的原始桌面，即可排除「看起來像模糊其實是
//! WebView2 透明本身的視覺效果」的誤判。
//!
//! ## 兩個容易漏掉、導致誤判「毛玻璃不呈現」的細節（皆非本次驗證重點、但會汙染結果）
//! 1. **`DwmSetWindowAttribute` 之後必須用 `SWP_FRAMECHANGED` 讓 DWM 重新計算非用戶端
//!    渲染**：只呼叫 `DwmSetWindowAttribute` 不夠，DWM 不一定會在視窗尺寸／位置沒變動時
//!    重新評估材質；本探針的 `SetWindowPos(HWND_BOTTOM, ...)` 因此加了
//!    `SWP_FRAMECHANGED`（純 no-op 尺寸/位置變更，只逼 DWM 重算）。待查證：這條在
//!    Microsoft Learn 的 `DwmSetWindowAttribute`／`SetWindowPos` 頁面都沒有明文寫「材質
//!    生效需要這個」，是社群報告的實務作法（見報告檔的來源連結），非官方文件保證。
//! 2. **`--force-active-a`（選用，預設關）：對 A 視窗子類別化 `WM_NCACTIVATE`，一律回覆
//!    `TRUE`**：這是探針額外做的對照實驗，不是預設行為。社群報告指出 DWM 在視窗處於
//!    「未啟用」狀態時會把材質畫成沒有模糊的平面色（呼應 design.md Risks 段「Acrylic
//!    可能只在視窗取得焦點時呈現」）；`WM_NCACTIVATE` 是文件記載的標準訊息（非
//!    `SetWindowCompositionAttribute` 那類未公開 API），子類別化後一律回覆 `TRUE` 只影響
//!    DWM 怎麼畫非用戶端外觀，**不**呼叫 `SetForegroundWindow`／不改變真正的 Win32
//!    輸入焦點或 `WS_EX_NOACTIVATE` 行為。用來對照：預設（不開這個旗標）呈現的結果，才是
//!    design.md 風險登記要問的「小工具永不取得焦點時，毛玻璃預設會不會呈現」；開這個旗標
//!    後的結果，回答的是「如果不呈現，有沒有不靠未公開 API 的方式讓它呈現」。
//!
//! 執行（純 cargo；驅動腳本見 `host/tools/probe-1.2.ps1`，會失焦＋截圖）：
//! `cargo run --release --example probe_acrylic -- --log <記錄檔> --info <json 檔>`
//! 旗標見 [`parse_args`]。
#![windows_subsystem = "windows"]

use std::{
    env,
    fs::{self, File},
    io::Write,
    mem::size_of,
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

use tauri::{
    http::{header, Response, StatusCode},
    WebviewUrl, WebviewWindowBuilder,
};
use windows::{
    core::w,
    Win32::{
        Foundation::{ERROR_SUCCESS, HWND, LPARAM, LRESULT, WPARAM},
        Graphics::Dwm::{
            DwmSetWindowAttribute, DWMSBT_MAINWINDOW, DWMSBT_NONE, DWMSBT_TABBEDWINDOW,
            DWMSBT_TRANSIENTWINDOW, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_WINDOW_CORNER_PREFERENCE,
            DWMWCP_DEFAULT, DWMWCP_DONOTROUND, DWMWCP_ROUND, DWMWCP_ROUNDSMALL,
            DWM_SYSTEMBACKDROP_TYPE, DWM_WINDOW_CORNER_PREFERENCE,
        },
        System::{
            Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ},
            SystemInformation::GetLocalTime,
        },
        UI::{
            Shell::{DefSubclassProc, SetWindowSubclass},
            WindowsAndMessaging::{
                GetWindowLongPtrW, SendMessageW, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE,
                HWND_BOTTOM, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
                SWP_SHOWWINDOW, WM_NCACTIVATE, WS_EX_TOOLWINDOW,
            },
        },
    },
};

/// `DWMWA_SYSTEMBACKDROP_TYPE` 最低支援 build（Microsoft Learn `DWMWINDOWATTRIBUTE` 列舉頁，
/// 條目「This value is supported starting with Windows 11 Build 22621」）；與
/// `host/src/desktop.rs::MIN_BUILD_FOR_SYSTEM_BACKDROP` 同一份一手來源、同一個數字。
const MIN_BUILD_FOR_SYSTEM_BACKDROP: u32 = 22_621;

const SCHEME: &str = "probe";

/// 兩個視窗共用的頁面：全透明 `html/body`＋一片半透明毛玻璃色調的圓角面板；面板內文標出
/// 這扇窗套用的背景材質／圓角偏好，供截圖判讀時對照。
fn page(label: &str, backdrop_name: &str, corner_name: &str, build: &str, note: &str) -> String {
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>probe</title>
<style>
html,body{{margin:0;height:100%;background:transparent}}
.panel{{
  position:absolute; inset:0; border-radius:16px; box-sizing:border-box;
  background:rgba(18,22,30,0.55); border:1px solid rgba(255,255,255,0.14);
  color:#f1faee; font:14px/1.5 "Segoe UI",sans-serif; padding:14px 16px;
  -webkit-user-select:none;
}}
.title{{font-weight:700; font-size:16px; margin-bottom:6px}}
.row{{color:#cbd5e1; font-size:12px}}
.sample{{margin-top:10px; font-size:22px; font-weight:700; letter-spacing:.5px}}
</style></head>
<body><div class="panel">
<div class="title">{label}</div>
<div class="row">backdrop={backdrop_name} corner={corner_name}</div>
<div class="row">buildNumber={build}</div>
<div class="row">{note}</div>
<div class="sample">財經 2330 +9.98%</div>
</div></body></html>"#
    )
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Corner {
    Default,
    DoNotRound,
    Round,
    RoundSmall,
}

impl Corner {
    fn parse(s: &str) -> Self {
        match s {
            "donotround" => Corner::DoNotRound,
            "roundsmall" => Corner::RoundSmall,
            "default" => Corner::Default,
            _ => Corner::Round,
        }
    }
    fn value(self) -> DWM_WINDOW_CORNER_PREFERENCE {
        match self {
            Corner::Default => DWMWCP_DEFAULT,
            Corner::DoNotRound => DWMWCP_DONOTROUND,
            Corner::Round => DWMWCP_ROUND,
            Corner::RoundSmall => DWMWCP_ROUNDSMALL,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Corner::Default => "DWMWCP_DEFAULT",
            Corner::DoNotRound => "DWMWCP_DONOTROUND",
            Corner::Round => "DWMWCP_ROUND",
            Corner::RoundSmall => "DWMWCP_ROUNDSMALL",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Backdrop {
    None,
    Main,
    Transient,
    Tabbed,
}

impl Backdrop {
    fn parse(s: &str) -> Self {
        match s {
            "mainwindow" => Backdrop::Main,
            "tabbedwindow" => Backdrop::Tabbed,
            "none" => Backdrop::None,
            _ => Backdrop::Transient,
        }
    }
    fn value(self) -> DWM_SYSTEMBACKDROP_TYPE {
        match self {
            Backdrop::None => DWMSBT_NONE,
            Backdrop::Main => DWMSBT_MAINWINDOW,
            Backdrop::Transient => DWMSBT_TRANSIENTWINDOW,
            Backdrop::Tabbed => DWMSBT_TABBEDWINDOW,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Backdrop::None => "DWMSBT_NONE",
            Backdrop::Main => "DWMSBT_MAINWINDOW",
            Backdrop::Transient => "DWMSBT_TRANSIENTWINDOW",
            Backdrop::Tabbed => "DWMSBT_TABBEDWINDOW",
        }
    }
}

struct Args {
    log: PathBuf,
    info: PathBuf,
    x: Option<f64>,
    y: Option<f64>,
    width: f64,
    height: f64,
    gap: f64,
    corner: Corner,
    backdrop_a: Backdrop,
    backdrop_b: Backdrop,
    monitor: usize,
    /// 見檔頭文件「兩個容易漏掉的細節」第 2 點：對照實驗，預設關閉。
    force_active_a: bool,
}

/// 旗標：`--log`／`--info`（路徑）、`--x`／`--y`（邏輯像素，未給時放在所選螢幕工作區右上角，
/// 與正式小工具「版面錨定右上」一致）、`--width`／`--height`／`--gap`（A／B 兩窗之間垂直間距，
/// 邏輯像素）、`--corner default|donotround|round|roundsmall`、
/// `--backdrop-a`／`--backdrop-b none|mainwindow|transientwindow|tabbedwindow`、
/// `--monitor <index>`（`available_monitors()` 的索引，預設 0；多螢幕時供人工驗收指定外接
/// 螢幕）、`--force-active-a true`（見檔頭「兩個容易漏掉的細節」第 2 點，預設 `false`）。
fn parse_args() -> Args {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut a = Args {
        log: manifest.join("target").join("probe_acrylic.log"),
        info: manifest.join("target").join("probe_acrylic.json"),
        x: None,
        y: None,
        width: 360.0,
        height: 190.0,
        gap: 24.0,
        corner: Corner::Round,
        backdrop_a: Backdrop::Transient,
        backdrop_b: Backdrop::None,
        monitor: 0,
        force_active_a: false,
    };
    let mut it = env::args().skip(1);
    while let Some(k) = it.next() {
        let v = it.next().unwrap_or_default();
        match k.as_str() {
            "--log" => a.log = PathBuf::from(v),
            "--info" => a.info = PathBuf::from(v),
            "--x" => a.x = v.parse().ok(),
            "--y" => a.y = v.parse().ok(),
            "--width" => a.width = v.parse().unwrap_or(a.width),
            "--height" => a.height = v.parse().unwrap_or(a.height),
            "--gap" => a.gap = v.parse().unwrap_or(a.gap),
            "--corner" => a.corner = Corner::parse(&v),
            "--backdrop-a" => a.backdrop_a = Backdrop::parse(&v),
            "--backdrop-b" => a.backdrop_b = Backdrop::parse(&v),
            "--monitor" => a.monitor = v.parse().unwrap_or(0),
            "--force-active-a" => a.force_active_a = v != "false",
            _ => {}
        }
    }
    a
}

static LOG: OnceLock<Mutex<File>> = OnceLock::new();

fn now() -> String {
    // SAFETY: GetLocalTime 只寫回傳值，無前置條件。
    let t = unsafe { GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, t.wMilliseconds
    )
}

fn log(msg: &str) {
    if let Some(m) = LOG.get() {
        if let Ok(mut f) = m.lock() {
            let _ = writeln!(f, "{} {}", now(), msg);
        }
    }
}

/// 讀取目前作業系統的 build 號：與 `host/src/desktop.rs::os_build_number()` 同一份一手來源
/// （`HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows NT\CurrentVersion` 的
/// `CurrentBuildNumber`，`REG_SZ`）。探針獨立於 `src/`（本 repo 只有 `[[bin]]`、無
/// `[lib]`，examples 連結不到 `crate::desktop`），故在此重新實作同一段讀法而非匯入。
/// 讀不到回傳 `None`。
fn os_build_number() -> Option<u32> {
    const BUF_LEN: usize = 64;
    let mut buf = [0u16; BUF_LEN];
    let mut len: u32 = (buf.len() * size_of::<u16>()) as u32;
    // SAFETY: buf 的位元組長度與傳入的 len 一致，RegGetValueW 不會寫超過此長度。
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion"),
            w!("CurrentBuildNumber"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut len),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end]).trim().parse().ok()
}

fn set_corner(hwnd: HWND, pref: DWM_WINDOW_CORNER_PREFERENCE) -> windows::core::Result<()> {
    // SAFETY: hwnd 為本行程剛建立的頂層視窗；pref 的大小與 DWMWA_WINDOW_CORNER_PREFERENCE
    // 要求的 DWM_WINDOW_CORNER_PREFERENCE 完全一致。
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            (&pref as *const DWM_WINDOW_CORNER_PREFERENCE).cast(),
            size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
        )
    }
}

fn set_backdrop(hwnd: HWND, kind: DWM_SYSTEMBACKDROP_TYPE) -> windows::core::Result<()> {
    // SAFETY: 同上，kind 的大小與 DWMWA_SYSTEMBACKDROP_TYPE 要求的 DWM_SYSTEMBACKDROP_TYPE
    // 完全一致。
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            (&kind as *const DWM_SYSTEMBACKDROP_TYPE).cast(),
            size_of::<DWM_SYSTEMBACKDROP_TYPE>() as u32,
        )
    }
}

/// `--force-active-a` 對照實驗：子類別化 `WM_NCACTIVATE`，一律回覆 `TRUE`（`lParam=-1`，
/// 社群慣例避免觸發預設重繪路徑），讓 DWM 一直用「使用中」的方式畫非用戶端外觀（含系統背景
/// 材質），藉此觀察「若預設不呈現，這個文件記載的訊息能不能在不搶真正焦點的情況下讓它呈現」。
/// `WM_NCACTIVATE` 是標準 Win32 訊息（非 `SetWindowCompositionAttribute` 那類未公開 API），
/// 不呼叫 `SetForegroundWindow`、不動 `WS_EX_NOACTIVATE`、不影響真正的鍵盤輸入焦點。
unsafe extern "system" fn force_active_subclass(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if msg == WM_NCACTIVATE {
        // SAFETY: 轉交下一個子類別程序／原視窗程序；固定回覆「使用中」。
        return unsafe { DefSubclassProc(hwnd, WM_NCACTIVATE, WPARAM(1), LPARAM(-1)) };
    }
    // SAFETY: 其餘訊息原樣轉交。
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

/// 視窗最終矩形（物理像素）：`(x, y, width, height)`。
type WindowRect = (i32, i32, i32, i32);

/// 建立單一探針視窗：D8「建立與顯示」＋「不搶焦點」的動作序列，回傳其 HWND 與最終
/// （物理像素）矩形。
fn create_probe_window(
    app: &tauri::AppHandle,
    label: &str,
    url_path: &str,
    x: f64,
    y: f64,
    a: &Args,
    backdrop: Backdrop,
) -> Result<(HWND, WindowRect), Box<dyn std::error::Error>> {
    let url = tauri::Url::parse(&format!("http://{SCHEME}.localhost/{url_path}"))?;
    let win = WebviewWindowBuilder::new(app, label, WebviewUrl::External(url))
        .title(label)
        .decorations(false)
        .skip_taskbar(true)
        .focusable(false)
        .focused(false)
        .resizable(false)
        .shadow(false)
        .transparent(true)
        .visible(false)
        .inner_size(a.width, a.height)
        .position(x, y)
        .build()?;
    let hwnd = win.hwnd()?;

    // SAFETY: hwnd 為本行程剛建立、尚未顯示的頂層視窗；只讀寫其樣式旗標。
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | WS_EX_TOOLWINDOW.0 as isize);
    }

    if label == "probe-a" && a.force_active_a {
        // SAFETY: hwnd 為本行程剛建立的頂層視窗；subclass_proc 生命週期為 'static。
        let ok = unsafe { SetWindowSubclass(hwnd, Some(force_active_subclass), 1, 0) };
        // `WS_EX_NOACTIVATE` 視窗設計上永遠不會被 Windows 送真正的啟用／停用循環，也就永遠
        // 不會自然收到任何 WM_NCACTIVATE——上面的子類別只能攔「以後收到的」訊息，攔不到
        // 「本來就不會發生的」訊息。故這裡額外主動送一次，建立初始的「使用中」非用戶端渲染
        // 狀態；子類別留著是為了防範日後真的有東西送 FALSE 進來（例如其他視窗啟用時系統
        // 廣播）。
        // SAFETY: hwnd 為本行程剛建立的有效頂層視窗；SendMessageW 對 WM_NCACTIVATE 是同步
        // 呼叫，訊息處理完才返回。
        let sent = unsafe { SendMessageW(hwnd, WM_NCACTIVATE, Some(WPARAM(1)), Some(LPARAM(-1))) };
        log(&format!(
            "FORCE-ACTIVE label={label} SetWindowSubclass={} initial-SendMessageW(WM_NCACTIVATE,TRUE)={}",
            ok.as_bool(),
            sent.0
        ));
    }

    let corner_result = set_corner(hwnd, a.corner.value());
    log(&format!(
        "DWM label={label} corner={} result={:?}",
        a.corner.name(),
        corner_result
    ));
    let backdrop_result = set_backdrop(hwnd, backdrop.value());
    log(&format!(
        "DWM label={label} backdrop={} result={:?}",
        backdrop.name(),
        backdrop_result
    ));

    // SAFETY: 一次完成顯示與置底（design.md D8：`visible(false)` 建立後
    // `SetWindowPos(HWND_BOTTOM, SWP_SHOWWINDOW|SWP_NOACTIVATE|SWP_NOMOVE|SWP_NOSIZE)`），
    // 避免建立瞬間短暫出現在上層或被啟用。額外加 `SWP_FRAMECHANGED`（檔頭「兩個容易漏掉的
    // 細節」第 1 點）：純粹是 no-op 的尺寸/位置變更旗標，逼 DWM 在
    // `DwmSetWindowAttribute` 之後重新計算非用戶端渲染，不然材質可能不會生效。
    unsafe {
        SetWindowPos(
            hwnd,
            Some(HWND_BOTTOM),
            0,
            0,
            0,
            0,
            SWP_SHOWWINDOW | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_FRAMECHANGED,
        )?;
    }

    let pos = win.outer_position()?;
    let size = win.outer_size()?;
    let rect = (pos.x, pos.y, size.width as i32, size.height as i32);
    log(&format!(
        "CREATED label={label} hwnd=0x{:X} rect={rect:?}",
        hwnd.0 as usize
    ));
    Ok((hwnd, rect))
}

fn setup(app: &mut tauri::App, a: &Args) -> Result<(), Box<dyn std::error::Error>> {
    let monitors = app.available_monitors()?;
    let mon = monitors
        .get(a.monitor)
        .or_else(|| monitors.first())
        .cloned()
        .ok_or("no monitor")?;
    let scale = mon.scale_factor();
    let work_right = (mon.work_area().position.x + mon.work_area().size.width as i32) as f64;
    let work_top = mon.work_area().position.y as f64;
    let (x, y_a) = match (a.x, a.y) {
        (Some(x), Some(y)) => (x, y),
        _ => (work_right / scale - a.width - 40.0, work_top / scale + 60.0),
    };
    let y_b = y_a + a.height + a.gap;

    let build = os_build_number();
    let build_str = build
        .map(|b| b.to_string())
        .unwrap_or_else(|| "未知".into());
    let build_ok = build.is_some_and(|b| b >= MIN_BUILD_FOR_SYSTEM_BACKDROP);
    log(&format!(
        "START buildNumber={build_str} minBuildForBackdrop={MIN_BUILD_FOR_SYSTEM_BACKDROP} \
         buildSupportsBackdrop={build_ok} monitorIndex={} scale={scale}",
        a.monitor
    ));
    if !build_ok {
        log("WARN build 號未知或低於 22621：DWMWA_SYSTEMBACKDROP_TYPE 預期不呈現任何材質（回退為視窗預設外觀），這本身也是一種有效結論。");
    }

    let (hwnd_a, rect_a) = create_probe_window(
        &app.handle().clone(),
        "probe-a",
        &format!(
            "a?backdrop={}&corner={}",
            a.backdrop_a.name(),
            a.corner.name()
        ),
        x,
        y_a,
        a,
        a.backdrop_a,
    )?;
    let (hwnd_b, rect_b) = create_probe_window(
        &app.handle().clone(),
        "probe-b",
        &format!(
            "b?backdrop={}&corner={}",
            a.backdrop_b.name(),
            a.corner.name()
        ),
        x,
        y_b,
        a,
        a.backdrop_b,
    )?;

    let info = format!(
        "{{\"pid\":{},\"buildNumber\":{},\"buildSupportsBackdrop\":{build_ok},\
         \"minBuildForBackdrop\":{MIN_BUILD_FOR_SYSTEM_BACKDROP},\"corner\":\"{}\",\
         \"a\":{{\"label\":\"probe-a\",\"hwnd\":\"0x{:X}\",\"backdrop\":\"{}\",\"rect\":[{},{},{},{}]}},\
         \"b\":{{\"label\":\"probe-b\",\"hwnd\":\"0x{:X}\",\"backdrop\":\"{}\",\"rect\":[{},{},{},{}]}}}}",
        std::process::id(),
        build.map(|b| b.to_string()).unwrap_or_else(|| "null".into()),
        a.corner.name(),
        hwnd_a.0 as usize,
        a.backdrop_a.name(),
        rect_a.0,
        rect_a.1,
        rect_a.2,
        rect_a.3,
        hwnd_b.0 as usize,
        a.backdrop_b.name(),
        rect_b.0,
        rect_b.1,
        rect_b.2,
        rect_b.3,
    );
    fs::write(&a.info, &info)?;
    log(&format!("READY {info}"));
    Ok(())
}

fn main() {
    let a = parse_args();
    if let Some(dir) = a.log.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let file = File::create(&a.log).expect("無法建立記錄檔");
    let _ = LOG.set(Mutex::new(file));

    tauri::Builder::default()
        .register_uri_scheme_protocol(SCHEME, |_ctx, req| {
            let path = req.uri().path().trim_start_matches('/');
            let query = req.uri().query().unwrap_or("");
            let mut backdrop_name = "?";
            let mut corner_name = "?";
            for kv in query.split('&') {
                match kv.split_once('=') {
                    Some(("backdrop", v)) => backdrop_name = v,
                    Some(("corner", v)) => corner_name = v,
                    _ => {}
                }
            }
            let build = os_build_number()
                .map(|b| b.to_string())
                .unwrap_or_else(|| "未知".into());
            let (label, note) = match path.chars().next() {
                Some('a') => ("A（測試組）", "背景是否呈現模糊＝本次探針的驗收重點"),
                _ => (
                    "B（控制組）",
                    "backdrop=NONE，用來對照 A 是否真的有額外效果",
                ),
            };
            let body = page(label, backdrop_name, corner_name, &build, note);
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                .body(body.into_bytes())
                .expect("靜態回應不會失敗")
        })
        .setup(move |app| {
            setup(app, &a)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("probe_acrylic 執行失敗");
}
