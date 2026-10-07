//! `fc-host.exe --fetch-once <目錄>`（data-layer-rust tasks.md 5.1；design.md D10；spec「單次抓取指令」）：
//! 抓一輪、寫入 `<目錄>/tw_events.json`（該目錄已有的同名檔視為上一份輸出）後結束。
//!
//! 位置與 `--restore-wallpaper` 相同：`main()` 在 `logging::init()` 之後、單一執行個體仲裁與
//! `tauri::Builder` 之前攔下旗標，整個行程交給 [`run`]，以結束碼回報。因此：
//!
//! - 不看 `data_fetch` 設定、不讀寫排程紀錄（5.2／5.3 的東西）、不經單一執行個體（常駐宿主在跑也能同時執行）、
//!   不建立任何視窗或系統匣、不登記重新啟動；
//! - 暫存檔名含行程 ID（`tw_events.json.<pid>.tmp`，見 `output::write_atomic`），與常駐宿主同時寫同一目錄
//!   不互相破壞。用途：實網對照（6.1）、除錯、宿主排程出問題時的手動補救。
//!
//! ## 結束碼
//!
//! | 碼 | 意義 |
//! |---|---|
//! | 0 | 寫檔成功。**即使所有來源都失敗、寫出的全是沿用的舊資料也是 0**（K-15；來源狀況看記錄檔與輸出的 `errors`） |
//! | 1 | 寫檔失敗；或缺目錄參數／目錄建不起來；或建不起 HTTP client／tokio runtime；或整輪組裝 panic（不寫檔） |
//!
//! ## 流程
//!
//! 解析參數 → 開始行 → 建立目錄 → 等網路（[`net::wait_for_network`]，逾時放行並把 `errors` 第一筆標成逾時）→
//! [`exec::execute_round`]（讀 `<目錄>/tw_events.json` 當舊輸出 → 跑一輪 → 寫檔）。每一步都記進宿主記錄檔
//! （target [`LOG_TARGET`]，順序與格式見 [`super::report`]；與 5.2 排程共用同一套）。
//!
//! ## 測試注入
//!
//! 核心 [`fetch_once_with`] 吃 `Fetch`＋`Clock`（正式入口 [`run`] 接上 `ReqwestFetch` 與 `Clock::now()`），
//! 單元測試以 `FixtureFetch`＋凍結時鐘呼叫它——是普通函式參數，不是環境變數或旗標，所以正式建置沒有
//! 可由外部觸發的注入點。

use std::fs;
use std::path::{Path, PathBuf};

use log::Level;

use super::clock::Clock;
use super::exec::{self, ExecStatus};
use super::http::{Fetch, ReqwestFetch};
use super::net;
use super::report::{self, Line, TRIGGER_FETCH_ONCE};

/// 命令列旗標。
pub const FLAG: &str = "--fetch-once";

/// 寫檔成功（含「全部來源失敗但寫出沿用資料」，K-15）。
pub const EXIT_OK: i32 = 0;
/// 失敗（寫檔失敗、參數或環境錯誤、整輪 panic）。
pub const EXIT_FAIL: i32 = 1;

/// 命令列是否帶 `--fetch-once`（第一個元素是執行檔路徑，略過）。
pub fn requested(args: &[String]) -> bool {
    args.iter().skip(1).any(|a| a == FLAG)
}

/// 取出 `--fetch-once` 後面的目錄。缺參數（旗標在最後）、後面接的是另一個旗標（`--` 開頭）或空字串都是錯誤。
pub fn parse_dir(args: &[String]) -> Result<PathBuf, String> {
    let pos = args
        .iter()
        .skip(1)
        .position(|a| a == FLAG)
        .map(|i| i + 1)
        .ok_or_else(|| format!("沒有 {FLAG}"))?;
    match args.get(pos + 1) {
        Some(d) if !d.is_empty() && !d.starts_with("--") => Ok(PathBuf::from(d)),
        _ => Err(format!("{FLAG} 後面必須接輸出目錄")),
    }
}

/// 正式入口：解析 `std::env::args()`、建目錄、等網路、抓一輪、寫檔；回傳結束碼（呼叫端 `process::exit`）。
pub fn run() -> i32 {
    let args: Vec<String> = std::env::args().collect();
    run_with(
        &args,
        ReqwestFetch::new,
        || net::wait_for_network(net::TIMEOUT_EACH, net::INTERVAL, net::MAX_WAIT),
        Clock::now,
        &mut |(level, text): Line| report::write_log(level, &text),
    )
}

/// [`run`] 的可注入版本：`make_fetch` 建 HTTP client、`wait_network` 等網路（回傳是否就緒）、`now` 取時鐘
/// （在等網路**之後**取，所以時間反映實際開始抓取的時刻），每一行記錄交給 `sink`。
fn run_with<F: Fetch, E: std::fmt::Display>(
    args: &[String],
    make_fetch: impl FnOnce() -> Result<F, E>,
    wait_network: impl FnOnce() -> bool,
    now: impl FnOnce() -> Clock,
    sink: &mut (dyn FnMut(Line) + Send),
) -> i32 {
    let dir = match parse_dir(args) {
        Ok(d) => d,
        Err(e) => {
            sink((Level::Error, format!("{FLAG} 參數錯誤：{e}")));
            return EXIT_FAIL;
        }
    };
    // 開始行最先（B-FLOW-18）：目錄建立失敗、等網路的記錄都排在它後面。
    sink(report::start_line(TRIGGER_FETCH_ONCE, &dir));
    if let Err(e) = fs::create_dir_all(&dir) {
        sink((
            Level::Error,
            format!("{FLAG} 輸出目錄 {} 不存在且建立失敗：{e}", dir.display()),
        ));
        return EXIT_FAIL;
    }
    let fetch = match make_fetch() {
        Ok(f) => f,
        Err(e) => {
            sink((Level::Error, format!("{FLAG} 建立 HTTP client 失敗：{e}")));
            return EXIT_FAIL;
        }
    };
    let net_ok = wait_network();
    fetch_once_core(&fetch, &now(), &dir, !net_ok, sink)
}

/// `--fetch-once` 的核心（不碰命令列、網路探測與真實網路）：開始行與等網路已由呼叫端處理，這裡從「讀舊檔、
/// 跑一輪、寫檔」開始，每一行記錄都交給 `sink`（正式入口寫進宿主記錄檔，測試收集起來斷言），回傳結束碼。
/// 注入點是普通函式參數（`Fetch`＋`Clock`），不是環境變數或旗標，所以正式建置沒有可由外部觸發的後門。
fn fetch_once_core<F: Fetch>(
    fetch: &F,
    clock: &Clock,
    dir: &Path,
    net_timed_out: bool,
    sink: &mut (dyn FnMut(Line) + Send),
) -> i32 {
    let never_stop = || false;
    let executed = exec::execute_round(fetch, clock, dir, net_timed_out, &never_stop, sink);
    match executed.status {
        ExecStatus::Written => EXIT_OK,
        ExecStatus::WriteFailed
        | ExecStatus::RoundPanicked
        | ExecStatus::SetupFailed
        | ExecStatus::Aborted => EXIT_FAIL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::errors::normalize_output;
    use crate::fetch::fixture::Scenario;
    use crate::fetch::output::OUTPUT_FILE;
    use crate::fetch::round;
    use crate::fetch::test_util::TempDir;
    use serde_json::Value;
    use std::sync::Mutex;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn run_core(sc: &Scenario, dir: &Path) -> (i32, Vec<Line>) {
        let mut lines = Vec::new();
        let code = fetch_once_core(&sc.fetch, &sc.clock, dir, false, &mut |l| lines.push(l));
        (code, lines)
    }

    fn read_output(dir: &Path) -> Value {
        let bytes = fs::read(dir.join(OUTPUT_FILE)).expect("輸出檔存在");
        normalize_output(serde_json::from_slice(&bytes).expect("輸出檔是合法 JSON"))
    }

    fn tmp_leftovers(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect()
    }

    // ───────────── 參數 ─────────────

    #[test]
    fn requested_looks_only_at_arguments_after_the_exe() {
        assert!(requested(&args(&[
            "fc-host.exe",
            "--fetch-once",
            "D:\\out"
        ])));
        assert!(requested(&args(&["fc-host.exe", "--fetch-once"])));
        assert!(!requested(&args(&["fc-host.exe"])));
        assert!(!requested(&args(&["--fetch-once"]))); // 第一個是執行檔路徑
        assert!(!requested(&args(&["fc-host.exe", "--restore-wallpaper"])));
    }

    #[test]
    fn parse_dir_takes_the_next_argument() {
        let got = parse_dir(&args(&["fc-host.exe", "--fetch-once", "D:\\my out"])).unwrap();
        assert_eq!(got, PathBuf::from("D:\\my out"));
        let got = parse_dir(&args(&["fc-host.exe", "--log", "x", "--fetch-once", "o"])).unwrap();
        assert_eq!(got, PathBuf::from("o"));
    }

    #[test]
    fn parse_dir_rejects_a_missing_or_flag_like_directory() {
        for a in [
            args(&["fc-host.exe", "--fetch-once"]),
            args(&["fc-host.exe", "--fetch-once", ""]),
            args(&["fc-host.exe", "--fetch-once", "--log", "x"]),
            args(&["fc-host.exe"]),
        ] {
            assert!(parse_dir(&a).is_err(), "{a:?}");
        }
    }

    // ───────────── 整合：FixtureFetch＋固定時鐘 ─────────────

    #[test]
    fn recorded_scenario_writes_a_file_equal_to_expected_json() {
        let sc = Scenario::named("recorded-20261005").unwrap();
        let dir = TempDir::new("fetch-once-recorded");
        let (code, lines) = run_core(&sc, dir.path());
        assert_eq!(code, EXIT_OK, "{lines:?}");

        let got = read_output(dir.path());
        let expected = normalize_output(sc.expected().unwrap());
        assert_eq!(got, expected, "輸出檔與 Python 的 expected.json 不等價");
        assert!(tmp_leftovers(dir.path()).is_empty(), "不該留下暫存檔");

        // 日誌（開始行與等網路由 `run` 負責，不在核心裡）：窗口行 → 各來源 → 已輸出 → 最後才是完成摘要
        // （5.1 審查 m1：與 Python 的順序一致）。
        let texts: Vec<&str> = lines.iter().map(|(_, t)| t.as_str()).collect();
        assert!(texts[0].starts_with("抓取"), "{}", texts[0]);
        let n = texts.len();
        assert!(
            texts[n - 2].starts_with("已輸出 → ") && texts[n - 2].contains("位元組"),
            "{}",
            texts[n - 2]
        );
        assert_eq!(lines[n - 2].0, Level::Info);
        assert!(texts[n - 1].starts_with("完成："), "{}", texts[n - 1]);
        assert_eq!(texts.iter().filter(|t| t.starts_with("完成：")).count(), 1);
        assert!(
            !texts.iter().any(|t| t.starts_with("開始抓取一輪")),
            "開始行不在核心裡（它要排在等網路之前）"
        );
    }

    #[test]
    fn the_second_run_on_the_first_output_is_stable() {
        let sc = Scenario::named("recorded-20261005").unwrap();
        let dir = TempDir::new("fetch-once-twice");
        let (c1, _) = run_core(&sc, dir.path());
        let first = read_output(dir.path());
        let (c2, _) = run_core(&sc, dir.path());
        let second = read_output(dir.path());
        assert_eq!((c1, c2), (EXIT_OK, EXIT_OK));
        assert_eq!(first, second, "以第一次輸出當舊檔重跑，結果必須不變");
    }

    #[test]
    fn all_sources_failing_still_writes_the_old_data_and_exits_zero() {
        // K-15：全部來源失敗、寫出的是沿用的舊資料，結束碼仍是 0。
        let sc = Scenario::named("all-sources-fail-with-old").unwrap();
        let dir = TempDir::new("fetch-once-allfail");
        sc.install_old_into(dir.path()).unwrap();
        let (code, lines) = run_core(&sc, dir.path());
        assert_eq!(code, EXIT_OK, "{lines:?}");
        let got = read_output(dir.path());
        assert_eq!(got, normalize_output(sc.expected().unwrap()));
        assert!(
            !got["errors"].as_array().unwrap().is_empty(),
            "來源失敗要留在 errors"
        );
        let summary = lines.iter().find(|(_, t)| t.starts_with("完成：")).unwrap();
        assert_eq!(summary.0, Level::Warn, "有警告的摘要用 warn：{}", summary.1);
    }

    #[test]
    fn net_timeout_flag_reaches_errors() {
        let sc = Scenario::named("recorded-20261005").unwrap();
        let dir = TempDir::new("fetch-once-nettimeout");
        let code = fetch_once_core(&sc.fetch, &sc.clock, dir.path(), true, &mut |_| {});
        assert_eq!(code, EXIT_OK);
        let got = read_output(dir.path());
        assert_eq!(got["errors"][0], round::MSG_NET_TIMEOUT);
    }

    #[test]
    fn a_missing_directory_is_created() {
        let sc = Scenario::named("recorded-20261005").unwrap();
        let base = TempDir::new("fetch-once-mkdir");
        let dir = base.path().join("a").join("b");
        let (code, _) = run_core(&sc, &dir);
        assert_eq!(code, EXIT_OK);
        assert!(dir.join(OUTPUT_FILE).is_file());
    }

    #[test]
    fn an_unwritable_directory_exits_one_and_logs_the_error() {
        // 「目錄」其實是一個既有的普通檔案：建目錄與寫暫存檔都不可能成功（不依賴 ACL，跨環境穩定）。
        let sc = Scenario::named("recorded-20261005").unwrap();
        let base = TempDir::new("fetch-once-unwritable");
        let not_a_dir = base.path().join("im-a-file");
        fs::write(&not_a_dir, "x").unwrap();
        let (code, lines) = run_core(&sc, &not_a_dir);
        assert_eq!(code, EXIT_FAIL);
        let failure = lines
            .iter()
            .find(|(_, t)| t.starts_with("輸出到 "))
            .expect("有寫檔失敗那一行");
        assert_eq!(failure.0, Level::Error, "{failure:?}");
        assert!(failure.1.contains("失敗："), "{}", failure.1);
        // Python 的順序：不論寫檔成敗，最後都印「完成：…」。
        assert!(lines.last().unwrap().1.starts_with("完成："), "{lines:?}");
        assert_eq!(fs::read(&not_a_dir).unwrap(), b"x", "不得動到那個檔案");
    }

    #[test]
    fn the_start_line_comes_before_the_network_wait_and_the_round() {
        // 5.1 審查 m1：每輪第一行是開始行，等網路（它自己會記 `[網路]` 行）與各來源都在它後面。
        let sc = Scenario::named("recorded-20261005").unwrap();
        let dir = TempDir::new("fetch-once-order");
        let events = Mutex::new(Vec::<String>::new());
        let argv = args(&["fc-host.exe", "--fetch-once", &dir.path().to_string_lossy()]);
        let mut sink = |(_, text): Line| events.lock().unwrap().push(text);
        let code = run_with(
            &argv,
            || Ok::<_, String>(sc.fetch),
            || {
                events.lock().unwrap().push("<等網路>".to_string());
                true
            },
            || sc.clock,
            &mut sink,
        );
        assert_eq!(code, EXIT_OK);
        let events = events.lock().unwrap();
        assert!(
            events[0].starts_with("開始抓取一輪 觸發=fetch-once"),
            "{}",
            events[0]
        );
        assert!(events[0].contains(&dir.path().display().to_string()));
        assert_eq!(events[1], "<等網路>");
        assert!(events[2].starts_with("抓取"), "{}", events[2]);
        assert!(events.last().unwrap().starts_with("完成："));
    }

    #[test]
    fn a_network_timeout_reaches_errors_through_run_with() {
        let sc = Scenario::named("recorded-20261005").unwrap();
        let dir = TempDir::new("fetch-once-order-timeout");
        let argv = args(&["fc-host.exe", "--fetch-once", &dir.path().to_string_lossy()]);
        let code = run_with(
            &argv,
            || Ok::<_, String>(sc.fetch),
            || false,
            || sc.clock,
            &mut |_| {},
        );
        assert_eq!(code, EXIT_OK);
        assert_eq!(read_output(dir.path())["errors"][0], round::MSG_NET_TIMEOUT);
    }

    #[test]
    fn argument_and_setup_errors_exit_one_and_say_why() {
        let sc = Scenario::named("recorded-20261005").unwrap();
        let mut lines = Vec::new();
        let code = run_with(
            &args(&["fc-host.exe", "--fetch-once"]),
            || {
                Ok::<crate::fetch::test_util::PanicFetch, String>(
                    crate::fetch::test_util::PanicFetch,
                )
            },
            || panic!("參數錯誤不該等網路"),
            || sc.clock,
            &mut |l| lines.push(l),
        );
        assert_eq!(code, EXIT_FAIL);
        assert_eq!(lines[0].0, Level::Error);
        assert!(lines[0].1.contains("必須接輸出目錄"), "{}", lines[0].1);

        let dir = TempDir::new("fetch-once-client-fail");
        let argv = args(&["fc-host.exe", "--fetch-once", &dir.path().to_string_lossy()]);
        let mut lines = Vec::new();
        let code = run_with(
            &argv,
            || Err::<crate::fetch::fixture::FixtureFetch, _>("TLS 初始化失敗".to_string()),
            || panic!("client 建不起來不該等網路"),
            || sc.clock,
            &mut |l| lines.push(l),
        );
        assert_eq!(code, EXIT_FAIL);
        assert!(lines
            .last()
            .unwrap()
            .1
            .contains("建立 HTTP client 失敗：TLS 初始化失敗"));
        assert!(!dir.path().join(OUTPUT_FILE).exists());
    }

    #[test]
    fn stale_tmp_files_from_a_killed_run_are_cleaned_by_the_next_write() {
        // 殘留的暫存檔（行程被殺）超過 1 小時就在寫檔前清掉；剛留下的（可能是常駐宿主正在寫）不動。
        let sc = Scenario::named("recorded-20261005").unwrap();
        let dir = TempDir::new("fetch-once-stale");
        let stale = dir.path().join("tw_events.json.4242.tmp");
        let fresh = dir.path().join("tw_events.json.4343.tmp");
        fs::write(&stale, "x").unwrap();
        fs::write(&fresh, "x").unwrap();
        let f = fs::OpenOptions::new().write(true).open(&stale).unwrap();
        f.set_modified(std::time::SystemTime::now() - 2 * round::STALE_TMP_AGE)
            .unwrap();
        drop(f);
        let (code, _) = run_core(&sc, dir.path());
        assert_eq!(code, EXIT_OK);
        assert!(!stale.exists());
        assert!(fresh.exists());
    }
}
