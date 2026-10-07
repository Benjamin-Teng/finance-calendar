//! 桌布路徑比對（dynamic-wallpaper task 4.4；brief「路徑比對」）。
//!
//! Windows 的檔案系統路徑不分大小寫，而且不只 ASCII：使用者名稱含 `Ö`、`É` 等字母時，
//! `GetWallpaper` 讀回的大小寫可能與宿主記錄的不同。只處理 ASCII 的 `eq_ignore_ascii_case`
//! 會把這種情況誤判成「使用者自行換了桌布」而讓位，所以一律用 Windows 的
//! `CompareStringOrdinal(..., bIgnoreCase = TRUE)`（與檔案系統相同的序數、不分大小寫規則，
//! 不受地區設定影響）。
//!
//! 比對前只做一項正規化：`/` 視同 `\`。不展開環境變數、不解析 `..`、不處理 `\\?\` 前綴——
//! `GetWallpaper` 與宿主自己組出的路徑都是一般的絕對路徑。
//!
//! 本檔是 Win32 呼叫（AGENTS.md：Win32 集中在 `desktop` 底下），但不碰任何視窗或 explorer，
//! 任何執行緒都能呼叫。

use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::Win32::Globalization::{CompareStringOrdinal, CSTR_EQUAL};

const BACKSLASH: u16 = b'\\' as u16;
const SLASH: u16 = b'/' as u16;

fn normalize_units(units: impl Iterator<Item = u16>) -> Vec<u16> {
    units
        .map(|u| if u == SLASH { BACKSLASH } else { u })
        .collect()
}

/// 兩段 UTF-16 是否在「序數、不分大小寫」下相等。
fn units_eq_ignore_case(a: &[u16], b: &[u16]) -> bool {
    // 序數比較逐一比對（大寫化後的）字碼單元，長度不同必不相等；空字串不必呼叫 API。
    if a.len() != b.len() {
        return false;
    }
    if a.is_empty() {
        return true;
    }
    // SAFETY: 兩個切片在呼叫期間存活，windows crate 以切片長度傳入字元數（不需要 NUL 結尾）；
    // 不保留任何指標。路徑長度遠小於 i32::MAX，crate 內部的長度轉換不會失敗。
    unsafe { CompareStringOrdinal(a, b, true) == CSTR_EQUAL }
}

/// 兩個路徑字串是否指向同一路徑（Windows 序數、不分大小寫；`/` 視同 `\`）。
pub fn path_eq(a: &str, b: &str) -> bool {
    let a = normalize_units(a.encode_utf16());
    let b = normalize_units(b.encode_utf16());
    units_eq_ignore_case(&a, &b)
}

/// `path` 是否位於資料夾 `dir` 之內（任意深度；`dir` 本身不算）。比對規則同 [`path_eq`]，
/// 且以路徑元件為界：`C:\out2\x.png` 不在 `C:\out` 之內。
pub fn path_is_within(path: &str, dir: &Path) -> bool {
    let path = normalize_units(path.encode_utf16());
    let mut dir = normalize_units(dir.as_os_str().encode_wide());
    while dir.last() == Some(&BACKSLASH) {
        dir.pop();
    }
    if dir.is_empty() || path.len() <= dir.len() + 1 {
        return false;
    }
    // 在反斜線處切開：反斜線不是 surrogate，切點不會拆開一個字元。
    path[dir.len()] == BACKSLASH && units_eq_ignore_case(&path[..dir.len()], &dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_ascii_case_insensitive_equality() {
        let a = "C:\\Users\\Ölaf\\Pictures\\Été.jpg";
        let b = "c:\\USERS\\ölaf\\pictures\\ÉTÉ.JPG";
        assert!(
            !a.eq_ignore_ascii_case(b),
            "對照組：只處理 ASCII 的比較會判定不同（這正是要避免的誤判）"
        );
        assert!(path_eq(a, b), "Windows 序數不分大小寫比較應判定相同");
    }

    #[test]
    fn different_paths_are_not_equal() {
        assert!(!path_eq("C:\\a\\b.jpg", "C:\\a\\c.jpg"));
        assert!(!path_eq("C:\\a\\b.jpg", "C:\\a\\b.jpg2"));
        assert!(!path_eq("", "C:\\a.jpg"));
        assert!(path_eq("", ""), "空字串＝純色，兩邊都是純色時相等");
    }

    #[test]
    fn slash_and_backslash_are_equivalent() {
        assert!(path_eq("C:/Users/x/a.png", "C:\\Users\\x\\a.png"));
    }

    #[test]
    fn within_dir_respects_component_boundary_and_case() {
        let dir = Path::new("C:\\Users\\Ölaf\\AppData\\Local\\tw.fintools.fc-host\\wallpaper");
        assert!(path_is_within(
            "c:\\users\\ÖLAF\\appdata\\local\\TW.FINTOOLS.FC-HOST\\Wallpaper\\m-1-a.png",
            dir
        ));
        assert!(path_is_within(
            "C:\\Users\\Ölaf\\AppData\\Local\\tw.fintools.fc-host\\wallpaper\\original\\m-1.jpg",
            dir
        ));
        assert!(
            !path_is_within(
                "C:\\Users\\Ölaf\\AppData\\Local\\tw.fintools.fc-host\\wallpaper2\\x.png",
                dir
            ),
            "同字首但不同資料夾"
        );
        assert!(
            !path_is_within(
                "C:\\Users\\Ölaf\\AppData\\Local\\tw.fintools.fc-host\\wallpaper",
                dir
            ),
            "資料夾本身不算在內"
        );
        assert!(!path_is_within("", dir));
        assert!(
            path_is_within("C:\\out\\a.png", Path::new("C:\\out\\")),
            "資料夾結尾的反斜線不影響判定"
        );
    }
}
