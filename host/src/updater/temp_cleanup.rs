//! 更新暫存安裝檔清理（installer-auto-update 4.x 實機發現 A4）。
//!
//! `tauri-plugin-updater` 2.13.1 在 Windows 把下載的安裝檔寫成
//! `%TEMP%\<app_name>-<version>-updater-<6 碼英數>\<app_name>-<version>-installer.exe`（`updater.rs`
//! `make_temp_dir`＝`tempfile::Builder::prefix("{app_name}-{version}-updater-").tempdir().keep()`，隨機尾碼是
//! `tempfile` 預設的 6 個英數字元；`write_to_temp`＝其中的 `{app_name}-{version}-installer<副檔名>`）。
//! 外掛呼叫 `ShellExecuteW` 後立刻 `exit(0)`，安裝檔又還在執行，沒有人能刪它——每次更新殘留約 30 MB。
//!
//! 宿主啟動後在**背景執行緒**清掉：
//!
//! - 只看目前使用者的 `%TEMP%`（`std::env::temp_dir()`，與外掛寫入處相同）**頂層**，不遞迴；
//! - 只刪「資料夾」（外掛只建資料夾；頂層的檔案一律不碰），名稱必須**整串**符合
//!   `<app_name>-<版本>-updater-<6 碼英數>`（版本＝SemVer 字元集、以數字開頭）；
//! - 資料夾本身必須是真正的資料夾（不是符號連結／接合點），內容只能是外掛寫的安裝檔
//!   （`<app_name>-…-installer….exe|.msi` 的一般檔案）；多出任何別的東西就整個不碰；
//! - 資料夾與其內容的**最新修改時間**距今超過 [`STALE_AFTER`]（1 小時）才刪——剛下載、安裝檔仍在執行的
//!   不會被刪（新宿主由安裝檔啟動時，安裝檔還在跑，修改時間是幾秒前）；
//! - 逐檔刪除後 `remove_dir`（不用 `remove_dir_all`）；任何失敗（檔案被占用等）只記錄、不影響宿主。

use std::fs;
use std::io;
use std::path::Path;
use std::time::{Duration, SystemTime};

use super::LOG_TARGET;

/// 超過這個時間沒有修改的暫存資料夾才清掉。
pub const STALE_AFTER: Duration = Duration::from_secs(60 * 60);

/// `tempfile` 預設的隨機尾碼長度。
const RANDOM_SUFFIX_LEN: usize = 6;

/// 一次清理的結果（記錄用，也供測試斷言）。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CleanupReport {
    /// 已刪除的資料夾名稱。
    pub removed: Vec<String>,
    /// 名稱符合但還不夠舊（不刪）。
    pub too_recent: usize,
    /// 名稱符合但內容或型態可疑（符號連結、有別的東西），整個不碰。
    pub skipped: Vec<String>,
    /// 刪除失敗（名稱、原因）。
    pub failed: Vec<(String, String)>,
}

/// 名稱是否整串符合外掛的暫存資料夾命名：`<app_name>-<版本>-updater-<6 碼英數>`。
pub fn is_updater_temp_dir_name(name: &str, app_name: &str) -> bool {
    if app_name.is_empty() {
        return false;
    }
    let Some(rest) = name
        .strip_prefix(app_name)
        .and_then(|r| r.strip_prefix('-'))
    else {
        return false;
    };
    let Some((version, suffix)) = rest.rsplit_once("-updater-") else {
        return false;
    };
    version.starts_with(|c: char| c.is_ascii_digit())
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
        && suffix.len() == RANDOM_SUFFIX_LEN
        && suffix.chars().all(|c| c.is_ascii_alphanumeric())
}

/// 內容檔名是否為外掛寫的安裝檔：`<app_name>-…-installer….exe|.msi`。
fn is_plugin_installer_name(name: &str, app_name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    name.starts_with(&format!("{app_name}-"))
        && lower.contains("-installer")
        && (lower.ends_with(".exe") || lower.ends_with(".msi"))
}

/// 資料夾與其內容的最新修改時間；內容不是「一般檔案、名稱符合安裝檔」就回 `None`（可疑，不碰）。
fn newest_mtime_if_plugin_dir(dir: &Path, app_name: &str) -> io::Result<Option<SystemTime>> {
    let mut newest = fs::symlink_metadata(dir)?.modified()?;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let meta = fs::symlink_metadata(entry.path())?;
        let name = entry.file_name();
        let plausible = meta.file_type().is_file()
            && name
                .to_str()
                .is_some_and(|n| is_plugin_installer_name(n, app_name));
        if !plausible {
            return Ok(None);
        }
        newest = newest.max(meta.modified()?);
    }
    Ok(Some(newest))
}

/// 掃 `temp_dir` 頂層並刪除過期的外掛暫存資料夾。`now` 與 `stale_after` 可注入（測試用）。
/// 只有列舉 `temp_dir` 本身失敗才回 `Err`；單一資料夾的問題記在報告裡。
pub fn sweep(
    temp_dir: &Path,
    app_name: &str,
    now: SystemTime,
    stale_after: Duration,
) -> io::Result<CleanupReport> {
    sweep_with(temp_dir, app_name, now, stale_after, remove_plugin_dir)
}

/// [`sweep`] 的本體；刪除動作可注入（測試用它模擬檔案被占用等刪不掉的情況）。
fn sweep_with(
    temp_dir: &Path,
    app_name: &str,
    now: SystemTime,
    stale_after: Duration,
    remove: impl Fn(&Path) -> io::Result<()>,
) -> io::Result<CleanupReport> {
    let mut report = CleanupReport::default();
    for entry in fs::read_dir(temp_dir)? {
        let Ok(entry) = entry else { continue };
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        if !is_updater_temp_dir_name(name, app_name) {
            continue;
        }
        let path = entry.path();
        let is_real_dir = fs::symlink_metadata(&path)
            .map(|m| m.file_type().is_dir())
            .unwrap_or(false);
        if !is_real_dir {
            continue; // 外掛只建資料夾；同名的檔案或連結一律不碰。
        }
        let newest = match newest_mtime_if_plugin_dir(&path, app_name) {
            Ok(Some(t)) => t,
            Ok(None) => {
                report.skipped.push(name.to_owned());
                continue;
            }
            Err(e) => {
                report.failed.push((name.to_owned(), e.to_string()));
                continue;
            }
        };
        // 修改時間在未來（時鐘調整）視為剛修改，不刪。
        let age = now.duration_since(newest).unwrap_or(Duration::ZERO);
        if age <= stale_after {
            report.too_recent += 1;
            continue;
        }
        match remove(&path) {
            Ok(()) => report.removed.push(name.to_owned()),
            Err(e) => report.failed.push((name.to_owned(), e.to_string())),
        }
    }
    Ok(report)
}

/// 逐檔刪除（上面已確認內容只有一般檔案）再 `remove_dir`；不遞迴。
fn remove_plugin_dir(dir: &Path) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        fs::remove_file(entry?.path())?;
    }
    fs::remove_dir(dir)
}

/// 啟動後在背景執行一次（不阻塞啟動；失敗只記錄）。`app_name` 取外掛用的同一個值（`package_info().name`）。
pub fn spawn(app_name: String) {
    let spawned = std::thread::Builder::new()
        .name("fc-update-temp-cleanup".to_owned())
        .spawn(move || {
            let temp = std::env::temp_dir();
            match sweep(&temp, &app_name, SystemTime::now(), STALE_AFTER) {
                Ok(report) => log_report(&temp, &report),
                Err(e) => log::warn!(
                    target: LOG_TARGET,
                    "更新暫存檔清理：無法列舉 {}（{e}）",
                    temp.display()
                ),
            }
        });
    if let Err(e) = spawned {
        log::warn!(target: LOG_TARGET, "更新暫存檔清理：無法建立背景執行緒（{e}）");
    }
}

fn log_report(temp: &Path, report: &CleanupReport) {
    if !report.removed.is_empty() {
        log::info!(
            target: LOG_TARGET,
            "更新暫存檔清理：已刪除 {} 個過期資料夾（{}）於 {}",
            report.removed.len(),
            report.removed.join("、"),
            temp.display()
        );
    }
    for name in &report.skipped {
        log::warn!(target: LOG_TARGET, "更新暫存檔清理：{name} 內容不是預期的安裝檔，未處理");
    }
    for (name, why) in &report.failed {
        log::warn!(target: LOG_TARGET, "更新暫存檔清理：刪除 {name} 失敗（{why}），下次啟動再試");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::test_util::TempDir;

    const APP: &str = "fc-host";

    fn hours(n: u64) -> Duration {
        Duration::from_secs(n * 3600)
    }

    /// 建一個外掛風格的暫存資料夾（內含安裝檔）。
    fn make_plugin_dir(root: &Path, dir_name: &str) -> std::path::PathBuf {
        let dir = root.join(dir_name);
        fs::create_dir(&dir).unwrap();
        let version = dir_name
            .strip_prefix("fc-host-")
            .and_then(|r| r.split("-updater-").next())
            .unwrap_or("0.0.0");
        fs::write(dir.join(format!("fc-host-{version}-installer.exe")), b"MZ").unwrap();
        dir
    }

    #[test]
    fn name_pattern_matches_the_plugin_naming_and_nothing_wider() {
        for ok in [
            "fc-host-0.1.1-updater-REOjM3",
            "fc-host-0.1.2-updater-Pdyv58",
            "fc-host-1.20.3-updater-abcdef",
            "fc-host-0.2.0-beta.1-updater-A1b2C3",
            "fc-host-0.2.0+build.5-updater-A1b2C3",
        ] {
            assert!(is_updater_temp_dir_name(ok, APP), "{ok}");
        }
        for bad in [
            "",
            "fc-host",
            "fc-host-updater-REOjM3",              // 沒有版本
            "fc-host--updater-REOjM3",             // 版本空
            "fc-host-0.1.1-updater-REOjM",         // 尾碼 5 碼
            "fc-host-0.1.1-updater-REOjM33",       // 尾碼 7 碼
            "fc-host-0.1.1-updater-REO_M3",        // 尾碼含非英數
            "fc-host-0.1.1-updater-REOjM3.bak",    // 多餘後綴
            "x-fc-host-0.1.1-updater-REOjM3",      // 前綴不符
            "fc-host-v0.1.1-updater-REOjM3",       // 版本非數字開頭
            "other-0.1.1-updater-REOjM3",          // 別的應用程式
            "fc-host-0.1.1-installer.exe",         // 檔名樣式
            "fc-host-0.1.1-updater-REOjM3/../etc", // 路徑
            "fc-hostx-0.1.1-updater-REOjM3",       // app 名稱只是前綴
            "tauri_current_app12345",
        ] {
            assert!(!is_updater_temp_dir_name(bad, APP), "{bad}");
        }
        assert!(!is_updater_temp_dir_name("-0.1.1-updater-REOjM3", ""));
    }

    #[test]
    fn removes_stale_plugin_dirs_with_their_installer() {
        let tmp = TempDir::new("tmpclean-stale");
        let a = make_plugin_dir(tmp.path(), "fc-host-0.1.1-updater-REOjM3");
        let b = make_plugin_dir(tmp.path(), "fc-host-0.1.2-updater-Pdyv58");
        let report = sweep(tmp.path(), APP, SystemTime::now() + hours(2), STALE_AFTER).unwrap();
        assert_eq!(report.removed.len(), 2, "{report:?}");
        assert!(!a.exists() && !b.exists());
        assert!(report.failed.is_empty() && report.skipped.is_empty());
    }

    #[test]
    fn keeps_recent_dirs() {
        let tmp = TempDir::new("tmpclean-recent");
        let dir = make_plugin_dir(tmp.path(), "fc-host-0.1.1-updater-REOjM3");
        // 剛建立：安裝檔可能還在執行。
        let report = sweep(tmp.path(), APP, SystemTime::now(), STALE_AFTER).unwrap();
        assert!(report.removed.is_empty(), "{report:?}");
        assert_eq!(report.too_recent, 1);
        assert!(dir.exists());
        // 剛好 59 分鐘也不刪。
        let report = sweep(
            tmp.path(),
            APP,
            SystemTime::now() + Duration::from_secs(59 * 60),
            STALE_AFTER,
        )
        .unwrap();
        assert!(report.removed.is_empty(), "{report:?}");
        assert!(dir.exists());
    }

    /// 現在時間早於修改時間（時鐘被調回）＝視為剛修改，不刪。
    #[test]
    fn keeps_dirs_modified_in_the_future() {
        let tmp = TempDir::new("tmpclean-future");
        let dir = make_plugin_dir(tmp.path(), "fc-host-0.1.1-updater-REOjM3");
        let past = SystemTime::now() - hours(5);
        let report = sweep(tmp.path(), APP, past, STALE_AFTER).unwrap();
        assert!(report.removed.is_empty());
        assert!(dir.exists());
    }

    #[test]
    fn never_touches_anything_else_in_temp() {
        let tmp = TempDir::new("tmpclean-others");
        let root = tmp.path();
        // 別的應用程式、名稱不符、頂層同名檔案、使用者的檔案。
        let other_app = root.join("other-0.1.1-updater-REOjM3");
        fs::create_dir(&other_app).unwrap();
        fs::write(other_app.join("other-0.1.1-installer.exe"), b"x").unwrap();
        let tauri_dir = root.join("tauri_current_appABC123");
        fs::create_dir(&tauri_dir).unwrap();
        let plain_file = root.join("fc-host-0.1.1-updater-REOjM3.txt");
        fs::write(&plain_file, b"x").unwrap();
        let name_like_file = root.join("fc-host-0.1.1-updater-ABCDEF");
        fs::write(&name_like_file, b"x").unwrap(); // 名稱符合、但是檔案不是資料夾
        let installer_file = root.join("fc-host-0.1.1-installer.exe");
        fs::write(&installer_file, b"x").unwrap();
        let report = sweep(root, APP, SystemTime::now() + hours(48), STALE_AFTER).unwrap();
        assert_eq!(report, CleanupReport::default(), "{report:?}");
        for p in [
            &other_app,
            &tauri_dir,
            &plain_file,
            &name_like_file,
            &installer_file,
        ] {
            assert!(p.exists(), "{}", p.display());
        }
    }

    /// 名稱符合、但內容有別的東西（使用者檔案、子資料夾）：整個不碰，也不刪其中的安裝檔。
    #[test]
    fn dirs_with_unexpected_content_are_left_alone() {
        let tmp = TempDir::new("tmpclean-suspicious");
        let with_extra_file = make_plugin_dir(tmp.path(), "fc-host-0.1.1-updater-AAAAAA");
        fs::write(with_extra_file.join("notes.txt"), b"mine").unwrap();
        let with_subdir = make_plugin_dir(tmp.path(), "fc-host-0.1.2-updater-BBBBBB");
        fs::create_dir(with_subdir.join("nested")).unwrap();
        let wrong_ext = tmp.path().join("fc-host-0.1.3-updater-CCCCCC");
        fs::create_dir(&wrong_ext).unwrap();
        fs::write(wrong_ext.join("fc-host-0.1.3-installer.dll"), b"x").unwrap();
        let report = sweep(tmp.path(), APP, SystemTime::now() + hours(48), STALE_AFTER).unwrap();
        assert!(report.removed.is_empty(), "{report:?}");
        assert_eq!(report.skipped.len(), 3, "{report:?}");
        assert!(with_extra_file.join("notes.txt").exists());
        assert!(with_extra_file.join("fc-host-0.1.1-installer.exe").exists());
        assert!(with_subdir.join("nested").exists());
        assert!(wrong_ext.join("fc-host-0.1.3-installer.dll").exists());
    }

    /// 空資料夾（安裝檔已被刪、資料夾殘留）也算外掛殘留，過期就清。
    #[test]
    fn removes_stale_empty_plugin_dir() {
        let tmp = TempDir::new("tmpclean-empty");
        let dir = tmp.path().join("fc-host-0.1.1-updater-REOjM3");
        fs::create_dir(&dir).unwrap();
        let report = sweep(tmp.path(), APP, SystemTime::now() + hours(2), STALE_AFTER).unwrap();
        assert_eq!(report.removed, vec!["fc-host-0.1.1-updater-REOjM3"]);
        assert!(!dir.exists());
    }

    /// 暫存目錄本身列舉失敗：回 `Err`（呼叫端只記錄），不 panic。
    #[test]
    fn unreadable_temp_dir_is_an_error_not_a_panic() {
        let tmp = TempDir::new("tmpclean-missing");
        let missing = tmp.path().join("no-such-dir");
        assert!(sweep(&missing, APP, SystemTime::now(), STALE_AFTER).is_err());
    }

    /// 刪除失敗（例如安裝檔仍被占用）只記在報告裡，不影響其他資料夾，也不 panic。
    #[test]
    fn a_failed_removal_is_reported_and_other_dirs_still_go() {
        let tmp = TempDir::new("tmpclean-locked");
        let locked = make_plugin_dir(tmp.path(), "fc-host-0.1.1-updater-LOCKD1");
        let free = make_plugin_dir(tmp.path(), "fc-host-0.1.2-updater-FREE22");
        let report = sweep_with(
            tmp.path(),
            APP,
            SystemTime::now() + hours(2),
            STALE_AFTER,
            |dir| {
                if dir.ends_with("fc-host-0.1.1-updater-LOCKD1") {
                    Err(io::Error::new(io::ErrorKind::PermissionDenied, "被占用"))
                } else {
                    remove_plugin_dir(dir)
                }
            },
        )
        .unwrap();
        assert_eq!(report.failed.len(), 1, "{report:?}");
        assert_eq!(report.failed[0].0, "fc-host-0.1.1-updater-LOCKD1");
        assert!(report.failed[0].1.contains("被占用"));
        assert_eq!(report.removed, vec!["fc-host-0.1.2-updater-FREE22"]);
        assert!(locked.exists());
        assert!(!free.exists());
    }
}
