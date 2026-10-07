//! 設定視窗的「抓取狀態」查詢（data-layer-rust tasks.md 5.5、design.md D12；spec
//! `market-data-fetch`「標示資料來源與抓取狀態」）。
//!
//! ## 指令與資料來源
//!
//! [`get_fetch_status`]：一次性查詢，設定視窗開啟時呼叫一次（不推送、不輪詢；與 `get_settings` 同為
//! 「頁面主動查詢」，沒有資料需要持續推給訂閱者，所以不走 `ipc::Channel`）。只接受 label 為 `settings` 的
//! webview（[`require_settings_window`]；做法與 `wallpaper_settings` 的指令相同——`default.json` capability
//! 涵蓋所有小工具與設定視窗、沒有 per-command 權限，限制只能由指令自己依呼叫端 label 擋）。
//!
//! 回傳 [`FetchStatusView`]：
//!
//! - `kind`：`active`／`off_by_setting`／`off_isolated`，由 [`super::scheduler::decide_gate`] 決定——與排程
//!   迴圈判定抓取開關用的是同一個函式與同樣的輸入（目前設定的 `data_fetch`、行程啟動時同一種偵測
//!   [`crate::desktop::detect_isolated_local_app_data`]），所以頁面顯示的與排程實際做的一致；
//! - `last_start`／`last_finish`：排程紀錄 `fetch-state.json` 的上一輪開始與完成時間，轉成**本機時間**
//!   `YYYY-MM-DD HH:MM`；沒有紀錄（從沒抓過、紀錄壞掉）或一輪從沒結束＝`null`；
//! - `source_failed`：上一輪是否有來源失敗。**以資料檔 `tw_events.json` 的 `errors` 是否非空為準**
//!   （`資料目錄\tw_events.json`，與前端小工具讀的是同一份、也是 Python 版頁腳「來源失敗」的依據），
//!   再加上排程紀錄的 `last_any_success == false`（整輪沒有任何來源成功、或寫檔失敗——這時資料檔沒被
//!   更新、`errors` 還是舊的）。選資料檔而非只看排程紀錄，是因為紀錄只存「成敗」一個布林，不知道有幾個來源
//!   失敗；而「部分來源失敗」正是 `errors` 非空但仍有來源成功的情況。資料檔讀不到或解析失敗＝視為沒有
//!   錯誤（顯示面不該為了讀不到診斷檔而誤報失敗）；
//! - `round_failed`：排程紀錄的 `last_any_success == false`——**整輪沒有任何來源成功，或來源成功但寫檔失敗**
//!   （`scheduler.rs` 把兩者記成同一個布林，指令分不出是哪一種）。所以頁面用中性的「本輪未成功更新」，不寫
//!   「所有來源失敗」（寫檔失敗時來源其實都成功、使用者會誤去查網路）；
//! - `settings_path`：宿主實際使用的設定檔路徑（`AppState::settings_path`），隔離說明要告訴使用者去哪裡改
//!   `data_fetch`；
//! - `isolation`：隔離時的兩個路徑（環境變數 `LOCALAPPDATA` 與系統登記），供頁面寫明原因；其餘狀態為
//!   `null`。
//!
//! 狀態不是 `active` 時 `last_*`／`source_failed` 仍照實回傳（那是上次還在抓取時留下的），由頁面決定要不要
//! 顯示——頁面在「未抓取」時主文字只寫原因，另在補充行顯示「最後一次更新」（有完成時間才有），隔離時再列
//! 兩個路徑與設定檔路徑。

use std::path::Path;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Webview};
use time::macros::format_description;
use time::{OffsetDateTime, UtcOffset};

use crate::desktop::{self, LocalAppDataMismatch};
use crate::settings::DataFetch;
use crate::widgets::AppState;

use super::output::OUTPUT_FILE;
use super::sched::FetchState;
use super::scheduler::{decide_gate, Gate};
use super::state;

/// 抓取目前的狀態種類（頁面據此選文字；穩定字串）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FetchStatusKind {
    /// 抓取啟用中。
    Active,
    /// 使用者把 `data_fetch` 設成 `off`。
    OffBySetting,
    /// `data_fetch="auto"` 且偵測到隔離環境。
    OffIsolated,
}

/// 隔離環境的兩個路徑（[`LocalAppDataMismatch`] 的輸出形狀）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IsolationView {
    /// 環境變數 `LOCALAPPDATA`（展開後）。
    pub env_value: String,
    /// 系統登記的 LocalAppData（展開後）。
    pub registered: String,
}

/// [`get_fetch_status`] 的回應（欄位語意見模組文件）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FetchStatusView {
    pub kind: FetchStatusKind,
    pub last_start: Option<String>,
    pub last_finish: Option<String>,
    pub source_failed: bool,
    /// 上一輪沒有成功更新：整輪沒有任何來源成功、**或**來源成功但寫檔失敗（兩者在排程紀錄裡是同一個
    /// 布林，分不出來）；頁面據此把「部分來源失敗」改成中性的「本輪未成功更新」。為 `true` 時
    /// `source_failed` 一定也是 `true`。
    pub round_failed: bool,
    /// 宿主實際使用的設定檔路徑（隔離環境的說明要告訴使用者去哪裡改 `data_fetch`）。
    pub settings_path: String,
    pub isolation: Option<IsolationView>,
}

/// 頁面顯示用的本機時間格式。
fn local_stamp(
    t: OffsetDateTime,
    to_local: &dyn Fn(OffsetDateTime) -> UtcOffset,
) -> Option<String> {
    t.to_offset(to_local(t))
        .format(format_description!("[year]-[month]-[day] [hour]:[minute]"))
        .ok()
}

/// 資料檔裡只關心 `errors`。
#[derive(Deserialize)]
struct OutputErrors {
    #[serde(default)]
    errors: Vec<serde_json::Value>,
}

/// 資料檔的 `errors` 是否非空；檔案不存在、讀不到、不是 JSON 物件＝`false`。
pub fn output_has_errors(data_dir: &Path) -> bool {
    let Ok(bytes) = std::fs::read(data_dir.join(OUTPUT_FILE)) else {
        return false;
    };
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    serde_json::from_slice::<OutputErrors>(bytes)
        .map(|o| !o.errors.is_empty())
        .unwrap_or(false)
}

/// 組出回應（純函式，輸入都由呼叫端給，測試可注入）。
pub fn build_view(
    gate: &Gate,
    record: Option<&FetchState>,
    output_has_errors: bool,
    settings_path: &Path,
    to_local: &dyn Fn(OffsetDateTime) -> UtcOffset,
) -> FetchStatusView {
    let round_failed = record.is_some_and(|r| !r.last_any_success);
    let (kind, isolation) = match gate {
        Gate::Enabled => (FetchStatusKind::Active, None),
        Gate::Off => (FetchStatusKind::OffBySetting, None),
        Gate::Isolated(m) => (
            FetchStatusKind::OffIsolated,
            Some(IsolationView {
                env_value: m.env_value.clone(),
                registered: m.registered.clone(),
            }),
        ),
    };
    FetchStatusView {
        kind,
        last_start: record.and_then(|r| local_stamp(r.last_start, to_local)),
        last_finish: record
            .and_then(|r| r.last_finish)
            .and_then(|t| local_stamp(t, to_local)),
        source_failed: output_has_errors || round_failed,
        round_failed,
        settings_path: settings_path.display().to_string(),
        isolation,
    }
}

/// 讀排程紀錄與資料檔後組出回應；與指令分開，讓測試不必建 Tauri 視窗。
pub fn fetch_status(
    mode: DataFetch,
    isolation: Option<&LocalAppDataMismatch>,
    data_dir: &Path,
    state_path: &Path,
    settings_path: &Path,
    to_local: &dyn Fn(OffsetDateTime) -> UtcOffset,
) -> FetchStatusView {
    let gate = decide_gate(mode, isolation);
    let record = state::load(state_path);
    build_view(
        &gate,
        record.as_ref(),
        output_has_errors(data_dir),
        settings_path,
        to_local,
    )
}

/// 該時刻的本機 UTC 偏移；取不到（極少見）退回 UTC。
fn system_local_offset(t: OffsetDateTime) -> UtcOffset {
    UtcOffset::local_offset_at(t).unwrap_or(UtcOffset::UTC)
}

/// 呼叫端 webview 必須是設定視窗。
pub fn require_settings_window(label: &str) -> Result<(), String> {
    if label == crate::tray::SETTINGS_WINDOW_LABEL {
        Ok(())
    } else {
        log::warn!("拒絕視窗 {label} 呼叫抓取狀態指令（只給設定視窗）");
        Err(format!("視窗 {label} 不能呼叫抓取狀態指令"))
    }
}

/// 設定視窗的抓取狀態查詢（見模組文件）。非同步指令、檔案讀取丟到阻塞執行緒：不佔用主執行緒。
#[tauri::command]
pub async fn get_fetch_status(webview: Webview, app: AppHandle) -> Result<FetchStatusView, String> {
    require_settings_window(webview.label())?;
    let (data_dir, mode, settings_path) = {
        let state = app.state::<AppState>();
        let settings = state
            .settings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (
            settings.data_dir.clone(),
            settings.data_fetch,
            state.settings_path.clone(),
        )
    };
    tauri::async_runtime::spawn_blocking(move || {
        let isolation = desktop::detect_isolated_local_app_data();
        fetch_status(
            mode,
            isolation.as_ref(),
            &data_dir,
            &state::default_state_path(),
            &settings_path,
            &system_local_offset,
        )
    })
    .await
    .map_err(|e| format!("查詢抓取狀態失敗：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::test_util::TempDir;
    use time::macros::{datetime, offset};

    const TPE: &dyn Fn(OffsetDateTime) -> UtcOffset = &|_| offset!(+8);
    const SP: &str = r"C:\Users\Ben\AppData\Roaming\tw.fintools.fc-host\settings.json";

    fn record(any_success: bool) -> FetchState {
        FetchState {
            last_start: datetime!(2026-10-05 07:02:11 UTC),
            last_finish: Some(datetime!(2026-10-05 07:02:52 UTC)),
            last_any_success: any_success,
            consecutive_failures: u32::from(!any_success),
        }
    }

    fn mismatch() -> LocalAppDataMismatch {
        LocalAppDataMismatch {
            env_value: r"C:\Temp\iso\Local".into(),
            registered: r"C:\Users\Ben\AppData\Local".into(),
        }
    }

    fn write_output(dir: &Path, errors_json: &str) {
        std::fs::write(
            dir.join(OUTPUT_FILE),
            format!(r#"{{"updated":"x","errors":{errors_json},"events":[]}}"#),
        )
        .unwrap();
    }

    /// 狀態一：抓取啟用（時間轉成本機時間、沒有失敗）。
    #[test]
    fn active_reports_local_times_and_no_failure() {
        let dir = TempDir::new("status-active");
        write_output(dir.path(), "[]");
        let state_path = dir.path().join("fetch-state.json");
        state::save(&state_path, &record(true)).unwrap();

        let v = fetch_status(
            DataFetch::Auto,
            None,
            dir.path(),
            &state_path,
            Path::new(SP),
            TPE,
        );
        assert_eq!(v.kind, FetchStatusKind::Active);
        assert_eq!(v.last_start.as_deref(), Some("2026-10-05 15:02"));
        assert_eq!(v.last_finish.as_deref(), Some("2026-10-05 15:02"));
        assert!(!v.source_failed);
        assert!(!v.round_failed);
        assert_eq!(v.settings_path, SP);
        assert_eq!(v.isolation, None);
    }

    /// 狀態一的失敗變體：資料檔 `errors` 非空（部分來源失敗，仍有來源成功）。
    #[test]
    fn active_with_errors_in_the_output_reports_source_failure() {
        let dir = TempDir::new("status-active-errors");
        write_output(dir.path(), r#"["總經日曆：x"]"#);
        let state_path = dir.path().join("fetch-state.json");
        state::save(&state_path, &record(true)).unwrap();

        let v = fetch_status(
            DataFetch::On,
            None,
            dir.path(),
            &state_path,
            Path::new(SP),
            TPE,
        );
        assert_eq!(v.kind, FetchStatusKind::Active);
        assert!(v.source_failed);
        assert!(!v.round_failed, "有來源成功、只是部分失敗：不是整輪失敗");
    }

    /// 整輪沒有任何來源成功（或寫檔失敗，資料檔沒更新）：即使資料檔 errors 是舊的空陣列也算失敗。
    #[test]
    fn a_round_with_no_successful_source_counts_as_failed_even_if_the_output_is_stale() {
        let dir = TempDir::new("status-no-success");
        write_output(dir.path(), "[]");
        let state_path = dir.path().join("fetch-state.json");
        state::save(&state_path, &record(false)).unwrap();

        let v = fetch_status(
            DataFetch::Auto,
            None,
            dir.path(),
            &state_path,
            Path::new(SP),
            TPE,
        );
        assert!(v.source_failed);
        assert!(v.round_failed);
    }

    /// 狀態二：使用者在設定關閉（不論是否隔離）。
    #[test]
    fn off_by_setting_even_in_an_isolated_environment() {
        let dir = TempDir::new("status-off");
        let state_path = dir.path().join("fetch-state.json");
        let m = mismatch();
        let v = fetch_status(
            DataFetch::Off,
            Some(&m),
            dir.path(),
            &state_path,
            Path::new(SP),
            TPE,
        );
        assert_eq!(v.kind, FetchStatusKind::OffBySetting);
        assert_eq!(v.isolation, None);
    }

    /// 狀態三：隔離環境（auto＋不一致）帶兩個路徑；`on` 在隔離環境仍抓取（使用者明確要求）。
    #[test]
    fn isolated_environment_reports_both_paths_and_on_overrides_it() {
        let dir = TempDir::new("status-isolated");
        let state_path = dir.path().join("fetch-state.json");
        let m = mismatch();

        let v = fetch_status(
            DataFetch::Auto,
            Some(&m),
            dir.path(),
            &state_path,
            Path::new(SP),
            TPE,
        );
        assert_eq!(v.kind, FetchStatusKind::OffIsolated);
        assert_eq!(
            v.isolation,
            Some(IsolationView {
                env_value: r"C:\Temp\iso\Local".into(),
                registered: r"C:\Users\Ben\AppData\Local".into(),
            })
        );
        // 沒有紀錄、沒有資料檔：時間 null、沒有失敗。
        assert_eq!(v.last_start, None);
        assert_eq!(v.last_finish, None);
        assert!(!v.source_failed);

        let v = fetch_status(
            DataFetch::On,
            Some(&m),
            dir.path(),
            &state_path,
            Path::new(SP),
            TPE,
        );
        assert_eq!(v.kind, FetchStatusKind::Active);
        assert_eq!(v.isolation, None);
    }

    #[test]
    fn a_round_that_never_finished_has_no_finish_time() {
        let mut r = record(true);
        r.last_finish = None;
        let v = build_view(&Gate::Enabled, Some(&r), false, Path::new(SP), TPE);
        assert_eq!(v.last_start.as_deref(), Some("2026-10-05 15:02"));
        assert_eq!(v.last_finish, None);
    }

    #[test]
    fn an_unreadable_or_non_object_output_is_not_a_failure() {
        let dir = TempDir::new("status-bad-output");
        assert!(!output_has_errors(dir.path()), "檔案不存在");
        for bad in [
            "",
            "not json",
            "[]",
            "{\"errors\":\"x\"}",
            "\u{feff}{\"errors\":[]}",
        ] {
            std::fs::write(dir.path().join(OUTPUT_FILE), bad).unwrap();
            assert!(!output_has_errors(dir.path()), "{bad:?}");
        }
        std::fs::write(dir.path().join(OUTPUT_FILE), "\u{feff}{\"errors\":[\"x\"]}").unwrap();
        assert!(output_has_errors(dir.path()), "BOM 後的非空 errors 要認得");
    }

    #[test]
    fn the_view_serializes_with_stable_snake_case_keys() {
        let v = build_view(
            &Gate::Isolated(mismatch()),
            Some(&record(true)),
            false,
            Path::new(SP),
            TPE,
        );
        let json = serde_json::to_value(&v).unwrap();
        assert_eq!(json["kind"], "off_isolated");
        assert_eq!(json["isolation"]["env_value"], r"C:\Temp\iso\Local");
        assert_eq!(
            json["isolation"]["registered"],
            r"C:\Users\Ben\AppData\Local"
        );
        assert_eq!(json["last_start"], "2026-10-05 15:02");
        assert_eq!(json["source_failed"], false);
        assert_eq!(json["round_failed"], false);
        assert_eq!(json["settings_path"], SP);
        let off =
            serde_json::to_value(build_view(&Gate::Off, None, false, Path::new(SP), TPE)).unwrap();
        assert_eq!(off["kind"], "off_by_setting");
        assert!(off["last_start"].is_null() && off["isolation"].is_null());
    }

    #[test]
    fn only_the_settings_window_may_call_the_command() {
        assert!(require_settings_window("settings").is_ok());
        for label in ["w-clock", "w-quotes", "main", "settings2", ""] {
            assert!(require_settings_window(label).is_err(), "{label}");
        }
    }
}
