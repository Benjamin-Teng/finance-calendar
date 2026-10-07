//! 總經日曆（ForexFactory 週曆，behavior-inventory 4.1／B-FF-1～11、C-6、K-14；
//! 原始碼 `update_tw_events.py` 的 `fetch_macro`、`zh_title`、`T`）。
//!
//! # 與 Python 的差異（都是刻意的）
//!
//! - **單列欄位型別異常只略過該列**（K-14／design D8-2）：Python 的 `(r.get("forecast") or "").strip()`
//!   對「非空且非字串」的值（數字、`true`、非空陣列／物件）丟 `AttributeError`，沒有任何 `try` 接住，
//!   整個 `main()` 崩潰、整輪不寫檔。這裡只略過該列（其餘列、其他來源不受影響）；`null`、`0`、
//!   `false`、`""`、`[]`、`{}` 這些「假值」與 Python 一樣視為空字串、保留該列。`title` 非字串
//!   （含 `null`，Python 同樣會在 `.replace` 崩潰）、整列不是物件，也是略過該列。被略過的列**不**進
//!   去重集合（等同它不存在，後面的重複列照常保留）。
//! - **回應不是陣列**：本週檔是「非空且非陣列」（例如 `{"a":1}`）時視為本週檔失敗（Python 會把物件的
//!   鍵當成列、之後崩潰）；下週檔同樣情形視為「尚未發布」；`null`、`{}`、`0`、`""` 與 Python 一樣
//!   當成空陣列。
//! - **日期與時間的分隔字元**：Python 3.11+ 的 `fromisoformat` 接受任何單一字元（空白、全形空白、
//!   `é`、emoji 都實測可解析），這裡同樣接受：把位置 10（或基本格式的位置 8）的整個字元換成 `T`
//!   再解析（`2026-09-28 08:30:00-04:00` 可解析，D8）。
//! - **日期超出 Python `datetime` 範圍**：Python 的 `astimezone` 先換成 UTC、再換成台北，UTC 或台北
//!   的年份落在 1..=9999 之外都丟 `OverflowError`、只略過該列；這裡同樣檢查兩者（UTC 不得早於
//!   `0001-01-01T00:00Z`、台北年份在 1..=9999），避免 `time` 的 `to_offset` 超出範圍時 panic。
//! - **沒有時區偏移的 `date`**（如 `2026-09-28T08:30:00`、`2026-09-28`）略過該列：Python 會把它當
//!   「系統本機時區」，結果隨執行環境而變；ForexFactory 實際一律帶偏移，不會發生。
//!
//! # 沿用規則（B-FF-2、B-FF-11）
//!
//! 只有**本週檔**失敗才算來源失敗：記 `總經來源失敗（本週檔）：{e}`，`value` 取舊輸出的 `macro`
//! （逐筆判形狀，壞元素丟棄，見 [`PreviousOutput::get_items`]；沒有舊值＝空），`fresh=false`。
//! 下週檔失敗（通常週末才發布、平日 404）只留日誌、不進 `errors`、不影響 `fresh`。本週檔成功
//! 就算 `fresh`（結果是空陣列也算）。

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use serde_json::{Map, Value};
use time::format_description::well_known::Iso8601;
use time::macros::format_description;
use time::OffsetDateTime;

use super::clock::{Clock, TPE_OFFSET};
use super::dates::is_py_space;
use super::errors::FetchError;
use super::http::{get_json, Fetch};
use super::json_util::truthy;
use super::outcome::{guarded, SourceOutcome};
use super::output::{MacroEvent, PreviousOutput};

/// B-FF-1：本週檔。
pub const URL_FF_THIS: &str = "https://nfs.faireconomy.media/ff_calendar_thisweek.json";
/// B-FF-1：下週檔（通常接近週末才發布）。
pub const URL_FF_NEXT: &str = "https://nfs.faireconomy.media/ff_calendar_nextweek.json";
/// B-FLOW-9：本週檔失敗的前綴。
pub const ERR_PREFIX: &str = "總經來源失敗（本週檔）：";

/// Python `datetime` 的最小值 `0001-01-01T00:00:00Z` 的 epoch 秒。
const MIN_PY_DATETIME_UTC: i64 = -62_135_596_800;

/// B-FF-3（C-1）：只收這三個幣別，區分大小寫。
const MACRO_CURRENCIES: [&str; 3] = ["USD", "EUR", "JPY"];
/// B-FF-3（C-1）：只收這兩種重要性，區分大小寫。
const MACRO_IMPACTS: [&str; 2] = ["High", "Medium"];

/// B-FF-8：幣別 → （國碼，旗標字）。Python 的 `CCY` 另含 CNY／GBP，因篩選而不會用到，不抄。
fn country_of(ccy: &str) -> (&'static str, &'static str) {
    match ccy {
        "USD" => ("US", "美"),
        "EUR" => ("EU", "歐"),
        _ => ("JP", "日"), // 只剩 JPY（呼叫端已用 MACRO_CURRENCIES 篩過）
    }
}

/// C-6：中譯字典 `T`，94 條，**逐字**抄自 `update_tw_events.py:169-256`（由 Python 實際的 `T.items()`
/// 機械產生、順序相同；條目數、逐條內容由 `tests::dictionary_*` 與 task 3.1 報告記載的一次性比對確認）。
pub const TITLES: [(&str, &str); 94] = [
    ("Non-Farm Employment Change", "非農就業人數"),
    ("ADP Non-Farm Employment Change", "ADP 非農就業"),
    ("Unemployment Rate", "失業率"),
    ("Average Hourly Earnings m/m", "平均時薪 (月增)"),
    ("Unemployment Claims", "初請失業金人數"),
    ("CPI m/m", "CPI (月增)"),
    ("CPI y/y", "CPI (年增)"),
    ("Core CPI m/m", "核心CPI (月增)"),
    ("PPI m/m", "PPI (月增)"),
    ("PPI y/y", "PPI (年增)"),
    ("Core PPI m/m", "核心PPI (月增)"),
    ("Core PCE Price Index m/m", "核心PCE物價 (月增)"),
    ("Retail Sales m/m", "零售銷售 (月增)"),
    ("Retail Sales y/y", "零售銷售 (年增)"),
    ("Core Retail Sales m/m", "核心零售銷售 (月增)"),
    ("ISM Manufacturing PMI", "ISM 製造業PMI"),
    ("ISM Services PMI", "ISM 非製造業PMI"),
    ("Flash Manufacturing PMI", "製造業PMI初值"),
    ("Flash Services PMI", "服務業PMI初值"),
    ("Final Manufacturing PMI", "製造業PMI終值"),
    ("Final Services PMI", "服務業PMI終值"),
    ("Prelim UoM Consumer Sentiment", "密大消費者信心初值"),
    ("Revised UoM Consumer Sentiment", "密大消費者信心終值"),
    ("CB Consumer Confidence", "諮商會消費者信心"),
    ("Federal Funds Rate", "Fed 利率決議"),
    ("FOMC Statement", "FOMC 聲明"),
    ("FOMC Press Conference", "FOMC 記者會"),
    ("FOMC Meeting Minutes", "FOMC 會議紀要"),
    ("FOMC Economic Projections", "FOMC 經濟預測"),
    ("Fed Chair Powell Speaks", "Fed 主席鮑爾談話"),
    ("Fed Chair Powell Testifies", "Fed 主席鮑爾聽證"),
    ("Fed Monetary Policy Report", "Fed 貨幣政策報告"),
    ("Empire State Manufacturing Index", "紐約州製造業指數"),
    ("Philly Fed Manufacturing Index", "費城聯儲製造業指數"),
    ("Industrial Production m/m", "工業生產 (月增)"),
    ("Housing Starts", "新屋開工"),
    ("Building Permits", "營建許可"),
    ("Existing Home Sales", "成屋銷售"),
    ("New Home Sales", "新屋銷售"),
    ("Pending Home Sales m/m", "成屋簽約銷售 (月增)"),
    ("Durable Goods Orders m/m", "耐久財訂單 (月增)"),
    ("Core Durable Goods Orders m/m", "核心耐久財訂單 (月增)"),
    ("Advance GDP q/q", "GDP 季增初值"),
    ("Prelim GDP q/q", "GDP 季增修正值"),
    ("Final GDP q/q", "GDP 季增終值"),
    ("JOLTS Job Openings", "JOLTS 職位空缺"),
    ("Trade Balance", "貿易帳"),
    ("Crude Oil Inventories", "EIA 原油庫存"),
    ("Natural Gas Storage", "EIA 天然氣庫存"),
    ("Personal Income m/m", "個人所得 (月增)"),
    ("Personal Spending m/m", "個人支出 (月增)"),
    ("Factory Orders m/m", "工廠訂單 (月增)"),
    ("Consumer Credit m/m", "消費信貸"),
    ("Main Refinancing Rate", "歐洲央行利率決議"),
    ("Monetary Policy Statement", "貨幣政策聲明"),
    ("ECB Press Conference", "歐洲央行記者會"),
    ("ECB Monetary Policy Meeting Accounts", "歐洲央行會議紀要"),
    ("ECB President Lagarde Speaks", "歐洲央行總裁拉加德談話"),
    ("German ZEW Economic Sentiment", "德國ZEW景氣指數"),
    ("ZEW Economic Sentiment", "歐元區ZEW景氣指數"),
    ("German ifo Business Climate", "德國ifo商業景氣"),
    ("German Prelim CPI m/m", "德國CPI初值 (月增)"),
    ("German Final CPI m/m", "德國CPI終值 (月增)"),
    ("French Final CPI m/m", "法國CPI終值 (月增)"),
    ("French Flash CPI m/m", "法國CPI初值 (月增)"),
    ("Spanish Flash CPI y/y", "西班牙CPI初值 (年增)"),
    ("CPI Flash Estimate y/y", "歐元區CPI初值 (年增)"),
    ("Core CPI Flash Estimate y/y", "歐元區核心CPI初值 (年增)"),
    ("Final CPI y/y", "CPI終值 (年增)"),
    ("Final Core CPI y/y", "核心CPI終值 (年增)"),
    ("German Factory Orders m/m", "德國工廠訂單 (月增)"),
    ("German Industrial Production m/m", "德國工業生產 (月增)"),
    ("German Trade Balance", "德國貿易帳"),
    ("Sentix Investor Confidence", "Sentix投資人信心"),
    ("German Flash Manufacturing PMI", "德國製造業PMI初值"),
    ("German Flash Services PMI", "德國服務業PMI初值"),
    ("BOJ Policy Rate", "日銀利率決議"),
    ("BOJ Outlook Report", "日銀展望報告"),
    ("BOJ Press Conference", "日銀記者會"),
    ("Monetary Policy Meeting Minutes", "日銀會議紀要"),
    ("BOJ Gov Ueda Speaks", "日銀總裁植田談話"),
    ("Tankan Manufacturing Index", "短觀製造業指數"),
    ("Tankan Non-Manufacturing Index", "短觀非製造業指數"),
    ("National Core CPI y/y", "全國核心CPI (年增)"),
    ("Tokyo Core CPI y/y", "東京核心CPI (年增)"),
    ("Household Spending y/y", "家庭支出 (年增)"),
    ("Average Cash Earnings y/y", "平均現金收入 (年增)"),
    ("Bank Lending y/y", "銀行放款 (年增)"),
    ("Current Account", "經常帳"),
    ("Economy Watchers Sentiment", "景氣觀察指數"),
    ("M2 Money Stock y/y", "M2貨幣供給 (年增)"),
    ("Prelim Machine Tool Orders y/y", "工具機訂單初值 (年增)"),
    ("Leading Indicators", "領先指標"),
    ("Bank Holiday", "休市"),
];

fn titles() -> &'static HashMap<&'static str, &'static str> {
    static MAP: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| TITLES.iter().copied().collect())
}

/// B-FF-9：`zh_title`——先查字典（精確、區分大小寫）；沒對到依序全域取代五個後綴；再沒有保留英文。
pub fn zh_title(t: &str) -> String {
    if let Some(zh) = titles().get(t) {
        return (*zh).to_string();
    }
    let mut s = t.to_string();
    for (a, b) in [
        (" m/m", " (月增)"),
        (" y/y", " (年增)"),
        (" q/q", " (季增)"),
        (" Speaks", " 談話"),
        (" Testifies", " 聽證"),
    ] {
        s = s.replace(a, b);
    }
    s
}

/// 舊輸出的 `macro`（逐筆判形狀；沒有＝空）。來源整個失敗時沿用它（B-FF-11）。
pub fn previous(prev: &PreviousOutput) -> Vec<MacroEvent> {
    prev.get_items::<MacroEvent>("macro").unwrap_or_default()
}

/// 由組裝端呼叫：[`run`] 加上 panic 隔離（D7）。
pub async fn run_guarded<F: Fetch>(
    fetch: &F,
    clock: &Clock,
    prev: &PreviousOutput,
) -> SourceOutcome<Vec<MacroEvent>> {
    guarded(ERR_PREFIX, previous(prev), run(fetch, clock, prev)).await
}

/// 抓本週＋下週、轉換、排序（B-FF-1～11）。`clock` 不使用（總經不套日期窗口，B-FF-3），
/// 為與其他來源同一形狀而保留。
async fn run<F: Fetch>(
    fetch: &F,
    _clock: &Clock,
    prev: &PreviousOutput,
) -> SourceOutcome<Vec<MacroEvent>> {
    let mut out = SourceOutcome::new(Vec::new(), false);
    let mut rows: Vec<Value> = Vec::new();
    let mut this_week_failed = false;
    // 兩檔依序抓（B-FF-1）；本週檔失敗後仍照 Python 的順序去抓下週檔（結果不會被用到）。
    for url in [URL_FF_THIS, URL_FF_NEXT] {
        match fetch_rows(fetch, url).await {
            Ok(mut r) => rows.append(&mut r),
            Err(e) if url == URL_FF_NEXT => {
                out.logs
                    .push(format!("[總經] 下週檔尚未發布（{e}），先只用本週"));
            }
            Err(e) => {
                out.errors.push(format!("{ERR_PREFIX}{e}"));
                this_week_failed = true;
            }
        }
    }
    if this_week_failed {
        out.logs.push("[總經] 本輪失敗，沿用上次資料".to_string());
        out.value = previous(prev);
        return out;
    }
    out.value = convert(&rows);
    out.fresh = true;
    out
}

/// `http_json(url) or []`：假值（`null`、`{}`、`0`、`""`、`false`）＝空；陣列＝各列；其餘＝失敗。
async fn fetch_rows<F: Fetch>(fetch: &F, url: &str) -> Result<Vec<Value>, FetchError> {
    match get_json(fetch, url).await? {
        Value::Array(rows) => Ok(rows),
        other if !truthy(&other) => Ok(Vec::new()),
        _ => Err(FetchError::Decode("回應不是 JSON 陣列".to_string())),
    }
}

/// B-FF-3～8：篩選、換算台北時間、去重、轉中文、依 `dt` 穩定排序。
fn convert(rows: &[Value]) -> Vec<MacroEvent> {
    let mut events = Vec::new();
    let mut seen: HashSet<(Option<&str>, &str)> = HashSet::new();
    for row in rows {
        let Some(obj) = row.as_object() else {
            continue; // 整列不是物件：略過（Python 在 `.get` 崩潰，K-14 精神）
        };
        // B-FF-3：缺鍵、非字串、不在清單都略過（Python：缺鍵得 ""，不在清單；非字串不可能相等）。
        let Some(ccy) = obj.get("country").and_then(Value::as_str) else {
            continue;
        };
        let Some(imp) = obj.get("impact").and_then(Value::as_str) else {
            continue;
        };
        if !MACRO_CURRENCIES.contains(&ccy) || !MACRO_IMPACTS.contains(&imp) {
            continue;
        }
        // B-FF-4：解析失敗（含缺鍵、非字串、沒有偏移）靜默略過。
        let Some(date_raw) = obj.get("date").and_then(Value::as_str) else {
            continue;
        };
        let Some(dt) = parse_ff_date(date_raw) else {
            continue;
        };
        // K-14：title 缺鍵＝""，非字串略過；forecast／previous 非空非字串略過。
        let title_raw = obj.get("title");
        let title = match title_raw {
            None => "",
            Some(Value::String(s)) => s.as_str(),
            Some(_) => continue,
        };
        let (Some(forecast), Some(previous)) =
            (text_field(obj, "forecast"), text_field(obj, "previous"))
        else {
            continue;
        };
        // B-FF-6：以「原始」(title, date) 去重，先到先留（缺 title 與 title="" 是不同的鍵）。
        if !seen.insert((title_raw.and_then(Value::as_str), date_raw)) {
            continue;
        }
        let (code, flag) = country_of(ccy);
        // Python：`astimezone` 換算後年份超出 1..=9999 會丟 `OverflowError`，被 `except Exception` 接住、
        // 略過該列。`to_offset` 在超出 `time` 支援範圍時是 panic，所以用 `checked_to_offset` 並自行
        // 限縮年份（`time` 的年份下限是 -9999，Python 是 1）。
        // Python 的 `astimezone` 先換成 UTC 再換成台北：UTC 早於 0001-01-01T00:00Z（-62135596800 秒）
        // 也是 OverflowError，即使台北時間看起來還在第 1 年（N1）。UTC 上界不必查（台北 ≥ UTC）。
        if dt.unix_timestamp() < MIN_PY_DATETIME_UTC {
            continue;
        }
        let Some(tpe) = dt
            .checked_to_offset(TPE_OFFSET)
            .filter(|t| (1..=9999).contains(&t.year()))
        else {
            continue;
        };
        events.push(MacroEvent {
            dt: fmt(
                tpe,
                format_description!("[year]-[month]-[day]T[hour]:[minute]"),
            ),
            date: fmt(tpe, format_description!("[year]-[month]-[day]")),
            time: fmt(tpe, format_description!("[hour]:[minute]")),
            ts: Some(tpe.unix_timestamp()),
            country: code.to_string(),
            flag: flag.to_string(),
            impact: if imp == "High" { "high" } else { "medium" }.to_string(),
            title: zh_title(title),
            title_en: title.to_string(),
            forecast,
            previous,
        });
    }
    // B-FF-7：依 dt 字串升冪；`sort_by` 是穩定排序（同 dt 保持來源順序，本週先於下週）。
    events.sort_by(|a, b| a.dt.cmp(&b.dt));
    events
}

fn fmt(t: OffsetDateTime, f: &[time::format_description::BorrowedFormatItem<'_>]) -> String {
    t.format(f).unwrap_or_default()
}

/// Python `datetime.fromisoformat(s)` 在 FF 資料上的子集：帶偏移的 ISO 8601（`-04:00`、`Z`、
/// `+0900`、秒的小數都可）。日期與時間的分隔字元：Python 3.11+ 接受任何單一字元（含空白與非 ASCII），
/// 這裡把該字元整個換成 `T` 再解析（D8）。沒有偏移回 `None`（見模組文件）。
fn parse_ff_date(s: &str) -> Option<OffsetDateTime> {
    let bytes = s.as_bytes();
    // 日期部分：`YYYY-MM-DD`（分隔字元在第 10 個位元組）或 `YYYYMMDD`（第 8 個）。
    let sep = if bytes.len() > 10 && bytes[4] == b'-' {
        Some(10)
    } else if bytes.len() > 8 && bytes[..8].iter().all(u8::is_ascii_digit) {
        Some(8)
    } else {
        None
    };
    if let Some(i) = sep {
        // 位置 i 可能落在多位元組字元中間（不是字元邊界）：那就不是合法分隔位置，照原字串解析（會失敗）。
        if s.is_char_boundary(i) {
            if let Some(ch) = s[i..].chars().next() {
                if ch != 'T' {
                    let mut normalized = s.to_string();
                    normalized.replace_range(i..i + ch.len_utf8(), "T");
                    return OffsetDateTime::parse(&normalized, &Iso8601::DEFAULT).ok();
                }
            }
        }
    }
    OffsetDateTime::parse(s, &Iso8601::DEFAULT).ok()
}

/// `(r.get(key) or "").strip()`：缺鍵／假值＝`""`；字串＝去頭尾空白（Python `str.strip`）；
/// 非空且非字串＝`None`（呼叫端略過該列，K-14）。
fn text_field(obj: &Map<String, Value>, key: &str) -> Option<String> {
    match obj.get(key) {
        None => Some(String::new()),
        Some(Value::String(s)) => Some(s.trim_matches(is_py_space).to_string()),
        Some(v) if !truthy(v) => Some(String::new()),
        Some(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::fixture::Scenario;
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

    fn run_with(fetch: &MapFetch, prev: &PreviousOutput) -> SourceOutcome<Vec<MacroEvent>> {
        block_on(run(fetch, &clock(), prev))
    }

    fn row(title: &str, date: &str) -> Value {
        json!({"title": title, "country": "USD", "date": date, "impact": "High",
               "forecast": "1%", "previous": "2%"})
    }

    fn weeks(this: Value, next: Value) -> MapFetch {
        MapFetch::new()
            .ok(URL_FF_THIS, &this.to_string())
            .ok(URL_FF_NEXT, &next.to_string())
    }

    fn old_macro_event(title: &str) -> Value {
        json!({"dt": "2026-09-28T21:30", "date": "2026-09-28", "time": "21:30", "ts": 1790602200,
               "country": "EU", "flag": "歐", "impact": "medium", "title": title,
               "title_en": title, "forecast": "", "previous": ""})
    }

    // ───── B-FF-1：端點與請求順序 ─────

    #[test]
    fn b_ff_1_urls_are_verbatim_and_fetched_this_week_first() {
        assert_eq!(
            URL_FF_THIS,
            "https://nfs.faireconomy.media/ff_calendar_thisweek.json"
        );
        assert_eq!(
            URL_FF_NEXT,
            "https://nfs.faireconomy.media/ff_calendar_nextweek.json"
        );
        let f = weeks(json!([]), json!([]));
        let out = run_with(&f, &PreviousOutput::default());
        assert_eq!(f.requests(), [URL_FF_THIS, URL_FF_NEXT]);
        assert!(out.value.is_empty() && out.fresh && out.errors.is_empty());
    }

    // ───── B-FF-2：失敗語意 ─────

    #[test]
    fn b_ff_2_this_week_failure_fails_the_source_and_keeps_old_macro() {
        let prev = PreviousOutput::from_value(json!({"macro": [old_macro_event("舊")]}));
        let f = MapFetch::new().status(URL_FF_THIS, 500).ok(
            URL_FF_NEXT,
            &json!([row("CPI m/m", "2026-10-06T08:30:00-04:00")]).to_string(),
        );
        let out = run_with(&f, &prev);
        assert_eq!(
            out.errors,
            ["總經來源失敗（本週檔）：HTTP Error 500: Internal Server Error"]
        );
        assert!(!out.fresh);
        assert_eq!(out.value.len(), 1);
        assert_eq!(out.value[0].title, "舊", "沿用舊值，不混入下週檔");
        assert_eq!(out.logs, ["[總經] 本輪失敗，沿用上次資料"]);
        // Python 的迴圈仍會去抓下週檔。
        assert_eq!(f.requests(), [URL_FF_THIS, URL_FF_NEXT]);
    }

    #[test]
    fn b_ff_2_this_week_network_error_and_bad_json_are_failures_too() {
        for f in [
            MapFetch::new()
                .network_error(URL_FF_THIS, "<urlopen error timed out>")
                .ok(URL_FF_NEXT, "[]"),
            MapFetch::new()
                .ok(URL_FF_THIS, "{ not json")
                .ok(URL_FF_NEXT, "[]"),
            MapFetch::new()
                .ok(URL_FF_THIS, "\u{feff}[]") // B-HTTP-5：BOM 讓 json.loads 失敗
                .ok(URL_FF_NEXT, "[]"),
            MapFetch::new()
                .ok(URL_FF_THIS, r#"{"a":1}"#) // 非空非陣列
                .ok(URL_FF_NEXT, "[]"),
        ] {
            let out = run_with(&f, &PreviousOutput::default());
            assert_eq!(out.errors.len(), 1, "{:?}", out.errors);
            assert!(out.errors[0].starts_with(ERR_PREFIX));
            assert!(!out.fresh && out.value.is_empty());
        }
    }

    #[test]
    fn b_ff_2_next_week_failure_is_log_only_and_not_an_error() {
        for next in [
            MapFetch::new()
                .ok(
                    URL_FF_THIS,
                    &json!([row("CPI m/m", "2026-10-06T08:30:00-04:00")]).to_string(),
                )
                .status(URL_FF_NEXT, 404),
            MapFetch::new()
                .ok(
                    URL_FF_THIS,
                    &json!([row("CPI m/m", "2026-10-06T08:30:00-04:00")]).to_string(),
                )
                .network_error(URL_FF_NEXT, "boom"),
            MapFetch::new()
                .ok(
                    URL_FF_THIS,
                    &json!([row("CPI m/m", "2026-10-06T08:30:00-04:00")]).to_string(),
                )
                .ok(URL_FF_NEXT, r#"{"a":1}"#),
        ] {
            let out = run_with(&next, &PreviousOutput::default());
            assert!(out.errors.is_empty(), "{:?}", out.errors);
            assert!(out.fresh);
            assert_eq!(out.value.len(), 1);
            assert_eq!(out.logs.len(), 1);
            assert!(out.logs[0].starts_with("[總經] 下週檔尚未發布（"));
            assert!(out.logs[0].ends_with("），先只用本週"));
        }
        // 404 的內文照 B-FLOW-18：`下週檔尚未發布（HTTP Error 404: Not Found），先只用本週`。
        let f = MapFetch::new()
            .ok(URL_FF_THIS, "[]")
            .status(URL_FF_NEXT, 404);
        assert_eq!(
            run_with(&f, &PreviousOutput::default()).logs,
            ["[總經] 下週檔尚未發布（HTTP Error 404: Not Found），先只用本週"]
        );
    }

    #[test]
    fn falsy_top_level_json_counts_as_empty_like_python_or_list() {
        for body in ["null", "{}", "0", r#""""#, "false", "[]"] {
            let f = MapFetch::new().ok(URL_FF_THIS, body).ok(URL_FF_NEXT, body);
            let out = run_with(&f, &PreviousOutput::default());
            assert!(
                out.errors.is_empty() && out.fresh && out.value.is_empty(),
                "{body}"
            );
            assert!(out.logs.is_empty(), "{body}");
        }
    }

    // ───── B-FF-3：篩選 ─────

    #[test]
    fn b_ff_3_filters_currency_and_impact_case_sensitively_without_a_date_window() {
        let mk = |country: &str, impact: &str, title: &str| {
            json!({"title": title, "country": country, "date": "2020-01-02T08:30:00-05:00",
                   "impact": impact, "forecast": "", "previous": ""})
        };
        let f = weeks(
            json!([
                mk("USD", "High", "a"),
                mk("EUR", "Medium", "b"),
                mk("JPY", "High", "c"),
                mk("CNY", "High", "dropped-ccy"),
                mk("GBP", "Medium", "dropped-ccy2"),
                mk("USD", "Low", "dropped-low"),
                mk("USD", "Holiday", "dropped-holiday"),
                mk("usd", "High", "dropped-case-ccy"),
                mk("USD", "high", "dropped-case-imp"),
                json!({"title": "no-country", "date": "2026-10-06T08:30:00-04:00", "impact": "High"}),
                json!({"title": "null-impact", "country": "USD", "impact": null, "date": "2026-10-06T08:30:00-04:00"}),
            ]),
            json!([]),
        );
        let out = run_with(&f, &PreviousOutput::default());
        let titles: Vec<_> = out.value.iter().map(|e| e.title_en.as_str()).collect();
        assert_eq!(titles, ["a", "b", "c"], "2020 年的列也留（不套 14 天窗口）");
        assert_eq!(out.value[0].impact, "high");
        assert_eq!(out.value[1].impact, "medium");
    }

    // ───── B-FF-4／B-FF-5：日期與時間欄 ─────

    #[test]
    fn b_ff_4_5_converts_offsets_to_taipei_with_python_verified_values() {
        // 期望值來自真的跑 Python：datetime.fromisoformat(s).astimezone(+8) 的
        // strftime("%Y-%m-%dT%H:%M")／int(timestamp())。
        let cases = [
            (
                "2026-09-28T08:30:00-04:00",
                "2026-09-28T20:30",
                1790598600_i64,
            ),
            ("2026-09-28T23:45:00Z", "2026-09-29T07:45", 1790639100),
            ("2026-12-31T20:00:00+09:00", "2026-12-31T19:00", 1798714800),
            (
                "2026-09-28T08:30:00.500-04:00",
                "2026-09-28T20:30",
                1790598600,
            ),
            ("2026-09-28T08:30:00-0400", "2026-09-28T20:30", 1790598600),
        ];
        for (i, (raw, dt, ts)) in cases.iter().enumerate() {
            let f = weeks(json!([row(&format!("t{i}"), raw)]), json!([]));
            let out = run_with(&f, &PreviousOutput::default());
            assert_eq!(out.value.len(), 1, "{raw}");
            let e = &out.value[0];
            assert_eq!(e.dt, *dt, "{raw}");
            assert_eq!(e.date, dt[..10], "{raw}");
            assert_eq!(e.time, dt[11..], "{raw}");
            assert_eq!(e.ts, Some(*ts), "{raw}");
        }
    }

    #[test]
    fn b_ff_4_unparsable_missing_non_string_and_offsetless_dates_skip_only_that_row() {
        let f = weeks(
            json!([
                row("bad", "not a date"),
                row("naive", "2026-09-28T08:30:00"),
                row("date-only", "2026-09-28"),
                json!({"title": "nodate", "country": "USD", "impact": "High"}),
                json!({"title": "numdate", "country": "USD", "impact": "High", "date": 20260928}),
                row("ok", "2026-09-28T08:30:00-04:00"),
            ]),
            json!([]),
        );
        let out = run_with(&f, &PreviousOutput::default());
        assert_eq!(out.value.len(), 1);
        assert_eq!(out.value[0].title_en, "ok");
        assert!(out.errors.is_empty() && out.fresh);
    }

    // ───── B-FF-6：去重 ─────

    #[test]
    fn b_ff_6_dedupes_on_raw_title_and_date_first_one_wins() {
        let mut dup = row("CPI m/m", "2026-10-06T08:30:00-04:00");
        dup["forecast"] = json!("SECOND");
        // 同樣的時刻、不同的原始字串（偏移寫法不同）不算重複（以「原始」字串比對）。
        let same_instant = row("CPI m/m", "2026-10-06T12:30:00Z");
        let other_day = row("CPI m/m", "2026-10-07T08:30:00-04:00");
        let f = weeks(
            json!([
                row("CPI m/m", "2026-10-06T08:30:00-04:00"),
                dup,
                same_instant
            ]),
            json!([row("CPI m/m", "2026-10-06T08:30:00-04:00"), other_day]),
        );
        let out = run_with(&f, &PreviousOutput::default());
        assert_eq!(out.value.len(), 3);
        assert_eq!(out.value[0].forecast, "1%", "先到先留");
        assert!(out.value.iter().all(|e| e.forecast != "SECOND"));
    }

    #[test]
    fn b_ff_6_missing_title_and_empty_title_are_different_keys() {
        let missing =
            json!({"country": "USD", "impact": "High", "date": "2026-10-06T08:30:00-04:00"});
        let empty = json!({"title": "", "country": "USD", "impact": "High", "date": "2026-10-06T08:30:00-04:00"});
        let f = weeks(json!([missing, empty]), json!([]));
        let out = run_with(&f, &PreviousOutput::default());
        assert_eq!(out.value.len(), 2);
        assert!(out
            .value
            .iter()
            .all(|e| e.title.is_empty() && e.title_en.is_empty()));
    }

    // ───── B-FF-7：排序 ─────

    #[test]
    fn b_ff_7_sorts_by_dt_and_is_stable_with_this_week_before_next_week() {
        let f = weeks(
            json!([
                row("b-late", "2026-10-07T08:30:00-04:00"),
                row("a-same-1", "2026-10-06T08:30:00-04:00"),
                row("a-same-2", "2026-10-06T12:30:00Z"), // 台北同一分鐘，不同原始字串
            ]),
            json!([
                row("n-same", "2026-10-06T08:30:00-04:00"), // 與 a-same-1 同 dt 且同標題以外
                row("n-early", "2026-10-05T08:30:00-04:00"),
            ]),
        );
        let out = run_with(&f, &PreviousOutput::default());
        let order: Vec<_> = out.value.iter().map(|e| e.title_en.as_str()).collect();
        assert_eq!(
            order,
            ["n-early", "a-same-1", "a-same-2", "n-same", "b-late"]
        );
    }

    // ───── B-FF-8：欄位對照 ─────

    #[test]
    fn b_ff_8_field_mapping_and_python_strip_of_forecast_and_previous() {
        let mk = |country: &str, impact: &str, forecast: Value, previous: Value| {
            // 日期依幣別不同，免得被 B-FF-6 的 (title, date) 去重吃掉。
            let day = match country {
                "USD" => "06",
                "EUR" => "07",
                _ => "08",
            };
            json!({"title": "Unemployment Rate", "country": country,
                   "date": format!("2026-10-{day}T08:30:00-04:00"), "impact": impact,
                   "forecast": forecast, "previous": previous})
        };
        // Python `str.strip()` 會去掉全形空白與 \x1c–\x1f（Rust 預設 trim 不含後者）。
        let f = weeks(
            json!([
                mk(
                    "USD",
                    "High",
                    json!("\u{3000}4.2%\u{1c} \t"),
                    json!(" 4.1% ")
                ),
                mk("EUR", "Medium", json!(""), json!(null)),
            ]),
            json!([mk("JPY", "High", json!(0), json!(false))]),
        );
        let out = run_with(&f, &PreviousOutput::default());
        assert_eq!(out.value.len(), 3);
        let by = |c: &str| out.value.iter().find(|e| e.country == c).unwrap();
        let us = by("US");
        assert_eq!((us.flag.as_str(), us.impact.as_str()), ("美", "high"));
        assert_eq!(
            (us.title.as_str(), us.title_en.as_str()),
            ("失業率", "Unemployment Rate")
        );
        assert_eq!(
            (us.forecast.as_str(), us.previous.as_str()),
            ("4.2%", "4.1%")
        );
        let eu = by("EU");
        assert_eq!((eu.flag.as_str(), eu.impact.as_str()), ("歐", "medium"));
        assert_eq!((eu.forecast.as_str(), eu.previous.as_str()), ("", ""));
        let jp = by("JP");
        assert_eq!(jp.flag, "日");
        assert_eq!((jp.forecast.as_str(), jp.previous.as_str()), ("", ""));
    }

    #[test]
    fn b_ff_8_missing_forecast_and_previous_keys_are_empty_strings() {
        let r = json!({"title": "X", "country": "USD", "impact": "High", "date": "2026-10-06T08:30:00-04:00"});
        let out = run_with(&weeks(json!([r]), json!([])), &PreviousOutput::default());
        assert_eq!(out.value.len(), 1);
        assert_eq!(
            (
                out.value[0].forecast.as_str(),
                out.value[0].previous.as_str()
            ),
            ("", "")
        );
    }

    // ───── B-FF-9／C-6：中譯 ─────

    #[test]
    fn c_6_dictionary_has_94_unique_entries_and_every_one_resolves() {
        assert_eq!(TITLES.len(), 94);
        assert_eq!(titles().len(), 94, "鍵有重複會讓 HashMap 變少");
        for (en, zh) in TITLES {
            assert_eq!(zh_title(en), zh, "{en}");
        }
        // 抽樣逐字（含中點、全形括號、含空格與數字的條目）。
        assert_eq!(zh_title("Non-Farm Employment Change"), "非農就業人數");
        assert_eq!(zh_title("CPI m/m"), "CPI (月增)");
        assert_eq!(zh_title("Core PCE Price Index m/m"), "核心PCE物價 (月增)");
        assert_eq!(zh_title("FOMC Press Conference"), "FOMC 記者會");
        assert_eq!(
            zh_title("ECB President Lagarde Speaks"),
            "歐洲央行總裁拉加德談話"
        );
        assert_eq!(zh_title("Bank Holiday"), "休市");
    }

    #[test]
    fn b_ff_9_dictionary_lookup_is_exact_and_beats_suffix_rules() {
        // 在字典裡的標題即使含後綴也取字典值。
        assert_eq!(zh_title("Unemployment Claims"), "初請失業金人數");
        assert_eq!(zh_title("Retail Sales m/m"), "零售銷售 (月增)");
        // 精確比對：大小寫、頭尾空白不同就不命中，改走後綴規則。
        assert_eq!(zh_title("cpi m/m"), "cpi (月增)");
        assert_eq!(zh_title(" CPI m/m"), " CPI (月增)");
        assert_eq!(zh_title("Bank Holiday "), "Bank Holiday ");
    }

    #[test]
    fn b_ff_9_suffix_rules_replace_globally_in_order_and_unknown_stays_english() {
        assert_eq!(zh_title("Foo y/y Speaks"), "Foo (年增) 談話");
        assert_eq!(zh_title("Bar m/m"), "Bar (月增)");
        assert_eq!(zh_title("Baz q/q"), "Baz (季增)");
        assert_eq!(zh_title("Chair Testifies"), "Chair 聽證");
        // 全域取代：同一字串裡出現多次都換。
        assert_eq!(zh_title("A m/m B m/m"), "A (月增) B (月增)");
        // 必須帶前導空白才換：`m/m` 緊貼文字不動。
        assert_eq!(zh_title("Xm/m"), "Xm/m");
        assert_eq!(zh_title("Speaks"), "Speaks");
        assert_eq!(zh_title("Some Unknown Event"), "Some Unknown Event");
        assert_eq!(zh_title(""), "");
    }

    // ───── B-FF-10／K-14：型別異常只略過該列 ─────

    #[test]
    fn k_14_non_empty_non_string_forecast_or_previous_skips_only_that_row() {
        let mk = |title: &str, forecast: Value, previous: Value| {
            json!({"title": title, "country": "USD", "impact": "High",
                   "date": "2026-10-06T08:30:00-04:00", "forecast": forecast, "previous": previous})
        };
        let f = weeks(
            json!([
                mk("num-forecast", json!(1.5), json!("")),
                mk("num-previous", json!(""), json!(13)),
                mk("true-forecast", json!(true), json!("")),
                mk("list-forecast", json!([1]), json!("")),
                mk("obj-previous", json!(""), json!({"a": 1})),
                // 假值：與 Python 一樣當空字串、保留該列。
                mk("null-forecast", json!(null), json!(null)),
                mk("zero-forecast", json!(0), json!(0.0)),
                mk("false-forecast", json!(false), json!([])),
                mk("empty-obj", json!({}), json!("")),
            ]),
            json!([]),
        );
        let out = run_with(&f, &PreviousOutput::default());
        let kept: Vec<_> = out.value.iter().map(|e| e.title_en.as_str()).collect();
        assert_eq!(
            kept,
            [
                "null-forecast",
                "zero-forecast",
                "false-forecast",
                "empty-obj"
            ]
        );
        assert!(out
            .value
            .iter()
            .all(|e| e.forecast.is_empty() && e.previous.is_empty()));
        assert!(
            out.errors.is_empty() && out.fresh,
            "其他列與來源本身不受影響"
        );
    }

    #[test]
    fn k_14_skipped_rows_do_not_poison_the_dedupe_set() {
        // 第一列因 forecast 型別異常被略過；等同它不存在，後面同 (title, date) 的好列要保留。
        let mut bad = row("CPI m/m", "2026-10-06T08:30:00-04:00");
        bad["forecast"] = json!(1.5);
        let good = row("CPI m/m", "2026-10-06T08:30:00-04:00");
        let out = run_with(
            &weeks(json!([bad, good]), json!([])),
            &PreviousOutput::default(),
        );
        assert_eq!(out.value.len(), 1);
        assert_eq!(out.value[0].forecast, "1%");
    }

    #[test]
    fn k_14_non_object_rows_and_non_string_titles_skip_only_that_row() {
        let f = weeks(
            json!([
                "oops",
                5,
                null,
                [1],
                {"title": null, "country": "USD", "impact": "High", "date": "2026-10-06T08:30:00-04:00"},
                {"title": 7, "country": "USD", "impact": "High", "date": "2026-10-06T08:30:00-04:00"},
                row("ok", "2026-10-06T08:30:00-04:00"),
            ]),
            json!([]),
        );
        let out = run_with(&f, &PreviousOutput::default());
        assert_eq!(out.value.len(), 1);
        assert_eq!(out.value[0].title_en, "ok");
    }

    // ───── B-FF-11：沿用與 fresh ─────

    #[test]
    fn b_ff_11_empty_but_successful_result_is_fresh_and_does_not_reuse_old() {
        let prev = PreviousOutput::from_value(json!({"macro": [old_macro_event("舊")]}));
        let out = run_with(&weeks(json!([]), json!([])), &prev);
        assert!(out.fresh);
        assert!(out.value.is_empty(), "成功但空＝空，不沿用舊值");
    }

    #[test]
    fn b_ff_11_old_macro_is_reused_per_item_and_wrong_type_means_absent() {
        let mut no_ts = old_macro_event("缺ts");
        no_ts.as_object_mut().unwrap().remove("ts");
        let prev = PreviousOutput::from_value(json!({
            "macro": [old_macro_event("甲"), no_ts, "oops", old_macro_event("乙")]
        }));
        let got = previous(&prev);
        let titles: Vec<_> = got.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(titles, ["甲", "缺ts", "乙"], "缺 ts 的舊列保留（m4）");
        assert_eq!(got[1].ts, None);
        for bad in [json!("oops"), json!({"a": 1}), json!(null), json!(5)] {
            let prev = PreviousOutput::from_value(json!({"macro": bad}));
            assert!(previous(&prev).is_empty());
        }
        assert!(previous(&PreviousOutput::default()).is_empty());
    }

    // ───── I1：換算後超出 1..=9999 年 ─────

    #[test]
    fn i1_dates_whose_taipei_conversion_leaves_year_1_to_9999_skip_only_that_row() {
        // Python：astimezone 丟 OverflowError，被 `except Exception: continue` 接住。
        // 上界：9999-12-31 23:00 -04:00 → 台北 10000-01-01 11:00。
        // 下界：0001-01-01 00:00 +14:00 → 台北 0000-12-31 18:00。
        let f = weeks(
            json!([
                row("over-upper", "9999-12-31T23:00:00-04:00"),
                row("over-lower", "0001-01-01T00:00:00+14:00"),
                row("ok", "2026-10-06T08:30:00-04:00"),
            ]),
            json!([]),
        );
        let out = run_with(&f, &PreviousOutput::default());
        assert_eq!(out.value.len(), 1, "{:?}", out.value);
        assert_eq!(out.value[0].title_en, "ok");
        assert!(out.errors.is_empty() && out.fresh);
        // 經 run_guarded 也不會走到 panic 路徑。
        let g = block_on(run_guarded(&f, &clock(), &PreviousOutput::default()));
        assert_eq!(g, run_with(&f, &PreviousOutput::default()));
    }

    #[test]
    fn i1_the_extreme_values_that_still_fit_are_kept() {
        // 9999-12-31 00:00 +08:00 剛好是台北 9999-12-31；0001-01-01 08:00 +08:00 剛好是 0001-01-01。
        let f = weeks(
            json!([
                row("max", "9999-12-31T00:00:00+08:00"),
                row("min", "0001-01-01T08:00:00+08:00"),
            ]),
            json!([]),
        );
        let out = run_with(&f, &PreviousOutput::default());
        let dts: Vec<_> = out.value.iter().map(|e| e.dt.as_str()).collect();
        assert_eq!(dts, ["0001-01-01T08:00", "9999-12-31T00:00"]);
    }

    #[test]
    fn n1_utc_before_year_1_skips_the_row_even_if_taipei_is_still_year_1() {
        // Python astimezone 先換 UTC：下面兩個 UTC 都落在第 0 年 → OverflowError、略過；
        // 0001-01-01T08:00+08:00 的 UTC 恰為 0001-01-01T00:00Z → 兩邊都保留。
        let f = weeks(
            json!([
                row("utc-year-0-a", "0001-01-01T07:59:00+08:00"),
                row("utc-year-0-b", "0001-01-01T10:00:00+14:00"),
                row("edge", "0001-01-01T08:00:00+08:00"),
            ]),
            json!([]),
        );
        let out = run_with(&f, &PreviousOutput::default());
        let titles: Vec<_> = out.value.iter().map(|e| e.title_en.as_str()).collect();
        assert_eq!(titles, ["edge"]);
        assert!(out.errors.is_empty() && out.fresh);
    }

    // ───── I2(e)：日期與時間的分隔字元 ─────

    #[test]
    fn i2e_any_ascii_separator_between_date_and_time_parses_like_python() {
        // Python 3.11+ `fromisoformat` 實測：下列都解析成 2026-09-28T08:30-04:00（→ 台北 20:30）。
        for raw in [
            "2026-09-28 08:30:00-04:00",
            "2026-09-28x08:30:00-04:00",
            "2026-09-28t08:30:00-04:00",
            "2026-09-28_08:30:00-04:00",
            "20260928 083000-0400",
            "20260928T083000-0400",
        ] {
            let f = weeks(json!([row("t", raw)]), json!([]));
            let out = run_with(&f, &PreviousOutput::default());
            assert_eq!(out.value.len(), 1, "{raw}");
            assert_eq!(out.value[0].dt, "2026-09-28T20:30", "{raw}");
            assert_eq!(out.value[0].ts, Some(1790598600), "{raw}");
        }
        // 非 ASCII 分隔字元：Python 3.11～3.14 實測同樣解析成 2026-09-28T20:30（N2）。
        for raw in [
            "2026-09-28\u{3000}08:30:00-04:00",
            "2026-09-28\u{e9}08:30:00-04:00",
            "2026-09-28\u{1F600}08:30:00-04:00",
        ] {
            let f = weeks(json!([row("t", raw)]), json!([]));
            let out = run_with(&f, &PreviousOutput::default());
            assert_eq!(out.value.len(), 1, "{raw}");
            assert_eq!(out.value[0].dt, "2026-09-28T20:30", "{raw}");
        }
        // 沒有時間部分、沒有偏移、分隔位置不是字元邊界仍略過。
        for raw in [
            "2026-09-28 08:30:00",
            "2026-09-28",
            "2026-09-2\u{e9}08:30:00-04:00",
        ] {
            let f = weeks(json!([row("t", raw)]), json!([]));
            let out = run_with(&f, &PreviousOutput::default());
            assert!(out.value.is_empty(), "{raw}");
        }
    }

    // ───── D7：panic 隔離 ─────

    #[test]
    fn d7_a_panic_inside_the_source_keeps_the_old_macro() {
        let prev = PreviousOutput::from_value(json!({"macro": [old_macro_event("舊")]}));
        let out = block_on(run_guarded(&PanicFetch, &clock(), &prev));
        assert_eq!(out.value.len(), 1);
        assert_eq!(out.value[0].title, "舊");
        assert!(!out.fresh);
        assert_eq!(out.errors.len(), 1);
        assert!(out.errors[0].starts_with("總經來源失敗（本週檔）：panic: "));
    }

    #[test]
    fn d7_run_guarded_is_transparent_when_nothing_panics() {
        let f = weeks(
            json!([row("CPI m/m", "2026-10-06T08:30:00-04:00")]),
            json!([]),
        );
        let prev = PreviousOutput::default();
        let plain = run_with(&f, &prev);
        let wrapped = block_on(run_guarded(&f, &clock(), &prev));
        assert_eq!(plain, wrapped);
    }

    // ───── oracle ─────

    #[test]
    fn oracle_every_scenario_matches_python_expected_json() {
        oracle::for_each_scenario(|name, sc: &Scenario, expected| {
            let out = block_on(run_guarded(&sc.fetch, &sc.clock, &sc.previous()));
            assert_eq!(
                serde_json::to_value(&out.value).unwrap(),
                expected["macro"],
                "情境 {name}：macro 不符"
            );
            oracle::assert_errors(name, &out.errors, expected, &[ERR_PREFIX]);
        });
    }
}
