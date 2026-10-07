//! 當機迴圈保護的啟動標記（design.md D3「當機迴圈保護」）。
//!
//! 標記檔 `%LOCALAPPDATA%\tw.fintools.fc-host\startup-marker`（內容為版本與時間）的生命週期：
//!
//! ```text
//! 主行程 begin() ──寫入──▶ Armed ──5 分鐘到期──▶ Expired（檔案刪除）
//!                           │  ▲
//!        工作階段結束 ──────┘  └── 工作階段結束被取消（5 分鐘內）──▶ 重新寫入
//!                           ▼
//!                       Suspended（檔案刪除）
//!   任何狀態 ──正常結束（系統匣「結束」、事件迴圈結束、過渡期結束）──▶ Cleared（檔案刪除）
//! ```
//!
//! 只有當機（panic、abort、原生當機、被強制終止）會讓檔案留到下一次啟動——此時
//! [`BeginOutcome::previous`] 為 [`PreviousMarker::Present`]。
//!
//! **只由主行程讀寫**：[`StartupMarker::begin`] 在 `role != Primary` 時**完全不做任何檔案存取**
//! （Secondary、仲裁機制故障的 `Unarbitrated` 都在此列），否則正常執行的前 5 分鐘標記本來就在，任何
//! 重複啟動都會被誤判成當機。`--restore-wallpaper`／`--fetch-once` 在 `main()` 取得仲裁之前就結束，
//! 根本不會呼叫到這裡。路徑與時間都由呼叫端注入，測試用暫存資料夾。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::cancel::{CancelToken, Timer};
use crate::desktop::StartupRole;

/// 標記檔名（位於 `%LOCALAPPDATA%\tw.fintools.fc-host\`）。
pub const MARKER_FILE_NAME: &str = "startup-marker";
/// 標記存活多久後刪除（design.md D3：5 分鐘）。
pub const MARKER_LIFETIME: Duration = Duration::from_secs(5 * 60);

/// 舊標記的內容（只用於記錄；是否為「當機」只看標記存在與否）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkerInfo {
    pub version: Option<String>,
    pub unix_secs: Option<i64>,
}

/// 啟動時讀到的舊標記狀態。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviousMarker {
    /// 不是主行程：沒有讀（也沒有寫）。
    NotChecked,
    /// 沒有舊標記：上次正常結束，或根本沒有上次。
    Absent,
    /// 舊標記仍在：上次啟動不到 5 分鐘就非正常結束。
    Present(MarkerInfo),
}

impl PreviousMarker {
    /// 是否為「上次非正常結束」。
    pub fn is_stale(&self) -> bool {
        matches!(self, Self::Present(_))
    }
}

/// [`StartupMarker::begin`] 的結果。
pub struct BeginOutcome {
    /// 主行程才有；Secondary 等為 `None`。
    pub marker: Option<Arc<StartupMarker>>,
    pub previous: PreviousMarker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerState {
    Armed,
    Suspended,
    Expired,
    Cleared,
}

/// 本行程寫下的標記。
pub struct StartupMarker {
    path: PathBuf,
    version: String,
    state: Mutex<MarkerState>,
    expiry_cancel: CancelToken,
}

impl StartupMarker {
    /// 主行程啟動時呼叫（仲裁成 Primary 之後、`tauri::Builder` 之前）：讀舊標記、寫新標記。
    /// `role` 不是 [`StartupRole::Primary`] 時**不碰檔案**。
    pub fn begin(dir: &Path, role: StartupRole, version: &str, now_unix: i64) -> BeginOutcome {
        if role != StartupRole::Primary {
            return BeginOutcome {
                marker: None,
                previous: PreviousMarker::NotChecked,
            };
        }
        let path = dir.join(MARKER_FILE_NAME);
        let previous = match fs::read_to_string(&path) {
            Ok(text) => PreviousMarker::Present(parse_marker(&text)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => PreviousMarker::Absent,
            // 存在但讀不出來（非 UTF-8 等）：檔案在＝上次沒有正常刪除，仍視為舊標記；
            // 讀取本身被拒（權限）則無從得知，保守視為沒有。
            Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                PreviousMarker::Present(MarkerInfo {
                    version: None,
                    unix_secs: None,
                })
            }
            Err(e) => {
                log::warn!(target: super::LOG_TARGET,
                    "啟動標記：讀取 {} 失敗（{e}），視為沒有舊標記",
                    path.display()
                );
                PreviousMarker::Absent
            }
        };
        let marker = Arc::new(Self {
            path,
            version: version.to_owned(),
            state: Mutex::new(MarkerState::Armed),
            expiry_cancel: CancelToken::new(),
        });
        if let Err(e) = marker.write_file(now_unix) {
            log::warn!(target: super::LOG_TARGET,
                "啟動標記：寫入 {} 失敗（{e}），本次啟動沒有當機迴圈保護",
                marker.path.display()
            );
        }
        BeginOutcome {
            marker: Some(marker),
            previous,
        }
    }

    fn write_file(&self, now_unix: i64) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(
            &self.path,
            format!("version={}\nunix={now_unix}\n", self.version),
        )
    }

    fn remove_file(&self) {
        match fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => {
                log::warn!(target: super::LOG_TARGET, "啟動標記：刪除 {} 失敗：{e}", self.path.display())
            }
        }
    }

    #[cfg(test)]
    pub fn state(&self) -> MarkerState {
        *self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 啟動「存活 `lifetime` 後刪除」的計時執行緒。`Cleared` 會取消它。
    ///
    /// `lifetime` 以**清醒時間**計（相對逾時，系統睡眠不計入）是正確語意：標記刪除代表「穩定執行滿 5 分鐘」，
    /// 啟動後馬上睡眠、醒來不久就當機，仍應被當成當機迴圈。
    pub fn spawn_expiry(
        self: &Arc<Self>,
        timer: Arc<dyn Timer>,
        lifetime: Duration,
    ) -> io::Result<JoinHandle<()>> {
        let me = Arc::clone(self);
        thread::Builder::new()
            .name("fc-startup-marker".to_owned())
            .spawn(move || {
                if !timer.sleep(lifetime, &me.expiry_cancel) {
                    me.expire();
                }
            })
    }

    /// 存活夠久：刪除標記，之後不再恢復。
    pub fn expire(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(*state, MarkerState::Armed | MarkerState::Suspended) {
            self.remove_file();
            *state = MarkerState::Expired;
            log::info!(target: super::LOG_TARGET,
                "啟動標記：存活已滿 {} 分鐘，已刪除",
                MARKER_LIFETIME.as_secs() / 60
            );
        }
    }

    /// 正常結束路徑（系統匣「結束」、事件迴圈結束、過渡期結束、之後 3.3 的「因更新結束」）：刪除標記並
    /// 停止到期計時。冪等。
    pub fn clear(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if *state != MarkerState::Cleared {
            self.remove_file();
            *state = MarkerState::Cleared;
            self.expiry_cancel.cancel();
            log::info!(target: super::LOG_TARGET, "啟動標記：正常結束，已刪除");
        }
    }

    /// 工作階段結束（`WM_QUERYENDSESSION`／`WM_ENDSESSION`）：先刪除標記，但保留「被取消就恢復」的可能
    /// （QUERYENDSESSION 之後系統可能取消登出／關機）。到期計時照舊。
    pub fn suspend_for_session_end(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if *state == MarkerState::Armed {
            self.remove_file();
            *state = MarkerState::Suspended;
            log::info!(target: super::LOG_TARGET, "啟動標記：工作階段結束，已刪除（若被取消會在 5 分鐘內恢復）");
        }
    }

    /// 工作階段結束被取消：還在 5 分鐘內（尚未到期）就重新寫入標記。
    pub fn resume_after_session_end_cancelled(&self, now_unix: i64) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if *state == MarkerState::Suspended {
            match self.write_file(now_unix) {
                Ok(()) => {
                    *state = MarkerState::Armed;
                    log::info!(target: super::LOG_TARGET, "啟動標記：工作階段結束被取消，已恢復");
                }
                Err(e) => log::warn!(target: super::LOG_TARGET, "啟動標記：恢復失敗：{e}"),
            }
        }
    }
}

fn parse_marker(text: &str) -> MarkerInfo {
    let mut info = MarkerInfo {
        version: None,
        unix_secs: None,
    };
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("version=") {
            info.version = Some(v.trim().to_owned());
        } else if let Some(t) = line.strip_prefix("unix=") {
            info.unix_secs = t.trim().parse().ok();
        }
    }
    info
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::test_util::TempDir;

    /// 立即返回的假計時器：記錄要求睡多久；`fires` 決定是否「睡滿」（false＝回報被取消）。
    struct FakeTimer {
        fires: bool,
        requested: Mutex<Vec<Duration>>,
    }

    impl Timer for FakeTimer {
        fn sleep(&self, duration: Duration, _cancel: &CancelToken) -> bool {
            self.requested.lock().unwrap().push(duration);
            !self.fires
        }
    }

    fn marker_path(dir: &TempDir) -> PathBuf {
        dir.path().join(MARKER_FILE_NAME)
    }

    #[test]
    fn primary_without_old_marker_writes_a_fresh_one() {
        let dir = TempDir::new("marker-fresh");
        let out = StartupMarker::begin(dir.path(), StartupRole::Primary, "0.1.0", 1_000);
        assert_eq!(out.previous, PreviousMarker::Absent);
        assert!(!out.previous.is_stale());
        let text = fs::read_to_string(marker_path(&dir)).expect("標記應已寫入");
        assert_eq!(text, "version=0.1.0\nunix=1000\n");
        assert_eq!(
            out.marker.expect("主行程有標記把手").state(),
            MarkerState::Armed
        );
    }

    #[test]
    fn primary_reads_stale_marker_then_overwrites_it() {
        let dir = TempDir::new("marker-stale");
        fs::write(marker_path(&dir), "version=0.0.9\nunix=500\n").unwrap();
        let out = StartupMarker::begin(dir.path(), StartupRole::Primary, "0.1.0", 2_000);
        assert_eq!(
            out.previous,
            PreviousMarker::Present(MarkerInfo {
                version: Some("0.0.9".into()),
                unix_secs: Some(500)
            })
        );
        assert!(out.previous.is_stale());
        assert_eq!(
            fs::read_to_string(marker_path(&dir)).unwrap(),
            "version=0.1.0\nunix=2000\n",
            "讀完舊標記後寫入新標記"
        );
    }

    #[test]
    fn unreadable_or_garbage_marker_still_counts_as_stale() {
        let dir = TempDir::new("marker-corrupt");
        // 非 UTF-8：檔案在，只是內容讀不出來——仍是舊標記。
        fs::write(marker_path(&dir), [0xff, 0xfe, 0x00]).unwrap();
        let out = StartupMarker::begin(dir.path(), StartupRole::Primary, "0.1.0", 1);
        assert!(out.previous.is_stale(), "非 UTF-8 內容＝上次沒有正常刪除");
        fs::write(marker_path(&dir), "garbage").unwrap();
        let out = StartupMarker::begin(dir.path(), StartupRole::Primary, "0.1.0", 1);
        assert_eq!(
            out.previous,
            PreviousMarker::Present(MarkerInfo {
                version: None,
                unix_secs: None
            }),
            "內容無法解析但檔案存在＝仍是舊標記"
        );
    }

    #[test]
    fn secondary_and_unarbitrated_never_touch_the_marker() {
        // spec「剛啟動就再點一次」：第二個行程不得讀、也不得寫、更不得刪標記。
        for role in [StartupRole::Secondary, StartupRole::Unarbitrated] {
            let dir = TempDir::new("marker-secondary");
            let before = "version=0.1.0\nunix=42\n";
            fs::write(marker_path(&dir), before).unwrap();
            let out = StartupMarker::begin(dir.path(), role, "9.9.9", 999);
            assert_eq!(
                out.previous,
                PreviousMarker::NotChecked,
                "{role:?} 不得讀標記"
            );
            assert!(!out.previous.is_stale(), "{role:?} 不得觸發同步更新檢查");
            assert!(
                out.marker.is_none(),
                "{role:?} 沒有標記把手＝之後也無從刪除"
            );
            assert_eq!(
                fs::read_to_string(marker_path(&dir)).unwrap(),
                before,
                "{role:?} 不得改寫標記"
            );
        }
        // 標記不存在時也不得被建立。
        let dir = TempDir::new("marker-secondary-absent");
        let _ = StartupMarker::begin(dir.path(), StartupRole::Secondary, "0.1.0", 1);
        assert!(!marker_path(&dir).exists(), "Secondary 不得建立標記檔");
    }

    #[test]
    fn clear_removes_the_marker_and_is_idempotent() {
        let dir = TempDir::new("marker-clear");
        let marker = StartupMarker::begin(dir.path(), StartupRole::Primary, "0.1.0", 1)
            .marker
            .unwrap();
        assert!(marker_path(&dir).exists());
        marker.clear();
        assert!(!marker_path(&dir).exists(), "正常結束要刪標記");
        assert_eq!(marker.state(), MarkerState::Cleared);
        marker.clear();
        // 之後的啟動讀不到舊標記。
        let next = StartupMarker::begin(dir.path(), StartupRole::Primary, "0.1.0", 2);
        assert_eq!(next.previous, PreviousMarker::Absent);
    }

    #[test]
    fn expiry_deletes_after_five_minutes_and_cancel_prevents_it() {
        let dir = TempDir::new("marker-expiry");
        let marker = StartupMarker::begin(dir.path(), StartupRole::Primary, "0.1.0", 1)
            .marker
            .unwrap();
        let timer = Arc::new(FakeTimer {
            fires: true,
            requested: Mutex::new(Vec::new()),
        });
        marker
            .spawn_expiry(timer.clone(), MARKER_LIFETIME)
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(
            *timer.requested.lock().unwrap(),
            vec![Duration::from_secs(300)]
        );
        assert!(!marker_path(&dir).exists(), "存活 5 分鐘後刪除");
        assert_eq!(marker.state(), MarkerState::Expired);

        // 被取消（正常結束）時，到期執行緒不得再動作。
        let dir2 = TempDir::new("marker-expiry-cancel");
        let marker2 = StartupMarker::begin(dir2.path(), StartupRole::Primary, "0.1.0", 1)
            .marker
            .unwrap();
        let cancelled = Arc::new(FakeTimer {
            fires: false,
            requested: Mutex::new(Vec::new()),
        });
        marker2
            .spawn_expiry(cancelled, MARKER_LIFETIME)
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(
            marker2.state(),
            MarkerState::Armed,
            "被取消的計時不得觸發到期"
        );
    }

    #[test]
    fn session_end_removes_marker_and_cancel_restores_it_within_lifetime() {
        let dir = TempDir::new("marker-session");
        let marker = StartupMarker::begin(dir.path(), StartupRole::Primary, "0.1.0", 1)
            .marker
            .unwrap();
        marker.suspend_for_session_end();
        assert!(!marker_path(&dir).exists(), "工作階段結束要刪標記");
        marker.resume_after_session_end_cancelled(77);
        assert_eq!(
            fs::read_to_string(marker_path(&dir)).unwrap(),
            "version=0.1.0\nunix=77\n",
            "登出被取消、仍在 5 分鐘內：恢復保護"
        );
        // 已到期後被取消不得恢復。
        marker.suspend_for_session_end();
        marker.expire();
        marker.resume_after_session_end_cancelled(88);
        assert!(!marker_path(&dir).exists(), "到期之後不再恢復");
        assert_eq!(marker.state(), MarkerState::Expired);
    }

    #[test]
    fn state_transitions_after_clear_are_inert() {
        let dir = TempDir::new("marker-inert");
        let marker = StartupMarker::begin(dir.path(), StartupRole::Primary, "0.1.0", 1)
            .marker
            .unwrap();
        marker.clear();
        marker.suspend_for_session_end();
        marker.resume_after_session_end_cancelled(5);
        marker.expire();
        assert!(!marker_path(&dir).exists());
        assert_eq!(marker.state(), MarkerState::Cleared);
    }
}
