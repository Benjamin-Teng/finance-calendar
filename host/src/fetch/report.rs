//! 抓取日誌（tasks.md 5.4；behavior-inventory B-FLOW-18）：把一輪的過程寫進宿主記錄檔（`logging.rs`，
//! 每日輪替的同一份檔案），target 一律 [`LOG_TARGET`]。
//!
//! 這裡只負責**組字串與決定等級**（純函式、單元測試直接斷言），實際寫出由 [`emit`] 經 `log` facade 做，
//! 測試不必安裝全域記錄器。呼叫端（`--fetch-once` 的 [`super::cli`]、5.2 的排程 [`super::scheduler`]）依序送出，
//! 順序與 Python 一致（B-FLOW-18）：
//!
//! 1. [`start_line`]：每輪開始一行，帶觸發原因（[`TRIGGER_FETCH_ONCE`] 或
//!    [`super::sched::Trigger::as_str`] 的 `first-fetch`／`slot`／`missed`／`retry`）與輸出目錄；
//!    等網路的 `[網路] …` 行在它之後；
//! 2. 各來源的日誌行（[`line_of`] 配上等級），**每個來源跑完當下就送出**（`RoundHooks::on_log`），
//!    所以記錄檔的時間戳反映各來源實際花的時間；
//! 3. [`write_line`]：寫檔結果一行（路徑與位元組數，或錯誤）；
//! 4. 最後才是 `RoundResult.logs` 結尾的「完成：…」摘要（Python 也是寫檔後才印）。
//!
//! 等級：沿用舊資料、失敗、逾時、跳過一律 `warn`；整輪組裝 panic 與寫檔失敗是 `error`；其餘 `info`。
//! 日誌**不含時間戳**（記錄器自己加）。

use std::io;
use std::path::Path;

use log::Level;

use super::output::OUTPUT_FILE;

/// 抓取相關記錄的 target（與 `fc_host::wallpaper_cli` 等並列，可依 target 過濾）。
pub const LOG_TARGET: &str = "fc_host::fetch";

/// `--fetch-once` 的觸發原因參數。
pub const TRIGGER_FETCH_ONCE: &str = "fetch-once";

/// 一行記錄：等級與內容。
pub type Line = (Level, String);

/// 每輪開始一行：觸發原因與輸出目錄。
///
/// 在等網路**之前**送出（B-FLOW-18 的順序），所以不含等網路是否逾時；逾時由 `net::wait_for_network` 自己
/// 記一行 `warn`，並寫進輸出的 `errors` 第一筆。
pub fn start_line(trigger: &str, dir: &Path) -> Line {
    (
        Level::Info,
        format!("開始抓取一輪 觸發={trigger} 輸出目錄={}", dir.display()),
    )
}

/// 一行日誌（Python 風格的中文句子）配上等級。
pub fn line_of(text: &str) -> Line {
    (level_of(text), text.to_string())
}

/// 依內容判斷等級（日誌行是 Python 風格的中文句子，沒有結構化等級）。
fn level_of(line: &str) -> Level {
    if line.starts_with("[整輪]") {
        return Level::Error;
    }
    if let Some(rest) = line.strip_prefix("完成：") {
        return if rest.contains("；警告") || rest.contains("；桌布資料警告") {
            Level::Warn
        } else {
            Level::Info
        };
    }
    const WARN_WORDS: [&str; 6] = ["失敗", "沿用", "逾時", "跳過", "改用", "非預期"];
    if WARN_WORDS.iter().any(|w| line.contains(w)) {
        Level::Warn
    } else {
        Level::Info
    }
}

/// 寫檔結果一行：成功＝`已輸出 → <路徑>（N 位元組）`（位元組數讀不到就省略），失敗＝
/// `輸出到 <目錄> 失敗：<錯誤>`（B-FLOW-13 的措辭）。
pub fn write_line(dir: &Path, result: &io::Result<()>) -> Line {
    match result {
        Ok(()) => {
            let path = dir.join(OUTPUT_FILE);
            let size = std::fs::metadata(&path)
                .map(|m| format!("（{} 位元組）", m.len()))
                .unwrap_or_default();
            (Level::Info, format!("已輸出 → {}{size}", path.display()))
        }
        Err(e) => (Level::Error, format!("輸出到 {} 失敗：{e}", dir.display())),
    }
}

/// 經 `log` facade 寫入宿主記錄檔。
pub fn write_log(level: Level, text: &str) {
    log::log!(target: LOG_TARGET, level, "{text}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::test_util::TempDir;

    #[test]
    fn start_line_carries_trigger_and_dir() {
        let (lv, text) = start_line(TRIGGER_FETCH_ONCE, Path::new("D:\\out"));
        assert_eq!(lv, Level::Info);
        assert!(text.contains("觸發=fetch-once"), "{text}");
        assert!(text.contains("D:\\out"), "{text}");
        let (_, text) = start_line("slot", Path::new("D:\\out"));
        assert!(text.contains("觸發=slot"), "{text}");
    }

    #[test]
    fn levels_follow_the_python_style_sentences() {
        let cases = [
            (
                "抓取總經日曆＋台股動態事件＋行情＋休市日曆（2026-10-05 ~ 2026-10-26）…",
                Level::Info,
            ),
            ("[總經] 下週檔尚未發布（HTTP 404）", Level::Info),
            ("[總經] 本輪失敗，沿用上次資料", Level::Warn),
            (
                "[除權息] 即時 API 失敗（x），改用 openapi 備援",
                Level::Warn,
            ),
            ("[處置股] 上市沿用上次資料（3 筆）", Level::Warn),
            ("[整輪] 非預期錯誤，本輪不寫檔：boom", Level::Error),
            (
                "完成：總經 5、除權息 1 筆、行情 13/13、休市日曆 20 筆",
                Level::Info,
            ),
            (
                "完成：總經 0、行情 0/13、休市日曆 抓取失敗；警告 2 項：a；b",
                Level::Warn,
            ),
            ("完成：總經 5；桌布資料警告 1 項：x", Level::Warn),
        ];
        for (line, want) in cases {
            assert_eq!(level_of(line), want, "{line}");
        }
    }

    #[test]
    fn line_of_pairs_the_text_with_its_level() {
        assert_eq!(line_of("a"), (Level::Info, "a".to_string()));
        let (lv, text) = line_of("[總經] 本輪失敗，沿用上次資料");
        assert_eq!(lv, Level::Warn);
        assert_eq!(text, "[總經] 本輪失敗，沿用上次資料");
    }

    #[test]
    fn write_line_reports_path_and_size_or_the_error() {
        let dir = TempDir::new("report-write-line");
        std::fs::write(dir.path().join(OUTPUT_FILE), "12345").unwrap();
        let (lv, text) = write_line(dir.path(), &Ok(()));
        assert_eq!(lv, Level::Info);
        assert!(text.starts_with("已輸出 → "), "{text}");
        assert!(
            text.contains(OUTPUT_FILE) && text.contains("（5 位元組）"),
            "{text}"
        );

        let err = Err(io::Error::other("磁碟已滿"));
        let (lv, text) = write_line(dir.path(), &err);
        assert_eq!(lv, Level::Error);
        assert!(
            text.starts_with("輸出到 ") && text.contains("失敗：磁碟已滿"),
            "{text}"
        );
    }
}
