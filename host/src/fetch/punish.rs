//! 處置股（上市 TWSE openapi＋上櫃 TPEx openapi；behavior-inventory 4.7／B-PUN-1～11、K-8、B-DATE-5、
//! B-FLOW-6／7／9／18；原始碼 `update_tw_events.py` 的 `fetch_punish`、`parse_disposition_period`、
//! `punish_times`、`is_stock_or_etf`，`:592-701`）。
//!
//! # 來源語意
//!
//! - 兩個端點**各自獨立**成功或失敗（B-PUN-6）：`處置股來源失敗（上市）：{e}`／`處置股來源失敗（上櫃）：{e}`，
//!   不阻斷對方。回應先 `or []`（假值＝空）再逐列處理：代號 `str().strip()` 後只收股票／ETF
//!   （[`is_stock_or_etf`]，B-PUN-2）；期間 [`parse_disposition_period`] 解析不了就**靜默略過**（B-PUN-3、
//!   B-DATE-5；TPEx 沒資料時回的「單筆全空白樣板列」就是被代號過濾擋掉，B-PUN-4，不算錯誤）。
//! - **失敗沿用（按 market 分段，B-PUN-7／B-FLOW-6）**：上市失敗＝併入舊輸出 `market == "上市"` 的列，上櫃同理；
//!   沒有舊列就不記沿用日誌。列的串接順序是 `上市新抓 ＋ 上櫃新抓 ＋（上市失敗時）舊上市 ＋（上櫃失敗時）舊上櫃`，
//!   這個順序在「同 code、`(end, start)` 完全相同」時決定誰勝（先到先留），要照 Python。
//! - **二度處置（K-8／B-PUN-8）**：同 `code`（跨市場）只留 `(end, start)` 字串比較最大者，`times` 取保留列自己
//!   的值；輸出依 `code` 字串升冪（B-PUN-9）。不套 14 天窗口；不計 `fresh`（B-FLOW-7，B-PUN-10）。
//! - **中途失敗的殘留列**（Python 實測）：迴圈跑到一半遇到非物件的元素會丟例外，但**先前已收的列仍留在
//!   `rows`**，之後再接上舊列的沿用，所以輸出同時含「新抓到的前半」與「舊列」。本模組照做（同 3.2 B-DIV-6）。
//!
//! # 與 Python 的差異
//!
//! 1. **`times` 是 `i64`，接受 Python `int()` 接受的任何整數**（含負數 `"-2"`、`99999999999`）並原樣輸出。
//!    **唯一殘留差異**：超出 `i64` 的整數（Python 任意精度、Rust 無法表示）視為解析失敗＝`1`；真實來源是個位數的
//!    累計次數。
//! 2. **舊輸出的列逐筆判形狀**（D8-3）：型別不對的舊列丟棄（Python 在 `r.get` 會崩潰）；舊輸出 `punish` 不是
//!    陣列＝沒有舊資料。
//! 3. 每個端點的呼叫另加 `catch_unwind`（D7）：段內 panic＝該段失敗（記 `…：panic: …`、改用舊列），另一段照常。
//! 4. `name` 取自非字串欄位時（真實來源一律字串）經共用的 `json_util::py_str` 近似：浮點用 Rust 的最短表示
//!    （`1e16`、`1e-5`，Python 是 `1e+16`、`1e-05`）、物件／陣列用 JSON 文字而非 `repr`。代號不受影響（這類值
//!    不會通過代號 regex）。
//!
//! 其餘與 Python 逐項實測對齊（`testdata/punish_python_cases.json` 由真的 `fetch_punish` 產生，含 139 個
//! 期間／`times`／代號／形狀變體），包括「Python 不崩潰但看似怪異」的行為：`parse_disposition_period` 把
//! 全形數字與阿拉伯數字當數字、兩段內的 `/` 全部拿掉（含全形斜線以外的位置）、`~` 後面還有 `~` 就失敗。

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value};
use time::{Date, Month};

use super::clock::Clock;
use super::dates::{digit_value, is_decimal_digit, iso_date};
use super::http::{get_json, Fetch};
use super::json_util::{py_dict, py_iter, py_str, py_str_get, py_strip, truthy};
use super::outcome::{guarded, SourceOutcome};
use super::output::{PreviousOutput, Punish};

/// B-PUN-1：上市端點。
pub const URL_PUNISH_TWSE: &str = "https://openapi.twse.com.tw/v1/announcement/punish";
/// B-PUN-1：上櫃端點（不吃日期參數，固定 snapshot，B-PUN-4）。
pub const URL_PUNISH_TPEX: &str = "https://www.tpex.org.tw/openapi/v1/tpex_disposal_information";
/// B-FLOW-9：失敗前綴（逐字）。
pub const ERR_PREFIX_TWSE: &str = "處置股來源失敗（上市）：";
pub const ERR_PREFIX_TPEX: &str = "處置股來源失敗（上櫃）：";
/// 外層（兩段之外，即合併步驟）panic 時的錯誤前綴。
const ERR_PREFIX_ALL: &str = "處置股來源失敗：";
/// B-PUN-11：`market` 值（前端 U-4 依賴「上櫃」字樣）。
pub const MARKET_TWSE: &str = "上市";
pub const MARKET_TPEX: &str = "上櫃";

/// B-PUN-2：`RE_STOCK_CODE = ^\d{4}[A-Z]?$`。Python 的 `$` 也配對「結尾換行之前」，Rust 的 `$` 只配對文字結尾，
/// 所以多一個 `\n?`（等價；呼叫端傳進來的代號已 `strip()`，實際不會有結尾換行）。`\d` 兩邊都是 Unicode Nd。
const RE_STOCK_CODE: &str = r"^\d{4}[A-Z]?\n?$";
/// B-PUN-2：`RE_ETF_CODE = ^00\d{2,4}$`（同上處理結尾換行）。
const RE_ETF_CODE: &str = r"^00\d{2,4}\n?$";

static RX_STOCK_CODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(RE_STOCK_CODE).expect("固定的 regex 必能編譯"));
static RX_ETF_CODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(RE_ETF_CODE).expect("固定的 regex 必能編譯"));

/// 只收股票／ETF 代號，濾掉權證、可轉債等衍生商品（含字母的 ETF 如 `00981A` 照 Python 現況被擋）。
pub fn is_stock_or_etf(code: &str) -> bool {
    RX_STOCK_CODE.is_match(code) || RX_ETF_CODE.is_match(code)
}

/// 去掉所有 `/` 後恰 7 個十進位數字（Python：`len(digits) == 7 and digits.isdigit()`，再 `int()`；`isdigit()` 為真
/// 但 `int()` 失敗的上標數字最後也是 `ValueError`＝`None`，所以等價於「7 個 Nd」）→ 西元 ISO 日期。
fn roc7_to_iso(part: &str) -> Option<String> {
    let digits: Vec<char> = py_strip(part).chars().filter(|&c| c != '/').collect();
    if digits.len() != 7 || !digits.iter().all(|&c| is_decimal_digit(c)) {
        return None;
    }
    let num = |range: std::ops::Range<usize>| {
        digits[range]
            .iter()
            .fold(0i32, |acc, &c| acc * 10 + digit_value(c) as i32)
    };
    let year = num(0..3) + 1911;
    let month = Month::try_from(u8::try_from(num(3..5)).ok()?).ok()?;
    let day = u8::try_from(num(5..7)).ok()?;
    Some(iso_date(Date::from_calendar_date(year, month, day).ok()?))
}

/// 處置期間 → `(start_iso, end_iso)`（B-PUN-3）。假值（`null`、`""`、`0`、`false`、空容器）＝`None`；其餘
/// `str().strip()` 後：含全形 `～`（U+FF5E）以第一個切成兩段，否則含半形 `~` 同樣處理，否則 `None`；
/// 兩段各 `strip()`、拿掉全部 `/`、須恰 7 個數字、年＋1911 組日期（不合法→`None`）。
pub fn parse_disposition_period(raw: Option<&Value>) -> Option<(String, String)> {
    let raw = raw.filter(|v| truthy(v))?;
    let s = py_str(raw);
    let s = py_strip(&s);
    let (a, b) = s.split_once('\u{ff5e}').or_else(|| s.split_once('~'))?;
    Some((roc7_to_iso(a)?, roc7_to_iso(b)?))
}

/// Python `int(str)`（呼叫端已 `strip()`）：可選正負號、十進位數字（含 Unicode Nd）、底線只准夾在兩個數字之間。
/// 回傳 `None`＝`ValueError`（超過 i128 的巨大數字也當 `None`，反正會落在 `i64` 之外）。
fn py_int(s: &str) -> Option<i128> {
    let (neg, body) = match s.chars().next()? {
        '+' => (false, &s[1..]),
        '-' => (true, &s[1..]),
        _ => (false, s),
    };
    let chars: Vec<char> = body.chars().collect();
    if chars.is_empty() {
        return None;
    }
    let mut value: i128 = 0;
    for (i, &c) in chars.iter().enumerate() {
        if c == '_' {
            let between = i > 0
                && is_decimal_digit(chars[i - 1])
                && chars.get(i + 1).is_some_and(|&n| is_decimal_digit(n));
            if !between {
                return None;
            }
        } else if is_decimal_digit(c) {
            value = value
                .checked_mul(10)?
                .checked_add(i128::from(digit_value(c)))?;
        } else {
            return None;
        }
    }
    Some(if neg { -value } else { value })
}

/// 累計處置次數（B-PUN-5）：`int(str(raw).strip())`，解析失敗（含欄位缺漏＝`str(None)`）一律 `1`。
/// 超出 `i64` 的值（唯一殘留差異）見模組文件「與 Python 的差異」第 1 點。
pub fn punish_times(raw: Option<&Value>) -> i64 {
    let text = raw.map_or_else(|| "None".to_string(), py_str);
    py_int(py_strip(&text))
        .and_then(|v| i64::try_from(v).ok())
        .unwrap_or(1)
}

/// 單一端點的結果。`rows` 在失敗時**仍可能有前半段已收的列**（見模組文件）；`ok` 對應 Python 的
/// `twse_ok`／`tpex_ok`。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Segment {
    rows: Vec<Punish>,
    ok: bool,
}

impl Segment {
    fn failed() -> Self {
        Segment {
            rows: Vec::new(),
            ok: false,
        }
    }
}

/// 逐列解析一個端點的回應，收進 `rows`（B-PUN-2、3、5、11）。形狀不符時回 `Err`，**已收進 `rows` 的列保留**。
fn parse_rows(
    json: Value,
    code_key: &str,
    name_key: &str,
    market: &str,
    rows: &mut Vec<Punish>,
) -> Result<(), String> {
    // `http_json(...) or []`
    let items = if truthy(&json) {
        py_iter(json)?
    } else {
        Vec::new()
    };
    for item in items {
        let obj: &Map<String, Value> = py_dict(&item)?;
        let code = py_strip(&py_str_get(obj, code_key)).to_string();
        if !is_stock_or_etf(&code) {
            continue;
        }
        let Some((start, end)) = parse_disposition_period(obj.get("DispositionPeriod")) else {
            continue;
        };
        rows.push(Punish {
            code,
            name: py_strip(&py_str_get(obj, name_key)).to_string(),
            start,
            end,
            times: punish_times(obj.get("NumberOfAnnouncement")),
            market: market.to_string(),
        });
    }
    Ok(())
}

/// 抓一個端點；失敗記 `{prefix}{e}`（不含沿用，沿用在組裝時依 market 統一處理）。
async fn fetch_segment<F: Fetch>(
    fetch: &F,
    url: &str,
    prefix: &str,
    code_key: &str,
    name_key: &str,
    market: &str,
) -> SourceOutcome<Segment> {
    let mut rows = Vec::new();
    let result = match get_json(fetch, url).await {
        Ok(json) => parse_rows(json, code_key, name_key, market, &mut rows),
        Err(e) => Err(e.to_string()),
    };
    let ok = result.is_ok();
    let mut out = SourceOutcome::new(Segment { rows, ok }, false);
    if let Err(e) = result {
        out.errors.push(format!("{prefix}{e}"));
    }
    out
}

/// 舊輸出的處置股列（逐筆判形狀；`punish` 不是陣列＝沒有舊資料）。
fn old_rows(prev: &PreviousOutput) -> Vec<Punish> {
    prev.get_items::<Punish>("punish").unwrap_or_default()
}

/// 串接（新抓＋沿用）、依 code 去重（`(end, start)` 最大者勝、平手先到先留）、依 code 排序（B-PUN-7～9）。
/// `logs` 收沿用日誌（有沿用的列才記）。
fn assemble(twse: Segment, tpex: Segment, old: &[Punish], logs: &mut Vec<String>) -> Vec<Punish> {
    #[cfg(test)]
    if test_hooks::PANIC_IN_ASSEMBLE.with(std::cell::Cell::get) {
        panic!("注入的 assemble panic");
    }
    let mut rows = twse.rows;
    rows.extend(tpex.rows);
    for (ok, market) in [(twse.ok, MARKET_TWSE), (tpex.ok, MARKET_TPEX)] {
        if ok {
            continue;
        }
        let fallback: Vec<&Punish> = old.iter().filter(|r| r.market == market).collect();
        if !fallback.is_empty() {
            logs.push(format!(
                "[處置股] {market}沿用上次資料（{} 筆）",
                fallback.len()
            ));
        }
        rows.extend(fallback.into_iter().cloned());
    }
    let mut best: BTreeMap<String, Punish> = BTreeMap::new();
    for r in rows {
        match best.get(&r.code) {
            Some(cur) if (&r.end, &r.start) <= (&cur.end, &cur.start) => {}
            _ => {
                best.insert(r.code.clone(), r);
            }
        }
    }
    best.into_values().collect()
}

/// 來源整個失敗（兩段都沒抓到）時沿用的值：舊輸出的上市＋上櫃列，經同一套去重排序。僅測試用（組裝端不需要：
/// [`run_guarded`] 已自行沿用）。
#[cfg(test)]
pub fn previous(prev: &PreviousOutput) -> Vec<Punish> {
    assemble(
        Segment::failed(),
        Segment::failed(),
        &old_rows(prev),
        &mut Vec::new(),
    )
}

/// 由組裝端呼叫：兩段各自 panic 隔離，外層再包一層 `guarded` 把 `assemble` 也納入來源層防線（D7：每個來源含
/// 解析各自隔離）。結果 `fresh` 恆為 `false`（B-PUN-10、B-FLOW-7）。
///
/// 外層 panic（只可能出在 `assemble`／`old_rows`）時沿用舊輸出的上市＋上櫃列（與 `previous(prev)` 一致、丟掉
/// 其他市場；舊輸出本來就是
/// 去重排序後的結果，不必再過 `assemble`——fallback 本身不能再有 panic 的機會），錯誤記
/// `處置股來源失敗：panic: …`。
pub async fn run_guarded<F: Fetch>(
    fetch: &F,
    clock: &Clock,
    prev: &PreviousOutput,
) -> SourceOutcome<Vec<Punish>> {
    let fallback = old_rows(prev)
        .into_iter()
        .filter(|r| r.market == MARKET_TWSE || r.market == MARKET_TPEX)
        .collect();
    guarded(ERR_PREFIX_ALL, fallback, run(fetch, clock, prev)).await
}

async fn run<F: Fetch>(
    fetch: &F,
    _clock: &Clock,
    prev: &PreviousOutput,
) -> SourceOutcome<Vec<Punish>> {
    let twse = guarded(
        ERR_PREFIX_TWSE,
        Segment::failed(),
        fetch_segment(
            fetch,
            URL_PUNISH_TWSE,
            ERR_PREFIX_TWSE,
            "Code",
            "Name",
            MARKET_TWSE,
        ),
    )
    .await;
    let tpex = guarded(
        ERR_PREFIX_TPEX,
        Segment::failed(),
        fetch_segment(
            fetch,
            URL_PUNISH_TPEX,
            ERR_PREFIX_TPEX,
            "SecuritiesCompanyCode",
            "CompanyName",
            MARKET_TPEX,
        ),
    )
    .await;
    let mut errors = twse.errors;
    errors.extend(tpex.errors);
    let mut logs = twse.logs;
    logs.extend(tpex.logs);
    let value = assemble(twse.value, tpex.value, &old_rows(prev), &mut logs);
    SourceOutcome {
        value,
        errors,
        fresh: false,
        logs,
    }
}

/// 僅測試：讓 `assemble` 在本執行緒 panic（驗 D7 外層防線）。
#[cfg(test)]
mod test_hooks {
    use std::cell::Cell;
    thread_local! {
        pub static PANIC_IN_ASSEMBLE: Cell<bool> = const { Cell::new(false) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::errors::normalize_error;
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

    const P: &str = "115/10/05\u{ff5e}115/10/12";
    const Q: &str = "1151005~1151012";

    fn tw(code: &str, period: &str, name: &str, times: Value) -> Value {
        json!({"Code": code, "Name": name, "DispositionPeriod": period, "NumberOfAnnouncement": times})
    }

    fn tp(code: &str, period: &str, name: &str, times: Value) -> Value {
        json!({"SecuritiesCompanyCode": code, "CompanyName": name,
               "DispositionPeriod": period, "NumberOfAnnouncement": times})
    }

    fn fetch_of(twse: Value, tpex: Value) -> MapFetch {
        MapFetch::new()
            .ok(URL_PUNISH_TWSE, &twse.to_string())
            .ok(URL_PUNISH_TPEX, &tpex.to_string())
    }

    fn old_row(code: &str, market: &str, name: &str, start: &str, end: &str, times: i64) -> Value {
        json!({"code": code, "name": name, "start": start, "end": end, "times": times, "market": market})
    }

    fn old_of(rows: Vec<Value>) -> PreviousOutput {
        PreviousOutput::from_value(json!({ "punish": rows }))
    }

    fn run_with(fetch: &MapFetch, prev: &PreviousOutput) -> SourceOutcome<Vec<Punish>> {
        block_on(run_guarded(fetch, &clock(), prev))
    }

    fn summary(rows: &[Punish]) -> Vec<(&str, &str, &str, i64, &str)> {
        rows.iter()
            .map(|r| {
                (
                    r.code.as_str(),
                    r.start.as_str(),
                    r.end.as_str(),
                    r.times,
                    r.market.as_str(),
                )
            })
            .collect()
    }

    // ───── B-PUN-1：端點與前綴 ─────

    #[test]
    fn b_pun_1_urls_prefixes_and_markets_are_verbatim() {
        assert_eq!(
            URL_PUNISH_TWSE,
            "https://openapi.twse.com.tw/v1/announcement/punish"
        );
        assert_eq!(
            URL_PUNISH_TPEX,
            "https://www.tpex.org.tw/openapi/v1/tpex_disposal_information"
        );
        assert_eq!(ERR_PREFIX_TWSE, "處置股來源失敗（上市）：");
        assert_eq!(ERR_PREFIX_TPEX, "處置股來源失敗（上櫃）：");
        assert_eq!((MARKET_TWSE, MARKET_TPEX), ("上市", "上櫃"));
        let f = fetch_of(json!([]), json!([]));
        run_with(&f, &PreviousOutput::default());
        assert_eq!(f.requests(), [URL_PUNISH_TWSE, URL_PUNISH_TPEX]);
    }

    #[test]
    fn b_pun_1_field_names_differ_per_market() {
        let out = run_with(
            &fetch_of(
                json!([tw("1101", P, "台泥", json!(2))]),
                json!([
                    tp("6488", Q, "環球晶", json!("3")),
                    // 上櫃端點用 `Code`／`Name` 的列＝沒有 `SecuritiesCompanyCode`＝空代號，被代號過濾擋掉
                    tw("0050", Q, "wrong keys", json!(1))
                ]),
            ),
            &PreviousOutput::default(),
        );
        assert_eq!(
            summary(&out.value),
            [
                ("1101", "2026-10-05", "2026-10-12", 2, "上市"),
                ("6488", "2026-10-05", "2026-10-12", 3, "上櫃"),
            ]
        );
        assert_eq!(out.value[1].name, "環球晶");
        assert!(out.errors.is_empty() && out.logs.is_empty() && !out.fresh);
    }

    // ───── B-PUN-2：代號過濾（RE_STOCK_CODE／RE_ETF_CODE） ─────

    #[test]
    fn b_pun_2_regexes_are_the_python_originals_modulo_trailing_newline() {
        assert_eq!(RE_STOCK_CODE.replace(r"\n?", ""), r"^\d{4}[A-Z]?$");
        assert_eq!(RE_ETF_CODE.replace(r"\n?", ""), r"^00\d{2,4}$");
    }

    #[test]
    fn b_pun_2_code_filter_matches_python_table() {
        // 期望值全部來自真的跑 Python `is_stock_or_etf`（含全形、阿拉伯數字、結尾換行、小寫字母）。
        let table: [(&str, bool); 22] = [
            ("2330", true),
            ("\u{ff12}\u{ff13}\u{ff13}\u{ff10}", true),
            ("2891B", true),
            ("2891b", false),
            ("00981A", false),
            ("0050", true),
            ("006208", true),
            ("00800", true),
            ("0", false),
            ("00123456", false),
            ("\u{ff12}\u{ff13}\u{ff13}\u{ff10}\u{ff22}", false),
            ("2330\n", true),
            (" 2330 ", false),
            ("\u{662}\u{663}\u{663}\u{660}", true),
            ("0\u{ff10}50", true),
            ("23300", false),
            ("123", false),
            ("", false),
            ("2330\n\n", false),
            ("2330 B", false),
            ("086845", false),
            ("2891BB", false),
        ];
        for (code, want) in table {
            assert_eq!(is_stock_or_etf(code), want, "{code:?}");
        }
    }

    // ───── B-PUN-3：期間雙格式（對照 Python 實測） ─────

    fn period(v: Value) -> Option<(String, String)> {
        parse_disposition_period(Some(&v))
    }

    #[test]
    fn b_pun_3_both_period_formats_and_their_mixes() {
        let want = Some(("2026-10-05".to_string(), "2026-10-12".to_string()));
        for s in [
            "115/10/05\u{ff5e}115/10/12",
            "1151005~1151012",
            "1151005\u{ff5e}1151012",
            "115/10/05~115/10/12",
            " 115/10/05 \u{ff5e} 115/10/12 ",
            "1151005 ~1151012",
            "1151005~1151012\n",
            "1151005~1151012\u{1f}",
            "11510/05~1151012",
            // 全形數字、阿拉伯數字都是 Nd
            "\u{ff11}\u{ff11}\u{ff15}\u{ff11}\u{ff10}\u{ff10}\u{ff15}~\u{ff11}\u{ff11}\u{ff15}\u{ff11}\u{ff10}\u{ff11}\u{ff12}",
            "\u{661}\u{661}\u{665}\u{661}\u{660}\u{660}\u{665}~\u{661}\u{661}\u{665}\u{661}\u{660}\u{661}\u{662}",
        ] {
            assert_eq!(period(json!(s)), want, "{s:?}");
        }
    }

    #[test]
    fn b_pun_3_bad_periods_are_none() {
        for v in [
            json!(""),
            json!(null),
            json!(0),
            json!(false),
            json!([]),
            json!({}),
            json!(12345),
            json!(true),
            json!({"a": 1}),
            json!("115/10/05~115/10/12~x"), // 只切第一個，第二段含 `~x`
            json!("1151005\u{ff5e}1151012~1"),
            json!("1151005~115100\u{b2}"), // 上標 2：isdigit 真、int 失敗
            json!("\u{ff5e}"),
            json!("~"),
            json!("1151005"),
            json!("115-10-05~115-10-12"),
            json!("1151305~1151312"), // 13 月
            json!("1150230~1150231"), // 2 月 30 日
            json!("1151005~"),
            json!("~1151012"),
            json!("115/10/05\u{ff5e}115/10/12\u{ff5e}115/10/13"),
            json!("1151005\u{ff5e}1151012~1151013"),
        ] {
            assert_eq!(period(v.clone()), None, "{v}");
        }
        assert_eq!(parse_disposition_period(None), None);
    }

    #[test]
    fn b_pun_3_year_bounds_follow_python_date() {
        // 民國 0 年＝1911；民國 999 年＝2910（`date()` 在 1..=9999 內都合法）
        assert_eq!(
            period(json!("0001005~0001006")),
            Some(("1911-10-05".into(), "1911-10-06".into()))
        );
        assert_eq!(
            period(json!("9991231~9991231")),
            Some(("2910-12-31".into(), "2910-12-31".into()))
        );
    }

    // ───── B-PUN-5：times ─────

    #[test]
    fn b_pun_5_times_matches_python_int() {
        let t = |v: Value| punish_times(Some(&v));
        assert_eq!(t(json!(4)), 4);
        assert_eq!(t(json!(" 3 ")), 3);
        assert_eq!(t(json!("+3")), 3);
        assert_eq!(t(json!("3_0")), 30);
        assert_eq!(t(json!("\u{663}")), 3);
        assert_eq!(t(json!("\u{ff13}")), 3);
        assert_eq!(t(json!("03")), 3);
        assert_eq!(t(json!("0")), 0);
        assert_eq!(t(json!("-0")), 0);
        assert_eq!(t(json!("9223372036854775807")), i64::MAX);
        assert_eq!(t(json!("-9223372036854775808")), i64::MIN);
        // 解析失敗＝1
        for bad in [
            json!("x"),
            json!(null),
            json!("3__0"),
            json!("+_3"),
            json!("_3"),
            json!("3_"),
            json!(3.0),
            json!(true),
            json!("1e2"),
            json!(""),
            json!(" "),
            json!("\u{b2}"),
            json!([3]),
            json!({"a": 3}),
        ] {
            assert_eq!(t(bad.clone()), 1, "{bad}");
        }
        assert_eq!(punish_times(None), 1, "欄位缺漏＝str(None)");
        assert_eq!(t(json!(5)), 5);
    }

    #[test]
    fn times_accepts_every_integer_python_accepts_up_to_i64() {
        // Python 實測：-2、99999999999、4294967296 都原樣輸出（主控裁定：`times` 為 i64）。
        for (v, want) in [
            (json!("-2"), -2),
            (json!(-2), -2),
            (json!("99999999999"), 99_999_999_999),
            (json!(99_999_999_999_i64), 99_999_999_999),
            (json!("4294967296"), 4_294_967_296),
        ] {
            assert_eq!(punish_times(Some(&v)), want, "{v}");
        }
    }

    #[test]
    fn times_beyond_i64_is_the_one_remaining_deviation_and_falls_back_to_1() {
        // Python 任意精度會輸出這些值；Rust 無法表示，視為解析失敗＝1（模組文件差異 1）。
        for v in [
            json!("9223372036854775808"),
            json!("-9223372036854775809"),
            json!("1_000_000_000_000_000_000_000"),
        ] {
            assert_eq!(punish_times(Some(&v)), 1, "{v}");
        }
    }

    #[test]
    fn a_negative_old_times_survives_reuse_verbatim() {
        // 舊檔（Python 寫出）的 times 為負：反序列化不可丟列，該 market 失敗時原樣沿用。
        let old = old_of(vec![
            old_row("2330", "上市", "a", "2026-10-05", "2026-10-12", -2),
            old_row(
                "3293",
                "上櫃",
                "b",
                "2026-10-05",
                "2026-10-12",
                99_999_999_999,
            ),
        ]);
        let f = MapFetch::new()
            .network_error(URL_PUNISH_TWSE, "x")
            .network_error(URL_PUNISH_TPEX, "y");
        let out = run_with(&f, &old);
        assert_eq!(
            summary(&out.value),
            [
                ("2330", "2026-10-05", "2026-10-12", -2, "上市"),
                ("3293", "2026-10-05", "2026-10-12", 99_999_999_999, "上櫃"),
            ]
        );
    }

    // ───── B-PUN-6／7：各自失敗、按 market 沿用 ─────

    #[test]
    fn b_pun_6_each_endpoint_fails_independently() {
        let f = MapFetch::new().status(URL_PUNISH_TWSE, 500).ok(
            URL_PUNISH_TPEX,
            &json!([tp("6488", Q, "A", json!(2))]).to_string(),
        );
        let out = run_with(&f, &PreviousOutput::default());
        assert_eq!(
            summary(&out.value),
            [("6488", "2026-10-05", "2026-10-12", 2, "上櫃")]
        );
        assert_eq!(
            out.errors,
            ["處置股來源失敗（上市）：HTTP Error 500: Internal Server Error"]
        );
        assert!(out.logs.is_empty(), "沒有舊列就不記沿用日誌");
        assert!(!out.fresh);
    }

    #[test]
    fn b_pun_7_failed_market_reuses_only_its_own_old_rows_and_logs_the_count() {
        let old = old_of(vec![
            old_row("2330", "上市", "oldtw", "2026-10-05", "2026-10-12", 9),
            old_row("3293", "上櫃", "oldtp", "2026-10-05", "2026-10-12", 9),
            old_row("0050", "興櫃", "other", "2026-10-05", "2026-10-12", 9),
            old_row("1101", "上市", "oldtw2", "2026-10-05", "2026-10-12", 9),
        ]);
        // 上櫃失敗：上市以新抓為準（舊上市列不帶進來），上櫃沿用舊的
        let f = MapFetch::new()
            .ok(
                URL_PUNISH_TWSE,
                &json!([tw("2330", P, "newtw", json!(1))]).to_string(),
            )
            .network_error(URL_PUNISH_TPEX, "net");
        let out = run_with(&f, &old);
        assert_eq!(
            summary(&out.value),
            [
                ("2330", "2026-10-05", "2026-10-12", 1, "上市"),
                ("3293", "2026-10-05", "2026-10-12", 9, "上櫃"),
            ]
        );
        assert_eq!(out.errors, ["處置股來源失敗（上櫃）：net"]);
        assert_eq!(out.logs, ["[處置股] 上櫃沿用上次資料（1 筆）"]);
    }

    #[test]
    fn b_pun_7_both_fail_reuses_both_markets_and_drops_other_markets() {
        let old = old_of(vec![
            old_row("2330", "上市", "a", "2026-10-05", "2026-10-12", 9),
            old_row("3293", "上櫃", "b", "2026-10-05", "2026-10-12", 9),
            old_row("0050", "興櫃", "c", "2026-10-05", "2026-10-12", 9),
        ]);
        let f = MapFetch::new()
            .network_error(URL_PUNISH_TWSE, "x")
            .network_error(URL_PUNISH_TPEX, "y");
        let out = run_with(&f, &old);
        assert_eq!(
            summary(&out.value),
            [
                ("2330", "2026-10-05", "2026-10-12", 9, "上市"),
                ("3293", "2026-10-05", "2026-10-12", 9, "上櫃"),
            ]
        );
        assert_eq!(
            out.errors,
            ["處置股來源失敗（上市）：x", "處置股來源失敗（上櫃）：y"]
        );
        assert_eq!(
            out.logs,
            [
                "[處置股] 上市沿用上次資料（1 筆）",
                "[處置股] 上櫃沿用上次資料（1 筆）"
            ]
        );
        // `previous` ＝ 兩段都失敗時的沿用值（給來源整個 panic 時用）
        assert_eq!(previous(&old), out.value);
    }

    #[test]
    fn b_pun_7_successful_but_empty_endpoint_does_not_reuse_old_rows() {
        let old = old_of(vec![old_row(
            "3293",
            "上櫃",
            "oldtp",
            "2026-10-05",
            "2026-10-12",
            9,
        )]);
        let out = run_with(&fetch_of(json!([]), json!([])), &old);
        assert!(out.value.is_empty() && out.errors.is_empty() && out.logs.is_empty());
    }

    #[test]
    fn b_pun_4_tpex_blank_template_row_is_filtered_not_an_error_and_not_a_failure() {
        let blank = json!([{
            "Date": "", "SecuritiesCompanyCode": "", "CompanyName": "",
            "DispositionPeriod": "", "DispositionReasons": "", "DisposalCondition": ""
        }]);
        let old = old_of(vec![old_row(
            "3293",
            "上櫃",
            "oldtp",
            "2026-10-05",
            "2026-10-12",
            9,
        )]);
        let out = run_with(
            &fetch_of(json!([tw("1101", P, "台泥", json!(1))]), blank),
            &old,
        );
        assert_eq!(
            summary(&out.value),
            [("1101", "2026-10-05", "2026-10-12", 1, "上市")]
        );
        assert!(out.errors.is_empty() && out.logs.is_empty());
    }

    // ───── B-PUN-8／9：二度處置去重、排序 ─────

    #[test]
    fn b_pun_8_latest_end_then_latest_start_wins_regardless_of_row_order() {
        let out = run_with(
            &fetch_of(
                json!([
                    // 2455：第一次先、第二次後
                    tw("2455", "115/10/01\u{ff5e}115/10/05", "first", json!(1)),
                    tw("2455", "115/10/06\u{ff5e}115/10/16", "second", json!(2)),
                    // 3016：第二次先、第一次後
                    tw("3016", "115/10/07\u{ff5e}115/10/20", "second", json!(2)),
                    tw("3016", "115/10/01\u{ff5e}115/10/06", "first", json!(1)),
                    // 同結束日：開始日較晚者勝，兩種列序
                    tw(
                        "3167",
                        "115/10/05\u{ff5e}115/10/12",
                        "late-start-first",
                        json!(2)
                    ),
                    tw(
                        "3167",
                        "115/10/01\u{ff5e}115/10/12",
                        "early-start-last",
                        json!(1)
                    ),
                    tw(
                        "3055",
                        "115/10/01\u{ff5e}115/10/12",
                        "early-start-first",
                        json!(1)
                    ),
                    tw(
                        "3055",
                        "115/10/05\u{ff5e}115/10/12",
                        "late-start-last",
                        json!(3)
                    ),
                ]),
                json!([]),
            ),
            &PreviousOutput::default(),
        );
        assert_eq!(
            summary(&out.value),
            [
                ("2455", "2026-10-06", "2026-10-16", 2, "上市"),
                ("3016", "2026-10-07", "2026-10-20", 2, "上市"),
                ("3055", "2026-10-05", "2026-10-12", 3, "上市"),
                ("3167", "2026-10-05", "2026-10-12", 2, "上市"),
            ]
        );
        assert_eq!(out.value[0].name, "second", "name 取保留列自己的");
    }

    #[test]
    fn b_pun_8_dedupe_is_across_markets_and_ties_keep_the_first_row() {
        // 同 code、同期間：新抓的上市在上櫃之前＝上市勝
        let both = run_with(
            &fetch_of(
                json!([tw("2330", P, "twse", json!(1))]),
                json!([tp("2330", Q, "tpex", json!(5))]),
            ),
            &PreviousOutput::default(),
        );
        assert_eq!(both.value[0].market, "上市");
        // 上市失敗：列序是「上櫃新抓 ＋ 舊上市」＝上櫃勝
        let old_tw = old_of(vec![old_row(
            "2330",
            "上市",
            "oldtw",
            "2026-10-05",
            "2026-10-12",
            9,
        )]);
        let f = MapFetch::new().network_error(URL_PUNISH_TWSE, "x").ok(
            URL_PUNISH_TPEX,
            &json!([tp("2330", Q, "tpex", json!(5))]).to_string(),
        );
        let out = run_with(&f, &old_tw);
        assert_eq!(
            summary(&out.value),
            [("2330", "2026-10-05", "2026-10-12", 5, "上櫃")]
        );
        // 上櫃失敗：列序是「上市新抓 ＋ 舊上櫃」＝上市勝
        let old_tp = old_of(vec![old_row(
            "2330",
            "上櫃",
            "oldtp",
            "2026-10-05",
            "2026-10-12",
            9,
        )]);
        let f = MapFetch::new()
            .ok(
                URL_PUNISH_TWSE,
                &json!([tw("2330", P, "twse", json!(1))]).to_string(),
            )
            .network_error(URL_PUNISH_TPEX, "y");
        let out = run_with(&f, &old_tp);
        assert_eq!(
            summary(&out.value),
            [("2330", "2026-10-05", "2026-10-12", 1, "上市")]
        );
    }

    #[test]
    fn b_pun_9_output_is_sorted_by_code_string_order() {
        let out = run_with(
            &fetch_of(
                json!([
                    tw("2891B", P, "a", json!(1)),
                    tw("0050", P, "a", json!(1)),
                    tw("1101", P, "a", json!(1)),
                    tw("006208", P, "a", json!(1)),
                    tw("2330", P, "a", json!(1)),
                ]),
                json!([tp("3293", Q, "a", json!(1)), tp("0051", Q, "a", json!(1))]),
            ),
            &PreviousOutput::default(),
        );
        let codes: Vec<&str> = out.value.iter().map(|r| r.code.as_str()).collect();
        assert_eq!(
            codes,
            ["0050", "0051", "006208", "1101", "2330", "2891B", "3293"]
        );
    }

    #[test]
    fn old_rows_compare_as_strings() {
        let old = old_of(vec![
            old_row("2330", "上市", "a", "zzz", "zzz", 9),
            old_row("2330", "上市", "b", "1", "zzz", 8),
        ]);
        let f = MapFetch::new()
            .network_error(URL_PUNISH_TWSE, "n")
            .ok(URL_PUNISH_TPEX, "[]");
        let out = run_with(&f, &old);
        assert_eq!(out.value[0].name, "a");
        assert_eq!(out.logs, ["[處置股] 上市沿用上次資料（2 筆）"]);
    }

    // ───── 中途失敗的殘留列（Python 實測） ─────

    #[test]
    fn mid_stream_failure_keeps_the_rows_collected_so_far_and_adds_the_old_ones() {
        let old = old_of(vec![old_row(
            "1101",
            "上市",
            "o",
            "2026-10-05",
            "2026-10-12",
            9,
        )]);
        let out = run_with(
            &fetch_of(
                json!([
                    tw("2330", P, "A", json!(2)),
                    "x",
                    tw("2317", P, "B", json!(1))
                ]),
                json!([]),
            ),
            &old,
        );
        assert_eq!(
            summary(&out.value),
            [
                ("1101", "2026-10-05", "2026-10-12", 9, "上市"),
                ("2330", "2026-10-05", "2026-10-12", 2, "上市"),
            ],
            "2317 在壞元素之後、不收；2330 留下；舊 1101 接上"
        );
        assert_eq!(
            out.errors,
            ["處置股來源失敗（上市）：'str' object has no attribute 'get'"]
        );
        assert_eq!(out.logs, ["[處置股] 上市沿用上次資料（1 筆）"]);
    }

    // ───── 與 Python 實測的整表對照（真的跑 fetch_punish 產生） ─────

    #[test]
    fn python_cross_check_table_matches_the_real_fetch_punish() {
        let cases: Vec<Value> =
            serde_json::from_str(include_str!("testdata/punish_python_cases.json"))
                .expect("測試資料是合法 JSON");
        assert!(cases.len() >= 139, "表應有 139 列，實得 {}", cases.len());
        let mut compared = 0;
        let mut crashes = 0;
        for case in &cases {
            let desc = case["desc"].as_str().expect("desc");
            let f = MapFetch::new();
            let f = register(f, URL_PUNISH_TWSE, &case["twse"]);
            let f = register(f, URL_PUNISH_TPEX, &case["tpex"]);
            let prev = match &case["old"] {
                Value::Array(rows) => old_of(rows.clone()),
                _ => PreviousOutput::default(),
            };
            let out = run_with(&f, &prev);
            let expect = &case["expect"];
            if expect.get("crash").is_some() {
                // Python 在舊資料形狀錯誤時崩潰；Rust 依 D8-3 逐筆判形狀（見下方專測）
                crashes += 1;
                continue;
            }
            assert_eq!(
                serde_json::to_value(&out.value).expect("序列化"),
                expect["punish"],
                "{desc}：punish 不符"
            );
            assert_eq!(
                out.logs,
                expect["logs"]
                    .as_array()
                    .expect("logs")
                    .iter()
                    .map(|v| v.as_str().expect("log").to_string())
                    .collect::<Vec<_>>(),
                "{desc}：logs 不符"
            );
            // errors：Python 的傳輸失敗訊息（`net`）與 Rust 不同，比前綴；形狀錯誤的訊息逐字相同
            let got: Vec<String> = out.errors.iter().map(|e| normalize_error(e)).collect();
            let wanted: Vec<String> = expect["errors"]
                .as_array()
                .expect("errors")
                .iter()
                .map(|v| v.as_str().expect("error").to_string())
                .collect();
            let transport = matches!(case["twse"], Value::String(ref s) if s == "NET")
                || matches!(case["tpex"], Value::String(ref s) if s == "NET");
            if transport {
                let wanted: Vec<String> = wanted.iter().map(|e| normalize_error(e)).collect();
                assert_eq!(got, wanted, "{desc}：errors（前綴）不符");
            } else {
                assert_eq!(out.errors, wanted, "{desc}：errors 不符");
            }
            assert!(!out.fresh, "{desc}");
            compared += 1;
        }
        assert_eq!(crashes, 2, "Python 崩潰的情境只有兩個（舊資料形狀錯誤）");
        assert_eq!(compared, 137);
    }

    /// 測試資料裡 `"NET"` 代表 Python 側注入的傳輸失敗；Rust 側回連線錯誤。
    fn register(f: MapFetch, url: &str, body: &Value) -> MapFetch {
        match body {
            Value::String(s) if s == "NET" => f.network_error(url, "net"),
            other => f.ok(url, &other.to_string()),
        }
    }

    #[test]
    fn d8_3_bad_old_punish_shapes_are_dropped_instead_of_crashing() {
        let f = || {
            MapFetch::new()
                .network_error(URL_PUNISH_TWSE, "x")
                .ok(URL_PUNISH_TPEX, "[]")
        };
        // 不是陣列＝沒有舊資料
        for bad in [json!("garbage"), json!(null), json!({"a": 1}), json!(5)] {
            let prev = PreviousOutput::from_value(json!({ "punish": bad }));
            let out = run_with(&f(), &prev);
            assert!(out.value.is_empty() && out.logs.is_empty(), "{bad}");
            assert_eq!(out.errors.len(), 1);
        }
        // 陣列內的壞元素逐筆丟棄，其餘照用
        let prev = old_of(vec![
            old_row("2330", "上市", "a", "2026-10-05", "2026-10-12", 9),
            json!({"code": "x"}),
            json!("junk"),
            old_row("1101", "上市", "b", "2026-10-05", "2026-10-12", 9),
        ]);
        let out = run_with(&f(), &prev);
        let codes: Vec<&str> = out.value.iter().map(|r| r.code.as_str()).collect();
        assert_eq!(codes, ["1101", "2330"]);
        assert_eq!(out.logs, ["[處置股] 上市沿用上次資料（2 筆）"]);
    }

    // ───── D7：panic 隔離 ─────

    #[test]
    fn d7_a_panic_in_either_endpoint_is_that_markets_failure_and_reuses_its_old_rows() {
        let old = old_of(vec![
            old_row("2330", "上市", "a", "2026-10-05", "2026-10-12", 9),
            old_row("3293", "上櫃", "b", "2026-10-05", "2026-10-12", 9),
        ]);
        // 兩個端點都 panic：兩段都失敗、都沿用、都記錯
        let out = block_on(run_guarded(&PanicFetch, &clock(), &old));
        assert_eq!(
            summary(&out.value),
            [
                ("2330", "2026-10-05", "2026-10-12", 9, "上市"),
                ("3293", "2026-10-05", "2026-10-12", 9, "上櫃"),
            ]
        );
        assert_eq!(out.errors.len(), 2);
        assert!(out.errors[0].starts_with("處置股來源失敗（上市）：panic: "));
        assert!(out.errors[1].starts_with("處置股來源失敗（上櫃）：panic: "));
        assert!(!out.fresh);
        assert!(out
            .logs
            .iter()
            .any(|l| l == "[處置股] 上市沿用上次資料（1 筆）"));
        assert!(out
            .logs
            .iter()
            .any(|l| l == "[處置股] 上櫃沿用上次資料（1 筆）"));
    }

    #[test]
    fn d7_a_panic_in_assemble_degrades_to_reusing_the_old_rows() {
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                test_hooks::PANIC_IN_ASSEMBLE.with(|c| c.set(false));
            }
        }
        let old = old_of(vec![
            old_row("2330", "上市", "a", "2026-10-05", "2026-10-12", 9),
            old_row("3293", "上櫃", "b", "2026-10-05", "2026-10-12", 9),
            old_row("0050", "興櫃", "c", "2026-10-05", "2026-10-12", 9),
        ]);
        let f = fetch_of(json!([tw("1101", P, "台泥", json!(2))]), json!([]));
        test_hooks::PANIC_IN_ASSEMBLE.with(|c| c.set(true));
        let _reset = Reset;
        let out = block_on(run_guarded(&f, &clock(), &old));
        assert_eq!(
            summary(&out.value),
            [
                ("2330", "2026-10-05", "2026-10-12", 9, "上市"),
                ("3293", "2026-10-05", "2026-10-12", 9, "上櫃"),
            ]
        );
        // 與兩段都失敗（previous）一致：其他市場（興櫃）的舊列被丟掉。
        assert!(out.value.iter().all(|r| r.market != "興櫃"));
        assert_eq!(out.errors.len(), 1);
        assert!(out.errors[0].starts_with("處置股來源失敗：panic: "));
        assert!(out.errors[0].contains("注入的 assemble panic"));
        assert!(!out.fresh);
    }

    #[test]
    fn d7_run_guarded_is_transparent_when_nothing_panics() {
        let f = fetch_of(json!([tw("1101", P, "台泥", json!(2))]), json!([]));
        let out = run_with(&f, &PreviousOutput::default());
        assert_eq!(out.value.len(), 1);
        assert!(out.errors.is_empty() && out.logs.is_empty());
    }

    // ───── oracle ─────

    #[test]
    fn oracle_every_scenario_matches_python_expected_json() {
        oracle::for_each_scenario(|name, sc, expected| {
            let out = block_on(run_guarded(&sc.fetch, &sc.clock, &sc.previous()));
            assert_eq!(
                serde_json::to_value(&out.value).expect("序列化"),
                expected["punish"],
                "情境 {name}：punish 不符"
            );
            oracle::assert_errors(
                name,
                &out.errors,
                expected,
                &[ERR_PREFIX_TWSE, ERR_PREFIX_TPEX],
            );
            assert!(!out.fresh, "情境 {name}：處置股不計 fresh");
        });
    }

    #[test]
    fn oracle_punish_scenarios_exist_and_cover_the_special_paths() {
        // 防止「情境被改名／刪掉後 oracle 測試靜默變弱」：這四個一定要在，且逐個核對關鍵證據。
        for name in [
            "punish-period-formats",
            "punish-second-disposition",
            "punish-tpex-blank-template",
            "punish-tpex-fail",
        ] {
            let sc = crate::fetch::fixture::Scenario::named(name).expect("情境存在");
            let expected = sc.expected().expect("expected.json");
            let out = block_on(run_guarded(&sc.fetch, &sc.clock, &sc.previous()));
            assert_eq!(
                serde_json::to_value(&out.value).expect("序列化"),
                expected["punish"],
                "{name}"
            );
            match name {
                "punish-period-formats" => {
                    let codes: Vec<&str> = out
                        .value
                        .iter()
                        .filter(|r| r.market == "上市")
                        .map(|r| r.code.as_str())
                        .collect();
                    assert_eq!(
                        codes,
                        [
                            "0050", "006208", "1101", "1102", "1216", "1301", "1303", "1326",
                            "2891B"
                        ]
                    );
                    assert!(out.errors.is_empty());
                }
                "punish-second-disposition" => {
                    let by = |c: &str| out.value.iter().find(|r| r.code == c).expect("code");
                    assert_eq!(
                        (by("2455").times, by("2455").end.as_str()),
                        (2, "2026-10-16")
                    );
                    assert_eq!(
                        (by("3016").times, by("3016").end.as_str()),
                        (2, "2026-10-20")
                    );
                    assert_eq!(by("3167").times, 2);
                    assert_eq!(by("3055").times, 3);
                    assert_eq!(by("2221").market, "上市");
                    assert_eq!(by("8084").market, "上櫃");
                }
                "punish-tpex-blank-template" => {
                    assert!(out.errors.is_empty());
                    assert!(out.value.iter().all(|r| r.market == "上市"));
                }
                "punish-tpex-fail" => {
                    assert_eq!(out.errors.len(), 1);
                    assert!(out.errors[0].starts_with(ERR_PREFIX_TPEX));
                    let tpex: Vec<&str> = out
                        .value
                        .iter()
                        .filter(|r| r.market == "上櫃")
                        .map(|r| r.code.as_str())
                        .collect();
                    assert_eq!(tpex, ["3293", "6488"]);
                    assert!(out.value.iter().all(|r| r.code != "2002"));
                }
                _ => unreachable!(),
            }
        }
    }
}
