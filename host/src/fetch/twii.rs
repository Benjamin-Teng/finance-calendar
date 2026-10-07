//! 動態桌布的加權指數兩鍵：盤中走勢 `twii_intraday`（B-ID）、日 K `twii_daily`（B-DK）
//! （behavior-inventory B-ID-1～6、B-DK-1～4、B-GRD-1～3、C-3、K-4、K-10、U-2、U-6；原始碼
//! `update_tw_events.py` 的 `parse_twii_intraday`／`fetch_twii_intraday`／`parse_twii_daily`／
//! `fetch_twii_daily`／`guarded_wallpaper_fetch`，`:845-957`、`:1085-1095`）。
//!
//! # 與其他來源不同的三件事（B-GRD-2、K-10）
//!
//! 回傳 [`WallpaperOutcome`]（見 [`super::outcome`]）：錯誤進 `wallpaper_errors`、**不進** `errors`、
//! **不計** `fresh`（型別上就沒有那兩個欄位）。每個鍵各自獨立沿用舊值（含其日期）；全新安裝又失敗＝
//! `value` 為 `None`＝輸出省略該鍵。`value` 是原始 JSON：沿用時把舊值（`old` 是物件／陣列即可，不檢查內部）
//! 原樣放回，同 Python 的 `return old`。
//!
//! # 盤中走勢（B-ID）
//!
//! 只取「最後一個完整交易日」＝台北 13:30 已過的最新交易日（用 [`Clock::tpe`]、不吃系統時區）。盤中執行時
//! 當日不合格，取前一個交易日，所以部分序列不會取代上一個完整交易日。逐筆：`close` 為 `null` 或時間戳
//! 不在 5 分鐘格線上（Yahoo 另附的即時報價）者丟棄；台北時間 09:00–13:30（含兩端，13:30 整點的收盤試撮點
//! 保留）才收；價格 `round2`（K-4）。選定的那一天若起點晚於 09:05、終點早於 13:25，或相鄰點相隔超過 300 秒，
//! 一律失敗（不拿不完整序列冒充完整日，也不偷偷退回更早一天）。成功但日期比舊值早＝**靜默**沿用舊值
//! （不記錯誤；與日 K 不同）。輸出 `{"date", "points": [[epoch 秒 int, 價 float], …]}`。
//!
//! # 日 K（B-DK）
//!
//! 由舊到新、至少 [`DAILY_MIN`]（40）根、保留整個 3 個月窗口。台北 13:30 前執行時排除當日（未收盤）那根；
//! OHLC 任一為 `null` 的列丟棄；同一天重複取**後者**；OHLC `round2`。新抓的最後一根早於舊值的最後一根
//! ＝沿用舊值並**記錯誤**（`加權日K來源回傳較舊的資料（…），沿用上次資料`）。
//!
//! # 與 Python 的差異
//!
//! - **D8-6**（數值欄位的極端型別）：布林或非有限值（`"nan"`／`"inf"` 字串、算出 `inf`）當價格／OHLC，
//!   Python 的 `float(True)` 是 1.0、`float("inf")` 是 `inf`，這裡一律視為該來源失敗；日 K 的時間戳為布林亦同。
//!   盤中走勢的時間戳為布林：Python 的 `True % 300` 是 1（丟棄）、`False % 300` 是 0 但換算成 1970 年不在盤中
//!   時段（丟棄），這裡也是丟棄（結果相同，不算差異）。
//! - 時間戳為非整數浮點：盤中走勢一律不在 5 分鐘格線上而丟棄（與 Python 相同）；日 K 依 Python 的
//!   `fromtimestamp` 取當地日期（向下取整，與 Python 的微秒四捨五入只在離整秒不到 0.5 微秒時不同）。
//!   超出 Python `datetime` 範圍（年份不在 1–9999）＝失敗。真實來源（Yahoo）一律整數秒。
//! - 錯誤訊息的內文：Python 的 `TypeError`／`KeyError` 等訊息盡量逐字，其餘（含 `result 為空（{error}）` 的
//!   `repr`）用 [`py_repr`] 近似；內文無消費端解析（B-HTTP-7），4.2 以前綴比對。
//! - 非預期的 panic：Python 沒有（它的 `except Exception` 只接 Python 例外）；這裡每個鍵另有一層
//!   [`guarded_wallpaper`]（D7），錯誤字串前綴同 B-GRD-1。
//! - 輸出物件的鍵序：Python 是 `date, points`／`date, open, high, low, close`；這裡經 `serde_json::Value`
//!   存放（`Output` 的三個桌布鍵是 `Value`），物件鍵依字母序寫出。內容相同，消費端依鍵名讀取。

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{Map, Value};
use time::macros::time;
use time::{Date, OffsetDateTime, PrimitiveDateTime};

use super::clock::{Clock, TPE_OFFSET};
use super::dates::{iso_date, round2};
use super::http::{get_json, Fetch};
use super::json_util::{
    py_dict, py_index, py_iter, py_repr, py_str_or_empty, py_type_name, truthy,
};
use super::outcome::{guarded_wallpaper, WallpaperOutcome};
use super::output::PreviousOutput;
use super::quotes::{dict_or_empty, to_float};

/// B-ID-1：盤中走勢端點（5m 資料 Yahoo 只提供 60 天內；`range=5d`＝最近 5 個交易日，連假也撈得到上一個完整日）。
pub const URL_INTRADAY: &str =
    "https://query1.finance.yahoo.com/v8/finance/chart/%5ETWII?interval=5m&range=5d";
/// B-DK-1：日 K 端點。
pub const URL_DAILY: &str =
    "https://query1.finance.yahoo.com/v8/finance/chart/%5ETWII?interval=1d&range=3mo";
/// B-GRD-1 的 label。
pub const LABEL_INTRADAY: &str = "加權盤中走勢";
pub const LABEL_DAILY: &str = "加權日K";
/// 一般失敗前綴（B-FLOW-9）。
pub const ERR_PREFIX_INTRADAY: &str = "加權盤中走勢來源失敗：";
pub const ERR_PREFIX_DAILY: &str = "加權日K來源失敗：";

/// C-3：盤中 09:00–13:30（含收盤試撮）、相鄰間隔上限（秒）、日 K 最少根數。
const OPEN_SECS: i64 = 9 * 3600;
const CLOSE_SECS: i64 = 13 * 3600 + 30 * 60;
pub const INTRADAY_STEP: i64 = 300;
pub const DAILY_MIN: usize = 40;

const MSG_INTRADAY_SHAPE: &str = "盤中序列缺 timestamp 或與 close 長度不符";
const MSG_DAILY_SHAPE: &str = "日 K 缺 timestamp 或各欄長度不符";

/// 輸出的 `twii_intraday`（B-ID-5）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Intraday {
    pub date: String,
    /// `[epoch 秒, 價]`，依時間排序。
    pub points: Vec<(i64, f64)>,
}

/// 輸出的 `twii_daily` 單根 K（B-DK-2）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Candle {
    pub date: String,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
}

// ───────────────────────── 共用小函式 ─────────────────────────

/// 台北當地日期與當日秒數（`datetime.fromtimestamp(t, TPE)` 的日期與 `.time()`）；年份不在 1–9999＝失敗。
fn tpe_day_and_secs(t: i64) -> Result<(Date, i64), String> {
    const UNIX_EPOCH_JULIAN_DAY: i64 = 2_440_588;
    let shifted = t
        .checked_add(i64::from(TPE_OFFSET.whole_seconds()))
        .ok_or_else(|| "timestamp out of range for platform time_t".to_string())?;
    let days = shifted.div_euclid(86_400);
    let date = i32::try_from(UNIX_EPOCH_JULIAN_DAY + days)
        .ok()
        .and_then(|jd| Date::from_julian_day(jd).ok())
        .filter(|d| (1..=9999).contains(&d.year()))
        .ok_or_else(|| "year is out of range".to_string())?;
    Ok((date, shifted.rem_euclid(86_400)))
}

/// `tpe_close_at(d)`：該日台北 13:30。
fn close_at(d: Date) -> OffsetDateTime {
    PrimitiveDateTime::new(d, time!(13:30)).assume_offset(TPE_OFFSET)
}

/// `hhmm_tpe` 的格式：當日秒數 → `HH:MM`。
fn hhmm(secs: i64) -> String {
    format!("{:02}:{:02}", secs / 3600, secs % 3600 / 60)
}

/// Yahoo v8 chart 回應 → `result[0]`（Python `chart_result`）；空結果失敗（附 Yahoo 的 error 內容）。
fn chart_result(j: &Value) -> Result<Map<String, Value>, String> {
    let empty = Map::new();
    // `(j or {}).get("chart") or {}`：j 為假值當空字典；非空非字典 → AttributeError。
    let top = if truthy(j) { py_dict(j)? } else { &empty };
    let chart = dict_or_empty(top.get("chart"), &empty)?;
    let first = match chart.get("result") {
        Some(v) if truthy(v) => py_index(v, 0)?,
        _ => {
            let error = chart
                .get("error")
                .map_or_else(|| "None".to_string(), py_repr);
            return Err(format!("result 為空（{error}）"));
        }
    };
    Ok(py_dict(&first)?.clone())
}

/// Python 的 `len(v)`：陣列／字串（字元數）／物件可；數字與布林 `TypeError`。
fn py_len(v: &Value) -> Result<usize, String> {
    match v {
        Value::Array(a) => Ok(a.len()),
        Value::String(s) => Ok(s.chars().count()),
        Value::Object(o) => Ok(o.len()),
        other => Err(format!(
            "object of type '{}' has no len()",
            py_type_name(other)
        )),
    }
}

/// `x or []`：假值（含缺鍵）當空序列（`None`），其餘原值（是否可迭代、有沒有長度留給後續各自失敗）。
fn or_empty(v: Option<&Value>) -> Option<&Value> {
    v.filter(|v| truthy(v))
}

/// `((r.get("indicators") or {}).get("quote") or [{}])[0]`。
fn first_quote(r: &Map<String, Value>) -> Result<Map<String, Value>, String> {
    let empty = Map::new();
    let indicators = dict_or_empty(r.get("indicators"), &empty)?;
    let first = match indicators.get("quote") {
        Some(v) if truthy(v) => py_index(v, 0)?,
        _ => Value::Object(Map::new()),
    };
    Ok(py_dict(&first)?.clone())
}

// ───────────────────────── 盤中走勢 ─────────────────────────

/// 盤中的一筆時間戳經 `t % 300` 判斷後的結果（只在該筆 `close` 非 `null` 時才會被求值，同 Python 的短路）。
enum Stamp {
    /// 不在 5 分鐘格線上（或 Python 會因換算成 1970 年而落在盤外的布林）。
    Skip,
    Use(i64),
}

fn intraday_stamp(t: &Value) -> Result<Stamp, String> {
    let on_grid = |i: i64| i.rem_euclid(INTRADAY_STEP) == 0;
    match t {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                return Ok(if on_grid(i) {
                    Stamp::Use(i)
                } else {
                    Stamp::Skip
                });
            }
            if let Some(u) = n.as_u64() {
                // 大於 i64::MAX 的整數：不在格線上就丟棄，在格線上時 `fromtimestamp` 會 OverflowError。
                return if u % INTRADAY_STEP as u64 == 0 {
                    Err("timestamp out of range for platform time_t".to_string())
                } else {
                    Ok(Stamp::Skip)
                };
            }
            let f = n.as_f64().ok_or("時間戳不是有限值")?;
            // 浮點的 `% 300`（與 Python 同為精確的 fmod）：非整數必不為 0；整數值的浮點等同整數。
            if f % INTRADAY_STEP as f64 != 0.0 {
                return Ok(Stamp::Skip);
            }
            if f.abs() >= 9.0e18 {
                return Err("timestamp out of range for platform time_t".to_string());
            }
            Ok(Stamp::Use(f as i64))
        }
        Value::Bool(_) => Ok(Stamp::Skip),
        Value::String(_) => Err("not all arguments converted during string formatting".to_string()),
        other => Err(format!(
            "unsupported operand type(s) for %: '{}' and 'int'",
            py_type_name(other)
        )),
    }
}

/// 盤中走勢（B-ID-2～5）。`now` 任何時區皆可（比的是同一個時刻）。`Err` 的文字是 `{e}`（不含前綴）。
pub fn parse_intraday(j: &Value, now: OffsetDateTime) -> Result<Intraday, String> {
    let r = chart_result(j)?;
    let stamps_v = or_empty(r.get("timestamp"));
    let q = first_quote(&r)?;
    let closes_v = or_empty(q.get("close"));
    // `if not stamps or len(stamps) != len(closes)`：先看 stamps 是否為空，再依序取兩個長度（數字沒有 len）。
    let Some(stamps_v) = stamps_v else {
        return Err(MSG_INTRADAY_SHAPE.to_string());
    };
    let (stamps_len, closes_len) = (py_len(stamps_v)?, closes_v.map_or(Ok(0), py_len)?);
    if stamps_len != closes_len {
        return Err(MSG_INTRADAY_SHAPE.to_string());
    }
    let stamps = py_iter(stamps_v.clone())?;
    let closes = closes_v.map_or(Ok(Vec::new()), |v| py_iter(v.clone()))?;
    let mut days: BTreeMap<Date, BTreeMap<i64, f64>> = BTreeMap::new();
    for (t, c) in stamps.iter().zip(&closes) {
        if c.is_null() {
            continue;
        }
        let Stamp::Use(t) = intraday_stamp(t)? else {
            continue;
        };
        let (date, secs) = tpe_day_and_secs(t)?;
        if !(OPEN_SECS..=CLOSE_SECS).contains(&secs) {
            continue;
        }
        days.entry(date)
            .or_default()
            .insert(t, round2(to_float(c)?));
    }
    let day = days
        .keys()
        .filter(|d| close_at(**d) <= now)
        .max()
        .copied()
        .ok_or_else(|| "序列內沒有任何已過 13:30 的交易日".to_string())?;
    let points: Vec<(i64, f64)> = days[&day].iter().map(|(t, p)| (*t, *p)).collect();
    let day_text = iso_date(day);
    // `days[day]` 由至少一筆建立，所以 first／last 必在。
    let (first_t, last_t) = (points[0].0, points[points.len() - 1].0);
    let first_secs = tpe_day_and_secs(first_t)?.1;
    let last_secs = tpe_day_and_secs(last_t)?.1;
    if first_secs > OPEN_SECS + INTRADAY_STEP {
        return Err(format!(
            "{day_text} 序列起點 {} 太晚，視為不完整",
            hhmm(first_secs)
        ));
    }
    if last_secs < CLOSE_SECS - INTRADAY_STEP {
        return Err(format!(
            "{day_text} 序列終點 {} 太早（來源尚未追上收盤），視為不完整",
            hhmm(last_secs)
        ));
    }
    for pair in points.windows(2) {
        if pair[1].0 - pair[0].0 > INTRADAY_STEP {
            return Err(format!(
                "{day_text} 序列在 {} 後缺口超過 5 分鐘",
                hhmm(tpe_day_and_secs(pair[0].0)?.1)
            ));
        }
    }
    Ok(Intraday {
        date: day_text,
        points,
    })
}

/// 舊輸出的 `twii_intraday`：**是物件**才算（Python `isinstance(old, dict)`；不檢查內部形狀、空物件也算）。
pub fn previous_intraday(prev: &PreviousOutput) -> Option<Value> {
    prev.raw("twii_intraday").filter(|v| v.is_object()).cloned()
}

/// 由組裝端呼叫：[`run_intraday`] 加上 panic 隔離（D7、B-GRD-1）。
pub async fn run_intraday_guarded<F: Fetch>(
    fetch: &F,
    clock: &Clock,
    prev: &PreviousOutput,
) -> WallpaperOutcome<Option<Value>> {
    guarded_wallpaper(
        LABEL_INTRADAY,
        previous_intraday(prev),
        run_intraday(fetch, clock, prev),
    )
    .await
}

async fn run_intraday<F: Fetch>(
    fetch: &F,
    clock: &Clock,
    prev: &PreviousOutput,
) -> WallpaperOutcome<Option<Value>> {
    let old = previous_intraday(prev);
    let mut out = WallpaperOutcome::new(None);
    let parsed = match get_json(fetch, URL_INTRADAY).await {
        Ok(j) => parse_intraday(&j, clock.tpe),
        Err(e) => Err(e.to_string()),
    };
    match parsed {
        Err(e) => {
            out.wallpaper_errors
                .push(format!("{ERR_PREFIX_INTRADAY}{e}"));
            out.logs
                .push(format!("[盤中走勢] 本輪失敗，沿用上次資料：{e}"));
            out.value = old;
        }
        Ok(new) => {
            // 日期不倒退：`if old and str(old.get("date") or "") > new["date"]: return old`（靜默）。
            let regressed = old
                .as_ref()
                .and_then(Value::as_object)
                .is_some_and(|o| !o.is_empty() && py_str_or_empty(o.get("date")) > new.date);
            out.value = if regressed { old } else { Some(to_json(&new)) };
        }
    }
    out
}

pub(super) fn to_json<T: Serialize>(v: &T) -> Value {
    // 只含字串與有限浮點／整數的結構，序列化不會失敗。
    serde_json::to_value(v).unwrap_or(Value::Null)
}

// ───────────────────────── 日 K ─────────────────────────

/// 日 K 的時間戳 → 台北當地日期（`datetime.fromtimestamp(t, TPE).date()`）。
fn daily_day(t: &Value) -> Result<Date, String> {
    let secs = match t {
        Value::Number(n) => match (n.as_i64(), n.as_f64()) {
            (Some(i), _) => i,
            (None, Some(f)) if f.is_finite() && f.abs() < 9.0e18 => f.floor() as i64,
            _ => return Err("timestamp out of range for platform time_t".to_string()),
        },
        Value::Bool(_) => return Err("布林值不是時間戳".to_string()),
        other => {
            return Err(format!(
                "argument must be int or float, not {}",
                py_type_name(other)
            ))
        }
    };
    Ok(tpe_day_and_secs(secs)?.0)
}

/// 日 K（B-DK-2、3）。`Err` 的文字是 `{e}`（不含前綴）。
pub fn parse_daily(j: &Value, now: OffsetDateTime) -> Result<Vec<Candle>, String> {
    let r = chart_result(j)?;
    let stamps_v = or_empty(r.get("timestamp"));
    let q = first_quote(&r)?;
    let col_values: Vec<Option<&Value>> = ["open", "high", "low", "close"]
        .iter()
        .map(|k| or_empty(q.get(*k)))
        .collect();
    // `if not stamps or any(len(c) != len(stamps) for c in cols)`：`any` 在第一個不符處就停。
    let Some(stamps_v) = stamps_v else {
        return Err(MSG_DAILY_SHAPE.to_string());
    };
    for c in &col_values {
        let len_c = c.map_or(Ok(0), py_len)?;
        if len_c != py_len(stamps_v)? {
            return Err(MSG_DAILY_SHAPE.to_string());
        }
    }
    let stamps = py_iter(stamps_v.clone())?;
    let mut cols = Vec::with_capacity(4);
    for c in col_values {
        cols.push(c.map_or(Ok(Vec::new()), |v| py_iter(v.clone()))?);
    }
    let mut rows: BTreeMap<Date, Candle> = BTreeMap::new();
    for (i, t) in stamps.iter().enumerate() {
        let (o, h, lo, c) = (&cols[0][i], &cols[1][i], &cols[2][i], &cols[3][i]);
        if [o, h, lo, c].iter().any(|v| v.is_null()) {
            continue;
        }
        let d = daily_day(t)?;
        if close_at(d) > now {
            continue;
        }
        rows.insert(
            d,
            Candle {
                date: iso_date(d),
                open: round2(to_float(o)?),
                high: round2(to_float(h)?),
                low: round2(to_float(lo)?),
                close: round2(to_float(c)?),
            },
        );
    }
    let out: Vec<Candle> = rows.into_values().collect();
    if out.len() < DAILY_MIN {
        return Err(format!("日 K 只有 {} 根，不足 {DAILY_MIN} 根", out.len()));
    }
    Ok(out)
}

/// 舊輸出的 `twii_daily`：**是陣列**才算（Python `isinstance(old, list)`；不檢查內部形狀、空陣列也算）。
pub fn previous_daily(prev: &PreviousOutput) -> Option<Value> {
    prev.raw("twii_daily").filter(|v| v.is_array()).cloned()
}

/// 由組裝端呼叫：[`run_daily`] 加上 panic 隔離（D7、B-GRD-1）。
pub async fn run_daily_guarded<F: Fetch>(
    fetch: &F,
    clock: &Clock,
    prev: &PreviousOutput,
) -> WallpaperOutcome<Option<Value>> {
    guarded_wallpaper(
        LABEL_DAILY,
        previous_daily(prev),
        run_daily(fetch, clock, prev),
    )
    .await
}

async fn run_daily<F: Fetch>(
    fetch: &F,
    clock: &Clock,
    prev: &PreviousOutput,
) -> WallpaperOutcome<Option<Value>> {
    let old = previous_daily(prev);
    let mut out = WallpaperOutcome::new(None);
    let parsed = match get_json(fetch, URL_DAILY).await {
        Ok(j) => parse_daily(&j, clock.tpe),
        Err(e) => Err(e.to_string()),
    };
    match parsed {
        Err(e) => {
            out.wallpaper_errors.push(format!("{ERR_PREFIX_DAILY}{e}"));
            out.logs.push(format!("[日K] 本輪失敗，沿用上次資料：{e}"));
            out.value = old;
        }
        Ok(new) => {
            // `old_last = str(old[-1].get("date") or "") if old and isinstance(old[-1], dict) else ""`
            let old_last = old
                .as_ref()
                .and_then(Value::as_array)
                .and_then(|a| a.last())
                .and_then(Value::as_object)
                .map(|o| py_str_or_empty(o.get("date")))
                .unwrap_or_default();
            // `new` 一定非空（`DAILY_MIN`）。
            let new_last = new.last().map(|c| c.date.clone()).unwrap_or_default();
            if old_last > new_last {
                out.wallpaper_errors.push(format!(
                    "加權日K來源回傳較舊的資料（最後一根 {new_last} 早於上次的 {old_last}），沿用上次資料"
                ));
                out.logs.push(format!(
                    "[日K] 來源最後一根 {new_last} 早於上次的 {old_last}，沿用上次資料"
                ));
                out.value = old;
            } else {
                out.value = Some(to_json(&new));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::errors::normalize_error;
    use crate::fetch::oracle;
    use crate::fetch::test_util::{block_on, py_fixture, MapFetch, PanicFetch};
    use serde_json::json;
    use time::format_description::well_known::Iso8601;
    use time::macros::{date, datetime};

    // ───────────── 共用小工具 ─────────────

    fn tpe(y: i32, mo: u8, d: u8, h: u8, mi: u8, s: u8) -> OffsetDateTime {
        PrimitiveDateTime::new(
            Date::from_calendar_date(y, time::Month::try_from(mo).unwrap(), d).unwrap(),
            time::Time::from_hms(h, mi, s).unwrap(),
        )
        .assume_offset(TPE_OFFSET)
    }

    fn clock_at(now: OffsetDateTime) -> Clock {
        let tpe = now.to_offset(TPE_OFFSET);
        Clock::new(tpe, PrimitiveDateTime::new(tpe.date(), tpe.time()))
    }

    fn prev_with(key: &str, old: Option<Value>) -> PreviousOutput {
        match old {
            Some(v) => PreviousOutput::from_value(json!({ key: v })),
            None => PreviousOutput::default(),
        }
    }

    fn fetch_of(url: &str, body: &Value) -> MapFetch {
        MapFetch::new().ok(url, &body.to_string())
    }

    fn run_intraday_with(
        body: &Value,
        now: OffsetDateTime,
        old: Option<Value>,
    ) -> WallpaperOutcome<Option<Value>> {
        block_on(run_intraday_guarded(
            &fetch_of(URL_INTRADAY, body),
            &clock_at(now),
            &prev_with("twii_intraday", old),
        ))
    }

    fn run_daily_with(
        body: &Value,
        now: OffsetDateTime,
        old: Option<Value>,
    ) -> WallpaperOutcome<Option<Value>> {
        block_on(run_daily_guarded(
            &fetch_of(URL_DAILY, body),
            &clock_at(now),
            &prev_with("twii_daily", old),
        ))
    }

    fn to_json_intraday(i: &Intraday) -> Value {
        serde_json::to_value(i).unwrap()
    }

    fn secs_in_tpe(t: i64) -> (Date, i64) {
        tpe_day_and_secs(t).unwrap()
    }

    fn hhmm_of(t: i64) -> String {
        hhmm(secs_in_tpe(t).1)
    }

    /// 依 `keep(台北時間)` 過濾 Yahoo chart 回應的 timestamp 與各序列（模擬「盤中那輪」）。
    fn chart_with_filter(j: &Value, keep: impl Fn(OffsetDateTime) -> bool) -> Value {
        let mut out = j.clone();
        let r = &mut out["chart"]["result"][0];
        let stamps = r["timestamp"].as_array().unwrap().clone();
        let idx: Vec<usize> = stamps
            .iter()
            .enumerate()
            .filter(|(_, t)| {
                keep(
                    OffsetDateTime::from_unix_timestamp(t.as_i64().unwrap())
                        .unwrap()
                        .to_offset(TPE_OFFSET),
                )
            })
            .map(|(i, _)| i)
            .collect();
        let pick =
            |v: &Value| -> Value { Value::Array(idx.iter().map(|&i| v[i].clone()).collect()) };
        r["timestamp"] = pick(&r["timestamp"]);
        let q = r["indicators"]["quote"][0].as_object_mut().unwrap();
        for v in q.values_mut() {
            *v = pick(v);
        }
        out
    }

    fn insert_point(j: &mut Value, at: usize, t: i64, close: f64) {
        let r = &mut j["chart"]["result"][0];
        r["timestamp"].as_array_mut().unwrap().insert(at, json!(t));
        let q = r["indicators"]["quote"][0].as_object_mut().unwrap();
        for (k, v) in q.iter_mut() {
            let x = if k == "close" { json!(close) } else { json!(0) };
            v.as_array_mut().unwrap().insert(at, x);
        }
    }

    fn timestamps(j: &Value) -> Vec<i64> {
        j["chart"]["result"][0]["timestamp"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.as_i64().unwrap())
            .collect()
    }

    fn last_ts_of_day(j: &Value, d: Date) -> i64 {
        timestamps(j)
            .into_iter()
            .filter(|t| secs_in_tpe(*t).0 == d)
            .max()
            .unwrap()
    }

    fn parse_iso(s: &str) -> OffsetDateTime {
        OffsetDateTime::parse(s, &Iso8601::DEFAULT).unwrap()
    }

    /// 與 Python `assert_valid_series` 相同的檢查。
    fn assert_valid_series(res: &Intraday, day: &str) {
        assert_eq!(res.date, day);
        let stamps: Vec<i64> = res.points.iter().map(|p| p.0).collect();
        let mut sorted = stamps.clone();
        sorted.sort_unstable();
        assert_eq!(stamps, sorted, "序列要依時間排序");
        sorted.dedup();
        assert_eq!(stamps.len(), sorted.len(), "時間點不得重複");
        for w in stamps.windows(2) {
            assert!(w[1] - w[0] <= 300, "相鄰間隔不得超過 5 分鐘");
        }
        for t in &stamps {
            assert_eq!(iso_date(secs_in_tpe(*t).0), day, "不得混入別天的點");
        }
        assert_eq!(hhmm_of(stamps[0]), "09:00");
        assert!(hhmm_of(stamps[stamps.len() - 1]).as_str() >= "13:25");
    }

    fn err_of<T: std::fmt::Debug>(r: Result<T, String>) -> String {
        r.expect_err("應該失敗")
    }

    // ───────────── 常數與文字（逐字） ─────────────

    #[test]
    fn constants_and_messages_are_verbatim() {
        assert_eq!(
            URL_INTRADAY,
            "https://query1.finance.yahoo.com/v8/finance/chart/%5ETWII?interval=5m&range=5d"
        );
        assert_eq!(
            URL_DAILY,
            "https://query1.finance.yahoo.com/v8/finance/chart/%5ETWII?interval=1d&range=3mo"
        );
        assert_eq!(OPEN_SECS, 32_400);
        assert_eq!(CLOSE_SECS, 48_600);
        assert_eq!((INTRADAY_STEP, DAILY_MIN), (300, 40));
        assert_eq!(
            (LABEL_INTRADAY, ERR_PREFIX_INTRADAY),
            ("加權盤中走勢", "加權盤中走勢來源失敗：")
        );
        assert_eq!(
            (LABEL_DAILY, ERR_PREFIX_DAILY),
            ("加權日K", "加權日K來源失敗：")
        );
    }

    // ───────────── 移植：tests/test_wallpaper_data.py::TwiiIntradayTest ─────────────

    mod twii_intraday_test {
        use super::*;

        fn chart() -> Value {
            py_fixture("yahoo_twii_5m_5d.json")
        }

        #[test]
        fn test_after_close_picks_that_day() {
            // 15:00 那輪：10/1 已過 13:30 → 當日、日期為當日
            let res = parse_intraday(&chart(), tpe(2026, 10, 1, 15, 0, 0)).unwrap();
            assert_valid_series(&res, "2026-10-01");
            assert_eq!(res.points.len(), 54);
            assert!((res.points[53].1 - 48281.21).abs() < 0.005);
        }

        #[test]
        fn test_recorded_partial_today_is_ignored() {
            // 樣本錄於 10/2 10:50，當日只有 09:00–10:27 的未收盤序列＋一筆即時報價
            let res = parse_intraday(&chart(), tpe(2026, 10, 2, 10, 50, 0)).unwrap();
            assert_valid_series(&res, "2026-10-01");
        }

        #[test]
        fn test_noon_run_does_not_overwrite() {
            // 10/1 中午 12:00 那輪：Yahoo 只有 10/1 到 12:00 的部分序列（用真實序列截斷模擬）
            let partial = chart_with_filter(&chart(), |d| {
                d.date() <= date!(2026 - 10 - 01)
                    && !(d.date() == date!(2026 - 10 - 01)
                        && u32::from(d.hour()) * 60 + u32::from(d.minute()) > 12 * 60)
            });
            let old =
                to_json_intraday(&parse_intraday(&chart(), tpe(2026, 9, 30, 15, 0, 0)).unwrap());
            assert_eq!(old["date"], "2026-09-30");
            let out = run_intraday_with(&partial, tpe(2026, 10, 1, 12, 0, 0), Some(old.clone()));
            assert_eq!(
                out.value,
                Some(old.clone()),
                "中午那輪不可用未收盤序列取代前一個完整交易日"
            );
            assert_eq!(out.value.unwrap()["date"], "2026-09-30");
            assert!(out.wallpaper_errors.is_empty());
        }

        #[test]
        fn test_afternoon_run_updates() {
            let old =
                to_json_intraday(&parse_intraday(&chart(), tpe(2026, 9, 30, 15, 0, 0)).unwrap());
            let out = run_intraday_with(&chart(), tpe(2026, 10, 1, 15, 0, 0), Some(old.clone()));
            let res = out.value.unwrap();
            assert_eq!(res["date"], "2026-10-01");
            assert_ne!(res, old);
            assert!(out.wallpaper_errors.is_empty());
        }

        #[test]
        fn test_failure_keeps_old_and_records_error() {
            let old =
                to_json_intraday(&parse_intraday(&chart(), tpe(2026, 9, 30, 15, 0, 0)).unwrap());
            let fetch = MapFetch::new().network_error(URL_INTRADAY, "連線逾時");
            let out = block_on(run_intraday_guarded(
                &fetch,
                &clock_at(tpe(2026, 10, 1, 15, 0, 0)),
                &prev_with("twii_intraday", Some(old.clone())),
            ));
            assert_eq!(out.value, Some(old));
            assert_eq!(out.wallpaper_errors.len(), 1);
            assert!(out.wallpaper_errors[0].contains("連線逾時"));
            assert!(out.wallpaper_errors[0].starts_with(ERR_PREFIX_INTRADAY));
        }

        #[test]
        fn test_failure_without_old_returns_none() {
            let fetch = MapFetch::new().network_error(URL_INTRADAY, "x");
            let out = block_on(run_intraday_guarded(
                &fetch,
                &clock_at(tpe(2026, 10, 1, 15, 0, 0)),
                &PreviousOutput::default(),
            ));
            assert_eq!(out.value, None);
            assert_eq!(out.wallpaper_errors.len(), 1);
        }

        #[test]
        fn test_unparsable_response_keeps_old() {
            let old =
                to_json_intraday(&parse_intraday(&chart(), tpe(2026, 9, 30, 15, 0, 0)).unwrap());
            let bad = json!({"chart": {"result": null, "error": {"code": "Not Found"}}});
            let out = run_intraday_with(&bad, tpe(2026, 10, 1, 15, 0, 0), Some(old.clone()));
            assert_eq!(out.value, Some(old));
            assert_eq!(out.wallpaper_errors.len(), 1);
        }

        #[test]
        fn test_live_quote_point_is_excluded() {
            // Yahoo 會在 K 之外另附一筆即時報價（時間不在 5 分鐘格線上），不可混入序列
            let mut j = chart();
            let last_1001 = last_ts_of_day(&j, date!(2026 - 10 - 01));
            let live = last_1001 + 5 * 60 - 3; // 13:29:57，離格線 3 秒
            let i = timestamps(&j).iter().position(|t| *t == last_1001).unwrap() + 1;
            insert_point(&mut j, i, live, 99999.0);
            let res = parse_intraday(&j, tpe(2026, 10, 1, 15, 0, 0)).unwrap();
            assert!(!res.points.iter().any(|p| p.0 == live));
            assert_valid_series(&res, "2026-10-01");
            assert!(!res.points.iter().any(|p| p.1 == 99999.0));
        }

        #[test]
        fn test_close_auction_point_at_1330_is_kept() {
            let mut j = chart();
            let last_1001 = last_ts_of_day(&j, date!(2026 - 10 - 01));
            let i = timestamps(&j).iter().position(|t| *t == last_1001).unwrap() + 1;
            insert_point(&mut j, i, last_1001 + 300, 48353.49); // 13:30:00
            let res = parse_intraday(&j, tpe(2026, 10, 1, 15, 0, 0)).unwrap();
            let last = res.points.last().unwrap();
            assert_eq!(hhmm_of(last.0), "13:30");
            assert_eq!(last.1, 48353.49);
        }

        #[test]
        fn test_null_close_points_are_dropped() {
            let mut j = chart();
            // 把 10/1 的最後一根改成 null：序列只剩到 13:20，13:20 → 結束缺口 → 判不完整
            let last_1001 = last_ts_of_day(&j, date!(2026 - 10 - 01));
            let i = timestamps(&j).iter().position(|t| *t == last_1001).unwrap();
            j["chart"]["result"][0]["indicators"]["quote"][0]["close"][i] = Value::Null;
            let e = err_of(parse_intraday(&j, tpe(2026, 10, 1, 15, 0, 0)));
            assert!(e.contains("太早"), "{e}");
        }

        #[test]
        fn test_stale_series_after_close_is_incomplete() {
            // 10/2 14:00：Yahoo 的 10/2 序列只到 10:27（還沒追上）→ 不可當完整日，也不可退回舊日當新資料
            let e = err_of(parse_intraday(&chart(), tpe(2026, 10, 2, 14, 0, 0)));
            assert!(
                e.contains("序列終點 10:25 太早（來源尚未追上收盤），視為不完整"),
                "{e}"
            );
            let old =
                to_json_intraday(&parse_intraday(&chart(), tpe(2026, 10, 1, 15, 0, 0)).unwrap());
            let out = run_intraday_with(&chart(), tpe(2026, 10, 2, 14, 0, 0), Some(old.clone()));
            assert_eq!(out.value, Some(old));
            assert_eq!(out.wallpaper_errors.len(), 1);
        }

        #[test]
        fn test_gap_over_five_minutes_is_rejected() {
            let mut j = chart();
            let victim = timestamps(&j)
                .into_iter()
                .find(|t| {
                    OffsetDateTime::from_unix_timestamp(*t).unwrap() == tpe(2026, 10, 1, 11, 0, 0)
                })
                .unwrap();
            let i = timestamps(&j).iter().position(|t| *t == victim).unwrap();
            j["chart"]["result"][0]["indicators"]["quote"][0]["close"][i] = Value::Null;
            let e = err_of(parse_intraday(&j, tpe(2026, 10, 1, 15, 0, 0)));
            assert_eq!(e, "2026-10-01 序列在 10:55 後缺口超過 5 分鐘");
        }

        #[test]
        fn test_weekend_uses_last_complete_trading_day() {
            // 週六執行：樣本的 10/2 未收完 → 判不完整；改用只含完整日的子集驗證「週末取最近交易日」
            let full = chart_with_filter(&chart(), |d| d.date() <= date!(2026 - 10 - 01));
            let res = parse_intraday(&full, tpe(2026, 10, 3, 9, 0, 0)).unwrap();
            assert_valid_series(&res, "2026-10-01");
        }

        #[test]
        fn test_does_not_regress_to_older_day_than_old() {
            let newer = json!({"date": "2026-10-02", "points": [[1, 1.0]]});
            let out = run_intraday_with(&chart(), tpe(2026, 10, 1, 15, 0, 0), Some(newer.clone()));
            assert_eq!(out.value, Some(newer));
            assert!(
                out.wallpaper_errors.is_empty(),
                "盤中走勢的日期倒退是靜默的（B-ID-6）"
            );
            assert!(out.logs.is_empty());
        }

        #[test]
        fn test_uses_taipei_clock_not_system_timezone() {
            // 同一個瞬間用 UTC 表示（10/1 05:00Z＝台北 13:00，未收盤）→ 仍應判 10/1 未完整、取 9/30
            let now_utc = datetime!(2026-10-01 05:00:00 UTC);
            let res = parse_intraday(&chart(), now_utc).unwrap();
            assert_eq!(res.date, "2026-09-30");
            let now_utc2 = datetime!(2026-10-01 05:30:00 UTC); // 台北 13:30 整
            assert_eq!(
                parse_intraday(&chart(), now_utc2).unwrap().date,
                "2026-10-01"
            );
            // 經 Clock：本機時間（這裡故意給很離譜的值）完全不影響
            let clock = Clock::new(now_utc, datetime!(2030-01-01 00:00:00));
            let out = block_on(run_intraday_guarded(
                &fetch_of(URL_INTRADAY, &chart()),
                &clock,
                &PreviousOutput::default(),
            ));
            assert_eq!(out.value.unwrap()["date"], "2026-09-30");
        }
    }

    // ───────────── 移植：TwiiDailyTest ─────────────

    mod twii_daily_test {
        use super::*;

        fn chart() -> Value {
            py_fixture("yahoo_twii_1d_3mo.json")
        }

        fn dates_of(res: &[Candle]) -> Vec<String> {
            res.iter().map(|c| c.date.clone()).collect()
        }

        #[test]
        fn test_enough_sorted_and_last_is_latest_closed_day() {
            let res = parse_daily(&chart(), tpe(2026, 10, 2, 10, 50, 0)).unwrap();
            assert!(res.len() >= 40);
            let dates = dates_of(&res);
            let mut sorted = dates.clone();
            sorted.sort();
            assert_eq!(dates, sorted);
            sorted.dedup();
            assert_eq!(dates.len(), sorted.len());
            // 盤中執行：排除當日（10/2）未收盤那根，最後一筆＝10/1
            assert_eq!(dates.last().unwrap(), "2026-10-01");
            let last = &res[res.len() - 1];
            // 輸出的鍵就這 5 個
            let v = serde_json::to_value(last).unwrap();
            let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
            keys.sort_unstable();
            assert_eq!(keys, ["close", "date", "high", "low", "open"]);
            assert!((last.close - 48353.49).abs() < 0.005);
            assert!((res[res.len() - 2].close - 47940.13).abs() < 0.005); // 9/30 收盤，與 MI_INDEX 一致
        }

        #[test]
        fn test_includes_today_after_close() {
            let res = parse_daily(&chart(), tpe(2026, 10, 2, 14, 0, 0)).unwrap();
            assert_eq!(res[res.len() - 1].date, "2026-10-02");
            assert_eq!(res[res.len() - 2].date, "2026-10-01");
        }

        #[test]
        fn test_excludes_today_exactly_before_1330() {
            let res = parse_daily(&chart(), tpe(2026, 10, 2, 13, 29, 0)).unwrap();
            assert_eq!(res.last().unwrap().date, "2026-10-01");
            let res = parse_daily(&chart(), tpe(2026, 10, 2, 13, 30, 0)).unwrap();
            assert_eq!(res.last().unwrap().date, "2026-10-02");
        }

        #[test]
        fn test_null_rows_are_dropped_and_duplicates_keep_last() {
            let mut j = chart();
            let r = &mut j["chart"]["result"][0];
            // 重複日（Yahoo 另附即時報價）：後者 O/H/L 為 null → 不可蓋掉完整的 K
            let second_last = {
                let ts = r["timestamp"].as_array().unwrap();
                ts[ts.len() - 2].clone()
            };
            r["timestamp"].as_array_mut().unwrap().push(second_last); // 10/1 再來一筆
            let q = r["indicators"]["quote"][0].as_object_mut().unwrap();
            for v in q.values_mut() {
                v.as_array_mut().unwrap().push(Value::Null);
            }
            let n = q["close"].as_array().unwrap().len();
            q.get_mut("close").unwrap()[n - 1] = json!(12345.0);
            let res = parse_daily(&j, tpe(2026, 10, 2, 10, 50, 0)).unwrap();
            let d1001: Vec<&Candle> = res.iter().filter(|c| c.date == "2026-10-01").collect();
            assert_eq!(d1001.len(), 1);
            assert!((d1001[0].close - 48353.49).abs() < 0.005);
        }

        #[test]
        fn test_unsorted_input_is_sorted() {
            let mut j = chart();
            let r = &mut j["chart"]["result"][0];
            r["timestamp"].as_array_mut().unwrap().reverse();
            for v in r["indicators"]["quote"][0]
                .as_object_mut()
                .unwrap()
                .values_mut()
            {
                v.as_array_mut().unwrap().reverse();
            }
            let res = parse_daily(&j, tpe(2026, 10, 2, 10, 50, 0)).unwrap();
            let dates = dates_of(&res);
            let mut sorted = dates.clone();
            sorted.sort();
            assert_eq!(dates, sorted);
            assert_eq!(dates.last().unwrap(), "2026-10-01");
        }

        #[test]
        fn test_fewer_than_40_is_rejected() {
            let j = chart_with_filter(&chart(), |d| d.date() >= date!(2026 - 09 - 01));
            let e = err_of(parse_daily(&j, tpe(2026, 10, 2, 10, 50, 0)));
            assert!(
                e.starts_with("日 K 只有 ") && e.ends_with(" 根，不足 40 根"),
                "{e}"
            );
        }

        #[test]
        fn test_failure_keeps_old_and_records_error() {
            let old = to_json(&parse_daily(&chart(), tpe(2026, 10, 1, 15, 0, 0)).unwrap());
            let fetch = MapFetch::new().network_error(URL_DAILY, "來源無回應");
            let out = block_on(run_daily_guarded(
                &fetch,
                &clock_at(tpe(2026, 10, 2, 15, 0, 0)),
                &prev_with("twii_daily", Some(old.clone())),
            ));
            assert_eq!(out.value, Some(old));
            assert_eq!(out.wallpaper_errors.len(), 1);
            assert!(out.wallpaper_errors[0].contains("來源無回應"));
        }

        #[test]
        fn test_success_replaces_old() {
            let old =
                json!([{"date": "2026-01-02", "open": 1.0, "high": 1.0, "low": 1.0, "close": 1.0}]);
            let out = run_daily_with(&chart(), tpe(2026, 10, 2, 10, 50, 0), Some(old.clone()));
            let res = out.value.unwrap();
            assert_ne!(res, old);
            assert!(res.as_array().unwrap().len() >= 40);
            assert!(out.wallpaper_errors.is_empty());
        }

        #[test]
        fn test_does_not_regress_to_older_day_than_old() {
            // Yahoo 偶發回較舊的視窗（最後一根早於上次）→ 沿用舊值並記錯誤（與盤中走勢不同）
            let old = to_json(&parse_daily(&chart(), tpe(2026, 10, 2, 14, 0, 0)).unwrap());
            assert_eq!(old[old.as_array().unwrap().len() - 1]["date"], "2026-10-02");
            let out = run_daily_with(&chart(), tpe(2026, 10, 2, 10, 50, 0), Some(old.clone()));
            assert_eq!(
                out.value,
                Some(old),
                "新抓的最後一根（10/1）早於舊值（10/2）"
            );
            assert_eq!(out.wallpaper_errors.len(), 1);
            assert!(out.wallpaper_errors[0].contains("日K"));
            assert!(out.wallpaper_errors[0].contains("2026-10-01"));
            assert_eq!(
                out.wallpaper_errors[0],
                "加權日K來源回傳較舊的資料（最後一根 2026-10-01 早於上次的 2026-10-02），沿用上次資料"
            );
            assert_eq!(
                out.logs,
                ["[日K] 來源最後一根 2026-10-01 早於上次的 2026-10-02，沿用上次資料"]
            );
        }

        #[test]
        fn test_truncated_response_keeps_old_and_records_error() {
            // 回應本體傳到一半斷線（Python 的 IncompleteRead）：Rust 是連線層錯誤或解碼錯誤
            let old = to_json(&parse_daily(&chart(), tpe(2026, 10, 1, 15, 0, 0)).unwrap());
            for fetch in [
                MapFetch::new()
                    .network_error(URL_DAILY, "IncompleteRead(3 bytes read, 97 more expected)"),
                MapFetch::new().ok(URL_DAILY, r#"{"chart": {"result": [{"timestamp": [1, 2"#),
            ] {
                let out = block_on(run_daily_guarded(
                    &fetch,
                    &clock_at(tpe(2026, 10, 2, 15, 0, 0)),
                    &prev_with("twii_daily", Some(old.clone())),
                ));
                assert_eq!(out.value, Some(old.clone()));
                assert_eq!(out.wallpaper_errors.len(), 1);
                assert!(out.wallpaper_errors[0].starts_with(ERR_PREFIX_DAILY));
            }
        }
    }

    // ───────────── 移植：MalformedOldTest（盤中、日 K 兩個） ─────────────

    mod malformed_old_test {
        use super::*;

        #[test]
        fn test_intraday_malformed_old() {
            let chart = py_fixture("yahoo_twii_5m_5d.json");
            for bad in [json!(["x"]), json!("字串"), json!(5), json!([])] {
                let out = run_intraday_with(&chart, tpe(2026, 10, 1, 15, 0, 0), Some(bad.clone()));
                assert_eq!(out.value.unwrap()["date"], "2026-10-01", "{bad}");
                let fetch = MapFetch::new().network_error(URL_INTRADAY, "x");
                let out = block_on(run_intraday_guarded(
                    &fetch,
                    &clock_at(tpe(2026, 10, 1, 15, 0, 0)),
                    &prev_with("twii_intraday", Some(bad.clone())),
                ));
                assert_eq!(out.value, None, "{bad}");
            }
        }

        #[test]
        fn test_daily_malformed_old() {
            let chart = py_fixture("yahoo_twii_1d_3mo.json");
            for bad in [json!({"a": 1}), json!("字串"), json!(5)] {
                let out = run_daily_with(&chart, tpe(2026, 10, 2, 10, 50, 0), Some(bad.clone()));
                assert!(out.value.unwrap().as_array().unwrap().len() >= 40, "{bad}");
                let fetch = MapFetch::new().network_error(URL_DAILY, "x");
                let out = block_on(run_daily_guarded(
                    &fetch,
                    &clock_at(tpe(2026, 10, 2, 10, 50, 0)),
                    &prev_with("twii_daily", Some(bad.clone())),
                ));
                assert_eq!(out.value, None, "{bad}");
            }
        }
    }

    // ───────────── 移植：OutputShapeTest（盤中、日 K 部分） ─────────────

    #[test]
    fn test_new_keys_are_json_serializable_twii() {
        let intraday = parse_intraday(
            &py_fixture("yahoo_twii_5m_5d.json"),
            tpe(2026, 10, 1, 15, 0, 0),
        )
        .unwrap();
        let daily = parse_daily(
            &py_fixture("yahoo_twii_1d_3mo.json"),
            tpe(2026, 10, 2, 10, 50, 0),
        )
        .unwrap();
        let payload = json!({"twii_intraday": intraday, "twii_daily": daily});
        let text = serde_json::to_string(&payload).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), payload);
        // points 是 [int, float]（B-ID-5）
        let p = &payload["twii_intraday"]["points"][0];
        assert!(p[0].is_i64() && p[1].is_f64(), "{p}");
    }

    // ───────────── 行為條目 → 測試 ─────────────

    #[test]
    fn b_id_2_grid_hours_and_null_filters_on_a_synthetic_day() {
        // 09:00、13:30 兩端保留；08:55、13:35 丟棄；格線外丟棄；null 丟棄
        let d = date!(2026 - 10 - 01);
        let t = |h: u8, m: u8, s: u8| tpe(2026, 10, 1, h, m, s).unix_timestamp();
        let mut stamps: Vec<i64> = (0..54).map(|i| t(9, 0, 0) + i * 300).collect();
        stamps.insert(0, t(8, 55, 0));
        stamps.push(t(13, 30, 0));
        stamps.push(t(13, 35, 0));
        stamps.push(t(13, 20, 1));
        let closes: Vec<Value> = (0..stamps.len()).map(|i| json!(100.0 + i as f64)).collect();
        let body = json!({"chart": {"result": [{"timestamp": stamps, "indicators": {"quote": [{"close": closes}]}}]}});
        let res = parse_intraday(&body, tpe(2026, 10, 1, 15, 0, 0)).unwrap();
        assert_eq!(res.date, iso_date(d));
        assert_eq!(res.points.len(), 55);
        assert_eq!(hhmm_of(res.points[0].0), "09:00");
        assert_eq!(hhmm_of(res.points[54].0), "13:30");
    }

    #[test]
    fn b_id_5_prices_are_round2_like_python_round_k4() {
        // Python `round(x, 2)`：0.125→0.12（恰好二進位 .5 取偶）、2.675→2.67（二進位略小）、1.005→1.0
        let t0 = tpe(2026, 10, 1, 9, 0, 0).unix_timestamp();
        let stamps: Vec<i64> = (0..54).map(|i| t0 + i * 300).collect();
        let mut closes: Vec<Value> = (0..54).map(|_| json!(100.0)).collect();
        for (i, v) in [0.125, 2.675, 1.005, 0.375, 2.5, 8.345, 0.005, 1e-9]
            .iter()
            .enumerate()
        {
            closes[i] = json!(v);
        }
        let body = json!({"chart": {"result": [{"timestamp": stamps, "indicators": {"quote": [{"close": closes}]}}]}});
        let res = parse_intraday(&body, tpe(2026, 10, 1, 15, 0, 0)).unwrap();
        let got: Vec<f64> = res.points.iter().take(8).map(|p| p.1).collect();
        assert_eq!(got, [0.12, 2.67, 1.0, 0.38, 2.5, 8.35, 0.01, 0.0]);
    }

    #[test]
    fn b_id_3_now_is_compared_as_an_instant_to_1330_taipei() {
        let chart = py_fixture("yahoo_twii_5m_5d.json");
        // 13:29:59 不算收盤；13:30:00 算（`<=`）
        assert_eq!(
            parse_intraday(&chart, tpe(2026, 10, 1, 13, 29, 59))
                .unwrap()
                .date,
            "2026-09-30"
        );
        assert_eq!(
            parse_intraday(&chart, tpe(2026, 10, 1, 13, 30, 0))
                .unwrap()
                .date,
            "2026-10-01"
        );
    }

    #[test]
    fn b_dk_2_ohlc_are_round2_and_rows_are_ascending_with_full_window() {
        let res = parse_daily(
            &py_fixture("yahoo_twii_1d_3mo.json"),
            tpe(2026, 10, 2, 10, 50, 0),
        )
        .unwrap();
        // 樣本 65 筆原始列：63 個已收盤日＋10/2 盤中那根（排除）＋7/10 一筆 OHLC 為 null（丟棄）→ 保留整個窗口
        assert_eq!(res.len(), 63);
        for c in &res {
            for v in [c.open, c.high, c.low, c.close] {
                assert_eq!(v, round2(v), "{c:?}");
            }
        }
    }

    // ───────────── 型別層保證（B-GRD-2、K-10） ─────────────

    #[test]
    fn b_grd_2_the_wallpaper_outcome_type_has_no_errors_or_fresh_field() {
        // 結構解構：若將來有人加了 `errors`／`fresh` 欄位，這裡會編譯失敗、提醒回頭檢查 B-GRD-2。
        let WallpaperOutcome {
            value: _,
            wallpaper_errors: _,
            logs: _,
        } = run_daily_with(
            &py_fixture("yahoo_twii_1d_3mo.json"),
            tpe(2026, 10, 2, 14, 0, 0),
            None,
        );
    }

    // ───────────── D7：panic 隔離（B-GRD-1） ─────────────

    #[test]
    fn d7_a_panic_in_the_intraday_source_keeps_the_old_value_and_goes_to_wallpaper_errors() {
        let old = json!({"date": "2026-09-30", "points": [[1, 1.0]]});
        let out = block_on(run_intraday_guarded(
            &PanicFetch,
            &clock_at(tpe(2026, 10, 1, 15, 0, 0)),
            &prev_with("twii_intraday", Some(old.clone())),
        ));
        assert_eq!(out.value, Some(old));
        assert_eq!(out.wallpaper_errors.len(), 1);
        assert!(out.wallpaper_errors[0].starts_with("加權盤中走勢來源發生非預期錯誤：panic: "));
        assert_eq!(
            normalize_error(&out.wallpaper_errors[0]),
            "加權盤中走勢來源發生非預期錯誤："
        );
        assert_eq!(out.logs.len(), 1);
    }

    #[test]
    fn d7_a_panic_in_the_daily_source_keeps_the_old_value_and_goes_to_wallpaper_errors() {
        let old = json!([{"date": "2026-09-30"}]);
        let out = block_on(run_daily_guarded(
            &PanicFetch,
            &clock_at(tpe(2026, 10, 1, 15, 0, 0)),
            &prev_with("twii_daily", Some(old.clone())),
        ));
        assert_eq!(out.value, Some(old));
        assert!(out.wallpaper_errors[0].starts_with("加權日K來源發生非預期錯誤：panic: "));
    }

    #[test]
    fn d7_a_panic_without_a_usable_old_value_omits_the_key() {
        // 舊值型別不對（盤中要物件、日 K 要陣列）＝沒有舊值（B-GRD-1：`old if isinstance(old, old_type) else None`）
        let out = block_on(run_intraday_guarded(
            &PanicFetch,
            &clock_at(tpe(2026, 10, 1, 15, 0, 0)),
            &prev_with("twii_intraday", Some(json!([1]))),
        ));
        assert_eq!(out.value, None);
        let out = block_on(run_daily_guarded(
            &PanicFetch,
            &clock_at(tpe(2026, 10, 1, 15, 0, 0)),
            &prev_with("twii_daily", Some(json!({"a": 1}))),
        ));
        assert_eq!(out.value, None);
        assert_eq!(out.wallpaper_errors.len(), 1);
    }

    #[test]
    fn d7_run_guarded_is_transparent_when_nothing_panics() {
        let out = run_daily_with(
            &py_fixture("yahoo_twii_1d_3mo.json"),
            tpe(2026, 10, 2, 14, 0, 0),
            None,
        );
        assert!(out.value.is_some() && out.wallpaper_errors.is_empty() && out.logs.is_empty());
    }

    // ───────────── 與 Python 的對照表（真的呼叫 update_tw_events 產生，見 testdata） ─────────────

    fn table() -> Value {
        serde_json::from_str(include_str!("testdata/twii_python_cases.json"))
            .expect("twii_python_cases.json 是合法 JSON")
    }

    fn opt(v: &Value) -> Option<Value> {
        if v.is_null() {
            None
        } else {
            Some(v.clone())
        }
    }

    fn is_deviation(name: &str) -> bool {
        name.starts_with("d8_6_")
    }

    /// Python 回傳的是「例外型別為 ValueError」且訊息逐字可比的案例，要求錯誤訊息逐字相同。
    fn exact_message_expected(case: &Value) -> bool {
        case["exc"][0] == "ValueError"
    }

    fn check_case(kind: &str, case: &Value, prefix: &str) {
        let name = case["name"].as_str().unwrap();
        let body = &case["body"];
        let now = parse_iso(case["now"].as_str().unwrap());
        let old = case.get("old").cloned();
        let out = if kind == "intraday" {
            run_intraday_with(body, now, old.clone())
        } else {
            run_daily_with(body, now, old.clone())
        };
        if is_deviation(name) {
            // D8-6：Python 成功（或產生非有限值），Rust 一律視為該來源失敗＝沿用舊值＋一筆錯誤。
            assert_eq!(
                out.wallpaper_errors.len(),
                1,
                "{kind}/{name}: {:?}",
                out.wallpaper_errors
            );
            assert!(out.wallpaper_errors[0].starts_with(prefix), "{kind}/{name}");
            let kept = old.filter(|o| {
                if kind == "intraday" {
                    o.is_object()
                } else {
                    o.is_array()
                }
            });
            assert_eq!(out.value, kept, "{kind}/{name}");
            return;
        }
        assert_eq!(out.value, opt(&case["value"]), "{kind}/{name}: value");
        let expected: Vec<String> = case["errors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| normalize_error(e.as_str().unwrap()))
            .collect();
        let got: Vec<String> = out
            .wallpaper_errors
            .iter()
            .map(|e| normalize_error(e))
            .collect();
        assert_eq!(got, expected, "{kind}/{name}: errors");
        if exact_message_expected(case) && !expected.is_empty() {
            assert_eq!(
                out.wallpaper_errors,
                case["errors"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|e| e.as_str().unwrap().to_string())
                    .collect::<Vec<_>>(),
                "{kind}/{name}: 錯誤訊息逐字"
            );
        }
    }

    #[test]
    fn python_cross_check_intraday_table_matches_the_real_functions() {
        let t = table();
        let cases = t["intraday"].as_array().unwrap();
        assert!(cases.len() >= 90, "{}", cases.len());
        for c in cases {
            check_case("intraday", c, ERR_PREFIX_INTRADAY);
        }
    }

    #[test]
    fn python_cross_check_daily_table_matches_the_real_functions() {
        let t = table();
        let cases = t["daily"].as_array().unwrap();
        assert!(cases.len() >= 60, "{}", cases.len());
        for c in cases {
            check_case("daily", c, ERR_PREFIX_DAILY);
        }
    }

    #[test]
    fn python_cross_check_error_messages_that_differ_from_python_are_only_the_known_kinds() {
        // 逐字比對之外的案例必須屬於已知類型；新增案例造成未知差異會在這裡現形。
        let t = table();
        let mut differing = Vec::new();
        for (kind, prefix) in [("intraday", "加權盤中走勢來源"), ("daily", "加權日K來源")]
        {
            for case in t[kind].as_array().unwrap() {
                let name = case["name"].as_str().unwrap();
                // 平台相關：Windows 上 Python 的 `fromtimestamp` 對超出範圍的年份回 `[Errno 22] Invalid argument`。
                if is_deviation(name) || name.starts_with("t_year_out_of_range") {
                    continue;
                }
                let now = parse_iso(case["now"].as_str().unwrap());
                let old = case.get("old").cloned();
                let out = if kind == "intraday" {
                    run_intraday_with(&case["body"], now, old)
                } else {
                    run_daily_with(&case["body"], now, old)
                };
                let expected: Vec<String> = case["errors"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|e| e.as_str().unwrap().to_string())
                    .collect();
                assert!(expected.iter().all(|e| e.starts_with(prefix)));
                if out.wallpaper_errors != expected {
                    differing.push(format!(
                        "{kind}/{name}\n  py={expected:?}\n  rs={:?}",
                        out.wallpaper_errors
                    ));
                }
            }
        }
        assert!(differing.is_empty(), "{}", differing.join("\n"));
    }

    #[test]
    fn python_cross_check_table_has_the_evidence_cases() {
        // 防呆：表格沒有被改空、關鍵案例都還在（避免整張表變成空轉）。
        let t = table();
        let names = |k: &str| -> Vec<String> {
            t[k].as_array()
                .unwrap()
                .iter()
                .map(|c| c["name"].as_str().unwrap().to_string())
                .collect()
        };
        for want in [
            "ok_single_full_day",
            "now_one_second_before_1330_not_closed",
            "old_newer_kept_silently",
            "d8_6_bool_close",
            "t_year_out_of_range_on_grid",
        ] {
            assert!(names("intraday").iter().any(|n| n == want), "{want}");
        }
        for want in [
            "ok_exactly_40",
            "only_39_rejected",
            "old_newer_kept_with_error",
            "d8_6_bool_ohlc",
        ] {
            assert!(names("daily").iter().any(|n| n == want), "{want}");
        }
        // 至少有成功與失敗兩種結果
        let ok = t["intraday"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| !c["value"].is_null() && c["errors"].as_array().unwrap().is_empty())
            .count();
        assert!(ok >= 20, "{ok}");
    }

    // ───────────── 結構性單元測試 ─────────────

    #[test]
    fn tpe_day_and_secs_matches_python_fromtimestamp_in_range_and_rejects_out_of_range() {
        assert_eq!(
            tpe_day_and_secs(tpe(2026, 10, 1, 9, 5, 0).unix_timestamp()).unwrap(),
            (date!(2026 - 10 - 01), 9 * 3600 + 300)
        );
        // 16:00 UTC＝隔天 00:00 台北
        assert_eq!(
            tpe_day_and_secs(datetime!(2026-09-30 16:00:00 UTC).unix_timestamp()).unwrap(),
            (date!(2026 - 10 - 01), 0)
        );
        // 負值（1970 年之前）也能換算
        assert_eq!(
            tpe_day_and_secs(-1).unwrap(),
            (date!(1970 - 01 - 01), 8 * 3600 - 1)
        );
        assert!(tpe_day_and_secs(300 * 100_000_000_000).is_err());
        assert!(tpe_day_and_secs(i64::MAX).is_err());
        assert!(tpe_day_and_secs(i64::MIN).is_err());
    }

    #[test]
    fn hhmm_matches_strftime() {
        assert_eq!(hhmm(0), "00:00");
        assert_eq!(hhmm(9 * 3600 + 5 * 60), "09:05");
        assert_eq!(hhmm(13 * 3600 + 30 * 60), "13:30");
    }

    #[test]
    fn previous_values_only_need_the_right_container_type() {
        let p = PreviousOutput::from_value(json!({
            "twii_intraday": {}, "twii_daily": [], "other": 1
        }));
        assert_eq!(previous_intraday(&p), Some(json!({})));
        assert_eq!(previous_daily(&p), Some(json!([])));
        let p = PreviousOutput::from_value(json!({"twii_intraday": [], "twii_daily": {}}));
        assert_eq!(previous_intraday(&p), None);
        assert_eq!(previous_daily(&p), None);
        assert_eq!(previous_intraday(&PreviousOutput::default()), None);
    }

    // ───────────── oracle ─────────────

    fn expected_wallpaper_errors(expected: &Value, prefix: &str) -> Vec<String> {
        expected["wallpaper_errors"]
            .as_array()
            .map(|a| a.as_slice())
            .unwrap_or_default()
            .iter()
            .filter_map(Value::as_str)
            .filter(|m| m.starts_with(prefix))
            .map(normalize_error)
            .collect()
    }

    fn check_wallpaper_errors(name: &str, got: &[String], expected: &Value, prefix: &str) {
        let got: Vec<String> = got.iter().map(|m| normalize_error(m)).collect();
        assert_eq!(
            got,
            expected_wallpaper_errors(expected, prefix),
            "情境 {name}：wallpaper_errors 不符"
        );
    }

    #[test]
    fn oracle_every_scenario_matches_python_expected_json() {
        let mut n = 0;
        oracle::for_each_scenario(|name, sc, expected| {
            n += 1;
            let prev = sc.previous();
            let i = block_on(run_intraday_guarded(&sc.fetch, &sc.clock, &prev));
            assert_eq!(
                i.value,
                expected.get("twii_intraday").cloned(),
                "情境 {name}：twii_intraday"
            );
            check_wallpaper_errors(name, &i.wallpaper_errors, expected, "加權盤中走勢來源");
            let d = block_on(run_daily_guarded(&sc.fetch, &sc.clock, &prev));
            assert_eq!(
                d.value,
                expected.get("twii_daily").cloned(),
                "情境 {name}：twii_daily"
            );
            check_wallpaper_errors(name, &d.wallpaper_errors, expected, "加權日K來源");
            // 桌布錯誤不得出現在 errors（B-GRD-2）
            for e in expected["errors"].as_array().unwrap() {
                let e = e.as_str().unwrap();
                assert!(
                    !e.starts_with("加權盤中走勢") && !e.starts_with("加權日K"),
                    "情境 {name}：{e}"
                );
            }
        });
        assert!(n >= 30, "情境數 {n}");
    }

    #[test]
    fn oracle_noon_and_afternoon_scenarios_show_the_key_evidence() {
        let noon = oracle_run("twii-intraday-1200");
        let i = noon.0.value.unwrap();
        assert_eq!(i["date"], "2026-10-01");
        let pts = i["points"].as_array().unwrap();
        assert_eq!(pts.len(), 54, "一個完整日 54 點");
        assert_eq!(hhmm_of(pts[0][0].as_i64().unwrap()), "09:00");
        assert_eq!(hhmm_of(pts[53][0].as_i64().unwrap()), "13:25");
        let d = noon.1.value.unwrap();
        assert_eq!(
            d[d.as_array().unwrap().len() - 1]["date"],
            "2026-10-01",
            "10/2 當日那根被丟棄"
        );
        assert!(noon.0.wallpaper_errors.is_empty() && noon.1.wallpaper_errors.is_empty());

        let pm = oracle_run("twii-intraday-1500");
        let i = pm.0.value.unwrap();
        assert_eq!(i["date"], "2026-10-02");
        let pts = i["points"].as_array().unwrap();
        assert_eq!(pts.len(), 55, "含 13:30 收盤點");
        assert_eq!(hhmm_of(pts[54][0].as_i64().unwrap()), "13:30");
        let d = pm.1.value.unwrap();
        let last = &d[d.as_array().unwrap().len() - 1];
        assert_eq!(last["date"], "2026-10-02");
        assert_eq!(last["close"], 48475.74);
        assert!(pm.0.wallpaper_errors.is_empty() && pm.1.wallpaper_errors.is_empty());
    }

    fn oracle_run(
        name: &str,
    ) -> (
        WallpaperOutcome<Option<Value>>,
        WallpaperOutcome<Option<Value>>,
    ) {
        let sc = crate::fetch::fixture::Scenario::named(name).unwrap();
        let prev = sc.previous();
        (
            block_on(run_intraday_guarded(&sc.fetch, &sc.clock, &prev)),
            block_on(run_daily_guarded(&sc.fetch, &sc.clock, &prev)),
        )
    }

    #[test]
    fn oracle_regression_incomplete_and_date_regression_scenarios_show_the_key_evidence() {
        // 日期倒退：日 K 記錯誤、盤中靜默；兩者都沿用舊檔
        let sc = crate::fetch::fixture::Scenario::named("twii-date-regression").unwrap();
        let old = sc.previous();
        let (i, d) = oracle_run("twii-date-regression");
        assert_eq!(i.value.as_ref(), old.raw("twii_intraday"));
        assert_eq!(i.value.as_ref().unwrap()["date"], "2026-10-06");
        assert!(i.wallpaper_errors.is_empty(), "盤中走勢的倒退沒有錯誤訊息");
        assert_eq!(d.value.as_ref(), old.raw("twii_daily"));
        assert_eq!(
            d.wallpaper_errors,
            ["加權日K來源回傳較舊的資料（最後一根 2026-10-02 早於上次的 2026-10-06），沿用上次資料"]
        );
        // 不完整：記錯誤並沿用舊檔
        let sc =
            crate::fetch::fixture::Scenario::named("twii-intraday-incomplete-keeps-old").unwrap();
        let old = sc.previous();
        let (i, _) = oracle_run("twii-intraday-incomplete-keeps-old");
        assert_eq!(i.value.as_ref(), old.raw("twii_intraday"));
        assert_eq!(i.value.as_ref().unwrap()["date"], "2026-10-01");
        assert_eq!(i.wallpaper_errors.len(), 1);
        assert!(i.wallpaper_errors[0].starts_with("加權盤中走勢來源失敗：2026-10-02 序列終點"));
        assert!(i.wallpaper_errors[0].contains("太早（來源尚未追上收盤）"));
    }
}
