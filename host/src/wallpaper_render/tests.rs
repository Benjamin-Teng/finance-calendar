//! 渲染管線單元測試（task 4.5）：假渲染器（[`FakeSurface`]）取代 Tauri 隱藏視窗，其餘——render
//! id 配對、逾時、尺寸驗證、a/b 寫檔與殘檔清除、本體解碼——都是正式程式碼。排程器＋假渲染器的
//! 組合測試模擬 4.7 的呼叫端契約（`crate::wallpaper` 模組文件）。

use std::collections::{HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use tauri::ipc::InvokeBody;

use super::*;
use crate::settings::WallpaperTheme;
use crate::wallpaper::{
    next_action, retry_delay, Action, CivilDate, DataDates, ExplorerResources, FailureKind,
    FixedOffset, MonitorGeometry, MonitorSnapshot, SchedulerInput, SchedulerState,
};
use crate::widgets::PauseReason;

const KEY: &str = "m-0123456789abcdef";
const OTHER_KEY: &str = "m-fedcba9876543210";

// ── 測試輔助 ──────────────────────────────────────────────────────────────────────────────

/// 最小的「看得懂尺寸」PNG：簽章＋IHDR（CRC 不驗）＋一個標記位元組，用來分辨是哪一次的圖。
fn fake_png(w: u32, h: u32, tag: u8) -> Vec<u8> {
    let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    v.extend_from_slice(&13u32.to_be_bytes());
    v.extend_from_slice(b"IHDR");
    v.extend_from_slice(&w.to_be_bytes());
    v.extend_from_slice(&h.to_be_bytes());
    v.extend_from_slice(&[8, 2, 0, 0, 0]);
    v.extend_from_slice(&[0, 0, 0, 0]);
    v.extend_from_slice(b"tag");
    v.push(tag);
    v
}

fn meta_header(rid: &str, w: u32, h: u32, warnings: &[&str]) -> String {
    let meta = json!({ "rid": rid, "w": w, "h": h, "tz": "Asia/Taipei", "nowMs": 0,
        "warnings": warnings, "dataStatus": "ok", "dataSource": "fixture" });
    utf8_percent_encode(&meta.to_string())
}

/// `encodeURIComponent` 的等價（測試用；只保留英數與 `-_.!~*'()`）。
fn utf8_percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// 從頁面網址取出 `rid` 參數（假渲染器扮演頁面：頁面只看得到自己的網址）。
fn rid_of(url: &str) -> String {
    url.split(['?', '&'])
        .find_map(|kv| kv.strip_prefix("rid="))
        .expect("網址應帶 rid")
        .to_owned()
}

/// 假渲染器一次「開窗」的行為。
#[derive(Debug, Clone)]
enum Behavior {
    /// 回報成功，原始位元組本體。
    Png { w: u32, h: u32, tag: u8 },
    /// 回報成功，JSON 數字陣列本體（postMessage 退路）。
    PngJsonArray { w: u32, h: u32, tag: u8 },
    /// 回報成功但本體是 JSON 物件（壞本體）。
    BadBody,
    /// 頁面回報失敗。
    PageFailed(&'static str),
    /// 什麼都不回報（模組沒執行、卡住）。
    Silent,
    /// 先送一份 rid 不符的 PNG（例如上一次逾時後晚到），再送正確的。
    StaleThenPng {
        stale_rid: String,
        w: u32,
        h: u32,
        stale_tag: u8,
        tag: u8,
    },
    /// 只送 rid 不符的 PNG。
    OnlyStale { stale_rid: String, w: u32, h: u32 },
    /// 成功但 meta 沒帶 rid。
    PngWithoutRid { w: u32, h: u32 },
    /// 另一條執行緒延遲後才回報（非同步）。
    DelayedPng {
        delay_ms: u64,
        w: u32,
        h: u32,
        tag: u8,
    },
    /// 開窗失敗。
    OpenError,
    /// 建立視窗卡住（WebView2 環境建立不返回），直到 [`Gate::release`]；之後回傳一個不回報的視窗。
    BlockOpen(Gate),
    /// 建立視窗的呼叫 panic（例如 Tauri 內部 unwrap）。
    PanicOpen,
    /// 建立視窗本身（同步）就花掉 `open_ms`，返回後再過 `report_ms` 才回報。
    SlowOpenThenDelayedPng {
        open_ms: u64,
        report_ms: u64,
        w: u32,
        h: u32,
        tag: u8,
    },
}

/// 測試用的閘門：`wait` 一直等到 `release`。
#[derive(Debug, Clone, Default)]
struct Gate(Arc<(Mutex<bool>, std::sync::Condvar)>);

impl Gate {
    fn wait(&self) {
        let (m, cv) = &*self.0;
        let mut open = m.lock().unwrap();
        while !*open {
            open = cv.wait(open).unwrap();
        }
    }
    fn release(&self) {
        let (m, cv) = &*self.0;
        *m.lock().unwrap() = true;
        cv.notify_all();
    }
}

struct FakeSurface {
    broker: Arc<RenderBroker>,
    script: Mutex<VecDeque<Behavior>>,
    default: Behavior,
    opens: AtomicUsize,
    closes: Arc<AtomicUsize>,
    urls: Mutex<Vec<String>>,
    labels: Mutex<Vec<String>>,
    /// 還沒真的消失的視窗 label（模擬 Tauri：同 label 的視窗還在時再建會失敗）。
    alive: Arc<Mutex<HashSet<String>>>,
    /// 下一次關窗等不到 Destroyed（視窗留著）。
    fail_next_close: Arc<AtomicBool>,
    /// 每次建立視窗依序配給的 browser 行程（空＝讀不到 PID）。
    browsers: Mutex<VecDeque<Arc<FakeBrowser>>>,
    /// 建立視窗與 browser 行程操作的先後（bug-leaked-renderer 測試核對順序）。
    events: Arc<Mutex<Vec<String>>>,
}

impl FakeSurface {
    fn new(broker: Arc<RenderBroker>, default: Behavior) -> Self {
        FakeSurface {
            broker,
            script: Mutex::new(VecDeque::new()),
            default,
            opens: AtomicUsize::new(0),
            closes: Arc::new(AtomicUsize::new(0)),
            urls: Mutex::new(Vec::new()),
            labels: Mutex::new(Vec::new()),
            alive: Arc::new(Mutex::new(HashSet::new())),
            fail_next_close: Arc::new(AtomicBool::new(false)),
            browsers: Mutex::new(VecDeque::new()),
            events: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn push(&self, b: Behavior) {
        self.script.lock().unwrap().push_back(b);
    }

    fn opens(&self) -> usize {
        self.opens.load(Ordering::SeqCst)
    }

    fn closes(&self) -> usize {
        self.closes.load(Ordering::SeqCst)
    }
}

/// 以 render id 為 `rid` 的那個視窗（label＝`renderer_label(rid)`）送出 PNG：render N 的頁面住在視窗 N。
fn deliver_png(broker: &RenderBroker, rid: &str, w: u32, h: u32, tag: u8) -> Delivery {
    let label = renderer_label(rid.parse().unwrap_or(0));
    deliver_png_from(broker, &label, rid, w, h, tag)
}

fn deliver_png_from(
    broker: &RenderBroker,
    label: &str,
    rid: &str,
    w: u32,
    h: u32,
    tag: u8,
) -> Delivery {
    broker.deliver_done(
        label,
        &InvokeBody::Raw(fake_png(w, h, tag)),
        Some(&meta_header(rid, w, h, &[])),
    )
}

struct FakeOpen {
    closes: Arc<AtomicUsize>,
    alive: Arc<Mutex<HashSet<String>>>,
    fail_next_close: Arc<AtomicBool>,
    label: String,
    browser: Option<Arc<dyn BrowserProcess>>,
}

impl OpenSurface for FakeOpen {
    fn close(self: Box<Self>, _timeout: Duration) -> Result<(), String> {
        self.closes.fetch_add(1, Ordering::SeqCst);
        if self.fail_next_close.swap(false, Ordering::SeqCst) {
            return Err("等不到 Destroyed（測試）".into());
        }
        self.alive.lock().unwrap().remove(&self.label);
        Ok(())
    }

    fn browser(&self) -> Option<Arc<dyn BrowserProcess>> {
        self.browser.clone()
    }
}

impl RenderSurface for FakeSurface {
    fn open(&self, label: &str, url: &str) -> Result<Box<dyn OpenSurface>, String> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        self.urls.lock().unwrap().push(url.to_owned());
        self.labels.lock().unwrap().push(label.to_owned());
        self.events.lock().unwrap().push(format!("open {label}"));
        if self.alive.lock().unwrap().contains(label) {
            return Err(format!("label {label} 的視窗已存在（測試）"));
        }
        let window = || -> Box<dyn OpenSurface> {
            self.alive.lock().unwrap().insert(label.to_owned());
            let browser = self
                .browsers
                .lock()
                .unwrap()
                .pop_front()
                .map(|b| b as Arc<dyn BrowserProcess>);
            Box::new(FakeOpen {
                closes: Arc::clone(&self.closes),
                alive: Arc::clone(&self.alive),
                fail_next_close: Arc::clone(&self.fail_next_close),
                label: label.to_owned(),
                browser,
            })
        };
        let behavior = self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| self.default.clone());
        let rid = rid_of(url);
        let rid_n: RenderId = rid.parse().expect("rid 是數字");
        let b = &self.broker;
        match behavior {
            Behavior::Png { w, h, tag } => {
                assert_eq!(
                    deliver_png_from(b, label, &rid, w, h, tag),
                    Delivery::Accepted
                );
            }
            Behavior::PngJsonArray { w, h, tag } => {
                let arr = fake_png(w, h, tag)
                    .into_iter()
                    .map(|x| json!(x))
                    .collect::<Vec<Value>>();
                b.deliver_done(
                    label,
                    &InvokeBody::Json(Value::Array(arr)),
                    Some(&meta_header(&rid, w, h, &["w 不合法"])),
                );
            }
            Behavior::BadBody => {
                b.deliver_done(
                    label,
                    &InvokeBody::Json(json!({ "not": "png" })),
                    Some(&meta_header(&rid, 1, 1, &[])),
                );
            }
            Behavior::PageFailed(msg) => {
                let meta = json!({ "rid": rid, "w": 0, "h": 0, "warnings": [] });
                b.deliver_failed(label, msg.to_owned(), &meta);
            }
            Behavior::Silent => {}
            Behavior::StaleThenPng {
                stale_rid,
                w,
                h,
                stale_tag,
                tag,
            } => {
                assert!(matches!(
                    deliver_png(b, &stale_rid, w, h, stale_tag),
                    Delivery::Stale { .. }
                ));
                assert_eq!(
                    deliver_png_from(b, label, &rid, w, h, tag),
                    Delivery::Accepted
                );
            }
            Behavior::OnlyStale { stale_rid, w, h } => {
                assert!(matches!(
                    deliver_png(b, &stale_rid, w, h, 9),
                    Delivery::Stale { .. }
                ));
            }
            Behavior::PngWithoutRid { w, h } => {
                let meta = utf8_percent_encode(&json!({ "w": w, "h": h }).to_string());
                let d = b.deliver_done(label, &InvokeBody::Raw(fake_png(w, h, 1)), Some(&meta));
                assert!(matches!(d, Delivery::Stale { .. }), "{d:?}");
            }
            Behavior::DelayedPng {
                delay_ms,
                w,
                h,
                tag,
            } => {
                let b = Arc::clone(b);
                let label = label.to_owned();
                thread::spawn(move || {
                    thread::sleep(Duration::from_millis(delay_ms));
                    deliver_png_from(&b, &label, &rid, w, h, tag);
                });
            }
            Behavior::OpenError => return Err("建立視窗失敗（測試）".into()),
            Behavior::BlockOpen(gate) => gate.wait(),
            Behavior::PanicOpen => panic!("建立渲染視窗時 panic（測試）"),
            Behavior::SlowOpenThenDelayedPng {
                open_ms,
                report_ms,
                w,
                h,
                tag,
            } => {
                thread::sleep(Duration::from_millis(open_ms));
                let b = Arc::clone(b);
                let label = label.to_owned();
                thread::spawn(move || {
                    thread::sleep(Duration::from_millis(report_ms));
                    deliver_png_from(&b, &label, &rid, w, h, tag);
                });
            }
        }
        assert_eq!(
            label,
            renderer_label(rid_n),
            "每次渲染的視窗 label 帶自己的 render id"
        );
        Ok(window())
    }
}

static DIR_SEQ: AtomicU64 = AtomicU64::new(0);

/// 每個測試自己的暫存輸出資料夾（不碰真正的 %LOCALAPPDATA%）。
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let n = DIR_SEQ.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!(
            "fc-host-render-test-{}-{tag}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn short_config() -> RenderConfig {
    RenderConfig {
        timeout: Duration::from_millis(150),
        close_timeout: Duration::from_millis(150),
        ..RenderConfig::default()
    }
}

fn renderer(default: Behavior) -> Renderer<FakeSurface> {
    renderer_with(default, short_config())
}

fn renderer_with(default: Behavior, config: RenderConfig) -> Renderer<FakeSurface> {
    let broker = Arc::new(RenderBroker::new());
    let surface = FakeSurface::new(Arc::clone(&broker), default);
    Renderer::new(surface, broker, config)
}

fn request(w: u32, h: u32) -> RenderRequest {
    RenderRequest {
        monitor_key: KEY.to_owned(),
        width: w,
        height: h,
        theme: WallpaperTheme::Astrolabe,
        // 2026-10-05T00:30:00Z（台北週一 08:30）
        as_of: 1_791_160_200,
        tz: "Asia/Taipei".to_owned(),
        fixture: None,
    }
}

fn target(dir: &Path) -> OutputTarget<'_> {
    OutputTarget {
        dir,
        displayed: None,
    }
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

// ── 網址與時間 ────────────────────────────────────────────────────────────────────────────

#[test]
fn iso_utc_formats_with_z() {
    assert_eq!(iso_utc(0), "1970-01-01T00:00:00Z");
    assert_eq!(iso_utc(1_791_160_200), "2026-10-05T00:30:00Z");
    assert_eq!(iso_utc(951_782_400 + 86_399), "2000-02-29T23:59:59Z");
}

#[test]
fn page_url_carries_size_time_tz_and_render_id() {
    let url = page_url(&request(3840, 2160), 7).unwrap();
    assert_eq!(
        url,
        "wallpapers/astrolabe.html?w=3840&h=2160&t=2026-10-05T00%3A30%3A00Z&tz=Asia%2FTaipei&rid=7"
    );
    let mut r = request(2560, 1600);
    r.theme = WallpaperTheme::Skyline;
    r.fixture = Some("../fixtures/tw-events.json".into());
    let url = page_url(&r, 12).unwrap();
    assert!(
        url.starts_with("wallpapers/skyline.html?w=2560&h=1600&"),
        "{url}"
    );
    assert!(
        url.ends_with("&rid=12&fixture=..%2Ffixtures%2Ftw-events.json"),
        "{url}"
    );
}

#[test]
fn page_url_without_tz_lets_the_page_use_the_system_time_zone() {
    // task 4.7a：正式排程不帶 tz（空字串），頁面以系統時區顯示（core.mjs parseQuery）。
    let mut r = request(3840, 2160);
    r.tz = String::new();
    assert_eq!(
        page_url(&r, 7).unwrap(),
        "wallpapers/astrolabe.html?w=3840&h=2160&t=2026-10-05T00%3A30%3A00Z&rid=7"
    );
}

#[test]
fn page_url_rejects_none_theme_and_bad_size() {
    let mut r = request(3840, 2160);
    r.theme = WallpaperTheme::None;
    assert!(page_url(&r, 1).is_err());
    assert!(page_url(&request(0, 2160), 1).is_err());
    assert!(page_url(&request(MIN_DIMENSION - 1, 2160), 1).is_err());
    assert!(page_url(&request(MIN_DIMENSION, MAX_DIMENSION), 1).is_ok());
    assert!(page_url(&request(3840, MAX_DIMENSION + 1), 1).is_err());
}

#[test]
fn monitor_key_format_is_enforced() {
    assert!(is_valid_monitor_key(KEY));
    for bad in [
        "",
        "m-",
        "m-0123",
        "x-0123456789abcdef",
        "m-0123456789ABCDEF",
        "m-0123456789abcdeg",
        "m-../../../x",
    ] {
        assert!(!is_valid_monitor_key(bad), "{bad}");
    }
}

// ── 本體與 meta ───────────────────────────────────────────────────────────────────────────

#[test]
fn decode_raw_and_json_array_bodies() {
    let png = fake_png(3840, 2160, 5);
    let (kind, bytes) = decode_png_body(&InvokeBody::Raw(png.clone())).unwrap();
    assert_eq!((kind, bytes.as_slice()), (BodyKind::Raw, png.as_slice()));
    let arr = Value::Array(png.iter().map(|b| json!(b)).collect());
    let (kind, bytes) = decode_png_body(&InvokeBody::Json(arr)).unwrap();
    assert_eq!((kind, bytes), (BodyKind::JsonArray, png));
    assert!(decode_png_body(&InvokeBody::Json(json!([1, 256]))).is_err());
    assert!(decode_png_body(&InvokeBody::Json(json!([1, -1]))).is_err());
    assert!(decode_png_body(&InvokeBody::Json(json!({ "a": 1 }))).is_err());
}

#[test]
fn png_dimensions_reads_ihdr_only_for_png() {
    assert_eq!(png_dimensions(&fake_png(3840, 2160, 0)), Some((3840, 2160)));
    assert_eq!(png_dimensions(b"GIF89a................"), None);
    assert_eq!(png_dimensions(&fake_png(1, 1, 0)[..20]), None);
}

#[test]
fn meta_header_round_trip_reads_rid_and_warnings() {
    let m = parse_meta_header(&meta_header("42", 10, 20, &["警告一"])).unwrap();
    assert_eq!(m.rid.as_deref(), Some("42"));
    assert_eq!((m.w, m.h), (Some(10), Some(20)));
    assert_eq!(m.warnings, vec!["警告一".to_owned()]);
    // rid 以數字送來也認得
    let m = parse_meta_value(&json!({ "rid": 42 }));
    assert_eq!(m.rid.as_deref(), Some("42"));
    assert!(parse_meta_header("%zz").is_err());
    assert!(parse_meta_header("not-json").is_err());
}

// ── broker：render id 配對 ────────────────────────────────────────────────────────────────

#[test]
fn broker_discards_mismatched_and_unexpected_reports() {
    let broker = RenderBroker::new();
    // 沒有等待中的渲染
    assert!(matches!(
        deliver_png(&broker, "1", 1, 1, 0),
        Delivery::Stale { waiting: None, .. }
    ));
    let ticket = broker.begin();
    let id = ticket.id();
    let other = (id + 1).to_string();
    assert!(matches!(
        deliver_png(&broker, &other, 1, 1, 0),
        Delivery::Stale { waiting: Some(w), .. } if w == id
    ));
    // 不是渲染視窗送來的一律拒收
    assert_eq!(
        broker.deliver_done(
            "w-macro",
            &InvokeBody::Raw(fake_png(1, 1, 0)),
            Some(&meta_header(&id.to_string(), 1, 1, &[]))
        ),
        Delivery::NotRenderer
    );
    assert!(
        ticket.wait(Duration::from_millis(30)).is_none(),
        "不符者都沒有送進來"
    );
    assert_eq!(
        deliver_png(&broker, &id.to_string(), 1, 1, 3),
        Delivery::Accepted
    );
    assert!(matches!(
        ticket.wait(Duration::from_millis(30)),
        Some(PageReport::Done { .. })
    ));
    // 同一個 id 第二次回報＝已經收過，丟棄
    assert!(matches!(
        deliver_png(&broker, &id.to_string(), 1, 1, 4),
        Delivery::Stale { .. }
    ));
    drop(ticket);
    assert!(matches!(
        deliver_png(&broker, &id.to_string(), 1, 1, 5),
        Delivery::Stale { waiting: None, .. }
    ));
    let next = broker.begin();
    assert!(next.id() > id, "id 遞增、不重用");
}

// ── 渲染：成功、a/b、殘檔 ─────────────────────────────────────────────────────────────────

#[test]
fn render_success_writes_slot_a_then_alternates() {
    let dir = TempDir::new("ab");
    let r = renderer(Behavior::Png {
        w: 3840,
        h: 2160,
        tag: 1,
    });
    let first = r.render(&request(3840, 2160), &target(dir.path())).unwrap();
    assert_eq!(first.slot, Slot::A);
    assert_eq!(first.path, dir.path().join(format!("{KEY}-a.png")));
    assert_eq!((first.width, first.height), (3840, 2160));
    assert_eq!(first.body, BodyKind::Raw);
    assert_eq!(fs::read(&first.path).unwrap(), fake_png(3840, 2160, 1));

    // 第二次：螢幕上是 a（displayed）→ 寫 b
    r.surface().push(Behavior::Png {
        w: 3840,
        h: 2160,
        tag: 2,
    });
    let second = r
        .render(
            &request(3840, 2160),
            &OutputTarget {
                dir: dir.path(),
                displayed: Some(&first.path),
            },
        )
        .unwrap();
    assert_eq!(second.slot, Slot::B);
    assert_eq!(
        fs::read(&first.path).unwrap(),
        fake_png(3840, 2160, 1),
        "a 不動"
    );
    assert_eq!(fs::read(&second.path).unwrap(), fake_png(3840, 2160, 2));

    // 第三次：螢幕上是 b → 寫回 a
    r.surface().push(Behavior::Png {
        w: 3840,
        h: 2160,
        tag: 3,
    });
    let third = r
        .render(
            &request(3840, 2160),
            &OutputTarget {
                dir: dir.path(),
                displayed: Some(&second.path),
            },
        )
        .unwrap();
    assert_eq!(third.slot, Slot::A);
    assert_eq!(
        names(dir.path()),
        vec![format!("{KEY}-a.png"), format!("{KEY}-b.png")]
    );
    assert_eq!(r.surface().opens(), 3);
    assert_eq!(r.surface().closes.load(Ordering::SeqCst), 3, "每次都關窗");
}

#[test]
fn choose_slot_without_displayed_uses_missing_then_older() {
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
    let t1 = t0 + Duration::from_secs(60);
    assert_eq!(choose_slot(None, None, None), Slot::A);
    assert_eq!(choose_slot(Some(t0), None, None), Slot::B);
    assert_eq!(choose_slot(None, Some(t0), None), Slot::A);
    assert_eq!(choose_slot(Some(t0), Some(t1), None), Slot::A, "a 較舊");
    assert_eq!(choose_slot(Some(t1), Some(t0), None), Slot::B, "b 較舊");
    // 螢幕上正顯示的那一格一律不寫
    assert_eq!(choose_slot(Some(t0), Some(t1), Some(Slot::A)), Slot::B);
    assert_eq!(choose_slot(None, None, Some(Slot::A)), Slot::B);
    assert_eq!(choose_slot(Some(t1), Some(t0), Some(Slot::B)), Slot::A);
}

#[test]
fn write_without_displayed_overwrites_older_file_by_mtime() {
    let dir = TempDir::new("mtime");
    let a = dir.path().join(format!("{KEY}-a.png"));
    let b = dir.path().join(format!("{KEY}-b.png"));
    fs::write(&a, fake_png(10, 10, 1)).unwrap();
    fs::write(&b, fake_png(10, 10, 2)).unwrap();
    let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    fs::File::options()
        .write(true)
        .open(&b)
        .unwrap()
        .set_modified(old)
        .unwrap();
    let w = write_output(dir.path(), KEY, &fake_png(10, 10, 3), (10, 10), None).unwrap();
    assert_eq!(w.slot, Slot::B, "b 的修改時間較舊");
    assert_eq!(fs::read(&a).unwrap(), fake_png(10, 10, 1));
    assert_eq!(fs::read(&b).unwrap(), fake_png(10, 10, 3));
}

#[test]
fn write_removes_own_residue_but_keeps_other_monitors_and_backups() {
    let dir = TempDir::new("residue");
    let p = dir.path();
    for n in [
        format!("{KEY}-a.png.tmp"),
        format!("{KEY}-b.png.tmp"),
        format!("{KEY}-c.png"),
        format!("{KEY}-old.png"),
        format!("{OTHER_KEY}-a.png"),
        format!("{OTHER_KEY}-b.png"),
        format!("{OTHER_KEY}-a.png.tmp"),
        "readme.txt".to_owned(),
    ] {
        fs::write(p.join(n), b"x").unwrap();
    }
    fs::create_dir_all(p.join("original")).unwrap();
    fs::write(p.join("original").join(format!("{KEY}.png")), b"backup").unwrap();

    let w = write_output(p, KEY, &fake_png(8, 8, 1), (8, 8), None).unwrap();
    assert_eq!(w.slot, Slot::A);
    let mut removed: Vec<String> = w
        .removed
        .iter()
        .map(|x| x.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    removed.sort();
    // a 的暫存檔被這次寫入本身覆寫並改名成 a，不算清除。
    assert_eq!(
        removed,
        vec![
            format!("{KEY}-b.png.tmp"),
            format!("{KEY}-c.png"),
            format!("{KEY}-old.png"),
        ]
    );
    assert_eq!(
        names(p),
        vec![
            format!("{KEY}-a.png"),
            format!("{OTHER_KEY}-a.png"),
            format!("{OTHER_KEY}-a.png.tmp"),
            format!("{OTHER_KEY}-b.png"),
            "original".to_owned(),
            "readme.txt".to_owned(),
        ]
    );
    assert_eq!(
        fs::read(p.join("original").join(format!("{KEY}.png"))).unwrap(),
        b"backup"
    );
}

#[test]
fn write_rejects_invalid_key_and_size_mismatch() {
    let dir = TempDir::new("badkey");
    assert!(write_output(dir.path(), "../evil", &fake_png(8, 8, 1), (8, 8), None).is_err());
    assert!(write_output(dir.path(), KEY, &fake_png(8, 9, 1), (8, 8), None).is_err());
    assert!(names(dir.path()).is_empty(), "驗證失敗什麼都不寫");
}

#[test]
fn write_failure_is_render_failure_and_keeps_old_image() {
    let dir = TempDir::new("writefail");
    // 讓輸出資料夾路徑其實是一個檔案 → 建資料夾失敗
    let blocker = dir.path().join("not-a-dir");
    fs::write(&blocker, b"file").unwrap();
    let r = renderer(Behavior::Png {
        w: 64,
        h: 32,
        tag: 1,
    });
    let err = r.render(&request(64, 32), &target(&blocker)).unwrap_err();
    assert!(
        matches!(err.reason, RenderFailureReason::WriteFailed(_)),
        "{err:?}"
    );
    assert_eq!(err.kind(), FailureKind::Render);
}

// ── 渲染：失敗分類（一律 FailureKind::Render，舊圖不動）──────────────────────────────────

fn seed_old_images(dir: &Path) -> (Vec<u8>, Vec<u8>) {
    let a = fake_png(3840, 2160, 0xA0);
    let b = fake_png(3840, 2160, 0xB0);
    fs::write(dir.join(format!("{KEY}-a.png")), &a).unwrap();
    fs::write(dir.join(format!("{KEY}-b.png")), &b).unwrap();
    (a, b)
}

fn assert_old_images_kept(dir: &Path, old: &(Vec<u8>, Vec<u8>)) {
    assert_eq!(fs::read(dir.join(format!("{KEY}-a.png"))).unwrap(), old.0);
    assert_eq!(fs::read(dir.join(format!("{KEY}-b.png"))).unwrap(), old.1);
    assert_eq!(names(dir).len(), 2);
}

#[test]
fn timeout_keeps_old_images_and_is_render_failure() {
    let dir = TempDir::new("timeout");
    let old = seed_old_images(dir.path());
    let r = renderer(Behavior::Silent);
    let started = std::time::Instant::now();
    let err = r
        .render(&request(3840, 2160), &target(dir.path()))
        .unwrap_err();
    assert!(started.elapsed() >= Duration::from_millis(150));
    assert!(
        matches!(err.reason, RenderFailureReason::Timeout(d) if d == Duration::from_millis(150)),
        "{err:?}"
    );
    assert_eq!(err.kind(), FailureKind::Render);
    assert_eq!(r.surface().closes.load(Ordering::SeqCst), 1, "逾時也關窗");
    assert_old_images_kept(dir.path(), &old);
}

#[test]
fn production_timeout_is_30_seconds() {
    assert_eq!(RENDER_TIMEOUT, Duration::from_secs(30));
    assert_eq!(RenderConfig::default().timeout, RENDER_TIMEOUT);
}

#[test]
fn late_png_after_timeout_is_not_counted_for_next_render() {
    let dir = TempDir::new("late");
    let r = renderer(Behavior::Silent);
    let err = r
        .render(&request(3840, 2160), &target(dir.path()))
        .unwrap_err();
    let stale_rid = err.render_id.expect("有 id").to_string();
    // 逾時之後才到：沒有人在等 → 丟棄
    assert!(matches!(
        deliver_png(r.broker(), &stale_rid, 3840, 2160, 0xEE),
        Delivery::Stale { .. }
    ));
    // 下一次渲染期間又晚到一次：仍丟棄，下一次只收自己的
    r.surface().push(Behavior::StaleThenPng {
        stale_rid,
        w: 3840,
        h: 2160,
        stale_tag: 0xEE,
        tag: 0x11,
    });
    let ok = r.render(&request(3840, 2160), &target(dir.path())).unwrap();
    assert_eq!(fs::read(&ok.path).unwrap(), fake_png(3840, 2160, 0x11));
}

#[test]
fn mismatched_render_id_only_times_out() {
    let dir = TempDir::new("mismatch");
    let old = seed_old_images(dir.path());
    let r = renderer(Behavior::OnlyStale {
        stale_rid: "999999".into(),
        w: 3840,
        h: 2160,
    });
    let err = r
        .render(&request(3840, 2160), &target(dir.path()))
        .unwrap_err();
    assert!(
        matches!(err.reason, RenderFailureReason::Timeout(_)),
        "{err:?}"
    );
    assert_old_images_kept(dir.path(), &old);
    // meta 沒帶 rid 也一樣丟棄
    r.surface()
        .push(Behavior::PngWithoutRid { w: 3840, h: 2160 });
    let err = r
        .render(&request(3840, 2160), &target(dir.path()))
        .unwrap_err();
    assert!(
        matches!(err.reason, RenderFailureReason::Timeout(_)),
        "{err:?}"
    );
    assert_old_images_kept(dir.path(), &old);
}

#[test]
fn page_failure_keeps_old_images() {
    let dir = TempDir::new("pagefail");
    let old = seed_old_images(dir.path());
    let r = renderer(Behavior::PageFailed("字型載入失敗"));
    let err = r
        .render(&request(3840, 2160), &target(dir.path()))
        .unwrap_err();
    assert!(
        matches!(&err.reason, RenderFailureReason::PageFailed(m) if m.contains("字型")),
        "{err:?}"
    );
    assert_eq!(err.kind(), FailureKind::Render);
    assert_old_images_kept(dir.path(), &old);
}

#[test]
fn size_mismatch_is_failure_and_keeps_old_images() {
    let dir = TempDir::new("size");
    let old = seed_old_images(dir.path());
    // 頁面把不合法的 w 改畫成預設尺寸（wallpaper.mjs「宿主接法」）→ 宿主核對不符
    let r = renderer(Behavior::Png {
        w: 1920,
        h: 1080,
        tag: 7,
    });
    let err = r
        .render(&request(3840, 2160), &target(dir.path()))
        .unwrap_err();
    assert!(
        matches!(
            err.reason,
            RenderFailureReason::SizeMismatch {
                expected: (3840, 2160),
                actual: Some((1920, 1080))
            }
        ),
        "{err:?}"
    );
    assert_old_images_kept(dir.path(), &old);
}

#[test]
fn json_array_body_is_accepted_and_bad_body_fails() {
    let dir = TempDir::new("json");
    let r = renderer(Behavior::PngJsonArray {
        w: 640,
        h: 360,
        tag: 4,
    });
    let ok = r.render(&request(640, 360), &target(dir.path())).unwrap();
    assert_eq!(ok.body, BodyKind::JsonArray);
    assert_eq!(
        ok.warnings,
        vec!["w 不合法".to_owned()],
        "警告照收、仍算成功"
    );
    assert_eq!(fs::read(&ok.path).unwrap(), fake_png(640, 360, 4));

    r.surface().push(Behavior::BadBody);
    let err = r
        .render(&request(640, 360), &target(dir.path()))
        .unwrap_err();
    assert!(
        matches!(err.reason, RenderFailureReason::BadBody(_)),
        "{err:?}"
    );
}

#[test]
fn delayed_report_from_another_thread_is_received() {
    let dir = TempDir::new("delayed");
    let r = renderer(Behavior::DelayedPng {
        delay_ms: 40,
        w: 100,
        h: 50,
        tag: 6,
    });
    let ok = r.render(&request(100, 50), &target(dir.path())).unwrap();
    assert_eq!(fs::read(&ok.path).unwrap(), fake_png(100, 50, 6));
    assert!(ok.timings.page >= Duration::from_millis(40));
}

#[test]
fn open_error_and_invalid_request_are_render_failures() {
    let dir = TempDir::new("open");
    let r = renderer(Behavior::OpenError);
    let err = r.render(&request(64, 64), &target(dir.path())).unwrap_err();
    assert!(
        matches!(err.reason, RenderFailureReason::OpenFailed(_)),
        "{err:?}"
    );
    assert_eq!(err.kind(), FailureKind::Render);

    let mut bad = request(64, 64);
    bad.monitor_key = "x".into();
    let err = r.render(&bad, &target(dir.path())).unwrap_err();
    assert!(
        matches!(err.reason, RenderFailureReason::InvalidRequest(_)),
        "{err:?}"
    );
    assert_eq!(r.surface().opens(), 1, "請求不合法時不開窗");
}

#[test]
fn render_refuses_forbidden_thread() {
    let dir = TempDir::new("thread");
    let r = renderer(Behavior::Png {
        w: 16,
        h: 16,
        tag: 1,
    })
    .forbid_thread(thread::current().id());
    let err = r.render(&request(16, 16), &target(dir.path())).unwrap_err();
    assert!(
        matches!(err.reason, RenderFailureReason::WrongThread),
        "{err:?}"
    );
    assert_eq!(r.surface().opens(), 0);
}

#[test]
fn concurrent_renders_are_serialized() {
    let dir = TempDir::new("serial");
    let r = Arc::new(renderer(Behavior::DelayedPng {
        delay_ms: 30,
        w: 16,
        h: 16,
        tag: 1,
    }));
    let handles: Vec<_> = (0..3)
        .map(|_| {
            let r = Arc::clone(&r);
            let d = dir.path().to_path_buf();
            thread::spawn(move || r.render(&request(16, 16), &target(&d)).is_ok())
        })
        .collect();
    for h in handles {
        assert!(
            h.join().unwrap(),
            "同一個 label 一次只開一個視窗，三次都成功"
        );
    }
    assert_eq!(r.surface().opens(), 3);
}

// ── 排程器＋假渲染器（4.7 呼叫端契約的模擬）──────────────────────────────────────────────

const TPE: FixedOffset = FixedOffset(8 * 3600);
const DEVICE: &str = r"\\?\DISPLAY#TEST#1&0&UID1#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";

fn monitors() -> Vec<MonitorSnapshot> {
    vec![MonitorSnapshot {
        device_path: DEVICE.to_owned(),
        geometry: MonitorGeometry {
            width: 64,
            height: 36,
            dpi: 96,
        },
    }]
}

/// 模擬 4.7 的一次喚醒：評估 → 若 Redraw 就逐台渲染並記錄 → 立即再評估取 next_wake。
/// 回傳（這次是否渲染、下一次喚醒時刻）。
fn wake(
    state: &mut SchedulerState,
    r: &Renderer<FakeSurface>,
    dir: &Path,
    theme: WallpaperTheme,
    now: i64,
) -> (bool, Option<i64>) {
    let pause: HashSet<PauseReason> = HashSet::new();
    let mons = monitors();
    let data = DataDates {
        twii_intraday: CivilDate::parse_iso("2026-10-02"),
        twii_daily: CivilDate::parse_iso("2026-10-02"),
        margin: None,
    };
    let input = |now| SchedulerInput {
        now,
        tz: &TPE,
        theme,
        pause: &pause,
        data,
        monitors: &mons,
        already_taken_over: false,
        explorer: ExplorerResources::NotSampled,
    };
    let decision = next_action(state, &input(now));
    state.apply_forget(&decision);
    let Action::Redraw(plan) = decision.action else {
        return (false, decision.next_wake);
    };
    for t in &plan.targets {
        let req = RenderRequest {
            monitor_key: KEY.to_owned(),
            width: t.monitor.geometry.width,
            height: t.monitor.geometry.height,
            theme: plan.theme,
            as_of: plan.as_of,
            tz: "Asia/Taipei".into(),
            fixture: None,
        };
        match r.render(&req, &target(dir)) {
            Ok(_) => assert!(state.record_drawn(&plan, &t.monitor)),
            Err(e) => assert!(state.record_failed(&plan, t, e.kind(), now)),
        }
    }
    let after = next_action(state, &input(now));
    assert!(
        !matches!(after.action, Action::Redraw(_)),
        "同一個 now 再評估不會再渲染"
    );
    (true, after.next_wake)
}

/// 台北 2026-10-05 的 hh:mm:ss（UTC 秒）。
fn tpe(h: i64, m: i64, s: i64) -> i64 {
    1_791_158_400 + h * 3600 + m * 60 + s - 8 * 3600
}

#[test]
fn astrolabe_renders_once_per_slot_and_never_between() {
    let dir = TempDir::new("slots");
    let r = renderer(Behavior::Png {
        w: 64,
        h: 36,
        tag: 1,
    });
    let mut state = SchedulerState::default();
    let mut renders_at = Vec::new();
    // 10:15:00 到 10:59:30，每 30 秒喚醒一次（模擬資料通道、顯示變更等各種事件）。
    let mut now = tpe(10, 15, 0);
    while now < tpe(11, 0, 0) {
        let (rendered, _) = wake(&mut state, &r, dir.path(), WallpaperTheme::Astrolabe, now);
        if rendered {
            renders_at.push(now);
        }
        now += 30;
    }
    assert_eq!(
        renders_at,
        vec![tpe(10, 15, 0), tpe(10, 30, 0), tpe(10, 45, 0)],
        "10:15 畫完到 10:30 之前沒有任何繪製"
    );
    assert_eq!(r.surface().opens(), 3, "渲染視窗只在時點開過三次");
}

#[test]
fn consecutive_render_failures_back_off_with_retry_delay() {
    let dir = TempDir::new("backoff");
    let old = seed_old_images(dir.path());
    let r = renderer(Behavior::PageFailed("draw 拋錯"));
    let mut state = SchedulerState::default();
    // 天際線只看資料日期、沒有時間觸發：重試時刻完全由退避決定。
    let mut now = tpe(9, 0, 0);
    let mut attempts = Vec::new();
    for _ in 0..7 {
        let (rendered, next) = wake(&mut state, &r, dir.path(), WallpaperTheme::Skyline, now);
        assert!(rendered);
        attempts.push(now);
        let next = next.expect("失敗後有重試時刻");
        // 重試時刻之前的喚醒一律不渲染
        for probe in [now + 1, (now + next) / 2, next - 1] {
            let before = r.surface().opens();
            let (again, _) = wake(&mut state, &r, dir.path(), WallpaperTheme::Skyline, probe);
            assert!(!again, "重試時刻之前不得渲染（{probe}）");
            assert_eq!(r.surface().opens(), before);
        }
        now = next;
    }
    let gaps: Vec<i64> = attempts.windows(2).map(|w| w[1] - w[0]).collect();
    let expected: Vec<i64> = (1..=6).map(retry_delay).collect();
    assert_eq!(
        gaps, expected,
        "沿用 wallpaper::retry_delay：2、4、8、16、30、30 分"
    );
    assert_eq!(expected[..5], [120, 240, 480, 960, 1800]);
    assert_old_images_kept(dir.path(), &old);
}

#[test]
fn timeout_failure_then_success_resets_backoff() {
    let dir = TempDir::new("recover");
    let r = renderer(Behavior::Png {
        w: 64,
        h: 36,
        tag: 1,
    });
    r.surface().push(Behavior::Silent);
    r.surface().push(Behavior::Silent);
    let mut state = SchedulerState::default();
    let t0 = tpe(9, 0, 0);
    let (_, n1) = wake(&mut state, &r, dir.path(), WallpaperTheme::Skyline, t0);
    assert_eq!(n1, Some(t0 + retry_delay(1)));
    let (_, n2) = wake(
        &mut state,
        &r,
        dir.path(),
        WallpaperTheme::Skyline,
        t0 + retry_delay(1),
    );
    assert_eq!(
        n2,
        Some(t0 + retry_delay(1) + retry_delay(2)),
        "連續逾時，間隔遞增"
    );
    let (_, n3) = wake(
        &mut state,
        &r,
        dir.path(),
        WallpaperTheme::Skyline,
        n2.unwrap(),
    );
    assert_eq!(n3, None, "成功後天際線只等資料，沒有重試");
    assert_eq!(names(dir.path()), vec![format!("{KEY}-a.png")]);
}

// ── 修正輪 1：建立視窗也受 30 秒限制、每次渲染一個 label、強制逾時頁 ─────────────────────

/// 等某個條件成立（最多 2 秒）。
fn eventually(mut cond: impl FnMut() -> bool) -> bool {
    let end = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < end {
        if cond() {
            return true;
        }
        thread::sleep(Duration::from_millis(5));
    }
    cond()
}

#[test]
fn stuck_window_creation_is_bounded_by_the_deadline_and_abandoned_window_is_closed() {
    let dir = TempDir::new("stuckopen");
    let old = seed_old_images(dir.path());
    let gate = Gate::default();
    let r = renderer(Behavior::Png {
        w: 3840,
        h: 2160,
        tag: 1,
    });
    r.surface().push(Behavior::BlockOpen(gate.clone()));
    let started = std::time::Instant::now();
    let err = r
        .render(&request(3840, 2160), &target(dir.path()))
        .unwrap_err();
    let elapsed = started.elapsed();
    assert!(
        matches!(err.reason, RenderFailureReason::Timeout(d) if d == Duration::from_millis(150)),
        "{err:?}"
    );
    assert_eq!(err.kind(), FailureKind::Render);
    assert!(
        elapsed < Duration::from_secs(1),
        "建立視窗卡住也在期限內返回：{elapsed:?}"
    );
    assert_eq!(
        err.window_closed, None,
        "視窗還沒建好，交給建立執行緒事後關閉"
    );
    assert_old_images_kept(dir.path(), &old);

    // 上一次的建立還沒返回：不再疊一條卡住的建立，直接失敗（不開窗）
    let err = r
        .render(&request(3840, 2160), &target(dir.path()))
        .unwrap_err();
    assert!(
        matches!(err.reason, RenderFailureReason::OpenFailed(_)),
        "{err:?}"
    );
    assert_eq!(r.surface().opens(), 1);

    // 卡住的建立終於返回：被放棄的視窗由建立執行緒關掉
    gate.release();
    assert!(
        eventually(|| r.surface().closes() == 1),
        "被放棄的視窗要關閉"
    );
    assert!(r.surface().alive.lock().unwrap().is_empty());
    let ok = r.render(&request(3840, 2160), &target(dir.path())).unwrap();
    assert_eq!(fs::read(&ok.path).unwrap(), fake_png(3840, 2160, 1));
}

/// 期限涵蓋「建立視窗＋頁面」的設定：1 秒期限，建立與頁面各自都在期限內、合計才超過。
fn deadline_config() -> RenderConfig {
    RenderConfig {
        timeout: Duration::from_millis(1000),
        close_timeout: Duration::from_millis(150),
        ..RenderConfig::default()
    }
}

#[test]
fn deadline_covers_open_plus_page() {
    // 建立本身（同步）花 700 ms、頁面在建立返回後 700 ms 才回報：兩段各自都在 1 秒內，合計 1.4 秒。
    // 期限涵蓋建立（正式行為）→ 頁面只剩約 300 ms → 逾時；期限若從建立返回才起算（舊行為）→ 頁面有
    // 整整 1 秒 → 成功。兩邊的餘裕都是數百毫秒（回報最早在 1.4 秒、期限在 1.0 秒；舊行為的期限在
    // 1.7 秒），不依賴幾十毫秒的實際時間差。
    let dir = TempDir::new("deadline");
    let r = renderer_with(
        Behavior::SlowOpenThenDelayedPng {
            open_ms: 700,
            report_ms: 700,
            w: 16,
            h: 16,
            tag: 1,
        },
        deadline_config(),
    );
    let err = r.render(&request(16, 16), &target(dir.path())).unwrap_err();
    assert!(
        matches!(err.reason, RenderFailureReason::Timeout(d) if d == Duration::from_millis(1000)),
        "期限要涵蓋建立視窗：{err:?}"
    );
    assert_eq!(
        err.window_closed,
        Some(true),
        "逾時後視窗已關閉並等到 Destroyed"
    );

    // 對照：合計 600 ms（建立 300＋頁面 300）在 1 秒內 → 成功（頁面拿到的是剩下的時間，不是零）。
    let r = renderer_with(
        Behavior::SlowOpenThenDelayedPng {
            open_ms: 300,
            report_ms: 300,
            w: 16,
            h: 16,
            tag: 1,
        },
        deadline_config(),
    );
    let ok = r.render(&request(16, 16), &target(dir.path())).unwrap();
    assert_eq!(fs::read(&ok.path).unwrap(), fake_png(16, 16, 1));
}

#[test]
fn panic_while_opening_fails_this_render_and_does_not_block_the_next() {
    // 建立視窗的呼叫 panic：這次立刻以建立失敗結束（不等滿期限），之後的渲染照常（不卡在
    // 「上一次建立還沒返回」）。
    let dir = TempDir::new("openpanic");
    let old = seed_old_images(dir.path());
    let r = renderer_with(
        Behavior::Png {
            w: 16,
            h: 16,
            tag: 1,
        },
        deadline_config(),
    );
    r.surface().push(Behavior::PanicOpen);
    let started = std::time::Instant::now();
    let err = r
        .render(&request(3840, 2160), &target(dir.path()))
        .unwrap_err();
    assert!(
        matches!(&err.reason, RenderFailureReason::OpenFailed(m) if m.contains("panic")),
        "{err:?}"
    );
    assert_eq!(err.kind(), FailureKind::Render);
    assert_eq!(err.window_closed, None);
    assert!(
        started.elapsed() < Duration::from_millis(900),
        "panic 立即回報，不等滿 1 秒期限：{:?}",
        started.elapsed()
    );
    assert_old_images_kept(dir.path(), &old);

    let ok = r.render(&request(16, 16), &target(dir.path())).unwrap();
    assert_eq!(fs::read(&ok.path).unwrap(), fake_png(16, 16, 1));
    assert_eq!(r.surface().opens(), 2);
}

/// 強制逾時頁一定要存在（不存在時 Tauri 會退回載入 `index.html`，語意隨那頁改變），而且不得碰任何
/// 回報管道（不 import 共用模組、不呼叫 Tauri）。
#[test]
fn never_reports_page_exists_and_cannot_report() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("ui")
        .join(NEVER_REPORTS_PAGE);
    let html = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("強制逾時頁 {} 必須存在：{e}", path.display()));
    for banned in [
        "import",
        "__TAURI",
        "invoke",
        "wallpaper_render",
        "wallpaper.mjs",
        "bridge.js",
        "<link",
        "src=",
    ] {
        assert!(!html.contains(banned), "強制逾時頁不得含 {banned:?}");
    }
    assert!(html.contains("self-test"), "檔案內要標明是 self-test 專用");
    assert!(
        NEVER_REPORTS_PAGE.starts_with("wallpapers/__self-test-"),
        "{NEVER_REPORTS_PAGE}"
    );
}

#[test]
fn each_render_uses_its_own_label_with_the_renderer_prefix() {
    let dir = TempDir::new("labels");
    let r = renderer(Behavior::Png {
        w: 16,
        h: 16,
        tag: 1,
    });
    let a = r.render(&request(16, 16), &target(dir.path())).unwrap();
    let b = r.render(&request(16, 16), &target(dir.path())).unwrap();
    let labels = r.surface().labels.lock().unwrap().clone();
    assert_eq!(
        labels,
        vec![renderer_label(a.render_id), renderer_label(b.render_id)]
    );
    assert_ne!(labels[0], labels[1]);
    for l in &labels {
        assert!(l.starts_with(RENDERER_LABEL_PREFIX), "{l}");
        assert!(is_renderer_label(l), "{l}");
    }
    assert_eq!(renderer_label(7), "wallpaper-renderer-7");
    assert!(!is_renderer_label("wallpaper-renderer"));
    assert!(!is_renderer_label("wallpaper-renderer-"));
    assert!(!is_renderer_label("wallpaper-renderer-x1"));
    assert!(!is_renderer_label("w-macro"));
}

#[test]
fn window_that_never_reaches_destroyed_does_not_block_the_next_render() {
    let dir = TempDir::new("lingering");
    let r = renderer(Behavior::Png {
        w: 16,
        h: 16,
        tag: 1,
    });
    r.surface().fail_next_close.store(true, Ordering::SeqCst);
    let first = r.render(&request(16, 16), &target(dir.path())).unwrap();
    assert!(
        r.surface()
            .alive
            .lock()
            .unwrap()
            .contains(&renderer_label(first.render_id)),
        "舊視窗還在"
    );
    // 固定 label 時這裡會因 label 已存在而失敗；每次一個 label 就不受影響
    let second = r.render(&request(16, 16), &target(dir.path())).unwrap();
    assert_ne!(first.render_id, second.render_id);
}

#[test]
fn lingering_renderer_windows_are_listed_except_the_current_one() {
    let labels = [
        "w-macro",
        "settings",
        "gatekeeper",
        "wallpaper-renderer-3",
        "wallpaper-renderer-9",
        "wallpaper-renderer",
    ];
    assert_eq!(
        lingering_renderer_labels(labels.iter().copied(), "wallpaper-renderer-9"),
        vec!["wallpaper-renderer-3".to_owned()]
    );
    assert!(lingering_renderer_labels(["w-macro"].into_iter(), "wallpaper-renderer-1").is_empty());
}

#[test]
fn broker_requires_the_label_of_the_waiting_render() {
    let broker = RenderBroker::new();
    let ticket = broker.begin();
    let id = ticket.id();
    let rid = id.to_string();
    // 舊視窗（別的 render id 的 label）帶著目前的 rid：丟棄
    assert!(matches!(
        deliver_png_from(&broker, &renderer_label(id + 100), &rid, 1, 1, 0),
        Delivery::Stale { .. }
    ));
    // 舊的固定 label、或不是渲染視窗：拒收
    assert_eq!(
        deliver_png_from(&broker, "wallpaper-renderer", &rid, 1, 1, 0),
        Delivery::NotRenderer
    );
    assert!(ticket.wait(Duration::from_millis(20)).is_none());
    assert_eq!(
        deliver_png_from(&broker, &renderer_label(id), &rid, 1, 1, 0),
        Delivery::Accepted
    );
}

#[test]
fn forced_timeout_page_times_out_and_closes_the_window() {
    let dir = TempDir::new("forced");
    let old = seed_old_images(dir.path());
    let r = renderer(Behavior::Silent);
    let err = r
        .render_custom_page(
            &request(3840, 2160),
            &target(dir.path()),
            NEVER_REPORTS_PAGE,
        )
        .unwrap_err();
    assert!(
        matches!(err.reason, RenderFailureReason::Timeout(_)),
        "{err:?}"
    );
    assert_eq!(err.window_closed, Some(true));
    let url = r.surface().urls.lock().unwrap()[0].clone();
    assert!(
        url.starts_with(&format!("{NEVER_REPORTS_PAGE}?w=3840&h=2160&")),
        "{url}"
    );
    assert!(r.surface().alive.lock().unwrap().is_empty());
    assert_old_images_kept(dir.path(), &old);
}

#[test]
fn page_url_for_custom_path_keeps_the_same_query() {
    let req = request(3840, 2160);
    let themed = page_url(&req, 5).unwrap();
    let custom = page_url_for("wallpapers/x.html", &req, 5).unwrap();
    assert_eq!(
        custom.split_once('?').unwrap().1,
        themed.split_once('?').unwrap().1
    );
    assert!(custom.starts_with("wallpapers/x.html?"));
}

// ── bug-leaked-renderer：前一個渲染 browser 行程 ───────────────────────────────────────────

/// 假的 browser 行程＋假時鐘：`exits_after`＝關窗後多久才結束（`None`＝卡在結束流程，被結束才退出）；
/// `wait_exit(timeout)` 不真的睡，`exits_after <= timeout` 就算等到。每個操作都記進共用的 `events`。
/// `terminate` 與真實的 `TerminateProcess` 一樣是非同步的（修正輪 2 審查 N1 實測）：結束後立刻
/// `wait_exit(0)` 仍回 false，給大於 0 的等待才回 true。
struct FakeBrowser {
    pid: u32,
    exits_after: Option<Duration>,
    owned: Result<(), String>,
    terminate_fails: bool,
    terminated: AtomicBool,
    events: Arc<Mutex<Vec<String>>>,
    /// `wait_exit` 真的睡（等到結束的那一刻或滿上限），用來量「等待是否算進渲染期限」。
    sleeps: AtomicBool,
}

impl FakeBrowser {
    fn new(pid: u32, exits_after: Option<Duration>, events: &Arc<Mutex<Vec<String>>>) -> Arc<Self> {
        Self::with(pid, exits_after, Ok(()), false, events)
    }

    fn with(
        pid: u32,
        exits_after: Option<Duration>,
        owned: Result<(), String>,
        terminate_fails: bool,
        events: &Arc<Mutex<Vec<String>>>,
    ) -> Arc<Self> {
        Arc::new(FakeBrowser {
            pid,
            exits_after,
            owned,
            terminate_fails,
            terminated: AtomicBool::new(false),
            events: Arc::clone(events),
            sleeps: AtomicBool::new(false),
        })
    }

    fn terminated(&self) -> bool {
        self.terminated.load(Ordering::SeqCst)
    }

    fn log(&self, e: String) {
        self.events.lock().unwrap().push(e);
    }
}

impl BrowserProcess for FakeBrowser {
    fn pid(&self) -> u32 {
        self.pid
    }

    fn wait_exit(&self, timeout: Duration) -> bool {
        self.log(format!("wait {} {}ms", self.pid, timeout.as_millis()));
        let exited = (self.terminated() && !timeout.is_zero())
            || self.exits_after.is_some_and(|d| d <= timeout);
        if self.sleeps.load(Ordering::SeqCst) && !self.terminated() {
            thread::sleep(match self.exits_after {
                Some(d) if exited => d,
                _ => timeout,
            });
        }
        exited
    }

    fn check_owned(&self) -> Result<(), String> {
        self.log(format!("check {}", self.pid));
        self.owned.clone()
    }

    fn terminate(&self) -> Result<(), String> {
        self.log(format!("terminate {}", self.pid));
        if self.terminate_fails {
            return Err("拒絕存取（測試）".into());
        }
        self.terminated.store(true, Ordering::SeqCst);
        Ok(())
    }
}

fn png_renderer() -> Renderer<FakeSurface> {
    renderer(Behavior::Png {
        w: 3840,
        h: 2160,
        tag: 1,
    })
}

fn events(r: &Renderer<FakeSurface>) -> Vec<String> {
    r.surface().events.lock().unwrap().clone()
}

fn give_browser(r: &Renderer<FakeSurface>, b: &Arc<FakeBrowser>) {
    r.surface()
        .browsers
        .lock()
        .unwrap()
        .push_back(Arc::clone(b));
}

fn remember(r: &Renderer<FakeSurface>, b: &Arc<FakeBrowser>) {
    store_browser(
        &r.last_browser,
        Some(Arc::clone(b) as Arc<dyn BrowserProcess>),
    );
}

#[test]
fn next_render_waits_for_previous_browser_before_opening() {
    let dir = TempDir::new("browser-wait");
    let r = png_renderer();
    let ev = Arc::clone(&r.surface().events);
    // 第一個 browser 關窗後 300 ms 才結束（＜5 秒上限）：下一次要先等它、不結束它。
    let b1 = FakeBrowser::new(101, Some(Duration::from_millis(300)), &ev);
    let b2 = FakeBrowser::new(102, Some(Duration::ZERO), &ev);
    give_browser(&r, &b1);
    give_browser(&r, &b2);
    let first = r.render(&request(3840, 2160), &target(dir.path())).unwrap();
    assert_eq!(first.browser_pid, Some(101));
    r.render(&request(3840, 2160), &target(dir.path())).unwrap();
    assert_eq!(
        events(&r),
        vec![
            "open wallpaper-renderer-1".to_owned(),
            format!("wait 101 {}ms", BROWSER_EXIT_WAIT.as_millis()),
            "open wallpaper-renderer-2".to_owned(),
        ],
        "第一次沒有前一個可等；第二次在建立視窗之前先等第一個 browser 結束"
    );
    assert!(!b1.terminated());
}

#[test]
fn previous_browser_already_exited_starts_immediately() {
    let r = png_renderer();
    let ev = Arc::clone(&r.surface().events);
    let b = FakeBrowser::new(201, Some(Duration::ZERO), &ev);
    remember(&r, &b);
    match r.settle_previous_browser() {
        BrowserSettle::Exited { pid: 201, .. } => {}
        other => panic!("{other:?}"),
    }
    assert!(!b.terminated());
    assert_eq!(
        events(&r),
        vec![format!("wait 201 {}ms", BROWSER_EXIT_WAIT.as_millis())]
    );
    assert_eq!(
        r.settle_previous_browser(),
        BrowserSettle::None,
        "處置過就清掉"
    );
}

#[test]
fn stuck_previous_browser_is_terminated_after_limit_then_render_proceeds() {
    let dir = TempDir::new("browser-stuck");
    let r = png_renderer();
    let ev = Arc::clone(&r.surface().events);
    let stuck = FakeBrowser::new(301, None, &ev);
    give_browser(&r, &stuck);
    r.render(&request(3840, 2160), &target(dir.path())).unwrap();
    let second = r.render(&request(3840, 2160), &target(dir.path()));
    assert!(second.is_ok(), "{second:?}");
    assert!(stuck.terminated());
    assert_eq!(
        events(&r),
        vec![
            "open wallpaper-renderer-1".to_owned(),
            format!("wait 301 {}ms", BROWSER_EXIT_WAIT.as_millis()),
            "check 301".to_owned(),
            "terminate 301".to_owned(),
            format!("wait 301 {}ms", BROWSER_KILL_WAIT.as_millis()),
            "open wallpaper-renderer-2".to_owned(),
        ]
    );
}

#[test]
fn stuck_browser_failing_ownership_check_is_left_alone() {
    let r = png_renderer();
    let ev = Arc::clone(&r.surface().events);
    let other = FakeBrowser::with(401, None, Err("父行程不是宿主（測試）".into()), false, &ev);
    remember(&r, &other);
    assert_eq!(
        r.settle_previous_browser(),
        BrowserSettle::Left { pid: 401 }
    );
    assert!(!other.terminated());
    assert!(!events(&r).iter().any(|e| e.starts_with("terminate")));
}

#[test]
fn stuck_browser_whose_terminate_fails_is_reported_left() {
    let r = png_renderer();
    let ev = Arc::clone(&r.surface().events);
    let b = FakeBrowser::with(501, None, Ok(()), true, &ev);
    remember(&r, &b);
    assert_eq!(
        r.settle_previous_browser(),
        BrowserSettle::Left { pid: 501 }
    );
    assert!(!b.terminated());
}

#[test]
fn terminated_browser_reports_gone() {
    let r = png_renderer();
    let ev = Arc::clone(&r.surface().events);
    let b = FakeBrowser::new(601, None, &ev);
    remember(&r, &b);
    assert_eq!(
        r.settle_previous_browser(),
        BrowserSettle::Terminated {
            pid: 601,
            gone: true
        }
    );
    assert!(b.terminated());
}

#[test]
fn shutdown_settles_the_last_render_browser() {
    let dir = TempDir::new("browser-shutdown");
    let r = png_renderer();
    let ev = Arc::clone(&r.surface().events);
    let stuck = FakeBrowser::new(701, None, &ev);
    give_browser(&r, &stuck);
    r.render(&request(3840, 2160), &target(dir.path())).unwrap();
    assert_eq!(
        r.shutdown(),
        BrowserSettle::Terminated {
            pid: 701,
            gone: true
        }
    );
    assert!(stuck.terminated());
    assert_eq!(r.shutdown(), BrowserSettle::None);
}

/// 「因更新結束」的處置（D4 步驟 6）：卡住的 browser 在分到的時限內就被核對並結束，等待加確認的總和不超過 `limit`
/// （修正前沿用 `shutdown`，固定等 5 秒再結束、再等 2 秒，在 5 秒的步驟預算內根本走不到結束）。
#[test]
fn shutdown_within_terminates_a_stuck_browser_inside_the_limit() {
    let dir = TempDir::new("browser-shutdown-within");
    let r = png_renderer();
    let ev = Arc::clone(&r.surface().events);
    let stuck = FakeBrowser::new(801, None, &ev);
    give_browser(&r, &stuck);
    r.render(&request(3840, 2160), &target(dir.path())).unwrap();
    let limit = Duration::from_secs(5);
    assert_eq!(
        r.shutdown_within(limit),
        BrowserSettle::Terminated {
            pid: 801,
            gone: true
        }
    );
    assert!(stuck.terminated());
    let waits: Vec<u128> = events(&r)
        .iter()
        .filter_map(|e| e.strip_prefix("wait 801 "))
        .map(|rest| rest.trim_end_matches("ms").parse().unwrap())
        .collect();
    assert_eq!(waits.len(), 2, "等待＋結束後確認：{:?}", events(&r));
    assert!(
        waits.iter().sum::<u128>() <= limit.as_millis(),
        "兩段等待加總不得超過 limit：{waits:?}"
    );
    assert_eq!(
        r.shutdown_within(limit),
        BrowserSettle::None,
        "處置過就清掉"
    );
}

#[test]
fn shutdown_limit_split_never_exceeds_the_limit() {
    for ms in [0u64, 1, 30, 300, 900, 1500, 3000, 5000, 20_000] {
        let limit = Duration::from_millis(ms);
        let (wait, kill) = split_shutdown_limit(limit);
        assert_eq!(wait + kill, limit, "{ms} ms");
        assert!(kill <= BROWSER_KILL_WAIT);
    }
    assert_eq!(
        split_shutdown_limit(Duration::from_secs(3)),
        (Duration::from_secs(2), Duration::from_secs(1))
    );
}

#[test]
fn verify_renderer_browser_accepts_only_the_host_child_browser_of_the_renderer_dir() {
    let dir = Path::new(r"C:\Users\u\AppData\Local\tw.fintools.fc-host\wallpaper-renderer");
    let browser = r#""C:\Program Files (x86)\Microsoft\EdgeWebView\Application\154.0.4258.53\msedgewebview2.exe" --embedded-browser-webview=1 --webview-exe-name=fc-host.exe --user-data-dir="C:\Users\u\AppData\Local\tw.fintools.fc-host\wallpaper-renderer\EBWebView" --noerrdialogs"#;
    assert_eq!(verify_renderer_browser(Some(42), browser, 42, dir), Ok(()));
    // 大小寫不同仍是同一個資料夾（Windows 路徑）。
    let upper = browser.replace("wallpaper-renderer", "WALLPAPER-RENDERER");
    assert_eq!(verify_renderer_browser(Some(42), &upper, 42, dir), Ok(()));
    // 不帶引號的寫法（crashpad 等子行程的命令列就是這種）。
    let bare = r"msedgewebview2.exe --user-data-dir=C:\Users\u\AppData\Local\tw.fintools.fc-host\wallpaper-renderer\EBWebView --noerrdialogs";
    assert_eq!(verify_renderer_browser(Some(42), bare, 42, dir), Ok(()));

    assert!(
        verify_renderer_browser(Some(7), browser, 42, dir).is_err(),
        "父行程不是宿主"
    );
    assert!(
        verify_renderer_browser(None, browser, 42, dir).is_err(),
        "父行程不明"
    );
    let crashpad = format!("{bare} --type=crashpad-handler");
    assert!(
        verify_renderer_browser(Some(42), &crashpad, 42, dir).is_err(),
        "不是 browser 行程"
    );
    let widgets = browser.replace(r"\wallpaper-renderer\EBWebView", r"\EBWebView");
    assert!(
        verify_renderer_browser(Some(42), &widgets, 42, dir).is_err(),
        "小工具那組"
    );
    assert!(
        verify_renderer_browser(Some(42), "msedgewebview2.exe --noerrdialogs", 42, dir).is_err()
    );
}

#[test]
fn user_data_dir_arg_reads_quoted_and_bare_values() {
    assert_eq!(
        user_data_dir_arg(r#"x.exe --a --user-data-dir="C:\a b\EBWebView" --c"#),
        Some(r"C:\a b\EBWebView".to_owned())
    );
    assert_eq!(
        user_data_dir_arg(r"x.exe --user-data-dir=C:\ab\EBWebView --c"),
        Some(r"C:\ab\EBWebView".to_owned())
    );
    assert_eq!(user_data_dir_arg("x.exe --c"), None);
    assert_eq!(
        user_data_dir_arg(r#"x.exe --user-data-dir="unterminated"#),
        None
    );
}

// ── 修正輪 1（審查 L1／L2／L3）─────────────────────────────────────────────────────────────

#[test]
fn settling_previous_browser_is_not_counted_in_the_render_deadline() {
    // 審查 L1：渲染期限 150 ms；前一個 browser 卡住，處置要真的等 300 ms（再結束它）。頁面在開窗後 50 ms
    // 才回報。處置在期限起算之前（正式行為）→ 成功、建立＋頁面遠低於 300 ms；處置若算進期限 → 期限在等待
    // 中就用完 → 逾時。
    let dir = TempDir::new("settle-deadline");
    let r = renderer_with(
        Behavior::Png {
            w: 3840,
            h: 2160,
            tag: 1,
        },
        RenderConfig {
            timeout: Duration::from_millis(150),
            close_timeout: Duration::from_millis(150),
            browser_exit_wait: Duration::from_millis(300),
            browser_kill_wait: Duration::from_millis(50),
        },
    );
    let ev = Arc::clone(&r.surface().events);
    let stuck = FakeBrowser::new(801, None, &ev);
    stuck.sleeps.store(true, Ordering::SeqCst);
    give_browser(&r, &stuck);
    r.render(&request(3840, 2160), &target(dir.path())).unwrap();
    r.surface().push(Behavior::DelayedPng {
        delay_ms: 50,
        w: 3840,
        h: 2160,
        tag: 2,
    });
    let started = std::time::Instant::now();
    let second = r.render(&request(3840, 2160), &target(dir.path()));
    let elapsed = started.elapsed();
    let ok = second.expect("處置前一個 browser 的等待不能吃掉渲染期限");
    assert!(stuck.terminated());
    assert!(
        elapsed >= Duration::from_millis(300),
        "確實等了前一個 browser：{elapsed:?}"
    );
    assert!(
        ok.timings.to_png() < Duration::from_millis(300),
        "建立＋頁面不含處置的等待：{:?}",
        ok.timings
    );
}

#[test]
fn browser_of_a_window_created_after_the_deadline_is_settled_before_the_next_render() {
    // 審查 L2：建立視窗卡住、期限後才建好的那個視窗由建立執行緒關掉；它的 browser 行程（這裡卡住）也要在
    // 下一次建立視窗之前被處置。
    let dir = TempDir::new("late-browser");
    let gate = Gate::default();
    let r = png_renderer();
    let ev = Arc::clone(&r.surface().events);
    let late = FakeBrowser::new(901, None, &ev);
    give_browser(&r, &late);
    r.surface().push(Behavior::BlockOpen(gate.clone()));
    let err = r
        .render(&request(3840, 2160), &target(dir.path()))
        .unwrap_err();
    assert!(
        matches!(err.reason, RenderFailureReason::Timeout(_)),
        "{err:?}"
    );
    gate.release();
    assert!(
        eventually(|| r.surface().closes() == 1),
        "被放棄的視窗要關閉"
    );
    assert!(eventually(|| !r.open_in_flight.load(Ordering::SeqCst)));
    r.render(&request(3840, 2160), &target(dir.path())).unwrap();
    assert!(late.terminated());
    assert_eq!(
        events(&r),
        vec![
            "open wallpaper-renderer-1".to_owned(),
            format!("wait 901 {}ms", BROWSER_EXIT_WAIT.as_millis()),
            "check 901".to_owned(),
            "terminate 901".to_owned(),
            format!("wait 901 {}ms", BROWSER_KILL_WAIT.as_millis()),
            "open wallpaper-renderer-2".to_owned(),
        ]
    );
}

#[test]
fn event_loop_exit_terminates_a_live_previous_browser_without_waiting() {
    // 審查 L3：事件迴圈結束時不等待——仍存活就核對、結束，不等它退出。
    // 修正輪 2（N1）：結束是非同步的、又沒有等，結果是「已要求結束、是否退出未知」
    // （`TerminateRequested`），不是「結束後仍未退出」（`Terminated { gone: false }`）。
    let r = png_renderer();
    let ev = Arc::clone(&r.surface().events);
    let stuck = FakeBrowser::new(1001, None, &ev);
    remember(&r, &stuck);
    assert_eq!(
        r.settle_on_exit(false),
        Some(BrowserSettle::TerminateRequested { pid: 1001 })
    );
    assert!(stuck.terminated());
    assert!(
        !stuck.wait_exit(Duration::ZERO),
        "前提：與真實 TerminateProcess 一樣非同步，結束後立刻看仍未退出"
    );
    ev.lock().unwrap().pop();
    assert_eq!(
        events(&r),
        vec![
            "wait 1001 0ms".to_owned(),
            "check 1001".to_owned(),
            "terminate 1001".to_owned(),
        ],
        "等待都是 0，結束後不再確認"
    );
    assert_eq!(r.settle_on_exit(false), Some(BrowserSettle::None));
}

#[test]
fn event_loop_exit_leaves_exited_or_foreign_browsers_alone() {
    let r = png_renderer();
    let ev = Arc::clone(&r.surface().events);
    let gone = FakeBrowser::new(1101, Some(Duration::ZERO), &ev);
    remember(&r, &gone);
    assert!(matches!(
        r.settle_on_exit(false),
        Some(BrowserSettle::Exited { pid: 1101, .. })
    ));
    let foreign = FakeBrowser::with(1102, None, Err("父行程不是宿主（測試）".into()), false, &ev);
    remember(&r, &foreign);
    assert_eq!(
        r.settle_on_exit(false),
        Some(BrowserSettle::Left { pid: 1102 })
    );
    assert!(!foreign.terminated());
}

#[test]
fn event_loop_exit_during_session_end_leaves_the_browser_to_the_system() {
    let r = png_renderer();
    let ev = Arc::clone(&r.surface().events);
    let stuck = FakeBrowser::new(1201, None, &ev);
    remember(&r, &stuck);
    assert_eq!(r.settle_on_exit(true), None);
    assert!(events(&r).is_empty(), "工作階段結束中：完全不碰它");
    assert!(!stuck.terminated());
    assert!(r.last_browser().is_some(), "記錄保留（交給系統）");
}
