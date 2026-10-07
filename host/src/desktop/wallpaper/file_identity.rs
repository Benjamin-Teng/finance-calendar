//! 檔案識別（dynamic-wallpaper task 4.4 修正輪 2，審查 finding 10）：以「磁碟區序號＋檔案索引」
//! 判斷兩個寫法不同的路徑（8.3 短檔名、junction、`\\?\` 前綴……）是否為同一個檔案。
//!
//! 路徑比對（`pathcmp`）仍是第一道判斷；只有路徑字串不相等時才用這裡做第二道判斷。查不到
//! （檔案不存在、沒有權限）一律回 `None`＝「無法證明是同一個檔」，呼叫端照路徑比對的結果走。
//!
//! 網路路徑一律不查（[`identity_lookup_allowed`]）：UNC（`\\server\share\...`、`\\?\UNC\...`，
//! [`is_network_path`]），以及對映成磁碟機代號的網路磁碟機（`GetDriveTypeW` 回 `DRIVE_REMOTE`，
//! [`Win32DriveTypes`]；修正輪 4）。伺服器離線時 `CreateFileW` 可能卡上數十秒，拖住系統匣結束時的
//! 還原。這類路徑只靠路徑比對。呼叫端另外只在兩個路徑位於同一個磁碟機代號時才查
//! （[`same_local_drive`]）。
//!
//! 真實實作 [`Win32FileIdentity`]（`CreateFileW` 只要求 `FILE_READ_ATTRIBUTES`、三種共用全開、
//! `FILE_FLAG_BACKUP_SEMANTICS` 以便開啟資料夾，再 `GetFileInformationByHandle`）；測試注入
//! 假實作。只讀中繼資料，不碰檔案內容，也不碰 explorer。

use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::core::PCWSTR;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Storage::FileSystem::{
    CreateFileW, GetDriveTypeW, GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, OPEN_EXISTING,
};

/// 一個檔案（或資料夾）的識別。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileId {
    pub volume_serial: u32,
    pub file_index: u64,
}

/// 查檔案識別的介面（測試注入假實作）。
pub trait FileIdentity {
    /// `path` 的識別；查不到為 `None`。
    fn file_id(&self, path: &Path) -> Option<FileId>;
}

/// 是否為網路（UNC）路徑：`\\server\...`、`//server/...`、`\\?\UNC\...`（不分大小寫）。
/// `\\?\C:\...` 與 `\\.\...`（本機裝置）不算。
pub fn is_network_path(path: &Path) -> bool {
    let s = path.to_string_lossy().replace('/', r"\");
    let lower = s.to_ascii_lowercase();
    if lower.starts_with(r"\\?\unc\") {
        return true;
    }
    lower.starts_with(r"\\") && !lower.starts_with(r"\\?\") && !lower.starts_with(r"\\.\")
}

/// 查磁碟機類型的介面（測試注入假實作）。
pub trait DriveTypes {
    /// 磁碟機代號 `letter`（大寫 ASCII）是否為網路磁碟機。
    fn is_remote(&self, letter: char) -> bool;
}

/// `GetDriveTypeW` 的 `DRIVE_REMOTE`。常數本身在 `Win32_System_WindowsProgramming`（查證於
/// `~/.cargo/registry/src` 的 windows-0.62.2：`src/Windows/Win32/System/WindowsProgramming/mod.rs`
/// 的 `pub const DRIVE_REMOTE: u32 = 4u32;`），只為一個常數不另開 feature。
const DRIVE_REMOTE: u32 = 4;

/// 真實實作：`GetDriveTypeW("<代號>:\")`。只查本機的磁碟機對應，不開檔（斷線的網路磁碟機是否
/// 也立即回傳，留待實機確認）。
#[derive(Debug, Default, Clone, Copy)]
pub struct Win32DriveTypes;

impl DriveTypes for Win32DriveTypes {
    fn is_remote(&self, letter: char) -> bool {
        let root: Vec<u16> = format!("{letter}:\\")
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: `root` 以 NUL 結尾且在呼叫期間存活；函式不保留指標。
        unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) == DRIVE_REMOTE }
    }
}

/// 路徑的磁碟機代號（大寫）：`C:\...`、`C:/...`、`\\?\C:\...`、`\\.\C:\...`。UNC、
/// `\\?\Volume{...}`、相對路徑與 `C:相對` 一律 `None`。
pub fn drive_letter(path: &str) -> Option<char> {
    let s = path.replace('/', r"\");
    let rest = s
        .strip_prefix(r"\\?\")
        .or_else(|| s.strip_prefix(r"\\.\"))
        .unwrap_or(&s);
    let mut chars = rest.chars();
    let letter = chars.next().filter(char::is_ascii_alphabetic)?;
    if chars.next() != Some(':') {
        return None;
    }
    match chars.next() {
        Some('\\') => Some(letter.to_ascii_uppercase()),
        _ => None,
    }
}

/// 兩個路徑是否在同一個磁碟機代號上（兩邊都要有代號）。不同磁碟機（含 `subst`、跨磁碟機的
/// junction）不比對檔案識別——這是刻意的取捨：只靠路徑比對，換取不對無關的磁碟機開檔。
pub fn same_local_drive(a: &str, b: &str) -> bool {
    matches!((drive_letter(a), drive_letter(b)), (Some(x), Some(y)) if x == y)
}

/// 可否對 `path` 查檔案識別（開檔）：要有磁碟機代號、不是 UNC、所在磁碟機不是網路磁碟機
/// （斷線的對映磁碟機開檔可能卡上數十秒）。
pub fn identity_lookup_allowed(path: &Path, drives: &dyn DriveTypes) -> bool {
    if is_network_path(path) {
        return false;
    }
    drive_letter(&path.to_string_lossy()).is_some_and(|d| !drives.is_remote(d))
}

/// 真實實作（Win32）。
#[derive(Debug, Default, Clone, Copy)]
pub struct Win32FileIdentity;

impl FileIdentity for Win32FileIdentity {
    fn file_id(&self, path: &Path) -> Option<FileId> {
        if !identity_lookup_allowed(path, &Win32DriveTypes) {
            return None;
        }
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: `wide` 以 NUL 結尾且在呼叫期間存活；不傳安全性屬性與範本把手。
        let handle = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                FILE_READ_ATTRIBUTES.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                None,
            )
        }
        .ok()?;
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: `handle` 由上面成功的 CreateFileW 取得；`info` 為可寫的區域變數。
        let got = unsafe { GetFileInformationByHandle(handle, &mut info) };
        // SAFETY: `handle` 有效且只關閉一次。
        unsafe {
            let _ = CloseHandle(handle);
        }
        got.ok()?;
        Some(FileId {
            volume_serial: info.dwVolumeSerialNumber,
            file_index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        })
    }
}

/// 一律查不到（呼叫端只靠路徑比對）。
#[derive(Debug, Default, Clone, Copy)]
pub struct NoFileIdentity;

impl FileIdentity for NoFileIdentity {
    fn file_id(&self, _path: &Path) -> Option<FileId> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// 只在 `%TEMP%` 下建立與讀取暫存檔。
    #[test]
    fn same_file_through_different_path_forms() {
        let dir = std::env::temp_dir().join(format!("fc-host-fileid-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.png");
        let b = dir.join("b.png");
        fs::write(&a, b"A").unwrap();
        fs::write(&b, b"B").unwrap();

        let ids = Win32FileIdentity;
        let id_a = ids.file_id(&a).expect("應查得到暫存檔的識別");
        let verbatim = format!("\\\\?\\{}", a.display());
        assert_eq!(
            ids.file_id(Path::new(&verbatim)),
            Some(id_a),
            "`\\\\?\\` 前綴與一般寫法是同一個檔"
        );
        assert_ne!(ids.file_id(&b), Some(id_a), "不同檔案識別不同");
        assert!(ids.file_id(&dir).is_some(), "資料夾也查得到");
        assert_eq!(ids.file_id(&dir.join("missing.png")), None);
        assert_eq!(ids.file_id(Path::new("")), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn network_paths_are_classified_and_skipped() {
        // 原始字串（r"..."）：反斜線照字面，不經跳脫。
        for p in [
            r"\\server\share\a.jpg",
            r"//server/share/a.jpg",
            r"\\?\UNC\server\share\a.jpg",
            r"\\?\unc\server\share\a.jpg",
        ] {
            assert!(is_network_path(Path::new(p)), "{p}");
        }
        for p in [
            r"C:\Users\x\a.jpg",
            r"\\?\C:\Users\x\a.jpg",
            r"\\.\C:\a.jpg",
            "",
        ] {
            assert!(!is_network_path(Path::new(p)), "{p}");
        }
        // 不連線、立即回 None（離線的伺服器不會讓呼叫端卡住數十秒）。
        let started = std::time::Instant::now();
        assert_eq!(
            Win32FileIdentity.file_id(Path::new(r"\\fc-host-no-such-server\share\a.jpg")),
            None
        );
        assert!(started.elapsed() < std::time::Duration::from_millis(200));
    }

    /// 假的磁碟機類型：列出的代號是網路磁碟機。
    struct FakeDrives(&'static [char]);

    impl DriveTypes for FakeDrives {
        fn is_remote(&self, letter: char) -> bool {
            self.0.contains(&letter)
        }
    }

    #[test]
    fn drive_letter_forms() {
        // 原始字串（r"..."）：反斜線照字面，不經跳脫。
        assert_eq!(drive_letter(r"C:\Users\x\a.jpg"), Some('C'));
        assert_eq!(drive_letter(r"d:/pics/a.jpg"), Some('D'));
        assert_eq!(drive_letter(r"\\?\z:\a.jpg"), Some('Z'));
        assert_eq!(drive_letter(r"\\.\E:\a.jpg"), Some('E'));
        assert_eq!(drive_letter(r"C:\"), Some('C'));
        for p in [
            r"\\server\share\a.jpg",
            r"\\?\UNC\server\share\a.jpg",
            r"\\?\Volume{0000}\a.jpg",
            r"relative\a.jpg",
            r"C:relative.jpg",
            r"1:\a.jpg",
            "",
        ] {
            assert_eq!(drive_letter(p), None, "{p}");
        }
        assert!(same_local_drive(r"C:\a.jpg", r"\\?\c:\b\c.jpg"));
        assert!(!same_local_drive(r"C:\a.jpg", r"D:\a.jpg"));
        assert!(!same_local_drive(r"\\server\s\a.jpg", r"\\server\s\a.jpg"));
    }

    #[test]
    fn identity_lookup_skips_remote_drives_and_unc() {
        let drives = FakeDrives(&['Z']);
        assert!(identity_lookup_allowed(Path::new(r"C:\a.jpg"), &drives));
        assert!(identity_lookup_allowed(Path::new(r"\\?\C:\a.jpg"), &drives));
        for p in [
            r"Z:\nas\a.jpg",
            r"z:\nas\a.jpg",
            r"\\?\Z:\nas\a.jpg",
            r"\\server\share\a.jpg",
            r"relative\a.jpg",
            "",
        ] {
            assert!(!identity_lookup_allowed(Path::new(p), &drives), "{p}");
        }
        // 真實查詢：暫存資料夾所在的磁碟機不是網路磁碟機（只查本機對應，不開檔）。
        let temp = std::env::temp_dir().to_string_lossy().into_owned();
        let letter = drive_letter(&temp).expect("暫存資料夾有磁碟機代號");
        assert!(!Win32DriveTypes.is_remote(letter));
    }
}
