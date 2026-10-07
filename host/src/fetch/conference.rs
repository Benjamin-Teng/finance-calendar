//! 法說會／財報（MOPS 法人說明會一覽表）與重大訊息備援（behavior-inventory 4.5～4.6／B-CONF-1～10、B-NEWS-1～5、
//! B-DATE-4、K-5、K-7、D-4；原始碼 `update_tw_events.py` 的 `mops_rows`／`quarter_of`／`fetch_conference`／
//! `fetch_conference_news`，`:458-589`，與 `main()` 的退回鏈 `:1181-1189`）。
//!
//! # 流程（B-CONF-10，一個來源＝一組 conference＋earnings，B-FLOW-6）
//!
//! 1. `top100` 為空 → `errors` 多一筆 [`MSG_NO_TOP100`]，MOPS 視為失敗（**不發請求**，D-4）。
//! 2. 否則抓 MOPS（本月＋下月，任一失敗＝整體失敗，**不保留第一個月**，K-7）。失敗記
//!    `法說會來源（MOPS）失敗：{e}`。
//! 3. MOPS 失敗 → 重大訊息備援 [`fetch_conference_news`]（只有當日公告、只產 `conference`、不篩前百大）。備援成功時
//!    **`earnings` 型別事件整批消失**，且 MOPS 的失敗訊息仍留在 `errors`（Python 的已知怪行為，design D8 末段：照 Python）。
//!    備援也失敗 → 記 `法說會備援來源（重大訊息）失敗：{e}`、沿用舊輸出的 `conference` 後接 `earnings`。
//! 4. `fresh`＝最終值非 `None`（含備援成功，B-FLOW-7）。
//!
//! 兩個階段（MOPS、備援）各自以 [`guarded`] 隔離 panic（D7）：階段 panic＝該階段失敗（記
//! `{前綴}panic: …`）、照常往下一階段退回；最外層 [`run_guarded`] 再包一層最後防線（panic＝沿用舊事件）。
//!
//! # 解析（B-CONF-4～8）：逐字照抄 Python 的 regex
//!
//! 見各 `RE_*` 常數旁的對照。Python `re` 與 Rust `regex` 的語意差異（以 Python 3.14／Unicode 16 實測，見測試與報告）：
//!
//! - `\d`：兩邊都是 Unicode `Nd`（K-5：不可改成 ASCII）。
//! - `\s`：Python 多認 U+001C–U+001F 四個控制字元（`str.isspace()`），Rust 的 `\s`（White_Space）沒有；
//!   凡 Python 原文有 `\s` 的地方，這裡寫成 `[\s\x1c-\x1f]`。
//! - `\b`／`\w`：Python 的 `\w` ＝ `isalnum() or '_'` ＝ Unicode `\p{L}\p{N}_`（逐碼位窮舉實測完全相同）；Rust 的 `\w`
//!   還多收 `\p{M}`（組合記號）、`\p{Pc}`（連接標點，如 `‿`）與 Join_Control，`1Q26` 後面緊接一個組合記號時兩邊的
//!   `\b` 判斷會不同。所以 `RE_Q_EN` 的兩個 `\b` 改成「前一個字元不是 `[\p{L}\p{N}_]`（或在開頭）」與「後一個字元不是
//!   `[\p{L}\p{N}_]`（或在結尾）」，見 [`RE_Q_EN`]。Rust `regex` 沒有 lookaround，所以邊界字元會被一起吃進比對，
//!   只取第一個命中（`quarter_of` 只要第一個），不影響結果。
//! - `re.S|re.I` ＝ `(?si)`。`re.I` 下本檔用到的字母（`tr`、`td`、`th`、`Q`）在兩邊的 Unicode 大小寫摺疊都只有
//!   `x ↔ X`。
//!
//! # 與 Python 的差異
//!
//! 沒有刻意偏離。`html.unescape` 由 [`super::html_text::unescape`] 重現（具名表為由 Python `html.entities.html5` 產生的 `super::html_entities`）。已知近似：
//! `str()` 對巢狀陣列／物件欄位用 JSON 文字而非 Python repr（真實來源恆為字串；備援的 `符合條款`、`主旨`、`說明`
//! 只用到「是否含某子字串」）。

use std::sync::LazyLock;

use regex::Regex;

use super::clock::Clock;
use super::dates::{iso_date, roc_to_date};
use super::events::previous_events;
use super::html_text::unescape;
use super::http::{get_json, post_form_ok, Fetch};
use super::json_util::{py_dict, py_iter, py_str, py_str_get, py_strip, roc_to_date_value, truthy};
use super::outcome::{guarded, SourceOutcome};
use super::output::{Event, EventType, PreviousOutput};

/// B-CONF-2：MOPS 法人說明會一覽表（舊站 mopsov 才吃）。
pub const URL_MOPS: &str = "https://mopsov.twse.com.tw/mops/web/ajax_t100sb02_1";
/// B-NEWS-1：重大訊息（只有當日公告）。
pub const URL_NEWS: &str = "https://openapi.twse.com.tw/v1/opendata/t187ap04_L";
/// B-FLOW-9：MOPS 失敗前綴。
pub const ERR_PREFIX_MOPS: &str = "法說會來源（MOPS）失敗：";
/// B-FLOW-9：備援失敗前綴。
pub const ERR_PREFIX_NEWS: &str = "法說會備援來源（重大訊息）失敗：";
/// B-CONF-1／D-4：前百大名單為空時多記的一筆（逐字）。
pub const MSG_NO_TOP100: &str = "法說會：無市值前百大名單可篩，本輪改用備援來源";

/// `RE_TR = re.compile(r"<tr[^>]*>(.*?)</tr>", re.S | re.I)`
const RE_TR: &str = r"(?si)<tr[^>]*>(.*?)</tr>";
/// `RE_TD = re.compile(r"<t[dh][^>]*>(.*?)</t[dh]>", re.S | re.I)`
const RE_TD: &str = r"(?si)<t[dh][^>]*>(.*?)</t[dh]>";
/// `RE_TAG = re.compile(r"<[^>]+>")`（無旗標）
const RE_TAG: &str = r"<[^>]+>";
/// `RE_CODE = re.compile(r"^\d{4}$")`
const RE_CODE: &str = r"^\d{4}$";
/// `RE_Q_ZH = re.compile(r"第\s*([一二三四1-4])\s*季")`（無旗標；`\s` 見模組文件）
const RE_Q_ZH: &str = r"第[\s\x1c-\x1f]*([一二三四1-4])[\s\x1c-\x1f]*季";
/// `RE_Q_EN = re.compile(r"\b([1-4])Q\d{2}\b|\bQ([1-4])\b", re.I)`；`\b` 換成明確的邊界字元（見模組文件）。
/// 兩個分支的第一個字元不同（數字／`Q`），所以「最左邊的命中」與 Python 的 `search` 一致。
const RE_Q_EN: &str =
    r"(?:^|[^\p{L}\p{N}_])(?:([1-4])(?i:q)\d{2}|(?i:q)([1-4]))(?:[^\p{L}\p{N}_]|$)";
/// B-NEWS-3：`re.search(r"第\s*12\s*款", clause)`
const RE_CLAUSE_12: &str = r"第[\s\x1c-\x1f]*12[\s\x1c-\x1f]*款";
/// B-NEWS-4：`re.search(r"召開法人說明會之日期[：:]\s*(\d{2,3}/\d{1,2}/\d{1,2})", body)`
const RE_NEWS_DATE: &str = r"召開法人說明會之日期[：:][\s\x1c-\x1f]*(\d{2,3}/\d{1,2}/\d{1,2})";

fn compile(pattern: &str) -> Regex {
    Regex::new(pattern).expect("固定的 regex 必能編譯")
}

static RX_TR: LazyLock<Regex> = LazyLock::new(|| compile(RE_TR));
static RX_TD: LazyLock<Regex> = LazyLock::new(|| compile(RE_TD));
static RX_TAG: LazyLock<Regex> = LazyLock::new(|| compile(RE_TAG));
static RX_CODE: LazyLock<Regex> = LazyLock::new(|| compile(RE_CODE));
static RX_Q_ZH: LazyLock<Regex> = LazyLock::new(|| compile(RE_Q_ZH));
static RX_Q_EN: LazyLock<Regex> = LazyLock::new(|| compile(RE_Q_EN));
static RX_CLAUSE_12: LazyLock<Regex> = LazyLock::new(|| compile(RE_CLAUSE_12));
static RX_NEWS_DATE: LazyLock<Regex> = LazyLock::new(|| compile(RE_NEWS_DATE));

/// 舊輸出中 `type` 為 `conference` 的事件，後接 `earnings` 的事件（B-FLOW-6，`_old_events_of("conference") +
/// _old_events_of("earnings")`）。
pub fn previous(prev: &PreviousOutput) -> Vec<Event> {
    previous_events(prev, &[EventType::Conference, EventType::Earnings])
}

/// 由組裝端呼叫：[`run`] 加上 panic 隔離（D7）。`top100` 是 3.2 [`super::top100`] 交出的名單
/// （`Top100::codes`，失敗時為沿用的舊名單；可為空）。
pub async fn run_guarded<F: Fetch>(
    fetch: &F,
    clock: &Clock,
    prev: &PreviousOutput,
    top100: &[String],
) -> SourceOutcome<Vec<Event>> {
    guarded(
        ERR_PREFIX_MOPS,
        previous(prev),
        run(fetch, clock, prev, top100),
    )
    .await
}

/// 退回鏈（B-CONF-10）。
async fn run<F: Fetch>(
    fetch: &F,
    clock: &Clock,
    prev: &PreviousOutput,
    top100: &[String],
) -> SourceOutcome<Vec<Event>> {
    let mut errors: Vec<String> = Vec::new();
    let mut logs: Vec<String> = Vec::new();

    // 階段一：MOPS。
    let mut conf: Option<Vec<Event>> = if top100.is_empty() {
        errors.push(MSG_NO_TOP100.to_string());
        None
    } else {
        let stage = guarded(ERR_PREFIX_MOPS, None, mops_stage(fetch, clock, top100)).await;
        errors.extend(stage.errors);
        logs.extend(stage.logs);
        stage.value
    };

    // 階段二：重大訊息備援。
    if conf.is_none() {
        logs.push("[法說會] MOPS 來源失敗，改用重大訊息備援".to_string());
        let stage = guarded(ERR_PREFIX_NEWS, None, news_stage(fetch)).await;
        errors.extend(stage.errors);
        logs.extend(stage.logs);
        conf = stage.value;
    }

    // 階段三：沿用舊資料。
    let fresh = conf.is_some();
    let value = conf.unwrap_or_else(|| {
        logs.push("[法說會] 本輪失敗，沿用上次資料".to_string());
        previous(prev)
    });
    SourceOutcome {
        value,
        errors,
        fresh,
        logs,
    }
}

/// `fetch_conference`（`top100` 非空）；失敗記 `法說會來源（MOPS）失敗：{e}` 並回 `None`。
async fn mops_stage<F: Fetch>(
    fetch: &F,
    clock: &Clock,
    top100: &[String],
) -> SourceOutcome<Option<Vec<Event>>> {
    match fetch_conference(fetch, clock, top100).await {
        Ok(ev) => SourceOutcome::new(Some(ev), true),
        Err(e) => {
            let mut out = SourceOutcome::new(None, false);
            out.errors.push(format!("{ERR_PREFIX_MOPS}{e}"));
            out
        }
    }
}

/// `fetch_conference_news`；失敗記 `法說會備援來源（重大訊息）失敗：{e}` 並回 `None`。
async fn news_stage<F: Fetch>(fetch: &F) -> SourceOutcome<Option<Vec<Event>>> {
    match fetch_conference_news(fetch).await {
        Ok(ev) => SourceOutcome::new(Some(ev), true),
        Err(e) => {
            let mut out = SourceOutcome::new(None, false);
            out.errors.push(format!("{ERR_PREFIX_NEWS}{e}"));
            out
        }
    }
}

/// B-CONF-2：本月（k=0）與下月（k=1）的 `(民國年, 月)`，跨年進位。對應 Python：
/// `mo = today.month + k; y, mo = today.year + (mo - 1) // 12, (mo - 1) % 12 + 1`。
fn month_params(year: i32, month: u8, k: i32) -> (i32, i32) {
    let mo = i32::from(month) + k;
    (
        year + (mo - 1).div_euclid(12) - 1911,
        (mo - 1).rem_euclid(12) + 1,
    )
}

/// `fetch_conference`（B-CONF-2～9）。`Err`＝來源失敗（連線／HTTP／`html.unescape` 的 `ValueError`）。
async fn fetch_conference<F: Fetch>(
    fetch: &F,
    clock: &Clock,
    top100: &[String],
) -> Result<Vec<Event>, String> {
    let today = clock.tpe_date();
    let mut ev = Vec::new();
    for k in 0..2 {
        let (y, mo) = month_params(today.year(), u8::from(today.month()), k);
        let bytes = post_form_ok(
            fetch,
            URL_MOPS,
            [
                ("encodeURIComponent", "1".to_string()),
                ("step", "1".to_string()),
                ("firstin", "1".to_string()),
                ("off", "1".to_string()),
                ("TYPEK", "sii".to_string()),
                ("year", y.to_string()),
                ("month", format!("{mo:02}")),
            ],
        )
        .await
        .map_err(|e| e.to_string())?;
        // `.decode("utf-8", "replace")`：壞位元組換成 U+FFFD。
        let text = String::from_utf8_lossy(&bytes);
        for r in mops_rows(&text)? {
            let code = &r[0];
            if !top100.iter().any(|c| c == code) || r.len() < 6 {
                continue;
            }
            // 日期欄有單日「115/07/16」與區間「115/06/30 至 115/07/03」兩種格式，一律取起日。
            let start = r[2].split('至').next().unwrap_or_default();
            let Some(d) = roc_to_date(py_strip(start)) else {
                continue;
            };
            let (place, brief) = (&r[4], &r[5]);
            let q = quarter_of(brief);
            let mut note = match q {
                Some(q) => format!("Q{q} 財報"),
                None => "法說會".to_string(),
            };
            if place.contains("線上") || brief.contains("線上") {
                note.push_str("（線上）");
            }
            ev.push(Event {
                date: iso_date(d),
                kind: if q.is_some() {
                    EventType::Earnings
                } else {
                    EventType::Conference
                },
                code: code.clone(),
                name: r[1].clone(),
                note,
            });
        }
    }
    Ok(ev)
}

/// `mops_rows`：HTML → 每列的純文字欄位（去標籤 → `html.unescape` → `strip`）；只留代號欄恰為 4 位數字的列。
/// `Err`＝`html.unescape` 丟 `ValueError`。
fn mops_rows(text: &str) -> Result<Vec<Vec<String>>, String> {
    let mut rows = Vec::new();
    for tr in RX_TR.captures_iter(text) {
        let tr = tr.get(1).map_or("", |m| m.as_str());
        let mut cells = Vec::new();
        for td in RX_TD.captures_iter(tr) {
            let c = td.get(1).map_or("", |m| m.as_str());
            let stripped = RX_TAG.replace_all(c, "");
            cells.push(py_strip(&unescape(&stripped)?).to_string());
        }
        if cells.first().is_some_and(|c| RX_CODE.is_match(c)) {
            rows.push(cells);
        }
    }
    Ok(rows)
}

/// `quarter_of`：先 `RE_Q_ZH`（`ZH_NUM`：一/1→1、二/2→2、三/3→3、四/4→4），再 `RE_Q_EN`；判不出＝`None`。
pub fn quarter_of(s: &str) -> Option<u8> {
    if let Some(m) = RX_Q_ZH.captures(s).and_then(|c| c.get(1)) {
        return match m.as_str() {
            "一" | "1" => Some(1),
            "二" | "2" => Some(2),
            "三" | "3" => Some(3),
            "四" | "4" => Some(4),
            _ => None,
        };
    }
    let c = RX_Q_EN.captures(s)?;
    // `int(m.group(1) or m.group(2))`：兩個群組都是單一個 ASCII 數字 1–4。
    c.get(1)
        .or_else(|| c.get(2))
        .and_then(|m| m.as_str().parse().ok())
}

/// `fetch_conference_news`（B-NEWS-1～5）。`Err`＝來源失敗（連線／HTTP／壞 JSON／形狀不符）。
async fn fetch_conference_news<F: Fetch>(fetch: &F) -> Result<Vec<Event>, String> {
    let j = get_json(fetch, URL_NEWS).await.map_err(|e| e.to_string())?;
    let mut ev = Vec::new();
    for r in py_iter(j)? {
        let obj = py_dict(&r)?;
        let clause = py_str_get(obj, "符合條款");
        // `str(r.get("主旨 ") or r.get("主旨") or "")`：鍵名帶尾端空白的優先。
        let subj = match [obj.get("主旨 "), obj.get("主旨")]
            .into_iter()
            .flatten()
            .find(|v| truthy(v))
        {
            Some(v) => py_str(v),
            None => String::new(),
        };
        let body = py_str_get(obj, "說明");
        if !RX_CLAUSE_12.is_match(&clause) && !subj.contains("法人說明會") {
            continue;
        }
        if !subj.contains("法人說明會") && !body.contains("法人說明會") {
            continue;
        }
        let d = match RX_NEWS_DATE.captures(&body).and_then(|c| c.get(1)) {
            Some(m) => roc_to_date(m.as_str()),
            None => roc_to_date_value(obj.get("事實發生日")),
        };
        let Some(d) = d else {
            continue;
        };
        let mut note = "法說會".to_string();
        let head: String = body.chars().take(200).collect();
        if subj.contains("線上") || head.contains("線上") {
            note.push_str("（線上）");
        }
        ev.push(Event {
            date: iso_date(d),
            kind: EventType::Conference,
            code: py_strip(&py_str_get(obj, "公司代號")).to_string(),
            name: py_strip(&py_str_get(obj, "公司名稱")).to_string(),
            note,
        });
    }
    Ok(ev)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;
    use crate::fetch::errors::FetchError;
    use crate::fetch::http::{Method, Request, RequestKey, Response};
    use crate::fetch::oracle;
    use crate::fetch::test_util::{block_on, MapFetch, PanicFetch};
    use serde_json::{json, Value};
    use time::macros::datetime;

    // ───── 測試用的 Fetch：依「方法＋網址＋編碼後的表單」回應（MapFetch 不看表單，分不出兩個月）─────

    #[derive(Default)]
    struct RouteFetch {
        routes: HashMap<RequestKey, Result<Response, FetchError>>,
        requested: Mutex<Vec<Request>>,
    }

    fn mops_key(year: &str, month: &str) -> RequestKey {
        RequestKey {
            method: Method::Post,
            url: URL_MOPS.to_string(),
            form: Some(format!(
                "encodeURIComponent=1&step=1&firstin=1&off=1&TYPEK=sii&year={year}&month={month}"
            )),
        }
    }

    impl RouteFetch {
        fn route(mut self, key: RequestKey, r: Result<Response, FetchError>) -> Self {
            self.routes.insert(key, r);
            self
        }
        /// 某個月的 MOPS 回應（200、本體位元組）。
        fn mops(self, year: &str, month: &str, body: &[u8]) -> Self {
            let r = Ok(Response {
                status: 200,
                body: body.to_vec(),
            });
            self.route(mops_key(year, month), r)
        }
        fn mops_status(self, year: &str, month: &str, status: u16) -> Self {
            let r = Ok(Response {
                status,
                body: Vec::new(),
            });
            self.route(mops_key(year, month), r)
        }
        fn news(self, body: &str) -> Self {
            let r = Ok(Response {
                status: 200,
                body: body.as_bytes().to_vec(),
            });
            self.route(
                RequestKey {
                    method: Method::Get,
                    url: URL_NEWS.to_string(),
                    form: None,
                },
                r,
            )
        }
        fn news_status(self, status: u16) -> Self {
            let r = Ok(Response {
                status,
                body: Vec::new(),
            });
            self.route(
                RequestKey {
                    method: Method::Get,
                    url: URL_NEWS.to_string(),
                    form: None,
                },
                r,
            )
        }
        fn requests(&self) -> Vec<Request> {
            self.requested
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }
        fn keys(&self) -> Vec<RequestKey> {
            self.requests().iter().map(Request::key).collect()
        }
    }

    impl Fetch for RouteFetch {
        async fn fetch(&self, req: &Request) -> Result<Response, FetchError> {
            self.requested
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(req.clone());
            match self.routes.get(&req.key()) {
                Some(r) => r.clone(),
                None => Err(FetchError::Network(format!(
                    "RouteFetch 未登記：{:?}",
                    req.key()
                ))),
            }
        }
    }

    /// 只有 POST 會 panic（MOPS 階段），GET 照常（重大訊息備援階段）。
    struct PostPanicFetch(RouteFetch);

    impl Fetch for PostPanicFetch {
        async fn fetch(&self, req: &Request) -> Result<Response, FetchError> {
            if req.method == Method::Post {
                panic!("注入的 POST panic");
            }
            self.0.fetch(req).await
        }
    }

    fn clock() -> Clock {
        Clock::new(
            datetime!(2026-10-05 01:12:14 +8),
            datetime!(2026-10-05 01:12:14),
        )
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn tr(cells: &[&str]) -> String {
        let tds: String = cells.iter().map(|c| format!("<td>{c}</td>")).collect();
        format!("<tr>{tds}</tr>")
    }

    fn table(rows: &[Vec<&str>]) -> String {
        let trs: String = rows.iter().map(|r| tr(r)).collect();
        format!("<table><tr><th>公司代號</th></tr>{trs}</table>")
    }

    fn row8<'a>(
        code: &'a str,
        name: &'a str,
        date: &'a str,
        place: &'a str,
        brief: &'a str,
    ) -> Vec<&'a str> {
        vec![code, name, date, "14:00", place, brief, "x", "y"]
    }

    fn ev(date: &str, kind: EventType, code: &str, name: &str, note: &str) -> Event {
        Event {
            date: date.into(),
            kind,
            code: code.into(),
            name: name.into(),
            note: note.into(),
        }
    }

    fn kind_str(k: EventType) -> &'static str {
        match k {
            EventType::Conference => "conference",
            EventType::Dividend => "dividend",
            EventType::Earnings => "earnings",
            EventType::Meeting => "meeting",
        }
    }

    fn run_with<F: Fetch>(
        f: &F,
        prev: &PreviousOutput,
        top100: &[String],
    ) -> SourceOutcome<Vec<Event>> {
        block_on(run(f, &clock(), prev, top100))
    }

    fn old_events() -> PreviousOutput {
        PreviousOutput::from_value(json!({"events": [
            {"date":"2026-10-08","type":"earnings","code":"2330","name":"台積電","note":"Q3 財報"},
            {"date":"2026-10-09","type":"conference","code":"2317","name":"鴻海","note":"法說會"},
            {"date":"2026-10-10","type":"dividend","code":"1101","name":"台泥","note":"除息"},
            {"date":"2026-10-11","type":"earnings","code":"2454","name":"聯發科","note":"Q3 財報"},
            {"date":"2026-10-12","type":"meeting","code":"2412","name":"中華電","note":"股東常會"},
            {"date":"2026-10-13","type":"conference","code":"2308","name":"台達電","note":"法說會"},
        ]}))
    }

    const NEWS_ONE: &str = r#"[{"符合條款":"第12款","主旨 ":"召開法人說明會","說明":"召開法人說明會之日期：115/10/15","事實發生日":"1151001","公司代號":"2330","公司名稱":"台積電"}]"#;

    /// 兩個月都回「沒有任何資料列」的表格。
    fn empty_months() -> RouteFetch {
        RouteFetch::default()
            .mops("115", "10", b"<table></table>")
            .mops("115", "11", b"<table></table>")
    }

    // ═════════ B-CONF-2：請求 ═════════

    #[test]
    fn b_conf_2_urls_prefixes_and_messages_are_verbatim() {
        assert_eq!(
            URL_MOPS,
            "https://mopsov.twse.com.tw/mops/web/ajax_t100sb02_1"
        );
        assert_eq!(
            URL_NEWS,
            "https://openapi.twse.com.tw/v1/opendata/t187ap04_L"
        );
        assert_eq!(ERR_PREFIX_MOPS, "法說會來源（MOPS）失敗：");
        assert_eq!(ERR_PREFIX_NEWS, "法說會備援來源（重大訊息）失敗：");
        assert_eq!(
            MSG_NO_TOP100,
            "法說會：無市值前百大名單可篩，本輪改用備援來源"
        );
    }

    #[test]
    fn b_conf_2_two_posts_this_month_then_next_with_python_form_order() {
        let f = empty_months();
        let out = run_with(&f, &PreviousOutput::default(), &s(&["2330"]));
        assert!(out.value.is_empty() && out.fresh && out.errors.is_empty() && out.logs.is_empty());
        // 表單鍵順序、民國年不補零、月份兩位（B-HTTP-6 的 `urlencode`），只有 TYPEK=sii（B-CONF-3：不抓上櫃）。
        assert_eq!(f.keys(), [mops_key("115", "10"), mops_key("115", "11")]);
        assert_eq!(
            f.keys()[0].form.as_deref(),
            Some("encodeURIComponent=1&step=1&firstin=1&off=1&TYPEK=sii&year=115&month=10")
        );
        assert!(f
            .keys()
            .iter()
            .all(|k| k.method == Method::Post && !k.form.as_deref().unwrap_or("").contains("otc")));
    }

    #[test]
    fn b_conf_2_month_arithmetic_carries_the_year_and_does_not_pad_the_roc_year() {
        // 對照 Python：`mo = m + k; y, mo = year + (mo - 1) // 12, (mo - 1) % 12 + 1`，再 `str(y - 1911)`、`f"{mo:02d}"`。
        for (year, month, k, want) in [
            (2026, 10, 0, (115, 10)),
            (2026, 10, 1, (115, 11)),
            (2026, 11, 1, (115, 12)),
            (2026, 12, 0, (115, 12)),
            (2026, 12, 1, (116, 1)),
            (2027, 1, 0, (116, 1)),
            (2027, 1, 1, (116, 2)),
            (2000, 12, 1, (90, 1)),
            (2000, 3, 0, (89, 3)),
            (2038, 12, 1, (128, 1)),
        ] {
            assert_eq!(month_params(year, month, k), want, "{year}-{month} k={k}");
        }
    }

    #[test]
    fn b_conf_2_december_requests_next_year_january_using_the_taipei_date() {
        // 台北 2026-12-31 23:30，本機已是 2027-01-01：必須用台北日期（B-DATE-2）。
        let c = Clock::new(
            datetime!(2026-12-31 23:30:00 +8),
            datetime!(2027-01-01 00:30:00),
        );
        let f = RouteFetch::default()
            .mops("115", "12", b"<table></table>")
            .mops("116", "01", b"<table></table>");
        let out = block_on(run(&f, &c, &PreviousOutput::default(), &s(&["2330"])));
        assert!(out.errors.is_empty() && out.fresh, "{:?}", out.errors);
        assert_eq!(f.keys(), [mops_key("115", "12"), mops_key("116", "01")]);
    }

    // ═════════ B-CONF-4：HTML 表格解析（對 Python `mops_rows` 的逐項對照）═════════

    #[test]
    fn b_conf_4_mops_rows_matches_python_on_a_table() {
        for (html, want) in rows_table() {
            assert_eq!(mops_rows(html), want, "輸入 {html:?}");
        }
    }

    #[allow(clippy::type_complexity)]
    fn rows_table() -> Vec<(&'static str, Result<Vec<Vec<String>>, String>)> {
        vec![
        ("<tr><td>2330</td><td>台積電</td><td>115/10/15</td><td>14:00</td><td>線上</td><td>第3季</td></tr>", Ok(vec![vec!["2330".to_string(), "台積電".to_string(), "115/10/15".to_string(), "14:00".to_string(), "線上".to_string(), "第3季".to_string()]])),
        ("<TR class='a' id=x>\n<TD nowrap>2330</TD>\n<Td>台積電\n</Td><tD>115/10/15</tD><TD></TD><TD>a</TD><TD>b</TD></Tr>", Ok(vec![vec!["2330".to_string(), "台積電".to_string(), "115/10/15".to_string(), "".to_string(), "a".to_string(), "b".to_string()]])),
        ("<tr><td>2330</td><td>A&nbsp;B&amp;C&#174;&lt;x&gt;&unknown;&amp;amp;&</td><td><b>1</b><br/>2</td></tr>", Ok(vec![vec!["2330".to_string(), "A\u{a0}B&C\u{ae}<x>&unknown;&amp;&".to_string(), "12".to_string()]])),
        ("<tr><th>2330</th><th>名稱</th><td>x</td></tr>", Ok(vec![vec!["2330".to_string(), "名稱".to_string(), "x".to_string()]])),
        ("<tr class='tblHead'><th>公司代號</th><th>公司名稱</th></tr><tr><td>23301</td><td>x</td></tr><tr><td> 2330 </td><td>y</td></tr><tr><td>\n2330\n</td></tr>", Ok(vec![vec!["2330".to_string(), "y".to_string()], vec!["2330".to_string()]])),
        ("<tr><td>\u{ff12}\u{ff13}\u{ff13}\u{ff10}</td><td>全形</td></tr>", Ok(vec![vec!["\u{ff12}\u{ff13}\u{ff13}\u{ff10}".to_string(), "全形".to_string()]])),
        ("<tr></tr><tr><td></td></tr><tr><td>1101</td></tr>", Ok(vec![vec!["1101".to_string()]])),
        ("<table><tr><td>2330</td><td><table><tr><td>x</td></tr></table></td></tr></table>", Ok(vec![vec!["2330".to_string(), "x".to_string()]])),
        ("<trfoo><td>2331</td></tr>", Ok(vec![vec!["2331".to_string()]])),
        ("<tr><thead>2330</thead></tr>", Ok(vec![])),
        ("<tr><td>2330</td><td>沒有結尾", Ok(vec![])),
        ("<tr><td>2330</td></tr", Ok(vec![])),
        ("<tR><Td>1101</tD></TR>", Ok(vec![vec!["1101".to_string()]])),
        ("<tr><td><!-- c -->2330<b>x</b></td><td>&#x41;&#65;&#X42;&nbsp&nbsp;</td></tr>", Ok(vec![])),
        ("<tr><td title=\"a>b\">2330</td></tr>", Ok(vec![])),
        ("<tr>\r\n<td>\r\n2330\r\n</td>\r\n<td>\u{3000}名稱\u{3000}</td>\r\n</tr>", Ok(vec![vec!["2330".to_string(), "名稱".to_string()]])),
        ("<tr><td>1234</td><td>&#9999999999;</td></tr>", Ok(vec![vec!["1234".to_string(), "\u{fffd}".to_string()]])),
        ("<tr><td>1234</td><td>a&lt;b&gt;c</td><td>&#28201;&#21843;</td></tr>", Ok(vec![vec!["1234".to_string(), "a<b>c".to_string(), "温啓".to_string()]])),
        ("<tr><td>1234</td><td>\u{1c}\u{1d} x \u{1e}\u{1f}</td></tr>", Ok(vec![vec!["1234".to_string(), "x".to_string()]])),
        ("<tr><td>1234</td><td>x\u{a0}y\u{a0}</td><td>\u{200b}z\u{200b}</td></tr>", Ok(vec![vec!["1234".to_string(), "x\u{a0}y".to_string(), "\u{200b}z\u{200b}".to_string()]])),
        ("no table at all", Ok(vec![])),
        ("", Ok(vec![])),
        ("<tr><td>1234</td></tr><tr><td>5678</td></tr><tr><td>abcd</td></tr>", Ok(vec![vec!["1234".to_string()], vec!["5678".to_string()]])),
        ]
    }

    #[test]
    fn b_conf_4_regex_sources_are_the_python_originals() {
        // 與 `update_tw_events.py:458-465` 逐字比對（`\s` 與 `\b` 的改寫見模組文件，另有測試證明等價）。
        assert_eq!(RE_TR, r"(?si)<tr[^>]*>(.*?)</tr>");
        assert_eq!(RE_TD, r"(?si)<t[dh][^>]*>(.*?)</t[dh]>");
        assert_eq!(RE_TAG, r"<[^>]+>");
        assert_eq!(RE_CODE, r"^\d{4}$");
        assert_eq!(
            RE_Q_ZH.replace(r"[\s\x1c-\x1f]", r"\s"),
            r"第\s*([一二三四1-4])\s*季"
        );
        assert_eq!(
            RE_CLAUSE_12.replace(r"[\s\x1c-\x1f]", r"\s"),
            r"第\s*12\s*款"
        );
        assert_eq!(
            RE_NEWS_DATE.replace(r"[\s\x1c-\x1f]", r"\s"),
            r"召開法人說明會之日期[：:]\s*(\d{2,3}/\d{1,2}/\d{1,2})"
        );
    }

    // ═════════ K-5：Unicode 語意（以 Python 3.14／Unicode 16 窮舉比對的指紋）═════════

    /// `(個數, 碼位總和, 碼位 XOR)`，與 Python 對同一集合算出的值相同。若將來 `regex-syntax` 升級到
    /// 新版 Unicode 而讓這個測試失敗：先用新版 Python 重算，確認 `\w`／`\s`／`\d` 與 `[\p{L}\p{N}_]`／
    /// `[\s\x1c-\x1f]`／`\d` 仍相等，再更新常數。重算指令（Python 端；`\w` 換成 `\s` 或 `\d` 即得另兩組）：
    ///
    /// ```text
    /// uv run --no-project python -c "import re; w=re.compile(r'\w'); n=s=x=0
    /// for i in range(0x110000):
    ///     if 0xd800<=i<=0xdfff: continue
    ///     if w.match(chr(i)): n+=1; s+=i; x^=i
    /// print(n,s,x)"
    /// ```
    fn fingerprint(pattern: &str) -> (u32, u64, u32) {
        let re = Regex::new(pattern).expect("regex");
        let (mut n, mut sum, mut x) = (0u32, 0u64, 0u32);
        let mut buf = [0u8; 4];
        for cp in 0..0x11_0000u32 {
            let Some(c) = char::from_u32(cp) else {
                continue;
            };
            if re.is_match(c.encode_utf8(&mut buf)) {
                n += 1;
                sum += u64::from(cp);
                x ^= cp;
            }
        }
        (n, sum, x)
    }

    #[test]
    fn k5_word_class_used_for_the_boundaries_equals_python_w() {
        // Python：`re.match(r'\w', chr(i))` 的集合（Unicode 16.0）＝ `isalnum() or '_'` ＝ L＋N＋底線。
        assert_eq!(
            fingerprint(r"^[\p{L}\p{N}_]$"),
            (142_940, 15_351_581_248, 57_284)
        );
    }

    #[test]
    fn k5_rust_native_w_and_s_differ_from_python_on_named_code_points() {
        // 不隨 Unicode 版本變的具名反例，說明為何不能用 Rust 原生 `\w`（`\b`）與 `\s`：
        let matches = |pat: &str, c: char| Regex::new(pat).unwrap().is_match(&c.to_string());
        // U+0301（Mn 組合記號）、U+203F（Pc 連接標點）、U+200D（Join_Control）：Rust `\w` 收，Python `\w` 不收
        // （`1Q26` 後接它們時 Python 的 `\b` 成立、命中）；`[\p{L}\p{N}_]` 與 Python 一致、都不收。
        for c in ['\u{301}', '\u{203f}', '\u{200d}'] {
            assert!(matches(r"^\w$", c), "Rust \\w 收 U+{:04X}", c as u32);
            assert!(!matches(r"^[\p{L}\p{N}_]$", c), "U+{:04X}", c as u32);
        }
        // U+00B2（上標 2，No）：Python `\w` 收（`isnumeric`），Rust `\w` 不收；`[\p{L}\p{N}_]` 收。
        assert!(!matches(r"^\w$", '\u{b2}'));
        assert!(matches(r"^[\p{L}\p{N}_]$", '\u{b2}'));
        // U+001C–001F：Python `\s` 收，Rust `\s` 不收；`[\s\x1c-\x1f]` 收。
        for c in '\u{1c}'..='\u{1f}' {
            assert!(!matches(r"^\s$", c), "Rust \\s 不收 U+{:04X}", c as u32);
            assert!(matches(r"^[\s\x1c-\x1f]$", c), "U+{:04X}", c as u32);
        }
    }

    #[test]
    fn k5_whitespace_class_equals_python_s_and_digits_equal_python_d() {
        // Python `\s`：U+0009–000D、U+001C–0020、U+0085、U+00A0、U+1680、U+2000–200A、U+2028、U+2029、U+202F、U+205F、U+3000。
        assert_eq!(fingerprint(r"^[\s\x1c-\x1f]$"), (29, 141_704, 1_782));
        // Python `\d`（str 模式）＝ Unicode Nd（760 個）；Rust `\d` 同。
        assert_eq!(fingerprint(r"^\d$"), (760, 39_891_450, 10));
    }

    #[test]
    fn k5_quarter_of_matches_python_on_a_table() {
        // 每列都是真的跑 `update_tw_events.quarter_of` 的結果（Python 3.14.6）。
        let table: &[(&str, Option<u8>)] = &[
            ("第2季法說會", Some(2)),
            ("第二季", Some(2)),
            ("第 三 季", Some(3)),
            ("第\u{3000}四\u{3000}季", Some(4)),
            ("第五季", None),
            ("第0季", None),
            ("第\u{ff11}季", None),
            ("第四季 2Q26", Some(4)),
            ("公布2Q26財報", None),
            ("公布 2Q26 財報", Some(2)),
            ("2Q26", Some(2)),
            ("Q2", Some(2)),
            ("q3 earnings", Some(3)),
            ("(Q4)", Some(4)),
            ("Q4財報", None),
            ("FQ2", None),
            ("Q2x", None),
            ("Q22", None),
            ("1Q2", None),
            ("1Q26", Some(1)),
            ("1q26", Some(1)),
            ("4Q2026", None),
            ("5Q26", None),
            ("Q5", None),
            ("\u{ff10}Q26", None),
            ("1Q\u{ff12}\u{ff16}", Some(1)),
            ("1Q26x", None),
            ("1Q26_", None),
            ("1Q26中", None),
            ("中1Q26", None),
            ("a 1Q26", Some(1)),
            ("1Q26 Q2", Some(1)),
            ("2Q26 2Q27", Some(2)),
            ("x2Q26 Q3", Some(3)),
            ("Q1Q2", None),
            ("1Q26\u{301}", Some(1)),
            ("1Q26\u{203f}", Some(1)),
            ("2Q26\u{b2}", None),
            ("第\u{1c}2\u{1d}季", Some(2)),
            ("第\u{a0}2\u{a0}季", Some(2)),
            ("第\u{200b}2季", None),
            ("第\n2\n季", Some(2)),
            ("券商邀約座談", None),
            ("", None),
            ("第2季\n", Some(2)),
            ("2Q26\n", Some(2)),
            ("Q2.", Some(2)),
            ("Q2-", Some(2)),
            ("Q2_", None),
            ("_Q2", None),
            ("\u{24c6}2 Q3", Some(3)),
            ("\u{ff11}Q\u{ff12}\u{ff16} 3Q27", Some(3)),
            ("Q\u{662}", None),
            ("第\u{662}季", None),
            ("財報 Q1\u{ff0c}線上", Some(1)),
            ("3Q26\u{ff09}", Some(3)),
            ("\u{ff08}4Q25", Some(4)),
            ("第一季和第二季", Some(1)),
            ("Q2第3季", Some(3)),
            ("第一 季", Some(1)),
            ("第 一季", Some(1)),
            ("第\u{3000}一\u{3000}季", Some(1)),
        ];
        for (input, want) in table {
            assert_eq!(quarter_of(input), *want, "輸入 {input:?}");
        }
    }

    #[test]
    fn b_conf_7_zh_numerals_and_digits_map_to_quarters() {
        for (txt, q) in [
            ("第一季", 1),
            ("第二季", 2),
            ("第三季", 3),
            ("第四季", 4),
            ("第1季", 1),
            ("第2季", 2),
            ("第3季", 3),
            ("第4季", 4),
        ] {
            assert_eq!(quarter_of(txt), Some(q), "{txt}");
        }
        // ZH 規則優先於 EN：兩種都在時取中文的（Python：`第2季 Q3` → 2、`Q3 第2季` → 2）。
        assert_eq!(quarter_of("Q3 第2季"), Some(2));
        assert_eq!(quarter_of("第2季 Q3"), Some(2));
    }

    // ═════════ B-CONF-5～8：Python 的 `fetch_conference` 逐列對照 ═════════

    #[allow(clippy::type_complexity)]
    fn mops_table() -> Vec<(
        &'static str,
        Result<
            &'static [(
                &'static str,
                &'static str,
                &'static str,
                &'static str,
                &'static str,
            )],
            &'static str,
        >,
    )> {
        vec![
        ("<table><tr><td>2330</td><td>台積電</td><td>115/10/15</td><td>14:00</td><td>台北</td><td>第3季法說會</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-15", "earnings", "2330", "台積電", "Q3 財報")])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/10/15 至 115/10/17</td><td>14:00</td><td>台北</td><td>第3季法說會</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-15", "earnings", "2330", "台積電", "Q3 財報")])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/10/15至115/10/17</td><td>14:00</td><td>台北</td><td>第3季法說會</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-15", "earnings", "2330", "台積電", "Q3 財報")])),
        ("<table><tr><td>2330</td><td>台積電</td><td> 115/10/15 至 </td><td>14:00</td><td>台北</td><td>第3季法說會</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-15", "earnings", "2330", "台積電", "Q3 財報")])),
        ("<table><tr><td>2330</td><td>台積電</td><td>至 115/10/17</td><td>14:00</td><td>台北</td><td>第3季法說會</td><td>x</td><td>y</td></tr></table>", Ok(&[])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/10/32</td><td>14:00</td><td>台北</td><td>第3季法說會</td><td>x</td><td>y</td></tr></table>", Ok(&[])),
        ("<table><tr><td>2330</td><td>台積電</td><td>1151015</td><td>14:00</td><td>台北</td><td>第3季法說會</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-15", "earnings", "2330", "台積電", "Q3 財報")])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115年10月15日</td><td>14:00</td><td>台北</td><td>第3季法說會</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-15", "earnings", "2330", "台積電", "Q3 財報")])),
        ("<table><tr><td>2330</td><td>台積電</td><td>\u{ff11}\u{ff11}\u{ff15}/10/15</td><td>14:00</td><td>台北</td><td>第3季法說會</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-15", "earnings", "2330", "台積電", "Q3 財報")])),
        ("<table><tr><td>2330</td><td>台積電</td><td>2026/10/15</td><td>14:00</td><td>台北</td><td>第3季法說會</td><td>x</td><td>y</td></tr></table>", Ok(&[])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/10/15\n至\n115/10/17</td><td>14:00</td><td>台北</td><td>第3季法說會</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-15", "earnings", "2330", "台積電", "Q3 財報")])),
        ("<table><tr><td>2330</td><td>台積電</td><td></td><td>14:00</td><td>台北</td><td>第3季法說會</td><td>x</td><td>y</td></tr></table>", Ok(&[])),
        ("<table><tr><td>2330</td><td>台積電</td><td>至</td><td>14:00</td><td>台北</td><td>第3季法說會</td><td>x</td><td>y</td></tr></table>", Ok(&[])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/10/15 ~ 115/10/17</td><td>14:00</td><td>台北</td><td>第3季法說會</td><td>x</td><td>y</td></tr></table>", Ok(&[])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/10/15 \u{ff0d} 115/10/17</td><td>14:00</td><td>台北</td><td>第3季法說會</td><td>x</td><td>y</td></tr></table>", Ok(&[])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115.10.15</td><td>14:00</td><td>台北</td><td>第3季法說會</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-15", "earnings", "2330", "台積電", "Q3 財報")])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115-10-15</td><td>14:00</td><td>台北</td><td>第3季法說會</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-15", "earnings", "2330", "台積電", "Q3 財報")])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/9/5</td><td>14:00</td><td>台北</td><td>第3季法說會</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-09-05", "earnings", "2330", "台積電", "Q3 財報")])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/12/31</td><td>14:00</td><td>台北</td><td>第3季法說會</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-12-31", "earnings", "2330", "台積電", "Q3 財報")])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/10/04</td><td>14:00</td><td>台北</td><td>第3季法說會</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-04", "earnings", "2330", "台積電", "Q3 財報")])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/10/19</td><td>14:00</td><td>台北</td><td>第3季法說會</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-19", "earnings", "2330", "台積電", "Q3 財報")])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/10/20</td><td>14:00</td><td>台北</td><td>第3季法說會</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-20", "earnings", "2330", "台積電", "Q3 財報")])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/10/15</td><td>14:00</td><td>線上</td><td>第3季</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-15", "earnings", "2330", "台積電", "Q3 財報\u{ff08}線上\u{ff09}")])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/10/15</td><td>14:00</td><td>台北</td><td>線上法說會第3季</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-15", "earnings", "2330", "台積電", "Q3 財報\u{ff08}線上\u{ff09}")])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/10/15</td><td>14:00</td><td>線上</td><td>線上</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-15", "conference", "2330", "台積電", "法說會\u{ff08}線上\u{ff09}")])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/10/15</td><td>14:00</td><td>台北</td><td>第二季</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-15", "earnings", "2330", "台積電", "Q2 財報")])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/10/15</td><td>14:00</td><td>台北</td><td>第 3 季</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-15", "earnings", "2330", "台積電", "Q3 財報")])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/10/15</td><td>14:00</td><td>台北</td><td>2Q26</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-15", "earnings", "2330", "台積電", "Q2 財報")])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/10/15</td><td>14:00</td><td>台北</td><td>Q4</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-15", "earnings", "2330", "台積電", "Q4 財報")])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/10/15</td><td>14:00</td><td>台北</td><td>公布2Q26財報</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-15", "conference", "2330", "台積電", "法說會")])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/10/15</td><td>14:00</td><td>台北</td><td>券商座談</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-15", "conference", "2330", "台積電", "法說會")])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/10/15</td><td>14:00</td><td>台北</td><td>第一季及第二季</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-15", "earnings", "2330", "台積電", "Q1 財報")])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/10/15</td><td>14:00</td><td>台北</td><td>第2季 Q3</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-15", "earnings", "2330", "台積電", "Q2 財報")])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/10/15</td><td>14:00</td><td>台北</td><td>Q3 第2季</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-15", "earnings", "2330", "台積電", "Q2 財報")])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/10/15</td><td>14:00</td><td>&#32218;&#19978;</td><td>x</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-15", "conference", "2330", "台積電", "法說會\u{ff08}線上\u{ff09}")])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/10/15</td><td>14:00</td><td>a</td><td>&#31532;&#20108;&#23395;</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-15", "earnings", "2330", "台積電", "Q2 財報")])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/10/15</td><td>14:00</td><td>a</td><td>&lt;第3季&gt;</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-15", "earnings", "2330", "台積電", "Q3 財報")])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/10/15</td><td>14:00</td><td>a</td><td><b>第</b>3季</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-15", "earnings", "2330", "台積電", "Q3 財報")])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/10/15</td><td>14:00</td><td>a</td><td>第<br>3<br>季</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-15", "earnings", "2330", "台積電", "Q3 財報")])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/10/15</td><td>14:00</td><td></td><td></td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-15", "conference", "2330", "台積電", "法說會")])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/10/15</td><td>14:00</td><td>a</td><td>第\u{3000}3\u{3000}季</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-15", "earnings", "2330", "台積電", "Q3 財報")])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/10/15</td><td>14:00</td><td>a</td><td>第\u{1c}3\u{1d}季</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-15", "earnings", "2330", "台積電", "Q3 財報")])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/10/15</td><td>14:00</td><td>a</td><td>第五季</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-15", "conference", "2330", "台積電", "法說會")])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/10/15</td><td>14:00</td><td>a</td><td>1q26</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-15", "earnings", "2330", "台積電", "Q1 財報")])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/10/15</td><td>14:00</td><td>a</td><td>2Q26中</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-15", "conference", "2330", "台積電", "法說會")])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/10/15</td><td>14:00</td><td>a</td><td>\u{ff08}4Q25\u{ff09}</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-15", "earnings", "2330", "台積電", "Q4 財報")])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/10/15</td><td>14:00</td><td>台北</td><td>第3季法說會</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-15", "earnings", "2330", "台積電", "Q3 財報")])),
        ("<table><tr><td>2454</td><td>台積電</td><td>115/10/15</td><td>14:00</td><td>台北</td><td>第3季法說會</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-15", "earnings", "2454", "台積電", "Q3 財報")])),
        ("<table><tr><td>9999</td><td>台積電</td><td>115/10/15</td><td>14:00</td><td>台北</td><td>第3季法說會</td><td>x</td><td>y</td></tr></table>", Ok(&[])),
        ("<table><tr><td>1101</td><td>台積電</td><td>115/10/15</td><td>14:00</td><td>台北</td><td>第3季法說會</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-15", "earnings", "1101", "台積電", "Q3 財報")])),
        ("<table><tr><td>233</td><td>台積電</td><td>115/10/15</td><td>14:00</td><td>台北</td><td>第3季法說會</td><td>x</td><td>y</td></tr></table>", Ok(&[])),
        ("<table><tr><td>23301</td><td>台積電</td><td>115/10/15</td><td>14:00</td><td>台北</td><td>第3季法說會</td><td>x</td><td>y</td></tr></table>", Ok(&[])),
        ("<table><tr><td> 2330 </td><td>台積電</td><td>115/10/15</td><td>14:00</td><td>台北</td><td>第3季法說會</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-15", "earnings", "2330", "台積電", "Q3 財報")])),
        ("<table><tr><td>\u{ff12}\u{ff13}\u{ff13}\u{ff10}</td><td>台積電</td><td>115/10/15</td><td>14:00</td><td>台北</td><td>第3季法說會</td><td>x</td><td>y</td></tr></table>", Ok(&[])),
        ("<table><tr><td>2330 台積電</td><td>台積電</td><td>115/10/15</td><td>14:00</td><td>台北</td><td>第3季法說會</td><td>x</td><td>y</td></tr></table>", Ok(&[])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/10/15</td><td>14:00</td><td>台北</td></tr></table>", Ok(&[])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/10/15</td><td>14:00</td><td>台北</td><td>第3季</td></tr></table>", Ok(&[("2026-10-15", "earnings", "2330", "台積電", "Q3 財報")])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/10/15</td><td>14:00</td><td>台北</td><td>第3季</td><td>a</td><td>b</td><td>c</td></tr></table>", Ok(&[("2026-10-15", "earnings", "2330", "台積電", "Q3 財報")])),
        ("<table><tr><td>2330</td><td>台積電</td><td>115/10/15</td><td>14:00</td><td>台北</td><td>第3季法說會</td><td>x</td><td>y</td></tr><tr><td>2454</td><td>台積電</td><td>115/10/16</td><td>14:00</td><td>台北</td><td>券商</td><td>x</td><td>y</td></tr><tr><td>2330</td><td>台積電</td><td>115/10/15</td><td>14:00</td><td>台北</td><td>第3季重複</td><td>x</td><td>y</td></tr><tr><td>1101</td><td>台積電</td><td>115/10/04</td><td>14:00</td><td>台北</td><td>第一季</td><td>x</td><td>y</td></tr></table>", Ok(&[("2026-10-15", "earnings", "2330", "台積電", "Q3 財報"), ("2026-10-16", "conference", "2454", "台積電", "法說會"), ("2026-10-15", "earnings", "2330", "台積電", "Q3 財報"), ("2026-10-04", "earnings", "1101", "台積電", "Q1 財報")])),
        ]
    }

    #[test]
    fn b_conf_5_to_8_fetch_conference_matches_python_row_by_row() {
        // 每列：本月的 HTML、Python 版實跑得到的事件（下月回空表格）。前百大＝2330、2454、2317、1101。
        let top = s(&["2330", "2454", "2317", "1101"]);
        for (html, want) in mops_table() {
            let f = RouteFetch::default()
                .mops("115", "10", html.as_bytes())
                .mops("115", "11", b"<table></table>");
            let got = block_on(fetch_conference(&f, &clock(), &top)).map(|ev| {
                ev.iter()
                    .map(|e| {
                        (
                            e.date.clone(),
                            kind_str(e.kind).to_string(),
                            e.code.clone(),
                            e.name.clone(),
                            e.note.clone(),
                        )
                    })
                    .collect::<Vec<_>>()
            });
            let want: Result<Vec<_>, String> = match want {
                Ok(rows) => Ok(rows
                    .iter()
                    .map(|(a, b, c, d, e)| {
                        (
                            a.to_string(),
                            b.to_string(),
                            c.to_string(),
                            d.to_string(),
                            e.to_string(),
                        )
                    })
                    .collect()),
                Err(m) => Err(m.to_string()),
            };
            assert_eq!(got, want, "輸入 {html:?}");
        }
    }

    #[test]
    fn b_conf_5_only_top100_codes_and_at_least_six_columns() {
        let html = table(&[
            row8("2330", "台積電", "115/10/15", "台北", "第3季"),
            row8("9999", "非前百大", "115/10/15", "台北", "第3季"),
            vec!["2454", "聯發科", "115/10/16", "14:00", "台北"], // 只有 5 欄
            vec!["2317", "鴻海", "115/10/17", "14:00", "台北", "券商座談"], // 剛好 6 欄
        ]);
        let f = RouteFetch::default()
            .mops("115", "10", html.as_bytes())
            .mops("115", "11", b"<table></table>");
        let out = run_with(
            &f,
            &PreviousOutput::default(),
            &s(&["2330", "2454", "2317"]),
        );
        assert_eq!(
            out.value,
            [
                ev(
                    "2026-10-15",
                    EventType::Earnings,
                    "2330",
                    "台積電",
                    "Q3 財報"
                ),
                ev(
                    "2026-10-17",
                    EventType::Conference,
                    "2317",
                    "鴻海",
                    "法說會"
                ),
            ]
        );
    }

    #[test]
    fn b_conf_6_range_dates_use_the_start_day() {
        let html = table(&[
            row8("2330", "a", "115/10/06 至 115/10/08", "線上", "第3季"),
            row8("2454", "b", "115/10/07", "台北", "第二季"),
            row8("2317", "c", "115/10/01至115/10/08", "台北", "x"),
            row8("1101", "d", " 115/10/14 \n 至 \n 115/10/15 ", "台北", "x"),
        ]);
        let f = RouteFetch::default()
            .mops("115", "10", html.as_bytes())
            .mops("115", "11", b"<table></table>");
        let out = run_with(
            &f,
            &PreviousOutput::default(),
            &s(&["2330", "2454", "2317", "1101"]),
        );
        let got: Vec<_> = out
            .value
            .iter()
            .map(|e| (e.date.as_str(), e.code.as_str(), e.note.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("2026-10-06", "2330", "Q3 財報（線上）"),
                ("2026-10-07", "2454", "Q2 財報"),
                ("2026-10-01", "2317", "法說會"), // 起日在窗口外也照出（窗口由組裝端套用）
                ("2026-10-14", "1101", "法說會"),
            ]
        );
    }

    // ═════════ 解碼：`decode("utf-8", "replace")` ═════════

    #[test]
    fn utf8_decode_replaces_bad_bytes_like_python() {
        // Python `bytes.decode("utf-8", "replace")` 的實測（見報告）：每個「最大無效子序列」一個 U+FFFD。
        let mut html: Vec<u8> = Vec::new();
        html.extend_from_slice("<table>".as_bytes());
        let cases: &[(&[u8], &str)] = &[
            (b"\xe3\x81", "\u{fffd}"),
            (b"\xf0\x9f\x98", "\u{fffd}"),
            (b"\xc0\xaf", "\u{fffd}\u{fffd}"),
            (b"\xed\xa0\x80", "\u{fffd}\u{fffd}\u{fffd}"),
            (b"\xf4\x90\x80\x80", "\u{fffd}\u{fffd}\u{fffd}\u{fffd}"),
            (b"\xe4\xb8\xad\xe6", "中\u{fffd}"),
            (b"\xef\xbb\xbf", "\u{feff}"),
            (b"\x80\x80", "\u{fffd}\u{fffd}"),
            (b"\xf0\x9f\x98\x80\xf0\x9f", "\u{1f600}\u{fffd}"),
        ];
        let codes = [
            "2330", "2454", "2317", "1101", "2412", "2308", "2882", "2881", "2891",
        ];
        for ((bytes, _), code) in cases.iter().zip(codes) {
            html.extend_from_slice(format!("<tr><td>{code}</td><td>名").as_bytes());
            html.extend_from_slice(bytes);
            html.extend_from_slice(
                "稱</td><td>115/10/15</td><td>14:00</td><td>台北</td><td>x</td></tr>".as_bytes(),
            );
        }
        html.extend_from_slice(b"</table>");
        let f =
            RouteFetch::default()
                .mops("115", "10", &html)
                .mops("115", "11", b"<table></table>");
        let top: Vec<String> = codes.iter().map(|c| c.to_string()).collect();
        let out = run_with(&f, &PreviousOutput::default(), &top);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        let names: Vec<_> = out.value.iter().map(|e| e.name.clone()).collect();
        let want: Vec<String> = cases.iter().map(|(_, r)| format!("名{r}稱")).collect();
        // 順序＝HTML 順序（不排序），逐一對上。
        assert_eq!(names, want);
    }

    // ═════════ B-CONF-9／K-7、B-CONF-10：失敗與退回鏈 ═════════

    fn mops_ok_table() -> String {
        table(&[
            row8("2330", "台積電", "115/10/15", "台北", "第3季"),
            row8("2454", "聯發科", "115/10/16", "台北", "x"),
        ])
    }

    #[test]
    fn b_conf_9_k7_a_failing_second_month_discards_the_first_month() {
        let f = RouteFetch::default()
            .mops("115", "10", mops_ok_table().as_bytes())
            .mops_status("115", "11", 500)
            .news(NEWS_ONE);
        let out = run_with(&f, &old_events(), &s(&["2330", "2454"]));
        // 本月的兩筆（財報、法說會）一筆都不在：整體失敗、改用備援，備援的內容才是最終值。
        assert_eq!(
            out.value,
            [ev(
                "2026-10-15",
                EventType::Conference,
                "2330",
                "台積電",
                "法說會"
            )]
        );
        assert_eq!(
            out.errors,
            ["法說會來源（MOPS）失敗：HTTP Error 500: Internal Server Error"]
        );
        assert!(out.fresh);
        assert_eq!(out.logs, ["[法說會] MOPS 來源失敗，改用重大訊息備援"]);
        assert_eq!(f.keys().len(), 3, "兩個月的 POST＋備援的 GET");
    }

    #[test]
    fn b_conf_9_a_failing_first_month_does_not_request_the_second() {
        let f = RouteFetch::default()
            .route(
                mops_key("115", "10"),
                Err(FetchError::Network("boom".into())),
            )
            .mops("115", "11", b"<table></table>")
            .news(NEWS_ONE);
        let out = run_with(&f, &PreviousOutput::default(), &s(&["2330"]));
        assert_eq!(out.errors, ["法說會來源（MOPS）失敗：boom"]);
        assert_eq!(
            f.keys(),
            [
                mops_key("115", "10"),
                RequestKey {
                    method: Method::Get,
                    url: URL_NEWS.to_string(),
                    form: None
                }
            ]
        );
    }

    #[test]
    fn b_conf_9_an_html_unescape_value_error_fails_the_whole_source_like_python() {
        // Python：`html.unescape("&#" + "9"*4301 + ";")` 丟 ValueError（int 位數上限），被 `except Exception` 接住。
        let html = format!(
            "<table><tr><td>2330</td><td>&#{};</td></tr></table>",
            "9".repeat(4301)
        );
        let f = RouteFetch::default()
            .mops("115", "10", html.as_bytes())
            .news(NEWS_ONE);
        let out = run_with(&f, &PreviousOutput::default(), &s(&["2330"]));
        assert_eq!(out.errors.len(), 1);
        assert!(
            out.errors[0].starts_with("法說會來源（MOPS）失敗：Exceeds the limit (4300 digits)"),
            "{:?}",
            out.errors
        );
        assert!(out.fresh, "備援成功");
    }

    #[test]
    fn b_conf_1_d4_empty_top100_skips_mops_adds_an_error_and_uses_the_news_fallback() {
        let f = RouteFetch::default().news(NEWS_ONE);
        let out = run_with(&f, &old_events(), &[]);
        assert_eq!(out.errors, [MSG_NO_TOP100]);
        assert_eq!(
            out.value,
            [ev(
                "2026-10-15",
                EventType::Conference,
                "2330",
                "台積電",
                "法說會"
            )]
        );
        assert!(out.fresh);
        assert_eq!(out.logs, ["[法說會] MOPS 來源失敗，改用重大訊息備援"]);
        assert_eq!(f.requests().len(), 1, "一個 MOPS 請求都不能有");
        assert_eq!(f.requests()[0].method, Method::Get);
    }

    #[test]
    fn b_conf_10_news_success_makes_the_earnings_events_disappear_and_keeps_the_mops_error() {
        // 已知怪行為（design D8 末段：照 Python）：舊輸出裡的 earnings 不會被沿用，錄製的 MOPS 財報也不見。
        let f = RouteFetch::default()
            .mops("115", "10", mops_ok_table().as_bytes())
            .mops_status("115", "11", 500)
            .news(NEWS_ONE);
        let out = run_with(&f, &old_events(), &s(&["2330"]));
        assert!(out.value.iter().all(|e| e.kind == EventType::Conference));
        assert_eq!(out.value.len(), 1);
        assert_eq!(
            crate::fetch::events::count_of(&out.value, EventType::Earnings),
            0
        );
        assert_eq!(out.errors.len(), 1, "MOPS 的失敗訊息仍在 errors");
        assert!(out.errors[0].starts_with(ERR_PREFIX_MOPS));
    }

    #[test]
    fn b_conf_10_news_success_with_zero_rows_is_still_fresh_and_does_not_reuse_old_events() {
        let f = empty_months_failing().news("[]");
        let out = run_with(&f, &old_events(), &s(&["2330"]));
        assert!(out.value.is_empty());
        assert!(out.fresh, "備援成功（含空清單）＝fresh（B-FLOW-7）");
        assert_eq!(out.errors.len(), 1);
    }

    fn empty_months_failing() -> RouteFetch {
        RouteFetch::default().mops_status("115", "10", 503)
    }

    #[test]
    fn b_conf_10_both_failing_reuses_old_conference_then_old_earnings() {
        let f = RouteFetch::default()
            .mops_status("115", "10", 500)
            .news_status(404);
        let out = run_with(&f, &old_events(), &s(&["2330"]));
        assert!(!out.fresh);
        // 串接順序＝先全部 conference、再全部 earnings（各自保持原順序），不是依日期交錯。
        assert_eq!(
            out.value,
            [
                ev(
                    "2026-10-09",
                    EventType::Conference,
                    "2317",
                    "鴻海",
                    "法說會"
                ),
                ev(
                    "2026-10-13",
                    EventType::Conference,
                    "2308",
                    "台達電",
                    "法說會"
                ),
                ev(
                    "2026-10-08",
                    EventType::Earnings,
                    "2330",
                    "台積電",
                    "Q3 財報"
                ),
                ev(
                    "2026-10-11",
                    EventType::Earnings,
                    "2454",
                    "聯發科",
                    "Q3 財報"
                ),
            ]
        );
        assert_eq!(
            out.errors,
            [
                "法說會來源（MOPS）失敗：HTTP Error 500: Internal Server Error",
                "法說會備援來源（重大訊息）失敗：HTTP Error 404: Not Found",
            ]
        );
        assert_eq!(
            out.logs,
            [
                "[法說會] MOPS 來源失敗，改用重大訊息備援",
                "[法說會] 本輪失敗，沿用上次資料"
            ]
        );
    }

    #[test]
    fn b_conf_10_empty_top100_and_failing_news_keeps_both_errors_in_python_order() {
        let f = RouteFetch::default().news_status(500);
        let out = run_with(&f, &PreviousOutput::default(), &[]);
        assert_eq!(
            out.errors,
            [
                MSG_NO_TOP100,
                "法說會備援來源（重大訊息）失敗：HTTP Error 500: Internal Server Error"
            ]
        );
        assert!(out.value.is_empty() && !out.fresh);
    }

    #[test]
    fn b_flow_6_previous_only_takes_conference_and_earnings_in_that_order() {
        let got = previous(&old_events());
        let kinds: Vec<_> = got.iter().map(|e| kind_str(e.kind)).collect();
        assert_eq!(kinds, ["conference", "conference", "earnings", "earnings"]);
        assert!(previous(&PreviousOutput::default()).is_empty());
    }

    #[test]
    fn b_flow_7_a_successful_empty_mops_is_fresh_and_never_touches_the_news_endpoint() {
        let f = empty_months();
        let out = run_with(&f, &old_events(), &s(&["2330"]));
        assert!(out.value.is_empty() && out.fresh && out.errors.is_empty());
        assert_eq!(f.requests().len(), 2);
    }

    // ═════════ B-NEWS：重大訊息備援（對 Python 的逐筆對照）═════════

    #[allow(clippy::type_complexity)]
    fn news_table() -> Vec<(
        &'static str,
        Result<&'static [(&'static str, &'static str, &'static str, &'static str)], &'static str>,
    )> {
        vec![
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/10/15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("2026-10-15", "2330", "台積電", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/10/15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb", "\u4e3b\u65e8": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703"}]"##,
                Ok(&[("2026-10-15", "2330", "台積電", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/10/15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb", "\u4e3b\u65e8": "x\u6cd5\u4eba\u8aaa\u660e\u6703x"}]"##,
                Ok(&[("2026-10-15", "2330", "台積電", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": 0, "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/10/15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb", "\u4e3b\u65e8": "x\u6cd5\u4eba\u8aaa\u660e\u6703x"}]"##,
                Ok(&[("2026-10-15", "2330", "台積電", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": null, "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/10/15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb", "\u4e3b\u65e8": null}]"##,
                Ok(&[("2026-10-15", "2330", "台積電", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": 123, "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/10/15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("2026-10-15", "2330", "台積電", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u7121\u95dc", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/10/15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("2026-10-15", "2330", "台積電", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u7121\u95dc", "\u8aaa\u660e": "\u7121\u95dc", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c 12 \u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/10/15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("2026-10-15", "2330", "台積電", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c\u001c12\u001d\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/10/15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("2026-10-15", "2330", "台積電", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c\u3000\u300012\u3000\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/10/15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("2026-10-15", "2330", "台積電", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c\uff11\uff12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/10/15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("2026-10-15", "2330", "台積電", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u689d", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/10/15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("2026-10-15", "2330", "台積電", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c122\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/10/15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("2026-10-15", "2330", "台積電", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\uff08\u7b2c12\u6b3e\uff09", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/10/15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("2026-10-15", "2330", "台積電", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c11\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/10/15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("2026-10-15", "2330", "台積電", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c11\u6b3e", "\u4e3b\u65e8 ": "\u5176\u4ed6", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/10/15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[]),
            ),
            (
                r##"[{"\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/10/15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("2026-10-15", "2330", "台積電", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": null, "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/10/15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("2026-10-15", "2330", "台積電", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f:115/10/15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("2026-10-15", "2330", "台積電", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a 115/10/15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("2026-10-15", "2330", "台積電", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a\u3000\n115/10/15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("2026-10-15", "2330", "台積電", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a\u001c115/10/15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("2026-10-15", "2330", "台積電", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a\uff11\uff11\uff15/\uff11\uff10/\uff11\uff15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("2026-10-15", "2330", "台積電", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/13/45", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/2/30", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a1151015", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("2026-10-01", "2330", "台積電", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/10/15/16", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("2026-10-15", "2330", "台積電", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a1115/10/15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("2026-10-01", "2330", "台積電", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a15/10/15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("1926-10-15", "2330", "台積電", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/1/5 \u8207 115/10/15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("2026-01-05", "2330", "台積電", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\uff0c\u65e5\u671f115/10/15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("2026-10-01", "2330", "台積電", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151016", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("2026-10-16", "2330", "台積電", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u4e8b\u5be6\u767c\u751f\u65e5": "115/10/16", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("2026-10-16", "2330", "台積電", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u4e8b\u5be6\u767c\u751f\u65e5": " 1151016 ", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("2026-10-16", "2330", "台積電", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u4e8b\u5be6\u767c\u751f\u65e5": "", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u4e8b\u5be6\u767c\u751f\u65e5": null, "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u4e8b\u5be6\u767c\u751f\u65e5": 1151016, "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("2026-10-16", "2330", "台積電", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u4e8b\u5be6\u767c\u751f\u65e5": 0, "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u4e8b\u5be6\u767c\u751f\u65e5": "20261016", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u4e8b\u5be6\u767c\u751f\u65e5": "abc", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u7dda\u4e0a\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/10/15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("2026-10-15", "2330", "台積電", "法說會\u{ff08}線上\u{ff09}")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/10/15 \u7dda\u4e0a", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("2026-10-15", "2330", "台積電", "法說會\u{ff08}線上\u{ff09}")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/10/15xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\u7dda\u4e0a", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("2026-10-15", "2330", "台積電", "法說會\u{ff08}線上\u{ff09}")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/10/15xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\u7dda\u4e0a", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("2026-10-15", "2330", "台積電", "法說會\u{ff08}線上\u{ff09}")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/10/15xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\u7dda\u4e0a", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("2026-10-15", "2330", "台積電", "法說會\u{ff08}線上\u{ff09}")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/10/15xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\u7dda\u4e0a", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("2026-10-15", "2330", "台積電", "法說會\u{ff08}線上\u{ff09}")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/10/15xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\u7dda\u4e0a", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("2026-10-15", "2330", "台積電", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u7dda\u4e0a\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/10/15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("2026-10-15", "2330", "台積電", "法說會\u{ff08}線上\u{ff09}")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/10/15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": " 2330 ", "\u516c\u53f8\u540d\u7a31": " \u53f0\u7a4d\u96fb "}]"##,
                Ok(&[("2026-10-15", "2330", "台積電", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/10/15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001"}]"##,
                Ok(&[("2026-10-15", "", "", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/10/15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": null, "\u516c\u53f8\u540d\u7a31": null}]"##,
                Ok(&[("2026-10-15", "None", "None", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/10/15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": 2330, "\u516c\u53f8\u540d\u7a31": 1}]"##,
                Ok(&[("2026-10-15", "2330", "1", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/10/15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "", "\u516c\u53f8\u540d\u7a31": ""}]"##,
                Ok(&[("2026-10-15", "", "", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": null, "\u4e8b\u5be6\u767c\u751f\u65e5": "1151016", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("2026-10-16", "2330", "台積電", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": 5, "\u4e8b\u5be6\u767c\u751f\u65e5": "1151016", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("2026-10-16", "2330", "台積電", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u6cd5\u4eba\u8aaa\u660e\u6703", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151016", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("2026-10-16", "2330", "台積電", "法說會")]),
            ),
            (r##"[{}]"##, Ok(&[])),
            (
                r##"[{"\u4e3b\u65e8": "\u6cd5\u4eba\u8aaa\u660e\u6703", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151016"}]"##,
                Ok(&[("2026-10-16", "", "", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/10/15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "9958", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[("2026-10-15", "9958", "台積電", "法說會")]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/10/15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}, {"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/10/16", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "1101", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}, {"\u7b26\u5408\u689d\u6b3e": "\u7b2c11\u6b3e", "\u4e3b\u65e8 ": "\u5176\u4ed6", "\u8aaa\u660e": "\u5176\u4ed6", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[
                    ("2026-10-15", "2330", "台積電", "法說會"),
                    ("2026-10-16", "1101", "台積電", "法說會"),
                ]),
            ),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/10/15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}, {"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/10/15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Ok(&[
                    ("2026-10-15", "2330", "台積電", "法說會"),
                    ("2026-10-15", "2330", "台積電", "法說會"),
                ]),
            ),
            (r##"[]"##, Ok(&[])),
            (r##"{}"##, Ok(&[])),
            (r##""""##, Ok(&[])),
            (r##"{"a": 1}"##, Err("'str' object has no attribute 'get'")),
            (r##""abc""##, Err("'str' object has no attribute 'get'")),
            (r##"null"##, Err("'NoneType' object is not iterable")),
            (r##"5"##, Err("'int' object is not iterable")),
            (r##"0"##, Err("'int' object is not iterable")),
            (r##"true"##, Err("'bool' object is not iterable")),
            (r##"[5]"##, Err("'int' object has no attribute 'get'")),
            (
                r##"[null]"##,
                Err("'NoneType' object has no attribute 'get'"),
            ),
            (r##"["x"]"##, Err("'str' object has no attribute 'get'")),
            (r##"[[1]]"##, Err("'list' object has no attribute 'get'")),
            (
                r##"[{"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/10/15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}, 5]"##,
                Err("'int' object has no attribute 'get'"),
            ),
            (
                r##"[5, {"\u7b26\u5408\u689d\u6b3e": "\u7b2c12\u6b3e", "\u4e3b\u65e8 ": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703", "\u8aaa\u660e": "\u53ec\u958b\u6cd5\u4eba\u8aaa\u660e\u6703\u4e4b\u65e5\u671f\uff1a115/10/15", "\u4e8b\u5be6\u767c\u751f\u65e5": "1151001", "\u516c\u53f8\u4ee3\u865f": "2330", "\u516c\u53f8\u540d\u7a31": "\u53f0\u7a4d\u96fb"}]"##,
                Err("'int' object has no attribute 'get'"),
            ),
        ]
    }

    #[test]
    fn b_news_1_to_5_fetch_conference_news_matches_python_case_by_case() {
        for (body, want) in news_table() {
            let f = MapFetch::new().ok(URL_NEWS, body);
            let got = block_on(fetch_conference_news(&f)).map(|ev| {
                assert!(ev.iter().all(|e| e.kind == EventType::Conference), "{body}");
                ev.iter()
                    .map(|e| {
                        (
                            e.date.clone(),
                            e.code.clone(),
                            e.name.clone(),
                            e.note.clone(),
                        )
                    })
                    .collect::<Vec<_>>()
            });
            let want: Result<Vec<_>, String> = match want {
                Ok(rows) => Ok(rows
                    .iter()
                    .map(|(a, b, c, d)| {
                        (a.to_string(), b.to_string(), c.to_string(), d.to_string())
                    })
                    .collect()),
                Err(m) => Err(m.to_string()),
            };
            assert_eq!(got, want, "輸入 {body}");
        }
    }

    #[test]
    fn b_news_1_the_fallback_requests_only_the_news_endpoint_and_does_not_filter_top100() {
        let f = MapFetch::new().ok(URL_NEWS, NEWS_ONE);
        let out = block_on(run(&f, &clock(), &PreviousOutput::default(), &[]));
        assert_eq!(f.requests(), [URL_NEWS]);
        // 備援不篩前百大：名單雖然是空的（或不含 2330），事件照收。
        assert_eq!(out.value.len(), 1);
        let out = block_on(run(
            &MapFetch::new().ok(URL_NEWS, NEWS_ONE),
            &clock(),
            &PreviousOutput::default(),
            &s(&["9999"]),
        ));
        // 名單非空 → 先走 MOPS（MapFetch 沒登記 MOPS，回連線失敗）→ 再走備援。
        assert_eq!(out.value.len(), 1);
        assert_eq!(out.errors.len(), 1);
        assert!(out.errors[0].starts_with(ERR_PREFIX_MOPS));
    }

    #[test]
    fn b_news_2_the_subject_key_with_a_trailing_space_wins_only_when_truthy() {
        let body = |a: &str, b: &str| {
            format!(
                r#"[{{"符合條款":"第11款","主旨 ":{a},"主旨":{b},"說明":"召開法人說明會之日期：115/10/15","公司代號":"1","公司名稱":"n"}}]"#
            )
        };
        let run_body = |a: &str, b: &str| {
            block_on(fetch_conference_news(
                &MapFetch::new().ok(URL_NEWS, &body(a, b)),
            ))
            .expect("成功")
        };
        assert_eq!(run_body(r#""法人說明會""#, r#""無關""#).len(), 1);
        assert_eq!(run_body(r#""""#, r#""法人說明會""#).len(), 1);
        assert_eq!(run_body("null", r#""法人說明會""#).len(), 1);
        assert_eq!(run_body("0", r#""法人說明會""#).len(), 1);
        assert_eq!(
            run_body(r#""無關""#, r#""法人說明會""#).len(),
            0,
            "有值的「主旨 」優先，不看「主旨」"
        );
        assert_eq!(run_body(r#""""#, r#""""#).len(), 0);
    }

    // ═════════ D7：panic 隔離 ═════════

    #[test]
    fn d7_a_panic_in_every_stage_keeps_the_old_conference_and_earnings_events() {
        let out = block_on(run_guarded(
            &PanicFetch,
            &clock(),
            &old_events(),
            &s(&["2330"]),
        ));
        assert_eq!(out.value, previous(&old_events()));
        assert_eq!(out.value.len(), 4);
        assert!(!out.fresh);
        // MOPS 階段與備援階段各記一筆 panic（階段各自隔離，備援照常嘗試）。
        assert_eq!(out.errors.len(), 2);
        assert!(
            out.errors[0].starts_with("法說會來源（MOPS）失敗：panic: "),
            "{:?}",
            out.errors
        );
        assert!(
            out.errors[1].starts_with("法說會備援來源（重大訊息）失敗：panic: "),
            "{:?}",
            out.errors
        );
        assert_eq!(
            crate::fetch::errors::normalize_error(&out.errors[0]),
            ERR_PREFIX_MOPS
        );
        assert_eq!(
            crate::fetch::errors::normalize_error(&out.errors[1]),
            ERR_PREFIX_NEWS
        );
    }

    #[test]
    fn d7_a_panic_in_the_mops_stage_still_lets_the_news_fallback_run() {
        let f = PostPanicFetch(RouteFetch::default().news(NEWS_ONE));
        let out = block_on(run_guarded(&f, &clock(), &old_events(), &s(&["2330"])));
        assert_eq!(
            out.value,
            [ev(
                "2026-10-15",
                EventType::Conference,
                "2330",
                "台積電",
                "法說會"
            )]
        );
        assert!(out.fresh);
        assert_eq!(out.errors.len(), 1);
        assert!(out.errors[0].starts_with("法說會來源（MOPS）失敗：panic: "));
    }

    #[test]
    fn d7_run_guarded_is_transparent_when_nothing_panics() {
        let f = RouteFetch::default()
            .mops("115", "10", mops_ok_table().as_bytes())
            .mops("115", "11", b"<table></table>");
        let out = block_on(run_guarded(
            &f,
            &clock(),
            &PreviousOutput::default(),
            &s(&["2330", "2454"]),
        ));
        assert_eq!(out.value.len(), 2);
        assert!(out.fresh && out.errors.is_empty() && out.logs.is_empty());
    }

    // ═════════ oracle：每個情境對 Python 的 expected.json ═════════

    fn conf_earn_expected(expected: &Value) -> Vec<Value> {
        let mut v = oracle::expected_events(expected, "conference");
        v.extend(oracle::expected_events(expected, "earnings"));
        v.sort_by(|a, b| {
            let key = |x: &Value| {
                (
                    x["date"].as_str().unwrap_or("").to_string(),
                    x["type"].as_str().unwrap_or("").to_string(),
                    x["code"].as_str().unwrap_or("").to_string(),
                )
            };
            key(a).cmp(&key(b))
        });
        v
    }

    #[test]
    fn oracle_every_scenario_matches_python_expected_json() {
        let mut saw_fallback = 0;
        oracle::for_each_scenario(|name, sc, expected| {
            // 前百大名單由 3.2 的來源算出（情境若有舊名單／快取也一併走），並與 Python 的輸出核對。
            let top = block_on(crate::fetch::top100::run_guarded(
                &sc.fetch,
                &sc.clock,
                &sc.previous(),
            ));
            let want_top: Vec<String> = expected["top100"]
                .as_array()
                .expect("top100")
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect();
            assert_eq!(top.value.codes, want_top, "情境 {name}：前百大名單不符");

            let out = block_on(run_guarded(
                &sc.fetch,
                &sc.clock,
                &sc.previous(),
                &top.value.codes,
            ));
            let (start, end) = crate::fetch::events::window(sc.clock.tpe_date());
            let merged = crate::fetch::events::merge_events(out.value.clone(), start, end);
            assert_eq!(
                serde_json::to_value(&merged).unwrap(),
                Value::Array(conf_earn_expected(expected)),
                "情境 {name}：conference／earnings 事件不符"
            );
            // errors：該來源的前綴子序列（含「無市值前百大名單」那一筆），順序與 Python 相同。
            let mine: Vec<String> = out
                .errors
                .iter()
                .map(|m| crate::fetch::errors::normalize_error(m))
                .collect();
            let want_err: Vec<String> = expected["errors"]
                .as_array()
                .expect("errors")
                .iter()
                .filter_map(Value::as_str)
                .filter(|m| {
                    m.starts_with(ERR_PREFIX_MOPS)
                        || m.starts_with(ERR_PREFIX_NEWS)
                        || *m == MSG_NO_TOP100
                })
                .map(crate::fetch::errors::normalize_error)
                .collect();
            assert_eq!(mine, want_err, "情境 {name}：errors 不符");
            if want_err
                .iter()
                .any(|m| m.starts_with(ERR_PREFIX_MOPS) || m == MSG_NO_TOP100)
            {
                saw_fallback += 1;
            }
        });
        assert!(
            saw_fallback >= 2,
            "備援情境應至少涵蓋 mops-month-fail 與 top100-empty 兩個"
        );
    }

    #[test]
    fn oracle_conference_range_date_picks_start_days_and_drops_out_of_window_ones() {
        let sc = crate::fetch::fixture::Scenario::named("conference-range-date").expect("載入情境");
        let top = block_on(crate::fetch::top100::run_guarded(
            &sc.fetch,
            &sc.clock,
            &sc.previous(),
        ));
        let out = block_on(run_guarded(
            &sc.fetch,
            &sc.clock,
            &sc.previous(),
            &top.value.codes,
        ));
        assert!(out.errors.is_empty() && out.fresh);
        let (start, end) = crate::fetch::events::window(sc.clock.tpe_date());
        let merged = crate::fetch::events::merge_events(out.value.clone(), start, end);
        let got: Vec<_> = merged
            .iter()
            .map(|e| {
                (
                    e.date.as_str(),
                    kind_str(e.kind),
                    e.code.as_str(),
                    e.note.as_str(),
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                ("2026-10-06", "earnings", "2330", "Q3 財報（線上）"), // 區間 10-06～10-08 取起日
                ("2026-10-07", "earnings", "2454", "Q2 財報"),
                ("2026-10-09", "conference", "2308", "法說會"),
                ("2026-10-14", "conference", "2412", "法說會"),
            ]
        );
        // 起日在窗口外的 2317（10-01）本模組照出、由 B-EVT 窗口剔除；9999、2882 各自被名單與欄數擋掉。
        assert!(out
            .value
            .iter()
            .any(|e| e.code == "2317" && e.date == "2026-10-01"));
        assert!(out
            .value
            .iter()
            .all(|e| e.code != "9999" && e.code != "2882"));
        assert_eq!(sc.fetch.request_count(URL_MOPS), 2);
    }

    #[test]
    fn oracle_mops_month_failure_scenario_uses_the_news_fallback_and_drops_earnings() {
        let sc = crate::fetch::fixture::Scenario::named("mops-month-fail-news-fallback")
            .expect("載入情境");
        let top = block_on(crate::fetch::top100::run_guarded(
            &sc.fetch,
            &sc.clock,
            &sc.previous(),
        ));
        assert!(!top.value.codes.is_empty());
        let out = block_on(run_guarded(
            &sc.fetch,
            &sc.clock,
            &sc.previous(),
            &top.value.codes,
        ));
        assert_eq!(
            out.errors,
            ["法說會來源（MOPS）失敗：HTTP Error 500: Internal Server Error"]
        );
        assert!(out.fresh);
        assert_eq!(
            crate::fetch::events::count_of(&out.value, EventType::Earnings),
            0
        );
        assert!(out.value.iter().any(|e| e.code == "9958"), "備援不篩前百大");
        assert_eq!(sc.fetch.request_count(URL_MOPS), 2, "本月成功、下月 500");
        assert_eq!(sc.fetch.request_count(URL_NEWS), 1);
    }

    #[test]
    fn oracle_empty_top100_scenario_goes_straight_to_the_fallback_without_a_mops_request() {
        let sc = crate::fetch::fixture::Scenario::named("top100-empty-conference-fallback")
            .expect("載入情境");
        let top = block_on(crate::fetch::top100::run_guarded(
            &sc.fetch,
            &sc.clock,
            &sc.previous(),
        ));
        assert!(top.value.codes.is_empty());
        let out = block_on(run_guarded(
            &sc.fetch,
            &sc.clock,
            &sc.previous(),
            &top.value.codes,
        ));
        assert_eq!(out.errors, [MSG_NO_TOP100]);
        assert!(out.fresh);
        assert_eq!(sc.fetch.request_count(URL_MOPS), 0);
        assert_eq!(sc.fetch.request_count(URL_NEWS), 1);
        let codes: Vec<_> = out
            .value
            .iter()
            .map(|e| (e.code.as_str(), e.date.as_str(), e.note.as_str()))
            .collect();
        assert_eq!(
            codes,
            [
                ("2330", "2026-10-15", "法說會"),
                ("2408", "2026-10-12", "法說會（線上）")
            ]
        );
    }
}
