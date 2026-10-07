//! Task 5.5 驗收輔助：製造一個真正的全螢幕視窗，讓 `SHQueryUserNotificationState` 回報
//! `QUNS_BUSY`（Microsoft Learn「QUERY_USER_NOTIFICATION_STATE」：「QUNS_BUSY: A full-screen
//! application is running or Presentation Settings are applied.」——**不需要**獨占模式
//! Direct3D，一般全螢幕視窗即可），供 `host/tools/verify-5.5.ps1` 驗證宿主的 5 秒輪詢
//! （`desktop::poll_user_busy`／`BUSY_POLL_TIMER_ID`）與 [`crate::widgets::PauseReason::
//! SystemBusy`] 端對端可用，不需要真的寫一個獨占全螢幕 Direct3D swapchain。
//!
//! 判準（Windows 殼層對「全螢幕」的一般認定，非本檔發明）：前景視窗、無邊框（無
//! `WS_CAPTION`）、矩形與主螢幕**完整** `Monitor.size`／`Monitor.position` 相同（不是
//! `work_area`——後者已扣掉工作列，永遠不會被判定為全螢幕）。本探針視窗因此故意蓋住工作列。
//!
//! 用法：`cargo run --release --example probe_fullscreen_busy -- --duration-secs 20`
//! （預設 20 秒；期間視窗持續佔滿主螢幕並保持前景，逾時後自動結束行程、視窗銷毀、畫面復原）。
//!
//! 自包含（不依賴 `main.rs` 的私有模組，同 `probe_monitors.rs`／既有慣例：本 crate 沒有
//! `[lib]` target，examples 看不到 `mod desktop`／`mod widgets`）；沿用 `probe_monitors.rs`
//! 已驗證合法的 `probe-acl.html`（`WebviewUrl::App`）當視窗內容，畫面本身不重要，只有視窗
//! 矩形與前景狀態要正確。
#![windows_subsystem = "windows"]

use std::{env, thread, time::Duration};

use tauri::{WebviewUrl, WebviewWindowBuilder};

fn parse_duration_secs() -> u64 {
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--duration-secs" {
            if let Some(v) = args.next() {
                if let Ok(n) = v.parse() {
                    return n;
                }
            }
        }
    }
    20
}

fn main() {
    let duration = Duration::from_secs(parse_duration_secs());

    tauri::Builder::default()
        .setup(move |app| {
            let monitor = app
                .primary_monitor()?
                .expect("找不到主螢幕（primary_monitor 回傳 None）");
            let size = monitor.size();
            let position = monitor.position();
            println!(
                "probe_fullscreen_busy：主螢幕矩形 x={} y={} w={} h={}，視窗將維持前景 {duration:?}",
                position.x, position.y, size.width, size.height
            );

            let window = WebviewWindowBuilder::new(
                app,
                "probe-fullscreen-busy",
                WebviewUrl::App("probe-acl.html".into()),
            )
            .title("probe_fullscreen_busy")
            // 無邊框、不可調整大小：不是一般應用程式視窗的外觀，符合殼層「全螢幕應用程式」
            // 判準（有標題列／邊框的視窗即使佔滿螢幕也不算）。
            .decorations(false)
            .resizable(false)
            .skip_taskbar(true)
            // 故意留 focusable/focused 預設值（可取得焦點、建立時嘗試取得焦點）：全螢幕判準
            // 要求「前景視窗」，本探針必須能成為前景視窗，與正式小工具
            // `.focusable(false)`／永不取得前景的設計刻意相反。
            .always_on_top(true)
            .inner_size(size.width as f64, size.height as f64)
            .position(position.x as f64, position.y as f64)
            .visible(true)
            .build()?;

            // 建立後再次確保視窗矩形精確等於整個主螢幕（部分平台 inner_size／position 在
            // `build()` 當下可能因 DPI 換算差一兩個像素，`set_size`／`set_position` 用實體
            // 像素單位重新套用一次，消除誤差）。
            let _ = window.set_size(tauri::PhysicalSize::new(size.width, size.height));
            let _ = window.set_position(tauri::PhysicalPosition::new(position.x, position.y));
            let _ = window.set_focus();

            let handle = app.handle().clone();
            thread::spawn(move || {
                thread::sleep(duration);
                println!("probe_fullscreen_busy：時間到，結束行程（視窗隨之銷毀）");
                handle.exit(0);
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("probe_fullscreen_busy 啟動失敗");
}
