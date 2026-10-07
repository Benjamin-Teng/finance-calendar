//! 市值前百大（`t187ap03_L` 股數 × `STOCK_DAY_ALL` 收盤價；behavior-inventory 4.4／B-TOP-1～7、K-3、K-17；
//! 原始碼 `update_tw_events.py` 的 `fetch_top100`，`:489-518`，與 `main()` 的沿用段 `:1170-1179`）。
//!
//! # 每日只算一次（B-TOP-2、B-TOP-6、K-3）
//!
//! 排名變動極慢，兩個檔合計約 1.6 MB，所以：舊輸出 `top100` 非空**且** `top100_date` 等於台北今天
//! → 直接沿用，**不發任何請求**，`fresh = false`（沒抓到新資料，不可刷新 `fetched`）。否則抓取：
//! 成功＝`date` 設為台北今天、`fresh = true`；失敗＝記 `市值前百大來源失敗：{e}`，沿用舊名單，且
//! **`date` 必須保留舊的 `top100_date`**（全新安裝＝`None`）——若誤設成今天，會被當成今天已算過而整天
//! 不再重試。
//!
//! # 計算（B-TOP-3／4／5，K-17）
//!
//! 先抓 `STOCK_DAY_ALL`（價格表），再抓 `t187ap03_L`（股數）。價格：`ClosingPrice` 以 `str(x or "")`、去逗號、
//! strip、[`py_float`]，`> 0` 才收（`nan` 不收；其餘解析失敗略過）；後面的同代號覆蓋前面。市值：`公司代號`
//! 在價格表內、且 `已發行普通股數或TDR原股發行股數`（strip）的 `isdigit()` 為真，才算
//! `int(股數) × 價格`。**不檢查 `STOCK_DAY_ALL` 的日期單一性**（只有券資比那一版檢查，K-17）。依
//! `(市值, 代號)` **兩者都降冪**排序（同市值時代號字串大的在前，B-TOP-5），取前 [`TOP_N`] 個代號；
//! 一家都算不出來＝失敗（`市值一家都算不出來（來源欄位可能改名）`）。
//!
//! # 給 3.3 的交接
//!
//! [`Top100::codes`] 就是 Python 傳給 `fetch_conference` 的 `top100`（失敗時＝沿用的舊名單）。**名單為空**
//! 時法說會要走備援（B-TOP-7、D-4）——由 3.3／4.1 判斷。
//!
//! # 與 Python 對齊的細節（Python 實測，見測試）
//!
//! - **`isdigit()` 與 `int()` 的落差**：Python 的 `str.isdigit()` 對上標、圈號數字（`²` 等，共 128 個
//!   Unicode 字元）也為真，但 `int()` 只認十進位數字（Nd，含全形）——這種字串在「代號在價格表內」時會讓
//!   整個來源失敗（`ValueError` 不被逐列的 `except ValueError` 接住：那個只包 `float(v)`）。[`NON_DECIMAL_DIGITS`]
//!   由 Python 3.14／Unicode 16 枚舉 `c.isdigit() and not c.isdecimal()` 產生。
//! - 股數轉浮點與 Python `int × float` 一致（整數先轉最近的雙精度）；超過雙精度範圍＝失敗
//!   （`int too large to convert to float`）。
//! - 頂層／每列的形狀失敗規則同 [`super::dividend`]：物件逐鍵、字串逐字元、元素不是物件＝失敗；
//!   `null`／數字＝失敗；空物件／空字串＝空表（接著因「一家都算不出來」失敗）。
//! - 重複代號：價格後者覆蓋前者；股數表重複的列各算一筆（名單可能出現重複代號，Python 同）。
//!
//! # 與 Python 的差異（刻意）
//!
//! - D8-6（NaN／inf 一律視為失敗）：`ClosingPrice` 為 `inf`／`Infinity`（含 `1e400`）的列略過（比照該列
//!   `ValueError`；Python 會收進價格表、讓它的市值變 `inf` 排最前，若股數為 0 則得 `NaN`，排序結果取決於
//!   timsort 的比較順序、無從重現）；市值（股數×價格）算出非有限值（有限價格乘大股數溢位成 `inf`，Python
//!   不報錯）＝整個來源失敗。
//! - 舊輸出 `top100` 逐筆判形狀（非字串元素丟棄）、`top100_date` 非字串＝沒有（D8-3）。
//! - JSON 整數超出 u64 的股數（>1.8e19）會先被解析成浮點而 `isdigit()` 為假＝略過該列；真實來源
//!   最大約 1e11。

use std::collections::HashMap;

use super::clock::Clock;
use super::dates::{digit_value, is_decimal_digit, iso_date};
use super::http::{get_json, Fetch};
use super::json_util::{py_dict, py_float, py_iter, py_str_or_empty, py_strip};
use super::outcome::{guarded, SourceOutcome};
use super::output::PreviousOutput;

/// B-TOP-1：股數（約 1.3 MB）。
pub const URL_BASIC: &str = "https://openapi.twse.com.tw/v1/opendata/t187ap03_L";
/// B-TOP-1：收盤價（約 0.3 MB）。
pub const URL_DAY_ALL: &str = "https://openapi.twse.com.tw/v1/exchangeReport/STOCK_DAY_ALL";
/// 取市值前幾大（`TOP_N`）。
pub const TOP_N: usize = 100;
/// B-FLOW-9：失敗前綴。
pub const ERR_PREFIX: &str = "市值前百大來源失敗：";
/// 一家都算不出來時的訊息（逐字，B-TOP-5）。
pub const MSG_NO_CAPS: &str = "市值一家都算不出來（來源欄位可能改名）";

const KEY_SHARES: &str = "已發行普通股數或TDR原股發行股數";

/// 名單與它對應的台北日期。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Top100 {
    /// 市值由大到小的代號。
    pub codes: Vec<String>,
    /// 輸出 `top100_date`：本輪算出或命中快取＝台北今天；失敗沿用＝舊值（可能是 `None`）。
    pub date: Option<String>,
}

/// 舊輸出的名單與日期（來源失敗時沿用）：`old.top100 or []`、`old.top100_date`。
pub fn previous(prev: &PreviousOutput) -> Top100 {
    Top100 {
        codes: prev.get_items::<String>("top100").unwrap_or_default(),
        date: prev.get::<String>("top100_date"),
    }
}

/// 由組裝端呼叫：[`run`] 加上 panic 隔離（D7）。panic 時沿用舊名單與舊日期。
pub async fn run_guarded<F: Fetch>(
    fetch: &F,
    clock: &Clock,
    prev: &PreviousOutput,
) -> SourceOutcome<Top100> {
    guarded(ERR_PREFIX, previous(prev), run(fetch, clock, prev)).await
}

/// 算前百大（B-TOP-1～7）。`fresh`＝本輪真的抓並算出（快取命中不算，B-FLOW-7）。
async fn run<F: Fetch>(fetch: &F, clock: &Clock, prev: &PreviousOutput) -> SourceOutcome<Top100> {
    let today = iso_date(clock.tpe_date());
    let old = previous(prev);
    if !old.codes.is_empty() && old.date.as_deref() == Some(today.as_str()) {
        return SourceOutcome::new(
            Top100 {
                codes: old.codes,
                date: Some(today),
            },
            false,
        );
    }
    match compute(fetch).await {
        Ok(codes) => SourceOutcome::new(
            Top100 {
                codes,
                date: Some(today),
            },
            true,
        ),
        Err(e) => {
            let mut out = SourceOutcome::new(old, false);
            out.errors.push(format!("{ERR_PREFIX}{e}"));
            out.logs.push("[前百大] 本輪失敗，沿用上次名單".to_string());
            out
        }
    }
}

async fn compute<F: Fetch>(fetch: &F) -> Result<Vec<String>, String> {
    // 順序同 Python：先價格、再股數；價格表失敗時不會請求股數表。
    let day_all = get_json(fetch, URL_DAY_ALL)
        .await
        .map_err(|e| e.to_string())?;
    let mut price: HashMap<String, f64> = HashMap::new();
    for r in py_iter(day_all)? {
        let obj = py_dict(&r)?;
        let raw = py_str_or_empty(obj.get("ClosingPrice")).replace(',', "");
        // `float(v) > 0`：解析失敗（ValueError）與 `nan`、非正數都略過；`inf` 依 D8-6 也略過。
        if let Some(v) = py_float(py_strip(&raw)).filter(|v| v.is_finite() && *v > 0.0) {
            price.insert(py_strip(&py_str_or_empty(obj.get("Code"))).to_string(), v);
        }
    }
    let basic = get_json(fetch, URL_BASIC)
        .await
        .map_err(|e| e.to_string())?;
    let mut caps: Vec<(f64, String)> = Vec::new();
    for b in py_iter(basic)? {
        let obj = py_dict(&b)?;
        let code = py_strip(&py_str_or_empty(obj.get("公司代號"))).to_string();
        let shares = py_strip(&py_str_or_empty(obj.get(KEY_SHARES))).to_string();
        let Some(&px) = price.get(&code) else {
            continue;
        };
        if !py_isdigit(&shares) {
            continue;
        }
        let cap = int_to_f64(&shares)? * px;
        if !cap.is_finite() {
            return Err("市值不是有限值（股數乘收盤價溢位）".to_string());
        }
        caps.push((cap, code));
    }
    if caps.is_empty() {
        return Err(MSG_NO_CAPS.to_string());
    }
    // `caps.sort(reverse=True)`：市值降冪，同市值時代號字串降冪（Rust 的 `str` 比較是位元組序，
    // 與 Python 依碼點比較的結果相同）。元素相等＝完全相同的元組，穩定與否看不出差別。
    caps.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    Ok(caps.into_iter().take(TOP_N).map(|(_, c)| c).collect())
}

/// Python `int(shares)` 再當雙精度用：整串必須是十進位數字（Nd，含全形）。`isdigit()` 為真卻含上標等
/// 非十進位數字＝`ValueError`（整個來源失敗）；超出雙精度＝`OverflowError`。
fn int_to_f64(shares: &str) -> Result<f64, String> {
    let mut ascii = String::with_capacity(shares.len());
    for c in shares.chars() {
        if !is_decimal_digit(c) {
            return Err(format!(
                "invalid literal for int() with base 10: '{shares}'"
            ));
        }
        ascii.push(char::from_digit(digit_value(c), 10).unwrap_or('0'));
    }
    match ascii.parse::<f64>() {
        Ok(v) if v.is_finite() => Ok(v),
        _ => Err("int too large to convert to float".to_string()),
    }
}

/// Python `str.isdigit()`：非空，且每個字元都是十進位數字（Nd）或 [`NON_DECIMAL_DIGITS`] 內的「數字」字元。
fn py_isdigit(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| is_decimal_digit(c) || is_non_decimal_digit(c))
}

fn is_non_decimal_digit(c: char) -> bool {
    let c = c as u32;
    NON_DECIMAL_DIGITS
        .iter()
        .any(|&(lo, hi)| (lo..=hi).contains(&c))
}

/// Python `isdigit()` 為真、但 `isdecimal()` 為假的字元範圍（含頭尾）：上下標、圈號數字、衣索比亞數字等。
/// 由 Python 3.14.6（Unicode 16.0）枚舉 `chr(c).isdigit() and not chr(c).isdecimal()` 產生，共 128 字元。
const NON_DECIMAL_DIGITS: [(u32, u32); 20] = [
    (0xB2, 0xB3),
    (0xB9, 0xB9),
    (0x1369, 0x1371),
    (0x19DA, 0x19DA),
    (0x2070, 0x2070),
    (0x2074, 0x2079),
    (0x2080, 0x2089),
    (0x2460, 0x2468),
    (0x2474, 0x247C),
    (0x2488, 0x2490),
    (0x24EA, 0x24EA),
    (0x24F5, 0x24FD),
    (0x24FF, 0x24FF),
    (0x2776, 0x277E),
    (0x2780, 0x2788),
    (0x278A, 0x2792),
    (0x10A40, 0x10A43),
    (0x10E60, 0x10E68),
    (0x11052, 0x1105A),
    (0x1F100, 0x1F10A),
];

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

    const TODAY: &str = "2026-10-05";

    fn px(code: impl Into<Value>, price: impl Into<Value>) -> Value {
        json!({"Code": code.into(), "ClosingPrice": price.into()})
    }

    fn sh(code: impl Into<Value>, shares: impl Into<Value>) -> Value {
        json!({"公司代號": code.into(), KEY_SHARES: shares.into()})
    }

    fn feed(day: Value, basic: Value) -> MapFetch {
        MapFetch::new()
            .ok(URL_DAY_ALL, &day.to_string())
            .ok(URL_BASIC, &basic.to_string())
    }

    fn run_with(f: &MapFetch, prev: &PreviousOutput) -> SourceOutcome<Top100> {
        block_on(run(f, &clock(), prev))
    }

    fn codes_of(day: Value, basic: Value) -> Result<Vec<String>, Vec<String>> {
        let out = run_with(&feed(day, basic), &PreviousOutput::default());
        if out.fresh {
            assert!(out.errors.is_empty());
            Ok(out.value.codes)
        } else {
            Err(out.errors)
        }
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn old(codes: &[&str], date: Value) -> PreviousOutput {
        PreviousOutput::from_value(json!({"top100": codes, "top100_date": date}))
    }

    // ───── B-TOP-1／2：端點、快取 ─────

    #[test]
    fn b_top_1_urls_and_constants_are_verbatim() {
        assert_eq!(
            URL_BASIC,
            "https://openapi.twse.com.tw/v1/opendata/t187ap03_L"
        );
        assert_eq!(
            URL_DAY_ALL,
            "https://openapi.twse.com.tw/v1/exchangeReport/STOCK_DAY_ALL"
        );
        assert_eq!(TOP_N, 100);
        assert_eq!(ERR_PREFIX, "市值前百大來源失敗：");
        assert_eq!(MSG_NO_CAPS, "市值一家都算不出來（來源欄位可能改名）");
        // 先價格、再股數。
        let f = feed(json!([px("A", "5")]), json!([sh("A", "1")]));
        run_with(&f, &PreviousOutput::default());
        assert_eq!(f.requests(), [URL_DAY_ALL, URL_BASIC]);
    }

    #[test]
    fn b_top_2_same_day_cache_makes_no_request_and_is_not_fresh() {
        let f = MapFetch::new(); // 任何請求都會回 Network 錯誤——但這裡根本不該有請求
        let out = run_with(&f, &old(&["2330", "2317"], json!(TODAY)));
        assert!(f.requests().is_empty(), "快取命中不可發請求");
        assert_eq!(out.value.codes, s(&["2330", "2317"]));
        assert_eq!(out.value.date.as_deref(), Some(TODAY));
        assert!(!out.fresh, "快取命中不算抓到新資料（B-FLOW-7）");
        assert!(out.errors.is_empty() && out.logs.is_empty());
    }

    #[test]
    fn b_top_2_cache_needs_both_a_non_empty_list_and_todays_date() {
        let fresh_feed = || feed(json!([px("A", "5")]), json!([sh("A", "1")]));
        for prev in [
            old(&["1"], json!("2026-10-04")), // 日期是昨天
            old(&["1"], json!("2026-10-06")), // 日期是未來
            old(&[], json!(TODAY)),           // 名單空
            old(&["1"], json!(null)),         // 沒有日期
            old(&["1"], json!(20261005)),     // 日期型別不對
            PreviousOutput::from_value(json!({"top100": ["1"]})),
            PreviousOutput::from_value(json!({"top100_date": TODAY})),
            PreviousOutput::default(),
        ] {
            let f = fresh_feed();
            let out = run_with(&f, &prev);
            assert_eq!(f.requests().len(), 2, "{prev:?} 不應命中快取");
            assert_eq!(out.value.codes, s(&["A"]));
            assert!(out.fresh);
        }
    }

    #[test]
    fn b_top_2_the_date_is_the_taipei_date_not_the_local_one() {
        // 台北 10/05 00:30、本機（日本）已是 10/05 01:30 ——另一天的情況：台北 10/04 23:30。
        let c = Clock::new(
            datetime!(2026-10-04 23:30:00 +8),
            datetime!(2026-10-05 00:30:00),
        );
        let f = MapFetch::new();
        let out = block_on(run(&f, &c, &old(&["1"], json!("2026-10-04"))));
        assert!(f.requests().is_empty());
        assert_eq!(out.value.date.as_deref(), Some("2026-10-04"));
    }

    // ───── B-TOP-6：成功／失敗後的日期 ─────

    #[test]
    fn b_top_6_success_sets_todays_date_and_is_fresh() {
        let out = run_with(
            &feed(json!([px("A", "5")]), json!([sh("A", "1")])),
            &old(&["OLD"], json!("2026-10-04")),
        );
        assert_eq!(out.value.codes, s(&["A"]));
        assert_eq!(out.value.date.as_deref(), Some(TODAY));
        assert!(out.fresh && out.errors.is_empty() && out.logs.is_empty());
    }

    #[test]
    fn b_top_6_failure_keeps_the_old_list_and_the_old_date() {
        let f = MapFetch::new()
            .ok(URL_DAY_ALL, &json!([px("A", "5")]).to_string())
            .status(URL_BASIC, 500);
        let out = run_with(&f, &old(&["2330", "2308"], json!("2026-10-04")));
        assert_eq!(out.value.codes, s(&["2330", "2308"]));
        assert_eq!(
            out.value.date.as_deref(),
            Some("2026-10-04"),
            "失敗時不可把日期改成今天"
        );
        assert!(!out.fresh);
        assert_eq!(
            out.errors,
            ["市值前百大來源失敗：HTTP Error 500: Internal Server Error"]
        );
        assert_eq!(out.logs, ["[前百大] 本輪失敗，沿用上次名單"]);
    }

    #[test]
    fn b_top_6_fresh_install_failure_is_an_empty_list_and_a_null_date() {
        let f = MapFetch::new().status(URL_DAY_ALL, 503);
        let out = run_with(&f, &PreviousOutput::default());
        assert!(out.value.codes.is_empty());
        assert_eq!(out.value.date, None);
        assert!(!out.fresh);
        assert_eq!(f.requests(), [URL_DAY_ALL], "價格表失敗不會請求股數表");
    }

    #[test]
    fn b_top_6_failure_with_a_wrong_typed_old_entry_treats_it_as_absent() {
        // D8-3：舊值型別不對＝沒有（deliberate-old-wrong-types 的單元版）。
        let prev = PreviousOutput::from_value(json!({"top100": 7, "top100_date": 20261004}));
        let out = run_with(&MapFetch::new().status(URL_DAY_ALL, 500), &prev);
        assert!(out.value.codes.is_empty());
        assert_eq!(out.value.date, None);
        // 名單逐筆判形狀：非字串元素丟棄。
        let prev = PreviousOutput::from_value(
            json!({"top100": ["1", 2, null, "3"], "top100_date": "2026-10-04"}),
        );
        let out = run_with(&MapFetch::new().status(URL_DAY_ALL, 500), &prev);
        assert_eq!(out.value.codes, s(&["1", "3"]));
        assert_eq!(out.value.date.as_deref(), Some("2026-10-04"));
    }

    #[test]
    fn failure_cases_also_never_mark_the_date_even_when_the_old_date_is_today() {
        // 名單空但日期是今天（不命中快取）→ 抓取失敗 → 沿用空名單與「今天」這個舊日期（照 Python）。
        let out = run_with(
            &MapFetch::new().status(URL_DAY_ALL, 500),
            &old(&[], json!(TODAY)),
        );
        assert!(out.value.codes.is_empty());
        assert_eq!(out.value.date.as_deref(), Some(TODAY));
    }

    // ───── B-TOP-3／4／5：計算 ─────

    #[test]
    fn b_top_5_sorts_by_market_cap_descending() {
        let got = codes_of(
            json!([px("A", "10"), px("B", "10"), px("C", "5")]),
            json!([sh("A", "100"), sh("B", "100"), sh("C", "1000")]),
        );
        assert_eq!(got, Ok(s(&["C", "B", "A"]))); // C=5000、B 與 A=1000（同市值代號降冪）
    }

    #[test]
    fn b_top_5_equal_market_cap_breaks_ties_by_code_descending() {
        let got = codes_of(
            json!([px("2330", "10"), px("2317", "10"), px("9999", "10")]),
            json!([sh("2330", "100"), sh("2317", "100"), sh("9999", "100")]),
        );
        assert_eq!(got, Ok(s(&["9999", "2330", "2317"])));
        // 字串比較、不是數字比較：「9」> 「10」。
        let got = codes_of(
            json!([px("9", "1"), px("10", "1"), px("100", "1")]),
            json!([sh("9", "1"), sh("10", "1"), sh("100", "1")]),
        );
        assert_eq!(got, Ok(s(&["9", "100", "10"])));
    }

    #[test]
    fn b_top_5_only_the_first_100_are_kept() {
        let day: Vec<Value> = (0..120).map(|i| px(format!("C{i:03}"), "1")).collect();
        let basic: Vec<Value> = (0..120)
            .map(|i| sh(format!("C{i:03}"), (i + 1).to_string()))
            .collect();
        let got = codes_of(json!(day), json!(basic)).unwrap();
        assert_eq!(got.len(), 100);
        assert_eq!(got[0], "C119");
        assert_eq!(got[99], "C020");
    }

    #[test]
    fn b_top_5_empty_caps_is_a_failure_with_the_verbatim_message() {
        for (day, basic) in [
            (json!([px("A", "5")]), json!([sh("Z", "1")])), // 代號對不上
            (json!([]), json!([sh("A", "1")])),             // 沒有價格
            (json!({}), json!([sh("A", "1")])),             // 空物件＝空表
            (json!([px("A", "5")]), json!([])),             // 沒有股數
            (json!([px("A", "5")]), json!([sh("A", "")])),  // 股數空
            (json!([px("A", "5")]), json!([sh("A", Value::Null)])),
            (json!([px("A", "5")]), json!([sh("A", "12.5")])), // 小數不是 isdigit
            (json!([px("A", "5")]), json!([sh("A", 12.0)])),   // 浮點取 str() 為 "12.0"
            (json!([px("A", "5")]), json!([sh("Z", "²")])),    // 代號不在價格表：短路、不碰 int()
        ] {
            assert_eq!(
                codes_of(day.clone(), basic.clone()),
                Err(vec![format!("{ERR_PREFIX}{MSG_NO_CAPS}")]),
                "{day} / {basic}"
            );
        }
    }

    #[test]
    fn b_top_3_price_rules() {
        // 去逗號、非正數／破折號／空／null／0 都不收。
        let codes = ["A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L"];
        let got = codes_of(
            json!([
                px("A", "1,000.5"),
                px("B", "0"),
                px("C", "-1"),
                px("D", "--"),
                px("E", ""),
                px("F", Value::Null),
                px("G", 0),
                px("H", "5"),
                px("I", 12.5),
                px("J", "1_0"),
                px("K", "\u{ff11}\u{ff10}"),
                px("L", "nan"),
            ]),
            json!(codes.map(|c| sh(c, "1"))),
        )
        .unwrap();
        // 收：A=1000.5、H=5、I=12.5、J=10、K=10。排序：A、I、K、J（同為 10，代號降冪 K>J）、H。
        assert_eq!(got, s(&["A", "I", "K", "J", "H"]));
    }

    #[test]
    fn b_top_3_duplicate_codes() {
        // 價格後者覆蓋前者；股數表重複的列各算一筆（名單會有重複代號）。
        let got = codes_of(
            json!([px("A", "5"), px("A", "7")]),
            json!([sh("A", "1"), sh("A", "3")]),
        );
        assert_eq!(got, Ok(s(&["A", "A"])));
        let got = codes_of(
            json!([px("A", "5"), px("A", "7"), px("B", "6")]),
            json!([sh("A", "1"), sh("B", "1")]),
        );
        assert_eq!(got, Ok(s(&["A", "B"])), "A 的價格是 7（後者覆蓋）");
    }

    #[test]
    fn b_top_4_code_and_shares_fields_follow_python_str_and_strip() {
        let got = codes_of(
            json!([px(" A ", "5"), px(2330, "5")]),
            json!([sh("A\t", "1"), sh(2330, "2")]),
        );
        assert_eq!(got, Ok(s(&["2330", "A"])));
        // 股數：strip 後才判斷；整數／全形數字可、前後空白可、含空白或小數點不可。
        let got = codes_of(
            json!([
                px("A", "1"),
                px("B", "1"),
                px("C", "1"),
                px("D", "1"),
                px("E", "1")
            ]),
            json!([
                sh("A", " 7 "),
                sh("B", "\u{ff11}\u{ff12}"),
                sh("C", 5),
                sh("D", "1 2"),
                sh("E", "-3")
            ]),
        );
        assert_eq!(got, Ok(s(&["B", "A", "C"])));
        // 股數 0 也算（市值 0）。
        assert_eq!(
            codes_of(json!([px("A", "5")]), json!([sh("A", "0")])),
            Ok(s(&["A"]))
        );
    }

    #[test]
    fn b_top_4_shares_beyond_u64_use_float_conversion_like_python() {
        let big = format!("1{}", "0".repeat(29));
        let bigger = format!("1{}1", "0".repeat(29));
        let got = codes_of(
            json!([px("A", "5"), px("B", "5")]),
            json!([sh("A", big), sh("B", bigger)]),
        );
        assert_eq!(got, Ok(s(&["B", "A"])));
        // 超出雙精度：Python `int too large to convert to float` → 整個來源失敗。
        let huge = "9".repeat(400);
        let err = codes_of(json!([px("A", "5")]), json!([sh("A", huge)])).unwrap_err();
        assert!(err[0].starts_with(ERR_PREFIX) && err[0].contains("too large"));
        // 價格極大、乘積溢位成 inf：Python 不報錯（inf 排最前），D8-6 視為整個來源失敗。
        let err = codes_of(
            json!([px("A", "1e308"), px("B", "5")]),
            json!([sh("A", "99"), sh("B", "99")]),
        )
        .unwrap_err();
        assert!(err[0].starts_with(ERR_PREFIX) && err[0].contains("不是有限值"));
    }

    #[test]
    fn isdigit_but_not_decimal_in_a_priced_row_fails_the_whole_source() {
        // Python：`"²".isdigit()` 為真、`int("²")` 是 ValueError，不被逐列的 `except ValueError`（只包 `float(v)`）接住。
        for shares in ["²", "①", "１²", "⁰", "₅"] {
            let err = codes_of(
                json!([px("A", "5"), px("B", "5")]),
                json!([sh("B", "1"), sh("A", shares)]),
            )
            .unwrap_err();
            assert!(
                err[0].starts_with(ERR_PREFIX) && err[0].contains("invalid literal for int()"),
                "{shares}：{err:?}"
            );
        }
    }

    #[test]
    fn non_decimal_digit_table_matches_python_isdigit() {
        // 範圍數與 Python 3.14.6 / Unicode 16.0 枚舉的 128 個字元一致。
        let n: u32 = NON_DECIMAL_DIGITS.iter().map(|(a, b)| b - a + 1).sum();
        assert_eq!(n, 128);
        for c in [
            '²', '³', '¹', '⁰', '⁴', '₀', '₉', '①', '⑨', '⓪', '❶', '➀', '🄀',
        ] {
            assert!(py_isdigit(&c.to_string()), "{c:?}");
        }
        for c in ['0', '٣', '１', 'a', ' ', '.', '⑩', '½', '一'] {
            let is_decimal = is_decimal_digit(c);
            assert_eq!(py_isdigit(&c.to_string()), is_decimal, "{c:?}");
        }
        assert!(!py_isdigit(""));
        assert!(!py_isdigit("12a"));
    }

    #[test]
    fn d8_6_infinite_price_rows_are_skipped_and_infinite_caps_fail_the_source() {
        // Python 會把 `inf` 價格收進表、市值 inf 排最前（`['A','B']`）；D8-6：該列略過。
        for bad in ["inf", "+inf", "Infinity", "1e400", "-inf", "nan"] {
            let got = codes_of(
                json!([px("A", bad), px("B", "5")]),
                json!([sh("A", "1"), sh("B", "1")]),
            );
            assert_eq!(got, Ok(s(&["B"])), "{bad}");
        }
        // 股數 0 × inf：價格列已被略過，不再有 NaN；整份只剩 inf 價格＝一家都算不出來。
        let err = codes_of(json!([px("A", "inf")]), json!([sh("A", "0")])).unwrap_err();
        assert_eq!(err, [format!("{ERR_PREFIX}{MSG_NO_CAPS}")]);
        // 同代號後面一列是 inf：該列略過，保留前一列的有限價格。
        let got = codes_of(json!([px("A", "5"), px("A", "inf")]), json!([sh("A", "1")]));
        assert_eq!(got, Ok(s(&["A"])));
    }

    // ───── 形狀（Python 實測） ─────

    #[test]
    fn shapes_that_python_treats_as_failure() {
        let bad_day = [
            json!(null),
            json!(5),
            json!(true),
            json!({"a": 1}), // 物件逐鍵 → 字串列 `.get`
            json!("abc"),
            json!([5]),
            json!([null]),
            json!([[1]]),
        ];
        for body in bad_day {
            let err = codes_of(body.clone(), json!([sh("A", "1")])).unwrap_err();
            assert!(err[0].starts_with(ERR_PREFIX), "{body}");
            assert_ne!(err[0], format!("{ERR_PREFIX}{MSG_NO_CAPS}"), "{body}");
        }
        for body in [
            json!(null),
            json!(5),
            json!({"a": 1}),
            json!([5]),
            json!([null]),
        ] {
            let err = codes_of(json!([px("A", "5")]), body.clone()).unwrap_err();
            assert!(err[0].starts_with(ERR_PREFIX), "{body}");
            assert_ne!(err[0], format!("{ERR_PREFIX}{MSG_NO_CAPS}"), "{body}");
        }
    }

    #[test]
    fn transport_failures_of_either_endpoint_fail_the_source() {
        let day_ok = json!([px("A", "5")]).to_string();
        for f in [
            MapFetch::new().status(URL_DAY_ALL, 500),
            MapFetch::new().network_error(URL_DAY_ALL, "boom"),
            MapFetch::new().ok(URL_DAY_ALL, "{ not json"),
            MapFetch::new()
                .ok(URL_DAY_ALL, &day_ok)
                .status(URL_BASIC, 404),
            MapFetch::new()
                .ok(URL_DAY_ALL, &day_ok)
                .ok(URL_BASIC, "\u{feff}[]"),
            MapFetch::new()
                .ok(URL_DAY_ALL, &day_ok)
                .network_error(URL_BASIC, "x"),
        ] {
            let out = run_with(&f, &old(&["9"], json!("2026-10-04")));
            assert_eq!(out.errors.len(), 1);
            assert!(out.errors[0].starts_with(ERR_PREFIX));
            assert_eq!(out.value.codes, s(&["9"]));
            assert_eq!(out.value.date.as_deref(), Some("2026-10-04"));
            assert!(!out.fresh);
        }
    }

    // ───── D7：panic ─────

    #[test]
    fn d7_a_panic_inside_the_source_keeps_the_old_list_and_the_old_date() {
        let prev = old(&["2330", "2308"], json!("2026-10-04"));
        let out = block_on(run_guarded(&PanicFetch, &clock(), &prev));
        assert_eq!(out.value.codes, s(&["2330", "2308"]));
        assert_eq!(out.value.date.as_deref(), Some("2026-10-04"));
        assert!(!out.fresh);
        assert!(out.errors[0].starts_with("市值前百大來源失敗：panic: "));
        assert_eq!(
            crate::fetch::errors::normalize_error(&out.errors[0]),
            ERR_PREFIX
        );
        // 全新安裝 panic：空名單、null 日期。
        let out = block_on(run_guarded(
            &PanicFetch,
            &clock(),
            &PreviousOutput::default(),
        ));
        assert!(out.value.codes.is_empty() && out.value.date.is_none());
    }

    #[test]
    fn d7_run_guarded_is_transparent_when_nothing_panics() {
        let f = feed(json!([px("A", "5")]), json!([sh("A", "1")]));
        let out = block_on(run_guarded(&f, &clock(), &PreviousOutput::default()));
        assert_eq!(out.value.codes, s(&["A"]));
        assert!(out.fresh);
    }

    // ───── oracle ─────

    #[test]
    fn oracle_every_scenario_matches_python_expected_json() {
        oracle::for_each_scenario(|name, sc, expected| {
            let out = block_on(run_guarded(&sc.fetch, &sc.clock, &sc.previous()));
            let want_codes: Vec<String> = expected["top100"]
                .as_array()
                .expect("expected.top100 是陣列")
                .iter()
                .map(|v| v.as_str().expect("代號是字串").to_string())
                .collect();
            assert_eq!(out.value.codes, want_codes, "情境 {name}：top100 不符");
            assert_eq!(
                serde_json::to_value(&out.value.date).unwrap(),
                expected["top100_date"],
                "情境 {name}：top100_date 不符"
            );
            oracle::assert_errors(name, &out.errors, expected, &[ERR_PREFIX]);
        });
    }

    #[test]
    fn oracle_cache_hit_scenarios_make_no_request_to_either_endpoint() {
        // 兩個快取命中情境（同日快取、命中但其他來源全失敗）：t187ap03_L／STOCK_DAY_ALL 一個請求都不能有。
        for name in ["top100-same-day-cache", "top100-cache-hit-not-fresh"] {
            let sc = crate::fetch::fixture::Scenario::named(name).expect("載入情境");
            let out = block_on(run_guarded(&sc.fetch, &sc.clock, &sc.previous()));
            assert!(!out.fresh, "{name}：快取命中不算 fresh");
            assert_eq!(sc.fetch.request_count_containing("t187ap03_L"), 0, "{name}");
            assert_eq!(
                sc.fetch.request_count_containing("STOCK_DAY_ALL"),
                0,
                "{name}"
            );
            assert!(sc.fetch.requests().is_empty(), "{name}");
            assert_eq!(out.value.date.as_deref(), Some(TODAY), "{name}");
        }
    }

    #[test]
    fn oracle_fresh_scenarios_do_request_both_endpoints() {
        // 對照組：沒有快取的情境一定會打兩個端點，證明上面的「零請求」不是假陽性。
        let sc = crate::fetch::fixture::Scenario::named("recorded-20261005").expect("載入情境");
        let out = block_on(run_guarded(&sc.fetch, &sc.clock, &sc.previous()));
        assert!(out.fresh);
        assert_eq!(sc.fetch.request_count_containing("t187ap03_L"), 1);
        assert_eq!(sc.fetch.request_count_containing("STOCK_DAY_ALL"), 1);
        assert_eq!(out.value.codes.len(), TOP_N);
    }
}
