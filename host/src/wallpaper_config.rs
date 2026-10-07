//! 動態桌布的主題設定檔 `wallpaper-config.json`（dynamic-wallpaper task 4.1；design.md D7；
//! specs/wallpaper-themes「可編輯的主題設定檔」）。
//!
//! ## 檔案與內建預設
//!
//! - 位置：宿主設定資料夾的 [`CONFIG_FILE_NAME`]，與 `settings.json` 同一個資料夾
//!   （[`default_config_path`]＝`crate::settings::default_settings_path` 換檔名）。
//! - 內建預設：[`DEFAULT_CONFIG_JSON`] 以 `include_str!` 把
//!   `host/ui/wallpapers/config/wallpaper-config.default.json` 編進執行檔，是**唯一來源**——
//!   Rust 不另寫一份預設值。
//!
//! ## 合併分工（Rust 只管頂層）
//!
//! Rust 只做**頂層鍵**合併（[`merge_top_level`]）：
//!
//! - 使用者檔有、且 JSON 型別（object／array／string／number／boolean）與內建預設相同的頂層鍵
//!   → 整個採用使用者的值（不往下合併）。
//! - 型別不同（含 `null`）→ 該鍵採內建預設並記警告；其他鍵不受影響。
//! - 使用者檔缺的頂層鍵 → 以內建預設補上並記警告（spec「缺欄位時……記錄警告」）。每次載入
//!   只記**一行**、點名全部缺鍵，避免預設檔日後新增頂層鍵時既有使用者檔逐鍵洗版。
//! - 使用者多出來的頂層鍵原樣保留（向前相容、使用者備註）。
//!
//! 巢狀內容（市場時段、休市表的年度與日期、門檻數值……）的細部補齊與驗證，以頁面端的
//! `mergeConfig`（`host/ui/wallpapers/lib/core.mjs`）與 `validateThemeConfig`
//! （`host/ui/wallpapers/lib/config-holidays.mjs`）為**單一事實來源**，Rust 不重寫：頁面收到
//! 本模組合併後的設定，會再與自己的內建預設逐層合併並驗證。
//!
//! ## 檔案生命週期
//!
//! - [`load_for_startup`]：缺檔時以內建預設**原文**原子寫出一份，讓使用者有東西可編輯；本次使用
//!   內建預設。寫出用「暫存檔＋`fs::hard_link`」（目標已存在就失敗、不取代）：判定缺檔之後才出現的
//!   檔案（使用者剛建、或另一個行程剛寫）一律不覆寫，改讀它；寫出失敗記警告、清掉暫存檔、用預設。
//! - [`reload`]：設定檔變更後重新讀取；[`ConfigWatch`]（task 4.6，`wallpaper` 通道的設定來源）
//!   以同一條路徑做變更監看。**從不寫檔**——監看端對自己寫出的檔案再觸發一次重新載入會形成迴圈；缺檔時只記警告並使用內建預設，下次啟動再補。
//! - **啟動仲裁落敗的 Secondary 執行個體一律走 [`reload`]、絕不寫檔**（比照
//!   `crate::settings::load_for_arbitrated_startup`）；只有 Primary 呼叫 [`load_for_startup`]（4.7 接線）。
//! - 損壞（不是合法 JSON、不是 UTF-8、或頂層不是 object）：記警告、本次使用內建預設，**不覆寫、
//!   不改名、不刪除**使用者的檔案（與 `settings.json` 的備份重設不同：這份是使用者手改的檔，
//!   修好存檔即可生效）。不另寫範本檔：刪掉損壞的檔後下次啟動會重新產生完整預設檔。
//! - 讀不到（權限、鎖定）：記警告、使用內建預設，不寫檔。
//! - UTF-8 BOM（記事本存檔常見）會先去掉再解析，不算損壞。
//!
//! 載入函式不會失敗：任何情況都回傳一份可用的設定（頂層一定是 object），警告與資訊另放在
//! [`WallpaperConfig::warnings`]／[`WallpaperConfig::notes`]，並分別以 `log::warn!`／`log::info!`
//! 寫入記錄檔。

// task 4.6 起 wallpaper 通道經 [`ConfigWatch`] 使用本模組；`load_for_startup` 等的呼叫端（啟動與
// 設定變更接線）在 task 4.7。
#![allow(dead_code)]

use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::settings;

/// 主題設定檔檔名（放在宿主設定資料夾）。
pub const CONFIG_FILE_NAME: &str = "wallpaper-config.json";

/// 內建預設（唯一來源：`host/ui/wallpapers/config/wallpaper-config.default.json`）。
pub const DEFAULT_CONFIG_JSON: &str =
    include_str!("../ui/wallpapers/config/wallpaper-config.default.json");

/// 這次的設定從哪裡來。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSource {
    /// 讀到使用者檔並完成頂層合併（可能帶警告）。
    User,
    /// 缺檔，已把內建預設寫出成使用者檔；本次使用內建預設。
    CreatedDefault,
    /// 缺檔且沒有寫出（[`reload`] 不寫檔，或寫出失敗）；本次使用內建預設。
    MissingDefault,
    /// 使用者檔損壞；本次使用內建預設，原檔不動。
    CorruptDefault,
    /// 使用者檔讀不到（權限、鎖定）；本次使用內建預設。
    UnreadableDefault,
}

/// 載入結果。
#[derive(Debug, Clone, PartialEq)]
pub struct WallpaperConfig {
    /// 合併後的設定，頂層一定是 object（`wallpaper` 通道 payload 的 `config`）。
    pub value: Value,
    pub source: ConfigSource,
    /// 需要使用者注意的事（缺鍵、型別錯、損壞、讀不到、寫出失敗）；以 `log::warn!` 記錄。
    pub warnings: Vec<String>,
    /// 一般資訊（已寫出預設檔）；以 `log::info!` 記錄。
    pub notes: Vec<String>,
}

/// 頂層合併結果（[`merge_top_level`]）。
#[derive(Debug, Clone, PartialEq)]
pub struct TopLevelMerge {
    pub value: Map<String, Value>,
    /// 缺鍵（全部合併成一則、列在最前）與型別不符（每鍵一則）的警告。
    pub warnings: Vec<String>,
}

/// 主題設定檔預設路徑：與 `settings.json` 同一個資料夾。
pub fn default_config_path() -> PathBuf {
    settings::default_settings_path().with_file_name(CONFIG_FILE_NAME)
}

/// 內建預設（已解析、行程內只解析一次）。
pub fn builtin_defaults() -> &'static Map<String, Value> {
    static DEFAULTS: OnceLock<Map<String, Value>> = OnceLock::new();
    DEFAULTS.get_or_init(|| {
        // 編進執行檔的固定內容，單元測試 builtin_defaults_parse_as_object_with_expected_top_level_keys
        // 保證它是合法的 JSON 物件；壞了是建置錯誤，不是執行期可恢復的情況。
        match serde_json::from_str(DEFAULT_CONFIG_JSON) {
            Ok(Value::Object(map)) => map,
            _ => panic!("內建預設 wallpaper-config.default.json 必須是頂層為物件的合法 JSON"),
        }
    })
}

/// JSON 值的型別名稱（合併時比對用；整數與小數同為 `number`）。
fn json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// 把使用者檔的頂層物件與內建預設合併（規則見模組文件「合併分工」）。不往下合併巢狀內容。
pub fn merge_top_level(defaults: &Map<String, Value>, user: Map<String, Value>) -> TopLevelMerge {
    let mut value = user;
    let mut warnings = Vec::new();
    let mut missing: Vec<&str> = Vec::new();
    for (key, default) in defaults {
        match value.get(key) {
            None => {
                missing.push(key);
                value.insert(key.clone(), default.clone());
            }
            Some(given) if json_type(given) != json_type(default) => {
                warnings.push(format!(
                    "{CONFIG_FILE_NAME} 的頂層鍵 {key} 型別應為 {}，實際是 {}，已改用內建預設",
                    json_type(default),
                    json_type(given)
                ));
                value.insert(key.clone(), default.clone());
            }
            Some(_) => {}
        }
    }
    if !missing.is_empty() {
        // 一次載入只記一行，點名所有缺鍵（spec 要求記警告；合併成一行以免逐鍵洗版）。
        warnings.insert(
            0,
            format!(
                "{CONFIG_FILE_NAME} 缺少頂層鍵 {}，已用內建預設補上",
                missing.join("、")
            ),
        );
    }
    TopLevelMerge { value, warnings }
}

/// 解析使用者檔：去掉 UTF-8 BOM 後必須是頂層為物件的合法 JSON。`Err`＝損壞的原因。
fn parse_user_config(bytes: &[u8]) -> Result<Map<String, Value>, String> {
    const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];
    let bytes = bytes.strip_prefix(UTF8_BOM).unwrap_or(bytes);
    match serde_json::from_slice::<Value>(bytes) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(other) => Err(format!("頂層應為物件，實際是 {}", json_type(&other))),
        Err(err) => Err(format!("不是合法的 JSON（{err}）")),
    }
}

/// 使用內建預設的結果。
fn builtin(source: ConfigSource, warnings: Vec<String>, notes: Vec<String>) -> WallpaperConfig {
    WallpaperConfig {
        value: Value::Object(builtin_defaults().clone()),
        source,
        warnings,
        notes,
    }
}

/// [`load_for_startup`] 與 [`reload`] 的共同實作；`create_if_missing` 決定缺檔時要不要寫出預設檔。
fn load(path: &Path, create_if_missing: bool) -> WallpaperConfig {
    let shown = path.display();
    match fs::read(path) {
        Ok(bytes) => match parse_user_config(&bytes) {
            Ok(user) => {
                let merged = merge_top_level(builtin_defaults(), user);
                WallpaperConfig {
                    value: Value::Object(merged.value),
                    source: ConfigSource::User,
                    warnings: merged.warnings,
                    notes: Vec::new(),
                }
            }
            Err(reason) => builtin(
                ConfigSource::CorruptDefault,
                vec![format!(
                    "主題設定檔 {shown} 損壞：{reason}；本次使用內建預設，原檔不動（修正後存檔即可；或刪除此檔，下次啟動會重新產生預設檔）"
                )],
                Vec::new(),
            ),
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            if !create_if_missing {
                return builtin(
                    ConfigSource::MissingDefault,
                    vec![format!(
                        "主題設定檔 {shown} 不存在；本次使用內建預設，下次啟動會重新產生"
                    )],
                    Vec::new(),
                );
            }
            create_then_load(path)
        }
        Err(err) => builtin(
            ConfigSource::UnreadableDefault,
            vec![format!(
                "主題設定檔 {shown} 讀取失敗（{err}）；本次使用內建預設"
            )],
            Vec::new(),
        ),
    }
}

/// 缺檔時寫出預設檔的結果（[`create_default_file`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CreateOutcome {
    /// 已寫出。
    Created,
    /// 寫出前目標已存在（判定缺檔之後才出現的檔案），沒有動它。
    AlreadyExists,
}

/// 以內建預設原文寫出 `path`，**目標已存在就不動它**（task 4.1 修正輪 1）：先寫同資料夾的
/// `<檔名>.tmp`，再以 `fs::hard_link(tmp, path)` 放上最終路徑——hard link 在目標已存在時失敗
/// （`AlreadyExists`），不像 `fs::rename` 會取代既有檔；成功時讀者也只會看到完整內容（原子）。
/// 不論成敗都清掉暫存檔。不用 `crate::settings::write_atomic`：那是覆蓋式 rename，判定缺檔與寫出
/// 之間若使用者剛建了檔就會被蓋掉。
///
/// 修正輪 2：寫暫存檔前先刪掉既有的 `.tmp`（NotFound 以外的錯誤即放棄），再以 `create_new` 開新檔。
/// 前一次 hard_link 成功、刪 `.tmp` 卻失敗（或行程在兩步之間結束）時，殘留的 `.tmp` 是使用者設定檔
/// 的第二個名字；直接 `fs::write` 會截斷並改寫同一份內容（例如使用者已改名的備份）。刪除只解除這個
/// 名字，不動其他連結；`create_new` 確保寫入的一定是剛建立的新檔。
fn create_default_file(path: &Path) -> std::io::Result<CreateOutcome> {
    use std::io::Write;

    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_file_name(format!("{CONFIG_FILE_NAME}.tmp"));
    match fs::remove_file(&tmp) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    let linked = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .and_then(|mut file| file.write_all(DEFAULT_CONFIG_JSON.as_bytes()))
        .and_then(|()| fs::hard_link(&tmp, path));
    let _ = fs::remove_file(&tmp);
    match linked {
        Ok(()) => Ok(CreateOutcome::Created),
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            Ok(CreateOutcome::AlreadyExists)
        }
        Err(err) => Err(err),
    }
}

/// 缺檔分支（僅 [`load_for_startup`]）：寫出預設檔；目標在寫出前已出現就改讀它。
fn create_then_load(path: &Path) -> WallpaperConfig {
    let shown = path.display();
    match create_default_file(path) {
        Ok(CreateOutcome::Created) => builtin(
            ConfigSource::CreatedDefault,
            Vec::new(),
            vec![format!("主題設定檔 {shown} 不存在，已寫出內建預設供編輯")],
        ),
        Ok(CreateOutcome::AlreadyExists) => load(path, false),
        Err(write_err) => builtin(
            ConfigSource::MissingDefault,
            vec![format!(
                "主題設定檔 {shown} 不存在，寫出預設檔失敗（{write_err}）；本次使用內建預設"
            )],
            Vec::new(),
        ),
    }
}

/// 把載入結果的警告與資訊交給 `sink`（等級＋訊息）；[`log_outcome`] 以 `log` crate 記錄，測試可換成收集器。
fn log_outcome_with(cfg: &WallpaperConfig, mut sink: impl FnMut(log::Level, &str)) {
    for warning in &cfg.warnings {
        sink(log::Level::Warn, warning);
    }
    for note in &cfg.notes {
        sink(log::Level::Info, note);
    }
}

/// 把載入結果的警告與資訊寫入記錄檔（警告 `log::warn!`、資訊 `log::info!`）。
fn log_outcome(cfg: &WallpaperConfig) {
    log_outcome_with(cfg, |level, message| log::log!(level, "{message}"));
}

/// 宿主啟動用：讀檔＋頂層合併；缺檔時以內建預設原文原子寫出一份。不會失敗。
pub fn load_for_startup(path: &Path) -> WallpaperConfig {
    let cfg = load(path, true);
    log_outcome(&cfg);
    cfg
}

/// 設定檔變更後重新讀取（task 4.6／4.7 的檔案監看呼叫）；**從不寫檔**，缺檔只記警告並使用
/// 內建預設。不會失敗。
pub fn reload(path: &Path) -> WallpaperConfig {
    let cfg = load(path, false);
    log_outcome(&cfg);
    cfg
}

/// 主題設定檔的變更監看（task 4.6：`wallpaper` 通道的 `config` 來源，見
/// `crate::data::ChannelRegistry::watch_wallpaper_config`）。由通道註冊表的 30 秒輪詢呼叫
/// [`ConfigWatch::poll`]，比照 `crate::data::JsonFileSource` 以「修改時間＋大小」判斷要不要重讀。
///
/// - 重讀走 [`reload`] 的同一條路徑（`load(path, false)`）：**從不寫檔**；缺檔以內建預設代替並記
///   警告。警告只在重讀時記（檔案沒變不重讀），不會每 30 秒洗版。
/// - **損壞或讀不到**（task 4.6 修正輪 2／3，複審 M1 的裁決；specs/wallpaper-themes「設定檔損壞」）：
///   1. 這個檔案版本第一次讀到問題時**只標記「待重讀」**（[`Self::retry_pending`]）：不等待、不退回、
///      不記錄，值不變。等待由排程層在**放掉註冊表的鎖之後**做（`crate::widgets::poll_and_notify_settled`：
///      等 [`CONFIG_RETRY_DELAY`] 再輪詢一次）——本函式在持有註冊表鎖時被呼叫，主執行緒的
///      `get_snapshot`／`update_settings` 也要取那把鎖，不得在這裡等待（design.md「核心不得同步等待」）。
///   2. 下一次輪詢看到待重讀，不論版本有沒有變都直接重讀並做最終判定：成功就照常採用（存檔途中的
///      競態因此不退回、不記警告）；仍失敗就**依 spec 改用內建預設照常更新**（內容改變就回傳 `true`，
///      通道推送、協調迴圈重畫），並記一則警告——文字來自 `load` 本身，含解析錯誤的原因（行、欄）並
///      寫明「本次使用內建預設」。
///   3. 記錄以**錯誤內容**去重：同一段錯誤持續不重記；使用者再存一次、錯在別處 → 再記一次；修好之後
///      又壞掉 → 再記一次。
///   4. 損壞：記下版本，檔案沒再變就不重讀、不再延後。讀不到（鎖檔、權限）：每輪重試（鎖解除時檔案
///      版本可能不變），但同一個版本只延後一次。
/// - [`ConfigWatch::poll`] 只在「合併後的內容」真的改變時回傳 `true`（只改排版、存同樣內容不推送）。
pub struct ConfigWatch {
    path: PathBuf,
    /// 上次處理過的版本（修改時間＋大小；內層 `None`＝檔案不存在）。外層 `None`＝從未處理。
    seen: Option<Option<(std::time::SystemTime, u64)>>,
    value: Value,
    /// 上次記錄的問題警告（錯誤內容；去重用）。檔案恢復正常時清掉。
    last_problem: Option<String>,
    /// 目前是「讀不到」：每輪都要重讀（鎖解除時版本可能不變）。
    retry_every_poll: bool,
    /// 新版本第一次讀到問題，等排程層延後重讀（見型別文件第 1 點）。
    retry_pending: bool,
    /// 合併後的內容改變過幾次（task 4.7a：協調迴圈以它判斷「主題設定檔變了、要重畫」）。
    changes: u64,
}

/// 監看讀到損壞／讀不到時，排程層延後多久再輪詢一次（task 4.6 修正輪 2／3；存檔途中的競態窗口）。
pub const CONFIG_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(1);

/// 檔案目前的版本（修改時間＋大小；不存在為 `None`）。
fn file_stamp(path: &Path) -> Option<(std::time::SystemTime, u64)> {
    fs::metadata(path)
        .ok()
        .and_then(|m| Some((m.modified().ok()?, m.len())))
}

fn is_problem(source: ConfigSource) -> bool {
    matches!(
        source,
        ConfigSource::CorruptDefault | ConfigSource::UnreadableDefault
    )
}

impl ConfigWatch {
    /// 尚未輪詢前的設定是內建預設。
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            seen: None,
            value: Value::Object(builtin_defaults().clone()),
            last_problem: None,
            retry_every_poll: false,
            retry_pending: false,
            changes: 0,
        }
    }

    /// 以啟動時已載入的結果（Primary 的 [`load_for_startup`]、Secondary 的 [`reload`]）開始監看
    /// （task 4.7a）：第一輪輪詢不重讀同一份檔、不重記同樣的警告。損壞的結果記下版本與錯誤內容
    /// （檔案沒變不重讀、不重記）；讀不到的結果下一輪照常重試（同一個錯誤不重記）。
    ///
    /// 版本取「此刻」的檔案修改時間＋大小：載入與這裡之間檔案剛好又被改掉時，要等下一次改動才會
    /// 讀到（兩者相隔只有同一個函式內的幾個呼叫）。
    pub fn primed(path: PathBuf, startup: &WallpaperConfig) -> Self {
        let problem = is_problem(startup.source);
        Self {
            seen: Some(file_stamp(&path)),
            value: startup.value.clone(),
            last_problem: problem.then(|| startup.warnings.join("；")),
            retry_every_poll: startup.source == ConfigSource::UnreadableDefault,
            retry_pending: false,
            changes: 0,
            path,
        }
    }

    /// 合併後的內容改變過幾次（[`Self::poll`] 回傳 `true` 的次數）。
    pub fn changes(&self) -> u64 {
        self.changes
    }

    /// 新版本第一次讀到問題、等排程層延後重讀中（見型別文件第 1 點）。
    pub fn retry_pending(&self) -> bool {
        self.retry_pending
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 目前的設定（頂層合併後，頂層一定是 object）。
    pub fn value(&self) -> &Value {
        &self.value
    }

    /// 檢查設定檔；合併後的內容改變時回傳 `true`（規則見型別文件）。
    pub fn poll(&mut self) -> bool {
        self.poll_with(|level, message| log::log!(level, "{message}"))
    }

    /// [`Self::poll`] 的本體；記錄交給 `sink`（等級＋訊息），測試可換成收集器。**從不等待**。
    pub fn poll_with(&mut self, mut sink: impl FnMut(log::Level, &str)) -> bool {
        // 版本在讀檔**前**取：讀完之後檔案又被改掉時，下一輪看到的版本不同、會再讀。
        let stamp = file_stamp(&self.path);
        let same_version = self.seen == Some(stamp);
        if same_version && !self.retry_every_poll && !self.retry_pending {
            return false;
        }
        let cfg = load(&self.path, false);
        if is_problem(cfg.source) && !same_version && !self.retry_pending {
            // 這個版本第一次出問題：可能是存檔途中——只標記待重讀，等待交給排程層（鎖外）。
            self.retry_pending = true;
            return false;
        }
        self.retry_pending = false;
        self.seen = Some(stamp);
        if is_problem(cfg.source) {
            // 依 spec 改用內建預設（`cfg.value` 就是內建預設）；同一段錯誤不重記。
            let text = cfg.warnings.join("；");
            if self.last_problem.as_deref() != Some(text.as_str()) {
                log_outcome_with(&cfg, &mut sink);
                self.last_problem = Some(text);
            }
            self.retry_every_poll = cfg.source == ConfigSource::UnreadableDefault;
        } else {
            self.last_problem = None;
            self.retry_every_poll = false;
            log_outcome_with(&cfg, &mut sink);
        }
        if cfg.value == self.value {
            return false;
        }
        self.value = cfg.value;
        self.changes += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    /// 每個測試一個獨立的暫存資料夾（行程 id＋測試名），測試前清空；絕不碰真正的
    /// `%APPDATA%`／`%LOCALAPPDATA%`。
    fn temp_dir(name: &str) -> PathBuf {
        let dir = env::temp_dir()
            .join(format!(
                "fc-host-wallpaper-config-test-{}",
                std::process::id()
            ))
            .join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("建立暫存資料夾失敗");
        dir
    }

    fn dir_entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .expect("讀取暫存資料夾失敗")
            .map(|e| {
                e.expect("讀取目錄項目失敗")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }

    fn defaults_value() -> Value {
        serde_json::from_str(DEFAULT_CONFIG_JSON).expect("內建預設應為合法 JSON")
    }

    fn write_json(path: &Path, value: &Value) -> Vec<u8> {
        let raw = serde_json::to_vec_pretty(value).expect("序列化失敗");
        fs::write(path, &raw).expect("寫入測試檔失敗");
        raw
    }

    #[test]
    fn builtin_defaults_parse_as_object_with_expected_top_level_keys() {
        let d = builtin_defaults();
        for key in [
            "version",
            "markets",
            "holidays",
            "holidaySources",
            "orderSeasons",
            "earningsWeek",
            "thresholds",
            "phrases",
        ] {
            assert!(d.contains_key(key), "內建預設缺頂層鍵 {key}");
        }
        assert_eq!(Value::Object(d.clone()), defaults_value());
    }

    #[test]
    fn default_config_path_sits_beside_settings_json() {
        let p = default_config_path();
        assert_eq!(
            p.file_name().and_then(|n| n.to_str()),
            Some(CONFIG_FILE_NAME)
        );
        assert_eq!(
            p.parent(),
            settings::default_settings_path().parent(),
            "應與 settings.json 同一個資料夾"
        );
    }

    // ── 缺檔 ─────────────────────────────────────────────────────────────────────

    #[test]
    fn startup_with_missing_file_writes_builtin_default_verbatim() {
        let dir = temp_dir("missing-startup");
        let path = dir.join("nested").join(CONFIG_FILE_NAME);

        let cfg = load_for_startup(&path);

        assert_eq!(cfg.source, ConfigSource::CreatedDefault);
        assert_eq!(cfg.value, defaults_value());
        assert!(cfg.warnings.is_empty(), "{:?}", cfg.warnings);
        assert!(!cfg.notes.is_empty(), "寫出預設檔應記一筆資訊");
        assert_eq!(
            fs::read(&path).expect("應已寫出預設檔"),
            DEFAULT_CONFIG_JSON.as_bytes(),
            "寫出的內容應與內建預設逐位元組相同"
        );
        assert_eq!(
            dir_entries(path.parent().expect("有父資料夾")),
            vec![CONFIG_FILE_NAME.to_string()],
            "原子寫入不應殘留暫存檔"
        );

        // 第二次啟動讀到剛寫出的檔：使用者來源、無警告、不重寫。
        let again = load_for_startup(&path);
        assert_eq!(again.source, ConfigSource::User);
        assert_eq!(again.value, defaults_value());
        assert!(again.warnings.is_empty() && again.notes.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reload_with_missing_file_uses_default_without_writing() {
        let dir = temp_dir("missing-reload");
        let path = dir.join(CONFIG_FILE_NAME);

        let cfg = reload(&path);

        assert_eq!(cfg.source, ConfigSource::MissingDefault);
        assert_eq!(cfg.value, defaults_value());
        assert_eq!(cfg.warnings.len(), 1, "{:?}", cfg.warnings);
        assert!(!path.exists(), "reload 不得寫檔");
        assert!(dir_entries(&dir).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    // ── 缺鍵 ─────────────────────────────────────────────────────────────────────

    #[test]
    fn missing_keys_are_filled_from_default_with_one_warning_naming_them() {
        let dir = temp_dir("missing-keys");
        let path = dir.join(CONFIG_FILE_NAME);
        let user_thresholds = serde_json::json!({ "shortRatio": 3 });
        let raw = write_json(&path, &serde_json::json!({ "thresholds": user_thresholds }));

        let cfg = load_for_startup(&path);

        assert_eq!(cfg.source, ConfigSource::User);
        // spec「缺欄位時……記錄警告」：缺鍵記 warn 等級（warnings），每次載入合併成一行點名所有缺鍵。
        assert!(cfg.notes.is_empty(), "缺鍵不得降為資訊：{:?}", cfg.notes);
        assert_eq!(
            cfg.warnings.len(),
            1,
            "缺鍵合併成一則警告：{:?}",
            cfg.warnings
        );
        let d = defaults_value();
        let missing: Vec<&String> = d
            .as_object()
            .expect("物件")
            .keys()
            .filter(|k| k.as_str() != "thresholds")
            .collect();
        for key in &missing {
            assert_eq!(
                cfg.value[key.as_str()],
                d[key.as_str()],
                "缺的 {key} 應補預設"
            );
            assert!(
                cfg.warnings[0].contains(key.as_str()),
                "警告應點名缺的鍵 {key}：{:?}",
                cfg.warnings
            );
        }
        assert!(
            !cfg.warnings[0].contains("thresholds"),
            "有給的鍵不應被列為缺鍵：{:?}",
            cfg.warnings
        );
        assert_eq!(
            cfg.value["thresholds"], user_thresholds,
            "同型別頂層鍵整個採用使用者的值；巢狀補齊交給頁面 mergeConfig"
        );
        assert_eq!(fs::read(&path).expect("原檔"), raw, "讀檔不得改寫使用者檔");
        let _ = fs::remove_dir_all(&dir);
    }

    // ── 型別錯 ───────────────────────────────────────────────────────────────────

    #[test]
    fn wrong_type_key_falls_back_to_default_with_warning_others_kept() {
        let dir = temp_dir("wrong-type");
        let path = dir.join(CONFIG_FILE_NAME);
        let mut user = defaults_value();
        let obj = user.as_object_mut().expect("物件");
        obj.insert("markets".into(), serde_json::json!({ "TPE": {} })); // 應為陣列
        obj.insert("phrases".into(), serde_json::json!("宜：寫程式")); // 應為物件
        obj.insert("version".into(), serde_json::json!("1")); // 應為數字
        obj.insert("earningsWeek".into(), Value::Null); // null 也算型別不符
        let user_orders = serde_json::json!([{ "label": "自訂旺季", "months": [1] }]);
        obj.insert("orderSeasons".into(), user_orders.clone());
        obj.insert(
            "thresholds".into(),
            serde_json::json!({ "shortRatio": 2.5 }),
        );
        write_json(&path, &user);

        let cfg = load_for_startup(&path);

        assert_eq!(cfg.source, ConfigSource::User);
        let d = defaults_value();
        for key in ["markets", "phrases", "version", "earningsWeek"] {
            assert_eq!(cfg.value[key], d[key], "{key} 型別錯應採預設");
            assert!(
                cfg.warnings.iter().any(|w| w.contains(key)),
                "應有一則點名 {key} 的警告：{:?}",
                cfg.warnings
            );
        }
        assert_eq!(cfg.warnings.len(), 4, "{:?}", cfg.warnings);
        assert_eq!(
            cfg.value["orderSeasons"], user_orders,
            "其他鍵保留使用者的值"
        );
        assert_eq!(
            cfg.value["thresholds"],
            serde_json::json!({ "shortRatio": 2.5 })
        );
        assert_eq!(cfg.value["holidays"], d["holidays"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn integer_and_float_are_the_same_json_type() {
        let d = builtin_defaults();
        let mut user = d.clone();
        user.insert("version".into(), serde_json::json!(1.5));
        let merged = merge_top_level(d, user);
        assert_eq!(merged.value["version"], serde_json::json!(1.5));
        assert!(merged.warnings.is_empty(), "{:?}", merged.warnings);
    }

    // ── 多出來的頂層鍵 ───────────────────────────────────────────────────────────

    #[test]
    fn extra_top_level_keys_are_kept_untouched() {
        let dir = temp_dir("extra-keys");
        let path = dir.join(CONFIG_FILE_NAME);
        let mut user = defaults_value();
        let obj = user.as_object_mut().expect("物件");
        obj.insert("myNote".into(), serde_json::json!("2027 年記得補 TPE"));
        obj.insert(
            "futureKey".into(),
            serde_json::json!({ "a": [1, null, "x"] }),
        );
        obj.insert("nullKey".into(), Value::Null);
        write_json(&path, &user);

        let cfg = load_for_startup(&path);

        assert_eq!(cfg.value, user, "多出的頂層鍵（含 null 值）原樣保留");
        assert!(cfg.warnings.is_empty() && cfg.notes.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    // ── 損壞 ─────────────────────────────────────────────────────────────────────

    #[test]
    fn corrupt_file_uses_default_with_warning_and_leaves_bytes_untouched() {
        let cases: [(&str, &[u8]); 6] = [
            ("not-json", b"{ \"markets\": [ oops"),
            ("empty", b""),
            ("top-level-array", b"[1, 2, 3]"),
            ("top-level-string", b"\"markets\""),
            ("top-level-null", b"null"),
            ("not-utf8", &[0xFF, 0xFE, b'{', 0x00, b'}', 0x00]),
        ];
        for (name, raw) in cases {
            let dir = temp_dir(&format!("corrupt-{name}"));
            let path = dir.join(CONFIG_FILE_NAME);
            fs::write(&path, raw).expect("寫入測試檔失敗");

            for cfg in [load_for_startup(&path), reload(&path)] {
                assert_eq!(cfg.source, ConfigSource::CorruptDefault, "{name}");
                assert_eq!(cfg.value, defaults_value(), "{name}：本次用內建預設");
                assert_eq!(cfg.warnings.len(), 1, "{name}：{:?}", cfg.warnings);
                assert!(
                    cfg.warnings[0].contains(CONFIG_FILE_NAME),
                    "{name}：警告應點出檔案：{:?}",
                    cfg.warnings
                );
            }
            assert_eq!(
                fs::read(&path).expect("原檔應仍在"),
                raw,
                "{name}：位元組不變"
            );
            assert_eq!(
                dir_entries(&dir),
                vec![CONFIG_FILE_NAME.to_string()],
                "{name}：不得改名、備份或另寫檔案"
            );
            let _ = fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn utf8_bom_is_accepted() {
        let dir = temp_dir("bom");
        let path = dir.join(CONFIG_FILE_NAME);
        let mut raw = vec![0xEF, 0xBB, 0xBF];
        raw.extend_from_slice(DEFAULT_CONFIG_JSON.as_bytes());
        fs::write(&path, &raw).expect("寫入測試檔失敗");

        let cfg = reload(&path);

        assert_eq!(cfg.source, ConfigSource::User);
        assert_eq!(cfg.value, defaults_value());
        assert!(cfg.warnings.is_empty(), "{:?}", cfg.warnings);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unreadable_path_uses_default_with_warning_without_writing() {
        // 以資料夾冒充設定檔：fs::read 失敗但不是 NotFound。
        let dir = temp_dir("unreadable");
        let path = dir.join(CONFIG_FILE_NAME);
        fs::create_dir_all(&path).expect("建立資料夾失敗");

        let cfg = load_for_startup(&path);

        assert_eq!(cfg.source, ConfigSource::UnreadableDefault);
        assert_eq!(cfg.value, defaults_value());
        assert_eq!(cfg.warnings.len(), 1, "{:?}", cfg.warnings);
        assert!(path.is_dir(), "不得動到原路徑");
        let _ = fs::remove_dir_all(&dir);
    }

    // ── 缺檔寫出：不覆寫判定缺檔之後才出現的檔案、寫出失敗不 panic（task 4.1 修正輪 1）──────

    #[test]
    fn default_creation_never_overwrites_a_file_that_appeared_after_the_missing_check() {
        let dir = temp_dir("appeared-meanwhile");
        let path = dir.join(CONFIG_FILE_NAME);
        // 模擬「fs::read 回 NotFound 之後、寫出之前」使用者或另一個行程建好了檔案。
        let mut user = defaults_value();
        user.as_object_mut()
            .expect("物件")
            .insert("myNote".into(), serde_json::json!("我剛建的"));
        let raw = write_json(&path, &user);

        assert_eq!(
            create_default_file(&path).expect("目標已存在不算錯誤"),
            CreateOutcome::AlreadyExists
        );
        let cfg = create_then_load(&path);

        assert_eq!(cfg.source, ConfigSource::User, "應改讀使用者的檔");
        assert_eq!(cfg.value["myNote"], serde_json::json!("我剛建的"));
        assert_eq!(fs::read(&path).expect("原檔"), raw, "不得覆寫使用者的檔");
        assert_eq!(
            dir_entries(&dir),
            vec![CONFIG_FILE_NAME.to_string()],
            "失敗時要清掉 .tmp"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_tmp_hard_link_does_not_write_through_to_a_renamed_backup() {
        // 前一次寫出 hard_link 成功、但刪 .tmp 失敗（或行程在兩步之間結束）：殘留的 .tmp 是使用者
        // 設定檔的第二個名字。使用者之後把設定檔改名備份，下次缺檔寫出不得經由 .tmp 改寫那份備份。
        let dir = temp_dir("stale-tmp-link");
        let path = dir.join(CONFIG_FILE_NAME);
        let tmp = dir.join(format!("{CONFIG_FILE_NAME}.tmp"));
        let backup = dir.join("wallpaper-config.backup.json");
        let mut user = defaults_value();
        user.as_object_mut()
            .expect("物件")
            .insert("myNote".into(), serde_json::json!("要留著的備份"));
        let raw = write_json(&path, &user);
        fs::hard_link(&path, &tmp).expect("建立殘留的 .tmp 連結失敗");
        fs::rename(&path, &backup).expect("模擬改名備份失敗");

        let cfg = load_for_startup(&path);

        assert_eq!(cfg.source, ConfigSource::CreatedDefault);
        assert_eq!(cfg.value, defaults_value());
        assert_eq!(
            fs::read(&backup).expect("備份"),
            raw,
            "備份的位元組不得改變"
        );
        assert_eq!(
            fs::read(&path).expect("新設定檔"),
            DEFAULT_CONFIG_JSON.as_bytes(),
            "新設定檔應為內建預設"
        );
        assert!(!tmp.exists(), "不得殘留 .tmp");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_write_failure_uses_default_logs_warn_and_leaves_no_tmp() {
        // 父路徑是一般檔案：fs::read 回 NotFound，建立資料夾／寫暫存檔都會失敗。
        let dir = temp_dir("write-failure");
        let blocker = dir.join("blocker");
        fs::write(&blocker, b"not a directory").expect("寫入測試檔失敗");
        let path = blocker.join(CONFIG_FILE_NAME);

        let cfg = load_for_startup(&path);

        assert_eq!(cfg.source, ConfigSource::MissingDefault);
        assert_eq!(cfg.value, defaults_value(), "本次用內建預設");
        assert_eq!(cfg.warnings.len(), 1, "{:?}", cfg.warnings);
        assert!(
            cfg.warnings[0].contains("寫出預設檔失敗"),
            "{:?}",
            cfg.warnings
        );
        let mut logged = Vec::new();
        log_outcome_with(&cfg, |level, message| {
            logged.push((level, message.to_string()))
        });
        assert_eq!(
            logged,
            vec![(log::Level::Warn, cfg.warnings[0].clone())],
            "寫出失敗以 warn 等級記錄"
        );
        assert_eq!(
            dir_entries(&dir),
            vec!["blocker".to_string()],
            "不得殘留 .tmp"
        );
        assert_eq!(fs::read(&blocker).expect("blocker"), b"not a directory");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn created_default_is_logged_at_info_and_missing_keys_at_warn() {
        let dir = temp_dir("log-levels");
        let path = dir.join(CONFIG_FILE_NAME);
        let mut logged = Vec::new();
        log_outcome_with(&load_for_startup(&path), |level, _| logged.push(level));
        assert_eq!(logged, vec![log::Level::Info], "寫出預設檔只是資訊");

        write_json(
            &path,
            &serde_json::json!({ "thresholds": { "shortRatio": 3 } }),
        );
        let mut logged = Vec::new();
        log_outcome_with(&reload(&path), |level, _| logged.push(level));
        assert_eq!(logged, vec![log::Level::Warn], "缺鍵記一則 warn");
        let _ = fs::remove_dir_all(&dir);
    }

    // ── task 4.6：ConfigWatch（wallpaper 通道的設定來源）──────────────────────────────

    /// mtime 解析度可能只有約 10ms；改檔前後睡一下，確保修改時間往前推進。
    fn settle() {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    #[test]
    fn config_watch_first_poll_loads_user_file_then_skips_unchanged() {
        let dir = temp_dir("watch-first");
        let path = dir.join(CONFIG_FILE_NAME);
        let mut user = defaults_value();
        user["thresholds"] = serde_json::json!({ "marker": "user" });
        write_json(&path, &user);
        settle();

        let mut watch = ConfigWatch::new(path.clone());
        assert_eq!(watch.value(), &defaults_value(), "尚未輪詢前是內建預設");
        assert!(
            watch.poll(),
            "第一次輪詢讀到使用者檔，內容不同於內建預設 → 變更"
        );
        assert_eq!(watch.value()["thresholds"]["marker"], "user");
        assert!(!watch.poll(), "檔案未變 → 不算變更");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_watch_reports_change_only_when_content_changes() {
        let dir = temp_dir("watch-change");
        let path = dir.join(CONFIG_FILE_NAME);
        let mut user = defaults_value();
        user["thresholds"] = serde_json::json!({ "marker": "v1" });
        write_json(&path, &user);
        settle();
        let mut watch = ConfigWatch::new(path.clone());
        assert!(watch.poll());

        user["thresholds"] = serde_json::json!({ "marker": "v2" });
        write_json(&path, &user);
        settle();
        assert!(watch.poll(), "內容改變 → 變更");
        assert_eq!(watch.value()["thresholds"]["marker"], "v2");

        // 只改排版（大小不同、內容相同）：重讀但合併結果不變 → 不算變更。
        fs::write(&path, serde_json::to_vec(&user).expect("序列化失敗")).expect("寫檔失敗");
        settle();
        assert!(!watch.poll(), "合併結果相同 → 不推送");
        let _ = fs::remove_dir_all(&dir);
    }

    /// task 4.7a：啟動時 `load_for_startup` 的結果交給監看——第一輪輪詢不重讀、不重記警告；缺檔時
    /// 確實寫出預設檔，之後檔案改變照常重讀，`changes` 只在內容改變時遞增。
    #[test]
    fn config_watch_primed_with_startup_result_skips_first_reload_and_relog() {
        let dir = temp_dir("watch-primed");
        let path = dir.join(CONFIG_FILE_NAME);
        // 缺檔：Primary 啟動寫出預設檔。
        let cfg = load_for_startup(&path);
        assert_eq!(cfg.source, ConfigSource::CreatedDefault);
        assert!(path.exists(), "啟動時要確實產生預設檔");
        settle();
        let mut watch = ConfigWatch::primed(path.clone(), &cfg);
        assert_eq!(watch.value(), &cfg.value);
        assert_eq!(watch.changes(), 0);
        let mut logged = Vec::new();
        assert!(!watch.poll_with(|l, m| logged.push((l, m.to_owned()))));
        assert!(logged.is_empty(), "第一輪不重記：{logged:?}");

        // 有缺鍵的使用者檔：啟動記一次警告，監看第一輪不再記。
        let mut user = defaults_value();
        user.as_object_mut().unwrap().remove("thresholds");
        write_json(&path, &user);
        settle();
        let cfg = reload(&path);
        assert!(!cfg.warnings.is_empty());
        let mut watch = ConfigWatch::primed(path.clone(), &cfg);
        let mut logged = Vec::new();
        assert!(!watch.poll_with(|l, m| logged.push((l, m.to_owned()))));
        assert!(logged.is_empty(), "缺鍵警告不重複：{logged:?}");

        user["thresholds"] = serde_json::json!({ "marker": "edited" });
        write_json(&path, &user);
        settle();
        assert!(watch.poll(), "之後的修改照常重讀");
        assert_eq!(watch.changes(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    /// 損壞的檔交給監看：保留啟動結果（內建預設），持續損壞不重記，修好後讀入。
    #[test]
    fn config_watch_primed_with_corrupt_startup_result_retries_without_relogging() {
        let dir = temp_dir("watch-primed-corrupt");
        let path = dir.join(CONFIG_FILE_NAME);
        fs::write(&path, b"{ broken").expect("寫檔失敗");
        settle();
        let cfg = load_for_startup(&path);
        assert_eq!(cfg.source, ConfigSource::CorruptDefault);
        let mut watch = ConfigWatch::primed(path.clone(), &cfg);
        let mut logged = Vec::new();
        assert!(!watch.poll_with(|l, m| logged.push((l, m.to_owned()))));
        assert!(logged.is_empty(), "同一個損壞不重記：{logged:?}");
        let mut user = defaults_value();
        user["thresholds"] = serde_json::json!({ "marker": "fixed" });
        write_json(&path, &user);
        settle();
        assert!(watch.poll());
        assert_eq!(watch.value()["thresholds"]["marker"], "fixed");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_watch_missing_file_uses_defaults_and_never_writes() {
        let dir = temp_dir("watch-missing");
        let path = dir.join(CONFIG_FILE_NAME);
        let mut watch = ConfigWatch::new(path.clone());
        assert!(!watch.poll(), "缺檔＝內建預設，與初始值相同 → 不算變更");
        assert_eq!(watch.value(), &defaults_value());
        assert!(!path.exists(), "監看從不寫檔");
        assert!(dir_entries(&dir).is_empty(), "{:?}", dir_entries(&dir));

        // 之後使用者建立檔案 → 讀入。
        let mut user = defaults_value();
        user["thresholds"] = serde_json::json!({ "marker": "late" });
        write_json(&path, &user);
        settle();
        assert!(watch.poll());
        assert_eq!(watch.value()["thresholds"]["marker"], "late");
        let _ = fs::remove_dir_all(&dir);
    }

    // ── task 4.6 修正輪 2／3（複審 M1）：損壞或讀不到 → 延後重讀一次（等待在鎖外，由排程層做）→
    //    仍失敗才依 spec 退回內建預設 ────────────────────────────────────────────────────────

    fn collect_poll(watch: &mut ConfigWatch) -> (bool, Vec<(log::Level, String)>) {
        let mut logged = Vec::new();
        let changed = watch.poll_with(|level, message| logged.push((level, message.to_owned())));
        (changed, logged)
    }

    fn user_with_marker(marker: &str) -> Value {
        let mut user = defaults_value();
        user["thresholds"] = serde_json::json!({ "marker": marker });
        user
    }

    /// 新版本第一次出問題：只標記「待重讀」——不等待、不退回、不記錄、不推送。
    fn assert_deferred(watch: &mut ConfigWatch, still: &str) {
        let (changed, logged) = collect_poll(watch);
        assert!(!changed, "第一次讀到問題只標記待重讀");
        assert!(logged.is_empty(), "第一次讀到問題不記錄：{logged:?}");
        assert!(watch.retry_pending(), "應標記待重讀");
        assert_eq!(
            watch.value()["thresholds"]["marker"],
            still,
            "待重讀期間值不變"
        );
    }

    /// 存檔途中讀到半個檔：標記待重讀；排程層等過之後重讀讀到完整的新內容——直接採用，不退回、不記警告。
    #[test]
    fn config_watch_mid_save_retry_reads_completed_file_without_fallback() {
        let dir = temp_dir("watch-mid-save");
        let path = dir.join(CONFIG_FILE_NAME);
        write_json(&path, &user_with_marker("v1"));
        settle();
        let mut watch = ConfigWatch::new(path.clone());
        assert!(collect_poll(&mut watch).0);
        assert!(!watch.retry_pending());

        let full = serde_json::to_vec_pretty(&user_with_marker("v2")).expect("序列化失敗");
        fs::write(&path, &full[..full.len() / 2]).expect("寫檔失敗");
        settle();
        assert_deferred(&mut watch, "v1");

        // 等待期間存檔完成。
        fs::write(&path, &full).expect("寫檔失敗");
        settle();
        let (changed, logged) = collect_poll(&mut watch);
        assert!(changed, "重讀讀到新內容 → 變更");
        assert!(!watch.retry_pending());
        assert_eq!(watch.value()["thresholds"]["marker"], "v2");
        assert!(logged.is_empty(), "存檔競態不應留下警告：{logged:?}");
        assert_eq!(watch.changes(), 2);
        let _ = fs::remove_dir_all(&dir);
    }

    /// 重讀後仍損壞：依 spec「設定檔損壞」以內建預設照常更新，警告帶解析錯誤的原因並寫明改用內建預設；
    /// 檔案沒再變就不重讀、不重記。
    #[test]
    fn config_watch_persistent_corruption_falls_back_to_defaults_with_reason() {
        let dir = temp_dir("watch-persistent-corrupt");
        let path = dir.join(CONFIG_FILE_NAME);
        write_json(&path, &user_with_marker("v1"));
        settle();
        let mut watch = ConfigWatch::new(path.clone());
        assert!(collect_poll(&mut watch).0);

        fs::write(&path, b"{ \"version\": 1, oops").expect("寫檔失敗");
        settle();
        assert_deferred(&mut watch, "v1");
        let (changed, logged) = collect_poll(&mut watch);
        assert!(changed, "改用內建預設＝內容改變，要推送並重畫");
        assert!(!watch.retry_pending());
        assert_eq!(watch.value(), &defaults_value());
        assert_eq!(logged.len(), 1, "{logged:?}");
        assert_eq!(logged[0].0, log::Level::Warn);
        assert!(logged[0].1.contains("內建預設"), "{}", logged[0].1);
        assert!(
            logged[0].1.contains("不是合法的 JSON") && logged[0].1.contains("line"),
            "警告要帶解析錯誤的原因：{}",
            logged[0].1
        );

        let (changed, logged) = collect_poll(&mut watch);
        assert!(!changed);
        assert!(logged.is_empty(), "同一份損壞檔不重記：{logged:?}");
        assert!(!watch.retry_pending(), "檔案沒變不再延後重讀");
        let _ = fs::remove_dir_all(&dir);
    }

    /// 去重看錯誤內容：使用者再存一次、錯在別處 → 再記一次；存回同樣的錯 → 不重記；修好 → 讀入。
    #[test]
    fn config_watch_relogs_when_corruption_changes_after_resave() {
        let dir = temp_dir("watch-corrupt-resave");
        let path = dir.join(CONFIG_FILE_NAME);
        let mut watch = ConfigWatch::new(path.clone());
        // 兩輪：第一輪標記待重讀，第二輪做最終判定。
        fn two_polls(watch: &mut ConfigWatch) -> Vec<(log::Level, String)> {
            let (_, first) = collect_poll(watch);
            assert!(first.is_empty(), "{first:?}");
            assert!(watch.retry_pending());
            collect_poll(watch).1
        }

        fs::write(&path, b"{ \"version\": 1, oops").expect("寫檔失敗");
        settle();
        let first = two_polls(&mut watch);
        assert_eq!(first.len(), 1, "{first:?}");

        fs::write(&path, b"{\n  \"version\": 1,\n  \"markets\": [ broken\n").expect("寫檔失敗");
        settle();
        let second = two_polls(&mut watch);
        assert_eq!(second.len(), 1, "錯誤內容不同要再記：{second:?}");
        assert_ne!(first[0].1, second[0].1);

        // 再存一次同樣的錯誤內容（修改時間變了＝新版本、會重讀，但錯誤相同）：不重記。
        std::thread::sleep(std::time::Duration::from_millis(50));
        fs::write(&path, b"{\n  \"version\": 1,\n  \"markets\": [ broken\n").expect("寫檔失敗");
        settle();
        let third = two_polls(&mut watch);
        assert!(third.is_empty(), "錯誤內容相同不重記：{third:?}");

        write_json(&path, &user_with_marker("fixed"));
        settle();
        let (changed, logged) = collect_poll(&mut watch);
        assert!(changed);
        assert!(logged.is_empty(), "{logged:?}");
        assert_eq!(watch.value()["thresholds"]["marker"], "fixed");

        // 修好之後又壞成第一種錯：再記一次（問題已解除過）。
        fs::write(&path, b"{ \"version\": 1, oops").expect("寫檔失敗");
        settle();
        let again = two_polls(&mut watch);
        assert_eq!(again.len(), 1, "{again:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// 讀不到（編輯器鎖檔）：延後重讀一次仍讀不到 → 依 spec 改用內建預設並記警告；鎖住期間每輪重試
    /// （不再延後、不重記），解除後即使檔案未變也讀入。
    #[cfg(windows)]
    #[test]
    fn config_watch_locked_file_falls_back_then_recovers_without_file_change() {
        use std::os::windows::fs::OpenOptionsExt;

        let dir = temp_dir("watch-locked");
        let path = dir.join(CONFIG_FILE_NAME);
        write_json(&path, &user_with_marker("v1"));
        settle();
        let mut watch = ConfigWatch::new(path.clone());
        assert!(collect_poll(&mut watch).0);

        write_json(&path, &user_with_marker("v2"));
        settle();
        let lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .expect("以獨占模式開檔失敗");
        assert_deferred(&mut watch, "v1");
        let (changed, logged) = collect_poll(&mut watch);
        assert!(changed, "改用內建預設");
        assert_eq!(watch.value(), &defaults_value());
        assert_eq!(logged.len(), 1, "{logged:?}");
        assert!(
            logged[0].1.contains("讀取失敗") && logged[0].1.contains("內建預設"),
            "{}",
            logged[0].1
        );

        let (changed, logged) = collect_poll(&mut watch);
        assert!(!changed);
        assert!(logged.is_empty(), "{logged:?}");
        assert!(!watch.retry_pending(), "持續鎖住：每輪重試但不再延後");

        drop(lock);
        assert!(
            collect_poll(&mut watch).0,
            "解除鎖定後（檔案未再變）也要重讀"
        );
        assert_eq!(watch.value()["thresholds"]["marker"], "v2");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_watch_first_load_of_corrupt_file_uses_builtin_defaults() {
        let dir = temp_dir("watch-corrupt-first");
        let path = dir.join(CONFIG_FILE_NAME);
        fs::write(&path, b"{ \"version\": 1, ").expect("寫檔失敗");
        settle();
        let mut watch = ConfigWatch::new(path.clone());
        let (changed, logged) = collect_poll(&mut watch);
        assert!(!changed && logged.is_empty() && watch.retry_pending());
        let (changed, logged) = collect_poll(&mut watch);
        assert!(!changed, "初始值本來就是內建預設 → 不算變更");
        assert_eq!(watch.value(), &defaults_value());
        assert_eq!(logged.len(), 1, "{logged:?}");
        assert_eq!(logged[0].0, log::Level::Warn);
        assert!(logged[0].1.contains("內建預設"), "{}", logged[0].1);

        write_json(&path, &user_with_marker("fixed"));
        settle();
        assert!(collect_poll(&mut watch).0);
        assert_eq!(watch.value()["thresholds"]["marker"], "fixed");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_retry_delay_is_one_to_two_seconds() {
        assert!(
            (std::time::Duration::from_secs(1)..=std::time::Duration::from_secs(2))
                .contains(&CONFIG_RETRY_DELAY)
        );
    }
}
