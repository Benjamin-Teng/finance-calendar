//! WebView2 使用者資料夾（dynamic-wallpaper task 4.5；design.md D2「渲染視窗使用獨立的 WebView2
//! 環境」）。
//!
//! ## 兩組環境
//!
//! - **小工具那組**（小工具、設定視窗、置底守門視窗、self-test 視窗）：沿用 Tauri 的預設——
//!   `data_directory` 不指定時 Tauri 2.12 一律補成 `app_local_data_dir()`（查證：
//!   `tauri-2.12.0/src/manager/webview.rs`「in `windows`, we need to force a data_directory」一段），
//!   WebView2 再於其下建 `EBWebView`。
//! - **渲染視窗**（label 前綴 `wallpaper-renderer-`，每次渲染一個 `wallpaper-renderer-<render id>`，
//!   以 `wallpaper_render::is_renderer_label` 判斷、不比對固定字串）：資料夾固定為
//!   `<同一個根>\wallpaper-renderer`（所有渲染視窗共用這一個資料夾），WebView2 於其下建
//!   自己的 `EBWebView`，因此有自己的 browser 行程；視窗關閉、最後一個引用它的 webview 消失後，
//!   Tauri 移除該 data directory 對應的 `WebContext`（`tauri-runtime-wry-2.12.0/src/lib.rs`
//!   `WebviewWrapper::drop`），wry 以它建立的 `ICoreWebView2Environment` 隨之釋放，browser 行程結束。
//!   每個 data directory 各自一個 `WebContext`（同檔 `create_webview`：以
//!   `webview_attributes.data_directory` 為鍵），wry 0.57 以該路徑呼叫
//!   `CreateCoreWebView2EnvironmentWithOptions(userDataFolder)`（`wry-0.57.0/src/webview2/mod.rs`
//!   `create_environment`）。
//!
//! ## 為什麼要接手 `WEBVIEW2_USER_DATA_FOLDER`
//!
//! WebView2 的環境變數 `WEBVIEW2_USER_DATA_FOLDER` 會**取代**呼叫端傳入的 `userDataFolder`
//! （Microsoft Learn，WebView2 Win32 參考 `CreateCoreWebView2EnvironmentWithOptions`：
//! 「If you find an override environment variable, use the browserExecutableFolder and userDataFolder
//! values as replacements for the corresponding values」）。驗收腳本以它把小工具那組隔離到暫存
//! 目錄；若照原樣留在行程環境裡，渲染視窗指定的資料夾會被同一個值蓋掉、兩組併成同一個 browser
//! 行程，D2 的獨立環境就不成立（使用者自行設了這個變數時亦同）。
//!
//! 所以 `main()` 一開始（建立任何執行緒與視窗之前）呼叫 [`capture_user_data_override`]：讀出值、
//! **從本行程環境移除**，之後小工具那組的每個建立點都以 [`with_widget_data_dir`] 明確指定這個值
//! （與原本由環境變數套用的結果相同，小工具那組行為不變），渲染視窗則用
//! `<這個值>\wallpaper-renderer`（[`renderer_data_dir`]）。沒設這個變數時兩者都走上一節的預設。
//!
//! 已知限制：登錄中的 WebView2 使用者資料夾原則（`...\Policies\Microsoft\Edge\WebView2\UserDataFolder`）
//! 同樣會取代呼叫端的值，本模組不處理（企業原則情境，記於 task 4.5 報告）。

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use tauri::{Manager, Runtime, WebviewWindowBuilder};

/// WebView2 讀取的使用者資料夾覆寫環境變數名。
pub const USER_DATA_ENV: &str = "WEBVIEW2_USER_DATA_FOLDER";
/// 渲染視窗的 WebView2 使用者資料夾（相對於小工具那組的根）。
pub const RENDERER_DATA_SUBDIR: &str = "wallpaper-renderer";

static OVERRIDE: OnceLock<Option<PathBuf>> = OnceLock::new();

/// 環境變數值 → 覆寫路徑；沒設或空字串＝沒有覆寫（純函式）。
pub fn interpret_override(value: Option<OsString>) -> Option<PathBuf> {
    value.filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// 渲染視窗的使用者資料夾：有覆寫時在覆寫值底下，否則在 app 本機資料夾底下（純函式）。兩者都
/// 沒有（app 本機資料夾解析失敗）回 `None`，呼叫端視為渲染失敗——**不**退回共用環境。
pub fn renderer_data_dir(override_dir: Option<&Path>, app_local: Option<&Path>) -> Option<PathBuf> {
    override_dir
        .or(app_local)
        .map(|root| root.join(RENDERER_DATA_SUBDIR))
}

/// 讀出並移除 `WEBVIEW2_USER_DATA_FOLDER`（見模組文件）。只有第一次呼叫生效，之後回傳同一個值。
/// 必須在 `main()` 建立任何執行緒之前呼叫（修改行程環境）。
pub fn capture_user_data_override() -> Option<PathBuf> {
    OVERRIDE
        .get_or_init(|| {
            let value = interpret_override(std::env::var_os(USER_DATA_ENV));
            if value.is_some() {
                // 單執行緒階段（main 開頭）修改自己的環境；Windows 上 std 以 SetEnvironmentVariableW
                // 實作，沒有 POSIX `setenv` 的執行緒安全問題。
                std::env::remove_var(USER_DATA_ENV);
            }
            value
        })
        .clone()
}

/// 小工具那組明確指定的使用者資料夾（`None`＝用 Tauri 預設）。
pub fn widget_data_dir() -> Option<PathBuf> {
    OVERRIDE.get().cloned().flatten()
}

/// 小工具那組的每個 `WebviewWindowBuilder` 建立點都經過這裡（見模組文件）。
pub fn with_widget_data_dir<'a, R: Runtime, M: Manager<R>>(
    builder: WebviewWindowBuilder<'a, R, M>,
) -> WebviewWindowBuilder<'a, R, M> {
    match widget_data_dir() {
        Some(dir) => builder.data_directory(dir),
        None => builder,
    }
}

/// 正式宿主中渲染視窗的使用者資料夾（`app_local_data_dir` 解析失敗時 `None`）。
pub fn renderer_data_dir_for<R: Runtime, M: Manager<R>>(manager: &M) -> Option<PathBuf> {
    let app_local = manager.path().app_local_data_dir().ok();
    renderer_data_dir(widget_data_dir().as_deref(), app_local.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_empty_or_missing_is_none() {
        assert_eq!(interpret_override(None), None);
        assert_eq!(interpret_override(Some(OsString::new())), None);
        assert_eq!(
            interpret_override(Some(OsString::from(r"C:\t\udf"))),
            Some(PathBuf::from(r"C:\t\udf"))
        );
    }

    #[test]
    fn renderer_dir_is_separate_subfolder() {
        let o = PathBuf::from(r"C:\t\udf");
        let l = PathBuf::from(r"C:\Users\u\AppData\Local\tw.fintools.fc-host");
        assert_eq!(
            renderer_data_dir(Some(&o), Some(&l)),
            Some(o.join("wallpaper-renderer")),
            "有覆寫時以覆寫值為根（小工具那組此時就用覆寫值本身）"
        );
        assert_eq!(
            renderer_data_dir(None, Some(&l)),
            Some(l.join("wallpaper-renderer"))
        );
        assert_eq!(
            renderer_data_dir(None, None),
            None,
            "解析不到不退回共用環境"
        );
        let r = renderer_data_dir(None, Some(&l)).expect("有值");
        assert_ne!(r, l, "渲染視窗的資料夾不可等於小工具那組的資料夾");
    }
}
