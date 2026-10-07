//! 更新檢查的啟動模式與排程計畫（design.md D3）。純函式，隨機性與時間由呼叫端注入。
//!
//! | 啟動 | 第一次檢查 | 之後 |
//! |---|---|---|
//! | 一般啟動 | 60–180 秒隨機延遲後 | 每 6 小時 |
//! | `--autostart`（登入） | 網路就緒後立即（等網路上限沿用 `fetch::net` 預設 300 秒） | 每 6 小時 |
//! | 讀到舊標記（上次當機） | 網路就緒後立即，等網路上限 60 秒（與下載合計的 60 秒逾時由過渡期看門狗負責） | 每 6 小時 |
//! | `--fetch-once`／`--restore-wallpaper` | **不檢查**（這兩個命令在 `main()` 仲裁之前就結束，不會走到更新器；本檔以型別再表達一次，方便測試） | — |
//!
//! ## 兩種時間
//!
//! 可取消的睡眠（[`super::cancel::CancelToken::wait`]）是相對逾時，依保守假設**不計入系統睡眠時間**：
//!
//! - Microsoft Learn 的 `WaitForSingleObject`（<https://learn.microsoft.com/en-us/windows/win32/api/synchapi/nf-synchapi-waitforsingleobject>）
//!   明寫 Windows 8 以後 `dwMilliseconds` 不計入低電源狀態的時間。
//! - 標準庫在 Windows（非 win7 target）以 `WaitOnAddress` 實作 `Condvar`：rust-lang/rust 1.97.1 的
//!   `library/std/src/sys/sync/condvar/mod.rs`（Windows 選 `futex`）、`library/std/src/sys/pal/windows/futex.rs`
//!   （呼叫 `WaitOnAddress`）。`WaitOnAddress` 的文件
//!   （<https://learn.microsoft.com/en-us/windows/win32/api/synchapi/nf-synchapi-waitonaddress>）沒有寫睡眠算不算，
//!   故依保守假設視為與 `WaitForSingleObject` 相同。
//! - `Condvar::wait_timeout_while`（同版本 `library/std/src/sync/poison/condvar.rs`）只在 OS 等待返回後才以
//!   `Instant` 重算剩餘時間；`Instant` 在 Windows 是 `QueryPerformanceCounter`，含睡眠時間
//!   （<https://learn.microsoft.com/en-us/windows/win32/sysinfo/acquiring-high-resolution-time-stamps>），但不會讓
//!   等待提早返回。
//!
//! 所以：
//!
//! - **6 小時檢查週期以牆上時鐘計（含睡眠）**：每 [`CHECK_POLL_INTERVAL`]（清醒時間）醒來一次，以
//!   [`check_due`] 比對 `SystemTime` 距上次檢查是否已滿 [`CHECK_INTERVAL`]。否則常闔蓋、很少重新登入的筆電，
//!   「每 6 小時」會拉長成好幾天。
//! - **啟動首檢延遲（60–180 秒）、過渡期逾時（60 秒）、標記穩定時間（5 分鐘）以清醒時間計**：它們量的是
//!   「這個行程實際跑了多久」，睡眠期間沒有任何進展，不該被算進去。

use std::time::{Duration, SystemTime};

/// 一般啟動的第一次檢查延遲下限（含）。
pub const FIRST_CHECK_DELAY_MIN: Duration = Duration::from_secs(60);
/// 一般啟動的第一次檢查延遲上限（含）。
pub const FIRST_CHECK_DELAY_MAX: Duration = Duration::from_secs(180);
/// 之後的檢查週期（以牆上時鐘計、含睡眠，見模組文件「兩種時間」）。
pub const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
/// 週期檢查之間的醒來間隔（清醒時間）：每次醒來以 [`check_due`] 判斷是否已滿 [`CHECK_INTERVAL`]。
/// 取消（[`super::cancel::CancelToken`]）會立即喚醒，不受它影響。
pub const CHECK_POLL_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// 檢查清單（`latest.json`）請求的逾時。
pub const CHECK_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// 下載安裝檔的逾時（design.md D3：10 分鐘）。注意 `UpdaterBuilder::timeout` 只作用於清單請求；
/// 下載走 `Update::timeout`（外掛 `check()` 產生的 `Update` 預設為 `None`），見 `plugin_backend`。
pub const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// 讀到舊標記時，等網路與檢查／下載合計的上限（過渡期看門狗）。
pub const TRANSITION_TIMEOUT: Duration = Duration::from_secs(60);

/// 本行程的啟動方式（只分更新器關心的幾類）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartMode {
    /// 使用者手動啟動、系統重新啟動（`--restarted`）等。
    Normal,
    /// `--autostart`：開機自啟（登入）或更新後重新啟動。
    Autostart,
    /// `--fetch-once <目錄>`：單次抓取，不檢查更新。
    FetchOnce,
    /// `--restore-wallpaper`：安裝檔呼叫，不檢查更新。
    RestoreWallpaper,
}

impl StartMode {
    /// 由命令列參數（含第一個元素的執行檔路徑亦可）判定。`--fetch-once`／`--restore-wallpaper`
    /// 優先於 `--autostart`。
    pub fn from_args<S: AsRef<str>>(args: &[S]) -> Self {
        let has = |flag: &str| args.iter().any(|a| a.as_ref() == flag);
        if has("--restore-wallpaper") {
            Self::RestoreWallpaper
        } else if has("--fetch-once") {
            Self::FetchOnce
        } else if has("--autostart") {
            Self::Autostart
        } else {
            Self::Normal
        }
    }

    /// 這種啟動方式要不要檢查更新。
    pub fn checks_for_updates(self) -> bool {
        matches!(self, Self::Normal | Self::Autostart)
    }
}

/// 第一次檢查怎麼安排。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirstCheck {
    /// 先睡這麼久再檢查（一般啟動）。
    AfterDelay(Duration),
    /// 等網路就緒（最多 `max_wait`）後立即檢查。
    AfterNetwork { max_wait: Duration },
}

/// 第一次檢查的計畫。`stale_marker`＝讀到舊標記（當機後的下一次啟動，優先於啟動方式）；
/// `entropy` 是呼叫端給的隨機數（只用來在 60–180 秒內取值）；`default_network_wait` 是
/// `fetch::net::MAX_WAIT`（`--autostart` 沿用它的預設）。
///
/// 回傳 `None`＝這種啟動方式不檢查更新。
pub fn plan_first_check(
    mode: StartMode,
    stale_marker: bool,
    entropy: u64,
    default_network_wait: Duration,
) -> Option<FirstCheck> {
    if !mode.checks_for_updates() {
        return None;
    }
    if stale_marker {
        return Some(FirstCheck::AfterNetwork {
            max_wait: TRANSITION_TIMEOUT,
        });
    }
    Some(match mode {
        StartMode::Autostart => FirstCheck::AfterNetwork {
            max_wait: default_network_wait,
        },
        _ => FirstCheck::AfterDelay(first_check_delay(entropy)),
    })
}

/// 週期檢查是否到期（[`check_due`] 的結果）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckDue {
    /// 距上次檢查已滿週期（含睡眠中經過的時間、時鐘大幅前跳）：立即檢查。
    Due,
    /// 還沒到。
    NotYet,
    /// 牆上時鐘比上次檢查還早（使用者調時間、時間同步往回校正）：呼叫端把「上次檢查」重設為現在，
    /// 從現在起重新計算一個週期——不因倒退而永不檢查，也不因此立即檢查。
    ClockWentBack,
}

/// 以牆上時鐘判斷週期檢查是否到期。`last_check`＝上次檢查（結束）時的 `SystemTime`，`now`＝現在。
pub fn check_due(last_check: SystemTime, now: SystemTime, interval: Duration) -> CheckDue {
    match now.duration_since(last_check) {
        Ok(elapsed) if elapsed >= interval => CheckDue::Due,
        Ok(_) => CheckDue::NotYet,
        Err(_) => CheckDue::ClockWentBack,
    }
}

/// 60–180 秒（含兩端）之間以 `entropy` 取值。
pub fn first_check_delay(entropy: u64) -> Duration {
    let span = FIRST_CHECK_DELAY_MAX.as_secs() - FIRST_CHECK_DELAY_MIN.as_secs() + 1;
    FIRST_CHECK_DELAY_MIN + Duration::from_secs(entropy % span)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NET_WAIT: Duration = Duration::from_secs(300);

    #[test]
    fn mode_from_args() {
        assert_eq!(StartMode::from_args(&["fc-host.exe"]), StartMode::Normal);
        assert_eq!(
            StartMode::from_args(&["fc-host.exe", "--restarted"]),
            StartMode::Normal
        );
        assert_eq!(
            StartMode::from_args(&["fc-host.exe", "--autostart"]),
            StartMode::Autostart
        );
        assert_eq!(
            StartMode::from_args(&["fc-host.exe", "--fetch-once", "D:\\out"]),
            StartMode::FetchOnce
        );
        assert_eq!(
            StartMode::from_args(&["fc-host.exe", "--restore-wallpaper"]),
            StartMode::RestoreWallpaper
        );
        assert_eq!(
            StartMode::from_args(&["fc-host.exe", "--autostart", "--restore-wallpaper"]),
            StartMode::RestoreWallpaper,
            "命令列工具模式優先"
        );
    }

    #[test]
    fn fetch_once_and_restore_wallpaper_never_check() {
        // spec「自動檢查更新」：--fetch-once／--restore-wallpaper 啟動的行程不得檢查更新，
        // 即使（假設）帶著舊標記也一樣。
        for mode in [StartMode::FetchOnce, StartMode::RestoreWallpaper] {
            assert!(!mode.checks_for_updates());
            assert_eq!(plan_first_check(mode, false, 7, NET_WAIT), None);
            assert_eq!(plan_first_check(mode, true, 7, NET_WAIT), None);
        }
    }

    #[test]
    fn normal_start_waits_random_60_to_180_seconds() {
        for entropy in [0, 1, 59, 60, 120, 121, 122, u64::MAX] {
            let plan = plan_first_check(StartMode::Normal, false, entropy, NET_WAIT);
            let Some(FirstCheck::AfterDelay(d)) = plan else {
                panic!("一般啟動應是延遲檢查：{plan:?}");
            };
            assert!(
                d >= Duration::from_secs(60) && d <= Duration::from_secs(180),
                "entropy={entropy} 延遲 {d:?} 超出 60–180 秒"
            );
        }
        assert_eq!(first_check_delay(0), Duration::from_secs(60));
        assert_eq!(first_check_delay(120), Duration::from_secs(180));
        assert_eq!(
            first_check_delay(121),
            Duration::from_secs(60),
            "取模回到下限"
        );
    }

    #[test]
    fn autostart_checks_immediately_after_network_with_default_wait() {
        assert_eq!(
            plan_first_check(StartMode::Autostart, false, 5, NET_WAIT),
            Some(FirstCheck::AfterNetwork { max_wait: NET_WAIT }),
            "--autostart：網路就緒後立即檢查，不套用 60–180 秒延遲"
        );
    }

    #[test]
    fn stale_marker_checks_immediately_with_sixty_second_cap() {
        for mode in [StartMode::Normal, StartMode::Autostart] {
            assert_eq!(
                plan_first_check(mode, true, 5, NET_WAIT),
                Some(FirstCheck::AfterNetwork {
                    max_wait: Duration::from_secs(60)
                }),
                "{mode:?}：舊標記優先，立即檢查、等網路上限 60 秒"
            );
        }
    }

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    const T0: u64 = 1_800_000_000;
    const HOUR: u64 = 3600;

    #[test]
    fn check_due_normal_interval() {
        let last = at(T0);
        assert_eq!(check_due(last, last, CHECK_INTERVAL), CheckDue::NotYet);
        assert_eq!(
            check_due(last, at(T0 + 6 * HOUR - 1), CHECK_INTERVAL),
            CheckDue::NotYet,
            "差一秒未滿 6 小時"
        );
        assert_eq!(
            check_due(last, at(T0 + 6 * HOUR), CHECK_INTERVAL),
            CheckDue::Due,
            "剛好 6 小時到期"
        );
        assert_eq!(
            check_due(last, at(T0 + 6 * HOUR + 300), CHECK_INTERVAL),
            CheckDue::Due
        );
    }

    #[test]
    fn check_due_right_after_waking_from_two_days_of_sleep() {
        // 睡眠期間清醒時間不前進，但牆上時鐘照走：醒來後第一次比對就到期。
        assert_eq!(
            check_due(at(T0), at(T0 + 48 * HOUR + 5 * 60), CHECK_INTERVAL),
            CheckDue::Due
        );
    }

    #[test]
    fn check_due_clock_going_back_restarts_instead_of_never_or_always() {
        assert_eq!(
            check_due(at(T0), at(T0 - 1), CHECK_INTERVAL),
            CheckDue::ClockWentBack,
            "倒退一秒也算倒退（呼叫端重設起點）"
        );
        assert_eq!(
            check_due(at(T0), at(T0 - 30 * 24 * HOUR), CHECK_INTERVAL),
            CheckDue::ClockWentBack,
            "大幅倒退：不得當成已到期（否則會連續狂檢查）"
        );
        // 呼叫端重設起點後，從新的「現在」起重新算 6 小時。
        let reset = at(T0 - 30 * 24 * HOUR);
        assert_eq!(
            check_due(reset, at(T0 - 30 * 24 * HOUR + HOUR), CHECK_INTERVAL),
            CheckDue::NotYet
        );
        assert_eq!(
            check_due(reset, at(T0 - 30 * 24 * HOUR + 6 * HOUR), CHECK_INTERVAL),
            CheckDue::Due,
            "倒退後不會永不檢查"
        );
    }

    #[test]
    fn check_due_large_forward_jump_is_due_once() {
        // 時鐘大幅前跳：到期一次；檢查後起點改為新的「現在」，下一次照常 6 小時後。
        let jumped = at(T0 + 365 * 24 * HOUR);
        assert_eq!(check_due(at(T0), jumped, CHECK_INTERVAL), CheckDue::Due);
        assert_eq!(
            check_due(jumped, at(T0 + 365 * 24 * HOUR + 300), CHECK_INTERVAL),
            CheckDue::NotYet,
            "前跳後檢查過一次，不會連續到期"
        );
    }

    #[test]
    fn periodic_constants_match_design() {
        assert_eq!(CHECK_INTERVAL, Duration::from_secs(21_600));
        assert_eq!(CHECK_POLL_INTERVAL, Duration::from_secs(300));
        assert_eq!(DOWNLOAD_TIMEOUT, Duration::from_secs(600));
        assert_eq!(TRANSITION_TIMEOUT, Duration::from_secs(60));
    }
}
