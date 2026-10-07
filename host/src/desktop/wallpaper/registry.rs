//! 桌布相關登錄：`HKCU\Control Panel\Desktop` 三個值的忠實快照／還原，以及 Windows 焦點狀態
//! （dynamic-wallpaper task 4.2 修正輪 2）。
//!
//! ## 為什麼不經過 COM 執行緒
//!
//! 這些呼叫只碰登錄、不碰 explorer，不會卡住；放在一般函式裡，任何執行緒（含 COM 執行緒忙碌
//! 或已死時）都能直接呼叫。探針的還原在 COM 始終不可用時仍會寫回三個登錄值，讓下次登入時
//! explorer 依登錄載入原圖——這條後備路徑必須獨立於 COM 執行緒。
//!
//! ## 忠實快照
//!
//! - 每個值記錄**原始型別與原始位元組**（[`RegValue::Present`]），不經字串轉換：`REG_EXPAND_SZ`
//!   寫回仍是 `REG_EXPAND_SZ`，非字串型別也照樣記錄、不讓整個讀取失敗。
//! - 「原本不存在」是獨立狀態（[`RegValue::Missing`]），還原時**刪除**該值（`SetWallpaper` 可能
//!   新建了它）。快照永遠完整描述三個值，還原就是把三個值逐一變回快照——沒有「不動」這種狀態。
//! - 還原逐字寫回當初讀到的位元組，不正規化大小寫（探針實測登錄的 `Wallpaper` 是全小寫路徑，
//!   與 `GetWallpaper` 的大小寫不同；兩者不可互相比對）。
//! - 快照只能在第一次 `SetWallpaper` 之前取（之後 `Wallpaper` 已被改成 `TranscodedWallpaper`），
//!   4.4 要把快照存進狀態檔，當機重啟後以狀態檔為準、不重新快照。
//!
//! ## 注入假實作
//!
//! 所有登錄存取都經 [`RegistryStore`]；真實實作 [`HkcuRegistry`]（`HKCU`），測試用記憶體內的
//! 假實作，**測試從不寫真的 HKCU**。對外的便利函式（[`snapshot_desktop_registry`] 等）只是把
//! `HkcuRegistry` 傳進 `*_in` 版本。

use std::ffi::c_void;
use std::fmt;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteKeyValueW, RegOpenKeyExW, RegQueryValueExW, RegSetKeyValueW, HKEY,
    HKEY_CURRENT_USER, KEY_QUERY_VALUE, REG_VALUE_TYPE,
};

/// `HKCU` 下桌布三個值所在的機碼。
pub const DESKTOP_KEY: &str = "Control Panel\\Desktop";
/// 桌布三個值的名稱（探針實測：`SetWallpaper` 會改寫 `Wallpaper`，還原時三個要一起寫回）。
pub const DESKTOP_VALUE_NAMES: [&str; 3] = ["Wallpaper", "WallpaperStyle", "TileWallpaper"];
/// Windows 焦點開關所在機碼（design.md D5）。
pub const SPOTLIGHT_KEY: &str =
    "Software\\Microsoft\\Windows\\CurrentVersion\\DesktopSpotlight\\Settings";
/// 背景類型所在機碼（只供診斷）。
pub const WALLPAPERS_KEY: &str =
    "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Wallpapers";

/// 登錄值型別 `REG_SZ`。
pub const REG_TYPE_SZ: u32 = 1;
/// 登錄值型別 `REG_EXPAND_SZ`。
pub const REG_TYPE_EXPAND_SZ: u32 = 2;
/// 登錄值型別 `REG_DWORD`。
pub const REG_TYPE_DWORD: u32 = 4;

/// 一個登錄值的忠實記錄。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegValue {
    /// 值不存在（含機碼不存在）。
    Missing,
    /// 值存在：原始型別（`REG_SZ`＝1、`REG_EXPAND_SZ`＝2、`REG_DWORD`＝4……）與原始位元組。
    Present { kind: u32, data: Vec<u8> },
}

impl RegValue {
    /// 以字串建立 `REG_SZ`／`REG_EXPAND_SZ` 值（含結尾 NUL，與 Windows 寫入的格式相同）。
    pub fn string(kind: u32, s: &str) -> Self {
        let data = s
            .encode_utf16()
            .chain(std::iter::once(0))
            .flat_map(u16::to_le_bytes)
            .collect();
        Self::Present { kind, data }
    }

    /// 以數值建立 `REG_DWORD` 值。
    pub fn dword(v: u32) -> Self {
        Self::Present {
            kind: REG_TYPE_DWORD,
            data: v.to_le_bytes().to_vec(),
        }
    }

    /// 字串型別（`REG_SZ`／`REG_EXPAND_SZ`）解碼成字串（去掉結尾 NUL，不展開環境變數）；
    /// 其他型別、不存在、或無效 UTF-16 回 `None`。只供顯示與診斷，還原一律用原始位元組。
    pub fn as_string(&self) -> Option<String> {
        match self {
            Self::Present { kind, data } if *kind == REG_TYPE_SZ || *kind == REG_TYPE_EXPAND_SZ => {
                if data.len() % 2 != 0 {
                    return None;
                }
                let units: Vec<u16> = data
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
                String::from_utf16(&units[..end]).ok()
            }
            _ => None,
        }
    }

    /// `REG_DWORD`（恰 4 位元組）解碼；其他情況回 `None`。
    pub fn as_dword(&self) -> Option<u32> {
        match self {
            Self::Present { kind, data } if *kind == REG_TYPE_DWORD && data.len() == 4 => {
                Some(u32::from_le_bytes([data[0], data[1], data[2], data[3]]))
            }
            _ => None,
        }
    }
}

/// 登錄存取失敗。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryError(pub String);

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "登錄存取失敗：{}", self.0)
    }
}

impl std::error::Error for RegistryError {}

/// 登錄存取的最小介面（`HKCU` 下）。測試以記憶體內的假實作取代。
pub trait RegistryStore {
    /// 讀值；值或機碼不存在回 [`RegValue::Missing`]，任何型別都照原樣回傳。
    fn get(&self, subkey: &str, name: &str) -> Result<RegValue, RegistryError>;
    /// 以指定型別寫入原始位元組（機碼不存在時建立）。
    fn set(
        &mut self,
        subkey: &str,
        name: &str,
        kind: u32,
        data: &[u8],
    ) -> Result<(), RegistryError>;
    /// 刪除值；值或機碼本來就不存在視為成功。
    fn delete(&mut self, subkey: &str, name: &str) -> Result<(), RegistryError>;
}

/// `HKCU\Control Panel\Desktop` 三個值的忠實快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopRegistrySnapshot {
    pub wallpaper: RegValue,
    pub wallpaper_style: RegValue,
    pub tile_wallpaper: RegValue,
}

impl DesktopRegistrySnapshot {
    fn values(&self) -> [(&'static str, &RegValue); 3] {
        [
            (DESKTOP_VALUE_NAMES[0], &self.wallpaper),
            (DESKTOP_VALUE_NAMES[1], &self.wallpaper_style),
            (DESKTOP_VALUE_NAMES[2], &self.tile_wallpaper),
        ]
    }
}

/// Windows 焦點相關登錄值（design.md D5；盡力而為，讀不到或型別不是 `REG_DWORD` 為 `None`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SpotlightState {
    /// `HKCU\Software\Microsoft\Windows\CurrentVersion\DesktopSpotlight\Settings\EnabledState`。
    pub enabled_state: Option<u32>,
    /// `HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\Wallpapers\BackgroundType`
    /// （只供診斷）。
    pub background_type: Option<u32>,
}

impl SpotlightState {
    /// 是否判定為 Windows 焦點：只看 `EnabledState`＝1（機碼在停用後仍殘留，不可用「存在」判定）。
    pub fn is_spotlight(&self) -> bool {
        self.enabled_state == Some(1)
    }
}

/// 從 `store` 取三個值的快照；任一值讀取失敗（不含「不存在」）即回錯＝記錄失敗、不得接管。
pub fn snapshot_desktop_registry_in(
    store: &dyn RegistryStore,
) -> Result<DesktopRegistrySnapshot, RegistryError> {
    Ok(DesktopRegistrySnapshot {
        wallpaper: store.get(DESKTOP_KEY, DESKTOP_VALUE_NAMES[0])?,
        wallpaper_style: store.get(DESKTOP_KEY, DESKTOP_VALUE_NAMES[1])?,
        tile_wallpaper: store.get(DESKTOP_KEY, DESKTOP_VALUE_NAMES[2])?,
    })
}

/// 把三個值逐一還原成快照：`Present` 以原型別寫回原位元組，`Missing` 刪除。某個值失敗時仍繼續
/// 處理其餘的值，最後回報全部失敗（盡量還原越多越好）。
pub fn restore_desktop_registry_in(
    store: &mut dyn RegistryStore,
    snapshot: &DesktopRegistrySnapshot,
) -> Result<(), RegistryError> {
    let mut failures = Vec::new();
    for (name, value) in snapshot.values() {
        let result = match value {
            RegValue::Missing => store.delete(DESKTOP_KEY, name),
            RegValue::Present { kind, data } => store.set(DESKTOP_KEY, name, *kind, data),
        };
        if let Err(e) = result {
            failures.push(format!("{name}：{}", e.0));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(RegistryError(format!(
            "還原桌布登錄值失敗（其餘值已處理）：{}",
            failures.join("；")
        )))
    }
}

/// 讀 Windows 焦點相關值（盡力而為：失敗或型別不符一律 `None`）。
pub fn read_spotlight_in(store: &dyn RegistryStore) -> SpotlightState {
    let dword = |subkey: &str, name: &str| store.get(subkey, name).ok().and_then(|v| v.as_dword());
    SpotlightState {
        enabled_state: dword(SPOTLIGHT_KEY, "EnabledState"),
        background_type: dword(WALLPAPERS_KEY, "BackgroundType"),
    }
}

/// [`snapshot_desktop_registry_in`] 對真實 `HKCU`。
pub fn snapshot_desktop_registry() -> Result<DesktopRegistrySnapshot, RegistryError> {
    snapshot_desktop_registry_in(&HkcuRegistry)
}

/// [`restore_desktop_registry_in`] 對真實 `HKCU`。
pub fn restore_desktop_registry(snapshot: &DesktopRegistrySnapshot) -> Result<(), RegistryError> {
    restore_desktop_registry_in(&mut HkcuRegistry, snapshot)
}

/// [`read_spotlight_in`] 對真實 `HKCU`。
pub fn read_spotlight() -> SpotlightState {
    read_spotlight_in(&HkcuRegistry)
}

// ---------------------------------------------------------------------------------------------
// 真實實作：HKCU
// ---------------------------------------------------------------------------------------------

/// 真實的 `HKCU` 登錄存取。讀取用 `RegOpenKeyExW`＋`RegQueryValueExW`（取原始位元組，不像
/// `RegGetValueW` 會補結尾 NUL 或限制型別）。
#[derive(Debug, Default, Clone, Copy)]
pub struct HkcuRegistry;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 開啟後自動關閉的機碼把手。
struct OpenKey(HKEY);

impl Drop for OpenKey {
    fn drop(&mut self) {
        // SAFETY: self.0 由成功的 RegOpenKeyExW 取得，只關閉一次。
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

impl RegistryStore for HkcuRegistry {
    fn get(&self, subkey: &str, name: &str) -> Result<RegValue, RegistryError> {
        let subkey_w = wide(subkey);
        let name_w = wide(name);
        let mut key = HKEY::default();
        // SAFETY: subkey_w 以 NUL 結尾且在呼叫期間存活；key 為可寫區域變數。
        let st = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(subkey_w.as_ptr()),
                None,
                KEY_QUERY_VALUE,
                &mut key,
            )
        };
        if st == ERROR_FILE_NOT_FOUND {
            return Ok(RegValue::Missing);
        }
        if st != ERROR_SUCCESS {
            return Err(RegistryError(format!(
                "RegOpenKeyExW({subkey}) 失敗：{st:?}"
            )));
        }
        let key = OpenKey(key);
        let mut len: u32 = 0;
        // 值可能在兩次呼叫之間變長（ERROR_MORE_DATA），重試數次。
        for _ in 0..4 {
            let mut kind = REG_VALUE_TYPE::default();
            let mut buf = vec![0u8; len as usize];
            let mut cb = len;
            let data_ptr = if buf.is_empty() {
                None
            } else {
                Some(buf.as_mut_ptr())
            };
            // SAFETY: name_w 以 NUL 結尾；buf 長度與 cb 一致（為空時不傳緩衝區、只查大小與型別）。
            let st = unsafe {
                RegQueryValueExW(
                    key.0,
                    PCWSTR(name_w.as_ptr()),
                    None,
                    Some(&mut kind),
                    data_ptr,
                    Some(&mut cb),
                )
            };
            if st == ERROR_FILE_NOT_FOUND {
                return Ok(RegValue::Missing);
            }
            if st == ERROR_MORE_DATA || (st == ERROR_SUCCESS && buf.is_empty() && cb > 0) {
                len = cb;
                continue;
            }
            if st != ERROR_SUCCESS {
                return Err(RegistryError(format!(
                    "RegQueryValueExW({subkey}\\{name}) 失敗：{st:?}"
                )));
            }
            buf.truncate(cb as usize);
            return Ok(RegValue::Present {
                kind: kind.0,
                data: buf,
            });
        }
        Err(RegistryError(format!(
            "RegQueryValueExW({subkey}\\{name}) 反覆回報緩衝區不足"
        )))
    }

    fn set(
        &mut self,
        subkey: &str,
        name: &str,
        kind: u32,
        data: &[u8],
    ) -> Result<(), RegistryError> {
        let subkey_w = wide(subkey);
        let name_w = wide(name);
        let ptr = (!data.is_empty()).then_some(data.as_ptr() as *const c_void);
        // SAFETY: 機碼與值名以 NUL 結尾；data 在呼叫期間存活，cbdata 為其位元組長度。
        let st = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                PCWSTR(subkey_w.as_ptr()),
                PCWSTR(name_w.as_ptr()),
                kind,
                ptr,
                data.len() as u32,
            )
        };
        if st == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(RegistryError(format!(
                "RegSetKeyValueW({subkey}\\{name}) 失敗：{st:?}"
            )))
        }
    }

    fn delete(&mut self, subkey: &str, name: &str) -> Result<(), RegistryError> {
        let subkey_w = wide(subkey);
        let name_w = wide(name);
        // SAFETY: 機碼與值名以 NUL 結尾。
        let st = unsafe {
            RegDeleteKeyValueW(
                HKEY_CURRENT_USER,
                PCWSTR(subkey_w.as_ptr()),
                PCWSTR(name_w.as_ptr()),
            )
        };
        if st == ERROR_SUCCESS || st == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(RegistryError(format!(
                "RegDeleteKeyValueW({subkey}\\{name}) 失敗：{st:?}"
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    /// 記憶體內的假登錄；可指定某些值寫入時失敗。
    #[derive(Default)]
    struct FakeRegistry {
        values: BTreeMap<(String, String), (u32, Vec<u8>)>,
        fail_set: BTreeSet<String>,
        fail_get: BTreeSet<String>,
    }

    impl FakeRegistry {
        fn put(&mut self, subkey: &str, name: &str, v: RegValue) {
            match v {
                RegValue::Missing => {
                    self.values.remove(&(subkey.to_owned(), name.to_owned()));
                }
                RegValue::Present { kind, data } => {
                    self.values
                        .insert((subkey.to_owned(), name.to_owned()), (kind, data));
                }
            }
        }
        fn value(&self, subkey: &str, name: &str) -> RegValue {
            self.get(subkey, name).expect("假登錄讀取不應失敗")
        }
    }

    impl RegistryStore for FakeRegistry {
        fn get(&self, subkey: &str, name: &str) -> Result<RegValue, RegistryError> {
            if self.fail_get.contains(name) {
                return Err(RegistryError(format!("假讀取失敗：{name}")));
            }
            Ok(
                match self.values.get(&(subkey.to_owned(), name.to_owned())) {
                    Some((kind, data)) => RegValue::Present {
                        kind: *kind,
                        data: data.clone(),
                    },
                    None => RegValue::Missing,
                },
            )
        }
        fn set(
            &mut self,
            subkey: &str,
            name: &str,
            kind: u32,
            data: &[u8],
        ) -> Result<(), RegistryError> {
            if self.fail_set.contains(name) {
                return Err(RegistryError(format!("假寫入失敗：{name}")));
            }
            self.values
                .insert((subkey.to_owned(), name.to_owned()), (kind, data.to_vec()));
            Ok(())
        }
        fn delete(&mut self, subkey: &str, name: &str) -> Result<(), RegistryError> {
            self.values.remove(&(subkey.to_owned(), name.to_owned()));
            Ok(())
        }
    }

    fn original() -> FakeRegistry {
        let mut r = FakeRegistry::default();
        r.put(
            DESKTOP_KEY,
            "Wallpaper",
            RegValue::string(
                REG_TYPE_EXPAND_SZ,
                "%SystemRoot%\\Web\\Wallpaper\\Windows\\img0.jpg",
            ),
        );
        r.put(
            DESKTOP_KEY,
            "WallpaperStyle",
            RegValue::string(REG_TYPE_SZ, "10"),
        );
        // TileWallpaper 原本不存在。
        r
    }

    #[test]
    fn snapshot_preserves_type_bytes_and_missing() {
        let r = original();
        let snap = snapshot_desktop_registry_in(&r).expect("快照應成功");
        assert_eq!(
            snap.wallpaper,
            RegValue::string(
                REG_TYPE_EXPAND_SZ,
                "%SystemRoot%\\Web\\Wallpaper\\Windows\\img0.jpg"
            )
        );
        assert_eq!(
            snap.wallpaper.as_string().as_deref(),
            Some("%SystemRoot%\\Web\\Wallpaper\\Windows\\img0.jpg"),
            "不展開環境變數"
        );
        assert_eq!(snap.wallpaper_style, RegValue::string(REG_TYPE_SZ, "10"));
        assert_eq!(snap.tile_wallpaper, RegValue::Missing);
    }

    #[test]
    fn non_string_value_does_not_fail_snapshot() {
        let mut r = original();
        r.put(DESKTOP_KEY, "TileWallpaper", RegValue::dword(0));
        let snap = snapshot_desktop_registry_in(&r).expect("非字串型別不應讓快照失敗");
        assert_eq!(snap.tile_wallpaper, RegValue::dword(0));
        assert_eq!(snap.tile_wallpaper.as_string(), None);
    }

    #[test]
    fn snapshot_read_failure_is_error() {
        let mut r = original();
        r.fail_get.insert("WallpaperStyle".to_owned());
        assert!(snapshot_desktop_registry_in(&r).is_err());
    }

    #[test]
    fn restore_brings_back_exact_type_bytes_and_deletes_originally_missing() {
        let mut r = original();
        let snap = snapshot_desktop_registry_in(&r).expect("快照應成功");
        // 模擬 SetWallpaper 的副作用與其他改動。
        r.put(
            DESKTOP_KEY,
            "Wallpaper",
            RegValue::string(
                REG_TYPE_SZ,
                "C:\\Users\\u\\AppData\\Roaming\\Microsoft\\Windows\\Themes\\TranscodedWallpaper",
            ),
        );
        r.put(
            DESKTOP_KEY,
            "WallpaperStyle",
            RegValue::string(REG_TYPE_SZ, "6"),
        );
        r.put(
            DESKTOP_KEY,
            "TileWallpaper",
            RegValue::string(REG_TYPE_SZ, "0"),
        );

        restore_desktop_registry_in(&mut r, &snap).expect("還原應成功");
        assert_eq!(
            r.value(DESKTOP_KEY, "Wallpaper"),
            snap.wallpaper,
            "型別也要還原成 REG_EXPAND_SZ"
        );
        assert_eq!(r.value(DESKTOP_KEY, "WallpaperStyle"), snap.wallpaper_style);
        assert_eq!(
            r.value(DESKTOP_KEY, "TileWallpaper"),
            RegValue::Missing,
            "原本不存在的值要刪除"
        );
        assert_eq!(snapshot_desktop_registry_in(&r).unwrap(), snap);
    }

    #[test]
    fn restore_continues_after_one_value_fails_and_reports_error() {
        let mut r = original();
        let snap = snapshot_desktop_registry_in(&r).expect("快照應成功");
        r.put(DESKTOP_KEY, "Wallpaper", RegValue::string(REG_TYPE_SZ, "X"));
        r.put(
            DESKTOP_KEY,
            "WallpaperStyle",
            RegValue::string(REG_TYPE_SZ, "6"),
        );
        r.put(
            DESKTOP_KEY,
            "TileWallpaper",
            RegValue::string(REG_TYPE_SZ, "1"),
        );
        r.fail_set.insert("Wallpaper".to_owned());

        let err = restore_desktop_registry_in(&mut r, &snap).unwrap_err();
        assert!(err.0.contains("Wallpaper"), "{err}");
        assert_eq!(r.value(DESKTOP_KEY, "WallpaperStyle"), snap.wallpaper_style);
        assert_eq!(r.value(DESKTOP_KEY, "TileWallpaper"), RegValue::Missing);
    }

    #[test]
    fn spotlight_reads_dwords_best_effort() {
        let mut r = FakeRegistry::default();
        assert_eq!(read_spotlight_in(&r), SpotlightState::default());
        r.put(SPOTLIGHT_KEY, "EnabledState", RegValue::dword(1));
        r.put(WALLPAPERS_KEY, "BackgroundType", RegValue::dword(3));
        let s = read_spotlight_in(&r);
        assert_eq!(s.enabled_state, Some(1));
        assert_eq!(s.background_type, Some(3));
        assert!(s.is_spotlight());

        r.put(
            SPOTLIGHT_KEY,
            "EnabledState",
            RegValue::string(REG_TYPE_SZ, "1"),
        );
        assert_eq!(
            read_spotlight_in(&r).enabled_state,
            None,
            "型別不是 DWORD 視為讀不到"
        );
        r.fail_get.insert("BackgroundType".to_owned());
        assert_eq!(
            read_spotlight_in(&r).background_type,
            None,
            "讀取失敗視為讀不到"
        );
    }

    #[test]
    fn spotlight_detection_uses_enabled_state_only() {
        let s = |e, b| SpotlightState {
            enabled_state: e,
            background_type: b,
        };
        assert!(s(Some(1), None).is_spotlight());
        assert!(
            !s(Some(0), Some(3)).is_spotlight(),
            "BackgroundType 只供診斷"
        );
        assert!(!s(None, None).is_spotlight());
    }

    #[test]
    fn reg_value_decoding() {
        assert_eq!(RegValue::Missing.as_string(), None);
        assert_eq!(
            RegValue::string(REG_TYPE_SZ, "").as_string().as_deref(),
            Some("")
        );
        assert_eq!(RegValue::dword(7).as_dword(), Some(7));
        assert_eq!(RegValue::string(REG_TYPE_SZ, "7").as_dword(), None);
        let lone_surrogate = RegValue::Present {
            kind: REG_TYPE_SZ,
            data: vec![0x00, 0xD8, 0, 0],
        };
        assert_eq!(
            lone_surrogate.as_string(),
            None,
            "無效 UTF-16 不得變成空字串"
        );
    }
}
