//! 小工具視窗管理與註冊表（design.md D1、D4、D6、D7）。
//!
//! 本 task（2.7）交付核心↔頁面的 IPC 介面（D4）：
//! - [`AppState`]：Tauri managed state——設定、設定檔路徑、[`crate::data::ChannelRegistry`]、
//!   編輯版面旗標，五個 IPC 指令與排程推播都透過它存取／變更。
//! - 指令：[`get_snapshot`]、[`get_settings`]、[`update_settings`]、[`set_edit_mode`]、
//!   [`report_content`]，以及 fix round 1 新增的 [`subscribe_data`]（`data` 推播的訂閱入口）。
//! - [`poll_and_notify`]：排程（`main.rs` 每 30 秒呼叫一次）輪詢所有通道，只對「這輪真的更新
//!   了」的通道推播快照、只送給以 [`subscribe_data`] 訂閱該通道的 webview（design.md D5／D4）。
//!   **`data` 不再是 Tauri 事件**：改走 `tauri::ipc::Channel`，投遞對象由接收 webview 的
//!   身分決定，理由見 [`DataSubscribers`]（fix round 1，Codex 2.7）。
//! - [`WIDGET_SPECS`]／[`channel_for_label`]：Rust 端小工具清單（id、通道、倍率設計框
//!   `ZoomBox`——widget-adaptive-zoom-and-grid design.md D1；task 7.2 起版面欄位只放 Rust，
//!   `host/ui/registry.js` 只列 id 與通道）。預設
//!   開關與格座標在 [`crate::settings`]（`DEFAULT_GRID_RECTS`）。
//!
//! task 3.1 補上視窗工廠（design.md D1、D3、D8、D9）：
//! - [`plan_window_changes`]：純函式，依設定與目前已存在的小工具視窗算出要建立／關閉哪些。
//! - [`sync_widget_windows`]：啟動時（`main.rs` setup）與每次 [`update_settings`] 之後呼叫，
//!   依上面的計畫建立或關閉視窗；以 [`AppState::window_sync`] 串行化。
//! - [`create_widget_window`]：`visible(false)` 建立 `w-<id>` 視窗後交給
//!   [`crate::desktop::present_widget`] 一次完成樣式、位置、顯示與置底。
//!
//! task 7.2 改為格線版面（design.md D7、D9；取代 task 3.2／3.5／5.3 的錨點模型）：
//! - 視窗的位置與大小**完全由格子決定**：[`resolve_enabled_widgets`]（純函式）把設定裡所有
//!   開啟中小工具的記錄位置＋目前顯示器清單交給 [`crate::layout::resolve_grid_placements`]
//!   做兩階段推導，得到每個小工具的實體矩形與倍率（[`crate::layout::content_zoom`]：依矩形
//!   邏輯寬高、倍率設計框與字級設定，夾 0.5–3，widget-adaptive-zoom-and-grid design.md D1）。
//!   視窗建立（[`create_widget_window`]）、多螢幕重排與 `update_settings` 後重排
//!   （[`relayout_all_widgets`]）、拖曳結束（[`finish_widget_drag`]）都走這條路徑。
//! - [`report_content`]（task 7.3 取代 `report_size`；頁面端「內容高度為 0」的判定不變，只是
//!   改成布林值傳來，見 `host/ui/widget.html`）：不改變視窗大小——無內容 → 隱藏（編輯版面期間
//!   例外，見下方）；有內容 → 顯示並重套倍率（頁面 Reload 後 ZoomFactor 可能回到 1 的保險）。
//! - [`WidgetRuntime`]（[`AppState::widget_runtime`]）：每個視窗 label 目前套用的倍率、
//!   「無內容」與「空間不足暫時隱藏」兩種隱藏原因。[`WidgetRuntime::should_show`]（task 7.3
//!   加了 `edit_mode` 參數）：空間不足永遠隱藏；無內容則只在**不在編輯模式**時隱藏——編輯
//!   版面期間 SHALL 看得到所有開啟中且有格子的小工具（design.md D7「無內容」；
//!   specs/widget-host-windows「編輯版面時看得到無內容的小工具」），沒有格子的（空間不足）
//!   不算在內。
//! - [`finish_widget_drag`]：`desktop::register_widget_drag_hooks` 登記的回呼，`WM_EXITSIZEMOVE`
//!   （拖曳結束）時呼叫；fix F6b 起只記下待判定（[`DragTracker`]），`desktop` 投遞自訂訊息後
//!   由 [`settle_widget_drag`] 以當時的視窗矩形判定（[`settle_drop`]）——以 [`crate::layout::legal_move_placement`] 做移動對齊（保留目前格數）
//!   與合法判斷（task 7.5），合法就寫回記錄，一律重新推導並套用（不合法＝彈回推導出的原矩形）。
//! - task 7.5：[`begin_widget_drag`]（`WM_ENTERSIZEMOVE`，建立 [`DragSession`] 快照）與
//!   [`update_widget_drag`]（`WM_MOVING`）以同一個合法判斷預告「若此刻放開」的結果，合法性改變
//!   時廣播 `edit-preview { id, valid }`（[`EditPreviewTracker`] 去重），頁面據此切換紅框。
//! - task 7.6：調整大小。進出編輯版面時 [`sync_widget_resizable`] 切換視窗的 `WS_SIZEBOX`
//!   （探針實測沒有它 `startResizeDragging` 進不了尺寸迴圈，design.md D7「調整大小的機制」）；
//!   [`update_widget_resize`]（`WM_SIZING`）以 [`crate::layout::legal_resize_placement`] 預告，
//!   [`DragSession`] 記下被拖邊，延後判定（[`settle_drop`]）據此改走 [`resize_end_settings`]
//!   （只對齊被拖邊；不合法＝彈回推導出的原矩形）。
//! - [`set_edit_mode`]／[`switch_edit_mode`]（task 5.3；fix F2 起先存檔再切換）：系統匣「編輯版面」是目前唯一的
//!   解鎖入口，進入編輯模式即把 `Settings::layout_locked` 同步成 `false`（解鎖）、離開即同步回
//!   `true`（上鎖）並存檔＋廣播 `settings` 事件。「為什麼在 Rust 端偵測拖曳結束、鎖定為何與
//!   3.3／3.4 的置底機制無干擾、`startDragging()` 是否搶焦點」的完整分析在 `crate::desktop`
//!   模組文件「task 5.3 補上」一節。task 7.3 另補：切換 `edit_mode` 後呼叫
//!   [`sync_widget_visibility_for_edit_mode`]，依新的 `edit_mode` 值重新對帳每個小工具視窗的
//!   顯示狀態（進入時補顯示因無內容而隱藏者、離開時把它們藏回去）。
//!
//! task 5.1 補上暫停原因集合（design.md D12「自動暫停」；specs/widget-host-lifecycle）：
//! - [`PauseReason`]／[`AppState::pause_reasons`]：暫停原因以集合表示，任一原因啟用即暫停
//!   （`paused = !reasons.is_empty()`）；本 task 只有 [`PauseReason::Manual`] 這一個成員
//!   （系統匣「暫停／繼續」，見 `crate::tray`）。5.5 的 `SHQueryUserNotificationState` 輪詢、
//!   鎖定、顯示器關閉、電源狀態等其餘暫停來源加入時，只需要新增 `PauseReason` 變體＋把它排進
//!   [`PAUSE_REASON_PRIORITY`］、在該來源偵測到狀態變化時呼叫 [`set_pause_reason`]，不需要
//!   再改這裡的機制或 IPC 介面本身。
//! - [`set_pause_reason`]：變更某個原因的啟用狀態，重算整體 `paused` 並廣播 `pause` 事件
//!   （design.md D4：`{ paused, reason }`，比照 [`set_edit_mode`] 的 `edit-mode` 廣播方式）。
//!
//! task 5.5 依上述預留擴充：
//! - [`PauseReason`] 補上 `Locked`／`SystemBusy`／`DisplayOff`／`Battery`／`PowerSaver` 五個
//!   變體（`crate::desktop` 的偵測機制見其「D12：自動暫停偵測」一節）；`PAUSE_REASON_PRIORITY`
//!   與 [`reason_label`] 一併擴充。
//! - [`manual_pause_status`]：取代原本的 `is_paused`——系統匣「暫停／繼續」只切
//!   [`PauseReason::Manual`]，選單文字（`crate::tray::pause_toggle_label`）需要區分「這個
//!   按鈕本身的開關狀態」與「除了手動之外還有沒有其他原因在暫停」，故拆成
//!   `(manual_active, other_reason)` 兩個值，[`top_other_reason`] 是後者的純函式核心。
//! - 電池／省電「是否要真的觸發暫停」取決於 `settings::PauseRules`，見「電池／省電：業務判斷」
//!   一節（[`set_battery_raw`]／[`set_power_saver_raw`]／[`reapply_power_pause_rules`]）。
//! - [`refresh_all_widget_appearance`]：design.md D8「省電模式變化時重新判定外觀」的重判路徑。
//!
//! task 7.4 補上「開啟小工具找空位」與系統匣「空間不足暫時隱藏」標示（design.md D7「記錄位置的
//! 寫回時機」第 2 點、D9；specs/widget-host-windows「小工具互不重疊」「開啟小工具但原位置已被
//! 佔用」「空間不足」Scenario）：
//! - [`place_newly_enabled_widgets`]：[`update_settings`] 合併後、存檔前呼叫，對本次 patch 由
//!   關閉變開啟的小工具依 [`WIDGET_SPECS`] 順序逐一委派 [`crate::layout::
//!   placement_for_opening_widget`]；找到空位就寫回，任何一個找不到空位就整筆失敗（純函式，
//!   不碰 `AppState`，單元測試直接餵合成的 `Settings`／`MonitorInfo`）。
//! - [`WidgetSpec::display_name`]：系統匣「空間不足，暫時隱藏：<名稱>」要用的中文名，沿用設定
//!   視窗 `host/ui/settings.html` 既有的 `WIDGET_LABELS` 文案（Rust 端原本沒有這份名稱）。
//! - [`no_space_hidden_menu_entries`]：純函式，[`resolve_enabled_widgets`] 的推導結果 →
//!   系統匣選單要顯示的 `(小工具 id, 選單文字)` 清單，只含 `HiddenNoSpace` 者。
//!   [`current_no_space_hidden`] 是它的 `AppHandle` 版本，供 `crate::tray::refresh` 呼叫。
//! - 選單更新時機：`crate::tray::refresh` 在每次 [`relayout_all_widgets`]（所有既有小工具重排）
//!   與 [`sync_widget_windows`]（新建立小工具、可能立即因空間不足而隱藏）之後各呼叫一次——這兩個
//!   函式是「推導結果可能改變」的收斂點，比照 task 5.5 `set_pause_reason` 文件「唯一收斂點」的
//!   同一個理由：讓每個觸發來源各自記得呼叫容易漏，收斂到少數幾個必經之處才不會有選單落後於
//!   實際狀態的遺漏。
//!
//! ## Command 權限（design.md D3；task 2.7 查證）
//!
//! 這五個指令（及 fix round 1 新增的 `subscribe_data`）**沒有**、也**不需要**在
//! `capabilities/` 底下另外宣告權限。Channel 大 payload 走的 `plugin:__TAURI_CHANNEL__|fetch`
//! 在 `on_message` 中被明文排除於 ACL 檢查之外（`request.cmd != FETCH_CHANNEL_DATA_COMMAND`）。查證
//! `tauri-2.12.0/src/webview/mod.rs` 的 `on_message`（約 2081–2114 行）：自訂 app 指令只有在
//! 「該指令帶 `plugin:` 前綴」「app 定義了自己的 ACL manifest（`RuntimeAuthority::
//! has_app_manifest`，即 `host/permissions/` 下有給本 crate 自己的權限檔）」或「呼叫來源不是
//! 本機（remote origin）」這三個條件之一成立時才會做 ACL 檢查；否則整段檢查直接略過、指令照常
//! 執行。本 crate 沒有 `host/permissions/` 目錄（`build.rs` 也沒呼叫
//! `tauri_build::Attributes::app_manifest(..)`），`tauri-utils-2.10.0/src/acl/mod.rs` 的
//! `has_app_manifest` 純粹檢查該 map 是否含 `APP_ACL_KEY`，沒有權限檔就沒有這把鑰匙、
//! `has_app_manifest` 恆為 `false`；小工具頁面（`WebviewUrl::App`，`tauri://localhost`，
//! `is_local_url` 判定為本機）呼叫這五個指令因此三個條件全部不成立，**ACL 檢查整段被略過、
//! 一律放行**——`capabilities/default.json` 的 `permissions` 清單只影響 `core:*`／
//! plugin 指令（fix F2 起已無 `autostart:*`），跟這五個自訂指令無關。本 task 的驗收（見
//! `host/tools/evidence/2.7-*.log`）在**未修改** `capabilities/default.json` 的前提下確認
//! release build 能成功 `invoke` 這五個指令，直接證實上述推論。
//!
//! 這個「預設放行」是 Tauri v2 的既有行為，非本專案特有設定；若日後這個 crate 真的建立了
//! `host/permissions/` 目錄（例如要用 tauri-plugin 化其中一部分邏輯），`has_app_manifest`
//! 就會變成 `true`，屆時這五個指令會需要被 `capabilities/default.json`（或新的 capability
//! 檔）明確放行，否則會突然被 ACL 擋下——留意這個前提改變時要回頭補權限宣告。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;
use std::thread;

use serde::Serialize;
use serde_json::Value;
use tauri::ipc::Channel;
use tauri::{
    AppHandle, Emitter, Manager, State, Webview, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
    Window,
};
use windows::Win32::Foundation::HWND;

use crate::data::{self, ChannelRegistry, Snapshot, SnapshotMeta};
use crate::desktop;
use crate::layout::{self, GridWidgetInput, MonitorInfo, ResolvedWidgetPlacement};
use crate::recovery::{self, RecoveryState};
use crate::settings::{self, Settings};
use crate::wallpaper_render;

mod grid_sync;

/// Rust 端小工具清單的一筆（design.md D6：只保存 `id`、通道與版面需要的尺寸常數，不認得
/// 小工具內容）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WidgetSpec {
    pub id: &'static str,
    /// 中文顯示名稱（task 7.4：系統匣「空間不足，暫時隱藏」項目要用；沿用設定視窗
    /// `host/ui/settings.html` 的 `WIDGET_LABELS` 既有文案，例如擴充插槽用「擴充插槽 N」
    /// 而非 id 本身）。
    pub display_name: &'static str,
    /// 訂閱的資料通道（design.md D4）。
    pub channel: &'static str,
    /// 倍率設計框（widget-adaptive-zoom-and-grid design.md D1）：最小框決定倍率上限與最小
    /// 格數（D3），舒適框決定自適應倍率；高度含上下兩個 [`WIDGET_GAP_CSS_PX`]。不變式
    /// `comfort ≥ min`（測試 `zoom_boxes_keep_comfort_at_least_min`）。
    pub zoom_box: layout::ZoomBox,
}

/// `host/ui/widget.css` 的 `--widget-gap`（CSS 像素）：`#widget-root` 四周的透明邊距
/// （visual fix `443718d`）。最小框高度要把上下兩個 gap 算進去（測試
/// `zoom_box_min_heights_include_widget_gap` 核對與 CSS 一致）。
pub const WIDGET_GAP_CSS_PX: f64 = 8.0;

/// 十個小工具（design.md D4／D6／D7）：財經五個共用 `tw-events`，`customN` 訂閱同名通道。
///
/// 版面欄位只放 Rust（design.md D6，task 7.2）：前端 `host/ui/registry.js` 只列 id 與通道
/// （一致性測試 `specs_match_frontend_registry_js` 核對）。倍率設計框數值＝
/// widget-adaptive-zoom-and-grid design.md D1 表格（測試 `zoom_boxes_match_design_d1_table`）：
///
/// | 小工具 | min（寬×高） | comfort（寬×高） |
/// |---|---|---|
/// | 時鐘 | 212 × 160 | 同 min |
/// | 總經日曆 | 375 × 216 | 500 × 324 |
/// | 台股固定／動態事件 | 352.5 × 176 | 470 × 264 |
/// | 行情條 | 992 × 60 | 不限 × 60 |
/// | 擴充插槽 | 352.5 × 136 | 470 × 204 |
///
/// - 時鐘：task 2.1 以 headless Edge 實測最寬內容的視窗需求 207.625 × 155.1875（已含左右、
///   上下 gap），寬高取整後加 4 作字型差異餘裕（`openspec/changes/widget-adaptive-zoom-and-grid/
///   task-2.1-report.md`；重跑 `node host/tests/compare/measure-clock-natural-width.mjs`）。
///   舒適框同 min：文字要填滿框。min 高 160 大於舊設計最小高 156，D3 記載其窄帶例外。
/// - 清單類（總經日曆、台股事件、擴充插槽）：comfort 寬＝原設計寬（Lively 版
///   `finance-calendar.html` CONFIG v6.2 的 `macroWidth='500px'`、`eventsWidth='470px'`；擴充
///   插槽取台股事件欄寬），框夠高時倍率與舊模型相同；min 寬＝0.75 × 設計寬；min 高＝面板內容
///   最小高度（總經 200、台股事件 160、擴充插槽 120，tasks.md 7.2 初值）＋上下 gap；comfort
///   高＝1.5 × min 高。
/// - 行情條：跑馬燈寬度不受限（comfort 寬 `None`），字級由高度決定；min 高＝原 `.ticker` 固定
///   高度 44＋上下 gap；min 寬 992（兩欄＋欄距 500 + 470 + 22）只用於最小格數（維持 496 邏輯
///   像素的最小寬）。
///
/// 順序與 [`crate::settings::WIDGET_IDS`] 相同（測試核對）。
pub const WIDGET_SPECS: [WidgetSpec; 10] = [
    finance_spec("clock", "時鐘", zoom_box(212.0, 160.0, Some(212.0), 160.0)),
    finance_spec("macro", "總經日曆", list_box(375.0, 200.0, 500.0)),
    finance_spec("fixed", "台股固定事件", list_box(352.5, 160.0, 470.0)),
    finance_spec("dynamic", "台股動態事件", list_box(352.5, 160.0, 470.0)),
    finance_spec(
        "quotes",
        "行情條",
        zoom_box(
            992.0,
            44.0 + 2.0 * WIDGET_GAP_CSS_PX,
            None,
            44.0 + 2.0 * WIDGET_GAP_CSS_PX,
        ),
    ),
    custom_spec("custom1", "擴充插槽 1"),
    custom_spec("custom2", "擴充插槽 2"),
    custom_spec("custom3", "擴充插槽 3"),
    custom_spec("custom4", "擴充插槽 4"),
    custom_spec("custom5", "擴充插槽 5"),
];

const fn zoom_box(
    min_width: f64,
    min_height: f64,
    comfort_width: Option<f64>,
    comfort_height: f64,
) -> layout::ZoomBox {
    layout::ZoomBox {
        min_width,
        min_height,
        comfort_width,
        comfort_height,
    }
}

/// 清單類小工具的框（widget-adaptive-zoom-and-grid design.md D1）：min＝`min_width` ×（面板
/// 內容最小高度＋上下 gap），comfort＝`comfort_width` × 1.5 倍 min 高。
const fn list_box(min_width: f64, panel_min_height: f64, comfort_width: f64) -> layout::ZoomBox {
    let min_height = panel_min_height + 2.0 * WIDGET_GAP_CSS_PX;
    zoom_box(min_width, min_height, Some(comfort_width), min_height * 1.5)
}

const fn finance_spec(
    id: &'static str,
    display_name: &'static str,
    zoom_box: layout::ZoomBox,
) -> WidgetSpec {
    WidgetSpec {
        id,
        display_name,
        channel: data::TW_EVENTS_CHANNEL,
        zoom_box,
    }
}

/// 擴充插槽：通道與 id 同名；框同台股事件寬度、面板最小高度 120（tasks.md 7.2）。
const fn custom_spec(id: &'static str, display_name: &'static str) -> WidgetSpec {
    WidgetSpec {
        id,
        display_name,
        channel: id,
        zoom_box: list_box(352.5, 120.0, 470.0),
    }
}

/// 依 id 查 [`WIDGET_SPECS`]。
pub fn widget_spec(id: &str) -> Option<&'static WidgetSpec> {
    WIDGET_SPECS.iter().find(|spec| spec.id == id)
}

/// 小工具視窗 label 的固定規則（design.md D3、`capabilities/default.json` 的
/// `windows: ["w-*", "settings"]`）；[`widget_id_from_label`] 是它的反向查詢。
///
/// task 5.6：WebView2 故障復原重建的視窗改用 `w-<id>-r<n>`
/// （[`crate::recovery::rebuilt_window_label`]）——先建新視窗、再拆舊視窗的期間兩者同時
/// 存在，Tauri 的 label 必須唯一。因此「這個視窗是哪個小工具」一律經
/// [`widget_id_from_label`] 解析，不可再假設 label＝`w-<id>`。
pub fn window_label(widget_id: &str) -> String {
    format!("w-{widget_id}")
}

/// 由視窗 label 解析小工具 id：`w-<id>`（一般建立）或 `w-<id>-r<n>`（task 5.6 重建，`n` 為
/// 十進位數字）。不是小工具視窗（`settings`、守門視窗）、未知 id、或後綴格式不符時回傳
/// `None`。小工具 id 本身不含 `-`（[`WIDGET_SPECS`]），故以第一個 `-r` 切開不會誤判。
pub fn widget_id_from_label(label: &str) -> Option<&'static str> {
    let rest = label.strip_prefix("w-")?;
    let id = match rest.split_once("-r") {
        Some((id, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => id,
        Some(_) => return None,
        None => rest,
    };
    widget_spec(id).map(|spec| spec.id)
}

/// 目前存在的所有小工具視窗 label（依 [`widget_id_from_label`] 判定，含重建後的
/// `w-<id>-r<n>`）。task 5.6 看門狗與全部重建用。
pub fn widget_window_labels(app: &AppHandle) -> Vec<String> {
    let mut labels: Vec<String> = app
        .webview_windows()
        .into_keys()
        .filter(|label| widget_id_from_label(label).is_some())
        .collect();
    labels.sort();
    labels
}

/// 小工具頁面的 App URL（design.md D10：`widget.html?w=<id>`）。`WebviewUrl::App` 以
/// `Url::join` 接在 app 基底 URL 之後（`tauri-2.12.0/src/manager/webview.rs`），query string
/// 會被保留；task 2.7 的 `self_test_ipc` 已以 `test-ipc.html?channel=…` 實測可用。
pub fn widget_url(widget_id: &str) -> String {
    format!("widget.html?w={widget_id}")
}

/// 核心狀態（Tauri managed state）：五個 IPC 指令與排程推播都透過這裡存取／變更設定與通道
/// 註冊表。`settings`／`registry`／`edit_mode` 各自獨立上鎖（不共用一把鎖）——三者的讀寫
/// 情境彼此獨立（例如排程執行緒輪詢通道時，設定視窗仍可同時讀設定），沒有必要互相阻塞。
pub struct AppState {
    pub settings: Mutex<Settings>,
    /// 設定檔路徑，啟動時決定（[`crate::settings::default_settings_path`]），全程不變，故不需
    /// 上鎖。
    pub settings_path: PathBuf,
    pub registry: Mutex<ChannelRegistry>,
    /// 「編輯版面」目前是否啟用（specs/widget-host-windows「編輯版面」）。
    pub edit_mode: Mutex<bool>,
    /// `data` 推播的訂閱表（[`subscribe_data`] 登記、[`poll_and_notify`] 投遞），見
    /// [`DataSubscribers`]。
    pub data_subscribers: Mutex<DataSubscribers>,
    /// 視窗工廠的串行化鎖（[`sync_widget_windows`]）：啟動與連續多次 `update_settings` 可能
    /// 在不同執行緒同時觸發同步，不串行化會重複建立同一個 label 或在建立途中被關閉。
    pub window_sync: Mutex<()>,
    /// 已送出 `destroy()`、但尚未收到 `WindowEvent::Destroyed` 的小工具視窗 label（fix round 1，
    /// Codex 3.1）。`destroy()` 只把銷毀排入事件迴圈，視窗在 `Destroyed` 之前仍留在
    /// `app.webview_windows()` 裡；[`plan_window_changes`] 據此不把它們當成「已存在」，同一個
    /// 小工具要重開時延後到 [`on_window_destroyed`] 再同步一次（見該函式文件）。
    pub closing_windows: Mutex<HashSet<String>>,
    /// fix F3（review task-3.1-fixA-opus.md [low]）：[`create_widget_window`] 建立後設定失敗而
    /// `destroy()` 的視窗 label（同時也在 [`AppState::closing_windows`]）。Destroyed 後與一般關閉
    /// 一樣清掉標記，但不補同步（見 [`finish_closing`]）。
    pub abandoned_windows: Mutex<HashSet<String>>,
    /// 目前啟用中的暫停原因集合（task 5.1、design.md D12「暫停原因集合」，見本檔模組文件）。
    pub pause_reasons: Mutex<HashSet<PauseReason>>,
    /// 電池／省電的原始訊號快取（task 5.5，見 [`RawPowerSignals`] 文件）。
    pub raw_power: Mutex<RawPowerSignals>,
    /// WebView2 故障復原的共用狀態（task 5.6：去重、看門狗、重建次數上限，見
    /// [`crate::recovery`]）。
    pub recovery: RecoveryState,
    /// task 7.2：每個小工具視窗目前套用的倍率與隱藏原因，見 [`WidgetRuntime`]。
    pub widget_runtime: Mutex<WidgetRuntime>,
    /// fix F2（review 5.4）：開機自啟登錄的寫入端，見 [`AutostartRegistry`]。預設
    /// [`NoopAutostartRegistry`]；正式宿主由 `main.rs` 以 [`AppState::with_autostart_registry`]
    /// 換成真正的登錄。
    pub autostart_registry: Box<dyn AutostartRegistry>,
    /// task 7.5：進行中的編輯版面拖曳（[`DragSession`]）；fix F6b 起連同「等待延後判定」的
    /// 拖曳結束一起由 [`DragTracker`] 管理。同一時間只可能有一個進行中（系統的移動迴圈是模態的）。
    pub drag: Mutex<DragTracker>,
}

/// 小工具視窗的執行期狀態（task 7.2；行程存活期間的快取，不落地）。鍵一律是視窗 label
/// （`w-<id>` 或重建後的 `w-<id>-r<n>`）：重建的新視窗是新頁面，會重新回報內容，不沿用舊
/// label 的狀態。視窗關閉或重建時只清除 [`WidgetRuntime::presented`]（同 label 重開時必須重新
/// 等 present，見 [`on_window_destroyed`]）；其餘孤兒項目沒有對應視窗會被查到，不影響正確性。
#[derive(Debug, Default)]
pub struct WidgetRuntime {
    /// 最後一次套用的倍率（[`crate::layout::content_zoom`]；寫入點：[`relayout_all_widgets`]、
    /// [`create_widget_window`]、拖曳中跨螢幕的 [`record_drag_move_zoom`]）。[`report_content`]
    /// 回報有內容時據此重套（頁面 Reload 後 ZoomFactor 可能回到 1，見 [`apply_report_content`]）。
    pub zoom: HashMap<String, f64>,
    /// 頁面回報無內容而隱藏的視窗（design.md D7「無內容」）。
    pub content_empty: HashSet<String>,
    /// 推導結果為「空間不足，暫時隱藏」的視窗（design.md D9）。
    pub no_space_hidden: HashSet<String>,
    /// 視窗工廠已完成 present（[`desktop::present_widget`]）或隱藏準備
    /// （[`desktop::prepare_widget_hidden`]）的視窗（fix F1，review 7.3 M2）。不在這裡的＝建立中，
    /// 位置與樣式都還沒套好，[`sync_widget_visibility_for_edit_mode`] 不得顯示它。
    pub presented: HashSet<String>,
}

impl WidgetRuntime {
    /// task 7.3（design.md D7「無內容」；specs/widget-host-windows「編輯版面時看得到無內容的
    /// 小工具」）：空間不足者永遠不顯示（沒有格子，編輯版面也看不到）；無內容者則只在**不在
    /// 編輯模式**時隱藏——`edit_mode` 為真時，開啟中且有格子的小工具一律顯示（由頁面畫佔位
    /// 外框），讓使用者能移動／調整它。
    pub fn should_show(&self, label: &str, edit_mode: bool) -> bool {
        if self.no_space_hidden.contains(label) {
            return false;
        }
        edit_mode || !self.content_empty.contains(label)
    }

    /// fix F2（F1 範圍外發現，比照 review 7.3 M2）：記錄頁面回報的內容有無，回傳視窗要不要改成
    /// 顯示（`Some(true)`）或隱藏（`Some(false)`）。尚未 present（建立中，位置與樣式都還沒套好）
    /// 的視窗回傳 `None`＝不動，由視窗工廠在 present 完成時依記錄決定（[`Self::mark_presented`]）。
    pub fn record_report_content(
        &mut self,
        label: &str,
        has_content: bool,
        edit_mode: bool,
    ) -> Option<bool> {
        if has_content {
            self.content_empty.remove(label);
        } else {
            self.content_empty.insert(label.to_string());
        }
        self.presented
            .contains(label)
            .then(|| self.should_show(label, edit_mode))
    }

    /// [`apply_report_content`] 的狀態部分：[`Self::record_report_content`] 之後，連同要重套的
    /// 倍率（[`Self::zoom`] 快取——relayout、建立視窗與拖曳中跨螢幕套用時都寫入這裡，是重套的
    /// 單一來源）一起回傳。`None`＝尚未 present，不動。
    pub fn report_content_outcome(
        &mut self,
        label: &str,
        has_content: bool,
        edit_mode: bool,
    ) -> Option<(bool, Option<f64>)> {
        let show = self.record_report_content(label, has_content, edit_mode)?;
        Some((show, self.zoom.get(label).copied()))
    }

    /// 視窗工廠完成 present／隱藏準備：記為已 present，回傳此刻該不該顯示——頁面若在 present
    /// 之前就回報了無內容（[`Self::record_report_content`] 只記錄），present 後要立刻藏回去。
    pub fn mark_presented(&mut self, label: &str, edit_mode: bool) -> bool {
        self.presented.insert(label.to_string());
        self.should_show(label, edit_mode)
    }

    /// fix F1（review 7.3 M2）：切換編輯版面時，`label`（目前是否可見＝`visible`）要不要改變
    /// 顯示狀態：`Some(要顯示與否)`＝要改，`None`＝不動。編輯版面要翻轉的只有「已 present、且
    /// 因無內容而由核心記錄」的視窗——
    /// - 建立中尚未 present（不在 [`WidgetRuntime::presented`]）：不動，由視窗工廠自己決定。
    /// - 不在 [`WidgetRuntime::content_empty`]（有內容）：可見性與編輯版面無關，不動（空間不足
    ///   解除後的重新顯示是 [`relayout_all_widgets`] 的事）。
    /// - 其餘依 [`WidgetRuntime::should_show`]，與目前狀態不同才回傳（空間不足者恆為不顯示）。
    pub fn edit_mode_visibility_change(
        &self,
        label: &str,
        visible: bool,
        edit_mode: bool,
    ) -> Option<bool> {
        if !self.presented.contains(label) || !self.content_empty.contains(label) {
            return None;
        }
        let show = self.should_show(label, edit_mode);
        (show != visible).then_some(show)
    }
}

impl AppState {
    /// 依目前的 `settings.data_dir` 建立通道註冊表（design.md D5：`build_default_registry`）。
    ///
    /// dynamic-wallpaper task 4.6：同時開啟 `wallpaper` 衍生通道，主題設定檔＝`settings_path` 同一個
    /// 資料夾的 `wallpaper-config.json`（正式宿主即 `wallpaper_config::default_config_path`；測試用暫存
    /// 的 `settings_path` 時跟著落在暫存資料夾）。監看只讀不寫，缺檔時以內建預設代替。
    pub fn new(settings: Settings, settings_path: PathBuf) -> Self {
        let mut registry = data::build_default_registry(&settings.data_dir);
        registry.watch_wallpaper_config(
            &settings_path.with_file_name(crate::wallpaper_config::CONFIG_FILE_NAME),
        );
        Self {
            settings: Mutex::new(settings),
            settings_path,
            registry: Mutex::new(registry),
            edit_mode: Mutex::new(false),
            data_subscribers: Mutex::new(HashMap::new()),
            window_sync: Mutex::new(()),
            closing_windows: Mutex::new(HashSet::new()),
            abandoned_windows: Mutex::new(HashSet::new()),
            pause_reasons: Mutex::new(HashSet::new()),
            raw_power: Mutex::new(RawPowerSignals::default()),
            recovery: RecoveryState::default(),
            widget_runtime: Mutex::new(WidgetRuntime::default()),
            drag: Mutex::new(DragTracker::default()),
            autostart_registry: Box::new(NoopAutostartRegistry),
        }
    }

    /// 換掉開機自啟登錄的寫入端（正式宿主用，見 [`AppState::autostart_registry`]）。
    pub fn with_autostart_registry(mut self, registry: Box<dyn AutostartRegistry>) -> Self {
        self.autostart_registry = registry;
        self
    }

    /// dynamic-wallpaper task 4.7a：把啟動時已載入的主題設定檔結果（Primary 的
    /// `wallpaper_config::load_for_startup`、Secondary 的 `reload`）交給 `wallpaper` 通道的監看，
    /// 避免第一輪輪詢重讀同一份檔、重記同樣的警告（[`ChannelRegistry::watch_wallpaper_config_primed`]）。
    /// 監看的路徑與 [`AppState::new`] 相同。
    pub fn with_wallpaper_config(self, startup: &crate::wallpaper_config::WallpaperConfig) -> Self {
        let path = self
            .settings_path
            .with_file_name(crate::wallpaper_config::CONFIG_FILE_NAME);
        self.registry
            .lock()
            .expect("registry mutex poisoned")
            .watch_wallpaper_config_primed(&path, startup);
        self
    }
}

/// `get_snapshot` 的回應形狀（design.md D4）：`{ channel, status: "ok"|"empty", data, meta }`，
/// `status`／`data`／`meta` 由 [`ChannelRegistry::snapshot`] 回傳的 `Option` 直接推導——
/// `Some` 是 `"ok"`、`data`／`meta` 皆有值；`None`（該通道從未取得有效快照，規格書「尚無
/// 資料」）是 `"empty"`、`data`／`meta` 皆為 `null`。`meta` 的 camelCase 由
/// [`crate::data::SnapshotMeta`] 的 `#[serde(rename_all = "camelCase")]` 提供。
///
/// task 4.1 fix round 3：`generation` 為通道註冊表世代（[`ChannelRegistry::generation`]），
/// `"ok"` 與 `"empty"` 都帶——empty 也是「某個世代的狀態」，資料目錄切換後的 empty 比切換前
/// 的任何快照都新，頁面據此以 (generation, loadedAt) 比較新舊（`host/ui/widget.html`）。
#[derive(Debug, Clone, Serialize)]
pub struct SnapshotResponse {
    pub channel: String,
    pub status: &'static str,
    pub data: Option<Value>,
    pub meta: Option<SnapshotMeta>,
    pub generation: u64,
}

/// 由註冊表目前狀態組出 [`SnapshotResponse`]。從 [`get_snapshot`] 抽出（指令本身需要 Tauri
/// `State` 無法單元測試），快照與世代在呼叫端持有的同一把鎖下讀出，兩者必定一致。
pub fn snapshot_response(registry: &ChannelRegistry, channel: String) -> SnapshotResponse {
    let generation = registry.generation();
    match registry.snapshot(&channel) {
        Some(snapshot) => SnapshotResponse {
            channel,
            status: "ok",
            data: Some(snapshot.data.clone()),
            meta: Some(snapshot.meta.clone()),
            generation,
        },
        None => SnapshotResponse {
            channel,
            status: "empty",
            data: None,
            meta: None,
            generation,
        },
    }
}

/// [`get_snapshot`] 的可測核心：先以**呼叫端 webview 的 label** 判斷能不能取得這個通道
/// （[`may_access_channel`]：`wallpaper` 只給桌布渲染視窗），不行就回 `Err`（與
/// [`subscribe_data`] 拒絕未訂閱視窗同一種方式；頁面端 `invoke` 會 reject），可以才組
/// [`SnapshotResponse`]。
pub fn snapshot_for_label(
    registry: &ChannelRegistry,
    label: &str,
    channel: String,
) -> Result<SnapshotResponse, String> {
    if !may_access_channel(label, &channel) {
        log::warn!("拒絕視窗 {label} 查詢通道 {channel}（只給桌布渲染視窗）");
        return Err(format!("視窗 {label} 不能查詢通道 {channel}"));
    }
    Ok(snapshot_response(registry, channel))
}

/// design.md D4：任何小工具開啟或重新載入時，SHALL 能立即取得其訂閱通道的當前快照
/// （specs/widget-data-feed「小工具取得當前快照」）。
///
/// dynamic-wallpaper task 4.6：多帶呼叫端 `webview`（Tauri 注入，頁面不傳），`wallpaper` 通道只回應
/// 桌布渲染視窗、其他 webview 一律 `Err`（[`snapshot_for_label`]）；其餘通道行為不變。
#[tauri::command]
pub fn get_snapshot(
    webview: Webview,
    channel: String,
    state: State<AppState>,
) -> Result<SnapshotResponse, String> {
    let registry = state.registry.lock().expect("registry mutex poisoned");
    snapshot_for_label(&registry, webview.label(), channel)
}

/// design.md D4：`get_settings() -> Settings`。回傳目前設定的複本，前端據此渲染設定視窗
/// （specs/widget-host-lifecycle「設定持久化」）。`autostart` 欄位以登錄實況為準
/// （[`with_registered_autostart`]，design.md D12）。
#[tauri::command]
pub fn get_settings(state: State<AppState>) -> Settings {
    let settings = state.settings.lock().expect("settings mutex poisoned");
    with_registered_autostart(&settings, state.autostart_registry.as_ref())
}

/// design.md D4：`update_settings(patch)`——合併、存檔、廣播 `settings` 事件（未過濾，送給
/// 所有視窗）。合併失敗（`patch` 帶了型別對不上的值）回傳 `Err`，不落地、不廣播，呼叫端的
/// `Settings` 完全不受影響（[`settings::merge_patch`] 的保證）。
///
/// `data_dir` 若因這次 patch 改變，重建 [`ChannelRegistry`]（specs/widget-data-feed
/// 「資料目錄」：60 秒內生效）：`JsonFileSource` 的路徑在建構當下就固定了，換目錄唯一辦法是
/// 整份重建；重建後舊目錄的快照全部消失（新註冊表從空狀態開始），下一輪排程（至多 30 秒後，
/// `main.rs` 排程間隔）就會把新目錄裡現有的檔案當成「全新出現」讀入並依 D5 推播——重建本身
/// 是同步、立即發生的，唯一的延遲是「等下一次輪詢」，故總延遲上限是排程間隔（30 秒），遠低於
/// spec 要求的 60 秒。
///
/// task 5.4：合併、寫登錄、存檔由 [`commit_settings_patch`] 完成——`autostart` 以登錄實況為
/// 合併基準，使用者實際切換時先寫入／刪除 Run 值；寫入失敗整筆回傳以
/// [`AUTOSTART_WRITE_FAILED`] 開頭的錯誤、不落地也不廣播。回傳與廣播的設定裡 `autostart` 是
/// 寫入後的登錄實況（specs/widget-host-lifecycle「開機自動啟動」）。
///
/// task 7.2（design.md D7）：patch 含任何 `placement` 欄位時整筆拒絕並回傳錯誤
/// （[`settings::merge_user_patch`]）——版面只能經編輯版面放開（[`finish_widget_drag`]）與
/// 開啟小工具找空位（task 7.4）兩條路徑改。
///
/// task 7.4（design.md D7「記錄位置的寫回時機」第 2 點）：合併成功、存檔之前，把本次 patch
/// 由關閉變開啟的小工具交給 [`place_newly_enabled_widgets`]——原位置與該顯示器上其他開啟中
/// 小工具的記錄位置相交才找空位並就地改寫 `merged`，找不到空位就整筆失敗（`merged` 被捨棄，
/// 沿用 `settings_guard` 目前的設定，不落地、不廣播），回傳的錯誤字串直接是
/// 「空間不足，請先調整版面」，供 `settings.html` 顯示並把該開關還原。
///
/// 合併成功後同步呼叫 [`relayout_all_widgets`]（design.md D9「重新推導時機」：開啟或關閉
/// 小工具）：關閉的小工具空出的格子可能讓先前空間不足而隱藏者重新出現。與
/// `desktop::GatekeeperCallbacks::relayout_all` 的呼叫方式相同——同步、在目前執行緒；本指令
/// 是同步指令、在主執行緒執行，[`relayout_all_widgets`] 不呼叫 `WebviewWindowBuilder::build()`，
/// 不受 wry#583 的死鎖限制。新開啟的小工具此時還沒有視窗，由 [`sync_widget_windows`] 建立時
/// 自行推導（[`create_widget_window`]）。
#[tauri::command]
pub fn update_settings(
    patch: Value,
    state: State<AppState>,
    app: AppHandle,
) -> Result<Settings, String> {
    let mut settings_guard = state.settings.lock().expect("settings mutex poisoned");
    let old_data_dir = settings_guard.data_dir.clone();

    let monitors = current_monitors(&app);
    let stored = commit_settings_patch(
        &settings_guard,
        state.autostart_registry.as_ref(),
        &patch,
        |base, merged| place_newly_enabled_widgets(&monitors, base, merged),
        |s| settings::save(&state.settings_path, s).map_err(|e| e.to_string()),
        &current_exe_string,
    )?;
    *settings_guard = stored.clone();
    drop(settings_guard);

    // 回傳與廣播的 `autostart` 是寫入後的登錄實況（design.md D12）。
    let merged = settings_event_payload(&stored, state.autostart_registry.as_ref());

    if merged.data_dir != old_data_dir {
        // fix round 3：`replace_with` 同時遞增註冊表世代，重建後的 empty 回覆與之後的推送都帶
        // 新世代，頁面據此拒收重建前的舊目錄資料（見 [`ChannelRegistry::replace_with`]）。
        let mut registry = state.registry.lock().expect("registry mutex poisoned");
        registry.replace_with(data::build_default_registry(&merged.data_dir));
        // task 2.7 follow-up：重建只遞增世代、不主動推送；已開啟的小工具會停在切換前的資料，
        // 直到（若曾經）新目錄出現有效檔案為止。這裡對已訂閱通道各推一筆新世代的 empty，見
        // push_empty_snapshots_after_rebuild 文件。fix F3（review 2.7 medium）：推送必須在
        // registry 鎖內完成，排程執行緒才不可能在「重建」與「補推 empty」之間先送出同世代的
        // ok、再被 empty 蓋掉。
        push_empty_snapshots_after_rebuild(&state, &registry);
        drop(registry);
    }

    emit_settings(&app, &stored);

    // task 5.5：`pause_rules`（電池／省電）或 `appearance_mode` 可能剛被改掉；用目前已知的
    // 原始訊號立即重算，不必等下一次電源通知（design.md D12「電池／省電依設定」），並重判外觀
    // （design.md D8）。
    reapply_power_pause_rules(&app);

    // task 3.1：小工具開關改了就即時建立／關閉對應視窗（[`sync_widget_windows`] 冪等，未改
    // 開關時什麼都不做）。放到獨立執行緒：非 async 的指令在主執行緒執行，而在主執行緒的指令
    // 內 `WebviewWindowBuilder::build()` 會在 Windows 上死鎖（Tauri 文件對同步指令建立視窗的
    // 警告，wry#583）；獨立執行緒呼叫 `build()` 只是送訊息給事件迴圈並等待，本指令返回後
    // 主執行緒即可處理。
    let handle = app.clone();
    thread::spawn(move || sync_widget_windows(&handle));

    // 開關改變後重新推導（見上方文件）；同步呼叫，理由同上方文件。
    relayout_all_widgets(&app);

    // dynamic-wallpaper task 4.7a：通知桌布協調迴圈（主題、資料目錄、暫停規則都可能變了）。只投遞
    // 通知、不等它（協調迴圈不存在時什麼都不做）。
    crate::wallpaper_coordinator::notify_app(
        &app,
        crate::wallpaper_coordinator::Wake::SettingsChanged,
    );

    Ok(merged)
}

/// dynamic-wallpaper task 4.7a：桌布狀態機要求「把主題改成不接管」（讓位、焦點取消、安全閥）時，
/// 由桌布協調迴圈（非主執行緒）呼叫：在設定鎖內讀改寫並存檔（[`commit_theme_none`]，與
/// [`update_settings`] 同一條提交路徑），放鎖後廣播 `settings`。已經是不接管時什麼都不做。存檔失敗
/// 回傳錯誤（記憶體中仍改成不接管，排程不會再接管）。
pub fn set_wallpaper_theme_none(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let (changed, result) = commit_theme_none(
        &state.settings,
        state.autostart_registry.as_ref(),
        &current_exe_string,
        |s| settings::save(&state.settings_path, s).map_err(|e| e.to_string()),
    );
    if let Some(stored) = changed {
        emit_settings(app, &stored);
    }
    result
}

/// task 6.4（審查 R2b-L1）：主題改「不接管」時存檔失敗（記憶體已改），協調迴圈依退避重試存檔。記憶體
/// 中的主題仍是「不接管」才存（回 `Ok(true)`）；使用者這段期間已選回某個主題時不動（回 `Ok(false)`，
/// 那次選擇已經由 [`update_settings`] 存檔）。
pub fn resave_wallpaper_theme_none(app: &AppHandle) -> Result<bool, String> {
    let state = app.state::<AppState>();
    resave_theme_none(&state.settings, |s| {
        settings::save(&state.settings_path, s).map_err(|e| e.to_string())
    })
}

/// task 6.4（審查 R1 low）：系統匣「結束」等完還原後呼叫——主題不是「不接管」就改並存檔；已是「不接管」
/// （協調迴圈已改，但存檔可能失敗、或迴圈沒在時限內處理要求）就再存一次。結果：下次啟動一律不接管。
pub fn ensure_wallpaper_theme_none_saved(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let (changed, result) = commit_theme_none(
        &state.settings,
        state.autostart_registry.as_ref(),
        &current_exe_string,
        |s| settings::save(&state.settings_path, s).map_err(|e| e.to_string()),
    );
    if let Some(stored) = changed {
        emit_settings(app, &stored);
        return result;
    }
    resave_theme_none(&state.settings, |s| {
        settings::save(&state.settings_path, s).map_err(|e| e.to_string())
    })
    .map(|_| ())
}

/// [`resave_wallpaper_theme_none`] 的本體（可注入存檔）：**持有設定鎖**，主題是「不接管」才以鎖內的
/// 最新設定存檔。
pub(crate) fn resave_theme_none(
    cell: &Mutex<Settings>,
    save: impl FnOnce(&Settings) -> Result<(), String>,
) -> Result<bool, String> {
    let guard = cell.lock().unwrap_or_else(|e| e.into_inner());
    if guard.wallpaper_theme != settings::WallpaperTheme::None {
        return Ok(false);
    }
    save(&guard).map(|()| true)
}

/// [`set_wallpaper_theme_none`] 的本體（可注入登錄與存檔，供單元測試；4.7a 修正輪 1，審查 low 6）：
/// **持有設定鎖**完成「讀最新設定 → 合併 `{"wallpaper_theme":"none"}` → 存檔 → 寫回記憶體」，不會用
/// 舊複本蓋掉同時進行的 [`update_settings`]。提交走 [`commit_settings_patch`]（與 `update_settings`
/// 相同；`autostart` 沒變所以不碰登錄、沒有新開小工具所以不找空位）。存檔失敗時記憶體仍改成不接管
/// 並回傳錯誤。回傳（改變後的設定——沒有改變時 `None`，提交結果）。
pub(crate) fn commit_theme_none(
    cell: &Mutex<Settings>,
    registry: &dyn AutostartRegistry,
    current_exe: &dyn Fn() -> Result<String, String>,
    save: impl FnOnce(&Settings) -> Result<(), String>,
) -> (Option<Settings>, Result<(), String>) {
    let mut guard = cell.lock().unwrap_or_else(|e| e.into_inner());
    if guard.wallpaper_theme == settings::WallpaperTheme::None {
        return (None, Ok(()));
    }
    let committed = commit_settings_patch(
        &guard,
        registry,
        &serde_json::json!({ "wallpaper_theme": "none" }),
        |_, merged| Ok(merged),
        save,
        current_exe,
    );
    let result = match committed {
        Ok(stored) => {
            *guard = stored;
            Ok(())
        }
        Err(e) => {
            guard.wallpaper_theme = settings::WallpaperTheme::None;
            Err(e)
        }
    };
    (Some(guard.clone()), result)
}

/// fix F2（review 5.4）：開機自啟登錄的寫入介面。正式宿主用 `crate::desktop::HkcuRunRegistry`
/// （`HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 底下值名 `fc-host`）；單元測試、
/// `--self-test-ipc` 與 [`AppState::new`] 的預設是 [`NoopAutostartRegistry`]，碰不到使用者
/// 真正的登錄。
///
/// **只有 Run 值**：不再經 `tauri-plugin-autostart`——它底下的 `auto-launch-0.5.0`
/// `enable()` 每次都把 `...\Explorer\StartupApproved\Run\<name>` 改寫成「啟用」
/// （`TASK_MANAGER_OVERRIDE_ENABLED_VALUE`），會撤銷使用者在工作管理員／「設定 > 啟動應用程式」
/// 做的停用；Run 值的 exe 路徑也沒加引號。本宿主永不主動改寫 `StartupApproved`。
pub trait AutostartRegistry: Send + Sync {
    /// 寫入（覆寫）Run 值為 `command`。
    fn set_run_value(&self, command: &str) -> Result<(), String>;
    /// 刪除 Run 值；本來就不存在視為成功。
    fn delete_run_value(&self) -> Result<(), String>;
    /// Run 值目前存不存在（不論內容指向哪個 exe）。
    fn run_value_registered(&self) -> Result<bool, String>;
}

/// 開機自啟顯示以登錄實況為準（design.md D12）：回傳 `settings` 的複本，`autostart` 換成 Run 值
/// 目前存不存在；讀不到登錄（含 [`NoopAutostartRegistry`]）時保留設定檔的值。
///
/// 登錄由安裝檔負責寫入，設定檔預設的 `true` 只代表「預期已由安裝檔登錄」——沒有安裝檔的開發
/// 建置會是「設定檔 true、登錄沒值」。設定視窗若照設定檔顯示「開」，使用者以為會自啟、其實不會，
/// 而且要先關再開才寫得進去；故 [`get_settings`]、[`update_settings`] 的回傳與所有 `settings`
/// 事件（經 [`emit_settings`]／[`settings_event_payload`]）一律是這個值。
pub fn with_registered_autostart(
    settings: &Settings,
    registry: &dyn AutostartRegistry,
) -> Settings {
    let mut shown = settings.clone();
    match registry.run_value_registered() {
        Ok(registered) => shown.autostart = registered,
        Err(err) => log::debug!("autostart：讀不到開機自啟登錄，沿用設定檔的值：{err}"),
    }
    shown
}

/// [`update_settings`] 的合併步驟：先以 [`with_registered_autostart`] 把 `current` 的
/// `autostart` 換成登錄實況（＝使用者在設定視窗看到的值），再合併 `patch`。回傳
/// `(合併前的基準, 合併結果)`；呼叫端以基準的 `autostart` 當「前值」交給
/// [`autostart_sync_target`]——只有 patch 把開關切到與登錄實況不同的值才會寫登錄，設定檔與
/// 登錄不一致本身不會觸發寫入。
pub fn merge_settings_patch(
    current: &Settings,
    registry: &dyn AutostartRegistry,
    patch: &Value,
) -> Result<(Settings, Settings), String> {
    let base = with_registered_autostart(current, registry);
    let merged = settings::merge_user_patch(&base, patch)?;
    Ok((base, merged))
}

/// 不做任何事的 [`AutostartRegistry`]（[`AppState::new`] 的預設）。
pub struct NoopAutostartRegistry;

impl AutostartRegistry for NoopAutostartRegistry {
    fn set_run_value(&self, command: &str) -> Result<(), String> {
        log::debug!("autostart：未接上系統登錄（測試／驗收建置），略過寫入 {command}");
        Ok(())
    }
    fn delete_run_value(&self) -> Result<(), String> {
        log::debug!("autostart：未接上系統登錄（測試／驗收建置），略過刪除");
        Ok(())
    }
    fn run_value_registered(&self) -> Result<bool, String> {
        Err("未接上系統登錄".to_string())
    }
}

/// 什麼時機在考慮同步開機自啟登錄（[`autostart_sync_target`]）。
///
/// 使用者決定（2026-10-01）：開機自啟登錄由安裝檔寫入與移除，宿主啟動（含設定檔首次建立）
/// 一律不寫，故只有「設定更新」這一種時機。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutostartTrigger {
    /// [`update_settings`] 合併成功；`previous` 是合併前、以登錄實況為準的 `autostart`
    /// （[`merge_settings_patch`] 的基準）。
    SettingsUpdated { previous: bool },
}

/// fix F2（review 5.4 medium＋low）＋使用者決定（2026-10-01）：要不要同步登錄、同步成什麼。
/// 只有使用者在設定裡**實際切換**了「登入時自動啟動」才寫；宿主啟動與其他設定欄位的修改一律
/// 不碰登錄——驗收腳本以暫存設定啟動的行程、或使用者在系統層的停用都不會被無聲覆寫。
pub fn autostart_sync_target(trigger: AutostartTrigger, current: bool) -> Option<bool> {
    match trigger {
        AutostartTrigger::SettingsUpdated { previous } => (previous != current).then_some(current),
    }
}

/// Run 值內容：exe 路徑一律加引號（路徑含空白時，未加引號的命令列會被 Windows 依序嘗試
/// `C:\Users\John.exe` 等前綴），後接 `--autostart`（`crate::tray::is_automated_relaunch` 據此
/// 認出開機自啟）。
pub fn autostart_command(exe: &str) -> String {
    format!("\"{exe}\" --autostart")
}

/// 依 `enabled` 寫入或刪除 Run 值（只動 Run 值，見 [`AutostartRegistry`]）。
pub fn apply_autostart(
    registry: &dyn AutostartRegistry,
    enabled: bool,
    exe: &str,
) -> Result<(), String> {
    if enabled {
        registry.set_run_value(&autostart_command(exe))
    } else {
        registry.delete_run_value()
    }
}

/// 寫入／刪除開機自啟登錄失敗時，[`update_settings`] 回傳錯誤字串的開頭（`settings.html`
/// 顯示「套用失敗（自動啟動）：…」並把開關還原）。
pub const AUTOSTART_WRITE_FAILED: &str = "無法寫入開機自動啟動設定";

/// task 5.4／design.md D12：把「登入時自動啟動」這個使用者意願在 [`autostart_sync_target`]
/// 判定需要時同步到系統的開機自啟登錄。唯一呼叫端：[`commit_settings_patch`]（使用者實際切換
/// 開關時）；宿主啟動不呼叫（登錄歸安裝檔）。
///
/// 失敗（取不到執行檔路徑、或登錄寫入被拒，例如企業原則鎖住該機碼）回傳以
/// [`AUTOSTART_WRITE_FAILED`] 開頭的錯誤——呼叫端整筆設定不落地，設定視窗把開關還原。
pub fn sync_autostart(
    registry: &dyn AutostartRegistry,
    trigger: AutostartTrigger,
    current: bool,
    current_exe: &dyn Fn() -> Result<String, String>,
) -> Result<(), String> {
    let Some(enabled) = autostart_sync_target(trigger, current) else {
        return Ok(());
    };
    match current_exe().and_then(|exe| apply_autostart(registry, enabled, &exe)) {
        Ok(()) => {
            log::info!("autostart：已同步開機自啟登錄 enabled={enabled}（{trigger:?}）");
            Ok(())
        }
        Err(err) => {
            log::error!("autostart：同步系統登錄狀態失敗（enabled={enabled}）：{err}");
            Err(format!("{AUTOSTART_WRITE_FAILED}：{err}"))
        }
    }
}

/// 目前執行檔的路徑字串（[`sync_autostart`] 寫入 Run 值用）。
fn current_exe_string() -> Result<String, String> {
    std::env::current_exe()
        .map(|path| path.display().to_string())
        .map_err(|err| format!("取得執行檔路徑失敗：{err}"))
}

/// [`update_settings`] 除了廣播以外的提交步驟（可注入登錄、版面放置與存檔，供單元測試）：
///
/// 1. [`merge_settings_patch`]：以登錄實況為基準合併；
/// 2. `place`：新開啟小工具找空位（[`place_newly_enabled_widgets`]）；
/// 3. 使用者實際切換開關時寫登錄（[`sync_autostart`]）——**在存檔之前**：寫入失敗就整筆回傳
///    錯誤、不存檔，設定檔、登錄與畫面都維持原狀；
/// 4. 存檔；存檔失敗時把第 3 步改過的登錄改回原值（盡力而為，失敗只記錄），再回傳存檔錯誤。
///
/// 回傳要放進記憶體的設定（其 `autostart` 與寫入後的登錄一致）。
pub fn commit_settings_patch(
    current: &Settings,
    registry: &dyn AutostartRegistry,
    patch: &Value,
    place: impl FnOnce(&Settings, Settings) -> Result<Settings, String>,
    save: impl FnOnce(&Settings) -> Result<(), String>,
    current_exe: &dyn Fn() -> Result<String, String>,
) -> Result<Settings, String> {
    let (base, merged) = merge_settings_patch(current, registry, patch)?;
    let merged = place(&base, merged)?;
    sync_autostart(
        registry,
        AutostartTrigger::SettingsUpdated {
            previous: base.autostart,
        },
        merged.autostart,
        current_exe,
    )?;
    if let Err(err) = save(&merged) {
        // 存檔失敗：把剛改過的登錄改回原值（沒改過時 `sync_autostart` 不做事）。
        let undo = sync_autostart(
            registry,
            AutostartTrigger::SettingsUpdated {
                previous: merged.autostart,
            },
            base.autostart,
            current_exe,
        );
        if let Err(undo_err) = undo {
            log::error!("autostart：存檔失敗後還原開機自啟登錄也失敗：{undo_err}");
        }
        return Err(err);
    }
    Ok(merged)
}

/// `settings` 事件名稱；只有 [`emit_settings`] 可以用它發事件。
pub const SETTINGS_EVENT: &str = "settings";

/// `settings` 事件的 payload：`autostart` 換成登錄實況（design.md D12「顯示以登錄實況為準」），
/// 與 [`get_settings`] 一致。純函式。
pub fn settings_event_payload(settings: &Settings, registry: &dyn AutostartRegistry) -> Settings {
    with_registered_autostart(settings, registry)
}

/// 廣播 `settings` 事件（未過濾，送給所有視窗）的唯一入口：payload 一律經
/// [`settings_event_payload`]，任何路徑（設定更新、編輯版面切換、拖曳放開）廣播的
/// `autostart` 都是登錄實況（`startup_never_syncs_autostart_registry` 旁的靜態測試守住）。
///
/// dynamic-wallpaper task 5.2：所有設定變更都經這裡廣播，故也是「主題可能變了」的收斂點——
/// 同時要求刷新系統匣與設定視窗圖示（[`crate::app_icon::request_refresh`] 立即返回，實際主題在
/// 套用前重讀，見該模組文件）。
fn emit_settings(app: &AppHandle, settings: &Settings) {
    let state = app.state::<AppState>();
    let payload = settings_event_payload(settings, state.autostart_registry.as_ref());
    let _ = app.emit(SETTINGS_EVENT, &payload);
    crate::app_icon::request_refresh(app);
}

/// design.md D4：`set_edit_mode(bool)`——存旗標、廣播 `edit-mode` 事件（未過濾，送給所有
/// 視窗），並依 task 5.3 把「版面鎖定」同步成「不在編輯模式」（specs/widget-host-windows
/// 「編輯版面」：系統匣「編輯版面」是目前唯一的解鎖／上鎖入口，沒有另外的鎖定開關；個別小工具
/// 的位置在每次拖曳結束（[`finish_widget_drag`]）時已經個別存過，這裡只補上鎖定旗標本身）。
///
/// task 7.3：切換完 `edit_mode` 旗標後呼叫 [`sync_widget_visibility_for_edit_mode`]，依新值
/// 重新對帳每個小工具視窗目前的顯示狀態（design.md D7「無內容」）。
///
/// fix F2（review 5.3 low）：鎖定旗標存檔失敗時不切換、回傳錯誤（[`switch_edit_mode`]），
/// `edit_mode` 與 `layout_locked` 不會失步；先廣播 `settings` 再廣播 `edit-mode`。
#[tauri::command]
pub fn set_edit_mode(enabled: bool, state: State<AppState>, app: AppHandle) -> Result<(), String> {
    let changed = {
        let mut settings_guard = state.settings.lock().expect("settings mutex poisoned");
        let mut edit_mode_guard = state.edit_mode.lock().expect("edit_mode mutex poisoned");
        let path = state.settings_path.clone();
        switch_edit_mode(&mut edit_mode_guard, &mut settings_guard, enabled, |s| {
            settings::save(&path, s).map_err(|e| e.to_string())
        })
    };
    let changed = match changed {
        Ok(changed) => changed,
        Err(err) => {
            log::error!("編輯版面：切換為 {enabled} 失敗，維持原狀：{err}");
            return Err(err);
        }
    };
    if let Some(merged) = changed {
        emit_settings(&app, &merged);
    }
    let _ = app.emit("edit-mode", enabled);
    sync_widget_visibility_for_edit_mode(&app, enabled);
    sync_widget_resizable(&app, resizable_now(&state));
    // widget-adaptive-zoom-and-grid task 5.2：進入編輯版面顯示格線、離開時移除（本指令在主執行緒，
    // `sync_grid_overlay` 經 `run_on_main_thread` 當場同步執行）。
    sync_grid_overlay(&app);
    Ok(())
}

/// fix F1（review 7.3 M3）：`get_edit_mode() -> bool`，比照 [`get_pause`]。`edit-mode` 事件只在
/// 切換時廣播一次；頁面啟動或重新載入（含 WebView2 故障復原的 `Reload()`／重建）時以本指令
/// 取得目前狀態，順序同其他查詢：先 `listen('edit-mode')` 再查，查詢期間收到的事件優先
/// （`host/ui/widget.html`）。否則編輯版面期間重建的無內容小工具會被核心顯示（依核心的
/// `edit_mode`），頁面卻不畫佔位外框、不掛拖曳區。
#[tauri::command]
pub fn get_edit_mode(state: State<AppState>) -> bool {
    edit_mode_of(&state)
}

/// [`get_edit_mode`] 的本體（單元測試直接呼叫）。
fn edit_mode_of(state: &AppState) -> bool {
    *state.edit_mode.lock().expect("edit_mode mutex poisoned")
}

/// task 7.6：小工具視窗此刻是否該可調整大小（`WS_SIZEBOX`）＝在編輯版面且版面未鎖定（與前端
/// `applyEditAffordance` 的 `draggable` 同一個條件）。
fn resizable_now(state: &AppState) -> bool {
    let edit_mode = *state.edit_mode.lock().expect("edit_mode mutex poisoned");
    let locked = state
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .layout_locked;
    edit_mode && !locked
}

/// task 7.6（design.md D7「調整大小的機制」退路）：進出編輯版面時對每個小工具視窗切換
/// `WS_SIZEBOX`（[`desktop::set_widget_resizable_on_main_thread`]；不經 tao 的 `set_resizable`）。
/// 探針實測沒有這個樣式時 `startResizeDragging` 進不了尺寸迴圈；離開編輯版面時拿掉，鎖定時
/// 即使有人呼叫 `startResizeDragging` 也拉不動。
fn sync_widget_resizable(app: &AppHandle, resizable: bool) {
    for (label, window) in app.webview_windows() {
        if widget_id_from_label(&label).is_none() {
            continue;
        }
        if let Ok(hwnd) = window.hwnd() {
            desktop::set_widget_resizable_on_main_thread(app, hwnd, move || resizable);
        }
    }
}

/// task 7.3（design.md D7「無內容」；specs/widget-host-windows「編輯版面時看得到無內容的小
/// 工具」）：`set_edit_mode` 切換旗標後呼叫，把每個目前存在的小工具視窗的顯示狀態對齊
/// [`WidgetRuntime::should_show`] 依新 `edit_mode` 值算出的結果——
/// - 進入編輯模式：因無內容而隱藏者補顯示（沒有格子的「空間不足」隱藏者不受影響，
///   `should_show` 對它們恆回傳 `false`）。
/// - 離開編輯模式：剛才因編輯模式而顯示、實際仍無內容者藏回去。
///
/// 只改變視窗的顯示／隱藏，不動矩形或倍率（矩形在進入編輯模式前就已經是推導結果，這裡不需要
/// 重新推導）。找不到 HWND 的視窗（理論上不會發生）靜默略過。
///
/// fix F1（review 7.3 M2）：同步機制比照 [`relayout_all_widgets`]——不取 `window_sync`（本函式
/// 在主執行緒，工作執行緒持 `window_sync` 時會同步等主執行緒建立視窗，主執行緒再等那把鎖會
/// 死鎖），改以核心自己的記錄決定能動誰：只翻轉 [`WidgetRuntime::edit_mode_visibility_change`]
/// 認可的視窗（已 present 且因無內容而隱藏／顯示者），建立中尚未 present 的視窗與空間不足者
/// 一律不動，維持 [`relayout_all_widgets`] 文件寫的「不對建立中視窗誤呼叫 `show_at_bottom`」。
fn sync_widget_visibility_for_edit_mode(app: &AppHandle, edit_mode: bool) {
    let state = app.state::<AppState>();
    for (label, window) in app.webview_windows() {
        if widget_id_from_label(&label).is_none() {
            continue;
        }
        let Ok(hwnd) = window.hwnd() else {
            continue;
        };
        let visible = desktop::is_widget_visible(hwnd);
        let change = state
            .widget_runtime
            .lock()
            .expect("widget_runtime mutex poisoned")
            .edit_mode_visibility_change(&label, visible, edit_mode);
        let Some(show) = change else {
            continue;
        };
        if show {
            if let Err(err) = desktop::show_at_bottom(hwnd) {
                log::error!("編輯版面：顯示小工具失敗（{label}）：{err}");
            }
        } else if let Err(err) = desktop::hide_widget(hwnd) {
            log::warn!("編輯版面：隱藏小工具失敗（{label}）：{err}");
        }
    }
}

/// fix F2（review 5.3 low）：拖曳結束時能不能把放開位置存成記錄位置——與
/// [`begin_widget_drag`] 建立拖曳的條件一致：編輯版面中且版面未鎖定。兩個旗標失步（例如
/// `layout_locked=false` 但不在編輯版面）時一律不存。純函式。
fn drag_end_may_save(layout_locked: bool, edit_mode: bool) -> bool {
    edit_mode && !layout_locked
}

/// fix F2（review 5.3 low）：切換編輯版面的狀態轉移（純邏輯，存檔由 `save` 注入）。
///
/// 先把 `layout_locked`（＝不在編輯版面）存檔，**成功後**才一起改 `settings` 與 `edit_mode`；
/// 存檔失敗時兩者都不動並回傳錯誤——不會出現「已進入編輯版面但仍鎖定」或「已離開但記憶體／
/// 磁碟仍是解鎖」的失步。鎖定旗標本來就是目標值時不存檔，直接切換 `edit_mode`。
/// 回傳 `Some(新設定)`＝設定有變、呼叫端要廣播 `settings`。
fn switch_edit_mode(
    edit_mode: &mut bool,
    settings: &mut Settings,
    enabled: bool,
    save: impl FnOnce(&Settings) -> Result<(), String>,
) -> Result<Option<Settings>, String> {
    let locked = !enabled;
    let changed = if settings.layout_locked == locked {
        None
    } else {
        let merged =
            settings::merge_patch(settings, &serde_json::json!({ "layout_locked": locked }))
                .map_err(|e| format!("更新 layout_locked 失敗：{e}"))?;
        save(&merged).map_err(|e| format!("layout_locked 存檔失敗：{e}"))?;
        *settings = merged.clone();
        Some(merged)
    };
    *edit_mode = enabled;
    Ok(changed)
}

// ── 暫停原因集合（task 5.1／5.5；design.md D12「自動暫停」）─────────────────────────

/// 暫停原因（design.md D12；本檔模組文件「task 5.1 補上暫停原因集合」一節）。只是「這個原因
/// 目前是否成立」的旗標集合成員，不含判定邏輯本身——判定邏輯（輪詢
/// `SHQueryUserNotificationState`、鎖定通知、電源設定通知等，task 5.5）屬於
/// `crate::desktop`（見其「D12：自動暫停偵測」一節），本檔只提供集合、業務判斷（電池／省電
/// 是否真的要暫停取決於使用者設定）與廣播機制。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PauseReason {
    /// 使用者從系統匣選單手動暫停／繼續（task 5.1，見 `crate::tray`）。
    Manual,
    /// 工作階段鎖定（task 5.5：`WTSRegisterSessionNotification`）。
    Locked,
    /// `SHQueryUserNotificationState` 判定為忙碌（全螢幕應用程式／簡報設定／獨占全螢幕
    /// Direct3D 三者之一，task 5.5：每 5 秒輪詢，見 `crate::desktop::quns_indicates_busy`）。
    SystemBusy,
    /// 顯示器關閉（task 5.5：`RegisterPowerSettingNotification(GUID_CONSOLE_DISPLAY_STATE)`）。
    DisplayOff,
    /// 使用電池（task 5.5；`settings::PauseRules::pause_on_battery` 開啟才會真的觸發，見
    /// [`apply_battery_pause_rule`]）。
    Battery,
    /// 省電模式（Battery Saver，task 5.5；`settings::PauseRules::pause_on_power_saver`
    /// 開啟才會真的觸發，見 [`apply_power_saver_pause_rule`]）。
    PowerSaver,
}

/// `pause` 事件的 `reason` 挑選優先序（design.md D4：payload 只有單一 `reason`，多個原因同時
/// 成立時取這份清單中最前面那個）；也是系統匣選單文字（task 5.5：[`top_other_reason`]）排除
/// `Manual` 之後挑選「其他原因」的依據。順序取捨：鎖定最具決定性（使用者離開電腦，其餘原因
/// 是否成立已不重要）排最前；其次是使用者正在做的事（全螢幕／簡報）；顯示器關閉次之；電池／
/// 省電是較邊緣的可選規則排最後；`Manual` 排在最後——使用者手動暫停時，若同時還有其他更具體
/// 的原因，選單文字（見 `crate::tray::pause_toggle_label`）應該優先講那個原因，而不是「手動」
/// 這個使用者自己剛做的動作。
const PAUSE_REASON_PRIORITY: [PauseReason; 6] = [
    PauseReason::Locked,
    PauseReason::SystemBusy,
    PauseReason::DisplayOff,
    PauseReason::Battery,
    PauseReason::PowerSaver,
    PauseReason::Manual,
];

/// 依目前啟用中的原因集合算出 `(paused, reason)`（純函式，見 `pause_reason_tests`）：
/// `paused` 為集合非空；`reason` 是 [`PAUSE_REASON_PRIORITY`] 中第一個「仍在集合裡」的成員，
/// 集合為空時是 `None`。
fn resolve_pause(reasons: &HashSet<PauseReason>) -> (bool, Option<PauseReason>) {
    let paused = !reasons.is_empty();
    let reason = PAUSE_REASON_PRIORITY
        .iter()
        .find(|r| reasons.contains(r))
        .copied();
    (paused, reason)
}

/// 排除 [`PauseReason::Manual`] 後，目前優先序最高的「其他」暫停原因（task 5.5；純函式，見
/// `pause_reason_tests`）。供系統匣「暫停／繼續」選單文字使用（`crate::tray`）：手動暫停只切
/// `Manual` 這一個原因的成員資格（見 [`set_pause_reason`] 呼叫端 `crate::tray::
/// toggle_manual_pause`），若其他原因仍在，點「繼續」之後整體仍會維持暫停——選單文字需要知道
/// 「除了手動之外還有沒有其他原因」，這正是本函式要回答的問題。
pub fn top_other_reason(reasons: &HashSet<PauseReason>) -> Option<PauseReason> {
    PAUSE_REASON_PRIORITY
        .iter()
        .find(|r| **r != PauseReason::Manual && reasons.contains(r))
        .copied()
}

/// [`PauseReason`] 的中文顯示文字（task 5.5；`crate::tray::pause_toggle_label` 組合選單文字
/// 用）。`Manual` 不會出現在這裡的呼叫端（[`top_other_reason`] 已排除），但仍提供完整對照以防
/// 未來需要。
pub fn reason_label(reason: PauseReason) -> &'static str {
    match reason {
        PauseReason::Manual => "手動暫停",
        PauseReason::Locked => "鎖定",
        PauseReason::SystemBusy => "全螢幕或忙碌",
        PauseReason::DisplayOff => "顯示器關閉",
        PauseReason::Battery => "使用電池",
        PauseReason::PowerSaver => "省電模式",
    }
}

/// `pause` 事件 payload（design.md D4：`{ paused, reason }`）。`reason` 在 `paused` 為
/// `false` 時序列化為 `null`。
#[derive(Debug, Clone, Serialize)]
pub struct PauseEvent {
    pub paused: bool,
    pub reason: Option<PauseReason>,
}

/// 依暫停原因集合組出 `pause` 事件／`get_pause` 回應（純函式，見 `pause_reason_tests`）。
fn pause_event_for(reasons: &HashSet<PauseReason>) -> PauseEvent {
    let (paused, reason) = resolve_pause(reasons);
    PauseEvent { paused, reason }
}

/// design.md D4：`get_pause() -> { paused, reason }`（task 5.6 fix round 1）。`pause` 事件只在
/// 狀態變化時廣播；頁面啟動或重新載入（含 WebView2 故障復原的 `Reload()`／重建）時以本指令
/// 取得目前狀態，順序同其他查詢：先 `listen('pause')` 再查，查詢期間收到的事件優先
/// （`host/ui/widget.html`）。
#[tauri::command]
pub fn get_pause(state: State<AppState>) -> PauseEvent {
    let reasons = state
        .pause_reasons
        .lock()
        .expect("pause_reasons mutex poisoned");
    pause_event_for(&reasons)
}

/// fix F2（review 5.5 low）：把 `reason` 設成 `active`；集合**實際改變**時回傳新的事件，沒有變化
/// （重複設定、移除本來就不在的原因）回傳 `None`（design.md D4「`pause` 事件只在變化時廣播」）。
pub(crate) fn apply_pause_reason(
    reasons: &mut HashSet<PauseReason>,
    reason: PauseReason,
    active: bool,
) -> Option<PauseEvent> {
    let changed = if active {
        reasons.insert(reason)
    } else {
        reasons.remove(&reason)
    };
    changed.then(|| pause_event_for(reasons))
}

/// 變更某個暫停原因的啟用狀態，重算整體 `paused`（[`resolve_pause`]）並廣播 `pause` 事件
/// （未過濾，送給所有視窗，比照 [`set_edit_mode`] 的 `edit-mode` 廣播方式；
/// specs/widget-host-lifecycle「自動暫停」）。回傳變更後的整體 `paused`。集合沒有實際改變時
/// 什麼都不送（fix F2，review 5.5 low：design.md D4「只在變化時廣播」）。
///
/// task 5.5：所有暫停原因的變更（手動、鎖定、忙碌、顯示器、電池、省電）最終都經過這個函式，
/// 是唯一的收斂點——因此系統匣選單重整（`crate::tray::refresh`）也放在這裡呼叫一次，而不是
/// 讓每個呼叫端（`crate::tray` 的手動切換、`main.rs` 接的自動訊號、
/// [`apply_battery_pause_rule`]／[`apply_power_saver_pause_rule`]）各自記得呼叫一次：任何一處
/// 漏叫就會讓選單文字（`pause_toggle_label`）落後於實際暫停狀態。`widgets` 依賴 `tray` 只有
/// 這一處——兩個模組本來就互相依賴（`tray.rs` 早已 `use crate::widgets::{AppState,
/// PauseReason}`），Rust 的模組系統不像 C 標頭檔需要避免循環引用。
pub fn set_pause_reason(app: &AppHandle, reason: PauseReason, active: bool) -> bool {
    let state = app.state::<AppState>();
    let event = {
        let mut reasons = state
            .pause_reasons
            .lock()
            .expect("pause_reasons mutex poisoned");
        match apply_pause_reason(&mut reasons, reason, active) {
            Some(event) => event,
            // fix F2（review 5.5 low）：沒有變化——不廣播、不重建系統匣選單、不寫記錄。
            None => return !reasons.is_empty(),
        }
    };
    let paused = event.paused;
    log::info!(
        "暫停原因變更：{reason:?}={active} → paused={paused} reason={:?}",
        event.reason
    );
    let _ = app.emit("pause", event);
    crate::tray::refresh(app);
    // dynamic-wallpaper task 4.7a：桌布排程也看同一個暫停原因集合；只投遞通知、不等它。
    crate::wallpaper_coordinator::notify_app(app, crate::wallpaper_coordinator::Wake::PauseChanged);
    paused
}

/// 系統匣選單需要的暫停狀態（task 5.5；`crate::tray::refresh` 用）：`Manual` 這個原因目前是否
/// 啟用（決定選單項目點下去是要「暫停」還是「繼續」），以及排除 `Manual` 後目前最優先的其他
/// 原因（[`top_other_reason`]；決定選單文字要不要附註「仍因……暫停中」，見
/// `crate::tray::pause_toggle_label`）。
pub fn manual_pause_status(app: &AppHandle) -> (bool, Option<PauseReason>) {
    let state = app.state::<AppState>();
    let reasons = state
        .pause_reasons
        .lock()
        .expect("pause_reasons mutex poisoned");
    (
        reasons.contains(&PauseReason::Manual),
        top_other_reason(&reasons),
    )
}

// ── 電池／省電：業務判斷（task 5.5；design.md D12「電池／省電依設定」）────────────────
//
// `crate::desktop` 只負責偵測「目前是否使用電池／是否處於省電模式」這兩個原始訊號
// （[`crate::desktop::PauseSignal::OnBattery`]／[`crate::desktop::PauseSignal::PowerSaver`]），
// 要不要因此真的觸發暫停，取決於 `settings::PauseRules`（使用者可關閉）。原始訊號另外存一份
// （[`RawPowerSignals`]），是因為使用者改設定（`update_settings`）時要能立即套用新規則，而不必
// 等下一次電源通知才生效——只存「目前的 `pause_reasons` 集合」看不出「電池／省電目前的原始
// 狀態」，因為關閉規則後對應的 `PauseReason` 就從集合裡移除了。

/// [`AppState::raw_power`]：電池／省電的原始訊號快取，不受使用者設定影響。
#[derive(Debug, Clone, Copy, Default)]
pub struct RawPowerSignals {
    pub on_battery: bool,
    pub power_saver: bool,
}

/// `crate::desktop` 偵測到電源來源變化時呼叫（`PauseSignal::OnBattery`）：更新原始訊號快取，
/// 依目前設定的 `pause_on_battery` 重算 [`PauseReason::Battery`]。
pub fn set_battery_raw(app: &AppHandle, on_battery: bool) {
    {
        let state = app.state::<AppState>();
        state
            .raw_power
            .lock()
            .expect("raw_power mutex poisoned")
            .on_battery = on_battery;
    }
    apply_battery_pause_rule(app);
}

/// `crate::desktop` 偵測到省電模式變化時呼叫（`PauseSignal::PowerSaver`）：更新原始訊號快取，
/// 依目前設定的 `pause_on_power_saver` 重算 [`PauseReason::PowerSaver`]；並依 design.md D8
/// 「省電狀態變化時也要重新判定外觀」重新套用所有已開啟小工具的外觀（[`refresh_all_widget_
/// appearance`]；1.2 定案毛玻璃恆退回純色前，這個呼叫實際上是 no-op，但重判路徑先接上）。
pub fn set_power_saver_raw(app: &AppHandle, active: bool) {
    {
        let state = app.state::<AppState>();
        state
            .raw_power
            .lock()
            .expect("raw_power mutex poisoned")
            .power_saver = active;
    }
    apply_power_saver_pause_rule(app);
    refresh_all_widget_appearance(app);
}

/// `PauseRules` 的電池規則（純函式）：原始訊號成立且使用者開啟「用電池時暫停」，
/// [`PauseReason::Battery`] 才算成立。宿主（[`apply_battery_pause_rule`]）與桌布排程器
/// （`crate::wallpaper` 的測試，4.7 經 `pause_reasons` 取得同一結果）共用這一個判定。
pub fn battery_pause_active(raw: RawPowerSignals, rules: settings::PauseRules) -> bool {
    raw.on_battery && rules.pause_on_battery
}

/// `PauseRules` 的省電規則（純函式）：原始訊號成立且使用者開啟「省電模式時暫停」，
/// [`PauseReason::PowerSaver`] 才算成立。共用方式同 [`battery_pause_active`]。
pub fn power_saver_pause_active(raw: RawPowerSignals, rules: settings::PauseRules) -> bool {
    raw.power_saver && rules.pause_on_power_saver
}

/// 依目前的原始訊號＋設定重算 [`PauseReason::Battery`]（[`battery_pause_active`]）。
fn apply_battery_pause_rule(app: &AppHandle) {
    let state = app.state::<AppState>();
    let raw = *state.raw_power.lock().expect("raw_power mutex poisoned");
    let rules = state
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .pause_rules;
    set_pause_reason(app, PauseReason::Battery, battery_pause_active(raw, rules));
}

/// 依目前的原始訊號＋設定重算 [`PauseReason::PowerSaver`]（[`power_saver_pause_active`]）。
fn apply_power_saver_pause_rule(app: &AppHandle) {
    let state = app.state::<AppState>();
    let raw = *state.raw_power.lock().expect("raw_power mutex poisoned");
    let rules = state
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .pause_rules;
    set_pause_reason(
        app,
        PauseReason::PowerSaver,
        power_saver_pause_active(raw, rules),
    );
}

/// `update_settings` 變更 `pause_rules` 後呼叫：用目前已知的原始訊號重新套用規則，不必等下一
/// 次電源通知（design.md D12「電池／省電依設定」——使用者剛好在已經使用電池／省電模式時打開
/// 這兩個開關，應該立即生效）。一併重判外觀（涵蓋使用者同時改了 `appearance_mode` 的情況，
/// 見 [`set_power_saver_raw`] 文件）。
pub fn reapply_power_pause_rules(app: &AppHandle) {
    apply_battery_pause_rule(app);
    apply_power_saver_pause_rule(app);
    refresh_all_widget_appearance(app);
}

/// design.md D8：省電模式等系統狀態變化後，重新判定並套用所有已開啟小工具的外觀（task 5.5：
/// 「重判路徑要接上」）。[`desktop::apply_appearance`] 目前對 `Acrylic` 恆退回 `Solid`
/// （探針 1.2 尚未定案毛玻璃實際套用方式），故本函式眼下實際上是 no-op；1.2 定案後不需要再
/// 補呼叫點，直接在 `apply_appearance` 裡接上實作即可。
pub fn refresh_all_widget_appearance(app: &AppHandle) {
    let user_mode = app
        .state::<AppState>()
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .appearance_mode;
    let mode = desktop::current_appearance(user_mode);
    for (label, window) in app.webview_windows() {
        if label.starts_with("w-") {
            desktop::apply_appearance(&window, mode);
        }
    }
}

/// design.md D4／D7：`report_content(has_content)`（task 7.3 取代 `report_size`）——頁面自行
/// 判斷「內容高度為 0」的舊條件（見 `host/ui/widget.html` 的 `ResizeObserver` 回呼）後只送一個
/// 布林值，核心不再需要自己解讀尺寸。`window` 由 Tauri 自動注入（依 IPC 訊息來源的視窗，不是
/// 前端傳的參數），故一定是送出這個 `invoke` 的那個小工具視窗，不需要另外驗證 label。
///
/// **不改變視窗大小**——視窗矩形完全由格子決定；只用來決定顯示／隱藏並重套倍率，見
/// [`apply_report_content`]。
#[tauri::command]
pub fn report_content(window: Window, has_content: bool) {
    apply_report_content(&window, has_content);
}

/// `report_content` 的實際處理（task 7.3；取代 task 7.2 的 `apply_report_size`）：
///
/// - `window` 不是小工具視窗（例如 `settings`）或取不到 HWND：忽略（防禦性）。
/// - 先依 `has_content` 更新 [`WidgetRuntime::content_empty`]，再以目前的 `edit_mode`
///   （design.md D7「無內容」：編輯版面期間 SHALL 顯示所有開啟中且有格子的小工具）算出
///   [`WidgetRuntime::should_show`]：
///   - 不該顯示：[`desktop::hide_widget`] 隱藏，不動位置與尺寸（格子照樣保留），且不重套倍率
///     （視窗反正看不到）。
///   - 該顯示（有內容，或無內容但正在編輯版面）：重套 [`WidgetRuntime::zoom`] 記錄的倍率
///     （頁面 Reload 後 WebView2 ZoomFactor 可能回到 1，見 `crate::recovery`；倍率未變時
///     `SetZoomFactor` 不改變版面、不會再觸發 `ResizeObserver`，不會形成迴圈）；視窗目前不
///     可見時以 [`desktop::show_at_bottom`] 重新顯示（編輯版面期間無內容者由頁面自己畫佔位
///     外框，見 `host/ui/widget.html`）。
pub fn apply_report_content(window: &Window, has_content: bool) {
    let label = window.label().to_string();
    let Some(id) = widget_id_from_label(&label) else {
        return;
    };
    let Ok(hwnd) = window.hwnd() else {
        log::warn!("report_content：取得 HWND 失敗（{id}）");
        return;
    };

    let state = window.state::<AppState>();
    let edit_mode = *state.edit_mode.lock().expect("edit_mode mutex poisoned");

    let (show, zoom) = {
        let mut runtime = state
            .widget_runtime
            .lock()
            .expect("widget_runtime mutex poisoned");
        // fix F2（F1 範圍外發現）：建立中尚未 present 的視窗只記錄、不顯示也不隱藏。
        let Some(outcome) = runtime.report_content_outcome(&label, has_content, edit_mode) else {
            log::debug!("report_content：{label} 尚未 present，只記錄 has_content={has_content}");
            return;
        };
        outcome
    };

    if !show {
        if let Err(err) = desktop::hide_widget(hwnd) {
            log::warn!("隱藏小工具視窗失敗（{id}）：{err}");
        }
        return;
    }

    if let (Some(zoom), Some(webview_window)) =
        (zoom, window.app_handle().get_webview_window(&label))
    {
        apply_content_zoom(&webview_window, zoom);
    }
    if !desktop::is_widget_visible(hwnd) {
        if let Err(err) = desktop::show_at_bottom(hwnd) {
            log::error!("重新顯示小工具視窗失敗（{id}）：{err}");
        }
    }
}

/// 把倍率套用到頁面內容（WebView2 `ZoomFactor`，`WebviewWindow::set_zoom` → wry
/// `ICoreWebView2Controller::SetZoomFactor`）。倍率由推導出的矩形、該小工具的倍率設計框與
/// 字級設定計算（widget-adaptive-zoom-and-grid design.md D1，[`crate::layout::content_zoom`]），
/// 頁面 CSS viewport＝矩形邏輯尺寸 ÷ 倍率。`set_zoom` 只動 WebView2 controller，不改 tao 視窗
/// 旗標（小工具不可呼叫會改 tao 旗標的 API，見 desktop.rs 不變量）。失敗只記錄。
fn apply_content_zoom(window: &WebviewWindow, zoom: f64) {
    if let Err(err) = window.set_zoom(zoom) {
        log::warn!("套用小工具內容縮放失敗（{}）：{err}", window.label());
    }
}

// ── task 7.2：格線推導（design.md D7、D9）───────────────────────────────────────────

/// 設定中所有**開啟中**小工具的推導輸入（依 [`WIDGET_SPECS`] 順序＝註冊表順序，D9 同級
/// 優先序即此順序）。純函式。
fn grid_inputs(settings: &Settings) -> Vec<GridWidgetInput> {
    WIDGET_SPECS
        .iter()
        .filter_map(|spec| {
            let config = settings.widgets.get(spec.id).filter(|c| c.enabled)?;
            Some(GridWidgetInput {
                id: spec.id,
                monitor: config.placement.monitor.clone(),
                record_rect: config.placement.grid_rect(),
                zoom_box: spec.zoom_box,
            })
        })
        .collect()
}

/// design.md D9「實際位置推導」：設定中所有開啟中的小工具 × 目前顯示器清單 → 每個小工具的
/// 推導結果（[`ResolvedWidgetPlacement`]），順序同 [`WIDGET_SPECS`]。關閉中的小工具不參與
/// （不佔格）。純函式，委派給 [`layout::resolve_grid_placements`]；倍率用的字級取
/// `settings.font_scale`——呼叫端傳入當下設定（[`relayout_all_widgets`]、建立視窗）或拖曳開始
/// 時的快照（[`DragSession`]），widget-adaptive-zoom-and-grid design.md D2。
pub fn resolve_enabled_widgets(
    monitors: &[MonitorInfo],
    settings: &Settings,
) -> Vec<(&'static str, ResolvedWidgetPlacement)> {
    let inputs = grid_inputs(settings);
    let resolved = layout::resolve_grid_placements(monitors, &inputs, settings.font_scale);
    inputs.iter().map(|w| w.id).zip(resolved).collect()
}

/// 重新推導（[`relayout_all_widgets`]、系統匣「空間不足」清單）用的推導：`monitors` 為空
/// （顯示器列舉暫時失敗，例如顯示組態切換瞬間 `available_monitors()` 回錯誤）時回傳 `None`
/// ——呼叫端維持現狀不動，不把所有小工具判成空間不足並隱藏（fix F1，review 7.2 L）；下一次
/// 顯示器／工作區／DPI 事件會再推導一次。純函式。
pub fn resolve_for_relayout(
    monitors: &[MonitorInfo],
    settings: &Settings,
) -> Option<Vec<(&'static str, ResolvedWidgetPlacement)>> {
    (!monitors.is_empty()).then(|| resolve_enabled_widgets(monitors, settings))
}

/// 在 [`resolve_enabled_widgets`] 的結果裡查某個小工具；不在其中（關閉中）回傳 `None`。
fn resolution_for(
    resolved: &[(&'static str, ResolvedWidgetPlacement)],
    id: &str,
) -> Option<ResolvedWidgetPlacement> {
    resolved.iter().find(|(rid, _)| *rid == id).map(|(_, r)| *r)
}

/// task 7.4（design.md D7「記錄位置的寫回時機」第 2 點；specs/widget-host-windows「小工具互不
/// 重疊」「開啟小工具但原位置已被佔用」「空間不足」Scenario）：`update_settings` 合併後、存檔
/// 前，對本次 patch 由關閉變開啟的小工具（`before` 是 `false`、`merged` 是 `true`）依
/// [`WIDGET_SPECS`] 順序（即註冊表順序）逐一委派 [`layout::placement_for_opening_widget`]。
///
/// - 找到空位：就地改寫 `merged` 中該小工具的記錄格子（`monitor` 不變），繼續處理下一個——
///   同一筆 patch 同時開啟多個小工具時，後處理者組 `others` 會讀到 `merged` 目前的狀態，因此
///   看得到先前已處理者剛寫入的新位置（brief「同一筆 patch 同時開啟多個小工具時……後處理者要
///   把先前已寫入的新位置算進去」）。
/// - 找不到空位：立即回傳 `Err("空間不足，請先調整版面")`，呼叫端應捨棄整個 `merged`、沿用
///   `before`，不落地、不廣播（brief「任何一個失敗整筆拒絕」）。
/// - 不相交、或所屬顯示器不存在：不改動，繼續下一個（[`layout::placement_for_opening_widget`]
///   的 `Ok(None)`）。
///
/// 純函式（不碰 `AppState`、不需要 `AppHandle`）：呼叫端只需傳入目前的顯示器清單，方便單元
/// 測試直接餵合成的 [`MonitorInfo`]。
fn place_newly_enabled_widgets(
    monitors: &[MonitorInfo],
    before: &Settings,
    mut merged: Settings,
) -> Result<Settings, String> {
    for spec in WIDGET_SPECS {
        let was_enabled = before.widgets.get(spec.id).is_some_and(|c| c.enabled);
        let now_enabled = merged.widgets.get(spec.id).is_some_and(|c| c.enabled);
        if was_enabled || !now_enabled {
            continue;
        }
        // merge_user_patch 已 backfill，十個 id 在 merged 中必定齊全；理論上不會落入 else。
        let Some(config) = merged.widgets.get(spec.id) else {
            continue;
        };
        let target_monitor = config.placement.monitor.clone();
        let target_record = config.placement.grid_rect();

        let others: Vec<(settings::MonitorId, layout::GridRect)> = merged
            .widgets
            .iter()
            .filter(|(other_id, c)| other_id.as_str() != spec.id && c.enabled)
            .map(|(_, c)| (c.placement.monitor.clone(), c.placement.grid_rect()))
            .collect();

        match layout::placement_for_opening_widget(
            monitors,
            &target_monitor,
            target_record,
            &spec.zoom_box,
            &others,
        ) {
            Ok(None) => {}
            Ok(Some(rect)) => {
                let placement = &mut merged
                    .widgets
                    .get_mut(spec.id)
                    .expect("上面已確認存在")
                    .placement;
                placement.col = rect.col;
                placement.row = rect.row;
                placement.w = rect.w;
                placement.h = rect.h;
            }
            Err(()) => return Err("空間不足，請先調整版面".to_string()),
        }
    }
    Ok(merged)
}

/// task 7.4（design.md D9；specs/widget-host-windows「多螢幕與 DPI 定位」「小工具互不重疊」）：
/// [`resolve_enabled_widgets`] 的推導結果 → 系統匣選單要顯示的「空間不足，暫時隱藏」項目清單，
/// 依 [`WIDGET_SPECS`] 順序（`resolved` 本身就是這個順序）回傳 `(小工具 id, 選單文字)`。
/// `id` 供 `crate::tray` 當這個 `tauri::menu::MenuItem` 的 id（不可點，收到事件也會被忽略）；
/// 中文名稱查 [`widget_spec`]，理論上一定找得到（`resolved` 的 id 只會來自 [`WIDGET_SPECS`]
/// 本身），查不到時退回 id 原文而非 panic，不讓選單重整因為一筆異常資料整個失敗。純函式，
/// 供單元測試直接餵合成的 `resolved` 清單。
pub fn no_space_hidden_menu_entries(
    resolved: &[(&'static str, ResolvedWidgetPlacement)],
) -> Vec<(&'static str, String)> {
    resolved
        .iter()
        .filter(|(_, r)| matches!(r, ResolvedWidgetPlacement::HiddenNoSpace))
        .map(|(id, _)| {
            let name = widget_spec(id).map_or(*id, |spec| spec.display_name);
            (*id, format!("空間不足，暫時隱藏：{name}"))
        })
        .collect()
}

/// 顯示設定變更、`update_settings`、拖曳結束後重新推導並套用到所有已存在的小工具視窗
/// （design.md D9「重新推導時機」）。觸發端：
/// - `WM_DISPLAYCHANGE`／`WM_SETTINGCHANGE(SPI_SETWORKAREA)`：`desktop::GatekeeperCallbacks::
///   relayout_all`（守門視窗）。
/// - `WM_DPICHANGED`：Tauri `WindowEvent::ScaleFactorChanged`（`main.rs`）。
/// - [`update_settings`]、[`finish_widget_drag`]。
///
/// 冪等：只吃「目前」的顯示器清單與設定。每個視窗：
/// - 推導為 `Placed`：[`desktop::set_window_rect`] 設矩形（不動 z-order、不顯示，隱藏中的視窗
///   維持隱藏）、套倍率並記入 [`WidgetRuntime::zoom`]；若先前因空間不足而隱藏，移出
///   `no_space_hidden`，且有內容時以 [`desktop::show_at_bottom`] 重新顯示。
/// - 推導為 `HiddenNoSpace`：記入 `no_space_hidden` 並隱藏，結尾呼叫 `crate::tray::refresh`
///   讓系統匣選單的「空間不足，暫時隱藏」標示跟上（task 7.4：[`no_space_hidden_menu_entries`]）。
/// - 小工具已關閉（不在推導結果中，視窗正在關閉）：略過。
///
/// 不會對「建立中尚未 present 的視窗」誤呼叫 `show_at_bottom`：只有被本函式或
/// [`create_widget_window`] 記為空間不足隱藏者才會在這裡被重新顯示。
///
/// 回傳實際套用新矩形的小工具數（記錄用）。
///
/// widget-adaptive-zoom-and-grid task 5.2（design.md D4「生命週期」）：重排之後一律呼叫
/// [`sync_grid_overlay`]——顯示器、DPI、工作區變更要讓格線跟著新工作區重畫，[`update_settings`]
/// 的主題色變更也經這裡重畫格線顏色。本函式只是包裝：實際重排在 [`relayout_widgets_now`]，它有
/// 「沒有小工具視窗」「顯示器列舉為空」兩條提早返回，格線同步不能被它們略過（編輯版面中即使沒有任何
/// 小工具，格線照樣要顯示）。
pub fn relayout_all_widgets(app: &AppHandle) -> usize {
    let applied = relayout_widgets_now(app);
    sync_grid_overlay(app);
    applied
}

/// [`relayout_all_widgets`] 的重排本體（不含格線同步）。
fn relayout_widgets_now(app: &AppHandle) -> usize {
    let state = app.state::<AppState>();
    let settings = state
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .clone();
    let windows: Vec<(String, &'static str, WebviewWindow)> = app
        .webview_windows()
        .into_iter()
        .filter_map(|(label, window)| widget_id_from_label(&label).map(|id| (label, id, window)))
        .collect();
    if windows.is_empty() {
        return 0;
    }

    let monitors = current_monitors(app);
    let Some(resolved) = resolve_for_relayout(&monitors, &settings) else {
        log::warn!("relayout_all_widgets：顯示器列舉為空，維持現狀不重排");
        return 0;
    };
    // task 7.5：拖曳中的視窗不動——跨不同縮放的顯示器拖曳時 `WM_DPICHANGED` 會觸發
    // `ScaleFactorChanged` → 本函式；若此時把視窗設回推導矩形，會把它從滑鼠底下拉走。
    // 放開時 [`finish_widget_drag`] 會先丟棄 [`DragSession`] 再呼叫本函式，屆時照常套用。
    // fix F6b：等待延後判定的視窗同樣不動（[`DragTracker::holds`]），判定時才讀它的最終矩形。
    let held: HashSet<isize> = {
        let tracker = state.drag.lock().expect("drag mutex poisoned");
        windows
            .iter()
            .filter_map(|(_, _, w)| w.hwnd().ok().map(|h| h.0 as isize))
            .filter(|h| tracker.holds(*h))
            .collect()
    };

    let mut applied = 0usize;
    for (label, id, window) in windows {
        let Some(resolution) = resolution_for(&resolved, id) else {
            continue;
        };
        let Ok(hwnd) = window.hwnd() else {
            continue;
        };
        if held.contains(&(hwnd.0 as isize)) {
            log::info!(
                "relayout_all_widgets：{label} 拖曳中或等待放開判定，暫不套用（判定後重新推導）"
            );
            continue;
        }
        match resolution {
            ResolvedWidgetPlacement::Placed {
                physical_rect,
                zoom,
                ..
            } => {
                if let Err(err) = desktop::set_window_rect(hwnd, physical_rect) {
                    log::error!("relayout_all_widgets：套用矩形失敗（{label}）：{err}");
                    continue;
                }
                apply_content_zoom(&window, zoom);
                // fix F1（review 7.3 L1）：編輯模式在這一刻才讀（不沿用迴圈前的值），與
                // `set_edit_mode` 交錯時以最新狀態決定要不要重新顯示無內容者。
                let edit_mode = *state.edit_mode.lock().expect("edit_mode mutex poisoned");
                let reshow = {
                    let mut runtime = state
                        .widget_runtime
                        .lock()
                        .expect("widget_runtime mutex poisoned");
                    runtime.zoom.insert(label.clone(), zoom);
                    runtime.no_space_hidden.remove(&label) && runtime.should_show(&label, edit_mode)
                };
                if reshow {
                    if let Err(err) = desktop::show_at_bottom(hwnd) {
                        log::error!("relayout_all_widgets：重新顯示失敗（{label}）：{err}");
                    }
                }
                applied += 1;
            }
            ResolvedWidgetPlacement::HiddenNoSpace => {
                state
                    .widget_runtime
                    .lock()
                    .expect("widget_runtime mutex poisoned")
                    .no_space_hidden
                    .insert(label.clone());
                log::warn!("relayout_all_widgets：空間不足，暫時隱藏 {label}");
                if let Err(err) = desktop::hide_widget(hwnd) {
                    log::warn!("relayout_all_widgets：隱藏失敗（{label}）：{err}");
                }
            }
        }
    }
    // task 7.4：`no_space_hidden` 集合可能剛被本輪改變（新增或移出），系統匣選單需要跟上
    // （design.md D9「多螢幕與 DPI 定位」；理由同 task 5.5 `set_pause_reason` 文件「唯一收斂
    // 點」——這裡是所有既有小工具重新推導的收斂點）。
    crate::tray::refresh(app);
    applied
}

/// widget-adaptive-zoom-and-grid task 5.2（design.md D4「生命週期」「執行緒」）：編輯版面格線疊加視窗的
/// **唯一收斂點**。冪等：讀目前的編輯版面旗標、主題色、顯示器清單，交給
/// [`grid_sync::plan_overlays`] 算出目標狀態（編輯中→每台顯示器一個、矩形＝工作區；否則全部銷毀），
/// 再在主執行緒照計畫建立／重畫／銷毀（[`grid_sync::apply_on_this_thread`]）。
///
/// 呼叫時機：[`set_edit_mode`] 切換成功後、[`relayout_all_widgets`] 結尾（顯示器、DPI、工作區變更；
/// [`update_settings`] 的主題色變更也經它，不另外呼叫）。
///
/// ## 執行緒
///
/// 格線視窗（`desktop::grid_overlay::GridOverlay`）是 `!Send`，建立、重畫、銷毀都必須在有訊息迴圈的
/// 主執行緒；格線集合因此放在 `grid_sync` 的 `thread_local!`，不放 [`AppState`]（managed state 必須
/// `Send + Sync`）。一律經 `AppHandle::run_on_main_thread`：
/// - 在主執行緒（[`set_edit_mode`]／[`update_settings`] 這類同步指令、`ScaleFactorChanged`）＝**同步**
///   直接執行——查證 `tauri-runtime-wry-2.12.0/src/lib.rs` 的 `Context::send_user_message`（約 263–280 行）：
///   `current_thread().id() == self.main_thread_id` 時直接 `handle_user_message`，`Message::Task` 當場執行；
/// - 在其他執行緒（守門視窗的延後重排、拖曳放開判定的工作執行緒等）＝只投遞到事件迴圈，**不等待**。
///
/// 執行時才讀狀態（不在投遞當下讀），投遞後到執行前若又切換了編輯版面，以執行當下的最新狀態為準。
/// 不持有任何 [`AppState`] 的鎖呼叫本函式（主執行緒同步執行時會再取 `edit_mode`／`settings` 鎖）。
///
/// ## 不變式
///
/// - `updater::is_exiting_for_update()` 為真時不建立（AGENTS.md「自動更新」：新的視窗建立入口都要查；
///   規劃前讀一次（[`grid_sync::plan_overlays`]），修正第 1 輪起每次建立前後再各讀一次——旗標由更新器在
///   背景執行緒設，可能在規劃後或建立期間才翻轉，建立後為真就立即銷毀剛建好的格線，見
///   [`grid_sync::apply_plan`]）。收尾開始前已存在的格線由 `updater::exit` 的 `close_ui` 在主執行緒經
///   [`clear_grid_overlays`] 銷毀。
/// - `build_ui` 的 `StartGate` 規則不適用：格線不是 `build_ui` 啟動的背景元件（不寫檔、不呼叫桌布
///   API、不啟動執行緒），只在使用者進入編輯版面後於主執行緒建立；收尾期間則由上一條擋住。
pub fn sync_grid_overlay(app: &AppHandle) {
    let handle = app.clone();
    if let Err(err) = app.run_on_main_thread(move || sync_grid_overlay_on_main_thread(&handle)) {
        log::warn!("格線同步：投遞到主執行緒失敗：{err}");
    }
}

/// [`sync_grid_overlay`] 在主執行緒上的本體：讀狀態並套用。顯示器只在「編輯中且不是因更新結束」時
/// 才列舉（`QueryDisplayConfig` 不便宜，平時每次重排都會走到這裡）。
fn sync_grid_overlay_on_main_thread(app: &AppHandle) {
    let exiting_for_update = crate::updater::is_exiting_for_update();
    let state = app.state::<AppState>();
    let edit_mode = *state.edit_mode.lock().expect("edit_mode mutex poisoned");
    let color = desktop::grid_overlay::accent_or_default(
        &state
            .settings
            .lock()
            .expect("settings mutex poisoned")
            .accent_color,
    );
    let monitors = if edit_mode && !exiting_for_update {
        current_monitors(app)
    } else {
        Vec::new()
    };
    grid_sync::apply_on_this_thread(
        grid_sync::OverlayInputs {
            edit_mode,
            exiting_for_update,
            monitors: &monitors,
            color,
        },
        &crate::updater::is_exiting_for_update,
    );
}

/// task 5.2 修正第 1 輪：銷毀所有格線疊加視窗，回傳數量。**必須在主執行緒呼叫**（格線集合在主執行緒的
/// thread-local；在其他執行緒呼叫只會看到空集合、回 0）。給更新收尾 `updater::exit` 的 `close_ui`
/// （它在主執行緒銷毀所有 WebView 視窗時一併呼叫）——`updater` 不直接碰 `desktop::grid_overlay`。
pub fn clear_grid_overlays() -> usize {
    grid_sync::clear_on_this_thread()
}

// ── 編輯版面拖曳（task 5.3；task 7.2 改為格線；task 7.5 加合法判斷、紅框預告與彈回；
//    design.md D7「編輯版面」、D4 `edit-preview`）──────────────────────────────────────

/// `edit-preview` 事件的 payload（design.md D4：`{ id, valid }`）：編輯版面拖曳中「若此刻
/// 放開」的合法性改變時廣播，頁面只理會 `id` 等於自己的那筆，用來切換紅色外框。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct EditPreview {
    pub id: &'static str,
    pub valid: bool,
}

/// `edit-preview` 的去重（task 7.5）：只在合法性相對上一次改變時回報。拖曳開始先當作合法
/// （`Default`＝合法），因此一路合法的拖曳一則事件都不會發。純狀態機，單元測試直接驅動。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditPreviewTracker {
    last_valid: bool,
}

impl Default for EditPreviewTracker {
    fn default() -> Self {
        Self { last_valid: true }
    }
}

impl EditPreviewTracker {
    /// 記下這次的合法性；與上一次不同才回傳 `Some(valid)`（呼叫端據此廣播）。
    pub fn observe(&mut self, valid: bool) -> Option<bool> {
        if valid == self.last_valid {
            return None;
        }
        self.last_valid = valid;
        Some(valid)
    }
}

/// 一次編輯版面拖曳的上下文（task 7.5）：`WM_ENTERSIZEMOVE` 時建立（[`begin_widget_drag`]），
/// 快照當下的顯示器清單、設定與推導結果，讓拖曳中每一則 `WM_MOVING`（[`update_widget_drag`]）
/// 只做純計算——子類別化回呼內不列舉顯示器、不讀檔、不建視窗（memory：
/// sync-wndproc-slow-work-blocks-sender）。`WM_EXITSIZEMOVE`（[`finish_widget_drag`]）時轉成
/// 待判定，延後判定（[`settle_widget_drag`]）時取出丟棄；放開的判斷改用當下最新的設定與
/// 顯示器清單重算（同一個純函式，拖曳中顯示組態若有變化，以放開當下為準）。
///
/// `hwnd` 存成 `isize`：`HWND` 不是 `Send`，而本結構放在 Tauri managed state
/// （[`AppState::drag`]）裡。
#[derive(Debug, Clone)]
pub struct DragSession {
    hwnd: isize,
    id: &'static str,
    monitors: Vec<MonitorInfo>,
    settings: Settings,
    resolved: Vec<(&'static str, ResolvedWidgetPlacement)>,
    tracker: EditPreviewTracker,
    /// task 7.6：收到過 `WM_SIZING` 就是調整大小（被拖邊；一次迴圈內固定），`None`＝移動。
    edges: Option<layout::ResizeEdges>,
    /// fix drag-dpi：移動時的尺寸換算上下文（[`DragSession::with_drag_start`]）；`None`＝拖曳
    /// 開始時讀不到視窗矩形或游標，維持舊行為（不換尺寸、預告照中心判定）。
    sizing: Option<layout::DragResize>,
    /// fix drag-dpi：上一次 [`layout::resolve_drag_rect`] 決定的目標顯示器（快照內索引）。
    target: Option<usize>,
    /// fix drag-dpi：內容倍率目前對應的顯示器（拖曳開始＝起始顯示器）；與 `target` 不同時
    /// [`DragSession::moving`] 回報新倍率。
    zoomed_for: Option<usize>,
    /// fix F6b：拖曳開始時的視窗矩形（[`DragSession::with_drag_start`]）。延後判定時視窗矩形
    /// 等於它＝這次迴圈沒有改變任何東西（典型是拖回原位放開，見 [`settle_drop`]）。
    start: Option<layout::PhysicalRect>,
}

/// [`DragSession::moving`] 的結果（fix drag-dpi）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DragMove {
    /// 視窗此刻應有的矩形；`None`＝沒有尺寸換算上下文，不介入。
    pub rect: Option<layout::PhysicalRect>,
    /// 合法性改變時要廣播的 `edit-preview`。
    pub preview: Option<EditPreview>,
    /// 目標顯示器改變時，新矩形在新顯示器上的內容倍率（[`layout::content_zoom`]，字級取拖曳
    /// 開始時的快照），讓拖曳中
    /// 內容大小也與放下後一致；目標沒變為 `None`。
    pub zoom: Option<f64>,
}

impl DragSession {
    /// 以拖曳開始當下的顯示器清單與設定建立（推導一次、之後沿用）。
    pub fn new(
        hwnd: isize,
        id: &'static str,
        monitors: Vec<MonitorInfo>,
        settings: Settings,
    ) -> Self {
        let resolved = resolve_enabled_widgets(&monitors, &settings);
        Self {
            hwnd,
            id,
            monitors,
            settings,
            resolved,
            tracker: EditPreviewTracker::default(),
            edges: None,
            sizing: None,
            target: None,
            zoomed_for: None,
            start: None,
        }
    }

    /// fix drag-dpi：記下拖曳開始時的視窗矩形與游標位置，建立尺寸換算上下文
    /// （[`layout::DragResize`]）：起始顯示器＝視窗中心所在者、保留格數＝目前實際格數（推導
    /// 不到時退回記錄格數，與 [`drop_placement`] 相同）、抓點＝游標在視窗內的比例位置。
    pub fn with_drag_start(mut self, window: layout::PhysicalRect, cursor: (i32, i32)) -> Self {
        self = self.with_start_rect(window);
        let center = (
            window.x.saturating_add(window.width / 2),
            window.y.saturating_add(window.height / 2),
        );
        let start_monitor = layout::monitor_index_for_point(&self.monitors, center.0, center.1);
        let keep = keep_cells(&self.settings, &self.resolved, self.id);
        if let (Some(start_monitor), Some(keep)) = (start_monitor, keep) {
            self.sizing = Some(layout::DragResize {
                start_monitor,
                start_size: (window.width, window.height),
                keep,
                grab: layout::grab_fraction(window, cursor),
            });
            self.target = Some(start_monitor);
            self.zoomed_for = Some(start_monitor);
        }
        self
    }

    /// `WM_MOVING`（fix drag-dpi）：依游標算出視窗此刻應有的矩形（目標顯示器上保留格數的
    /// 實際大小、游標維持在同一個比例位置，[`layout::resolve_drag_rect`]），並以**這個矩形與
    /// 目標顯示器**算「若此刻放開」的合法性（[`DragMove`]）。沒有尺寸換算上下文時矩形為
    /// `None`，預告退回以 `proposed` 的中心判定（[`Self::preview`]）。
    pub fn moving(&mut self, proposed: layout::PhysicalRect, cursor: (i32, i32)) -> DragMove {
        let Some((target, rect)) = self.drag_rect(cursor) else {
            return DragMove {
                rect: None,
                preview: self.preview(proposed),
                zoom: None,
            };
        };
        let valid = drop_placement(
            &self.monitors,
            &self.settings,
            &self.resolved,
            self.id,
            rect,
            Some(target),
        )
        .is_ok();
        let preview = self
            .tracker
            .observe(valid)
            .map(|valid| EditPreview { id: self.id, valid });
        let zoom = if self.zoomed_for == Some(target) {
            None
        } else {
            self.zoomed_for = Some(target);
            // 字級取拖曳開始時的設定快照，拖曳中不變（widget-adaptive-zoom-and-grid
            // design.md D2）。`self.id` 來自 WIDGET_SPECS，查不到不會發生；防禦起見不改倍率。
            widget_spec(self.id).map(|spec| {
                layout::content_zoom(
                    rect.width,
                    rect.height,
                    self.monitors[target].scale_factor,
                    &spec.zoom_box,
                    self.settings.font_scale,
                )
            })
        };
        DragMove {
            rect: Some(rect),
            preview,
            zoom,
        }
    }

    /// 依游標算出此刻的目標顯示器與矩形並記下目標（不算合法性）。fix F6：拖曳中的 DPI 訊息
    /// 不再另外呼叫它——`desktop` 只回答自己正在套用的矩形（也就是 [`Self::moving`] 的結果）。
    fn drag_rect(&mut self, cursor: (i32, i32)) -> Option<(usize, layout::PhysicalRect)> {
        let sizing = self.sizing?;
        let current = self.target.unwrap_or(sizing.start_monitor);
        let (target, rect) = layout::resolve_drag_rect(&self.monitors, &sizing, current, cursor)?;
        self.target = Some(target);
        Some((target, rect))
    }

    /// 放開判定用的目標顯示器（fix drag-dpi）：放開當下的顯示器清單與拖曳快照相同時沿用拖曳中
    /// 決定的目標（與最後一次預告同一台）；組態已變（索引可能對不上）或沒有尺寸換算上下文時
    /// 回傳 `None`，交回中心判定。
    pub fn drop_target(&self, monitors: &[MonitorInfo]) -> Option<usize> {
        if monitors == self.monitors.as_slice() {
            self.target
        } else {
            None
        }
    }

    /// 這次迴圈是否為調整大小（收到過 `WM_SIZING`），是的話回傳被拖邊。
    pub fn resize_edges(&self) -> Option<layout::ResizeEdges> {
        self.edges
    }

    /// task 7.6：以 `WM_SIZING` 的被拖邊與提議矩形算「若此刻放開」的合法性（記下被拖邊，放開
    /// 時據此改走調整大小的判斷），合法性改變時回傳要廣播的 `edit-preview`。
    pub fn preview_resize(
        &mut self,
        edges: layout::ResizeEdges,
        proposed: layout::PhysicalRect,
    ) -> Option<EditPreview> {
        self.edges = Some(edges);
        let valid = resize_placement(
            &self.monitors,
            &self.settings,
            &self.resolved,
            self.id,
            edges,
            proposed,
        )
        .is_ok();
        self.tracker
            .observe(valid)
            .map(|valid| EditPreview { id: self.id, valid })
    }

    /// fix F6b：只記下拖曳開始時的視窗矩形（讀不到游標、無法建立尺寸換算上下文時，仍要能
    /// 判斷「回到起點」，見 [`Self::ended_at_start`]）。
    pub fn with_start_rect(mut self, window: layout::PhysicalRect) -> Self {
        self.start = Some(window);
        self
    }

    /// fix F6b：延後判定時讀到的視窗矩形是否就是拖曳開始時的矩形（沒有記下起點時為 `false`）。
    pub fn ended_at_start(&self, settled: layout::PhysicalRect) -> bool {
        self.start == Some(settled)
    }

    /// 這個拖曳是否屬於 `hwnd`（`HWND.0 as isize`）。
    pub fn is_for(&self, hwnd: isize) -> bool {
        self.hwnd == hwnd
    }

    /// 以提議矩形（`WM_MOVING` 的 `lParam`）算「若此刻放開」的合法性，合法性改變時回傳要
    /// 廣播的 `edit-preview`。
    pub fn preview(&mut self, proposed: layout::PhysicalRect) -> Option<EditPreview> {
        let valid = drop_placement(
            &self.monitors,
            &self.settings,
            &self.resolved,
            self.id,
            proposed,
            None,
        )
        .is_ok();
        self.tracker
            .observe(valid)
            .map(|valid| EditPreview { id: self.id, valid })
    }
}

/// fix F6b：一筆等待延後判定的拖曳結束（[`DragTracker::exit`] 建立、[`DragTracker::settle`]
/// 取出）。`session` 為 `None`＝這次拖曳沒有建立 session（版面鎖定或不在編輯版面）。
#[derive(Debug)]
pub struct PendingDrop {
    pub hwnd: isize,
    /// 這次結束的序號，隨 posted message 的 `wParam` 帶回，用來分辨過期的訊息。
    pub seq: usize,
    pub session: Option<DragSession>,
}

/// 進行中的系統移動／調整大小迴圈。
#[derive(Debug)]
struct ActiveDrag {
    hwnd: isize,
    session: Option<DragSession>,
}

/// fix F6b：編輯版面拖曳的狀態機（純資料，不碰 Win32，可單元測試）。
///
/// `WM_EXITSIZEMOVE` 時不判定放開位置，只把進行中的拖曳轉成「待判定」（[`Self::exit`]），由
/// `desktop` `PostMessageW` 一則自訂訊息給同一扇視窗；處理那則訊息時才取出（[`Self::settle`]）、
/// 以當時的視窗矩形判定。`WM_EXITSIZEMOVE` 不保證視窗已在最終位置（例如關閉「拖曳時顯示視窗
/// 內容」時，系統在它之後才定位視窗）；posted message 要等系統的移動迴圈與 `WM_SYSCOMMAND`
/// 處理完、回到訊息迴圈才會派送，屆時視窗已在實際最終位置（design.md D7「取消」）。小工具不可
/// 聚焦，拖曳中按 Esc 不會取消（Esc 送進使用者的前景程式）；反悔＝拖回原位或放到不合法位置。
///
/// 規則：
/// - 同一時間最多一個進行中、一個待判定（系統的移動迴圈是模態的）。
/// - 延後期間又開始新的拖曳（[`Self::begin`]，同一扇或另一扇）：前一筆**交回呼叫端先判定**，
///   不丟棄——此時前一個迴圈早已結束，視窗矩形就是最終位置；丟棄的話那扇視窗會停在未對齊、
///   未保存的位置。舊訊息之後到達時序號不符，不再判定第二次。
/// - 進行中與待判定的視窗都不讓 [`relayout_all_widgets`] 動（[`Self::holds`]）：待判定期間若
///   被重排拉回記錄位置，判定就會讀到重排後的矩形。
/// - 視窗銷毀（`WM_NCDESTROY`，[`Self::forget`]）：丟棄它的進行中與待判定，不寫回。
#[derive(Debug, Default)]
pub struct DragTracker {
    active: Option<ActiveDrag>,
    pending: Option<PendingDrop>,
    next_seq: usize,
}

impl DragTracker {
    /// `WM_ENTERSIZEMOVE`：`hwnd` 開始一次迴圈（session 之後以 [`Self::attach`] 補上）。回傳
    /// 尚未判定的前一筆，呼叫端要先判定它。
    pub fn begin(&mut self, hwnd: isize) -> Option<PendingDrop> {
        self.active = Some(ActiveDrag {
            hwnd,
            session: None,
        });
        self.pending.take()
    }

    /// 把 session 掛到同一扇視窗進行中的迴圈上（不是它的迴圈就丟棄）。
    pub fn attach(&mut self, session: DragSession) {
        if let Some(active) = self.active.as_mut() {
            if session.is_for(active.hwnd) {
                active.session = Some(session);
            }
        }
    }

    /// `hwnd` 進行中迴圈的 session（`WM_MOVING`／`WM_SIZING` 用）。
    pub fn session_mut(&mut self, hwnd: isize) -> Option<&mut DragSession> {
        self.active
            .as_mut()
            .filter(|a| a.hwnd == hwnd)
            .and_then(|a| a.session.as_mut())
    }

    /// `hwnd` 是否正在拖曳或等待判定（[`relayout_all_widgets`] 不動它）。
    pub fn holds(&self, hwnd: isize) -> bool {
        self.active.as_ref().is_some_and(|a| a.hwnd == hwnd)
            || self.pending.as_ref().is_some_and(|p| p.hwnd == hwnd)
    }

    /// `WM_EXITSIZEMOVE`：`hwnd` 的迴圈結束，轉成待判定。回傳要隨訊息帶回的序號，以及被取代
    /// 的前一筆待判定（正常不會有；有的話呼叫端先判定它）。
    pub fn exit(&mut self, hwnd: isize) -> (usize, Option<PendingDrop>) {
        let session = match self.active.take() {
            Some(active) if active.hwnd == hwnd => active.session,
            other => {
                self.active = other;
                None
            }
        };
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        let superseded = self.pending.replace(PendingDrop { hwnd, seq, session });
        (seq, superseded)
    }

    /// posted message 到達：序號與視窗都相符才取出待判定（只取一次）。
    pub fn settle(&mut self, hwnd: isize, seq: usize) -> Option<PendingDrop> {
        if self
            .pending
            .as_ref()
            .is_some_and(|p| p.hwnd == hwnd && p.seq == seq)
        {
            self.pending.take()
        } else {
            None
        }
    }

    /// `hwnd` 銷毀：丟棄它的進行中與待判定。
    pub fn forget(&mut self, hwnd: isize) {
        if self.active.as_ref().is_some_and(|a| a.hwnd == hwnd) {
            self.active = None;
        }
        if self.pending.as_ref().is_some_and(|p| p.hwnd == hwnd) {
            self.pending = None;
        }
    }
}

/// fix F6b：延後判定的結果（[`settle_drop`]）。
#[derive(Debug)]
enum DropDecision {
    /// 視窗回到拖曳開始時的矩形（拖回原位，或原地放開）：不寫回。
    Unchanged,
    /// 合法：寫回這份設定。
    Save(Settings),
    /// 不合法：不寫回（重新推導＝彈回）。
    Reject(DropRejected),
}

/// fix F6b：延後判定的純函式部分。`settled`＝posted message 處理時讀到的視窗矩形；`session`
/// 決定移動或調整大小（收到過 `WM_SIZING`）與移動的目標顯示器。
fn settle_drop(
    monitors: &[MonitorInfo],
    settings: &Settings,
    id: &str,
    session: Option<&DragSession>,
    settled: layout::PhysicalRect,
) -> DropDecision {
    if session.is_some_and(|s| s.ended_at_start(settled)) {
        return DropDecision::Unchanged;
    }
    let result = match session.and_then(DragSession::resize_edges) {
        Some(edges) => resize_end_settings(monitors, settings, id, edges, settled),
        None => {
            let target = session.and_then(|s| s.drop_target(monitors));
            drag_end_settings_on(monitors, settings, id, settled, target)
        }
    };
    match result {
        Ok(updated) => DropDecision::Save(updated),
        Err(rejected) => DropDecision::Reject(rejected),
    }
}

/// 由 `hwnd` 反查小工具視窗的 label 與 id（找不到＝不是小工具視窗或視窗已銷毀）。
fn widget_label_for_hwnd(app: &AppHandle, hwnd: HWND) -> Option<(String, &'static str)> {
    let label = app
        .webview_windows()
        .into_iter()
        .find(|(_, w)| w.hwnd().map(|h| h == hwnd).unwrap_or(false))
        .map(|(label, _)| label)?;
    let id = widget_id_from_label(&label)?;
    Some((label, id))
}

/// `WM_ENTERSIZEMOVE`（`desktop::widget_subclass_proc`，經 `desktop::register_widget_drag_hooks`
/// 登記）：建立 [`DragSession`]。版面鎖定或不在編輯模式時不建立（鎖定時前端不掛
/// `data-tauri-drag-region`，理論上根本進不了拖曳迴圈，這裡是保險）。
///
/// fix drag-dpi：`cursor`＝迴圈開始時的游標位置（`desktop` 以 `GetCursorPos` 讀出；讀不到為
/// `None`），連同此刻的視窗矩形建立尺寸換算上下文（[`DragSession::with_drag_start`]）。
///
/// fix F6b：先在 [`DragTracker`] 標記這扇視窗正在拖曳，若還有前一次拖曳結束尚未延後判定，
/// 立刻判定它（[`judge_pending_drop`]；此時重排不會動到正在拖曳的這扇），之後才以最新的設定
/// 建立這次的 session。
pub fn begin_widget_drag(app: &AppHandle, hwnd: HWND, cursor: Option<(i32, i32)>) {
    let state = app.state::<AppState>();
    let previous = state
        .drag
        .lock()
        .expect("drag mutex poisoned")
        .begin(hwnd.0 as isize);
    if let Some(previous) = previous {
        log::info!(
            "拖曳開始：前一次拖曳結束尚未判定（seq={}），先判定",
            previous.seq
        );
        judge_pending_drop(app, previous);
    }
    let Some((_, id)) = widget_label_for_hwnd(app, hwnd) else {
        return;
    };
    let settings = state
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .clone();
    let edit_mode = *state.edit_mode.lock().expect("edit_mode mutex poisoned");
    if !drag_end_may_save(settings.layout_locked, edit_mode) {
        return;
    }
    let monitors = current_monitors(app);
    let mut session = DragSession::new(hwnd.0 as isize, id, monitors, settings);
    match (desktop::window_rect(hwnd), cursor) {
        (Ok(window), Some(cursor)) => {
            // fix F6b 實機探針：與結束、延後判定兩行對照，看最終位置在哪一步才確定。
            log::info!(
                "拖曳開始（WM_ENTERSIZEMOVE）：{id} 視窗矩形 x={} y={} {}×{}",
                window.x,
                window.y,
                window.width,
                window.height
            );
            session = session.with_drag_start(window, cursor);
        }
        (Ok(window), None) => {
            log::warn!("拖曳開始：讀不到游標（{id}），拖曳中不換尺寸");
            session = session.with_start_rect(window);
        }
        (Err(err), cursor) => {
            log::warn!("拖曳開始：讀不到視窗矩形（{id}；{err}、游標 {cursor:?}），拖曳中不換尺寸")
        }
    }
    state
        .drag
        .lock()
        .expect("drag mutex poisoned")
        .attach(session);
}

/// `WM_MOVING`：以提議矩形更新紅框預告，合法性改變時廣播 `edit-preview`（design.md D4；
/// 事件不含資料，廣播即可）。沒有對應的 [`DragSession`]（鎖定、或不是這扇視窗的拖曳）時略過。
///
/// fix drag-dpi：回傳視窗此刻應有的矩形（[`DragSession::moving`]：目標顯示器上保留格數的
/// 實際大小、游標維持同一比例位置），由 `desktop` 改寫提議矩形並在尺寸不同時套用；預告用的
/// 也是這個矩形。`None`＝不介入。鎖在回傳前就放開——`desktop` 套用尺寸時可能同步觸發
/// `WM_DPICHANGED`，tao 轉送的 `ScaleFactorChanged` 等路徑不應撞上仍被持有的鎖。
///
/// 目標顯示器改變時同步套用新的內容倍率（[`DragMove::zoom`]）。在主執行緒同步呼叫，不排到
/// 其他執行緒：否則排隊中的倍率可能晚於放開時 [`relayout_all_widgets`] 同步套用的倍率才生效，
/// 把它蓋掉（放開時的重排同樣是在 `WM_EXITSIZEMOVE` 處理常式內同步套倍率，同一種呼叫情境）。
pub fn update_widget_drag(
    app: &AppHandle,
    hwnd: HWND,
    proposed: layout::PhysicalRect,
    cursor: Option<(i32, i32)>,
) -> Option<layout::PhysicalRect> {
    let state = app.state::<AppState>();
    let moved = {
        let mut guard = state.drag.lock().expect("drag mutex poisoned");
        match (guard.session_mut(hwnd.0 as isize), cursor) {
            (Some(session), Some(cursor)) => session.moving(proposed, cursor),
            (Some(session), None) => DragMove {
                rect: None,
                preview: session.preview(proposed),
                zoom: None,
            },
            _ => return None,
        }
    };
    if let Some(preview) = moved.preview {
        let _ = app.emit("edit-preview", preview);
    }
    if let Some(zoom) = moved.zoom {
        if let Some((label, _)) = widget_label_for_hwnd(app, hwnd) {
            // 先寫快取再套用：期間進來的 `report_content` 重套的也是新倍率。鎖序：`drag` 已在
            // 上方區塊放掉，這裡只單獨取 `widget_runtime`、放掉後才呼叫 WebView2。
            record_drag_move_zoom(
                &mut state
                    .widget_runtime
                    .lock()
                    .expect("widget_runtime mutex poisoned"),
                &label,
                &moved,
            );
            if let Some(window) = app.get_webview_window(&label) {
                apply_content_zoom(&window, zoom);
            }
        }
    }
    moved.rect
}

/// 拖曳中跨螢幕換算出新倍率（[`DragMove::zoom`]）時，同步寫進 [`WidgetRuntime::zoom`]
/// （Task A 修正第 1 輪，Codex review medium）：快取是 [`apply_report_content`] 重套的單一
/// 來源，不同步的話拖曳中頁面重載或內容由空轉有時會套回起始螢幕的倍率，且
/// `DragSession::zoomed_for` 讓同一目標螢幕不再重算。`zoom` 為 `None`（目標沒變）時不動。
fn record_drag_move_zoom(runtime: &mut WidgetRuntime, label: &str, moved: &DragMove) {
    if let Some(zoom) = moved.zoom {
        runtime.zoom.insert(label.to_string(), zoom);
    }
}

/// task 7.6：`WM_SIZING`——以被拖邊與提議矩形更新紅框預告（[`DragSession::preview_resize`]），
/// 合法性改變時廣播 `edit-preview`（與 [`update_widget_drag`] 同一套去重）。沒有對應的
/// [`DragSession`] 時略過。
pub fn update_widget_resize(
    app: &AppHandle,
    hwnd: HWND,
    edges: layout::ResizeEdges,
    proposed: layout::PhysicalRect,
) {
    let state = app.state::<AppState>();
    let preview = {
        let mut guard = state.drag.lock().expect("drag mutex poisoned");
        guard
            .session_mut(hwnd.0 as isize)
            .and_then(|session| session.preview_resize(edges, proposed))
    };
    if let Some(preview) = preview {
        let _ = app.emit("edit-preview", preview);
    }
}

/// `WM_EXITSIZEMOVE`（拖曳結束）：`desktop::widget_subclass_proc` 在小工具視窗收到這則訊息時
/// 呼叫，交來觸發拖曳的 `hwnd`。
///
/// fix F6b：這裡**不判定**放開位置，只把進行中的拖曳轉成待判定（[`DragTracker::exit`]），回傳
/// 序號；`desktop` 以 `PostMessageW` 把序號送回同一扇視窗，處理時呼叫 [`settle_widget_drag`]
/// 判定。理由見 [`DragTracker`]：`WM_EXITSIZEMOVE` 當下視窗不一定已在最終位置，延後到 posted
/// message 時系統的定位都已完成。此刻的視窗矩形只寫進記錄，供實機探針比對（design.md D7
/// 「取消」）。
pub fn finish_widget_drag(app: &AppHandle, hwnd: HWND) -> usize {
    let state = app.state::<AppState>();
    let (seq, superseded) = state
        .drag
        .lock()
        .expect("drag mutex poisoned")
        .exit(hwnd.0 as isize);
    match desktop::window_rect(hwnd) {
        Ok(r) => log::info!(
            "拖曳結束（WM_EXITSIZEMOVE）：視窗矩形 x={} y={} {}×{}，延後判定 seq={seq}",
            r.x,
            r.y,
            r.width,
            r.height
        ),
        Err(err) => {
            log::info!("拖曳結束（WM_EXITSIZEMOVE）：讀不到視窗矩形（{err}），延後判定 seq={seq}")
        }
    }
    if let Some(superseded) = superseded {
        // 正常不會發生（每次迴圈開始都會先取走待判定）；保險起見照樣判定，不丟棄。
        log::warn!(
            "拖曳結束：還有未判定的前一筆（seq={}），先判定",
            superseded.seq
        );
        judge_pending_drop(app, superseded);
    }
    seq
}

/// fix F6b：`desktop` 處理 [`finish_widget_drag`] 之後投遞的自訂訊息（或投遞失敗時立即）呼叫：
/// 序號相符就取出待判定並判定（[`judge_pending_drop`]），過期的訊息（已在下一次拖曳開始時判定
/// 過）略過。
pub fn settle_widget_drag(app: &AppHandle, hwnd: HWND, seq: usize) {
    let pending = app
        .state::<AppState>()
        .drag
        .lock()
        .expect("drag mutex poisoned")
        .settle(hwnd.0 as isize, seq);
    match pending {
        Some(pending) => judge_pending_drop(app, pending),
        None => log::info!("拖曳延後判定：seq={seq} 已判定或已丟棄，略過"),
    }
}

/// fix F6b：小工具視窗 `WM_NCDESTROY`——丟棄它的進行中拖曳與待判定，不寫回（[`DragTracker::forget`]）。
pub fn forget_widget_drag(app: &AppHandle, hwnd: HWND) {
    if let Some(state) = app.try_state::<AppState>() {
        state
            .drag
            .lock()
            .expect("drag mutex poisoned")
            .forget(hwnd.0 as isize);
    }
}

/// 判定一筆拖曳結束（task 5.3／7.5／7.6；fix F6b 起延後到 posted message 才呼叫）。
///
/// 步驟：
/// 1. 從 `app.webview_windows()` 反查 `hwnd` 對應的 label／小工具 id（找不到＝不是小工具視窗或
///    視窗已銷毀：不寫回、不重排）。
/// 2. 版面鎖定或不在編輯版面（[`drag_end_may_save`]）時不存檔，但照常重新推導把視窗彈回。
/// 3. 讀**此刻**的視窗矩形與顯示器清單，以 [`settle_drop`] 判定：回到起點＝未移動（典型是
///    拖回原位放開）不寫回；收到過 `WM_SIZING` 走調整大小（[`resize_end_settings`]），否則走移動
///    （[`drag_end_settings_on`]；顯示器組態未變時沿用拖曳中決定的目標，見
///    [`DragSession::drop_target`]），保留小工具**目前實際**的格數。
/// 4. 合法：把新記錄位置直接寫進設定（不走 [`settings::merge_user_patch`]——它拒收
///    `placement`），立即存檔並廣播 `settings`。不合法：不寫回。
/// 5. 一律 [`relayout_all_widgets`] 重新推導並套用：寫回成功時視窗移到對齊後的格子；其餘情況
///    記錄未變，推導結果即拖曳前的矩形，`SetWindowPos` 把視窗彈回原位（design.md D7：「拖曳前
///    矩形即推導結果，不另存」）。
/// 6. 一律廣播 `edit-preview { id, valid: true }` 清掉紅框。
fn judge_pending_drop(app: &AppHandle, pending: PendingDrop) {
    let hwnd = HWND(pending.hwnd as *mut std::ffi::c_void);
    let Some((_, id)) = widget_label_for_hwnd(app, hwnd) else {
        log::info!("拖曳延後判定：視窗已不存在（seq={}），不寫回", pending.seq);
        return;
    };
    let state = app.state::<AppState>();
    let settings = state
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .clone();
    let edit_mode = *state.edit_mode.lock().expect("edit_mode mutex poisoned");
    if !drag_end_may_save(settings.layout_locked, edit_mode) {
        // fix F2（review 5.3 low）：不存檔，但照常重新推導——把被（擴充模組等）拖走的視窗彈回
        // 推導位置（Scenario「鎖定時拖曳無效：小工具不移動」）。
        log::warn!(
            "拖曳結束但版面鎖定或不在編輯版面（layout_locked={}、edit_mode={edit_mode}），\
             不存檔並彈回（widget={id}）",
            settings.layout_locked
        );
        relayout_all_widgets(app);
        let _ = app.emit("edit-preview", EditPreview { id, valid: true });
        return;
    }

    match desktop::window_rect(hwnd) {
        Ok(settled) => {
            log::info!(
                "拖曳延後判定（posted）：{id} 視窗矩形 x={} y={} {}×{}（seq={}）",
                settled.x,
                settled.y,
                settled.width,
                settled.height,
                pending.seq
            );
            let monitors = current_monitors(app);
            match settle_drop(&monitors, &settings, id, pending.session.as_ref(), settled) {
                DropDecision::Unchanged => log::info!(
                    "拖曳結束：視窗回到拖曳開始的位置（拖回原位），不寫回（{id}）"
                ),
                DropDecision::Save(updated) => {
                    if let Err(err) = settings::save(&state.settings_path, &updated) {
                        log::error!("拖曳結束：存檔失敗（{id}）：{err}");
                    } else {
                        *state.settings.lock().expect("settings mutex poisoned") = updated.clone();
                        emit_settings(app, &updated);
                    }
                }
                // fix monitor-id：寫出具體原因、對齊後格子與放開矩形，不必再猜是哪一條不成立。
                DropDecision::Reject(rejected) => log::info!(
                    "拖曳結束：放開位置不合法，彈回原位（{id}）：{rejected}；放開矩形 x={} y={} {}×{}",
                    settled.x,
                    settled.y,
                    settled.width,
                    settled.height
                ),
            }
        }
        Err(err) => log::warn!("拖曳結束：取得視窗矩形失敗（{id}）：{err}"),
    }
    relayout_all_widgets(app);
    let _ = app.emit("edit-preview", EditPreview { id, valid: true });
}

/// 拖曳／調整大小放開不能寫回的原因（fix monitor-id：記錄要寫出具體原因）。
#[derive(Debug, Clone, PartialEq)]
enum DropRejected {
    /// `id` 不在規格表或設定裡（不應發生，防禦用）。
    UnknownWidget,
    /// 調整大小時 `id` 目前沒有實際格子（空間不足隱藏、已關閉），沒有可調整的對象。
    NotPlaced,
    /// 合法判斷不成立，附全部原因與判斷對象。
    Placement(layout::PlacementRejection),
}

impl std::fmt::Display for DropRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownWidget => write!(f, "原因：未知的小工具"),
            Self::NotPlaced => write!(f, "原因：小工具目前沒有實際格子（空間不足隱藏或已關閉）"),
            Self::Placement(rejection) => rejection.fmt(f),
        }
    }
}

/// 移動放開的合法判斷（task 7.5）：以 `id` 目前推導出的格數（推導不到時退回記錄格數）做移動
/// 對齊，交給 [`layout::check_move_placement`]。`neighbors`＝其他開啟中小工具的記錄位置與
/// 實際位置（`resolved` 與 [`grid_inputs`] 同序）。合法回傳新記錄位置，否則回傳原因。純函式。
///
/// fix drag-dpi：`target` 為 `Some` 時以該顯示器判斷（拖曳中由 [`layout::resolve_drag_rect`]
/// 決定，預告與放開同一台）；`None` 時照舊取 `dropped` 中心所在者。
fn drop_placement(
    monitors: &[MonitorInfo],
    settings: &Settings,
    resolved: &[(&'static str, ResolvedWidgetPlacement)],
    id: &str,
    dropped: layout::PhysicalRect,
    target: Option<usize>,
) -> Result<settings::WidgetPlacement, DropRejected> {
    widget_spec(id).ok_or(DropRejected::UnknownWidget)?;
    let keep = keep_cells(settings, resolved, id).ok_or(DropRejected::UnknownWidget)?;
    let neighbors = drop_neighbors(settings, resolved, id);
    // fix F6：移動不檢查最小格數（只在調整大小時檢查，見 [`resize_placement`]）。
    match target {
        Some(target) => {
            layout::check_move_placement_on(monitors, target, dropped, keep, &neighbors)
        }
        None => layout::check_move_placement(monitors, dropped, keep, &neighbors),
    }
    .map_err(DropRejected::Placement)
}

/// 移動保留的格數：`id` 目前推導出的實際格數，推導不到（空間不足隱藏）時退回記錄格數；`id`
/// 不在設定裡回傳 `None`。純函式。
fn keep_cells(
    settings: &Settings,
    resolved: &[(&'static str, ResolvedWidgetPlacement)],
    id: &str,
) -> Option<(i32, i32)> {
    let record = settings.widgets.get(id)?.placement.grid_rect();
    Some(match resolution_for(resolved, id) {
        Some(ResolvedWidgetPlacement::Placed { rect, .. }) => (rect.w, rect.h),
        _ => (record.w, record.h),
    })
}

/// 其他開啟中小工具（不含 `id`）的記錄位置與實際位置（`resolved` 與 [`grid_inputs`] 同序），
/// 供移動／調整大小的合法判斷使用。純函式。
fn drop_neighbors(
    settings: &Settings,
    resolved: &[(&'static str, ResolvedWidgetPlacement)],
    id: &str,
) -> Vec<layout::DropNeighbor> {
    grid_inputs(settings)
        .into_iter()
        .filter(|input| input.id != id)
        .map(|input| {
            let actual = match resolution_for(resolved, input.id) {
                Some(ResolvedWidgetPlacement::Placed {
                    monitor_index,
                    rect,
                    ..
                }) => Some((monitor_index, rect)),
                _ => None,
            };
            layout::DropNeighbor {
                record_monitor: input.monitor,
                record_rect: input.record_rect,
                actual,
            }
        })
        .collect()
}

/// 調整大小放開的合法判斷（task 7.6）：以 `id` 目前推導出的實際格子與所在顯示器為原矩形，
/// 只對齊 `edges` 指出的被拖邊，交給 [`layout::check_resize_placement`]。`id` 目前沒有實際
/// 格子（空間不足隱藏、已關閉）時沒有可調整的對象，回傳 [`DropRejected::NotPlaced`]。純函式。
fn resize_placement(
    monitors: &[MonitorInfo],
    settings: &Settings,
    resolved: &[(&'static str, ResolvedWidgetPlacement)],
    id: &str,
    edges: layout::ResizeEdges,
    proposed: layout::PhysicalRect,
) -> Result<settings::WidgetPlacement, DropRejected> {
    let spec = widget_spec(id).ok_or(DropRejected::UnknownWidget)?;
    let Some(ResolvedWidgetPlacement::Placed {
        monitor_index,
        rect,
        ..
    }) = resolution_for(resolved, id)
    else {
        return Err(DropRejected::NotPlaced);
    };
    layout::check_resize_placement(
        monitors,
        monitor_index,
        rect,
        edges,
        proposed,
        &spec.zoom_box,
        &drop_neighbors(settings, resolved, id),
    )
    .map_err(DropRejected::Placement)
}

/// [`settle_drop`] 調整大小分支的純函式部分（同 [`drag_end_settings`]）：合法時回傳
/// 寫回新記錄位置後的設定，不合法回傳原因（呼叫端記錄、不寫回、重新推導＝彈回原大小）。
fn resize_end_settings(
    monitors: &[MonitorInfo],
    settings: &Settings,
    id: &str,
    edges: layout::ResizeEdges,
    dropped: layout::PhysicalRect,
) -> Result<Settings, DropRejected> {
    let resolved = resolve_enabled_widgets(monitors, settings);
    let placement = resize_placement(monitors, settings, &resolved, id, edges, dropped)?;
    with_placement(settings, id, placement)
}

/// [`settle_drop`] 移動分支的純函式部分：放開位置合法時回傳寫回新記錄位置後的設定；不合法或
/// 無法得出可保存的位置時回傳原因（呼叫端記錄、不寫回、重新推導＝彈回原位）。
#[cfg(test)]
fn drag_end_settings(
    monitors: &[MonitorInfo],
    settings: &Settings,
    id: &str,
    dropped: layout::PhysicalRect,
) -> Result<Settings, DropRejected> {
    drag_end_settings_on(monitors, settings, id, dropped, None)
}

/// [`drag_end_settings`] 指定判斷顯示器的版本（fix drag-dpi）：`target`＝拖曳中決定的目標
/// （[`DragSession::drop_target`]），`None` 時取 `dropped` 中心所在者。
fn drag_end_settings_on(
    monitors: &[MonitorInfo],
    settings: &Settings,
    id: &str,
    dropped: layout::PhysicalRect,
    target: Option<usize>,
) -> Result<Settings, DropRejected> {
    let resolved = resolve_enabled_widgets(monitors, settings);
    let placement = drop_placement(monitors, settings, &resolved, id, dropped, target)?;
    with_placement(settings, id, placement)
}

/// 複製一份設定並把 `id` 的記錄位置換成 `placement`。
fn with_placement(
    settings: &Settings,
    id: &str,
    placement: settings::WidgetPlacement,
) -> Result<Settings, DropRejected> {
    let mut updated = settings.clone();
    updated
        .widgets
        .get_mut(id)
        .ok_or(DropRejected::UnknownWidget)?
        .placement = placement;
    Ok(updated)
}

/// `data` 推播的 payload（design.md D4：`{ channel, generation, snapshot }`）。`snapshot` 直接放
/// [`crate::data::Snapshot`]（本身已含 `channel`／`data`／`meta`）——頂層 `channel` 重複一份
/// 是照 D4 字面寫的形狀，方便前端不用解一層就能先按 `channel` 分派。`generation`（task 4.1
/// fix round 3）＝取得這份快照時的註冊表世代，見 [`SnapshotResponse`] 文件。
///
/// task 2.7 follow-up（task-4.1-report.md fix round 3「疑慮」：改資料目錄後若新目錄一直沒有
/// 有效檔案，已開啟的小工具會永遠停在切換前的資料——重建只遞增世代，不主動推送任何東西）：
/// `snapshot` 改成 `Option<Snapshot>`，`None` 序列化為 `null`，代表「這個世代（目前）尚無
/// 資料」，形狀與 [`SnapshotResponse`] 的 empty 回覆（`data`／`meta` 皆為 `null`）一致。
/// [`push_empty_snapshots_after_rebuild`] 用 `None` 推送；[`poll_and_notify`] 真的輪到有效
/// 快照時一律是 `Some`。頁面端 `widget.html` 依 `payload.snapshot === null` 正規化成
/// `status:'empty'`（與 `get_snapshot` 的 empty 回覆同形，小工具模組不需要多認一種形狀）。
#[derive(Debug, Clone, Serialize)]
pub struct DataEvent {
    pub channel: String,
    pub generation: u64,
    pub snapshot: Option<Snapshot>,
}

/// 依視窗 label 查它訂閱的通道（[`WIDGET_SPECS`]；label 規則見 [`window_label`]）。
/// 不是小工具視窗（例如 `settings`）或未知 id 回傳 `None`＝不訂閱任何通道。
pub fn channel_for_label(label: &str) -> Option<&'static str> {
    let id = widget_id_from_label(label)?;
    widget_spec(id).map(|spec| spec.channel)
}

/// [`subscribe_data`] 依呼叫端 label 決定的訂閱通道：桌布渲染視窗（`wallpaper-renderer-<id>`，
/// [`wallpaper_render::is_renderer_label`]——每次渲染一個 label，以前綴判斷、不比對固定字串）
/// 訂閱 [`data::WALLPAPER_CHANNEL`]；其餘同 [`channel_for_label`]（小工具訂閱自己的通道、其他
/// webview 不訂閱）。dynamic-wallpaper task 4.6。
pub fn data_channel_for_label(label: &str) -> Option<&'static str> {
    if wallpaper_render::is_renderer_label(label) {
        return Some(data::WALLPAPER_CHANNEL);
    }
    channel_for_label(label)
}

/// 呼叫端 webview `label` 能不能取得 `channel` 的資料（查詢與推送共用的閘門，dynamic-wallpaper
/// task 4.6）：[`data::WALLPAPER_CHANNEL`] 只給桌布渲染視窗；其餘通道維持既有行為（不限制）。
pub fn may_access_channel(label: &str, channel: &str) -> bool {
    channel != data::WALLPAPER_CHANNEL || wallpaper_render::is_renderer_label(label)
}

/// 桌布渲染視窗銷毀時移除它的資料訂閱（dynamic-wallpaper task 4.6）。渲染視窗每次渲染一個新
/// label、永不重用，不清掉的話訂閱表會隨渲染次數一直變長，直到下一次 `wallpaper` 推送送失敗才
/// 移除。小工具 label 會重用，沿用既有的關閉／重建路徑清除，這裡不動。回傳是否有移除。
pub fn forget_renderer_subscription(state: &AppState, label: &str) -> bool {
    if !wallpaper_render::is_renderer_label(label) {
        return false;
    }
    state
        .data_subscribers
        .lock()
        .expect("data_subscribers mutex poisoned")
        .remove(label)
        .is_some()
}

/// `data` 推播的訂閱表：webview label → (訂閱通道, 該 webview 傳入的 [`Channel`])。
///
/// ## 為什麼不用 Tauri 事件（fix round 1，Codex 2.7）
///
/// 原本以 `emit_filter("data", ..)` 依監聽者宣告的 target 過濾；但 Tauri 2.12
/// `event/listener.rs` 的 `emit_js_filter` 對每個 webview 的每個監聽器套
/// `match_any_or_filter`：`*target == EventTarget::Any || filter(target)`——頁面用裸
/// `window.__TAURI__.event.listen('data', ..)`（target＝Any）時過濾函式根本不會被呼叫、一律
/// 放行，未訂閱的視窗照樣收到其他通道的完整快照（實測見 `host/tools/evidence/
/// 2.7-fix1-red.log`）。`emit_to` 的 `filter_target` 同樣以監聽者 target 比對，且整條路徑
/// 最後仍經過 `match_any_or_filter`，也不行。事件過濾的依據是「監聽者自己宣告的身分」，
/// 核心無從驗證。
///
/// 改用 `tauri::ipc::Channel`：頁面在 [`subscribe_data`] 指令中傳入 JS 端建立的 Channel，
/// Tauri 的 `CommandArg for Channel`（`ipc/channel.rs`）把它綁定在**發出這個 invoke 的
/// webview**（`command.message.webview()`），之後 `Channel::send` 只對該 webview `eval`
/// 回呼（大於 8 KiB 的 payload 走 `plugin:__TAURI_CHANNEL__|fetch`，其暫存佇列也以 webview
/// label 分隔，別的 webview 取不到）。訂閱的通道則由核心依**呼叫端 webview 的 label** 查
/// [`channel_for_label`] 決定，頁面無法自選——投遞限制因此建立在實際接收 webview 的身分上，
/// 不再有 `data` 這個 Tauri 事件可被任何頁面監聽。
pub type DataSubscribers = HashMap<String, (String, Channel<DataEvent>)>;

/// design.md D4：小工具頁面訂閱自己的資料通道（取代 `data` 事件的 listen）。
///
/// - 通道由呼叫端 webview 的 label 決定（[`data_channel_for_label`]），頁面不傳通道名；不是訂閱
///   通道的小工具視窗（`settings`、未知 label）回傳 `Err`，不登記。dynamic-wallpaper task 4.6：
///   桌布渲染視窗（`wallpaper-renderer-<id>`）訂閱到 `wallpaper`，其他 webview 永遠訂閱不到它。
/// - 同一 webview 再次呼叫（頁面重新載入後重新訂閱）以新的 Channel 取代舊的。
/// - 回傳訂閱到的通道名。頁面啟動順序仍是「先 `subscribe_data`，再 `get_snapshot`」：本指令
///   回傳時登記已完成，之後的推播不會漏接（D4「先 listen 再 get」）。
#[tauri::command]
pub fn subscribe_data(
    webview: Webview,
    on_data: Channel<DataEvent>,
    state: State<AppState>,
) -> Result<String, String> {
    let label = webview.label().to_string();
    let channel = data_channel_for_label(&label)
        .ok_or_else(|| format!("視窗 {label} 沒有訂閱任何資料通道"))?
        .to_string();
    let replaced = state
        .data_subscribers
        .lock()
        .expect("data_subscribers mutex poisoned")
        .insert(label.clone(), (channel.clone(), on_data))
        .is_some();
    // task 5.6：頁面重新載入／重建後會重新訂閱，這一行是復原後「資料訂閱已恢復」的記錄證據
    // （每個小工具頁面每次載入只呼叫一次，量很小）。
    log::info!("資料訂閱 widget={label} channel={channel} replaced={replaced}");
    Ok(channel)
}

/// 排程執行緒的一輪（task 4.6 修正輪 3，複審 M1）：[`poll_and_notify`]；主題設定檔新版本第一次
/// 讀到損壞或讀不到時（[`crate::data::ChannelRegistry::wallpaper_retry_pending`]），**放掉註冊表的鎖
/// 之後**以 `wait` 等 [`crate::wallpaper_config::CONFIG_RETRY_DELAY`]，再呼叫一次 [`poll_and_notify`]
/// 做最終判定（存檔途中的競態 → 讀到完整檔；仍壞 → 內建預設）。等待期間不持有任何鎖，主執行緒的
/// `get_snapshot`／`update_settings` 不受影響。回傳兩輪變更通道的聯集（順序依出現先後）。
///
/// 正式由 `main.rs` 以 `thread::sleep` 呼叫；測試注入等待。
pub fn poll_and_notify_settled(
    state: &AppState,
    mut wait: impl FnMut(std::time::Duration),
) -> Vec<String> {
    let mut changed = poll_and_notify(state);
    let pending = state
        .registry
        .lock()
        .expect("registry mutex poisoned")
        .wallpaper_retry_pending();
    if pending {
        wait(crate::wallpaper_config::CONFIG_RETRY_DELAY);
        for channel in poll_and_notify(state) {
            if !changed.contains(&channel) {
                changed.push(channel);
            }
        }
    }
    changed
}

/// 輪詢一次所有通道，對「這一輪真的更新了」的通道推播快照，只送給以 [`subscribe_data`]
/// 訂閱該通道的 webview（design.md D4／D5）。由 `main.rs` 的排程執行緒每 30 秒呼叫一次（含
/// 啟動時的第一次立即呼叫，見 `main.rs` 說明），經 [`poll_and_notify_settled`]。
///
/// dynamic-wallpaper task 4.7a：回傳這輪有更新的通道（`main.rs` 據此喚醒桌布協調迴圈）；推播行為
/// 不變。
pub fn poll_and_notify(state: &AppState) -> Vec<String> {
    let changed = {
        let mut registry = state.registry.lock().expect("registry mutex poisoned");
        registry.poll_all_changed()
    };

    for channel in changed.iter().cloned() {
        // 先把收件者複製出來再送，送出（webview.eval）時不持有訂閱表的鎖。
        let recipients: Vec<(String, Channel<DataEvent>)> = state
            .data_subscribers
            .lock()
            .expect("data_subscribers mutex poisoned")
            .iter()
            // 第二道閘門（task 4.6）：訂閱表只會有 subscribe_data 依 label 決定的通道，這裡再以
            // 收件 webview 的 label 核一次，`wallpaper` 不論如何只送渲染視窗。
            .filter(|(label, (ch, _))| *ch == channel && may_access_channel(label, &channel))
            .map(|(label, (_, sink))| (label.clone(), sink.clone()))
            .collect();
        if recipients.is_empty() {
            // 沒有任何小工具訂閱這個通道（例如使用者從未開啟對應的 customN 小工具）。
            continue;
        }

        // 快照與世代在同一把鎖下讀出（fix round 3）：兩次上鎖之間若剛好重建註冊表，新註冊表
        // 尚未輪詢、`snapshot()` 為 None 而略過；拿到的快照必定屬於同時讀出的那個世代。
        let (snapshot, generation) = {
            let registry = state.registry.lock().expect("registry mutex poisoned");
            (registry.snapshot(&channel).cloned(), registry.generation())
        };
        let Some(snapshot) = snapshot else {
            // 理論上不會發生：poll_all_changed 剛回報這個通道變更，snapshot() 必然是 Some。
            // 防禦性處理，避免中間有人改壞 data.rs 的不變量時在這裡 panic。
            continue;
        };

        let payload = DataEvent {
            channel: channel.clone(),
            generation,
            snapshot: Some(snapshot),
        };
        for (label, sink) in recipients {
            if let Err(err) = sink.send(payload.clone()) {
                // 送不出去（webview 已關閉）：移除該筆訂閱，頁面若重開會重新訂閱。
                log::warn!("推播資料失敗（channel={channel}, webview={label}）：{err}");
                // 只移除「還是這一個 Channel」的那筆：送出前後頁面若剛好重新訂閱，新的
                // Channel id 不同，不可誤刪。
                let mut subscribers = state
                    .data_subscribers
                    .lock()
                    .expect("data_subscribers mutex poisoned");
                if subscribers
                    .get(&label)
                    .is_some_and(|(_, current)| current.id() == sink.id())
                {
                    subscribers.remove(&label);
                }
            }
        }
    }
    changed
}

/// task 2.7 follow-up（task-4.1-report.md fix round 3「疑慮」）：`update_settings` 改
/// `data_dir` 用 `replace_with` 整份重建 [`ChannelRegistry`] 後呼叫。重建本身只遞增世代、
/// 不主動推送任何東西（[`ChannelRegistry::replace_with`] 的行為）——若不在這裡補推一筆，
/// 已開啟的小工具會停在切換前的資料，且若新目錄一直沒有出現有效檔案就永遠不會更新。
///
/// 對目前**已訂閱**的每個通道各推一筆 `snapshot: None`（新世代的 empty；序列化為
/// `null`，形狀與 [`SnapshotResponse`] 的 empty 回覆一致），頁面依此顯示「尚無資料」
/// （specs/widget-data-feed「尚無資料」）；之後若新目錄出現有效檔案，下一輪
/// [`poll_and_notify`]（至多 30 秒，`main.rs` 排程間隔）照常推送 `ok` 快照，世代不變
/// （同一次重建內產生的世代）。未訂閱的通道（沒有小工具在看）不推送，省下無意義的 IPC。
///
/// fix F3（review task-2.7-datadir-opus.md [medium]）：呼叫端必須**持有 registry 鎖**、把
/// 剛重建的註冊表傳進來，送出也在鎖內完成。原本先放鎖再推送，排程執行緒可以在窗口內輪詢到
/// 新目錄的檔案、先送出同世代的 ok，之後的 empty 把它蓋掉，小工具停在「尚無資料」直到檔案
/// 下次改寫（`JsonFileSource` 未變不再推送）。在鎖內推送後，同世代的 ok 一定晚於 empty 送出；
/// 另外只推給該世代**尚無快照**的通道（`registry.snapshot(channel)` 為 `None`），已有新世代
/// 快照的通道不送 empty。鎖序為 registry → data_subscribers；其他地方不會在持有
/// data_subscribers 時再取 registry（[`poll_and_notify`] 兩把鎖分開取），不會死鎖。
///
/// 世代取自傳入的註冊表（與呼叫端 `replace_with` 同一把鎖下）。送出失敗（webview 已關閉）的
/// 處理與 [`poll_and_notify`] 相同：移除該筆訂閱，頁面若重開會重新訂閱。
fn push_empty_snapshots_after_rebuild(state: &AppState, registry: &ChannelRegistry) {
    let generation = registry.generation();
    let recipients: Vec<(String, String, Channel<DataEvent>)> = state
        .data_subscribers
        .lock()
        .expect("data_subscribers mutex poisoned")
        .iter()
        .filter(|(label, (channel, _))| {
            registry.snapshot(channel).is_none() && may_access_channel(label, channel)
        })
        .map(|(label, (channel, sink))| (label.clone(), channel.clone(), sink.clone()))
        .collect();

    for (label, channel, sink) in recipients {
        let payload = DataEvent {
            channel: channel.clone(),
            generation,
            snapshot: None,
        };
        if let Err(err) = sink.send(payload) {
            log::warn!("推播 empty 快照失敗（channel={channel}, webview={label}）：{err}");
            // 只移除「還是這一個 Channel」的那筆：送出前後頁面若剛好重新訂閱，新的
            // Channel id 不同，不可誤刪（同 poll_and_notify 的理由）。
            let mut subscribers = state
                .data_subscribers
                .lock()
                .expect("data_subscribers mutex poisoned");
            if subscribers
                .get(&label)
                .is_some_and(|(_, current)| current.id() == sink.id())
            {
                subscribers.remove(&label);
            }
        }
    }
}

// ── 視窗工廠（task 3.1；design.md D1、D3、D8、D9）────────────────────────────────────

/// [`plan_window_changes`] 的一筆動作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowChange {
    /// 設定開啟、視窗不存在 → 建立。
    Create(&'static str),
    /// 設定關閉（或設定裡沒有這個 id）、視窗存在 → 關閉。
    Close(&'static str),
}

/// 依設定與「目前已存在的小工具視窗 label」算出要建立／關閉哪些視窗（純函式）。
///
/// - 只看 [`WIDGET_SPECS`] 的十個 id；`existing_labels` 中不屬於它們的 label（例如
///   `settings`）不理會。label 經 [`widget_id_from_label`] 解析，故 task 5.6 重建後的
///   `w-<id>-r<n>` 也算「該小工具的視窗已存在」。
/// - 設定裡缺某個 id 時視為關閉（`settings::load_or_default` 會補齊十個 id，這只是防禦）。
/// - 已開啟且視窗已存在者不動作：位置或縮放的變更由編輯版面（5.3）／多螢幕（3.5）處理，
///   本函式只管開關。
/// - 輸出依 [`WIDGET_SPECS`] 的順序（財經五個在前），建立順序因此固定、可重現。
///
/// `closing_labels`（fix round 1，Codex 3.1）：已送出 `destroy()`、尚未 `Destroyed` 的 label
/// （[`AppState::closing_windows`]）。它們仍在 `existing_labels` 裡，但不算「已存在」：設定
/// 關閉時不再重複關閉；設定開啟時**不**在此輪建立（舊視窗還佔著視窗表，同名 label 會建立
/// 失敗），由 [`on_window_destroyed`] 在銷毀完成後再同步一次建立。
pub fn plan_window_changes(
    settings: &Settings,
    existing_labels: &HashSet<String>,
    closing_labels: &HashSet<String>,
) -> Vec<WindowChange> {
    let mut live_ids: HashSet<&str> = HashSet::new();
    let mut closing_ids: HashSet<&str> = HashSet::new();
    for label in existing_labels {
        let Some(id) = widget_id_from_label(label) else {
            continue;
        };
        if closing_labels.contains(label) {
            closing_ids.insert(id);
        } else {
            live_ids.insert(id);
        }
    }
    WIDGET_SPECS
        .iter()
        .filter_map(|spec| {
            let enabled = settings.widgets.get(spec.id).is_some_and(|c| c.enabled);
            let live = live_ids.contains(spec.id);
            match (enabled, live) {
                // 舊視窗仍在銷毀中：延後到 Destroyed 之後（on_window_destroyed）再建立。
                (true, false) if closing_ids.contains(spec.id) => None,
                (true, false) => Some(WindowChange::Create(spec.id)),
                (false, true) => Some(WindowChange::Close(spec.id)),
                _ => None,
            }
        })
        .collect()
}

/// [`on_window_destroyed`] 的純函式部分：把 `label` 從「銷毀中」集合（與「建立失敗而銷毀」
/// 集合）移除，回傳要不要再同步一次——這次銷毀是本宿主主動關閉的（在 `closing` 裡），需要補建
/// 銷毀期間延後的視窗；但若是建立失敗而銷毀（在 `abandoned` 裡，fix F3）就不補同步，否則持續
/// 失敗的建立會無限「建立→失敗→銷毀→重建」。兩個集合都一定會清掉。
fn finish_closing(
    closing: &mut HashSet<String>,
    abandoned: &mut HashSet<String>,
    label: &str,
) -> bool {
    let was_closing = closing.remove(label);
    let was_abandoned = abandoned.remove(label);
    was_closing && !was_abandoned
}

/// fix F3（review task-3.1-fixA-opus.md [low]）：新視窗以 `label` 建立成功（`build()` 回 Ok＝
/// Tauri 視窗表裡沒有同名的舊視窗）時呼叫，清掉該 label 殘留的「銷毀中」／「建立失敗而銷毀」
/// 標記。標記原本只靠 Destroyed 清除；外部關閉（Alt+F4／WM_CLOSE）若搶在 `destroy()` 之前完成，
/// `destroy()` 回 Ok 卻不會再有 Destroyed，標記殘留，之後同名的新視窗會被永久當成「銷毀中」
/// （[`plan_window_changes`] 不再關閉它、[`mark_closing`] 也回 false）。舊視窗既然已不在視窗表，
/// 它也不可能再送來 Destroyed，清掉不會誤刪仍在銷毀中的標記。
fn claim_label_for_new_window(
    closing: &mut HashSet<String>,
    abandoned: &mut HashSet<String>,
    label: &str,
) {
    closing.remove(label);
    abandoned.remove(label);
}

/// `main.rs` 在 `RunEvent::WindowEvent { event: WindowEvent::Destroyed, .. }` 呼叫（fix round 1，
/// Codex 3.1）。Tauri 在把事件交給 run 回呼**之前**就已把視窗從視窗表移除
/// （`tauri-2.12.0/src/app.rs` `on_event_loop_event` → `AppManager::on_window_close`），故此時
/// 以同名 label 重建不會撞到「label 已存在」。
///
/// 若 `label` 是本宿主以 `destroy()` 關閉的視窗（在 [`AppState::closing_windows`] 裡）：移除
/// 並在獨立執行緒再跑一次 [`sync_widget_windows`]——銷毀期間若使用者又把同一個小工具開回來，
/// 那一輪同步會因舊視窗仍在銷毀中而延後建立（見 [`plan_window_changes`]），由這裡補上。同步
/// 冪等，設定沒有變化時什麼都不做。run 回呼在主執行緒，主執行緒不可直接建立視窗（理由同
/// [`update_settings`]），故丟到獨立執行緒。
pub fn on_window_destroyed(app: &AppHandle, label: &str) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    // fix F1（review 7.3 M2）：關閉後再開啟同一個小工具會沿用同一個 label（`w-<id>`），舊的
    // 「已 present」記錄不清掉，新視窗在 present 之前就會被編輯版面切換當成已 present。
    state
        .widget_runtime
        .lock()
        .expect("widget_runtime mutex poisoned")
        .presented
        .remove(label);
    // fix F2（review 5.6 high）：被取代的舊視窗已真的銷毀，label 之後可被重新使用。
    state
        .recovery
        .replaced
        .lock()
        .expect("replaced mutex poisoned")
        .on_window_destroyed(label);
    // dynamic-wallpaper task 4.6：渲染視窗 label 不重用，銷毀就清掉它的 wallpaper 訂閱。
    forget_renderer_subscription(&state, label);
    let resync = finish_closing(
        &mut state
            .closing_windows
            .lock()
            .expect("closing_windows mutex poisoned"),
        &mut state
            .abandoned_windows
            .lock()
            .expect("abandoned_windows mutex poisoned"),
        label,
    );
    if resync {
        log::debug!("小工具視窗已銷毀（{label}），重新同步開關");
        let handle = app.clone();
        thread::spawn(move || sync_widget_windows(&handle));
    }
}

/// 依目前設定建立／關閉小工具視窗（design.md D1：每個開啟的小工具一個 `WebviewWindow`，
/// 共用同一個 WebView2 環境——Tauri 預設所有 webview 使用同一個 user data folder）。
///
/// 呼叫時機：啟動時在 `setup` 內（主執行緒，Tauri 允許在 setup 建立視窗）；
/// [`update_settings`] 之後在獨立執行緒（理由見該指令內註解）。以
/// [`AppState::window_sync`] 串行化。個別視窗失敗只記錄、不中斷其餘小工具。
///
/// task 7.4：新建立的視窗可能因空間不足直接以 `HiddenNoSpace` 建立（[`create_widget_window`]
/// 記入 `no_space_hidden`），結尾呼叫 `crate::tray::refresh` 讓系統匣選單跟上（只在
/// `changes` 非空、即真的做了什麼時才呼叫，理由同上方 `if changes.is_empty() { return; }`：
/// 沒有變動就不可能有新的隱藏狀態）。
pub fn sync_widget_windows(app: &AppHandle) {
    // installer-auto-update task 3.3（審查 I1）：正在因更新結束時不建立／關閉視窗（步驟 5 銷毀視窗之後
    // `on_window_destroyed` 會再觸發一輪同步，不擋就把小工具建回來）。
    if crate::updater::is_exiting_for_update() {
        log::info!("小工具視窗同步：正在因更新結束，略過");
        return;
    }
    let state = app.state::<AppState>();
    let _guard = state
        .window_sync
        .lock()
        .expect("window_sync mutex poisoned");
    let settings = state
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .clone();

    // 先取「銷毀中」再取視窗表：兩次讀取之間若剛好收到 Destroyed，視窗表已不含該 label，
    // 規劃結果是直接建立（正確）；即使規劃成「延後」，那個 Destroyed 也一定會觸發下一輪同步
    // （on_window_destroyed，它要等本輪釋放 window_sync），本輪延後的建立會在那一輪補上。
    let closing = state
        .closing_windows
        .lock()
        .expect("closing_windows mutex poisoned")
        .clone();
    let existing: HashSet<String> = app.webview_windows().into_keys().collect();
    let changes = plan_window_changes(&settings, &existing, &closing);
    if changes.is_empty() {
        return;
    }

    // 推導只在真的要建立視窗時做一次（QueryDisplayConfig 不便宜）：同一輪建立的視窗都用同一份
    // 「全部開啟中小工具」的推導結果，彼此不相交（design.md D9）。
    let mut resolved: Option<Vec<(&'static str, ResolvedWidgetPlacement)>> = None;
    for change in changes {
        match change {
            WindowChange::Create(id) => {
                let resolved = resolved.get_or_insert_with(|| {
                    resolve_enabled_widgets(&current_monitors(app), &settings)
                });
                let Some(spec) = widget_spec(id) else {
                    continue;
                };
                let Some(resolution) = resolution_for(resolved, id) else {
                    continue;
                };
                if let Err(err) = create_widget_window(
                    app,
                    &window_label(id),
                    spec,
                    resolution,
                    settings.appearance_mode,
                ) {
                    // 因更新收尾略過＝info；真正的建立失敗＝error（見 `CreateWidgetError`）。
                    match err.log_level() {
                        log::Level::Error => log::error!("建立小工具視窗失敗（{id}）：{err}"),
                        level => log::log!(level, "小工具視窗（{id}）：{err}"),
                    }
                }
            }
            WindowChange::Close(id) => close_widget_window(app, &state, id),
        }
    }
    crate::tray::refresh(app);
}

/// 目前的顯示器清單（design.md D9：穩定識別經 `DisplayConfigGetDeviceInfo`）。查詢失敗回傳
/// 空清單：重新推導（[`relayout_all_widgets`]、系統匣清單）經 [`resolve_for_relayout`] 維持
/// 現狀不動；拖曳結束的合法判斷對空清單一律不合法（不寫回，接著的重排也不動）；建立視窗時
/// 推導結果是 `HiddenNoSpace`，以隱藏狀態建立，等下一次重新推導有顯示器時再顯示。
fn current_monitors(app: &AppHandle) -> Vec<MonitorInfo> {
    let monitors = app.available_monitors().unwrap_or_default();
    let primary = app.primary_monitor().ok().flatten();
    desktop::monitor_infos_from_tauri_monitors(
        &monitors,
        primary.as_ref(),
        &desktop::source_name_to_device_path(),
    )
}

/// task 7.4（design.md D9；specs/widget-host-windows）：目前推導結果中「空間不足，暫時隱藏」
/// 的小工具，供 `crate::tray::refresh` 組系統匣選單。每次呼叫都重新讀一次設定＋顯示器清單並
/// 重新推導（成本與 [`relayout_all_widgets`] 同一量級，選單重整本身的頻率已經很低）。
pub fn current_no_space_hidden(app: &AppHandle) -> Vec<(&'static str, String)> {
    let settings = app
        .state::<AppState>()
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .clone();
    let monitors = current_monitors(app);
    resolve_for_relayout(&monitors, &settings)
        .map(|resolved| no_space_hidden_menu_entries(&resolved))
        .unwrap_or_default()
}

/// 建立單一小工具視窗：`visible(false)` 建立 → 套外觀 → [`desktop::present_widget`]（樣式、
/// 矩形、顯示並置底一次完成，design.md D8）。
///
/// 視窗屬性（design.md D8 與 controller 對 1.2／1.3 未定案前的暫定樣式裁決）：
/// - `visible(false)`：建立當下不顯示，顯示與置底由桌面層一次完成（專案 memory「置底視窗
///   建立當下不會置底」）。
/// - `focusable(false)`（tao 產生 `WS_EX_NOACTIVATE`）＋`focused(false)`：不搶焦點。
/// - `always_on_bottom(true)`：tao 在之後任何 z-order 變更請求時改寫成 `HWND_BOTTOM`。
/// - `skip_taskbar(true)`：tao 以 `ITaskbarList::DeleteTab` 再保險一次；真正讓視窗不進工作列
///   與 Alt+Tab 的是桌面層套的 `WS_EX_TOOLWINDOW`。
/// - `decorations(false)`＋`shadow(false)`＋`resizable(false)`：無邊框、無外框陰影，視窗
///   矩形就是內容矩形，面板以外不多出透明邊框（specs「面板以外的區域不攔截滑鼠」）。
/// - `transparent(true)`：純色模式下由頁面畫半透明深色面板與圓角（見
///   [`desktop::apply_appearance`]）。
///
/// 尺寸與位置（task 7.2）：完全由格子決定——`resolution` 是呼叫端以
/// [`resolve_enabled_widgets`] 對所有開啟中小工具推導出的結果：
/// - `Placed`：以推導出的實體矩形建立並顯示，倍率＝推導出的 zoom（記入
///   [`WidgetRuntime::zoom`]，[`report_content`] 據此重套）。
/// - `HiddenNoSpace`：建立視窗、套好樣式但不顯示（[`desktop::prepare_widget_hidden`]），記入
///   [`WidgetRuntime::no_space_hidden`]；之後 [`relayout_all_widgets`] 推導出空位時才顯示。
///
/// `label`：一般建立用 [`window_label`]（`w-<id>`）；task 5.6 故障復原重建時用
/// [`RecoveryState::next_label`]（`w-<id>-r<n>`），因為舊視窗此時仍存在。
///
/// 成功後訂閱 WebView2 `ProcessFailed`（task 5.8 的記錄＋task 5.6 的復原，見
/// [`desktop::install_process_failed_handler`]）——每個視窗只在這裡訂閱一次；`Reload()` 沿用
/// 同一個 `ICoreWebView2`，訂閱仍有效，不需要也不可以重訂。
///
/// 失敗時銷毀已建立的視窗並回傳 `Err`。
pub fn create_widget_window(
    app: &AppHandle,
    label: &str,
    spec: &WidgetSpec,
    resolution: ResolvedWidgetPlacement,
    user_appearance: settings::AppearanceMode,
) -> Result<WebviewWindow, CreateWidgetError> {
    // installer-auto-update 4.x 審查 M2：故障重建在工作執行緒上通過入口的旗標檢查後才開始收尾時，這裡是
    // 最後兩道關卡——建立前一次，`build()` 回來後一次。`build()` 在工作執行緒呼叫時由 Tauri 轉到主執行緒
    // 執行、我們拿不到那個閉包，所以建好後再檢查：旗標為真就在顯示前自己銷毀。旗標為假、其後才開始收尾
    // 的，步驟 5 `close_ui`（主執行緒、在 `build()` 之後才排入）會銷毀它。
    if crate::updater::is_exiting_for_update() {
        return Err(CreateWidgetError::SkippedForUpdate("建立前"));
    }
    let placed = match resolution {
        ResolvedWidgetPlacement::Placed {
            physical_rect,
            zoom,
            ..
        } => {
            if physical_rect.width <= 0 || physical_rect.height <= 0 {
                return Err(CreateWidgetError::Failed(format!(
                    "推導出的視窗矩形無效：{physical_rect:?}"
                )));
            }
            Some((physical_rect, zoom))
        }
        ResolvedWidgetPlacement::HiddenNoSpace => None,
    };

    // 建立時的邏輯尺寸只是暫定值（視窗隱藏中），實際矩形由下方 present／relayout 設定。
    let window = crate::webview_env::with_widget_data_dir(WebviewWindowBuilder::new(
        app,
        label,
        WebviewUrl::App(widget_url(spec.id).into()),
    ))
    .title(format!("fc-host {}", spec.id))
    .visible(false)
    .focused(false)
    .focusable(false)
    .always_on_bottom(true)
    .skip_taskbar(true)
    .decorations(false)
    .shadow(false)
    .resizable(false)
    // task 7.7：不要 WS_MAXIMIZEBOX——編輯版面加上 WS_THICKFRAME 後，系統吸附會把拖到螢幕
    // 頂端的小工具最大化、把拖到螢幕邊的上下緣垂直吸附（見 `desktop::widget_style_with_sizebox`）。
    .maximizable(false)
    .transparent(true)
    .inner_size(spec.zoom_box.min_width, spec.zoom_box.min_height)
    .build()
    .map_err(|e| CreateWidgetError::Failed(format!("WebviewWindowBuilder::build 失敗：{e}")))?;
    if crate::updater::is_exiting_for_update() {
        // 視窗仍是隱藏的（`visible(false)`），尚未登記任何狀態，直接銷毀即可。
        let _ = window.destroy();
        return Err(CreateWidgetError::SkippedForUpdate("建立後立即銷毀"));
    }

    let state = app.state::<AppState>();
    // fix F3（review 3.1 low）：同名舊視窗已不在視窗表，殘留的標記一律清掉（見
    // claim_label_for_new_window）。鎖序同 on_window_destroyed：closing → abandoned。
    claim_label_for_new_window(
        &mut state
            .closing_windows
            .lock()
            .expect("closing_windows mutex poisoned"),
        &mut state
            .abandoned_windows
            .lock()
            .expect("abandoned_windows mutex poisoned"),
        label,
    );

    desktop::apply_appearance(&window, desktop::current_appearance(user_appearance));

    let presented = window
        .hwnd()
        .map_err(|e| format!("取得 HWND 失敗：{e}"))
        .and_then(|hwnd| match placed {
            Some((rect, zoom)) => {
                apply_content_zoom(&window, zoom);
                state
                    .widget_runtime
                    .lock()
                    .expect("widget_runtime mutex poisoned")
                    .zoom
                    .insert(label.to_string(), zoom);
                desktop::present_widget(app, hwnd, rect).map(|_| ())
            }
            None => {
                log::warn!("建立小工具視窗：空間不足，暫時隱藏 {label}");
                state
                    .widget_runtime
                    .lock()
                    .expect("widget_runtime mutex poisoned")
                    .no_space_hidden
                    .insert(label.to_string());
                desktop::prepare_widget_hidden(app, hwnd)
            }
        });
    match presented {
        Ok(_) => {
            // fix F2（review 5.6 high）：label 可能被重新使用（關掉再開的 `w-<id>`）——這扇是新
            // 視窗，不是先前「已被重建取代」的那扇。
            state
                .recovery
                .replaced
                .lock()
                .expect("replaced mutex poisoned")
                .on_window_created(label);
            // fix F1（review 7.3 M2）：present／隱藏準備完成後才讓編輯版面切換可以動這扇視窗。
            // fix F2（F1 範圍外發現）：頁面若在 present 之前就回報無內容（apply_report_content 只
            // 記錄），present 後依記錄藏回去。
            let edit_mode = *state.edit_mode.lock().expect("edit_mode mutex poisoned");
            let show = state
                .widget_runtime
                .lock()
                .expect("widget_runtime mutex poisoned")
                .mark_presented(label, edit_mode);
            if !show && placed.is_some() {
                match window.hwnd() {
                    Ok(hwnd) => {
                        if let Err(err) = desktop::hide_widget(hwnd) {
                            log::warn!("建立小工具視窗：present 前已回報無內容，隱藏失敗（{label}）：{err}");
                        }
                    }
                    Err(err) => log::warn!(
                        "建立小工具視窗：取得 HWND 失敗，無法依內容隱藏（{label}）：{err}"
                    ),
                }
            }
            // task 7.6：編輯版面期間新建（開啟小工具、故障重建）的視窗也要可調整大小。
            // fix F1（review 7.6 L1）：一律排進主執行緒，「要不要 WS_SIZEBOX」在主執行緒真正
            // 執行時才以 `resizable_now` 判斷（排隊期間離開編輯版面就不會殘留）；與
            // `set_edit_mode` 的 `sync_widget_resizable` 同在主執行緒、彼此序列化。新視窗本來就
            // 沒有 WS_SIZEBOX，判斷為否時是 no-op。
            // review 7.6 L3：取不到 HWND 時寫記錄，不再靜默略過。
            match window.hwnd() {
                Ok(hwnd) => {
                    let handle = app.clone();
                    desktop::set_widget_resizable_on_main_thread(app, hwnd, move || {
                        resizable_now(&handle.state::<AppState>())
                    });
                }
                Err(err) => log::warn!(
                    "建立小工具視窗：取得 HWND 失敗，無法同步編輯版面的 WS_SIZEBOX（{label}）：{err}"
                ),
            }
            // task 5.8／5.6：訂閱失敗只記錄、不影響視窗本身（見
            // `desktop::install_process_failed_handler` 文件）。事件在 UI 執行緒派送，復原動作
            // 由 `recovery::on_webview_failure` 決定（重建一律丟到獨立執行緒）。
            let handle = app.clone();
            desktop::install_process_failed_handler(&window, label, move |failure| {
                recovery::on_webview_failure(
                    &handle,
                    &failure.label,
                    failure.kind,
                    failure.browser_pid,
                    failure.reload,
                );
            });
            Ok(window)
        }
        Err(err) => {
            // fix F3（review 3.1 low）：與一般關閉一致，銷毀完成前記為「銷毀中」（同步不把它當成
            // 已存在）；另記為「建立失敗而銷毀」，Destroyed 後清標記但不補同步（見 finish_closing）。
            // destroy() 送不出去則撤回兩個標記。
            mark_closing(&state, label);
            state
                .abandoned_windows
                .lock()
                .expect("abandoned_windows mutex poisoned")
                .insert(label.to_string());
            if let Err(destroy_err) = window.destroy() {
                unmark_closing(&state, label);
                state
                    .abandoned_windows
                    .lock()
                    .expect("abandoned_windows mutex poisoned")
                    .remove(label);
                log::warn!("建立失敗後銷毀小工具視窗失敗（{label}）：{destroy_err}");
            }
            Err(CreateWidgetError::Failed(err))
        }
    }
}

/// [`create_widget_window`] 的失敗：被「因更新結束」的閘門擋下不是錯誤（最終審查：收尾期間的略過原本記成
/// `[ERROR]`），與真正的建立失敗分開，呼叫端以 [`CreateWidgetError::log_level`] 決定記錄層級。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateWidgetError {
    /// 正在因更新結束，閘門擋下（內容＝在哪一道關卡擋下）。記 info。
    SkippedForUpdate(&'static str),
    /// 真正的建立失敗。記 error。
    Failed(String),
}

impl CreateWidgetError {
    /// 記錄層級：因更新收尾略過＝info，其餘＝error。
    pub fn log_level(&self) -> log::Level {
        match self {
            Self::SkippedForUpdate(_) => log::Level::Info,
            Self::Failed(_) => log::Level::Error,
        }
    }
}

impl std::fmt::Display for CreateWidgetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SkippedForUpdate(stage) => {
                write!(f, "因更新收尾略過（{stage}），不建立小工具視窗")
            }
            Self::Failed(msg) => f.write_str(msg),
        }
    }
}

/// 關閉單一小工具視窗：先移除它的 `data` 訂閱（不必等下一次推播失敗才清），再
/// `destroy()`（不送 `CloseRequested`，小工具沒有「取消關閉」的情境）。
///
/// task 5.6：同一個小工具的視窗 label 可能是 `w-<id>` 或重建後的 `w-<id>-r<n>`，故以
/// [`widget_id_from_label`] 比對，關閉該小工具所有的視窗。
///
/// fix round 1（Codex 3.1）：`destroy()` 是非同步的（只排入事件迴圈），送出前先把 label 記進
/// [`AppState::closing_windows`]，銷毀完成由 [`on_window_destroyed`] 移除並補同步；已在銷毀中
/// 的視窗不重複送 `destroy()`。送出失敗（沒有排入銷毀）則撤回標記。
fn close_widget_window(app: &AppHandle, state: &AppState, id: &str) {
    for (label, window) in app.webview_windows() {
        if widget_id_from_label(&label) != Some(id) {
            continue;
        }
        if !mark_closing(state, &label) {
            continue;
        }
        state
            .data_subscribers
            .lock()
            .expect("data_subscribers mutex poisoned")
            .remove(&label);
        if let Err(err) = window.destroy() {
            unmark_closing(state, &label);
            log::warn!("關閉小工具視窗失敗（{label}）：{err}");
        }
    }
}

/// 把 `label` 記為「銷毀中」；已在集合裡回傳 `false`（呼叫端不要重複 `destroy()`）。
fn mark_closing(state: &AppState, label: &str) -> bool {
    state
        .closing_windows
        .lock()
        .expect("closing_windows mutex poisoned")
        .insert(label.to_string())
}

/// `destroy()` 送出失敗時撤回 [`mark_closing`]。
fn unmark_closing(state: &AppState, label: &str) {
    state
        .closing_windows
        .lock()
        .expect("closing_windows mutex poisoned")
        .remove(label);
}

/// task 5.6（design.md D12「WebView2 故障」）：以 3.1 的視窗工廠「先建新視窗、再拆舊視窗」
/// 重建 `old_labels` 指定的小工具視窗，回傳 [`recovery::RebuildOutcome`]：`failed`＝**建立失敗**
/// 的舊 label（這些舊視窗保留不拆，由呼叫端決定是否重試）；`skipped`＝已被取代或已不存在而
/// 沒有動作的 label（fix F2，review 5.6 high：不算成功）。
///
/// - 以 [`AppState::window_sync`] 與一般的開關同步（[`sync_widget_windows`]）串行化。
/// - 舊視窗已不存在（例如剛被使用者關掉）→ 略過；該小工具在設定裡已關閉 → 只拆舊的、不建新的。
/// - 新視窗 label 由 [`RecoveryState::next_label`] 產生（`w-<id>-r<n>`）；位置、尺寸、外觀、
///   置底與 `ProcessFailed` 訂閱全部走 [`create_widget_window`]，與一般建立完全相同。新頁面
///   載入後自己 `subscribe_data`＋`get_snapshot`，核心這裡只移除舊 label 的訂閱。
/// - 全部新視窗建好之後才拆舊視窗：過程中小工具視窗數量不會歸零（design.md D12 先建後拆；
///   `main.rs` 另對 `ExitRequested { code: None }` 呼叫 `prevent_exit()` 兜底）。
///
/// 會阻塞（等主執行緒建立視窗），**不可**在主執行緒呼叫，見 [`update_settings`] 的說明。
pub fn rebuild_widget_windows(app: &AppHandle, old_labels: &[String]) -> recovery::RebuildOutcome {
    // installer-auto-update task 3.3（審查 I1）：正在因更新結束時不重建（全部視為沒有動作）。
    if crate::updater::is_exiting_for_update() {
        log::info!(
            "小工具視窗重建：正在因更新結束，略過 {} 個視窗",
            old_labels.len()
        );
        return recovery::RebuildOutcome {
            failed: Vec::new(),
            skipped: old_labels.to_vec(),
        };
    }
    let state = app.state::<AppState>();
    let _guard = state
        .window_sync
        .lock()
        .expect("window_sync mutex poisoned");
    let settings = state
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .clone();

    let mut resolved: Option<Vec<(&'static str, ResolvedWidgetPlacement)>> = None;
    let mut retire: Vec<(String, WebviewWindow)> = Vec::new();
    let mut outcome = recovery::RebuildOutcome::default();
    for old_label in old_labels {
        let already_replaced = state
            .recovery
            .replaced
            .lock()
            .expect("replaced mutex poisoned")
            .contains(old_label);
        if already_replaced {
            log::info!("WebView2 復原：{old_label} 已被重建取代，略過");
            outcome.skipped.push(old_label.clone());
            continue;
        }
        let Some(old_window) = app.get_webview_window(old_label) else {
            log::info!("WebView2 復原：{old_label} 已不存在，略過");
            outcome.skipped.push(old_label.clone());
            continue;
        };
        let Some(spec) = widget_id_from_label(old_label).and_then(widget_spec) else {
            outcome.skipped.push(old_label.clone());
            continue;
        };
        let resolved = resolved
            .get_or_insert_with(|| resolve_enabled_widgets(&current_monitors(app), &settings));
        // 關閉中的小工具不在推導結果裡（design.md D9：只推導開啟中者）。
        let Some(resolution) = resolution_for(resolved, spec.id) else {
            log::info!("WebView2 復原：{old_label} 在設定中已關閉，只移除舊視窗");
            retire.push((old_label.clone(), old_window));
            continue;
        };
        let new_label = state.recovery.next_label(spec.id);
        match create_widget_window(app, &new_label, spec, resolution, settings.appearance_mode) {
            Ok(_) => {
                log::info!("WebView2 復原：已建立新視窗 {old_label} → {new_label}");
                retire.push((old_label.clone(), old_window));
            }
            // 因更新收尾略過：不是重建失敗（入口檢查也把整批記為 skipped），記 info、不排重試。
            Err(err @ CreateWidgetError::SkippedForUpdate(_)) => {
                log::info!("WebView2 復原：{old_label} 不重建，保留舊視窗：{err}");
                outcome.skipped.push(old_label.clone());
            }
            Err(err) => {
                log::error!("WebView2 復原：重建 {old_label} 失敗，保留舊視窗：{err}");
                outcome.failed.push(old_label.clone());
            }
        }
    }

    for (old_label, old_window) in retire {
        state
            .recovery
            .replaced
            .lock()
            .expect("replaced mutex poisoned")
            .mark(&old_label);
        state
            .data_subscribers
            .lock()
            .expect("data_subscribers mutex poisoned")
            .remove(&old_label);
        // fix round 1（Codex 3.1）：同 close_widget_window，銷毀完成前標記為「銷毀中」，避免
        // 開關同步把還沒真的消失的舊視窗當成「已存在」；已在銷毀中（例如剛被使用者關掉）則
        // 不重複送出。
        if !mark_closing(&state, &old_label) {
            continue;
        }
        match old_window.destroy() {
            Ok(()) => log::info!("WebView2 復原：已移除舊視窗 {old_label}"),
            Err(err) => {
                unmark_closing(&state, &old_label);
                log::warn!("WebView2 復原：移除舊視窗 {old_label} 失敗：{err}");
            }
        }
    }
    outcome
}

// ── 置底守門（task 3.3；design.md D8）的兩個回呼 ────────────────────────────────────
//
// `desktop::spawn_gatekeeper` 需要「目前可見的小工具 HWND」與「全部重新置底」兩個動作，但
// desktop 模組不認得小工具註冊表——這兩個函式提供給 `main.rs` 包成
// `desktop::GatekeeperCallbacks` 傳進去，本檔本身不直接呼叫 `spawn_gatekeeper`（守門視窗的
// 建立時機是 `main.rs` 的 setup，緊接在 `sync_widget_windows` 之後）。

/// 目前所有「可見」小工具視窗的 HWND（design.md D8 保險檢查／`TaskbarCreated` 的輸入；
/// design.md D7：內容為 0 而被 [`crate::desktop::hide_widget`] 隱藏的小工具不算——守門邏輯
/// 不該把它們強制重新顯示，這正是必須用 [`desktop::is_widget_visible`] 篩選、而不是「所有
/// 已開啟」小工具的原因）。只看 `w-` 前綴的視窗 label（[`window_label`] 的反向對照）。
pub fn visible_widget_hwnds(app: &AppHandle) -> Vec<HWND> {
    app.webview_windows()
        .into_iter()
        .filter(|(label, _)| label.starts_with("w-"))
        .filter_map(|(_, window)| window.hwnd().ok())
        .filter(|hwnd| desktop::is_widget_visible(*hwnd))
        .collect()
}

/// design.md D8：把目前每一個可見小工具視窗重新 [`desktop::show_at_bottom`]（explorer 重啟／
/// 保險檢查觸發）。回傳成功重排的數量，供 [`crate::desktop::GatekeeperCallbacks`] 的呼叫端
/// 記錄。個別視窗失敗只記錄、不中斷其餘小工具（同 [`sync_widget_windows`] 的錯誤處理原則）。
pub fn rebottom_all_visible_widgets(app: &AppHandle) -> usize {
    let mut count = 0;
    for hwnd in visible_widget_hwnds(app) {
        match desktop::show_at_bottom(hwnd) {
            Ok(()) => count += 1,
            Err(err) => log::warn!("置底守門：重新置底失敗：{err}"),
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 最終審查：收尾期間被閘門擋下的小工具建立是 info（訊息寫明「因更新收尾略過」），真正的建立失敗仍是 error。
    #[test]
    fn widget_creation_skipped_for_update_is_info_and_real_failures_stay_errors() {
        for stage in ["建立前", "建立後立即銷毀"] {
            let skipped = CreateWidgetError::SkippedForUpdate(stage);
            assert_eq!(skipped.log_level(), log::Level::Info);
            let text = skipped.to_string();
            assert!(text.contains("因更新收尾略過"), "{text}");
            assert!(text.contains(stage), "{text}");
        }
        let failed = CreateWidgetError::Failed("WebviewWindowBuilder::build 失敗：x".to_owned());
        assert_eq!(failed.log_level(), log::Level::Error);
        assert_eq!(failed.to_string(), "WebviewWindowBuilder::build 失敗：x");
    }

    // ── 視窗工廠：開關計畫（task 3.1）────────────────────────────────────────

    fn labels(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|id| window_label(id)).collect()
    }

    #[test]
    fn plan_on_first_launch_creates_five_finance_widgets_in_spec_order() {
        let plan = plan_window_changes(&Settings::default(), &HashSet::new(), &HashSet::new());
        assert_eq!(
            plan,
            vec![
                WindowChange::Create("clock"),
                WindowChange::Create("macro"),
                WindowChange::Create("fixed"),
                WindowChange::Create("dynamic"),
                WindowChange::Create("quotes"),
            ]
        );
    }

    /// fix round 1（Codex 3.1）：模擬視窗工廠對非同步 `destroy()` 的完整流程。`table`＝Tauri
    /// 視窗表，`closing`＝[`AppState::closing_windows`]；`destroy()` 只排入 `queue`，要等
    /// [`SimFactory::process_destroyed`]（事件迴圈處理 Destroy、Tauri 移出視窗表、run 回呼呼叫
    /// [`on_window_destroyed`]）才真的消失。
    struct SimFactory {
        table: HashSet<String>,
        closing: HashSet<String>,
        abandoned: HashSet<String>,
        queue: Vec<String>,
    }

    impl SimFactory {
        fn new(ids: &[&str]) -> Self {
            Self {
                table: labels(ids),
                closing: HashSet::new(),
                abandoned: HashSet::new(),
                queue: Vec::new(),
            }
        }

        /// 同 [`sync_widget_windows`]＋[`close_widget_window`] 的決策（不含 Tauri 呼叫）。
        fn sync(&mut self, settings: &Settings) {
            for change in plan_window_changes(settings, &self.table, &self.closing) {
                match change {
                    WindowChange::Create(id) => {
                        let label = window_label(id);
                        assert!(
                            self.table.insert(label.clone()),
                            "label 已存在 → 建立失敗（{id}）"
                        );
                        // 同 create_widget_window：build() 成功後清掉同名的殘留標記。
                        claim_label_for_new_window(&mut self.closing, &mut self.abandoned, &label);
                    }
                    WindowChange::Close(id) => {
                        let label = window_label(id);
                        if self.closing.insert(label.clone()) {
                            self.queue.push(label);
                        }
                    }
                }
            }
        }

        /// review task-3.1-fixA-opus.md [low]：視窗在 [`close_widget_window`] 取得視窗表快照後、
        /// 送出 `destroy()` 前已被外部關閉（Alt+F4／WM_CLOSE）——它的 Destroyed 早在標記之前就
        /// 處理完，之後的 `destroy()` 回 Ok 卻再也不會有 Destroyed。排隊中的 label 直接從視窗表
        /// 消失，不經 [`finish_closing`]。
        fn lose_destroyed(&mut self) {
            for label in std::mem::take(&mut self.queue) {
                self.table.remove(&label);
            }
        }

        /// 事件迴圈處理排隊中的 Destroy；[`finish_closing`] 為真時再同步一次（同
        /// [`on_window_destroyed`]）。
        fn process_destroyed(&mut self, settings: &Settings) {
            for label in std::mem::take(&mut self.queue) {
                self.table.remove(&label);
                if finish_closing(&mut self.closing, &mut self.abandoned, &label) {
                    self.sync(settings);
                }
            }
        }
    }

    /// Codex 3.1 [high]：關閉那輪送出 destroy → 開啟那輪在 Destroyed 之前規劃 → Destroyed。
    /// 修正前開啟那輪看見舊視窗仍在、判定「已存在」而不動作，Destroyed 後沒有人補建。
    #[test]
    fn rapid_off_on_during_async_destroy_ends_with_window() {
        let mut settings = Settings::default();
        let mut sim = SimFactory::new(&["clock", "macro", "fixed", "dynamic", "quotes"]);

        settings.widgets.get_mut("macro").unwrap().enabled = false;
        sim.sync(&settings);
        settings.widgets.get_mut("macro").unwrap().enabled = true;
        sim.sync(&settings); // 銷毀尚未處理
        sim.process_destroyed(&settings);

        assert!(
            sim.table.contains(&window_label("macro")),
            "設定為開啟，最終卻沒有 macro 視窗"
        );
        assert!(sim.closing.is_empty());
    }

    /// 反向：開 → 關（銷毀排隊中）→ 再關一次同步：不重複 destroy；Destroyed 後不補建。
    #[test]
    fn off_while_closing_does_not_destroy_twice_or_recreate() {
        let mut settings = Settings::default();
        let mut sim = SimFactory::new(&["clock", "macro", "fixed", "dynamic", "quotes"]);

        settings.widgets.get_mut("macro").unwrap().enabled = false;
        sim.sync(&settings);
        sim.sync(&settings);
        assert_eq!(sim.queue.len(), 1, "銷毀中的視窗不應再排一次 destroy");
        sim.process_destroyed(&settings);

        assert!(!sim.table.contains(&window_label("macro")));
        assert!(sim.closing.is_empty());
    }

    /// off→on→off→on 連續切換，Destroyed 最後才處理：最終與設定一致（開啟、恰一個視窗）。
    #[test]
    fn repeated_toggle_before_destroyed_converges_to_settings() {
        let mut settings = Settings::default();
        let mut sim = SimFactory::new(&["clock", "macro", "fixed", "dynamic", "quotes"]);
        for enabled in [false, true, false, true] {
            settings.widgets.get_mut("macro").unwrap().enabled = enabled;
            sim.sync(&settings);
        }
        sim.process_destroyed(&settings);
        let macro_windows = sim
            .table
            .iter()
            .filter(|l| widget_id_from_label(l) == Some("macro"))
            .count();
        assert_eq!(macro_windows, 1);
    }

    /// fix F3（review task-3.1-fixA-opus.md [low]）：外部關閉搶在 `destroy()` 之前完成，「銷毀中」
    /// 標記殘留；之後以同名 `w-<id>` 重開的新視窗不得被永久當成「銷毀中」，設定再關閉時要能關掉。
    #[test]
    fn stale_closing_label_does_not_block_closing_reopened_window() {
        let mut settings = Settings::default();
        let mut sim = SimFactory::new(&["clock", "macro", "fixed", "dynamic", "quotes"]);

        settings.widgets.get_mut("macro").unwrap().enabled = false;
        sim.sync(&settings);
        sim.lose_destroyed();
        settings.widgets.get_mut("macro").unwrap().enabled = true;
        sim.sync(&settings);
        assert!(
            sim.table.contains(&window_label("macro")),
            "重開後應以同名 label 建立新視窗"
        );

        settings.widgets.get_mut("macro").unwrap().enabled = false;
        sim.sync(&settings);
        assert_eq!(
            sim.queue,
            vec![window_label("macro")],
            "重開的新視窗被殘留的「銷毀中」標記擋住，關不掉"
        );
        sim.process_destroyed(&settings);
        assert!(!sim.table.contains(&window_label("macro")));
        assert!(sim.closing.is_empty());
    }

    /// fix F3（review task-3.1-fixA-opus.md [low] 第二點）：[`create_widget_window`] 建立後設定
    /// 失敗而 `destroy()` 的視窗，與一般關閉一樣先記為「銷毀中」（同步不把它當成已存在），
    /// Destroyed 後清掉標記；但**不**補同步——否則持續失敗的建立會無限「建立→失敗→銷毀→重建」。
    #[test]
    fn destroyed_after_failed_create_clears_marks_without_resync() {
        let mut closing = labels(&["macro"]);
        let mut abandoned = labels(&["macro"]);
        assert!(!finish_closing(&mut closing, &mut abandoned, "w-macro"));
        assert!(closing.is_empty() && abandoned.is_empty());

        let mut closing = labels(&["macro"]);
        let mut abandoned = HashSet::new();
        assert!(
            finish_closing(&mut closing, &mut abandoned, "w-macro"),
            "一般關閉的 Destroyed 仍要補同步"
        );

        let mut closing = labels(&["macro"]);
        let mut abandoned = labels(&["macro"]);
        claim_label_for_new_window(&mut closing, &mut abandoned, "w-macro");
        assert!(closing.is_empty() && abandoned.is_empty());
    }

    #[test]
    fn plan_defers_create_while_same_widget_is_closing() {
        let existing = labels(&["clock", "macro", "fixed", "dynamic", "quotes"]);
        let closing = labels(&["macro"]);
        // 設定開啟、唯一的視窗在銷毀中 → 本輪不建立（等 Destroyed）。
        assert!(plan_window_changes(&Settings::default(), &existing, &closing).is_empty());
        // 設定關閉、唯一的視窗已在銷毀中 → 不再關閉。
        let mut settings = Settings::default();
        settings.widgets.get_mut("macro").unwrap().enabled = false;
        assert!(plan_window_changes(&settings, &existing, &closing).is_empty());
    }

    #[test]
    fn plan_is_empty_when_windows_match_settings() {
        let existing = labels(&["clock", "macro", "fixed", "dynamic", "quotes"]);
        assert!(plan_window_changes(&Settings::default(), &existing, &HashSet::new()).is_empty());
    }

    #[test]
    fn plan_creates_newly_enabled_and_closes_newly_disabled() {
        let mut settings = Settings::default();
        settings.widgets.get_mut("custom1").unwrap().enabled = true;
        settings.widgets.get_mut("quotes").unwrap().enabled = false;
        let existing = labels(&["clock", "macro", "fixed", "dynamic", "quotes"]);
        assert_eq!(
            plan_window_changes(&settings, &existing, &HashSet::new()),
            vec![
                WindowChange::Close("quotes"),
                WindowChange::Create("custom1")
            ]
        );
    }

    #[test]
    fn plan_ignores_non_widget_labels_and_treats_missing_config_as_disabled() {
        let mut settings = Settings::default();
        settings.widgets.remove("clock");
        let existing = labels(&["clock", "macro", "fixed", "dynamic", "quotes"])
            .into_iter()
            .chain(["settings".to_string(), "w-probe".to_string()])
            .collect();
        assert_eq!(
            plan_window_changes(&settings, &existing, &HashSet::new()),
            vec![WindowChange::Close("clock")]
        );
    }

    #[test]
    fn plan_treats_rebuilt_label_as_existing_widget_window() {
        // task 5.6：重建後的視窗 label 是 `w-<id>-r<n>`，不可被當成「視窗不存在」再建一次，
        // 也要能在設定關閉時被關掉。
        let mut existing = labels(&["macro", "fixed", "dynamic", "quotes"]);
        existing.insert("w-clock-r3".to_string());
        assert!(plan_window_changes(&Settings::default(), &existing, &HashSet::new()).is_empty());

        let mut settings = Settings::default();
        settings.widgets.get_mut("clock").unwrap().enabled = false;
        assert_eq!(
            plan_window_changes(&settings, &existing, &HashSet::new()),
            vec![WindowChange::Close("clock")]
        );
    }

    #[test]
    fn widget_id_from_label_accepts_plain_and_rebuilt_labels() {
        assert_eq!(widget_id_from_label("w-clock"), Some("clock"));
        assert_eq!(widget_id_from_label("w-custom5"), Some("custom5"));
        assert_eq!(widget_id_from_label("w-clock-r1"), Some("clock"));
        assert_eq!(widget_id_from_label("w-custom1-r42"), Some("custom1"));
        assert_eq!(
            widget_id_from_label(&recovery::rebuilt_window_label("quotes", 9)),
            Some("quotes")
        );
    }

    #[test]
    fn widget_id_from_label_rejects_malformed_or_foreign_labels() {
        assert_eq!(widget_id_from_label("settings"), None);
        assert_eq!(widget_id_from_label("fc-gatekeeper"), None);
        assert_eq!(widget_id_from_label("w-probe"), None);
        assert_eq!(widget_id_from_label("w-"), None);
        assert_eq!(widget_id_from_label("clock"), None);
        assert_eq!(widget_id_from_label("w-clock-r"), None, "後綴缺數字");
        assert_eq!(widget_id_from_label("w-clock-rx"), None, "後綴不是數字");
        assert_eq!(widget_id_from_label("w-clock-r1-r2"), None);
        assert_eq!(widget_id_from_label("w-clock-x1"), None);
        assert_eq!(widget_id_from_label("w-probe-r1"), None, "未知 id");
    }

    #[test]
    fn rebuilt_label_maps_to_same_channel_as_plain_label() {
        for spec in WIDGET_SPECS {
            assert_eq!(
                channel_for_label(&recovery::rebuilt_window_label(spec.id, 1)),
                Some(spec.channel)
            );
        }
    }

    // ── task 4.6：wallpaper 通道只給渲染視窗（design.md D7＋desktop-widget-host D4）────────

    #[test]
    fn data_channel_for_label_maps_renderer_labels_to_wallpaper_only() {
        assert_eq!(
            data_channel_for_label("wallpaper-renderer-7"),
            Some(data::WALLPAPER_CHANNEL)
        );
        assert_eq!(
            data_channel_for_label("wallpaper-renderer-123456"),
            Some(data::WALLPAPER_CHANNEL)
        );
        // 小工具維持原本的通道；其他 label 一律不訂閱。
        assert_eq!(
            data_channel_for_label("w-clock"),
            Some(data::TW_EVENTS_CHANNEL)
        );
        assert_eq!(data_channel_for_label("w-custom1"), Some("custom1"));
        for label in [
            "settings",
            "w-probe",
            "wallpaper-renderer",
            "wallpaper-renderer-",
            "wallpaper-renderer-x",
            "wallpaper-renderer-7-r1",
            "w-wallpaper-renderer-7",
        ] {
            assert_eq!(data_channel_for_label(label), None, "{label}");
        }
        // 小工具清單本身沒有任何一個訂閱 wallpaper。
        assert!(WIDGET_SPECS
            .iter()
            .all(|spec| spec.channel != data::WALLPAPER_CHANNEL));
    }

    #[test]
    fn may_access_channel_gates_wallpaper_by_renderer_label() {
        assert!(may_access_channel(
            "wallpaper-renderer-7",
            data::WALLPAPER_CHANNEL
        ));
        for label in ["w-clock", "w-macro", "w-custom1", "settings", "w-probe"] {
            assert!(
                !may_access_channel(label, data::WALLPAPER_CHANNEL),
                "{label} 不得取得 wallpaper"
            );
        }
        // 既有通道不受影響（不改 tw-events 的行為）。
        assert!(may_access_channel("w-clock", data::TW_EVENTS_CHANNEL));
        assert!(may_access_channel("settings", data::TW_EVENTS_CHANNEL));
        assert!(may_access_channel("w-custom1", "custom1"));
    }

    fn wallpaper_test_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "fc-host-widgets-wallpaper-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建立暫存目錄失敗");
        dir
    }

    fn write_wallpaper_config(dir: &std::path::Path, marker: &str) {
        let mut cfg: Value =
            serde_json::from_str(crate::wallpaper_config::DEFAULT_CONFIG_JSON).expect("JSON");
        cfg["thresholds"] = serde_json::json!({ "marker": marker });
        std::fs::write(
            dir.join(crate::wallpaper_config::CONFIG_FILE_NAME),
            serde_json::to_vec_pretty(&cfg).expect("序列化失敗"),
        )
        .expect("寫入設定檔失敗");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    const WALLPAPER_TW_EVENTS: &str = r#"{"updated":"2026-10-02 14:00","fetched":"2026-10-02 14:00",
        "macro":[{"title":"CPI"}],"events":[{"code":"2330"}],"punish":[],"quotes":[{"name":"USD/TWD"}],
        "twii_daily":[{"d":"2026-10-02"}],"holidays":["2026-10-10"]}"#;

    #[test]
    fn snapshot_for_label_rejects_wallpaper_queries_from_non_renderer_webviews() {
        let dir = wallpaper_test_dir("snapshot-gate");
        std::fs::write(dir.join("tw_events.json"), WALLPAPER_TW_EVENTS).expect("寫檔失敗");
        write_wallpaper_config(&dir, "gate");
        let mut registry = data::build_default_registry(&dir);
        registry.watch_wallpaper_config(&dir.join(crate::wallpaper_config::CONFIG_FILE_NAME));
        registry.poll_all();

        for label in ["w-clock", "w-macro", "w-custom1", "settings", "w-probe"] {
            let denied = snapshot_for_label(&registry, label, data::WALLPAPER_CHANNEL.to_string());
            assert!(denied.is_err(), "{label} 查 wallpaper 應被拒絕：{denied:?}");
        }
        let allowed = snapshot_for_label(
            &registry,
            "wallpaper-renderer-7",
            data::WALLPAPER_CHANNEL.to_string(),
        )
        .expect("渲染視窗查得到 wallpaper");
        assert_eq!(allowed.status, "ok");
        let payload = allowed.data.expect("ok 有 data");
        assert_eq!(payload["config"]["thresholds"]["marker"], "gate");
        assert_eq!(
            payload["data"]["holidays"],
            serde_json::json!(["2026-10-10"])
        );
        assert!(payload["data"].get("macro").is_none(), "只帶擷取的鍵");

        // 既有通道照舊：小工具查 tw-events 拿到完整快照。
        let tw = snapshot_for_label(&registry, "w-macro", data::TW_EVENTS_CHANNEL.to_string())
            .expect("tw-events 照舊放行");
        assert_eq!(tw.status, "ok");
        assert!(tw.data.expect("data")["macro"].is_array());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 負向測試（核心層）：訂閱表裡即使出現「非渲染視窗卻登記 wallpaper」的項目（偽造／日後改壞
    /// subscribe_data），`poll_and_notify` 也只投遞給渲染視窗；小工具只收到自己的 tw-events。設定檔
    /// 變更（資料未變）也要推新快照給存活的渲染視窗。
    #[test]
    fn poll_and_notify_delivers_wallpaper_only_to_renderer_subscribers() {
        let dir = wallpaper_test_dir("notify-gate");
        std::fs::write(dir.join("tw_events.json"), WALLPAPER_TW_EVENTS).expect("寫檔失敗");
        write_wallpaper_config(&dir, "v1");
        let settings = Settings {
            data_dir: dir.clone(),
            ..Settings::default()
        };
        let state = AppState::new(settings, dir.join("settings.json"));
        let (renderer, renderer_rx) = recording_data_channel();
        let (widget, widget_rx) = recording_data_channel();
        let (forged, forged_rx) = recording_data_channel();
        {
            let mut subscribers = state
                .data_subscribers
                .lock()
                .expect("data_subscribers mutex poisoned");
            subscribers.insert(
                "wallpaper-renderer-3".to_string(),
                (data::WALLPAPER_CHANNEL.to_string(), renderer),
            );
            subscribers.insert(
                "w-macro".to_string(),
                (data::TW_EVENTS_CHANNEL.to_string(), widget),
            );
            subscribers.insert(
                "w-probe".to_string(),
                (data::WALLPAPER_CHANNEL.to_string(), forged),
            );
        }

        poll_and_notify(&state);

        let renderer_events = renderer_rx.lock().expect("mutex").clone();
        assert_eq!(renderer_events.len(), 1, "{renderer_events:?}");
        assert_eq!(renderer_events[0]["channel"], data::WALLPAPER_CHANNEL);
        let payload = &renderer_events[0]["snapshot"]["data"];
        assert_eq!(payload["config"]["thresholds"]["marker"], "v1");
        assert_eq!(payload["data"]["twii_daily"][0]["d"], "2026-10-02");

        let widget_events = widget_rx.lock().expect("mutex").clone();
        assert_eq!(widget_events.len(), 1, "{widget_events:?}");
        assert_eq!(widget_events[0]["channel"], data::TW_EVENTS_CHANNEL);
        assert!(
            forged_rx.lock().expect("mutex").is_empty(),
            "非渲染視窗即使登記了 wallpaper 也不得收到"
        );

        // 只改設定檔：渲染視窗收到新快照，小工具不受打擾。
        write_wallpaper_config(&dir, "v2");
        poll_and_notify(&state);
        let renderer_events = renderer_rx.lock().expect("mutex").clone();
        assert_eq!(renderer_events.len(), 2, "{renderer_events:?}");
        assert_eq!(
            renderer_events[1]["snapshot"]["data"]["config"]["thresholds"]["marker"],
            "v2"
        );
        assert_eq!(widget_rx.lock().expect("mutex").len(), 1);
        assert!(forged_rx.lock().expect("mutex").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// task 4.6 修正輪 3（複審 M1）：設定檔讀到損壞時的重讀等待在**鎖外**——等待期間別的執行緒
    /// （主執行緒的 `get_snapshot`／`update_settings`）取得到 registry 鎖；等過之後的第二輪才做最終
    /// 判定（仍壞 → 內建預設並推送）。
    #[test]
    fn poll_and_notify_settled_waits_for_config_retry_outside_the_registry_lock() {
        let dir = wallpaper_test_dir("settled-lock");
        std::fs::write(dir.join("tw_events.json"), WALLPAPER_TW_EVENTS).expect("寫檔失敗");
        write_wallpaper_config(&dir, "v1");
        let state = AppState::new(
            Settings {
                data_dir: dir.clone(),
                ..Settings::default()
            },
            dir.join("settings.json"),
        );
        let first = poll_and_notify_settled(&state, |_| panic!("沒有問題不應等待"));
        assert!(first.contains(&data::WALLPAPER_CHANNEL.to_string()));

        std::fs::write(
            dir.join(crate::wallpaper_config::CONFIG_FILE_NAME),
            b"{ \"version\": 1, oops",
        )
        .expect("寫檔失敗");
        std::thread::sleep(std::time::Duration::from_millis(20));
        let mut waits = Vec::new();
        let changed = poll_and_notify_settled(&state, |delay| {
            waits.push(delay);
            // 等待期間：另一個執行緒取得到 registry 鎖，且值還沒退回（只標記了待重讀）。
            let (acquired, marker) = std::thread::scope(|s| {
                s.spawn(|| match state.registry.try_lock() {
                    Ok(registry) => (
                        true,
                        registry
                            .snapshot(data::WALLPAPER_CHANNEL)
                            .map(|snap| snap.data["config"]["thresholds"]["marker"].clone()),
                    ),
                    Err(_) => (false, None),
                })
                .join()
                .expect("thread")
            });
            assert!(acquired, "重讀等待期間 registry 鎖必須是空的");
            assert_eq!(marker, Some(serde_json::json!("v1")), "等待期間仍是上一份");
        });
        assert_eq!(waits, vec![crate::wallpaper_config::CONFIG_RETRY_DELAY]);
        assert!(
            changed.contains(&data::WALLPAPER_CHANNEL.to_string()),
            "第二輪退回內建預設＝wallpaper 變更：{changed:?}"
        );
        let registry = state.registry.lock().expect("registry");
        let snap = registry
            .snapshot(data::WALLPAPER_CHANNEL)
            .expect("wallpaper");
        assert!(
            snap.data["config"]["thresholds"].get("marker").is_none(),
            "仍損壞 → 內建預設"
        );
        drop(registry);

        // 存檔競態：等待期間寫完 → 第二輪直接採用新內容。
        std::fs::write(
            dir.join(crate::wallpaper_config::CONFIG_FILE_NAME),
            b"{ \"version\": 1, half",
        )
        .expect("寫檔失敗");
        std::thread::sleep(std::time::Duration::from_millis(20));
        poll_and_notify_settled(&state, |_| write_wallpaper_config(&dir, "v2"));
        let registry = state.registry.lock().expect("registry");
        let snap = registry
            .snapshot(data::WALLPAPER_CHANNEL)
            .expect("wallpaper");
        assert_eq!(snap.data["config"]["thresholds"]["marker"], "v2");
        drop(registry);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 渲染視窗每次一個新 label：銷毀時移除訂閱，訂閱表不會隨渲染次數一直變長；小工具的訂閱不動。
    #[test]
    fn forget_renderer_subscription_removes_only_renderer_entries() {
        let dir = wallpaper_test_dir("forget");
        let state = AppState::new(
            Settings {
                data_dir: dir.clone(),
                ..Settings::default()
            },
            dir.join("settings.json"),
        );
        {
            let mut subscribers = state
                .data_subscribers
                .lock()
                .expect("data_subscribers mutex poisoned");
            subscribers.insert(
                "wallpaper-renderer-9".to_string(),
                (
                    data::WALLPAPER_CHANNEL.to_string(),
                    recording_data_channel().0,
                ),
            );
            subscribers.insert(
                "w-macro".to_string(),
                (
                    data::TW_EVENTS_CHANNEL.to_string(),
                    recording_data_channel().0,
                ),
            );
        }
        assert!(!forget_renderer_subscription(&state, "w-macro"));
        assert!(forget_renderer_subscription(&state, "wallpaper-renderer-9"));
        assert!(!forget_renderer_subscription(
            &state,
            "wallpaper-renderer-9"
        ));
        let subscribers = state
            .data_subscribers
            .lock()
            .expect("data_subscribers mutex poisoned");
        assert_eq!(
            subscribers.keys().cloned().collect::<Vec<_>>(),
            vec!["w-macro".to_string()]
        );
        drop(subscribers);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn widget_url_carries_id_in_query() {
        assert_eq!(widget_url("clock"), "widget.html?w=clock");
    }

    // ── Rust 清單與設定／前端 registry.js 一致 ─────────────────────────────────

    #[test]
    fn spec_ids_match_settings_widget_ids_in_order() {
        let ids: Vec<&str> = WIDGET_SPECS.iter().map(|s| s.id).collect();
        assert_eq!(ids, settings::WIDGET_IDS.to_vec());
    }

    /// design.md D6（task 7.2）：前端 `registry.js` 只列 id 與通道，兩端比對這兩項。
    /// `host/ui/registry.js` 的 `WIDGETS` 解析成 `(id, channel, defaultEnabled)`，依檔案順序。
    /// 明列項目逐一截到該項自己的 `}` 為止（fix F1，review 7.2 L：原本從 id 一路找到檔尾，後面
    /// 其他項目的 channel 也會命中）；customN 區塊依 `channel: id`、`defaultEnabled` 產生。
    fn parse_registry_js(registry: &str) -> Vec<(String, String, bool)> {
        let start = registry
            .find("export const WIDGETS")
            .expect("找不到 WIDGETS");
        let end = start
            + registry[start..]
                .find("\n];")
                .expect("找不到 WIDGETS 陣列結尾");
        let list = &registry[start..end];
        let quoted = |s: &str, key: &str| -> Option<String> {
            let at = s.find(key)? + key.len();
            let rest = &s[at..];
            Some(rest[..rest.find('\'')?].to_string())
        };
        let bool_after = |s: &str, key: &str| -> bool {
            let at = s.find(key).unwrap_or_else(|| panic!("缺 {key}：{s}")) + key.len();
            let rest = s[at..].trim_start();
            if rest.starts_with("true") {
                true
            } else if rest.starts_with("false") {
                false
            } else {
                panic!("{key} 不是布林值：{s}")
            }
        };

        let mut entries = Vec::new();
        let mut rest = list;
        while let Some(at) = rest.find("{ id: '") {
            let entry_end = at + rest[at..].find('}').expect("項目缺 }");
            let entry = &rest[at..=entry_end];
            entries.push((
                quoted(entry, "id: '").expect("缺 id"),
                quoted(entry, "channel: '").unwrap_or_else(|| panic!("缺 channel：{entry}")),
                bool_after(entry, "defaultEnabled:"),
            ));
            rest = &rest[entry_end + 1..];
        }
        if let Some(at) = rest.find("...[") {
            let ids_end = at + rest[at..].find(']').expect("customN 陣列缺 ]");
            let ids: Vec<String> = rest[at + 4..ids_end]
                .split(',')
                .map(|s| s.trim().trim_matches('\'').to_string())
                .collect();
            let block = &rest[ids_end..];
            assert!(block.contains("channel: id"), "customN 的通道應與 id 同名");
            let enabled = bool_after(block, "defaultEnabled:");
            for id in ids {
                entries.push((id.clone(), id, enabled));
            }
        }
        entries
    }

    /// fix min-height（沿用到 widget-adaptive-zoom-and-grid D1）：最小框高度是整扇視窗的 CSS
    /// 高度（倍率由視窗矩形算），而 `#widget-root` 上下各有 `--widget-gap` 透明邊距（visual fix
    /// `443718d`），面板實際可用高度＝min 高 − 2×gap。gap 的數值以 widget.css 為準，這裡解析出來
    /// 核對，改 CSS 會讓本測試失敗、提醒同步調整。
    #[test]
    fn zoom_box_min_heights_include_widget_gap() {
        let css = include_str!("../ui/widget.css");
        let gap: f64 = css
            .split("--widget-gap:")
            .nth(1)
            .and_then(|rest| rest.split("px").next())
            .and_then(|v| v.trim().parse().ok())
            .expect("widget.css 應宣告 --widget-gap: <n>px");
        assert_eq!(
            gap, WIDGET_GAP_CSS_PX,
            "WIDGET_GAP_CSS_PX 應與 widget.css 一致"
        );
        // 面板內容最小高度（不含 gap）：時鐘＝task 2.1 實測視窗需求 155.1875（已含 gap）取整
        // 加 4 餘裕＝160，扣 gap 得 144；行情條 `.ticker` 原固定高度 44；其餘為 tasks.md 7.2 初值。
        let panel_min = [
            ("clock", 144.0),
            ("macro", 200.0),
            ("fixed", 160.0),
            ("dynamic", 160.0),
            ("quotes", 44.0),
            ("custom1", 120.0),
            ("custom2", 120.0),
            ("custom3", 120.0),
            ("custom4", 120.0),
            ("custom5", 120.0),
        ];
        for (id, panel) in panel_min {
            let spec = widget_spec(id).expect("規格表應有此 id");
            assert_eq!(
                spec.zoom_box.min_height - 2.0 * gap,
                panel,
                "{id}：min 高扣掉上下 gap 後應留給面板 {panel}"
            );
        }
    }

    /// widget-adaptive-zoom-and-grid design.md D1 不變式：舒適框各軸不小於最小框（comfort 寬
    /// `None`＝不限，視為無限大）；否則自適應倍率在 font_scale 1 時就會被上限截住，「舒適框」
    /// 失去意義。所有長度為正的有限值。
    #[test]
    fn zoom_boxes_keep_comfort_at_least_min() {
        for spec in &WIDGET_SPECS {
            let b = spec.zoom_box;
            for v in [b.min_width, b.min_height, b.comfort_height] {
                assert!(v.is_finite() && v > 0.0, "{}：{b:?}", spec.id);
            }
            assert!(
                b.comfort_height >= b.min_height,
                "{}：comfort 高 < min 高",
                spec.id
            );
            if let Some(cw) = b.comfort_width {
                assert!(
                    cw.is_finite() && cw >= b.min_width,
                    "{}：comfort 寬 < min 寬",
                    spec.id
                );
            }
        }
    }

    /// 數值逐字對照 widget-adaptive-zoom-and-grid design.md D1 表格。
    #[test]
    fn zoom_boxes_match_design_d1_table() {
        let expected: [(&str, f64, f64, Option<f64>, f64); 10] = [
            ("clock", 212.0, 160.0, Some(212.0), 160.0),
            ("macro", 375.0, 216.0, Some(500.0), 324.0),
            ("fixed", 352.5, 176.0, Some(470.0), 264.0),
            ("dynamic", 352.5, 176.0, Some(470.0), 264.0),
            ("quotes", 992.0, 60.0, None, 60.0),
            ("custom1", 352.5, 136.0, Some(470.0), 204.0),
            ("custom2", 352.5, 136.0, Some(470.0), 204.0),
            ("custom3", 352.5, 136.0, Some(470.0), 204.0),
            ("custom4", 352.5, 136.0, Some(470.0), 204.0),
            ("custom5", 352.5, 136.0, Some(470.0), 204.0),
        ];
        for (spec, (id, min_w, min_h, comfort_w, comfort_h)) in WIDGET_SPECS.iter().zip(expected) {
            assert_eq!(spec.id, id);
            assert_eq!(
                spec.zoom_box,
                layout::ZoomBox {
                    min_width: min_w,
                    min_height: min_h,
                    comfort_width: comfort_w,
                    comfort_height: comfort_h,
                },
                "{id}"
            );
        }
    }

    #[test]
    fn specs_match_frontend_registry_js() {
        let registry = include_str!("../ui/registry.js");
        let parsed = parse_registry_js(registry);
        let expected: Vec<(String, String, bool)> = WIDGET_SPECS
            .iter()
            .map(|spec| {
                (
                    spec.id.to_string(),
                    spec.channel.to_string(),
                    Settings::default().widgets[spec.id].enabled,
                )
            })
            .collect();
        assert_eq!(
            parsed, expected,
            "registry.js 的 id 集合與順序、每個 id 的通道與預設開關，應與 WIDGET_SPECS／settings 一致"
        );
        // 版面欄位只放 Rust：registry.js 不得再有寬度、高度上限或預設位置。
        let list = &registry[registry
            .find("export const WIDGETS")
            .expect("找不到 WIDGETS")..];
        for field in ["width:", "maxHeight", "defaultPosition"] {
            assert!(
                !list.contains(field),
                "registry.js 不應再有版面欄位 `{field}`（design.md D6）"
            );
        }
    }

    /// 解析器本身的鑑別力：某一項的通道或預設開關被改掉時，解析結果要反映在**那一項**上，
    /// 不能被後面其他項目的同名字串蓋過。
    #[test]
    fn parse_registry_js_reads_each_entrys_own_channel() {
        let registry = include_str!("../ui/registry.js");
        let tampered = registry.replacen(
            "{ id: 'clock', channel: 'tw-events', defaultEnabled: true }",
            "{ id: 'clock', channel: 'other', defaultEnabled: false }",
            1,
        );
        assert_ne!(
            tampered, registry,
            "前提：registry.js 的 clock 項目格式未變"
        );
        let parsed = parse_registry_js(&tampered);
        assert_eq!(parsed[0], ("clock".to_string(), "other".to_string(), false));
        assert_eq!(parsed.len(), 10);
    }

    /// design.md D6：預設格座標兩兩不相交、全在界內，且在高寬比 ≥ 0.53 的多種工作區尺寸 ×
    /// 縮放比例下，每個小工具都不小於最小格數（D7；與 `layout::grid_tests` 同一性質，這裡以
    /// 正式的 `WIDGET_SPECS` × `settings::DEFAULT_GRID_RECTS` 組合再核一次）。
    #[test]
    fn default_grid_rects_disjoint_and_meet_min_size_on_tall_enough_workspaces() {
        use layout::{in_grid_bounds, meets_min_grid_size, rects_overlap, PhysicalRect};
        let rects = settings::DEFAULT_GRID_RECTS;
        for (i, a) in rects.iter().enumerate() {
            assert!(in_grid_bounds(*a), "{} 超界", WIDGET_SPECS[i].id);
            for (j, b) in rects.iter().enumerate().skip(i + 1) {
                assert!(
                    !rects_overlap(*a, *b),
                    "{} 與 {} 重疊",
                    WIDGET_SPECS[i].id,
                    WIDGET_SPECS[j].id
                );
            }
        }
        for (w, h) in [
            (2560, 1600),
            (3840, 2160),
            (1920, 1080),
            (2560, 1440),
            (1680, 1050),
            (2000, 1060),
        ] {
            for scale in [1.0, 1.25, 1.5, 1.75, 2.0] {
                let taskbar = (40.0_f64 * scale).round() as i32;
                let work_area = PhysicalRect {
                    x: 0,
                    y: 0,
                    width: w,
                    height: h - taskbar,
                };
                let ratio = f64::from(work_area.height) / f64::from(work_area.width);
                if ratio < 0.53 {
                    continue;
                }
                for (spec, rect) in WIDGET_SPECS.iter().zip(rects) {
                    assert!(
                        meets_min_grid_size(work_area, rect, scale, &spec.zoom_box),
                        "{}：{rect:?} 在 {work_area:?}＠{scale} 小於最小格數",
                        spec.id
                    );
                }
            }
        }
    }

    // ── 通道訂閱對照表 ───────────────────────────────────────────────────────

    #[test]
    fn tw_events_channel_has_five_finance_widget_subscribers() {
        let mut ids: Vec<&str> = WIDGET_SPECS
            .iter()
            .filter(|spec| spec.channel == data::TW_EVENTS_CHANNEL)
            .map(|spec| spec.id)
            .collect();
        ids.sort();
        assert_eq!(ids, vec!["clock", "dynamic", "fixed", "macro", "quotes"]);
    }

    #[test]
    fn custom_channel_has_exactly_its_own_widget_as_subscriber() {
        for n in 1..=5 {
            let channel = format!("custom{n}");
            let ids: Vec<&str> = WIDGET_SPECS
                .iter()
                .filter(|spec| spec.channel == channel)
                .map(|spec| spec.id)
                .collect();
            assert_eq!(ids, vec![channel.as_str()]);
        }
    }

    #[test]
    fn window_label_follows_w_dash_id_rule() {
        assert_eq!(window_label("clock"), "w-clock");
        assert_eq!(window_label("custom5"), "w-custom5");
    }

    // ── 投遞對象（fix round 1，Codex 2.7）：通道由 webview label 決定，頁面不能自選 ──────

    #[test]
    fn channel_for_label_maps_widget_windows_to_their_channel() {
        assert_eq!(channel_for_label("w-macro"), Some(data::TW_EVENTS_CHANNEL));
        assert_eq!(channel_for_label("w-quotes"), Some(data::TW_EVENTS_CHANNEL));
        assert_eq!(channel_for_label("w-custom1"), Some("custom1"));
        assert_eq!(channel_for_label("w-custom5"), Some("custom5"));
    }

    #[test]
    fn channel_for_label_rejects_non_widget_or_unknown_labels() {
        assert_eq!(channel_for_label("settings"), None);
        assert_eq!(channel_for_label("w-probe"), None);
        assert_eq!(channel_for_label("macro"), None, "缺 w- 前綴不是小工具視窗");
        assert_eq!(channel_for_label("w-"), None);
    }

    #[test]
    fn every_widget_label_round_trips_through_channel_for_label() {
        for spec in WIDGET_SPECS {
            assert_eq!(
                channel_for_label(&window_label(spec.id)),
                Some(spec.channel)
            );
        }
    }

    // ── SnapshotResponse 形狀（design.md D4）─────────────────────────────────

    #[test]
    fn snapshot_response_ok_shape_matches_d4() {
        let response = SnapshotResponse {
            channel: "tw-events".to_string(),
            status: "ok",
            data: Some(serde_json::json!({"macro": []})),
            meta: Some(SnapshotMeta {
                loaded_at: 1_700_000_000_000,
                fetched: Some("2026-09-28 08:00".to_string()),
                updated: Some("2026-09-28 08:00".to_string()),
            }),
            generation: 3,
        };
        let json = serde_json::to_value(&response).expect("序列化失敗");
        assert_eq!(json["channel"], "tw-events");
        assert_eq!(json["status"], "ok");
        assert_eq!(json["data"], serde_json::json!({"macro": []}));
        assert_eq!(json["meta"]["loadedAt"], 1_700_000_000_000u64);
        assert_eq!(json["meta"]["fetched"], "2026-09-28 08:00");
        assert!(
            json.get("meta").unwrap().get("loaded_at").is_none(),
            "meta 欄位須為 camelCase（loadedAt），不可殘留 snake_case 的 loaded_at"
        );
    }

    #[test]
    fn snapshot_response_empty_shape_has_null_data_and_meta() {
        let response = SnapshotResponse {
            channel: "custom1".to_string(),
            status: "empty",
            data: None,
            meta: None,
            generation: 4,
        };
        let json = serde_json::to_value(&response).expect("序列化失敗");
        assert_eq!(json["channel"], "custom1");
        assert_eq!(json["status"], "empty");
        assert!(json["data"].is_null());
        assert!(json["meta"].is_null());
        assert_eq!(json["generation"], 4u64, "empty 回覆也必須帶世代");
    }

    /// task 4.1 fix round 3（Codex task-4.1-fix-codex-r1.md [high]）：改資料目錄重建註冊表後，
    /// `get_snapshot` 的 empty 回覆必須帶**比重建前 ok 快照更新**的世代，頁面才能據此拒收
    /// 重建前緩衝的舊目錄資料；`data` 推送的 payload 也要帶世代。
    #[test]
    fn snapshot_response_generation_advances_across_registry_rebuild() {
        let base =
            std::env::temp_dir().join(format!("fc-host-widgets-generation-{}", std::process::id()));
        let old_dir = base.join("old");
        let new_dir = base.join("new");
        std::fs::create_dir_all(&old_dir).expect("建立暫存目錄失敗");
        std::fs::create_dir_all(&new_dir).expect("建立暫存目錄失敗");
        std::fs::write(old_dir.join("custom1.json"), r#"{"from":"old"}"#).expect("寫檔失敗");

        let mut registry = data::build_default_registry(&old_dir);
        registry.poll_all();
        let before = snapshot_response(&registry, "custom1".to_string());
        assert_eq!(before.status, "ok");

        registry.replace_with(data::build_default_registry(&new_dir));
        let after = snapshot_response(&registry, "custom1".to_string());
        assert_eq!(after.status, "empty");
        assert!(
            after.generation > before.generation,
            "重建後的 empty（generation={}）必須比重建前的 ok（generation={}）新",
            after.generation,
            before.generation
        );

        let snapshot = {
            std::fs::write(new_dir.join("custom1.json"), r#"{"from":"new"}"#).expect("寫檔失敗");
            registry.poll_all();
            registry
                .snapshot("custom1")
                .cloned()
                .expect("新目錄的檔案應被讀入")
        };
        let event = DataEvent {
            channel: "custom1".to_string(),
            generation: registry.generation(),
            snapshot: Some(snapshot),
        };
        let json = serde_json::to_value(&event).expect("序列化失敗");
        assert_eq!(json["generation"], after.generation);
        assert_eq!(json["snapshot"]["data"]["from"], "new");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 測試用的 `Channel<DataEvent>`：把每筆送出的 payload 解成 JSON 收進共用的 Vec。
    fn recording_data_channel() -> (Channel<DataEvent>, std::sync::Arc<Mutex<Vec<Value>>>) {
        let received = std::sync::Arc::new(Mutex::new(Vec::new()));
        let sink = std::sync::Arc::clone(&received);
        let channel = Channel::new(move |body| {
            if let tauri::ipc::InvokeResponseBody::Json(text) = body {
                let value: Value = serde_json::from_str(&text).expect("payload 應為 JSON");
                sink.lock().expect("received mutex poisoned").push(value);
            }
            Ok(())
        });
        (channel, received)
    }

    /// fix F3（review task-2.7-datadir-opus.md [medium]）：改 `data_dir` 重建註冊表後，排程執行緒
    /// 可能先輪詢到新目錄的檔案、送出新世代的 ok；之後才補推的 empty 若照樣送出，同世代的
    /// empty 會把 ok 蓋掉，小工具停在「尚無資料」直到檔案下次改寫。empty 只能推給「新世代
    /// 尚無快照」的通道；尚無快照的已訂閱通道仍要收到新世代的 empty。
    #[test]
    fn push_empty_after_rebuild_skips_channels_already_holding_new_generation_ok() {
        let base = std::env::temp_dir().join(format!(
            "fc-host-widgets-empty-after-ok-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let old_dir = base.join("old");
        let new_dir = base.join("new");
        std::fs::create_dir_all(&old_dir).expect("建立暫存目錄失敗");
        std::fs::create_dir_all(&new_dir).expect("建立暫存目錄失敗");
        std::fs::write(new_dir.join("custom1.json"), r#"{"from":"new"}"#).expect("寫檔失敗");

        let settings = Settings {
            data_dir: old_dir.clone(),
            ..Settings::default()
        };
        let state = AppState::new(settings, base.join("settings.json"));
        let (with_file, with_file_rx) = recording_data_channel();
        let (without_file, without_file_rx) = recording_data_channel();
        {
            let mut subscribers = state
                .data_subscribers
                .lock()
                .expect("data_subscribers mutex poisoned");
            subscribers.insert("w-custom1".to_string(), ("custom1".to_string(), with_file));
            subscribers.insert(
                "w-custom2".to_string(),
                ("custom2".to_string(), without_file),
            );
        }

        let generation = {
            let mut registry = state.registry.lock().expect("registry mutex poisoned");
            registry.replace_with(data::build_default_registry(&new_dir));
            registry.generation()
        };
        // 排程執行緒搶先：新世代的 ok 先送達。
        poll_and_notify(&state);
        // 之後才輪到重建後的 empty 補推。
        {
            let registry = state.registry.lock().expect("registry mutex poisoned");
            push_empty_snapshots_after_rebuild(&state, &registry);
        }

        let with_file_events = with_file_rx
            .lock()
            .expect("received mutex poisoned")
            .clone();
        assert_eq!(
            with_file_events.len(),
            1,
            "已有新世代 ok 的通道不可再收到同世代 empty：{with_file_events:?}"
        );
        assert_eq!(with_file_events[0]["generation"], generation);
        assert_eq!(with_file_events[0]["snapshot"]["data"]["from"], "new");

        let without_file_events = without_file_rx
            .lock()
            .expect("received mutex poisoned")
            .clone();
        assert_eq!(
            without_file_events.len(),
            1,
            "新世代尚無快照的通道要收到一筆 empty：{without_file_events:?}"
        );
        assert_eq!(without_file_events[0]["generation"], generation);
        assert!(without_file_events[0]["snapshot"].is_null());

        let _ = std::fs::remove_dir_all(&base);
    }

    /// task 2.7 follow-up：`DataEvent.snapshot` 為 `None`（重建後推送 empty）序列化時
    /// `snapshot` 欄位須為 `null`——前端 `widget.html` 依 `payload.snapshot === null` 判斷
    /// 這是一筆 empty 推送（見該檔案 `data` 監聽器的正規化邏輯）。
    #[test]
    fn data_event_with_none_snapshot_serializes_snapshot_as_null() {
        let event = DataEvent {
            channel: "tw-events".to_string(),
            generation: 7,
            snapshot: None,
        };
        let json = serde_json::to_value(&event).expect("序列化失敗");
        assert_eq!(json["channel"], "tw-events");
        assert_eq!(json["generation"], 7u64);
        assert!(
            json["snapshot"].is_null(),
            "empty 推送的 snapshot 必須是 null"
        );
    }

    // ── 開機自啟：只在使用者切換開關時同步；顯示以登錄實況為準（2026-10-01） ──────────────

    /// 假的 Run 值：`registered`＝登錄裡有沒有 `fc-host` 值；`read_fails`＝讀登錄失敗；
    /// `write_fails`＝寫入／刪除被拒（例如企業原則鎖住 Run 機碼）。
    #[derive(Default)]
    struct FakeRunRegistry {
        ops: Mutex<Vec<String>>,
        registered: Mutex<bool>,
        read_fails: bool,
        write_fails: bool,
    }

    impl FakeRunRegistry {
        fn with_registered(registered: bool) -> Self {
            FakeRunRegistry {
                registered: Mutex::new(registered),
                ..FakeRunRegistry::default()
            }
        }
    }

    impl AutostartRegistry for FakeRunRegistry {
        fn set_run_value(&self, command: &str) -> Result<(), String> {
            if self.write_fails {
                return Err("存取被拒".to_string());
            }
            self.ops.lock().unwrap().push(format!("set {command}"));
            *self.registered.lock().unwrap() = true;
            Ok(())
        }
        fn delete_run_value(&self) -> Result<(), String> {
            if self.write_fails {
                return Err("存取被拒".to_string());
            }
            self.ops.lock().unwrap().push("delete".to_string());
            *self.registered.lock().unwrap() = false;
            Ok(())
        }
        fn run_value_registered(&self) -> Result<bool, String> {
            if self.read_fails {
                return Err("讀取失敗".to_string());
            }
            Ok(*self.registered.lock().unwrap())
        }
    }

    /// 使用者決定（2026-10-01）：開機自啟登錄歸安裝檔，宿主啟動（含設定檔首次建立）一律不寫
    /// 登錄。啟動流程在 `main.rs` 的 setup 閉包裡、單元測試建不起來，故直接檢查原始碼：
    /// 不得呼叫任何會寫登錄的函式（`AutostartTrigger::Startup` 已刪除，型別本身擋住）。
    #[test]
    fn startup_never_syncs_autostart_registry() {
        let main_rs = include_str!("main.rs");
        for call in [
            "sync_autostart(",
            "commit_settings_patch(",
            "apply_autostart(",
            "set_run_value(",
            "delete_run_value(",
        ] {
            assert!(
                !main_rs.contains(call),
                "main.rs 不得在啟動時寫開機自啟登錄（{call}）"
            );
        }
    }

    /// review M1：`settings` 事件只能經 [`emit_settings`] 發出（payload 套用登錄實況）。
    /// 檢查會發事件的模組原始碼裡沒有直接以字面事件名 settings 呼叫 emit／emit_filter／emit_to，
    /// 且 [`SETTINGS_EVENT`] 只在 [`emit_settings`] 裡用來發事件。比對字串在執行期拼接，避免
    /// 本測試自己的原始碼命中。
    #[test]
    fn settings_event_is_emitted_only_through_emit_settings() {
        let sources = [
            ("widgets.rs", include_str!("widgets.rs")),
            ("main.rs", include_str!("main.rs")),
            ("tray.rs", include_str!("tray.rs")),
            ("desktop.rs", include_str!("desktop.rs")),
        ];
        let quoted = ["\"", "settings", "\""].concat();
        for (name, src) in sources {
            let compact: String = src.chars().filter(|c| !c.is_whitespace()).collect();
            for bare in [format!("emit({quoted}"), format!("emit_filter({quoted}")] {
                assert!(
                    !compact.contains(&bare),
                    "{name} 有繞過 emit_settings 的 {bare}"
                );
            }
            for line in src.lines().filter(|l| l.contains("emit_to(")) {
                assert!(
                    !line.contains(&quoted),
                    "{name} 以 emit_to 發 settings：{line}"
                );
            }
        }
        let via_const = ["emit(", "SETTINGS_EVENT"].concat();
        assert_eq!(
            include_str!("widgets.rs").matches(&via_const).count(),
            1,
            "SETTINGS_EVENT 只能在 emit_settings 裡發"
        );
    }

    /// review M1：編輯版面切換（[`switch_edit_mode`] 回傳的設定檔複本）與拖曳放開
    /// （[`drag_end_settings`] 的結果，同樣是設定檔複本）廣播的 payload，`autostart` 必須是
    /// 登錄實況：開發建置（設定檔 true、Run 值不存在）→ false；反過來 → true。
    #[test]
    fn settings_event_payload_shows_registered_autostart() {
        let mut edit_mode = false;
        let mut stored = Settings::default();
        assert!(stored.autostart);
        let changed = switch_edit_mode(&mut edit_mode, &mut stored, true, |_| Ok(()))
            .expect("切換成功")
            .expect("layout_locked 有變，要廣播");
        let reg = FakeRunRegistry::with_registered(false);
        assert!(!settings_event_payload(&changed, &reg).autostart);
        assert!(stored.autostart, "payload 不得改到設定檔本身");

        let stored_off = Settings {
            autostart: false,
            ..Settings::default()
        };
        let reg = FakeRunRegistry::with_registered(true);
        assert!(settings_event_payload(&stored_off, &reg).autostart);
    }

    fn fake_exe() -> Result<String, String> {
        Ok(r"D:\x\fc-host.exe".to_string())
    }

    /// review L1：寫 Run 值失敗 → 整筆回傳可理解的錯誤、不存檔、登錄不變（settings.html 據此
    /// 把開關還原；設定檔、記憶體與廣播都沒動，不會自相矛盾）。
    #[test]
    fn commit_settings_patch_fails_without_saving_when_registry_write_fails() {
        let reg = FakeRunRegistry {
            write_fails: true,
            ..FakeRunRegistry::default()
        };
        let saved = std::cell::Cell::new(false);
        let result = commit_settings_patch(
            &Settings::default(),
            &reg,
            &serde_json::json!({ "autostart": true }),
            |_, merged| Ok(merged),
            |_| {
                saved.set(true);
                Ok(())
            },
            &fake_exe,
        );
        let err = result.expect_err("寫入失敗必須回傳錯誤");
        assert!(err.starts_with(AUTOSTART_WRITE_FAILED), "錯誤訊息：{err}");
        assert!(!saved.get(), "寫入失敗不得存檔");
        assert!(!*reg.registered.lock().unwrap(), "登錄維持原狀");
    }

    /// 存檔失敗 → 把剛寫的登錄改回原值，回傳存檔錯誤（畫面還原後與登錄實況一致）。
    #[test]
    fn commit_settings_patch_rolls_back_registry_when_save_fails() {
        let reg = FakeRunRegistry::with_registered(false);
        let result = commit_settings_patch(
            &Settings::default(),
            &reg,
            &serde_json::json!({ "autostart": true }),
            |_, merged| Ok(merged),
            |_| Err("磁碟已滿".to_string()),
            &fake_exe,
        );
        assert_eq!(result.expect_err("存檔失敗要回報"), "磁碟已滿");
        assert!(!*reg.registered.lock().unwrap(), "登錄改回原值");
    }

    /// 成功：回傳的設定 `autostart` 與寫入後的登錄一致，只寫一次。
    #[test]
    fn commit_settings_patch_writes_once_and_stores_registered_value() {
        let reg = FakeRunRegistry::with_registered(false);
        let stored = commit_settings_patch(
            &Settings::default(),
            &reg,
            &serde_json::json!({ "autostart": true }),
            |_, merged| Ok(merged),
            |_| Ok(()),
            &fake_exe,
        )
        .expect("提交成功");
        assert!(stored.autostart);
        assert!(*reg.registered.lock().unwrap());
        assert_eq!(reg.ops.lock().unwrap().len(), 1);
    }

    /// dynamic-wallpaper task 4.7a 修正輪 1（審查 low 6）：主題改「不接管」在設定鎖內完成讀改寫與存檔，
    /// 走與 `update_settings` 相同的提交路徑；存的是鎖內的最新設定（不是先前的複本）。存檔失敗時記憶體仍
    /// 改成不接管（排程不會再接管）並回傳錯誤；已是不接管時什麼都不做。
    #[test]
    fn theme_none_commit_saves_latest_settings_while_holding_the_lock() {
        let reg = FakeRunRegistry::with_registered(false);
        let cell = Mutex::new(Settings {
            wallpaper_theme: settings::WallpaperTheme::Astrolabe,
            opacity: 0.5,
            ..Settings::default()
        });
        let saved = std::cell::RefCell::new(None::<Settings>);
        let (changed, result) = commit_theme_none(&cell, &reg, &fake_exe, |s| {
            assert!(cell.try_lock().is_err(), "存檔時必須持有設定鎖");
            *saved.borrow_mut() = Some(s.clone());
            Ok(())
        });
        assert!(result.is_ok());
        let stored = changed.expect("有改變");
        assert_eq!(stored.wallpaper_theme, settings::WallpaperTheme::None);
        let saved = saved.into_inner().expect("有存檔");
        assert_eq!(saved.wallpaper_theme, settings::WallpaperTheme::None);
        assert_eq!(saved.opacity, 0.5, "存的是鎖內最新的完整設定");
        assert_eq!(
            cell.lock().unwrap().wallpaper_theme,
            settings::WallpaperTheme::None
        );
        assert!(reg.ops.lock().unwrap().is_empty(), "不碰開機自啟登錄");

        // 已是不接管：不存檔。
        let (changed, result) =
            commit_theme_none(&cell, &reg, &fake_exe, |_| panic!("已是不接管時不應存檔"));
        assert!(changed.is_none() && result.is_ok());

        // 存檔失敗：記憶體仍改成不接管，回傳錯誤。
        let cell = Mutex::new(Settings {
            wallpaper_theme: settings::WallpaperTheme::Skyline,
            ..Settings::default()
        });
        let (changed, result) =
            commit_theme_none(&cell, &reg, &fake_exe, |_| Err("磁碟滿了".to_owned()));
        assert!(result.is_err());
        assert_eq!(
            changed.map(|s| s.wallpaper_theme),
            Some(settings::WallpaperTheme::None)
        );
        assert_eq!(
            cell.lock().unwrap().wallpaper_theme,
            settings::WallpaperTheme::None
        );
    }

    /// task 6.4（審查 R2b-L1）：主題改「不接管」的存檔失敗後重試——記憶體仍是「不接管」才存（鎖內的最新
    /// 設定），使用者已選回某個主題時不動、也不把主題改回「不接管」。
    #[test]
    fn theme_none_resave_saves_only_while_still_none() {
        let cell = Mutex::new(Settings {
            wallpaper_theme: settings::WallpaperTheme::None,
            opacity: 0.7,
            ..Settings::default()
        });
        let saved = std::cell::RefCell::new(None::<Settings>);
        let result = resave_theme_none(&cell, |s| {
            assert!(cell.try_lock().is_err(), "存檔時必須持有設定鎖");
            *saved.borrow_mut() = Some(s.clone());
            Ok(())
        });
        assert_eq!(result, Ok(true));
        let saved = saved.into_inner().expect("有存檔");
        assert_eq!(saved.wallpaper_theme, settings::WallpaperTheme::None);
        assert_eq!(saved.opacity, 0.7);

        assert_eq!(
            resave_theme_none(&cell, |_| Err("仍然寫不進去".to_owned())),
            Err("仍然寫不進去".to_owned())
        );

        let reselected = Mutex::new(Settings {
            wallpaper_theme: settings::WallpaperTheme::Skyline,
            ..Settings::default()
        });
        assert_eq!(
            resave_theme_none(&reselected, |_| panic!("使用者已選回主題，不應存檔")),
            Ok(false)
        );
        assert_eq!(
            reselected.lock().unwrap().wallpaper_theme,
            settings::WallpaperTheme::Skyline
        );
    }

    #[test]
    fn update_settings_syncs_only_when_autostart_actually_changed() {
        let updated = |previous| AutostartTrigger::SettingsUpdated { previous };
        assert_eq!(
            autostart_sync_target(updated(true), true),
            None,
            "沒改 → 不寫"
        );
        assert_eq!(autostart_sync_target(updated(false), false), None);
        assert_eq!(autostart_sync_target(updated(false), true), Some(true));
        assert_eq!(autostart_sync_target(updated(true), false), Some(false));
    }

    /// 顯示落差：沒有安裝檔（開發建置）時設定檔預設 `autostart=true`、登錄卻沒有值。設定視窗
    /// 拿到的 `autostart` 一律是登錄實況，不是設定檔裡的值。
    #[test]
    fn displayed_autostart_follows_the_run_value_not_the_stored_flag() {
        let stored_on = Settings::default();
        assert!(stored_on.autostart);
        let reg = FakeRunRegistry::with_registered(false);
        assert!(!with_registered_autostart(&stored_on, &reg).autostart);

        let stored_off = Settings {
            autostart: false,
            ..Settings::default()
        };
        let reg = FakeRunRegistry::with_registered(true);
        assert!(with_registered_autostart(&stored_off, &reg).autostart);
    }

    #[test]
    fn displayed_autostart_falls_back_to_stored_flag_when_registry_unreadable() {
        let reg = FakeRunRegistry {
            read_fails: true,
            ..FakeRunRegistry::default()
        };
        for stored in [true, false] {
            let settings = Settings {
                autostart: stored,
                ..Settings::default()
            };
            assert_eq!(with_registered_autostart(&settings, &reg).autostart, stored);
        }
    }

    /// 開發建置：設定檔 `true`、登錄沒值、開關顯示「關」。使用者打開開關 → 必須真的寫入
    /// （「前值」是登錄實況 false，不是設定檔的 true）。
    #[test]
    fn turning_on_without_a_registered_value_writes_the_run_value() {
        let reg = FakeRunRegistry::with_registered(false);
        let (base, merged) = merge_settings_patch(
            &Settings::default(),
            &reg,
            &serde_json::json!({ "autostart": true }),
        )
        .expect("合併失敗");
        assert_eq!(
            autostart_sync_target(
                AutostartTrigger::SettingsUpdated {
                    previous: base.autostart
                },
                merged.autostart
            ),
            Some(true)
        );
    }

    /// 開發建置下改別的設定：設定檔的 `true` 與登錄（沒值）不一致，也不得因此寫登錄。
    #[test]
    fn unrelated_patch_never_writes_even_if_stored_flag_disagrees_with_registry() {
        for registered in [true, false] {
            let stored = Settings {
                autostart: !registered,
                ..Settings::default()
            };
            let reg = FakeRunRegistry::with_registered(registered);
            let (base, merged) =
                merge_settings_patch(&stored, &reg, &serde_json::json!({ "opacity": 0.5 }))
                    .expect("合併失敗");
            assert_eq!(merged.opacity, 0.5);
            assert_eq!(
                autostart_sync_target(
                    AutostartTrigger::SettingsUpdated {
                        previous: base.autostart
                    },
                    merged.autostart
                ),
                None,
                "registered={registered}"
            );
        }
    }

    #[test]
    fn autostart_command_quotes_the_exe_path() {
        assert_eq!(
            autostart_command(r"C:\Users\John Smith\AppData\Local\fc-host\fc-host.exe"),
            r#""C:\Users\John Smith\AppData\Local\fc-host\fc-host.exe" --autostart"#
        );
    }

    #[test]
    fn apply_autostart_writes_only_the_run_value() {
        let reg = FakeRunRegistry::default();
        apply_autostart(&reg, true, r"D:\x\fc-host.exe").unwrap();
        apply_autostart(&reg, false, r"D:\x\fc-host.exe").unwrap();
        assert_eq!(
            *reg.ops.lock().unwrap(),
            vec![
                r#"set "D:\x\fc-host.exe" --autostart"#.to_string(),
                "delete".to_string()
            ]
        );
    }

    // ── get_snapshot／get_settings／update_settings／set_edit_mode：端對端（不含視窗）──
    //
    // 這四個是 `#[tauri::command]`，簽章裡的 `State`／`AppHandle` 需要真正的 Tauri app 才能
    // 建構，單元測試無法直接呼叫指令函式本身；上面已個別測過它們依賴的邏輯（AppState 的
    // 建構、SnapshotResponse 的形狀、settings::merge_patch／save）。指令函式本體只是這些邏輯
    // 的組裝＋上鎖，端對端行為（含「先 listen 再 get」與事件過濾）由 release build 的
    // `--self-test-ipc`（`src/self_test_ipc.rs`）驗證，證據見 `host/tools/evidence/2.7-*.log`
    // （task brief 要求的驗收方式）。

    #[test]
    fn app_state_new_builds_registry_from_given_data_dir() {
        use std::fs;

        let dir = std::env::temp_dir().join(format!(
            "fc-host-widgets-test-{}-app-state",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("建立暫存目錄失敗");

        let settings = Settings {
            data_dir: dir.clone(),
            ..Settings::default()
        };
        let state = AppState::new(settings, dir.join("settings.json"));

        let mut channels = state
            .registry
            .lock()
            .expect("registry mutex poisoned")
            .channels()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
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
        assert!(!*state.edit_mode.lock().expect("edit_mode mutex poisoned"));

        let _ = fs::remove_dir_all(&dir);
    }

    /// dynamic-wallpaper task 4.7a：啟動時的主題設定載入結果交給 `wallpaper` 通道的監看（路徑同
    /// `AppState::new`），第一輪照樣組出 `wallpaper` 快照、帶的是啟動時讀到的設定。
    #[test]
    fn app_state_with_wallpaper_config_primes_the_wallpaper_watch() {
        use std::fs;

        let dir = std::env::temp_dir().join(format!(
            "fc-host-widgets-test-{}-primed-config",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("建立暫存目錄失敗");
        let config_path = dir.join(crate::wallpaper_config::CONFIG_FILE_NAME);
        let startup = crate::wallpaper_config::load_for_startup(&config_path);
        assert!(config_path.exists());

        let settings = Settings {
            data_dir: dir.clone(),
            ..Settings::default()
        };
        let state =
            AppState::new(settings, dir.join("settings.json")).with_wallpaper_config(&startup);
        let mut registry = state.registry.lock().expect("registry mutex poisoned");
        assert_eq!(registry.wallpaper_config_version(), Some(0));
        let changed = registry.poll_all_changed();
        assert!(changed.contains(&crate::data::WALLPAPER_CHANNEL.to_string()));
        assert_eq!(
            registry
                .snapshot(crate::data::WALLPAPER_CHANNEL)
                .expect("wallpaper")
                .data["config"],
            startup.value
        );
        drop(registry);
        let _ = fs::remove_dir_all(&dir);
    }

    // ── task 7.3：report_content 只搬一個布林值；should_show 加 edit_mode 參數；格線推導與
    // 拖曳結束的純函式部分 ─────────────────────────────────────────────────────────

    #[test]
    fn widget_runtime_shows_only_when_neither_hide_reason_applies_outside_edit_mode() {
        let mut rt = WidgetRuntime::default();
        assert!(rt.should_show("w-clock", false));
        rt.content_empty.insert("w-clock".to_string());
        assert!(!rt.should_show("w-clock", false), "無內容 → 隱藏");
        rt.content_empty.clear();
        rt.no_space_hidden.insert("w-clock".to_string());
        assert!(!rt.should_show("w-clock", false), "空間不足 → 隱藏");
        assert!(
            rt.should_show("w-clock-r1", false),
            "以 label 區分，重建後的新視窗不受影響"
        );
    }

    #[test]
    fn widget_runtime_edit_mode_shows_no_content_widgets_but_not_no_space_ones() {
        // design.md D7「無內容」；specs/widget-host-windows「編輯版面時看得到無內容的小工具」：
        // 編輯版面期間，因無內容而隱藏者要顯示（讓使用者能看到並移動它），但因空間不足而暫時
        // 隱藏者（沒有格子）不算在內，兩種模式下都不顯示。
        let mut rt = WidgetRuntime::default();
        rt.content_empty.insert("w-quotes".to_string());
        assert!(
            rt.should_show("w-quotes", true),
            "編輯版面期間，無內容的小工具仍要顯示（頁面畫佔位外框）"
        );
        assert!(
            !rt.should_show("w-quotes", false),
            "離開編輯版面後，無內容的小工具恢復隱藏"
        );

        rt.no_space_hidden.insert("w-custom1".to_string());
        assert!(
            !rt.should_show("w-custom1", true),
            "空間不足（沒有格子）者，編輯版面期間也不顯示"
        );
        assert!(!rt.should_show("w-custom1", false));

        // 一般情況（有內容、有格子）不受 edit_mode 影響，兩種模式下都顯示。
        assert!(rt.should_show("w-clock", true));
        assert!(rt.should_show("w-clock", false));
    }

    /// fix F2（F1 範圍外發現）：頁面在視窗 present 之前就回報內容時，只記錄、不動視窗（建立中
    /// 的視窗由視窗工廠決定）；present 之後依記錄決定要不要藏。
    #[test]
    fn report_content_before_present_only_records() {
        let mut rt = WidgetRuntime::default();
        assert_eq!(
            rt.record_report_content("w-quotes", false, false),
            None,
            "尚未 present：不顯示也不隱藏"
        );
        assert!(rt.content_empty.contains("w-quotes"), "但無內容要記下來");
        assert_eq!(
            rt.record_report_content("w-clock", true, false),
            None,
            "有內容同樣不動建立中的視窗"
        );
    }

    #[test]
    fn report_content_after_present_follows_should_show() {
        let mut rt = WidgetRuntime::default();
        rt.presented.insert("w-quotes".to_string());
        assert_eq!(
            rt.record_report_content("w-quotes", false, false),
            Some(false)
        );
        assert_eq!(
            rt.record_report_content("w-quotes", false, true),
            Some(true),
            "編輯版面中無內容也顯示（佔位外框）"
        );
        assert_eq!(
            rt.record_report_content("w-quotes", true, false),
            Some(true)
        );
        rt.no_space_hidden.insert("w-quotes".to_string());
        assert_eq!(
            rt.record_report_content("w-quotes", true, false),
            Some(false),
            "空間不足者恆不顯示"
        );
    }

    #[test]
    fn present_after_empty_report_tells_factory_to_hide() {
        let mut rt = WidgetRuntime::default();
        rt.record_report_content("w-quotes", false, false);
        assert!(
            !rt.mark_presented("w-quotes", false),
            "present 前已回報無內容 → present 後要藏"
        );
        assert!(rt.presented.contains("w-quotes"));
        let mut rt = WidgetRuntime::default();
        rt.record_report_content("w-quotes", false, true);
        assert!(rt.mark_presented("w-quotes", true), "編輯版面中照常顯示");
        let mut rt = WidgetRuntime::default();
        assert!(rt.mark_presented("w-clock", false), "沒回報過 → 顯示");
    }

    /// fix F1（review 7.3 M2）：切換編輯版面時只翻轉「已 present、且因無內容而由核心記錄」的
    /// 視窗；建立中尚未 present 者（不論推導結果是放置或空間不足）與空間不足者一律不動，維持
    /// `relayout_all_widgets` 文件寫的不變量（不對建立中視窗誤呼叫 show_at_bottom）。
    #[test]
    fn edit_mode_visibility_change_only_flips_presented_no_content_windows() {
        let mut rt = WidgetRuntime::default();
        // 建立中：頁面已回報無內容，但視窗工廠還沒 present。
        rt.content_empty.insert("w-quotes".to_string());
        assert_eq!(
            rt.edit_mode_visibility_change("w-quotes", false, true),
            None,
            "尚未 present 的視窗不得被編輯版面切換顯示"
        );
        // 建立中且推導為空間不足（prepare_widget_hidden 只套樣式、不隱藏）：同樣不動。
        assert_eq!(
            rt.edit_mode_visibility_change("w-custom1", false, true),
            None
        );

        rt.presented.insert("w-quotes".to_string());
        assert_eq!(
            rt.edit_mode_visibility_change("w-quotes", false, true),
            Some(true),
            "已 present 的無內容視窗：進入編輯版面要顯示"
        );
        assert_eq!(
            rt.edit_mode_visibility_change("w-quotes", true, false),
            Some(false),
            "離開編輯版面要藏回去"
        );
        assert_eq!(rt.edit_mode_visibility_change("w-quotes", true, true), None);

        // 已 present、空間不足且無內容：編輯版面期間也不顯示（沒有格子）。
        rt.presented.insert("w-custom2".to_string());
        rt.content_empty.insert("w-custom2".to_string());
        rt.no_space_hidden.insert("w-custom2".to_string());
        assert_eq!(
            rt.edit_mode_visibility_change("w-custom2", false, true),
            None
        );

        // 已 present、有內容但目前不可見（例如空間不足剛解除、等 relayout 重顯示）：不是編輯
        // 版面要翻轉的集合，不動。
        rt.presented.insert("w-clock".to_string());
        assert_eq!(rt.edit_mode_visibility_change("w-clock", false, true), None);
        assert_eq!(
            rt.edit_mode_visibility_change("w-clock", false, false),
            None
        );
    }

    fn laptop_monitor() -> MonitorInfo {
        // 本機筆電 2560×1600＠175%，工作列約 84 實體像素。
        MonitorInfo {
            id: Some(settings::MonitorId::Device("laptop".to_string())),
            work_area: layout::PhysicalRect {
                x: 0,
                y: 0,
                width: 2560,
                height: 1516,
            },
            scale_factor: 1.75,
            is_primary: true,
        }
    }

    #[test]
    fn resolve_enabled_widgets_places_default_finance_widgets_at_their_default_grids() {
        let monitors = [laptop_monitor()];
        let resolved = resolve_enabled_widgets(&monitors, &Settings::default());
        let ids: Vec<&str> = resolved.iter().map(|(id, _)| *id).collect();
        assert_eq!(
            ids,
            vec!["clock", "macro", "fixed", "dynamic", "quotes"],
            "只推導開啟中者，依註冊表順序"
        );
        let mut physical = Vec::new();
        for (id, r) in &resolved {
            let ResolvedWidgetPlacement::Placed {
                rect,
                physical_rect,
                zoom,
                moved_from_elsewhere,
                ..
            } = r
            else {
                panic!("{id} 預設版面應放得下：{r:?}");
            };
            assert_eq!(
                Some(*rect),
                settings::default_grid_rect(id),
                "{id} 在記錄格子上"
            );
            assert!(!moved_from_elsewhere);
            assert!((0.5..=3.0).contains(zoom));
            physical.push(*physical_rect);
        }
        for (i, a) in physical.iter().enumerate() {
            for b in physical.iter().skip(i + 1) {
                let disjoint = a.x + a.width <= b.x
                    || b.x + b.width <= a.x
                    || a.y + a.height <= b.y
                    || b.y + b.height <= a.y;
                assert!(disjoint, "實體矩形相交：{a:?} / {b:?}");
            }
        }
    }

    /// fix F1（review 7.2 L）：顯示器列舉回空清單（顯示組態切換瞬間）時，重排與拖曳結束不得
    /// 把所有小工具判成空間不足——沒有推導結果，呼叫端維持現狀不動。
    #[test]
    fn resolve_for_relayout_is_none_when_monitor_list_is_empty() {
        assert_eq!(resolve_for_relayout(&[], &Settings::default()), None);
        let resolved = resolve_for_relayout(&[laptop_monitor()], &Settings::default())
            .expect("有顯示器時應有推導結果");
        assert_eq!(resolved.len(), 5);
        assert!(resolved
            .iter()
            .all(|(_, r)| matches!(r, ResolvedWidgetPlacement::Placed { .. })));
    }

    #[test]
    fn resolve_enabled_widgets_skips_disabled_and_includes_enabled_custom() {
        let mut s = Settings::default();
        s.widgets.get_mut("quotes").unwrap().enabled = false;
        s.widgets.get_mut("custom3").unwrap().enabled = true;
        let resolved = resolve_enabled_widgets(&[laptop_monitor()], &s);
        let ids: Vec<&str> = resolved.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, vec!["clock", "macro", "fixed", "dynamic", "custom3"]);
        assert!(resolution_for(&resolved, "quotes").is_none());
    }

    // ── task 7.4：系統匣「空間不足，暫時隱藏」選單文字（純函式）─────────────────────────

    #[test]
    fn no_space_hidden_menu_entries_only_includes_hidden_ones_with_display_name() {
        let resolved: Vec<(&'static str, ResolvedWidgetPlacement)> = vec![
            (
                "clock",
                ResolvedWidgetPlacement::Placed {
                    monitor_index: 0,
                    rect: layout::GridRect {
                        col: 0,
                        row: 0,
                        w: 1,
                        h: 1,
                    },
                    physical_rect: layout::PhysicalRect {
                        x: 0,
                        y: 0,
                        width: 1,
                        height: 1,
                    },
                    zoom: 1.0,
                    moved_from_elsewhere: false,
                },
            ),
            ("custom3", ResolvedWidgetPlacement::HiddenNoSpace),
            ("quotes", ResolvedWidgetPlacement::HiddenNoSpace),
        ];
        let entries = no_space_hidden_menu_entries(&resolved);
        assert_eq!(
            entries,
            vec![
                ("custom3", "空間不足，暫時隱藏：擴充插槽 3".to_string()),
                ("quotes", "空間不足，暫時隱藏：行情條".to_string()),
            ],
            "只含 HiddenNoSpace 者，依輸入順序，文字用 WidgetSpec::display_name"
        );
    }

    #[test]
    fn no_space_hidden_menu_entries_empty_when_nothing_hidden() {
        let resolved: Vec<(&'static str, ResolvedWidgetPlacement)> = vec![(
            "clock",
            ResolvedWidgetPlacement::Placed {
                monitor_index: 0,
                rect: layout::GridRect {
                    col: 0,
                    row: 0,
                    w: 1,
                    h: 1,
                },
                physical_rect: layout::PhysicalRect {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                },
                zoom: 1.0,
                moved_from_elsewhere: false,
            },
        )];
        assert!(no_space_hidden_menu_entries(&resolved).is_empty());
    }

    // ── task 7.4：開啟小工具找空位（design.md D7 記錄位置寫回時機第 2 點）───────────────

    fn grid(col: i32, row: i32, w: i32, h: i32) -> layout::GridRect {
        layout::GridRect { col, row, w, h }
    }

    #[test]
    fn place_newly_enabled_widgets_relocates_and_writes_back_when_record_overlaps() {
        // clock 開啟中，佔滿整台顯示器的一大塊；custom1 原本關閉、記錄格子與 clock 相交，
        // 這次 patch 把它打開——應找到空位並把新格子寫回 merged，其餘小工具不受影響。
        let monitor = laptop_monitor();
        let mut before = Settings::default();
        before.widgets.get_mut("clock").unwrap().placement =
            settings::WidgetPlacement::primary(grid(0, 0, 40, 40));
        let mut merged = before.clone();
        merged.widgets.get_mut("custom1").unwrap().enabled = true;
        merged.widgets.get_mut("custom1").unwrap().placement =
            settings::WidgetPlacement::primary(grid(0, 0, 5, 5));

        let result = place_newly_enabled_widgets(std::slice::from_ref(&monitor), &before, merged)
            .expect("有空位應成功");

        let new_rect = result.widgets["custom1"].placement.grid_rect();
        assert_ne!(
            new_rect,
            grid(0, 0, 5, 5),
            "應換到別處，而非留在相交的記錄格子"
        );
        assert!(!layout::rects_overlap(new_rect, grid(0, 0, 40, 40)));
        assert_eq!(
            result.widgets["clock"].placement.grid_rect(),
            grid(0, 0, 40, 40),
            "已開啟中的 clock 不受影響"
        );
        assert_eq!(
            result.widgets["macro"], before.widgets["macro"],
            "無關的小工具完全不受影響"
        );
    }

    #[test]
    fn place_newly_enabled_widgets_rejects_whole_patch_when_no_space_left() {
        // 用兩個已開啟的大矩形恰好只留下一個 5x5 的洞（手法同 layout.rs 的
        // find_slot_preferring_record_size_falls_back_to_min_size）；custom1、custom2 同一筆
        // patch 一起打開，記錄都與 clock 相交、記錄格數都是 5x5：custom1（註冊表順序在前）拿走
        // 唯一的洞，custom2 應該找不到任何空位、整筆失敗。
        let monitor = laptop_monitor();
        let mut before = Settings::default();
        before.widgets.get_mut("clock").unwrap().placement =
            settings::WidgetPlacement::primary(grid(0, 0, 48, 43));
        before.widgets.get_mut("macro").unwrap().placement =
            settings::WidgetPlacement::primary(grid(5, 43, 43, 5));
        let mut merged = before.clone();
        for id in ["custom1", "custom2"] {
            merged.widgets.get_mut(id).unwrap().enabled = true;
            merged.widgets.get_mut(id).unwrap().placement =
                settings::WidgetPlacement::primary(grid(0, 0, 5, 5));
        }

        let result = place_newly_enabled_widgets(std::slice::from_ref(&monitor), &before, merged);
        assert_eq!(
            result,
            Err("空間不足，請先調整版面".to_string()),
            "custom2 連最小格數都放不下時，整筆應拒絕"
        );
    }

    #[test]
    fn place_newly_enabled_widgets_leaves_record_untouched_when_target_monitor_missing() {
        // custom1 記錄在一台已拔除的顯示器（laptop_monitor 之外的其他 Device），即使記錄格子
        // 與其他開啟中小工具的記錄相交，也不應找空位、不應改記錄，交給 D9 推導處理。
        let monitor = laptop_monitor();
        let mut before = Settings::default();
        before.widgets.get_mut("clock").unwrap().placement =
            settings::WidgetPlacement::primary(grid(0, 0, 40, 40));
        let mut merged = before.clone();
        let unplugged = grid(0, 0, 5, 5);
        merged.widgets.get_mut("custom1").unwrap().enabled = true;
        merged.widgets.get_mut("custom1").unwrap().placement = settings::WidgetPlacement {
            monitor: settings::MonitorId::Device("unplugged".to_string()),
            col: unplugged.col,
            row: unplugged.row,
            w: unplugged.w,
            h: unplugged.h,
        };

        let result = place_newly_enabled_widgets(std::slice::from_ref(&monitor), &before, merged)
            .expect("所屬顯示器不存在不應被拒絕，只是不找空位");

        assert_eq!(
            result.widgets["custom1"].placement.grid_rect(),
            unplugged,
            "所屬顯示器不存在時記錄格子應原樣保留"
        );
    }

    #[test]
    fn place_newly_enabled_widgets_keeps_record_when_no_overlap() {
        let monitor = laptop_monitor();
        let before = Settings::default();
        let mut merged = before.clone();
        merged.widgets.get_mut("custom1").unwrap().enabled = true;
        // custom1 的預設格子（col0..14）本來就不與任何開啟中的財經小工具相交。
        let original = merged.widgets["custom1"].placement.grid_rect();

        let result = place_newly_enabled_widgets(std::slice::from_ref(&monitor), &before, merged)
            .expect("不相交應直接成功");
        assert_eq!(
            result.widgets["custom1"].placement.grid_rect(),
            original,
            "不相交時不應改記錄"
        );
    }

    #[test]
    fn place_newly_enabled_widgets_processes_in_registry_order_and_avoids_prior_new_position() {
        // 同一筆 patch 一起打開 custom1、custom2：兩者原記錄都與 clock 相交且完全相同（刻意
        // 製造「若各自獨立找空位、會找到同一格」的情境）。custom1（註冊表順序在前）先處理，
        // custom2 後處理時應該看得到 custom1 剛寫入的新位置，兩者最終不應互相重疊。
        let monitor = laptop_monitor();
        let mut before = Settings::default();
        before.widgets.get_mut("clock").unwrap().placement =
            settings::WidgetPlacement::primary(grid(0, 0, 40, 10));
        let mut merged = before.clone();
        let clashing = grid(0, 0, 5, 5);
        for id in ["custom1", "custom2"] {
            merged.widgets.get_mut(id).unwrap().enabled = true;
            merged.widgets.get_mut(id).unwrap().placement =
                settings::WidgetPlacement::primary(clashing);
        }

        let result = place_newly_enabled_widgets(std::slice::from_ref(&monitor), &before, merged)
            .expect("兩者都應該找得到空位");

        let r1 = result.widgets["custom1"].placement.grid_rect();
        let r2 = result.widgets["custom2"].placement.grid_rect();
        assert_ne!(r1, clashing);
        assert_ne!(r2, clashing);
        assert_ne!(
            r1, r2,
            "custom2 應看到 custom1 剛寫入的新位置，不應落在同一格"
        );
        assert!(!layout::rects_overlap(r1, r2), "兩者最終不應互相重疊");
    }

    #[test]
    fn drag_end_keeps_current_grid_size_and_writes_back_record() {
        let m = laptop_monitor();
        let s = Settings::default();
        // 把時鐘（16×10）拖到左上角附近：一格 53.33×31.58 實體像素，(110, 70) → col 2、row 2。
        let dropped = layout::PhysicalRect {
            x: 110,
            y: 70,
            width: 1,
            height: 1,
        };
        let updated =
            drag_end_settings(std::slice::from_ref(&m), &s, "clock", dropped).expect("應寫回");
        let p = &updated.widgets["clock"].placement;
        assert_eq!(p.monitor, settings::MonitorId::Device("laptop".to_string()));
        assert_eq!(
            (p.col, p.row, p.w, p.h),
            (2, 2, 16, 10),
            "保留 w/h，左上角對齊"
        );
        // 其他小工具與其他欄位不受影響。
        assert_eq!(updated.widgets["macro"], s.widgets["macro"]);
        assert_eq!(updated.opacity, s.opacity);
    }

    #[test]
    fn drag_end_keeps_displaced_widgets_actual_size_not_record_size() {
        // custom1 記錄格子（33×47）與財經小工具相交且排在它們之後 → 第二階段找空位；記錄格數
        // 放不下，退到最小格數。拖曳結束保留的是「目前推導出的」格數，不是記錄格數。
        let m = laptop_monitor();
        let mut s = Settings::default();
        {
            let c = s.widgets.get_mut("custom1").unwrap();
            c.enabled = true;
            c.placement.col = 15;
            c.placement.row = 1;
            c.placement.w = 33;
            c.placement.h = 47;
        }
        let resolved = resolve_enabled_widgets(std::slice::from_ref(&m), &s);
        let Some(ResolvedWidgetPlacement::Placed { rect, .. }) =
            resolution_for(&resolved, "custom1")
        else {
            panic!("custom1 應以最小格數放在剩餘空間：{resolved:?}");
        };
        assert_ne!((rect.w, rect.h), (33, 47), "前提：推導出的格數與記錄不同");

        let dropped = layout::PhysicalRect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        };
        let updated =
            drag_end_settings(std::slice::from_ref(&m), &s, "custom1", dropped).expect("應寫回");
        let p = &updated.widgets["custom1"].placement;
        assert_eq!((p.w, p.h), (rect.w, rect.h), "保留目前看得到的格數");
    }

    #[test]
    fn drag_end_on_unresolved_monitor_does_not_write_back() {
        let mut m = laptop_monitor();
        m.id = None;
        let dropped = layout::PhysicalRect {
            x: 110,
            y: 70,
            width: 1,
            height: 1,
        };
        assert!(drag_end_settings(
            std::slice::from_ref(&m),
            &Settings::default(),
            "clock",
            dropped
        )
        .is_err());
    }

    // ── fix monitor-id：2026-10-01 拔插後「放開位置不合法」的實況重建 ─────────────────
    //
    // 拔插第二輪後的實際狀態（host/tools/probe-monitor-ids.ps1 實測兩台都解析得到穩定識別）：
    // 外接 4K（主螢幕、工作區 3840×2088＠150%）上是預設格座標的總經日曆／台股固定／動態／
    // 行情條，時鐘的記錄在筆電（2560×1516＠175%，左上角 (3840, 4)）的 (2, 3, 16, 9)。
    // fix min-height 後時鐘設計最小高度含上下 gap（156），16×9 在 4K＠150% 低於最小格數，
    // 這組固定裝置改用 16×10（預設格座標同步加高一格、總經日曆下移到 row 12）。
    // 這組測試釘住：識別都解析得到時，不合法的放開是真的重疊／超界，而且原因要說得出來。

    fn replug_4k() -> MonitorInfo {
        MonitorInfo {
            id: Some(settings::MonitorId::Device("4k".to_string())),
            work_area: layout::PhysicalRect {
                x: 0,
                y: 0,
                width: 3840,
                height: 2088,
            },
            scale_factor: 1.5,
            is_primary: true,
        }
    }

    fn replug_laptop() -> MonitorInfo {
        MonitorInfo {
            id: Some(settings::MonitorId::Device("laptop".to_string())),
            work_area: layout::PhysicalRect {
                x: 3840,
                y: 4,
                width: 2560,
                height: 1516,
            },
            scale_factor: 1.75,
            is_primary: false,
        }
    }

    fn replug_settings() -> Settings {
        let mut s = Settings::default();
        for id in ["macro", "dynamic", "quotes"] {
            let p = &mut s.widgets.get_mut(id).unwrap().placement;
            p.monitor = settings::MonitorId::Device("4k".to_string());
        }
        s.widgets.get_mut("clock").unwrap().placement = settings::WidgetPlacement {
            monitor: settings::MonitorId::Device("laptop".to_string()),
            col: 2,
            row: 3,
            w: 16,
            h: 10,
        };
        s
    }

    /// 4K 工作區上 (col, row) 左上角、大小為「時鐘帶著筆電邏輯尺寸跨過來」的實體矩形
    /// （tao 在 WM_DPICHANGED 保持邏輯尺寸：854×285 ÷ 1.75 × 1.5 ≈ 732×244）。
    fn on_4k(col: i32, row: i32, width: i32, height: i32) -> layout::PhysicalRect {
        let wa = replug_4k().work_area;
        layout::PhysicalRect {
            x: layout::edge(wa.x, wa.width, col),
            y: layout::edge(wa.y, wa.height, row),
            width,
            height,
        }
    }

    #[test]
    fn replug_clock_back_to_4k_is_legal_only_where_its_16x9_footprint_fits() {
        let monitors = [replug_4k(), replug_laptop()];
        let s = replug_settings();

        let ok = drag_end_settings(&monitors, &s, "clock", on_4k(15, 1, 732, 244))
            .expect("原位（15,1）放得下 16×10");
        assert_eq!(
            ok.widgets["clock"].placement.monitor,
            settings::MonitorId::Device("4k".to_string())
        );

        // 視窗只有 244 實體像素高（約 5.6 格），看起來放得進總經日曆上方的空白，但保留的格數
        // 是 10 列：row 3 → 3＋10＝13 > 12，與總經日曆（15,12,16,30）重疊。
        let err = drag_end_settings(&monitors, &s, "clock", on_4k(15, 3, 732, 244))
            .expect_err("row 3 與總經日曆重疊")
            .to_string();
        assert!(err.contains("重疊"), "原因應寫出重疊：{err}");
        assert!(
            !err.contains("無穩定識別"),
            "識別已解析，不應歸因於識別：{err}"
        );
        assert!(
            err.contains("col=15 row=12 w=16 h=30"),
            "應點出相交的格子：{err}"
        );
    }

    /// fix drag-dpi：時鐘從筆電拖到 4K，拖曳中視窗即時變成 16×10 在 4K 上的實際大小
    /// （1280×435，不再是 tao 保持邏輯尺寸的 732×244），紅框預告以同一個大小、同一台顯示器判斷。
    #[test]
    fn drag_session_resizes_to_target_monitor_footprint_and_previews_with_it() {
        let monitors = vec![replug_4k(), replug_laptop()];
        let s = replug_settings();
        let start = layout::grid_rect_to_physical(
            replug_laptop().work_area,
            layout::GridRect {
                col: 2,
                row: 3,
                w: 16,
                h: 10,
            },
        );
        // 抓在視窗正中央。
        let grab_at = (start.x + start.width / 2, start.y + start.height / 2);
        let mut session = DragSession::new(0x1234, "clock", monitors.clone(), s.clone())
            .with_drag_start(start, grab_at);

        // 還在筆電上：大小不變、不改內容倍率。
        let m = session.moving(start, (grab_at.0 + 10, grab_at.1));
        let rect = m.rect.expect("有尺寸換算上下文");
        assert_eq!((rect.width, rect.height), (start.width, start.height));
        assert_eq!(m.preview, None);
        assert_eq!(m.zoom, None);

        // 拖進 4K、左上角落在 (15, 3)：變成 1280×391；row 3＋9 列與總經日曆（15,11,…）重疊。
        // 游標＝左上角＋新尺寸的一半（抓點在正中央；y 差 1 px 不影響對齊到最近格線）。
        let at = |col: i32, row: i32| {
            let wa = replug_4k().work_area;
            (
                layout::edge(wa.x, wa.width, col) + 640,
                layout::edge(wa.y, wa.height, row) + 218,
            )
        };
        let m = session.moving(start, at(15, 3));
        let rect = m.rect.expect("有尺寸換算上下文");
        assert_eq!((rect.width, rect.height), (1280, 435));
        // 換到 4K：內容倍率跟著換成 4K 上的值（新矩形 1280×435 在 150% 下的 content_zoom），
        // 所見即所得。
        let clock_box = widget_spec("clock").unwrap().zoom_box;
        let expected = layout::content_zoom(1280, 435, 1.5, &clock_box, s.font_scale);
        assert_eq!(m.zoom, Some(expected));
        assert_eq!(
            m.preview,
            Some(EditPreview {
                id: "clock",
                valid: false
            })
        );
        // 左上角 (15, 1)：16×10 放得下 → 合法。
        let m = session.moving(start, at(15, 1));
        let rect = m.rect;
        assert_eq!(rect.map(|r| (r.width, r.height)), Some((1280, 435)));
        assert_eq!(m.zoom, None, "目標沒變不重設倍率");
        assert_eq!(
            m.preview,
            Some(EditPreview {
                id: "clock",
                valid: true
            })
        );
        // 放開沿用同一個目標顯示器（顯示器組態沒變時）；組態變了就交回中心判定。
        assert_eq!(session.drop_target(&monitors), Some(0));
        assert_eq!(session.drop_target(&monitors[..1]), None);
        // 放開判定與預告一致：同一個矩形、同一台顯示器 → 寫回 4K (15, 1)。
        let updated =
            drag_end_settings_on(&monitors, &s, "clock", rect.unwrap(), Some(0)).expect("合法");
        let p = &updated.widgets["clock"].placement;
        assert_eq!(p.monitor, settings::MonitorId::Device("4k".to_string()));
        assert_eq!((p.col, p.row, p.w, p.h), (15, 1, 16, 10));

        // 拖回筆電：變回起始大小與起始倍率。
        let m = session.moving(start, grab_at);
        assert_eq!(m.rect, Some(start));
        let laptop_zoom =
            layout::content_zoom(start.width, start.height, 1.75, &clock_box, s.font_scale);
        assert_eq!(m.zoom, Some(laptop_zoom));
    }

    /// widget-adaptive-zoom-and-grid design.md D2：拖曳中的倍率用拖曳開始時快照的字級。時鐘的
    /// 舒適框＝最小框，字級 > 1 一律被上限截住，故用 0.7 驗證字級確實進了換算。
    #[test]
    fn drag_session_zoom_uses_font_scale_snapshot_from_drag_start() {
        let monitors = vec![replug_4k(), replug_laptop()];
        let mut s = replug_settings();
        s.font_scale = 0.7;
        let start = layout::grid_rect_to_physical(
            replug_laptop().work_area,
            layout::GridRect {
                col: 2,
                row: 3,
                w: 16,
                h: 10,
            },
        );
        let grab_at = (start.x + start.width / 2, start.y + start.height / 2);
        let mut session =
            DragSession::new(0x1234, "clock", monitors, s).with_drag_start(start, grab_at);
        let wa = replug_4k().work_area;
        let cursor = (
            layout::edge(wa.x, wa.width, 15) + 640,
            layout::edge(wa.y, wa.height, 1) + 218,
        );
        let m = session.moving(start, cursor);
        let clock_box = widget_spec("clock").unwrap().zoom_box;
        let at_07 = layout::content_zoom(1280, 435, 1.5, &clock_box, 0.7);
        let at_10 = layout::content_zoom(1280, 435, 1.5, &clock_box, 1.0);
        assert_ne!(at_07, at_10, "前提：字級 0.7 會改變這個矩形的倍率");
        assert_eq!(m.zoom, Some(at_07));
    }

    /// 推導出的倍率帶入設定的字級（widget-adaptive-zoom-and-grid design.md D1／D2）；放置位置
    /// 與字級無關（規格「字級設定不影響最小格數」：原本合法的版面不被移動或隱藏）。
    #[test]
    fn resolve_enabled_widgets_applies_font_scale_to_zoom_but_not_to_placement() {
        let monitor = laptop_monitor();
        let base = resolve_enabled_widgets(std::slice::from_ref(&monitor), &Settings::default());
        for font_scale in [0.7, 1.2, 1.5] {
            let s = Settings {
                font_scale,
                ..Settings::default()
            };
            let scaled = resolve_enabled_widgets(std::slice::from_ref(&monitor), &s);
            assert_eq!(base.len(), scaled.len());
            let mut zoom_changed = false;
            for ((id, a), (_, b)) in base.iter().zip(&scaled) {
                let (
                    ResolvedWidgetPlacement::Placed {
                        rect: ra,
                        physical_rect: pa,
                        zoom: za,
                        ..
                    },
                    ResolvedWidgetPlacement::Placed {
                        rect: rb,
                        physical_rect: pb,
                        zoom: zb,
                        ..
                    },
                ) = (a, b)
                else {
                    panic!("{id}：預設版面在筆電上應全部放置：{a:?} / {b:?}");
                };
                assert_eq!(ra, rb, "{id}：字級 {font_scale} 不應移動小工具");
                let spec = widget_spec(id).unwrap();
                assert_eq!(
                    *zb,
                    layout::content_zoom(
                        pb.width,
                        pb.height,
                        monitor.scale_factor,
                        &spec.zoom_box,
                        font_scale
                    ),
                    "{id}：倍率應帶入字級 {font_scale}"
                );
                assert_eq!(pa, pb);
                zoom_changed |= za != zb;
            }
            assert!(
                zoom_changed,
                "字級 {font_scale}：至少一個小工具的倍率應改變"
            );
        }
    }

    /// 沒有拖曳起點（讀不到視窗矩形／游標）時維持舊行為：不換尺寸、預告照中心判定。
    #[test]
    fn drag_session_without_start_does_not_resize() {
        let monitors = vec![replug_4k(), replug_laptop()];
        let mut session = DragSession::new(0x1234, "clock", monitors.clone(), replug_settings());
        let proposed = on_4k(15, 1, 732, 244);
        let m = session.moving(proposed, (0, 0));
        assert_eq!(m.rect, None);
        assert_eq!(m.preview, None);
        assert_eq!(m.zoom, None);
        assert_eq!(session.drop_target(&monitors), None);
    }

    #[test]
    fn replug_fixed_moved_two_cells_is_out_of_bounds_or_overlap_one_cell_is_fine() {
        let monitors = [replug_4k(), replug_laptop()];
        let s = replug_settings();
        // 台股固定事件預設 (32, 1, 15, 17)：四周只有一格空隙。
        for (col, row) in [(33, 1), (31, 1), (32, 0), (32, 2)] {
            assert!(
                drag_end_settings(&monitors, &s, "fixed", on_4k(col, row, 1200, 740)).is_ok(),
                "移一格 ({col},{row}) 應合法"
            );
        }
        let right = drag_end_settings(&monitors, &s, "fixed", on_4k(34, 1, 1200, 740))
            .expect_err("右移兩格：34＋15＝49 > 48")
            .to_string();
        assert!(right.contains("超出格線範圍"), "{right}");
        let left = drag_end_settings(&monitors, &s, "fixed", on_4k(30, 1, 1200, 740))
            .expect_err("左移兩格：與總經日曆重疊")
            .to_string();
        assert!(left.contains("重疊"), "{left}");
        let down = drag_end_settings(&monitors, &s, "fixed", on_4k(32, 3, 1200, 740))
            .expect_err("下移兩格：與動態事件重疊")
            .to_string();
        assert!(down.contains("重疊"), "{down}");
    }

    #[test]
    fn drop_on_unresolved_monitor_names_the_missing_identity() {
        let mut m4k = replug_4k();
        m4k.id = None;
        let err = drag_end_settings(
            &[m4k, replug_laptop()],
            &replug_settings(),
            "clock",
            on_4k(15, 1, 732, 244),
        )
        .expect_err("4K 無識別時不可寫回")
        .to_string();
        assert!(err.contains("顯示器無穩定識別"), "{err}");
    }

    // ── task 7.5：放開合法判斷、拖曳中紅框預告的去重 ─────────────────────────────

    /// 筆電工作區 (col, row) 左上角的實體座標，拖曳測試用。
    fn laptop_cell(col: i32, row: i32) -> layout::PhysicalRect {
        let wa = laptop_monitor().work_area;
        layout::PhysicalRect {
            x: layout::edge(wa.x, wa.width, col),
            y: layout::edge(wa.y, wa.height, row),
            width: 800,
            height: 300,
        }
    }

    #[test]
    fn drag_end_rejects_drop_overlapping_another_widgets_actual_position() {
        // 預設版面把時鐘拖到總經日曆（15,12,16,30）上 → 不合法，不寫回（呼叫端彈回）。
        let m = laptop_monitor();
        assert!(drag_end_settings(
            std::slice::from_ref(&m),
            &Settings::default(),
            "clock",
            laptop_cell(17, 20)
        )
        .is_err());
    }

    #[test]
    fn drag_end_rejects_drop_partially_past_right_edge_but_accepts_flush() {
        // fix round 1：放開會超出工作區 → 不合法（彈回）；剛好貼齊右緣（col+w＝48）合法。
        let m = laptop_monitor();
        let mut s = Settings::default();
        for (id, c) in s.widgets.iter_mut() {
            c.enabled = id == "clock";
        }
        assert!(
            drag_end_settings(std::slice::from_ref(&m), &s, "clock", laptop_cell(40, 20)).is_err()
        );
        let flush = drag_end_settings(std::slice::from_ref(&m), &s, "clock", laptop_cell(32, 20))
            .expect("貼齊右緣合法");
        assert_eq!(
            flush.widgets["clock"].placement.grid_rect(),
            grid(32, 20, 16, 10)
        );
    }

    /// 只開時鐘（記錄 0,0,16,9）與 custom1（記錄 0,0,14,8，與時鐘相交 → 第二階段換位）。
    fn clock_and_displaced_custom1() -> Settings {
        let mut s = Settings::default();
        for (id, c) in s.widgets.iter_mut() {
            c.enabled = id == "clock" || id == "custom1";
        }
        s.widgets.get_mut("clock").unwrap().placement =
            settings::WidgetPlacement::primary(grid(0, 0, 16, 9));
        s.widgets.get_mut("custom1").unwrap().placement =
            settings::WidgetPlacement::primary(grid(0, 0, 14, 8));
        s
    }

    #[test]
    fn drag_end_rejects_drop_on_native_record_of_a_displaced_widget() {
        let m = laptop_monitor();
        let s = clock_and_displaced_custom1();
        let resolved = resolve_enabled_widgets(std::slice::from_ref(&m), &s);
        let Some(ResolvedWidgetPlacement::Placed { rect, .. }) =
            resolution_for(&resolved, "custom1")
        else {
            panic!("custom1 應被換位放置：{resolved:?}");
        };
        assert_ne!(
            rect,
            grid(0, 0, 14, 8),
            "前提：custom1 實際位置不在記錄格子"
        );

        // 時鐘挪到 (2,2)：不碰 custom1 的實際位置，但壓到它（記錄就在這台）的記錄格子。
        assert!(
            drag_end_settings(std::slice::from_ref(&m), &s, "clock", laptop_cell(2, 2)).is_err()
        );
        // 挪到空白處 (20,20) 合法。
        let updated = drag_end_settings(std::slice::from_ref(&m), &s, "clock", laptop_cell(20, 20))
            .expect("空白處合法");
        assert_eq!(
            updated.widgets["clock"].placement.grid_rect(),
            grid(20, 20, 16, 9)
        );
    }

    #[test]
    fn edit_preview_tracker_starts_valid_and_reports_only_changes() {
        let mut t = EditPreviewTracker::default();
        assert_eq!(t.observe(true), None, "拖曳開始先當作合法，合法不發事件");
        assert_eq!(t.observe(false), Some(false));
        assert_eq!(t.observe(false), None, "未變化不重發");
        assert_eq!(t.observe(true), Some(true));
        assert_eq!(t.observe(true), None);
    }

    #[test]
    fn drag_session_preview_emits_edit_preview_only_on_validity_change() {
        let m = laptop_monitor();
        let mut session = DragSession::new(0x1234, "clock", vec![m.clone()], Settings::default());
        // 空白處（左側 0..14 欄預設沒有開啟中的小工具）：仍合法，不發事件。
        assert_eq!(session.preview(laptop_cell(0, 2)), None);
        // 壓到總經日曆：變為不合法。
        assert_eq!(
            session.preview(laptop_cell(17, 20)),
            Some(EditPreview {
                id: "clock",
                valid: false
            })
        );
        // 仍在總經日曆上（位置不同）：不重發。
        assert_eq!(session.preview(laptop_cell(18, 22)), None);
        // 回到空白處：恢復合法。
        assert_eq!(
            session.preview(laptop_cell(0, 2)),
            Some(EditPreview {
                id: "clock",
                valid: true
            })
        );
    }

    // ── task 7.6：編輯版面調整大小 ────────────────────────────────────────────────

    /// 筆電工作區上某個格子的實體矩形（調整大小的提議矩形）。
    fn laptop_grid(col: i32, row: i32, w: i32, h: i32) -> layout::PhysicalRect {
        layout::grid_rect_to_physical(laptop_monitor().work_area, grid(col, row, w, h))
    }

    /// 預設版面但關掉台股固定事件（時鐘右側 col 32 起空出來，可往右加寬）。
    fn defaults_without_fixed() -> Settings {
        let mut s = Settings::default();
        s.widgets.get_mut("fixed").unwrap().enabled = false;
        s
    }

    #[test]
    fn resize_end_widen_right_edge_two_cells_writes_back_record() {
        // spec「調整大小」：右緣向右加寬兩格、新範圍不重疊 → 寬度 +2，其餘邊不動。
        let m = laptop_monitor();
        let s = defaults_without_fixed();
        let mut proposed = laptop_grid(15, 1, 18, 10);
        proposed.width += 9; // 放開位置偏離格線幾個像素，round 回 col 33
        let updated = resize_end_settings(
            std::slice::from_ref(&m),
            &s,
            "clock",
            layout::ResizeEdges::RIGHT,
            proposed,
        )
        .expect("加寬兩格應合法");
        assert_eq!(
            updated.widgets["clock"].placement.grid_rect(),
            grid(15, 1, 18, 10)
        );
        assert_eq!(
            updated.widgets["clock"].placement.monitor,
            settings::MonitorId::Device("laptop".to_string())
        );
    }

    #[test]
    fn resize_end_below_min_size_or_into_neighbor_is_rejected() {
        let m = laptop_monitor();
        // 2560＠175%：時鐘（min 寬 212，門檻 106 邏輯 px，widget-adaptive-zoom-and-grid D3）
        // 最小寬 4 格（213 px＝121.7）；縮到 3 格（160 px＝91.4）→ 不合法（彈回）。舊模型設計寬
        // 500 時最小寬是 9 格，D1 實測時鐘 min 寬變窄後改用 3／4 格。
        let s = defaults_without_fixed();
        assert_eq!(
            layout::min_grid_size(
                m.work_area,
                m.scale_factor,
                &widget_spec("clock").unwrap().zoom_box
            )
            .0,
            4,
            "前提：時鐘在這台的最小寬格數"
        );
        assert!(resize_end_settings(
            std::slice::from_ref(&m),
            &s,
            "clock",
            layout::ResizeEdges::RIGHT,
            laptop_grid(15, 1, 3, 9),
        )
        .is_err());
        assert!(resize_end_settings(
            std::slice::from_ref(&m),
            &s,
            "clock",
            layout::ResizeEdges::RIGHT,
            laptop_grid(15, 1, 4, 9),
        )
        .is_ok());
        // 預設版面（fixed 在 col 32）：右緣加寬到 col 33 → 與 fixed 相交，不合法。
        assert!(resize_end_settings(
            std::slice::from_ref(&m),
            &Settings::default(),
            "clock",
            layout::ResizeEdges::RIGHT,
            laptop_grid(15, 1, 18, 9),
        )
        .is_err());
    }

    #[test]
    fn drag_session_resize_preview_tracks_validity_and_remembers_edges() {
        let m = laptop_monitor();
        let mut session = DragSession::new(0x1234, "clock", vec![m], defaults_without_fixed());
        assert_eq!(session.resize_edges(), None, "尚未收到 WM_SIZING＝移動");
        assert_eq!(
            session.preview_resize(layout::ResizeEdges::RIGHT, laptop_grid(15, 1, 18, 9)),
            None,
            "合法不發事件"
        );
        assert_eq!(session.resize_edges(), Some(layout::ResizeEdges::RIGHT));
        assert_eq!(
            // 時鐘在這台的最小寬 4 格（見 `resize_end_below_min_size_or_into_neighbor_is_rejected`）。
            session.preview_resize(layout::ResizeEdges::RIGHT, laptop_grid(15, 1, 3, 9)),
            Some(EditPreview {
                id: "clock",
                valid: false
            }),
            "縮到小於最小格數 → 紅框"
        );
        assert_eq!(
            session.preview_resize(layout::ResizeEdges::RIGHT, laptop_grid(15, 1, 2, 9)),
            None,
            "仍不合法不重發"
        );
    }

    #[test]
    fn drag_session_matches_only_its_own_hwnd() {
        let session =
            DragSession::new(0x1234, "clock", vec![laptop_monitor()], Settings::default());
        assert!(session.is_for(0x1234));
        assert!(!session.is_for(0x5678));
    }

    // ── fix F6b：拖曳結束延後判定 ────────────────────────────────────────────────

    /// 筆電上時鐘的起點矩形（`replug_settings` 的記錄 (2,3,16,10)）。
    fn replug_clock_start() -> layout::PhysicalRect {
        layout::grid_rect_to_physical(
            replug_laptop().work_area,
            layout::GridRect {
                col: 2,
                row: 3,
                w: 16,
                h: 10,
            },
        )
    }

    /// 從筆電起點抓正中央開始拖，拖進 4K 左上角 (15, 1)（合法位置），回傳 session 與
    /// 拖到的矩形。
    fn replug_clock_dragged_to_4k() -> (DragSession, layout::PhysicalRect) {
        let monitors = vec![replug_4k(), replug_laptop()];
        let start = replug_clock_start();
        let grab_at = (start.x + start.width / 2, start.y + start.height / 2);
        let mut session = DragSession::new(0x1234, "clock", monitors, replug_settings())
            .with_drag_start(start, grab_at);
        let wa = replug_4k().work_area;
        let cursor = (
            layout::edge(wa.x, wa.width, 15) + 640,
            layout::edge(wa.y, wa.height, 1) + 218,
        );
        let rect = session
            .moving(start, cursor)
            .rect
            .expect("有尺寸換算上下文");
        (session, rect)
    }

    /// Task A 修正第 1 輪（Codex review medium）：跨 DPI 拖曳時套用的新倍率必須同步寫進
    /// [`WidgetRuntime::zoom`]（`report_content` 重套的單一來源）。否則拖曳中頁面重載或內容由
    /// 空轉有時，`report_content(true)` 會把起始螢幕的舊倍率套回去，而 `zoomed_for` 又讓同一
    /// 目標螢幕不再重算——拖曳預覽就不是放開後的實際結果（widget-adaptive-zoom-and-grid
    /// design.md D2）。這裡走 [`update_widget_drag`] 同一個純函式 [`record_drag_move_zoom`]。
    #[test]
    fn report_content_after_cross_dpi_drag_does_not_revert_to_start_monitor_zoom() {
        let label = "w-clock";
        let s = replug_settings();
        let clock_box = widget_spec("clock").unwrap().zoom_box;
        let start = replug_clock_start();
        let start_zoom =
            layout::content_zoom(start.width, start.height, 1.75, &clock_box, s.font_scale);
        let mut runtime = WidgetRuntime::default();
        runtime.zoom.insert(label.to_string(), start_zoom);
        assert!(runtime.mark_presented(label, true));

        let grab_at = (start.x + start.width / 2, start.y + start.height / 2);
        let mut session = DragSession::new(0x1234, "clock", vec![replug_4k(), replug_laptop()], s)
            .with_drag_start(start, grab_at);
        let wa = replug_4k().work_area;
        let at = |row: i32| {
            (
                layout::edge(wa.x, wa.width, 15) + 640,
                layout::edge(wa.y, wa.height, row) + 218,
            )
        };
        // 拖進 4K：倍率換成 4K 上的值，並同步進快取。
        let moved = session.moving(start, at(1));
        let uhd_zoom = moved.zoom.expect("換到 4K 應回報新倍率");
        assert_ne!(uhd_zoom, start_zoom, "前提：兩台的倍率不同");
        record_drag_move_zoom(&mut runtime, label, &moved);
        assert_eq!(
            runtime.report_content_outcome(label, true, true),
            Some((true, Some(uhd_zoom))),
            "拖曳中 report_content 應重套 4K 的倍率，不是起始螢幕的"
        );
        // 同一目標繼續移動：不重算（zoom＝None），快取維持 4K 的倍率。
        let moved = session.moving(start, at(2));
        assert_eq!(moved.zoom, None);
        record_drag_move_zoom(&mut runtime, label, &moved);
        assert_eq!(
            runtime.report_content_outcome(label, true, true),
            Some((true, Some(uhd_zoom)))
        );
    }

    #[test]
    fn settled_drop_saves_the_rect_read_after_the_loop() {
        // 正常放開：延後判定時讀到的就是拖到的位置 → 寫回 4K (15, 1)。
        let monitors = [replug_4k(), replug_laptop()];
        let (session, dropped) = replug_clock_dragged_to_4k();
        match settle_drop(
            &monitors,
            &replug_settings(),
            "clock",
            Some(&session),
            dropped,
        ) {
            DropDecision::Save(updated) => {
                let p = &updated.widgets["clock"].placement;
                assert_eq!(p.monitor, settings::MonitorId::Device("4k".to_string()));
                assert_eq!((p.col, p.row, p.w, p.h), (15, 1, 16, 10));
            }
            other => panic!("應寫回：{other:?}"),
        }
    }

    #[test]
    fn settled_drop_back_at_start_is_unchanged_even_if_target_was_elsewhere() {
        // 跨顯示器拖曳後又拖回原位放開：延後判定時視窗在起點；拖曳中最後的目標是 4K，
        // 但結果必須是「沒有移動」，不得寫成另一台顯示器上的位置。
        let monitors = [replug_4k(), replug_laptop()];
        let (session, _) = replug_clock_dragged_to_4k();
        let decision = settle_drop(
            &monitors,
            &replug_settings(),
            "clock",
            Some(&session),
            replug_clock_start(),
        );
        assert!(
            matches!(decision, DropDecision::Unchanged),
            "回到起點＝未移動：{decision:?}"
        );
    }

    #[test]
    fn settled_resize_back_at_start_is_unchanged() {
        // 調整大小後又拉回原大小放開：同樣以延後讀到的矩形判定，回到起點＝未改變。
        let monitors = [replug_4k(), replug_laptop()];
        let start = replug_clock_start();
        let mut session = DragSession::new(0x1234, "clock", monitors.to_vec(), replug_settings())
            .with_drag_start(start, (start.x + start.width, start.y + 10));
        let mut wider = start;
        wider.width += 300;
        let _ = session.preview_resize(layout::ResizeEdges::RIGHT, wider);
        let decision = settle_drop(
            &monitors,
            &replug_settings(),
            "clock",
            Some(&session),
            start,
        );
        assert!(matches!(decision, DropDecision::Unchanged), "{decision:?}");
    }

    #[test]
    fn drag_tracker_defers_judgement_until_the_matching_settle() {
        let mut t = DragTracker::default();
        assert!(t.begin(0x1234).is_none(), "沒有待判定");
        t.attach(DragSession::new(
            0x1234,
            "clock",
            vec![replug_laptop()],
            replug_settings(),
        ));
        assert!(t.session_mut(0x1234).is_some());
        assert!(t.session_mut(0x5678).is_none(), "別扇視窗拿不到");
        assert!(t.holds(0x1234), "拖曳中的視窗不被重排");

        let (seq, superseded) = t.exit(0x1234);
        assert!(superseded.is_none());
        assert!(t.session_mut(0x1234).is_none(), "迴圈結束後不再是拖曳中");
        assert!(
            t.holds(0x1234),
            "待判定期間仍不被重排（以免讀到重排後的矩形）"
        );

        assert!(
            t.settle(0x1234, seq.wrapping_add(1)).is_none(),
            "過期的訊息不判定"
        );
        assert!(t.settle(0x5678, seq).is_none(), "別扇視窗不判定");
        let pending = t.settle(0x1234, seq).expect("對應的訊息取出待判定");
        assert_eq!(pending.hwnd, 0x1234);
        assert!(pending.session.as_ref().is_some_and(|s| s.is_for(0x1234)));
        assert!(t.settle(0x1234, seq).is_none(), "只判定一次");
        assert!(!t.holds(0x1234));
    }

    #[test]
    fn drag_tracker_hands_back_the_previous_pending_when_a_new_drag_begins() {
        // 延後期間同一扇（或另一扇）視窗又開始拖曳：前一筆交回呼叫端先判定（不丟棄），它
        // 自己那則 posted message 之後到達時已過期、不再判定第二次。
        let mut t = DragTracker::default();
        t.begin(0x1234);
        let (seq, _) = t.exit(0x1234);
        let previous = t.begin(0x1234).expect("前一筆待判定交回");
        assert_eq!(previous.hwnd, 0x1234);
        assert_eq!(previous.seq, seq);
        assert!(t.holds(0x1234), "新的拖曳中");
        assert!(t.settle(0x1234, seq).is_none(), "舊訊息到達時不再判定");

        let (seq2, _) = t.exit(0x1234);
        assert_ne!(seq2, seq, "每次結束一個新序號");
        let previous = t.begin(0x5678).expect("另一扇視窗開始拖曳也交回");
        assert_eq!(previous.hwnd, 0x1234);
        assert!(!t.holds(0x1234));
        assert!(t.holds(0x5678));
    }

    #[test]
    fn drag_tracker_forgets_a_destroyed_window_without_judging_it() {
        let mut t = DragTracker::default();
        t.begin(0x1234);
        let (seq, _) = t.exit(0x1234);
        t.forget(0x1234);
        assert!(t.settle(0x1234, seq).is_none(), "已銷毀：不判定、不寫回");
        assert!(!t.holds(0x1234));
        assert!(t.begin(0x5678).is_none(), "也不會在下一次拖曳開始時被交回");

        // 拖曳中被銷毀：結束訊息不會再來，下一次結束不得撿到它的 session。
        t.forget(0x5678);
        assert!(!t.holds(0x5678));
    }

    // ── 暫停原因集合（task 5.1）─────────────────────────────────────────────────

    #[test]
    fn resolve_pause_is_not_paused_when_reason_set_is_empty() {
        assert_eq!(resolve_pause(&HashSet::new()), (false, None));
    }

    #[test]
    fn resolve_pause_is_paused_with_the_active_reason_when_one_reason_active() {
        let mut reasons = HashSet::new();
        reasons.insert(PauseReason::Manual);
        assert_eq!(resolve_pause(&reasons), (true, Some(PauseReason::Manual)));
    }

    // ── get_pause（task 5.6 fix round 1）─────────────────────────────────────────

    /// fix F1（review 7.6 L1）：`resizable_now` 每次呼叫都讀當下的 `edit_mode` 與
    /// `layout_locked`——`create_widget_window` 把它包進排到主執行緒的閉包，在主執行緒**真正
    /// 執行時**才判斷，排隊期間離開編輯版面就不會殘留 `WS_SIZEBOX`。
    #[test]
    fn resizable_now_reflects_state_at_call_time() {
        let state = AppState::new(Settings::default(), PathBuf::from("unused-settings.json"));
        let set = |edit: bool, locked: bool| {
            *state.edit_mode.lock().expect("edit_mode mutex poisoned") = edit;
            state
                .settings
                .lock()
                .expect("settings mutex poisoned")
                .layout_locked = locked;
        };
        for (edit, locked, expected) in [
            (true, false, true),
            (true, true, false),
            (false, false, false),
            (false, true, false),
        ] {
            set(edit, locked);
            assert_eq!(
                resizable_now(&state),
                expected,
                "edit_mode={edit} layout_locked={locked}"
            );
        }
    }

    /// fix F2（review 5.5 low）：狀態沒有實際改變時不產生事件（design.md D4「`pause` 事件只在
    /// 變化時廣播」），呼叫端據此不廣播、不重建系統匣選單、不寫記錄。
    #[test]
    fn apply_pause_reason_reports_an_event_only_when_the_set_changes() {
        let mut reasons = HashSet::new();
        let first = apply_pause_reason(&mut reasons, PauseReason::Battery, true);
        assert!(first.is_some_and(|e| e.paused), "新增原因 → 有事件");
        assert!(
            apply_pause_reason(&mut reasons, PauseReason::Battery, true).is_none(),
            "重複設定同一個原因 → 無事件"
        );
        assert!(
            apply_pause_reason(&mut reasons, PauseReason::PowerSaver, false).is_none(),
            "移除本來就不在的原因 → 無事件"
        );
        let cleared = apply_pause_reason(&mut reasons, PauseReason::Battery, false);
        assert!(
            cleared.is_some_and(|e| !e.paused),
            "移除最後一個原因 → 有事件"
        );
    }

    // ── fix F2（review 5.3 low×2）────────────────────────────────────────────────────

    #[test]
    fn drag_end_saves_only_in_unlocked_edit_mode() {
        assert!(drag_end_may_save(false, true), "編輯版面且解鎖 → 可存");
        assert!(!drag_end_may_save(true, true), "鎖定 → 不存");
        assert!(
            !drag_end_may_save(false, false),
            "未鎖定但不在編輯版面（狀態失步）→ 不存"
        );
        assert!(!drag_end_may_save(true, false));
    }

    #[test]
    fn switch_edit_mode_keeps_flag_and_lock_in_step_when_save_fails() {
        let mut edit_mode = false;
        let mut settings = Settings::default();
        assert!(settings.layout_locked);
        let result = switch_edit_mode(&mut edit_mode, &mut settings, true, |_| {
            Err("磁碟已滿".to_string())
        });
        assert!(result.is_err(), "存檔失敗要回報錯誤");
        assert!(!edit_mode, "存檔失敗：edit_mode 不切換（回滾）");
        assert!(settings.layout_locked, "存檔失敗：layout_locked 不變");
    }

    #[test]
    fn switch_edit_mode_updates_both_after_successful_save() {
        let mut edit_mode = false;
        let mut settings = Settings::default();
        let mut saved = None;
        let changed = switch_edit_mode(&mut edit_mode, &mut settings, true, |s| {
            saved = Some(s.layout_locked);
            Ok(())
        })
        .expect("存檔成功");
        assert!(edit_mode);
        assert!(!settings.layout_locked);
        assert_eq!(saved, Some(false), "存的是解鎖後的設定");
        assert!(
            changed.is_some_and(|s| !s.layout_locked),
            "回傳要廣播的新設定"
        );

        // 離開：同樣先存檔再切換。
        let changed = switch_edit_mode(&mut edit_mode, &mut settings, false, |_| Ok(())).unwrap();
        assert!(!edit_mode && settings.layout_locked && changed.is_some());
    }

    #[test]
    fn switch_edit_mode_skips_save_when_lock_already_matches() {
        let mut edit_mode = false;
        let mut settings = Settings::default(); // 已鎖定
        let changed = switch_edit_mode(&mut edit_mode, &mut settings, false, |_| {
            panic!("鎖定旗標已是目標值，不應存檔")
        })
        .unwrap();
        assert!(changed.is_none());
        assert!(!edit_mode);
    }

    /// fix F1（review 7.3 M3）：`get_edit_mode` 回報核心目前的編輯模式旗標（頁面 Reload／故障
    /// 重建時據此恢復，edit-mode 事件只在切換時廣播一次）。
    #[test]
    fn edit_mode_of_reports_current_flag() {
        let state = AppState::new(Settings::default(), PathBuf::from("unused-settings.json"));
        assert!(!edit_mode_of(&state), "預設不在編輯版面");
        *state.edit_mode.lock().expect("edit_mode mutex poisoned") = true;
        assert!(edit_mode_of(&state));
    }

    #[test]
    fn pause_event_for_empty_set_is_not_paused() {
        let event = pause_event_for(&HashSet::new());
        assert!(!event.paused);
        assert_eq!(event.reason, None);
    }

    #[test]
    fn pause_event_for_reports_highest_priority_reason() {
        let reasons: HashSet<PauseReason> = [PauseReason::Manual, PauseReason::Locked]
            .into_iter()
            .collect();
        let event = pause_event_for(&reasons);
        assert!(event.paused);
        assert_eq!(event.reason, Some(PauseReason::Locked));
        let json = serde_json::to_value(&event).expect("序列化失敗");
        assert_eq!(
            json,
            serde_json::json!({ "paused": true, "reason": "locked" })
        );
    }

    #[test]
    fn pause_event_serializes_reason_as_null_when_not_paused() {
        let json = serde_json::to_value(PauseEvent {
            paused: false,
            reason: None,
        })
        .expect("序列化失敗");
        assert_eq!(json["paused"], false);
        assert!(json["reason"].is_null());
    }

    #[test]
    fn pause_event_serializes_reason_as_snake_case_string_when_paused() {
        let json = serde_json::to_value(PauseEvent {
            paused: true,
            reason: Some(PauseReason::Manual),
        })
        .expect("序列化失敗");
        assert_eq!(json["paused"], true);
        assert_eq!(json["reason"], "manual");
    }
}
