//! 系統匣與設定視窗圖示隨桌布主題切換（dynamic-wallpaper task 5.2；design.md D9；
//! specs/app-icon「系統匣與設定視窗圖示隨主題切換」）。
//!
//! exe 本身的圖示資源（統一圖示 `icons/icon.ico`，八個尺寸）由 `build.rs` 的
//! `window_icon_path` 內嵌，不經本模組。本模組只負責執行期換圖示：主題為「不接管」用統一圖示，
//! 五套主題各用 `icons/theme-<id>.ico` 的同一組點陣。
//!
//! ## 圖示來源：`tauri::include_image!`＋逐尺寸 PNG
//!
//! - `include_image!`（`tauri-macros-2.7.0/src/lib.rs`，經 `tauri-codegen-2.7.0/src/image.rs`
//!   的 `CachedIcon`）在**編譯期**把 PNG／ICO 解成 RGBA 存進執行檔，執行期不需 `image-png`／
//!   `image-ico` feature。但它吃 `.ico` 時只取**最大**的 entry（`largest_ico_entry`），系統匣拿到
//!   256px 再由系統縮小，16／20／24 的簡化造型就用不到了。
//! - 故 `host/tools/make-icons.mjs` 由各 `.ico` 衍生逐尺寸 PNG（`host/icons/png/<名稱>-<尺寸>.png`，
//!   與 `.ico` 的 entry 逐位元組相同，`--verify` 與 `host/tests/ico.test.mjs` 核對），本模組逐張
//!   `include_image!`。內嵌 [`EMBEDDED_SIZES`]＝16–64 七個尺寸（256 不內嵌：未壓縮 RGBA 一張就
//!   256 KB）。
//! - `include_image!` 只追蹤它自己寫進 `OUT_DIR` 的快取檔，不追蹤來源 PNG；`build.rs` 對
//!   `icons/png` 下 `rerun-if-changed`，換 PNG 會重跑建置腳本並重編本 crate。
//!
//! ## 尺寸（以原始碼查證）
//!
//! - 系統匣：Tauri `TrayIcon::set_icon` 把 [`Image`] 交給 `tray-icon-0.25.1`
//!   `platform_impl/windows/icon.rs` 的 `RgbaIcon::into_windows_icon`，以 `CreateIcon` 依影像本身
//!   寬高建立**單一尺寸**的 HICON，再 `Shell_NotifyIconW(NIM_MODIFY)`；不會依 DPI 挑尺寸，
//!   系統只會把這張縮放到通知區域要的大小。通知區域要的是小圖示（96 DPI 下 16px，隨縮放：
//!   125%＝20、150%＝24、200%＝32），故依**主要顯示器**（工作列通知區域所在）的縮放挑
//!   [`tray_size`]，縮放或顯示組態改變時重新設定（[`request_refresh`] 的呼叫端見下）。
//! - 設定視窗：Tauri `WebviewWindow::set_icon` 經 `tauri-runtime-wry-2.12.0` 的
//!   `WindowMessage::SetIcon` 呼叫 tao `set_window_icon`，只設 `WM_SETICON(ICON_SMALL)`
//!   （`tao-0.37.1/src/platform_impl/windows/window.rs`；tao 的視窗類別不帶圖示）。這一張同時是
//!   標題列圖示與工作列按鈕的退回圖示；Tauri 自己的預設視窗圖示
//!   （`tauri-2.12.0/src/image/mod.rs` `default_window_icon_from_app_icon_resource`）因此取
//!   **大圖示**尺寸（`SM_CXICON`，96 DPI 下 32px），本模組沿用同一判斷（[`window_size`]），
//!   以視窗所在顯示器的縮放計算，視窗換到不同縮放的顯示器時重新設定。
//!
//! ## 收斂點與執行緒（呼叫端契約）
//!
//! - **唯一入口** [`request_refresh`]：「主題可能變了／縮放可能變了／設定視窗剛建立」時呼叫，
//!   不帶主題參數——實際主題在套用前由 [`refresh_in_order`] 從 `AppState.settings` 讀最新值。
//!   呼叫點：`widgets::emit_settings`（所有設定變更都經這裡廣播，含設定視窗選主題、讓位、焦點
//!   取消、安全閥、系統匣結束、`--restore-wallpaper` 交接——它們都走
//!   `widgets::update_settings`／`widgets::set_wallpaper_theme_none`）、啟動（`main.rs` setup）、
//!   設定視窗建立後（`tray::create_settings_window`）、顯示組態變更（守門視窗的 `relayout_all`
//!   回呼）、設定視窗的 `ScaleFactorChanged`（`main.rs` `.run()`）、explorer 重新啟動
//!   （[`on_explorer_restarted`]，先作廢系統匣去重紀錄）。
//! - 系統匣設定失敗（explorer 不在）會清掉去重紀錄並延後重試（[`apply_tray_choice`]、
//!   [`tray_retry_delay`]）。
//! - [`request_refresh`] **不阻塞呼叫端**：開一條短命執行緒讀設定並以 `run_on_main_thread` 投遞
//!   套用工作就返回——呼叫端可能是同步指令（主執行緒，`update_settings`）、守門視窗的 WndProc 或
//!   協調迴圈；`Shell_NotifyIconW` 要跨行程送到 explorer，不在這些地方同步做（memory：同步
//!   WndProc 裡做慢工作會卡住發送端）。
//! - 套用（[`apply_on_main_thread`]）一律在**主執行緒**：tray／視窗 API 有執行緒親和性
//!   （memory：Win32 視窗 API 有執行緒親和性），`run_on_main_thread` 在工作執行緒呼叫時只投遞、
//!   不就地執行（`tauri-runtime-wry` `send_user_message`）。
//! - 順序：多個 [`request_refresh`] 並行時，「讀主題＋投遞」在 [`REFRESH_ORDER`] 鎖內完成，主執行緒
//!   的工作佇列又是先進先出，故最後一個被套用的一定是最後讀到的主題（[`refresh_in_order`]）。

use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use tauri::image::Image;
use tauri::{include_image, AppHandle, Manager};

use crate::settings::WallpaperTheme;
use crate::widgets::AppState;

/// 執行期內嵌的尺寸（由小到大；`host/tools/lib/icon-sources.mjs` 的 `RUNTIME_PNG_SIZES` 必須相同，
/// 測試核對）。
pub const EMBEDDED_SIZES: [u32; 7] = [16, 20, 24, 32, 40, 48, 64];

/// 96 DPI 下的小圖示邊長（`SM_CXSMICON`）：系統匣通知區域。
const TRAY_BASE_PX: f64 = 16.0;
/// 96 DPI 下的大圖示邊長（`SM_CXICON`）：設定視窗（見模組文件「尺寸」）。
const WINDOW_BASE_PX: f64 = 32.0;

/// 一組圖示（＝`host/icons/` 下一個 `.ico`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IconSet {
    /// 統一產品圖示（`icon.ico`）。
    Unified,
    Astrolabe,
    Tearoff,
    Ridgeline,
    Contour,
    Skyline,
}

impl IconSet {
    /// 全部六組，順序同 `WallpaperTheme::ALL`（只有測試逐組檢查時用到）。
    #[cfg(test)]
    pub const ALL: [IconSet; 6] = [
        IconSet::Unified,
        IconSet::Astrolabe,
        IconSet::Tearoff,
        IconSet::Ridgeline,
        IconSet::Contour,
        IconSet::Skyline,
    ];

    /// specs/app-icon：「不接管」用統一圖示，其餘用該主題的主題圖示。
    pub fn for_theme(theme: WallpaperTheme) -> Self {
        match theme {
            WallpaperTheme::None => IconSet::Unified,
            WallpaperTheme::Astrolabe => IconSet::Astrolabe,
            WallpaperTheme::Tearoff => IconSet::Tearoff,
            WallpaperTheme::Ridgeline => IconSet::Ridgeline,
            WallpaperTheme::Contour => IconSet::Contour,
            WallpaperTheme::Skyline => IconSet::Skyline,
        }
    }

    /// `.ico`／PNG 的檔名主幹（`icon`、`theme-<id>`）。
    pub fn name(self) -> &'static str {
        match self {
            IconSet::Unified => "icon",
            IconSet::Astrolabe => "theme-astrolabe",
            IconSet::Tearoff => "theme-tearoff",
            IconSet::Ridgeline => "theme-ridgeline",
            IconSet::Contour => "theme-contour",
            IconSet::Skyline => "theme-skyline",
        }
    }

    fn table(self) -> &'static [Image<'static>; 7] {
        match self {
            IconSet::Unified => &UNIFIED,
            IconSet::Astrolabe => &ASTROLABE,
            IconSet::Tearoff => &TEAROFF,
            IconSet::Ridgeline => &RIDGELINE,
            IconSet::Contour => &CONTOUR,
            IconSet::Skyline => &SKYLINE,
        }
    }
}

// 逐張列出：`include_image!` 只吃字串字面值（不吃 `concat!`）。順序必須同 [`EMBEDDED_SIZES`]
// （測試核對每張的寬高）。
static UNIFIED: [Image<'static>; 7] = [
    include_image!("icons/png/icon-16.png"),
    include_image!("icons/png/icon-20.png"),
    include_image!("icons/png/icon-24.png"),
    include_image!("icons/png/icon-32.png"),
    include_image!("icons/png/icon-40.png"),
    include_image!("icons/png/icon-48.png"),
    include_image!("icons/png/icon-64.png"),
];
static ASTROLABE: [Image<'static>; 7] = [
    include_image!("icons/png/theme-astrolabe-16.png"),
    include_image!("icons/png/theme-astrolabe-20.png"),
    include_image!("icons/png/theme-astrolabe-24.png"),
    include_image!("icons/png/theme-astrolabe-32.png"),
    include_image!("icons/png/theme-astrolabe-40.png"),
    include_image!("icons/png/theme-astrolabe-48.png"),
    include_image!("icons/png/theme-astrolabe-64.png"),
];
static TEAROFF: [Image<'static>; 7] = [
    include_image!("icons/png/theme-tearoff-16.png"),
    include_image!("icons/png/theme-tearoff-20.png"),
    include_image!("icons/png/theme-tearoff-24.png"),
    include_image!("icons/png/theme-tearoff-32.png"),
    include_image!("icons/png/theme-tearoff-40.png"),
    include_image!("icons/png/theme-tearoff-48.png"),
    include_image!("icons/png/theme-tearoff-64.png"),
];
static RIDGELINE: [Image<'static>; 7] = [
    include_image!("icons/png/theme-ridgeline-16.png"),
    include_image!("icons/png/theme-ridgeline-20.png"),
    include_image!("icons/png/theme-ridgeline-24.png"),
    include_image!("icons/png/theme-ridgeline-32.png"),
    include_image!("icons/png/theme-ridgeline-40.png"),
    include_image!("icons/png/theme-ridgeline-48.png"),
    include_image!("icons/png/theme-ridgeline-64.png"),
];
static CONTOUR: [Image<'static>; 7] = [
    include_image!("icons/png/theme-contour-16.png"),
    include_image!("icons/png/theme-contour-20.png"),
    include_image!("icons/png/theme-contour-24.png"),
    include_image!("icons/png/theme-contour-32.png"),
    include_image!("icons/png/theme-contour-40.png"),
    include_image!("icons/png/theme-contour-48.png"),
    include_image!("icons/png/theme-contour-64.png"),
];
static SKYLINE: [Image<'static>; 7] = [
    include_image!("icons/png/theme-skyline-16.png"),
    include_image!("icons/png/theme-skyline-20.png"),
    include_image!("icons/png/theme-skyline-24.png"),
    include_image!("icons/png/theme-skyline-32.png"),
    include_image!("icons/png/theme-skyline-40.png"),
    include_image!("icons/png/theme-skyline-48.png"),
    include_image!("icons/png/theme-skyline-64.png"),
];

/// 要畫成 `px` 邊長時用哪個內嵌尺寸：四捨五入後**不小於**它的最小內嵌尺寸（讓系統縮小，不放大；
/// 175%＝28px 用 32），超過最大者用 64。`px` 非有限值或不大於 0 時當成 16。
pub fn size_for_px(px: f64) -> u32 {
    let smallest = EMBEDDED_SIZES[0];
    let largest = EMBEDDED_SIZES[EMBEDDED_SIZES.len() - 1];
    if !px.is_finite() || px <= 0.0 {
        return smallest;
    }
    let want = px.round();
    EMBEDDED_SIZES
        .iter()
        .copied()
        .find(|&s| f64::from(s) >= want)
        .unwrap_or(largest)
}

/// 系統匣圖示尺寸：`16 × 縮放`（見模組文件「尺寸」）。
pub fn tray_size(scale: f64) -> u32 {
    size_for_px(TRAY_BASE_PX * scale)
}

/// 設定視窗圖示尺寸：`32 × 縮放`（見模組文件「尺寸」）。
pub fn window_size(scale: f64) -> u32 {
    size_for_px(WINDOW_BASE_PX * scale)
}

/// 取一張內嵌圖示（借用執行檔內的 RGBA，複製成本只有一個 `Cow::Borrowed`）。`size` 不是內嵌尺寸時
/// 先經 [`size_for_px`] 換成內嵌尺寸。
pub fn image(set: IconSet, size: u32) -> Image<'static> {
    let size = size_for_px(f64::from(size));
    let index = EMBEDDED_SIZES
        .iter()
        .position(|&s| s == size)
        .unwrap_or(EMBEDDED_SIZES.len() - 1);
    set.table()[index].clone()
}

/// 一次套用的選擇（哪組、多大）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IconChoice {
    pub set: IconSet,
    pub size: u32,
}

/// 系統匣圖示的去重紀錄：`emit_settings` 在每次設定變更（含編輯版面、拖曳）都會觸發刷新，主題與
/// 縮放都沒變時不重送 `Shell_NotifyIconW`。只在主執行緒使用（[`apply_on_main_thread`]）。
#[derive(Debug, Default)]
pub struct TrayMemo(Mutex<Option<IconChoice>>);

impl TrayMemo {
    pub const fn new() -> Self {
        TrayMemo(Mutex::new(None))
    }

    /// 這次選擇是否需要真的設定（與上次成功套用的不同才要），需要時先記下。
    pub fn should_apply(&self, choice: IconChoice) -> bool {
        let mut last = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if *last == Some(choice) {
            return false;
        }
        *last = Some(choice);
        true
    }

    /// 設定失敗：清掉紀錄，下次刷新必定重試。
    pub fn forget(&self) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

static TRAY_MEMO: TrayMemo = TrayMemo::new();

/// 系統匣設定失敗後的重試間隔與次數上限（修正輪 1，審查 low1）：explorer 重啟期間
/// `Shell_NotifyIconW(NIM_MODIFY)` 會失敗，而 `tray-icon-0.25.1` 的 `set_icon` 失敗時提早返回、
/// **不**存下新圖示（`platform_impl/windows/mod.rs` 的 `set_icon`），explorer 起來後它以存著的舊圖示
/// 重新 `NIM_ADD`。重試讓失敗不會停在舊主題；15 次 × 2 秒涵蓋一般的 explorer 重啟時間，之後還有
/// [`on_explorer_restarted`] 兜底。
const TRAY_RETRY_INTERVAL: Duration = Duration::from_secs(2);
const TRAY_MAX_RETRIES: u32 = 15;

/// 第 `attempt` 次（0＝首次）設定系統匣失敗後，多久後重試；用完次數回 `None`。
pub fn tray_retry_delay(attempt: u32) -> Option<Duration> {
    (attempt < TRAY_MAX_RETRIES).then_some(TRAY_RETRY_INTERVAL)
}

/// 系統匣套用一次的結果（[`apply_tray_choice`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayApply {
    /// 與上次成功套用的相同，沒有重送。
    Skipped,
    /// 已設定。
    Applied,
    /// 設定失敗（已清掉去重紀錄）；`retry_in` 是建議的重試延遲，`None`＝次數用完。
    Failed {
        error: String,
        retry_in: Option<Duration>,
    },
}

/// 系統匣套用的本體（可注入 `set`，供單元測試模擬 explorer 不在／重啟）：去重 → 設定 → 失敗時清掉
/// 紀錄並給出重試延遲。
pub fn apply_tray_choice(
    memo: &TrayMemo,
    choice: IconChoice,
    attempt: u32,
    set: impl FnOnce(IconChoice) -> Result<(), String>,
) -> TrayApply {
    if !memo.should_apply(choice) {
        return TrayApply::Skipped;
    }
    match set(choice) {
        Ok(()) => TrayApply::Applied,
        Err(error) => {
            memo.forget();
            TrayApply::Failed {
                error,
                retry_in: tray_retry_delay(attempt),
            }
        }
    }
}

/// explorer 重新啟動（`TaskbarCreated`）時去重紀錄作廢：tray-icon 以它存的那張重新登記，那張不一定
/// 是最新主題（見 [`TRAY_RETRY_INTERVAL`] 文件），故下一次刷新必定重送。
pub fn explorer_restarted(memo: &TrayMemo) {
    memo.forget();
}

/// 守門視窗收到 explorer 重新啟動（`main.rs` 的 `SystemEvent::ExplorerRestarted`）：作廢去重紀錄並
/// 刷新。立即返回。
pub fn on_explorer_restarted(app: &AppHandle) {
    explorer_restarted(&TRAY_MEMO);
    request_refresh(app);
}

/// 「讀主題＋投遞」的順序鎖（見模組文件「順序」）。
static REFRESH_ORDER: Mutex<()> = Mutex::new(());

/// 在 `order` 鎖內先 `read` 再 `post(讀到的值)`。`post` 只投遞（不等結果），且投遞的目的地是先進
/// 先出佇列時，最後一次被處理的一定是最後一次 `read` 的結果——之後任何寫入都會再觸發一次刷新。
pub fn refresh_in_order<T>(order: &Mutex<()>, read: impl FnOnce() -> T, post: impl FnOnce(T)) {
    let _guard = order.lock().unwrap_or_else(|e| e.into_inner());
    post(read());
}

/// 主題、縮放可能變了，或設定視窗剛建立：重新設定系統匣與設定視窗圖示（唯一入口，見模組文件
/// 「收斂點與執行緒」）。立即返回。
pub fn request_refresh(app: &AppHandle) {
    spawn_refresh(app, 0, None);
}

/// 開短命執行緒（必要時先等 `delay`）再 [`refresh_now`]；`attempt` 是系統匣重試的次數（0＝首次）。
fn spawn_refresh(app: &AppHandle, attempt: u32, delay: Option<Duration>) {
    let handle = app.clone();
    let spawned = thread::Builder::new()
        .name("fc-app-icon".to_owned())
        .spawn(move || {
            if let Some(delay) = delay {
                thread::sleep(delay);
            }
            refresh_now(&handle, attempt);
        });
    if let Err(err) = spawned {
        log::warn!("圖示：無法開執行緒刷新系統匣／設定視窗圖示：{err}");
    }
}

fn refresh_now(app: &AppHandle, attempt: u32) {
    refresh_in_order(
        &REFRESH_ORDER,
        || {
            app.try_state::<AppState>().map(|state| {
                state
                    .settings
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .wallpaper_theme
            })
        },
        |theme| {
            let Some(theme) = theme else {
                log::warn!("圖示：AppState 未就緒，略過刷新");
                return;
            };
            let handle = app.clone();
            if let Err(err) =
                app.run_on_main_thread(move || apply_on_main_thread(&handle, theme, attempt))
            {
                log::warn!("圖示：投遞到主執行緒失敗：{err}");
            }
        },
    );
}

/// 主執行緒：依 `theme` 設定系統匣（主要顯示器縮放）與設定視窗（視窗所在顯示器縮放）的圖示。
fn apply_on_main_thread(app: &AppHandle, theme: WallpaperTheme, attempt: u32) {
    let set = IconSet::for_theme(theme);

    if let Some(tray) = app.tray_by_id(crate::tray::TRAY_ID) {
        let scale = app
            .primary_monitor()
            .ok()
            .flatten()
            .map(|m| m.scale_factor())
            .unwrap_or(1.0);
        let choice = IconChoice {
            set,
            size: tray_size(scale),
        };
        let outcome = apply_tray_choice(&TRAY_MEMO, choice, attempt, |c| {
            tray.set_icon(Some(image(c.set, c.size)))
                .map_err(|e| e.to_string())
        });
        match outcome {
            TrayApply::Skipped => {}
            TrayApply::Applied => log::info!(
                "圖示：系統匣 → {} {}px（主題 {}，主要顯示器縮放 {scale:.2}）",
                choice.set.name(),
                choice.size,
                theme.as_str()
            ),
            TrayApply::Failed {
                error,
                retry_in: Some(delay),
            } => {
                log::warn!(
                    "圖示：設定系統匣圖示失敗（第 {} 次，{} 秒後重試；explorer 可能正在重新啟動）：{error}",
                    attempt + 1,
                    delay.as_secs()
                );
                spawn_refresh(app, attempt + 1, Some(delay));
            }
            TrayApply::Failed {
                error,
                retry_in: None,
            } => log::warn!(
                "圖示：設定系統匣圖示失敗（第 {} 次，不再重試，等下一次設定或 explorer 重新啟動）：{error}",
                attempt + 1
            ),
        }
    }

    if let Some(window) = app.get_webview_window(crate::tray::SETTINGS_WINDOW_LABEL) {
        let scale = window.scale_factor().unwrap_or(1.0);
        let size = window_size(scale);
        match window.set_icon(image(set, size)) {
            Ok(()) => log::info!(
                "圖示：設定視窗 → {} {size}px（主題 {}，縮放 {scale:.2}）",
                set.name(),
                theme.as_str()
            ),
            Err(err) => log::warn!("圖示：設定設定視窗圖示失敗：{err}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::Arc;

    fn manifest_dir() -> &'static Path {
        Path::new(env!("CARGO_MANIFEST_DIR"))
    }

    #[test]
    fn theme_none_uses_unified_icon_and_each_theme_its_own() {
        let got: Vec<(WallpaperTheme, IconSet)> = WallpaperTheme::ALL
            .iter()
            .map(|&t| (t, IconSet::for_theme(t)))
            .collect();
        assert_eq!(
            got,
            vec![
                (WallpaperTheme::None, IconSet::Unified),
                (WallpaperTheme::Astrolabe, IconSet::Astrolabe),
                (WallpaperTheme::Tearoff, IconSet::Tearoff),
                (WallpaperTheme::Ridgeline, IconSet::Ridgeline),
                (WallpaperTheme::Contour, IconSet::Contour),
                (WallpaperTheme::Skyline, IconSet::Skyline),
            ]
        );
        // 主題圖示的名稱＝theme-<主題字串>，且 .ico 存在
        for theme in WallpaperTheme::ALL {
            let set = IconSet::for_theme(theme);
            if theme != WallpaperTheme::None {
                assert_eq!(set.name(), format!("theme-{}", theme.as_str()));
            }
            let ico = manifest_dir()
                .join("icons")
                .join(format!("{}.ico", set.name()));
            assert!(ico.exists(), "{} 不存在", ico.display());
        }
    }

    #[test]
    fn tray_size_follows_small_icon_metric_per_scale() {
        let cases = [
            (1.0, 16),
            (1.25, 20),
            (1.5, 24),
            (1.75, 32), // 28 → 不小於它的最小內嵌尺寸
            (2.0, 32),
            (2.25, 40), // 36
            (2.5, 40),
            (3.0, 48),
            (3.5, 64), // 56
            (4.0, 64),
            (5.0, 64), // 80 → 最大內嵌
        ];
        for (scale, want) in cases {
            assert_eq!(tray_size(scale), want, "縮放 {scale}");
        }
    }

    #[test]
    fn window_size_follows_large_icon_metric_per_scale() {
        let cases = [
            (1.0, 32),
            (1.25, 40),
            (1.5, 48),
            (1.75, 64),
            (2.0, 64),
            (3.0, 64),
        ];
        for (scale, want) in cases {
            assert_eq!(window_size(scale), want, "縮放 {scale}");
        }
    }

    #[test]
    fn size_for_px_guards_nonsense_input() {
        for px in [0.0, -3.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(size_for_px(px), 16, "px={px}");
        }
        assert_eq!(size_for_px(1.0), 16);
        assert_eq!(size_for_px(16.4), 16, "四捨五入");
        assert_eq!(size_for_px(16.6), 20);
        assert_eq!(size_for_px(64.0), 64);
        assert_eq!(size_for_px(256.0), 64);
    }

    #[test]
    fn every_embedded_image_has_its_declared_size() {
        for set in IconSet::ALL {
            for size in EMBEDDED_SIZES {
                let img = image(set, size);
                assert_eq!((img.width(), img.height()), (size, size), "{set:?} {size}");
                assert_eq!(
                    img.rgba().len(),
                    (size * size * 4) as usize,
                    "{set:?} {size}"
                );
            }
        }
        // 非內嵌尺寸換成內嵌尺寸
        assert_eq!(image(IconSet::Unified, 28).width(), 32);
        assert_eq!(image(IconSet::Unified, 256).width(), 64);
    }

    #[test]
    fn sets_are_distinct_at_every_size() {
        for size in EMBEDDED_SIZES {
            for (i, a) in IconSet::ALL.iter().enumerate() {
                for b in &IconSet::ALL[i + 1..] {
                    assert_ne!(
                        image(*a, size).rgba(),
                        image(*b, size).rgba(),
                        "{a:?} 與 {b:?} 的 {size}px 一模一樣"
                    );
                }
            }
        }
    }

    /// `host/icons/png/` 下的檔案恰好是本模組內嵌的那 42 張（make-icons 多產或少產都會失敗），且尺寸表
    /// 與 icon-sources.mjs 的 `RUNTIME_PNG_SIZES` 相同。
    #[test]
    fn png_directory_matches_embedded_table() {
        let dir = manifest_dir().join("icons").join("png");
        let mut on_disk: Vec<String> = std::fs::read_dir(&dir)
            .expect("讀 icons/png")
            .map(|e| {
                e.expect("目錄項目")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        on_disk.sort();
        let mut want: Vec<String> = IconSet::ALL
            .iter()
            .flat_map(|s| EMBEDDED_SIZES.map(|n| format!("{}-{n}.png", s.name())))
            .collect();
        want.sort();
        assert_eq!(on_disk, want);

        let src = include_str!("../tools/lib/icon-sources.mjs");
        let sizes = EMBEDDED_SIZES.map(|n| n.to_string()).join(", ");
        assert!(
            src.contains(&format!("RUNTIME_PNG_SIZES = [{sizes}]")),
            "icon-sources.mjs 的 RUNTIME_PNG_SIZES 與 EMBEDDED_SIZES 不一致"
        );
    }

    #[test]
    fn tray_memo_skips_repeats_and_retries_after_forget() {
        let memo = TrayMemo::new();
        let a = IconChoice {
            set: IconSet::Unified,
            size: 16,
        };
        let b = IconChoice {
            set: IconSet::Astrolabe,
            size: 16,
        };
        let c = IconChoice {
            set: IconSet::Astrolabe,
            size: 24,
        };
        assert!(memo.should_apply(a), "第一次一定要設");
        assert!(!memo.should_apply(a), "沒變不重設");
        assert!(memo.should_apply(b), "主題變了");
        assert!(memo.should_apply(c), "縮放變了");
        assert!(!memo.should_apply(c));
        memo.forget();
        assert!(memo.should_apply(c), "失敗後要重試");
    }

    /// 並行的「寫入主題後刷新」：投遞進先進先出佇列的最後一筆必定等於最後寫入的值（模擬
    /// `run_on_main_thread` 的工作佇列）。
    #[test]
    fn refresh_in_order_last_post_matches_last_write() {
        for _round in 0..50 {
            let value = Arc::new(Mutex::new(0u32));
            let queue = Arc::new(Mutex::new(Vec::<u32>::new()));
            let order = Arc::new(Mutex::new(()));
            let threads: Vec<_> = (1..=8u32)
                .map(|i| {
                    let (value, queue, order) = (value.clone(), queue.clone(), order.clone());
                    thread::spawn(move || {
                        *value.lock().unwrap() = i;
                        refresh_in_order(
                            &order,
                            || *value.lock().unwrap(),
                            |v| {
                                assert!(order.try_lock().is_err(), "投遞時順序鎖必須仍被持有");
                                queue.lock().unwrap().push(v);
                            },
                        );
                    })
                })
                .collect();
            for t in threads {
                t.join().unwrap();
            }
            let last_write = *value.lock().unwrap();
            assert_eq!(queue.lock().unwrap().last().copied(), Some(last_write));
        }
    }

    /// 修正輪 1（審查 low2）：順序保證來自「讀與投遞都在鎖內」。單執行緒下確定性地檢查：讀與投遞
    /// 時鎖都被持有（`try_lock` 失敗），返回後鎖已放開；先讀後投遞、投遞的是讀到的值。改成鎖外讀或
    /// 鎖外投遞都會失敗（並行測試的競態窗口太小，抓不到）。
    #[test]
    fn refresh_in_order_reads_and_posts_while_holding_the_lock() {
        let order = Mutex::new(());
        let log = Mutex::new(Vec::<&'static str>::new());
        refresh_in_order(
            &order,
            || {
                assert!(order.try_lock().is_err(), "讀時順序鎖必須被持有");
                log.lock().unwrap().push("read");
                7u32
            },
            |v| {
                assert!(order.try_lock().is_err(), "投遞時順序鎖必須仍被持有");
                assert_eq!(v, 7);
                log.lock().unwrap().push("post");
            },
        );
        assert_eq!(*log.lock().unwrap(), vec!["read", "post"]);
        assert!(order.try_lock().is_ok(), "返回後順序鎖已放開");
    }

    fn choice(set: IconSet, size: u32) -> IconChoice {
        IconChoice { set, size }
    }

    #[test]
    fn tray_retry_delay_is_bounded() {
        for attempt in 0..TRAY_MAX_RETRIES {
            assert_eq!(
                tray_retry_delay(attempt),
                Some(TRAY_RETRY_INTERVAL),
                "第 {attempt} 次"
            );
        }
        assert_eq!(tray_retry_delay(TRAY_MAX_RETRIES), None);
        assert_eq!(tray_retry_delay(u32::MAX), None);
    }

    /// 修正輪 1（審查 low1）：模擬 explorer 重啟。
    /// 1. 正常套用後重複刷新不重送；
    /// 2. explorer 重啟後（[`explorer_restarted`]）同一張也一定重送一次——tray-icon 重新登記用的是它
    ///    存的那張，不一定是最新的；
    /// 3. explorer 不在時換主題 → 設定失敗、清紀錄、給重試延遲；重試時（explorer 已起來）重送成功；
    /// 4. 次數用完不再建議重試，但之後的 explorer 重啟仍會重送。
    #[test]
    fn explorer_restart_and_failures_force_a_resend() {
        let memo = TrayMemo::new();
        let calls = Mutex::new(Vec::<IconChoice>::new());
        let ok = |c: IconChoice| {
            calls.lock().unwrap().push(c);
            Ok(())
        };
        let a = choice(IconSet::Unified, 24);
        let b = choice(IconSet::Astrolabe, 24);

        assert_eq!(apply_tray_choice(&memo, a, 0, ok), TrayApply::Applied);
        assert_eq!(apply_tray_choice(&memo, a, 0, ok), TrayApply::Skipped);

        explorer_restarted(&memo);
        assert_eq!(
            apply_tray_choice(&memo, a, 0, ok),
            TrayApply::Applied,
            "重啟後必定重送"
        );
        assert_eq!(apply_tray_choice(&memo, a, 0, ok), TrayApply::Skipped);

        let down = |c: IconChoice| {
            calls.lock().unwrap().push(c);
            Err("NIM_MODIFY 失敗".to_owned())
        };
        assert_eq!(
            apply_tray_choice(&memo, b, 0, down),
            TrayApply::Failed {
                error: "NIM_MODIFY 失敗".to_owned(),
                retry_in: Some(TRAY_RETRY_INTERVAL),
            }
        );
        assert_eq!(
            apply_tray_choice(&memo, b, 1, ok),
            TrayApply::Applied,
            "重試重送"
        );

        explorer_restarted(&memo);
        assert!(matches!(
            apply_tray_choice(&memo, b, TRAY_MAX_RETRIES, down),
            TrayApply::Failed { retry_in: None, .. }
        ));
        explorer_restarted(&memo);
        assert_eq!(apply_tray_choice(&memo, b, 0, ok), TrayApply::Applied);

        assert_eq!(
            *calls.lock().unwrap(),
            vec![a, a, b, b, b, b],
            "Skipped 的那幾次不能呼叫 set"
        );
    }

    /// 接線（原始碼檢查，同 `widgets::tests::settings_event_is_emitted_only_through_emit_settings`
    /// 的做法）：所有設定廣播都經 `emit_settings`，故在那裡觸發刷新即涵蓋全部改主題的入口；啟動、
    /// 設定視窗建立、顯示組態與設定視窗縮放變更也要觸發；套用只經 `run_on_main_thread`。
    #[test]
    fn refresh_is_wired_into_every_entry_point() {
        let compact = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
        let call = "crate::app_icon::request_refresh(";

        let widgets = compact(include_str!("widgets.rs"));
        let emit = widgets
            .split("fnemit_settings(")
            .nth(1)
            .expect("widgets.rs 有 emit_settings");
        let emit_body = &emit[..emit.find("///").unwrap_or(emit.len())];
        assert!(
            emit_body.contains(call),
            "emit_settings 本體沒有呼叫 {call}"
        );

        let main = compact(include_str!("main.rs"));
        assert!(
            main.contains("app_icon::request_refresh(app.handle())"),
            "main.rs setup 沒有在啟動時刷新"
        );
        assert!(
            main.matches("app_icon::request_refresh(").count() >= 3,
            "main.rs 應在 setup、relayout_all、設定視窗 ScaleFactorChanged 三處刷新"
        );
        // 修正輪 1：explorer 重新啟動的分支要作廢去重並刷新。
        let explorer_arm = main
            .split("desktop::SystemEvent::ExplorerRestarted=>{")
            .nth(1)
            .expect("main.rs 有 ExplorerRestarted 分支");
        let explorer_arm = &explorer_arm[..explorer_arm.find('}').unwrap_or(explorer_arm.len())];
        assert!(
            explorer_arm.contains("app_icon::on_explorer_restarted(&gatekeeper_handle_e)"),
            "ExplorerRestarted 分支沒有刷新圖示"
        );

        let tray = compact(include_str!("tray.rs"));
        let create = tray
            .split("fncreate_settings_window(")
            .nth(1)
            .expect("tray.rs 有 create_settings_window");
        let create_body = &create[..create.find("fntoggle_manual_pause").unwrap_or(create.len())];
        assert!(
            create_body.contains(call),
            "create_settings_window 建好視窗後沒有刷新圖示"
        );

        let me = compact(include_str!("app_icon.rs"));
        let refresh = me
            .split("fnrefresh_now(")
            .nth(1)
            .expect("refresh_now")
            .split("fnapply_on_main_thread(")
            .next()
            .unwrap();
        assert!(
            refresh.contains("run_on_main_thread(move||apply_on_main_thread("),
            "套用必須經 run_on_main_thread"
        );
        // 修正輪 1：系統匣設定失敗時依建議延遲重試；explorer 重啟入口先作廢再刷新。
        let apply = me
            .split("fnapply_on_main_thread(")
            .nth(1)
            .expect("apply_on_main_thread")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        assert!(
            apply.contains("apply_tray_choice(&TRAY_MEMO,")
                && apply.contains("spawn_refresh(app,attempt+1,Some(delay))"),
            "系統匣失敗沒有排程重試"
        );
        let on_restart = me
            .split("pubfnon_explorer_restarted(")
            .nth(1)
            .expect("on_explorer_restarted")
            .split("///")
            .next()
            .unwrap();
        assert!(
            on_restart.contains("explorer_restarted(&TRAY_MEMO);request_refresh(app);"),
            "on_explorer_restarted 要先作廢去重再刷新"
        );
    }
}
