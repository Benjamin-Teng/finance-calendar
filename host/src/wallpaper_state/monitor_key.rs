//! 螢幕鍵（design.md D4；task 4.4 brief 裁決 3）：原圖備份檔名
//! `wallpaper\original\<螢幕鍵>.<副檔名>` 與狀態檔的逐螢幕紀錄都以它為鍵，4.5 的輸出檔
//! `<螢幕鍵>-a.png`／`-b.png` 也應沿用 [`monitor_key`]。
//!
//! ## 對應方式（集中在本檔，1.5 探針後可替換）
//!
//! 1. 以 `GetMonitorRECT` 的矩形，在既有的穩定顯示器識別（task 2.5：
//!    `crate::desktop::source_name_to_device_path` 查得的 `monitorDevicePath`，配上該螢幕的
//!    矩形，由 4.7 組成 [`StableDisplay`] 清單）中找矩形完全相同的一台，取它的
//!    `monitorDevicePath`；
//! 2. 對應不到（離線、兩次列舉之間組態變動、清單為空）時，退回 `GetMonitorDevicePathAt` 的
//!    裝置路徑。
//!
//! 兩者都經同一個雜湊（FNV-1a 64，對 ASCII 小寫化後的字串；跨版本、跨執行固定）變成
//! 可當檔名的 `m-<16 位十六進位>`。探針 1.1 的記錄顯示 `GetMonitorDevicePathAt` 回傳的是
//! `\\?\DISPLAY#...#{e6f07b5f-...}` 形式，與 `monitorDevicePath` 同一形式；兩條路徑取得的字串
//! 相同時鍵也相同，所以「這次對應得到、下次對應不到」不會讓同一台螢幕換鍵。若 1.5 證實兩者
//! 不同，只改本檔：狀態檔另以裝置路徑做第二層比對（`super::find_record`），換鍵也不會把同一台
//! 螢幕當成新螢幕重新記錄。

use crate::layout::PhysicalRect;

/// 一台螢幕的穩定識別與它目前的矩形（虛擬桌面座標、實體像素）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StableDisplay {
    /// 穩定的 `monitorDevicePath`（task 2.5）。
    pub device_path: String,
    /// 該螢幕的矩形，用來與 `GetMonitorRECT` 對應。
    pub rect: PhysicalRect,
}

/// 由 `IDesktopWallpaper` 的裝置路徑與矩形算出螢幕鍵（規則見模組文件）。`rect` 為 `None`
/// （離線）時一律走退回路徑。
pub fn monitor_key(
    wallpaper_device_path: &str,
    rect: Option<PhysicalRect>,
    stable: &[StableDisplay],
) -> String {
    let source = rect
        .and_then(|r| stable.iter().find(|d| d.rect == r))
        .map_or(wallpaper_device_path, |d| d.device_path.as_str());
    format!("m-{:016x}", fnv1a64(&source.to_ascii_lowercase()))
}

/// FNV-1a 64 位元（固定演算法，不用 `std` 的 `DefaultHasher`——後者不保證跨版本穩定，
/// 而螢幕鍵會寫進檔名與狀態檔）。
fn fnv1a64(s: &str) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    s.bytes()
        .fold(OFFSET, |h, b| (h ^ u64::from(b)).wrapping_mul(PRIME))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATH_A: &str =
        "\\\\?\\DISPLAY#BOE0CDF#4&102fce2&0&UID8388688#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";
    const PATH_B: &str =
        "\\\\?\\DISPLAY#AUSAA34#5&1091fafa&0&UID4356#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";

    fn rect(x: i32) -> PhysicalRect {
        PhysicalRect {
            x,
            y: 0,
            width: 3840,
            height: 2160,
        }
    }

    #[test]
    fn key_is_filename_safe_and_deterministic() {
        let k = monitor_key(PATH_A, Some(rect(0)), &[]);
        assert!(k.starts_with("m-"), "{k}");
        assert_eq!(k.len(), 2 + 16, "{k}");
        assert!(k[2..].chars().all(|c| c.is_ascii_hexdigit()), "{k}");
        assert_eq!(k, monitor_key(PATH_A, Some(rect(0)), &[]), "同輸入同鍵");
        assert_ne!(k, monitor_key(PATH_B, Some(rect(0)), &[]), "不同螢幕不同鍵");
    }

    #[test]
    fn stable_identity_matched_by_rect_wins() {
        let stable = [
            StableDisplay {
                device_path: "STABLE-X".to_owned(),
                rect: rect(0),
            },
            StableDisplay {
                device_path: "STABLE-Y".to_owned(),
                rect: rect(3840),
            },
        ];
        assert_eq!(
            monitor_key(PATH_A, Some(rect(3840)), &stable),
            monitor_key("STABLE-Y", None, &[]),
            "以矩形對應到穩定識別"
        );
        assert_eq!(
            monitor_key(PATH_A, Some(rect(9999)), &stable),
            monitor_key(PATH_A, None, &[]),
            "對應不到時退回裝置路徑"
        );
        assert_eq!(
            monitor_key(PATH_A, None, &stable),
            monitor_key(PATH_A, None, &[]),
            "離線一律退回裝置路徑"
        );
    }

    #[test]
    fn same_string_from_either_source_gives_same_key() {
        let stable = [StableDisplay {
            device_path: PATH_A.to_ascii_uppercase(),
            rect: rect(0),
        }];
        assert_eq!(
            monitor_key(PATH_A, Some(rect(0)), &stable),
            monitor_key(PATH_A, None, &[]),
            "兩條路徑取得相同字串（大小寫不同亦然）時鍵相同"
        );
    }
}
