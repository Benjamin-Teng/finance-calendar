//! 勿打擾讀值注入（dynamic-wallpaper task 4.7c；**只在 `self-test-ipc` 建置**，正式建置不含本模組）。
//! 供 tasks 6.1 實機驗收：不必真的切換使用者的勿打擾或專注設定（本專案不得自動化 Windows 設定）。
//! 作法比照 4.9 的 `FC_HOST_SELF_TEST_EXPLORER_GDI_FILE`。
//!
//! ## 用法
//!
//! 啟動宿主前設環境變數：
//!
//! - [`ENV_VAR`]＝一個文字檔的路徑。輪詢執行緒**每次**讀值都重讀這個檔，驗收腳本可在宿主執行中改寫。
//! - [`POLL_ENV_VAR`]＝輪詢間隔秒數（1–3600，選用；預設 60 秒）。驗收不必每步等一分鐘。讀值時效（上層模組契約 7）
//!   跟著縮成 3 個間隔。
//!
//! 檔案內容是以空白、逗號或換行分隔的 `鍵=值`（大小寫不拘），沒寫的鍵＝真正的讀值：
//!
//! | 鍵 | 值 | 讀值 |
//! |---|---|---|
//! | `focus` | `on`／`off` | `IsFocusActive` 為真／假 |
//! | `focus` | `unsupported` | `IsSupported` 為假（未知） |
//! | `focus` | `fail` | 讀取失敗（未知） |
//! | `wnf` | 整數，例如 `0`、`1`、`2`、`-1` | 那個 4 位元組整數（> 0＝勿打擾） |
//! | `wnf` | `short` | 資料只有 2 位元組（未知） |
//! | `wnf` | `fail` | NTSTATUS 失敗（未知） |
//! | 任一 | `real` | 真正的讀值 |
//!
//! 檔案不存在、空白或只寫 `real`＝完全不注入。內容不認得＝兩個來源都當讀取失敗（說明含原文）。
//! 注入值一律經 `interpret_focus`／`interpret_wnf` 判讀（與真正的讀值走同一段程式），每次注入都以
//! 協調迴圈的記錄 target 記一行「self-test 注入勿打擾讀值」，證據檔可據此分辨注入值與真實值。

use std::time::Duration;

use log::Level;

use crate::desktop::dnd::WnfRaw;
use crate::wallpaper_coordinator::LogSink;

use super::{interpret_focus, interpret_wnf, DndSources, SourceSample};

/// 指向注入檔的環境變數。
pub const ENV_VAR: &str = "FC_HOST_SELF_TEST_DND_FILE";

/// 輪詢間隔（秒）的環境變數。
pub const POLL_ENV_VAR: &str = "FC_HOST_SELF_TEST_DND_POLL_SECS";

/// STATUS_UNSUCCESSFUL（注入 `wnf=fail` 時用的 NTSTATUS）。
const STATUS_UNSUCCESSFUL: i32 = 0xC000_0001_u32 as i32;

/// 官方半邊的注入。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusInjection {
    On,
    Off,
    Unsupported,
    Fail,
}

/// WNF 半邊的注入。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WnfInjection {
    Value(i32),
    Short,
    Fail,
}

/// 注入檔的內容（`None`＝該半邊用真正的讀值）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Injection {
    pub focus: Option<FocusInjection>,
    pub wnf: Option<WnfInjection>,
}

/// 解析注入檔內容（規則見模組文件）。
pub fn parse(text: &str) -> Result<Injection, String> {
    let mut out = Injection::default();
    let bad = |token: &str| format!("self-test 勿打擾注入檔內容不認得：{token:?}（全文 {text:?}）");
    for token in text
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|t| !t.is_empty())
    {
        if token.eq_ignore_ascii_case("real") {
            continue;
        }
        let (key, value) = token.split_once('=').ok_or_else(|| bad(token))?;
        let value = value.trim().to_ascii_lowercase();
        match key.trim().to_ascii_lowercase().as_str() {
            "focus" => {
                out.focus = match value.as_str() {
                    "real" => None,
                    "on" => Some(FocusInjection::On),
                    "off" => Some(FocusInjection::Off),
                    "unsupported" => Some(FocusInjection::Unsupported),
                    "fail" => Some(FocusInjection::Fail),
                    _ => return Err(bad(token)),
                }
            }
            "wnf" => {
                out.wnf = match value.as_str() {
                    "real" => None,
                    "short" => Some(WnfInjection::Short),
                    "fail" => Some(WnfInjection::Fail),
                    v => Some(WnfInjection::Value(
                        v.parse::<i32>().map_err(|_| bad(token))?,
                    )),
                }
            }
            _ => return Err(bad(token)),
        }
    }
    Ok(out)
}

/// 注入的官方讀值（經正式判讀）。
pub fn focus_sample(injection: FocusInjection) -> SourceSample {
    let mut sample = interpret_focus(match injection {
        FocusInjection::On => Ok(Some(true)),
        FocusInjection::Off => Ok(Some(false)),
        FocusInjection::Unsupported => Ok(None),
        FocusInjection::Fail => Err("self-test 注入的讀取失敗".to_owned()),
    });
    sample.detail = format!("{}（self-test 注入）", sample.detail);
    sample
}

/// 注入的 WNF 讀值（經正式判讀）。
pub fn wnf_sample(injection: WnfInjection) -> SourceSample {
    let raw = match injection {
        WnfInjection::Value(v) => WnfRaw {
            status: 0,
            size: 4,
            data: v.to_le_bytes(),
        },
        WnfInjection::Short => WnfRaw {
            status: 0,
            size: 2,
            data: [1, 0, 0, 0],
        },
        WnfInjection::Fail => WnfRaw {
            status: STATUS_UNSUCCESSFUL,
            size: 4,
            data: [0; 4],
        },
    };
    let mut sample = interpret_wnf(Ok(raw));
    sample.detail = format!("{}（self-test 注入）", sample.detail);
    sample
}

/// [`POLL_ENV_VAR`] 指定的輪詢間隔（沒設、不是 1–3600 的整數＝`None`，照預設）。
pub fn poll_interval_override() -> Option<Duration> {
    let secs = std::env::var(POLL_ENV_VAR)
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()?;
    (1..=3600)
        .contains(&secs)
        .then(|| Duration::from_secs(secs))
}

/// 包住真正的來源：讀官方半邊時重讀注入檔，讀 WNF 時沿用同一次的內容（輪詢器一律先讀官方）。
pub struct InjectingSources<S> {
    real: S,
    log: LogSink,
    current: Result<Injection, String>,
}

impl<S: DndSources> InjectingSources<S> {
    pub fn new(real: S, log: LogSink) -> Self {
        Self {
            real,
            log,
            current: Ok(Injection::default()),
        }
    }

    fn reload(&mut self) {
        self.current = match std::env::var_os(ENV_VAR) {
            None => Ok(Injection::default()),
            Some(path) => match std::fs::read_to_string(&path) {
                Ok(text) => parse(&text),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Injection::default()),
                Err(e) => Err(format!(
                    "self-test 勿打擾注入檔 {} 讀不到：{e}",
                    std::path::Path::new(&path).display()
                )),
            },
        };
    }

    fn note(&self, half: &str, sample: &SourceSample) {
        (self.log)(
            Level::Info,
            &format!(
                "self-test 注入勿打擾讀值（{half}）：{:?} {}",
                sample.tri, sample.detail
            ),
        );
    }
}

impl<S: DndSources> DndSources for InjectingSources<S> {
    fn read_focus(&mut self) -> SourceSample {
        self.reload();
        let sample = match &self.current {
            Err(e) => SourceSample::unknown(e.clone()),
            Ok(Injection { focus: None, .. }) => return self.real.read_focus(),
            Ok(Injection { focus: Some(f), .. }) => focus_sample(*f),
        };
        self.note("官方", &sample);
        sample
    }

    fn read_wnf(&mut self) -> SourceSample {
        let sample = match &self.current {
            Err(e) => SourceSample::unknown(e.clone()),
            Ok(Injection { wnf: None, .. }) => return self.real.read_wnf(),
            Ok(Injection { wnf: Some(w), .. }) => wnf_sample(*w),
        };
        self.note("WNF", &sample);
        sample
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wallpaper_dnd::Tri;

    #[test]
    fn parses_keys_values_and_passthrough() {
        assert_eq!(parse("").unwrap(), Injection::default());
        assert_eq!(parse("  real \n").unwrap(), Injection::default());
        assert_eq!(
            parse("focus=on, wnf=2").unwrap(),
            Injection {
                focus: Some(FocusInjection::On),
                wnf: Some(WnfInjection::Value(2)),
            }
        );
        assert_eq!(
            parse("WNF=-1\nFocus=Unsupported").unwrap(),
            Injection {
                focus: Some(FocusInjection::Unsupported),
                wnf: Some(WnfInjection::Value(-1)),
            }
        );
        assert_eq!(
            parse("focus=real wnf=short").unwrap().wnf,
            Some(WnfInjection::Short)
        );
        assert_eq!(parse("wnf=fail").unwrap().wnf, Some(WnfInjection::Fail));
        assert!(parse("wnf=abc").is_err());
        assert!(parse("dnd=on").is_err());
        assert!(parse("on").is_err());
    }

    #[test]
    fn injected_values_go_through_the_real_interpretation() {
        assert_eq!(focus_sample(FocusInjection::On).tri, Tri::On);
        assert_eq!(focus_sample(FocusInjection::Off).tri, Tri::Off);
        assert_eq!(focus_sample(FocusInjection::Unsupported).tri, Tri::Unknown);
        assert_eq!(focus_sample(FocusInjection::Fail).tri, Tri::Unknown);
        assert_eq!(wnf_sample(WnfInjection::Value(1)).tri, Tri::On);
        assert_eq!(wnf_sample(WnfInjection::Value(0)).tri, Tri::Off);
        assert_eq!(wnf_sample(WnfInjection::Value(-5)).tri, Tri::Off);
        assert_eq!(wnf_sample(WnfInjection::Short).tri, Tri::Unknown);
        assert_eq!(wnf_sample(WnfInjection::Fail).tri, Tri::Unknown);
        assert!(wnf_sample(WnfInjection::Value(1))
            .detail
            .contains("self-test 注入"));
    }
}
