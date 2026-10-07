//! 啟動時等網路（behavior-inventory B-FLOW-2；Python `wait_for_network`，`update_tw_events.py:305-323`）。
//!
//! 開機排程補跑時網路常常還沒就緒：以**原始 TCP 連線**探測 `www.twse.com.tw:443`（刻意不走代理、不吃
//! 代理環境變數，只測底層網路本身；`std::net` 本來就不讀代理設定），每 `interval` 重試，累計睡滿
//! `max_wait` 仍失敗就放行（回傳 `false`，由呼叫端把 `net_timed_out` 傳給 [`super::round::run_round`]，
//! 後續靠各來源的「沿用舊資料」兜底），不讓流程卡死在等待迴圈。
//!
//! 與 Python 相同的計時語意：`waited` 只累計睡眠時間、不含各次連線逾時，所以預設值下最壞約 16 次探測、
//! 牆鐘超過 300 秒。`--fetch-once`（5.1）與排程（5.2）共用本函式。
//!
//! 這是**阻塞**函式（探測與睡眠都是同步呼叫），呼叫端要在非 async 的執行緒上呼叫。

use std::io;
use std::net::{TcpStream, ToSocketAddrs};
use std::thread;
use std::time::Duration;

use super::report::LOG_TARGET;

/// 探測目標主機（與 Python 預設值相同）。
pub const PROBE_HOST: &str = "www.twse.com.tw";
/// 探測目標連接埠。
pub const PROBE_PORT: u16 = 443;
/// 單次連線逾時（B-FLOW-2 預設 5 秒）。
pub const TIMEOUT_EACH: Duration = Duration::from_secs(5);
/// 兩次探測之間的睡眠（預設 20 秒）。
pub const INTERVAL: Duration = Duration::from_secs(20);
/// 累計睡眠上限（預設 300 秒）。
pub const MAX_WAIT: Duration = Duration::from_secs(300);

/// 以原始 TCP 連線探測 [`PROBE_HOST`]:[`PROBE_PORT`]，回傳網路是否在 `max_wait` 內就緒。
///
/// `interval` 為零時不重試（避免無限空轉）：第一次失敗就回 `false`。
pub fn wait_for_network(timeout_each: Duration, interval: Duration, max_wait: Duration) -> bool {
    wait_for_network_with(
        || tcp_probe(timeout_each),
        thread::sleep,
        interval,
        max_wait,
    )
}

/// [`wait_for_network`] 的可注入版本：`connect` 回 `Ok` 就視為網路就緒，`sleep` 負責睡眠。
/// 單元測試以假連線與假睡眠驗證流程，不真的連線也不真的等。
pub fn wait_for_network_with<C, S>(
    mut connect: C,
    mut sleep: S,
    interval: Duration,
    max_wait: Duration,
) -> bool
where
    C: FnMut() -> io::Result<()>,
    S: FnMut(Duration),
{
    let mut waited = Duration::ZERO;
    loop {
        match connect() {
            Ok(()) => {
                if !waited.is_zero() {
                    log::info!(
                        target: LOG_TARGET,
                        "[網路] 已就緒（等待 {} 秒）",
                        waited.as_secs()
                    );
                }
                return true;
            }
            Err(e) => {
                if waited >= max_wait || interval.is_zero() {
                    log::warn!(
                        target: LOG_TARGET,
                        "[網路] 等待逾時（{} 秒）仍未就緒，改用既有資料兜底：{e}",
                        max_wait.as_secs()
                    );
                    return false;
                }
                log::info!(
                    target: LOG_TARGET,
                    "[網路] 尚未就緒（{e}），{} 秒後重試…",
                    interval.as_secs()
                );
                sleep(interval);
                waited += interval;
            }
        }
    }
}

/// 睡眠的切片長度：停止要求最慢在這個時間內被看到。
const STOP_POLL: Duration = Duration::from_millis(250);

/// 睡 `total`，每個切片檢查一次 `stop`；`stop` 為真就提早醒來。
fn sleep_unless_stopped(total: Duration, stop: &dyn Fn() -> bool) {
    let mut left = total;
    while !left.is_zero() && !stop() {
        let slice = left.min(STOP_POLL);
        thread::sleep(slice);
        left -= slice;
    }
}

/// 可被停止的等網路（5.2 排程用；停止介面見 [`super::scheduler`]）：與 [`wait_for_network`] 相同的參數與
/// 計時，但睡眠分成小片、每片檢查 `stop`，`stop` 為真就立即回傳 `true`（**不是**「網路就緒」的意思，是
/// 「別再等了」——呼叫端必須在回傳後先檢查 `stop` 再決定要不要往下跑）。
pub fn wait_for_network_unless_stopped(stop: &(dyn Fn() -> bool + Sync)) -> bool {
    wait_stoppable_with(
        || tcp_probe(TIMEOUT_EACH),
        |d| sleep_unless_stopped(d, stop),
        stop,
        INTERVAL,
        MAX_WAIT,
    )
}

/// [`wait_for_network_unless_stopped`] 的可注入版本。`stop` 在每次探測前問一次；為真就不再探測、直接回 `true`。
fn wait_stoppable_with<C, S>(
    mut connect: C,
    sleep: S,
    stop: &dyn Fn() -> bool,
    interval: Duration,
    max_wait: Duration,
) -> bool
where
    C: FnMut() -> io::Result<()>,
    S: FnMut(Duration),
{
    wait_for_network_with(
        || {
            if stop() {
                Ok(())
            } else {
                connect()
            }
        },
        sleep,
        interval,
        max_wait,
    )
}

/// 單次探測：解析 [`PROBE_HOST`] 後逐一嘗試連線（每個位址各 `timeout`），任一成功即 `Ok`。
/// DNS 解析本身沒有逾時參數（`std` 限制，同 Python `socket.create_connection`）。
pub(crate) fn tcp_probe(timeout: Duration) -> io::Result<()> {
    let mut last: Option<io::Error> = None;
    for addr in (PROBE_HOST, PROBE_PORT).to_socket_addrs()? {
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(_) => return Ok(()),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| io::Error::other("解析不到任何位址")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    fn refused() -> io::Error {
        io::Error::new(io::ErrorKind::ConnectionRefused, "模擬連線被拒")
    }

    #[test]
    fn ready_on_first_probe_never_sleeps() {
        let sleeps = RefCell::new(Vec::new());
        let ok = wait_for_network_with(
            || Ok(()),
            |d| sleeps.borrow_mut().push(d),
            INTERVAL,
            MAX_WAIT,
        );
        assert!(ok);
        assert!(sleeps.borrow().is_empty());
    }

    #[test]
    fn retries_every_interval_until_ready() {
        let calls = Cell::new(0u32);
        let sleeps = RefCell::new(Vec::new());
        let ok = wait_for_network_with(
            || {
                calls.set(calls.get() + 1);
                if calls.get() < 3 {
                    Err(refused())
                } else {
                    Ok(())
                }
            },
            |d| sleeps.borrow_mut().push(d),
            INTERVAL,
            MAX_WAIT,
        );
        assert!(ok);
        assert_eq!(calls.get(), 3);
        assert_eq!(*sleeps.borrow(), vec![INTERVAL, INTERVAL]);
    }

    #[test]
    fn gives_up_after_max_wait_of_accumulated_sleep() {
        // 同 Python：waited 只累計睡眠；0、20、…、300 共 16 次探測、15 次睡眠，之後放行（false）。
        let calls = Cell::new(0u32);
        let slept = Cell::new(Duration::ZERO);
        let ok = wait_for_network_with(
            || {
                calls.set(calls.get() + 1);
                Err(refused())
            },
            |d| slept.set(slept.get() + d),
            INTERVAL,
            MAX_WAIT,
        );
        assert!(!ok);
        assert_eq!(calls.get(), 16);
        assert_eq!(slept.get(), Duration::from_secs(300));
    }

    #[test]
    fn zero_max_wait_probes_once_and_gives_up() {
        let calls = Cell::new(0u32);
        let ok = wait_for_network_with(
            || {
                calls.set(calls.get() + 1);
                Err(refused())
            },
            |_| panic!("max_wait 為零不該睡眠"),
            INTERVAL,
            Duration::ZERO,
        );
        assert!(!ok);
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn zero_interval_does_not_spin_forever() {
        let calls = Cell::new(0u32);
        let ok = wait_for_network_with(
            || {
                calls.set(calls.get() + 1);
                Err(refused())
            },
            |_| panic!("interval 為零不該睡眠"),
            Duration::ZERO,
            MAX_WAIT,
        );
        assert!(!ok);
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn a_stop_request_ends_the_wait_without_probing_again() {
        // 連線永遠失敗；第二次睡眠之後停止要求成立：不再探測、直接回傳（呼叫端要自己檢查 stop）。
        let probes = Cell::new(0u32);
        let sleeps = Cell::new(0u32);
        let stop = || sleeps.get() >= 2;
        let returned = wait_stoppable_with(
            || {
                probes.set(probes.get() + 1);
                Err(refused())
            },
            |_| sleeps.set(sleeps.get() + 1),
            &stop,
            INTERVAL,
            MAX_WAIT,
        );
        assert!(returned);
        assert_eq!(probes.get(), 2, "停止後不再發探測");
        assert_eq!(sleeps.get(), 2);
    }

    #[test]
    fn without_a_stop_request_the_stoppable_wait_behaves_like_the_plain_one() {
        let probes = Cell::new(0u32);
        let stop = || false;
        let ok = wait_stoppable_with(
            || {
                probes.set(probes.get() + 1);
                Err(refused())
            },
            |_| {},
            &stop,
            INTERVAL,
            MAX_WAIT,
        );
        assert!(!ok, "逾時仍是 false");
        assert_eq!(probes.get(), 16);
    }

    #[test]
    fn stoppable_sleep_returns_promptly_when_stopped() {
        let started = std::time::Instant::now();
        sleep_unless_stopped(Duration::from_secs(60), &|| true);
        assert!(started.elapsed() < Duration::from_secs(1));
        // 睡眠中途才收到停止：在下一個切片內醒來。
        let calls = Cell::new(0u32);
        let started = std::time::Instant::now();
        sleep_unless_stopped(Duration::from_secs(60), &|| {
            calls.set(calls.get() + 1);
            calls.get() > 2
        });
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn last_probe_after_the_final_sleep_can_still_succeed() {
        // 睡到第 15 次（waited=300）之後的第 16 次探測成功＝就緒，不是逾時。
        let calls = Cell::new(0u32);
        let ok = wait_for_network_with(
            || {
                calls.set(calls.get() + 1);
                if calls.get() == 16 {
                    Ok(())
                } else {
                    Err(refused())
                }
            },
            |_| {},
            INTERVAL,
            MAX_WAIT,
        );
        assert!(ok);
        assert_eq!(calls.get(), 16);
    }
}
