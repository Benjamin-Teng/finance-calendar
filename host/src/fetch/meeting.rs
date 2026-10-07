//! 股東會（TWSE openapi `t187ap41_L`；behavior-inventory 4.3／B-MTG-1～3；原始碼 `update_tw_events.py` 的
//! `fetch_meeting`，`:436-455`，與 `main()` 的沿用段 `:1165-1169`）。
//!
//! 端點回物件陣列，欄位（中文鍵）：`開會日期`（`roc_to_date`）、`股東常(臨時)會`、`是否改選董監`、`公司代號`、
//! `公司名稱`（B-MTG-1）。`note`：`kind`（`str().strip()`）非空→`"股東" + kind`（`股東常會`、`股東臨時會`），空→
//! `股東會`；`是否改選董監`（`str().strip()`）恰為 `是` 再加 `・改選董監`（B-MTG-2）。日期解析失敗的列略過；
//! **本函式不套窗口**（B-EVT 在組裝時做）。來源失敗（連線、HTTP、壞 JSON、形狀不符）記
//! `股東會來源失敗：{e}`、沿用舊輸出的 meeting 事件（B-MTG-3、B-FLOW-6：meeting 一組）；**不保留部分結果**
//! （Python 的 `except` 在整個迴圈外，中途出錯整批丟棄）。
//!
//! # 形狀（Python 實測）
//!
//! 頂層：陣列＝各列；**物件＝逐鍵、字串＝逐字元**（元素是字串，不是物件就失敗；只有空物件、空字串會成功並回空
//! 清單）；`null`、數字、布林＝失敗（`TypeError`）。每列必須是物件（否則 `.get` 的 `AttributeError`＝整批失敗）；
//! 欄位缺＝空字串、`null`＝`"None"`（`str(None)`）、數字取 `str()`；`開會日期` 不是字串也可（`str()` 後解析）。
//!
//! 沒有刻意偏離 Python 的地方。已知近似：`str()` 對巢狀陣列／物件欄位用 JSON 文字而非 Python repr
//! （真實來源恆為字串）。

use super::clock::Clock;
use super::dates::iso_date;
use super::events::previous_events;
use super::http::{get_json, Fetch};
use super::json_util::{py_dict, py_iter, py_str_get, py_strip, roc_to_date_value};
use super::outcome::{guarded, SourceOutcome};
use super::output::{Event, EventType, PreviousOutput};

/// B-MTG-1：端點。
pub const URL_MEETING: &str = "https://openapi.twse.com.tw/v1/opendata/t187ap41_L";
/// B-FLOW-9：失敗前綴。
pub const ERR_PREFIX: &str = "股東會來源失敗：";

/// 舊輸出中 `type == "meeting"` 的事件（B-FLOW-6）。
pub fn previous(prev: &PreviousOutput) -> Vec<Event> {
    previous_events(prev, &[EventType::Meeting])
}

/// 由組裝端呼叫：[`run`] 加上 panic 隔離（D7）。
pub async fn run_guarded<F: Fetch>(
    fetch: &F,
    clock: &Clock,
    prev: &PreviousOutput,
) -> SourceOutcome<Vec<Event>> {
    guarded(ERR_PREFIX, previous(prev), run(fetch, clock, prev)).await
}

/// 抓股東會（B-MTG-1～3）。`fresh`＝抓到（`meeting_ev is not None`，B-FLOW-7，含空清單）。
async fn run<F: Fetch>(
    fetch: &F,
    _clock: &Clock,
    prev: &PreviousOutput,
) -> SourceOutcome<Vec<Event>> {
    match fetch_meeting(fetch).await {
        Ok(ev) => SourceOutcome::new(ev, true),
        Err(e) => {
            let mut out = SourceOutcome::new(previous(prev), false);
            out.errors.push(format!("{ERR_PREFIX}{e}"));
            out.logs.push("[股東會] 本輪失敗，沿用上次資料".to_string());
            out
        }
    }
}

async fn fetch_meeting<F: Fetch>(fetch: &F) -> Result<Vec<Event>, String> {
    let j = get_json(fetch, URL_MEETING)
        .await
        .map_err(|e| e.to_string())?;
    let mut ev = Vec::new();
    for r in py_iter(j)? {
        let obj = py_dict(&r)?;
        let Some(d) = roc_to_date_value(obj.get("開會日期")) else {
            continue;
        };
        let kind = py_strip(&py_str_get(obj, "股東常(臨時)會")).to_string();
        let mut note = if kind.is_empty() {
            "股東會".to_string()
        } else {
            format!("股東{kind}")
        };
        if py_strip(&py_str_get(obj, "是否改選董監")) == "是" {
            note.push_str("・改選董監");
        }
        ev.push(Event {
            date: iso_date(d),
            kind: EventType::Meeting,
            code: py_strip(&py_str_get(obj, "公司代號")).to_string(),
            name: py_strip(&py_str_get(obj, "公司名稱")).to_string(),
            note,
        });
    }
    Ok(ev)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::oracle;
    use crate::fetch::test_util::{block_on, MapFetch, PanicFetch};
    use serde_json::{json, Value};
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

    fn ok(body: Value) -> MapFetch {
        MapFetch::new().ok(URL_MEETING, &body.to_string())
    }

    fn rec(date: Value, kind: &str, elect: &str, code: &str, name: &str) -> Value {
        json!({
            "開會日期": date, "股東常(臨時)會": kind, "是否改選董監": elect,
            "公司代號": code, "公司名稱": name,
        })
    }

    fn old_meeting() -> PreviousOutput {
        PreviousOutput::from_value(json!({"events": [
            {"date":"2026-10-08","type":"meeting","code":"2330","name":"台積電","note":"股東常會"},
            {"date":"2026-10-07","type":"dividend","code":"1101","name":"台泥","note":"除息"},
        ]}))
    }

    #[test]
    fn b_mtg_1_url_and_prefix_are_verbatim() {
        assert_eq!(
            URL_MEETING,
            "https://openapi.twse.com.tw/v1/opendata/t187ap41_L"
        );
        assert_eq!(ERR_PREFIX, "股東會來源失敗：");
        let f = ok(json!([]));
        run_with(&f, &PreviousOutput::default());
        assert_eq!(f.requests(), [URL_MEETING]);
    }

    #[test]
    fn b_mtg_2_note_kind_and_election_suffix() {
        let out = run_with(
            &ok(json!([
                rec(json!("1151120"), "常會", "是", "2330", "台積電"),
                rec(json!("115/11/21"), "臨時會", "否", "2317", "鴻海"),
                rec(json!("115年11月22日"), "", "", "1101", "台泥"),
                rec(json!("1151123"), "", "是", "1102", "亞泥"),
                rec(json!("1151124"), " 常會 ", " 是 ", " 2412 ", " 中華電 "),
                rec(json!("1151125"), "常會", "是的", "3008", "大立光"),
            ])),
            &PreviousOutput::default(),
        );
        let got: Vec<_> = out
            .value
            .iter()
            .map(|e| {
                (
                    e.date.as_str(),
                    e.code.as_str(),
                    e.name.as_str(),
                    e.note.as_str(),
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                ("2026-11-20", "2330", "台積電", "股東常會・改選董監"),
                ("2026-11-21", "2317", "鴻海", "股東臨時會"),
                ("2026-11-22", "1101", "台泥", "股東會"),
                ("2026-11-23", "1102", "亞泥", "股東會・改選董監"),
                ("2026-11-24", "2412", "中華電", "股東常會・改選董監"),
                ("2026-11-25", "3008", "大立光", "股東常會"),
            ]
        );
        assert!(out.value.iter().all(|e| e.kind == EventType::Meeting));
        assert!(out.fresh && out.errors.is_empty() && out.logs.is_empty());
    }

    #[test]
    fn b_mtg_2_missing_null_and_non_string_fields_follow_python_str() {
        let out = run_with(
            &ok(json!([
                {"開會日期": "1151120"},
                {"開會日期": 1151121, "公司代號": 2330, "公司名稱": null, "股東常(臨時)會": null},
                {"開會日期": " 115/11/22 "},
            ])),
            &PreviousOutput::default(),
        );
        let got: Vec<_> = out
            .value
            .iter()
            .map(|e| {
                (
                    e.date.as_str(),
                    e.code.as_str(),
                    e.name.as_str(),
                    e.note.as_str(),
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                ("2026-11-20", "", "", "股東會"),
                // `str(None)` 是 "None"：kind 非空 → 「股東None」
                ("2026-11-21", "2330", "None", "股東None"),
                ("2026-11-22", "", "", "股東會"),
            ]
        );
    }

    #[test]
    fn b_mtg_3_rows_without_a_parsable_date_are_skipped() {
        let out = run_with(
            &ok(json!([
                {"公司代號": "1"},
                {"開會日期": null},
                {"開會日期": ""},
                {"開會日期": "abc"},
                {"開會日期": "20261120"},
                {"開會日期": "1151340"},
                {},
                rec(json!("1151120"), "常會", "否", "2", "ok"),
            ])),
            &PreviousOutput::default(),
        );
        assert_eq!(out.value.len(), 1);
        assert_eq!(out.value[0].code, "2");
    }

    #[test]
    fn b_mtg_3_empty_success_is_fresh_and_does_not_reuse_old_events() {
        for body in ["[]", "{}", r#""""#] {
            let out = run_with(&MapFetch::new().ok(URL_MEETING, body), &old_meeting());
            assert!(out.value.is_empty(), "{body}");
            assert!(out.fresh && out.errors.is_empty(), "{body}");
        }
    }

    #[test]
    fn b_mtg_3_failure_records_error_and_reuses_only_old_meeting_events() {
        for f in [
            MapFetch::new().status(URL_MEETING, 500),
            MapFetch::new().network_error(URL_MEETING, "boom"),
            MapFetch::new().ok(URL_MEETING, "{ not json"),
            MapFetch::new().ok(URL_MEETING, "\u{feff}[]"),
        ] {
            let out = run_with(&f, &old_meeting());
            assert_eq!(out.value, previous(&old_meeting()));
            assert_eq!(out.value.len(), 1, "只沿用 meeting、不含舊的 dividend");
            assert!(!out.fresh);
            assert_eq!(out.errors.len(), 1);
            assert!(out.errors[0].starts_with(ERR_PREFIX));
            assert_eq!(out.logs, ["[股東會] 本輪失敗，沿用上次資料"]);
        }
        let out = run_with(
            &MapFetch::new().status(URL_MEETING, 500),
            &PreviousOutput::default(),
        );
        assert!(out.value.is_empty() && !out.fresh);
        assert_eq!(
            out.errors,
            ["股東會來源失敗：HTTP Error 500: Internal Server Error"]
        );
    }

    #[test]
    fn shapes_that_python_treats_as_failure() {
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
            // 前面的列有效、後面的列壞：整批失敗，不保留部分結果。
            json!([rec(json!("1151120"), "常會", "是", "2330", "台積電"), 5]),
        ];
        for body in bad {
            let out = run_with(&ok(body.clone()), &old_meeting());
            assert_eq!(out.errors.len(), 1, "{body}");
            assert_eq!(out.value, previous(&old_meeting()), "{body}");
            assert!(!out.fresh, "{body}");
        }
    }

    #[test]
    fn d7_a_panic_inside_the_source_keeps_the_old_meeting_events() {
        let out = block_on(run_guarded(&PanicFetch, &clock(), &old_meeting()));
        assert_eq!(out.value, previous(&old_meeting()));
        assert_eq!(out.value.len(), 1);
        assert!(!out.fresh);
        assert!(out.errors[0].starts_with("股東會來源失敗：panic: "));
        assert_eq!(
            crate::fetch::errors::normalize_error(&out.errors[0]),
            ERR_PREFIX
        );
    }

    #[test]
    fn d7_run_guarded_is_transparent_when_nothing_panics() {
        let f = ok(json!([rec(
            json!("1151120"),
            "常會",
            "是",
            "2330",
            "台積電"
        )]));
        let out = block_on(run_guarded(&f, &clock(), &PreviousOutput::default()));
        assert_eq!(out.value.len(), 1);
        assert!(out.fresh);
    }

    #[test]
    fn oracle_every_scenario_matches_python_expected_json() {
        oracle::for_each_scenario(|name, sc, expected| {
            let out = block_on(run_guarded(&sc.fetch, &sc.clock, &sc.previous()));
            let (start, end) = crate::fetch::events::window(sc.clock.tpe_date());
            let merged = crate::fetch::events::merge_events(out.value, start, end);
            let want = oracle::expected_events(expected, "meeting");
            assert_eq!(
                serde_json::to_value(&merged).unwrap(),
                Value::Array(want),
                "情境 {name}：meeting 事件不符"
            );
            oracle::assert_errors(name, &out.errors, expected, &[ERR_PREFIX]);
        });
    }
}
