//! 編輯版面的格線疊加視窗（widget-adaptive-zoom-and-grid task 5.1，widget-adaptive-zoom-and-grid design.md D4）。
//!
//! ## 做法：每台顯示器一個原生分層視窗
//!
//! [`GridOverlay`] 擁有一個 `WS_POPUP` 頂層視窗，延伸樣式 `WS_EX_LAYERED | WS_EX_TRANSPARENT |
//! WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW`，矩形＝該台顯示器的工作區（實體像素，虛擬桌面座標）：
//! - **畫線**：32 位元 premultiplied ARGB 的 top-down DIB（[`fill_grid_pixels`] 產生像素），
//!   `UpdateLayeredWindow(ULW_ALPHA)` 一次更新位置、尺寸與內容。分層視窗由系統保存這份內容，
//!   不需要 `WM_PAINT`、也不需要保留 DIB：每次畫完就釋放 DIB 與記憶體 DC。
//! - **滑鼠穿透**：`WS_EX_LAYERED | WS_EX_TRANSPARENT` 讓命中測試直接略過本視窗；視窗程序對
//!   `WM_NCHITTEST` 回 `HTTRANSPARENT` 作第二道保險（design.md D4）。
//! - **不搶焦點、不進工作列與 Alt+Tab**：`WS_EX_NOACTIVATE`（點擊不啟用）＋`WS_EX_TOOLWINDOW`
//!   （不出現在工作列與 Alt+Tab），顯示用 `SW_SHOWNOACTIVATE`。
//! - **置底**：顯示後 `SetWindowPos(HWND_BOTTOM, SWP_NOACTIVATE|SWP_NOMOVE|SWP_NOSIZE)` 一次。
//!   之後**不**跟小工具搶 z-order（design.md D4「z-order 不硬搶」）：拖曳中的小工具因
//!   `always_on_bottom` 暫時跑到格線下方時肉眼幾乎看不出，重新排序反而會讓兩組置底視窗互搶、
//!   造成閃爍或訊息風暴。
//! - **線條**：內部 47 條垂直＋47 條水平，位置＝`layout::grid_line_offsets`（與吸附同源），1 實體
//!   像素寬，顏色取主題色；一般線 alpha [`MINOR_ALPHA`]，每 [`MAJOR_EVERY`] 格一條加強線
//!   [`MAJOR_ALPHA`]，48 格可以 8 塊 × 6 格目測。工作區外框不畫（小工具貼邊時框線會被誤認成小工具
//!   邊框）。
//!
//! ## 執行緒（呼叫端前提）
//!
//! 建立、[`GridOverlay::update`]、銷毀（`Drop`）都必須在**同一條、有訊息迴圈的執行緒**——宿主裡就是
//! 主執行緒：視窗屬於建立它的執行緒，訊息由 tao 事件迴圈的 `GetMessage` 一併派送；Win32 視窗 API
//! 有執行緒親和性（`DestroyWindow` 在別的執行緒呼叫會失敗，memory：
//! `win32-window-apis-have-thread-affinity.md`）。工作執行緒（例如守門視窗觸發的重排）要經
//! `AppHandle::run_on_main_thread` 投遞（task 5.2 的 `sync_grid_overlay`）。
//!
//! 型別層面的保證：[`GridOverlay`] 帶 `PhantomData<*const ()>`，是 `!Send`／`!Sync`，編譯器不允許
//! 把它搬到別的執行緒更新或銷毀；debug 建置另以 `debug_assert_eq!` 核對 `update`／`Drop` 與建立時的
//! Win32 執行緒 id 相同。**沒有**斷言「必須是主執行緒」：既有的 `updater` 主執行緒判定在
//! `mark_main_thread` 未呼叫時（單元測試）一律回 `false`，本模組的實機冒煙測試也刻意在測試執行緒
//! 建立視窗；「有訊息迴圈的執行緒」這個前提由呼叫端（task 5.2）負責。
//!
//! ## 一手來源（Microsoft Learn）與 crate 簽名
//!
//! - 〈UpdateLayeredWindow function〉：`pptDst`／`psize` 同時決定新位置與尺寸；`ULW_ALPHA` 搭配
//!   `BLENDFUNCTION.AlphaFormat = AC_SRC_ALPHA` 時來源必須是 premultiplied alpha 的 32 bpp 點陣圖。
//! - 〈Extended Window Styles〉：`WS_EX_TRANSPARENT` 與 `WS_EX_LAYERED` 並用時視窗對滑鼠穿透；
//!   `WS_EX_NOACTIVATE` 點擊不成為前景；`WS_EX_TOOLWINDOW` 不出現在工作列與 Alt+Tab。
//! - 〈WM_NCHITTEST〉：`HTTRANSPARENT`＝交給同一執行緒底下的視窗處理。
//! - 〈CreateDIBSection〉：`biHeight` 為負＝top-down DIB（第一列在最上方）。
//!
//! windows crate 0.62.2 的簽名查證於 `~/.cargo/registry/src/*/windows-0.62.2/src/Windows/Win32/`：
//! `UI/WindowsAndMessaging/mod.rs` 的 `UpdateLayeredWindow(HWND, Option<HDC>, Option<*const POINT>,
//! Option<*const SIZE>, Option<HDC>, Option<*const POINT>, COLORREF, Option<*const BLENDFUNCTION>,
//! UPDATE_LAYERED_WINDOW_FLAGS) -> Result<()>`、`RegisterClassExW(*const WNDCLASSEXW) -> u16`、
//! `HTTRANSPARENT: i32`；`Graphics/Gdi/mod.rs` 的 `CreateDIBSection(Option<HDC>, *const BITMAPINFO,
//! DIB_USAGE, *mut *mut c_void, Option<HANDLE>, u32) -> Result<HBITMAP>`、`AC_SRC_OVER`／
//! `AC_SRC_ALPHA: u32`。全部在既有的 `Win32_UI_WindowsAndMessaging`／`Win32_Graphics_Gdi` feature，
//! 不需新增 Cargo feature。

use std::marker::PhantomData;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    COLORREF, ERROR_CLASS_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject,
    AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS,
    HBITMAP, HDC, HGDIOBJ,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassExW, SetWindowPos, ShowWindow,
    UpdateLayeredWindow, HTTRANSPARENT, HWND_BOTTOM, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SW_SHOWNOACTIVATE, ULW_ALPHA, WM_DPICHANGED, WM_NCHITTEST, WNDCLASSEXW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::layout::{grid_line_offsets, PhysicalRect};

/// 視窗類別名稱（行程內唯一；重複註冊回 `ERROR_CLASS_ALREADY_EXISTS` 視為成功）。
const CLASS_NAME: PCWSTR = w!("fc-host-grid-overlay");

/// 視窗標題（不顯示；方便 Spy++／`watch-zorder.ps1` 這類工具辨認）。task 5.2：刻意不寫成
/// `fc-host <名稱>`——`host/tools/` 的驗收腳本以 `fc-host *` 認小工具視窗，見測試
/// `title_and_class_do_not_look_like_a_widget_window`。
const WINDOW_TITLE: PCWSTR = w!("fc-host-grid-overlay");

/// 疊加視窗的延伸樣式（design.md D4）。
pub const OVERLAY_EX_STYLE: u32 =
    WS_EX_LAYERED.0 | WS_EX_TRANSPARENT.0 | WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0;

/// 解析失敗時的主題色（＝`settings.rs` 的 `accent_color` 預設 `#e0aa54`）。
pub const DEFAULT_ACCENT: Rgb = Rgb {
    r: 0xe0,
    g: 0xaa,
    b: 0x54,
};

/// 一般線的 alpha（widget-adaptive-zoom-and-grid design.md D4「一般線 alpha 約 0x30」）。
pub const MINOR_ALPHA: u8 = 0x30;

/// 每 6 格一條的加強線 alpha（D4「每 6 格一條較明顯（alpha 約 0x70）」）。
pub const MAJOR_ALPHA: u8 = 0x70;

/// 加強線的間隔（格數）：48 格分成 8 塊 × 6 格。
pub const MAJOR_EVERY: usize = 6;

// 加強線必須比一般線明顯，且 48 格要能被加強線整除成整塊（編譯期檢查）。
const _: () = assert!(MAJOR_ALPHA > MINOR_ALPHA);
const _: () = assert!((crate::layout::GRID as usize).is_multiple_of(MAJOR_EVERY));

/// 未 premultiply 的 sRGB 顏色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// 解析設定的主題色 `#RRGGBB`（`Settings.accent_color` 的格式，大小寫皆可）。格式不符回 `None`：
/// 長度不是 7 位元組、不以 `#` 起頭、或任一位不是 ASCII 十六進位數字（`u8::from_str_radix`
/// 會接受前導 `+`，所以先逐字檢查，不只靠它）。
pub fn parse_accent_color(s: &str) -> Option<Rgb> {
    let hex = s.strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some(Rgb {
        r: channel(0)?,
        g: channel(2)?,
        b: channel(4)?,
    })
}

/// 同 [`parse_accent_color`]，解析失敗退回 [`DEFAULT_ACCENT`]（設定檔被手改壞時格線照樣畫得出來）。
pub fn accent_or_default(s: &str) -> Rgb {
    parse_accent_color(s).unwrap_or(DEFAULT_ACCENT)
}

/// 第 `line` 條內部線（1..=47，＝`layout::grid_line_offsets` 的索引＋1）的 alpha：每
/// [`MAJOR_EVERY`] 格一條加強線（6、12、…、42），其餘一般線。
pub fn line_alpha(line: usize) -> u8 {
    if line.is_multiple_of(MAJOR_EVERY) {
        MAJOR_ALPHA
    } else {
        MINOR_ALPHA
    }
}

/// 32 位元 premultiplied ARGB 像素（`UpdateLayeredWindow` 搭配 `AC_SRC_ALPHA` 要求的格式）：
/// 各色版先乘以 `alpha / 255`（四捨五入），任一色版都不會超過 alpha。DIB 的記憶體順序是
/// B、G、R、A，以 little-endian `u32` 看即 `0xAARRGGBB`。
pub fn premultiplied_pixel(color: Rgb, alpha: u8) -> u32 {
    let mul = |c: u8| (u32::from(c) * u32::from(alpha) + 127) / 255;
    (u32::from(alpha) << 24) | (mul(color.r) << 16) | (mul(color.g) << 8) | mul(color.b)
}

/// 把格線畫進 `buf`（top-down、列優先、`width × height` 個像素）：先全部清成 0（alpha 0＝完全
/// 透明），再畫 47 條水平線、47 條垂直線，位置＝`layout::grid_line_offsets`（與吸附同源，
/// widget-adaptive-zoom-and-grid design.md D4），1 像素寬，alpha 由 [`line_alpha`] 決定；交點取
/// 兩者中較明顯的那個。工作區外框不畫：偏移落在 0（工作區小於 48 像素時才會發生）或超出範圍的線
/// 直接略過。
///
/// `width`／`height` ≤ 0、或 `buf` 長度不等於 `width × height` 時什麼都不做（不 panic）。
pub fn fill_grid_pixels(buf: &mut [u32], width: i32, height: i32, color: Rgb) {
    let (Ok(w), Ok(h)) = (usize::try_from(width), usize::try_from(height)) else {
        return;
    };
    if w == 0 || h == 0 || w.checked_mul(h) != Some(buf.len()) {
        return;
    }
    buf.fill(0);
    let inside = |offset: i32, extent: usize| {
        usize::try_from(offset)
            .ok()
            .filter(|&o| o > 0 && o < extent)
    };
    for (k, &y) in grid_line_offsets(height).iter().enumerate() {
        let Some(y) = inside(y, h) else { continue };
        let px = premultiplied_pixel(color, line_alpha(k + 1));
        for p in &mut buf[y * w..(y + 1) * w] {
            if *p >> 24 < px >> 24 {
                *p = px;
            }
        }
    }
    for (k, &x) in grid_line_offsets(width).iter().enumerate() {
        let Some(x) = inside(x, w) else { continue };
        let px = premultiplied_pixel(color, line_alpha(k + 1));
        for row in buf.chunks_exact_mut(w) {
            if row[x] >> 24 < px >> 24 {
                row[x] = px;
            }
        }
    }
}

// ── Win32：視窗本體 ─────────────────────────────────────────────────────────────

/// 一台顯示器的格線疊加視窗；擁有 HWND，`Drop` 時 `DestroyWindow`。
///
/// 只能在建立它的執行緒（宿主裡是主執行緒）使用與丟棄，見模組文件「執行緒」。
#[derive(Debug)]
pub struct GridOverlay {
    hwnd: HWND,
    work_area: PhysicalRect,
    color: Rgb,
    /// 建立時的 Win32 執行緒 id（debug 建置核對 `update`／`Drop` 在同一條執行緒）。
    owner_thread: u32,
    /// 讓本型別 `!Send`／`!Sync`：不依賴 `HWND` 目前恰好是裸指標包裝這個實作細節。
    _not_send: PhantomData<*const ()>,
}

impl GridOverlay {
    /// 建立並顯示一個覆蓋 `work_area`（實體像素）的格線視窗：註冊類別（已註冊則沿用）→
    /// `CreateWindowExW` → 畫線（`UpdateLayeredWindow`）→ `ShowWindow(SW_SHOWNOACTIVATE)` →
    /// `SetWindowPos(HWND_BOTTOM)`。任一步失敗回一行中文說明，已建立的視窗由 `Drop` 銷毀，不留殘骸。
    ///
    /// `work_area` 寬或高 ≤ 0 時不建立視窗、直接回錯誤。
    pub fn create(work_area: PhysicalRect, color: Rgb) -> Result<Self, String> {
        validate_extent(work_area)?;
        let hinstance = register_class()?;
        // SAFETY: 類別剛註冊（或先前已註冊）於本模組的 hinstance；字串是 `w!` 產生的靜態寬字元
        // 常量；沒有父視窗、選單與建立參數。失敗時 windows crate 回 Err，不會給無效 HWND。
        let hwnd = unsafe {
            CreateWindowExW(
                windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(OVERLAY_EX_STYLE),
                CLASS_NAME,
                WINDOW_TITLE,
                WS_POPUP,
                work_area.x,
                work_area.y,
                work_area.width,
                work_area.height,
                None,
                None,
                Some(hinstance),
                None,
            )
        }
        .map_err(|e| format!("建立格線視窗失敗：{e}"))?;
        // 從這裡起由 `Drop` 負責銷毀：之後任何一步失敗回 Err 時，`overlay` 被丟棄即 DestroyWindow。
        let overlay = Self {
            hwnd,
            work_area,
            color,
            owner_thread: super::current_thread_id(),
            _not_send: PhantomData,
        };
        paint(hwnd, work_area, color)?;
        // SAFETY: hwnd 是本執行緒剛建立、仍存活的頂層視窗；回傳值只表示先前是否可見，不是錯誤。
        let _ = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
        // SAFETY: 同上；NOMOVE／NOSIZE 保持 UpdateLayeredWindow 設好的矩形，NOACTIVATE 不搶焦點。
        unsafe {
            SetWindowPos(
                hwnd,
                Some(HWND_BOTTOM),
                0,
                0,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
            )
        }
        .map_err(|e| format!("SetWindowPos（格線置底）失敗：{e}"))?;
        Ok(overlay)
    }

    /// 矩形或顏色有變才重畫（`UpdateLayeredWindow` 一次搬移、改尺寸、換內容），回傳是否重畫。
    /// 失敗時保留舊的矩形與顏色紀錄，下次呼叫會再試。不改 z-order（design.md D4「z-order 不硬搶」）。
    pub fn update(&mut self, work_area: PhysicalRect, color: Rgb) -> Result<bool, String> {
        self.debug_assert_owner_thread();
        if work_area == self.work_area && color == self.color {
            return Ok(false);
        }
        validate_extent(work_area)?;
        paint(self.hwnd, work_area, color)?;
        self.work_area = work_area;
        self.color = color;
        Ok(true)
    }

    /// 視窗把手（記錄與實機測試用；不要拿去跨執行緒操作視窗）。
    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// 目前畫的工作區（task 5.2 比對「顯示器清單與工作區，不同就重建／更新」用）。
    pub fn work_area(&self) -> PhysicalRect {
        self.work_area
    }

    /// 目前畫的顏色。
    pub fn color(&self) -> Rgb {
        self.color
    }

    fn debug_assert_owner_thread(&self) {
        debug_assert_eq!(
            self.owner_thread,
            super::current_thread_id(),
            "GridOverlay 必須在建立它的執行緒使用與銷毀（見模組文件「執行緒」）"
        );
    }
}

impl Drop for GridOverlay {
    fn drop(&mut self) {
        self.debug_assert_owner_thread();
        // SAFETY: hwnd 由本物件在同一執行緒建立、只銷毀這一次；視窗已被外力銷毀時 API 回錯誤，
        // 只記錄、不 panic。
        if let Err(e) = unsafe { DestroyWindow(self.hwnd) } {
            log::warn!("銷毀格線視窗失敗：{e}");
        }
    }
}

/// 寬高必須為正（`UpdateLayeredWindow` 與 DIB 都不接受 0 或負尺寸）。
fn validate_extent(work_area: PhysicalRect) -> Result<(), String> {
    if work_area.width > 0 && work_area.height > 0 {
        Ok(())
    } else {
        Err(format!("格線工作區尺寸不合法：{work_area:?}"))
    }
}

/// 註冊視窗類別並回傳本模組的 hinstance；已註冊（`ERROR_CLASS_ALREADY_EXISTS`）視為成功——
/// 每次建立都呼叫一次，比「只註冊一次」的旗標簡單，且註冊失敗後下次還會再試。
fn register_class() -> Result<HINSTANCE, String> {
    // SAFETY: None＝本行程 exe 模組，不增加參考計數、不需釋放。
    let module = unsafe { GetModuleHandleW(PCWSTR::null()) }
        .map_err(|e| format!("GetModuleHandleW 失敗：{e}"))?;
    let hinstance = HINSTANCE(module.0);
    let class = WNDCLASSEXW {
        cbSize: u32::try_from(std::mem::size_of::<WNDCLASSEXW>()).unwrap_or(u32::MAX),
        lpfnWndProc: Some(overlay_wndproc),
        hInstance: hinstance,
        lpszClassName: CLASS_NAME,
        ..Default::default()
    };
    // SAFETY: `class` 由本函式擁有、cbSize 正確；類別名稱是靜態常量，系統會複製一份。
    let atom = unsafe { RegisterClassExW(&class) };
    if atom != 0 {
        return Ok(hinstance);
    }
    let err = windows::core::Error::from_thread();
    if err.code() == ERROR_CLASS_ALREADY_EXISTS.to_hresult() {
        Ok(hinstance)
    } else {
        Err(format!("註冊格線視窗類別失敗：{err}"))
    }
}

/// 視窗程序：`WM_NCHITTEST`→`HTTRANSPARENT`（滑鼠穿透的第二道保險）；`WM_DPICHANGED` 吃掉
/// （預設處理不該改動矩形——幾何一律由 task 5.2 依工作區重建或 [`GridOverlay::update`] 決定）；
/// 其餘交給 `DefWindowProcW`。
unsafe extern "system" fn overlay_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_DPICHANGED => LRESULT(0),
        // SAFETY: 原樣轉交系統預設處理，參數來自系統派送。
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// 以 `UpdateLayeredWindow` 把 `work_area` 大小的格線畫到 `hwnd`，並同時把視窗搬到該矩形。
/// DIB 與 DC 都是暫時的：函式結束即釋放（分層視窗由系統保存內容）。
fn paint(hwnd: HWND, work_area: PhysicalRect, color: Rgb) -> Result<(), String> {
    let (w, h) = (work_area.width, work_area.height);
    let pixels = usize::try_from(w)
        .ok()
        .zip(usize::try_from(h).ok())
        .and_then(|(w, h)| w.checked_mul(h))
        .filter(|&n| n > 0 && n.checked_mul(4).is_some())
        .ok_or_else(|| format!("格線工作區尺寸不合法：{work_area:?}"))?;

    let screen = ScreenDc::get()?;
    let mut mem = PaintDc::create(&screen)?;
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: u32::try_from(std::mem::size_of::<BITMAPINFOHEADER>()).unwrap_or(u32::MAX),
            biWidth: w,
            // 負值＝top-down，第一列在最上方，與 `fill_grid_pixels` 的列序一致。
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let bits = mem
        .create_dib(&bmi)
        .map_err(|e| format!("{e}（{w}×{h}）"))?;
    // SAFETY: `bmi` 描述 32 bpp、未壓縮、寬 w 的 DIB，每列 4w 位元組（已是 DWORD 對齊，沒有列尾
    // 填充），總共 w×h 個 u32；`bits` 非空、記憶體由 `mem` 擁有的 DIB 持有，`mem` 活到本函式結束，
    // 而 `buf` 只在下一行使用、之後不再存取，期間沒有其他別名。
    let buf = unsafe { std::slice::from_raw_parts_mut(bits.cast::<u32>(), pixels) };
    fill_grid_pixels(buf, w, h, color);

    // 選入失敗就中止，不拿 DC 預設的 1×1 點陣圖去更新視窗。
    mem.select_dib()?;
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    let dst = POINT {
        x: work_area.x,
        y: work_area.y,
    };
    let size = SIZE { cx: w, cy: h };
    let src = POINT { x: 0, y: 0 };
    // SAFETY: hwnd 是本執行緒建立的 WS_EX_LAYERED 視窗；兩個 DC 在本函式內有效，記憶體 DC 已選入
    // 同尺寸的 premultiplied 32 bpp DIB（上一行 `select_dib` 成功才會到這裡）；所有指標都指向本函式
    // 的局部變數，只在呼叫期間使用。
    unsafe {
        UpdateLayeredWindow(
            hwnd,
            Some(screen.0),
            Some(&dst),
            Some(&size),
            Some(mem.dc),
            Some(&src),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        )
    }
    .map_err(|e| format!("UpdateLayeredWindow（{work_area:?}）失敗：{e}"))
}

/// 繪圖 DC 收尾時 DIB 的選入狀態（決定 [`cleanup_order`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectionState {
    /// DIB 從未選入（`SelectObject` 失敗或還沒選）。
    NotSelected,
    /// 已把原物件選回，DIB 已不在 DC 中。
    Restored,
    /// 選回原物件失敗，DIB 仍被 DC 選取。
    RestoreFailed,
}

/// 收尾的一步。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CleanupStep {
    DeleteDib,
    DeleteDc,
}

/// 收尾順序（純函式）：
/// - DIB 不在 DC 中（從未選入或已選回）→ 先 `DeleteObject(DIB)` 再 `DeleteDC`。
/// - 選回失敗、DIB 仍被選取 → **先 `DeleteDC` 再 `DeleteObject(DIB)`**：仍被 DC 選取的點陣圖
///   `DeleteObject` 會失敗（Microsoft Learn〈DeleteObject〉：「Do not delete a drawing object
///   (pen or brush) while it is still selected into a DC」；點陣圖同理，實務上回 FALSE），刪掉 DC
///   後點陣圖就不再被選取，才刪得掉。
///
/// 兩種順序都兩個把手各刪一次，任何路徑都不會遺失仍存活的 GDI 把手。
fn cleanup_order(state: SelectionState) -> [CleanupStep; 2] {
    match state {
        SelectionState::NotSelected | SelectionState::Restored => {
            [CleanupStep::DeleteDib, CleanupStep::DeleteDc]
        }
        SelectionState::RestoreFailed => [CleanupStep::DeleteDc, CleanupStep::DeleteDib],
    }
}

/// 螢幕 DC（`GetDC(NULL)`），丟棄時 `ReleaseDC`。
struct ScreenDc(HDC);

impl ScreenDc {
    fn get() -> Result<Self, String> {
        // SAFETY: None＝整個螢幕的 DC；失敗時回無效 HDC。
        let dc = unsafe { GetDC(None) };
        if dc.is_invalid() {
            Err("GetDC(NULL) 失敗".to_owned())
        } else {
            Ok(Self(dc))
        }
    }
}

impl Drop for ScreenDc {
    fn drop(&mut self) {
        // SAFETY: 這個 DC 由 `GetDC(None)` 取得、只釋放一次。回 1＝已釋放、0＝失敗。
        if unsafe { ReleaseDC(None, self.0) } != 1 {
            log::warn!("ReleaseDC（螢幕 DC）失敗");
        }
    }
}

/// 繪圖用的記憶體 DC＋DIB section＋選入狀態，三者由同一個擁有者收尾（`Drop`），收尾順序依實際的
/// 選入結果由 [`cleanup_order`] 決定；每一個 `SelectObject`／`DeleteObject`／`DeleteDC` 的失敗都
/// 記 warn。宣告在 `ScreenDc` 之後，所以先於螢幕 DC 被丟棄。
struct PaintDc {
    dc: HDC,
    /// `CreateDIBSection` 成功後才有。
    dib: Option<HBITMAP>,
    /// DIB 選入成功時換出的原物件（`Some`＝DIB 目前被 `dc` 選取）。
    previous: Option<HGDIOBJ>,
}

impl PaintDc {
    fn create(screen: &ScreenDc) -> Result<Self, String> {
        // SAFETY: screen.0 是有效的螢幕 DC；失敗時回無效 HDC。
        let dc = unsafe { CreateCompatibleDC(Some(screen.0)) };
        if dc.is_invalid() {
            Err("CreateCompatibleDC 失敗".to_owned())
        } else {
            Ok(Self {
                dc,
                dib: None,
                previous: None,
            })
        }
    }

    /// 建立 `bmi` 描述的 DIB section 交給本物件擁有，回傳像素記憶體位址（非空）。
    fn create_dib(&mut self, bmi: &BITMAPINFO) -> Result<*mut core::ffi::c_void, String> {
        debug_assert!(self.dib.is_none(), "每個 PaintDc 只建一張 DIB");
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        // SAFETY: `bmi` 由呼叫端提供、在呼叫期間有效；`bits` 是本函式的局部可寫指標，成功時由系統
        // 寫入像素記憶體位址。失敗時 windows crate 回 Err（不會給無效 HBITMAP）。
        let dib =
            unsafe { CreateDIBSection(Some(self.dc), bmi, DIB_RGB_COLORS, &mut bits, None, 0) }
                .map_err(|e| format!("CreateDIBSection 失敗：{e}"))?;
        // 先交給本物件擁有，之後任何錯誤路徑都由 `Drop` 刪除。
        self.dib = Some(dib);
        if bits.is_null() {
            return Err("CreateDIBSection 沒有回傳像素記憶體".to_owned());
        }
        Ok(bits)
    }

    /// 把 DIB 選入 DC。`SelectObject` 回 NULL（或 HGDI_ERROR）＝失敗：DIB 沒有被選取，回錯誤，
    /// 呼叫端不得再以這個 DC 畫圖（否則用到的是 DC 預設的 1×1 點陣圖）。
    fn select_dib(&mut self) -> Result<(), String> {
        let dib = self.dib.ok_or_else(|| "尚未建立 DIB".to_owned())?;
        // SAFETY: self.dc 是有效的記憶體 DC，dib 是本物件擁有、尚未選入任何 DC 的 DIB section。
        let previous = unsafe { SelectObject(self.dc, HGDIOBJ(dib.0)) };
        if previous.is_invalid() {
            return Err("SelectObject（選入 DIB）失敗".to_owned());
        }
        self.previous = Some(previous);
        Ok(())
    }

    /// 選回原物件，回報實際的選入狀態（給 [`cleanup_order`]）。
    fn restore_selection(&mut self) -> SelectionState {
        let Some(previous) = self.previous.take() else {
            return SelectionState::NotSelected;
        };
        // SAFETY: 把 `select_dib` 時換出的原物件選回同一個（仍有效的）DC。
        let swapped_out = unsafe { SelectObject(self.dc, previous) };
        if swapped_out.is_invalid() {
            log::warn!("SelectObject（選回原物件）失敗，改為先刪記憶體 DC 再刪 DIB");
            SelectionState::RestoreFailed
        } else {
            SelectionState::Restored
        }
    }
}

impl Drop for PaintDc {
    fn drop(&mut self) {
        let state = self.restore_selection();
        for step in cleanup_order(state) {
            match step {
                CleanupStep::DeleteDib => {
                    if let Some(dib) = self.dib.take() {
                        // SAFETY: 這個點陣圖由 `create_dib` 建立、只刪除一次（take 之後不再持有）。依
                        // `cleanup_order`，執行到這裡時它已不被任何 DC 選取（已選回、從未選入，或 DC
                        // 已先刪除）。
                        if !unsafe { DeleteObject(HGDIOBJ(dib.0)) }.as_bool() {
                            log::warn!("DeleteObject（格線 DIB，選入狀態 {state:?}）失敗");
                        }
                    }
                }
                CleanupStep::DeleteDc => {
                    // SAFETY: 這個 DC 由 `CreateCompatibleDC` 建立、只在這裡刪除一次（`cleanup_order`
                    // 每種狀態都恰好含一次 DeleteDc）。
                    if !unsafe { DeleteDC(self.dc) }.as_bool() {
                        log::warn!("DeleteDC（格線記憶體 DC，選入狀態 {state:?}）失敗");
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha_of(px: u32) -> u8 {
        (px >> 24) as u8
    }

    fn render(width: i32, height: i32, color: Rgb) -> Vec<u32> {
        let mut buf = vec![0xDEAD_BEEF_u32; (width * height) as usize];
        fill_grid_pixels(&mut buf, width, height, color);
        buf
    }

    /// task 5.2：`host/tools/` 的驗收腳本大多以標題 `fc-host *`（PowerShell `-like`，`fc-host` 後接一個
    /// 空白）認小工具視窗（verify-3.1／3.3／3.4／3.5／5.6／6.1 等）；格線的標題若也符合，編輯版面期間就會
    /// 被誤數成小工具。標題改成不含「`fc-host` 後接空白」的形式，類別名稱同樣避開。
    #[test]
    fn title_and_class_do_not_look_like_a_widget_window() {
        // SAFETY: 兩者都是 `w!` 產生的靜態、以 NUL 結尾的 UTF-16 字串。
        let (title, class) = unsafe {
            (
                WINDOW_TITLE.to_string().expect("UTF-16"),
                CLASS_NAME.to_string().expect("UTF-16"),
            )
        };
        for name in [&title, &class] {
            assert!(!name.starts_with("fc-host "), "{name}");
            assert_ne!(name, "Tauri Window");
        }
        assert_eq!(title, "fc-host-grid-overlay");
    }

    #[test]
    fn parses_hex_accent_case_insensitively() {
        assert_eq!(
            parse_accent_color("#e0aa54"),
            Some(Rgb {
                r: 0xe0,
                g: 0xaa,
                b: 0x54
            })
        );
        assert_eq!(
            parse_accent_color("#0A0b0C"),
            Some(Rgb {
                r: 0x0a,
                g: 0x0b,
                b: 0x0c
            })
        );
    }

    #[test]
    fn rejects_malformed_accent() {
        for bad in [
            "",
            "#",
            "e0aa54",
            "#e0aa5",
            "#e0aa545",
            "#e0aa5g",
            "#+0aa54",
            "#ｅ0aa54",
            " #e0aa54",
        ] {
            assert_eq!(parse_accent_color(bad), None, "{bad:?} 應解析失敗");
        }
    }

    #[test]
    fn malformed_accent_falls_back_to_default() {
        assert_eq!(accent_or_default("not a color"), DEFAULT_ACCENT);
        assert_eq!(accent_or_default("#010203"), Rgb { r: 1, g: 2, b: 3 });
        assert_eq!(DEFAULT_ACCENT, parse_accent_color("#e0aa54").unwrap());
    }

    #[test]
    fn every_sixth_line_is_major() {
        let majors: Vec<usize> = (1..=47).filter(|&i| line_alpha(i) == MAJOR_ALPHA).collect();
        assert_eq!(majors, vec![6, 12, 18, 24, 30, 36, 42]);
        assert!((1..=47)
            .filter(|i| i % MAJOR_EVERY != 0)
            .all(|i| line_alpha(i) == MINOR_ALPHA));
    }

    #[test]
    fn premultiply_scales_channels_by_alpha_with_rounding() {
        let white = Rgb {
            r: 255,
            g: 255,
            b: 255,
        };
        assert_eq!(premultiplied_pixel(white, 0xff), 0xffff_ffff);
        assert_eq!(premultiplied_pixel(white, 0x30), 0x3030_3030);
        assert_eq!(premultiplied_pixel(DEFAULT_ACCENT, 0), 0);
        // 0xe0×0x70／255＝98.37→98（0x62）；0xaa×0x70／255＝74.67→75（0x4b）；
        // 0x54×0x70／255＝36.89→37（0x25）。BGRA 記憶體順序＝u32 的 0xAARRGGBB。
        assert_eq!(premultiplied_pixel(DEFAULT_ACCENT, 0x70), 0x7062_4b25);
        // 任一色版都不得超過 alpha（premultiplied 的不變式）。
        for a in [1u8, 0x30, 0x70, 0xfe] {
            let px = premultiplied_pixel(white, a);
            for shift in [0, 8, 16] {
                assert!(((px >> shift) & 0xff) as u8 <= a);
            }
        }
    }

    #[test]
    fn draws_47_vertical_and_47_horizontal_lines_at_grid_offsets() {
        let (w, h) = (960, 528);
        let buf = render(w, h, DEFAULT_ACCENT);
        let xs = grid_line_offsets(w);
        let ys = grid_line_offsets(h);
        // 取一列不在任何水平線上的像素列：非零的 x 恰好是 47 條垂直線。
        let free_row = (0..h).find(|y| !ys.contains(y)).unwrap();
        let lit_x: Vec<i32> = (0..w)
            .filter(|&x| buf[(free_row * w + x) as usize] != 0)
            .collect();
        assert_eq!(lit_x, xs.to_vec());
        let free_col = (0..w).find(|x| !xs.contains(x)).unwrap();
        let lit_y: Vec<i32> = (0..h)
            .filter(|&y| buf[(y * w + free_col) as usize] != 0)
            .collect();
        assert_eq!(lit_y, ys.to_vec());
    }

    #[test]
    fn line_pixels_use_major_or_minor_alpha_and_accent_color() {
        let (w, h) = (960, 528);
        let buf = render(w, h, DEFAULT_ACCENT);
        let xs = grid_line_offsets(w);
        let ys = grid_line_offsets(h);
        let free_row = (0..h).find(|y| !ys.contains(y)).unwrap();
        for (k, &x) in xs.iter().enumerate() {
            let px = buf[(free_row * w + x) as usize];
            let expected = premultiplied_pixel(DEFAULT_ACCENT, line_alpha(k + 1));
            assert_eq!(px, expected, "垂直線 {}（x＝{x}）", k + 1);
        }
        let free_col = (0..w).find(|x| !xs.contains(x)).unwrap();
        for (k, &y) in ys.iter().enumerate() {
            let px = buf[(y * w + free_col) as usize];
            assert_eq!(
                alpha_of(px),
                line_alpha(k + 1),
                "水平線 {}（y＝{y}）",
                k + 1
            );
        }
    }

    #[test]
    fn intersections_take_the_stronger_alpha() {
        let (w, h) = (960, 528);
        let buf = render(w, h, DEFAULT_ACCENT);
        let xs = grid_line_offsets(w);
        let ys = grid_line_offsets(h);
        // 垂直加強線（第 6 條）× 水平一般線（第 1 條）→ 加強。
        assert_eq!(alpha_of(buf[(ys[0] * w + xs[5]) as usize]), MAJOR_ALPHA);
        // 垂直一般線（第 1 條）× 水平加強線（第 12 條）→ 加強。
        assert_eq!(alpha_of(buf[(ys[11] * w + xs[0]) as usize]), MAJOR_ALPHA);
        // 一般 × 一般 → 一般。
        assert_eq!(alpha_of(buf[(ys[0] * w + xs[0]) as usize]), MINOR_ALPHA);
    }

    #[test]
    fn work_area_border_is_not_drawn_and_background_is_fully_transparent() {
        let (w, h) = (960, 528);
        let buf = render(w, h, DEFAULT_ACCENT);
        let xs = grid_line_offsets(w);
        let ys = grid_line_offsets(h);
        for x in 0..w {
            if !xs.contains(&x) {
                assert_eq!(buf[x as usize], 0, "上緣 x＝{x}");
                assert_eq!(buf[((h - 1) * w + x) as usize], 0, "下緣 x＝{x}");
            }
        }
        for y in 0..h {
            if !ys.contains(&y) {
                assert_eq!(buf[(y * w) as usize], 0, "左緣 y＝{y}");
                assert_eq!(buf[(y * w + w - 1) as usize], 0, "右緣 y＝{y}");
            }
        }
        // 背景（不在任何線上）全為 0（alpha 0＝完全透明），且原本的垃圾值已被清掉。
        let lit = buf.iter().filter(|&&p| p != 0).count();
        let expected = 47 * h as usize + 47 * w as usize - 47 * 47;
        assert_eq!(lit, expected);
    }

    #[test]
    fn tiny_extent_never_draws_on_the_border() {
        // 寬 30（< 48）時前幾條線的偏移為 0＝左外框位置，不得畫。
        // x＝0 那一欄只有被水平線（y > 0）穿過的點會亮，本身不是一條垂直線；y＝0 那一列同理。
        let (w, h) = (30, 20);
        let buf = render(w, h, DEFAULT_ACCENT);
        let xs = grid_line_offsets(w);
        let ys = grid_line_offsets(h);
        assert!(xs.contains(&0) && ys.contains(&0), "前提：偏移含 0");
        for y in 0..h {
            let crossed = y > 0 && ys.contains(&y);
            assert_eq!(buf[(y * w) as usize] != 0, crossed, "x＝0, y＝{y}");
        }
        for x in 0..w {
            let crossed = x > 0 && xs.contains(&x);
            assert_eq!(buf[x as usize] != 0, crossed, "y＝0, x＝{x}");
        }
    }

    #[test]
    fn cleanup_deletes_dib_before_dc_when_dib_is_not_selected() {
        for state in [SelectionState::NotSelected, SelectionState::Restored] {
            assert_eq!(
                cleanup_order(state),
                [CleanupStep::DeleteDib, CleanupStep::DeleteDc],
                "{state:?}"
            );
        }
    }

    #[test]
    fn cleanup_deletes_dc_first_when_restoring_the_original_object_failed() {
        assert_eq!(
            cleanup_order(SelectionState::RestoreFailed),
            [CleanupStep::DeleteDc, CleanupStep::DeleteDib]
        );
    }

    #[test]
    fn create_rejects_empty_work_area_without_creating_a_window() {
        let rect = PhysicalRect {
            x: 0,
            y: 0,
            width: 0,
            height: 100,
        };
        assert!(GridOverlay::create(rect, DEFAULT_ACCENT).is_err());
    }

    /// 實機冒煙：在主螢幕工作區內建立一個 800×600 的格線視窗（約 2 秒、滑鼠穿透），驗延伸樣式、
    /// 矩形、穿透、不搶前景、`update` 的重畫判定與搬移，最後銷毀。不注入任何輸入、不改設定。
    /// 先確認工作階段未鎖定再跑：
    /// `cargo test --bin fc-host -- --ignored grid_overlay::tests::real_overlay_smoke --nocapture`
    #[test]
    #[ignore = "實機：會在主螢幕短暫顯示格線"]
    fn real_overlay_smoke() {
        use windows::Win32::Foundation::RECT;
        use windows::Win32::UI::HiDpi::{
            SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        };
        use windows::Win32::UI::WindowsAndMessaging::{
            GetClassNameW, GetForegroundWindow, GetWindow, GetWindowLongPtrW, GetWindowRect,
            IsWindow, IsWindowVisible, SendMessageW, SystemParametersInfoW, WindowFromPoint,
            GWL_EXSTYLE, GWL_STYLE, GW_HWNDNEXT, GW_HWNDPREV, HWND_NOTOPMOST, HWND_TOPMOST,
            SPI_GETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WS_EX_APPWINDOW, WS_EX_TOPMOST,
            WS_VISIBLE,
        };

        fn rect_of(hwnd: HWND) -> PhysicalRect {
            let mut r = RECT::default();
            // SAFETY: hwnd 是本測試建立、仍存活的視窗；r 是局部可寫變數。
            unsafe { GetWindowRect(hwnd, &mut r) }.expect("GetWindowRect");
            PhysicalRect {
                x: r.left,
                y: r.top,
                width: r.right - r.left,
                height: r.bottom - r.top,
            }
        }

        // 測試執行檔沒有宿主的 manifest（預設 DPI unaware），比照宿主改成 per-monitor v2，
        // 座標才是實體像素。
        // SAFETY: 只影響本測試執行緒。
        let previous =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        println!(
            "SetThreadDpiAwarenessContext 前值非空＝{}",
            !previous.is_invalid()
        );

        let mut wa = RECT::default();
        // SAFETY: SPI_GETWORKAREA 寫入一個 RECT；不帶更新旗標、不改任何系統設定。
        unsafe {
            SystemParametersInfoW(
                SPI_GETWORKAREA,
                0,
                Some((&mut wa as *mut RECT).cast()),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            )
        }
        .expect("SPI_GETWORKAREA");
        println!("主螢幕工作區 {wa:?}");
        let rect = PhysicalRect {
            x: wa.left + 100,
            y: wa.top + 100,
            width: 800,
            height: 600,
        };

        // SAFETY: 無參數。
        let fg_before = unsafe { GetForegroundWindow() };
        let mut overlay = GridOverlay::create(rect, DEFAULT_ACCENT).expect("建立格線視窗");
        let hwnd = overlay.hwnd();
        // SAFETY: 同上。
        let fg_after = unsafe { GetForegroundWindow() };

        // SAFETY: hwnd 是本測試剛建立、仍存活的視窗。
        let ex = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
        // SAFETY: 同上。
        let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32;
        println!(
            "GWL_EXSTYLE＝0x{ex:08x}（期望含 0x{OVERLAY_EX_STYLE:08x}）；GWL_STYLE＝0x{style:08x}"
        );
        let actual = rect_of(hwnd);
        println!("GetWindowRect＝{actual:?}（期望 {rect:?}）");

        let xs = grid_line_offsets(rect.width);
        let ys = grid_line_offsets(rect.height);
        let probes = [
            POINT {
                x: rect.x + rect.width / 2 + 3,
                y: rect.y + rect.height / 2 + 3,
            },
            // 落在加強線交點上（有不透明像素的位置）。
            POINT {
                x: rect.x + xs[5],
                y: rect.y + ys[5],
            },
        ];
        // 置底時上方的一般視窗本來就會擋住探測點，WindowFromPoint 沒命中證明不了穿透；探測期間
        // 暫時把格線（不啟用地）設成 topmost，探完取消 topmost 並放回最底。只改本測試視窗的 z-order。
        // 不用 HWND_TOP：實測背景行程的 HWND_TOP 回成功卻沒有提到前景程式之上（前景鎖定）。
        let class_of = |h: HWND| {
            let mut name = [0u16; 128];
            // SAFETY: name 是局部緩衝；h 可能為空，API 回 0。
            let n = unsafe { GetClassNameW(h, &mut name) };
            String::from_utf16_lossy(&name[..usize::try_from(n).unwrap_or(0)])
        };
        let restack = |after: HWND| {
            // SAFETY: hwnd 是本測試建立、仍存活的視窗；NOMOVE／NOSIZE／NOACTIVATE 只改 z-order。
            unsafe {
                SetWindowPos(
                    hwnd,
                    Some(after),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
                )
            }
            .expect("SetWindowPos（測試調整 z-order）");
        };
        restack(HWND_TOPMOST);
        // 探測前提：格線上方沒有「可見、非 topmost、矩形蓋到探測點」的視窗（隱藏的 IME 之類不算）。
        let mut covering = Vec::new();
        // SAFETY: 只查詢 z-order 鄰居。
        let mut above = unsafe { GetWindow(hwnd, GW_HWNDPREV) }.unwrap_or_default();
        while !above.is_invalid() {
            // SAFETY: 只查詢樣式、可見性與矩形；視窗剛好消失時回 0／錯誤，略過即可。
            let above_ex = unsafe { GetWindowLongPtrW(above, GWL_EXSTYLE) } as u32;
            let visible = unsafe { IsWindowVisible(above) }.as_bool();
            let mut r = RECT::default();
            let has_rect = unsafe { GetWindowRect(above, &mut r) }.is_ok();
            let covers = has_rect
                && probes
                    .iter()
                    .any(|p| p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom);
            if visible && above_ex & WS_EX_TOPMOST.0 == 0 && covers {
                covering.push(format!("{:?}「{}」", above.0, class_of(above)));
            }
            // SAFETY: 同上。
            above = unsafe { GetWindow(above, GW_HWNDPREV) }.unwrap_or_default();
        }
        println!("探測期間蓋住探測點的一般視窗：{covering:?}（空＝格線在最上層）");
        let nothing_covering = covering.is_empty();
        let lparam = LPARAM(((probes[1].y as u16 as isize) << 16) | (probes[1].x as u16 as isize));
        // SAFETY: 同執行緒的視窗，SendMessageW 直接呼叫視窗程序；只查詢命中區域。
        let nchit = unsafe { SendMessageW(hwnd, WM_NCHITTEST, None, Some(lparam)) };
        println!(
            "WM_NCHITTEST 回傳 {}（HTTRANSPARENT＝{HTTRANSPARENT}）",
            nchit.0
        );
        let mut hits = Vec::new();
        for p in probes {
            // SAFETY: 只查詢、不送輸入。
            let under = unsafe { WindowFromPoint(p) };
            println!(
                "WindowFromPoint({}, {})＝{:?}（類別「{}」）",
                p.x,
                p.y,
                under.0,
                class_of(under)
            );
            hits.push(under);
        }
        restack(HWND_NOTOPMOST);
        restack(HWND_BOTTOM);
        // SAFETY: 只查詢樣式。
        let ex_after_probe = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
        // SAFETY: 只查詢 z-order 鄰居。
        let next = unsafe { GetWindow(hwnd, GW_HWNDNEXT) }.unwrap_or_default();
        println!(
            "放回最底後 z-order 下一個視窗＝{:?}（類別「{}」）",
            next.0,
            class_of(next)
        );

        std::thread::sleep(std::time::Duration::from_millis(1500));

        let same = overlay
            .update(rect, DEFAULT_ACCENT)
            .expect("update（不變）");
        let recolored = overlay
            .update(
                rect,
                Rgb {
                    r: 0x40,
                    g: 0xa0,
                    b: 0xff,
                },
            )
            .expect("update（換色）");
        let moved_rect = PhysicalRect {
            x: rect.x + 50,
            y: rect.y + 40,
            width: 640,
            height: 480,
        };
        let moved = overlay
            .update(moved_rect, overlay.color())
            .expect("update（搬移）");
        let after_move = rect_of(hwnd);
        println!("update：不變＝{same}、換色＝{recolored}、搬移＝{moved} → {after_move:?}");
        std::thread::sleep(std::time::Duration::from_millis(500));

        drop(overlay);
        // SAFETY: 只查詢把手是否仍指向視窗。
        let alive = unsafe { IsWindow(Some(hwnd)) }.as_bool();
        println!("銷毀後 IsWindow＝{alive}");

        assert_eq!(ex & OVERLAY_EX_STYLE, OVERLAY_EX_STYLE, "延伸樣式");
        assert_eq!(ex & WS_EX_APPWINDOW.0, 0, "不得有 WS_EX_APPWINDOW");
        assert_eq!(style & WS_POPUP.0, WS_POPUP.0, "WS_POPUP");
        assert_eq!(style & WS_VISIBLE.0, WS_VISIBLE.0, "已顯示");
        assert_eq!(actual, rect, "視窗矩形＝工作區");
        assert!(
            nothing_covering,
            "探測前提：探測期間格線上方不得有蓋住探測點的一般視窗"
        );
        assert_eq!(ex_after_probe & WS_EX_TOPMOST.0, 0, "探測後已取消 topmost");
        assert_eq!(
            nchit.0, HTTRANSPARENT as isize,
            "WM_NCHITTEST→HTTRANSPARENT"
        );
        assert!(
            hits.iter().all(|&h| h != hwnd),
            "WindowFromPoint 不得命中格線視窗"
        );
        assert_eq!(fg_before, fg_after, "建立格線不得改變前景視窗");
        assert!(!same && recolored && moved);
        assert_eq!(after_move, moved_rect, "update 搬移後的矩形");
        assert!(!alive, "Drop 後視窗應已銷毀");
    }

    #[test]
    fn invalid_or_mismatched_sizes_leave_buffer_untouched_without_panic() {
        let mut buf = vec![7u32; 10];
        fill_grid_pixels(&mut buf, 0, 10, DEFAULT_ACCENT);
        fill_grid_pixels(&mut buf, -5, 2, DEFAULT_ACCENT);
        fill_grid_pixels(&mut buf, 4, 4, DEFAULT_ACCENT); // 需要 16 個像素，只有 10 個
        assert_eq!(buf, vec![7u32; 10]);
    }
}
