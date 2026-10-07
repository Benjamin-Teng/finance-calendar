//! Python `html.unescape` 的逐步移植（task 3.3；MOPS 表格儲存格的文字處理用，B-CONF-4）。
//!
//! 對照 CPython `Lib/html/__init__.py`（字元參照的規則自 3.4 起沒變；表格與數字皆以 Python 3.14 實測）：
//!
//! ```text
//! _charref = re.compile(r'&(#[0-9]+;?|#[xX][0-9a-fA-F]+;?|[^\t\n\f <&#;]{1,32};?)')
//! 數字參照：num in _invalid_charrefs → 取表（0x00→U+FFFD、0x0D→\r、0x80–0x9F→Windows-1252 對照）
//!           0xD800–0xDFFF 或 > 0x10FFFF → U+FFFD
//!           num in _invalid_codepoints → ''（空字串）
//!           其餘 chr(num)
//! 具名參照：s in html5（含分號與舊式不帶分號的名稱）→ 值
//!           否則由長到短找最長的前綴（長度 len(s)-1 … 2，以字元計）→ 值 + 剩餘文字
//!           都沒有 → '&' + s
//! ```
//!
//! 具名表（2231 筆）是 [`super::html_entities::HTML5_ENTITIES`]：由 `tests/fetch_oracle/gen_html_entities.py` 從
//! Python 的 `html.entities.html5` 直接產生的靜態表（依鍵排序，`binary_search` 查詢），不依賴任何外部 crate。
//! `_invalid_charrefs` 的 0x80–0x9F 對照（Windows-1252）在 [`C1_REPLACEMENTS`] 手寫，照 Python `html` 模組。
//!
//! 與 Python 的一處差異：無。十進位數字參照超過 4300 位數時 Python 3.11+ 的 `int()` 會丟 `ValueError`
//! （整個 `html.unescape` 因而失敗，MOPS 來源記一筆失敗），這裡同樣回傳 `Err`，訊息逐字相同。

use std::sync::OnceLock;

use regex::Regex;

use super::html_entities::HTML5_ENTITIES;

/// `html._invalid_charrefs` 的 0x80–0x9F 部分（Windows-1252 對照；0x81、0x8D、0x8F、0x90、0x9D 沒有定義，
/// Python 的表直接回傳該控制字元本身）。索引＝碼位 − 0x80。
const C1_REPLACEMENTS: [u32; 32] = [
    0x20ac, 0x81, 0x201a, 0x192, 0x201e, 0x2026, 0x2020, 0x2021, 0x2c6, 0x2030, 0x160, 0x2039,
    0x152, 0x8d, 0x17d, 0x8f, 0x90, 0x2018, 0x2019, 0x201c, 0x201d, 0x2022, 0x2013, 0x2014, 0x2dc,
    0x2122, 0x161, 0x203a, 0x153, 0x9d, 0x17e, 0x178,
];

/// `html._charref`（逐字）。
const CHARREF: &str = r"&(#[0-9]+;?|#[xX][0-9a-fA-F]+;?|[^\t\n\f <&#;]{1,32};?)";

/// Python 3.11+ 預設的 `sys.int_info.default_max_str_digits`。
const MAX_STR_DIGITS: usize = 4300;

fn charref() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(CHARREF).expect("固定的 regex 必能編譯"))
}

/// `html.unescape(s)`。`Err`＝Python 會丟 `ValueError`（十進位數字參照超過 4300 位數）。
pub fn unescape(s: &str) -> Result<String, String> {
    if !s.contains('&') {
        return Ok(s.to_string());
    }
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    for caps in charref().captures_iter(s) {
        let (Some(whole), Some(body)) = (caps.get(0), caps.get(1)) else {
            continue;
        };
        out.push_str(&s[last..whole.start()]);
        replace_charref(body.as_str(), &mut out)?;
        last = whole.end();
    }
    out.push_str(&s[last..]);
    Ok(out)
}

/// `_replace_charref`：`s` 是 regex 的第 1 群（不含開頭的 `&`）。
fn replace_charref(s: &str, out: &mut String) -> Result<(), String> {
    if let Some(num_part) = s.strip_prefix('#') {
        let (digits, radix) = match num_part.strip_prefix(['x', 'X']) {
            Some(hex) => (hex, 16),
            None => (num_part, 10),
        };
        let digits = digits.trim_end_matches(';');
        if radix == 10 && digits.len() > MAX_STR_DIGITS {
            return Err(format!(
                "Exceeds the limit ({MAX_STR_DIGITS} digits) for integer string conversion: \
                 value has {} digits; use sys.set_int_max_str_digits() to increase the limit",
                digits.len()
            ));
        }
        // 超過 u32 的值都遠大於 0x10FFFF，一律當成同一類（→ U+FFFD），不必精確。
        let num = digits.chars().fold(0u32, |acc, c| {
            let d = c.to_digit(radix).unwrap_or(0);
            acc.saturating_mul(radix).saturating_add(d)
        });
        out.push_str(&numeric_charref(num));
        return Ok(());
    }
    // 具名參照。
    if let Some(v) = named(s) {
        out.push_str(&v);
        return Ok(());
    }
    // 由長到短找最長的前綴（以字元計，長度 len-1 … 2）。
    let idx: Vec<usize> = s.char_indices().map(|(i, _)| i).collect();
    let n = idx.len();
    for x in (2..n).rev() {
        let cut = idx[x];
        if let Some(v) = named(&s[..cut]) {
            out.push_str(&v);
            out.push_str(&s[cut..]);
            return Ok(());
        }
    }
    out.push('&');
    out.push_str(s);
    Ok(())
}

/// 數字參照的值（`_invalid_charrefs`、代理對與超界、`_invalid_codepoints`、一般字元，依 Python 的判斷順序）。
fn numeric_charref(num: u32) -> String {
    match num {
        0x00 => return "\u{fffd}".to_string(),
        0x0d => return "\r".to_string(),
        0x80..=0x9f => {
            return char::from_u32(C1_REPLACEMENTS[(num - 0x80) as usize])
                .map(String::from)
                .unwrap_or_default();
        }
        _ => {}
    }
    if (0xd800..=0xdfff).contains(&num) || num > 0x10ffff {
        return "\u{fffd}".to_string();
    }
    if is_invalid_codepoint(num) {
        return String::new();
    }
    char::from_u32(num).map(String::from).unwrap_or_default()
}

/// `html._invalid_codepoints`（0x80–0x9F 已在前面被 `_invalid_charrefs` 攔走，不會走到這裡）：
/// 0x01–0x08、0x0B、0x0E–0x1F、0x7F、0xFDD0–0xFDEF、每個平面最後兩個碼位（0xFFFE、0xFFFF、0x1FFFE…0x10FFFF）。
fn is_invalid_codepoint(num: u32) -> bool {
    matches!(num, 0x01..=0x08 | 0x0b | 0x0e..=0x1f | 0x7f | 0xfdd0..=0xfdef)
        || (num & 0xfffe) == 0xfffe
}

/// `html5` 表的查詢（`key in html.entities.html5`）。
fn named(key: &str) -> Option<String> {
    HTML5_ENTITIES
        .binary_search_by(|(k, _)| (*k).cmp(key))
        .ok()
        .map(|i| HTML5_ENTITIES[i].1.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每一列 `(輸入, 輸出)` 都是真的跑 Python `html.unescape` 的結果
    /// （Python 3.14.6／Unicode 16.0，`uv run --no-project python`）。
    const PYTHON_TABLE: &[(&str, &str)] = &[
        ("&nbsp;", "\u{a0}"),
        ("a&nbsp;b", "a\u{a0}b"),
        ("&nbsp", "\u{a0}"),
        ("&#174;", "\u{ae}"),
        ("&#xae;", "\u{ae}"),
        ("&#XAE;", "\u{ae}"),
        ("&#xAE", "\u{ae}"),
        ("&#174", "\u{ae}"),
        ("&", "&"),
        ("a & b", "a & b"),
        ("&;", "&;"),
        ("&#;", "&#;"),
        ("&#x;", "&#x;"),
        ("&#xZ;", "&#xZ;"),
        ("&amp;", "&"),
        ("&amp", "&"),
        ("&ampx", "&x"),
        ("&amp;amp;", "&amp;"),
        ("&AMP;", "&"),
        ("&Amp;", "&Amp;"),
        ("&lt;&gt;", "<>"),
        ("&ltx;", "<x;"),
        ("&notit;", "\u{ac}it;"),
        ("&notin;", "\u{2209}"),
        ("&notit", "\u{ac}it"),
        ("&noti", "\u{ac}i"),
        ("&copy2026", "\u{a9}2026"),
        ("&copy;", "\u{a9}"),
        ("&copyx;", "\u{a9}x;"),
        ("&eacute;", "\u{e9}"),
        ("&eacuteX", "\u{e9}X"),
        ("&aacute", "\u{e1}"),
        ("&acE;", "\u{223e}\u{333}"),
        ("&bne;", "=\u{20e5}"),
        ("&NotEqualTilde;", "\u{2242}\u{338}"),
        ("&#0;", "\u{fffd}"),
        ("&#13;", "\r"),
        ("&#128;", "\u{20ac}"),
        ("&#129;", "\u{81}"),
        ("&#141;", "\u{8d}"),
        ("&#159;", "\u{178}"),
        ("&#160;", "\u{a0}"),
        ("&#127;", ""),
        ("&#11;", ""),
        ("&#1;", ""),
        ("&#8;", ""),
        ("&#14;", ""),
        ("&#31;", ""),
        ("&#32;", " "),
        ("&#xD800;", "\u{fffd}"),
        ("&#55296;", "\u{fffd}"),
        ("&#57343;", "\u{fffd}"),
        ("&#1114111;", ""),
        ("&#1114112;", "\u{fffd}"),
        ("&#xFFFE;", ""),
        ("&#xFFFF;", ""),
        ("&#x1FFFE;", ""),
        ("&#x10FFFF;", ""),
        ("&#xFDD0;", ""),
        ("&#xFDEF;", ""),
        ("&#xFDF0;", "\u{fdf0}"),
        ("&#99999999999999;", "\u{fffd}"),
        ("&#x99999999999999;", "\u{fffd}"),
        ("&#21843;", "啓"),
        ("&#28201;", "温"),
        ("&#x5578;", "啸"),
        ("&#00065;", "A"),
        ("&#x0041;", "A"),
        ("&#65;&#66;", "AB"),
        ("&lt", "<"),
        ("&lt;", "<"),
        ("&LT", "<"),
        ("&gt", ">"),
        ("&quot", "\""),
        ("&QUOT;", "\""),
        ("&apos;", "'"),
        ("&apos", "&apos"),
        ("&nbsp;&nbsp;", "\u{a0}\u{a0}"),
        ("&nbsp&nbsp", "\u{a0}\u{a0}"),
        ("&nbsp;;", "\u{a0};"),
        (
            "&aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa;",
            "&aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa;",
        ),
        (
            "&aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa;",
            "&aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa;",
        ),
        (
            "&ampaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "&aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ),
        (
            "&ampaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "&aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ),
        (
            "&amp;aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "&aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ),
        ("&foo bar;", "&foo bar;"),
        ("&foo\tbar;", "&foo\tbar;"),
        ("&foo\nbar;", "&foo\nbar;"),
        ("&foo\rbar;", "&foo\rbar;"),
        ("&foo\u{c}bar;", "&foo\u{c}bar;"),
        ("&&amp;", "&&"),
        ("&#&amp;", "&#&"),
        ("&#x&amp;", "&#x&"),
        ("&a&amp;", "&a&"),
        ("&中;", "&中;"),
        ("&中文;", "&中文;"),
        ("5 &lt 6", "5 < 6"),
        ("AT&T", "AT&T"),
        ("AT&T;", "AT&T;"),
        ("R&D", "R&D"),
        ("R&D;", "R&D;"),
        ("&hearts;", "\u{2665}"),
        ("&hearts", "&hearts"),
        ("&Hat;", "^"),
        ("&Hacek;", "\u{2c7}"),
        ("&frac12", "\u{bd}"),
        ("&frac12;", "\u{bd}"),
        ("&frac34x", "\u{be}x"),
        ("&fracx;", "&fracx;"),
        ("&para", "\u{b6}"),
        ("&parax", "\u{b6}x"),
        ("&sect;", "\u{a7}"),
        ("&#x1F600;", "\u{1f600}"),
        ("&#128512;", "\u{1f600}"),
        ("&#xe9;", "\u{e9}"),
        ("&#233;", "\u{e9}"),
        ("&#233", "\u{e9}"),
        ("&#233x", "\u{e9}x"),
        ("&#x233g", "\u{233}g"),
        ("&#xg;", "&#xg;"),
        ("&#0x41;", "\u{fffd}x41;"),
        ("&\u{3000};", "&\u{3000};"),
        ("&\u{a0};", "&\u{a0};"),
        ("&nbsp\u{3000}", "\u{a0}\u{3000}"),
        ("&amp\u{3000};", "&\u{3000};"),
    ];

    /// 數字參照：`&#N;` 與 `&#xN;` 兩種寫法在 Python 都給同一個結果（腳本內 `assert`）。
    const PYTHON_NUMERIC: &[(u64, &str)] = &[
        (0x0, "\u{fffd}"),
        (0x1, ""),
        (0x2, ""),
        (0x3, ""),
        (0x4, ""),
        (0x5, ""),
        (0x6, ""),
        (0x7, ""),
        (0x8, ""),
        (0x9, "\t"),
        (0xa, "\n"),
        (0xb, ""),
        (0xc, "\u{c}"),
        (0xd, "\r"),
        (0xe, ""),
        (0xf, ""),
        (0x10, ""),
        (0x11, ""),
        (0x12, ""),
        (0x13, ""),
        (0x14, ""),
        (0x15, ""),
        (0x16, ""),
        (0x17, ""),
        (0x18, ""),
        (0x19, ""),
        (0x1a, ""),
        (0x1b, ""),
        (0x1c, ""),
        (0x1d, ""),
        (0x1e, ""),
        (0x1f, ""),
        (0x20, " "),
        (0x21, "!"),
        (0x7e, "~"),
        (0x7f, ""),
        (0x80, "\u{20ac}"),
        (0x81, "\u{81}"),
        (0x82, "\u{201a}"),
        (0x83, "\u{192}"),
        (0x84, "\u{201e}"),
        (0x85, "\u{2026}"),
        (0x86, "\u{2020}"),
        (0x87, "\u{2021}"),
        (0x88, "\u{2c6}"),
        (0x89, "\u{2030}"),
        (0x8a, "\u{160}"),
        (0x8b, "\u{2039}"),
        (0x8c, "\u{152}"),
        (0x8d, "\u{8d}"),
        (0x8e, "\u{17d}"),
        (0x8f, "\u{8f}"),
        (0x90, "\u{90}"),
        (0x91, "\u{2018}"),
        (0x92, "\u{2019}"),
        (0x93, "\u{201c}"),
        (0x94, "\u{201d}"),
        (0x95, "\u{2022}"),
        (0x96, "\u{2013}"),
        (0x97, "\u{2014}"),
        (0x98, "\u{2dc}"),
        (0x99, "\u{2122}"),
        (0x9a, "\u{161}"),
        (0x9b, "\u{203a}"),
        (0x9c, "\u{153}"),
        (0x9d, "\u{9d}"),
        (0x9e, "\u{17e}"),
        (0x9f, "\u{178}"),
        (0xa0, "\u{a0}"),
        (0xa1, "\u{a1}"),
        (0xd7ff, "\u{d7ff}"),
        (0xd800, "\u{fffd}"),
        (0xdbff, "\u{fffd}"),
        (0xdc00, "\u{fffd}"),
        (0xdfff, "\u{fffd}"),
        (0xe000, "\u{e000}"),
        (0xfdcf, "\u{fdcf}"),
        (0xfdd0, ""),
        (0xfdef, ""),
        (0xfdf0, "\u{fdf0}"),
        (0xfffd, "\u{fffd}"),
        (0xfffe, ""),
        (0xffff, ""),
        (0x10000, "\u{10000}"),
        (0x1fffd, "\u{1fffd}"),
        (0x1fffe, ""),
        (0x1ffff, ""),
        (0x20000, "\u{20000}"),
        (0xefffe, ""),
        (0xeffff, ""),
        (0xffffe, ""),
        (0xfffff, ""),
        (0x10fffd, "\u{10fffd}"),
        (0x10fffe, ""),
        (0x10ffff, ""),
        (0x110000, "\u{fffd}"),
        (0x110001, "\u{fffd}"),
        (0x4e2d, "中"),
        (0x1f600, "\u{1f600}"),
        (0xffffffff, "\u{fffd}"),
        (0x100000000, "\u{fffd}"),
    ];

    #[test]
    fn matches_python_html_unescape_on_a_table() {
        for (input, want) in PYTHON_TABLE {
            assert_eq!(unescape(input).as_deref(), Ok(*want), "輸入 {input:?}");
        }
    }

    #[test]
    fn numeric_references_match_python_in_decimal_and_hex() {
        for &(n, want) in PYTHON_NUMERIC {
            assert_eq!(
                unescape(&format!("&#{n};")).as_deref(),
                Ok(want),
                "十進位 {n:#x}"
            );
            assert_eq!(
                unescape(&format!("&#x{n:x};")).as_deref(),
                Ok(want),
                "十六進位 {n:#x}"
            );
            assert_eq!(
                unescape(&format!("&#X{n:X};")).as_deref(),
                Ok(want),
                "大寫 X {n:#x}"
            );
        }
    }

    #[test]
    fn a_string_without_ampersand_is_returned_untouched() {
        assert_eq!(
            unescape("沒有實體 <b> ;#"),
            Ok("沒有實體 <b> ;#".to_string())
        );
    }

    #[test]
    fn decimal_reference_over_4300_digits_fails_like_python_int_limit() {
        // Python 實測：`html.unescape("&#" + "9"*4301 + ";")` → ValueError（前導零也算位數）；
        // 4300 位數可以（結果 U+FFFD）；十六進位不受限。
        let ok = format!("&#{};", "9".repeat(4300));
        assert_eq!(unescape(&ok), Ok("\u{fffd}".to_string()));
        let bad = format!("&#{};", "9".repeat(4301));
        assert_eq!(
            unescape(&bad),
            Err("Exceeds the limit (4300 digits) for integer string conversion: value has 4301 digits; \
                 use sys.set_int_max_str_digits() to increase the limit"
                .to_string())
        );
        let zeros = format!("&#{}1;", "0".repeat(5000));
        assert!(unescape(&zeros).is_err());
        let hex = format!("&#x{};", "9".repeat(5000));
        assert_eq!(unescape(&hex), Ok("\u{fffd}".to_string()));
    }

    #[test]
    fn the_named_table_has_the_html5_shape() {
        // 條數（Python `len(html.entities.html5)`）與依鍵嚴格遞增（binary_search 的前提）。
        assert_eq!(HTML5_ENTITIES.len(), 2231);
        assert!(HTML5_ENTITIES.windows(2).all(|w| w[0].0 < w[1].0));
        // 抽樣（Python `html.entities.html5` 的內容）：含分號、舊式不帶分號、雙碼位實體、最長名稱。
        for (k, v) in [
            ("amp;", "&"),
            ("amp", "&"),
            ("AMP", "&"),
            ("nbsp;", "\u{a0}"),
            ("nbsp", "\u{a0}"),
            ("acE;", "\u{223e}\u{333}"),
            ("bne;", "=\u{20e5}"),
            ("NotEqualTilde;", "\u{2242}\u{338}"),
            ("hearts;", "\u{2665}"),
            ("lt", "<"),
            ("quot;", "\""),
            ("apos;", "'"),
            ("copy", "\u{a9}"),
            ("eacute;", "\u{e9}"),
            ("CounterClockwiseContourIntegral;", "\u{2233}"),
        ] {
            assert_eq!(named(k).as_deref(), Some(v), "{k}");
        }
        // 不在表內：前綴、空字串、只有帶分號形式的名稱不帶分號、大小寫不符。
        assert_eq!(named("am"), None);
        assert_eq!(named(""), None);
        assert_eq!(named("hearts"), None, "hearts 只有帶分號的形式");
        assert_eq!(named("Amp;"), None);
        assert_eq!(named("unknown;"), None);
    }
}
