//! 自動代理（PAC／WPAD）解析：reqwest 的 `system-proxy` 只讀登錄的固定代理（`ProxyEnable`／
//! `ProxyServer`／`ProxyOverride`），不讀 `AutoConfigURL` 與自動偵測。只靠 PAC 的企業網路下宿主會
//! 全部直連，PAC 規定走代理的網域（ForexFactory、Yahoo、NY Fed）被防火牆重置（os error 10054）。
//! 本模組以 WinHTTP 依網址取得代理，由 `fetch::http` 經 `reqwest::Proxy::custom` 套用。

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

use windows::core::{w, HRESULT, PCWSTR, PWSTR};
use windows::Win32::Foundation::{GlobalFree, HGLOBAL};
use windows::Win32::Networking::WinHttp::{
    WinHttpCloseHandle, WinHttpGetIEProxyConfigForCurrentUser, WinHttpGetProxyForUrl, WinHttpOpen,
    ERROR_WINHTTP_LOGIN_FAILURE, WINHTTP_ACCESS_TYPE_NAMED_PROXY, WINHTTP_ACCESS_TYPE_NO_PROXY,
    WINHTTP_AUTOPROXY_AUTO_DETECT, WINHTTP_AUTOPROXY_CONFIG_URL, WINHTTP_AUTOPROXY_OPTIONS,
    WINHTTP_AUTO_DETECT_TYPE_DHCP, WINHTTP_AUTO_DETECT_TYPE_DNS_A,
    WINHTTP_CURRENT_USER_IE_PROXY_CONFIG, WINHTTP_PROXY_INFO,
};
use windows::Win32::System::Registry::HKEY_CURRENT_USER;

/// 目前使用者的自動代理設定（PAC 網址、自動偵測）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoProxyConfig {
    auto_detect: bool,
    config_url: Option<String>,
}

/// 「網際網路選項」的手動代理（「使用 Proxy 伺服器」那一格與例外清單）。
///
/// 「自動偵測設定」是 Windows 的預設值，所以只設手動代理的使用者通常兩者並存；自動偵測找不到設定時
/// Windows 自己會退回手動代理，宿主也要照做（[`choose`]）——只看自動那一半會把手動代理整個丟掉、全部直連。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualProxy {
    list: String,
    bypass: String,
}

impl ManualProxy {
    /// `list`＝`ProxyServer` 的寫法（`host:port`，或 `http=a:1;https=b:2` 逐協定）；`bypass`＝例外清單
    /// （`;` 分隔，可用 `*` 萬用字元，`<local>`＝不含句點的主機名）。`list` 為空白＝沒有手動代理。
    pub fn new(list: &str, bypass: &str) -> Option<Self> {
        (!list.trim().is_empty()).then(|| ManualProxy {
            list: list.to_string(),
            bypass: bypass.to_string(),
        })
    }

    /// `scheme://host` 該走的手動代理（`http://host:port` 形式）；例外清單命中或沒有適用的項目＝`None`。
    /// 逐協定寫法只取與 `scheme` 同名的項目（與 Windows 相同：只設 `http=` 時 https 不走代理）；
    /// `socks=` 不支援。
    pub fn proxy_for(&self, scheme: &str, host: &str) -> Option<String> {
        if self.bypasses(host) {
            return None;
        }
        let entries = || {
            self.list
                .split([';', ' ', '\t'])
                .map(str::trim)
                .filter(|s| !s.is_empty())
        };
        let entry = entries()
            .find_map(|e| {
                e.split_once('=')
                    .filter(|(k, _)| k.trim().eq_ignore_ascii_case(scheme))
                    .map(|(_, v)| v.trim())
            })
            .or_else(|| entries().find(|e| !e.contains('=')))
            .filter(|e| !e.is_empty())?;
        Some(if entry.contains("://") {
            entry.to_string()
        } else {
            format!("http://{entry}")
        })
    }

    fn bypasses(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        self.bypass
            .split([';', ' ', '\t'])
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .any(|pattern| {
                if pattern == "<local>" {
                    return !host.contains('.');
                }
                let pattern = pattern
                    .split_once("://")
                    .map_or(pattern.as_str(), |(_, rest)| rest);
                wildcard_match(pattern, &host)
            })
    }
}

/// `*` 萬用字元比對（其餘字元逐字相等；兩邊都已轉小寫）。
fn wildcard_match(pattern: &str, text: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or_default();
    let Some(mut rest) = text.strip_prefix(first) else {
        return false;
    };
    let mut parts = parts.peekable();
    if parts.peek().is_none() {
        return rest.is_empty();
    }
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            return rest.ends_with(part);
        }
        match rest.find(part) {
            Some(i) => rest = &rest[i + part.len()..],
            None => return false,
        }
    }
    true
}

/// 「網際網路選項」的整份代理設定。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IeProxyConfig {
    /// 有設 PAC 或開了自動偵測。
    pub auto: Option<AutoProxyConfig>,
    /// 有設手動代理。
    pub manual: Option<ManualProxy>,
}

/// 讀「網際網路選項」的代理設定；讀不到＝兩者皆無。
pub fn current() -> IeProxyConfig {
    let mut raw = WINHTTP_CURRENT_USER_IE_PROXY_CONFIG::default();
    // SAFETY: `raw` 是有效的輸出緩衝；成功時其中的字串由 WinHTTP 以 GlobalAlloc 配置，下面逐一
    // 以 `take_global_string` 讀出並釋放。
    if unsafe { WinHttpGetIEProxyConfigForCurrentUser(&mut raw) }.is_err() {
        return IeProxyConfig::default();
    }
    // SAFETY: 三個字串皆來自上面的成功呼叫，各釋放一次。
    let (config_url, proxy, bypass) = unsafe {
        (
            take_global_string(raw.lpszAutoConfigUrl),
            take_global_string(raw.lpszProxy),
            take_global_string(raw.lpszProxyBypass),
        )
    };
    let config_url = config_url.filter(|s| !s.trim().is_empty());
    let auto_detect = raw.fAutoDetect.as_bool();
    IeProxyConfig {
        auto: (auto_detect || config_url.is_some()).then_some(AutoProxyConfig {
            auto_detect,
            config_url,
        }),
        manual: proxy
            .filter(|_| manual_proxy_enabled(read_proxy_enable()))
            .and_then(|p| ManualProxy::new(&p, bypass.as_deref().unwrap_or_default())),
    }
}

/// 登錄 `Internet Settings\ProxyEnable`（「使用 Proxy 伺服器」那個勾）；讀不到＝`None`。
fn read_proxy_enable() -> Option<u32> {
    super::read_registry_dword(
        HKEY_CURRENT_USER,
        w!("Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings"),
        w!("ProxyEnable"),
    )
}

/// 手動代理算不算啟用：登錄明確寫 0（取消勾選）就不算——取消勾選後 `ProxyServer` 的舊值仍留在登錄，
/// 微軟文件沒有明說這時 WinHTTP 回不回那個舊值，所以自己再擋一次，不把請求送去已停用的代理。
/// 讀不到登錄值時相信 WinHTTP 的回覆。
fn manual_proxy_enabled(proxy_enable: Option<u32>) -> bool {
    proxy_enable != Some(0)
}

/// 自動解析結果與手動代理合起來的決定：自動那一半給了答案（代理或直連）就照它；自動解析失敗
/// （PAC 抓不到、自動偵測找不到設定）或根本沒設自動＝退回手動代理；都沒有＝直連（`None`）。
pub fn choose(
    auto: Option<Resolution>,
    manual: Option<&ManualProxy>,
    scheme: &str,
    host: &str,
) -> Option<String> {
    match auto {
        Some(Resolution::Proxy(proxy)) => Some(proxy),
        Some(Resolution::Direct) => None,
        Some(Resolution::Failed) | None => manual.and_then(|m| m.proxy_for(scheme, host)),
    }
}

/// 一份代理設定的「逐網址決定」：自動解析器與手動代理綁在一起，呼叫端拿不到其中一半
/// （v0.3.4 的回歸就是只接了自動那一半）。
pub struct ProxySelector {
    resolver: Option<Resolver>,
    manual: Option<ManualProxy>,
}

impl ProxySelector {
    /// 由整份設定建立；有設自動代理卻開不了 WinHTTP session 時，自動那一半視為沒有（只剩手動代理）。
    pub fn new(config: IeProxyConfig) -> Self {
        ProxySelector {
            resolver: config.auto.and_then(Resolver::new),
            manual: config.manual,
        }
    }

    /// 解析失敗只記 debug（給每 20 秒重建一次的探測用，不讓等網路期間的記錄檔被洗版）。
    fn quiet(self) -> Self {
        if let Some(resolver) = &self.resolver {
            resolver.failure_logged.store(true, Ordering::Relaxed);
        }
        self
    }

    /// `url`（完整網址，PAC 可依路徑分流；`scheme`／`host` 是它的協定與主機）該走的代理，`None`＝直連。
    /// 會阻塞（首次要下載並執行 PAC）。
    pub fn select(&self, url: &str, scheme: &str, host: &str) -> Option<String> {
        let auto = self.resolver.as_ref().map(|r| r.resolve(url));
        choose(auto, self.manual.as_ref(), scheme, host)
    }
}

/// 單次查詢 `scheme://host/` 該走的代理（每次重讀設定、開一個新的 WinHTTP session）；給等網路的探測用，
/// 抓取本身走 `fetch::http` 裡長期持有的 [`ProxySelector`]。
pub fn proxy_for_once(scheme: &str, host: &str) -> Option<String> {
    ProxySelector::new(current())
        .quiet()
        .select(&format!("{scheme}://{host}/"), scheme, host)
}

/// 一次解析的結果。`Failed`（PAC 下載或執行失敗）由呼叫端決定怎麼處理，不與 `Direct` 混為一談。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    Direct,
    /// `http://host:port`
    Proxy(String),
    Failed,
}

/// 持有一個長期的 WinHTTP session，讓 WinHTTP 自己快取 PAC 腳本與自動偵測結果；每個網址都重新
/// 詢問（PAC 可依完整網址分流，不能以主機快取）。`resolve` 會阻塞（首次要下載並執行 PAC）。
pub struct Resolver {
    config: AutoProxyConfig,
    session: Mutex<SessionHandle>,
    /// 解析失敗已記過一次（之後降成 debug：只開自動偵測、網路上沒有 WPAD 是一般網路的常態，每個請求都會失敗）。
    failure_logged: AtomicBool,
}

struct SessionHandle(*mut c_void);

// SAFETY: WinHTTP session handle 可跨執行緒使用；本模組只經 `Mutex` 存取、在 Drop 關閉一次。
unsafe impl Send for SessionHandle {}

impl Drop for SessionHandle {
    fn drop(&mut self) {
        // SAFETY: handle 由 WinHttpOpen 取得、非 null，只在這裡關一次。
        let _ = unsafe { WinHttpCloseHandle(self.0) };
    }
}

impl Resolver {
    /// 開 WinHTTP session；失敗回 `None`（呼叫端維持原本行為）。
    pub fn new(config: AutoProxyConfig) -> Option<Self> {
        // SAFETY: 參數皆為 null 或常數；回傳的 handle 由 `SessionHandle` 擁有。
        let session = unsafe {
            WinHttpOpen(
                PCWSTR::null(),
                WINHTTP_ACCESS_TYPE_NO_PROXY,
                PCWSTR::null(),
                PCWSTR::null(),
                0,
            )
        };
        if session.is_null() {
            log::warn!("自動代理：WinHttpOpen 失敗，維持原本的連線方式");
            return None;
        }
        Some(Resolver {
            config,
            session: Mutex::new(SessionHandle(session)),
            failure_logged: AtomicBool::new(false),
        })
    }

    /// 依 PAC／自動偵測取得 `url` 該走的代理。
    pub fn resolve(&self, url: &str) -> Resolution {
        // 自動登入先關著（開著會停用 WinHTTP 的 PAC 快取），PAC 伺服器要求驗證時才開啟重試（微軟範例的作法）。
        let mut options = WINHTTP_AUTOPROXY_OPTIONS::default();
        let config_url_w: Option<Vec<u16>> = self.config.config_url.as_deref().map(wide);
        if let Some(w) = &config_url_w {
            options.dwFlags |= WINHTTP_AUTOPROXY_CONFIG_URL;
            options.lpszAutoConfigUrl = PCWSTR(w.as_ptr());
        }
        if self.config.auto_detect {
            options.dwFlags |= WINHTTP_AUTOPROXY_AUTO_DETECT;
            options.dwAutoDetectFlags =
                WINHTTP_AUTO_DETECT_TYPE_DHCP | WINHTTP_AUTO_DETECT_TYPE_DNS_A;
        }
        let url_w = wide(url);
        let mut info = WINHTTP_PROXY_INFO::default();
        let session = self.session.lock().unwrap_or_else(PoisonError::into_inner);
        // SAFETY: session 有效（持鎖期間不會被關）；`options` 的字串指標指向仍存活的
        // `config_url_w`；`url_w` 存活到呼叫結束。
        let mut result = unsafe {
            WinHttpGetProxyForUrl(session.0, PCWSTR(url_w.as_ptr()), &mut options, &mut info)
        };
        if matches!(&result, Err(e) if e.code() == HRESULT::from_win32(ERROR_WINHTTP_LOGIN_FAILURE))
        {
            options.fAutoLogonIfChallenged = true.into();
            // SAFETY: 同上一次呼叫。
            result = unsafe {
                WinHttpGetProxyForUrl(session.0, PCWSTR(url_w.as_ptr()), &mut options, &mut info)
            };
        }
        drop(session);
        if let Err(e) = result {
            if self.failure_logged.swap(true, Ordering::Relaxed) {
                log::debug!("自動代理：解析 {url} 失敗（{e}）");
            } else if self.config.config_url.is_some() {
                log::warn!("自動代理：解析 {url} 失敗（{e}）；之後的失敗不再逐筆記錄");
            } else {
                log::info!(
                    "自動代理：自動偵測沒有找到代理設定（{e}；沒有 WPAD 的網路屬正常），改用手動代理或直連"
                );
            }
            return Resolution::Failed;
        }
        // SAFETY: 成功時兩個字串由 WinHTTP 以 GlobalAlloc 配置，各釋放一次。
        let (proxy, _) = unsafe {
            (
                take_global_string(info.lpszProxy),
                take_global_string(info.lpszProxyBypass),
            )
        };
        if info.dwAccessType != WINHTTP_ACCESS_TYPE_NAMED_PROXY {
            return Resolution::Direct;
        }
        proxy
            .as_deref()
            .and_then(first_proxy)
            .map_or(Resolution::Direct, Resolution::Proxy)
    }
}

/// WinHTTP 的代理清單（`;` 或空白分隔，可能帶 `http=` 前綴）取第一個，轉成 `http://host:port`。
/// 已知限制：PAC 給的備援代理不輪替（`reqwest::Proxy::custom` 一次只給一個），主代理掛掉時該請求失敗。
fn first_proxy(list: &str) -> Option<String> {
    let entry = list
        .split([';', ' ', '\t'])
        .map(str::trim)
        .find(|s| !s.is_empty())?;
    let entry = entry.rsplit_once('=').map_or(entry, |(_, v)| v);
    if entry.contains("://") {
        Some(entry.to_string())
    } else {
        Some(format!("http://{entry}"))
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 讀出 WinHTTP 以 GlobalAlloc 配置的字串並釋放。
///
/// # Safety
/// `p` 須為 null 或 WinHTTP 回傳、尚未釋放的 GlobalAlloc 字串。
unsafe fn take_global_string(p: PWSTR) -> Option<String> {
    if p.is_null() {
        return None;
    }
    // SAFETY: 依函式前提，`p` 是有效的 NUL 結尾 UTF-16 字串。
    let s = unsafe { p.to_string() }.ok();
    // SAFETY: 依函式前提，`p` 由 GlobalAlloc 配置且只在此釋放一次。
    let _ = unsafe { GlobalFree(Some(HGLOBAL(p.0.cast()))) };
    s
}

#[cfg(test)]
mod tests {
    use super::{choose, first_proxy, wildcard_match, ManualProxy, Resolution};

    fn manual(list: &str, bypass: &str) -> ManualProxy {
        ManualProxy::new(list, bypass).expect("非空的代理清單")
    }

    #[test]
    fn manual_proxy_single_entry_applies_to_every_scheme() {
        let m = manual("proxy.corp:8080", "");
        for scheme in ["http", "https"] {
            assert_eq!(
                m.proxy_for(scheme, "query1.finance.yahoo.com").as_deref(),
                Some("http://proxy.corp:8080")
            );
        }
        assert_eq!(
            manual("http://p:1", "")
                .proxy_for("https", "a.b")
                .as_deref(),
            Some("http://p:1"),
            "已有 scheme 就不再加"
        );
        assert_eq!(ManualProxy::new("  ", "x"), None);
    }

    #[test]
    fn manual_proxy_per_protocol_entries_pick_the_matching_scheme() {
        let m = manual("http=a:1;https=b:2;ftp=c:3", "");
        assert_eq!(m.proxy_for("https", "x.y").as_deref(), Some("http://b:2"));
        assert_eq!(m.proxy_for("http", "x.y").as_deref(), Some("http://a:1"));
        // 只設 http= 時 https 不走代理（與 Windows 相同）；socks 不支援。
        assert_eq!(manual("http=a:1", "").proxy_for("https", "x.y"), None);
        assert_eq!(manual("socks=s:1080", "").proxy_for("https", "x.y"), None);
        // 逐協定與通用項目並存：沒有同名協定就用通用的。
        assert_eq!(
            manual("ftp=c:3 d:4", "")
                .proxy_for("https", "x.y")
                .as_deref(),
            Some("http://d:4")
        );
    }

    #[test]
    fn manual_proxy_bypass_list_is_honoured() {
        let m = manual(
            "p:8080",
            "*.twse.com.tw; 10.*;<local>;https://exact.example.com",
        );
        assert_eq!(m.proxy_for("https", "openapi.twse.com.tw"), None);
        assert_eq!(m.proxy_for("https", "OPENAPI.TWSE.COM.TW"), None);
        assert_eq!(m.proxy_for("https", "10.1.2.3"), None);
        assert_eq!(m.proxy_for("https", "intranet"), None, "<local>＝不含句點");
        assert_eq!(m.proxy_for("https", "exact.example.com"), None);
        assert_eq!(
            m.proxy_for("https", "twse.com.tw").as_deref(),
            Some("http://p:8080"),
            "*.twse.com.tw 不含裸網域"
        );
        assert_eq!(
            m.proxy_for("https", "query1.finance.yahoo.com").as_deref(),
            Some("http://p:8080")
        );
    }

    #[test]
    fn wildcard_match_cases() {
        for (pattern, text, want) in [
            ("abc", "abc", true),
            ("abc", "abcd", false),
            ("*", "anything", true),
            ("*.tw", "a.tw", true),
            ("*.tw", "tw", false),
            ("10.*", "10.0.0.1", true),
            ("a*c*e", "abcde", true),
            ("a*c*e", "abde", false),
            ("a*a", "a", false),
            ("", "", true),
            ("", "x", false),
        ] {
            assert_eq!(wildcard_match(pattern, text), want, "{pattern} vs {text}");
        }
    }

    /// 抓取實際走的那個物件（`fetch::http` 把它掛上 `reqwest::Proxy::custom`），經真正的 WinHTTP：
    /// PAC 指向本機沒有人聽的連接埠＝解析失敗（不連外），這時手動代理要接得上、例外清單照樣生效。
    /// 只接自動那一半就會回到 v0.3.4 的全部直連。
    #[test]
    fn selector_falls_back_to_the_manual_proxy_when_the_pac_cannot_be_fetched() {
        let unreachable_pac = || super::AutoProxyConfig {
            auto_detect: false,
            config_url: Some("http://127.0.0.1:9/missing.pac".to_string()),
        };
        let url = "https://query1.finance.yahoo.com/v8/finance/chart/x";
        let host = "query1.finance.yahoo.com";

        let with_manual = super::ProxySelector::new(super::IeProxyConfig {
            auto: Some(unreachable_pac()),
            manual: Some(manual("proxy.corp:8080", "*.tw")),
        });
        assert_eq!(
            with_manual.select(url, "https", host).as_deref(),
            Some("http://proxy.corp:8080")
        );
        assert_eq!(
            with_manual.select(
                "https://openapi.twse.com.tw/v1/x",
                "https",
                "openapi.twse.com.tw"
            ),
            None,
            "例外清單命中＝直連"
        );

        let auto_only = super::ProxySelector::new(super::IeProxyConfig {
            auto: Some(unreachable_pac()),
            manual: None,
        });
        assert_eq!(auto_only.select(url, "https", host), None);

        // 沒有自動設定：只看手動代理。
        let manual_only = super::ProxySelector::new(super::IeProxyConfig {
            auto: None,
            manual: Some(manual("proxy.corp:8080", "")),
        });
        assert_eq!(
            manual_only.select(url, "https", host).as_deref(),
            Some("http://proxy.corp:8080")
        );
    }

    #[test]
    fn manual_proxy_counts_as_enabled_unless_the_registry_says_zero() {
        assert!(!super::manual_proxy_enabled(Some(0)), "取消勾選");
        assert!(super::manual_proxy_enabled(Some(1)));
        assert!(super::manual_proxy_enabled(None), "讀不到就相信 WinHTTP");
    }

    /// v0.3.4 的回歸：自動偵測（Windows 預設開啟）失敗時手動代理被丟掉、全部直連。
    #[test]
    fn choose_falls_back_to_the_manual_proxy_when_auto_resolution_fails() {
        let m = manual("proxy.corp:8080", "*.tw");
        let pick = |auto| choose(auto, Some(&m), "https", "query1.finance.yahoo.com");
        assert_eq!(
            pick(Some(Resolution::Failed)).as_deref(),
            Some("http://proxy.corp:8080")
        );
        assert_eq!(pick(None).as_deref(), Some("http://proxy.corp:8080"));
        // 自動那一半有答案就照它：PAC 說直連＝直連，不再套手動代理。
        assert_eq!(pick(Some(Resolution::Direct)), None);
        assert_eq!(
            pick(Some(Resolution::Proxy("http://pac:3128".into()))).as_deref(),
            Some("http://pac:3128")
        );
        // 退回手動代理時例外清單照樣生效；沒有手動代理＝直連。
        assert_eq!(
            choose(
                Some(Resolution::Failed),
                Some(&m),
                "https",
                "openapi.twse.com.tw"
            ),
            None
        );
        assert_eq!(choose(Some(Resolution::Failed), None, "https", "a.b"), None);
    }

    #[test]
    fn first_proxy_takes_first_entry_and_adds_scheme() {
        assert_eq!(
            first_proxy("10.0.0.1:8080; 10.0.0.2:8080").as_deref(),
            Some("http://10.0.0.1:8080")
        );
        assert_eq!(
            first_proxy("https=proxy.corp:3128").as_deref(),
            Some("http://proxy.corp:3128")
        );
        assert_eq!(first_proxy("  ").as_deref(), None);
    }

    /// 實機檢查：`FC_HOST_TEST_PAC_URL` 指向一份 PAC（以 HTTP 提供），內容為
    /// `if (shExpMatch(host, "*.tw")) return "DIRECT"; return "PROXY 127.0.0.1:3128";`，
    /// 再跑 `cargo test pac_resolves_per_host -- --ignored`（2026-10-12 本機通過）。
    /// 寫 PAC 時用 `shExpMatch`：同樣判斷改用 `dnsDomainIs(host, ".tw")` 時，WinHTTP 對 `.tw` 也回代理。
    #[test]
    #[ignore = "需要本機 HTTP 伺服器提供 PAC"]
    fn pac_resolves_per_host() {
        let url = std::env::var("FC_HOST_TEST_PAC_URL").expect("設定 FC_HOST_TEST_PAC_URL");
        let config = super::AutoProxyConfig {
            auto_detect: false,
            config_url: Some(url),
        };
        let resolver = super::Resolver::new(config).expect("WinHttpOpen");
        assert_eq!(
            resolver.resolve("https://query1.finance.yahoo.com/v8/finance/chart/x"),
            super::Resolution::Proxy("http://127.0.0.1:3128".to_string())
        );
        assert_eq!(
            resolver.resolve("https://openapi.twse.com.tw/v1/x"),
            super::Resolution::Direct
        );

        // PAC 抓不到＝Failed，不得與 Direct 混為一談。
        let broken = super::Resolver::new(super::AutoProxyConfig {
            auto_detect: false,
            config_url: Some("http://127.0.0.1:9/missing.pac".to_string()),
        })
        .expect("WinHttpOpen");
        assert_eq!(
            broken.resolve("https://query1.finance.yahoo.com/"),
            super::Resolution::Failed
        );
    }
}
