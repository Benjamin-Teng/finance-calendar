//! 抓取層的錯誤型別，以及「錯誤訊息正規化」（design.md D4）。
//!
//! # `FetchError`
//!
//! 只有「連線失敗、逾時、TLS 失敗」才是 `Err`；HTTP 4xx／5xx 在 [`super::http::Fetch`]
//! 層回傳狀態碼、不是 `Err`（對應 Python 的 `HTTPError`，B-HTTP-3），由呼叫端決定要不要當失敗。
//! [`FetchError::Status`] 只在便利函式 [`super::http::get_ok`] 把非 2xx 轉成錯誤時出現。
//!
//! `Display` 的文字就是之後填進 B-FLOW-9 各訊息 `{e}` 位置的內容；不必與 Python 的
//! `str(e)` 相同（`{e}` 內文因語言／函式庫而異，無消費端解析，B-HTTP-7）。
//!
//! # 錯誤訊息正規化
//!
//! Python 與 Rust 的 `{e}` 不可能相同，所以等價測試比對 `errors`／`wallpaper_errors` 前，
//! 先把「命中已知前綴」的訊息截成前綴；清單外的訊息（如「啟動時網路等待逾時，改用既有資料
//! 兜底」「法說會：無市值前百大名單可篩，本輪改用備援來源」、帶日期的「加權日K來源回傳較舊的
//! 資料（…），沿用上次資料」）整句保留、整句比對。前綴清單逐字抄自 behavior-inventory
//! B-FLOW-9 帶 `{e}` 的訊息。

use std::fmt;

#[cfg(test)]
use serde_json::Value;

/// 抓取失敗（連線層）。HTTP 狀態碼非 2xx 不在此列，見模組文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// 連線失敗（DNS、拒絕連線、TLS 握手與憑證驗證失敗、讀取中斷等）；內容是錯誤鏈的文字。
    /// 測試用的 `FixtureFetch` 對「error 紀錄」與「查無對應」也回這個。
    Network(String),
    /// 單一請求超過逾時（B-HTTP-2：30 秒）。
    Timeout,
    /// 便利函式把非 2xx 轉成的錯誤（B-HTTP-3 的 `HTTPError`）。
    Status(u16),
    /// 回應本體不是合法 JSON（B-HTTP-5：Python 的 `json.loads(bytes.decode("utf-8"))` 失敗，含 BOM、
    /// 壞 UTF-8）；內容是解析器的訊息。
    Decode(String),
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FetchError::Network(msg) | FetchError::Decode(msg) => f.write_str(msg),
            FetchError::Timeout => f.write_str("timed out"),
            FetchError::Status(code) => {
                // 與 Python `HTTPError` 的 `str(e)` 同格式：`HTTP Error 500: Internal Server Error`。
                match reqwest::StatusCode::from_u16(*code)
                    .ok()
                    .and_then(|s| s.canonical_reason())
                {
                    Some(reason) => write!(f, "HTTP Error {code}: {reason}"),
                    None => write!(f, "HTTP Error {code}"),
                }
            }
        }
    }
}

impl std::error::Error for FetchError {}

/// 帶 `{e}` 的固定前綴（逐字抄自 B-FLOW-9）。`行情來源失敗（{name}／{symbol}）：`
/// 的名稱與代號是變動的，另由 [`normalize_error`] 特別處理。
#[cfg(test)]
const FIXED_PREFIXES: &[&str] = &[
    // errors
    "總經來源失敗（本週檔）：",
    "除權息來源失敗：",
    "股東會來源失敗：",
    "市值前百大來源失敗：",
    "法說會來源（MOPS）失敗：",
    "法說會備援來源（重大訊息）失敗：",
    "處置股來源失敗（上市）：",
    "處置股來源失敗（上櫃）：",
    "休市日曆來源失敗：",
    // wallpaper_errors
    "加權盤中走勢來源失敗：",
    "加權日K來源失敗：",
    "券資比／融資維持率來源失敗：",
    // `{label}來源發生非預期錯誤：{類名}: {e}`，label 三種（B-GRD-1）
    "加權盤中走勢來源發生非預期錯誤：",
    "加權日K來源發生非預期錯誤：",
    "券資比／融資維持率來源發生非預期錯誤：",
];

/// 行情來源失敗訊息的開頭；完整前綴是「開頭＋名稱／代號＋`）：`」。
#[cfg(test)]
const QUOTE_PREFIX_HEAD: &str = "行情來源失敗（";
#[cfg(test)]
const QUOTE_PREFIX_TAIL: &str = "）：";

/// 命中已知前綴就截成前綴（含該前綴自身），否則整句保留。
#[cfg(test)]
pub fn normalize_error(msg: &str) -> String {
    if let Some(p) = FIXED_PREFIXES.iter().find(|p| msg.starts_with(**p)) {
        return (*p).to_string();
    }
    if msg.starts_with(QUOTE_PREFIX_HEAD) {
        // 名稱與代號固定、不含 `）：`，第一個 `）：` 就是前綴的結尾。
        if let Some(i) = msg.find(QUOTE_PREFIX_TAIL) {
            return msg[..i + QUOTE_PREFIX_TAIL.len()].to_string();
        }
    }
    msg.to_string()
}

/// 對輸出（`serde_json::Value`）的 `errors`、`wallpaper_errors` 兩個字串陣列逐筆套
/// [`normalize_error`]，其餘原樣；筆數與順序不變。給 4.2 的等價比對用。
#[cfg(test)]
pub fn normalize_output(mut output: Value) -> Value {
    if let Some(obj) = output.as_object_mut() {
        for key in ["errors", "wallpaper_errors"] {
            if let Some(Value::Array(items)) = obj.get_mut(key) {
                for item in items.iter_mut() {
                    if let Value::String(s) = item {
                        *s = normalize_error(s);
                    }
                }
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn display_status_matches_python_httperror_format() {
        assert_eq!(
            FetchError::Status(500).to_string(),
            "HTTP Error 500: Internal Server Error"
        );
        assert_eq!(
            FetchError::Status(404).to_string(),
            "HTTP Error 404: Not Found"
        );
        assert_eq!(FetchError::Status(599).to_string(), "HTTP Error 599");
        assert_eq!(FetchError::Network("dns".into()).to_string(), "dns");
        assert_eq!(FetchError::Timeout.to_string(), "timed out");
    }

    #[test]
    fn every_fixed_prefix_is_cut_to_itself() {
        for p in FIXED_PREFIXES {
            let msg = format!("{p}HTTP Error 500: Internal Server Error");
            assert_eq!(normalize_error(&msg), *p, "前綴 {p}");
            // `{e}` 內容任意（含空字串、含 `：`、含換行）都截成同一個前綴。
            assert_eq!(normalize_error(p), *p);
            assert_eq!(normalize_error(&format!("{p}a：b\nc")), *p);
        }
        assert_eq!(FIXED_PREFIXES.len(), 15);
    }

    #[test]
    fn quote_prefix_keeps_name_and_symbol() {
        assert_eq!(
            normalize_error(
                "行情來源失敗（日經225／^N225）：HTTP Error 500: Internal Server Error"
            ),
            "行情來源失敗（日經225／^N225）："
        );
        assert_eq!(
            normalize_error("行情來源失敗（SOFR 3M／NY Fed）：timed out"),
            "行情來源失敗（SOFR 3M／NY Fed）："
        );
        // `{e}` 內若含 `）：`，仍只截到第一個。
        assert_eq!(
            normalize_error("行情來源失敗（USD/TWD／USDTWD=X）：a）：b"),
            "行情來源失敗（USD/TWD／USDTWD=X）："
        );
        // 沒有 `）：` 就當清單外，整句保留。
        assert_eq!(normalize_error("行情來源失敗（壞掉"), "行情來源失敗（壞掉");
    }

    #[test]
    fn messages_outside_the_list_are_unchanged() {
        for msg in [
            "啟動時網路等待逾時，改用既有資料兜底",
            "法說會：無市值前百大名單可篩，本輪改用備援來源",
            "加權日K來源回傳較舊的資料（最後一根 2026-10-02 早於上次的 2026-10-06），沿用上次資料",
        ] {
            assert_eq!(normalize_error(msg), msg);
        }
        // 半形冒號不算前綴（前綴逐字含全形「：」）。
        assert_eq!(normalize_error("除權息來源失敗:x"), "除權息來源失敗:x");
        assert_eq!(normalize_error(""), "");
    }

    #[test]
    fn normalize_output_only_touches_the_two_error_arrays() {
        let v = json!({
            "errors": [
                "除權息來源失敗：HTTP Error 500: Internal Server Error",
                "法說會：無市值前百大名單可篩，本輪改用備援來源",
                "行情來源失敗（KOSPI／^KS11）：timed out",
            ],
            "wallpaper_errors": [
                "加權盤中走勢來源失敗：2026-10-02 序列終點 ...太早",
                "券資比／融資維持率來源發生非預期錯誤：ValueError: x",
            ],
            "sources": { "macro": "處置股來源失敗（上市）：不該被動" },
            "other": ["除權息來源失敗：不該被動"],
        });
        let got = normalize_output(v);
        assert_eq!(
            got,
            json!({
                "errors": [
                    "除權息來源失敗：",
                    "法說會：無市值前百大名單可篩，本輪改用備援來源",
                    "行情來源失敗（KOSPI／^KS11）：",
                ],
                "wallpaper_errors": [
                    "加權盤中走勢來源失敗：",
                    "券資比／融資維持率來源發生非預期錯誤：",
                ],
                "sources": { "macro": "處置股來源失敗（上市）：不該被動" },
                "other": ["除權息來源失敗：不該被動"],
            })
        );
        // 非物件、缺鍵、型別不對都原樣返回、不 panic。
        assert_eq!(normalize_output(json!([1, 2])), json!([1, 2]));
        assert_eq!(
            normalize_output(json!({"errors": "x"})),
            json!({"errors": "x"})
        );
        assert_eq!(
            normalize_output(json!({"errors": [1, "除權息來源失敗：a"]})),
            json!({"errors": [1, "除權息來源失敗："]})
        );
    }
}
