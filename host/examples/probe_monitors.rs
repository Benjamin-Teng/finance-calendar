//! Task 2.5 顯示器識別驗收探針。
//!
//! 目的：在本機記錄目前所有顯示器的「Tauri 來源名稱（`\\.\DISPLAYn`）→ 穩定的
//! `monitorDevicePath`」對照，證明 `desktop::source_name_to_device_path` 這條 Win32 呼叫鏈
//! （`GetDisplayConfigBufferSizes`／`QueryDisplayConfig`／`DisplayConfigGetDeviceInfo`）在真實
//! 硬體上能跑通、且與 Tauri 實際列舉到的螢幕清單對得上（design.md D9、tasks.md 2.5）。
//!
//! 「拔插外接螢幕後識別不變」需要人在拔插前後各跑一次本探針比較輸出——這是必須人工的驗收，
//! 步驟見 `.superpowers/sdd/tasks/human-checklist.md`「Task 2.5」節，本探針本身只負責「跑一次、
//! 印出目前狀態」，不自動比較兩次結果。
//!
//! 自包含（不依賴 `desktop.rs`）：本 crate 沒有 `[lib]` target（`main.rs` 的 `mod desktop;`
//! 等只在 `fc-host` 這個 bin 內可見），examples 無法 `use` 到它——比照既有 task 1.1／2.2／1.4
//! 探針的慣例，探針各自獨立即可直接呼叫 Win32／Tauri，不追求與正式程式碼共用同一份實作。下方
//! `source_name_to_device_path`／`query_source_name`／`query_target_device_path`／
//! `utf16_buf_to_string` 四個函式與 `host/src/desktop.rs` 的同名函式邏輯逐字相同（複製時間：
//! 本檔建立當下；desktop.rs 的單元測試涵蓋這段邏輯的正確性，本探針只用來在真實硬體上跑一次、
//! 產生證據檔，兩邊之後各自演進時若走偏，靠 `cargo test` 的單元測試把關 desktop.rs 那份）。
//!
//! 視窗：借用 task 2.2 已有的 `ui/probe-acl.html`（`WebviewUrl::App`，已驗證是合法的本機來源，
//! 見 `probe_acl.rs` 文件），只用來取得一個能呼叫 `.available_monitors()`／`.primary_monitor()`
//! 的 `Window`，不使用它的任何 JS 內容。
//!
//! 執行（純 cargo）：
//! `cargo run --release --example probe_monitors -- --out <證據檔路徑>`
//! 不給 `--out` 時預設寫到 `host/tools/evidence/2.5-monitors.txt`。
#![windows_subsystem = "windows"]

use std::{collections::HashMap, env, fs, mem::size_of, path::PathBuf};

use tauri::{WebviewUrl, WebviewWindowBuilder};
use windows::Win32::{
    Devices::Display::{
        DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig,
        DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
        DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
        DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME, QDC_ONLY_ACTIVE_PATHS,
    },
    Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS},
};

const MAX_QUERY_ATTEMPTS: u32 = 8;

/// 與 `host/src/desktop.rs::source_name_to_device_path` 邏輯相同，見本檔頂部文件「自包含」。
fn source_name_to_device_path() -> HashMap<String, String> {
    let mut map = HashMap::new();

    for _ in 0..MAX_QUERY_ATTEMPTS {
        let mut path_count: u32 = 0;
        let mut mode_count: u32 = 0;
        let sizes_status = unsafe {
            GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count)
        };
        if sizes_status != ERROR_SUCCESS {
            return map;
        }

        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];
        let query_status = unsafe {
            QueryDisplayConfig(
                QDC_ONLY_ACTIVE_PATHS,
                &mut path_count,
                paths.as_mut_ptr(),
                &mut mode_count,
                modes.as_mut_ptr(),
                None,
            )
        };

        if query_status == ERROR_SUCCESS {
            paths.truncate(path_count as usize);
            for path in &paths {
                if let (Some(source_name), Some(device_path)) =
                    (query_source_name(path), query_target_device_path(path))
                {
                    map.insert(source_name, device_path);
                }
            }
            return map;
        }
        if query_status != ERROR_INSUFFICIENT_BUFFER {
            return map;
        }
    }

    map
}

fn query_source_name(path: &DISPLAYCONFIG_PATH_INFO) -> Option<String> {
    let mut request = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
            size: size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
            adapterId: path.sourceInfo.adapterId,
            id: path.sourceInfo.id,
        },
        ..Default::default()
    };
    let status = unsafe { DisplayConfigGetDeviceInfo(&mut request.header) };
    (status == 0).then(|| utf16_buf_to_string(&request.viewGdiDeviceName))
}

fn query_target_device_path(path: &DISPLAYCONFIG_PATH_INFO) -> Option<String> {
    let mut request = DISPLAYCONFIG_TARGET_DEVICE_NAME {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
            size: size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
            adapterId: path.targetInfo.adapterId,
            id: path.targetInfo.id,
        },
        ..Default::default()
    };
    let status = unsafe { DisplayConfigGetDeviceInfo(&mut request.header) };
    (status == 0).then(|| utf16_buf_to_string(&request.monitorDevicePath))
}

fn utf16_buf_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// 旗標：`--out <path>`。
fn parse_out_path() -> PathBuf {
    let default_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tools")
        .join("evidence")
        .join("2.5-monitors.txt");
    let mut it = env::args().skip(1);
    while let Some(k) = it.next() {
        let v = it.next().unwrap_or_default();
        if k == "--out" && !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    default_path
}

fn main() {
    let out_path = parse_out_path();

    tauri::Builder::default()
        .setup(move |app| {
            let window = WebviewWindowBuilder::new(
                app,
                "probe-monitors",
                WebviewUrl::App("probe-acl.html".into()),
            )
            .visible(false)
            .build()?;

            let monitors = window.available_monitors().unwrap_or_default();
            let primary = window.primary_monitor().ok().flatten();
            let device_paths = source_name_to_device_path();

            let mut lines = Vec::new();
            lines.push(format!("monitor_count={}", monitors.len()));
            lines.push(format!(
                "device_path_table_count={}",
                device_paths.len()
            ));

            for m in &monitors {
                let name = m
                    .name()
                    .cloned()
                    .unwrap_or_else(|| "<none>".to_string());
                let device_path = device_paths
                    .get(&name)
                    .cloned()
                    .unwrap_or_else(|| "<not-found-in-table>".to_string());
                let wa = m.work_area();
                let is_primary = primary.as_ref().and_then(|p| p.name()) == m.name();
                lines.push(format!(
                    "name={name} device_path={device_path} work_area=(x={}, y={}, w={}, h={}) scale_factor={} is_primary={is_primary}",
                    wa.position.x, wa.position.y, wa.size.width, wa.size.height, m.scale_factor()
                ));
            }

            if let Some(dir) = out_path.parent() {
                let _ = fs::create_dir_all(dir);
            }
            let body = lines.join("\n") + "\n";
            fs::write(&out_path, &body)?;
            print!("{body}");
            println!("已寫入 {}", out_path.display());

            app.handle().exit(0);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("probe_monitors 啟動失敗");
}
