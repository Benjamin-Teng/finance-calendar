//! 資料來源抽象與通道註冊表（design.md D5）。
//!
//! - [`DataSource`]：`poll() -> Option<Snapshot>`。`None` 代表「這次沒有新東西」——
//!   來源檔未變、解析失敗、或驗證未通過皆回傳 `None`；呼叫端（[`ChannelRegistry`]）
//!   只在收到 `Some` 時才覆蓋既有快照，因此「解析失敗／全空時保留舊快照」不需要
//!   `DataSource` 自己記住上一份快照，只要「不回傳新的」就自動等於沒發生事。
//! - [`ChannelRegistry`]：通道名 → [`DataSource`] 實例的註冊表，`poll_all()` 逐一輪詢、
//!   `snapshot(channel)` 查目前快照（`None`＝該通道從未取得有效快照＝「尚無資料」）。
//! - [`JsonFileSource`]：以檔案修改時間＋大小判斷是否需要重讀（design.md D5：每 30 秒
//!   比對，有變才讀取／解析／驗證）；通過驗證才回傳新快照。
//! - [`build_default_registry`]：本 change 提供的六個通道——`tw-events`（來源
//!   `tw_events.json`，套用移植自 Lively 版 `isEmptyPayload` 的全空拒收）與
//!   `custom1`–`custom5`（來源 `customN.json`，任意合法 JSON 皆可）。
//! - dynamic-wallpaper task 4.6：衍生通道 [`WALLPAPER_CHANNEL`]（`wallpaper`）——不是檔案來源，
//!   由 `tw-events` 快照擷取 [`WALLPAPER_DATA_KEYS`]＋主題設定組成 `{ data, config }`，以
//!   [`ChannelRegistry::watch_wallpaper_config`] 開啟（`crate::widgets::AppState::new` 呼叫）。
//!   只有桌布渲染視窗能查詢／訂閱，閘門在 `crate::widgets`（`may_access_channel`）。
//!
//! 30 秒輪詢排程與「有變更才通知訂閱小工具」的事件推送是 task 2.7 的工作（IPC／事件
//! 層）；本檔只提供 `poll()` 這個可被排程呼叫的建構區塊。
//!
//! 本 task（2.6）尚未有任何呼叫端把 [`ChannelRegistry`] 接進 `main.rs`／IPC（task 2.7
//! 負責），故公開項目在 `cargo clippy` 下會是 dead_code；比照 `layout.rs`、`desktop.rs`
//! 的既有做法，以模組層級 `allow` 抑制，避免之後每個公開項目各自標註。

#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::wallpaper_config::ConfigWatch;

/// 通道 `tw-events` 的固定名稱與來源檔名（design.md D5；資料層 `update_tw_events.py` 的
/// 產出檔名，凍結不可改）。
pub const TW_EVENTS_CHANNEL: &str = "tw-events";
const TW_EVENTS_FILE: &str = "tw_events.json";

/// 五個擴充通道的固定名稱（design.md D6／規格書「資料通道」）。
pub const CUSTOM_CHANNELS: [&str; 5] = ["custom1", "custom2", "custom3", "custom4", "custom5"];

/// 動態桌布的資料通道（dynamic-wallpaper design.md D7）。不是一個檔案來源，而是由 `tw-events`
/// 快照擷取 [`WALLPAPER_DATA_KEYS`]、再附上主題設定（[`ConfigWatch`]）衍生出來的通道，見
/// [`ChannelRegistry::watch_wallpaper_config`]。只有桌布渲染視窗
/// （`crate::wallpaper_render::is_renderer_label`）能查詢與訂閱，閘門在 `crate::widgets`。
pub const WALLPAPER_CHANNEL: &str = "wallpaper";

/// `wallpaper` 通道從 `tw-events` 擷取的鍵（存在才帶，值原樣帶過去，包括 `null`）：盤中走勢、
/// 日 K、融資、休市日、法說會（撕日曆的台積電法說會）、資料層的桌布專屬錯誤。
pub const WALLPAPER_DATA_KEYS: [&str; 6] = [
    "twii_intraday",
    "twii_daily",
    "margin",
    "holidays",
    "events",
    "wallpaper_errors",
];

/// 從 `tw-events` 的資料擷取 [`WALLPAPER_DATA_KEYS`] 中存在的鍵，回傳 JSON 物件（缺鍵不帶、多餘的
/// 鍵不帶）。輸入不是物件時回傳空物件。
pub fn extract_wallpaper_data(tw_events: &Value) -> Value {
    let mut out = serde_json::Map::new();
    if let Some(obj) = tw_events.as_object() {
        for key in WALLPAPER_DATA_KEYS {
            if let Some(v) = obj.get(key) {
                out.insert(key.to_owned(), v.clone());
            }
        }
    }
    Value::Object(out)
}

/// `wallpaper` 通道的 payload：`{ data, config }`（頁面契約見 `host/ui/wallpapers/lib/core.mjs`
/// 第 7 點）。尚無 `tw-events` 快照時 `data` 為空物件——頁面仍判成信封（`data` 是物件），主題依
/// 缺鍵畫缺資料畫面，而使用者的主題設定照常生效（星盤的時段表、休市表不依賴 tw-events）。
pub fn wallpaper_payload(tw_events: Option<&Value>, config: &Value) -> Value {
    let data = tw_events.map_or_else(
        || Value::Object(serde_json::Map::new()),
        extract_wallpaper_data,
    );
    serde_json::json!({ "data": data, "config": config })
}

/// 由目前的 `tw-events` 快照與主題設定組出 `wallpaper` 快照；`meta.fetched`／`updated` 沿用
/// `tw-events` 的（頁面判斷資料鮮度用），`loaded_at` 是這次組出的時間。
fn wallpaper_snapshot(tw_events: Option<&Snapshot>, config: &Value) -> Snapshot {
    Snapshot {
        channel: WALLPAPER_CHANNEL.to_owned(),
        data: wallpaper_payload(tw_events.map(|s| &s.data), config),
        meta: SnapshotMeta {
            loaded_at: now_epoch_ms(),
            fetched: tw_events.and_then(|s| s.meta.fetched.clone()),
            updated: tw_events.and_then(|s| s.meta.updated.clone()),
        },
    }
}

/// [`ChannelRegistry`] 內 `wallpaper` 衍生通道的狀態。
struct WallpaperFeed {
    config: ConfigWatch,
    /// 下一輪輪詢必須重算（剛開始監看、或註冊表剛整份重建）。
    dirty: bool,
}

/// 單一通道的快照（design.md D4：`get_snapshot` 回應的 `data`／`meta` 部分）。
///
/// 刻意不含 `status` 欄位：`status: "ok"|"empty"` 是 `get_snapshot`（task 2.7）的 IPC
/// 回應形狀，由「該通道是否曾經成功取得快照」（即 [`ChannelRegistry::snapshot`] 回傳
/// `Option` 的 `Some`/`None`）在那一層推導即可——`Snapshot` 只在驗證通過時才會被建構，
/// 本身恆為「有效」，重複放一個恆為 `"ok"` 的欄位沒有資訊量。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub channel: String,
    pub data: Value,
    pub meta: SnapshotMeta,
}

/// 快照的中介資訊（design.md D4：`get_snapshot`／`data` 事件回應的 `meta`，欄位一律
/// camelCase——`loaded_at` 序列化為 `loadedAt`，`fetched`／`updated` 本身無底線、camelCase
/// 與 snake_case 剛好同形。task 2.7 加上 `rename_all`，欄位本身（`loaded_at`）在 task 2.6
/// 就已就位，這裡只是把 IPC 對外形狀釘死成 design.md D4 規定的樣子。）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotMeta {
    /// 本快照被載入（驗證通過）的時間。
    ///
    /// 選 epoch 毫秒（`u64`）而非 ISO 8601 字串：host 只用 `std`（未加 `chrono`／`time`
    /// crate，依 task 2.6 brief 新增 crate 前要先回報，故不引入），`SystemTime` 沒有內建
    /// 的行事曆／時區換算能力，手刻 ISO 8601 格式化反而更容易出錯；epoch 毫秒是單一
    /// 數字、無時區歧義，前端（task 4.x）用 `new Date(loadedAt)` 一行就能換算成本機
    /// 時區顯示，與 AGENTS.md「總經事件由 ts＋系統時區動態換算」的既有慣例一致。
    pub loaded_at: u64,
    /// 資料本身的 `fetched` 欄位（資料層原子寫入時間）；只有 `tw-events` 這類本身帶
    /// 該欄位的來源才有值，`customN` 的任意 JSON 通常沒有、故為 `None`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fetched: Option<String>,
    /// 資料本身的 `updated` 欄位，語意同上。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated: Option<String>,
}

fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn extract_top_level_string(data: &Value, key: &str) -> Option<String> {
    data.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// 資料來源抽象（design.md D5）。
pub trait DataSource: Send {
    /// 檢查來源是否有「新的、驗證通過」的資料；沒有就回傳 `None`（見模組文件）。
    fn poll(&mut self) -> Option<Snapshot>;
}

/// 驗證函式：輸入解析後的 JSON，回傳「是否可以拿來當作新快照」（`true`＝通過）。
pub type Validator = Box<dyn Fn(&Value) -> bool + Send>;

/// 移植自 `finance-calendar.html`（Lively 版、已凍結、唯讀）第 798–803 行
/// `isEmptyPayload`，逐條對照：
///
/// ```js
/// function isEmptyPayload(d){   /* 合法但全空的 payload（如斷網時資料層仍寫出 updated 較新
///                                   但 macro/events/punish/quotes 皆空的檔案）；holidays 不計入 */
///   if (!d) return false;
///   const len = k => (d[k] && d[k].length) || 0;
///   return len('macro') + len('events') + len('punish') + len('quotes') === 0;
/// }
/// ```
///
/// 原函式回傳「是否全空」；這裡回傳「是否通過驗證」（語意相反，故取反），且本函式的
/// 輸入在呼叫前已保證是成功解析的 JSON（不會是缺值／`null`），故不需要原文 `!d` 那條
/// 早退。`holidays` 依原文不計入總和。
fn validate_tw_events(data: &Value) -> bool {
    let len = |key: &str| -> usize { data.get(key).and_then(Value::as_array).map_or(0, Vec::len) };
    let total = len("macro") + len("events") + len("punish") + len("quotes");
    total > 0
}

/// `customN` 通道只要求為合法 JSON（design.md D5）；`poll()` 已經先做過
/// `serde_json::from_str` 才會呼叫到這裡，故此處永遠通過。
fn validate_any_json(_data: &Value) -> bool {
    true
}

/// 以「修改時間＋檔案大小」判斷是否需要重讀的 JSON 檔案資料來源（design.md D5）。
pub struct JsonFileSource {
    channel: String,
    path: PathBuf,
    validator: Validator,
    /// 上次「成功讀到內容」（不論解析／驗證是否成功；讀檔 I/O 失敗不算）的 (修改時間,
    /// 檔案大小)；`None`＝來源檔不存在或從未成功讀過。
    last_seen: Option<(SystemTime, u64)>,
    /// 前置 I/O（metadata、修改時間、開檔讀取）的健康狀態，只用來決定「要不要寫記錄」：
    /// 狀態改變才記錄，持續同一種故障不重複（排程每 30 秒一輪）。見 [`IoHealth`]。
    health: IoHealth,
    /// 來源檔是否曾經存在過（`fs::metadata` 成功過）。fix F3（review 5.8 low）：只用來決定
    /// 轉成 [`IoHealth::Vanished`] 時的記錄文字——從未存在過的檔案不能記成「已不存在」。
    ever_present: bool,
    /// 實際開檔讀取的次數，只供測試斷言「未變不重讀」，正式流程不消費此值。
    reads: usize,
}

/// 前置 I/O 發生在哪一步（記錄用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IoStage {
    /// `fs::metadata`（檔案不存在以外的錯誤，例如權限、路徑非法、網路磁碟離線）。
    Metadata,
    /// `Metadata::modified`（極少數檔案系統不提供修改時間）。
    Modified,
    /// `fs::read`（開檔／讀取，例如防毒掃描或資料層寫入中的共用衝突）。
    Read,
}

impl IoStage {
    fn describe(self) -> &'static str {
        match self {
            IoStage::Metadata => "取得檔案資訊",
            IoStage::Modified => "取得修改時間",
            IoStage::Read => "讀檔",
        }
    }
}

/// [`JsonFileSource`] 前置 I/O 的健康狀態（fix round 1，Codex 5.8：前置 I/O 失敗原本無聲
/// 返回，記錄檔看不出畫面為何停在舊快照）。
///
/// 記錄規則（[`JsonFileSource::set_health`]）：
/// - `Idle → Idle`（首次就不存在＝尚無資料，例如從未使用的 `customN` 通道）：不記錄——這是
///   正常狀態，每 30 秒記一次只會洗版。
/// - 進入 `Failed`／`Vanished`，或從一種故障換成另一種（階段或錯誤碼不同）：warn 一次。
/// - 同一種故障持續：不記錄。
/// - 從 `Failed`／`Vanished` 回到 `Ok`：info 一次（恢復）。
#[derive(Debug, Clone, PartialEq, Eq)]
enum IoHealth {
    /// 尚未輪詢，或從一開始來源檔就不存在。
    Idle,
    /// 最近一輪前置 I/O 正常（檔案未變而略過、或讀檔成功——解析／驗證失敗另有記錄，不在此列）。
    Ok,
    /// 曾經存在（或曾經發生過其他故障）的來源檔現在不存在。
    Vanished,
    /// 前置 I/O 失敗；以階段＋錯誤種類＋OS 錯誤碼判斷「是不是同一種故障」。
    Failed {
        stage: IoStage,
        kind: io::ErrorKind,
        os_code: Option<i32>,
    },
}

impl IoHealth {
    fn failed(stage: IoStage, err: &io::Error) -> Self {
        IoHealth::Failed {
            stage,
            kind: err.kind(),
            os_code: err.raw_os_error(),
        }
    }

    fn is_problem(&self) -> bool {
        matches!(self, IoHealth::Vanished | IoHealth::Failed { .. })
    }
}

impl JsonFileSource {
    pub fn new(channel: impl Into<String>, path: PathBuf, validator: Validator) -> Self {
        Self {
            channel: channel.into(),
            path,
            validator,
            last_seen: None,
            health: IoHealth::Idle,
            ever_present: false,
            reads: 0,
        }
    }

    /// 更新 [`IoHealth`]，依本型別文件的規則決定是否寫記錄；`err` 是進入 `Failed` 時的原始
    /// 錯誤（記錄內容要含原始錯誤訊息）。
    fn set_health(&mut self, next: IoHealth, err: Option<&io::Error>) {
        if self.health == next {
            return;
        }
        match &next {
            IoHealth::Failed { stage, .. } => {
                let detail = err.map_or_else(String::new, ToString::to_string);
                log::warn!(
                    "資料來源{}失敗（channel={}, path={}）：{detail}；保留舊快照，持續失敗不再重複記錄",
                    stage.describe(),
                    self.channel,
                    self.path.display()
                );
            }
            IoHealth::Vanished if self.ever_present => log::warn!(
                "資料來源檔已不存在（channel={}, path={}）；保留舊快照",
                self.channel,
                self.path.display()
            ),
            // fix F3（review 5.8 low）：`Idle → Failed（非 NotFound）→ NotFound`——檔案從來沒存在
            // 過，先前的錯誤已不再發生，現在只是找不到檔案；不可寫成「已不存在」。
            IoHealth::Vanished => log::warn!(
                "資料來源檔不存在（channel={}, path={}）；先前的存取錯誤已不再發生，此檔從未出現過",
                self.channel,
                self.path.display()
            ),
            IoHealth::Ok if self.health.is_problem() => log::info!(
                "資料來源恢復正常（channel={}, path={}）",
                self.channel,
                self.path.display()
            ),
            IoHealth::Ok | IoHealth::Idle => {}
        }
        self.health = next;
    }

    /// 供測試斷言「未變不重讀」。
    #[cfg(test)]
    fn read_count(&self) -> usize {
        self.reads
    }
}

impl DataSource for JsonFileSource {
    fn poll(&mut self) -> Option<Snapshot> {
        let metadata = match fs::metadata(&self.path) {
            Ok(m) => m,
            Err(err) => {
                // 取不到檔案資訊（不存在、或其他 I/O 錯誤）：清掉記憶，下次一旦恢復務必當成
                // 「新檔」重讀，不會被舊的 (mtime, size) 誤判成未變。
                self.last_seen = None;
                // fix round 1（Codex 5.8）：原本這裡無聲返回。首次就不存在＝尚無資料（正常，
                // 維持 Idle 不記錄）；曾經存在後消失、或非「不存在」的錯誤 → 依 IoHealth 規則
                // 記錄一次。
                let next = if err.kind() == io::ErrorKind::NotFound {
                    if self.health == IoHealth::Idle {
                        IoHealth::Idle
                    } else {
                        IoHealth::Vanished
                    }
                } else {
                    IoHealth::failed(IoStage::Metadata, &err)
                };
                self.set_health(next, Some(&err));
                return None;
            }
        };
        self.ever_present = true;
        // mtime 若讀不到（極少數檔案系統／權限問題）視同無法判斷是否變更，保守跳過
        // 這次、下次再試，不冒進當成新檔重讀。fix round 1（Codex 5.8）：原本 `.ok()?` 無聲
        // 吞掉，改為依 IoHealth 規則記錄。
        let mtime = match metadata.modified() {
            Ok(mtime) => mtime,
            Err(err) => {
                self.set_health(IoHealth::failed(IoStage::Modified, &err), Some(&err));
                return None;
            }
        };
        let size = metadata.len();
        let current = (mtime, size);

        if self.last_seen == Some(current) {
            // 修改時間與大小都未變：略過，不重讀。前置 I/O 本身是正常的（例如 mtime 暫時讀不到
            // 後恢復、檔案沒變），故也算恢復。
            self.set_health(IoHealth::Ok, None);
            return None;
        }
        // 讀檔（I/O）失敗——暫時鎖定、權限——不記錄版本，直接回傳 None：下一輪即使
        // (mtime, size) 未變也會重試（fix round 1，Codex 2.6：原本先記版本再讀，暫時讀取
        // 失敗後檔案不變就永不重試）。task 5.8：記一筆 warn；fix round 1（Codex 5.8）起改走
        // IoHealth 去重——持續鎖定時每 30 秒重試一次，只在第一次記錄，恢復時記一筆 info。
        let raw = match fs::read(&self.path) {
            Ok(raw) => raw,
            Err(err) => {
                self.set_health(IoHealth::failed(IoStage::Read, &err), Some(&err));
                return None;
            }
        };
        self.set_health(IoHealth::Ok, None);
        self.reads += 1;
        // 內容已成功讀到：不論接下來解析／驗證是否成功，這個 (mtime, size) 版本都算「已經
        // 處理過」，避免對同一份損壞／全空內容每次輪詢都重讀一次。記錄的是讀檔**前**取得的
        // 版本——讀檔途中若來源又被改寫，下一輪看到的 mtime 不同，會再讀一次。
        self.last_seen = Some(current);

        // 解析失敗（含非 UTF-8）→ None，保留舊快照。task 5.8：記一筆 warn（資料層寫出的檔案
        // 正常情況下一定是合法 JSON，出現在記錄檔代表寫入中途被讀到或上游壞掉，值得追查）。
        let data: Value = match serde_json::from_slice(&raw) {
            Ok(data) => data,
            Err(err) => {
                log::warn!(
                    "資料來源解析失敗（channel={}, path={}）：{err}",
                    self.channel,
                    self.path.display()
                );
                return None;
            }
        };

        if !(self.validator)(&data) {
            // debug 而非 warn：這是預期會發生的正常狀態（例如假日 tw-events 全空），不是
            // 「載入錯誤」；預設等級門檻（`logging::init` 設 Info）下不會寫進記錄檔，只在需要
            // 加大詳細度時看得到。
            log::debug!(
                "資料來源驗證未通過，保留舊快照（channel={}, path={}）",
                self.channel,
                self.path.display()
            );
            return None; // 驗證未通過（如 tw-events 全空）→ None，保留舊快照
        }

        let fetched = extract_top_level_string(&data, "fetched");
        let updated = extract_top_level_string(&data, "updated");
        Some(Snapshot {
            channel: self.channel.clone(),
            data,
            meta: SnapshotMeta {
                loaded_at: now_epoch_ms(),
                fetched,
                updated,
            },
        })
    }
}

/// 通道名 → [`DataSource`] 實例的註冊表（design.md D5）；`snapshot()` 回傳目前各通道
/// 最新一份「驗證通過」的快照，`None`＝該通道從未取得有效快照（規格書「尚無資料」）。
///
/// task 4.1 fix round 3（Codex task-4.1-fix-codex-r1.md [high]）：註冊表帶一個**世代**
/// （[`Self::generation`]），整份重建（[`Self::replace_with`]，`update_settings` 改
/// `data_dir` 時）就遞增。重建後各通道回到「尚無資料」（`get_snapshot` 回 empty），這個
/// empty 比重建前的任何快照都新；頁面若只比 `meta.loadedAt`，empty（meta=null）無從比較，
/// 會讓重建前緩衝到的舊目錄資料復活。世代放進快照／empty 回覆與 `data` 推送（design.md D4），
/// 頁面以 (generation, loadedAt) 比較新舊。
pub struct ChannelRegistry {
    sources: Vec<(String, Box<dyn DataSource>)>,
    snapshots: HashMap<String, Snapshot>,
    generation: u64,
    /// 要監看存在與否的資料目錄（[`Self::watch_data_dir`]，[`build_default_registry`] 設定）。
    data_dir: Option<PathBuf>,
    /// `data_dir` 的健康狀態，只用來決定要不要寫記錄（見 [`DirHealth`]）。
    dir_health: DirHealth,
    /// `wallpaper` 衍生通道（[`Self::watch_wallpaper_config`]）；`None`＝沒有這個通道。
    wallpaper: Option<WallpaperFeed>,
}

/// 資料目錄本身的健康狀態（fix F3，review task-5.8-fixA-opus.md [low]）。目錄不存在或不可達
/// （例如 `data_dir` 打錯、網路磁碟離線）時，各來源檔都只是「首次就不存在」而維持
/// [`IoHealth::Idle`] 不記錄，記錄檔完全看不出原因；改在註冊表層級每輪檢查一次目錄：
/// - 進入 `Unavailable`（或換成另一種錯誤）：warn 一次，含路徑與原始錯誤。
/// - 同一種狀態持續：不記錄（一個註冊表只對應一個目錄，同一路徑只記一次）。
/// - 從 `Unavailable` 回到 `Present`：info 一次（恢復）。`Unknown → Present` 不記錄。
#[derive(Debug, Clone, PartialEq, Eq)]
enum DirHealth {
    /// 尚未檢查。
    Unknown,
    /// 目錄存在。
    Present,
    /// 目錄不存在、不可達，或該路徑不是目錄（`kind`／`os_code` 為 `None`）。
    Unavailable {
        kind: Option<io::ErrorKind>,
        os_code: Option<i32>,
    },
}

impl ChannelRegistry {
    pub fn new() -> Self {
        Self {
            sources: Vec::new(),
            snapshots: HashMap::new(),
            generation: 0,
            data_dir: None,
            dir_health: DirHealth::Unknown,
            wallpaper: None,
        }
    }

    /// 開啟 `wallpaper` 衍生通道（dynamic-wallpaper task 4.6，design.md D7）：主題設定檔在
    /// `config_path`（宿主設定資料夾的 `wallpaper-config.json`），由 [`ConfigWatch`] 監看。
    ///
    /// 每輪 [`Self::poll_all_changed`] 在檔案來源輪詢完之後檢查：`tw-events` 這輪有新快照、設定檔
    /// 內容改變、或剛開始監看／剛重建（`dirty`）時，重算 `wallpaper` 快照並把它列入這輪的變更清單
    /// （排程據此推給訂閱它的渲染視窗）。`tw-events` 通道本身的快照完全不動。
    pub fn watch_wallpaper_config(&mut self, config_path: &Path) {
        self.wallpaper = Some(WallpaperFeed {
            config: ConfigWatch::new(config_path.to_path_buf()),
            dirty: true,
        });
    }

    /// 同 [`Self::watch_wallpaper_config`]，但以啟動時已載入的結果開始監看（task 4.7a，
    /// [`ConfigWatch::primed`]）：第一輪不重讀、不重記警告；`wallpaper` 快照照樣在第一輪組出。
    pub fn watch_wallpaper_config_primed(
        &mut self,
        config_path: &Path,
        startup: &crate::wallpaper_config::WallpaperConfig,
    ) {
        self.wallpaper = Some(WallpaperFeed {
            config: ConfigWatch::primed(config_path.to_path_buf(), startup),
            dirty: true,
        });
    }

    /// 主題設定檔內容改變過幾次（[`ConfigWatch::changes`]；沒有 `wallpaper` 通道時 `None`）。桌布
    /// 協調迴圈（task 4.7a）以它判斷「主題設定檔變了、目前的主題要重畫」。
    pub fn wallpaper_config_version(&self) -> Option<u64> {
        self.wallpaper.as_ref().map(|feed| feed.config.changes())
    }

    /// 主題設定檔目前的內容（Rust 頂層合併後；[`ConfigWatch::value`]；沒有 `wallpaper` 通道時
    /// `None`）。dynamic-wallpaper task 4.8：設定視窗據此判定休市表快過期（頁面再以內建預設逐層合併）。
    pub fn wallpaper_config_value(&self) -> Option<&Value> {
        self.wallpaper.as_ref().map(|feed| feed.config.value())
    }

    /// 主題設定檔新版本第一次讀到問題、等延後重讀中（[`ConfigWatch::retry_pending`]；task 4.6
    /// 修正輪 3）。排程層（`crate::widgets::poll_and_notify_settled`）放掉鎖之後據此等一下再輪詢一次。
    pub fn wallpaper_retry_pending(&self) -> bool {
        self.wallpaper
            .as_ref()
            .is_some_and(|feed| feed.config.retry_pending())
    }

    /// 見 [`Self::watch_wallpaper_config`]；這輪有重算時回傳 `true`。
    fn refresh_wallpaper(&mut self, tw_events_changed: bool) -> bool {
        let Some(feed) = self.wallpaper.as_mut() else {
            return false;
        };
        // 設定檔每輪都要看（不可被 `||` 短路掉），變更旗標才不會延後到下一輪。
        let config_changed = feed.config.poll();
        if !(feed.dirty || config_changed || tw_events_changed) {
            return false;
        }
        feed.dirty = false;
        let snapshot =
            wallpaper_snapshot(self.snapshots.get(TW_EVENTS_CHANNEL), feed.config.value());
        self.snapshots
            .insert(WALLPAPER_CHANNEL.to_owned(), snapshot);
        true
    }

    /// 每輪輪詢前檢查 `dir` 存不存在（見 [`DirHealth`]）。
    pub fn watch_data_dir(&mut self, dir: &Path) {
        self.data_dir = Some(dir.to_path_buf());
        self.dir_health = DirHealth::Unknown;
    }

    /// 檢查 [`Self::watch_data_dir`] 設定的目錄，狀態改變才寫記錄（見 [`DirHealth`]）。
    fn check_data_dir(&mut self) {
        let Some(dir) = &self.data_dir else {
            return;
        };
        let (next, detail) = match fs::metadata(dir) {
            Ok(m) if m.is_dir() => (DirHealth::Present, String::new()),
            Ok(_) => (
                DirHealth::Unavailable {
                    kind: None,
                    os_code: None,
                },
                "該路徑不是資料夾".to_string(),
            ),
            Err(err) => (
                DirHealth::Unavailable {
                    kind: Some(err.kind()),
                    os_code: err.raw_os_error(),
                },
                err.to_string(),
            ),
        };
        if next == self.dir_health {
            return;
        }
        match &next {
            DirHealth::Unavailable { .. } => log::warn!(
                "資料目錄不存在或無法存取（path={}）：{detail}；各小工具顯示尚無資料或保留舊快照，持續如此不再重複記錄",
                dir.display()
            ),
            DirHealth::Present if self.dir_health != DirHealth::Unknown => {
                log::info!("資料目錄已可存取（path={}）", dir.display());
            }
            DirHealth::Present | DirHealth::Unknown => {}
        }
        self.dir_health = next;
    }

    /// 目前世代（見型別文件）。新建的註冊表為 0，每次 [`Self::replace_with`] 加 1。
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// 以 `fresh`（通常是依新資料目錄 [`build_default_registry`] 建出的註冊表）整份取代自己，
    /// 世代＝舊世代＋1（`fresh` 自身的世代值忽略）。取代後舊的來源與快照全部丟棄。
    /// `saturating_add`：u64 實務上不可能溢位（每秒重建一次也要五千億年），仍避免 debug
    /// build 的溢位 panic 成為理論上的故障點。
    ///
    /// task 4.6：`wallpaper` 衍生通道（[`Self::watch_wallpaper_config`]）的設定監看與資料目錄無關，
    /// `fresh` 沒有自己的監看時沿用舊的；快照與其他通道一樣清掉（回到尚無資料），並標成下一輪必須
    /// 重算——否則新目錄一直沒有 `tw_events.json`、設定檔也沒變時，`wallpaper` 會永遠停在尚無資料。
    pub fn replace_with(&mut self, fresh: ChannelRegistry) {
        let next = self.generation.saturating_add(1);
        let wallpaper = self.wallpaper.take();
        *self = fresh;
        self.generation = next;
        if self.wallpaper.is_none() {
            self.wallpaper = wallpaper;
        }
        if let Some(feed) = self.wallpaper.as_mut() {
            feed.dirty = true;
        }
    }

    /// 註冊一個通道。同名重複註冊會覆蓋先前的來源（保留目前已有的快照，來源換了不代表
    /// 資料失效）。
    pub fn register(&mut self, channel: impl Into<String>, source: Box<dyn DataSource>) {
        let channel = channel.into();
        self.sources.retain(|(name, _)| name != &channel);
        self.sources.push((channel, source));
    }

    /// 逐一輪詢所有通道；只有回傳 `Some` 的通道才覆蓋快照，`None` 的通道維持原狀
    /// （新檔未出現、未變、解析失敗、驗證未過皆屬此類）。不需要知道「哪些通道變了」的呼叫端
    /// 用這個；task 2.7 的排程需要知道才能只對變更的通道推播事件，見 [`Self::poll_all_changed`]。
    pub fn poll_all(&mut self) {
        self.poll_all_changed();
    }

    /// 邏輯與 [`Self::poll_all`] 完全相同，額外回傳「這一輪真的取得新快照」的通道名清單
    /// （design.md D5「有變才通知訂閱小工具，未變不通知」；task 2.7 排程只對這份清單裡的通道
    /// 推播 `data` 事件，其餘通道這一輪不動作）。清單為空即代表這一輪沒有任何通道變更。
    pub fn poll_all_changed(&mut self) -> Vec<String> {
        self.check_data_dir();
        let mut changed = Vec::new();
        for (channel, source) in &mut self.sources {
            if let Some(snapshot) = source.poll() {
                self.snapshots.insert(channel.clone(), snapshot);
                changed.push(channel.clone());
            }
        }
        let tw_events_changed = changed.iter().any(|c| c == TW_EVENTS_CHANNEL);
        if self.refresh_wallpaper(tw_events_changed) {
            changed.push(WALLPAPER_CHANNEL.to_owned());
        }
        changed
    }

    /// 目前的快照；`None`＝該通道從未取得有效快照。
    pub fn snapshot(&self, channel: &str) -> Option<&Snapshot> {
        self.snapshots.get(channel)
    }

    /// 已註冊的通道名清單，依註冊順序。
    pub fn channels(&self) -> Vec<&str> {
        self.sources.iter().map(|(name, _)| name.as_str()).collect()
    }
}

impl Default for ChannelRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// 建立本 change 提供的六個通道：`tw-events`＋`custom1`–`custom5`（design.md D5）。
/// `data_dir` 即設定中的資料目錄（task 2.3 `Settings::data_dir`）；新增資料源時比照在此
/// 加一行 `register`。
pub fn build_default_registry(data_dir: &Path) -> ChannelRegistry {
    let mut registry = ChannelRegistry::new();
    registry.watch_data_dir(data_dir);
    registry.register(
        TW_EVENTS_CHANNEL,
        Box::new(JsonFileSource::new(
            TW_EVENTS_CHANNEL,
            data_dir.join(TW_EVENTS_FILE),
            Box::new(validate_tw_events),
        )),
    );
    for channel in CUSTOM_CHANNELS {
        registry.register(
            channel,
            Box::new(JsonFileSource::new(
                channel,
                data_dir.join(format!("{channel}.json")),
                Box::new(validate_any_json),
            )),
        );
    }
    registry
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::env;
    use std::thread;
    use std::time::Duration;

    /// 每個測試用行程 id＋呼叫端給的名字組出唯一的暫存路徑，避免依賴額外的 crate
    /// （如 `tempfile`）；做法與 settings.rs 的測試一致。
    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            env::temp_dir().join(format!("fc-host-data-test-{}-{}", std::process::id(), name));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("建立暫存目錄失敗");
        dir
    }

    fn cleanup(dir: &Path) {
        let _ = fs::remove_dir_all(dir);
    }

    /// mtime 解析度在某些檔案系統／CI 環境下可能只有約 10ms；寫檔之間睡一下確保
    /// 修改時間確實往前推進，避免測試因時間精度而誤判「未變」。
    fn settle() {
        thread::sleep(Duration::from_millis(20));
    }

    fn write(dir: &Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, content).expect("寫入測試檔失敗");
        settle();
        path
    }

    // ── JsonFileSource 直接測試（未變不重讀）───────────────────────────────────

    #[test]
    fn json_file_source_reads_new_file_then_skips_unchanged() {
        let dir = temp_dir("unchanged");
        let path = write(&dir, "custom1.json", r#"{"a":1}"#);

        let mut source = JsonFileSource::new("custom1", path.clone(), Box::new(validate_any_json));

        // 情況一：新檔——第一次 poll 應該讀到內容。
        let first = source.poll();
        assert!(first.is_some(), "新檔應該被讀到");
        assert_eq!(source.read_count(), 1);
        assert_eq!(first.unwrap().data, serde_json::json!({"a": 1}));

        // 情況二：未變——同一份檔案沒有任何改動，poll 不應該重讀。
        let second = source.poll();
        assert!(second.is_none(), "未變時 poll 應回傳 None");
        assert_eq!(source.read_count(), 1, "未變時不應該重讀檔案");

        cleanup(&dir);
    }

    #[test]
    fn json_file_source_rereads_after_content_changes() {
        let dir = temp_dir("changed");
        let path = write(&dir, "custom1.json", r#"{"a":1}"#);
        let mut source = JsonFileSource::new("custom1", path.clone(), Box::new(validate_any_json));
        assert!(source.poll().is_some());
        assert_eq!(source.read_count(), 1);

        write(&dir, "custom1.json", r#"{"a":2,"b":true}"#);
        let updated = source.poll();
        assert!(updated.is_some(), "內容變更後應該重讀");
        assert_eq!(source.read_count(), 2);
        assert_eq!(
            updated.unwrap().data,
            serde_json::json!({"a": 2, "b": true})
        );

        cleanup(&dir);
    }

    // ── 情況三：損壞 ────────────────────────────────────────────────────────

    #[test]
    fn json_file_source_returns_none_on_corrupt_content() {
        let dir = temp_dir("corrupt");
        let path = write(&dir, "custom1.json", r#"{"a":1}"#);
        let mut source = JsonFileSource::new("custom1", path.clone(), Box::new(validate_any_json));
        assert!(source.poll().is_some());

        write(&dir, "custom1.json", "{ 這不是合法的 JSON ,,,");
        let result = source.poll();
        assert!(result.is_none(), "無法解析的內容應回傳 None");

        cleanup(&dir);
    }

    // ── 情況四：全空（tw-events 專屬 validator）─────────────────────────────

    #[test]
    fn tw_events_validator_rejects_all_empty_payload() {
        // 逐條對照 isEmptyPayload：macro/events/punish/quotes 皆空（不含 holidays）
        // → 視為全空，驗證不通過。
        let empty = serde_json::json!({
            "updated": "2026-09-28 12:00",
            "macro": [],
            "events": [],
            "punish": [],
            "quotes": [],
            "holidays": ["2026-10-10"] // holidays 不計入，即使非空也不影響全空判定
        });
        assert!(!validate_tw_events(&empty));

        let non_empty = serde_json::json!({
            "updated": "2026-09-28 12:00",
            "macro": [],
            "events": [],
            "punish": [],
            "quotes": [{"name": "USD/TWD"}]
        });
        assert!(validate_tw_events(&non_empty));
    }

    #[test]
    fn json_file_source_rejects_empty_tw_events_and_keeps_old_snapshot_available() {
        let dir = temp_dir("empty-tw-events");
        let good = r#"{"updated":"2026-09-28 08:00","fetched":"2026-09-28 08:00",
            "macro":[],"events":[],"punish":[],"quotes":[{"name":"USD/TWD"}]}"#;
        let path = write(&dir, TW_EVENTS_FILE, good);
        let mut source = JsonFileSource::new(
            TW_EVENTS_CHANNEL,
            path.clone(),
            Box::new(validate_tw_events),
        );
        let first = source.poll().expect("非空內容應該通過驗證");
        assert_eq!(first.meta.updated.as_deref(), Some("2026-09-28 08:00"));

        // 改寫成合法但全空的內容（updated 較新、macro/events/punish/quotes 皆空）。
        let empty = r#"{"updated":"2026-09-28 09:00","fetched":"2026-09-28 09:00",
            "macro":[],"events":[],"punish":[],"quotes":[]}"#;
        write(&dir, TW_EVENTS_FILE, empty);
        let rejected = source.poll();
        assert!(rejected.is_none(), "全空內容應被拒收（回傳 None）");

        cleanup(&dir);
    }

    // ── ChannelRegistry：端對端行為（保留舊快照／尚無資料）───────────────────

    #[test]
    fn registry_keeps_last_good_snapshot_when_new_content_is_corrupt() {
        let dir = temp_dir("registry-corrupt");
        write(&dir, "custom1.json", r#"{"v":1}"#);
        let mut registry = ChannelRegistry::new();
        registry.register(
            "custom1",
            Box::new(JsonFileSource::new(
                "custom1",
                dir.join("custom1.json"),
                Box::new(validate_any_json),
            )),
        );

        registry.poll_all();
        let good = registry
            .snapshot("custom1")
            .expect("第一次輪詢後應有快照")
            .clone();
        assert_eq!(good.data, serde_json::json!({"v": 1}));

        write(&dir, "custom1.json", "{ 壞掉的 json ,,,");
        registry.poll_all();
        let after = registry.snapshot("custom1").expect("損壞後仍應保留舊快照");
        assert_eq!(after, &good, "損壞後快照內容應與損壞前完全相同");

        cleanup(&dir);
    }

    #[test]
    fn registry_keeps_last_good_snapshot_when_tw_events_becomes_empty() {
        let dir = temp_dir("registry-empty");
        let good = r#"{"updated":"2026-09-28 08:00","fetched":"2026-09-28 08:00",
            "macro":[{"title":"CPI"}],"events":[],"punish":[],"quotes":[]}"#;
        write(&dir, TW_EVENTS_FILE, good);
        let mut registry = ChannelRegistry::new();
        registry.register(
            TW_EVENTS_CHANNEL,
            Box::new(JsonFileSource::new(
                TW_EVENTS_CHANNEL,
                dir.join(TW_EVENTS_FILE),
                Box::new(validate_tw_events),
            )),
        );

        registry.poll_all();
        let before = registry
            .snapshot(TW_EVENTS_CHANNEL)
            .expect("第一次輪詢後應有非空快照")
            .clone();

        let empty = r#"{"updated":"2026-09-28 09:00","fetched":"2026-09-28 09:00",
            "macro":[],"events":[],"punish":[],"quotes":[]}"#;
        write(&dir, TW_EVENTS_FILE, empty);
        registry.poll_all();
        let after = registry
            .snapshot(TW_EVENTS_CHANNEL)
            .expect("變成全空後仍應保留舊快照");
        assert_eq!(after, &before, "全空之後應繼續顯示上一份非空資料");

        cleanup(&dir);
    }

    #[test]
    fn registry_reports_no_snapshot_when_source_file_never_existed() {
        let dir = temp_dir("no-data-yet");
        // 刻意不建立 tw_events.json：資料目錄存在但檔案不存在。
        let mut registry = ChannelRegistry::new();
        registry.register(
            TW_EVENTS_CHANNEL,
            Box::new(JsonFileSource::new(
                TW_EVENTS_CHANNEL,
                dir.join(TW_EVENTS_FILE),
                Box::new(validate_tw_events),
            )),
        );

        registry.poll_all();
        assert!(
            registry.snapshot(TW_EVENTS_CHANNEL).is_none(),
            "從未有來源檔時應回報「尚無資料」（None）"
        );

        cleanup(&dir);
    }

    #[test]
    fn registry_picks_up_file_that_appears_after_being_absent() {
        let dir = temp_dir("appears-later");
        let mut registry = ChannelRegistry::new();
        registry.register(
            "custom1",
            Box::new(JsonFileSource::new(
                "custom1",
                dir.join("custom1.json"),
                Box::new(validate_any_json),
            )),
        );

        registry.poll_all();
        assert!(registry.snapshot("custom1").is_none());

        write(&dir, "custom1.json", r#"{"v":1}"#);
        registry.poll_all();
        assert!(
            registry.snapshot("custom1").is_some(),
            "檔案出現後應該被讀到"
        );

        cleanup(&dir);
    }

    // ── build_default_registry：通道組成 ───────────────────────────────────

    // ── poll_all_changed：只回報這一輪真的更新的通道（task 2.7 事件推播依據）───────

    #[test]
    fn poll_all_changed_reports_only_channels_that_actually_updated() {
        let dir = temp_dir("poll-all-changed");
        write(&dir, "custom1.json", r#"{"a":1}"#);
        write(&dir, "custom2.json", r#"{"b":1}"#);
        let mut registry = ChannelRegistry::new();
        registry.register(
            "custom1",
            Box::new(JsonFileSource::new(
                "custom1",
                dir.join("custom1.json"),
                Box::new(validate_any_json),
            )),
        );
        registry.register(
            "custom2",
            Box::new(JsonFileSource::new(
                "custom2",
                dir.join("custom2.json"),
                Box::new(validate_any_json),
            )),
        );

        // 第一輪：兩個通道都是新檔，皆應回報。
        let mut first = registry.poll_all_changed();
        first.sort();
        assert_eq!(first, vec!["custom1", "custom2"]);

        // 第二輪：兩個檔案都沒變，應回報空清單。
        let second = registry.poll_all_changed();
        assert!(second.is_empty(), "未變時不應回報任何通道：{second:?}");

        // 第三輪：只改 custom1，只有 custom1 應被回報。
        write(&dir, "custom1.json", r#"{"a":2}"#);
        let third = registry.poll_all_changed();
        assert_eq!(third, vec!["custom1"]);

        cleanup(&dir);
    }

    #[test]
    fn build_default_registry_registers_tw_events_and_five_custom_channels() {
        let dir = temp_dir("default-registry");
        let registry = build_default_registry(&dir);
        let mut channels = registry.channels();
        channels.sort();
        assert_eq!(
            channels,
            vec![
                "custom1",
                "custom2",
                "custom3",
                "custom4",
                "custom5",
                "tw-events",
            ]
        );
        cleanup(&dir);
    }

    #[test]
    fn build_default_registry_reads_real_tw_events_sample_shape() {
        // 對照真實資料層輸出的欄位形狀（fetched/updated/macro/events/punish/quotes 皆
        // 存在、非空），確保 validator 與 meta 抽取邏輯與實際產出相容。
        let dir = temp_dir("real-shape");
        let sample = r#"{
            "updated": "2026-09-28 00:00",
            "fetched": "2026-09-28 00:00",
            "errors": [],
            "macro": [{"title": "CPI"}],
            "events": [{"name": "台積電法說會"}],
            "punish": [{"code": "1234"}],
            "quotes": [{"name": "USD/TWD", "price": 31.5}],
            "holidays": []
        }"#;
        write(&dir, TW_EVENTS_FILE, sample);

        let mut registry = build_default_registry(&dir);
        registry.poll_all();
        let snapshot = registry
            .snapshot(TW_EVENTS_CHANNEL)
            .expect("真實形狀的資料應該通過驗證");
        assert_eq!(snapshot.meta.fetched.as_deref(), Some("2026-09-28 00:00"));
        assert_eq!(snapshot.meta.updated.as_deref(), Some("2026-09-28 00:00"));
        assert!(snapshot.meta.loaded_at > 0);

        cleanup(&dir);
    }

    // ── task 4.1 fix round 3：註冊表世代 ─────────────────────────────────────────

    #[test]
    fn replace_with_bumps_generation_and_drops_old_snapshots() {
        let old_dir = temp_dir("generation-old");
        let new_dir = temp_dir("generation-new");
        write(&old_dir, "custom1.json", r#"{"from": "old"}"#);

        let mut registry = build_default_registry(&old_dir);
        assert_eq!(registry.generation(), 0, "新建註冊表的世代為 0");
        registry.poll_all();
        assert!(registry.snapshot("custom1").is_some());

        // 改資料目錄：整份重建 → 世代 +1、舊目錄快照消失（新目錄尚無檔案＝empty）。
        registry.replace_with(build_default_registry(&new_dir));
        assert_eq!(registry.generation(), 1);
        assert!(
            registry.snapshot("custom1").is_none(),
            "重建後不得殘留舊目錄的快照"
        );

        // 再切一次，世代繼續遞增（`fresh` 自身的世代值被忽略）。
        registry.replace_with(build_default_registry(&old_dir));
        assert_eq!(registry.generation(), 2);
        registry.poll_all();
        assert!(registry.snapshot("custom1").is_some());

        cleanup(&old_dir);
        cleanup(&new_dir);
    }

    // ── fix round 1（Codex 2.6）：暫時讀取失敗後，檔案未變也要能重試 ─────────────────

    /// 以「不共用」模式（share_mode 0）開著檔案，模擬防毒掃描／資料層寫入中造成的暫時鎖定：
    /// `fs::metadata` 仍成功（std 在共用衝突時退回目錄列舉取屬性），但開檔讀內容會失敗。
    #[cfg(windows)]
    #[test]
    fn transient_read_failure_is_retried_without_file_change() {
        use std::os::windows::fs::OpenOptionsExt;

        let dir = temp_dir("transient-lock");
        let path = write(&dir, "custom2.json", r#"{"b":2}"#);
        let mut source = JsonFileSource::new("custom2", path.clone(), Box::new(validate_any_json));

        let lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .expect("以獨占模式開檔失敗");
        assert!(
            fs::metadata(&path).is_ok(),
            "前提：鎖定期間 metadata 仍可取得"
        );
        assert!(source.poll().is_none(), "鎖定期間讀不到內容 → None");
        drop(lock);

        // 檔案內容、mtime、大小完全沒變，只是解除鎖定：下一輪 poll 必須重試並成功載入。
        let recovered = source.poll();
        assert!(
            recovered.is_some(),
            "暫時讀取失敗後檔案未變也要重試（否則直到來源改寫前都不會載入）"
        );
        assert_eq!(recovered.unwrap().data, serde_json::json!({"b": 2}));

        // 成功之後才記錄版本：再下一輪未變 → 不重讀。
        let reads = source.read_count();
        assert!(source.poll().is_none());
        assert_eq!(source.read_count(), reads, "成功載入後未變不重讀");

        cleanup(&dir);
    }

    // ── fix round 1（Codex 5.8）：前置 I/O 失敗要留下記錄，且持續故障不洗版 ─────────────

    /// 測試用的全域 logger：只收集「目前執行緒」發出的記錄（cargo test 平行跑，各測試在
    /// 自己的執行緒；thread-local 讓測試之間互不干擾）。`log::set_logger` 全行程只能設一次，
    /// 以 `Once` 保護；本 crate 其他測試不設全域 logger（logging.rs 的測試直接呼叫
    /// `DailyRotatingLogger::log`），不會衝突。`wallpaper_state` 的測試也共用這一份（dynamic-wallpaper 6.1
    /// 修正輪 4：斷言讓位記錄），不另裝 logger。
    pub(crate) mod capture {
        use log::{Level, LevelFilter, Log, Metadata, Record};
        use std::cell::RefCell;
        use std::sync::Once;

        thread_local! {
            static RECORDS: RefCell<Vec<(Level, String)>> = const { RefCell::new(Vec::new()) };
        }

        struct CaptureLogger;

        impl Log for CaptureLogger {
            fn enabled(&self, _: &Metadata) -> bool {
                true
            }
            fn log(&self, record: &Record) {
                RECORDS.with(|r| {
                    r.borrow_mut()
                        .push((record.level(), record.args().to_string()))
                });
            }
            fn flush(&self) {}
        }

        static LOGGER: CaptureLogger = CaptureLogger;
        static INIT: Once = Once::new();

        /// 安裝（第一次）並清空本執行緒已收集的記錄。
        pub fn start() {
            INIT.call_once(|| {
                let _ = log::set_logger(&LOGGER);
                log::set_max_level(LevelFilter::Trace);
            });
            RECORDS.with(|r| r.borrow_mut().clear());
        }

        /// 取出並清空本執行緒自上次 `start`／`take` 後的 info 以上記錄。
        pub fn take() -> Vec<(Level, String)> {
            RECORDS.with(|r| {
                r.borrow_mut()
                    .drain(..)
                    .filter(|(level, _)| *level <= Level::Info)
                    .collect()
            })
        }
    }

    /// `fs::metadata` 失敗但不是「檔案不存在」（Windows 上檔名含 `<`、`>` 等非法字元 →
    /// ERROR_INVALID_NAME）：必須寫一筆 warn，含 channel、path 與原始錯誤；持續失敗的後續
    /// 輪詢不重複記錄（每 30 秒一輪，不去重會洗版）。
    #[cfg(windows)]
    #[test]
    fn metadata_failure_is_logged_once_with_channel_and_path() {
        capture::start();
        let dir = temp_dir("metadata-fail");
        let path = dir.join("bad<name>.json");
        let err = fs::metadata(&path).expect_err("前提：非法檔名的 metadata 必定失敗");
        assert_ne!(
            err.kind(),
            std::io::ErrorKind::NotFound,
            "前提：錯誤不是 NotFound"
        );
        let mut source = JsonFileSource::new("custom3", path.clone(), Box::new(validate_any_json));

        assert!(source.poll().is_none());
        let logs = capture::take();
        assert_eq!(logs.len(), 1, "前置 I/O 失敗應記錄一筆：{logs:?}");
        let (level, msg) = &logs[0];
        assert_eq!(*level, log::Level::Warn);
        assert!(msg.contains("custom3"), "記錄要含 channel：{msg}");
        assert!(msg.contains("bad<name>.json"), "記錄要含 path：{msg}");

        assert!(source.poll().is_none());
        assert!(source.poll().is_none());
        assert!(capture::take().is_empty(), "同一錯誤持續發生時不應重複記錄");
        cleanup(&dir);
    }

    /// 首次就不存在（尚無資料，例如從未使用的 custom 通道）是正常狀態，不記錄；曾經成功讀過的
    /// 來源檔消失則要記錄一次（畫面保留舊快照，記錄檔要說明原因），重新出現並成功載入時記錄
    /// 恢復。
    #[test]
    fn missing_file_is_silent_at_first_but_disappearance_is_logged_once() {
        capture::start();
        let dir = temp_dir("disappear");
        let path = dir.join("custom4.json");
        let mut source = JsonFileSource::new("custom4", path.clone(), Box::new(validate_any_json));

        assert!(source.poll().is_none());
        assert!(source.poll().is_none());
        assert!(capture::take().is_empty(), "首次不存在不記錄");

        fs::write(&path, r#"{"c":3}"#).expect("寫入測試檔失敗");
        settle();
        assert!(source.poll().is_some());
        assert!(capture::take().is_empty(), "正常載入不記錄");

        fs::remove_file(&path).expect("刪檔失敗");
        assert!(source.poll().is_none());
        assert!(source.poll().is_none());
        let logs = capture::take();
        assert_eq!(logs.len(), 1, "來源檔消失應記錄一次：{logs:?}");
        assert_eq!(logs[0].0, log::Level::Warn);
        assert!(logs[0].1.contains("custom4"), "{}", logs[0].1);

        fs::write(&path, r#"{"c":4}"#).expect("寫入測試檔失敗");
        settle();
        assert!(source.poll().is_some());
        let logs = capture::take();
        assert_eq!(logs.len(), 1, "恢復應記錄一次：{logs:?}");
        assert_eq!(logs[0].0, log::Level::Info);
        cleanup(&dir);
    }

    /// 讀檔（開檔）持續失敗：只在第一次記錄 warn；解除後成功載入記錄一次恢復（info）。
    #[cfg(windows)]
    #[test]
    fn persistent_read_failure_is_logged_once_then_recovery_is_logged() {
        use std::os::windows::fs::OpenOptionsExt;

        capture::start();
        let dir = temp_dir("persistent-lock");
        let path = write(&dir, "custom5.json", r#"{"d":5}"#);
        let mut source = JsonFileSource::new("custom5", path.clone(), Box::new(validate_any_json));

        let lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .expect("以獨占模式開檔失敗");
        assert!(source.poll().is_none());
        assert!(source.poll().is_none());
        assert!(source.poll().is_none());
        let logs = capture::take();
        assert_eq!(logs.len(), 1, "持續讀檔失敗只記錄一次：{logs:?}");
        assert_eq!(logs[0].0, log::Level::Warn);
        drop(lock);

        assert!(source.poll().is_some());
        let logs = capture::take();
        assert_eq!(logs.len(), 1, "恢復應記錄一次：{logs:?}");
        assert_eq!(logs[0].0, log::Level::Info);
        assert!(logs[0].1.contains("custom5"), "{}", logs[0].1);
        cleanup(&dir);
    }

    /// fix F3（review task-5.8-fixA-opus.md [low]）：資料目錄本身不存在（例如 data_dir 打錯）時
    /// 各來源檔都是「首次就不存在」而不記錄，記錄檔完全看不出原因。目錄層級要記一筆 warn（含
    /// 路徑），持續不存在不重複記錄；目錄出現時記一筆 info。目錄存在但沒有檔案是正常的尚無資料，
    /// 不記錄。
    #[test]
    fn missing_data_dir_is_logged_once_and_recovery_is_logged() {
        capture::start();
        let base = temp_dir("missing-data-dir");
        let dir = base.join("typo-data-dir");
        let mut registry = build_default_registry(&dir);

        registry.poll_all();
        registry.poll_all();
        registry.poll_all();
        let logs = capture::take();
        assert_eq!(logs.len(), 1, "資料目錄不存在應恰好記錄一筆：{logs:?}");
        assert_eq!(logs[0].0, log::Level::Warn);
        assert!(logs[0].1.contains("資料目錄"), "{}", logs[0].1);
        assert!(
            logs[0].1.contains("typo-data-dir"),
            "記錄要含路徑：{}",
            logs[0].1
        );

        fs::create_dir_all(&dir).expect("建立目錄失敗");
        registry.poll_all();
        registry.poll_all();
        let logs = capture::take();
        assert_eq!(logs.len(), 1, "資料目錄恢復應記錄一筆：{logs:?}");
        assert_eq!(logs[0].0, log::Level::Info);
        assert!(logs[0].1.contains("typo-data-dir"), "{}", logs[0].1);
        cleanup(&base);
    }

    #[test]
    fn existing_empty_data_dir_is_silent() {
        capture::start();
        let dir = temp_dir("empty-data-dir");
        let mut registry = build_default_registry(&dir);
        registry.poll_all();
        registry.poll_all();
        assert!(
            capture::take().is_empty(),
            "目錄存在、尚無檔案＝正常，不記錄"
        );
        cleanup(&dir);
    }

    /// fix F3（review task-5.8-fixA-opus.md [low] 第二點）：`Idle → Failed（非 NotFound）→ NotFound`
    /// 的檔案從來沒存在過，記錄不得寫成「已不存在」（暗示曾經存在後消失）。
    #[test]
    fn not_found_after_other_failure_on_never_present_file_is_not_called_vanished() {
        capture::start();
        let dir = temp_dir("never-present");
        let path = dir.join("custom2.json");
        let mut source = JsonFileSource::new("custom2", path.clone(), Box::new(validate_any_json));
        source.set_health(
            IoHealth::failed(IoStage::Metadata, &io::Error::from_raw_os_error(5)),
            None,
        );
        capture::take();

        assert!(source.poll().is_none());
        let logs = capture::take();
        assert_eq!(logs.len(), 1, "故障轉成不存在應記錄一筆：{logs:?}");
        assert!(
            !logs[0].1.contains("已不存在"),
            "從未存在過的檔案不得記成「已不存在」：{}",
            logs[0].1
        );
        assert!(logs[0].1.contains("custom2"), "{}", logs[0].1);
        cleanup(&dir);
    }

    /// D4 契約：`Snapshot`（含 `meta`）直接序列化時鍵為 `loadedAt`（task 2.7 已加
    /// `rename_all = "camelCase"`；本測試在 data 模組層級釘住，避免日後有人直接序列化
    /// `Snapshot` 時退回 snake_case）。
    #[test]
    fn snapshot_meta_serializes_loaded_at_as_camel_case() {
        let snapshot = Snapshot {
            channel: "custom1".to_string(),
            data: serde_json::json!({}),
            meta: SnapshotMeta {
                loaded_at: 42,
                fetched: None,
                updated: None,
            },
        };
        let json = serde_json::to_value(&snapshot).expect("序列化失敗");
        assert_eq!(json["meta"]["loadedAt"], 42);
        assert!(json["meta"].get("loaded_at").is_none());
    }

    // ── task 4.6：wallpaper 通道（design.md D7）──────────────────────────────────────

    fn wallpaper_config_file(dir: &Path, marker: &str) -> PathBuf {
        let mut cfg: Value = serde_json::from_str(crate::wallpaper_config::DEFAULT_CONFIG_JSON)
            .expect("內建預設應為合法 JSON");
        cfg["thresholds"] = serde_json::json!({ "marker": marker });
        let path = dir.join(crate::wallpaper_config::CONFIG_FILE_NAME);
        fs::write(&path, serde_json::to_vec_pretty(&cfg).expect("序列化失敗"))
            .expect("寫入設定檔失敗");
        settle();
        path
    }

    const TW_WITH_WALLPAPER_KEYS: &str = r#"{
        "updated": "2026-10-02 14:00", "fetched": "2026-10-02 14:00",
        "macro": [{"title": "CPI"}], "punish": [], "quotes": [{"name": "USD/TWD"}],
        "errors": ["x"],
        "events": [{"code": "2330", "type": "earnings"}],
        "twii_intraday": {"date": "2026-10-02", "points": [1, 2]},
        "margin": {"date": "2026-10-02"},
        "wallpaper_errors": []
    }"#;

    #[test]
    fn extract_wallpaper_data_keeps_only_present_wanted_keys() {
        let tw: Value = serde_json::from_str(TW_WITH_WALLPAPER_KEYS).expect("JSON");
        let data = extract_wallpaper_data(&tw);
        let mut keys: Vec<&str> = data
            .as_object()
            .expect("data 應為物件")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        // twii_daily、holidays 缺 → 不帶；macro／quotes／punish／errors／fetched 不是桌布要的 → 不帶。
        assert_eq!(
            keys,
            vec!["events", "margin", "twii_intraday", "wallpaper_errors"]
        );
        assert_eq!(data["twii_intraday"], tw["twii_intraday"], "值原樣帶過去");
        let mut wanted = WALLPAPER_DATA_KEYS.to_vec();
        wanted.sort_unstable();
        assert_eq!(
            wanted,
            vec![
                "events",
                "holidays",
                "margin",
                "twii_daily",
                "twii_intraday",
                "wallpaper_errors"
            ]
        );
    }

    #[test]
    fn extract_wallpaper_data_of_non_object_is_empty_object() {
        assert_eq!(
            extract_wallpaper_data(&serde_json::json!([1, 2])),
            serde_json::json!({})
        );
    }

    #[test]
    fn wallpaper_payload_is_data_config_envelope() {
        let tw: Value = serde_json::from_str(TW_WITH_WALLPAPER_KEYS).expect("JSON");
        let config = serde_json::json!({ "version": 1 });
        let payload = wallpaper_payload(Some(&tw), &config);
        let obj = payload.as_object().expect("payload 應為物件");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["config", "data"], "形狀只有 data 與 config");
        assert_eq!(payload["config"], config);
        assert_eq!(payload["data"], extract_wallpaper_data(&tw));

        // 尚無 tw-events：data 為空物件（頁面判成信封，主題依缺鍵畫缺資料畫面），設定照帶。
        let bare = wallpaper_payload(None, &config);
        assert_eq!(
            bare,
            serde_json::json!({ "data": {}, "config": { "version": 1 } })
        );
    }

    #[test]
    fn registry_without_wallpaper_watch_has_no_wallpaper_channel() {
        let dir = temp_dir("wallpaper-unwatched");
        write(&dir, TW_EVENTS_FILE, TW_WITH_WALLPAPER_KEYS);
        let mut registry = build_default_registry(&dir);
        let changed = registry.poll_all_changed();
        assert_eq!(changed, vec![TW_EVENTS_CHANNEL.to_string()]);
        assert!(registry.snapshot(WALLPAPER_CHANNEL).is_none());
        cleanup(&dir);
    }

    #[test]
    fn wallpaper_channel_derives_data_and_config_from_tw_events_and_config_file() {
        let dir = temp_dir("wallpaper-derive");
        write(&dir, TW_EVENTS_FILE, TW_WITH_WALLPAPER_KEYS);
        let config_path = wallpaper_config_file(&dir, "first");
        let mut registry = build_default_registry(&dir);
        registry.watch_wallpaper_config(&config_path);

        let changed = registry.poll_all_changed();
        assert!(
            changed.contains(&TW_EVENTS_CHANNEL.to_string()),
            "{changed:?}"
        );
        assert!(
            changed.contains(&WALLPAPER_CHANNEL.to_string()),
            "{changed:?}"
        );

        let tw = registry
            .snapshot(TW_EVENTS_CHANNEL)
            .expect("tw-events")
            .clone();
        let wp = registry
            .snapshot(WALLPAPER_CHANNEL)
            .expect("wallpaper 應有快照");
        assert_eq!(wp.channel, WALLPAPER_CHANNEL);
        assert_eq!(wp.data["data"], extract_wallpaper_data(&tw.data));
        assert_eq!(wp.data["config"]["thresholds"]["marker"], "first");
        assert!(
            wp.data["config"]["markets"].is_array(),
            "config 是頂層合併後的完整設定"
        );
        assert_eq!(
            wp.meta.fetched, tw.meta.fetched,
            "meta 沿用 tw-events 的 fetched"
        );
        assert_eq!(wp.meta.updated, tw.meta.updated);

        assert!(
            registry.poll_all_changed().is_empty(),
            "兩個來源都沒變 → 這輪沒有任何通道變更"
        );
        cleanup(&dir);
    }

    #[test]
    fn wallpaper_channel_changes_when_only_config_file_changes() {
        let dir = temp_dir("wallpaper-config-change");
        write(&dir, TW_EVENTS_FILE, TW_WITH_WALLPAPER_KEYS);
        let config_path = wallpaper_config_file(&dir, "v1");
        let mut registry = build_default_registry(&dir);
        registry.watch_wallpaper_config(&config_path);
        registry.poll_all_changed();

        wallpaper_config_file(&dir, "v2");
        let changed = registry.poll_all_changed();
        assert_eq!(
            changed,
            vec![WALLPAPER_CHANNEL.to_string()],
            "只有設定變 → 只有 wallpaper"
        );
        let wp = registry.snapshot(WALLPAPER_CHANNEL).expect("wallpaper");
        assert_eq!(wp.data["config"]["thresholds"]["marker"], "v2");
        cleanup(&dir);
    }

    #[test]
    fn wallpaper_channel_follows_tw_events_changes_and_keeps_tw_events_unchanged() {
        let dir = temp_dir("wallpaper-tw-change");
        write(&dir, TW_EVENTS_FILE, TW_WITH_WALLPAPER_KEYS);
        let config_path = wallpaper_config_file(&dir, "same");
        let mut registry = build_default_registry(&dir);
        registry.watch_wallpaper_config(&config_path);
        registry.poll_all_changed();

        let updated = TW_WITH_WALLPAPER_KEYS.replace("\"points\": [1, 2]", "\"points\": [1, 2, 3]");
        write(&dir, TW_EVENTS_FILE, &updated);
        let mut changed = registry.poll_all_changed();
        changed.sort();
        assert_eq!(
            changed,
            vec![TW_EVENTS_CHANNEL.to_string(), WALLPAPER_CHANNEL.to_string()]
        );
        let tw = registry.snapshot(TW_EVENTS_CHANNEL).expect("tw-events");
        let expected_tw: Value = serde_json::from_str(&updated).expect("JSON");
        assert_eq!(
            tw.data, expected_tw,
            "tw-events 通道內容不受影響（完整、未擷取）"
        );
        let wp = registry.snapshot(WALLPAPER_CHANNEL).expect("wallpaper");
        assert_eq!(
            wp.data["data"]["twii_intraday"]["points"],
            serde_json::json!([1, 2, 3])
        );
        cleanup(&dir);
    }

    #[test]
    fn wallpaper_channel_exists_without_tw_events_and_carries_config() {
        let dir = temp_dir("wallpaper-no-tw");
        let config_path = wallpaper_config_file(&dir, "lonely");
        let mut registry = build_default_registry(&dir);
        registry.watch_wallpaper_config(&config_path);
        let changed = registry.poll_all_changed();
        assert_eq!(changed, vec![WALLPAPER_CHANNEL.to_string()]);
        let wp = registry.snapshot(WALLPAPER_CHANNEL).expect("wallpaper");
        assert_eq!(wp.data["data"], serde_json::json!({}));
        assert_eq!(wp.data["config"]["thresholds"]["marker"], "lonely");
        assert!(wp.meta.fetched.is_none());
        cleanup(&dir);
    }

    #[test]
    fn wallpaper_channel_survives_registry_rebuild_and_is_recomputed_next_poll() {
        let base = temp_dir("wallpaper-rebuild");
        let old_dir = base.join("old");
        let new_dir = base.join("new");
        fs::create_dir_all(&old_dir).expect("建立目錄失敗");
        fs::create_dir_all(&new_dir).expect("建立目錄失敗");
        write(&old_dir, TW_EVENTS_FILE, TW_WITH_WALLPAPER_KEYS);
        let config_path = wallpaper_config_file(&base, "kept");
        let mut registry = build_default_registry(&old_dir);
        registry.watch_wallpaper_config(&config_path);
        registry.poll_all_changed();
        assert!(registry.snapshot(WALLPAPER_CHANNEL).is_some());

        registry.replace_with(build_default_registry(&new_dir));
        assert!(
            registry.snapshot(WALLPAPER_CHANNEL).is_none(),
            "重建後與其他通道一樣回到尚無資料"
        );
        let changed = registry.poll_all_changed();
        assert_eq!(
            changed,
            vec![WALLPAPER_CHANNEL.to_string()],
            "新目錄沒有 tw_events、設定也沒變，仍要重算 wallpaper（否則永遠停在尚無資料）"
        );
        let wp = registry.snapshot(WALLPAPER_CHANNEL).expect("wallpaper");
        assert_eq!(
            wp.data["data"],
            serde_json::json!({}),
            "不得殘留舊目錄的資料"
        );
        assert_eq!(
            wp.data["config"]["thresholds"]["marker"], "kept",
            "設定沿用"
        );
        cleanup(&base);
    }
    /// task 4.7a：以啟動時的載入結果開始監看——第一輪仍會組出 `wallpaper` 快照（dirty），設定版本
    /// 從 0 開始；設定檔改變後版本遞增（協調迴圈據此重畫）。沒有監看時版本為 `None`。
    #[test]
    fn primed_wallpaper_config_reports_version_and_still_builds_first_snapshot() {
        let dir = temp_dir("wallpaper-primed");
        let config_path = wallpaper_config_file(&dir, "boot");
        let cfg = crate::wallpaper_config::reload(&config_path);
        let mut registry = build_default_registry(&dir);
        assert_eq!(registry.wallpaper_config_version(), None);
        registry.watch_wallpaper_config_primed(&config_path, &cfg);
        assert_eq!(registry.wallpaper_config_version(), Some(0));
        assert_eq!(
            registry.poll_all_changed(),
            vec![WALLPAPER_CHANNEL.to_string()]
        );
        let wp = registry.snapshot(WALLPAPER_CHANNEL).expect("wallpaper");
        assert_eq!(wp.data["config"]["thresholds"]["marker"], "boot");
        assert_eq!(registry.wallpaper_config_version(), Some(0));
        wallpaper_config_file(&dir, "edited");
        registry.poll_all_changed();
        assert_eq!(registry.wallpaper_config_version(), Some(1));
        cleanup(&dir);
    }
}
