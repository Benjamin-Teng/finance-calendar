//! Task 1.3 焦點與觸控探針。
//!
//! 驗證 design.md D8「不搶焦點」／「不進 Alt+Tab」在 task 3.1 的實作
//! （`focusable(false)` ＋ `desktop::apply_widget_ex_style` 加 `WS_EX_TOOLWINDOW`）
//! 是否足夠，不需要另外攔 `WM_MOUSEACTIVATE`：
//!
//! - 建一扇與正式小工具相同組態的視窗（見 `setup` 內註解，對照 `host/src/widgets.rs`
//!   `create_widget_window` 與 `host/src/desktop.rs`；examples 無法 `use` 到 bin crate 的
//!   模組——同 `probe_wind.rs` 的作法，本檔重新實作所需的最小子集，不是重造整個模組）。
//! - 頁面內容是一個可捲動清單（60 筆項目、`overflow-y:auto`），對照正式小工具
//!   `.evlist.scroll-area`（`host/ui/widget.css`）的捲動方式。清單 `scroll` 事件把
//!   `scrollTop` 寫進 `location.hash`（`#st-<n>`），本探針用 window timer 呼叫
//!   `WebviewWindow::url()` 讀回目前網址、取 fragment，把每次變化記錄下來——沒有走 Tauri
//!   IPC／自訂 scheme 以外的機制，維持與 `probe_wind.rs` 同等的「零額外相依」風格。
//!   （原本試過改寫 `document.title`，實測 wry／WebView2 不會把頁面的 `document.title`
//!   同步回原生視窗標題——`GetWindowTextW` 讀到的一直是建立時的靜態標題，故改用
//!   `location.hash`＋`url()` 這條確定會反映到 Rust 端的路徑。）
//! - 子類別化 `WM_MOUSEACTIVATE`／`WM_ACTIVATE`／`WM_SETFOCUS`／`WM_NCACTIVATE`：一律呼叫
//!   `DefSubclassProc`（不覆寫回傳值），只記錄「有沒有收到、系統預設怎麼處理」——用來判斷
//!   `WS_EX_NOACTIVATE` 視窗的預設行為（文件記載 `DefWindowProc` 對這類視窗的
//!   `WM_MOUSEACTIVATE` 預設回 `MA_NOACTIVATE`）是否已經足夠，brief 要求「不行則改
//!   `WM_MOUSEACTIVATE` 方案」的判斷依據就在這幾行記錄。
//! - 啟動時立刻做一次「切換置底旗標」測試：呼叫 Tauri `WebviewWindow::set_always_on_bottom`
//!   在 `false`／`true` 之間切換各一次（design.md D8：tao 每次旗標變更都整組重寫
//!   `GWL_EXSTYLE`，`WS_EX_TOOLWINDOW` 從未被 tao 設過，因此必定被沖掉；`WS_EX_NOACTIVATE`
//!   由「可否聚焦」旗標決定，理論上不受影響），每次切換後都記錄「tao 剛重寫完」與
//!   「本檔重新套用 `apply_widget_ex_style` 之後」兩個時間點的 `GWL_EXSTYLE`，並各自附一次
//!   Alt+Tab 判準結果。這是 task 1.3 brief 明文要求的情境（不是 Win+D 的暫停置底路徑——
//!   那條路徑依 design.md D8 定案是子類別化 `WM_WINDOWPOSCHANGING`、刻意不呼叫
//!   `set_always_on_bottom`，見 `desktop.rs` 模組文件「重要不變量」一節）。
//! - Alt+Tab 判準（controller 2026-09-28 補充：不模擬 Alt+Tab 按鍵、不截圖，改用清單判準
//!   直接檢查本視窗自己是否符合「會出現在 Alt+Tab」的條件）：可見
//!   （`IsWindowVisible`）、非 cloaked（`DwmGetWindowAttribute(DWMWA_CLOAKED)`）、無 owner
//!   （`GetWindow(hwnd, GW_OWNER)`）、且（無 `WS_EX_TOOLWINDOW` 或有 `WS_EX_APPWINDOW`）。
//!   四項全部成立才判定「會出現」；本探針視窗預期在每個檢查點都不成立。
//!
//! 點擊不搶焦點、滾輪捲動由驅動腳本 `host/tools/probe-1.3.ps1` 從外部注入（本機游標／鍵盤，
//! 注入前檢查 `LogonUI.exe`，見該檔）；觸控板兩指捲動與觸控螢幕拖曳無法自動模擬，人工步驟見
//! `.superpowers/sdd/tasks/human-checklist.md`「Task 1.3」節。
//!
//! 執行（純 cargo）：
//! `cargo run --release --example probe_focus_touch -- --log <記錄檔> --info <json 檔>`
//! 旗標見 [`parse_args`]。
#![windows_subsystem = "windows"]

use std::{
    env,
    fs::{self, File},
    io::Write,
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex, OnceLock,
    },
};

use tauri::{
    http::{header, Response, StatusCode},
    WebviewUrl, WebviewWindow, WebviewWindowBuilder, Wry,
};
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED},
    System::SystemInformation::GetLocalTime,
    UI::{
        Shell::{DefSubclassProc, SetWindowSubclass},
        WindowsAndMessaging::{
            GetWindow, GetWindowLongPtrW, GetWindowRect, IsWindowVisible, SetTimer,
            SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, GW_OWNER, HWND_BOTTOM, SWP_FRAMECHANGED,
            SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, WM_ACTIVATE,
            WM_MOUSEACTIVATE, WM_NCACTIVATE, WM_SETFOCUS, WS_EX_APPWINDOW, WS_EX_NOACTIVATE,
            WS_EX_TOOLWINDOW,
        },
    },
};

/// 本探針用的具體 Runtime 型別：examples 的 `tauri::App`／`WebviewWindowBuilder` 沒有另外
/// 指定 Runtime 泛型時預設就是 `Wry`（`tauri::App<R = Wry>`），這裡寫出具體型別只是為了讓
/// [`WIN_PTR`] 能有一個可以還原的具體型別，不是改變執行期行為。
type Win = WebviewWindow<Wry>;

const SCHEME: &str = "probe13";
/// 60 筆項目、`overflow-y:auto`——對照正式小工具 `.evlist.scroll-area`
/// （`host/ui/widget.css`／`host/ui/widgets/dynamic.js`）的捲動方式，但內容自成一頁、不依賴
/// 任何 Tauri IPC。`scroll` 事件把 `scrollTop` 寫進 `document.title`，供本檔的視窗計時器輪詢
/// 記錄（見 [`title_poll_proc`]）。
const PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>probe_focus_touch</title>
<style>
  html,body{margin:0;height:100%;background:#0f1b2d;color:#e6edf3;
    font:13px/1.4 -apple-system,Segoe UI,sans-serif;overflow:hidden}
  #list{height:100%;overflow-y:auto;box-sizing:border-box;padding:8px 12px}
  .item{padding:8px 4px;border-bottom:1px solid rgba(255,255,255,.08)}
  .hdr{font-weight:bold;color:#ffd166;padding:4px}
</style></head>
<body>
  <div id="list"><div class="hdr">task 1.3 probe_focus_touch</div></div>
  <script>
    const list = document.getElementById('list');
    for (let i = 0; i < 60; i++) {
      const d = document.createElement('div');
      d.className = 'item';
      d.textContent = 'probe item #' + i;
      list.appendChild(d);
    }
    let last = -1;
    location.hash = 'st-0';
    list.addEventListener('scroll', () => {
      const st = Math.round(list.scrollTop);
      if (st !== last) { last = st; location.hash = 'st-' + st; }
    });
  </script>
</body></html>"#;

struct Args {
    log: PathBuf,
    info: PathBuf,
    width: f64,
    height: f64,
    /// 視窗左上角（**實體像素**，虛擬桌面座標）；未給時放筆電面板（2560×1600 實體像素）
    /// 工作區左上角，找不到該規格的螢幕才退回主螢幕（controller 2026-09-28：不要動使用者在
    /// 4K 外接螢幕上的視窗）。用實體像素而非 builder 的邏輯像素 API，理由見 `setup` 內
    /// `SetWindowPos` 前的踩雷記錄註解。
    pos: Option<(f64, f64)>,
}

/// 旗標：`--log <path>`、`--info <path>`、`--width`／`--height`（邏輯像素，會依所選顯示器
/// 縮放換算成實體像素）、`--x`／`--y`（**實體像素**，覆寫預設放置位置，見 [`Args::pos`]）。
fn parse_args() -> Args {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut a = Args {
        log: manifest.join("target").join("probe_focus_touch.log"),
        info: manifest.join("target").join("probe_focus_touch.json"),
        width: 300.0,
        height: 320.0,
        pos: None,
    };
    let mut x: Option<f64> = None;
    let mut y: Option<f64> = None;
    let mut it = env::args().skip(1);
    while let Some(k) = it.next() {
        let v = it.next().unwrap_or_default();
        match k.as_str() {
            "--log" => a.log = PathBuf::from(v),
            "--info" => a.info = PathBuf::from(v),
            "--width" => a.width = v.parse().unwrap_or(a.width),
            "--height" => a.height = v.parse().unwrap_or(a.height),
            "--x" => x = v.parse().ok(),
            "--y" => y = v.parse().ok(),
            _ => {}
        }
    }
    if let (Some(x), Some(y)) = (x, y) {
        a.pos = Some((x, y));
    }
    a
}

static LOG: OnceLock<Mutex<File>> = OnceLock::new();
static LAST_FRAGMENT: OnceLock<Mutex<String>> = OnceLock::new();

/// 建好的視窗物件洩漏成 `'static`（原始指標存進 atomic），供 [`title_poll_proc`]（實際輪詢的
/// 是 `url()` 的 fragment，函式名沿用「title」是舊稱，見下方註解）在 WM_TIMER 回呼裡呼叫
/// `WebviewWindow::url()`——`SetTimer` 的回呼只給 `HWND`／訊息／id／時間，拿不到 Tauri 物件，
/// 只能用這個方式跨函式邊界取用。
///
/// SAFETY／生命週期：全部存取都發生在主執行緒（視窗訊息迴圈派送 `WM_TIMER`），不會有資料
/// 競爭；洩漏的記憶體活到行程結束（探針行程壽命短、由驅動腳本結束後直接砍行程），不需要
/// 也不打算回收。
static WIN_PTR: AtomicUsize = AtomicUsize::new(0);

fn set_win(w: Win) -> &'static Win {
    let boxed: &'static Win = Box::leak(Box::new(w));
    WIN_PTR.store(boxed as *const Win as usize, Ordering::Relaxed);
    boxed
}

fn win() -> Option<&'static Win> {
    let p = WIN_PTR.load(Ordering::Relaxed);
    if p == 0 {
        None
    } else {
        // SAFETY: p 只會是 set_win 洩漏出的合法 &'static Win 位址；同一主執行緒讀寫。
        Some(unsafe { &*(p as *const Win) })
    }
}

fn now() -> String {
    // SAFETY: GetLocalTime 只寫回傳值，無前置條件。
    let t = unsafe { GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, t.wMilliseconds
    )
}

fn log(msg: &str) {
    if let Some(m) = LOG.get() {
        if let Ok(mut f) = m.lock() {
            let _ = writeln!(f, "{} {}", now(), msg);
        }
    }
}

fn hex(h: HWND) -> String {
    format!("0x{:X}", h.0 as usize)
}

// ── 本檔重新實作的 D8 子集（examples 無法 use 到 bin crate 的 host::desktop，
//    理由與作法同 probe_wind.rs；下列三個函式逐行對照 host/src/desktop.rs 的同名函式） ────

/// 對照 `desktop::widget_ex_style`：加 `WS_EX_TOOLWINDOW`、去 `WS_EX_APPWINDOW`，其餘位元
/// （含 `focusable(false)` 產生的 `WS_EX_NOACTIVATE`）原樣保留。
fn widget_ex_style(current: u32) -> u32 {
    (current | WS_EX_TOOLWINDOW.0) & !WS_EX_APPWINDOW.0
}

/// 對照 `desktop::apply_widget_ex_style`：讀現值 → 算目標值 → 寫入 → 讀回確認。
fn apply_widget_ex_style(hwnd: HWND) -> u32 {
    // SAFETY: hwnd 是本行程剛建立、仍存活的頂層視窗。
    let current = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
    let desired = widget_ex_style(current);
    if current != desired {
        // SAFETY: 同上；寫入延伸樣式位元，u32 as isize 在 64 位元目標上為零延伸。
        unsafe { SetWindowLongPtrW(hwnd, GWL_EXSTYLE, desired as isize) };
    }
    // SAFETY: 同上。
    (unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) }) as u32
}

/// 對照 `desktop::show_at_bottom`：`SetWindowPos(HWND_BOTTOM,
/// SWP_SHOWWINDOW|SWP_NOACTIVATE|SWP_NOMOVE|SWP_NOSIZE|SWP_FRAMECHANGED)`。
fn show_at_bottom(hwnd: HWND) {
    // SAFETY: hwnd 是本行程仍存活的頂層視窗；NOMOVE／NOSIZE 保持位置尺寸不變。
    let r = unsafe {
        SetWindowPos(
            hwnd,
            Some(HWND_BOTTOM),
            0,
            0,
            0,
            0,
            SWP_SHOWWINDOW | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_FRAMECHANGED,
        )
    };
    log(&format!("ACTION show-at-bottom ok={}", r.is_ok()));
}

// ── 記錄用的純查詢 helper ──────────────────────────────────────────────────────

fn ex_style_bits(ex: u32) -> String {
    format!(
        "ex=0x{ex:08X} TOOLWINDOW={} NOACTIVATE={} APPWINDOW={}",
        ex & WS_EX_TOOLWINDOW.0 != 0,
        ex & WS_EX_NOACTIVATE.0 != 0,
        ex & WS_EX_APPWINDOW.0 != 0
    )
}

fn log_ex_style(tag: &str, hwnd: HWND) {
    // SAFETY: 純查詢。
    let ex = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
    log(&format!("EXSTYLE {tag} {}", ex_style_bits(ex)));
}

fn is_cloaked(h: HWND) -> bool {
    let mut v: u32 = 0;
    // SAFETY: v 為 4 位元組可寫緩衝區，大小與 cbAttribute 一致。
    let r = unsafe {
        DwmGetWindowAttribute(
            h,
            DWMWA_CLOAKED,
            (&mut v as *mut u32).cast(),
            std::mem::size_of::<u32>() as u32,
        )
    };
    r.is_ok() && v != 0
}

/// Alt+Tab 清單判準（controller 2026-09-28）：可見、非 cloaked、無 owner、且（無
/// `WS_EX_TOOLWINDOW` 或有 `WS_EX_APPWINDOW`）。四項全部成立才判定「會出現在 Alt+Tab」。
fn log_alt_tab_membership(tag: &str, hwnd: HWND) {
    // SAFETY: 純查詢。
    let visible = unsafe { IsWindowVisible(hwnd) }.as_bool();
    let cloaked = is_cloaked(hwnd);
    // SAFETY: 純查詢；沒有 owner 時回 Err。
    let owner = unsafe { GetWindow(hwnd, GW_OWNER) }.ok();
    let ex = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
    let toolwindow = ex & WS_EX_TOOLWINDOW.0 != 0;
    let appwindow = ex & WS_EX_APPWINDOW.0 != 0;
    let would_appear = visible && !cloaked && owner.is_none() && (!toolwindow || appwindow);
    log(&format!(
        "ALTTAB {tag} visible={visible} cloaked={cloaked} owner={} toolwindow={toolwindow} \
         appwindow={appwindow} would_appear_in_alt_tab={would_appear}",
        owner.map(hex).unwrap_or_else(|| "none".into())
    ));
}

// ── 訊息觀察子類別：只記錄、一律轉交 DefSubclassProc，不覆寫任何回傳值 ────────────────

fn ma_name(r: isize) -> &'static str {
    match r {
        1 => "MA_ACTIVATE",
        2 => "MA_ACTIVATEANDEAT",
        3 => "MA_NOACTIVATE",
        4 => "MA_NOACTIVATEANDEAT",
        _ => "?",
    }
}

unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    // SAFETY: 轉交下一個子類別程序／原視窗程序；本函式不修改 wparam/lparam，純觀察。
    let r = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
    match msg {
        WM_MOUSEACTIVATE => log(&format!(
            "MSG WM_MOUSEACTIVATE default_return={} ({})",
            r.0,
            ma_name(r.0)
        )),
        WM_ACTIVATE => log(&format!("MSG WM_ACTIVATE wparam=0x{:X}", wparam.0)),
        WM_SETFOCUS => log("MSG WM_SETFOCUS（本視窗收到鍵盤焦點——不應發生）"),
        WM_NCACTIVATE => log(&format!("MSG WM_NCACTIVATE wparam=0x{:X}", wparam.0)),
        _ => {}
    }
    r
}

/// 視窗計時器：輪詢頁面目前網址的 fragment（`#st-<scrollTop>`），變了才記錄一行（design.md
/// 「透過探針頁面把 scrollTop 回報給 Rust 端記錄」）。函式名沿用「title」是最初設計的殘留
/// （最初想法是輪詢 `document.title`，實測 wry 不會同步——見本檔模組文件），機制已改為
/// `url()`，不影響呼叫端（`SetTimer` 只認函式指標）。
unsafe extern "system" fn title_poll_proc(_hwnd: HWND, _msg: u32, _id: usize, _time: u32) {
    let Some(w) = win() else { return };
    let Ok(url) = w.url() else { return };
    let fragment = url.fragment().unwrap_or("").to_string();
    let cell = LAST_FRAGMENT.get_or_init(|| Mutex::new(String::new()));
    if let Ok(mut last) = cell.lock() {
        if *last != fragment {
            log(&format!(
                "SCROLL fragment=\"{fragment}\" (prev=\"{}\")",
                *last
            ));
            *last = fragment;
        }
    }
}

/// 啟動時立刻做一次「切換置底旗標」測試（brief 明文情境，見本檔模組文件）：
/// `set_always_on_bottom(false)` → 記錄 tao 重寫後的 ex-style → 重新套用
/// `apply_widget_ex_style` → 記錄 → 記 Alt+Tab 判準；`set_always_on_bottom(true)` 重複一次；
/// 最後 `show_at_bottom` 收尾，確保測試結束後視窗回到正常置底狀態。
fn run_toggle_test<R: tauri::Runtime>(window: &tauri::WebviewWindow<R>, hwnd: HWND) {
    log("TOGGLE begin：set_always_on_bottom(false)");
    if let Err(e) = window.set_always_on_bottom(false) {
        log(&format!("TOGGLE set_always_on_bottom(false) 失敗：{e}"));
    }
    log_ex_style("after-tao-rewrite(off)", hwnd);
    apply_widget_ex_style(hwnd);
    show_at_bottom(hwnd);
    log_ex_style("after-reapply(off)", hwnd);
    log_alt_tab_membership("after-reapply(off)", hwnd);

    log("TOGGLE begin：set_always_on_bottom(true)");
    if let Err(e) = window.set_always_on_bottom(true) {
        log(&format!("TOGGLE set_always_on_bottom(true) 失敗：{e}"));
    }
    log_ex_style("after-tao-rewrite(on)", hwnd);
    apply_widget_ex_style(hwnd);
    show_at_bottom(hwnd);
    log_ex_style("after-reapply(on)", hwnd);
    log_alt_tab_membership("after-reapply(on)", hwnd);
    log("TOGGLE end");
}

fn setup(app: &mut tauri::App, a: &Args) -> Result<(), Box<dyn std::error::Error>> {
    // 找筆電面板（2560×1600 實體像素，見專案 memory「本機環境與 CLAUDE.md 的落差」）；
    // 找不到才退回主螢幕——不動使用者正在用的 4K 外接螢幕（controller 2026-09-28）。
    let monitors = app.available_monitors()?;
    for m in &monitors {
        log(&format!(
            "MONITOR name={:?} size={}x{} pos=({},{}) work_area=({},{},{}x{}) scale={}",
            m.name(),
            m.size().width,
            m.size().height,
            m.position().x,
            m.position().y,
            m.work_area().position.x,
            m.work_area().position.y,
            m.work_area().size.width,
            m.work_area().size.height,
            m.scale_factor()
        ));
    }
    let laptop = monitors
        .iter()
        .find(|m| m.size().width == 2560 && m.size().height == 1600);
    // `primary` 綁成具名區域變數（活到本函式結束）才能安全借用；直接寫
    // `app.primary_monitor()?.as_ref()` 會借用一個只活到該陳述式結尾的暫存值。
    let primary = app.primary_monitor()?;
    log(&format!(
        "MONITOR primary={:?} laptop_match={}",
        primary.as_ref().and_then(|m| m.name()),
        laptop.is_some()
    ));
    let mon = laptop
        .or(primary.as_ref())
        .or_else(|| monitors.first())
        .ok_or("no monitor")?;
    log(&format!("MONITOR chosen={:?}", mon.name()));
    let scale = mon.scale_factor();
    let work = mon.work_area();
    // 實體像素目標矩形；margin／width／height 都用「所選顯示器自己的縮放」換算成實體像素。
    let margin_phys = (40.0 * scale).round() as i32;
    let (phys_x, phys_y) = a
        .pos
        .map(|(px, py)| (px as i32, py as i32))
        .unwrap_or((work.position.x + margin_phys, work.position.y + margin_phys));
    let phys_w = (a.width * scale).round() as i32;
    let phys_h = (a.height * scale).round() as i32;

    let url = tauri::Url::parse(&format!("http://{SCHEME}.localhost/"))?;
    let win = WebviewWindowBuilder::new(app, "probe13", WebviewUrl::External(url))
        .title("probe_focus_touch")
        // 以下組態逐項對照 host/src/widgets.rs create_widget_window（task 3.1 的正式小工具
        // 視窗設定），是本探針要驗證的目標配置本身，不是探針自訂的簡化版。位置／尺寸不靠
        // builder 的 `.position()`／`.inner_size()`（只收「邏輯像素」，見下方 SetWindowPos
        // 前的說明），這裡的 `inner_size` 只是隱藏狀態下的初始尺寸提示，無視覺後果。
        .decorations(false)
        .always_on_bottom(true)
        .skip_taskbar(true)
        .focusable(false)
        .focused(false)
        .resizable(false)
        .shadow(false)
        .transparent(true)
        .visible(false)
        .inner_size(a.width, a.height)
        .build()?;
    // 洩漏成 'static，讓 title_poll_proc（WM_TIMER 回呼）能透過 win() 存取（見 WIN_PTR 文件）。
    let win = set_win(win);
    let hwnd = win.hwnd()?;

    // 踩雷記錄（本探針實測，2026-09-28）：builder 的 `.position()`／`.inner_size()` 文件只寫
    // 「邏輯像素」，沒寫「相對哪一台顯示器的縮放換算」；本機兩台顯示器縮放不同（4K 主螢幕
    // 150%、筆電面板 175%）時實測 tao 一律用「主螢幕」的縮放換算初始位置，即使目標邏輯座標
    // 是依「筆電面板」的縮放算出來的——結果視窗實際落在主螢幕（4K、使用者 Chrome 全螢幕的
    // 那台）而非筆電面板，即使 available_monitors() 回報的筆電面板 `size()` 明明是
    // 2560×1600、有正確比對到。這正是 host/src/desktop.rs `set_window_rect` 全程只用實體
    // 像素、不透過 tao 的 logical 位置 API 的理由（該檔文件也提到跨 DPI 移動時 tao 靠
    // `WM_DPICHANGED` 校正尺寸，但那是「視窗移動之後」的事後修正，不是建立當下的邏輯換算）。
    // 對策：隱藏狀態下建完就立刻用 Win32 `SetWindowPos` 以實體像素蓋掉 builder 算出的位置，
    // 不倚賴 builder 對縮放的假設。
    // SAFETY: hwnd 是本行程剛建立、仍隱藏的頂層視窗；SWP_NOZORDER 保證不動 z-order、
    // SWP_NOACTIVATE 保證不啟用。
    // 對照 desktop.rs `set_window_rect` 的「最多設兩次」：第一次呼叫本身就會跨顯示器移動
    // （從 tao 建立時假設的主螢幕移到目標顯示器），觸發系統送 `WM_DPICHANGED`，tao 收到後會
    // 依「邏輯尺寸不變」用新 DPI 又改寫一次尺寸（實測：本檔第一版只呼叫一次，結果視窗尺寸被
    // 二次縮放，613×653 而非要求的 525×560——`1.75/1.5≈1.1667`、`525×1.1667≈613`，正是
    // desktop.rs 文件描述的那個效應）。第二次呼叫時視窗已經在目標顯示器上、不會再跨 DPI，
    // 結果必定精確。
    for attempt in 0..2 {
        // SAFETY: hwnd 是本行程剛建立、仍隱藏的頂層視窗；SWP_NOZORDER 保證不動 z-order、
        // SWP_NOACTIVATE 保證不啟用。
        unsafe {
            SetWindowPos(
                hwnd,
                None,
                phys_x,
                phys_y,
                phys_w,
                phys_h,
                SWP_NOZORDER | SWP_NOACTIVATE,
            )
        }?;
        let mut actual = RECT::default();
        // SAFETY: actual 是本函式的局部變數，GetWindowRect 只寫入這個結構。
        unsafe { GetWindowRect(hwnd, &mut actual) }?;
        log(&format!(
            "PLACE attempt={attempt} target=({phys_x},{phys_y},{phys_w}x{phys_h}) \
             actual=({},{},{}x{})",
            actual.left,
            actual.top,
            actual.right - actual.left,
            actual.bottom - actual.top
        ));
        if actual.right - actual.left == phys_w && actual.bottom - actual.top == phys_h {
            break;
        }
    }

    log_ex_style("initial(before-apply)", hwnd);
    apply_widget_ex_style(hwnd);
    log_ex_style("initial(after-apply)", hwnd);

    // SAFETY: hwnd 為本行程剛建立的頂層視窗；subclass_proc 生命週期為 'static。
    if unsafe { !SetWindowSubclass(hwnd, Some(subclass_proc), 1, 0).as_bool() } {
        return Err("SetWindowSubclass failed".into());
    }
    show_at_bottom(hwnd);
    log_alt_tab_membership("initial", hwnd);

    // SAFETY: 視窗計時器，WM_TIMER 由本視窗的訊息迴圈派送給 title_poll_proc；hwnd 存活到
    // 行程結束或視窗銷毀（SetTimer 隨視窗銷毀自動失效，不需手動 KillTimer）。
    if unsafe { SetTimer(Some(hwnd), 1, 100, Some(title_poll_proc)) } == 0 {
        return Err("SetTimer(title poll) failed".into());
    }

    run_toggle_test(win, hwnd);

    let pos = win.outer_position()?;
    let size = win.outer_size()?;
    let info = format!(
        "{{\"pid\":{},\"hwnd\":\"{}\",\"rect\":[{},{},{},{}]}}",
        std::process::id(),
        hex(hwnd),
        pos.x,
        pos.y,
        size.width,
        size.height
    );
    fs::write(&a.info, &info)?;
    log(&format!("START {info}"));
    Ok(())
}

fn main() {
    let a = parse_args();
    if let Some(dir) = a.log.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let file = File::create(&a.log).expect("無法建立記錄檔");
    let _ = LOG.set(Mutex::new(file));

    tauri::Builder::default()
        .register_uri_scheme_protocol(SCHEME, |_ctx, _req| {
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                .body(PAGE.as_bytes().to_vec())
                .expect("靜態回應不會失敗")
        })
        .setup(move |app| {
            setup(app, &a)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("probe_focus_touch 執行失敗");
}
