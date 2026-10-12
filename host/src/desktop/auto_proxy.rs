//! 自動代理（PAC／WPAD）解析：reqwest 的 `system-proxy` 只讀登錄的固定代理（`ProxyEnable`／
//! `ProxyServer`／`ProxyOverride`），不讀 `AutoConfigURL` 與自動偵測。只靠 PAC 的企業網路下宿主會
//! 全部直連，PAC 規定走代理的網域（ForexFactory、Yahoo、NY Fed）被防火牆重置（os error 10054）。
//! 本模組以 WinHTTP 依網址取得代理，由 `fetch::http` 經 `reqwest::Proxy::custom` 套用。

use std::ffi::c_void;
use std::sync::{Mutex, PoisonError};

use windows::core::{HRESULT, PCWSTR, PWSTR};
use windows::Win32::Foundation::{GlobalFree, HGLOBAL};
use windows::Win32::Networking::WinHttp::{
    WinHttpCloseHandle, WinHttpGetIEProxyConfigForCurrentUser, WinHttpGetProxyForUrl, WinHttpOpen,
    ERROR_WINHTTP_LOGIN_FAILURE, WINHTTP_ACCESS_TYPE_NAMED_PROXY, WINHTTP_ACCESS_TYPE_NO_PROXY,
    WINHTTP_AUTOPROXY_AUTO_DETECT, WINHTTP_AUTOPROXY_CONFIG_URL, WINHTTP_AUTOPROXY_OPTIONS,
    WINHTTP_AUTO_DETECT_TYPE_DHCP, WINHTTP_AUTO_DETECT_TYPE_DNS_A,
    WINHTTP_CURRENT_USER_IE_PROXY_CONFIG, WINHTTP_PROXY_INFO,
};

/// 目前使用者的自動代理設定；`None`＝沒有設 PAC 也沒開自動偵測（維持 reqwest 內建行為）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoProxyConfig {
    auto_detect: bool,
    config_url: Option<String>,
}

/// 讀「網際網路選項」的自動代理設定。
pub fn current_config() -> Option<AutoProxyConfig> {
    let mut raw = WINHTTP_CURRENT_USER_IE_PROXY_CONFIG::default();
    // SAFETY: `raw` 是有效的輸出緩衝；成功時其中的字串由 WinHTTP 以 GlobalAlloc 配置，下面逐一
    // 以 `take_global_string` 讀出並釋放。
    unsafe { WinHttpGetIEProxyConfigForCurrentUser(&mut raw) }.ok()?;
    // SAFETY: 三個字串皆來自上面的成功呼叫，各釋放一次。
    let (config_url, _, _) = unsafe {
        (
            take_global_string(raw.lpszAutoConfigUrl),
            take_global_string(raw.lpszProxy),
            take_global_string(raw.lpszProxyBypass),
        )
    };
    let config_url = config_url.filter(|s| !s.trim().is_empty());
    let auto_detect = raw.fAutoDetect.as_bool();
    (auto_detect || config_url.is_some()).then_some(AutoProxyConfig {
        auto_detect,
        config_url,
    })
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
            log::warn!("自動代理：解析 {url} 失敗（{e}）");
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
    use super::first_proxy;

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
