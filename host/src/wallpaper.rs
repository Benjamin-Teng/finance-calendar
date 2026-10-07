//! 動態桌布排程器（dynamic-wallpaper task 4.3；design.md D6；specs/desktop-wallpaper「重畫時機」
//! 「兩個重畫時點之間不渲染」「暫停更新」「首次安裝無資料時不接管」）。
//!
//! 本模組只有**純函式**：[`next_action`] 依輸入決定「現在要不要重畫、重畫哪幾台、下一次何時再
//! 評估」。不讀時鐘、不讀檔案、不碰 Win32、不開執行緒；時間、本機時區、主題、暫停原因、資料
//! 日期、螢幕清單全部由參數帶入。計時器、事件接線屬 task 4.7，渲染屬 4.5，狀態檔屬 4.4。
//!
//! ## 呼叫端契約（4.7）
//!
//! 1. 啟動時建立空的 [`SchedulerState`]（**不持久化**：宿主每次啟動都從空狀態開始，所以
//!    「啟動」不需要另外的事件——每台螢幕都還沒有重畫紀錄，自然全部重畫；當機後重啟亦同）。
//! 2. 任何喚醒（計時器到點、資料通道變更、螢幕／解析度／DPI 變更、暫停原因變化、設定變更）
//!    都只做一件事：組好 [`SchedulerInput`] 呼叫 [`next_action`]。
//! 3. 先 [`SchedulerState::apply_forget`]（移除已拔除螢幕或「不接管」時的全部紀錄），再依
//!    [`Decision::action`] 行動；[`Action::Redraw`] 中每台成功設定後呼叫
//!    [`SchedulerState::record_drawn`]、失敗後呼叫 [`SchedulerState::record_failed`]，並指明
//!    失敗種類（[`FailureKind`]）：
//!    - 渲染失敗（頁面出錯、逾時、沒有 PNG）＝[`FailureKind::Render`]，由 4.5 判定，依
//!      [`retry_delay`] 退避重試；
//!    - 設定桌布逾時（explorer 卡住）＝[`FailureKind::Apply`]，由 4.7 判定，不退避，於下一個時點
//!      （或設定路徑恢復時）再試；
//!    - 設定桌布或設定前讀回時 explorer 有回應但回錯、或螢幕清單不是最新＝
//!      [`FailureKind::ApplyError`]，由 4.7 判定，依 [`retry_delay`] 退避重試（不等下一個時點）。
//!      沒有到期的重試以 [`Decision::pending_retries`] 列出，呼叫端不得把它記成單純的「已是最新」。
//!
//!    - COM 執行緒忙碌、渲染前預檢跳過（見下）＝[`FailureKind::Busy`]，由 4.7 判定，時點語意與
//!      `Apply` 相同，但不動連續渲染失敗次數（沒有渲染）。
//!
//!    設定桌布失敗相關的 4.7 義務：
//!    - COM 執行緒逾時後忙碌旗標解除、或處理完 explorer 重新啟動時，呼叫
//!      [`SchedulerState::apply_path_recovered`]（「設定路徑恢復」事件）再評估一次：上次設定
//!      失敗（或預檢跳過）的螢幕重試，**每台每個時點最多一次**——explorer 反覆「逾時後才返回」
//!      時不會形成「渲染 → 逾時 → 恢復 → 再渲染」的循環。
//!    - **開始渲染之前先查 COM 執行緒的忙碌旗標**（`WallpaperService::is_busy`）：忙碌時不渲染、
//!      不呼叫桌布 API，直接以 [`FailureKind::Busy`] 記錄，避免 explorer 卡住期間白跑渲染。
//!    - **每一次預檢跳過都必須寫記錄檔**（螢幕、時點、忙碌從何時開始）：這條路徑沒有 API 呼叫，
//!      「桌布 API 呼叫結果」的記錄不會涵蓋它，而 spec「explorer 無回應」要求放棄並記錄。
//!    - [`SchedulerState::consecutive_busy_skip_slots`] 從低於 [`BUSY_SKIP_WARN_SLOTS`] 變成
//!      達到它時（連續的預檢跳過跨了至少 3 個時點），**記一行 warn 層級的警告**（長期卡住的訊號；
//!      同一段卡住只記一次，計數歸零後再達到才再記）。
//!
//!    `now` 傳**判定失敗的那一刻**，不是做出決策的時刻。**每台都必須記錄其一**：沒有記錄的
//!    螢幕下次評估仍是「沒有紀錄」，會被一再列入。失敗的螢幕之後只在渲染失敗的重試時刻、設定路徑
//!    恢復事件、下一個時點、主題或出圖條件改變時才再列入，其他事件不會讓它在兩個時點之間重畫。
//! 4. **執行 `Redraw` 期間（渲染是非同步的，單台最長約 30 秒＋設定逾時）到達的喚醒，只標記
//!    「待評估」，不得呼叫 [`next_action`] 再派發渲染**——此時結果還沒記錄，再評估會拿到同一份
//!    `Redraw`，同一台螢幕被並行渲染兩次。執行完、每台都記錄之後，把期間累積的喚醒合併成
//!    **一次**評估（這是 4.7 的義務）。合併可能吞掉短暫的「不接管」，所以 4.4 每次還原原桌布或
//!    讓位之後**必須**呼叫 [`SchedulerState::reset`]，不能依賴評估到 [`Action::Idle`]。
//!
//!    **還原＋`reset` 必須等進行中的 `Redraw` 結束（或先中止它）之後才執行**（4.4／4.7 的 I/O
//!    層義務）：中止時，還沒送出 `SetWallpaper` 的螢幕不得再送。排程器擋不住已經送出的
//!    `SetWallpaper`——它排在還原之後執行就會蓋掉剛還原的原桌布。排程器這一側只保證紀錄不被寫回：
//!    每份 [`RedrawPlan`] 帶著做出時的世代，`reset` 遞增世代，過時計畫的
//!    [`SchedulerState::record_drawn`]／[`SchedulerState::record_failed`] 一律忽略（回傳 `false`）。
//! 5. 執行完 [`Action::Redraw`]、記錄完每台的結果後，**立即再呼叫一次** [`next_action`]（以
//!    當下的 `now`），計時器用這一次的 [`Decision::next_wake`]——`Redraw` 那次的 `next_wake`
//!    不知道這次的成敗，不含失敗後的重試時刻。以同一個 `now` 再評估不會再回傳 `Redraw`；以較新
//!    的 `now` 再評估**可能合法地回傳 `Redraw`**（執行期間跨過時點邊界、某台的重試時刻已到），
//!    照常執行即可，不是錯誤。這不會形成忙迴圈：每次執行都會記錄，重試時刻嚴格往後、間隔遞增。
//! 6. 依 [`Decision::next_wake`] 設**單一**計時器（`None`＝沒有時間觸發，只等事件）。計時器
//!    提早到點無害（[`next_action`] 會回 [`Action::UpToDate`] 並給新的喚醒時刻）。
//! 7. explorer 資源安全閥（task 4.9；design.md D11）：評估結果是 [`Action::Redraw`] 時才讀一次
//!    explorer 的 GDI 物件數，以 [`ExplorerResources::Sampled`]（讀取失敗＝
//!    [`ExplorerResources::ReadFailed`]）再評估一次。[`Decision::rebaseline`] 有值就寫進狀態檔；
//!    [`Action::StopTakeover`] 就不渲染、不設定，走「不接管」的還原路徑（安全閥原因）。
//!
//! ## 為什麼不用「錯過的時點」清單
//!
//! 每台螢幕記錄「上次畫的是哪一個時點／哪一天／哪一份資料」（[`Stamp`]）。評估時算出「現在應該
//! 呈現的時點」，比紀錄新就重畫——暫停（或睡眠，期間行程根本不執行）錯過幾個時點都一樣只差
//! 一個比較，恢復後的第一次評估只補畫**一次**，而且畫的是最近的時點（spec「解鎖後補畫」：
//! 10:05 鎖定、11:20 解鎖 → 畫 11:15）。暫停期間不需要另外累積任何東西。
//!
//! ## 本機時間與夏令時間
//!
//! 06:00、本機換日、0／15／30／45 分都以本機時間為準。本機時區由 [`UtcOffsetSource`] 帶入
//! （任意 UTC 時刻 → 當時的 UTC 偏移秒數），測試注入固定偏移或模擬的夏令規則，不依賴執行機器的
//! 時區。時間類時點一律以「新於紀錄」（`>`）比較，不是「不同」：夏令時間結束、本機時間倒退
//! 一小時時，日期鍵或時點鍵可能比紀錄舊，此時不重畫，避免同一時點畫兩次。下一次喚醒時刻以
//! 二分搜尋找「時點鍵開始變新」的最早秒數，跨越偏移切換也不會晚到（見 [`next_change`]）。

// 呼叫端（計時器、事件接線）在 task 4.7。
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};

use crate::settings::WallpaperTheme;
use crate::widgets::PauseReason;

/// UNIX 時間（UTC，秒）。
pub type UnixSeconds = i64;

const SECS_PER_DAY: i64 = 86_400;
/// 星盤的重畫間隔（15 分鐘）。
const ASTROLABE_SLOT_SECS: i64 = 15 * 60;
/// 脊線、等高線每日重畫的本機時刻（06:00）。
const DAILY_REDRAW_SECS_OF_DAY: i64 = 6 * 3600;

/// 本機時區：任意 UTC 時刻當下的 UTC 偏移（秒，東正西負）。4.7 以 Windows 的時區資訊實作；
/// 測試注入固定偏移或模擬的夏令規則。
pub trait UtcOffsetSource {
    fn offset_at(&self, utc: UnixSeconds) -> i32;
}

/// 固定 UTC 偏移（沒有夏令時間的時區，例如台北 +08:00＝`FixedOffset(8 * 3600)`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FixedOffset(pub i32);

impl UtcOffsetSource for FixedOffset {
    fn offset_at(&self, _utc: UnixSeconds) -> i32 {
        self.0
    }
}

/// 民曆日期（前推格里曆），可比較先後。資料層的日期字串 `YYYY-MM-DD` 以 [`CivilDate::parse_iso`]
/// 轉成本型別。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CivilDate {
    pub year: i32,
    pub month: u8,
    pub day: u8,
}

impl CivilDate {
    /// 1970-01-01 起算的日數 → 日期（Howard Hinnant 的 `civil_from_days`，以 3 月為年首、
    /// 400 年為一個循環）。年份超出 `i32` 的日數不會出現在實務輸入中。
    pub fn from_days(days: i64) -> Self {
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097); // [0, 146096]
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
        let mp = (5 * doy + 2) / 153; // [0, 11]，0＝3 月
        let day = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
        let month = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
        let year = yoe + era * 400 + i64::from(month <= 2);
        CivilDate {
            year: year as i32,
            month: month as u8,
            day: day as u8,
        }
    }

    /// 日期 → 1970-01-01 起算的日數（Howard Hinnant 的 `days_from_civil`）。
    pub fn to_days(self) -> i64 {
        let month = i64::from(self.month);
        let year = i64::from(self.year) - i64::from(month <= 2);
        let era = year.div_euclid(400);
        let yoe = year.rem_euclid(400); // [0, 399]
        let mp = if month > 2 { month - 3 } else { month + 9 }; // [0, 11]
        let doy = (153 * mp + 2) / 5 + i64::from(self.day) - 1; // [0, 365]
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
        era * 146_097 + doe - 719_468
    }

    /// 解析 `YYYY-MM-DD`（固定 10 字元、真實存在的日期）；其他格式回 `None`。
    pub fn parse_iso(s: &str) -> Option<Self> {
        let b = s.as_bytes();
        if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
            return None;
        }
        let number = |range: std::ops::Range<usize>| -> Option<u32> {
            b[range].iter().try_fold(0u32, |acc, &c| {
                c.is_ascii_digit().then(|| acc * 10 + u32::from(c - b'0'))
            })
        };
        let year = i32::try_from(number(0..4)?).ok()?;
        let month = u8::try_from(number(5..7)?).ok()?;
        let day = u8::try_from(number(8..10)?).ok()?;
        if !(1..=12).contains(&month) || day == 0 || day > days_in_month(year, month) {
            return None;
        }
        Some(CivilDate { year, month, day })
    }
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        2 if is_leap_year(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// 一台螢幕的出圖條件。任一欄改變（解析度、縮放比例）都要重畫這台。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorGeometry {
    /// 實體像素寬。
    pub width: u32,
    /// 實體像素高。
    pub height: u32,
    /// 縮放比例對應的 DPI（96＝100%、144＝150%、168＝175%）。
    pub dpi: u32,
}

/// 一台**在線**螢幕（離線、已拔除仍被列舉的螢幕由呼叫端濾掉，不放進輸入）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorSnapshot {
    /// `IDesktopWallpaper` 的裝置路徑（`SetWallpaper` 的 monitorID），用來識別同一台螢幕。
    pub device_path: String,
    pub geometry: MonitorGeometry,
}

/// 各資料鍵目前的日期（`tw-events` 快照擷取；鍵不存在或日期無法解析＝`None`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DataDates {
    /// `twii_intraday.date`（脊線、等高線必需）。
    pub twii_intraday: Option<CivilDate>,
    /// `twii_daily` 最後一筆的日期（天際線必需，也是天際線的重畫觸發）。
    pub twii_daily: Option<CivilDate>,
    /// `margin.date`（撕日曆的重畫觸發之一；缺漏時頁面自行略過，不必等待）。
    pub margin: Option<CivilDate>,
}

/// 所選主題缺少的資料鍵（[`Action::WaitingForData`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataKey {
    TwiiIntraday,
    TwiiDaily,
}

/// 安全閥門檻：讀值比基準多出**超過**這個數就停止接管（design.md D11；增量恰好 2,000 不觸發）。
pub const EXPLORER_GDI_DELTA_LIMIT: u32 = 2_000;
/// 安全閥門檻：讀值本身**超過**這個數就停止接管（單一行程 GDI 上限 10,000；恰好 8,000 不觸發）。
pub const EXPLORER_GDI_ABSOLUTE_LIMIT: u32 = 8_000;

/// 一次 explorer GDI 物件數讀值（殼層視窗擁有者的 PID＋`GetGuiResources(GR_GDIOBJECTS)`）。
/// 也是基準的形式：基準＝某一次讀值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExplorerSample {
    pub pid: u32,
    pub gdi: u32,
}

/// explorer 資源安全閥的輸入（task 4.9；design.md D11）。
///
/// 判定在 [`next_action`] 中標記「4.9 安全閥」的位置：只在本次評估確定要重畫、即將設定桌布之前
/// 判定（spec「每次設定桌布前讀取」）。呼叫端（4.7 協調迴圈）的做法：先以
/// [`ExplorerResources::NotSampled`] 評估，結果是 [`Action::Redraw`] 才讀一次 explorer、以
/// [`ExplorerResources::Sampled`]／[`ExplorerResources::ReadFailed`] 再評估一次（純函式，同一份
/// 輸入結果相同）——主題為「不接管」、暫停、不需重畫時都不讀。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExplorerResources {
    /// 本次沒有讀值：不判定，照常設定。
    #[default]
    NotSampled,
    /// 讀取失敗（殼層視窗不存在、`OpenProcess` 或 `GetGuiResources` 失敗）：spec 要求照常設定、
    /// **不**停止接管；記錄由呼叫端負責。行為與 `NotSampled` 相同，分開是為了讓意圖可測。
    ReadFailed,
    /// 讀到了。`baseline`＝狀態檔記錄的基準；**未接管時呼叫端傳 `None`**（接管開始時重設基準，
    /// 不沿用上一次接管留下的值）。使用者重新開啟接管時，呼叫端先清掉狀態檔的基準，這裡也是 `None`。
    Sampled {
        reading: ExplorerSample,
        baseline: Option<ExplorerSample>,
    },
}

/// 為什麼重設基準（[`ExplorerRebaseline`]；記錄用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebaselineCause {
    /// 沒有基準：接管開始、使用者重新開啟接管、或舊狀態檔沒有這個欄位。
    NoBaseline,
    /// explorer 重新啟動（PID 改變）：以重新啟動後第一次的讀值為新基準（spec Scenario「explorer
    /// 重新啟動」）。
    PidChanged { previous: ExplorerSample },
}

/// 本次評估決定的新基準：呼叫端寫進狀態檔（[`Decision::rebaseline`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExplorerRebaseline {
    pub baseline: ExplorerSample,
    pub cause: RebaselineCause,
}

/// 安全閥觸發（[`Action::StopTakeover`]）：讀值、判定用的基準與超過的門檻。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SafetyValveTrip {
    pub reading: ExplorerSample,
    /// 判定用的基準（同一個 PID 的基準）；沒有基準（接管開始、PID 剛改變）而只因絕對值觸發時為
    /// `None`。
    pub baseline: Option<ExplorerSample>,
    /// 增量超過 [`EXPLORER_GDI_DELTA_LIMIT`]。
    pub delta_exceeded: bool,
    /// 讀值超過 [`EXPLORER_GDI_ABSOLUTE_LIMIT`]。
    pub absolute_exceeded: bool,
}

impl SafetyValveTrip {
    /// 讀值比基準多出多少（沒有基準時 `None`；讀值比基準少時為負）。
    pub fn delta(&self) -> Option<i64> {
        self.baseline
            .map(|b| i64::from(self.reading.gdi) - i64::from(b.gdi))
    }

    /// 給使用者與記錄的說明（`RestoreReason::SafetyValve` 的 `detail`、系統匣通知的原因）：讀值、
    /// 基準、PID 與門檻。4.8 在通知中另外加上「可在設定中重新開啟」。
    pub fn detail(&self) -> String {
        let reading = self.reading;
        let what = match (self.baseline, self.delta()) {
            (Some(b), Some(delta)) => format!(
                "explorer（PID {}）的 GDI 物件數 {}，比基準 {} 多出 {delta}",
                reading.pid, reading.gdi, b.gdi
            ),
            _ => format!(
                "explorer（PID {}）的 GDI 物件數 {}（尚無基準）",
                reading.pid, reading.gdi
            ),
        };
        let exceeded = match (self.delta_exceeded, self.absolute_exceeded) {
            (true, true) => "增量與絕對值都超過門檻",
            (true, false) => "增量超過門檻",
            (false, true) => "絕對值超過門檻",
            (false, false) => "未超過門檻",
        };
        format!(
            "{what}；{exceeded}（門檻：增量超過 {EXPLORER_GDI_DELTA_LIMIT} 或絕對值超過 {EXPLORER_GDI_ABSOLUTE_LIMIT}）"
        )
    }
}

/// [`next_action`] 的全部輸入。
pub struct SchedulerInput<'a> {
    /// 現在（UTC）。
    pub now: UnixSeconds,
    /// 本機時區。
    pub tz: &'a dyn UtcOffsetSource,
    /// 設定中的桌布主題。
    pub theme: WallpaperTheme,
    /// 目前成立的暫停原因（`AppState::pause_reasons`）。電池／省電兩個原因在
    /// `crate::widgets` 已依 `PauseRules` 過濾（`widgets::battery_pause_active`／
    /// `power_saver_pause_active`）——規則關閉時根本不會出現在集合裡——本模組不再重複判定。
    pub pause: &'a HashSet<PauseReason>,
    /// 各資料鍵的日期。
    pub data: DataDates,
    /// 目前在線的螢幕。
    pub monitors: &'a [MonitorSnapshot],
    /// 宿主是否已接管過桌布（4.4 狀態檔記錄了原桌布、且已設定過宿主的圖）。宿主重啟後
    /// [`SchedulerState`] 是空的，靠這個欄位分辨「從未接管」與「接管中缺資料」；本次執行中只要
    /// 曾有任何一台成功畫過（之後即使失敗），也視為已接管，直到切成「不接管」（見
    /// [`Action::HoldWithoutData`]）。4.4 要定義旗標何時變回假（還原、讓位之後）。
    pub already_taken_over: bool,
    /// explorer 資源安全閥讀值（4.9；見 [`ExplorerResources`]）。
    pub explorer: ExplorerResources,
}

/// 某次重畫「呈現的是哪一個時點」，存在每台螢幕的紀錄裡，用來判斷是否過時。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stamp {
    /// 星盤：15 分鐘時點的開始時刻（UTC）。
    Slot(UnixSeconds),
    /// 脊線、等高線：本機 06:00 已過的那一天（本機時間減 6 小時後的日期）。
    Day(CivilDate),
    /// 天際線：`twii_daily` 最後一筆的日期。
    Daily(CivilDate),
    /// 撕日曆：本機日期＋`margin.date`。
    Tearoff {
        date: CivilDate,
        margin: Option<CivilDate>,
    },
}

/// 某台螢幕為什麼要重畫（記錄檔用；4.7 要記「排程決策」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedrawReason {
    /// 這台還沒有重畫紀錄：宿主剛啟動、螢幕剛接上，或從「不接管」切回某個主題。
    Unrecorded,
    /// 主題與上次重畫時不同。
    ThemeChanged,
    /// 解析度或縮放比例與上次重畫時不同。
    GeometryChanged,
    /// 主題節奏的時點到了（含暫停期間錯過、恢復後補畫的那一次）。
    Scheduled,
    /// 上次渲染失敗、重試時刻已到（[`FailureKind::Render`]、[`retry_delay`]），或上次設定失敗
    /// （或預檢跳過）、之後收到設定路徑恢復事件（[`SchedulerState::apply_path_recovered`]）。
    RetryAfterFailure,
}

/// 要重畫的一台螢幕。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedrawTarget {
    pub monitor: MonitorSnapshot,
    pub reason: RedrawReason,
}

/// 一次重畫的內容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedrawPlan {
    pub theme: WallpaperTheme,
    /// 只列受影響的螢幕。
    pub targets: Vec<RedrawTarget>,
    /// 寫進紀錄的時點（[`SchedulerState::record_drawn`]／[`SchedulerState::record_failed`]）。
    pub stamp: Stamp,
    /// 圖要呈現的時刻（UTC）：星盤＝15 分鐘時點的開始（補畫時是最近的時點，例如 11:20 解鎖 →
    /// 11:15），其餘主題＝現在。
    pub as_of: UnixSeconds,
    /// 做出這份計畫時排程器的世代（[`SchedulerState::reset`] 遞增）。`reset` 之後才回報的結果
    /// 一律忽略，見模組文件「呼叫端契約」第 4 點。不公開：呼叫端不得自行改寫。
    generation: u64,
}

/// 排程決策。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// 主題為「不接管」：不動作。同一個 [`Decision`] 的 `forget` 會列出全部紀錄，切回某個主題
    /// 時每台都重畫。讓位與還原屬 4.4 的狀態機。
    Idle,
    /// 所選主題需要的資料還不存在：不重畫、不接管，設定視窗顯示「等待資料中」。
    WaitingForData { missing: DataKey },
    /// 已接管、但所選主題需要的資料鍵不見了（資料目錄改指、檔案被舊格式覆蓋……）：保留目前的
    /// 桌布、不重畫、**不還原、不讓位**，呼叫端記錄一行警告。與 [`Action::WaitingForData`]
    /// （從未接管）分開，避免 4.4／4.8 把它當成「未接管」。
    HoldWithoutData { missing: DataKey },
    /// 暫停中：不重畫。錯過的時點不另外記錄，恢復後第一次評估自然補畫一次。
    Paused,
    /// 不需要重畫。
    UpToDate,
    /// 重畫列出的螢幕。
    Redraw(RedrawPlan),
    /// explorer 資源安全閥觸發（4.9；design.md D11）：本來要重畫，但 explorer 的 GDI 物件數超過
    /// 門檻——不設定桌布，呼叫端走「不接管」的還原路徑（`RestoreReason::SafetyValve`：還原原桌布與
    /// 全域填滿方式、主題改「不接管」、系統匣通知、排程器 `reset`）。
    StopTakeover(SafetyValveTrip),
}

/// [`next_action`] 的回傳值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub action: Action,
    /// 要從紀錄中移除的螢幕（已拔除者；主題為「不接管」時是全部）。重新接上的螢幕因此會被當成
    /// 新螢幕重畫。
    pub forget: Vec<String>,
    /// 下一次時間觸發的時刻（UTC）：下一個時點與各台未到期的重試時刻（渲染失敗的退避、恢復事件排入的重試）取最早；`None`＝沒有時間
    /// 觸發，只等事件（暫停中、等待資料、接管中缺資料、不接管、天際線只看資料且沒有待重試）。
    /// [`Action::Redraw`] 這次的值不含本次成敗，見模組文件「呼叫端契約」第 5 點。
    pub next_wake: Option<UnixSeconds>,
    /// 安全閥的新基準（4.9）：[`ExplorerResources::Sampled`] 而沒有基準、或 explorer 的 PID 改變時
    /// 為 `Some`，呼叫端寫進狀態檔；其餘（含 [`Action::StopTakeover`]）為 `None`＝基準不變。
    pub rebaseline: Option<ExplorerRebaseline>,
    /// 上次失敗、重試時刻還沒到的螢幕（依裝置路徑排序；已列入 `next_wake`）。只在
    /// [`Action::UpToDate`]／[`Action::Redraw`] 時可能非空。呼叫端據此把「沒有要重畫、但有螢幕等短間隔
    /// 重試」與真正的「已是最新」分開記錄與顯示——後者不得掩蓋失敗（2026-10-06 在場驗收）。
    pub pending_retries: Vec<PendingRetry>,
}

/// 一台上次失敗、等待重試的螢幕（[`Decision::pending_retries`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingRetry {
    pub device_path: String,
    /// 上次失敗的種類。
    pub kind: FailureKind,
    /// 連續失敗次數（渲染失敗＝連續渲染失敗、[`FailureKind::ApplyError`]＝連續套用失敗；恢復事件排入
    /// 的 `Apply`／`Busy` 重試為 0）。
    pub failures: u32,
    /// 重試時刻（UTC）。
    pub at: UnixSeconds,
}

/// 首次渲染失敗後的重試間隔（秒）。
const RETRY_BASE_SECS: i64 = 2 * 60;
/// 重試間隔上限（秒）。
const RETRY_MAX_SECS: i64 = 30 * 60;

/// 連續第 `failures` 次**渲染**失敗後，到下一次重試的間隔（秒）：2、4、8、16 分鐘，之後固定
/// 30 分鐘。`failures` 為 0 視同 1。
///
/// 排程器用於 [`FailureKind::Render`]（spec「渲染失敗時保留舊圖」：之後重試、連續失敗逐次拉長
/// 間隔）與 [`FailureKind::ApplyError`]（見下）。4.5 的渲染退避應沿用本函式，讓宿主只有一條退避曲線；4.5 若需要不同的曲線，改這裡
/// 而不是另寫一份。[`FailureKind::ApplyError`]（explorer 有回應但設定或讀回失敗）以它自己的連續
/// 次數沿用同一條曲線；設定逾時（[`FailureKind::Apply`]）不退避。
pub fn retry_delay(failures: u32) -> i64 {
    let doublings = failures.saturating_sub(1).min(16);
    RETRY_BASE_SECS
        .saturating_mul(1_i64 << doublings)
        .min(RETRY_MAX_SECS)
}

/// 重畫失敗的種類（[`SchedulerState::record_failed`]）。兩種失敗各依 spec 的不同條文處理。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// 渲染失敗：頁面出錯或逾時，沒有產生 PNG（由 4.5 判定）。依 [`retry_delay`] 退避重試
    /// （spec「渲染失敗時保留舊圖」）。
    Render,
    /// 設定桌布失敗而 explorer 卡住：`SetWallpaper` 等桌布 API 逾時（或其他無法歸到
    /// [`FailureKind::ApplyError`] 的拒絕；由 4.7 依 `crate::desktop::wallpaper::WallpaperError` 判定）。
    /// **不退避、不提早重試**，於下一個重畫時點再試（spec「不因 explorer 卡住而拖累宿主」：逾時時
    /// 放棄該次操作、於下一個重畫時點再試；design D3：排程照常推進到下一個時點）——explorer 卡住
    /// 時反覆重試，每次都要先完整渲染一張圖，正是那條 spec 要避免的負擔。例外：4.7 回報設定路徑
    /// 恢復（[`SchedulerState::apply_path_recovered`]）時重試，每台每個時點最多一次——逾時的請求
    /// 返回、忙碌解除就是這個事件，所以卡住的 explorer 恢復後不必等到下一個時點。
    Apply,
    /// busy 預檢跳過：開始渲染之前查到 COM 執行緒忙碌（上一個桌布請求逾時還沒返回），**沒有
    /// 渲染、也沒有呼叫桌布 API**（由 4.7 判定）。時點語意與 [`FailureKind::Apply`] 相同（等下一個
    /// 時點，或恢復事件重試一次）；差別是保留連續渲染失敗次數（沒有渲染，不能當成渲染成功而
    /// 歸零），並計入 [`SchedulerState::consecutive_busy_skip_slots`]。4.7 每次都要寫記錄檔，見
    /// 模組文件「呼叫端契約」第 3 點。
    Busy,
    /// 設定桌布或設定前讀回時 explorer **有回應但回報失敗**（COM 回錯、處理中途中止），或呼叫端的
    /// 螢幕清單不是最新（列舉失敗，不得以舊座標出圖設定）。由 4.7 判定。與 [`FailureKind::Apply`]
    /// 不同：explorer 沒有卡住、不會有「設定路徑恢復」事件來觸發重試，所以**依 [`retry_delay`] 退避
    /// 重試**（連續次數另計、跨時點累計），不等下一個時點。2026-10-06 在場驗收：一台螢幕離線使讀回
    /// 整批失敗，舊做法記成 `Apply`、等下一個時點，在線螢幕的補畫只能靠別的事件順帶發生。設定路徑
    /// 恢復事件也可提早重試一次（每台每個時點最多一次，同 `Apply`）。
    ApplyError,
}

/// 連續的 busy 預檢跳過跨了這麼多個時點時，4.7 記一行 warn 層級的警告（explorer 長期卡住、桌布
/// 停止更新的訊號；[`SchedulerState::consecutive_busy_skip_slots`]）。
pub const BUSY_SKIP_WARN_SLOTS: u32 = 3;

/// 上次嘗試的結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// 成功設定。
    Drawn,
    /// 失敗。
    /// - `kind`＝這次失敗的種類。
    /// - `render_failures`＝這台**連續渲染失敗**的次數（跨時點累計）。成功、或設定失敗（代表
    ///   渲染已成功）時歸零；預檢跳過（[`FailureKind::Busy`]，沒有渲染）時保留原值。
    /// - `retry_at`＝重試時刻：渲染失敗、[`FailureKind::ApplyError`] 時有值（退避）；`Apply`、
    ///   預檢跳過時為 `None`（等下一個時點、主題或出圖條件改變，或設定路徑恢復事件）。
    /// - `recovery_used`＝這個時點（同主題、同出圖條件、同 [`Stamp`]）的恢復重試額度已用掉：
    ///   [`SchedulerState::apply_path_recovered`] 排入重試時設為真，同一個時點內之後的失敗延續
    ///   此值；時點、主題或出圖條件一變就是新的紀錄，從假開始。
    /// - `apply_failures`＝這台**連續 [`FailureKind::ApplyError`]** 的次數（跨時點累計，退避用）。
    ///   成功或其他種類的失敗歸零；預檢跳過（`Busy`）保留原值。
    Failed {
        kind: FailureKind,
        render_failures: u32,
        apply_failures: u32,
        retry_at: Option<UnixSeconds>,
        recovery_used: bool,
    },
}

/// 一台螢幕上次嘗試重畫的紀錄（成功或失敗都記，見 [`Outcome`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MonitorRecord {
    theme: WallpaperTheme,
    geometry: MonitorGeometry,
    stamp: Stamp,
    outcome: Outcome,
}

/// 排程器的記憶：每台螢幕上次嘗試重畫的主題、出圖條件、時點與結果，以及本次執行是否已接管。
/// 只存在記憶體中，不持久化。
#[derive(Debug, Clone, Default)]
pub struct SchedulerState {
    records: HashMap<String, MonitorRecord>,
    /// 本次執行中是否有任何一台成功畫過。第一次成功時設為真，之後的失敗不會清掉（螢幕上仍是
    /// 宿主的圖）；只有 [`SchedulerState::reset`] 與 [`Action::Idle`] 才清掉。
    taken_over_this_run: bool,
    /// 世代：[`SchedulerState::reset`] 遞增；[`RedrawPlan`] 帶著做出時的值，不相符的結果一律忽略。
    generation: u64,
    /// 連續 busy 預檢跳過跨了幾個時點（[`SchedulerState::consecutive_busy_skip_slots`]）。
    busy_skip_slots: u32,
    /// 上一次計入 `busy_skip_slots` 的（主題、時點）：同一個時點的多台、恢復重試只算一次。
    last_busy_skip: Option<(WallpaperTheme, Stamp)>,
}

impl SchedulerState {
    /// 某台螢幕依 `plan` 重畫成功後呼叫。回傳是否記錄：`false`＝`plan` 是
    /// [`SchedulerState::reset`] 之前做出的（過時），結果被忽略，紀錄與「本次執行已接管」都不動。
    pub fn record_drawn(&mut self, plan: &RedrawPlan, monitor: &MonitorSnapshot) -> bool {
        if plan.generation != self.generation {
            return false;
        }
        self.taken_over_this_run = true;
        self.clear_busy_skips();
        self.records.insert(
            monitor.device_path.clone(),
            MonitorRecord {
                theme: plan.theme,
                geometry: monitor.geometry,
                stamp: plan.stamp,
                outcome: Outcome::Drawn,
            },
        );
        true
    }

    /// 某台螢幕依 `plan` 重畫失敗後呼叫；`kind` 區分渲染失敗（4.5 判定）、設定桌布失敗與 busy
    /// 預檢跳過（4.7 判定），見 [`FailureKind`]。回傳是否記錄：`false`＝`plan` 是
    /// [`SchedulerState::reset`] 之前做出的（過時），結果被忽略。
    ///
    /// `now` 是**判定失敗的那一刻**（渲染逾時、桌布 API 回錯的時候），不是 [`next_action`] 做出
    /// 決策的時刻：多台螢幕依序處理、每台最長要等渲染 30 秒＋設定逾時，用決策時刻會讓重試時刻
    /// 落在過去，退避形同失效。
    ///
    /// 之後這台只在以下情況再列入重畫：
    /// - 渲染失敗的重試時刻已到，或設定失敗後收到「設定路徑恢復」事件
    ///   （[`SchedulerState::apply_path_recovered`]）——兩者都是
    ///   [`RedrawReason::RetryAfterFailure`]；
    /// - 時點變新（天際線、撕日曆的資料日期也屬於時點，[`Stamp`]）；
    /// - 主題改變、出圖條件（解析度、DPI）改變。
    ///
    /// 其他事件（資料通道、暫停原因、別台螢幕）不會讓它在兩個時點之間重畫（spec「兩個重畫時點
    /// 之間不渲染」）。
    pub fn record_failed(
        &mut self,
        plan: &RedrawPlan,
        target: &RedrawTarget,
        kind: FailureKind,
        now: UnixSeconds,
    ) -> bool {
        if plan.generation != self.generation {
            return false;
        }
        let path = &target.monitor.device_path;
        // 前一次失敗的連續渲染失敗次數（跨時點累計）；恢復額度只在同一個時點（同主題、同出圖
        // 條件、同 Stamp）內延續。
        let (previous_render_failures, previous_apply_failures, recovery_used) =
            match self.records.get(path) {
                Some(MonitorRecord {
                    theme,
                    geometry,
                    stamp,
                    outcome:
                        Outcome::Failed {
                            render_failures,
                            apply_failures,
                            recovery_used,
                            ..
                        },
                }) => (
                    *render_failures,
                    *apply_failures,
                    *recovery_used
                        && *theme == plan.theme
                        && *geometry == target.monitor.geometry
                        && *stamp == plan.stamp,
                ),
                _ => (0, 0, false),
            };
        let (render_failures, apply_failures, retry_at) = match kind {
            FailureKind::Render => {
                let render_failures = previous_render_failures.saturating_add(1);
                (render_failures, 0, Some(now + retry_delay(render_failures)))
            }
            FailureKind::Apply => (0, 0, None),
            FailureKind::Busy => (previous_render_failures, previous_apply_failures, None),
            FailureKind::ApplyError => {
                let apply_failures = previous_apply_failures.saturating_add(1);
                (0, apply_failures, Some(now + retry_delay(apply_failures)))
            }
        };
        if kind == FailureKind::Busy {
            let slot = (plan.theme, plan.stamp);
            if self.last_busy_skip != Some(slot) {
                self.busy_skip_slots = self.busy_skip_slots.saturating_add(1);
                self.last_busy_skip = Some(slot);
            }
        } else {
            self.clear_busy_skips();
        }
        self.records.insert(
            path.clone(),
            MonitorRecord {
                theme: plan.theme,
                geometry: target.monitor.geometry,
                stamp: plan.stamp,
                outcome: Outcome::Failed {
                    kind,
                    render_failures,
                    apply_failures,
                    retry_at,
                    recovery_used,
                },
            },
        );
        true
    }

    /// 連續的 busy 預檢跳過（[`FailureKind::Busy`]）跨了幾個時點：同一個時點的多台、同一個時點內
    /// 的恢復重試只算一次。任何一台通過預檢（成功、渲染失敗、設定失敗）或
    /// [`SchedulerState::reset`] 時歸零。4.7 在它從低於 [`BUSY_SKIP_WARN_SLOTS`] 變成達到時記一行
    /// warn 層級的警告（模組文件「呼叫端契約」第 3 點）。
    pub fn consecutive_busy_skip_slots(&self) -> u32 {
        self.busy_skip_slots
    }

    fn clear_busy_skips(&mut self) {
        self.busy_skip_slots = 0;
        self.last_busy_skip = None;
    }

    /// 「設定桌布的路徑恢復了」事件：4.7 在 COM 執行緒逾時後忙碌旗標解除、或處理完 explorer
    /// 重新啟動（重建 `IDesktopWallpaper`）時呼叫，`now` 為那一刻。回傳被重新排入的螢幕數（記錄用；
    /// 實際重畫的螢幕以下一次 [`Decision`] 的 targets 為準）。
    ///
    /// 上次失敗是 [`FailureKind::Apply`] 或 [`FailureKind::Busy`]、還在等下一個時點、且這個時點的
    /// 恢復額度還沒用掉的螢幕，重試時刻設為 `now`：下一次評估以 [`RedrawReason::RetryAfterFailure`]
    /// 重畫，不必等下一個時點（首次接管時那可能是明天 06:00 或下一個交易日）。其他螢幕不受影響：
    /// 成功的照舊，渲染失敗的維持自己的退避。
    ///
    /// **每台每個時點最多一次**：排入時就用掉額度。重試又失敗（不論種類）之後，同一個時點內的
    /// 恢復事件對這台不再有作用，直到下一個時點、換主題、解析度或 DPI 改變——explorer 反覆
    /// 「逾時後才返回」時（每次逾時後忙碌旗標都會解除），不會形成「渲染 → 逾時 → 恢復 → 再渲染」
    /// 的無上限循環（spec「不因 explorer 卡住而拖累宿主」）。
    ///
    /// [`FailureKind::ApplyError`]（已有退避的重試時刻）同樣適用：重試時刻提早到 `now`、用掉額度。
    pub fn apply_path_recovered(&mut self, now: UnixSeconds) -> usize {
        let mut count = 0;
        for record in self.records.values_mut() {
            let Outcome::Failed {
                kind,
                retry_at,
                recovery_used,
                ..
            } = &mut record.outcome
            else {
                continue;
            };
            let eligible = match kind {
                FailureKind::Apply | FailureKind::Busy => retry_at.is_none(),
                FailureKind::ApplyError => retry_at.is_some_and(|at| at > now),
                FailureKind::Render => false,
            };
            if eligible && !*recovery_used {
                *retry_at = Some(now);
                *recovery_used = true;
                count += 1;
            }
        }
        count
    }

    /// 目前的世代（測試用：確認 4.4 的還原／讓位有呼叫 [`SchedulerState::reset`]）。
    #[cfg(test)]
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// 清除全部紀錄（每台的時點、失敗紀錄）、「本次執行已接管」與 busy 跳過計數，回到剛啟動的
    /// 狀態：下次評估每台都當成沒有紀錄而重畫，缺資料時是 [`Action::WaitingForData`]。同時遞增
    /// 世代：`reset` 之前做出的 [`RedrawPlan`]，之後才回報的結果一律忽略。
    ///
    /// **時機上的前提**：還原＋`reset` 要等進行中的 `Redraw` 結束或中止之後（模組文件「呼叫端
    /// 契約」第 4 點）。世代只保證紀錄不被寫回，擋不住已經送出的 `SetWallpaper`。
    ///
    /// **4.4 必須在以下時機呼叫**：還原原桌布之後（選「不接管」、系統匣「結束」、explorer 資源
    /// 安全閥觸發），以及讓位之後（使用者自行更換桌布）。不能只靠 [`Action::Idle`]：4.7 會把
    /// `Redraw` 執行期間的喚醒合併成一次評估（模組文件「呼叫端契約」第 4 點），使用者在那段
    /// 期間「不接管 → 還原 → 又切回原主題」時，排程器從未以「不接管」評估、`Idle` 不會出現，
    /// 紀錄會讓它以為螢幕上仍是宿主的圖。
    pub fn reset(&mut self) {
        self.records.clear();
        self.taken_over_this_run = false;
        self.clear_busy_skips();
        self.generation = self.generation.wrapping_add(1);
    }

    /// 只清掉每台螢幕的紀錄（時點、主題、出圖條件、失敗與退避），**保留**「本次執行已接管」、
    /// busy 跳過計數與世代（task 4.7a）。下次評估每台都當成沒有紀錄而重畫一次。
    ///
    /// 用途：時點以「嚴格較新」比較（[`Stamp`]），系統時間被調回、或時區改變使本機時間倒退時，
    /// 現在的時點會一直「不比紀錄新」，要等時間追上紀錄才重畫；主題設定檔改變時，時點沒變但圖要
    /// 重畫。這兩種情況都不是還原或讓位，不該動世代（沒有要作廢的計畫）或「已接管」（缺資料時
    /// 仍應是 [`Action::HoldWithoutData`]）。**只在沒有進行中的 `Redraw` 時呼叫**。
    pub fn clear_records(&mut self) {
        self.records.clear();
    }

    /// 套用 [`Decision::forget`]；[`Action::Idle`] 時一併清掉「本次執行已接管」。
    pub fn apply_forget(&mut self, decision: &Decision) {
        for path in &decision.forget {
            self.records.remove(path);
        }
        if decision.action == Action::Idle {
            self.taken_over_this_run = false;
        }
    }
}

/// 這個暫停原因是否停止桌布重畫（spec「暫停更新」）。刻意窮舉：日後 `PauseReason` 新增成員時
/// 編譯器會要求在這裡決定。
///
/// - 鎖定、全螢幕／簡報（`SystemBusy`：`SHQueryUserNotificationState` 的 `QUNS_BUSY`／
///   `QUNS_RUNNING_D3D_FULL_SCREEN`／`QUNS_PRESENTATION_MODE`）、顯示器關閉：spec 明列。
/// - spec 也要求「勿打擾」時暫停，但勿打擾**不經** `PauseReason`（`SystemBusy` 不涵蓋 Windows 的
///   勿打擾）：4.7c 由 `crate::wallpaper_dnd` 偵測（官方 FocusSessionManager＋非官方 WNF，OR 合併），
///   協調迴圈在評估結果是 `Redraw` 時經 `CoordinatorPorts::do_not_disturb` 判定、改為
///   `CoordinatorState::DoNotDisturb` 不渲染。它只停桌布重畫、不暫停小工具，所以不在這張表裡。
/// - 電池、省電：`crate::widgets` 已依 `PauseRules` 過濾，出現在集合裡就表示規則開啟且條件成立。
/// - 手動暫停：spec 未明列；系統匣的「暫停」是使用者要宿主整體停下，桌布一併停止（報告列為
///   待 controller 確認的判斷）。
/// - 系統睡眠沒有對應成員：睡眠時行程根本不執行，醒來後第一次評估自然補畫。
pub(crate) fn pauses_wallpaper(reason: PauseReason) -> bool {
    match reason {
        PauseReason::Locked
        | PauseReason::SystemBusy
        | PauseReason::DisplayOff
        | PauseReason::Battery
        | PauseReason::PowerSaver
        | PauseReason::Manual => true,
    }
}

/// 本機時刻（UTC 秒加上當時的偏移；「本機的 UNIX 秒」，只用來取日期與時刻）。
fn local_secs(t: UnixSeconds, tz: &dyn UtcOffsetSource) -> i64 {
    t + i64::from(tz.offset_at(t))
}

/// 星盤時點：本機時刻往下取整到 15 分鐘、再換回 UTC＝該時點的開始時刻（UTC）。
fn astrolabe_slot(t: UnixSeconds, tz: &dyn UtcOffsetSource) -> UnixSeconds {
    let offset = i64::from(tz.offset_at(t));
    (t + offset).div_euclid(ASTROLABE_SLOT_SECS) * ASTROLABE_SLOT_SECS - offset
}

/// 脊線、等高線的「日」：本機時間減 6 小時後的日數（06:00 之前仍屬前一天）。
fn daily_redraw_day(t: UnixSeconds, tz: &dyn UtcOffsetSource) -> i64 {
    (local_secs(t, tz) - DAILY_REDRAW_SECS_OF_DAY).div_euclid(SECS_PER_DAY)
}

/// 撕日曆的「日」：本機日數。
fn local_day(t: UnixSeconds, tz: &dyn UtcOffsetSource) -> i64 {
    local_secs(t, tz).div_euclid(SECS_PER_DAY)
}

/// 時間類主題的時點鍵：數值變大＝進入新的時點。沒有時間觸發的主題（天際線、不接管）回 `None`。
fn time_key(theme: WallpaperTheme, t: UnixSeconds, tz: &dyn UtcOffsetSource) -> Option<i64> {
    match theme {
        WallpaperTheme::Astrolabe => Some(astrolabe_slot(t, tz)),
        WallpaperTheme::Ridgeline | WallpaperTheme::Contour => Some(daily_redraw_day(t, tz)),
        WallpaperTheme::Tearoff => Some(local_day(t, tz)),
        WallpaperTheme::Skyline | WallpaperTheme::None => None,
    }
}

/// 本機時刻 `local` 之後的下一個時點邊界（本機秒）；只對有時間觸發的主題呼叫。
fn next_local_boundary(theme: WallpaperTheme, local: i64) -> Option<i64> {
    match theme {
        WallpaperTheme::Astrolabe => {
            Some((local.div_euclid(ASTROLABE_SLOT_SECS) + 1) * ASTROLABE_SLOT_SECS)
        }
        WallpaperTheme::Ridgeline | WallpaperTheme::Contour => Some(
            ((local - DAILY_REDRAW_SECS_OF_DAY).div_euclid(SECS_PER_DAY) + 1) * SECS_PER_DAY
                + DAILY_REDRAW_SECS_OF_DAY,
        ),
        WallpaperTheme::Tearoff => Some((local.div_euclid(SECS_PER_DAY) + 1) * SECS_PER_DAY),
        WallpaperTheme::Skyline | WallpaperTheme::None => None,
    }
}

impl Stamp {
    /// `self`（現在應呈現的時點）是否比 `old`（上次畫的）新到需要重畫。
    ///
    /// 時間與交易日一律「嚴格較新」才算：本機時間因夏令時間結束倒退、或資料層退回舊快照時，
    /// 不重畫（避免同一時點畫兩次）。撕日曆的融資日期是「變了」就算（brief 裁定 3）。
    /// 種類不同（主題不同）一律算過時；實際上主題不同時 [`next_action`] 已先判為
    /// [`RedrawReason::ThemeChanged`]。
    fn supersedes(self, old: Stamp) -> bool {
        match (self, old) {
            (Stamp::Slot(new), Stamp::Slot(old)) => new > old,
            (Stamp::Day(new), Stamp::Day(old)) => new > old,
            (Stamp::Daily(new), Stamp::Daily(old)) => new > old,
            (
                Stamp::Tearoff { date, margin },
                Stamp::Tearoff {
                    date: old_date,
                    margin: old_margin,
                },
            ) => date > old_date || margin != old_margin,
            _ => true,
        }
    }
}

/// 排程核心（design.md D6）：見模組文件「呼叫端契約」。
///
/// 判定順序：
/// 1. 主題「不接管」→ [`Action::Idle`]，`forget` 列出全部紀錄。
/// 2. `forget` 列出紀錄中、但已不在 `monitors` 的螢幕（以下各情況都會帶）。
/// 3. 所選主題需要的資料不存在：
///    - 尚未接管（`already_taken_over` 為假、本次執行也還沒有任何一台成功畫過）→
///      [`Action::WaitingForData`]；
///    - 已接管 → [`Action::HoldWithoutData`]（保留目前的桌布，不還原、不讓位）。
///
///    兩者都比暫停優先，設定視窗在暫停中也能顯示資料狀態。
/// 4. 任一暫停原因成立 → [`Action::Paused`]。
/// 5. 逐台比較紀錄：沒有紀錄／主題不同／解析度或縮放不同／時點過時／重試時刻已到（渲染失敗的退避、設定路徑恢復） →
///    列入重畫；都沒有 → [`Action::UpToDate`]。上次失敗而重試時刻未到、時點與條件都沒變的螢幕，其他事件一律
///    不讓它重畫（[`SchedulerState::record_failed`]）。
/// 6. `next_wake`＝下一個時點與各台未到期的重試時刻（渲染失敗的退避、恢復事件排入的重試）取最早。
/// 7. 確定要重畫時（即將設定桌布之前）才判定 4.9 安全閥（[`ExplorerResources`]）：超過門檻 →
///    [`Action::StopTakeover`]；否則照常 [`Action::Redraw`]，必要時在 [`Decision::rebaseline`]
///    帶出新基準。
pub fn next_action(state: &SchedulerState, input: &SchedulerInput<'_>) -> Decision {
    let theme = input.theme;
    let mut forget: Vec<String> = state
        .records
        .keys()
        .filter(|path| {
            theme == WallpaperTheme::None || !input.monitors.iter().any(|m| &m.device_path == *path)
        })
        .cloned()
        .collect();
    forget.sort();

    // 每條路徑只回傳一次，`forget` 直接移進結果。
    let decide = move |action: Action, next_wake: Option<UnixSeconds>| Decision {
        action,
        forget,
        next_wake,
        rebaseline: None,
        pending_retries: Vec::new(),
    };

    let now = input.now;
    let tz = input.tz;
    let stamp = match theme {
        WallpaperTheme::None => return decide(Action::Idle, None),
        WallpaperTheme::Astrolabe => Ok(Stamp::Slot(astrolabe_slot(now, tz))),
        WallpaperTheme::Ridgeline | WallpaperTheme::Contour => match input.data.twii_intraday {
            None => Err(DataKey::TwiiIntraday),
            Some(_) => Ok(Stamp::Day(CivilDate::from_days(daily_redraw_day(now, tz)))),
        },
        WallpaperTheme::Skyline => input
            .data
            .twii_daily
            .map(Stamp::Daily)
            .ok_or(DataKey::TwiiDaily),
        WallpaperTheme::Tearoff => Ok(Stamp::Tearoff {
            date: CivilDate::from_days(local_day(now, tz)),
            margin: input.data.margin,
        }),
    };
    let stamp = match stamp {
        Ok(stamp) => stamp,
        Err(missing) if input.already_taken_over || state.taken_over_this_run => {
            return decide(Action::HoldWithoutData { missing }, None)
        }
        Err(missing) => return decide(Action::WaitingForData { missing }, None),
    };

    if input.pause.iter().any(|reason| pauses_wallpaper(*reason)) {
        return decide(Action::Paused, None);
    }

    let mut next_wake = next_change(theme, now, tz);
    let mut targets = Vec::new();
    let mut pending_retries = Vec::new();
    for monitor in input.monitors {
        let reason = match state.records.get(&monitor.device_path) {
            None => RedrawReason::Unrecorded,
            Some(record) if record.theme != theme => RedrawReason::ThemeChanged,
            Some(record) if record.geometry != monitor.geometry => RedrawReason::GeometryChanged,
            Some(record) if stamp.supersedes(record.stamp) => RedrawReason::Scheduled,
            Some(MonitorRecord {
                outcome:
                    Outcome::Failed {
                        kind,
                        render_failures,
                        apply_failures,
                        retry_at: Some(retry_at),
                        ..
                    },
                ..
            }) => {
                if now >= *retry_at {
                    RedrawReason::RetryAfterFailure
                } else {
                    next_wake = Some(next_wake.map_or(*retry_at, |wake| wake.min(*retry_at)));
                    pending_retries.push(PendingRetry {
                        device_path: monitor.device_path.clone(),
                        kind: *kind,
                        failures: match kind {
                            FailureKind::Render => *render_failures,
                            FailureKind::ApplyError => *apply_failures,
                            FailureKind::Apply | FailureKind::Busy => 0,
                        },
                        at: *retry_at,
                    });
                    continue;
                }
            }
            Some(_) => continue,
        };
        targets.push(RedrawTarget {
            monitor: monitor.clone(),
            reason,
        });
    }
    pending_retries.sort_by(|a, b| a.device_path.cmp(&b.device_path));
    if targets.is_empty() {
        let mut decision = decide(Action::UpToDate, next_wake);
        decision.pending_retries = pending_retries;
        return decision;
    }

    // 4.9 安全閥：即將設定桌布，依 explorer 讀值決定是否改回傳「停止接管」。
    let rebaseline = match judge_explorer(input.explorer) {
        ExplorerJudgement::Trip(trip) => return decide(Action::StopTakeover(trip), None),
        ExplorerJudgement::Proceed { rebaseline } => rebaseline,
    };

    let as_of = match stamp {
        Stamp::Slot(slot_start) => slot_start,
        Stamp::Day(_) | Stamp::Daily(_) | Stamp::Tearoff { .. } => now,
    };
    let mut decision = decide(
        Action::Redraw(RedrawPlan {
            theme,
            targets,
            stamp,
            as_of,
            generation: state.generation,
        }),
        next_wake,
    );
    decision.rebaseline = rebaseline;
    decision.pending_retries = pending_retries;
    decision
}

/// [`judge_explorer`] 的結果。
enum ExplorerJudgement {
    /// 照常設定；`rebaseline` 為要寫進狀態檔的新基準（沒有就不變）。
    Proceed {
        rebaseline: Option<ExplorerRebaseline>,
    },
    /// 停止接管。
    Trip(SafetyValveTrip),
}

/// 安全閥判定（design.md D11；只在即將設定桌布時呼叫）：
///
/// 1. 沒有讀值或讀取失敗 → 照常設定（spec：讀取失敗 MUST NOT 停止接管）。
/// 2. 判定用的基準＝狀態檔的基準，但 PID 不同（explorer 重新啟動）時視為沒有基準。
/// 3. 讀值超過 [`EXPLORER_GDI_ABSOLUTE_LIMIT`]，或有基準且讀值比基準多出超過
///    [`EXPLORER_GDI_DELTA_LIMIT`] → 停止接管。絕對值不依賴基準：explorer 已接近單一行程上限
///    10,000 時，接管開始或 explorer 剛重啟也不再往上加。
/// 4. 否則照常設定；沒有判定用的基準時以這次讀值為新基準（spec「以接管開始時讀到的值為基準」
///    「以重新啟動後第一次讀到的值為新基準」）。讀值低於基準時基準不變。
fn judge_explorer(explorer: ExplorerResources) -> ExplorerJudgement {
    let (reading, stored) = match explorer {
        ExplorerResources::NotSampled | ExplorerResources::ReadFailed => {
            return ExplorerJudgement::Proceed { rebaseline: None }
        }
        ExplorerResources::Sampled { reading, baseline } => (reading, baseline),
    };
    let baseline = stored.filter(|b| b.pid == reading.pid);
    let delta_exceeded =
        baseline.is_some_and(|b| reading.gdi.saturating_sub(b.gdi) > EXPLORER_GDI_DELTA_LIMIT);
    let absolute_exceeded = reading.gdi > EXPLORER_GDI_ABSOLUTE_LIMIT;
    if delta_exceeded || absolute_exceeded {
        return ExplorerJudgement::Trip(SafetyValveTrip {
            reading,
            baseline,
            delta_exceeded,
            absolute_exceeded,
        });
    }
    let rebaseline = match (baseline, stored) {
        (Some(_), _) => None,
        (None, None) => Some(ExplorerRebaseline {
            baseline: reading,
            cause: RebaselineCause::NoBaseline,
        }),
        (None, Some(previous)) => Some(ExplorerRebaseline {
            baseline: reading,
            cause: RebaselineCause::PidChanged { previous },
        }),
    };
    ExplorerJudgement::Proceed { rebaseline }
}

/// 從 `now` 之後，主題的時間鍵第一次變新的時刻（UTC 秒）；主題沒有時間觸發時回 `None`。
///
/// 只用「現在的偏移」把下一個本機邊界換回 UTC，遇到中間有夏令切換會差一個切換量（春季晚到
/// 一小時）。做法：
/// 1. 以現在的偏移與「候選時刻當下的偏移」各換算一次，加上往後 1、2 小時的保險，找出第一個
///    「時間鍵確實已變新」的候選，當作上界；
/// 2. 在 `(now, 上界]` 二分搜尋「時間鍵變新」的最早秒數。時間鍵在一般情況下隨時間單調不減，
///    所以結果就是真正的切換點；本機時間跳過邊界（夏令開始的空缺）時落在跳躍那一秒。
///
/// 找不到上界（理論上不會發生）時回傳最早的候選：提早喚醒無害，評估會回
/// [`Action::UpToDate`] 並給新的喚醒時刻。
fn next_change(
    theme: WallpaperTheme,
    now: UnixSeconds,
    tz: &dyn UtcOffsetSource,
) -> Option<UnixSeconds> {
    let key_now = time_key(theme, now, tz)?;
    let is_newer = |t: UnixSeconds| time_key(theme, t, tz).is_some_and(|key| key > key_now);

    let next_local = next_local_boundary(theme, local_secs(now, tz))?;
    let by_offset_now = next_local - i64::from(tz.offset_at(now));
    let by_offset_then = next_local - i64::from(tz.offset_at(by_offset_now));
    let mut candidates = [
        by_offset_then,
        by_offset_now,
        by_offset_now + 3600,
        by_offset_now + 7200,
    ];
    candidates.sort_unstable();
    let mut candidates = candidates.into_iter().filter(|&t| t > now);
    let earliest = candidates.clone().next()?;
    let Some(mut hi) = candidates.find(|&t| is_newer(t)) else {
        return Some(earliest);
    };

    // 不變式：is_newer(lo) 為假（lo 起於 now，時間鍵等於 key_now）、is_newer(hi) 為真。
    let mut lo = now;
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if is_newer(mid) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    Some(hi)
}

#[cfg(test)]
mod tests;
