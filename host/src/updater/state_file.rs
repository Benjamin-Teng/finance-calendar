//! `update-state.json`：自動安裝的嘗試紀錄（design.md D3「防止自動安裝循環」）。
//!
//! 位置 `%LOCALAPPDATA%\tw.fintools.fc-host\update-state.json`。格式與讀寫在 task 3.1 建立；**何時寫入**
//! （自動／手動安裝前記下「嘗試的版本與時間」）與據此退避的決策接線在 task 3.2。規則：啟動後若目前版本
//! 仍低於上次嘗試的版本、候選版本不高於它（同一版本）、且距該次嘗試未滿 24 小時，只通知、不自動安裝，並記錄
//! 一筆錯誤；出現更新的版本時照常自動安裝；使用者手動點選不受此限。當機迴圈保護的同步安裝同樣受此限。

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::version;

pub const UPDATE_STATE_FILE_NAME: &str = "update-state.json";
/// 自動安裝退避的時間窗（design.md D3：24 小時）。
pub const AUTO_INSTALL_BACKOFF_SECS: i64 = 24 * 60 * 60;
const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateState {
    /// 檔案格式版本（目前 1）。
    #[serde(default = "default_format_version")]
    pub format: u32,
    /// 上次嘗試安裝的版本（`latest.json` 宣告者）。
    pub attempted_version: String,
    /// 上次嘗試的時間（Unix 秒）。
    pub attempted_at_unix: i64,
}

fn default_format_version() -> u32 {
    FORMAT_VERSION
}

impl UpdateState {
    pub fn new(attempted_version: &str, attempted_at_unix: i64) -> Self {
        Self {
            format: FORMAT_VERSION,
            attempted_version: attempted_version.to_owned(),
            attempted_at_unix,
        }
    }

    /// 自動安裝是否該退避：目前版本仍低於上次嘗試的版本、候選版本**不高於**上次嘗試的版本（同一版本；出現更新的版本
    /// 照常安裝，spec「24 小時內不再自動安裝同一版本」），且距該次嘗試未滿 24 小時。
    /// 時鐘倒退（`now` 早於嘗試時間）視同「未滿 24 小時」。
    pub fn backs_off_auto_install(
        &self,
        current_version: &str,
        candidate_version: &str,
        now_unix: i64,
    ) -> bool {
        let still_older = version::is_newer(current_version, &self.attempted_version);
        let same_or_older_candidate =
            !version::is_newer(&self.attempted_version, candidate_version);
        still_older
            && same_or_older_candidate
            && now_unix.saturating_sub(self.attempted_at_unix) < AUTO_INSTALL_BACKOFF_SECS
    }
}

/// 讀取。檔案不存在回 `None`；損壞（無法解析）記錄警告後也回 `None`——退避紀錄遺失只代表可能多試一次，
/// 不得因此讓宿主出錯。
pub fn read(dir: &Path) -> Option<UpdateState> {
    let path = dir.join(UPDATE_STATE_FILE_NAME);
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
        Err(e) => {
            log::warn!(target: super::LOG_TARGET, "update-state.json：讀取 {} 失敗：{e}", path.display());
            return None;
        }
    };
    match serde_json::from_str(&text) {
        Ok(state) => Some(state),
        Err(e) => {
            log::warn!(target: super::LOG_TARGET,
                "update-state.json：{} 內容無法解析（{e}），視為沒有紀錄",
                path.display()
            );
            None
        }
    }
}

/// 原子寫入（`.tmp` → rename，不留半份檔）。
pub fn write(dir: &Path, state: &UpdateState) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let path = dir.join(UPDATE_STATE_FILE_NAME);
    let tmp = dir.join(format!("{UPDATE_STATE_FILE_NAME}.tmp"));
    let json = serde_json::to_string_pretty(state).map_err(io::Error::other)?;
    fs::write(&tmp, json)?;
    fs::rename(&tmp, &path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::test_util::TempDir;

    #[test]
    fn round_trip() {
        let dir = TempDir::new("state-roundtrip");
        assert_eq!(read(dir.path()), None, "沒檔案＝沒有紀錄");
        let state = UpdateState::new("0.1.2", 1_700_000_000);
        write(dir.path(), &state).unwrap();
        assert_eq!(read(dir.path()), Some(state));
        assert!(
            !dir.path().join("update-state.json.tmp").exists(),
            "原子寫入不留暫存檔"
        );
    }

    #[test]
    fn corrupt_file_reads_as_none_without_panicking() {
        let dir = TempDir::new("state-corrupt");
        fs::write(dir.path().join(UPDATE_STATE_FILE_NAME), "{not json").unwrap();
        assert_eq!(read(dir.path()), None);
    }

    #[test]
    fn missing_format_field_defaults_to_current() {
        let dir = TempDir::new("state-noformat");
        fs::write(
            dir.path().join(UPDATE_STATE_FILE_NAME),
            r#"{"attempted_version":"0.1.2","attempted_at_unix":5}"#,
        )
        .unwrap();
        assert_eq!(read(dir.path()), Some(UpdateState::new("0.1.2", 5)));
    }

    #[test]
    fn backoff_rule() {
        let state = UpdateState::new("0.1.2", 1_000);
        let day = AUTO_INSTALL_BACKOFF_SECS;
        // spec「自動安裝沒有生效」：重新啟動的仍是 0.1.1，24 小時內不再自動安裝 0.1.2。
        assert!(state.backs_off_auto_install("0.1.1", "0.1.2", 1_000 + 60));
        assert!(state.backs_off_auto_install("0.1.1", "0.1.2", 1_000 + day - 1));
        assert!(
            !state.backs_off_auto_install("0.1.1", "0.1.2", 1_000 + day),
            "滿 24 小時可再試"
        );
        // 目前版本已追上：不退避。
        assert!(!state.backs_off_auto_install("0.1.2", "0.1.2", 1_060));
        assert!(!state.backs_off_auto_install("0.2.0", "0.2.0", 1_060));
        // 更新的版本（0.1.3）不受 0.1.2 的退避影響；同版本與更舊的候選仍退避。
        assert!(!state.backs_off_auto_install("0.1.1", "0.1.3", 1_000 + 60));
        assert!(state.backs_off_auto_install("0.1.1", "0.1.2", 1_000 + 60));
        assert!(state.backs_off_auto_install("0.1.1", "0.1.1", 1_000 + 60));
        // 時鐘倒退：視同未滿。
        assert!(state.backs_off_auto_install("0.1.1", "0.1.2", 10));
    }
}
