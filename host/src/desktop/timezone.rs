//! 本機時區（dynamic-wallpaper task 4.7a）：[`crate::wallpaper::UtcOffsetSource`] 的 Windows 實作。
//!
//! 排程器（4.3）只需要「任意 UTC 時刻當下的 UTC 偏移」。這裡每次都以
//! `GetDynamicTimeZoneInformation` 取**目前**設定的時區（含動態夏令規則），再以
//! `SystemTimeToTzSpecificLocalTimeEx` 把該 UTC 時刻換成本機時刻，兩者相減即偏移。不快取時區：
//! 使用者改了時區，下一次呼叫就用新的（是否確實不需重新啟動行程，待實機驗證，見 4.7a 報告）。
//!
//! 取不到（API 失敗、年份超出 `SYSTEMTIME` 範圍）時沿用上一次成功的偏移（第一次就失敗時為 0），
//! 並記一行警告（同一段失敗只記一次）。

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use windows::Win32::Foundation::SYSTEMTIME;
use windows::Win32::System::Time::{
    GetDynamicTimeZoneInformation, SystemTimeToTzSpecificLocalTimeEx,
    DYNAMIC_TIME_ZONE_INFORMATION, TIME_ZONE_ID_INVALID,
};

use crate::wallpaper::{CivilDate, UnixSeconds, UtcOffsetSource};

const SECS_PER_DAY: i64 = 86_400;

/// UNIX 秒 → `SYSTEMTIME`（UTC，毫秒為 0）。超出 `SYSTEMTIME` 年份範圍（1601–30827）回 `None`。
pub fn systemtime_from_unix(t: UnixSeconds) -> Option<SYSTEMTIME> {
    let date = CivilDate::from_days(t.div_euclid(SECS_PER_DAY));
    if !(1601..=30827).contains(&date.year) {
        return None;
    }
    let secs = t.rem_euclid(SECS_PER_DAY);
    // 1970-01-01 是週四（4）；SYSTEMTIME 的星期以週日為 0。
    let weekday = (t.div_euclid(SECS_PER_DAY) + 4).rem_euclid(7);
    Some(SYSTEMTIME {
        wYear: u16::try_from(date.year).ok()?,
        wMonth: u16::from(date.month),
        wDayOfWeek: u16::try_from(weekday).ok()?,
        wDay: u16::from(date.day),
        wHour: u16::try_from(secs / 3600).ok()?,
        wMinute: u16::try_from(secs / 60 % 60).ok()?,
        wSecond: u16::try_from(secs % 60).ok()?,
        wMilliseconds: 0,
    })
}

/// `SYSTEMTIME` → 「該欄位所代表的時刻」的 UNIX 秒（不看時區；本機 `SYSTEMTIME` 得到的是本機秒）。
pub fn unix_from_systemtime(st: &SYSTEMTIME) -> Option<i64> {
    let month = u8::try_from(st.wMonth).ok()?;
    let day = u8::try_from(st.wDay).ok()?;
    if !(1..=12).contains(&month) || day == 0 || day > 31 {
        return None;
    }
    let days = CivilDate {
        year: i32::from(st.wYear),
        month,
        day,
    }
    .to_days();
    Some(
        days * SECS_PER_DAY
            + i64::from(st.wHour) * 3600
            + i64::from(st.wMinute) * 60
            + i64::from(st.wSecond),
    )
}

/// 目前 Windows 時區下，UTC 時刻 `utc` 的偏移秒數；API 失敗回 `None`。
pub fn current_offset_at(utc: UnixSeconds) -> Option<i32> {
    let st = systemtime_from_unix(utc)?;
    let mut tz = DYNAMIC_TIME_ZONE_INFORMATION::default();
    // SAFETY: `tz` 是本函式的區域變數，API 只寫入它。
    if unsafe { GetDynamicTimeZoneInformation(&mut tz) } == TIME_ZONE_ID_INVALID {
        return None;
    }
    let mut local = SYSTEMTIME::default();
    // SAFETY: 三個指標都指向本函式的區域變數，生命週期涵蓋這次呼叫。
    unsafe { SystemTimeToTzSpecificLocalTimeEx(Some(&tz), &st, &mut local) }.ok()?;
    let offset = unix_from_systemtime(&local)? - utc;
    i32::try_from(offset).ok()
}

/// Windows 本機時區（[`UtcOffsetSource`]）。見模組文件。
#[derive(Debug, Default)]
pub struct WindowsLocalTime {
    last_good: AtomicI32,
    failing: AtomicBool,
}

impl UtcOffsetSource for WindowsLocalTime {
    fn offset_at(&self, utc: UnixSeconds) -> i32 {
        match current_offset_at(utc) {
            Some(offset) => {
                self.last_good.store(offset, Ordering::Relaxed);
                if self.failing.swap(false, Ordering::Relaxed) {
                    log::info!("本機時區查詢恢復正常（偏移 {offset} 秒）");
                }
                offset
            }
            None => {
                let fallback = self.last_good.load(Ordering::Relaxed);
                if !self.failing.swap(true, Ordering::Relaxed) {
                    log::warn!("查詢本機時區失敗，沿用上一次的偏移 {fallback} 秒");
                }
                fallback
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn systemtime_round_trips_and_knows_the_weekday() {
        // 2026-10-05T02:05:07Z 是週一。
        let t = CivilDate {
            year: 2026,
            month: 10,
            day: 5,
        }
        .to_days()
            * SECS_PER_DAY
            + 2 * 3600
            + 5 * 60
            + 7;
        let st = systemtime_from_unix(t).unwrap();
        assert_eq!(
            (st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond),
            (2026, 10, 5, 2, 5, 7)
        );
        assert_eq!(st.wDayOfWeek, 1);
        assert_eq!(unix_from_systemtime(&st), Some(t));
        assert_eq!(systemtime_from_unix(-400 * 365 * SECS_PER_DAY), None);
    }

    /// 唯讀查詢真正的系統時區：只要求結果是合理的偏移、且同一時刻查兩次相同。
    #[test]
    fn current_offset_is_a_sane_value() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let now = i64::try_from(now).unwrap();
        let offset = current_offset_at(now).expect("查詢本機時區失敗");
        assert!((-14 * 3600..=14 * 3600).contains(&offset), "{offset}");
        assert_eq!(offset % 900, 0, "偏移應是 15 分鐘的倍數：{offset}");
        assert_eq!(WindowsLocalTime::default().offset_at(now), offset);
    }
}
