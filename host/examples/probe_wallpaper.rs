//! dynamic-wallpaper task 1.1／1.2 探針（拋棄式）：以 `IDesktopWallpaper` 反覆設桌布，
//! 同時量測 explorer 的資源負擔（1.1）與設桌布時廣播給頂層視窗的訊息（1.2）。
//!
//! 背景：動態桌布功能會定時產生 PNG，再以 `IDesktopWallpaper::SetWallpaper` 逐螢幕設定，
//! 最頻繁的主題每 15 分鐘每螢幕一次。本探針以每 10 秒的頻率加速跑，量兩件事：
//!
//! - **1.1**：explorer 是否累積資源。每 30 秒寫一列 CSV（explorer 與探針自身各一組：CPU、
//!   工作集、私有位元組、控制代碼數、GDI／USER 物件數含 peak、工作階段是否鎖定）。
//! - **1.2**：隱藏的一般頂層視窗（不是 message-only 視窗——它收不到廣播）在自己的執行緒跑訊息
//!   迴圈，記錄收到的所有訊息；每次設桌布後 3 秒內收到的訊息依類型計數，基線與冷卻期間收到的
//!   同類訊息另計作對照。
//!
//! 三段時程（每段時長都可用參數調整）：基線只取樣 → 交替設桌布 → 冷卻只取樣，最後還原。
//!
//! task 1.7（GDI 累積受控重測）另有三個選用參數，不給時行為與 1.1 完全相同：`--monitor <idx>`
//! 只設一台螢幕；`--sample-after-set-secs <n>` 設桌布階段改為每次設定後 n 秒取樣一列（基線與冷卻
//! 仍週期取樣）；`--extra-cols` 在 CSV 末尾追加取樣種類、週期序號、閒置秒數（`GetLastInputInfo`，
//! 只讀）、explorer 頂層視窗數與其中可見者（`EnumWindows`＋`GetWindowThreadProcessId`）。
//!
//! 安全性（本探針會改使用者的桌布）：
//!
//! - **備份先於一切**：設定之前先把每台螢幕的 `GetWallpaper`、全域 `GetPosition`／
//!   `GetBackgroundColor`／`GetStatus`、`HKCU\Control Panel\Desktop` 的 `Wallpaper`／
//!   `WallpaperStyle`／`TileWallpaper` 以原子方式（先寫 `.tmp` 再 `rename`）寫進備份 JSON。
//! - **三種結束都還原**：正常結束、Ctrl+C（含關閉主控台視窗）、錯誤中止（含 panic）。還原後
//!   逐項讀回比對並記錄；比對不一致結束碼為 4。也可單獨以 `--restore <備份 JSON>` 還原。
//! - 本探針只對「自己的兩張測試 PNG」呼叫 `SetWallpaper`，不刪除、不覆寫任何使用者圖檔；
//!   測試圖路徑若與備份中任一螢幕的原桌布路徑相同，拒絕執行。
//! - 投影片模式（`GetStatus` 帶 `DSS_SLIDESHOW`）無法由本探針還原，偵測到就拒絕執行（結束碼 3）。
//! - 執行期間以 `SetThreadExecutionState` 要求系統與顯示器保持開啟（Modern Standby 筆電顯示器
//!   一關就鎖定並待機，無人值守的長時間連跑會被凍結）；結束時清除要求。不改任何電源計畫或登錄。
//!
//! 證據檔寫進 `--out-dir`，檔名 `dw-1.1-<tag>.csv`、`dw-1.1-<tag>.log`、`dw-1.2-<tag>.log`；
//! 寫出前把 `%LOCALAPPDATA%`／`%APPDATA%`／`%USERPROFILE%`／`%TEMP%` 改寫成字樣（repo 公開，
//! 證據不得留下使用者設定檔路徑）。備份 JSON 不去識別（還原需要原始路徑），只放在備份目錄。
//!
//! 執行（純 cargo；驅動稿見 `host/tools/probe-dw-1.1.ps1`，用法見 `host/tools/README.md`）：
//! `cargo run --release --example probe_wallpaper -- --help`
//!
//! 結束碼：0 成功；2 參數／環境錯誤（尚未改任何桌布）；3 BLOCKED（鎖定、投影片等，尚未改桌布）；
//! 4 還原後讀回不一致；5 執行中出錯（已還原且一致）。
//!
//! Win32 直接呼叫：本檔是拋棄式探針，不受「Win32 只在 desktop.rs」規則約束（比照既有 `probe_*.rs`）。

use std::{
    collections::{BTreeMap, HashMap},
    env,
    ffi::c_void,
    fs::{self, File},
    io::Write,
    panic::{catch_unwind, AssertUnwindSafe},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU8, Ordering},
        mpsc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use windows::{
    core::{w, Interface, BOOL, PCWSTR, PWSTR},
    Win32::{
        Foundation::{CloseHandle, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM},
        System::{
            Com::{
                CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
                COINIT_APARTMENTTHREADED,
            },
            Console::{
                SetConsoleCtrlHandler, CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT,
            },
            Diagnostics::{
                Debug::ReadProcessMemory,
                ToolHelp::{
                    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
                    TH32CS_SNAPPROCESS,
                },
            },
            LibraryLoader::GetModuleHandleW,
            Power::{
                SetThreadExecutionState, ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED,
            },
            ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX},
            Registry::{
                RegCloseKey, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
                HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ, REG_VALUE_TYPE,
            },
            SystemInformation::{GetLocalTime, GetTickCount},
            Threading::{
                GetCurrentProcess, GetCurrentProcessId, GetGuiResources, GetProcessHandleCount,
                GetProcessTimes, OpenProcess, GR_GDIOBJECTS, GR_GDIOBJECTS_PEAK, GR_USEROBJECTS,
                GR_USEROBJECTS_PEAK, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
            },
        },
        UI::{
            Shell::{
                DesktopWallpaper, IDesktopWallpaper, DESKTOP_WALLPAPER_POSITION, DSS_SLIDESHOW,
            },
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DispatchMessageW, EnumWindows, GetMessageW,
                GetShellWindow, GetWindowThreadProcessId, IsWindowVisible, PostMessageW,
                PostQuitMessage, RegisterClassExW, SendMessageTimeoutW, TranslateMessage,
                HWND_BROADCAST, MSG, SMTO_ABORTIFHUNG, WINDOW_EX_STYLE, WM_CLOSE, WM_DESTROY,
                WNDCLASSEXW, WS_OVERLAPPEDWINDOW,
            },
        },
    },
};

/// 取代標準的 `println!`／`eprintln!`：stdout／stderr 寫不出去（管線的讀取端已死、主控台已關）時，
/// 標準版會 panic，連帶讓還原整段跑不到。本探針的主控台輸出只是附帶，一律忽略寫入錯誤——
/// 任何輸出都不得妨礙還原。（巨集在此檔其餘處遮蔽 prelude 的同名巨集。）
macro_rules! println {
    ($($arg:tt)*) => {{
        let _ = writeln!(std::io::stdout().lock(), $($arg)*);
    }};
}
macro_rules! eprintln {
    ($($arg:tt)*) => {{
        let _ = writeln!(std::io::stderr().lock(), $($arg)*);
    }};
}

// ---------------------------------------------------------------------------------------------
// 常數與全域旗標
// ---------------------------------------------------------------------------------------------

const WM_SETTINGCHANGE_ID: u32 = 0x001A;
const WM_DISPLAYCHANGE_ID: u32 = 0x007E;
const WM_THEMECHANGED_ID: u32 = 0x031A;
const WM_DWMCOLORIZATIONCOLORCHANGED_ID: u32 = 0x0320;
const WM_SYSCOLORCHANGE_ID: u32 = 0x0015;

/// 兩次取樣間隔超過這個秒數就記 WARN（行程可能被凍結：待機、鎖定後系統休眠）。
const FREEZE_GAP_SECS: f64 = 90.0;
/// 連續設桌布失敗這麼多個週期就中止並還原。
const MAX_CONSECUTIVE_FAIL_CYCLES: u32 = 20;
/// 1.2 log 裡「視窗之外」的逐筆訊息明細上限（超過只計數）。
const MAX_OUTSIDE_DETAIL_LINES: usize = 3000;
/// 訊息緩衝上限（防失控）。
const MAX_MSGS: usize = 2_000_000;

const PH_INIT: u8 = 0;
const PH_BASELINE: u8 = 1;
const PH_SETTING: u8 = 2;
const PH_COOLDOWN: u8 = 3;
const PH_RESTORE: u8 = 4;
const PHASE_NAMES: [&str; 5] = ["init", "baseline", "setting", "cooldown", "restore"];

static STOP: AtomicBool = AtomicBool::new(false);
static RESTORE_DONE: AtomicBool = AtomicBool::new(false);
static PHASE: AtomicU8 = AtomicU8::new(PH_INIT);
static SINK_HWND: AtomicIsize = AtomicIsize::new(0);
/// 測試用：接下來 N 次 `create_dw()` 一律失敗（模擬 explorer 重啟期間 COM 取不到）。
static TEST_FAIL_CREATES: AtomicU32 = AtomicU32::new(0);

// ---------------------------------------------------------------------------------------------
// 參數
// ---------------------------------------------------------------------------------------------

enum Mode {
    Run,
    BackupOnly,
    Restore(PathBuf),
}

struct Cfg {
    mode: Mode,
    image_a: PathBuf,
    image_b: PathBuf,
    baseline: Duration,
    setting: Duration,
    cooldown: Duration,
    interval: Duration,
    sample: Duration,
    window: Duration,
    out_dir: PathBuf,
    tag: String,
    backup_dir: PathBuf,
    allow_locked: bool,
    control_broadcast: bool,
    /// 測試用：開始後 N 秒模擬 Ctrl+C（設 STOP 旗標）。
    test_stop_secs: Option<u64>,
    /// 測試用：開始後 N 秒讓主迴圈 panic。
    test_panic_secs: Option<u64>,
    /// 測試用：開始後 N 秒丟掉手上的 IDesktopWallpaper（模擬 explorer 重啟後介面失效），驗證重建。
    test_drop_dw_secs: Option<u64>,
    /// 測試用：丟掉介面之後，接下來 N 次 CoCreateInstance 失敗（搭配 --test-drop-dw-secs）。
    test_fail_creates: u32,
    /// 測試用：進入還原前，讓接下來 N 次 CoCreateInstance 失敗，驗證還原的重試與登錄後備寫回。
    test_restore_fail_creates: u32,
    /// 1.7：只對這個螢幕索引（`GetMonitorDevicePathAt` 的序號）設桌布；None＝所有在線螢幕（1.1 預設）。
    monitor: Option<u32>,
    /// 1.7：每個設桌布週期結束後這麼久取樣一列（設桌布階段改為「每次設定一列」，不再做週期取樣；
    /// 基線與冷卻仍照 `--sample-secs` 週期取樣）。None＝1.1 預設的純週期取樣。
    post_set_sample: Option<Duration>,
    /// 1.7：CSV 末尾追加 `sample_kind,set_cycle,idle_s,exp_top_windows,exp_top_visible` 五欄。
    extra_cols: bool,
}

const USAGE: &str = "\
probe_wallpaper：IDesktopWallpaper explorer 負擔＋廣播訊息探針（dynamic-wallpaper 1.1／1.2）

用法：probe_wallpaper [選項]
  --a <png> --b <png>        兩張測試圖（預設 %LOCALAPPDATA%\\fc-probe-wallpaper\\a.png／b.png）
  --baseline-secs <n>        基線只取樣的秒數（預設 600）
  --set-secs <n>             交替設桌布的秒數（預設 7200）
  --cooldown-secs <n>        冷卻只取樣的秒數（預設 600）
  --interval-secs <n>        設桌布週期（預設 10）
  --sample-secs <n>          取樣週期（預設 30）
  --window-secs <n>          設桌布後計訊息的視窗秒數（預設 3）
  --out-dir <dir>            證據檔目錄（預設 %LOCALAPPDATA%\\fc-probe-wallpaper\\out）
  --tag <name>               證據檔名尾巴，例如 smoke、full（預設 run）
  --backup-dir <dir>         備份 JSON 目錄（預設 %LOCALAPPDATA%\\fc-probe-wallpaper）
  --allow-locked             工作階段鎖定時仍照跑（預設鎖定就 BLOCKED）
  --no-control               不送對照用的 WM_SETTINGCHANGE 廣播（預設基線開始 2 秒後送一次）
  --test-stop-secs <n>       測試用：開始後 n 秒模擬 Ctrl+C，驗證中止後仍還原
  --test-panic-secs <n>      測試用：開始後 n 秒讓主迴圈 panic，驗證仍還原
  --test-drop-dw-secs <n>    測試用：開始後 n 秒丟掉 COM 介面，驗證主迴圈重建
  --test-fail-creates <n>    測試用：丟掉介面後接下來 n 次 CoCreateInstance 失敗
  --test-restore-fail-creates <n> 測試用：還原前讓接下來 n 次 CoCreateInstance 失敗
  --monitor <idx>            1.7：只對這個螢幕索引設桌布（GetMonitorDevicePathAt 序號；預設所有在線螢幕）
  --sample-after-set-secs <n> 1.7：每次設定後 n 秒取樣一列，設桌布階段不再週期取樣（n 須小於 --interval-secs）
  --extra-cols               1.7：CSV 追加 sample_kind,set_cycle,idle_s,exp_top_windows,exp_top_visible
  --backup-only              只寫備份 JSON 與印出目前狀態，不改任何桌布
  --restore <備份.json>      只做還原（含讀回比對）
";

fn local_appdata() -> PathBuf {
    PathBuf::from(env::var_os("LOCALAPPDATA").unwrap_or_default())
}

fn parse_args() -> Result<Cfg, String> {
    let root = local_appdata().join("fc-probe-wallpaper");
    let mut cfg = Cfg {
        mode: Mode::Run,
        image_a: root.join("a.png"),
        image_b: root.join("b.png"),
        baseline: Duration::from_secs(600),
        setting: Duration::from_secs(7200),
        cooldown: Duration::from_secs(600),
        interval: Duration::from_secs(10),
        sample: Duration::from_secs(30),
        window: Duration::from_secs(3),
        out_dir: root.join("out"),
        tag: "run".to_string(),
        backup_dir: root.clone(),
        allow_locked: false,
        control_broadcast: true,
        test_stop_secs: None,
        test_panic_secs: None,
        test_drop_dw_secs: None,
        test_fail_creates: 0,
        test_restore_fail_creates: 0,
        monitor: None,
        post_set_sample: None,
        extra_cols: false,
    };
    let mut it = env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} 缺少參數值"));
        let secs = |v: String, name: &str| -> Result<Duration, String> {
            v.parse::<u64>()
                .map(Duration::from_secs)
                .map_err(|_| format!("{name} 需為非負整數秒：{v}"))
        };
        match flag.as_str() {
            "--a" => cfg.image_a = PathBuf::from(value("--a")?),
            "--b" => cfg.image_b = PathBuf::from(value("--b")?),
            "--baseline-secs" => cfg.baseline = secs(value("--baseline-secs")?, "--baseline-secs")?,
            "--set-secs" => cfg.setting = secs(value("--set-secs")?, "--set-secs")?,
            "--cooldown-secs" => cfg.cooldown = secs(value("--cooldown-secs")?, "--cooldown-secs")?,
            "--interval-secs" => cfg.interval = secs(value("--interval-secs")?, "--interval-secs")?,
            "--sample-secs" => cfg.sample = secs(value("--sample-secs")?, "--sample-secs")?,
            "--window-secs" => cfg.window = secs(value("--window-secs")?, "--window-secs")?,
            "--out-dir" => cfg.out_dir = PathBuf::from(value("--out-dir")?),
            "--tag" => cfg.tag = value("--tag")?,
            "--backup-dir" => cfg.backup_dir = PathBuf::from(value("--backup-dir")?),
            "--allow-locked" => cfg.allow_locked = true,
            "--no-control" => cfg.control_broadcast = false,
            "--test-stop-secs" => {
                cfg.test_stop_secs =
                    Some(secs(value("--test-stop-secs")?, "--test-stop-secs")?.as_secs());
            }
            "--test-drop-dw-secs" => {
                cfg.test_drop_dw_secs =
                    Some(secs(value("--test-drop-dw-secs")?, "--test-drop-dw-secs")?.as_secs());
            }
            "--test-fail-creates" => {
                cfg.test_fail_creates = value("--test-fail-creates")?
                    .parse()
                    .map_err(|_| "--test-fail-creates 需為非負整數".to_string())?;
            }
            "--test-restore-fail-creates" => {
                cfg.test_restore_fail_creates = value("--test-restore-fail-creates")?
                    .parse()
                    .map_err(|_| "--test-restore-fail-creates 需為非負整數".to_string())?;
            }
            "--test-panic-secs" => {
                cfg.test_panic_secs =
                    Some(secs(value("--test-panic-secs")?, "--test-panic-secs")?.as_secs());
            }
            "--monitor" => {
                cfg.monitor = Some(
                    value("--monitor")?
                        .parse()
                        .map_err(|_| "--monitor 需為非負整數".to_string())?,
                );
            }
            "--sample-after-set-secs" => {
                cfg.post_set_sample = Some(secs(
                    value("--sample-after-set-secs")?,
                    "--sample-after-set-secs",
                )?);
            }
            "--extra-cols" => cfg.extra_cols = true,
            "--backup-only" => cfg.mode = Mode::BackupOnly,
            "--restore" => cfg.mode = Mode::Restore(PathBuf::from(value("--restore")?)),
            "--help" | "-h" => return Err(String::new()),
            other => return Err(format!("未知參數：{other}")),
        }
    }
    if cfg.interval.is_zero() || cfg.sample.is_zero() {
        return Err("--interval-secs 與 --sample-secs 不可為 0".to_string());
    }
    if cfg
        .post_set_sample
        .is_some_and(|d| d.is_zero() || d >= cfg.interval)
    {
        return Err("--sample-after-set-secs 須大於 0 且小於 --interval-secs".to_string());
    }
    Ok(cfg)
}

// ---------------------------------------------------------------------------------------------
// 工具：時間戳、去識別、記錄檔
// ---------------------------------------------------------------------------------------------

/// 民用曆日期 → 自 1970-01-01 起的天數（Howard Hinnant 的 days_from_civil）。
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// 本機時間戳，含毫秒與 UTC 位移：`2026-10-02T14:03:05.123+08:00`。
fn stamp() -> String {
    let t = unsafe { GetLocalTime() };
    let local_secs = days_from_civil(i64::from(t.wYear), i64::from(t.wMonth), i64::from(t.wDay))
        * 86_400
        + i64::from(t.wHour) * 3600
        + i64::from(t.wMinute) * 60
        + i64::from(t.wSecond);
    let utc_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // 兩次讀時鐘之間可能差 1 秒，位移取最接近的 15 分鐘整數倍。
    let off_min = ((local_secs - utc_secs) as f64 / 900.0).round() as i64 * 15;
    let sign = if off_min < 0 { '-' } else { '+' };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}{}{:02}:{:02}",
        t.wYear,
        t.wMonth,
        t.wDay,
        t.wHour,
        t.wMinute,
        t.wSecond,
        t.wMilliseconds,
        sign,
        off_min.abs() / 60,
        off_min.abs() % 60
    )
}

/// 取代表（環境變數名、字樣）依值長度由長到短排序，讓 `%TEMP%` 先於 `%LOCALAPPDATA%` 命中。
fn redaction_table() -> &'static Vec<(String, &'static str)> {
    static TABLE: OnceLock<Vec<(String, &'static str)>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut v: Vec<(String, &'static str)> = Vec::new();
        for (var, token) in [
            ("TEMP", "%TEMP%"),
            ("LOCALAPPDATA", "%LOCALAPPDATA%"),
            ("APPDATA", "%APPDATA%"),
            ("USERPROFILE", "%USERPROFILE%"),
        ] {
            if let Ok(val) = env::var(var) {
                let val = val.trim_end_matches(['\\', '/']).to_string();
                if !val.is_empty() {
                    v.push((val, token));
                }
            }
        }
        v.sort_by_key(|(val, _)| std::cmp::Reverse(val.len()));
        v
    })
}

/// 把使用者設定檔路徑改寫成字樣（`\`、`/`、JSON 跳脫的 `\\` 三種寫法都認得，不分大小寫）。
fn redact(s: &str) -> String {
    let mut out = s.to_string();
    for (val, token) in redaction_table() {
        let variants = [
            val.clone(),
            val.replace('\\', "/"),
            val.replace('\\', "\\\\"),
        ];
        for pat in variants {
            // ASCII 小寫化不改變位元組長度，索引可直接對回原字串。
            loop {
                let lower = out.to_ascii_lowercase();
                match lower.find(&pat.to_ascii_lowercase()) {
                    Some(i) => out.replace_range(i..i + pat.len(), token),
                    None => break,
                }
            }
        }
    }
    out
}

/// 逐行附時間戳、去識別、立即寫檔的記錄檔；可選同步印到主控台。
struct Log {
    file: Mutex<File>,
    echo: bool,
}

impl Log {
    fn create(path: &Path, echo: bool) -> Result<Log, String> {
        let file = File::create(path).map_err(|e| format!("無法建立 {}：{e}", path.display()))?;
        Ok(Log {
            file: Mutex::new(file),
            echo,
        })
    }

    fn line(&self, msg: &str) {
        let text = format!("{} {}", stamp(), redact(msg));
        if self.echo {
            println!("{text}");
        }
        let mut f = self.file.lock().unwrap_or_else(|e| e.into_inner());
        let _ = writeln!(f, "{text}");
        let _ = f.flush();
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn pwstr_take(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    let s = unsafe { p.to_string() }.unwrap_or_default();
    unsafe { CoTaskMemFree(Some(p.0 as *const c_void)) };
    s
}

// ---------------------------------------------------------------------------------------------
// 登錄（HKCU\Control Panel\Desktop）
// ---------------------------------------------------------------------------------------------

const REG_NAMES: [&str; 3] = ["Wallpaper", "WallpaperStyle", "TileWallpaper"];

fn open_desktop_key(write: bool) -> Option<HKEY> {
    let mut key = HKEY::default();
    let access = if write {
        KEY_QUERY_VALUE | KEY_SET_VALUE
    } else {
        KEY_QUERY_VALUE
    };
    let st = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            w!("Control Panel\\Desktop"),
            None,
            access,
            &mut key,
        )
    };
    st.is_ok().then_some(key)
}

fn reg_read(name: &str) -> Option<String> {
    let key = open_desktop_key(false)?;
    let name_w = wide(name);
    let mut ty = REG_VALUE_TYPE::default();
    let mut len: u32 = 0;
    let probe = unsafe {
        RegQueryValueExW(
            key,
            PCWSTR(name_w.as_ptr()),
            None,
            Some(&mut ty),
            None,
            Some(&mut len),
        )
    };
    let result = if probe.is_ok() && len > 0 {
        let mut buf = vec![0u8; len as usize];
        let rd = unsafe {
            RegQueryValueExW(
                key,
                PCWSTR(name_w.as_ptr()),
                None,
                Some(&mut ty),
                Some(buf.as_mut_ptr()),
                Some(&mut len),
            )
        };
        rd.is_ok().then(|| {
            let u16s: Vec<u16> = buf[..len as usize]
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            let end = u16s.iter().position(|&c| c == 0).unwrap_or(u16s.len());
            String::from_utf16_lossy(&u16s[..end])
        })
    } else if probe.is_ok() {
        Some(String::new())
    } else {
        None
    };
    unsafe {
        let _ = RegCloseKey(key);
    }
    result
}

fn reg_write(name: &str, value: &str) -> bool {
    let Some(key) = open_desktop_key(true) else {
        return false;
    };
    let name_w = wide(name);
    let data: Vec<u8> = wide(value).iter().flat_map(|c| c.to_le_bytes()).collect();
    let st = unsafe { RegSetValueExW(key, PCWSTR(name_w.as_ptr()), None, REG_SZ, Some(&data)) };
    unsafe {
        let _ = RegCloseKey(key);
    }
    st.is_ok()
}

// ---------------------------------------------------------------------------------------------
// 桌布狀態：快照／備份／還原
// ---------------------------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug)]
struct MonSnap {
    id: String,
    online: bool,
    rect_hr: i32,
    rect: Option<[i32; 4]>,
    wallpaper: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Snapshot {
    version: u32,
    created: String,
    monitors: Vec<MonSnap>,
    position: Option<i32>,
    background_color: Option<u32>,
    status: Option<i32>,
    registry: BTreeMap<String, Option<String>>,
}

fn create_dw() -> Result<IDesktopWallpaper, String> {
    if TEST_FAIL_CREATES
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
        .is_ok()
    {
        return Err("（測試）模擬 CoCreateInstance 失敗".to_string());
    }
    unsafe { CoCreateInstance(&DesktopWallpaper, None, CLSCTX_ALL) }
        .map_err(|e| format!("CoCreateInstance(DesktopWallpaper) 失敗：{e}"))
}

/// 列舉螢幕（含已拔除者）。`GetMonitorRECT` 回 `S_FALSE`（1）代表離線，windows-rs 的包裝會把
/// `S_FALSE` 當成功而吞掉，故直接呼叫 vtable 取原始 HRESULT。
fn list_monitors(dw: &IDesktopWallpaper) -> Result<Vec<(String, i32, Option<RECT>)>, String> {
    let count = unsafe { dw.GetMonitorDevicePathCount() }
        .map_err(|e| format!("GetMonitorDevicePathCount 失敗：{e}"))?;
    let mut out = Vec::new();
    for i in 0..count {
        let id = pwstr_take(
            unsafe { dw.GetMonitorDevicePathAt(i) }
                .map_err(|e| format!("GetMonitorDevicePathAt({i}) 失敗：{e}"))?,
        );
        let id_w = wide(&id);
        let mut rect = RECT::default();
        let hr = unsafe {
            (Interface::vtable(dw).GetMonitorRECT)(
                Interface::as_raw(dw),
                PCWSTR(id_w.as_ptr()),
                &mut rect,
            )
        };
        out.push((id, hr.0, (hr.0 == 0).then_some(rect)));
    }
    Ok(out)
}

fn capture(dw: &IDesktopWallpaper) -> Result<Snapshot, String> {
    let mut monitors = Vec::new();
    for (id, hr, rect) in list_monitors(dw)? {
        let id_w = wide(&id);
        let wallpaper = unsafe { dw.GetWallpaper(PCWSTR(id_w.as_ptr())) }
            .ok()
            .map(pwstr_take);
        monitors.push(MonSnap {
            id,
            online: hr == 0,
            rect_hr: hr,
            rect: rect.map(|r| [r.left, r.top, r.right, r.bottom]),
            wallpaper,
        });
    }
    let mut registry = BTreeMap::new();
    for name in REG_NAMES {
        registry.insert(name.to_string(), reg_read(name));
    }
    Ok(Snapshot {
        version: 1,
        created: stamp(),
        monitors,
        position: unsafe { dw.GetPosition() }.ok().map(|p| p.0),
        background_color: unsafe { dw.GetBackgroundColor() }.ok().map(|c| c.0),
        status: unsafe { dw.GetStatus() }.ok().map(|s| s.0),
        registry,
    })
}

/// 原子寫入：先寫 `.tmp`、`sync_all`，再 `rename` 覆蓋。
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("json.tmp");
    let mut f = File::create(&tmp).map_err(|e| format!("無法建立 {}：{e}", tmp.display()))?;
    f.write_all(bytes)
        .and_then(|()| f.sync_all())
        .map_err(|e| format!("寫入 {} 失敗：{e}", tmp.display()))?;
    drop(f);
    fs::rename(&tmp, path).map_err(|e| format!("rename 到 {} 失敗：{e}", path.display()))
}

fn same_path(a: &str, b: &str) -> bool {
    a.replace('/', "\\")
        .eq_ignore_ascii_case(&b.replace('/', "\\"))
}

/// 還原時等 COM 恢復的最多嘗試次數與間隔（explorer 重啟後 DesktopWallpaper 物件要等新殼層註冊）。
const RESTORE_COM_ATTEMPTS: u32 = 15;
const RESTORE_COM_RETRY_DELAY: Duration = Duration::from_secs(2);

/// 還原到 `want`（備份快照）並讀回比對。回傳 `(全部一致, 說明行)`。
/// 只在目前值與備份不同時才寫入（乾淨狀態下什麼都不做）。
///
/// 不沿用呼叫端手上的 `IDesktopWallpaper`：它由 explorer 承載，explorer 重啟後會變成斷線的
/// proxy。每次嘗試都重新 `CoCreateInstance`，失敗就等一下重試；COM 始終不可用時仍把三個登錄值
/// 寫回（不依賴 COM），螢幕桌布留給 `--restore` 事後補還原，回傳 `false`。
fn restore(want: &Snapshot) -> (bool, Vec<String>) {
    let mut lines = Vec::new();

    let mut com: Option<(IDesktopWallpaper, Snapshot)> = None;
    for attempt in 1..=RESTORE_COM_ATTEMPTS {
        match create_dw().and_then(|d| capture(&d).map(|c| (d, c))) {
            Ok(pair) => {
                if attempt > 1 {
                    lines.push(format!("RESTORE-COM 第 {attempt} 次嘗試成功取得 COM"));
                }
                com = Some(pair);
                break;
            }
            Err(e) => {
                lines.push(format!(
                    "RESTORE-RETRY COM 第 {attempt}/{RESTORE_COM_ATTEMPTS} 次失敗：{e}"
                ));
                if attempt < RESTORE_COM_ATTEMPTS {
                    thread::sleep(RESTORE_COM_RETRY_DELAY);
                }
            }
        }
    }

    if let Some((dw, cur)) = &com {
        for m in want.monitors.iter().filter(|m| m.online) {
            let Some(wp) = &m.wallpaper else { continue };
            let now = cur.monitors.iter().find(|c| c.id == m.id && c.online);
            match now {
                None => lines.push(format!(
                    "RESTORE-SKIP 螢幕目前離線或不存在，無法還原：{}",
                    m.id
                )),
                Some(c) if c.wallpaper.as_deref().is_some_and(|p| same_path(p, wp)) => {
                    lines.push(format!(
                        "RESTORE-NOOP 螢幕桌布已與備份相同，不寫入：{}",
                        m.id
                    ));
                }
                Some(_) => {
                    let id_w = wide(&m.id);
                    let wp_w = wide(wp);
                    let r =
                        unsafe { dw.SetWallpaper(PCWSTR(id_w.as_ptr()), PCWSTR(wp_w.as_ptr())) };
                    lines.push(format!(
                        "RESTORE-SET SetWallpaper({}) -> {wp} : {}",
                        m.id,
                        r.map_or_else(|e| format!("失敗 {e}"), |()| "OK".to_string())
                    ));
                }
            }
        }
        if let Some(pos) = want.position {
            if cur.position != Some(pos) {
                let r = unsafe { dw.SetPosition(DESKTOP_WALLPAPER_POSITION(pos)) };
                lines.push(format!(
                    "RESTORE-SET SetPosition({pos}) : {}",
                    r.map_or_else(|e| format!("失敗 {e}"), |()| "OK".to_string())
                ));
            }
        }
        if let Some(color) = want.background_color {
            if cur.background_color != Some(color) {
                let r =
                    unsafe { dw.SetBackgroundColor(windows::Win32::Foundation::COLORREF(color)) };
                lines.push(format!(
                    "RESTORE-SET SetBackgroundColor(0x{color:08X}) : {}",
                    r.map_or_else(|e| format!("失敗 {e}"), |()| "OK".to_string())
                ));
            }
        }
        thread::sleep(Duration::from_millis(500));
    } else {
        lines.push(
            "RESTORE-FAIL COM 始終不可用：螢幕桌布未還原，只寫回登錄值；請事後以 --restore 補還原"
                .to_string(),
        );
    }

    // 登錄值若與備份不同（COM 還原沒有帶回，或 COM 根本不可用），只對這三個值原樣寫回。
    for (name, want_val) in &want.registry {
        if let Some(v) = want_val {
            let before = reg_read(name);
            if before.as_ref() != Some(v) {
                let ok = reg_write(name, v);
                lines.push(format!(
                    "RESTORE-REG 登錄 {name} 與備份不同（現在={before:?} 備份={v:?}），直接寫回：{}",
                    if ok { "OK" } else { "失敗" }
                ));
            }
        }
    }

    // 讀回比對：COM 部分重新取一次快照（取不到就一律判 DIFF）；登錄直接讀。
    let after = com.as_ref().and_then(|(dw, _)| capture(dw).ok());
    let mut all_ok = true;
    for m in want.monitors.iter().filter(|m| m.online) {
        let got = after
            .as_ref()
            .and_then(|a| a.monitors.iter().find(|c| c.id == m.id));
        let ok = match (&m.wallpaper, got.and_then(|g| g.wallpaper.as_ref())) {
            (None, _) => true,
            (Some(a), Some(b)) => same_path(a, b),
            _ => false,
        };
        all_ok &= ok;
        lines.push(format!(
            "READBACK {} monitor {} want={:?} got={:?}",
            if ok { "MATCH" } else { "DIFF " },
            m.id,
            m.wallpaper,
            got.and_then(|g| g.wallpaper.clone())
        ));
    }
    let mut cmp = |name: &str, want_v: String, got_v: String| {
        let ok = want_v == got_v;
        all_ok &= ok;
        lines.push(format!(
            "READBACK {} {name} want={want_v} got={got_v}",
            if ok { "MATCH" } else { "DIFF " }
        ));
    };
    cmp(
        "position",
        format!("{:?}", want.position),
        format!("{:?}", after.as_ref().and_then(|a| a.position)),
    );
    cmp(
        "background_color",
        format!("{:?}", want.background_color),
        format!("{:?}", after.as_ref().and_then(|a| a.background_color)),
    );
    for name in REG_NAMES {
        cmp(
            &format!("HKCU\\Control Panel\\Desktop\\{name}"),
            format!("{:?}", want.registry.get(name).cloned().flatten()),
            format!("{:?}", reg_read(name)),
        );
    }
    (all_ok, lines)
}

// ---------------------------------------------------------------------------------------------
// 行程取樣
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Default, Debug)]
struct ProcStat {
    pid: u32,
    cpu_100ns: u64,
    ws: u64,
    private: u64,
    handles: u32,
    gdi: u32,
    gdi_peak: u32,
    user: u32,
    user_peak: u32,
}

fn filetime_u64(ft: &windows::Win32::Foundation::FILETIME) -> u64 {
    (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime)
}

fn sample_handle(h: HANDLE, pid: u32) -> Option<ProcStat> {
    let mut creation = Default::default();
    let mut exit = Default::default();
    let mut kernel = Default::default();
    let mut user = Default::default();
    unsafe { GetProcessTimes(h, &mut creation, &mut exit, &mut kernel, &mut user) }.ok()?;
    let mut mem = PROCESS_MEMORY_COUNTERS_EX {
        cb: size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        ..Default::default()
    };
    unsafe {
        GetProcessMemoryInfo(
            h,
            std::ptr::addr_of_mut!(mem).cast(),
            size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        )
    }
    .ok()?;
    let mut handles = 0u32;
    unsafe { GetProcessHandleCount(h, &mut handles) }.ok()?;
    Some(ProcStat {
        pid,
        cpu_100ns: filetime_u64(&kernel) + filetime_u64(&user),
        ws: mem.WorkingSetSize as u64,
        private: mem.PrivateUsage as u64,
        handles,
        gdi: unsafe { GetGuiResources(h, GR_GDIOBJECTS) },
        gdi_peak: unsafe { GetGuiResources(h, GR_GDIOBJECTS_PEAK) },
        user: unsafe { GetGuiResources(h, GR_USEROBJECTS) },
        user_peak: unsafe { GetGuiResources(h, GR_USEROBJECTS_PEAK) },
    })
}

fn sample_pid(pid: u32) -> Option<ProcStat> {
    let h = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid) }.ok()?;
    let r = sample_handle(h, pid);
    unsafe {
        let _ = CloseHandle(h);
    }
    r
}

fn sample_self() -> ProcStat {
    sample_handle(unsafe { GetCurrentProcess() }, unsafe {
        GetCurrentProcessId()
    })
    .unwrap_or_default()
}

/// explorer 的 PID：以 `GetShellWindow` 的擁有者為準（explorer 重啟中沒有殼層視窗時回 0）。
fn explorer_pid() -> u32 {
    let hwnd = unsafe { GetShellWindow() };
    if hwnd.0.is_null() {
        return 0;
    }
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

/// 工作階段是否鎖定：`LogonUI.exe` 是否在跑（LockApp 可能殘留，不採用）。
fn is_locked() -> bool {
    let Ok(snap) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else {
        return false;
    };
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut found = false;
    let mut ok = unsafe { Process32FirstW(snap, &mut entry) }.is_ok();
    while ok {
        let end = entry
            .szExeFile
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(entry.szExeFile.len());
        if String::from_utf16_lossy(&entry.szExeFile[..end]).eq_ignore_ascii_case("LogonUI.exe") {
            found = true;
            break;
        }
        ok = unsafe { Process32NextW(snap, &mut entry) }.is_ok();
    }
    unsafe {
        let _ = CloseHandle(snap);
    }
    found
}

/// `GetLastInputInfo` 的結構（windows crate 的定義在未啟用的 `Win32_UI_Input_KeyboardAndMouse`
/// feature 下，探針不為此改 Cargo.toml，直接宣告；欄位與 winuser.h 的 `LASTINPUTINFO` 相同）。
#[repr(C)]
struct LastInputInfo {
    cb_size: u32,
    dw_time: u32,
}

#[link(name = "user32")]
extern "system" {
    fn GetLastInputInfo(plii: *mut LastInputInfo) -> BOOL;
}

/// 1.7：工作階段閒置秒數（只讀）：`GetTickCount() - LASTINPUTINFO.dwTime`，兩者同為開機後毫秒、
/// 32 位元會繞回（約 49.7 天），以 wrapping 相減處理。失敗回 None。
fn idle_secs() -> Option<f64> {
    let mut lii = LastInputInfo {
        cb_size: size_of::<LastInputInfo>() as u32,
        dw_time: 0,
    };
    // SAFETY：lii 是有效、可寫、cbSize 已正確設定的 LASTINPUTINFO；呼叫期間不被他處借用。
    let ok = unsafe { GetLastInputInfo(&mut lii) };
    if !ok.as_bool() {
        return None;
    }
    let now = unsafe { GetTickCount() };
    Some(f64::from(now.wrapping_sub(lii.dw_time)) / 1000.0)
}

struct TopCount {
    pid: u32,
    total: u32,
    visible: u32,
}

unsafe extern "system" fn count_top_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY：lparam 是 explorer_top_windows() 傳入的 &mut TopCount，EnumWindows 同步回呼、
    // 回傳前不會再使用，期間沒有其他借用。
    let acc = unsafe { &mut *(lparam.0 as *mut TopCount) };
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == acc.pid {
        acc.total += 1;
        if unsafe { IsWindowVisible(hwnd) }.as_bool() {
            acc.visible += 1;
        }
    }
    BOOL(1)
}

/// 1.7：explorer（`pid`）擁有的頂層視窗數（`EnumWindows`，不含 message-only 視窗）與其中可見者。
fn explorer_top_windows(pid: u32) -> Option<(u32, u32)> {
    if pid == 0 {
        return None;
    }
    let mut acc = TopCount {
        pid,
        total: 0,
        visible: 0,
    };
    // SAFETY：回呼只在 EnumWindows 執行期間被同步呼叫，acc 在此期間存活且只經由 lparam 存取。
    unsafe {
        EnumWindows(
            Some(count_top_proc),
            LPARAM(std::ptr::addr_of_mut!(acc) as isize),
        )
    }
    .ok()?;
    Some((acc.total, acc.visible))
}

// ---------------------------------------------------------------------------------------------
// 1.2：隱藏頂層視窗與訊息記錄
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Msg {
    at: Instant,
    phase: u8,
    id: u32,
    wparam: usize,
    lparam: isize,
    text: Option<String>,
}

fn msgs() -> &'static Mutex<Vec<Msg>> {
    static MSGS: OnceLock<Mutex<Vec<Msg>>> = OnceLock::new();
    MSGS.get_or_init(|| Mutex::new(Vec::new()))
}

/// 讀自己行程位址空間的 UTF-16 字串；用 `ReadProcessMemory` 而非直接解參考，指標無效時回失敗
/// 而不是讓行程當掉（`WM_SETTINGCHANGE` 的 lParam 理論上是字串指標，實務上也可能是 0 或整數）。
fn read_own_wstr(ptr: usize) -> Option<String> {
    if ptr < 0x10000 {
        return None;
    }
    let mut out: Vec<u16> = Vec::new();
    for i in 0..256usize {
        let mut ch: u16 = 0;
        let mut n = 0usize;
        let ok = unsafe {
            ReadProcessMemory(
                GetCurrentProcess(),
                (ptr + i * 2) as *const c_void,
                std::ptr::addr_of_mut!(ch).cast(),
                2,
                Some(&mut n),
            )
        }
        .is_ok();
        if !ok || n != 2 {
            return None;
        }
        if ch == 0 {
            break;
        }
        out.push(ch);
    }
    Some(String::from_utf16_lossy(&out))
}

fn record_message(id: u32, wparam: usize, lparam: isize) {
    let text = (id == WM_SETTINGCHANGE_ID)
        .then(|| read_own_wstr(lparam as usize))
        .flatten();
    let mut v = msgs().lock().unwrap_or_else(|e| e.into_inner());
    if v.len() < MAX_MSGS {
        v.push(Msg {
            at: Instant::now(),
            phase: PHASE.load(Ordering::Relaxed),
            id,
            wparam,
            lparam,
            text,
        });
    }
}

unsafe extern "system" fn sink_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    record_message(msg, wparam.0, lparam.0);
    if msg == WM_DESTROY {
        unsafe { PostQuitMessage(0) };
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// 建立隱藏的一般頂層視窗（無擁有者、不顯示；不是 message-only，才收得到廣播）並在專屬執行緒跑
/// 訊息迴圈。回傳執行緒 handle。
fn spawn_sink() -> Result<thread::JoinHandle<()>, String> {
    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    let handle = thread::spawn(move || {
        let setup = || -> Result<(), String> {
            let hmod =
                unsafe { GetModuleHandleW(None) }.map_err(|e| format!("GetModuleHandleW：{e}"))?;
            let hinst = HINSTANCE(hmod.0);
            let class = w!("FcProbeWallpaperMsgSink");
            let wc = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(sink_proc),
                hInstance: hinst,
                lpszClassName: class,
                ..Default::default()
            };
            if unsafe { RegisterClassExW(&wc) } == 0 {
                return Err("RegisterClassExW 失敗".to_string());
            }
            let hwnd = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    class,
                    w!("fc-probe-wallpaper msg sink"),
                    WS_OVERLAPPEDWINDOW,
                    0,
                    0,
                    100,
                    100,
                    None,
                    None,
                    Some(hinst),
                    None,
                )
            }
            .map_err(|e| format!("CreateWindowExW：{e}"))?;
            SINK_HWND.store(hwnd.0 as isize, Ordering::SeqCst);
            Ok(())
        };
        let r = setup();
        let ok = r.is_ok();
        let _ = tx.send(r);
        if !ok {
            return;
        }
        let mut m = MSG::default();
        while unsafe { GetMessageW(&mut m, None, 0, 0) }.0 > 0 {
            unsafe {
                let _ = TranslateMessage(&m);
                DispatchMessageW(&m);
            }
        }
    });
    rx.recv()
        .map_err(|_| "訊息視窗執行緒意外結束".to_string())??;
    Ok(handle)
}

/// 對照組：廣播一則自訂字串的 `WM_SETTINGCHANGE`（lParam＝`fc-probe-control`，一般應用程式會忽略
/// 未知字串）。用來證明隱藏視窗確實收得到廣播——否則「設桌布期間收到 0 則」無法區分
/// 「設桌布不廣播」與「視窗是聾的」。
fn send_control_broadcast(log12: &Log) {
    let text = wide("fc-probe-control");
    let mut result = 0usize;
    let t = Instant::now();
    let r = unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE_ID,
            WPARAM(0),
            LPARAM(text.as_ptr() as isize),
            SMTO_ABORTIFHUNG,
            2000,
            Some(&mut result),
        )
    };
    log12.line(&format!(
        "CONTROL 已廣播對照訊息 WM_SETTINGCHANGE lParam=\"fc-probe-control\"（SendMessageTimeout 回傳 {}，耗時 {:.0}ms）；之後應在下方出現一筆 OUTSIDE ... text=\"fc-probe-control\"，沒有就代表隱藏視窗收不到廣播",
        r.0,
        t.elapsed().as_secs_f64() * 1000.0
    ));
}

fn stop_sink(handle: thread::JoinHandle<()>) {
    let h = SINK_HWND.load(Ordering::SeqCst);
    if h != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(h as *mut c_void)), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
    let _ = handle.join();
}

fn msg_key(id: u32) -> String {
    match id {
        WM_SETTINGCHANGE_ID => "WM_SETTINGCHANGE".to_string(),
        WM_DISPLAYCHANGE_ID => "WM_DISPLAYCHANGE".to_string(),
        WM_THEMECHANGED_ID => "WM_THEMECHANGED".to_string(),
        WM_DWMCOLORIZATIONCOLORCHANGED_ID => "WM_DWMCOLORIZATIONCOLORCHANGED".to_string(),
        WM_SYSCOLORCHANGE_ID => "WM_SYSCOLORCHANGE".to_string(),
        other => format!("other:0x{other:04X}"),
    }
}

/// 常見的「其他」訊息名稱，只用於人讀的明細。
fn other_name(id: u32) -> &'static str {
    match id {
        0x0046 => "WM_WINDOWPOSCHANGING",
        0x0047 => "WM_WINDOWPOSCHANGED",
        0x001D => "WM_FONTCHANGE",
        0x0054 => "WM_USERCHANGED",
        0x0218 => "WM_POWERBROADCAST",
        0x0219 => "WM_DEVICECHANGE",
        0x031E => "WM_DWMCOMPOSITIONCHANGED",
        0x031F => "WM_DWMNCRENDERINGCHANGED",
        0x0321 => "WM_DWMWINDOWMAXIMIZEDCHANGE",
        0x0024 => "WM_GETMINMAXINFO",
        0x0081 => "WM_NCCREATE",
        0x0001 => "WM_CREATE",
        0x0083 => "WM_NCCALCSIZE",
        _ => "",
    }
}

fn key_display(key: &str) -> String {
    if let Some(hex) = key.strip_prefix("other:0x") {
        if let Ok(id) = u32::from_str_radix(hex, 16) {
            let n = other_name(id);
            if !n.is_empty() {
                return format!("{key}({n})");
            }
        }
    }
    key.to_string()
}

/// 一個「設桌布週期」：同一輪對各螢幕逐一 SetWallpaper。
struct SetRec {
    monitor: usize,
    image: &'static str,
    offset_ms: f64,
    dur_ms: f64,
    hr: i32,
}

struct Cycle {
    n: usize,
    label: String,
    start: Instant,
    start_stamp: String,
    end: Instant,
    sets: Vec<SetRec>,
    /// 這個週期列舉螢幕失敗（或根本沒有可用的 COM 介面）。
    enum_failed: bool,
}

#[derive(Default)]
struct MsgAcc {
    all: Vec<Msg>,
    cursor: usize,
    /// (階段, 類型) → 次數；所有收到的訊息。
    counts: HashMap<(u8, String), u64>,
    /// 視窗內（設桌布後 N 秒）的訊息類型 → 次數。
    in_window: HashMap<String, u64>,
    /// WM_SETTINGCHANGE 的 lParam 字串 → 次數（視窗內／外分開）。
    sc_text_in: HashMap<String, u64>,
    sc_text_out: HashMap<String, u64>,
    outside_logged: usize,
    cycles_final: usize,
    cycles_with_any: usize,
}

impl MsgAcc {
    fn drain(&mut self) {
        let mut v = msgs().lock().unwrap_or_else(|e| e.into_inner());
        for m in v.drain(..) {
            *self.counts.entry((m.phase, msg_key(m.id))).or_insert(0) += 1;
            self.all.push(m);
        }
    }

    fn describe(m: &Msg) -> String {
        format!(
            "{} wParam=0x{:X} lParam=0x{:X}{}",
            key_display(&msg_key(m.id)),
            m.wparam,
            m.lparam,
            m.text
                .as_ref()
                .map(|t| format!(" text={t:?}"))
                .unwrap_or_default()
        )
    }

    /// 處理游標到 `limit` 之前（不含）的視窗外訊息：逐筆明細（有上限）並計入字串統計。
    fn flush_outside(&mut self, limit: Option<Instant>, t0: Instant, log12: &Log) {
        while self.cursor < self.all.len() {
            let m = self.all[self.cursor].clone();
            if limit.is_some_and(|l| m.at >= l) {
                break;
            }
            self.cursor += 1;
            if m.phase == PH_INIT {
                continue;
            }
            if m.id == WM_SETTINGCHANGE_ID {
                *self
                    .sc_text_out
                    .entry(m.text.clone().unwrap_or_else(|| "<無字串>".to_string()))
                    .or_insert(0) += 1;
            }
            if self.outside_logged < MAX_OUTSIDE_DETAIL_LINES {
                self.outside_logged += 1;
                log12.line(&format!(
                    "OUTSIDE phase={} t+{:.3}s {}",
                    PHASE_NAMES[m.phase as usize],
                    m.at.saturating_duration_since(t0).as_secs_f64(),
                    Self::describe(&m)
                ));
            }
        }
    }

    /// 結算一個週期：視窗為 `[start, min(end+window, wend_limit))`。
    fn finalize(
        &mut self,
        c: &Cycle,
        window: Duration,
        next_start: Option<Instant>,
        t0: Instant,
        log12: &Log,
    ) {
        self.flush_outside(Some(c.start), t0, log12);
        let mut wend = c.end + window;
        if let Some(ns) = next_start {
            wend = wend.min(ns);
        }
        let mut per: BTreeMap<String, u64> = BTreeMap::new();
        let mut detail = Vec::new();
        while self.cursor < self.all.len() && self.all[self.cursor].at < wend {
            let m = self.all[self.cursor].clone();
            self.cursor += 1;
            let key = msg_key(m.id);
            *per.entry(key.clone()).or_insert(0) += 1;
            *self.in_window.entry(key).or_insert(0) += 1;
            if m.id == WM_SETTINGCHANGE_ID {
                *self
                    .sc_text_in
                    .entry(m.text.clone().unwrap_or_else(|| "<無字串>".to_string()))
                    .or_insert(0) += 1;
            }
            detail.push(format!(
                "    +{:.1}ms {}",
                m.at.saturating_duration_since(c.start).as_secs_f64() * 1000.0,
                Self::describe(&m)
            ));
        }
        self.cycles_final += 1;
        if !per.is_empty() {
            self.cycles_with_any += 1;
        }
        let total: u64 = per.values().sum();
        let named = |k: &str| per.get(k).copied().unwrap_or(0);
        let other: u64 = per
            .iter()
            .filter(|(k, _)| k.starts_with("other:"))
            .map(|(_, v)| *v)
            .sum();
        let sets_txt: Vec<String> = c
            .sets
            .iter()
            .map(|s| {
                format!(
                    "mon{}={}@+{:.1}ms/{:.1}ms/hr=0x{:08X}",
                    s.monitor, s.image, s.offset_ms, s.dur_ms, s.hr as u32
                )
            })
            .collect();
        log12.line(&format!(
            "CYCLE n={} label={} start={} sets=[{}] window_ms={:.0} total={} \
             WM_SETTINGCHANGE={} WM_DISPLAYCHANGE={} WM_THEMECHANGED={} \
             WM_DWMCOLORIZATIONCOLORCHANGED={} WM_SYSCOLORCHANGE={} other={}",
            c.n,
            c.label,
            c.start_stamp,
            sets_txt.join(" "),
            wend.saturating_duration_since(c.start).as_secs_f64() * 1000.0,
            total,
            named("WM_SETTINGCHANGE"),
            named("WM_DISPLAYCHANGE"),
            named("WM_THEMECHANGED"),
            named("WM_DWMCOLORIZATIONCOLORCHANGED"),
            named("WM_SYSCOLORCHANGE"),
            other
        ));
        for d in detail {
            log12.line(&d);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 取樣列與趨勢
// ---------------------------------------------------------------------------------------------

struct Row {
    phase: u8,
    elapsed_s: f64,
    exp: Option<ProcStat>,
    me: ProcStat,
}

fn csv_header(extra_cols: bool) -> String {
    let mut h =
        "ts,elapsed_s,phase,gap_s,locked,set_ok,set_fail,exp_pid,exp_pid_changed,exp_cpu_ms,\
exp_cpu_pct_1core,exp_ws_bytes,exp_private_bytes,exp_handles,exp_gdi,exp_gdi_peak,exp_user,\
exp_user_peak,self_cpu_ms,self_cpu_pct_1core,self_ws_bytes,self_private_bytes,self_handles,\
self_gdi,self_gdi_peak,self_user,self_user_peak"
            .to_string();
    if extra_cols {
        h.push_str(",sample_kind,set_cycle,idle_s,exp_top_windows,exp_top_visible");
    }
    h
}

/// 取樣列的種類（`--extra-cols` 的 `sample_kind` 欄）。
#[derive(Clone, Copy)]
enum SampleKind {
    /// 依 `--sample-secs` 的週期取樣（1.1 唯一的種類）。
    Periodic,
    /// 1.7：第 N 個設桌布週期結束後 `--sample-after-set-secs` 秒的取樣。
    PostSet(usize),
    /// 主迴圈結束時的終點取樣。
    Final,
}

/// 最小平方法斜率（y 對 x）。點數不足或 x 無變化回 None。
fn slope(points: &[(f64, f64)]) -> Option<f64> {
    if points.len() < 3 {
        return None;
    }
    let n = points.len() as f64;
    let mx = points.iter().map(|p| p.0).sum::<f64>() / n;
    let my = points.iter().map(|p| p.1).sum::<f64>() / n;
    let sxx: f64 = points.iter().map(|p| (p.0 - mx).powi(2)).sum();
    if sxx == 0.0 {
        return None;
    }
    let sxy: f64 = points.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
    Some(sxy / sxx)
}

fn mean(v: &[f64]) -> Option<f64> {
    (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64)
}

fn trend_report(rows: &[Row], log11: &Log) {
    log11.line("TREND 趨勢摘要（僅供參考；是否累積由人讀 CSV 判斷。斜率＝設桌布階段內最小平方法，單位每小時）");
    let Some(first_pid) = rows.iter().find_map(|r| r.exp.map(|e| e.pid)) else {
        log11.line("TREND 沒有任何 explorer 取樣");
        return;
    };
    let changed = rows
        .iter()
        .filter_map(|r| r.exp.map(|e| e.pid))
        .any(|p| p != first_pid);
    if changed {
        log11.line("TREND 注意：期間 explorer PID 變過，下列只計與第一個 PID 相同的列");
    }
    type Pick = fn(&ProcStat) -> f64;
    let metrics: [(&str, Pick); 5] = [
        ("ws_bytes", |s| s.ws as f64),
        ("private_bytes", |s| s.private as f64),
        ("handles", |s| f64::from(s.handles)),
        ("gdi", |s| f64::from(s.gdi)),
        ("user", |s| f64::from(s.user)),
    ];
    for who in ["exp", "self"] {
        for (name, pick) in metrics {
            let get = |r: &Row| -> Option<f64> {
                if who == "exp" {
                    r.exp.filter(|e| e.pid == first_pid).map(|e| pick(&e))
                } else {
                    Some(pick(&r.me))
                }
            };
            let in_phase = |p: u8| -> Vec<(f64, f64)> {
                rows.iter()
                    .filter(|r| r.phase == p)
                    .filter_map(|r| get(r).map(|v| (r.elapsed_s, v)))
                    .collect()
            };
            let base = in_phase(PH_BASELINE);
            let set = in_phase(PH_SETTING);
            let cool = in_phase(PH_COOLDOWN);
            let bm = mean(&base.iter().map(|p| p.1).collect::<Vec<_>>());
            let cm = mean(&cool.iter().map(|p| p.1).collect::<Vec<_>>());
            let sl = slope(&set).map(|s| s * 3600.0);
            let fmt = |v: Option<f64>| v.map_or_else(|| "n/a".to_string(), |x| format!("{x:.1}"));
            log11.line(&format!(
                "TREND {who}_{name}: baseline_mean={} setting_slope_per_hour={} cooldown_mean={} \
                 (samples base/set/cool={}/{}/{})",
                fmt(bm),
                fmt(sl),
                fmt(cm),
                base.len(),
                set.len(),
                cool.len()
            ));
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Ctrl+C
// ---------------------------------------------------------------------------------------------

unsafe extern "system" fn ctrl_handler(ty: u32) -> BOOL {
    STOP.store(true, Ordering::SeqCst);
    // 關閉視窗／登出／關機時行程約 5 秒後被終止：等主執行緒還原完成再回傳。
    if matches!(
        ty,
        CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT
    ) {
        let t = Instant::now();
        while !RESTORE_DONE.load(Ordering::SeqCst) && t.elapsed() < Duration::from_millis(4800) {
            thread::sleep(Duration::from_millis(50));
        }
    }
    BOOL(1)
}

// ---------------------------------------------------------------------------------------------
// 主流程
// ---------------------------------------------------------------------------------------------

struct RunReport {
    abort_reason: Option<String>,
    rows: Vec<Row>,
    acc: MsgAcc,
    cycles: Vec<Cycle>,
    t0: Instant,
    phase_span: [(f64, f64); 5],
    set_ok: u64,
    set_fail: u64,
    /// 已結算（寫進 1.2 log）的週期數。
    finalized: usize,
    /// 重建 IDesktopWallpaper 的次數（explorer 重啟或 COM 呼叫失敗後）。
    com_recreates: u32,
    /// 觀察到 explorer PID 改變的次數。
    explorer_restarts: u32,
}

#[allow(clippy::too_many_arguments)]
fn run_loop(
    cfg: &Cfg,
    dw: &mut Option<IDesktopWallpaper>,
    csv: &mut File,
    log11: &Log,
    log12: &Log,
    sink_alive: bool,
) -> RunReport {
    let t0 = Instant::now();
    let total = cfg.baseline + cfg.setting + cfg.cooldown;
    let mut rep = RunReport {
        abort_reason: None,
        rows: Vec::new(),
        acc: MsgAcc::default(),
        cycles: Vec::new(),
        t0,
        phase_span: [(f64::NAN, f64::NAN); 5],
        set_ok: 0,
        set_fail: 0,
        finalized: 0,
        com_recreates: 0,
        explorer_restarts: 0,
    };
    let imgs: [(&'static str, &Path); 2] = [("A", &cfg.image_a), ("B", &cfg.image_b)];

    let mut next_sample = t0;
    let mut next_set = t0 + cfg.baseline;
    let mut last_wall: Option<SystemTime> = None;
    let mut prev_exp: Option<ProcStat> = None;
    let mut prev_self: Option<ProcStat> = None;
    let mut consecutive_fail_cycles = 0u32;
    let mut cur_phase = PH_INIT;
    let mut control_pending = cfg.control_broadcast && sink_alive;
    let mut need_recreate = false;
    let mut dropped = false;
    // 1.7：待取的「設定後取樣」（到期時刻、週期序號）。
    let mut post_sample: Option<(Instant, usize)> = None;

    loop {
        if cfg
            .test_stop_secs
            .is_some_and(|n| t0.elapsed() >= Duration::from_secs(n))
        {
            STOP.store(true, Ordering::SeqCst);
        }
        if cfg
            .test_panic_secs
            .is_some_and(|n| t0.elapsed() >= Duration::from_secs(n))
        {
            panic!("--test-panic-secs 測試用 panic");
        }
        if !dropped
            && cfg
                .test_drop_dw_secs
                .is_some_and(|n| t0.elapsed() >= Duration::from_secs(n))
        {
            dropped = true;
            *dw = None;
            TEST_FAIL_CREATES.store(cfg.test_fail_creates, Ordering::SeqCst);
            log11.line(&format!(
                "TEST 丟掉 IDesktopWallpaper 介面；接下來 {} 次 CoCreateInstance 會失敗",
                cfg.test_fail_creates
            ));
        }
        if STOP.load(Ordering::SeqCst) {
            rep.abort_reason = Some("收到 Ctrl+C／關閉主控台".to_string());
            break;
        }
        let now = Instant::now();
        let elapsed = now.duration_since(t0);
        let phase = if elapsed < cfg.baseline {
            PH_BASELINE
        } else if elapsed < cfg.baseline + cfg.setting {
            PH_SETTING
        } else if elapsed < total {
            PH_COOLDOWN
        } else {
            break;
        };
        if phase != cur_phase {
            cur_phase = phase;
            PHASE.store(phase, Ordering::SeqCst);
            log11.line(&format!(
                "PHASE -> {} (elapsed {:.1}s)",
                PHASE_NAMES[phase as usize],
                elapsed.as_secs_f64()
            ));
        }
        let span = &mut rep.phase_span[phase as usize];
        if span.0.is_nan() {
            span.0 = elapsed.as_secs_f64();
        }
        span.1 = elapsed.as_secs_f64();

        // 1.7：設桌布階段改為「每次設定後 N 秒一列」時，這個階段不做週期取樣（只推進排程）。
        let periodic_off = phase == PH_SETTING && cfg.post_set_sample.is_some();
        if now >= next_sample && periodic_off {
            next_sample = now + cfg.sample;
        }
        if let Some((due, n)) = post_sample {
            if now >= due {
                post_sample = None;
                if take_sample(
                    phase,
                    elapsed.as_secs_f64(),
                    SampleKind::PostSet(n),
                    cfg.extra_cols,
                    &mut rep,
                    csv,
                    log11,
                    &mut last_wall,
                    &mut prev_exp,
                    &mut prev_self,
                ) {
                    rep.explorer_restarts += 1;
                    need_recreate = true;
                }
            }
        }
        if now >= next_sample {
            if take_sample(
                phase,
                elapsed.as_secs_f64(),
                SampleKind::Periodic,
                cfg.extra_cols,
                &mut rep,
                csv,
                log11,
                &mut last_wall,
                &mut prev_exp,
                &mut prev_self,
            ) {
                rep.explorer_restarts += 1;
                need_recreate = true;
            }
            next_sample += cfg.sample;
            if next_sample < now {
                next_sample = now + cfg.sample;
            }
        }

        if control_pending && elapsed >= Duration::from_secs(2) {
            control_pending = false;
            send_control_broadcast(log12);
        }

        if phase == PH_SETTING && now >= next_set {
            let n = rep.cycles.len();
            // explorer 重啟或上一輪 COM 呼叫失敗後，手上的介面（由 explorer 承載）可能已斷線：重建。
            if need_recreate || dw.is_none() {
                match create_dw() {
                    Ok(d) => {
                        *dw = Some(d);
                        need_recreate = false;
                        rep.com_recreates += 1;
                        log11.line(&format!(
                            "COM-RECREATE cycle={n} 已重新建立 IDesktopWallpaper（累計 {} 次，explorer 重啟觀察 {} 次）",
                            rep.com_recreates, rep.explorer_restarts
                        ));
                    }
                    Err(e) => {
                        *dw = None;
                        log11.line(&format!("WARN cycle={n} 重建 IDesktopWallpaper 失敗：{e}"));
                    }
                }
            }
            let cycle = do_cycle(dw.as_ref(), n, &imgs, cfg.monitor, log11);
            if let Some(after) = cfg.post_set_sample {
                post_sample = Some((cycle.end + after, n));
            }
            let ok = cycle.sets.iter().filter(|s| s.hr >= 0).count() as u64;
            let bad = cycle.sets.len() as u64 - ok;
            rep.set_ok += ok;
            rep.set_fail += bad;
            if cycle.enum_failed || bad > 0 {
                need_recreate = true;
            }
            // 列舉失敗、沒有任何在線螢幕可設（sets 為空）、或每次設定都失敗，都算「這個週期失敗」。
            if cycle.enum_failed || cycle.sets.is_empty() || (bad > 0 && ok == 0) {
                consecutive_fail_cycles += 1;
            } else {
                consecutive_fail_cycles = 0;
            }
            rep.cycles.push(cycle);
            next_set += cfg.interval;
            if next_set < Instant::now() {
                next_set = Instant::now() + cfg.interval;
            }
            if consecutive_fail_cycles >= MAX_CONSECUTIVE_FAIL_CYCLES {
                rep.abort_reason = Some(format!(
                    "連續 {MAX_CONSECUTIVE_FAIL_CYCLES} 個週期無法設桌布（列舉失敗、無在線螢幕或 SetWallpaper 全部失敗）"
                ));
                break;
            }
        }

        rep.acc.drain();
        finalize_ready(&mut rep, cfg.window, log12, false);
        thread::sleep(Duration::from_millis(100));
    }

    // 終點取樣（正常結束與中止都取，方便對照）。
    let elapsed = Instant::now().duration_since(t0).as_secs_f64();
    let phase = PHASE.load(Ordering::SeqCst);
    let _ = take_sample(
        phase,
        elapsed,
        SampleKind::Final,
        cfg.extra_cols,
        &mut rep,
        csv,
        log11,
        &mut last_wall,
        &mut prev_exp,
        &mut prev_self,
    );
    rep
}

#[allow(clippy::too_many_arguments)]
fn take_sample(
    phase: u8,
    elapsed_s: f64,
    kind: SampleKind,
    extra_cols: bool,
    rep: &mut RunReport,
    csv: &mut File,
    log11: &Log,
    last_wall: &mut Option<SystemTime>,
    prev_exp: &mut Option<ProcStat>,
    prev_self: &mut Option<ProcStat>,
) -> bool {
    let wall = SystemTime::now();
    let gap = last_wall
        .and_then(|l| wall.duration_since(l).ok())
        .map(|d| d.as_secs_f64());
    if let Some(g) = gap {
        if g > FREEZE_GAP_SECS {
            log11.line(&format!(
                "WARN 兩次取樣間隔 {g:.1}s 超過 {FREEZE_GAP_SECS:.0}s，行程疑似曾被凍結（待機／鎖定／休眠）"
            ));
        }
    }
    let pid = explorer_pid();
    let exp = (pid != 0).then(|| sample_pid(pid)).flatten();
    let me = sample_self();
    let locked = is_locked();

    let pid_changed = match (prev_exp.as_ref(), exp.as_ref()) {
        (Some(p), Some(e)) => p.pid != e.pid,
        _ => false,
    };
    if pid_changed {
        log11.line(&format!(
            "EXPLORER-RESTART explorer PID 改變 {} -> {}（explorer 重啟過）",
            prev_exp.map_or(0, |p| p.pid),
            exp.map_or(0, |e| e.pid)
        ));
    }
    if exp.is_none() {
        log11.line(&format!(
            "WARN 取不到 explorer 取樣（GetShellWindow 擁有者 PID={pid}，explorer 重啟中或存取被拒）"
        ));
    }
    let cpu_pct = |cur: &ProcStat, prev: &Option<ProcStat>| -> Option<f64> {
        let p = prev.as_ref().filter(|p| p.pid == cur.pid)?;
        let g = gap?;
        (g > 0.0).then(|| (cur.cpu_100ns.saturating_sub(p.cpu_100ns)) as f64 / 1e7 / g * 100.0)
    };
    let f = |v: Option<f64>| v.map_or(String::new(), |x| format!("{x:.2}"));
    let e = exp.unwrap_or_default();
    let have_e = exp.is_some();
    let ev = |x: u64| if have_e { x.to_string() } else { String::new() };
    let ev32 = |x: u32| if have_e { x.to_string() } else { String::new() };
    let mut row = format!(
        "{},{:.1},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
        stamp(),
        elapsed_s,
        PHASE_NAMES[phase as usize],
        gap.map_or(String::new(), |g| format!("{g:.1}")),
        u8::from(locked),
        rep.set_ok,
        rep.set_fail,
        pid,
        u8::from(pid_changed),
        if have_e {
            (e.cpu_100ns / 10_000).to_string()
        } else {
            String::new()
        },
        f(exp.as_ref().and_then(|c| cpu_pct(c, prev_exp))),
        ev(e.ws),
        ev(e.private),
        ev32(e.handles),
        ev32(e.gdi),
        ev32(e.gdi_peak),
        ev32(e.user),
        ev32(e.user_peak),
        me.cpu_100ns / 10_000,
        f(cpu_pct(&me, prev_self)),
        me.ws,
        me.private,
        me.handles,
        me.gdi,
        me.gdi_peak,
        me.user,
        me.user_peak
    );
    if extra_cols {
        let (kind_name, cycle) = match kind {
            SampleKind::Periodic => ("periodic", String::new()),
            SampleKind::PostSet(n) => ("post_set", n.to_string()),
            SampleKind::Final => ("final", String::new()),
        };
        let top = explorer_top_windows(pid);
        row.push_str(&format!(
            ",{kind_name},{cycle},{},{},{}",
            idle_secs().map_or(String::new(), |s| format!("{s:.1}")),
            top.map_or(String::new(), |t| t.0.to_string()),
            top.map_or(String::new(), |t| t.1.to_string()),
        ));
    }
    let _ = writeln!(csv, "{row}");
    let _ = csv.flush();
    println!("{}", redact(&row));

    if exp.is_some() {
        *prev_exp = exp;
    }
    *prev_self = Some(me);
    *last_wall = Some(wall);
    rep.rows.push(Row {
        phase,
        elapsed_s,
        exp,
        me,
    });
    pid_changed
}

/// 一個設桌布週期：對每台在線螢幕設圖，圖片在 A／B 之間依「週期＋螢幕序」交替，
/// 讓每次 SetWallpaper 都真的換圖。
fn do_cycle(
    dw: Option<&IDesktopWallpaper>,
    n: usize,
    imgs: &[(&'static str, &Path); 2],
    only: Option<u32>,
    log11: &Log,
) -> Cycle {
    let start = Instant::now();
    let start_stamp = stamp();
    let mut sets = Vec::new();
    let mut enum_failed = false;
    let listed = match dw {
        Some(d) => list_monitors(d),
        None => Err("沒有可用的 IDesktopWallpaper 介面".to_string()),
    };
    match (dw, listed) {
        (_, Err(e)) => {
            enum_failed = true;
            log11.line(&format!("WARN cycle {n} 列舉螢幕失敗：{e}"));
        }
        (None, Ok(_)) => {}
        (Some(dw), Ok(mons)) => {
            for (idx, (id, hr, _rect)) in mons.iter().enumerate() {
                // 1.7 `--monitor`：其他螢幕不碰、不記（啟動時已記一次 MONITOR 行）。
                if only.is_some_and(|m| m as usize != idx) {
                    continue;
                }
                if *hr != 0 {
                    log11.line(&format!(
                        "SKIP cycle={n} mon={idx} 離線（GetMonitorRECT hr=0x{:08X}）",
                        *hr as u32
                    ));
                    continue;
                }
                let (name, path) = imgs[(n + idx) % 2];
                let id_w = wide(id);
                let path_w = wide(&path.to_string_lossy());
                let t = Instant::now();
                let r = unsafe { dw.SetWallpaper(PCWSTR(id_w.as_ptr()), PCWSTR(path_w.as_ptr())) };
                let dur = t.elapsed();
                let hr = match &r {
                    Ok(()) => 0,
                    Err(e) => e.code().0,
                };
                log11.line(&format!(
                    "SET cycle={n} mon={idx} img={name} hr=0x{:08X} dur_ms={:.1}",
                    hr as u32,
                    dur.as_secs_f64() * 1000.0
                ));
                sets.push(SetRec {
                    monitor: idx,
                    image: name,
                    offset_ms: t.duration_since(start).as_secs_f64() * 1000.0,
                    dur_ms: dur.as_secs_f64() * 1000.0,
                    hr,
                });
            }
        }
    }
    Cycle {
        n,
        label: "set".to_string(),
        start,
        start_stamp,
        end: Instant::now(),
        sets,
        enum_failed,
    }
}

fn finalize_ready(rep: &mut RunReport, window: Duration, log12: &Log, force: bool) {
    while rep.finalized < rep.cycles.len() {
        let i = rep.finalized;
        let c = &rep.cycles[i];
        let ready = force || Instant::now() >= c.end + window;
        if !ready {
            break;
        }
        let next_start = rep.cycles.get(i + 1).map(|n| n.start);
        rep.acc.finalize(c, window, next_start, rep.t0, log12);
        rep.finalized += 1;
    }
}

fn summarize_messages(rep: &RunReport, log12: &Log) {
    let acc = &rep.acc;
    log12.line("SUMMARY ===== 訊息計數摘要 =====");
    for ph in [PH_BASELINE, PH_SETTING, PH_COOLDOWN, PH_RESTORE] {
        let span = rep.phase_span[ph as usize];
        let dur = if span.0.is_nan() {
            0.0
        } else {
            span.1 - span.0
        };
        let mut parts: Vec<String> = acc
            .counts
            .iter()
            .filter(|((p, _), _)| *p == ph)
            .map(|((_, k), v)| (key_display(k), *v))
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        parts.sort();
        log12.line(&format!(
            "SUMMARY phase={} approx_duration_s={:.0} all_received: {}",
            PHASE_NAMES[ph as usize],
            dur,
            if parts.is_empty() {
                "（無）".to_string()
            } else {
                parts.join(" ")
            }
        ));
    }
    let mut win: Vec<String> = acc
        .in_window
        .iter()
        .map(|(k, v)| format!("{}={v}", key_display(k)))
        .collect();
    win.sort();
    let n_set_cycles = rep.cycles.iter().filter(|c| c.label == "set").count();
    log12.line(&format!(
        "SUMMARY 設桌布後視窗內（共 {} 個週期，含還原週期 {}；其中有收到任何訊息的週期 {}）：{}",
        acc.cycles_final,
        acc.cycles_final.saturating_sub(n_set_cycles),
        acc.cycles_with_any,
        if win.is_empty() {
            "（無）".to_string()
        } else {
            win.join(" ")
        }
    ));
    for (title, map) in [("視窗內", &acc.sc_text_in), ("視窗外", &acc.sc_text_out)] {
        let mut v: Vec<String> = map.iter().map(|(k, c)| format!("{k:?}×{c}")).collect();
        v.sort();
        log12.line(&format!(
            "SUMMARY WM_SETTINGCHANGE lParam 字串（{title}）：{}",
            if v.is_empty() {
                "（無）".to_string()
            } else {
                v.join(" ")
            }
        ));
    }
}

/// `restore` 的保險包裝：panic 時再試一次（兩次都 panic 才放棄並回報失敗）。
/// 任何一次都不經過會 panic 的輸出（見檔頭的 println 巨集說明）。
fn restore_guarded(want: &Snapshot) -> (bool, Vec<String>) {
    let mut notes = Vec::new();
    for attempt in 1..=2 {
        match catch_unwind(AssertUnwindSafe(|| restore(want))) {
            Ok((ok, mut lines)) => {
                notes.append(&mut lines);
                return (ok, notes);
            }
            Err(_) => notes.push(format!("RESTORE-PANIC 還原第 {attempt}/2 次發生 panic")),
        }
    }
    (false, notes)
}

fn exit_code_restore(ok: bool) -> i32 {
    if ok {
        0
    } else {
        4
    }
}

fn print_and_log(log: Option<&Log>, line: &str) {
    match log {
        Some(l) => l.line(line),
        None => println!("{} {}", stamp(), redact(line)),
    }
}

fn mode_restore(path: &Path) -> i32 {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("無法讀取備份 {}：{e}", path.display());
            return 2;
        }
    };
    let snap: Snapshot = match serde_json::from_str(&text) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("備份 JSON 解析失敗：{e}");
            return 2;
        }
    };
    print_and_log(
        None,
        &format!("還原備份 {}（建立於 {}）", path.display(), snap.created),
    );
    let (ok, lines) = restore_guarded(&snap);
    for l in &lines {
        print_and_log(None, l);
    }
    print_and_log(
        None,
        if ok {
            "還原讀回：全部一致"
        } else {
            "還原讀回：有不一致（結束碼 4）"
        },
    );
    exit_code_restore(ok)
}

fn mode_backup_only(cfg: &Cfg) -> i32 {
    let dw = match create_dw() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let snap = match capture(&dw) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    if let Err(e) = fs::create_dir_all(&cfg.backup_dir) {
        eprintln!("無法建立備份目錄：{e}");
        return 2;
    }
    let path = cfg.backup_dir.join(format!("backup-{}.json", file_stamp()));
    let json = serde_json::to_string_pretty(&snap).unwrap_or_default();
    if let Err(e) = write_atomic(&path, json.as_bytes()) {
        eprintln!("{e}");
        return 2;
    }
    println!("備份已寫入 {}", redact(&path.display().to_string()));
    println!("{}", redact(&json));
    0
}

/// 檔名用時間戳（本機時間，無冒號）。
fn file_stamp() -> String {
    let t = unsafe { GetLocalTime() };
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    )
}

fn mode_run(cfg: &Cfg) -> i32 {
    // 前置檢查（尚未改任何桌布）。
    for (name, p) in [("--a", &cfg.image_a), ("--b", &cfg.image_b)] {
        match fs::metadata(p) {
            Ok(m) if m.is_file() && m.len() > 0 => {}
            _ => {
                eprintln!("測試圖 {name} 不存在或為空：{}", p.display());
                return 2;
            }
        }
    }
    if same_path(
        &cfg.image_a.to_string_lossy(),
        &cfg.image_b.to_string_lossy(),
    ) {
        eprintln!("--a 與 --b 不可為同一檔案");
        return 2;
    }
    if !cfg.allow_locked && is_locked() {
        eprintln!(
            "BLOCKED：工作階段鎖定中（LogonUI.exe 在跑），拒絕執行（要照跑請加 --allow-locked）"
        );
        return 3;
    }
    if let Err(e) =
        fs::create_dir_all(&cfg.out_dir).and_then(|()| fs::create_dir_all(&cfg.backup_dir))
    {
        eprintln!("無法建立輸出／備份目錄：{e}");
        return 2;
    }
    let dw = match create_dw() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let snap = match capture(&dw) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("備份失敗，拒絕執行：{e}");
            return 2;
        }
    };
    if snap.status.is_some_and(|s| s & DSS_SLIDESHOW.0 != 0) {
        eprintln!(
            "BLOCKED：目前是桌布投影片模式（GetStatus=0x{:X}），本探針無法還原投影片，拒絕執行",
            snap.status.unwrap_or(0)
        );
        return 3;
    }
    if let Some(idx) = cfg.monitor {
        match snap.monitors.get(idx as usize) {
            Some(m) if m.online => {}
            Some(_) => {
                eprintln!("--monitor {idx} 目前離線，拒絕執行");
                return 2;
            }
            None => {
                eprintln!(
                    "--monitor {idx} 超出範圍（共 {} 台，含離線）",
                    snap.monitors.len()
                );
                return 2;
            }
        }
    }
    for m in &snap.monitors {
        if let Some(p) = &m.wallpaper {
            for img in [&cfg.image_a, &cfg.image_b] {
                if same_path(p, &img.to_string_lossy()) {
                    eprintln!("測試圖與使用者目前桌布相同，拒絕執行：{p}");
                    return 2;
                }
            }
        }
    }

    // 備份先於一切：原子寫入，寫完才進入任何設定動作。
    let backup_path = cfg.backup_dir.join(format!("backup-{}.json", file_stamp()));
    let json = serde_json::to_string_pretty(&snap).unwrap_or_default();
    if let Err(e) = write_atomic(&backup_path, json.as_bytes()) {
        eprintln!("備份寫入失敗，拒絕執行：{e}");
        return 2;
    }

    let open = |name: String, echo: bool| Log::create(&cfg.out_dir.join(name), echo);
    let (log11, log12) = match (
        open(format!("dw-1.1-{}.log", cfg.tag), true),
        open(format!("dw-1.2-{}.log", cfg.tag), false),
    ) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let csv_path = cfg.out_dir.join(format!("dw-1.1-{}.csv", cfg.tag));
    let mut csv = match File::create(&csv_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("無法建立 CSV：{e}");
            return 2;
        }
    };
    let _ = writeln!(csv, "{}", csv_header(cfg.extra_cols));

    log11.line(&format!(
        "START probe_wallpaper tag={} baseline={}s setting={}s cooldown={}s interval={}s sample={}s window={}s",
        cfg.tag,
        cfg.baseline.as_secs(),
        cfg.setting.as_secs(),
        cfg.cooldown.as_secs(),
        cfg.interval.as_secs(),
        cfg.sample.as_secs(),
        cfg.window.as_secs()
    ));
    log11.line(&format!(
        "OPTIONS monitor={} sample_after_set={} extra_cols={}（1.7 參數；monitor=all、sample_after_set=off、extra_cols=false 即 1.1 預設行為）",
        cfg.monitor.map_or_else(|| "all".to_string(), |m| m.to_string()),
        cfg.post_set_sample
            .map_or_else(|| "off".to_string(), |d| format!("{}s", d.as_secs())),
        cfg.extra_cols
    ));
    if let Some(m) = cfg.monitor.and_then(|i| snap.monitors.get(i as usize)) {
        log11.line(&format!(
            "MONITOR 只設定 monitor[{}] id={} rect={:?}；其餘螢幕不呼叫 SetWallpaper",
            cfg.monitor.unwrap_or(0),
            m.id,
            m.rect
        ));
    }
    log11.line(&format!(
        "BACKUP 備份 JSON：{}（還原：probe_wallpaper --restore <此檔>）",
        backup_path.display()
    ));
    log11.line(&format!(
        "A={} B={}",
        cfg.image_a.display(),
        cfg.image_b.display()
    ));
    for (i, m) in snap.monitors.iter().enumerate() {
        log11.line(&format!(
            "BACKUP monitor[{i}] id={} online={} rect={:?} wallpaper={:?}",
            m.id, m.online, m.rect, m.wallpaper
        ));
    }
    log11.line(&format!(
        "BACKUP position={:?} background_color={:?} status={:?} registry={:?}",
        snap.position, snap.background_color, snap.status, snap.registry
    ));
    log12.line(&format!(
        "START dw-1.2 隱藏頂層視窗訊息計數（tag={}，視窗 {}s）；與 dw-1.1-{}.log 的 SET 行時間戳可交叉對照。注意：CYCLE／OUTSIDE 行是視窗期結束後才寫檔，行首時間戳為寫檔時間，事件時間看 start=／t+ 欄位",
        cfg.tag,
        cfg.window.as_secs(),
        cfg.tag
    ));

    // 防待機：從基線開始到還原完成（本執行緒存活整段）。不改電源計畫或登錄。
    let prev = unsafe {
        SetThreadExecutionState(ES_CONTINUOUS | ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED)
    };
    log11.line(&format!(
        "POWER SetThreadExecutionState(CONTINUOUS|DISPLAY_REQUIRED|SYSTEM_REQUIRED) -> prev=0x{:X}（0＝失敗）",
        prev.0
    ));

    unsafe {
        // 清除從父行程繼承的「忽略 Ctrl+C」旗標（工具環境啟動的行程常帶著），否則 Ctrl+C 到不了 handler。
        let _ = SetConsoleCtrlHandler(None, false);
        let _ = SetConsoleCtrlHandler(Some(ctrl_handler), true);
    }
    let sink = match spawn_sink() {
        Ok(h) => {
            log12.line("SINK 隱藏頂層視窗已建立（非 message-only），訊息迴圈執行緒運行中");
            Some(h)
        }
        Err(e) => {
            log11.line(&format!(
                "WARN 訊息視窗建立失敗，1.2 計數無效，仍繼續 1.1：{e}"
            ));
            None
        }
    };

    let mut dw = Some(dw);
    let run = catch_unwind(AssertUnwindSafe(|| {
        run_loop(cfg, &mut dw, &mut csv, &log11, &log12, sink.is_some())
    }));
    // 手上的介面（由 explorer 承載）可能已斷線；還原一律重新建立，不沿用。
    drop(dw);
    let main_loop_panicked = run.is_err();
    let mut rep = match run {
        Ok(r) => r,
        Err(_) => RunReport {
            abort_reason: Some("主迴圈 panic".to_string()),
            rows: Vec::new(),
            acc: MsgAcc::default(),
            cycles: Vec::new(),
            t0: Instant::now(),
            phase_span: [(f64::NAN, f64::NAN); 5],
            set_ok: 0,
            set_fail: 0,
            finalized: 0,
            com_recreates: 0,
            explorer_restarts: 0,
        },
    };

    // 還原（也當成一個「週期」記錄訊息，看還原時有沒有廣播）。**還原先於任何記錄輸出**：
    // 輸出寫不出去、檔案鎖等都不得妨礙還原（H1）。
    PHASE.store(PH_RESTORE, Ordering::SeqCst);
    let r_start = Instant::now();
    let r_stamp = stamp();
    TEST_FAIL_CREATES.store(cfg.test_restore_fail_creates, Ordering::SeqCst);
    let (restore_ok, lines) = restore_guarded(&snap);
    TEST_FAIL_CREATES.store(0, Ordering::SeqCst);
    if main_loop_panicked {
        log11.line("ABORT 主迴圈 panic，已進入還原");
    }
    if let Some(r) = &rep.abort_reason {
        log11.line(&format!("ABORT 中止原因：{r}"));
    }
    log11.line("RESTORE 還原桌布（以下為還原過程記錄；還原已先於本行執行）");
    for l in &lines {
        log11.line(l);
    }
    rep.phase_span[PH_RESTORE as usize] = (
        r_start.duration_since(rep.t0).as_secs_f64(),
        Instant::now().duration_since(rep.t0).as_secs_f64(),
    );
    rep.cycles.push(Cycle {
        n: rep.cycles.len(),
        label: "restore".to_string(),
        start: r_start,
        start_stamp: r_stamp,
        end: Instant::now(),
        sets: Vec::new(),
        enum_failed: false,
    });
    RESTORE_DONE.store(true, Ordering::SeqCst);
    log11.line(&format!(
        "RESTORE 讀回結論：{}（備份 JSON：{}）",
        if restore_ok {
            "全部一致"
        } else {
            "有不一致！請看上方 DIFF 行，並以 --restore 重試或手動處理"
        },
        backup_path.display()
    ));

    // 等最後一個視窗期，結算所有週期與殘餘的視窗外訊息。
    thread::sleep(cfg.window + Duration::from_millis(500));
    rep.acc.drain();
    finalize_ready(&mut rep, cfg.window, &log12, true);
    let t0 = rep.t0;
    rep.acc.flush_outside(None, t0, &log12);
    summarize_messages(&rep, &log12);
    trend_report(&rep.rows, &log11);
    log11.line(&format!(
        "DONE 設桌布成功 {} 次、失敗 {} 次；週期 {} 個；取樣 {} 列；探針共收到 {} 則訊息；\
         explorer 重啟觀察 {} 次、IDesktopWallpaper 重建 {} 次",
        rep.set_ok,
        rep.set_fail,
        rep.cycles.iter().filter(|c| c.label == "set").count(),
        rep.rows.len(),
        rep.acc.all.len(),
        rep.explorer_restarts,
        rep.com_recreates
    ));

    if let Some(h) = sink {
        stop_sink(h);
    }
    unsafe {
        let _ = SetConsoleCtrlHandler(Some(ctrl_handler), false);
        // 清除顯示器／系統保持開啟的要求（只剩 ES_CONTINUOUS）。
        let _ = SetThreadExecutionState(ES_CONTINUOUS);
    }

    if !restore_ok {
        4
    } else if rep
        .abort_reason
        .as_deref()
        .is_some_and(|r| !r.starts_with("收到"))
    {
        5
    } else {
        0
    }
}

fn main() {
    let cfg = match parse_args() {
        Ok(c) => c,
        Err(e) => {
            if e.is_empty() {
                println!("{USAGE}");
                std::process::exit(0);
            }
            eprintln!("{e}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if hr.is_err() {
        eprintln!("CoInitializeEx 失敗：{hr:?}");
        std::process::exit(2);
    }
    let code = match &cfg.mode {
        Mode::Run => mode_run(&cfg),
        Mode::BackupOnly => mode_backup_only(&cfg),
        Mode::Restore(p) => mode_restore(p),
    };
    unsafe { CoUninitialize() };
    std::process::exit(code);
}
