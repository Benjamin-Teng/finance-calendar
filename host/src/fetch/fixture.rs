//! `FixtureFetch`：從樣本目錄回放（僅測試，design.md D3、D4）。
//!
//! 語意**必須**與 `tests/fetch_oracle/expect.py` 的 `_resolve`／`load_scenario`／`replay_factory`
//! 逐條一致，否則 Rust 與 Python 吃到的不是同一份輸入：
//!
//! - 情境目錄可以是錄製樣本（`manifest.json`＋`responses/`），或 `scenario.json`
//!   （`base`＋`overrides`＋`now_tpe`／`now_local`）。`base` 遞迴解析（相對於宣告它的目錄），
//!   再疊上本層 `overrides`（鍵相同＝取代，否則新增）。回應檔路徑相對於「宣告它的那個目錄」。
//! - 請求鍵＝`(方法, 網址, 編碼後的表單)`；**方法由有無 `form` 決定**（紀錄裡的 `method` 欄位
//!   不參與，同 Python 的 `_normalize`）。
//! - 回放：命中且紀錄有 `error` → `Err(Network)`，訊息取 `error` 以第一個 `": "` 切開後的
//!   後半（沒有或為空就用全文）；查無 → `Err(Network)`；其餘回傳 `status`（預設 200）與本體
//!   （`file` 的位元組，沒有 `file` 就是空本體）。非 200 **不是** `Err`——對應 Python 的
//!   `HTTPError`，由 `get_ok` 轉成 `FetchError::Status`。
//! - 時間：manifest 的 `recorded_at_tpe`／`recorded_at_local`；`scenario.json` 給 `now_tpe`
//!   會**連 `now_local` 一起重設**（沒給 `now_local` 就取同一個台北牆上時間），只給 `now_local`
//!   則只改本機時間。
//! - 另可放 `old.json` 當舊的 `tw_events.json`。
//! - `scenario.json` 的 `expected_from`（`deliberate-*` 情境，只給 Python 產期望檔用）**忽略**：
//!   Rust 一律回放原始輸入（`base`＋`overrides`），並要在同一份 `expected.json` 上達成等價。
//!
//! 每次請求（含查無的）都記錄起來，供測試斷言「某來源沒有發請求」。回放**不等待**。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Deserialize;
use serde_json::Value;
use time::format_description::well_known::Iso8601;
use time::{OffsetDateTime, PrimitiveDateTime};

use super::clock::{Clock, TPE_OFFSET};
use super::errors::FetchError;
use super::http::{Fetch, Request, RequestKey, Response};
use super::output::{PreviousOutput, OUTPUT_FILE};

/// 樣本根目錄：`host/tests/fixtures/fetch/`。
pub fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fetch")
}

/// 一筆回放紀錄（manifest 的 `requests[]` 或 scenario 的 `overrides[]`）。
#[derive(Debug, Clone, Deserialize)]
struct RawEntry {
    url: String,
    /// `[[key, value], ...]`；dict 形式會讓解析失敗（目前沒有樣本用，且 dict 順序在此不可靠）。
    #[serde(default)]
    form: Option<Vec<(String, String)>>,
    #[serde(default)]
    status: Option<u16>,
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Clone)]
struct Entry {
    /// 回應檔相對的目錄（宣告這筆紀錄的那個目錄）。
    dir: PathBuf,
    status: u16,
    file: Option<String>,
    error: Option<String>,
}

fn normalize(raw: RawEntry, dir: &Path) -> (RequestKey, Entry) {
    let req = match raw.form {
        None => Request::get(raw.url),
        Some(form) => Request::post_form(raw.url, form),
    };
    (
        req.key(),
        Entry {
            dir: dir.to_path_buf(),
            status: raw.status.unwrap_or(200),
            file: raw.file,
            error: raw.error,
        },
    )
}

#[derive(Debug, Deserialize)]
struct Manifest {
    recorded_at_tpe: String,
    #[serde(default)]
    recorded_at_local: Option<String>,
    requests: Vec<RawEntry>,
}

#[derive(Debug, Default, Deserialize)]
struct ScenarioFile {
    #[serde(default)]
    base: Option<String>,
    #[serde(default)]
    overrides: Vec<RawEntry>,
    #[serde(default)]
    now_tpe: Option<String>,
    #[serde(default)]
    now_local: Option<String>,
    // `expected_from` 刻意不宣告：不是 `deny_unknown_fields`，反序列化時直接略過。
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    let bytes = fs::read(path).map_err(|e| format!("讀不到 {}：{e}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("解析 {} 失敗：{e}", path.display()))
}

/// 對應 expect.py 的 `_resolve`：回傳 `(請求表, now_tpe 字串, now_local 字串或 None)`。
type Resolved = (HashMap<RequestKey, Entry>, String, Option<String>);

fn resolve(dir: &Path) -> Result<Resolved, String> {
    let sc_path = dir.join("scenario.json");
    let scenario: ScenarioFile = if sc_path.exists() {
        read_json(&sc_path)?
    } else {
        ScenarioFile::default()
    };
    let (mut entries, mut now_tpe, mut now_local) = if let Some(base) = &scenario.base {
        resolve(&dir.join(base))?
    } else if dir.join("manifest.json").exists() {
        let m: Manifest = read_json(&dir.join("manifest.json"))?;
        let entries = m.requests.into_iter().map(|r| normalize(r, dir)).collect();
        (entries, m.recorded_at_tpe, m.recorded_at_local)
    } else {
        return Err(format!(
            "{} 沒有 manifest.json，也沒有帶 base 的 scenario.json",
            dir.display()
        ));
    };
    for raw in scenario.overrides {
        let (key, entry) = normalize(raw, dir);
        entries.insert(key, entry);
    }
    if let Some(t) = scenario.now_tpe {
        now_tpe = t;
        now_local = scenario.now_local;
    } else if scenario.now_local.is_some() {
        now_local = scenario.now_local;
    }
    Ok((entries, now_tpe, now_local))
}

/// 對應 harness.py 的 `parse_frozen`：台北時間字串無時區時視為 +08:00；本機時間省略就取台北牆上時間。
fn parse_clock(now_tpe: &str, now_local: Option<&str>) -> Result<Clock, String> {
    let tpe = match OffsetDateTime::parse(now_tpe, &Iso8601::DEFAULT) {
        Ok(t) => t,
        Err(_) => PrimitiveDateTime::parse(now_tpe, &Iso8601::DEFAULT)
            .map_err(|e| format!("now_tpe {now_tpe:?} 不是 ISO 時間：{e}"))?
            .assume_offset(TPE_OFFSET),
    }
    .to_offset(TPE_OFFSET);
    let local = match now_local {
        Some(s) => PrimitiveDateTime::parse(s, &Iso8601::DEFAULT)
            .map_err(|e| format!("now_local {s:?} 不是 ISO 時間：{e}"))?,
        None => PrimitiveDateTime::new(tpe.date(), tpe.time()),
    };
    Ok(Clock::new(tpe, local))
}

/// 以樣本目錄回放的 [`Fetch`]。
#[derive(Debug)]
pub struct FixtureFetch {
    entries: HashMap<RequestKey, Entry>,
    requested: Mutex<Vec<Request>>,
}

impl FixtureFetch {
    /// 依請求鍵直接查回應（同步，供測試與 `fetch` 共用）；不記錄請求。
    fn replay(&self, req: &Request) -> Result<Response, FetchError> {
        self.replay_key(&req.key())
    }

    fn replay_key(&self, key: &RequestKey) -> Result<Response, FetchError> {
        let Some(entry) = self.entries.get(key) else {
            return Err(FetchError::Network(format!(
                "查無對應的錄製回應：{} {}",
                key.method.as_str(),
                key.url
            )));
        };
        if let Some(error) = &entry.error {
            let msg = match error.split_once(": ") {
                Some((_, rest)) if !rest.is_empty() => rest,
                _ => error.as_str(),
            };
            return Err(FetchError::Network(msg.to_string()));
        }
        let body = match entry.file.as_deref() {
            Some(file) if !file.is_empty() => {
                let path = entry.dir.join(file);
                fs::read(&path).map_err(|e| {
                    FetchError::Network(format!("樣本回應檔讀取失敗 {}：{e}", path.display()))
                })?
            }
            _ => Vec::new(),
        };
        Ok(Response {
            status: entry.status,
            body,
        })
    }

    /// 到目前為止被請求過的請求（依發出順序，含查無對應與 error 的）。
    pub fn requests(&self) -> Vec<Request> {
        self.requested
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// 網址完全相等的請求次數（GET、POST 都算）。
    pub fn request_count(&self, url: &str) -> usize {
        self.requests().iter().filter(|r| r.url == url).count()
    }

    /// 網址包含 `needle` 的請求次數（例如整個主機）。
    pub fn request_count_containing(&self, needle: &str) -> usize {
        self.requests()
            .iter()
            .filter(|r| r.url.contains(needle))
            .count()
    }

    /// 樣本裡登記的所有請求鍵（供「每個請求都能回放」之類的測試）。
    pub fn known_keys(&self) -> Vec<RequestKey> {
        self.entries.keys().cloned().collect()
    }
}

impl Fetch for FixtureFetch {
    async fn fetch(&self, req: &Request) -> Result<Response, FetchError> {
        self.requested
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(req.clone());
        self.replay(req)
    }
}

/// 載入好的情境：回放器＋凍結時間＋舊輸出。
#[derive(Debug)]
pub struct Scenario {
    pub dir: PathBuf,
    pub fetch: FixtureFetch,
    pub clock: Clock,
    /// 情境目錄的 `old.json`（存在才有）。
    pub old_path: Option<PathBuf>,
}

impl Scenario {
    pub fn load(dir: &Path) -> Result<Scenario, String> {
        let (entries, now_tpe, now_local) = resolve(dir)?;
        let clock = parse_clock(&now_tpe, now_local.as_deref())?;
        let old = dir.join("old.json");
        Ok(Scenario {
            dir: dir.to_path_buf(),
            fetch: FixtureFetch {
                entries,
                requested: Mutex::new(Vec::new()),
            },
            clock,
            old_path: old.exists().then_some(old),
        })
    }

    /// 以情境名稱（`host/tests/fixtures/fetch/` 下的資料夾名）載入。
    pub fn named(name: &str) -> Result<Scenario, String> {
        Scenario::load(&fixtures_root().join(name))
    }

    /// 舊輸出：有 `old.json` 就讀成 [`PreviousOutput`]，沒有（或壞掉）＝空。
    pub fn previous(&self) -> PreviousOutput {
        self.old_path
            .as_ref()
            .and_then(|p| fs::read(p).ok())
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .map(PreviousOutput::from_value)
            .unwrap_or_default()
    }

    /// 把 `old.json` 放進某資料夾當 `tw_events.json`（給走檔案路徑的測試用）；沒有 `old.json` 就不動。
    pub fn install_old_into(&self, target_dir: &Path) -> std::io::Result<()> {
        if let Some(old) = &self.old_path {
            fs::create_dir_all(target_dir)?;
            fs::copy(old, target_dir.join(OUTPUT_FILE))?;
        }
        Ok(())
    }

    /// 已提交的 `expected.json`（Python oracle 產出）。
    pub fn expected(&self) -> Result<Value, String> {
        read_json(&self.dir.join("expected.json"))
    }
}

/// 樣本根目錄下所有情境（有 `manifest.json` 或 `scenario.json` 的直接子資料夾），依名稱排序。
pub fn scenario_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(fixtures_root())
        .expect("樣本根目錄存在")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.is_dir() && (p.join("manifest.json").exists() || p.join("scenario.json").exists())
        })
        .collect();
    dirs.sort();
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::http::{get_ok, post_form_ok};
    use crate::fetch::test_util::block_on;
    use time::macros::datetime;

    const RECORDED: &str = "recorded-20261005";

    fn load(name: &str) -> Scenario {
        Scenario::named(name).unwrap_or_else(|e| panic!("載入情境 {name} 失敗：{e}"))
    }

    fn fetch_one(s: &Scenario, req: &Request) -> Result<Response, FetchError> {
        block_on(async { s.fetch.fetch(req).await })
    }

    #[test]
    fn every_scenario_loads() {
        let dirs = scenario_dirs();
        // 不寫死數量（新增情境不該讓這個測試失敗）；只擋「情境清單意外變空」與錄製樣本消失。
        assert!(!dirs.is_empty(), "找不到任何情境");
        assert!(
            dirs.iter().any(|d| d.ends_with(RECORDED)),
            "錄製樣本 {RECORDED} 不見了：{dirs:?}"
        );
        for d in dirs {
            let s = Scenario::load(&d).unwrap_or_else(|e| panic!("載入 {} 失敗：{e}", d.display()));
            assert!(
                !s.fetch.known_keys().is_empty(),
                "{} 沒有任何請求",
                d.display()
            );
            assert!(
                s.expected().is_ok(),
                "{} 的 expected.json 讀不到或壞掉",
                d.display()
            );
        }
    }

    /// 錄製樣本的每個請求都能回放：狀態碼與 manifest 一致、本體與回應檔位元組相同。
    #[test]
    fn recorded_sample_replays_every_request() {
        let dir = fixtures_root().join(RECORDED);
        let s = Scenario::load(&dir).unwrap();
        let manifest: Manifest = read_json(&dir.join("manifest.json")).unwrap();
        assert_eq!(manifest.requests.len(), 28);
        assert_eq!(s.fetch.known_keys().len(), 28);
        let mut statuses = Vec::new();
        for raw in &manifest.requests {
            let req = match &raw.form {
                None => Request::get(&raw.url),
                Some(f) => Request::post_form(&raw.url, f.clone()),
            };
            let resp = fetch_one(&s, &req).unwrap_or_else(|e| panic!("{} 回放失敗：{e}", raw.url));
            assert_eq!(resp.status, raw.status.unwrap_or(200), "{}", raw.url);
            let want = fs::read(dir.join(raw.file.as_deref().unwrap())).unwrap();
            assert_eq!(resp.body, want, "{} 本體與回應檔不同", raw.url);
            statuses.push(resp.status);
        }
        assert_eq!(
            statuses.iter().filter(|s| **s == 404).count(),
            1,
            "下週總經檔 404"
        );
        assert_eq!(s.fetch.requests().len(), 28);
    }

    #[test]
    fn recorded_sample_clock_and_no_old() {
        let s = load(RECORDED);
        assert_eq!(s.clock.tpe, datetime!(2026-10-05 01:12:14 +8));
        assert_eq!(s.clock.local, datetime!(2026-10-05 01:12:14));
        assert_eq!(s.clock.local_stamp(), "2026-10-05 01:12");
        assert!(s.old_path.is_none());
        assert!(s.previous().is_empty());
    }

    #[test]
    fn http_errors_are_responses_not_err_and_get_ok_converts() {
        let s = load(RECORDED);
        let url = "https://nfs.faireconomy.media/ff_calendar_nextweek.json";
        let r = fetch_one(&s, &Request::get(url)).unwrap();
        assert_eq!(r.status, 404);
        assert!(!r.body.is_empty(), "非 200 仍回傳本體");
        assert_eq!(
            block_on(get_ok(&s.fetch, url)),
            Err(FetchError::Status(404))
        );
        let ok = block_on(get_ok(
            &s.fetch,
            "https://nfs.faireconomy.media/ff_calendar_thisweek.json",
        ));
        assert!(ok.unwrap().len() > 100);
    }

    #[test]
    fn unknown_request_is_network_error_and_still_recorded() {
        let s = load(RECORDED);
        let req = Request::get("https://example.invalid/never-recorded");
        let err = fetch_one(&s, &req).unwrap_err();
        assert!(
            matches!(&err, FetchError::Network(m) if m.contains("查無對應的錄製回應")),
            "{err:?}"
        );
        assert_eq!(
            s.fetch
                .request_count("https://example.invalid/never-recorded"),
            1
        );
        // POST 與 GET 是不同的鍵：把錄製的 GET 網址改發 POST 也查無。
        let thisweek = "https://nfs.faireconomy.media/ff_calendar_thisweek.json";
        let post = Request::post_form(thisweek, [("a", "b")]);
        assert!(fetch_one(&s, &post).is_err());
    }

    #[test]
    fn post_form_is_matched_by_encoded_form_in_order() {
        let s = load(RECORDED);
        let mops = "https://mopsov.twse.com.tw/mops/web/ajax_t100sb02_1";
        let recorded: Vec<Request> = s
            .fetch
            .known_keys()
            .into_iter()
            .filter(|k| k.url == mops)
            .map(|k| Request {
                method: k.method,
                url: k.url,
                form: k.form.map(|f| {
                    f.split('&')
                        .map(|p| {
                            let (a, b) = p.split_once('=').unwrap();
                            (a.to_string(), b.to_string())
                        })
                        .collect()
                }),
            })
            .collect();
        assert_eq!(recorded.len(), 2, "本月＋下月兩次 POST");
        for req in &recorded {
            assert!(fetch_one(&s, req).unwrap().status == 200);
        }
        // 表單順序不同＝不同鍵＝查無。
        let mut swapped = recorded[0].clone();
        swapped.form.as_mut().unwrap().reverse();
        assert!(fetch_one(&s, &swapped).is_err());
        // 便利函式。
        let form = recorded[0].form.clone().unwrap();
        assert!(block_on(post_form_ok(&s.fetch, mops, form)).is_ok());
    }

    #[test]
    fn duplicate_requests_replay_every_time() {
        // STOCK_DAY_ALL 一輪下載兩次（K-17），同一筆紀錄要能回放多次。
        let s = load(RECORDED);
        let url = "https://openapi.twse.com.tw/v1/exchangeReport/STOCK_DAY_ALL";
        let a = block_on(get_ok(&s.fetch, url)).unwrap();
        let b = block_on(get_ok(&s.fetch, url)).unwrap();
        assert_eq!(a, b);
        assert_eq!(s.fetch.request_count(url), 2);
    }

    #[test]
    fn error_override_returns_network_error_with_python_message() {
        // all-sources-fail-*：error 紀錄 `URLError: <urlopen error ...>`，取 `": "` 之後的部分。
        let s = load("all-sources-fail-with-old");
        let err = fetch_one(
            &s,
            &Request::get("https://nfs.faireconomy.media/ff_calendar_thisweek.json"),
        )
        .unwrap_err();
        assert_eq!(
            err,
            FetchError::Network("<urlopen error [Errno 11001] getaddrinfo failed>".into())
        );
    }

    #[test]
    fn status_override_replaces_the_base_entry() {
        // macro-this-week-fail-keeps-old：本週總經 500、下週總經覆寫成 200（有手工檔）。
        let s = load("macro-this-week-fail-keeps-old");
        let this = fetch_one(
            &s,
            &Request::get("https://nfs.faireconomy.media/ff_calendar_thisweek.json"),
        )
        .unwrap();
        assert_eq!(this.status, 500);
        let next = fetch_one(
            &s,
            &Request::get("https://nfs.faireconomy.media/ff_calendar_nextweek.json"),
        )
        .unwrap();
        assert_eq!(next.status, 200);
        assert!(String::from_utf8(next.body)
            .unwrap()
            .contains("Core CPI m/m"));
        // 沒被覆寫的請求仍來自錄製樣本。
        assert!(block_on(get_ok(
            &s.fetch,
            "https://www.twse.com.tw/rwd/zh/exRight/TWT48U?response=json"
        ))
        .is_ok());
    }

    #[test]
    fn override_file_is_resolved_relative_to_the_declaring_directory() {
        // conference-range-date：MOPS 10 月 POST 覆寫成情境目錄裡的手工 HTML；其餘仍讀錄製目錄。
        let s = load("conference-range-date");
        let sc: ScenarioFile = read_json(&s.dir.join("scenario.json")).unwrap();
        let ov = sc
            .overrides
            .iter()
            .find(|o| o.form.is_some())
            .expect("有 POST 覆寫");
        let req = Request::post_form(&ov.url, ov.form.clone().unwrap());
        let resp = fetch_one(&s, &req).unwrap();
        let want = fs::read(s.dir.join(ov.file.as_deref().unwrap())).unwrap();
        assert_eq!(resp.body, want);
        // 另一個 POST（下個月）沒被覆寫，本體來自 recorded 目錄。
        let rec = fixtures_root().join(RECORDED);
        let manifest: Manifest = read_json(&rec.join("manifest.json")).unwrap();
        let other = manifest
            .requests
            .iter()
            .find(|r| r.url == ov.url && r.form.as_ref() != ov.form.as_ref())
            .expect("錄製樣本另一個 MOPS POST");
        let resp = fetch_one(
            &s,
            &Request::post_form(&other.url, other.form.clone().unwrap()),
        )
        .unwrap();
        assert_eq!(
            resp.body,
            fs::read(rec.join(other.file.as_deref().unwrap())).unwrap()
        );
    }

    #[test]
    fn now_tpe_override_resets_local_to_the_taipei_wall_clock() {
        // twii-intraday-1200：只給 now_tpe → 本機時間取同一個台北牆上時間。
        let s = load("twii-intraday-1200");
        assert_eq!(s.clock.tpe, datetime!(2026-10-02 12:00:00 +8));
        assert_eq!(s.clock.local, datetime!(2026-10-02 12:00:00));
        let s = load("twii-intraday-1500");
        assert_eq!(s.clock.tpe, datetime!(2026-10-02 15:00:00 +8));
        assert_eq!(s.clock.local_stamp(), "2026-10-02 15:00");
    }

    #[test]
    fn parse_clock_follows_the_python_rules() {
        // 無時區的台北字串視為 +08:00。
        let c = parse_clock("2026-10-02T12:00:00", None).unwrap();
        assert_eq!(c.tpe, datetime!(2026-10-02 12:00:00 +8));
        // 其他時區會被換算到 +08:00（Python `astimezone(TPE)`）；本機時間省略＝換算後的台北牆上時間。
        let c = parse_clock("2026-10-02T04:00:00Z", None).unwrap();
        assert_eq!(c.tpe, datetime!(2026-10-02 12:00:00 +8));
        assert_eq!(c.local, datetime!(2026-10-02 12:00:00));
        // 本機時間獨立指定（人在 JST）。
        let c = parse_clock("2026-10-04T23:30:00+08:00", Some("2026-10-05T00:30:00")).unwrap();
        assert_eq!(c.tpe.date(), time::macros::date!(2026 - 10 - 04));
        assert_eq!(c.local, datetime!(2026-10-05 00:30:00));
        assert!(parse_clock("garbage", None).is_err());
    }

    #[test]
    fn old_json_is_loaded_and_expected_from_is_ignored() {
        // deliberate-old-wrong-types：old.json 是型別錯誤的版本；expected_from 指向 old.clean.json，
        // 但 Rust 必須吃原始的 old.json。
        let s = load("deliberate-old-wrong-types");
        assert!(s.old_path.as_ref().unwrap().ends_with("old.json"));
        let prev = s.previous();
        assert_eq!(prev.raw("top100"), Some(&Value::from(5)));
        // 型別不對的鍵：events 是物件、top100 與 top100_date 是整數——取不出型別化的值。
        assert!(prev.get::<Vec<String>>("top100").is_none());
        assert!(prev
            .get::<Vec<super::super::output::Event>>("events")
            .is_none());
        assert!(prev.get::<String>("top100_date").is_none());
        assert!(prev.raw("events").is_some(), "原始值仍在，只是型別不對");

        // deliberate-macro-nonstring-forecast：回放的是 overrides 的原始（含數字 forecast）檔，
        // 不是 expected_from 指向的 ff_this.clean.json。
        let s = load("deliberate-macro-nonstring-forecast");
        let body = block_on(get_ok(
            &s.fetch,
            "https://nfs.faireconomy.media/ff_calendar_thisweek.json",
        ))
        .unwrap();
        assert_eq!(body, fs::read(s.dir.join("ff_this.json")).unwrap());
        assert_ne!(body, fs::read(s.dir.join("ff_this.clean.json")).unwrap());
    }

    #[test]
    fn previous_and_install_old() {
        let s = load("top100-same-day-cache");
        let prev = s.previous();
        assert_eq!(
            prev.get::<String>("top100_date").as_deref(),
            Some("2026-10-05")
        );
        assert_eq!(prev.get::<Vec<String>>("top100").map(|v| v.len()), Some(3));
        let dir = crate::fetch::test_util::TempDir::new("fixture-old");
        s.install_old_into(dir.path()).unwrap();
        assert_eq!(
            crate::fetch::output::read_previous(dir.path())
                .get::<String>("top100_date")
                .as_deref(),
            Some("2026-10-05")
        );
        // 沒有 old.json 的情境：什麼都不放。
        let none = load(RECORDED);
        let dir2 = crate::fetch::test_util::TempDir::new("fixture-noold");
        none.install_old_into(dir2.path()).unwrap();
        assert!(!dir2.path().join(OUTPUT_FILE).exists());
    }

    #[test]
    fn request_log_supports_no_request_assertions() {
        // top100-same-day-cache：絆線＝`t187ap03_L`（回 500）；快取命中時整個流程不該碰它。
        let s = load("top100-same-day-cache");
        let trip = "https://openapi.twse.com.tw/v1/opendata/t187ap03_L";
        assert_eq!(s.fetch.request_count(trip), 0);
        let r = fetch_one(&s, &Request::get(trip)).unwrap();
        assert_eq!(r.status, 500);
        assert_eq!(s.fetch.request_count(trip), 1);
        assert_eq!(s.fetch.request_count_containing("openapi.twse.com.tw"), 1);
        assert_eq!(s.fetch.requests().len(), 1);
    }

    /// 每個 `expected.json` 都能讀進 `Output` 再寫出且 `Value` 相等——輸出結構的鍵齊全、
    /// 沒有多餘或缺漏、省略規則正確（含 `holidays`／動態桌布三鍵缺席的情境）。
    #[test]
    fn every_expected_json_round_trips_through_output() {
        use crate::fetch::output::Output;
        for d in scenario_dirs() {
            let s = Scenario::load(&d).unwrap();
            let want = s.expected().unwrap();
            let out: Output = serde_json::from_value(want.clone()).unwrap_or_else(|e| {
                panic!("{} 的 expected.json 無法讀成 Output：{e}", d.display())
            });
            let got = serde_json::to_value(&out).unwrap();
            assert_eq!(got, want, "{} 讀入再寫出不相等", d.display());
        }
    }
}
