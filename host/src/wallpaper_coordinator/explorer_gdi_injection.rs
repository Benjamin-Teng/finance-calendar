//! explorer GDI 讀值注入（dynamic-wallpaper task 4.9；**只在 `self-test-ipc` 建置**，正式建置不含本
//! 模組）。供 tasks 6.1 實機驗收觸發安全閥：不必真的讓 explorer 累積兩千個 GDI 物件。
//!
//! ## 用法
//!
//! 啟動宿主前設環境變數 [`ENV_VAR`]＝一個文字檔的路徑。協調迴圈**每次**要讀 explorer GDI 時（即將
//! 設定桌布之前）都重讀這個檔，所以驗收腳本可以在宿主執行中改寫內容：
//!
//! | 檔案內容（前後空白忽略） | 讀值 |
//! |---|---|
//! | 檔案不存在、空白、`real` | 真正的讀值（不注入） |
//! | `<gdi>`，例如 `1500` | 真正的 explorer PID＋指定的 GDI 數 |
//! | `<pid>:<gdi>`，例如 `999:300` | 指定的 PID 與 GDI 數（模擬 explorer 重新啟動） |
//! | `+<n>`／`-<n>`，例如 `+2001` | 真正的讀值加減 n（下限 0） |
//! | `fail` 或 `fail:<說明>` | 讀取失敗 |
//!
//! 內容不認得＝讀取失敗（說明含原文）。每次注入都以協調迴圈的記錄 target 記一行
//! 「self-test 注入 explorer GDI」，證據檔可據此分辨注入值與真實值。

use crate::desktop::explorer_gdi::ExplorerGdiReading;
use crate::wallpaper::ExplorerSample;

use super::LOG_TARGET;

/// 指向注入檔的環境變數。
pub const ENV_VAR: &str = "FC_HOST_SELF_TEST_EXPLORER_GDI_FILE";

/// 注入檔的內容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Injection {
    /// 不注入（真正的讀值）。
    PassThrough,
    /// 讀取失敗。
    Fail(String),
    /// 真正的 PID＋指定的 GDI 數。
    Gdi(u32),
    /// 指定的 PID 與 GDI 數。
    PidGdi(u32, u32),
    /// 真正的讀值加減。
    Offset(i64),
}

/// 解析注入檔內容（規則見模組文件）。
pub fn parse(text: &str) -> Result<Injection, String> {
    let t = text.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("real") {
        return Ok(Injection::PassThrough);
    }
    if t.eq_ignore_ascii_case("fail") {
        return Ok(Injection::Fail("self-test 注入的讀取失敗".to_owned()));
    }
    if let Some(msg) = t.strip_prefix("fail:") {
        return Ok(Injection::Fail(format!(
            "self-test 注入的讀取失敗：{}",
            msg.trim()
        )));
    }
    let bad = || format!("self-test 注入檔內容不認得：{t:?}");
    if t.starts_with('+') || t.starts_with('-') {
        return t.parse::<i64>().map(Injection::Offset).map_err(|_| bad());
    }
    if let Some((pid, gdi)) = t.split_once(':') {
        let pid = pid.trim().parse::<u32>().map_err(|_| bad())?;
        let gdi = gdi.trim().parse::<u32>().map_err(|_| bad())?;
        return Ok(Injection::PidGdi(pid, gdi));
    }
    t.parse::<u32>().map(Injection::Gdi).map_err(|_| bad())
}

/// 依注入內容產生讀值；需要真正的讀值時才呼叫 `real`。`PassThrough` 回 `None`（呼叫端走正式路徑）。
pub fn apply(
    injection: &Injection,
    real: impl FnOnce() -> Result<ExplorerGdiReading, String>,
) -> Option<Result<ExplorerSample, String>> {
    let to_sample = |r: ExplorerGdiReading| ExplorerSample {
        pid: r.pid,
        gdi: r.gdi,
    };
    Some(match injection {
        Injection::PassThrough => return None,
        Injection::Fail(msg) => Err(msg.clone()),
        Injection::PidGdi(pid, gdi) => Ok(ExplorerSample {
            pid: *pid,
            gdi: *gdi,
        }),
        Injection::Gdi(gdi) => real().map(|r| ExplorerSample {
            pid: r.pid,
            gdi: *gdi,
        }),
        Injection::Offset(delta) => real().map(|r| {
            let gdi = (i64::from(r.gdi) + delta).clamp(0, i64::from(u32::MAX));
            ExplorerSample {
                gdi: u32::try_from(gdi).unwrap_or(u32::MAX),
                ..to_sample(r)
            }
        }),
    })
}

/// 有設 [`ENV_VAR`] 且注入檔要求注入時回傳注入的讀值；否則 `None`（呼叫端走正式路徑）。
pub fn read(
    real: impl FnOnce() -> Result<ExplorerGdiReading, String>,
) -> Option<Result<ExplorerSample, String>> {
    let path = std::env::var_os(ENV_VAR)?;
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            return Some(Err(format!(
                "self-test 注入檔 {} 讀不到：{e}",
                std::path::Path::new(&path).display()
            )))
        }
    };
    let result = match parse(&text) {
        Ok(injection) => apply(&injection, real)?,
        Err(e) => Err(e),
    };
    log::info!(
        target: LOG_TARGET,
        "self-test 注入 explorer GDI：注入檔內容 {:?} → {result:?}",
        text.trim()
    );
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn real_ok() -> Result<ExplorerGdiReading, String> {
        Ok(ExplorerGdiReading {
            pid: 4242,
            gdi: 600,
        })
    }

    #[test]
    fn parses_every_documented_form() {
        assert_eq!(parse(""), Ok(Injection::PassThrough));
        assert_eq!(parse("  real \r\n"), Ok(Injection::PassThrough));
        assert!(matches!(parse("fail"), Ok(Injection::Fail(_))));
        assert!(matches!(parse("fail: 模擬"), Ok(Injection::Fail(m)) if m.contains("模擬")));
        assert_eq!(parse("1500\n"), Ok(Injection::Gdi(1500)));
        assert_eq!(parse("999:300"), Ok(Injection::PidGdi(999, 300)));
        assert_eq!(parse("+2001"), Ok(Injection::Offset(2001)));
        assert_eq!(parse("-50"), Ok(Injection::Offset(-50)));
        for bad in ["abc", "1:2:3", "+x", "-", "99999999999"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn applies_injection_on_top_of_the_real_reading() {
        assert_eq!(apply(&Injection::PassThrough, real_ok), None);
        assert_eq!(
            apply(&Injection::Gdi(3000), real_ok),
            Some(Ok(ExplorerSample {
                pid: 4242,
                gdi: 3000
            }))
        );
        assert_eq!(
            apply(&Injection::PidGdi(7, 8), || panic!("不需要真正的讀值")),
            Some(Ok(ExplorerSample { pid: 7, gdi: 8 }))
        );
        assert_eq!(
            apply(&Injection::Offset(2001), real_ok),
            Some(Ok(ExplorerSample {
                pid: 4242,
                gdi: 2601
            }))
        );
        assert_eq!(
            apply(&Injection::Offset(-9999), real_ok),
            Some(Ok(ExplorerSample { pid: 4242, gdi: 0 }))
        );
        assert!(matches!(
            apply(&Injection::Fail("x".into()), real_ok),
            Some(Err(_))
        ));
        assert_eq!(
            apply(&Injection::Gdi(1), || Err("真的讀不到".into())),
            Some(Err("真的讀不到".into()))
        );
    }
}
