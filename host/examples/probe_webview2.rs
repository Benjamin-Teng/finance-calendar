//! Task 1.4 WebView2 故障探針。
//!
//! 開 N 個（預設 3）同源小工具視窗（同一 user data folder＝同一 browser process），每個視窗以
//! `with_webview` → `PlatformWebview::controller()` → `ICoreWebView2` 訂閱 `ProcessFailed`
//! 與 `NavigationCompleted`，並定期以 `ICoreWebView2Environment13::GetProcessExtendedInfos`
//! 取得「行程 → 關聯 frame」對照，從 frame 的 `Source`（URL 路徑＝視窗標籤）推回每個視窗用
//! 哪個 renderer。
//!
//! 頁面每秒 `fetch('/hb?...')` 回報心跳，宿主端記錄「心跳中斷／恢復」，用來量測故障後內容
//! 恢復所需時間（規格要求 10 秒內）且不需截圖。
//!
//! 故障處理（探針版，供 task 5.6 定案用）：
//! - `RENDER_PROCESS_EXITED`／`RENDER_PROCESS_UNRESPONSIVE`：對收到事件的 webview 呼叫
//!   `Reload()`。
//! - `BROWSER_PROCESS_EXITED`：記錄舊視窗與 Rust 行程狀態、嘗試對舊 webview `Reload()`
//!   （預期失敗），`--on-browser-exit rebuild`（預設）時建立新一代視窗後銷毀舊視窗；
//!   `none` 時只記錄不處理。
//! - 其他種類：只記錄。
//!
//! 控制檔（`--ctl`）：驅動腳本寫入一行指令，探針每 250 ms 讀取並清空。
//! `hang <label>`＝令該視窗主執行緒 JS 無窮迴圈；`recreate <label>`＝銷毀該視窗並以新標籤
//! `<label>r` 重建；`dump`＝立即記錄行程對照；`quit`＝結束。
//!
//! 執行（純 cargo）：
//! `cargo run --release --example probe_webview2 -- --log <記錄檔> --info <json> --ctl <控制檔>
//! --udf <user data folder>`；驅動腳本見 `host/tools/probe-1.4.ps1`。
#![windows_subsystem = "windows"]

use std::{
    collections::HashMap,
    env,
    fs::{self, File},
    io::Write,
    path::PathBuf,
    sync::{Mutex, OnceLock},
    thread,
    time::{Duration, Instant},
};

use tauri::{
    http::{header, Response, StatusCode},
    AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder,
};
use webview2_com::{
    take_pwstr, GetProcessExtendedInfosCompletedHandler,
    Microsoft::Web::WebView2::Win32::{
        ICoreWebView2, ICoreWebView2Environment13, ICoreWebView2FrameInfoCollection,
        ICoreWebView2ProcessExtendedInfoCollection, ICoreWebView2ProcessFailedEventArgs,
        ICoreWebView2ProcessFailedEventArgs2, COREWEBVIEW2_PROCESS_FAILED_KIND,
        COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED,
        COREWEBVIEW2_PROCESS_FAILED_KIND_FRAME_RENDER_PROCESS_EXITED,
        COREWEBVIEW2_PROCESS_FAILED_KIND_GPU_PROCESS_EXITED,
        COREWEBVIEW2_PROCESS_FAILED_KIND_PPAPI_BROKER_PROCESS_EXITED,
        COREWEBVIEW2_PROCESS_FAILED_KIND_PPAPI_PLUGIN_PROCESS_EXITED,
        COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED,
        COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE,
        COREWEBVIEW2_PROCESS_FAILED_KIND_SANDBOX_HELPER_PROCESS_EXITED,
        COREWEBVIEW2_PROCESS_FAILED_KIND_UNKNOWN_PROCESS_EXITED,
        COREWEBVIEW2_PROCESS_FAILED_KIND_UTILITY_PROCESS_EXITED,
        COREWEBVIEW2_PROCESS_FAILED_REASON, COREWEBVIEW2_PROCESS_FAILED_REASON_CRASHED,
        COREWEBVIEW2_PROCESS_FAILED_REASON_LAUNCH_FAILED,
        COREWEBVIEW2_PROCESS_FAILED_REASON_OUT_OF_MEMORY,
        COREWEBVIEW2_PROCESS_FAILED_REASON_PROFILE_DELETED,
        COREWEBVIEW2_PROCESS_FAILED_REASON_TERMINATED,
        COREWEBVIEW2_PROCESS_FAILED_REASON_UNEXPECTED,
        COREWEBVIEW2_PROCESS_FAILED_REASON_UNRESPONSIVE, COREWEBVIEW2_PROCESS_KIND,
        COREWEBVIEW2_PROCESS_KIND_BROWSER, COREWEBVIEW2_PROCESS_KIND_GPU,
        COREWEBVIEW2_PROCESS_KIND_RENDERER, COREWEBVIEW2_PROCESS_KIND_UTILITY,
    },
    NavigationCompletedEventHandler, ProcessFailedEventHandler,
};
use windows::{
    core::{Interface, BOOL, PWSTR},
    Win32::{
        Foundation::HWND, System::SystemInformation::GetLocalTime,
        UI::WindowsAndMessaging::IsWindow,
    },
};

const SCHEME: &str = "probe";
/// 心跳超過此毫秒數未到＝視為中斷。
const HB_STOP_MS: u128 = 3000;
const PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>probe</title>
<style>html,body{margin:0;height:100%;background:#264653;color:#e9c46a;font:bold 22px sans-serif;
display:flex;align-items:center;justify-content:center;text-align:center}</style></head>
<body><div><span id="l"></span><br><span id="n" style="font-size:14px"></span></div>
<script>
const w = location.pathname.slice(1);
const id = Math.random().toString(36).slice(2, 8);
document.getElementById('l').textContent = w;
let n = 0;
function hb() {
  n++;
  document.getElementById('n').textContent = id + ' #' + n;
  fetch('/hb?w=' + w + '&id=' + id + '&n=' + n, { cache: 'no-store' }).catch(() => {});
}
hb();
setInterval(hb, 1000);
</script></body></html>"#;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum OnBrowserExit {
    Rebuild,
    None,
}

struct Args {
    log: PathBuf,
    info: PathBuf,
    ctl: PathBuf,
    udf: PathBuf,
    count: usize,
    on_browser_exit: OnBrowserExit,
    rebuild_delay_ms: u64,
}

/// 旗標：`--log`、`--info`、`--ctl`、`--udf`（路徑）、`--count <n>`、
/// `--on-browser-exit rebuild|none`、`--rebuild-delay-ms <n>`。
fn parse_args() -> Args {
    let target = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
    let mut a = Args {
        log: target.join("probe_webview2.log"),
        info: target.join("probe_webview2.json"),
        ctl: target.join("probe_webview2.ctl"),
        udf: target.join("probe_webview2_udf"),
        count: 3,
        on_browser_exit: OnBrowserExit::Rebuild,
        rebuild_delay_ms: 1000,
    };
    let mut it = env::args().skip(1);
    while let Some(k) = it.next() {
        let v = it.next().unwrap_or_default();
        match k.as_str() {
            "--log" => a.log = PathBuf::from(v),
            "--info" => a.info = PathBuf::from(v),
            "--ctl" => a.ctl = PathBuf::from(v),
            "--udf" => a.udf = PathBuf::from(v),
            "--count" => a.count = v.parse().unwrap_or(a.count).max(1),
            "--on-browser-exit" => {
                a.on_browser_exit = if v == "none" {
                    OnBrowserExit::None
                } else {
                    OnBrowserExit::Rebuild
                }
            }
            "--rebuild-delay-ms" => a.rebuild_delay_ms = v.parse().unwrap_or(a.rebuild_delay_ms),
            _ => {}
        }
    }
    a
}

// ---- 全域狀態 ----

struct Hb {
    last: Instant,
    id: String,
    stopped: bool,
}

#[derive(Default)]
struct State {
    /// 目前這一代的視窗標籤。
    gen: u32,
    labels: Vec<String>,
    /// 已經為哪一代排過重建（同一代的多個 BROWSER_PROCESS_EXITED 只處理一次）。
    rebuild_scheduled_for: Option<u32>,
    hb: HashMap<String, Hb>,
    /// 最近一次 ProcessFailed 的時間與描述，用來算心跳恢復耗時。
    last_failure: Option<(Instant, String)>,
    mapping: String,
}

static LOG: OnceLock<Mutex<File>> = OnceLock::new();
static ARGS: OnceLock<Args> = OnceLock::new();
static APP: OnceLock<AppHandle> = OnceLock::new();
static STATE: OnceLock<Mutex<State>> = OnceLock::new();

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE
        .get_or_init(|| Mutex::new(State::default()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn args() -> &'static Args {
    ARGS.get().expect("ARGS 在 main 開頭設定")
}

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

fn json_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn failed_kind_name(k: COREWEBVIEW2_PROCESS_FAILED_KIND) -> String {
    let n = match k {
        COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED => "BROWSER_PROCESS_EXITED",
        COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED => "RENDER_PROCESS_EXITED",
        COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE => {
            "RENDER_PROCESS_UNRESPONSIVE"
        }
        COREWEBVIEW2_PROCESS_FAILED_KIND_FRAME_RENDER_PROCESS_EXITED => {
            "FRAME_RENDER_PROCESS_EXITED"
        }
        COREWEBVIEW2_PROCESS_FAILED_KIND_UTILITY_PROCESS_EXITED => "UTILITY_PROCESS_EXITED",
        COREWEBVIEW2_PROCESS_FAILED_KIND_SANDBOX_HELPER_PROCESS_EXITED => {
            "SANDBOX_HELPER_PROCESS_EXITED"
        }
        COREWEBVIEW2_PROCESS_FAILED_KIND_GPU_PROCESS_EXITED => "GPU_PROCESS_EXITED",
        COREWEBVIEW2_PROCESS_FAILED_KIND_PPAPI_PLUGIN_PROCESS_EXITED => {
            "PPAPI_PLUGIN_PROCESS_EXITED"
        }
        COREWEBVIEW2_PROCESS_FAILED_KIND_PPAPI_BROKER_PROCESS_EXITED => {
            "PPAPI_BROKER_PROCESS_EXITED"
        }
        COREWEBVIEW2_PROCESS_FAILED_KIND_UNKNOWN_PROCESS_EXITED => "UNKNOWN_PROCESS_EXITED",
        _ => return format!("KIND({})", k.0),
    };
    n.to_string()
}

fn reason_name(r: COREWEBVIEW2_PROCESS_FAILED_REASON) -> String {
    let n = match r {
        COREWEBVIEW2_PROCESS_FAILED_REASON_UNEXPECTED => "UNEXPECTED",
        COREWEBVIEW2_PROCESS_FAILED_REASON_UNRESPONSIVE => "UNRESPONSIVE",
        COREWEBVIEW2_PROCESS_FAILED_REASON_TERMINATED => "TERMINATED",
        COREWEBVIEW2_PROCESS_FAILED_REASON_CRASHED => "CRASHED",
        COREWEBVIEW2_PROCESS_FAILED_REASON_LAUNCH_FAILED => "LAUNCH_FAILED",
        COREWEBVIEW2_PROCESS_FAILED_REASON_OUT_OF_MEMORY => "OUT_OF_MEMORY",
        COREWEBVIEW2_PROCESS_FAILED_REASON_PROFILE_DELETED => "PROFILE_DELETED",
        _ => return format!("REASON({})", r.0),
    };
    n.to_string()
}

fn process_kind_name(k: COREWEBVIEW2_PROCESS_KIND) -> String {
    let n = match k {
        COREWEBVIEW2_PROCESS_KIND_BROWSER => "browser",
        COREWEBVIEW2_PROCESS_KIND_RENDERER => "renderer",
        COREWEBVIEW2_PROCESS_KIND_GPU => "gpu",
        COREWEBVIEW2_PROCESS_KIND_UTILITY => "utility",
        _ => return format!("kind{}", k.0),
    };
    n.to_string()
}

/// 由 frame URL（`http://probe.localhost/<label>`）取出視窗標籤；非本探針 URL 原樣回傳。
fn label_of(url: &str) -> String {
    match url.find(".localhost/") {
        Some(i) => {
            let rest = &url[i + ".localhost/".len()..];
            rest.split(['?', '#']).next().unwrap_or(rest).to_string()
        }
        None => url.to_string(),
    }
}

fn frame_labels(c: &ICoreWebView2FrameInfoCollection) -> Vec<String> {
    let mut out = Vec::new();
    // SAFETY: COM 介面呼叫；所有輸出指標皆為區域變數，PWSTR 由 take_pwstr 以
    // CoTaskMemFree 釋放。
    unsafe {
        let Ok(it) = c.GetIterator() else {
            return out;
        };
        let mut has = BOOL::default();
        if it.HasCurrent(&mut has).is_err() {
            return out;
        }
        while has.as_bool() {
            if let Ok(fi) = it.GetCurrent() {
                let mut p = PWSTR::null();
                if fi.Source(&mut p).is_ok() {
                    out.push(label_of(&take_pwstr(p)));
                }
            }
            if it.MoveNext(&mut has).is_err() {
                break;
            }
        }
    }
    out
}

/// 處理某個 webview 收到的 ProcessFailed。在主（UI）執行緒上被呼叫。
fn on_process_failed(
    label: &str,
    sender: Option<ICoreWebView2>,
    args_: Option<ICoreWebView2ProcessFailedEventArgs>,
) {
    let Some(a) = args_ else {
        log(&format!("PF label={label} args=None"));
        return;
    };
    let mut kind = COREWEBVIEW2_PROCESS_FAILED_KIND::default();
    // SAFETY: kind 為有效輸出指標。
    let _ = unsafe { a.ProcessFailedKind(&mut kind) };
    let kind_name = failed_kind_name(kind);
    let mut detail = String::new();
    if let Ok(a2) = a.cast::<ICoreWebView2ProcessFailedEventArgs2>() {
        let mut reason = COREWEBVIEW2_PROCESS_FAILED_REASON::default();
        let mut exit = 0i32;
        let mut desc = PWSTR::null();
        // SAFETY: 輸出指標皆為區域變數；desc 由 take_pwstr 釋放。
        unsafe {
            let _ = a2.Reason(&mut reason);
            let _ = a2.ExitCode(&mut exit);
            let desc_s = if a2.ProcessDescription(&mut desc).is_ok() {
                take_pwstr(desc)
            } else {
                String::new()
            };
            let frames = a2
                .FrameInfosForFailedProcess()
                .map(|c| frame_labels(&c))
                .unwrap_or_default();
            detail = format!(
                " reason={} exitCode={exit} desc={desc_s:?} frames={frames:?}",
                reason_name(reason)
            );
        }
    }
    log(&format!("PF label={label} kind={kind_name}{detail}"));
    state().last_failure = Some((Instant::now(), format!("{kind_name}@{label}")));

    match kind {
        COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED
        | COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE => {
            if let Some(core) = sender {
                // SAFETY: 在 UI 執行緒上對事件來源 webview 呼叫 Reload。
                let r = unsafe { core.Reload() };
                log(&format!("ACTION label={label} reload result={r:?}"));
            }
        }
        COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED => on_browser_exited(label),
        _ => log(&format!("ACTION label={label} none (僅記錄)")),
    }
}

fn on_browser_exited(label: &str) {
    let gen = {
        let mut s = state();
        if s.rebuild_scheduled_for == Some(s.gen) {
            log(&format!(
                "BROWSER-EXIT label={label} gen={} 已排程處理，略過",
                s.gen
            ));
            return;
        }
        s.rebuild_scheduled_for = Some(s.gen);
        s.gen
    };
    let mode = args().on_browser_exit;
    let delay = args().rebuild_delay_ms;
    log(&format!(
        "BROWSER-EXIT label={label} gen={gen} mode={mode:?} delayMs={delay}"
    ));
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(delay));
        report_old_windows(gen, "after-browser-exit");
        if mode == OnBrowserExit::Rebuild {
            rebuild(gen);
        } else {
            // 不處理：隔一段時間再看一次舊視窗是否仍存活。
            thread::sleep(Duration::from_secs(10));
            report_old_windows(gen, "10s-later-no-action");
        }
    });
}

/// 記錄指定一代視窗的狀態：HWND 是否仍存在、是否可見，並嘗試對舊 webview `Reload()`。
fn report_old_windows(gen: u32, tag: &str) {
    let Some(app) = APP.get() else { return };
    let labels = state().labels.clone();
    log(&format!(
        "STATE[{tag}] gen={gen} rustPid={} alive=1 labels={labels:?}",
        std::process::id()
    ));
    for l in labels {
        let Some(w) = app.get_webview_window(&l) else {
            log(&format!("STATE[{tag}] {l} get_webview_window=None"));
            continue;
        };
        let hwnd = w.hwnd().map(|h| h.0 as isize).unwrap_or(0);
        // SAFETY: IsWindow 對任意值都安全。
        let is_win = unsafe { IsWindow(Some(HWND(hwnd as *mut _))) }.as_bool();
        let vis = w.is_visible();
        log(&format!(
            "STATE[{tag}] {l} hwnd=0x{hwnd:X} IsWindow={is_win} is_visible={vis:?}"
        ));
        let l2 = l.clone();
        let r = w.with_webview(move |pw| {
            // SAFETY: 在 UI 執行緒上；controller 由 Tauri 持有、此處只 clone 參考。
            unsafe {
                let ctrl = pw.controller();
                match ctrl.CoreWebView2() {
                    Ok(core) => {
                        let r = core.Reload();
                        log(&format!("STATE[old-reload] {l2} Reload={r:?}"));
                    }
                    Err(e) => log(&format!("STATE[old-reload] {l2} CoreWebView2 err={e:?}")),
                }
            }
        });
        if let Err(e) = r {
            log(&format!("STATE[{tag}] {l} with_webview err={e}"));
        }
    }
}

/// 建立新一代視窗，再銷毀舊一代。
fn rebuild(old_gen: u32) {
    let Some(app) = APP.get() else { return };
    let old_labels = state().labels.clone();
    let new_gen = old_gen + 1;
    log(&format!("REBUILD start gen {old_gen} -> {new_gen}"));
    match create_windows(app, new_gen) {
        Ok(()) => log(&format!("REBUILD created gen={new_gen}")),
        Err(e) => {
            log(&format!("REBUILD create failed gen={new_gen} err={e}"));
            return;
        }
    }
    for l in old_labels {
        if let Some(w) = app.get_webview_window(&l) {
            let r = w.destroy();
            log(&format!("REBUILD destroy {l} result={r:?}"));
        }
    }
}

fn create_windows(app: &AppHandle, gen: u32) -> Result<(), Box<dyn std::error::Error>> {
    let n = args().count;
    let mut labels = Vec::with_capacity(n);
    for i in 0..n {
        let label = format!("g{gen}w{i}");
        create_one(app, &label, (i as f64, f64::from(gen)))?;
        labels.push(label);
    }
    let mut s = state();
    s.gen = gen;
    s.labels = labels;
    Ok(())
}

/// 建立單一小工具視窗；`slot`＝（欄、列）決定擺放位置。
fn create_one(
    app: &AppHandle,
    label: &str,
    slot: (f64, f64),
) -> Result<(), Box<dyn std::error::Error>> {
    let url = tauri::Url::parse(&format!("http://{SCHEME}.localhost/{label}"))?;
    let win = WebviewWindowBuilder::new(app, label, WebviewUrl::External(url))
        .title(label)
        .data_directory(args().udf.clone())
        .decorations(false)
        .skip_taskbar(true)
        .focusable(false)
        .focused(false)
        .resizable(false)
        .shadow(false)
        .inner_size(240.0, 140.0)
        .position(40.0 + 260.0 * slot.0, 40.0 + 170.0 * slot.1)
        .build()?;
    attach(&win, label)?;
    Ok(())
}

/// 單一視窗重建（銷毀後以新標籤 `<label>r` 重建於同一位置）；用於 JS 卡死後的復原實驗。
fn recreate(label: &str) {
    let Some(app) = APP.get() else { return };
    let (idx, gen) = {
        let s = state();
        (s.labels.iter().position(|l| l == label), s.gen)
    };
    let Some(idx) = idx else {
        log(&format!("RECREATE {label} 不在目前這一代"));
        return;
    };
    let new_label = format!("{label}r");
    state().last_failure = Some((Instant::now(), format!("RECREATE@{label}")));
    match app.get_webview_window(label) {
        Some(w) => log(&format!(
            "RECREATE destroy {label} result={:?}",
            w.destroy()
        )),
        None => log(&format!("RECREATE {label} 找不到視窗")),
    }
    match create_one(app, &new_label, (idx as f64, f64::from(gen))) {
        Ok(()) => {
            state().labels[idx] = new_label.clone();
            log(&format!("RECREATE created {new_label}"));
        }
        Err(e) => log(&format!("RECREATE create {new_label} failed err={e}")),
    }
}

/// 在 webview 上訂閱 ProcessFailed／NavigationCompleted 並記錄 BrowserProcessId。
fn attach(win: &tauri::WebviewWindow, label: &str) -> tauri::Result<()> {
    let label = label.to_string();
    win.with_webview(move |pw| {
        // SAFETY: with_webview 的回呼在 UI 執行緒執行；事件處理常式同樣由 WebView2 在
        // UI 執行緒派送；token 為區域輸出變數。
        unsafe {
            let ctrl = pw.controller();
            let core = match ctrl.CoreWebView2() {
                Ok(c) => c,
                Err(e) => {
                    log(&format!("ATTACH {label} CoreWebView2 err={e:?}"));
                    return;
                }
            };
            let mut bpid = 0u32;
            let _ = core.BrowserProcessId(&mut bpid);
            let mut token = 0i64;
            let l1 = label.clone();
            let r1 = core.add_ProcessFailed(
                &ProcessFailedEventHandler::create(Box::new(move |sender, a| {
                    on_process_failed(&l1, sender, a);
                    Ok(())
                })),
                &mut token,
            );
            let l2 = label.clone();
            let r2 = core.add_NavigationCompleted(
                &NavigationCompletedEventHandler::create(Box::new(move |_, a| {
                    let mut ok = BOOL::default();
                    let mut st = Default::default();
                    if let Some(a) = a {
                        let _ = a.IsSuccess(&mut ok);
                        let _ = a.WebErrorStatus(&mut st);
                    }
                    log(&format!(
                        "NAV label={l2} success={} webErrorStatus={st:?}",
                        ok.as_bool()
                    ));
                    Ok(())
                })),
                &mut token,
            );
            log(&format!(
                "ATTACH {label} browserPid={bpid} addProcessFailed={r1:?} addNavigationCompleted={r2:?}"
            ));
        }
    })
}

/// 以目前一代的第一個視窗取得環境，查詢行程 → frame 對照；有變化才記錄並寫 info JSON。
fn query_mapping(force: bool) {
    let Some(app) = APP.get() else { return };
    let (label, gen) = {
        let s = state();
        (s.labels.first().cloned(), s.gen)
    };
    let Some(label) = label else { return };
    let Some(w) = app.get_webview_window(&label) else {
        return;
    };
    let _ = w.with_webview(move |pw| {
        let env13 = match pw.environment().cast::<ICoreWebView2Environment13>() {
            Ok(e) => e,
            Err(e) => {
                record_mapping(gen, format!("ERR cast Environment13 {e:?}"), None, force);
                return;
            }
        };
        let handler = GetProcessExtendedInfosCompletedHandler::create(Box::new(
            move |r, coll: Option<ICoreWebView2ProcessExtendedInfoCollection>| {
                match (r, coll) {
                    (Ok(()), Some(c)) => {
                        let (text, json) = describe(&c);
                        record_mapping(gen, text, Some(json), force);
                    }
                    (r, _) => record_mapping(gen, format!("ERR {r:?}"), None, force),
                }
                Ok(())
            },
        ));
        // SAFETY: 在 UI 執行緒上呼叫；完成回呼亦在 UI 執行緒派送。
        if let Err(e) = unsafe { env13.GetProcessExtendedInfos(&handler) } {
            record_mapping(
                gen,
                format!("ERR GetProcessExtendedInfos {e:?}"),
                None,
                force,
            );
        }
    });
}

/// 回傳（記錄用文字、info JSON 片段）。
fn describe(c: &ICoreWebView2ProcessExtendedInfoCollection) -> (String, String) {
    let mut parts = Vec::new();
    let mut procs = Vec::new();
    let mut count = 0u32;
    // SAFETY: COM 介面呼叫，輸出指標皆為區域變數。
    unsafe {
        let _ = c.Count(&mut count);
        for i in 0..count {
            let Ok(x) = c.GetValueAtIndex(i) else {
                continue;
            };
            let Ok(pi) = x.ProcessInfo() else { continue };
            let mut pid = 0i32;
            let mut kind = COREWEBVIEW2_PROCESS_KIND::default();
            let _ = pi.ProcessId(&mut pid);
            let _ = pi.Kind(&mut kind);
            let frames = x
                .AssociatedFrameInfos()
                .map(|f| frame_labels(&f))
                .unwrap_or_default();
            let kn = process_kind_name(kind);
            if frames.is_empty() {
                parts.push(format!("{kn}:{pid}"));
            } else {
                parts.push(format!("{kn}:{pid}{frames:?}"));
            }
            let fj: Vec<String> = frames.iter().map(|f| json_str(f)).collect();
            procs.push(format!(
                "{{\"pid\":{pid},\"kind\":{},\"frames\":[{}]}}",
                json_str(&kn),
                fj.join(",")
            ));
        }
    }
    parts.sort();
    (parts.join(" "), format!("[{}]", procs.join(",")))
}

fn record_mapping(gen: u32, text: String, json: Option<String>, force: bool) {
    let labels = {
        let mut s = state();
        if !force && s.mapping == text {
            return;
        }
        s.mapping = text.clone();
        s.labels.clone()
    };
    log(&format!("MAP gen={gen} {text}"));
    if let Some(j) = json {
        let lj: Vec<String> = labels.iter().map(|l| json_str(l)).collect();
        let info = format!(
            "{{\"pid\":{},\"gen\":{gen},\"udf\":{},\"labels\":[{}],\"processes\":{j}}}",
            std::process::id(),
            json_str(&args().udf.to_string_lossy()),
            lj.join(",")
        );
        let tmp = args().info.with_extension("tmp");
        if fs::write(&tmp, info).is_ok() {
            let _ = fs::rename(&tmp, &args().info);
        }
    }
}

/// 自訂協定：`/hb` 記錄心跳，其餘路徑回頁面。
fn handle_request(path: &str, query: &str) -> Response<Vec<u8>> {
    if path == "/hb" {
        let mut w = "";
        let mut id = "";
        for kv in query.split('&') {
            match kv.split_once('=') {
                Some(("w", v)) => w = v,
                Some(("id", v)) => id = v,
                _ => {}
            }
        }
        on_heartbeat(w, id);
        return Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(Vec::new())
            .expect("靜態回應不會失敗");
    }
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .body(PAGE.as_bytes().to_vec())
        .expect("靜態回應不會失敗")
}

fn on_heartbeat(w: &str, id: &str) {
    let now = Instant::now();
    let mut s = state();
    let since_fail = s.last_failure.as_ref().map(|(t, d)| {
        format!(
            " sinceFailureMs={} failure={d}",
            now.duration_since(*t).as_millis()
        )
    });
    let msg = match s.hb.get_mut(w) {
        None => Some(format!("HB-FIRST label={w} id={id}")),
        Some(h) => {
            let gap = now.duration_since(h.last).as_millis();
            let m = if h.id != id || h.stopped {
                Some(format!(
                    "HB-RESUME label={w} id={} -> {id} gapMs={gap}",
                    h.id
                ))
            } else {
                None
            };
            h.last = now;
            h.id = id.to_string();
            h.stopped = false;
            m
        }
    };
    if !s.hb.contains_key(w) {
        s.hb.insert(
            w.to_string(),
            Hb {
                last: now,
                id: id.to_string(),
                stopped: false,
            },
        );
    }
    drop(s);
    if let Some(m) = msg {
        log(&format!("{m}{}", since_fail.unwrap_or_default()));
    }
}

fn check_heartbeats() {
    let now = Instant::now();
    let mut stopped = Vec::new();
    {
        let mut s = state();
        let labels = s.labels.clone();
        for l in labels {
            if let Some(h) = s.hb.get_mut(&l) {
                let age = now.duration_since(h.last).as_millis();
                if !h.stopped && age > HB_STOP_MS {
                    h.stopped = true;
                    stopped.push(format!("HB-STOP label={l} ageMs={age}"));
                }
            }
        }
    }
    for m in stopped {
        log(&m);
    }
}

fn poll_ctl() {
    let path = &args().ctl;
    let Ok(text) = fs::read_to_string(path) else {
        return;
    };
    if text.trim().is_empty() {
        return;
    }
    let _ = fs::write(path, "");
    let Some(app) = APP.get() else { return };
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        log(&format!("CTL {line}"));
        let mut it = line.split_whitespace();
        match (it.next(), it.next()) {
            (Some("hang"), Some(label)) => match app.get_webview_window(label) {
                Some(w) => {
                    let r = w.eval("setTimeout(() => { for (;;) {} }, 0);");
                    log(&format!("CTL hang {label} eval={r:?}"));
                }
                None => log(&format!("CTL hang {label} 找不到視窗")),
            },
            (Some("recreate"), Some(label)) => recreate(label),
            (Some("dump"), _) => query_mapping(true),
            (Some("quit"), _) => {
                log("QUIT");
                app.exit(0);
            }
            _ => log("CTL 未知指令"),
        }
    }
}

fn main() {
    let a = parse_args();
    if let Some(dir) = a.log.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let file = File::create(&a.log).expect("無法建立記錄檔");
    let _ = LOG.set(Mutex::new(file));
    let _ = fs::write(&a.ctl, "");
    let _ = fs::remove_file(&a.info);
    log(&format!(
        "START rustPid={} udf={} count={} onBrowserExit={:?}",
        std::process::id(),
        a.udf.display(),
        a.count,
        a.on_browser_exit
    ));
    let _ = ARGS.set(a);

    let app = tauri::Builder::default()
        .register_uri_scheme_protocol(SCHEME, |_ctx, req| {
            handle_request(req.uri().path(), req.uri().query().unwrap_or(""))
        })
        .setup(|app| {
            let handle = app.handle().clone();
            let _ = APP.set(handle.clone());
            create_windows(&handle, 0)?;
            thread::spawn(|| {
                let mut tick = 0u32;
                loop {
                    thread::sleep(Duration::from_millis(250));
                    tick = tick.wrapping_add(1);
                    poll_ctl();
                    if tick.is_multiple_of(2) {
                        check_heartbeats();
                    }
                    if tick.is_multiple_of(8) {
                        query_mapping(false);
                    }
                }
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("probe_webview2 建立失敗");
    app.run(|_app, ev| {
        if let RunEvent::ExitRequested { api, code, .. } = ev {
            // 視窗全關（重建交接瞬間）不結束；只有明確 app.exit() 才結束。
            if code.is_none() {
                log("EXIT-REQUESTED code=None → prevent_exit");
                api.prevent_exit();
            } else {
                log(&format!("EXIT-REQUESTED code={code:?}"));
            }
        }
    });
}
