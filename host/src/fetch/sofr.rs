//! SOFR 3M（NY Fed 官方 API，behavior-inventory 4.9／B-SOFR-1～6、B-QT-10、C-2 第 13 檔；
//! 原始碼 `update_tw_events.py` 的 `fetch_sofr` 與 `main()` 的沿用段 `:1196-1202`）。
//!
//! # 來源語意
//!
//! 先抓 `last/2`；**回應成功但有效筆數不足 2 才**抓 `last/5`（B-SOFR-2）。`last/2` 本身失敗
//! （連線錯誤、HTTP 500）**不會**打備援——Python 的 `except` 在備援那一行之外，這是原始碼的實況
//! （情境 `sofr-main-500-no-fallback`），不是疏漏。備援本身失敗＝整個來源失敗。
//!
//! # 沿用與 fresh（B-SOFR-6）
//!
//! 失敗時記 `行情來源失敗（SOFR 3M／NY Fed）：{e}`，並沿用舊輸出 `quotes` 中名為 `SOFR 3M` 的那筆
//! （沒有就 `value=None`＝輸出的行情條沒有這一檔）；`fresh` 只在**本輪真的抓到**時為真（沿用的
//! 舊值不算）。`value` 即「要 append 在 `quotes` 最後」的那一筆（Python：`if sofr: quotes.append`）。
//!
//! # 與 Python 的差異（刻意）
//!
//! - **D8-6 的刻意改良**（數值欄位的極端型別，Python 不會崩潰但輸出會是非法 JSON 或不合理的值）：
//!   `average90day` 為布林（Python `float(True)` 得 1.0）、`"nan"`／`"inf"` 字串或由有限值算出
//!   `inf`（如 `1e308` 對 `-1e308`）、回應 JSON 含 `NaN`／`Infinity` 字面值（Python `json.loads`
//!   接受；這是共用的 `http::parse_json` 行為）一律視為該來源失敗，輸出永遠是合法 JSON。另外
//!   `prev=0`（Python 是 `ZeroDivisionError`）同樣失敗。
//! - `effectiveDate` 不是字串（Python 不會立刻崩潰）：與 Python 一樣照算筆數、照判斷要不要打
//!   `last/5`，錯誤延到排序／解析那一步才報（D8-2）。

use serde_json::Value;
use time::format_description::well_known::Iso8601;
use time::{Date, PrimitiveDateTime, Time};

use super::clock::Clock;
use super::http::{get_json, Fetch};
use super::json_util::{py_float, truthy};
use super::outcome::{guarded, SourceOutcome};
use super::output::{PreviousOutput, Quote, QuoteKind};

/// B-SOFR-1：主端點（最新＋前一營業日）。
pub const URL_SOFR: &str = "https://markets.newyorkfed.org/api/rates/secured/sofrai/last/2.json";
/// B-SOFR-1：備援端點（有效筆數不足時才用）。
pub const URL_SOFR_FALLBACK: &str =
    "https://markets.newyorkfed.org/api/rates/secured/sofrai/last/5.json";
/// 行情條上的名稱（C-2 第 13 檔）。
pub const SOFR_NAME: &str = "SOFR 3M";
/// B-FLOW-9：失敗前綴（含名稱與來源，逐字）。
pub const ERR_PREFIX: &str = "行情來源失敗（SOFR 3M／NY Fed）：";

/// 舊輸出 `quotes` 中的 SOFR 3M（逐筆判形狀；同名多筆取最後一筆，同 Python 的 dict 推導式）。
pub fn previous(prev: &PreviousOutput) -> Option<Quote> {
    prev.get_items::<Quote>("quotes")?
        .into_iter()
        .rev()
        .find(|q| q.name == SOFR_NAME)
}

/// 由組裝端呼叫：[`run`] 加上 panic 隔離（D7）。
pub async fn run_guarded<F: Fetch>(
    fetch: &F,
    clock: &Clock,
    prev: &PreviousOutput,
) -> SourceOutcome<Option<Quote>> {
    guarded(ERR_PREFIX, previous(prev), run(fetch, clock, prev)).await
}

/// 抓 SOFR、算漲跌，失敗時沿用舊值（B-SOFR-1～6）。`clock` 不使用（時間來自來源的 `effectiveDate`）。
async fn run<F: Fetch>(
    fetch: &F,
    _clock: &Clock,
    prev: &PreviousOutput,
) -> SourceOutcome<Option<Quote>> {
    match fetch_sofr(fetch).await {
        Ok(q) => SourceOutcome::new(Some(q), true),
        Err(e) => {
            let msg = format!("{ERR_PREFIX}{e}");
            let mut out = SourceOutcome::new(previous(prev), false);
            out.logs.push(format!("[行情] {msg}，跳過"));
            if out.value.is_some() {
                out.logs.push("[行情] SOFR 3M 沿用上次資料".to_string());
            }
            out.errors.push(msg);
            out
        }
    }
}

/// 一筆有效列：`effectiveDate`（真值、字串）與 `average90day`（非 null）。
struct Row {
    date: Value,
    avg: Value,
}

/// `(http_json(url) or {}).get("refRates") or []`，再套有效列過濾（B-SOFR-2）。
/// 任何形狀不符（頂層非物件、`refRates` 非陣列、元素非物件）＝失敗，與 Python 的
/// `AttributeError` 被 `except Exception` 接住一致；訊息只求可讀。
async fn valid_rows<F: Fetch>(fetch: &F, url: &str) -> Result<Vec<Row>, String> {
    let json = get_json(fetch, url).await.map_err(|e| e.to_string())?;
    let top = match &json {
        Value::Object(o) => o,
        other if !truthy(other) => return Ok(Vec::new()), // `or {}` → 沒有 refRates → []
        _ => return Err("回應不是 JSON 物件".to_string()),
    };
    let items = match top.get("refRates") {
        None => return Ok(Vec::new()),
        Some(Value::Array(a)) => a,
        Some(v) if !truthy(v) => return Ok(Vec::new()),
        Some(_) => return Err("refRates 不是陣列".to_string()),
    };
    let mut rows = Vec::new();
    for item in items {
        let Some(obj) = item.as_object() else {
            return Err("refRates 的元素不是物件".to_string());
        };
        let date = obj.get("effectiveDate");
        let avg = obj.get("average90day");
        // `r.get("effectiveDate") and r.get("average90day") is not None`
        let (Some(date), Some(avg)) = (date, avg) else {
            continue;
        };
        if !truthy(date) || avg.is_null() {
            continue;
        }
        // 非字串的日期先保留（Python 也照算筆數、照判斷要不要打 last/5），
        // 錯誤延到排序／解析那一步才報（見 `fetch_sofr`、D8-2）。
        rows.push(Row {
            date: date.clone(),
            avg: avg.clone(),
        });
    }
    Ok(rows)
}

async fn fetch_sofr<F: Fetch>(fetch: &F) -> Result<Quote, String> {
    let mut rows = valid_rows(fetch, URL_SOFR).await?;
    if rows.len() < 2 {
        rows = valid_rows(fetch, URL_SOFR_FALLBACK).await?;
    }
    // Python 先排序再數筆數；筆數不足時排序無從出錯，所以先數筆數結果相同。
    if rows.len() < 2 {
        return Err(format!("有效資料不足兩筆（{} 筆）", rows.len()));
    }
    // 兩筆以上、其中有非字串的 `effectiveDate`：Python 在 `sort`（字串與其他型別比較）或 `fromisoformat`
    // （收非字串）必然失敗，所以在這裡報錯與 Python 等價（只是訊息不同）。
    let mut dated = Vec::with_capacity(rows.len());
    for r in &rows {
        let Value::String(d) = &r.date else {
            return Err(format!("effectiveDate 不是字串：{}", r.date));
        };
        dated.push((d.as_str(), &r.avg));
    }
    // B-SOFR-3：依 effectiveDate 字串降冪；穩定排序（同日期保持來源順序，同 Python reverse=True）。
    dated.sort_by(|a, b| b.0.cmp(a.0));
    let (latest_date, latest_avg) = dated[0];
    let (_, prev_avg) = dated[1];
    let price = to_float(latest_avg)?;
    let prev_v = to_float(prev_avg)?;
    let date = Date::parse(latest_date, &Iso8601::DEFAULT)
        .map_err(|_| format!("Invalid isoformat string: '{latest_date}'"))?;
    // B-SOFR-4：該日 UTC 00:00 的 epoch 秒。
    let t = PrimitiveDateTime::new(date, Time::MIDNIGHT)
        .assume_utc()
        .unix_timestamp();
    if prev_v == 0.0 {
        return Err("float division by zero".to_string());
    }
    let chg_abs = price - prev_v;
    let chg_pct = (price / prev_v - 1.0) * 100.0;
    if !(chg_abs.is_finite() && chg_pct.is_finite()) {
        return Err("算出非有限數值".to_string());
    }
    Ok(Quote {
        name: SOFR_NAME.to_string(),
        kind: QuoteKind::Yield,
        price,
        prev: prev_v,
        chg_abs,
        chg_pct,
        t: Some(t),
    })
}

/// Python `float(x)`（只收數字與數字字串，字串語意見 [`py_float`]）；非有限值視為失敗（D8-6，見模組文件）。
fn to_float(v: &Value) -> Result<f64, String> {
    let f = match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => py_float(s),
        _ => None,
    };
    match f {
        Some(f) if f.is_finite() => Ok(f),
        _ => Err(format!("could not convert to float: {v}")),
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

    fn run_with(fetch: &MapFetch, prev: &PreviousOutput) -> SourceOutcome<Option<Quote>> {
        block_on(run(fetch, &clock(), prev))
    }

    fn rr(date: &str, avg: Value) -> Value {
        json!({"effectiveDate": date, "type": "SOFRAI", "average90day": avg})
    }

    fn body(rows: Vec<Value>) -> String {
        json!({"refRates": rows}).to_string()
    }

    fn old_with_sofr(price: f64) -> PreviousOutput {
        PreviousOutput::from_value(json!({"quotes": [
            {"name": "USD/TWD", "kind": "fx", "price": 31.5, "prev": 31.4,
             "chg_abs": 0.1, "chg_pct": 0.3, "t": null},
            {"name": "SOFR 3M", "kind": "yield", "price": price, "prev": 2.0,
             "chg_abs": 0.0, "chg_pct": 0.0, "t": 1790899200},
        ]}))
    }

    // ───── B-SOFR-1／2：端點與備援 ─────

    #[test]
    fn b_sofr_1_urls_are_verbatim() {
        assert_eq!(
            URL_SOFR,
            "https://markets.newyorkfed.org/api/rates/secured/sofrai/last/2.json"
        );
        assert_eq!(
            URL_SOFR_FALLBACK,
            "https://markets.newyorkfed.org/api/rates/secured/sofrai/last/5.json"
        );
        assert_eq!(ERR_PREFIX, "行情來源失敗（SOFR 3M／NY Fed）：");
    }

    #[test]
    fn b_sofr_1_two_valid_rows_never_touch_the_fallback() {
        let f = MapFetch::new().ok(
            URL_SOFR,
            &body(vec![
                rr("2026-10-02", json!(3.7)),
                rr("2026-10-01", json!(3.6)),
            ]),
        );
        let out = run_with(&f, &PreviousOutput::default());
        assert_eq!(f.requests(), [URL_SOFR]);
        assert!(out.fresh && out.errors.is_empty() && out.logs.is_empty());
        assert!(out.value.is_some());
    }

    #[test]
    fn b_sofr_2_too_few_valid_rows_fall_back_to_last_5() {
        let f = MapFetch::new()
            .ok(
                URL_SOFR,
                &body(vec![
                    rr("2026-10-02", json!(3.7)),
                    rr("2026-10-01", Value::Null),
                ]),
            )
            .ok(
                URL_SOFR_FALLBACK,
                &body(vec![
                    rr("2026-09-30", json!(3.6)),
                    rr("2026-10-02", json!(3.7)),
                    rr("2026-10-01", Value::Null),
                    json!({"type": "SOFRAI", "average90day": 9.9}),
                    rr("2026-09-29", json!(3.55)),
                ]),
            );
        let out = run_with(&f, &PreviousOutput::default());
        assert_eq!(f.requests(), [URL_SOFR, URL_SOFR_FALLBACK]);
        let q = out.value.expect("fresh");
        // 與 sofr-last5-fallback 情境（Python 實測）同值。
        assert_eq!((q.price, q.prev), (3.7, 3.6));
        assert_eq!(q.chg_abs, 0.10000000000000009);
        assert_eq!(q.chg_pct, 2.77777777777779);
        assert_eq!(q.t, Some(1790899200));
        assert!(out.fresh && out.errors.is_empty());
    }

    #[test]
    fn b_sofr_2_main_endpoint_failure_never_triggers_the_fallback() {
        for f in [
            MapFetch::new().status(URL_SOFR, 500).ok(
                URL_SOFR_FALLBACK,
                &body(vec![
                    rr("2026-10-02", json!(3.7)),
                    rr("2026-10-01", json!(3.6)),
                ]),
            ),
            MapFetch::new().network_error(URL_SOFR, "boom").ok(
                URL_SOFR_FALLBACK,
                &body(vec![
                    rr("2026-10-02", json!(3.7)),
                    rr("2026-10-01", json!(3.6)),
                ]),
            ),
            MapFetch::new().ok(URL_SOFR, "{ not json").ok(
                URL_SOFR_FALLBACK,
                &body(vec![
                    rr("2026-10-02", json!(3.7)),
                    rr("2026-10-01", json!(3.6)),
                ]),
            ),
        ] {
            let out = run_with(&f, &PreviousOutput::default());
            assert_eq!(f.requests(), [URL_SOFR], "備援不可被打");
            assert_eq!(out.errors.len(), 1);
            assert!(out.errors[0].starts_with(ERR_PREFIX));
            assert!(!out.fresh && out.value.is_none());
        }
    }

    #[test]
    fn b_sofr_2_valid_row_filter_matches_python_truthiness() {
        // effectiveDate 缺／空字串、average90day 缺／null 的列無效；average90day=0 是有效值（not None）。
        let f = MapFetch::new().ok(
            URL_SOFR,
            &body(vec![
                rr("", json!(1.0)),
                json!({"average90day": 2.0}),
                rr("2026-10-03", Value::Null),
                json!({"effectiveDate": "2026-10-04"}),
                rr("2026-10-02", json!(0.0)),
                rr("2026-10-01", json!(3.0)),
            ]),
        );
        let out = run_with(&f, &PreviousOutput::default());
        assert_eq!(
            f.requests(),
            [URL_SOFR],
            "有效 2 筆（0.0 算有效）就不打備援"
        );
        let q = out.value.unwrap();
        assert_eq!((q.price, q.prev), (0.0, 3.0));
        assert_eq!(q.chg_pct, -100.0);
    }

    #[test]
    fn b_sofr_2_still_short_after_the_fallback_fails_with_the_count() {
        let f = MapFetch::new()
            .ok(URL_SOFR, &body(vec![rr("2026-10-02", json!(3.7))]))
            .ok(URL_SOFR_FALLBACK, &body(vec![rr("2026-10-02", json!(3.7))]));
        let out = run_with(&f, &PreviousOutput::default());
        assert_eq!(
            out.errors,
            ["行情來源失敗（SOFR 3M／NY Fed）：有效資料不足兩筆（1 筆）"]
        );
        assert!(!out.fresh && out.value.is_none());
        // 備援本身失敗同樣是整個來源失敗。
        let f = MapFetch::new()
            .ok(URL_SOFR, &body(vec![]))
            .status(URL_SOFR_FALLBACK, 503);
        let out = run_with(&f, &PreviousOutput::default());
        assert_eq!(f.requests(), [URL_SOFR, URL_SOFR_FALLBACK]);
        assert!(out.errors[0].contains("HTTP Error 503"));
    }

    // ───── B-SOFR-3／4／5：排序與計算 ─────

    #[test]
    fn b_sofr_3_sorts_by_effective_date_descending_regardless_of_input_order() {
        let f = MapFetch::new().ok(
            URL_SOFR,
            &body(vec![
                rr("2026-09-29", json!(1.0)),
                rr("2026-10-02", json!(4.0)),
                rr("2026-10-01", json!(2.0)),
            ]),
        );
        let q = run_with(&f, &PreviousOutput::default()).value.unwrap();
        assert_eq!((q.price, q.prev), (4.0, 2.0));
    }

    #[test]
    fn b_sofr_4_5_output_shape_numbers_and_utc_midnight_timestamp() {
        // 數值字串也吃（Python float(str)）；t＝2026-10-02 UTC 00:00（Python 實測 1790899200）。
        let f = MapFetch::new().ok(
            URL_SOFR,
            &body(vec![
                rr("2026-10-02", json!(" 3.68732\n")),
                rr("2026-10-01", json!("3.68473")),
            ]),
        );
        let q = run_with(&f, &PreviousOutput::default()).value.unwrap();
        assert_eq!(q.name, "SOFR 3M");
        assert_eq!(q.kind, QuoteKind::Yield);
        assert_eq!((q.price, q.prev), (3.68732, 3.68473));
        assert_eq!(q.chg_abs, 3.68732 - 3.68473);
        assert_eq!(q.chg_pct, (3.68732 / 3.68473 - 1.0) * 100.0);
        assert_eq!(q.t, Some(1790899200));
        // 序列化後的鍵與順序（B-OUT-10）。
        let text = serde_json::to_string(&q).unwrap();
        assert!(text.starts_with(r#"{"name":"SOFR 3M","kind":"yield","price":3.68732,"prev":"#));
        assert!(text.ends_with(r#""t":1790899200}"#));
    }

    #[test]
    fn b_sofr_4_compact_and_week_date_forms_match_python_fromisoformat() {
        // date.fromisoformat 在 Python 3.11+ 也收 `20261002`。
        let f = MapFetch::new().ok(
            URL_SOFR,
            &body(vec![rr("20261002", json!(2.0)), rr("20261001", json!(1.0))]),
        );
        let q = run_with(&f, &PreviousOutput::default()).value.unwrap();
        assert_eq!(q.t, Some(1790899200));
    }

    #[test]
    fn m2_string_prices_use_python_float_semantics() {
        // 3.2 審查 m2：字串分支統一走 `py_float`（期望值來自真的跑 Python float()）。
        // `float("1_0")`＝10.0、`float("１２")`＝12.0 成功；`float("\x1f3")` 是 ValueError。
        let run_pair = |a: Value, b: Value| {
            let f = MapFetch::new().ok(
                URL_SOFR,
                &body(vec![rr("2026-10-02", a), rr("2026-10-01", b)]),
            );
            run_with(&f, &PreviousOutput::default())
        };
        let q = run_pair(json!("1_0"), json!("\u{ff12}")).value.unwrap();
        assert_eq!((q.price, q.prev), (10.0, 2.0));
        for bad in [
            "1__0",
            "_1",
            "\u{1f}3",
            "3\u{1f}",
            "\u{ff11}\u{ff0e}\u{ff15}",
        ] {
            let out = run_pair(json!(bad), json!("1.0"));
            assert_eq!(out.errors.len(), 1, "{bad:?}");
            assert!(out.value.is_none() && !out.fresh, "{bad:?}");
        }
        // Python 空白（含 NBSP、全形空白）可去除。
        let q = run_pair(json!("\u{a0}3.5\u{3000}"), json!(" 3.0 "))
            .value
            .unwrap();
        assert_eq!((q.price, q.prev), (3.5, 3.0));
    }

    // ───── B-SOFR-6：失敗、沿用、fresh ─────

    #[test]
    fn b_sofr_6_failure_reuses_old_sofr_logs_it_and_is_not_fresh() {
        let f = MapFetch::new().status(URL_SOFR, 500);
        let out = run_with(&f, &old_with_sofr(1.11));
        assert_eq!(
            out.errors,
            ["行情來源失敗（SOFR 3M／NY Fed）：HTTP Error 500: Internal Server Error"]
        );
        assert!(!out.fresh, "沿用舊值不算 fresh");
        assert_eq!(out.value.as_ref().map(|q| q.price), Some(1.11));
        assert_eq!(
            out.logs,
            [
                "[行情] 行情來源失敗（SOFR 3M／NY Fed）：HTTP Error 500: Internal Server Error，跳過",
                "[行情] SOFR 3M 沿用上次資料",
            ]
        );
    }

    #[test]
    fn b_sofr_6_failure_without_old_sofr_yields_none_and_only_the_skip_log() {
        let f = MapFetch::new().status(URL_SOFR, 500);
        let out = run_with(&f, &PreviousOutput::default());
        assert!(out.value.is_none() && !out.fresh);
        assert_eq!(out.logs.len(), 1);
        assert!(out.logs[0].ends_with("，跳過"));
    }

    #[test]
    fn b_sofr_6_old_quotes_are_checked_per_item_and_last_same_name_wins() {
        let prev = PreviousOutput::from_value(json!({"quotes": [
            "oops", 5,
            {"name": "SOFR 3M", "kind": "yield", "price": 1.0, "prev": 1.0, "chg_abs": 0.0, "chg_pct": 0.0, "t": null},
            {"name": "SOFR 3M", "kind": "yield", "price": 2.0, "prev": 1.0, "chg_abs": 1.0, "chg_pct": 100.0, "t": null},
            {"name": "SOFR 3M", "kind": "bogus"},
        ]}));
        assert_eq!(previous(&prev).map(|q| q.price), Some(2.0));
        // 整鍵型別不符＝不存在。
        for bad in [json!("bad"), json!({"a": 1}), json!(null), json!(7)] {
            assert!(previous(&PreviousOutput::from_value(json!({"quotes": bad}))).is_none());
        }
        assert!(previous(&PreviousOutput::default()).is_none());
        // 其他名稱不算。
        let prev = PreviousOutput::from_value(json!({"quotes": [
            {"name": "US 10Y", "kind": "yield", "price": 4.0, "prev": 4.0, "chg_abs": 0.0, "chg_pct": 0.0, "t": null}
        ]}));
        assert!(previous(&prev).is_none());
    }

    // ───── I2(a)：非字串的 effectiveDate 不提前失敗 ─────

    #[test]
    fn i2a_a_non_string_effective_date_still_counts_towards_the_row_count() {
        // Python：有效列照算（含日期是數字的那列），所以 last/2 有 2 列就不打 last/5；
        // 錯誤延到 sort（字串與數字比較 TypeError）才發生。
        let f = MapFetch::new()
            .ok(
                URL_SOFR,
                &body(vec![
                    rr("2026-10-02", json!(3.7)),
                    json!({"effectiveDate": 20261001, "average90day": 3.6}),
                ]),
            )
            .ok(
                URL_SOFR_FALLBACK,
                &body(vec![
                    rr("2026-10-02", json!(3.7)),
                    rr("2026-10-01", json!(3.6)),
                ]),
            );
        let out = run_with(&f, &PreviousOutput::default());
        assert_eq!(f.requests(), [URL_SOFR], "有 2 個有效列，不打備援");
        assert_eq!(out.errors.len(), 1);
        assert!(out.errors[0].starts_with(ERR_PREFIX));
        assert!(out.value.is_none() && !out.fresh);
    }

    #[test]
    fn i2a_a_lone_non_string_date_row_triggers_the_fallback_and_can_succeed() {
        // last/2 只有 1 個有效列（日期是數字）→ Python 照樣打 last/5，備援正常就成功。
        let f = MapFetch::new()
            .ok(
                URL_SOFR,
                &body(vec![
                    json!({"effectiveDate": 20261002, "average90day": 3.7}),
                ]),
            )
            .ok(
                URL_SOFR_FALLBACK,
                &body(vec![
                    rr("2026-10-02", json!(3.7)),
                    rr("2026-10-01", json!(3.6)),
                ]),
            );
        let out = run_with(&f, &PreviousOutput::default());
        assert_eq!(f.requests(), [URL_SOFR, URL_SOFR_FALLBACK]);
        assert!(out.errors.is_empty() && out.fresh);
        assert_eq!(out.value.map(|q| q.price), Some(3.7));
    }

    #[test]
    fn i2a_all_non_string_dates_with_two_rows_still_fail() {
        let b = body(vec![
            json!({"effectiveDate": 20261002, "average90day": 3.7}),
            json!({"effectiveDate": 20261001, "average90day": 3.6}),
        ]);
        let f = MapFetch::new().ok(URL_SOFR, &b);
        let out = run_with(&f, &PreviousOutput::default());
        assert_eq!(out.errors.len(), 1);
        assert!(out.value.is_none());
    }

    // ───── D8-6：刻意改良（數值欄位的極端型別視為失敗）─────

    #[test]
    fn d8_6_boolean_average90day_is_a_failure_not_1_or_0() {
        for v in [json!(true), json!(false)] {
            let f = MapFetch::new().ok(
                URL_SOFR,
                &body(vec![
                    rr("2026-10-02", v.clone()),
                    rr("2026-10-01", json!(3.6)),
                ]),
            );
            let out = run_with(&f, &PreviousOutput::default());
            assert_eq!(out.errors.len(), 1, "{v}");
            assert!(out.value.is_none() && !out.fresh, "{v}");
        }
    }

    #[test]
    fn d8_6_non_finite_values_are_failures() {
        // 字串 nan／inf（Python float() 接受）、以及由有限值算出 inf（1e308 − (−1e308)）。
        for (a, b) in [
            (json!("nan"), json!(1.0)),
            (json!("inf"), json!(1.0)),
            (json!(1.0), json!("-inf")),
            (json!(1e308), json!(-1e308)),
        ] {
            let f = MapFetch::new().ok(
                URL_SOFR,
                &body(vec![
                    rr("2026-10-02", a.clone()),
                    rr("2026-10-01", b.clone()),
                ]),
            );
            let out = run_with(&f, &PreviousOutput::default());
            assert_eq!(out.errors.len(), 1, "{a} {b}");
            assert!(out.value.is_none(), "{a} {b}");
        }
    }

    #[test]
    fn d8_6_json_nan_and_infinity_literals_fail_the_source() {
        // Python `json.loads` 接受 NaN／Infinity；共用的 parse_json 刻意不接受（輸出才不會有非法 JSON）。
        for lit in ["NaN", "Infinity", "-Infinity"] {
            let text = format!(
                r#"{{"refRates":[{{"effectiveDate":"2026-10-02","average90day":{lit}}},{{"effectiveDate":"2026-10-01","average90day":3.6}}]}}"#
            );
            let f = MapFetch::new().ok(URL_SOFR, &text);
            let out = run_with(&f, &PreviousOutput::default());
            assert_eq!(out.errors.len(), 1, "{lit}");
            assert!(out.errors[0].starts_with(ERR_PREFIX), "{lit}");
            assert!(out.value.is_none() && !out.fresh, "{lit}");
        }
    }

    #[test]
    fn malformed_payloads_fail_the_source_instead_of_panicking() {
        let bodies = [
            r#"[1,2]"#.to_string(),                  // 頂層非物件
            r#"{"refRates": {"a": 1}}"#.to_string(), // refRates 非陣列
            r#"{"refRates": "abc"}"#.to_string(),
            body(vec![json!("x"), json!(1)]), // 元素非物件
            body(vec![
                rr("2026-10-02", json!("abc")),
                rr("2026-10-01", json!(1.0)),
            ]), // 非數字
            body(vec![
                rr("2026-10-02", json!(true)),
                rr("2026-10-01", json!(1.0)),
            ]),
            body(vec![
                rr("not-a-date", json!(2.0)),
                rr("also-bad", json!(1.0)),
            ]),
            body(vec![
                json!({"effectiveDate": 20261002, "average90day": 1.0}),
                rr("2026-10-01", json!(1.0)),
            ]),
            body(vec![
                rr("2026-10-02", json!(2.0)),
                rr("2026-10-01", json!(0.0)),
            ]), // 除以 0
            body(vec![
                rr("2026-10-02", json!("nan")),
                rr("2026-10-01", json!(1.0)),
            ]),
            body(vec![
                rr("2026-10-02", json!("inf")),
                rr("2026-10-01", json!(1.0)),
            ]),
        ];
        for b in bodies {
            let f = MapFetch::new().ok(URL_SOFR, &b).ok(URL_SOFR_FALLBACK, &b);
            let out = run_with(&f, &PreviousOutput::default());
            assert_eq!(out.errors.len(), 1, "{b}");
            assert!(out.errors[0].starts_with(ERR_PREFIX), "{b}");
            assert!(!out.fresh && out.value.is_none(), "{b}");
        }
    }

    #[test]
    fn falsy_responses_count_as_no_rows_then_fail_with_the_count() {
        for b in [
            "null",
            "{}",
            r#"{"refRates": null}"#,
            r#"{"refRates": []}"#,
            "0",
        ] {
            let f = MapFetch::new().ok(URL_SOFR, b).ok(URL_SOFR_FALLBACK, b);
            let out = run_with(&f, &PreviousOutput::default());
            assert_eq!(f.requests(), [URL_SOFR, URL_SOFR_FALLBACK], "{b}");
            assert!(out.errors[0].ends_with("有效資料不足兩筆（0 筆）"), "{b}");
        }
    }

    // ───── D7：panic 隔離 ─────

    #[test]
    fn d7_a_panic_inside_the_source_keeps_the_old_sofr() {
        let out = block_on(run_guarded(&PanicFetch, &clock(), &old_with_sofr(1.11)));
        assert_eq!(out.value.map(|q| q.price), Some(1.11));
        assert!(!out.fresh);
        assert_eq!(out.errors.len(), 1);
        assert!(out.errors[0].starts_with("行情來源失敗（SOFR 3M／NY Fed）：panic: "));
        // 沒有舊值時為 None。
        let out = block_on(run_guarded(
            &PanicFetch,
            &clock(),
            &PreviousOutput::default(),
        ));
        assert!(out.value.is_none());
    }

    #[test]
    fn d7_run_guarded_is_transparent_when_nothing_panics() {
        let f = MapFetch::new().ok(
            URL_SOFR,
            &body(vec![
                rr("2026-10-02", json!(3.7)),
                rr("2026-10-01", json!(3.6)),
            ]),
        );
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
            // SOFR 永遠 append 在 quotes 最後（B-SOFR-6）；沒有就是 None。
            let want = expected["quotes"]
                .as_array()
                .and_then(|a| a.iter().find(|q| q["name"] == SOFR_NAME))
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(
                serde_json::to_value(&out.value).unwrap(),
                want,
                "情境 {name}：SOFR 3M 不符"
            );
            if !want.is_null() {
                let last = expected["quotes"].as_array().and_then(|a| a.last());
                assert_eq!(last, Some(&want), "情境 {name}：SOFR 必須在 quotes 最後");
            }
            oracle::assert_errors(name, &out.errors, expected, &[ERR_PREFIX]);
        });
    }
}
