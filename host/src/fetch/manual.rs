//! 設定視窗的「立即更新資料」（fetch-progress-and-manual-refresh design.md D3；spec `market-data-fetch`「手動立即更新」）。
//!
//! [`request_fetch_now`] 不自己抓：判斷可以抓之後只經 [`SchedulerHandle::request_manual`] 設旗標並喚醒排程執行緒，
//! 由排程迴圈以 `Trigger::Manual` 開始一輪（同一個執行緒、同一條寫檔路徑、同一組活動狀態回報；迴圈那一半見
//! [`super::scheduler`] 模組文件「手動更新」）。另開執行緒自己跑一輪的做法被否決：會有兩條執行緒同時寫資料檔與排程紀錄，
//! 也繞過系統匣結束與因更新結束的停止流程。
//!
//! 回應 `{ "outcome": "started" | "already_running" | "disabled" }`（判斷順序見 [`decide`]）：
//!
//! - `disabled`：閘門不是 `Enabled`（`data_fetch="off"` 或隔離環境）。閘門以**目前設定**現算（與排程迴圈同一個
//!   [`decide_gate`] 與同一種隔離偵測），不讀活動狀態的 `enabled`——那是排程上次醒來時的值，設定剛改的一分鐘內會落後；
//! - `already_running`：活動狀態 `running` 為真。不設旗標；進行中的那一輪已經滿足這次要求；
//! - `started`：設旗標並喚醒。頁面靠隨後的 `fetch-activity` 事件更新畫面。
//!
//! 「檢查 `running` 之後、設旗標之前剛好開始一輪」的空隙由迴圈補上：一輪結束時清旗標，這次要求由那一輪吸收，不會緊接著
//! 再抓一輪；閘門在檢查之後才被關掉時，迴圈取走旗標後丟棄，不發出請求。
//!
//! 沒有排程把手（排程執行緒啟動失敗、或因「正在因更新結束」沒有啟動）或正在因更新結束時回錯誤 [`NO_SCHEDULER`]。
//! 只接受設定視窗（[`require_settings_window`]；本機自訂指令沒有逐指令 ACL，限制只能由指令自己擋）。

use std::sync::PoisonError;

use serde::Serialize;
use tauri::{AppHandle, Manager, Webview};

use crate::desktop;
use crate::widgets::AppState;

use super::activity::FetchActivityStore;
use super::report;
use super::scheduler::{decide_gate, Gate, SchedulerHandle};
use super::status::require_settings_window;

/// 沒有排程可以要求時的錯誤字串。
pub const NO_SCHEDULER: &str = "抓取排程沒有啟動，無法立即更新";

/// [`request_fetch_now`] 的結果（序列化為 `started`／`already_running`／`disabled`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FetchNowOutcome {
    /// 已要求排程執行緒立即抓一輪。
    Started,
    /// 已有一輪正在進行，這次要求由它滿足。
    AlreadyRunning,
    /// 抓取未啟用（`data_fetch="off"` 或隔離環境），不抓。
    Disabled,
}

impl FetchNowOutcome {
    fn as_str(self) -> &'static str {
        match self {
            FetchNowOutcome::Started => "started",
            FetchNowOutcome::AlreadyRunning => "already_running",
            FetchNowOutcome::Disabled => "disabled",
        }
    }
}

/// [`request_fetch_now`] 的回應：`{ "outcome": … }`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FetchNowReply {
    pub outcome: FetchNowOutcome,
}

/// 純判斷（指令與測試共用）。順序：呼叫端不是設定視窗 → 錯誤；沒有可用的排程 → 錯誤 [`NO_SCHEDULER`]；
/// 閘門不是啟用 → `disabled`；進行中 → `already_running`；否則 `started`。`disabled` 先於 `already_running`：
/// 輪中被關掉、還沒停下的那一輪不算「進行中可以滿足要求」。
pub fn decide(
    label: &str,
    scheduler_available: bool,
    gate: &Gate,
    running: bool,
) -> Result<FetchNowOutcome, String> {
    require_settings_window(label)?;
    if !scheduler_available {
        return Err(NO_SCHEDULER.to_string());
    }
    if *gate != Gate::Enabled {
        return Ok(FetchNowOutcome::Disabled);
    }
    if running {
        return Ok(FetchNowOutcome::AlreadyRunning);
    }
    Ok(FetchNowOutcome::Started)
}

/// 設定視窗的「立即更新資料」（見模組文件）。非同步指令：不在主執行緒上鎖設定、讀登錄（隔離偵測）。
/// 本身不等待抓取，設旗標就返回。
#[tauri::command]
pub async fn request_fetch_now(webview: Webview, app: AppHandle) -> Result<FetchNowReply, String> {
    let mode = app
        .state::<AppState>()
        .settings
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .data_fetch;
    let gate = decide_gate(mode, desktop::detect_isolated_local_app_data().as_ref());
    let handle = app.try_state::<SchedulerHandle>();
    let scheduler_available = handle.is_some() && !crate::updater::is_exiting_for_update();
    // 活動狀態不在（不會發生：`build_ui` 在建小工具前就登記）時當成沒有進行中；有排程就交給迴圈判斷。
    let running = app
        .try_state::<FetchActivityStore>()
        .is_some_and(|store| store.get().running);
    let outcome = decide(webview.label(), scheduler_available, &gate, running)?;
    if outcome == FetchNowOutcome::Started {
        if let Some(handle) = handle {
            handle.request_manual();
        }
    }
    log::info!(
        target: report::LOG_TARGET,
        "設定視窗要求立即更新資料：{}",
        outcome.as_str()
    );
    Ok(FetchNowReply { outcome })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::LocalAppDataMismatch;

    fn isolated() -> Gate {
        Gate::Isolated(LocalAppDataMismatch {
            env_value: "C:\\Temp\\iso\\local".to_string(),
            registered: "C:\\Users\\Ben\\AppData\\Local".to_string(),
        })
    }

    #[test]
    fn an_enabled_idle_scheduler_starts_a_round() {
        assert_eq!(
            decide("settings", true, &Gate::Enabled, false),
            Ok(FetchNowOutcome::Started)
        );
    }

    #[test]
    fn a_round_in_progress_satisfies_the_request() {
        assert_eq!(
            decide("settings", true, &Gate::Enabled, true),
            Ok(FetchNowOutcome::AlreadyRunning)
        );
    }

    #[test]
    fn off_or_isolated_is_disabled_even_while_a_stopping_round_is_still_running() {
        for gate in [Gate::Off, isolated()] {
            for running in [false, true] {
                assert_eq!(
                    decide("settings", true, &gate, running),
                    Ok(FetchNowOutcome::Disabled),
                    "{gate:?} running={running}"
                );
            }
        }
    }

    #[test]
    fn no_scheduler_is_an_error() {
        assert_eq!(
            decide("settings", false, &Gate::Enabled, false),
            Err(NO_SCHEDULER.to_string())
        );
    }

    #[test]
    fn only_the_settings_window_may_request() {
        for label in [
            "",
            "main",
            "settings2",
            "w-clock",
            "w-macro",
            "w-dynamic-r2",
            "wallpaper-renderer-3",
            "fc-host-grid-overlay",
        ] {
            let result = decide(label, true, &Gate::Enabled, false);
            assert!(result.is_err(), "{label}: {result:?}");
            assert_ne!(result, Err(NO_SCHEDULER.to_string()), "{label}：先擋呼叫端");
        }
    }

    #[test]
    fn the_reply_serializes_with_the_outcome_strings() {
        for (outcome, text) in [
            (FetchNowOutcome::Started, "started"),
            (FetchNowOutcome::AlreadyRunning, "already_running"),
            (FetchNowOutcome::Disabled, "disabled"),
        ] {
            assert_eq!(
                serde_json::to_value(FetchNowReply { outcome }).unwrap(),
                serde_json::json!({ "outcome": text })
            );
            assert_eq!(outcome.as_str(), text);
        }
    }
}
