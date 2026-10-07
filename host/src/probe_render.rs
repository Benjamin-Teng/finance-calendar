//! dynamic-wallpaper task 1.4 渲染成本探針（`--probe-render`，需 `probe-render` cargo feature；
//! 預設 build 不含這個模組，不會出現在正式產品的 release exe）。
//!
//! ## 要回答的問題（design.md D2）
//!
//! 宿主以隱藏、不出現在工作列的渲染視窗（label `wallpaper-renderer`）載入主題頁，頁面畫完以
//! `invoke('wallpaper_render_done', pngBytes, { headers })` 傳回 PNG。視窗要「畫完即關」還是
//! 「常駐只重畫」，由本探針量測決定：
//!
//! - `--mode open-close`（A）：每次建立隱藏視窗 → 載入 → 收到 PNG → 關閉視窗。
//! - `--mode persistent`（B）：只建立一次，之後每次以新的 `t` 重新導覽（`navigate`），視窗不關。
//!
//! 兩種模式都畫星盤正式頁 `wallpapers/astrolabe.html`，3840×2160，每次的 `t` 依序加 15 分鐘；
//! 資料走頁面的 fixture 模式（`fixture=../fixtures/tw-events.json`，嵌入在 exe 內），不需要
//! `wallpaper` 通道（那是 4.6 的工作）。
//!
//! ## 為什麼放在 bin crate 內、以 feature 隔離
//!
//! 同 `self_test_ipc.rs`「為什麼不是 examples/」：要量的是正式 release 宿主內的 WebView2
//! 行為（嵌入的前端、同一套 Tauri 設定），且要重用 `desktop::apply_widget_ex_style`。
//! `wallpaper_render_done`／`wallpaper_render_failed` 兩個指令**只在本模組**（只在本 feature）
//! 存在；正式版由 task 4.5 實作，可以直接搬本模組的 [`decode_png_body`]／[`png_dimensions`]／
//! [`percent_decode`]，或整個換掉。
//!
//! ## Win32 呼叫
//!
//! AGENTS.md 規定 Win32 集中在 `desktop.rs`；本模組是**探針專用、只在 `probe-render` feature 內
//! 編譯**的例外（task 1.4 brief 明文允許）：行程記憶體取樣（Toolhelp＋`GetProcessMemoryInfo`）、
//! `IsWindowVisible`、`GetWindowLongPtrW`、`GetForegroundWindow`。視窗延伸樣式的「寫入」仍走
//! `desktop::apply_widget_ex_style`。
//!
//! ## 隔離（不碰使用者的真實資料）
//!
//! - `main()` 最開頭就分支到 [`run`]，早於記錄檔初始化、重新啟動註冊、單一執行個體仲裁、
//!   開機自啟、設定載入（這些都會在真實系統留下副作用）。
//! - 不註冊 single-instance plugin、不建立系統匣（把 context 的 `trayIcon` 清掉）、不建立小工具、
//!   不碰桌布。
//! - WebView2 使用者資料夾**必須**明確指定：`--webview-data <dir>`，沒給就讀環境變數
//!   `WEBVIEW2_USER_DATA_FOLDER`，兩者都沒有就拒絕執行（結束碼 2）。Tauri 預設的 `EBWebView`
//!   位置取自已知資料夾 API、不跟 `%LOCALAPPDATA%` 覆寫走（專案 memory
//!   `restart-manager-rmrestart-ignores-caller-env.md`），因此每個視窗都以
//!   `data_directory(...)` 指定。驅動稿 `host/tools/probe-dw-1.4.ps1` 另外讀 msedgewebview2
//!   命令列的 `--user-data-dir` 核對。
//!
//! ## 用法
//!
//! ```text
//! cargo build --release --features probe-render
//! fc-host.exe --probe-render --log <path> --out <dir> --webview-data <dir>
//!             [--mode open-close|persistent] [--runs N] [--tag <名稱>] [--no-extras]
//! ```
//!
//! 產出（`--out`）：`dw-1.4-<tag>-<mode>.csv`（每次一列）、第一次與最後一次的 PNG
//! （`dw-1.4-<tag>-<mode>-runNN.png`）；`--log` 記逐步事件與摘要。結束碼：0 全部成功；
//! 1 有失敗或逾時；2 參數／環境錯誤。

use std::{
    collections::{HashMap, HashSet},
    fmt::Write as _,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicIsize, Ordering},
        mpsc, Arc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};

use serde_json::Value;
use tauri::{
    ipc::{InvokeBody, Request},
    webview::PageLoadEvent,
    AppHandle, RunEvent, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
};
use windows::Win32::{
    Foundation::{CloseHandle, FILETIME, HWND},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
            TH32CS_SNAPPROCESS,
        },
        ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX},
        Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    },
    UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowLongPtrW, GetWindowThreadProcessId, IsWindowVisible,
        GWL_EXSTYLE, WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    },
};

use crate::desktop;

/// 渲染視窗的 label（design.md D2）。
pub const RENDERER_LABEL: &str = "wallpaper-renderer";
/// `--anchor` 的錨點 webview（載入同源的小檔案，與正式宿主的小工具頁同站）。
const ANCHOR_LABEL: &str = "probe-anchor";
const ANCHOR_PATH: &str = "fixtures/tw-events.json";
/// 驅動稿以這個字串確認 exe 是帶 `probe-render` feature 的建置（不帶 feature 的正式版看不到
/// `--probe-render`，會照常啟動宿主）。
pub const BUILD_MARKER: &str = "FC_HOST_PROBE_RENDER_BUILD";
/// 單次渲染逾時（design.md D2：30 秒；spec「渲染失敗時保留舊圖」）。
const RENDER_TIMEOUT: Duration = Duration::from_secs(30);
/// 收到 PNG（或關閉視窗）後等待多久再取第二個記憶體樣本。
const IDLE_AFTER: Duration = Duration::from_secs(2);
/// 等 `Destroyed` 的上限。
const DESTROY_TIMEOUT: Duration = Duration::from_secs(10);
/// 背景取樣間隔：可見性與前景每次取，記憶體每兩次取一次（100 ms）。
const SAMPLE_INTERVAL: Duration = Duration::from_millis(50);
/// 額外量測（匯出方式比較、JSON 陣列本體、header 上限）的總等待上限。
const EXTRAS_TIMEOUT: Duration = Duration::from_secs(180);
const W: u32 = 3840;
const H: u32 = 2160;
/// 第一次的 `t`：2026-10-05（週一）00:30Z＝台北 08:30，之後每次加 15 分鐘。
const T_BASE_DATE: &str = "2026-10-05";
const T_BASE_MINUTES: u32 = 30;
const T_STEP_MINUTES: u32 = 15;
/// 頁面回報 meta 用的 header 名（`wallpaper.mjs` 檔頭「宿主接法」）。
pub const META_HEADER: &str = "x-wallpaper-meta";

// ── 參數 ────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    OpenClose,
    Persistent,
    /// 常駐、不重新導覽：第一次照常載入頁面，之後以 `eval` 在同一份文件內重新呼叫
    /// `runWallpaper`（帶新的 `search`），字型與模組沿用已載入的。用來區分「常駐視窗」與
    /// 「每次重新導覽」各自的記憶體行為。
    PersistentRedraw,
}

impl Mode {
    fn as_str(self) -> &'static str {
        match self {
            Mode::OpenClose => "open-close",
            Mode::Persistent => "persistent",
            Mode::PersistentRedraw => "persistent-redraw",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeArgs {
    pub mode: Mode,
    pub runs: u32,
    pub log: PathBuf,
    pub out: PathBuf,
    pub tag: String,
    pub webview_data: Option<PathBuf>,
    pub extras: bool,
    /// 先建立一個隱藏的「錨點」webview 並保持到結束，模擬正式宿主裡小工具視窗讓 WebView2 browser
    /// 行程一直活著（沒有錨點時，模式 A 關掉唯一的 webview 會連 browser 行程一起結束）。
    pub anchor: bool,
    /// 建立後把 WebView2 controller 也設為不可見（`ICoreWebView2Controller::SetIsVisible(false)`）。
    /// Tauri 的 `visible(false)` 只隱藏 Win32 視窗，controller 仍是可見的（頁面
    /// `document.visibilityState` 為 `visible`）；這個選項量「頁面自認隱藏」時 toBlob 的行為。
    pub controller_hidden: bool,
}

/// 解析命令列（不含 argv[0]）。沒有 `--probe-render` 回傳 `None`（照常啟動宿主）；有但參數不合法
/// 回傳 `Some(Err)`。
pub fn parse_args<I: IntoIterator<Item = String>>(args: I) -> Option<Result<ProbeArgs, String>> {
    let mut present = false;
    let mut parsed = ProbeArgs {
        mode: Mode::OpenClose,
        runs: 10,
        log: PathBuf::from("probe_render.log"),
        out: PathBuf::from("."),
        tag: "run".to_string(),
        webview_data: None,
        extras: true,
        anchor: false,
        controller_hidden: false,
    };
    let mut error: Option<String> = None;
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        let mut value = |name: &str| -> Option<String> {
            let v = it.next();
            if v.is_none() && error.is_none() {
                error = Some(format!("{name} 缺少值"));
            }
            v
        };
        match arg.as_str() {
            "--probe-render" => present = true,
            "--log" => {
                if let Some(v) = value("--log") {
                    parsed.log = PathBuf::from(v);
                }
            }
            "--out" => {
                if let Some(v) = value("--out") {
                    parsed.out = PathBuf::from(v);
                }
            }
            "--webview-data" => {
                if let Some(v) = value("--webview-data") {
                    parsed.webview_data = Some(PathBuf::from(v));
                }
            }
            "--tag" => {
                if let Some(v) = value("--tag") {
                    parsed.tag = v;
                }
            }
            "--mode" => match value("--mode").as_deref() {
                Some("open-close") => parsed.mode = Mode::OpenClose,
                Some("persistent") => parsed.mode = Mode::Persistent,
                Some("persistent-redraw") => parsed.mode = Mode::PersistentRedraw,
                Some(other) => {
                    error.get_or_insert(format!(
                        "--mode 只接受 open-close／persistent／persistent-redraw：{other}"
                    ));
                }
                None => {}
            },
            "--runs" => {
                if let Some(v) = value("--runs") {
                    match v.parse::<u32>() {
                        Ok(n) if (1..=90).contains(&n) => parsed.runs = n,
                        _ => {
                            error.get_or_insert(format!("--runs 需為 1..=90：{v}"));
                        }
                    }
                }
            }
            "--no-extras" => parsed.extras = false,
            "--anchor" => parsed.anchor = true,
            "--controller-hidden" => parsed.controller_hidden = true,
            _ => {}
        }
    }
    if !present {
        return None;
    }
    Some(match error {
        Some(e) => Err(e),
        None => Ok(parsed),
    })
}

/// 第 `index` 次（0 起算）的 `t`：絕對 ISO（含 `Z`），同一天內每次加 15 分鐘（`--runs` 上限 90
/// 保證不跨日）。
pub fn t_param(index: u32) -> String {
    let total = T_BASE_MINUTES + T_STEP_MINUTES * index;
    format!("{T_BASE_DATE}T{:02}:{:02}:00Z", total / 60, total % 60)
}

/// 主題頁的 query（不含 `?`）。`tz`、`fixture` 的斜線在 query 內合法，不另編碼。
pub fn page_query(index: u32) -> String {
    format!(
        "w={W}&h={H}&t={}&tz=Asia/Taipei&fixture=../fixtures/tw-events.json",
        t_param(index)
    )
}

// ── 本體解碼與 PNG／meta 解析（task 4.5 可直接沿用）────────────────────────────────────

/// 請求本體的形式：`Raw`＝自訂協定送來的原始位元組；`JsonArray`＝退回 postMessage（或頁面刻意
/// 以 JSON 送）時，Uint8Array 被 `JSON.stringify` 成數字陣列（Tauri `process-ipc-message-fn.js`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyKind {
    Raw,
    JsonArray,
    Other,
}

impl BodyKind {
    fn as_str(self) -> &'static str {
        match self {
            BodyKind::Raw => "raw",
            BodyKind::JsonArray => "json-array",
            BodyKind::Other => "other",
        }
    }
}

/// 把 `wallpaper_render_done` 的本體還原成 PNG 位元組；原始位元組與 JSON 數字陣列兩種都收。
pub fn decode_png_body(body: &InvokeBody) -> (BodyKind, Result<Vec<u8>, String>) {
    match body {
        InvokeBody::Raw(bytes) => (BodyKind::Raw, Ok(bytes.clone())),
        InvokeBody::Json(Value::Array(items)) => {
            let mut out = Vec::with_capacity(items.len());
            for (i, v) in items.iter().enumerate() {
                match v.as_u64() {
                    Some(n) if n <= 255 => out.push(n as u8),
                    _ => {
                        return (
                            BodyKind::JsonArray,
                            Err(format!("JSON 陣列第 {i} 個元素不是 0..=255 的整數：{v}")),
                        )
                    }
                }
            }
            (BodyKind::JsonArray, Ok(out))
        }
        InvokeBody::Json(other) => (
            BodyKind::Other,
            Err(format!(
                "本體不是位元組也不是數字陣列：{}",
                json_type(other)
            )),
        ),
    }
}

fn json_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// 從 PNG 檔頭（簽章＋IHDR）讀寬高；不是 PNG 回傳 `None`。
pub fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if bytes.len() < 24 || bytes[..8] != SIG || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((w, h))
}

/// `encodeURIComponent` 的反向（`%XX` → 位元組 → UTF-8）。
pub fn percent_decode(s: &str) -> Result<String, String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes
                .get(i + 1..i + 3)
                .and_then(|h| std::str::from_utf8(h).ok())
                .and_then(|h| u8::from_str_radix(h, 16).ok())
                .ok_or_else(|| format!("位置 {i} 的 % 後不是兩位十六進位"))?;
            out.push(hex);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|e| format!("解碼後不是 UTF-8：{e}"))
}

/// FNV-1a 32 位元（頁面與宿主各算一次，比對兩條路徑送來的位元組相同）。
pub fn fnv1a32(bytes: &[u8]) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for &b in bytes {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// 中位數（偶數個取中間兩個平均）；空集合回傳 `None`。
pub fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    let mid = v.len() / 2;
    Some(if v.len().is_multiple_of(2) {
        (v[mid - 1] + v[mid]) / 2.0
    } else {
        v[mid]
    })
}

/// 最小平方法斜率（y 對 0,1,2,…）；少於兩點回傳 `None`。用來看常駐模式記憶體是否逐次上升。
pub fn slope(values: &[f64]) -> Option<f64> {
    let n = values.len();
    if n < 2 {
        return None;
    }
    let nf = n as f64;
    let mean_x = (nf - 1.0) / 2.0;
    let mean_y = values.iter().sum::<f64>() / nf;
    let (mut num, mut den) = (0.0, 0.0);
    for (i, y) in values.iter().enumerate() {
        let dx = i as f64 - mean_x;
        num += dx * (y - mean_y);
        den += dx * dx;
    }
    Some(num / den)
}

// ── 記錄檔與事件 ─────────────────────────────────────────────────────────────────────────

static LOG: OnceLock<Mutex<fs::File>> = OnceLock::new();

fn log(msg: &str) {
    if let Some(m) = LOG.get() {
        if let Ok(mut f) = m.lock() {
            let _ = writeln!(f, "{} {}", desktop::now_string(), msg);
        }
    }
}

enum ProbeEvent {
    PageLoad {
        at: Instant,
        url: String,
    },
    RenderDone {
        at: Instant,
        label: String,
        kind: BodyKind,
        png: Result<Vec<u8>, String>,
        decode_ms: f64,
        meta_header: Option<String>,
    },
    RenderFailed {
        at: Instant,
        label: String,
        error: String,
        meta: Value,
    },
    Report {
        json: String,
    },
    Destroyed {
        label: String,
    },
}

static EVENTS: OnceLock<Mutex<mpsc::Sender<ProbeEvent>>> = OnceLock::new();

fn send(ev: ProbeEvent) {
    if let Some(tx) = EVENTS.get() {
        if let Ok(tx) = tx.lock() {
            let _ = tx.send(ev);
        }
    }
}

// ── 指令（只在本 feature 內存在；正式版見 task 4.5）──────────────────────────────────────

/// 頁面畫完：本體＝PNG（原始位元組或 JSON 數字陣列），meta 在 `x-wallpaper-meta` header。
#[tauri::command]
pub fn wallpaper_render_done(webview: tauri::Webview, request: Request<'_>) -> Result<(), String> {
    let at = Instant::now();
    let started = Instant::now();
    let (kind, png) = decode_png_body(request.body());
    let decode_ms = ms(started.elapsed());
    let meta_header = request
        .headers()
        .get(META_HEADER)
        .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned());
    let result = match &png {
        Ok(_) => Ok(()),
        Err(e) => Err(e.clone()),
    };
    send(ProbeEvent::RenderDone {
        at,
        label: webview.label().to_string(),
        kind,
        png,
        decode_ms,
        meta_header,
    });
    result
}

/// 頁面失敗（字型、fixture、draw 拋錯、匯出失敗）。
#[tauri::command]
pub fn wallpaper_render_failed(webview: tauri::Webview, error: String, meta: Value) {
    send(ProbeEvent::RenderFailed {
        at: Instant::now(),
        label: webview.label().to_string(),
        error,
        meta,
    });
}

/// 額外量測的結果回報（JSON 字串）。
#[tauri::command]
pub fn probe_report(json: String) {
    send(ProbeEvent::Report { json });
}

/// header 上限測試：回傳收到的 `x-wallpaper-meta` 長度（位元組；沒有＝0）。
#[tauri::command]
pub fn probe_header_echo(request: Request<'_>) -> usize {
    request
        .headers()
        .get(META_HEADER)
        .map_or(0, |v| v.as_bytes().len())
}

// ── Win32：記憶體、可見性、前景（探針專用）──────────────────────────────────────────────

#[derive(Debug, Default, Clone, Copy)]
struct MemSample {
    host_ws: u64,
    host_priv: u64,
    wv_ws: u64,
    wv_priv: u64,
    wv_procs: u32,
}

impl MemSample {
    fn total_ws(&self) -> u64 {
        self.host_ws + self.wv_ws
    }
    fn total_priv(&self) -> u64 {
        self.host_priv + self.wv_priv
    }
}

struct ProcEntry {
    pid: u32,
    ppid: u32,
    exe: String,
}

fn list_processes() -> Vec<ProcEntry> {
    let mut out = Vec::new();
    // SAFETY: TH32CS_SNAPPROCESS 快照不需要額外權限；控制代碼在下方關閉。
    let Ok(snap) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else {
        return out;
    };
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    // SAFETY: entry.dwSize 已設；snap 是有效快照控制代碼。
    let mut ok = unsafe { Process32FirstW(snap, &mut entry) }.is_ok();
    while ok {
        let end = entry
            .szExeFile
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(entry.szExeFile.len());
        out.push(ProcEntry {
            pid: entry.th32ProcessID,
            ppid: entry.th32ParentProcessID,
            exe: String::from_utf16_lossy(&entry.szExeFile[..end]),
        });
        // SAFETY: 同上。
        ok = unsafe { Process32NextW(snap, &mut entry) }.is_ok();
    }
    // SAFETY: snap 由 CreateToolhelp32Snapshot 取得、只關一次。
    unsafe {
        let _ = CloseHandle(snap);
    }
    out
}

/// 行程建立時間（FILETIME 的 100ns 計數）與記憶體；取不到回傳 `None`。
fn query_process(pid: u32) -> Option<(u64, u64, u64)> {
    // SAFETY: 只要求查詢權限；控制代碼在下方關閉。
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: h 有效；四個輸出參數都是本函式的區域變數。
    let times = unsafe { GetProcessTimes(h, &mut created, &mut exited, &mut kernel, &mut user) };
    let mut mem = PROCESS_MEMORY_COUNTERS_EX {
        cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        ..Default::default()
    };
    // SAFETY: PROCESS_MEMORY_COUNTERS_EX 以 PROCESS_MEMORY_COUNTERS 開頭，cb 告知實際大小。
    let mem_ok = unsafe {
        GetProcessMemoryInfo(
            h,
            std::ptr::addr_of_mut!(mem).cast(),
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        )
    };
    // SAFETY: h 由 OpenProcess 取得、只關一次。
    unsafe {
        let _ = CloseHandle(h);
    }
    if times.is_err() || mem_ok.is_err() {
        return None;
    }
    let created = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
    Some((created, mem.WorkingSetSize as u64, mem.PrivateUsage as u64))
}

/// 本行程與其全部子孫（WebView2 的 msedgewebview2.exe）的工作集與私有位元組。子孫以父 PID 串起，
/// 並要求建立時間不早於父行程（排除父 PID 被重用的舊行程）。`names` 收集子孫的執行檔名。
fn sample_memory(root: u32, names: Option<&mut HashSet<String>>) -> MemSample {
    let procs = list_processes();
    let mut sample = MemSample::default();
    let Some((root_created, ws, private)) = query_process(root) else {
        return sample;
    };
    sample.host_ws = ws;
    sample.host_priv = private;
    let mut by_parent: HashMap<u32, Vec<&ProcEntry>> = HashMap::new();
    for p in &procs {
        if p.pid != p.ppid {
            by_parent.entry(p.ppid).or_default().push(p);
        }
    }
    let mut created_of: HashMap<u32, u64> = HashMap::from([(root, root_created)]);
    let mut queue = vec![root];
    let mut names = names;
    while let Some(cur) = queue.pop() {
        let Some(children) = by_parent.get(&cur) else {
            continue;
        };
        let parent_created = created_of.get(&cur).copied().unwrap_or(0);
        for child in children {
            if created_of.contains_key(&child.pid) {
                continue;
            }
            let Some((created, ws, private)) = query_process(child.pid) else {
                continue;
            };
            if created < parent_created {
                continue;
            }
            created_of.insert(child.pid, created);
            sample.wv_ws += ws;
            sample.wv_priv += private;
            sample.wv_procs += 1;
            if let Some(n) = names.as_deref_mut() {
                n.insert(child.exe.clone());
            }
            queue.push(child.pid);
        }
    }
    sample
}

fn hwnd_from(raw: isize) -> HWND {
    HWND(raw as *mut core::ffi::c_void)
}

fn is_visible(raw: isize) -> bool {
    // SAFETY: IsWindowVisible 對無效或已銷毀的 HWND 只回傳 FALSE。
    unsafe { IsWindowVisible(hwnd_from(raw)) }.as_bool()
}

fn ex_style(raw: isize) -> u32 {
    // SAFETY: 讀取延伸樣式；HWND 無效時回傳 0。
    unsafe { GetWindowLongPtrW(hwnd_from(raw), GWL_EXSTYLE) as u32 }
}

/// 前景視窗與其所屬 PID。
fn foreground() -> (isize, u32) {
    // SAFETY: 無前置條件。
    let fg = unsafe { GetForegroundWindow() };
    let mut pid = 0u32;
    if !fg.0.is_null() {
        // SAFETY: fg 是系統回傳的視窗控制代碼；pid 是區域變數。
        unsafe { GetWindowThreadProcessId(fg, Some(&mut pid)) };
    }
    (fg.0 as isize, pid)
}

fn describe_ex_style(style: u32) -> String {
    format!(
        "0x{style:08X}(TOOLWINDOW={} APPWINDOW={} NOACTIVATE={})",
        style & WS_EX_TOOLWINDOW.0 != 0,
        style & WS_EX_APPWINDOW.0 != 0,
        style & WS_EX_NOACTIVATE.0 != 0
    )
}

// ── 背景取樣器 ───────────────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Clone, Copy)]
struct RunStats {
    vis_samples: u32,
    vis_true: u32,
    fg_samples: u32,
    fg_own: u32,
    fg_changed: u32,
    mem_samples: u32,
    peak_ws: u64,
    peak_priv: u64,
}

struct Sampler {
    hwnd: AtomicIsize,
    stop: AtomicBool,
    fg_baseline: AtomicIsize,
    stats: Mutex<RunStats>,
}

impl Sampler {
    fn reset(&self) -> RunStats {
        std::mem::take(&mut *self.stats.lock().expect("stats mutex poisoned"))
    }
    fn snapshot(&self) -> RunStats {
        *self.stats.lock().expect("stats mutex poisoned")
    }
}

fn spawn_sampler(sampler: Arc<Sampler>) {
    let own_pid = std::process::id();
    thread::spawn(move || {
        let mut tick: u64 = 0;
        while !sampler.stop.load(Ordering::SeqCst) {
            let raw = sampler.hwnd.load(Ordering::SeqCst);
            let visible = (raw != 0).then(|| is_visible(raw));
            let (fg, fg_pid) = foreground();
            let mem = tick.is_multiple_of(2).then(|| sample_memory(own_pid, None));
            {
                let mut s = sampler.stats.lock().expect("stats mutex poisoned");
                if let Some(v) = visible {
                    s.vis_samples += 1;
                    s.vis_true += u32::from(v);
                }
                s.fg_samples += 1;
                s.fg_own += u32::from(fg_pid == own_pid);
                s.fg_changed += u32::from(fg != sampler.fg_baseline.load(Ordering::SeqCst));
                if let Some(m) = mem {
                    s.mem_samples += 1;
                    s.peak_ws = s.peak_ws.max(m.total_ws());
                    s.peak_priv = s.peak_priv.max(m.total_priv());
                }
            }
            tick += 1;
            thread::sleep(SAMPLE_INTERVAL);
        }
    });
}

// ── 每次的紀錄 ───────────────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Clone)]
struct RunRecord {
    run: u32,
    t: String,
    ok: bool,
    error: String,
    create_ms: f64,
    loaded_at_ms: Option<f64>,
    png_at_ms: Option<f64>,
    close_ms: Option<f64>,
    png_bytes: usize,
    png_w: u32,
    png_h: u32,
    png_fnv: u32,
    body_kind: String,
    decode_ms: f64,
    meta_header_len: usize,
    meta_wh: String,
    meta_warnings: usize,
    mem_png: MemSample,
    mem_idle: MemSample,
    stats: RunStats,
    visible_at_png: bool,
    ex_style_built: u32,
    ex_style_applied: u32,
}

impl RunRecord {
    fn load_segment_ms(&self) -> Option<f64> {
        self.loaded_at_ms.map(|l| l - self.create_ms)
    }
    fn png_segment_ms(&self) -> Option<f64> {
        match (self.png_at_ms, self.loaded_at_ms) {
            (Some(p), Some(l)) => Some(p - l),
            _ => None,
        }
    }
}

const CSV_HEADER: &str =
    "mode,run,t,ok,error,create_ms,load_ms,png_ms,total_ms,loaded_at_ms,close_ms,\
png_bytes,png_w,png_h,png_fnv,body_kind,decode_ms,meta_header_len,meta_wh,meta_warnings,\
host_ws_png,host_priv_png,wv_ws_png,wv_priv_png,wv_procs_png,\
host_ws_idle,host_priv_idle,wv_ws_idle,wv_priv_idle,wv_procs_idle,\
peak_total_ws,peak_total_priv,mem_samples,vis_samples,vis_true,visible_at_png,\
fg_samples,fg_own,fg_changed,ex_style_built,ex_style_applied";

fn opt(v: Option<f64>) -> String {
    v.map_or_else(String::new, |x| format!("{x:.1}"))
}

fn csv_row(mode: Mode, r: &RunRecord) -> String {
    let mut s = String::new();
    let _ = write!(
        s,
        "{},{},{},{},\"{}\",{:.1},{},{},{},{},{},",
        mode.as_str(),
        r.run,
        r.t,
        r.ok,
        r.error.replace('"', "'"),
        r.create_ms,
        opt(r.load_segment_ms()),
        opt(r.png_segment_ms()),
        opt(r.png_at_ms),
        opt(r.loaded_at_ms),
        opt(r.close_ms),
    );
    let _ = write!(
        s,
        "{},{},{},{:08x},{},{:.1},{},{},{},",
        r.png_bytes,
        r.png_w,
        r.png_h,
        r.png_fnv,
        r.body_kind,
        r.decode_ms,
        r.meta_header_len,
        r.meta_wh,
        r.meta_warnings
    );
    for m in [r.mem_png, r.mem_idle] {
        let _ = write!(
            s,
            "{},{},{},{},{},",
            m.host_ws, m.host_priv, m.wv_ws, m.wv_priv, m.wv_procs
        );
    }
    let _ = write!(
        s,
        "{},{},{},{},{},{},{},{},{},0x{:08X},0x{:08X}",
        r.stats.peak_ws,
        r.stats.peak_priv,
        r.stats.mem_samples,
        r.stats.vis_samples,
        r.stats.vis_true,
        r.visible_at_png,
        r.stats.fg_samples,
        r.stats.fg_own,
        r.stats.fg_changed,
        r.ex_style_built,
        r.ex_style_applied
    );
    s
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn mb(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

// ── 驅動 ─────────────────────────────────────────────────────────────────────────────────

fn build_renderer(
    app: &AppHandle,
    url: WebviewUrl,
    data_dir: &Path,
) -> tauri::Result<WebviewWindow> {
    WebviewWindowBuilder::new(app, RENDERER_LABEL, url)
        .title("fc-host wallpaper-renderer (probe)")
        .visible(false)
        .focused(false)
        .focusable(false)
        .skip_taskbar(true)
        .decorations(false)
        .inner_size(640.0, 360.0)
        .data_directory(data_dir.to_path_buf())
        .on_page_load(|_win, payload| {
            if payload.event() == PageLoadEvent::Finished {
                send(ProbeEvent::PageLoad {
                    at: Instant::now(),
                    url: payload.url().to_string(),
                });
            }
        })
        .build()
}

/// `--controller-hidden`：把 controller 設為不可見。`with_webview` 的回呼在主執行緒非同步執行，
/// 結果寫進記錄檔。
fn hide_controller(window: &WebviewWindow, run: u32) {
    let result = window.with_webview(move |pw| {
        // SAFETY: `with_webview` 的回呼由 Tauri 在擁有該 webview 的 UI 執行緒派送（同
        // `desktop::install_process_failed_handler` 的既有查證）；只呼叫 controller 的屬性設定。
        let r = unsafe { pw.controller().SetIsVisible(false) };
        log(&format!(
            "RUN {run} controller.SetIsVisible(false) -> {r:?}"
        ));
    });
    if let Err(e) = result {
        log(&format!("RUN {run} with_webview 失敗：{e}"));
    }
}

/// 把 meta header 解成 JSON，回傳 `(w×h, warnings 數)`。
fn parse_meta(header: &str) -> (String, usize) {
    let Ok(text) = percent_decode(header) else {
        return ("meta-decode-error".into(), 0);
    };
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return ("meta-json-error".into(), 0);
    };
    let wh = format!(
        "{}x{}",
        v.get("w").and_then(Value::as_u64).unwrap_or(0),
        v.get("h").and_then(Value::as_u64).unwrap_or(0)
    );
    let warnings = v
        .get("warnings")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    (wh, warnings)
}

struct Driver {
    app: AppHandle,
    args: ProbeArgs,
    data_dir: PathBuf,
    rx: mpsc::Receiver<ProbeEvent>,
    sampler: Arc<Sampler>,
    own_pid: u32,
}

impl Driver {
    fn drain(&self) {
        while let Ok(ev) = self.rx.try_recv() {
            log(&format!("STALE-EVENT {}", describe_event(&ev)));
        }
    }

    /// 等待第一個符合 `pred` 的 `Destroyed`。
    fn wait_destroyed(&self, deadline: Instant) -> bool {
        loop {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            match self.rx.recv_timeout(deadline - now) {
                Ok(ProbeEvent::Destroyed { label }) if label == RENDERER_LABEL => return true,
                Ok(ev) => log(&format!("EVENT(while-closing) {}", describe_event(&ev))),
                Err(_) => return false,
            }
        }
    }

    /// 單次渲染。`window` 為 `None`＝建立新視窗（A 每次、B 第一次）；否則以新的 `t` 重新導覽。
    fn render_once(
        &self,
        index: u32,
        window: &mut Option<WebviewWindow>,
    ) -> (RunRecord, Option<Vec<u8>>) {
        let mut rec = RunRecord {
            run: index + 1,
            t: t_param(index),
            ..Default::default()
        };
        let query = page_query(index);
        self.drain();
        self.sampler.reset();
        let t0 = Instant::now();

        match window {
            None => {
                let url = WebviewUrl::App(format!("wallpapers/astrolabe.html?{query}").into());
                match build_renderer(&self.app, url, &self.data_dir) {
                    Ok(w) => {
                        rec.create_ms = ms(t0.elapsed());
                        match w.hwnd() {
                            Ok(h) => {
                                let raw = h.0 as isize;
                                self.sampler.hwnd.store(raw, Ordering::SeqCst);
                                rec.ex_style_built = ex_style(raw);
                                if let Err(e) = desktop::apply_widget_ex_style(h) {
                                    log(&format!(
                                        "RUN {} apply_widget_ex_style 失敗：{e}",
                                        rec.run
                                    ));
                                }
                                rec.ex_style_applied = ex_style(raw);
                                log(&format!(
                                    "RUN {} WINDOW created hwnd=0x{raw:X} visible={} exStyle(built)={} exStyle(applied)={}",
                                    rec.run,
                                    is_visible(raw),
                                    describe_ex_style(rec.ex_style_built),
                                    describe_ex_style(rec.ex_style_applied)
                                ));
                            }
                            Err(e) => log(&format!("RUN {} hwnd() 失敗：{e}", rec.run)),
                        }
                        if self.args.controller_hidden {
                            hide_controller(&w, rec.run);
                        }
                        *window = Some(w);
                    }
                    Err(e) => {
                        rec.error = format!("建立視窗失敗：{e}");
                        log(&format!("RUN {} FAIL {}", rec.run, rec.error));
                        return (rec, None);
                    }
                }
            }
            Some(w) if self.args.mode == Mode::PersistentRedraw => {
                if let Err(e) = w.eval(redraw_script(&query)) {
                    rec.error = format!("eval 重畫失敗：{e}");
                    return (rec, None);
                }
                rec.create_ms = 0.0;
                // 沒有頁面載入；「載入」段記為 eval 送出所花的時間。
                rec.loaded_at_ms = Some(ms(t0.elapsed()));
                if let Ok(h) = w.hwnd() {
                    let raw = h.0 as isize;
                    rec.ex_style_built = ex_style(raw);
                    rec.ex_style_applied = rec.ex_style_built;
                }
            }
            Some(w) => {
                let mut url: Url = match w.url() {
                    Ok(u) => u,
                    Err(e) => {
                        rec.error = format!("取目前網址失敗：{e}");
                        return (rec, None);
                    }
                };
                url.set_query(Some(&query));
                if let Err(e) = w.navigate(url) {
                    rec.error = format!("navigate 失敗：{e}");
                    return (rec, None);
                }
                rec.create_ms = 0.0;
                if let Ok(h) = w.hwnd() {
                    let raw = h.0 as isize;
                    rec.ex_style_built = ex_style(raw);
                    rec.ex_style_applied = rec.ex_style_built;
                }
            }
        }

        let deadline = t0 + RENDER_TIMEOUT;
        let mut png_out: Option<Vec<u8>> = None;
        loop {
            let now = Instant::now();
            if now >= deadline {
                rec.error = "逾時（30 秒內沒收到 PNG）".into();
                break;
            }
            match self.rx.recv_timeout(deadline - now) {
                Ok(ProbeEvent::PageLoad { at, url }) => {
                    if rec.loaded_at_ms.is_none() && at >= t0 {
                        rec.loaded_at_ms = Some(ms(at - t0));
                    }
                    log(&format!(
                        "RUN {} PAGE-LOAD-FINISHED +{:.1}ms url={url}",
                        rec.run,
                        ms(at.saturating_duration_since(t0))
                    ));
                }
                Ok(ProbeEvent::RenderDone {
                    at,
                    label,
                    kind,
                    png,
                    decode_ms,
                    meta_header,
                }) if label == RENDERER_LABEL => {
                    rec.png_at_ms = Some(ms(at.saturating_duration_since(t0)));
                    rec.body_kind = kind.as_str().into();
                    rec.decode_ms = decode_ms;
                    if let Some(h) = &meta_header {
                        rec.meta_header_len = h.len();
                        let (wh, warnings) = parse_meta(h);
                        rec.meta_wh = wh;
                        rec.meta_warnings = warnings;
                        log(&format!(
                            "RUN {} META len={} decoded={}",
                            rec.run,
                            h.len(),
                            percent_decode(h).unwrap_or_else(|e| e)
                        ));
                    }
                    match png {
                        Ok(bytes) => {
                            rec.png_bytes = bytes.len();
                            rec.png_fnv = fnv1a32(&bytes);
                            if let Some((w, h)) = png_dimensions(&bytes) {
                                rec.png_w = w;
                                rec.png_h = h;
                            }
                            rec.ok = rec.png_w == W && rec.png_h == H;
                            if !rec.ok {
                                rec.error = format!("PNG 尺寸不符：{}x{}", rec.png_w, rec.png_h);
                            }
                            png_out = Some(bytes);
                        }
                        Err(e) => rec.error = format!("本體解碼失敗：{e}"),
                    }
                    break;
                }
                Ok(ProbeEvent::RenderFailed {
                    at,
                    label,
                    error,
                    meta,
                }) if label == RENDERER_LABEL => {
                    rec.png_at_ms = Some(ms(at.saturating_duration_since(t0)));
                    rec.error = format!("頁面回報失敗：{error}（meta={meta}）");
                    break;
                }
                Ok(ev) => log(&format!("RUN {} EVENT {}", rec.run, describe_event(&ev))),
                Err(_) => {
                    rec.error = "逾時（30 秒內沒收到 PNG）".into();
                    break;
                }
            }
        }

        rec.mem_png = sample_memory(self.own_pid, None);
        let raw = self.sampler.hwnd.load(Ordering::SeqCst);
        rec.visible_at_png = raw != 0 && is_visible(raw);
        rec.stats = self.sampler.snapshot();

        if self.args.mode == Mode::OpenClose {
            if let Some(w) = window.take() {
                let tc = Instant::now();
                match w.destroy() {
                    Ok(()) => {
                        if self.wait_destroyed(tc + DESTROY_TIMEOUT) {
                            rec.close_ms = Some(ms(tc.elapsed()));
                        } else {
                            log(&format!(
                                "RUN {} 等不到 Destroyed（{DESTROY_TIMEOUT:?}）",
                                rec.run
                            ));
                        }
                    }
                    Err(e) => log(&format!("RUN {} destroy 失敗：{e}", rec.run)),
                }
                self.sampler.hwnd.store(0, Ordering::SeqCst);
            }
        }

        thread::sleep(IDLE_AFTER);
        rec.mem_idle = sample_memory(self.own_pid, None);
        log(&format!(
            "RUN {} {} t={} create={:.1}ms load={} png={} total={} close={} bytes={} {}x{} body={} \
             decode={:.1}ms metaLen={} mem@png(host ws/priv={:.1}/{:.1}MB wv ws/priv={:.1}/{:.1}MB procs={}) \
             mem@idle(host ws/priv={:.1}/{:.1}MB wv ws/priv={:.1}/{:.1}MB procs={}) peak(ws/priv={:.1}/{:.1}MB) \
             vis={}/{} fgOwn={} fgChanged={}/{} {}",
            rec.run,
            if rec.ok { "OK" } else { "FAIL" },
            rec.t,
            rec.create_ms,
            opt(rec.load_segment_ms()),
            opt(rec.png_segment_ms()),
            opt(rec.png_at_ms),
            opt(rec.close_ms),
            rec.png_bytes,
            rec.png_w,
            rec.png_h,
            rec.body_kind,
            rec.decode_ms,
            rec.meta_header_len,
            mb(rec.mem_png.host_ws),
            mb(rec.mem_png.host_priv),
            mb(rec.mem_png.wv_ws),
            mb(rec.mem_png.wv_priv),
            rec.mem_png.wv_procs,
            mb(rec.mem_idle.host_ws),
            mb(rec.mem_idle.host_priv),
            mb(rec.mem_idle.wv_ws),
            mb(rec.mem_idle.wv_priv),
            rec.mem_idle.wv_procs,
            mb(rec.stats.peak_ws),
            mb(rec.stats.peak_priv),
            rec.stats.vis_true,
            rec.stats.vis_samples,
            rec.stats.fg_own,
            rec.stats.fg_changed,
            rec.stats.fg_samples,
            rec.error
        ));
        (rec, png_out)
    }

    /// 額外量測（常駐模式最後、視窗仍隱藏存活時）：toBlob／toDataURL／OffscreenCanvas 比較、
    /// 同一份位元組以原始本體與 JSON 數字陣列各送一次、`x-wallpaper-meta` 長度上限。
    fn extras(&self, window: &WebviewWindow) {
        self.drain();
        log("EXTRAS start");
        if let Err(e) = window.eval(EXTRAS_SCRIPT) {
            log(&format!("EXTRAS eval 失敗：{e}"));
            return;
        }
        let deadline = Instant::now() + EXTRAS_TIMEOUT;
        loop {
            let now = Instant::now();
            if now >= deadline {
                log("EXTRAS 逾時（沒收到 probe_report）");
                return;
            }
            match self.rx.recv_timeout(deadline - now) {
                Ok(ProbeEvent::Report { json }) => {
                    log(&format!("EXTRAS REPORT {json}"));
                    return;
                }
                Ok(ProbeEvent::RenderDone {
                    kind,
                    png,
                    decode_ms,
                    meta_header,
                    ..
                }) => {
                    let meta = meta_header
                        .as_deref()
                        .map(|h| percent_decode(h).unwrap_or_else(|e| e))
                        .unwrap_or_default();
                    match png {
                        Ok(bytes) => log(&format!(
                            "EXTRAS RENDER-DONE body={} bytes={} fnv={:08x} dims={:?} decode={decode_ms:.1}ms meta={meta}",
                            kind.as_str(),
                            bytes.len(),
                            fnv1a32(&bytes),
                            png_dimensions(&bytes)
                        )),
                        Err(e) => log(&format!(
                            "EXTRAS RENDER-DONE body={} 解碼失敗：{e} meta={meta}",
                            kind.as_str()
                        )),
                    }
                }
                Ok(ev) => log(&format!("EXTRAS EVENT {}", describe_event(&ev))),
                Err(_) => {
                    log("EXTRAS 逾時（沒收到 probe_report）");
                    return;
                }
            }
        }
    }

    fn run(self) -> i32 {
        let mut names = HashSet::new();
        let (fg0, fg0_pid) = foreground();
        self.sampler.fg_baseline.store(fg0, Ordering::SeqCst);
        let base = sample_memory(self.own_pid, Some(&mut names));
        log(&format!(
            "BASELINE fg=0x{fg0:X}(pid {fg0_pid}) host ws/priv={:.1}/{:.1}MB wv procs={} ws/priv={:.1}/{:.1}MB",
            mb(base.host_ws),
            mb(base.host_priv),
            base.wv_procs,
            mb(base.wv_ws),
            mb(base.wv_priv)
        ));

        let anchor = if self.args.anchor {
            let built = WebviewWindowBuilder::new(
                &self.app,
                ANCHOR_LABEL,
                WebviewUrl::App(ANCHOR_PATH.into()),
            )
            .title("fc-host probe anchor")
            .visible(false)
            .focused(false)
            .focusable(false)
            .skip_taskbar(true)
            .decorations(false)
            .inner_size(320.0, 180.0)
            .data_directory(self.data_dir.clone())
            .build();
            match built {
                Ok(w) => {
                    thread::sleep(IDLE_AFTER);
                    let a = sample_memory(self.own_pid, Some(&mut names));
                    log(&format!(
                        "ANCHOR created ({ANCHOR_PATH}) host ws/priv={:.1}/{:.1}MB wv procs={} ws/priv={:.1}/{:.1}MB",
                        mb(a.host_ws),
                        mb(a.host_priv),
                        a.wv_procs,
                        mb(a.wv_ws),
                        mb(a.wv_priv)
                    ));
                    Some(w)
                }
                Err(e) => {
                    log(&format!("ABORT 錨點 webview 建立失敗：{e}"));
                    self.sampler.stop.store(true, Ordering::SeqCst);
                    return 2;
                }
            }
        } else {
            None
        };

        let csv_path = self.args.out.join(format!(
            "dw-1.4-{}-{}.csv",
            self.args.tag,
            self.args.mode.as_str()
        ));
        let mut csv = String::from(CSV_HEADER);
        csv.push('\n');

        let mut records = Vec::new();
        let mut window: Option<WebviewWindow> = None;
        for i in 0..self.args.runs {
            let (rec, png) = self.render_once(i, &mut window);
            if i == 0 || i + 1 == self.args.runs {
                if let Some(bytes) = png {
                    let path = self.args.out.join(format!(
                        "dw-1.4-{}-{}-run{:02}.png",
                        self.args.tag,
                        self.args.mode.as_str(),
                        i + 1
                    ));
                    match fs::write(&path, bytes) {
                        Ok(()) => log(&format!("PNG saved {}", path.display())),
                        Err(e) => log(&format!("PNG 寫檔失敗 {}：{e}", path.display())),
                    }
                }
            }
            csv.push_str(&csv_row(self.args.mode, &rec));
            csv.push('\n');
            if let Err(e) = fs::write(&csv_path, &csv) {
                log(&format!("CSV 寫檔失敗：{e}"));
            }
            records.push(rec);
        }
        // 取一次完整的子孫行程名清單（看 WebView2 有哪些行程）。
        let _ = sample_memory(self.own_pid, Some(&mut names));

        if let Some(w) = window.take() {
            if self.args.extras {
                self.extras(&w);
            }
            let raw = self.sampler.hwnd.load(Ordering::SeqCst);
            log(&format!(
                "PERSISTENT end visible={} exStyle={}",
                raw != 0 && is_visible(raw),
                describe_ex_style(ex_style(raw))
            ));
            let tc = Instant::now();
            if w.destroy().is_ok() && self.wait_destroyed(tc + DESTROY_TIMEOUT) {
                log(&format!("PERSISTENT closed in {:.1}ms", ms(tc.elapsed())));
            }
            self.sampler.hwnd.store(0, Ordering::SeqCst);
            thread::sleep(IDLE_AFTER);
            let after = sample_memory(self.own_pid, None);
            log(&format!(
                "PERSISTENT after-close host ws/priv={:.1}/{:.1}MB wv procs={} ws/priv={:.1}/{:.1}MB",
                mb(after.host_ws),
                mb(after.host_priv),
                after.wv_procs,
                mb(after.wv_ws),
                mb(after.wv_priv)
            ));
        }

        if let Some(a) = anchor {
            let _ = a.destroy();
            log("ANCHOR destroyed");
        }

        let (fg1, fg1_pid) = foreground();
        log(&format!(
            "FOREGROUND before=0x{fg0:X}(pid {fg0_pid}) after=0x{fg1:X}(pid {fg1_pid}) changed={}",
            fg0 != fg1
        ));
        let mut names: Vec<_> = names.into_iter().collect();
        names.sort();
        log(&format!("CHILD-EXE {names:?}"));
        self.sampler.stop.store(true, Ordering::SeqCst);
        log_summary(self.args.mode, &records);
        if records.iter().all(|r| r.ok) {
            0
        } else {
            1
        }
    }
}

fn describe_event(ev: &ProbeEvent) -> String {
    match ev {
        ProbeEvent::PageLoad { url, .. } => format!("PageLoad url={url}"),
        ProbeEvent::RenderDone {
            label, kind, png, ..
        } => format!(
            "RenderDone label={label} body={} bytes={:?}",
            kind.as_str(),
            png.as_ref().map(Vec::len)
        ),
        ProbeEvent::RenderFailed { label, error, .. } => {
            format!("RenderFailed label={label} error={error}")
        }
        ProbeEvent::Report { json } => format!("Report {json}"),
        ProbeEvent::Destroyed { label } => format!("Destroyed label={label}"),
    }
}

fn log_summary(mode: Mode, records: &[RunRecord]) {
    let ok: Vec<&RunRecord> = records.iter().filter(|r| r.ok).collect();
    let stat = |name: &str, values: Vec<f64>| {
        let max = values.iter().copied().fold(f64::NAN, f64::max);
        log(&format!(
            "SUMMARY {} {name}: n={} median={} max={}",
            mode.as_str(),
            values.len(),
            opt(median(&values)),
            if max.is_nan() {
                String::new()
            } else {
                format!("{max:.1}")
            }
        ));
    };
    stat("total_ms", ok.iter().filter_map(|r| r.png_at_ms).collect());
    stat("create_ms", ok.iter().map(|r| r.create_ms).collect());
    stat(
        "load_ms",
        ok.iter().filter_map(|r| r.load_segment_ms()).collect(),
    );
    stat(
        "png_ms",
        ok.iter().filter_map(|r| r.png_segment_ms()).collect(),
    );
    stat("close_ms", ok.iter().filter_map(|r| r.close_ms).collect());
    stat(
        "total_ms(run>=2)",
        ok.iter()
            .filter(|r| r.run >= 2)
            .filter_map(|r| r.png_at_ms)
            .collect(),
    );
    let peak_priv = records.iter().map(|r| r.stats.peak_priv).max().unwrap_or(0);
    let peak_ws = records.iter().map(|r| r.stats.peak_ws).max().unwrap_or(0);
    log(&format!(
        "SUMMARY {} peak(total) ws={:.1}MB priv={:.1}MB",
        mode.as_str(),
        mb(peak_ws),
        mb(peak_priv)
    ));
    let idle_priv: Vec<f64> = records
        .iter()
        .map(|r| mb(r.mem_idle.total_priv()))
        .collect();
    let idle_ws: Vec<f64> = records.iter().map(|r| mb(r.mem_idle.total_ws())).collect();
    let png_priv: Vec<f64> = records.iter().map(|r| mb(r.mem_png.total_priv())).collect();
    let rises = idle_priv.windows(2).filter(|w| w[1] > w[0]).count();
    log(&format!(
        "SUMMARY {} idle priv(MB)={:?} slope={}MB/run rises={}/{}",
        mode.as_str(),
        idle_priv
            .iter()
            .map(|v| (v * 10.0).round() / 10.0)
            .collect::<Vec<_>>(),
        opt(slope(&idle_priv)),
        rises,
        idle_priv.len().saturating_sub(1)
    ));
    log(&format!(
        "SUMMARY {} idle ws(MB)={:?} slope={}MB/run",
        mode.as_str(),
        idle_ws
            .iter()
            .map(|v| (v * 10.0).round() / 10.0)
            .collect::<Vec<_>>(),
        opt(slope(&idle_ws))
    ));
    log(&format!(
        "SUMMARY {} png priv(MB)={:?}",
        mode.as_str(),
        png_priv
            .iter()
            .map(|v| (v * 10.0).round() / 10.0)
            .collect::<Vec<_>>()
    ));
    let release: Vec<f64> = records
        .iter()
        .map(|r| mb(r.mem_png.total_priv()) - mb(r.mem_idle.total_priv()))
        .collect();
    log(&format!(
        "SUMMARY {} release priv(png-idle, MB) median={} max={}",
        mode.as_str(),
        opt(median(&release)),
        opt(release.iter().copied().reduce(f64::max))
    ));
    let vis_true: u32 = records.iter().map(|r| r.stats.vis_true).sum();
    let vis_samples: u32 = records.iter().map(|r| r.stats.vis_samples).sum();
    let fg_own: u32 = records.iter().map(|r| r.stats.fg_own).sum();
    let fg_changed: u32 = records.iter().map(|r| r.stats.fg_changed).sum();
    let fg_samples: u32 = records.iter().map(|r| r.stats.fg_samples).sum();
    let max_meta = records.iter().map(|r| r.meta_header_len).max().unwrap_or(0);
    let kinds: HashSet<&str> = records.iter().map(|r| r.body_kind.as_str()).collect();
    log(&format!(
        "SUMMARY {} ok={}/{} visible={vis_true}/{vis_samples} fgOwn={fg_own} fgChanged={fg_changed}/{fg_samples} maxMetaHeader={max_meta} bodyKinds={kinds:?}",
        mode.as_str(),
        ok.len(),
        records.len()
    ));
}

/// `persistent-redraw` 每次送進頁面的重畫腳本：在同一份文件內以新的 query 再跑一次
/// `runWallpaper`（模組已在快取，同一個實例），與 `astrolabe.html` 內嵌腳本相同的 draw。
fn redraw_script(query: &str) -> String {
    format!(
        r#"(async () => {{
  const {{ runWallpaper }} = await import('/wallpapers/lib/wallpaper.mjs');
  const {{ ASTROLABE_FONTS, drawAstrolabe, summarize }} = await import('/wallpapers/lib/astrolabe-draw.mjs');
  await runWallpaper({{ canvas: '#c', fonts: ASTROLABE_FONTS, search: '?{query}',
    async draw(env) {{ const model = await drawAstrolabe(env); window.__wallpaperSummary = summarize(model); }} }});
}})().catch((e) => window.__TAURI__.core.invoke('wallpaper_render_failed', {{ error: 'redraw: ' + String((e && e.message) || e), meta: {{}} }}));"#
    )
}

/// 額外量測的頁面端腳本（在隱藏的渲染視窗內 eval）。逾時保護用 `setTimeout`：隱藏頁面的計時器
/// 可能被節流，但只是保險，宿主端另有 [`EXTRAS_TIMEOUT`]。
const EXTRAS_SCRIPT: &str = r#"(async () => {
  const inv = window.__TAURI__.core.invoke;
  const out = { kind: 'extras', visibilityState: document.visibilityState, hidden: document.hidden,
    hasFocus: document.hasFocus(), devicePixelRatio: window.devicePixelRatio,
    innerSize: [window.innerWidth, window.innerHeight], export: {}, ipc: {}, header: [] };
  const fnv = (u8) => { let h = 0x811c9dc5; for (let i = 0; i < u8.length; i++) { h ^= u8[i]; h = Math.imul(h, 0x01000193) >>> 0; } return h >>> 0; };
  const hex = (n) => n.toString(16).padStart(8, '0');
  const guard = (p, ms, what) => Promise.race([p, new Promise((_, rej) => setTimeout(() => rej(new Error(what + ' timeout')), ms))]);
  try {
    const cv = document.querySelector('#c');
    out.canvas = [cv.width, cv.height];
    let t = performance.now();
    const blob = await guard(new Promise((res, rej) => cv.toBlob((b) => (b ? res(b) : rej(new Error('toBlob null'))), 'image/png')), 30000, 'toBlob');
    const blobBytes = new Uint8Array(await blob.arrayBuffer());
    out.export.toBlob = { ms: +(performance.now() - t).toFixed(1), bytes: blobBytes.length, fnv: hex(fnv(blobBytes)) };
    t = performance.now();
    const url = cv.toDataURL('image/png');
    const t2 = performance.now();
    const bin = atob(url.slice(url.indexOf(',') + 1));
    const du = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) du[i] = bin.charCodeAt(i);
    out.export.toDataURL = { ms: +(t2 - t).toFixed(1), decodeMs: +(performance.now() - t2).toFixed(1), bytes: du.length, fnv: hex(fnv(du)) };
    if (typeof OffscreenCanvas !== 'undefined') {
      t = performance.now();
      const oc = new OffscreenCanvas(cv.width, cv.height);
      oc.getContext('2d', { alpha: false }).drawImage(cv, 0, 0);
      const ob = await guard(oc.convertToBlob({ type: 'image/png' }), 30000, 'convertToBlob');
      const ou = new Uint8Array(await ob.arrayBuffer());
      out.export.offscreen = { ms: +(performance.now() - t).toFixed(1), bytes: ou.length, fnv: hex(fnv(ou)) };
    } else {
      out.export.offscreen = { unsupported: true };
    }
    t = performance.now();
    await inv('wallpaper_render_done', blobBytes, { headers: { 'x-wallpaper-meta': encodeURIComponent(JSON.stringify({ probe: 'raw-repeat' })) } });
    out.ipc.raw = { ms: +(performance.now() - t).toFixed(1), bytes: blobBytes.length, fnv: hex(fnv(blobBytes)) };
    t = performance.now();
    await inv('wallpaper_render_done', { __TAURI_TO_IPC_KEY__: () => Array.from(blobBytes) },
      { headers: { 'x-wallpaper-meta': encodeURIComponent(JSON.stringify({ probe: 'json-array' })) } });
    out.ipc.jsonArray = { ms: +(performance.now() - t).toFixed(1), bytes: blobBytes.length, fnv: hex(fnv(blobBytes)) };
    for (const n of [1024, 4096, 8192, 16384, 32768, 65536, 131072, 262144, 1048576, 4194304]) {
      t = performance.now();
      try {
        const got = await guard(inv('probe_header_echo', {}, { headers: { 'x-wallpaper-meta': 'a'.repeat(n) } }), 15000, 'header');
        out.header.push({ n, ok: got === n, got, ms: +(performance.now() - t).toFixed(1) });
      } catch (e) {
        out.header.push({ n, ok: false, error: String((e && e.message) || e), ms: +(performance.now() - t).toFixed(1) });
      }
    }
  } catch (e) {
    out.error = String((e && e.message) || e);
  }
  await inv('probe_report', { json: JSON.stringify(out) });
})();"#;

// ── 進入點 ───────────────────────────────────────────────────────────────────────────────

/// `main.rs` 解析出 `--probe-render` 後呼叫；接手整個行程，不會回傳（`App::run` 以
/// `std::process::exit` 結束）。參數錯誤時回傳結束碼 2 給呼叫端。
pub fn run(parsed: Result<ProbeArgs, String>) -> i32 {
    let args = match parsed {
        Ok(a) => a,
        Err(e) => {
            eprintln!("--probe-render 參數錯誤：{e}");
            return 2;
        }
    };
    if let Some(dir) = args.log.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let Ok(file) = fs::File::create(&args.log) else {
        eprintln!("無法建立記錄檔 {}", args.log.display());
        return 2;
    };
    let _ = LOG.set(Mutex::new(file));
    std::panic::set_hook(Box::new(|info| log(&format!("PANIC {info}"))));

    let env_udf = std::env::var_os("WEBVIEW2_USER_DATA_FOLDER").map(PathBuf::from);
    log(&format!(
        "START {BUILD_MARKER} pid={} mode={} runs={} tag={} anchor={} controllerHidden={} out={} webviewData={:?} \
         env(WEBVIEW2_USER_DATA_FOLDER={:?} APPDATA={:?} LOCALAPPDATA={:?})",
        std::process::id(),
        args.mode.as_str(),
        args.runs,
        args.tag,
        args.anchor,
        args.controller_hidden,
        args.out.display(),
        args.webview_data,
        env_udf,
        std::env::var_os("APPDATA"),
        std::env::var_os("LOCALAPPDATA"),
    ));
    let Some(data_dir) = args.webview_data.clone().or(env_udf) else {
        log("ABORT 沒有 --webview-data 也沒有 WEBVIEW2_USER_DATA_FOLDER：拒絕使用預設的 EBWebView 位置");
        return 2;
    };
    if fs::create_dir_all(&data_dir).is_err() || fs::create_dir_all(&args.out).is_err() {
        log("ABORT 無法建立 webview 資料夾或輸出資料夾");
        return 2;
    }

    let (tx, rx) = mpsc::channel();
    let _ = EVENTS.set(Mutex::new(tx));
    let sampler = Arc::new(Sampler {
        hwnd: AtomicIsize::new(0),
        stop: AtomicBool::new(false),
        fg_baseline: AtomicIsize::new(0),
        stats: Mutex::new(RunStats::default()),
    });

    let mut context = tauri::generate_context!();
    // 不建立系統匣（tauri.conf.json 的 app.trayIcon 會在 build 時自動建立圖示）。
    context.config_mut().app.tray_icon = None;

    let mut driver_parts = Some((rx, args, data_dir, sampler));
    let app = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            wallpaper_render_done,
            wallpaper_render_failed,
            probe_report,
            probe_header_echo,
        ])
        .setup(move |app| {
            let Some((rx, args, data_dir, sampler)) = driver_parts.take() else {
                return Ok(());
            };
            spawn_sampler(sampler.clone());
            let handle = app.handle().clone();
            thread::spawn(move || {
                let driver = Driver {
                    app: handle.clone(),
                    args,
                    data_dir,
                    rx,
                    sampler,
                    own_pid: std::process::id(),
                };
                let code = driver.run();
                log(&format!("EXIT code={code}"));
                handle.exit(code);
            });
            Ok(())
        })
        .build(context);
    let app = match app {
        Ok(a) => a,
        Err(e) => {
            log(&format!("ABORT tauri build 失敗：{e}"));
            return 2;
        }
    };
    app.run(|_app, event| match &event {
        // 模式 A 關掉唯一的視窗時 Tauri 會要求結束；探針自己決定何時結束（`exit(code)` 帶 Some）。
        RunEvent::ExitRequested {
            code: None, api, ..
        } => api.prevent_exit(),
        RunEvent::WindowEvent {
            label,
            event: WindowEvent::Destroyed,
            ..
        } => send(ProbeEvent::Destroyed {
            label: label.clone(),
        }),
        _ => {}
    });
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Option<Result<ProbeArgs, String>> {
        parse_args(v.iter().map(|s| (*s).to_string()))
    }

    #[test]
    fn parse_absent_returns_none() {
        assert!(args(&["--log", "x"]).is_none());
    }

    #[test]
    fn parse_full() {
        let a = args(&[
            "--probe-render",
            "--mode",
            "persistent",
            "--runs",
            "3",
            "--log",
            "l.log",
            "--out",
            "o",
            "--tag",
            "t1",
            "--webview-data",
            "wd",
            "--no-extras",
            "--anchor",
            "--controller-hidden",
        ])
        .expect("present")
        .expect("valid");
        assert_eq!(a.mode, Mode::Persistent);
        assert_eq!(a.runs, 3);
        assert_eq!(a.log, PathBuf::from("l.log"));
        assert_eq!(a.out, PathBuf::from("o"));
        assert_eq!(a.tag, "t1");
        assert_eq!(a.webview_data, Some(PathBuf::from("wd")));
        assert!(!a.extras);
        assert!(a.anchor);
        assert!(a.controller_hidden);
    }

    #[test]
    fn parse_rejects_bad_values() {
        let r = args(&["--probe-render", "--mode", "persistent-redraw"]).expect("present");
        assert_eq!(r.expect("valid").mode, Mode::PersistentRedraw);
        assert!(args(&["--probe-render", "--mode", "x"])
            .expect("present")
            .is_err());
        assert!(args(&["--probe-render", "--runs", "0"])
            .expect("present")
            .is_err());
        assert!(args(&["--probe-render", "--runs", "91"])
            .expect("present")
            .is_err());
        assert!(args(&["--probe-render", "--log"])
            .expect("present")
            .is_err());
    }

    #[test]
    fn t_param_steps_15_minutes() {
        assert_eq!(t_param(0), "2026-10-05T00:30:00Z");
        assert_eq!(t_param(1), "2026-10-05T00:45:00Z");
        assert_eq!(t_param(2), "2026-10-05T01:00:00Z");
        assert_eq!(t_param(9), "2026-10-05T02:45:00Z");
        assert_eq!(t_param(89), "2026-10-05T22:45:00Z");
    }

    #[test]
    fn redraw_script_embeds_query() {
        let s = redraw_script(&page_query(1));
        assert!(s.contains("search: '?w=3840&h=2160&t=2026-10-05T00:45:00Z"));
        assert!(s.contains("import('/wallpapers/lib/wallpaper.mjs')"));
        assert!(!s.contains("{{"));
    }

    #[test]
    fn page_query_has_size_and_fixture() {
        let q = page_query(0);
        assert!(q.starts_with("w=3840&h=2160&t=2026-10-05T00:30:00Z&tz=Asia/Taipei"));
        assert!(q.ends_with("fixture=../fixtures/tw-events.json"));
    }

    fn png_header(w: u32, h: u32) -> Vec<u8> {
        let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
        v.extend_from_slice(b"IHDR");
        v.extend_from_slice(&w.to_be_bytes());
        v.extend_from_slice(&h.to_be_bytes());
        v.extend_from_slice(&[8, 2, 0, 0, 0]);
        v
    }

    #[test]
    fn png_dimensions_reads_ihdr() {
        assert_eq!(png_dimensions(&png_header(3840, 2160)), Some((3840, 2160)));
        assert_eq!(png_dimensions(&png_header(1, 1)), Some((1, 1)));
        assert_eq!(png_dimensions(b"not a png at all, definitely"), None);
        assert_eq!(png_dimensions(&png_header(3840, 2160)[..20]), None);
        let mut bad = png_header(3840, 2160);
        bad[12] = b'X';
        assert_eq!(png_dimensions(&bad), None);
    }

    #[test]
    fn decode_raw_and_json_array_bodies() {
        let bytes = png_header(3840, 2160);
        let (k, r) = decode_png_body(&InvokeBody::Raw(bytes.clone()));
        assert_eq!(k, BodyKind::Raw);
        assert_eq!(r.expect("raw"), bytes);

        let arr = Value::Array(bytes.iter().map(|b| Value::from(*b)).collect());
        let (k, r) = decode_png_body(&InvokeBody::Json(arr));
        assert_eq!(k, BodyKind::JsonArray);
        assert_eq!(r.expect("json array"), bytes);
    }

    #[test]
    fn decode_rejects_non_bytes() {
        let (k, r) = decode_png_body(&InvokeBody::Json(serde_json::json!([1, 256])));
        assert_eq!(k, BodyKind::JsonArray);
        assert!(r.is_err());
        let (k, r) = decode_png_body(&InvokeBody::Json(serde_json::json!([1, -1])));
        assert_eq!(k, BodyKind::JsonArray);
        assert!(r.is_err());
        let (k, r) = decode_png_body(&InvokeBody::Json(serde_json::json!({ "png": [1] })));
        assert_eq!(k, BodyKind::Other);
        assert!(r.is_err());
    }

    #[test]
    fn percent_decode_roundtrip() {
        // encodeURIComponent('{"w":3840,"tz":"Asia/Taipei","warnings":["中"]}')
        let enc = "%7B%22w%22%3A3840%2C%22tz%22%3A%22Asia%2FTaipei%22%2C%22warnings%22%3A%5B%22%E4%B8%AD%22%5D%7D";
        assert_eq!(
            percent_decode(enc).expect("decode"),
            r#"{"w":3840,"tz":"Asia/Taipei","warnings":["中"]}"#
        );
        assert!(percent_decode("%E4%B8").is_err());
        assert!(percent_decode("%zz").is_err());
        assert!(percent_decode("abc%").is_err());
    }

    #[test]
    fn parse_meta_reads_size_and_warnings() {
        let enc =
            "%7B%22w%22%3A3840%2C%22h%22%3A2160%2C%22warnings%22%3A%5B%22a%22%2C%22b%22%5D%7D";
        assert_eq!(parse_meta(enc), ("3840x2160".to_string(), 2));
        assert_eq!(parse_meta("%zz").0, "meta-decode-error");
    }

    #[test]
    fn fnv_known_vectors() {
        assert_eq!(fnv1a32(b""), 0x811c_9dc5);
        assert_eq!(fnv1a32(b"a"), 0xe40c_292c);
    }

    #[test]
    fn median_and_slope() {
        assert_eq!(median(&[]), None);
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&[4.0, 1.0, 3.0, 2.0]), Some(2.5));
        assert_eq!(slope(&[1.0]), None);
        let s = slope(&[1.0, 2.0, 3.0, 4.0]).expect("slope");
        assert!((s - 1.0).abs() < 1e-9);
        let flat = slope(&[5.0, 5.0, 5.0]).expect("slope");
        assert!(flat.abs() < 1e-9);
    }

    #[test]
    fn csv_row_matches_header_columns() {
        let rec = RunRecord {
            error: "a \"quoted\" error".into(),
            ..Default::default()
        };
        let row = csv_row(Mode::OpenClose, &rec);
        // error 欄位不含逗號時，欄數＝逗號數＋1。
        assert_eq!(row.matches(',').count(), CSV_HEADER.matches(',').count());
        assert!(!row.contains("\"quoted\""));
    }
}
