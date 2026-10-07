//! 行情條（Yahoo Finance v8 chart；behavior-inventory 4.8／B-QT-1～10、C-2、K-2、B-OUT-4、B-FLOW-6／7／9／18；
//! 原始碼 `update_tw_events.py` 的 `QUOTES`、`prev_close_from_chart`、`fetch_quotes`，`:103-116`、`:706-768`）。
//!
//! SOFR 3M 不在這裡（NY Fed，3.1 的 [`super::sofr`]）；組裝端把它 append 在本模組 12 檔之後（B-QT-10、C-2）。
//!
//! # 昨收（K-2；曾造成多日累計漲跌幅誤植的正式環境 bug）
//!
//! `prev = prev_close_from_chart(result) or meta.chartPreviousClose`。**不可只用 `chartPreviousClose`**：那是
//! 「所請求 range 起點之前」的收盤，`range=5d` 下是數個交易日前，漲跌幅會變成多日累計。
//! [`prev_close_from_chart`] 從 `close` 序列自取（B-QT-4）：
//!
//! 1. `stamps`（`timestamp`）非空且與 `closes` 等長：以 `meta.gmtoffset`（缺／`null`／`0`＝0）把每個 **close 非
//!    `null`** 的點換成「交易所當地日序」`(t + offset) // 86400`（向下取整，負值也正確），取最後一個日序不同於
//!    「最後一個點的日序」的收盤。盤中 Yahoo 會在當日 K 之外另附一筆即時報價，同一交易日佔兩個位置，所以不是
//!    倒數第二筆。**有非空的 `rows` 卻找不到不同日＝回 `None`，不走第 2 步**（呼叫端退回 `chartPreviousClose`）。
//! 2. 否則（沒有 `timestamp`、長度不符、或 `rows` 為空）：倒數第二筆非 `null` 收盤，不足兩筆＝`None`。
//!
//! 呼叫端的 `or` 是 Python 真值語意：序列算出 `0.0`（或 `-0.0`）也退回 `chartPreviousClose`；兩邊都缺或為 0
//! 才是失敗（B-QT-5）。價格與昨收 `float()` 後 `chg_abs = price - prev`、`chg_pct = (price / prev - 1) * 100`，
//! **都不捨入**（B-QT-6、B-OUT-4）。
//!
//! # 逐檔失敗與沿用（B-QT-7／8、B-FLOW-6）
//!
//! 每檔獨立：失敗記 `行情來源失敗（{name}／{symbol}）：{e}`（同時寫日誌 `[行情] …，跳過`）；舊輸出 `quotes`
//! 中同名項目（`{q.name: q}`，名稱非空、同名取最後一筆）存在就**原樣沿用在原位置**（日誌 `[行情] {name} 沿用上次
//! 資料`），否則該檔省略——**被沿用的檔仍然記錯誤**。`fresh` 恆為 `false`（B-QT-10：Yahoo 不算新鮮）。
//! 每檔的呼叫另加 `catch_unwind`（D7）：該檔 panic＝該檔失敗（記 `…：panic: …`、沿用舊項），其餘照常。
//!
//! # 與 Python 的差異
//!
//! - **D8-6**（數值欄位的極端型別，一律視為該檔失敗）：布林當價格／昨收／收盤／時間戳／`gmtoffset`（Python 的
//!   `float(True)` 是 1.0）、字串 `"nan"`／`"inf"` 或算出 `inf`（如 `1e308 / 1e-308`）。回應的 JSON 字面值
//!   `NaN`／`Infinity` 在共用的 `http::parse_json` 就失敗。另外昨收字串 `"0"`（真值、`float` 後為 0）→
//!   Python 是 `ZeroDivisionError`，這裡同樣是該檔失敗。
//! - **`t` 只收整數**：輸出結構的 `t` 是 `Option<i64>`；`regularMarketTime` 為字串或小數時 Python 照原樣輸出，這裡
//!   視為 `null`（真實來源是整數秒）。
//! - 錯誤訊息的內文：Python 型別錯誤用 `repr`，這裡 `result 為空（{error}）` 的 `{error}` 若是物件／陣列取 JSON
//!   文字、不是 Python 的 `repr`（B-HTTP-7：訊息內文無消費端解析）。

use std::collections::HashMap;

use serde_json::{Map, Value};

use super::clock::Clock;
use super::http::{get_json, Fetch};
use super::json_util::{py_dict, py_float, py_index, py_iter, py_str, py_type_name, truthy};
use super::outcome::{guarded, SourceOutcome};
use super::output::{PreviousOutput, Quote, QuoteKind};

/// B-QT-1：端點樣板。
pub const URL_QUOTE: &str =
    "https://query1.finance.yahoo.com/v8/finance/chart/{symbol}?interval=1d&range=5d";
/// 整個來源 panic 時的錯誤前綴（正常路徑的前綴是每檔各自的 `行情來源失敗（{name}／{symbol}）：`）。
pub const ERR_PREFIX: &str = "行情來源失敗（Yahoo）：";

/// C-2：Yahoo 的 12 檔（順序即顯示順序；第 13 檔 SOFR 3M 由 [`super::sofr`] 負責）。
/// 欄位：Yahoo symbol、顯示名、kind。
pub const QUOTES: [(&str, &str, QuoteKind); 12] = [
    ("USDTWD=X", "USD/TWD", QuoteKind::Fx),
    ("^TNX", "US 10Y", QuoteKind::Yield),
    ("CL=F", "WTI 原油", QuoteKind::Cmdty),
    ("^TWII", "加權指數", QuoteKind::Index),
    ("2330.TW", "台積電", QuoteKind::Stock),
    ("^N225", "日經225", QuoteKind::Index),
    ("^KS11", "KOSPI", QuoteKind::Index),
    ("^STOXX50E", "歐股50", QuoteKind::Index),
    ("^DJI", "道瓊", QuoteKind::Index),
    ("^GSPC", "S&P 500", QuoteKind::Index),
    ("^IXIC", "NASDAQ", QuoteKind::Index),
    ("^SOX", "費半", QuoteKind::Index),
];

/// Python `urllib.parse.quote(s)`（預設 `safe="/"`）：UTF-8 位元組中，英數與 `_.-~` 及 `/` 原樣，其餘 `%XX`（大寫）。
/// `^`→`%5E`、`=`→`%3D`（B-QT-1）。
pub fn py_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-' | b'~' | b'/') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// 某檔的請求網址（B-QT-1）。
pub fn quote_url(symbol: &str) -> String {
    URL_QUOTE.replace("{symbol}", &py_quote(symbol))
}

/// 舊輸出 `quotes` 以名稱為鍵（`{q.name: q for q in old.quotes if q.name}`：名稱非空、同名取最後一筆；
/// 逐筆判形狀，D8-3）。SOFR 3M 也在裡面，由 [`super::sofr`] 取用。
fn old_by_name(prev: &PreviousOutput) -> HashMap<String, Quote> {
    prev.get_items::<Quote>("quotes")
        .unwrap_or_default()
        .into_iter()
        .filter(|q| !q.name.is_empty())
        .map(|q| (q.name.clone(), q))
        .collect()
}

/// 來源整個失敗（panic）時沿用的值：12 檔中舊輸出有同名項目者，依 [`QUOTES`] 順序。
pub fn previous(prev: &PreviousOutput) -> Vec<Quote> {
    let old = old_by_name(prev);
    QUOTES
        .iter()
        .filter_map(|(_, name, _)| old.get(*name).cloned())
        .collect()
}

/// 由組裝端呼叫：[`run`] 加上最外層 panic 隔離（每檔另有各自的隔離，見模組文件）。
pub async fn run_guarded<F: Fetch>(
    fetch: &F,
    clock: &Clock,
    prev: &PreviousOutput,
) -> SourceOutcome<Vec<Quote>> {
    guarded(ERR_PREFIX, previous(prev), run(fetch, clock, prev)).await
}

async fn run<F: Fetch>(
    fetch: &F,
    _clock: &Clock,
    prev: &PreviousOutput,
) -> SourceOutcome<Vec<Quote>> {
    let old = old_by_name(prev);
    let mut out = SourceOutcome::new(Vec::new(), false);
    for (symbol, name, kind) in QUOTES {
        let fallback = old.get(name).cloned();
        let prefix = format!("行情來源失敗（{name}／{symbol}）：");
        let one = guarded(
            &prefix,
            fallback.clone(),
            one_quote(fetch, symbol, name, kind, &prefix, fallback.clone()),
        )
        .await;
        if let Some(q) = one.value {
            out.value.push(q);
        }
        out.errors.extend(one.errors);
        out.logs.extend(one.logs);
    }
    out
}

/// 抓一檔；失敗時回沿用值（或 `None`）並記錯誤＋日誌。
async fn one_quote<F: Fetch>(
    fetch: &F,
    symbol: &str,
    name: &str,
    kind: QuoteKind,
    prefix: &str,
    fallback: Option<Quote>,
) -> SourceOutcome<Option<Quote>> {
    let result = match get_json(fetch, &quote_url(symbol)).await {
        Ok(json) => parse_chart(&json, name, kind),
        Err(e) => Err(e.to_string()),
    };
    match result {
        Ok(q) => SourceOutcome::new(Some(q), false),
        Err(e) => {
            let msg = format!("{prefix}{e}");
            let reused = fallback.is_some();
            let mut out = SourceOutcome::new(fallback, false);
            out.logs.push(format!("[行情] {msg}，跳過"));
            out.errors.push(msg);
            if reused {
                out.logs.push(format!("[行情] {name} 沿用上次資料"));
            }
            out
        }
    }
}

/// 一個 JSON 數值（保留整數／浮點的區分，對應 Python 的 `int`／`float` 運算）。
#[derive(Debug, Clone, Copy)]
enum Num {
    Int(i128),
    Float(f64),
}

impl Num {
    /// 布林、字串等 Python 能做 `+` 但 D8-6 不收（或根本不能運算）的型別一律 `Err`。
    fn from_value(v: &Value) -> Result<Num, String> {
        match v {
            Value::Number(n) => Ok(match (n.as_i64(), n.as_u64()) {
                (Some(i), _) => Num::Int(i128::from(i)),
                (None, Some(u)) => Num::Int(i128::from(u)),
                _ => Num::Float(n.as_f64().ok_or("數值不是有限值")?),
            }),
            Value::Bool(_) => Err("布林值不是數字".to_string()),
            other => Err(format!(
                "unsupported operand type(s) for +: '{}'",
                py_type_name(other)
            )),
        }
    }

    /// `(self + offset) // 86400`（Python 的向下取整除法；整數對整數保持整數）。
    fn day_with(self, offset: Num) -> Result<i128, String> {
        const DAY: i128 = 86_400;
        match (self, offset) {
            (Num::Int(a), Num::Int(b)) => {
                let sum = a.checked_add(b).ok_or("整數溢位")?;
                Ok(sum.div_euclid(DAY))
            }
            (a, b) => {
                let sum = a.as_f64() + b.as_f64();
                if !sum.is_finite() {
                    return Err("時間戳加上 gmtoffset 不是有限值".to_string());
                }
                let day = DAY as f64;
                Ok(((sum - sum.rem_euclid(day)) / day).floor() as i128)
            }
        }
    }

    fn as_f64(self) -> f64 {
        match self {
            Num::Int(i) => i as f64,
            Num::Float(f) => f,
        }
    }
}

/// Python `float(x)`（`x` 非 `None`）；D8-6：布林與非有限值視為失敗。
pub(super) fn to_float(v: &Value) -> Result<f64, String> {
    let f = match v {
        Value::Number(n) => n.as_f64().ok_or("數值不是有限值")?,
        Value::String(s) => {
            py_float(s).ok_or_else(|| format!("could not convert string to float: '{s}'"))?
        }
        Value::Bool(_) => return Err("布林值不是數字".to_string()),
        other => {
            return Err(format!(
                "float() argument must be a string or a real number, not '{}'",
                py_type_name(other)
            ))
        }
    };
    if f.is_finite() {
        Ok(f)
    } else {
        Err(format!("數值不是有限值：{v}"))
    }
}

/// `x or []` 後逐元素迭代（陣列／物件逐鍵／字串逐字元；其餘 `TypeError`）。
pub(super) fn seq(v: Option<&Value>) -> Result<Vec<Value>, String> {
    match v {
        Some(v) if truthy(v) => py_iter(v.clone()),
        _ => Ok(Vec::new()),
    }
}

/// `x or {}` 後當字典用（`.get`）；非字典的真值＝`AttributeError`。
pub(super) fn dict_or_empty<'a>(
    v: Option<&'a Value>,
    empty: &'a Map<String, Value>,
) -> Result<&'a Map<String, Value>, String> {
    match v {
        Some(v) if truthy(v) => py_dict(v),
        _ => Ok(empty),
    }
}

/// 從日線序列取「上一交易日收盤」（B-QT-4，演算法與理由見模組文件）。`Ok(None)`＝取不到（呼叫端才知道要退回
/// `chartPreviousClose`）；`Err`＝Python 在此丟例外（形狀或型別不符）。
pub fn prev_close_from_chart(result: &Map<String, Value>) -> Result<Option<f64>, String> {
    let empty = Map::new();
    // quote = ((result.get("indicators") or {}).get("quote") or [{}])[0]
    let indicators = dict_or_empty(result.get("indicators"), &empty)?;
    let quote_first = match indicators.get("quote") {
        Some(v) if truthy(v) => py_index(v, 0)?,
        _ => Value::Object(Map::new()),
    };
    let quote = py_dict(&quote_first)?;
    let closes = seq(quote.get("close"))?;
    let stamps = seq(result.get("timestamp"))?;
    if !stamps.is_empty() && stamps.len() == closes.len() {
        let meta = dict_or_empty(result.get("meta"), &empty)?;
        // `meta.get("gmtoffset") or 0`；型別不對的 offset 只在真的要做 `t + offset` 時才失敗（Python 的惰性）。
        let offset = match meta.get("gmtoffset") {
            Some(v) if truthy(v) => Num::from_value(v),
            _ => Ok(Num::Int(0)),
        };
        let mut rows: Vec<(i128, f64)> = Vec::new();
        for (t, c) in stamps.iter().zip(closes.iter()) {
            if c.is_null() {
                continue;
            }
            let day = Num::from_value(t)?.day_with(offset.clone()?)?;
            rows.push((day, to_float(c)?));
        }
        if let Some(&(today, _)) = rows.last() {
            return Ok(rows.iter().rev().find(|(d, _)| *d != today).map(|r| r.1));
        }
    }
    let mut valid = Vec::new();
    for c in closes.iter().filter(|c| !c.is_null()) {
        valid.push(to_float(c)?);
    }
    Ok(if valid.len() >= 2 {
        Some(valid[valid.len() - 2])
    } else {
        None
    })
}

/// 一檔的回應 → [`Quote`]（B-QT-2、3、5、6、9）。`Err` 的文字是 `{e}`（不含前綴）。
pub fn parse_chart(j: &Value, name: &str, kind: QuoteKind) -> Result<Quote, String> {
    let empty = Map::new();
    let top = py_dict(j)?;
    let chart = dict_or_empty(top.get("chart"), &empty)?;
    let first = match chart.get("result") {
        Some(v) if truthy(v) => py_index(v, 0)?,
        _ => {
            let error = chart
                .get("error")
                .map_or_else(|| "None".to_string(), py_str);
            return Err(format!("result 為空（{error}）"));
        }
    };
    let result = py_dict(&first)?;
    let meta = dict_or_empty(result.get("meta"), &empty)?;
    let price = meta.get("regularMarketPrice").filter(|v| !v.is_null());
    // 先算序列昨收（可能因形狀問題丟例外，且早於「price 缺值」的檢查，與 Python 的求值順序相同）
    let from_series = prev_close_from_chart(result)?;
    let prev_value: Value = match from_series {
        Some(p) if p != 0.0 => Value::from(p), // 有限值，`Value::from` 不會變 null
        _ => meta
            .get("chartPreviousClose")
            .cloned()
            .unwrap_or(Value::Null),
    };
    let Some(price) = price else {
        return Err("regularMarketPrice 缺值".to_string());
    };
    if !truthy(&prev_value) {
        return Err("前一交易日收盤缺值或為 0".to_string());
    }
    let price = to_float(price)?;
    let prev = to_float(&prev_value)?;
    if prev == 0.0 {
        return Err("float division by zero".to_string());
    }
    let chg_abs = price - prev;
    let chg_pct = (price / prev - 1.0) * 100.0;
    if !(chg_abs.is_finite() && chg_pct.is_finite()) {
        return Err("算出非有限數值".to_string());
    }
    Ok(Quote {
        name: name.to_string(),
        kind,
        price,
        prev,
        chg_abs,
        chg_pct,
        t: meta.get("regularMarketTime").and_then(Value::as_i64),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::oracle;
    use crate::fetch::sofr;
    use crate::fetch::test_util::{block_on, MapFetch, PanicFetch};
    use serde_json::json;
    use time::macros::datetime;

    fn clock() -> Clock {
        Clock::new(
            datetime!(2026-10-05 00:00:00 +8),
            datetime!(2026-10-05 00:00:00),
        )
    }

    const DAY: i64 = 86_400;
    /// 一個 UTC 午夜。
    const BASE: i64 = 1_790_000_000 - 1_790_000_000 % DAY;

    /// 兩個交易日的正常回應：昨收 `prev`、現價 `price`。
    fn chart(price: f64, prev: f64, t: i64) -> Value {
        json!({"chart": {"result": [{
            "meta": {"regularMarketPrice": price, "gmtoffset": 0, "regularMarketTime": t},
            "timestamp": [BASE, BASE + DAY],
            "indicators": {"quote": [{"close": [prev, price]}]},
        }], "error": null}})
    }

    /// 12 檔都正常的 `MapFetch`（價格＝100＋序號、昨收＝100）。
    fn all_ok() -> MapFetch {
        let mut f = MapFetch::new();
        for (i, (symbol, _, _)) in QUOTES.iter().enumerate() {
            f = f.ok(
                &quote_url(symbol),
                &chart(100.0 + i as f64, 100.0, 1_790_000_000 + i as i64).to_string(),
            );
        }
        f
    }

    fn run_with(fetch: &MapFetch, prev: &PreviousOutput) -> SourceOutcome<Vec<Quote>> {
        block_on(run_guarded(fetch, &clock(), prev))
    }

    fn old_quote(name: &str, kind: &str, price: f64) -> Value {
        json!({"name": name, "kind": kind, "price": price, "prev": 2.0,
               "chg_abs": price - 2.0, "chg_pct": 0.5, "t": 1})
    }

    // ───── C-2、B-QT-1 ─────

    #[test]
    fn c_2_quotes_table_is_verbatim() {
        let got: Vec<(&str, &str, &str)> = QUOTES
            .iter()
            .map(|(s, n, k)| {
                (
                    *s,
                    *n,
                    match k {
                        QuoteKind::Fx => "fx",
                        QuoteKind::Yield => "yield",
                        QuoteKind::Cmdty => "cmdty",
                        QuoteKind::Index => "index",
                        QuoteKind::Stock => "stock",
                    },
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                ("USDTWD=X", "USD/TWD", "fx"),
                ("^TNX", "US 10Y", "yield"),
                ("CL=F", "WTI 原油", "cmdty"),
                ("^TWII", "加權指數", "index"),
                ("2330.TW", "台積電", "stock"),
                ("^N225", "日經225", "index"),
                ("^KS11", "KOSPI", "index"),
                ("^STOXX50E", "歐股50", "index"),
                ("^DJI", "道瓊", "index"),
                ("^GSPC", "S&P 500", "index"),
                ("^IXIC", "NASDAQ", "index"),
                ("^SOX", "費半", "index"),
            ]
        );
    }

    #[test]
    fn c_2_quotes_table_equals_the_python_one() {
        let table: Value = serde_json::from_str(include_str!("testdata/quotes_python_cases.json"))
            .expect("測試資料是合法 JSON");
        let py: Vec<(String, String, String)> = table["quotes"]
            .as_array()
            .expect("quotes")
            .iter()
            .map(|q| {
                (
                    q[0].as_str().expect("symbol").to_string(),
                    q[1].as_str().expect("name").to_string(),
                    q[2].as_str().expect("kind").to_string(),
                )
            })
            .collect();
        let rust: Vec<(String, String, String)> = QUOTES
            .iter()
            .map(|(s, n, k)| {
                (
                    (*s).to_string(),
                    (*n).to_string(),
                    serde_json::to_value(k)
                        .expect("序列化")
                        .as_str()
                        .expect("字串")
                        .to_string(),
                )
            })
            .collect();
        assert_eq!(rust, py);
        // 與 SOFR 的名稱不撞
        assert!(QUOTES.iter().all(|q| q.1 != sofr::SOFR_NAME));
    }

    #[test]
    fn b_qt_1_url_template_and_quote_encoding_match_python() {
        assert_eq!(
            URL_QUOTE,
            "https://query1.finance.yahoo.com/v8/finance/chart/{symbol}?interval=1d&range=5d"
        );
        // 文件中的三個範例
        assert_eq!(
            quote_url("USDTWD=X"),
            "https://query1.finance.yahoo.com/v8/finance/chart/USDTWD%3DX?interval=1d&range=5d"
        );
        assert_eq!(
            quote_url("CL=F").split('/').next_back().unwrap_or(""),
            "CL%3DF?interval=1d&range=5d"
        );
        assert_eq!(py_quote("^TWII"), "%5ETWII");
        // 逐項對 Python `urllib.parse.quote`（含 `&`、`%`、空白、`/`、`~`、中文、é、emoji、換行、NUL）
        let table: Value = serde_json::from_str(include_str!("testdata/quotes_python_cases.json"))
            .expect("測試資料是合法 JSON");
        let urls = table["urls"].as_array().expect("urls");
        assert!(urls.len() >= 25);
        for u in urls {
            let symbol = u["symbol"].as_str().expect("symbol");
            assert_eq!(
                py_quote(symbol),
                u["quoted"].as_str().expect("quoted"),
                "{symbol:?}"
            );
            assert_eq!(
                quote_url(symbol),
                u["url"].as_str().expect("url"),
                "{symbol:?}"
            );
        }
    }

    #[test]
    fn b_qt_1_all_twelve_requests_are_made_in_order_with_range_5d() {
        let f = all_ok();
        let out = run_with(&f, &PreviousOutput::default());
        let want: Vec<String> = QUOTES.iter().map(|q| quote_url(q.0)).collect();
        assert_eq!(f.requests(), want);
        assert!(f
            .requests()
            .iter()
            .all(|u| u.ends_with("?interval=1d&range=5d")));
        assert_eq!(out.value.len(), 12);
        assert_eq!(
            out.value
                .iter()
                .map(|q| q.name.as_str())
                .collect::<Vec<_>>(),
            QUOTES.iter().map(|q| q.1).collect::<Vec<_>>()
        );
        assert!(out.errors.is_empty() && out.logs.is_empty());
        assert!(!out.fresh, "B-QT-10：Yahoo 行情不計 fresh");
    }

    // ───── B-QT-2、B-QT-6、B-QT-9：取值與計算 ─────

    #[test]
    fn b_qt_2_6_9_fields_and_unrounded_arithmetic() {
        let q =
            parse_chart(&chart(11.0, 10.0, 1_790_000_100), "N", QuoteKind::Index).expect("成功");
        assert_eq!((q.price, q.prev, q.t), (11.0, 10.0, Some(1_790_000_100)));
        // 與 Python 實測同值：不捨入、保留浮點誤差
        assert_eq!(q.chg_abs, 1.0);
        assert_eq!(q.chg_pct, 10.000000000000009);
        // 輸出鍵序（B-QT-9）：name、kind、price、prev、chg_abs、chg_pct、t
        let text = serde_json::to_string(&q).expect("序列化");
        assert_eq!(
            text,
            r#"{"name":"N","kind":"index","price":11.0,"prev":10.0,"chg_abs":1.0,"chg_pct":10.000000000000009,"t":1790000100}"#
        );
    }

    #[test]
    fn b_qt_9_prev_keeps_a_long_fraction_and_t_is_null_when_missing() {
        let body = json!({"chart": {"result": [{
            "meta": {"regularMarketPrice": 31.9},
            "timestamp": [BASE, BASE + DAY],
            "indicators": {"quote": [{"close": [31.867900848388672, 31.9]}]},
        }]}});
        let q = parse_chart(&body, "N", QuoteKind::Fx).expect("成功");
        assert_eq!(q.prev, 31.867900848388672);
        assert_eq!(q.t, None);
        assert!(serde_json::to_string(&q)
            .expect("序列化")
            .ends_with(r#""t":null}"#));
    }

    // ───── 昨收（K-2）：對 Python `prev_close_from_chart` 的整表對照 ─────

    fn cases() -> Value {
        serde_json::from_str(include_str!("testdata/quotes_python_cases.json"))
            .expect("測試資料是合法 JSON")
    }

    /// Python 接受、但 D8-6 視為失敗的輸入（布林當數值）。
    const D8_6_BOOL_CASES: [&str; 2] = ["close true (bool)", "gmtoffset true"];

    #[test]
    fn k_2_prev_close_matches_python_on_the_whole_table() {
        let cases = cases();
        let table = cases["prev_close"].as_array().expect("prev_close");
        assert!(table.len() >= 50, "表應有 50 列，實得 {}", table.len());
        let (mut ok, mut err) = (0, 0);
        for c in table {
            let desc = c["desc"].as_str().expect("desc");
            let got = prev_close_from_chart(c["result"].as_object().expect("result"));
            if D8_6_BOOL_CASES.contains(&desc) {
                assert!(got.is_err(), "{desc}：D8-6 要求布林視為失敗，得 {got:?}");
                err += 1;
                continue;
            }
            match (&c["expect"]["ok"], c["expect"].get("err")) {
                (Value::Null, None) => {
                    assert_eq!(got, Ok(None), "{desc}");
                    ok += 1;
                }
                (v, None) if v.is_number() => {
                    assert_eq!(got, Ok(Some(v.as_f64().expect("f64"))), "{desc}");
                    ok += 1;
                }
                (_, Some(py_err)) => {
                    assert!(got.is_err(), "{desc}：Python 丟 {py_err}，Rust 得 {got:?}");
                    err += 1;
                }
                other => panic!("{desc}：看不懂的期望 {other:?}"),
            }
        }
        assert!(ok >= 30 && err >= 10, "ok={ok} err={err}");
    }

    fn series(stamps: &[i64], closes: Vec<Value>, offset: i64) -> Map<String, Value> {
        json!({
            "meta": {"gmtoffset": offset},
            "timestamp": stamps,
            "indicators": {"quote": [{"close": closes}]},
        })
        .as_object()
        .expect("物件")
        .clone()
    }

    #[test]
    fn k_2_named_scenarios() {
        let n = Value::Null;
        // 盤中另附的即時點與當日 K 同一當地日 → 取前一日，而不是倒數第二筆
        let r = series(
            &[BASE, BASE + DAY, BASE + DAY + 8 * 3600],
            vec![json!(10.0), n.clone(), json!(11.0)],
            0,
        );
        assert_eq!(prev_close_from_chart(&r), Ok(Some(10.0)));
        // 當日 K 已有部分收盤值：不可取到 10.5（倒數第二筆有效收盤）
        let r = series(
            &[BASE, BASE + DAY, BASE + DAY + 8 * 3600],
            vec![json!(10.0), json!(10.5), json!(11.0)],
            0,
        );
        assert_eq!(prev_close_from_chart(&r), Ok(Some(10.0)));
        // 全在同一當地日 → None（不走倒數第二筆）
        let r = series(
            &[BASE, BASE + 3600, BASE + 7200],
            vec![json!(4.0), json!(4.2), json!(4.5)],
            0,
        );
        assert_eq!(prev_close_from_chart(&r), Ok(None));
        // gmtoffset 把 UTC 15:00 推到下一個當地日（^N225 型）：offset=0 與 +9h 結果不同
        let stamps = [BASE - 9 * 3600, BASE + 15 * 3600, BASE + 25 * 3600];
        let closes = vec![json!(99.0), json!(100.0), json!(101.0)];
        assert_eq!(
            prev_close_from_chart(&series(&stamps, closes.clone(), 9 * 3600)),
            Ok(Some(99.0))
        );
        assert_eq!(
            prev_close_from_chart(&series(&stamps, closes, 0)),
            Ok(Some(100.0))
        );
        // 負 offset（美東）：用向下取整的當地日序
        let r = series(
            &[
                BASE + 4 * 3600,
                BASE + DAY + 4 * 3600,
                BASE + DAY + 12 * 3600,
            ],
            vec![json!(70.1), json!(70.2), json!(70.3)],
            -4 * 3600,
        );
        assert_eq!(prev_close_from_chart(&r), Ok(Some(70.1)));
        // 當地日序在 epoch 以前（負的 t + offset）也要向下取整
        let r = series(
            &[-3600, -1800, 3600],
            vec![json!(1.0), json!(2.0), json!(3.0)],
            0,
        );
        assert_eq!(
            prev_close_from_chart(&r),
            Ok(Some(2.0)),
            "-3600、-1800 同屬 -1 日，3600 屬 0 日"
        );
        // 沒有 timestamp：倒數第二筆有效收盤；只有一筆＝None
        let no_stamps = |closes: Vec<Value>| {
            json!({"meta": {}, "indicators": {"quote": [{"close": closes}]}})
                .as_object()
                .expect("物件")
                .clone()
        };
        assert_eq!(
            prev_close_from_chart(&no_stamps(vec![json!(1.0), json!(2.0), json!(3.0)])),
            Ok(Some(2.0))
        );
        assert_eq!(
            prev_close_from_chart(&no_stamps(vec![
                n.clone(),
                json!(5.0),
                n.clone(),
                json!(6.0)
            ])),
            Ok(Some(5.0))
        );
        assert_eq!(
            prev_close_from_chart(&no_stamps(vec![json!(1.0)])),
            Ok(None)
        );
        // 長度不符也走倒數第二筆
        let r = series(
            &[BASE, BASE + DAY],
            vec![json!(1.0), json!(2.0), json!(3.0)],
            0,
        );
        assert_eq!(prev_close_from_chart(&r), Ok(Some(2.0)));
    }

    #[test]
    fn k_2_zero_prev_from_series_falls_back_to_chart_previous_close_like_python_or() {
        let body = |cpc: Value| {
            json!({"chart": {"result": [{
                "meta": {"regularMarketPrice": 11.0, "gmtoffset": 0, "chartPreviousClose": cpc},
                "timestamp": [BASE, BASE + DAY],
                "indicators": {"quote": [{"close": [0.0, 2.0]}]},
            }]}})
        };
        let q = parse_chart(&body(json!(40.0)), "N", QuoteKind::Index).expect("退回 40");
        assert_eq!((q.prev, q.chg_abs, q.chg_pct), (40.0, -29.0, -72.5));
        assert_eq!(
            parse_chart(&body(Value::Null), "N", QuoteKind::Index),
            Err("前一交易日收盤缺值或為 0".to_string())
        );
        assert_eq!(
            parse_chart(&body(json!(0)), "N", QuoteKind::Index),
            Err("前一交易日收盤缺值或為 0".to_string())
        );
    }

    #[test]
    fn k_2_chart_previous_close_is_only_a_fallback_never_preferred() {
        // 序列有昨收 10.0；chartPreviousClose 刻意給 99.0（多日累計的值）→ 必須用 10.0
        let body = json!({"chart": {"result": [{
            "meta": {"regularMarketPrice": 11.0, "gmtoffset": 0, "chartPreviousClose": 99.0},
            "timestamp": [BASE, BASE + DAY],
            "indicators": {"quote": [{"close": [10.0, 11.0]}]},
        }]}});
        let q = parse_chart(&body, "N", QuoteKind::Index).expect("成功");
        assert_eq!(q.prev, 10.0);
        // 同一當地日、序列算不出昨收 → 才退回 chartPreviousClose
        let body = json!({"chart": {"result": [{
            "meta": {"regularMarketPrice": 11.0, "gmtoffset": 0, "chartPreviousClose": 5.0},
            "timestamp": [BASE, BASE + 3600],
            "indicators": {"quote": [{"close": [4.0, 4.2]}]},
        }]}});
        assert_eq!(
            parse_chart(&body, "N", QuoteKind::Index)
                .expect("成功")
                .prev,
            5.0
        );
    }

    // ───── B-QT-2／3／5：整檔回應對 Python `fetch_quotes` 的整表對照 ─────

    /// Python 接受、但 D8-6 視為失敗的整檔輸入。
    const D8_6_FETCH_CASES: [&str; 2] = ["price true", "fallback cpc true"];

    fn strip_prefix(msg: &str) -> &str {
        msg.strip_prefix("行情來源失敗（NAME／SYM）：")
            .unwrap_or(msg)
    }

    #[test]
    fn b_qt_5_whole_response_matches_python_on_the_whole_table() {
        let cases = cases();
        let table = cases["fetch"].as_array().expect("fetch");
        assert_eq!(table.len(), 58, "表應有 58 列");
        for c in table {
            let desc = c["desc"].as_str().expect("desc");
            let got = parse_chart(&c["body"], "NAME", QuoteKind::Index);
            let expect = &c["expect"];
            if D8_6_FETCH_CASES.contains(&desc) || expect.get("nonfinite").is_some() {
                assert!(got.is_err(), "{desc}：D8-6 要求失敗，得 {got:?}");
                continue;
            }
            if let Some(ok) = expect.get("ok") {
                let q = got.unwrap_or_else(|e| panic!("{desc}：Python 成功，Rust 失敗：{e}"));
                assert_eq!(q.name, "NAME");
                assert_eq!(
                    serde_json::to_value(q.kind).expect("序列化"),
                    ok["kind"],
                    "{desc}"
                );
                for key in ["price", "prev", "chg_abs", "chg_pct"] {
                    assert_eq!(
                        serde_json::to_value(&q).expect("序列化")[key],
                        ok[key],
                        "{desc}：{key}"
                    );
                }
                // `t`：Python 的字串時間戳在 Rust 是 null（見模組文件）
                let want_t = if ok["t"].is_i64() {
                    ok["t"].clone()
                } else {
                    Value::Null
                };
                assert_eq!(
                    serde_json::to_value(&q).expect("序列化")["t"],
                    want_t,
                    "{desc}：t"
                );
            } else if let Some(py_err) = expect.get("err") {
                let want = strip_prefix(py_err.as_str().expect("err 是字串"));
                let e = got.expect_err(&format!("{desc}：Python 失敗（{want}），Rust 卻成功"));
                // 訊息：`float division by zero`（Python 3.11–3.13）／`division by zero`（3.14）；
                // `result 為空（{error 物件}）` 的內文是 repr 與 JSON 之別（見模組文件）。
                if desc == "result null with error dict" {
                    assert!(
                        e.starts_with("result 為空（") && e.ends_with('）'),
                        "{desc}：{e}"
                    );
                } else if want.ends_with("division by zero") {
                    assert!(e.ends_with("division by zero"), "{desc}：{e}");
                } else {
                    assert_eq!(e, want, "{desc}");
                }
            } else {
                panic!("{desc}：Python 崩潰於 fetch_quotes 之外：{expect}");
            }
        }
    }

    #[test]
    fn b_qt_5_error_messages_are_verbatim() {
        let good = |price: Value| {
            json!({"chart": {"result": [{
                "meta": {"regularMarketPrice": price, "gmtoffset": 0},
                "timestamp": [BASE, BASE + DAY],
                "indicators": {"quote": [{"close": [10.0, 11.0]}]},
            }]}})
        };
        assert_eq!(
            parse_chart(&good(Value::Null), "N", QuoteKind::Fx),
            Err("regularMarketPrice 缺值".to_string())
        );
        assert_eq!(
            parse_chart(
                &json!({"chart": {"result": [], "error": null}}),
                "N",
                QuoteKind::Fx
            ),
            Err("result 為空（None）".to_string())
        );
        assert_eq!(
            parse_chart(
                &json!({"chart": {"result": null, "error": "boom"}}),
                "N",
                QuoteKind::Fx
            ),
            Err("result 為空（boom）".to_string())
        );
    }

    // ───── B-QT-7／8：逐檔沿用 ─────

    #[test]
    fn b_qt_7_a_failed_quote_reuses_its_old_one_in_place_and_still_records_the_error() {
        // 日經225（第 6 檔）與 KOSPI（第 7 檔）失敗；舊檔有日經225 的可辨識舊項、沒有 KOSPI
        let mut f = MapFetch::new();
        for (i, (symbol, _, _)) in QUOTES.iter().enumerate() {
            f = match *symbol {
                "^N225" | "^KS11" => f.status(&quote_url(symbol), 500),
                _ => f.ok(
                    &quote_url(symbol),
                    &chart(100.0 + i as f64, 100.0, 1).to_string(),
                ),
            };
        }
        let prev = PreviousOutput::from_value(json!({"quotes": [
            old_quote("USD/TWD", "fx", 7.0),
            old_quote("日經225", "index", 1.0),
            old_quote("SOFR 3M", "yield", 3.0),
        ]}));
        let out = run_with(&f, &prev);
        let names: Vec<&str> = out.value.iter().map(|q| q.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "USD/TWD",
                "US 10Y",
                "WTI 原油",
                "加權指數",
                "台積電",
                "日經225",
                "歐股50",
                "道瓊",
                "S&P 500",
                "NASDAQ",
                "費半"
            ],
            "日經225 在原位置（第 6 項）、KOSPI 省略、SOFR 不在這裡"
        );
        let n225 = &out.value[5];
        assert_eq!(
            (n225.price, n225.prev, n225.t),
            (1.0, 2.0, Some(1)),
            "原樣沿用舊項"
        );
        assert_eq!(out.value[0].price, 100.0, "沒失敗的檔取新值，不管舊檔");
        assert_eq!(
            out.errors,
            [
                "行情來源失敗（日經225／^N225）：HTTP Error 500: Internal Server Error",
                "行情來源失敗（KOSPI／^KS11）：HTTP Error 500: Internal Server Error",
            ]
        );
        assert_eq!(
            out.logs,
            [
                "[行情] 行情來源失敗（日經225／^N225）：HTTP Error 500: Internal Server Error，跳過",
                "[行情] 日經225 沿用上次資料",
                "[行情] 行情來源失敗（KOSPI／^KS11）：HTTP Error 500: Internal Server Error，跳過",
            ]
        );
        assert!(!out.fresh);
    }

    #[test]
    fn b_qt_8_old_by_name_skips_empty_names_takes_the_last_duplicate_and_drops_bad_items() {
        let prev = PreviousOutput::from_value(json!({"quotes": [
            old_quote("日經225", "index", 1.0),
            old_quote("日經225", "index", 5.0),
            old_quote("", "index", 9.0),
            {"name": "KOSPI", "kind": "bogus", "price": 1},
            "junk",
        ]}));
        let old = old_by_name(&prev);
        assert_eq!(old["日經225"].price, 5.0);
        assert!(!old.contains_key("") && !old.contains_key("KOSPI"));
        // 非陣列＝沒有舊資料
        for bad in [json!("x"), json!(null), json!({"a": 1})] {
            let p = PreviousOutput::from_value(json!({ "quotes": bad }));
            assert!(old_by_name(&p).is_empty());
        }
    }

    #[test]
    fn b_qt_7_every_symbol_failing_without_old_data_yields_nothing_but_twelve_errors() {
        let out = run_with(&MapFetch::new(), &PreviousOutput::default());
        assert!(out.value.is_empty());
        assert_eq!(out.errors.len(), 12);
        assert!(out.errors[0].starts_with("行情來源失敗（USD/TWD／USDTWD=X）："));
        assert!(out.errors[11].starts_with("行情來源失敗（費半／^SOX）："));
        assert_eq!(out.logs.len(), 12, "沒有舊項就沒有沿用日誌");
    }

    #[test]
    fn b_qt_10_sofr_is_not_touched_here_and_previous_excludes_it() {
        let prev = PreviousOutput::from_value(json!({"quotes": [
            old_quote("SOFR 3M", "yield", 3.0),
            old_quote("費半", "index", 4.0),
        ]}));
        let p = previous(&prev);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].name, "費半");
    }

    // ───── D7：panic 隔離 ─────

    #[test]
    fn d7_a_panic_on_every_symbol_reuses_old_quotes_per_symbol_and_records_errors() {
        let prev = PreviousOutput::from_value(json!({"quotes": [
            old_quote("USD/TWD", "fx", 7.0),
            old_quote("費半", "index", 4.0),
        ]}));
        let out = block_on(run_guarded(&PanicFetch, &clock(), &prev));
        assert_eq!(
            out.value
                .iter()
                .map(|q| q.name.as_str())
                .collect::<Vec<_>>(),
            ["USD/TWD", "費半"]
        );
        assert_eq!(out.errors.len(), 12);
        assert!(out.errors[0].starts_with("行情來源失敗（USD/TWD／USDTWD=X）：panic: "));
        assert!(!out.fresh);
    }

    #[test]
    fn d7_a_panic_in_one_symbol_does_not_affect_the_others() {
        // 用一個只對 ^DJI panic 的 Fetch
        struct PanicOnDji(MapFetch);
        impl Fetch for PanicOnDji {
            async fn fetch(
                &self,
                req: &crate::fetch::http::Request,
            ) -> Result<crate::fetch::http::Response, crate::fetch::errors::FetchError>
            {
                if req.url.contains("%5EDJI") {
                    panic!("注入的 panic");
                }
                self.0.fetch(req).await
            }
        }
        let out = block_on(run_guarded(
            &PanicOnDji(all_ok()),
            &clock(),
            &PreviousOutput::default(),
        ));
        assert_eq!(out.value.len(), 11);
        assert!(out.value.iter().all(|q| q.name != "道瓊"));
        assert_eq!(out.errors.len(), 1);
        assert!(out.errors[0].starts_with("行情來源失敗（道瓊／^DJI）：panic: "));
    }

    #[test]
    fn d7_run_guarded_is_transparent_when_nothing_panics() {
        let out = run_with(&all_ok(), &PreviousOutput::default());
        assert_eq!(out.value.len(), 12);
        assert!(out.errors.is_empty() && out.logs.is_empty());
    }

    // ───── oracle ─────

    #[test]
    fn oracle_every_scenario_matches_python_expected_json() {
        oracle::for_each_scenario(|name, sc, expected| {
            let out = block_on(run_guarded(&sc.fetch, &sc.clock, &sc.previous()));
            // expected.quotes ＝ Yahoo 各檔（含沿用）＋最後的 SOFR 3M；本模組負責前者
            let want: Vec<Value> = expected["quotes"]
                .as_array()
                .expect("quotes")
                .iter()
                .filter(|q| q["name"] != sofr::SOFR_NAME)
                .cloned()
                .collect();
            assert_eq!(
                serde_json::to_value(&out.value).expect("序列化"),
                Value::Array(want),
                "情境 {name}：quotes 不符"
            );
            // errors：以 `行情來源失敗（` 開頭、但不是 SOFR 的那些
            let want_errors: Vec<String> = oracle::expected_errors(expected, &["行情來源失敗（"])
                .into_iter()
                .filter(|e| e != sofr::ERR_PREFIX)
                .collect();
            let got: Vec<String> = out
                .errors
                .iter()
                .map(|e| crate::fetch::errors::normalize_error(e))
                .collect();
            assert_eq!(got, want_errors, "情境 {name}：errors 不符");
            assert!(!out.fresh, "情境 {name}：Yahoo 不計 fresh");
        });
    }

    #[test]
    fn oracle_quote_scenarios_exist_and_show_their_key_evidence() {
        let run = |name: &str| {
            let sc = crate::fetch::fixture::Scenario::named(name).expect("情境存在");
            block_on(run_guarded(&sc.fetch, &sc.clock, &sc.previous()))
        };
        let by = |out: &SourceOutcome<Vec<Quote>>, n: &str| {
            out.value.iter().find(|q| q.name == n).cloned()
        };

        // 盤中多一筆即時報價：昨收取前一個「當地日」的收盤
        let out = run("quote-prev-close-intraday-extra-point");
        let usd = by(&out, "USD/TWD").expect("USD/TWD");
        assert_eq!((usd.price, usd.prev), (31.5, 31.4));
        let wti = by(&out, "WTI 原油").expect("WTI");
        assert_eq!((wti.price, wti.prev), (70.5, 70.4));
        let n225 = by(&out, "日經225").expect("日經225");
        assert_eq!(
            (n225.price, n225.prev),
            (101.0, 100.0),
            "gmtoffset 32400 的當地日歸戶"
        );

        // 退回規則
        let out = run("quote-prev-close-fallbacks");
        assert_eq!(by(&out, "US 10Y").expect("US 10Y").prev, 5.0);
        assert_eq!(by(&out, "S&P 500").expect("S&P 500").prev, 101.0);
        assert_eq!(by(&out, "道瓊").expect("道瓊").prev, 40.0);
        assert!(by(&out, "NASDAQ").is_none() && by(&out, "費半").is_none());
        assert_eq!(out.value.len(), 10);
        assert_eq!(
            out.errors,
            [
                "行情來源失敗（NASDAQ／^IXIC）：前一交易日收盤缺值或為 0",
                "行情來源失敗（費半／^SOX）：regularMarketPrice 缺值"
            ]
        );

        // 單檔失敗沿用舊值
        let out = run("quote-single-failure-keeps-old");
        assert_eq!(out.value.len(), 11, "KOSPI 省略");
        let n225 = &out.value[5];
        assert_eq!(
            (n225.name.as_str(), n225.price, n225.prev, n225.t),
            ("日經225", 1.0, 2.0, Some(1))
        );
        assert_eq!(out.errors.len(), 2);
    }
}
