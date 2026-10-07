//! task 4.5 渲染管線自我測試（`--self-test-render`，需 `self-test-ipc` cargo feature；正式 release
//! build 不含本模組）。
//!
//! 與 `--self-test-ipc` 不同，本模式**走正常啟動**：單一執行個體仲裁、設定、小工具視窗、置底守門
//! 全部照常，`setup` 完成後才另開一條執行緒，以正式的 [`WallpaperRenderer`]（獨立 WebView2 環境、
//! 每次開關的隱藏視窗）依固定間隔出圖。這樣量到的才是「小工具那組常駐＋渲染視窗每次開關」的真實
//! 宿主（task 4.5 實測三件事）。驅動稿 `host/tools/measure-dw-4.5.ps1` 負責隔離環境與記憶體取樣。
//!
//! ## 用法
//!
//! ```text
//! fc-host.exe --self-test-render --render-log <path>
//!             [--count N | --duration-secs D] [--interval-secs S] [--warmup-secs W] [--tail-secs T]
//!             [--width 3840] [--height 2160] [--theme astrolabe] [--no-fixture]
//!             [--timeout-at N | --skip-render]
//! ```
//!
//! - `--count N`：渲染 N 次；沒給時以 `--duration-secs D` 換算（`max(1, D / S)` 次）；都沒給＝1 次。
//! - `--interval-secs S`（預設 60）：相鄰兩次的**開始**間隔。
//! - `--warmup-secs W`（預設 0）：第一次渲染前先等（讓小工具那組的記憶體穩定，當基準）。
//! - `--tail-secs T`（預設 0）：最後一次之後再等（觀察關窗後的殘留）。
//! - 每次的 `t` 從 2026-10-05T00:30:00Z 起每次加 15 分鐘（星盤每格不同的圖，同探針 1.4）；
//!   資料走頁面的 fixture（`../fixtures/tw-events.json`，嵌在 exe 內），`--no-fixture` 改走
//!   `wallpaper` 通道（4.6 之前通道不存在，頁面畫缺資料畫面）。
//! - 輸出寫進 `wallpaper_state::default_paths().output_dir`（隔離的 `%LOCALAPPDATA%` 底下），
//!   螢幕鍵固定為 `monitor_key("self-test", None, &[])`；每次把上一張當成「螢幕上正顯示的」
//!   傳入，驗證 a／b 交替。
//! - `--timeout-at N`：第 N 次改畫一定不回報的頁面（`wallpaper_render::NEVER_REPORTS_PAGE`），在真實
//!   宿主上走一次 30 秒逾時：成功的判準是回 `Timeout` 且視窗已關閉（`window_closed == Some(true)`），
//!   記錄 `RENDER-FORCED-TIMEOUT … PASS`。驅動稿另看渲染那組 browser 行程是否結束。
//! - `--skip-render`：對照組，同一個時刻表照記 `RENDER-START`，但不建立視窗（`RENDER-SKIPPED`）。
//! - 結束時 `app.exit(結束碼)`：0＝全部成功且讀回尺寸正確；1＝有失敗；參數錯誤在啟動時以 2 結束。
//!
//! 記錄檔逐行 `<UNIX 毫秒> <事件>`：`RENDER-START`、`RENDER-OK`／`RENDER-FAIL`（含 render id、
//! 各段耗時）、`VERIFY`、`SUMMARY`。驅動稿以 UNIX 毫秒與記憶體取樣對齊。

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Manager};

use crate::settings::WallpaperTheme;
use crate::wallpaper_render::{
    png_dimensions, BrowserProcess, OutputTarget, RenderFailureReason, RenderRequest,
    WallpaperRenderer, NEVER_REPORTS_PAGE,
};
use crate::wallpaper_state::{default_paths, monitor_key};

/// 驅動稿以這個字串確認 exe 是帶 `self-test-ipc` feature、且含本模式的建置。
pub const BUILD_MARKER: &str = "FC_HOST_SELF_TEST_RENDER_BUILD";
/// 第一次的 `t`：2026-10-05T00:30:00Z（台北週一 08:30）。
const T_BASE: i64 = 1_791_160_200;
const T_STEP: i64 = 15 * 60;
const FIXTURE: &str = "../fixtures/tw-events.json";
/// 觀察每個渲染 browser 行程結束的上限（bug-leaked-renderer；正常約 1.4 秒）。
const BROWSER_WATCH: Duration = Duration::from_secs(60);

/// `--self-test-render` 的參數。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    pub log: PathBuf,
    pub count: u32,
    pub interval: Duration,
    pub warmup: Duration,
    pub tail: Duration,
    pub width: u32,
    pub height: u32,
    pub theme: WallpaperTheme,
    pub fixture: bool,
    /// 第幾次（1 起算）改畫一定不回報的頁面，在真實宿主上走一次 30 秒逾時路徑（`--timeout-at N`）。
    pub timeout_at: Option<u32>,
    /// 對照組：照同樣的時刻表跑但不渲染（`--skip-render`）。
    pub skip_render: bool,
    /// bug-leaked-renderer 重現：每次渲染**返回後**等這麼久就開始下一次（取代 `--interval-secs` 的
    /// 固定開始間隔；`--gap-ms G`）。正式排程同一輪兩台螢幕的節奏約 1,000 ms。
    pub gap: Option<Duration>,
    /// 偶數次（第 2、4、…次）改用這個尺寸（`--alt-size WxH`；模擬同一輪兩台不同解析度的螢幕）。
    pub alt_size: Option<(u32, u32)>,
    /// 與 `--gap-ms` 並用：每兩次（偶數次之後）改等這麼久（`--pair-idle-ms P`；模擬兩輪之間的閒置）。
    pub pair_idle: Option<Duration>,
}

/// 解析命令列；沒有 `--self-test-render` 回 `None`。其他旗標（`--autostart` 等）略過。
pub fn parse_args<I: IntoIterator<Item = String>>(args: I) -> Option<Result<Args, String>> {
    let mut present = false;
    let mut log = None;
    let mut count: Option<u32> = None;
    let mut duration: Option<u64> = None;
    let mut interval = 60u64;
    let mut warmup = 0u64;
    let mut tail = 0u64;
    let mut width = 3840u32;
    let mut height = 2160u32;
    let mut theme = WallpaperTheme::Astrolabe;
    let mut fixture = true;
    let mut timeout_at: Option<u32> = None;
    let mut skip_render = false;
    let mut gap: Option<u64> = None;
    let mut alt_size: Option<(u32, u32)> = None;
    let mut pair_idle: Option<u64> = None;
    let mut it = args.into_iter();
    let mut error = None;
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} 缺少值"));
        let r: Result<(), String> = (|| {
            match a.as_str() {
                "--self-test-render" => present = true,
                "--render-log" => log = Some(PathBuf::from(value("--render-log")?)),
                "--count" => count = Some(num(&value("--count")?, "--count")?),
                "--duration-secs" => {
                    duration = Some(num(&value("--duration-secs")?, "--duration-secs")?)
                }
                "--interval-secs" => interval = num(&value("--interval-secs")?, "--interval-secs")?,
                "--warmup-secs" => warmup = num(&value("--warmup-secs")?, "--warmup-secs")?,
                "--tail-secs" => tail = num(&value("--tail-secs")?, "--tail-secs")?,
                "--width" => width = num(&value("--width")?, "--width")?,
                "--height" => height = num(&value("--height")?, "--height")?,
                "--theme" => {
                    let v = value("--theme")?;
                    theme = WallpaperTheme::ALL
                        .into_iter()
                        .find(|t| t.as_str() == v && *t != WallpaperTheme::None)
                        .ok_or_else(|| format!("--theme 不認得：{v}"))?;
                }
                "--no-fixture" => fixture = false,
                "--timeout-at" => timeout_at = Some(num(&value("--timeout-at")?, "--timeout-at")?),
                "--skip-render" => skip_render = true,
                "--gap-ms" => gap = Some(num(&value("--gap-ms")?, "--gap-ms")?),
                "--pair-idle-ms" => {
                    pair_idle = Some(num(&value("--pair-idle-ms")?, "--pair-idle-ms")?)
                }
                "--alt-size" => {
                    let v = value("--alt-size")?;
                    let (w, h) = v
                        .split_once('x')
                        .ok_or_else(|| format!("--alt-size 要寫成 WxH：{v}"))?;
                    alt_size = Some((num(w, "--alt-size")?, num(h, "--alt-size")?));
                }
                _ => {}
            }
            Ok(())
        })();
        if let Err(e) = r {
            error.get_or_insert(e);
        }
    }
    if !present {
        return None;
    }
    if let Some(e) = error {
        return Some(Err(e));
    }
    let Some(log) = log else {
        return Some(Err("--self-test-render 需要 --render-log <path>".into()));
    };
    if interval == 0 {
        return Some(Err("--interval-secs 不可為 0".into()));
    }
    let count: u32 = match (count, duration) {
        (Some(0), _) => return Some(Err("--count 不可為 0".into())),
        (Some(n), _) => n,
        (None, Some(d)) => u32::try_from((d / interval).max(1)).unwrap_or(u32::MAX),
        (None, None) => 1,
    };
    match timeout_at {
        Some(n) if n == 0 || n > count => {
            return Some(Err(format!("--timeout-at 必須在 1..={count}")))
        }
        Some(_) if skip_render => {
            return Some(Err("--timeout-at 不能與 --skip-render 同時使用".into()))
        }
        _ => {}
    }
    Some(Ok(Args {
        log,
        count,
        interval: Duration::from_secs(interval),
        warmup: Duration::from_secs(warmup),
        tail: Duration::from_secs(tail),
        width,
        height,
        theme,
        fixture,
        timeout_at,
        skip_render,
        gap: gap.map(Duration::from_millis),
        alt_size,
        pair_idle: pair_idle.map(Duration::from_millis),
    }))
}

fn num<T: std::str::FromStr>(v: &str, name: &str) -> Result<T, String> {
    v.parse().map_err(|_| format!("{name} 不是合法的數字：{v}"))
}

fn epoch_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis())
}

struct Log(Mutex<fs::File>);

impl Log {
    fn line(&self, msg: &str) {
        if let Ok(mut f) = self.0.lock() {
            let _ = writeln!(f, "{} {msg}", epoch_ms());
            let _ = f.flush();
        }
        log::info!("self-test-render：{msg}");
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// 中位數（空＝`None`）。
pub fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    })
}

fn read_dims(path: &Path) -> Option<(u32, u32)> {
    let mut head = [0u8; 24];
    fs::File::open(path).ok()?.read_exact(&mut head).ok()?;
    png_dimensions(&head)
}

/// 在新執行緒跑完整個自我測試，最後 `app.exit(結束碼)`。
pub fn spawn(app: AppHandle, args: Args) {
    thread::spawn(move || {
        let code = run(&app, &args);
        app.exit(code);
    });
}

fn run(app: &AppHandle, args: &Args) -> i32 {
    let file = match fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&args.log)
    {
        Ok(f) => f,
        Err(e) => {
            log::error!("self-test-render：開不了記錄檔 {}：{e}", args.log.display());
            return 2;
        }
    };
    let log = Arc::new(Log(Mutex::new(file)));
    log.line(&format!(
        "START {BUILD_MARKER} pid={} args={args:?}",
        std::process::id()
    ));
    let Some(renderer) = app.try_state::<WallpaperRenderer>() else {
        log.line("ABORT 渲染管線沒有登記（WebView2 使用者資料夾解析失敗）");
        return 2;
    };
    log.line(&format!(
        "RENDERER-UDF {}",
        renderer.surface().data_dir().display()
    ));
    let out_dir = default_paths().output_dir;
    let key = monitor_key("self-test", None, &[]);
    log.line(&format!("OUTPUT dir={} key={key}", out_dir.display()));

    if !args.warmup.is_zero() {
        log.line(&format!("WARMUP {} s", args.warmup.as_secs()));
        thread::sleep(args.warmup);
    }

    let start = Instant::now();
    let mut ok = 0u32;
    let mut to_png = Vec::new();
    let mut open = Vec::new();
    let mut page = Vec::new();
    let mut close = Vec::new();
    let mut displayed: Option<PathBuf> = None;
    // bug-leaked-renderer：每次渲染那組的 browser 行程（n, 行程），結束前逐一核對是否殘留。
    let mut browsers: Vec<(u32, Arc<dyn BrowserProcess>)> = Vec::new();
    let mut last_return: Option<Instant> = None;
    for i in 0..args.count {
        let due = match (args.gap, last_return) {
            (Some(gap), Some(t)) => match args.pair_idle {
                Some(idle) if i % 2 == 0 => t + idle,
                _ => t + gap,
            },
            (Some(_), None) => start,
            (None, _) => start + args.interval * i,
        };
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            thread::sleep(wait);
        }
        if let Some((n, prev)) = browsers.last() {
            log.line(&format!(
                "PREV-BROWSER n={} prev_n={n} pid={} alive={}",
                i + 1,
                prev.pid(),
                !prev.wait_exit(Duration::ZERO)
            ));
        }
        let (width, height) = match args.alt_size {
            Some(alt) if (i + 1) % 2 == 0 => alt,
            _ => (args.width, args.height),
        };
        let req = RenderRequest {
            monitor_key: key.clone(),
            width,
            height,
            theme: args.theme,
            as_of: T_BASE + T_STEP * i64::from(i),
            tz: "Asia/Taipei".into(),
            fixture: args.fixture.then(|| FIXTURE.to_owned()),
        };
        log.line(&format!("RENDER-START n={}", i + 1));
        let target = OutputTarget {
            dir: &out_dir,
            displayed: displayed.as_deref(),
        };
        if args.skip_render {
            // 對照組：同一個時刻表，不建立渲染視窗。
            log.line(&format!("RENDER-SKIPPED n={}", i + 1));
            ok += 1;
            continue;
        }
        if args.timeout_at == Some(i + 1) {
            // 在真實宿主上走一次逾時路徑：專用頁存在但永遠不回報（見 NEVER_REPORTS_PAGE 文件）。
            let t = Instant::now();
            let result = renderer.render_custom_page(&req, &target, NEVER_REPORTS_PAGE);
            let elapsed = ms(t.elapsed());
            let passed = match &result {
                Err(f) => {
                    let passed = matches!(f.reason, RenderFailureReason::Timeout(_))
                        && f.window_closed == Some(true);
                    log.line(&format!(
                        "RENDER-FORCED-TIMEOUT n={} id={:?} kind={:?} window_closed={:?} elapsed_ms={elapsed:.1} {} {f}",
                        i + 1,
                        f.render_id,
                        f.kind(),
                        f.window_closed,
                        if passed { "PASS" } else { "FAIL" }
                    ));
                    passed
                }
                Ok(s) => {
                    log.line(&format!(
                        "RENDER-FORCED-TIMEOUT n={} id={} 不應成功 FAIL",
                        i + 1,
                        s.render_id
                    ));
                    false
                }
            };
            if passed {
                ok += 1;
            }
            continue;
        }
        let result = renderer.render(&req, &target);
        last_return = Some(Instant::now());
        // 這次的 browser 行程：另開執行緒等它結束（最多 BROWSER_WATCH），記下返回後多久結束。
        if let Some(b) = renderer.last_browser() {
            let n = i + 1;
            if browsers.last().is_some_and(|(_, p)| p.pid() == b.pid()) {
                log.line(&format!("BROWSER-REUSED n={n} pid={}", b.pid()));
            }
            let watch = Arc::clone(&b);
            let wlog = Arc::clone(&log);
            let returned = Instant::now();
            thread::spawn(move || {
                let exited = watch.wait_exit(BROWSER_WATCH);
                wlog.line(&format!(
                    "{} n={n} pid={} after_return_ms={:.1}",
                    if exited {
                        "BROWSER-EXIT"
                    } else {
                        "BROWSER-NOT-EXITED"
                    },
                    watch.pid(),
                    ms(returned.elapsed())
                ));
            });
            browsers.push((n, b));
        }
        match result {
            Ok(s) => {
                let t = s.timings;
                log.line(&format!(
                    "RENDER-OK n={} id={} browser_pid={:?} slot={:?} path={} wh={}x{} body={} open_ms={:.1} page_ms={:.1} to_png_ms={:.1} close_ms={:.1} write_ms={:.1} warnings={}",
                    i + 1,
                    s.render_id,
                    s.browser_pid,
                    s.slot,
                    s.path.display(),
                    s.width,
                    s.height,
                    s.body.as_str(),
                    ms(t.open),
                    ms(t.page),
                    ms(t.to_png()),
                    ms(t.close),
                    ms(t.write),
                    s.warnings.len()
                ));
                let dims = read_dims(&s.path);
                let own_files = fs::read_dir(&out_dir)
                    .map(|d| {
                        d.flatten()
                            .filter(|e| {
                                e.file_name()
                                    .to_string_lossy()
                                    .starts_with(&format!("{key}-"))
                            })
                            .count()
                    })
                    .unwrap_or(0);
                let verified = dims == Some((width, height)) && own_files <= 2;
                log.line(&format!(
                    "VERIFY n={} dims={dims:?} expect={}x{} own_files={own_files} {}",
                    i + 1,
                    width,
                    height,
                    if verified { "PASS" } else { "FAIL" }
                ));
                if verified {
                    ok += 1;
                }
                to_png.push(ms(t.to_png()));
                open.push(ms(t.open));
                page.push(ms(t.page));
                close.push(ms(t.close));
                displayed = Some(s.path);
            }
            Err(f) => log.line(&format!("RENDER-FAIL n={} kind={:?} {f}", i + 1, f.kind())),
        }
    }
    log.line("RENDER-LOOP-END");
    if !args.tail.is_zero() {
        log.line(&format!("TAIL {} s", args.tail.as_secs()));
        thread::sleep(args.tail);
    }
    // 殘留：到這時（最後一次之後再等 `--tail-secs`）仍存活的渲染 browser 行程。
    let leaked: Vec<String> = browsers
        .iter()
        .filter(|(_, b)| !b.wait_exit(Duration::ZERO))
        .map(|(n, b)| format!("n{n}:pid{}", b.pid()))
        .collect();
    log.line(&format!(
        "BROWSERS total={} leaked={} [{}]",
        browsers.len(),
        leaked.len(),
        leaked.join(" ")
    ));
    // 宿主結束前的處置（同系統匣「結束」；最後一個卡住的話由這裡結束它）。
    log.line(&format!("SHUTDOWN-SETTLE {:?}", renderer.shutdown()));
    let max = |v: &[f64]| v.iter().copied().fold(f64::NAN, f64::max);
    log.line(&format!(
        "SUMMARY ok={ok}/{} to_png_median_ms={:.1} to_png_max_ms={:.1} open_median_ms={:.1} open_max_ms={:.1} page_median_ms={:.1} page_max_ms={:.1} close_median_ms={:.1}",
        args.count,
        median(&to_png).unwrap_or(f64::NAN),
        max(&to_png),
        median(&open).unwrap_or(f64::NAN),
        max(&open),
        median(&page).unwrap_or(f64::NAN),
        max(&page),
        median(&close).unwrap_or(f64::NAN),
    ));
    let code = if ok == args.count { 0 } else { 1 };
    log.line(&format!("EXIT code={code}"));
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(v: &[&str]) -> Option<Result<Args, String>> {
        parse_args(v.iter().map(|s| (*s).to_owned()))
    }

    #[test]
    fn absent_flag_is_none() {
        assert_eq!(parse(&["--autostart", "--count", "3"]), None);
    }

    #[test]
    fn defaults_one_4k_astrolabe_with_fixture() {
        let a = parse(&["--self-test-render", "--render-log", "x.log"])
            .unwrap()
            .unwrap();
        assert_eq!(a.count, 1);
        assert_eq!((a.width, a.height), (3840, 2160));
        assert_eq!(a.theme, WallpaperTheme::Astrolabe);
        assert!(a.fixture);
        assert_eq!(a.interval, Duration::from_secs(60));
        assert_eq!(a.log, PathBuf::from("x.log"));
    }

    #[test]
    fn duration_converts_to_count() {
        let a = parse(&[
            "--self-test-render",
            "--render-log",
            "x",
            "--duration-secs",
            "7200",
            "--interval-secs",
            "900",
            "--warmup-secs",
            "120",
            "--tail-secs",
            "60",
            "--no-fixture",
            "--theme",
            "skyline",
        ])
        .unwrap()
        .unwrap();
        assert_eq!(a.count, 8, "2 小時、15 分鐘間隔＝8 次");
        assert_eq!(a.warmup, Duration::from_secs(120));
        assert_eq!(a.tail, Duration::from_secs(60));
        assert!(!a.fixture);
        assert_eq!(a.theme, WallpaperTheme::Skyline);
        let a = parse(&[
            "--self-test-render",
            "--render-log",
            "x",
            "--duration-secs",
            "1800",
            "--interval-secs",
            "60",
        ])
        .unwrap()
        .unwrap();
        assert_eq!(a.count, 30);
    }

    #[test]
    fn rejects_bad_values() {
        for bad in [
            vec!["--self-test-render"],
            vec!["--self-test-render", "--render-log", "x", "--count", "0"],
            vec!["--self-test-render", "--render-log", "x", "--count", "abc"],
            vec![
                "--self-test-render",
                "--render-log",
                "x",
                "--interval-secs",
                "0",
            ],
            vec!["--self-test-render", "--render-log", "x", "--theme", "none"],
            vec!["--self-test-render", "--render-log"],
        ] {
            assert!(matches!(parse(&bad), Some(Err(_))), "{bad:?}");
        }
    }

    #[test]
    fn forced_timeout_and_control_flags() {
        let a = parse(&[
            "--self-test-render",
            "--render-log",
            "x",
            "--count",
            "8",
            "--timeout-at",
            "4",
        ])
        .unwrap()
        .unwrap();
        assert_eq!(a.timeout_at, Some(4));
        assert!(!a.skip_render);
        let a = parse(&[
            "--self-test-render",
            "--render-log",
            "x",
            "--count",
            "8",
            "--skip-render",
        ])
        .unwrap()
        .unwrap();
        assert!(a.skip_render);
        assert_eq!(a.timeout_at, None);
        for bad in [
            vec![
                "--self-test-render",
                "--render-log",
                "x",
                "--count",
                "8",
                "--timeout-at",
                "0",
            ],
            vec![
                "--self-test-render",
                "--render-log",
                "x",
                "--count",
                "8",
                "--timeout-at",
                "9",
            ],
            vec![
                "--self-test-render",
                "--render-log",
                "x",
                "--count",
                "8",
                "--timeout-at",
                "2",
                "--skip-render",
            ],
        ] {
            assert!(matches!(parse(&bad), Some(Err(_))), "{bad:?}");
        }
    }

    #[test]
    fn leaked_renderer_repro_flags() {
        let a = parse(&["--self-test-render", "--render-log", "x"])
            .unwrap()
            .unwrap();
        assert_eq!((a.gap, a.alt_size, a.pair_idle), (None, None, None));
        let a = parse(&[
            "--self-test-render",
            "--render-log",
            "x",
            "--gap-ms",
            "1000",
            "--alt-size",
            "3840x2160",
            "--pair-idle-ms",
            "8000",
        ])
        .unwrap()
        .unwrap();
        assert_eq!(a.gap, Some(Duration::from_millis(1000)));
        assert_eq!(a.alt_size, Some((3840, 2160)));
        assert_eq!(a.pair_idle, Some(Duration::from_millis(8000)));
        for bad in [
            vec!["--self-test-render", "--render-log", "x", "--gap-ms", "-1"],
            vec![
                "--self-test-render",
                "--render-log",
                "x",
                "--alt-size",
                "3840",
            ],
            vec![
                "--self-test-render",
                "--render-log",
                "x",
                "--alt-size",
                "ax2",
            ],
        ] {
            assert!(matches!(parse(&bad), Some(Err(_))), "{bad:?}");
        }
    }

    #[test]
    fn median_of_even_and_odd() {
        assert_eq!(median(&[]), None);
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&[4.0, 1.0, 2.0, 3.0]), Some(2.5));
    }
}
