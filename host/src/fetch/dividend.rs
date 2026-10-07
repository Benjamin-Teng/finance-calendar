//! 除權息（TWSE TWT48U；behavior-inventory 4.2／B-DIV-1～6、K-6、K-12；原始碼 `update_tw_events.py` 的
//! `fetch_dividend`／`fmt_cash`，`:344-349`、`:405-433`，與 `main()` 的沿用段 `:1157-1164`）。
//!
//! # 兩個端點與備援（B-DIV-1／2／3／5）
//!
//! 主端點 RWD（`www.twse.com.tw/rwd/zh/exRight/TWT48U`，即時）的 `data` 是「陣列的陣列」，欄位依位置：
//! `r[0]` 日期、`r[1]` 代號、`r[2]` 名稱、`r[3]` 權息別、`r[7]` 現金股利。**主端點成功（即使 `data`
//! 為空）就直接回傳，不會走備援**。主端點失敗（連線、HTTP 狀態、壞 JSON、解析中途出錯）只記日誌
//! （`[除權息] 即時 API 失敗（…），改用 openapi 備援`），**不進 `errors`**，改用 openapi
//! （`TWT48U_ALL`，物件陣列、欄位 `Date`／`Code`／`Name`／`Exdividend`／`CashDividend`）。兩者都失敗才記
//! `除權息來源失敗：{e}` 並沿用舊輸出的 dividend 事件（B-FLOW-6：dividend 一組）。
//!
//! # B-DIV-6：主端點中途失敗時的殘留列（刻意重現，D-3）
//!
//! Python 的 `ev` 在主端點失敗後**沒有清空**就接著跑備援：主端點處理到第 N 列才出錯（如某列少於 8 欄），
//! 前 N−1 列留在 `ev`，備援再追加完整資料，之後由 B-EVT-3 的去重吃掉重複。這裡以同一個 `ev` 先後傳給兩個
//! 階段、失敗時不清空來重現。兩者都失敗時 `ev` 整個丟棄（Python 回傳 `None`）。
//!
//! # 欄位與 `note`（B-DIV-2／3／4，U-4）
//!
//! `note` 一律以「除」開頭（前端 U-4 依賴）：`"除" + 權息別 + fmt_cash(現金)`。權息別：主端點
//! `str(r[3]).strip() or "權息"`；備援 `r.get("Exdividend") or "權息"`（**不 strip**、不 `str()`）。
//! [`fmt_cash`]：`float(str(cash).strip())`，`> 0` 才輸出 ` 現金{v:g}元`（前導一個空白；`%g` 用
//! [`super::dates::fmt_g`]，K-6），其餘（≤ 0、`nan`、解析失敗）為空字串。
//!
//! # 與 Python 對齊的細節（每一項都用 Python 實測過，見測試）
//!
//! Python 對 JSON 形狀完全不設防、靠 `except Exception` 兜底；這裡逐項重現「哪些形狀會失敗、哪些會被
//! 當成空或略過」，不是一概當失敗：
//!
//! - RWD `data`：`null`／假值＝空；陣列＝各列；**物件＝逐鍵、字串＝逐字元**（每個元素是字串，`r[0]` 取第
//!   一個字元、日期解析不了＝略過；空字串元素 `r[0]` 越界＝失敗）；數字、`true` 是 `TypeError`＝失敗。
//!   頂層不是物件（含陣列、`null`）＝失敗（`.get`）。
//! - RWD 每列：陣列依位置取值（日期解析不了先略過、**不碰後面的欄位**；欄位不足＝失敗）；字串列同上；
//!   物件列（`r[0]` 是 `KeyError`）、`null`、數字列＝失敗。日期欄不是字串也可（`str()` 後解析，如整數
//!   `1151008`）。
//! - openapi：頂層 `null`／數字＝失敗；物件逐鍵、字串逐字元，**元素不是物件就失敗**（只有空物件、空字串
//!   會成功並回空清單）；每列 `Exdividend` 為真值卻不是字串（數字、陣列）＝失敗（Python 的
//!   `str + int` `TypeError`），假值（`0`、`null`、`""`）＝「權息」。`Code`／`Name` 缺＝空字串、`null`＝
//!   `None`（`str(None)`）。
//! - `fmt_cash`：Python `float()` 的寬鬆（底線 `1_0`、全形數字、前後空白）與 `str()` 近似見
//!   [`super::json_util::py_float`]／`py_str`。**非有限值（`inf`、`Infinity`、`1e400`、`nan`）依 D8-6 當成
//!   「無法解析的金額」**，走與 Python `ValueError` 相同的路徑：回空字串、`note` 不帶現金（Python 對 `inf`
//!   會輸出 ` 現金inf元`，這是 D8-6 的刻意改良；`nan`／`-inf` 本來就因 `> 0` 為假而是空字串）。
//!
//! # 與 Python 的已知差異
//!
//! `str()` 對巢狀陣列／物件欄位（`r[1]` 是陣列之類）用 JSON 文字而非 Python repr；JSON 整數超出 u64 時
//! 先被解析成浮點。真實來源的這些欄位恆為字串，不會發生。

use serde_json::Value;

use super::clock::Clock;
use super::dates::{fmt_g, iso_date};
use super::events::previous_events;
use super::http::{get_json, Fetch};
use super::json_util::{
    py_dict, py_float, py_index, py_iter, py_str, py_str_get, py_strip, py_type_name,
    roc_to_date_value, truthy,
};
use super::outcome::{guarded, SourceOutcome};
use super::output::{Event, EventType, PreviousOutput};

/// B-DIV-1：主端點（即時）。
pub const URL_DIV_RWD: &str = "https://www.twse.com.tw/rwd/zh/exRight/TWT48U?response=json";
/// B-DIV-1：備援端點。
pub const URL_DIV_OAPI: &str = "https://openapi.twse.com.tw/v1/exchangeReport/TWT48U_ALL";
/// B-FLOW-9：兩者都失敗時的錯誤前綴。
pub const ERR_PREFIX: &str = "除權息來源失敗：";

/// 舊輸出中 `type == "dividend"` 的事件（B-FLOW-6）。
pub fn previous(prev: &PreviousOutput) -> Vec<Event> {
    previous_events(prev, &[EventType::Dividend])
}

/// 由組裝端呼叫：[`run`] 加上 panic 隔離（D7）。
pub async fn run_guarded<F: Fetch>(
    fetch: &F,
    clock: &Clock,
    prev: &PreviousOutput,
) -> SourceOutcome<Vec<Event>> {
    guarded(ERR_PREFIX, previous(prev), run(fetch, clock, prev)).await
}

/// 抓除權息：主端點 → 備援 → 沿用（B-DIV-1～6）。`value` 是未套窗口、未去重的事件（B-EVT 在組裝時做）。
/// `fresh`＝任一端點成功（`div_ev is not None`，B-FLOW-7）。
async fn run<F: Fetch>(
    fetch: &F,
    _clock: &Clock,
    prev: &PreviousOutput,
) -> SourceOutcome<Vec<Event>> {
    // B-DIV-6：兩個階段共用同一個 `ev`，主端點中途失敗時殘留的列不清空。
    let mut ev: Vec<Event> = Vec::new();
    let mut logs = Vec::new();
    match fetch_rwd(fetch, &mut ev).await {
        Ok(()) => return finished(ev, logs),
        Err(e) => logs.push(format!("[除權息] 即時 API 失敗（{e}），改用 openapi 備援")),
    }
    match fetch_openapi(fetch, &mut ev).await {
        Ok(()) => finished(ev, logs),
        Err(e) => {
            logs.push("[除權息] 本輪失敗，沿用上次資料".to_string());
            let mut out = SourceOutcome::new(previous(prev), false);
            out.errors.push(format!("{ERR_PREFIX}{e}"));
            out.logs = logs;
            out
        }
    }
}

fn finished(ev: Vec<Event>, logs: Vec<String>) -> SourceOutcome<Vec<Event>> {
    let mut out = SourceOutcome::new(ev, true);
    out.logs = logs;
    out
}

/// 主端點：`j.get("data") or []`，逐列依位置取值（B-DIV-2）。
async fn fetch_rwd<F: Fetch>(fetch: &F, ev: &mut Vec<Event>) -> Result<(), String> {
    let j = get_json(fetch, URL_DIV_RWD)
        .await
        .map_err(|e| e.to_string())?;
    let mut top = match j {
        Value::Object(map) => map,
        other => {
            return Err(format!(
                "'{}' object has no attribute 'get'",
                py_type_name(&other)
            ))
        }
    };
    let rows = match top.remove("data") {
        Some(data) if truthy(&data) => py_iter(data)?,
        _ => Vec::new(),
    };
    for r in rows {
        // 日期解析不了就略過，不碰後面的欄位（欄位不足的列只要日期壞掉就不會失敗）。
        let Some(d) = roc_to_date_value(Some(&py_index(&r, 0)?)) else {
            continue;
        };
        let code = py_strip(&py_str(&py_index(&r, 1)?)).to_string();
        let name = py_strip(&py_str(&py_index(&r, 2)?)).to_string();
        let kind = py_str(&py_index(&r, 3)?);
        let kind = match py_strip(&kind) {
            "" => "權息",
            k => k,
        };
        let cash = fmt_cash(&py_index(&r, 7)?);
        ev.push(Event {
            date: iso_date(d),
            kind: EventType::Dividend,
            code,
            name,
            note: format!("除{kind}{cash}"),
        });
    }
    Ok(())
}

/// 備援：物件陣列（B-DIV-3）。
async fn fetch_openapi<F: Fetch>(fetch: &F, ev: &mut Vec<Event>) -> Result<(), String> {
    let j = get_json(fetch, URL_DIV_OAPI)
        .await
        .map_err(|e| e.to_string())?;
    for r in py_iter(j)? {
        let obj = py_dict(&r)?;
        let Some(d) = roc_to_date_value(obj.get("Date")) else {
            continue;
        };
        // `(r.get("Exdividend") or "權息")`：真值必須是字串（`"除" + 非字串` 是 TypeError），且不 strip。
        let kind = match obj.get("Exdividend") {
            Some(Value::String(s)) if !s.is_empty() => s.clone(),
            Some(v) if truthy(v) => {
                return Err(format!(
                    "can only concatenate str (not \"{}\") to str",
                    py_type_name(v)
                ))
            }
            _ => "權息".to_string(),
        };
        let cash = fmt_cash(obj.get("CashDividend").unwrap_or(&Value::Null));
        ev.push(Event {
            date: iso_date(d),
            kind: EventType::Dividend,
            code: py_strip(&py_str_get(obj, "Code")).to_string(),
            name: py_strip(&py_str_get(obj, "Name")).to_string(),
            note: format!("除{kind}{cash}"),
        });
    }
    Ok(())
}

/// Python `fmt_cash`（B-DIV-4、K-6）：` 現金{v:g}元`（`v > 0` 且為有限值，D8-6），其餘空字串。
pub fn fmt_cash(cash: &Value) -> String {
    match py_float(py_strip(&py_str(cash))) {
        Some(v) if v.is_finite() && v > 0.0 => format!(" 現金{}元", fmt_g(v)),
        _ => String::new(),
    }
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

    fn run_with(fetch: &MapFetch, prev: &PreviousOutput) -> SourceOutcome<Vec<Event>> {
        block_on(run(fetch, &clock(), prev))
    }

    fn rwd(body: Value) -> MapFetch {
        MapFetch::new().ok(URL_DIV_RWD, &body.to_string())
    }

    /// 主端點故障（500）、備援回 `body`。
    fn oapi(body: Value) -> MapFetch {
        MapFetch::new()
            .status(URL_DIV_RWD, 500)
            .ok(URL_DIV_OAPI, &body.to_string())
    }

    fn row(date: &str, code: &str, name: &str, kind: &str, cash: &str) -> Value {
        json!([date, code, name, kind, "", "", "", cash])
    }

    fn notes(out: &SourceOutcome<Vec<Event>>) -> Vec<String> {
        out.value
            .iter()
            .map(|e| format!("{} {} {} {}", e.date, e.code, e.name, e.note))
            .collect()
    }

    fn old_dividend() -> PreviousOutput {
        PreviousOutput::from_value(json!({"events": [
            {"date":"2026-10-07","type":"dividend","code":"1101","name":"台泥","note":"除息 現金1元"},
            {"date":"2026-10-08","type":"meeting","code":"2330","name":"台積電","note":"股東會"},
        ]}))
    }

    // ───── B-DIV-1／2：端點與主端點解析 ─────

    #[test]
    fn b_div_1_urls_and_prefix_are_verbatim() {
        assert_eq!(
            URL_DIV_RWD,
            "https://www.twse.com.tw/rwd/zh/exRight/TWT48U?response=json"
        );
        assert_eq!(
            URL_DIV_OAPI,
            "https://openapi.twse.com.tw/v1/exchangeReport/TWT48U_ALL"
        );
        assert_eq!(ERR_PREFIX, "除權息來源失敗：");
    }

    #[test]
    fn b_div_2_main_endpoint_maps_columns_by_position_and_never_touches_the_fallback() {
        let f = rwd(json!({"data": [
            row("115/10/08", " 2330 ", " 台積電 ", " 息 ", " 6.0 "),
            row("1151009", "2317", "鴻海", "權息", "0.5"),
        ]}));
        let out = run_with(&f, &PreviousOutput::default());
        assert_eq!(
            notes(&out),
            [
                "2026-10-08 2330 台積電 除息 現金6元",
                "2026-10-09 2317 鴻海 除權息 現金0.5元",
            ]
        );
        assert!(out.fresh && out.errors.is_empty() && out.logs.is_empty());
        assert_eq!(f.requests(), [URL_DIV_RWD]);
        assert!(out.value.iter().all(|e| e.kind == EventType::Dividend));
    }

    #[test]
    fn b_div_2_empty_success_is_still_success_and_skips_the_fallback() {
        for body in [
            json!({"data": []}),
            json!({"data": null}),
            json!({}),
            json!({"data": 0}),
            json!({"data": ""}),
            json!({"data": false}),
        ] {
            let f = rwd(body.clone());
            let out = run_with(&f, &old_dividend());
            assert!(out.value.is_empty(), "{body}");
            assert!(out.fresh && out.errors.is_empty(), "{body}");
            assert_eq!(f.requests(), [URL_DIV_RWD], "{body}");
        }
    }

    #[test]
    fn b_div_2_blank_kind_becomes_quan_xi_and_note_always_starts_with_chu() {
        let f = rwd(json!({"data": [
            row("115/10/08", "1", "a", "", "0"),
            row("115/10/08", "2", "b", "   ", "-1"),
            row("115/10/08", "3", "c", "權", "abc"),
            row("115/10/08", "4", "d", "息", ""),
        ]}));
        let out = run_with(&f, &PreviousOutput::default());
        let got: Vec<_> = out.value.iter().map(|e| e.note.as_str()).collect();
        assert_eq!(got, ["除權息", "除權息", "除權", "除息"]);
        assert!(out.value.iter().all(|e| e.note.starts_with('除')));
    }

    #[test]
    fn b_div_2_rows_with_unparsable_dates_are_skipped_without_touching_other_columns() {
        // 日期壞掉的列即使欄位不足也只是略過（Python 先判日期、`continue`）。
        let f = rwd(json!({"data": [
            ["bad"], [], ["", "x"], [null], [""], ["20261008", "x"],
            row("115/10/08", "2330", "台積電", "息", "1"),
            [0], [false],
        ]}));
        // `[]` 的 `r[0]` 越界＝失敗，要走備援。
        let out = run_with(&f, &PreviousOutput::default());
        assert!(out.logs[0].starts_with("[除權息] 即時 API 失敗（list index out of range）"));
        // 去掉 `[]` 後才是純略過。
        let f = rwd(json!({"data": [
            ["bad"], ["", "x"], [null], [""], ["20261008", "x"], [0], [false],
            row("115/10/08", "2330", "台積電", "息", "1"),
        ]}));
        let out = run_with(&f, &PreviousOutput::default());
        assert_eq!(notes(&out), ["2026-10-08 2330 台積電 除息 現金1元"]);
        assert!(out.logs.is_empty() && out.errors.is_empty());
        assert_eq!(f.requests(), [URL_DIV_RWD]);
    }

    #[test]
    fn b_div_2_date_column_may_be_a_non_string() {
        // Python：`roc_to_date(1151008)` → `str()` 後解析。
        let f = rwd(json!({"data": [[1151008, 2330, "x", "息", "", "", "", 0.5]]}));
        let out = run_with(&f, &PreviousOutput::default());
        assert_eq!(notes(&out), ["2026-10-08 2330 x 除息 現金0.5元"]);
    }

    // ───── B-DIV-5：失敗、備援 ─────

    #[test]
    fn b_div_5_main_failure_falls_back_logs_only_and_never_records_an_error() {
        for main in [
            MapFetch::new().status(URL_DIV_RWD, 500),
            MapFetch::new().network_error(URL_DIV_RWD, "boom"),
            MapFetch::new().ok(URL_DIV_RWD, "{ not json"),
            MapFetch::new().ok(URL_DIV_RWD, "\u{feff}{}"),
        ] {
            let f = main.ok(
                URL_DIV_OAPI,
                &json!([{"Date":"1151008","Code":"2330","Name":"台積電","Exdividend":"息","CashDividend":"6"}])
                    .to_string(),
            );
            let out = run_with(&f, &old_dividend());
            assert_eq!(notes(&out), ["2026-10-08 2330 台積電 除息 現金6元"]);
            assert!(out.errors.is_empty(), "主端點失敗不進 errors");
            assert!(out.fresh);
            assert_eq!(out.logs.len(), 1);
            assert!(out.logs[0].starts_with("[除權息] 即時 API 失敗（"));
            assert!(out.logs[0].ends_with("），改用 openapi 備援"));
            assert_eq!(f.requests(), [URL_DIV_RWD, URL_DIV_OAPI]);
        }
    }

    #[test]
    fn b_div_5_both_fail_records_one_error_and_reuses_only_old_dividend_events() {
        let f = MapFetch::new()
            .status(URL_DIV_RWD, 500)
            .status(URL_DIV_OAPI, 503);
        let out = run_with(&f, &old_dividend());
        assert_eq!(out.value, previous(&old_dividend()));
        assert_eq!(out.value.len(), 1, "只沿用 dividend、不含舊的 meeting");
        assert!(!out.fresh);
        assert_eq!(
            out.errors,
            ["除權息來源失敗：HTTP Error 503: Service Unavailable"]
        );
        assert_eq!(out.logs.len(), 2);
        assert!(out.logs[0].starts_with("[除權息] 即時 API 失敗（HTTP Error 500"));
        assert_eq!(out.logs[1], "[除權息] 本輪失敗，沿用上次資料");
        // 沒有舊檔＝空清單（仍是失敗、不算 fresh）。
        let out = run_with(&f, &PreviousOutput::default());
        assert!(out.value.is_empty() && !out.fresh && out.errors.len() == 1);
    }

    // ───── B-DIV-6：主端點中途失敗的殘留列 ─────

    #[test]
    fn b_div_6_rows_before_a_mid_stream_main_failure_stay_and_the_fallback_appends() {
        let f = rwd(json!({"data": [
            row("115/10/08", "2330", "台積電", "息", "6"),
            row("115/10/09", "2317", "鴻海", "息", "1"),
            ["115/10/10", "1101", "台泥"], // 少於 8 欄：第 3 列才出錯
            row("115/10/11", "9999", "不會出現", "息", "1"),
        ]}))
        .ok(
            URL_DIV_OAPI,
            &json!([
                {"Date":"1151008","Code":"2330","Name":"台積電","Exdividend":"息","CashDividend":"6"},
                {"Date":"1151012","Code":"2412","Name":"中華電","Exdividend":"息","CashDividend":"4"},
            ])
            .to_string(),
        );
        let out = run_with(&f, &PreviousOutput::default());
        assert_eq!(
            notes(&out),
            [
                // 殘留的主端點前兩列（B-DIV-6，AGENTS.md 與此不同，以程式為準）
                "2026-10-08 2330 台積電 除息 現金6元",
                "2026-10-09 2317 鴻海 除息 現金1元",
                // 備援完整追加（含重複的 2330，之後由 B-EVT-3 去重）
                "2026-10-08 2330 台積電 除息 現金6元",
                "2026-10-12 2412 中華電 除息 現金4元",
            ]
        );
        assert!(out.errors.is_empty() && out.fresh);
        assert!(out.logs[0].contains("list index out of range"));
        // 去重後重複消失（B-EVT-3）。
        let (s, e) = crate::fetch::events::window(clock().tpe_date());
        let merged = crate::fetch::events::merge_events(out.value, s, e);
        assert_eq!(merged.len(), 3);
    }

    #[test]
    fn b_div_6_both_fail_after_partial_rows_discards_the_partial_rows() {
        let f = rwd(json!({"data": [
            row("115/10/08", "2330", "台積電", "息", "6"),
            ["115/10/10"],
        ]}))
        .status(URL_DIV_OAPI, 500);
        let out = run_with(&f, &old_dividend());
        assert_eq!(out.value, previous(&old_dividend()), "殘留列不可外洩");
        assert_eq!(out.errors.len(), 1);
    }

    // ───── 主端點形狀（Python 實測）：哪些是失敗、哪些是略過 ─────

    #[test]
    fn rwd_shapes_that_python_treats_as_failure_fall_back() {
        let bad_main = [
            json!([]),                                        // `list.get`
            json!(null),                                      // `None.get`
            json!("x"),                                       // `str.get`
            json!(5),                                         // `int.get`
            json!({"data": 5}),                               // 數字不可迭代
            json!({"data": true}),                            // 布林不可迭代
            json!({"data": [{"a": 1}]}),                      // 物件列：`r[0]` KeyError
            json!({"data": [null]}),                          // `None[0]`
            json!({"data": [5]}),                             // `int[0]`
            json!({"data": [[]]}),                            // `r[0]` 越界
            json!({"data": [""]}),                            // 空字串列：`r[0]` 越界
            json!({"data": {"": 1}}),                         // 物件逐鍵、空鍵：越界
            json!({"data": [["115/10/08", "1", "a", "息"]]}), // 日期有效、`r[7]` 越界
            json!({"data": [["115/10/08"]]}),                 // 日期有效、`r[1]` 越界
        ];
        for body in bad_main {
            let f = rwd(body.clone()).ok(URL_DIV_OAPI, "[]");
            let out = run_with(&f, &PreviousOutput::default());
            assert_eq!(f.requests().len(), 2, "{body} 應走備援");
            assert!(
                out.logs[0].starts_with("[除權息] 即時 API 失敗（"),
                "{body}"
            );
            assert!(out.errors.is_empty() && out.fresh, "{body}");
        }
    }

    #[test]
    fn rwd_shapes_that_python_treats_as_empty_success_do_not_fall_back() {
        // 物件逐鍵、字串逐字元：每個元素是單字元字串，日期解析不了＝全部略過。
        for body in [
            json!({"data": {"ab": 1, "c": 2}}),
            json!({"data": "abc"}),
            json!({"data": ["abc", "d"]}),
            json!({"data": [["bad"], ["115/13/40"]]}),
        ] {
            let f = rwd(body.clone());
            let out = run_with(&f, &old_dividend());
            assert!(out.value.is_empty(), "{body}");
            assert!(out.fresh && out.logs.is_empty(), "{body}");
            assert_eq!(f.requests(), [URL_DIV_RWD], "{body}");
        }
    }

    // ───── B-DIV-3：備援解析 ─────

    #[test]
    fn b_div_3_fallback_does_not_strip_exdividend_but_strips_code_and_name() {
        let out = run_with(
            &oapi(json!([
                {"Date":"1151009","Code":" 2317 ","Name":" 鴻海 ","Exdividend":" 息 ","CashDividend":" 2 "},
                {"Date":"1151010","Code":"1","Name":"n","Exdividend":"","CashDividend":null},
                {"Date":"1151011","Code":"2","Name":"n","Exdividend":null},
                {"Date":"1151012","Code":"3","Name":"n","Exdividend":0},
                {"Date":"1151013","Code":"4","Name":"n","Exdividend":"權"},
            ])),
            &PreviousOutput::default(),
        );
        assert_eq!(
            notes(&out),
            [
                "2026-10-09 2317 鴻海 除 息  現金2元", // Exdividend 前後空白原樣保留
                "2026-10-10 1 n 除權息",
                "2026-10-11 2 n 除權息",
                "2026-10-12 3 n 除權息",
                "2026-10-13 4 n 除權",
            ]
        );
    }

    #[test]
    fn b_div_3_fallback_missing_and_null_fields_follow_python_str() {
        let out = run_with(
            &oapi(json!([
                {"Date":"1151009","Exdividend":"息"},                    // Code/Name 缺 → 空
                {"Date":"1151010","Code":null,"Name":null,"Exdividend":"息"}, // null → "None"
                {"Date":"1151011","Code":2317,"Name":5,"Exdividend":"息"},   // 數字 → str
                {"Code":"9"},                                            // 沒有 Date → 略過
                {"Date":null,"Code":"9"},
                {"Date":"","Code":"9"},
                {"Date":"abc","Code":"9"},
            ])),
            &PreviousOutput::default(),
        );
        assert_eq!(
            notes(&out),
            [
                "2026-10-09   除息",
                "2026-10-10 None None 除息",
                "2026-10-11 2317 5 除息",
            ]
        );
    }

    #[test]
    fn b_div_3_fallback_shapes_that_python_treats_as_failure() {
        let bad = [
            json!(null),
            json!(5),
            json!(0),
            json!(true),
            json!({"a": 1}), // 物件逐鍵 → `str.get`
            json!("abc"),    // 字串逐字元 → `str.get`
            json!([5]),
            json!([null]),
            json!([[1]]),
            json!(["x"]),
            // 非字串的真值權息別：`"除" + 5`／`"除" + [..]` 是 TypeError。
            json!([{"Date":"1151009","Code":"1","Exdividend":5}]),
            json!([{"Date":"1151009","Code":"1","Exdividend":["a"]}]),
            json!([{"Date":"1151009","Code":"1","Exdividend":true}]),
            // 前面的列有效、後面的列壞：整個備援失敗（不保留部分結果）。
            json!([{"Date":"1151009","Code":"1","Exdividend":"息"}, 5]),
        ];
        for body in bad {
            let f = oapi(body.clone());
            let out = run_with(&f, &old_dividend());
            assert_eq!(out.errors.len(), 1, "{body}");
            assert!(out.errors[0].starts_with(ERR_PREFIX), "{body}");
            assert_eq!(out.value, previous(&old_dividend()), "{body}");
            assert!(!out.fresh, "{body}");
        }
    }

    #[test]
    fn b_div_3_fallback_shapes_that_python_treats_as_empty_success() {
        for body in [
            json!([]),
            json!({}),
            json!(""),
            json!([{"Code": "1"}]),
            json!([{"Date": "x"}]),
        ] {
            let f = oapi(body.clone());
            let out = run_with(&f, &old_dividend());
            assert!(
                out.value.is_empty() && out.errors.is_empty() && out.fresh,
                "{body}"
            );
        }
    }

    // ───── B-DIV-4／K-6：fmt_cash（期望值來自真的跑 Python `fmt_cash`） ─────

    #[test]
    fn b_div_4_fmt_cash_matches_python_table() {
        let cases: &[(Value, &str)] = &[
            (json!("0.5"), " 現金0.5元"),
            (json!("0.138"), " 現金0.138元"),
            (json!("100.0"), " 現金100元"),
            (json!("1.23456789"), " 現金1.23457元"),
            (json!("0.00001"), " 現金1e-05元"),
            (json!("1234567"), " 現金1.23457e+06元"),
            (json!("0"), ""),
            (json!("-0.0"), ""),
            (json!("-1"), ""),
            (json!(""), ""),
            (json!(" "), ""),
            (json!("abc"), ""),
            (json!("None"), ""),
            (json!("1,5"), ""),
            (json!(" 2 "), " 現金2元"),
            (json!("\u{1f}3\u{1f}"), " 現金3元"),
            (json!("1.5\u{a0}"), " 現金1.5元"),
            (json!("+1"), " 現金1元"),
            (json!(".5"), " 現金0.5元"),
            (json!("5."), " 現金5元"),
            (json!("1e3"), " 現金1000元"),
            (json!("1_0"), " 現金10元"),
            (json!("1__0"), ""),
            (json!("_1"), ""),
            (json!("1_"), ""),
            (json!("1_.5"), ""),
            (json!("1_000.5e1_0"), " 現金1.0005e+13元"),
            (json!("0x10"), ""),
            (json!("\u{ff11}\u{ff12}"), " 現金12元"), // 全形數字
            (json!("\u{ff11}\u{ff0e}\u{ff15}"), ""),  // 全形小數點不認得
            (json!("nan"), ""),
            (json!("-inf"), ""),
            // D8-6：Python 會輸出 ` 現金inf元`；這裡視為無法解析的金額（刻意偏離）。
            (json!("inf"), ""),
            (json!("+inf"), ""),
            (json!("Infinity"), ""),
            (json!("1e400"), ""),
            (json!("-1e400"), ""),
            (json!(null), ""),
            (json!(true), ""),
            (json!(0.5), " 現金0.5元"),
            (json!(5), " 現金5元"),
            (json!(100.0), " 現金100元"),
            (json!(1e-05), " 現金1e-05元"),
            (json!([1]), ""),
            (json!({"a": 1}), ""),
        ];
        for (v, want) in cases {
            assert_eq!(fmt_cash(v), *want, "輸入 {v}");
        }
    }

    // ───── D7：panic ─────

    #[test]
    fn d7_a_panic_inside_the_source_keeps_the_old_dividend_events() {
        let out = block_on(run_guarded(&PanicFetch, &clock(), &old_dividend()));
        assert_eq!(out.value, previous(&old_dividend()));
        assert_eq!(out.value.len(), 1);
        assert!(!out.fresh);
        assert_eq!(out.errors.len(), 1);
        assert!(out.errors[0].starts_with("除權息來源失敗：panic: "));
        assert_eq!(
            crate::fetch::errors::normalize_error(&out.errors[0]),
            ERR_PREFIX
        );
    }

    #[test]
    fn d7_run_guarded_is_transparent_when_nothing_panics() {
        let f = rwd(json!({"data": [row("115/10/08", "2330", "台積電", "息", "6")]}));
        let out = block_on(run_guarded(&f, &clock(), &PreviousOutput::default()));
        assert_eq!(notes(&out), ["2026-10-08 2330 台積電 除息 現金6元"]);
        assert!(out.fresh);
    }

    // ───── oracle ─────

    #[test]
    fn oracle_every_scenario_matches_python_expected_json() {
        oracle::for_each_scenario(|name, sc, expected| {
            let out = block_on(run_guarded(&sc.fetch, &sc.clock, &sc.previous()));
            // B-EVT：該來源輸出套窗口、去重、排序後，等於 expected.events 中 type=dividend 的子序列。
            let (start, end) = crate::fetch::events::window(sc.clock.tpe_date());
            let merged = crate::fetch::events::merge_events(out.value, start, end);
            let want = oracle::expected_events(expected, "dividend");
            assert_eq!(
                serde_json::to_value(&merged).unwrap(),
                Value::Array(want),
                "情境 {name}：dividend 事件不符"
            );
            oracle::assert_errors(name, &out.errors, expected, &[ERR_PREFIX]);
        });
    }
}
