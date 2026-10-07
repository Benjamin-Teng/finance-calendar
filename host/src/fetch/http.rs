//! HTTP 層（design.md D2、D3；behavior-inventory B-HTTP）。
//!
//! 所有來源只透過 [`Fetch`] 取資料：給定方法、網址與表單，回傳狀態碼與位元組。正式環境是
//! [`ReqwestFetch`]；測試用 `fixture::FixtureFetch`（`#[cfg(test)]`）從樣本目錄回放。
//!
//! # 錯誤分層（B-HTTP-3）
//!
//! - 連線失敗、逾時、TLS 失敗 → `Err(FetchError)`。
//! - HTTP 4xx／5xx **不是** `Err`：回傳 `Response { status, body }`，由呼叫端決定（對應 Python
//!   的 `HTTPError`，其本體也讀得到）。多數來源只在乎「2xx 或失敗」，用 [`get_ok`]／
//!   [`post_form_ok`]，非 2xx 轉成 `FetchError::Status(code)`。
//!
//! # 為什麼是泛型而不是 `dyn`
//!
//! 呼叫端一律 `F: Fetch`。trait 方法回傳的 future 宣告為 `Send`（`tauri::async_runtime::spawn`
//! 要求），所以 trait 本身要 `Sync`。

use std::collections::HashMap;
use std::future::Future;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::errors::FetchError;

/// B-HTTP-1：所有請求帶的固定 UA（逐字；Python 預設 UA 會被 Yahoo 擋）。
pub const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) finance-calendar/1.0";
/// B-HTTP-2：單一請求逾時（含連線與讀取），無重試、無退避。
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// D6：對同一主機的請求至少間隔（TWSE 社群流傳「5 秒 3 次」上限，未見官方數字）。
pub const MIN_HOST_GAP: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Method {
    Get,
    Post,
}

impl Method {
    #[cfg(test)]
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
        }
    }
}

/// 一個請求。`form` 保持插入順序（B-HTTP-6，對應 Python dict 的插入順序）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: Method,
    pub url: String,
    pub form: Option<Vec<(String, String)>>,
}

impl Request {
    pub fn get(url: impl Into<String>) -> Self {
        Request {
            method: Method::Get,
            url: url.into(),
            form: None,
        }
    }

    /// POST，body＝`application/x-www-form-urlencoded`（B-HTTP-6）。
    pub fn post_form<K, V>(url: impl Into<String>, form: impl IntoIterator<Item = (K, V)>) -> Self
    where
        K: Into<String>,
        V: Into<String>,
    {
        Request {
            method: Method::Post,
            url: url.into(),
            form: Some(
                form.into_iter()
                    .map(|(k, v)| (k.into(), v.into()))
                    .collect(),
            ),
        }
    }

    /// 回放對照用的鍵：`(方法, 網址, 編碼後的表單)`，與 `tests/fetch_oracle/harness.py`
    /// 的 `request_key` 同義（GET 的表單為 `None`；POST 為 [`urlencode`] 後的字串）。
    #[cfg(test)]
    pub fn key(&self) -> RequestKey {
        RequestKey {
            method: self.method,
            url: self.url.clone(),
            form: self.form.as_deref().map(urlencode),
        }
    }
}

/// [`Request::key`] 的結果。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg(test)]
pub struct RequestKey {
    pub method: Method,
    pub url: String,
    pub form: Option<String>,
}

/// 回應：狀態碼＋位元組（不做任何解碼；來源自己解析，B-HTTP-5）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Response {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// 取資料的唯一介面（D3）。
pub trait Fetch: Sync {
    fn fetch(&self, req: &Request) -> impl Future<Output = Result<Response, FetchError>> + Send;
}

/// GET 並要求 2xx；非 2xx 轉成 `FetchError::Status(code)`（對應 Python `http_bytes` 丟 `HTTPError`）。
pub async fn get_ok<F: Fetch>(fetch: &F, url: &str) -> Result<Vec<u8>, FetchError> {
    into_ok(fetch.fetch(&Request::get(url)).await?)
}

/// GET 並要求 2xx、再解析成 JSON（對應 Python `http_json`，B-HTTP-5）：嚴格 UTF-8、不容許 BOM。
pub async fn get_json<F: Fetch>(fetch: &F, url: &str) -> Result<serde_json::Value, FetchError> {
    parse_json(&get_ok(fetch, url).await?)
}

/// 位元組 → JSON（B-HTTP-5）；失敗為 [`FetchError::Decode`]。
///
/// 與 Python `json.loads` 的刻意差異（design.md D8-6）：`NaN`／`Infinity`／`-Infinity` 字面值**不接受**
/// （serde_json 寫不出非有限數，接受了也只會讓輸出變成非法 JSON）。
pub fn parse_json(bytes: &[u8]) -> Result<serde_json::Value, FetchError> {
    serde_json::from_slice(bytes).map_err(|e| FetchError::Decode(e.to_string()))
}

/// POST 表單並要求 2xx（MOPS 用）。
pub async fn post_form_ok<F, K, V>(
    fetch: &F,
    url: &str,
    form: impl IntoIterator<Item = (K, V)>,
) -> Result<Vec<u8>, FetchError>
where
    F: Fetch,
    K: Into<String>,
    V: Into<String>,
{
    into_ok(fetch.fetch(&Request::post_form(url, form)).await?)
}

fn into_ok(resp: Response) -> Result<Vec<u8>, FetchError> {
    if resp.is_success() {
        Ok(resp.body)
    } else {
        Err(FetchError::Status(resp.status))
    }
}

/// Python `urllib.parse.urlencode(pairs)` 的逐字等價（B-HTTP-6）：每個鍵與值各自
/// `quote_plus`——ASCII 英數與 `_.-~` 原樣，空白轉 `+`，其餘位元組（UTF-8）轉大寫 `%XX`；
/// 以 `=` 連鍵值、`&` 連各組。
///
/// 為什麼自寫而不用 reqwest 的 `.form()`：回放對照鍵與實際送出的 body 都要與 Python 相同；
/// serde_urlencoded 對 `~`、`*` 的處理與 Python 不同。
pub fn urlencode(pairs: &[(String, String)]) -> String {
    let mut out = String::new();
    for (i, (k, v)) in pairs.iter().enumerate() {
        if i > 0 {
            out.push('&');
        }
        quote_plus_into(&mut out, k);
        out.push('=');
        quote_plus_into(&mut out, v);
    }
    out
}

fn quote_plus_into(out: &mut String, s: &str) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => {
                out.push('%');
                out.push(HEX[(b >> 4) as usize] as char);
                out.push(HEX[(b & 0x0f) as usize] as char);
            }
        }
    }
}

/// 同主機請求間隔（D6）：以主機名為鍵記錄「上次請求完成的時刻」，下一個請求不足間隔就補
/// 睡到夠。純計算（時刻由呼叫端傳入），測試不必真的睡。
///
/// 來源本來就依序執行（B-FLOW-5），所以這裡不做「預約」；若將來有並行請求，同一主機可能
/// 同時看到「沒有紀錄」而不等待——屆時再改。
#[derive(Debug)]
pub struct HostThrottle {
    gap: Duration,
    last_done: Mutex<HashMap<String, Instant>>,
}

impl HostThrottle {
    pub fn new(gap: Duration) -> Self {
        HostThrottle {
            gap,
            last_done: Mutex::new(HashMap::new()),
        }
    }

    /// 在 `now` 對 `host` 發請求前，還需要等多久（沒有紀錄或已超過間隔＝0）。
    pub fn delay_for(&self, host: &str, now: Instant) -> Duration {
        let map = self.last_done.lock().unwrap_or_else(|e| e.into_inner());
        match map.get(host) {
            Some(&last) => self.gap.saturating_sub(now.saturating_duration_since(last)),
            None => Duration::ZERO,
        }
    }

    /// 記錄 `host` 的請求在 `now` 完成（成功或失敗都記，失敗的請求同樣算打過伺服器）。
    pub fn mark_done(&self, host: &str, now: Instant) {
        let mut map = self.last_done.lock().unwrap_or_else(|e| e.into_inner());
        map.insert(host.to_string(), now);
    }
}

/// 把 reqwest 錯誤分類：逾時 → `Timeout`，其餘 → `Network`（含錯誤鏈全文，TLS 驗證失敗也在內）。
fn classify(err: &reqwest::Error) -> FetchError {
    if err.is_timeout() {
        return FetchError::Timeout;
    }
    let mut msg = err.to_string();
    let mut src = std::error::Error::source(err);
    while let Some(s) = src {
        msg.push_str(": ");
        msg.push_str(&s.to_string());
        src = s.source();
    }
    FetchError::Network(msg)
}

/// 正式環境的 [`Fetch`]：reqwest（native-tls＝SChannel）。
///
/// - UA 固定（[`USER_AGENT`]）、單請求 30 秒逾時、跟隨轉址（reqwest 預設最多 10 次，同 urllib）。
/// - 系統代理：`system-proxy` feature 開啟時 `Client::builder()` 預設就會讀 Windows／環境代理
///   （reqwest 0.13.5 `auto_sys_proxy` 預設 true；只有呼叫 `.proxy()`／`.no_proxy()` 才關閉），
///   這裡不呼叫。
/// - **嚴格憑證驗證**：不呼叫任何 `danger_accept_invalid_*`（D2，不保留 Python 的 CERT_NONE 退回）。
/// - 不送 `Accept-Encoding`：reqwest 只在開了 gzip／brotli／zstd／deflate feature 時才送，
///   本專案一個都沒開（同 Python）。
/// - 同一主機兩次請求間隔 ≥ [`MIN_HOST_GAP`]。
#[derive(Debug)]
pub struct ReqwestFetch {
    client: reqwest::Client,
    throttle: HostThrottle,
}

impl ReqwestFetch {
    /// 設定共用的 builder：測試以 `.no_proxy()` 改成連 loopback 時，其餘設定與正式環境相同。
    fn builder() -> reqwest::ClientBuilder {
        reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(REQUEST_TIMEOUT)
    }

    /// 建立 client（初始化 SChannel；不連網）。
    pub fn new() -> Result<Self, FetchError> {
        Self::with_client(Self::builder(), MIN_HOST_GAP)
    }

    fn with_client(builder: reqwest::ClientBuilder, gap: Duration) -> Result<Self, FetchError> {
        let client = builder.build().map_err(|e| {
            FetchError::Network(format!("無法建立 HTTP client：{}", classify_text(&e)))
        })?;
        Ok(ReqwestFetch {
            client,
            throttle: HostThrottle::new(gap),
        })
    }

    async fn send(&self, url: reqwest::Url, req: &Request) -> Result<Response, FetchError> {
        let builder = match req.method {
            Method::Get => self.client.get(url),
            Method::Post => self
                .client
                .post(url)
                .header(
                    reqwest::header::CONTENT_TYPE,
                    "application/x-www-form-urlencoded",
                )
                .body(req.form.as_deref().map(urlencode).unwrap_or_default()),
        };
        let resp = builder.send().await.map_err(|e| classify(&e))?;
        let status = resp.status().as_u16();
        let body = resp.bytes().await.map_err(|e| classify(&e))?.to_vec();
        Ok(Response { status, body })
    }
}

fn classify_text(e: &reqwest::Error) -> String {
    classify(e).to_string()
}

impl Fetch for ReqwestFetch {
    async fn fetch(&self, req: &Request) -> Result<Response, FetchError> {
        let url = reqwest::Url::parse(&req.url)
            .map_err(|e| FetchError::Network(format!("網址不合法（{}）：{e}", req.url)))?;
        let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
        let wait = self.throttle.delay_for(&host, Instant::now());
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
        let result = self.send(url, req).await;
        self.throttle.mark_done(&host, Instant::now());
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// 期望值來自 `uv run --no-project python`：
    /// `urllib.parse.urlencode([...])`。
    #[test]
    fn urlencode_matches_python() {
        // urlencode([("encodeURIComponent","1"),("step","1"),("TYPEK","sii"),("year","115"),("month","10")])
        assert_eq!(
            urlencode(&pairs(&[
                ("encodeURIComponent", "1"),
                ("step", "1"),
                ("TYPEK", "sii"),
                ("year", "115"),
                ("month", "10"),
            ])),
            "encodeURIComponent=1&step=1&TYPEK=sii&year=115&month=10"
        );
        // urlencode([("a b","c&d=e"),("~*_.-","é中/?+%"),("","")])
        assert_eq!(
            urlencode(&pairs(&[("a b", "c&d=e"), ("~*_.-", "é中/?+%"), ("", "")])),
            "a+b=c%26d%3De&~%2A_.-=%C3%A9%E4%B8%AD%2F%3F%2B%25&="
        );
        assert_eq!(urlencode(&[]), "");
        // 順序即插入順序，不排序。
        assert_eq!(urlencode(&pairs(&[("z", "1"), ("a", "2")])), "z=1&a=2");
    }

    #[test]
    fn request_key_distinguishes_get_post_and_form_order() {
        let g = Request::get("https://x/y").key();
        assert_eq!(g.form, None);
        let p = Request::post_form("https://x/y", [("a", "1"), ("b", "2")]).key();
        assert_eq!(p.form.as_deref(), Some("a=1&b=2"));
        assert_ne!(g, p);
        let q = Request::post_form("https://x/y", [("b", "2"), ("a", "1")]).key();
        assert_ne!(p, q);
        // 空表單的 POST（Python `form=[]`）與 GET 不同。
        let empty = Request::post_form("https://x/y", Vec::<(String, String)>::new()).key();
        assert_eq!(empty.form.as_deref(), Some(""));
        assert_ne!(empty, g);
    }

    #[test]
    fn throttle_waits_only_the_remaining_gap_per_host() {
        let t = HostThrottle::new(Duration::from_secs(2));
        let t0 = Instant::now();
        // 沒有紀錄：不等。
        assert_eq!(t.delay_for("a.example", t0), Duration::ZERO);
        t.mark_done("a.example", t0);
        // 剛完成：等滿 2 秒；過了 0.5 秒：再等 1.5 秒；剛好 2 秒或更久：不等。
        assert_eq!(t.delay_for("a.example", t0), Duration::from_secs(2));
        assert_eq!(
            t.delay_for("a.example", t0 + Duration::from_millis(500)),
            Duration::from_millis(1500)
        );
        assert_eq!(
            t.delay_for("a.example", t0 + Duration::from_secs(2)),
            Duration::ZERO
        );
        assert_eq!(
            t.delay_for("a.example", t0 + Duration::from_secs(60)),
            Duration::ZERO
        );
        // 其他主機不受影響。
        assert_eq!(t.delay_for("b.example", t0), Duration::ZERO);
        // 以新的完成時刻重算。
        t.mark_done("a.example", t0 + Duration::from_secs(10));
        assert_eq!(
            t.delay_for("a.example", t0 + Duration::from_secs(11)),
            Duration::from_secs(1)
        );
        // 時鐘倒退（now 早於 last）不會 panic，視為剛完成。
        assert_eq!(t.delay_for("a.example", t0), Duration::from_secs(2));
    }

    #[test]
    fn constants_are_verbatim_from_the_inventory() {
        assert_eq!(
            USER_AGENT,
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) finance-calendar/1.0"
        );
        assert_eq!(REQUEST_TIMEOUT, Duration::from_secs(30));
        assert_eq!(MIN_HOST_GAP, Duration::from_secs(2));
    }

    #[test]
    fn parse_json_is_strict_like_python_json_loads() {
        assert_eq!(
            parse_json(br#"{"a":[1,"x"]}"#).unwrap(),
            serde_json::json!({"a": [1, "x"]})
        );
        // B-HTTP-5：BOM、壞 UTF-8、空本體、截斷都是失敗，且是 Decode（不是 Network）。
        for bad in [
            &[0xEF, 0xBB, 0xBF, b'{', b'}'][..],
            &[b'{', b'"', b'a', b'"', b':', b'"', 0xFF, b'"', b'}'][..],
            &b""[..],
            &br#"{"a":"#[..],
            // D8-6：Python 接受、這裡刻意拒絕。
            &br#"{"a":NaN}"#[..],
            &br#"[Infinity]"#[..],
            &br#"[-Infinity]"#[..],
        ] {
            assert!(
                matches!(parse_json(bad), Err(FetchError::Decode(_))),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn into_ok_maps_non_2xx_to_status_error() {
        let ok = Response {
            status: 204,
            body: b"x".to_vec(),
        };
        assert_eq!(into_ok(ok).unwrap(), b"x");
        let nf = Response {
            status: 404,
            body: b"nope".to_vec(),
        };
        assert_eq!(into_ok(nf), Err(FetchError::Status(404)));
        let redirect = Response {
            status: 302,
            body: vec![],
        };
        assert_eq!(into_ok(redirect), Err(FetchError::Status(302)));
    }

    #[test]
    fn reqwest_fetch_builds_without_network() {
        // 只初始化 SChannel 與 client，不發任何請求。
        assert!(ReqwestFetch::new().is_ok());
    }

    /// 一次性 loopback 伺服器：依序回應 `replies`（原始 HTTP 回應文字），並把每個收到的
    /// 原始請求（到空行為止＋Content-Length 指定的 body）送出 channel。只綁 127.0.0.1，
    /// 不碰任何外部主機。
    fn spawn_loopback(replies: Vec<String>) -> (u16, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for reply in replies {
                let (mut s, _) = match listener.accept() {
                    Ok(x) => x,
                    Err(_) => return,
                };
                let mut buf = Vec::new();
                let mut tmp = [0u8; 1024];
                let mut header_end = None;
                loop {
                    let n = s.read(&mut tmp).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if header_end.is_none() {
                        header_end = buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4);
                    }
                    if let Some(he) = header_end {
                        let head = String::from_utf8_lossy(&buf[..he]).to_ascii_lowercase();
                        let len = head
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .and_then(|v| v.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        if buf.len() >= he + len {
                            break;
                        }
                    }
                }
                let _ = tx.send(String::from_utf8_lossy(&buf).into_owned());
                let _ = s.write_all(reply.as_bytes());
                let _ = s.flush();
            }
        });
        (port, rx)
    }

    fn http(status: &str, headers: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",
            body.len()
        )
    }

    /// 用 loopback 驗證 `ReqwestFetch` 的線上行為：UA、不送 Accept-Encoding、POST 表單、
    /// 4xx 不是 Err、跟隨轉址、同主機間隔（用縮短的間隔，免得測試睡 2 秒）。
    #[test]
    fn reqwest_fetch_wire_behaviour_over_loopback() {
        let (port, rx) = spawn_loopback(vec![
            http("200 OK", "", "hello"),
            http("404 Not Found", "", "missing"),
            http("302 Found", "Location: /final\r\n", ""),
            http("200 OK", "", "landed"),
            http("200 OK", "", "posted"),
        ]);
        // `.no_proxy()`：loopback 不該繞系統代理；其餘設定（UA、逾時）與正式環境同一個 builder。
        let gap = Duration::from_millis(400);
        let fetch = ReqwestFetch::with_client(ReqwestFetch::builder().no_proxy(), gap).unwrap();
        let base = format!("http://127.0.0.1:{port}");

        crate::fetch::test_util::block_on(async {
            // 1. 200，檢查標頭。
            let r = fetch
                .fetch(&Request::get(format!("{base}/a")))
                .await
                .unwrap();
            assert_eq!((r.status, r.body.as_slice()), (200, b"hello".as_slice()));
            let raw = rx.recv().unwrap().to_ascii_lowercase();
            assert!(raw.starts_with("get /a http/1.1"), "{raw}");
            assert!(
                raw.contains(&format!(
                    "user-agent: {}\r\n",
                    USER_AGENT.to_ascii_lowercase()
                )),
                "{raw}"
            );
            assert!(
                !raw.contains("accept-encoding"),
                "不該送 Accept-Encoding：{raw}"
            );

            // 2. 404 不是 Err，本體讀得到；且與上一個請求（同主機）至少間隔 gap。
            let t = Instant::now();
            let r = fetch
                .fetch(&Request::get(format!("{base}/b")))
                .await
                .unwrap();
            assert_eq!((r.status, r.body.as_slice()), (404, b"missing".as_slice()));
            rx.recv().unwrap();
            // 第 1 個請求結束到這裡不到 gap（沒有人睡過），所以這次必須補睡。
            assert!(
                t.elapsed() >= Duration::from_millis(300),
                "同主機間隔未生效：{:?}",
                t.elapsed()
            );

            // 3. 跟隨轉址：302 → /final。
            let r = fetch
                .fetch(&Request::get(format!("{base}/redir")))
                .await
                .unwrap();
            assert_eq!((r.status, r.body.as_slice()), (200, b"landed".as_slice()));
            assert!(rx.recv().unwrap().starts_with("GET /redir"));
            assert!(rx.recv().unwrap().starts_with("GET /final"));

            // 4. POST 表單。
            let req =
                Request::post_form(format!("{base}/post"), [("TYPEK", "sii"), ("q", "a b~中")]);
            let r = fetch.fetch(&req).await.unwrap();
            assert_eq!(r.body, b"posted");
            let raw = rx.recv().unwrap();
            let lower = raw.to_ascii_lowercase();
            assert!(lower.starts_with("post /post http/1.1"), "{raw}");
            assert!(
                lower.contains("content-type: application/x-www-form-urlencoded\r\n"),
                "{raw}"
            );
            assert!(raw.ends_with("TYPEK=sii&q=a+b~%E4%B8%AD"), "{raw}");
        });
    }

    #[test]
    fn reqwest_fetch_connection_refused_is_network_error_not_status() {
        // 綁一個 port 再放掉，之後連它必定被拒絕（只碰 127.0.0.1）。
        let port = {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let fetch =
            ReqwestFetch::with_client(ReqwestFetch::builder().no_proxy(), Duration::ZERO).unwrap();
        let err = crate::fetch::test_util::block_on(async {
            fetch
                .fetch(&Request::get(format!("http://127.0.0.1:{port}/")))
                .await
        })
        .unwrap_err();
        assert!(matches!(err, FetchError::Network(_)), "{err:?}");
        let err = crate::fetch::test_util::block_on(async {
            get_ok(&fetch, &format!("http://127.0.0.1:{port}/")).await
        })
        .unwrap_err();
        assert!(matches!(err, FetchError::Network(_)), "{err:?}");
    }

    #[test]
    fn invalid_url_is_a_network_error() {
        let fetch = ReqwestFetch::new().unwrap();
        let err = crate::fetch::test_util::block_on(async {
            fetch.fetch(&Request::get("not a url")).await
        })
        .unwrap_err();
        assert!(matches!(err, FetchError::Network(_)), "{err:?}");
    }
}
