//! 動態桌布的上市券資比與融資維持率 `margin`（behavior-inventory B-MG-1～8、B-GRD-1～3、C-3、K-4、K-10、K-17；
//! 原始碼 `update_tw_events.py` 的 `parse_margin_summary`／`parse_margin_stocks`／`parse_day_all_prices`／
//! `compute_margin`／`fetch_margin`，`:960-1082`）。
//!
//! 回傳 [`WallpaperOutcome`]：錯誤進 `wallpaper_errors`、不進 `errors`、不計 `fresh`（B-GRD-2，同 [`super::twii`]）。
//!
//! # 流程（B-MG-7）
//!
//! 1. 取最新已公布的彙總（`selectType=MS`，不帶 `date`）→ 交易日 D；舊值 `date == D` 就**不再下載大表**，直接沿用。
//! 2. 以 D 取個股明細（`selectType=ALL&…&date=YYYYMMDD`，避免兩次請求跨過公布時點）。日期 ≠ D＝失敗（K-17：
//!    margin 這一版檢查日期單一性；`top100` 那一版不檢查）。
//! 3. 取 `STOCK_DAY_ALL` 收盤價（**每輪各下載一次**，與前百大是另一次請求）；價格日期 ≠ D（例如 15:00 那輪價格已是
//!    當日、融資仍是前一日）＝不得混用，**沿用舊值、不記錯誤**（晚間公布前屬正常；沒有舊值就省略該鍵）。
//! 4. 計算：券資比＝融券餘額張 ÷ 融資餘額張 × 100；融資維持率＝Σ(個股融資餘額張 × 1000 × 收盤價) ÷
//!    (融資金額仟元 × 1000) × 100，**皆 `round2`**（K-4）。查無收盤價的個股略過並列入 `unpriced`（依明細表順序），
//!    ETF 與一般股票同樣計入。融資餘額或金額 ≤ 0＝失敗；個股融資張數加總與彙總差距超過 1%
//!    （[`MARGIN_SUM_TOLERANCE`]）＝失敗（防欄位位移）。
//!
//! 任何抓取／解析失敗＝沿用舊值並記 `券資比／融資維持率來源失敗：{e}`。舊值只要是物件就算（Python
//! `isinstance(old, dict)`；不檢查內部）。
//!
//! # 與 Python 的差異
//!
//! - **D8-6**：`ClosingPrice` 為 `inf`／`Infinity` 的列略過（比照 [`super::top100`]：Python 會收進價格表、讓維持率變
//!   `inf`，輸出非法 JSON）；計算結果非有限值＝整個來源失敗。
//! - 張數與金額以 `i64` 存放（Python 是任意精度整數）；超出 `i64` 的數字視為失敗。真實值約 1e7（張）、6e8（仟元）。
//! - `int()` 以前的字串處理（`str(x).replace(",", "").strip()`）、`str.strip()`、Unicode 十進位數字與底線都與 Python 相同
//!   （[`py_int`]）；`fields` 不是陣列（Python 的 `str.index` 對字串是子字串搜尋）視為失敗。
//! - 錯誤訊息內文：`KeyError`／`ValueError` 等盡量逐字，其餘用 [`py_repr`] 近似（B-HTTP-7：內文無消費端解析）。
//! - 輸出物件鍵依字母序（`Output` 的桌布鍵是 `serde_json::Value`；內容相同、消費端依鍵名讀取）。
//! - 另有一層 [`guarded_wallpaper`]（D7）：panic＝非預期錯誤，前綴同 B-GRD-1。

use std::collections::{BTreeSet, HashMap};

use serde::Serialize;
use serde_json::{Map, Value};
use time::Date;

use super::clock::Clock;
use super::dates::{iso_date, round2};
use super::http::{get_json, Fetch};
use super::json_util::{
    py_dict, py_float, py_index, py_int, py_iter, py_repr, py_repr_str, py_str, py_str_or_empty,
    py_strip, py_type_name, roc_to_date_value, truthy,
};
use super::outcome::{guarded_wallpaper, WallpaperOutcome};
use super::output::PreviousOutput;
use super::top100::URL_DAY_ALL;
use super::twii::to_json;

/// B-MG-1：彙總（最新已公布日）。
pub const URL_MARGN_MS: &str =
    "https://www.twse.com.tw/rwd/zh/marginTrading/MI_MARGN?selectType=MS&response=json";
/// B-MG-1：個股明細（後面接 `&date=YYYYMMDD`）。
pub const URL_MARGN_ALL: &str =
    "https://www.twse.com.tw/rwd/zh/marginTrading/MI_MARGN?selectType=ALL&response=json";
/// B-GRD-1 的 label 與一般失敗前綴（B-FLOW-9）。
pub const LABEL: &str = "券資比／融資維持率";
pub const ERR_PREFIX: &str = "券資比／融資維持率來源失敗：";
/// C-3：個股明細加總與彙總張數的容許相對誤差。
pub const MARGIN_SUM_TOLERANCE: f64 = 0.01;

/// `parse_margin_summary` 的結果（單位：張、張、仟元）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub date: String,
    pub margin_lots: i64,
    pub short_lots: i64,
    pub margin_amount_k: i64,
}

/// 個股一列：代號、融資今日餘額（張）、融券今日餘額（張）。
pub type StockRow = (String, i64, i64);

/// 輸出的 `margin`（B-OUT 表）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Margin {
    pub date: String,
    pub margin_lots: i64,
    pub short_lots: i64,
    pub short_margin_ratio: f64,
    pub maintenance_ratio: f64,
    pub unpriced: Vec<String>,
}

/// Python `margin_int`：`int(str(s).replace(",", "").strip())`。
pub fn margin_int(v: &Value) -> Result<i64, String> {
    let text = py_str(v).replace(',', "");
    let text = py_strip(&text);
    let n = py_int(text).ok_or_else(|| {
        format!(
            "invalid literal for int() with base 10: {}",
            py_repr_str(text)
        )
    })?;
    i64::try_from(n).map_err(|_| format!("數字超出範圍：{text}"))
}

/// Python `ymd8_to_date`：`re.fullmatch(r"(\d{4})(\d{2})(\d{2})", str(s).strip())`（`\d` 含 Unicode 十進位數字）
/// → `date`。
pub fn ymd8_to_date(v: &Value) -> Result<Date, String> {
    use super::dates::{digit_value, is_decimal_digit};
    let text = py_str(v);
    let stripped = py_strip(&text);
    let digits: Vec<u32> = stripped
        .chars()
        .map(|c| is_decimal_digit(c).then(|| digit_value(c)))
        .collect::<Option<_>>()
        .filter(|d: &Vec<u32>| d.len() == 8)
        .ok_or_else(|| format!("日期格式不是 YYYYMMDD：{}", py_repr(v)))?;
    let num = |a: usize, b: usize| digits[a..b].iter().fold(0u32, |acc, d| acc * 10 + d);
    let (year, month, day) = (num(0, 4), num(4, 6), num(6, 8));
    // 訊息對照 Python 3.14 的 `date(y, m, d)`。
    if year == 0 {
        return Err(format!("year must be in 1..9999, not {year}"));
    }
    let month_name = time::Month::try_from(u8::try_from(month).unwrap_or(0))
        .map_err(|_| format!("month must be in 1..12, not {month}"))?;
    let days_in_month = month_name.length(year as i32);
    if !(1..=u32::from(days_in_month)).contains(&day) {
        return Err(format!(
            "day {day} must be in range 1..{days_in_month} for month {month} in year {year}"
        ));
    }
    Date::from_calendar_date(year as i32, month_name, day as u8).map_err(|e| e.to_string())
}

/// Python `margin_ok`：`(j or {}).get("stat") != "OK"` → `ValueError`；通過時回傳 `j` 的物件。
fn margin_ok(j: &Value) -> Result<&Map<String, Value>, String> {
    let stat_err = |stat: Option<&Value>| {
        format!(
            "MI_MARGN 回應 stat={}",
            stat.map_or_else(|| "None".to_string(), py_repr)
        )
    };
    if !truthy(j) {
        return Err(stat_err(None));
    }
    let obj = py_dict(j)?;
    match obj.get("stat") {
        Some(Value::String(s)) if s == "OK" => Ok(obj),
        other => Err(stat_err(other)),
    }
}

/// `d[key]`：缺鍵＝`KeyError`（訊息是鍵的 repr）。
fn key<'a>(obj: &'a Map<String, Value>, k: &str) -> Result<&'a Value, String> {
    obj.get(k).ok_or_else(|| py_repr_str(k))
}

/// Python 的 `v["k"]`（`v` 可能不是字典）：字典缺鍵＝`KeyError`；陣列／字串用字串下標＝`TypeError`；
/// `None`／數字／布林不可下標。
fn getitem<'a>(v: &'a Value, k: &str) -> Result<&'a Value, String> {
    match v {
        Value::Object(o) => key(o, k),
        Value::Array(_) => Err("list indices must be integers or slices, not str".to_string()),
        Value::String(_) => Err("string indices must be integers, not 'str'".to_string()),
        other => Err(format!(
            "'{}' object is not subscriptable",
            py_type_name(other)
        )),
    }
}

/// 彙總（`MS` 或 `ALL` 的第一張表）→ 交易日與彙總（B-MG-2、3）。採「今日餘額」欄（`前日餘額`要等下一個
/// 交易日才有，與當日收盤價對不上）。`Err` 的文字是 `{e}`（不含前綴）。
pub fn parse_margin_summary(j: &Value) -> Result<Summary, String> {
    let obj = margin_ok(j)?;
    let day = ymd8_to_date(key(obj, "date")?)?;
    let table = py_index(key(obj, "tables")?, 0)?;
    let col = match getitem(&table, "fields")? {
        Value::Array(fields) => fields
            .iter()
            .position(|f| f.as_str() == Some("今日餘額"))
            .ok_or_else(|| "list.index(x): x not in list".to_string())?,
        // `str.index` 是子字串搜尋（回傳字元位置）。
        Value::String(s) => s
            .find("今日餘額")
            .map(|byte| s[..byte].chars().count())
            .ok_or_else(|| "substring not found".to_string())?,
        other => {
            return Err(format!(
                "'{}' object has no attribute 'index'",
                py_type_name(other)
            ))
        }
    };
    let mut by_label: HashMap<String, Value> = HashMap::new();
    for r in py_iter(getitem(&table, "data")?.clone())? {
        let label = py_strip(&py_str(&py_index(&r, 0)?)).to_string();
        by_label.insert(label, r);
    }
    let cell = |label: &str| -> Result<i64, String> {
        let row = by_label.get(label).ok_or_else(|| py_repr_str(label))?;
        margin_int(&py_index(row, col)?)
    };
    Ok(Summary {
        date: iso_date(day),
        margin_lots: cell("融資(交易單位)")?,
        short_lots: cell("融券(交易單位)")?,
        margin_amount_k: cell("融資金額(仟元)")?,
    })
}

/// `selectType=ALL` → `(交易日, [(代號, 融資今日餘額張, 融券今日餘額張)])`（B-MG-4）。個股表的「今日餘額」欄
/// 出現兩次：第一次是融資、第二次是融券（依欄位順序取，不寫死位置）。
pub fn parse_margin_stocks(j: &Value) -> Result<(String, Vec<StockRow>), String> {
    let obj = margin_ok(j)?;
    let day = iso_date(ymd8_to_date(key(obj, "date")?)?);
    for t in py_iter(key(obj, "tables")?.clone())? {
        let table = py_dict(&t)?;
        let fields: Vec<Value> = match table.get("fields") {
            Some(Value::Array(a)) => a.clone(),
            // 字串：沒有任何一個「字元」等於「今日餘額」、`fields[:1]` 是字串不等於 `["代號"]` → 不是這張表。
            Some(Value::String(_)) => continue,
            // 非空字典：`enumerate` 逐鍵不出錯，`fields[:1]` 是 `KeyError(slice(None, 1, None))`。
            Some(Value::Object(o)) if !o.is_empty() => {
                return Err("slice(None, 1, None)".to_string())
            }
            // 非零數字／`true`：`enumerate(5)` 就是 `TypeError`。
            Some(v) if truthy(v) && !v.is_object() => {
                return Err(format!("'{}' object is not iterable", py_type_name(v)))
            }
            _ => Vec::new(),
        };
        let cols: Vec<usize> = fields
            .iter()
            .enumerate()
            .filter(|(_, f)| f.as_str() == Some("今日餘額"))
            .map(|(i, _)| i)
            .collect();
        if fields.first().and_then(Value::as_str) == Some("代號") && cols.len() == 2 {
            let mut stocks = Vec::new();
            for r in py_iter(key(table, "data")?.clone())? {
                let code = py_strip(&py_str(&py_index(&r, 0)?)).to_string();
                let margin = margin_int(&py_index(&r, cols[0])?)?;
                let short = margin_int(&py_index(&r, cols[1])?)?;
                stocks.push((code, margin, short));
            }
            return Ok((day, stocks));
        }
    }
    Err("MI_MARGN 回應找不到個股融資融券明細表".to_string())
}

/// `STOCK_DAY_ALL` → `(價格所屬交易日, {代號: 收盤價})`（B-MG-5）。`Date` 欄為民國 7 碼，全表須同一天；
/// `ClosingPrice` 空字串／非數字／≤0（停牌、無成交）＝查無收盤價，不收；後面的同代號覆蓋前面。
pub fn parse_day_all_prices(rows: &Value) -> Result<(String, HashMap<String, f64>), String> {
    let items = py_iter(rows.clone())?;
    let mut days: BTreeSet<Option<Date>> = BTreeSet::new();
    for r in &items {
        days.insert(roc_to_date_value(py_dict(r)?.get("Date")));
    }
    let single = match (days.len(), days.iter().next()) {
        (1, Some(Some(d))) => Some(*d),
        _ => None,
    };
    let Some(day) = single else {
        let mut shown: Vec<String> = days
            .iter()
            .map(|d| d.map_or_else(|| "None".to_string(), iso_date))
            .collect();
        shown.sort();
        shown.truncate(3);
        let listed = shown
            .iter()
            .map(|s| py_repr_str(s))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "STOCK_DAY_ALL 的日期欄不是單一有效日期：[{listed}]"
        ));
    };
    let mut prices = HashMap::new();
    for r in &items {
        let obj = py_dict(r)?;
        let raw = py_str_or_empty(obj.get("ClosingPrice")).replace(',', "");
        // `float(...)` 的 ValueError 與 `nan`、非正數都略過；`inf` 依 D8-6 也略過。
        if let Some(v) = py_float(py_strip(&raw)).filter(|v| v.is_finite() && *v > 0.0) {
            prices.insert(py_strip(&py_str_or_empty(obj.get("Code"))).to_string(), v);
        }
    }
    Ok((iso_date(day), prices))
}

/// 券資比與融資維持率（B-MG-6）。
pub fn compute_margin(
    summary: &Summary,
    stocks: &[StockRow],
    prices: &HashMap<String, f64>,
) -> Result<Margin, String> {
    let (margin_lots, short_lots) = (summary.margin_lots, summary.short_lots);
    let amount_yuan = i128::from(summary.margin_amount_k) * 1000;
    if margin_lots <= 0 || amount_yuan <= 0 {
        return Err(format!(
            "融資餘額為 {margin_lots} 張／{amount_yuan} 元，無法計算"
        ));
    }
    let detail: i128 = stocks.iter().map(|s| i128::from(s.1)).sum();
    if (detail - i128::from(margin_lots)).abs() as f64 > margin_lots as f64 * MARGIN_SUM_TOLERANCE {
        return Err(format!(
            "個股融資張數加總 {detail} 與彙總 {margin_lots} 差距過大，疑似欄位位移"
        ));
    }
    let mut value = 0.0_f64;
    let mut unpriced = Vec::new();
    for (code, lots, _short) in stocks {
        match prices.get(code) {
            None => unpriced.push(code.clone()),
            Some(p) => value += (i128::from(*lots) * 1000) as f64 * p,
        }
    }
    let short_margin_ratio = round2(short_lots as f64 / margin_lots as f64 * 100.0);
    let maintenance_ratio = round2(value / amount_yuan as f64 * 100.0);
    if !(short_margin_ratio.is_finite() && maintenance_ratio.is_finite()) {
        return Err("算出非有限數值".to_string());
    }
    Ok(Margin {
        date: summary.date.clone(),
        margin_lots,
        short_lots,
        short_margin_ratio,
        maintenance_ratio,
        unpriced,
    })
}

/// 舊輸出的 `margin`：**是物件**才算（Python `isinstance(old, dict)`；不檢查內部、空物件也算）。
pub fn previous(prev: &PreviousOutput) -> Option<Value> {
    prev.raw("margin").filter(|v| v.is_object()).cloned()
}

/// 由組裝端呼叫：[`run`] 加上 panic 隔離（D7、B-GRD-1）。
pub async fn run_guarded<F: Fetch>(
    fetch: &F,
    clock: &Clock,
    prev: &PreviousOutput,
) -> WallpaperOutcome<Option<Value>> {
    guarded_wallpaper(LABEL, previous(prev), run(fetch, clock, prev)).await
}

async fn run<F: Fetch>(
    fetch: &F,
    _clock: &Clock,
    prev: &PreviousOutput,
) -> WallpaperOutcome<Option<Value>> {
    let old = previous(prev);
    let mut out = WallpaperOutcome::new(old.clone());
    match flow(fetch, old.as_ref(), &mut out.logs).await {
        Ok(Some(new)) => out.value = Some(new),
        // 沿用舊值（同日或價格日期不同），不記錯誤。
        Ok(None) => {}
        Err(e) => {
            out.wallpaper_errors.push(format!("{ERR_PREFIX}{e}"));
            out.logs
                .push(format!("[券資比] 本輪失敗，沿用上次資料：{e}"));
        }
    }
    out
}

/// `Ok(Some(新值))`＝算出新值；`Ok(None)`＝沿用舊值；`Err`＝失敗（呼叫端記錯誤並沿用舊值）。
async fn flow<F: Fetch>(
    fetch: &F,
    old: Option<&Value>,
    logs: &mut Vec<String>,
) -> Result<Option<Value>, String> {
    let ms = get_json(fetch, URL_MARGN_MS)
        .await
        .map_err(|e| e.to_string())?;
    let summary = parse_margin_summary(&ms)?;
    let day = summary.date.clone();
    if old.and_then(|o| o.get("date")) == Some(&Value::String(day.clone())) {
        return Ok(None);
    }
    let d8 = day.replace('-', "");
    let all = get_json(fetch, &format!("{URL_MARGN_ALL}&date={d8}"))
        .await
        .map_err(|e| e.to_string())?;
    let (all_day, stocks) = parse_margin_stocks(&all)?;
    if all_day != day {
        return Err(format!("彙總日期 {day} 與個股明細日期 {all_day} 不一致"));
    }
    let day_all = get_json(fetch, URL_DAY_ALL)
        .await
        .map_err(|e| e.to_string())?;
    let (price_day, prices) = parse_day_all_prices(&day_all)?;
    if price_day != day {
        logs.push(format!(
            "[券資比] 收盤價日期 {price_day} ≠ 融資日期 {day}，不混用，沿用上次資料"
        ));
        return Ok(None);
    }
    Ok(Some(to_json(&compute_margin(&summary, &stocks, &prices)?)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::errors::normalize_error;
    use crate::fetch::oracle;
    use crate::fetch::test_util::{block_on, py_fixture, MapFetch, PanicFetch};
    use serde_json::json;
    use time::macros::datetime;

    // ───────────── 共用小工具 ─────────────

    fn clock() -> Clock {
        Clock::new(
            datetime!(2026-10-01 15:00:00 +8),
            datetime!(2026-10-01 15:00:00),
        )
    }

    fn prev_with(old: Option<Value>) -> PreviousOutput {
        match old {
            Some(v) => PreviousOutput::from_value(json!({ "margin": v })),
            None => PreviousOutput::default(),
        }
    }

    fn url_all(date8: &str) -> String {
        format!("{URL_MARGN_ALL}&date={date8}")
    }

    /// 依彙總的日期登記個股明細的網址（個股明細要用彙總的日期取）。
    fn fetch_of(ms: &Value, all: &Value, day_all: &Value) -> MapFetch {
        let date8 = ms["date"].as_str().map(str::to_string).unwrap_or_default();
        MapFetch::new()
            .ok(URL_MARGN_MS, &ms.to_string())
            .ok(&url_all(&date8), &all.to_string())
            .ok(URL_DAY_ALL, &day_all.to_string())
    }

    fn run_with(fetch: &MapFetch, old: Option<Value>) -> WallpaperOutcome<Option<Value>> {
        block_on(run_guarded(fetch, &clock(), &prev_with(old)))
    }

    fn fixture_fetch(ms: &str, all: &str, day_all: &str) -> MapFetch {
        fetch_of(&py_fixture(ms), &py_fixture(all), &py_fixture(day_all))
    }

    fn lots(day: &str) -> Value {
        py_fixture(&format!("margn_ms_{day}.json"))
    }

    fn near(a: &Value, b: f64) -> bool {
        (a.as_f64().unwrap() - b).abs() < 0.005
    }

    // ───────────── 常數與文字（逐字） ─────────────

    #[test]
    fn constants_and_urls_are_verbatim() {
        assert_eq!(
            URL_MARGN_MS,
            "https://www.twse.com.tw/rwd/zh/marginTrading/MI_MARGN?selectType=MS&response=json"
        );
        assert_eq!(
            url_all("20261001"),
            "https://www.twse.com.tw/rwd/zh/marginTrading/MI_MARGN?selectType=ALL&response=json&date=20261001"
        );
        assert_eq!(MARGIN_SUM_TOLERANCE, 0.01);
        assert_eq!(LABEL, "券資比／融資維持率");
        assert_eq!(ERR_PREFIX, "券資比／融資維持率來源失敗：");
    }

    // ───────────── 移植：tests/test_wallpaper_data.py::MarginTest ─────────────

    mod margin_test {
        use super::*;

        #[test]
        fn test_short_margin_ratio_20260930() {
            let s = parse_margin_summary(&lots("20260930")).unwrap();
            assert_eq!(s.date, "2026-09-30");
            assert_eq!(s.margin_lots, 9_286_318);
            assert_eq!(s.short_lots, 235_296);
            assert_eq!(s.margin_amount_k, 622_252_453);
            assert!((s.short_lots as f64 / s.margin_lots as f64 * 100.0 - 2.53).abs() < 0.005);
        }

        #[test]
        fn test_stocks_rows_sum_to_summary() {
            for d in ["20260930", "20261001"] {
                let s = parse_margin_summary(&lots(d)).unwrap();
                let (day, stocks) =
                    parse_margin_stocks(&py_fixture(&format!("margn_all_{d}.json"))).unwrap();
                assert_eq!(day, s.date);
                assert_eq!(stocks.iter().map(|x| x.1).sum::<i64>(), s.margin_lots);
                assert_eq!(stocks.iter().map(|x| x.2).sum::<i64>(), s.short_lots);
            }
        }

        #[test]
        fn test_day_all_prices_date_and_skip_empty() {
            let (day, prices) =
                parse_day_all_prices(&py_fixture("stock_day_all_20261001.json")).unwrap();
            assert_eq!(day, "2026-10-01");
            assert!(prices.contains_key("2330"));
            assert!(prices.contains_key("0050"));
            assert!(!prices.contains_key("00666R"), "ClosingPrice 為空字串");
            assert!(prices.values().all(|p| *p > 0.0));
        }

        #[test]
        fn test_day_all_with_mixed_dates_is_rejected() {
            let mut rows = py_fixture("stock_day_all_20261001.json");
            rows[0]["Date"] = json!("1150930");
            let e = parse_day_all_prices(&rows).unwrap_err();
            assert_eq!(
                e,
                "STOCK_DAY_ALL 的日期欄不是單一有效日期：['2026-09-30', '2026-10-01']"
            );
        }

        fn real_1001() -> (Summary, Vec<StockRow>, HashMap<String, f64>) {
            let s = parse_margin_summary(&lots("20261001")).unwrap();
            let (_, stocks) = parse_margin_stocks(&py_fixture("margn_all_20261001.json")).unwrap();
            let (_, prices) =
                parse_day_all_prices(&py_fixture("stock_day_all_20261001.json")).unwrap();
            (s, stocks, prices)
        }

        #[test]
        fn test_compute_skips_unpriced_and_keeps_etf() {
            let (s, stocks, prices) = real_1001();
            let out = compute_margin(&s, &stocks, &prices).unwrap();
            let mut unpriced = out.unpriced.clone();
            unpriced.sort();
            assert_eq!(
                unpriced,
                ["00666R", "00707R", "1589", "2323", "2601", "6655", "910322"]
            );
            assert!(stocks.iter().any(|x| x.0 == "0050"), "ETF 有融資 → 計入");
            assert!(!out.unpriced.iter().any(|c| c == "0050"));
            assert_eq!(out.short_margin_ratio, 2.49);
            // 獨立手算：Σ(融資餘額張 × 1000 × 收盤價) ÷ (融資金額仟元 × 1000) × 100
            assert_eq!(out.maintenance_ratio, 195.08);
        }

        #[test]
        fn test_compute_hand_checked_small_case() {
            let s = Summary {
                date: "2026-01-02".into(),
                margin_lots: 35,
                short_lots: 7,
                margin_amount_k: 1500,
            };
            let stocks: Vec<StockRow> = vec![
                ("1111".into(), 10, 1),
                ("2222".into(), 20, 2),
                ("3333".into(), 0, 0),
                ("4444".into(), 5, 0),
            ];
            let prices: HashMap<String, f64> = [("1111", 100.0), ("2222", 50.0), ("3333", 9.0)]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect();
            let out = compute_margin(&s, &stocks, &prices).unwrap();
            // 10×1000×100 + 20×1000×50 = 2,000,000；融資金額 1,500 仟元＝1,500,000 元
            assert_eq!(out.maintenance_ratio, 133.33);
            assert_eq!(out.short_margin_ratio, 20.0);
            assert_eq!(out.unpriced, ["4444"]);
            assert_eq!(out.date, "2026-01-02");
        }

        #[test]
        fn test_compute_rejects_inconsistent_rows() {
            let s = Summary {
                date: "2026-01-02".into(),
                margin_lots: 1000,
                short_lots: 3,
                margin_amount_k: 1500,
            };
            let prices = HashMap::from([("1111".to_string(), 100.0)]);
            let e = compute_margin(&s, &[("1111".into(), 10, 0)], &prices).unwrap_err();
            assert_eq!(e, "個股融資張數加總 10 與彙總 1000 差距過大，疑似欄位位移");
        }

        #[test]
        fn test_compute_rejects_zero_margin() {
            let s = Summary {
                date: "2026-01-02".into(),
                margin_lots: 0,
                short_lots: 0,
                margin_amount_k: 0,
            };
            let e = compute_margin(&s, &[], &HashMap::new()).unwrap_err();
            assert_eq!(e, "融資餘額為 0 張／0 元，無法計算");
        }

        #[test]
        fn test_fetch_same_day_prices_computes_and_requests_by_date() {
            let fetch = fixture_fetch(
                "margn_ms_20261001.json",
                "margn_all_20261001.json",
                "stock_day_all_20261001.json",
            );
            let out = run_with(&fetch, None);
            let res = out.value.unwrap();
            assert_eq!(res["date"], "2026-10-01");
            assert_eq!(res["margin_lots"], 9_352_816);
            assert_eq!(res["short_lots"], 233_322);
            assert!(near(&res["short_margin_ratio"], 2.49));
            assert!(near(&res["maintenance_ratio"], 195.08));
            assert!(out.wallpaper_errors.is_empty());
            // 個股明細要用彙總的日期取，避免兩次請求跨過公布時點；順序 MS → ALL → STOCK_DAY_ALL
            assert_eq!(
                fetch.requests(),
                [
                    URL_MARGN_MS.to_string(),
                    url_all("20261001"),
                    URL_DAY_ALL.to_string()
                ]
            );
        }

        #[test]
        fn test_fetch_prices_newer_than_margin_keeps_old() {
            // 15:00 那輪：價格已是 10/1、融資仍是 9/30 → 不得混用，沿用舊值
            let old = json!({"date": "2026-09-29", "margin_lots": 1, "short_lots": 1,
                "short_margin_ratio": 1.0, "maintenance_ratio": 1.0, "unpriced": []});
            let fetch = fixture_fetch(
                "margn_ms_20260930.json",
                "margn_all_20260930.json",
                "stock_day_all_20261001.json",
            );
            let out = run_with(&fetch, Some(old.clone()));
            assert_eq!(out.value, Some(old.clone()));
            assert_eq!(out.value.unwrap()["date"], "2026-09-29");
            assert!(
                out.wallpaper_errors.is_empty(),
                "晚間公布前屬正常情況，不算錯誤"
            );
            assert_eq!(
                out.logs,
                ["[券資比] 收盤價日期 2026-10-01 ≠ 融資日期 2026-09-30，不混用，沿用上次資料"]
            );
        }

        #[test]
        fn test_fetch_mismatch_without_old_returns_none() {
            let fetch = fixture_fetch(
                "margn_ms_20260930.json",
                "margn_all_20260930.json",
                "stock_day_all_20261001.json",
            );
            let out = run_with(&fetch, None);
            assert_eq!(out.value, None);
            assert!(out.wallpaper_errors.is_empty());
        }

        #[test]
        fn test_fetch_same_date_as_old_skips_heavy_downloads() {
            let old = json!({"date": "2026-10-01", "margin_lots": 1, "short_lots": 1,
                "short_margin_ratio": 1.0, "maintenance_ratio": 1.0, "unpriced": []});
            let fetch = fixture_fetch(
                "margn_ms_20261001.json",
                "margn_all_20261001.json",
                "stock_day_all_20261001.json",
            );
            let out = run_with(&fetch, Some(old.clone()));
            assert_eq!(out.value, Some(old));
            assert_eq!(fetch.requests(), [URL_MARGN_MS], "同日不重載大表");
            assert!(out.wallpaper_errors.is_empty() && out.logs.is_empty());
        }

        #[test]
        fn test_fetch_failure_keeps_old_and_records_error() {
            let old = json!({"date": "2026-09-30", "margin_lots": 9286318, "short_lots": 235296,
                "short_margin_ratio": 2.53, "maintenance_ratio": 195.7, "unpriced": []});
            let fetch = MapFetch::new().network_error(URL_MARGN_MS, "來源無回應");
            let out = run_with(&fetch, Some(old.clone()));
            assert_eq!(out.value, Some(old));
            assert_eq!(
                out.wallpaper_errors,
                ["券資比／融資維持率來源失敗：來源無回應"]
            );
            assert_eq!(out.logs, ["[券資比] 本輪失敗，沿用上次資料：來源無回應"]);
        }

        #[test]
        fn test_fetch_no_data_stat_keeps_old_and_records_error() {
            let old = json!({"date": "2026-09-30"});
            let fetch =
                MapFetch::new().ok(URL_MARGN_MS, r#"{"stat": "很抱歉，沒有符合條件的資料"}"#);
            let out = run_with(&fetch, Some(old.clone()));
            assert_eq!(out.value, Some(old));
            assert_eq!(
                out.wallpaper_errors,
                ["券資比／融資維持率來源失敗：MI_MARGN 回應 stat='很抱歉，沒有符合條件的資料'"]
            );
        }

        #[test]
        fn test_fetch_ms_and_all_date_mismatch_is_error() {
            let old = json!({"date": "2026-09-29"});
            // MS 是 10/1、ALL（以 10/1 請求）回的卻是 9/30
            let fetch = fixture_fetch(
                "margn_ms_20261001.json",
                "margn_all_20260930.json",
                "stock_day_all_20261001.json",
            );
            let out = run_with(&fetch, Some(old.clone()));
            assert_eq!(out.value, Some(old));
            assert_eq!(
                out.wallpaper_errors,
                ["券資比／融資維持率來源失敗：彙總日期 2026-10-01 與個股明細日期 2026-09-30 不一致"]
            );
            assert_eq!(
                fetch.requests().len(),
                2,
                "日期不一致時不再抓 STOCK_DAY_ALL"
            );
        }
    }

    // ───────────── 移植：MalformedOldTest.test_margin_malformed_old ─────────────

    #[test]
    fn test_margin_malformed_old() {
        for bad in [json!(["x"]), json!("字串"), json!(5)] {
            let fetch = fixture_fetch(
                "margn_ms_20261001.json",
                "margn_all_20261001.json",
                "stock_day_all_20261001.json",
            );
            let out = run_with(&fetch, Some(bad.clone()));
            assert_eq!(out.value.unwrap()["date"], "2026-10-01", "{bad}");
            let fetch = MapFetch::new().network_error(URL_MARGN_MS, "x");
            let out = run_with(&fetch, Some(bad.clone()));
            assert_eq!(out.value, None, "{bad}");
        }
    }

    // ───────────── 移植：MainIntegrationTest 屬於本來源的部分（組裝端的 fetched／errors 在 4.1） ─────────────

    mod main_integration_test {
        use super::*;

        #[test]
        fn test_truncated_responses_in_new_keys_never_stop_the_round() {
            // 回應傳到一半斷線／格式壞掉：只記 wallpaper_errors、沒有舊值就省略該鍵，且不 panic
            for body in [r#"{"stat": "OK", "date": "2026100"#, "", "null"] {
                let fetch = MapFetch::new().ok(URL_MARGN_MS, body);
                let out = run_with(&fetch, None);
                assert_eq!(out.value, None, "{body}");
                assert_eq!(out.wallpaper_errors.len(), 1, "{body}");
                assert!(out.wallpaper_errors[0].starts_with(ERR_PREFIX));
            }
        }

        #[test]
        fn test_unexpected_exception_in_a_new_key_never_stops_the_round() {
            // Python 的「非預期例外」在 Rust 是 panic：只記 wallpaper_errors（B-GRD-1 的前綴）、沿用舊值
            let old = json!({"date": "2026-09-30"});
            let out = block_on(run_guarded(
                &PanicFetch,
                &clock(),
                &prev_with(Some(old.clone())),
            ));
            assert_eq!(out.value, Some(old));
            assert_eq!(out.wallpaper_errors.len(), 1);
            assert!(out.wallpaper_errors[0]
                .starts_with("券資比／融資維持率來源發生非預期錯誤：panic: "));
            assert_eq!(
                normalize_error(&out.wallpaper_errors[0]),
                "券資比／融資維持率來源發生非預期錯誤："
            );
        }

        #[test]
        fn test_new_keys_do_not_refresh_fetched_and_failures_never_reach_errors() {
            // 型別層保證：WallpaperOutcome 沒有 `errors`／`fresh`（解構會編譯失敗）；
            // 組裝端的 fetched 與 errors 內容由 4.1 的組裝測試與 oracle 對照把關。
            let fetch = fixture_fetch(
                "margn_ms_20261001.json",
                "margn_all_20261001.json",
                "stock_day_all_20261001.json",
            );
            let WallpaperOutcome {
                value,
                wallpaper_errors,
                logs: _,
            } = run_with(&fetch, None);
            assert!(value.is_some() && wallpaper_errors.is_empty());
        }
    }

    // ───────────── 行為條目 → 測試 ─────────────

    #[test]
    fn b_mg_2_stat_must_be_ok_and_date_is_yyyymmdd() {
        assert_eq!(
            parse_margin_summary(&json!({"stat": "x", "date": "20261001"})).unwrap_err(),
            "MI_MARGN 回應 stat='x'"
        );
        assert_eq!(
            parse_margin_summary(&json!(null)).unwrap_err(),
            "MI_MARGN 回應 stat=None"
        );
        assert_eq!(
            ymd8_to_date(&json!("20261001")).unwrap(),
            time::macros::date!(2026 - 10 - 01)
        );
        assert_eq!(
            ymd8_to_date(&json!("2026-10-01")).unwrap_err(),
            "日期格式不是 YYYYMMDD：'2026-10-01'"
        );
        // `\d` 含 Unicode 十進位數字；strip 後再比對
        assert_eq!(
            ymd8_to_date(&json!(" ２０２６１００１ ")).unwrap(),
            time::macros::date!(2026 - 10 - 01)
        );
    }

    #[test]
    fn b_mg_3_summary_uses_the_first_today_balance_column_and_labels() {
        // 「今日餘額」欄第一次出現；列標籤 strip；重複標籤後者勝
        let j = json!({"stat": "OK", "date": "20261001", "tables": [{
        "fields": ["項目", "今日餘額", "x", "今日餘額"],
        "data": [
            ["  融資(交易單位)  ", "1,000", "0", "7"],
            ["融券(交易單位)", "25", "0", "7"],
            ["融資金額(仟元)", "50,000", "0", "7"],
            ["融券(交易單位)", "26", "0", "7"],
        ]}]});
        let s = parse_margin_summary(&j).unwrap();
        assert_eq!(
            (s.margin_lots, s.short_lots, s.margin_amount_k),
            (1000, 26, 50_000)
        );
    }

    #[test]
    fn b_mg_4_stocks_table_has_exactly_two_today_balance_columns_first_is_margin() {
        let (day, stocks) = parse_margin_stocks(&py_fixture("margn_all_20261001.json")).unwrap();
        assert_eq!(day, "2026-10-01");
        assert!(stocks.len() > 1000);
        // 第一列（00400A）：融資今日餘額 9,303、融券今日餘額 0（見 fixture 第一列 data）
        assert_eq!(stocks[0], ("00400A".to_string(), 9_303, 0));
    }

    #[test]
    fn b_mg_5_day_all_price_rules() {
        let rows = json!([
            {"Date": "1151001", "Code": " 1101 ", "ClosingPrice": "1,234.5"},
            {"Date": "1151001", "Code": "1102", "ClosingPrice": ""},
            {"Date": "1151001", "Code": "1103", "ClosingPrice": "0"},
            {"Date": "1151001", "Code": "1104", "ClosingPrice": "-1"},
            {"Date": "1151001", "Code": "1105", "ClosingPrice": "abc"},
            {"Date": "1151001", "Code": "1101", "ClosingPrice": "9"},
        ]);
        let (day, prices) = parse_day_all_prices(&rows).unwrap();
        assert_eq!(day, "2026-10-01");
        assert_eq!(prices.len(), 1);
        assert_eq!(prices["1101"], 9.0, "同代號後者覆蓋前者");
    }

    #[test]
    fn b_mg_6_tolerance_boundary_is_inclusive_and_checked_in_floating_point() {
        let s = |lots: i64| Summary {
            date: "2026-10-01".into(),
            margin_lots: lots,
            short_lots: 0,
            margin_amount_k: 1000,
        };
        let prices = HashMap::from([("1101".to_string(), 10.0)]);
        // 彙總 1000 張：差 10（恰好 1%）通過、差 11 失敗
        assert!(compute_margin(&s(1000), &[("1101".into(), 990, 0)], &prices).is_ok());
        assert!(compute_margin(&s(1000), &[("1101".into(), 1010, 0)], &prices).is_ok());
        assert!(compute_margin(&s(1000), &[("1101".into(), 989, 0)], &prices).is_err());
        assert!(compute_margin(&s(1000), &[("1101".into(), 1011, 0)], &prices).is_err());
    }

    #[test]
    fn b_mg_6_rounding_matches_python_round_for_the_k4_boundaries() {
        let s = |m: i64, sh: i64| Summary {
            date: "d".into(),
            margin_lots: m,
            short_lots: sh,
            margin_amount_k: m * 10,
        };
        let one = |m: i64, sh: i64| {
            compute_margin(
                &s(m, sh),
                &[("1101".into(), m, 0)],
                &HashMap::from([("1101".to_string(), 10.0)]),
            )
            .unwrap()
            .short_margin_ratio
        };
        assert_eq!(one(800, 1), 0.12, "0.125 恰好二進位 .5 取偶");
        assert_eq!(one(4000, 107), 2.67, "2.675 二進位略小");
        assert_eq!(one(3, 1), 33.33);
        assert_eq!(one(7, 1), 14.29);
    }

    #[test]
    fn b_mg_7_old_with_a_non_string_date_never_equals_the_day() {
        // `old.get("date") == day` 只在字串相等時為真：數字 20261001 不算同日 → 會重載並重算
        let fetch = fixture_fetch(
            "margn_ms_20261001.json",
            "margn_all_20261001.json",
            "stock_day_all_20261001.json",
        );
        let out = run_with(&fetch, Some(json!({"date": 20261001})));
        assert_eq!(out.value.unwrap()["date"], "2026-10-01");
        assert_eq!(fetch.requests().len(), 3);
    }

    #[test]
    fn b_mg_7_price_day_check_comes_after_the_detail_date_check() {
        // ALL 日期不一致優先報錯（即使價格日期也不同）
        let fetch = fixture_fetch(
            "margn_ms_20261001.json",
            "margn_all_20260930.json",
            "stock_day_all_20261001.json",
        );
        let out = run_with(&fetch, None);
        assert_eq!(out.wallpaper_errors.len(), 1);
        assert!(out.logs.iter().all(|l| !l.contains("不混用")));
    }

    #[test]
    fn b_mg_8_price_mismatch_without_old_omits_the_key_with_no_error() {
        let fetch = fixture_fetch(
            "margn_ms_20260930.json",
            "margn_all_20260930.json",
            "stock_day_all_20261001.json",
        );
        let out = run_with(&fetch, None);
        assert!(out.value.is_none() && out.wallpaper_errors.is_empty());
    }

    #[test]
    fn k_17_day_all_is_downloaded_every_round_and_dates_must_be_single() {
        // 價格表 Date 欄有兩個日期＝失敗（margin 這一版檢查日期單一性；top100 那一版不檢查）
        let mut rows = py_fixture("stock_day_all_20261001.json");
        rows[1]["Date"] = json!("1150930");
        let fetch = fetch_of(
            &lots("20261001"),
            &py_fixture("margn_all_20261001.json"),
            &rows,
        );
        let out = run_with(&fetch, Some(json!({"date": "2026-09-30"})));
        assert_eq!(out.wallpaper_errors.len(), 1);
        assert!(out.wallpaper_errors[0].contains("不是單一有效日期"));
    }

    // ───────────── D8-6 ─────────────

    #[test]
    fn d8_6_infinite_closing_price_rows_are_skipped_and_nan_is_skipped_like_python() {
        let rows = json!([
            {"Date": "1151001", "Code": "A", "ClosingPrice": "inf"},
            {"Date": "1151001", "Code": "B", "ClosingPrice": "Infinity"},
            {"Date": "1151001", "Code": "C", "ClosingPrice": "nan"},
            {"Date": "1151001", "Code": "D", "ClosingPrice": "1e400"},
            {"Date": "1151001", "Code": "E", "ClosingPrice": "5"},
        ]);
        let (_, prices) = parse_day_all_prices(&rows).unwrap();
        assert_eq!(prices.len(), 1);
        assert!(prices.contains_key("E"));
    }

    #[test]
    fn d8_6_a_non_finite_result_fails_the_source() {
        let t = table();
        let c = &t["compute_nonfinite"];
        let summary: Summary = summary_of(&c["summary"]);
        let prices: HashMap<String, f64> = c["prices"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_f64().unwrap()))
            .collect();
        let stocks = stocks_of(&c["stocks"]);
        // Python 實測：維持率算出 inf（`repr` 見表），輸出會是非法 JSON
        assert!(
            c["python_repr"].as_str().unwrap().contains("inf"),
            "{}",
            c["python_repr"]
        );
        assert_eq!(
            compute_margin(&summary, &stocks, &prices).unwrap_err(),
            "算出非有限數值"
        );
    }

    // ───────────── D7：panic 隔離 ─────────────

    #[test]
    fn d7_a_panic_without_a_usable_old_value_omits_the_key() {
        let out = block_on(run_guarded(
            &PanicFetch,
            &clock(),
            &prev_with(Some(json!([1]))),
        ));
        assert_eq!(out.value, None);
        assert_eq!(out.wallpaper_errors.len(), 1);
        let out = block_on(run_guarded(
            &PanicFetch,
            &clock(),
            &PreviousOutput::default(),
        ));
        assert_eq!(out.value, None);
    }

    #[test]
    fn d7_run_guarded_is_transparent_when_nothing_panics() {
        let fetch = fixture_fetch(
            "margn_ms_20261001.json",
            "margn_all_20261001.json",
            "stock_day_all_20261001.json",
        );
        let out = run_with(&fetch, None);
        assert!(out.value.is_some() && out.wallpaper_errors.is_empty() && out.logs.is_empty());
    }

    #[test]
    fn previous_only_needs_an_object() {
        let p = PreviousOutput::from_value(json!({"margin": {}}));
        assert_eq!(previous(&p), Some(json!({})));
        for bad in [json!([]), json!("x"), json!(5), json!(null)] {
            assert_eq!(
                previous(&PreviousOutput::from_value(json!({"margin": bad}))),
                None
            );
        }
        assert_eq!(previous(&PreviousOutput::default()), None);
    }

    // ───────────── 與 Python 的對照表（真的呼叫 update_tw_events 產生，見 testdata） ─────────────

    fn table() -> Value {
        serde_json::from_str(include_str!("testdata/margin_python_cases.json"))
            .expect("margin_python_cases.json 是合法 JSON")
    }

    fn summary_of(v: &Value) -> Summary {
        Summary {
            date: v["date"].as_str().unwrap().to_string(),
            margin_lots: v["margin_lots"].as_i64().unwrap(),
            short_lots: v["short_lots"].as_i64().unwrap(),
            margin_amount_k: v["margin_amount_k"].as_i64().unwrap(),
        }
    }

    fn stocks_of(v: &Value) -> Vec<StockRow> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|s| {
                (
                    s[0].as_str().unwrap().to_string(),
                    s[1].as_i64().unwrap(),
                    s[2].as_i64().unwrap(),
                )
            })
            .collect()
    }

    fn opt(v: &Value) -> Option<Value> {
        if v.is_null() {
            None
        } else {
            Some(v.clone())
        }
    }

    /// 把 Python 的請求紀錄（`MS`、`ALL:<date8>`、`DAY_ALL`）轉成網址。
    fn python_call_urls(calls: &Value) -> Vec<String> {
        calls
            .as_array()
            .unwrap()
            .iter()
            .map(|c| match c.as_str().unwrap() {
                "MS" => URL_MARGN_MS.to_string(),
                "DAY_ALL" => URL_DAY_ALL.to_string(),
                other => url_all(other.strip_prefix("ALL:").unwrap()),
            })
            .collect()
    }

    fn flow_fetch(case: &Value) -> MapFetch {
        let mut f = MapFetch::new().ok(URL_MARGN_MS, &case["ms"].to_string());
        // 個股明細登記在 Python 實際請求的日期；Rust 若請求了別的日期就會「未登記」而失敗。
        for call in case["calls"].as_array().unwrap() {
            if let Some(d) = call.as_str().unwrap().strip_prefix("ALL:") {
                f = f.ok(&url_all(d), &case["all"].to_string());
            }
        }
        f.ok(URL_DAY_ALL, &case["day_all"].to_string())
    }

    #[test]
    fn python_cross_check_flow_table_matches_the_real_fetch_margin() {
        let t = table();
        let cases = t["flow"].as_array().unwrap();
        assert!(cases.len() >= 150, "{}", cases.len());
        let mut exact = 0;
        for case in cases {
            let name = case["name"].as_str().unwrap();
            let old = case.get("old").cloned();
            let fetch = flow_fetch(case);
            let out = run_with(&fetch, old.clone());
            if name.starts_with("i64_") {
                // Python 任意精度整數；Rust 的 i64 放不下（模組文件「與 Python 的差異」）：Python 在計算階段
                // 才因加總不符失敗、Rust 在解析階段就失敗；兩者都沿用舊值並記一筆錯誤。
                assert_eq!(out.value, opt(&case["value"]), "{name}: value");
                assert_eq!(out.wallpaper_errors.len(), 1, "{name}");
                assert!(out.wallpaper_errors[0].starts_with(ERR_PREFIX), "{name}");
                continue;
            }
            if name.starts_with("d8_6_price_inf_") {
                // Python：該列價格為 inf → 維持率 inf（輸出非法 JSON）；Rust：該列略過（＝查無收盤價）。
                // 結果應等同「該列價格為空字串」的案例（price_cell_variant_1）。
                assert!(case["python_nonfinite"].as_bool().unwrap(), "{name}");
                let blank = cases
                    .iter()
                    .find(|c| c["name"] == "price_cell_variant_1")
                    .unwrap();
                assert_eq!(out.value, opt(&blank["value"]), "{name}");
                assert!(out.wallpaper_errors.is_empty(), "{name}");
                continue;
            }
            assert_eq!(out.value, opt(&case["value"]), "{name}: value");
            let expected: Vec<String> = case["errors"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| e.as_str().unwrap().to_string())
                .collect();
            let norm =
                |v: &[String]| -> Vec<String> { v.iter().map(|e| normalize_error(e)).collect() };
            assert_eq!(
                norm(&out.wallpaper_errors),
                norm(&expected),
                "{name}: errors"
            );
            assert_eq!(
                fetch.requests(),
                python_call_urls(&case["calls"]),
                "{name}: 請求順序與網址"
            );
            if out.wallpaper_errors == expected {
                exact += 1;
            }
        }
        assert!(exact >= 100, "錯誤訊息逐字相同的案例只有 {exact}");
    }

    #[test]
    fn python_cross_check_flow_error_messages_that_differ_from_python_are_only_the_known_kinds() {
        // 逐字比對之外的案例必須屬於已知類型（Python 的內建例外訊息、repr 的鍵序）；新增案例造成未知差異會在這裡現形。
        let t = table();
        let mut differing = Vec::new();
        for case in t["flow"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            if name.starts_with("d8_6_") || name.starts_with("i64_") {
                continue;
            }
            let fetch = flow_fetch(case);
            let out = run_with(&fetch, case.get("old").cloned());
            let expected: Vec<String> = case["errors"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| e.as_str().unwrap().to_string())
                .collect();
            if out.wallpaper_errors != expected {
                differing.push(format!(
                    "{name}\n  py={expected:?}\n  rs={:?}",
                    out.wallpaper_errors
                ));
            }
        }
        assert!(differing.is_empty(), "{}", differing.join("\n"));
    }

    #[test]
    fn python_cross_check_net_failure_matches() {
        let t = table();
        let nf = &t["net_fail"];
        let fetch = MapFetch::new().status(URL_MARGN_MS, 500);
        let out = run_with(&fetch, Some(json!({"date": "2026-09-30"})));
        assert_eq!(out.value, opt(&nf["value"]));
        assert_eq!(
            out.wallpaper_errors,
            nf["errors"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| e.as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        );
        assert_eq!(fetch.requests(), python_call_urls(&nf["calls"]));
    }

    #[test]
    fn python_cross_check_margin_int_table() {
        let t = table();
        let cases = t["margin_int"].as_array().unwrap();
        assert!(cases.len() >= 35);
        for c in cases {
            let got = margin_int(&c["in"]);
            match c.get("ok") {
                // 超出 i64 的整數（Python 任意精度）：Rust 視為失敗（模組文件已記）
                Some(v) if v.as_i64().is_some() => {
                    assert_eq!(got, Ok(v.as_i64().unwrap()), "{}", c["in"])
                }
                Some(v) => assert!(got.is_err(), "{} → Python {v}", c["in"]),
                None => {
                    let e = got.expect_err(&c["in"].to_string());
                    // 訊息逐字比對只限字串／純量輸入（陣列、物件的 `str()` 是 Python repr，這裡是 JSON 文字）
                    if c["err"][0] == "ValueError" && !c["in"].is_array() && !c["in"].is_object() {
                        assert_eq!(e, c["err"][1].as_str().unwrap(), "{}", c["in"]);
                    }
                }
            }
        }
    }

    #[test]
    fn python_cross_check_ymd8_table() {
        let t = table();
        let cases = t["ymd8"].as_array().unwrap();
        assert!(cases.len() >= 25);
        for c in cases {
            let got = ymd8_to_date(&c["in"]).map(iso_date);
            match c.get("ok") {
                Some(v) => assert_eq!(got.as_deref(), Ok(v.as_str().unwrap()), "{}", c["in"]),
                None => {
                    let e = got.expect_err(&c["in"].to_string());
                    assert_eq!(e, c["err"][1].as_str().unwrap(), "{}", c["in"]);
                }
            }
        }
    }

    #[test]
    fn python_cross_check_compute_table_including_round2_boundaries() {
        let t = table();
        let cases = t["compute"].as_array().unwrap();
        assert!(cases.len() >= 150, "{}", cases.len());
        let (mut oks, mut errs) = (0, 0);
        for c in cases {
            let prices: HashMap<String, f64> = c["prices"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), v.as_f64().unwrap()))
                .collect();
            let got = compute_margin(
                &summary_of(&c["summary"]),
                &stocks_of(&c["stocks"]),
                &prices,
            );
            match c.get("ok") {
                Some(v) => {
                    oks += 1;
                    // Python 的鍵序是 date, margin_lots, …；比內容（Value 相等不看鍵序）
                    assert_eq!(serde_json::to_value(got.unwrap()).unwrap(), *v, "{c}");
                }
                None => {
                    errs += 1;
                    assert_eq!(got.unwrap_err(), c["err"][1].as_str().unwrap(), "{c}");
                }
            }
        }
        assert!(oks >= 100 && errs >= 10, "ok={oks} err={errs}");
    }

    // ───────────── oracle ─────────────

    #[test]
    fn oracle_every_scenario_matches_python_expected_json_for_all_three_keys() {
        // 三鍵一起跑（B-FLOW-5 順序 intraday → daily → margin）：值、全部 wallpaper_errors（前綴子序列＝整個陣列）、
        // 鍵的省略、errors 不含桌布錯誤（B-GRD-2）。
        use crate::fetch::twii::{run_daily_guarded, run_intraday_guarded};
        let mut n = 0;
        oracle::for_each_scenario(|name, sc, expected| {
            n += 1;
            let prev = sc.previous();
            let i = block_on(run_intraday_guarded(&sc.fetch, &sc.clock, &prev));
            let d = block_on(run_daily_guarded(&sc.fetch, &sc.clock, &prev));
            let m = block_on(run_guarded(&sc.fetch, &sc.clock, &prev));
            assert_eq!(
                m.value,
                expected.get("margin").cloned(),
                "情境 {name}：margin"
            );
            let got: Vec<String> = i
                .wallpaper_errors
                .iter()
                .chain(&d.wallpaper_errors)
                .chain(&m.wallpaper_errors)
                .map(|e| normalize_error(e))
                .collect();
            let want: Vec<String> = expected["wallpaper_errors"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| normalize_error(e.as_str().unwrap()))
                .collect();
            assert_eq!(got, want, "情境 {name}：wallpaper_errors（三鍵合併、依序）");
            for e in expected["errors"].as_array().unwrap() {
                let e = e.as_str().unwrap();
                assert!(
                    !e.starts_with("加權盤中走勢")
                        && !e.starts_with("加權日K")
                        && !e.starts_with("券資比"),
                    "情境 {name}：桌布錯誤不得進 errors：{e}"
                );
            }
        });
        assert!(n >= 30, "情境數 {n}");
    }

    #[test]
    fn oracle_margin_scenarios_show_the_key_evidence() {
        let run = |name: &str| {
            let sc = crate::fetch::fixture::Scenario::named(name).unwrap();
            let prev = sc.previous();
            let out = block_on(run_guarded(&sc.fetch, &sc.clock, &prev));
            (out, prev, sc)
        };
        // 日期不一致：失敗、沿用舊檔
        let (out, prev, _) = run("margin-date-mismatch");
        assert_eq!(out.value.as_ref(), prev.raw("margin"));
        assert_eq!(
            out.wallpaper_errors,
            ["券資比／融資維持率來源失敗：彙總日期 2026-10-02 與個股明細日期 2026-10-01 不一致"]
        );
        // 價格日期不同：沿用舊檔、不記錯誤、日誌說明
        let (out, prev, sc) = run("margin-price-date-mismatch");
        assert_eq!(out.value.as_ref(), prev.raw("margin"));
        assert!(out.wallpaper_errors.is_empty());
        assert_eq!(out.logs.len(), 1);
        assert!(out.logs[0].starts_with("[券資比] 收盤價日期 2026-10-01 ≠ 融資日期 2026-10-02"));
        assert_eq!(
            sc.fetch.request_count(URL_DAY_ALL),
            1,
            "此來源只下載一次 STOCK_DAY_ALL（前百大走快取、不下載）"
        );
        // 一般情境：算出新值
        let (out, _, _) = run("recorded-20261005");
        let v = out.value.unwrap();
        assert!(out.wallpaper_errors.is_empty());
        assert!(v["margin_lots"].is_i64() && v["unpriced"].is_array());
    }
}
