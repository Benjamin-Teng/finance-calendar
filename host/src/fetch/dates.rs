//! 日期與數字格式化工具：民國年轉換（B-DATE-4）、與 Python 逐位一致的 `round(x, 2)`（K-4）、
//! Python `f"{v:g}"`（K-6）。
//!
//! 所有「與 Python 一致」的實作，期望值都來自真的跑 Python
//! （`uv run --no-project python -c "print(repr(round(x, 2)))"`、`print(f"{v:g}")`）的輸出，
//! 見各測試表格；不靠推論。

use std::sync::OnceLock;

use regex::Regex;
use time::{Date, Month};

/// B-DATE-4 第一條 regex（逐字）：`115/07/15`、`115年07月15日`、`115.7.5`、`115-07-15`。
const ROC_SEPARATED: &str = r"^(\d{2,3})[年/.\-](\d{1,2})[月/.\-](\d{1,2})日?$";
/// B-DATE-4 第二條 regex（逐字）：`1150715`。
const ROC_COMPACT: &str = r"^(\d{3})(\d{2})(\d{2})$";

/// 單一 Unicode 十進位數字（`\d`＝`\p{Nd}`），供 [`digit_value`] 判斷相鄰字元是否同屬一排數字。
fn digit_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\d$").expect("固定的 regex 必能編譯"))
}

fn roc_regexes() -> &'static (Regex, Regex) {
    static RE: OnceLock<(Regex, Regex)> = OnceLock::new();
    RE.get_or_init(|| {
        (
            Regex::new(ROC_SEPARATED).expect("固定的 regex 必能編譯"),
            Regex::new(ROC_COMPACT).expect("固定的 regex 必能編譯"),
        )
    })
}

/// Unicode 十進位數字（Nd）的數值。Python `int()` 認得全形等 Unicode 數字，而 Rust 的
/// `str::parse` 只吃 ASCII，所以 regex 以 Unicode `\d` 命中後要自己換算（K-5：`\d` 不可改成
/// ASCII，否則與 Python 分歧）。
///
/// Unicode 保證 Nd 以「連續 10 個、從 0 到 9」成排配置（數學用數字等相鄰排組也是 10 的倍數），
/// 所以往前數連續的 Nd 個數、對 10 取餘就是該數字的值。
pub(crate) fn digit_value(c: char) -> u32 {
    if let Some(d) = c.to_digit(10) {
        return d;
    }
    let re = digit_regex();
    let mut run = 0u32;
    let mut cur = c as u32;
    let mut buf = [0u8; 4];
    while cur > 0 {
        let prev = match char::from_u32(cur - 1) {
            Some(p) => p,
            None => break,
        };
        if !re.is_match(prev.encode_utf8(&mut buf)) {
            break;
        }
        run += 1;
        cur -= 1;
    }
    run % 10
}

/// `c` 是 Unicode 十進位數字（Nd；Python `str.isdecimal()` 為真，`int()` 認得）。
pub(crate) fn is_decimal_digit(c: char) -> bool {
    c.is_ascii_digit() || (!c.is_ascii() && digit_regex().is_match(c.encode_utf8(&mut [0u8; 4])))
}

/// 一串 Unicode 數字 → 整數（與 Python `int(str)` 一致）。
pub(crate) fn digits_to_u32(s: &str) -> u32 {
    s.chars().fold(0, |acc, c| acc * 10 + digit_value(c))
}

/// Python `str.strip()` 的空白判定：`str.isspace()`，含 `\x1c`–`\x1f`（Rust 的
/// `char::is_whitespace` 不含這四個）。
pub(crate) fn is_py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// 民國日期 → `Date`（B-DATE-4）：先 trim、依序試兩條 regex、年＋1911、不合法日期（如 2 月 30 日）
/// 回 `None`；8 位西元（`20261001`）不吃，回 `None`。
pub fn roc_to_date(s: &str) -> Option<Date> {
    let s = s.trim_matches(is_py_space);
    let (separated, compact) = roc_regexes();
    let caps = separated.captures(s).or_else(|| compact.captures(s))?;
    let year = digits_to_u32(&caps[1]) as i32 + 1911;
    let month = u8::try_from(digits_to_u32(&caps[2])).ok()?;
    let day = u8::try_from(digits_to_u32(&caps[3])).ok()?;
    Date::from_calendar_date(year, Month::try_from(month).ok()?, day).ok()
}

/// `date.isoformat()`：`YYYY-MM-DD`（民國年換算後一律 4 位數年份）。
pub fn iso_date(d: Date) -> String {
    format!("{:04}-{:02}-{:02}", d.year(), u8::from(d.month()), d.day())
}

/// 與 Python `round(x, 2)` 逐位一致（K-4）。
///
/// Python 以浮點的**精確十進位值**做「四捨六入五成雙」（例：`round(2.675, 2) == 2.67`，因為
/// 2.675 的二進位表示略小於 2.675；`round(0.125, 2) == 0.12` 是恰好的 .5 取偶）。Rust 的
/// `{:.2}` 同樣對精確十進位值正確捨入、恰好半數取偶，所以先格式化再 parse 回來即等價；
/// 與常見的 `(x * 100.0).round() / 100.0` 在邊界值不同（後者 `2.675 → 2.68`）。
/// 非有限值（inf／nan）Python 原樣返回，這裡也是。
pub fn round2(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    format!("{x:.2}").parse().unwrap_or(x)
}

/// 去掉小數部分尾端的 0（以及隨後孤立的小數點）；沒有小數點的字串不動。
fn strip_trailing_zeros(s: &str) -> &str {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.')
    } else {
        s
    }
}

/// Python `f"{v:g}"`（K-6）：6 位有效數字、去尾零（整數不帶小數點）、指數 `exp < -4 或 exp >= 6`
/// 改用科學記號且指數至少兩位（`1e+06`、`1e-05`）。`-0.0` 得 `-0`；`inf`／`nan` 同 Python。
pub fn fmt_g(v: f64) -> String {
    if v.is_nan() {
        return "nan".to_string();
    }
    if v.is_infinite() {
        return if v > 0.0 { "inf" } else { "-inf" }.to_string();
    }
    // 先以 6 位有效數字（小數點後 5 位的科學記號）求出「捨入後」的指數；捨入進位
    // （999999.5 → 1.00000e6）會讓指數加一，所以必須用捨入後的值判斷。
    let sci = format!("{v:.5e}");
    let (mantissa, exp) = sci.split_once('e').unwrap_or((sci.as_str(), "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    if !(-4..6).contains(&exp) {
        let sign = if exp < 0 { '-' } else { '+' };
        format!("{}e{sign}{:02}", strip_trailing_zeros(mantissa), exp.abs())
    } else {
        let decimals = (5 - exp) as usize;
        strip_trailing_zeros(&format!("{v:.decimals$}")).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    // ---- roc_to_date ----

    #[test]
    fn roc_to_date_all_documented_formats() {
        let d = Some(date!(2026 - 07 - 15));
        for s in [
            "115/07/15",
            "115年07月15日",
            "115年07月15", // 結尾的「日」可省略
            "115.7.15",
            "115-07-15",
            "115/7/15",
            "1150715",
            "  115/07/15  ", // trim
            "\t115-07-15\n",
            "115.07-15", // 分隔符各自獨立
            "115/07月15日",
        ] {
            assert_eq!(roc_to_date(s), d, "輸入 {s:?}");
        }
        assert_eq!(roc_to_date("115.7.5"), Some(date!(2026 - 07 - 05)));
        // 2 位數年（民國 99 年）。
        assert_eq!(roc_to_date("99/12/31"), Some(date!(2010 - 12 - 31)));
        // 日、月單位數。
        assert_eq!(roc_to_date("115/1/1"), Some(date!(2026 - 01 - 01)));
    }

    #[test]
    fn roc_to_date_rejects_invalid_and_unsupported() {
        for s in [
            "",
            "   ",
            "20261001",   // 8 位西元不吃
            "2026/10/01", // 4 位數年不吃第一條
            "115/02/30",  // 2 月 30 日
            "115/13/01",  // 13 月
            "115/00/10",  // 0 月
            "115/07/00",  // 0 日
            "115/07/32",
            "1150230",
            "115/07/15x",
            "x115/07/15",
            "11/07/15x",
            "115//15",
            "115/07",
            "115/07/150", // 日 3 位
            "115070",     // 6 位數字
            "11507150",   // 8 位數字
            "abc",
        ] {
            assert_eq!(roc_to_date(s), None, "輸入 {s:?}");
        }
    }

    #[test]
    fn roc_to_date_leap_day() {
        // 民國 113 年（西元 2024）是閏年，114（2025）不是。
        assert_eq!(roc_to_date("113/02/29"), Some(date!(2024 - 02 - 29)));
        assert_eq!(roc_to_date("114/02/29"), None);
        assert_eq!(roc_to_date("1130229"), Some(date!(2024 - 02 - 29)));
    }

    /// K-5：`\d` 是 Unicode 數字，與 Python 相同；Python `int("１１５")` 也成立。
    /// 期望值來自 Python：`re.match(...)` 對全形數字命中，`date(115+1911, 7, 15)`。
    #[test]
    fn roc_to_date_accepts_unicode_digits_like_python() {
        assert_eq!(roc_to_date("１１５/０７/１５"), Some(date!(2026 - 07 - 15)));
        assert_eq!(roc_to_date("１１５０７１５"), Some(date!(2026 - 07 - 15)));
        // 阿拉伯－印度數字（U+0661…）、天城文（U+0966…）。
        assert_eq!(roc_to_date("١١٥/٠٧/١٥"), Some(date!(2026 - 07 - 15)));
        assert_eq!(roc_to_date("११५-०७-१५"), Some(date!(2026 - 07 - 15)));
        // 全形與半形混用也成立。
        assert_eq!(roc_to_date("１１5/07/１5"), Some(date!(2026 - 07 - 15)));
    }

    #[test]
    fn digit_value_covers_every_digit_of_a_run() {
        for (zero, name) in [
            ('０', "全形"),
            ('٠', "阿拉伯－印度"),
            ('०', "天城文"),
            ('𝟎', "數學"),
        ] {
            for k in 0..10u32 {
                let c = char::from_u32(zero as u32 + k).expect("有效的碼位");
                assert_eq!(digit_value(c), k, "{name} 數字 {c}");
            }
        }
        // 數學用數字有 5 排（U+1D7CE–1D7FF）連續配置，最後一排也要對。
        assert_eq!(digit_value('\u{1D7FF}'), 9);
        assert_eq!(digit_value('\u{1D7F6}'), 0);
    }

    #[test]
    fn python_strip_whitespace_set_is_matched() {
        // Python `"\x1c115/07/15\x1f".strip()` 會去掉 \x1c–\x1f。
        assert_eq!(
            roc_to_date("\u{1c}115/07/15\u{1f}"),
            Some(date!(2026 - 07 - 15))
        );
        // 全形空白（U+3000）、不換行空白（U+00A0）Python 也算空白。
        assert_eq!(
            roc_to_date("\u{3000}115/07/15\u{a0}"),
            Some(date!(2026 - 07 - 15))
        );
    }

    // ---- round2 ----

    /// 期望值全部來自 `uv run --no-project python -c "print(repr(round(x, 2)))"`。
    /// 輸入以 Python `repr` 的最短往返寫法給出（Rust 的 f64 字面值解析到同一個浮點）。
    #[test]
    fn round2_matches_python_table() {
        let cases: &[(f64, f64)] = &[
            (0.125, 0.12),
            (0.135, 0.14),
            (2.675, 2.67),
            (1.005, 1.0),
            (-2.675, -2.67),
            (1e-3, 0.0),
            (123456.785, 123456.79),
            (0.0, 0.0),
            (1e300, 1e300),
            (1.7976931348623157e308, 1.7976931348623157e308),
            (0.005, 0.01),
            (0.015, 0.01),
            (0.025, 0.03),
            (0.035, 0.04),
            (0.045, 0.04),
            (0.075, 0.07),
            (0.5, 0.5),
            (1.5, 1.5),
            (2.5, 2.5),
            (0.994999, 0.99),
            (0.995, 0.99),
            (-0.125, -0.12),
            (-0.005, -0.01),
            (0.004, 0.0),
            (1e-10, 0.0),
            (5e-324, 0.0),
            (1234567.891, 1234567.89),
            (1e16, 1e16),
            (1e22, 1e22),
            (4503599627370496.0, 4503599627370496.0),
            (0.285, 0.28),
            (1.115, 1.11),
            (100.0, 100.0),
            (48000.0, 48000.0),
            (31.867900848388672, 31.87),
            (12.345, 12.35),
            (1.255, 1.25),
            (8.345, 8.35),
            (-1.005, -1.0),
            (0.0050000001, 0.01),
            (1000000000000000.5, 1000000000000000.5),
            (9007199254740992.0, 9007199254740992.0),
        ];
        for &(x, want) in cases {
            let got = round2(x);
            assert_eq!(
                got.to_bits(),
                want.to_bits(),
                "round2({x:?}) 得 {got:?}，Python 得 {want:?}"
            );
        }
    }

    /// `-0.0`、`-0.004`、`-0.0049999` 在 Python 都得 `-0.0`（負零），位元層級要分得出來。
    #[test]
    fn round2_keeps_negative_zero_like_python() {
        for x in [-0.0_f64, -0.004, -0.0049999] {
            let got = round2(x);
            assert_eq!(got.to_bits(), (-0.0_f64).to_bits(), "round2({x:?})");
        }
        assert_eq!(round2(0.0).to_bits(), 0.0_f64.to_bits());
    }

    #[test]
    fn round2_passes_non_finite_through() {
        assert_eq!(round2(f64::INFINITY), f64::INFINITY);
        assert_eq!(round2(f64::NEG_INFINITY), f64::NEG_INFINITY);
        assert!(round2(f64::NAN).is_nan());
    }

    /// 與常見的 `(x*100).round()/100` 在邊界不同——證明本實作不是那個寫法。
    #[test]
    fn round2_differs_from_naive_scale_and_round() {
        let naive = |x: f64| (x * 100.0).round() / 100.0;
        assert_eq!(naive(2.675), 2.68);
        assert_eq!(round2(2.675), 2.67);
        assert_eq!(naive(0.125), 0.13);
        assert_eq!(round2(0.125), 0.12);
    }

    // ---- fmt_g ----

    /// 期望值全部來自 `uv run --no-project python -c "print(f'{v:g}')"`。
    #[test]
    fn fmt_g_matches_python_table() {
        let cases: &[(f64, &str)] = &[
            (0.138, "0.138"),
            (1.0, "1"),
            (2.5, "2.5"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (123456.0, "123456"),
            (1234567.0, "1.23457e+06"),
            (1e16, "1e+16"),
            (0.0, "0"),
            (-0.0, "-0"),
            (100.0, "100"),
            (100000.0, "100000"),
            (999999.0, "999999"),
            (999999.5, "1e+06"),
            (9999995.0, "1e+07"),
            (0.00012345678, "0.000123457"),
            (1.23456789, "1.23457"),
            (1.5e-7, "1.5e-07"),
            (1e100, "1e+100"),
            (1e-100, "1e-100"),
            (12345.6789, "12345.7"),
            (0.1, "0.1"),
            (0.5, "0.5"),
            (9.87654321, "9.87654"),
            (-2.5, "-2.5"),
            (-0.00001, "-1e-05"),
            (1000000.0, "1e+06"),
            (123456.7, "123457"),
            (0.000123456789, "0.000123457"),
            (99999.95, "99999.9"),
            (999999.4, "999999"),
            (0.0000999999, "9.99999e-05"),
            (0.00009999995, "0.0001"),
            (5e-324, "4.94066e-324"),
            (1.7976931348623157e308, "1.79769e+308"),
            (1.23456, "1.23456"),
            (1.234565, "1.23456"),
            (2.5e-5, "2.5e-05"),
            (0.05, "0.05"),
        ];
        for &(v, want) in cases {
            assert_eq!(fmt_g(v), want, "fmt_g({v:?})");
        }
    }

    #[test]
    fn fmt_g_non_finite() {
        assert_eq!(fmt_g(f64::INFINITY), "inf");
        assert_eq!(fmt_g(f64::NEG_INFINITY), "-inf");
        assert_eq!(fmt_g(f64::NAN), "nan");
    }
}
