//! 排程紀錄檔 `fetch-state.json`（data-layer-rust tasks.md 5.2、design.md D10）。
//!
//! 位置：`%LOCALAPPDATA%\tw.fintools.fc-host\fetch-state.json`——宿主自己的資料夾，**不在資料目錄**，所以
//! 切換資料目錄不會重設 60 分鐘下限。內容：
//!
//! ```json
//! {"last_start":"2026-10-05T07:02:11Z","last_finish":"2026-10-05T07:02:52Z","last_any_success":true,"consecutive_failures":0}
//! ```
//!
//! - 時間一律寫成 UTC 的 RFC 3339（`Z` 結尾）；讀取時任何偏移都接受。
//! - **讀不到、不是 UTF-8、JSON 壞掉、欄位缺漏或型別不對、時間無法解析一律當作沒有紀錄**（[`load`]
//!   回 `None`），排程把它視同「錯過」（見 [`super::sched`] 模組文件）；不會因此 panic 或中止排程。
//!   `last_finish` 可以是 `null` 或缺漏（一輪從沒結束過）。
//! - 原子寫入：沿用 `settings::write_atomic`（寫同目錄暫存檔再改名）。只有排程執行緒寫這個檔。
//!
//! 紀錄只記「這個宿主上一輪的時間與成敗」，不含任何資料內容。

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::{OffsetDateTime, UtcOffset};

use super::sched::FetchState;

/// 排程紀錄的檔名。
pub const STATE_FILE: &str = "fetch-state.json";

/// 預設路徑：`%LOCALAPPDATA%\tw.fintools.fc-host\fetch-state.json`。環境變數不存在（極端情況）時退回目前
/// 工作目錄，不 panic（同 `settings::default_settings_path` 的慣例）。
pub fn default_state_path() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("tw.fintools.fc-host").join(STATE_FILE)
}

/// 檔案上的形狀（時間是字串，方便人讀與除錯）。
#[derive(Serialize, Deserialize)]
struct Raw {
    last_start: String,
    #[serde(default)]
    last_finish: Option<String>,
    last_any_success: bool,
    consecutive_failures: u32,
}

fn parse_time(s: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(s, &Rfc3339).ok()
}

fn format_time(t: OffsetDateTime) -> Option<String> {
    t.to_offset(UtcOffset::UTC).format(&Rfc3339).ok()
}

/// 解析檔案內容；任何不合格＝`None`。
pub fn parse(bytes: &[u8]) -> Option<FetchState> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let raw: Raw = serde_json::from_slice(bytes).ok()?;
    let last_start = parse_time(&raw.last_start)?;
    let last_finish = match raw.last_finish {
        None => None,
        Some(s) => Some(parse_time(&s)?),
    };
    Some(FetchState {
        last_start,
        last_finish,
        last_any_success: raw.last_any_success,
        consecutive_failures: raw.consecutive_failures,
    })
}

/// 產生檔案內容。
pub fn render(state: &FetchState) -> Option<String> {
    let raw = Raw {
        last_start: format_time(state.last_start)?,
        last_finish: match state.last_finish {
            None => None,
            Some(t) => Some(format_time(t)?),
        },
        last_any_success: state.last_any_success,
        consecutive_failures: state.consecutive_failures,
    };
    serde_json::to_string(&raw).ok()
}

/// 讀紀錄；讀不到或壞掉＝`None`。
pub fn load(path: &Path) -> Option<FetchState> {
    parse(&std::fs::read(path).ok()?)
}

/// 原子寫入紀錄。
pub fn save(path: &Path, state: &FetchState) -> io::Result<()> {
    let text = render(state)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "排程紀錄的時間無法格式化"))?;
    crate::settings::write_atomic(path, text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::test_util::TempDir;
    use time::macros::datetime;

    fn sample() -> FetchState {
        FetchState {
            last_start: datetime!(2026-10-05 15:02:11 +8),
            last_finish: Some(datetime!(2026-10-05 15:02:52 +8)),
            last_any_success: true,
            consecutive_failures: 0,
        }
    }

    #[test]
    fn render_writes_the_documented_shape_in_utc() {
        let text = render(&sample()).unwrap();
        assert_eq!(
            text,
            r#"{"last_start":"2026-10-05T07:02:11Z","last_finish":"2026-10-05T07:02:52Z","last_any_success":true,"consecutive_failures":0}"#
        );
    }

    #[test]
    fn round_trips_through_a_file_and_leaves_no_tmp() {
        let dir = TempDir::new("fetch-state-roundtrip");
        let path = dir.path().join("sub").join(STATE_FILE);
        save(&path, &sample()).unwrap();
        assert_eq!(load(&path), Some(sample()));
        let failed = FetchState {
            last_finish: None,
            last_any_success: false,
            consecutive_failures: 3,
            ..sample()
        };
        save(&path, &failed).unwrap();
        assert_eq!(load(&path), Some(failed));
        let tmp: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(tmp.is_empty(), "不該留下暫存檔");
    }

    #[test]
    fn accepts_any_offset_a_null_or_missing_finish_and_a_bom() {
        let s = parse(
            br#"{"last_start":"2026-10-05T15:02:11+08:00","last_finish":null,"last_any_success":false,"consecutive_failures":2}"#,
        )
        .unwrap();
        assert_eq!(s.last_start, datetime!(2026-10-05 07:02:11 UTC));
        assert_eq!(s.last_finish, None);
        assert_eq!((s.last_any_success, s.consecutive_failures), (false, 2));
        let s = parse(
            br#"{"last_start":"2026-10-05T07:02:11Z","last_any_success":true,"consecutive_failures":0}"#,
        )
        .unwrap();
        assert_eq!(s.last_finish, None);
        let mut with_bom = b"\xEF\xBB\xBF".to_vec();
        with_bom.extend_from_slice(
            br#"{"last_start":"2026-10-05T07:02:11Z","last_any_success":true,"consecutive_failures":0}"#,
        );
        assert!(parse(&with_bom).is_some());
    }

    #[test]
    fn missing_or_corrupt_records_read_as_none() {
        let good = r#"{"last_start":"2026-10-05T07:02:11Z","last_finish":null,"last_any_success":true,"consecutive_failures":0}"#;
        assert!(parse(good.as_bytes()).is_some());
        for (name, bad) in [
            ("空檔", ""),
            ("不是 JSON", "not json"),
            ("截斷", &good[..good.len() / 2]),
            ("頂層是陣列", "[]"),
            (
                "缺 last_start",
                r#"{"last_any_success":true,"consecutive_failures":0}"#,
            ),
            (
                "缺 last_any_success",
                r#"{"last_start":"2026-10-05T07:02:11Z","consecutive_failures":0}"#,
            ),
            (
                "缺 consecutive_failures",
                r#"{"last_start":"2026-10-05T07:02:11Z","last_any_success":true}"#,
            ),
            (
                "時間壞掉",
                r#"{"last_start":"yesterday","last_any_success":true,"consecutive_failures":0}"#,
            ),
            (
                "結束時間壞掉",
                r#"{"last_start":"2026-10-05T07:02:11Z","last_finish":"x","last_any_success":true,"consecutive_failures":0}"#,
            ),
            (
                "型別錯",
                r#"{"last_start":1,"last_any_success":true,"consecutive_failures":0}"#,
            ),
            (
                "失敗次數為負",
                r#"{"last_start":"2026-10-05T07:02:11Z","last_any_success":false,"consecutive_failures":-1}"#,
            ),
        ] {
            assert_eq!(parse(bad.as_bytes()), None, "{name}");
        }
        assert_eq!(parse(b"\xFF\xFE\x00garbage"), None, "非 UTF-8");
    }

    #[test]
    fn load_of_a_missing_or_corrupt_file_is_none_and_does_not_panic() {
        let dir = TempDir::new("fetch-state-corrupt");
        let path = dir.path().join(STATE_FILE);
        assert_eq!(load(&path), None);
        std::fs::write(&path, b"{").unwrap();
        assert_eq!(load(&path), None);
        // 損毀的檔案之後可以被正常覆寫。
        save(&path, &sample()).unwrap();
        assert_eq!(load(&path), Some(sample()));
    }

    #[test]
    fn default_path_lives_under_the_host_folder_not_the_data_dir() {
        let p = default_state_path();
        assert!(p.ends_with(std::path::Path::new("tw.fintools.fc-host").join(STATE_FILE)));
    }
}
