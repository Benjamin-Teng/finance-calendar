//! 可取消的睡眠：更新器各執行緒的計時共用（`Condvar` 逾時等待，取消時立即醒來）。
//!
//! [`Timer`] 是注入點：正式用 [`RealTimer`]（真的睡），單元測試用假計時器（記錄要睡多久、立即返回），
//! 讓「60–180 秒延遲」「6 小時週期」「5 分鐘到期」「60 秒過渡期逾時」都能不真的等待就驗證。
//!
//! 睡眠是相對逾時、以**清醒時間**計（Windows 8 以後不計入系統睡眠時間，見 `schedule` 模組文件「兩種時間」）；
//! 要以牆上時鐘計的（6 小時檢查週期）改用短週期醒來＋[`Timer::wall_now`] 比對。

use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, SystemTime};

/// 取消旗標＋喚醒：複製出去的把手共用同一個狀態。
#[derive(Clone, Default)]
pub struct CancelToken {
    inner: Arc<(Mutex<bool>, Condvar)>,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// 設為已取消並喚醒所有等待者。冪等。
    pub fn cancel(&self) {
        let (lock, cvar) = &*self.inner;
        *lock.lock().unwrap_or_else(|e| e.into_inner()) = true;
        cvar.notify_all();
    }

    pub fn is_cancelled(&self) -> bool {
        *self.inner.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 最多等 `timeout`；回傳是否已被取消（`true`＝取消、`false`＝時間到）。
    pub fn wait(&self, timeout: Duration) -> bool {
        let (lock, cvar) = &*self.inner;
        let guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let (guard, _) = cvar
            .wait_timeout_while(guard, timeout, |cancelled| !*cancelled)
            .unwrap_or_else(|e| e.into_inner());
        *guard
    }
}

/// 睡眠與牆上時鐘的注入點。
pub trait Timer: Send + Sync {
    /// 睡 `duration`（清醒時間）；`cancel` 被取消就提早醒來。回傳 `true`＝被取消、`false`＝睡滿。
    fn sleep(&self, duration: Duration, cancel: &CancelToken) -> bool;

    /// 牆上時鐘（含睡眠；使用者調時間時可能倒退或前跳）。測試以假時鐘覆寫。
    fn wall_now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// 正式計時器：真的睡（可被 [`CancelToken::cancel`] 喚醒）。
pub struct RealTimer;

impl Timer for RealTimer {
    fn sleep(&self, duration: Duration, cancel: &CancelToken) -> bool {
        cancel.wait(duration)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn wait_returns_false_after_timeout_and_true_when_cancelled() {
        let token = CancelToken::new();
        assert!(
            !token.wait(Duration::from_millis(10)),
            "沒取消，時間到回 false"
        );
        token.cancel();
        let start = Instant::now();
        assert!(token.wait(Duration::from_secs(30)), "已取消，立即回 true");
        assert!(start.elapsed() < Duration::from_secs(5));
        assert!(token.is_cancelled());
    }

    #[test]
    fn cancel_wakes_a_sleeping_thread() {
        let token = CancelToken::new();
        let t2 = token.clone();
        let handle = std::thread::spawn(move || RealTimer.sleep(Duration::from_secs(60), &t2));
        std::thread::sleep(Duration::from_millis(50));
        token.cancel();
        assert!(handle.join().expect("睡眠執行緒"), "被取消喚醒應回 true");
    }
}
