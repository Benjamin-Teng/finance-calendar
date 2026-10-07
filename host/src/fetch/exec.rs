//! 執行一輪並寫檔（`--fetch-once` 與 5.2 排程共用）。
//!
//! [`execute_round`] 做的事：讀 `<dir>/tw_events.json` 當上一份輸出 → 自建 `current_thread` tokio runtime 跑
//! [`run_round_with`] → 寫檔 → 把記錄行依序交給 `sink`。呼叫端負責在它**之前**送出開始行（[`super::report::start_line`]）
//! 與等網路（[`super::net`]），因為那兩者的記錄要排在各來源之前（B-FLOW-18 的順序）。
//!
//! 送出順序（與 Python 一致，見 [`super::report`]）：各來源的日誌行在**來源跑完當下**經 `RoundHooks::on_log` 送出
//! （5.1 審查 m2：記錄檔的時間戳反映實際耗時）→ 寫檔結果行（`已輸出 → …` 或 `輸出到 … 失敗：…`）→ 最後才是
//! 「完成：…」摘要（5.1 審查 m1）。整輪組裝 panic 時只有一行 `[整輪] 非預期錯誤，本輪不寫檔：…`（`error`）。
//!
//! # 停止
//!
//! `should_stop` 在每個來源跑完後被問一次（[`RoundHooks::should_stop`]）：回 `true` 就中止，**不寫檔**，回傳
//! [`ExecStatus::Aborted`]。最後一個來源之後不再問（輸出已完整，照常寫檔）。

use std::path::Path;

use log::Level;

use super::clock::Clock;
use super::http::Fetch;
use super::output;
use super::report::{self, Line};
use super::round::{self, RoundHooks, RoundResult};

/// 一輪的結局。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecStatus {
    /// 寫檔成功（含「全部來源失敗、寫出沿用的舊資料」，K-15）。
    Written,
    /// 寫檔失敗（記錄已送出）。
    WriteFailed,
    /// 整輪組裝 panic（D7 最後防線）：不寫檔。
    RoundPanicked,
    /// 建不起 tokio runtime：不寫檔。
    SetupFailed,
    /// 被停止要求在來源邊界中止：不寫檔、不算失敗。
    Aborted,
}

/// [`execute_round`] 的回傳。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Executed {
    pub status: ExecStatus,
    /// 本輪是否有任何來源成功（[`RoundResult::any_source_ok`]）；沒有跑完一輪時為 `false`。
    pub any_source_ok: bool,
}

/// 跑一輪並寫到 `dir`（見模組文件）。`sink` 收到每一行記錄（等級與內容）。
pub fn execute_round<F: Fetch>(
    fetch: &F,
    clock: &Clock,
    dir: &Path,
    net_timed_out: bool,
    should_stop: &(dyn Fn() -> bool + Sync),
    sink: &mut (dyn FnMut(Line) + Send),
) -> Executed {
    let prev = output::read_previous(dir);
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            sink((Level::Error, format!("建立 tokio runtime 失敗：{e}")));
            return Executed {
                status: ExecStatus::SetupFailed,
                any_source_ok: false,
            };
        }
    };
    let result = {
        let mut on_log = |text: &str| sink(report::line_of(text));
        let mut hooks = RoundHooks {
            on_log: Some(&mut on_log),
            should_stop: Some(should_stop),
        };
        runtime.block_on(round::run_round_with(
            fetch,
            clock,
            &prev,
            net_timed_out,
            &mut hooks,
        ))
    };
    finish_round(result, dir, sink)
}

/// 一輪組裝完成後的收尾：依結果寫檔並送出剩下的記錄行（見模組文件的順序）。
pub fn finish_round(
    result: RoundResult,
    dir: &Path,
    sink: &mut (dyn FnMut(Line) + Send),
) -> Executed {
    let any_source_ok = result.any_source_ok;
    if result.aborted {
        sink((
            Level::Warn,
            "收到停止要求，已在來源邊界中止本輪，不寫檔".to_string(),
        ));
        return Executed {
            status: ExecStatus::Aborted,
            any_source_ok: false,
        };
    }
    let output = match result.output {
        Ok(o) => o,
        Err(_) => {
            // `logs` 只有 `[整輪] 非預期錯誤，本輪不寫檔：…` 一行（來源日誌沒有機會送出）。
            for line in &result.logs {
                sink(report::line_of(line));
            }
            return Executed {
                status: ExecStatus::RoundPanicked,
                any_source_ok: false,
            };
        }
    };
    let written = round::write_round(dir, &output);
    sink(report::write_line(dir, &written));
    // 摘要是 `logs` 的最後一行（`assemble` 保證）；來源各行已在跑完當下送出。
    if let Some(summary) = result.logs.last() {
        sink(report::line_of(summary));
    }
    Executed {
        status: if written.is_ok() {
            ExecStatus::Written
        } else {
            ExecStatus::WriteFailed
        },
        any_source_ok,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::fixture::Scenario;
    use crate::fetch::output::OUTPUT_FILE;
    use crate::fetch::test_util::TempDir;

    fn result_without_output(aborted: bool) -> RoundResult {
        RoundResult {
            output: Err("boom".to_string()),
            logs: vec!["[整輪] 非預期錯誤，本輪不寫檔：boom".to_string()],
            any_source_ok: false,
            aborted,
        }
    }

    #[test]
    fn a_round_that_panicked_writes_nothing_and_logs_one_error_line() {
        let dir = TempDir::new("exec-panicked");
        let mut lines = Vec::new();
        let executed = finish_round(result_without_output(false), dir.path(), &mut |l| {
            lines.push(l)
        });
        assert_eq!(executed.status, ExecStatus::RoundPanicked);
        assert!(!executed.any_source_ok);
        assert!(!dir.path().join(OUTPUT_FILE).exists(), "Err 時不得寫檔");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].0, Level::Error);
        assert!(lines[0].1.contains("本輪不寫檔"), "{lines:?}");
    }

    #[test]
    fn an_aborted_round_writes_nothing_and_is_not_a_failure() {
        let dir = TempDir::new("exec-aborted");
        let mut lines = Vec::new();
        let executed = finish_round(result_without_output(true), dir.path(), &mut |l| {
            lines.push(l)
        });
        assert_eq!(executed.status, ExecStatus::Aborted);
        assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].0, Level::Warn);
        assert!(lines[0].1.contains("停止"), "{lines:?}");
    }

    #[test]
    fn a_successful_round_reports_written_and_any_source_ok() {
        let sc = Scenario::named("recorded-20261005").unwrap();
        let dir = TempDir::new("exec-written");
        let mut lines = Vec::new();
        let executed = execute_round(
            &sc.fetch,
            &sc.clock,
            dir.path(),
            false,
            &|| false,
            &mut |l| lines.push(l),
        );
        assert_eq!(executed.status, ExecStatus::Written);
        assert!(executed.any_source_ok);
        // 順序：窗口 → 各來源 → 已輸出 → 完成摘要（最後一行）。
        let n = lines.len();
        assert!(lines[0].1.starts_with("抓取"));
        assert!(lines[n - 2].1.starts_with("已輸出 → "));
        assert!(lines[n - 1].1.starts_with("完成："));
    }

    #[test]
    fn an_unwritable_directory_is_a_write_failure_and_the_summary_still_follows() {
        let sc = Scenario::named("recorded-20261005").unwrap();
        let base = TempDir::new("exec-writefail");
        let not_a_dir = base.path().join("im-a-file");
        std::fs::write(&not_a_dir, "x").unwrap();
        let mut lines = Vec::new();
        let executed = execute_round(
            &sc.fetch,
            &sc.clock,
            &not_a_dir,
            false,
            &|| false,
            &mut |l| lines.push(l),
        );
        assert_eq!(executed.status, ExecStatus::WriteFailed);
        let n = lines.len();
        assert_eq!(lines[n - 2].0, Level::Error, "{lines:?}");
        assert!(lines[n - 2].1.starts_with("輸出到 "), "{}", lines[n - 2].1);
        assert!(lines[n - 1].1.starts_with("完成："));
        assert_eq!(std::fs::read(&not_a_dir).unwrap(), b"x");
    }
}
