//! 抓取排程的**純判斷**（data-layer-rust tasks.md 5.2、design.md D5；spec「定時抓取」「啟動與恢復時補抓」
//! 「抓取間隔下限」）。
//!
//! 這個檔案不碰時鐘、檔案、執行緒或網路：輸入是「現在」（UTC 瞬間）、上一次醒來的時間、排程紀錄
//! （[`FetchState`]，可缺）、資料檔是否存在、亂數源（[`Rng`]），輸出是「何時開始下一輪＋觸發原因」
//! （[`Pending`]）或「不用」。真正的醒來迴圈、紀錄檔讀寫與執行一輪在 [`super::scheduler`]。
//!
//! # 規則（design.md D5，逐條）
//!
//! 時點＝台北時間每日 00／06／12／15／18 點（[`SLOT_HOURS`]）。每次醒來（約每 60 秒，[`WAKE_INTERVAL`]；
//! 有待啟動的一輪時改成睡到它的開始時刻）：
//!
//! 1. **已有待啟動的一輪**：到了開始時刻就開始，否則繼續睡。待啟動的時刻在決定當下就抽好亂數，
//!    之後不重抽（否則每分鐘重抽會讓「0–5 分鐘隨機」偏向最小值）。
//! 2. 否則依序判斷（先成立者勝）：
//!    1. **資料檔不存在** → [`Trigger::FirstFetch`]：現在起 5–15 秒，不受 60 分鐘下限，也不受舊的失敗紀錄
//!       影響（spec「首次安裝」：資料檔不存在時一律 5–15 秒）。唯一例外是**熱迴圈防護**：同一個資料目錄上一輪
//!       才剛跑完、檔案卻還是不存在（寫檔失敗：唯讀、磁碟滿、目錄建不起來），這時不首抓、改走重試退避
//!       （[`Wake::first_fetch_suppressed`]，由 [`Scheduler::round_finished`] 判定）；
//!    2. **上一輪沒有任何來源成功**（紀錄 `last_any_success=false`）→ [`Trigger::Retry`]：上一輪結束後
//!       10–15 分鐘（[`RETRY_BASE_SECS`]），連續失敗每次加倍、上限 60 分鐘（[`RETRY_CAP`]），**不受 60 分鐘
//!       下限**。重試時刻已過（宿主久未執行）改成現在起 30–120 秒；
//!    3. 紀錄讀不到或壞掉（`None`）→ 視同錯過 → [`Trigger::Missed`]：30–120 秒；
//!    4. 上一輪開始之後已經過了某個時點，才有下面兩種（否則「不用」）：
//!       - **到點**（[`Trigger::Slot`]）：上次醒來到這次醒來之間跨過該時點，且兩次醒來相隔不超過 3 分鐘
//!         （[`ON_TIME_MAX_GAP`]）→ 時點後 0–5 分鐘隨機；
//!       - **錯過**（[`Trigger::Missed`]）：不是到點（宿主剛啟動＝沒有上次醒來，或兩次醒來相隔超過
//!         3 分鐘＝睡眠或卡住）→ 現在起 30–120 秒；
//!       - 兩者都再套 **60 分鐘下限**：開始時刻不早於上一輪開始＋60 分鐘（延後、不丟棄）。
//! 3. **系統時鐘倒退**：紀錄裡的上一輪開始（或結束）時間晚於現在 → 視為現在（[`FetchState::clamped_to`]），
//!    因此不立即抓，60 分鐘下限從現在起算；待啟動時刻離現在超過 [`MAX_PENDING_AHEAD`] 也視為時鐘被調動、丟棄重算。
//!
//! 熱迴圈防護的設計：首抓與重試是兩個獨立的目標——資料檔不存在時**一律** 5–15 秒首抓（含舊紀錄停在失敗
//! 狀態），而「寫不出檔」的失敗輪由 [`FetchState::after_round`] 記為失敗、走加倍退避；防護只靠「剛跑完一輪、
//! 檔案仍不存在」這個事實，不靠延後首抓。
//!
//! 待啟動的一輪會在下列情況丟棄並重新判定：離現在超過 [`MAX_PENDING_AHEAD`]（時鐘被調動）；已到期但上次
//! 醒來距今超過 [`ON_TIME_MAX_GAP`]（睡眠或卡住：睡前排好的 `retry`／`slot` 恢復後要重走 30–120 秒的補抓
//! 延遲，不可立即開始）；資料檔消失且待啟動的不是 `FirstFetch`（例如換到空資料夾）。
//!
//! # 為什麼用系統時鐘不用單調時鐘
//!
//! 睡眠期間單調時鐘不一定前進，系統時鐘一定會；「時點」本身也是牆上時間（台北），兩者必須同一把尺
//! （design.md D5）。

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};

use time::{Duration, OffsetDateTime, Time};

use super::clock::TPE_OFFSET;

/// 台北時間每日的抓取時點（時）。
pub const SLOT_HOURS: [u8; 5] = [0, 6, 12, 15, 18];
/// 平常醒來的間隔。
pub const WAKE_INTERVAL: Duration = Duration::seconds(60);
/// 兩次醒來相隔不超過這個值才算「持續執行中跨過時點」（到點）；超過＝睡眠或卡住（錯過）。
pub const ON_TIME_MAX_GAP: Duration = Duration::minutes(3);
/// 兩輪抓取開始時間的最小間隔（spec「抓取間隔下限」）。
pub const MIN_INTERVAL: Duration = Duration::minutes(60);
/// 到點：時點後的隨機延遲（秒，含兩端）。
pub const SLOT_JITTER_SECS: (i64, i64) = (0, 300);
/// 錯過：現在起的隨機延遲（秒，含兩端）。
pub const MISSED_DELAY_SECS: (i64, i64) = (30, 120);
/// 資料檔不存在的首抓：現在起的隨機延遲（秒，含兩端）。
pub const FIRST_FETCH_DELAY_SECS: (i64, i64) = (5, 15);
/// 失敗重試的基底延遲（秒，含兩端）；第 k 次連續失敗乘 2^(k-1)。
pub const RETRY_BASE_SECS: (i64, i64) = (600, 900);
/// 失敗重試延遲的上限。
pub const RETRY_CAP: Duration = Duration::minutes(60);
/// 待啟動時刻離現在超過這個值＝系統時鐘被調動過，丟棄重算（合法的最長延遲是 60 分鐘下限＋5 分鐘）。
pub const MAX_PENDING_AHEAD: Duration = Duration::hours(2);

/// 亂數源（注入點）。測試給固定值，正式用 [`SystemRng`]。
pub trait Rng {
    /// 回傳 `[0, 1)` 的值。
    fn unit(&mut self) -> f64;
}

/// 作業系統給種子的亂數（`RandomState` 的每次建立都帶新的隨機金鑰，不需要額外 crate）。
#[derive(Debug, Default)]
pub struct SystemRng;

impl Rng for SystemRng {
    fn unit(&mut self) -> f64 {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u8(0);
        (hasher.finish() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// 在 `[lo, hi]` 秒（含兩端）內均勻抽一個時間長度。`Rng::unit` 超出 `[0,1)` 時夾回，結果必在範圍內。
pub fn pick_secs(rng: &mut dyn Rng, (lo, hi): (i64, i64)) -> Duration {
    let span = hi - lo;
    let u = rng.unit();
    let u = if u.is_finite() {
        u.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let offset = ((u * (span + 1) as f64).floor() as i64).clamp(0, span);
    Duration::seconds(lo + offset)
}

/// 為什麼開始這一輪（記錄檔「觸發」欄位，[`Trigger::as_str`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// 資料檔不存在的首抓（首次安裝、換到空的資料目錄）。
    FirstFetch,
    /// 到點：持續執行中跨過台北時間時點。
    Slot,
    /// 錯過：啟動或睡眠恢復後發現上一輪之後已過了時點（或排程紀錄不可用）。
    Missed,
    /// 上一輪沒有任何來源成功，重試。
    Retry,
}

impl Trigger {
    /// 記錄檔用的觸發原因字串。
    pub fn as_str(self) -> &'static str {
        match self {
            Trigger::FirstFetch => "first-fetch",
            Trigger::Slot => "slot",
            Trigger::Missed => "missed",
            Trigger::Retry => "retry",
        }
    }
}

/// 排程紀錄（`fetch-state.json`，design.md D10）。檔案讀寫在 [`super::state`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FetchState {
    /// 上一輪開始的時間。
    pub last_start: OffsetDateTime,
    /// 上一輪結束的時間（從沒結束過＝`None`）。
    pub last_finish: Option<OffsetDateTime>,
    /// 上一輪是否有任何來源成功（定義見 [`super::round::RoundResult::any_source_ok`]）。
    pub last_any_success: bool,
    /// 連續幾輪沒有任何來源成功。
    pub consecutive_failures: u32,
}

impl FetchState {
    /// 系統時鐘倒退的處理：開始或結束時間晚於 `now` 的，改成 `now`。回傳（處理後、是否有改動）。
    pub fn clamped_to(&self, now: OffsetDateTime) -> (FetchState, bool) {
        let mut s = *self;
        let mut changed = false;
        if s.last_start > now {
            s.last_start = now;
            changed = true;
        }
        if let Some(f) = s.last_finish {
            if f > now {
                s.last_finish = Some(now);
                changed = true;
            }
        }
        (s, changed)
    }

    /// 一輪結束後的新紀錄：`any_success` 為真時連續失敗歸零，否則在上一筆（沒有＝0）上加一。
    pub fn after_round(
        prev: Option<&FetchState>,
        start: OffsetDateTime,
        finish: OffsetDateTime,
        any_success: bool,
    ) -> FetchState {
        let failures = if any_success {
            0
        } else {
            prev.map_or(0, |p| p.consecutive_failures).saturating_add(1)
        };
        FetchState {
            last_start: start,
            last_finish: Some(finish),
            last_any_success: any_success,
            consecutive_failures: failures,
        }
    }
}

/// 一輪待啟動：何時、為什麼。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pending {
    /// 開始時刻（UTC 瞬間；可能已經過去＝下一次醒來就開始）。
    pub due: OffsetDateTime,
    pub trigger: Trigger,
}

/// 一次醒來給 [`evaluate`] 的輸入。
#[derive(Debug, Clone, Copy)]
pub struct Wake<'a> {
    pub now: OffsetDateTime,
    /// 上一次醒來的時間；宿主剛啟動（本行程第一次醒來）＝`None`。
    pub prev_wake: Option<OffsetDateTime>,
    /// 排程紀錄；讀不到或壞掉＝`None`。
    pub state: Option<&'a FetchState>,
    /// 目前資料目錄下的 `tw_events.json` 是否存在。
    pub data_file_exists: bool,
    /// 熱迴圈防護：同一個資料目錄上一輪剛跑完、檔案仍不存在（寫檔失敗）。為真時不首抓、走重試退避。
    pub first_fetch_suppressed: bool,
}

/// 不早於（含）`t` 的最近一個時點（台北時間 00／06／12／15／18 點整）。
pub fn latest_slot_at_or_before(t: OffsetDateTime) -> OffsetDateTime {
    let tpe = t.to_offset(TPE_OFFSET);
    let date = tpe.date();
    for hour in SLOT_HOURS.iter().rev() {
        let Ok(time) = Time::from_hms(*hour, 0, 0) else {
            continue;
        };
        let slot = date.with_time(time).assume_offset(TPE_OFFSET);
        if slot <= tpe {
            return slot;
        }
    }
    // 00:00 也是時點，上面一定會命中；這裡只是型別上的退路。
    date.with_time(Time::MIDNIGHT).assume_offset(TPE_OFFSET)
}

/// 第 `failures` 次連續失敗後的重試延遲：基底 10–15 分鐘，每多失敗一次加倍，上限 [`RETRY_CAP`]。
fn retry_delay(failures: u32, rng: &mut dyn Rng) -> Duration {
    let base = pick_secs(rng, RETRY_BASE_SECS);
    let doublings = failures.saturating_sub(1).min(8);
    let scaled = base * (1_i32 << doublings);
    if scaled > RETRY_CAP {
        RETRY_CAP
    } else {
        scaled
    }
}

/// 沒有待啟動的一輪時，這次醒來要不要排一輪（規則見模組文件）。回傳 `None`＝不用。
pub fn evaluate(wake: &Wake<'_>, rng: &mut dyn Rng) -> Option<Pending> {
    let now = wake.now;
    // 系統時鐘倒退：上一輪開始／結束晚於現在 → 視為現在。
    let state = wake.state.map(|s| s.clamped_to(now).0);

    // 資料檔不存在＝首抓，一律 5–15 秒（不看舊的失敗紀錄）；唯一例外是同一目錄剛寫檔失敗（熱迴圈防護）。
    if !wake.data_file_exists && !wake.first_fetch_suppressed {
        return Some(Pending {
            due: now + pick_secs(rng, FIRST_FETCH_DELAY_SECS),
            trigger: Trigger::FirstFetch,
        });
    }

    if let Some(s) = &state {
        if !s.last_any_success && s.consecutive_failures >= 1 {
            let base = s.last_finish.unwrap_or(s.last_start);
            let due = base + retry_delay(s.consecutive_failures, rng);
            let due = if due <= now {
                now + pick_secs(rng, MISSED_DELAY_SECS)
            } else {
                due
            };
            return Some(Pending {
                due,
                trigger: Trigger::Retry,
            });
        }
    }

    let Some(s) = state else {
        // 沒有可用的排程紀錄＝等同錯過。
        return Some(Pending {
            due: now + pick_secs(rng, MISSED_DELAY_SECS),
            trigger: Trigger::Missed,
        });
    };

    let slot = latest_slot_at_or_before(now);
    if slot <= s.last_start {
        return None;
    }
    let on_time = wake
        .prev_wake
        .is_some_and(|pw| pw < slot && now - pw <= ON_TIME_MAX_GAP);
    let (due, trigger) = if on_time {
        (slot + pick_secs(rng, SLOT_JITTER_SECS), Trigger::Slot)
    } else {
        (now + pick_secs(rng, MISSED_DELAY_SECS), Trigger::Missed)
    };
    // 60 分鐘下限：延後、不丟棄。
    Some(Pending {
        due: due.max(s.last_start + MIN_INTERVAL),
        trigger,
    })
}

/// [`Scheduler::step`] 的結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// 現在開始一輪（原因見 [`Trigger`]）。
    Start(Trigger),
    /// 睡這麼久再醒來（不超過 [`WAKE_INTERVAL`]；有待啟動的一輪時最長睡到它的開始時刻）。
    Sleep(Duration),
}

/// 醒來迴圈的記憶：待啟動的一輪與上次醒來的時間。仍是純邏輯（時間與輸入都由呼叫端給）。
#[derive(Debug, Default)]
pub struct Scheduler {
    pending: Option<Pending>,
    prev_wake: Option<OffsetDateTime>,
    /// 目前的資料目錄（換目錄就清掉 `suppress_first_fetch`）。
    data_dir: Option<std::path::PathBuf>,
    /// 同一個資料目錄上一輪剛跑完、檔案仍不存在（寫檔失敗）：不首抓、走重試退避。
    suppress_first_fetch: bool,
}

impl Scheduler {
    /// 目前待啟動的一輪（測試用）。
    #[cfg(test)]
    pub fn pending(&self) -> Option<Pending> {
        self.pending
    }

    /// 告知目前的資料目錄；與上一次不同（使用者換了資料夾）就解除首抓抑制——換到新的空資料夾要立刻首抓。
    pub fn set_data_dir(&mut self, dir: &std::path::Path) {
        if self.data_dir.as_deref() != Some(dir) {
            self.data_dir = Some(dir.to_path_buf());
            self.suppress_first_fetch = false;
        }
    }

    /// 一次醒來：決定要開始一輪還是繼續睡。呼叫端收到 [`Step::Start`] 就執行一輪，結束後呼叫
    /// [`Scheduler::round_finished`]。
    pub fn step(
        &mut self,
        now: OffsetDateTime,
        state: Option<&FetchState>,
        data_file_exists: bool,
        rng: &mut dyn Rng,
    ) -> Step {
        let prev_wake = self.prev_wake.replace(now);
        if data_file_exists {
            self.suppress_first_fetch = false;
        }
        if let Some(p) = self.pending {
            let too_far = p.due - now > MAX_PENDING_AHEAD;
            // 睡眠或卡住之後，睡前排好且已到期的一輪不可立即開始：重新判定（變成「錯過」就走 30–120 秒）。
            let overslept = p.due <= now && prev_wake.is_some_and(|pw| now - pw > ON_TIME_MAX_GAP);
            // 資料檔消失（換到空資料夾）：不必等原本的時刻，重新判定成首抓。
            let emptied =
                !data_file_exists && !self.suppress_first_fetch && p.trigger != Trigger::FirstFetch;
            if too_far || overslept || emptied {
                self.pending = None;
            }
        }
        if self.pending.is_none() {
            self.pending = evaluate(
                &Wake {
                    now,
                    prev_wake,
                    state,
                    data_file_exists,
                    first_fetch_suppressed: self.suppress_first_fetch,
                },
                rng,
            );
        }
        match self.pending {
            Some(p) if p.due <= now => {
                self.pending = None;
                Step::Start(p.trigger)
            }
            Some(p) => Step::Sleep((p.due - now).min(WAKE_INTERVAL)),
            None => Step::Sleep(WAKE_INTERVAL),
        }
    }

    /// 一輪跑完（不論成敗）：以結束時間當作「上次醒來」。一輪可能跑好幾分鐘，若不更新，下一次醒來會
    /// 把這段時間誤當成睡眠。`data_file_exists_after`＝跑完後資料檔是否存在：不存在代表寫檔失敗，之後不首抓、
    /// 改走重試退避（熱迴圈防護，見模組文件）。
    pub fn round_finished(&mut self, now: OffsetDateTime, data_file_exists_after: bool) {
        self.prev_wake = Some(now);
        self.pending = None;
        self.suppress_first_fetch = !data_file_exists_after;
    }

    /// 抓取停用期間（`data_fetch` 為 off 或隔離環境）的醒來：丟掉待啟動的一輪，只記醒來時間。重新啟用後
    /// 由排程紀錄決定要不要補抓（錯過規則）。
    ///
    /// 同時解除首抓抑制：抑制只屬於「同一個資料目錄剛跑完一輪、檔案仍不存在」，停用再啟用之後不該再沿用
    /// （審查 N2：首抓途中被關掉、再打開，仍要在 5–15 秒內首抓）。被中止的一輪（沒有跑完）同樣走這條。
    pub fn idle_wake(&mut self, now: OffsetDateTime) {
        self.prev_wake = Some(now);
        self.pending = None;
        self.suppress_first_fetch = false;
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use time::macros::datetime;

    // ───────────── 測試工具 ─────────────

    /// 固定值亂數：`unit()` 永遠回同一個值。
    pub struct Fixed(pub f64);
    impl Rng for Fixed {
        fn unit(&mut self) -> f64 {
            self.0
        }
    }
    /// 抽到範圍下界／上界的極值。
    pub const LO: f64 = 0.0;
    pub const HI: f64 = 0.999_999_999;

    /// 台北時間（+08:00）的瞬間。
    fn tpe(y: i32, mo: u8, d: u8, h: u8, mi: u8, s: u8) -> OffsetDateTime {
        time::Date::from_calendar_date(y, time::Month::try_from(mo).unwrap(), d)
            .unwrap()
            .with_hms(h, mi, s)
            .unwrap()
            .assume_offset(TPE_OFFSET)
    }

    fn secs(n: i64) -> Duration {
        Duration::seconds(n)
    }

    fn ok_state(start: OffsetDateTime) -> FetchState {
        FetchState {
            last_start: start,
            last_finish: Some(start + secs(40)),
            last_any_success: true,
            consecutive_failures: 0,
        }
    }

    /// 模擬器：依 [`Scheduler::step`] 的指示推進時間，一輪固定耗時 `round_secs`、成敗由 `round_ok` 決定。
    struct Sim {
        sched: Scheduler,
        state: Option<FetchState>,
        file_exists: bool,
        rng: Fixed,
        now: OffsetDateTime,
        round_secs: i64,
        round_ok: bool,
        /// 寫檔失敗（資料檔始終不存在）。
        write_fails: bool,
        /// 每次醒來比預定晚多少秒（模擬排程延遲）。
        wake_latency: i64,
        starts: Vec<(OffsetDateTime, Trigger)>,
    }

    impl Sim {
        fn new(
            now: OffsetDateTime,
            state: Option<FetchState>,
            file_exists: bool,
            unit: f64,
        ) -> Sim {
            Sim {
                sched: Scheduler::default(),
                state,
                file_exists,
                rng: Fixed(unit),
                now,
                round_secs: 40,
                round_ok: true,
                write_fails: false,
                wake_latency: 0,
                starts: Vec::new(),
            }
        }

        /// 一次醒來；回傳 `Some(trigger)` 表示這次醒來開始並跑完了一輪。
        fn wake_once(&mut self) -> Option<Trigger> {
            // 與正式迴圈相同：時鐘倒退造成的紀錄修正會寫回紀錄檔。
            if let Some(s) = self.state {
                let (clamped, changed) = s.clamped_to(self.now);
                if changed {
                    self.state = Some(clamped);
                }
            }
            let step = self.sched.step(
                self.now,
                self.state.as_ref(),
                self.file_exists,
                &mut self.rng,
            );
            match step {
                Step::Start(trigger) => {
                    let start = self.now;
                    self.starts.push((start, trigger));
                    let finish = start + secs(self.round_secs);
                    // 一輪一定會寫出資料檔（全部失敗也寫沿用資料，K-15），除非寫檔失敗。
                    self.file_exists = !self.write_fails;
                    self.state = Some(FetchState::after_round(
                        self.state.as_ref(),
                        start,
                        finish,
                        self.round_ok,
                    ));
                    self.now = finish;
                    self.sched.round_finished(finish, self.file_exists);
                    Some(trigger)
                }
                Step::Sleep(d) => {
                    assert!(d > Duration::ZERO && d <= WAKE_INTERVAL, "睡眠長度 {d}");
                    self.now += d + secs(self.wake_latency);
                    None
                }
            }
        }

        /// 一直醒來到 `until`（含）為止。
        fn run_until(&mut self, until: OffsetDateTime) {
            let mut guard = 0;
            while self.now <= until {
                self.wake_once();
                guard += 1;
                assert!(guard < 100_000, "模擬沒有推進");
            }
        }

        /// 沒有醒來的一段時間（睡眠、關機）：時間直接跳到 `t`。
        fn jump_to(&mut self, t: OffsetDateTime) {
            assert!(t >= self.now);
            self.now = t;
        }
    }

    // ───────────── 時點 ─────────────

    #[test]
    fn latest_slot_is_the_taipei_wall_clock_slot_regardless_of_the_input_offset() {
        let cases = [
            (tpe(2026, 10, 5, 0, 0, 0), tpe(2026, 10, 5, 0, 0, 0)),
            (tpe(2026, 10, 5, 5, 59, 59), tpe(2026, 10, 5, 0, 0, 0)),
            (tpe(2026, 10, 5, 6, 0, 0), tpe(2026, 10, 5, 6, 0, 0)),
            (tpe(2026, 10, 5, 11, 59, 59), tpe(2026, 10, 5, 6, 0, 0)),
            (tpe(2026, 10, 5, 12, 0, 0), tpe(2026, 10, 5, 12, 0, 0)),
            (tpe(2026, 10, 5, 14, 59, 59), tpe(2026, 10, 5, 12, 0, 0)),
            (tpe(2026, 10, 5, 15, 0, 0), tpe(2026, 10, 5, 15, 0, 0)),
            (tpe(2026, 10, 5, 17, 59, 59), tpe(2026, 10, 5, 15, 0, 0)),
            (tpe(2026, 10, 5, 18, 0, 0), tpe(2026, 10, 5, 18, 0, 0)),
            (tpe(2026, 10, 5, 23, 59, 59), tpe(2026, 10, 5, 18, 0, 0)),
        ];
        for (t, want) in cases {
            assert_eq!(latest_slot_at_or_before(t), want, "{t}");
            // 換成 UTC 與日本時間的表示法，答案不變（時點用台北、不看輸入的偏移）。
            assert_eq!(
                latest_slot_at_or_before(t.to_offset(time::UtcOffset::UTC)),
                want
            );
            assert_eq!(
                latest_slot_at_or_before(t.to_offset(time::macros::offset!(+9))),
                want
            );
        }
    }

    #[test]
    fn latest_slot_uses_taipei_date_when_utc_and_taipei_dates_differ() {
        // UTC 10/04 17:00 ＝ 台北 10/05 01:00 → 時點是台北 10/05 00:00。
        let t = datetime!(2026-10-04 17:00:00 UTC);
        assert_eq!(latest_slot_at_or_before(t), tpe(2026, 10, 5, 0, 0, 0));
    }

    #[test]
    fn trigger_names_are_the_log_strings() {
        assert_eq!(Trigger::FirstFetch.as_str(), "first-fetch");
        assert_eq!(Trigger::Slot.as_str(), "slot");
        assert_eq!(Trigger::Missed.as_str(), "missed");
        assert_eq!(Trigger::Retry.as_str(), "retry");
    }

    // ───────────── 亂數 ─────────────

    #[test]
    fn pick_secs_hits_both_inclusive_bounds_and_never_exceeds_them() {
        for range in [
            SLOT_JITTER_SECS,
            MISSED_DELAY_SECS,
            FIRST_FETCH_DELAY_SECS,
            RETRY_BASE_SECS,
        ] {
            assert_eq!(pick_secs(&mut Fixed(LO), range), secs(range.0));
            assert_eq!(pick_secs(&mut Fixed(HI), range), secs(range.1));
            // 越界或非有限值夾回範圍內。
            assert_eq!(pick_secs(&mut Fixed(1.0), range), secs(range.1));
            assert_eq!(pick_secs(&mut Fixed(7.5), range), secs(range.1));
            assert_eq!(pick_secs(&mut Fixed(-3.0), range), secs(range.0));
            assert_eq!(pick_secs(&mut Fixed(f64::NAN), range), secs(range.0));
        }
    }

    #[test]
    fn pick_secs_is_uniform_over_the_integer_range() {
        // 5–15 秒共 11 個整數，每個整數對應 1/11 的區段。
        let mut seen = std::collections::BTreeSet::new();
        for i in 0..11 {
            let unit = (f64::from(i) + 0.5) / 11.0;
            seen.insert(pick_secs(&mut Fixed(unit), FIRST_FETCH_DELAY_SECS).whole_seconds());
        }
        assert_eq!(seen.len(), 11);
        assert_eq!((*seen.first().unwrap(), *seen.last().unwrap()), (5, 15));
    }

    #[test]
    fn system_rng_stays_in_unit_interval_and_varies() {
        let mut rng = SystemRng;
        let draws: Vec<f64> = (0..200).map(|_| rng.unit()).collect();
        assert!(draws.iter().all(|u| (0.0..1.0).contains(u)));
        let first = draws[0];
        assert!(
            draws.iter().any(|u| (*u - first).abs() > 1e-9),
            "亂數不可全相同"
        );
    }

    // ───────────── spec Scenario：以注入的時鐘與極值亂數驗時間上界 ─────────────

    /// spec「收盤後時點」＋「未錯過時點」：宿主在 13:00 重新啟動、上一輪 12:02 開始 → 不補抓；15:00 時點在
    /// 15:00–15:06 之間開始（亂數取下界與上界）。
    #[test]
    fn scenario_slot_1500_starts_between_1500_and_1506_and_nothing_before() {
        for unit in [LO, HI] {
            let mut sim = Sim::new(
                tpe(2026, 10, 5, 13, 0, 0),
                Some(ok_state(tpe(2026, 10, 5, 12, 2, 0))),
                true,
                unit,
            );
            sim.wake_latency = 0;
            sim.run_until(tpe(2026, 10, 5, 16, 30, 0));
            assert_eq!(sim.starts.len(), 1, "unit={unit} {:?}", sim.starts);
            let (at, trigger) = sim.starts[0];
            assert_eq!(trigger, Trigger::Slot, "unit={unit}");
            assert!(at >= tpe(2026, 10, 5, 15, 0, 0), "不得早於 15:00：{at}");
            assert!(at <= tpe(2026, 10, 5, 15, 6, 0), "不得晚於 15:06：{at}");
        }
    }

    #[test]
    fn scenario_slot_1500_bounds_are_exact_for_the_extreme_random_values() {
        // 醒來時刻剛好落在 :00 時，下界＝15:00:00、上界＝15:05:00（時點後 0–5 分鐘隨機）。
        let run = |unit| {
            let mut sim = Sim::new(
                tpe(2026, 10, 5, 14, 30, 0),
                Some(ok_state(tpe(2026, 10, 5, 13, 30, 0))),
                true,
                unit,
            );
            sim.run_until(tpe(2026, 10, 5, 16, 0, 0));
            sim.starts[0].0
        };
        assert_eq!(run(LO), tpe(2026, 10, 5, 15, 0, 0));
        assert_eq!(run(HI), tpe(2026, 10, 5, 15, 5, 0));
    }

    #[test]
    fn scenario_slot_1500_with_scheduler_latency_still_within_the_spec_bound() {
        // 醒來比預定晚 30 秒（排程延遲）：上界仍在 15:06 內。
        let mut sim = Sim::new(
            tpe(2026, 10, 5, 14, 0, 10),
            Some(ok_state(tpe(2026, 10, 5, 13, 59, 0))),
            true,
            HI,
        );
        sim.wake_latency = 30;
        sim.run_until(tpe(2026, 10, 5, 16, 0, 0));
        let (at, trigger) = sim.starts[0];
        assert_eq!(trigger, Trigger::Slot);
        assert!(
            at >= tpe(2026, 10, 5, 15, 0, 0) && at <= tpe(2026, 10, 5, 15, 6, 0),
            "{at}"
        );
    }

    /// spec「隔夜開機」：前一晚 22:00 關機、隔天 08:00 開機 → 啟動後 2 分鐘內開始補抓（錯過 00:00 與 06:00）。
    #[test]
    fn scenario_overnight_boot_catches_up_within_two_minutes() {
        for (unit, want) in [(LO, 30), (HI, 120)] {
            let boot = tpe(2026, 10, 5, 8, 0, 0);
            let mut sim = Sim::new(boot, Some(ok_state(tpe(2026, 10, 4, 22, 0, 0))), true, unit);
            sim.run_until(boot + Duration::minutes(5));
            let (at, trigger) = sim.starts[0];
            assert_eq!(trigger, Trigger::Missed, "unit={unit}");
            assert_eq!((at - boot).whole_seconds(), want, "unit={unit}");
            assert!(at - boot <= Duration::minutes(2), "啟動後 2 分鐘內：{at}");
            assert_eq!(sim.starts.len(), 1);
        }
    }

    /// spec「睡眠跨過時點」：11:30 睡、12:30 喚醒 → 喚醒後 4 分鐘內開始補抓。最壞情況：醒來延遲 59 秒
    /// （平常 60 秒一次，睡眠恢復後的第一次醒來最晚在恢復後 60 秒內）加上隨機延遲上界 120 秒。
    #[test]
    fn scenario_sleep_across_a_slot_catches_up_within_four_minutes_of_waking() {
        for unit in [LO, HI] {
            let mut sim = Sim::new(
                tpe(2026, 10, 5, 11, 0, 0),
                Some(ok_state(tpe(2026, 10, 5, 6, 3, 0))),
                true,
                unit,
            );
            sim.run_until(tpe(2026, 10, 5, 11, 30, 0));
            assert!(sim.starts.is_empty(), "睡前不該有補抓：{:?}", sim.starts);
            let resume = tpe(2026, 10, 5, 12, 30, 0);
            sim.jump_to(resume + secs(59));
            sim.run_until(resume + Duration::minutes(10));
            let (at, trigger) = sim.starts[0];
            assert_eq!(trigger, Trigger::Missed, "unit={unit}");
            assert!(at - resume < Duration::minutes(4), "喚醒後 4 分鐘內：{at}");
            assert_eq!(sim.starts.len(), 1, "{:?}", sim.starts);
        }
    }

    /// spec「喚醒緊接排程時點」：11:40 喚醒補抓，接著 12:00 時點 → 12:00 那輪延後到補抓開始 60 分鐘之後
    /// （補抓最早 11:40:30 開始，所以最早 12:40:30；「12:40 之後」）。
    #[test]
    fn scenario_wake_right_before_a_slot_defers_the_slot_round_by_the_minimum_interval() {
        for unit in [LO, HI] {
            let mut sim = Sim::new(
                tpe(2026, 10, 5, 5, 40, 0),
                Some(ok_state(tpe(2026, 10, 5, 5, 30, 0))),
                true,
                unit,
            );
            sim.run_until(tpe(2026, 10, 5, 5, 50, 0));
            sim.jump_to(tpe(2026, 10, 5, 11, 40, 0));
            sim.run_until(tpe(2026, 10, 5, 14, 0, 0));
            assert_eq!(sim.starts.len(), 2, "unit={unit} {:?}", sim.starts);
            let (catch_up, t1) = sim.starts[0];
            let (slot_round, t2) = sim.starts[1];
            assert_eq!(t1, Trigger::Missed);
            assert_eq!(t2, Trigger::Slot, "12:00 時點照樣排、只是延後");
            assert!(catch_up >= tpe(2026, 10, 5, 11, 40, 30));
            assert!(
                slot_round >= catch_up + MIN_INTERVAL,
                "12:00 那輪不得早於補抓開始＋60 分鐘：補抓 {catch_up}、時點輪 {slot_round}"
            );
            assert!(
                slot_round >= tpe(2026, 10, 5, 12, 40, 0),
                "12:40 之後：{slot_round}"
            );
        }
    }

    /// spec「首次安裝」：資料目錄沒有 tw_events.json → 啟動後 15 秒內開始（不計等網路的時間，等網路在一輪內）。
    #[test]
    fn scenario_first_install_starts_within_fifteen_seconds() {
        for (unit, want) in [(LO, 5), (HI, 15)] {
            let boot = tpe(2026, 10, 5, 9, 17, 0);
            let mut sim = Sim::new(boot, None, false, unit);
            sim.run_until(boot + Duration::minutes(2));
            let (at, trigger) = sim.starts[0];
            assert_eq!(trigger, Trigger::FirstFetch, "unit={unit}");
            assert_eq!((at - boot).whole_seconds(), want, "unit={unit}");
            assert!(at - boot <= secs(15));
        }
    }

    #[test]
    fn first_fetch_is_exempt_from_the_minimum_interval() {
        // 5 分鐘前才成功抓過，但使用者把資料目錄換到空資料夾 → 仍在 5–15 秒內抓（資料檔不存在不受下限）。
        let now = tpe(2026, 10, 5, 9, 17, 0);
        let mut sim = Sim::new(now, Some(ok_state(now - Duration::minutes(5))), false, HI);
        sim.run_until(now + Duration::minutes(1));
        assert_eq!(sim.starts[0], (now + secs(15), Trigger::FirstFetch));
    }

    /// spec「首次安裝時網路不通」：首輪所有來源失敗 → 10–15 分鐘後再抓一輪，不必等到下一個時點。
    #[test]
    fn scenario_first_install_with_no_network_retries_after_ten_to_fifteen_minutes() {
        for (unit, minutes) in [(LO, 10), (HI, 15)] {
            let boot = tpe(2026, 10, 5, 9, 17, 0);
            let mut sim = Sim::new(boot, None, false, unit);
            sim.round_ok = false;
            sim.run_until(boot + Duration::minutes(40));
            let (first, t0) = sim.starts[0];
            let (second, t1) = sim.starts[1];
            assert_eq!(t0, Trigger::FirstFetch);
            assert_eq!(t1, Trigger::Retry, "unit={unit}");
            let finish = first + secs(sim.round_secs);
            assert_eq!(second - finish, Duration::minutes(minutes), "unit={unit}");
            assert!(second < tpe(2026, 10, 5, 12, 0, 0), "不必等到下一個時點");
        }
    }

    #[test]
    fn consecutive_failures_double_the_retry_delay_up_to_sixty_minutes() {
        // 下界：10、20、40、60（80 被夾到 60）、60；上界：15、30、60（夾）、60。
        for (unit, want_minutes) in [(LO, [10, 20, 40, 60, 60]), (HI, [15, 30, 60, 60, 60])] {
            let boot = tpe(2026, 10, 5, 9, 17, 0);
            let mut sim = Sim::new(boot, None, false, unit);
            sim.round_ok = false;
            sim.run_until(boot + Duration::hours(10));
            assert!(sim.starts.len() >= 6, "{:?}", sim.starts);
            for (i, minutes) in want_minutes.iter().enumerate() {
                let (prev_start, _) = sim.starts[i];
                let (next_start, trigger) = sim.starts[i + 1];
                assert_eq!(trigger, Trigger::Retry, "unit={unit} i={i}");
                let gap = next_start - (prev_start + secs(sim.round_secs));
                assert_eq!(
                    gap,
                    Duration::minutes(*minutes),
                    "unit={unit} 第 {} 次重試",
                    i + 1
                );
            }
        }
    }

    #[test]
    fn retry_is_not_limited_by_the_sixty_minute_floor_and_success_stops_it() {
        let boot = tpe(2026, 10, 5, 9, 17, 0);
        let mut sim = Sim::new(boot, None, false, LO);
        sim.round_ok = false;
        sim.run_until(boot + Duration::minutes(12));
        assert_eq!(sim.starts.len(), 2, "10 分鐘後重試（遠小於 60 分鐘下限）");
        // 重試成功 → 連續失敗歸零，之後只等時點。
        sim.round_ok = true;
        sim.run_until(tpe(2026, 10, 5, 11, 59, 0));
        assert_eq!(
            sim.starts.len(),
            3,
            "第 2 次重試（20 分鐘）成功：{:?}",
            sim.starts
        );
        assert_eq!(sim.state.unwrap().consecutive_failures, 0);
        assert!(sim.state.unwrap().last_any_success);
        sim.run_until(tpe(2026, 10, 5, 11, 59, 0) + Duration::minutes(60));
        assert_eq!(
            sim.starts.last().unwrap().1,
            Trigger::Slot,
            "之後回到時點節奏"
        );
    }

    /// spec「抓取間隔下限」：下限內觸發的時點或補抓延後、不丟棄。
    #[test]
    fn a_slot_inside_the_minimum_interval_is_deferred_not_dropped() {
        // 14:30 才開始過一輪（例如補抓）：15:00 時點的那輪延後到 15:30。
        let mut sim = Sim::new(
            tpe(2026, 10, 5, 14, 40, 0),
            Some(ok_state(tpe(2026, 10, 5, 14, 30, 0))),
            true,
            LO,
        );
        sim.run_until(tpe(2026, 10, 5, 16, 0, 0));
        assert_eq!(
            sim.starts,
            vec![(tpe(2026, 10, 5, 15, 30, 0), Trigger::Slot)]
        );
    }

    /// 審查 m2：資料檔不存在時一律 5–15 秒，舊紀錄停在失敗狀態（而且重試早已逾期）也不能延後。
    #[test]
    fn an_old_failed_record_does_not_delay_the_first_fetch() {
        for (unit, want) in [(LO, 5), (HI, 15)] {
            let now = tpe(2026, 10, 5, 9, 0, 0);
            let long_ago = now - Duration::days(3);
            let failed = FetchState::after_round(None, long_ago, long_ago, false);
            let mut sim = Sim::new(now, Some(failed), false, unit);
            sim.round_ok = false;
            sim.run_until(now + Duration::minutes(2));
            assert_eq!(
                sim.starts[0],
                (now + secs(want), Trigger::FirstFetch),
                "unit={unit}"
            );
        }
    }

    /// 熱迴圈防護：寫檔失敗（資料檔始終不存在）時，不是每 5–15 秒首抓一次，而是進入 10／20／40／60 分鐘退避。
    #[test]
    fn a_write_failure_backs_off_instead_of_hammering_the_first_fetch() {
        let now = tpe(2026, 10, 5, 9, 0, 0);
        let mut sim = Sim::new(now, None, false, LO);
        sim.round_ok = false;
        sim.write_fails = true;
        sim.run_until(now + Duration::hours(4));
        assert_eq!(sim.starts[0].1, Trigger::FirstFetch);
        let gaps: Vec<i64> = sim
            .starts
            .windows(2)
            .map(|w| (w[1].0 - (w[0].0 + secs(sim.round_secs))).whole_minutes())
            .collect();
        assert_eq!(gaps[..3], [10, 20, 40], "{:?}", sim.starts);
        assert!(sim.starts[1..].iter().all(|(_, t)| *t == Trigger::Retry));
        assert!(sim.starts.len() <= 6, "4 小時內最多 6 輪：{:?}", sim.starts);
    }

    #[test]
    fn switching_to_a_new_data_dir_lifts_the_first_fetch_suppression() {
        let now = tpe(2026, 10, 5, 9, 0, 0);
        let failed = FetchState::after_round(None, now, now, false);
        let mut sched = Scheduler::default();
        sched.set_data_dir(std::path::Path::new("D:\\data-a"));
        sched.round_finished(now, false); // 寫檔失敗：檔案不存在
                                          // 同一個目錄：抑制首抓、走重試退避（10 分鐘後）。
        let step = sched.step(now, Some(&failed), false, &mut Fixed(LO));
        assert_eq!(step, Step::Sleep(WAKE_INTERVAL));
        assert_eq!(sched.pending().unwrap().trigger, Trigger::Retry);
        // 使用者換到另一個空資料夾：待啟動的重試作廢、5 秒後首抓。
        sched.set_data_dir(std::path::Path::new("D:\\data-b"));
        let step = sched.step(now + secs(60), Some(&failed), false, &mut Fixed(LO));
        assert_eq!(step, Step::Sleep(secs(5)));
        assert_eq!(sched.pending().unwrap().trigger, Trigger::FirstFetch);
    }

    /// 審查 m1：睡前排好的重試，恢復後不可立即開始，要重走 30–120 秒。
    #[test]
    fn a_pending_retry_is_replanned_after_the_machine_slept() {
        for (unit, want) in [(LO, 30), (HI, 120)] {
            let t0 = tpe(2026, 10, 5, 9, 0, 0);
            let failed = FetchState::after_round(None, t0 - Duration::minutes(1), t0, false);
            let mut sched = Scheduler::default();
            // 失敗輪結束、重試排在 10 分鐘後；接著機器睡眠，45 分鐘後第一次醒來。
            assert_eq!(
                sched.step(t0, Some(&failed), true, &mut Fixed(LO)),
                Step::Sleep(WAKE_INTERVAL)
            );
            assert_eq!(sched.pending().unwrap().due, t0 + Duration::minutes(10));
            let resume = t0 + Duration::minutes(45);
            let step = sched.step(resume, Some(&failed), true, &mut Fixed(unit));
            assert_eq!(
                step,
                Step::Sleep(secs(want).min(WAKE_INTERVAL)),
                "unit={unit}"
            );
            let p = sched.pending().expect("重新判定後仍有待啟動的一輪");
            assert_eq!(p.trigger, Trigger::Retry);
            assert_eq!(p.due, resume + secs(want), "unit={unit}");
        }
    }

    #[test]
    fn a_pending_slot_round_is_replanned_after_sleeping_through_the_deferral() {
        // 11:40 補抓後 12:00 時點輪被下限延後到 ≥12:41；機器在 12:05 睡眠、13:30 才醒：不可立即開始。
        let mut sim = Sim::new(
            tpe(2026, 10, 5, 5, 40, 0),
            Some(ok_state(tpe(2026, 10, 5, 5, 30, 0))),
            true,
            LO,
        );
        sim.run_until(tpe(2026, 10, 5, 5, 50, 0));
        sim.jump_to(tpe(2026, 10, 5, 11, 40, 0));
        sim.run_until(tpe(2026, 10, 5, 12, 5, 0));
        assert_eq!(
            sim.starts.len(),
            1,
            "補抓之後時點輪還在等下限：{:?}",
            sim.starts
        );
        let resume = tpe(2026, 10, 5, 13, 30, 0);
        sim.jump_to(resume);
        sim.run_until(resume + Duration::minutes(5));
        let (at, trigger) = sim.starts[1];
        assert_eq!(trigger, Trigger::Missed, "恢復後重新判定成錯過");
        assert_eq!(at, resume + secs(30), "30 秒（亂數下界），不是立即");
    }

    /// 審查 m3：待啟動的時點輪因下限延後期間，資料檔消失（換到空資料夾）→ 立刻首抓，不必等。
    #[test]
    fn a_deferred_slot_round_becomes_a_first_fetch_when_the_data_file_disappears() {
        let mut sched = Scheduler::default();
        let state = ok_state(tpe(2026, 10, 5, 14, 30, 0));
        let _ = sched.step(
            tpe(2026, 10, 5, 14, 59, 30),
            Some(&state),
            true,
            &mut Fixed(LO),
        );
        let _ = sched.step(
            tpe(2026, 10, 5, 15, 0, 30),
            Some(&state),
            true,
            &mut Fixed(LO),
        );
        let p = sched.pending().unwrap();
        assert_eq!(
            (p.trigger, p.due),
            (Trigger::Slot, tpe(2026, 10, 5, 15, 30, 0))
        );
        let step = sched.step(
            tpe(2026, 10, 5, 15, 1, 30),
            Some(&state),
            false,
            &mut Fixed(LO),
        );
        assert_eq!(step, Step::Sleep(secs(5)));
        assert_eq!(sched.pending().unwrap().trigger, Trigger::FirstFetch);
    }

    /// 審查 N2：停用期間（`idle_wake`）解除首抓抑制——停用再啟用之後，同一個空資料夾仍要在 5–15 秒內首抓。
    #[test]
    fn idle_wake_lifts_the_first_fetch_suppression() {
        let now = tpe(2026, 10, 5, 9, 0, 0);
        let mut sched = Scheduler::default();
        sched.set_data_dir(std::path::Path::new("D:\\data-a"));
        sched.round_finished(now, false); // 寫檔失敗 → 抑制首抓
        let suppressed = sched.step(now, None, false, &mut Fixed(LO));
        assert_eq!(suppressed, Step::Sleep(secs(30)), "抑制中：沒有紀錄走錯過");
        sched.idle_wake(now + secs(60)); // 抓取被關掉
        let step = sched.step(now + secs(120), None, false, &mut Fixed(LO));
        assert_eq!(step, Step::Sleep(secs(5)));
        assert_eq!(sched.pending().unwrap().trigger, Trigger::FirstFetch);
    }

    // ───────────── 紀錄損毀、時鐘倒退 ─────────────

    #[test]
    fn missing_or_corrupt_record_counts_as_missed() {
        let now = tpe(2026, 10, 5, 9, 17, 0);
        // 資料檔存在、沒有排程紀錄 → 錯過：30–120 秒。
        for (unit, want) in [(LO, 30), (HI, 120)] {
            let mut sim = Sim::new(now, None, true, unit);
            sim.run_until(now + Duration::minutes(5));
            assert_eq!(
                sim.starts[0],
                (now + secs(want), Trigger::Missed),
                "unit={unit}"
            );
        }
        // 兩者皆缺 → 走 5–15 秒的首抓。
        let mut sim = Sim::new(now, None, false, LO);
        sim.run_until(now + Duration::minutes(5));
        assert_eq!(sim.starts[0], (now + secs(5), Trigger::FirstFetch));
    }

    #[test]
    fn clock_going_backwards_treats_last_start_as_now_and_does_not_fetch_immediately() {
        // 紀錄說上一輪 3 小時後才開始（時鐘被調回）：視為現在 → 不立即抓、也沒有「錯過」。
        let now = tpe(2026, 10, 5, 9, 17, 0);
        let mut sim = Sim::new(now, Some(ok_state(now + Duration::hours(3))), true, LO);
        sim.run_until(now + Duration::minutes(10));
        assert!(sim.starts.is_empty(), "{:?}", sim.starts);
    }

    #[test]
    fn after_a_clock_set_back_the_minimum_interval_counts_from_now() {
        // 12:20 時鐘從 15:20 調回；紀錄的上一輪開始＝15:10。12:00 的時點在「現在」之前、不算錯過；
        // 15:00 時點到點時，下限＝（視為現在的 12:20）＋60 分鐘，早已過，所以正常在 15:00–15:05 開始。
        let now = tpe(2026, 10, 5, 12, 20, 0);
        let state = ok_state(tpe(2026, 10, 5, 15, 10, 0));
        let (clamped, changed) = state.clamped_to(now);
        assert!(changed);
        assert_eq!(clamped.last_start, now);
        assert_eq!(clamped.last_finish, Some(now));
        let mut sim = Sim::new(now, Some(state), true, LO);
        sim.run_until(tpe(2026, 10, 5, 16, 0, 0));
        assert_eq!(sim.starts.len(), 1, "{:?}", sim.starts);
        let (at, trigger) = sim.starts[0];
        assert_eq!(trigger, Trigger::Slot);
        assert!(
            at >= tpe(2026, 10, 5, 15, 0, 0) && at <= tpe(2026, 10, 5, 15, 6, 0),
            "{at}"
        );
    }

    #[test]
    fn clamped_to_leaves_a_sane_record_alone() {
        let now = tpe(2026, 10, 5, 12, 0, 0);
        let s = ok_state(now - Duration::hours(1));
        assert_eq!(s.clamped_to(now), (s, false));
    }

    #[test]
    fn clock_going_backwards_between_wakes_does_not_trigger_a_slot() {
        // 上次醒來 15:30、這次 12:10（時鐘調回）：沒有「跨過時點」。
        let state = ok_state(tpe(2026, 10, 5, 12, 5, 0));
        let wake = Wake {
            now: tpe(2026, 10, 5, 12, 10, 0),
            prev_wake: Some(tpe(2026, 10, 5, 15, 30, 0)),
            state: Some(&state),
            data_file_exists: true,
            first_fetch_suppressed: false,
        };
        assert_eq!(evaluate(&wake, &mut Fixed(LO)), None);
    }

    #[test]
    fn a_pending_start_far_in_the_future_is_dropped_when_the_clock_jumps_back() {
        let mut sched = Scheduler::default();
        let t0 = tpe(2026, 10, 5, 8, 0, 0);
        // 先排一輪 5–15 秒後的首抓，再把時鐘往回調 5 小時（待啟動時刻離現在 > 2 小時）。
        let state = ok_state(t0 - Duration::hours(3));
        assert!(matches!(
            sched.step(t0, Some(&state), false, &mut Fixed(HI)),
            Step::Sleep(_)
        ));
        assert!(sched.pending().is_some());
        let back = t0 - Duration::hours(5);
        let step = sched.step(
            back,
            Some(&ok_state(back - Duration::hours(1))),
            true,
            &mut Fixed(HI),
        );
        assert_eq!(step, Step::Sleep(WAKE_INTERVAL));
        assert_eq!(sched.pending(), None);
    }

    // ───────────── 到點／錯過的邊界 ─────────────

    #[test]
    fn on_time_requires_a_gap_of_at_most_three_minutes() {
        let slot_now = tpe(2026, 10, 5, 12, 0, 30);
        let state = ok_state(tpe(2026, 10, 5, 6, 0, 10));
        let eval = |gap_secs: i64| {
            evaluate(
                &Wake {
                    now: slot_now,
                    prev_wake: Some(slot_now - secs(gap_secs)),
                    state: Some(&state),
                    data_file_exists: true,
                    first_fetch_suppressed: false,
                },
                &mut Fixed(LO),
            )
            .unwrap()
        };
        // 兩次醒來相隔恰好 3 分鐘（跨過 12:00）＝到點；多 1 秒＝睡眠／卡住＝錯過。
        assert_eq!(eval(180).trigger, Trigger::Slot);
        assert_eq!(eval(181).trigger, Trigger::Missed);
        assert_eq!(eval(60).trigger, Trigger::Slot);
        // 到點的開始時刻是時點（亂數取下界）；錯過是現在＋30 秒。
        assert_eq!(eval(60).due, tpe(2026, 10, 5, 12, 0, 0));
        assert_eq!(eval(181).due, slot_now + secs(30));
    }

    #[test]
    fn a_slot_that_was_already_passed_at_the_previous_wake_is_not_on_time() {
        // 上次醒來 12:00:00 整（時點「<=」上次醒來）→ 這次 12:01 不算跨過；紀錄也是 12:00 之前開始 → 錯過規則。
        let state = ok_state(tpe(2026, 10, 5, 6, 0, 10));
        let w = Wake {
            now: tpe(2026, 10, 5, 12, 1, 0),
            prev_wake: Some(tpe(2026, 10, 5, 12, 0, 0)),
            state: Some(&state),
            data_file_exists: true,
            first_fetch_suppressed: false,
        };
        assert_eq!(
            evaluate(&w, &mut Fixed(LO)).unwrap().trigger,
            Trigger::Missed
        );
    }

    #[test]
    fn no_slot_since_the_last_start_means_no_round() {
        // 上一輪 12:02 開始，現在 14:59：最近的時點 12:00 在上一輪之前 → 不用。
        let state = ok_state(tpe(2026, 10, 5, 12, 2, 0));
        for prev_wake in [None, Some(tpe(2026, 10, 5, 14, 58, 0))] {
            let w = Wake {
                now: tpe(2026, 10, 5, 14, 59, 0),
                prev_wake,
                state: Some(&state),
                data_file_exists: true,
                first_fetch_suppressed: false,
            };
            assert_eq!(evaluate(&w, &mut Fixed(LO)), None);
        }
    }

    #[test]
    fn a_slot_round_whose_random_start_already_passed_starts_at_the_next_step() {
        // 到點、亂數 0 → 開始時刻＝時點本身，已在 30 秒前；下一次 step 立即開始。
        let mut sched = Scheduler::default();
        let state = ok_state(tpe(2026, 10, 5, 13, 0, 0));
        let _ = sched.step(
            tpe(2026, 10, 5, 14, 59, 30),
            Some(&state),
            true,
            &mut Fixed(LO),
        );
        let step = sched.step(
            tpe(2026, 10, 5, 15, 0, 30),
            Some(&state),
            true,
            &mut Fixed(LO),
        );
        assert_eq!(step, Step::Start(Trigger::Slot));
    }

    #[test]
    fn host_starting_just_after_a_slot_counts_as_missed_not_on_time() {
        // 15:00:30 啟動（沒有上次醒來）、上一輪 12:10：錯過 → 30–120 秒；下限 13:10 早已過。
        let now = tpe(2026, 10, 5, 15, 0, 30);
        let state = ok_state(tpe(2026, 10, 5, 12, 10, 0));
        let w = Wake {
            now,
            prev_wake: None,
            state: Some(&state),
            data_file_exists: true,
            first_fetch_suppressed: false,
        };
        let p = evaluate(&w, &mut Fixed(LO)).unwrap();
        assert_eq!((p.trigger, p.due), (Trigger::Missed, now + secs(30)));
    }

    #[test]
    fn overdue_retry_after_a_long_shutdown_uses_the_missed_delay() {
        let now = tpe(2026, 10, 5, 9, 0, 0);
        let failed = FetchState::after_round(
            None,
            now - Duration::hours(30),
            now - Duration::hours(30),
            false,
        );
        let w = Wake {
            now,
            prev_wake: None,
            state: Some(&failed),
            data_file_exists: true,
            first_fetch_suppressed: false,
        };
        let p = evaluate(&w, &mut Fixed(HI)).unwrap();
        assert_eq!((p.trigger, p.due), (Trigger::Retry, now + secs(120)));
    }

    // ───────────── Scheduler.step 的睡眠長度 ─────────────

    #[test]
    fn step_sleeps_at_most_sixty_seconds_and_wakes_exactly_at_a_pending_start() {
        let mut sched = Scheduler::default();
        let now = tpe(2026, 10, 5, 9, 0, 0);
        let state = ok_state(now - Duration::minutes(10));
        // 沒事：睡 60 秒。
        assert_eq!(
            sched.step(now, Some(&state), true, &mut Fixed(LO)),
            Step::Sleep(WAKE_INTERVAL)
        );
        // 資料檔不存在：首抓在 5 秒後 → 睡 5 秒（不是 60 秒），醒來就開始。
        let mut sched = Scheduler::default();
        assert_eq!(
            sched.step(now, Some(&state), false, &mut Fixed(LO)),
            Step::Sleep(secs(5))
        );
        assert_eq!(
            sched.step(now + secs(5), Some(&state), false, &mut Fixed(LO)),
            Step::Start(Trigger::FirstFetch)
        );
        assert_eq!(sched.pending(), None);
    }

    #[test]
    fn pending_start_is_not_redrawn_on_every_wake() {
        // 到點的 0–5 分鐘隨機：抽一次就固定。第一次抽到上界，之後即使亂數改成下界，開始時刻不變。
        let mut sched = Scheduler::default();
        let state = ok_state(tpe(2026, 10, 5, 13, 0, 0));
        let _ = sched.step(
            tpe(2026, 10, 5, 14, 59, 30),
            Some(&state),
            true,
            &mut Fixed(LO),
        );
        let _ = sched.step(
            tpe(2026, 10, 5, 15, 0, 30),
            Some(&state),
            true,
            &mut Fixed(HI),
        );
        let due = sched.pending().unwrap().due;
        assert_eq!(due, tpe(2026, 10, 5, 15, 5, 0));
        let _ = sched.step(
            tpe(2026, 10, 5, 15, 1, 30),
            Some(&state),
            true,
            &mut Fixed(LO),
        );
        assert_eq!(sched.pending().unwrap().due, due);
    }

    #[test]
    fn idle_wake_drops_pending_and_catches_up_later_via_the_missed_rule() {
        let mut sched = Scheduler::default();
        let state = ok_state(tpe(2026, 10, 5, 13, 0, 0));
        let _ = sched.step(
            tpe(2026, 10, 5, 14, 59, 30),
            Some(&state),
            true,
            &mut Fixed(LO),
        );
        let _ = sched.step(
            tpe(2026, 10, 5, 15, 0, 30),
            Some(&state),
            true,
            &mut Fixed(HI),
        );
        assert!(sched.pending().is_some());
        // 抓取被關掉：丟掉待啟動的一輪。
        sched.idle_wake(tpe(2026, 10, 5, 15, 1, 30));
        assert_eq!(sched.pending(), None);
        // 之後（≥ 3 分鐘後）重新啟用：時點已過、上一輪仍是 13:00 → 錯過 → 補抓。
        let step = sched.step(
            tpe(2026, 10, 5, 16, 0, 0),
            Some(&state),
            true,
            &mut Fixed(LO),
        );
        assert_eq!(step, Step::Sleep(secs(30)));
        assert_eq!(sched.pending().unwrap().trigger, Trigger::Missed);
    }

    // ───────────── 紀錄的更新 ─────────────

    #[test]
    fn after_round_counts_failures_and_resets_on_success() {
        let t = tpe(2026, 10, 5, 9, 0, 0);
        let f1 = FetchState::after_round(None, t, t + secs(40), false);
        assert_eq!((f1.consecutive_failures, f1.last_any_success), (1, false));
        let f2 = FetchState::after_round(Some(&f1), t, t + secs(40), false);
        assert_eq!(f2.consecutive_failures, 2);
        let ok = FetchState::after_round(Some(&f2), t, t + secs(40), true);
        assert_eq!((ok.consecutive_failures, ok.last_any_success), (0, true));
        assert_eq!(ok.last_start, t);
        assert_eq!(ok.last_finish, Some(t + secs(40)));
        // 連續失敗計數不溢位。
        let max = FetchState {
            consecutive_failures: u32::MAX,
            ..f2
        };
        assert_eq!(
            FetchState::after_round(Some(&max), t, t, false).consecutive_failures,
            u32::MAX
        );
    }

    #[test]
    fn retry_delay_is_capped_even_for_absurd_failure_counts() {
        for failures in [1u32, 2, 3, 4, 10, 100, u32::MAX] {
            let d = retry_delay(failures, &mut Fixed(HI));
            assert!(d <= RETRY_CAP, "{failures} → {d}");
            assert!(d >= Duration::minutes(10), "{failures} → {d}");
        }
    }
}
