//! 更新器啟用判定（design.md D3、D7）：在註冊外掛之前，由設定檔的 `plugins.updater` 與建置旗標決定要不要
//! 啟用更新器。純函式，輸入是設定的 JSON 值與「這次建置有沒有 `update-e2e`」。
//!
//! 為什麼要在註冊外掛**之前**判定：
//!
//! - 外掛在 `Builder::build()` 時就解析 `plugins.updater`；解析失敗（例如 release 建置遇到非 https 端點）
//!   會讓整個 `tauri::Builder::build` 失敗＝宿主無法啟動。更新器是附屬功能，不得因此拖垮整個宿主，所以
//!   判定不過就根本不註冊外掛、其餘功能照常。
//! - 外掛對占位公鑰**不會**在建置時失敗，到下載完才在 `base64` 解碼時報錯——白白下載數十 MB。占位公鑰
//!   （正式金鑰尚未產生，task 1.1）在這裡就停用。
//! - `dangerousInsecureTransportProtocol`（允許 http 端點）是設定檔層級的旗標，Rust feature 擋不住；
//!   建置不含 `update-e2e` 卻讀到它＝後門漏進正式建置，停用並記錄錯誤（design.md D7 的第一道檢查，
//!   第二道是 `package.ps1` 對執行檔的字串搜尋）。`dangerousAcceptInvalidCerts`／
//!   `dangerousAcceptInvalidHostnames` 同屬後門旗標，比照辦理。
//! - 隔離環境（驗收腳本以暫存資料夾隔離宿主，環境變數 `LOCALAPPDATA` 與系統登記的不同）：NSIS 安裝檔與
//!   `HKCU\…\Run` 都不跟環境變數走，隔離的舊版測試宿主下載正式版後會安裝到**真正的**使用者資料夾、寫真正的
//!   登錄。與資料抓取的 `auto` 一致，隔離時停用（[`decide`]）；`update-e2e` 建置例外——e2e 要在任何環境都能跑
//!   更新。

use crate::desktop::LocalAppDataMismatch;
use serde_json::Value;

/// 更新器是否啟用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    Enabled,
    Disabled(DisabledReason),
}

impl Gate {
    pub fn is_enabled(&self) -> bool {
        matches!(self, Self::Enabled)
    }
}

/// 停用原因（記錄用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisabledReason {
    /// 設定檔沒有 `plugins.updater`。
    NoConfig,
    /// `plugins.updater` 解析失敗（含 release 建置的非 https 端點）。
    InvalidConfig(String),
    /// 沒有任何端點。
    NoEndpoints,
    /// 端點不是 https，也沒有（合法的）`dangerousInsecureTransportProtocol`。
    InsecureEndpoint(String),
    /// 設定允許不安全傳輸／憑證，但這次建置不含 `update-e2e`。
    DangerousFlagWithoutE2e(&'static str),
    /// 公鑰仍是占位字串（正式金鑰尚未設定）。
    PlaceholderPubkey,
    /// 隔離環境（`LOCALAPPDATA` 與系統登記的不同），且這次建置不含 `update-e2e`。
    Isolated(LocalAppDataMismatch),
}

impl DisabledReason {
    /// 這個原因該用 error 還是 warn 記錄：後門旗標與壞設定是錯誤；占位公鑰是開發中的已知狀態；
    /// 隔離環境是驗收腳本的預期情境。
    pub fn is_error(&self) -> bool {
        !matches!(
            self,
            Self::PlaceholderPubkey | Self::NoConfig | Self::Isolated(_)
        )
    }
}

impl std::fmt::Display for DisabledReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoConfig => write!(f, "設定檔沒有 plugins.updater"),
            Self::InvalidConfig(e) => write!(f, "plugins.updater 設定無法解析：{e}"),
            Self::NoEndpoints => write!(f, "plugins.updater.endpoints 是空的"),
            Self::InsecureEndpoint(url) => write!(f, "更新端點不是 https：{url}"),
            Self::DangerousFlagWithoutE2e(flag) => write!(
                f,
                "設定檔允許 {flag}，但這次建置不含 update-e2e feature（測試用後門不得進正式建置）"
            ),
            Self::PlaceholderPubkey => write!(f, "尚未設定正式公鑰（pubkey 仍是占位字串）"),
            Self::Isolated(m) => write!(
                f,
                "隔離環境，不檢查更新（LOCALAPPDATA={}，系統登記={}）",
                m.env_value, m.registered
            ),
        }
    }
}

/// `minisign` 公鑰檔的第一行固定是 `untrusted comment: …`，Tauri 要求的 `pubkey` 是整份 `.pub` 檔的
/// base64，所以一定以這段文字的 base64 開頭。
const PUBKEY_BASE64_PREFIX: &str = "dW50cnVzdGVkIGNvbW1lbnQ6";

/// 公鑰是不是占位字串（正式金鑰尚未產生、或 e2e 設定檔提交時的占位）。**啟發式**：真公鑰＝整份 minisign
/// 公鑰檔的 base64（字元全在 base64 字母表內、以 `untrusted comment:` 的 base64 開頭）；其餘一律視為
/// 占位。真正的解碼與驗簽仍由外掛負責，這裡只擋「明顯不可能驗簽成功」的情況。
pub fn pubkey_is_placeholder(pubkey: &str) -> bool {
    let key = pubkey.trim();
    let looks_base64 = !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '='));
    !(looks_base64 && key.starts_with(PUBKEY_BASE64_PREFIX))
}

/// 更新器啟用判定的入口：隔離環境（`isolation` 為 `Some`，來源是
/// [`crate::desktop::detect_isolated_local_app_data`]，與資料抓取共用同一個判斷）且不是 `update-e2e` 建置
/// ＝停用；否則依 `plugins.updater` 設定值與建置旗標判定。`e2e_build`＝`cfg!(feature = "update-e2e")`。
pub fn decide(
    plugin_config: Option<&Value>,
    e2e_build: bool,
    isolation: Option<&LocalAppDataMismatch>,
) -> Gate {
    if !e2e_build {
        if let Some(mismatch) = isolation {
            return Gate::Disabled(DisabledReason::Isolated(mismatch.clone()));
        }
    }
    evaluate(plugin_config, e2e_build)
}

/// 依 `plugins.updater` 設定值與建置旗標判定（不含隔離環境，見 [`decide`]）。
fn evaluate(plugin_config: Option<&Value>, e2e_build: bool) -> Gate {
    let Some(value) = plugin_config else {
        return Gate::Disabled(DisabledReason::NoConfig);
    };
    let config: tauri_plugin_updater::Config = match serde_json::from_value(value.clone()) {
        Ok(c) => c,
        Err(e) => return Gate::Disabled(DisabledReason::InvalidConfig(e.to_string())),
    };
    if !e2e_build {
        for (set, name) in [
            (
                config.dangerous_insecure_transport_protocol,
                "dangerousInsecureTransportProtocol",
            ),
            (
                config.dangerous_accept_invalid_certs,
                "dangerousAcceptInvalidCerts",
            ),
            (
                config.dangerous_accept_invalid_hostnames,
                "dangerousAcceptInvalidHostnames",
            ),
        ] {
            if set {
                return Gate::Disabled(DisabledReason::DangerousFlagWithoutE2e(name));
            }
        }
    }
    if config.endpoints.is_empty() {
        return Gate::Disabled(DisabledReason::NoEndpoints);
    }
    if !config.dangerous_insecure_transport_protocol {
        // 外掛在 release 建置才會拒絕非 https，debug 只警告；這裡兩種建置一致。
        if let Some(url) = config.endpoints.iter().find(|u| u.scheme() != "https") {
            return Gate::Disabled(DisabledReason::InsecureEndpoint(url.to_string()));
        }
    }
    if pubkey_is_placeholder(&config.pubkey) {
        return Gate::Disabled(DisabledReason::PlaceholderPubkey);
    }
    Gate::Enabled
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 長得像真公鑰的字串（`untrusted comment: minisign public key: AAAA` 的 base64＋金鑰本體），
    /// 只用來測啟發式，不是任何真金鑰。
    const REAL_LOOKING_PUBKEY: &str =
        "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDAwMDAwMDAwMDAwMDAwMDAKUldRQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUE9Cg==";

    fn cfg(pubkey: &str, endpoint: &str) -> Value {
        json!({
            "pubkey": pubkey,
            "endpoints": [endpoint],
            "requireSignedVersion": true,
            "windows": { "installMode": "passive" }
        })
    }

    #[test]
    fn good_production_config_is_enabled() {
        let v = cfg(REAL_LOOKING_PUBKEY, "https://example.test/latest.json");
        assert_eq!(evaluate(Some(&v), false), Gate::Enabled);
    }

    #[test]
    fn placeholder_pubkey_disables_updater_without_failing() {
        // tauri.conf.json 目前的占位字串（task 1.1 前）與 e2e 設定檔的占位字串。
        for placeholder in [
            "PRODUCTION_PUBKEY_PENDING_TASK_1_1",
            "E2E_TEST_PUBKEY_PLACEHOLDER",
            "",
            "   ",
        ] {
            let v = cfg(placeholder, "https://example.test/latest.json");
            assert_eq!(
                evaluate(Some(&v), false),
                Gate::Disabled(DisabledReason::PlaceholderPubkey),
                "{placeholder:?}"
            );
        }
        assert!(
            !DisabledReason::PlaceholderPubkey.is_error(),
            "開發中的已知狀態，不算錯誤"
        );
    }

    #[test]
    fn repo_tauri_conf_currently_disables_updater() {
        // 讀真正的 tauri.conf.json：正式公鑰設定（task 1.1）之前，更新器必須是停用狀態且宿主照常啟動。
        let conf: Value = serde_json::from_str(include_str!("../../tauri.conf.json"))
            .expect("tauri.conf.json 是合法 JSON");
        let gate = evaluate(conf.pointer("/plugins/updater"), false);
        let pubkey = conf
            .pointer("/plugins/updater/pubkey")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if pubkey_is_placeholder(pubkey) {
            assert_eq!(gate, Gate::Disabled(DisabledReason::PlaceholderPubkey));
        } else {
            assert_eq!(
                gate,
                Gate::Enabled,
                "已填入正式公鑰後，這份設定應能啟用更新器"
            );
        }
    }

    #[test]
    fn insecure_transport_flag_without_e2e_disables_updater() {
        // D7：設定允許 http，但建置不含 update-e2e＝後門漏進正式建置。
        let mut v = cfg(REAL_LOOKING_PUBKEY, "http://127.0.0.1:8737/latest.json");
        v["dangerousInsecureTransportProtocol"] = json!(true);
        let gate = evaluate(Some(&v), false);
        assert_eq!(
            gate,
            Gate::Disabled(DisabledReason::DangerousFlagWithoutE2e(
                "dangerousInsecureTransportProtocol"
            ))
        );
        assert!(
            matches!(&gate, Gate::Disabled(r) if r.is_error()),
            "這是要記錄為錯誤的情況"
        );
        // 有 update-e2e 的建置放行。
        assert_eq!(evaluate(Some(&v), true), Gate::Enabled);
    }

    /// 讀真正的 `tauri.conf.json` 疊上 `tauri.e2e.conf.json`（`cargo tauri build --config` 的合併語意：
    /// 物件逐鍵覆蓋），取出 `plugins.updater`。
    fn merged_e2e_updater_config() -> Value {
        let base: Value = serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
        let e2e: Value = serde_json::from_str(include_str!("../../tauri.e2e.conf.json")).unwrap();
        let mut merged = base.pointer("/plugins/updater").cloned().unwrap();
        for (k, v) in e2e
            .pointer("/plugins/updater")
            .unwrap()
            .as_object()
            .unwrap()
        {
            merged[k] = v.clone();
        }
        merged
    }

    #[test]
    fn e2e_config_in_a_non_e2e_build_disables_updater() {
        // design.md D7 第一道檢查：e2e 設定檔（http 端點＋dangerousInsecureTransportProtocol）被打進
        // 不含 `update-e2e` 的建置時，更新器必須停用，而不是去連 http 端點。
        let merged = merged_e2e_updater_config();
        assert_eq!(
            merged["dangerousInsecureTransportProtocol"],
            json!(true),
            "前提：e2e 設定檔確實開了這個旗標"
        );
        assert_eq!(
            evaluate(Some(&merged), false),
            Gate::Disabled(DisabledReason::DangerousFlagWithoutE2e(
                "dangerousInsecureTransportProtocol"
            ))
        );
        // 換成真的測試公鑰（e2e 流程每次重新產生並寫入）後，有 update-e2e 的建置放行。
        let mut with_key = merged;
        with_key["pubkey"] = json!(REAL_LOOKING_PUBKEY);
        assert_eq!(evaluate(Some(&with_key), true), Gate::Enabled);
    }

    /// 3.4：以「真的 e2e 設定檔（疊在 tauri.conf.json 上）＋e2e 流程換入的測試公鑰」測 `update-e2e` 建置。
    fn e2e_config_with_test_key() -> Value {
        let mut merged = merged_e2e_updater_config();
        merged["pubkey"] = json!(REAL_LOOKING_PUBKEY);
        merged
    }

    #[test]
    fn e2e_feature_build_enables_the_updater_with_the_e2e_config() {
        // 帶 `update-e2e` 的建置：e2e 設定檔（http 端點＋dangerousInsecureTransportProtocol＋測試公鑰）放行。
        let merged = e2e_config_with_test_key();
        assert_eq!(
            merged["endpoints"][0]
                .as_str()
                .map(|u| u.starts_with("http://127.0.0.1")),
            Some(true),
            "前提：端點是本機 http"
        );
        assert_eq!(evaluate(Some(&merged), true), Gate::Enabled);
        // 公鑰仍是提交時的占位字串（e2e 流程尚未換入拋棄式金鑰）：即使帶 feature 也不啟用，避免白下載。
        let committed = merged_e2e_updater_config();
        assert_eq!(
            evaluate(Some(&committed), true),
            Gate::Disabled(DisabledReason::PlaceholderPubkey)
        );
    }

    #[test]
    fn production_build_disables_the_updater_under_the_e2e_config() {
        // 不帶 `update-e2e` 的正式建置，即使 e2e 設定檔（含已換入的測試公鑰）被打進去也停用，且記為錯誤。
        let merged = e2e_config_with_test_key();
        let gate = evaluate(Some(&merged), false);
        assert_eq!(
            gate,
            Gate::Disabled(DisabledReason::DangerousFlagWithoutE2e(
                "dangerousInsecureTransportProtocol"
            ))
        );
        assert!(matches!(&gate, Gate::Disabled(r) if r.is_error()));
    }

    #[test]
    fn this_build_follows_its_own_update_e2e_feature() {
        // 在 `cargo test` 與 `cargo test --features update-e2e` 兩種跑法下都成立：旗標是否放行，
        // 取決於「這個二進位檔」編譯時有沒有 update-e2e，而不是設定檔。
        let mut v = cfg(REAL_LOOKING_PUBKEY, "http://127.0.0.1:8737/latest.json");
        v["dangerousInsecureTransportProtocol"] = json!(true);
        let gate = evaluate(Some(&v), cfg!(feature = "update-e2e"));
        if cfg!(feature = "update-e2e") {
            assert_eq!(gate, Gate::Enabled);
        } else {
            assert!(matches!(
                gate,
                Gate::Disabled(DisabledReason::DangerousFlagWithoutE2e(_))
            ));
        }
    }

    #[test]
    fn other_dangerous_flags_are_also_blocked_without_e2e() {
        for (flag, name) in [
            ("dangerousAcceptInvalidCerts", "dangerousAcceptInvalidCerts"),
            (
                "dangerousAcceptInvalidHostnames",
                "dangerousAcceptInvalidHostnames",
            ),
        ] {
            let mut v = cfg(REAL_LOOKING_PUBKEY, "https://example.test/latest.json");
            v[flag] = json!(true);
            assert_eq!(
                evaluate(Some(&v), false),
                Gate::Disabled(DisabledReason::DangerousFlagWithoutE2e(name))
            );
            assert_eq!(evaluate(Some(&v), true), Gate::Enabled);
        }
    }

    #[test]
    fn http_endpoint_without_flag_is_disabled_not_a_startup_failure() {
        let v = cfg(REAL_LOOKING_PUBKEY, "http://example.test/latest.json");
        assert!(matches!(
            evaluate(Some(&v), false),
            Gate::Disabled(DisabledReason::InsecureEndpoint(_))
        ));
    }

    #[test]
    fn missing_or_broken_config_is_disabled() {
        assert_eq!(
            evaluate(None, false),
            Gate::Disabled(DisabledReason::NoConfig)
        );
        // 缺必填欄位 pubkey。
        let broken = json!({ "endpoints": ["https://example.test/x.json"] });
        assert!(matches!(
            evaluate(Some(&broken), false),
            Gate::Disabled(DisabledReason::InvalidConfig(_))
        ));
        let none = json!({ "pubkey": REAL_LOOKING_PUBKEY, "endpoints": [] });
        assert_eq!(
            evaluate(Some(&none), false),
            Gate::Disabled(DisabledReason::NoEndpoints)
        );
    }

    fn mismatch() -> LocalAppDataMismatch {
        LocalAppDataMismatch {
            env_value: r"C:\Temp\iso\Local".to_string(),
            registered: r"C:\Users\someone\AppData\Local".to_string(),
        }
    }

    #[test]
    fn isolated_environment_disables_the_updater_in_a_normal_build() {
        let v = cfg(REAL_LOOKING_PUBKEY, "https://example.test/latest.json");
        let m = mismatch();
        let gate = decide(Some(&v), false, Some(&m));
        assert_eq!(gate, Gate::Disabled(DisabledReason::Isolated(m.clone())));
        // 記錄行要寫明原因與兩個路徑；不是錯誤。
        let Gate::Disabled(reason) = &gate else {
            unreachable!()
        };
        assert!(!reason.is_error());
        let text = reason.to_string();
        assert!(
            text.contains(&m.env_value) && text.contains(&m.registered),
            "{text}"
        );
    }

    #[test]
    fn non_isolated_environment_keeps_the_updater_enabled() {
        let v = cfg(REAL_LOOKING_PUBKEY, "https://example.test/latest.json");
        assert_eq!(decide(Some(&v), false, None), Gate::Enabled);
        assert_eq!(decide(Some(&v), true, None), Gate::Enabled);
    }

    #[test]
    fn update_e2e_build_ignores_isolation() {
        // e2e 要在任何環境（含隔離的暫存資料夾）都能跑更新。
        let v = cfg(REAL_LOOKING_PUBKEY, "https://example.test/latest.json");
        assert_eq!(decide(Some(&v), true, Some(&mismatch())), Gate::Enabled);
    }

    #[test]
    fn isolation_wins_over_config_problems_in_a_normal_build() {
        // 隔離時連設定都不必看（壞設定也不記成錯誤）；e2e 建置則照常評估設定。
        let m = mismatch();
        assert_eq!(
            decide(None, false, Some(&m)),
            Gate::Disabled(DisabledReason::Isolated(m.clone()))
        );
        assert_eq!(
            decide(None, true, Some(&m)),
            Gate::Disabled(DisabledReason::NoConfig)
        );
    }

    #[test]
    fn this_build_follows_its_own_update_e2e_feature_under_isolation() {
        // `cargo test` 與 `cargo test --features update-e2e` 兩種跑法：隔離是否停用取決於編譯時的 cfg。
        let v = cfg(REAL_LOOKING_PUBKEY, "https://example.test/latest.json");
        let gate = decide(Some(&v), cfg!(feature = "update-e2e"), Some(&mismatch()));
        if cfg!(feature = "update-e2e") {
            assert_eq!(gate, Gate::Enabled);
        } else {
            assert!(matches!(gate, Gate::Disabled(DisabledReason::Isolated(_))));
        }
    }

    #[test]
    fn pubkey_heuristic_accepts_real_shape_only() {
        assert!(!pubkey_is_placeholder(REAL_LOOKING_PUBKEY));
        assert!(pubkey_is_placeholder("PRODUCTION_PUBKEY_PENDING_TASK_1_1"));
        assert!(
            pubkey_is_placeholder("dW50cnVzdGVkIGNvbW1lbnQ6 not base64!"),
            "含非 base64 字元"
        );
        assert!(
            pubkey_is_placeholder("QUJDREVGRw=="),
            "合法 base64 但不是 minisign 公鑰檔"
        );
    }
}
