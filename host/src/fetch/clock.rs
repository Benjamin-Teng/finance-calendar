//! 時間注入（design.md D3、D11）。
//!
//! 抓取流程需要兩種「現在」，彼此獨立：
//!
//! - **台北時間**（固定 +08:00，B-DATE-1／B-DATE-2）：日期窗口、前百大「今天」、MOPS 月份、
//!   盤中走勢。不用系統時區，防排程在 JST 00:00 把窗口推前一日。
//! - **本機時間**（naive 牆上時間，B-DATE-3）：只用於 `updated`／`fetched`，格式
//!   `%Y-%m-%d %H:%M`。前端以本機時間解讀，所以不可寫 UTC 或台北時間。
//!
//! 各來源收一個 [`Clock`]（而不是自己取時間），測試才能凍結。

use time::{macros::format_description, Date, OffsetDateTime, PrimitiveDateTime, UtcOffset};

/// 台北固定 +08:00（無夏令）。
pub const TPE_OFFSET: UtcOffset = time::macros::offset!(+8);

/// 一輪抓取共用的「現在」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Clock {
    /// 台北時間（偏移固定 +08:00）。
    pub tpe: OffsetDateTime,
    /// 本機牆上時間（無時區），對應 Python 的 `datetime.now()`。
    pub local: PrimitiveDateTime,
}

impl Clock {
    /// 由兩個值組成；`tpe` 會被換算成 +08:00（偏移不同也沒關係，同一時刻）。
    pub fn new(tpe: OffsetDateTime, local: PrimitiveDateTime) -> Self {
        Clock {
            tpe: tpe.to_offset(TPE_OFFSET),
            local,
        }
    }

    /// 正式取法：台北＝`now_utc` 轉 +08:00；本機＝`OffsetDateTime::now_local()`，失敗退回 UTC
    /// 並以 `log` 記一筆警告（D11）。
    pub fn now() -> Self {
        let utc = OffsetDateTime::now_utc();
        let local = match OffsetDateTime::now_local() {
            Ok(l) => l,
            Err(e) => {
                log::warn!("fetch: 取不到本機時區偏移（{e}），本機時間退回 UTC");
                utc
            }
        };
        Clock::new(utc, PrimitiveDateTime::new(local.date(), local.time()))
    }

    /// 台北今天的日期（B-DATE-2）。
    pub fn tpe_date(&self) -> Date {
        self.tpe.date()
    }

    /// 本機時間字串 `%Y-%m-%d %H:%M`（B-DATE-3，`updated`／`fetched` 用）。
    pub fn local_stamp(&self) -> String {
        self.local
            .format(format_description!("[year]-[month]-[day] [hour]:[minute]"))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::{datetime, offset};

    #[test]
    fn new_normalises_tpe_to_plus_eight() {
        let c = Clock::new(
            datetime!(2026-10-04 18:30:00 UTC),
            datetime!(2026-10-05 01:30:00),
        );
        assert_eq!(c.tpe.offset(), offset!(+8));
        assert_eq!(c.tpe, datetime!(2026-10-05 02:30:00 +8));
        assert_eq!(c.tpe_date(), time::macros::date!(2026 - 10 - 05));
    }

    #[test]
    fn tpe_date_ignores_the_local_clock() {
        // 本機是日本時間 00:30（10/5），台北已是 23:30（10/4）：窗口要以台北為準（B-DATE-2）。
        let c = Clock::new(
            datetime!(2026-10-04 23:30:00 +8),
            datetime!(2026-10-05 00:30:00),
        );
        assert_eq!(c.tpe_date(), time::macros::date!(2026 - 10 - 04));
        assert_eq!(c.local_stamp(), "2026-10-05 00:30");
    }

    #[test]
    fn local_stamp_format_is_zero_padded_minute_resolution() {
        let c = Clock::new(
            datetime!(2026-01-02 03:04:59 +8),
            datetime!(2026-01-02 03:04:59.999),
        );
        assert_eq!(c.local_stamp(), "2026-01-02 03:04");
    }

    #[test]
    fn now_returns_a_plausible_clock() {
        let c = Clock::now();
        assert_eq!(c.tpe.offset(), TPE_OFFSET);
        // 本機時間與台北時間的差在 ±26 小時內（時區偏移範圍＋執行期間誤差）。
        let tpe_wall = PrimitiveDateTime::new(c.tpe.date(), c.tpe.time());
        let diff = (c.local - tpe_wall).whole_hours().abs();
        assert!(diff <= 26, "本機與台北時間相差 {diff} 小時");
    }
}
