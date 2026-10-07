//! 勿打擾偵測的測試（task 4.7c）。純函式（真值表、WNF 判讀、狀態追蹤）與輪詢器（假來源、假主題、
//! 毫秒級輪詢間隔）；不讀真正的系統狀態（本機唯讀實讀在 `desktop::dnd` 的測試）。

use super::*;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use log::Level;

use crate::desktop::dnd::WnfRaw;

fn raw(status: i32, size: u32, value: i32) -> Result<WnfRaw, String> {
    Ok(WnfRaw {
        status,
        size,
        data: value.to_le_bytes(),
    })
}

// ---------------------------------------------------------------------------------------------
// 純函式
// ---------------------------------------------------------------------------------------------

#[test]
fn combine_truth_table_is_or_and_unknown_never_pauses() {
    use Tri::{Off, On, Unknown};
    let cases = [
        (On, On, true),
        (On, Off, true),
        (On, Unknown, true),
        (Off, On, true),
        (Off, Off, false),
        (Off, Unknown, false),
        (Unknown, On, true),
        (Unknown, Off, false),
        (Unknown, Unknown, false),
    ];
    for (focus, wnf, expect) in cases {
        assert_eq!(combine(focus, wnf), expect, "專注 {focus:?} × WNF {wnf:?}");
    }
}

#[test]
fn wnf_value_interpretation() {
    assert_eq!(interpret_wnf(raw(0, 4, 0)).tri, Tri::Off, "0＝關閉");
    assert_eq!(interpret_wnf(raw(0, 4, 1)).tri, Tri::On, "1＝勿打擾");
    assert_eq!(interpret_wnf(raw(0, 4, 2)).tri, Tri::On, "2＝勿打擾（> 0）");
    assert_eq!(interpret_wnf(raw(0, 4, -1)).tri, Tri::Off, "負值不是 > 0");
    assert_eq!(
        interpret_wnf(raw(0, 4, i32::MIN)).tri,
        Tri::Off,
        "負值不是 > 0"
    );
    let short = interpret_wnf(raw(0, 2, 1));
    assert_eq!(short.tri, Tri::Unknown, "大小不足＝未知");
    assert!(short.detail.contains('2'), "{}", short.detail);
    assert_eq!(
        interpret_wnf(raw(0, 0, 0)).tri,
        Tri::Unknown,
        "沒有資料＝未知"
    );
    // STATUS_BUFFER_TOO_SMALL（0xC0000023）等非 0 的 NTSTATUS＝未知，緩衝區內容不看。
    let failed = interpret_wnf(raw(0xC000_0023_u32 as i32, 4, 1));
    assert_eq!(failed.tri, Tri::Unknown);
    assert!(failed.detail.contains("C0000023"), "{}", failed.detail);
    let missing = interpret_wnf(Err("ntdll.dll 沒有 NtQueryWnfStateData 匯出".to_owned()));
    assert_eq!(missing.tri, Tri::Unknown, "函式不存在＝未知");
    assert!(missing.detail.contains("NtQueryWnfStateData"));
    assert!(interpret_wnf(raw(0, 4, 0)).detail.contains("WNF=0"));
}

#[test]
fn focus_interpretation() {
    assert_eq!(interpret_focus(Ok(Some(true))).tri, Tri::On);
    assert_eq!(interpret_focus(Ok(Some(false))).tri, Tri::Off);
    let unsupported = interpret_focus(Ok(None));
    assert_eq!(unsupported.tri, Tri::Unknown, "IsSupported 為假＝未知");
    assert!(unsupported.detail.contains("IsSupported"));
    let failed = interpret_focus(Err(
        "FocusSessionManager.GetDefault 失敗：0x80040154".to_owned()
    ));
    assert_eq!(failed.tri, Tri::Unknown);
    assert!(failed.detail.contains("0x80040154"));
}

fn on(d: &str) -> SourceSample {
    SourceSample::on(d)
}
fn off(d: &str) -> SourceSample {
    SourceSample::off(d)
}
fn unknown(d: &str) -> SourceSample {
    SourceSample::unknown(d)
}

#[test]
fn tracker_logs_state_lines_only_on_change_and_reports_combined_changes() {
    let mut t = DndTracker::default();
    let first = t.observe(&off("IsFocusActive=false"), &off("WNF=0"));
    assert!(!first.combined);
    assert!(!first.changed, "一開始就是「不暫停」：不算改變");
    assert_eq!(first.lines.len(), 1, "第一次讀值記一行：{:?}", first.lines);
    let line = &first.lines[0].1;
    assert!(
        line.contains("IsFocusActive=false") && line.contains("WNF=0") && line.contains("不暫停"),
        "{line}"
    );

    let same = t.observe(&off("IsFocusActive=false"), &off("WNF=0"));
    assert!(!same.changed);
    assert!(same.lines.is_empty(), "讀值沒變不記");

    let dnd = t.observe(&off("IsFocusActive=false"), &on("WNF=1"));
    assert!(dnd.combined && dnd.changed);
    assert_eq!(dnd.lines.len(), 1);
    assert!(dnd.lines[0].1.contains("WNF=1") && dnd.lines[0].1.contains("暫停"));

    // 合併結果沒變、但其中一個來源的讀值變了：記一行、不算改變（不送喚醒）。
    let profile = t.observe(&off("IsFocusActive=false"), &on("WNF=2"));
    assert!(profile.combined && !profile.changed);
    assert_eq!(profile.lines.len(), 1);

    let both = t.observe(&on("IsFocusActive=true"), &on("WNF=2"));
    assert!(both.combined && !both.changed);

    let cleared = t.observe(&off("IsFocusActive=false"), &off("WNF=0"));
    assert!(!cleared.combined && cleared.changed);
}

#[test]
fn tracker_logs_a_failure_streak_once_and_its_recovery() {
    let mut t = DndTracker::default();
    t.observe(&off("IsFocusActive=false"), &off("WNF=0"));
    let first_fail = t.observe(&off("IsFocusActive=false"), &unknown("NTSTATUS 0xC0000034"));
    let warns: Vec<_> = first_fail
        .lines
        .iter()
        .filter(|(l, _)| *l == Level::Warn)
        .collect();
    assert_eq!(warns.len(), 1, "{:?}", first_fail.lines);
    assert!(warns[0].1.contains("0xC0000034"));
    assert!(!first_fail.changed, "未知＝不暫停，合併結果沒變");

    for msg in ["NTSTATUS 0xC0000034", "另一個錯誤", "NTSTATUS 0xC0000023"] {
        let again = t.observe(&off("IsFocusActive=false"), &unknown(msg));
        assert!(
            again.lines.is_empty(),
            "同一段連續失敗只記一次：{:?}",
            again.lines
        );
    }

    let recovered = t.observe(&off("IsFocusActive=false"), &off("WNF=0"));
    assert!(
        recovered
            .lines
            .iter()
            .any(|(l, m)| *l == Level::Info && m.contains("恢復")),
        "{:?}",
        recovered.lines
    );

    // 新的一段失敗再記一次；兩個來源各自計算。
    let again = t.observe(&unknown("GetDefault 失敗"), &unknown("NTSTATUS 0xC0000034"));
    let warns = again
        .lines
        .iter()
        .filter(|(l, _)| *l == Level::Warn)
        .count();
    assert_eq!(warns, 2, "{:?}", again.lines);
}

#[test]
fn tracker_reset_forgets_previous_readings() {
    let mut t = DndTracker::default();
    assert!(t.observe(&off("a"), &on("WNF=1")).changed);
    t.reset();
    let after = t.observe(&off("a"), &on("WNF=1"));
    assert!(
        after.changed,
        "重設後合併結果回到「不暫停」，再讀到勿打擾算改變"
    );
    assert_eq!(after.lines.len(), 1, "重設後第一次讀值重新記一行");
}

// ---------------------------------------------------------------------------------------------
// 輪詢器
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct FakeSystem {
    focus: Mutex<Option<SourceSample>>,
    wnf: Mutex<Option<SourceSample>>,
    reads: AtomicUsize,
    sources_created: AtomicUsize,
    takes_over: AtomicBool,
    notifies: AtomicUsize,
    logs: Mutex<Vec<(Level, String)>>,
    /// 建立來源時先等這麼久（模擬 WinRT 第一次啟用很慢）。
    create_delay: Mutex<Duration>,
    /// 為真時 `read_focus` 卡住（模擬跨行程呼叫無回應），直到改回假。
    block_focus: AtomicBool,
    /// `read_focus` 目前卡在裡面。
    in_blocked_read: AtomicBool,
    /// 假時鐘的位移（時效判定用；輪詢排程照真實時間走）。
    clock_offset: Mutex<Duration>,
}

impl FakeSystem {
    fn new() -> Arc<Self> {
        let s = Arc::new(Self::default());
        *s.focus.lock().unwrap() = Some(off("IsFocusActive=false"));
        *s.wnf.lock().unwrap() = Some(off("WNF=0"));
        s.takes_over.store(true, Ordering::SeqCst);
        s
    }
    fn set_wnf(&self, s: SourceSample) {
        *self.wnf.lock().unwrap() = Some(s);
    }
    fn advance_clock(&self, d: Duration) {
        *self.clock_offset.lock().unwrap() += d;
    }
    fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }
    fn notifies(&self) -> usize {
        self.notifies.load(Ordering::SeqCst)
    }
    fn count_logs(&self, level: Level, needle: &str) -> usize {
        self.logs
            .lock()
            .unwrap()
            .iter()
            .filter(|(l, m)| *l == level && m.contains(needle))
            .count()
    }
}

struct FakeSources(Arc<FakeSystem>);

impl DndSources for FakeSources {
    fn read_focus(&mut self) -> SourceSample {
        while self.0.block_focus.load(Ordering::SeqCst) {
            self.0.in_blocked_read.store(true, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(1));
        }
        self.0.in_blocked_read.store(false, Ordering::SeqCst);
        self.0.focus.lock().unwrap().clone().unwrap()
    }
    fn read_wnf(&mut self) -> SourceSample {
        let sample = self.0.wnf.lock().unwrap().clone().unwrap();
        // 修正輪 2（複審 N2）：讀完才遞增。`reads()` 只表示「來源已讀完幾次」，輪詢器更新狀態還在
        // 之後；需要「狀態已更新」的同步點改等判定結果或記錄行。
        self.0.reads.fetch_add(1, Ordering::SeqCst);
        sample
    }
}

fn monitor(sys: &Arc<FakeSystem>, poll: Duration, first_wait: Duration) -> DndMonitor {
    let for_sources = Arc::clone(sys);
    let for_theme = Arc::clone(sys);
    let for_notify = Arc::clone(sys);
    let for_log = Arc::clone(sys);
    let for_clock = Arc::clone(sys);
    let base = Instant::now();
    DndMonitor::new(
        DndConfig {
            poll_interval: poll,
            first_read_wait: first_wait,
            stale_after: STALE_AFTER,
        },
        DndHooks {
            sources: Box::new(move || {
                for_sources.sources_created.fetch_add(1, Ordering::SeqCst);
                let delay = *for_sources.create_delay.lock().unwrap();
                std::thread::sleep(delay);
                Box::new(FakeSources(for_sources)) as Box<dyn DndSources>
            }),
            takes_over: Box::new(move || for_theme.takes_over.load(Ordering::SeqCst)),
            notify: Box::new(move || {
                for_notify.notifies.fetch_add(1, Ordering::SeqCst);
            }),
            log: Arc::new(move |level, msg: &str| {
                for_log.logs.lock().unwrap().push((level, msg.to_owned()));
            }),
            clock: Arc::new(move || base + *for_clock.clock_offset.lock().unwrap()),
        },
    )
}

/// 等條件成立（最多 5 秒）。
fn wait_until(what: &str, cond: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !cond() {
        assert!(Instant::now() < deadline, "5 秒內沒有等到：{what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn monitor_that_is_never_queried_never_reads_or_spawns() {
    let sys = FakeSystem::new();
    let m = monitor(&sys, Duration::from_millis(5), Duration::from_secs(2));
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(
        sys.sources_created.load(Ordering::SeqCst),
        0,
        "沒有建立來源（沒有輪詢執行緒）"
    );
    assert_eq!(sys.reads(), 0);
    drop(m);
}

#[test]
fn first_query_waits_for_a_fresh_read() {
    let sys = FakeSystem::new();
    sys.set_wnf(on("WNF=1"));
    let m = monitor(&sys, Duration::from_secs(60), Duration::from_secs(5));
    assert!(m.query(), "第一次查詢等第一次讀值，看得到勿打擾");
    assert_eq!(sys.reads(), 1);
    assert!(m.query(), "之後回快取，不再讀");
    assert_eq!(sys.reads(), 1);
}

#[test]
fn only_a_change_of_the_combined_result_sends_a_wake() {
    let sys = FakeSystem::new();
    let m = monitor(&sys, Duration::from_millis(5), Duration::from_secs(5));
    assert!(!m.query());
    wait_until("輪詢數次", || sys.reads() >= 5);
    assert_eq!(sys.notifies(), 0, "讀值沒變不送喚醒");

    sys.set_wnf(on("WNF=1"));
    wait_until("偵測到勿打擾", || sys.notifies() == 1);
    assert!(m.query());
    let reads = sys.reads();
    wait_until("再輪詢數次", || sys.reads() >= reads + 5);
    assert_eq!(sys.notifies(), 1, "持續勿打擾不重複送");

    sys.set_wnf(on("WNF=2"));
    let reads = sys.reads();
    wait_until("再輪詢數次", || sys.reads() >= reads + 5);
    assert_eq!(sys.notifies(), 1, "合併結果沒變不送");

    sys.set_wnf(off("WNF=0"));
    wait_until("偵測到解除", || sys.notifies() == 2);
    assert!(!m.query());
}

#[test]
fn consecutive_read_failures_are_logged_once_through_the_poller() {
    let sys = FakeSystem::new();
    let m = monitor(&sys, Duration::from_millis(5), Duration::from_secs(5));
    sys.set_wnf(unknown("NTSTATUS 0xC0000034"));
    assert!(!m.query(), "讀不到＝不暫停");
    wait_until("連續失敗數次", || sys.reads() >= 6);
    assert_eq!(sys.count_logs(Level::Warn, "0xC0000034"), 1);
    assert_eq!(sys.notifies(), 0, "未知＝不暫停，沒有改變");
}

#[test]
fn theme_none_stops_polling_until_the_next_query() {
    let sys = FakeSystem::new();
    let m = monitor(&sys, Duration::from_millis(5), Duration::from_secs(5));
    assert!(!m.query());
    wait_until("開始輪詢", || sys.reads() >= 3);
    sys.takes_over.store(false, Ordering::SeqCst);
    wait_until("偵測到不接管而停止", || {
        sys.count_logs(Level::Info, "停止輪詢") == 1
    });
    let stopped_at = sys.reads();
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(sys.reads(), stopped_at, "主題不接管時不輪詢");

    // 重新選了主題、協調迴圈又要重畫：查詢時重新啟用並先讀一次。
    sys.takes_over.store(true, Ordering::SeqCst);
    sys.set_wnf(on("WNF=1"));
    assert!(m.query(), "重新啟用時等新的讀值");
    assert_eq!(
        sys.sources_created.load(Ordering::SeqCst),
        1,
        "沿用同一條執行緒"
    );
    wait_until("恢復輪詢", || sys.reads() >= stopped_at + 3);
}

#[test]
fn slow_first_read_returns_unknown_without_blocking_past_the_limit_then_wakes() {
    let sys = FakeSystem::new();
    *sys.create_delay.lock().unwrap() = Duration::from_millis(300);
    sys.set_wnf(on("WNF=1"));
    let m = monitor(&sys, Duration::from_secs(60), Duration::from_millis(30));
    let started = Instant::now();
    assert!(!m.query(), "等不到第一次讀值＝未知＝不暫停");
    assert!(started.elapsed() < Duration::from_millis(250), "只等上限");
    assert_eq!(sys.count_logs(Level::Warn, "第一次讀值"), 1);
    // 讀值稍後到了：合併結果改變 → 送喚醒，協調迴圈再評估時看得到。
    wait_until("讀值到了送喚醒", || sys.notifies() == 1);
    assert!(m.query());
}

#[test]
fn dropping_the_monitor_ends_the_poller_thread() {
    let sys = FakeSystem::new();
    let m = monitor(&sys, Duration::from_millis(5), Duration::from_secs(5));
    assert!(!m.query());
    wait_until("開始輪詢", || sys.reads() >= 2);
    drop(m);
    std::thread::sleep(Duration::from_millis(30));
    let after = sys.reads();
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(sys.reads(), after, "丟棄後輪詢執行緒結束");
}

/// 修正輪 1（審查 low1）＋修正輪 2（複審 N1、N2）：輪詢執行緒卡住時，最後讀值不能無限期有效——超過
/// [`STALE_AFTER`]（3 個輪詢週期＝180 秒，假時鐘）沒有新讀值時，第一次判定先要求立即重讀、最多等
/// `first_read_wait`；卡住等不到才視為未知、不暫停，同一段只 warn 一次、只等一次；恢復後照讀值判斷。
/// 同步點一律等可觀察的完成訊號（判定結果、記錄行），不以來源被呼叫的次數推斷讀值已完成（N2）。
#[test]
fn stale_reading_from_a_stuck_poller_is_treated_as_unknown() {
    assert_eq!(STALE_AFTER, POLL_INTERVAL * 3, "正式門檻＝3 個輪詢週期");
    let first_wait = Duration::from_millis(300);
    let sys = FakeSystem::new();
    sys.set_wnf(on("WNF=1"));
    let m = monitor(&sys, Duration::from_millis(5), first_wait);
    assert!(m.query(), "讀到勿打擾");

    sys.block_focus.store(true, Ordering::SeqCst);
    wait_until("輪詢執行緒卡在讀取裡", || {
        sys.in_blocked_read.load(Ordering::SeqCst)
    });
    sys.advance_clock(STALE_AFTER - Duration::from_secs(1));
    let started = Instant::now();
    assert!(m.query(), "門檻內仍以最後讀值判定");
    assert!(started.elapsed() < first_wait, "門檻內不等待");
    assert_eq!(sys.count_logs(Level::Warn, "沒有更新"), 0);

    sys.advance_clock(Duration::from_secs(2));
    let started = Instant::now();
    assert!(!m.query(), "超過門檻、立即重讀也等不到＝未知＝不暫停");
    assert!(
        started.elapsed() >= first_wait,
        "第一次判定過期時先等一次重讀"
    );
    sys.advance_clock(Duration::from_secs(600));
    let started = Instant::now();
    assert!(!m.query());
    assert!(
        started.elapsed() < first_wait / 2,
        "同一段過期只等一次，之後的評估立即返回"
    );
    assert_eq!(sys.count_logs(Level::Warn, "沒有更新"), 1, "同一段只記一次");

    // 卡住的呼叫返回：恢復照讀值判斷（仍是勿打擾）。等「恢復更新」那行（在狀態更新之後才寫）當同步點。
    sys.block_focus.store(false, Ordering::SeqCst);
    wait_until("恢復更新", || {
        sys.count_logs(Level::Info, "恢復更新") == 1
    });
    assert!(m.query(), "恢復後照讀值判斷");
    sys.set_wnf(off("WNF=0"));
    wait_until("讀到解除", || !m.query());
    assert_eq!(sys.count_logs(Level::Info, "恢復更新"), 1);

    // 新的一段卡住再記一次、再等一次。
    sys.set_wnf(on("WNF=1"));
    wait_until("又讀到勿打擾", || m.query());
    sys.block_focus.store(true, Ordering::SeqCst);
    wait_until("再次卡住", || {
        sys.in_blocked_read.load(Ordering::SeqCst)
    });
    sys.advance_clock(STALE_AFTER + Duration::from_secs(1));
    let started = Instant::now();
    assert!(!m.query());
    assert!(started.elapsed() >= first_wait, "新的一段再等一次");
    assert_eq!(sys.count_logs(Level::Warn, "沒有更新"), 2);
    sys.block_focus.store(false, Ordering::SeqCst);
}

/// 修正輪 2（複審 N1）：單調時鐘含睡眠時間（QPC），睡眠超過門檻喚醒後、輪詢器還沒讀到新值時協調迴圈
/// 先評估——讀值看起來過期，但輪詢器健康：立即重讀、在等待內拿到新值就照新值判斷（仍是勿打擾＝暫停），
/// 不記 warn；之後的評估不再等。
#[test]
fn stale_reading_after_sleep_is_refreshed_from_a_healthy_poller() {
    let sys = FakeSystem::new();
    sys.set_wnf(on("WNF=1"));
    // 輪詢間隔很長：睡眠期間輪詢器停在等待裡，只有立即重讀的要求會讓它讀。
    let m = monitor(&sys, Duration::from_secs(60), Duration::from_secs(5));
    assert!(m.query());
    assert_eq!(sys.reads(), 1);

    sys.advance_clock(Duration::from_secs(600));
    assert!(
        m.query(),
        "健康的輪詢器在等待內回應：照新值判斷，勿打擾中仍暫停"
    );
    assert_eq!(sys.reads(), 2, "過期時立即重讀一次");
    assert_eq!(sys.count_logs(Level::Warn, "沒有更新"), 0);
    assert!(m.query());
    assert_eq!(sys.reads(), 2, "拿到新值後不再重讀");

    // 睡眠期間勿打擾已解除：重讀拿到的是解除。
    sys.set_wnf(off("WNF=0"));
    sys.advance_clock(Duration::from_secs(600));
    assert!(!m.query(), "照重讀到的新值判斷");
    assert_eq!(sys.reads(), 3);
}

/// 修正輪 1（審查 low3）：正式接線的主題判斷與喚醒種類。
#[test]
fn host_wiring_takes_over_for_every_theme_but_none_and_sends_the_dnd_wake() {
    use crate::settings::WallpaperTheme;
    use crate::wallpaper_coordinator::Wake;

    let theme = Arc::new(Mutex::new(WallpaperTheme::None));
    let sent: Arc<Mutex<Vec<Wake>>> = Arc::new(Mutex::new(Vec::new()));
    let (takes_over, notify) = host_hooks(
        {
            let theme = Arc::clone(&theme);
            move || *theme.lock().unwrap()
        },
        {
            let sent = Arc::clone(&sent);
            move |w| sent.lock().unwrap().push(w)
        },
    );
    let expected = [
        (WallpaperTheme::None, false),
        (WallpaperTheme::Astrolabe, true),
        (WallpaperTheme::Tearoff, true),
        (WallpaperTheme::Ridgeline, true),
        (WallpaperTheme::Contour, true),
        (WallpaperTheme::Skyline, true),
    ];
    assert_eq!(
        expected.len(),
        WallpaperTheme::ALL.len(),
        "主題清單有變：更新本表"
    );
    for (t, want) in expected {
        *theme.lock().unwrap() = t;
        assert_eq!(takes_over(), want, "主題 {t:?}");
    }
    assert!(sent.lock().unwrap().is_empty());
    notify();
    assert_eq!(*sent.lock().unwrap(), vec![Wake::DoNotDisturbChanged]);
}
