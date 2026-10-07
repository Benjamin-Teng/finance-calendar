//! 本機記錄檔（design.md D12「記錄：本機檔案、每日輪替、保留 7 天」；task 5.8）。
//!
//! - [`default_log_dir`]：`%LOCALAPPDATA%\tw.fintools.fc-host\logs\`（task-5.8-brief 指定的
//!   路徑）。與 `settings::default_settings_path`／`default_gatekeeper_log_path` 的
//!   `%APPDATA%` 命名空間不同——記錄檔是本機易失資料，不像設定檔需要漫遊同步，放
//!   `LOCALAPPDATA` 較符合語意（Windows 慣例：`LOCALAPPDATA` 給不需漫遊的機器本機資料）。
//! - [`DailyRotatingLogger`]：`log::Log` 實作。每次寫入前檢查「今天」是否已變（`now` 由呼叫端
//!   注入，正式執行傳 [`crate::desktop::today`]），變了就換一份新檔並清掉超過保留天數的舊檔。
//! - [`init`]：`main()` 最開頭呼叫一次，安裝為 `log` crate 的全域 logger（`log::set_boxed_
//!   logger`），之後全 crate 都能用 `log::info!`／`log::warn!`／`log::error!` 巨集。
//!
//! ## 為什麼不是 tauri-plugin-log
//!
//! task 5.8 brief 要求「優先評估 tauri-plugin-log」。查過該 crate 原始碼
//! （`tauri-plugin-log-2.10.0/src/lib.rs`）：它的 `RotatingFile` 只在檔案**超過
//! `max_file_size`（位元組數）**時才輪替（`RotationStrategy::KeepAll`／`KeepOne`／
//! `KeepSome(n)`），沒有「行事曆日改變就換檔」這個策略；唯一跟「新檔」沾邊的
//! `FileOpenStrategy::Rotate` 是「**每次行程啟動**」輪替一次，不是「每天」——本宿主是常駐
//! 系統匣的長駐行程（design.md D12、task 6.2 要求連續執行 24 小時含睡眠喚醒），可能好幾天
//! 不重啟，用 `Rotate` 達不到「每日」語意；用純位元組數的 `KeepOne`／`KeepSome` 則完全沒有
//! 「保留 7 天」這個以行事曆日為單位的概念，只能保留「幾份檔案」，天數會因為記錄量而伸縮，
//! 不符合 design.md「保留 7 天」的明確語意。且該 plugin 一旦 `build()`／`split()` 建好、
//! 用 `log::set_boxed_logger` 裝上全域 logger 後就是不透明的 `Box<dyn log::Log>`，沒有介面能
//! 在單元測試裡注入「今天是哪一天」——brief 明講「輪替與 7 天保留以單元測試（注入時間）
//! 驗證」，這與該 plugin 的設計目標（一般桌面 App 的開發期記錄、依大小滾動）錯位。
//!
//! 改採「`log` facade crate＋自寫的每日輪替」（brief 允許的另一個選項）：`log` 本身只是
//! `main.rs`／`desktop.rs`／`widgets.rs`／`data.rs` 已經想用的 `log::info!`／`warn!`／
//! `error!` 巨集門面，全專案只多這一個必要依賴；輪替與保留天數的判定寫成純函式
//! （[`log_file_name`]／[`parse_log_file_date`]／[`is_expired`]），呼叫端注入「今天」與
//! 「保留天數」，因此能在不等真的過一天的情況下用固定日期做單元測試，也不需要 `chrono`／
//! `time` 這類完整曆法 crate（比照 `data.rs` 選擇 epoch 毫秒而非 `time` crate 的既有精神：
//! 這裡只需要「兩個日期差幾天」，不需要完整的行事曆換算，用眾所皆知的
//! Howard Hinnant `days_from_civil` 整數演算法自己實作、以往返測試自我驗證即可）。

#![allow(dead_code)]

use log::{LevelFilter, Log, Metadata, Record};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use crate::desktop;

/// 保留天數（design.md D12：「每日輪替、保留 7 天」）；含「今天」，即最舊可留到
/// 「今天－6 天」那一份，總共 7 個行事曆日。
pub const RETAIN_DAYS: u32 = 7;

const FILE_PREFIX: &str = "fc-host.";
const FILE_SUFFIX: &str = ".log";

/// 純日期（year, month, day），與時分秒無關——輪替以「行事曆日」為單位。
pub type Ymd = (i32, u32, u32);

/// 記錄檔目錄：`%LOCALAPPDATA%\tw.fintools.fc-host\logs\`。環境變數不存在時退回目前工作
/// 目錄，不 panic（同 `settings::default_settings_path` 既有慣例）。
pub fn default_log_dir() -> PathBuf {
    let base = env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("tw.fintools.fc-host").join("logs")
}

/// 民用曆 `(year, month, day)` → 自 1970-01-01 起的天數。只需要正向轉換（比較「今天」與
/// 「檔名日期」相差幾天），不需要反向還原日期，因此只實作這一半。
///
/// 演算法為 Howard Hinnant 公開發表的 `days_from_civil`
/// （<http://howardhinnant.github.io/date_algorithms.html>，公眾領域、僅整數運算，支援
/// proleptic Gregorian 曆），本檔以 Rust 重新實作。正確性以下方測試自我驗證（已知天數差、
/// 跨年、跨閏年 2 月），不依賴記憶中可能有誤的魔術數字。
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y: i64 = if m <= 2 {
        i64::from(y) - 1
    } else {
        i64::from(y)
    };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = (i64::from(m) + 9) % 12; // 3 月=0 …… 2 月=11
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// 記錄檔檔名：`fc-host.YYYY-MM-DD.log`。
pub fn log_file_name((y, m, d): Ymd) -> String {
    format!("{FILE_PREFIX}{y:04}-{m:02}-{d:02}{FILE_SUFFIX}")
}

/// 從檔名解析日期；格式不符（含月/日超出兩位數字、多餘片段）回傳 `None`——輪替清理時遇到
/// 不認得的檔名一律略過、不誤刪（例如使用者自己放進同目錄的東西，或未來版本换了命名慣例）。
pub fn parse_log_file_date(name: &str) -> Option<Ymd> {
    let rest = name.strip_prefix(FILE_PREFIX)?;
    let rest = rest.strip_suffix(FILE_SUFFIX)?;
    let mut parts = rest.split('-');
    let y: i32 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    let d: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None; // 多餘的 '-' 片段，不是預期格式
    }
    Some((y, m, d))
}

/// `file_date` 那份記錄檔相對於 `today` 是否已超過保留天數（含當天：偏移量
/// `0..retain_days-1` 保留，`>= retain_days` 視為過期）。未來日期（偏移量為負，例如系統時鐘
/// 被調回去後又调回来、或檔案本身時鐘飄移）一律不過期，不會被誤刪。
pub fn is_expired(file_date: Ymd, today: Ymd, retain_days: u32) -> bool {
    let offset = days_from_civil(today.0, today.1, today.2)
        - days_from_civil(file_date.0, file_date.1, file_date.2);
    offset >= i64::from(retain_days)
}

/// 掃描 `dir`，刪除超過 `retain_days` 的輪替檔（依檔名日期，見 [`is_expired`]）。任何一步
/// 失敗（目錄不存在、單一檔案刪除失敗、檔名非 UTF-8）都只略過，不中斷——記錄檔清理本身不該
/// 讓呼叫端（正在記一行事件）失敗，同 `settings::load_or_default`「不會失敗」的既有精神。
pub fn prune_old_logs(dir: &Path, today: Ymd, retain_days: u32) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(date) = parse_log_file_date(name) else {
            continue;
        };
        if is_expired(date, today, retain_days) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// 每日輪替記錄檔（`log::Log` 實作）。
///
/// `now` 由呼叫端提供「今天」的來源：正式執行傳 [`desktop::today`]（`GetLocalTime`，本機
/// 時區的行事曆日，與時鐘小工具一致）；單元測試傳固定或可自行推進的閉包，藉此不必真的等一天
/// 過去就能驗證換檔與清舊檔（task-5.8-brief「輪替與 7 天保留以單元測試（注入時間）驗證」）。
pub struct DailyRotatingLogger {
    inner: Mutex<Inner>,
    level: LevelFilter,
}

struct Inner {
    dir: PathBuf,
    retain_days: u32,
    now: Box<dyn Fn() -> Ymd + Send + Sync>,
    current_date: Option<Ymd>,
    file: Option<File>,
}

impl DailyRotatingLogger {
    /// 供測試與 [`with_local_clock`] 共用的建構子：`now` 完全由呼叫端決定，不碰任何時鐘。
    pub fn new(
        dir: PathBuf,
        retain_days: u32,
        level: LevelFilter,
        now: Box<dyn Fn() -> Ymd + Send + Sync>,
    ) -> Self {
        Self {
            inner: Mutex::new(Inner {
                dir,
                retain_days,
                now,
                current_date: None,
                file: None,
            }),
            level,
        }
    }

    /// 正式執行用：目錄取 [`default_log_dir`]，「今天」取 [`desktop::today`]（本機時區）。
    pub fn with_local_clock(retain_days: u32, level: LevelFilter) -> Self {
        Self::new(
            default_log_dir(),
            retain_days,
            level,
            Box::new(desktop::today),
        )
    }

    /// 取記錄器的鎖；被毒化（持鎖期間某次 panic）也照樣取回內部狀態繼續寫——`Inner` 只有
    /// 「目前日期＋開著的檔案」，半途 panic 頂多讓 `file` 為 `None`，下一次 `ensure_current_file`
    /// 會重開，沒有需要放棄的不一致狀態。反過來若毒化後就放棄記錄，之後所有記錄（含 panic
    /// hook 寫的那一行）都會靜默消失。
    fn lock_inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Inner {
    /// 確保目前開著的檔案對應「今天」；日期變了（含第一次呼叫，`current_date` 為 `None`）就
    /// 換檔並清掉超過保留天數的舊檔。任何 I/O 失敗都吞掉（`file` 維持 `None`，這一行記錄就
    /// 寫不出去）——記錄本身失敗不該讓呼叫端 panic 或中止。
    fn ensure_current_file(&mut self) {
        let today = (self.now)();
        if self.current_date == Some(today) && self.file.is_some() {
            return;
        }
        let _ = fs::create_dir_all(&self.dir);
        let path = self.dir.join(log_file_name(today));
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok();
        self.current_date = Some(today);
        prune_old_logs(&self.dir, today, self.retain_days);
    }
}

impl Log for DailyRotatingLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        // 先在**鎖外**格式化成單一 String（含結尾 \n）：fix F5（review fix-soak medium）——
        // 參數的 `Display`／`Debug` 實作 panic 時鎖還沒拿，不會毒化，panic hook 也能照常經
        // 記錄器寫下這次 panic。
        //
        // 單一 String 再一次 write_all，而非直接 `writeln!(file, ...)`：後者對 `Write` 的每個
        // 格式片段（字面／各參數）各自呼叫一次 `write_all`，等於一行拆成多次 WriteFile
        // syscall。多個行程（或本行程內多個獨立 File handle）同時 append 同一份記錄檔時，這些
        // 片段會互相交錯，把單行內容切開（task 5.1 壓測實測：6 行程同時啟動時「角色=Primary」
        // 字串被切斷、記錄檔統計失準，見 `.superpowers/sdd/tasks/task-5.1-report.md` 第
        // 245–256 行）。單次 write_all 對應單一 WriteFile syscall，搭配 append-mode 的
        // FILE_APPEND_DATA 語意（Windows）才具備「整行要嘛完整寫入、要嘛完全不寫入」的原子性。
        let line = format!(
            "{} [{:>5}] {}: {}\n",
            desktop::now_string(),
            record.level(),
            record.target(),
            record.args()
        );
        // 先立旗標再上鎖：持鎖期間的 panic（時鐘、I/O）由 panic hook 認出、不重入記錄器。
        let _in_logger = InLoggerGuard::enter();
        let mut inner = self.lock_inner();
        inner.ensure_current_file();
        if let Some(file) = inner.file.as_mut() {
            let _ = file.write_all(line.as_bytes());
            let _ = file.flush();
        }
    }

    fn flush(&self) {
        let _in_logger = InLoggerGuard::enter();
        if let Some(file) = self.lock_inner().file.as_mut() {
            let _ = file.flush();
        }
    }
}

/// 安裝為 `log` crate 的全域 logger（`main()` 最開頭呼叫一次，早於任何可能記錄事件的程式碼，
/// 包含 `--self-test-ipc` 分支）。等級門檻固定 [`LevelFilter::Info`]——記錄的是生命週期事件
/// （故障、復原、資料載入錯誤、置底重排、暫停變化），不是逐行除錯輸出，`Debug`／`Trace`
/// 用不到；安裝失敗（例如同一行程重複呼叫）只印到 stderr、不 panic，記錄器裝不上不該讓宿主
/// 開不起來。
pub fn init() {
    let logger = DailyRotatingLogger::with_local_clock(RETAIN_DAYS, LevelFilter::Info);
    if let Err(err) = log::set_boxed_logger(Box::new(logger)) {
        eprintln!("安裝記錄器失敗：{err}");
        return;
    }
    log::set_max_level(LevelFilter::Info);
}

thread_local! {
    /// 目前執行緒是否正在 [`DailyRotatingLogger::log`] 裡（持有記錄器的鎖）。panic hook 看到
    /// 這個旗標就不回頭寫記錄檔：同一執行緒重複鎖 `std::sync::Mutex` 會死結。
    static IN_LOGGER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// [`IN_LOGGER`] 的 RAII 守衛：離開 `log()`（含 panic 展開）時一定清掉旗標。
struct InLoggerGuard;

impl InLoggerGuard {
    fn enter() -> Self {
        IN_LOGGER.with(|f| f.set(true));
        Self
    }
}

impl Drop for InLoggerGuard {
    fn drop(&mut self) {
        IN_LOGGER.with(|f| f.set(false));
    }
}

/// 安裝 panic hook：把 panic 訊息、位置、執行緒與 backtrace 寫進記錄檔（`ERROR`，target
/// `fc_host::panic`），再交給原本的 hook（預設 hook 印到 stderr）。
///
/// 2026-10-01 soak：宿主在 WebView2 子行程連串當機後無聲消失，記錄檔、WER 都沒有痕跡。宿主
/// 是 GUI 子系統、沒有主控台，主執行緒 panic（含 tao 事件迴圈 `catch_unwind` 接住後在迴圈
/// 外 `resume_unwind` 的那種）只會把訊息印到看不到的 stderr、以結束碼 101 退出——不是
/// 當機，WER 不會留報告。裝上 hook 後，任何 panic 都會在記錄檔留下原因。必須在
/// [`init`] 之後呼叫（hook 經由 `log` facade 寫入）。
pub fn install_panic_hook() {
    install_panic_hook_with(|line| {
        log::error!(target: "fc_host::panic", "{line}");
        log::logger().flush();
    });
}

/// [`install_panic_hook`] 的可測版本：`sink` 收到一整筆 panic 記錄（單元測試注入收集器）。
/// panic 發生在記錄器內部（[`IN_LOGGER`]）時不呼叫 `sink`，只交給原本的 hook。
pub fn install_panic_hook_with<F>(sink: F)
where
    F: Fn(&str) + Send + Sync + 'static,
{
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if !IN_LOGGER.with(|f| f.get()) {
            sink(&panic_record(info));
        }
        previous(info);
    }));
}

/// 一筆 panic 記錄：`panic thread=<名稱> tid=<Win32 執行緒 id> at <檔案:行:欄>：<訊息>`，
/// 下一行是 exe 模組基底位址（[`desktop::exe_module_base`]），再下一行起是 backtrace。執行緒資訊經 [`desktop::current_thread_description`]／
/// [`desktop::current_thread_id`] 取得，不呼叫 `std::thread::current()`——後者在執行緒 TLS
/// 解構期間可能再 panic，hook 內再 panic 會直接 abort、訊息寫不進記錄檔（fix F5）。
fn panic_record(info: &std::panic::PanicHookInfo<'_>) -> String {
    let thread_name =
        desktop::current_thread_description().unwrap_or_else(|| "<未命名>".to_string());
    let tid = desktop::current_thread_id();
    let location = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_else(|| "<未知位置>".to_string());
    let payload = info.payload();
    let message = if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<非字串 payload>".to_string()
    };
    let backtrace = std::backtrace::Backtrace::force_capture();
    let exe_base = desktop::exe_module_base();
    format!(
        "panic thread={thread_name} tid={tid} at {location}：{message}\n\
         exe_base=0x{exe_base:X}（backtrace 位址減此值＝RVA，可對同版 PDB 解析）\n\
         backtrace:\n{backtrace}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::Level;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use std::thread;
    use std::time::Duration;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!(
            "fc-host-logging-test-{}-{}",
            std::process::id(),
            name
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("建立暫存目錄失敗");
        dir
    }

    fn cleanup(dir: &Path) {
        let _ = fs::remove_dir_all(dir);
    }

    /// 直接建一筆 `log::Record` 呼叫 `logger.log()`，繞過 `log` crate 的全域 logger／巨集
    /// （測試不能呼叫 `log::set_boxed_logger`——process 全域只能裝一次，會與其他測試互踩）。
    fn log_record(logger: &DailyRotatingLogger, level: Level, target: &str, msg: &str) {
        logger.log(
            &Record::builder()
                .level(level)
                .target(target)
                .args(format_args!("{msg}"))
                .build(),
        );
    }

    // ── days_from_civil：自我驗證（不依賴記憶中的魔術數字）──────────────────────

    #[test]
    fn days_from_civil_epoch_is_zero() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
    }

    #[test]
    fn days_from_civil_one_day_before_and_after_epoch() {
        assert_eq!(days_from_civil(1970, 1, 2), 1);
        assert_eq!(days_from_civil(1969, 12, 31), -1);
    }

    #[test]
    fn days_from_civil_ordinary_year_has_365_days() {
        // 2025 不是閏年：2025-01-01 → 2026-01-01 應相差 365 天。
        assert_eq!(
            days_from_civil(2026, 1, 1) - days_from_civil(2025, 1, 1),
            365
        );
    }

    #[test]
    fn days_from_civil_leap_year_has_366_days() {
        // 2024 是閏年（可被 4 整除、不可被 100 整除）：2024-01-01 → 2025-01-01 相差 366 天。
        assert_eq!(
            days_from_civil(2025, 1, 1) - days_from_civil(2024, 1, 1),
            366
        );
    }

    #[test]
    fn days_from_civil_feb_29_to_mar_1_is_one_day() {
        assert_eq!(
            days_from_civil(2024, 3, 1) - days_from_civil(2024, 2, 29),
            1
        );
    }

    #[test]
    fn days_from_civil_century_non_leap_year() {
        // 2100 可被 4、100 整除但不可被 400 整除 → 不是閏年，2 月只有 28 天。
        assert_eq!(
            days_from_civil(2100, 3, 1) - days_from_civil(2100, 2, 28),
            1
        );
    }

    #[test]
    fn days_from_civil_400_year_span_matches_known_day_count() {
        // 2000-03-01（已知：儒略日 2451605，2000 是能被 400 整除的閏年例外）到
        // 2400-03-01 恰好相差 400 個公曆年；累計閏年數 = 97（400 年週期內的公曆閏年數，
        // 這是格里曆定義本身：每 400 年 97 個閏年，此處只驗證我們的實作是否吻合這個定義）。
        let span = days_from_civil(2400, 3, 1) - days_from_civil(2000, 3, 1);
        assert_eq!(span, 400 * 365 + 97);
    }

    // ── log_file_name／parse_log_file_date：格式往返 ────────────────────────────

    #[test]
    fn log_file_name_formats_zero_padded() {
        assert_eq!(log_file_name((2026, 1, 5)), "fc-host.2026-01-05.log");
        assert_eq!(log_file_name((2026, 12, 31)), "fc-host.2026-12-31.log");
    }

    #[test]
    fn parse_log_file_date_round_trips_with_log_file_name() {
        let date = (2026, 9, 28);
        assert_eq!(parse_log_file_date(&log_file_name(date)), Some(date));
    }

    #[test]
    fn parse_log_file_date_rejects_unrelated_names() {
        assert_eq!(parse_log_file_date("gatekeeper.log"), None);
        assert_eq!(parse_log_file_date("fc-host.log"), None);
        assert_eq!(parse_log_file_date("fc-host.2026-09-28.log.bak"), None);
        assert_eq!(parse_log_file_date("fc-host.2026-09-28-extra.log"), None);
        assert_eq!(parse_log_file_date("fc-host.abcd-09-28.log"), None);
    }

    // ── is_expired：保留 7 天的邊界 ──────────────────────────────────────────

    #[test]
    fn is_expired_keeps_today_and_six_previous_days() {
        let today = (2026, 9, 28);
        // 偏移量 0..6（共 7 天，含今天）應保留。`shift_days` 正數＝往過去推（見其文件）。
        for offset in 0..7 {
            let file_date = shift_days(today, offset);
            assert!(
                !is_expired(file_date, today, 7),
                "偏移 {offset} 天應保留：{file_date:?}"
            );
        }
        // 偏移量 7（第 8 天前）應過期。
        let expired = shift_days(today, 7);
        assert!(
            is_expired(expired, today, 7),
            "偏移 7 天應過期：{expired:?}"
        );
    }

    #[test]
    fn is_expired_never_deletes_future_dated_files() {
        let today = (2026, 9, 28);
        let future = (2026, 9, 29);
        assert!(!is_expired(future, today, 7), "未來日期不該被視為過期");
    }

    #[test]
    fn is_expired_boundary_crosses_month() {
        // 保留 7 天，今天是 2026-10-03：最舊保留到 2026-09-27（偏移 6），2026-09-26（偏移 7）
        // 過期——驗證跨月邊界（9 月只有 30 天）算得對。
        let today = (2026, 10, 3);
        assert!(!is_expired((2026, 9, 27), today, 7));
        assert!(is_expired((2026, 9, 26), today, 7));
    }

    /// 測試專用：把 `(y, m, d)` 往前推 `days` 天（`days` 為負代表未來）。透過
    /// `days_from_civil` 算出目標的「天數座標」後，用簡單迴圈換算回民用曆——僅供測試使用
    /// （本模組正式邏輯不需要反向換算，見模組文件），不追求效率。
    fn shift_days(date: Ymd, days: i64) -> Ymd {
        let target = days_from_civil(date.0, date.1, date.2) - days;
        // 從已知基準往目標方向逐日走訪；測試用值域小（數十天內），效能無虞。
        let mut probe = date;
        let mut probe_days = days_from_civil(probe.0, probe.1, probe.2);
        while probe_days != target {
            if probe_days < target {
                probe = next_day(probe);
                probe_days += 1;
            } else {
                probe = prev_day(probe);
                probe_days -= 1;
            }
        }
        probe
    }

    fn days_in_month(y: i32, m: u32) -> u32 {
        match m {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 => {
                if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 {
                    29
                } else {
                    28
                }
            }
            _ => unreachable!("月份必為 1..=12"),
        }
    }

    fn next_day((y, m, d): Ymd) -> Ymd {
        if d < days_in_month(y, m) {
            (y, m, d + 1)
        } else if m < 12 {
            (y, m + 1, 1)
        } else {
            (y + 1, 1, 1)
        }
    }

    fn prev_day((y, m, d): Ymd) -> Ymd {
        if d > 1 {
            (y, m, d - 1)
        } else if m > 1 {
            (y, m - 1, days_in_month(y, m - 1))
        } else {
            (y - 1, 12, 31)
        }
    }

    // ── prune_old_logs：真的用檔案系統驗證刪舊留新 ──────────────────────────────

    #[test]
    fn prune_old_logs_deletes_expired_and_keeps_recent_and_unrelated() {
        let dir = temp_dir("prune");
        let today = (2026, 9, 28);

        // 保留範圍內（今天、6 天前）。
        fs::write(dir.join(log_file_name(today)), "today").unwrap();
        fs::write(dir.join(log_file_name((2026, 9, 22))), "6-days-ago").unwrap();
        // 超過保留範圍（7 天前）。
        fs::write(dir.join(log_file_name((2026, 9, 21))), "7-days-ago").unwrap();
        fs::write(dir.join(log_file_name((2026, 1, 1))), "old").unwrap();
        // 不是本記錄檔命名格式的檔案：不該被動到。
        fs::write(dir.join("gatekeeper.log"), "unrelated").unwrap();

        prune_old_logs(&dir, today, 7);

        let remaining: std::collections::BTreeSet<String> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            remaining,
            std::collections::BTreeSet::from([
                log_file_name(today),
                log_file_name((2026, 9, 22)),
                "gatekeeper.log".to_string(),
            ])
        );

        cleanup(&dir);
    }

    #[test]
    fn prune_old_logs_missing_dir_does_not_panic() {
        let dir = temp_dir("missing").join("does-not-exist");
        prune_old_logs(&dir, (2026, 9, 28), 7); // 不 panic 即通過
    }

    // ── DailyRotatingLogger：端對端（注入時間，驗證換檔與清舊檔）──────────────────

    #[test]
    fn logger_writes_into_file_named_for_injected_date() {
        let dir = temp_dir("logger-basic");
        let logger = DailyRotatingLogger::new(
            dir.clone(),
            7,
            LevelFilter::Info,
            Box::new(|| (2026, 9, 28)),
        );

        log_record(&logger, Level::Info, "logging::tests", "第一行");

        let path = dir.join(log_file_name((2026, 9, 28)));
        assert!(path.exists(), "應該建立當天的記錄檔：{path:?}");
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("第一行"), "內容：{content}");
        assert!(content.contains("[ INFO]"), "應含等級標記，內容：{content}");
        assert!(
            content.contains("logging::tests"),
            "應含 target，內容：{content}"
        );

        cleanup(&dir);
    }

    #[test]
    fn logger_below_level_filter_is_dropped() {
        let dir = temp_dir("logger-filtered");
        let logger = DailyRotatingLogger::new(
            dir.clone(),
            7,
            LevelFilter::Info,
            Box::new(|| (2026, 9, 28)),
        );

        log_record(&logger, Level::Debug, "logging::tests", "不該出現");

        let path = dir.join(log_file_name((2026, 9, 28)));
        // Debug 低於 Info 門檻：`enabled` 應擋下，連檔案都不該建立。
        assert!(!path.exists(), "Debug 等級不該被寫入或建檔");

        cleanup(&dir);
    }

    #[test]
    fn logger_rotates_to_new_file_when_injected_date_advances_and_prunes_old() {
        let dir = temp_dir("logger-rotate");
        // 用可變格子模擬「今天」隨呼叫推進：第一次呼叫回傳第 0 天，之後每次呼叫回傳最新設定值。
        let day = std::sync::Arc::new(Mutex::new((2026, 1, 1)));
        let day_for_closure = day.clone();
        let logger = DailyRotatingLogger::new(
            dir.clone(),
            7,
            LevelFilter::Info,
            Box::new(move || *day_for_closure.lock().unwrap()),
        );

        log_record(&logger, Level::Info, "t", "第一天");
        assert!(dir.join(log_file_name((2026, 1, 1))).exists());

        // 推進到保留範圍邊界之外（第 8 天，1/1 應被清掉）。
        *day.lock().unwrap() = (2026, 1, 8);
        log_record(&logger, Level::Info, "t", "第八天");

        assert!(
            dir.join(log_file_name((2026, 1, 8))).exists(),
            "應該換到新的一天的檔案"
        );
        assert!(
            !dir.join(log_file_name((2026, 1, 1))).exists(),
            "超過保留天數的舊檔應在換檔時被清掉"
        );

        cleanup(&dir);
    }

    #[test]
    fn logger_appends_within_the_same_injected_day() {
        let dir = temp_dir("logger-append");
        let logger = DailyRotatingLogger::new(
            dir.clone(),
            7,
            LevelFilter::Info,
            Box::new(|| (2026, 9, 28)),
        );

        log_record(&logger, Level::Info, "t", "第一行");
        log_record(&logger, Level::Warn, "t", "第二行");

        let content = fs::read_to_string(dir.join(log_file_name((2026, 9, 28)))).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2, "同一天應累加、不覆寫：{content}");
        assert!(lines[0].contains("第一行"));
        assert!(lines[1].contains("第二行"));

        cleanup(&dir);
    }

    #[test]
    fn default_log_dir_points_under_localappdata_namespace() {
        let path = default_log_dir();
        let normalized = path.to_string_lossy().replace('\\', "/");
        assert!(
            normalized.ends_with("tw.fintools.fc-host/logs"),
            "實際路徑：{normalized}"
        );
    }

    /// mtime 解析度在某些檔案系統下有限；避免與其他測試的暫存檔案操作前後腳踩到彼此
    /// （目前各測試各自用獨立子目錄，理論上不需要，保留這個小工具供未來若有共用目錄的測試用）。
    #[allow(dead_code)]
    fn settle() {
        thread::sleep(Duration::from_millis(20));
    }

    // ── 多執行緒／多 File handle 同寫：一行不得被交錯切開 ─────────────────────────
    //
    // task 5.1 壓測（6 個行程同時啟動）觀察到：多個行程各自對同一份記錄檔 append，
    // `writeln!` 對底層 `File` 分段寫入（格式化字串的每個字面／參數片段各自一次
    // `write_all` syscall），不同行程的片段可能交錯，把「角色=Primary」字串切開
    // （見 `.superpowers/sdd/tasks/task-5.1-report.md` 第 245–256 行）。
    //
    // 這裡不能共用同一個 `DailyRotatingLogger` 實例——它内部的 `Mutex<Inner>` 會把同
    // 一顆 `File` handle 的寫入序列化，測不出交錯。要重現「多行程」情境，改為每條執行緒
    // 各自建立一個獨立的 `DailyRotatingLogger`（各自獨立 `OpenOptions::append` 出來的
    // `File` handle），全部指向同一份記錄檔、同一個注入日期，模擬多行程各自 append。

    /// 驗證一行是否為某個 `(thread, seq)` 完整、未被截斷／未被拼接的記錄；抓不到就回傳
    /// `None`（呼叫端視為「無法辨識的損毀行」）。
    fn parse_full_marker_line(line: &str, padding: &str) -> Option<(usize, usize)> {
        const PREFIX: &str = "角色=Primary thread=";
        // 一行只能出現一次「thread=」——出現兩次代表兩筆記錄的片段被接在同一行裡。
        if line.matches("thread=").count() != 1 {
            return None;
        }
        let marker_pos = line.find(PREFIX)?;
        let rest = &line[marker_pos + PREFIX.len()..];
        let mut parts = rest.splitn(2, " seq=");
        let t: usize = parts.next()?.parse().ok()?;
        let seq_rest = parts.next()?;
        let mut seq_parts = seq_rest.splitn(2, ' ');
        let i: usize = seq_parts.next()?.parse().ok()?;
        let tail = seq_parts.next()?;
        // 整行必須恰好以完整填充字串結尾——被切斷或被其他行的片段接續都會在這裡失敗。
        if tail != padding {
            return None;
        }
        Some((t, i))
    }

    #[test]
    fn concurrent_writers_do_not_interleave_within_a_line() {
        let dir = temp_dir("logger-concurrent");
        let today = (2026, 9, 28);
        let thread_count: usize = 8;
        let messages_per_thread: usize = 200;
        const PADDING: &str = "填充填充填充填充填充填充填充填充填充填充填充填充填充填充填充填充";

        let handles: Vec<_> = (0..thread_count)
            .map(|t| {
                let dir = dir.clone();
                thread::spawn(move || {
                    let logger = DailyRotatingLogger::new(
                        dir,
                        7,
                        LevelFilter::Info,
                        Box::new(move || today),
                    );
                    for i in 0..messages_per_thread {
                        let msg = format!("角色=Primary thread={t:02} seq={i:04} {PADDING}");
                        log_record(&logger, Level::Info, "t", &msg);
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().expect("執行緒 panic");
        }

        let path = dir.join(log_file_name(today));
        let content = fs::read_to_string(&path).expect("讀取記錄檔失敗");

        let mut seen = std::collections::HashSet::new();
        let mut corrupted: Vec<String> = Vec::new();
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            match parse_full_marker_line(line, PADDING) {
                Some(key) => {
                    seen.insert(key);
                }
                None => corrupted.push(line.to_string()),
            }
        }

        assert!(
            corrupted.is_empty(),
            "偵測到 {} 行交錯／截斷的記錄，例如：{:#?}",
            corrupted.len(),
            &corrupted[..corrupted.len().min(3)]
        );
        assert_eq!(
            seen.len(),
            thread_count * messages_per_thread,
            "應有 {} 行完整可解析的記錄，實際 {} 行（可能有記錄互相蓋掉或合併成同一行）",
            thread_count * messages_per_thread,
            seen.len()
        );

        cleanup(&dir);
    }

    // ── panic hook（soak 事件：宿主無聲消失，panic 訊息只到 stderr）──────────────────

    /// panic hook 是行程全域狀態：會裝 hook 的測試彼此序列化，結束時還原成預設 hook。
    static PANIC_HOOK_TEST_LOCK: Mutex<()> = Mutex::new(());

    type Lines = std::sync::Arc<Mutex<Vec<String>>>;

    fn with_capturing_hook(body: impl FnOnce(&Lines)) {
        let _serial = PANIC_HOOK_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let lines: Lines = Default::default();
        let sink_lines = lines.clone();
        install_panic_hook_with(move |line| {
            sink_lines
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(line.to_string());
        });
        body(&lines);
        // 換回預設 hook（丟掉本測試裝的那一個）。
        let _ = std::panic::take_hook();
    }

    #[test]
    fn panic_hook_records_message_location_and_thread() {
        with_capturing_hook(|lines| {
            let (tid_tx, tid_rx) = std::sync::mpsc::channel();
            let joined = thread::Builder::new()
                .name("soak-panic-test".into())
                .spawn(move || {
                    let _ = tid_tx.send(desktop::current_thread_id());
                    panic!("boom {}", 42)
                })
                .expect("建立執行緒失敗")
                .join();
            assert!(joined.is_err(), "執行緒應該 panic");
            let tid = tid_rx.recv().expect("沒收到執行緒 id");
            // 先複製再放鎖：斷言失敗的 panic 會再經 hook 寫入同一個收集器，持鎖斷言會死結。
            let lines = lines.lock().unwrap_or_else(|p| p.into_inner()).clone();
            let line = lines
                .iter()
                .find(|l| l.contains("boom 42"))
                .unwrap_or_else(|| panic!("sink 沒收到 panic 訊息：{lines:?}"));
            assert!(line.contains("thread=soak-panic-test"), "{line}");
            assert!(line.contains(&format!("tid={tid} ")), "{line}");
            let base = desktop::exe_module_base();
            assert!(line.contains(&format!("exe_base=0x{base:X}")), "{line}");
            assert!(line.contains("logging.rs:"), "{line}");
        });
    }

    #[test]
    fn panic_record_handles_non_string_payload() {
        with_capturing_hook(|lines| {
            let joined = thread::spawn(|| std::panic::panic_any(7u32)).join();
            assert!(joined.is_err());
            // 先複製再放鎖：斷言失敗的 panic 會再經 hook 寫入同一個收集器，持鎖斷言會死結。
            let lines = lines.lock().unwrap_or_else(|p| p.into_inner()).clone();
            assert!(
                lines.iter().any(|l| l.contains("非字串")),
                "非字串 payload 也要留下一行：{lines:?}"
            );
        });
    }

    /// 在背景執行緒以 `catch_unwind` 執行 `body`，回傳「是否 panic」；10 秒內沒結束視為死結。
    fn run_catching_on_thread(body: impl FnOnce() + Send + 'static) -> bool {
        let (tx, rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
            let _ = tx.send(result.is_err());
        });
        rx.recv_timeout(Duration::from_secs(10))
            .expect("記錄器內 panic 後執行緒卡住（hook 重入記錄器造成死結？）")
    }

    /// fix F5（review fix-soak medium）：記錄參數的 `Display` 實作 panic 時，格式化在鎖外進行
    /// ——鎖不會被毒化，之後的記錄照常寫得進檔案；panic 本身也照常經 hook 留下記錄。
    #[test]
    fn panic_while_formatting_args_does_not_stop_later_records() {
        struct Explodes;
        impl std::fmt::Display for Explodes {
            fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                panic!("display exploded")
            }
        }
        let dir = temp_dir("panic-format");
        let logger = std::sync::Arc::new(DailyRotatingLogger::new(
            dir.clone(),
            RETAIN_DAYS,
            LevelFilter::Info,
            Box::new(|| (2026, 10, 1)),
        ));
        with_capturing_hook(|lines| {
            let worker_logger = logger.clone();
            let panicked = run_catching_on_thread(move || {
                worker_logger.log(
                    &Record::builder()
                        .level(Level::Error)
                        .target("t")
                        .args(format_args!("{}", Explodes))
                        .build(),
                );
            });
            assert!(panicked);
            // 先複製再放鎖：斷言失敗的 panic 會再經 hook 寫入同一個收集器，持鎖斷言會死結。
            let lines = lines.lock().unwrap_or_else(|p| p.into_inner()).clone();
            assert!(
                lines.iter().any(|l| l.contains("display exploded")),
                "鎖外的格式化 panic 應照常經 sink 留下記錄：{lines:?}"
            );
        });
        log_record(&logger, Level::Info, "t", "之後這行要寫得進去");
        let content =
            fs::read_to_string(dir.join(log_file_name((2026, 10, 1)))).unwrap_or_default();
        assert!(
            content.contains("之後這行要寫得進去"),
            "格式化 panic 之後記錄器不可失效，內容：{content}"
        );
        drop(logger);
        cleanup(&dir);
    }

    /// 記錄器自己在持有鎖時 panic（這裡用注入的 `now` 第一次呼叫就 panic 模擬），hook 不可再
    /// 回頭寫記錄檔——同一執行緒重複鎖 `std::sync::Mutex` 會死結，宿主會從「消失」變成「卡死」。
    /// 鎖因此被毒化後，之後的記錄仍要寫得進去（`PoisonError::into_inner`）。
    #[test]
    fn panic_inside_logger_lock_does_not_reenter_and_logger_recovers() {
        let dir = temp_dir("panic-reentry");
        let calls = std::sync::Arc::new(AtomicUsize::new(0));
        let calls_for_now = calls.clone();
        let logger = std::sync::Arc::new(DailyRotatingLogger::new(
            dir.clone(),
            RETAIN_DAYS,
            LevelFilter::Info,
            Box::new(move || {
                if calls_for_now.fetch_add(1, AtomicOrdering::SeqCst) == 0 {
                    panic!("clock exploded");
                }
                (2026, 10, 1)
            }),
        ));
        with_capturing_hook(|lines| {
            let worker_logger = logger.clone();
            let panicked = run_catching_on_thread(move || {
                log_record(&worker_logger, Level::Error, "t", "第一行");
            });
            assert!(panicked);
            let lines = lines.lock().unwrap_or_else(|p| p.into_inner()).clone();
            assert!(
                lines.iter().all(|l| !l.contains("clock exploded")),
                "持鎖期間的 panic 不應再經 sink 寫記錄：{lines:?}"
            );
        });
        log_record(&logger, Level::Info, "t", "毒化之後這行要寫得進去");
        logger.flush();
        let content =
            fs::read_to_string(dir.join(log_file_name((2026, 10, 1)))).unwrap_or_default();
        assert!(
            content.contains("毒化之後這行要寫得進去"),
            "鎖被毒化後記錄器不可失效，內容：{content}"
        );
        drop(logger);
        cleanup(&dir);
    }
}
