//! Task 2.2 權限驗收探針。
//!
//! 目的：在 **release build** 下證明 `host/capabilities/default.json`（`core:default`＋
//! `core:window:allow-start-dragging`＋`autostart:default`）確實放行小工具頁面要用的兩件事：
//! `window.__TAURI__.event.listen`／`emit`，以及 `getCurrentWindow().startDragging()`。
//!
//! 頁面：`host/ui/probe-acl.html`（不隨產品出貨、無任何 production 視窗指向它），以
//! `WebviewUrl::App` 從 `frontendDist` 載入——刻意不比照 `probe_webview2.rs` 的自訂 URI
//! scheme：`is_local_url`（`tauri-2.12.0/src/webview/mod.rs:1966`）只認 `tauri://localhost`
//! （Windows 上 `https://tauri.localhost`）這個本機來源，自訂 scheme 會被判成 remote
//! origin、直接被 capability 擋下（capability 沒設 `remote` 白名單），驗不出真正要測的東西。
//! 視窗標籤 `w-acl-test` 比對 `default.json` 的 `windows: ["w-*", "settings"]`，用的是與
//! 之後 task 3.1 真正小工具視窗相同的命名規則與同一份 capability 檔案。
//!
//! 回報機制：頁面用 `emit('probe-report', msg)`（JS→Rust 事件；`allow-emit` 已在
//! `core:event:default`／`core:default` 內）回報，不用 `invoke` 自訂 command——這樣不需要
//! 額外幫探針加 app 層 ACL 權限，驗的正是 D3 列出的那三條、不多不少。時序：頁面先
//! `listen('probe-event', …)`，完成後才回報 `"listening"`；本檔收到才 `emit_to` 測試事件回
//! 頁面，避免 listener 掛上前漏接（design.md D4 的啟動順序）。
//!
//! 執行（純 cargo）：`cargo run --release --example probe_acl -- --log <記錄檔路徑>`；
//! 5 秒後自動結束（`--timeout-secs` 可覆寫）。記錄檔逐行帶時間戳，含每次 `emit`／收到的
//! `probe-report` 內容；`startDragging` 的結果看 `REPORT drag-ok` 或
//! `REPORT drag-err:<訊息>`——`drag-err` 訊息只有包含 `"not allowed by ACL"`
//! （`tauri-2.12.0/src/webview/mod.rs:2112` release 分支的固定字串）才代表被 ACL 擋下；
//! 其餘失敗（例如當下沒有真的按著滑鼠鍵）是拖曳本身的執行結果，不是本 task 的驗收範圍
//! （實際拖曳留待 task 5.3）。
#![windows_subsystem = "windows"]

use std::{
    env,
    fs::File,
    io::Write,
    path::PathBuf,
    sync::{Mutex, OnceLock},
    thread,
    time::Duration,
};

use tauri::{Emitter, Listener, WebviewUrl, WebviewWindowBuilder};
use windows::Win32::System::SystemInformation::GetLocalTime;

static LOG: OnceLock<Mutex<File>> = OnceLock::new();

struct Args {
    log: PathBuf,
    timeout_secs: u64,
}

/// 旗標：`--log <path>`、`--timeout-secs <n>`。
fn parse_args() -> Args {
    let target = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
    let mut a = Args {
        log: target.join("probe_acl.log"),
        timeout_secs: 5,
    };
    let mut it = env::args().skip(1);
    while let Some(k) = it.next() {
        let v = it.next().unwrap_or_default();
        match k.as_str() {
            "--log" => a.log = PathBuf::from(v),
            "--timeout-secs" => a.timeout_secs = v.parse().unwrap_or(a.timeout_secs).max(1),
            _ => {}
        }
    }
    a
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

fn main() {
    let a = parse_args();
    if let Some(dir) = a.log.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let file = File::create(&a.log).expect("無法建立記錄檔");
    let _ = LOG.set(Mutex::new(file));
    log(&format!(
        "START rustPid={} timeoutSecs={}",
        std::process::id(),
        a.timeout_secs
    ));

    let timeout = a.timeout_secs;
    let app = tauri::Builder::default()
        .setup(move |app| {
            let win = WebviewWindowBuilder::new(
                app,
                "w-acl-test",
                WebviewUrl::App("probe-acl.html".into()),
            )
            .title("probe-acl")
            .inner_size(320.0, 200.0)
            .build()?;
            log(&format!(
                "WINDOW created label={:?} url={:?}",
                win.label(),
                win.url()
            ));

            let handle = app.handle().clone();
            app.listen_any("probe-report", move |event| {
                let payload = event.payload().to_string();
                log(&format!("REPORT {payload}"));
                // 引號包住的 JSON 字串（emit 的是純字串 payload）；去掉頭尾引號比對。
                let unquoted = payload.trim_matches('"');
                if unquoted == "listening" {
                    let r = handle.emit_to("w-acl-test", "probe-event", "hello-from-rust");
                    log(&format!("EMIT probe-event -> w-acl-test result={r:?}"));
                }
            });

            let handle2 = app.handle().clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_secs(timeout));
                log("TIMEOUT exit");
                handle2.exit(0);
            });

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("probe_acl 建立失敗");
    app.run(|_app, _ev| {});
}
