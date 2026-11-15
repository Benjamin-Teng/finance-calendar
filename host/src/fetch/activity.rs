//! 抓取活動狀態（fetch-progress-and-manual-refresh design.md D1、D2；spec `market-data-fetch`「抓取進行狀態」、
//! `widget-data-feed`「抓取完成後立即更新快照」）。
//!
//! # 狀態
//!
//! [`FetchActivity`]＝`{ running, enabled, last }`，序列化成 `{"running":bool,"enabled":bool,"last":"none"|"ok"|"failed"}`：
//!
//! - `running`：排程迴圈決定開始一輪到該輪結束（含等網路、含被中止、含 panic 被接住）之間為真；
//! - `enabled`：[`super::scheduler::decide_gate`] 是否為 `Gate::Enabled`；
//! - `last`：行程內最近一輪的結果；啟動時由排程紀錄 `fetch-state.json` 的 `last_any_success` 初始化，沒有紀錄＝`none`。
//!   被中止的一輪不改它。
//!
//! 內容是非敏感 UI 狀態（不含資料、路徑、代理），所以與 `edit-mode`／`widget-font` 一樣用**事件廣播**
//! （`fetch-activity`，payload 即整個狀態），不走 `ipc::Channel`；頁面另以 [`get_fetch_activity`] 隨時查詢。
//!
//! # 寫入者
//!
//! 只有排程執行緒寫（`scheduler.rs` 的 `SchedEnv::activity`，正式環境經 [`publish_to_app`]）。存放在 Tauri managed
//! state 的 [`FetchActivityStore`]，由 `scheduler::register_activity` 在 `build_ui` 建立小工具視窗**之前**登記
//! （`spawn_for_app` 啟動執行緒前會再確認一次）。登記之前、或排程因「正在因更新結束」而沒有登記時，[`get_fetch_activity`]
//! 回錯誤；頁面把它當成「狀態未知」，等第一筆 `fetch-activity` 事件校正（design.md D8；排程第一次醒來一定會送）。
//!
//! # 結束通知的順序（[`publish`]）
//!
//! `running` 由真變假時，先同步跑一次資料輪詢（與 `main.rs` 資料輪詢執行緒同一個函式
//! `widgets::poll_and_wake`，含桌布協調迴圈的 `DataChanged` 通知），再更新狀態、廣播事件。後端保證的只有這個
//! **呼叫順序**：新快照已進註冊表、已交給 `Channel::send`，之後才 `emit`。**送達順序不保證**——Tauri 的 `Channel::send`
//! 對 8192 bytes 以上的 JSON 不直接送達，而是讓頁面再發一次 IPC 取回（`tauri` 的 `ipc/channel.rs`，
//! `MAX_JSON_DIRECT_EXECUTE_THRESHOLD`），`tw-events` 快照遠大於門檻；`emit` 則直接送達，所以頁面通常先收到
//! 「沒有抓取進行中」、隔一次 IPC 來回才拿到快照。「結束通知不早於新快照」由小工具外殼補上（design.md D8：收到
//! `running` 由真變假且手上沒有有效快照時，先維持載入並主動查詢一次快照——此時後端已經有新快照）。
//! 狀態放在輪詢**之後**才更新，所以輪詢期間查詢到的仍是 `running: true`。
//!
//! 兩條執行緒（資料輪詢執行緒與這裡）可能同時輪詢。讀過 `data.rs`／`widgets.rs` 的結論：
//! `widgets::poll_and_notify` 在**同一次持有 registry 鎖**的期間呼叫 `ChannelRegistry::poll_all_changed`，
//! 而「檔案有沒有變」的判斷狀態（`JsonFileSource` 記住的修改時間＋大小）與快照的覆寫都在這個呼叫裡完成——所以同一次
//! 檔案改寫只會被其中一條執行緒看到、只推送一次；另一條這輪什麼都不推。推送的快照與世代在之後另一次持鎖時一起讀出，
//! 讀到的是當下最新的那份，不會是較舊世代（頁面另外也會丟棄較舊世代）。因此不需要改成「喚醒資料輪詢執行緒並等它」。
//! 剩下的理論窗口是「兩次持鎖之間檔案又被改寫一次」：兩條執行緒各推一份同世代快照、送出順序可能顛倒；這要在幾毫秒內
//! 改寫兩次資料檔，一輪抓取只寫一次，屬偏門情境，只記錄。

use std::sync::{Mutex, PoisonError};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Webview};

use super::sched::FetchState;

/// 最近一輪的結果（序列化為 `none`／`ok`／`failed`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum LastRound {
    /// 尚未有過任何一輪（沒有排程紀錄）。
    #[serde(rename = "none")]
    Never,
    /// 寫檔成功且有任何來源成功。
    #[serde(rename = "ok")]
    Succeeded,
    /// 寫檔失敗、整輪 panic、或沒有任何來源成功。
    #[serde(rename = "failed")]
    Failed,
}

impl LastRound {
    /// 由排程紀錄初始化（行程啟動時）：沒有紀錄＝[`LastRound::Never`]。
    pub fn from_record(record: Option<&FetchState>) -> LastRound {
        match record {
            None => LastRound::Never,
            Some(r) if r.last_any_success => LastRound::Succeeded,
            Some(_) => LastRound::Failed,
        }
    }
}

/// 抓取活動狀態（見模組文件「狀態」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FetchActivity {
    pub running: bool,
    pub enabled: bool,
    pub last: LastRound,
}

/// Tauri managed state：目前的活動狀態。
#[derive(Debug)]
pub struct FetchActivityStore(Mutex<FetchActivity>);

impl FetchActivityStore {
    pub fn new(initial: FetchActivity) -> Self {
        FetchActivityStore(Mutex::new(initial))
    }

    pub fn get(&self) -> FetchActivity {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn set(&self, next: FetchActivity) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = next;
    }
}

/// 更新狀態並通知（見模組文件「結束通知的順序」）：`running` 由真變假時先 `poll`，再寫入 `store`、`emit`。
/// 只有排程執行緒呼叫，所以「讀舊值 → 輪詢 → 寫新值」之間沒有別的寫入者。`poll` panic 時狀態不更新、不廣播：
/// 排程迴圈記得的「上次送出的值」也不會更新，下一次醒來會再送一次（再輪詢一次）。
pub fn publish(
    store: &FetchActivityStore,
    next: FetchActivity,
    poll: impl FnOnce(),
    emit: impl FnOnce(&FetchActivity),
) {
    if store.get().running && !next.running {
        poll();
    }
    store.set(next);
    emit(&next);
}

/// 正式環境的寫入點（`scheduler.rs` 的 `HostEnv::activity`）。沒有登記 store（不會發生：`spawn_for_app` 確認已登記才
/// 啟動執行緒）就什麼都不做。正在因更新結束時只更新狀態，不輪詢、不廣播（收尾期間不再推東西給正在關閉的視窗）。
pub fn publish_to_app(app: &AppHandle, next: FetchActivity) {
    let Some(store) = app.try_state::<FetchActivityStore>() else {
        return;
    };
    if crate::updater::is_exiting_for_update() {
        store.set(next);
        return;
    }
    publish(
        &store,
        next,
        || {
            crate::widgets::poll_and_wake(app);
        },
        |activity| {
            let _ = app.emit("fetch-activity", activity);
        },
    );
}

/// [`get_fetch_activity`] 只接受小工具視窗（`w-<id>`／重建的 `w-<id>-r<n>`，以
/// [`crate::widgets::widget_id_from_label`] 判斷）與設定視窗；本機自訂指令沒有逐指令 ACL，限制只能由指令自己擋。
pub fn require_activity_caller(label: &str) -> Result<(), String> {
    if label == crate::tray::SETTINGS_WINDOW_LABEL
        || crate::widgets::widget_id_from_label(label).is_some()
    {
        Ok(())
    } else {
        log::warn!("拒絕視窗 {label} 查詢抓取活動狀態（只給小工具與設定視窗）");
        Err(format!("視窗 {label} 不能查詢抓取活動狀態"))
    }
}

/// 查詢目前的抓取活動狀態（頁面先 `listen('fetch-activity')` 再呼叫，查詢在途期間收到的事件優先）。
/// 狀態還沒登記（理論上 `build_ui` 在建小工具前就登記；正在因更新結束時不登記）回錯誤 `抓取活動狀態尚未就緒`——
/// 語意是「未知」，不是「抓取關閉」：頁面不顯示載入也不顯示關閉提示，等第一筆事件校正（design.md D8）。
#[tauri::command]
pub fn get_fetch_activity(webview: Webview, app: AppHandle) -> Result<FetchActivity, String> {
    require_activity_caller(webview.label())?;
    app.try_state::<FetchActivityStore>()
        .map(|store| store.get())
        .ok_or_else(|| "抓取活動狀態尚未就緒".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use time::macros::datetime;

    fn act(running: bool, enabled: bool, last: LastRound) -> FetchActivity {
        FetchActivity {
            running,
            enabled,
            last,
        }
    }

    #[test]
    fn serializes_with_stable_field_names_and_last_strings() {
        for (last, text) in [
            (LastRound::Never, "none"),
            (LastRound::Succeeded, "ok"),
            (LastRound::Failed, "failed"),
        ] {
            let json = serde_json::to_value(act(true, false, last)).unwrap();
            assert_eq!(
                json,
                serde_json::json!({ "running": true, "enabled": false, "last": text })
            );
        }
    }

    #[test]
    fn last_is_initialized_from_the_schedule_record() {
        let record = |ok| FetchState {
            last_start: datetime!(2026-11-14 04:00:00 UTC),
            last_finish: Some(datetime!(2026-11-14 04:00:40 UTC)),
            last_any_success: ok,
            consecutive_failures: u32::from(!ok),
        };
        assert_eq!(LastRound::from_record(None), LastRound::Never);
        assert_eq!(
            LastRound::from_record(Some(&record(true))),
            LastRound::Succeeded
        );
        assert_eq!(
            LastRound::from_record(Some(&record(false))),
            LastRound::Failed
        );
    }

    /// 呼叫順序：結束時先輪詢、後廣播；輪詢期間 store 仍是 `running: true`。（只驗呼叫順序；頁面的送達順序不由後端保證，
    /// 見模組文件「結束通知的順序」。）
    #[test]
    fn the_end_of_a_round_polls_before_updating_and_broadcasting() {
        let store = FetchActivityStore::new(act(false, true, LastRound::Never));
        let calls = RefCell::new(Vec::<String>::new());

        publish(
            &store,
            act(true, true, LastRound::Never),
            || calls.borrow_mut().push("poll".into()),
            |a| {
                calls
                    .borrow_mut()
                    .push(format!("emit running={}", a.running))
            },
        );
        assert_eq!(*calls.borrow(), ["emit running=true"], "開始一輪不輪詢");

        calls.borrow_mut().clear();
        publish(
            &store,
            act(false, true, LastRound::Succeeded),
            || {
                assert!(store.get().running, "輪詢期間查到的仍是進行中");
                calls.borrow_mut().push("poll".into());
            },
            |a| {
                calls
                    .borrow_mut()
                    .push(format!("emit running={}", a.running))
            },
        );
        assert_eq!(*calls.borrow(), ["poll", "emit running=false"]);
        assert_eq!(store.get(), act(false, true, LastRound::Succeeded));

        // 沒有抓取進行中時的變化（例如閘門改變）不輪詢。
        calls.borrow_mut().clear();
        publish(
            &store,
            act(false, false, LastRound::Succeeded),
            || calls.borrow_mut().push("poll".into()),
            |a| {
                calls
                    .borrow_mut()
                    .push(format!("emit enabled={}", a.enabled))
            },
        );
        assert_eq!(*calls.borrow(), ["emit enabled=false"]);
    }

    /// 輪詢 panic：狀態不更新、不廣播（排程迴圈下一次醒來會補送、再輪詢一次）。
    #[test]
    fn a_panicking_poll_leaves_the_store_unchanged() {
        let store = FetchActivityStore::new(act(true, true, LastRound::Never));
        let emitted = RefCell::new(0);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            publish(
                &store,
                act(false, true, LastRound::Succeeded),
                || panic!("輪詢炸了"),
                |_| *emitted.borrow_mut() += 1,
            );
        }));
        assert!(result.is_err());
        assert_eq!(*emitted.borrow(), 0);
        assert!(store.get().running);
    }

    #[test]
    fn only_widget_and_settings_windows_may_query() {
        for label in [
            "settings",
            "w-clock",
            "w-macro",
            "w-quotes",
            "w-custom3",
            "w-dynamic-r2",
        ] {
            assert!(require_activity_caller(label).is_ok(), "{label}");
        }
        for label in [
            "",
            "main",
            "settings2",
            "w-probe",
            "w-",
            "w-macro-rx",
            "wallpaper-renderer-3",
            "fc-host-grid-overlay",
        ] {
            assert!(require_activity_caller(label).is_err(), "{label}");
        }
    }
}
