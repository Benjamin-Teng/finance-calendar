//! `crate::wallpaper` 排程純函式的單元測試（task 4.3）。所有時間、時區都由參數注入，不讀執行
//! 測試那台機器的時鐘與時區。

use super::*;
use std::collections::{HashMap, HashSet};

const TPE: FixedOffset = FixedOffset(8 * 3600);

fn d(year: i32, month: u8, day: u8) -> CivilDate {
    CivilDate { year, month, day }
}

/// 固定偏移下的本機時刻 → UTC 秒。
fn at(offset: i32, date: CivilDate, h: i64, mi: i64, s: i64) -> UnixSeconds {
    date.to_days() * SECS_PER_DAY + h * 3600 + mi * 60 + s - i64::from(offset)
}

/// 台北時間 → UTC 秒。
fn tpe(date: CivilDate, h: i64, mi: i64, s: i64) -> UnixSeconds {
    at(TPE.0, date, h, mi, s)
}

/// UTC 時刻 → UTC 秒。
fn utc(date: CivilDate, h: i64, mi: i64, s: i64) -> UnixSeconds {
    at(0, date, h, mi, s)
}

fn mon(path: &str, width: u32, height: u32, dpi: u32) -> MonitorSnapshot {
    MonitorSnapshot {
        device_path: path.to_string(),
        geometry: MonitorGeometry { width, height, dpi },
    }
}

/// spec「雙螢幕不同解析度」：A 為 3840×2160、150%；B 為 2560×1600、100%。
fn two_monitors() -> Vec<MonitorSnapshot> {
    vec![mon("A", 3840, 2160, 144), mon("B", 2560, 1600, 96)]
}

fn data_all(date: CivilDate) -> DataDates {
    DataDates {
        twii_intraday: Some(date),
        twii_daily: Some(date),
        margin: Some(date),
    }
}

/// 模擬夏令時間：`[start, end)` 期間用 `dst` 偏移，其餘用 `std` 偏移。
struct Dst {
    std: i32,
    dst: i32,
    start: UnixSeconds,
    end: UnixSeconds,
}

impl UtcOffsetSource for Dst {
    fn offset_at(&self, utc: UnixSeconds) -> i32 {
        if (self.start..self.end).contains(&utc) {
            self.dst
        } else {
            self.std
        }
    }
}

/// 類似歐洲中部：標準 +01:00、夏令 +02:00，2026-03-29 01:00Z 開始、2026-10-25 01:00Z 結束。
fn berlin_2026() -> Dst {
    Dst {
        std: 3600,
        dst: 7200,
        start: utc(d(2026, 3, 29), 1, 0, 0),
        end: utc(d(2026, 10, 25), 1, 0, 0),
    }
}

/// 模擬呼叫端（4.7）：持有狀態，每次評估後套用 forget；Redraw 中 `failing` 列出的螢幕依其失敗種類記為失敗，
/// 其餘記為成功。
struct Sim {
    state: SchedulerState,
    tz: Box<dyn UtcOffsetSource>,
    theme: WallpaperTheme,
    data: DataDates,
    pause: HashSet<PauseReason>,
    monitors: Vec<MonitorSnapshot>,
    already_taken_over: bool,
    failing: HashMap<String, FailureKind>,
    explorer: ExplorerResources,
}

impl Sim {
    fn new(tz: impl UtcOffsetSource + 'static, theme: WallpaperTheme) -> Self {
        Sim {
            state: SchedulerState::default(),
            tz: Box::new(tz),
            theme,
            data: data_all(d(2026, 10, 1)),
            pause: HashSet::new(),
            monitors: two_monitors(),
            already_taken_over: false,
            failing: HashMap::new(),
            explorer: ExplorerResources::NotSampled,
        }
    }

    fn eval(&self, now: UnixSeconds) -> Decision {
        next_action(
            &self.state,
            &SchedulerInput {
                now,
                tz: self.tz.as_ref(),
                theme: self.theme,
                pause: &self.pause,
                data: self.data,
                monitors: &self.monitors,
                already_taken_over: self.already_taken_over,
                explorer: self.explorer,
            },
        )
    }

    fn step(&mut self, now: UnixSeconds) -> Decision {
        let decision = self.eval(now);
        self.state.apply_forget(&decision);
        if let Action::Redraw(plan) = &decision.action {
            for target in &plan.targets {
                if let Some(kind) = self.failing.get(&target.monitor.device_path) {
                    self.state.record_failed(plan, target, *kind, now);
                } else {
                    self.state.record_drawn(plan, &target.monitor);
                }
            }
        }
        decision
    }

    /// 每 `every` 秒評估一次（模擬過量喚醒），回傳回傳 Redraw 的時刻。
    fn poll(&mut self, from: UnixSeconds, until: UnixSeconds, every: i64) -> Vec<UnixSeconds> {
        let mut drawn = Vec::new();
        let mut now = from;
        while now < until {
            if matches!(self.step(now).action, Action::Redraw(_)) {
                drawn.push(now);
            }
            now += every;
        }
        drawn
    }

    /// 只依 `next_wake` 喚醒（模擬 4.7 的單一計時器、沒有其他事件），回傳 Redraw 的時刻。
    fn follow_timer(&mut self, from: UnixSeconds, until: UnixSeconds) -> Vec<UnixSeconds> {
        let mut drawn = Vec::new();
        let mut now = from;
        while now < until {
            let mut decision = self.step(now);
            if matches!(decision.action, Action::Redraw(_)) {
                drawn.push(now);
                // 呼叫端契約：執行完 Redraw（成功或失敗都已記錄）後立即再評估一次，計時器用
                // 這一次的 next_wake（含失敗的重試時刻）。這次評估不得再回傳 Redraw。
                decision = self.step(now);
                assert!(
                    !matches!(decision.action, Action::Redraw(_)),
                    "執行完 Redraw 後的再評估又回傳 Redraw（忙迴圈）：{decision:?}"
                );
            }
            match decision.next_wake {
                Some(wake) => {
                    assert!(wake > now, "next_wake {wake} 必須晚於 now {now}");
                    now = wake;
                }
                None => break,
            }
        }
        drawn
    }
}

fn plan(decision: &Decision) -> &RedrawPlan {
    match &decision.action {
        Action::Redraw(plan) => plan,
        other => panic!("預期 Redraw，得到 {other:?}"),
    }
}

fn targets(decision: &Decision) -> Vec<(String, RedrawReason)> {
    plan(decision)
        .targets
        .iter()
        .map(|t| (t.monitor.device_path.clone(), t.reason))
        .collect()
}

fn both(reason: RedrawReason) -> Vec<(String, RedrawReason)> {
    vec![("A".to_string(), reason), ("B".to_string(), reason)]
}

// ── 民曆日期 ─────────────────────────────────────────────────────────────────

#[test]
fn civil_date_matches_known_epoch_days() {
    // 對照值由 GNU date（`date -u -d <日期> +%s` ÷ 86400）取得。
    let table = [
        (d(1970, 1, 1), 0),
        (d(1969, 12, 31), -1),
        (d(2000, 3, 1), 11_017),
        (d(2024, 2, 29), 19_782),
        (d(2026, 10, 2), 20_728),
        (d(1600, 3, 1), -135_080),
        (d(2100, 3, 1), 47_541),
    ];
    for (date, days) in table {
        assert_eq!(date.to_days(), days, "{date:?}");
        assert_eq!(CivilDate::from_days(days), date, "{days}");
    }
}

#[test]
fn civil_date_round_trips_and_is_ordered() {
    let mut prev = CivilDate::from_days(-150_001);
    for days in -150_000..60_000 {
        let date = CivilDate::from_days(days);
        assert_eq!(date.to_days(), days);
        assert!(date > prev, "{prev:?} → {date:?}");
        prev = date;
    }
}

#[test]
fn parse_iso_accepts_only_real_dates() {
    let table = [
        ("2026-10-02", Some(d(2026, 10, 2))),
        ("2024-02-29", Some(d(2024, 2, 29))),
        ("2000-02-29", Some(d(2000, 2, 29))),
        ("2025-02-29", None),
        ("1900-02-29", None),
        ("2026-02-30", None),
        ("2026-04-31", None),
        ("2026-13-01", None),
        ("2026-00-10", None),
        ("2026-10-00", None),
        ("2026-1-02", None),
        ("2026-10-02T00:00", None),
        ("2026/10/02", None),
        ("+202-10-02", None),
        ("", None),
        ("abcd-ef-gh", None),
    ];
    for (text, want) in table {
        assert_eq!(CivilDate::parse_iso(text), want, "{text:?}");
    }
}

// ── 不接管 ───────────────────────────────────────────────────────────────────

#[test]
fn theme_none_is_idle_without_wake() {
    let sim = Sim::new(TPE, WallpaperTheme::None);
    let decision = sim.eval(tpe(d(2026, 10, 2), 10, 0, 0));
    assert_eq!(decision.action, Action::Idle);
    assert_eq!(decision.next_wake, None);
    assert!(decision.forget.is_empty());
}

#[test]
fn switching_to_none_forgets_everything_and_back_redraws_all() {
    let day = d(2026, 10, 2);
    let mut sim = Sim::new(TPE, WallpaperTheme::Astrolabe);
    sim.step(tpe(day, 10, 0, 0));

    sim.theme = WallpaperTheme::None;
    let decision = sim.step(tpe(day, 10, 1, 0));
    assert_eq!(decision.action, Action::Idle);
    assert_eq!(decision.forget, vec!["A".to_string(), "B".to_string()]);
    assert_eq!(decision.next_wake, None);

    // 4.4 還原原桌布後，使用者在同一個時點內切回星盤：每台都要重畫。
    sim.theme = WallpaperTheme::Astrolabe;
    let decision = sim.step(tpe(day, 10, 2, 0));
    assert_eq!(targets(&decision), both(RedrawReason::Unrecorded));
}

// ── 等待資料 ─────────────────────────────────────────────────────────────────

#[test]
fn missing_required_data_waits_without_redrawing() {
    let date = d(2026, 10, 1);
    let none = DataDates::default();
    let only_daily = DataDates {
        twii_daily: Some(date),
        ..none
    };
    let only_intraday = DataDates {
        twii_intraday: Some(date),
        ..none
    };
    let table = [
        (WallpaperTheme::Ridgeline, none, Some(DataKey::TwiiIntraday)),
        (
            WallpaperTheme::Ridgeline,
            only_daily,
            Some(DataKey::TwiiIntraday),
        ),
        (WallpaperTheme::Ridgeline, only_intraday, None),
        (WallpaperTheme::Contour, none, Some(DataKey::TwiiIntraday)),
        (
            WallpaperTheme::Contour,
            only_daily,
            Some(DataKey::TwiiIntraday),
        ),
        (WallpaperTheme::Contour, only_intraday, None),
        (WallpaperTheme::Skyline, none, Some(DataKey::TwiiDaily)),
        (
            WallpaperTheme::Skyline,
            only_intraday,
            Some(DataKey::TwiiDaily),
        ),
        (WallpaperTheme::Skyline, only_daily, None),
        // 撕日曆、星盤不需要市場資料就能畫。
        (WallpaperTheme::Tearoff, none, None),
        (WallpaperTheme::Astrolabe, none, None),
    ];
    for (theme, data, missing) in table {
        let mut sim = Sim::new(TPE, theme);
        sim.data = data;
        let decision = sim.eval(tpe(d(2026, 10, 2), 10, 0, 0));
        match missing {
            Some(key) => {
                assert_eq!(
                    decision.action,
                    Action::WaitingForData { missing: key },
                    "{theme:?} {data:?}"
                );
                assert_eq!(decision.next_wake, None, "{theme:?}");
            }
            None => assert_eq!(
                targets(&decision),
                both(RedrawReason::Unrecorded),
                "{theme:?} {data:?}"
            ),
        }
    }
}

#[test]
fn takeover_starts_on_first_evaluation_after_data_appears() {
    // spec「資料層還沒跑過」：選天際線、沒有日 K → 等待；日 K 出現後的下一次檢查即開始接管。
    let day = d(2026, 10, 2);
    let mut sim = Sim::new(TPE, WallpaperTheme::Skyline);
    sim.data = DataDates::default();
    for minute in 0..30 {
        let decision = sim.step(tpe(day, 10, minute, 0));
        assert_eq!(
            decision.action,
            Action::WaitingForData {
                missing: DataKey::TwiiDaily
            }
        );
    }
    sim.data.twii_daily = Some(d(2026, 10, 1));
    let decision = sim.step(tpe(day, 10, 30, 0));
    assert_eq!(targets(&decision), both(RedrawReason::Unrecorded));
    assert_eq!(plan(&decision).stamp, Stamp::Daily(d(2026, 10, 1)));
}

// ── 星盤 ─────────────────────────────────────────────────────────────────────

#[test]
fn astrolabe_slot_boundaries() {
    let day = d(2026, 10, 2);
    let mut sim = Sim::new(TPE, WallpaperTheme::Astrolabe);
    sim.step(tpe(day, 10, 0, 0));

    let before = sim.eval(tpe(day, 10, 14, 59));
    assert_eq!(before.action, Action::UpToDate);
    assert_eq!(before.next_wake, Some(tpe(day, 10, 15, 0)));

    let on = sim.eval(tpe(day, 10, 15, 0));
    assert_eq!(targets(&on), both(RedrawReason::Scheduled));
    assert_eq!(plan(&on).as_of, tpe(day, 10, 15, 0));
    assert_eq!(plan(&on).stamp, Stamp::Slot(tpe(day, 10, 15, 0)));
    assert_eq!(on.next_wake, Some(tpe(day, 10, 30, 0)));
}

#[test]
fn astrolabe_no_redraw_until_next_quarter() {
    // spec「星盤對齊 15 分鐘」「星盤兩次更新之間」：10:15 重畫後，10:30 前任何呼叫都不再重畫。
    let day = d(2026, 10, 2);
    let mut sim = Sim::new(TPE, WallpaperTheme::Astrolabe);
    sim.step(tpe(day, 10, 0, 0));
    assert!(matches!(
        sim.step(tpe(day, 10, 15, 0)).action,
        Action::Redraw(_)
    ));
    let mut now = tpe(day, 10, 15, 1);
    while now < tpe(day, 10, 30, 0) {
        let decision = sim.step(now);
        assert_eq!(decision.action, Action::UpToDate, "at {now}");
        assert_eq!(decision.next_wake, Some(tpe(day, 10, 30, 0)));
        now += 7;
    }
    assert!(matches!(
        sim.step(tpe(day, 10, 30, 0)).action,
        Action::Redraw(_)
    ));
}

#[test]
fn astrolabe_startup_mid_slot_draws_current_slot() {
    let day = d(2026, 10, 2);
    let sim = Sim::new(TPE, WallpaperTheme::Astrolabe);
    let decision = sim.eval(tpe(day, 10, 7, 30));
    assert_eq!(targets(&decision), both(RedrawReason::Unrecorded));
    assert_eq!(plan(&decision).as_of, tpe(day, 10, 0, 0));
    assert_eq!(decision.next_wake, Some(tpe(day, 10, 15, 0)));
}

#[test]
fn astrolabe_crosses_midnight() {
    let day = d(2026, 10, 2);
    let next = d(2026, 10, 3);
    let mut sim = Sim::new(TPE, WallpaperTheme::Astrolabe);
    sim.step(tpe(day, 23, 45, 0));
    let last = sim.eval(tpe(day, 23, 59, 59));
    assert_eq!(last.action, Action::UpToDate);
    assert_eq!(last.next_wake, Some(tpe(next, 0, 0, 0)));
    let midnight = sim.eval(tpe(next, 0, 0, 0));
    assert_eq!(targets(&midnight), both(RedrawReason::Scheduled));
    assert_eq!(plan(&midnight).as_of, tpe(next, 0, 0, 0));
}

#[test]
fn astrolabe_full_day_draws_exactly_on_quarters() {
    // 「兩個重畫時點之間不渲染」：每分鐘評估一次，重畫只發生在 0／15／30／45 分。
    let day = d(2026, 10, 2);
    let start = tpe(day, 0, 0, 0);
    let end = tpe(d(2026, 10, 3), 0, 0, 0);
    let expected: Vec<_> = (0..96).map(|i| start + i * ASTROLABE_SLOT_SECS).collect();

    let mut polled = Sim::new(TPE, WallpaperTheme::Astrolabe);
    assert_eq!(polled.poll(start, end, 60), expected);
    let mut timed = Sim::new(TPE, WallpaperTheme::Astrolabe);
    assert_eq!(timed.follow_timer(start, end), expected);
}

// ── 脊線、等高線 ─────────────────────────────────────────────────────────────

#[test]
fn daily_themes_redraw_at_0600_local() {
    let day = d(2026, 10, 2);
    let next = d(2026, 10, 3);
    for theme in [WallpaperTheme::Ridgeline, WallpaperTheme::Contour] {
        let mut sim = Sim::new(TPE, theme);
        let start = sim.step(tpe(day, 3, 0, 0));
        assert_eq!(targets(&start), both(RedrawReason::Unrecorded), "{theme:?}");
        assert_eq!(plan(&start).stamp, Stamp::Day(d(2026, 10, 1)), "{theme:?}");
        assert_eq!(plan(&start).as_of, tpe(day, 3, 0, 0), "{theme:?}");
        assert_eq!(start.next_wake, Some(tpe(day, 6, 0, 0)), "{theme:?}");

        let before = sim.eval(tpe(day, 5, 59, 59));
        assert_eq!(before.action, Action::UpToDate, "{theme:?}");
        assert_eq!(before.next_wake, Some(tpe(day, 6, 0, 0)), "{theme:?}");

        let on = sim.step(tpe(day, 6, 0, 0));
        assert_eq!(targets(&on), both(RedrawReason::Scheduled), "{theme:?}");
        assert_eq!(plan(&on).stamp, Stamp::Day(day), "{theme:?}");
        assert_eq!(on.next_wake, Some(tpe(next, 6, 0, 0)), "{theme:?}");

        for (date, h, mi, s) in [(day, 6, 0, 1), (day, 12, 0, 0), (day, 23, 59, 59)] {
            assert_eq!(
                sim.eval(tpe(date, h, mi, s)).action,
                Action::UpToDate,
                "{theme:?}"
            );
        }
        // 跨日（本機 00:00）不觸發，要等到 06:00。
        let after_midnight = sim.eval(tpe(next, 0, 0, 0));
        assert_eq!(after_midnight.action, Action::UpToDate, "{theme:?}");
        assert_eq!(after_midnight.next_wake, Some(tpe(next, 6, 0, 0)));
        assert_eq!(
            sim.eval(tpe(next, 5, 59, 59)).action,
            Action::UpToDate,
            "{theme:?}"
        );
        let next_on = sim.step(tpe(next, 6, 0, 0));
        assert_eq!(plan(&next_on).stamp, Stamp::Day(next), "{theme:?}");
    }
}

#[test]
fn daily_themes_ignore_data_updates() {
    let day = d(2026, 10, 2);
    for theme in [WallpaperTheme::Ridgeline, WallpaperTheme::Contour] {
        let mut sim = Sim::new(TPE, theme);
        sim.step(tpe(day, 7, 0, 0));
        sim.data = data_all(day);
        assert_eq!(
            sim.eval(tpe(day, 15, 5, 0)).action,
            Action::UpToDate,
            "{theme:?}"
        );
    }
}

#[test]
fn daily_themes_timer_draws_once_per_day() {
    let day = d(2026, 10, 2);
    let start = tpe(day, 3, 0, 0);
    let end = tpe(d(2026, 10, 5), 3, 0, 0);
    let expected = vec![
        start,
        tpe(day, 6, 0, 0),
        tpe(d(2026, 10, 3), 6, 0, 0),
        tpe(d(2026, 10, 4), 6, 0, 0),
    ];
    for theme in [WallpaperTheme::Ridgeline, WallpaperTheme::Contour] {
        assert_eq!(
            Sim::new(TPE, theme).follow_timer(start, end),
            expected,
            "{theme:?}"
        );
        assert_eq!(
            Sim::new(TPE, theme).poll(start, end, 60),
            expected,
            "{theme:?}"
        );
    }
}

// ── 天際線 ───────────────────────────────────────────────────────────────────

#[test]
fn skyline_redraws_only_on_newer_trading_day() {
    let day = d(2026, 10, 2);
    let mut sim = Sim::new(TPE, WallpaperTheme::Skyline);
    let start = sim.step(tpe(day, 10, 0, 0));
    assert_eq!(plan(&start).stamp, Stamp::Daily(d(2026, 10, 1)));
    assert_eq!(start.next_wake, None, "天際線沒有時間觸發");

    // 換日、06:00 都不觸發。
    for (date, h) in [(d(2026, 10, 3), 0), (d(2026, 10, 3), 6)] {
        let decision = sim.eval(tpe(date, h, 0, 0));
        assert_eq!(decision.action, Action::UpToDate);
        assert_eq!(decision.next_wake, None);
    }
    // 其他鍵變更不觸發。
    sim.data.twii_intraday = Some(day);
    sim.data.margin = Some(day);
    assert_eq!(sim.eval(tpe(day, 15, 1, 0)).action, Action::UpToDate);

    // 日 K 換成新交易日 → 重畫。
    sim.data.twii_daily = Some(day);
    let newer = sim.step(tpe(day, 15, 5, 0));
    assert_eq!(targets(&newer), both(RedrawReason::Scheduled));
    assert_eq!(plan(&newer).stamp, Stamp::Daily(day));
    assert_eq!(plan(&newer).as_of, tpe(day, 15, 5, 0));

    // 資料退回舊日期（來源失敗沿用舊快照之類）→ 不重畫。
    sim.data.twii_daily = Some(d(2026, 10, 1));
    assert_eq!(sim.eval(tpe(day, 21, 5, 0)).action, Action::UpToDate);
}

// ── 撕日曆 ───────────────────────────────────────────────────────────────────

#[test]
fn tearoff_redraws_on_local_date_change_and_margin_change() {
    let day = d(2026, 10, 2);
    let next = d(2026, 10, 3);
    let m1 = d(2026, 10, 1);
    let m2 = d(2026, 10, 2);
    let mut sim = Sim::new(TPE, WallpaperTheme::Tearoff);
    sim.data.margin = Some(m1);
    let start = sim.step(tpe(day, 20, 0, 0));
    assert_eq!(
        plan(&start).stamp,
        Stamp::Tearoff {
            date: day,
            margin: Some(m1)
        }
    );
    assert_eq!(start.next_wake, Some(tpe(next, 0, 0, 0)));

    let before = sim.eval(tpe(day, 23, 59, 59));
    assert_eq!(before.action, Action::UpToDate);
    assert_eq!(before.next_wake, Some(tpe(next, 0, 0, 0)));

    let midnight = sim.step(tpe(next, 0, 0, 0));
    assert_eq!(targets(&midnight), both(RedrawReason::Scheduled));
    assert_eq!(
        plan(&midnight).stamp,
        Stamp::Tearoff {
            date: next,
            margin: Some(m1)
        }
    );

    // 其他鍵變更不觸發。
    sim.data.twii_intraday = Some(next);
    sim.data.twii_daily = Some(next);
    assert_eq!(sim.eval(tpe(next, 15, 5, 0)).action, Action::UpToDate);

    // 融資日期變更 → 重畫（不必等換日）。
    sim.data.margin = Some(m2);
    let margin = sim.step(tpe(next, 21, 30, 0));
    assert_eq!(targets(&margin), both(RedrawReason::Scheduled));
    assert_eq!(
        plan(&margin).stamp,
        Stamp::Tearoff {
            date: next,
            margin: Some(m2)
        }
    );
    assert_eq!(sim.eval(tpe(next, 21, 31, 0)).action, Action::UpToDate);
}

// ── 暫停 ─────────────────────────────────────────────────────────────────────

#[test]
fn unlock_catches_up_once_with_latest_slot() {
    // spec「解鎖後補畫」：10:05 鎖定、11:20 解鎖 → 鎖定期間不更新，解鎖後立即畫 11:15 的盤面。
    let day = d(2026, 10, 2);
    let mut sim = Sim::new(TPE, WallpaperTheme::Astrolabe);
    sim.step(tpe(day, 10, 0, 0));

    sim.pause.insert(PauseReason::Locked);
    let mut now = tpe(day, 10, 5, 0);
    while now < tpe(day, 11, 20, 0) {
        let decision = sim.step(now);
        assert_eq!(decision.action, Action::Paused, "at {now}");
        assert_eq!(decision.next_wake, None);
        now += 60;
    }

    sim.pause.clear();
    let resumed = sim.step(tpe(day, 11, 20, 0));
    assert_eq!(targets(&resumed), both(RedrawReason::Scheduled));
    assert_eq!(plan(&resumed).as_of, tpe(day, 11, 15, 0));
    assert_eq!(resumed.next_wake, Some(tpe(day, 11, 30, 0)));

    // 只補一次。
    assert_eq!(
        sim.poll(tpe(day, 11, 20, 1), tpe(day, 11, 30, 0), 13),
        Vec::<UnixSeconds>::new()
    );
}

#[test]
fn every_pause_reason_stops_redraw() {
    let day = d(2026, 10, 2);
    let reasons = [
        PauseReason::Locked,
        PauseReason::SystemBusy,
        PauseReason::DisplayOff,
        PauseReason::Battery,
        PauseReason::PowerSaver,
        PauseReason::Manual,
    ];
    for reason in reasons {
        let mut sim = Sim::new(TPE, WallpaperTheme::Astrolabe);
        sim.step(tpe(day, 10, 0, 0));
        sim.pause.insert(reason);
        let decision = sim.eval(tpe(day, 10, 15, 0));
        assert_eq!(decision.action, Action::Paused, "{reason:?}");
        assert_eq!(decision.next_wake, None, "{reason:?}");
    }
}

#[test]
fn battery_and_power_saver_follow_pause_rules() {
    // 走宿主實際的路徑（4.7 讀 `AppState::pause_reasons`，那份集合就是這樣維護的）：
    // 原始電源訊號＋使用者的 `PauseRules` → `widgets::battery_pause_active`／
    // `power_saver_pause_active`（`apply_battery_pause_rule` 等用的同一個判定）→
    // `widgets::apply_pause_reason` 更新集合 → `next_action`。只差 AppHandle 與 mutex 的膠水。
    use crate::settings::PauseRules;
    use crate::widgets::{
        apply_pause_reason, battery_pause_active, power_saver_pause_active, RawPowerSignals,
    };

    let day = d(2026, 10, 2);
    // (使用電池, 省電模式, 用電池時暫停, 省電時暫停, 預期暫停)
    let table = [
        (false, false, true, true, false),
        (true, false, false, false, false),
        (true, false, false, true, false),
        (true, false, true, false, true),
        (false, true, false, false, false),
        (false, true, true, false, false),
        (false, true, false, true, true),
        (true, true, false, false, false),
        (true, true, true, true, true),
    ];
    for (on_battery, power_saver, pause_on_battery, pause_on_power_saver, paused) in table {
        let raw = RawPowerSignals {
            on_battery,
            power_saver,
        };
        let rules = PauseRules {
            pause_on_battery,
            pause_on_power_saver,
        };
        let mut sim = Sim::new(TPE, WallpaperTheme::Astrolabe);
        sim.step(tpe(day, 10, 0, 0));
        apply_pause_reason(
            &mut sim.pause,
            PauseReason::Battery,
            battery_pause_active(raw, rules),
        );
        apply_pause_reason(
            &mut sim.pause,
            PauseReason::PowerSaver,
            power_saver_pause_active(raw, rules),
        );
        let decision = sim.eval(tpe(day, 10, 15, 0));
        let label = format!("{raw:?} {rules:?}");
        if paused {
            assert_eq!(decision.action, Action::Paused, "{label}");
        } else {
            assert_eq!(targets(&decision), both(RedrawReason::Scheduled), "{label}");
        }
    }
}

#[test]
fn maximized_window_is_not_a_pause() {
    // spec「視窗最大化照常更新」：一般最大化不是暫停原因（集合裡沒有對應成員），照常每 15 分鐘。
    let day = d(2026, 10, 2);
    let mut sim = Sim::new(TPE, WallpaperTheme::Astrolabe);
    let start = tpe(day, 10, 0, 0);
    assert_eq!(
        sim.poll(start, tpe(day, 10, 46, 0), 60),
        vec![
            start,
            tpe(day, 10, 15, 0),
            tpe(day, 10, 30, 0),
            tpe(day, 10, 45, 0)
        ]
    );
}

#[test]
fn resume_without_missed_slot_does_not_redraw() {
    let day = d(2026, 10, 2);
    let mut sim = Sim::new(TPE, WallpaperTheme::Astrolabe);
    sim.step(tpe(day, 10, 15, 0));
    sim.pause.insert(PauseReason::Locked);
    assert_eq!(sim.step(tpe(day, 10, 16, 0)).action, Action::Paused);
    sim.pause.clear();
    let resumed = sim.step(tpe(day, 10, 20, 0));
    assert_eq!(resumed.action, Action::UpToDate);
    assert_eq!(resumed.next_wake, Some(tpe(day, 10, 30, 0)));
}

#[test]
fn daily_theme_missed_0600_during_pause_catches_up_once() {
    let day = d(2026, 10, 2);
    let mut sim = Sim::new(TPE, WallpaperTheme::Ridgeline);
    sim.step(tpe(day, 3, 0, 0));
    sim.pause.insert(PauseReason::DisplayOff);
    assert!(sim
        .poll(tpe(day, 5, 0, 0), tpe(day, 7, 30, 0), 60)
        .is_empty());
    sim.pause.clear();
    let resumed = sim.step(tpe(day, 7, 30, 0));
    assert_eq!(plan(&resumed).stamp, Stamp::Day(day));
    assert_eq!(resumed.next_wake, Some(tpe(d(2026, 10, 3), 6, 0, 0)));
    assert!(sim
        .poll(tpe(day, 7, 30, 1), tpe(day, 23, 0, 0), 600)
        .is_empty());
}

#[test]
fn sleep_without_events_catches_up_once() {
    // 系統睡眠期間行程不執行、沒有任何事件；醒來後計時器遲到，第一次評估只補畫最近的時點。
    let day = d(2026, 10, 2);
    let mut sim = Sim::new(TPE, WallpaperTheme::Astrolabe);
    sim.step(tpe(day, 10, 0, 0));
    let woke = sim.step(tpe(day, 13, 7, 0));
    assert_eq!(targets(&woke), both(RedrawReason::Scheduled));
    assert_eq!(plan(&woke).as_of, tpe(day, 13, 0, 0));
    assert_eq!(sim.step(tpe(day, 13, 7, 1)).action, Action::UpToDate);
}

#[test]
fn monitor_changes_during_pause_wait_for_resume() {
    let day = d(2026, 10, 2);
    let mut sim = Sim::new(TPE, WallpaperTheme::Astrolabe);
    sim.step(tpe(day, 10, 0, 0));
    sim.pause.insert(PauseReason::Locked);
    sim.monitors.push(mon("C", 1920, 1080, 96));
    let paused = sim.step(tpe(day, 10, 3, 0));
    assert_eq!(paused.action, Action::Paused);
    assert!(paused.forget.is_empty());

    // 暫停中拔除的螢幕照樣從紀錄移除（不重畫）。
    sim.monitors.retain(|m| m.device_path != "B");
    let unplugged = sim.step(tpe(day, 10, 4, 0));
    assert_eq!(unplugged.action, Action::Paused);
    assert_eq!(unplugged.forget, vec!["B".to_string()]);

    sim.pause.clear();
    let resumed = sim.step(tpe(day, 10, 5, 0));
    assert_eq!(
        targets(&resumed),
        vec![("C".to_string(), RedrawReason::Unrecorded)]
    );
}

// ── 事件 ─────────────────────────────────────────────────────────────────────

#[test]
fn startup_redraws_every_monitor() {
    for theme in [
        WallpaperTheme::Astrolabe,
        WallpaperTheme::Tearoff,
        WallpaperTheme::Ridgeline,
        WallpaperTheme::Contour,
        WallpaperTheme::Skyline,
    ] {
        let sim = Sim::new(TPE, theme);
        let decision = sim.eval(tpe(d(2026, 10, 2), 10, 7, 0));
        assert_eq!(
            targets(&decision),
            both(RedrawReason::Unrecorded),
            "{theme:?}"
        );
        assert_eq!(plan(&decision).theme, theme);
        assert_eq!(
            plan(&decision)
                .targets
                .iter()
                .map(|t| t.monitor.clone())
                .collect::<Vec<_>>(),
            two_monitors(),
            "目標要帶完整的螢幕資訊、依輸入順序"
        );
    }
}

#[test]
fn theme_switch_redraws_every_monitor() {
    let day = d(2026, 10, 2);
    let switches = [
        (WallpaperTheme::Astrolabe, WallpaperTheme::Tearoff),
        (WallpaperTheme::Ridgeline, WallpaperTheme::Contour),
        (WallpaperTheme::Contour, WallpaperTheme::Ridgeline),
        (WallpaperTheme::Skyline, WallpaperTheme::Astrolabe),
        (WallpaperTheme::Tearoff, WallpaperTheme::Skyline),
    ];
    for (from, to) in switches {
        let mut sim = Sim::new(TPE, from);
        sim.step(tpe(day, 10, 0, 0));
        sim.theme = to;
        let decision = sim.step(tpe(day, 10, 3, 0));
        assert_eq!(
            targets(&decision),
            both(RedrawReason::ThemeChanged),
            "{from:?}→{to:?}"
        );
        assert_eq!(plan(&decision).theme, to);
        assert_eq!(sim.eval(tpe(day, 10, 4, 0)).action, Action::UpToDate);
    }
}

#[test]
fn monitor_events_redraw_only_the_affected_monitor() {
    let day = d(2026, 10, 2);
    let mut sim = Sim::new(TPE, WallpaperTheme::Astrolabe);
    sim.step(tpe(day, 10, 0, 0));

    // 接上新螢幕。
    sim.monitors.push(mon("C", 1920, 1080, 96));
    let plugged = sim.step(tpe(day, 10, 1, 0));
    assert_eq!(
        targets(&plugged),
        vec![("C".to_string(), RedrawReason::Unrecorded)]
    );
    assert!(plugged.forget.is_empty());

    // 拔除：只移除紀錄，其他螢幕不重畫。
    sim.monitors.retain(|m| m.device_path != "C");
    let unplugged = sim.step(tpe(day, 10, 2, 0));
    assert_eq!(unplugged.action, Action::UpToDate);
    assert_eq!(unplugged.forget, vec!["C".to_string()]);
    assert_eq!(unplugged.next_wake, Some(tpe(day, 10, 15, 0)));

    // 重新接上：當作新螢幕重畫。
    sim.monitors.push(mon("C", 1920, 1080, 96));
    let replugged = sim.step(tpe(day, 10, 3, 0));
    assert_eq!(
        targets(&replugged),
        vec![("C".to_string(), RedrawReason::Unrecorded)]
    );

    // 解析度改變。
    sim.monitors[1].geometry.width = 1920;
    sim.monitors[1].geometry.height = 1200;
    let resized = sim.step(tpe(day, 10, 4, 0));
    assert_eq!(
        targets(&resized),
        vec![("B".to_string(), RedrawReason::GeometryChanged)]
    );

    // spec「縮放比例改變」：150% → 175%。
    sim.monitors[0].geometry.dpi = 168;
    let rescaled = sim.step(tpe(day, 10, 5, 0));
    assert_eq!(
        targets(&rescaled),
        vec![("A".to_string(), RedrawReason::GeometryChanged)]
    );
    assert_eq!(sim.eval(tpe(day, 10, 6, 0)).action, Action::UpToDate);
}

// ── 失敗後的重試 ─────────────────────────────────────────────────────────────

#[test]
fn retry_delay_grows_and_is_bounded() {
    let table = [
        (0, 2 * 60),
        (1, 2 * 60),
        (2, 4 * 60),
        (3, 8 * 60),
        (4, 16 * 60),
        (5, 30 * 60),
        (6, 30 * 60),
        (100, 30 * 60),
        (u32::MAX, 30 * 60),
    ];
    for (failures, secs) in table {
        assert_eq!(retry_delay(failures), secs, "{failures}");
    }
}

#[test]
fn slot_failure_retries_only_at_retry_time_despite_unrelated_events() {
    // review M1＋複審 1 裁定：時點重畫失敗後，兩個時點之間的無關事件（全螢幕進出、資料更新）
    // 都不得讓它重畫；只有 retry_at（2 分鐘後）或真正的觸發才可以。
    let day = d(2026, 10, 2);
    let mut sim = Sim::new(TPE, WallpaperTheme::Astrolabe);
    sim.step(tpe(day, 10, 0, 0));
    sim.failing.insert("B".to_string(), FailureKind::Render);
    let failed = sim.step(tpe(day, 10, 15, 0));
    assert_eq!(targets(&failed), both(RedrawReason::Scheduled));

    let mut now = tpe(day, 10, 15, 1);
    let mut busy = false;
    while now < tpe(day, 10, 17, 0) {
        busy = !busy;
        if busy {
            sim.pause.insert(PauseReason::SystemBusy);
        } else {
            sim.pause.remove(&PauseReason::SystemBusy);
        }
        sim.data.twii_intraday = Some(d(2026, 10, 2));
        sim.data.margin = Some(d(2026, 10, 2));
        let decision = sim.step(now);
        if busy {
            assert_eq!(decision.action, Action::Paused, "at {now}");
        } else {
            assert_eq!(decision.action, Action::UpToDate, "at {now}");
            assert_eq!(decision.next_wake, Some(tpe(day, 10, 17, 0)));
        }
        now += 11;
    }

    sim.pause.clear();
    sim.failing.clear();
    let retry = sim.step(tpe(day, 10, 17, 0));
    assert_eq!(
        targets(&retry),
        vec![("B".to_string(), RedrawReason::RetryAfterFailure)]
    );
    let settled = sim.eval(tpe(day, 10, 17, 30));
    assert_eq!(settled.action, Action::UpToDate);
    assert_eq!(settled.next_wake, Some(tpe(day, 10, 30, 0)));
}

#[test]
fn tearoff_midnight_failure_retries_with_backoff() {
    // 複審 1 裁定：撕日曆 00:00 失敗＝螢幕顯示昨天的日期，要依退避曲線重試（+2、再 +4 分鐘）。
    let day = d(2026, 10, 2);
    let next = d(2026, 10, 3);
    let mut sim = Sim::new(TPE, WallpaperTheme::Tearoff);
    sim.step(tpe(day, 20, 0, 0));
    sim.failing.insert("A".to_string(), FailureKind::Render);
    let midnight = sim.step(tpe(next, 0, 0, 0));
    assert_eq!(targets(&midnight), both(RedrawReason::Scheduled));
    assert_eq!(
        sim.eval(tpe(next, 0, 0, 30)).next_wake,
        Some(tpe(next, 0, 2, 0))
    );
    assert_eq!(
        sim.poll(tpe(next, 0, 0, 1), tpe(next, 0, 2, 0), 9),
        Vec::<UnixSeconds>::new()
    );
    let first = sim.step(tpe(next, 0, 2, 0));
    assert_eq!(
        targets(&first),
        vec![("A".to_string(), RedrawReason::RetryAfterFailure)]
    );
    assert_eq!(
        sim.eval(tpe(next, 0, 2, 30)).next_wake,
        Some(tpe(next, 0, 6, 0))
    );
    sim.failing.clear();
    let second = sim.step(tpe(next, 0, 6, 0));
    assert_eq!(
        targets(&second),
        vec![("A".to_string(), RedrawReason::RetryAfterFailure)]
    );
    assert_eq!(
        sim.eval(tpe(next, 0, 6, 30)).next_wake,
        Some(tpe(d(2026, 10, 4), 0, 0, 0))
    );
}

#[test]
fn apply_failure_waits_for_next_slot() {
    // 複審 2 N1 裁定：設定桌布失敗（COM 逾時、忙碌、錯誤＝explorer 有狀況）不走退避，
    // 於下一個重畫時點再試（spec「不因 explorer 卡住而拖累宿主」、design D3）。
    let day = d(2026, 10, 2);
    let next = d(2026, 10, 3);
    let after = d(2026, 10, 4);
    let mut sim = Sim::new(TPE, WallpaperTheme::Tearoff);
    sim.step(tpe(day, 20, 0, 0));
    sim.failing.insert("A".to_string(), FailureKind::Apply);
    let midnight = sim.step(tpe(next, 0, 0, 0));
    assert_eq!(targets(&midnight), both(RedrawReason::Scheduled));
    assert_eq!(
        sim.eval(tpe(next, 0, 0, 30)).next_wake,
        Some(tpe(after, 0, 0, 0))
    );
    // 整天每分鐘評估、期間資料也在變：都不重試。
    let mut now = tpe(next, 0, 0, 1);
    while now < tpe(after, 0, 0, 0) {
        sim.data.twii_intraday = Some(next);
        assert_eq!(sim.step(now).action, Action::UpToDate, "at {now}");
        now += 60;
    }
    sim.failing.clear();
    let next_slot = sim.step(tpe(after, 0, 0, 0));
    assert_eq!(targets(&next_slot), both(RedrawReason::Scheduled));
}

/// 2026-10-06 在場驗收：explorer 有回應但設定或讀回回錯（`ApplyError`）不等下一個時點，依 `retry_delay`
/// 退避重試；等待期間 `pending_retries` 列出它（呼叫端據此不把決策記成「已是最新」）。
#[test]
fn apply_error_backs_off_and_is_listed_as_pending_retry() {
    let day = d(2026, 10, 2);
    let next = d(2026, 10, 3);
    let after = d(2026, 10, 4);
    let mut sim = Sim::new(TPE, WallpaperTheme::Tearoff);
    sim.step(tpe(day, 20, 0, 0));
    sim.failing.insert("A".to_string(), FailureKind::ApplyError);
    let midnight = sim.step(tpe(next, 0, 0, 0));
    assert_eq!(targets(&midnight), both(RedrawReason::Scheduled));

    // 第 1 次：2 分鐘後重試；期間是 UpToDate＋pending_retries，不是單純的已是最新。
    let waiting = sim.eval(tpe(next, 0, 0, 30));
    assert_eq!(waiting.action, Action::UpToDate);
    assert_eq!(waiting.next_wake, Some(tpe(next, 0, 2, 0)));
    assert_eq!(
        waiting.pending_retries,
        vec![PendingRetry {
            device_path: "A".to_string(),
            kind: FailureKind::ApplyError,
            failures: 1,
            at: tpe(next, 0, 2, 0),
        }]
    );

    // 第 2 次（重試又失敗）：間隔拉長到 4 分鐘。
    let retry = sim.step(tpe(next, 0, 2, 0));
    assert_eq!(
        targets(&retry),
        vec![("A".to_string(), RedrawReason::RetryAfterFailure)]
    );
    let waiting = sim.eval(tpe(next, 0, 2, 30));
    assert_eq!(waiting.next_wake, Some(tpe(next, 0, 6, 0)));
    assert_eq!(waiting.pending_retries[0].failures, 2);

    // 恢復：重試成功後 pending_retries 清空、計時器回到下一個時點。
    sim.failing.clear();
    sim.step(tpe(next, 0, 6, 0));
    let settled = sim.eval(tpe(next, 0, 6, 30));
    assert_eq!(settled.action, Action::UpToDate);
    assert!(settled.pending_retries.is_empty());
    assert_eq!(settled.next_wake, Some(tpe(after, 0, 0, 0)));
}

/// 設定路徑恢復事件可把退避中的 ApplyError 提早到現在，每台每個時點一次；連續失敗的間隔封頂 30 分鐘。
#[test]
fn apply_error_recovery_event_and_backoff_cap() {
    let day = d(2026, 10, 2);
    let next = d(2026, 10, 3);
    let mut sim = Sim::new(TPE, WallpaperTheme::Tearoff);
    sim.step(tpe(day, 20, 0, 0));
    sim.failing.insert("A".to_string(), FailureKind::ApplyError);
    sim.step(tpe(next, 0, 0, 0));
    // 恢復事件：00:00:10 提早重試（額度用掉）。
    assert_eq!(sim.state.apply_path_recovered(tpe(next, 0, 0, 10)), 1);
    let early = sim.step(tpe(next, 0, 0, 10));
    assert_eq!(
        targets(&early),
        vec![("A".to_string(), RedrawReason::RetryAfterFailure)]
    );
    // 又失敗（第 2 次）→ 4 分鐘；同一個時點內的恢復事件不再提早。
    assert_eq!(sim.state.apply_path_recovered(tpe(next, 0, 0, 20)), 0);
    assert_eq!(
        sim.eval(tpe(next, 0, 0, 30)).next_wake,
        Some(tpe(next, 0, 4, 10))
    );
    // 一直失敗：第 3–10 次，間隔 8、16 分鐘後封頂 30 分鐘。
    let mut attempt = tpe(next, 0, 4, 10);
    let mut gaps = Vec::new();
    for _ in 0..8 {
        sim.step(attempt);
        let retry_at = sim.eval(attempt + 1).pending_retries[0].at;
        gaps.push(retry_at - attempt);
        attempt = retry_at;
    }
    assert_eq!(
        gaps,
        vec![480, 960, 1800, 1800, 1800, 1800, 1800, 1800],
        "{gaps:?}"
    );
}

#[test]
fn render_failure_then_apply_failure_follows_apply_rule() {
    let day = d(2026, 10, 2);
    let next = d(2026, 10, 3);
    let after = d(2026, 10, 4);
    let mut sim = Sim::new(TPE, WallpaperTheme::Tearoff);
    sim.step(tpe(day, 20, 0, 0));

    // 00:00 渲染失敗 → 00:02 重試。
    sim.failing.insert("A".to_string(), FailureKind::Render);
    sim.step(tpe(next, 0, 0, 0));
    assert_eq!(
        sim.eval(tpe(next, 0, 0, 30)).next_wake,
        Some(tpe(next, 0, 2, 0))
    );

    // 00:02 重試時渲染成功、設定失敗 → 依設定失敗的規則，等下一個時點。
    sim.failing.insert("A".to_string(), FailureKind::Apply);
    let retry = sim.step(tpe(next, 0, 2, 0));
    assert_eq!(
        targets(&retry),
        vec![("A".to_string(), RedrawReason::RetryAfterFailure)]
    );
    assert_eq!(
        sim.eval(tpe(next, 0, 2, 30)).next_wake,
        Some(tpe(after, 0, 0, 0))
    );
    assert!(sim
        .poll(tpe(next, 0, 2, 1), tpe(next, 3, 0, 0), 60)
        .is_empty());

    // 設定失敗代表渲染成功：連續渲染失敗次數歸零，下一次渲染失敗又從 2 分鐘起算。
    sim.failing.insert("A".to_string(), FailureKind::Render);
    sim.step(tpe(after, 0, 0, 0));
    assert_eq!(
        sim.eval(tpe(after, 0, 0, 30)).next_wake,
        Some(tpe(after, 0, 2, 0))
    );
}

#[test]
fn reset_after_restore_survives_coalesced_none_flip() {
    // 複審 2 N2：Redraw 執行期間使用者切「不接管」→ 4.4 還原 → 又切回脊線；4.7 把期間的喚醒
    // 合併成一次評估，從未以「不接管」評估，`Idle` 不會出現。
    let day = d(2026, 10, 2);
    let mut sim = Sim::new(TPE, WallpaperTheme::Ridgeline);
    sim.step(tpe(day, 10, 0, 0));
    // 沒有 reset：紀錄仍是脊線、同一天 → UpToDate，螢幕停在原桌布到明天 06:00（這就是要防的事）。
    assert_eq!(sim.eval(tpe(day, 10, 1, 0)).action, Action::UpToDate);

    // 4.4 還原後呼叫 reset：每台都重畫。
    sim.state.reset();
    let redraw = sim.step(tpe(day, 10, 1, 0));
    assert_eq!(targets(&redraw), both(RedrawReason::Unrecorded));

    // reset 也清掉「本次執行已接管」：還原（或讓位）後缺資料＝從未接管，等待資料。
    let mut sim = Sim::new(TPE, WallpaperTheme::Ridgeline);
    sim.step(tpe(day, 10, 0, 0));
    sim.state.reset();
    sim.data = DataDates::default();
    assert_eq!(
        sim.eval(tpe(day, 10, 1, 0)).action,
        Action::WaitingForData {
            missing: DataKey::TwiiIntraday
        }
    );
}

#[test]
fn astrolabe_retry_runs_before_slot_or_yields_to_it() {
    // 退避比下一個時點短 → 先重試；比較長 → 讓給時點（時點本身就會重畫這台）。
    let day = d(2026, 10, 2);
    let mut sim = Sim::new(TPE, WallpaperTheme::Astrolabe);
    sim.failing.insert("B".to_string(), FailureKind::Render);
    // B 從 10:00 起連續失敗：10:02、10:06、10:14 重試（2、4、8 分鐘）；第 4 次失敗後的 16 分鐘
    // （10:30）晚於 10:15 時點 → 下一次是 10:15 時點；之後 30 分鐘的退避也讓給 10:30、10:45。
    let drawn = sim.follow_timer(tpe(day, 10, 0, 0), tpe(day, 10, 46, 0));
    assert_eq!(
        drawn,
        vec![
            tpe(day, 10, 0, 0),
            tpe(day, 10, 2, 0),
            tpe(day, 10, 6, 0),
            tpe(day, 10, 14, 0),
            tpe(day, 10, 15, 0),
            tpe(day, 10, 30, 0),
            tpe(day, 10, 45, 0),
        ]
    );
    // 讓位的時刻：10:14 失敗後下一次是 10:15 時點，不是 10:30 的重試。
    let mut check = Sim::new(TPE, WallpaperTheme::Astrolabe);
    check.failing.insert("B".to_string(), FailureKind::Render);
    for t in [
        tpe(day, 10, 0, 0),
        tpe(day, 10, 2, 0),
        tpe(day, 10, 6, 0),
        tpe(day, 10, 14, 0),
    ] {
        check.step(t);
    }
    assert_eq!(
        check.eval(tpe(day, 10, 14, 30)).next_wake,
        Some(tpe(day, 10, 15, 0))
    );
    check.step(tpe(day, 10, 15, 0));
    assert_eq!(
        check.eval(tpe(day, 10, 15, 30)).next_wake,
        Some(tpe(day, 10, 30, 0)),
        "第 5 次失敗退避 30 分鐘（10:45）晚於 10:30 時點"
    );
}

#[test]
fn first_takeover_failure_retries_with_growing_delay() {
    // 首次接管失敗：脊線的下一個時點是明天 06:00，不能等一整天——retry_at 併進 next_wake。
    let day = d(2026, 10, 2);
    let tomorrow = d(2026, 10, 3);
    let mut sim = Sim::new(TPE, WallpaperTheme::Ridgeline);
    sim.failing.insert("B".to_string(), FailureKind::Render);
    let start = sim.step(tpe(day, 10, 0, 0));
    assert_eq!(targets(&start), both(RedrawReason::Unrecorded));

    let after = sim.eval(tpe(day, 10, 0, 30));
    assert_eq!(after.action, Action::UpToDate);
    assert_eq!(after.next_wake, Some(tpe(day, 10, 2, 0)));

    // 重試時刻之前，無關事件不觸發。
    let mut now = tpe(day, 10, 0, 1);
    while now < tpe(day, 10, 2, 0) {
        sim.data.margin = Some(d(2026, 10, 2));
        sim.data.twii_daily = Some(d(2026, 10, 2));
        assert_eq!(sim.step(now).action, Action::UpToDate, "at {now}");
        now += 7;
    }

    // 只重試失敗的那台，間隔 2、4、8、16、30、30 分鐘。
    let retry = sim.eval(tpe(day, 10, 2, 0));
    assert_eq!(
        targets(&retry),
        vec![("B".to_string(), RedrawReason::RetryAfterFailure)]
    );
    assert_eq!(
        sim.follow_timer(tpe(day, 10, 0, 30), tpe(day, 11, 31, 0)),
        vec![
            tpe(day, 10, 2, 0),
            tpe(day, 10, 6, 0),
            tpe(day, 10, 14, 0),
            tpe(day, 10, 30, 0),
            tpe(day, 11, 0, 0),
            tpe(day, 11, 30, 0),
        ]
    );

    // 成功之後回到正常節奏：下一次是明天 06:00。
    sim.failing.clear();
    let ok = sim.step(tpe(day, 12, 0, 0));
    assert_eq!(
        targets(&ok),
        vec![("B".to_string(), RedrawReason::RetryAfterFailure)]
    );
    assert_eq!(ok.next_wake, Some(tpe(tomorrow, 6, 0, 0)));
    assert_eq!(
        sim.follow_timer(tpe(day, 12, 0, 1), tpe(tomorrow, 6, 0, 1)),
        vec![tpe(tomorrow, 6, 0, 0)]
    );
}

#[test]
fn consecutive_failures_keep_growing_across_slots() {
    // 連續失敗次數跨時點累計：首次接管失敗（10:16 重試）前時點 10:15 先到、又失敗 → 第 2 次，
    // 退避 4 分鐘。
    let day = d(2026, 10, 2);
    let mut sim = Sim::new(TPE, WallpaperTheme::Astrolabe);
    sim.failing.insert("B".to_string(), FailureKind::Render);
    sim.step(tpe(day, 10, 14, 0));
    assert_eq!(
        sim.eval(tpe(day, 10, 14, 30)).next_wake,
        Some(tpe(day, 10, 15, 0)),
        "時點 10:15 比重試 10:16 早"
    );
    let slot = sim.step(tpe(day, 10, 15, 0));
    assert_eq!(targets(&slot), both(RedrawReason::Scheduled));
    // 連續第二次失敗：10:15 + 4 分鐘，早於下一個時點 10:30。
    assert_eq!(
        sim.eval(tpe(day, 10, 15, 30)).next_wake,
        Some(tpe(day, 10, 19, 0))
    );
}

#[test]
fn failed_monitor_retargets_on_theme_or_geometry_change() {
    let day = d(2026, 10, 2);
    let mut sim = Sim::new(TPE, WallpaperTheme::Astrolabe);
    sim.step(tpe(day, 10, 0, 0));
    sim.failing.insert("B".to_string(), FailureKind::Render);
    sim.step(tpe(day, 10, 15, 0));

    // 縮放比例改變：立即重畫那台（不等時點）。
    sim.monitors[1].geometry.dpi = 120;
    let rescaled = sim.step(tpe(day, 10, 20, 0));
    assert_eq!(
        targets(&rescaled),
        vec![("B".to_string(), RedrawReason::GeometryChanged)]
    );
    // 出圖條件改變後又失敗＝這台沒有正確的圖 → 提早重試。
    assert_eq!(
        sim.eval(tpe(day, 10, 20, 30)).next_wake,
        Some(tpe(day, 10, 24, 0))
    );

    // 換主題：兩台都重畫。
    sim.theme = WallpaperTheme::Tearoff;
    let switched = sim.step(tpe(day, 10, 21, 0));
    assert_eq!(targets(&switched), both(RedrawReason::ThemeChanged));
}

#[test]
fn data_driven_failure_retries_early_and_retargets_on_newer_data() {
    // 天際線沒有時間觸發；首次接管失敗仍要有 retry_at，資料變新則立即重畫。
    let day = d(2026, 10, 2);
    let mut sim = Sim::new(TPE, WallpaperTheme::Skyline);
    sim.failing.insert("A".to_string(), FailureKind::Render);
    sim.failing.insert("B".to_string(), FailureKind::Render);
    sim.step(tpe(day, 10, 0, 0));
    let after = sim.eval(tpe(day, 10, 0, 30));
    assert_eq!(after.action, Action::UpToDate);
    assert_eq!(after.next_wake, Some(tpe(day, 10, 2, 0)));

    sim.failing.clear();
    sim.data.twii_daily = Some(day);
    let newer = sim.step(tpe(day, 10, 1, 0));
    assert_eq!(targets(&newer), both(RedrawReason::Scheduled));
    let settled = sim.eval(tpe(day, 10, 1, 30));
    assert_eq!(settled.action, Action::UpToDate);
    assert_eq!(settled.next_wake, None);
}

// ── 設定路徑恢復事件 ─────────────────────────────────────────────────────────

#[test]
fn apply_path_recovered_retries_only_apply_failed_monitor_once() {
    // 首次接管時 A 設定失敗（explorer 卡住）→ 依規則等明天 06:00；4.7 偵測到 COM 執行緒恢復
    // → 只有 A 重畫一次。
    let day = d(2026, 10, 2);
    let mut sim = Sim::new(TPE, WallpaperTheme::Ridgeline);
    sim.failing.insert("A".to_string(), FailureKind::Apply);
    let start = sim.step(tpe(day, 10, 0, 0));
    assert_eq!(targets(&start), both(RedrawReason::Unrecorded));
    let waiting = sim.eval(tpe(day, 10, 5, 0));
    assert_eq!(waiting.action, Action::UpToDate);
    assert_eq!(waiting.next_wake, Some(tpe(d(2026, 10, 3), 6, 0, 0)));

    sim.failing.clear();
    assert_eq!(sim.state.apply_path_recovered(tpe(day, 10, 6, 0)), 1);
    let retry = sim.step(tpe(day, 10, 6, 0));
    assert_eq!(
        targets(&retry),
        vec![("A".to_string(), RedrawReason::RetryAfterFailure)]
    );
    assert_eq!(sim.eval(tpe(day, 10, 6, 30)).action, Action::UpToDate);
}

#[test]
fn apply_path_recovered_without_apply_failures_changes_nothing() {
    let day = d(2026, 10, 2);
    // 全部成功：沒有東西要重試。
    let mut sim = Sim::new(TPE, WallpaperTheme::Astrolabe);
    sim.step(tpe(day, 10, 0, 0));
    assert_eq!(sim.state.apply_path_recovered(tpe(day, 10, 1, 0)), 0);
    let decision = sim.eval(tpe(day, 10, 1, 0));
    assert_eq!(decision.action, Action::UpToDate);
    assert_eq!(decision.next_wake, Some(tpe(day, 10, 15, 0)));

    // 渲染失敗的螢幕維持自己的退避（10:02），不因恢復事件提早。
    let mut sim = Sim::new(TPE, WallpaperTheme::Astrolabe);
    sim.failing.insert("B".to_string(), FailureKind::Render);
    sim.step(tpe(day, 10, 0, 0));
    assert_eq!(sim.state.apply_path_recovered(tpe(day, 10, 1, 0)), 0);
    let decision = sim.eval(tpe(day, 10, 1, 0));
    assert_eq!(decision.action, Action::UpToDate);
    assert_eq!(decision.next_wake, Some(tpe(day, 10, 2, 0)));
}

#[test]
fn apply_failure_after_recovery_waits_for_next_slot() {
    let day = d(2026, 10, 2);
    let next = d(2026, 10, 3);
    let after = d(2026, 10, 4);
    let mut sim = Sim::new(TPE, WallpaperTheme::Tearoff);
    sim.step(tpe(day, 20, 0, 0));
    sim.failing.insert("A".to_string(), FailureKind::Apply);
    sim.step(tpe(next, 0, 0, 0));

    // 恢復 → 重試一次，又是設定失敗（explorer 又卡住）。
    assert_eq!(sim.state.apply_path_recovered(tpe(next, 0, 10, 0)), 1);
    let retry = sim.step(tpe(next, 0, 10, 0));
    assert_eq!(
        targets(&retry),
        vec![("A".to_string(), RedrawReason::RetryAfterFailure)]
    );
    // 之後不再重試，直到下一個時點……
    assert_eq!(
        sim.eval(tpe(next, 0, 10, 30)).next_wake,
        Some(tpe(after, 0, 0, 0))
    );
    assert!(sim
        .poll(tpe(next, 0, 10, 1), tpe(next, 6, 0, 0), 60)
        .is_empty());
    // ……同一個時點內的恢復事件也不再重試（修正輪 4 M1：每台每個時點最多一次恢復重試）。
    sim.failing.clear();
    assert_eq!(sim.state.apply_path_recovered(tpe(next, 6, 0, 0)), 0);
    assert_eq!(sim.eval(tpe(next, 6, 0, 0)).action, Action::UpToDate);
    // 下一個時點照常重畫；成功後恢復事件沒有東西可重試。
    let next_slot = sim.step(tpe(after, 0, 0, 0));
    assert_eq!(targets(&next_slot), both(RedrawReason::Scheduled));
    assert_eq!(sim.state.apply_path_recovered(tpe(after, 0, 1, 0)), 0);
    assert_eq!(sim.eval(tpe(after, 0, 1, 0)).action, Action::UpToDate);
}

/// 修正輪 4 M1：explorer「逾時後才返回」反覆發生——每次設定都逾時（記 `Apply`）、之後忙碌旗標
/// 解除（恢復事件）。每台每個時點最多一次恢復重試，不形成無上限的「渲染 → 逾時 → 恢復」循環。
#[test]
fn recovery_retry_at_most_once_per_slot_under_timeout_return_cycle() {
    let day = d(2026, 10, 2);
    let next = d(2026, 10, 3);
    let after = d(2026, 10, 4);
    let mut sim = Sim::new(TPE, WallpaperTheme::Tearoff);
    sim.step(tpe(day, 20, 0, 0));
    sim.failing.insert("A".to_string(), FailureKind::Apply);

    // 每 30 秒一圈「逾時後返回 → 恢復事件 → 評估」，回傳（重新排入的台數合計、渲染的台數合計）。
    let cycle = |sim: &mut Sim, from: UnixSeconds, until: UnixSeconds| -> (usize, usize) {
        let (mut granted, mut renders) = (0, 0);
        let mut now = from;
        while now < until {
            granted += sim.state.apply_path_recovered(now);
            if let Action::Redraw(plan) = &sim.step(now).action {
                renders += plan.targets.len();
            }
            now += 30;
        }
        (granted, renders)
    };

    // 一個時點內：時點本身 1 次＋恢復重試 1 次，之後的恢復事件都不再渲染。
    let slot = sim.step(tpe(next, 0, 0, 0));
    assert_eq!(targets(&slot), both(RedrawReason::Scheduled));
    assert_eq!(
        cycle(&mut sim, tpe(next, 0, 0, 30), tpe(next, 3, 0, 0)),
        (1, 1),
        "同一個時點只該有一次恢復重試"
    );
    assert_eq!(
        sim.eval(tpe(next, 3, 0, 0)).next_wake,
        Some(tpe(after, 0, 0, 0))
    );

    // 下一個時點：重新有一次恢復重試的額度。
    let slot = sim.step(tpe(after, 0, 0, 0));
    assert_eq!(targets(&slot), both(RedrawReason::Scheduled));
    assert_eq!(
        cycle(&mut sim, tpe(after, 0, 0, 30), tpe(after, 1, 0, 0)),
        (1, 1)
    );

    // 解析度／DPI 改變（新的出圖條件）：也重新有一次額度。
    sim.monitors[0].geometry.dpi = 168;
    let changed = sim.step(tpe(after, 1, 0, 0));
    assert_eq!(
        targets(&changed),
        vec![("A".to_string(), RedrawReason::GeometryChanged)]
    );
    assert_eq!(
        cycle(&mut sim, tpe(after, 1, 0, 30), tpe(after, 2, 0, 0)),
        (1, 1)
    );

    // 換主題：也重新有一次額度。
    sim.theme = WallpaperTheme::Astrolabe;
    let switched = sim.step(tpe(after, 2, 0, 0));
    assert_eq!(targets(&switched), both(RedrawReason::ThemeChanged));
    assert_eq!(
        cycle(&mut sim, tpe(after, 2, 0, 30), tpe(after, 2, 14, 30)),
        (1, 1)
    );
}

/// 修正輪 4 M1：恢復重試變成渲染失敗、退避重試後才又設定失敗——同一個時點內的恢復額度已用掉。
#[test]
fn recovery_quota_survives_render_failure_within_the_slot() {
    let day = d(2026, 10, 2);
    let next = d(2026, 10, 3);
    let mut sim = Sim::new(TPE, WallpaperTheme::Tearoff);
    sim.step(tpe(day, 20, 0, 0));
    sim.failing.insert("A".to_string(), FailureKind::Apply);
    sim.step(tpe(next, 0, 0, 0));

    assert_eq!(sim.state.apply_path_recovered(tpe(next, 0, 1, 0)), 1);
    sim.failing.insert("A".to_string(), FailureKind::Render);
    let retry = sim.step(tpe(next, 0, 1, 0));
    assert_eq!(
        targets(&retry),
        vec![("A".to_string(), RedrawReason::RetryAfterFailure)]
    );
    // 渲染失敗照退避（+2 分鐘）重試；這次是設定失敗。
    sim.failing.insert("A".to_string(), FailureKind::Apply);
    let backoff = sim.step(tpe(next, 0, 3, 0));
    assert_eq!(
        targets(&backoff),
        vec![("A".to_string(), RedrawReason::RetryAfterFailure)]
    );
    sim.failing.clear();
    assert_eq!(sim.state.apply_path_recovered(tpe(next, 0, 4, 0)), 0);
    assert_eq!(sim.eval(tpe(next, 0, 4, 0)).action, Action::UpToDate);
}

// ── busy 預檢跳過（FailureKind::Busy） ─────────────────────────────────────────

/// 修正輪 4 L1：busy 預檢跳過沒有渲染，不得把連續渲染失敗次數歸零；時點語意與 `Apply` 相同。
#[test]
fn busy_skip_keeps_render_failure_count() {
    let day = d(2026, 10, 2);
    let next = d(2026, 10, 3);
    let after = d(2026, 10, 4);
    let mut sim = Sim::new(TPE, WallpaperTheme::Tearoff);
    sim.step(tpe(day, 20, 0, 0));

    // 00:00 渲染失敗（第 1 次，+2 分）、00:02 再失敗（第 2 次，+4 分 → 00:06）。
    sim.failing.insert("A".to_string(), FailureKind::Render);
    sim.step(tpe(next, 0, 0, 0));
    sim.step(tpe(next, 0, 2, 0));
    assert_eq!(
        sim.eval(tpe(next, 0, 2, 30)).next_wake,
        Some(tpe(next, 0, 6, 0))
    );

    // 00:06 重試時 COM 忙碌：預檢跳過。等下一個時點（與 Apply 相同），不在時點之間重試。
    sim.failing.insert("A".to_string(), FailureKind::Busy);
    let skipped = sim.step(tpe(next, 0, 6, 0));
    assert_eq!(
        targets(&skipped),
        vec![("A".to_string(), RedrawReason::RetryAfterFailure)]
    );
    assert_eq!(
        sim.eval(tpe(next, 0, 6, 30)).next_wake,
        Some(tpe(after, 0, 0, 0))
    );
    assert!(sim
        .poll(tpe(next, 0, 6, 1), tpe(next, 2, 0, 0), 60)
        .is_empty());

    // 恢復事件 → 重試又渲染失敗：這是第 3 次連續渲染失敗（+8 分），不是從 2 分鐘重新起算。
    assert_eq!(sim.state.apply_path_recovered(tpe(next, 2, 0, 0)), 1);
    sim.failing.insert("A".to_string(), FailureKind::Render);
    sim.step(tpe(next, 2, 0, 0));
    assert_eq!(
        sim.eval(tpe(next, 2, 0, 30)).next_wake,
        Some(tpe(next, 2, 8, 0))
    );
}

/// 修正輪 4 L2：連續 busy 預檢跳過跨了幾個時點（同一個時點的多台、恢復重試只算一次）；任何一台
/// 通過預檢（成功、渲染失敗或設定失敗）就歸零，`reset` 也歸零。
#[test]
fn consecutive_busy_skip_slots_counts_distinct_slots() {
    let day = d(2026, 10, 2);
    let mut sim = Sim::new(TPE, WallpaperTheme::Astrolabe);
    assert_eq!(sim.state.consecutive_busy_skip_slots(), 0);
    sim.failing.insert("A".to_string(), FailureKind::Busy);
    sim.failing.insert("B".to_string(), FailureKind::Busy);

    sim.step(tpe(day, 10, 0, 0));
    assert_eq!(
        sim.state.consecutive_busy_skip_slots(),
        1,
        "兩台同一個時點只算一次"
    );
    assert_eq!(sim.state.apply_path_recovered(tpe(day, 10, 5, 0)), 2);
    sim.step(tpe(day, 10, 5, 0));
    assert_eq!(
        sim.state.consecutive_busy_skip_slots(),
        1,
        "恢復重試仍是同一個時點"
    );
    sim.step(tpe(day, 10, 15, 0));
    assert_eq!(sim.state.consecutive_busy_skip_slots(), 2);
    sim.step(tpe(day, 10, 30, 0));
    assert_eq!(
        sim.state.consecutive_busy_skip_slots(),
        BUSY_SKIP_WARN_SLOTS,
        "跨 3 個時點：4.7 該記一行警告"
    );

    // 設定失敗也代表通過了預檢（API 有呼叫）：歸零。
    sim.failing.insert("A".to_string(), FailureKind::Apply);
    sim.failing.insert("B".to_string(), FailureKind::Apply);
    sim.step(tpe(day, 10, 45, 0));
    assert_eq!(sim.state.consecutive_busy_skip_slots(), 0);

    // 渲染失敗同理。
    sim.failing.insert("A".to_string(), FailureKind::Busy);
    sim.failing.insert("B".to_string(), FailureKind::Busy);
    sim.step(tpe(day, 11, 0, 0));
    assert_eq!(sim.state.consecutive_busy_skip_slots(), 1);
    sim.failing.insert("A".to_string(), FailureKind::Render);
    sim.failing.insert("B".to_string(), FailureKind::Render);
    sim.step(tpe(day, 11, 15, 0));
    assert_eq!(sim.state.consecutive_busy_skip_slots(), 0);

    // 又開始跳過 → 1；reset 歸零。
    sim.failing.insert("A".to_string(), FailureKind::Busy);
    sim.failing.insert("B".to_string(), FailureKind::Busy);
    sim.step(tpe(day, 11, 30, 0));
    assert_eq!(sim.state.consecutive_busy_skip_slots(), 1);
    sim.state.reset();
    assert_eq!(sim.state.consecutive_busy_skip_slots(), 0);

    // 成功也歸零。
    sim.step(tpe(day, 11, 45, 0));
    assert_eq!(sim.state.consecutive_busy_skip_slots(), 1);
    sim.failing.clear();
    sim.step(tpe(day, 12, 0, 0));
    assert_eq!(sim.state.consecutive_busy_skip_slots(), 0);
}

// ── reset 與進行中的 Redraw（世代） ──────────────────────────────────────────

/// 修正輪 4 L3：`reset` 之前做出的 `Redraw`，之後才回報的結果一律忽略，不把紀錄與「本次執行
/// 已接管」寫回去。
#[test]
fn stale_records_after_reset_are_ignored() {
    let day = d(2026, 10, 2);
    let mut sim = Sim::new(TPE, WallpaperTheme::Ridgeline);
    let in_flight = sim.eval(tpe(day, 10, 0, 0));
    let stale = plan(&in_flight).clone();

    // 4.4 還原並 reset 之後，進行中的 Redraw 才回報結果。
    sim.state.reset();
    assert!(!sim.state.record_drawn(&stale, &stale.targets[0].monitor));
    assert!(!sim.state.record_failed(
        &stale,
        &stale.targets[1],
        FailureKind::Render,
        tpe(day, 10, 0, 20)
    ));

    // 紀錄沒有被寫回：每台仍是沒有紀錄；也沒有算成已接管（缺資料時仍是等待資料）。
    let after = sim.eval(tpe(day, 10, 1, 0));
    assert_eq!(targets(&after), both(RedrawReason::Unrecorded));
    assert_eq!(after.next_wake, Some(tpe(d(2026, 10, 3), 6, 0, 0)));
    let saved = sim.data;
    sim.data = DataDates::default();
    assert_eq!(
        sim.eval(tpe(day, 10, 1, 0)).action,
        Action::WaitingForData {
            missing: DataKey::TwiiIntraday
        }
    );
    sim.data = saved;

    // reset 之後做出的新計畫照常記錄。
    let fresh = sim.eval(tpe(day, 10, 1, 0));
    let fresh = plan(&fresh).clone();
    assert!(sim.state.record_drawn(&fresh, &fresh.targets[0].monitor));
    assert!(sim.state.record_failed(
        &fresh,
        &fresh.targets[1],
        FailureKind::Apply,
        tpe(day, 10, 1, 20)
    ));
    assert_eq!(sim.eval(tpe(day, 10, 2, 0)).action, Action::UpToDate);

    // 再 reset 一次：前一個世代的計畫也成了過時的。
    sim.state.reset();
    assert!(!sim.state.record_drawn(&fresh, &fresh.targets[0].monitor));
    assert_eq!(
        targets(&sim.eval(tpe(day, 10, 3, 0))),
        both(RedrawReason::Unrecorded)
    );
}

// ── 接管後資料消失 ───────────────────────────────────────────────────────────

#[test]
fn missing_data_after_takeover_holds_current_wallpaper() {
    let day = d(2026, 10, 2);
    let cases = [
        (WallpaperTheme::Ridgeline, DataKey::TwiiIntraday),
        (WallpaperTheme::Contour, DataKey::TwiiIntraday),
        (WallpaperTheme::Skyline, DataKey::TwiiDaily),
    ];
    for (theme, missing) in cases {
        let mut sim = Sim::new(TPE, theme);
        sim.step(tpe(day, 10, 0, 0));
        sim.data = DataDates::default();
        let decision = sim.step(tpe(day, 10, 5, 0));
        assert_eq!(
            decision.action,
            Action::HoldWithoutData { missing },
            "{theme:?}"
        );
        assert_eq!(decision.next_wake, None, "{theme:?}");
        assert!(decision.forget.is_empty(), "{theme:?}");

        // 資料回來：圖仍是對的，不必重畫。
        sim.data = data_all(d(2026, 10, 1));
        assert_eq!(
            sim.eval(tpe(day, 10, 6, 0)).action,
            Action::UpToDate,
            "{theme:?}"
        );
    }

    // 已接管（星盤）後切到缺資料的主題：保留目前的星盤圖，不是「等待資料」。
    let mut sim = Sim::new(TPE, WallpaperTheme::Astrolabe);
    sim.step(tpe(day, 10, 0, 0));
    sim.data = DataDates::default();
    sim.theme = WallpaperTheme::Skyline;
    assert_eq!(
        sim.eval(tpe(day, 10, 1, 0)).action,
        Action::HoldWithoutData {
            missing: DataKey::TwiiDaily
        }
    );
}

#[test]
fn takeover_flag_distinguishes_hold_from_waiting_after_restart() {
    // 重啟後排程器狀態是空的，靠 4.4 的「已接管」旗標分辨。
    let day = d(2026, 10, 2);
    for (taken_over, want) in [
        (
            true,
            Action::HoldWithoutData {
                missing: DataKey::TwiiIntraday,
            },
        ),
        (
            false,
            Action::WaitingForData {
                missing: DataKey::TwiiIntraday,
            },
        ),
    ] {
        let mut sim = Sim::new(TPE, WallpaperTheme::Ridgeline);
        sim.already_taken_over = taken_over;
        sim.data = DataDates::default();
        assert_eq!(sim.eval(tpe(day, 10, 0, 0)).action, want, "{taken_over}");
    }
}

#[test]
fn only_failed_attempts_do_not_count_as_takeover() {
    let day = d(2026, 10, 2);
    let mut sim = Sim::new(TPE, WallpaperTheme::Ridgeline);
    sim.failing.insert("A".to_string(), FailureKind::Render);
    sim.failing.insert("B".to_string(), FailureKind::Render);
    sim.step(tpe(day, 10, 0, 0));
    sim.data = DataDates::default();
    assert_eq!(
        sim.eval(tpe(day, 10, 1, 0)).action,
        Action::WaitingForData {
            missing: DataKey::TwiiIntraday
        }
    );
}

#[test]
fn success_then_failure_still_counts_as_takeover_until_idle() {
    // 複審 1 N1：曾經成功過，之後的失敗不會讓它變回「從未接管」；切到「不接管」才清掉。
    let day = d(2026, 10, 2);
    let mut sim = Sim::new(TPE, WallpaperTheme::Ridgeline);
    sim.step(tpe(day, 10, 0, 0));
    sim.failing.insert("A".to_string(), FailureKind::Render);
    sim.failing.insert("B".to_string(), FailureKind::Render);
    let failed = sim.step(tpe(d(2026, 10, 3), 6, 0, 0));
    assert_eq!(targets(&failed), both(RedrawReason::Scheduled));
    sim.data = DataDates::default();
    assert!(!sim.already_taken_over);
    assert_eq!(
        sim.eval(tpe(d(2026, 10, 3), 6, 1, 0)).action,
        Action::HoldWithoutData {
            missing: DataKey::TwiiIntraday
        }
    );

    // 不接管（4.4 還原原桌布）後切回脊線、仍缺資料 → 從未接管的狀態，等待資料。
    sim.theme = WallpaperTheme::None;
    assert_eq!(sim.step(tpe(d(2026, 10, 3), 6, 2, 0)).action, Action::Idle);
    sim.theme = WallpaperTheme::Ridgeline;
    assert_eq!(
        sim.eval(tpe(d(2026, 10, 3), 6, 3, 0)).action,
        Action::WaitingForData {
            missing: DataKey::TwiiIntraday
        }
    );
}

#[test]
fn no_online_monitor_is_up_to_date() {
    let mut sim = Sim::new(TPE, WallpaperTheme::Astrolabe);
    sim.monitors.clear();
    let decision = sim.eval(tpe(d(2026, 10, 2), 10, 7, 0));
    assert_eq!(decision.action, Action::UpToDate);
    assert_eq!(decision.next_wake, Some(tpe(d(2026, 10, 2), 10, 15, 0)));
}

// ── 夏令時間 ─────────────────────────────────────────────────────────────────

#[test]
fn daily_0600_once_per_day_across_dst_transitions() {
    // 06:00 本機：夏令前 +01:00＝05:00Z、夏令中 +02:00＝04:00Z。
    let spring = (
        utc(d(2026, 3, 27), 0, 0, 0),
        utc(d(2026, 3, 31), 0, 0, 0),
        vec![
            utc(d(2026, 3, 27), 0, 0, 0),
            utc(d(2026, 3, 27), 5, 0, 0),
            utc(d(2026, 3, 28), 5, 0, 0),
            utc(d(2026, 3, 29), 4, 0, 0),
            utc(d(2026, 3, 30), 4, 0, 0),
        ],
    );
    let autumn = (
        utc(d(2026, 10, 23), 0, 0, 0),
        utc(d(2026, 10, 27), 0, 0, 0),
        vec![
            utc(d(2026, 10, 23), 0, 0, 0),
            utc(d(2026, 10, 23), 4, 0, 0),
            utc(d(2026, 10, 24), 4, 0, 0),
            utc(d(2026, 10, 25), 5, 0, 0),
            utc(d(2026, 10, 26), 5, 0, 0),
        ],
    );
    for (from, until, expected) in [spring, autumn] {
        for theme in [WallpaperTheme::Ridgeline, WallpaperTheme::Contour] {
            assert_eq!(
                Sim::new(berlin_2026(), theme).follow_timer(from, until),
                expected,
                "{theme:?}"
            );
            assert_eq!(
                Sim::new(berlin_2026(), theme).poll(from, until, 60),
                expected,
                "{theme:?}"
            );
        }
    }
}

#[test]
fn next_wake_is_exact_across_spring_forward() {
    // 切換（01:00Z）之前算的 06:00 喚醒，要用切換後的偏移：04:00Z，不是 05:00Z。
    let mut sim = Sim::new(berlin_2026(), WallpaperTheme::Ridgeline);
    sim.step(utc(d(2026, 3, 28), 23, 0, 0));
    let decision = sim.eval(utc(d(2026, 3, 29), 0, 30, 0));
    assert_eq!(decision.action, Action::UpToDate);
    assert_eq!(decision.next_wake, Some(utc(d(2026, 3, 29), 4, 0, 0)));
}

#[test]
fn astrolabe_every_quarter_across_dst_transitions() {
    for day in [d(2026, 3, 29), d(2026, 10, 25)] {
        let from = utc(day, 0, 0, 0) - 2 * 3600;
        let until = utc(day, 4, 0, 0);
        let expected: Vec<_> = (0..24).map(|i| from + i * ASTROLABE_SLOT_SECS).collect();
        assert_eq!(
            Sim::new(berlin_2026(), WallpaperTheme::Astrolabe).follow_timer(from, until),
            expected,
            "{day:?}"
        );
        assert_eq!(
            Sim::new(berlin_2026(), WallpaperTheme::Astrolabe).poll(from, until, 60),
            expected,
            "{day:?}"
        );
    }
}

#[test]
fn tearoff_midnight_across_dst_transitions() {
    // 本機 00:00：3/29（+01:00）＝3/28 23:00Z；3/30（+02:00）＝3/29 22:00Z；
    // 10/25（+02:00）＝10/24 22:00Z；10/26（+01:00）＝10/25 23:00Z。
    let cases = [
        (
            utc(d(2026, 3, 28), 12, 0, 0),
            utc(d(2026, 3, 30), 12, 0, 0),
            vec![
                utc(d(2026, 3, 28), 12, 0, 0),
                utc(d(2026, 3, 28), 23, 0, 0),
                utc(d(2026, 3, 29), 22, 0, 0),
            ],
        ),
        (
            utc(d(2026, 10, 24), 12, 0, 0),
            utc(d(2026, 10, 26), 12, 0, 0),
            vec![
                utc(d(2026, 10, 24), 12, 0, 0),
                utc(d(2026, 10, 24), 22, 0, 0),
                utc(d(2026, 10, 25), 23, 0, 0),
            ],
        ),
    ];
    for (from, until, expected) in cases {
        assert_eq!(
            Sim::new(berlin_2026(), WallpaperTheme::Tearoff).follow_timer(from, until),
            expected
        );
        assert_eq!(
            Sim::new(berlin_2026(), WallpaperTheme::Tearoff).poll(from, until, 60),
            expected
        );
    }
}

#[test]
fn local_time_going_back_over_trigger_does_not_redraw_twice() {
    // 合成時區：本機時間倒退時跨過觸發點（06:30 → 05:30、00:30 → 前一天 23:30），
    // 同一天的 06:00／換日不得畫兩次。
    let day = d(2026, 10, 2);
    let back_over_0600 = || Dst {
        std: 0,
        dst: 3600,
        start: i64::MIN,
        end: utc(day, 5, 30, 0), // 本機 06:30（+01:00）→ 05:30（+00:00）
    };
    let from = utc(day, 0, 0, 0);
    let until = utc(d(2026, 10, 3), 12, 0, 0);
    let expected = vec![from, utc(day, 5, 0, 0), utc(d(2026, 10, 3), 6, 0, 0)];
    for theme in [WallpaperTheme::Ridgeline, WallpaperTheme::Contour] {
        assert_eq!(
            Sim::new(back_over_0600(), theme).poll(from, until, 60),
            expected,
            "{theme:?}"
        );
        assert_eq!(
            Sim::new(back_over_0600(), theme).follow_timer(from, until),
            expected,
            "{theme:?}"
        );
    }

    let back_over_midnight = || Dst {
        std: 0,
        dst: 3600,
        start: i64::MIN,
        end: utc(d(2026, 10, 1), 23, 30, 0), // 本機 10/2 00:30（+01:00）→ 10/1 23:30（+00:00）
    };
    let from = utc(d(2026, 10, 1), 12, 0, 0);
    let until = utc(d(2026, 10, 3), 12, 0, 0);
    let expected = vec![
        from,
        utc(d(2026, 10, 1), 23, 0, 0),
        utc(d(2026, 10, 3), 0, 0, 0),
    ];
    assert_eq!(
        Sim::new(back_over_midnight(), WallpaperTheme::Tearoff).poll(from, until, 60),
        expected
    );
    assert_eq!(
        Sim::new(back_over_midnight(), WallpaperTheme::Tearoff).follow_timer(from, until),
        expected
    );
}

#[test]
fn spring_gap_containing_0600_draws_at_the_jump_not_later() {
    // 合成時區：本機 05:30 直接跳到 06:30（06:00 不存在）。應在跳躍那一刻畫，不晚到 06:30Z 之類。
    let day = d(2026, 10, 2);
    let gap = || Dst {
        std: 0,
        dst: 3600,
        start: utc(day, 5, 30, 0),
        end: i64::MAX,
    };
    let mut sim = Sim::new(gap(), WallpaperTheme::Ridgeline);
    sim.step(utc(day, 0, 0, 0));
    assert_eq!(
        sim.eval(utc(day, 0, 0, 1)).next_wake,
        Some(utc(day, 5, 30, 0))
    );

    let from = utc(day, 0, 0, 0);
    let until = utc(d(2026, 10, 3), 12, 0, 0);
    let expected = vec![from, utc(day, 5, 30, 0), utc(d(2026, 10, 3), 5, 0, 0)];
    assert_eq!(
        Sim::new(gap(), WallpaperTheme::Ridgeline).follow_timer(from, until),
        expected
    );
    assert_eq!(
        Sim::new(gap(), WallpaperTheme::Ridgeline).poll(from, until, 60),
        expected
    );
}

/// task 4.7a：`clear_records` 只清紀錄，不動世代與「本次執行已接管」——時間倒退或主題設定檔改變
/// 後每台重畫一次；缺資料時仍是 HoldWithoutData（不是 WaitingForData）；先前做出的計畫照樣記錄。
#[test]
fn clear_records_redraws_all_but_keeps_generation_and_takeover() {
    let mut sim = Sim::new(TPE, WallpaperTheme::Ridgeline);
    let t = tpe(d(2026, 10, 5), 7, 0, 0);
    assert!(matches!(sim.step(t).action, Action::Redraw(_)));
    assert_eq!(sim.step(t).action, Action::UpToDate);
    let generation = sim.state.generation();

    let pending = sim.eval(t);
    sim.state.clear_records();
    assert_eq!(sim.state.generation(), generation, "世代不變");
    let again = sim.eval(t);
    let redraw = plan(&again);
    assert_eq!(redraw.targets.len(), 2);
    assert!(redraw
        .targets
        .iter()
        .all(|x| x.reason == RedrawReason::Unrecorded));
    // 清除前做出的（UpToDate）決策與世代無關；清除後做出的計畫照樣記錄得進去。
    assert_eq!(pending.action, Action::UpToDate);
    assert!(sim.state.record_drawn(redraw, &redraw.targets[0].monitor));

    sim.state.clear_records();
    sim.data.twii_intraday = None;
    assert_eq!(
        sim.eval(t).action,
        Action::HoldWithoutData {
            missing: DataKey::TwiiIntraday
        },
        "本次執行已接管的記憶保留"
    );
}

// ── 4.9 explorer 資源安全閥（design.md D11） ─────────────────────────────────

fn sample(pid: u32, gdi: u32) -> ExplorerSample {
    ExplorerSample { pid, gdi }
}

/// 第一次評估就要重畫（沒有紀錄）的情境，explorer 讀值由參數帶入。
fn valve_eval(explorer: ExplorerResources) -> Decision {
    let mut sim = Sim::new(TPE, WallpaperTheme::Astrolabe);
    sim.already_taken_over = true;
    sim.explorer = explorer;
    sim.eval(tpe(d(2026, 10, 5), 10, 5, 0))
}

fn sampled(reading: ExplorerSample, baseline: Option<ExplorerSample>) -> ExplorerResources {
    ExplorerResources::Sampled { reading, baseline }
}

fn trip(decision: &Decision) -> SafetyValveTrip {
    match decision.action {
        Action::StopTakeover(trip) => trip,
        ref other => panic!("預期 StopTakeover，得到 {other:?}"),
    }
}

#[test]
fn safety_valve_delta_of_exactly_2000_does_not_trip_but_2001_does() {
    let base = sample(4242, 1_000);
    let ok = valve_eval(sampled(sample(4242, 3_000), Some(base)));
    assert!(matches!(ok.action, Action::Redraw(_)), "{ok:?}");
    assert_eq!(ok.rebaseline, None, "同 PID 有基準：基準不變");

    let tripped = valve_eval(sampled(sample(4242, 3_001), Some(base)));
    let t = trip(&tripped);
    assert_eq!(t.reading, sample(4242, 3_001));
    assert_eq!(t.baseline, Some(base));
    assert!(t.delta_exceeded);
    assert!(!t.absolute_exceeded);
    assert_eq!(t.delta(), Some(2_001));
    assert_eq!(tripped.rebaseline, None, "觸發時不重設基準");
    assert_eq!(tripped.next_wake, None, "觸發後只等還原與事件");
}

#[test]
fn safety_valve_absolute_8000_does_not_trip_but_8001_does() {
    let base = sample(7, 7_000);
    let ok = valve_eval(sampled(sample(7, 8_000), Some(base)));
    assert!(matches!(ok.action, Action::Redraw(_)), "{ok:?}");

    let t = trip(&valve_eval(sampled(sample(7, 8_001), Some(base))));
    assert!(t.absolute_exceeded);
    assert!(!t.delta_exceeded, "增量 1,001 未超過");
}

#[test]
fn safety_valve_absolute_limit_applies_even_without_a_baseline() {
    // 接管開始（沒有基準）或 explorer 剛重啟時也不再往已接近上限的 explorer 加。
    let t = trip(&valve_eval(sampled(sample(9, 8_001), None)));
    assert_eq!(t.baseline, None);
    assert!(t.absolute_exceeded);
    assert_eq!(t.delta(), None);

    let t = trip(&valve_eval(sampled(
        sample(10, 8_500),
        Some(sample(9, 100)),
    )));
    assert_eq!(t.baseline, None, "PID 不同的基準不參與判定");
    assert!(t.absolute_exceeded && !t.delta_exceeded);
}

#[test]
fn safety_valve_read_failure_or_no_sample_never_trips() {
    for explorer in [ExplorerResources::ReadFailed, ExplorerResources::NotSampled] {
        let decision = valve_eval(explorer);
        assert!(
            matches!(decision.action, Action::Redraw(_)),
            "{explorer:?}：{decision:?}"
        );
        assert_eq!(
            decision.rebaseline, None,
            "{explorer:?}：沒有讀值就不動基準"
        );
    }
}

#[test]
fn safety_valve_first_reading_without_baseline_becomes_the_baseline() {
    let decision = valve_eval(sampled(sample(4242, 600), None));
    assert!(matches!(decision.action, Action::Redraw(_)), "{decision:?}");
    assert_eq!(
        decision.rebaseline,
        Some(ExplorerRebaseline {
            baseline: sample(4242, 600),
            cause: RebaselineCause::NoBaseline,
        })
    );
}

#[test]
fn safety_valve_pid_change_rebaselines_on_new_reading_without_tripping() {
    // spec Scenario「explorer 重新啟動」：GDI 降回較低的值，以重新啟動後第一次讀值為新基準。
    let old = sample(4242, 2_900);
    let decision = valve_eval(sampled(sample(5151, 400), Some(old)));
    assert!(matches!(decision.action, Action::Redraw(_)), "{decision:?}");
    assert_eq!(
        decision.rebaseline,
        Some(ExplorerRebaseline {
            baseline: sample(5151, 400),
            cause: RebaselineCause::PidChanged { previous: old },
        })
    );
    // 新 PID 的讀值比舊基準多出 2,000 以上也不觸發：舊 PID 的基準不能拿來比。
    let decision = valve_eval(sampled(sample(5151, 3_500), Some(sample(4242, 500))));
    assert!(matches!(decision.action, Action::Redraw(_)), "{decision:?}");
    assert_eq!(
        decision.rebaseline.map(|r| r.baseline),
        Some(sample(5151, 3_500))
    );
}

#[test]
fn safety_valve_reopened_takeover_resets_baseline_even_far_above_the_old_one() {
    // 使用者重新開啟接管：呼叫端已清掉狀態檔的基準（傳 None），這次讀值成為新基準、不觸發。
    let decision = valve_eval(sampled(sample(4242, 4_800), None));
    assert!(matches!(decision.action, Action::Redraw(_)), "{decision:?}");
    assert_eq!(
        decision.rebaseline,
        Some(ExplorerRebaseline {
            baseline: sample(4242, 4_800),
            cause: RebaselineCause::NoBaseline,
        })
    );
    // 下一次以它為基準：再多出 2,001 才觸發。
    let base = sample(4242, 4_800);
    assert!(matches!(
        valve_eval(sampled(sample(4242, 6_800), Some(base))).action,
        Action::Redraw(_)
    ));
    assert!(matches!(
        valve_eval(sampled(sample(4242, 6_801), Some(base))).action,
        Action::StopTakeover(_)
    ));
}

#[test]
fn safety_valve_reading_below_baseline_keeps_baseline() {
    let decision = valve_eval(sampled(sample(4242, 300), Some(sample(4242, 900))));
    assert!(matches!(decision.action, Action::Redraw(_)), "{decision:?}");
    assert_eq!(decision.rebaseline, None);
}

#[test]
fn safety_valve_is_judged_only_when_about_to_set_a_wallpaper() {
    let over = sampled(sample(4242, 9_999), Some(sample(4242, 100)));

    // 不需要重畫：不判定、不動基準。
    let mut sim = Sim::new(TPE, WallpaperTheme::Astrolabe);
    sim.already_taken_over = true;
    let now = tpe(d(2026, 10, 5), 10, 5, 0);
    assert!(matches!(sim.step(now).action, Action::Redraw(_)));
    sim.explorer = over;
    let decision = sim.eval(now + 60);
    assert_eq!(decision.action, Action::UpToDate);
    assert_eq!(decision.rebaseline, None);

    // 暫停中、不接管、等待資料：同樣不判定。
    let mut paused = Sim::new(TPE, WallpaperTheme::Astrolabe);
    paused.explorer = over;
    paused.pause.insert(PauseReason::Locked);
    assert_eq!(paused.eval(now).action, Action::Paused);

    let mut none = Sim::new(TPE, WallpaperTheme::None);
    none.explorer = over;
    let decision = none.eval(now);
    assert_eq!(decision.action, Action::Idle);
    assert_eq!(decision.rebaseline, None);

    let mut waiting = Sim::new(TPE, WallpaperTheme::Ridgeline);
    waiting.data = DataDates::default();
    waiting.explorer = over;
    assert!(matches!(
        waiting.eval(now).action,
        Action::WaitingForData { .. }
    ));
}

#[test]
fn safety_valve_trip_detail_names_reading_baseline_pid_and_limits() {
    let t = trip(&valve_eval(sampled(
        sample(4242, 3_501),
        Some(sample(4242, 1_500)),
    )));
    let detail = t.detail();
    for needle in [
        "4242",
        "3501",
        "1500",
        "2001",
        "2000",
        "8000",
        "增量超過門檻",
    ] {
        assert!(detail.contains(needle), "detail 缺 {needle}：{detail}");
    }
    let t = trip(&valve_eval(sampled(sample(77, 8_200), None)));
    let detail = t.detail();
    for needle in ["77", "8200", "尚無基準", "絕對值超過門檻"] {
        assert!(detail.contains(needle), "detail 缺 {needle}：{detail}");
    }
}
