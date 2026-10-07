//! Task 1.1 Win+D（顯示桌面）探針。
//!
//! 開一個與正式小工具相同組態的 Tauri 視窗（無邊框、`always_on_bottom`、不可聚焦、
//! `WS_EX_TOOLWINDOW`、以 `visible(false)` 建立後用 `SetWindowPos(HWND_BOTTOM)` 顯示），
//! 並以 design.md D8 的判準偵測「顯示桌面」：
//!
//! - 判準：由 z-order 頂端往下走，略過自己、非可見／最小化／cloaked、`WS_EX_TOPMOST`
//!   視窗後，**第一個**遇到的視窗若是殼層行程（`GetShellWindow` 的 PID）的
//!   `Progman`／`WorkerW` ＝桌面位於一般視窗之上＝進入顯示桌面；若是一般應用程式視窗
//!   （非 tool window、非零尺寸）＝離開。
//! - 進入：設旗標讓子類別化的 `WM_WINDOWPOSCHANGING` 在 tao 改寫成 `HWND_BOTTOM` 之後把
//!   `hwndInsertAfter` 還原成原本的請求值（等同暫停 tao 的強制置底，但不呼叫
//!   `set_always_on_bottom(false)`，避免其 `HWND_NOTOPMOST` 與 `GWL_EXSTYLE` 重寫），再以
//!   `SetWindowPos` 把自己插到桌面視窗正上方。
//! - 離開：清旗標並 `SetWindowPos(HWND_BOTTOM)`。
//! - 偵測時機：`SetWinEventHook`（前景、最小化、cloak、顯示／隱藏、reorder）與主執行緒
//!   `SetTimer` 輪詢併用，記錄檔會標出每次狀態轉換由哪個來源觸發，以判斷哪種時機足夠。
//!
//! 所有邏輯都跑在主執行緒（WinEvent out-of-context 回呼與 thread timer 都由主執行緒的
//! 訊息迴圈派送），不需鎖、也不會跨執行緒 `SetWindowPos` 互等。
//!
//! 執行（純 cargo）：
//! `cargo run --release --example probe_wind -- --log <記錄檔> --info <json 檔>`
//! 旗標見 [`parse_args`]。驅動腳本見 `host/tools/probe-1.1.ps1`。
#![windows_subsystem = "windows"]

use std::{
    env,
    fs::{self, File},
    io::Write,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering},
        Mutex, OnceLock,
    },
};

use tauri::{
    http::{header, Response, StatusCode},
    WebviewUrl, WebviewWindowBuilder,
};
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED},
    System::SystemInformation::GetLocalTime,
    UI::{
        Accessibility::{SetWinEventHook, HWINEVENTHOOK},
        Shell::{DefSubclassProc, SetWindowSubclass},
        WindowsAndMessaging::{
            GetClassNameW, GetShellWindow, GetTopWindow, GetWindow, GetWindowLongPtrW,
            GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindowVisible, SetTimer,
            SetWindowLongPtrW, SetWindowPos, EVENT_OBJECT_CLOAKED, EVENT_OBJECT_HIDE,
            EVENT_OBJECT_REORDER, EVENT_OBJECT_SHOW, EVENT_OBJECT_UNCLOAKED,
            EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND, EVENT_SYSTEM_MINIMIZESTART,
            GWL_EXSTYLE, GW_HWNDNEXT, GW_HWNDPREV, HWND_BOTTOM, HWND_TOP, SWP_NOACTIVATE,
            SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, WINDOWPOS, WINEVENT_OUTOFCONTEXT,
            WM_WINDOWPOSCHANGING, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
        },
    },
};

const SCHEME: &str = "probe";
const PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>probe</title>
<style>html,body{margin:0;height:100%;background:#1d3557;color:#f1faee;font:bold 28px sans-serif;
display:flex;align-items:center;justify-content:center;text-align:center}</style></head>
<body><div>Win+D 探針<br><span style="font-size:16px">task 1.1 probe_wind</span></div></body></html>"#;

/// 插入位置策略。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum InsertMode {
    /// 插在桌面視窗正上方（`hwndInsertAfter` ＝桌面視窗的上一個視窗）。
    AboveDesktop,
    /// 非 topmost 頂端（`HWND_TOP`）。
    Top,
}

/// 偵測時機。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Detect {
    Event,
    Poll,
    Both,
}

struct Args {
    log: PathBuf,
    info: PathBuf,
    insert: InsertMode,
    detect: Detect,
    poll_ms: u32,
    width: f64,
    height: f64,
    /// 視窗左上角（邏輯像素，主螢幕座標）；未給時放主螢幕右上角。
    pos: Option<(f64, f64)>,
}

/// 旗標：`--log <path>`、`--info <path>`、`--insert above-desktop|top`、
/// `--detect event|poll|both`、`--poll-ms <n>`、`--width`／`--height`（邏輯像素）、
/// `--x`／`--y`（邏輯像素）。
fn parse_args() -> Args {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut a = Args {
        log: manifest.join("target").join("probe_wind.log"),
        info: manifest.join("target").join("probe_wind.json"),
        insert: InsertMode::AboveDesktop,
        detect: Detect::Both,
        poll_ms: 250,
        width: 420.0,
        height: 300.0,
        pos: None,
    };
    let mut x: Option<f64> = None;
    let mut y: Option<f64> = None;
    let mut it = env::args().skip(1);
    while let Some(k) = it.next() {
        let v = it.next().unwrap_or_default();
        match k.as_str() {
            "--log" => a.log = PathBuf::from(v),
            "--info" => a.info = PathBuf::from(v),
            "--insert" => {
                a.insert = if v == "top" {
                    InsertMode::Top
                } else {
                    InsertMode::AboveDesktop
                }
            }
            "--detect" => {
                a.detect = match v.as_str() {
                    "event" => Detect::Event,
                    "poll" => Detect::Poll,
                    _ => Detect::Both,
                }
            }
            "--poll-ms" => a.poll_ms = v.parse().unwrap_or(a.poll_ms),
            "--width" => a.width = v.parse().unwrap_or(a.width),
            "--height" => a.height = v.parse().unwrap_or(a.height),
            "--x" => x = v.parse().ok(),
            "--y" => y = v.parse().ok(),
            _ => {}
        }
    }
    if let (Some(x), Some(y)) = (x, y) {
        a.pos = Some((x, y));
    }
    a
}

// ---- 全域狀態（只在主執行緒讀寫；用 atomic 只是為了能放在 static） ----

static WIDGET: AtomicIsize = AtomicIsize::new(0);
static DESKTOP_MODE: AtomicBool = AtomicBool::new(false);
static INSERT_TOP: AtomicBool = AtomicBool::new(false);
static REASSERTS: AtomicU32 = AtomicU32::new(0);
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

fn widget() -> HWND {
    HWND(WIDGET.load(Ordering::Relaxed) as *mut _)
}

fn class_of(h: HWND) -> String {
    let mut buf = [0u16; 256];
    // SAFETY: buf 為有效可寫緩衝區；h 失效時 API 回傳 0。
    let n = unsafe { GetClassNameW(h, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn pid_of(h: HWND) -> u32 {
    let mut pid = 0u32;
    // SAFETY: pid 為有效輸出指標。
    unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
    pid
}

fn ex_style(h: HWND) -> u32 {
    // SAFETY: 讀取任意視窗的樣式，h 失效時回傳 0。
    (unsafe { GetWindowLongPtrW(h, GWL_EXSTYLE) }) as u32
}

fn is_cloaked(h: HWND) -> bool {
    let mut v: u32 = 0;
    // SAFETY: v 為 4 位元組可寫緩衝區，大小與 cbattribute 一致。
    let r = unsafe {
        DwmGetWindowAttribute(
            h,
            DWMWA_CLOAKED,
            (&mut v as *mut u32).cast(),
            std::mem::size_of::<u32>() as u32,
        )
    };
    r.is_ok() && v != 0
}

fn shown(h: HWND) -> bool {
    // SAFETY: 純查詢。
    unsafe { IsWindowVisible(h).as_bool() && !IsIconic(h).as_bool() && !is_cloaked(h) }
}

fn has_area(h: HWND) -> bool {
    let mut r = RECT::default();
    // SAFETY: r 為有效輸出指標。
    unsafe { GetWindowRect(h, &mut r) }.is_ok() && r.right > r.left && r.bottom > r.top
}

fn next(h: HWND) -> Option<HWND> {
    // SAFETY: 純查詢；失敗（到底）回 Err。
    unsafe { GetWindow(h, GW_HWNDNEXT) }.ok()
}

fn hex(h: HWND) -> String {
    format!("0x{:X}", h.0 as usize)
}

/// z-order 頂端第一個「一般視窗或桌面視窗」的分類結果。
enum Top {
    /// 殼層的 Progman／WorkerW 位於所有一般視窗之上。
    Desktop(HWND),
    /// 一般應用程式視窗位於桌面之上。
    App(HWND),
    Nothing,
}

fn scan_top(own: HWND) -> Top {
    // SAFETY: 純查詢。
    let shell_pid = pid_of(unsafe { GetShellWindow() });
    // SAFETY: 純查詢。
    let mut cur = unsafe { GetTopWindow(None) }.ok();
    while let Some(h) = cur {
        cur = next(h);
        if h == own || !shown(h) || ex_style(h) & WS_EX_TOPMOST.0 != 0 {
            continue;
        }
        let cls = class_of(h);
        if (cls == "Progman" || cls == "WorkerW") && pid_of(h) == shell_pid {
            return Top::Desktop(h);
        }
        if ex_style(h) & WS_EX_TOOLWINDOW.0 == 0 && has_area(h) {
            return Top::App(h);
        }
    }
    Top::Nothing
}

/// `own` 是否在 z-order 中位於 `desk` 之上。
fn is_above(own: HWND, desk: HWND) -> bool {
    // SAFETY: 純查詢。
    let mut cur = unsafe { GetTopWindow(None) }.ok();
    while let Some(h) = cur {
        if h == own {
            return true;
        }
        if h == desk {
            return false;
        }
        cur = next(h);
    }
    false
}

fn insert_above_desktop(own: HWND, desk: HWND, why: &str) {
    let after = if INSERT_TOP.load(Ordering::Relaxed) {
        HWND_TOP
    } else {
        // SAFETY: 純查詢。
        match unsafe { GetWindow(desk, GW_HWNDPREV) } {
            Ok(p) if p == own => return,
            Ok(p) if ex_style(p) & WS_EX_TOPMOST.0 == 0 => p,
            _ => HWND_TOP,
        }
    };
    // SAFETY: own 為本行程的有效頂層視窗；不啟用、不移動、不改尺寸。
    let r = unsafe {
        SetWindowPos(
            own,
            Some(after),
            0,
            0,
            0,
            0,
            SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
        )
    };
    log(&format!(
        "ACTION insert-above-desktop why={why} desk={}({}) after={} ok={}",
        hex(desk),
        class_of(desk),
        if after == HWND_TOP {
            "HWND_TOP".to_string()
        } else {
            format!("{}({})", hex(after), class_of(after))
        },
        r.is_ok()
    ));
}

fn send_bottom(own: HWND, why: &str) {
    // SAFETY: 同上。
    let r = unsafe {
        SetWindowPos(
            own,
            Some(HWND_BOTTOM),
            0,
            0,
            0,
            0,
            SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
        )
    };
    log(&format!("ACTION send-bottom why={why} ok={}", r.is_ok()));
}

/// 核心：依目前 z-order 決定進入／離開顯示桌面模式。
fn evaluate(source: &str) {
    let own = widget();
    if own.0.is_null() {
        return;
    }
    let mode = DESKTOP_MODE.load(Ordering::Relaxed);
    match scan_top(own) {
        Top::Desktop(desk) => {
            if !mode {
                DESKTOP_MODE.store(true, Ordering::Relaxed);
                log(&format!(
                    "ENTER source={source} desk={}({})",
                    hex(desk),
                    class_of(desk)
                ));
                insert_above_desktop(own, desk, "enter");
            } else if !is_above(own, desk) {
                let n = REASSERTS.fetch_add(1, Ordering::Relaxed) + 1;
                log(&format!(
                    "REASSERT#{n} source={source} desk={}({}) widget-was-below-desktop",
                    hex(desk),
                    class_of(desk)
                ));
                insert_above_desktop(own, desk, "reassert");
            }
        }
        Top::App(app) => {
            if mode {
                DESKTOP_MODE.store(false, Ordering::Relaxed);
                log(&format!(
                    "EXIT source={source} top-app={}({}) pid={}",
                    hex(app),
                    class_of(app),
                    pid_of(app)
                ));
                send_bottom(own, "exit");
            }
        }
        Top::Nothing => {}
    }
}

fn event_name(e: u32) -> &'static str {
    match e {
        x if x == EVENT_SYSTEM_FOREGROUND => "FOREGROUND",
        x if x == EVENT_SYSTEM_MINIMIZESTART => "MINIMIZESTART",
        x if x == EVENT_SYSTEM_MINIMIZEEND => "MINIMIZEEND",
        x if x == EVENT_OBJECT_SHOW => "SHOW",
        x if x == EVENT_OBJECT_HIDE => "HIDE",
        x if x == EVENT_OBJECT_REORDER => "REORDER",
        x if x == EVENT_OBJECT_CLOAKED => "CLOAKED",
        x if x == EVENT_OBJECT_UNCLOAKED => "UNCLOAKED",
        _ => "OTHER",
    }
}

unsafe extern "system" fn win_event(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    // 只關心視窗本身（OBJID_WINDOW = 0）；REORDER 以容器身分送出，不限 id。
    if event != EVENT_OBJECT_REORDER && id_object != 0 {
        return;
    }
    let name = event_name(event);
    if event == EVENT_SYSTEM_FOREGROUND {
        log(&format!(
            "EVENT FOREGROUND hwnd={}({}) pid={}",
            hex(hwnd),
            class_of(hwnd),
            pid_of(hwnd)
        ));
    }
    let src = format!("event:{name}:{}", class_of(hwnd));
    evaluate(&src);
}

unsafe extern "system" fn timer_proc(_h: HWND, _msg: u32, _id: usize, _t: u32) {
    evaluate("poll");
}

/// 子類別化：tao 在 `WM_WINDOWPOSCHANGING` 一律把 `hwndInsertAfter` 改成
/// `HWND_BOTTOM`；顯示桌面模式時在 tao 處理之後還原成原請求值。
unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if msg != WM_WINDOWPOSCHANGING {
        // SAFETY: 轉交下一個子類別程序／原視窗程序。
        return unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
    }
    let wp = lparam.0 as *mut WINDOWPOS;
    // SAFETY: WM_WINDOWPOSCHANGING 的 lparam 保證指向有效 WINDOWPOS。
    let (requested, flags) = unsafe { ((*wp).hwndInsertAfter, (*wp).flags) };
    // SAFETY: 同上。
    let r = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
    if flags.0 & SWP_NOZORDER.0 == 0 {
        let mode = DESKTOP_MODE.load(Ordering::Relaxed);
        if mode {
            // SAFETY: 同上。
            unsafe { (*wp).hwndInsertAfter = requested };
        }
        let req = if requested == HWND_BOTTOM {
            "HWND_BOTTOM".to_string()
        } else if requested == HWND_TOP {
            "HWND_TOP".to_string()
        } else {
            format!("{}({})", hex(requested), class_of(requested))
        };
        log(&format!(
            "WPC requested={req} desktop_mode={mode} final={}",
            if mode {
                "requested"
            } else {
                "HWND_BOTTOM(tao)"
            }
        ));
    }
    r
}

fn setup(app: &mut tauri::App, a: &Args) -> Result<(), Box<dyn std::error::Error>> {
    let mon = app
        .primary_monitor()?
        .or_else(|| app.available_monitors().ok()?.into_iter().next())
        .ok_or("no monitor")?;
    let scale = mon.scale_factor();
    let (x, y) = a.pos.unwrap_or_else(|| {
        let right = (mon.position().x + mon.size().width as i32) as f64 / scale;
        (
            right - a.width - 40.0,
            mon.position().y as f64 / scale + 80.0,
        )
    });
    let url = tauri::Url::parse(&format!("http://{SCHEME}.localhost/"))?;
    let win = WebviewWindowBuilder::new(app, "probe", WebviewUrl::External(url))
        .title("probe_wind")
        .decorations(false)
        .always_on_bottom(true)
        .skip_taskbar(true)
        .focusable(false)
        .focused(false)
        .resizable(false)
        .shadow(false)
        .visible(false)
        .inner_size(a.width, a.height)
        .position(x, y)
        .build()?;
    let hwnd = win.hwnd()?;
    WIDGET.store(hwnd.0 as isize, Ordering::Relaxed);

    // SAFETY: hwnd 為本行程剛建立的頂層視窗；subclass_proc 生命週期為 'static。
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | WS_EX_TOOLWINDOW.0 as isize);
        if !SetWindowSubclass(hwnd, Some(subclass_proc), 1, 0).as_bool() {
            return Err("SetWindowSubclass failed".into());
        }
        SetWindowPos(
            hwnd,
            Some(HWND_BOTTOM),
            0,
            0,
            0,
            0,
            SWP_SHOWWINDOW | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
        )?;
    }

    if matches!(a.detect, Detect::Event | Detect::Both) {
        let ranges = [
            (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
            (EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND),
            (EVENT_OBJECT_SHOW, EVENT_OBJECT_REORDER),
            (EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED),
        ];
        for (lo, hi) in ranges {
            // SAFETY: out-of-context 掛鉤，回呼在本（主）執行緒的訊息迴圈派送；
            // 掛鉤存活到行程結束。
            let h = unsafe {
                SetWinEventHook(lo, hi, None, Some(win_event), 0, 0, WINEVENT_OUTOFCONTEXT)
            };
            if h.is_invalid() {
                return Err(format!("SetWinEventHook({lo:#x}..{hi:#x}) failed").into());
            }
        }
    }
    if matches!(a.detect, Detect::Poll | Detect::Both) {
        // SAFETY: thread timer，回呼由主執行緒 DispatchMessage 派送。
        if unsafe { SetTimer(None, 0, a.poll_ms, Some(timer_proc)) } == 0 {
            return Err("SetTimer failed".into());
        }
    }

    let pos = win.outer_position()?;
    let size = win.outer_size()?;
    let info = format!(
        "{{\"pid\":{},\"hwnd\":\"{}\",\"rect\":[{},{},{},{}],\"insert\":\"{:?}\",\"detect\":\"{:?}\",\"pollMs\":{}}}",
        std::process::id(),
        hex(hwnd),
        pos.x,
        pos.y,
        size.width,
        size.height,
        a.insert,
        a.detect,
        a.poll_ms
    );
    fs::write(&a.info, &info)?;
    log(&format!("START {info}"));
    evaluate("startup");
    Ok(())
}

fn main() {
    let a = parse_args();
    if let Some(dir) = a.log.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let file = File::create(&a.log).expect("無法建立記錄檔");
    let _ = LOG.set(Mutex::new(file));
    INSERT_TOP.store(a.insert == InsertMode::Top, Ordering::Relaxed);

    tauri::Builder::default()
        .register_uri_scheme_protocol(SCHEME, |_ctx, _req| {
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                .body(PAGE.as_bytes().to_vec())
                .expect("靜態回應不會失敗")
        })
        .setup(move |app| {
            setup(app, &a)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("probe_wind 執行失敗");
}
