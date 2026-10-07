//! oracle 對照測試的共用小工具（僅 `cfg(test)`）。
//!
//! 每個來源（3.1～3.5）對 [`super::fixture::scenario_dirs`] 的**每個**情境跑一次 `run`，與 Python
//! 產出的 `expected.json` 比對。情境數量不寫死（task 2 審查 m6）：新增情境自動納入。
//!
//! 用法（見 `macro_ff.rs` 的 `oracle_*` 測試）：
//!
//! ```ignore
//! oracle::for_each_scenario(|name, sc, expected| {
//!     let out = block_on(run(&sc.fetch, &sc.clock, &sc.previous()));
//!     assert_eq!(serde_json::to_value(&out.value).unwrap(), expected["macro"], "{name}");
//!     oracle::assert_errors(name, &out.errors, expected, &["總經來源失敗（本週檔）："]);
//! });
//! ```

use serde_json::Value;

use super::errors::normalize_error;
use super::fixture::{scenario_dirs, Scenario};

/// 對每個情境呼叫 `f(情境名, 情境, expected.json)`；載入或讀 expected 失敗直接 panic（附情境名）。
/// 情境清單為空也 panic（否則整個測試會靜默變成空轉）。
pub fn for_each_scenario(mut f: impl FnMut(&str, &Scenario, &Value)) {
    let dirs = scenario_dirs();
    assert!(!dirs.is_empty(), "找不到任何樣本情境");
    for dir in dirs {
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let sc = Scenario::load(&dir).unwrap_or_else(|e| panic!("載入情境 {name} 失敗：{e}"));
        let expected = sc
            .expected()
            .unwrap_or_else(|e| panic!("讀 {name} 的 expected.json 失敗：{e}"));
        f(&name, &sc, &expected);
    }
}

/// `expected["errors"]` 中以任一 `prefixes` 開頭的訊息（依原順序），套 [`normalize_error`]。
pub fn expected_errors(expected: &Value, prefixes: &[&str]) -> Vec<String> {
    expected["errors"]
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(Value::as_str)
        .filter(|m| prefixes.iter().any(|p| m.starts_with(p)))
        .map(normalize_error)
        .collect()
}

/// 該來源的 `errors`（正規化後）等於 `expected.json` 中屬於該來源前綴的子序列。
pub fn assert_errors(name: &str, got: &[String], expected: &Value, prefixes: &[&str]) {
    let got: Vec<String> = got.iter().map(|m| normalize_error(m)).collect();
    assert_eq!(
        got,
        expected_errors(expected, prefixes),
        "情境 {name}：errors 不符"
    );
}

/// `expected.json` 的 `events` 中 `type == kind` 的元素（依原順序）。B-EVT 的合併、窗口、去重、排序
/// 都在 `expected.events` 裡做完了，所以單一來源的輸出要先套 [`super::events::merge_events`] 再來比。
pub fn expected_events(expected: &Value, kind: &str) -> Vec<Value> {
    expected["events"]
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter(|e| e["type"] == kind)
        .cloned()
        .collect()
}
