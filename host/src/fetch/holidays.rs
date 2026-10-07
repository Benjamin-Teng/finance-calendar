//! 休市日曆（TWSE openapi `holidaySchedule`，behavior-inventory 4.10／B-HOL-1～5、B-DATE-5、K-9；
//! 原始碼 `update_tw_events.py` 的 `fetch_holidays` 與 `main()` 的沿用段 `:1204-1208`）。
//!
//! # 排除「交易日說明列」的兩步判斷（B-HOL-4／5，memory `twse-holidayschedule-includes-trading-days`）
//!
//! API 除休市日外還混入交易日的說明列（「國曆新年開始交易日」「農曆春節前最後交易日」…），那些日子
//! 照常開盤、不可當休市。判斷兩步、不綁日期：
//!
//! 1. `Name` 含「開始交易」或「最後交易」（Python：`(開始|最後)交易` 的 `search`）＝疑似說明列；
//! 2. 但 `Description` 以「。」切開的**第一句**含「無交易」，這天本身是結算交割日（真休市），要留。
//!    只看第一句：2023-01-17（交易日）的說明第二句提到 1/18、1/19 無交易，看整段會誤判成休市。
//!
//! # 日期解析（B-DATE-5）
//!
//! `str(Date).strip()` 後須恰為 7 個字元且全是十進位數字（Unicode `Nd`，與 Python
//! `isdigit()`＋`int()` 一致，全形數字也收），前 3 碼民國年（＋1911）、中 2 碼月、後 2 碼日；
//! 不合法的日期（2 月 30 日、月＝0／13）略過該列。
//!
//! # 沿用與鍵省略（B-HOL-2、K-9）
//!
//! 來源失敗：記 `休市日曆來源失敗：{e}`，舊輸出有 `holidays` 陣列就沿用，否則 `value=None`＝輸出整鍵
//! 省略（三種「沒有」：沒有舊檔、舊值是 `null`、舊值型別不對）。本輪抓到（含空清單）＝`fresh`。
//!
//! # 與 Python 的差異（刻意）
//!
//! Python 對「回應不是列陣列」「列不是物件」不設防（在 `try` 之外崩潰、整輪不寫檔）。這裡：回應
//! 非空且非陣列＝來源失敗（走沿用）；列不是物件＝略過該列。

use std::collections::BTreeSet;
use std::sync::OnceLock;

use regex::Regex;
use serde_json::{Map, Value};
use time::{Date, Month};

use super::clock::Clock;
use super::dates::{digits_to_u32, is_py_space};
use super::errors::FetchError;
use super::http::{get_json, Fetch};
use super::json_util::{py_str, truthy};
use super::outcome::{guarded, SourceOutcome};
use super::output::PreviousOutput;

/// B-HOL-1：端點（只回當年度，跨年由前端週末規則兜底）。
pub const URL_HOLIDAY: &str = "https://openapi.twse.com.tw/v1/holidaySchedule/holidaySchedule";
/// B-FLOW-9：失敗前綴。
pub const ERR_PREFIX: &str = "休市日曆來源失敗：";

/// 舊輸出的 `holidays`（鍵不是陣列＝不存在；陣列逐筆只留字串）。
pub fn previous(prev: &PreviousOutput) -> Option<Vec<String>> {
    prev.get_items::<String>("holidays")
}

/// 由組裝端呼叫：[`run`] 加上 panic 隔離（D7）。
pub async fn run_guarded<F: Fetch>(
    fetch: &F,
    clock: &Clock,
    prev: &PreviousOutput,
) -> SourceOutcome<Option<Vec<String>>> {
    guarded(ERR_PREFIX, previous(prev), run(fetch, clock, prev)).await
}

/// 抓休市日曆、排除說明列、排序去重（B-HOL-1～5）。`clock` 不使用（API 只回當年度）。
async fn run<F: Fetch>(
    fetch: &F,
    _clock: &Clock,
    prev: &PreviousOutput,
) -> SourceOutcome<Option<Vec<String>>> {
    match fetch_rows(fetch).await {
        Ok(rows) => SourceOutcome::new(Some(parse_holidays(&rows)), true),
        Err(e) => {
            let mut out = SourceOutcome::new(previous(prev), false);
            out.logs.push(format!("[休市日曆] 來源失敗，略過：{e}"));
            if out.value.is_some() {
                out.logs
                    .push("[休市日曆] 本輪失敗，沿用上次資料".to_string());
            }
            out.errors.push(format!("{ERR_PREFIX}{e}"));
            out
        }
    }
}

/// `raw or []`：假值＝空；陣列＝各列；其餘＝失敗。
async fn fetch_rows<F: Fetch>(fetch: &F) -> Result<Vec<Value>, FetchError> {
    match get_json(fetch, URL_HOLIDAY).await? {
        Value::Array(rows) => Ok(rows),
        other if !truthy(&other) => Ok(Vec::new()),
        _ => Err(FetchError::Decode("回應不是 JSON 陣列".to_string())),
    }
}

/// B-HOL-3：只由字元數與十進位數字決定的 7 碼（Unicode `\d`＝`Nd`）。
fn seven_digits() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\d{7}$").expect("固定的 regex 必能編譯"))
}

fn parse_holidays(rows: &[Value]) -> Vec<String> {
    let mut out = BTreeSet::new();
    for row in rows {
        let Some(obj) = row.as_object() else {
            continue;
        };
        if is_trading_day_notice(obj) {
            continue;
        }
        let date_text = text_of(obj, "Date");
        if let Some(d) = parse_roc7(date_text.trim_matches(is_py_space)) {
            out.insert(format!(
                "{:04}-{:02}-{:02}",
                d.year(),
                u8::from(d.month()),
                d.day()
            ));
        }
    }
    out.into_iter().collect()
}

/// `str(r.get(key, ""))`。
fn text_of(obj: &Map<String, Value>, key: &str) -> String {
    obj.get(key).map(py_str).unwrap_or_default()
}

/// B-HOL-4：`Name` 命中且 `Description` 第一句不含「無交易」。
fn is_trading_day_notice(obj: &Map<String, Value>) -> bool {
    let name = text_of(obj, "Name");
    let desc = text_of(obj, "Description");
    let first_sentence = desc.split('。').next().unwrap_or_default();
    (name.contains("開始交易") || name.contains("最後交易")) && !first_sentence.contains("無交易")
}

/// 7 碼民國日期（B-DATE-5）：不是恰 7 個十進位數字、或日期不合法回 `None`。
fn parse_roc7(s: &str) -> Option<Date> {
    if !seven_digits().is_match(s) {
        return None;
    }
    let chars: Vec<char> = s.chars().collect();
    let num = |range: std::ops::Range<usize>| -> u32 {
        digits_to_u32(&chars[range].iter().collect::<String>())
    };
    let year = i32::try_from(num(0..3)).ok()? + 1911;
    let month = Month::try_from(u8::try_from(num(3..5)).ok()?).ok()?;
    let day = u8::try_from(num(5..7)).ok()?;
    Date::from_calendar_date(year, month, day).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::oracle;
    use crate::fetch::test_util::{block_on, MapFetch, PanicFetch};
    use serde_json::json;
    use time::macros::datetime;

    fn clock() -> Clock {
        Clock::new(
            datetime!(2026-10-05 00:00:00 +8),
            datetime!(2026-10-05 00:00:00),
        )
    }

    fn run_with(fetch: &MapFetch, prev: &PreviousOutput) -> SourceOutcome<Option<Vec<String>>> {
        block_on(run(fetch, &clock(), prev))
    }

    fn ok(rows: Value) -> MapFetch {
        MapFetch::new().ok(URL_HOLIDAY, &rows.to_string())
    }

    fn rows_of(rows: Value) -> Vec<String> {
        run_with(&ok(rows), &PreviousOutput::default())
            .value
            .expect("成功應有值")
    }

    // ───── B-HOL-1：端點 ─────

    #[test]
    fn b_hol_1_url_and_prefix_are_verbatim() {
        assert_eq!(
            URL_HOLIDAY,
            "https://openapi.twse.com.tw/v1/holidaySchedule/holidaySchedule"
        );
        assert_eq!(ERR_PREFIX, "休市日曆來源失敗：");
        let f = ok(json!([]));
        run_with(&f, &PreviousOutput::default());
        assert_eq!(f.requests(), [URL_HOLIDAY]);
    }

    // ───── B-HOL-2：失敗、沿用、鍵省略 ─────

    #[test]
    fn b_hol_2_success_is_fresh_even_when_empty() {
        let out = run_with(
            &ok(json!([])),
            &PreviousOutput::from_value(json!({"holidays": ["2026-01-01"]})),
        );
        assert_eq!(out.value, Some(vec![]));
        assert!(out.fresh && out.errors.is_empty() && out.logs.is_empty());
        // `raw or []`：假值回應也當空清單、算抓到。
        for body in ["null", "{}", "0", r#""""#] {
            let f = MapFetch::new().ok(URL_HOLIDAY, body);
            let out = run_with(&f, &PreviousOutput::default());
            assert_eq!(out.value, Some(vec![]), "{body}");
            assert!(out.fresh, "{body}");
        }
    }

    #[test]
    fn b_hol_2_failure_reuses_old_holidays_and_is_not_fresh() {
        let prev = PreviousOutput::from_value(json!({"holidays": ["2026-01-01", "2026-02-16"]}));
        let out = run_with(&MapFetch::new().status(URL_HOLIDAY, 500), &prev);
        assert_eq!(
            out.value,
            Some(vec!["2026-01-01".to_string(), "2026-02-16".to_string()])
        );
        assert!(!out.fresh);
        assert_eq!(
            out.errors,
            ["休市日曆來源失敗：HTTP Error 500: Internal Server Error"]
        );
        assert_eq!(
            out.logs,
            [
                "[休市日曆] 來源失敗，略過：HTTP Error 500: Internal Server Error",
                "[休市日曆] 本輪失敗，沿用上次資料",
            ]
        );
        // 舊值是空陣列（不是 None）也沿用。
        let prev = PreviousOutput::from_value(json!({"holidays": []}));
        let out = run_with(&MapFetch::new().network_error(URL_HOLIDAY, "boom"), &prev);
        assert_eq!(out.value, Some(vec![]));
    }

    #[test]
    fn b_hol_2_failure_omits_the_key_in_the_three_no_old_value_cases() {
        for prev in [
            PreviousOutput::default(),                               // 沒有舊檔
            PreviousOutput::from_value(json!({"holidays": null})),   // 舊值是 null
            PreviousOutput::from_value(json!({"other": 1})),         // 沒有此鍵
            PreviousOutput::from_value(json!({"holidays": "oops"})), // 型別不對（D8-3）
            PreviousOutput::from_value(json!({"holidays": {"a": 1}})),
        ] {
            let out = run_with(&MapFetch::new().status(URL_HOLIDAY, 503), &prev);
            assert_eq!(out.value, None, "{prev:?}");
            assert!(!out.fresh);
            assert_eq!(out.errors.len(), 1);
            assert_eq!(out.logs.len(), 1, "沒有舊值就沒有『沿用』那行");
        }
    }

    #[test]
    fn b_hol_2_old_holidays_are_checked_per_item() {
        let prev =
            PreviousOutput::from_value(json!({"holidays": ["2026-01-01", 5, null, "2026-02-16"]}));
        assert_eq!(
            previous(&prev),
            Some(vec!["2026-01-01".to_string(), "2026-02-16".to_string()])
        );
    }

    #[test]
    fn non_array_truthy_response_fails_the_source() {
        let prev = PreviousOutput::from_value(json!({"holidays": ["2026-01-01"]}));
        let out = run_with(&MapFetch::new().ok(URL_HOLIDAY, r#"{"a":1}"#), &prev);
        assert!(out.errors[0].starts_with(ERR_PREFIX));
        assert_eq!(out.value, Some(vec!["2026-01-01".to_string()]));
        // 壞 JSON、BOM 同樣是失敗。
        for body in ["{ not json", "\u{feff}[]"] {
            let out = run_with(
                &MapFetch::new().ok(URL_HOLIDAY, body),
                &PreviousOutput::default(),
            );
            assert_eq!(out.errors.len(), 1, "{body}");
            assert!(out.value.is_none());
        }
    }

    // ───── B-HOL-3：日期解析、排序去重 ─────

    #[test]
    fn b_hol_3_parses_roc_seven_digits_sorted_and_deduped() {
        let got = rows_of(json!([
            {"Name": "b", "Date": "1150216", "Description": ""},
            {"Name": "a", "Date": "1150101", "Description": ""},
            {"Name": "dup", "Date": "1150216", "Description": ""},
            {"Name": "old", "Date": "1100208", "Description": ""},
        ]));
        assert_eq!(got, ["2021-02-08", "2026-01-01", "2026-02-16"]);
    }

    #[test]
    fn b_hol_3_invalid_dates_skip_only_that_row() {
        let got = rows_of(json!([
            {"Name": "ok", "Date": "1150216"},
            {"Name": "6 碼", "Date": "115022"},
            {"Name": "8 碼", "Date": "11502288"},
            {"Name": "西元 8 碼", "Date": "20260216"},
            {"Name": "字母", "Date": "11A0228"},
            {"Name": "斜線", "Date": "115/2/28"},
            {"Name": "2/30", "Date": "1150230"},
            {"Name": "月 13", "Date": "1151301"},
            {"Name": "月 0", "Date": "1150001"},
            {"Name": "日 0", "Date": "1150200"},
            {"Name": "上標", "Date": "11503²3"},
            {"Name": "缺 Date"},
            {"Name": "null Date", "Date": null},
            {"Name": "空", "Date": ""},
        ]));
        assert_eq!(got, ["2026-02-16"]);
    }

    #[test]
    fn b_hol_3_date_text_is_stripped_and_non_strings_go_through_str() {
        let got = rows_of(json!([
            {"Name": "空白", "Date": " 1150228\t"},
            {"Name": "數字", "Date": 1150302},
            {"Name": "全形", "Date": "１１５０３０１"}, // Python isdigit()＋int() 認得
        ]));
        assert_eq!(got, ["2026-02-28", "2026-03-01", "2026-03-02"]);
        // 閏日：2028-02-29（民國 117）合法，2027-02-29（民國 116）不合法。
        assert_eq!(
            rows_of(json!([{"Date": "1170229"}, {"Date": "1160229"}])),
            ["2028-02-29"]
        );
    }

    // ───── B-HOL-4／5：說明列的兩步判斷 ─────

    #[test]
    fn b_hol_4_trading_day_notice_rows_are_not_holidays() {
        let got = rows_of(json!([
            {"Name": "國曆新年開始交易日", "Date": "1150102", "Description": "國曆新年後開始交易日。"},
            {"Name": "農曆春節前最後交易日", "Date": "1150211", "Description": "農曆春節前最後交易日。"},
            {"Name": "春節", "Date": "1150216", "Description": "無"},
        ]));
        assert_eq!(got, ["2026-02-16"]);
    }

    #[test]
    fn b_hol_4_only_the_first_sentence_of_description_counts() {
        // 2023-01-17 型：Name 不含說明列字樣，一律保留（Description 提到無交易也無妨）。
        assert_eq!(
            rows_of(
                json!([{"Name": "交易日", "Date": "1120117", "Description": "最後交易日為1/17。1/18、1/19無交易。"}])
            ),
            ["2023-01-17"]
        );
        // 說明列：第一句沒有「無交易」，第二句才有 → 仍排除（看整段會誤判成休市）。
        assert!(
            rows_of(json!([{"Name": "農曆春節前最後交易日", "Date": "1120117",
            "Description": "最後交易日為1/17。1/18、1/19無交易。"}]))
            .is_empty()
        );
        // 第一句為空（描述以「。」開頭）→ 排除；「無交易」在第一句 → 保留。
        assert!(rows_of(
            json!([{"Name": "最後交易日", "Date": "1150307", "Description": "。無交易"}])
        )
        .is_empty());
        assert_eq!(
            rows_of(
                json!([{"Name": "最後交易日", "Date": "1150308", "Description": "無交易。其他"}])
            ),
            ["2026-03-08"]
        );
    }

    #[test]
    fn b_hol_5_settlement_only_days_in_2021_are_real_closures() {
        // 2021／2022：結算交割日的 Name 也寫「農曆春節前最後交易日」，Description 第一句寫無交易。
        let got = rows_of(json!([
            {"Name": "農曆春節前最後交易日", "Date": "1100208",
             "Description": "2月8日市場無交易，僅辦理結算交割作業。"},
            {"Name": "農曆春節前最後交易日", "Date": "1100209",
             "Description": "2月9日為農曆春節前最後交易日。"},
        ]));
        assert_eq!(got, ["2021-02-08"]);
    }

    #[test]
    fn b_hol_4_missing_or_non_string_name_and_description_follow_python_str() {
        // 與 Python 實測一致（見 task 3.1 報告）：缺 Name／Description 當 ""；None→"None"；
        // 非字串 Description 以 str() 轉換後仍可含「無交易」。
        let got = rows_of(json!([
            {"Date": "1150303"},                                              // 缺 Name＋Description
            {"Name": null, "Date": "1150304", "Description": null},
            {"Name": "開始交易日", "Date": "1150305", "Description": null},   // "None" 不含無交易 → 排除
            {"Name": "開始交易日", "Date": "1150306"},                         // 缺 Description → 排除
            {"Name": "最後交易日", "Date": "1150309", "Description": ["無交易"]}, // str(list) 含無交易 → 保留
            {"Name": "開始交易", "Date": "1150310", "Description": "無"},      // 排除
            {"Name": "開始 交易日", "Date": "1150311", "Description": "無"},   // 中間有空格，不命中 → 保留
        ]));
        assert_eq!(
            got,
            ["2026-03-03", "2026-03-04", "2026-03-09", "2026-03-11"]
        );
    }

    #[test]
    fn non_object_rows_are_skipped_not_fatal() {
        assert_eq!(
            rows_of(json!(["oops", 5, null, [1], {"Date": "1150216"}])),
            ["2026-02-16"]
        );
    }

    #[test]
    fn python_cross_check_table_matches_the_real_fetch_holidays() {
        // 這張表的期望值來自真的跑 Python `fetch_holidays`（monkeypatch http_json），見 task 3.1 報告。
        let got = rows_of(json!([
            {"Name": "國曆新年開始交易日", "Date": "1150102", "Description": "國曆新年後開始交易日。"},
            {"Name": "農曆春節前最後交易日", "Date": "1150211", "Description": "農曆春節前最後交易日。"},
            {"Name": "農曆春節前最後交易日", "Date": "1100208", "Description": "2月8日市場無交易，僅辦理結算交割作業。"},
            {"Name": "交易日", "Date": "1120117", "Description": "最後交易日為1/17。1/18、1/19無交易。"},
            {"Name": "春節", "Date": "1150216", "Description": "無"},
            {"Name": "春節", "Date": "1150216", "Description": "重複"},
            {"Name": "和平紀念日", "Date": " 1150228 ", "Description": "x"},
            {"Name": "x", "Date": "115022", "Description": "x"},
            {"Name": "x", "Date": "11502288", "Description": "x"},
            {"Name": "x", "Date": "11A0228", "Description": "x"},
            {"Name": "二月三十", "Date": "1150230", "Description": "x"},
            {"Name": "月13", "Date": "1151301", "Description": "x"},
            {"Name": "月0", "Date": "1150001", "Description": "x"},
            {"Name": "全形", "Date": "１１５０３０１", "Description": "x"},
            {"Name": "數字", "Date": 1150302, "Description": "x"},
            {"Name": "上標", "Date": "11503²3", "Description": "x"},
            {"Date": "1150303"},
            {"Name": null, "Date": "1150304", "Description": null},
            {"Name": "開始交易日", "Date": "1150305", "Description": null},
            {"Name": "開始交易日", "Date": "1150306"},
            {"Name": "最後交易日", "Date": "1150307", "Description": "。無交易"},
            {"Name": "最後交易日", "Date": "1150308", "Description": "無交易。其他"},
            {"Name": "最後交易日", "Date": "1150309", "Description": ["無交易"]},
            {"Name": "開始交易", "Date": "1150310", "Description": "無"},
            {"Name": "開始 交易日", "Date": "1150311", "Description": "無"},
        ]));
        assert_eq!(
            got,
            [
                "2021-02-08",
                "2023-01-17",
                "2026-02-16",
                "2026-02-28",
                "2026-03-01",
                "2026-03-02",
                "2026-03-03",
                "2026-03-04",
                "2026-03-08",
                "2026-03-09",
                "2026-03-11"
            ]
        );
    }

    // ───── D7：panic 隔離 ─────

    #[test]
    fn d7_a_panic_inside_the_source_keeps_the_old_holidays() {
        let prev = PreviousOutput::from_value(json!({"holidays": ["2026-01-01"]}));
        let out = block_on(run_guarded(&PanicFetch, &clock(), &prev));
        assert_eq!(out.value, Some(vec!["2026-01-01".to_string()]));
        assert!(!out.fresh);
        assert!(out.errors[0].starts_with("休市日曆來源失敗：panic: "));
        // 沒有舊值＝省略該鍵。
        let out = block_on(run_guarded(
            &PanicFetch,
            &clock(),
            &PreviousOutput::default(),
        ));
        assert_eq!(out.value, None);
    }

    #[test]
    fn d7_run_guarded_is_transparent_when_nothing_panics() {
        let f = ok(json!([{"Name": "春節", "Date": "1150216"}]));
        let prev = PreviousOutput::default();
        assert_eq!(
            run_with(&f, &prev),
            block_on(run_guarded(&f, &clock(), &prev))
        );
    }

    // ───── oracle ─────

    #[test]
    fn oracle_every_scenario_matches_python_expected_json() {
        oracle::for_each_scenario(|name, sc, expected| {
            let out = block_on(run_guarded(&sc.fetch, &sc.clock, &sc.previous()));
            // `holidays` 鍵省略＝value 為 None。
            let want = expected.get("holidays").cloned().unwrap_or(Value::Null);
            assert_eq!(
                serde_json::to_value(&out.value).unwrap(),
                want,
                "情境 {name}：holidays 不符"
            );
            oracle::assert_errors(name, &out.errors, expected, &[ERR_PREFIX]);
        });
    }

    // ───── 移植：tests/test_wallpaper_data.py::HolidaysTest（用 tests/fixtures 的真實錄製回應；3.5 補上） ─────

    mod holidays_test {
        use super::*;
        use crate::fetch::test_util::py_fixture;
        use std::collections::BTreeSet;

        const FIXTURE: &str = "holiday_schedule_20261002.json";

        fn run_fetch(raw: Value) -> Vec<String> {
            let out = run_with(&ok(raw), &PreviousOutput::default());
            assert!(out.errors.is_empty(), "{:?}", out.errors);
            out.value.expect("成功應有值")
        }

        /// 人工逐列閱讀 Name／Description 語意後列出的交易日說明列，不由實作推導（見 Python 測試的註解）。
        const TRADING_DAYS: [(i32, [&str; 3]); 6] = [
            (2021, ["2021-01-04", "2021-02-05", "2021-02-17"]),
            (2022, ["2022-01-03", "2022-01-26", "2022-02-07"]),
            (2023, ["2023-01-03", "2023-01-17", "2023-01-30"]),
            (2024, ["2024-01-02", "2024-02-05", "2024-02-15"]),
            (2025, ["2025-01-02", "2025-01-22", "2025-02-03"]),
            (2026, ["2026-01-02", "2026-02-11", "2026-02-23"]),
        ];

        /// rwd 回應（`{data: [[日期, 名稱, 說明], …]}`，日期為西元 ISO）→ openapi 形狀（`Date` 民國 7 碼）。
        /// 只在測試層轉形狀，正式程式仍只抓 openapi。
        fn rwd_rows(year: i32) -> Value {
            let j = py_fixture(&format!("holiday_schedule_rwd_{year}_20261002.json"));
            Value::Array(
                j["data"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|r| {
                        let d = r[0].as_str().unwrap();
                        let mut parts = d.split('-');
                        let y: i32 = parts.next().unwrap().parse().unwrap();
                        let (mo, dd) = (parts.next().unwrap(), parts.next().unwrap());
                        json!({
                            "Name": r[1],
                            "Date": format!("{:03}{mo}{dd}", y - 1911),
                            "Description": r[2],
                        })
                    })
                    .collect(),
            )
        }

        fn raw_for(year: i32) -> Value {
            if year == 2026 {
                py_fixture(FIXTURE)
            } else {
                rwd_rows(year)
            }
        }

        #[test]
        fn test_trading_day_notice_rows_excluded() {
            let out = run_fetch(py_fixture(FIXTURE));
            for d in ["2026-01-02", "2026-02-11", "2026-02-23"] {
                assert!(
                    !out.iter().any(|x| x == d),
                    "{d} 是交易日（開始／最後交易日說明列），不得列為休市"
                );
            }
        }

        #[test]
        fn test_settlement_only_days_kept() {
            let out = run_fetch(py_fixture(FIXTURE));
            for d in ["2026-02-12", "2026-02-13"] {
                assert!(
                    out.iter().any(|x| x == d),
                    "{d}「市場無交易，僅辦理結算交割作業」是真的不交易，必須保留"
                );
            }
        }

        #[test]
        fn test_regular_holidays_kept() {
            let out = run_fetch(py_fixture(FIXTURE));
            for d in [
                "2026-01-01",
                "2026-02-16",
                "2026-02-20",
                "2026-04-03",
                "2026-10-09",
                "2026-12-25",
            ] {
                assert!(out.iter().any(|x| x == d), "{d}");
            }
            let sorted: Vec<String> = out
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            assert_eq!(out, sorted, "輸出維持排序、去重");
        }

        #[test]
        fn test_settlement_days_named_as_last_trading_day_are_kept() {
            // 2021／2022 的真實寫法：Name＝農曆春節前最後交易日，Description 第一句寫市場無交易
            for (year, days) in [
                (2021, ["2021-02-08", "2021-02-09"]),
                (2022, ["2022-01-27", "2022-01-28"]),
            ] {
                let out = run_fetch(raw_for(year));
                for d in days {
                    assert!(
                        out.iter().any(|x| x == d),
                        "{d} 市場無交易、僅辦理結算交割，是真休市"
                    );
                }
            }
        }

        #[test]
        fn test_trading_day_whose_description_mentions_other_days_no_trading() {
            // 2023-01-17 是交易日；說明第二句提到 1/18、1/19 無交易，不可因此被當成休市
            let out = run_fetch(raw_for(2023));
            assert!(!out.iter().any(|x| x == "2023-01-17"));
            for d in ["2023-01-18", "2023-01-19"] {
                assert!(out.iter().any(|x| x == d), "{d}");
            }
        }

        #[test]
        fn test_every_row_2021_to_2026_classified_correctly() {
            // 每年每一列：輸出＝資料中所有日期扣掉人工列出的交易日說明列
            for (year, trading) in TRADING_DAYS {
                let raw = raw_for(year);
                let all_dates: BTreeSet<String> = raw
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|r| {
                        let d = r["Date"].as_str().unwrap();
                        let y: i32 = d[..3].parse().unwrap();
                        format!("{}-{}-{}", y + 1911, &d[3..5], &d[5..7])
                    })
                    .collect();
                for d in trading {
                    assert!(
                        all_dates.contains(d),
                        "{year} 資料裡應有交易日說明列 {d}（fixture 或清單有誤）"
                    );
                }
                let want: Vec<String> = all_dates
                    .iter()
                    .filter(|d| !trading.contains(&d.as_str()))
                    .cloned()
                    .collect();
                assert_eq!(run_fetch(raw), want, "{year}");
            }
        }
    }
}
