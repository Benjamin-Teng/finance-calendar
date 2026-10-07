//! 一輪抓取的組裝（design.md D4、D7；tasks.md 4.1；behavior-inventory B-FLOW-3～11、B-EVT、B-OUT）。
//!
//! 對照 Python `update_tw_events.py` 的 `main()`（`:1116-1315`）：依序呼叫十二個來源、串接錯誤、合併事件、
//! 算 `counts`／`fetched`／`updated`，產生整份 [`Output`]。**所有來源專屬的沿用規則都在各來源的
//! `run_guarded` 裡**（見 [`super::outcome`]），這裡只做 `main()` 自己做的事：
//!
//! | 步驟 | Python `main()` | 這裡 |
//! | --- | --- | --- |
//! | 等網路逾時 | `errors.append("啟動時網路等待逾時，改用既有資料兜底")`（第一筆） | `net_timed_out` 旗標（等網路本身在 5.2） |
//! | 窗口 | `datetime.now(TPE).date()`＋`WINDOW_DAYS` | [`events::window`]（台北今天，B-DATE-2） |
//! | 抓取順序 | macro → 除權息 → 股東會 → 前百大 → 法說會 → 處置股 → 行情 → SOFR → 休市日曆 → 三個桌布鍵 | 同（序列，B-FLOW-5） |
//! | `errors` | 依上面順序各來源 append | 依同順序串接各 [`SourceOutcome::errors`]（B-FLOW-9、B-FLOW-10 每輪重算） |
//! | `wallpaper_errors` | 三個桌布鍵依序 append | 依序串接 [`WallpaperOutcome::wallpaper_errors`]，**不**進 `errors` |
//! | 事件 | `div_ev + meeting_ev + conf_ev` → 窗口、去重、排序 | 同，用 [`events::merge_events`] |
//! | `quotes` | `fetch_quotes` 的 12 檔，SOFR 3M 有值就 append 在最後 | 同（B-OUT-10） |
//! | `fetched` | 任一「計入新鮮度」的來源 fresh → 現在；否則 `old.fetched or old.updated or 現在` | 同（B-FLOW-7） |
//! | `updated` | `datetime.now()` 本機時間 | `clock.local_stamp()`（B-DATE-3） |
//!
//! 計入 `fresh` 的來源：macro、除權息、股東會、前百大（走快取不算）、法說會（含備援）、SOFR、休市日曆。
//! **不計**：處置股、Yahoo 行情、三個桌布鍵——後者在型別上就沒有 `fresh` 欄位（[`WallpaperOutcome`]）。
//!
//! # 防線（D7）
//!
//! 每個來源各自已由 `run_guarded` 隔離 panic（失敗＋沿用舊資料，該輪照常寫檔）。[`run_round`] 再包一層
//! `catch_unwind` 當最後防線：組裝本身（來源以外的程式）若 panic，回傳 `Err`、**不產生輸出**，呼叫端
//! 不寫檔、記錄錯誤，下一個時點照常。
//!
//! # 寫檔
//!
//! [`write_round`]：寫檔前清掉同目錄修改時間超過 1 小時的 `tw_events.json.*.tmp`（行程在
//! `sync_all`／`rename` 之間被殺會留下殘檔，task 2 審查 m1），再用 [`output::write_atomic`] 原子寫入。

use std::future::Future;
use std::io;
use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::time::{Duration, SystemTime};

use futures_util::FutureExt;

use super::clock::Clock;
use super::dates::iso_date;
use super::events::{self, count_of};
use super::http::Fetch;
use super::outcome::panic_message;
use super::output::{
    self, Counts, EventType, MacroMeta, Output, PreviousOutput, Quote, Sources, Window, OUTPUT_FILE,
};
use super::quotes::QUOTES;
use super::{
    conference, dividend, holidays, macro_ff, margin, meeting, punish, quotes, sofr, top100, twii,
};

/// 等網路逾時時放在 `errors` 第一筆的訊息（B-FLOW-9，逐字）。
pub const MSG_NET_TIMEOUT: &str = "啟動時網路等待逾時，改用既有資料兜底";

/// 殘留暫存檔的保留時間：超過就在寫檔前清掉。
pub const STALE_TMP_AGE: Duration = Duration::from_secs(3600);

/// 一輪的結果。
#[derive(Debug, Clone, PartialEq)]
pub struct RoundResult {
    /// `Ok`＝整份輸出，呼叫端以 [`write_round`] 寫檔；`Err`＝組裝本身 panic（最後防線，D7）或被要求停止，
    /// 訊息為 panic 內容（停止時見 [`RoundResult::aborted`]），呼叫端不寫檔、只記錄。
    pub output: Result<Output, String>,
    /// 日誌行（B-FLOW-18，不含時間戳；給 5.4 的記錄輸出）：開頭一行抓取窗口、各來源日誌依執行順序、結尾一行摘要。
    /// 「已輸出 →」由寫檔的人補。`output` 為 `Err` 時只有一行說明。
    pub logs: Vec<String>,
    /// 本輪是否有任何來源成功（5.2 排程的失敗重試依據，design.md D5「上一輪沒有任何來源成功」）。
    ///
    /// 為什麼不直接用 `fetched` 是否刷新：`fetched` 刻意排除 Yahoo 行情與處置股（B-FLOW-7，否則「已 N 天
    /// 未更新」永遠不會出現），但只要行情抓得到就代表網路與至少一個來源是通的，不該再每 10–15 分鐘重試。
    /// 定義＝`fetched` 會被刷新的來源（總經、除權息、股東會、非快取命中的前百大、法說會、SOFR、休市日曆）
    /// 任一抓到新資料，**或**處置股兩個市場至少一個沒有錯誤，**或**行情 12 檔中至少一檔沒有錯誤。
    /// 動態桌布三鍵（盤中走勢、日 K、券資比）不計。前百大當日快取沿用、沒有錯誤也不算成功（離線時它永遠
    /// 「沒有錯誤」）。整輪 panic 或被停止＝`false`。
    pub any_source_ok: bool,
    /// 是否因停止要求（[`RoundHooks::should_stop`]）在來源邊界中止：此時 `output` 為 `Err`、**不應寫檔**、
    /// 也不算失敗（呼叫端不更新排程紀錄）。
    pub aborted: bool,
}

/// 一輪的外部掛鉤（5.2：排程的即時日誌與停止介面；`--fetch-once` 只用日誌）。
///
/// 都要求 `Send`／`Sync`，這樣 [`run_round_with`] 的 future 仍是 `Send`（`run_round_future_is_send`）。
pub struct RoundHooks<'a> {
    /// 每個來源**跑完當下**收到它的日誌行（窗口行也算；結尾的「完成：…」摘要不經這裡，因為要等寫檔結果
    /// 之後才記，與 Python 的順序一致）。讓長輪的記錄檔有各來源的實際時間，被中途結束時也留得下進度。
    pub on_log: Option<&'a mut (dyn FnMut(&str) + Send + 'a)>,
    /// 每個來源跑完後問一次：回 `true`＝停在這個來源邊界（不跑後面的來源、不產生輸出）。最後一個來源之後
    /// 不再問——輸出已完整，讓呼叫端照常寫檔。
    pub should_stop: Option<&'a (dyn Fn() -> bool + Sync + 'a)>,
}

#[cfg(test)]
impl RoundHooks<'_> {
    /// 沒有掛鉤。
    pub fn none() -> Self {
        RoundHooks {
            on_log: None,
            should_stop: None,
        }
    }
}

/// 跑一輪、沒有掛鉤（測試用的簡寫）。
#[cfg(test)]
pub async fn run_round<F: Fetch>(
    fetch: &F,
    clock: &Clock,
    prev: &PreviousOutput,
    net_timed_out: bool,
) -> RoundResult {
    run_round_with(fetch, clock, prev, net_timed_out, &mut RoundHooks::none()).await
}

/// 跑一輪（B-FLOW-3～11）。
///
/// - `prev`：上一份輸出（`output::read_previous`；不存在或壞掉＝空）。
/// - `net_timed_out`：啟動時等網路逾時（B-FLOW-2）；為真時 `errors` 第一筆是 [`MSG_NET_TIMEOUT`]。
/// - `hooks`：即時日誌與停止，見 [`RoundHooks`]。
pub async fn run_round_with<F: Fetch>(
    fetch: &F,
    clock: &Clock,
    prev: &PreviousOutput,
    net_timed_out: bool,
    hooks: &mut RoundHooks<'_>,
) -> RoundResult {
    match catch_round(assemble(fetch, clock, prev, net_timed_out, hooks)).await {
        Ok(Ok((output, logs, any_source_ok))) => RoundResult {
            output: Ok(output),
            logs,
            any_source_ok,
            aborted: false,
        },
        Ok(Err(Stopped { logs })) => RoundResult {
            output: Err("已依停止要求在來源邊界中止".to_string()),
            logs,
            any_source_ok: false,
            aborted: true,
        },
        Err(msg) => {
            log::error!("fetch: 整輪組裝 panic，本輪不寫檔：{msg}");
            RoundResult {
                logs: vec![format!("[整輪] 非預期錯誤，本輪不寫檔：{msg}")],
                output: Err(msg),
                any_source_ok: false,
                aborted: false,
            }
        }
    }
}

/// 被停止要求中止（保留已跑部分的日誌）。
struct Stopped {
    logs: Vec<String>,
}

/// 累積日誌並即時送出的小工具。
struct Progress<'h, 'a> {
    logs: Vec<String>,
    hooks: &'h mut RoundHooks<'a>,
}

impl Progress<'_, '_> {
    fn add(&mut self, lines: &[String]) {
        for line in lines {
            if let Some(f) = self.hooks.on_log.as_mut() {
                f(line);
            }
            self.logs.push(line.clone());
        }
    }

    fn stop_requested(&self) -> bool {
        self.hooks.should_stop.is_some_and(|f| f())
    }

    fn stopped(self) -> Stopped {
        Stopped { logs: self.logs }
    }
}

/// 最後防線：把 `fut` 裡的 panic 轉成 `Err(訊息)`（D7）。
async fn catch_round<T, Fut: Future<Output = T>>(fut: Fut) -> Result<T, String> {
    AssertUnwindSafe(fut)
        .catch_unwind()
        .await
        .map_err(|payload| panic_message(payload.as_ref()))
}

/// 一個來源跑完：送出它的日誌，若被要求停止就在這個邊界中止。
macro_rules! source_done {
    ($progress:expr, $outcome_logs:expr) => {
        $progress.add(&$outcome_logs);
        if $progress.stop_requested() {
            return Err($progress.stopped());
        }
    };
}

async fn assemble<F: Fetch>(
    fetch: &F,
    clock: &Clock,
    prev: &PreviousOutput,
    net_timed_out: bool,
    hooks: &mut RoundHooks<'_>,
) -> Result<(Output, Vec<String>, bool), Stopped> {
    let mut errors: Vec<String> = Vec::new();
    let mut progress = Progress {
        logs: Vec::new(),
        hooks,
    };
    if net_timed_out {
        errors.push(MSG_NET_TIMEOUT.to_string());
    }

    let (start, end) = events::window(clock.tpe_date());
    progress.add(&[format!(
        "抓取總經日曆＋台股動態事件＋行情＋休市日曆（{} ~ {}）…",
        iso_date(start),
        iso_date(end)
    )]);

    // B-FLOW-5：序列執行，順序即 `errors` 的順序；每個來源跑完就送出它的日誌並檢查停止要求。
    let macro_o = macro_ff::run_guarded(fetch, clock, prev).await;
    source_done!(progress, macro_o.logs);
    let div_o = dividend::run_guarded(fetch, clock, prev).await;
    source_done!(progress, div_o.logs);
    let meeting_o = meeting::run_guarded(fetch, clock, prev).await;
    source_done!(progress, meeting_o.logs);
    let top_o = top100::run_guarded(fetch, clock, prev).await;
    source_done!(progress, top_o.logs);
    let conf_o = conference::run_guarded(fetch, clock, prev, &top_o.value.codes).await;
    source_done!(progress, conf_o.logs);
    let punish_o = punish::run_guarded(fetch, clock, prev).await;
    source_done!(progress, punish_o.logs);
    let quotes_o = quotes::run_guarded(fetch, clock, prev).await;
    source_done!(progress, quotes_o.logs);
    let sofr_o = sofr::run_guarded(fetch, clock, prev).await;
    source_done!(progress, sofr_o.logs);
    let holidays_o = holidays::run_guarded(fetch, clock, prev).await;
    source_done!(progress, holidays_o.logs);
    // 動態桌布三鍵：錯誤只進 `wallpaper_errors`、不計 fresh（型別上就沒有這兩個欄位）。
    let intraday_o = twii::run_intraday_guarded(fetch, clock, prev).await;
    source_done!(progress, intraday_o.logs);
    let daily_o = twii::run_daily_guarded(fetch, clock, prev).await;
    source_done!(progress, daily_o.logs);
    let margin_o = margin::run_guarded(fetch, clock, prev).await;
    // 最後一個來源之後不再檢查停止：輸出已完整，讓呼叫端照常寫檔。
    progress.add(&margin_o.logs);

    // `fetched`（B-FLOW-7）：punish、Yahoo 行情、三個桌布鍵不計。
    let any_fresh = macro_o.fresh
        || div_o.fresh
        || meeting_o.fresh
        || top_o.fresh
        || conf_o.fresh
        || sofr_o.fresh
        || holidays_o.fresh;
    // 排程用的「有任何來源成功」（定義與理由見 [`RoundResult::any_source_ok`]）：處置股兩個市場各記一筆
    // 錯誤、行情每檔各記一筆錯誤，所以「錯誤數少於來源數」＝至少一個成功。
    let any_source_ok =
        any_fresh || punish_o.errors.len() < 2 || quotes_o.errors.len() < QUOTES.len();

    // `errors`：依 Python `main()` 的 append 順序（B-FLOW-9）。
    errors.extend(macro_o.errors.iter().cloned());
    errors.extend(div_o.errors.iter().cloned());
    errors.extend(meeting_o.errors.iter().cloned());
    errors.extend(top_o.errors.iter().cloned());
    errors.extend(conf_o.errors.iter().cloned());
    errors.extend(punish_o.errors.iter().cloned());
    errors.extend(quotes_o.errors.iter().cloned());
    errors.extend(sofr_o.errors.iter().cloned());
    errors.extend(holidays_o.errors.iter().cloned());

    let mut wallpaper_errors: Vec<String> = Vec::new();
    wallpaper_errors.extend(intraday_o.wallpaper_errors.iter().cloned());
    wallpaper_errors.extend(daily_o.wallpaper_errors.iter().cloned());
    wallpaper_errors.extend(margin_o.wallpaper_errors.iter().cloned());

    // 事件：除權息 → 股東會 → 法說會／財報（B-EVT-1），窗口、去重、排序（B-EVT-2～4）。
    let mut raw = div_o.value;
    raw.extend(meeting_o.value);
    raw.extend(conf_o.value);
    let events = events::merge_events(raw, start, end);

    // `quotes`：12 檔，SOFR 3M 有值就接在最後（B-OUT-10）。
    let mut quote_list: Vec<Quote> = quotes_o.value;
    if let Some(s) = sofr_o.value {
        quote_list.push(s);
    }

    let punish_list = punish_o.value;
    let macro_list = macro_o.value;
    let counts = Counts {
        dividend: count_of(&events, EventType::Dividend),
        meeting: count_of(&events, EventType::Meeting),
        conference: count_of(&events, EventType::Conference),
        earnings: count_of(&events, EventType::Earnings),
        macro_events: macro_list.len(),
        quotes: quote_list.len(),
        punish: punish_list.len(),
    };

    let now_str = clock.local_stamp();
    let fetched = if any_fresh {
        now_str.clone()
    } else {
        // `old.get("fetched") or old.get("updated") or now_str`：空字串視為沒有（Python 的 falsy）。
        non_empty_string(prev, "fetched")
            .or_else(|| non_empty_string(prev, "updated"))
            .unwrap_or_else(|| now_str.clone())
    };

    let holiday_list = holidays_o.value;
    progress.logs.push(summary_line(
        &counts,
        &punish_list,
        holiday_list.as_ref().map(Vec::len),
        &errors,
        &wallpaper_errors,
    ));

    let output = Output {
        updated: now_str,
        fetched,
        window: Window {
            start: iso_date(start),
            end: iso_date(end),
        },
        counts,
        errors,
        wallpaper_errors,
        sources: Sources::default(),
        macro_meta: MacroMeta::default(),
        macro_events: macro_list,
        events,
        punish: punish_list,
        quotes: quote_list,
        top100: top_o.value.codes,
        top100_date: top_o.value.date,
        holidays: holiday_list,
        twii_intraday: intraday_o.value,
        twii_daily: daily_o.value,
        margin: margin_o.value,
    };
    Ok((output, progress.logs, any_source_ok))
}

fn non_empty_string(prev: &PreviousOutput, key: &str) -> Option<String> {
    prev.get::<String>(key).filter(|s| !s.is_empty())
}

/// B-FLOW-18 結尾那一行（`:1303-1314`）。
fn summary_line(
    counts: &Counts,
    punish: &[output::Punish],
    holidays: Option<usize>,
    errors: &[String],
    wallpaper_errors: &[String],
) -> String {
    let h_desc = match holidays {
        Some(n) => format!("{n} 筆"),
        None => "抓取失敗".to_string(),
    };
    let quote_total = QUOTES.len() + 1; // +1＝SOFR 3M（NY Fed，不在 QUOTES 清單內）
    let punish_twse = punish
        .iter()
        .filter(|p| p.market == punish::MARKET_TWSE)
        .count();
    let punish_tpex = punish
        .iter()
        .filter(|p| p.market == punish::MARKET_TPEX)
        .count();
    let mut s = format!(
        "完成：總經 {}、除權息 {}、股東會 {}、財報 {}、法說會 {} 筆、處置 {} 筆（上市 {punish_twse}／上櫃 {punish_tpex}）、行情 {}/{quote_total}、休市日曆 {h_desc}",
        counts.macro_events,
        counts.dividend,
        counts.meeting,
        counts.earnings,
        counts.conference,
        counts.punish,
        counts.quotes,
    );
    if !errors.is_empty() {
        s.push_str(&format!(
            "；警告 {} 項：{}",
            errors.len(),
            errors.join("；")
        ));
    }
    if !wallpaper_errors.is_empty() {
        s.push_str(&format!(
            "；桌布資料警告 {} 項：{}",
            wallpaper_errors.len(),
            wallpaper_errors.join("；")
        ));
    }
    s
}

/// 寫 `<dir>/tw_events.json`：先清掉同目錄超過 [`STALE_TMP_AGE`] 的殘留暫存檔，再原子寫入
/// （[`output::write_atomic`]）。清理失敗不影響寫檔；寫檔失敗回傳錯誤（呼叫端只記錄，D7）。
pub fn write_round(dir: &Path, output: &Output) -> io::Result<()> {
    clean_stale_tmp(dir, SystemTime::now());
    output::write_atomic(dir, output)
}

/// 刪除 `dir` 下名為 `tw_events.json.<任意非空>.tmp`、且修改時間比 `now` 早超過 [`STALE_TMP_AGE`] 的**檔案**；
/// 回傳刪掉幾個。目錄不存在、讀不到、刪不掉都靜默略過。`now` 可注入以便測試。
fn clean_stale_tmp(dir: &Path, now: SystemTime) -> usize {
    let prefix = format!("{OUTPUT_FILE}.");
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in rd.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let is_tmp = name.len() > prefix.len() + ".tmp".len()
            && name.starts_with(&prefix)
            && name.ends_with(".tmp");
        if !is_tmp {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let stale = meta
            .modified()
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .is_some_and(|age| age > STALE_TMP_AGE);
        if stale && std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::errors::normalize_output;
    use crate::fetch::errors::FetchError;
    use crate::fetch::fixture::{scenario_dirs, FixtureFetch, Scenario};
    use crate::fetch::http::{Request, Response};
    use crate::fetch::test_util::{block_on, py_fixture, TempDir};
    use serde_json::{json, Value};

    // ───────────── 4.2：整份輸出與 Python oracle 等價 ─────────────

    /// 兩個 JSON 值的差異路徑（最多 `limit` 條）；相等回空。
    fn diff(path: &str, a: &Value, b: &Value, out: &mut Vec<String>, limit: usize) {
        if out.len() >= limit {
            return;
        }
        match (a, b) {
            (Value::Object(x), Value::Object(y)) => {
                for k in x.keys().chain(y.keys().filter(|k| !x.contains_key(*k))) {
                    let p = format!("{path}.{k}");
                    match (x.get(k), y.get(k)) {
                        (Some(l), Some(r)) => diff(&p, l, r, out, limit),
                        (Some(l), None) => out.push(format!("{p}：只有 Rust 有 {}", short(l))),
                        (None, Some(r)) => out.push(format!("{p}：只有 Python 有 {}", short(r))),
                        (None, None) => {}
                    }
                }
            }
            (Value::Array(x), Value::Array(y)) => {
                if x.len() != y.len() {
                    out.push(format!("{path}：長度 rust={} python={}", x.len(), y.len()));
                }
                for (i, (l, r)) in x.iter().zip(y).enumerate() {
                    diff(&format!("{path}[{i}]"), l, r, out, limit);
                }
            }
            _ => {
                if a != b {
                    out.push(format!("{path}：rust={} python={}", short(a), short(b)));
                }
            }
        }
    }

    fn short(v: &Value) -> String {
        let s = v.to_string();
        if s.chars().count() > 120 {
            format!("{}…", s.chars().take(120).collect::<String>())
        } else {
            s
        }
    }

    fn run_scenario(sc: &Scenario) -> Value {
        let result = block_on(run_round(&sc.fetch, &sc.clock, &sc.previous(), false));
        let output = result.output.expect("整輪不應 panic");
        serde_json::to_value(&output).expect("輸出可序列化")
    }

    fn compare_scenario(name: &str, sc: &Scenario) -> Vec<String> {
        let got = normalize_output(run_scenario(sc));
        let expected = normalize_output(sc.expected().expect("expected.json"));
        let mut d = Vec::new();
        diff("$", &got, &expected, &mut d, 20);
        d.into_iter().map(|l| format!("{name} {l}")).collect()
    }

    fn all_scenarios(deliberate: bool) -> Vec<(String, Scenario)> {
        scenario_dirs()
            .into_iter()
            .map(|dir| {
                let name = dir.file_name().unwrap().to_string_lossy().into_owned();
                let sc = Scenario::load(&dir).unwrap_or_else(|e| panic!("載入 {name}：{e}"));
                (name, sc)
            })
            .filter(|(name, _)| name.starts_with("deliberate-") == deliberate)
            .collect()
    }

    #[test]
    fn equivalence_every_python_scenario_matches_expected_json() {
        let scenarios = all_scenarios(false);
        assert!(!scenarios.is_empty(), "找不到任何一般情境");
        let diffs: Vec<String> = scenarios
            .iter()
            .flat_map(|(name, sc)| compare_scenario(name, sc))
            .collect();
        assert!(
            diffs.is_empty(),
            "與 Python 輸出不一致：\n{}",
            diffs.join("\n")
        );
    }

    #[test]
    fn equivalence_every_deliberate_improvement_scenario_matches_expected_json() {
        let scenarios = all_scenarios(true);
        assert!(!scenarios.is_empty(), "找不到 deliberate-* 情境");
        let diffs: Vec<String> = scenarios
            .iter()
            .flat_map(|(name, sc)| compare_scenario(name, sc))
            .collect();
        assert!(
            diffs.is_empty(),
            "刻意改良情境與期望檔不一致：\n{}",
            diffs.join("\n")
        );
    }

    #[test]
    fn the_diff_helper_reports_paths() {
        let a = json!({"a": [1, {"b": 2}], "c": 1, "only_rs": 1});
        let b = json!({"a": [1, {"b": 3}], "c": 1, "only_py": 2});
        let mut d = Vec::new();
        diff("$", &a, &b, &mut d, 20);
        d.sort();
        assert_eq!(
            d,
            [
                "$.a[1].b：rust=2 python=3",
                "$.only_py：只有 Python 有 2",
                "$.only_rs：只有 Rust 有 1",
            ]
        );
        let mut same = Vec::new();
        diff("$", &a, &a, &mut same, 20);
        assert!(same.is_empty());
    }

    /// `run_round` 的 future 必須是 `Send`（5.x 要丟進 `tauri::async_runtime::spawn`）；只需要能編譯。
    #[test]
    fn run_round_future_is_send() {
        fn assert_send<T: Send>(_: &T) {}
        let sc = Scenario::named("recorded-20261005").unwrap();
        let prev = PreviousOutput::default();
        let fut = run_round(&sc.fetch, &sc.clock, &prev, false);
        assert_send(&fut);
    }

    #[test]
    fn the_output_round_is_deterministic_for_a_scenario() {
        let sc = Scenario::named("recorded-20261005").unwrap();
        assert_eq!(run_scenario(&sc), run_scenario(&sc));
    }

    // ───────────── 4.1：組裝層行為（Python `MainIntegrationTest` 的對應） ─────────────

    /// 疊在 [`FixtureFetch`] 上的覆寫：`rule(url)` 回 `Some` 就用它，否則照回放。可以 panic（測試 D7）。
    type Rule = Box<dyn Fn(&str) -> Option<Result<Response, FetchError>> + Send + Sync>;

    struct Overlay {
        inner: FixtureFetch,
        rule: Rule,
    }

    impl Fetch for Overlay {
        async fn fetch(&self, req: &Request) -> Result<Response, FetchError> {
            match (self.rule)(&req.url) {
                Some(r) => r,
                None => self.inner.fetch(req).await,
            }
        }
    }

    fn net_err(msg: &str) -> Option<Result<Response, FetchError>> {
        Some(Err(FetchError::Network(msg.to_string())))
    }

    fn ok_body(body: &str) -> Option<Result<Response, FetchError>> {
        Some(Ok(Response {
            status: 200,
            body: body.as_bytes().to_vec(),
        }))
    }

    const OLD_FETCHED: &str = "2026-09-01 10:00";

    fn old_with_only_stamps() -> PreviousOutput {
        PreviousOutput::from_value(json!({"fetched": OLD_FETCHED, "updated": OLD_FETCHED}))
    }

    fn wallpaper_url(url: &str) -> bool {
        url == twii::URL_INTRADAY
            || url == twii::URL_DAILY
            || url.contains("MI_MARGN")
            || url == top100::URL_DAY_ALL
    }

    /// Python `MainIntegrationTest.run_main`：除桌布三鍵（與其所需的收盤價）外的來源全部失敗；
    /// `rule` 可再蓋掉個別網址。`holidays_ok` 讓休市日曆照回放成功。
    fn run_main(
        holidays_ok: bool,
        rule: impl Fn(&str) -> Option<Result<Response, FetchError>> + Send + Sync + 'static,
    ) -> Output {
        let sc = Scenario::named("recorded-20261005").unwrap();
        let fetch = Overlay {
            inner: sc.fetch,
            rule: Box::new(move |url| {
                if let Some(r) = rule(url) {
                    return Some(r);
                }
                if wallpaper_url(url) || (holidays_ok && url == holidays::URL_HOLIDAY) {
                    None
                } else {
                    net_err("模擬來源失敗")
                }
            }),
        };
        let result = block_on(run_round(&fetch, &sc.clock, &old_with_only_stamps(), false));
        result.output.expect("整輪不應 panic")
    }

    #[test]
    fn truncated_responses_in_wallpaper_keys_never_stop_the_round() {
        // 截斷的日 K 回應（JSON 傳到一半）、券資比連線中斷：既有鍵照常更新、桌布鍵沿用舊值（這裡沒有舊值＝省略）、
        // 錯誤進 wallpaper_errors。
        let full = serde_json::to_string(&py_fixture("yahoo_twii_1d_3mo.json")).unwrap();
        let truncated = full[..full.len() / 2].to_string();
        let out = run_main(true, move |url| {
            if url == twii::URL_DAILY {
                ok_body(&truncated)
            } else if url.contains("MI_MARGN") {
                net_err("IncompleteRead(3 bytes read, 97 more expected)")
            } else {
                None
            }
        });
        assert!(out.holidays.is_some(), "既有鍵（休市日曆）照常更新");
        assert_ne!(out.fetched, OLD_FETCHED, "有既有來源成功，fetched 照常刷新");
        assert!(out.twii_intraday.is_some(), "其他桌布鍵照常更新");
        assert!(out.twii_daily.is_none());
        assert!(out.margin.is_none());
        assert_eq!(out.wallpaper_errors.len(), 2, "{:?}", out.wallpaper_errors);
        assert!(out.wallpaper_errors[0].starts_with(twii::ERR_PREFIX_DAILY));
        assert!(out.wallpaper_errors[1].starts_with(margin::ERR_PREFIX));
    }

    #[test]
    fn an_unexpected_failure_in_a_wallpaper_source_never_stops_the_round() {
        // 防線第二層：桌布來源 panic（Python 的「非白名單例外」）只記 wallpaper_errors、不拖垮整輪。
        let sc = Scenario::named("recorded-20261005").unwrap();
        let fetch = Overlay {
            inner: sc.fetch,
            rule: Box::new(|url| {
                if url.contains("MI_MARGN") {
                    panic!("意外的程式錯誤");
                }
                if wallpaper_url(url) || url == holidays::URL_HOLIDAY {
                    None
                } else {
                    net_err("模擬來源失敗")
                }
            }),
        };
        let result = block_on(run_round(&fetch, &sc.clock, &old_with_only_stamps(), false));
        let out = result.output.expect("整輪不應 panic");
        assert!(out.holidays.is_some());
        assert!(out.twii_daily.is_some());
        assert!(out.margin.is_none());
        assert!(
            out.wallpaper_errors
                .iter()
                .any(|e| e.contains("意外的程式錯誤")),
            "{:?}",
            out.wallpaper_errors
        );
        assert!(!out.errors.iter().any(|e| e.contains("意外的程式錯誤")));
    }

    #[test]
    fn wallpaper_keys_alone_do_not_refresh_fetched() {
        let out = run_main(false, |_| None);
        assert!(out.twii_intraday.is_some() && out.twii_daily.is_some() && out.margin.is_some());
        assert!(!out.errors.is_empty(), "原有來源確實都失敗了");
        assert_eq!(
            out.fetched, OLD_FETCHED,
            "只有桌布鍵抓到新資料時，fetched 必須維持舊值"
        );
        assert_ne!(out.updated, OLD_FETCHED, "updated 仍每輪刷新");
        assert!(out.wallpaper_errors.is_empty());
    }

    /// spec「fetched 只在抓到新資料時刷新」的「只有行情抓得到」情境（review m4）：Yahoo 行情條 12 檔照回放、
    /// 其餘來源（含 SOFR、桌布三鍵）全部失敗 → `quotes` 有新資料，但 `fetched` 必須維持舊值。
    #[test]
    fn only_yahoo_quotes_succeeding_does_not_refresh_fetched() {
        const CHART_PREFIX: &str = "https://query1.finance.yahoo.com/v8/finance/chart/";
        let sc = Scenario::named("recorded-20261005").unwrap();
        let fetch = Overlay {
            inner: sc.fetch,
            rule: Box::new(|url| {
                if url.starts_with(CHART_PREFIX) && url.ends_with("?interval=1d&range=5d") {
                    None
                } else {
                    net_err("模擬來源失敗")
                }
            }),
        };
        let result = block_on(run_round(&fetch, &sc.clock, &old_with_only_stamps(), false));
        let out = result.output.expect("整輪不應 panic");
        assert_eq!(
            out.quotes.len(),
            QUOTES.len(),
            "行情 12 檔都抓到（SOFR 失敗、沒有舊值）：{:?}",
            out.errors
        );
        assert_eq!(
            out.fetched, OLD_FETCHED,
            "只有 Yahoo 行情抓到新資料時，fetched 必須維持舊值"
        );
        assert_ne!(out.updated, OLD_FETCHED, "updated 仍每輪刷新");
    }

    #[test]
    fn wallpaper_failure_goes_to_wallpaper_errors_not_errors() {
        let ok = run_main(false, |_| None);
        let bad = run_main(false, |url| {
            if url == twii::URL_DAILY {
                net_err("模擬來源失敗")
            } else {
                None
            }
        });
        assert!(bad.twii_daily.is_none(), "沒有舊值又失敗＝省略該鍵");
        assert_eq!(bad.wallpaper_errors.len(), 1);
        assert!(bad.wallpaper_errors[0].contains("日K"));
        assert_eq!(bad.errors, ok.errors, "桌布鍵失敗不得動到既有的 errors");
        assert!(!bad.errors.iter().any(|e| e.contains("日K")));
    }

    // ───────────── 4.1：其他組裝規則 ─────────────

    #[test]
    fn net_timeout_flag_puts_the_message_first_in_errors() {
        let sc = Scenario::named("recorded-20261005").unwrap();
        let with = block_on(run_round(&sc.fetch, &sc.clock, &sc.previous(), true));
        let without = block_on(run_round(&sc.fetch, &sc.clock, &sc.previous(), false));
        let (with, without) = (with.output.unwrap(), without.output.unwrap());
        assert_eq!(with.errors[0], MSG_NET_TIMEOUT);
        assert_eq!(&with.errors[1..], &without.errors[..]);
        assert!(!without.errors.iter().any(|e| e == MSG_NET_TIMEOUT));
    }

    #[test]
    fn errors_are_recomputed_every_round_not_accumulated() {
        // 舊檔的 errors 不會被帶進來（B-FLOW-10）。
        let sc = Scenario::named("recorded-20261005").unwrap();
        let prev = PreviousOutput::from_value(
            json!({"errors": ["舊的錯誤"], "wallpaper_errors": ["舊的桌布錯誤"]}),
        );
        let out = block_on(run_round(&sc.fetch, &sc.clock, &prev, false))
            .output
            .unwrap();
        assert!(!out.errors.iter().any(|e| e.contains("舊的")));
        assert!(!out.wallpaper_errors.iter().any(|e| e.contains("舊的")));
    }

    #[test]
    fn fetched_falls_back_prev_fetched_then_prev_updated_then_now() {
        let sc = Scenario::named("all-sources-fail-no-old").unwrap();
        let run = |prev: Value| {
            block_on(run_round(
                &sc.fetch,
                &sc.clock,
                &PreviousOutput::from_value(prev),
                false,
            ))
            .output
            .unwrap()
        };
        let now = sc.clock.local_stamp();
        assert_eq!(run(json!({"fetched": "F", "updated": "U"})).fetched, "F");
        assert_eq!(run(json!({"fetched": "", "updated": "U"})).fetched, "U");
        assert_eq!(run(json!({"updated": "U"})).fetched, "U");
        // D8-3：非字串視為不存在（Python 會原樣輸出 5）
        assert_eq!(run(json!({"fetched": 5, "updated": 6})).fetched, now);
        assert_eq!(run(json!({})).fetched, now);
        assert_eq!(run(json!({"fetched": "F"})).updated, now);
    }

    #[test]
    fn summary_line_matches_the_python_format() {
        let counts = Counts {
            dividend: 1,
            meeting: 2,
            conference: 3,
            earnings: 4,
            macro_events: 5,
            quotes: 13,
            punish: 2,
        };
        let p = |m: &str| output::Punish {
            code: "1".into(),
            name: "n".into(),
            start: "s".into(),
            end: "e".into(),
            times: 1,
            market: m.into(),
        };
        let punish = [p("上市"), p("上櫃")];
        assert_eq!(
            summary_line(&counts, &punish, Some(7), &[], &[]),
            "完成：總經 5、除權息 1、股東會 2、財報 4、法說會 3 筆、處置 2 筆（上市 1／上櫃 1）、行情 13/13、休市日曆 7 筆"
        );
        assert_eq!(
            summary_line(
                &counts,
                &punish,
                None,
                &["a".into(), "b".into()],
                &["w".into()]
            ),
            "完成：總經 5、除權息 1、股東會 2、財報 4、法說會 3 筆、處置 2 筆（上市 1／上櫃 1）、行情 13/13、休市日曆 抓取失敗；警告 2 項：a；b；桌布資料警告 1 項：w"
        );
    }

    #[test]
    fn logs_start_with_the_window_and_end_with_the_summary() {
        let sc = Scenario::named("recorded-20261005").unwrap();
        let r = block_on(run_round(&sc.fetch, &sc.clock, &sc.previous(), false));
        assert_eq!(
            r.logs[0],
            "抓取總經日曆＋台股動態事件＋行情＋休市日曆（2026-10-05 ~ 2026-10-19）…"
        );
        assert!(r.logs.last().unwrap().starts_with("完成：總經 "));
        assert_eq!(r.logs.iter().filter(|l| l.starts_with("完成：")).count(), 1);
    }

    #[test]
    fn a_panic_outside_the_sources_is_caught_and_yields_no_output() {
        let got = block_on(catch_round(async {
            if true {
                panic!("組裝 bug {}", 1);
            }
            5
        }));
        assert_eq!(got, Err("組裝 bug 1".to_string()));
        let ok = block_on(catch_round(async { 5 }));
        assert_eq!(ok, Ok(5));
        let non_text = block_on(catch_round(async {
            if true {
                std::panic::panic_any(7_i32);
            }
            5
        }));
        assert_eq!(non_text, Err("（非文字的 panic 載荷）".to_string()));
    }

    #[test]
    fn every_source_panicking_still_yields_a_full_output() {
        // 每個請求都 panic：十二個來源各自隔離，整輪仍有輸出（D7）。
        let sc = Scenario::named("recorded-20261005").unwrap();
        let prev = Scenario::named("all-sources-fail-with-old")
            .unwrap()
            .previous();
        let r = block_on(run_round(
            &crate::fetch::test_util::PanicFetch,
            &sc.clock,
            &prev,
            false,
        ));
        let out = r.output.expect("來源 panic 不應使整輪失敗");
        assert!(
            out.errors.iter().all(|e| e.contains("panic")),
            "{:?}",
            out.errors
        );
        assert!(!out.errors.is_empty());
        assert_eq!(out.fetched, prev.get::<String>("fetched").unwrap());
        assert_eq!(out.counts.quotes, 13, "沿用舊檔的行情");
        assert_eq!(out.wallpaper_errors.len(), 3);
    }

    // ───────────── write_round ─────────────

    fn sample_output() -> Output {
        let sc = Scenario::named("recorded-20261005").unwrap();
        block_on(run_round(
            &sc.fetch,
            &sc.clock,
            &PreviousOutput::default(),
            false,
        ))
        .output
        .unwrap()
    }

    #[test]
    fn write_round_writes_a_readable_file() {
        let dir = TempDir::new("write-round");
        let out = sample_output();
        write_round(dir.path(), &out).unwrap();
        let back = output::read_previous(dir.path());
        assert_eq!(
            back.get::<String>("updated").as_deref(),
            Some(out.updated.as_str())
        );
    }

    #[test]
    fn stale_tmp_files_are_removed_and_fresh_or_unrelated_files_kept() {
        let dir = TempDir::new("stale-tmp");
        let p = dir.path();
        let files = [
            "tw_events.json.1234.tmp",
            "tw_events.json.99.tmp",
            "tw_events.json",
            "tw_events.json..tmp",
            "tw_events.json.tmp",
            "other.json.1.tmp",
            "tw_events.json.1.bak",
        ];
        for f in files {
            std::fs::write(p.join(f), "x").unwrap();
        }
        std::fs::create_dir(p.join("tw_events.json.5.tmp")).unwrap();

        // 剛建立的檔案：現在視角下不算舊，一個都不刪。
        assert_eq!(clean_stale_tmp(p, SystemTime::now()), 0);
        // 邊界（review m1）：以檔案實際的修改時間為基準，正好一小時不刪（要「超過」），多 1 秒才刪。
        // 兩個符合檔名的檔案修改時間可能差幾毫秒，所以只拿其中一個當基準，另一個不列入斷言。
        let probe = p.join("tw_events.json.1234.tmp");
        let mtime = std::fs::metadata(&probe).unwrap().modified().unwrap();
        clean_stale_tmp(p, mtime + STALE_TMP_AGE);
        assert!(probe.exists(), "正好一小時不刪");
        clean_stale_tmp(p, mtime + STALE_TMP_AGE + Duration::from_secs(1));
        assert!(!probe.exists(), "超過一小時才刪");
        // 重建被刪掉的檔案，讓下面「兩小時後」的斷言維持原樣。
        for f in ["tw_events.json.1234.tmp", "tw_events.json.99.tmp"] {
            std::fs::write(p.join(f), "x").unwrap();
        }
        // 兩小時後：只刪符合 `tw_events.json.<非空>.tmp` 的檔案。
        let n = clean_stale_tmp(p, SystemTime::now() + 2 * STALE_TMP_AGE);
        assert_eq!(n, 2);
        for gone in ["tw_events.json.1234.tmp", "tw_events.json.99.tmp"] {
            assert!(!p.join(gone).exists(), "{gone} 應被清除");
        }
        for kept in [
            "tw_events.json",
            "tw_events.json..tmp",
            "tw_events.json.tmp",
            "other.json.1.tmp",
            "tw_events.json.1.bak",
            "tw_events.json.5.tmp",
        ] {
            assert!(p.join(kept).exists(), "{kept} 不該被清除");
        }
        assert!(p.join("tw_events.json.5.tmp").is_dir(), "同名目錄不動");
    }

    #[test]
    fn write_round_cleans_an_old_tmp_via_file_mtime() {
        // 真的把殘檔的修改時間往前調 2 小時，走正式的 write_round 路徑。
        let dir = TempDir::new("stale-tmp-real");
        let stale = dir.path().join("tw_events.json.777.tmp");
        let fresh = dir.path().join("tw_events.json.888.tmp");
        std::fs::write(&stale, "x").unwrap();
        std::fs::write(&fresh, "x").unwrap();
        let f = std::fs::OpenOptions::new()
            .write(true)
            .open(&stale)
            .unwrap();
        f.set_modified(SystemTime::now() - 2 * STALE_TMP_AGE)
            .unwrap();
        drop(f);
        write_round(dir.path(), &sample_output()).unwrap();
        assert!(!stale.exists(), "超過 1 小時的殘檔應被清掉");
        assert!(
            fresh.exists(),
            "剛留下的暫存檔（可能是另一個行程正在寫）不可動"
        );
        assert!(dir.path().join("tw_events.json").exists());
    }

    #[test]
    fn write_round_creates_a_missing_directory_and_ignores_cleanup_on_a_missing_one() {
        let dir = TempDir::new("write-round-mkdir");
        let target = dir.path().join("a").join("b");
        assert_eq!(clean_stale_tmp(&target, SystemTime::now()), 0);
        write_round(&target, &sample_output()).unwrap();
        assert!(target.join("tw_events.json").exists());
    }

    // ───────────── 5.2：掛鉤（即時日誌、停止）與 any_source_ok ─────────────

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    /// 每個請求先記一筆 `REQ:<網址>` 到 `events`，再交給 `rule`（`None`＝照回放）。
    fn recording_overlay(
        events: &Arc<Mutex<Vec<String>>>,
        rule: impl Fn(&str) -> Option<Result<Response, FetchError>> + Send + Sync + 'static,
    ) -> (Overlay, Clock) {
        let sc = Scenario::named("recorded-20261005").unwrap();
        let events = Arc::clone(events);
        (
            Overlay {
                inner: sc.fetch,
                rule: Box::new(move |url| {
                    events.lock().unwrap().push(format!("REQ:{url}"));
                    rule(url)
                }),
            },
            sc.clock,
        )
    }

    fn all_fail(_: &str) -> Option<Result<Response, FetchError>> {
        net_err("模擬來源失敗")
    }

    #[test]
    fn on_log_receives_each_sources_lines_before_the_next_source_starts() {
        // 5.1 審查 m2：日誌不能等整輪結束才一次送出。所有來源都失敗 → 每個來源跑完都有一行失敗日誌；
        // 若是批次送出，所有 `LOG:` 都會排在所有 `REQ:` 之後。
        let events = Arc::new(Mutex::new(Vec::<String>::new()));
        let (fetch, clock) = recording_overlay(&events, all_fail);
        let sink = Arc::clone(&events);
        let mut on_log = move |line: &str| sink.lock().unwrap().push(format!("LOG:{line}"));
        let mut hooks = RoundHooks {
            on_log: Some(&mut on_log),
            should_stop: None,
        };
        let result = block_on(run_round_with(
            &fetch,
            &clock,
            &old_with_only_stamps(),
            false,
            &mut hooks,
        ));
        let events = events.lock().unwrap().clone();
        assert!(
            events[0].starts_with("LOG:抓取總經日曆"),
            "第一件事是窗口行：{:?}",
            events.first()
        );
        let first_source_log = events
            .iter()
            .position(|e| e.starts_with("LOG:[總經]"))
            .expect("總經失敗的日誌行");
        let last_req = events.iter().rposition(|e| e.starts_with("REQ:")).unwrap();
        assert!(
            first_source_log < last_req,
            "總經的日誌要在後面的來源開始請求之前就送出：{events:#?}"
        );
        // 送出的行就是 `RoundResult.logs` 去掉最後的「完成：」摘要。
        let logged: Vec<&str> = events
            .iter()
            .filter_map(|e| e.strip_prefix("LOG:"))
            .collect();
        let expected: Vec<&str> = result.logs[..result.logs.len() - 1]
            .iter()
            .map(String::as_str)
            .collect();
        assert_eq!(logged, expected);
        assert!(result.logs.last().unwrap().starts_with("完成："));
    }

    #[test]
    fn should_stop_is_asked_after_each_source_but_not_after_the_last() {
        let events = Arc::new(Mutex::new(Vec::<String>::new()));
        let (fetch, clock) = recording_overlay(&events, all_fail);
        let asked = AtomicUsize::new(0);
        let stop = || {
            asked.fetch_add(1, Ordering::SeqCst);
            false
        };
        let mut hooks = RoundHooks {
            on_log: None,
            should_stop: Some(&stop),
        };
        let result = block_on(run_round_with(
            &fetch,
            &clock,
            &old_with_only_stamps(),
            false,
            &mut hooks,
        ));
        assert!(!result.aborted);
        assert!(result.output.is_ok());
        assert_eq!(
            asked.load(Ordering::SeqCst),
            11,
            "12 個來源，最後一個之後不再問（輸出已完整）"
        );
    }

    #[test]
    fn a_stop_request_halts_at_the_next_source_boundary_and_produces_no_output() {
        let events = Arc::new(Mutex::new(Vec::<String>::new()));
        let (fetch, clock) = recording_overlay(&events, all_fail);
        let asked = AtomicUsize::new(0);
        let requests_at_stop = AtomicUsize::new(usize::MAX);
        let stop_events = Arc::clone(&events);
        let stop = || {
            let n = asked.fetch_add(1, Ordering::SeqCst) + 1;
            if n == 3 {
                let reqs = stop_events
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|e| e.starts_with("REQ:"))
                    .count();
                requests_at_stop.store(reqs, Ordering::SeqCst);
                true
            } else {
                false
            }
        };
        let mut hooks = RoundHooks {
            on_log: None,
            should_stop: Some(&stop),
        };
        let result = block_on(run_round_with(
            &fetch,
            &clock,
            &old_with_only_stamps(),
            false,
            &mut hooks,
        ));
        assert!(result.aborted, "要求停止就中止");
        assert!(result.output.is_err(), "中止時不產生輸出（不寫半份檔）");
        assert!(!result.any_source_ok);
        assert_eq!(asked.load(Ordering::SeqCst), 3, "停止後不再往下問");
        let total_reqs = events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.starts_with("REQ:"))
            .count();
        assert_eq!(
            total_reqs,
            requests_at_stop.load(Ordering::SeqCst),
            "停止之後不得再對任何來源發請求"
        );
        assert!(total_reqs > 0);
        // 已跑的部分日誌保留（窗口行＋前三個來源）。
        assert!(result.logs[0].starts_with("抓取總經日曆"));
        assert!(!result.logs.iter().any(|l| l.starts_with("完成：")));
    }

    #[test]
    fn any_source_ok_is_false_only_when_nothing_succeeded() {
        // 全部失敗（含桌布三鍵與前百大）：沒有任何來源成功。
        let events = Arc::new(Mutex::new(Vec::new()));
        let (fetch, clock) = recording_overlay(&events, all_fail);
        let r = block_on(run_round(&fetch, &clock, &PreviousOutput::default(), false));
        assert!(r.output.is_ok(), "全部失敗仍產生輸出（K-15）");
        assert!(!r.any_source_ok, "全部失敗");
        assert!(!r.aborted);
    }

    #[test]
    fn any_source_ok_counts_sources_that_do_not_refresh_fetched() {
        // 只有 Yahoo 行情成功：fetched 不刷新（見 only_yahoo_quotes_succeeding_does_not_refresh_fetched），
        // 但網路與來源是通的 → 不該每 10–15 分鐘重試。
        const CHART_PREFIX: &str = "https://query1.finance.yahoo.com/v8/finance/chart/";
        let events = Arc::new(Mutex::new(Vec::new()));
        let (fetch, clock) = recording_overlay(&events, |url| {
            if url.starts_with(CHART_PREFIX) && url.ends_with("?interval=1d&range=5d") {
                None
            } else {
                net_err("模擬來源失敗")
            }
        });
        let r = block_on(run_round(&fetch, &clock, &old_with_only_stamps(), false));
        let out = r.output.as_ref().unwrap();
        assert_eq!(out.fetched, OLD_FETCHED);
        assert!(r.any_source_ok, "行情抓得到＝有來源成功");

        // 只有上櫃處置股成功（上市失敗）→ 仍算成功；兩個市場都失敗才不算。
        let (fetch, clock) = recording_overlay(&events, |url| {
            if url == punish::URL_PUNISH_TPEX {
                None
            } else {
                net_err("模擬來源失敗")
            }
        });
        let r = block_on(run_round(&fetch, &clock, &PreviousOutput::default(), false));
        assert!(r.any_source_ok, "一個處置股市場成功");
    }

    #[test]
    fn a_same_day_top100_cache_hit_is_not_a_success() {
        // 前百大當日快取命中：不發請求、也沒有錯誤——離線時它永遠「沒有錯誤」，不能因此判定這輪成功，
        // 否則首次安裝的快取＋斷網會永遠不重試。
        let events = Arc::new(Mutex::new(Vec::new()));
        let (fetch, clock) = recording_overlay(&events, all_fail);
        let prev = PreviousOutput::from_value(json!({
            "top100": ["2330", "2317"],
            "top100_date": iso_date(clock.tpe_date()),
        }));
        let r = block_on(run_round(&fetch, &clock, &prev, false));
        let out = r.output.as_ref().unwrap();
        assert!(
            !out.errors.iter().any(|e| e.starts_with(top100::ERR_PREFIX)),
            "快取命中沒有前百大錯誤：{:?}",
            out.errors
        );
        assert!(!r.any_source_ok);
    }
}
