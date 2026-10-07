//! 設定視窗的動態桌布部分與系統匣通知文字（dynamic-wallpaper task 4.8；specs/desktop-wallpaper
//! 「桌布主題選擇」「原桌布為 Windows 焦點時先提醒」「提示 Windows 備份會帶走桌布」「使用者自行
//! 更換桌布時讓位」「首次安裝無資料時不接管」「explorer 資源安全閥」「不因 explorer 卡住而拖累
//! 宿主」；specs/wallpaper-themes「休市表快過期」）。
//!
//! ## 指令（設定視窗 `host/ui/settings.html` 經 `host/ui/bridge.js` 呼叫；只接受 label 為
//! `settings` 的 webview）
//!
//! - [`get_wallpaper_status`]：協調迴圈發佈的狀態快照（[`WallpaperStatusView`]）。只讀
//!   `CoordinatorHandle::status`（只在發佈時短暫持有的鎖）、HKCU 的焦點值、通道註冊表的設定版本，
//!   **不等協調執行緒、不碰桌布 COM 執行緒**——explorer 卡住時設定視窗照常回應。設定視窗開著時每
//!   3 秒查一次（頁面端輪詢；狀態變化不頻繁，不另設推送事件）。
//! - [`get_wallpaper_config`]：主題設定檔（Rust 頂層合併後的內容）與路徑。休市表快過期由**頁面**
//!   判定：先以頁面的內建預設 `mergeThemeConfig` 逐層合併、再 `holidayExpiryReminder`
//!   （`host/ui/wallpapers/lib/config-holidays.mjs`），與繪圖頁同一套規則，Rust 不另寫一份
//!   （`wallpaper_config` 模組文件「合併分工」：巢狀內容以頁面為單一事實來源）。
//! - [`select_wallpaper_theme`]：選主題。要先問 Windows 焦點時（[`needs_spotlight_confirmation`]）
//!   不存檔、回 `needs_spotlight_confirmation`，頁面開確認對話後帶答案再呼叫一次：確認＝先登記「這個
//!   主題已事先確認」（`wallpaper_coordinator::SpotlightPreconfirm`，協調迴圈看到設定的主題正是它時才
//!   套用）再存檔；取消＝撤回事先確認，仍未接管且主題不是「不接管」時存成「不接管」（修正輪 1；見
//!   [`select_theme_core`]）。存檔走 [`crate::widgets::update_settings`]（與其他設定欄位同一條提交、
//!   廣播、通知協調迴圈的路徑）。
//! - [`respond_spotlight_confirmation`]：協調迴圈自己發現要等焦點確認（例如事先確認之後宿主重新
//!   啟動、資料出現時才第一次設定）時，設定視窗依 `awaiting_spotlight_confirmation` 開同一個對話；
//!   答案投遞 `Wake::SpotlightConfirmed`／`Wake::SpotlightCancelled`（取消時狀態機把主題改回
//!   「不接管」）。
//!
//! ## 系統匣通知文字
//!
//! [`notice_text`]：讓位與安全閥（含 4.9 的 `TakeoverNotice::SafetyValve`）；安全閥依那次還原的
//! 結果（`ValveRestore`）選字，未接管時（`NothingToRestore`）不寫「已還原」。Windows 通知橫幅只顯示標題下
//! 約 4 行內文（task 6.1 A9 實機），所以結果句放第一句、整則控制在 4 行內，數值與路徑只寫記錄檔。[`spotlight_pending_text`]：
//! 等焦點確認。顯示由 `crate::tray::show_notification`（工作執行緒＋`desktop::tray_balloon`）負責；
//! 每個事件只在協調迴圈套用事件時呼叫一次（狀態機保證同一事件不重複產生），顯示層不另外去重。

use std::path::Path;

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Manager, State, Webview};

use crate::desktop::tray_balloon::BalloonKind;
use crate::desktop::wallpaper::registry::{self, SpotlightState};
use crate::settings::{Settings, WallpaperTheme};
use crate::wallpaper::DataKey;
use crate::wallpaper_coordinator::{CoordinatorHandle, CoordinatorState, CoordinatorStatus, Wake};
use crate::wallpaper_state::{needs_spotlight_confirmation, Phase, TakeoverNotice, ValveRestore};
use crate::widgets::AppState;

/// 設定視窗的 webview label（`tray::SETTINGS_WINDOW_LABEL`）。
const SETTINGS_LABEL: &str = crate::tray::SETTINGS_WINDOW_LABEL;

/// 讓位／安全閥通知的結尾（spec：「可在設定中重新開啟」）。
pub const REOPEN_HINT: &str = "可在設定中重新開啟。";

/// 一則系統匣通知的文字。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationText {
    pub title: String,
    pub body: String,
    pub kind: BalloonKind,
}

/// 讓位、安全閥的系統匣通知文字。
pub fn notice_text(notice: &TakeoverNotice) -> NotificationText {
    match notice {
        TakeoverNotice::Yielded { .. } => NotificationText {
            title: "財經桌布：已停止接管桌布".to_owned(),
            body: format!("偵測到你換了桌布，已保留你選的桌布、不再覆蓋。{REOPEN_HINT}"),
            kind: BalloonKind::Info,
        },
        // task 6.1 修正（low：橫幅只顯示前約 4 行內文，6.1 A9 的結果句被截掉）：結果（已停止接管＋是否已還原）
        // 放第一句；讀值、基準、門檻只寫記錄檔（協調迴圈觸發時記一行，`detail` 也在還原原因裡）。
        TakeoverNotice::SafetyValve { restore, .. } => {
            let outcome = match restore {
                ValveRestore::Restored => "已停止接管，並已還原原本的桌布。",
                ValveRestore::RestoredKeepingUserChoice => {
                    "已停止接管；你自行換過的桌布維持不動，其餘已還原。"
                }
                ValveRestore::NothingToRestore => "已停止接管（桌布尚未被更換，不需要還原）。",
                ValveRestore::RetryLater => "已停止接管；原本的桌布尚未還原，稍後會自動再試。",
                ValveRestore::Unable => "已停止接管；原本的桌布無法自動還原。",
            };
            NotificationText {
                title: "財經桌布：已停止接管桌布".to_owned(),
                body: format!(
                    "{outcome}原因：explorer 資源接近上限（數值見記錄檔）。{REOPEN_HINT}"
                ),
                kind: BalloonKind::Warning,
            }
        }
        // task 6.4（審查 R1-M3a）：路徑等細節只寫進記錄（通知有長度上限）。
        TakeoverNotice::BackupFailed { .. } => NotificationText {
            title: "財經桌布：沒有接管桌布".to_owned(),
            body: format!(
                "無法備份目前的桌布，已停止接管（主題改為「不接管」）。原因見記錄檔。{REOPEN_HINT}"
            ),
            kind: BalloonKind::Warning,
        },
    }
}

/// 等焦點確認的系統匣通知文字（設定視窗可能沒開）。
pub fn spotlight_pending_text() -> NotificationText {
    NotificationText {
        title: "財經桌布：接管桌布前需要確認".to_owned(),
        body: "目前的桌布是 Windows 焦點。停止接管時無法自動切回 Windows 焦點，需要到 Windows 設定手動切回。請從系統匣開啟「設定」確認後，才會開始接管桌布。".to_owned(),
        kind: BalloonKind::Info,
    }
}

// ---------------------------------------------------------------------------------------------
// 選主題
// ---------------------------------------------------------------------------------------------

/// 選主題這一步要做什麼。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeSelectionPlan {
    /// 存檔。`preconfirm`＝使用者已確認焦點提示：存檔前登記「這個主題已事先確認」
    /// （`wallpaper_coordinator::SpotlightPreconfirm`）；為 `false` 時把登記清成空的。
    Commit { preconfirm: bool },
    /// 要先問 Windows 焦點：不存檔，請頁面開確認對話。
    AskSpotlight,
    /// 使用者在焦點提示中取消：撤回登記；尚未接管且主題不是「不接管」時存成「不接管」
    /// （見 [`select_theme_core`]）。
    Cancelled,
}

/// 依「要不要問」與使用者的答案（`None`＝還沒問、`Some(true)`＝確認、`Some(false)`＝取消）決定。
/// 選「不接管」永遠直接存檔。
pub fn plan_theme_selection(
    theme: WallpaperTheme,
    needs_confirmation: bool,
    answer: Option<bool>,
) -> ThemeSelectionPlan {
    if theme == WallpaperTheme::None {
        return ThemeSelectionPlan::Commit { preconfirm: false };
    }
    match answer {
        Some(true) => ThemeSelectionPlan::Commit { preconfirm: true },
        Some(false) => ThemeSelectionPlan::Cancelled,
        None if needs_confirmation => ThemeSelectionPlan::AskSpotlight,
        None => ThemeSelectionPlan::Commit { preconfirm: false },
    }
}

/// [`select_wallpaper_theme`] 的回應。
#[derive(Debug, Clone, Serialize)]
pub struct ThemeSelectResponse {
    /// `applied`／`needs_spotlight_confirmation`／`cancelled`。
    pub outcome: &'static str,
    /// 目前（已生效）的設定：`applied` 是存檔後的設定，其餘是沒變的設定。
    pub settings: Settings,
}

/// 由協調迴圈狀態快照與焦點登錄值判定「選主題前要不要先問」。
pub fn spotlight_check_needed(status: &CoordinatorStatus, spotlight: &SpotlightState) -> bool {
    let phase = if status.spotlight_confirmed {
        Phase::SpotlightConfirmed
    } else {
        Phase::Idle
    };
    needs_spotlight_confirmation(status.taken_over, phase, spotlight)
}

/// [`select_wallpaper_theme`] 的可測本體。`taken_over`＝協調迴圈快照的「接管中」；`preconfirm` 登記
/// （`Some`）或撤回（`None`）設定視窗的事先確認（`wallpaper_coordinator::SpotlightPreconfirm`）；`commit`
/// 存檔（回傳存檔後的設定）；`current` 讀目前設定。
///
/// - 確認：先登記「這個主題已事先確認」再存檔；存檔失敗時撤回（修正輪 1，審查 L5(b)）。協調迴圈看到設定
///   的主題正是登記的主題時才套用，不會先用舊主題畫一次。不需要確認的選擇把登記清成空的（修正輪 2）。
/// - 取消（修正輪 1，審查 medium）：撤回任何事先確認；仍未接管且目前主題不是「不接管」時存成「不接管」
///   （spec「使用者取消時主題設定 SHALL 維持不接管」；與後備路徑的取消一致，也不會在第一次設定前被系統匣
///   與對話再問一次）。已接管時不動（這時本來不會問，只可能是過期的答案）。
pub fn select_theme_core(
    theme: WallpaperTheme,
    answer: Option<bool>,
    needs_confirmation: bool,
    taken_over: bool,
    mut preconfirm: impl FnMut(Option<WallpaperTheme>),
    mut commit: impl FnMut(Value) -> Result<Settings, String>,
    current: impl FnOnce() -> Settings,
) -> Result<ThemeSelectResponse, String> {
    let patch_for = |t: WallpaperTheme| serde_json::json!({ "wallpaper_theme": t.as_str() });
    match plan_theme_selection(theme, needs_confirmation, answer) {
        ThemeSelectionPlan::AskSpotlight => Ok(ThemeSelectResponse {
            outcome: "needs_spotlight_confirmation",
            settings: current(),
        }),
        ThemeSelectionPlan::Cancelled => {
            preconfirm(None);
            let now = current();
            if taken_over || now.wallpaper_theme == WallpaperTheme::None {
                log::info!(
                    "使用者在 Windows 焦點提示中取消（選 {}）：主題維持 {}",
                    theme.as_str(),
                    now.wallpaper_theme.as_str()
                );
                return Ok(ThemeSelectResponse {
                    outcome: "cancelled",
                    settings: now,
                });
            }
            log::info!(
                "使用者在 Windows 焦點提示中取消（選 {}）：尚未接管，主題由 {} 改為「不接管」",
                theme.as_str(),
                now.wallpaper_theme.as_str()
            );
            let settings = commit(patch_for(WallpaperTheme::None))?;
            Ok(ThemeSelectResponse {
                outcome: "cancelled",
                settings,
            })
        }
        ThemeSelectionPlan::Commit { preconfirm: pre } => {
            if pre {
                log::info!(
                    "使用者確認 Windows 焦點提示（主題 {}）：登記事先確認後存檔",
                    theme.as_str()
                );
            }
            // 每一次選擇都蓋掉之前的登記（修正輪 2，複審 low）：不需要確認的選擇寫入 `None`，舊主題的
            // 登記不會殘留到之後，讓後備路徑的確認一直被當成「另有其他主題的事先確認」而忽略。
            preconfirm(pre.then_some(theme));
            match commit(patch_for(theme)) {
                Ok(settings) => Ok(ThemeSelectResponse {
                    outcome: "applied",
                    settings,
                }),
                Err(e) => {
                    if pre {
                        preconfirm(None);
                    }
                    Err(e)
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 狀態快照
// ---------------------------------------------------------------------------------------------

/// 設定視窗的狀態快照（[`get_wallpaper_status`] 的回應）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WallpaperStatusView {
    /// 協調迴圈有沒有在跑（沒有時其餘欄位是預設值）。
    pub coordinator_running: bool,
    /// 協調迴圈狀態（穩定字串，見 [`state_name`]）。
    pub state: &'static str,
    /// 等待資料中：缺的資料（中文名稱）。
    pub waiting_for: Option<&'static str>,
    /// 已接管但資料不見了：缺的資料（中文名稱）。
    pub holding_without: Option<&'static str>,
    /// 狀態檔讀不懂的原因。
    pub blocked: Option<String>,
    /// 協調迴圈在等焦點確認（設定視窗開確認對話）。
    pub awaiting_spotlight_confirmation: bool,
    /// 現在選某個主題要先問 Windows 焦點。
    pub spotlight_check_needed: bool,
    /// 狀態檔為接管中。
    pub taken_over: bool,
    /// 主題設定檔改過幾次（頁面據此決定要不要重新取設定檔內容）。
    pub config_version: Option<u64>,
}

/// [`CoordinatorState`] 的穩定字串（頁面依它顯示）。
pub fn state_name(state: &CoordinatorState) -> &'static str {
    match state {
        CoordinatorState::Starting => "starting",
        CoordinatorState::Idle => "idle",
        CoordinatorState::WaitingForData(_) => "waiting_for_data",
        CoordinatorState::HoldWithoutData(_) => "hold_without_data",
        CoordinatorState::Paused => "paused",
        CoordinatorState::DoNotDisturb => "do_not_disturb",
        CoordinatorState::AwaitingSpotlightConfirmation => "awaiting_spotlight_confirmation",
        CoordinatorState::UpToDate => "up_to_date",
        CoordinatorState::RetryPending => "retry_pending",
        CoordinatorState::Redrawing => "redrawing",
        CoordinatorState::Blocked(_) => "blocked",
        CoordinatorState::ResumingRestore => "resuming_restore",
        CoordinatorState::WaitingForStateFile => "waiting_for_state_file",
        CoordinatorState::SessionEnding => "session_ending",
    }
}

/// 資料鍵的中文名稱。
pub fn data_key_label(key: DataKey) -> &'static str {
    match key {
        DataKey::TwiiIntraday => "加權指數盤中走勢",
        DataKey::TwiiDaily => "加權指數日 K",
    }
}

/// 組出狀態快照（`status`＝`None`：協調迴圈沒有在跑）。
pub fn status_view(
    status: Option<&CoordinatorStatus>,
    spotlight: &SpotlightState,
    config_version: Option<u64>,
) -> WallpaperStatusView {
    let fallback = CoordinatorStatus::default();
    let s = status.unwrap_or(&fallback);
    WallpaperStatusView {
        coordinator_running: status.is_some(),
        state: state_name(&s.state),
        waiting_for: match s.state {
            CoordinatorState::WaitingForData(k) => Some(data_key_label(k)),
            _ => None,
        },
        holding_without: match s.state {
            CoordinatorState::HoldWithoutData(k) => Some(data_key_label(k)),
            _ => None,
        },
        blocked: match &s.state {
            CoordinatorState::Blocked(r) => Some(r.clone()),
            _ => None,
        },
        awaiting_spotlight_confirmation: s.awaiting_spotlight_confirmation,
        spotlight_check_needed: spotlight_check_needed(s, spotlight),
        taken_over: s.taken_over,
        config_version,
    }
}

/// [`get_wallpaper_config`] 的回應。
#[derive(Debug, Clone, Serialize)]
pub struct WallpaperConfigView {
    /// 主題設定檔路徑（提示「請更新明年的休市表」時附上）。
    pub path: String,
    /// Rust 頂層合併後的設定（沒有 `wallpaper` 通道時 `None`；頁面改用內建預設）。
    pub config: Option<Value>,
}

// ---------------------------------------------------------------------------------------------
// Tauri 指令
// ---------------------------------------------------------------------------------------------

fn require_settings_window(webview: &Webview) -> Result<(), String> {
    if webview.label() == SETTINGS_LABEL {
        Ok(())
    } else {
        log::warn!(
            "拒絕視窗 {} 呼叫桌布設定指令（只給設定視窗）",
            webview.label()
        );
        Err(format!("視窗 {} 不能呼叫桌布設定指令", webview.label()))
    }
}

fn coordinator_status(app: &AppHandle) -> Option<CoordinatorStatus> {
    app.try_state::<CoordinatorHandle>().map(|h| h.status())
}

/// 設定視窗的桌布狀態快照（見模組文件）。非同步指令：不佔用主執行緒。
#[tauri::command]
pub async fn get_wallpaper_status(
    webview: Webview,
    app: AppHandle,
) -> Result<WallpaperStatusView, String> {
    require_settings_window(&webview)?;
    let status = coordinator_status(&app);
    let spotlight = registry::read_spotlight();
    let config_version = app
        .try_state::<AppState>()
        .and_then(|s| lock_registry_version(&s));
    Ok(status_view(status.as_ref(), &spotlight, config_version))
}

fn lock_registry_version(state: &AppState) -> Option<u64> {
    let registry = state.registry.lock().unwrap_or_else(|e| e.into_inner());
    registry.wallpaper_config_version()
}

/// 主題設定檔內容與路徑（見模組文件）。非同步指令：不佔用主執行緒。
#[tauri::command]
pub async fn get_wallpaper_config(
    webview: Webview,
    app: AppHandle,
) -> Result<WallpaperConfigView, String> {
    require_settings_window(&webview)?;
    let state = app.state::<AppState>();
    let path = state
        .settings_path
        .with_file_name(crate::wallpaper_config::CONFIG_FILE_NAME);
    let config = {
        let registry = state.registry.lock().unwrap_or_else(|e| e.into_inner());
        registry.wallpaper_config_value().cloned()
    };
    Ok(WallpaperConfigView {
        path: display_path(&path),
        config,
    })
}

fn display_path(path: &Path) -> String {
    path.display().to_string()
}

/// 選主題（見模組文件）。同步指令（主執行緒）：存檔走 [`crate::widgets::update_settings`]，它要在
/// 主執行緒重新排版小工具；這裡只多讀一次 HKCU 的焦點值與協調迴圈的狀態快照，都不等待。
#[tauri::command]
pub fn select_wallpaper_theme(
    webview: Webview,
    theme: WallpaperTheme,
    spotlight_answer: Option<bool>,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<ThemeSelectResponse, String> {
    require_settings_window(&webview)?;
    let status = coordinator_status(&app).unwrap_or_default();
    let needs = spotlight_check_needed(&status, &registry::read_spotlight());
    let preconfirm_cell = app
        .try_state::<CoordinatorHandle>()
        .map(|h| h.spotlight_preconfirm());
    select_theme_core(
        theme,
        spotlight_answer,
        needs,
        status.taken_over,
        |t| {
            if let Some(cell) = &preconfirm_cell {
                cell.set(t);
            }
        },
        |patch| crate::widgets::update_settings(patch, state.clone(), app.clone()),
        || {
            let settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
            crate::widgets::with_registered_autostart(&settings, state.autostart_registry.as_ref())
        },
    )
}

/// 協調迴圈在等焦點確認時，設定視窗對話的答案（見模組文件）。
#[tauri::command]
pub fn respond_spotlight_confirmation(
    webview: Webview,
    confirmed: bool,
    app: AppHandle,
) -> Result<(), String> {
    require_settings_window(&webview)?;
    let wake = if confirmed {
        Wake::SpotlightConfirmed
    } else {
        Wake::SpotlightCancelled
    };
    log::info!("設定視窗回覆 Windows 焦點提示：{wake:?}");
    crate::wallpaper_coordinator::notify_app(&app, wake);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const SPOTLIGHT_ON: SpotlightState = SpotlightState {
        enabled_state: Some(1),
        background_type: None,
    };
    const SPOTLIGHT_OFF: SpotlightState = SpotlightState {
        enabled_state: Some(0),
        background_type: None,
    };

    /// 實際可能出現的最長安全閥說明（PID、讀值、基準都是大數，兩個門檻都超過）。
    fn longest_detail() -> String {
        use crate::wallpaper::{ExplorerSample, SafetyValveTrip};
        SafetyValveTrip {
            reading: ExplorerSample {
                pid: u32::MAX,
                gdi: u32::MAX,
            },
            baseline: Some(ExplorerSample {
                pid: u32::MAX,
                gdi: 1_000_000,
            }),
            delta_exceeded: true,
            absolute_exceeded: true,
        }
        .detail()
    }

    fn utf16_len(s: &str) -> usize {
        s.encode_utf16().count()
    }

    // ── 通知文字 ───────────────────────────────────────────────────────────────────────

    #[test]
    fn yield_notice_explains_and_offers_reopen() {
        let t = notice_text(&TakeoverNotice::Yielded {
            device_paths: vec!["DEV".to_owned()],
        });
        assert!(t.body.contains("偵測到你換了桌布"), "{}", t.body);
        assert!(t.body.ends_with(REOPEN_HINT), "{}", t.body);
        assert!(
            !t.body.contains("還原"),
            "讓位保留使用者的桌布，不是還原：{}",
            t.body
        );
        assert!(!t.title.is_empty());
    }

    /// task 6.1 修正：讀值、基準等數值只寫記錄檔（協調迴圈觸發時記一行），通知只說原因與結果。
    #[test]
    fn safety_valve_notice_includes_reason_and_reopen() {
        let detail = longest_detail();
        let t = notice_text(&TakeoverNotice::SafetyValve {
            detail: detail.clone(),
            restore: ValveRestore::Restored,
        });
        assert!(t.body.contains("explorer 資源接近上限"), "{}", t.body);
        assert!(t.body.contains("記錄檔"), "數值在記錄檔：{}", t.body);
        assert!(t.body.contains("已還原"), "{}", t.body);
        assert!(t.body.ends_with(REOPEN_HINT), "{}", t.body);
        assert_eq!(t.kind, BalloonKind::Warning);
    }

    #[test]
    fn safety_valve_without_takeover_never_claims_restored() {
        for restore in [
            ValveRestore::NothingToRestore,
            ValveRestore::RetryLater,
            ValveRestore::Unable,
        ] {
            let t = notice_text(&TakeoverNotice::SafetyValve {
                detail: "GDI 9000".to_owned(),
                restore,
            });
            assert!(!t.body.contains("已還原"), "{restore:?}：{}", t.body);
            assert!(
                t.body.contains("explorer 資源接近上限"),
                "{restore:?}：{}",
                t.body
            );
            assert!(t.body.ends_with(REOPEN_HINT), "{restore:?}：{}", t.body);
        }
        let retry = notice_text(&TakeoverNotice::SafetyValve {
            detail: "d".to_owned(),
            restore: ValveRestore::RetryLater,
        });
        assert!(retry.body.contains("稍後"), "{}", retry.body);
    }

    /// task 6.4（審查 R2b nit）：還原時有螢幕保留了使用者自己換的桌布——不寫「已還原原本的桌布」，如實
    /// 說明自換的維持不動、其餘已還原。
    #[test]
    fn safety_valve_keeping_user_choice_does_not_claim_full_restore() {
        let t = notice_text(&TakeoverNotice::SafetyValve {
            detail: "GDI 9000".to_owned(),
            restore: ValveRestore::RestoredKeepingUserChoice,
        });
        assert!(!t.body.contains("已還原原本的桌布"), "{}", t.body);
        assert!(t.body.contains("你自行換過的桌布維持不動"), "{}", t.body);
        assert!(t.body.ends_with(REOPEN_HINT), "{}", t.body);
    }

    /// task 6.4（審查 R1-M3a）：原桌布無法備份的通知——說明沒有接管、主題改「不接管」，不帶路徑。
    #[test]
    fn backup_failed_notice_explains_and_omits_the_path() {
        let t = notice_text(&TakeoverNotice::BackupFailed {
            detail: "螢幕 X 的原桌布 C:\\很長的路徑\\a.jpg 無法備份".to_owned(),
        });
        assert!(t.body.contains("無法備份目前的桌布"), "{}", t.body);
        assert!(t.body.contains("不接管"), "{}", t.body);
        assert!(!t.body.contains("a.jpg"), "路徑只寫進記錄：{}", t.body);
        assert!(t.body.ends_with(REOPEN_HINT), "{}", t.body);
        assert_eq!(t.kind, BalloonKind::Warning);
    }

    #[test]
    fn every_notification_fits_the_shell_limits() {
        // Microsoft Learn〈NOTIFYICONDATAW〉：標題 64、內文 256 個 UTF-16 單位（皆含結尾 null）。
        let mut all = vec![
            notice_text(&TakeoverNotice::Yielded {
                device_paths: Vec::new(),
            }),
            spotlight_pending_text(),
            notice_text(&TakeoverNotice::BackupFailed {
                detail: longest_detail(),
            }),
        ];
        for restore in [
            ValveRestore::Restored,
            ValveRestore::RestoredKeepingUserChoice,
            ValveRestore::NothingToRestore,
            ValveRestore::RetryLater,
            ValveRestore::Unable,
        ] {
            all.push(notice_text(&TakeoverNotice::SafetyValve {
                detail: longest_detail(),
                restore,
            }));
        }
        for t in all {
            assert!(!t.title.is_empty() && !t.body.is_empty(), "{t:?}");
            assert!(utf16_len(&t.title) <= 63, "{t:?}");
            assert!(
                utf16_len(&t.body) <= 255,
                "{} 個單位：{t:?}",
                utf16_len(&t.body)
            );
        }
    }

    /// 顯示寬度（全形字 2 欄、ASCII 1 欄）。
    fn display_cols(s: &str) -> usize {
        s.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum()
    }

    /// 6.1 A9 截圖：通知橫幅一行約 36 欄，標題之下只顯示約 4 行內文，其餘被截掉。留一點換行損耗：
    /// 一行算 34 欄。
    const BANNER_LINE_COLS: usize = 34;
    /// 內文在橫幅上看得到的行數。
    const BANNER_BODY_LINES: usize = 4;

    /// task 6.1 修正（low：安全閥通知被截斷）：最重要的資訊（已停止接管＋是否已還原／為何沒接管）放在第一句，
    /// 而且第一句在前兩行之內；整則內文在橫幅可見的 4 行之內（細節只寫記錄檔）。
    #[test]
    fn notices_lead_with_the_outcome_and_fit_the_banner() {
        let mut all = vec![
            (
                "yield",
                notice_text(&TakeoverNotice::Yielded {
                    device_paths: vec!["DEV".to_owned()],
                }),
                "已保留你選的桌布",
            ),
            (
                "backup",
                notice_text(&TakeoverNotice::BackupFailed {
                    detail: longest_detail(),
                }),
                "已停止接管",
            ),
        ];
        for (restore, key) in [
            (ValveRestore::Restored, "已還原原本的桌布"),
            (ValveRestore::RestoredKeepingUserChoice, "其餘已還原"),
            (ValveRestore::NothingToRestore, "不需要還原"),
            (ValveRestore::RetryLater, "尚未還原"),
            (ValveRestore::Unable, "無法自動還原"),
        ] {
            all.push((
                "valve",
                notice_text(&TakeoverNotice::SafetyValve {
                    detail: longest_detail(),
                    restore,
                }),
                key,
            ));
        }
        for (what, t, key) in all {
            let first = t.body.split_inclusive('。').next().unwrap_or_default();
            assert!(
                first.contains(key),
                "{what}：第一句要有「{key}」：{}",
                t.body
            );
            assert!(
                display_cols(first) <= 2 * BANNER_LINE_COLS,
                "{what}：第一句 {} 欄超過兩行：{first}",
                display_cols(first)
            );
            assert!(
                display_cols(&t.body) <= BANNER_BODY_LINES * BANNER_LINE_COLS,
                "{what}：內文 {} 欄超過橫幅可見的 {BANNER_BODY_LINES} 行：{}",
                display_cols(&t.body),
                t.body
            );
            if what == "valve" {
                assert!(first.starts_with("已停止接管"), "{}", t.body);
                assert!(
                    !t.body.contains(&longest_detail()),
                    "數值只寫記錄檔：{}",
                    t.body
                );
            }
        }
    }

    #[test]
    fn spotlight_pending_text_quotes_the_spec_warning() {
        let t = spotlight_pending_text();
        assert!(
            t.body
                .contains("停止接管時無法自動切回 Windows 焦點，需要到 Windows 設定手動切回"),
            "{}",
            t.body
        );
        assert!(t.body.contains("設定"), "{}", t.body);
    }

    // ── 選主題 ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn plan_asks_only_when_needed_and_unanswered() {
        use ThemeSelectionPlan::*;
        let t = WallpaperTheme::Astrolabe;
        assert_eq!(plan_theme_selection(t, true, None), AskSpotlight);
        assert_eq!(
            plan_theme_selection(t, true, Some(true)),
            Commit { preconfirm: true }
        );
        assert_eq!(plan_theme_selection(t, true, Some(false)), Cancelled);
        assert_eq!(
            plan_theme_selection(t, false, None),
            Commit { preconfirm: false }
        );
        // 「不接管」永遠直接存檔，不問、不事先確認。
        for answer in [None, Some(true), Some(false)] {
            assert_eq!(
                plan_theme_selection(WallpaperTheme::None, true, answer),
                Commit { preconfirm: false },
                "{answer:?}"
            );
        }
    }

    fn settings_with(theme: WallpaperTheme) -> Settings {
        Settings {
            wallpaper_theme: theme,
            ..Settings::default()
        }
    }

    /// 呼叫紀錄：事先確認的登記／撤回（`pre Some(..)`／`pre None`）與存檔（`commit <patch>`）。
    #[derive(Default)]
    struct Calls(RefCell<Vec<String>>);

    impl Calls {
        fn run(
            &self,
            theme: WallpaperTheme,
            answer: Option<bool>,
            needs: bool,
            taken_over: bool,
            saved: WallpaperTheme,
            commit_result: Result<(), &str>,
        ) -> Result<ThemeSelectResponse, String> {
            select_theme_core(
                theme,
                answer,
                needs,
                taken_over,
                |t| self.0.borrow_mut().push(format!("pre {t:?}")),
                |patch| {
                    self.0.borrow_mut().push(format!("commit {patch}"));
                    commit_result.map_err(str::to_owned)?;
                    let theme: WallpaperTheme =
                        serde_json::from_value(patch["wallpaper_theme"].clone()).unwrap();
                    Ok(settings_with(theme))
                },
                || settings_with(saved),
            )
        }
        fn list(&self) -> Vec<String> {
            self.0.borrow().clone()
        }
    }

    /// spec「原桌布為 Windows 焦點時先提醒」Scenario「使用者取消」：主題本來就是「不接管」時取消＝什麼都不存，
    /// 只撤回任何事先確認。
    #[test]
    fn cancel_from_none_keeps_none_without_saving() {
        let calls = Calls::default();
        let r = calls
            .run(
                WallpaperTheme::Skyline,
                Some(false),
                true,
                false,
                WallpaperTheme::None,
                Ok(()),
            )
            .unwrap();
        assert_eq!(r.outcome, "cancelled");
        assert_eq!(r.settings.wallpaper_theme, WallpaperTheme::None);
        assert_eq!(
            calls.list(),
            vec!["pre None".to_owned()],
            "不存檔、不登記確認"
        );
    }

    /// 4.8 修正輪 1（審查 medium）：主題已是 A（尚未接管）時改選 B、在提示中取消 → 存成「不接管」（spec：取消時
    /// 主題設定維持「不接管」；與後備路徑的取消一致）。留著 A 會在第一次設定前被系統匣與對話再問一次。
    #[test]
    fn cancel_from_another_theme_saves_none() {
        let calls = Calls::default();
        let r = calls
            .run(
                WallpaperTheme::Contour,
                Some(false),
                true,
                false,
                WallpaperTheme::Skyline,
                Ok(()),
            )
            .unwrap();
        assert_eq!(r.outcome, "cancelled");
        assert_eq!(r.settings.wallpaper_theme, WallpaperTheme::None);
        assert_eq!(
            calls.list(),
            vec![
                "pre None".to_owned(),
                r#"commit {"wallpaper_theme":"none"}"#.to_owned()
            ]
        );
    }

    /// 已接管時不會問；過期的「取消」答案不得動已接管的主題。
    #[test]
    fn stale_cancel_while_taken_over_changes_nothing() {
        let calls = Calls::default();
        let r = calls
            .run(
                WallpaperTheme::Contour,
                Some(false),
                false,
                true,
                WallpaperTheme::Skyline,
                Ok(()),
            )
            .unwrap();
        assert_eq!(r.outcome, "cancelled");
        assert_eq!(r.settings.wallpaper_theme, WallpaperTheme::Skyline);
        assert!(!calls.list().iter().any(|c| c.starts_with("commit")));
    }

    #[test]
    fn cancel_save_error_is_returned() {
        let calls = Calls::default();
        let r = calls.run(
            WallpaperTheme::Contour,
            Some(false),
            true,
            false,
            WallpaperTheme::Skyline,
            Err("存檔失敗"),
        );
        assert_eq!(r.unwrap_err(), "存檔失敗");
    }

    #[test]
    fn needs_confirmation_returns_without_saving() {
        let calls = Calls::default();
        let r = calls
            .run(
                WallpaperTheme::Ridgeline,
                None,
                true,
                false,
                WallpaperTheme::None,
                Ok(()),
            )
            .unwrap();
        assert_eq!(r.outcome, "needs_spotlight_confirmation");
        assert_eq!(r.settings.wallpaper_theme, WallpaperTheme::None);
        assert!(calls.list().is_empty(), "還沒確認：不登記、不存檔");
    }

    /// 確認：先登記「這個主題已事先確認」再存檔（協調迴圈看到這個主題時才套用，不會先用舊主題畫）。
    #[test]
    fn confirm_registers_preconfirm_for_the_theme_before_saving() {
        let calls = Calls::default();
        let r = calls
            .run(
                WallpaperTheme::Contour,
                Some(true),
                true,
                false,
                WallpaperTheme::None,
                Ok(()),
            )
            .unwrap();
        assert_eq!(r.outcome, "applied");
        assert_eq!(r.settings.wallpaper_theme, WallpaperTheme::Contour);
        assert_eq!(
            calls.list(),
            vec![
                "pre Some(Contour)".to_owned(),
                r#"commit {"wallpaper_theme":"contour"}"#.to_owned()
            ]
        );
    }

    /// 4.8 修正輪 1（審查 L5(b)）：存檔失敗時撤回事先確認，不留在記憶體。
    #[test]
    fn commit_error_withdraws_the_preconfirm() {
        let calls = Calls::default();
        let r = calls.run(
            WallpaperTheme::Tearoff,
            Some(true),
            true,
            false,
            WallpaperTheme::None,
            Err("存檔失敗"),
        );
        assert_eq!(r.unwrap_err(), "存檔失敗");
        assert_eq!(
            calls.list(),
            vec![
                "pre Some(Tearoff)".to_owned(),
                r#"commit {"wallpaper_theme":"tearoff"}"#.to_owned(),
                "pre None".to_owned()
            ]
        );
    }

    #[test]
    fn plain_selection_saves_without_preconfirm() {
        let calls = Calls::default();
        let r = calls
            .run(
                WallpaperTheme::Tearoff,
                None,
                false,
                false,
                WallpaperTheme::None,
                Ok(()),
            )
            .unwrap();
        assert_eq!(r.outcome, "applied");
        assert_eq!(
            calls.list(),
            vec![
                "pre None".to_owned(),
                r#"commit {"wallpaper_theme":"tearoff"}"#.to_owned()
            ],
            "不需要確認的選擇也要蓋掉舊的事先確認登記（修正輪 2）"
        );
    }

    // ── 狀態快照 ───────────────────────────────────────────────────────────────────────

    #[test]
    fn spotlight_check_follows_takeover_and_confirmation() {
        let mut s = CoordinatorStatus::default();
        assert!(spotlight_check_needed(&s, &SPOTLIGHT_ON));
        assert!(!spotlight_check_needed(&s, &SPOTLIGHT_OFF));
        assert!(!spotlight_check_needed(&s, &SpotlightState::default()));
        s.spotlight_confirmed = true;
        assert!(!spotlight_check_needed(&s, &SPOTLIGHT_ON));
        s.spotlight_confirmed = false;
        s.taken_over = true;
        assert!(!spotlight_check_needed(&s, &SPOTLIGHT_ON));
    }

    #[test]
    fn status_view_reports_waiting_for_data_with_label() {
        let s = CoordinatorStatus {
            state: CoordinatorState::WaitingForData(DataKey::TwiiDaily),
            ..CoordinatorStatus::default()
        };
        let v = status_view(Some(&s), &SPOTLIGHT_OFF, Some(3));
        assert!(v.coordinator_running);
        assert_eq!(v.state, "waiting_for_data");
        assert_eq!(v.waiting_for, Some("加權指數日 K"));
        assert_eq!(v.holding_without, None);
        assert_eq!(v.config_version, Some(3));
    }

    #[test]
    fn status_view_without_coordinator_uses_defaults() {
        let v = status_view(None, &SPOTLIGHT_ON, None);
        assert!(!v.coordinator_running);
        assert_eq!(v.state, "starting");
        assert!(v.spotlight_check_needed);
        assert!(!v.awaiting_spotlight_confirmation);
    }

    #[test]
    fn status_view_reports_blocked_reason_and_awaiting_flag() {
        let s = CoordinatorStatus {
            state: CoordinatorState::Blocked("損壞".to_owned()),
            awaiting_spotlight_confirmation: true,
            ..CoordinatorStatus::default()
        };
        let v = status_view(Some(&s), &SPOTLIGHT_OFF, None);
        assert_eq!(v.state, "blocked");
        assert_eq!(v.blocked.as_deref(), Some("損壞"));
        assert!(v.awaiting_spotlight_confirmation);
    }
}
