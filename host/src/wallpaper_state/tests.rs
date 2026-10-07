//! 狀態檔與接管狀態機的單元測試。一律用假的桌布後端（經真正的 `WallpaperService` 工作執行緒）、
//! 記憶體內的假登錄與暫存資料夾：**不**碰真正的 `%APPDATA%`／`%LOCALAPPDATA%`、HKCU，也不改桌布。

use super::*;

use std::env;
use std::fs;
use std::sync::{Arc, Mutex};

use crate::desktop::wallpaper::file_identity::{FileId, FileIdentity, NoFileIdentity};
use crate::desktop::wallpaper::registry::{
    RegistryError, DESKTOP_KEY, REG_TYPE_EXPAND_SZ, REG_TYPE_SZ, SPOTLIGHT_KEY, WALLPAPERS_KEY,
};
use crate::desktop::wallpaper::{
    BackendError, MonitorEntry, MonitorWallpaper, SlideshowInfo, WallpaperBackend,
    WallpaperPosition, WallpaperServiceConfig, WallpaperSnapshot, SLIDESHOW_OPTION_SHUFFLE,
    SLIDESHOW_STATE_ENABLED, SLIDESHOW_STATE_SLIDESHOW,
};
use crate::layout::PhysicalRect;

const DEV_A: &str =
    "\\\\?\\DISPLAY#BOE0CDF#4&102fce2&0&UID8388688#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";
const DEV_B: &str =
    "\\\\?\\DISPLAY#AUSAA34#5&1091fafa&0&UID4356#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";
const ORIGINAL_COLOR: u32 = 0x0011_2233;
const ORIGINAL_POSITION: i32 = 3; // FIT：與宿主設定的 FILL 不同，才看得出還原
const TRANSCODED: &str =
    "C:\\Users\\u\\AppData\\Roaming\\Microsoft\\Windows\\Themes\\TranscodedWallpaper";

// ---------------------------------------------------------------------------------------------
// 假的 explorer（桌布後端）
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct FakeMonitor {
    device_path: String,
    rect: Option<PhysicalRect>,
    wallpaper: Option<String>,
}

#[derive(Debug, Clone)]
struct Desk {
    monitors: Vec<FakeMonitor>,
    position: i32,
    color: u32,
    slideshow_status: i32,
    slideshow: Option<SlideshowInfo>,
}

impl Desk {
    fn wallpaper(&self, device: &str) -> Option<String> {
        self.monitors
            .iter()
            .find(|m| m.device_path == device)
            .and_then(|m| m.wallpaper.clone())
    }
}

#[derive(Debug)]
struct Shared {
    desk: Desk,
    /// 後端收到的請求（依序）。
    calls: Vec<String>,
    /// 之後的 N 次 `read` 失敗。
    fail_reads: u32,
    /// `Some(n)`：之後先有 n 次 `read` 成功，其後一律失敗（直到改回 `None`）。
    ok_reads_before_fail: Option<u32>,
    /// `set_wallpaper` 被接受但沒有效果（explorer 沒處理）。
    ignore_sets: bool,
    /// 讀回時把路徑轉成大寫（Unicode，含 `Ö`）——模擬讀回的大小寫與宿主記錄的不同。
    upper_case_readback: bool,
    /// 每次 `set_wallpaper`／`set_position` 被呼叫時，狀態檔當下的內容（驗證先寫後設）。
    state_file: Option<PathBuf>,
    state_at_set: Vec<Option<String>>,
    /// `Some`：模擬 explorer 非同步改寫登錄（修正輪 2：改成確定性，不靠計時，見 [`ExplorerSim`]）。
    explorer_registry: Option<Arc<Mutex<ExplorerSim>>>,
    /// 對這個裝置路徑的 `set_wallpaper` 回 COM 錯誤（確定失敗，沒有任何效果）。
    fail_set_device: Option<String>,
    /// `Some`：照 6.1 實機（B9）模擬 explorer 的純色語意——純色是**全域**狀態。值是 explorer 記憶中的
    /// 逐螢幕圖片（裝置路徑 → 路徑）：
    /// - `set_solid_color`（`SetWallpaper(NULL, "")`）→ 所有在線螢幕讀回空字串、`GetStatus` 變 0；
    /// - 純色狀態下只設一台圖片 → 其餘在線螢幕讀回立刻變成記憶中的圖片；
    /// - 逐螢幕設空字串 → 那台換回記憶中的圖片（不會變純色）。
    ///
    /// `None`：舊的簡化模型（逐螢幕空字串＝那台純色）。
    explorer_memory: Option<BTreeMap<String, String>>,
    /// 修正輪 4（複審 N1，6.1 重跑 2b 實機時序）：`explorer_memory` 下逐螢幕設空字串時，**第一次**讀回仍是空字串，
    /// 那次讀完才換成記憶中的圖片（實機是數秒後）。`false`＝立刻換成記憶中的圖片。
    delayed_memory_swap: bool,
    /// `delayed_memory_swap` 排定、下一次讀回之後才生效的換圖（裝置路徑, 圖）。
    pending_memory_swap: Vec<(String, String)>,
}

/// explorer 非同步改寫登錄的確定性模擬（修正輪 2：原本以執行緒＋60 ms 計時，高負載下偶發失敗）。
///
/// 每次生效的設定（`set_wallpaper`／`set_solid_color`）只記一筆「待改寫」；explorer 的改寫在之後第
/// `polls_before_rewrite` 次讀登錄 `Wallpaper` 時才發生（＝「處理需要一段時間」，但不依賴時鐘），一次處理完
/// 所有待改寫。`events` 依序記下 explorer 的改寫與還原寫回登錄 `Wallpaper` 的時刻，測試據此斷言順序。
#[derive(Debug)]
struct ExplorerSim {
    values: Arc<Mutex<RegMap>>,
    /// explorer 會寫成的值。
    rewrite_to: String,
    pending: u32,
    polls: u32,
    polls_before_rewrite: u32,
    events: Vec<&'static str>,
    /// 下一次改寫要寫成的值（`None`＝`rewrite_to`）：「全部螢幕」設定時 explorer 寫成原圖路徑、不是轉存檔（實機，
    /// memory `setwallpaper-side-effects`）。
    pending_target: Option<String>,
    /// 最近一次 explorer 改寫寫入的值（測試據此斷言 explorer 寫的是什麼）。
    last_rewrite: Option<String>,
}

impl ExplorerSim {
    fn on_set(&mut self) {
        self.pending += 1;
        self.polls = 0;
    }

    /// 「全部螢幕」設定：explorer 把登錄 `Wallpaper` 寫成這張圖的路徑（不是 `TranscodedWallpaper`）。
    fn on_set_all(&mut self, image: &str) {
        self.on_set();
        self.pending_target = Some(image.to_owned());
    }

    fn on_poll(&mut self) {
        if self.pending == 0 {
            return;
        }
        self.polls += 1;
        if self.polls < self.polls_before_rewrite {
            return;
        }
        let target = self
            .pending_target
            .take()
            .unwrap_or_else(|| self.rewrite_to.clone());
        let RegValue::Present { kind, data } = RegValue::string(REG_TYPE_SZ, &target) else {
            return;
        };
        self.last_rewrite = Some(target);
        self.values.lock().unwrap().insert(
            (DESKTOP_KEY.to_owned(), "Wallpaper".to_owned()),
            (kind, data),
        );
        self.pending = 0;
        self.events.push("explorer-rewrite");
    }
}

impl Shared {
    /// 實機語意下，目前是否為全域純色（所有在線螢幕都讀回空字串）。
    fn is_solid(&self) -> bool {
        let mut online = self.desk.monitors.iter().filter(|m| m.rect.is_some());
        let mut any = false;
        let all = online.all(|m| {
            any = true;
            m.wallpaper.as_deref() == Some("")
        });
        any && all
    }

    /// 實機語意：離開純色時，`except` 以外的在線螢幕換回 explorer 記憶中的圖片。
    fn leave_solid(&mut self, except: Option<&str>) {
        let Some(memory) = self.explorer_memory.clone() else {
            return;
        };
        for m in self.desk.monitors.iter_mut().filter(|m| m.rect.is_some()) {
            if Some(m.device_path.as_str()) == except {
                continue;
            }
            if let Some(img) = memory.get(&m.device_path) {
                m.wallpaper = Some(img.clone());
            }
        }
        self.desk.slideshow_status |= SLIDESHOW_STATE_ENABLED;
    }
}

struct FakeBackend {
    shared: Arc<Mutex<Shared>>,
}

impl FakeBackend {
    fn capture_state(s: &mut Shared) {
        let content = s
            .state_file
            .as_ref()
            .and_then(|p| fs::read_to_string(p).ok());
        s.state_at_set.push(content);
    }
}

impl WallpaperBackend for FakeBackend {
    fn is_ready(&self) -> bool {
        true
    }
    fn recreate(&mut self) -> Result<(), BackendError> {
        Ok(())
    }
    fn explorer_pid(&self) -> Option<u32> {
        Some(1)
    }
    fn list_monitors(&mut self) -> Result<Vec<MonitorEntry>, BackendError> {
        let s = self.shared.lock().unwrap();
        Ok(s.desk
            .monitors
            .iter()
            .map(|m| MonitorEntry {
                device_path: m.device_path.clone(),
                rect: m.rect,
            })
            .collect())
    }
    fn read(&mut self) -> Result<WallpaperSnapshot, BackendError> {
        let mut s = self.shared.lock().unwrap();
        s.calls.push("read".to_owned());
        if s.fail_reads > 0 {
            s.fail_reads -= 1;
            return Err(BackendError::Com("假讀取失敗".to_owned()));
        }
        match s.ok_reads_before_fail {
            Some(0) => return Err(BackendError::Com("假讀取失敗（之後一律失敗）".to_owned())),
            Some(n) => s.ok_reads_before_fail = Some(n - 1),
            None => {}
        }
        let upper = s.upper_case_readback;
        let swaps = std::mem::take(&mut s.pending_memory_swap);
        let d = s.desk.clone();
        for (device, img) in swaps {
            if let Some(m) = s.desk.monitors.iter_mut().find(|m| m.device_path == device) {
                m.wallpaper = Some(img);
            }
        }
        Ok(WallpaperSnapshot {
            monitors: d
                .monitors
                .iter()
                .map(|m| MonitorWallpaper {
                    monitor: MonitorEntry {
                        device_path: m.device_path.clone(),
                        rect: m.rect,
                    },
                    wallpaper: m
                        .wallpaper
                        .clone()
                        .map(|w| if upper { w.to_uppercase() } else { w }),
                })
                .collect(),
            position: WallpaperPosition(d.position),
            background_color: d.color,
            slideshow_status: d.slideshow_status,
            slideshow: d.slideshow.clone(),
        })
    }
    fn set_wallpaper(&mut self, device_path: &str, image: &Path) -> Result<(), BackendError> {
        let mut s = self.shared.lock().unwrap();
        s.calls
            .push(format!("set_wallpaper {device_path} {}", image.display()));
        Self::capture_state(&mut s);
        if s.fail_set_device.as_deref() == Some(device_path) {
            return Err(BackendError::Com("假 SetWallpaper 失敗".to_owned()));
        }
        if s.ignore_sets {
            return Ok(());
        }
        let mut image = image.to_string_lossy().into_owned();
        if s.explorer_memory.is_some() {
            if image.is_empty() {
                // 逐螢幕空字串＝換回記憶中的圖片（6.1 B9），不會變純色。
                let was_solid = s.is_solid();
                if let Some(img) = s
                    .explorer_memory
                    .as_ref()
                    .and_then(|mem| mem.get(device_path))
                    .cloned()
                {
                    if s.delayed_memory_swap {
                        s.pending_memory_swap.push((device_path.to_owned(), img));
                    } else {
                        image = img;
                    }
                }
                if was_solid {
                    s.leave_solid(Some(device_path));
                }
            } else if s.is_solid() {
                s.leave_solid(Some(device_path));
            }
        }
        if let Some(m) = s
            .desk
            .monitors
            .iter_mut()
            .find(|m| m.device_path == device_path)
        {
            m.wallpaper = Some(image);
        }
        if let Some(sim) = &s.explorer_registry {
            sim.lock().unwrap().on_set();
        }
        // 設定單張圖片會結束投影片。
        s.desk.slideshow_status &= !SLIDESHOW_STATE_SLIDESHOW;
        s.desk.slideshow = None;
        Ok(())
    }
    fn set_wallpaper_all(&mut self, image: &Path) -> Result<(), BackendError> {
        let mut s = self.shared.lock().unwrap();
        s.calls
            .push(format!("set_wallpaper_all {}", image.display()));
        Self::capture_state(&mut s);
        if s.ignore_sets {
            return Ok(());
        }
        // 「全部螢幕」設定：所有在線螢幕讀回同一張圖，投影片結束。
        let image = image.to_string_lossy().into_owned();
        for m in s.desk.monitors.iter_mut().filter(|m| m.rect.is_some()) {
            m.wallpaper = Some(image.clone());
        }
        s.desk.slideshow_status &= !SLIDESHOW_STATE_SLIDESHOW;
        s.desk.slideshow = None;
        if let Some(sim) = &s.explorer_registry {
            sim.lock().unwrap().on_set_all(&image);
        }
        Ok(())
    }
    fn set_solid_color(&mut self) -> Result<(), BackendError> {
        let mut s = self.shared.lock().unwrap();
        s.calls.push("set_solid_color".to_owned());
        Self::capture_state(&mut s);
        if s.ignore_sets {
            return Ok(());
        }
        for m in s.desk.monitors.iter_mut().filter(|m| m.rect.is_some()) {
            m.wallpaper = Some(String::new());
        }
        // 6.1 B9：純色時 GetStatus 回 0；投影片也隨之結束。
        s.desk.slideshow_status = 0;
        s.desk.slideshow = None;
        if let Some(sim) = &s.explorer_registry {
            sim.lock().unwrap().on_set();
        }
        Ok(())
    }
    fn set_position(&mut self, position: WallpaperPosition) -> Result<(), BackendError> {
        let mut s = self.shared.lock().unwrap();
        s.calls.push(format!("set_position {}", position.0));
        Self::capture_state(&mut s);
        if !s.ignore_sets {
            s.desk.position = position.0;
        }
        Ok(())
    }
    fn set_background_color(&mut self, colorref: u32) -> Result<(), BackendError> {
        let mut s = self.shared.lock().unwrap();
        s.calls.push(format!("set_background_color {colorref:#x}"));
        if !s.ignore_sets {
            s.desk.color = colorref;
        }
        Ok(())
    }
    fn restore_slideshow(&mut self, slideshow: &SlideshowInfo) -> Result<(), BackendError> {
        let mut s = self.shared.lock().unwrap();
        s.calls.push("restore_slideshow".to_owned());
        if s.ignore_sets {
            return Ok(());
        }
        s.desk.slideshow_status |= SLIDESHOW_STATE_SLIDESHOW;
        s.desk.slideshow = Some(slideshow.clone());
        let first = format!("{}\\slide-1.jpg", slideshow.items[0]);
        for m in s.desk.monitors.iter_mut().filter(|m| m.rect.is_some()) {
            m.wallpaper = Some(first.clone());
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// 假登錄
// ---------------------------------------------------------------------------------------------

type RegMap = BTreeMap<(String, String), (u32, Vec<u8>)>;

#[derive(Default)]
struct FakeRegistry {
    values: Arc<Mutex<RegMap>>,
    /// 寫入與刪除的紀錄（驗證讓位後不呼叫登錄還原）。
    writes: Vec<String>,
    /// 修正輪 2：explorer 非同步改寫的確定性模擬（讀 `Wallpaper` 時推進，見 [`ExplorerSim`]）。
    explorer: Option<Arc<Mutex<ExplorerSim>>>,
}

impl FakeRegistry {
    fn put(&mut self, subkey: &str, name: &str, v: RegValue) {
        match v {
            RegValue::Missing => {
                self.values
                    .lock()
                    .unwrap()
                    .remove(&(subkey.to_owned(), name.to_owned()));
            }
            RegValue::Present { kind, data } => {
                self.values
                    .lock()
                    .unwrap()
                    .insert((subkey.to_owned(), name.to_owned()), (kind, data));
            }
        }
    }
    fn shared(&self) -> Arc<Mutex<RegMap>> {
        Arc::clone(&self.values)
    }
    fn value(&self, subkey: &str, name: &str) -> RegValue {
        self.get(subkey, name).unwrap()
    }
}

impl RegistryStore for FakeRegistry {
    fn get(&self, subkey: &str, name: &str) -> Result<RegValue, RegistryError> {
        if subkey == DESKTOP_KEY && name == "Wallpaper" {
            if let Some(sim) = &self.explorer {
                sim.lock().unwrap().on_poll();
            }
        }
        Ok(
            match self
                .values
                .lock()
                .unwrap()
                .get(&(subkey.to_owned(), name.to_owned()))
            {
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
        self.writes.push(format!("set {subkey}\\{name}"));
        if subkey == DESKTOP_KEY && name == "Wallpaper" {
            if let Some(sim) = &self.explorer {
                sim.lock().unwrap().events.push("restore-write");
            }
        }
        self.values
            .lock()
            .unwrap()
            .insert((subkey.to_owned(), name.to_owned()), (kind, data.to_vec()));
        Ok(())
    }
    fn delete(&mut self, subkey: &str, name: &str) -> Result<(), RegistryError> {
        self.writes.push(format!("delete {subkey}\\{name}"));
        self.values
            .lock()
            .unwrap()
            .remove(&(subkey.to_owned(), name.to_owned()));
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// 假的檔案識別
// ---------------------------------------------------------------------------------------------

/// 路徑（小寫）→ 識別；沒登記的查不到。
#[derive(Default)]
struct FakeIds(BTreeMap<String, FileId>);

impl FakeIds {
    fn add(&mut self, path: &str, index: u64) {
        self.0.insert(
            path.to_lowercase(),
            FileId {
                volume_serial: 7,
                file_index: index,
            },
        );
    }
}

impl FileIdentity for FakeIds {
    fn file_id(&self, path: &Path) -> Option<FileId> {
        self.0.get(&path.to_string_lossy().to_lowercase()).copied()
    }
}

// ---------------------------------------------------------------------------------------------
// 測試台
// ---------------------------------------------------------------------------------------------

fn rect(x: i32) -> PhysicalRect {
    PhysicalRect {
        x,
        y: 0,
        width: 3840,
        height: 2160,
    }
}

struct Harness {
    root: PathBuf,
    paths: StatePaths,
    pictures: PathBuf,
    shared: Arc<Mutex<Shared>>,
    service: WallpaperService,
    registry: FakeRegistry,
    sched: SchedulerState,
    now: i64,
    /// 傳給 `TakeoverIo::stable_displays`。
    stable: Vec<StableDisplay>,
    /// 傳給 `TakeoverIo::files`（預設全部查不到＝只靠路徑比對）。
    files: FakeIds,
    /// 接管前登錄 `Wallpaper` 的原值（[`Self::original_registry`] 用；預設是 `sunset.jpg` 的小寫路徑＝「全部
    /// 螢幕」設定，[`Self::per_monitor_setting`] 改成 Themes 的 `TranscodedWallpaper`）。
    original_wallpaper_reg: RegValue,
}

const ORIGINAL_BYTES: &[u8] = b"ORIGINAL-JPEG-BYTES";
/// explorer 快取的轉存檔內容（JPEG 檔頭＋標記；task 6.4：原圖不存在時改備份它）。
const TRANSCODED_BYTES: &[u8] = b"\xFF\xD8\xFF\xE0TRANSCODED-CACHE";

/// 逐螢幕轉存檔的假內容（修正輪 3，6.1 重跑實機：格式與尺寸沿用原圖，不是「PNG＋螢幕解析度」）：
/// 000＝JPEG、001＝非螢幕尺寸（3840×2400，螢幕是 3840×2160）的 PNG。
fn monitor_transcoded_bytes(i: usize) -> Vec<u8> {
    match i {
        0 => b"\xFF\xD8\xFF\xE0MONITOR-0".to_vec(),
        _ => png_bytes(3840, 2400, &format!("MONITOR-{i}")),
    }
}

/// 最小的 PNG 檔頭（簽章＋IHDR 的寬高）加上可辨識的標記。
fn png_bytes(width: u32, height: u32, tag: &str) -> Vec<u8> {
    let mut v = b"\x89PNG\r\n\x1a\n".to_vec();
    v.extend_from_slice(&13u32.to_be_bytes());
    v.extend_from_slice(b"IHDR");
    v.extend_from_slice(&width.to_be_bytes());
    v.extend_from_slice(&height.to_be_bytes());
    v.extend_from_slice(&[8, 6, 0, 0, 0]);
    v.extend_from_slice(tag.as_bytes());
    v
}

impl Harness {
    /// 兩台在線螢幕，原桌布都是使用者的 `sunset.jpg`；暫存資料夾路徑含非 ASCII 字母。
    fn new(name: &str) -> Self {
        let root = env::temp_dir()
            .join(format!("fc-host-wpstate-{}-{name}", std::process::id()))
            .join("ölaf");
        let _ = fs::remove_dir_all(root.parent().unwrap());
        let pictures = root.join("Pictures");
        fs::create_dir_all(&pictures).unwrap();
        let sunset = pictures.join("sunset.jpg");
        fs::write(&sunset, ORIGINAL_BYTES).unwrap();
        let paths = StatePaths {
            state_file: root.join("Roaming").join(STATE_FILE_NAME),
            output_dir: root.join("Local").join(OUTPUT_DIR_NAME),
            themes_dir: root
                .join("Roaming")
                .join("Microsoft")
                .join("Windows")
                .join("Themes"),
        };
        // explorer 的桌布快取：接管前它就是使用者目前桌布的轉存檔。
        fs::create_dir_all(&paths.themes_dir).unwrap();
        fs::write(paths.transcoded_wallpaper(), TRANSCODED_BYTES).unwrap();
        // 6.1 B4：逐螢幕設定時每台另有自己的轉存檔（編號＝列舉索引）；修正輪 3：格式與尺寸沿用原圖。
        for i in 0..2 {
            fs::write(paths.monitor_transcoded(i), monitor_transcoded_bytes(i)).unwrap();
        }
        let sunset = sunset.to_string_lossy().into_owned();
        let desk = Desk {
            monitors: vec![
                FakeMonitor {
                    device_path: DEV_A.to_owned(),
                    rect: Some(rect(0)),
                    wallpaper: Some(sunset.clone()),
                },
                FakeMonitor {
                    device_path: DEV_B.to_owned(),
                    rect: Some(rect(3840)),
                    wallpaper: Some(sunset.clone()),
                },
            ],
            position: ORIGINAL_POSITION,
            color: ORIGINAL_COLOR,
            slideshow_status: SLIDESHOW_STATE_ENABLED,
            slideshow: None,
        };
        let shared = Arc::new(Mutex::new(Shared {
            desk,
            calls: Vec::new(),
            fail_reads: 0,
            ok_reads_before_fail: None,
            ignore_sets: false,
            upper_case_readback: false,
            state_file: Some(paths.state_file.clone()),
            state_at_set: Vec::new(),
            explorer_registry: None,
            fail_set_device: None,
            explorer_memory: None,
            delayed_memory_swap: false,
            pending_memory_swap: Vec::new(),
        }));
        let backend_shared = Arc::clone(&shared);
        let service =
            WallpaperService::spawn_with(WallpaperServiceConfig::default(), move || FakeBackend {
                shared: backend_shared,
            })
            .unwrap();
        let mut registry = FakeRegistry::default();
        // 探針實測：登錄的 Wallpaper 是全小寫；這裡用 REG_EXPAND_SZ 驗證型別也被忠實保留。
        // 修正輪 4：值＝各螢幕讀回的共同路徑＝接管前是「全部螢幕」設定（6.1 重跑 L3 實機）。
        let original_wallpaper_reg = RegValue::string(REG_TYPE_EXPAND_SZ, &sunset.to_lowercase());
        registry.put(DESKTOP_KEY, "Wallpaper", original_wallpaper_reg.clone());
        registry.put(
            DESKTOP_KEY,
            "WallpaperStyle",
            RegValue::string(REG_TYPE_SZ, "10"),
        );
        // TileWallpaper 原本不存在。
        Self {
            root,
            paths,
            pictures,
            shared,
            service,
            registry,
            sched: SchedulerState::default(),
            now: 1_000,
            stable: Vec::new(),
            files: FakeIds::default(),
            original_wallpaper_reg,
        }
    }

    /// 接管前是「逐螢幕」設定（修正輪 4，6.1 實機）：逐螢幕 `SetWallpaper` 之後 explorer 把登錄 `Wallpaper` 改寫成
    /// Themes 的 `TranscodedWallpaper`。
    fn per_monitor_setting(&mut self) {
        let v = RegValue::string(
            REG_TYPE_SZ,
            &self.paths.transcoded_wallpaper().to_string_lossy(),
        );
        self.registry.put(DESKTOP_KEY, "Wallpaper", v.clone());
        self.original_wallpaper_reg = v;
    }

    fn sunset(&self) -> String {
        self.pictures
            .join("sunset.jpg")
            .to_string_lossy()
            .into_owned()
    }

    fn out(&self, name: &str) -> PathBuf {
        self.paths.output_dir.join(name)
    }

    fn load(&self) -> WallpaperTakeover {
        WallpaperTakeover::load(
            self.paths.clone(),
            TakeoverConfig {
                verify_attempts: 2,
                verify_interval: Duration::ZERO,
                registry_settle_timeout: Duration::from_millis(300),
                registry_poll_interval: Duration::from_millis(2),
                transient_retry_delay: Duration::from_millis(150),
            },
            self.now,
        )
    }

    fn set(&mut self, t: &mut WallpaperTakeover, device: &str, image: &Path) -> SetOutcome {
        let mut io = TakeoverIo {
            wallpaper: &self.service,
            registry: &mut self.registry,
            stable_displays: &self.stable,
            files: &self.files,
            now: self.now,
        };
        t.set_monitor_wallpaper(&mut io, &mut self.sched, device, image)
    }

    fn restore(&mut self, t: &mut WallpaperTakeover, reason: RestoreReason) -> RestoreOutcome {
        let mut io = TakeoverIo {
            wallpaper: &self.service,
            registry: &mut self.registry,
            stable_displays: &self.stable,
            files: &self.files,
            now: self.now,
        };
        t.restore(&mut io, &mut self.sched, reason)
    }

    fn restore_pending(&mut self, t: &mut WallpaperTakeover) -> PendingReport {
        let mut io = TakeoverIo {
            wallpaper: &self.service,
            registry: &mut self.registry,
            stable_displays: &self.stable,
            files: &self.files,
            now: self.now,
        };
        t.restore_pending_monitors(&mut io)
    }

    fn confirm(&mut self, t: &mut WallpaperTakeover, device: &str, readback: &str) -> bool {
        let io = TakeoverIo {
            wallpaper: &self.service,
            registry: &mut self.registry,
            stable_displays: &self.stable,
            files: &self.files,
            now: self.now,
        };
        t.confirm_applied(&io, device, readback)
    }

    fn desk(&self) -> Desk {
        self.shared.lock().unwrap().desk.clone()
    }

    fn with_desk(&self, f: impl FnOnce(&mut Desk)) {
        f(&mut self.shared.lock().unwrap().desk);
    }

    fn calls(&self) -> Vec<String> {
        self.shared.lock().unwrap().calls.clone()
    }

    fn clear_calls(&self) {
        let mut s = self.shared.lock().unwrap();
        s.calls.clear();
        s.state_at_set.clear();
    }

    fn set_calls(&self) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter(|c| c.starts_with("set_") || c == "restore_slideshow")
            .collect()
    }

    fn state_at_set(&self) -> Vec<Option<String>> {
        self.shared.lock().unwrap().state_at_set.clone()
    }

    fn state_on_disk(&self) -> StateFile {
        let text = fs::read_to_string(&self.paths.state_file).expect("狀態檔應存在");
        serde_json::from_str(&text).expect("狀態檔應可解析")
    }

    /// 模擬 explorer 處理 SetWallpaper 的登錄副作用（探針 1.1 實測）：`Wallpaper` 改寫成**這個** Themes 資料夾的
    /// `TranscodedWallpaper`（修正輪 4：之後重新接管時，它就是「逐螢幕設定」的判別依據）。
    fn explorer_rewrites_registry(&mut self) {
        let transcoded = self.paths.transcoded_wallpaper();
        self.registry.put(
            DESKTOP_KEY,
            "Wallpaper",
            RegValue::string(REG_TYPE_SZ, &transcoded.to_string_lossy()),
        );
        self.registry.put(
            DESKTOP_KEY,
            "TileWallpaper",
            RegValue::string(REG_TYPE_SZ, "0"),
        );
        self.registry.put(
            DESKTOP_KEY,
            "WallpaperStyle",
            RegValue::string(REG_TYPE_SZ, "6"),
        );
    }

    fn original_registry(&self) -> [RegValue; 3] {
        [
            self.original_wallpaper_reg.clone(),
            RegValue::string(REG_TYPE_SZ, "10"),
            RegValue::Missing,
        ]
    }

    fn current_registry(&self) -> [RegValue; 3] {
        [
            self.registry.value(DESKTOP_KEY, "Wallpaper"),
            self.registry.value(DESKTOP_KEY, "WallpaperStyle"),
            self.registry.value(DESKTOP_KEY, "TileWallpaper"),
        ]
    }

    fn backups(&self) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(self.paths.backup_dir())
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(self.root.parent().unwrap());
    }
}

fn key(device: &str, x: i32) -> String {
    monitor_key(device, Some(rect(x)), &[])
}

/// 接管兩台螢幕（各設一次），回傳狀態機。
fn take_over_both(h: &mut Harness) -> WallpaperTakeover {
    let mut t = h.load();
    let a = h.out("a-1.png");
    let b = h.out("b-1.png");
    assert_eq!(h.set(&mut t, DEV_A, &a), SetOutcome::Applied);
    assert_eq!(h.set(&mut t, DEV_B, &b), SetOutcome::Applied);
    h.explorer_rewrites_registry();
    t
}

fn assert_desk_restored(h: &Harness) {
    let d = h.desk();
    assert_eq!(d.wallpaper(DEV_A), Some(h.sunset()));
    assert_eq!(d.wallpaper(DEV_B), Some(h.sunset()));
    assert_eq!(d.position, ORIGINAL_POSITION, "全域填滿方式要還原");
    assert_eq!(d.color, ORIGINAL_COLOR);
    assert_eq!(
        h.current_registry(),
        h.original_registry(),
        "三個登錄值逐字還原"
    );
}

// ---------------------------------------------------------------------------------------------
// spec「桌布主題選擇」：新安裝不動桌布
// ---------------------------------------------------------------------------------------------

#[test]
fn fresh_install_touches_nothing() {
    let h = Harness::new("fresh");
    let t = h.load();
    assert!(t.blocked().is_none());
    assert!(!t.already_taken_over());
    assert_eq!(t.state().unwrap().status, TakeoverStatus::NotTakenOver);
    assert!(!h.paths.state_file.exists(), "沒有接管就不寫狀態檔");
    assert!(h.calls().is_empty(), "不呼叫任何桌布 API");
}

// ---------------------------------------------------------------------------------------------
// spec「接管前記錄原桌布」
// ---------------------------------------------------------------------------------------------

#[test]
fn records_original_and_persists_before_first_set() {
    let mut h = Harness::new("record-first");
    let mut t = h.load();
    let img = h.out("a-1.png");
    assert_eq!(h.set(&mut t, DEV_A, &img), SetOutcome::Applied);

    // 先寫後設：set_position 與 set_wallpaper 被呼叫的當下，狀態檔已經有完整記錄。
    let snaps = h.state_at_set();
    assert_eq!(h.set_calls().len(), 2, "{:?}", h.set_calls());
    assert!(
        h.set_calls()[0].starts_with("set_position 4"),
        "首次接管設 FILL"
    );
    for snap in &snaps {
        let text = snap.as_ref().expect("設定的當下狀態檔必須已存在");
        let st: StateFile = serde_json::from_str(text).unwrap();
        assert_eq!(st.status, TakeoverStatus::TakenOver);
        let rec = &st.monitors[&key(DEV_A, 0)];
        assert_eq!(rec.original_wallpaper, h.sunset());
        assert_eq!(rec.last_set.as_deref(), Some(img.to_str().unwrap()));
        let orig = st.original.as_ref().unwrap();
        assert_eq!(orig.position, ORIGINAL_POSITION);
        assert_eq!(orig.background_color, ORIGINAL_COLOR);
        assert_eq!(orig.registry.wallpaper, h.original_registry()[0]);
        assert_eq!(orig.registry.tile_wallpaper, RegValue::Missing);
    }

    // 兩台在線螢幕都記錄、各備份一份原圖。
    let st = h.state_on_disk();
    assert!(st.monitors.contains_key(&key(DEV_B, 3840)));
    let backups = h.backups();
    assert_eq!(
        backups,
        {
            let mut v = vec![
                format!("{}.jpg", key(DEV_A, 0)),
                format!("{}.jpg", key(DEV_B, 3840)),
            ];
            v.sort();
            v
        },
        "每台一份備份"
    );
    for b in &backups {
        assert_eq!(
            fs::read(h.paths.backup_dir().join(b)).unwrap(),
            ORIGINAL_BYTES
        );
    }
    assert!(
        t.already_taken_over(),
        "排程器的 already_taken_over 由狀態檔提供"
    );
    assert_eq!(
        h.desk().wallpaper(DEV_A),
        Some(img.to_string_lossy().into_owned())
    );
}

#[test]
fn restores_from_backup_when_original_deleted() {
    let mut h = Harness::new("backup-restore");
    let mut t = take_over_both(&mut h);
    fs::remove_file(h.sunset()).unwrap();

    let out = h.restore(&mut t, RestoreReason::NotTakeover);
    assert!(
        matches!(out.result, RestoreResult::Restored { .. }),
        "{out:?}"
    );
    let backup = h.paths.backup_dir().join(format!("{}.jpg", key(DEV_A, 0)));
    let d = h.desk();
    assert_eq!(
        d.wallpaper(DEV_A),
        Some(backup.to_string_lossy().into_owned())
    );
    assert_eq!(
        fs::read(&backup).unwrap(),
        ORIGINAL_BYTES,
        "還原的是原本那張圖片的內容"
    );
    assert_eq!(h.current_registry(), h.original_registry());
}

// ---------------------------------------------------------------------------------------------
// installer-auto-update 4.x（實機 4.4）：原桌布是「全部螢幕」設定時，還原也要回到「全部螢幕」
// ---------------------------------------------------------------------------------------------

/// 預設測試台＝接管前是「全部螢幕」設定（登錄 `Wallpaper`＝兩台讀回的共同路徑）：接管時記下
/// `all_monitors`，還原用**一次** `SetWallpaper(NULL, 原圖)`，不逐螢幕設定；最終桌布、登錄、全域設定照舊還原。
#[test]
fn all_monitors_original_is_restored_with_one_global_set() {
    let mut h = Harness::new("all-monitors-restore");
    let mut t = take_over_both(&mut h);
    let orig = h.state_on_disk().original.expect("接管中有原桌布記錄");
    assert!(orig.all_monitors, "登錄＝共同路徑＝全部螢幕設定：{orig:?}");
    h.clear_calls();

    let out = h.restore(&mut t, RestoreReason::NotTakeover);
    assert!(
        matches!(out.result, RestoreResult::Restored { .. }),
        "{out:?}"
    );
    let sets: Vec<String> = h
        .set_calls()
        .into_iter()
        .filter(|c| c.starts_with("set_wallpaper"))
        .collect();
    assert_eq!(
        sets,
        vec![format!("set_wallpaper_all {}", h.sunset())],
        "只有一次全部螢幕設定、沒有逐螢幕設定：{sets:?}"
    );
    assert_desk_restored(&h);
    assert_eq!(
        h.registry.value(DESKTOP_KEY, "TileWallpaper"),
        RegValue::Missing
    );
}

/// 逐螢幕設定的原桌布（登錄＝Themes 的 `TranscodedWallpaper`）：行為不變——逐台 `SetWallpaper(<螢幕>, 原圖)`，
/// 不呼叫全部螢幕設定。
#[test]
fn per_monitor_original_is_still_restored_per_monitor() {
    let mut h = Harness::new("per-monitor-restore");
    h.per_monitor_setting();
    let mut t = take_over_both(&mut h);
    let orig = h.state_on_disk().original.expect("接管中有原桌布記錄");
    assert!(!orig.all_monitors, "逐螢幕設定不記為全部螢幕：{orig:?}");
    h.clear_calls();

    let out = h.restore(&mut t, RestoreReason::NotTakeover);
    assert!(
        matches!(out.result, RestoreResult::Restored { .. }),
        "{out:?}"
    );
    let sets: Vec<String> = h
        .set_calls()
        .into_iter()
        .filter(|c| c.starts_with("set_wallpaper"))
        .collect();
    let mut want = vec![
        format!("set_wallpaper {DEV_A} {}", h.sunset()),
        format!("set_wallpaper {DEV_B} {}", h.sunset()),
    ];
    want.sort();
    let mut sets = sets;
    sets.sort();
    assert_eq!(sets, want, "每台各設一次（順序不拘）");
    assert_desk_restored(&h);
}

/// 全部螢幕的原桌布、但還原前使用者自己換了其中一台：不能用全域設定（會蓋掉使用者的選擇）——只還原宿主的那台，
/// 整次不寫登錄（既有規則）。
#[test]
fn all_monitors_original_with_a_user_choice_restores_only_the_host_monitor() {
    let mut h = Harness::new("all-monitors-user-choice");
    let mut t = take_over_both(&mut h);
    let chosen = h.pictures.join("mine.jpg").to_string_lossy().into_owned();
    h.with_desk(|d| {
        d.monitors
            .iter_mut()
            .find(|m| m.device_path == DEV_B)
            .unwrap()
            .wallpaper = Some(chosen.clone());
    });
    h.clear_calls();

    let out = h.restore(&mut t, RestoreReason::NotTakeover);
    assert!(
        matches!(out.result, RestoreResult::Restored { .. }),
        "{out:?}"
    );
    let sets: Vec<String> = h
        .set_calls()
        .into_iter()
        .filter(|c| c.starts_with("set_wallpaper"))
        .collect();
    assert_eq!(
        sets,
        vec![format!("set_wallpaper {DEV_A} {}", h.sunset())],
        "{sets:?}"
    );
    assert_eq!(h.desk().wallpaper(DEV_B), Some(chosen), "使用者的選擇不動");
}

/// 審查 M1：有離線螢幕時維持逐螢幕還原（`SetWallpaper(NULL, …)` 對離線螢幕的語意未驗證），離線那台標為待還原。
#[test]
fn all_monitors_original_with_an_offline_monitor_is_restored_per_monitor() {
    let mut h = Harness::new("all-monitors-offline");
    let mut t = take_over_both(&mut h);
    h.with_desk(|d| {
        d.monitors[1].rect = None;
        d.monitors[1].wallpaper = None;
    });
    h.clear_calls();

    let out = h.restore(&mut t, RestoreReason::NotTakeover);
    assert!(
        matches!(out.result, RestoreResult::Restored { .. }),
        "{out:?}"
    );
    let sets: Vec<String> = h
        .set_calls()
        .into_iter()
        .filter(|c| c.starts_with("set_wallpaper"))
        .collect();
    assert_eq!(
        sets,
        vec![format!("set_wallpaper {DEV_A} {}", h.sunset())],
        "只逐螢幕還原在線的那台：{sets:?}"
    );
    assert!(h.state_on_disk().monitors[&key(DEV_B, 3840)].pending_restore);
}

/// 全域設定沒生效（explorer 沒處理）：讀回驗證失敗、保留狀態檔供下次重試，與逐螢幕設定失敗的處置相同。
#[test]
fn global_restore_that_does_not_take_effect_keeps_the_state_for_retry() {
    let mut h = Harness::new("all-monitors-ignored");
    let mut t = take_over_both(&mut h);
    h.shared.lock().unwrap().ignore_sets = true;

    let out = h.restore(&mut t, RestoreReason::NotTakeover);
    assert!(
        matches!(out.result, RestoreResult::Failed { .. }),
        "{out:?}"
    );
    assert!(t.already_taken_over(), "仍是接管中，下次再還原");
    assert_eq!(h.state_on_disk().status, TakeoverStatus::TakenOver);
    assert!(h
        .set_calls()
        .iter()
        .any(|c| c.starts_with("set_wallpaper_all")));
}

/// 投影片、全域純色、Windows 焦點不記為全部螢幕（它們各有自己的還原路徑）。
#[test]
fn all_monitors_is_not_recorded_for_slideshow_solid_or_spotlight() {
    // 投影片（登錄仍是共同路徑，但投影片還原走自己的路）。
    let mut h = Harness::new("all-monitors-not-slideshow");
    let album = h.pictures.join("Album").to_string_lossy().into_owned();
    let slide = format!("{album}\\slide-1.jpg");
    h.with_desk(|d| {
        d.slideshow_status = SLIDESHOW_STATE_ENABLED | SLIDESHOW_STATE_SLIDESHOW;
        d.slideshow = Some(SlideshowInfo {
            items: vec![album.clone()],
            options: 0,
            tick_ms: 600_000,
        });
        for m in &mut d.monitors {
            m.wallpaper = Some(slide.clone());
        }
    });
    h.registry.put(
        DESKTOP_KEY,
        "Wallpaper",
        RegValue::string(REG_TYPE_SZ, &slide),
    );
    let _t = take_over_both(&mut h);
    assert!(!h.state_on_disk().original.unwrap().all_monitors);

    // 全域純色。
    let mut h = Harness::new("all-monitors-not-solid");
    let _ = solid_original(&mut h);
    let _t = take_over_both_confirmed(&mut h);
    assert!(!h.state_on_disk().original.unwrap().all_monitors);

    // Windows 焦點：讀回路徑一致、登錄＝共同路徑，也不記。
    let mut h = Harness::new("all-monitors-not-spotlight");
    h.registry
        .put(SPOTLIGHT_KEY, "EnabledState", RegValue::dword(1));
    let mut t = h.load();
    let img = h.out("a-1.png");
    assert_eq!(
        h.set(&mut t, DEV_A, &img),
        SetOutcome::AwaitingSpotlightConfirmation
    );
    t.confirm_spotlight();
    assert_eq!(h.set(&mut t, DEV_A, &img), SetOutcome::Applied);
    assert!(!h.state_on_disk().original.unwrap().all_monitors);
}

/// 狀態檔相容：沒有 `all_monitors` 的舊檔讀成「否」（維持逐螢幕還原）；為否時不寫出，為真時寫出。
#[test]
fn all_monitors_field_defaults_to_false_and_is_omitted_when_false() {
    let mut h = Harness::new("all-monitors-serde");
    let _t = take_over_both(&mut h);
    let text = fs::read_to_string(&h.paths.state_file).unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        value["original"]["all_monitors"],
        serde_json::Value::Bool(true),
        "{text}"
    );
    value["original"]
        .as_object_mut()
        .unwrap()
        .remove("all_monitors");
    let old: StateFile = serde_json::from_value(value).expect("舊檔（沒有欄位）仍可解析");
    assert!(!old.original.as_ref().unwrap().all_monitors);
    let rewritten = serde_json::to_string(&old).unwrap();
    assert!(!rewritten.contains("all_monitors"), "{rewritten}");
}

/// `restore_all_monitors_path` 的條件（純函式）：任何一項不成立都回 `None`（維持逐螢幕還原）。
#[test]
fn global_restore_path_requires_every_online_monitor_to_be_covered() {
    fn item(device: &str, current: &str, want: &str, class: RestoreClass) -> PlanItem {
        PlanItem {
            key: device.to_owned(),
            device: device.to_owned(),
            current: current.to_owned(),
            want: Want {
                path: want.to_owned(),
                own_copy: None,
            },
            class,
        }
    }
    fn snap(monitors: &[(&str, bool)]) -> WallpaperSnapshot {
        WallpaperSnapshot {
            monitors: monitors
                .iter()
                .map(|(d, online)| MonitorWallpaper {
                    monitor: MonitorEntry {
                        device_path: (*d).to_owned(),
                        rect: online.then(|| rect(0)),
                    },
                    wallpaper: online.then(|| "x".to_owned()),
                })
                .collect(),
            position: WallpaperPosition(4),
            background_color: 0,
            slideshow_status: 0,
            slideshow: None,
        }
    }
    let host = |d: &str, want: &str| item(d, "C:\\out\\a.png", want, RestoreClass::Host);
    let both = snap(&[(DEV_A, true), (DEV_B, true)]);

    // 全部是宿主的圖、要還原成同一張（大小寫不同也算同一張）。
    let ok = [host(DEV_A, "C:\\p\\a.jpg"), host(DEV_B, "c:\\P\\A.JPG")];
    assert_eq!(
        restore_all_monitors_path(&ok, &both).as_deref(),
        Some("C:\\p\\a.jpg")
    );
    // 有離線螢幕（審查 M1）：維持逐螢幕還原。
    let with_offline = snap(&[(DEV_A, true), (DEV_B, false)]);
    assert!(restore_all_monitors_path(&ok[..1], &with_offline).is_none());
    // 另一台已經顯示這張圖（Original）也算涵蓋。
    let mixed = [
        host(DEV_A, "C:\\p\\a.jpg"),
        item(
            DEV_B,
            "C:\\p\\a.jpg",
            "C:\\p\\a.jpg",
            RestoreClass::Original,
        ),
    ];
    assert!(restore_all_monitors_path(&mixed, &both).is_some());

    // 沒有要還原的螢幕。
    assert!(restore_all_monitors_path(&[], &both).is_none());
    // 還原成不同的圖（用備份副本還原者各不相同）。
    let differ = [host(DEV_A, "C:\\p\\a.jpg"), host(DEV_B, "C:\\b\\b.jpg")];
    assert!(restore_all_monitors_path(&differ, &both).is_none());
    // 空路徑（純色）。
    let solid = [host(DEV_A, ""), host(DEV_B, "")];
    assert!(restore_all_monitors_path(&solid, &both).is_none());
    // 有在線螢幕不在計畫裡：全域設定會蓋掉它。
    assert!(restore_all_monitors_path(&ok[..1], &both).is_none());
    // 另一台是「已是原桌布」但不是同一張圖。
    let other_original = [
        host(DEV_A, "C:\\p\\a.jpg"),
        item(
            DEV_B,
            "C:\\q\\o.jpg",
            "C:\\q\\o.jpg",
            RestoreClass::Original,
        ),
    ];
    assert!(restore_all_monitors_path(&other_original, &both).is_none());
    // 使用者自選。
    let chose = [
        host(DEV_A, "C:\\p\\a.jpg"),
        item(
            DEV_B,
            "C:\\m\\mine.jpg",
            "C:\\p\\a.jpg",
            RestoreClass::UserChoice,
        ),
    ];
    assert!(restore_all_monitors_path(&chose, &both).is_none());
}

#[test]
fn restores_solid_color_background() {
    let mut h = Harness::new("solid");
    h.with_desk(|d| {
        for m in &mut d.monitors {
            m.wallpaper = Some(String::new());
        }
    });
    let mut t = take_over_both(&mut h);
    assert!(h.backups().is_empty(), "純色沒有圖片可備份");

    let out = h.restore(&mut t, RestoreReason::NotTakeover);
    assert!(
        matches!(out.result, RestoreResult::Restored { .. }),
        "{out:?}"
    );
    let d = h.desk();
    assert_eq!(d.wallpaper(DEV_A).as_deref(), Some(""));
    assert_eq!(d.wallpaper(DEV_B).as_deref(), Some(""));
    assert_eq!(d.color, ORIGINAL_COLOR, "還原為原本的純色");
}

// ---------------------------------------------------------------------------------------------
// task 6.1 修正（Bug 1／B9）：純色原桌布＝全域狀態（假後端照實機語意）
// ---------------------------------------------------------------------------------------------

/// 原桌布為全域純色（`SetWallpaper(NULL, "")` 之後的狀態），explorer 記憶中的逐螢幕圖片是
/// `remembered.jpg`；登錄 `Wallpaper` 是空字串（6.1 B9 實測）。回傳記憶中的圖片路徑。
fn solid_original(h: &mut Harness) -> String {
    let remembered = h.pictures.join("remembered.jpg");
    fs::write(&remembered, b"REMEMBERED").unwrap();
    let remembered = remembered.to_string_lossy().into_owned();
    {
        let mut s = h.shared.lock().unwrap();
        s.explorer_memory = Some(
            [DEV_A, DEV_B]
                .iter()
                .map(|d| ((*d).to_owned(), remembered.clone()))
                .collect(),
        );
        for m in &mut s.desk.monitors {
            m.wallpaper = Some(String::new());
        }
        s.desk.slideshow_status = 0;
    }
    h.registry
        .put(DESKTOP_KEY, "Wallpaper", RegValue::string(REG_TYPE_SZ, ""));
    remembered
}

fn solid_registry() -> [RegValue; 3] {
    [
        RegValue::string(REG_TYPE_SZ, ""),
        RegValue::string(REG_TYPE_SZ, "10"),
        RegValue::Missing,
    ]
}

/// 照協調迴圈的順序接管兩台（每台 Applied 後確認）。
fn take_over_both_confirmed(h: &mut Harness) -> WallpaperTakeover {
    let mut t = h.load();
    for (dev, name) in [(DEV_A, "a-1.png"), (DEV_B, "b-1.png")] {
        let img = h.out(name);
        assert_eq!(h.set(&mut t, dev, &img), SetOutcome::Applied, "{dev}");
        assert!(h.confirm(&mut t, dev, &img.to_string_lossy()), "{dev}");
    }
    h.explorer_rewrites_registry();
    t
}

/// B9 第一段：設第一台之後 explorer 把第二台換回記憶中的圖片——那是接管途中的過渡讀回，不得讓位。
#[test]
fn solid_original_transition_readback_is_not_a_yield() {
    let mut h = Harness::new("r61-solid-transition");
    let remembered = solid_original(&mut h);
    let mut t = h.load();
    let a = h.out("a-1.png");
    assert_eq!(h.set(&mut t, DEV_A, &a), SetOutcome::Applied);
    assert_eq!(
        h.desk().wallpaper(DEV_B),
        Some(remembered.clone()),
        "假後端照實機：第二台換回記憶中的圖片"
    );
    assert!(h.confirm(&mut t, DEV_A, &a.to_string_lossy()));
    let b = h.out("b-1.png");
    assert_eq!(h.set(&mut t, DEV_B, &b), SetOutcome::Applied, "不得讓位");
    assert!(h.confirm(&mut t, DEV_B, &b.to_string_lossy()));
    let st = h.state_on_disk();
    assert_eq!(st.status, TakeoverStatus::TakenOver);
    let orig = st.original.as_ref().unwrap();
    assert!(orig.solid_color, "記為全域純色");
    assert_eq!(orig.background_color, ORIGINAL_COLOR, "含背景色");
    for (dev, x) in [(DEV_A, 0), (DEV_B, 3840)] {
        let rec = &st.monitors[&key(dev, x)];
        assert_eq!(rec.original_wallpaper, "", "{dev} 的原桌布仍是純色");
        assert!(rec.host_applied, "{dev}");
    }
    assert!(t.already_taken_over(), "沒有讓位");
    assert_eq!(t.phase(), Phase::Idle, "沒有進入停止");
}

/// B9 第二段：還原純色一律 `SetWallpaper(NULL, "")`，不用逐螢幕空字串；之後寫回背景色、填滿方式、登錄。
#[test]
fn solid_original_restores_with_global_solid() {
    let mut h = Harness::new("r61-solid-restore");
    solid_original(&mut h);
    let mut t = take_over_both_confirmed(&mut h);
    h.with_desk(|d| d.color = 0);
    h.clear_calls();
    let (kept, saved) = restored_parts(&h.restore(&mut t, RestoreReason::TrayExit));
    assert!(kept.is_empty() && saved, "{kept:?}");
    let calls = h.set_calls();
    assert!(calls.contains(&"set_solid_color".to_owned()), "{calls:?}");
    assert!(
        !calls
            .iter()
            .any(|c| c.starts_with("set_wallpaper ") && c.ends_with(' ')),
        "不得逐螢幕設空字串：{calls:?}"
    );
    let solid_at = calls.iter().position(|c| c == "set_solid_color").unwrap();
    for later in ["set_background_color", "set_position"] {
        let at = calls.iter().position(|c| c.starts_with(later));
        assert!(
            at.is_some_and(|i| i > solid_at),
            "{later} 在純色之後：{calls:?}"
        );
    }
    let d = h.desk();
    assert_eq!(d.wallpaper(DEV_A).as_deref(), Some(""));
    assert_eq!(d.wallpaper(DEV_B).as_deref(), Some(""));
    assert_eq!(d.color, ORIGINAL_COLOR);
    assert_eq!(d.position, ORIGINAL_POSITION);
    assert_eq!(h.current_registry(), solid_registry(), "三個登錄值逐字還原");
    assert!(!t.already_taken_over());
}

/// 接管途中（只設了第一台、第二台停在 explorer 記憶中的圖）就要求還原：同樣以全域純色還原，不把第二台的
/// 過渡讀回當成使用者自選。
#[test]
fn solid_original_restore_mid_takeover_is_global_solid() {
    let mut h = Harness::new("r61-solid-mid");
    solid_original(&mut h);
    let mut t = h.load();
    let a = h.out("a-1.png");
    assert_eq!(h.set(&mut t, DEV_A, &a), SetOutcome::Applied);
    assert!(h.confirm(&mut t, DEV_A, &a.to_string_lossy()));
    let (kept, _) = restored_parts(&h.restore(&mut t, RestoreReason::NotTakeover));
    assert!(kept.is_empty(), "過渡讀回不是使用者自選：{kept:?}");
    assert!(h.set_calls().contains(&"set_solid_color".to_owned()));
    let d = h.desk();
    assert_eq!(d.wallpaper(DEV_A).as_deref(), Some(""));
    assert_eq!(d.wallpaper(DEV_B).as_deref(), Some(""));
    assert_eq!(h.current_registry(), solid_registry());
}

/// 接管完成之後使用者自換照常讓位；其他仍是宿主圖的螢幕逐螢幕還原（純色無法逐螢幕還原：讀回會是 explorer
/// 記憶中的圖片，視為已還原、不標待還原），不寫登錄、不呼叫全域純色（會蓋掉使用者的選擇）。
#[test]
fn solid_original_user_change_after_takeover_yields() {
    let mut h = Harness::new("r61-solid-yield");
    let remembered = solid_original(&mut h);
    let mut t = take_over_both_confirmed(&mut h);
    let photo = photo_on(&h, &[0]);
    h.registry.writes.clear();
    let outcome = h.set(&mut t, DEV_B, &h.out("b-2.png"));
    assert!(matches!(outcome, SetOutcome::Yielded { .. }), "{outcome:?}");
    let d = h.desk();
    assert_eq!(d.wallpaper(DEV_A), Some(photo), "不覆蓋使用者的選擇");
    assert_eq!(d.wallpaper(DEV_B), Some(remembered), "B 不再是宿主的圖");
    assert!(!h.set_calls().contains(&"set_solid_color".to_owned()));
    assert!(h.registry.writes.is_empty(), "{:?}", h.registry.writes);
    let st = h.state_on_disk();
    assert!(
        !st.monitors[&key(DEV_B, 3840)].pending_restore,
        "逐螢幕還原已盡力完成，不標待還原"
    );
}

/// 修正輪 4（複審 N1）：讓位時把原為全域純色的螢幕逐螢幕設空字串，Windows 只會換回記憶中的圖片、不會回到純色。
/// 不論設定後第一次讀回仍是空字串（6.1 重跑 2b 實機時序：數秒後才換成記憶中的圖片），還是已經是圖片，記錄都一律
/// 寫「原為純色，無法單獨還原」並附當下讀回，不得寫「成功：純色」。
#[test]
fn solid_original_yield_logs_cannot_restore_solid_regardless_of_readback_timing() {
    use crate::data::tests::capture;
    for delayed in [true, false] {
        let mut h = Harness::new(&format!("r61e-solid-yield-log-{delayed}"));
        let remembered = solid_original(&mut h);
        let mut t = take_over_both_confirmed(&mut h);
        h.shared.lock().unwrap().delayed_memory_swap = delayed;
        photo_on(&h, &[0]);
        capture::start();
        let outcome = h.set(&mut t, DEV_B, &h.out("b-2.png"));
        let logs = capture::take();
        assert!(matches!(outcome, SetOutcome::Yielded { .. }), "{outcome:?}");
        let lines: Vec<&(log::Level, String)> = logs
            .iter()
            .filter(|(_, m)| m.starts_with("讓位時還原") && m.contains(DEV_B))
            .collect();
        assert_eq!(lines.len(), 1, "delayed={delayed}：{logs:?}");
        let (level, msg) = lines[0];
        assert_eq!(*level, log::Level::Warn, "delayed={delayed}：{msg}");
        assert!(
            msg.contains("原為純色，無法單獨還原") && !msg.contains("成功"),
            "delayed={delayed}：{msg}"
        );
        if delayed {
            assert!(msg.contains("讀回空字串"), "附當下讀回：{msg}");
        } else {
            assert!(msg.contains(&remembered), "附當下讀回：{msg}");
        }
        assert_eq!(
            h.desk().wallpaper(DEV_B),
            Some(remembered.clone()),
            "delayed={delayed}：最後顯示的是 explorer 記憶中的圖片"
        );
        assert!(
            !h.state_on_disk().monitors[&key(DEV_B, 3840)].pending_restore,
            "delayed={delayed}：逐螢幕已盡力，不標待還原"
        );
    }
}

/// 接管途中，已確認套用宿主圖的那台被使用者換掉：照常讓位（過渡讀回的例外只給還沒設定過的螢幕）。
#[test]
fn solid_original_change_on_applied_monitor_mid_takeover_yields() {
    let mut h = Harness::new("r61-solid-mid-yield");
    solid_original(&mut h);
    let mut t = h.load();
    let a = h.out("a-1.png");
    assert_eq!(h.set(&mut t, DEV_A, &a), SetOutcome::Applied);
    assert!(h.confirm(&mut t, DEV_A, &a.to_string_lossy()));
    photo_on(&h, &[0]);
    let outcome = h.set(&mut t, DEV_B, &h.out("b-1.png"));
    assert!(matches!(outcome, SetOutcome::Yielded { .. }), "{outcome:?}");
}

/// 修正輪 1（審查 L2）：已設定、還沒確認套用的螢幕，途中被使用者換成別的圖——照常讓位（過渡讀回的豁免只給
/// 設定前就看到的 explorer 記憶圖片）。
#[test]
fn solid_original_set_but_unconfirmed_monitor_changed_by_user_yields() {
    let mut h = Harness::new("r61b-solid-unconfirmed");
    solid_original(&mut h);
    let mut t = h.load();
    let a = h.out("a-1.png");
    assert_eq!(h.set(&mut t, DEV_A, &a), SetOutcome::Applied);
    assert!(h.confirm(&mut t, DEV_A, &a.to_string_lossy()));
    // B 的 SetWallpaper 被接受但 explorer 還沒處理（沒有確認）。
    h.shared.lock().unwrap().ignore_sets = true;
    assert_eq!(h.set(&mut t, DEV_B, &h.out("b-1.png")), SetOutcome::Applied);
    h.shared.lock().unwrap().ignore_sets = false;
    let photo = photo_on(&h, &[1]);
    let outcome = h.set(&mut t, DEV_A, &h.out("a-2.png"));
    assert!(matches!(outcome, SetOutcome::Yielded { .. }), "{outcome:?}");
    assert_eq!(h.desk().wallpaper(DEV_B), Some(photo), "不覆蓋使用者的選擇");
}

/// 修正輪 1（審查 L2 的反面）：B 的設定沒有生效（SetFailed、explorer 沒處理）而仍顯示設定前看到的 explorer
/// 記憶圖片——仍是過渡讀回，不得因為 `last_set` 已有值就誤判成使用者自換（B9 的失效換個路徑重現）。
#[test]
fn solid_original_unapplied_set_keeps_transition_readback() {
    let mut h = Harness::new("r61b-solid-unapplied");
    let remembered = solid_original(&mut h);
    let mut t = h.load();
    let a = h.out("a-1.png");
    assert_eq!(h.set(&mut t, DEV_A, &a), SetOutcome::Applied);
    assert!(h.confirm(&mut t, DEV_A, &a.to_string_lossy()));
    h.shared.lock().unwrap().ignore_sets = true;
    assert_eq!(h.set(&mut t, DEV_B, &h.out("b-1.png")), SetOutcome::Applied);
    h.shared.lock().unwrap().ignore_sets = false;
    assert_eq!(h.desk().wallpaper(DEV_B), Some(remembered));
    assert_eq!(
        h.set(&mut t, DEV_B, &h.out("b-2.png")),
        SetOutcome::Applied,
        "不得讓位"
    );
    assert!(t.already_taken_over());
}

/// 修正輪 2（複審 L2 殘餘，探針 P2）：A 的設定 explorer 還沒處理時 B 判定讀到空字串（沒記下記憶圖片），接著
/// B 的設定**確定失敗**——`last_set` 退回設定前的值（＝沒設定過）；之後 B 顯示 explorer 記憶中的圖片仍是過渡
/// 讀回，不讓位。
#[test]
fn solid_original_failed_set_rolls_back_last_set() {
    let mut h = Harness::new("r61c-solid-failed-set");
    let remembered = solid_original(&mut h);
    let mut t = h.load();
    let a = h.out("a-1.png");
    // A：SetWallpaper 被接受但 explorer 還沒處理（不確認）。
    h.shared.lock().unwrap().ignore_sets = true;
    assert_eq!(h.set(&mut t, DEV_A, &a), SetOutcome::Applied);
    h.shared.lock().unwrap().ignore_sets = false;
    // B：判定時讀到空字串；設定確定失敗。
    h.shared.lock().unwrap().fail_set_device = Some(DEV_B.to_owned());
    let outcome = h.set(&mut t, DEV_B, &h.out("b-1.png"));
    assert!(matches!(outcome, SetOutcome::SetFailed(_)), "{outcome:?}");
    h.shared.lock().unwrap().fail_set_device = None;
    let rec = h.state_on_disk().monitors[&key(DEV_B, 3840)].clone();
    assert_eq!(rec.last_set, None, "確定失敗：退回設定前的值");
    // explorer 這時才處理 A：B 換回記憶中的圖片。
    let a_str = a.to_string_lossy().into_owned();
    let mem = remembered.clone();
    h.with_desk(move |d| {
        d.monitors[0].wallpaper = Some(a_str);
        d.monitors[1].wallpaper = Some(mem);
    });
    assert_eq!(
        h.set(&mut t, DEV_B, &h.out("b-2.png")),
        SetOutcome::Applied,
        "不得讓位"
    );
    assert!(t.already_taken_over());
}

/// 修正輪 2：一般情況下設定確定失敗也退回 `last_set`（設定前的值），而且先寫後設不變——設定被呼叫的當下，
/// 狀態檔已經是新的 `last_set`。
#[test]
fn failed_set_rolls_back_last_set_after_write_first() {
    let mut h = Harness::new("r61c-failed-set");
    let mut t = take_over_both(&mut h);
    let before = h.state_on_disk().monitors[&key(DEV_A, 0)].last_set.clone();
    assert_eq!(
        before,
        Some(h.out("a-1.png").to_string_lossy().into_owned())
    );
    h.clear_calls();
    h.shared.lock().unwrap().fail_set_device = Some(DEV_A.to_owned());
    let next = h.out("a-2.png");
    let outcome = h.set(&mut t, DEV_A, &next);
    assert!(matches!(outcome, SetOutcome::SetFailed(_)), "{outcome:?}");
    let at_set = h.state_at_set();
    let written: StateFile =
        serde_json::from_str(at_set.last().unwrap().as_ref().expect("設定當下狀態檔存在")).unwrap();
    assert_eq!(
        written.monitors[&key(DEV_A, 0)].last_set.as_deref(),
        next.to_str(),
        "先寫後設：設定當下已是新值"
    );
    assert_eq!(
        h.state_on_disk().monitors[&key(DEV_A, 0)].last_set,
        before,
        "確定失敗後退回"
    );
    assert_eq!(
        t.state().unwrap().monitors[&key(DEV_A, 0)].last_set,
        before,
        "記憶體中也退回"
    );
}

/// 修正輪 1（審查 L1，M7）：只有部分螢幕讀回空字串時，不記為全域純色。
#[test]
fn partial_empty_readback_is_not_global_solid() {
    let mut h = Harness::new("r61b-partial-empty");
    h.with_desk(|d| d.monitors[0].wallpaper = Some(String::new()));
    drop(take_over_both(&mut h));
    let st = h.state_on_disk();
    assert!(!st.original.as_ref().unwrap().solid_color, "{st:?}");
}

/// 修正輪 1（審查 L5）：原桌布是 Windows 焦點（`EnabledState`＝1）時，讀回空字串不算全域純色，照焦點既有流程。
#[test]
fn spotlight_with_empty_readback_is_not_global_solid() {
    let mut h = Harness::new("r61b-spotlight-empty");
    solid_original(&mut h);
    h.registry
        .put(SPOTLIGHT_KEY, "EnabledState", RegValue::dword(1));
    let mut t = h.load();
    let img = h.out("a-1.png");
    assert_eq!(
        h.set(&mut t, DEV_A, &img),
        SetOutcome::AwaitingSpotlightConfirmation
    );
    t.confirm_spotlight();
    assert_eq!(h.set(&mut t, DEV_A, &img), SetOutcome::Applied);
    let st = h.state_on_disk();
    assert!(!st.original.as_ref().unwrap().solid_color, "{st:?}");
}

/// 原桌布全域純色：接管、B 離線時還原（A 回到純色、B 標為待還原），回傳狀態機。
fn solid_restore_with_b_offline(h: &mut Harness) -> WallpaperTakeover {
    solid_original(h);
    let mut t = take_over_both_confirmed(h);
    h.with_desk(|d| {
        d.monitors[1].rect = None;
        d.monitors[1].wallpaper = None;
    });
    restored_parts(&h.restore(&mut t, RestoreReason::TrayExit));
    assert!(h.state_on_disk().monitors[&key(DEV_B, 3840)].pending_restore);
    assert_eq!(h.desk().wallpaper(DEV_A).as_deref(), Some(""));
    t
}

/// 修正輪 1（審查 L4 裁定）：原桌布全域純色、B 離線而待還原。B 重新接上、其餘在線螢幕仍是純色——再呼叫一次
/// `SetWallpaper(NULL, "")`，**不得**對 B 單獨設空字串（那會換回記憶圖片，甚至把其他螢幕帶出純色）。
#[test]
fn solid_pending_monitor_rejoins_with_global_solid() {
    let mut h = Harness::new("r61b-solid-pending");
    let mut t = solid_restore_with_b_offline(&mut h);
    b_comes_online(&h, &h.out("b-1.png").to_string_lossy());
    h.clear_calls();
    let report = h.restore_pending(&mut t);
    assert_eq!(report.restored, vec![DEV_B.to_owned()], "{report:?}");
    let calls = h.set_calls();
    assert_eq!(calls, vec!["set_solid_color".to_owned()], "{calls:?}");
    let d = h.desk();
    assert_eq!(d.wallpaper(DEV_A).as_deref(), Some(""));
    assert_eq!(d.wallpaper(DEV_B).as_deref(), Some(""));
    assert!(!h.state_on_disk().monitors[&key(DEV_B, 3840)].pending_restore);
}

/// 修正輪 1（審查 L4 裁定）：B 重新接上時其他螢幕已不是純色（使用者改選了圖片）——不動 B、記一行、清掉待還原。
#[test]
fn solid_pending_monitor_left_alone_when_others_left_solid() {
    let mut h = Harness::new("r61b-solid-pending-user");
    let mut t = solid_restore_with_b_offline(&mut h);
    let photo = photo_on(&h, &[0]);
    let host_b = h.out("b-1.png").to_string_lossy().into_owned();
    b_comes_online(&h, &host_b);
    h.clear_calls();
    let report = h.restore_pending(&mut t);
    // 修正輪 2：不得歸在「清除標記」（那是「已不是宿主的圖」）——另列「仍顯示宿主的圖」。
    assert_eq!(
        report.left_showing_host,
        vec![DEV_B.to_owned()],
        "{report:?}"
    );
    assert!(report.cleared.is_empty(), "{report:?}");
    assert!(report.failed.is_empty(), "{report:?}");
    assert!(h.set_calls().is_empty(), "{:?}", h.set_calls());
    let d = h.desk();
    assert_eq!(d.wallpaper(DEV_A), Some(photo), "不碰使用者的選擇");
    assert_eq!(d.wallpaper(DEV_B), Some(host_b), "不對 B 單獨設空字串");
    assert!(!h.state_on_disk().monitors[&key(DEV_B, 3840)].pending_restore);
}

#[test]
fn restores_slideshow_source_and_interval() {
    let mut h = Harness::new("slideshow");
    let album = h.pictures.join("Album").to_string_lossy().into_owned();
    let info = SlideshowInfo {
        items: vec![album.clone()],
        options: SLIDESHOW_OPTION_SHUFFLE,
        tick_ms: 600_000,
    };
    let slide = format!("{album}\\slide-3.jpg");
    // 投影片目前那張不存在（原圖複製不了）：走逐螢幕轉存檔的備份（修正輪 4：登錄＝TranscodedWallpaper）。
    h.per_monitor_setting();
    h.with_desk(|d| {
        d.slideshow_status = SLIDESHOW_STATE_ENABLED | SLIDESHOW_STATE_SLIDESHOW;
        d.slideshow = Some(info.clone());
        for m in &mut d.monitors {
            m.wallpaper = Some(slide.clone());
        }
    });
    let mut t = take_over_both(&mut h);
    assert_eq!(h.desk().slideshow, None, "接管後投影片已停止");
    h.clear_calls();

    let out = h.restore(&mut t, RestoreReason::NotTakeover);
    assert!(
        matches!(out.result, RestoreResult::Restored { .. }),
        "{out:?}"
    );
    let d = h.desk();
    assert_eq!(d.slideshow, Some(info), "來源資料夾與切換間隔與原本相同");
    assert!(d.slideshow_status & SLIDESHOW_STATE_SLIDESHOW != 0);
    assert!(
        !h.set_calls().iter().any(|c| c.starts_with("set_wallpaper")),
        "投影片由 restore_slideshow 還原，不逐螢幕設單張圖：{:?}",
        h.set_calls()
    );
}

#[test]
fn crash_mid_takeover_keeps_record_and_restores() {
    let mut h = Harness::new("crash-mid");
    // explorer 收到請求但還沒處理宿主就當機：桌布仍是原圖，狀態檔已有記錄。
    h.shared.lock().unwrap().ignore_sets = true;
    {
        let mut t = h.load();
        let img = h.out("a-1.png");
        assert_eq!(h.set(&mut t, DEV_A, &img), SetOutcome::Applied);
    } // 當機：狀態機消失
    h.shared.lock().unwrap().ignore_sets = false;

    let mut t = h.load();
    assert!(t.already_taken_over(), "下次啟動仍持有原桌布記錄");
    assert_eq!(
        t.state().unwrap().monitors[&key(DEV_A, 0)].original_wallpaper,
        h.sunset()
    );
    // 下次啟動照常接管：讀回仍是原圖不算使用者自換。
    let img = h.out("a-2.png");
    assert_eq!(h.set(&mut t, DEV_A, &img), SetOutcome::Applied);
    h.explorer_rewrites_registry();

    let out = h.restore(&mut t, RestoreReason::NotTakeover);
    assert!(
        matches!(out.result, RestoreResult::Restored { .. }),
        "{out:?}"
    );
    assert_desk_restored(&h);
}

// ---------------------------------------------------------------------------------------------
// spec「原桌布為 Windows 焦點時先提醒」
// ---------------------------------------------------------------------------------------------

#[test]
fn spotlight_cancel_keeps_wallpaper_and_sets_theme_none() {
    let mut h = Harness::new("spotlight-cancel");
    h.registry
        .put(SPOTLIGHT_KEY, "EnabledState", RegValue::dword(1));
    let mut t = h.load();
    let img = h.out("a-1.png");
    assert_eq!(
        h.set(&mut t, DEV_A, &img),
        SetOutcome::AwaitingSpotlightConfirmation
    );
    assert_eq!(t.phase(), Phase::AwaitingSpotlightConfirmation);
    assert_eq!(
        h.set(&mut t, DEV_B, &img),
        SetOutcome::AwaitingSpotlightConfirmation,
        "等待確認期間任何螢幕都不設定"
    );
    let events = t.cancel_spotlight();
    assert_eq!(
        events,
        vec![TakeoverEvent::ThemeSetToNone {
            cause: ThemeNoneCause::SpotlightCancelled
        }]
    );
    assert_eq!(t.phase(), Phase::Idle);
    assert!(h.set_calls().is_empty(), "桌布不變");
    assert!(!h.paths.state_file.exists());
    assert!(!t.already_taken_over());
}

#[test]
fn spotlight_confirm_takes_over_and_records_diagnostics() {
    let mut h = Harness::new("spotlight-confirm");
    h.registry
        .put(SPOTLIGHT_KEY, "EnabledState", RegValue::dword(1));
    h.registry
        .put(WALLPAPERS_KEY, "BackgroundType", RegValue::dword(3));
    let mut t = h.load();
    let img = h.out("a-1.png");
    assert_eq!(
        h.set(&mut t, DEV_A, &img),
        SetOutcome::AwaitingSpotlightConfirmation
    );
    t.confirm_spotlight();
    assert_eq!(h.set(&mut t, DEV_A, &img), SetOutcome::Applied);
    let st = h.state_on_disk();
    assert_eq!(
        st.original.unwrap().spotlight,
        PersistedSpotlight {
            enabled_state: Some(1),
            background_type: Some(3),
        }
    );
    assert_eq!(t.phase(), Phase::Idle);
}

/// 4.8：設定視窗在使用者選主題時就先問（渲染前）。確認＝事先確認：第一次設定直接接管、不再等待。
#[test]
fn spotlight_preconfirmed_before_first_set_takes_over_without_waiting() {
    let mut h = Harness::new("spotlight-preconfirm");
    h.registry
        .put(SPOTLIGHT_KEY, "EnabledState", RegValue::dword(1));
    let mut t = h.load();
    assert!(t.confirm_spotlight(), "未接管時事先確認要生效");
    assert_eq!(t.phase(), Phase::SpotlightConfirmed);
    // 使用者接著把主題從「不接管」改成某個主題：事先確認不得被清掉。
    t.theme_selected(WallpaperTheme::Astrolabe);
    assert_eq!(t.phase(), Phase::SpotlightConfirmed);
    let img = h.out("a-1.png");
    assert_eq!(h.set(&mut t, DEV_A, &img), SetOutcome::Applied);
    assert!(t.already_taken_over());
}

/// 4.8：已接管時不會再問焦點，事先確認不改變階段。
#[test]
fn spotlight_preconfirm_is_ignored_once_taken_over() {
    let mut h = Harness::new("spotlight-preconfirm-taken");
    let mut t = take_over_both(&mut h);
    assert!(!t.confirm_spotlight());
    assert_eq!(t.phase(), Phase::Idle);
}

/// 4.8：事先確認之後使用者改回「不接管」＝撤回同意，下次選主題要再問。
#[test]
fn spotlight_preconfirm_is_withdrawn_by_selecting_none() {
    let h = Harness::new("spotlight-preconfirm-none");
    let mut t = h.load();
    assert!(t.confirm_spotlight());
    t.theme_selected(WallpaperTheme::None);
    assert_eq!(t.phase(), Phase::Idle);
}

/// 4.8：讓位／安全閥之後（未接管＋停止）事先確認也要生效，否則確認與主題變更分在兩次評估時，
/// 第一次設定前還會再問一次。
#[test]
fn spotlight_preconfirm_after_stop_when_not_taken_over() {
    let mut h = Harness::new("spotlight-preconfirm-stopped");
    let mut t = h.load();
    let out = h.restore(
        &mut t,
        RestoreReason::SafetyValve {
            detail: "GDI 9000".to_owned(),
        },
    );
    assert_eq!(out.result, RestoreResult::NothingToRestore);
    assert_eq!(t.phase(), Phase::Stopped);
    assert!(t.confirm_spotlight());
    assert_eq!(t.phase(), Phase::SpotlightConfirmed);
}

/// 4.8 修正輪 1（審查 L1）：安全閥觸發但還原失敗（仍是接管中）時，停止階段不得被過期的事先確認解除。
#[test]
fn spotlight_preconfirm_rejected_while_stopped_and_still_taken_over() {
    let mut h = Harness::new("spotlight-preconfirm-stopped-taken");
    let mut t = take_over_both(&mut h);
    h.shared.lock().unwrap().fail_reads = 1;
    let out = h.restore(
        &mut t,
        RestoreReason::SafetyValve {
            detail: "GDI 9000".to_owned(),
        },
    );
    assert!(
        matches!(out.result, RestoreResult::Failed { .. }),
        "{out:?}"
    );
    assert!(t.already_taken_over());
    assert_eq!(t.phase(), Phase::Stopped);
    assert!(!t.confirm_spotlight(), "仍接管中不接受事先確認");
    assert_eq!(t.phase(), Phase::Stopped);
}

/// 只讀的假登錄（焦點判定用）：`EnabledState` 的值，或讀取失敗。
struct SpotlightReg(Result<RegValue, ()>);

impl RegistryStore for SpotlightReg {
    fn get(&self, subkey: &str, name: &str) -> Result<RegValue, RegistryError> {
        if subkey != SPOTLIGHT_KEY || name != "EnabledState" {
            return Ok(RegValue::Missing);
        }
        self.0
            .clone()
            .map_err(|()| RegistryError("假讀取失敗".to_owned()))
    }
    fn set(&mut self, _: &str, _: &str, _: u32, _: &[u8]) -> Result<(), RegistryError> {
        unreachable!("焦點判定不寫登錄")
    }
    fn delete(&mut self, _: &str, _: &str) -> Result<(), RegistryError> {
        unreachable!("焦點判定不寫登錄")
    }
}

/// 4.8 焦點判定：只有「未接管、還沒確認、`EnabledState`＝1」要問；`EnabledState` 為 0、不存在、
/// 型別不符、讀取失敗一律視同一般圖片。
#[test]
fn needs_spotlight_confirmation_only_for_enabled_state_one_before_takeover() {
    use crate::desktop::wallpaper::registry::read_spotlight_in;
    let read = |v: Result<RegValue, ()>| read_spotlight_in(&SpotlightReg(v));
    let on = read(Ok(RegValue::dword(1)));

    assert!(needs_spotlight_confirmation(false, Phase::Idle, &on));
    assert!(needs_spotlight_confirmation(false, Phase::Stopped, &on));
    assert!(needs_spotlight_confirmation(
        false,
        Phase::AwaitingSpotlightConfirmation,
        &on
    ));
    for (name, v) in [
        ("EnabledState=0", Ok(RegValue::dword(0))),
        ("不存在", Ok(RegValue::Missing)),
        ("型別不符", Ok(RegValue::string(REG_TYPE_SZ, "1"))),
        ("讀取失敗", Err(())),
    ] {
        assert!(
            !needs_spotlight_confirmation(false, Phase::Idle, &read(v)),
            "{name} 視同一般圖片"
        );
    }
    assert!(
        !needs_spotlight_confirmation(true, Phase::Idle, &on),
        "已接管不問"
    );
    assert!(
        !needs_spotlight_confirmation(false, Phase::SpotlightConfirmed, &on),
        "已確認不問"
    );
}

/// 4.8 ledger：未接管時安全閥觸發，還原結果是 `NothingToRestore`——通知要知道「沒有還原」，不可寫
/// 「已還原您的桌布」。
#[test]
fn safety_valve_notice_carries_nothing_to_restore_when_not_taken_over() {
    let mut h = Harness::new("safety-valve-idle");
    let mut t = h.load();
    let out = h.restore(
        &mut t,
        RestoreReason::SafetyValve {
            detail: "GDI 9000".to_owned(),
        },
    );
    assert_eq!(out.result, RestoreResult::NothingToRestore);
    assert!(
        out.events
            .contains(&TakeoverEvent::Notify(TakeoverNotice::SafetyValve {
                detail: "GDI 9000".to_owned(),
                restore: ValveRestore::NothingToRestore,
            })),
        "{:?}",
        out.events
    );
}

#[test]
fn valve_restore_maps_every_restore_result() {
    assert_eq!(
        ValveRestore::from_result(&RestoreResult::NothingToRestore),
        ValveRestore::NothingToRestore
    );
    assert_eq!(
        ValveRestore::from_result(&RestoreResult::Restored {
            offline_unverified: Vec::new(),
            user_choice_kept: Vec::new(),
            warnings: Vec::new(),
            state_saved: true,
        }),
        ValveRestore::Restored
    );
    // task 6.4（審查 R2b nit）：有螢幕保留使用者自換的桌布時，通知不得寫「已還原原本的桌布」。
    assert_eq!(
        ValveRestore::from_result(&RestoreResult::Restored {
            offline_unverified: Vec::new(),
            user_choice_kept: vec!["DEV".to_owned()],
            warnings: Vec::new(),
            state_saved: true,
        }),
        ValveRestore::RestoredKeepingUserChoice
    );
    for retry in [
        RestoreResult::Failed {
            problems: vec!["x".to_owned()],
        },
        RestoreResult::StateUnavailable,
        RestoreResult::Queued,
    ] {
        assert_eq!(
            ValveRestore::from_result(&retry),
            ValveRestore::RetryLater,
            "{retry:?}"
        );
    }
    assert_eq!(
        ValveRestore::from_result(&RestoreResult::Blocked),
        ValveRestore::Unable
    );
}

#[test]
fn spotlight_diagnostics_recorded_when_not_spotlight() {
    let mut h = Harness::new("spotlight-off");
    h.registry
        .put(SPOTLIGHT_KEY, "EnabledState", RegValue::dword(0));
    let mut t = h.load();
    assert_eq!(h.set(&mut t, DEV_A, &h.out("a-1.png")), SetOutcome::Applied);
    assert_eq!(
        h.state_on_disk().original.unwrap().spotlight,
        PersistedSpotlight {
            enabled_state: Some(0),
            background_type: None,
        }
    );
}

// ---------------------------------------------------------------------------------------------
// spec「逐螢幕依原生解析度出圖」：新接上的螢幕先記錄
// ---------------------------------------------------------------------------------------------

#[test]
fn new_monitor_is_recorded_before_it_is_set() {
    let mut h = Harness::new("new-monitor");
    let other = h.pictures.join("other.png");
    fs::write(&other, b"OTHER").unwrap();
    let other = other.to_string_lossy().into_owned();
    h.with_desk(|d| d.monitors.truncate(1));
    let mut t = h.load();
    assert_eq!(h.set(&mut t, DEV_A, &h.out("a-1.png")), SetOutcome::Applied);
    assert!(!h.state_on_disk().monitors.contains_key(&key(DEV_B, 3840)));

    // 接管期間接上 B。
    h.with_desk(|d| {
        d.monitors.push(FakeMonitor {
            device_path: DEV_B.to_owned(),
            rect: Some(rect(3840)),
            wallpaper: Some(other.clone()),
        })
    });
    h.clear_calls();
    let img = h.out("b-1.png");
    assert_eq!(h.set(&mut t, DEV_B, &img), SetOutcome::Applied);
    let at_set = h.state_at_set();
    let st: StateFile = serde_json::from_str(at_set.last().unwrap().as_ref().unwrap()).unwrap();
    let rec = &st.monitors[&key(DEV_B, 3840)];
    assert_eq!(
        rec.original_wallpaper, other,
        "設定前已記錄 B 當下的桌布為原桌布"
    );
    assert_eq!(rec.last_set.as_deref(), Some(img.to_str().unwrap()));
    assert_eq!(
        fs::read(
            h.paths
                .backup_dir()
                .join(format!("{}.png", key(DEV_B, 3840)))
        )
        .unwrap(),
        b"OTHER"
    );
    assert!(
        !h.set_calls().iter().any(|c| c.starts_with("set_position")),
        "填滿方式只在首次接管時設定"
    );
}

#[test]
fn offline_monitor_record_is_kept() {
    let mut h = Harness::new("offline");
    let mut t = take_over_both(&mut h);
    h.with_desk(|d| {
        d.monitors[1].rect = None;
        d.monitors[1].wallpaper = None;
    });
    assert_eq!(h.set(&mut t, DEV_A, &h.out("a-2.png")), SetOutcome::Applied);
    assert!(
        h.state_on_disk().monitors.contains_key(&key(DEV_B, 3840)),
        "離線螢幕的紀錄不刪除"
    );
    assert_eq!(
        h.set(&mut t, DEV_B, &h.out("b-2.png")),
        SetOutcome::Refused(RefuseReason::MonitorNotOnline)
    );

    let out = h.restore(&mut t, RestoreReason::NotTakeover);
    match out.result {
        RestoreResult::Restored {
            offline_unverified, ..
        } => assert_eq!(offline_unverified, vec![DEV_B.to_owned()]),
        other => panic!("{other:?}"),
    }
    let st = h.state_on_disk();
    assert_eq!(st.status, TakeoverStatus::NotTakenOver);
    let b = &st.monitors[&key(DEV_B, 3840)];
    assert!(b.pending_restore, "離線螢幕標為待還原");
    assert_eq!(b.original_wallpaper, h.sunset(), "保留它的原桌布紀錄");
    assert!(!st.monitors[&key(DEV_A, 0)].pending_restore);
}

// ---------------------------------------------------------------------------------------------
// spec「使用者自行更換桌布時讓位」
// ---------------------------------------------------------------------------------------------

#[test]
fn user_changed_wallpaper_yields_and_is_not_overwritten() {
    let mut h = Harness::new("yield");
    let mut t = take_over_both(&mut h);
    let photo = h.pictures.join("photo.jpg").to_string_lossy().into_owned();
    h.with_desk(|d| d.monitors[0].wallpaper = Some(photo.clone()));
    let gen_before = h.sched.generation();
    h.clear_calls();

    let out = h.set(&mut t, DEV_A, &h.out("a-2.png"));
    assert_eq!(
        out,
        SetOutcome::Yielded {
            events: vec![
                TakeoverEvent::ThemeSetToNone {
                    cause: ThemeNoneCause::UserChangedWallpaper
                },
                TakeoverEvent::Notify(TakeoverNotice::Yielded {
                    device_paths: vec![DEV_A.to_owned()]
                }),
            ]
        }
    );
    assert!(
        !h.set_calls().iter().any(|c| c.contains(DEV_A)),
        "不得覆蓋使用者的照片：{:?}",
        h.set_calls()
    );
    assert_eq!(h.desk().wallpaper(DEV_A), Some(photo.clone()));
    assert_eq!(
        h.desk().wallpaper(DEV_B),
        Some(h.sunset()),
        "其他仍是宿主圖的螢幕還原為各自的原桌布"
    );
    assert!(!t.already_taken_over(), "讓位後 already_taken_over 為假");
    assert_ne!(h.sched.generation(), gen_before, "讓位後必須 reset 排程器");
    assert_eq!(t.phase(), Phase::Stopped);
    let st = h.state_on_disk();
    assert_eq!(st.status, TakeoverStatus::NotTakenOver);
    assert_eq!(
        st.monitors[&key(DEV_A, 0)].original_wallpaper,
        photo,
        "使用者新選的桌布記為原桌布"
    );

    // 同一次重畫的其餘螢幕不得再設定，也不得重新接管。
    h.clear_calls();
    assert_eq!(
        h.set(&mut t, DEV_B, &h.out("b-2.png")),
        SetOutcome::Refused(RefuseReason::Stopped)
    );
    assert!(h.set_calls().is_empty());

    // 使用者在設定中重新開啟：以當時的桌布為新的原桌布重新記錄。
    t.theme_selected(WallpaperTheme::Astrolabe);
    assert_eq!(h.set(&mut t, DEV_A, &h.out("a-3.png")), SetOutcome::Applied);
    let st = h.state_on_disk();
    assert_eq!(st.status, TakeoverStatus::TakenOver);
    assert_eq!(st.monitors[&key(DEV_A, 0)].original_wallpaper, photo);
}

#[test]
fn yield_never_restores_registry() {
    let mut h = Harness::new("yield-registry");
    let mut t = take_over_both(&mut h);
    let photo = h.pictures.join("photo.jpg").to_string_lossy().into_owned();
    h.with_desk(|d| d.monitors[0].wallpaper = Some(photo.clone()));
    h.registry.writes.clear();
    let registry_after_user_change = h.current_registry();

    assert!(matches!(
        h.set(&mut t, DEV_A, &h.out("a-2.png")),
        SetOutcome::Yielded { .. }
    ));
    let out = h.restore(&mut t, RestoreReason::NotTakeover);
    assert_eq!(out.result, RestoreResult::NothingToRestore);
    let out = h.restore(&mut t, RestoreReason::TrayExit);
    assert_eq!(out.result, RestoreResult::NothingToRestore);
    assert!(
        h.registry.writes.is_empty(),
        "讓位後不得寫登錄：{:?}",
        h.registry.writes
    );
    assert_eq!(h.current_registry(), registry_after_user_change);
    assert_eq!(h.desk().wallpaper(DEV_A), Some(photo));
}

// ---------------------------------------------------------------------------------------------
// 修正輪 1（裁決 3）：讓位時還原其他仍是宿主圖的螢幕
// ---------------------------------------------------------------------------------------------

#[test]
fn yield_restores_other_host_monitors_but_not_registry() {
    let mut h = Harness::new("yield-others");
    let mut t = take_over_both(&mut h);
    let photo = h.pictures.join("photo.jpg").to_string_lossy().into_owned();
    h.with_desk(|d| d.monitors[0].wallpaper = Some(photo.clone()));
    h.registry.writes.clear();
    h.clear_calls();

    assert!(matches!(
        h.set(&mut t, DEV_A, &h.out("a-2.png")),
        SetOutcome::Yielded { .. }
    ));
    assert_eq!(
        h.set_calls(),
        vec![format!("set_wallpaper {DEV_B} {}", h.sunset())],
        "只逐螢幕還原 B；A 不動、全域設定不動"
    );
    assert_eq!(h.desk().wallpaper(DEV_A), Some(photo));
    assert_eq!(h.desk().wallpaper(DEV_B), Some(h.sunset()));
    assert!(
        h.registry.writes.is_empty(),
        "讓位不寫登錄：{:?}",
        h.registry.writes
    );
    let st = h.state_on_disk();
    let b = &st.monitors[&key(DEV_B, 3840)];
    assert!(!b.pending_restore);
    assert_eq!(b.last_set, None);
}

#[test]
fn yield_other_monitor_readback_failure_stays_pending() {
    let mut h = Harness::new("yield-pending");
    let mut t = take_over_both(&mut h);
    let photo = h.pictures.join("photo.jpg").to_string_lossy().into_owned();
    h.with_desk(|d| d.monitors[0].wallpaper = Some(photo.clone()));
    {
        // 讓位判定那次讀取成功，之後的讀回全部失敗；B 的設定也沒有生效。
        let mut s = h.shared.lock().unwrap();
        s.ok_reads_before_fail = Some(1);
        s.ignore_sets = true;
    }
    assert!(matches!(
        h.set(&mut t, DEV_A, &h.out("a-2.png")),
        SetOutcome::Yielded { .. }
    ));
    let st = h.state_on_disk();
    assert_eq!(st.status, TakeoverStatus::NotTakenOver);
    assert!(
        st.monitors[&key(DEV_B, 3840)].pending_restore,
        "B 的讀回失敗：保留為待還原"
    );
    assert!(!st.monitors[&key(DEV_A, 0)].pending_restore);

    // 之後恢復正常：待還原的 B 仍是宿主的圖 → 還原。
    {
        let mut s = h.shared.lock().unwrap();
        s.ok_reads_before_fail = None;
        s.ignore_sets = false;
    }
    let report = h.restore_pending(&mut t);
    assert_eq!(report.restored, vec![DEV_B.to_owned()]);
    assert_eq!(h.desk().wallpaper(DEV_B), Some(h.sunset()));
    assert_eq!(h.desk().wallpaper(DEV_A), Some(photo));
    assert!(!h.state_on_disk().monitors[&key(DEV_B, 3840)].pending_restore);
}

#[test]
fn yield_with_offline_host_monitor_marks_it_pending() {
    let mut h = Harness::new("yield-offline");
    let mut t = take_over_both(&mut h);
    let photo = h.pictures.join("photo.jpg").to_string_lossy().into_owned();
    h.with_desk(|d| {
        d.monitors[0].wallpaper = Some(photo.clone());
        d.monitors[1].rect = None;
        d.monitors[1].wallpaper = None;
    });
    assert!(matches!(
        h.set(&mut t, DEV_A, &h.out("a-2.png")),
        SetOutcome::Yielded { .. }
    ));
    assert!(h.state_on_disk().monitors[&key(DEV_B, 3840)].pending_restore);
}

// ---------------------------------------------------------------------------------------------
// 修正輪 1（裁決 4）：還原時離線的螢幕成為待還原
// ---------------------------------------------------------------------------------------------

/// 接管兩台後 B 離線，選「不接管」還原：B 成為待還原。
fn restore_with_b_offline(h: &mut Harness) -> WallpaperTakeover {
    let mut t = take_over_both(h);
    h.with_desk(|d| {
        d.monitors[1].rect = None;
        d.monitors[1].wallpaper = None;
    });
    let out = h.restore(&mut t, RestoreReason::NotTakeover);
    assert!(
        matches!(out.result, RestoreResult::Restored { .. }),
        "{out:?}"
    );
    assert!(h.state_on_disk().monitors[&key(DEV_B, 3840)].pending_restore);
    t
}

fn b_comes_online(h: &Harness, wallpaper: &str) {
    let w = wallpaper.to_owned();
    h.with_desk(|d| {
        d.monitors[1].rect = Some(rect(3840));
        d.monitors[1].wallpaper = Some(w);
    });
}

#[test]
fn pending_monitor_restored_when_it_comes_online() {
    let mut h = Harness::new("pending-online");
    let mut t = restore_with_b_offline(&mut h);
    // 重新接上時 Windows 仍顯示宿主最後一張圖。
    b_comes_online(&h, &h.out("b-1.png").to_string_lossy());
    h.registry.writes.clear();
    let report = h.restore_pending(&mut t);
    assert_eq!(report.restored, vec![DEV_B.to_owned()]);
    assert!(report.cleared.is_empty() && report.still_offline.is_empty());
    assert_eq!(h.desk().wallpaper(DEV_B), Some(h.sunset()));
    assert!(h.registry.writes.is_empty(), "逐螢幕還原不寫登錄");
    let st = h.state_on_disk();
    assert!(!st.monitors[&key(DEV_B, 3840)].pending_restore);
    assert_eq!(st.monitors[&key(DEV_B, 3840)].last_set, None);
}

#[test]
fn pending_monitor_with_user_image_is_cleared_not_overwritten() {
    let mut h = Harness::new("pending-user");
    let mut t = restore_with_b_offline(&mut h);
    let photo = h.pictures.join("photo.jpg").to_string_lossy().into_owned();
    b_comes_online(&h, &photo);
    h.clear_calls();
    let report = h.restore_pending(&mut t);
    assert_eq!(report.cleared, vec![DEV_B.to_owned()]);
    assert!(report.restored.is_empty());
    assert!(h.set_calls().is_empty(), "使用者這段期間自己換的桌布不覆蓋");
    assert_eq!(h.desk().wallpaper(DEV_B), Some(photo));
    assert!(!h.state_on_disk().monitors[&key(DEV_B, 3840)].pending_restore);
}

#[test]
fn pending_monitor_still_offline_stays_pending() {
    let mut h = Harness::new("pending-offline");
    let mut t = restore_with_b_offline(&mut h);
    let report = h.restore_pending(&mut t);
    assert_eq!(report.still_offline, vec![DEV_B.to_owned()]);
    assert!(h.state_on_disk().monitors[&key(DEV_B, 3840)].pending_restore);
}

#[test]
fn takeover_while_pending_reuses_original_record() {
    let mut h = Harness::new("pending-takeover");
    let mut t = restore_with_b_offline(&mut h);
    b_comes_online(&h, &h.out("b-1.png").to_string_lossy());
    t.theme_selected(WallpaperTheme::Astrolabe);
    assert_eq!(h.set(&mut t, DEV_A, &h.out("a-3.png")), SetOutcome::Applied);
    let st = h.state_on_disk();
    let b = &st.monitors[&key(DEV_B, 3840)];
    assert_eq!(
        b.original_wallpaper,
        h.sunset(),
        "待還原螢幕目前的宿主圖不得被記成原桌布"
    );
    assert!(!b.pending_restore, "重新接管後成為一般紀錄");

    // 之後還原：B 回到真正的原桌布。
    let out = h.restore(&mut t, RestoreReason::NotTakeover);
    assert!(
        matches!(out.result, RestoreResult::Restored { .. }),
        "{out:?}"
    );
    assert_eq!(h.desk().wallpaper(DEV_B), Some(h.sunset()));
}

#[test]
fn pending_survives_restart_and_command_line_restores_it() {
    let mut h = Harness::new("pending-restart");
    drop(restore_with_b_offline(&mut h));
    let mut t = h.load();
    assert!(!t.already_taken_over());
    assert!(t.state().unwrap().monitors[&key(DEV_B, 3840)].pending_restore);
    b_comes_online(&h, &h.out("b-1.png").to_string_lossy());
    h.registry.writes.clear();
    let out = h.restore(&mut t, RestoreReason::CommandLine);
    assert!(
        matches!(out.result, RestoreResult::Restored { .. }),
        "{out:?}"
    );
    assert_eq!(h.desk().wallpaper(DEV_B), Some(h.sunset()));
    assert!(h.registry.writes.is_empty(), "只有待還原螢幕時不寫登錄");
    assert!(!h.state_on_disk().monitors[&key(DEV_B, 3840)].pending_restore);
}

#[test]
fn readback_failure_is_not_a_yield() {
    let mut h = Harness::new("readback-fail");
    let mut t = take_over_both(&mut h);
    h.shared.lock().unwrap().fail_reads = 1;
    h.clear_calls();
    let out = h.set(&mut t, DEV_A, &h.out("a-2.png"));
    assert!(matches!(out, SetOutcome::ReadbackFailed(_)), "{out:?}");
    assert!(h.set_calls().is_empty(), "讀回失敗本次跳過、不設定");
    assert!(t.already_taken_over(), "讀回失敗不算自換");
    assert_eq!(t.phase(), Phase::Idle);
    assert_eq!(h.set(&mut t, DEV_A, &h.out("a-2.png")), SetOutcome::Applied);
}

#[test]
fn non_ascii_case_difference_in_readback_is_not_a_yield() {
    let mut h = Harness::new("non-ascii");
    let mut t = take_over_both(&mut h);
    // 讀回的路徑整條轉成大寫（含 ölaf → ÖLAF）。
    h.shared.lock().unwrap().upper_case_readback = true;
    assert_eq!(h.set(&mut t, DEV_A, &h.out("a-2.png")), SetOutcome::Applied);
    assert!(t.already_taken_over());
}

// ---------------------------------------------------------------------------------------------
// spec「還原原桌布」
// ---------------------------------------------------------------------------------------------

#[test]
fn tray_exit_restores_wallpaper_position_and_registry() {
    let mut h = Harness::new("tray-exit");
    let mut t = take_over_both(&mut h);
    assert_ne!(h.current_registry(), h.original_registry());
    let gen_before = h.sched.generation();

    let out = h.restore(&mut t, RestoreReason::TrayExit);
    assert!(
        matches!(
            out.result,
            RestoreResult::Restored {
                state_saved: true,
                ..
            }
        ),
        "{out:?}"
    );
    assert!(out.events.is_empty());
    assert_desk_restored(&h);
    assert_eq!(
        h.registry.value(DESKTOP_KEY, "TileWallpaper"),
        RegValue::Missing,
        "原本不存在的登錄值在還原時被刪除"
    );
    assert!(!t.already_taken_over());
    assert_ne!(h.sched.generation(), gen_before, "還原後必須 reset 排程器");
    let st = h.state_on_disk();
    assert_eq!(st.status, TakeoverStatus::NotTakenOver);
    assert!(st.original.is_none());
    assert!(matches!(st.last_event, Some(LastEvent::Restored { .. })));
}

#[test]
fn crash_restart_resumes_takeover_without_resnapshot() {
    let mut h = Harness::new("crash-restart");
    drop(take_over_both(&mut h)); // 當機：桌布停在最後一張圖
    assert_eq!(
        h.desk().wallpaper(DEV_A),
        Some(h.out("a-1.png").to_string_lossy().into_owned())
    );
    let mut t = h.load();
    assert!(t.already_taken_over());
    assert_eq!(h.set(&mut t, DEV_A, &h.out("a-2.png")), SetOutcome::Applied);
    let orig = h.state_on_disk().original.unwrap();
    assert_eq!(
        [
            orig.registry.wallpaper,
            orig.registry.wallpaper_style,
            orig.registry.tile_wallpaper
        ],
        h.original_registry(),
        "當機重啟以狀態檔為準、不重新快照（此時登錄已被 explorer 改寫）"
    );
}

#[test]
fn session_end_does_not_restore() {
    let mut h = Harness::new("logout");
    drop(take_over_both(&mut h));
    let t = h.load();
    assert!(t.already_taken_over(), "狀態保持接管中，下次登入照常接管");
    assert_eq!(
        h.desk().wallpaper(DEV_A),
        Some(h.out("a-1.png").to_string_lossy().into_owned()),
        "保留最後一張圖"
    );
}

#[test]
fn command_line_restore_from_fresh_process() {
    let mut h = Harness::new("cli");
    drop(take_over_both(&mut h));
    let mut t = h.load();
    let out = h.restore(&mut t, RestoreReason::CommandLine);
    assert!(
        matches!(out.result, RestoreResult::Restored { .. }),
        "{out:?}"
    );
    assert_desk_restored(&h);
}

#[test]
fn restore_verification_failure_keeps_state() {
    let mut h = Harness::new("verify-fail");
    let mut t = take_over_both(&mut h);
    h.shared.lock().unwrap().ignore_sets = true;
    let gen_before = h.sched.generation();
    let out = h.restore(&mut t, RestoreReason::TrayExit);
    assert!(
        matches!(out.result, RestoreResult::Failed { .. }),
        "{out:?}"
    );
    assert!(t.already_taken_over(), "驗證失敗保留接管狀態，供下次重試");
    assert_eq!(h.state_on_disk().status, TakeoverStatus::TakenOver);
    assert!(h.state_on_disk().original.is_some());
    assert_ne!(h.sched.generation(), gen_before);

    // 下次（例如 --restore-wallpaper）可以重試成功。
    h.shared.lock().unwrap().ignore_sets = false;
    let mut t = h.load();
    let out = h.restore(&mut t, RestoreReason::CommandLine);
    assert!(
        matches!(out.result, RestoreResult::Restored { .. }),
        "{out:?}"
    );
    assert_desk_restored(&h);
}

#[test]
fn restore_waits_for_redraw_in_progress() {
    let mut h = Harness::new("queued");
    let mut t = take_over_both(&mut h);
    t.redraw_started();
    h.clear_calls();
    let out = h.restore(&mut t, RestoreReason::NotTakeover);
    assert_eq!(out.result, RestoreResult::Queued);
    assert!(h.set_calls().is_empty(), "Redraw 進行中不還原");
    assert_eq!(
        h.set(&mut t, DEV_B, &h.out("b-2.png")),
        SetOutcome::Refused(RefuseReason::RestorePending),
        "還原排隊中，未送出的螢幕不得再送"
    );
    assert_eq!(t.redraw_finished(), Some(RestoreReason::NotTakeover));
    assert_eq!(t.redraw_finished(), None);
    let out = h.restore(&mut t, RestoreReason::NotTakeover);
    assert!(
        matches!(out.result, RestoreResult::Restored { .. }),
        "{out:?}"
    );
    assert_desk_restored(&h);
}

#[test]
fn safety_valve_restores_and_stops_with_notice() {
    let mut h = Harness::new("safety-valve");
    let mut t = take_over_both(&mut h);
    let detail = "GDI 增量 2001".to_owned();
    let out = h.restore(
        &mut t,
        RestoreReason::SafetyValve {
            detail: detail.clone(),
        },
    );
    assert!(
        matches!(out.result, RestoreResult::Restored { .. }),
        "{out:?}"
    );
    assert_eq!(
        out.events,
        vec![
            TakeoverEvent::ThemeSetToNone {
                cause: ThemeNoneCause::SafetyValve
            },
            TakeoverEvent::Notify(TakeoverNotice::SafetyValve {
                detail,
                restore: ValveRestore::Restored,
            }),
        ]
    );
    assert_desk_restored(&h);
    assert_eq!(t.phase(), Phase::Stopped);
}

// ---------------------------------------------------------------------------------------------
// spec「輸出檔數量有上限」：原桌布備份每台至多一份
// ---------------------------------------------------------------------------------------------

#[test]
fn backup_at_most_one_per_monitor_and_not_recopied() {
    let mut h = Harness::new("backup-cap");
    let mut t = take_over_both(&mut h);
    // 同一次接管期間不重複複製：原圖之後被改，備份仍是第一次的內容。
    fs::write(h.sunset(), b"CHANGED").unwrap();
    assert_eq!(h.set(&mut t, DEV_A, &h.out("a-2.png")), SetOutcome::Applied);
    let backup_a = h.paths.backup_dir().join(format!("{}.jpg", key(DEV_A, 0)));
    assert_eq!(fs::read(&backup_a).unwrap(), ORIGINAL_BYTES);

    // 還原後使用者改用 PNG，再次接管：重新記錄、每台仍只有一份備份。
    let out = h.restore(&mut t, RestoreReason::NotTakeover);
    assert!(
        matches!(out.result, RestoreResult::Restored { .. }),
        "{out:?}"
    );
    let png = h.pictures.join("new.png");
    fs::write(&png, b"NEW-PNG").unwrap();
    let png = png.to_string_lossy().into_owned();
    h.with_desk(|d| {
        for m in &mut d.monitors {
            m.wallpaper = Some(png.clone());
        }
    });
    t.theme_selected(WallpaperTheme::Astrolabe);
    assert_eq!(h.set(&mut t, DEV_A, &h.out("a-3.png")), SetOutcome::Applied);
    let backups = h.backups();
    assert_eq!(backups.len(), 2, "{backups:?}");
    assert!(backups.iter().all(|b| b.ends_with(".png")), "{backups:?}");
    assert_eq!(
        h.state_on_disk().monitors[&key(DEV_A, 0)].original_wallpaper,
        png,
        "以當時的桌布為新的原桌布"
    );
}

// ---------------------------------------------------------------------------------------------
// brief 補充：先寫後設失敗、損壞、未知版本、登錄序列化
// ---------------------------------------------------------------------------------------------

#[test]
fn state_write_failure_prevents_any_set() {
    let mut h = Harness::new("write-fail");
    let mut t = h.load();
    // 讓狀態檔的上層「資料夾」其實是一個檔案：建立資料夾與寫檔都會失敗。
    let blocker = h.root.join("blocker");
    fs::write(&blocker, b"x").unwrap();
    t.paths.state_file = blocker.join(STATE_FILE_NAME);
    let out = h.set(&mut t, DEV_A, &h.out("a-1.png"));
    assert!(
        matches!(out, SetOutcome::Refused(RefuseReason::StateWriteFailed(_))),
        "{out:?}"
    );
    assert!(h.set_calls().is_empty(), "寫入失敗就不設定");
    assert!(!t.already_taken_over());
}

#[test]
fn corrupt_state_file_is_kept_and_blocks_takeover() {
    let mut h = Harness::new("corrupt");
    fs::create_dir_all(h.paths.state_file.parent().unwrap()).unwrap();
    let garbage = b"{ \"version\": 1, \"status\": ";
    fs::write(&h.paths.state_file, garbage).unwrap();

    let mut t = h.load();
    assert!(
        matches!(t.blocked(), Some(BlockReason::Corrupt(_))),
        "{:?}",
        t.blocked()
    );
    assert!(t.already_taken_over(), "不確定時保守視為接管中");
    assert_eq!(
        fs::read(&h.paths.state_file).unwrap(),
        garbage,
        "不覆寫原檔"
    );
    let copy = h
        .paths
        .state_file
        .with_file_name(format!("{STATE_FILE_NAME}.corrupt-1000"));
    assert_eq!(
        fs::read(&copy).unwrap(),
        garbage,
        "保留一份 .corrupt-<時間>"
    );

    assert_eq!(
        h.set(&mut t, DEV_A, &h.out("a-1.png")),
        SetOutcome::Refused(RefuseReason::Blocked)
    );
    let out = h.restore(&mut t, RestoreReason::NotTakeover);
    assert_eq!(out.result, RestoreResult::Blocked);
    assert!(h.calls().is_empty(), "不呼叫任何桌布 API");
    assert!(h.registry.writes.is_empty());
    assert_eq!(fs::read(&h.paths.state_file).unwrap(), garbage);

    // 再次啟動不重複產生相同內容的副本。
    h.now = 2_000;
    let t = h.load();
    assert!(t.blocked().is_some());
    let copies = fs::read_dir(h.paths.state_file.parent().unwrap())
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".corrupt-")
        })
        .count();
    assert_eq!(copies, 1);
}

fn write_state_json(h: &Harness, value: &serde_json::Value) {
    fs::create_dir_all(h.paths.state_file.parent().unwrap()).unwrap();
    fs::write(
        &h.paths.state_file,
        serde_json::to_string_pretty(value).unwrap(),
    )
    .unwrap();
}

fn taken_over_json(h: &mut Harness) -> serde_json::Value {
    drop(take_over_both(h));
    serde_json::from_str(&fs::read_to_string(&h.paths.state_file).unwrap()).unwrap()
}

#[test]
fn unknown_version_blocks_without_overwrite() {
    let mut h = Harness::new("version");
    let mut v = taken_over_json(&mut h);
    v["version"] = serde_json::json!(99);
    write_state_json(&h, &v);
    let before = fs::read(&h.paths.state_file).unwrap();
    let mut t = h.load();
    assert_eq!(t.blocked(), Some(&BlockReason::UnknownVersion(Some(99))));
    assert_eq!(
        h.set(&mut t, DEV_A, &h.out("a-2.png")),
        SetOutcome::Refused(RefuseReason::Blocked)
    );
    assert_eq!(fs::read(&h.paths.state_file).unwrap(), before);
}

#[test]
fn registry_values_round_trip_and_bad_encoding_is_corrupt_not_missing() {
    let mut h = Harness::new("reg-serde");
    let v = taken_over_json(&mut h);
    let reg = &v["original"]["registry"];
    assert_eq!(reg["tile_wallpaper"]["state"], "missing");
    assert_eq!(reg["wallpaper"]["state"], "present");
    assert_eq!(reg["wallpaper"]["kind"], REG_TYPE_EXPAND_SZ);
    let t = h.load();
    assert!(t.blocked().is_none());

    // 位元組編碼壞掉：整份視為損壞，不可默默當成「原本不存在」（還原時會誤刪）。
    let mut bad = v.clone();
    bad["original"]["registry"]["wallpaper"]["data_hex"] = serde_json::json!("zz");
    write_state_json(&h, &bad);
    assert!(matches!(h.load().blocked(), Some(BlockReason::Corrupt(_))));

    let mut bad = v.clone();
    bad["original"]["registry"]["wallpaper_style"] = serde_json::json!({});
    write_state_json(&h, &bad);
    assert!(matches!(h.load().blocked(), Some(BlockReason::Corrupt(_))));

    let mut bad = v.clone();
    bad["original"]["registry"]
        .as_object_mut()
        .unwrap()
        .remove("tile_wallpaper");
    write_state_json(&h, &bad);
    assert!(
        matches!(h.load().blocked(), Some(BlockReason::Corrupt(_))),
        "缺少的登錄值不得被補成 Missing"
    );
}

#[test]
fn inconsistent_status_is_corrupt() {
    let mut h = Harness::new("inconsistent");
    let mut v = taken_over_json(&mut h);
    v["original"] = serde_json::Value::Null;
    write_state_json(&h, &v);
    assert!(matches!(h.load().blocked(), Some(BlockReason::Corrupt(_))));
}

#[test]
fn refuses_images_outside_host_output() {
    let mut h = Harness::new("not-output");
    let mut t = h.load();
    let user_pic = PathBuf::from(h.sunset());
    assert_eq!(
        h.set(&mut t, DEV_A, &user_pic),
        SetOutcome::Refused(RefuseReason::NotHostOutput)
    );
    let backup = h.paths.backup_dir().join("m-0.png");
    assert_eq!(
        h.set(&mut t, DEV_A, &backup),
        SetOutcome::Refused(RefuseReason::NotHostOutput),
        "備份資料夾不是輸出圖"
    );
    assert!(h.calls().is_empty());
}

// ---------------------------------------------------------------------------------------------
// 修正輪 2
// ---------------------------------------------------------------------------------------------

fn photo_on(h: &Harness, which: &[usize]) -> String {
    let photo = h.pictures.join("photo.jpg");
    fs::write(&photo, b"PHOTO").unwrap();
    let photo = photo.to_string_lossy().into_owned();
    let p = photo.clone();
    let which = which.to_vec();
    h.with_desk(move |d| {
        for i in which {
            d.monitors[i].wallpaper = Some(p.clone());
        }
    });
    photo
}

fn restored_parts(out: &RestoreOutcome) -> (Vec<String>, bool) {
    match &out.result {
        RestoreResult::Restored {
            user_choice_kept,
            state_saved,
            ..
        } => {
            let mut kept = user_choice_kept.clone();
            kept.sort();
            (kept, *state_saved)
        }
        other => panic!("應為 Restored：{other:?}"),
    }
}

// 裁決 1：還原前的讓位判定

#[test]
fn restore_does_not_overwrite_photo_chosen_before_quit() {
    let mut h = Harness::new("r2-quit-photo");
    let mut t = take_over_both(&mut h);
    // 使用者在 Windows 設定換了照片（套用到所有螢幕），還沒到下一個重畫時點就從系統匣結束。
    let photo = photo_on(&h, &[0, 1]);
    h.registry.writes.clear();
    h.clear_calls();
    let out = h.restore(&mut t, RestoreReason::TrayExit);
    let (kept, _) = restored_parts(&out);
    let mut both = vec![DEV_A.to_owned(), DEV_B.to_owned()];
    both.sort();
    assert_eq!(kept, both);
    assert!(h.set_calls().is_empty(), "不得覆蓋：{:?}", h.set_calls());
    assert!(
        h.registry.writes.is_empty(),
        "不寫登錄：{:?}",
        h.registry.writes
    );
    assert_eq!(h.desk().wallpaper(DEV_A), Some(photo.clone()));
    assert_eq!(h.desk().wallpaper(DEV_B), Some(photo.clone()));
    let st = h.state_on_disk();
    assert_eq!(st.status, TakeoverStatus::NotTakenOver);
    assert_eq!(st.monitors[&key(DEV_A, 0)].original_wallpaper, photo);
}

#[test]
fn restore_mixed_monitors_restores_host_ones_and_skips_registry() {
    let mut h = Harness::new("r2-mixed");
    let mut t = take_over_both(&mut h);
    let photo = photo_on(&h, &[0]);
    h.registry.writes.clear();
    h.clear_calls();
    let out = h.restore(&mut t, RestoreReason::NotTakeover);
    let (kept, _) = restored_parts(&out);
    assert_eq!(kept, vec![DEV_A.to_owned()]);
    assert_eq!(
        h.set_calls(),
        vec![format!("set_wallpaper {DEV_B} {}", h.sunset())],
        "只還原仍是宿主圖的 B；全域設定與登錄都不動"
    );
    assert!(h.registry.writes.is_empty(), "{:?}", h.registry.writes);
    assert_eq!(h.desk().wallpaper(DEV_A), Some(photo));
    assert_eq!(h.desk().wallpaper(DEV_B), Some(h.sunset()));
}

#[test]
fn yield_write_failure_then_command_line_restore_still_does_not_overwrite() {
    let mut h = Harness::new("r2-yield-writefail");
    let mut t = take_over_both(&mut h);
    let photo = photo_on(&h, &[0]);
    // 讓位當下狀態檔寫不進去：磁碟上仍是接管中、A 的紀錄仍是舊原桌布。
    let blocker = h.root.join("blocker");
    fs::write(&blocker, b"x").unwrap();
    t.paths.state_file = blocker.join(STATE_FILE_NAME);
    assert!(matches!(
        h.set(&mut t, DEV_A, &h.out("a-2.png")),
        SetOutcome::Yielded { .. }
    ));
    assert_eq!(h.state_on_disk().status, TakeoverStatus::TakenOver);

    // 之後（例如解除安裝）執行 --restore-wallpaper。
    let mut t = h.load();
    h.registry.writes.clear();
    h.clear_calls();
    let out = h.restore(&mut t, RestoreReason::CommandLine);
    let (kept, _) = restored_parts(&out);
    assert_eq!(kept, vec![DEV_A.to_owned()]);
    assert!(!h.set_calls().iter().any(|c| c.contains(DEV_A)));
    assert!(h.registry.writes.is_empty(), "{:?}", h.registry.writes);
    assert_eq!(h.desk().wallpaper(DEV_A), Some(photo));
}

#[test]
fn restore_readback_failure_sets_nothing_and_keeps_state() {
    let mut h = Harness::new("r2-restore-readfail");
    let mut t = take_over_both(&mut h);
    h.shared.lock().unwrap().fail_reads = 1;
    h.registry.writes.clear();
    h.clear_calls();
    let out = h.restore(&mut t, RestoreReason::TrayExit);
    assert!(
        matches!(out.result, RestoreResult::Failed { .. }),
        "{out:?}"
    );
    assert!(h.set_calls().is_empty());
    assert!(h.registry.writes.is_empty(), "不知道螢幕上是什麼時不寫登錄");
    assert_eq!(h.state_on_disk().status, TakeoverStatus::TakenOver);
}

// 裁決 2：設定後確認已套用

#[test]
fn confirm_applied_closes_the_not_yet_applied_window() {
    let mut h = Harness::new("r2-confirm");
    h.with_desk(|d| d.monitors.truncate(1));
    let mut t = h.load();
    let img = h.out("a-1.png");
    assert_eq!(h.set(&mut t, DEV_A, &img), SetOutcome::Applied);
    assert!(
        !h.confirm(&mut t, DEV_A, &h.sunset()),
        "讀回不是宿主的圖：不確認"
    );
    assert!(!h.state_on_disk().monitors[&key(DEV_A, 0)].host_applied);
    assert!(h.confirm(&mut t, DEV_A, &img.to_string_lossy()));
    assert!(h.state_on_disk().monitors[&key(DEV_A, 0)].host_applied);

    // 使用者在「最近使用的圖片」點回原圖：下一次設定前必須讓位，而不是當成「尚未套用」覆蓋。
    let sunset = h.sunset();
    h.with_desk(|d| d.monitors[0].wallpaper = Some(sunset));
    assert!(matches!(
        h.set(&mut t, DEV_A, &h.out("a-2.png")),
        SetOutcome::Yielded { .. }
    ));
}

// 裁決 3：theme_selected(None) 不解除 Stopped

#[test]
fn theme_none_does_not_clear_stopped() {
    let mut h = Harness::new("r2-none-stopped");
    let mut t = take_over_both(&mut h);
    photo_on(&h, &[0]);
    assert!(matches!(
        h.set(&mut t, DEV_A, &h.out("a-2.png")),
        SetOutcome::Yielded { .. }
    ));
    t.theme_selected(WallpaperTheme::None);
    assert_eq!(t.phase(), Phase::Stopped);
    h.clear_calls();
    assert_eq!(
        h.set(&mut t, DEV_B, &h.out("b-2.png")),
        SetOutcome::Refused(RefuseReason::Stopped),
        "同一次重畫的其餘螢幕不得走首次接管"
    );
    assert!(h.set_calls().is_empty());
}

// 裁決 4：顯示宿主圖的新螢幕，原桌布記為未知

#[test]
fn new_monitor_showing_host_image_gets_unknown_original() {
    let mut h = Harness::new("r2-unknown-new");
    h.with_desk(|d| d.monitors.truncate(1));
    let mut t = h.load();
    let a1 = h.out("a-1.png");
    assert_eq!(h.set(&mut t, DEV_A, &a1), SetOutcome::Applied);
    // Windows 把新接上的 B 設成最近一次設定的圖（宿主為 A 產生的輸出圖）。
    let a1s = a1.to_string_lossy().into_owned();
    h.with_desk(|d| {
        d.monitors.push(FakeMonitor {
            device_path: DEV_B.to_owned(),
            rect: Some(rect(3840)),
            wallpaper: Some(a1s),
        })
    });
    assert_eq!(h.set(&mut t, DEV_B, &h.out("b-1.png")), SetOutcome::Applied);
    let rec = h.state_on_disk().monitors[&key(DEV_B, 3840)].clone();
    assert!(rec.original_unknown, "不得把宿主的圖記成原桌布");
    assert_eq!(rec.backup, None, "也不得備份它");
    assert_eq!(h.backups(), vec![format!("{}.jpg", key(DEV_A, 0))]);

    // 還原：已知的原桌布只有一種（sunset）→ B 也還原為它。
    let out = h.restore(&mut t, RestoreReason::NotTakeover);
    restored_parts(&out);
    assert_eq!(h.desk().wallpaper(DEV_B), Some(h.sunset()));
}

#[test]
fn unknown_original_fallback_rules() {
    let p = paths();
    let user = "C:\\Users\\ölaf\\Pictures\\x.jpg";
    let other = "C:\\Users\\ölaf\\Pictures\\y.jpg";
    let a = record(user, None, true);
    let b = record(&user.to_uppercase(), None, true);
    let c = record(other, None, true);
    let reg_file = "C:\\Windows\\Web\\Wallpaper\\Windows\\img0.jpg";
    let reg = RegValue::string(REG_TYPE_SZ, reg_file);
    let all = |_: &Path| true;
    assert_eq!(
        unknown_original_fallback(&[&a, &b], Some(&reg), &p, &all, &NoFileIdentity),
        RestoreTarget::Image(user.to_owned()),
        "已知的原桌布都是同一張"
    );
    assert_eq!(
        unknown_original_fallback(&[&a, &c], Some(&reg), &p, &all, &NoFileIdentity),
        RestoreTarget::Image(reg_file.to_owned()),
        "不一致時用登錄記錄的原桌布"
    );
    assert_eq!(
        unknown_original_fallback(
            &[],
            Some(&reg),
            &p,
            &|q: &Path| q != Path::new(reg_file),
            &NoFileIdentity
        ),
        RestoreTarget::Missing,
        "登錄的檔案不存在 → 呼叫端改純色並警告"
    );
    let transcoded = RegValue::string(REG_TYPE_SZ, TRANSCODED);
    assert_eq!(
        unknown_original_fallback(&[&a, &c], Some(&transcoded), &p, &all, &NoFileIdentity),
        RestoreTarget::Missing,
        "TranscodedWallpaper 不是原桌布"
    );
    let host = RegValue::string(
        REG_TYPE_SZ,
        "C:\\Users\\ölaf\\AppData\\Local\\fc\\wallpaper\\m-1-a.png",
    );
    assert_eq!(
        unknown_original_fallback(&[], Some(&host), &p, &all, &NoFileIdentity),
        RestoreTarget::Missing,
        "宿主輸出圖不是原桌布"
    );
    let solid_a = record("", None, true);
    let solid_b = record("", None, true);
    assert_eq!(
        unknown_original_fallback(&[&solid_a, &solid_b], None, &p, &all, &NoFileIdentity),
        RestoreTarget::Solid
    );
    std::env::set_var("FC_HOST_TEST_WALLROOT", "C:\\Windows");
    let expand = RegValue::string(
        REG_TYPE_EXPAND_SZ,
        "%FC_HOST_TEST_WALLROOT%\\Web\\Wallpaper\\Windows\\img0.jpg",
    );
    assert_eq!(
        unknown_original_fallback(&[], Some(&expand), &p, &all, &NoFileIdentity),
        RestoreTarget::Image(reg_file.to_owned()),
        "REG_EXPAND_SZ 展開環境變數"
    );
}

// 裁決 5：螢幕鍵相撞

const DEV_C: &str =
    "\\\\?\\DISPLAY#DELA0B1#5&2222&0&UID9999#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";

#[test]
fn stale_key_hit_with_other_device_is_recorded_as_new_monitor() {
    let mut h = Harness::new("r2-key-stale");
    h.stable = vec![StableDisplay {
        device_path: "STABLE-1".to_owned(),
        rect: rect(0),
    }];
    h.with_desk(|d| d.monitors.truncate(1));
    let mut t = h.load();
    assert_eq!(h.set(&mut t, DEV_A, &h.out("a-1.png")), SetOutcome::Applied);
    let shared_key = monitor_key(DEV_A, Some(rect(0)), &h.stable);
    assert_eq!(h.state_on_disk().monitors[&shared_key].device_path, DEV_A);

    // 拔掉 A、在同一位置接上 C；4.7 的對應表還是舊的，C 會算出 A 的鍵。
    let other = h.pictures.join("c.png");
    fs::write(&other, b"C").unwrap();
    let other = other.to_string_lossy().into_owned();
    let o = other.clone();
    h.with_desk(move |d| {
        d.monitors[0] = FakeMonitor {
            device_path: DEV_C.to_owned(),
            rect: Some(rect(0)),
            wallpaper: Some(o),
        }
    });
    assert_eq!(h.set(&mut t, DEV_C, &h.out("c-1.png")), SetOutcome::Applied);
    let st = h.state_on_disk();
    let a = &st.monitors[&shared_key];
    assert_eq!(a.device_path, DEV_A, "A 的紀錄不得被覆寫");
    assert_eq!(a.original_wallpaper, h.sunset());
    let c: Vec<&MonitorRecord> = st
        .monitors
        .values()
        .filter(|r| r.device_path == DEV_C)
        .collect();
    assert_eq!(c.len(), 1, "C 以另一個鍵記錄為新螢幕");
    assert_eq!(c[0].original_wallpaper, other);
}

#[test]
fn begin_state_key_collision_keeps_both_records() {
    let mut h = Harness::new("r2-key-begin");
    h.stable = vec![StableDisplay {
        device_path: "STABLE-1".to_owned(),
        rect: rect(0),
    }];
    let other = h.pictures.join("c.png");
    fs::write(&other, b"C").unwrap();
    let other = other.to_string_lossy().into_owned();
    let o = other.clone();
    // 兩台在線螢幕的矩形相同（例如複製顯示），算出同一個鍵。
    h.with_desk(move |d| {
        d.monitors[1] = FakeMonitor {
            device_path: DEV_C.to_owned(),
            rect: Some(rect(0)),
            wallpaper: Some(o),
        }
    });
    let mut t = h.load();
    assert_eq!(h.set(&mut t, DEV_A, &h.out("a-1.png")), SetOutcome::Applied);
    let st = h.state_on_disk();
    let devices: Vec<&str> = st
        .monitors
        .values()
        .map(|r| r.device_path.as_str())
        .collect();
    assert_eq!(st.monitors.len(), 2, "{devices:?}");
    let shared_key = monitor_key(DEV_A, Some(rect(0)), &h.stable);
    assert_eq!(
        st.monitors[&shared_key].device_path, DEV_A,
        "先記錄的保留在原鍵"
    );
    assert!(st
        .monitors
        .values()
        .any(|r| r.device_path == DEV_C && r.original_wallpaper == other));
}

// 裁決 6：登錄在 explorer 非同步處理 SetWallpaper 之後才寫回

#[test]
fn registry_written_back_after_explorer_async_rewrite() {
    let mut h = Harness::new("r2-async-reg");
    let mut t = take_over_both(&mut h);
    // 還原前的登錄值與 explorer 之後會寫的不同，才能觀察到「已處理」。
    h.registry.put(
        DESKTOP_KEY,
        "Wallpaper",
        RegValue::string(REG_TYPE_SZ, "PRE-RESTORE"),
    );
    // 修正輪 2：確定性模擬——explorer 在設定之後第 3 次讀登錄時才改寫（不靠計時，高負載下也不會偶發失敗）。
    let sim = Arc::new(Mutex::new(ExplorerSim {
        values: h.registry.shared(),
        rewrite_to: TRANSCODED.to_owned(),
        pending: 0,
        polls: 0,
        polls_before_rewrite: 3,
        events: Vec::new(),
        pending_target: None,
        last_rewrite: None,
    }));
    h.shared.lock().unwrap().explorer_registry = Some(Arc::clone(&sim));
    h.registry.explorer = Some(Arc::clone(&sim));
    let out = h.restore(&mut t, RestoreReason::TrayExit);
    restored_parts(&out);
    let events = sim.lock().unwrap().events.clone();
    let rewrite = events.iter().position(|e| *e == "explorer-rewrite");
    let first_write = events.iter().position(|e| *e == "restore-write");
    assert!(
        matches!((rewrite, first_write), (Some(r), Some(w)) if r < w),
        "登錄要等 explorer 改寫之後才寫回：{events:?}"
    );
    assert_eq!(
        events.last(),
        Some(&"restore-write"),
        "最後一筆是宿主寫回：{events:?}"
    );
    assert_eq!(sim.lock().unwrap().pending, 0, "explorer 已處理完");
    assert_eq!(h.current_registry(), h.original_registry());
}

/// 審查 M5：「全部螢幕」還原時 explorer 把登錄寫成**原圖路徑**（不是 `TranscodedWallpaper`）；逐螢幕還原才寫成轉存檔。
/// 兩種情況登錄最終都被宿主逐字寫回原值。
#[test]
fn explorer_rewrites_registry_to_the_image_for_a_global_set_and_to_transcoded_for_per_monitor() {
    for (name, per_monitor) in [("m5-global", false), ("m5-per-monitor", true)] {
        let mut h = Harness::new(name);
        if per_monitor {
            h.per_monitor_setting();
        }
        let mut t = take_over_both(&mut h);
        let sim = Arc::new(Mutex::new(ExplorerSim {
            values: h.registry.shared(),
            rewrite_to: TRANSCODED.to_owned(),
            pending: 0,
            polls: 0,
            polls_before_rewrite: 2,
            events: Vec::new(),
            pending_target: None,
            last_rewrite: None,
        }));
        h.shared.lock().unwrap().explorer_registry = Some(Arc::clone(&sim));
        h.registry.explorer = Some(Arc::clone(&sim));
        let out = h.restore(&mut t, RestoreReason::TrayExit);
        restored_parts(&out);
        let written = sim.lock().unwrap().last_rewrite.clone();
        let want = if per_monitor {
            TRANSCODED.to_owned()
        } else {
            h.sunset()
        };
        assert_eq!(written, Some(want), "{name}");
        assert_eq!(h.current_registry(), h.original_registry(), "{name}");
    }
}

// 裁決 8：暫時性讀檔失敗

fn lock_file(path: &Path) -> fs::File {
    use std::os::windows::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .expect("鎖住狀態檔")
}

#[test]
fn transient_read_error_does_not_block_and_is_retried() {
    let mut h = Harness::new("r2-transient");
    drop(take_over_both(&mut h));
    let lock = lock_file(&h.paths.state_file);
    let mut t = h.load();
    assert!(t.blocked().is_none(), "暫時性錯誤不封鎖");
    assert!(t.unavailable().is_some());
    assert!(t.already_taken_over(), "不確定時保守視為接管中");
    h.clear_calls();
    assert_eq!(
        h.set(&mut t, DEV_A, &h.out("a-2.png")),
        SetOutcome::Refused(RefuseReason::StateUnavailable)
    );
    assert!(h.calls().is_empty());
    assert!(!h
        .paths
        .state_file
        .with_file_name(format!("{STATE_FILE_NAME}.corrupt-1000"))
        .exists());

    drop(lock);
    assert_eq!(
        h.set(&mut t, DEV_A, &h.out("a-2.png")),
        SetOutcome::Applied,
        "下一次評估重新讀檔"
    );
    assert!(t.unavailable().is_none());
}

#[test]
fn tray_exit_rereads_once_before_giving_up() {
    let mut h = Harness::new("r2-transient-exit");
    drop(take_over_both(&mut h));
    let lock = lock_file(&h.paths.state_file);
    let mut t = h.load();
    assert!(t.unavailable().is_some());
    let releaser = thread::spawn(move || {
        thread::sleep(Duration::from_millis(40));
        drop(lock);
    });
    let out = h.restore(&mut t, RestoreReason::TrayExit);
    releaser.join().unwrap();
    restored_parts(&out);
    assert_desk_restored(&h);
}

#[test]
fn corrupt_still_blocks_after_transient_handling() {
    let h = Harness::new("r2-corrupt-still");
    fs::create_dir_all(h.paths.state_file.parent().unwrap()).unwrap();
    fs::write(&h.paths.state_file, b"not json").unwrap();
    let t = h.load();
    assert!(matches!(t.blocked(), Some(BlockReason::Corrupt(_))));
    assert!(t.unavailable().is_none());
}

// 裁決 9：孤兒備份

#[test]
fn orphan_backups_are_removed_and_referenced_ones_kept() {
    let mut h = Harness::new("r2-orphans");
    let mut t = take_over_both(&mut h);
    let stray = h.paths.backup_dir().join("m-00000000deadbeef.jpg");
    fs::write(&stray, b"stray").unwrap();
    let unrelated = h.paths.backup_dir().join("readme.txt");
    fs::write(&unrelated, b"keep").unwrap();
    photo_on(&h, &[0]);
    assert!(matches!(
        h.set(&mut t, DEV_A, &h.out("a-2.png")),
        SetOutcome::Yielded { .. }
    ));
    let st = h.state_on_disk();
    assert_eq!(st.monitors[&key(DEV_A, 0)].backup, None);
    let a_backup = format!("{}.jpg", key(DEV_A, 0));
    let b_backup = format!("{}.jpg", key(DEV_B, 3840));
    assert_eq!(
        st.monitors[&key(DEV_B, 3840)].backup.as_deref(),
        Some(b_backup.as_str())
    );
    // task 6.4（審查 R1-M4）：讓位時被取代的 A 舊備份保留到下一次接管開始，不在同一次寫入刪掉。
    assert_eq!(st.superseded_backups, vec![a_backup.clone()]);
    let mut want = vec![a_backup, b_backup, "readme.txt".to_owned()];
    want.sort();
    assert_eq!(
        h.backups(),
        want,
        "只刪沒有紀錄引用、也不是被取代保留中的 m-* 備份"
    );
}

// 裁決 10：檔案識別（假的識別提供者）

#[test]
fn file_identity_is_second_check_for_host_output_and_original() {
    let p = paths();
    let mut ids = FakeIds::default();
    let out_dir = p.output_dir.to_string_lossy().into_owned();
    let short_dir = "C:\\Users\\OLAF~1\\AppData\\Local\\fc\\wallpaper".to_owned();
    ids.add(&out_dir, 1);
    ids.add(&short_dir, 1);
    let long_a = format!("{out_dir}\\m-1-a.png");
    let short_a = format!("{short_dir}\\M-1-A.PNG");
    ids.add(&long_a, 2);
    ids.add(&short_a, 2);
    let user_long = "C:\\Users\\ölaf\\Pictures\\x.jpg";
    // 同一個磁碟機上的 junction（修正輪 4 起，不同磁碟機不比識別）。
    let user_junction = r"C:\Pics\x.jpg";
    ids.add(user_long, 3);
    ids.add(user_junction, 3);

    assert!(is_host_output_file(&short_a, &p, &ids), "8.3 寫法的輸出圖");
    assert!(!is_host_output_file(&short_a, &p, &NoFileIdentity));
    assert!(same_file(&short_a, &long_a, &ids));
    assert!(same_file(user_junction, user_long, &ids));
    assert!(!same_file(user_junction, &long_a, &ids));
    assert!(same_file("", "", &ids), "兩邊都是純色");

    let o = original_desktop();
    let r = record(user_long, Some(&long_a), true);
    assert_eq!(
        judge_readback(&r, &o, &short_a, &p, &ids),
        Readback::Host,
        "讀回是上次設定的同一個檔（寫法不同）"
    );
    let fresh = record(user_long, Some(&long_a), false);
    assert_eq!(
        judge_readback(&fresh, &o, user_junction, &p, &ids),
        Readback::NotYetApplied,
        "讀回是原圖（經 junction）"
    );
    assert_eq!(
        judge_readback(&fresh, &o, user_junction, &p, &NoFileIdentity),
        Readback::UserChanged
    );
}

// 裁決 11：待還原寫檔失敗時 state_saved 照實回報

#[test]
fn pending_restore_reports_state_saved_false_on_write_failure() {
    let mut h = Harness::new("r2-pending-saved");
    let mut t = restore_with_b_offline(&mut h);
    b_comes_online(&h, &h.out("b-1.png").to_string_lossy());
    let blocker = h.root.join("blocker");
    fs::write(&blocker, b"x").unwrap();
    t.paths.state_file = blocker.join(STATE_FILE_NAME);
    let out = h.restore(&mut t, RestoreReason::CommandLine);
    let (_, saved) = restored_parts(&out);
    assert!(!saved, "寫檔失敗要照實回報");
    assert_eq!(h.desk().wallpaper(DEV_B), Some(h.sunset()));

    let mut h2 = Harness::new("r2-pending-saved-2");
    let mut t2 = restore_with_b_offline(&mut h2);
    b_comes_online(&h2, &h2.out("b-1.png").to_string_lossy());
    let blocker = h2.root.join("blocker");
    fs::write(&blocker, b"x").unwrap();
    t2.paths.state_file = blocker.join(STATE_FILE_NAME);
    let report = h2.restore_pending(&mut t2);
    assert_eq!(report.restored, vec![DEV_B.to_owned()]);
    assert!(!report.state_saved);
}

// ---------------------------------------------------------------------------------------------
// 修正輪 3
// ---------------------------------------------------------------------------------------------

// 裁決 1：正在當桌布來源的備份不得被當成孤兒刪掉（審查者的完整情境）

#[test]
fn backup_in_use_survives_retakeover_while_its_monitor_is_offline() {
    let mut h = Harness::new("r3-backup-chain");
    let mut t = take_over_both(&mut h);
    // 1. 接管中，使用者刪了原圖。
    fs::remove_file(h.sunset()).unwrap();
    // 2. 還原改用備份：B 的桌布來源從此就是備份檔。
    let out = h.restore(&mut t, RestoreReason::NotTakeover);
    restored_parts(&out);
    let backup_b = h
        .paths
        .backup_dir()
        .join(format!("{}.jpg", key(DEV_B, 3840)));
    assert_eq!(
        h.desk().wallpaper(DEV_B),
        Some(backup_b.to_string_lossy().into_owned())
    );
    // 3. B 離線時重新接管。
    h.with_desk(|d| {
        d.monitors[1].rect = None;
        d.monitors[1].wallpaper = None;
    });
    t.theme_selected(WallpaperTheme::Astrolabe);
    assert_eq!(h.set(&mut t, DEV_A, &h.out("a-3.png")), SetOutcome::Applied);
    // 4. B 的紀錄與它的備份（也就是 B 正在用的桌布來源）都必須還在。
    let st = h.state_on_disk();
    assert!(
        st.monitors.contains_key(&key(DEV_B, 3840)),
        "離線螢幕的紀錄不得因為重新接管而丟棄"
    );
    assert_eq!(
        fs::read(&backup_b).expect("B 的桌布來源檔不得被刪"),
        ORIGINAL_BYTES
    );
}

#[test]
fn orphan_cleanup_keeps_backups_referenced_by_original_or_in_use() {
    let h = Harness::new("r3-orphan-rules");
    let dir = h.paths.backup_dir();
    fs::create_dir_all(&dir).unwrap();
    for n in 1..=5 {
        fs::write(dir.join(format!("m-{n}.jpg")), b"x").unwrap();
    }
    let full = |n: u32| {
        dir.join(format!("m-{n}.jpg"))
            .to_string_lossy()
            .into_owned()
    };
    let mut by_backup = record("C:\\pics\\one.jpg", None, true);
    by_backup.backup = Some("m-1.jpg".to_owned());
    // 原路徑就是備份檔（從備份還原後重新記錄、螢幕鍵已改變）。
    let by_original = record(&full(2), None, true);
    // 原路徑是同一個檔的另一種寫法（以檔案識別比對）。
    // 與備份資料夾同一個磁碟機（修正輪 4 起，不同磁碟機不比識別）。
    let drive = dir.to_string_lossy().chars().next().unwrap();
    let other_form = format!(r"{drive}:\SHORT~1\M-5.JPG");
    let other_form = other_form.as_str();
    let by_identity = record(other_form, None, true);
    let mut ids = FakeIds::default();
    ids.add(other_form, 55);
    ids.add(&full(5), 55);
    let mut state = StateFile::empty();
    state.monitors.insert("k1".to_owned(), by_backup);
    state.monitors.insert("k2".to_owned(), by_original);
    state.monitors.insert("k5".to_owned(), by_identity);
    // m-3：沒有紀錄引用，但某台在線螢幕目前的桌布就是它。
    let in_use = vec![full(3).to_uppercase()];

    remove_orphan_backups(&h.paths, &state, &in_use, &ids);
    let left = h.backups();
    assert_eq!(
        left,
        vec!["m-1.jpg", "m-2.jpg", "m-3.jpg", "m-5.jpg"],
        "只刪真正的孤兒 m-4"
    );
}

// 裁決 2：在線但讀不到桌布的螢幕＝未知

#[test]
fn restore_with_online_monitor_readback_none_skips_registry_and_keeps_it_pending() {
    let mut h = Harness::new("r3-online-none");
    let mut t = take_over_both(&mut h);
    h.with_desk(|d| d.monitors[1].wallpaper = None); // B 在線但這次讀不到
    h.registry.writes.clear();
    h.clear_calls();
    let out = h.restore(&mut t, RestoreReason::TrayExit);
    restored_parts(&out);
    assert!(
        h.registry.writes.is_empty(),
        "不知道 B 上是什麼時不得寫登錄：{:?}",
        h.registry.writes
    );
    assert!(
        !h.set_calls().iter().any(|c| c.starts_with("set_position")),
        "也不動全域設定"
    );
    assert_eq!(h.desk().wallpaper(DEV_A), Some(h.sunset()), "A 照常還原");
    let st = h.state_on_disk();
    assert!(
        st.monitors[&key(DEV_B, 3840)].pending_restore,
        "B 標為待還原"
    );
}

// 裁決 3：begin_state 先讓沿用舊紀錄的螢幕取回舊鍵，再配新鍵

#[test]
fn begin_state_reused_monitor_keeps_its_key_regardless_of_enumeration_order() {
    let mut h = Harness::new("r3-two-pass");
    h.stable = vec![StableDisplay {
        device_path: "STABLE-1".to_owned(),
        rect: rect(0),
    }];
    h.with_desk(|d| d.monitors.truncate(1));
    let mut t = h.load();
    let a1 = h.out("a-1.png");
    assert_eq!(h.set(&mut t, DEV_A, &a1), SetOutcome::Applied);
    let key_a = monitor_key(DEV_A, Some(rect(0)), &h.stable);
    // A 離線時還原 → A 待還原（仍顯示宿主的圖）。
    h.with_desk(|d| {
        d.monitors[0].rect = None;
        d.monitors[0].wallpaper = None;
    });
    restored_parts(&h.restore(&mut t, RestoreReason::NotTakeover));
    assert!(h.state_on_disk().monitors[&key_a].pending_restore);

    // 新螢幕 C 接在 A 原本的位置（對應表過期 → C 算出 A 的鍵），且列舉時排在 A 前面；
    // A 改接到另一個位置，仍顯示宿主的圖。
    let c_pic = h.pictures.join("c.jpg");
    fs::write(&c_pic, b"C-JPEG").unwrap();
    let c_pic = c_pic.to_string_lossy().into_owned();
    let a1s = a1.to_string_lossy().into_owned();
    h.with_desk(move |d| {
        d.monitors = vec![
            FakeMonitor {
                device_path: DEV_C.to_owned(),
                rect: Some(rect(0)),
                wallpaper: Some(c_pic),
            },
            FakeMonitor {
                device_path: DEV_A.to_owned(),
                rect: Some(rect(3840)),
                wallpaper: Some(a1s),
            },
        ]
    });
    t.theme_selected(WallpaperTheme::Astrolabe);
    assert_eq!(h.set(&mut t, DEV_C, &h.out("c-1.png")), SetOutcome::Applied);
    let st = h.state_on_disk();
    let a = &st.monitors[&key_a];
    assert_eq!(a.device_path, DEV_A, "A 取回自己的舊鍵");
    assert!(!a.original_unknown, "A 沿用真正的原桌布紀錄");
    assert_eq!(a.original_wallpaper, h.sunset());
    assert_eq!(
        fs::read(h.paths.backup_dir().join(format!("{key_a}.jpg"))).unwrap(),
        ORIGINAL_BYTES,
        "A 的備份不得被 C 覆寫"
    );
    assert_eq!(
        st.monitors
            .values()
            .filter(|r| r.device_path == DEV_C)
            .count(),
        1
    );
}

#[test]
fn new_record_never_takes_a_key_whose_backup_another_monitor_is_displaying() {
    let mut h = Harness::new("r3-key-reserved");
    h.stable = vec![StableDisplay {
        device_path: "STABLE-1".to_owned(),
        rect: rect(0),
    }];
    h.with_desk(|d| d.monitors.truncate(1));
    let mut t = h.load();
    assert_eq!(h.set(&mut t, DEV_A, &h.out("a-1.png")), SetOutcome::Applied);
    let key_a = monitor_key(DEV_A, Some(rect(0)), &h.stable);
    // A 的原圖被刪、從備份還原：A 的桌布來源從此是 m-<A 的鍵>.jpg。
    fs::remove_file(h.sunset()).unwrap();
    restored_parts(&h.restore(&mut t, RestoreReason::NotTakeover));
    let backup_a = h.paths.backup_dir().join(format!("{key_a}.jpg"));
    let backup_a_s = backup_a.to_string_lossy().into_owned();
    assert_eq!(h.desk().wallpaper(DEV_A), Some(backup_a_s.clone()));

    // A 改接到另一個位置（算出別的鍵），新螢幕 C 接在 A 原本的位置（對應表過期 → C 算出 A 的舊鍵）。
    let c_pic = h.pictures.join("c.jpg");
    fs::write(&c_pic, b"C-JPEG").unwrap();
    let c_pic = c_pic.to_string_lossy().into_owned();
    let a_now = backup_a_s.clone();
    h.with_desk(move |d| {
        d.monitors = vec![
            FakeMonitor {
                device_path: DEV_C.to_owned(),
                rect: Some(rect(0)),
                wallpaper: Some(c_pic),
            },
            FakeMonitor {
                device_path: DEV_A.to_owned(),
                rect: Some(rect(3840)),
                wallpaper: Some(a_now),
            },
        ]
    });
    t.theme_selected(WallpaperTheme::Astrolabe);
    assert_eq!(h.set(&mut t, DEV_C, &h.out("c-1.png")), SetOutcome::Applied);
    assert_eq!(
        fs::read(&backup_a).expect("A 正在顯示的備份檔不得被刪"),
        ORIGINAL_BYTES,
        "A 正在顯示的備份檔不得被 C 的原圖覆寫"
    );
    let st = h.state_on_disk();
    let c_key = st
        .monitors
        .iter()
        .find(|(_, r)| r.device_path == DEV_C)
        .map(|(k, _)| k.clone())
        .unwrap();
    assert_ne!(c_key, key_a, "C 不得取用 A 舊紀錄的鍵");
}

// 離線紀錄保留後：重新接上時若已不是宿主的圖，重新記錄而不是讓位

#[test]
fn kept_offline_record_coming_back_with_other_image_is_rerecorded_not_yielded() {
    let mut h = Harness::new("r3-stale");
    let mut t = take_over_both(&mut h);
    restored_parts(&h.restore(&mut t, RestoreReason::NotTakeover));
    h.with_desk(|d| {
        d.monitors[1].rect = None;
        d.monitors[1].wallpaper = None;
    });
    t.theme_selected(WallpaperTheme::Astrolabe);
    assert_eq!(h.set(&mut t, DEV_A, &h.out("a-3.png")), SetOutcome::Applied);
    // B 離線期間被使用者換成照片，之後接上。
    let photo = h.pictures.join("photo.jpg");
    fs::write(&photo, b"PHOTO").unwrap();
    let photo = photo.to_string_lossy().into_owned();
    b_comes_online(&h, &photo);
    assert_eq!(
        h.set(&mut t, DEV_A, &h.out("a-4.png")),
        SetOutcome::Applied,
        "這次接管從未碰過 B：不是讓位"
    );
    assert_eq!(h.set(&mut t, DEV_B, &h.out("b-4.png")), SetOutcome::Applied);
    let b = h.state_on_disk().monitors[&key(DEV_B, 3840)].clone();
    assert_eq!(b.original_wallpaper, photo, "以接上時的桌布重新記錄");
    assert!(!b.stale);
}

// 裁決 4：狀態檔自己的持久寫入

#[test]
fn durable_write_replaces_atomically_and_leaves_no_temp() {
    let h = Harness::new("r3-durable");
    let p = h.root.join("sub").join("f.json");
    write_durable(&p, b"one").unwrap();
    write_durable(&p, b"two").unwrap();
    assert_eq!(fs::read(&p).unwrap(), b"two");
    assert!(!p.with_file_name("f.json.tmp").exists());
}

// ---------------------------------------------------------------------------------------------
// 純函式
// ---------------------------------------------------------------------------------------------

fn paths() -> StatePaths {
    StatePaths {
        state_file: PathBuf::from("C:\\Users\\ölaf\\AppData\\Roaming\\fc\\wallpaper-state.json"),
        output_dir: PathBuf::from("C:\\Users\\ölaf\\AppData\\Local\\fc\\wallpaper"),
        themes_dir: PathBuf::from("C:\\Users\\ölaf\\AppData\\Roaming\\Microsoft\\Windows\\Themes"),
    }
}

fn original_desktop() -> OriginalDesktop {
    OriginalDesktop {
        recorded_at: 0,
        position: 4,
        background_color: 0,
        slideshow_status: SLIDESHOW_STATE_ENABLED,
        slideshow: None,
        registry: PersistedRegistry {
            wallpaper: RegValue::Missing,
            wallpaper_style: RegValue::Missing,
            tile_wallpaper: RegValue::Missing,
        },
        spotlight: PersistedSpotlight {
            enabled_state: None,
            background_type: None,
        },
        solid_color: false,
        all_monitors: false,
    }
}

fn record(original: &str, last_set: Option<&str>, applied: bool) -> MonitorRecord {
    MonitorRecord {
        device_path: DEV_A.to_owned(),
        original_wallpaper: original.to_owned(),
        backup: None,
        recorded_at: 0,
        last_set: last_set.map(str::to_owned),
        host_applied: applied,
        pending_restore: false,
        original_unknown: false,
        stale: false,
        original_file_id: None,
        solid_transition_readback: None,
    }
}

#[test]
fn judge_readback_rules() {
    let p = paths();
    let o = original_desktop();
    let ours = "C:\\Users\\ölaf\\AppData\\Local\\fc\\wallpaper\\m-1-a.png";
    let other_ours = "C:\\Users\\ölaf\\AppData\\Local\\fc\\wallpaper\\m-1-b.png";
    let user = "C:\\Users\\ölaf\\Pictures\\x.jpg";
    let r = record(user, Some(ours), true);
    assert_eq!(
        judge_readback(&r, &o, ours, &p, &NoFileIdentity),
        Readback::Host
    );
    assert_eq!(
        judge_readback(&r, &o, &ours.to_uppercase(), &p, &NoFileIdentity),
        Readback::Host,
        "非 ASCII 大小寫不同仍是同一路徑"
    );
    assert_eq!(
        judge_readback(&r, &o, other_ours, &p, &NoFileIdentity),
        Readback::Host,
        "輸出資料夾內的另一個檔（a/b 交替、設定途中當機）"
    );
    assert_eq!(
        judge_readback(&r, &o, user, &p, &NoFileIdentity),
        Readback::UserChanged,
        "已確認套用過宿主的圖之後，換回原圖也是使用者自換"
    );
    assert_eq!(
        judge_readback(&r, &o, "", &p, &NoFileIdentity),
        Readback::UserChanged
    );
    let backup = "C:\\Users\\ölaf\\AppData\\Local\\fc\\wallpaper\\original\\m-1.jpg";
    assert_eq!(
        judge_readback(&r, &o, backup, &p, &NoFileIdentity),
        Readback::UserChanged,
        "原圖備份資料夾不算宿主輸出"
    );

    let fresh = record(user, Some(ours), false);
    assert_eq!(
        judge_readback(&fresh, &o, &user.to_uppercase(), &p, &NoFileIdentity),
        Readback::NotYetApplied,
        "尚未確認套用：讀回仍是原圖"
    );
    assert_eq!(
        judge_readback(
            &fresh,
            &o,
            "C:\\Users\\ölaf\\Pictures\\y.jpg",
            &p,
            &NoFileIdentity
        ),
        Readback::UserChanged
    );
}

#[test]
fn judge_readback_slideshow_and_spotlight_before_first_apply() {
    let p = paths();
    let mut o = original_desktop();
    o.slideshow = Some(PersistedSlideshow {
        items: vec!["C:\\Users\\ölaf\\Pictures\\Album".to_owned()],
        options: 0,
        tick_ms: 60_000,
    });
    let r = record("C:\\Users\\ölaf\\Pictures\\Album\\1.jpg", None, false);
    assert_eq!(
        judge_readback(
            &r,
            &o,
            "C:\\Users\\ölaf\\Pictures\\Album\\7.jpg",
            &p,
            &NoFileIdentity
        ),
        Readback::NotYetApplied,
        "投影片換到下一張不是使用者自換"
    );
    let applied = record("C:\\Users\\ölaf\\Pictures\\Album\\1.jpg", None, true);
    assert_eq!(
        judge_readback(
            &applied,
            &o,
            "C:\\Users\\ölaf\\Pictures\\Album\\7.jpg",
            &p,
            &NoFileIdentity
        ),
        Readback::UserChanged
    );

    let mut o = original_desktop();
    o.spotlight.enabled_state = Some(1);
    let iris = "C:\\Users\\ölaf\\AppData\\Local\\Packages\\X\\LocalCache\\IrisService\\a.jpg";
    let r = record(iris, None, false);
    assert_eq!(
        judge_readback(
            &r,
            &o,
            "C:\\Users\\ölaf\\AppData\\Local\\Packages\\X\\LocalCache\\IrisService\\b.jpg",
            &p,
            &NoFileIdentity
        ),
        Readback::NotYetApplied,
        "焦點輪替同資料夾的下一張"
    );
}

#[test]
fn restore_target_rules() {
    let p = paths();
    let user = "C:\\Users\\ölaf\\Pictures\\x.jpg";
    let mut r = record(user, None, true);
    r.backup = Some("m-1.jpg".to_owned());
    let backup_full = p
        .backup_dir()
        .join("m-1.jpg")
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        restore_target(&r, &p, &|_: &Path| true, &NoFileIdentity),
        RestoreTarget::Image(user.to_owned())
    );
    assert_eq!(
        restore_target(&r, &p, &|q: &Path| q != Path::new(user), &NoFileIdentity),
        RestoreTarget::Image(backup_full.clone()),
        "原檔不存在改用備份"
    );
    assert_eq!(
        restore_target(&r, &p, &|_: &Path| false, &NoFileIdentity),
        RestoreTarget::Missing
    );
    assert_eq!(
        restore_target(
            &record("", None, true),
            &p,
            &|_: &Path| true,
            &NoFileIdentity
        ),
        RestoreTarget::Solid
    );
    // 記錄當下就是宿主的輸出圖（狀態檔遺失後重新接管）：輸出圖會被輪替覆寫，一律用備份。
    let mut r = record(
        "C:\\Users\\ölaf\\AppData\\Local\\fc\\wallpaper\\m-1-a.png",
        None,
        true,
    );
    r.backup = Some("m-1.jpg".to_owned());
    assert_eq!(
        restore_target(&r, &p, &|_: &Path| true, &NoFileIdentity),
        RestoreTarget::Image(backup_full)
    );
}

/// task 6.4（審查 R1-M3b）：原路徑在桌布快取資料夾、或檔案識別已與記錄不同，有備份就用備份；沒有備份
/// 才用原路徑。識別相同或查不到時照舊用原路徑。
#[test]
fn restore_target_distrusts_cache_and_replaced_originals() {
    let p = paths();
    let backup_full = p
        .backup_dir()
        .join("m-1.jpg")
        .to_string_lossy()
        .into_owned();
    let cache =
        "C:\\Users\\ölaf\\AppData\\Roaming\\Microsoft\\Windows\\Themes\\TranscodedWallpaper";
    let mut r = record(cache, None, true);
    r.backup = Some("m-1.jpg".to_owned());
    assert_eq!(
        restore_target(&r, &p, &|_: &Path| true, &NoFileIdentity),
        RestoreTarget::Image(backup_full.clone()),
        "快取資料夾內的原路徑：用備份"
    );
    r.backup = None;
    assert_eq!(
        restore_target(&r, &p, &|_: &Path| true, &NoFileIdentity),
        RestoreTarget::Image(cache.to_owned()),
        "沒有備份只能用原路徑"
    );

    let user = "C:\\Users\\ölaf\\Pictures\\x.jpg";
    let mut r = record(user, None, true);
    r.backup = Some("m-1.jpg".to_owned());
    r.original_file_id = Some(PersistedFileId {
        volume_serial: 7,
        file_index: 1,
    });
    let mut ids = FakeIds::default();
    ids.add(user, 1);
    assert_eq!(
        restore_target(&r, &p, &|_: &Path| true, &ids),
        RestoreTarget::Image(user.to_owned()),
        "識別相同：原路徑"
    );
    let mut replaced = FakeIds::default();
    replaced.add(user, 2);
    assert_eq!(
        restore_target(&r, &p, &|_: &Path| true, &replaced),
        RestoreTarget::Image(backup_full),
        "識別不同（檔案被換掉）：用備份"
    );
    assert_eq!(
        restore_target(&r, &p, &|_: &Path| true, &NoFileIdentity),
        RestoreTarget::Image(user.to_owned()),
        "查不到識別：不臆測，用原路徑"
    );
}

/// task 6.4 修正第 2 輪（複審 N1）：桌布快取只認 explorer 的工作檔——`themes_dir`（Roaming）下的
/// `TranscodedWallpaper`、同目錄 `Transcoded_` 開頭的逐螢幕轉存檔、`CachedFiles\` 子資料夾，以及任何位置
/// 檔名就是 `TranscodedWallpaper` 的檔。使用者套用的佈景主題資料夾（`%LOCALAPPDATA%\Microsoft\Windows\Themes\
/// <名稱>\DesktopBackground\`）、`themes_dir` 裡其他的檔、一般圖片、宿主輸出、空字串都不是。
#[test]
fn themes_cache_detection() {
    let p = paths();
    for yes in [
        "C:\\Users\\ölaf\\AppData\\Roaming\\Microsoft\\Windows\\Themes\\TranscodedWallpaper",
        "c:\\users\\ÖLAF\\appdata\\roaming\\microsoft\\windows\\themes\\transcoded_000",
        "c:\\users\\ölaf\\appdata\\roaming\\microsoft\\windows\\themes\\CachedFiles\\x.jpg",
        "C:\\Users\\ölaf\\AppData\\Roaming\\Microsoft\\Windows\\Themes\\CachedFiles\\sub\\y.jpg",
        "D:\\elsewhere\\TranscodedWallpaper",
        // task 6.1 B6：實機 explorer 用的是 `TranscodedWallpaperCache\`（`CachedFiles\` 沒出現過）。
        "C:\\Users\\ölaf\\AppData\\Roaming\\Microsoft\\Windows\\Themes\\TranscodedWallpaperCache\\TranscodedWallpaper_0123abcd",
        "c:\\users\\ölaf\\appdata\\roaming\\microsoft\\windows\\themes\\transcodedwallpapercache\\TranscodedWallpaper_0123abcd.meta",
    ] {
        assert!(p.is_themes_cache(yes), "{yes}");
    }
    for no in [
        "",
        "C:\\Users\\ölaf\\Pictures\\Themes\\x.jpg",
        "C:\\Users\\ölaf\\AppData\\Local\\fc\\wallpaper\\m-1-a.png",
        "C:\\Windows\\Web\\Wallpaper\\Windows\\img0.jpg",
        "C:\\Users\\ölaf\\AppData\\Local\\Microsoft\\Windows\\Themes\\Nature\\DesktopBackground\\img1.jpg",
        "C:\\Users\\ölaf\\AppData\\Roaming\\Microsoft\\Windows\\Themes\\Custom\\DesktopBackground\\a.jpg",
        "C:\\Users\\ölaf\\AppData\\Roaming\\Microsoft\\Windows\\Themes\\slideshow.jpg",
        "\\\\srv\\profiles\\u\\AppData\\Roaming\\Microsoft\\Windows\\Themes\\Transcoded_000",
        "C:\\Users\\ölaf\\AppData\\Roaming\\Microsoft\\Windows\\Themes\\CachedFilesOld\\x.jpg",
        "C:\\Users\\ölaf\\AppData\\Roaming\\Microsoft\\Windows\\Themes\\TranscodedWallpaperCacheOld\\x.jpg",
        "C:\\Users\\ölaf\\Pictures\\TranscodedWallpaperCache\\x.jpg",
    ] {
        assert!(!p.is_themes_cache(no), "{no}");
    }
}

/// task 6.4 新欄位（`original_file_id`、`superseded_backups`）：沒有值時不寫出，舊檔讀得進來。
#[test]
fn new_optional_fields_are_omitted_when_empty() {
    let mut h = Harness::new("r64-fields");
    drop(take_over_both(&mut h));
    let text = fs::read_to_string(&h.paths.state_file).unwrap();
    assert!(!text.contains("superseded_backups"), "{text}");
    assert!(
        !text.contains("original_file_id"),
        "FakeIds 查不到識別：{text}"
    );
    assert!(h.load().blocked().is_none());
}

// ---------------------------------------------------------------------------------------------
// 修正輪 4
// ---------------------------------------------------------------------------------------------

const DEVS: [&str; 2] = [DEV_A, DEV_B];
const XS: [i32; 2] = [0, 3840];

/// 只有 `known` 那台在線時接管；之後接上 `unknown` 那台（Windows 把宿主的圖套給它）並設定：
/// `unknown` 的原桌布記為未知。
fn take_over_with_unknown(h: &mut Harness, known: usize, unknown: usize) -> WallpaperTakeover {
    let saved = h.desk().monitors[unknown].clone();
    h.with_desk(move |d| {
        d.monitors.remove(unknown);
    });
    let mut t = h.load();
    let k_out = h.out("k-1.png");
    assert_eq!(h.set(&mut t, DEVS[known], &k_out), SetOutcome::Applied);
    let host = k_out.to_string_lossy().into_owned();
    h.with_desk(move |d| {
        let mut m = saved;
        m.wallpaper = Some(host);
        d.monitors.insert(unknown, m);
    });
    assert_eq!(
        h.set(&mut t, DEVS[unknown], &h.out("u-1.png")),
        SetOutcome::Applied
    );
    h.explorer_rewrites_registry();
    assert!(
        h.state_on_disk().monitors[&key(DEVS[unknown], XS[unknown])].original_unknown,
        "前提：這台的原桌布未知"
    );
    t
}

fn set_offline(h: &Harness, i: usize) {
    h.with_desk(move |d| {
        d.monitors[i].rect = None;
        d.monitors[i].wallpaper = None;
    });
}

/// 原桌布未知的螢幕必須擁有自己的備份：它顯示的檔就是它紀錄的 `backup`。
fn assert_owns_shown_backup(h: &Harness, i: usize, shown: &str) {
    let st = h.state_on_disk();
    let rec = &st.monitors[&key(DEVS[i], XS[i])];
    let own = rec
        .backup
        .as_ref()
        .map(|b| h.paths.backup_dir().join(b).to_string_lossy().into_owned());
    assert!(
        own.as_deref().is_some_and(|o| path_eq(o, shown)),
        "這台顯示的 {shown} 必須是它自己的備份（紀錄：{rec:?}）"
    );
    assert!(path_eq(&rec.original_wallpaper, shown));
    assert!(!rec.original_unknown, "沿用的結果採納為它的原桌布");
    for (k, other) in &st.monitors {
        if k != &key(DEVS[i], XS[i]) {
            assert_ne!(other.backup, rec.backup, "備份檔只屬於一筆紀錄");
        }
    }
}

/// 審查觸發 A：原桌布未知的螢幕從別台的備份還原、之後離線，那台以新圖重新記錄。
fn unknown_fallback_copy_survives_other_rerecording(ext: &str, name: &str) {
    let mut h = Harness::new(name);
    let (known, unknown) = (0, 1);
    let mut t = take_over_with_unknown(&mut h, known, unknown);
    // 1. 接管中使用者刪了原圖；還原時已知那台改用它的備份，未知那台沿用同一張。
    fs::remove_file(h.sunset()).unwrap();
    restored_parts(&h.restore(&mut t, RestoreReason::NotTakeover));
    let shown = h.desk().wallpaper(DEVS[unknown]).expect("在線");
    assert_eq!(fs::read(&shown).unwrap(), ORIGINAL_BYTES);
    // 2. 未知那台離線；使用者把已知那台換成新圖，再重新接管。
    set_offline(&h, unknown);
    let q = h.pictures.join(format!("q.{ext}"));
    fs::write(&q, b"Q-NEW-IMAGE").unwrap();
    let q = q.to_string_lossy().into_owned();
    h.with_desk(move |d| d.monitors[known].wallpaper = Some(q));
    t.theme_selected(WallpaperTheme::Astrolabe);
    assert_eq!(
        h.set(&mut t, DEVS[known], &h.out("k-3.png")),
        SetOutcome::Applied
    );
    // 3. 未知那台正在用的桌布來源（也是原圖僅存的副本）不得被刪、也不得被換掉內容。
    assert_eq!(
        fs::read(&shown).expect("離線螢幕正在用的桌布來源不得被刪"),
        ORIGINAL_BYTES,
        "也不得被別台的新備份覆寫"
    );
    assert_owns_shown_backup(&h, unknown, &shown);
}

#[test]
fn unknown_fallback_copy_survives_other_rerecording_same_extension() {
    unknown_fallback_copy_survives_other_rerecording("jpg", "r4-fallback-a-jpg");
}

#[test]
fn unknown_fallback_copy_survives_other_rerecording_other_extension() {
    unknown_fallback_copy_survives_other_rerecording("png", "r4-fallback-a-png");
}

/// 審查觸發 B：同一次 `restore_pending_monitors` 內，原桌布未知的螢幕先沿用已知那台的備份，
/// 已知那台接著被判為使用者自換（放掉它的備份）。
#[test]
fn pending_unknown_fallback_backup_survives_same_pass_adoption() {
    let mut h = Harness::new("r4-fallback-b");
    // BTreeMap 中原桌布未知那台要排在前面。
    let (known, unknown) = if key(DEV_B, 3840) < key(DEV_A, 0) {
        (0, 1)
    } else {
        (1, 0)
    };
    let mut t = take_over_with_unknown(&mut h, known, unknown);
    set_offline(&h, 0);
    set_offline(&h, 1);
    restored_parts(&h.restore(&mut t, RestoreReason::NotTakeover));
    let st = h.state_on_disk();
    assert!(
        st.monitors.values().all(|r| r.pending_restore),
        "前提：兩台都待還原"
    );
    fs::remove_file(h.sunset()).unwrap();
    let u = h.pictures.join("u.png");
    fs::write(&u, b"U").unwrap();
    let u = u.to_string_lossy().into_owned();
    let host = h.out("u-1.png").to_string_lossy().into_owned();
    h.with_desk(move |d| {
        d.monitors[known].rect = Some(rect(XS[known]));
        d.monitors[known].wallpaper = Some(u);
        d.monitors[unknown].rect = Some(rect(XS[unknown]));
        d.monitors[unknown].wallpaper = Some(host);
    });
    let report = h.restore_pending(&mut t);
    assert_eq!(report.restored, vec![DEVS[unknown].to_owned()]);
    assert_eq!(report.cleared, vec![DEVS[known].to_owned()]);
    let shown = h.desk().wallpaper(DEVS[unknown]).expect("在線");
    assert_eq!(
        fs::read(&shown).expect("剛還原上去的桌布檔不得在同一次呼叫內被刪"),
        ORIGINAL_BYTES
    );
    assert!(
        t.seen_wallpapers.iter().any(|w| path_eq(w, &shown)),
        "逐螢幕還原成功的圖列入在用保護：{:?}",
        t.seen_wallpapers
    );
    assert_owns_shown_backup(&h, unknown, &shown);
}

/// 裁決 3：`stale` 紀錄不參與「共同原圖」的判定。
#[test]
fn unknown_fallback_ignores_stale_records() {
    let p = paths();
    let user = r"C:\Users\ölaf\Pictures\x.jpg";
    let old = r"C:\Users\ölaf\Pictures\old.jpg";
    let a = record(user, None, true);
    let mut b = record(old, None, false);
    b.stale = true;
    let all = |_: &Path| true;
    assert_eq!(
        unknown_original_fallback(&[&a, &b], None, &p, &all, &NoFileIdentity),
        RestoreTarget::Image(user.to_owned()),
        "離線很久的紀錄不得讓共同原圖不成立"
    );
    assert_eq!(
        unknown_original_fallback(&[&b], None, &p, &all, &NoFileIdentity),
        RestoreTarget::Missing,
        "只有 stale 紀錄時也不採用它很久以前的圖"
    );
}

/// 記下被查識別的路徑。
#[derive(Default)]
struct CountingIds {
    inner: FakeIds,
    asked: std::cell::RefCell<Vec<String>>,
}

impl FileIdentity for CountingIds {
    fn file_id(&self, path: &Path) -> Option<FileId> {
        self.asked
            .borrow_mut()
            .push(path.to_string_lossy().into_owned());
        self.inner.file_id(path)
    }
}

/// 裁決 2：只在路徑不相等、且兩邊在同一個本機磁碟機時才查識別（開檔）。
#[test]
fn identity_only_queried_for_different_paths_on_same_drive() {
    let mut ids = CountingIds::default();
    let user = r"C:\Users\ölaf\Pictures\x.jpg";
    let short = r"C:\Users\OLAF~1\Pictures\x.jpg";
    let verbatim = r"\\?\C:\Users\ölaf\Pictures\x.jpg";
    let other_drive = r"Z:\Pics\x.jpg";
    for p in [user, short, verbatim, other_drive] {
        ids.inner.add(p, 3);
    }
    assert!(same_file(user, r"c:\USERS\ÖLAF\pictures\X.JPG", &ids));
    assert!(ids.asked.borrow().is_empty(), "路徑已相等：不開檔");
    assert!(!same_file(other_drive, user, &ids), "不同磁碟機不比識別");
    assert!(ids.asked.borrow().is_empty(), "不同磁碟機：不開檔");
    assert!(same_file(short, user, &ids), "同磁碟機的 8.3 寫法");
    assert!(
        same_file(verbatim, user, &ids),
        r"`\\?\` 前綴視為同一個磁碟機"
    );
    assert_eq!(ids.asked.borrow().len(), 4);

    // 輸出資料夾在 C:，Z: 上的檔不查它所在資料夾的識別。
    let p = paths();
    ids.asked.borrow_mut().clear();
    assert!(!is_host_output_file(r"Z:\fc\wallpaper\m-1-a.png", &p, &ids));
    assert!(ids.asked.borrow().is_empty(), "{:?}", ids.asked.borrow());
}

// ---------------------------------------------------------------------------------------------
// 修正輪 5
// ---------------------------------------------------------------------------------------------

const DEV_D: &str = r"\\?\DISPLAY#GSM5B09#6&3333&0&UID7777#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";

fn add_monitor(h: &Harness, device: &str, x: i32, wallpaper: &str) {
    let m = FakeMonitor {
        device_path: device.to_owned(),
        rect: Some(rect(x)),
        wallpaper: Some(wallpaper.to_owned()),
    };
    h.with_desk(move |d| d.monitors.push(m));
}

/// 裁決 1：使用者從「最近使用的背景」選了宿主的備份檔 P；P 的主人之後以同副檔名的新圖重新記錄，
/// 不得覆寫 P，之後還原仍回到 P。
///
/// `b_offline`：A 重新記錄時 B 離線（P 不在「正在顯示」之列，只靠 B 的紀錄引用保護）。
fn picked_backup_chain(name: &str, b_offline: bool) {
    let mut h = Harness::new(name);
    let mut t = take_over_both(&mut h);
    let p_path = h.paths.backup_dir().join(format!("{}.jpg", key(DEV_A, 0)));
    let p = p_path.to_string_lossy().into_owned();
    assert_eq!(fs::read(&p_path).unwrap(), ORIGINAL_BYTES);
    // 1. 使用者替 B 選了 P → 讓位，B 的原桌布記為 P。
    let pc = p.clone();
    h.with_desk(move |d| d.monitors[1].wallpaper = Some(pc));
    assert!(matches!(
        h.set(&mut t, DEV_A, &h.out("a-2.png")),
        SetOutcome::Yielded { .. }
    ));
    assert!(path_eq(
        &h.state_on_disk().monitors[&key(DEV_B, 3840)].original_wallpaper,
        &p
    ));
    // 2. 使用者把 A 換成同副檔名的新圖 Q，再選主題重新接管。
    let q = h.pictures.join("q.jpg");
    fs::write(&q, b"Q-NEW-IMAGE").unwrap();
    let qs = q.to_string_lossy().into_owned();
    h.with_desk(move |d| d.monitors[0].wallpaper = Some(qs));
    if b_offline {
        set_offline(&h, 1);
    }
    t.theme_selected(WallpaperTheme::Astrolabe);
    assert_eq!(h.set(&mut t, DEV_A, &h.out("a-3.png")), SetOutcome::Applied);
    if b_offline {
        b_comes_online(&h, &p);
    }
    assert_eq!(
        fs::read(&p_path).unwrap(),
        ORIGINAL_BYTES,
        "B 引用的 P 不得被 A 的新備份覆寫"
    );
    let st = h.state_on_disk();
    let a_backup = h.paths.backup_dir().join(
        st.monitors[&key(DEV_A, 0)]
            .backup
            .as_ref()
            .expect("A 有備份"),
    );
    assert!(
        !path_eq(&a_backup.to_string_lossy(), &p),
        "A 的新備份改用另一個檔名"
    );
    assert_eq!(fs::read(&a_backup).unwrap(), b"Q-NEW-IMAGE");
    // 3. B 也被接管，之後還原：B 回到 P 的內容。
    assert_eq!(h.set(&mut t, DEV_B, &h.out("b-3.png")), SetOutcome::Applied);
    restored_parts(&h.restore(&mut t, RestoreReason::NotTakeover));
    let shown = h.desk().wallpaper(DEV_B).expect("在線");
    assert!(path_eq(&shown, &p), "{shown}");
    assert_eq!(fs::read(&shown).unwrap(), ORIGINAL_BYTES);
    assert_eq!(
        h.desk().wallpaper(DEV_A),
        Some(q.to_string_lossy().into_owned())
    );
}

#[test]
fn user_picked_host_backup_is_not_overwritten_by_its_owner_rerecording() {
    picked_backup_chain("r5-picked-backup", false);
}

#[test]
fn user_picked_host_backup_is_not_overwritten_while_its_picker_is_offline() {
    picked_backup_chain("r5-picked-backup-offline", true);
}

/// 裁決 1：備份的命名規則與孤兒清理。
#[test]
fn backup_never_overwrites_a_file_another_record_references() {
    let h = Harness::new("r5-backup-name");
    let dir = h.paths.backup_dir();
    fs::create_dir_all(&dir).unwrap();
    let full = |n: &str| dir.join(n).to_string_lossy().into_owned();
    fs::write(full("m-k.jpg"), b"P").unwrap();
    fs::write(full("m-k-2.jpg"), b"OTHER-2").unwrap();
    fs::write(full("m-k-4.jpg"), b"STALE").unwrap();
    // 別筆紀錄：原桌布是 m-k.jpg（使用者選的）；另一筆的備份是 m-k-2.jpg（鍵相撞時的序號鍵）。
    let picked = record(&full("m-k.jpg"), None, true);
    let mut numbered = record(r"C:\x.jpg", None, true);
    numbered.backup = Some("m-k-2.jpg".to_owned());
    let mut own = record(&h.sunset(), None, true);
    own.backup = Some("m-k.jpg".to_owned());
    let mut all = BTreeMap::new();
    all.insert("m-z".to_owned(), picked);
    all.insert("m-k-2".to_owned(), numbered);
    all.insert("m-k".to_owned(), own.clone());
    let guard = BackupGuard {
        paths: &h.paths,
        states: vec![&all],
        in_use: &[],
        files: &NoFileIdentity,
    };
    let name = backup_original(&guard, "m-k", &h.sunset());
    assert_eq!(name.as_deref(), Some("m-k-3.jpg"));
    assert_eq!(fs::read(full("m-k.jpg")).unwrap(), b"P");
    assert_eq!(fs::read(full("m-k-2.jpg")).unwrap(), b"OTHER-2");
    assert_eq!(fs::read(full("m-k-3.jpg")).unwrap(), ORIGINAL_BYTES);
    // 來源就是目的檔本身（內容不變）時照常沿用。
    assert_eq!(
        backup_original(&guard, "m-k", &full("m-k.jpg")).as_deref(),
        Some("m-k.jpg")
    );
    assert_eq!(fs::read(full("m-k.jpg")).unwrap(), b"P");
    // 沒有人引用時沿用自己的鍵名（覆寫自己的舊備份）。
    let empty = BTreeMap::new();
    let free = BackupGuard {
        paths: &h.paths,
        states: vec![&empty],
        in_use: &[],
        files: &NoFileIdentity,
    };
    assert_eq!(
        backup_original(&free, "m-k", &h.sunset()).as_deref(),
        Some("m-k.jpg")
    );
    // 正在顯示的檔也不覆寫。
    let shown = [full("m-k.jpg")];
    let in_use = BackupGuard {
        paths: &h.paths,
        states: vec![&empty],
        in_use: &shown,
        files: &NoFileIdentity,
    };
    assert_eq!(
        backup_original(&in_use, "m-k", &h.sunset()).as_deref(),
        Some("m-k-2.jpg")
    );

    // 孤兒清理認得序號檔名：自己現在的 m-k-3 與別人引用的都留下，沒人用的 m-k-4 刪掉。
    let mut state = StateFile::empty();
    state.monitors = all;
    state.monitors.get_mut("m-k").unwrap().backup = Some("m-k-3.jpg".to_owned());
    remove_orphan_backups(&h.paths, &state, &[], &NoFileIdentity);
    assert_eq!(h.backups(), vec!["m-k-2.jpg", "m-k-3.jpg", "m-k.jpg"]);
}

/// 只有 A 在線時接管，再接上 B、C（Windows 把宿主的圖套給它們）並設定：B、C 的原桌布未知。
fn take_over_with_two_unknown(h: &mut Harness) -> WallpaperTakeover {
    h.with_desk(|d| d.monitors.truncate(1));
    let mut t = h.load();
    let a1 = h.out("a-1.png");
    assert_eq!(h.set(&mut t, DEV_A, &a1), SetOutcome::Applied);
    let host = a1.to_string_lossy().into_owned();
    add_monitor(h, DEV_B, 3840, &host);
    assert_eq!(h.set(&mut t, DEV_B, &h.out("b-1.png")), SetOutcome::Applied);
    add_monitor(h, DEV_C, 7680, &host);
    assert_eq!(h.set(&mut t, DEV_C, &h.out("c-1.png")), SetOutcome::Applied);
    h.explorer_rewrites_registry();
    let st = h.state_on_disk();
    assert!(st.monitors[&key(DEV_B, 3840)].original_unknown);
    assert!(st.monitors[&key(DEV_C, 7680)].original_unknown);
    t
}

/// 裁決 2：同一次 `restore_pending_monitors` 裡兩台原桌布未知的螢幕都拿到共同原圖（與
/// `restore_now` 一致），不因第一台先採納複本而讓第二台退成純色。
#[test]
fn pending_two_unknown_monitors_both_get_the_shared_original() {
    let mut h = Harness::new("r5-pending-two-unknown");
    let mut t = take_over_with_two_unknown(&mut h);
    h.with_desk(|d| {
        for m in &mut d.monitors {
            m.rect = None;
            m.wallpaper = None;
        }
    });
    restored_parts(&h.restore(&mut t, RestoreReason::NotTakeover));
    assert!(h
        .state_on_disk()
        .monitors
        .values()
        .all(|r| r.pending_restore));
    fs::remove_file(h.sunset()).unwrap();
    let outs: Vec<String> = ["a-1.png", "b-1.png", "c-1.png"]
        .iter()
        .map(|n| h.out(n).to_string_lossy().into_owned())
        .collect();
    h.with_desk(move |d| {
        for (i, m) in d.monitors.iter_mut().enumerate() {
            m.rect = Some(rect(3840 * i as i32));
            m.wallpaper = Some(outs[i].clone());
        }
    });
    let report = h.restore_pending(&mut t);
    assert_eq!(report.restored.len(), 3, "{report:?}");
    for dev in [DEV_A, DEV_B, DEV_C] {
        let shown = h.desk().wallpaper(dev).expect("在線");
        assert!(!shown.is_empty(), "{dev} 不得退成純色");
        assert_eq!(fs::read(&shown).unwrap(), ORIGINAL_BYTES, "{dev}");
    }
}

/// 裁決 2：讓位時同樣先算完所有螢幕要還原成什麼，再採納（含使用者自換那台的新選擇）。
#[test]
fn yield_two_unknown_monitors_both_get_the_shared_original() {
    let mut h = Harness::new("r5-yield-two-unknown");
    let mut t = take_over_both(&mut h);
    let host = h.out("a-1.png").to_string_lossy().into_owned();
    add_monitor(&h, DEV_C, 7680, &host);
    assert_eq!(h.set(&mut t, DEV_C, &h.out("c-1.png")), SetOutcome::Applied);
    add_monitor(&h, DEV_D, 11520, &host);
    assert_eq!(h.set(&mut t, DEV_D, &h.out("d-1.png")), SetOutcome::Applied);
    let st = h.state_on_disk();
    assert!(st.monitors[&key(DEV_C, 7680)].original_unknown);
    assert!(st.monitors[&key(DEV_D, 11520)].original_unknown);
    fs::remove_file(h.sunset()).unwrap();
    photo_on(&h, &[1]);
    assert!(matches!(
        h.set(&mut t, DEV_A, &h.out("a-2.png")),
        SetOutcome::Yielded { .. }
    ));
    for dev in [DEV_A, DEV_C, DEV_D] {
        let shown = h.desk().wallpaper(dev).expect("在線");
        assert!(!shown.is_empty(), "{dev} 不得退成純色");
        assert_eq!(fs::read(&shown).unwrap(), ORIGINAL_BYTES, "{dev}");
    }
}

/// 裁決 3：fallback 的複製失敗時，先試登錄記錄的原值，再不行才純色。
#[test]
fn fallback_copy_failure_tries_registry_original_before_solid() {
    let h = Harness::new("r5-copy-fail");
    let dir = h.paths.backup_dir();
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("m-a.jpg"), ORIGINAL_BYTES).unwrap();
    // 讓 `m-c.jpg.tmp` 是資料夾：複製到它一定失敗。
    fs::create_dir_all(dir.join("m-c.jpg.tmp")).unwrap();
    let mut a = record(&h.pictures.join("gone.jpg").to_string_lossy(), None, true);
    a.backup = Some("m-a.jpg".to_owned());
    let mut c = record("", None, false);
    c.original_unknown = true;
    let mut all = BTreeMap::new();
    all.insert("m-a".to_owned(), a);
    all.insert("m-c".to_owned(), c.clone());
    let reg_file = h.pictures.join("reg.jpg");
    fs::write(&reg_file, b"REG").unwrap();
    let reg_file = reg_file.to_string_lossy().into_owned();
    let reg = RegValue::string(REG_TYPE_SZ, &reg_file);
    let guard = BackupGuard {
        paths: &h.paths,
        states: vec![&all],
        in_use: &[],
        files: &NoFileIdentity,
    };
    let mut warnings = Vec::new();
    let want = restore_want("m-c", &c, "", &all, Some(&reg), &guard, &mut warnings);
    assert_eq!(
        want,
        Want {
            path: reg_file,
            own_copy: None
        }
    );
    assert!(!warnings.is_empty(), "複製失敗要記警告");
    let mut warnings = Vec::new();
    let want = restore_want("m-c", &c, "", &all, None, &guard, &mut warnings);
    assert_eq!(want.path, "", "登錄也沒有可用的原值：純色");
}

// ---------------------------------------------------------------------------------------------
// 修正輪 R（複審 `task-4.4-rereview-4.md`）
// ---------------------------------------------------------------------------------------------

/// 複審 4 medium：原桌布未知那台的備份由別台複製而來；第一次 `restore` 在設定之後、寫回全域設定
/// 與狀態之前中斷，重跑時那台正顯示自己上次寫下的複本。它必須判為「已是原桌布」並重用該複本，
/// 整次照常寫回登錄、填滿方式、背景色——不得被誤判成使用者自選、在沒寫回的情況下丟掉 `original`。
#[test]
fn restore_retry_reuses_own_fallback_copy_and_still_restores_globals() {
    let mut h = Harness::new("rR-retry-own-copy");
    let (known, unknown) = (0, 1);
    let mut t = take_over_with_unknown(&mut h, known, unknown);
    // 已知那台的原圖被刪：未知那台的 fallback 選中已知那台的備份，要複製成自己的備份。
    fs::remove_file(h.sunset()).unwrap();
    let own_copy = h
        .paths
        .backup_dir()
        .join(format!("{}.jpg", key(DEVS[unknown], XS[unknown])))
        .to_string_lossy()
        .into_owned();

    // 1. 第一次還原：設定都生效，但之後的讀回全部失敗 → 驗證失敗、狀態檔不變。
    h.shared.lock().unwrap().ok_reads_before_fail = Some(1);
    let out = h.restore(&mut t, RestoreReason::TrayExit);
    assert!(
        matches!(out.result, RestoreResult::Failed { .. }),
        "{out:?}"
    );
    let shown = h.desk().wallpaper(DEVS[unknown]).expect("在線");
    assert!(
        path_eq(&shown, &own_copy),
        "前提：未知那台已顯示自己的複本：{shown}"
    );
    let st = h.state_on_disk();
    assert_eq!(st.status, TakeoverStatus::TakenOver);
    assert!(st.original.is_some(), "前提：原桌布設定仍在狀態檔");
    assert!(
        st.monitors[&key(DEVS[unknown], XS[unknown])].original_unknown,
        "前提：狀態檔沒有更新"
    );

    // 2. 模擬第一次在寫回全域設定之前就被終止（或 explorer 事後又改寫）：填滿方式仍是宿主的
    //    FILL、登錄是 explorer 改寫後的值、背景色也不是原值。
    h.shared.lock().unwrap().ok_reads_before_fail = None;
    h.with_desk(|d| {
        d.position = WallpaperPosition::FILL.0;
        d.color = 0x00ab_cdef;
    });
    h.explorer_rewrites_registry();

    // 3. 重跑（新行程）：未知那台顯示的是自己的複本＝已是原桌布，整次照常寫回全域設定。
    h.now += 10;
    let mut t = h.load();
    let out = h.restore(&mut t, RestoreReason::CommandLine);
    let (kept, saved) = restored_parts(&out);
    assert!(kept.is_empty(), "自己的複本不是使用者的選擇：{out:?}");
    assert!(saved);
    let d = h.desk();
    assert_eq!(d.position, ORIGINAL_POSITION, "填滿方式要寫回");
    assert_eq!(d.color, ORIGINAL_COLOR, "背景色要寫回");
    assert_eq!(h.current_registry(), h.original_registry(), "登錄要寫回");
    // 4. 重用既有複本、不另寫序號檔；它採納為這台的原桌布。
    let shown = d.wallpaper(DEVS[unknown]).expect("在線");
    assert!(path_eq(&shown, &own_copy), "{shown}");
    assert_eq!(fs::read(&shown).unwrap(), ORIGINAL_BYTES);
    assert_owns_shown_backup(&h, unknown, &shown);
    assert_eq!(h.backups().len(), 2, "不另寫序號檔：{:?}", h.backups());
}

/// 修正 1 的邊界：`<鍵>-<n>` 也可能是另一筆紀錄（`free_key` 的序號鍵）的備份——被其他紀錄引用的
/// 檔不算這台自己的複本；自己的序號複本（沒人引用）則重用、不另寫。
#[test]
fn shown_own_copy_is_reused_but_not_a_file_another_record_references() {
    let h = Harness::new("rR-own-copy-name");
    let dir = h.paths.backup_dir();
    fs::create_dir_all(&dir).unwrap();
    let full = |n: &str| dir.join(n).to_string_lossy().into_owned();
    fs::write(full("m-a.jpg"), ORIGINAL_BYTES).unwrap();
    fs::write(full("m-c-2.jpg"), b"OTHER").unwrap();
    fs::write(full("m-c-3.jpg"), ORIGINAL_BYTES).unwrap();
    let gone = h.pictures.join("gone.jpg").to_string_lossy().into_owned();
    let mut a = record(&gone, None, true);
    a.backup = Some("m-a.jpg".to_owned());
    let mut numbered = record(&gone, None, true);
    numbered.backup = Some("m-c-2.jpg".to_owned());
    let mut c = record("", None, true);
    c.original_unknown = true;
    let mut all = BTreeMap::new();
    all.insert("m-a".to_owned(), a);
    all.insert("m-c-2".to_owned(), numbered);
    all.insert("m-c".to_owned(), c.clone());

    // C 顯示自己的序號複本 m-c-3.jpg（沒人引用）：重用它，不另寫。
    let shown = [full("m-c-3.jpg")];
    let guard = BackupGuard {
        paths: &h.paths,
        states: vec![&all],
        in_use: &shown,
        files: &NoFileIdentity,
    };
    let mut warnings = Vec::new();
    let want = restore_want("m-c", &c, &shown[0], &all, None, &guard, &mut warnings);
    assert_eq!(
        want,
        Want {
            path: full("m-c-3.jpg"),
            own_copy: Some("m-c-3.jpg".to_owned()),
        }
    );
    assert_eq!(
        classify_for_restore(
            &c,
            None,
            &shown[0],
            &want.path,
            &h.paths,
            &NoFileIdentity,
            false
        ),
        RestoreClass::Original
    );
    assert_eq!(h.backups(), vec!["m-a.jpg", "m-c-2.jpg", "m-c-3.jpg"]);

    // C 顯示 m-c-2.jpg（另一筆紀錄的備份）：不是自己的複本，照常從 fallback 複製成自己的備份。
    let shown = [full("m-c-2.jpg")];
    let guard = BackupGuard {
        paths: &h.paths,
        states: vec![&all],
        in_use: &shown,
        files: &NoFileIdentity,
    };
    let want = restore_want("m-c", &c, &shown[0], &all, None, &guard, &mut warnings);
    assert_eq!(want.own_copy.as_deref(), Some("m-c.jpg"));
    assert_eq!(fs::read(full("m-c-2.jpg")).unwrap(), b"OTHER");
    assert_eq!(fs::read(full("m-c.jpg")).unwrap(), ORIGINAL_BYTES);
}

/// 修正 1 的範圍（審查 fix44R low 1）：原桌布**已知**的紀錄只認自己的 `backup`；鍵名相同、但這筆
/// 紀錄沒有引用的殘留檔（序號檔）不算「已是原桌布」。
#[test]
fn known_record_only_counts_its_own_backup_as_original() {
    let h = Harness::new("rR-known-own-backup");
    let dir = h.paths.backup_dir();
    fs::create_dir_all(&dir).unwrap();
    let full = |n: &str| dir.join(n).to_string_lossy().into_owned();
    fs::write(full("m-c.jpg"), ORIGINAL_BYTES).unwrap();
    fs::write(full("m-c-2.jpg"), b"LEFTOVER").unwrap();
    // 原圖仍在；這台確認套用過宿主的圖。
    let mut c = record(&h.sunset(), None, true);
    c.backup = Some("m-c.jpg".to_owned());
    let mut all = BTreeMap::new();
    all.insert("m-c".to_owned(), c.clone());
    let classify = |shown: &str| {
        let in_use = [shown.to_owned()];
        let guard = BackupGuard {
            paths: &h.paths,
            states: vec![&all],
            in_use: &in_use,
            files: &NoFileIdentity,
        };
        let mut warnings = Vec::new();
        let want = restore_want("m-c", &c, shown, &all, None, &guard, &mut warnings);
        let class = classify_for_restore(
            &c,
            None,
            shown,
            &want.path,
            &h.paths,
            &NoFileIdentity,
            false,
        );
        (want, class)
    };

    // 顯示自己的 backup：已是原桌布，不採納（它本來就是這筆紀錄的備份）。
    let (want, class) = classify(&full("m-c.jpg"));
    assert_eq!(class, RestoreClass::Original);
    assert_eq!(want.own_copy, None);

    // 顯示鍵名相同、但沒被這筆紀錄引用的殘留檔：不是「已是原桌布」，照常以原圖為目標。
    let (want, class) = classify(&full("m-c-2.jpg"));
    assert_ne!(class, RestoreClass::Original, "殘留的同鍵檔不得算原桌布");
    assert!(path_eq(&want.path, &h.sunset()), "{want:?}");
    assert_eq!(want.own_copy, None);
}

/// 複審 4 low：讓位後宿主停止，使用者替 B 選了 A 的備份檔 P，之後 B 離線（Windows 仍回報它的桌布）；
/// 使用者把 A 換成新圖 Q 再重新接管。B 正在用的 P 不得被覆寫（同副檔名），也不得被當成孤兒刪掉
/// （不同副檔名）。
fn offline_picked_backup_chain(ext: &str, name: &str) {
    let mut h = Harness::new(name);
    let mut t = take_over_both(&mut h);
    let p_path = h.paths.backup_dir().join(format!("{}.jpg", key(DEV_A, 0)));
    let p = p_path.to_string_lossy().into_owned();
    // 1. 使用者把 B 換成照片 → 讓位；A 還原回原圖，保留它的備份 P。
    photo_on(&h, &[1]);
    assert!(matches!(
        h.set(&mut t, DEV_A, &h.out("a-2.png")),
        SetOutcome::Yielded { .. }
    ));
    let a_rec = h.state_on_disk().monitors[&key(DEV_A, 0)].clone();
    assert!(
        a_rec
            .backup
            .as_deref()
            .is_some_and(|b| path_eq(&h.paths.backup_dir().join(b).to_string_lossy(), &p)),
        "前提：P 是 A 的備份：{a_rec:?}"
    );
    drop(t);
    // 2. 宿主停止期間：使用者替 B 選了 P，之後 B 離線（讀回仍是 P）；使用者把 A 換成新圖 Q。
    let q = h.pictures.join(format!("q.{ext}"));
    fs::write(&q, b"Q-NEW-IMAGE").unwrap();
    let q = q.to_string_lossy().into_owned();
    let pc = p.clone();
    h.with_desk(move |d| {
        d.monitors[1].wallpaper = Some(pc);
        d.monitors[1].rect = None;
        d.monitors[0].wallpaper = Some(q);
    });
    // 3. 新行程重新接管 A。
    h.now += 10;
    let mut t = h.load();
    assert_eq!(h.set(&mut t, DEV_A, &h.out("a-3.png")), SetOutcome::Applied);
    assert_eq!(
        fs::read(&p_path).expect("離線螢幕正在用的檔不得被當成孤兒刪掉"),
        ORIGINAL_BYTES,
        "也不得被 A 的新備份覆寫"
    );
    let st = h.state_on_disk();
    let a_backup = h.paths.backup_dir().join(
        st.monitors[&key(DEV_A, 0)]
            .backup
            .as_ref()
            .expect("A 有備份"),
    );
    assert!(!path_eq(&a_backup.to_string_lossy(), &p));
    assert_eq!(fs::read(&a_backup).unwrap(), b"Q-NEW-IMAGE");
}

#[test]
fn offline_monitor_showing_a_host_backup_protects_it_same_extension() {
    offline_picked_backup_chain("jpg", "rR-offline-pick-jpg");
}

#[test]
fn offline_monitor_showing_a_host_backup_protects_it_other_extension() {
    offline_picked_backup_chain("png", "rR-offline-pick-png");
}

// ---------------------------------------------------------------------------------------------
// task 4.7b：還原進行中標記（restore_in_progress）
// ---------------------------------------------------------------------------------------------

/// 4.4 寫出的舊狀態檔沒有 `restore_in_progress` 欄位：照常讀取，視為沒有標記。
#[test]
fn old_state_file_without_marker_field_loads_as_no_marker() {
    let mut h = Harness::new("b-old-file");
    drop(take_over_both(&mut h));
    let mut value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&h.paths.state_file).unwrap()).unwrap();
    value.as_object_mut().unwrap().remove("restore_in_progress");
    fs::write(&h.paths.state_file, value.to_string()).unwrap();
    let t = h.load();
    assert!(t.blocked().is_none(), "{:?}", t.blocked());
    assert!(t.already_taken_over());
    assert_eq!(t.restore_marker(), None);
    // 沒有標記時寫出的檔也不帶這個欄位（與 4.4 的檔逐鍵相同）。
    assert!(!fs::read_to_string(&h.paths.state_file)
        .unwrap()
        .contains("restore_in_progress"));
}

/// 還原開始前寫入標記（每個 SetWallpaper 當下磁碟上都已有標記），成功後清除。
#[test]
fn restore_writes_marker_before_first_set_and_clears_it_on_success() {
    let mut h = Harness::new("b-marker-ok");
    let mut t = take_over_both(&mut h);
    h.clear_calls();
    let out = h.restore(&mut t, RestoreReason::TrayExit);
    restored_parts(&out);
    let at_set = h.state_at_set();
    assert!(!at_set.is_empty());
    for content in &at_set {
        let content = content.as_deref().expect("設定當下狀態檔存在");
        let v: serde_json::Value = serde_json::from_str(content).unwrap();
        assert_eq!(v["restore_in_progress"]["reason"], "tray_exit", "{content}");
        assert_eq!(v["restore_in_progress"]["started_at"], h.now);
    }
    let st = h.state_on_disk();
    assert_eq!(st.status, TakeoverStatus::NotTakenOver);
    assert_eq!(st.restore_in_progress, None);
    assert_eq!(t.restore_marker(), None);
}

/// 還原失敗（讀取失敗）：標記留在磁碟上，下次啟動讀得到原因（含安全閥的說明）。
#[test]
fn failed_restore_keeps_marker_for_the_next_start() {
    let mut h = Harness::new("b-marker-failed");
    let mut t = take_over_both(&mut h);
    h.shared.lock().unwrap().fail_reads = 5;
    let out = h.restore(
        &mut t,
        RestoreReason::SafetyValve {
            detail: "GDI 9000".to_owned(),
        },
    );
    assert!(
        matches!(out.result, RestoreResult::Failed { .. }),
        "{out:?}"
    );
    drop(t);
    let t = h.load();
    assert!(t.already_taken_over(), "還原沒做完，仍是接管中");
    let marker = t.restore_marker().expect("標記保留");
    assert_eq!(marker.reason, MarkerReason::SafetyValve);
    assert_eq!(
        marker.restore_reason(),
        RestoreReason::SafetyValve {
            detail: "GDI 9000".to_owned()
        }
    );
    assert_eq!(marker.started_at, h.now);
}

/// 標記的原因與 `RestoreReason` 一一對應。
#[test]
fn marker_reason_round_trips_every_restore_reason() {
    for reason in [
        RestoreReason::NotTakeover,
        RestoreReason::TrayExit,
        RestoreReason::CommandLine,
        RestoreReason::SafetyValve {
            detail: "d".to_owned(),
        },
    ] {
        let marker = RestoreMarker::new(&reason, 7);
        let text = serde_json::to_string(&marker).unwrap();
        let back: RestoreMarker = serde_json::from_str(&text).unwrap();
        assert_eq!(back.restore_reason(), reason, "{text}");
    }
}

/// 標記的原因不認得：與其他讀不懂的內容一樣封鎖（不猜）。
#[test]
fn unknown_marker_reason_blocks() {
    let mut h = Harness::new("b-marker-unknown");
    drop(take_over_both(&mut h));
    let mut value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&h.paths.state_file).unwrap()).unwrap();
    value["restore_in_progress"] = serde_json::json!({ "reason": "teleport", "started_at": 1 });
    fs::write(&h.paths.state_file, value.to_string()).unwrap();
    let t = h.load();
    assert!(matches!(t.blocked(), Some(BlockReason::Corrupt(_))));
}

/// 沒有東西要還原（從未接管）：不寫標記、不建立狀態檔。
#[test]
fn nothing_to_restore_writes_no_marker() {
    let mut h = Harness::new("b-marker-nothing");
    let mut t = h.load();
    let out = h.restore(&mut t, RestoreReason::TrayExit);
    assert_eq!(out.result, RestoreResult::NothingToRestore);
    assert!(!h.paths.state_file.exists());
    assert_eq!(t.restore_marker(), None);
}

/// 使用者重新選了主題（放棄還原）：清除標記並存檔，狀態維持接管中、原桌布記錄不動。
#[test]
fn abandoning_the_restore_clears_the_marker_on_disk() {
    let mut h = Harness::new("b-marker-abandon");
    let mut t = take_over_both(&mut h);
    h.shared.lock().unwrap().fail_reads = 5;
    let _ = h.restore(&mut t, RestoreReason::NotTakeover);
    assert!(t.restore_marker().is_some());
    let original_before = h.state_on_disk().original;
    assert!(t.abandon_restore_marker().unwrap());
    let st = h.state_on_disk();
    assert_eq!(st.restore_in_progress, None);
    assert_eq!(st.status, TakeoverStatus::TakenOver);
    assert_eq!(st.original, original_before);
    assert!(!t.abandon_restore_marker().unwrap(), "沒有標記時不寫");
}

/// 未接管卻留著標記（例如舊版寫入）：還原時順手清掉，不當成「還原進行中」。
#[test]
fn stale_marker_on_not_taken_over_state_is_cleared_by_restore() {
    let mut h = Harness::new("b-marker-stale");
    let mut t = take_over_both(&mut h);
    restored_parts(&h.restore(&mut t, RestoreReason::NotTakeover));
    drop(t);
    let mut value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&h.paths.state_file).unwrap()).unwrap();
    value["restore_in_progress"] = serde_json::json!({ "reason": "tray_exit", "started_at": 1 });
    fs::write(&h.paths.state_file, value.to_string()).unwrap();
    let mut t = h.load();
    assert!(t.restore_marker().is_some());
    h.clear_calls();
    let out = h.restore(&mut t, RestoreReason::TrayExit);
    assert_eq!(out.result, RestoreResult::NothingToRestore);
    assert!(h.set_calls().is_empty(), "{:?}", h.set_calls());
    assert_eq!(t.restore_marker(), None);
    assert_eq!(h.state_on_disk().restore_in_progress, None);
}

// ---------------------------------------------------------------------------------------------
// task 4.7b 修正輪 1：系統匣結束／命令列交接在等協調迴圈之前就寫標記
// ---------------------------------------------------------------------------------------------

/// 外部要求的標記立即寫進狀態檔；協調迴圈之後以記憶體中的狀態（沒有標記）寫檔也不會蓋掉它；
/// 還原成功時照常清除。
#[test]
fn requested_marker_is_written_now_and_survives_a_concurrent_state_write() {
    let mut h = Harness::new("b-requested-marker");
    let mut t = take_over_both(&mut h);
    let request = Arc::new(RestoreRequestMarker::default());
    request.set_paths(h.paths.clone());
    t.attach_restore_request(Arc::clone(&request));
    assert_eq!(
        request.request(&RestoreReason::TrayExit, 77, Duration::from_secs(1)),
        MarkOutcome::Written
    );
    let marker = h.state_on_disk().restore_in_progress.expect("立即寫入");
    assert_eq!(
        (marker.reason, marker.started_at),
        (MarkerReason::TrayExit, 77)
    );
    assert_eq!(t.restore_marker(), None, "記憶體中的狀態不知道外部寫了標記");

    h.now += 1;
    assert_eq!(h.set(&mut t, DEV_A, &h.out("a-2.png")), SetOutcome::Applied);
    assert_eq!(
        h.state_on_disk().restore_in_progress.map(|m| m.reason),
        Some(MarkerReason::TrayExit),
        "協調迴圈的寫入不得蓋掉外部要求的標記"
    );

    restored_parts(&h.restore(&mut t, RestoreReason::TrayExit));
    assert_eq!(h.state_on_disk().restore_in_progress, None);
    // 要求清掉後，之後的寫入不再帶標記。
    request.clear();
    h.now += 1;
    assert_eq!(h.set(&mut t, DEV_A, &h.out("a-3.png")), SetOutcome::Applied);
    assert_eq!(h.state_on_disk().restore_in_progress, None);
}

/// 未接管、狀態檔讀不懂、還沒接上路徑時不寫。
#[test]
fn requested_marker_is_not_written_when_there_is_nothing_to_mark() {
    let h = Harness::new("b-requested-none");
    let request = RestoreRequestMarker::default();
    assert_eq!(
        request.request(&RestoreReason::TrayExit, 1, Duration::from_secs(1)),
        MarkOutcome::NotAttached
    );
    request.set_paths(h.paths.clone());
    assert_eq!(
        request.request(&RestoreReason::TrayExit, 1, Duration::from_secs(1)),
        MarkOutcome::NotNeeded
    );
    assert!(!h.paths.state_file.exists(), "從未接管：不建立狀態檔");
    fs::create_dir_all(h.paths.state_file.parent().unwrap()).unwrap();
    fs::write(&h.paths.state_file, b"not json").unwrap();
    assert_eq!(
        request.request(&RestoreReason::TrayExit, 1, Duration::from_secs(1)),
        MarkOutcome::NotNeeded
    );
    assert_eq!(
        fs::read(&h.paths.state_file).unwrap(),
        b"not json",
        "讀不懂的檔不動"
    );
}

// ---------------------------------------------------------------------------------------------
// task 4.7b 修正輪 2（審查 task-4.7b-review.md F1／F6）
// ---------------------------------------------------------------------------------------------

/// [F6] 外部要求取共用鎖有上限：協調迴圈卡在寫檔（持鎖）時，系統匣結束不會無限期等。
#[test]
fn external_marker_request_gives_up_on_the_lock_within_the_bound() {
    let h = Harness::new("b-fix2-lock-bound");
    let request = Arc::new(RestoreRequestMarker::default());
    request.set_paths(h.paths.clone());
    let held = request.hold_lock_for_test();
    let t = Instant::now();
    assert_eq!(
        request.request(&RestoreReason::TrayExit, 1, Duration::from_millis(80)),
        MarkOutcome::LockTimeout
    );
    let elapsed = t.elapsed();
    assert!(elapsed >= Duration::from_millis(70), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
    drop(held);
    assert_eq!(
        request.request(&RestoreReason::TrayExit, 1, Duration::from_millis(80)),
        MarkOutcome::NotNeeded,
        "鎖放開後照常判定（從未接管：不需要）"
    );
}

/// [F6] 放棄自己的標記時，外部要求的標記（系統匣結束）不得被一起清掉：合併寫入。
#[test]
fn abandoning_own_marker_keeps_an_externally_requested_one() {
    let mut h = Harness::new("b-fix2-abandon-merge");
    let mut t = take_over_both(&mut h);
    let request = Arc::new(RestoreRequestMarker::default());
    request.set_paths(h.paths.clone());
    t.attach_restore_request(Arc::clone(&request));
    h.shared.lock().unwrap().fail_reads = 5;
    let _ = h.restore(&mut t, RestoreReason::NotTakeover);
    assert_eq!(
        t.restore_marker().map(|m| m.reason),
        Some(MarkerReason::NotTakeover)
    );
    // 磁碟上已有（自己的）標記：外部要求只記下、不寫。
    assert_eq!(
        request.request(&RestoreReason::TrayExit, 9, Duration::from_secs(1)),
        MarkOutcome::NotNeeded
    );
    assert!(t.abandon_restore_marker().unwrap());
    assert_eq!(
        h.state_on_disk().restore_in_progress.map(|m| m.reason),
        Some(MarkerReason::TrayExit),
        "外部要求的標記保留"
    );
}

/// [F1] 防禦：接管中且有「還原進行中」標記時，狀態機拒絕設定（不得照常接管、也不做讓位判定）。
#[test]
fn set_is_refused_while_a_restore_marker_is_pending() {
    let mut h = Harness::new("b-fix2-refuse");
    let mut t = take_over_both(&mut h);
    h.shared.lock().unwrap().fail_reads = 5;
    let _ = h.restore(&mut t, RestoreReason::TrayExit);
    h.shared.lock().unwrap().fail_reads = 0;
    drop(t);
    let mut t = h.load();
    h.clear_calls();
    assert_eq!(
        h.set(&mut t, DEV_A, &h.out("a-9.png")),
        SetOutcome::Refused(RefuseReason::RestorePending)
    );
    assert!(h.set_calls().is_empty(), "{:?}", h.set_calls());
    assert!(h.state_on_disk().original.is_some(), "原桌布記錄不丟");
}

// ---------------------------------------------------------------------------------------------
// task 4.9：explorer 資源安全閥的基準（explorer_baseline）
// ---------------------------------------------------------------------------------------------

fn gdi(pid: u32, gdi: u32) -> ExplorerSample {
    ExplorerSample { pid, gdi }
}

/// 4.4／4.7b 寫出的舊狀態檔沒有 `explorer_baseline`：照常讀取，視為沒有基準；沒有基準時寫出的檔
/// 也不帶這個欄位。
#[test]
fn old_state_file_without_explorer_baseline_loads_as_no_baseline() {
    let mut h = Harness::new("v-old-file");
    drop(take_over_both(&mut h));
    let text = fs::read_to_string(&h.paths.state_file).unwrap();
    assert!(!text.contains("explorer_baseline"), "{text}");
    let mut value: serde_json::Value = serde_json::from_str(&text).unwrap();
    value.as_object_mut().unwrap().remove("explorer_baseline");
    fs::write(&h.paths.state_file, value.to_string()).unwrap();
    let mut t = h.load();
    assert!(t.blocked().is_none(), "{:?}", t.blocked());
    assert!(t.already_taken_over());
    assert_eq!(t.valve_baseline(), None);
    assert_eq!(h.set(&mut t, DEV_A, &h.out("a-2.png")), SetOutcome::Applied);
    assert!(!fs::read_to_string(&h.paths.state_file)
        .unwrap()
        .contains("explorer_baseline"));
}

/// 宿主重啟（重新讀檔）沿用基準與 PID；PID 是否相同由排程器判定（`next_action`）。
#[test]
fn explorer_baseline_is_persisted_and_survives_reload() {
    let mut h = Harness::new("v-persist");
    let mut t = take_over_both(&mut h);
    assert!(t.set_explorer_baseline(gdi(4242, 777), h.now).unwrap());
    let st = h.state_on_disk();
    let rec = st.explorer_baseline.expect("已寫入狀態檔");
    assert_eq!((rec.pid, rec.gdi, rec.recorded_at), (4242, 777, h.now));
    assert_eq!(st.status, TakeoverStatus::TakenOver, "其餘內容不變");

    let t2 = h.load();
    assert_eq!(t2.valve_baseline(), Some(gdi(4242, 777)));
}

/// 接管開始前（未接管）寫入的基準＝這次接管開始時的讀值：未接管時不提供給安全閥判定（不沿用上一次
/// 接管的值），接管開始時帶進新的接管狀態。
#[test]
fn explorer_baseline_written_before_takeover_is_carried_into_it() {
    let mut h = Harness::new("v-carry");
    let mut t = h.load();
    assert!(t.set_explorer_baseline(gdi(9, 500), h.now).unwrap());
    assert_eq!(t.valve_baseline(), None, "未接管：判定時一律視為沒有基準");
    assert_eq!(h.set(&mut t, DEV_A, &h.out("a-1.png")), SetOutcome::Applied);
    assert_eq!(t.valve_baseline(), Some(gdi(9, 500)));
    assert_eq!(
        h.state_on_disk().explorer_baseline.map(|b| (b.pid, b.gdi)),
        Some((9, 500))
    );
}

/// 還原成功與讓位（回到未接管）清掉基準：下次接管重新取基準。
#[test]
fn restore_and_yield_clear_explorer_baseline() {
    let mut h = Harness::new("v-clear-restore");
    let mut t = take_over_both(&mut h);
    t.set_explorer_baseline(gdi(1, 100), h.now).unwrap();
    restored_parts(&h.restore(&mut t, RestoreReason::NotTakeover));
    assert_eq!(h.state_on_disk().explorer_baseline, None);

    let mut h = Harness::new("v-clear-yield");
    let mut t = take_over_both(&mut h);
    t.set_explorer_baseline(gdi(1, 100), h.now).unwrap();
    let photo = h.pictures.join("photo.jpg").to_string_lossy().into_owned();
    h.with_desk(|d| d.monitors[0].wallpaper = Some(photo));
    assert!(matches!(
        h.set(&mut t, DEV_A, &h.out("a-2.png")),
        SetOutcome::Yielded { .. }
    ));
    assert_eq!(h.state_on_disk().explorer_baseline, None);
}

/// 使用者重新開啟接管（4.7 呼叫）：清掉基準並存檔。
#[test]
fn clear_explorer_baseline_writes_state_file() {
    let mut h = Harness::new("v-clear");
    let mut t = take_over_both(&mut h);
    assert!(!t.clear_explorer_baseline().unwrap(), "本來就沒有");
    t.set_explorer_baseline(gdi(3, 300), h.now).unwrap();
    assert!(t.clear_explorer_baseline().unwrap());
    assert_eq!(t.valve_baseline(), None);
    assert_eq!(h.state_on_disk().explorer_baseline, None);
    assert_eq!(h.state_on_disk().status, TakeoverStatus::TakenOver);
}

/// 基準欄位型別不符＝讀不懂，與其他欄位一樣封鎖（不默默當成沒有基準）。
#[test]
fn malformed_explorer_baseline_blocks_like_other_fields() {
    let mut h = Harness::new("v-malformed");
    drop(take_over_both(&mut h));
    let mut value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&h.paths.state_file).unwrap()).unwrap();
    value["explorer_baseline"] = serde_json::json!({ "pid": "x", "gdi": 1, "recorded_at": 1 });
    fs::write(&h.paths.state_file, value.to_string()).unwrap();
    assert!(h.load().blocked().is_some());
}

// ---------------------------------------------------------------------------------------------
// task 6.4 最終審查（R1）：還原安全
// ---------------------------------------------------------------------------------------------

/// 原桌布是投影片的接管：兩台都確認套用過（`host_applied`）。
fn take_over_slideshow(h: &mut Harness) -> (WallpaperTakeover, SlideshowInfo) {
    let album = h.pictures.join("Album").to_string_lossy().into_owned();
    let info = SlideshowInfo {
        items: vec![album.clone()],
        options: SLIDESHOW_OPTION_SHUFFLE,
        tick_ms: 600_000,
    };
    let slide = format!("{album}\\slide-3.jpg");
    let shown = info.clone();
    // 投影片目前那張不存在（原圖複製不了）：走逐螢幕轉存檔的備份（修正輪 4：登錄＝TranscodedWallpaper）。
    h.per_monitor_setting();
    h.with_desk(move |d| {
        d.slideshow_status = SLIDESHOW_STATE_ENABLED | SLIDESHOW_STATE_SLIDESHOW;
        d.slideshow = Some(shown);
        for m in &mut d.monitors {
            m.wallpaper = Some(slide.clone());
        }
    });
    let mut t = take_over_both(h);
    for (dev, name) in [(DEV_A, "a-1.png"), (DEV_B, "b-1.png")] {
        let out = h.out(name).to_string_lossy().into_owned();
        assert!(h.confirm(&mut t, dev, &out), "前提：{dev} 確認套用");
    }
    (t, info)
}

/// 模擬「還原進行中」標記已寫入、`restore_slideshow` 已生效（explorer 正顯示投影片的某一張，不是
/// 記錄當下那張），之後還原中斷（逾時、被終止、驗證失敗）。
fn interrupt_after_slideshow_restored(h: &Harness, info: &SlideshowInfo) {
    let mut st = h.state_on_disk();
    st.restore_in_progress = Some(RestoreMarker::new(&RestoreReason::TrayExit, h.now));
    super::write_state_json(&h.paths.state_file, &st).unwrap();
    let info = info.clone();
    h.with_desk(move |d| {
        d.slideshow_status |= SLIDESHOW_STATE_SLIDESHOW;
        d.slideshow = Some(info.clone());
        for m in &mut d.monitors {
            m.wallpaper = Some(format!("{}\\slide-1.jpg", info.items[0]));
        }
    });
}

/// R1-M1：投影片還原中斷後續做，不得把每台都誤判為「使用者自選」而略過全域設定；填滿方式回到
/// 原值 3、背景色與三個登錄值寫回，備份一個都不刪。
#[test]
fn slideshow_restore_resumed_after_interruption_restores_globals() {
    let mut h = Harness::new("r64-slideshow-resume");
    let (t, info) = take_over_slideshow(&mut h);
    drop(t);
    interrupt_after_slideshow_restored(&h, &info);
    let backups_before = h.backups();
    assert!(!backups_before.is_empty(), "前提：接管時有備份");

    let mut t = h.load();
    assert!(t.restore_marker().is_some(), "前提：續做");
    let out = h.restore(&mut t, RestoreReason::TrayExit);
    let (kept, saved) = restored_parts(&out);
    assert!(
        kept.is_empty(),
        "投影片還原途中可解釋的讀回不是使用者自選：{out:?}"
    );
    assert!(saved);
    let d = h.desk();
    assert_eq!(d.position, ORIGINAL_POSITION, "填滿方式回到原值 3");
    assert_eq!(d.color, ORIGINAL_COLOR);
    assert_eq!(d.slideshow, Some(info));
    assert_eq!(
        h.current_registry(),
        h.original_registry(),
        "三個登錄值寫回"
    );
    assert_eq!(h.backups(), backups_before, "原圖備份不刪");
    assert_eq!(h.state_on_disk().status, TakeoverStatus::NotTakenOver);
}

/// R1-M1 的對照：沒有「還原進行中」標記（不是續做）時，使用者自己從投影片資料夾挑了一張單張圖
/// 仍是使用者的選擇，不覆蓋、不寫登錄。
#[test]
fn slideshow_folder_picture_without_resume_marker_is_still_user_choice() {
    let mut h = Harness::new("r64-slideshow-no-marker");
    let (mut t, info) = take_over_slideshow(&mut h);
    let picked = format!("{}\\slide-7.jpg", info.items[0]);
    let shown = picked.clone();
    h.with_desk(move |d| {
        d.monitors[0].wallpaper = Some(shown);
    });
    let out = h.restore(&mut t, RestoreReason::TrayExit);
    let (kept, _) = restored_parts(&out);
    assert_eq!(kept, vec![DEV_A.to_owned()], "{out:?}");
    assert_eq!(h.desk().wallpaper(DEV_A), Some(picked));
    assert_ne!(
        h.current_registry(),
        h.original_registry(),
        "使用者已自選：不寫登錄"
    );
}

/// R1-M2：接管期間接上、沒有紀錄、卻正顯示宿主圖的螢幕，還原時也要還原（全域原桌布＝登錄原值），
/// 並在結果留一行說明，不得回報成乾淨的 Restored 而忽略它。
#[test]
fn unrecorded_monitor_showing_host_image_is_restored_on_restore() {
    let mut h = Harness::new("r64-unrecorded-restore");
    let mut t = take_over_both(&mut h);
    let host = h.out("a-1.png").to_string_lossy().into_owned();
    add_monitor(&h, DEV_C, 7680, &host);

    let out = h.restore(&mut t, RestoreReason::TrayExit);
    let RestoreResult::Restored { warnings, .. } = &out.result else {
        panic!("應為 Restored：{out:?}");
    };
    assert!(
        warnings.iter().any(|w| w.contains(DEV_C)),
        "沒有紀錄的那台要記一行：{warnings:?}"
    );
    let c = h.desk().wallpaper(DEV_C).expect("C 在線");
    assert!(path_eq(&c, &h.sunset()), "還原為全域原桌布：{c}");
    assert_desk_restored(&h);
}

/// R1-M2：讓位時同樣列舉所有在線螢幕，沒有紀錄但顯示宿主圖者逐螢幕還原（不寫登錄）。
#[test]
fn unrecorded_monitor_showing_host_image_is_restored_on_yield() {
    let mut h = Harness::new("r64-unrecorded-yield");
    let mut t = take_over_both(&mut h);
    let host = h.out("b-1.png").to_string_lossy().into_owned();
    add_monitor(&h, DEV_C, 7680, &host);
    photo_on(&h, &[0]);
    assert!(matches!(
        h.set(&mut t, DEV_B, &h.out("b-2.png")),
        SetOutcome::Yielded { .. }
    ));
    let c = h.desk().wallpaper(DEV_C).expect("C 在線");
    assert!(path_eq(&c, &h.sunset()), "還原為全域原桌布：{c}");
    assert_eq!(h.desk().wallpaper(DEV_B), Some(h.sunset()));
}

/// R1-M3a＋task 6.1 修正（Bug 2／B3）＋修正輪 4：原圖在接管前已不存在（explorer 仍以快取顯示），接管前是逐螢幕
/// 設定（登錄 `Wallpaper`＝Themes 的 `TranscodedWallpaper`）——每台改備份**自己的**轉存檔 `Transcoded_<索引>`
/// （索引＝列舉順序），還原時用它。
#[test]
fn missing_original_backs_up_each_monitors_own_transcoded() {
    let mut h = Harness::new("r64-transcoded-backup");
    h.per_monitor_setting();
    fs::remove_file(h.sunset()).unwrap();
    let mut t = take_over_both(&mut h);
    let st = h.state_on_disk();
    for (dev, x, i) in [(DEV_A, 0, 0), (DEV_B, 3840, 1)] {
        let b = st.monitors[&key(dev, x)].backup.clone().expect("有備份");
        assert_eq!(
            fs::read(h.paths.backup_dir().join(&b)).unwrap(),
            monitor_transcoded_bytes(i),
            "{dev} 備份的是自己的轉存檔"
        );
    }
    // explorer 在 SetWallpaper 之後改寫轉存檔：原圖內容只剩備份。
    for i in 0..2 {
        fs::write(h.paths.monitor_transcoded(i), b"HOST-IMAGE").unwrap();
    }
    restored_parts(&h.restore(&mut t, RestoreReason::NotTakeover));
    for (dev, i) in [(DEV_A, 0), (DEV_B, 1)] {
        let shown = h.desk().wallpaper(dev).expect("在線");
        assert_eq!(
            fs::read(&shown).unwrap(),
            monitor_transcoded_bytes(i),
            "{shown}"
        );
    }
}

/// 修正輪 3（6.1 重跑：轉存檔格式沿用原圖）：JPEG 的逐螢幕轉存檔照樣能當備份，副檔名依檔頭為 `.jpg`。
#[test]
fn own_transcoded_jpeg_is_backed_up() {
    let mut h = Harness::new("r61d-jpeg");
    h.per_monitor_setting();
    fs::remove_file(h.sunset()).unwrap();
    drop(take_over_both(&mut h));
    let b = h.state_on_disk().monitors[&key(DEV_A, 0)]
        .backup
        .clone()
        .expect("有備份");
    assert_eq!(b, format!("{}.jpg", key(DEV_A, 0)));
    assert_eq!(
        fs::read(h.paths.backup_dir().join(&b)).unwrap(),
        monitor_transcoded_bytes(0)
    );
}

/// 修正輪 3（6.1 重跑：轉存檔尺寸沿用原圖）：尺寸不等於螢幕解析度（3840×2400 對 3840×2160）照樣能當備份。
#[test]
fn own_transcoded_not_screen_size_is_backed_up() {
    let mut h = Harness::new("r61d-size");
    h.per_monitor_setting();
    fs::remove_file(h.sunset()).unwrap();
    drop(take_over_both(&mut h));
    let b = h.state_on_disk().monitors[&key(DEV_B, 3840)]
        .backup
        .clone()
        .expect("有備份");
    assert_eq!(b, format!("{}.png", key(DEV_B, 3840)));
    assert_eq!(
        fs::read(h.paths.backup_dir().join(&b)).unwrap(),
        monitor_transcoded_bytes(1)
    );
}

/// 修正輪 3（6.1 重跑：「全部螢幕」設定 `SetWallpaper(NULL, 圖)` 不產生逐螢幕轉存檔）＋修正輪 4：登錄 `Wallpaper`
/// ＝各台讀回的共同路徑——全域 `TranscodedWallpaper` 代表所有螢幕，多螢幕也用它。
#[test]
fn all_monitor_setting_backs_up_global_transcoded() {
    let mut h = Harness::new("r61d-all-monitors");
    fs::remove_file(h.sunset()).unwrap();
    for i in 0..2 {
        fs::remove_file(h.paths.monitor_transcoded(i)).unwrap();
    }
    let mut t = take_over_both(&mut h);
    let st = h.state_on_disk();
    for (dev, x) in [(DEV_A, 0), (DEV_B, 3840)] {
        let b = st.monitors[&key(dev, x)].backup.clone().expect("有備份");
        assert_eq!(b, format!("{}.jpg", key(dev, x)), "依檔頭判斷是 JPEG");
        assert_eq!(
            fs::read(h.paths.backup_dir().join(&b)).unwrap(),
            TRANSCODED_BYTES,
            "{dev} 備份的是全域轉存檔"
        );
    }
    fs::write(h.paths.transcoded_wallpaper(), b"HOST-IMAGE").unwrap();
    restored_parts(&h.restore(&mut t, RestoreReason::NotTakeover));
    for dev in [DEV_A, DEV_B] {
        let shown = h.desk().wallpaper(dev).expect("在線");
        assert_eq!(fs::read(&shown).unwrap(), TRANSCODED_BYTES, "{shown}");
    }
}

/// 修正輪 4（複審 N2）：接管前是「全部螢幕」設定（登錄 `Wallpaper`＝各台讀回的共同路徑，不分大小寫），但 Themes
/// 裡**殘留**先前逐螢幕設定留下的 `Transcoded_<索引>`——那是舊圖，不代表目前的桌布；一律用全域
/// `TranscodedWallpaper`，還原內容也是它。
#[test]
fn all_monitor_setting_ignores_leftover_own_transcoded() {
    let mut h = Harness::new("r61e-all-leftover");
    fs::remove_file(h.sunset()).unwrap();
    assert!(
        (0..2).all(|i| h.paths.monitor_transcoded(i).exists()),
        "前提：逐螢幕轉存檔殘留"
    );
    let mut t = take_over_both(&mut h);
    let st = h.state_on_disk();
    for (dev, x) in [(DEV_A, 0), (DEV_B, 3840)] {
        let b = st.monitors[&key(dev, x)].backup.clone().expect("有備份");
        assert_eq!(
            fs::read(h.paths.backup_dir().join(&b)).unwrap(),
            TRANSCODED_BYTES,
            "{dev} 備份的是全域轉存檔，不是殘留的逐螢幕檔"
        );
    }
    fs::write(h.paths.transcoded_wallpaper(), b"HOST-IMAGE").unwrap();
    restored_parts(&h.restore(&mut t, RestoreReason::NotTakeover));
    for dev in [DEV_A, DEV_B] {
        let shown = h.desk().wallpaper(dev).expect("在線");
        assert_eq!(fs::read(&shown).unwrap(), TRANSCODED_BYTES, "{shown}");
    }
}

/// 修正輪 4（複審 N2）：登錄 `Wallpaper` 是 `REG_EXPAND_SZ`、含環境變數時先展開再比對共同讀回路徑。環境變數取
/// 本機現有、值是暫存資料夾前綴的那一個（都沒有就略過這個案例）。
#[test]
fn all_monitor_setting_expands_registry_env_vars() {
    let mut h = Harness::new("r61e-all-envvar");
    let sunset = h.sunset();
    let Some((name, value)) = ["TMP", "TEMP", "LOCALAPPDATA", "USERPROFILE"]
        .iter()
        .filter_map(|n| env::var(n).ok().map(|v| (*n, v)))
        .find(|(_, v)| {
            !v.is_empty()
                && sunset
                    .get(..v.len())
                    .is_some_and(|head| head.eq_ignore_ascii_case(v))
        })
    else {
        return;
    };
    let with_var = format!("%{name}%{}", &sunset[value.len()..]);
    h.registry.put(
        DESKTOP_KEY,
        "Wallpaper",
        RegValue::string(REG_TYPE_EXPAND_SZ, &with_var),
    );
    fs::remove_file(&sunset).unwrap();
    let mut t = h.load();
    assert_eq!(
        h.set(&mut t, DEV_A, &h.out("a-1.png")),
        SetOutcome::Applied,
        "{with_var}"
    );
    let b = h.state_on_disk().monitors[&key(DEV_A, 0)]
        .backup
        .clone()
        .expect("有備份");
    assert_eq!(
        fs::read(h.paths.backup_dir().join(&b)).unwrap(),
        TRANSCODED_BYTES,
        "展開後＝共同讀回路徑：全部螢幕設定，用全域檔"
    );
}

/// 修正輪 4（複審 N2）：逐螢幕設定（登錄＝Themes 的 `TranscodedWallpaper`），即使各台讀回剛好是同一張圖，也只用
/// 各自的 `Transcoded_<索引>`；兩台的逐螢幕檔都不見時不能退回全域檔，改「不接管」。
#[test]
fn per_monitor_setting_without_own_transcoded_refuses_backup() {
    let mut h = Harness::new("r61e-per-monitor-missing");
    h.per_monitor_setting();
    fs::remove_file(h.sunset()).unwrap();
    for i in 0..2 {
        fs::remove_file(h.paths.monitor_transcoded(i)).unwrap();
    }
    assert!(h.paths.transcoded_wallpaper().exists(), "全域轉存檔還在");
    let mut t = h.load();
    let outcome = h.set(&mut t, DEV_A, &h.out("a-1.png"));
    assert!(
        matches!(outcome, SetOutcome::BackupFailed { .. }),
        "{outcome:?}"
    );
    assert!(h.set_calls().is_empty(), "{:?}", h.set_calls());
    assert!(!h.paths.state_file.exists(), "沒有接管就不寫狀態檔");
}

/// 修正輪 4（複審 N2）：判定設定方式的登錄值以檔案識別比對 Themes 的 `TranscodedWallpaper`——字串不同（例如經
/// junction 指到同一個 Themes 資料夾的隔離環境）但識別相同，仍是逐螢幕設定，用各自的 `Transcoded_<索引>`。
#[test]
fn per_monitor_setting_recognized_by_file_identity() {
    let mut h = Harness::new("r61e-per-monitor-identity");
    let alias = h
        .root
        .join("junction")
        .join("Themes")
        .join("TranscodedWallpaper")
        .to_string_lossy()
        .into_owned();
    h.files.add(&alias, 4242);
    let real = h
        .paths
        .transcoded_wallpaper()
        .to_string_lossy()
        .into_owned();
    h.files.add(&real, 4242);
    h.registry.put(
        DESKTOP_KEY,
        "Wallpaper",
        RegValue::string(REG_TYPE_SZ, &alias),
    );
    fs::remove_file(h.sunset()).unwrap();
    let mut t = h.load();
    assert_eq!(h.set(&mut t, DEV_A, &h.out("a-1.png")), SetOutcome::Applied);
    let b = h.state_on_disk().monitors[&key(DEV_A, 0)]
        .backup
        .clone()
        .expect("有備份");
    assert_eq!(
        fs::read(h.paths.backup_dir().join(&b)).unwrap(),
        monitor_transcoded_bytes(0)
    );
}

/// 修正輪 4（複審 N2）：登錄 `Wallpaper` 判斷不出設定方式（不存在、空字串、不是字串、與讀回不符又不是這個
/// Themes 的轉存檔）——即使逐螢幕檔與全域檔都在，也不知道哪個代表目前的桌布：不接管＋通知。
#[test]
fn unexpected_registry_wallpaper_refuses_transcoded_backup() {
    let other = "D:\\Pictures\\other.jpg";
    for (name, value) in [
        ("missing", RegValue::Missing),
        ("empty", RegValue::string(REG_TYPE_SZ, "")),
        (
            "dword",
            RegValue::Present {
                kind: 4,
                data: vec![1, 0, 0, 0],
            },
        ),
        ("other-path", RegValue::string(REG_TYPE_SZ, other)),
        (
            "elsewhere-transcoded",
            RegValue::string(REG_TYPE_SZ, TRANSCODED),
        ),
    ] {
        let mut h = Harness::new(&format!("r61e-reg-{name}"));
        h.registry.put(DESKTOP_KEY, "Wallpaper", value);
        fs::remove_file(h.sunset()).unwrap();
        let mut t = h.load();
        let outcome = h.set(&mut t, DEV_A, &h.out("a-1.png"));
        assert!(
            matches!(outcome, SetOutcome::BackupFailed { .. }),
            "{name}：{outcome:?}"
        );
        assert!(h.set_calls().is_empty(), "{name}：{:?}", h.set_calls());
    }
}

/// task 6.1 修正（Bug 2）：轉存檔編號是列舉索引而不是螢幕本身——列舉順序對調時，備份跟著索引走，絕不備到
/// 別台的轉存檔。
#[test]
fn transcoded_backup_follows_enumeration_index() {
    let mut h = Harness::new("r61-transcoded-swapped");
    h.per_monitor_setting();
    fs::remove_file(h.sunset()).unwrap();
    h.with_desk(|d| d.monitors.reverse());
    drop(take_over_both(&mut h));
    let st = h.state_on_disk();
    for (dev, x, i) in [(DEV_B, 3840, 0), (DEV_A, 0, 1)] {
        let b = st.monitors[&key(dev, x)].backup.clone().expect("有備份");
        assert_eq!(
            fs::read(h.paths.backup_dir().join(&b)).unwrap(),
            monitor_transcoded_bytes(i),
            "{dev}（索引 {i}）"
        );
    }
}

/// task 6.1 修正（Bug 2／B3 回歸）＋修正輪 4：逐螢幕設定，有些在線螢幕有逐螢幕轉存檔、有些沒有——全域
/// `TranscodedWallpaper` 不代表缺檔那台，不得拿它代替（即使它是有效的圖片、讀回路徑相同），改「不接管」。
#[test]
fn mixed_transcoded_presence_refuses_global_backup() {
    let mut h = Harness::new("r61-no-global-multi");
    h.per_monitor_setting();
    fs::remove_file(h.sunset()).unwrap();
    fs::remove_file(h.paths.monitor_transcoded(1)).unwrap();
    assert!(h.paths.transcoded_wallpaper().exists(), "全域轉存檔還在");
    let mut t = h.load();
    let outcome = h.set(&mut t, DEV_A, &h.out("a-1.png"));
    assert!(
        matches!(outcome, SetOutcome::BackupFailed { .. }),
        "{outcome:?}"
    );
    assert!(h.set_calls().is_empty(), "{:?}", h.set_calls());
    assert!(!h.paths.state_file.exists(), "沒有接管就不寫狀態檔");
}

/// 修正輪 3＋修正輪 4：各台讀回路徑不同（沒有共同路徑），登錄又不是 Themes 的 `TranscodedWallpaper`——判斷不出
/// 設定方式，全域檔與逐螢幕檔都不用，改「不接管」。
#[test]
fn different_readbacks_without_own_transcoded_refuse_backup() {
    let mut h = Harness::new("r61d-different");
    fs::remove_file(h.sunset()).unwrap();
    for i in 0..2 {
        fs::remove_file(h.paths.monitor_transcoded(i)).unwrap();
    }
    let gone = h.pictures.join("gone.jpg").to_string_lossy().into_owned();
    h.with_desk(move |d| d.monitors[1].wallpaper = Some(gone));
    let mut t = h.load();
    let outcome = h.set(&mut t, DEV_A, &h.out("a-1.png"));
    assert!(
        matches!(outcome, SetOutcome::BackupFailed { .. }),
        "{outcome:?}"
    );
    assert!(h.set_calls().is_empty(), "{:?}", h.set_calls());
}

/// 修正輪 3：轉存檔必須存在、大小 > 0，且依檔頭判斷得出是圖片（PNG／JPEG／BMP／GIF／WebP）；空檔或認不出的
/// 內容視為不能備份（不退回全域檔），改「不接管」。
#[test]
fn empty_or_unrecognized_transcoded_refuses_backup() {
    for (name, bytes) in [("empty", &b""[..]), ("garbage", &b"NOT-AN-IMAGE"[..])] {
        let mut h = Harness::new(&format!("r61d-bad-{name}"));
        h.per_monitor_setting();
        fs::remove_file(h.sunset()).unwrap();
        fs::write(h.paths.monitor_transcoded(0), bytes).unwrap();
        let mut t = h.load();
        let outcome = h.set(&mut t, DEV_A, &h.out("a-1.png"));
        assert!(
            matches!(outcome, SetOutcome::BackupFailed { .. }),
            "{name}：{outcome:?}"
        );
        assert!(h.set_calls().is_empty(), "{name}：{:?}", h.set_calls());
    }
}

/// 修正輪 1（審查 L1，M6）：索引算的是**全部**列舉到的螢幕（含離線者）——離線螢幕排在前面時，A 是索引 1、
/// 要備 `Transcoded_001`，不得錯位成 `_000`。
#[test]
fn transcoded_index_counts_offline_monitors() {
    let mut h = Harness::new("r61b-offline-first");
    h.per_monitor_setting();
    fs::remove_file(h.sunset()).unwrap();
    h.with_desk(|d| {
        d.monitors.truncate(1);
        d.monitors.insert(
            0,
            FakeMonitor {
                device_path: DEV_B.to_owned(),
                rect: None,
                wallpaper: None,
            },
        );
    });
    let mut t = h.load();
    assert_eq!(h.set(&mut t, DEV_A, &h.out("a-1.png")), SetOutcome::Applied);
    let b = h.state_on_disk().monitors[&key(DEV_A, 0)]
        .backup
        .clone()
        .expect("有備份");
    assert_eq!(
        fs::read(h.paths.backup_dir().join(&b)).unwrap(),
        monitor_transcoded_bytes(1),
        "A 在列舉索引 1"
    );
}

/// 只有一台螢幕時同一套規則（修正輪 4）：登錄＝讀回路徑（全部螢幕設定）→ 全域 `TranscodedWallpaper`（即使殘留
/// `Transcoded_000`；任何圖片格式都可以，認不出是圖片就不能備份）；登錄＝Themes 的 `TranscodedWallpaper`（逐螢幕
/// 設定）→ `Transcoded_000`，沒有就不能備份（不退回全域檔）。
#[test]
fn single_monitor_backup_source_follows_setting_method() {
    let own = b"\xFF\xD8\xFF\xE0OWN".to_vec();
    let global = png_bytes(1920, 1080, "GLOBAL");
    for (name, per_monitor, own_file, global_file, expect) in [
        (
            "all-own-leftover",
            false,
            Some(own.clone()),
            global.clone(),
            Some(global.clone()),
        ),
        (
            "all-no-own",
            false,
            None,
            global.clone(),
            Some(global.clone()),
        ),
        (
            "all-bmp",
            false,
            None,
            b"BMGLOBAL".to_vec(),
            Some(b"BMGLOBAL".to_vec()),
        ),
        ("all-garbage", false, None, b"????".to_vec(), None),
        (
            "per-own",
            true,
            Some(own.clone()),
            global.clone(),
            Some(own.clone()),
        ),
        ("per-no-own", true, None, global.clone(), None),
    ] {
        let mut h = Harness::new(&format!("r61-single-{name}"));
        if per_monitor {
            h.per_monitor_setting();
        }
        fs::remove_file(h.sunset()).unwrap();
        h.with_desk(|d| d.monitors.truncate(1));
        fs::remove_file(h.paths.monitor_transcoded(1)).unwrap();
        match &own_file {
            Some(bytes) => fs::write(h.paths.monitor_transcoded(0), bytes).unwrap(),
            None => fs::remove_file(h.paths.monitor_transcoded(0)).unwrap(),
        }
        fs::write(h.paths.transcoded_wallpaper(), &global_file).unwrap();
        let mut t = h.load();
        let outcome = h.set(&mut t, DEV_A, &h.out("a-1.png"));
        match &expect {
            Some(bytes) => {
                assert_eq!(outcome, SetOutcome::Applied, "{name}");
                let b = h.state_on_disk().monitors[&key(DEV_A, 0)]
                    .backup
                    .clone()
                    .expect("有備份");
                assert_eq!(
                    &fs::read(h.paths.backup_dir().join(&b)).unwrap(),
                    bytes,
                    "{name}"
                );
            }
            None => assert!(
                matches!(outcome, SetOutcome::BackupFailed { .. }),
                "{name}：{outcome:?}"
            ),
        }
    }
}

/// R1-M3a：原圖與轉存檔都備份不了 → 不接管：不設定任何桌布、不寫狀態檔，主題改「不接管」並通知。
#[test]
fn takeover_refused_when_neither_original_nor_cache_can_be_backed_up() {
    let mut h = Harness::new("r64-no-backup");
    fs::remove_file(h.sunset()).unwrap();
    fs::remove_file(h.paths.transcoded_wallpaper()).unwrap();
    for i in 0..2 {
        fs::remove_file(h.paths.monitor_transcoded(i)).unwrap();
    }
    let mut t = h.load();
    let outcome = h.set(&mut t, DEV_A, &h.out("a-1.png"));
    let dbg = format!("{outcome:?}");
    assert!(dbg.contains("BackupFailed"), "{dbg}");
    assert!(
        dbg.contains("ThemeSetToNone") && dbg.contains("Notify"),
        "主題改不接管並通知：{dbg}"
    );
    assert!(h.set_calls().is_empty(), "{:?}", h.set_calls());
    assert!(!t.already_taken_over());
    assert!(!h.paths.state_file.exists(), "沒有接管就不寫狀態檔");
}

/// R1-M3a：接管期間新接上的螢幕，原圖不存在時不能用轉存檔（已是宿主的圖）：不設定那台、停止接管。
#[test]
fn new_monitor_without_backupable_original_stops_takeover() {
    let mut h = Harness::new("r64-new-monitor-no-backup");
    let mut t = take_over_both(&mut h);
    let gone = h.pictures.join("gone.jpg").to_string_lossy().into_owned();
    add_monitor(&h, DEV_C, 7680, &gone);
    h.clear_calls();
    let outcome = h.set(&mut t, DEV_C, &h.out("c-1.png"));
    let dbg = format!("{outcome:?}");
    assert!(dbg.contains("BackupFailed"), "{dbg}");
    assert_eq!(h.desk().wallpaper(DEV_C), Some(gone), "C 不動");
    assert!(
        !h.set_calls().iter().any(|c| c.starts_with("set_wallpaper")),
        "{:?}",
        h.set_calls()
    );
}

/// R1-M3b：原路徑本身在 Themes 快取下（explorer 會改寫它）：還原時優先用備份。
#[test]
fn restore_prefers_backup_when_original_is_in_themes_cache() {
    let mut h = Harness::new("r64-cache-original");
    let cached = h.paths.themes_dir.join("CachedFiles");
    fs::create_dir_all(&cached).unwrap();
    let cached = cached.join("CachedImage_3840_2160_POS4.jpg");
    fs::write(&cached, ORIGINAL_BYTES).unwrap();
    let cached = cached.to_string_lossy().into_owned();
    let shown = cached.clone();
    h.with_desk(move |d| {
        for m in &mut d.monitors {
            m.wallpaper = Some(shown.clone());
        }
    });
    let mut t = take_over_both(&mut h);
    // explorer 把快取改成宿主的圖。
    fs::write(&cached, b"HOST-IMAGE").unwrap();
    restored_parts(&h.restore(&mut t, RestoreReason::NotTakeover));
    for dev in [DEV_A, DEV_B] {
        let now = h.desk().wallpaper(dev).expect("在線");
        assert!(
            path_is_within(&now, &h.paths.backup_dir()),
            "{dev} 應還原成備份：{now}"
        );
        assert_eq!(fs::read(&now).unwrap(), ORIGINAL_BYTES);
    }
}

/// R1-M4：讀回落在 Themes 快取（例如 explorer 重新啟動後）不判為使用者自換：不讓位、不改原桌布、
/// 不刪備份。
#[test]
fn readback_in_themes_cache_is_not_a_yield() {
    let mut h = Harness::new("r64-cache-readback");
    let mut t = take_over_both(&mut h);
    for (dev, name) in [(DEV_A, "a-1.png"), (DEV_B, "b-1.png")] {
        let out = h.out(name).to_string_lossy().into_owned();
        assert!(h.confirm(&mut t, dev, &out));
    }
    let backups = h.backups();
    let cache = h
        .paths
        .transcoded_wallpaper()
        .to_string_lossy()
        .into_owned();
    h.with_desk(move |d| d.monitors[0].wallpaper = Some(cache));
    let outcome = h.set(&mut t, DEV_B, &h.out("b-2.png"));
    assert_eq!(outcome, SetOutcome::Applied, "不讓位");
    assert!(t.already_taken_over());
    let st = h.state_on_disk();
    assert_eq!(st.monitors[&key(DEV_A, 0)].original_wallpaper, h.sunset());
    assert_eq!(h.backups(), backups, "備份不動");
}

/// R1-M4：讓位時被取代的舊備份保留到下一次接管開始才清（同一次寫入不得當孤兒刪掉）。
#[test]
fn yield_keeps_superseded_backup_until_next_takeover() {
    let mut h = Harness::new("r64-superseded");
    let mut t = take_over_both(&mut h);
    let a_backup = format!("{}.jpg", key(DEV_A, 0));
    // 使用者換成 PNG 照片：新備份的副檔名不同，看得出舊的 .jpg 備份有沒有被清掉。
    let photo = h.pictures.join("photo.png");
    fs::write(&photo, b"\x89PNG\r\n\x1a\nPHOTO").unwrap();
    let photo = photo.to_string_lossy().into_owned();
    h.with_desk(move |d| d.monitors[0].wallpaper = Some(photo));
    assert!(matches!(
        h.set(&mut t, DEV_A, &h.out("a-2.png")),
        SetOutcome::Yielded { .. }
    ));
    assert!(
        h.backups().contains(&a_backup),
        "讓位後舊備份仍在：{:?}",
        h.backups()
    );
    assert_eq!(h.state_on_disk().superseded_backups, vec![a_backup.clone()]);
    // 使用者重新開啟接管：下一次接管開始時才清掉（清單清空、沒人用的舊檔刪除）。
    t.theme_selected(WallpaperTheme::Astrolabe);
    assert_eq!(h.set(&mut t, DEV_A, &h.out("a-3.png")), SetOutcome::Applied);
    let st = h.state_on_disk();
    assert!(
        st.superseded_backups.is_empty(),
        "接管開始時清空：{:?}",
        st.superseded_backups
    );
    assert!(
        !h.backups().contains(&a_backup),
        "被取代的舊備份在接管開始後清掉：{:?}",
        h.backups()
    );
    let new_backup = st.monitors[&key(DEV_A, 0)].backup.clone().expect("新備份");
    assert_eq!(new_backup, format!("{}.png", key(DEV_A, 0)));
    assert_eq!(
        fs::read(h.paths.backup_dir().join(&new_backup)).unwrap(),
        b"\x89PNG\r\n\x1a\nPHOTO",
        "新的原桌布是使用者的照片"
    );
}

/// 修正第 2 輪（複審 N1）：接管中使用者套用 Store／`.themepack` 佈景主題（桌布解壓到
/// `%LOCALAPPDATA%\Microsoft\Windows\Themes\<名稱>\DesktopBackground\`）是使用者的選擇：照常讓位，不覆蓋。
#[test]
fn theme_pack_applied_mid_takeover_yields() {
    let mut h = Harness::new("r64b-theme-pack");
    let mut t = take_over_both(&mut h);
    for (dev, name) in [(DEV_A, "a-1.png"), (DEV_B, "b-1.png")] {
        let out = h.out(name).to_string_lossy().into_owned();
        assert!(h.confirm(&mut t, dev, &out));
    }
    let pack = h
        .root
        .join("Local")
        .join("Microsoft")
        .join("Windows")
        .join("Themes")
        .join("Nature")
        .join("DesktopBackground");
    fs::create_dir_all(&pack).unwrap();
    let img = pack.join("img1.jpg");
    fs::write(&img, b"NATURE").unwrap();
    let img = img.to_string_lossy().into_owned();
    let shown = img.clone();
    h.with_desk(move |d| d.monitors[0].wallpaper = Some(shown));
    assert!(matches!(
        h.set(&mut t, DEV_B, &h.out("b-2.png")),
        SetOutcome::Yielded { .. }
    ));
    assert_eq!(
        h.desk().wallpaper(DEV_A),
        Some(img.clone()),
        "不覆蓋使用者的佈景主題"
    );
    assert_eq!(
        h.state_on_disk().monitors[&key(DEV_A, 0)].original_wallpaper,
        img
    );
}

/// 修正第 2 輪（複審 N1 對照）：讀回是 Roaming `Themes\CachedFiles\` 內的 explorer 快取檔：不讓位。
#[test]
fn readback_in_roaming_cached_files_is_not_a_yield() {
    let mut h = Harness::new("r64b-cachedfiles");
    let mut t = take_over_both(&mut h);
    for (dev, name) in [(DEV_A, "a-1.png"), (DEV_B, "b-1.png")] {
        let out = h.out(name).to_string_lossy().into_owned();
        assert!(h.confirm(&mut t, dev, &out));
    }
    let cached = h
        .paths
        .themes_dir
        .join("CachedFiles")
        .join("CachedImage_3840_2160_POS4.jpg")
        .to_string_lossy()
        .into_owned();
    h.with_desk(move |d| d.monitors[0].wallpaper = Some(cached));
    assert_eq!(h.set(&mut t, DEV_B, &h.out("b-2.png")), SetOutcome::Applied);
    assert!(t.already_taken_over());
}

/// 修正第 2 輪（複審 N2）：接管中新接上的螢幕讀回本身就是轉存檔（內容已是宿主的圖）：不得把它複製成
/// 原圖備份——記為原桌布未知、不備份，還原時走原桌布未知的後備（這裡＝已知螢幕共同的原桌布）。
#[test]
fn new_monitor_reading_back_transcoded_is_recorded_as_unknown() {
    let mut h = Harness::new("r64b-new-transcoded");
    let mut t = take_over_both(&mut h);
    fs::write(h.paths.transcoded_wallpaper(), b"HOST-IMAGE").unwrap();
    let cache = h
        .paths
        .transcoded_wallpaper()
        .to_string_lossy()
        .into_owned();
    add_monitor(&h, DEV_C, 7680, &cache);
    let backups_before = h.backups();
    assert_eq!(h.set(&mut t, DEV_C, &h.out("c-1.png")), SetOutcome::Applied);
    let rec = h.state_on_disk().monitors[&key(DEV_C, 7680)].clone();
    assert!(rec.original_unknown, "{rec:?}");
    assert_eq!(rec.backup, None);
    assert_eq!(rec.original_wallpaper, "");
    assert_eq!(h.backups(), backups_before, "轉存檔不得被當成原圖備份");

    restored_parts(&h.restore(&mut t, RestoreReason::NotTakeover));
    let c = h.desk().wallpaper(DEV_C).expect("C 在線");
    assert!(path_eq(&c, &h.sunset()), "還原為原桌布：{c}");
}

/// 修正第 2 輪（複審 N2 附帶）：原圖沒有圖片副檔名（例如接管開始時讀回就是轉存檔）時，備份的副檔名依內容
/// 判斷（PNG／JPEG 簽章），判斷不出來用 `.jpg`——不再產生 `.bin`。
#[test]
fn backup_extension_follows_content_when_source_has_no_image_extension() {
    for (name, bytes, ext) in [
        ("png", &b"\x89PNG\r\n\x1a\nDATA"[..], "png"),
        ("jpeg", &b"\xFF\xD8\xFF\xE0DATA"[..], "jpg"),
        ("unknown", &b"????"[..], "jpg"),
    ] {
        let mut h = Harness::new(&format!("r64b-ext-{name}"));
        let src = h.pictures.join("wallpaper-cache");
        fs::write(&src, bytes).unwrap();
        let src = src.to_string_lossy().into_owned();
        h.with_desk(move |d| {
            for m in &mut d.monitors {
                m.wallpaper = Some(src.clone());
            }
        });
        drop(take_over_both(&mut h));
        let st = h.state_on_disk();
        let backup = st.monitors[&key(DEV_A, 0)].backup.clone().expect("有備份");
        assert_eq!(backup, format!("{}.{ext}", key(DEV_A, 0)), "{name}");
        assert_eq!(fs::read(h.paths.backup_dir().join(&backup)).unwrap(), bytes);
    }
}

/// 修正第 2 輪（複審 L2）：還原時記錄中的螢幕讀回落在桌布快取（內容可能是宿主的圖）——照原紀錄還原，
/// 不算使用者自選（全域設定與登錄照常寫回）。
#[test]
fn restore_treats_cache_readback_as_host_image() {
    let mut h = Harness::new("r64b-restore-cache");
    let mut t = take_over_both(&mut h);
    for (dev, name) in [(DEV_A, "a-1.png"), (DEV_B, "b-1.png")] {
        let out = h.out(name).to_string_lossy().into_owned();
        assert!(h.confirm(&mut t, dev, &out));
    }
    let cache = h
        .paths
        .transcoded_wallpaper()
        .to_string_lossy()
        .into_owned();
    h.with_desk(move |d| d.monitors[0].wallpaper = Some(cache));
    let (kept, _) = restored_parts(&h.restore(&mut t, RestoreReason::NotTakeover));
    assert!(kept.is_empty(), "快取讀回不是使用者自選：{kept:?}");
    assert_desk_restored(&h);
}

/// 修正第 2 輪（複審 L2）：讓位時，其他記錄中螢幕的讀回落在桌布快取——照原紀錄逐螢幕還原（不是只清標記
/// 留著快取裡的宿主圖）。
#[test]
fn yield_restores_other_monitor_reading_back_cache() {
    let mut h = Harness::new("r64b-yield-cache");
    let mut t = take_over_both(&mut h);
    for (dev, name) in [(DEV_A, "a-1.png"), (DEV_B, "b-1.png")] {
        let out = h.out(name).to_string_lossy().into_owned();
        assert!(h.confirm(&mut t, dev, &out));
    }
    let cache = h
        .paths
        .transcoded_wallpaper()
        .to_string_lossy()
        .into_owned();
    h.with_desk(move |d| d.monitors[1].wallpaper = Some(cache));
    photo_on(&h, &[0]);
    assert!(matches!(
        h.set(&mut t, DEV_A, &h.out("a-2.png")),
        SetOutcome::Yielded { .. }
    ));
    assert_eq!(
        h.desk().wallpaper(DEV_B),
        Some(h.sunset()),
        "B 還原為原桌布"
    );
}

/// 修正第 2 輪（複審 nit）：「續做」旗標要有鑑別力——不是續做（記憶體中沒有標記）時，使用者自己重新開啟
/// 原本的投影片仍是使用者的選擇：不寫登錄、不動全域設定。
#[test]
fn slideshow_reenabled_by_user_without_resume_is_user_choice() {
    let mut h = Harness::new("r64b-slideshow-user");
    let (mut t, info) = take_over_slideshow(&mut h);
    let shown = info.clone();
    h.with_desk(move |d| {
        d.slideshow_status |= SLIDESHOW_STATE_SLIDESHOW;
        d.slideshow = Some(shown.clone());
        for m in &mut d.monitors {
            m.wallpaper = Some(format!("{}\\slide-1.jpg", shown.items[0]));
        }
    });
    let (kept, _) = restored_parts(&h.restore(&mut t, RestoreReason::TrayExit));
    assert_eq!(kept.len(), 2, "{kept:?}");
    assert_ne!(
        h.current_registry(),
        h.original_registry(),
        "使用者已自選：不寫登錄"
    );
    assert_ne!(h.desk().position, ORIGINAL_POSITION, "不動全域填滿方式");
}

/// 修正第 2 輪（複審 nit）：還原時無紀錄螢幕補上的紀錄，不得影響原桌布未知那台的後備判定（與讓位一致：
/// 先算各紀錄要還原成什麼，再補紀錄）。登錄原值（reg.jpg）與已知螢幕的原桌布（sunset）不同時，原桌布未知
/// 的那台仍用已知螢幕的原桌布。
#[test]
fn unrecorded_monitor_does_not_change_unknown_fallback_on_restore() {
    let mut h = Harness::new("r64b-order");
    let reg = h.pictures.join("reg.jpg");
    fs::write(&reg, b"REG").unwrap();
    let reg = reg.to_string_lossy().into_owned();
    h.registry.put(
        DESKTOP_KEY,
        "Wallpaper",
        RegValue::string(REG_TYPE_SZ, &reg),
    );
    let (known, unknown) = (0, 1);
    let mut t = take_over_with_unknown(&mut h, known, unknown);
    let host = h.out("k-1.png").to_string_lossy().into_owned();
    add_monitor(&h, DEV_C, 7680, &host);
    restored_parts(&h.restore(&mut t, RestoreReason::NotTakeover));
    let b = h.desk().wallpaper(DEVS[unknown]).expect("在線");
    assert_eq!(
        fs::read(&b).unwrap(),
        ORIGINAL_BYTES,
        "原桌布未知那台：已知螢幕的原桌布（{b}）"
    );
    let c = h.desk().wallpaper(DEV_C).expect("C 在線");
    assert!(path_eq(&c, &reg), "無紀錄那台：登錄原值（{c}）");
}

/// 修正輪 2：只有「確定沒有生效」的設定錯誤才退回 `last_set`；逾時與處理中途中止可能已送到 explorer，不退。
#[test]
fn only_definite_set_failures_roll_back() {
    use crate::desktop::wallpaper::RequestKind;
    let r = RequestKind::SetWallpaper;
    for (e, expect) in [
        (
            WallpaperError::Backend {
                request: r,
                error: BackendError::Com("x".to_owned()),
            },
            true,
        ),
        (WallpaperError::Busy { request: r }, true),
        (WallpaperError::WrongThread { request: r }, true),
        (WallpaperError::WorkerGone { request: r }, true),
        (
            WallpaperError::Timeout {
                request: r,
                timeout: Duration::from_secs(10),
            },
            false,
        ),
        (WallpaperError::Aborted { request: r }, false),
    ] {
        assert_eq!(set_definitely_not_applied(&e), expect, "{e:?}");
    }
}
