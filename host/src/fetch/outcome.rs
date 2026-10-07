//! 各來源共用的回傳形狀與 panic 隔離（design.md D7；task 3.1 定下，3.2～3.5 沿用）。
//!
//! # 每個來源一個檔、一個形狀
//!
//! ```ignore
//! async fn run<F: Fetch>(fetch: &F, clock: &Clock, prev: &PreviousOutput) -> SourceOutcome<T>;   // 模組私有
//! pub fn previous(prev: &PreviousOutput) -> T;   // 來源整個失敗時「沿用舊資料」的值
//! pub async fn run_guarded<F: Fetch>(..) -> SourceOutcome<T>;   // = guarded(前綴, previous(prev), run(..))
//! ```
//!
//! [`SourceOutcome::value`] 是**已套用 Python `main()` 對該來源的沿用規則之後**、要放進輸出的值
//! （省略鍵的情況用 `Option`），所以組裝端（4.1）只負責依序呼叫（一律呼叫 `run_guarded`；`run` 刻意不公開，免得繞過 panic 防護）、串接
//! `errors`、合併 `events`、計算 `counts`／`fetched`，不再有任何來源專屬的邏輯。
//!
//! # 動態桌布三鍵（3.5）：另一個型別 [`WallpaperOutcome`]
//!
//! [`guarded`] 把 panic 寫進 `errors`、格式 `{前綴}panic: {訊息}`，適用於前十個來源。動態桌布三鍵
//! （加權盤中走勢／日 K／券資比）的錯誤要進 `wallpaper_errors`（B-GRD-1：
//! `{label}來源發生非預期錯誤：{類名}: {e}`）、**不進** `errors`、**不計** `fresh`（B-GRD-2）。這三件事
//! 用**型別**保證：三鍵回傳 [`WallpaperOutcome`]（只有 `value`、`wallpaper_errors`、`logs`，沒有
//! `errors`／`fresh` 欄位），組裝端（4.1）沒有辦法把它們誤接進 `errors` 或 `any_fresh`；panic 隔離用
//! [`guarded_wallpaper`]（**不可**用 [`guarded`]）。
//!
//! # panic 隔離（D7）
//!
//! 來源是序列執行、同一個行程內共用輸出；任何一個來源的 bug（索引越界、`unwrap`…）不可讓整輪
//! 不寫檔。[`guarded`] 用 `catch_unwind` 把 panic 轉成「該來源失敗＋沿用舊資料」的 outcome。
//! 這是**最後防線**：預期內的壞資料仍要在來源裡以一般失敗路徑處理（`errors` 字串與 Python 一致）。
//! 前提是 `panic = "unwind"`（本專案 profile 未改 `panic`，預設即 unwind）。

use std::future::Future;
use std::panic::AssertUnwindSafe;

use futures_util::FutureExt;

/// 一個來源跑完的結果。
#[derive(Debug, Clone, PartialEq)]
pub struct SourceOutcome<T> {
    /// 要放進輸出的值（已套沿用規則）。
    pub value: T,
    /// 要進輸出 `errors` 的字串，依 Python 的順序（動態桌布三鍵的錯誤不在這裡，見 [`WallpaperOutcome`]）。
    pub errors: Vec<String>,
    /// 是否計入 `any_fresh`（B-FLOW-7；`fetched` 刷新條件）。
    pub fresh: bool,
    /// 日誌行（B-FLOW-18，不含時間戳；給 5.4 的記錄輸出用），依 Python 的順序。
    pub logs: Vec<String>,
}

impl<T> SourceOutcome<T> {
    /// 沒有錯誤、沒有日誌；`fresh` 由呼叫端指定。
    pub fn new(value: T, fresh: bool) -> Self {
        SourceOutcome {
            value,
            errors: Vec::new(),
            fresh,
            logs: Vec::new(),
        }
    }
}

/// 跑一個來源，panic 時改回傳「失敗＋沿用舊資料」。
///
/// - `error_prefix`：該來源的失敗前綴（逐字取自 B-FLOW-9，如 `總經來源失敗（本週檔）：`），panic 的
///   錯誤字串是 `{error_prefix}panic: {訊息}`，所以 [`super::errors::normalize_error`] 仍認得它。
/// - `fallback`：該來源整個失敗時應沿用的值（各來源的 `previous(prev)`）。
///
/// panic 時回傳的 outcome 一律 `fresh: false`。
pub async fn guarded<T, Fut>(error_prefix: &str, fallback: T, fut: Fut) -> SourceOutcome<T>
where
    Fut: Future<Output = SourceOutcome<T>>,
{
    match AssertUnwindSafe(fut).catch_unwind().await {
        Ok(outcome) => outcome,
        Err(payload) => {
            let msg = panic_message(payload.as_ref());
            log::error!("fetch: 來源 panic，改用沿用資料：{error_prefix}{msg}");
            SourceOutcome {
                value: fallback,
                errors: vec![format!("{error_prefix}panic: {msg}")],
                fresh: false,
                logs: vec![format!(
                    "[panic] {error_prefix}{msg}，本輪改用沿用值（沒有舊值則為空）"
                )],
            }
        }
    }
}

/// 動態桌布三鍵（加權盤中走勢、日 K、券資比）一個來源跑完的結果（B-GRD-1～3，K-10）。
///
/// 與 [`SourceOutcome`] 的差別：錯誤進 `wallpaper_errors`（輸出的同名頂層鍵，**不是** `errors`），而且
/// **沒有** `fresh` 欄位——三鍵永遠不計入 `any_fresh`（否則只要 Yahoo 通，`fetched` 每輪刷新、蓋掉
/// 「已 N 天未更新」）。
#[derive(Debug, Clone, PartialEq)]
pub struct WallpaperOutcome<T> {
    /// 要放進輸出的值（已套沿用規則）；`None`＝整鍵省略（全新安裝又失敗）。三鍵的值保留原始 JSON 形狀
    /// （沿用的舊值原樣放回，同 Python 的 `return old`）。
    pub value: T,
    /// 要進輸出 `wallpaper_errors` 的字串，依 Python 的順序。
    pub wallpaper_errors: Vec<String>,
    /// 日誌行（B-FLOW-18，不含時間戳），依 Python 的順序。
    pub logs: Vec<String>,
}

impl<T> WallpaperOutcome<T> {
    /// 沒有錯誤、沒有日誌。
    pub fn new(value: T) -> Self {
        WallpaperOutcome {
            value,
            wallpaper_errors: Vec::new(),
            logs: Vec::new(),
        }
    }
}

/// [`guarded`] 的桌布版（Python 的 `guarded_wallpaper_fetch`，B-GRD-1）：panic 時回傳 `fallback`（各來源的
/// `previous(prev)`：舊值型別對才沿用，否則 `None`），並記
/// `{label}來源發生非預期錯誤：panic: {訊息}`（Python 是 `{類名}: {e}`，`{類名}: {e}` 部分的內容不同、
/// 前綴相同；4.2 以前綴正規化比對）到 `wallpaper_errors`。
pub async fn guarded_wallpaper<T, Fut>(label: &str, fallback: T, fut: Fut) -> WallpaperOutcome<T>
where
    Fut: Future<Output = WallpaperOutcome<T>>,
{
    match AssertUnwindSafe(fut).catch_unwind().await {
        Ok(outcome) => outcome,
        Err(payload) => {
            let msg = panic_message(payload.as_ref());
            log::error!("fetch: 桌布來源 panic，改用沿用資料：{label}：{msg}");
            WallpaperOutcome {
                value: fallback,
                wallpaper_errors: vec![format!("{label}來源發生非預期錯誤：panic: {msg}")],
                logs: vec![format!("[{label}] 非預期錯誤，沿用上次資料：{msg}")],
            }
        }
    }
}

/// `panic!("…")`／`panic!("{x}")` 的載荷是 `&str` 或 `String`；其餘型別給固定文字。
pub(crate) fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "（非文字的 panic 載荷）".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::test_util::block_on;

    #[test]
    fn passes_a_normal_outcome_through_untouched() {
        let mut o = SourceOutcome::new(vec![1, 2], true);
        o.errors.push("e".into());
        o.logs.push("l".into());
        let got = block_on(guarded("前綴：", vec![9], async { o.clone() }));
        assert_eq!(got, o);
    }

    #[test]
    fn a_panic_becomes_a_failed_outcome_with_the_fallback() {
        let got = block_on(guarded("休市日曆來源失敗：", Some(vec!["x"]), async {
            if true {
                panic!("boom {}", 7);
            }
            SourceOutcome::new(None, true)
        }));
        assert_eq!(got.value, Some(vec!["x"]));
        assert_eq!(got.errors, ["休市日曆來源失敗：panic: boom 7"]);
        assert!(!got.fresh);
        assert_eq!(got.logs.len(), 1);
        // 前綴仍被正規化認得。
        assert_eq!(
            crate::fetch::errors::normalize_error(&got.errors[0]),
            "休市日曆來源失敗："
        );
    }

    #[test]
    fn non_string_panic_payloads_do_not_panic_again() {
        let got = block_on(guarded("前綴：", 0u8, async {
            if true {
                std::panic::panic_any(42_i32);
            }
            SourceOutcome::new(1u8, true)
        }));
        assert_eq!(got.value, 0);
        assert!(got.errors[0].starts_with("前綴：panic: "));
    }

    #[test]
    fn wallpaper_guard_passes_a_normal_outcome_through_untouched() {
        let mut o = WallpaperOutcome::new(Some(1u8));
        o.wallpaper_errors.push("e".into());
        o.logs.push("l".into());
        let got = block_on(guarded_wallpaper("標籤", Some(9u8), async { o.clone() }));
        assert_eq!(got, o);
    }

    #[test]
    fn wallpaper_guard_records_the_panic_in_wallpaper_errors_with_the_b_grd_1_prefix() {
        let got = block_on(guarded_wallpaper("加權日K", Some(vec![1]), async {
            if true {
                panic!("boom {}", 7);
            }
            WallpaperOutcome::new(None)
        }));
        assert_eq!(got.value, Some(vec![1]));
        assert_eq!(
            got.wallpaper_errors,
            ["加權日K來源發生非預期錯誤：panic: boom 7"]
        );
        assert_eq!(got.logs.len(), 1);
        assert_eq!(
            crate::fetch::errors::normalize_error(&got.wallpaper_errors[0]),
            "加權日K來源發生非預期錯誤："
        );
    }

    #[test]
    fn wallpaper_guard_without_a_fallback_yields_none() {
        let got = block_on(guarded_wallpaper::<Option<u8>, _>(
            "券資比／融資維持率",
            None,
            async {
                tokio::task::yield_now().await;
                if true {
                    std::panic::panic_any(42_i32);
                }
                WallpaperOutcome::new(Some(1))
            },
        ));
        assert_eq!(got.value, None);
        assert_eq!(got.wallpaper_errors.len(), 1);
        assert!(got.wallpaper_errors[0].starts_with("券資比／融資維持率來源發生非預期錯誤："));
    }

    #[test]
    fn a_panic_after_an_await_point_is_also_caught() {
        let got = block_on(guarded("前綴：", 1u8, async {
            tokio::task::yield_now().await;
            if true {
                panic!("late");
            }
            SourceOutcome::new(2u8, true)
        }));
        assert_eq!(got.value, 1);
        assert_eq!(got.errors, ["前綴：panic: late"]);
    }
}
