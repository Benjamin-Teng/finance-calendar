//! 台股事件的合併、窗口、去重、排序（behavior-inventory B-EVT-1～5；`update_tw_events.py` 的
//! `main()` `:1141-1142`、`:1224-1239`），以及「舊輸出沿用某類事件」（B-FLOW-6）。
//!
//! 3.2（除權息、股東會）、3.3（法說會／財報）與 4.1（組裝）共用這一份，不再各寫一遍：
//!
//! - [`window`]：窗口 `[today - INCLUDE_PAST_DAYS, today + WINDOW_DAYS]`（閉區間，B-EVT-2）。
//! - [`merge_events`]：依序套窗口、以 `(type, code, date)` 去重（先到先留，B-EVT-3）、依
//!   `(date, type, code)` 穩定排序（B-EVT-4）。呼叫端要自己依 B-EVT-1 的來源序
//!   （除權息→股東會→法說會／財報）串好 `raw`，沿用的舊事件也在 `raw` 裡（會一起被窗口過濾）。
//! - [`previous_events`]：舊輸出 `events` 中指定類型的事件（沿用用，B-FLOW-6）。
//!
//! # 與 Python 的差異（刻意，D8-3）
//!
//! 舊輸出 `events` 的元素逐筆判形狀（缺欄位、型別不對、`type` 不認得＝丟棄該筆）；Python 對這種元素
//! 要嘛在 `e.get` 崩潰、要嘛原樣帶過去。窗口判斷用的日期字串解析不了（Python 的
//! `date.fromisoformat` 會崩潰）＝該筆略過。

use std::collections::HashSet;

use time::macros::format_description;
use time::{Date, Duration};

use super::output::{Event, EventType, PreviousOutput};

/// 窗口長度（天）：今天～今天＋14（B-EVT-2；`WINDOW_DAYS`）。
pub const WINDOW_DAYS: i64 = 14;
/// 往前保留的天數（`INCLUDE_PAST_DAYS`）。
pub const INCLUDE_PAST_DAYS: i64 = 0;

/// 窗口起訖（閉區間）：`today` 是台北今天（`Clock::tpe_date`，B-DATE-2）。
pub fn window(today: Date) -> (Date, Date) {
    (
        today - Duration::days(INCLUDE_PAST_DAYS),
        today + Duration::days(WINDOW_DAYS),
    )
}

/// 舊輸出 `events` 中屬於 `kinds` 的事件：依 `kinds` 的順序各自取出再串接（Python 的
/// `_old_events_of("conference") + _old_events_of("earnings")`，同類內保持原順序）。
pub fn previous_events(prev: &PreviousOutput, kinds: &[EventType]) -> Vec<Event> {
    let all = prev.get_items::<Event>("events").unwrap_or_default();
    kinds
        .iter()
        .flat_map(|k| all.iter().filter(move |e| e.kind == *k).cloned())
        .collect()
}

/// B-EVT-2～4：窗口過濾、去重、排序。`raw` 的順序就是「先到先留」的順序（B-EVT-1）。
pub fn merge_events(raw: Vec<Event>, start: Date, end: Date) -> Vec<Event> {
    let mut seen: HashSet<(EventType, String, String)> = HashSet::new();
    let mut events = Vec::new();
    for e in raw {
        let Some(d) = parse_iso_date(&e.date) else {
            continue;
        };
        if !(start <= d && d <= end) {
            continue;
        }
        if !seen.insert((e.kind, e.code.clone(), e.date.clone())) {
            continue;
        }
        events.push(e);
    }
    // 日期是字串比較（Python 同）；類型的衍生 Ord 宣告順序＝字母序（見 `EventType`）。穩定排序。
    events.sort_by(|a, b| (&a.date, a.kind, &a.code).cmp(&(&b.date, b.kind, &b.code)));
    events
}

/// 窗口判斷用的日期解析：只收 `YYYY-MM-DD` 與 `YYYYMMDD`（Python 3.11+ `date.fromisoformat` 也收週日期
/// `2026-W41-1`，但來源恆以 `isoformat()` 產生，不支援；序數日期 `2026-281` Python 本來就不收）。
fn parse_iso_date(s: &str) -> Option<Date> {
    Date::parse(s, format_description!("[year]-[month]-[day]"))
        .or_else(|_| Date::parse(s, format_description!("[year][month][day]")))
        .ok()
}

/// `counts` 用：某類型的事件數。
pub fn count_of(events: &[Event], kind: EventType) -> usize {
    events.iter().filter(|e| e.kind == kind).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use time::macros::date;

    fn ev(d: &str, kind: EventType, code: &str, note: &str) -> Event {
        Event {
            date: d.into(),
            kind,
            code: code.into(),
            name: format!("n{code}"),
            note: note.into(),
        }
    }

    #[test]
    fn b_evt_2_window_is_today_through_today_plus_14_inclusive() {
        let (s, e) = window(date!(2026 - 10 - 05));
        assert_eq!((s, e), (date!(2026 - 10 - 05), date!(2026 - 10 - 19)));
        let raw = vec![
            ev("2026-10-04", EventType::Dividend, "1", ""),
            ev("2026-10-05", EventType::Dividend, "2", ""),
            ev("2026-10-19", EventType::Dividend, "3", ""),
            ev("2026-10-20", EventType::Dividend, "4", ""),
        ];
        let got: Vec<_> = merge_events(raw, s, e)
            .into_iter()
            .map(|e| e.code)
            .collect();
        assert_eq!(got, ["2", "3"]);
    }

    #[test]
    fn b_evt_3_dedupe_key_is_type_code_date_first_one_wins() {
        let (s, e) = window(date!(2026 - 10 - 05));
        let raw = vec![
            ev("2026-10-08", EventType::Dividend, "2330", "先"),
            ev("2026-10-08", EventType::Dividend, "2330", "後"),
            // 類型、代號、日期任一不同都是不同的事件。
            ev("2026-10-08", EventType::Meeting, "2330", "股東會"),
            ev("2026-10-08", EventType::Dividend, "2317", "別檔"),
            ev("2026-10-09", EventType::Dividend, "2330", "別日"),
        ];
        let got = merge_events(raw, s, e);
        assert_eq!(got.len(), 4);
        let first = got
            .iter()
            .find(|x| x.kind == EventType::Dividend && x.code == "2330" && x.date == "2026-10-08");
        assert_eq!(first.map(|x| x.note.as_str()), Some("先"));
    }

    #[test]
    fn b_evt_4_sorts_by_date_then_type_alphabetically_then_code() {
        let (s, e) = window(date!(2026 - 10 - 05));
        let raw = vec![
            ev("2026-10-09", EventType::Dividend, "1", ""),
            ev("2026-10-08", EventType::Meeting, "9", ""),
            ev("2026-10-08", EventType::Earnings, "5", ""),
            ev("2026-10-08", EventType::Dividend, "7", ""),
            ev("2026-10-08", EventType::Dividend, "3", ""),
            ev("2026-10-08", EventType::Conference, "8", ""),
        ];
        let got: Vec<_> = merge_events(raw, s, e)
            .into_iter()
            .map(|e| (e.date, e.kind, e.code))
            .collect();
        let row = |d: &str, k: EventType, c: &str| (d.to_string(), k, c.to_string());
        assert_eq!(
            got,
            [
                row("2026-10-08", EventType::Conference, "8"),
                row("2026-10-08", EventType::Dividend, "3"),
                row("2026-10-08", EventType::Dividend, "7"),
                row("2026-10-08", EventType::Earnings, "5"),
                row("2026-10-08", EventType::Meeting, "9"),
                row("2026-10-09", EventType::Dividend, "1"),
            ]
        );
    }

    #[test]
    fn b_evt_4_code_order_is_string_order_not_numeric() {
        let (s, e) = window(date!(2026 - 10 - 05));
        let raw = vec![
            ev("2026-10-08", EventType::Dividend, "9", ""),
            ev("2026-10-08", EventType::Dividend, "10", ""),
            ev("2026-10-08", EventType::Dividend, "2330", ""),
        ];
        let got: Vec<_> = merge_events(raw, s, e)
            .into_iter()
            .map(|e| e.code)
            .collect();
        assert_eq!(got, ["10", "2330", "9"]);
    }

    #[test]
    fn unparsable_dates_are_skipped_instead_of_crashing() {
        let (s, e) = window(date!(2026 - 10 - 05));
        let raw = vec![
            ev("garbage", EventType::Dividend, "1", ""),
            ev("", EventType::Dividend, "2", ""),
            ev("2026-02-30", EventType::Dividend, "3", ""),
            ev("2026-10-08", EventType::Dividend, "4", ""),
            // 序數日期、週日期（m3）不在支援範圍：略過。
            ev("2026-281", EventType::Dividend, "5", ""),
            ev("2026-W41-4", EventType::Dividend, "6", ""),
            ev("2026-10-08x", EventType::Dividend, "7", ""),
            // 緊湊格式 `YYYYMMDD` 照 Python 收。
            ev("20261009", EventType::Dividend, "8", ""),
        ];
        let got = merge_events(raw, s, e);
        let codes: Vec<_> = got.iter().map(|e| e.code.as_str()).collect();
        assert_eq!(codes, ["4", "8"]);
    }

    #[test]
    fn previous_events_filters_by_type_and_concatenates_in_the_given_order() {
        let prev = PreviousOutput::from_value(json!({"events": [
            {"date":"2026-10-06","type":"earnings","code":"1","name":"a","note":"Q2 財報"},
            {"date":"2026-10-07","type":"dividend","code":"2","name":"b","note":"除息"},
            {"date":"2026-10-08","type":"conference","code":"3","name":"c","note":"法說會"},
            {"date":"2026-10-09","type":"earnings","code":"4","name":"d","note":"Q2 財報"},
            {"date":"2026-10-10","type":"meeting","code":"5","name":"e","note":"股東會"},
        ]}));
        let codes = |kinds: &[EventType]| -> Vec<String> {
            previous_events(&prev, kinds)
                .into_iter()
                .map(|e| e.code)
                .collect()
        };
        assert_eq!(codes(&[EventType::Dividend]), ["2"]);
        assert_eq!(codes(&[EventType::Meeting]), ["5"]);
        // 法說會＋財報：先 conference 後 earnings（Python 串接順序），同類內保持原序。
        assert_eq!(
            codes(&[EventType::Conference, EventType::Earnings]),
            ["3", "1", "4"]
        );
    }

    #[test]
    fn previous_events_without_a_usable_events_key_is_empty() {
        for v in [
            json!({}),
            json!({"events": null}),
            json!({"events": {"a": 1}}),
            json!({"events": "x"}),
            json!({"events": 5}),
        ] {
            let prev = PreviousOutput::from_value(v);
            assert!(previous_events(&prev, &[EventType::Dividend]).is_empty());
        }
        assert!(previous_events(&PreviousOutput::default(), &[EventType::Meeting]).is_empty());
    }

    #[test]
    fn previous_events_drops_only_the_malformed_elements() {
        let prev = PreviousOutput::from_value(json!({"events": [
            {"date":"2026-10-07","type":"dividend","code":"1","name":"a","note":"除息"},
            {"date":"2026-10-07","type":"dividend","code":5,"name":"a","note":"除息"},
            {"date":"2026-10-07","type":"nonsense","code":"3","name":"a","note":"x"},
            "oops",
            {"date":"2026-10-08","type":"dividend","code":"2","name":"b","note":"除權"},
        ]}));
        let codes: Vec<_> = previous_events(&prev, &[EventType::Dividend])
            .into_iter()
            .map(|e| e.code)
            .collect();
        assert_eq!(codes, ["1", "2"]);
    }

    #[test]
    fn count_of_counts_one_type() {
        let evs = vec![
            ev("2026-10-08", EventType::Dividend, "1", ""),
            ev("2026-10-08", EventType::Dividend, "2", ""),
            ev("2026-10-08", EventType::Meeting, "3", ""),
        ];
        assert_eq!(count_of(&evs, EventType::Dividend), 2);
        assert_eq!(count_of(&evs, EventType::Meeting), 1);
        assert_eq!(count_of(&evs, EventType::Earnings), 0);
    }
}
