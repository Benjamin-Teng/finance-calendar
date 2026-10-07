//! 動態桌布渲染管線（dynamic-wallpaper task 4.5；design.md D2、D4；specs/desktop-wallpaper
//! 「逐螢幕依原生解析度出圖」「兩個重畫時點之間不渲染」「渲染失敗時保留舊圖」「輸出檔數量有上限」）。
//!
//! 一次渲染＝一台螢幕一張圖：
//!
//! 1. 配一個 render id（[`RenderBroker::begin`]），組頁面網址（[`page_url`]：`w`／`h`＝該螢幕實體像素、
//!    `t`＝含 `Z` 的絕對時刻、`tz`、`rid`）；
//! 2. 建立**隱藏**渲染視窗（label `wallpaper-renderer-<render id>`，[`renderer_label`]；**每次渲染一個
//!    label**，前綴 [`RENDERER_LABEL_PREFIX`]——4.6 的 `wallpaper` 通道等以 label 判斷渲染視窗的地方一律
//!    用 [`is_renderer_label`]，不比對固定字串；舊視窗等不到 `Destroyed` 也不會卡住下一次，
//!    [`TauriSurface`]）：`visible(false)`、
//!    `focused(false)`、`focusable(false)`、`skip_taskbar(true)`、無邊框，建好後套
//!    `desktop::apply_widget_ex_style`（`WS_EX_TOOLWINDOW`／`WS_EX_NOACTIVATE`，同探針 1.4 驗證過的
//!    做法）。使用者資料夾是**獨立的 WebView2 環境**（[`crate::webview_env`]），所以有自己的 browser
//!    行程，視窗關閉後該行程結束；
//! 3. 頁面畫完自己回報（`wallpaper_render_done`：本體＝PNG 位元組、header `x-wallpaper-meta`；失敗
//!    `wallpaper_render_failed`），meta 帶回 `rid`。**id 不符一律丟棄並記錄**（[`Delivery::Stale`]）——
//!    逾時後才晚到的 PNG 不會被算給下一次；
//! 4. 收到回報、或 [`RENDER_TIMEOUT`]（30 秒，**從開始建立視窗起算**，建立視窗卡住也算在內）到期，
//!    **一律關窗**（每次開關，design.md D2）。建立視窗在期限內沒返回時本次失敗；建立執行緒之後
//!    返回時自己把視窗關掉，在它返回之前的渲染請求直接失敗、不再疊一條卡住的建立；
//! 5. 核對 PNG 尺寸等於要求，原子寫入 `<輸出資料夾>\<螢幕鍵>-a.png`／`-b.png` 中「不是螢幕上正顯示
//!    的那一格」（[`write_output`]、[`choose_slot`]），寫完讀回尺寸再核對一次，清掉這台螢幕的殘檔。
//!
//! ## 給 4.7 的 API
//!
//! - 宿主啟動時 [`install`]（`setup` 內、主執行緒）：登記 [`RenderBroker`]（兩個指令用）與
//!   [`WallpaperRenderer`]（Tauri managed state）。
//! - 重畫時（**工作執行緒**，不得在主執行緒或同步指令處理中呼叫——會等頁面回報，在主執行緒會
//!   死鎖；主執行緒呼叫一律回 [`RenderFailureReason::WrongThread`]）：
//!   `app.state::<WallpaperRenderer>().render(&RenderRequest { .. }, &OutputTarget { dir, displayed })`
//!   - `dir`＝`wallpaper_state::StatePaths::output_dir`（與 4.4 的「是否為宿主輸出」判斷同一個資料夾）；
//!   - `displayed`＝這台螢幕目前顯示的宿主輸出圖（協調迴圈以讀回確認的路徑為準，讀不到才用 4.4 狀態檔
//!     的 `last_set`——那是「準備設定」的路徑，設定失敗時螢幕仍是另一格；task 6.4。不知道就 `None`，
//!     改以修改時間挑較舊的一格）；
//!   - 成功：[`RenderSuccess::path`] 交給 `WallpaperTakeover::set_monitor_wallpaper`；
//!   - 失敗：`SchedulerState::record_failed(plan, target, failure.kind(), now)`——[`RenderFailure::kind`]
//!     恆為 [`FailureKind::Render`]，退避沿用 `wallpaper::retry_delay`，本模組不另訂曲線。
//! - 同一時間只有一個渲染（內部互斥）；多台螢幕由 4.7 依序呼叫。
//! - 本模組**不呼叫** `SetWallpaper`、不接排程與事件（4.7）、不提供資料通道（4.6）。
//!
//! ## 前一個渲染 browser 行程（bug-leaked-renderer）
//!
//! 每次渲染都用同一個渲染專用資料夾，關窗後它的 browser 行程才開始結束（重現量測：返回後約 0.18 秒
//! 結束）。WebView2 的規則是「同一個使用者資料夾同時只有一組 browser 行程」：在它還沒決定結束前
//! （關窗後約 100 ms 內）建立的新環境會**併進它**、拿到同一個 PID；之後到結束前則與它的結束流程重疊。
//! 6.2 長跑另有一次它卡在結束流程（只剩 1 條執行緒）、宿主結束後仍存活數小時。所以：
//!
//! - 建立視窗後以 `ICoreWebView2::BrowserProcessId` 讀 PID、在 UI 執行緒上立刻開行程把手
//!   （`desktop::webview_browser_process`），之後不怕 PID 重用；
//! - **每次建立視窗之前**（不算進 30 秒期限）[`Renderer::settle_previous_browser`]：等前一個結束，最多
//!   [`BROWSER_EXIT_WAIT`]；仍在就核對身分（父行程＝宿主、沒有 `--type=`、`--user-data-dir`＝渲染資料夾
//!   的 `EBWebView`，[`verify_renderer_browser`]）後結束它並記 warn，核對不符只記 warn、不動它；
//! - 宿主從系統匣結束時同樣處置一次（[`Renderer::shutdown`]，在結束執行緒）；其他經 `app.exit` 的結束在
//!   `RunEvent::Exit` 再做一次**不等待**的處置（[`Renderer::settle_on_exit`]：仍存活就核對、結束，不等它
//!   退出）；工作階段結束中（[`note_session_ending`]）則交給系統。
//!
//! ## 失敗分類（全部是 [`FailureKind::Render`]，輸出資料夾裡的舊圖不動）
//!
//! 開窗失敗、30 秒逾時（含 id 不符被丟棄而沒等到正確回報）、頁面回報失敗、本體不是 PNG 位元組、
//! PNG 尺寸不符（含頁面因參數不合法改畫預設尺寸）、寫檔或讀回核對失敗。頁面回報成功但 meta 帶
//! 警告仍算成功，警告記在 [`RenderSuccess::warnings`] 並寫記錄檔。

// 排程呼叫端（[`WallpaperRenderer::render`]）在 task 4.7；目前只有 self-test 與單元測試使用。
#![allow(dead_code)]

use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant, SystemTime};

use serde_json::Value;
use tauri::ipc::{InvokeBody, Request};
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::desktop;
use crate::desktop::wallpaper::pathcmp::path_eq;
use crate::settings::WallpaperTheme;
use crate::wallpaper::{CivilDate, FailureKind, UnixSeconds};
use crate::webview_env;

/// 渲染視窗 label 的前綴（design.md D2 的 `wallpaper-renderer`，後接 render id；[`renderer_label`]）。
/// 4.6 的 `wallpaper` 通道只推給 [`is_renderer_label`] 為真的 webview。
pub const RENDERER_LABEL_PREFIX: &str = "wallpaper-renderer-";
/// 單次渲染逾時（spec「渲染失敗時保留舊圖」：超過 30 秒未完成）：從開始建立視窗到收到回報。
pub const RENDER_TIMEOUT: Duration = Duration::from_secs(30);
/// 關窗後等 `Destroyed` 的上限（等不到只記錄；下一次用新的 label，不受影響）。
pub const CLOSE_TIMEOUT: Duration = Duration::from_secs(10);
/// 下一次渲染前（與宿主結束時）等前一個渲染 browser 行程結束的上限。正常關窗後約 0.2 秒就結束
/// （bug-leaked-renderer 重現量測），等滿這麼久仍在＝卡在結束流程，由宿主結束它。
pub const BROWSER_EXIT_WAIT: Duration = Duration::from_secs(5);
/// 結束卡住的渲染 browser 行程後等它退出的上限（等不到只記錄）。
pub const BROWSER_KILL_WAIT: Duration = Duration::from_secs(2);
/// 一定不會回報的頁面：self-test 用它在真實宿主上走一次逾時路徑（[`Renderer::render_custom_page`]）。
/// 檔案確實存在於 `host/ui/wallpapers/`（不存在時 Tauri 會退回載入根目錄的 `index.html`，語意隨那頁
/// 改變），內容只有一段停在永不完成的 Promise 上的模組腳本、不載入共用模組也不碰 IPC；正式頁面不會
/// 連到它。單元測試 `never_reports_page_exists_and_cannot_report` 守住這些條件。
pub const NEVER_REPORTS_PAGE: &str = "wallpapers/__self-test-never-reports__.html";

/// 第 `id` 次渲染的視窗 label：`wallpaper-renderer-<id>`。
pub fn renderer_label(id: RenderId) -> String {
    format!("{RENDERER_LABEL_PREFIX}{id}")
}

/// 是否為渲染視窗的 label（前綴＋十進位 render id）。
pub fn is_renderer_label(label: &str) -> bool {
    label
        .strip_prefix(RENDERER_LABEL_PREFIX)
        .is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
}

/// 除了 `current` 之外仍存在的渲染視窗（舊的等不到 `Destroyed`、或建立逾時後才建好的）。
pub fn lingering_renderer_labels<'a>(
    labels: impl Iterator<Item = &'a str>,
    current: &str,
) -> Vec<String> {
    let mut out: Vec<String> = labels
        .filter(|l| *l != current && is_renderer_label(l))
        .map(str::to_owned)
        .collect();
    out.sort();
    out
}
/// 頁面回報 meta 的 header 名（`wallpaper.mjs`「宿主接法」）。
pub const META_HEADER: &str = "x-wallpaper-meta";
/// 頁面接受的尺寸範圍（與 `host/ui/wallpapers/lib/core.mjs` 的 `MIN_DIM`／`MAX_DIM` 一致；超出時
/// 頁面會改畫預設尺寸，宿主事先拒絕比事後核對失敗清楚）。
pub const MIN_DIMENSION: u32 = 16;
/// 見 [`MIN_DIMENSION`]。
pub const MAX_DIMENSION: u32 = 16_384;

/// 一次渲染的 id（行程內遞增、不重用）。
pub type RenderId = u64;

// ── 請求與網址 ────────────────────────────────────────────────────────────────────────────

/// 渲染一台螢幕的請求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderRequest {
    /// 螢幕鍵（`wallpaper_state::monitor_key`，`m-<16 位小寫十六進位>`）；輸出檔名的前綴。
    pub monitor_key: String,
    /// 該螢幕實體像素寬（design.md D4；縮放比例不得影響）。
    pub width: u32,
    /// 該螢幕實體像素高。
    pub height: u32,
    /// 主題（`None`＝不接管，不能渲染）。
    pub theme: WallpaperTheme,
    /// 圖要呈現的時刻（`RedrawPlan::as_of`）。
    pub as_of: UnixSeconds,
    /// 顯示時區（IANA，例如 `Asia/Taipei`）；空字串＝不帶 `tz` 參數，頁面使用系統時區（正式排程
    /// 用這個，task 4.7a）。
    pub tz: String,
    /// 頁面的 `fixture` 參數（相對頁面的資料檔網址）。**只供 self-test／量測**；正式渲染為
    /// `None`，資料走 `wallpaper` 通道（4.6）。
    pub fixture: Option<String>,
}

/// UNIX 秒 → `YYYY-MM-DDTHH:MM:SSZ`（頁面的 `t` 參數；絕對時刻、帶 `Z`）。
pub fn iso_utc(t: UnixSeconds) -> String {
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    let d = CivilDate::from_days(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        d.year,
        d.month,
        d.day,
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    )
}

/// query 值編碼：保留 RFC 3986 的未保留字元，其餘以 UTF-8 的 `%XX` 表示（頁面以
/// `URLSearchParams` 解回）。
fn encode_query_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// 頁面網址（相對前端根目錄，`WebviewUrl::App` 用）：
/// `wallpapers/<主題>.html?w=&h=&t=&tz=&rid=[&fixture=]`。
pub fn page_url(req: &RenderRequest, id: RenderId) -> Result<String, String> {
    if req.theme == WallpaperTheme::None {
        return Err("主題為「不接管」，沒有可渲染的頁面".into());
    }
    page_url_for(&format!("wallpapers/{}.html", req.theme.as_str()), req, id)
}

/// 指定頁面路徑、query 與 [`page_url`] 相同（self-test 的 [`NEVER_REPORTS_PAGE`] 用）。
pub fn page_url_for(page: &str, req: &RenderRequest, id: RenderId) -> Result<String, String> {
    for (name, v) in [("寬", req.width), ("高", req.height)] {
        if !(MIN_DIMENSION..=MAX_DIMENSION).contains(&v) {
            return Err(format!("{name} {v} 超出 {MIN_DIMENSION}..={MAX_DIMENSION}"));
        }
    }
    // task 4.7a：`tz` 空字串＝不帶 `tz` 參數，頁面改用系統時區（`core.mjs` `parseQuery`）。渲染視窗
    // 每次都是新的 browser 行程，跟隨當下的 Windows 時區；宿主不必把 Windows 時區換成 IANA 名稱。
    let tz = if req.tz.is_empty() {
        String::new()
    } else {
        format!("&tz={}", encode_query_value(&req.tz))
    };
    let mut url = format!(
        "{page}?w={}&h={}&t={}{tz}&rid={id}",
        req.width,
        req.height,
        encode_query_value(&iso_utc(req.as_of)),
    );
    if let Some(f) = &req.fixture {
        url.push_str("&fixture=");
        url.push_str(&encode_query_value(f));
    }
    Ok(url)
}

/// 螢幕鍵是否為 `wallpaper_state::monitor_key` 的格式（`m-` 加 16 位小寫十六進位）。輸出檔名與
/// 殘檔清除都以 `<鍵>-` 為前綴，固定格式保證不會跨到別台螢幕的檔，也擋掉路徑字元。
pub fn is_valid_monitor_key(key: &str) -> bool {
    key.len() == 18
        && key.starts_with("m-")
        && key[2..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

// ── 本體與 meta（由探針 1.4 提升；探針本身維持在 probe-render feature 內不動）──────────────

/// `wallpaper_render_done` 本體的形狀。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyKind {
    /// 原始位元組（自訂協定 IPC，正常路徑）。
    Raw,
    /// JSON 數字陣列（IPC 退回 postMessage 時；探針 1.4 實測慢約 12 倍，只當相容退路）。
    JsonArray,
}

impl BodyKind {
    pub fn as_str(self) -> &'static str {
        match self {
            BodyKind::Raw => "raw",
            BodyKind::JsonArray => "json-array",
        }
    }
}

/// 把 `wallpaper_render_done` 的本體還原成 PNG 位元組；原始位元組與 JSON 數字陣列兩種都收。
pub fn decode_png_body(body: &InvokeBody) -> Result<(BodyKind, Vec<u8>), String> {
    match body {
        InvokeBody::Raw(bytes) => Ok((BodyKind::Raw, bytes.clone())),
        InvokeBody::Json(Value::Array(items)) => {
            let mut out = Vec::with_capacity(items.len());
            for (i, v) in items.iter().enumerate() {
                match v.as_u64().and_then(|n| u8::try_from(n).ok()) {
                    Some(n) => out.push(n),
                    None => return Err(format!("JSON 陣列第 {i} 個元素不是 0..=255 的整數：{v}")),
                }
            }
            Ok((BodyKind::JsonArray, out))
        }
        InvokeBody::Json(_) => Err("本體不是位元組也不是數字陣列".into()),
    }
}

/// 從 PNG 檔頭（簽章＋IHDR）讀寬高；不是 PNG 回傳 `None`。
pub fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if bytes.len() < 24 || bytes[..8] != SIG || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((w, h))
}

/// `encodeURIComponent` 的反向（`%XX` → 位元組 → UTF-8）。
pub fn percent_decode(s: &str) -> Result<String, String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes
                .get(i + 1..i + 3)
                .and_then(|h| std::str::from_utf8(h).ok())
                .and_then(|h| u8::from_str_radix(h, 16).ok())
                .ok_or_else(|| format!("位置 {i} 的 % 後不是兩位十六進位"))?;
            out.push(hex);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|e| format!("解碼後不是 UTF-8：{e}"))
}

/// 頁面回報的 meta（`wallpaper.mjs` 的 `reportMeta`；只取宿主用得到的欄位）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageMeta {
    /// render id（頁面從自己網址的 `rid` 取得、原樣帶回）。
    pub rid: Option<String>,
    pub w: Option<u64>,
    pub h: Option<u64>,
    pub warnings: Vec<String>,
    pub data_status: Option<String>,
}

/// meta JSON → [`PageMeta`]。`rid` 收字串或整數。
pub fn parse_meta_value(v: &Value) -> PageMeta {
    let rid = match v.get("rid") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    };
    let warnings = v
        .get("warnings")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|w| w.as_str().map_or_else(|| w.to_string(), str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    PageMeta {
        rid,
        w: v.get("w").and_then(Value::as_u64),
        h: v.get("h").and_then(Value::as_u64),
        warnings,
        data_status: v
            .get("dataStatus")
            .and_then(Value::as_str)
            .map(str::to_owned),
    }
}

/// `x-wallpaper-meta` header（`encodeURIComponent(JSON.stringify(meta))`）→ [`PageMeta`]。
pub fn parse_meta_header(s: &str) -> Result<PageMeta, String> {
    let text = percent_decode(s)?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("meta 不是 JSON：{e}"))?;
    Ok(parse_meta_value(&v))
}

// ── broker：render id 配對 ────────────────────────────────────────────────────────────────

/// 頁面的一次回報。
#[derive(Debug)]
pub enum PageReport {
    /// 畫完（本體解碼結果＋meta）。
    Done {
        png: Result<(BodyKind, Vec<u8>), String>,
        meta: PageMeta,
    },
    /// 頁面回報失敗。
    Failed { error: String, meta: PageMeta },
}

/// [`RenderBroker`] 收到回報後的處置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivery {
    /// id 相符，交給等待中的渲染。
    Accepted,
    /// id 不符、沒帶 id、或沒有等待中的渲染（逾時後晚到、同一個 id 第二次回報）：丟棄並記錄。
    Stale {
        got: Option<String>,
        waiting: Option<RenderId>,
    },
    /// 不是渲染視窗送來的：拒收。
    NotRenderer,
}

struct BrokerInner {
    last_id: RenderId,
    waiting: Option<(RenderId, mpsc::Sender<PageReport>)>,
}

/// 頁面回報與等待中的渲染之間的配對（兩個指令與 [`Renderer`] 共用）。同時最多一個等待中的渲染。
pub struct RenderBroker {
    inner: Mutex<BrokerInner>,
}

impl Default for RenderBroker {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderBroker {
    pub fn new() -> Self {
        RenderBroker {
            inner: Mutex::new(BrokerInner {
                last_id: 0,
                waiting: None,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BrokerInner> {
        // 鎖內只有配對與 channel 送出，不會 panic；萬一中毒仍取回內容（配對狀態本身一致）。
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 開始一次渲染：配新的 id，之後只有帶這個 id 的回報會送進 [`Ticket::wait`]。前一張
    /// [`Ticket`] 若還在，它的等待被取代（[`Renderer`] 以互斥保證不會發生）。
    pub fn begin(&self) -> Ticket<'_> {
        let (tx, rx) = mpsc::channel();
        let mut inner = self.lock();
        inner.last_id += 1;
        let id = inner.last_id;
        inner.waiting = Some((id, tx));
        Ticket {
            broker: self,
            id,
            rx,
        }
    }

    /// 配對：label 必須是渲染視窗、`rid` 必須等於等待中的 id；相符才建立回報（解碼大本體）並
    /// 送出，且同一個 id 只收一次。
    fn deliver_with(
        &self,
        label: &str,
        rid: Option<&str>,
        make: impl FnOnce() -> PageReport,
    ) -> Delivery {
        if !is_renderer_label(label) {
            log::warn!("桌布渲染回報來自非渲染視窗（{label}），拒收");
            return Delivery::NotRenderer;
        }
        let mut inner = self.lock();
        let waiting = inner.waiting.as_ref().map(|(id, _)| *id);
        // rid 與送出的視窗都要是等待中的那一次（舊視窗晚到的回報，即使帶著相同的 rid 也不收）。
        let matches = matches!((waiting, rid), (Some(id), Some(r))
            if r == id.to_string() && label == renderer_label(id));
        if !matches {
            log::warn!(
                "桌布渲染回報的 render id 或視窗不符，丟棄（收到 {:?}／{label}，等待中 {:?}）",
                rid,
                waiting
            );
            return Delivery::Stale {
                got: rid.map(str::to_owned),
                waiting,
            };
        }
        let Some((_, tx)) = inner.waiting.take() else {
            unreachable!("matches 已確認有等待中的渲染");
        };
        drop(inner);
        // 接收端（Ticket）已放棄時送不出去：等同晚到，記錄即可。
        if tx.send(make()).is_err() {
            log::warn!("桌布渲染回報送達時等待已結束，丟棄（rid {rid:?}）");
        }
        Delivery::Accepted
    }

    /// `wallpaper_render_done`：本體＝PNG（原始位元組或 JSON 數字陣列），meta 在 header。
    pub fn deliver_done(
        &self,
        label: &str,
        body: &InvokeBody,
        meta_header: Option<&str>,
    ) -> Delivery {
        let meta = match meta_header.map(parse_meta_header) {
            Some(Ok(m)) => m,
            Some(Err(e)) => {
                log::warn!("桌布渲染回報的 meta 無法解析：{e}");
                PageMeta::default()
            }
            None => PageMeta::default(),
        };
        let rid = meta.rid.clone();
        self.deliver_with(label, rid.as_deref(), || PageReport::Done {
            png: decode_png_body(body),
            meta,
        })
    }

    /// `wallpaper_render_failed`：錯誤原因＋meta（JSON 物件）。
    pub fn deliver_failed(&self, label: &str, error: String, meta: &Value) -> Delivery {
        let meta = parse_meta_value(meta);
        let rid = meta.rid.clone();
        self.deliver_with(label, rid.as_deref(), || PageReport::Failed { error, meta })
    }
}

/// 一次渲染的等待憑證；丟棄時撤銷等待，之後才到的同 id 回報一律 [`Delivery::Stale`]。
pub struct Ticket<'a> {
    broker: &'a RenderBroker,
    id: RenderId,
    rx: mpsc::Receiver<PageReport>,
}

impl Ticket<'_> {
    pub fn id(&self) -> RenderId {
        self.id
    }

    /// 等回報，最多 `timeout`。
    pub fn wait(&self, timeout: Duration) -> Option<PageReport> {
        self.rx.recv_timeout(timeout).ok()
    }
}

impl Drop for Ticket<'_> {
    fn drop(&mut self) {
        let mut inner = self.broker.lock();
        if inner.waiting.as_ref().map(|(id, _)| *id) == Some(self.id) {
            inner.waiting = None;
        }
    }
}

// ── 視窗抽象（正式：Tauri 隱藏視窗；測試：假渲染器）─────────────────────────────────────

/// 開一個載入 `url` 的渲染視窗。
/// 由 [`Renderer`] 在專屬的建立執行緒呼叫（才能以期限約束它），`label`＝[`renderer_label`]。
pub trait RenderSurface: Send + Sync {
    fn open(&self, label: &str, url: &str) -> Result<Box<dyn OpenSurface>, String>;
}

/// 開著的渲染視窗（可能由建立執行緒在期限之後自行關閉，所以要能跨執行緒）。
pub trait OpenSurface: Send {
    /// 關閉並等它真的消失（最多 `timeout`）。
    fn close(self: Box<Self>, timeout: Duration) -> Result<(), String>;
    /// 這個視窗那組 WebView2 的 browser 行程（建立時讀到；讀不到為 `None`）。
    fn browser(&self) -> Option<Arc<dyn BrowserProcess>> {
        None
    }
}

/// 渲染視窗那組 WebView2 的 browser 行程（正式：[`TauriBrowser`]；測試：假的）。
pub trait BrowserProcess: Send + Sync {
    fn pid(&self) -> u32;
    /// 等它結束，最多 `timeout`；已結束回 `true`。
    fn wait_exit(&self, timeout: Duration) -> bool;
    /// 宿主可以結束它嗎：必須是本行程的子行程、是 browser 行程（命令列沒有 `--type=`）、
    /// `--user-data-dir` 是渲染視窗的資料夾（[`verify_renderer_browser`]）。不行回原因。
    fn check_owned(&self) -> Result<(), String>;
    /// 結束它。
    fn terminate(&self) -> Result<(), String>;
}

/// 從 Chromium 命令列取出 `--user-data-dir=` 的值（可帶雙引號；沒有回 `None`）。
pub fn user_data_dir_arg(command_line: &str) -> Option<String> {
    const KEY: &str = "--user-data-dir=";
    let start = command_line.find(KEY)? + KEY.len();
    let rest = &command_line[start..];
    let value = match rest.strip_prefix('"') {
        Some(quoted) => &quoted[..quoted.find('"')?],
        None => rest.split_whitespace().next().unwrap_or(""),
    };
    (!value.is_empty()).then(|| value.to_owned())
}

/// 殘留的渲染 browser 行程可以由宿主結束的條件（純函式，[`BrowserProcess::check_owned`] 用）：
/// 父行程是 `host_pid`、命令列沒有 `--type=`（是 browser 行程，不是 renderer／GPU／crashpad 等
/// 子行程）、`--user-data-dir` 等於 `<renderer_dir>\EBWebView`（WebView2 在呼叫端指定的資料夾下
/// 建 `EBWebView`；6.2 殘留行程的命令列即為此形）。
pub fn verify_renderer_browser(
    parent_pid: Option<u32>,
    command_line: &str,
    host_pid: u32,
    renderer_dir: &Path,
) -> Result<(), String> {
    if parent_pid != Some(host_pid) {
        return Err(format!("父行程 {parent_pid:?} 不是宿主（PID {host_pid}）"));
    }
    if command_line.contains("--type=") {
        return Err("命令列帶 --type=，不是 browser 行程".to_owned());
    }
    let expected = renderer_dir.join("EBWebView");
    match user_data_dir_arg(command_line) {
        Some(dir) if path_eq(&dir, &expected.to_string_lossy()) => Ok(()),
        Some(dir) => Err(format!(
            "--user-data-dir {dir} 不是渲染視窗的資料夾 {}",
            expected.display()
        )),
        None => Err("命令列沒有 --user-data-dir".to_owned()),
    }
}

/// 下一次渲染前（與宿主結束時）處置前一個渲染 browser 行程的結果（[`Renderer::settle_previous_browser`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserSettle {
    /// 沒有記錄中的前一個 browser 行程（第一次渲染、讀不到 PID、已處置過）。
    None,
    /// 已結束（`waited`＝這次等了多久；呼叫前就結束約為 0）。
    Exited { pid: u32, waited: Duration },
    /// 等滿上限仍未結束、核對符合，已結束它；`gone`＝結束後在上限內確實退出（有等待才回這個）。
    Terminated { pid: u32, gone: bool },
    /// 核對符合、已要求結束，但**沒有等待確認**（事件迴圈結束時的不等待處置，
    /// [`Renderer::settle_on_exit`]）。`TerminateProcess` 是非同步的，結束後立刻查一定還在（修正輪 2
    /// 審查 N1 實測），所以是否已退出是「未知」，不得記成「結束後仍未退出」。另開變體而不把 `gone` 改成
    /// 三態：`Terminated` 的 `gone` 在有等待的路徑永遠是確定值，三態只會把「未知」擴散到不可能出現它的地方。
    TerminateRequested { pid: u32 },
    /// 等滿上限仍未結束，但核對不符或結束失敗，沒有結束它（記 warn）。
    Left { pid: u32 },
}

/// 渲染的時間參數（測試用短值）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderConfig {
    /// 從開窗到收到回報的上限。
    pub timeout: Duration,
    /// 關窗後等它消失的上限。
    pub close_timeout: Duration,
    /// 下一次渲染前等前一個渲染 browser 行程結束的上限（[`BROWSER_EXIT_WAIT`]）。
    pub browser_exit_wait: Duration,
    /// 結束殘留的 browser 行程後等它退出的上限（[`BROWSER_KILL_WAIT`]）。
    pub browser_kill_wait: Duration,
}

impl Default for RenderConfig {
    fn default() -> Self {
        RenderConfig {
            timeout: RENDER_TIMEOUT,
            close_timeout: CLOSE_TIMEOUT,
            browser_exit_wait: BROWSER_EXIT_WAIT,
            browser_kill_wait: BROWSER_KILL_WAIT,
        }
    }
}

// ── 輸出檔 ────────────────────────────────────────────────────────────────────────────────

/// 每台螢幕的兩個輸出檔之一（design.md D4：同路徑重設可能不觸發重新載入，兩檔交替）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    A,
    B,
}

impl Slot {
    fn other(self) -> Slot {
        match self {
            Slot::A => Slot::B,
            Slot::B => Slot::A,
        }
    }

    fn suffix(self) -> &'static str {
        match self {
            Slot::A => "a",
            Slot::B => "b",
        }
    }
}

/// `<資料夾>\<螢幕鍵>-a.png`／`-b.png`。
pub fn output_path(dir: &Path, key: &str, slot: Slot) -> PathBuf {
    dir.join(format!("{key}-{}.png", slot.suffix()))
}

/// 這次寫哪一格（純函式）：螢幕上正顯示的那一格一律不寫；不知道顯示哪一格時，先補不存在的
/// （a 優先），兩格都在就寫修改時間較舊的（同時間寫 a）。
pub fn choose_slot(a: Option<SystemTime>, b: Option<SystemTime>, displayed: Option<Slot>) -> Slot {
    if let Some(shown) = displayed {
        return shown.other();
    }
    match (a, b) {
        (None, _) => Slot::A,
        (Some(_), None) => Slot::B,
        (Some(ta), Some(tb)) => {
            if tb < ta {
                Slot::B
            } else {
                Slot::A
            }
        }
    }
}

/// 一次寫檔的結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrittenOutput {
    pub path: PathBuf,
    pub slot: Slot,
    /// 被清掉的這台螢幕的殘檔（暫存檔、a／b 以外的同前綴檔）。
    pub removed: Vec<PathBuf>,
}

fn modified(p: &Path) -> Option<SystemTime> {
    fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// 寫入一台螢幕的輸出圖（見模組文件第 5 點）：先核對 `png` 的尺寸、選格、寫到
/// `<檔名>.tmp`＋`sync_all` 後改名換上（原子取代；不會留下寫一半的輸出圖），讀回檔頭再核對尺寸，
/// 最後清掉這台螢幕 `<鍵>-` 開頭、不是 a／b 的檔。別台螢幕的檔、備份子資料夾一律不碰。
pub fn write_output(
    dir: &Path,
    key: &str,
    png: &[u8],
    expect: (u32, u32),
    displayed: Option<&Path>,
) -> Result<WrittenOutput, String> {
    if !is_valid_monitor_key(key) {
        return Err(format!("螢幕鍵格式不符：{key:?}"));
    }
    match png_dimensions(png) {
        Some(d) if d == expect => {}
        other => return Err(format!("PNG 尺寸 {other:?} 不等於要求的 {expect:?}")),
    }
    fs::create_dir_all(dir).map_err(|e| format!("建立輸出資料夾 {} 失敗：{e}", dir.display()))?;

    let a = output_path(dir, key, Slot::A);
    let b = output_path(dir, key, Slot::B);
    let displayed_slot = displayed.and_then(|d| {
        let d = d.to_string_lossy();
        if path_eq(&d, &a.to_string_lossy()) {
            Some(Slot::A)
        } else if path_eq(&d, &b.to_string_lossy()) {
            Some(Slot::B)
        } else {
            None
        }
    });
    let slot = choose_slot(modified(&a), modified(&b), displayed_slot);
    let path = output_path(dir, key, slot);
    let tmp = dir.join(format!("{key}-{}.png.tmp", slot.suffix()));

    let written = (|| -> io::Result<()> {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(png)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, &path)
    })();
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(format!("寫入 {} 失敗：{e}", path.display()));
    }

    let mut head = [0u8; 24];
    let readback = fs::File::open(&path)
        .and_then(|mut f| f.read_exact(&mut head))
        .map_err(|e| format!("讀回 {} 失敗：{e}", path.display()))
        .map(|()| png_dimensions(&head))?;
    if readback != Some(expect) {
        return Err(format!(
            "讀回 {} 的尺寸 {readback:?} 不等於要求的 {expect:?}",
            path.display()
        ));
    }

    let keep = [
        a.file_name().map(|n| n.to_os_string()),
        b.file_name().map(|n| n.to_os_string()),
    ];
    let prefix = format!("{key}-");
    let mut removed = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let is_file = entry.file_type().map(|t| t.is_file()).unwrap_or(false);
            if !is_file
                || keep.iter().any(|k| k.as_deref() == Some(name.as_os_str()))
                || !name.to_string_lossy().starts_with(&prefix)
            {
                continue;
            }
            let p = entry.path();
            match fs::remove_file(&p) {
                Ok(()) => removed.push(p),
                Err(e) => log::warn!("清除桌布輸出殘檔 {} 失敗：{e}", p.display()),
            }
        }
    }
    Ok(WrittenOutput {
        path,
        slot,
        removed,
    })
}

// ── 渲染 ──────────────────────────────────────────────────────────────────────────────────

/// 寫輸出檔的位置。
#[derive(Debug, Clone, Copy)]
pub struct OutputTarget<'a> {
    /// 輸出資料夾（`wallpaper_state::StatePaths::output_dir`）。
    pub dir: &'a Path,
    /// 這台螢幕目前顯示的宿主輸出圖（不寫那一格）；不知道時 `None`。
    pub displayed: Option<&'a Path>,
}

/// 各段耗時。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RenderTimings {
    /// 建立視窗（含 WebView2 環境；獨立環境下每次都是冷啟動 browser 行程）。
    pub open: Duration,
    /// 開窗完成到收到回報（載入、字型、資料、繪製、匯出 PNG、IPC）。
    pub page: Duration,
    /// 關窗到 `Destroyed`。
    pub close: Duration,
    /// 寫檔、讀回核對、清殘檔。
    pub write: Duration,
}

impl RenderTimings {
    /// 從開始建立視窗到收到 PNG（＝冷啟動耗時）。
    pub fn to_png(self) -> Duration {
        self.open + self.page
    }
}

/// 渲染成功。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderSuccess {
    pub render_id: RenderId,
    /// 本次寫入的輸出圖（4.7 拿它設桌布）。
    pub path: PathBuf,
    pub slot: Slot,
    pub width: u32,
    pub height: u32,
    /// 頁面的 meta.warnings（畫完帶警告仍算成功）。
    pub warnings: Vec<String>,
    pub body: BodyKind,
    pub timings: RenderTimings,
    /// 這次渲染視窗那組的 browser 行程 PID（讀不到為 `None`）。
    pub browser_pid: Option<u32>,
}

/// 渲染失敗的原因（全部歸 [`FailureKind::Render`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderFailureReason {
    /// 在禁止的執行緒（主執行緒）呼叫——會死鎖，直接拒絕。
    WrongThread,
    /// 請求不合法（主題為不接管、尺寸超出範圍、螢幕鍵格式不符）。
    InvalidRequest(String),
    /// 建立渲染視窗失敗。
    OpenFailed(String),
    /// 逾時沒有收到 id 相符的回報。
    Timeout(Duration),
    /// 頁面回報失敗。
    PageFailed(String),
    /// 本體不是 PNG 位元組。
    BadBody(String),
    /// PNG 尺寸不等於要求（`actual` 為 `None`＝不是 PNG）。
    SizeMismatch {
        expected: (u32, u32),
        actual: Option<(u32, u32)>,
    },
    /// 寫檔或讀回核對失敗。
    WriteFailed(String),
}

impl fmt::Display for RenderFailureReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RenderFailureReason::WrongThread => write!(f, "不可在主執行緒渲染"),
            RenderFailureReason::InvalidRequest(m) => write!(f, "請求不合法：{m}"),
            RenderFailureReason::OpenFailed(m) => write!(f, "建立渲染視窗失敗：{m}"),
            RenderFailureReason::Timeout(d) => write!(f, "{} 秒內沒有收到回報", d.as_secs_f64()),
            RenderFailureReason::PageFailed(m) => write!(f, "頁面回報失敗：{m}"),
            RenderFailureReason::BadBody(m) => write!(f, "本體無法解碼：{m}"),
            RenderFailureReason::SizeMismatch { expected, actual } => {
                write!(f, "PNG 尺寸 {actual:?} 不等於要求的 {expected:?}")
            }
            RenderFailureReason::WriteFailed(m) => write!(f, "寫入輸出圖失敗：{m}"),
        }
    }
}

/// 渲染失敗。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderFailure {
    /// 這次的 render id（請求不合法、執行緒不對時還沒配 id）。
    pub render_id: Option<RenderId>,
    pub reason: RenderFailureReason,
    /// 視窗是否已關閉並等到 `Destroyed`：`None`＝沒有開成視窗（請求不合法、建立失敗），或建立在期限
    /// 內沒返回（建立執行緒事後自行關閉）；`Some(false)`＝關了但等不到 `Destroyed`。
    pub window_closed: Option<bool>,
}

impl RenderFailure {
    /// 排程器的失敗種類：恆為 [`FailureKind::Render`]（依 `retry_delay` 退避）。
    pub fn kind(&self) -> FailureKind {
        FailureKind::Render
    }
}

impl fmt::Display for RenderFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.render_id {
            Some(id) => write!(f, "渲染 #{id} 失敗：{}", self.reason),
            None => write!(f, "渲染失敗：{}", self.reason),
        }
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// 建立執行緒與 [`Renderer::render`] 之間的交接：誰先到誰決定視窗的去留。
struct OpenHandoff {
    /// 建立的結果（建立執行緒放入；render 取走）。
    result: Option<Result<Box<dyn OpenSurface>, String>>,
    /// render 已過期限、不再等：建立執行緒之後拿到的視窗由它自己關掉。
    abandoned: bool,
}

/// 建立執行緒的 RAII 守衛：正常路徑在交接時呼叫 [`OpenThreadGuard::release`] 清掉「建立中」旗標；
/// 沒呼叫到就離開（`surface.open` panic 而展開）時，drop 會清旗標，並在 render 還在等時交給它一個
/// 「建立失敗」，讓這次立刻結束、之後的渲染不被卡住（否則旗標永遠是 true，直到宿主重啟）。
struct OpenThreadGuard {
    handoff: Arc<(Mutex<OpenHandoff>, Condvar)>,
    in_flight: Arc<AtomicBool>,
    released: bool,
}

impl OpenThreadGuard {
    /// 正常交接：清旗標（呼叫端持有交接鎖）。之後的 drop 不再動旗標——這時下一次渲染可能已經把它
    /// 設回 true，不能誤清。
    fn release(&mut self) {
        self.in_flight.store(false, Ordering::SeqCst);
        self.released = true;
    }
}

impl Drop for OpenThreadGuard {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        let (lock, cv) = &*self.handoff;
        let mut slot = lock.lock().unwrap_or_else(|e| e.into_inner());
        self.in_flight.store(false, Ordering::SeqCst);
        if !slot.abandoned && slot.result.is_none() {
            slot.result = Some(Err(
                "建立渲染視窗的呼叫 panic（原因見記錄檔 fc_host::panic 一行）".into(),
            ));
            cv.notify_all();
        }
    }
}

/// 渲染器：每次開關一個隱藏視窗（見模組文件）。
pub struct Renderer<S> {
    surface: Arc<S>,
    broker: Arc<RenderBroker>,
    config: RenderConfig,
    busy: Mutex<()>,
    forbidden: Option<ThreadId>,
    /// 有一個建立視窗的呼叫還沒返回（逾時後被放棄的那個）。在它返回之前的渲染直接失敗，不再疊一條
    /// 卡住的建立執行緒。建立執行緒以 [`OpenThreadGuard`] 保證任何離開路徑（含 panic）都會清掉它。
    open_in_flight: Arc<AtomicBool>,
    /// 上一次渲染視窗那組的 browser 行程（關窗後記下；建立逾時後才建好、由建立執行緒自行關掉的視窗
    /// 也記在這裡）。下一次渲染前與宿主結束時由 [`Renderer::settle_previous_browser`] 處置。
    last_browser: Arc<Mutex<Option<Arc<dyn BrowserProcess>>>>,
}

type BrowserSlot = Arc<Mutex<Option<Arc<dyn BrowserProcess>>>>;

fn store_browser(slot: &BrowserSlot, browser: Option<Arc<dyn BrowserProcess>>) {
    let Some(b) = browser else { return };
    let previous = slot.lock().unwrap_or_else(|e| e.into_inner()).replace(b);
    // 正常不會發生（每次開窗前都先處置）；萬一有，至少記下 PID。
    if let Some(p) = previous {
        log::warn!(
            "桌布渲染：前一個渲染 browser 行程（PID {}）尚未處置就被取代",
            p.pid()
        );
    }
}

/// [`Renderer::shutdown_within`] 的時間切分：`(等它自己結束, 結束後確認)`，兩者相加等於 `limit`。
fn split_shutdown_limit(limit: Duration) -> (Duration, Duration) {
    let kill_wait = (limit / 3).min(BROWSER_KILL_WAIT);
    (limit.saturating_sub(kill_wait), kill_wait)
}

impl<S: RenderSurface + 'static> Renderer<S> {
    pub fn new(surface: S, broker: Arc<RenderBroker>, config: RenderConfig) -> Self {
        Renderer {
            surface: Arc::new(surface),
            broker,
            config,
            busy: Mutex::new(()),
            forbidden: None,
            open_in_flight: Arc::new(AtomicBool::new(false)),
            last_browser: Arc::new(Mutex::new(None)),
        }
    }

    /// 上一次渲染視窗那組的 browser 行程（self-test 用來觀察它何時結束）。
    pub fn last_browser(&self) -> Option<Arc<dyn BrowserProcess>> {
        self.last_browser
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// 處置前一次渲染的 browser 行程（bug-leaked-renderer）：等它結束，最多
    /// [`RenderConfig::browser_exit_wait`]；仍在就核對身分（[`BrowserProcess::check_owned`]）後結束它
    /// 並記一行 warn。核對不符或結束失敗只記 warn、不動它。
    ///
    /// 每次渲染建立視窗**之前**呼叫（不算進 30 秒渲染期限），宿主結束時也呼叫一次（[`Self::shutdown`]）。
    /// 在呼叫端執行緒等待（協調迴圈／結束執行緒），不得在主執行緒呼叫。
    pub fn settle_previous_browser(&self) -> BrowserSettle {
        let limit = self.config.browser_exit_wait;
        self.settle_with(
            limit,
            self.config.browser_kill_wait,
            &format!("{} 秒內沒有結束（卡在結束流程）", limit.as_secs_f64()),
        )
    }

    /// [`Self::settle_previous_browser`] 的本體：等 `wait`、逾時就核對並結束、再等 `kill_wait`。
    /// `situation` 是記錄裡「為何要處置」的說明（例如「5 秒內沒有結束」）。
    fn settle_with(&self, wait: Duration, kill_wait: Duration, situation: &str) -> BrowserSettle {
        let taken = self
            .last_browser
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        let Some(prev) = taken else {
            return BrowserSettle::None;
        };
        let pid = prev.pid();
        let t = Instant::now();
        if prev.wait_exit(wait) {
            let waited = t.elapsed();
            if waited >= Duration::from_millis(10) {
                log::info!(
                    "桌布渲染：前一個渲染 browser 行程（PID {pid}）已結束（等了 {:.0} ms）",
                    ms(waited)
                );
            }
            return BrowserSettle::Exited { pid, waited };
        }
        if let Err(e) = prev.check_owned() {
            log::warn!(
                "桌布渲染：前一個渲染 browser 行程（PID {pid}）{situation}，但核對不符、不結束它：{e}"
            );
            return BrowserSettle::Left { pid };
        }
        if let Err(e) = prev.terminate() {
            log::warn!(
                "桌布渲染：前一個渲染 browser 行程（PID {pid}）{situation}，結束它失敗：{e}"
            );
            return BrowserSettle::Left { pid };
        }
        if kill_wait.is_zero() {
            // 修正輪 2（N1）：不等待時無從得知是否已退出（TerminateProcess 是非同步的），不能記成
            // 「結束後仍未退出」——那句只留給有等待且等到逾時的路徑，供長跑統計「結束無效」的次數。
            log::warn!(
                "桌布渲染：前一個渲染 browser 行程（PID {pid}）{situation}，已要求結束（不等待確認）"
            );
            return BrowserSettle::TerminateRequested { pid };
        }
        let gone = prev.wait_exit(kill_wait);
        log::warn!(
            "桌布渲染：前一個渲染 browser 行程（PID {pid}）{situation}，已由宿主結束{}",
            if gone {
                ""
            } else {
                "，但結束後仍未退出"
            }
        );
        BrowserSettle::Terminated { pid, gone }
    }

    /// 審查 L3：事件迴圈結束時（`RunEvent::Exit`，主執行緒）的**不等待**處置，涵蓋系統匣「結束」以外
    /// 經 `app.exit` 的正常結束路徑。前一個渲染 browser 行程仍存活就核對、結束它，**不等**它退出（所有
    /// 等待都是 0；結束了就回 [`BrowserSettle::TerminateRequested`]，是否已退出為未知）；核對只讀 Toolhelp
    /// 快照與命令列，不送訊息、不阻塞。
    ///
    /// `session_ending`（工作階段正在結束，[`note_session_ending`]）為真時完全不碰、回 `None`：交給
    /// 系統結束工作階段內所有行程。系統匣「結束」已在結束執行緒處置過（[`Self::shutdown`]），這裡
    /// 通常沒有東西可做（回 `Some(BrowserSettle::None)`）。
    pub fn settle_on_exit(&self, session_ending: bool) -> Option<BrowserSettle> {
        if session_ending {
            return None;
        }
        Some(self.settle_with(Duration::ZERO, Duration::ZERO, "在事件迴圈結束時仍未結束"))
    }

    /// 宿主結束前呼叫：確保最後一次渲染的 browser 行程已結束或被結束（同
    /// [`Self::settle_previous_browser`]）。
    pub fn shutdown(&self) -> BrowserSettle {
        // 渲染進行中（協調迴圈應已停止，正常不會）：不等它，先處置已記錄的那個。
        if self.busy.try_lock().is_err() {
            log::warn!("桌布渲染：宿主結束時仍有渲染進行中，只處置已記錄的前一個 browser 行程");
        }
        self.settle_previous_browser()
    }

    /// 「因更新結束」（installer-auto-update task 3.3）的處置：同 [`Self::shutdown`]，但總共不超過 `limit`——
    /// 先等 `limit − 結束後確認`（`確認` ＝ `min(limit/3, BROWSER_KILL_WAIT)`），仍在就核對身分後結束，再等確認。
    /// 呼叫端（D4 步驟 6）的 `limit` 是分到的 5 秒或更少，**必須**在 `limit` 內返回，否則整段收尾會超過 20 秒。
    pub fn shutdown_within(&self, limit: Duration) -> BrowserSettle {
        if self.busy.try_lock().is_err() {
            log::warn!("桌布渲染：因更新結束時仍有渲染進行中，只處置已記錄的前一個 browser 行程");
        }
        let (wait, kill_wait) = split_shutdown_limit(limit);
        self.settle_with(
            wait,
            kill_wait,
            &format!("{:.1} 秒內沒有結束（宿主因更新結束）", wait.as_secs_f32()),
        )
    }

    /// 在這條執行緒呼叫 [`Renderer::render`] 一律回 [`RenderFailureReason::WrongThread`]（正式＝
    /// 主執行緒：等頁面回報時事件迴圈停住，回報永遠送不進來）。
    pub fn forbid_thread(mut self, id: ThreadId) -> Self {
        self.forbidden = Some(id);
        self
    }

    pub fn surface(&self) -> &S {
        &self.surface
    }

    pub fn broker(&self) -> &RenderBroker {
        &self.broker
    }

    /// 渲染一台螢幕並寫出輸出圖（阻塞，最長約 [`RenderConfig::timeout`]（含建立視窗）＋關窗上限＋寫檔）。
    pub fn render(
        &self,
        req: &RenderRequest,
        out: &OutputTarget<'_>,
    ) -> Result<RenderSuccess, RenderFailure> {
        self.render_page(req, out, None)
    }

    /// 以指定頁面代替主題頁（self-test 用 [`NEVER_REPORTS_PAGE`] 在真實宿主上走一次逾時路徑）；
    /// 其餘流程與 [`Renderer::render`] 完全相同。
    pub fn render_custom_page(
        &self,
        req: &RenderRequest,
        out: &OutputTarget<'_>,
        page: &str,
    ) -> Result<RenderSuccess, RenderFailure> {
        self.render_page(req, out, Some(page))
    }

    fn render_page(
        &self,
        req: &RenderRequest,
        out: &OutputTarget<'_>,
        page: Option<&str>,
    ) -> Result<RenderSuccess, RenderFailure> {
        let fail = |id: Option<RenderId>, reason: RenderFailureReason| RenderFailure {
            render_id: id,
            reason,
            window_closed: None,
        };
        if self.forbidden == Some(thread::current().id()) {
            log::error!("桌布渲染在禁止的執行緒被呼叫（主執行緒），拒絕");
            return Err(fail(None, RenderFailureReason::WrongThread));
        }
        let url_of = |id| match page {
            Some(p) => page_url_for(p, req, id),
            None => page_url(req, id),
        };
        let invalid = if is_valid_monitor_key(&req.monitor_key) {
            url_of(0).err()
        } else {
            Some(format!("螢幕鍵格式不符：{:?}", req.monitor_key))
        };
        if let Some(m) = invalid {
            let failure = fail(None, RenderFailureReason::InvalidRequest(m));
            log::warn!("{failure}");
            return Err(failure);
        }

        let _serial = self.busy.lock().unwrap_or_else(|e| e.into_inner());
        if self.open_in_flight.load(Ordering::SeqCst) {
            let failure = fail(
                None,
                RenderFailureReason::OpenFailed(
                    "上一次建立渲染視窗逾時後仍未返回，本次不再建立".into(),
                ),
            );
            log::warn!("{failure}");
            return Err(failure);
        }
        // bug-leaked-renderer：前一個渲染 browser 行程還在時以同一個資料夾建立新環境，新環境會併進它
        // （重現：關窗後約 100 ms 內開始的下一次拿到同一個 PID）或與它的結束流程重疊；6.2 長跑另有一次
        // 它卡在結束流程、永不退出。先等它結束（逾時且核對符合就由宿主結束它），不算進下面的 30 秒期限。
        self.settle_previous_browser();
        let ticket = self.broker.begin();
        let id = ticket.id();
        let url = url_of(id).map_err(|m| fail(Some(id), RenderFailureReason::InvalidRequest(m)))?;
        let label = renderer_label(id);
        log::info!(
            "桌布渲染 #{id} 開始：{} {}×{} t={} tz={} 螢幕 {}{}",
            req.theme.as_str(),
            req.width,
            req.height,
            iso_utc(req.as_of),
            req.tz,
            req.monitor_key,
            page.map_or_else(String::new, |p| format!("（頁面 {p}）"))
        );

        let mut timings = RenderTimings::default();
        let t0 = Instant::now();
        let deadline = t0 + self.config.timeout;
        let window = match self.open_with_deadline(id, &label, &url, deadline) {
            Ok(w) => w,
            Err(failure) => {
                log::warn!("{failure}（保留舊圖）");
                return Err(failure);
            }
        };
        timings.open = t0.elapsed();
        let t1 = Instant::now();
        let report = ticket.wait(deadline.saturating_duration_since(Instant::now()));
        timings.page = t1.elapsed();
        // 先撤銷等待再關窗：關窗期間才到的回報一律算晚到。
        drop(ticket);
        let browser = window.browser();
        let browser_pid = browser.as_ref().map(|b| b.pid());
        let t2 = Instant::now();
        let closed = match window.close(self.config.close_timeout) {
            Ok(()) => true,
            Err(e) => {
                log::warn!(
                    "桌布渲染 #{id} 關閉渲染視窗 {label} 異常：{e}（下一次使用新的 label，不受影響）"
                );
                false
            }
        };
        timings.close = t2.elapsed();
        store_browser(&self.last_browser, browser);

        let result = self
            .finish(id, req, out, report, &mut timings)
            .map(|mut s| {
                s.browser_pid = browser_pid;
                s
            })
            .map_err(|mut f| {
                f.window_closed = Some(closed);
                f
            });
        match &result {
            Ok(s) => {
                for w in &s.warnings {
                    log::warn!("桌布渲染 #{id} 頁面警告：{w}");
                }
                log::info!(
                    "桌布渲染 #{id} 完成：{}（{}）建立 {:.0} ms、頁面 {:.0} ms、關窗 {:.0} ms、寫檔 {:.0} ms、本體 {}",
                    s.path.display(),
                    s.slot.suffix(),
                    ms(timings.open),
                    ms(timings.page),
                    ms(timings.close),
                    ms(timings.write),
                    s.body.as_str()
                );
            }
            Err(f) => log::warn!(
                "{f}（保留舊圖；視窗{}）",
                if closed { "已關閉" } else { "關閉異常" }
            ),
        }
        result
    }

    /// 在專屬執行緒建立視窗，最多等到 `deadline`。逾時就放棄：之後建立執行緒拿到的視窗由它自己關掉。
    fn open_with_deadline(
        &self,
        id: RenderId,
        label: &str,
        url: &str,
        deadline: Instant,
    ) -> Result<Box<dyn OpenSurface>, RenderFailure> {
        let fail = |reason| RenderFailure {
            render_id: Some(id),
            reason,
            window_closed: None,
        };
        let handoff = Arc::new((
            Mutex::new(OpenHandoff {
                result: None,
                abandoned: false,
            }),
            Condvar::new(),
        ));
        self.open_in_flight.store(true, Ordering::SeqCst);
        let spawned = {
            let surface = Arc::clone(&self.surface);
            let mut guard = OpenThreadGuard {
                handoff: Arc::clone(&handoff),
                in_flight: Arc::clone(&self.open_in_flight),
                released: false,
            };
            let close_timeout = self.config.close_timeout;
            let late_browser = Arc::clone(&self.last_browser);
            let (label, url) = (label.to_owned(), url.to_owned());
            thread::Builder::new()
                .name("wallpaper-render-open".into())
                .spawn(move || {
                    let result = surface.open(&label, &url);
                    let handoff = Arc::clone(&guard.handoff);
                    let (lock, cv) = &*handoff;
                    let mut slot = lock.lock().unwrap_or_else(|e| e.into_inner());
                    if slot.abandoned {
                        // 期限之後才建好的視窗由這裡關掉；它的 browser 行程同樣交給下一次渲染前處置
                        // （先記下再放行下一次渲染）。
                        if let Ok(window) = &result {
                            store_browser(&late_browser, window.browser());
                        }
                    }
                    guard.release();
                    if slot.abandoned {
                        drop(slot);
                        if let Ok(window) = result {
                            log::warn!("桌布渲染視窗 {label} 在期限之後才建好，直接關閉");
                            if let Err(e) = window.close(close_timeout) {
                                log::warn!("關閉逾時後才建好的渲染視窗 {label} 異常：{e}");
                            }
                        }
                    } else {
                        slot.result = Some(result);
                        cv.notify_all();
                    }
                })
        };
        if let Err(e) = spawned {
            self.open_in_flight.store(false, Ordering::SeqCst);
            return Err(fail(RenderFailureReason::OpenFailed(format!(
                "無法建立「建立視窗」執行緒：{e}"
            ))));
        }

        let (lock, cv) = &*handoff;
        let mut slot = lock.lock().unwrap_or_else(|e| e.into_inner());
        while slot.result.is_none() {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                slot.abandoned = true;
                return Err(fail(RenderFailureReason::Timeout(self.config.timeout)));
            }
            slot = cv
                .wait_timeout(slot, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        match slot.result.take() {
            Some(Ok(window)) => Ok(window),
            Some(Err(e)) => Err(fail(RenderFailureReason::OpenFailed(e))),
            None => unreachable!("迴圈結束時 result 一定有值"),
        }
    }

    fn finish(
        &self,
        id: RenderId,
        req: &RenderRequest,
        out: &OutputTarget<'_>,
        report: Option<PageReport>,
        timings: &mut RenderTimings,
    ) -> Result<RenderSuccess, RenderFailure> {
        let fail = |reason| RenderFailure {
            render_id: Some(id),
            reason,
            window_closed: None,
        };
        let expected = (req.width, req.height);
        let (body, png, meta) = match report {
            None => return Err(fail(RenderFailureReason::Timeout(self.config.timeout))),
            Some(PageReport::Failed { error, meta }) => {
                for w in &meta.warnings {
                    log::warn!("桌布渲染 #{id} 頁面警告：{w}");
                }
                return Err(fail(RenderFailureReason::PageFailed(error)));
            }
            Some(PageReport::Done { png: Err(e), .. }) => {
                return Err(fail(RenderFailureReason::BadBody(e)))
            }
            Some(PageReport::Done {
                png: Ok((body, png)),
                meta,
            }) => (body, png, meta),
        };
        let actual = png_dimensions(&png);
        if actual != Some(expected) {
            return Err(fail(RenderFailureReason::SizeMismatch { expected, actual }));
        }
        let t = Instant::now();
        let written = write_output(out.dir, &req.monitor_key, &png, expected, out.displayed)
            .map_err(|e| fail(RenderFailureReason::WriteFailed(e)))?;
        timings.write = t.elapsed();
        for p in &written.removed {
            log::info!("桌布渲染 #{id} 清除殘檔 {}", p.display());
        }
        Ok(RenderSuccess {
            render_id: id,
            path: written.path,
            slot: written.slot,
            width: req.width,
            height: req.height,
            warnings: meta.warnings,
            body,
            timings: *timings,
            browser_pid: None,
        })
    }
}

// ── Tauri：隱藏渲染視窗與兩個指令 ─────────────────────────────────────────────────────────

/// 正式的渲染視窗工廠：每次 [`RenderSurface::open`] 建一個隱藏視窗，使用獨立的 WebView2 使用者
/// 資料夾（[`webview_env::renderer_data_dir_for`]）。
pub struct TauriSurface {
    app: AppHandle,
    data_dir: PathBuf,
}

impl TauriSurface {
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }
}

struct TauriOpen {
    window: WebviewWindow,
    destroyed: mpsc::Receiver<()>,
    browser: Option<Arc<dyn BrowserProcess>>,
}

/// 讀 browser 行程 PID 的上限（`with_webview` 要等主執行緒派送）。
const BROWSER_PID_TIMEOUT: Duration = Duration::from_secs(5);

/// 正式的 [`BrowserProcess`]：渲染視窗建好時開的行程把手。
pub struct TauriBrowser {
    handle: desktop::browser_process::ProcessHandle,
    /// 渲染視窗的 WebView2 使用者資料夾（[`TauriSurface::data_dir`]）。
    renderer_dir: PathBuf,
}

impl BrowserProcess for TauriBrowser {
    fn pid(&self) -> u32 {
        self.handle.pid()
    }

    fn wait_exit(&self, timeout: Duration) -> bool {
        self.handle.wait_exit(timeout)
    }

    fn check_owned(&self) -> Result<(), String> {
        let id = self.handle.identity()?;
        verify_renderer_browser(
            id.parent_pid,
            &id.command_line,
            std::process::id(),
            &self.renderer_dir,
        )
    }

    fn terminate(&self) -> Result<(), String> {
        self.handle.terminate()
    }
}

impl RenderSurface for TauriSurface {
    fn open(&self, label: &str, url: &str) -> Result<Box<dyn OpenSurface>, String> {
        // 舊的渲染視窗還在（上次等不到 Destroyed、或建立逾時後才建好）：記錄並再要求關一次。
        // 每次渲染用新的 label，所以它們不會擋住這一次。
        let windows = self.app.webview_windows();
        let lingering = lingering_renderer_labels(windows.keys().map(String::as_str), label);
        if !lingering.is_empty() {
            log::warn!("舊的桌布渲染視窗仍存在：{lingering:?}，再要求關閉一次");
            for l in &lingering {
                if let Some(w) = windows.get(l) {
                    if let Err(e) = w.destroy() {
                        log::warn!("再次關閉渲染視窗 {l} 失敗：{e}");
                    }
                }
            }
        }
        drop(windows);
        let window = WebviewWindowBuilder::new(&self.app, label, WebviewUrl::App(url.into()))
            .title("fc-host wallpaper-renderer")
            .visible(false)
            .focused(false)
            .focusable(false)
            .skip_taskbar(true)
            .decorations(false)
            .inner_size(640.0, 360.0)
            .data_directory(self.data_dir.clone())
            .build()
            .map_err(|e| format!("WebviewWindowBuilder::build 失敗：{e}"))?;
        let (tx, rx) = mpsc::channel();
        window.on_window_event(move |event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                let _ = tx.send(());
            }
        });
        // 不出現在工作列、不搶焦點的保險（探針 1.4：視窗從未顯示，工作列本來就不會登記它）。
        match window.hwnd() {
            Ok(hwnd) => {
                if let Err(e) = desktop::apply_widget_ex_style(hwnd) {
                    log::warn!("渲染視窗套用延伸樣式失敗：{e}");
                }
            }
            Err(e) => log::warn!("渲染視窗取得 HWND 失敗：{e}"),
        }
        let browser: Option<Arc<dyn BrowserProcess>> =
            match desktop::webview_browser_process(&window, BROWSER_PID_TIMEOUT) {
                Ok(handle) => Some(Arc::new(TauriBrowser {
                    handle,
                    renderer_dir: self.data_dir.clone(),
                })),
                Err(e) => {
                    log::warn!("渲染視窗 {label} 讀不到 browser 行程：{e}");
                    None
                }
            };
        Ok(Box::new(TauriOpen {
            window,
            destroyed: rx,
            browser,
        }))
    }
}

impl OpenSurface for TauriOpen {
    fn browser(&self) -> Option<Arc<dyn BrowserProcess>> {
        self.browser.clone()
    }

    fn close(self: Box<Self>, timeout: Duration) -> Result<(), String> {
        self.window
            .destroy()
            .map_err(|e| format!("destroy 失敗：{e}"))?;
        self.destroyed
            .recv_timeout(timeout)
            .map_err(|_| format!("{} 秒內沒有收到 Destroyed", timeout.as_secs_f64()))
    }
}

/// 正式渲染器（Tauri managed state）。
pub type WallpaperRenderer = Renderer<TauriSurface>;

/// 工作階段正在結束（`WM_QUERYENDSESSION`／`WM_ENDSESSION` 且 wParam 非 0；取消時清除）。只給
/// [`settle_on_event_loop_exit`] 判斷「交給系統」用。
static SESSION_ENDING: AtomicBool = AtomicBool::new(false);

/// 記下工作階段是否正在結束（`main.rs` 的系統事件回呼呼叫；立即返回）。
pub fn note_session_ending(ending: bool) {
    SESSION_ENDING.store(ending, Ordering::SeqCst);
}

/// `RunEvent::Exit`（主執行緒）呼叫：[`Renderer::settle_on_exit`]，不等待。沒有渲染器（Secondary、
/// 渲染管線停用）時什麼都不做。
pub fn settle_on_event_loop_exit(app: &AppHandle) {
    let Some(renderer) = app.try_state::<WallpaperRenderer>() else {
        return;
    };
    match renderer.settle_on_exit(SESSION_ENDING.load(Ordering::SeqCst)) {
        None => log::info!("事件迴圈結束：工作階段結束中，渲染 browser 行程交給系統"),
        Some(BrowserSettle::None) => {}
        Some(s) => log::info!("事件迴圈結束：前一個渲染 browser 行程 {s:?}"),
    }
}

/// 登記渲染管線的 managed state（`setup` 內、主執行緒呼叫；見模組文件「給 4.7 的 API」）。
/// 渲染視窗的使用者資料夾解析不到時只登記 broker、不登記渲染器並回 `false`——**不**退回與小工具
/// 共用的環境。
pub fn install(app: &AppHandle) -> bool {
    let broker = Arc::new(RenderBroker::new());
    app.manage(Arc::clone(&broker));
    let Some(data_dir) = webview_env::renderer_data_dir_for(app) else {
        log::error!("無法決定桌布渲染視窗的 WebView2 使用者資料夾，桌布渲染停用");
        return false;
    };
    log::info!(
        "桌布渲染管線就緒：渲染視窗使用者資料夾 {}（小工具那組：{}）",
        data_dir.display(),
        webview_env::widget_data_dir().map_or_else(
            || "Tauri 預設（app 本機資料夾）".to_owned(),
            |d| d.display().to_string()
        )
    );
    let surface = TauriSurface {
        app: app.clone(),
        data_dir,
    };
    app.manage(
        Renderer::new(surface, broker, RenderConfig::default())
            .forbid_thread(thread::current().id()),
    );
    true
}

/// 頁面畫完（`wallpaper.mjs`「宿主接法」）：本體＝PNG，meta 在 `x-wallpaper-meta` header。id 不符
/// 或不是渲染視窗送來的回 `Err`（頁面只記 `console.warn`）。
///
/// Rust 函式名刻意與 IPC 指令名不同：`#[tauri::command]` 對 `pub` 函式產生 `#[macro_export]` 的
/// 輔助巨集（`tauri-macros-2.7.0/src/command/wrapper.rs`），與探針 1.4（`probe-render` feature）的
/// 同名函式會在 crate 根相撞；指令名以 `rename` 維持頁面契約的 `wallpaper_render_done`。
#[tauri::command(rename = "wallpaper_render_done")]
pub fn render_done_command(
    webview: tauri::Webview,
    request: Request<'_>,
    broker: State<'_, Arc<RenderBroker>>,
) -> Result<(), String> {
    let meta = request
        .headers()
        .get(META_HEADER)
        .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned());
    match broker.deliver_done(webview.label(), request.body(), meta.as_deref()) {
        Delivery::Accepted => Ok(()),
        Delivery::Stale { .. } => Err("render id 不符或已逾時，宿主已丟棄這份圖".into()),
        Delivery::NotRenderer => Err("只有桌布渲染視窗可以回報".into()),
    }
}

/// 頁面失敗（字型、資料、draw 拋錯、匯出失敗）。
#[tauri::command(rename = "wallpaper_render_failed")]
pub fn render_failed_command(
    webview: tauri::Webview,
    error: String,
    meta: Value,
    broker: State<'_, Arc<RenderBroker>>,
) -> Result<(), String> {
    match broker.deliver_failed(webview.label(), error, &meta) {
        Delivery::Accepted => Ok(()),
        Delivery::Stale { .. } => Err("render id 不符或已逾時，宿主已丟棄這份回報".into()),
        Delivery::NotRenderer => Err("只有桌布渲染視窗可以回報".into()),
    }
}

#[cfg(test)]
mod tests;
