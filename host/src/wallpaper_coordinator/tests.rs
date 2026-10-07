//! 桌布協調迴圈的測試（task 4.7a）。一律用假的桌布後端（經真正的 `WallpaperService` 工作執行緒）、
//! 假渲染器、假時鐘、記憶體內的假登錄與暫存資料夾：**不**碰真正的 `%APPDATA%`／`%LOCALAPPDATA%`、
//! HKCU，也不改桌布。

use super::*;

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use log::Level;

use crate::desktop::wallpaper::file_identity::NoFileIdentity;
use crate::desktop::wallpaper::registry::{RegValue, RegistryError, RegistryStore};
use crate::desktop::wallpaper::{
    assemble_monitor_list, BackendError, MonitorEntry, MonitorProbe, MonitorWallpaper,
    SlideshowInfo, WallpaperBackend, WallpaperPosition, WallpaperService, WallpaperServiceConfig,
    WallpaperSnapshot,
};
use crate::layout::PhysicalRect;
use crate::settings::WallpaperTheme;
use crate::wallpaper::{
    CivilDate, DataDates, DataKey, ExplorerSample, UnixSeconds, UtcOffsetSource,
};
use crate::wallpaper_render::{
    write_output, BodyKind, OutputTarget, RenderFailure, RenderFailureReason, RenderRequest,
    RenderSuccess, RenderTimings,
};
use crate::wallpaper_state::{
    monitor_key, StableDisplay, StatePaths, TakeoverConfig, TakeoverNotice, ThemeNoneCause,
    STATE_FILE_NAME,
};
use crate::widgets::PauseReason;

const DEV_A: &str = "\\\\?\\DISPLAY#FAKEA01#1&aaa&0&UID1#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";
const DEV_B: &str = "\\\\?\\DISPLAY#FAKEB02#1&bbb&0&UID2#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";
const DEV_C: &str = "\\\\?\\DISPLAY#FAKEC03#1&ccc&0&UID3#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";
const TAIPEI: i32 = 8 * 3600;
/// 假 explorer 的 PID（4.9 安全閥）。
const EXPLORER_PID: u32 = 4242;

fn rect(x: i32, w: i32, h: i32) -> PhysicalRect {
    PhysicalRect {
        x,
        y: 0,
        width: w,
        height: h,
    }
}

fn rect_a() -> PhysicalRect {
    rect(0, 1920, 1080)
}
fn rect_b() -> PhysicalRect {
    rect(1920, 2560, 1440)
}
fn rect_c() -> PhysicalRect {
    rect(4480, 1280, 1024)
}

/// 2026-10-05（週一）台北 `hh:mm:ss` 的 UNIX 秒。
fn taipei(hh: i64, mm: i64, ss: i64) -> UnixSeconds {
    let day = CivilDate {
        year: 2026,
        month: 10,
        day: 5,
    }
    .to_days();
    day * 86_400 + hh * 3600 + mm * 60 + ss - i64::from(TAIPEI)
}

// ---------------------------------------------------------------------------------------------
// 假的 explorer（桌布後端）
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct FakeMonitor {
    device_path: String,
    rect: Option<PhysicalRect>,
    wallpaper: Option<String>,
}

#[derive(Debug, Default)]
struct DeskState {
    monitors: Vec<FakeMonitor>,
    position: i32,
    calls: Vec<String>,
    /// 這些螢幕的 `SetWallpaper` 回 COM 錯誤。
    fail_set: HashSet<String>,
    /// `SetWallpaper` 卡住直到 [`Gate`] 放行（模擬 explorer 無回應 → 逾時 → 忙碌）。
    hang_set: bool,
    /// `read` 回 COM 錯誤（還原前讀取失敗）。
    fail_read: bool,
    /// `list_monitors` 回 COM 錯誤。
    fail_list: bool,
    /// `list_monitors` 卡住直到 [`Gate`] 放行（列舉逾時 → 忙碌）。
    hang_list: bool,
    /// `read`（設定前讀回）回報這些螢幕離線，`list_monitors` 不受影響（列舉之後才拔掉）。
    read_offline: HashSet<String>,
    /// 這些螢幕的 `GetMonitorRECT` 回 `E_FAIL`（2026-10-06 實機的拔除；`rect` 為 `None` 時預設回
    /// `S_FALSE`）。列舉與讀取經真正的 [`assemble_monitor_list`] 組清單；系統作用中的顯示器數＝`rect`
    /// 為 `Some` 的台數（`rect` 是系統的拓樸，`rect_fail` 是 explorer 的回應：`rect` 為 `Some` 又在
    /// `rect_fail` 裡＝作用中螢幕的暫時錯誤或 explorer 落後）。
    rect_fail: HashSet<String>,
    /// 這些螢幕的 `GetMonitorRECT` 回 `S_FALSE`（文件記載的已拔除），即使 `rect` 為 `Some`（系統仍作用中
    /// ＝explorer 落後）。
    rect_s_false: HashSet<String>,
}

const E_FAIL_HR: i32 = 0x8000_4005_u32 as i32;

impl DeskState {
    /// 依各台的矩形與 [`DeskState::rect_fail`] 模擬 `GetMonitorRECT`，交給真正的組清單邏輯。
    fn list(&self) -> Result<Vec<MonitorEntry>, BackendError> {
        assemble_monitor_list(
            self.monitors
                .iter()
                .map(|m| {
                    let (hr, ltrb) = if self.rect_fail.contains(&m.device_path) {
                        (E_FAIL_HR, [0; 4])
                    } else if self.rect_s_false.contains(&m.device_path) {
                        (1, [0; 4])
                    } else {
                        match m.rect {
                            Some(r) => (0, [r.x, r.y, r.x + r.width, r.y + r.height]),
                            None => (1, [0; 4]),
                        }
                    };
                    MonitorProbe {
                        device_path: m.device_path.clone(),
                        hr,
                        ltrb,
                    }
                })
                .collect(),
            u32::try_from(self.monitors.iter().filter(|m| m.rect.is_some()).count()).ok(),
        )
    }
}

/// 放行卡住的 `SetWallpaper`（測試結束時一定放行，工作執行緒才會結束）。
#[derive(Default)]
struct Gate {
    released: Mutex<bool>,
    cv: Condvar,
}

impl Gate {
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.cv.notify_all();
    }
    fn wait(&self) {
        let mut g = self.released.lock().unwrap();
        while !*g {
            g = self.cv.wait(g).unwrap();
        }
    }
}

struct FakeBackend {
    desk: Arc<Mutex<DeskState>>,
    gate: Arc<Gate>,
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
        let hang = {
            let mut d = self.desk.lock().unwrap();
            d.calls.push("list_monitors".into());
            d.hang_list
        };
        if hang {
            self.gate.wait();
        }
        let d = self.desk.lock().unwrap();
        if d.fail_list {
            return Err(BackendError::Com("假列舉失敗".into()));
        }
        d.list()
    }
    fn read(&mut self) -> Result<WallpaperSnapshot, BackendError> {
        let mut d = self.desk.lock().unwrap();
        d.calls.push("read".into());
        if d.fail_read {
            return Err(BackendError::Com("假讀取失敗".into()));
        }
        let list = d.list()?;
        Ok(WallpaperSnapshot {
            monitors: list
                .into_iter()
                .zip(d.monitors.iter())
                .map(|(mut monitor, m)| {
                    if d.read_offline.contains(&m.device_path) {
                        monitor.rect = None;
                    }
                    MonitorWallpaper {
                        monitor,
                        wallpaper: m.wallpaper.clone(),
                    }
                })
                .collect(),
            position: WallpaperPosition(d.position),
            background_color: 0,
            slideshow_status: 0,
            slideshow: None,
        })
    }
    fn set_wallpaper(&mut self, device_path: &str, image: &Path) -> Result<(), BackendError> {
        let hang = {
            let mut d = self.desk.lock().unwrap();
            d.calls
                .push(format!("set_wallpaper {device_path} {}", image.display()));
            d.hang_set
        };
        if hang {
            self.gate.wait();
        }
        let mut d = self.desk.lock().unwrap();
        if d.fail_set.contains(device_path) {
            return Err(BackendError::Com("假 SetWallpaper 失敗".into()));
        }
        let image = image.to_string_lossy().into_owned();
        if let Some(m) = d.monitors.iter_mut().find(|m| m.device_path == device_path) {
            m.wallpaper = Some(image);
        }
        Ok(())
    }
    fn set_wallpaper_all(&mut self, image: &Path) -> Result<(), BackendError> {
        let mut d = self.desk.lock().unwrap();
        d.calls
            .push(format!("set_wallpaper_all {}", image.display()));
        for m in d.monitors.iter_mut().filter(|m| m.rect.is_some()) {
            m.wallpaper = Some(image.to_string_lossy().into_owned());
        }
        Ok(())
    }
    fn set_solid_color(&mut self) -> Result<(), BackendError> {
        let mut d = self.desk.lock().unwrap();
        d.calls.push("set_solid_color".into());
        for m in d.monitors.iter_mut().filter(|m| m.rect.is_some()) {
            m.wallpaper = Some(String::new());
        }
        Ok(())
    }
    fn set_position(&mut self, position: WallpaperPosition) -> Result<(), BackendError> {
        let mut d = self.desk.lock().unwrap();
        d.calls.push(format!("set_position {}", position.0));
        d.position = position.0;
        Ok(())
    }
    fn set_background_color(&mut self, colorref: u32) -> Result<(), BackendError> {
        self.desk
            .lock()
            .unwrap()
            .calls
            .push(format!("set_background_color {colorref:#x}"));
        Ok(())
    }
    fn restore_slideshow(&mut self, _slideshow: &SlideshowInfo) -> Result<(), BackendError> {
        self.desk
            .lock()
            .unwrap()
            .calls
            .push("restore_slideshow".into());
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// 假登錄
// ---------------------------------------------------------------------------------------------

type RegMap = HashMap<(String, String), (u32, Vec<u8>)>;

#[derive(Default)]
struct FakeRegistry {
    values: Arc<Mutex<RegMap>>,
}

impl RegistryStore for FakeRegistry {
    fn get(&self, subkey: &str, name: &str) -> Result<RegValue, RegistryError> {
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
        self.values
            .lock()
            .unwrap()
            .insert((subkey.to_owned(), name.to_owned()), (kind, data.to_vec()));
        Ok(())
    }
    fn delete(&mut self, subkey: &str, name: &str) -> Result<(), RegistryError> {
        self.values
            .lock()
            .unwrap()
            .remove(&(subkey.to_owned(), name.to_owned()));
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// 假宿主（時鐘、時區、設定／暫停／資料、顯示器、渲染、副作用）
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct ClockState {
    wall: UnixSeconds,
    mono: Duration,
}

struct SwitchableTz(Arc<AtomicI32>);

impl UtcOffsetSource for SwitchableTz {
    fn offset_at(&self, _utc: UnixSeconds) -> i32 {
        self.0.load(Ordering::SeqCst)
    }
}

type RenderHook = Box<dyn FnMut(&RenderRequest) + Send>;

struct Shared {
    clock: Mutex<ClockState>,
    tz: Arc<AtomicI32>,
    inputs: Mutex<HostInputs>,
    displays: Mutex<Vec<DisplayInfo>>,
    renders: Mutex<Vec<RenderRequest>>,
    render_fail: Mutex<HashSet<String>>,
    render_hook: Mutex<Option<RenderHook>>,
    /// 渲染時檢查宿主輸入的鎖沒有被持有（契約 1：呼叫 `render()` 時不得持有主執行緒也會取的鎖）。
    lock_held_during_render: AtomicBool,
    effects: Mutex<Vec<String>>,
    dnd: AtomicBool,
    /// `do_not_disturb()` 被呼叫的次數（4.7c：主題為不接管時必須是 0）。
    dnd_calls: AtomicUsize,
    /// `displays()`／`dpi_for_rect()` 被呼叫的次數（主題為不接管時必須是 0）。
    displays_calls: AtomicUsize,
    dpi_calls: AtomicUsize,
    /// 以 `MonitorFromRect` 查得的 DPI（鍵＝矩形）；沒有時退回顯示器清單裡矩形相同者。
    dpi_by_rect: Mutex<HashMap<RectKey, Option<u32>>>,
    /// `sleep` 的每一次長度。
    sleeps: Mutex<Vec<Duration>>,
    /// 假的 explorer GDI 讀值（4.9）與被讀的次數。
    explorer: Mutex<Result<ExplorerSample, String>>,
    explorer_reads: AtomicUsize,
    /// 接下來幾次「主題改不接管」的存檔失敗（task 6.4，審查 R2b-L1）；記憶體照樣改成不接管。
    theme_save_failures: AtomicUsize,
}

type RectKey = (i32, i32, i32, i32);

fn rect_key(r: PhysicalRect) -> RectKey {
    (r.x, r.y, r.width, r.height)
}

struct FakePorts {
    shared: Arc<Shared>,
    tz: SwitchableTz,
}

impl CoordinatorPorts for FakePorts {
    fn now(&self) -> UnixSeconds {
        self.shared.clock.lock().unwrap().wall
    }
    fn monotonic(&self) -> Duration {
        self.shared.clock.lock().unwrap().mono
    }
    fn tz(&self) -> &dyn UtcOffsetSource {
        &self.tz
    }
    fn inputs(&self) -> HostInputs {
        self.shared.inputs.lock().unwrap().clone()
    }
    fn displays(&self) -> Vec<DisplayInfo> {
        self.shared.displays_calls.fetch_add(1, Ordering::SeqCst);
        self.shared.displays.lock().unwrap().clone()
    }
    fn dpi_for_rect(&self, rect: PhysicalRect) -> Option<u32> {
        self.shared.dpi_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(v) = self.shared.dpi_by_rect.lock().unwrap().get(&rect_key(rect)) {
            return *v;
        }
        self.shared
            .displays
            .lock()
            .unwrap()
            .iter()
            .find(|d| d.rect == rect)
            .map(|d| d.dpi)
    }
    fn render(
        &self,
        req: &RenderRequest,
        out: &OutputTarget<'_>,
    ) -> Result<RenderSuccess, RenderFailure> {
        if self.shared.inputs.try_lock().is_err() {
            self.shared
                .lock_held_during_render
                .store(true, Ordering::SeqCst);
        }
        self.shared.renders.lock().unwrap().push(req.clone());
        let hook = self.shared.render_hook.lock().unwrap().take();
        if let Some(mut hook) = hook {
            hook(req);
            let mut slot = self.shared.render_hook.lock().unwrap();
            if slot.is_none() {
                *slot = Some(hook);
            }
        }
        if self
            .shared
            .render_fail
            .lock()
            .unwrap()
            .contains(&req.monitor_key)
        {
            return Err(RenderFailure {
                render_id: Some(1),
                reason: RenderFailureReason::PageFailed("假渲染失敗".into()),
                window_closed: Some(true),
            });
        }
        let png = fake_png(req.width, req.height);
        let written = write_output(
            out.dir,
            &req.monitor_key,
            &png,
            (req.width, req.height),
            out.displayed,
        )
        .expect("假渲染寫檔失敗");
        Ok(RenderSuccess {
            render_id: 1,
            path: written.path,
            slot: written.slot,
            width: req.width,
            height: req.height,
            warnings: Vec::new(),
            body: BodyKind::Raw,
            timings: RenderTimings::default(),
            browser_pid: None,
        })
    }
    fn set_theme_none(&self, cause: ThemeNoneCause) -> Result<(), String> {
        self.shared
            .effects
            .lock()
            .unwrap()
            .push(format!("theme_none {cause:?}"));
        self.shared.inputs.lock().unwrap().theme = WallpaperTheme::None;
        self.take_save_failure()
    }
    fn resave_theme_none(&self) -> Result<bool, String> {
        self.shared
            .effects
            .lock()
            .unwrap()
            .push("theme_none_resave".to_owned());
        self.take_save_failure()?;
        Ok(self.shared.inputs.lock().unwrap().theme == WallpaperTheme::None)
    }
    fn notify_user(&self, notice: &TakeoverNotice) {
        self.shared
            .effects
            .lock()
            .unwrap()
            .push(format!("notify {notice:?}"));
    }
    fn spotlight_confirmation_needed(&self) {
        self.shared
            .effects
            .lock()
            .unwrap()
            .push("spotlight_pending".to_owned());
    }
    fn sleep(&self, d: Duration) {
        self.shared.sleeps.lock().unwrap().push(d);
        let mut c = self.shared.clock.lock().unwrap();
        c.mono += d;
        c.wall += i64::try_from(d.as_secs()).unwrap_or(0);
    }
    fn do_not_disturb(&self) -> bool {
        self.shared.dnd_calls.fetch_add(1, Ordering::SeqCst);
        self.shared.dnd.load(Ordering::SeqCst)
    }
    fn explorer_gdi(&self) -> Result<ExplorerSample, String> {
        self.shared.explorer_reads.fetch_add(1, Ordering::SeqCst);
        self.shared.explorer.lock().unwrap().clone()
    }
}

impl FakePorts {
    fn take_save_failure(&self) -> Result<(), String> {
        let left = &self.shared.theme_save_failures;
        if left.load(Ordering::SeqCst) > 0 {
            left.fetch_sub(1, Ordering::SeqCst);
            return Err("假的存檔失敗".to_owned());
        }
        Ok(())
    }
}

/// 只有檔頭的「PNG」（簽章＋IHDR 寬高）：`write_output` 只核對檔頭尺寸。
fn fake_png(w: u32, h: u32) -> Vec<u8> {
    let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
    v.extend_from_slice(b"IHDR");
    v.extend_from_slice(&w.to_be_bytes());
    v.extend_from_slice(&h.to_be_bytes());
    v
}

#[derive(Default)]
struct TestInbox {
    queue: Mutex<VecDeque<Wake>>,
    shutdown: AtomicBool,
    /// 模擬「信箱裡有尚未處理的要求」（ExitForUpdate 等在 `take_requests` 之後才到）。
    restore: AtomicBool,
}

impl TestInbox {
    fn push(&self, w: Wake) {
        self.queue.lock().unwrap().push_back(w);
    }
}

impl Inbox for TestInbox {
    fn drain(&self) -> Vec<Wake> {
        self.queue.lock().unwrap().drain(..).collect()
    }
    fn shutdown_requested(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }
    fn restore_requested(&self) -> bool {
        self.restore.load(Ordering::SeqCst)
    }
    fn session_end_pending(&self) -> bool {
        session_end_pending_in(self.queue.lock().unwrap().iter().copied())
    }
}

type Logs = Arc<Mutex<Vec<(Level, String)>>>;

struct Env {
    dir: PathBuf,
    paths: StatePaths,
    desk: Arc<Mutex<DeskState>>,
    gate: Arc<Gate>,
    shared: Arc<Shared>,
    logs: Logs,
    inbox: Arc<TestInbox>,
    registry_values: Arc<Mutex<RegMap>>,
    service_timeout: Duration,
    confirm_delay: Duration,
}

impl Drop for Env {
    fn drop(&mut self) {
        self.gate.release();
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn stable(dev: &str) -> String {
    format!("{dev}#stable")
}

fn display(dev: &str, r: PhysicalRect, dpi: u32) -> DisplayInfo {
    DisplayInfo {
        device_path: Some(stable(dev)),
        rect: r,
        dpi,
    }
}

impl Env {
    fn new(name: &str, theme: WallpaperTheme) -> Self {
        let dir = std::env::temp_dir()
            .join(format!("fc-host-coordinator-test-{}", std::process::id()))
            .join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let paths = StatePaths {
            state_file: dir.join(STATE_FILE_NAME),
            output_dir: dir.join("wallpaper"),
            themes_dir: dir.join("Themes"),
        };
        // 使用者的原桌布（真實存在的檔，還原時才用得到原路徑）。
        let pictures = dir.join("pictures");
        fs::create_dir_all(&pictures).unwrap();
        for name in ["a", "b", "c"] {
            fs::write(pictures.join(format!("{name}.jpg")), b"jpg").unwrap();
        }
        let orig = |name: &str| {
            pictures
                .join(format!("{name}.jpg"))
                .to_string_lossy()
                .into_owned()
        };
        let desk = Arc::new(Mutex::new(DeskState {
            monitors: vec![
                FakeMonitor {
                    device_path: DEV_A.into(),
                    rect: Some(rect_a()),
                    wallpaper: Some(orig("a")),
                },
                FakeMonitor {
                    device_path: DEV_B.into(),
                    rect: Some(rect_b()),
                    wallpaper: Some(orig("b")),
                },
            ],
            position: 3,
            ..DeskState::default()
        }));
        let shared = Arc::new(Shared {
            clock: Mutex::new(ClockState {
                wall: taipei(10, 5, 0),
                mono: Duration::from_secs(1_000),
            }),
            tz: Arc::new(AtomicI32::new(TAIPEI)),
            inputs: Mutex::new(HostInputs {
                theme,
                pause: HashSet::new(),
                data: DataDates::default(),
                config_version: Some(0),
            }),
            displays: Mutex::new(vec![
                display(DEV_A, rect_a(), 96),
                display(DEV_B, rect_b(), 144),
            ]),
            renders: Mutex::new(Vec::new()),
            render_fail: Mutex::new(HashSet::new()),
            render_hook: Mutex::new(None),
            lock_held_during_render: AtomicBool::new(false),
            effects: Mutex::new(Vec::new()),
            dnd: AtomicBool::new(false),
            dnd_calls: AtomicUsize::new(0),
            displays_calls: AtomicUsize::new(0),
            dpi_calls: AtomicUsize::new(0),
            dpi_by_rect: Mutex::new(HashMap::new()),
            sleeps: Mutex::new(Vec::new()),
            explorer: Mutex::new(Ok(ExplorerSample {
                pid: EXPLORER_PID,
                gdi: 500,
            })),
            explorer_reads: AtomicUsize::new(0),
            theme_save_failures: AtomicUsize::new(0),
        });
        Env {
            dir,
            paths,
            desk,
            gate: Arc::new(Gate::default()),
            shared,
            logs: Arc::new(Mutex::new(Vec::new())),
            inbox: Arc::new(TestInbox::default()),
            registry_values: Arc::new(Mutex::new(HashMap::new())),
            service_timeout: Duration::from_secs(5),
            confirm_delay: Duration::from_secs(1),
        }
    }

    fn orig(&self, name: &str) -> String {
        self.dir
            .join("pictures")
            .join(format!("{name}.jpg"))
            .to_string_lossy()
            .into_owned()
    }

    fn ports(&self) -> FakePorts {
        FakePorts {
            shared: Arc::clone(&self.shared),
            tz: SwitchableTz(Arc::clone(&self.shared.tz)),
        }
    }

    fn coordinator(&self) -> Coordinator<FakePorts> {
        self.coordinator_with(self.ports())
    }

    fn coordinator_with<P: CoordinatorPorts>(&self, ports: P) -> Coordinator<P> {
        let desk = Arc::clone(&self.desk);
        let gate = Arc::clone(&self.gate);
        let service = WallpaperService::spawn_with(
            WallpaperServiceConfig {
                timeout: self.service_timeout,
                ..WallpaperServiceConfig::default()
            },
            move || FakeBackend { desk, gate },
        )
        .unwrap();
        let logs = Arc::clone(&self.logs);
        let sink: LogSink = Arc::new(move |level, msg: &str| {
            logs.lock().unwrap().push((level, msg.to_owned()));
        });
        Coordinator::new(CoordinatorParts {
            ports,
            service,
            registry: Box::new(FakeRegistry {
                values: Arc::clone(&self.registry_values),
            }),
            files: Box::new(NoFileIdentity),
            paths: self.paths.clone(),
            config: CoordinatorConfig {
                confirm_delay: self.confirm_delay,
                takeover: TakeoverConfig {
                    verify_attempts: 1,
                    verify_interval: Duration::from_millis(1),
                    registry_settle_timeout: Duration::from_millis(5),
                    registry_poll_interval: Duration::from_millis(1),
                    transient_retry_delay: Duration::from_millis(1),
                },
                ..CoordinatorConfig::default()
            },
            log: sink,
        })
    }

    fn step<P: CoordinatorPorts>(&self, c: &mut Coordinator<P>, wakes: &[Wake]) -> StepOutcome {
        c.step(wakes.to_vec(), &*self.inbox)
    }

    fn advance_to(&self, t: UnixSeconds) {
        let mut c = self.shared.clock.lock().unwrap();
        let delta = t - c.wall;
        assert!(delta >= 0, "advance_to 只能往後");
        c.wall = t;
        c.mono += Duration::from_secs(u64::try_from(delta).unwrap());
    }

    fn renders(&self) -> Vec<RenderRequest> {
        self.shared.renders.lock().unwrap().clone()
    }

    fn render_count(&self) -> usize {
        self.shared.renders.lock().unwrap().len()
    }

    fn calls(&self) -> Vec<String> {
        self.desk.lock().unwrap().calls.clone()
    }

    fn set_calls(&self) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter(|c| c.starts_with("set_wallpaper"))
            .collect()
    }

    fn wallpaper(&self, dev: &str) -> Option<String> {
        self.desk
            .lock()
            .unwrap()
            .monitors
            .iter()
            .find(|m| m.device_path == dev)
            .and_then(|m| m.wallpaper.clone())
    }

    fn logs(&self) -> Vec<(Level, String)> {
        self.logs.lock().unwrap().clone()
    }

    fn log_text(&self) -> String {
        self.logs()
            .iter()
            .map(|(l, m)| format!("[{l}] {m}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn has_log(&self, needle: &str) -> bool {
        self.logs().iter().any(|(_, m)| m.contains(needle))
    }

    fn count_logs(&self, needle: &str) -> usize {
        self.logs()
            .iter()
            .filter(|(_, m)| m.contains(needle))
            .count()
    }

    fn set_theme(&self, theme: WallpaperTheme) {
        self.shared.inputs.lock().unwrap().theme = theme;
    }

    fn key(&self, dev: &str, r: PhysicalRect) -> String {
        monitor_key(
            dev,
            Some(r),
            &[
                StableDisplay {
                    device_path: stable(DEV_A),
                    rect: rect_a(),
                },
                StableDisplay {
                    device_path: stable(DEV_B),
                    rect: rect_b(),
                },
                StableDisplay {
                    device_path: stable(DEV_C),
                    rect: rect_c(),
                },
            ],
        )
    }

    fn state_json(&self) -> serde_json::Value {
        serde_json::from_str(&fs::read_to_string(&self.paths.state_file).unwrap()).unwrap()
    }

    fn add_monitor_c(&self) {
        self.desk.lock().unwrap().monitors.push(FakeMonitor {
            device_path: DEV_C.into(),
            rect: Some(rect_c()),
            wallpaper: Some(self.orig("c")),
        });
        self.shared
            .displays
            .lock()
            .unwrap()
            .push(display(DEV_C, rect_c(), 96));
    }
}

fn is_host(path: &Option<String>, env: &Env) -> bool {
    path.as_deref()
        .is_some_and(|p| Path::new(p).starts_with(&env.paths.output_dir))
}

// ---------------------------------------------------------------------------------------------
// 契約 1：計時器與事件喚醒、單一計時器
// ---------------------------------------------------------------------------------------------

#[test]
fn startup_redraws_every_monitor_and_sets_timer_to_next_slot() {
    let env = Env::new("startup", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    let out = env.step(&mut c, &[Wake::Startup]);

    let renders = env.renders();
    assert_eq!(renders.len(), 2, "{}", env.log_text());
    assert_eq!((renders[0].width, renders[0].height), (1920, 1080));
    assert_eq!((renders[1].width, renders[1].height), (2560, 1440));
    assert_eq!(renders[0].monitor_key, env.key(DEV_A, rect_a()));
    // 星盤呈現 10:00 的時點；時區交給頁面（空字串＝系統時區）。
    assert_eq!(renders[0].as_of, taipei(10, 0, 0));
    assert_eq!(renders[0].tz, "");
    assert!(is_host(&env.wallpaper(DEV_A), &env));
    assert!(is_host(&env.wallpaper(DEV_B), &env));
    // 下一次計時器＝10:15（Redraw 之後再評估一次，取那一次的 next_wake）。
    assert_eq!(out.next_wake, Some(taipei(10, 15, 0)));
    assert_eq!(c.status().state, CoordinatorState::UpToDate);
}

#[test]
fn timer_wake_before_slot_does_nothing_and_at_slot_redraws() {
    let env = Env::new("timer", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    env.advance_to(taipei(10, 14, 0));
    let out = env.step(&mut c, &[Wake::Timer]);
    assert_eq!(env.render_count(), 2, "時點前喚醒不重畫");
    assert_eq!(out.next_wake, Some(taipei(10, 15, 0)));

    env.advance_to(taipei(10, 15, 0));
    let out = env.step(&mut c, &[Wake::Timer]);
    assert_eq!(env.render_count(), 4, "時點到了兩台都重畫");
    assert_eq!(env.renders()[2].as_of, taipei(10, 15, 0));
    assert_eq!(out.next_wake, Some(taipei(10, 30, 0)));
}

#[test]
fn data_event_wakes_skyline_redraw_only_when_trading_day_changes() {
    let env = Env::new("data-event", WallpaperTheme::Skyline);
    env.shared.inputs.lock().unwrap().data.twii_daily = CivilDate::parse_iso("2026-10-02");
    let mut c = env.coordinator();
    let out = env.step(&mut c, &[Wake::Startup]);
    assert_eq!(env.render_count(), 2);
    assert_eq!(out.next_wake, None, "天際線只看資料，沒有時間觸發");

    // 資料通道更新但交易日沒變：不重畫。
    env.step(&mut c, &[Wake::DataChanged]);
    assert_eq!(env.render_count(), 2);

    env.shared.inputs.lock().unwrap().data.twii_daily = CivilDate::parse_iso("2026-10-05");
    env.step(&mut c, &[Wake::DataChanged]);
    assert_eq!(env.render_count(), 4, "新交易日 → 重畫");
}

#[test]
fn renderer_is_called_without_holding_host_input_locks() {
    let env = Env::new("no-lock", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    assert_eq!(env.render_count(), 2);
    assert!(
        !env.shared.lock_held_during_render.load(Ordering::SeqCst),
        "render() 期間不得持有宿主輸入的鎖"
    );
}

// ---------------------------------------------------------------------------------------------
// 契約 2：Redraw 執行流程（Render／Apply 分類、confirm_applied、事件合併、恢復順序）
// ---------------------------------------------------------------------------------------------

#[test]
fn applied_monitors_are_confirmed_by_readback_after_a_delay() {
    let env = Env::new("confirm", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    let mono_before = env.shared.clock.lock().unwrap().mono;
    env.step(&mut c, &[Wake::Startup]);
    let mono_after = env.shared.clock.lock().unwrap().mono;
    assert!(
        mono_after - mono_before >= Duration::from_secs(1),
        "設定後約 1 秒再讀回"
    );
    let state = env.state_json();
    let monitors = state["monitors"].as_object().unwrap();
    assert_eq!(monitors.len(), 2);
    for (_, rec) in monitors {
        assert_eq!(rec["host_applied"], true, "{rec}");
    }
    // 設定之後還有一次讀回（confirm）。
    let calls = env.calls();
    let last_set = calls
        .iter()
        .rposition(|c| c.starts_with("set_wallpaper"))
        .unwrap();
    assert!(
        calls[last_set + 1..].iter().any(|c| c == "read"),
        "{calls:?}"
    );
    assert!(env.has_log("確認已套用"), "{}", env.log_text());
}

/// task 6.4（審查 R2a-L1）：設定失敗時螢幕仍是上一格；下一次渲染不得寫進「螢幕正在顯示的那一格」
/// 再以同一路徑重設（a/b 選格以讀回確認顯示中的格為準，不用「準備設定」的 `last_set`）。
#[test]
fn set_failure_does_not_make_the_next_render_overwrite_the_displayed_slot() {
    let env = Env::new("r64-displayed-slot", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    env.desk.lock().unwrap().fail_set.insert(DEV_B.into());
    env.advance_to(taipei(10, 15, 0));
    env.step(&mut c, &[Wake::Timer]);
    env.desk.lock().unwrap().fail_set.clear();
    env.advance_to(taipei(10, 30, 0));
    env.step(&mut c, &[Wake::Timer]);

    let b_sets: Vec<String> = env
        .set_calls()
        .into_iter()
        .filter(|s| s.contains(DEV_B))
        .collect();
    assert_eq!(b_sets.len(), 3, "{b_sets:?}\n{}", env.log_text());
    assert!(b_sets[0].ends_with("-a.png"), "{b_sets:?}");
    assert!(
        b_sets[1].ends_with("-b.png"),
        "第二次（失敗）寫 -b：{b_sets:?}"
    );
    assert!(
        b_sets[2].ends_with("-b.png"),
        "螢幕仍顯示 -a：第三次不得覆寫 -a、要寫 -b：{b_sets:?}"
    );
    assert!(
        env.wallpaper(DEV_B).is_some_and(|w| w.ends_with("-b.png")),
        "{:?}",
        env.wallpaper(DEV_B)
    );
}

/// task 6.4（審查 R2b-L3）：多螢幕重畫途中進入暫停（鎖定）：其餘螢幕中止、已設定的保留；解除後依既有
/// 規則補畫中止的那台。
#[test]
fn pause_arriving_mid_redraw_aborts_the_remaining_monitors() {
    let env = Env::new("r64-pause-mid", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    let sets_before = env.set_calls().len();
    let shared = Arc::clone(&env.shared);
    let mut fired = false;
    *env.shared.render_hook.lock().unwrap() = Some(Box::new(move |_req| {
        if !fired {
            fired = true;
            shared
                .inputs
                .lock()
                .unwrap()
                .pause
                .insert(PauseReason::Locked);
        }
    }));
    env.advance_to(taipei(10, 15, 0));
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(env.render_count(), 3, "第二台不渲染\n{}", env.log_text());
    assert_eq!(
        env.set_calls().len(),
        sets_before,
        "已渲染的那台也不送：{:?}",
        env.set_calls()
    );
    assert!(env.has_log("Redraw 中止"), "{}", env.log_text());

    env.shared.inputs.lock().unwrap().pause.clear();
    env.step(&mut c, &[Wake::PauseChanged]);
    assert_eq!(env.render_count(), 5, "解除後補畫兩台\n{}", env.log_text());
}

/// task 6.4（審查 R2b-L3）：重畫途中進入勿打擾：同樣中止其餘螢幕。
#[test]
fn do_not_disturb_arriving_mid_redraw_aborts_the_remaining_monitors() {
    let env = Env::new("r64-dnd-mid", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    let sets_before = env.set_calls().len();
    let shared = Arc::clone(&env.shared);
    *env.shared.render_hook.lock().unwrap() = Some(Box::new(move |_req| {
        shared.dnd.store(true, Ordering::SeqCst);
    }));
    env.advance_to(taipei(10, 15, 0));
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(env.render_count(), 3, "{}", env.log_text());
    assert_eq!(env.set_calls().len(), sets_before, "{:?}", env.set_calls());
    assert_eq!(c.status().state, CoordinatorState::DoNotDisturb);
}

/// task 6.4（審查 R2a-L2）：協調執行緒 panic 時信箱照樣關閉——系統匣結束立即得到「斷線」、不必等滿
/// 上限，命令列交接立即知道交接失敗。
#[test]
fn coordinator_panic_closes_the_mailbox() {
    struct PanickingPorts(FakePorts);
    impl CoordinatorPorts for PanickingPorts {
        fn now(&self) -> UnixSeconds {
            self.0.now()
        }
        fn monotonic(&self) -> Duration {
            self.0.monotonic()
        }
        fn tz(&self) -> &dyn UtcOffsetSource {
            self.0.tz()
        }
        fn inputs(&self) -> HostInputs {
            panic!("假的協調迴圈 panic（測試）");
        }
        fn displays(&self) -> Vec<DisplayInfo> {
            self.0.displays()
        }
        fn dpi_for_rect(&self, rect: PhysicalRect) -> Option<u32> {
            self.0.dpi_for_rect(rect)
        }
        fn render(
            &self,
            req: &RenderRequest,
            out: &OutputTarget<'_>,
        ) -> Result<RenderSuccess, RenderFailure> {
            self.0.render(req, out)
        }
        fn set_theme_none(&self, cause: ThemeNoneCause) -> Result<(), String> {
            self.0.set_theme_none(cause)
        }
        fn resave_theme_none(&self) -> Result<bool, String> {
            self.0.resave_theme_none()
        }
        fn notify_user(&self, notice: &TakeoverNotice) {
            self.0.notify_user(notice);
        }
        fn sleep(&self, d: Duration) {
            self.0.sleep(d);
        }
        fn explorer_gdi(&self) -> Result<ExplorerSample, String> {
            self.0.explorer_gdi()
        }
    }
    let env = Env::new("r64-panic", WallpaperTheme::Astrolabe);
    let c = env.coordinator_with(PanickingPorts(env.ports()));
    let (handle, join) = spawn_coordinator(move || Ok(c)).unwrap();
    assert!(join.join().is_err(), "協調執行緒 panic");
    let t = Instant::now();
    let wait = exit_and_restore(
        &handle,
        ExitLimits {
            redraw_wait: Duration::from_secs(5),
            restore_limit: Duration::from_secs(5),
        },
    );
    assert_eq!(wait, ExitWait::Disconnected);
    assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
}

/// 渲染失敗與「explorer 有回應但設定回錯」（`ApplyError`）都由排程器依 `retry_delay` 退避重試，不等
/// 下一個時點；等待期間的決策是 `RetryPending`、記錄寫明等待重試，不是 `UpToDate`（2026-10-06 在場
/// 驗收：舊做法把套用失敗記成 Apply、決策寫 UpToDate，補畫只能靠別的事件順帶發生）。
#[test]
fn render_failure_and_apply_error_back_off_instead_of_waiting_for_next_slot() {
    let env = Env::new("classify", WallpaperTheme::Astrolabe);
    env.shared
        .render_fail
        .lock()
        .unwrap()
        .insert(env.key(DEV_A, rect_a()));
    env.desk.lock().unwrap().fail_set.insert(DEV_B.into());
    let mut c = env.coordinator();
    let out = env.step(&mut c, &[Wake::Startup]);
    assert_eq!(env.render_count(), 2);
    // A 渲染失敗、B 設定回錯 → 兩台都 2 分鐘後重試（不是 10:15）。
    let now = env.shared.clock.lock().unwrap().wall;
    assert_eq!(out.next_wake, Some(now + 120), "{}", env.log_text());
    assert!(env.has_log("渲染失敗"));
    assert!(env.has_log("記錄 ApplyError"), "{}", env.log_text());
    assert_eq!(c.status().state, CoordinatorState::RetryPending);
    let decision = last_decision(&env);
    assert!(
        decision.starts_with("決策：RetryPending；等待重試："),
        "{decision}"
    );
    assert!(decision.contains("套用失敗，第 1 次"), "{decision}");
    assert!(decision.contains("渲染失敗，第 1 次"), "{decision}");

    // 2 分鐘後兩台都重試；B 再失敗 → 第 2 次，間隔拉長到 4 分鐘。
    env.advance_to(now + 120);
    let out = env.step(&mut c, &[Wake::Timer]);
    assert_eq!(env.render_count(), 4, "A、B 都重試\n{}", env.log_text());
    let t = env.shared.clock.lock().unwrap().wall;
    let b_sets = env.set_calls().iter().filter(|s| s.contains(DEV_B)).count();
    assert_eq!(b_sets, 2, "{:?}", env.set_calls());
    assert!(
        last_decision(&env).contains("套用失敗，第 2 次"),
        "{}",
        env.log_text()
    );
    // 兩台都是第 2 次失敗：渲染與套用失敗走同一條退避曲線，都是 4 分鐘後。
    assert_eq!(out.next_wake, Some(t + 240), "{}", env.log_text());

    // 恢復：下一次重試成功，之後才是真正的 UpToDate、計時器回到下一個時點。
    env.desk.lock().unwrap().fail_set.clear();
    env.shared.render_fail.lock().unwrap().clear();
    env.advance_to(t + 240);
    let out = env.step(&mut c, &[Wake::Timer]);
    assert!(is_host(&env.wallpaper(DEV_A), &env), "{}", env.log_text());
    assert!(is_host(&env.wallpaper(DEV_B), &env), "{}", env.log_text());
    assert_eq!(c.status().state, CoordinatorState::UpToDate);
    assert_eq!(out.next_wake, Some(taipei(10, 15, 0)));
}

// ---------------------------------------------------------------------------------------------
// 一台螢幕離線（GetMonitorRECT 回 E_FAIL）：2026-10-06 在場驗收
// ---------------------------------------------------------------------------------------------

impl Env {
    /// 拔掉 A：`IDesktopWallpaper` 仍列舉它、`GetMonitorRECT` 回 E_FAIL；顯示器清單只剩 B，B 移到 (0,0)。
    fn unplug_a_with_rect_failure(&self) {
        let mut d = self.desk.lock().unwrap();
        d.rect_fail.insert(DEV_A.into());
        d.monitors[0].rect = None;
        d.monitors[1].rect = Some(rect(0, 2560, 1440));
        drop(d);
        *self.shared.displays.lock().unwrap() = vec![display(DEV_B, rect(0, 2560, 1440), 144)];
    }

    /// 插回 A：矩形恢復、B 回到原位。
    fn replug_a(&self) {
        let mut d = self.desk.lock().unwrap();
        d.rect_fail.clear();
        d.monitors[0].rect = Some(rect_a());
        d.monitors[1].rect = Some(rect_b());
        drop(d);
        *self.shared.displays.lock().unwrap() =
            vec![display(DEV_A, rect_a(), 96), display(DEV_B, rect_b(), 144)];
    }

    fn sets_for(&self, dev: &str) -> usize {
        self.set_calls().iter().filter(|s| s.contains(dev)).count()
    }
}

/// 接管中拔掉一台（RECT 回 E_FAIL）：列舉不整批失敗，在線那台以新座標照常出圖與設定，離線那台不渲染、
/// 不設定；插回後以「沒有紀錄」補畫。
#[test]
fn one_monitor_rect_failure_does_not_block_the_online_monitor() {
    let env = Env::new("offline-efail", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    assert_eq!(env.render_count(), 2);
    let (a_sets, b_sets) = (env.sets_for(DEV_A), env.sets_for(DEV_B));

    env.unplug_a_with_rect_failure();
    env.step(&mut c, &[Wake::DisplayChanged]);
    assert!(!env.has_log("列舉螢幕失敗"), "{}", env.log_text());
    assert!(env.has_log("在線 1 台"), "{}", env.log_text());
    assert!(
        env.has_log(&format!("{DEV_A}（離線）")),
        "{}",
        env.log_text()
    );

    // 下一個時點：只有 B 出圖並設定成功；A 不渲染、不設定。
    env.advance_to(taipei(10, 15, 0));
    let out = env.step(&mut c, &[Wake::Timer]);
    let renders = env.renders();
    assert_eq!(renders.len(), 3, "{}", env.log_text());
    assert_eq!((renders[2].width, renders[2].height), (2560, 1440));
    assert_eq!(env.sets_for(DEV_A), a_sets, "離線那台不設定");
    assert_eq!(env.sets_for(DEV_B), b_sets + 1);
    assert!(
        env.has_log("→ Applied"),
        "設定前讀回不因 A 離線而失敗\n{}",
        env.log_text()
    );
    assert!(!env.has_log("讀回目前顯示的桌布失敗"), "{}", env.log_text());
    assert!(!env.has_log("設定桌布失敗"), "{}", env.log_text());
    assert!(!env.has_log("無法取得 DPI"), "{}", env.log_text());
    assert_eq!(c.status().state, CoordinatorState::UpToDate);
    assert_eq!(out.next_wake, Some(taipei(10, 30, 0)));

    // 插回：A 以「沒有紀錄」補畫，B 不重畫。
    env.replug_a();
    env.step(&mut c, &[Wake::DisplayChanged]);
    assert!(
        env.has_log(&format!("{DEV_A}（Unrecorded）")),
        "離線期間 A 的紀錄已移除（沒有被當成已畫好）\n{}",
        env.log_text()
    );
    assert_eq!(env.render_count(), 4, "{}", env.log_text());
    assert_eq!(env.sets_for(DEV_A), a_sets + 1);
    assert_eq!(env.sets_for(DEV_B), b_sets + 1);
    assert!(is_host(&env.wallpaper(DEV_A), &env));
}

/// 離線那台進入既有的離線處理：拔掉時改「不接管」→ 在線那台還原、離線那台標為待還原（還原不因它的
/// RECT 失敗而整批失敗）；插回時補還原。
#[test]
fn restore_with_one_monitor_rect_failure_marks_it_pending_and_restores_it_on_replug() {
    let env = Env::new("offline-efail-restore", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    assert!(is_host(&env.wallpaper(DEV_A), &env));

    env.unplug_a_with_rect_failure();
    env.step(&mut c, &[Wake::DisplayChanged]);
    env.set_theme(WallpaperTheme::None);
    env.step(&mut c, &[Wake::SettingsChanged]);
    assert_eq!(
        env.wallpaper(DEV_B),
        Some(env.orig("b")),
        "{}",
        env.log_text()
    );
    assert!(is_host(&env.wallpaper(DEV_A), &env), "離線那台還沒還原");
    let state = env.state_json();
    let pending: Vec<bool> = state["monitors"]
        .as_object()
        .unwrap()
        .values()
        .filter(|r| r["device_path"] == DEV_A)
        .map(|r| r["pending_restore"] == true)
        .collect();
    assert_eq!(pending, vec![true], "{state}");

    env.replug_a();
    env.step(&mut c, &[Wake::DisplayChanged]);
    assert_eq!(
        env.wallpaper(DEV_A),
        Some(env.orig("a")),
        "{}",
        env.log_text()
    );
    assert_eq!(env.wallpaper(DEV_B), Some(env.orig("b")));
}

/// 全部螢幕 RECT 失敗：沿用既有錯誤處理（列舉回錯、沿用上次清單），但舊清單的座標不拿來出圖設定——
/// 記 ApplyError、決策是 RetryPending；RECT 恢復後於退避時刻以新清單補畫。
#[test]
fn all_monitor_rect_failures_keep_old_list_but_never_draw_with_its_coordinates() {
    let env = Env::new("offline-efail-all", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    let sets = env.set_calls().len();
    {
        let mut d = env.desk.lock().unwrap();
        d.rect_fail.insert(DEV_A.into());
        d.rect_fail.insert(DEV_B.into());
    }
    env.advance_to(taipei(10, 15, 0));
    let out = env.step(&mut c, &[Wake::DisplayChanged]);
    assert!(env.has_log("列舉螢幕失敗"), "{}", env.log_text());
    assert!(
        env.has_log("預檢跳過：螢幕清單不是最新"),
        "{}",
        env.log_text()
    );
    assert_eq!(env.render_count(), 2, "不以舊座標渲染\n{}", env.log_text());
    assert_eq!(env.set_calls().len(), sets, "不設定");
    assert_eq!(c.status().state, CoordinatorState::RetryPending);
    let decision = last_decision(&env);
    assert!(decision.contains("螢幕清單沿用上次"), "{decision}");
    let now = env.shared.clock.lock().unwrap().wall;
    assert_eq!(out.next_wake, Some(now + 120), "{}", env.log_text());

    env.desk.lock().unwrap().rect_fail.clear();
    env.advance_to(now + 120);
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(env.render_count(), 4, "{}", env.log_text());
    assert_eq!(env.set_calls().len(), sets + 2);
    assert_eq!(c.status().state, CoordinatorState::UpToDate);
}

/// 列舉失敗而這次沒有要重畫的螢幕：決策不記成 UpToDate、不等下一個時點，依退避短間隔再列舉（2、4 分鐘）；
/// 恢復後清單更新、回到 UpToDate、計時器回到下一個時點。
#[test]
fn monitor_list_failure_without_redraw_retries_listing_with_backoff() {
    let env = Env::new("list-retry", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    assert_eq!(env.render_count(), 2);
    let lists = |env: &Env| {
        env.calls()
            .iter()
            .filter(|c| c.as_str() == "list_monitors")
            .count()
    };

    env.desk.lock().unwrap().fail_list = true;
    let before = lists(&env);
    let out = env.step(&mut c, &[Wake::DisplayChanged]);
    let t0 = env.shared.clock.lock().unwrap().wall;
    assert!(env.has_log("列舉螢幕失敗"), "{}", env.log_text());
    assert_eq!(lists(&env), before + 1);
    assert_eq!(c.status().state, CoordinatorState::RetryPending);
    assert_eq!(out.next_wake, Some(t0 + 120), "{}", env.log_text());
    let decision = last_decision(&env);
    assert!(decision.contains("再列舉螢幕"), "{decision}");
    assert!(decision.contains("螢幕清單沿用上次"), "{decision}");

    // 到期再列舉、又失敗 → 第 2 次，間隔 4 分鐘。
    env.advance_to(t0 + 120);
    let out = env.step(&mut c, &[Wake::Timer]);
    assert_eq!(lists(&env), before + 2, "{}", env.log_text());
    assert_eq!(out.next_wake, Some(t0 + 120 + 240), "{}", env.log_text());
    assert_eq!(c.status().state, CoordinatorState::RetryPending);

    // 恢復：列舉成功、沒有要重畫的螢幕 → 真正的 UpToDate。
    env.desk.lock().unwrap().fail_list = false;
    env.advance_to(t0 + 360);
    let out = env.step(&mut c, &[Wake::Timer]);
    assert_eq!(lists(&env), before + 3, "{}", env.log_text());
    assert_eq!(c.status().state, CoordinatorState::UpToDate);
    assert_eq!(out.next_wake, Some(taipei(10, 15, 0)));
    assert!(!last_decision(&env).contains("再列舉螢幕"));
    assert_eq!(env.render_count(), 2, "清單沒變，不重畫");
}

impl Env {
    /// 插回 A，但 COM 落後：顯示器清單（系統）已含 A、矩形恢復，`GetMonitorRECT(A)` 仍回 E_FAIL。
    fn replug_a_with_com_lagging(&self) {
        self.replug_a();
        self.desk.lock().unwrap().rect_fail.insert(DEV_A.into());
    }

    /// 同上，但 explorer 落後時對 A 回 `S_FALSE`（文件記載的「已拔除」回法），不是 E_FAIL。
    fn replug_a_with_com_lagging_s_false(&self) {
        self.replug_a();
        self.desk.lock().unwrap().rect_s_false.insert(DEV_A.into());
    }

    fn now(&self) -> UnixSeconds {
        self.shared.clock.lock().unwrap().wall
    }
}

/// [審查 High-1] 接管中插回時 COM 比顯示變更事件慢：事件到達時 A 仍回 E_FAIL。不得把這次列舉當成
/// 最新、記成 UpToDate 後就不再看——要短間隔再列舉，COM 追上後（沒有新事件）補畫 A。
#[test]
fn replug_while_com_lags_behind_display_change_is_redrawn_once_com_catches_up() {
    replug_lag_takeover_case("offline-com-lag", Env::replug_a_with_com_lagging);
}

/// [複審 Medium-A] 同上，但 explorer 落後時對 A 回 `S_FALSE`（文件記載的回法）：S_FALSE 同樣要以系統
/// 作用中的顯示器數佐證才算離線。
#[test]
fn replug_while_com_lags_reporting_s_false_is_redrawn_once_com_catches_up() {
    replug_lag_takeover_case(
        "offline-com-lag-s-false",
        Env::replug_a_with_com_lagging_s_false,
    );
}

impl Env {
    /// explorer 追上：A 的 `GetMonitorRECT` 恢復正常。
    fn com_catches_up(&self) {
        let mut d = self.desk.lock().unwrap();
        d.rect_fail.clear();
        d.rect_s_false.clear();
    }
}

fn replug_lag_takeover_case(name: &str, replug: fn(&Env)) {
    let env = Env::new(name, WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    let a_sets = env.sets_for(DEV_A);
    env.unplug_a_with_rect_failure();
    env.step(&mut c, &[Wake::DisplayChanged]);

    replug(&env);
    let out = env.step(&mut c, &[Wake::DisplayChanged]);
    let t0 = env.now();
    assert_eq!(env.sets_for(DEV_A), a_sets, "COM 還沒恢復，不設定 A");
    assert_ne!(
        c.status().state,
        CoordinatorState::UpToDate,
        "{}",
        env.log_text()
    );
    let wake = out.next_wake.expect("要排短間隔再列舉");
    assert!(
        wake > t0 && wake <= t0 + 120,
        "{wake} vs {t0}\n{}",
        env.log_text()
    );

    // COM 追上，沒有新事件：到期再列舉並補畫 A。
    env.com_catches_up();
    env.advance_to(wake);
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(env.sets_for(DEV_A), a_sets + 1, "{}", env.log_text());
    assert!(is_host(&env.wallpaper(DEV_A), &env));
    assert_eq!(c.status().state, CoordinatorState::UpToDate);
}

/// [審查 High-1] 待還原的螢幕插回時 COM 落後：顯示變更當下讀不到 A，要短間隔重試待還原，COM 追上後
/// （沒有新事件）補還原 A。
#[test]
fn pending_restore_retries_when_com_lags_behind_display_change() {
    replug_lag_pending_case("offline-com-lag-restore", Env::replug_a_with_com_lagging);
}

/// [複審 Medium-A] 同上，explorer 落後時回 `S_FALSE`：不得算成「仍離線」而不重試。
#[test]
fn pending_restore_retries_when_com_lags_reporting_s_false() {
    replug_lag_pending_case(
        "offline-com-lag-restore-s-false",
        Env::replug_a_with_com_lagging_s_false,
    );
}

/// 接管中拔掉 A、改「不接管」（B 還原、A 標為待還原），回傳協調迴圈。
fn pending_a_after_unplug(env: &Env) -> Coordinator<FakePorts> {
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    env.unplug_a_with_rect_failure();
    env.step(&mut c, &[Wake::DisplayChanged]);
    env.set_theme(WallpaperTheme::None);
    env.step(&mut c, &[Wake::SettingsChanged]);
    assert_eq!(env.wallpaper(DEV_B), Some(env.orig("b")));
    assert!(is_host(&env.wallpaper(DEV_A), env));
    c
}

fn replug_lag_pending_case(name: &str, replug: fn(&Env)) {
    let env = Env::new(name, WallpaperTheme::Astrolabe);
    let mut c = pending_a_after_unplug(&env);

    replug(&env);
    let out = env.step(&mut c, &[Wake::DisplayChanged]);
    let t0 = env.now();
    assert!(is_host(&env.wallpaper(DEV_A), &env), "COM 還沒恢復");
    let wake = out.next_wake.expect("待還原要排重試");
    assert!(
        wake > t0 && wake <= t0 + 120,
        "{wake} vs {t0}\n{}",
        env.log_text()
    );

    env.com_catches_up();
    env.advance_to(wake);
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(
        env.wallpaper(DEV_A),
        Some(env.orig("a")),
        "{}",
        env.log_text()
    );
    assert_eq!(env.wallpaper(DEV_B), Some(env.orig("b")));
}

/// [複審 Low-B] 待還原重試排定後工作階段結束：時刻越過重試時刻也不得處理待還原（不呼叫桌布 COM）——
/// 工作階段結束不還原。
#[test]
fn pending_retry_is_not_run_while_session_is_ending() {
    let env = Env::new("pending-retry-session-end", WallpaperTheme::Astrolabe);
    let mut c = pending_a_after_unplug(&env);
    env.replug_a_with_com_lagging();
    let out = env.step(&mut c, &[Wake::DisplayChanged]);
    let wake = out.next_wake.expect("待還原要排重試");
    env.com_catches_up();

    env.step(&mut c, &[Wake::SessionEnding]);
    let calls = env.calls().len();
    env.advance_to(wake + 60);
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(
        env.calls()[calls..].to_vec(),
        Vec::<String>::new(),
        "工作階段結束中不得呼叫桌布 COM\n{}",
        env.log_text()
    );
    assert!(is_host(&env.wallpaper(DEV_A), &env), "不還原");
}

/// [v0.1.0 最終審查 Minor 2] 待還原重試到期時信箱裡有尚未處理的要求（例如 ExitForUpdate 在
/// `take_requests` 之後才到、或 Redraw 因要求中止後的再評估）：不得處理待還原（不呼叫桌布 COM），
/// 要求優先；要求處理完（這裡以清掉旗標模擬命令列還原後繼續）後重試照常做、不遺失。
#[test]
fn pending_retry_is_deferred_while_request_is_pending() {
    let env = Env::new("pending-retry-request", WallpaperTheme::Astrolabe);
    let mut c = pending_a_after_unplug(&env);
    env.replug_a_with_com_lagging();
    let out = env.step(&mut c, &[Wake::DisplayChanged]);
    let wake = out.next_wake.expect("待還原要排重試");
    env.com_catches_up();

    env.inbox.restore.store(true, Ordering::SeqCst);
    let calls = env.calls().len();
    env.advance_to(wake + 1);
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(
        env.calls()[calls..].to_vec(),
        Vec::<String>::new(),
        "有待處理的要求時不得呼叫桌布 COM\n{}",
        env.log_text()
    );
    assert!(is_host(&env.wallpaper(DEV_A), &env), "還沒還原");

    env.inbox.restore.store(false, Ordering::SeqCst);
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(
        env.wallpaper(DEV_A),
        Some(env.orig("a")),
        "要求處理完後重試照常做\n{}",
        env.log_text()
    );
}

/// [v0.1.0 最終審查 Minor 2] 同上，觸發點是顯示變更：有待處理的要求時不處理待還原，改排成立即到期的
/// 待還原重試，要求處理完後的下一次 step 補做（顯示變更事件不會再來，不能直接丟掉）。
#[test]
fn display_change_pending_restore_is_deferred_while_request_is_pending() {
    let env = Env::new("pending-display-request", WallpaperTheme::Astrolabe);
    let mut c = pending_a_after_unplug(&env);
    env.replug_a();

    env.inbox.restore.store(true, Ordering::SeqCst);
    let calls = env.calls().len();
    env.step(&mut c, &[Wake::DisplayChanged]);
    assert_eq!(
        env.calls()[calls..].to_vec(),
        Vec::<String>::new(),
        "有待處理的要求時不得呼叫桌布 COM\n{}",
        env.log_text()
    );
    assert!(is_host(&env.wallpaper(DEV_A), &env), "還沒還原");

    env.inbox.restore.store(false, Ordering::SeqCst);
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(
        env.wallpaper(DEV_A),
        Some(env.orig("a")),
        "要求處理完後補做待還原\n{}",
        env.log_text()
    );
}

/// [複審 Low-C] 狀態檔只有在啟動時才可能讀不到（讀到之後狀態機不會再回到「讀不到」）。這段期間的顯示
/// 變更處理不了待還原螢幕，但不會遺失：狀態檔讀到時的延後啟動判定（主題「不接管」）會走還原，處理
/// 待還原的螢幕，不需要再有顯示變更事件。
#[test]
fn pending_restore_during_unreadable_state_at_startup_is_handled_once_readable() {
    let env = Env::new("pending-unreadable-startup", WallpaperTheme::Astrolabe);
    let c = pending_a_after_unplug(&env);
    drop(c);
    // 宿主重新啟動前 A 已插回（explorer 也已追上）。
    env.replug_a();

    let lock = lock_state_file(&env.paths.state_file);
    let mut c = env.coordinator();
    let out = env.step(&mut c, &[Wake::Startup]);
    assert_eq!(c.status().state, CoordinatorState::WaitingForStateFile);
    env.step(&mut c, &[Wake::DisplayChanged]);
    assert!(
        env.has_log("狀態檔暫時讀不到，待還原螢幕等狀態檔讀到後處理"),
        "{}",
        env.log_text()
    );
    assert!(is_host(&env.wallpaper(DEV_A), &env));

    drop(lock);
    env.advance_to(out.next_wake.expect("要排重讀狀態檔"));
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(
        env.wallpaper(DEV_A),
        Some(env.orig("a")),
        "{}",
        env.log_text()
    );
}

/// [審查 Medium-1] 作用中的螢幕暫時回錯（系統顯示器數沒變、只是 `GetMonitorRECT` 回錯）不得被當成離線：
/// 還原不把它標成待還原（否則解除安裝後永久停在宿主的圖），整次還原改排重試，恢復後兩台都還原。
#[test]
fn transient_rect_failure_on_an_active_monitor_is_not_treated_as_offline() {
    let env = Env::new("transient-rect-failure", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    assert!(is_host(&env.wallpaper(DEV_B), &env));

    env.desk.lock().unwrap().rect_fail.insert(DEV_B.into());
    env.set_theme(WallpaperTheme::None);
    env.step(&mut c, &[Wake::SettingsChanged]);
    let state = env.state_json();
    let pending_b = state["monitors"]
        .as_object()
        .map(|m| {
            m.values()
                .any(|r| r["device_path"] == DEV_B && r["pending_restore"] == true)
        })
        .unwrap_or(false);
    assert!(!pending_b, "B 不得被標成待還原（離線）\n{state}");
    let retry_at = c.status().restore_retry_at.expect("還原要排重試");

    env.desk.lock().unwrap().rect_fail.clear();
    env.advance_to(retry_at);
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(
        env.wallpaper(DEV_A),
        Some(env.orig("a")),
        "{}",
        env.log_text()
    );
    assert_eq!(
        env.wallpaper(DEV_B),
        Some(env.orig("b")),
        "{}",
        env.log_text()
    );
}

/// [審查 Low-3] 設定前讀回說這台不在線（列舉之後才拔掉）＝狀態機拒絕（MonitorNotOnline）：螢幕清單已
/// 不可信，下一次評估前重新列舉，而不是等下一個顯示變更事件。
#[test]
fn refused_monitor_not_online_relists_monitors() {
    let env = Env::new("refused-not-online", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    let lists = |env: &Env| {
        env.calls()
            .iter()
            .filter(|c| c.as_str() == "list_monitors")
            .count()
    };
    env.desk.lock().unwrap().read_offline.insert(DEV_A.into());
    let before = lists(&env);
    env.advance_to(taipei(10, 15, 0));
    env.step(&mut c, &[Wake::Timer]);
    assert!(env.has_log("MonitorNotOnline"), "{}", env.log_text());
    assert!(lists(&env) > before, "拒絕後要重新列舉\n{}", env.log_text());
}

/// [審查 Low-1] 列舉逾時（COM 忙碌）後時間越過列舉重試時刻：忙碌期間不列舉，也不得排過去的喚醒
/// （否則迴圈空轉）；忙碌解除後才再列舉。
#[test]
fn overdue_list_retry_while_busy_never_schedules_a_past_wake() {
    let mut env = Env::new("list-retry-busy", WallpaperTheme::Astrolabe);
    env.service_timeout = Duration::from_millis(40);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    env.desk.lock().unwrap().hang_list = true;
    env.step(&mut c, &[Wake::DisplayChanged]);
    assert!(c.service_busy(), "{}", env.log_text());
    assert!(env.has_log("列舉螢幕失敗"), "{}", env.log_text());

    let t0 = env.now();
    env.advance_to(t0 + 300);
    let out = env.step(&mut c, &[Wake::Timer]);
    let now = env.now();
    assert!(c.service_busy());
    assert!(
        out.next_wake.is_none_or(|w| w > now),
        "next_wake {:?} 不得在過去（now {now}）\n{}",
        out.next_wake,
        env.log_text()
    );

    env.desk.lock().unwrap().hang_list = false;
    env.gate.release();
    wait_until_not_busy(&c);
}

fn last_decision(env: &Env) -> String {
    env.logs()
        .into_iter()
        .rev()
        .find(|(_, m)| m.starts_with("決策："))
        .map(|(_, m)| m)
        .unwrap_or_default()
}

/// 設定逾時（explorer 卡住）維持原語意：記 Apply、不排退避重試，等逾時的請求返回（設定路徑恢復）或
/// 下一個時點；explorer 有回應但回錯才是 ApplyError。
#[test]
fn apply_failure_kind_separates_stuck_explorer_from_error_replies() {
    let request = crate::desktop::wallpaper::RequestKind::SetWallpaper;
    assert_eq!(
        apply_failure_kind(&WallpaperError::Timeout {
            request,
            timeout: Duration::from_secs(10)
        }),
        FailureKind::Apply
    );
    assert_eq!(
        apply_failure_kind(&WallpaperError::Busy { request }),
        FailureKind::Busy
    );
    assert_eq!(
        apply_failure_kind(&WallpaperError::Backend {
            request,
            error: BackendError::Com("x".into())
        }),
        FailureKind::ApplyError
    );
    assert_eq!(
        apply_failure_kind(&WallpaperError::Aborted { request }),
        FailureKind::ApplyError
    );
}

#[test]
fn wakes_arriving_during_redraw_are_merged_into_one_evaluation() {
    let env = Env::new("merge", WallpaperTheme::Astrolabe);
    let inbox = Arc::clone(&env.inbox);
    *env.shared.render_hook.lock().unwrap() = Some(Box::new(move |_req| {
        inbox.push(Wake::DataChanged);
        inbox.push(Wake::PauseChanged);
        inbox.push(Wake::DisplayChanged);
    }));
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    // 兩台都只畫一次：期間的喚醒沒有再派發渲染。
    assert_eq!(env.render_count(), 2, "{}", env.log_text());
    // Redraw 之後合併成一次評估（一行「合併」記錄，列出 3 種喚醒、各一次）。
    assert_eq!(
        env.count_logs("Redraw 期間的喚醒合併"),
        1,
        "{}",
        env.log_text()
    );
    let merged = env
        .logs()
        .into_iter()
        .find(|(_, m)| m.contains("Redraw 期間的喚醒合併"))
        .unwrap()
        .1;
    assert!(merged.contains("DataChanged") && merged.contains("PauseChanged"));
    assert_eq!(merged.matches("DataChanged").count(), 1, "{merged}");
}

#[test]
fn path_recovery_during_redraw_is_applied_after_every_monitor_is_recorded() {
    // B 的設定失敗（Apply）；「設定路徑恢復」在渲染 B 時就到了。若在 B 記錄之前套用，B 不會
    // 被排入重試；正確順序（每台記錄之後才套用）→ 同一次 step 內 B 以重試再畫一次。
    let env = Env::new("recover-order", WallpaperTheme::Astrolabe);
    env.desk.lock().unwrap().fail_set.insert(DEV_B.into());
    let mut c = env.coordinator();
    // 第一次渲染 B 時送來「設定路徑恢復」；第二次（重試）之前 explorer 已恢復正常。
    let desk = Arc::clone(&env.desk);
    let key_b2 = env.key(DEV_B, rect_b());
    let mut count = 0;
    let inbox2 = Arc::clone(&env.inbox);
    *env.shared.render_hook.lock().unwrap() = Some(Box::new(move |req| {
        if req.monitor_key == key_b2 {
            count += 1;
            if count == 1 {
                inbox2.push(Wake::ComRecovered);
            } else {
                desk.lock().unwrap().fail_set.clear();
            }
        }
    }));
    env.step(&mut c, &[Wake::Startup]);
    let b_renders = env
        .renders()
        .iter()
        .filter(|r| r.monitor_key == env.key(DEV_B, rect_b()))
        .count();
    assert_eq!(
        b_renders,
        2,
        "B 應在同一次 step 內重試一次\n{}",
        env.log_text()
    );
    assert!(is_host(&env.wallpaper(DEV_B), &env));
    assert!(env.has_log("設定路徑恢復"), "{}", env.log_text());
}

// ---------------------------------------------------------------------------------------------
// 契約 3：Busy 預檢跳過與 warn
// ---------------------------------------------------------------------------------------------

#[test]
fn busy_preflight_skips_render_logs_every_skip_and_warns_once_after_three_slots() {
    let mut env = Env::new("busy", WallpaperTheme::Astrolabe);
    env.service_timeout = Duration::from_millis(40);
    env.desk.lock().unwrap().hang_set = true;
    let mut c = env.coordinator();
    // 第一台設定逾時 → 忙碌；第二台預檢跳過（不渲染）。
    env.step(&mut c, &[Wake::Startup]);
    assert_eq!(env.render_count(), 1, "{}", env.log_text());
    assert_eq!(env.count_logs("預檢跳過"), 1, "{}", env.log_text());
    assert!(c.status().com_busy);

    // 之後三個時點都卡著：每台都跳過、每次都記錄；跨滿 3 個時點時記一次 warn。
    for slot in [15, 30, 45] {
        env.advance_to(taipei(10, slot, 0));
        env.step(&mut c, &[Wake::Timer]);
    }
    assert_eq!(env.render_count(), 1, "忙碌期間不渲染");
    assert_eq!(env.count_logs("預檢跳過"), 1 + 3 * 2, "{}", env.log_text());
    let warns: Vec<_> = env
        .logs()
        .into_iter()
        .filter(|(l, m)| *l == Level::Warn && m.contains("連續"))
        .collect();
    assert_eq!(warns.len(), 1, "{}", env.log_text());

    env.advance_to(taipei(11, 0, 0));
    env.step(&mut c, &[Wake::Timer]);
    let warns = env
        .logs()
        .into_iter()
        .filter(|(l, m)| *l == Level::Warn && m.contains("連續"))
        .count();
    assert_eq!(warns, 1, "同一段卡住只記一次");
    env.gate.release();
}

#[test]
fn busy_clearing_triggers_path_recovered_retry() {
    let mut env = Env::new("busy-recover", WallpaperTheme::Astrolabe);
    env.service_timeout = Duration::from_millis(40);
    env.desk.lock().unwrap().hang_set = true;
    let mut c = env.coordinator();
    let out = env.step(&mut c, &[Wake::Startup]);
    assert!(out.busy, "忙碌時迴圈要縮短等待去查忙碌旗標");
    // explorer 恢復：卡住的請求返回 → 忙碌解除。
    env.desk.lock().unwrap().hang_set = false;
    env.gate.release();
    let deadline = Instant::now() + Duration::from_secs(5);
    while c.service_busy() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    env.step(&mut c, &[Wake::Timer]);
    assert!(env.has_log("設定路徑恢復"), "{}", env.log_text());
    // 兩台都重試成功（A 先前 Apply 失敗、B 預檢跳過）。
    assert!(is_host(&env.wallpaper(DEV_A), &env), "{}", env.log_text());
    assert!(is_host(&env.wallpaper(DEV_B), &env), "{}", env.log_text());
}

// ---------------------------------------------------------------------------------------------
// 契約 4：顯示變更
// ---------------------------------------------------------------------------------------------

#[test]
fn display_change_reenumerates_redraws_new_monitor_and_cleans_stale_outputs() {
    let env = Env::new("display", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    assert_eq!(env.render_count(), 2);

    // 已拔除螢幕留下的輸出檔（沒有任何紀錄）與備份資料夾內的檔。
    let stale = env.paths.output_dir.join("m-00000000000000aa-a.png");
    let stale_tmp = env.paths.output_dir.join("m-00000000000000aa-b.png.tmp");
    fs::write(&stale, b"x").unwrap();
    fs::write(&stale_tmp, b"x").unwrap();
    let unrelated = env.paths.output_dir.join("readme.txt");
    fs::write(&unrelated, b"x").unwrap();

    env.add_monitor_c();
    env.step(&mut c, &[Wake::DisplayChanged]);
    let renders = env.renders();
    assert_eq!(renders.len(), 3, "只有新螢幕重畫\n{}", env.log_text());
    assert_eq!(renders[2].monitor_key, env.key(DEV_C, rect_c()));
    assert!(!stale.exists(), "已拔除螢幕的輸出檔要清掉");
    assert!(!stale_tmp.exists());
    assert!(unrelated.exists(), "不是輸出檔格式的檔不碰");
    // 在線螢幕的輸出檔保留。
    let key_a = env.key(DEV_A, rect_a());
    assert!(env.paths.output_dir.join(format!("{key_a}-a.png")).exists());
    assert!(env.has_log("列舉螢幕"), "{}", env.log_text());
    assert!(env.has_log("待還原"), "{}", env.log_text());
}

/// 清理只看輸出資料夾本身的 `<螢幕鍵>-a/b.png(.tmp)`：子資料夾（原圖備份）、其他檔名、保留的鍵與
/// 被紀錄引用的路徑都不列入。
#[test]
fn stale_output_files_only_lists_unkept_output_files_in_the_folder_itself() {
    let env = Env::new("stale-fn", WallpaperTheme::None);
    let dir = env.paths.output_dir.clone();
    fs::create_dir_all(dir.join("original")).unwrap();
    let names = [
        "m-00000000000000aa-a.png",
        "m-00000000000000aa-b.png.tmp",
        "m-00000000000000bb-a.png",
        "m-00000000000000cc-b.png",
        "m-00000000000000AA-a.png",
        "notes-a.png",
        "m-00000000000000dd.jpg",
    ];
    for n in names {
        fs::write(dir.join(n), b"x").unwrap();
    }
    fs::write(dir.join("original").join("m-00000000000000ee-a.png"), b"x").unwrap();
    let keep_keys: HashSet<String> = ["m-00000000000000bb".to_owned()].into();
    let referenced = dir
        .join("m-00000000000000cc-b.png")
        .to_string_lossy()
        .to_uppercase();
    let stale = stale_output_files(&dir, &keep_keys, &[referenced]);
    let names: Vec<String> = stale
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        vec!["m-00000000000000aa-a.png", "m-00000000000000aa-b.png.tmp"]
    );
}

#[test]
fn output_files_of_offline_monitor_with_a_record_are_kept() {
    let env = Env::new("display-offline", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    let key_b = env.key(DEV_B, rect_b());
    let b_out = env.paths.output_dir.join(format!("{key_b}-a.png"));
    assert!(b_out.exists());
    // B 拔除（離線但仍被列舉）：它的紀錄還在，輸出檔可能仍是它正在顯示的圖。
    env.desk.lock().unwrap().monitors[1].rect = None;
    env.shared.displays.lock().unwrap().truncate(1);
    env.step(&mut c, &[Wake::DisplayChanged]);
    assert!(b_out.exists(), "有紀錄的離線螢幕輸出檔要保留");
}

#[test]
fn dpi_change_is_detected_from_display_list_and_redraws_that_monitor() {
    let env = Env::new("dpi", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    env.shared.displays.lock().unwrap()[1].dpi = 168;
    env.step(&mut c, &[Wake::DpiChanged]);
    let renders = env.renders();
    assert_eq!(renders.len(), 3, "{}", env.log_text());
    assert_eq!(renders[2].monitor_key, env.key(DEV_B, rect_b()));
}

// ---------------------------------------------------------------------------------------------
// 契約 5：時區或系統時間變更
// ---------------------------------------------------------------------------------------------

#[test]
fn clock_set_backwards_clears_slot_records_so_current_slot_is_drawn() {
    let env = Env::new("clock-back", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    assert_eq!(env.render_count(), 2);
    // 系統時間被調回 08:20（單調時鐘照走 5 秒）：紀錄是 10:00 時點，嚴格較新比較會讓 08:15
    // 一直「不夠新」，不清紀錄就要等兩個小時。
    {
        let mut clk = env.shared.clock.lock().unwrap();
        clk.wall = taipei(8, 20, 0);
        clk.mono += Duration::from_secs(5);
    }
    env.step(&mut c, &[Wake::TimeChanged]);
    let renders = env.renders();
    assert_eq!(renders.len(), 4, "{}", env.log_text());
    assert_eq!(renders[3].as_of, taipei(8, 15, 0));
    assert!(env.has_log("時點紀錄"), "{}", env.log_text());
}

#[test]
fn clock_step_backwards_is_detected_without_an_event() {
    let env = Env::new("clock-back-silent", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    {
        let mut clk = env.shared.clock.lock().unwrap();
        clk.wall = taipei(8, 20, 0);
        clk.mono += Duration::from_secs(60);
    }
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(env.render_count(), 4, "{}", env.log_text());
}

#[test]
fn timezone_change_clears_slot_records() {
    let env = Env::new("tz", WallpaperTheme::Tearoff);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    assert_eq!(env.render_count(), 2);
    // 改到 UTC-10：本機日期變成 10/4，比紀錄的 10/5 舊；不清紀錄要等到明天。
    env.shared.tz.store(-10 * 3600, Ordering::SeqCst);
    env.step(&mut c, &[Wake::TimeChanged]);
    assert_eq!(env.render_count(), 4, "{}", env.log_text());
    assert!(env.has_log("時區"), "{}", env.log_text());
}

#[test]
fn small_forward_clock_adjustment_does_not_clear_records() {
    let env = Env::new("clock-fwd", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    {
        let mut clk = env.shared.clock.lock().unwrap();
        clk.wall += 30;
        clk.mono += Duration::from_secs(1);
    }
    env.step(&mut c, &[Wake::TimeChanged]);
    assert_eq!(env.render_count(), 2, "{}", env.log_text());
}

// ---------------------------------------------------------------------------------------------
// 契約 6：設定變更（主題切換、還原等 Redraw 結束、主題設定檔）
// ---------------------------------------------------------------------------------------------

#[test]
fn switching_to_none_restores_original_wallpapers_and_resets_scheduler() {
    let env = Env::new("to-none", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    assert!(is_host(&env.wallpaper(DEV_A), &env));
    let generation = c.scheduler_generation();

    env.set_theme(WallpaperTheme::None);
    env.step(&mut c, &[Wake::SettingsChanged]);
    assert_eq!(
        env.wallpaper(DEV_A).as_deref(),
        Some(env.orig("a").as_str()),
        "{}",
        env.log_text()
    );
    assert_eq!(
        env.wallpaper(DEV_B).as_deref(),
        Some(env.orig("b").as_str())
    );
    assert_eq!(env.state_json()["status"], "not_taken_over");
    assert!(c.scheduler_generation() > generation, "還原後 reset");
    assert_eq!(c.status().state, CoordinatorState::Idle);
    assert!(env.has_log("還原"), "{}", env.log_text());
}

#[test]
fn restore_waits_for_running_redraw_and_unsent_monitors_are_not_set() {
    let env = Env::new("restore-wait", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    let sets_before = env.set_calls().len();
    assert_eq!(sets_before, 2);

    // 10:15 的 Redraw 中，渲染第一台時使用者切到「不接管」。
    let shared = Arc::clone(&env.shared);
    let inbox = Arc::clone(&env.inbox);
    let mut fired = false;
    *env.shared.render_hook.lock().unwrap() = Some(Box::new(move |_req| {
        if !fired {
            fired = true;
            shared.inputs.lock().unwrap().theme = WallpaperTheme::None;
            inbox.push(Wake::SettingsChanged);
        }
    }));
    env.advance_to(taipei(10, 15, 0));
    env.step(&mut c, &[Wake::Timer]);

    assert_eq!(env.render_count(), 3, "第二台不再渲染\n{}", env.log_text());
    // 渲染完的那一台也不送 SetWallpaper（還原排在 Redraw 之後，送了會蓋掉還原）。
    let host_sets_after: Vec<_> = env.set_calls()[sets_before..]
        .iter()
        .filter(|c| c.contains(env.paths.output_dir.to_str().unwrap()))
        .cloned()
        .collect();
    assert!(host_sets_after.is_empty(), "{host_sets_after:?}");
    assert_eq!(
        env.wallpaper(DEV_A).as_deref(),
        Some(env.orig("a").as_str())
    );
    assert_eq!(
        env.wallpaper(DEV_B).as_deref(),
        Some(env.orig("b").as_str())
    );
    let text = env.log_text();
    let abort = text.find("Redraw 中止").expect(&text);
    let restore = text.find("還原原桌布").expect(&text);
    assert!(abort < restore, "還原在 Redraw 結束之後\n{text}");
}

#[test]
fn theme_config_change_redraws_current_theme() {
    let env = Env::new("config", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    assert_eq!(env.render_count(), 2);
    // 資料通道更新但主題設定沒變：不重畫。
    env.step(&mut c, &[Wake::DataChanged]);
    assert_eq!(env.render_count(), 2);
    env.shared.inputs.lock().unwrap().config_version = Some(1);
    env.step(&mut c, &[Wake::DataChanged]);
    assert_eq!(env.render_count(), 4, "{}", env.log_text());
    assert!(env.has_log("主題設定檔"), "{}", env.log_text());
}

#[test]
fn yield_applies_theme_none_and_notifies() {
    let env = Env::new("yield", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    // 使用者自己把 A 換成別的圖。
    env.desk.lock().unwrap().monitors[0].wallpaper =
        Some("C:\\Users\\u\\Pictures\\mine.jpg".into());
    env.advance_to(taipei(10, 15, 0));
    env.step(&mut c, &[Wake::Timer]);
    let effects = env.shared.effects.lock().unwrap().clone();
    assert!(
        effects
            .iter()
            .any(|e| e.contains("theme_none UserChangedWallpaper")),
        "{effects:?}\n{}",
        env.log_text()
    );
    assert!(
        effects.iter().any(|e| e.starts_with("notify")),
        "{effects:?}"
    );
    assert_eq!(c.status().state, CoordinatorState::Idle);
    // 讓位之後不再設定。
    env.advance_to(taipei(10, 30, 0));
    let sets = env.set_calls().len();
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(env.set_calls().len(), sets);
}

// ---------------------------------------------------------------------------------------------
// 契約 7：暫停（PauseRules、最大化不暫停、勿打擾介面）
// ---------------------------------------------------------------------------------------------

#[test]
fn pause_reasons_pause_and_resume_redraws_missed_slot_once() {
    let env = Env::new("pause", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    env.shared
        .inputs
        .lock()
        .unwrap()
        .pause
        .insert(PauseReason::Locked);
    let out = env.step(&mut c, &[Wake::PauseChanged]);
    assert_eq!(c.status().state, CoordinatorState::Paused);
    assert_eq!(out.next_wake, None);
    env.advance_to(taipei(11, 20, 0));
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(env.render_count(), 2, "暫停中不重畫");
    env.shared.inputs.lock().unwrap().pause.clear();
    env.step(&mut c, &[Wake::PauseChanged]);
    let renders = env.renders();
    assert_eq!(renders.len(), 4);
    assert_eq!(renders[3].as_of, taipei(11, 15, 0), "補畫最近的時點");
}

#[test]
fn maximized_ordinary_window_does_not_pause() {
    use windows::Win32::UI::Shell::{QUNS_ACCEPTS_NOTIFICATIONS, QUNS_BUSY};
    // 一般視窗最大化時 QUNS 仍是「接受通知」，宿主不產生任何暫停原因；全螢幕才是 QUNS_BUSY。
    assert!(!crate::desktop::quns_indicates_busy(
        QUNS_ACCEPTS_NOTIFICATIONS
    ));
    assert!(crate::desktop::quns_indicates_busy(QUNS_BUSY));

    let env = Env::new("maximized", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    env.advance_to(taipei(10, 15, 0));
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(env.render_count(), 4, "沒有暫停原因 → 照常重畫");

    // 電池規則關閉：使用電池也不會出現 Battery 原因（PauseRules 由 widgets 套用）。
    let rules = crate::settings::PauseRules {
        pause_on_battery: false,
        ..crate::settings::PauseRules::default()
    };
    let raw = crate::widgets::RawPowerSignals {
        on_battery: true,
        power_saver: false,
    };
    assert!(!crate::widgets::battery_pause_active(raw, rules));
}

#[test]
fn do_not_disturb_hook_blocks_redraw_until_cleared() {
    let env = Env::new("dnd", WallpaperTheme::Astrolabe);
    env.shared.dnd.store(true, Ordering::SeqCst);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    assert_eq!(env.render_count(), 0);
    assert_eq!(c.status().state, CoordinatorState::DoNotDisturb);
    env.shared.dnd.store(false, Ordering::SeqCst);
    env.step(&mut c, &[Wake::DoNotDisturbChanged]);
    assert_eq!(env.render_count(), 2);
}

/// 4.7c：勿打擾跨過兩個時點 → 期間不渲染、狀態為 DoNotDisturb、不排計時器；解除的喚醒立即補畫最近
/// 錯過的時點一次，之後的喚醒不再重畫。
#[test]
fn do_not_disturb_spanning_slots_redraws_the_latest_missed_slot_once_when_cleared() {
    let env = Env::new("dnd-missed", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    assert_eq!(env.render_count(), 2, "啟動時兩台都畫 10:00");

    env.shared.dnd.store(true, Ordering::SeqCst);
    env.advance_to(taipei(10, 15, 0));
    let out = env.step(&mut c, &[Wake::Timer]);
    assert_eq!(env.render_count(), 2, "勿打擾中不渲染");
    assert_eq!(c.status().state, CoordinatorState::DoNotDisturb);
    assert_eq!(
        out.next_wake, None,
        "勿打擾中不排計時器（靠解除的喚醒與等待上限）"
    );
    assert!(env.set_calls().len() <= 2, "勿打擾中不設定桌布");

    env.advance_to(taipei(11, 20, 0));
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(env.render_count(), 2, "跨過第二個時點仍不渲染");
    assert_eq!(c.status().state, CoordinatorState::DoNotDisturb);

    env.shared.dnd.store(false, Ordering::SeqCst);
    let out = env.step(&mut c, &[Wake::DoNotDisturbChanged]);
    let renders = env.renders();
    assert_eq!(renders.len(), 4, "解除後立即補畫一次（兩台）");
    assert_eq!(renders[2].as_of, taipei(11, 15, 0), "補畫最近的時點");
    assert_eq!(renders[3].as_of, taipei(11, 15, 0));
    assert_eq!(out.next_wake, Some(taipei(11, 30, 0)));
    assert_eq!(c.status().state, CoordinatorState::UpToDate);

    env.step(&mut c, &[Wake::DoNotDisturbChanged]);
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(env.render_count(), 4, "只補一次");
}

/// 4.7c：勿打擾在兩個時點之間開始又解除 → 沒有錯過時點，解除後不提前畫；時點到了照常畫。
#[test]
fn do_not_disturb_between_slots_does_not_draw_early_when_cleared() {
    let env = Env::new("dnd-between", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    assert_eq!(env.render_count(), 2);

    env.advance_to(taipei(10, 8, 0));
    env.shared.dnd.store(true, Ordering::SeqCst);
    env.step(&mut c, &[Wake::DoNotDisturbChanged]);
    assert_eq!(env.render_count(), 2);
    assert_eq!(
        c.status().state,
        CoordinatorState::UpToDate,
        "沒有要重畫的時點：勿打擾不影響狀態"
    );

    env.advance_to(taipei(10, 12, 0));
    env.shared.dnd.store(false, Ordering::SeqCst);
    let out = env.step(&mut c, &[Wake::DoNotDisturbChanged]);
    assert_eq!(env.render_count(), 2, "解除時沒有錯過時點：不提前畫");
    assert_eq!(out.next_wake, Some(taipei(10, 15, 0)));

    env.advance_to(taipei(10, 15, 0));
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(env.render_count(), 4, "時點到了照常畫");
    assert_eq!(env.renders()[2].as_of, taipei(10, 15, 0));
}

/// 4.7c：主題為「不接管」時協調迴圈從不查勿打擾（正式實作因此不建立輪詢執行緒、不讀任何來源），
/// 勿打擾喚醒也不碰 COM 與螢幕列舉。
#[test]
fn theme_none_never_queries_do_not_disturb() {
    let env = Env::new("dnd-none", WallpaperTheme::None);
    env.shared.dnd.store(true, Ordering::SeqCst);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    env.step(&mut c, &[Wake::DoNotDisturbChanged]);
    env.advance_to(taipei(11, 0, 0));
    env.step(&mut c, &[Wake::Timer, Wake::DoNotDisturbChanged]);
    assert_eq!(env.shared.dnd_calls.load(Ordering::SeqCst), 0);
    assert_eq!(env.shared.displays_calls.load(Ordering::SeqCst), 0);
    assert_eq!(env.render_count(), 0);
    assert_eq!(c.status().state, CoordinatorState::Idle);
}

// ---------------------------------------------------------------------------------------------
// 契約 8：blocked 不渲染；StateUnavailable 的還原重試
// ---------------------------------------------------------------------------------------------

#[test]
fn blocked_state_file_never_schedules_rendering() {
    let env = Env::new("blocked", WallpaperTheme::Astrolabe);
    fs::write(&env.paths.state_file, b"{ not json").unwrap();
    let mut c = env.coordinator();
    let out = env.step(&mut c, &[Wake::Startup]);
    assert_eq!(env.render_count(), 0);
    assert!(env.set_calls().is_empty());
    assert!(matches!(c.status().state, CoordinatorState::Blocked(_)));
    assert_eq!(out.next_wake, None);
    env.advance_to(taipei(10, 15, 0));
    env.step(&mut c, &[Wake::Timer, Wake::DataChanged]);
    assert_eq!(env.render_count(), 0);
    assert!(env
        .logs()
        .iter()
        .any(|(l, m)| *l == Level::Error && m.contains("封鎖")));
}

#[test]
fn state_unavailable_restore_is_retried_with_backoff_until_it_succeeds() {
    let env = Env::new("unavailable", WallpaperTheme::None);
    // 狀態檔路徑是資料夾：讀檔失敗但不是「不存在」＝暫時讀不到。
    fs::create_dir_all(&env.paths.state_file).unwrap();
    let mut c = env.coordinator();
    let t0 = env.shared.clock.lock().unwrap().wall;
    let out = env.step(&mut c, &[Wake::Startup]);
    assert_eq!(out.next_wake, Some(t0 + 120), "{}", env.log_text());
    // 4.7b 修正輪 2（審查 F1）：啟動時讀不到＝啟動判定延後（不先還原），依同一套退避重讀。
    assert_eq!(c.status().state, CoordinatorState::WaitingForStateFile);

    env.advance_to(t0 + 120);
    let out = env.step(&mut c, &[Wake::Timer]);
    assert_eq!(out.next_wake, Some(t0 + 120 + 240), "第二次失敗間隔加倍");

    // 恢復：檔案讀得到了。
    fs::remove_dir_all(&env.paths.state_file).unwrap();
    env.advance_to(t0 + 360);
    let out = env.step(&mut c, &[Wake::Timer]);
    assert_eq!(out.next_wake, None, "{}", env.log_text());
    assert_eq!(c.status().restore_retry_at, None);
    assert_eq!(env.count_logs("暫時讀不到"), 2, "{}", env.log_text());
}

// ---------------------------------------------------------------------------------------------
// 契約 9：Secondary
// ---------------------------------------------------------------------------------------------

#[test]
fn secondary_only_reloads_config_and_runs_no_coordinator() {
    use crate::desktop::StartupRole;
    let secondary = startup_plan(StartupRole::Secondary);
    assert!(!secondary.write_config, "Secondary 不寫主題設定檔");
    assert!(!secondary.run_coordinator, "Secondary 不跑協調執行緒");
    for role in [StartupRole::Primary, StartupRole::Unarbitrated] {
        let p = startup_plan(role);
        assert!(p.write_config && p.run_coordinator, "{role:?}");
    }
}

#[test]
fn secondary_config_load_never_writes_the_default_file() {
    let env = Env::new("secondary-config", WallpaperTheme::None);
    let path = env.dir.join(crate::wallpaper_config::CONFIG_FILE_NAME);
    let cfg = load_theme_config(startup_plan(crate::desktop::StartupRole::Secondary), &path);
    assert!(!path.exists(), "Secondary 不得寫檔");
    assert_eq!(
        cfg.source,
        crate::wallpaper_config::ConfigSource::MissingDefault
    );
    let cfg = load_theme_config(startup_plan(crate::desktop::StartupRole::Primary), &path);
    assert!(path.exists(), "Primary 缺檔時寫出預設檔");
    assert_eq!(
        cfg.source,
        crate::wallpaper_config::ConfigSource::CreatedDefault
    );
}

// ---------------------------------------------------------------------------------------------
// 契約 10：首次無資料
// ---------------------------------------------------------------------------------------------

#[test]
fn waiting_for_data_is_queryable_and_does_not_take_over() {
    let env = Env::new("waiting", WallpaperTheme::Ridgeline);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    assert_eq!(
        c.status().state,
        CoordinatorState::WaitingForData(DataKey::TwiiIntraday)
    );
    assert_eq!(env.render_count(), 0);
    assert!(
        !env.paths.state_file.exists(),
        "沒有資料就不接管、不寫狀態檔"
    );
    env.shared.inputs.lock().unwrap().data.twii_intraday = CivilDate::parse_iso("2026-10-02");
    env.step(&mut c, &[Wake::DataChanged]);
    assert_eq!(env.render_count(), 2);
    assert_eq!(env.state_json()["status"], "taken_over");
}

#[test]
fn data_dates_are_extracted_from_tw_events() {
    let v = serde_json::json!({
        "twii_intraday": { "date": "2026-10-02", "points": [] },
        "twii_daily": [ { "date": "2026-09-30" }, { "date": "2026-10-01" }, { "date": "bad" } ],
        "margin": { "date": "2026-10-01" }
    });
    let d = data_dates(&v);
    assert_eq!(d.twii_intraday, CivilDate::parse_iso("2026-10-02"));
    assert_eq!(d.twii_daily, CivilDate::parse_iso("2026-10-01"));
    assert_eq!(d.margin, CivilDate::parse_iso("2026-10-01"));
    assert_eq!(data_dates(&serde_json::json!({})), DataDates::default());
    assert_eq!(
        data_dates(&serde_json::json!({ "twii_daily": [] })),
        DataDates::default()
    );
}

// ---------------------------------------------------------------------------------------------
// 契約 11：記錄
// ---------------------------------------------------------------------------------------------

#[test]
fn every_event_decision_and_api_call_is_logged() {
    let env = Env::new("logs", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    env.step(&mut c, &[Wake::PauseChanged]);
    let text = env.log_text();
    for needle in [
        "喚醒：Startup",
        "喚醒：PauseChanged",
        "決策：Redraw",
        "決策：UpToDate",
        "列舉螢幕",
        "SetWallpaper",
        "Applied",
        "確認已套用",
    ] {
        assert!(text.contains(needle), "缺「{needle}」\n{text}");
    }
    // 純計時器喚醒且決策沒變：不洗版。
    let before = env.logs().len();
    env.advance_to(taipei(10, 6, 0));
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(env.logs().len(), before, "{}", env.log_text());
}

// ---------------------------------------------------------------------------------------------
// 協調執行緒（handle：通知不阻塞、關閉）
// ---------------------------------------------------------------------------------------------

#[test]
fn handle_notify_never_blocks_while_coordinator_is_rendering_and_shutdown_stops_loop() {
    let env = Env::new("thread", WallpaperTheme::Astrolabe);
    let started = Arc::new((Mutex::new(false), Condvar::new()));
    let started2 = Arc::clone(&started);
    *env.shared.render_hook.lock().unwrap() = Some(Box::new(move |_req| {
        {
            let (m, cv) = &*started2;
            *m.lock().unwrap() = true;
            cv.notify_all();
        }
        std::thread::sleep(Duration::from_millis(300));
    }));
    let c = env.coordinator();
    let (handle, join) = spawn_coordinator(move || Ok(c)).unwrap();
    {
        let (m, cv) = &*started;
        let mut g = m.lock().unwrap();
        while !*g {
            g = cv.wait(g).unwrap();
        }
    }
    let t = Instant::now();
    handle.notify(Wake::DataChanged);
    handle.notify(Wake::SettingsChanged);
    let _ = handle.status();
    assert!(
        t.elapsed() < Duration::from_millis(100),
        "notify／status 不得等協調迴圈"
    );
    handle.shutdown();
    join.join().unwrap();
    // 關閉要求在第一台渲染期間到達：渲染完不設定、第二台不渲染，迴圈結束。
    assert_eq!(env.render_count(), 1, "{}", env.log_text());
    assert!(env.set_calls().is_empty(), "{:?}", env.set_calls());
    assert!(env.has_log("宿主結束"), "{}", env.log_text());
}

// ---------------------------------------------------------------------------------------------
// 修正輪 1（審查 task-4.7a-review.md）
// ---------------------------------------------------------------------------------------------

fn wait_until_not_busy(c: &Coordinator<FakePorts>) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while c.service_busy() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!c.service_busy(), "COM 執行緒應已解除忙碌");
}

/// [medium 1] COM 忙碌時切「不接管」→ 還原 Failed → 留在協調迴圈；忙碌解除（設定路徑恢復）時重試成功。
#[test]
fn restore_failed_while_com_busy_is_retried_after_recovery() {
    let mut env = Env::new("restore-busy", WallpaperTheme::Astrolabe);
    env.service_timeout = Duration::from_millis(40);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    assert!(is_host(&env.wallpaper(DEV_A), &env));

    // 10:15：explorer 卡住，第一台設定逾時 → 忙碌。
    env.desk.lock().unwrap().hang_set = true;
    env.advance_to(taipei(10, 15, 0));
    env.step(&mut c, &[Wake::Timer]);
    assert!(c.service_busy());

    env.set_theme(WallpaperTheme::None);
    env.step(&mut c, &[Wake::SettingsChanged]);
    assert_eq!(env.state_json()["status"], "taken_over", "忙碌時還原失敗");
    assert!(c.status().restore_retry_at.is_some(), "{}", env.log_text());
    assert!(env.has_log("還原失敗"), "{}", env.log_text());

    // explorer 恢復：卡住的請求返回、忙碌解除 → 下一次喚醒重試還原。
    env.desk.lock().unwrap().hang_set = false;
    env.gate.release();
    wait_until_not_busy(&c);
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(
        env.wallpaper(DEV_A).as_deref(),
        Some(env.orig("a").as_str()),
        "{}",
        env.log_text()
    );
    assert_eq!(
        env.wallpaper(DEV_B).as_deref(),
        Some(env.orig("b").as_str())
    );
    assert_eq!(env.state_json()["status"], "not_taken_over");
    assert_eq!(c.status().restore_retry_at, None);
}

/// [medium 1] 還原一再失敗：依 retry_delay（2、4 分鐘…）重試，每次都記錄，成功後停止。
#[test]
fn failed_restore_is_retried_on_retry_delay_backoff_until_restored() {
    let env = Env::new("restore-backoff", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    env.desk.lock().unwrap().fail_read = true;
    env.set_theme(WallpaperTheme::None);
    let t0 = env.shared.clock.lock().unwrap().wall;
    let out = env.step(&mut c, &[Wake::SettingsChanged]);
    assert_eq!(out.next_wake, Some(t0 + 120), "{}", env.log_text());

    env.advance_to(t0 + 120);
    let out = env.step(&mut c, &[Wake::Timer]);
    assert_eq!(out.next_wake, Some(t0 + 120 + 240), "{}", env.log_text());
    // 其他喚醒（不是恢復事件）不會提早重試。
    env.advance_to(t0 + 200);
    env.step(&mut c, &[Wake::DataChanged]);
    assert_eq!(env.count_logs("還原原桌布：原因"), 2, "{}", env.log_text());

    env.desk.lock().unwrap().fail_read = false;
    env.advance_to(t0 + 360);
    let out = env.step(&mut c, &[Wake::Timer]);
    assert_eq!(out.next_wake, None, "{}", env.log_text());
    assert_eq!(env.count_logs("還原原桌布：原因"), 3);
    assert_eq!(env.count_logs("還原失敗"), 2);
    assert_eq!(
        env.wallpaper(DEV_A).as_deref(),
        Some(env.orig("a").as_str())
    );
    assert_eq!(c.status().restore_retry_at, None);
}

/// [medium 2] 原桌布是 Windows 焦點：第一台渲染後才知道要等確認；之後各時點、恢復事件都不再渲染，
/// 直到使用者確認。
#[test]
fn no_render_while_awaiting_spotlight_confirmation() {
    use crate::desktop::wallpaper::registry::SPOTLIGHT_KEY;
    let env = Env::new("spotlight", WallpaperTheme::Astrolabe);
    let RegValue::Present { kind, data } = RegValue::dword(1) else {
        unreachable!()
    };
    env.registry_values.lock().unwrap().insert(
        (SPOTLIGHT_KEY.to_owned(), "EnabledState".to_owned()),
        (kind, data),
    );
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    assert_eq!(env.render_count(), 1, "{}", env.log_text());
    assert!(c.status().awaiting_spotlight_confirmation);
    assert_eq!(
        c.status().state,
        CoordinatorState::AwaitingSpotlightConfirmation
    );
    for slot in [15, 30] {
        env.advance_to(taipei(10, slot, 0));
        env.step(&mut c, &[Wake::Timer]);
    }
    env.step(&mut c, &[Wake::ExplorerRestarted]);
    env.step(&mut c, &[Wake::ComRecovered]);
    assert_eq!(
        env.render_count(),
        1,
        "等確認期間不渲染\n{}",
        env.log_text()
    );
    assert!(env.set_calls().is_empty());

    env.step(&mut c, &[Wake::SpotlightConfirmed]);
    assert_eq!(env.render_count(), 3, "{}", env.log_text());
    assert!(is_host(&env.wallpaper(DEV_A), &env));
    assert!(is_host(&env.wallpaper(DEV_B), &env));
}

fn put_spotlight_on(env: &Env) {
    use crate::desktop::wallpaper::registry::SPOTLIGHT_KEY;
    let RegValue::Present { kind, data } = RegValue::dword(1) else {
        unreachable!()
    };
    env.registry_values.lock().unwrap().insert(
        (SPOTLIGHT_KEY.to_owned(), "EnabledState".to_owned()),
        (kind, data),
    );
}

/// 4.8：等焦點確認時發一次系統匣提示（設定視窗可能沒開）；之後的喚醒不重複。
#[test]
fn spotlight_pending_prompts_once_until_answered() {
    let env = Env::new("spotlight-pending-once", WallpaperTheme::Astrolabe);
    put_spotlight_on(&env);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    assert!(c.status().awaiting_spotlight_confirmation);
    for slot in [15, 30] {
        env.advance_to(taipei(10, slot, 0));
        env.step(&mut c, &[Wake::Timer]);
    }
    env.step(&mut c, &[Wake::DisplayChanged]);
    assert_eq!(
        env.count_effects("spotlight_pending"),
        1,
        "{:?}",
        env.effects()
    );
}

/// 4.8：設定視窗在選主題時先問並事先確認（[`Coordinator::spotlight_preconfirm`]，主題還沒存檔時就登記）；
/// 之後主題改成某個主題，第一次設定直接接管，不再進入「等確認」、不發提示。
#[test]
fn spotlight_preconfirmed_by_settings_takes_over_without_waiting() {
    let env = Env::new("spotlight-preconfirm", WallpaperTheme::None);
    put_spotlight_on(&env);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    assert!(!c.status().taken_over);
    assert!(!c.status().spotlight_confirmed);

    c.spotlight_preconfirm()
        .set(Some(WallpaperTheme::Astrolabe));
    // 主題還沒被看到（存檔前）的評估：事先確認先保留，不套用。
    env.step(&mut c, &[Wake::Timer]);
    assert!(!c.status().spotlight_confirmed);
    env.set_theme(WallpaperTheme::Astrolabe);
    env.step(&mut c, &[Wake::SettingsChanged]);
    assert_eq!(env.render_count(), 2, "{}", env.log_text());
    assert!(is_host(&env.wallpaper(DEV_A), &env));
    assert!(is_host(&env.wallpaper(DEV_B), &env));
    assert!(!c.status().awaiting_spotlight_confirmation);
    assert!(c.status().taken_over, "接管後要發佈給設定視窗");
    assert_eq!(c.spotlight_preconfirm().get(), None, "套用後清除");
    assert_eq!(env.count_effects("spotlight_pending"), 0);
}

/// 4.8：事先確認、主題變更與（後備路徑的）確認喚醒落在同一次評估也一樣直接接管。
#[test]
fn spotlight_preconfirm_and_theme_change_in_same_step() {
    let env = Env::new("spotlight-preconfirm-same", WallpaperTheme::None);
    put_spotlight_on(&env);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    c.spotlight_preconfirm()
        .set(Some(WallpaperTheme::Astrolabe));
    env.set_theme(WallpaperTheme::Astrolabe);
    env.step(&mut c, &[Wake::SpotlightConfirmed, Wake::SettingsChanged]);
    assert_eq!(env.render_count(), 2, "{}", env.log_text());
    assert!(c.status().taken_over);
    assert_eq!(env.count_effects("spotlight_pending"), 0);
}

/// 4.8 修正輪 1（審查 L5(a)、controller 第 3 點）：協調迴圈正在等主題 A 的焦點確認，使用者在設定改選 B 並
/// 確認。確認先到、主題變更後到（舊做法的競態）時，**不得**先用 A 畫一次；確認後第一次畫的就是 B。
#[test]
fn preconfirm_for_new_theme_never_draws_the_old_theme_first() {
    let env = Env::new("spotlight-preconfirm-a-to-b", WallpaperTheme::Astrolabe);
    put_spotlight_on(&env);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    assert!(c.status().awaiting_spotlight_confirmation);
    let before = env.render_count();

    c.spotlight_preconfirm().set(Some(WallpaperTheme::Tearoff));
    // 確認的喚醒先被處理、主題變更還沒被看到：不得把 A 的等待當成已確認。
    env.step(&mut c, &[Wake::SpotlightConfirmed]);
    assert_eq!(
        env.render_count(),
        before,
        "主題仍是 A 時不得因確認而渲染 A\n{}",
        env.log_text()
    );
    assert!(env.set_calls().is_empty(), "A 不得被設成桌布");

    env.set_theme(WallpaperTheme::Tearoff);
    env.step(&mut c, &[Wake::SettingsChanged]);
    let after: Vec<WallpaperTheme> = env.renders()[before..].iter().map(|r| r.theme).collect();
    assert_eq!(
        after,
        vec![WallpaperTheme::Tearoff, WallpaperTheme::Tearoff],
        "{}",
        env.log_text()
    );
    assert!(c.status().taken_over);
    assert!(!c.status().awaiting_spotlight_confirmation);
}

/// 4.8 修正輪 2（複審 low）：確認 A 之後、迴圈還沒看到 A 就又選了不需要確認的主題，舊登記必須被蓋掉；否則
/// 之後另一個主題進入等確認時，後備路徑的確認會因為「另有其他主題的事先確認」而一直被忽略。整段經
/// `wallpaper_settings::select_theme_core`（設定視窗的選主題本體）操作登記與主題。
#[test]
fn stale_preconfirm_is_overwritten_by_a_later_plain_selection() {
    use crate::settings::Settings;
    use crate::wallpaper_settings::select_theme_core;

    let env = Env::new("stale-preconfirm", WallpaperTheme::None);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    let cell = c.spotlight_preconfirm();
    let select = |theme: WallpaperTheme, answer: Option<bool>, needs: bool| {
        select_theme_core(
            theme,
            answer,
            needs,
            false,
            |t| cell.set(t),
            |patch| {
                let t: WallpaperTheme =
                    serde_json::from_value(patch["wallpaper_theme"].clone()).unwrap();
                env.set_theme(t);
                Ok(Settings {
                    wallpaper_theme: t,
                    ..Settings::default()
                })
            },
            Settings::default,
        )
        .unwrap()
    };

    // 確認 A（星盤），迴圈還沒處理就改回「不接管」（不需要問）。
    select(WallpaperTheme::Astrolabe, Some(true), true);
    select(WallpaperTheme::None, None, false);
    env.step(&mut c, &[Wake::SettingsChanged]);
    assert_eq!(cell.get(), None, "舊登記要被蓋掉");

    // 之後選撕日曆時原桌布還不是焦點（不問），第一次設定前才變成焦點 → 迴圈進入等確認。
    select(WallpaperTheme::Tearoff, None, false);
    put_spotlight_on(&env);
    env.step(&mut c, &[Wake::SettingsChanged]);
    assert!(
        c.status().awaiting_spotlight_confirmation,
        "{}",
        env.log_text()
    );

    // 後備對話確認：必須生效。
    env.step(&mut c, &[Wake::SpotlightConfirmed]);
    assert!(
        !c.status().awaiting_spotlight_confirmation,
        "後備確認被忽略\n{}",
        env.log_text()
    );
    assert!(c.status().taken_over, "{}", env.log_text());
}

/// 4.8 修正輪 2（複審 nit）：`CoordinatorStatus::notices` 只保留最近 [`MAX_STATUS_NOTICES`] 筆。
#[test]
fn status_keeps_only_the_most_recent_notices() {
    let env = Env::new("notices-cap", WallpaperTheme::None);
    let mut c = env.coordinator();
    let total = MAX_STATUS_NOTICES + 4;
    for i in 0..total {
        c.apply_events(vec![TakeoverEvent::Notify(TakeoverNotice::Yielded {
            device_paths: vec![format!("DEV{i}")],
        })]);
    }
    let notices = c.status().notices;
    assert_eq!(notices.len(), MAX_STATUS_NOTICES);
    assert_eq!(
        notices.first(),
        Some(&TakeoverNotice::Yielded {
            device_paths: vec![format!("DEV{}", total - MAX_STATUS_NOTICES)]
        }),
        "丟掉的是最舊的"
    );
    assert_eq!(
        notices.last(),
        Some(&TakeoverNotice::Yielded {
            device_paths: vec![format!("DEV{}", total - 1)]
        })
    );
    assert_eq!(env.count_effects("notify"), total, "每一筆仍各通知一次");
}

/// 4.8 修正輪 1（審查 L2）：等確認中主題改為「不接管」，發佈的等確認旗標要跟著變 false；之後再選主題又進入
/// 等確認時，系統匣提示要再發一次。
#[test]
fn awaiting_flag_follows_phase_and_prompt_repeats_for_a_new_wait() {
    let env = Env::new("spotlight-awaiting-none", WallpaperTheme::Astrolabe);
    put_spotlight_on(&env);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    assert!(c.status().awaiting_spotlight_confirmation);
    assert_eq!(env.count_effects("spotlight_pending"), 1);

    env.set_theme(WallpaperTheme::None);
    env.step(&mut c, &[Wake::SettingsChanged]);
    assert!(
        !c.status().awaiting_spotlight_confirmation,
        "改回「不接管」就不再等確認\n{}",
        env.log_text()
    );
    // 取消（設定視窗存成「不接管」）之後：不渲染、不再發系統匣提示（修正輪 1，審查 medium）。
    let renders = env.render_count();
    for slot in [15, 30] {
        env.advance_to(taipei(10, slot, 0));
        env.step(&mut c, &[Wake::Timer]);
    }
    assert_eq!(env.render_count(), renders);
    assert_eq!(env.count_effects("spotlight_pending"), 1);

    env.set_theme(WallpaperTheme::Astrolabe);
    env.step(&mut c, &[Wake::SettingsChanged]);
    assert!(c.status().awaiting_spotlight_confirmation);
    assert_eq!(
        env.count_effects("spotlight_pending"),
        2,
        "新的一段等待再提示一次\n{}",
        env.log_text()
    );
}

/// [low 3] COM 長時間忙碌時不反覆列舉、不洗版；忙碌解除後才列舉。不忙碌時的列舉失敗只在第一次與
/// 錯誤改變時記錄。
#[test]
fn monitor_listing_is_deferred_while_busy_and_failures_are_not_repeated() {
    let mut env = Env::new("list-busy", WallpaperTheme::Astrolabe);
    env.service_timeout = Duration::from_millis(40);
    env.desk.lock().unwrap().hang_set = true;
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    assert!(c.service_busy());
    let lists = |env: &Env| {
        env.calls()
            .iter()
            .filter(|c| c.as_str() == "list_monitors")
            .count()
    };
    let before = lists(&env);
    env.step(&mut c, &[Wake::DisplayChanged]);
    for _ in 0..5 {
        env.step(&mut c, &[Wake::Timer]);
    }
    assert_eq!(lists(&env), before, "忙碌期間不列舉");
    assert_eq!(env.count_logs("列舉螢幕失敗"), 0, "{}", env.log_text());
    assert_eq!(env.count_logs("延後列舉"), 1, "{}", env.log_text());

    env.desk.lock().unwrap().hang_set = false;
    env.gate.release();
    wait_until_not_busy(&c);
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(lists(&env), before + 1, "忙碌解除後列舉一次");

    env.desk.lock().unwrap().fail_list = true;
    for _ in 0..3 {
        env.step(&mut c, &[Wake::DisplayChanged]);
    }
    assert_eq!(env.count_logs("列舉螢幕失敗"), 1, "{}", env.log_text());
}

/// [low 4] DPI 以螢幕矩形直接查（MonitorFromRect＋GetDpiForMonitor），不依賴顯示器清單逐像素相等；
/// 查不到時記一次警告並退回 96。
#[test]
fn dpi_is_looked_up_by_monitor_rect_and_missing_dpi_is_warned_once() {
    let env = Env::new("dpi-rect", WallpaperTheme::Astrolabe);
    // 顯示器清單的 B 矩形與 IDesktopWallpaper 的差 1 像素：舊做法會靜默退回 96。
    env.shared.displays.lock().unwrap()[1].rect = rect(1921, 2560, 1440);
    env.shared
        .dpi_by_rect
        .lock()
        .unwrap()
        .insert(rect_key(rect_b()), Some(144));
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    assert!(env.has_log("2560×1440 DPI 144"), "{}", env.log_text());
    // 縮放改 175%：該台重畫。
    env.shared
        .dpi_by_rect
        .lock()
        .unwrap()
        .insert(rect_key(rect_b()), Some(168));
    env.step(&mut c, &[Wake::DpiChanged]);
    assert_eq!(env.render_count(), 3, "{}", env.log_text());
    // 查不到：警告一次。
    env.shared
        .dpi_by_rect
        .lock()
        .unwrap()
        .insert(rect_key(rect_b()), None);
    for _ in 0..3 {
        env.step(&mut c, &[Wake::DpiChanged]);
    }
    assert_eq!(env.count_logs("無法取得 DPI"), 1, "{}", env.log_text());
}

/// [low 5] 讓位後使用者重選主題：設定已改、`SettingsChanged` 還沒送到時剛好計時器喚醒——主題變更
/// 仍先交給狀態機（解除停止），這一輪就照常接管，不被 `Refused(Stopped)` 卡到下一個時點。
#[test]
fn theme_reselected_after_yield_reaches_state_machine_before_render() {
    let env = Env::new("reselect-race", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    env.desk.lock().unwrap().monitors[0].wallpaper = Some(env.orig("c"));
    env.advance_to(taipei(10, 15, 0));
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(c.status().state, CoordinatorState::Idle, "已讓位");

    env.set_theme(WallpaperTheme::Astrolabe);
    env.advance_to(taipei(10, 16, 0));
    env.step(&mut c, &[Wake::Timer]);
    assert!(!env.has_log("Stopped"), "{}", env.log_text());
    assert!(is_host(&env.wallpaper(DEV_A), &env), "{}", env.log_text());
    assert!(is_host(&env.wallpaper(DEV_B), &env));
    // 遲到的 SettingsChanged：主題未變，不重複動作。
    let renders = env.render_count();
    env.step(&mut c, &[Wake::SettingsChanged]);
    assert_eq!(env.render_count(), renders);
}

/// [low 7] 每台在**自己**設定成功後約一個延遲（可注入）就讀回確認，不等整批：渲染 B 時 A 已確認。
#[test]
fn each_monitor_is_confirmed_one_delay_after_its_own_apply() {
    let mut env = Env::new("confirm-each", WallpaperTheme::Astrolabe);
    env.confirm_delay = Duration::from_secs(3);
    let a_confirmed_before_b = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&a_confirmed_before_b);
    let key_b = env.key(DEV_B, rect_b());
    let state_file = env.paths.state_file.clone();
    *env.shared.render_hook.lock().unwrap() = Some(Box::new(move |req| {
        if req.monitor_key == key_b {
            let v: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&state_file).unwrap()).unwrap();
            let confirmed = v["monitors"]
                .as_object()
                .unwrap()
                .values()
                .any(|r| r["host_applied"] == true);
            flag.store(confirmed, Ordering::SeqCst);
        }
    }));
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    assert!(
        a_confirmed_before_b.load(Ordering::SeqCst),
        "A 應在渲染 B 之前就確認\n{}",
        env.log_text()
    );
    assert_eq!(
        *env.shared.sleeps.lock().unwrap(),
        vec![Duration::from_secs(3), Duration::from_secs(3)]
    );
    let monitors = env.state_json()["monitors"].clone();
    assert!(monitors
        .as_object()
        .unwrap()
        .values()
        .all(|r| r["host_applied"] == true));
}

/// [low 9a] 主題為「不接管」（desktop-widget-host 預設）：任何喚醒都不碰 COM、不寫檔、不查顯示器。
#[test]
fn theme_none_touches_no_com_no_files_and_no_monitor_queries() {
    let env = Env::new("none-quiet", WallpaperTheme::None);
    let mut c = env.coordinator();
    for wake in [
        Wake::Startup,
        Wake::DisplayChanged,
        Wake::DpiChanged,
        Wake::DataChanged,
        Wake::Timer,
        Wake::PauseChanged,
        Wake::TimeChanged,
        Wake::ExplorerRestarted,
        Wake::ComRecovered,
        Wake::SettingsChanged,
        Wake::SpotlightConfirmed,
    ] {
        let next = env.shared.clock.lock().unwrap().wall + 61;
        env.advance_to(next);
        env.step(&mut c, &[wake]);
    }
    assert!(env.calls().is_empty(), "{:?}", env.calls());
    assert_eq!(env.shared.displays_calls.load(Ordering::SeqCst), 0);
    assert_eq!(env.shared.dpi_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        env.shared.explorer_reads.load(Ordering::SeqCst),
        0,
        "不接管時不讀 explorer GDI（4.9）"
    );
    assert_eq!(env.render_count(), 0);
    assert!(!env.paths.state_file.exists());
    assert!(!env.paths.output_dir.exists());
    let mut entries: Vec<String> = fs::read_dir(&env.dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    entries.sort();
    assert_eq!(entries, vec!["pictures"], "不寫任何檔");
}

/// [low 9b] 以**真正的** `AppState` 與正式的 `inputs_from_state`（`TauriPorts::inputs` 用的同一個函式）
/// 讀宿主輸入，渲染時 `AppState` 的鎖（設定、暫停原因、通道註冊表、`window_sync`）全部可取得。
/// `TauriPorts` 其餘部分（`AppHandle` 查 managed state、渲染視窗）需要執行中的 Tauri，單元測試做不到。
#[test]
fn real_app_state_inputs_leave_every_host_lock_free_during_render() {
    struct AppStatePorts {
        inner: FakePorts,
        app: Arc<crate::widgets::AppState>,
        all_free: Arc<AtomicBool>,
    }
    impl CoordinatorPorts for AppStatePorts {
        fn now(&self) -> UnixSeconds {
            self.inner.now()
        }
        fn monotonic(&self) -> Duration {
            self.inner.monotonic()
        }
        fn tz(&self) -> &dyn UtcOffsetSource {
            self.inner.tz()
        }
        fn inputs(&self) -> HostInputs {
            inputs_from_state(&self.app)
        }
        fn displays(&self) -> Vec<DisplayInfo> {
            self.inner.displays()
        }
        fn dpi_for_rect(&self, rect: PhysicalRect) -> Option<u32> {
            self.inner.dpi_for_rect(rect)
        }
        fn render(
            &self,
            req: &RenderRequest,
            out: &OutputTarget<'_>,
        ) -> Result<RenderSuccess, RenderFailure> {
            let free = self.app.settings.try_lock().is_ok()
                && self.app.pause_reasons.try_lock().is_ok()
                && self.app.registry.try_lock().is_ok()
                && self.app.window_sync.try_lock().is_ok();
            if !free {
                self.all_free.store(false, Ordering::SeqCst);
            }
            self.inner.render(req, out)
        }
        fn set_theme_none(&self, cause: ThemeNoneCause) -> Result<(), String> {
            self.inner.set_theme_none(cause)
        }
        fn resave_theme_none(&self) -> Result<bool, String> {
            self.inner.resave_theme_none()
        }
        fn notify_user(&self, notice: &TakeoverNotice) {
            self.inner.notify_user(notice);
        }
        fn sleep(&self, d: Duration) {
            self.inner.sleep(d);
        }
        fn explorer_gdi(&self) -> Result<ExplorerSample, String> {
            self.inner.explorer_gdi()
        }
    }

    let env = Env::new("real-inputs", WallpaperTheme::None);
    let settings = crate::settings::Settings {
        data_dir: env.dir.join("data"),
        wallpaper_theme: WallpaperTheme::Astrolabe,
        ..crate::settings::Settings::default()
    };
    let app = Arc::new(crate::widgets::AppState::new(
        settings,
        env.dir.join("settings.json"),
    ));
    let all_free = Arc::new(AtomicBool::new(true));
    let mut c = env.coordinator_with(AppStatePorts {
        inner: env.ports(),
        app: Arc::clone(&app),
        all_free: Arc::clone(&all_free),
    });
    env.step(&mut c, &[Wake::Startup]);
    assert_eq!(env.render_count(), 2, "{}", env.log_text());
    assert!(
        all_free.load(Ordering::SeqCst),
        "render() 期間不得持有 AppState 的鎖"
    );
}

// ---------------------------------------------------------------------------------------------
// task 4.7b：啟停路徑
// ---------------------------------------------------------------------------------------------

type Progress = Arc<Mutex<Vec<RequestProgress>>>;

/// 收集回覆的假回呼。
fn collector() -> (ReplyFn, Progress) {
    let got: Progress = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&got);
    (Box::new(move |p| sink.lock().unwrap().push(p)), got)
}

fn finished(got: &Progress) -> Option<RestoreReport> {
    got.lock().unwrap().iter().find_map(|p| match p {
        RequestProgress::Finished(r) => Some(r.clone()),
        RequestProgress::Started | RequestProgress::ExitedForUpdate => None,
    })
}

impl Env {
    fn fake_service(&self) -> WallpaperService {
        let desk = Arc::clone(&self.desk);
        let gate = Arc::clone(&self.gate);
        WallpaperService::spawn_with(
            WallpaperServiceConfig {
                timeout: self.service_timeout,
                ..WallpaperServiceConfig::default()
            },
            move || FakeBackend { desk, gate },
        )
        .unwrap()
    }

    fn command_line_parts(&self, settings_path: &Path) -> CommandLineParts {
        let logs = Arc::clone(&self.logs);
        CommandLineParts {
            service: self.fake_service(),
            registry: Box::new(FakeRegistry {
                values: Arc::clone(&self.registry_values),
            }),
            files: Box::new(NoFileIdentity),
            paths: self.paths.clone(),
            settings_path: settings_path.to_path_buf(),
            takeover: TakeoverConfig {
                verify_attempts: 1,
                verify_interval: Duration::from_millis(1),
                registry_settle_timeout: Duration::from_millis(5),
                registry_poll_interval: Duration::from_millis(1),
                transient_retry_delay: Duration::from_millis(1),
            },
            now: self.shared.clock.lock().unwrap().wall,
            log: Arc::new(move |level, msg: &str| {
                logs.lock().unwrap().push((level, msg.to_owned()));
            }),
        }
    }

    fn effects(&self) -> Vec<String> {
        self.shared.effects.lock().unwrap().clone()
    }

    /// 系統匣結束已把主題存成「不接管」（task 6.4）之後，模擬那次存檔沒有落地、下次啟動仍是原主題：
    /// 驗的是還原中斷續做本身（與它完成後改「不接管」的後備 `RestoreResumed`）。清掉副作用紀錄。
    fn forget_exit_theme_save(&self, theme: WallpaperTheme) {
        assert!(
            self.effects()
                .contains(&format!("theme_none {:?}", ThemeNoneCause::TrayExit)),
            "系統匣結束一律存成不接管：{:?}",
            self.effects()
        );
        self.set_theme(theme);
        self.shared.effects.lock().unwrap().clear();
    }

    fn marker_reason(&self) -> Option<String> {
        let v = self.state_json();
        v.get("restore_in_progress")
            .and_then(|m| m.get("reason"))
            .and_then(|r| r.as_str())
            .map(str::to_owned)
    }

    fn assert_originals(&self) {
        assert_eq!(
            self.wallpaper(DEV_A).as_deref(),
            Some(self.orig("a").as_str()),
            "{}",
            self.log_text()
        );
        assert_eq!(
            self.wallpaper(DEV_B).as_deref(),
            Some(self.orig("b").as_str()),
            "{}",
            self.log_text()
        );
    }
}

/// task 6.4（審查 R2b-L1）：讓位把主題改「不接管」時存檔失敗 → 依 `retry_delay` 重試存檔直到成功，
/// 否則重啟後會照設定檔的舊主題重新接管、蓋掉使用者剛換的桌布。
#[test]
fn theme_none_save_failure_is_retried_until_it_succeeds() {
    let env = Env::new("r64-theme-save-retry", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    env.shared.theme_save_failures.store(2, Ordering::SeqCst);
    env.desk.lock().unwrap().monitors[0].wallpaper = Some(r"C:\Users\u\Pictures\mine.jpg".into());
    let t0 = taipei(10, 15, 0);
    env.advance_to(t0);
    let out = env.step(&mut c, &[Wake::Timer]);
    assert!(
        env.effects()
            .contains(&"theme_none UserChangedWallpaper".to_owned()),
        "{:?}",
        env.effects()
    );
    assert_eq!(
        out.next_wake,
        Some(t0 + 120),
        "第 1 次失敗後 2 分鐘重試\n{}",
        env.log_text()
    );

    // 第一次重試仍失敗 → 退避 4 分鐘。
    env.advance_to(t0 + 120);
    let out = env.step(&mut c, &[Wake::Timer]);
    assert_eq!(
        env.count_effects("theme_none_resave"),
        1,
        "{:?}",
        env.effects()
    );
    assert_eq!(out.next_wake, Some(t0 + 120 + 240), "{}", env.log_text());

    // 第二次重試成功 → 不再重試。
    env.advance_to(t0 + 360);
    let out = env.step(&mut c, &[Wake::Timer]);
    assert_eq!(env.count_effects("theme_none_resave"), 2);
    assert_eq!(out.next_wake, None, "{}", env.log_text());
    assert!(
        env.has_log("主題「不接管」重新存檔成功"),
        "{}",
        env.log_text()
    );
    env.advance_to(t0 + 3600);
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(env.count_effects("theme_none_resave"), 2);
}

/// 存檔重試期間使用者選回主題：停止重試（那次選擇已存檔），不把主題改回「不接管」。
#[test]
fn theme_none_save_retry_stops_when_user_reselects_a_theme() {
    let env = Env::new("r64-theme-save-reselect", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    env.shared.theme_save_failures.store(1, Ordering::SeqCst);
    env.desk.lock().unwrap().monitors[0].wallpaper = Some(r"C:\Users\u\Pictures\mine.jpg".into());
    let t0 = taipei(10, 15, 0);
    env.advance_to(t0);
    env.step(&mut c, &[Wake::Timer]);
    env.set_theme(WallpaperTheme::Skyline);
    env.advance_to(t0 + 120);
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(
        env.shared.inputs.lock().unwrap().theme,
        WallpaperTheme::Skyline
    );
    assert!(env.has_log("使用者已選回主題"), "{}", env.log_text());
}

/// 系統匣「結束」：先回報開始、還原（含填滿方式）、回報結果，協調迴圈結束；記錄可辨識。
#[test]
fn tray_exit_request_restores_then_ends_the_loop() {
    let env = Env::new("b-tray-exit", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    assert!(is_host(&env.wallpaper(DEV_A), &env));
    env.desk.lock().unwrap().position = 10; // 宿主設的填滿方式（與原本的 3 不同）

    let (reply, got) = collector();
    let control = c.handle_request(Request::ExitAndRestore(reply));
    assert_eq!(control, LoopControl::Exit);
    let progress = got.lock().unwrap().clone();
    assert_eq!(progress.first(), Some(&RequestProgress::Started));
    let report = finished(&got).expect("回報結果");
    assert_eq!(report.code, RestoreCode::Restored, "{report:?}");
    assert_eq!(report.code.exit_code(), 0);
    env.assert_originals();
    assert_eq!(env.desk.lock().unwrap().position, 3, "全域填滿方式還原");
    assert_eq!(env.state_json()["status"], "not_taken_over");
    assert_eq!(env.marker_reason(), None, "成功後清除標記");
    assert!(env.has_log("系統匣「結束」"), "{}", env.log_text());
    assert!(
        env.has_log("還原原桌布：原因 TrayExit"),
        "{}",
        env.log_text()
    );
}

/// task 6.4（審查 R1 low）：系統匣「結束」一律把主題存成「不接管」——還原成功時也一樣，下次啟動不再
/// 接管（與還原逾時、下次續做完成後的結果一致）。
#[test]
fn tray_exit_always_saves_theme_none() {
    let env = Env::new("r64-tray-exit-theme", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    let (reply, got) = collector();
    assert_eq!(
        c.handle_request(Request::ExitAndRestore(reply)),
        LoopControl::Exit
    );
    assert_eq!(finished(&got).unwrap().code, RestoreCode::Restored);
    env.assert_originals();
    assert!(
        env.effects()
            .contains(&format!("theme_none {:?}", ThemeNoneCause::TrayExit)),
        "{:?}",
        env.effects()
    );
    assert_eq!(
        env.shared.inputs.lock().unwrap().theme,
        WallpaperTheme::None
    );
}

/// task 6.4（審查 R1-M3a）：接管期間接上的新螢幕原圖備份不了 → 不設定它、主題改「不接管」並通知，
/// 已接管的其他螢幕在 Redraw 結束後照「不接管」還原。
#[test]
fn backup_failure_on_new_monitor_stops_takeover_and_restores_the_rest() {
    let env = Env::new("r64-backup-failed", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    assert!(is_host(&env.wallpaper(DEV_A), &env));
    let gone = env
        .dir
        .join("pictures")
        .join("gone.jpg")
        .to_string_lossy()
        .into_owned();
    env.desk.lock().unwrap().monitors.push(FakeMonitor {
        device_path: DEV_C.into(),
        rect: Some(rect_c()),
        wallpaper: Some(gone.clone()),
    });
    env.shared
        .displays
        .lock()
        .unwrap()
        .push(display(DEV_C, rect_c(), 96));
    env.step(&mut c, &[Wake::DisplayChanged]);
    assert_eq!(
        env.wallpaper(DEV_C),
        Some(gone),
        "C 不動\n{}",
        env.log_text()
    );
    let effects = env.effects();
    assert!(
        effects.contains(&format!("theme_none {:?}", ThemeNoneCause::BackupFailed)),
        "{effects:?}"
    );
    assert!(
        effects.iter().any(|e| e.contains("BackupFailed")),
        "{effects:?}"
    );
    env.assert_originals();
    assert_eq!(env.state_json()["status"], "not_taken_over");
}

/// 主題為「不接管」時的系統匣結束：不碰 COM、不寫任何檔，立即回報（與改動前相同的結束）。
#[test]
fn tray_exit_with_theme_none_touches_no_com_and_no_files() {
    let env = Env::new("b-tray-exit-none", WallpaperTheme::None);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    let (reply, got) = collector();
    assert_eq!(
        c.handle_request(Request::ExitAndRestore(reply)),
        LoopControl::Exit
    );
    assert_eq!(finished(&got).unwrap().code, RestoreCode::Restored);
    assert!(env.calls().is_empty(), "{:?}", env.calls());
    assert!(!env.paths.state_file.exists());
    assert_eq!(env.shared.displays_calls.load(Ordering::SeqCst), 0);
}

/// 結束要求在 Redraw 中途到達：已渲染的那台不送、其餘不渲染，Redraw 中止後才還原，迴圈結束。
#[test]
fn tray_exit_during_redraw_aborts_it_then_restores() {
    let env = Env::new("b-tray-exit-redraw", WallpaperTheme::Astrolabe);
    let c = env.coordinator();
    let shared = Arc::clone(&env.shared);
    let (handle, join) = spawn_coordinator(move || Ok(c)).unwrap();
    // 等啟動接管完成（兩台都畫好）。
    let deadline = Instant::now() + Duration::from_secs(5);
    while handle.status().state != CoordinatorState::UpToDate && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(env.render_count(), 2, "{}", env.log_text());
    let sets_before = env.set_calls().len();

    // 10:15 的 Redraw：第一台渲染期間要求結束。
    let started = Arc::new((Mutex::new(false), Condvar::new()));
    let started2 = Arc::clone(&started);
    *shared.render_hook.lock().unwrap() = Some(Box::new(move |_req| {
        {
            let (m, cv) = &*started2;
            *m.lock().unwrap() = true;
            cv.notify_all();
        }
        std::thread::sleep(Duration::from_millis(200));
    }));
    env.advance_to(taipei(10, 15, 0));
    handle.notify(Wake::Timer);
    {
        let (m, cv) = &*started;
        let mut g = m.lock().unwrap();
        while !*g {
            g = cv.wait(g).unwrap();
        }
    }
    let wait = exit_and_restore(
        &handle,
        ExitLimits {
            redraw_wait: Duration::from_secs(5),
            restore_limit: Duration::from_secs(5),
        },
    );
    join.join().unwrap();
    let ExitWait::Finished(report) = wait else {
        panic!("應回報結果：{wait:?}\n{}", env.log_text());
    };
    assert_eq!(report.code, RestoreCode::Restored);
    assert_eq!(env.render_count(), 3, "第二台不再渲染\n{}", env.log_text());
    let host_sets_after: Vec<_> = env.set_calls()[sets_before..]
        .iter()
        .filter(|c| c.contains(env.paths.output_dir.to_str().unwrap()))
        .cloned()
        .collect();
    assert!(host_sets_after.is_empty(), "{host_sets_after:?}");
    env.assert_originals();
    let text = env.log_text();
    let aborted = text.find("Redraw 中止").expect("記錄 Redraw 中止");
    let restored = text.find("還原原桌布：原因 TrayExit").expect("記錄還原");
    assert!(aborted < restored, "{text}");
}

/// 還原超過上限（explorer 卡住）：照常結束；狀態檔留著「還原進行中」標記，下次啟動續做。
#[test]
fn tray_exit_restore_timeout_leaves_the_marker_on_disk() {
    let mut env = Env::new("b-tray-exit-timeout", WallpaperTheme::Astrolabe);
    env.service_timeout = Duration::from_secs(5);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    env.desk.lock().unwrap().hang_set = true;
    let (handle, join) = spawn_coordinator(move || Ok(c)).unwrap();
    let wait = exit_and_restore(
        &handle,
        ExitLimits {
            redraw_wait: Duration::from_secs(5),
            restore_limit: Duration::from_millis(300),
        },
    );
    assert_eq!(wait, ExitWait::TimedOut, "{}", env.log_text());
    assert_eq!(env.marker_reason().as_deref(), Some("tray_exit"));
    assert_eq!(env.state_json()["status"], "taken_over");
    env.gate.release();
    join.join().unwrap();
}

/// 結束等待：沒在時限內開始、開始後逾時、通道斷了。
#[test]
fn exit_wait_classifies_not_started_timeout_and_disconnect() {
    let limits = ExitLimits {
        redraw_wait: Duration::from_millis(30),
        restore_limit: Duration::from_millis(30),
    };
    let (_tx, rx) = std::sync::mpsc::channel::<RequestProgress>();
    assert_eq!(wait_exit_restore(&rx, limits), ExitWait::NotStarted);
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(RequestProgress::Started).unwrap();
    assert_eq!(wait_exit_restore(&rx, limits), ExitWait::TimedOut);
    let (tx, rx) = std::sync::mpsc::channel::<RequestProgress>();
    drop(tx);
    assert_eq!(wait_exit_restore(&rx, limits), ExitWait::Disconnected);
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(RequestProgress::Started).unwrap();
    let report = RestoreReport {
        code: RestoreCode::Failed,
        summary: "x".into(),
    };
    tx.send(RequestProgress::Finished(report.clone())).unwrap();
    assert_eq!(wait_exit_restore(&rx, limits), ExitWait::Finished(report));
}

/// 迴圈已結束（或從未啟動）時送出的要求立即斷線，不讓系統匣結束空等。
#[test]
fn request_to_a_closed_loop_disconnects_immediately() {
    let (handle, join) =
        spawn_coordinator::<FakePorts, _>(|| Err("假的啟動失敗".to_owned())).unwrap();
    join.join().unwrap();
    let t = Instant::now();
    let wait = exit_and_restore(
        &handle,
        ExitLimits {
            redraw_wait: Duration::from_secs(5),
            restore_limit: Duration::from_secs(5),
        },
    );
    assert_eq!(wait, ExitWait::Disconnected);
    assert!(t.elapsed() < Duration::from_secs(1));
}

/// 修正第 2 輪（複審 nit）：A6 之後最常見的路徑——系統匣結束已把主題存成「不接管」、還原卻沒做完（標記
/// 留著）。下次啟動主題是「不接管」：照樣先續做還原（不渲染、不接管），完成後清掉標記；主題本來就是
/// 「不接管」，不必再改。
#[test]
fn theme_none_with_restore_marker_resumes_on_next_start() {
    let env = Env::new("r64b-none-marker", WallpaperTheme::Astrolabe);
    {
        let mut c = env.coordinator();
        env.step(&mut c, &[Wake::Startup]);
        env.desk.lock().unwrap().fail_read = true;
        let (reply, got) = collector();
        c.handle_request(Request::ExitAndRestore(reply));
        assert_eq!(finished(&got).unwrap().code, RestoreCode::Failed);
    }
    assert_eq!(env.marker_reason().as_deref(), Some("tray_exit"));
    assert_eq!(
        env.shared.inputs.lock().unwrap().theme,
        WallpaperTheme::None
    );
    env.desk.lock().unwrap().fail_read = false;
    let renders_before = env.render_count();
    let effects_before = env.effects().len();

    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    env.assert_originals();
    assert_eq!(env.state_json()["status"], "not_taken_over");
    assert_eq!(env.marker_reason(), None);
    assert!(env.has_log("還原中斷續做"), "{}", env.log_text());
    assert_eq!(env.render_count(), renders_before, "續做期間與之後都不渲染");
    assert!(
        env.effects()[effects_before..]
            .iter()
            .all(|e| !e.starts_with("theme_none")),
        "主題本來就是不接管：{:?}",
        env.effects()
    );
    env.advance_to(taipei(10, 15, 0));
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(env.render_count(), renders_before);
}

/// 還原中斷（系統匣結束時讀取失敗）→ 下次啟動先續做還原、不渲染不接管；完成後主題改「不接管」。
#[test]
fn interrupted_tray_exit_restore_resumes_on_next_start_and_stays_not_taken_over() {
    let env = Env::new("b-resume", WallpaperTheme::Astrolabe);
    {
        let mut c = env.coordinator();
        env.step(&mut c, &[Wake::Startup]);
        env.desk.lock().unwrap().fail_read = true;
        let (reply, got) = collector();
        assert_eq!(
            c.handle_request(Request::ExitAndRestore(reply)),
            LoopControl::Exit
        );
        assert_eq!(finished(&got).unwrap().code, RestoreCode::Failed);
    }
    assert_eq!(env.marker_reason().as_deref(), Some("tray_exit"));
    env.desk.lock().unwrap().fail_read = false;
    env.forget_exit_theme_save(WallpaperTheme::Astrolabe);
    let renders_before = env.render_count();

    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    assert_eq!(
        env.render_count(),
        renders_before,
        "續做還原，不接管\n{}",
        env.log_text()
    );
    env.assert_originals();
    assert_eq!(env.state_json()["status"], "not_taken_over");
    assert_eq!(env.marker_reason(), None);
    assert!(
        env.effects()
            .contains(&format!("theme_none {:?}", ThemeNoneCause::RestoreResumed)),
        "{:?}",
        env.effects()
    );
    assert!(env.has_log("還原中斷續做"), "{}", env.log_text());
    // 之後的時點也不接管（主題已是不接管）。
    env.advance_to(taipei(10, 15, 0));
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(env.render_count(), renders_before);
}

/// 續做的還原失敗：不渲染、不走讓位（原桌布記錄不丟）、依退避重試；成功後才結束續做。
#[test]
fn failing_resume_blocks_takeover_and_retries_without_yielding() {
    let env = Env::new("b-resume-fail", WallpaperTheme::Astrolabe);
    {
        let mut c = env.coordinator();
        env.step(&mut c, &[Wake::Startup]);
        env.desk.lock().unwrap().fail_read = true;
        let (reply, _got) = collector();
        c.handle_request(Request::ExitAndRestore(reply));
    }
    env.forget_exit_theme_save(WallpaperTheme::Astrolabe);
    let renders_before = env.render_count();
    let mut c = env.coordinator();
    let t0 = env.shared.clock.lock().unwrap().wall;
    let out = env.step(&mut c, &[Wake::Startup]);
    assert_eq!(c.status().state, CoordinatorState::ResumingRestore);
    assert_eq!(out.next_wake, Some(t0 + 120), "{}", env.log_text());
    assert_eq!(env.render_count(), renders_before);
    assert_eq!(env.state_json()["status"], "taken_over");
    assert!(env.state_json()["original"].is_object(), "原桌布記錄不丟");
    assert!(env.effects().is_empty(), "不讓位：{:?}", env.effects());
    // 資料更新不會開始渲染。
    env.step(&mut c, &[Wake::DataChanged]);
    assert_eq!(env.render_count(), renders_before);

    env.desk.lock().unwrap().fail_read = false;
    env.advance_to(t0 + 120);
    env.step(&mut c, &[Wake::Timer]);
    env.assert_originals();
    assert_eq!(env.marker_reason(), None);
    assert_eq!(env.render_count(), renders_before);
    assert_ne!(c.status().state, CoordinatorState::ResumingRestore);
}

/// 續做期間使用者重新選了主題：續做完成後照常接管（不強制改回不接管）。
#[test]
fn theme_reselected_during_resume_takes_over_after_it_completes() {
    let env = Env::new("b-resume-reselect", WallpaperTheme::Astrolabe);
    {
        let mut c = env.coordinator();
        env.step(&mut c, &[Wake::Startup]);
        env.desk.lock().unwrap().fail_read = true;
        let (reply, _got) = collector();
        c.handle_request(Request::ExitAndRestore(reply));
    }
    env.forget_exit_theme_save(WallpaperTheme::Astrolabe);
    let renders_before = env.render_count();
    let mut c = env.coordinator();
    let t0 = env.shared.clock.lock().unwrap().wall;
    env.step(&mut c, &[Wake::Startup]);
    env.set_theme(WallpaperTheme::Skyline);
    env.shared.inputs.lock().unwrap().data.twii_daily = CivilDate::parse_iso("2026-10-02");
    env.step(&mut c, &[Wake::SettingsChanged]);
    assert_eq!(env.render_count(), renders_before, "續做完成前不接管");
    env.desk.lock().unwrap().fail_read = false;
    env.advance_to(t0 + 120);
    env.step(&mut c, &[Wake::Timer]);
    assert!(
        !env.effects().iter().any(|e| e.starts_with("theme_none")),
        "{:?}",
        env.effects()
    );
    assert_eq!(env.render_count(), renders_before + 2, "{}", env.log_text());
    assert!(is_host(&env.wallpaper(DEV_A), &env));
}

/// 當機後重啟（沒有標記）：照常接管並重畫，記錄可辨識。
#[test]
fn restart_after_crash_without_marker_takes_over_and_redraws() {
    let env = Env::new("b-crash", WallpaperTheme::Astrolabe);
    {
        let mut c = env.coordinator();
        env.step(&mut c, &[Wake::Startup]);
    } // 當機：狀態機消失，狀態檔仍是接管中
    assert_eq!(env.state_json()["status"], "taken_over");
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    assert_eq!(env.render_count(), 4, "{}", env.log_text());
    assert!(is_host(&env.wallpaper(DEV_A), &env));
    assert!(env.effects().is_empty(), "{:?}", env.effects());
    assert!(env.has_log("照常接管"), "{}", env.log_text());
}

/// 工作階段結束：不還原、不呼叫桌布 API、不渲染；狀態維持接管中。取消後恢復。
#[test]
fn session_end_does_not_restore_and_stops_rendering_until_cancelled() {
    let env = Env::new("b-session-end", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    let calls_before = env.calls().len();
    env.step(&mut c, &[Wake::SessionEnding]);
    env.advance_to(taipei(10, 15, 0));
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(env.calls().len(), calls_before, "{:?}", env.calls());
    assert_eq!(env.render_count(), 2);
    assert_eq!(env.state_json()["status"], "taken_over");
    assert_eq!(env.marker_reason(), None);
    assert_eq!(c.status().state, CoordinatorState::SessionEnding);
    assert!(env.has_log("工作階段結束"), "{}", env.log_text());
    assert!(env.has_log("不還原"), "{}", env.log_text());

    env.step(&mut c, &[Wake::SessionEndCancelled]);
    assert_eq!(env.render_count(), 4, "取消後補畫\n{}", env.log_text());
}

/// 工作階段結束期間，排定的「不接管」還原重試也不執行（不還原）。
#[test]
fn session_end_suspends_pending_restore_retries() {
    let env = Env::new("b-session-end-retry", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    env.desk.lock().unwrap().fail_read = true;
    env.set_theme(WallpaperTheme::None);
    let t0 = env.shared.clock.lock().unwrap().wall;
    env.step(&mut c, &[Wake::SettingsChanged]);
    env.desk.lock().unwrap().fail_read = false;
    env.step(&mut c, &[Wake::SessionEnding]);
    env.advance_to(t0 + 120);
    env.step(&mut c, &[Wake::Timer, Wake::ComRecovered]);
    assert_eq!(env.count_logs("還原原桌布：原因"), 1, "{}", env.log_text());
    assert_eq!(env.state_json()["status"], "taken_over");
}

/// 命令列交給執行中的宿主：主題改不接管並存檔、還原、回報；迴圈繼續，之後的設定變更不重複還原。
#[test]
fn command_line_request_sets_theme_none_restores_and_replies() {
    let env = Env::new("b-cli-primary", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    let (reply, got) = collector();
    assert_eq!(
        c.handle_request(Request::CommandLineRestore(reply)),
        LoopControl::Continue
    );
    assert_eq!(finished(&got).unwrap().code, RestoreCode::Restored);
    assert!(
        env.effects()
            .contains(&format!("theme_none {:?}", ThemeNoneCause::CommandLine)),
        "{:?}",
        env.effects()
    );
    env.assert_originals();
    assert_eq!(env.state_json()["status"], "not_taken_over");
    env.step(&mut c, &[Wake::SettingsChanged]);
    assert_eq!(env.count_logs("還原原桌布：原因"), 1, "{}", env.log_text());
    assert!(env.has_log("命令列"), "{}", env.log_text());
}

/// 命令列在沒有宿主時直接還原：主題改不接管並存檔、還原、結束碼 0；狀態檔封鎖時結束碼 2。
#[test]
fn command_line_local_restore_writes_theme_none_and_reports_exit_codes() {
    let env = Env::new("b-cli-local", WallpaperTheme::Astrolabe);
    {
        let mut c = env.coordinator();
        env.step(&mut c, &[Wake::Startup]);
    }
    let settings_path = env.dir.join("settings.json");
    crate::settings::save(
        &settings_path,
        &crate::settings::Settings {
            wallpaper_theme: WallpaperTheme::Astrolabe,
            ..crate::settings::Settings::default()
        },
    )
    .unwrap();
    let report = restore_for_command_line(env.command_line_parts(&settings_path));
    assert_eq!(
        report.code,
        RestoreCode::Restored,
        "{report:?}\n{}",
        env.log_text()
    );
    env.assert_originals();
    assert_eq!(
        crate::settings::load_for_arbitrated_startup(&settings_path, false)
            .settings
            .wallpaper_theme,
        WallpaperTheme::None
    );
    assert_eq!(env.marker_reason(), None);
    assert!(env.has_log("--restore-wallpaper"), "{}", env.log_text());

    // 再跑一次：沒有東西要還原，仍是 0，不再設定桌布。
    let sets = env.set_calls().len();
    let report = restore_for_command_line(env.command_line_parts(&settings_path));
    assert_eq!(report.code, RestoreCode::Restored);
    assert_eq!(env.set_calls().len(), sets);

    fs::write(&env.paths.state_file, b"not json").unwrap();
    let report = restore_for_command_line(env.command_line_parts(&settings_path));
    assert_eq!(report.code, RestoreCode::Blocked);
    assert_eq!(report.code.exit_code(), 2);
}

/// 設定檔不存在：不建立它（預設就是不接管）；從未接管也不建立狀態檔。
#[test]
fn command_line_local_restore_does_not_create_a_settings_file() {
    let env = Env::new("b-cli-no-settings", WallpaperTheme::Astrolabe);
    let settings_path = env.dir.join("settings.json");
    let report = restore_for_command_line(env.command_line_parts(&settings_path));
    assert_eq!(report.code, RestoreCode::Restored);
    assert!(!settings_path.exists());
    assert!(!env.paths.state_file.exists());
}

/// 還原重試限頻（比照 4.3 M1）：設定路徑恢復觸發的重試每個退避窗至多一次；退避時刻到的重試開啟
/// 新的額度。不會形成「逾時→恢復→立即重試」的無限循環。
#[test]
fn recovery_triggered_restore_retries_are_limited_to_one_per_backoff_window() {
    let env = Env::new("b-rate-limit", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    env.desk.lock().unwrap().fail_read = true;
    env.set_theme(WallpaperTheme::None);
    let t0 = env.shared.clock.lock().unwrap().wall;
    env.step(&mut c, &[Wake::SettingsChanged]);
    let attempts = |env: &Env| env.count_logs("還原原桌布：原因");
    assert_eq!(attempts(&env), 1);

    env.step(&mut c, &[Wake::ComRecovered]);
    assert_eq!(attempts(&env), 2, "這個退避窗的第一次恢復：重試");
    for wake in [
        Wake::ComRecovered,
        Wake::ExplorerRestarted,
        Wake::ComRecovered,
    ] {
        env.step(&mut c, &[wake]);
    }
    assert_eq!(
        attempts(&env),
        2,
        "同一個退避窗不再因恢復而重試\n{}",
        env.log_text()
    );
    assert!(env.has_log("已用過"), "{}", env.log_text());

    // 恢復重試失敗後的新退避窗（第 2 次失敗 → 240 秒）到期：照常重試，並重新取得一次恢復額度。
    let retry_at = c.status().restore_retry_at.expect("仍排定重試");
    assert!(retry_at >= t0 + 240, "{retry_at} vs {t0}");
    env.advance_to(retry_at);
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(attempts(&env), 3);
    env.step(&mut c, &[Wake::ComRecovered]);
    assert_eq!(attempts(&env), 4);
    env.step(&mut c, &[Wake::ComRecovered]);
    assert_eq!(attempts(&env), 4);
}

/// 「不接管」的還原失敗後使用者又選回主題：取消重試並清除標記（之後照常接管，不會在重啟時續做）。
#[test]
fn reselecting_a_theme_after_a_failed_restore_clears_the_marker() {
    let env = Env::new("b-abandon", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    env.desk.lock().unwrap().fail_read = true;
    env.set_theme(WallpaperTheme::None);
    env.step(&mut c, &[Wake::SettingsChanged]);
    assert_eq!(env.marker_reason().as_deref(), Some("not_takeover"));
    env.desk.lock().unwrap().fail_read = false;
    env.set_theme(WallpaperTheme::Astrolabe);
    env.step(&mut c, &[Wake::SettingsChanged]);
    assert_eq!(env.marker_reason(), None, "{}", env.log_text());
    assert_eq!(c.status().restore_retry_at, None);
}

/// 啟動方式的記錄文字（RegisterApplicationRestart 重啟、開機自啟、一般）。
#[test]
fn launch_kind_names_restart_manager_and_autostart() {
    let args = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    assert!(
        launch_kind(&args(&["fc-host.exe", "--restarted"])).contains("RegisterApplicationRestart")
    );
    assert!(launch_kind(&args(&["fc-host.exe", "--autostart"])).contains("開機自啟"));
    assert!(launch_kind(&args(&["fc-host.exe"])).contains("一般"));
}

/// 工作階段結束中切到「不接管」：不還原；結束被取消後才還原。
#[test]
fn switching_to_none_during_session_end_defers_the_restore() {
    let env = Env::new("b-session-end-none", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    env.step(&mut c, &[Wake::SessionEnding]);
    env.set_theme(WallpaperTheme::None);
    env.step(&mut c, &[Wake::SettingsChanged]);
    assert_eq!(env.count_logs("還原原桌布：原因"), 0, "{}", env.log_text());
    assert_eq!(env.state_json()["status"], "taken_over");
    env.step(&mut c, &[Wake::SessionEndCancelled]);
    assert_eq!(env.count_logs("還原原桌布：原因"), 1, "{}", env.log_text());
    env.assert_originals();
}

/// 工作階段結束通知後行程一直沒被結束（錯過取消通知）：逾時後恢復排程，不會永遠停畫。
#[test]
fn stale_session_end_resumes_scheduling_after_the_limit() {
    let env = Env::new("b-session-end-stale", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    let t0 = env.shared.clock.lock().unwrap().wall;
    env.step(&mut c, &[Wake::SessionEnding]);
    env.advance_to(t0 + 5 * 60);
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(env.render_count(), 2, "上限內仍不渲染");
    env.advance_to(t0 + 11 * 60);
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(env.render_count(), 4, "{}", env.log_text());
    assert!(env.has_log("錯過取消通知"), "{}", env.log_text());
}

// ---------------------------------------------------------------------------------------------
// task 4.7b 修正輪 1：等進行中的 Redraw 之前就寫「還原進行中」標記
// ---------------------------------------------------------------------------------------------

/// 啟動一個協調執行緒並等它接管完成；讓 10:15 那次的第一台渲染卡在回傳的閘門上。
fn spawn_and_block_next_render(
    env: &Env,
) -> (CoordinatorHandle, std::thread::JoinHandle<()>, Arc<Gate>) {
    let c = env.coordinator();
    let (handle, join) = spawn_coordinator(move || Ok(c)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while handle.status().state != CoordinatorState::UpToDate && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(env.render_count(), 2, "{}", env.log_text());
    let started = Arc::new((Mutex::new(false), Condvar::new()));
    let started2 = Arc::clone(&started);
    let gate = Arc::new(Gate::default());
    let gate2 = Arc::clone(&gate);
    *env.shared.render_hook.lock().unwrap() = Some(Box::new(move |_req| {
        {
            let (m, cv) = &*started2;
            *m.lock().unwrap() = true;
            cv.notify_all();
        }
        gate2.wait();
    }));
    env.advance_to(taipei(10, 15, 0));
    handle.notify(Wake::Timer);
    {
        let (m, cv) = &*started;
        let mut g = m.lock().unwrap();
        while !*g {
            g = cv.wait(g).unwrap();
        }
    }
    (handle, join, gate)
}

/// 渲染卡住超過上限：沒有還原就結束，但標記已在等待之前寫好；下次啟動續做還原、不接管。
#[test]
fn tray_quit_with_a_render_in_flight_past_the_cap_leaves_the_marker_and_next_start_resumes() {
    let env = Env::new("b-fix1-inflight", WallpaperTheme::Astrolabe);
    let (handle, join, gate) = spawn_and_block_next_render(&env);
    let wait = exit_and_restore(
        &handle,
        ExitLimits {
            redraw_wait: Duration::from_millis(100),
            restore_limit: Duration::from_secs(1),
        },
    );
    assert_eq!(wait, ExitWait::NotStarted, "{}", env.log_text());
    assert_eq!(env.marker_reason().as_deref(), Some("tray_exit"));
    assert_eq!(env.state_json()["status"], "taken_over");
    assert!(is_host(&env.wallpaper(DEV_A), &env), "還沒還原");

    // 模擬行程在這一刻結束：保留這時的狀態檔與桌面，讓卡住的執行緒收尾後再放回去。
    let state_at_exit = fs::read(&env.paths.state_file).unwrap();
    let desk_at_exit: Vec<Option<String>> = env
        .desk
        .lock()
        .unwrap()
        .monitors
        .iter()
        .map(|m| m.wallpaper.clone())
        .collect();
    gate.release();
    join.join().unwrap();
    fs::write(&env.paths.state_file, &state_at_exit).unwrap();
    for (m, w) in env
        .desk
        .lock()
        .unwrap()
        .monitors
        .iter_mut()
        .zip(desk_at_exit)
    {
        m.wallpaper = w;
    }
    env.forget_exit_theme_save(WallpaperTheme::Astrolabe);
    let renders_before = env.render_count();

    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    assert_eq!(
        env.render_count(),
        renders_before,
        "續做，不接管\n{}",
        env.log_text()
    );
    env.assert_originals();
    assert_eq!(env.state_json()["status"], "not_taken_over");
    assert_eq!(env.marker_reason(), None);
    assert!(env.has_log("還原中斷續做"), "{}", env.log_text());
    assert!(
        env.effects()
            .contains(&format!("theme_none {:?}", ThemeNoneCause::RestoreResumed)),
        "{:?}",
        env.effects()
    );
}

/// 沒有進行中的渲染：照常還原，標記清除。
#[test]
fn tray_quit_without_a_render_in_flight_restores_and_clears_the_marker() {
    let env = Env::new("b-fix1-idle", WallpaperTheme::Astrolabe);
    let c = env.coordinator();
    let (handle, join) = spawn_coordinator(move || Ok(c)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while handle.status().state != CoordinatorState::UpToDate && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    let wait = exit_and_restore(&handle, ExitLimits::default());
    join.join().unwrap();
    let ExitWait::Finished(report) = wait else {
        panic!("{wait:?}\n{}", env.log_text());
    };
    assert_eq!(report.code, RestoreCode::Restored);
    env.assert_originals();
    assert_eq!(env.state_json()["status"], "not_taken_over");
    assert_eq!(env.marker_reason(), None);
}

/// 命令列交接：宿主接受要求的當下就寫標記（不等進行中的渲染）；處理完清除並回報。
#[test]
fn command_line_handoff_writes_the_marker_before_the_coordinator_handles_it() {
    let env = Env::new("b-fix1-cli", WallpaperTheme::Astrolabe);
    let (handle, join, gate) = spawn_and_block_next_render(&env);
    let (reply, got) = collector();
    assert_eq!(
        handle.request_restore(Request::CommandLineRestore(reply)),
        MarkOutcome::Written
    );
    assert_eq!(env.marker_reason().as_deref(), Some("command_line"));
    assert!(finished(&got).is_none(), "渲染還卡著，還沒處理");
    gate.release();
    let deadline = Instant::now() + Duration::from_secs(5);
    while finished(&got).is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(finished(&got).map(|r| r.code), Some(RestoreCode::Restored));
    env.assert_originals();
    assert_eq!(env.marker_reason(), None);
    handle.shutdown();
    join.join().unwrap();
}

// ---------------------------------------------------------------------------------------------
// task 4.7b 修正輪 2（審查 task-4.7b-review.md）
// ---------------------------------------------------------------------------------------------

/// 以獨佔共用模式開著檔案＝其他人讀不到（共用違規），模擬防毒或備份軟體短暫鎖住狀態檔。
fn lock_state_file(path: &Path) -> fs::File {
    use std::os::windows::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .expect("鎖住狀態檔")
}

/// 先以系統匣結束（讀取失敗）留下 tray_exit 標記，回傳之前的渲染次數。
fn leave_tray_exit_marker(env: &Env) -> usize {
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    env.desk.lock().unwrap().fail_read = true;
    let (reply, got) = collector();
    c.handle_request(Request::ExitAndRestore(reply));
    assert_eq!(finished(&got).unwrap().code, RestoreCode::Failed);
    assert_eq!(env.marker_reason().as_deref(), Some("tray_exit"));
    env.desk.lock().unwrap().fail_read = false;
    env.forget_exit_theme_save(WallpaperTheme::Astrolabe);
    env.render_count()
}

/// [medium F1] 啟動時狀態檔暫時讀不到：不接管、不渲染，依退避重讀；第一次讀到時照啟動規則判定——
/// 有標記就續做還原，從頭到尾都不渲染。
#[test]
fn unavailable_state_at_startup_with_marker_resumes_and_never_renders() {
    let env = Env::new("b-fix2-unavailable", WallpaperTheme::Astrolabe);
    let renders_before = leave_tray_exit_marker(&env);
    let lock = lock_state_file(&env.paths.state_file);
    let mut c = env.coordinator();
    let t0 = env.shared.clock.lock().unwrap().wall;
    let out = env.step(&mut c, &[Wake::Startup]);
    assert_eq!(c.status().state, CoordinatorState::WaitingForStateFile);
    assert_eq!(out.next_wake, Some(t0 + 120), "{}", env.log_text());
    for wake in [Wake::DataChanged, Wake::ComRecovered, Wake::DisplayChanged] {
        env.step(&mut c, &[wake]);
    }
    env.advance_to(t0 + 60);
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(env.render_count(), renders_before, "{}", env.log_text());

    drop(lock);
    env.advance_to(t0 + 120);
    env.step(&mut c, &[Wake::Timer]);
    env.assert_originals();
    assert_eq!(env.state_json()["status"], "not_taken_over");
    assert_eq!(env.marker_reason(), None);
    assert!(env.has_log("還原中斷續做"), "{}", env.log_text());
    assert!(
        env.effects()
            .contains(&format!("theme_none {:?}", ThemeNoneCause::RestoreResumed)),
        "{:?}",
        env.effects()
    );
    env.advance_to(taipei(10, 15, 0));
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(
        env.render_count(),
        renders_before,
        "從頭到尾不渲染\n{}",
        env.log_text()
    );
}

/// [F1] 狀態檔讀不到期間使用者改選主題：不清標記，排在續做之後——續做完成才照新主題接管。
#[test]
fn theme_change_while_the_state_file_is_unreadable_is_queued_behind_the_resume() {
    let env = Env::new("b-fix2-unavailable-reselect", WallpaperTheme::Astrolabe);
    let renders_before = leave_tray_exit_marker(&env);
    let lock = lock_state_file(&env.paths.state_file);
    let mut c = env.coordinator();
    let t0 = env.shared.clock.lock().unwrap().wall;
    env.step(&mut c, &[Wake::Startup]);
    env.set_theme(WallpaperTheme::Skyline);
    env.shared.inputs.lock().unwrap().data.twii_daily = CivilDate::parse_iso("2026-10-02");
    env.step(&mut c, &[Wake::SettingsChanged]);
    assert_eq!(env.render_count(), renders_before);
    drop(lock);
    assert_eq!(
        env.marker_reason().as_deref(),
        Some("tray_exit"),
        "標記不得被放棄"
    );

    env.advance_to(t0 + 120);
    env.step(&mut c, &[Wake::Timer]);
    let text = env.log_text();
    let resumed = text.find("還原中斷續做完成").expect("先續做");
    assert!(
        text[resumed..].contains("Redraw 開始：主題 skyline"),
        "續做完成之後才照新主題接管\n{text}"
    );
    assert!(
        !text[..resumed].contains("Redraw 開始：主題 skyline"),
        "{text}"
    );
    assert!(
        !env.effects().iter().any(|e| e.starts_with("theme_none")),
        "{:?}",
        env.effects()
    );
    assert_eq!(env.render_count(), renders_before + 2);
}

/// [low F2] Redraw 中途收到工作階段結束：在下一個檢查點中止，已渲染的不送、其餘不渲染。
#[test]
fn session_end_arriving_mid_redraw_aborts_it() {
    let env = Env::new("b-fix2-session-mid", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    let sets_before = env.set_calls().len();
    let inbox = Arc::clone(&env.inbox);
    let mut fired = false;
    *env.shared.render_hook.lock().unwrap() = Some(Box::new(move |_req| {
        if !fired {
            fired = true;
            inbox.push(Wake::SessionEnding);
        }
    }));
    env.advance_to(taipei(10, 15, 0));
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(env.render_count(), 3, "第二台不渲染\n{}", env.log_text());
    assert_eq!(env.set_calls().len(), sets_before, "{:?}", env.set_calls());
    assert!(
        env.has_log("Redraw 中止（工作階段結束）"),
        "{}",
        env.log_text()
    );
    assert_eq!(c.status().state, CoordinatorState::SessionEnding);
}

/// [F3] 系統匣結束的還原失敗後關閉的迴圈：要求被丟棄（回覆接口釋放，命令列端回「交接失敗」）。
#[test]
fn request_after_a_failed_tray_exit_is_dropped() {
    let env = Env::new("b-fix2-closed-failed", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    env.desk.lock().unwrap().fail_read = true;
    let (handle, join) = spawn_coordinator(move || Ok(c)).unwrap();
    let wait = exit_and_restore(&handle, ExitLimits::default());
    join.join().unwrap();
    assert!(
        matches!(&wait, ExitWait::Finished(r) if r.code == RestoreCode::Failed),
        "{wait:?}"
    );
    let dropped = Arc::new(AtomicBool::new(false));
    struct OnDrop(Arc<AtomicBool>);
    impl Drop for OnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let guard = OnDrop(Arc::clone(&dropped));
    let (reply, got) = collector();
    let mut reply = Some(reply);
    handle.request_restore(Request::CommandLineRestore(Box::new(move |p| {
        let _keep = &guard;
        if let Some(r) = reply.as_mut() {
            r(p);
        }
    })));
    assert!(dropped.load(Ordering::SeqCst), "要求應被丟棄");
    assert!(finished(&got).is_none());
}

/// [F5] 結果分類：有待還原螢幕＝部分還原（非 0）；使用者自選、沒有東西要還原＝0。
#[test]
fn partially_restored_has_its_own_exit_code() {
    let partial = RestoreReport::from_result(&RestoreResult::Restored {
        offline_unverified: vec!["X".into()],
        user_choice_kept: Vec::new(),
        warnings: Vec::new(),
        state_saved: true,
    });
    assert_eq!(partial.code, RestoreCode::PartiallyRestored);
    assert_ne!(partial.code.exit_code(), 0);
    let kept = RestoreReport::from_result(&RestoreResult::Restored {
        offline_unverified: Vec::new(),
        user_choice_kept: vec!["Y".into()],
        warnings: Vec::new(),
        state_saved: true,
    });
    assert_eq!(kept.code, RestoreCode::Restored);
    assert_eq!(
        RestoreReport::from_result(&RestoreResult::NothingToRestore).code,
        RestoreCode::Restored
    );
}

/// [F5] 找不到狀態檔仍是 0，但記下解析出的狀態檔路徑與使用者，安裝檔的記錄看得出用了哪個設定檔。
#[test]
fn command_line_without_a_state_file_logs_the_resolved_path_and_user() {
    let env = Env::new("b-fix2-no-state", WallpaperTheme::Astrolabe);
    let report = restore_for_command_line(env.command_line_parts(&env.dir.join("settings.json")));
    assert_eq!(report.code, RestoreCode::Restored);
    let state = env.paths.state_file.display().to_string();
    assert!(
        env.logs()
            .iter()
            .any(|(_, m)| m.contains("找不到狀態檔") && m.contains(&state) && m.contains("使用者")),
        "{}",
        env.log_text()
    );
}

// ---------------------------------------------------------------------------------------------
// task 4.7b 修正輪 3（複審 task-4.7b-rereview-1.md N1／N2／N4）
// ---------------------------------------------------------------------------------------------

/// [N1] 等狀態檔期間工作階段開始結束：狀態檔之後讀得到了也不做啟動判定、不還原（工作階段結束 MUST NOT
/// 還原），標記留給下次啟動；同一批喚醒裡同時有「結束」也一樣。結束被取消後才續做。
#[test]
fn deferred_startup_decision_does_not_restore_while_the_session_is_ending() {
    let env = Env::new("b-fix3-session-wait", WallpaperTheme::Astrolabe);
    let renders_before = leave_tray_exit_marker(&env);
    let host_a = env.wallpaper(DEV_A);
    assert!(is_host(&host_a, &env));
    let lock = lock_state_file(&env.paths.state_file);
    let mut c = env.coordinator();
    let t0 = env.shared.clock.lock().unwrap().wall;
    env.step(&mut c, &[Wake::Startup]);
    let attempts = env.count_logs("還原原桌布：原因");
    drop(lock);
    env.advance_to(t0 + 180);
    // 重讀時刻已到、檔案也讀得到，但同一批喚醒帶著工作階段結束。
    env.step(&mut c, &[Wake::Timer, Wake::SessionEnding]);
    env.advance_to(t0 + 600);
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(
        env.count_logs("還原原桌布：原因"),
        attempts,
        "工作階段結束中不還原\n{}",
        env.log_text()
    );
    assert_eq!(env.wallpaper(DEV_A), host_a);
    assert_eq!(
        env.marker_reason().as_deref(),
        Some("tray_exit"),
        "標記留給下次啟動"
    );
    assert_eq!(c.status().state, CoordinatorState::SessionEnding);

    env.step(&mut c, &[Wake::SessionEndCancelled]);
    env.assert_originals();
    assert_eq!(env.marker_reason(), None);
    assert_eq!(env.render_count(), renders_before);
}

/// [N2] 系統匣結束已還原完、迴圈關閉後才到的命令列要求：不走捷徑，丟棄（宿主端回「交接失敗」）；
/// 命令列等宿主結束後在本行程還原——主題存成「不接管」，結果與另外兩條路相同（升級重啟不會再接管）。
#[test]
fn command_line_after_a_successful_tray_exit_ends_with_theme_none_persisted() {
    let env = Env::new("b-fix3-closed-ok", WallpaperTheme::Astrolabe);
    let settings_path = env.dir.join("settings.json");
    crate::settings::save(
        &settings_path,
        &crate::settings::Settings {
            wallpaper_theme: WallpaperTheme::Astrolabe,
            ..crate::settings::Settings::default()
        },
    )
    .unwrap();
    let c = env.coordinator();
    let (handle, join) = spawn_coordinator(move || Ok(c)).unwrap();
    let wait = exit_and_restore(&handle, ExitLimits::default());
    join.join().unwrap();
    assert!(
        matches!(&wait, ExitWait::Finished(r) if r.code == RestoreCode::Restored),
        "{wait:?}"
    );
    let (reply, got) = collector();
    handle.request_restore(Request::CommandLineRestore(reply));
    assert!(finished(&got).is_none(), "不走捷徑：要求被丟棄");

    // 命令列的後備（宿主結束後在本行程還原）。
    let report = restore_for_command_line(env.command_line_parts(&settings_path));
    assert_eq!(report.code, RestoreCode::Restored, "{report:?}");
    assert_eq!(
        crate::settings::load_for_arbitrated_startup(&settings_path, false)
            .settings
            .wallpaper_theme,
        WallpaperTheme::None,
        "主題存成「不接管」"
    );
    env.assert_originals();
}

/// [N4] 等狀態檔期間收到命令列交接：讀不到 → 回 1、主題改不接管；協調迴圈層的 StateUnavailable 還原
/// 依 retry_delay 退避（120、再 240 秒）；讀到時先續做、再完成重試，全程不渲染。
#[test]
fn command_line_handoff_while_waiting_for_the_state_file_backs_off_and_completes() {
    let env = Env::new("b-fix3-cli-wait", WallpaperTheme::Astrolabe);
    let renders_before = leave_tray_exit_marker(&env);
    let lock = lock_state_file(&env.paths.state_file);
    let mut c = env.coordinator();
    let t0 = env.shared.clock.lock().unwrap().wall;
    env.step(&mut c, &[Wake::Startup]);
    let (reply, got) = collector();
    assert_eq!(
        c.handle_request(Request::CommandLineRestore(reply)),
        LoopControl::Continue
    );
    assert_eq!(finished(&got).map(|r| r.code), Some(RestoreCode::Failed));
    assert!(
        env.effects()
            .contains(&format!("theme_none {:?}", ThemeNoneCause::CommandLine)),
        "{:?}",
        env.effects()
    );
    env.step(&mut c, &[Wake::SettingsChanged]);
    assert_eq!(
        c.status().restore_retry_at,
        Some(t0 + 120),
        "{}",
        env.log_text()
    );
    assert_eq!(c.status().state, CoordinatorState::WaitingForStateFile);

    env.advance_to(t0 + 120);
    let out = env.step(&mut c, &[Wake::Timer]);
    assert_eq!(
        c.status().restore_retry_at,
        Some(t0 + 120 + 240),
        "第二次失敗間隔加倍\n{}",
        env.log_text()
    );
    assert_eq!(out.next_wake, Some(t0 + 360));

    drop(lock);
    env.advance_to(t0 + 360);
    env.step(&mut c, &[Wake::Timer]);
    env.assert_originals();
    assert_eq!(env.state_json()["status"], "not_taken_over");
    assert_eq!(env.marker_reason(), None);
    assert_eq!(c.status().restore_retry_at, None);
    assert_eq!(env.render_count(), renders_before);
}

/// [N4] 等狀態檔期間系統匣結束：仍讀不到 → 回報失敗、標記留著；讀得到了（重讀時刻還沒到）→ 照常還原。
#[test]
fn tray_quit_while_waiting_for_the_state_file() {
    let env = Env::new("b-fix3-tray-wait", WallpaperTheme::Astrolabe);
    leave_tray_exit_marker(&env);
    {
        let lock = lock_state_file(&env.paths.state_file);
        let mut c = env.coordinator();
        env.step(&mut c, &[Wake::Startup]);
        let (reply, got) = collector();
        assert_eq!(
            c.handle_request(Request::ExitAndRestore(reply)),
            LoopControl::Exit
        );
        assert_eq!(finished(&got).map(|r| r.code), Some(RestoreCode::Failed));
        drop(lock);
    }
    assert_eq!(env.marker_reason().as_deref(), Some("tray_exit"));

    let lock = lock_state_file(&env.paths.state_file);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    assert_eq!(c.status().state, CoordinatorState::WaitingForStateFile);
    drop(lock);
    let (reply, got) = collector();
    c.handle_request(Request::ExitAndRestore(reply));
    assert_eq!(finished(&got).map(|r| r.code), Some(RestoreCode::Restored));
    env.assert_originals();
    assert_eq!(env.marker_reason(), None);
}

// ---------------------------------------------------------------------------------------------
// task 4.9：explorer 資源安全閥（design.md D11；spec「explorer 資源安全閥」）
// ---------------------------------------------------------------------------------------------

fn gdi_sample(pid: u32, gdi: u32) -> ExplorerSample {
    ExplorerSample { pid, gdi }
}

impl Env {
    fn set_explorer(&self, reading: Result<ExplorerSample, String>) {
        *self.shared.explorer.lock().unwrap() = reading;
    }

    fn explorer_reads(&self) -> usize {
        self.shared.explorer_reads.load(Ordering::SeqCst)
    }

    /// 狀態檔的基準（PID、GDI）；沒有欄位時 `None`。
    fn baseline(&self) -> Option<(u64, u64)> {
        let v = self.state_json();
        let b = v.get("explorer_baseline")?;
        Some((b["pid"].as_u64().unwrap(), b["gdi"].as_u64().unwrap()))
    }

    fn count_effects(&self, prefix: &str) -> usize {
        self.effects()
            .iter()
            .filter(|e| e.starts_with(prefix))
            .count()
    }

    fn assert_originals_restored(&self) {
        assert_eq!(
            self.wallpaper(DEV_A).as_deref(),
            Some(self.orig("a").as_str()),
            "{}",
            self.log_text()
        );
        assert_eq!(
            self.wallpaper(DEV_B).as_deref(),
            Some(self.orig("b").as_str())
        );
        assert_eq!(self.desk.lock().unwrap().position, 3, "全域填滿方式還原");
    }
}

const PID: u32 = EXPLORER_PID;

/// spec Scenario「GDI 增量超過門檻」：多出 2,001 → 不再設定桌布、所有螢幕還原原桌布與填滿方式、主題
/// 改「不接管」、一則通知；記錄一行含讀值、基準、PID、門檻與決策；排程器 reset。
#[test]
fn safety_valve_trip_stops_takeover_restores_and_notifies_once() {
    let env = Env::new("valve-trip", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    assert_eq!(env.render_count(), 2, "{}", env.log_text());
    assert_eq!(
        env.baseline(),
        Some((u64::from(PID), 500)),
        "接管開始時的讀值為基準：{}",
        env.log_text()
    );
    let generation = c.scheduler_generation();

    env.set_explorer(Ok(gdi_sample(PID, 2_501)));
    env.advance_to(taipei(10, 15, 0));
    env.step(&mut c, &[Wake::Timer]);

    assert_eq!(env.render_count(), 2, "觸發後不渲染：{}", env.log_text());
    env.assert_originals_restored();
    assert_eq!(env.state_json()["status"], "not_taken_over");
    assert_eq!(env.baseline(), None, "還原後清除基準");
    assert!(c.scheduler_generation() > generation, "排程器 reset");
    assert_eq!(
        env.effects().first().map(String::as_str),
        Some("theme_none SafetyValve"),
        "{:?}",
        env.effects()
    );
    assert_eq!(
        env.count_effects("notify SafetyValve"),
        1,
        "{:?}",
        env.effects()
    );
    let notice = env
        .effects()
        .into_iter()
        .find(|e| e.starts_with("notify SafetyValve"))
        .unwrap();
    for needle in ["4242", "2501", "500", "2001", "2000", "8000"] {
        assert!(notice.contains(needle), "通知缺 {needle}：{notice}");
    }
    assert_eq!(c.status().notices.len(), 1);
    assert!(
        env.logs().iter().any(|(level, m)| *level == Level::Warn
            && m.contains("安全閥觸發")
            && m.contains("PID 4242")
            && m.contains("2501")
            && m.contains("500")
            && m.contains("2000")
            && m.contains("8000")
            && m.contains("停止接管")),
        "{}",
        env.log_text()
    );

    // 之後主題是「不接管」：不再讀 explorer、不再渲染、不再通知。
    let reads = env.explorer_reads();
    env.advance_to(taipei(10, 30, 0));
    env.step(&mut c, &[Wake::Timer]);
    env.step(&mut c, &[Wake::DataChanged]);
    assert_eq!(env.render_count(), 2);
    assert_eq!(env.explorer_reads(), reads);
    assert_eq!(env.count_effects("notify"), 1);
    assert_eq!(c.status().state, CoordinatorState::Idle);
}

#[test]
fn safety_valve_delta_of_exactly_2000_keeps_drawing() {
    let env = Env::new("valve-2000", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    env.set_explorer(Ok(gdi_sample(PID, 2_500)));
    env.advance_to(taipei(10, 15, 0));
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(env.render_count(), 4, "{}", env.log_text());
    assert!(env.effects().is_empty(), "{:?}", env.effects());
    assert_eq!(env.baseline(), Some((u64::from(PID), 500)), "基準不變");
    assert!(
        env.has_log("explorer GDI：PID 4242 讀值 2500"),
        "每次設定前的讀值都記一行：{}",
        env.log_text()
    );
}

/// 絕對值超過 8,000：接管開始前（沒有基準）也不接管——不渲染、不設定、主題改「不接管」並通知。
#[test]
fn safety_valve_absolute_limit_blocks_even_the_first_takeover() {
    let env = Env::new("valve-abs-first", WallpaperTheme::Astrolabe);
    env.set_explorer(Ok(gdi_sample(PID, 8_001)));
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    assert_eq!(env.render_count(), 0, "{}", env.log_text());
    assert!(env.set_calls().is_empty(), "{:?}", env.set_calls());
    assert_eq!(
        env.wallpaper(DEV_A).as_deref(),
        Some(env.orig("a").as_str())
    );
    assert_eq!(env.count_effects("theme_none SafetyValve"), 1);
    assert_eq!(env.count_effects("notify SafetyValve"), 1);
    let notice = env.effects().join("\n");
    assert!(
        notice.contains("8001") && notice.contains("尚無基準"),
        "{notice}"
    );
}

/// 讀取失敗：照常設定、不停止；同一段連續失敗只記一次 warn，恢復時記一行；恢復後第一次讀值成為基準。
#[test]
fn safety_valve_read_failure_keeps_drawing_and_warns_once_per_streak() {
    let env = Env::new("valve-readfail", WallpaperTheme::Astrolabe);
    env.set_explorer(Err("假的讀取失敗".into()));
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    env.advance_to(taipei(10, 15, 0));
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(env.render_count(), 4, "{}", env.log_text());
    assert!(env.effects().is_empty(), "{:?}", env.effects());
    assert_eq!(
        env.logs()
            .iter()
            .filter(|(l, m)| *l == Level::Warn && m.contains("讀取 explorer GDI 失敗"))
            .count(),
        1,
        "{}",
        env.log_text()
    );
    assert!(env.has_log("假的讀取失敗"));
    assert_eq!(env.baseline(), None, "沒有讀值就不設基準");

    env.set_explorer(Ok(gdi_sample(PID, 900)));
    env.advance_to(taipei(10, 30, 0));
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(env.render_count(), 6);
    assert!(env.has_log("讀取 explorer GDI 恢復"), "{}", env.log_text());
    assert_eq!(env.baseline(), Some((u64::from(PID), 900)));

    // 再失敗是新的一段：再記一次。
    env.set_explorer(Err("又失敗".into()));
    env.advance_to(taipei(10, 45, 0));
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(
        env.count_logs("讀取 explorer GDI 失敗"),
        2,
        "{}",
        env.log_text()
    );
}

/// spec Scenario「explorer 重新啟動」：PID 改變、GDI 降回較低的值 → 以新讀值為基準，照常接管。
#[test]
fn safety_valve_pid_change_rebaselines_and_keeps_taking_over() {
    let env = Env::new("valve-pid", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    env.set_explorer(Ok(gdi_sample(5151, 2_600)));
    env.advance_to(taipei(10, 15, 0));
    env.step(&mut c, &[Wake::ExplorerRestarted, Wake::Timer]);
    assert_eq!(env.render_count(), 4, "{}", env.log_text());
    assert!(env.effects().is_empty(), "{:?}", env.effects());
    assert_eq!(env.baseline(), Some((5151, 2_600)));
    assert!(env.has_log("PID 改變"), "{}", env.log_text());

    // 之後以新基準判定。
    env.set_explorer(Ok(gdi_sample(5151, 4_601)));
    env.advance_to(taipei(10, 30, 0));
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(
        env.count_effects("notify SafetyValve"),
        1,
        "{}",
        env.log_text()
    );
}

/// 宿主重啟：PID 相同沿用狀態檔的基準（重啟後第一次設定前就判定），PID 不同重設。
#[test]
fn host_restart_keeps_baseline_for_same_pid_and_resets_for_new_pid() {
    let env = Env::new("valve-restart-same", WallpaperTheme::Astrolabe);
    drop({
        let mut c = env.coordinator();
        env.step(&mut c, &[Wake::Startup]);
        c
    });
    assert_eq!(env.baseline(), Some((u64::from(PID), 500)));
    env.set_explorer(Ok(gdi_sample(PID, 2_501)));
    env.advance_to(taipei(10, 6, 0));
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    assert_eq!(
        env.render_count(),
        2,
        "重啟後沿用基準 500 而觸發：{}",
        env.log_text()
    );
    assert_eq!(env.count_effects("notify SafetyValve"), 1);
    env.assert_originals_restored();

    let env = Env::new("valve-restart-new", WallpaperTheme::Astrolabe);
    drop({
        let mut c = env.coordinator();
        env.step(&mut c, &[Wake::Startup]);
        c
    });
    env.set_explorer(Ok(gdi_sample(5151, 2_501)));
    env.advance_to(taipei(10, 6, 0));
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    assert_eq!(env.render_count(), 4, "{}", env.log_text());
    assert!(env.effects().is_empty(), "{:?}", env.effects());
    assert_eq!(env.baseline(), Some((5151, 2_501)));
}

/// ledger（4.7a 複審 note for 4.9）：安全閥的還原失敗 → 保留重試（依 retry_delay），重試不重複通知、
/// 不重複改主題；最後還原成功。
#[test]
fn safety_valve_restore_failure_is_retried_without_repeating_the_notice() {
    let env = Env::new("valve-restore-fail", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    env.desk.lock().unwrap().fail_read = true;
    env.set_explorer(Ok(gdi_sample(PID, 2_501)));
    let t0 = taipei(10, 15, 0);
    env.advance_to(t0);
    let out = env.step(&mut c, &[Wake::Timer]);
    assert_eq!(
        env.count_effects("notify SafetyValve"),
        1,
        "{:?}",
        env.effects()
    );
    assert_eq!(env.count_effects("theme_none SafetyValve"), 1);
    assert_eq!(
        c.status().restore_retry_at,
        Some(t0 + 120),
        "還原失敗要保留重試：{}",
        env.log_text()
    );
    assert_eq!(out.next_wake, Some(t0 + 120));
    assert_eq!(env.state_json()["status"], "taken_over");

    env.advance_to(t0 + 120);
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(
        c.status().restore_retry_at,
        Some(t0 + 120 + 240),
        "{}",
        env.log_text()
    );
    assert_eq!(
        env.count_effects("notify"),
        1,
        "重試不重複通知：{:?}",
        env.effects()
    );
    assert_eq!(env.count_effects("theme_none"), 1, "{:?}", env.effects());

    env.desk.lock().unwrap().fail_read = false;
    env.advance_to(t0 + 360);
    env.step(&mut c, &[Wake::Timer]);
    env.assert_originals_restored();
    assert_eq!(c.status().restore_retry_at, None);
    assert_eq!(env.count_effects("notify"), 1, "{:?}", env.effects());
    assert_eq!(c.status().notices.len(), 1);
    assert_eq!(env.render_count(), 2);
}

/// 使用者重新開啟接管（主題從「不接管」改回某個主題）時重設基準：安全閥的還原失敗、狀態檔仍是接管中
/// 且留著舊基準，選回主題後以當下讀值為新基準，不會立刻又觸發。
#[test]
fn reopening_takeover_resets_the_baseline() {
    let env = Env::new("valve-reopen", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    env.desk.lock().unwrap().fail_read = true;
    env.set_explorer(Ok(gdi_sample(PID, 2_501)));
    env.advance_to(taipei(10, 15, 0));
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(
        env.state_json()["status"],
        "taken_over",
        "{}",
        env.log_text()
    );
    assert_eq!(env.baseline(), Some((u64::from(PID), 500)));

    env.desk.lock().unwrap().fail_read = false;
    env.set_explorer(Ok(gdi_sample(PID, 4_000)));
    env.set_theme(WallpaperTheme::Astrolabe);
    env.advance_to(taipei(10, 16, 0));
    env.step(&mut c, &[Wake::SettingsChanged]);
    assert_eq!(env.render_count(), 4, "{}", env.log_text());
    assert_eq!(env.count_effects("notify"), 1, "{:?}", env.effects());
    assert_eq!(env.baseline(), Some((u64::from(PID), 4_000)));
    assert!(env.has_log("使用者重新開啟接管"), "{}", env.log_text());
}

/// 未接管時留下的舊基準（例如上一次嘗試接管時寫入、之後焦點確認被取消）不得在接管開始時被沿用：
/// 接管前讀取失敗時清掉它，下一次讀到的值才是基準。
#[test]
fn stale_baseline_from_before_takeover_is_not_carried_when_the_read_fails() {
    let env = Env::new("valve-stale-pre", WallpaperTheme::Astrolabe);
    fs::write(
        &env.paths.state_file,
        serde_json::json!({
            "version": 1,
            "status": "not_taken_over",
            "original": null,
            "monitors": {},
            "last_event": null,
            "explorer_baseline": { "pid": PID, "gdi": 100, "recorded_at": 1 }
        })
        .to_string(),
    )
    .unwrap();
    env.set_explorer(Err("接管前讀不到".into()));
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    assert_eq!(env.render_count(), 2, "{}", env.log_text());
    assert_eq!(env.state_json()["status"], "taken_over");
    assert_eq!(env.baseline(), None, "{}", env.log_text());

    env.set_explorer(Ok(gdi_sample(PID, 2_200)));
    env.advance_to(taipei(10, 15, 0));
    env.step(&mut c, &[Wake::Timer]);
    assert_eq!(env.render_count(), 4, "{}", env.log_text());
    assert!(env.effects().is_empty(), "{:?}", env.effects());
    assert_eq!(env.baseline(), Some((u64::from(PID), 2_200)));
}

// ---------------------------------------------------------------------------------------------
// task 4.9 修正輪 1（審查 task-4.9-review.md L1、L2）
// ---------------------------------------------------------------------------------------------

/// 先接管（基準＝500），結束這個協調迴圈；回傳時狀態檔為接管中且有基準。
fn take_over_with_baseline(env: &Env) {
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    assert_eq!(env.state_json()["status"], "taken_over");
    assert_eq!(env.baseline(), Some((u64::from(PID), 500)));
}

/// [L1] 主題 A→B（沒有經過「不接管」）不是重新開啟接管：基準保留，累積的增量照算（這裡 2,100 → 觸發）。
#[test]
fn switching_between_themes_keeps_the_baseline() {
    let env = Env::new("valve-r1-a-to-b", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    env.set_explorer(Ok(gdi_sample(PID, 2_600)));
    env.set_theme(WallpaperTheme::Tearoff);
    env.advance_to(taipei(10, 6, 0));
    env.step(&mut c, &[Wake::SettingsChanged]);
    assert!(!env.has_log("使用者重新開啟接管"), "{}", env.log_text());
    assert_eq!(
        env.count_effects("notify SafetyValve"),
        1,
        "基準 500 沿用、增量 2,100 觸發：{}",
        env.log_text()
    );
}

/// [L1] 啟動時狀態檔讀不到、延後判定期間主題 A→B：讀到時不得當成重新開啟接管而清掉基準。
#[test]
fn delayed_startup_decision_after_a_to_b_keeps_the_baseline() {
    let env = Env::new("valve-r1-delayed-a-to-b", WallpaperTheme::Astrolabe);
    take_over_with_baseline(&env);
    let lock = lock_state_file(&env.paths.state_file);
    let mut c = env.coordinator();
    let t0 = env.shared.clock.lock().unwrap().wall;
    env.step(&mut c, &[Wake::Startup]);
    assert_eq!(c.status().state, CoordinatorState::WaitingForStateFile);
    env.set_theme(WallpaperTheme::Tearoff);
    env.step(&mut c, &[Wake::SettingsChanged]);
    drop(lock);

    env.set_explorer(Ok(gdi_sample(PID, 2_600)));
    env.advance_to(t0 + 120);
    env.step(&mut c, &[Wake::Timer]);
    assert!(env.has_log("做延後的啟動判定"), "{}", env.log_text());
    assert!(!env.has_log("使用者重新開啟接管"), "{}", env.log_text());
    assert_eq!(
        env.count_effects("notify SafetyValve"),
        1,
        "基準 500 沿用、增量 2,100 觸發：{}",
        env.log_text()
    );
}

/// [L1] 啟動時狀態檔讀不到、延後判定期間主題「不接管」→A＝重新開啟接管：讀到時清掉舊基準，以當下讀值
/// 為新基準（不觸發）。
#[test]
fn delayed_startup_decision_after_none_to_a_resets_the_baseline() {
    let env = Env::new("valve-r1-delayed-none-to-a", WallpaperTheme::Astrolabe);
    take_over_with_baseline(&env);
    env.set_theme(WallpaperTheme::None);
    let lock = lock_state_file(&env.paths.state_file);
    let mut c = env.coordinator();
    let t0 = env.shared.clock.lock().unwrap().wall;
    env.step(&mut c, &[Wake::Startup]);
    assert_eq!(c.status().state, CoordinatorState::WaitingForStateFile);
    env.set_theme(WallpaperTheme::Astrolabe);
    env.step(&mut c, &[Wake::SettingsChanged]);
    drop(lock);

    env.set_explorer(Ok(gdi_sample(PID, 2_600)));
    env.advance_to(t0 + 120);
    env.step(&mut c, &[Wake::Timer]);
    assert!(env.has_log("使用者重新開啟接管"), "{}", env.log_text());
    assert!(env.effects().is_empty(), "{:?}", env.effects());
    assert_eq!(
        env.baseline(),
        Some((u64::from(PID), 2_600)),
        "{}",
        env.log_text()
    );
}

/// [L2] 安全閥的還原回 `StateUnavailable`（狀態檔暫時讀不到）：依 retry_delay 保留重試；主題改「不接管」
/// 與通知各只一次（重試不再套用）；狀態檔恢復可讀後還原成功。
///
/// 協調迴圈裡，啟動判定要等狀態檔讀到才做，之後記憶體中的狀態不會再變回「讀不到」，所以評估中觸發
/// 的安全閥碰不到這個分支；這裡直接以 `restore_now` 驅動 `restore_with`（ledger 修正的所在），鎖住
/// 狀態檔讓它回 `StateUnavailable`，鎖定這個分支的行為。
#[test]
fn safety_valve_restore_state_unavailable_keeps_retry_and_notifies_once() {
    let env = Env::new("valve-r1-unavailable", WallpaperTheme::Astrolabe);
    take_over_with_baseline(&env);
    let lock = lock_state_file(&env.paths.state_file);
    let mut c = env.coordinator();
    let t0 = env.shared.clock.lock().unwrap().wall;
    let reason = RestoreReason::SafetyValve {
        detail: "explorer（PID 4242）的 GDI 物件數 2501，比基準 500 多出 2001".to_owned(),
    };

    let result = c.restore_now(reason.clone());
    assert_eq!(
        result,
        RestoreResult::StateUnavailable,
        "{}",
        env.log_text()
    );
    assert_eq!(
        c.restore_retry.as_ref().map(|r| (r.at, r.failures)),
        Some((t0 + retry_delay(1), 1)),
        "StateUnavailable 要保留依 retry_delay 的重試：{}",
        env.log_text()
    );
    assert_eq!(
        env.count_effects("theme_none SafetyValve"),
        1,
        "{:?}",
        env.effects()
    );
    assert_eq!(
        env.count_effects("notify SafetyValve"),
        1,
        "{:?}",
        env.effects()
    );

    env.advance_to(t0 + retry_delay(1));
    let result = c.restore_now(reason.clone());
    assert_eq!(result, RestoreResult::StateUnavailable);
    assert_eq!(
        c.restore_retry.as_ref().map(|r| (r.at, r.failures)),
        Some((t0 + retry_delay(1) + retry_delay(2), 2)),
        "{}",
        env.log_text()
    );
    assert_eq!(
        env.count_effects("theme_none"),
        1,
        "重試不重複改主題：{:?}",
        env.effects()
    );
    assert_eq!(
        env.count_effects("notify"),
        1,
        "重試不重複通知：{:?}",
        env.effects()
    );

    drop(lock);
    env.advance_to(t0 + retry_delay(1) + retry_delay(2));
    let result = c.restore_now(reason);
    assert!(
        matches!(result, RestoreResult::Restored { .. }),
        "{result:?}\n{}",
        env.log_text()
    );
    assert!(c.restore_retry.is_none());
    env.assert_originals_restored();
    assert_eq!(env.state_json()["status"], "not_taken_over");
    assert_eq!(env.count_effects("theme_none"), 1, "{:?}", env.effects());
    assert_eq!(env.count_effects("notify"), 1, "{:?}", env.effects());
}

// ---------------------------------------------------------------------------------------------
// installer-auto-update task 3.3：「因更新結束」（不還原、不動狀態檔與主題；處理完迴圈結束）
// ---------------------------------------------------------------------------------------------

/// 「因更新結束」：狀態檔位元組、主題、桌布 API 呼叫紀錄、副作用紀錄（含 `theme_none`）全都沒有變化，
/// 迴圈結束並回報 `ExitedForUpdate`；沒有「還原進行中」標記、沒有還原。對照
/// `tray_exit_request_restores_then_ends_the_loop`（同一個環境下系統匣「結束」會還原並存「不接管」）。
#[test]
fn exit_for_update_request_changes_nothing_and_ends_the_loop() {
    let env = Env::new("u33-exit-for-update", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    assert!(is_host(&env.wallpaper(DEV_A), &env));
    let state_before = fs::read(&env.paths.state_file).unwrap();
    let calls_before = env.calls();
    let effects_before = env.effects();
    let theme_before = env.shared.inputs.lock().unwrap().theme;
    let position_before = env.desk.lock().unwrap().position;

    let (reply, got) = collector();
    assert_eq!(
        c.handle_request(Request::ExitForUpdate(reply)),
        LoopControl::Exit
    );

    assert_eq!(
        got.lock().unwrap().clone(),
        vec![RequestProgress::ExitedForUpdate]
    );
    assert_eq!(
        fs::read(&env.paths.state_file).unwrap(),
        state_before,
        "狀態檔位元組不變"
    );
    assert_eq!(env.state_json()["status"], "taken_over");
    assert_eq!(env.marker_reason(), None, "不寫還原進行中標記");
    assert_eq!(env.calls(), calls_before, "沒有任何桌布 API 呼叫（含還原）");
    assert_eq!(env.desk.lock().unwrap().position, position_before);
    assert!(
        is_host(&env.wallpaper(DEV_A), &env),
        "仍是宿主的桌布，沒有還原"
    );
    assert_eq!(env.effects(), effects_before, "沒有存主題等副作用");
    assert!(
        !env.effects().iter().any(|e| e.starts_with("theme_none")),
        "{:?}",
        env.effects()
    );
    assert_eq!(env.shared.inputs.lock().unwrap().theme, theme_before);
    assert!(env.has_log("因更新結束"), "{}", env.log_text());
    assert!(
        !env.has_log("還原原桌布：原因"),
        "不得走還原路徑\n{}",
        env.log_text()
    );
}

/// 協調迴圈正處在「工作階段結束」狀態（Restart Manager 先通知了）時，要求照樣處理、迴圈結束。
#[test]
fn exit_for_update_request_is_handled_while_session_ending() {
    let env = Env::new("u33-exit-during-session-end", WallpaperTheme::Astrolabe);
    let mut c = env.coordinator();
    env.step(&mut c, &[Wake::Startup]);
    env.step(&mut c, &[Wake::SessionEnding]);
    let state_before = fs::read(&env.paths.state_file).unwrap();
    let calls_before = env.calls();
    let (reply, got) = collector();
    assert_eq!(
        c.handle_request(Request::ExitForUpdate(reply)),
        LoopControl::Exit
    );
    assert_eq!(
        got.lock().unwrap().clone(),
        vec![RequestProgress::ExitedForUpdate]
    );
    assert_eq!(fs::read(&env.paths.state_file).unwrap(), state_before);
    assert_eq!(env.calls(), calls_before);
}

/// 真正的協調執行緒：`exit_for_update` 回 `Exited`、執行緒結束、之後的要求立即 `Disconnected`；
/// 狀態檔不變、沒有還原進行中標記（`request_exit_for_update` 只推要求、不寫標記）。
#[test]
fn exit_for_update_through_the_real_loop_leaves_state_untouched() {
    let env = Env::new("u33-exit-real-loop", WallpaperTheme::Astrolabe);
    let c = env.coordinator();
    let (handle, join) = spawn_coordinator(move || Ok(c)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while handle.status().state != CoordinatorState::UpToDate && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    let state_before = fs::read(&env.paths.state_file).unwrap();
    let calls_before = env.calls();

    assert_eq!(
        exit_for_update(&handle, Duration::from_secs(5)),
        ExitForUpdateWait::Exited,
        "{}",
        env.log_text()
    );
    join.join().unwrap();
    assert_eq!(fs::read(&env.paths.state_file).unwrap(), state_before);
    assert_eq!(env.marker_reason(), None);
    assert_eq!(env.calls(), calls_before);
    assert!(
        !env.effects().iter().any(|e| e.starts_with("theme_none")),
        "{:?}",
        env.effects()
    );
    assert_eq!(env.state_json()["status"], "taken_over");
    assert_eq!(
        exit_for_update(&handle, Duration::from_secs(5)),
        ExitForUpdateWait::Disconnected,
        "迴圈已結束，信箱關閉"
    );
}

/// 渲染卡在 explorer／頁面裡：時限內等不到就放棄（`TimedOut`），宿主照常結束；狀態檔與主題沒動、沒有標記。
/// 渲染放行後迴圈在下一個檢查點處理要求並結束（不會重新排程渲染）。
#[test]
fn exit_for_update_with_a_render_in_flight_times_out_without_touching_state() {
    let env = Env::new("u33-exit-inflight", WallpaperTheme::Astrolabe);
    let (handle, join, gate) = spawn_and_block_next_render(&env);
    let state_before = fs::read(&env.paths.state_file).unwrap();

    assert_eq!(
        exit_for_update(&handle, Duration::from_millis(100)),
        ExitForUpdateWait::TimedOut,
        "{}",
        env.log_text()
    );
    assert_eq!(fs::read(&env.paths.state_file).unwrap(), state_before);
    assert_eq!(env.marker_reason(), None, "沒有先寫還原進行中標記");
    assert!(is_host(&env.wallpaper(DEV_A), &env), "沒有還原");

    gate.release();
    join.join().unwrap();
    assert_eq!(env.marker_reason(), None);
    assert!(
        !env.effects().iter().any(|e| e.starts_with("theme_none")),
        "{:?}",
        env.effects()
    );
    assert_eq!(env.state_json()["status"], "taken_over");
}

/// 純等待邏輯：收到 `ExitedForUpdate` 才算完成；其他進度忽略；逾時與斷線分開回報。
#[test]
fn wait_exit_for_update_distinguishes_exited_timeout_and_disconnected() {
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(RequestProgress::Started).unwrap();
    tx.send(RequestProgress::ExitedForUpdate).unwrap();
    assert_eq!(
        wait_exit_for_update(&rx, Duration::from_secs(1)),
        ExitForUpdateWait::Exited
    );
    assert_eq!(
        wait_exit_for_update(&rx, Duration::from_millis(30)),
        ExitForUpdateWait::TimedOut
    );
    drop(tx);
    assert_eq!(
        wait_exit_for_update(&rx, Duration::from_secs(1)),
        ExitForUpdateWait::Disconnected
    );
}
