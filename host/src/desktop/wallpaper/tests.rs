//! 桌布 COM 執行緒的單元測試：一律用假後端，**不**呼叫真的 `IDesktopWallpaper`、不改桌布。
//! 真實 COM 後端的唯讀驗收見檔尾 `#[ignore]` 測試（`cargo test` 不跑）。登錄的測試在
//! `registry.rs`（假登錄，不寫真的 HKCU）。

use super::*;

use std::sync::mpsc::{Receiver, Sender};

// ---------------------------------------------------------------------------------------------
// 測試輔助：攔截記錄、假後端
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct CaptureLog {
    warns: Mutex<Vec<String>>,
    infos: Mutex<Vec<String>>,
}

impl CaptureLog {
    fn warns(&self) -> Vec<String> {
        self.warns.lock().unwrap().clone()
    }
    fn infos(&self) -> Vec<String> {
        self.infos.lock().unwrap().clone()
    }
}

impl WallpaperLog for CaptureLog {
    fn warn(&self, msg: &str) {
        self.warns.lock().unwrap().push(msg.to_owned());
    }
    fn info(&self, msg: &str) {
        self.infos.lock().unwrap().push(msg.to_owned());
    }
}

/// 假後端與測試共享的可觀察／可控制狀態。
#[derive(Default)]
struct Shared {
    /// 後端實際收到的請求（依序）。
    calls: Vec<&'static str>,
    /// `recreate` 被呼叫的次數。
    recreates: u32,
    /// `explorer_pid` 回傳值。
    pid: Option<u32>,
    /// 下一個請求要回的錯誤（取用一次）。
    fail_next: Option<BackendError>,
    /// 下一個請求要 panic（取用一次）。
    panic_next: bool,
    /// 每個請求的人為延遲。
    delay: Duration,
    /// `list_monitors` 的回傳值。
    monitors: Vec<MonitorEntry>,
}

struct FakeBackend {
    shared: Arc<Mutex<Shared>>,
    ready: bool,
    /// `Some` 時，`list_monitors` 開始後先通知 `entered`，再等 `hold` 收到一則訊息（或斷線）
    /// 才返回＝可控的「卡住的 explorer」。
    hold: Option<Receiver<()>>,
    entered: Option<Sender<()>>,
}

impl FakeBackend {
    fn begin(&mut self, name: &'static str) -> Result<(), BackendError> {
        let (delay, fail, panic) = {
            let mut s = self.shared.lock().unwrap();
            s.calls.push(name);
            (
                s.delay,
                s.fail_next.take(),
                std::mem::take(&mut s.panic_next),
            )
        };
        if panic {
            panic!("假後端依指示 panic（{name}）");
        }
        if !delay.is_zero() {
            thread::sleep(delay);
        }
        match fail {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

fn sample_snapshot() -> WallpaperSnapshot {
    WallpaperSnapshot {
        monitors: vec![MonitorWallpaper {
            monitor: monitor("\\\\?\\DISPLAY#A", true),
            wallpaper: Some("C:\\a.jpg".to_owned()),
        }],
        position: WallpaperPosition::FILL,
        background_color: 0,
        slideshow_status: SLIDESHOW_STATE_ENABLED,
        slideshow: None,
    }
}

impl WallpaperBackend for FakeBackend {
    fn is_ready(&self) -> bool {
        self.ready
    }
    fn recreate(&mut self) -> Result<(), BackendError> {
        self.shared.lock().unwrap().recreates += 1;
        self.ready = true;
        Ok(())
    }
    fn explorer_pid(&self) -> Option<u32> {
        self.shared.lock().unwrap().pid
    }
    fn list_monitors(&mut self) -> Result<Vec<MonitorEntry>, BackendError> {
        self.begin("list_monitors")?;
        if let Some(hold) = &self.hold {
            if let Some(entered) = &self.entered {
                let _ = entered.send(());
            }
            let _ = hold.recv();
        }
        Ok(self.shared.lock().unwrap().monitors.clone())
    }
    fn read(&mut self) -> Result<WallpaperSnapshot, BackendError> {
        self.begin("read")?;
        Ok(sample_snapshot())
    }
    fn set_wallpaper(&mut self, _device_path: &str, _image: &Path) -> Result<(), BackendError> {
        self.begin("set_wallpaper")
    }
    fn set_wallpaper_all(&mut self, _image: &Path) -> Result<(), BackendError> {
        self.begin("set_wallpaper_all")
    }
    fn set_solid_color(&mut self) -> Result<(), BackendError> {
        self.begin("set_solid_color")
    }
    fn set_position(&mut self, _position: WallpaperPosition) -> Result<(), BackendError> {
        self.begin("set_position")
    }
    fn set_background_color(&mut self, _colorref: u32) -> Result<(), BackendError> {
        self.begin("set_background_color")
    }
    fn restore_slideshow(&mut self, _slideshow: &SlideshowInfo) -> Result<(), BackendError> {
        self.begin("restore_slideshow")
    }
}

struct Harness {
    service: WallpaperService,
    shared: Arc<Mutex<Shared>>,
    log: Arc<CaptureLog>,
    /// 放行卡住的 `list_monitors`（送一則訊息放行一次）。
    release: Option<Sender<()>>,
    /// `list_monitors` 開始執行的通知。
    entered: Option<Receiver<()>>,
}

impl Harness {
    fn calls(&self) -> Vec<&'static str> {
        self.shared.lock().unwrap().calls.clone()
    }
    fn recreates(&self) -> u32 {
        self.shared.lock().unwrap().recreates
    }
    fn wait_entered(&self) {
        self.entered
            .as_ref()
            .expect("需要以 blocking 建立")
            .recv_timeout(Duration::from_secs(5))
            .expect("list_monitors 應在 5 秒內開始執行");
    }
    fn release_once(&self) {
        self.release
            .as_ref()
            .expect("需要以 blocking 建立")
            .send(())
            .expect("工作執行緒應仍在等待放行");
    }
    fn wait_not_busy(&self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.service.is_busy() {
            assert!(Instant::now() < deadline, "5 秒內應解除忙碌");
            thread::sleep(Duration::from_millis(5));
        }
    }
}

fn harness(timeout: Duration, slow_threshold: Duration, blocking: bool) -> Harness {
    let shared = Arc::new(Mutex::new(Shared::default()));
    let log = Arc::new(CaptureLog::default());
    let (release, hold) = if blocking {
        let (tx, rx) = mpsc::channel();
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };
    let (entered_tx, entered_rx) = if blocking {
        let (tx, rx) = mpsc::channel();
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };
    let config = WallpaperServiceConfig {
        timeout,
        slow_threshold,
        log: log.clone(),
    };
    let backend_shared = shared.clone();
    let service = WallpaperService::spawn_with(config, move || FakeBackend {
        shared: backend_shared,
        ready: false,
        hold,
        entered: entered_tx,
    })
    .expect("應能建立工作執行緒");
    Harness {
        service,
        shared,
        log,
        release,
        entered: entered_rx,
    }
}

fn fast() -> Harness {
    harness(Duration::from_secs(5), Duration::from_secs(5), false)
}

fn monitor(path: &str, online: bool) -> MonitorEntry {
    MonitorEntry {
        device_path: path.to_owned(),
        rect: online.then_some(PhysicalRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        }),
    }
}

// ---------------------------------------------------------------------------------------------
// 正常請求
// ---------------------------------------------------------------------------------------------

#[test]
fn normal_requests_return_backend_results() {
    let h = fast();
    h.shared.lock().unwrap().monitors = vec![
        monitor("\\\\?\\DISPLAY#A", true),
        monitor("\\\\?\\DISPLAY#B", false),
    ];
    let mons = h.service.list_monitors().expect("列舉應成功");
    assert_eq!(mons.len(), 2);
    assert!(mons[0].is_online());
    assert!(!mons[1].is_online(), "S_FALSE 的螢幕應標為離線");
    let snap = h.service.read().expect("讀取應成功");
    assert_eq!(snap, sample_snapshot());
    h.service
        .set_wallpaper("\\\\?\\DISPLAY#A", Path::new("C:\\x-a.png"))
        .expect("設定應成功");
    assert_eq!(h.calls(), vec!["list_monitors", "read", "set_wallpaper"]);
    assert!(!h.service.is_busy());
    assert!(
        h.log.warns().is_empty(),
        "快速請求不應記警告：{:?}",
        h.log.warns()
    );
}

#[test]
fn backend_error_is_returned_with_request_kind() {
    let h = fast();
    h.shared.lock().unwrap().fail_next = Some(BackendError::Invalid("路徑無效".to_owned()));
    let err = h.service.set_position(WallpaperPosition::FILL).unwrap_err();
    assert_eq!(
        err,
        WallpaperError::Backend {
            request: RequestKind::SetPosition,
            error: BackendError::Invalid("路徑無效".to_owned()),
        }
    );
    assert!(!h.service.is_busy(), "後端失敗不等於逾時，不應忙碌");
}

// ---------------------------------------------------------------------------------------------
// 逾時與忙碌
// ---------------------------------------------------------------------------------------------

const SHORT_TIMEOUT: Duration = Duration::from_millis(120);
/// 逾時返回的容許延遲上限（排程抖動）；遠小於「卡住」的時間（卡住的請求不放行就永遠不返回）。
const TIMEOUT_SLACK: Duration = Duration::from_millis(600);

#[test]
fn timeout_returns_after_timeout_value_not_blocking_time() {
    let h = harness(SHORT_TIMEOUT, Duration::from_secs(5), true);
    let t = Instant::now();
    let err = h.service.list_monitors().unwrap_err();
    let elapsed = t.elapsed();
    assert_eq!(
        err,
        WallpaperError::Timeout {
            request: RequestKind::ListMonitors,
            timeout: SHORT_TIMEOUT,
        }
    );
    assert!(elapsed >= SHORT_TIMEOUT, "不應早於逾時返回：{elapsed:?}");
    assert!(
        elapsed < SHORT_TIMEOUT + TIMEOUT_SLACK,
        "應在逾時後立即返回，實測 {elapsed:?}"
    );
    assert!(h.service.is_busy(), "逾時後應標為忙碌");
    assert!(
        h.log
            .warns()
            .iter()
            .any(|w| w.contains("list_monitors") && w.contains("逾時")),
        "逾時應記警告：{:?}",
        h.log.warns()
    );
    h.release_once();
}

#[test]
fn busy_rejects_immediately_without_reaching_backend() {
    let h = harness(SHORT_TIMEOUT, Duration::from_secs(5), true);
    assert!(matches!(
        h.service.list_monitors(),
        Err(WallpaperError::Timeout { .. })
    ));
    let before = h.calls();

    for _ in 0..3 {
        let t = Instant::now();
        let err = h
            .service
            .set_wallpaper("\\\\?\\DISPLAY#A", Path::new("C:\\x-a.png"))
            .unwrap_err();
        let elapsed = t.elapsed();
        assert_eq!(
            err,
            WallpaperError::Busy {
                request: RequestKind::SetWallpaper
            }
        );
        assert!(
            elapsed < Duration::from_millis(50),
            "忙碌時應立即返回，實測 {elapsed:?}"
        );
    }
    assert!(matches!(h.service.read(), Err(WallpaperError::Busy { .. })));

    h.release_once();
    h.wait_not_busy();
    assert_eq!(before, vec!["list_monitors"]);
    assert_eq!(
        h.calls(),
        before,
        "忙碌期間的請求不得送到後端（也不得事後補送）"
    );
}

#[test]
fn busy_clears_when_hung_request_returns_and_new_requests_flow() {
    let h = harness(SHORT_TIMEOUT, Duration::from_secs(5), true);
    assert!(matches!(
        h.service.list_monitors(),
        Err(WallpaperError::Timeout { .. })
    ));
    assert!(h.service.is_busy());

    h.release_once();
    h.wait_not_busy();
    // 解除忙碌的記錄在放開閘門後才寫，稍等。
    let deadline = Instant::now() + Duration::from_secs(5);
    while !h.log.infos().iter().any(|i| i.contains("解除忙碌")) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        h.log.infos().iter().any(|i| i.contains("解除忙碌")),
        "解除忙碌應記一行：{:?}",
        h.log.infos()
    );

    h.service
        .set_position(WallpaperPosition::FILL)
        .expect("解除忙碌後應可再送");
    assert_eq!(h.calls(), vec!["list_monitors", "set_position"]);
}

#[test]
fn queued_request_of_timed_out_caller_is_skipped() {
    // 兩個呼叫端並行：A 的請求卡住（逾時較長，尚未逾時），B 的請求排在後面並先逾時。
    // B 逾時後，工作執行緒輪到它時應直接略過，不在 explorer 恢復後才執行。
    let h = harness(SHORT_TIMEOUT, Duration::from_secs(5), true);
    let svc_a = WallpaperService {
        inner: Arc::new(Inner {
            tx: h.service.inner.tx.clone(),
            gate: h.service.inner.gate.clone(),
            config: WallpaperServiceConfig {
                timeout: Duration::from_secs(10),
                slow_threshold: Duration::from_secs(5),
                log: h.log.clone(),
            },
        }),
    };
    let a = thread::spawn(move || svc_a.list_monitors());
    h.wait_entered();

    let t = Instant::now();
    let err_b = h
        .service
        .set_wallpaper("\\\\?\\DISPLAY#A", Path::new("C:\\x-a.png"))
        .unwrap_err();
    assert!(matches!(err_b, WallpaperError::Timeout { .. }), "{err_b:?}");
    assert!(t.elapsed() < SHORT_TIMEOUT + TIMEOUT_SLACK);

    h.release_once();
    let res_a = a.join().expect("A 不應 panic");
    assert!(res_a.is_ok(), "A 未逾時，應拿到結果：{res_a:?}");
    h.wait_not_busy();
    h.service.read().expect("佇列清空後應可再送");
    assert_eq!(
        h.calls(),
        vec!["list_monitors", "read"],
        "B 的 set_wallpaper 已被放棄，不得執行"
    );
}

#[test]
fn result_arriving_with_timeout_is_returned_not_discarded() {
    // 逾時與結果同時到達（結果已在 channel、工作執行緒還沒標記完成）：應回結果，不宣告逾時、
    // 不設忙碌、不記「逾時」警告。直接驅動逾時分支以重現這個只有幾微秒寬的時間窗。
    let gate = Mutex::new(Gate {
        next_id: 1,
        last_finished: 0,
        hung: None,
    });
    let (tx, rx) = mpsc::sync_channel::<Result<u32, BackendError>>(1);
    tx.send(Ok(7)).unwrap();
    let cancelled = AtomicBool::new(false);
    let log = CaptureLog::default();
    let r = on_timeout(
        &rx,
        &gate,
        &cancelled,
        1,
        RequestKind::Read,
        SHORT_TIMEOUT,
        &log,
    );
    assert_eq!(r, Ok(7));
    assert!(lock_gate(&gate).hung.is_none(), "不應設忙碌");
    assert!(!cancelled.load(Ordering::SeqCst), "不應標記取消");
    assert!(log.warns().is_empty(), "不應記逾時警告：{:?}", log.warns());
}

#[test]
fn timeout_without_result_marks_busy_and_cancels() {
    let gate = Mutex::new(Gate {
        next_id: 1,
        last_finished: 0,
        hung: None,
    });
    let (_tx, rx) = mpsc::sync_channel::<Result<u32, BackendError>>(1);
    let cancelled = AtomicBool::new(false);
    let log = CaptureLog::default();
    let r = on_timeout(
        &rx,
        &gate,
        &cancelled,
        1,
        RequestKind::Read,
        SHORT_TIMEOUT,
        &log,
    );
    assert_eq!(
        r,
        Err(WallpaperError::Timeout {
            request: RequestKind::Read,
            timeout: SHORT_TIMEOUT
        })
    );
    assert_eq!(lock_gate(&gate).hung.map(|h| h.id), Some(1));
    assert!(cancelled.load(Ordering::SeqCst));
    assert_eq!(log.warns().len(), 1);
}

#[test]
fn dropping_service_while_worker_hung_does_not_block() {
    let h = harness(SHORT_TIMEOUT, Duration::from_secs(5), true);
    assert!(matches!(
        h.service.list_monitors(),
        Err(WallpaperError::Timeout { .. })
    ));
    let Harness {
        service, release, ..
    } = h;
    let t = Instant::now();
    drop(service);
    assert!(
        t.elapsed() < Duration::from_millis(50),
        "丟棄服務不得等待卡住的工作執行緒"
    );
    drop(release);
}

// ---------------------------------------------------------------------------------------------
// 主執行緒防護
// ---------------------------------------------------------------------------------------------

#[test]
fn forbidden_caller_is_main_thread_only() {
    assert!(is_forbidden_caller(Some("main")));
    assert!(!is_forbidden_caller(Some("fc-wallpaper-scheduler")));
    assert!(!is_forbidden_caller(Some("tokio-runtime-worker")));
    assert!(!is_forbidden_caller(None));
}

#[test]
fn request_on_thread_named_main_is_rejected_without_reaching_backend() {
    let h = fast();
    let svc = h.service.clone();
    let joined = thread::Builder::new()
        .name("main".to_owned())
        .spawn(move || svc.read())
        .expect("應能建立執行緒")
        .join();
    if cfg!(debug_assertions) {
        assert!(joined.is_err(), "debug 建置應 debug_assert 失敗");
    } else {
        assert_eq!(
            joined.expect("release 建置不應 panic"),
            Err(WallpaperError::WrongThread {
                request: RequestKind::Read
            })
        );
    }
    assert!(h.calls().is_empty(), "主執行緒的請求不得送到後端");
    h.service.read().expect("一般執行緒照常可用");
}

// ---------------------------------------------------------------------------------------------
// 純函式：GetMonitorRECT 分類、UTF-16 解碼、投影片適用性
// ---------------------------------------------------------------------------------------------

const E_FAIL_HR: i32 = 0x8000_4005_u32 as i32;

fn probe(path: &str, hr: i32, ltrb: [i32; 4]) -> MonitorProbe {
    MonitorProbe {
        device_path: path.to_owned(),
        hr,
        ltrb,
    }
}

#[test]
fn monitor_rect_classification() {
    let r = classify_monitor_rect(0, [3840, 4, 6400, 1604]).expect("S_OK 應成功");
    assert_eq!(
        r,
        MonitorRect::Online(PhysicalRect {
            x: 3840,
            y: 4,
            width: 2560,
            height: 1600
        })
    );
    assert_eq!(
        classify_monitor_rect(1, [0; 4]),
        Ok(MonitorRect::Detached),
        "S_FALSE＝離線"
    );
    // 2026-10-06 實機：拔掉的螢幕 GetMonitorRECT 回 E_FAIL＝這台離線，不是整個介面失效。
    assert_eq!(
        classify_monitor_rect(E_FAIL_HR, [0; 4]),
        Ok(MonitorRect::Failed(E_FAIL_HR))
    );
    for lost in [
        0x8001_0108_u32, // RPC_E_DISCONNECTED
        0x8001_0007,     // RPC_E_SERVER_DIED
        0x8001_0012,     // RPC_E_SERVER_DIED_DNE
        0x8001_0114,     // RPC_E_INVALID_OBJECT
        0x8004_01FD,     // CO_E_OBJNOTCONNECTED
        0x8008_0008,     // CO_E_SERVER_STOPPING
        0x8007_06BA,     // HRESULT_FROM_WIN32(RPC_S_SERVER_UNAVAILABLE)
        0x8007_06BE,     // HRESULT_FROM_WIN32(RPC_S_CALL_FAILED)
        0x8007_06BF,     // HRESULT_FROM_WIN32(RPC_S_CALL_FAILED_DNE)
    ] {
        let err = classify_monitor_rect(lost as i32, [0; 4]).unwrap_err();
        assert!(matches!(err, BackendError::Com(_)), "{lost:#x}：{err:?}");
        assert!(err.invalidates_interface(), "{lost:#x} 應觸發重建");
    }
    assert!(
        classify_monitor_rect(2, [0; 4]).is_err(),
        "非預期的成功碼也不當離線"
    );
}

#[test]
fn one_failing_monitor_rect_marks_only_that_monitor_offline() {
    // 拔掉 4K：系統作用中只剩 1 台、在線 1 台 → 失敗的那台必然不是作用中的＝離線。
    let list = assemble_monitor_list(
        vec![
            probe("\\\\?\\DISPLAY#4K", E_FAIL_HR, [0; 4]),
            probe("\\\\?\\DISPLAY#LAPTOP", 0, [0, 0, 2560, 1600]),
        ],
        Some(1),
    )
    .expect("在線那台照常回傳");
    assert_eq!(list.len(), 2, "離線那台仍帶路徑、維持索引順序");
    assert_eq!(list[0].device_path, "\\\\?\\DISPLAY#4K");
    assert!(!list[0].is_online());
    assert!(list[1].is_online());
    assert_eq!(list[1].rect.map(|r| (r.x, r.width)), Some((0, 2560)));
}

/// [審查 Medium-1／High-1] 在線台數少於系統作用中的顯示器數＝失敗者至少一台是作用中的（暫時錯誤、
/// 插回後 explorer 落後）：整批回錯（可重試），不得當成離線。系統顯示器數讀不到時也一樣。
#[test]
fn failing_rect_on_an_active_monitor_fails_the_whole_list() {
    let probes = || {
        vec![
            probe("A", E_FAIL_HR, [0; 4]),
            probe("B", 0, [0, 0, 2560, 1600]),
        ]
    };
    for (active, why) in [
        (Some(2), "作用中 2 台、在線 1 台"),
        (None, "讀不到系統顯示器數"),
    ] {
        let err = assemble_monitor_list(probes(), active).unwrap_err();
        assert!(matches!(err, BackendError::Com(_)), "{why}：{err:?}");
    }
    // 其他暫時錯誤碼（不是介面斷線）同樣依台數判定。
    let rejected = 0x8001_0001_u32 as i32; // RPC_E_CALL_REJECTED
    assert!(assemble_monitor_list(
        vec![probe("A", rejected, [0; 4]), probe("B", 0, [0, 0, 10, 10])],
        Some(2)
    )
    .is_err());
    // 台數已足：同一組結果視為離線。
    assert!(assemble_monitor_list(probes(), Some(1)).is_ok());
    // 沒有任何不在線的項目：不看系統顯示器數（explorer 漏列某台、列舉少於系統數也照常回傳，不得
    // 永久失敗）。
    let list = assemble_monitor_list(vec![probe("B", 0, [0, 0, 10, 10])], Some(3)).unwrap();
    assert!(list[0].is_online());
    let list = assemble_monitor_list(vec![probe("B", 0, [0, 0, 10, 10])], None).unwrap();
    assert!(list[0].is_online());
}

/// [複審 Medium-A] `S_FALSE`（文件記載的已拔除）同樣要以台數佐證：插回時 explorer 落後也可能這樣回。
/// 台數已足才算離線；讀不到系統台數時照舊視為離線（失敗 HRESULT 則否）。
#[test]
fn s_false_monitor_also_needs_the_active_count_to_be_offline() {
    let probes = || vec![probe("A", 1, [0; 4]), probe("B", 0, [0, 0, 10, 10])];
    let err = assemble_monitor_list(probes(), Some(2)).unwrap_err();
    assert!(matches!(err, BackendError::Com(_)), "{err:?}");
    let list = assemble_monitor_list(probes(), Some(1)).unwrap();
    assert!(!list[0].is_online());
    let list = assemble_monitor_list(probes(), None).unwrap();
    assert!(!list[0].is_online(), "讀不到台數：S_FALSE 照舊離線");
    // 全部 S_FALSE、沒有在線者：系統仍有作用中的顯示器＝explorer 落後，回錯；系統也是 0 台才照常回傳。
    assert!(assemble_monitor_list(vec![probe("A", 1, [0; 4])], Some(1)).is_err());
    let list = assemble_monitor_list(vec![probe("A", 1, [0; 4])], Some(0)).unwrap();
    assert!(!list[0].is_online());
}

#[test]
fn monitor_list_fails_only_when_nothing_is_online_or_interface_is_lost() {
    // 全部 RECT 失敗：沿用既有錯誤處理（整個列舉回錯、觸發重建）。
    let err = assemble_monitor_list(
        vec![probe("A", E_FAIL_HR, [0; 4]), probe("B", E_FAIL_HR, [0; 4])],
        Some(2),
    )
    .unwrap_err();
    assert!(err.invalidates_interface(), "{err:?}");
    // 失敗＋S_FALSE、沒有在線者：同上（系統顯示器數為 0 也一樣）。
    for active in [Some(1), Some(0), None] {
        assert!(assemble_monitor_list(
            vec![probe("A", E_FAIL_HR, [0; 4]), probe("B", 1, [0; 4])],
            active
        )
        .is_err());
    }
    // 全部 S_FALSE、系統也沒有作用中的顯示器：照舊回傳（全部離線，不是錯誤）。
    let all_detached = assemble_monitor_list(vec![probe("A", 1, [0; 4])], Some(0)).unwrap();
    assert!(!all_detached[0].is_online());
    // 空列舉：照舊回空清單。
    assert_eq!(assemble_monitor_list(Vec::new(), Some(1)), Ok(Vec::new()));
    // 介面斷線：即使另一台在線、台數已足也整個回錯。
    assert!(assemble_monitor_list(
        vec![
            probe("A", 0, [0, 0, 10, 10]),
            probe("B", 0x8001_0108_u32 as i32, [0; 4]),
        ],
        Some(1)
    )
    .is_err());
}

/// 介面斷線類 HRESULT 與 windows crate 的常數一致（數值查證）。
#[test]
fn interface_lost_hresults_match_windows_constants() {
    use windows::Win32::Foundation::{
        CO_E_OBJNOTCONNECTED, CO_E_SERVER_STOPPING, E_FAIL, RPC_E_CALL_REJECTED,
        RPC_E_DISCONNECTED, RPC_E_INVALID_OBJECT, RPC_E_SERVER_DIED, RPC_E_SERVER_DIED_DNE,
    };
    assert_eq!(RPC_E_DISCONNECTED.0 as u32, 0x8001_0108);
    assert_eq!(RPC_E_SERVER_DIED.0 as u32, 0x8001_0007);
    assert_eq!(RPC_E_SERVER_DIED_DNE.0 as u32, 0x8001_0012);
    assert_eq!(RPC_E_INVALID_OBJECT.0 as u32, 0x8001_0114);
    assert_eq!(CO_E_OBJNOTCONNECTED.0 as u32, 0x8004_01FD);
    assert_eq!(CO_E_SERVER_STOPPING.0 as u32, 0x8008_0008);
    // RPC_S_SERVER_UNAVAILABLE＝1722、RPC_S_CALL_FAILED＝1726、RPC_S_CALL_FAILED_DNE＝1727：windows crate
    // 的 `Win32::System::Rpc`（本專案未開這個 feature，故不 import；數值以 crate 原始碼核對）。
    assert_eq!(
        windows::core::HRESULT::from_win32(1722).0 as u32,
        0x8007_06BA
    );
    assert_eq!(
        windows::core::HRESULT::from_win32(1726).0 as u32,
        0x8007_06BE
    );
    assert_eq!(
        windows::core::HRESULT::from_win32(1727).0 as u32,
        0x8007_06BF
    );
    // 測試用的暫時錯誤碼（不在斷線清單內）。
    assert_eq!(RPC_E_CALL_REJECTED.0 as u32, 0x8001_0001);
    assert_eq!(E_FAIL.0, E_FAIL_HR);
}

#[test]
fn decode_wide_rejects_invalid_utf16_instead_of_empty() {
    let ok: Vec<u16> = "C:\\桌布\\a.jpg".encode_utf16().collect();
    assert_eq!(decode_wide("路徑", &ok).as_deref(), Ok("C:\\桌布\\a.jpg"));
    assert_eq!(
        decode_wide("路徑", &[]).as_deref(),
        Ok(""),
        "空字串＝純色，合法"
    );
    let err = decode_wide("路徑", &[0x43, 0xD800, 0x41]).unwrap_err();
    assert!(matches!(err, BackendError::Invalid(_)), "{err:?}");
    assert!(!err.invalidates_interface(), "資料問題不觸發重建");
}

#[test]
fn slideshow_applicable_follows_slideshow_flag() {
    assert!(!slideshow_applicable(0));
    assert!(
        !slideshow_applicable(SLIDESHOW_STATE_ENABLED),
        "本機實測 status=1：不是投影片"
    );
    assert!(slideshow_applicable(SLIDESHOW_STATE_SLIDESHOW));
    assert!(slideshow_applicable(
        SLIDESHOW_STATE_ENABLED | SLIDESHOW_STATE_SLIDESHOW
    ));
    assert!(!slideshow_applicable(
        SLIDESHOW_STATE_DISABLED_BY_REMOTE_SESSION
    ));
}

// ---------------------------------------------------------------------------------------------
// 耗時警告
// ---------------------------------------------------------------------------------------------

#[test]
fn slow_request_logs_warning() {
    let h = harness(Duration::from_secs(5), Duration::from_millis(30), false);
    h.shared.lock().unwrap().delay = Duration::from_millis(80);
    h.service
        .set_wallpaper("\\\\?\\DISPLAY#A", Path::new("C:\\x-a.png"))
        .expect("慢但成功");
    // 結果先送回呼叫端、工作執行緒才量耗時記警告，故稍等它寫完。
    let deadline = Instant::now() + Duration::from_secs(5);
    while h.log.warns().is_empty() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    let warns = h.log.warns();
    assert_eq!(warns.len(), 1, "應恰好一則警告：{warns:?}");
    assert!(warns[0].contains("set_wallpaper"), "{warns:?}");
    assert!(warns[0].contains("耗時"), "{warns:?}");
}

#[test]
fn fast_request_does_not_log_warning() {
    let h = harness(Duration::from_secs(5), Duration::from_millis(500), false);
    h.service.read().expect("讀取應成功");
    // 下一個請求返回時，前一個請求的耗時判定必已完成（佇列依序處理）。
    h.service.list_monitors().expect("列舉應成功");
    assert!(h.log.warns().is_empty(), "{:?}", h.log.warns());
}

#[test]
fn default_config_matches_design_values() {
    let c = WallpaperServiceConfig::default();
    assert_eq!(c.timeout, Duration::from_secs(10));
    assert_eq!(c.slow_threshold, Duration::from_secs(1));
}

// ---------------------------------------------------------------------------------------------
// 介面重建
// ---------------------------------------------------------------------------------------------

#[test]
fn first_request_creates_interface_once() {
    let h = fast();
    assert_eq!(h.recreates(), 0, "建立服務時不建立介面");
    h.service.list_monitors().expect("列舉應成功");
    assert_eq!(h.recreates(), 1, "第一個請求前應建立介面");
    h.service.read().expect("讀取應成功");
    assert_eq!(h.recreates(), 1, "介面正常時不重建");
}

#[test]
fn interface_lost_triggers_recreate_before_next_request() {
    let h = fast();
    h.service.list_monitors().expect("列舉應成功");
    assert_eq!(h.recreates(), 1);

    h.shared.lock().unwrap().fail_next = Some(BackendError::Com("RPC_E_DISCONNECTED".to_owned()));
    let err = h
        .service
        .set_wallpaper("\\\\?\\DISPLAY#A", Path::new("C:\\x-a.png"))
        .unwrap_err();
    assert!(matches!(
        err,
        WallpaperError::Backend {
            error: BackendError::Com(_),
            ..
        }
    ));
    assert_eq!(h.recreates(), 1, "失敗的請求不自動重試");

    h.service.list_monitors().expect("重建後列舉應成功");
    assert_eq!(h.recreates(), 2, "介面失效後，下一個請求前應重建");
    h.service.read().expect("讀取應成功");
    assert_eq!(h.recreates(), 2, "重建成功後不再重建");
    assert!(
        h.log
            .infos()
            .iter()
            .any(|i| i.contains("重建") && i.contains("上一個 COM 請求失敗")),
        "{:?}",
        h.log.infos()
    );
}

#[test]
fn invalid_data_error_does_not_trigger_recreate() {
    let h = fast();
    h.service.list_monitors().expect("列舉應成功");
    h.shared.lock().unwrap().fail_next = Some(BackendError::Invalid("無效 UTF-16".to_owned()));
    assert!(h.service.read().is_err());
    h.service.list_monitors().expect("列舉應成功");
    assert_eq!(h.recreates(), 1);
}

#[test]
fn explorer_pid_change_triggers_recreate() {
    let h = fast();
    h.shared.lock().unwrap().pid = Some(100);
    h.service.list_monitors().expect("列舉應成功");
    assert_eq!(h.recreates(), 1);

    // explorer 重啟中：查不到 PID，不算改變。
    h.shared.lock().unwrap().pid = None;
    h.service.list_monitors().expect("列舉應成功");
    assert_eq!(h.recreates(), 1);

    h.shared.lock().unwrap().pid = Some(200);
    h.service.list_monitors().expect("列舉應成功");
    assert_eq!(h.recreates(), 2, "explorer PID 改變應重建");
    assert!(
        h.log
            .infos()
            .iter()
            .any(|i| i.contains("100") && i.contains("200")),
        "{:?}",
        h.log.infos()
    );

    h.service.list_monitors().expect("列舉應成功");
    assert_eq!(h.recreates(), 2, "PID 不變不重建");
}

#[test]
fn backend_panic_aborts_request_and_worker_survives() {
    let h = fast();
    h.service.list_monitors().expect("列舉應成功");
    h.shared.lock().unwrap().panic_next = true;
    let err = h.service.read().unwrap_err();
    assert_eq!(
        err,
        WallpaperError::Aborted {
            request: RequestKind::Read
        }
    );
    assert!(!h.service.is_busy());
    h.service
        .list_monitors()
        .expect("panic 後工作執行緒應仍可用");
    assert_eq!(h.recreates(), 2, "panic 後狀態未知，下一個請求前重建");
}

#[test]
fn tracker_decisions() {
    let mut t = RecreateTracker::default();
    assert_eq!(t.decide(false, Some(1)), Some(RecreateReason::NotReady));
    t.on_recreated(true, Some(1));
    assert_eq!(t.decide(true, Some(1)), None);
    assert_eq!(t.decide(true, None), None, "PID 查不到不算改變");
    assert_eq!(
        t.decide(true, Some(2)),
        Some(RecreateReason::ExplorerRestarted { old: 1, new: 2 })
    );
    t.on_job_done(true);
    assert_eq!(
        t.decide(true, Some(1)),
        Some(RecreateReason::PreviousFailure)
    );
    t.on_recreated(false, Some(1));
    assert_eq!(
        t.decide(true, Some(1)),
        Some(RecreateReason::PreviousFailure),
        "重建失敗時維持待重建"
    );
    t.on_recreated(true, None);
    assert_eq!(
        t.decide(true, Some(5)),
        None,
        "建立時 PID 未知：之後第一次查到才綁定"
    );
    t.observe_pid(Some(5));
    assert_eq!(
        t.decide(true, Some(6)),
        Some(RecreateReason::ExplorerRestarted { old: 5, new: 6 })
    );
}

// ---------------------------------------------------------------------------------------------
// 真實 COM 後端：人工唯讀驗收（`cargo test` 不跑）
// ---------------------------------------------------------------------------------------------

/// 只做唯讀操作（列舉＋讀取＋讀登錄快照＋讀焦點），不改桌布、不寫登錄。執行前先確認沒有其他
/// 桌面驗收在跑：`cargo test real_com_backend_read_only -- --ignored --nocapture`
#[test]
#[ignore = "呼叫真的 IDesktopWallpaper（唯讀）；人工驗收用"]
fn real_com_backend_read_only() {
    // 與 spawn_com 相同的後端，只多在工作執行緒上回報執行緒 ID，供下方檢查它擁有的視窗。
    let (tid_tx, tid_rx) = mpsc::channel();
    let service = WallpaperService::spawn_with(WallpaperServiceConfig::default(), move || {
        let _ = tid_tx.send(crate::desktop::current_thread_id());
        ComBackend::new()
    })
    .expect("應能建立工作執行緒");
    let mons = service.list_monitors().expect("列舉應成功");
    println!("monitors = {mons:#?}");
    let snap = service.read().expect("讀取應成功（Ok＝記錄完整）");
    println!("snapshot = {snap:#?}");
    for m in snap.monitors.iter().filter(|m| m.monitor.is_online()) {
        assert!(m.wallpaper.is_some(), "在線螢幕的桌布必為 Some：{m:?}");
    }
    assert_eq!(
        snap.slideshow.is_some(),
        slideshow_applicable(snap.slideshow_status),
        "slideshow 為 Some 若且唯若狀態含 SLIDESHOW"
    );
    // 登錄與焦點：一般函式，不經 COM 執行緒；唯讀。
    let reg = registry::snapshot_desktop_registry().expect("讀登錄快照應成功");
    println!("registry = {reg:#?}");
    println!(
        "registry（解碼）= Wallpaper {:?} / WallpaperStyle {:?} / TileWallpaper {:?}",
        reg.wallpaper.as_string(),
        reg.wallpaper_style.as_string(),
        reg.tile_wallpaper.as_string()
    );
    let spot = registry::read_spotlight();
    println!("spotlight = {spot:#?}");
    assert!(!mons.is_empty(), "至少應列舉到一台螢幕");

    // 工作執行緒閒置（阻塞在佇列）時不得擁有任何視窗（見模組文件「COM apartment」）。
    let worker_tid = tid_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("工作執行緒應回報 ID");
    let worker_windows = com_backend::thread_windows(worker_tid);
    println!(
        "worker thread {worker_tid} 擁有視窗 {} 扇：{worker_windows:?}",
        worker_windows.len()
    );

    // 對照組：同樣的唯讀呼叫放在 STA 執行緒上，證明上面的檢查偵測得到 OLE 隱藏視窗。
    match com_backend::sta_control_thread_windows() {
        Ok(w) => println!("STA 對照組擁有視窗 {} 扇：{w:?}", w.len()),
        Err(e) => println!("STA 對照組失敗：{e}"),
    }
    assert!(
        worker_windows.is_empty(),
        "桌布 COM 工作執行緒不得擁有視窗：{worker_windows:?}"
    );
}
