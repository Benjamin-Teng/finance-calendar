//! 設定模型（design.md D7、D9、D12；specs/widget-host-lifecycle「設定持久化」「自動暫停」
//! 「開機自動啟動」；specs/widget-host-windows「編輯版面」「多螢幕與 DPI 定位」「預設版面」）。
//!
//! 本檔提供：
//! - [`Settings`]：`version`、外觀模式、透明度、主題色、顯示除權息、資料目錄、十個小工具的
//!   開關／[`WidgetPlacement`]（所屬顯示器＋48×48 格線座標）、[`PauseRules`]、版面鎖定、
//!   自動啟動、桌布主題 [`WallpaperTheme`]（dynamic-wallpaper task 4.1；預設不接管，舊檔缺此
//!   欄位讀成不接管、不重寫；值不認得時只有這個欄位退回不接管並記警告）。
//! - [`Settings::default`]：全新安裝（無設定檔）時的預設值；預設格座標見
//!   [`DEFAULT_GRID_RECTS`]（task 7.2）。
//! - [`load_for_startup`]（測試另用 `load_or_default`，同一套讀取邏輯）：讀取
//!   `%APPDATA%\tw.fintools.fc-host\settings.json`
//!   （[`default_settings_path`]）。版本判斷在補齊預設值**之前**、對原始 JSON 做（design.md
//!   D9）：缺 `version` 或 `version < 2`、內容無法解析（損壞）時，把原檔備份為
//!   `settings.json.bad-<Unix 秒>` 後以預設值啟動；可解析的 v2 檔案缺欄位以預設值補齊，
//!   載入後格座標超界的小工具在記憶體中退回自己的預設格座標。此函式不會失敗——I/O 或備份
//!   失敗一律靜默忽略、回傳可用的 `Settings`，確保宿主一定能啟動（存檔錯誤另交呼叫端記錄）。
//!   缺檔時把預設值落地；損壞檔備份成功後也立刻存回預設值，下一次啟動不必再處理同一份損壞檔。
//! - [`save`]：原子寫入（暫存檔＋`fs::rename`）。
//! - [`merge_patch`]：把任意 JSON 物件遞迴合併進目前設定（核心內部用，例如版面鎖定）。
//! - [`merge_user_patch`]：`update_settings(patch)` IPC 指令用的合併——先拒收 `version` 與任何
//!   `placement` 欄位（design.md D7：版面只能經編輯版面放開與開啟小工具找空位兩條路徑改），
//!   再走 [`merge_patch`]。
//!
//! 版面欄位只放 Rust（design.md D6）：設計寬度與設計最小高度在 `crate::widgets` 的規格表，
//! 預設格座標在本檔；前端 `host/ui/registry.js` 只列 id 與通道。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::layout::{in_grid_bounds, GridRect};

/// 目前設定檔結構版本（design.md D9：2＝格線模型；1＝已移除的錨點模型，不遷移）。
pub const SETTINGS_VERSION: u32 = 2;

/// 十個小工具的固定 id（design.md D6）。順序與 `crate::widgets::WIDGET_SPECS`、前端
/// `host/ui/registry.js` 相同（兩處皆有單元測試核對）。
pub const WIDGET_IDS: [&str; 10] = [
    "clock", "macro", "fixed", "dynamic", "quotes", "custom1", "custom2", "custom3", "custom4",
    "custom5",
];

/// 十個小工具的預設格座標（design.md D6／D7；specs/widget-host-windows「預設版面」），順序同
/// [`WIDGET_IDS`]。財經五個為 tasks.md 7.2 的初值，排列近似 Lively 版儀表板：中右欄＝時鐘＋
/// 總經日曆、最右欄＝台股固定＋動態事件、行情條橫貫其下；左側 col 0–14 留白給桌面圖示。
/// 時鐘高 10 格：headless 量測時鐘 `.panel` 的 `scrollHeight`＝137（border box 139.19）
/// CSS 像素，面板最小高度取 140，加上下 `--widget-gap` 後設計最小高度 156（fix min-height）。
/// 時鐘寬 16 格時可用設計高度＝500 × h ÷ 16 × 高寬比：高寬比 0.53 下 h=9 只有約 149（< 156，
/// 4K＠150% 的工作區 3840×2088 也只有約 153），h=10 約 166，故加高一格；總經日曆隨之下移
/// 一格（row 12、高 30），仍與時鐘、行情條各空一列（量測方法見 task-7.2-report.md）。五個擴充插槽預設關閉，放在左側 col 0、寬 14、高 8，
/// 上下各空一格、與時鐘／總經日曆欄（col 15 起）空一欄，互不相交。寬度取 14 而非 controller
/// 補充定值的 13：13 格在 1680×1050＠200%（邏輯寬 840）只有 227.5 邏輯像素，低於設計寬度
/// 470 × 0.5 = 235，違反「高寬比 ≥ 0.53 不小於最小格數」；14 格為 245，通過。
///
/// 兩兩不相交、全部在界內，以及「高寬比 ≥ 0.53 的工作區都不小於最小格數」由
/// `settings::tests`、`widgets::tests`、`layout::grid_tests` 的屬性測試守住。
pub const DEFAULT_GRID_RECTS: [GridRect; 10] = [
    grid(15, 1, 16, 10),  // clock
    grid(15, 12, 16, 30), // macro
    grid(32, 1, 15, 17),  // fixed
    grid(32, 19, 15, 23), // dynamic
    grid(15, 43, 32, 4),  // quotes
    grid(0, 1, 14, 8),    // custom1
    grid(0, 10, 14, 8),   // custom2
    grid(0, 19, 14, 8),   // custom3
    grid(0, 28, 14, 8),   // custom4
    grid(0, 37, 14, 8),   // custom5
];

const fn grid(col: i32, row: i32, w: i32, h: i32) -> GridRect {
    GridRect { col, row, w, h }
}

/// 預設開啟的小工具：五個財經小工具（specs/widget-host-windows「預設版面」）。
const DEFAULT_ENABLED: [&str; 5] = ["clock", "macro", "fixed", "dynamic", "quotes"];

/// 依 id 查預設格座標；未知 id 回傳 `None`。
pub fn default_grid_rect(id: &str) -> Option<GridRect> {
    WIDGET_IDS
        .iter()
        .position(|known| *known == id)
        .map(|i| DEFAULT_GRID_RECTS[i])
}

/// 小工具外觀模式（specs/widget-host-windows「小工具外觀模式」）。實際在毛玻璃不可用時
/// 降級為純色是執行期判定（design.md D8、task 2.8 的純函式），不在此結構內。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AppearanceMode {
    /// 毛玻璃（系統原生 Acrylic）。
    #[default]
    Acrylic,
    /// 純色（半透明深色、無模糊）。
    Solid,
}

/// 顯示器識別（design.md D9）。刻意用列舉而非「空字串代表主螢幕」的隱性慣例：空字串在
/// 型別上仍是合法的 `Device` 值，容易與「真的找不到裝置路徑」混淆；列舉讓兩者在型別層級
/// 就分開。
///
/// - `Primary`：主螢幕。全新設定檔的預設值，跟隨目前主螢幕。
/// - `Device(String)`：`DisplayConfigGetDeviceInfo` 取得的 `monitorDevicePath`（穩定識別，
///   **不含**解析度、**不用** `\\.\DISPLAYn`）。記錄的顯示器目前找不到時暫時退回主螢幕，
///   不改寫記錄，顯示器重新連接後復位（`crate::layout::resolve_grid_placements`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MonitorId {
    #[default]
    Primary,
    Device(String),
}

/// 小工具記錄位置（design.md D9）：所屬顯示器＋48×48 格線座標（整數格，佔用
/// `[col, col+w) × [row, row+h)`）。大小由格數決定、比例由格線決定，沒有縮放欄位。
/// 實際位置由 `crate::layout::resolve_grid_placements` 從記錄位置＋目前顯示器清單推出。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WidgetPlacement {
    pub monitor: MonitorId,
    pub col: i32,
    pub row: i32,
    pub w: i32,
    pub h: i32,
}

impl WidgetPlacement {
    /// 主螢幕上的指定格子。
    pub fn primary(rect: GridRect) -> Self {
        WidgetPlacement {
            monitor: MonitorId::Primary,
            col: rect.col,
            row: rect.row,
            w: rect.w,
            h: rect.h,
        }
    }

    /// 格線座標部分。
    pub fn grid_rect(&self) -> GridRect {
        GridRect {
            col: self.col,
            row: self.row,
            w: self.w,
            h: self.h,
        }
    }
}

impl Default for WidgetPlacement {
    /// 型別層級的預設值（主螢幕左上角 1×1）。只在沒有 id 可查的情況使用；有 id 時一律用
    /// [`DEFAULT_GRID_RECTS`] 的專屬預設（載入與合併都先在 JSON 層疊上 [`Settings::default`]，
    /// 見 [`parse_with_defaults`]）。
    fn default() -> Self {
        WidgetPlacement::primary(grid(0, 0, 1, 1))
    }
}

/// 單一小工具的設定：開關＋記錄位置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct WidgetConfig {
    pub enabled: bool,
    pub placement: WidgetPlacement,
}

/// 自動暫停的可設定項目（specs/widget-host-lifecycle「自動暫停」）。全螢幕／忙碌／鎖定／
/// 顯示器關閉／手動暫停這五種情況一律暫停、不可關閉，故不在此結構內；只有「使用電池時」與
/// 「省電模式時」是使用者可調整的規則。兩者預設皆關閉——spec 列出的五種強制暫停情況已涵蓋
/// 主要場景，這兩項屬於更保守的額外選項，預設不開啟以免過度暫停（行情條動畫本身耗資源
/// 極低）；此為本 task 的判斷，非 spec 明文預設值，controller 覆核時可調整。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PauseRules {
    pub pause_on_battery: bool,
    pub pause_on_power_saver: bool,
}

/// 桌布主題（dynamic-wallpaper；specs/desktop-wallpaper「桌布主題選擇」）。序列化為穩定的英文字串，
/// 與 `host/ui/wallpapers/<字串>.html` 的檔名一致（`none` 除外，它不對應任何頁面）。
///
/// 預設 [`WallpaperTheme::None`]（「不接管」）：新安裝與舊版設定檔（沒有這個欄位）一律讀成
/// 不接管，宿主不改任何螢幕的桌布（design.md Migration Plan 第 2 點）。設定檔裡的值不認得時，
/// 讀檔端只把**這個欄位**退回不接管並記警告，其他欄位照常載入（[`sanitize_wallpaper_theme`]）；
/// `update_settings` 的 patch 帶不認得的值則照 serde 嚴格拒收。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WallpaperTheme {
    /// 不接管（宿主不改桌布）。
    #[default]
    None,
    /// 星盤。
    Astrolabe,
    /// 撕日曆。
    Tearoff,
    /// 脊線。
    Ridgeline,
    /// 等高線。
    Contour,
    /// 天際線。
    Skyline,
}

impl WallpaperTheme {
    /// 全部六個值（含不接管），順序同設定視窗選單。
    pub const ALL: [WallpaperTheme; 6] = [
        WallpaperTheme::None,
        WallpaperTheme::Astrolabe,
        WallpaperTheme::Tearoff,
        WallpaperTheme::Ridgeline,
        WallpaperTheme::Contour,
        WallpaperTheme::Skyline,
    ];

    /// 序列化字串（與 serde 相同；非 `none` 者＝`host/ui/wallpapers/<字串>.html` 的檔名主幹）。
    pub fn as_str(self) -> &'static str {
        match self {
            WallpaperTheme::None => "none",
            WallpaperTheme::Astrolabe => "astrolabe",
            WallpaperTheme::Tearoff => "tearoff",
            WallpaperTheme::Ridgeline => "ridgeline",
            WallpaperTheme::Contour => "contour",
            WallpaperTheme::Skyline => "skyline",
        }
    }
}

/// 市場資料抓取開關（data-layer-rust tasks.md 5.3、design.md D10；specs/market-data-fetch「抓取開關與隔離環境」）。
///
/// - `Auto`（預設）：環境變數 `LOCALAPPDATA` 與系統登記的本機應用程式資料夾不同（驗收腳本以暫存資料夾隔離宿主）
///   就視同 `Off` 並在記錄檔寫明原因，否則視同 `On`；
/// - `On`：一律抓取；
/// - `Off`：不發任何抓取請求、不寫 `tw_events.json`，只讀資料目錄中已有的檔案。
///
/// 舊設定檔沒有這個欄位時由 `#[serde(default)]` 補成 `Auto`，不改 `SETTINGS_VERSION`、不觸發重寫；
/// 檔案裡的值不認得時，讀檔端只把**這個欄位**視為 `Auto` 並記警告（[`sanitize_data_fetch`]）；
/// `update_settings` 的 patch 帶不認得的值則照 serde 嚴格拒收。排程每次醒來讀目前值，所以變更即時生效。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DataFetch {
    /// 依隔離偵測決定（預設）。
    #[default]
    Auto,
    /// 一律抓取。
    On,
    /// 不抓取。
    Off,
}

impl DataFetch {
    /// 全部三個值，順序同文件。
    pub const ALL: [DataFetch; 3] = [DataFetch::Auto, DataFetch::On, DataFetch::Off];

    /// 序列化字串（與 serde 相同）。
    pub fn as_str(self) -> &'static str {
        match self {
            DataFetch::Auto => "auto",
            DataFetch::On => "on",
            DataFetch::Off => "off",
        }
    }
}

/// [`Settings::data_fetch`] 在設定檔中的鍵名。
const DATA_FETCH_KEY: &str = "data_fetch";

/// [`Settings::wallpaper_theme`] 在設定檔中的鍵名。
const WALLPAPER_THEME_KEY: &str = "wallpaper_theme";

/// 宿主設定（specs/widget-host-lifecycle「設定持久化」）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    pub appearance_mode: AppearanceMode,
    /// 面板不透明度 0.0–1.0。
    pub opacity: f64,
    /// 強調色，`#RRGGBB`。
    pub accent_color: String,
    /// 顯示除權息（台股動態事件預告清單是否納入除權息場次）。
    pub show_dividend: bool,
    pub data_dir: PathBuf,
    /// 十個小工具的開關與記錄位置，鍵為 [`WIDGET_IDS`] 的 id。
    pub widgets: BTreeMap<String, WidgetConfig>,
    pub pause_rules: PauseRules,
    /// 版面鎖定（specs/widget-host-windows「編輯版面」：預設鎖定，鎖定時無法拖曳）。
    pub layout_locked: bool,
    /// 登入時自動啟動。開機自啟登錄由安裝檔寫入與移除（design.md D12）；本欄位只是讀不到登錄時
    /// 的退回值——設定視窗看到的、[`crate::widgets::get_settings`] 回傳的一律是登錄實況
    /// （`crate::widgets::with_registered_autostart`），使用者切換時才由
    /// `crate::widgets::sync_autostart` 寫入登錄（task 5.4）。
    pub autostart: bool,
    /// 桌布主題（預設不接管）。舊版設定檔沒有這個欄位時由 `#[serde(default)]` 補成不接管，
    /// 不觸發重寫；值不認得時見 [`sanitize_wallpaper_theme`]。
    pub wallpaper_theme: WallpaperTheme,
    /// 市場資料抓取開關（見 [`DataFetch`]）。舊版設定檔沒有這個欄位時補成 `auto`、不觸發重寫。
    pub data_fetch: DataFetch,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            version: SETTINGS_VERSION,
            appearance_mode: AppearanceMode::default(),
            // finance-calendar.html CONFIG.panelOpacity（v6.2）。
            opacity: 0.55,
            // finance-calendar.html CONFIG.accentColor（v6.2）。
            accent_color: "#e0aa54".to_string(),
            // finance-calendar.html CONFIG.dynTypes 預設不含 'dividend'（v6.2）。
            show_dividend: false,
            data_dir: default_data_dir(),
            widgets: default_widgets(),
            pause_rules: PauseRules::default(),
            // specs/widget-host-windows「編輯版面」：版面 SHALL 預設為鎖定狀態。
            layout_locked: true,
            // specs/widget-host-lifecycle「開機自動啟動」：預設開啟（＝預期已由安裝檔登錄）。
            autostart: true,
            // specs/desktop-wallpaper「桌布主題選擇」：新安裝 SHALL 為「不接管」。
            wallpaper_theme: WallpaperTheme::None,
            // specs/market-data-fetch「抓取開關與隔離環境」：預設 `auto`。
            data_fetch: DataFetch::Auto,
        }
    }
}

/// 十個小工具的預設設定：開關見 [`DEFAULT_ENABLED`]，位置＝主螢幕＋[`DEFAULT_GRID_RECTS`]。
fn default_widgets() -> BTreeMap<String, WidgetConfig> {
    WIDGET_IDS
        .iter()
        .zip(DEFAULT_GRID_RECTS)
        .map(|(id, rect)| {
            (
                (*id).to_string(),
                WidgetConfig {
                    enabled: DEFAULT_ENABLED.contains(id),
                    placement: WidgetPlacement::primary(rect),
                },
            )
        })
        .collect()
}

/// 資料目錄預設值：`%LOCALAPPDATA%\tw.fintools.fc-host\data`（design.md D5）。
/// 環境變數不存在（極端情況）時退回目前工作目錄，不 panic。
fn default_data_dir() -> PathBuf {
    let base = env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("tw.fintools.fc-host").join("data")
}

/// 設定檔預設路徑：`%APPDATA%\tw.fintools.fc-host\settings.json`（design.md D12）。
/// 環境變數不存在（極端情況）時退回目前工作目錄，不 panic。
pub fn default_settings_path() -> PathBuf {
    let base = env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("tw.fintools.fc-host").join("settings.json")
}

/// 置底守門記錄檔預設路徑：`%APPDATA%\tw.fintools.fc-host\gatekeeper.log`（design.md D8
/// 保險檢查、task-3.3-brief：宿主端加重排次數的記錄，供驗收判讀「explorer 重啟／人為介入
/// 30 秒內恢復」與「平常 10 分鐘內無任何重排」）。與 [`default_settings_path`] 同一命名空間
/// 目錄，環境變數不存在時同樣退回目前工作目錄，不 panic。
pub fn default_gatekeeper_log_path() -> PathBuf {
    let base = env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("tw.fintools.fc-host").join("gatekeeper.log")
}

/// 讀取設定檔；檔案不存在（首次啟動）或欄位缺漏時以預設值補齊；內容無法解析（損壞）、缺
/// `version` 或 `version < 2`（舊版錨點模型，不遷移）時把原檔備份為 `<檔名>.bad-<Unix 秒>`
/// 後以預設值啟動（design.md D9、D12；specs/widget-host-lifecycle「設定持久化」）。
///
/// 不會失敗：讀檔、解析、備份任何一步出錯都只忽略錯誤，一律回傳可用的 [`Settings`]，
/// 確保宿主一定能啟動（即使設定目錄本身有權限問題）。
///
/// 宿主啟動走 [`load_for_startup`]（同一套讀取邏輯＋必要時立刻存檔）；本函式只留給測試。
#[cfg(test)]
pub fn load_or_default(path: &Path) -> Settings {
    load_with_outcome(path).0
}

/// 讀取設定檔走了哪條路（[`load_for_startup`] 據此決定要不要存檔）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoadOutcome {
    /// 正常載入（含缺欄位補預設值）。
    Loaded,
    /// 檔案不存在（首次啟動）。
    Missing,
    /// 其他讀取失敗（權限、鎖定）：內容未知，不能覆寫。
    Unreadable,
    /// 損壞或版本不符，原檔已改名備份、現以預設值啟動。
    ResetAfterBackup,
    /// 損壞或版本不符，但備份失敗（原檔還在原位）：不能覆寫，否則原內容就沒了。
    CorruptNotBackedUp,
}

fn load_with_outcome(path: &Path) -> (Settings, LoadOutcome) {
    // 讀原始位元組而非 `read_to_string`：非 UTF-8 內容（位元組損壞、誤存為 UTF-16）在
    // `read_to_string` 會變成 I/O 錯誤 `InvalidData`，與「檔案不存在」混在同一個 `Err` 裡被
    // 當成首次啟動、跳過備份（fix round 1，Codex 2.3-b）。改由 `from_slice` 解析，編碼錯誤與
    // JSON 語法錯誤一律走下方的損壞備份路徑。
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        // 檔案不存在＝首次啟動，不算「損壞」，不需要備份，直接用預設值。其他讀取失敗
        // （權限、鎖定）內容未知，也不能判定為損壞而搬走，同樣直接用預設值。
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return (Settings::default(), LoadOutcome::Missing)
        }
        Err(_) => return (Settings::default(), LoadOutcome::Unreadable),
    };

    match parse_with_defaults(&bytes) {
        Ok(settings) => (settings, LoadOutcome::Loaded),
        Err(_) => {
            let outcome = if backup_corrupt_file(path) {
                LoadOutcome::ResetAfterBackup
            } else {
                LoadOutcome::CorruptNotBackedUp
            };
            (Settings::default(), outcome)
        }
    }
}

/// 宿主啟動時讀設定的結果（[`load_for_startup`]）。
#[derive(Debug)]
pub struct StartupSettings {
    pub settings: Settings,
    /// 本次啟動該存檔卻存不進去時的錯誤（供呼叫端記錄）。
    pub save_error: Option<std::io::Error>,
}

/// 宿主啟動用的讀設定（[`load_or_default`]＋必要時立刻存檔）：
///
/// - 缺檔（首次啟動）：把預設值落地；存檔失敗時記下錯誤（下次啟動再試）。
/// - 損壞或版本不符且原檔已備份（fix F2b，重審 low）：立刻把預設值存回去，下一次啟動不必再
///   處理同一份損壞檔。
/// - 其他（正常載入、讀不到、損壞但備份失敗）：不存檔——內容未知或原檔仍在原位，覆寫會毀掉它。
///
/// 使用者決定（2026-10-01）：首次建立設定檔**不**寫開機自啟登錄（登錄歸安裝檔），故本函式
/// 不再回報「是否首次建立」。
pub fn load_for_startup(path: &Path) -> StartupSettings {
    let (settings, outcome) = load_with_outcome(path);
    let save_error = match outcome {
        LoadOutcome::Missing | LoadOutcome::ResetAfterBackup => save(path, &settings).err(),
        LoadOutcome::Loaded | LoadOutcome::Unreadable | LoadOutcome::CorruptNotBackedUp => None,
    };
    StartupSettings {
        settings,
        save_error,
    }
}

/// fix F3（review task-5.1-r-opus.md [low]）：依啟動仲裁結果讀設定。`may_write`＝仲裁沒有判定
/// 本行程落敗（`desktop::StartupRole::may_write_startup_state`：Primary，或仲裁機制故障而退回
/// 只靠 plugin 的 Unarbitrated）——只有這時才走 [`load_for_startup`]（首次建立落地、損壞檔備份
/// 並存回預設值）。
///
/// 落敗的 Secondary 只會交接給既有執行個體後結束，**不得寫任何東西**：改用 [`load_read_only`]
/// ——不建立、不備份（改名）、不覆寫設定檔。否則首次啟動時 Primary 與 Secondary 會各自寫預設
/// 設定檔，Secondary 還可能把 Primary 剛要讀的損壞檔搬走。
pub fn load_for_arbitrated_startup(path: &Path, may_write: bool) -> StartupSettings {
    if may_write {
        return load_for_startup(path);
    }
    StartupSettings {
        settings: load_read_only(path),
        save_error: None,
    }
}

/// 唯讀讀取設定檔（[`load_for_arbitrated_startup`] 的 Secondary 分支）：與 [`load_with_outcome`]
/// 同一套解析，但缺檔、讀不到、損壞或版本不符一律只回傳預設值，**不備份、不存檔**。
fn load_read_only(path: &Path) -> Settings {
    fs::read(path)
        .ok()
        .and_then(|bytes| parse_with_defaults(&bytes).ok())
        .unwrap_or_default()
}

/// 解析設定檔內容：版本判斷 → 疊在 [`Settings::default`] 的 JSON 上 → 反序列化 → 補齊缺漏
/// 的小工具 → 格座標範圍驗證。`Err` 的內容只供除錯，呼叫端一律走備份路徑。
///
/// 版本判斷必須在 [`merge_json`] **之前**、對原始 JSON 做（design.md D9）：合併後缺少的
/// `version` 會被預設值補成 [`SETTINGS_VERSION`]，舊檔就會被誤當成新版。
///
/// 為何先在 JSON 層合併而不直接 `from_slice::<Settings>`（fix round 1，Codex 2.3-a）：
/// `#[serde(default)]` 對巢狀結構套用的是**型別**的預設值——`{"widgets":{"clock":{}}}` 的
/// clock 會得到 `WidgetConfig::default()`（關閉、左上角 1×1），而不是 clock 自己的預設；
/// 先以 [`merge_json`] 合併到完整的預設值上，每個既有小工具的缺漏子欄位就會帶入該 id 的
/// 預設值，明確提供的值則保留。檔案裡值為 `null` 的鍵依 RFC 7396 視為缺漏（補預設值）。
///
/// 合併前先以 [`sanitize_wallpaper_theme`] 移除不認得的桌布主題值（記警告），避免單一欄位讓整份
/// 設定反序列化失敗而被備份重設（dynamic-wallpaper task 4.1）。
fn parse_with_defaults(bytes: &[u8]) -> Result<Settings, String> {
    let mut file_value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if !is_current_version(&file_value) {
        return Err(format!(
            "設定檔版本不符（需要 version >= {SETTINGS_VERSION}）：{:?}",
            file_value.get("version")
        ));
    }
    if let Some(warning) = sanitize_wallpaper_theme(&mut file_value) {
        log::warn!("{warning}");
    }
    if let Some(warning) = sanitize_data_fetch(&mut file_value) {
        log::warn!("{warning}");
    }
    let mut base = serde_json::to_value(Settings::default()).map_err(|e| e.to_string())?;
    merge_json(&mut base, &file_value);
    let mut settings: Settings = serde_json::from_value(base).map_err(|e| e.to_string())?;
    backfill_widgets(&mut settings);
    reset_out_of_range_grids(&mut settings);
    Ok(settings)
}

/// 原始 JSON 的 `version` 是否為 ≥ [`SETTINGS_VERSION`] 的非負整數。缺少、`null`、字串、
/// 小數或頂層不是物件一律視為否（design.md D9：「缺 `version` 或 `version < 2` 視同損壞」；
/// 無法判讀的版本同樣不冒險合併）。比目前版本新的檔案照常載入，未知欄位由 serde 忽略。
fn is_current_version(file_value: &serde_json::Value) -> bool {
    file_value
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .is_some_and(|v| v >= u64::from(SETTINGS_VERSION))
}

/// 補齊 `widgets` 內缺漏的小工具 id（container 層級的 `#[serde(default)]` 只處理整個
/// `widgets` 欄位「完全不存在」的情況；若設定檔裡的 `widgets` 物件存在但只列出部分 id，
/// 需要在這裡逐一補上缺的，使用該 id 專屬的預設值，而非泛用的 `WidgetConfig::default()`）。
/// 已存在的 id 不動。
fn backfill_widgets(settings: &mut Settings) {
    let defaults = default_widgets();
    for id in WIDGET_IDS {
        settings
            .widgets
            .entry(id.to_string())
            .or_insert_with(|| defaults[id].clone());
    }
}

/// 載入後逐一驗證格座標範圍（design.md D9：`w、h ≥ 1`、`col + w ≤ 48`、`row + h ≤ 48`，
/// 另含 `col、row ≥ 0`，即 [`in_grid_bounds`]）：不合法的那個小工具在記憶體中退回它的預設
/// 格座標（`monitor` 不是格座標，保留原值），下一次任何存檔時一併寫回。不認得的 id（設定檔裡
/// 多出來的鍵）沒有預設值可退，原樣保留——視窗管理只依 [`WIDGET_IDS`] 運作，不會用到它們。
fn reset_out_of_range_grids(settings: &mut Settings) {
    for (id, config) in settings.widgets.iter_mut() {
        if in_grid_bounds(config.placement.grid_rect()) {
            continue;
        }
        if let Some(rect) = default_grid_rect(id) {
            let placement = &mut config.placement;
            placement.col = rect.col;
            placement.row = rect.row;
            placement.w = rect.w;
            placement.h = rect.h;
        }
    }
}

/// 讀檔端的主題欄位防護：原始 JSON 的 `wallpaper_theme` 存在、不是 `null`、又不是認得的字串
/// （拼錯、大小寫不符、數字、物件……）時，把**這個鍵**移除並回傳警告訊息；之後的合併會以預設值
/// （不接管）補上。缺鍵與 `null`（RFC 7396 視為缺漏）不算錯，回傳 `None` 且不改動輸入。
///
/// 必須在反序列化成 [`Settings`] 之前做：否則 serde 對未知列舉值回 `Err`，整份設定會被當成損壞
/// 備份並重設，使用者其他欄位全部遺失（task 4.1 controller 裁定：只把主題退回不接管）。
fn sanitize_wallpaper_theme(file_value: &mut serde_json::Value) -> Option<String> {
    let map = file_value.as_object_mut()?;
    let raw = map.get(WALLPAPER_THEME_KEY)?;
    if raw.is_null() || serde_json::from_value::<WallpaperTheme>(raw.clone()).is_ok() {
        return None;
    }
    let warning = format!(
        "設定檔的 {WALLPAPER_THEME_KEY} 值 {raw} 不認得（可用值：{}），本次以「不接管」（none）載入",
        WallpaperTheme::ALL.map(WallpaperTheme::as_str).join("、")
    );
    map.remove(WALLPAPER_THEME_KEY);
    Some(warning)
}

/// 讀檔端的抓取開關防護：原始 JSON 的 `data_fetch` 存在、不是 `null`、又不是認得的字串（`auto`／`on`／`off`，
/// 大小寫與拼字須完全相符）時，把**這個鍵**移除並回傳警告訊息；之後的合併以預設值（`auto`）補上
/// （design.md D10：不認得的值視為 `auto` 並記錄）。缺鍵與 `null` 不算錯，回傳 `None` 且不改動輸入。
/// 必須在反序列化之前做，否則 serde 對未知列舉值回 `Err`、整份設定被當成損壞備份重設。
fn sanitize_data_fetch(file_value: &mut serde_json::Value) -> Option<String> {
    let map = file_value.as_object_mut()?;
    let raw = map.get(DATA_FETCH_KEY)?;
    if raw.is_null() || serde_json::from_value::<DataFetch>(raw.clone()).is_ok() {
        return None;
    }
    let warning = format!(
        "設定檔的 {DATA_FETCH_KEY} 值 {raw} 不認得（可用值：{}），本次視為 auto",
        DataFetch::ALL.map(DataFetch::as_str).join("、")
    );
    map.remove(DATA_FETCH_KEY);
    Some(warning)
}

/// 把無法解析（或版本不符）的設定檔改名備份為 `<檔名>.bad-<Unix 秒>`，供事後查看原內容。
/// 備份失敗（例如權限問題）只回傳 `false`，呼叫端仍會以預設值啟動。
fn backup_corrupt_file(path: &Path) -> bool {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("settings.json");
    let backup = path.with_file_name(format!("{file_name}.bad-{ts}"));
    fs::rename(path, backup).is_ok()
}

/// 把 [`Settings`] 寫回 `path`（specs/widget-host-lifecycle「設定持久化」：修改後立即生效並在
/// 重啟後保留）。原子寫入：先寫到同目錄下的暫存檔（`<檔名>.tmp`），成功後 `fs::rename` 換上——
/// 與 AGENTS.md 資料層既有的「原子寫入」慣例一致，避免寫到一半當機或並發讀取看到半份檔案。
/// 目錄不存在時先建立（首次啟動、使用者從未存過設定的情況）。
pub fn save(path: &Path, settings: &Settings) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(settings)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    write_atomic(path, text.as_bytes())
}

/// 原子寫入任意位元組（[`save`] 與 `crate::wallpaper_config` 共用）：目錄不存在時先建立，
/// 寫到同目錄的 `<檔名>.tmp` 後 `fs::rename` 換上，讀者不會看到半份檔案。
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("settings.json");
    let tmp = path.with_file_name(format!("{file_name}.tmp"));
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)
}

/// 把 `base` 依 RFC 7396 JSON Merge Patch 語意遞迴合併 `patch`：兩邊都是物件時逐鍵合併
/// （`patch` 裡值為 `null` 的鍵視為「刪除該鍵」，其餘鍵遞迴合併），其他情況（型別不同、非
/// 物件）`patch` 的值直接整個取代 `base`（含陣列——本專案的設定裡沒有陣列欄位，但語意上與
/// RFC 7396 對齊，不特別處理）。
fn merge_json(base: &mut serde_json::Value, patch: &serde_json::Value) {
    use serde_json::Value;
    match (base, patch) {
        (Value::Object(base_map), Value::Object(patch_map)) => {
            for (key, patch_value) in patch_map {
                if patch_value.is_null() {
                    base_map.remove(key);
                } else {
                    merge_json(
                        base_map.entry(key.clone()).or_insert(Value::Null),
                        patch_value,
                    );
                }
            }
        }
        (base_slot, patch_value) => {
            *base_slot = patch_value.clone();
        }
    }
}

/// 把 `patch`（任意 JSON 物件，可以只給想改的欄位，物件型欄位如 `widgets.clock` 也可以只給
/// 想改的子欄位）依 [`merge_json`] 合併進 `current` 的序列化結果，再反序列化回 [`Settings`]。
/// 核心內部用（例如 `crate::widgets` 同步版面鎖定）；使用者的 `update_settings` 走
/// [`merge_user_patch`]。
///
/// 反序列化失敗（`patch` 帶了型別對不上的值，例如把 `opacity` 設成字串）回傳 `Err`，呼叫端
/// 據此判斷要不要落地存檔與廣播——合併失敗時原設定完全不受影響。
///
/// 成功時額外呼叫 [`backfill_widgets`]：`patch` 若把 `widgets` 底下某個 id 整個刪成
/// `null`（或合併後該 id 因故消失），須補回該 id 自己的預設值，維持 `Settings::widgets`
/// 恆為十個小工具都有項目的不變量（與 [`load_or_default`] 對讀檔的保證一致）。
pub fn merge_patch(current: &Settings, patch: &serde_json::Value) -> serde_json::Result<Settings> {
    let mut base = serde_json::to_value(current)?;
    merge_json(&mut base, patch);
    let mut merged: Settings = serde_json::from_value(base)?;
    backfill_widgets(&mut merged);
    Ok(merged)
}

/// `update_settings(patch)` IPC 指令（design.md D4）的合併：先拒收 `version` 欄位（fix F1，
/// review 7.2 L：寫成舊版本號會讓下次啟動把整份設定備份並重設），再以 [`reject_placement_patch`]
/// 拒收任何會改到版面的 patch（design.md D7：`update_settings` 拒收 `placement`，版面只能經
/// 編輯版面放開與開啟小工具找空位兩條路徑改），再走 [`merge_patch`]。任一步失敗回傳 `Err`
/// （錯誤訊息直接回給前端），`current` 不受影響。
pub fn merge_user_patch(current: &Settings, patch: &serde_json::Value) -> Result<Settings, String> {
    if patch.get("version").is_some() {
        return Err("update_settings 不接受 version 欄位（設定結構版本只由核心決定）".to_string());
    }
    reject_placement_patch(patch)?;
    merge_patch(current, patch).map_err(|e| e.to_string())
}

/// `patch` 是否可能改到任何小工具的記錄位置：`widgets` 存在時必須是物件、每個小工具項目也
/// 必須是物件且不含 `placement` 鍵（值為 `null` 也算，RFC 7396 的 `null` 會刪鍵後由預設值補
/// 回，一樣改到版面）。整個 `widgets` 或某個小工具項目設成 `null` 同理（[`backfill_widgets`]
/// 會補回預設格座標），一律拒絕。
fn reject_placement_patch(patch: &serde_json::Value) -> Result<(), String> {
    let Some(widgets) = patch.get("widgets") else {
        return Ok(());
    };
    let Some(widgets) = widgets.as_object() else {
        return Err("update_settings 不接受整個 widgets 欄位的取代或刪除".to_string());
    };
    for (id, entry) in widgets {
        let Some(entry) = entry.as_object() else {
            return Err(format!(
                "update_settings 不接受 widgets.{id} 整項取代或刪除（會改到版面）"
            ));
        };
        if entry.contains_key("placement") {
            return Err(format!(
                "update_settings 不接受 widgets.{id}.placement：版面只能經編輯版面或開啟小工具改變"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{in_grid_bounds, rects_overlap, GridRect};

    /// 每個測試用行程 id＋呼叫端給的名字組出唯一的暫存路徑，避免依賴額外的 crate
    /// （如 `tempfile`）；測試前後都嘗試清掉檔案，容忍「本來就不存在」。
    fn temp_path(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("fc-host-settings-test-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        dir.join(name)
    }

    fn cleanup(path: &Path) {
        let _ = fs::remove_file(path);
    }

    fn grid(col: i32, row: i32, w: i32, h: i32) -> GridRect {
        GridRect { col, row, w, h }
    }

    // ── 情況一：正常（無設定檔＝首次啟動）──────────────────────────────────────
    // 對應 specs/widget-host-windows「預設版面」：五個財經小工具在主螢幕、格座標兩兩不相交，
    // 擴充插槽預設關閉、放左側。

    #[test]
    fn settings_version_is_two() {
        assert_eq!(SETTINGS_VERSION, 2);
        assert_eq!(Settings::default().version, 2);
    }

    #[test]
    fn default_settings_has_grid_dashboard_layout() {
        let s = Settings::default();

        assert_eq!(s.version, SETTINGS_VERSION);
        assert_eq!(s.appearance_mode, AppearanceMode::Acrylic);
        assert_eq!(s.opacity, 0.55);
        assert_eq!(s.accent_color, "#e0aa54");
        assert!(!s.show_dividend);
        assert!(s.layout_locked, "版面預設應鎖定");
        assert!(s.autostart, "自動啟動預設應開啟");
        assert!(!s.pause_rules.pause_on_battery);
        assert!(!s.pause_rules.pause_on_power_saver);
        let data_dir_normalized = s.data_dir.to_string_lossy().replace('\\', "/");
        assert!(
            data_dir_normalized.ends_with("tw.fintools.fc-host/data"),
            "實際路徑：{data_dir_normalized}"
        );

        assert_eq!(s.widgets.len(), 10, "十個小工具都要有預設項目");
        for id in ["clock", "macro", "fixed", "dynamic", "quotes"] {
            assert!(s.widgets[id].enabled, "{id} 預設應開啟");
        }
        for id in ["custom1", "custom2", "custom3", "custom4", "custom5"] {
            assert!(!s.widgets[id].enabled, "{id} 預設應關閉");
        }
        for (id, cfg) in &s.widgets {
            assert_eq!(
                cfg.placement.monitor,
                MonitorId::Primary,
                "{id} 首次啟動一律主螢幕"
            );
        }

        // tasks.md 7.2 初值（時鐘 h 依 headless 量測定案，見 task-7.2-report.md；fix min-height
        // 加計 --widget-gap 後時鐘加高一格、總經日曆下移一格）。
        let expect = [
            ("clock", grid(15, 1, 16, 10)),
            ("macro", grid(15, 12, 16, 30)),
            ("fixed", grid(32, 1, 15, 17)),
            ("dynamic", grid(32, 19, 15, 23)),
            ("quotes", grid(15, 43, 32, 4)),
            ("custom1", grid(0, 1, 14, 8)),
            ("custom2", grid(0, 10, 14, 8)),
            ("custom3", grid(0, 19, 14, 8)),
            ("custom4", grid(0, 28, 14, 8)),
            ("custom5", grid(0, 37, 14, 8)),
        ];
        for (id, rect) in expect {
            assert_eq!(s.widgets[id].placement.grid_rect(), rect, "{id} 預設格座標");
        }
    }

    #[test]
    fn default_grid_rects_are_in_bounds_and_pairwise_disjoint() {
        for (i, a) in DEFAULT_GRID_RECTS.iter().enumerate() {
            assert!(in_grid_bounds(*a), "{} 超界：{a:?}", WIDGET_IDS[i]);
            for (j, b) in DEFAULT_GRID_RECTS.iter().enumerate().skip(i + 1) {
                assert!(
                    !rects_overlap(*a, *b),
                    "{} 與 {} 重疊：{a:?} / {b:?}",
                    WIDGET_IDS[i],
                    WIDGET_IDS[j]
                );
            }
        }
    }

    #[test]
    fn load_or_default_on_missing_file_matches_default() {
        let path = temp_path("no-such-settings.json");
        cleanup(&path);

        let loaded = load_or_default(&path);
        assert_eq!(loaded, Settings::default());
    }

    // ── 情況二：缺欄位補齊（version 2 的檔案）──────────────────────────────────

    #[test]
    fn missing_top_level_fields_are_backfilled() {
        let path = temp_path("partial-top-level.json");
        fs::write(&path, r#"{"version": 2, "opacity": 0.7}"#).expect("寫入測試檔失敗");

        let loaded = load_or_default(&path);

        assert_eq!(
            loaded.opacity, 0.7,
            "有給的欄位要保留原值，不能被預設值蓋掉"
        );
        assert_eq!(loaded.accent_color, "#e0aa54", "缺的欄位補齊為預設值");
        assert!(loaded.layout_locked);
        assert!(loaded.autostart);
        assert_eq!(
            loaded.widgets.len(),
            10,
            "缺整個 widgets 欄位也要補滿十個小工具"
        );
        assert!(loaded.widgets["clock"].enabled);
        assert!(!loaded.widgets["custom1"].enabled);
        assert!(path.exists(), "version 2 的檔案不應被當成舊版搬走");

        cleanup(&path);
    }

    #[test]
    fn missing_widget_entries_are_backfilled_with_their_own_defaults() {
        let path = temp_path("partial-widgets.json");
        fs::write(
            &path,
            r#"{"version": 2, "widgets": {"clock": {"enabled": false, "placement": {"col": 0, "row": 40, "w": 10, "h": 8}}}}"#,
        )
        .expect("寫入測試檔失敗");

        let loaded = load_or_default(&path);

        assert_eq!(loaded.widgets.len(), 10);
        assert!(!loaded.widgets["clock"].enabled);
        assert_eq!(
            loaded.widgets["clock"].placement.grid_rect(),
            grid(0, 40, 10, 8)
        );
        let defaults = Settings::default();
        assert_eq!(loaded.widgets["macro"], defaults.widgets["macro"]);
        assert_eq!(loaded.widgets["quotes"], defaults.widgets["quotes"]);
        assert!(loaded.widgets["fixed"].enabled);

        cleanup(&path);
    }

    #[test]
    fn existing_widget_with_empty_object_gets_its_own_defaults() {
        let path = temp_path("widget-empty-object.json");
        fs::write(&path, r#"{"version": 2, "widgets": {"clock": {}}}"#).expect("寫入測試檔失敗");

        let loaded = load_or_default(&path);

        assert_eq!(
            loaded.widgets["clock"],
            default_widgets()["clock"],
            "clock 只給空物件：enabled 與 placement 全部要是 clock 自己的預設值"
        );
        cleanup(&path);
    }

    #[test]
    fn existing_widget_missing_enabled_keeps_given_placement_fields() {
        let path = temp_path("widget-missing-enabled.json");
        fs::write(
            &path,
            r#"{"version": 2, "widgets": {"macro": {"placement": {"col": 1}}}}"#,
        )
        .expect("寫入測試檔失敗");

        let loaded = load_or_default(&path);
        let def = &default_widgets()["macro"];
        let got = &loaded.widgets["macro"];

        assert!(got.enabled, "缺 enabled 應補 macro 的預設（開啟）");
        assert_eq!(got.placement.col, 1, "明確提供的 col 要保留");
        assert_eq!(got.placement.row, def.placement.row, "缺 row 補 macro 預設");
        assert_eq!(got.placement.w, def.placement.w);
        assert_eq!(got.placement.h, def.placement.h);
        assert_eq!(got.placement.monitor, def.placement.monitor);
        cleanup(&path);
    }

    #[test]
    fn existing_widget_missing_placement_gets_its_own_placement() {
        let path = temp_path("widget-missing-placement.json");
        fs::write(
            &path,
            r#"{"version": 2, "widgets": {"quotes": {"enabled": false}}}"#,
        )
        .expect("寫入測試檔失敗");

        let loaded = load_or_default(&path);

        assert!(
            !loaded.widgets["quotes"].enabled,
            "明確提供的 enabled 要保留"
        );
        assert_eq!(
            loaded.widgets["quotes"].placement,
            default_widgets()["quotes"].placement,
            "缺整個 placement 應補 quotes 自己的預設格座標"
        );
        cleanup(&path);
    }

    #[test]
    fn explicit_null_top_level_field_falls_back_to_default() {
        let path = temp_path("null-field.json");
        fs::write(
            &path,
            r##"{"version": 2, "opacity": null, "accent_color": "#010203"}"##,
        )
        .expect("寫入測試檔失敗");

        let loaded = load_or_default(&path);

        assert_eq!(loaded.opacity, Settings::default().opacity);
        assert_eq!(loaded.accent_color, "#010203");
        assert!(path.exists(), "可解析的檔案不應被當成損壞搬走");
        cleanup(&path);
    }

    // ── 載入後格座標範圍驗證（design.md D9）───────────────────────────────────

    /// fix F1（review 7.2 M1）：`col + w` 等加法溢位不得繞過範圍驗證（release 溢位成負數被當
    /// 合法、debug 直接 panic）。極值一律退回預設格座標，載入不 panic。
    #[test]
    fn overflowing_grid_values_fall_back_to_default_without_panicking() {
        let path = temp_path("overflow-grid.json");
        let raw = r#"{"version": 2, "widgets": {
            "clock":   {"placement": {"col": 2147483000, "row": 1, "w": 1000, "h": 9}},
            "macro":   {"placement": {"col": 15, "row": 2147483647, "w": 16, "h": 2147483647}},
            "fixed":   {"placement": {"col": 1, "row": 1, "w": 2147483647, "h": 17}},
            "dynamic": {"placement": {"col": -2147483648, "row": 0, "w": 2147483647, "h": 5}},
            "quotes":  {"placement": {"col": 15, "row": 1, "w": 32, "h": 2147483647}}
        }}"#;
        fs::write(&path, raw).expect("寫入測試檔失敗");

        let loaded = load_or_default(&path);
        let defaults = Settings::default();
        for id in ["clock", "macro", "fixed", "dynamic", "quotes"] {
            assert_eq!(
                loaded.widgets[id].placement.grid_rect(),
                defaults.widgets[id].placement.grid_rect(),
                "{id} 的極值格座標應退回自己的預設格座標"
            );
        }
        cleanup(&path);
    }

    #[test]
    fn out_of_range_grid_falls_back_to_that_widgets_default_in_memory() {
        let path = temp_path("out-of-range-grid.json");
        // clock：col + w = 50 > 48；macro：w = 0；fixed：row 負值；dynamic：合法、要保留；
        // quotes：row + h = 49 > 48。monitor 欄位不是格座標，保留原值。
        let raw = r#"{"version": 2, "opacity": 0.3, "widgets": {
            "clock":   {"placement": {"monitor": {"device": "X"}, "col": 40, "row": 1, "w": 10, "h": 9}},
            "macro":   {"placement": {"col": 15, "row": 11, "w": 0, "h": 31}},
            "fixed":   {"placement": {"col": 32, "row": -1, "w": 15, "h": 17}},
            "dynamic": {"placement": {"col": 30, "row": 20, "w": 18, "h": 28}},
            "quotes":  {"placement": {"col": 15, "row": 45, "w": 32, "h": 4}}
        }}"#;
        fs::write(&path, raw).expect("寫入測試檔失敗");

        let loaded = load_or_default(&path);
        let defaults = Settings::default();

        for id in ["clock", "macro", "fixed", "quotes"] {
            assert_eq!(
                loaded.widgets[id].placement.grid_rect(),
                defaults.widgets[id].placement.grid_rect(),
                "{id} 超界應退回自己的預設格座標"
            );
        }
        assert_eq!(
            loaded.widgets["clock"].placement.monitor,
            MonitorId::Device("X".to_string()),
            "只退回格座標，monitor 保留"
        );
        assert_eq!(
            loaded.widgets["dynamic"].placement.grid_rect(),
            grid(30, 20, 18, 28),
            "合法的格座標原樣保留"
        );
        assert_eq!(loaded.opacity, 0.3, "其餘欄位不受影響");
        assert!(
            path.exists(),
            "格座標超界不是損壞，不搬檔（下一次存檔時才寫回）"
        );
        assert_eq!(
            fs::read_to_string(&path).expect("讀檔失敗"),
            raw,
            "退回只在記憶體中，不改寫檔案"
        );
        cleanup(&path);
    }

    // ── 情況三：損壞檔／舊版／缺版本 → 改名備份 ─────────────────────────────────

    /// 找出 `path` 的 `.bad-*` 備份檔名（測試共用）。
    fn backups_of(path: &Path) -> Vec<PathBuf> {
        let dir = path.parent().expect("暫存路徑一定有上層目錄");
        let stem = path.file_name().unwrap().to_str().unwrap().to_string();
        fs::read_dir(dir)
            .expect("讀暫存目錄失敗")
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(&format!("{stem}.bad-"))
            })
            .map(|e| e.path())
            .collect()
    }

    fn remove_backups(path: &Path) {
        for b in backups_of(path) {
            let _ = fs::remove_file(b);
        }
    }

    /// 寫入 `raw`、載入，斷言：以預設值啟動、原檔被改名成恰好一個 `.bad-*` 備份且內容不變。
    fn assert_backed_up_and_defaulted(name: &str, raw: &[u8]) {
        let path = temp_path(name);
        remove_backups(&path);
        fs::write(&path, raw).expect("寫入測試檔失敗");

        let loaded = load_or_default(&path);
        assert_eq!(loaded, Settings::default(), "{name}：要以預設值啟動");
        assert!(!path.exists(), "{name}：原檔應被改名搬走");
        let backups = backups_of(&path);
        assert_eq!(backups.len(), 1, "{name}：應恰好一個備份檔：{backups:?}");
        assert_eq!(
            fs::read(&backups[0]).expect("讀備份失敗"),
            raw,
            "{name}：備份要完整保留原始位元組"
        );
        remove_backups(&path);
    }

    #[test]
    fn version_1_file_is_backed_up_and_defaults_used() {
        // 舊版（錨點模型）設定檔：version 1、anchor／dx／dy／scale。
        assert_backed_up_and_defaulted(
            "v1-settings.json",
            br#"{"version": 1, "opacity": 0.9, "widgets": {"clock": {"enabled": true,
                "placement": {"monitor": "primary", "anchor": "top_right", "dx": 522, "dy": 26, "scale": 1.0}}}}"#,
        );
    }

    #[test]
    fn missing_version_file_is_backed_up_and_defaults_used() {
        // 版本判斷必須在 merge_json 之前：否則缺少的 version 會被預設值補成 2。
        assert_backed_up_and_defaulted("no-version-settings.json", br#"{"opacity": 0.9}"#);
    }

    #[test]
    fn version_0_and_non_numeric_version_are_backed_up() {
        assert_backed_up_and_defaulted("v0-settings.json", br#"{"version": 0}"#);
        assert_backed_up_and_defaulted("vstr-settings.json", br#"{"version": "2"}"#);
        assert_backed_up_and_defaulted("vnull-settings.json", br#"{"version": null}"#);
    }

    #[test]
    fn non_object_json_is_backed_up() {
        assert_backed_up_and_defaulted("array-settings.json", b"[]");
    }

    // fix round 1（Codex 2.3-b）：非 UTF-8 位元組（位元組損壞、誤存 UTF-16）也是「損壞」，
    // 必須備份並保留原始內容，不能當成缺檔靜默重設。
    #[test]
    fn non_utf8_file_is_backed_up_with_original_bytes() {
        // UTF-16 LE BOM ＋ `{}`，外加一個無效 UTF-8 位元組。
        assert_backed_up_and_defaulted(
            "non-utf8-settings.json",
            &[0xFF, 0xFE, b'{', 0x00, b'}', 0x00, 0xC3],
        );
    }

    #[test]
    fn corrupt_file_is_backed_up_and_default_used() {
        assert_backed_up_and_defaulted(
            "corrupt-settings.json",
            "{ 這不是合法的 JSON ,,,".as_bytes(),
        );
    }

    // ── 額外保險：serde 標註本身正確（往返一致）─────────────────────────────────

    #[test]
    fn default_settings_round_trips_through_json() {
        let original = Settings::default();
        let text = serde_json::to_string_pretty(&original).expect("序列化失敗");
        let parsed: Settings = serde_json::from_str(&text).expect("反序列化失敗");
        assert_eq!(original, parsed);
    }

    #[test]
    fn placement_serializes_as_monitor_plus_grid_fields_only() {
        let json = serde_json::to_value(&Settings::default().widgets["clock"].placement)
            .expect("序列化失敗");
        let mut keys: Vec<&str> = json
            .as_object()
            .expect("placement 是物件")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["col", "h", "monitor", "row", "w"]);
    }

    #[test]
    fn default_settings_path_points_under_appdata_namespace() {
        let path = default_settings_path();
        let normalized = path.to_string_lossy().replace('\\', "/");
        assert!(
            normalized.ends_with("tw.fintools.fc-host/settings.json"),
            "實際路徑：{normalized}"
        );
    }

    #[test]
    fn default_gatekeeper_log_path_points_under_appdata_namespace() {
        let path = default_gatekeeper_log_path();
        let normalized = path.to_string_lossy().replace('\\', "/");
        assert!(
            normalized.ends_with("tw.fintools.fc-host/gatekeeper.log"),
            "實際路徑：{normalized}"
        );
    }

    // ── save：原子寫入＋可被 load_or_default 讀回 ──────────────────────────────

    #[test]
    fn save_then_load_round_trips() {
        let path = temp_path("save-roundtrip.json");
        cleanup(&path);

        let mut settings = Settings {
            opacity: 0.42,
            accent_color: "#123456".to_string(),
            ..Settings::default()
        };
        settings.widgets.get_mut("clock").unwrap().placement = WidgetPlacement {
            monitor: MonitorId::Device("\\\\?\\DISPLAY#X".to_string()),
            col: 2,
            row: 3,
            w: 12,
            h: 9,
        };

        save(&path, &settings).expect("寫入設定檔失敗");
        assert!(path.exists(), "存檔後檔案應存在");
        assert!(
            !path.with_file_name("save-roundtrip.json.tmp").exists(),
            "暫存檔應已被 rename 換掉，不應殘留"
        );

        let loaded = load_or_default(&path);
        assert_eq!(loaded, settings, "存檔帶 version 2，重新載入不被當成舊版");

        cleanup(&path);
    }

    #[test]
    fn save_creates_missing_parent_directory() {
        let base = env::temp_dir().join(format!(
            "fc-host-settings-test-{}-nested-{}",
            std::process::id(),
            "save-mkdir"
        ));
        let _ = fs::remove_dir_all(&base);
        let path = base.join("sub").join("settings.json");

        save(&path, &Settings::default()).expect("目錄不存在時應自動建立");
        assert!(path.exists());

        let _ = fs::remove_dir_all(&base);
    }

    // ── merge_patch：頂層欄位、物件巢狀合併、缺漏補齊、合併失敗 ─────────────────────

    #[test]
    fn merge_patch_overwrites_only_given_top_level_field() {
        let current = Settings::default();
        let patch = serde_json::json!({ "opacity": 0.9 });

        let merged = merge_patch(&current, &patch).expect("合併應成功");

        assert_eq!(merged.opacity, 0.9, "有給的欄位要套用新值");
        assert_eq!(
            merged.accent_color, current.accent_color,
            "沒給的欄位應保持原值，不能被 patch 清空"
        );
        assert_eq!(
            merged.widgets, current.widgets,
            "沒提到的巢狀欄位應維持原狀"
        );
    }

    #[test]
    fn merge_patch_deep_merges_nested_widget_object_without_clobbering_siblings() {
        let current = Settings::default();
        let patch = serde_json::json!({
            "widgets": { "clock": { "enabled": false } }
        });

        let merged = merge_patch(&current, &patch).expect("合併應成功");

        assert!(!merged.widgets["clock"].enabled, "clock.enabled 應被改掉");
        assert_eq!(
            merged.widgets["clock"].placement, current.widgets["clock"].placement,
            "clock.placement 沒被提到，應維持原值（深合併，不是整個物件被取代）"
        );
        assert_eq!(merged.widgets["macro"], current.widgets["macro"]);
        assert_eq!(merged.widgets["custom1"], current.widgets["custom1"]);
        assert_eq!(merged.widgets.len(), 10, "十個小工具都要還在");
    }

    #[test]
    fn merge_patch_null_removes_key_then_backfill_restores_widget_default() {
        let current = Settings::default();
        let patch = serde_json::json!({ "widgets": { "custom1": null } });

        let merged = merge_patch(&current, &patch).expect("合併應成功");

        assert_eq!(
            merged.widgets.len(),
            10,
            "custom1 被刪掉後應由 backfill 補回，十個小工具仍應齊全"
        );
        assert_eq!(
            merged.widgets["custom1"],
            default_widgets()["custom1"],
            "補回的應是 custom1 自己的預設值，不是泛用預設"
        );
    }

    #[test]
    fn merge_patch_rejects_value_with_wrong_type() {
        let current = Settings::default();
        let patch = serde_json::json!({ "opacity": "not-a-number" });

        let result = merge_patch(&current, &patch);
        assert!(result.is_err(), "型別對不上應回傳 Err，不應靜默接受");
    }

    #[test]
    fn merge_patch_leaves_current_settings_untouched_on_failure() {
        let current = Settings::default();
        let patch = serde_json::json!({ "opacity": "not-a-number" });

        let _ = merge_patch(&current, &patch);
        assert_eq!(current, Settings::default());
    }

    // ── merge_user_patch：update_settings 拒收 placement（design.md D7）─────────────

    #[test]
    fn merge_user_patch_accepts_enabled_and_top_level_fields() {
        let current = Settings::default();
        let patch = serde_json::json!({
            "opacity": 0.8,
            "widgets": { "custom2": { "enabled": true } }
        });
        let merged = merge_user_patch(&current, &patch).expect("不含 placement 的 patch 應接受");
        assert_eq!(merged.opacity, 0.8);
        assert!(merged.widgets["custom2"].enabled);
        assert_eq!(
            merged.widgets["custom2"].placement,
            current.widgets["custom2"].placement
        );
    }

    #[test]
    fn merge_user_patch_rejects_any_placement_field() {
        let current = Settings::default();
        let rejected = [
            serde_json::json!({ "widgets": { "clock": { "placement": { "col": 0 } } } }),
            serde_json::json!({ "widgets": { "clock": { "enabled": true, "placement": {} } } }),
            serde_json::json!({ "widgets": { "clock": { "placement": null } } }),
            // 整個小工具或整個 widgets 設成 null 會由 backfill 重設成預設格座標，同樣是改版面。
            serde_json::json!({ "widgets": { "clock": null } }),
            serde_json::json!({ "widgets": null }),
            // 型別不是物件的 widgets／小工具項目也一律拒絕（無法確認不含 placement）。
            serde_json::json!({ "widgets": [] }),
            serde_json::json!({ "widgets": { "clock": true } }),
        ];
        for patch in rejected {
            let result = merge_user_patch(&current, &patch);
            assert!(result.is_err(), "應拒收：{patch}");
        }
    }

    /// fix F1（review 7.2 L）：前端 fixture 模式的 `host/ui/fixtures/settings.json` 是「真實宿主
    /// `get_settings` 回覆」的權威樣本，必須是目前版本的格線格式：版本判斷通過、不含任何錨點
    /// 模型欄位，且反序列化後的版面與預設設定一致（只有 `data_dir` 是範例路徑）。
    #[test]
    fn ui_fixture_settings_json_is_current_grid_format() {
        let raw = include_str!("../ui/fixtures/settings.json");
        let value: serde_json::Value = serde_json::from_str(raw).expect("fixture 應為合法 JSON");
        assert!(
            is_current_version(&value),
            "fixture 應為 v{SETTINGS_VERSION}"
        );
        for field in ["anchor", "\"dx\"", "\"dy\"", "\"scale\""] {
            assert!(!raw.contains(field), "fixture 不應再有錨點模型欄位 {field}");
        }
        let parsed: Settings = serde_json::from_value(value).expect("fixture 應能反序列化");
        let expected = Settings {
            data_dir: parsed.data_dir.clone(),
            ..Settings::default()
        };
        assert_eq!(parsed, expected, "fixture 除 data_dir 外應等於預設設定");
    }

    /// fix F1（review 7.2 L）：`version` 寫成 1／0 會讓下次啟動把整份設定判成舊版、備份並重設；
    /// `update_settings` 比照 `placement` 拒收任何 `version` 欄位（含 `null` 與目前版本值）。
    #[test]
    fn merge_user_patch_rejects_version_field() {
        let current = Settings::default();
        for patch in [
            serde_json::json!({ "version": 1 }),
            serde_json::json!({ "version": 0 }),
            serde_json::json!({ "version": null }),
            serde_json::json!({ "version": SETTINGS_VERSION }),
            serde_json::json!({ "opacity": 0.5, "version": 1 }),
        ] {
            assert!(
                merge_user_patch(&current, &patch).is_err(),
                "應拒收：{patch}"
            );
        }
    }

    #[test]
    fn merge_user_patch_still_rejects_wrong_types() {
        let current = Settings::default();
        let patch = serde_json::json!({ "opacity": "x" });
        assert!(merge_user_patch(&current, &patch).is_err());
    }

    // ── load_for_startup：首次落地與損壞檔存回（fix F2b，重審 low：損壞／版本不符） ──────

    #[test]
    fn startup_with_missing_file_creates_it() {
        let path = temp_path("startup-missing.json");
        cleanup(&path);
        let first = load_for_startup(&path);
        assert!(first.save_error.is_none());
        assert_eq!(first.settings, Settings::default());
        assert!(path.exists(), "首次啟動要把預設值落地");
        let second = load_for_startup(&path);
        assert!(second.save_error.is_none());
        assert_eq!(second.settings, Settings::default());
        cleanup(&path);
    }

    /// 損壞檔／版本不符：本次備份後要立刻把預設值存回去，下一次啟動不再被當成損壞檔處理。
    #[test]
    fn startup_after_corrupt_or_old_file_saves_defaults_back() {
        for (name, raw) in [
            ("startup-corrupt.json", &b"{not json"[..]),
            (
                "startup-old-version.json",
                &br#"{"version":1,"autostart":false}"#[..],
            ),
        ] {
            let path = temp_path(name);
            remove_backups(&path);
            fs::write(&path, raw).expect("寫入測試檔失敗");

            let first = load_for_startup(&path);
            assert!(first.save_error.is_none(), "{name}：{:?}", first.save_error);
            assert_eq!(first.settings, Settings::default(), "{name}：以預設值啟動");
            assert_eq!(backups_of(&path).len(), 1, "{name}：原檔要有一份備份");
            assert!(path.exists(), "{name}：預設值要立刻存回設定檔");
            assert_eq!(
                load_or_default(&path),
                Settings::default(),
                "{name}：存回的是預設值"
            );

            let second = load_for_startup(&path);
            assert_eq!(second.settings, Settings::default(), "{name}");
            assert_eq!(backups_of(&path).len(), 1, "{name}：存回的檔不再被當成損壞");

            cleanup(&path);
            remove_backups(&path);
        }
    }

    // ── fix F3（review task-5.1-r-opus.md [low]）：仲裁落敗的 Secondary 不寫任何東西 ──────────

    #[test]
    fn secondary_startup_with_missing_file_does_not_create_it() {
        let path = temp_path("startup-secondary-missing.json");
        cleanup(&path);
        let loaded = load_for_arbitrated_startup(&path, false);
        assert!(loaded.save_error.is_none());
        assert_eq!(loaded.settings, Settings::default());
        assert!(!path.exists(), "Secondary 不得把預設值落地");
        cleanup(&path);
    }

    #[test]
    fn secondary_startup_with_corrupt_file_leaves_it_untouched() {
        let path = temp_path("startup-secondary-corrupt.json");
        remove_backups(&path);
        fs::write(&path, b"{not json").expect("寫入測試檔失敗");
        let loaded = load_for_arbitrated_startup(&path, false);
        assert_eq!(loaded.settings, Settings::default(), "讀不懂就用預設值");
        assert_eq!(
            fs::read(&path).expect("原檔應仍在原位"),
            b"{not json",
            "Secondary 不得覆寫損壞的設定檔"
        );
        assert!(
            backups_of(&path).is_empty(),
            "Secondary 不得備份（改名）損壞的設定檔"
        );
        cleanup(&path);
        remove_backups(&path);
    }

    #[test]
    fn secondary_startup_reads_existing_settings() {
        let path = temp_path("startup-secondary-existing.json");
        cleanup(&path);
        let saved = Settings {
            autostart: false,
            ..Settings::default()
        };
        save(&path, &saved).expect("存檔失敗");
        let loaded = load_for_arbitrated_startup(&path, false);
        assert_eq!(loaded.settings, saved);
        cleanup(&path);
    }

    #[test]
    fn primary_startup_keeps_first_launch_behavior() {
        let path = temp_path("startup-primary-missing.json");
        cleanup(&path);
        let loaded = load_for_arbitrated_startup(&path, true);
        assert!(loaded.save_error.is_none());
        assert!(path.exists(), "Primary 首次啟動照舊把預設值落地");
        cleanup(&path);
    }

    // ── 桌布主題（dynamic-wallpaper task 4.1）──────────────────────────────────

    #[test]
    fn default_wallpaper_theme_is_none() {
        assert_eq!(Settings::default().wallpaper_theme, WallpaperTheme::None);
        assert_eq!(WallpaperTheme::default(), WallpaperTheme::None);
    }

    #[test]
    fn wallpaper_theme_serializes_as_stable_strings_matching_page_files() {
        let expected = [
            "none",
            "astrolabe",
            "tearoff",
            "ridgeline",
            "contour",
            "skyline",
        ];
        let pages = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("ui")
            .join("wallpapers");
        for (theme, want) in WallpaperTheme::ALL.iter().zip(expected) {
            assert_eq!(theme.as_str(), want);
            assert_eq!(
                serde_json::to_value(theme).expect("序列化失敗"),
                serde_json::Value::String(want.to_string()),
                "serde 字串應與 as_str 相同"
            );
            let back: WallpaperTheme =
                serde_json::from_value(serde_json::json!(want)).expect("反序列化失敗");
            assert_eq!(back, *theme);
            if *theme != WallpaperTheme::None {
                assert!(
                    pages.join(format!("{want}.html")).is_file(),
                    "主題字串 {want} 應對應 host/ui/wallpapers/{want}.html"
                );
            }
        }
    }

    #[test]
    fn old_settings_without_theme_reads_none_and_keeps_other_fields_without_rewrite() {
        let path = temp_path("old-no-theme.json");
        cleanup(&path);
        remove_backups(&path);
        let mut old = serde_json::to_value(Settings {
            opacity: 0.8,
            accent_color: "#abcdef".to_string(),
            autostart: false,
            ..Settings::default()
        })
        .expect("序列化失敗");
        old.as_object_mut()
            .expect("應為物件")
            .remove(WALLPAPER_THEME_KEY);
        let raw = serde_json::to_vec_pretty(&old).expect("序列化失敗");
        assert!(!String::from_utf8_lossy(&raw).contains(WALLPAPER_THEME_KEY));
        fs::write(&path, &raw).expect("寫入測試檔失敗");

        let loaded = load_for_startup(&path);

        assert!(loaded.save_error.is_none());
        let s = loaded.settings;
        assert_eq!(s.wallpaper_theme, WallpaperTheme::None);
        assert_eq!(s.opacity, 0.8);
        assert_eq!(s.accent_color, "#abcdef");
        assert!(!s.autostart);
        assert_eq!(
            fs::read(&path).expect("原檔應仍在"),
            raw,
            "舊版設定檔缺主題欄位不得觸發重寫"
        );
        assert!(backups_of(&path).is_empty(), "不得被當成損壞而備份");
        cleanup(&path);
    }

    #[test]
    fn saved_theme_round_trips() {
        let path = temp_path("theme-roundtrip.json");
        cleanup(&path);
        let saved = Settings {
            wallpaper_theme: WallpaperTheme::Tearoff,
            ..Settings::default()
        };
        save(&path, &saved).expect("存檔失敗");
        assert!(fs::read_to_string(&path)
            .expect("讀檔失敗")
            .contains(r#""wallpaper_theme": "tearoff""#));
        assert_eq!(load_or_default(&path), saved);
        cleanup(&path);
    }

    #[test]
    fn unknown_theme_value_reads_none_with_warning_and_keeps_other_fields() {
        for (name, bad) in [
            ("theme-unknown-string.json", serde_json::json!("galaxy")),
            ("theme-wrong-case.json", serde_json::json!("Astrolabe")),
            ("theme-number.json", serde_json::json!(3)),
            ("theme-object.json", serde_json::json!({"id": "astrolabe"})),
        ] {
            let path = temp_path(name);
            cleanup(&path);
            remove_backups(&path);
            let raw = serde_json::to_vec(&serde_json::json!({
                "version": 2,
                "opacity": 0.3,
                "layout_locked": false,
                "wallpaper_theme": bad,
            }))
            .expect("序列化失敗");
            fs::write(&path, &raw).expect("寫入測試檔失敗");

            let loaded = load_for_startup(&path).settings;

            assert_eq!(loaded.wallpaper_theme, WallpaperTheme::None, "{name}");
            assert_eq!(loaded.opacity, 0.3, "{name}：其他欄位不得遺失");
            assert!(!loaded.layout_locked, "{name}：其他欄位不得遺失");
            assert!(
                backups_of(&path).is_empty(),
                "{name}：主題值不認得不是損壞，不得備份重設"
            );
            assert_eq!(
                fs::read(&path).expect("原檔應仍在"),
                raw,
                "{name}：不得重寫"
            );
            cleanup(&path);
        }
    }

    #[test]
    fn sanitize_wallpaper_theme_warns_only_for_unrecognized_values() {
        let mut bad = serde_json::json!({"version": 2, "wallpaper_theme": "galaxy"});
        let warning = sanitize_wallpaper_theme(&mut bad).expect("不認得的值應回警告");
        assert!(warning.contains("galaxy"), "警告應帶出原值：{warning}");
        assert!(bad.get(WALLPAPER_THEME_KEY).is_none(), "不認得的值應移除");

        for ok in [
            serde_json::json!({"version": 2}),
            serde_json::json!({"version": 2, "wallpaper_theme": null}),
            serde_json::json!({"version": 2, "wallpaper_theme": "skyline"}),
            serde_json::json!({"version": 2, "wallpaper_theme": "none"}),
        ] {
            let mut v = ok.clone();
            assert_eq!(sanitize_wallpaper_theme(&mut v), None, "{ok}");
            assert_eq!(v, ok, "認得的值或缺漏不應被改動：{ok}");
        }
    }

    #[test]
    fn merge_user_patch_sets_theme_and_rejects_unknown_value() {
        let merged = merge_user_patch(
            &Settings::default(),
            &serde_json::json!({"wallpaper_theme": "astrolabe"}),
        )
        .expect("合法主題應可設定");
        assert_eq!(merged.wallpaper_theme, WallpaperTheme::Astrolabe);
        assert!(
            merge_user_patch(
                &Settings::default(),
                &serde_json::json!({"wallpaper_theme": "galaxy"})
            )
            .is_err(),
            "update_settings 帶不認得的主題應拒收"
        );
    }

    // ── data_fetch（data-layer-rust 5.3）──────────────────────────────────────────

    #[test]
    fn default_data_fetch_is_auto_and_version_is_unchanged() {
        assert_eq!(Settings::default().data_fetch, DataFetch::Auto);
        assert_eq!(SETTINGS_VERSION, 2, "5.3 不改 SETTINGS_VERSION");
    }

    #[test]
    fn data_fetch_serializes_as_lowercase_strings() {
        for (v, text) in [
            (DataFetch::Auto, "auto"),
            (DataFetch::On, "on"),
            (DataFetch::Off, "off"),
        ] {
            assert_eq!(v.as_str(), text);
            assert_eq!(serde_json::to_value(v).unwrap(), serde_json::json!(text));
            assert_eq!(
                serde_json::from_value::<DataFetch>(serde_json::json!(text)).unwrap(),
                v
            );
        }
    }

    #[test]
    fn old_settings_file_without_data_fetch_reads_auto_and_is_not_rewritten() {
        let path = temp_path("data-fetch-missing.json");
        cleanup(&path);
        remove_backups(&path);
        let raw = serde_json::to_vec(&serde_json::json!({"version": 2, "opacity": 0.4}))
            .expect("序列化失敗");
        fs::write(&path, &raw).expect("寫入測試檔失敗");
        let loaded = load_for_startup(&path).settings;
        assert_eq!(loaded.data_fetch, DataFetch::Auto);
        assert_eq!(loaded.opacity, 0.4);
        assert!(backups_of(&path).is_empty());
        assert_eq!(fs::read(&path).unwrap(), raw, "舊檔缺欄位不得觸發重寫");
        cleanup(&path);
    }

    #[test]
    fn data_fetch_values_round_trip_through_a_settings_file() {
        for v in DataFetch::ALL {
            let path = temp_path(&format!("data-fetch-{}.json", v.as_str()));
            cleanup(&path);
            let saved = Settings {
                data_fetch: v,
                ..Settings::default()
            };
            save(&path, &saved).expect("存檔失敗");
            assert!(fs::read_to_string(&path)
                .unwrap()
                .contains(&format!("\"data_fetch\": \"{}\"", v.as_str())));
            assert_eq!(load_or_default(&path).data_fetch, v);
            cleanup(&path);
        }
    }

    #[test]
    fn unknown_data_fetch_value_reads_auto_keeps_other_fields_and_does_not_reset() {
        for (name, bad) in [
            ("fetch-unknown-string.json", serde_json::json!("maybe")),
            ("fetch-wrong-case.json", serde_json::json!("OFF")),
            ("fetch-bool.json", serde_json::json!(false)),
            ("fetch-number.json", serde_json::json!(0)),
        ] {
            let path = temp_path(name);
            cleanup(&path);
            remove_backups(&path);
            let raw = serde_json::to_vec(&serde_json::json!({
                "version": 2,
                "opacity": 0.3,
                "data_fetch": bad,
            }))
            .expect("序列化失敗");
            fs::write(&path, &raw).expect("寫入測試檔失敗");
            let loaded = load_for_startup(&path).settings;
            assert_eq!(loaded.data_fetch, DataFetch::Auto, "{name}");
            assert_eq!(loaded.opacity, 0.3, "{name}：其他欄位不得遺失");
            assert!(
                backups_of(&path).is_empty(),
                "{name}：不是損壞，不得備份重設"
            );
            assert_eq!(fs::read(&path).unwrap(), raw, "{name}：不得重寫");
            cleanup(&path);
        }
    }

    #[test]
    fn sanitize_data_fetch_warns_only_for_unrecognized_values() {
        let mut bad = serde_json::json!({"version": 2, "data_fetch": "maybe"});
        let warning = sanitize_data_fetch(&mut bad).expect("不認得的值應回警告");
        assert!(warning.contains("maybe"), "警告應帶出原值：{warning}");
        assert!(bad.get(DATA_FETCH_KEY).is_none());
        for ok in [
            serde_json::json!({"version": 2}),
            serde_json::json!({"version": 2, "data_fetch": null}),
            serde_json::json!({"version": 2, "data_fetch": "auto"}),
            serde_json::json!({"version": 2, "data_fetch": "on"}),
            serde_json::json!({"version": 2, "data_fetch": "off"}),
        ] {
            let mut v = ok.clone();
            assert_eq!(sanitize_data_fetch(&mut v), None, "{ok}");
            assert_eq!(v, ok);
        }
    }

    #[test]
    fn merge_user_patch_can_change_data_fetch_and_rejects_unknown_value() {
        let current = Settings::default();
        let merged = merge_user_patch(&current, &serde_json::json!({"data_fetch": "off"}))
            .expect("data_fetch 可由 update_settings 改");
        assert_eq!(merged.data_fetch, DataFetch::Off);
        let back = merge_user_patch(&merged, &serde_json::json!({"data_fetch": "on"})).unwrap();
        assert_eq!(back.data_fetch, DataFetch::On);
        assert!(merge_user_patch(&current, &serde_json::json!({"data_fetch": "maybe"})).is_err());
    }
}
