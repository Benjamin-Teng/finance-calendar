//! Python 真值與型別語意的小工具（各來源移植 `x or ""`、`str(x)` 這類寫法時共用）。

use serde_json::{Map, Value};
use time::Date;

use super::dates::{digit_value, is_decimal_digit, is_py_space, roc_to_date};

/// Python 的 `bool(x)`：`null`、`false`、`0`／`0.0`、空字串、空陣列、空物件為假，其餘為真。
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Python 的 `str(x)`（只求「是否含某子字串」「是否為數字字串」夠用的近似）：字串原樣、`null`→`None`、
/// 布林→`True`／`False`、數字取 JSON 文字，陣列／物件取 JSON 文字（Python 是 repr，內含的字串仍是子字串）。
pub fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "None".to_string(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        other => other.to_string(),
    }
}

/// Python 的 `type(x).__name__`（只用在錯誤訊息，對照 Python 的 `'NoneType' object is not iterable`）。
pub fn py_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// Python 的 `for x in v`：陣列逐元素、物件逐鍵（字串）、字串逐字元（各一個字元的字串）；其餘
/// （`null`、布林、數字）是 `TypeError`。逐項取用時各自再失敗（如 `x.get` 對字串）由呼叫端處理。
///
/// 注意：物件逐鍵的順序是**鍵的排序序**（`serde_json` 未開 `preserve_order`，`Map` 是 BTreeMap），
/// 不是 Python 的插入順序；只能用在順序不影響結果的地方（目前的呼叫端：物件頂層的每個鍵元素不是被略過、
/// 就是讓整個來源失敗）。
pub fn py_iter(v: Value) -> Result<Vec<Value>, String> {
    match v {
        Value::Array(items) => Ok(items),
        Value::Object(map) => Ok(map.into_iter().map(|(k, _)| Value::String(k)).collect()),
        Value::String(s) => Ok(s.chars().map(|c| Value::String(c.to_string())).collect()),
        other => Err(format!("'{}' object is not iterable", py_type_name(&other))),
    }
}

/// Python 的 `x.get(...)` 前提：只有 `dict`（JSON 物件）有 `.get`；其餘是 `AttributeError`。
pub fn py_dict(v: &Value) -> Result<&Map<String, Value>, String> {
    v.as_object()
        .ok_or_else(|| format!("'{}' object has no attribute 'get'", py_type_name(v)))
}

/// Python 的 `x[i]`（非負整數索引）：陣列取第 i 個、字串取第 i 個字元；物件（鍵是字串、沒有整數鍵）
/// 是 `KeyError`；`null`／布林／數字不可下標。越界是 `IndexError`。
pub fn py_index(v: &Value, i: usize) -> Result<Value, String> {
    match v {
        Value::Array(a) => a
            .get(i)
            .cloned()
            .ok_or_else(|| "list index out of range".to_string()),
        Value::String(s) => s
            .chars()
            .nth(i)
            .map(|c| Value::String(c.to_string()))
            .ok_or_else(|| "string index out of range".to_string()),
        Value::Object(_) => Err(i.to_string()),
        other => Err(format!(
            "'{}' object is not subscriptable",
            py_type_name(other)
        )),
    }
}

/// `str(obj.get(key, ""))`：鍵不存在＝空字串；存在（含 `null`→`None`）一律 [`py_str`]。
pub fn py_str_get(obj: &Map<String, Value>, key: &str) -> String {
    obj.get(key).map(py_str).unwrap_or_default()
}

/// `str(x or "")`：假值＝空字串，否則 [`py_str`]。
pub fn py_str_or_empty(v: Option<&Value>) -> String {
    match v {
        Some(v) if truthy(v) => py_str(v),
        _ => String::new(),
    }
}

/// Python `str.strip()`。
pub fn py_strip(s: &str) -> &str {
    s.trim_matches(is_py_space)
}

/// 與 Python `float(str)` 一致的字串轉浮點：先去頭尾空白（**只有** ASCII 空白與 Unicode 空白——`float()`
/// 不像 `str.strip()`，不認 U+001C 到 U+001F 這四個控制字元（實測 float 對它們是 `ValueError`）；要 `strip()` 語意的呼叫端
/// 先自己 [`py_strip`]）；Unicode 十進位數字當
/// ASCII 數字；底線只准夾在兩個數字之間（`1_000` 可、`1__0`／`_1`／`1_`／`1_.5` 不可）；其餘交給
/// Rust 的 `f64::from_str`（兩者都收 `inf`／`infinity`／`nan` 的任意大小寫與正負號、`1e3`、`.5`、`5.`）。
/// 回傳 `None`＝Python 的 `ValueError`。可能回傳 `inf`／`nan`（呼叫端依該欄位的語意處理）。
pub fn py_float(s: &str) -> Option<f64> {
    let chars: Vec<char> = s.trim_matches(char::is_whitespace).chars().collect();
    let mut norm = String::with_capacity(chars.len());
    for (i, &c) in chars.iter().enumerate() {
        if c == '_' {
            let between_digits = i > 0
                && is_decimal_digit(chars[i - 1])
                && chars.get(i + 1).is_some_and(|&n| is_decimal_digit(n));
            if !between_digits {
                return None;
            }
        } else if c.is_ascii() {
            norm.push(c);
        } else if is_decimal_digit(c) {
            norm.push(char::from_digit(digit_value(c), 10)?);
        } else {
            return None;
        }
    }
    norm.parse::<f64>().ok()
}

/// Python 的 `roc_to_date(s)`（`update_tw_events.py`）：假值＝`None`；其餘 `str(s).strip()` 後交給
/// [`roc_to_date`]。
pub fn roc_to_date_value(v: Option<&Value>) -> Option<Date> {
    match v {
        Some(v) if truthy(v) => roc_to_date(&py_str(v)),
        _ => None,
    }
}

/// Python `int(str)`（呼叫端已 `strip()`）：可選正負號、十進位數字（含 Unicode Nd）、底線只准夾在兩個數字之間。
/// 回傳 `None`＝`ValueError`（超過 `i128` 的巨大數字也當 `None`，反正落在任何欄位的合理範圍外）。
pub fn py_int(s: &str) -> Option<i128> {
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

/// Python 的 `repr(x)`，**只供錯誤訊息使用**（B-HTTP-7：訊息內文無消費端解析）：`None`／`True`／`False`、
/// 字串的單引號寫法（含單引號不含雙引號時改用雙引號；反斜線、換行、控制字元與非 ASCII 空白跳脫）、陣列 `[a, b]`、物件
/// `{'k': v}`。與 Python 的差異：物件的鍵依字母序（`serde_json` 的 `Map` 是 BTreeMap，不是插入序）、
/// 浮點取 serde 的文字。
pub fn py_repr(v: &Value) -> String {
    match v {
        Value::Null => "None".to_string(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => py_repr_str(s),
        Value::Array(a) => format!("[{}]", a.iter().map(py_repr).collect::<Vec<_>>().join(", ")),
        Value::Object(o) => format!(
            "{{{}}}",
            o.iter()
                .map(|(k, v)| format!("{}: {}", py_repr_str(k), py_repr(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Python 的 `repr(str)`。
pub fn py_repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            // Python 的 `isprintable()` 為假者跳脫（近似：控制字元與非 ASCII 空白）。
            c if c.is_control() || (c.is_whitespace() && c != ' ') => {
                let n = c as u32;
                if n <= 0xff {
                    out.push_str(&format!("\\x{n:02x}"));
                } else if n <= 0xffff {
                    out.push_str(&format!("\\u{n:04x}"));
                } else {
                    out.push_str(&format!("\\U{n:08x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn truthiness_matches_python() {
        for falsy in [
            json!(null),
            json!(false),
            json!(0),
            json!(0.0),
            json!(""),
            json!([]),
            json!({}),
        ] {
            assert!(!truthy(&falsy), "{falsy}");
        }
        for t in [
            json!(true),
            json!(1),
            json!(-0.5),
            json!(" "),
            json!("0"),
            json!([0]),
            json!({"a": null}),
        ] {
            assert!(truthy(&t), "{t}");
        }
    }

    #[test]
    fn py_str_matches_python_for_scalars() {
        assert_eq!(py_str(&json!("a")), "a");
        assert_eq!(py_str(&json!(null)), "None");
        assert_eq!(py_str(&json!(true)), "True");
        assert_eq!(py_str(&json!(1150101)), "1150101");
        assert_eq!(py_str(&json!(1.5)), "1.5");
    }

    // ───── py_float：期望值來自真的跑 Python `float(s)`（Python 3.14.6） ─────

    /// 期望字串：`"nan"`／`"inf"`／`"-inf"` 代表非有限值，其餘是 Python `repr(float)`（Rust 同樣能解析）。
    #[test]
    fn py_float_matches_python_float_table() {
        let cases: &[(&str, Option<&str>)] = &[
            ("0", Some("0.0")),
            ("1", Some("1.0")),
            ("-1", Some("-1.0")),
            ("+1", Some("1.0")),
            ("1.5", Some("1.5")),
            (".5", Some("0.5")),
            ("5.", Some("5.0")),
            ("1e3", Some("1000.0")),
            ("1E3", Some("1000.0")),
            ("1e-3", Some("0.001")),
            ("1e+3", Some("1000.0")),
            ("1e", None),
            ("e1", None),
            ("+.5e-3", Some("0.0005")),
            ("-.e1", None),
            (".", None),
            ("-", None),
            ("+", None),
            ("1.e5", Some("100000.0")),
            ("inf", Some("inf")),
            ("-inf", Some("-inf")),
            ("+inf", Some("inf")),
            ("INF", Some("inf")),
            ("Infinity", Some("inf")),
            ("-infinity", Some("-inf")),
            ("infinit", None),
            ("nan", Some("nan")),
            ("-nan", Some("nan")),
            ("NaN", Some("nan")),
            ("+nan", Some("nan")),
            ("nan1", None),
            ("in f", None),
            ("1_0", Some("10.0")),
            ("1__0", None),
            ("_1", None),
            ("1_", None),
            ("1_.5", None),
            ("1._5", None),
            ("1_e5", None),
            ("1e_5", None),
            ("1e1_0", Some("10000000000.0")),
            ("1_0.0_1", Some("10.01")),
            ("+_1", None),
            ("-1_0", Some("-10.0")),
            (" 2 ", Some("2.0")),
            ("\u{9}2\u{a}", Some("2.0")),
            ("\u{a0}2\u{a0}", Some("2.0")),
            ("\u{3000}2\u{3000}", Some("2.0")),
            ("\u{1f}3\u{1f}", None),
            ("\u{200b}2", None),
            ("2\u{200b}", None),
            ("\u{0}2", None),
            ("\u{ff11}\u{ff12}", Some("12.0")),
            ("\u{ff11}\u{ff0e}\u{ff15}", None),
            ("\u{ff11}e\u{ff12}", Some("100.0")),
            ("\u{663}", Some("3.0")),
            ("\u{663}.\u{665}", Some("3.5")),
            ("\u{967}\u{968}", Some("12.0")),
            ("1,5", None),
            ("1 5", None),
            ("0x10", None),
            ("0b1", None),
            ("1e400", Some("inf")),
            ("-1e400", Some("-inf")),
            ("1e-400", Some("0.0")),
            ("00012", Some("12.0")),
            ("0.0000001", Some("1e-07")),
            (
                "123456789012345678901234567890",
                Some("1.2345678901234568e+29"),
            ),
            ("1.5e+", None),
            ("1.5e-", None),
            ("--1", None),
            ("++1", None),
            ("+-1", None),
            ("1.2.3", None),
            ("\u{ff11}_\u{ff10}", Some("10.0")),
            ("1_\u{663}", Some("13.0")),
            ("\u{661}\u{662}", Some("12.0")),
            ("1\u{660}", Some("10.0")),
            ("\u{2460}", None),
            ("\u{b2}", None),
            ("1\u{b2}", None),
            ("", None),
            ("  ", None),
        ];
        for (input, want) in cases {
            let got = py_float(input);
            match (want, got) {
                (None, None) => {}
                (Some("nan"), Some(v)) => assert!(v.is_nan(), "輸入 {input:?}"),
                (Some(w), Some(v)) => {
                    let w: f64 = w.parse().expect("期望值是浮點字串");
                    assert_eq!(v.to_bits(), w.to_bits(), "輸入 {input:?}");
                }
                (w, g) => panic!("輸入 {input:?}：Python {w:?}、Rust {g:?}"),
            }
        }
    }

    #[test]
    fn py_iter_follows_python_for_loops() {
        assert_eq!(
            py_iter(json!([1, "a"])).unwrap(),
            vec![json!(1), json!("a")]
        );
        // 物件逐鍵、字串逐字元（元素都是字串）。
        assert_eq!(py_iter(json!({"k": 1})).unwrap(), vec![json!("k")]);
        assert_eq!(py_iter(json!("ab")).unwrap(), vec![json!("a"), json!("b")]);
        assert!(py_iter(json!({})).unwrap().is_empty());
        assert!(py_iter(json!("")).unwrap().is_empty());
        for bad in [json!(null), json!(0), json!(1.5), json!(true)] {
            assert!(py_iter(bad.clone()).is_err(), "{bad}");
        }
        assert_eq!(
            py_iter(json!(null)).unwrap_err(),
            "'NoneType' object is not iterable"
        );
        assert_eq!(
            py_iter(json!(5)).unwrap_err(),
            "'int' object is not iterable"
        );
    }

    #[test]
    fn py_index_follows_python_subscripting() {
        assert_eq!(py_index(&json!([1, 2]), 1).unwrap(), json!(2));
        assert_eq!(
            py_index(&json!([1]), 1).unwrap_err(),
            "list index out of range"
        );
        assert_eq!(py_index(&json!("ab"), 1).unwrap(), json!("b"));
        assert_eq!(
            py_index(&json!(""), 0).unwrap_err(),
            "string index out of range"
        );
        // 多位元組字元以字元（不是位元組）計。
        assert_eq!(py_index(&json!("台積"), 1).unwrap(), json!("積"));
        // 物件沒有整數鍵、其餘型別不可下標。
        assert!(py_index(&json!({"0": 1}), 0).is_err());
        for bad in [json!(null), json!(5), json!(true)] {
            assert!(py_index(&bad, 0).is_err(), "{bad}");
        }
        assert_eq!(
            py_index(&json!(null), 0).unwrap_err(),
            "'NoneType' object is not subscriptable"
        );
    }

    #[test]
    fn py_dict_and_type_names() {
        assert!(py_dict(&json!({"a": 1})).is_ok());
        assert_eq!(
            py_dict(&json!([1])).unwrap_err(),
            "'list' object has no attribute 'get'"
        );
        assert_eq!(py_type_name(&json!(1)), "int");
        assert_eq!(py_type_name(&json!(1.5)), "float");
        assert_eq!(py_type_name(&json!("x")), "str");
        assert_eq!(py_type_name(&json!(true)), "bool");
    }

    #[test]
    fn py_str_helpers() {
        let obj = json!({"a": "x", "b": null, "c": 0, "d": 5});
        let obj = obj.as_object().unwrap();
        assert_eq!(py_str_get(obj, "a"), "x");
        assert_eq!(py_str_get(obj, "b"), "None"); // 存在但 null → str(None)
        assert_eq!(py_str_get(obj, "zz"), ""); // 缺 → 預設空字串
                                               // `str(x or "")`：假值（含 0、null）→ 空字串。
        assert_eq!(py_str_or_empty(obj.get("a")), "x");
        assert_eq!(py_str_or_empty(obj.get("b")), "");
        assert_eq!(py_str_or_empty(obj.get("c")), "");
        assert_eq!(py_str_or_empty(obj.get("d")), "5");
        assert_eq!(py_str_or_empty(None), "");
    }

    #[test]
    fn roc_to_date_value_follows_python_roc_to_date() {
        use time::macros::date;
        let d = Some(date!(2026 - 10 - 08));
        assert_eq!(roc_to_date_value(Some(&json!("115/10/08"))), d);
        assert_eq!(roc_to_date_value(Some(&json!(1151008))), d); // str(int)
        assert_eq!(roc_to_date_value(Some(&json!(" 1151008 "))), d);
        for none in [
            json!(null),
            json!(""),
            json!(0),
            json!(false),
            json!("x"),
            json!(20261008),
            json!(1151008.0),
        ] {
            assert_eq!(roc_to_date_value(Some(&none)), None, "{none}");
        }
        assert_eq!(roc_to_date_value(None), None);
    }

    // ───── py_int／py_repr：期望值來自真的跑 Python `int(s)`／`repr(x)`（Python 3.14.6） ─────

    #[test]
    fn py_int_matches_python_int_for_strings() {
        for (s, want) in [
            ("1", Some(1)),
            ("+1", Some(1)),
            ("-1", Some(-1)),
            ("1_000", Some(1000)),
            ("-0", Some(0)),
            ("١٢", Some(12)),
            ("１２", Some(12)),
            ("1__0", None),
            ("_1", None),
            ("1_", None),
            ("", None),
            ("+", None),
            ("1.0", None),
            ("1e3", None),
            (" 1", None), // 呼叫端先 strip
        ] {
            assert_eq!(py_int(s), want, "{s:?}");
        }
        // 40 位數字：Python 任意精度；這裡 i128 放不下（超過 1.7e38）→ None
        assert_eq!(py_int(&"9".repeat(40)), None);
        assert_eq!(py_int(&"9".repeat(38)), Some(10_i128.pow(38) - 1));
    }

    #[test]
    fn py_repr_matches_python_repr_for_the_error_message_cases() {
        assert_eq!(py_repr_str("abc"), "'abc'");
        assert_eq!(py_repr_str("it's"), "\"it's\"");
        assert_eq!(py_repr_str("say \"hi\""), "'say \"hi\"'");
        assert_eq!(py_repr_str("both ' and \""), "'both \\' and \"'");
        assert_eq!(py_repr_str("tab\tx"), "'tab\\tx'");
        assert_eq!(py_repr_str("nl\nx"), "'nl\\nx'");
        assert_eq!(py_repr_str("\u{1}"), "'\\x01'");
        assert_eq!(py_repr_str("\u{a0}"), "'\\xa0'");
        assert_eq!(py_repr_str("\u{3000}"), "'\\u3000'");
        assert_eq!(py_repr_str("\u{7f}"), "'\\x7f'");
        assert_eq!(py_repr_str("中文é"), "'中文é'");
        assert_eq!(py_repr_str("a\\b"), "'a\\\\b'");
        assert_eq!(
            py_repr(&json!({"a": [1, null, true, "x"]})),
            "{'a': [1, None, True, 'x']}"
        );
        assert_eq!(py_repr(&json!(null)), "None");
        assert_eq!(py_repr(&json!(false)), "False");
    }
}
