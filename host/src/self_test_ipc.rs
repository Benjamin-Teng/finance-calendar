//! Task 2.7 IPC 驗收（`--self-test-ipc`，需 `self-test-ipc` cargo feature；預設 build 不含
//! 這個模組，不會出現在正式產品的 release exe）。
//!
//! ## 為什麼不是 `examples/`
//!
//! 本 crate 沒有 `[lib]` target（`Cargo.toml` 只有 `[[bin]] name = "fc-host"`），
//! `examples/*.rs` 各自是獨立 crate，看不到 `mod widgets`／`mod data`／`mod settings`
//! 這些 private 模組（既有探針 `probe_monitors.rs`／`probe_wind.rs` 因此都是各自重刻一份
//! Win32 邏輯，不是重用 `src/` 的程式碼——查證過兩份原始碼，皆自帶 `use windows::Win32::...`
//! 而非 `use fc_host::...`）。本 task 要驗證的正是 `widgets.rs` 的五個真正 IPC 指令與
//! `poll_and_notify` 本身（不是重新刻一份等價邏輯，那樣驗不出真正的程式碼有沒有問題），唯一
//! 能同時「拿到這些私有模組」又「以真正的 Tauri release build 跑」的辦法，是留在同一個
//! binary crate 裡、用 CLI 旗標切換成測試模式的 `tauri::Builder`（取代正常產品的那個），而非
//! 另開一個 crate。`self-test-ipc` feature 預設不開，不影響 `cargo build --release`
//! （不加 `--features`）產出的正式 exe。
//!
//! ## 驗證內容（task 2.7 brief 要求）
//!
//! - **「先 listen 再 get」**：`host/ui/test-ipc.html` 先以 `bridge.js` 的
//!   `listen('data')`（`subscribe_data`＋Channel）與 `listen('settings'|'edit-mode')` 掛好
//!   監聽器，才呼叫 `get_snapshot`。
//! - **投遞限制（「未訂閱的通道不會收到」）**：開三個測試視窗——`w-macro`
//!   （`widgets::WIDGET_SPECS` 裡訂閱 `tw-events`）、`w-custom1`（訂閱 `custom1`）與
//!   `w-probe`（不在對照表內＝未訂閱任何通道；fix round 1，Codex 2.7 的負向測試）。三個視窗
//!   都回報完 `snapshot` 後同時改動兩個通道的來源檔，呼叫真正的
//!   [`crate::widgets::poll_and_notify`]（產品程式碼本身，不是替身）。每個頁面另掛裸
//!   `event.listen('data')` 與 `win.listen('data')` 兩個負向探針。結束前 [`log_verdict`]
//!   自動判定：任何視窗、任何路徑收到非自己訂閱的通道即 FAIL；`w-macro`／`w-custom1` 必須從
//!   正規路徑收到自己的通道；`w-probe` 的 `subscribe_data` 必須被拒絕。
//! - **`wallpaper` 通道只給渲染視窗（dynamic-wallpaper task 4.6）**：第四個視窗
//!   `wallpaper-renderer-1`（`wallpaper_render::is_renderer_label` 為真）訂閱並查詢 `wallpaper`；
//!   觸發時改動 `tw_events.json`，`poll_and_notify` 衍生出新的 `wallpaper` 快照。判定：
//!   - 渲染視窗必須從正規路徑（Channel）收到 `wallpaper`，且**內容**正確：`{ data, config }` 信封、
//!     `config.selfTestMarker == "4.6"`、`data` 的鍵恰好是 `events`／`holidays`（修正輪 1，審查 low 1：
//!     只核形狀會讓內容錯的 payload 也 PASS）；
//!   - 三個非渲染視窗的 `get_snapshot('wallpaper')` 必須被拒絕、渲染視窗的必須成功且內容同上；
//!   - 四個視窗的裸 `event.listen('data')`／`win.listen('data')` 都不得收到任何東西（沿用上面的
//!     外洩判定：收到非自己的通道即 FAIL）；另在 `BARE_PROBE_EVENTS` 的每個事件名（宿主會 emit 的
//!     全部事件＋`data`／`wallpaper`）掛裸 listen，payload 含設定檔標記或資料標記即 FAIL，且每個視窗都必須
//!     回報全部掛上。
//!
//!   正式宿主的渲染視窗不屬於 `capabilities/default.json` 的 `w-*`／`settings`，沒有任何 core
//!   權限（頁面只用自訂指令，自訂指令不經 ACL）。測試頁要用 `emit`／`event.listen` 回報與當負向
//!   探針，所以本模組在**測試行程內**以 `add_capability` 給 `wallpaper-renderer-*` 加
//!   `core:default`——這只會讓渲染視窗「能」裸 listen，用來證明即使能 listen 也收不到，比正式
//!   環境更嚴格。渲染視窗用測試資料夾下自己的 WebView2 使用者資料夾（同正式：獨立環境）。
//! - **Command 權限**：`host/capabilities/default.json` 完全沒有為這五個指令加任何
//!   `permissions` 項目（本 task 未修改該檔），驗證 release build 下五個指令仍然全部
//!   `invoke` 成功——呼應 `widgets.rs` 模組文件「Command 權限」一節查證的 Tauri v2 預設
//!   行為（沒有 app ACL manifest 時，本機來源呼叫自訂指令整段略過 ACL 檢查）。
//!
//! 每一步結果都由頁面以 `emit('probe-report', JSON.stringify(...))` 回報（JS→Rust 事件，
//! `allow-emit` 已在 `core:event:default`／`core:default` 內，不需要額外權限——比照
//! task 2.2 `probe_acl.rs` 的既有做法），本模組把每一筆都寫進 `--log` 指定的記錄檔，逐行帶
//! 時間戳，供事後查驗（`host/tools/evidence/2.7-*.log`）。
//!
//! ## 用法
//!
//! ```text
//! cargo run --release --features self-test-ipc --bin fc-host -- --self-test-ipc --log <path> [--timeout-secs N]
//! ```
//!
//! `--timeout-secs`（預設 10）到期後自動結束行程；正常情況下驗證流程會在數百毫秒內全部
//! 跑完（本機 IPC，沒有真正的網路或 30 秒輪詢等待）。

use std::{
    collections::HashSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    thread,
    time::Duration,
};

use tauri::{Emitter, Listener, Manager, WebviewUrl, WebviewWindowBuilder};
use windows::Win32::System::SystemInformation::GetLocalTime;

use crate::settings::Settings;
use crate::widgets::{self, AppState};

static LOG: OnceLock<Mutex<fs::File>> = OnceLock::new();

fn now() -> String {
    // SAFETY: GetLocalTime 只寫回傳值，無前置條件（與 probe_acl.rs 相同用法）。
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

// task 4.6：加 `holidays`（wallpaper 通道會擷取的鍵），渲染視窗收到的 `data` 才不是空物件；修正輪 2
// （複審 L3）：`events` 帶一筆含資料標記 [`WALLPAPER_DATA_MARKER`] 的法說會，用來偵測 `data` 部分外洩。
const INITIAL_TW_EVENTS: &str = r#"{"updated":"2026-09-28 08:00","fetched":"2026-09-28 08:00","macro":[{"title":"初始 CPI"}],"events":[{"code":"2330","type":"conference","note":"wallpaper-data-marker-4.6"}],"punish":[],"quotes":[],"holidays":["2026-10-10"]}"#;
const UPDATED_TW_EVENTS: &str = r#"{"updated":"2026-09-28 09:00","fetched":"2026-09-28 09:00","macro":[{"title":"更新後 CPI"}],"events":[{"code":"2330","type":"conference","note":"wallpaper-data-marker-4.6"}],"punish":[],"quotes":[],"holidays":["2026-10-10","2026-10-26"]}"#;
const INITIAL_CUSTOM1: &str = r#"{"marker":"initial"}"#;
const UPDATED_CUSTOM1: &str = r#"{"marker":"updated"}"#;

fn write_fixture(dir: &Path, name: &str, content: &str) {
    fs::write(dir.join(name), content).expect("寫入 fixture 失敗");
}

/// 測試用的桌布渲染視窗 label（dynamic-wallpaper task 4.6）：`wallpaper-renderer-<數字>`，與正式
/// 渲染視窗同一個前綴規則（`wallpaper_render::renderer_label`）。
const RENDERER_LABEL: &str = "wallpaper-renderer-1";

/// 主題設定檔 fixture 頂層多加的 `selfTestMarker` 值（task 4.6）。
const WALLPAPER_CONFIG_MARKER: &str = "4.6";

/// tw-events fixture 的 `events` 裡一筆資料的獨特值（修正輪 2，複審 L3）：渲染視窗收到的 `data` 必須
/// 含它；裸 listen 在任何事件看到它＝wallpaper 的 `data` 部分外洩（頁面同名常數 `DATA_MARKER`）。
const WALLPAPER_DATA_MARKER: &str = "wallpaper-data-marker-4.6";

/// `wallpaper` 的 `data` 必須**恰好**是這些鍵（已排序）：兩份 tw-events fixture 都保證有 `events`
/// 與 `holidays`，而 `macro`／`punish`／`quotes`／`fetched`／`updated` 不在擷取清單內、不得出現
/// （task 4.6 修正輪 1，審查 low 1：只核形狀會讓內容錯的 payload 也 PASS）。
const EXPECTED_WALLPAPER_DATA_KEYS: [&str; 2] = ["events", "holidays"];

/// 每個頁面以**裸** `window.__TAURI__.event.listen`（target＝Any，`emit_filter`／`emit_to` 擋不住）
/// 掛上的事件名（task 4.6 修正輪 1，審查 low 1）。`wallpaper` 資料本體只走 `ipc::Channel`
/// （`subscribe_data`）與 `get_snapshot` 的回應，Channel 的回呼是對單一 webview `eval`、或該
/// webview 自己的 `plugin:__TAURI_CHANNEL__|fetch`，**不經事件系統**——所以「資料可能搭的事件」
/// 就是宿主會 emit 的全部事件名：`settings`／`edit-mode`／`pause`／`edit-preview`／`widget-font`
/// （`widgets.rs`；`widget-font` 為 widget-font-scale-per-widget design.md D3）、
/// 本 self-test 的 `drive-now`，再加上本來就不存在、但若有人日後誤加最可能用的 `data` 與
/// `wallpaper`。單元測試 `bare_probe_events_cover_every_event_the_host_emits` 掃 `src/` 的
/// `.emit*(`／UFCS `::emit*(` 呼叫，宿主新增事件而這裡沒跟上就失敗；頁面端的同名清單也由該測試核對
/// 一致。頁面對每個事件檢查 payload 是否含設定檔標記或資料標記（[`WALLPAPER_DATA_MARKER`]），含就回報
/// `bare-wallpaper-leak`。
const BARE_PROBE_EVENTS: [&str; 8] = [
    "data",
    "wallpaper",
    "settings",
    "edit-mode",
    "pause",
    "edit-preview",
    "widget-font",
    "drive-now",
];

/// 參與驗證的四個視窗：label → 查詢的通道 → 該視窗「應該」收到的唯一通道（`None`＝未訂閱任何
/// 通道）。`w-probe` 不在 `widgets::WIDGET_SPECS` 裡（fix round 1，Codex 2.7 負向測試）：它代表
/// 「未訂閱任何通道、卻用裸 `event.listen('data')` 監聽」的頁面，不得收到任何通道的快照。
/// [`RENDERER_LABEL`]（task 4.6）是唯一應收到 `wallpaper` 的視窗。
const PROBE_WINDOWS: [(&str, &str, Option<&str>); 4] = [
    ("w-macro", "tw-events", Some("tw-events")),
    ("w-custom1", "custom1", Some("custom1")),
    ("w-probe", "none", None),
    (RENDERER_LABEL, "wallpaper", Some("wallpaper")),
];

/// 頁面對一份 `wallpaper` payload（`snapshot.data` 或 `get_snapshot` 回應的 `data`）的觀察。
#[derive(Debug, Clone, Default)]
struct WallpaperContent {
    /// `{ data: 物件, config: 物件 }`。
    envelope: bool,
    /// `config.selfTestMarker`。
    marker: Option<String>,
    /// `Object.keys(data)`；`data` 不是物件時 `None`。
    data_keys: Option<Vec<String>>,
    /// `data` 的 JSON 文字含 [`WALLPAPER_DATA_MARKER`]。
    data_marker: bool,
}

impl WallpaperContent {
    #[cfg(test)]
    fn none() -> Self {
        Self::default()
    }

    /// 從頁面回報的 `envelope`／`marker`／`dataKeys` 欄位讀出。
    fn from_report(v: &serde_json::Value) -> Self {
        Self {
            envelope: v
                .get("envelope")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            marker: v.get("marker").and_then(|m| m.as_str()).map(str::to_owned),
            data_keys: v.get("dataKeys").and_then(|k| k.as_array()).map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_owned))
                    .collect()
            }),
            data_marker: v
                .get("dataMarker")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        }
    }

    /// 內容是否正確；不正確時回傳原因。
    fn problem(&self) -> Option<String> {
        let mut keys = self.data_keys.clone().unwrap_or_default();
        keys.sort_unstable();
        let ok = self.envelope
            && self.marker.as_deref() == Some(WALLPAPER_CONFIG_MARKER)
            && self.data_keys.is_some()
            && keys == EXPECTED_WALLPAPER_DATA_KEYS
            && self.data_marker;
        (!ok).then(|| {
            format!(
                "envelope={} marker={:?} dataKeys={:?} dataMarker={}（應為信封、marker={WALLPAPER_CONFIG_MARKER:?}、dataKeys={EXPECTED_WALLPAPER_DATA_KEYS:?}、data 含 {WALLPAPER_DATA_MARKER:?}）",
                self.envelope, self.marker, self.data_keys, self.data_marker
            )
        })
    }
}

/// 頁面回報中與 `data` 投遞有關的一筆：哪個視窗、哪條路徑（`kind`）、收到哪個通道；`content`＝
/// 對 `snapshot.data` 的觀察（只對 `wallpaper` 有意義）；`marker_seen`＝整份 payload 的 JSON 文字
/// 含設定檔標記（任何通道都可能帶，非渲染視窗出現即外洩）。
struct DataReport {
    label: String,
    kind: String,
    channel: String,
    content: WallpaperContent,
    marker_seen: bool,
}

/// 每個視窗以 `get_snapshot('wallpaper')` 探測的結果（task 4.6）：`allowed`＝invoke 成功。
struct WallpaperProbe {
    label: String,
    allowed: bool,
    content: WallpaperContent,
}

/// 執行期間收集到的全部回報。
#[derive(Default)]
struct Collected {
    reports: Vec<DataReport>,
    /// `subscribe_data` 被拒的視窗。
    rejected: Vec<String>,
    probes: Vec<WallpaperProbe>,
    /// (視窗, 事件名)：裸 listen 在該事件收到含設定檔標記的 payload。
    content_leaks: Vec<(String, String)>,
    /// (視窗, 已掛上的裸 listen 事件名)。
    installed: Vec<(String, Vec<String>)>,
}

/// 依收集到的回報判定，回傳所有失敗（空＝PASS）。
/// - D4：任何視窗、任何監聽路徑收到「不是自己訂閱的通道」＝外洩（`leak`）；有訂閱的視窗必須從
///   正規路徑（`kind == "data-received"`）收到自己的通道（`missing`）；未訂閱的視窗（`own == None`）
///   的 `subscribe_data` 必須被拒絕（`not-rejected`）。
/// - task 4.6：渲染視窗從正規路徑收到的每一筆 `wallpaper` 與 `get_snapshot('wallpaper')` 回應都要
///   內容正確——信封、設定檔標記、`data` 恰好是 [`EXPECTED_WALLPAPER_DATA_KEYS`]（`wrong-content`）；
///   每個視窗都要探測過（`wallpaper-probe-missing`），非渲染視窗必須被拒（`wallpaper-query-leak`）、
///   渲染視窗必須成功（`wallpaper-query-denied`）。
/// - task 4.6 修正輪 1：每個視窗都要在 [`BARE_PROBE_EVENTS`] 的每個事件名掛好裸 listen
///   （`bare-probes-missing`）；任何非渲染視窗在任何事件或任何資料回報裡看到設定檔標記
///   （`wallpaper-content-leak`）即 FAIL。
fn verdict_failures(c: &Collected) -> Vec<String> {
    let mut failures = Vec::new();
    for (label, _, own) in PROBE_WINDOWS {
        if own.is_none() && !c.rejected.iter().any(|r| r == label) {
            failures.push(format!(
                "VERDICT-FAIL not-rejected label={label} (未訂閱視窗的 subscribe_data 應被拒絕)"
            ));
        }
    }
    for r in &c.reports {
        let own = PROBE_WINDOWS
            .iter()
            .find(|(label, _, _)| *label == r.label)
            .and_then(|(_, _, own)| *own);
        if own != Some(r.channel.as_str()) {
            failures.push(format!(
                "VERDICT-FAIL leak label={} kind={} channel={} (own={own:?})",
                r.label, r.kind, r.channel
            ));
        }
        if r.marker_seen && !crate::wallpaper_render::is_renderer_label(&r.label) {
            failures.push(format!(
                "VERDICT-FAIL wallpaper-content-leak label={} kind={} channel={} (非渲染視窗的回報含設定檔標記)",
                r.label, r.kind, r.channel
            ));
        }
    }
    for (label, event) in &c.content_leaks {
        failures.push(format!(
            "VERDICT-FAIL wallpaper-content-leak label={label} event={event} (裸 listen 收到含設定檔標記的 payload)"
        ));
    }
    for (label, _, own) in PROBE_WINDOWS {
        let Some(own) = own else { continue };
        let delivered: Vec<&DataReport> = c
            .reports
            .iter()
            .filter(|r| r.label == label && r.kind == "data-received" && r.channel == own)
            .collect();
        if delivered.is_empty() {
            failures.push(format!(
                "VERDICT-FAIL missing label={label} channel={own} (正規路徑未收到自己的通道)"
            ));
        } else if own == crate::data::WALLPAPER_CHANNEL {
            for r in delivered {
                if let Some(why) = r.content.problem() {
                    failures.push(format!(
                        "VERDICT-FAIL wrong-content label={label} via=data-received {why}"
                    ));
                }
            }
        }
    }
    for (label, _, _) in PROBE_WINDOWS {
        let mine: Vec<&WallpaperProbe> = c.probes.iter().filter(|p| p.label == label).collect();
        if mine.is_empty() {
            failures.push(format!(
                "VERDICT-FAIL wallpaper-probe-missing label={label} (沒有回報 get_snapshot('wallpaper') 的結果)"
            ));
            continue;
        }
        let renderer = crate::wallpaper_render::is_renderer_label(label);
        for p in mine {
            if !renderer && p.allowed {
                failures.push(format!(
                    "VERDICT-FAIL wallpaper-query-leak label={label} (非渲染視窗查得到 wallpaper)"
                ));
            }
            if renderer && !p.allowed {
                failures.push(format!(
                    "VERDICT-FAIL wallpaper-query-denied label={label} (渲染視窗應查得到 wallpaper)"
                ));
            }
            if renderer && p.allowed {
                if let Some(why) = p.content.problem() {
                    failures.push(format!(
                        "VERDICT-FAIL wrong-content label={label} via=get_snapshot {why}"
                    ));
                }
            }
        }
    }
    for (label, _, _) in PROBE_WINDOWS {
        let installed: Vec<&String> = c
            .installed
            .iter()
            .filter(|(l, _)| l == label)
            .flat_map(|(_, events)| events)
            .collect();
        let missing: Vec<&str> = BARE_PROBE_EVENTS
            .iter()
            .copied()
            .filter(|e| !installed.iter().any(|i| i.as_str() == *e))
            .collect();
        if !missing.is_empty() {
            failures.push(format!(
                "VERDICT-FAIL bare-probes-missing label={label} events={missing:?} (裸 listen 探針沒有全部掛上)"
            ));
        }
    }
    failures
}

/// [`verdict_failures`] 的結果逐條寫進記錄檔，最後一行 `VERDICT PASS|FAIL`。
fn log_verdict(c: &Collected) {
    let failures = verdict_failures(c);
    for f in &failures {
        log(f);
    }
    log(&format!(
        "VERDICT {} dataReports={} wallpaperProbes={} bareProbeWindows={}",
        if failures.is_empty() { "PASS" } else { "FAIL" },
        c.reports.len(),
        c.probes.len(),
        c.installed.len()
    ));
}

/// `main.rs` 解析出 `--self-test-ipc` 後呼叫；接手整個行程，不會再啟動正常的產品視窗與
/// 正常的排程執行緒。
pub fn run(log_path: PathBuf, timeout_secs: u64) {
    if let Some(dir) = log_path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let file = fs::File::create(&log_path).expect("無法建立記錄檔");
    let _ = LOG.set(Mutex::new(file));
    log(&format!(
        "START rustPid={} timeoutSecs={}",
        std::process::id(),
        timeout_secs
    ));

    // 隔離的資料目錄：不能借用真正的 %LOCALAPPDATA%\tw.fintools.fc-host\data（可能不存在，
    // 也不該讓一次性的驗證探針動到使用者的正式資料），fixture 自己準備、結束時自己收尾。
    let data_dir =
        std::env::temp_dir().join(format!("fc-host-self-test-ipc-{}", std::process::id()));
    let _ = fs::remove_dir_all(&data_dir);
    fs::create_dir_all(&data_dir).expect("建立測試資料目錄失敗");
    write_fixture(&data_dir, "tw_events.json", INITIAL_TW_EVENTS);
    write_fixture(&data_dir, "custom1.json", INITIAL_CUSTOM1);
    // task 4.6：主題設定檔與 settings.json 同資料夾（`AppState::new` 依 settings_path 決定）；以內建
    // 預設為底、頂層多一個標記鍵（多出來的鍵原樣保留），記錄檔可對照渲染視窗收到的確實是這份。
    let wallpaper_config = crate::wallpaper_config::DEFAULT_CONFIG_JSON.replacen(
        '{',
        &format!("{{\"selfTestMarker\":\"{WALLPAPER_CONFIG_MARKER}\","),
        1,
    );
    write_fixture(
        &data_dir,
        crate::wallpaper_config::CONFIG_FILE_NAME,
        &wallpaper_config,
    );
    log(&format!("FIXTURE dataDir={}", data_dir.display()));

    let settings = Settings {
        data_dir: data_dir.clone(),
        ..Settings::default()
    };
    let settings_path = data_dir.join("settings.json");
    let state = AppState::new(settings, settings_path);
    // 開視窗前先做一次初始輪詢，讓兩個通道在頁面出現時就已經有快照——比照production
    // main.rs 的「啟動時立即輪詢一次」，也避免把「這次驗證用的探針忘記先填資料」誤判成
    // 「get_snapshot 有 bug」。之後頁面「先 listen 再 get」量到的就是這份初始快照。
    state
        .registry
        .lock()
        .expect("registry mutex poisoned")
        .poll_all();
    log("INITIAL poll_all done (tw-events + custom1 + wallpaper should now be status=ok)");

    let data_dir_for_trigger = data_dir.clone();
    // 兩個測試視窗都回報過 "snapshot" 之後才觸發資料變更——確保接下來收到的 `data` 事件不可能
    // 是「listen 掛上前就已經錯過」的僥倖時序，而是貨真價實驗證了 D4「先 listen 再 get」的
    // 啟動順序本身不會漏事件。
    let ready: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
    let triggered = Arc::new(AtomicBool::new(false));
    let collected: Arc<Mutex<Collected>> = Arc::new(Mutex::new(Collected::default()));
    let renderer_data_dir = data_dir.join("renderer-webview2");

    let app = tauri::Builder::default()
        .manage(state)
        // 這五個指令沒有、也不需要額外的 capability 宣告，見 `widgets.rs` 模組文件
        // 「Command 權限」一節；`host/capabilities/default.json` 在本 task 沒有任何改動。
        .invoke_handler(tauri::generate_handler![
            widgets::get_snapshot,
            widgets::subscribe_data,
            widgets::get_settings,
            widgets::update_settings,
            widgets::set_edit_mode,
            widgets::report_content,
        ])
        .setup(move |app| {
            // 關鍵順序：先掛 Rust 端的 `probe-report` 監聽器，才建立任何視窗。Tauri 的事件
            // 是「即時發送、不緩衝」——監聽器掛上之前發出的 emit 直接消失、不會補送。頁面載入
            // 後幾乎立刻就會 emit 第一筆回報，若監聽器晚於視窗建立才掛上，會在實測中真的漏掉
            // 最早的幾筆報告（本檔開發時曾經在本機用「先建視窗、後掛監聽器」的順序跑過一次，
            // 記錄檔裡 `w-macro` 最早的 `listening`／`snapshot` 兩筆報告確實不見了，只留下
            // 它後續的 `settings`／`update_settings-ok` 等——直接證實了這個順序陷阱，因此改成
            // 現在這個順序）。
            let handle = app.handle().clone();
            // task 4.6：正式的渲染視窗沒有任何 capability（不在 default.json 的 `w-*`／`settings`），
            // 測試頁卻要 `emit` 回報、要裸 `event.listen` 當負向探針，所以只在本測試行程內給
            // `wallpaper-renderer-*` 加 `core:default`（見模組文件）。
            app.add_capability(
                tauri::ipc::CapabilityBuilder::new("self-test-wallpaper-renderer")
                    .window("wallpaper-renderer-*")
                    .permission("core:default"),
            )?;
            log("CAPABILITY self-test-wallpaper-renderer added (core:default for wallpaper-renderer-*)");
            {
                let handle = handle.clone();
                let ready = ready.clone();
                let triggered = triggered.clone();
                let data_dir_for_trigger = data_dir_for_trigger.clone();
                let collected = collected.clone();
                app.listen_any("probe-report", move |event| {
                    let raw = event.payload().to_string();
                    log(&format!("REPORT {raw}"));

                    // payload 是 JSON 字串（頁面 emit 的是 JSON.stringify 過的字串）：先解一層
                    // JSON string 編碼，再把內容當 JSON 物件解析。
                    let Ok(unquoted) = serde_json::from_str::<String>(&raw) else {
                        return;
                    };
                    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&unquoted) else {
                        return;
                    };
                    let label = parsed.get("label").and_then(|v| v.as_str()).unwrap_or("");
                    let kind = parsed.get("kind").and_then(|v| v.as_str()).unwrap_or("");

                    // 所有 `*data-received` 回報（正規路徑與負向探針）都收集起來供最後判定；
                    // 頁面在回報裡直接帶上收到的通道名（`channel` 欄位）。
                    if kind.ends_with("data-received") {
                        let channel = parsed
                            .get("channel")
                            .and_then(|v| v.as_str())
                            .unwrap_or("<missing>");
                        let report = DataReport {
                            label: label.to_string(),
                            kind: kind.to_string(),
                            channel: channel.to_string(),
                            content: WallpaperContent::from_report(&parsed),
                            marker_seen: parsed
                                .get("markerSeen")
                                .and_then(serde_json::Value::as_bool)
                                .unwrap_or(false),
                        };
                        collected
                            .lock()
                            .expect("collected mutex poisoned")
                            .reports
                            .push(report);
                        return;
                    }

                    // task 4.6：每個視窗以 get_snapshot('wallpaper') 探測的結果。
                    if kind == "wallpaper-snapshot" {
                        let probe = WallpaperProbe {
                            label: label.to_string(),
                            allowed: parsed
                                .get("allowed")
                                .and_then(serde_json::Value::as_bool)
                                .unwrap_or(false),
                            content: WallpaperContent::from_report(&parsed),
                        };
                        collected
                            .lock()
                            .expect("collected mutex poisoned")
                            .probes
                            .push(probe);
                        return;
                    }

                    // task 4.6 修正輪 1：裸 listen 在某個事件收到含設定檔標記的 payload。
                    if kind == "bare-wallpaper-leak" {
                        let event = parsed
                            .get("event")
                            .and_then(|v| v.as_str())
                            .unwrap_or("<missing>");
                        collected
                            .lock()
                            .expect("collected mutex poisoned")
                            .content_leaks
                            .push((label.to_string(), event.to_string()));
                        return;
                    }

                    // task 4.6 修正輪 1：頁面實際掛上的裸 listen 事件名。
                    if kind == "bare-probes-installed" {
                        let events: Vec<String> = parsed
                            .get("events")
                            .and_then(|v| v.as_array())
                            .map(|a| {
                                a.iter()
                                    .filter_map(|e| e.as_str().map(str::to_owned))
                                    .collect()
                            })
                            .unwrap_or_default();
                        collected
                            .lock()
                            .expect("collected mutex poisoned")
                            .installed
                            .push((label.to_string(), events));
                        return;
                    }

                    if kind == "subscribe-rejected" {
                        collected
                            .lock()
                            .expect("collected mutex poisoned")
                            .rejected
                            .push(label.to_string());
                        return;
                    }

                    if kind != "snapshot" {
                        return;
                    }
                    let all_ready = {
                        let mut set = ready.lock().expect("ready mutex poisoned");
                        set.insert(label.to_string());
                        PROBE_WINDOWS
                            .iter()
                            .all(|(label, _, _)| set.contains(*label))
                    };
                    if !all_ready || triggered.swap(true, Ordering::SeqCst) {
                        return;
                    }

                    // 更新後的 tw-events 刻意墊到 8 KiB 以上：Tauri Channel 對超過
                    // MAX_JSON_DIRECT_EXECUTE_THRESHOLD（8192 bytes，`ipc/channel.rs`）的
                    // payload 改走 `plugin:__TAURI_CHANNEL__|fetch`，真實 tw_events.json 約
                    // 17 KiB 必走這條路徑——兩條投遞路徑都要驗到（custom1 的小 payload 走 eval）。
                    let padded = UPDATED_TW_EVENTS.replacen(
                        '{',
                        &format!("{{\"pad\":\"{}\",", "x".repeat(10_000)),
                        1,
                    );
                    log(&format!("TRIGGER tw-events payload bytes={}", padded.len()));
                    write_fixture(&data_dir_for_trigger, "tw_events.json", &padded);
                    write_fixture(&data_dir_for_trigger, "custom1.json", UPDATED_CUSTOM1);
                    log("TRIGGER poll_and_notify (after all windows reported snapshot)");
                    let state = handle.state::<AppState>();
                    widgets::poll_and_notify(&state);
                    log("TRIGGER poll_and_notify done");

                    // 兩個視窗這時都保證已經掛好 `settings`／`edit-mode` 的監聽器（各自的
                    // `listen` 批次都在自己的 "snapshot" 報告之前完成），driver 視窗
                    // （`w-macro`）現在才可以安全地開始跑 update_settings／set_edit_mode——
                    // 見 `test-ipc.html` 檔頭註解「`?driver=1` 的視窗」一段。
                    let result = handle.emit_to("w-macro", "drive-now", ());
                    log(&format!("EMIT drive-now -> w-macro result={result:?}"));
                });
            }

            for (label, channel, _) in PROBE_WINDOWS {
                let driver = if label == "w-macro" { "&driver=1" } else { "" };
                let builder = WebviewWindowBuilder::new(
                    app,
                    label,
                    WebviewUrl::App(format!("test-ipc.html?channel={channel}{driver}").into()),
                );
                // task 4.6：渲染視窗比照正式（`wallpaper_render::TauriSurface`）用自己的 WebView2
                // 使用者資料夾——放在本測試的暫存資料夾裡，不碰正式渲染視窗那份。
                let builder = if crate::wallpaper_render::is_renderer_label(label) {
                    builder.data_directory(renderer_data_dir.clone())
                } else {
                    crate::webview_env::with_widget_data_dir(builder)
                };
                let win = builder
                    .title(format!("test-ipc-{label}"))
                    .inner_size(360.0, 240.0)
                    .build()?;
                log(&format!(
                    "WINDOW created label={:?} url={:?}",
                    win.label(),
                    win.url()
                ));
            }

            let handle_for_timeout = handle.clone();
            let collected = collected.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_secs(timeout_secs));
                log_verdict(&collected.lock().expect("collected mutex poisoned"));
                log("TIMEOUT exit");
                handle_for_timeout.exit(0);
            });

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("self-test-ipc 建立失敗");

    // `app.run()` 在桌面平台是事件迴圈、正常情況下不會回傳（`AppHandle::exit` 內部會直接讓
    // 行程結束），寫在它之後的程式碼不可靠、實測會被跳過（沿用 probe_acl.rs 的既有寫法）。
    // fixture 目錄的清理因此要靠 `RunEvent::Exit`（迴圈真正結束前一定會收到，Tauri 官方文件
    // 記載的清理掛鉤點），不能放在 `.run()` 呼叫之後。
    app.run(move |_app, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            let _ = fs::remove_dir_all(&data_dir);
        }
    });
}

/// task 5.6 fix round 1 驗收用指令（只在 `self-test-ipc` feature 下註冊進正常宿主的指令清單，
/// 見 `main.rs::invoke_handler`）：切換 [`widgets::PauseReason::Manual`]，等同系統匣「暫停／
/// 繼續」，讓 `host/tools/verify-5.6.ps1` 以 CDP `invoke` 進入手動暫停，不必模擬點擊系統匣
/// （不注入輸入，鎖定時也可做）。回傳切換後的整體 `paused`。
#[tauri::command]
pub fn self_test_set_manual_pause(active: bool, app: tauri::AppHandle) -> bool {
    widgets::set_pause_reason(&app, widgets::PauseReason::Manual, active)
}

/// task 5.7 驗收用指令（只在 `self-test-ipc` feature 下註冊進正常宿主的指令清單，見
/// `main.rs::invoke_handler`）：呼叫與系統匣「結束」完全相同的內部路徑
/// （[`crate::tray::quit`]：先 `UnregisterApplicationRestart` 再 `app.exit(0)`），讓
/// `host/tools/rm-restart-test.ps1` 以 CDP `invoke` 驗證「從系統匣結束後 Restart Manager
/// 不會把宿主重新啟動」，不必模擬點擊系統匣選單（不注入輸入，鎖定時也可做）。呼叫後行程即
/// 結束，本函式不會回傳任何有意義的值給呼叫端（CDP 端只需確認行程消失）。
#[tauri::command]
pub fn self_test_quit(app: tauri::AppHandle) {
    crate::tray::quit(&app);
}

/// fix F2（review 5.7 medium）驗收用指令：只呼叫系統匣「結束」路徑的前半段——與
/// [`crate::tray::quit`] 同一個 `crate::desktop::unregister_restart`——**不**結束行程。
/// `host/tools/rm-restart-test.ps1` 情境 B 據此讓「已取消註冊、但仍存活滿 60 秒」的宿主真的被
/// `RmShutdown` 關掉，再驗 `RmRestart` 不會把它重新啟動（與情境 A「有註冊 → 會重啟」對照）；
/// 只靠 `self_test_quit` 時行程在 `RmShutdown` 之前就自己結束了，RM 本來就無從重啟，判準沒有
/// 鑑別力。回傳 `true`（呼叫已執行；`UnregisterApplicationRestart` 的成敗寫在記錄檔）。
#[tauri::command]
pub fn self_test_unregister_restart() -> bool {
    crate::desktop::unregister_restart();
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 內容完全正確的 `wallpaper`（fixture 的標記＋保證存在的鍵）。
    fn expected() -> WallpaperContent {
        WallpaperContent {
            envelope: true,
            marker: Some(WALLPAPER_CONFIG_MARKER.to_string()),
            data_keys: Some(
                EXPECTED_WALLPAPER_DATA_KEYS
                    .iter()
                    .map(|k| k.to_string())
                    .collect(),
            ),
            data_marker: true,
        }
    }

    /// 只有信封形狀、內容不對（沒有標記、沒有鍵）：審查 low 1 要擋下的情況。
    fn shape_only() -> WallpaperContent {
        WallpaperContent {
            envelope: true,
            marker: None,
            data_keys: Some(Vec::new()),
            data_marker: false,
        }
    }

    /// 修正輪 2（複審 L3）：鍵對、設定標記對，但 `data` 裡沒有 fixture 的資料標記（例如擷取到別份
    /// 資料）也不行。
    #[test]
    fn verdict_fails_when_wallpaper_data_lacks_fixture_data_marker() {
        let mut c = good();
        c.reports[2].content.data_marker = false;
        fails_with(&c, "wrong-content", RENDERER_LABEL);
        let mut c = good();
        c.probes[3].content.data_marker = false;
        fails_with(&c, "wrong-content", RENDERER_LABEL);
    }

    /// 兩份 tw-events fixture 的 `events` 都帶資料標記，且只在 wallpaper 會擷取的鍵裡；頁面端的同名
    /// 常數與 Rust 一致。
    #[test]
    fn data_marker_is_in_both_fixtures_extraction_and_page() {
        for raw in [INITIAL_TW_EVENTS, UPDATED_TW_EVENTS] {
            let tw: serde_json::Value = serde_json::from_str(raw).expect("JSON");
            let extracted = crate::data::extract_wallpaper_data(&tw);
            assert!(
                extracted["events"]
                    .to_string()
                    .contains(WALLPAPER_DATA_MARKER),
                "{extracted}"
            );
        }
        let html = include_str!("../ui/test-ipc.html");
        assert!(
            html.contains(&format!("const DATA_MARKER = '{WALLPAPER_DATA_MARKER}';")),
            "test-ipc.html 的 DATA_MARKER 要與 Rust 一致"
        );
    }

    /// 從原始碼找出所有 Tauri `Emitter` 發事件的呼叫，回傳事件名（修正輪 2，複審 L1）。
    ///
    /// - 認得方法呼叫 `x.emit(`／`x.emit_to(`／`x.emit_filter(`／`x.emit_str*(` 與 UFCS
    ///   `Emitter::emit(&app, ..)`、`tauri::Emitter::emit_to(&app, ..)`（`::emit*(`）；前面不是 `.`
    ///   或 `::` 的（例如 `emit_settings(`、字串 `"emit("`）不算。
    /// - 事件名取對的參數：方法呼叫第 1 個、`_to` 第 2 個（第 1 個是 target）；UFCS 再往後一個
    ///   （第 1 個是接收者）。參數以最外層逗號切開（略過括號／中括號／大括號與字串內的逗號）。
    /// - 字面字串取其內容；`SETTINGS_EVENT`、`WIDGET_FONT_EVENT`（含路徑前綴）換成它的值；其他一律回傳
    ///   `<非字面值：…>`，讓覆蓋測試失敗（不會靜默漏掉）。
    /// - 整行 `//` 註解先濾掉；行尾註解與區塊註解沒有濾——只會多掃到東西而失敗，方向安全。
    fn emitted_event_names(code: &str) -> Vec<String> {
        const VARIANTS: [&str; 6] = ["", "_to", "_filter", "_str", "_str_to", "_str_filter"];
        let code: String = code
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut out = Vec::new();
        for (idx, _) in code.match_indices("emit") {
            let before = &code[..idx];
            let ufcs = before.ends_with("::");
            if !ufcs && !before.ends_with('.') {
                continue;
            }
            let rest = &code[idx + "emit".len()..];
            let Some(open) = rest.find('(') else {
                continue;
            };
            let variant = &rest[..open];
            if !VARIANTS.contains(&variant) {
                continue;
            }
            let index = usize::from(variant.ends_with("_to")) + usize::from(ufcs);
            let args = top_level_args(&rest[open + 1..]);
            let arg = args.get(index).map_or("", |a| a.trim());
            out.push(
                if arg == "SETTINGS_EVENT" || arg.ends_with("::SETTINGS_EVENT") {
                    crate::widgets::SETTINGS_EVENT.to_string()
                } else if arg == "WIDGET_FONT_EVENT" || arg.ends_with("::WIDGET_FONT_EVENT") {
                    crate::widgets::WIDGET_FONT_EVENT.to_string()
                } else if let Some(lit) = arg.strip_prefix('"').and_then(|a| a.strip_suffix('"')) {
                    lit.to_string()
                } else {
                    let head: String = arg.chars().take(40).collect();
                    format!("<非字面值：{head}>")
                },
            );
        }
        out
    }

    /// `s` 從呼叫的左括號之後開始，以最外層逗號切出參數，到對應的右括號為止。
    fn top_level_args(s: &str) -> Vec<&str> {
        let mut args = Vec::new();
        let (mut depth, mut start, mut in_str, mut escaped) = (0i32, 0usize, false, false);
        for (i, ch) in s.char_indices() {
            if in_str {
                match (escaped, ch) {
                    (true, _) => escaped = false,
                    (false, '\\') => escaped = true,
                    (false, '"') => in_str = false,
                    _ => {}
                }
                continue;
            }
            match ch {
                '"' => in_str = true,
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' if depth == 0 => {
                    args.push(&s[start..i]);
                    return args;
                }
                ')' | ']' | '}' => depth -= 1,
                ',' if depth == 0 => {
                    args.push(&s[start..i]);
                    start = i + 1;
                }
                _ => {}
            }
        }
        args.push(&s[start..]);
        args
    }

    /// 修正輪 2（複審 L1）：掃描器要認得方法呼叫與 UFCS，`_to` 變體取正確的參數當事件名。
    #[test]
    fn emitted_event_names_handles_method_ufcs_and_target_argument() {
        let code = r#"
            app.emit("a", x);
            handle.emit_filter("b", p, |t| true);
            app.emit_to("w-macro", "c", ());
            Emitter::emit(&app, "d", x);
            tauri::Emitter::emit_to(&handle(), "w-x", "e", y(1, 2));
            Emitter::emit_filter(&app, "f", p, f);
            app.emit(SETTINGS_EVENT, p);
            app.emit(WIDGET_FONT_EVENT, p);
            app.emit(name, p);
            emit_settings(&app, &s);
            let s = ["emit(", "SETTINGS_EVENT"].concat();
        "#;
        let names = emitted_event_names(code);
        assert_eq!(
            names,
            vec![
                "a",
                "b",
                "c",
                "d",
                "e",
                "f",
                "settings",
                "widget-font",
                "<非字面值：name>"
            ]
        );
    }

    fn data(label: &str, kind: &str, channel: &str, content: WallpaperContent) -> DataReport {
        DataReport {
            label: label.into(),
            kind: kind.into(),
            channel: channel.into(),
            marker_seen: content.marker.is_some(),
            content,
        }
    }

    fn probe(label: &str, allowed: bool, content: WallpaperContent) -> WallpaperProbe {
        WallpaperProbe {
            label: label.into(),
            allowed,
            content,
        }
    }

    /// 全部正確的一組回報（task 4.6 判定的基準）。
    fn good() -> Collected {
        Collected {
            reports: vec![
                data(
                    "w-macro",
                    "data-received",
                    "tw-events",
                    WallpaperContent::none(),
                ),
                data(
                    "w-custom1",
                    "data-received",
                    "custom1",
                    WallpaperContent::none(),
                ),
                data(RENDERER_LABEL, "data-received", "wallpaper", expected()),
            ],
            rejected: vec!["w-probe".to_string()],
            probes: vec![
                probe("w-macro", false, WallpaperContent::none()),
                probe("w-custom1", false, WallpaperContent::none()),
                probe("w-probe", false, WallpaperContent::none()),
                probe(RENDERER_LABEL, true, expected()),
            ],
            content_leaks: Vec::new(),
            installed: PROBE_WINDOWS
                .iter()
                .map(|(label, _, _)| {
                    (
                        label.to_string(),
                        BARE_PROBE_EVENTS.iter().map(|e| e.to_string()).collect(),
                    )
                })
                .collect(),
        }
    }

    fn fails_with(c: &Collected, needle: &str, label: &str) {
        let failures = verdict_failures(c);
        assert!(
            failures
                .iter()
                .any(|f| f.contains(needle) && f.contains(label)),
            "應有 {needle}（{label}）：{failures:?}"
        );
    }

    #[test]
    fn verdict_passes_when_only_renderer_gets_correct_wallpaper() {
        let failures = verdict_failures(&good());
        assert!(failures.is_empty(), "{failures:?}");
    }

    #[test]
    fn verdict_fails_when_bare_listen_on_widget_receives_wallpaper() {
        let mut c = good();
        c.reports.push(data(
            "w-macro",
            "bare-data-received",
            "wallpaper",
            expected(),
        ));
        fails_with(&c, "leak", "w-macro");
    }

    #[test]
    fn verdict_fails_when_wallpaper_content_shows_up_on_any_bare_event() {
        let mut c = good();
        c.content_leaks
            .push(("w-custom1".to_string(), "settings".to_string()));
        fails_with(&c, "wallpaper-content-leak", "w-custom1");
    }

    #[test]
    fn verdict_fails_when_non_renderer_data_report_carries_wallpaper_marker() {
        let mut c = good();
        c.reports[0].marker_seen = true;
        fails_with(&c, "wallpaper-content-leak", "w-macro");
    }

    #[test]
    fn verdict_fails_when_a_window_did_not_install_every_bare_probe() {
        let mut c = good();
        c.installed.retain(|(label, _)| label != "w-probe");
        fails_with(&c, "bare-probes-missing", "w-probe");

        let mut c = good();
        c.installed[1].1.retain(|e| e != "pause");
        fails_with(&c, "bare-probes-missing", "w-custom1");
    }

    #[test]
    fn verdict_fails_when_non_renderer_can_query_wallpaper() {
        let mut c = good();
        c.probes[2] = probe("w-probe", true, expected());
        fails_with(&c, "wallpaper-query-leak", "w-probe");
    }

    #[test]
    fn verdict_fails_when_renderer_misses_wallpaper_or_query_is_denied() {
        let mut c = good();
        c.reports.retain(|r| r.label != RENDERER_LABEL);
        fails_with(&c, "missing", RENDERER_LABEL);

        let mut c = good();
        c.probes[3] = probe(RENDERER_LABEL, false, WallpaperContent::none());
        fails_with(&c, "wallpaper-query-denied", RENDERER_LABEL);
    }

    /// 審查 low 1：只有 `{ data, config }` 形狀、內容不對的 payload 不得 PASS（推送與查詢兩條路）。
    #[test]
    fn verdict_fails_on_shape_only_wallpaper_payload() {
        let mut c = good();
        c.reports[2].content = shape_only();
        fails_with(&c, "wrong-content", RENDERER_LABEL);

        let mut c = good();
        c.probes[3].content = shape_only();
        fails_with(&c, "wrong-content", RENDERER_LABEL);

        // 標記對但多帶了不該帶的鍵（例如 macro）也不行。
        let mut c = good();
        let mut keys = c.probes[3].content.data_keys.clone().expect("keys");
        keys.push("macro".to_string());
        c.probes[3].content.data_keys = Some(keys);
        fails_with(&c, "wrong-content", RENDERER_LABEL);

        // 不是信封。
        let mut c = good();
        c.reports[2].content.envelope = false;
        fails_with(&c, "wrong-content", RENDERER_LABEL);
    }

    #[test]
    fn verdict_fails_when_a_window_never_probed_wallpaper() {
        let mut c = good();
        c.probes.retain(|p| p.label != "w-custom1");
        fails_with(&c, "wallpaper-probe-missing", "w-custom1");
    }

    /// fixture 保證存在、且擷取後恰好是這些鍵（初始與更新後兩份都核對）。
    #[test]
    fn expected_keys_match_the_fixture_and_the_extraction() {
        for raw in [INITIAL_TW_EVENTS, UPDATED_TW_EVENTS] {
            let tw: serde_json::Value = serde_json::from_str(raw).expect("JSON");
            let extracted = crate::data::extract_wallpaper_data(&tw);
            let mut keys: Vec<&str> = extracted
                .as_object()
                .expect("object")
                .keys()
                .map(String::as_str)
                .collect();
            keys.sort_unstable();
            assert_eq!(keys, EXPECTED_WALLPAPER_DATA_KEYS);
        }
    }

    #[test]
    fn renderer_probe_label_is_a_real_renderer_label() {
        assert!(crate::wallpaper_render::is_renderer_label(RENDERER_LABEL));
        for (label, _, _) in PROBE_WINDOWS {
            if label != RENDERER_LABEL {
                assert!(
                    !crate::wallpaper_render::is_renderer_label(label),
                    "{label}"
                );
            }
        }
    }

    /// 裸 listen 探針必須涵蓋宿主會 emit 的**每一個**事件名：掃 `src/` 下所有正式程式碼（不含
    /// self-test／探針模組）的 `.emit*(` 與 UFCS `::emit*(` 呼叫（[`emitted_event_names`]），取事件名（字面字串或 `SETTINGS_EVENT`）；不是字面值
    /// 的一律視為未知、讓測試失敗——日後新增事件時這裡會逼著把它加進 [`BARE_PROBE_EVENTS`]。
    /// 同時核對 test-ipc.html 掛的清單與 Rust 端一致。
    #[test]
    fn bare_probe_events_cover_every_event_the_host_emits() {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            for entry in fs::read_dir(dir).expect("read_dir") {
                let path = entry.expect("entry").path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        walk(&src, &mut files);
        let mut names = std::collections::BTreeSet::new();
        for file in files {
            let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == "self_test_ipc.rs" || name.starts_with("probe_") {
                continue;
            }
            let text = fs::read_to_string(&file).expect("讀原始碼");
            let code: String = text
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n");
            for event in emitted_event_names(&code) {
                names.insert(if event.starts_with('<') {
                    format!("{event} in {name}")
                } else {
                    event
                });
            }
        }
        assert!(
            names.contains("settings") && names.contains("pause"),
            "掃描失效：{names:?}"
        );
        for event in &names {
            assert!(
                BARE_PROBE_EVENTS.contains(&event.as_str()),
                "宿主會 emit「{event}」，但裸 listen 探針沒有涵蓋；請加進 BARE_PROBE_EVENTS 與 test-ipc.html"
            );
        }

        let html = include_str!("../ui/test-ipc.html");
        let start = html
            .find("const BARE_PROBE_EVENTS = [")
            .expect("test-ipc.html 應宣告 BARE_PROBE_EVENTS");
        let list = &html[start..];
        let list = &list[..list.find(']').expect("陣列結尾")];
        let mut page: Vec<&str> = list.split('\'').skip(1).step_by(2).collect();
        page.sort_unstable();
        let mut rust = BARE_PROBE_EVENTS.to_vec();
        rust.sort_unstable();
        assert_eq!(
            page, rust,
            "test-ipc.html 與 Rust 的 BARE_PROBE_EVENTS 不一致"
        );
    }
}
