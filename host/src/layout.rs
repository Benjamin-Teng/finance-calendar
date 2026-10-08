//! 小工具位置計算純函式：48×48 格線版面（design.md D7「格線版面與小工具尺寸」、D9「位置
//! 模型」；specs/widget-host-windows「版面格線」「小工具尺寸由版面格決定」「小工具互不重疊」
//! 「編輯版面」「多螢幕與 DPI 定位」「預設版面」）。
//!
//! 本模組只做資料轉換，**不呼叫 Win32、不依賴 tauri 型別**（全域約束；`crate::desktop` 負責
//! 把系統列舉到的顯示器資訊轉成 [`MonitorInfo`]，再呼叫這裡的函式）。主要入口：
//!
//! - [`resolve_grid_placements`]：記錄位置＋目前顯示器清單 → 每個小工具的實際格子、實體矩形
//!   與倍率（兩階段推導，D9），供視窗建立、多螢幕重排、`update_settings` 後重排呼叫
//!   （`crate::widgets`）。
//! - [`legal_move_placement`]（task 7.5）：編輯版面拖曳中／放開時的實體矩形 → 「若此刻放開」
//!   的新記錄位置（移動對齊、保留格數＋合法判斷，D7），不合法回傳 `None`；供
//!   `crate::widgets` 的紅框預告與 `finish_widget_drag` 呼叫。[`placement_after_move`] 是它
//!   不含合法判斷的前身（task 7.2），保留供測試與流程驗證使用。
//! - 其餘（碰撞、找空位、最小格數、合法判斷、調整大小對齊）供上述兩者與 task 7.4–7.6 使用。
//!
//! task 7.2 已移除舊的錨點模型（`resolve_placement_rect`／`plan_relayout`／
//! `placement_from_drag_end`／`snapped_placement_and_rect` 與其測試）。部分公開項目
//! （`align_resize`／`ResizeEdges`／`is_legal_grid_rect` 等）要到 task 7.5／7.6 才有呼叫端，
//! 以模組層級 `allow(dead_code)` 抑制，避免之後逐項標註。

#![allow(dead_code)]

use crate::settings::{MonitorId, WidgetPlacement};

/// 顯示器資訊，實體像素、虛擬桌面座標系（多螢幕時左／上方螢幕的座標可能為負）。由呼叫端
/// （`crate::desktop::monitor_infos_from_tauri_monitors`）從 `DisplayConfigGetDeviceInfo`／
/// `GetMonitorInfo` 等系統列舉轉換而來；本結構刻意不含任何 Win32 型別。
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorInfo {
    /// 穩定的顯示器識別（design.md D9：`monitorDevicePath`，**不含**解析度、**不用**
    /// `\\.\DISPLAYn`）。即使是主螢幕，這裡放的也是它的實際 `Device` 識別，不是
    /// `MonitorId::Primary`——`Primary` 是 [`WidgetPlacement::monitor`] 用來表示「動態跟隨當前
    /// 主螢幕」的哨兵值，不是某台顯示器自己的身分。
    ///
    /// `None`＝本次列舉**查不到**穩定識別（見 `desktop::monitors_from_tauri`）：這台顯示器
    /// 仍可作為主螢幕退回目標被定位（工作區、縮放照用），但**沒有可保存的身分**——
    /// [`resolve_grid_placements`] 不會把任何 `Device(_)` 記錄對應到它，[`placement_after_move`]
    /// 對它回傳 `None`（呼叫端保留原記錄、不落地）。刻意用 `Option` 而非以 `\\.\DISPLAYn`
    /// 暫代：DISPLAYn 會隨拔插重新編號，一旦被存進設定就可能在日後對到另一台螢幕
    /// （design.md D9；fix round 1，Codex 2.5）。
    pub id: Option<MonitorId>,
    /// 工作區（扣除工作列後的可用範圍），實體像素。
    pub work_area: PhysicalRect,
    /// 該顯示器的 DPI 縮放係數（例如 125% → 1.25）。
    pub scale_factor: f64,
    pub is_primary: bool,
}

/// 實體像素矩形，虛擬桌面座標系（左／上方螢幕的座標可能為負，故用 `i32` 而非無號整數）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhysicalRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// 縮放係數防護：非有限值（NaN、±∞）或 ≤ 0 一律視為 1.0（負縮放／零縮放沒有合理的幾何
/// 意義，退回原尺寸比產生負尺寸或零尺寸視窗安全）。
fn sane_scale(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// i64 → i32，超出範圍飽和。
fn saturate_i32(v: i64) -> i32 {
    v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

// ═══════════════════════════════════════════════════════════════════════════════════
// task 7.1：格線版面純函式（design.md D7「格線版面與小工具尺寸」、D9「位置模型」；
// specs/widget-host-windows「版面格線」「小工具尺寸由版面格決定」「小工具互不重疊」
// 「編輯版面」「多螢幕與 DPI 定位」「預設版面」）。
// ═══════════════════════════════════════════════════════════════════════════════════
/// 每個顯示器工作區在水平／垂直方向的格數（design.md D7；
/// specs/widget-host-windows「版面格線」）。
pub const GRID: i32 = 48;

/// 格線座標的小工具矩形：佔用 `[col, col+w) × [row, row+h)` 格（整數格，欄、列從 0 起算）。
/// 記錄位置 [`WidgetPlacement`] 的格線部分即此型別（`WidgetPlacement::grid_rect`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridRect {
    pub col: i32,
    pub row: i32,
    pub w: i32,
    pub h: i32,
}

impl GridRect {
    /// 右緣格線編號（`col + w`，飽和加法：不合法的極值不會溢位，fix F1 review 7.2 M1）。
    pub fn right(&self) -> i32 {
        self.col.saturating_add(self.w)
    }

    /// 下緣格線編號（`row + h`，飽和加法，同 [`GridRect::right`]）。
    pub fn bottom(&self) -> i32 {
        self.row.saturating_add(self.h)
    }
}

/// 編輯版面調整大小時，`WM_SIZING` 的 `wParam`（`WMSZ_*`）指出的被拖邊（design.md D7：
/// 「調整大小只對齊被拖曳的邊，其餘邊不動」）。刻意不依賴 Win32 的 `WMSZ_*` 常數——本模組
/// 不得呼叫 Win32、也不依賴其型別（全域約束），轉譯是呼叫端（`desktop` 模組）的職責。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResizeEdges {
    pub left: bool,
    pub right: bool,
    pub top: bool,
    pub bottom: bool,
}

impl ResizeEdges {
    pub const LEFT: Self = ResizeEdges {
        left: true,
        right: false,
        top: false,
        bottom: false,
    };
    pub const RIGHT: Self = ResizeEdges {
        left: false,
        right: true,
        top: false,
        bottom: false,
    };
    pub const TOP: Self = ResizeEdges {
        left: false,
        right: false,
        top: true,
        bottom: false,
    };
    pub const BOTTOM: Self = ResizeEdges {
        left: false,
        right: false,
        top: false,
        bottom: true,
    };
    pub const TOP_LEFT: Self = ResizeEdges {
        left: true,
        right: false,
        top: true,
        bottom: false,
    };
    pub const TOP_RIGHT: Self = ResizeEdges {
        left: false,
        right: true,
        top: true,
        bottom: false,
    };
    pub const BOTTOM_LEFT: Self = ResizeEdges {
        left: true,
        right: false,
        top: false,
        bottom: true,
    };
    pub const BOTTOM_RIGHT: Self = ResizeEdges {
        left: false,
        right: true,
        top: false,
        bottom: true,
    };
}

/// D9「實際位置推導」的單一小工具輸入：id（或註冊表索引，這裡用 `&'static str` 與
/// [`crate::settings::WIDGET_IDS`]／`widgets.rs` 的 `WidgetSpec::id` 一致）、記錄位置
/// （`monitor` + 記錄格子）、倍率設計框（[`ZoomBox`]，widget-adaptive-zoom-and-grid design.md
/// D1／D3：算倍率與最小格數）。
/// 記錄位置在 [`resolve_grid_placements`] 內永不被修改——本結構本身即使被呼叫端修改
/// （模擬「使用者放開後寫回新記錄」），也只是呼叫端拿新值再呼叫一次，純函式不持有狀態。
#[derive(Debug, Clone, PartialEq)]
pub struct GridWidgetInput {
    pub id: &'static str,
    pub monitor: MonitorId,
    pub record_rect: GridRect,
    pub zoom_box: ZoomBox,
}

/// [`resolve_grid_placements`] 的單一小工具輸出（design.md D9）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ResolvedWidgetPlacement {
    /// 已放置：`monitor_index` 是它在呼叫端傳入的 `monitors` 切片中的索引（不持有
    /// [`MonitorInfo`] 的複本，呼叫端已有那份清單）；`moved_from_elsewhere` 表示記錄的顯示器
    /// 與目前解析到的顯示器不是同一台（`Device` 找不到、退回主螢幕）——用於 D7「編輯版面」
    /// 合法性判斷「從別處換來者的記錄不算」，與這次是否剛好待在記錄格子本身無關。
    Placed {
        monitor_index: usize,
        rect: GridRect,
        physical_rect: PhysicalRect,
        zoom: f64,
        moved_from_elsewhere: bool,
    },
    /// 空間不足（含連最小格數都放不下），暫時隱藏、不佔任何格子（design.md D9；
    /// specs/widget-host-windows「小工具互不重疊」「多螢幕與 DPI 定位」）。
    HiddenNoSpace,
}

/// D7 格線公式：`edge(i) = 工作區起點 + floor(i × 工作區實體長度 ÷ 48)`。`origin`／`extent`
/// 是同一軸（x 或 y）的工作區起點與實體長度；`extent < 0`（不合法輸入）視為 0。乘法先在 i64
/// 進行避免中間溢位，結果飽和回 i32（沿用 [`saturate_i32`]）。`i` 理論上落在 `0..=GRID`，但
/// 本函式對任意 `i`（含負值、超出 48）都給出線性外插的一致結果，不另外檢查範圍——範圍檢查是
/// [`in_grid_bounds`] 的職責。
pub fn edge(origin: i32, extent: i32, i: i32) -> i32 {
    let scaled = i64::from(i) * i64::from(extent.max(0)) / i64::from(GRID);
    origin.saturating_add(saturate_i32(scaled))
}

/// 工作區內部 47 條格線（i＝1..=47）相對於工作區起點的實體像素偏移，即 `edge(0, extent, i)`。
/// 與吸附（[`edge`]、[`grid_rect_to_physical`]）同源，編輯版面畫格線時加上工作區原點即得座標
/// （widget-adaptive-zoom-and-grid design.md D4）。邊界線 0 與 48 不含在內。
pub fn grid_line_offsets(extent: i32) -> [i32; 47] {
    std::array::from_fn(|k| edge(0, extent, k as i32 + 1))
}

/// [`GridRect`]（格線座標）→ 實體像素矩形，以 `work_area` 為基準（D7）。寬高由左右／上下
/// 兩條格線的實際像素差算出，因此共用同一條格線的兩個小工具，該邊必落在同一像素上——不需要
/// 額外的「共邊」特判。
pub fn grid_rect_to_physical(work_area: PhysicalRect, rect: GridRect) -> PhysicalRect {
    let x0 = edge(work_area.x, work_area.width, rect.col);
    let x1 = edge(work_area.x, work_area.width, rect.right());
    let y0 = edge(work_area.y, work_area.height, rect.row);
    let y1 = edge(work_area.y, work_area.height, rect.bottom());
    PhysicalRect {
        x: x0,
        y: y0,
        width: x1.saturating_sub(x0).max(0),
        height: y1.saturating_sub(y0).max(0),
    }
}

/// 實體像素位置反推最近的格線編號：`round((physical − origin) × 48 ÷ extent)`，夾在
/// `[0, GRID]`（design.md D7「對齊」；「round((edge(i) − 起點) × 48 / 長度) = i」的反函式）。
/// `extent <= 0`（不合法輸入）沒有可換算的格線比例，一律回傳 0，不除以零。
pub fn nearest_grid_index(origin: i32, extent: i32, physical: i32) -> i32 {
    grid_index_unclamped(origin, extent, physical).clamp(0, GRID)
}

/// [`nearest_grid_index`] 不夾範圍的版本（task 7.5 fix round 1）：`round((physical − origin)
/// × 48 ÷ extent)`，位置在工作區外時可為負或大於 48，供編輯版面合法判斷辨識「超出工作區」。
/// `extent <= 0` 回傳 0；結果飽和在 i32 範圍。
pub fn grid_index_unclamped(origin: i32, extent: i32, physical: i32) -> i32 {
    if extent <= 0 {
        return 0;
    }
    let offset = i64::from(physical) - i64::from(origin);
    let idx = (offset as f64 * f64::from(GRID) / f64::from(extent)).round();
    if idx.is_finite() {
        saturate_i32(idx as i64)
    } else {
        0
    }
}

/// 小工具的倍率設計框（widget-adaptive-zoom-and-grid design.md D1；specs/widget-host-windows
/// 「小工具尺寸由版面格決定」）。長度一律是邏輯（CSS）像素，高度含上下兩個
/// `crate::widgets::WIDGET_GAP_CSS_PX`。不變式 `comfort ≥ min`（各軸）由
/// `crate::widgets` 的測試守住，本模組不假設、也不檢查。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZoomBox {
    /// 最小框寬：倍率 1 時內容剛好塞得下的邏輯寬。決定上限（`comfort_width` 有值時）與最小
    /// 格數的寬度條件（[`meets_min_grid_size`]：邏輯寬 ≥ `min_width × 0.5`）。
    pub min_width: f64,
    /// 最小框高：倍率 1 時內容剛好塞得下的邏輯高。決定上限與最小格數的高度條件。
    pub min_height: f64,
    /// 舒適框寬；`None`＝寬度不限制倍率（橫向跑馬燈的行情條），此時上限也不看寬。
    pub comfort_width: Option<f64>,
    /// 舒適框高：自適應倍率的高度目標。
    pub comfort_height: f64,
}

/// 內容倍率（widget-adaptive-zoom-and-grid design.md D1，取代舊 change D7 只看寬度的
/// `grid_zoom`）：
///
/// ```text
/// 邏輯寬 = 實體寬 ÷ 縮放比例；邏輯高同理
/// auto = min(邏輯寬 ÷ comfort_width（有才算）, 邏輯高 ÷ comfort_height)
/// cap  = min(邏輯寬 ÷ min_width（comfort_width 有才算）, 邏輯高 ÷ min_height)
/// zoom = clamp(min(auto × font_scale, cap), 0.5, 3.0)
/// ```
///
/// 因此矩形不小於最小格數（`cap ≥ 0.5`）時倍率不超過上限，內容不會因倍率而超出矩形（測試
/// `content_zoom_stays_in_range_and_never_exceeds_cap_when_rect_meets_min_size`）。非法輸入
/// 不 panic：縮放比例沿用 [`sane_scale`]、框的各長度與 `font_scale` 沿用
/// [`sane_positive_len`]（非有限值或 ≤ 0 視為 1.0；`font_scale` 視為 1.0 即「字級 100%」）；
/// 實體寬高 ≤ 0 時算出的比例 ≤ 0，被夾到 0.5。
pub fn content_zoom(
    physical_width: i32,
    physical_height: i32,
    scale_factor: f64,
    zoom_box: &ZoomBox,
    font_scale: f64,
) -> f64 {
    let scale = sane_scale(scale_factor);
    let logical_width = f64::from(physical_width) / scale;
    let logical_height = f64::from(physical_height) / scale;
    let height_auto = logical_height / sane_positive_len(zoom_box.comfort_height);
    let height_cap = logical_height / sane_positive_len(zoom_box.min_height);
    let (auto, cap) = match zoom_box.comfort_width {
        Some(comfort_width) => (
            (logical_width / sane_positive_len(comfort_width)).min(height_auto),
            (logical_width / sane_positive_len(zoom_box.min_width)).min(height_cap),
        ),
        None => (height_auto, height_cap),
    };
    (auto * sane_positive_len(font_scale))
        .min(cap)
        .clamp(0.5, 3.0)
}

/// 正數長度防護：非有限值或 ≤ 0 視為 1.0（與 [`sane_scale`] 同精神，用於 [`ZoomBox`] 各長度
/// 與 `font_scale` 這類必須為正的輸入）。
fn sane_positive_len(v: f64) -> f64 {
    if v.is_finite() && v > 0.0 {
        v
    } else {
        1.0
    }
}

/// 碰撞判斷（design.md D7）：兩個格子矩形是否相交。共用同一條格線（邊界相接、不重疊）判為
/// 不相交。
pub fn rects_overlap(a: GridRect, b: GridRect) -> bool {
    a.col < b.right() && b.col < a.right() && a.row < b.bottom() && b.row < a.bottom()
}

/// 範圍檢查：`w`、`h` 至少 1 格，且完全落在 `0..=GRID` 內（specs/widget-host-windows
/// 「小工具互不重疊」：「全部落在 0 到 48 的格線範圍內」）。右／下緣用飽和加法（[`GridRect::right`]），
/// `col、row ≥ 0` 且 `w、h ≥ 1` 時真實和一定 ≥ 飽和值，溢位的極值必然 > 48 而判為不合法。
pub fn in_grid_bounds(rect: GridRect) -> bool {
    rect.w >= 1
        && rect.h >= 1
        && rect.col >= 0
        && rect.row >= 0
        && rect.right() <= GRID
        && rect.bottom() <= GRID
}

/// 最小格數的邏輯長度門檻（widget-adaptive-zoom-and-grid design.md D3）：最小框各軸的一半
/// （倍率 0.5 時塞得下）。寬度一律看 `min_width`，與 `comfort_width` 是否限制倍率無關（行情條
/// 的 min 寬只用於最小格數，D1 表格）；與字級設定無關（規格「字級設定不影響最小格數」）。
/// 框的長度經 [`sane_positive_len`] 防護。
fn min_size_thresholds(zoom_box: &ZoomBox) -> (f64, f64) {
    (
        sane_positive_len(zoom_box.min_width) * 0.5,
        sane_positive_len(zoom_box.min_height) * 0.5,
    )
}

/// 最小格數（widget-adaptive-zoom-and-grid design.md D3，依顯示器當下工作區計算，非常數）：
///
/// ```text
/// min_w = 第一個 w 使 邏輯寬(w) ≥ min_width × 0.5
/// min_h = 第一個 h 使 邏輯高(h) ≥ min_height × 0.5
/// ```
///
/// 舊 change D7 的高度條件是「設計最小高 × 依寬度算出的 zoom」；新倍率已被高度上限壓住
/// （[`content_zoom`]），兩軸不再耦合。兩者對 `w`／`h` 都是單調函式（`edge` 對格數單調不減），
/// 故由 1 往上找到第一個滿足者即為最小值；若直到 `GRID` 仍不滿足（螢幕物理上小到不可能達到
/// 0.5 倍），退回 `GRID`——不代表保證合法，只代表「這台顯示器能給的最大格數」，legality 仍由
/// [`meets_min_grid_size`] 判斷。
pub fn min_grid_size(work_area: PhysicalRect, scale_factor: f64, zoom_box: &ZoomBox) -> (i32, i32) {
    let scale = sane_scale(scale_factor);
    let (half_w, half_h) = min_size_thresholds(zoom_box);
    let first_reaching = |extent: i32, threshold: f64| {
        let extent = extent.max(0);
        (1..=GRID)
            .find(|&n| f64::from(edge(0, extent, n)) / scale >= threshold)
            .unwrap_or(GRID)
    };
    (
        first_reaching(work_area.width, half_w),
        first_reaching(work_area.height, half_h),
    )
}

/// 放開合法判斷的「大小」半條（widget-adaptive-zoom-and-grid design.md D3；
/// specs/widget-host-windows「小工具尺寸由版面格決定」最小格數段）：矩形換算成邏輯尺寸後，
/// 邏輯寬 ≥ `min_width × 0.5` 且邏輯高 ≥ `min_height × 0.5`——與 [`min_grid_size`] 同一組門檻，
/// 但這裡是對「任意給定矩形」直接判斷，不用逐格搜尋（`grid_tests::
/// min_grid_size_result_itself_meets_min_grid_size` 驗證兩者自洽）。範圍（`in_grid_bounds`）與
/// 碰撞（`rects_overlap`）不在這裡檢查，屬 [`is_legal_grid_rect`] 的其他兩個條件。
///
/// D3 與舊條件的關係：對 `min_height ≤ 舊設計最小高` 的小工具不比舊條件嚴；時鐘（min 高 160 >
/// 舊 156）在舊倍率約 0.5–0.513 的窄帶內舊合法、新不合法（`grid_tests::
/// clock_old_legal_new_illegal_narrow_band_is_the_documented_d3_exception`）。
pub fn meets_min_grid_size(
    work_area: PhysicalRect,
    rect: GridRect,
    scale_factor: f64,
    zoom_box: &ZoomBox,
) -> bool {
    let phys = grid_rect_to_physical(work_area, rect);
    let scale = sane_scale(scale_factor);
    let (half_w, half_h) = min_size_thresholds(zoom_box);
    let logical_width = f64::from(phys.width) / scale;
    let logical_height = f64::from(phys.height) / scale;
    // 極小的浮點誤差容許（1e-9）：min_grid_size 逐格找出的臨界值，套回這裡應剛好判定合法，
    // 不應因為運算順序不同而在邊界上出現 false negative。
    const EPS: f64 = 1e-9;
    logical_width + EPS >= half_w && logical_height + EPS >= half_h
}

/// 放開合法判斷（design.md D7；specs/widget-host-windows「編輯版面」「小工具互不重疊」）：
/// 範圍內、與 `others`（呼叫端決定要比對的對象——實際位置或記錄位置，見兩處規格文字的差異）
/// 兩兩不相交、且不小於最小格數（[`meets_min_grid_size`]）。
pub fn is_legal_grid_rect(
    work_area: PhysicalRect,
    rect: GridRect,
    others: &[GridRect],
    scale_factor: f64,
    zoom_box: &ZoomBox,
) -> bool {
    in_grid_bounds(rect)
        && !others.iter().any(|&other| rects_overlap(rect, other))
        && meets_min_grid_size(work_area, rect, scale_factor, zoom_box)
}

/// 兩點間「點到矩形最近距離」的平方（`extent <= 0` 的軸視為單一點 `origin`），供
/// [`monitor_index_for_point`] 在沒有任何工作區包含該點時取最近者用。用 `saturating` 運算避免
/// 極端座標下的整數溢位。
fn distance_sq_to_rect(rect: PhysicalRect, x: i32, y: i32) -> i64 {
    let clamp_axis = |v: i32, origin: i32, extent: i32| -> i32 {
        if extent <= 0 {
            origin
        } else {
            v.clamp(origin, origin.saturating_add(extent - 1))
        }
    };
    let cx = clamp_axis(x, rect.x, rect.width);
    let cy = clamp_axis(y, rect.y, rect.height);
    let dx = i64::from(x.saturating_sub(cx));
    let dy = i64::from(y.saturating_sub(cy));
    dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy))
}

/// 移動對齊「所屬顯示器」半條（design.md D7：「所屬顯示器取矩形中心所在的顯示器（中心不在
/// 任何工作區時取最近者）」）。`monitors` 為空回傳 `None`（呼叫端理論上至少會有一台顯示器）。
pub fn monitor_index_for_point(monitors: &[MonitorInfo], x: i32, y: i32) -> Option<usize> {
    let contains =
        |r: PhysicalRect| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height;
    if let Some(i) = monitors.iter().position(|m| contains(m.work_area)) {
        return Some(i);
    }
    monitors
        .iter()
        .enumerate()
        .min_by_key(|(_, m)| distance_sq_to_rect(m.work_area, x, y))
        .map(|(i, _)| i)
}

/// 移動對齊（design.md D7）：保留 `{keep_w, keep_h}`，只對左上角座標 `round` 到最近格線；
/// 對齊後若讓矩形超出 `0..=GRID`，把左上角夾回界內（格數仍不變，不縮小）。「所屬顯示器」的
/// 判斷由呼叫端先用 [`monitor_index_for_point`] 決定、傳入對應的 `monitor`（兩者分開是因為
/// 呼叫端通常需要先知道所屬顯示器才能決定要跟哪些小工具比對合法性）。
pub fn align_move(
    monitor: &MonitorInfo,
    physical_x: i32,
    physical_y: i32,
    keep_w: i32,
    keep_h: i32,
) -> GridRect {
    let wa = monitor.work_area;
    let col = nearest_grid_index(wa.x, wa.width, physical_x).clamp(0, (GRID - keep_w).max(0));
    let row = nearest_grid_index(wa.y, wa.height, physical_y).clamp(0, (GRID - keep_h).max(0));
    GridRect {
        col,
        row,
        w: keep_w,
        h: keep_h,
    }
}

/// 移動對齊的「不夾回」版本（task 7.5 fix round 1；specs/widget-host-windows「編輯版面」：
/// 放開會超出工作區 SHALL 紅框並彈回）：保留 `{keep_w, keep_h}`、左上角 round 到最近格線，
/// **不**夾回界內——超出工作區的結果照實回傳，由 [`in_grid_bounds`] 判為不合法。編輯版面的
/// 預告與放開（[`legal_move_placement`]）一律用這個版本；[`align_move`] 的夾回版本只留給
/// [`placement_after_move`]。
pub fn align_move_unclamped(
    monitor: &MonitorInfo,
    physical_x: i32,
    physical_y: i32,
    keep_w: i32,
    keep_h: i32,
) -> GridRect {
    let wa = monitor.work_area;
    GridRect {
        col: grid_index_unclamped(wa.x, wa.width, physical_x),
        row: grid_index_unclamped(wa.y, wa.height, physical_y),
        w: keep_w,
        h: keep_h,
    }
}

/// 編輯版面拖曳結束（design.md D7「對齊」；task 7.2 的 `crate::widgets::finish_widget_drag`）：
/// 放開當下的實體矩形 `dropped` → 新的記錄位置。
///
/// - 所屬顯示器：`dropped` 中心所在的顯示器，中心不在任何工作區時取最近者
///   （[`monitor_index_for_point`]）。
/// - 格子：保留 `{keep_w, keep_h}`（呼叫端傳入小工具目前實際的格數），只把左上角對齊到最近
///   格線並夾在界內（[`align_move`]）。
/// - 回傳 `None`：`monitors` 為空，或所屬顯示器查不到穩定識別（[`MonitorInfo::id`] 為
///   `None`）——沒有可保存的身分，呼叫端保留原記錄、不落地（不得以 `\\.\DISPLAYn` 暫代）。
///
/// 合法判斷（重疊、過小）與彈回是 task 7.5 的事，本函式不檢查。
pub fn placement_after_move(
    monitors: &[MonitorInfo],
    dropped: PhysicalRect,
    keep_w: i32,
    keep_h: i32,
) -> Option<WidgetPlacement> {
    let center_x = saturate_i32(i64::from(dropped.x) + i64::from(dropped.width) / 2);
    let center_y = saturate_i32(i64::from(dropped.y) + i64::from(dropped.height) / 2);
    let monitor = &monitors[monitor_index_for_point(monitors, center_x, center_y)?];
    let id = monitor.id.clone()?;
    let rect = align_move(monitor, dropped.x, dropped.y, keep_w, keep_h);
    Some(WidgetPlacement {
        monitor: id,
        col: rect.col,
        row: rect.row,
        w: rect.w,
        h: rect.h,
    })
}

/// task 7.5：[`legal_move_placement`] 的「其他小工具」（不含被拖曳者本身）。
#[derive(Debug, Clone, PartialEq)]
pub struct DropNeighbor {
    /// 記錄位置的顯示器（設定中的 `placement.monitor`）。
    pub record_monitor: MonitorId,
    /// 記錄位置的格子。
    pub record_rect: GridRect,
    /// 目前推導出的實際位置：`(顯示器索引, 格子)`；`None`＝空間不足暫時隱藏（沒有格子，
    /// design.md D9）。因無內容而隱藏者仍有格子，照樣是 `Some`（D7「隱藏中的小工具格子照樣
    /// 保留」）。
    pub actual: Option<(usize, GridRect)>,
}

/// 編輯版面移動的合法判斷（task 7.5；design.md D7「編輯版面」合法條件；
/// specs/widget-host-windows「編輯版面」）：「若在 `dropped` 放開」會得到的新記錄位置，
/// 不合法回傳 `None`。拖曳中的紅框預告（`WM_MOVING`）與放開（`WM_EXITSIZEMOVE`）用同一個
/// 判斷。
///
/// - 對齊：所屬顯示器＝`dropped` 中心所在者（[`monitor_index_for_point`]），保留 `keep`
///   `(w, h)`、左上角 round 到最近格線，**不夾回界內**（[`align_move_unclamped`]，fix round 1）。
/// - 合法條件（全部成立）：
///   1. 所屬顯示器有穩定識別（`MonitorInfo::id` 為 `Some`）——否則沒有可保存的身分，放開也
///      無法寫回，視同不合法（彈回）。
///   2. 不與 `neighbors` 中位於同一台顯示器的**實際位置**相交。
///   3. 不與「記錄就在該顯示器上」（[`assigned_monitor`] 判定 native 且為同一台）的
///      `neighbors` **記錄位置**相交；空間不足暫時隱藏者（`actual == None`）與從別處換來者的
///      記錄不算。
///   4. 在格線範圍內（[`in_grid_bounds`]）。fix F6：移動保留格數、不改大小，**不檢查**最小
///      格數（只有調整大小檢查，[`check_resize_placement`]）——既有記錄小於目前最小格數者
///      （例如舊預設 16×9 的時鐘在 4K＠150%）照樣能移到合法空位。
///
/// 關於「超出工作區」（fix round 1）：round 後只要 `col < 0`、`row < 0`、`col + w > 48` 或
/// `row + h > 48` 就不合法（條件 4 的 [`in_grid_bounds`]）——拖曳中出紅框、放開彈回；剛好貼齊
/// 邊緣（`col + w == 48`）仍合法。跨顯示器時以中心所在顯示器的格座標判斷，左上角落在隔壁
/// 顯示器（相對本台為負）同樣算超界。
pub fn legal_move_placement(
    monitors: &[MonitorInfo],
    dropped: PhysicalRect,
    keep: (i32, i32),
    neighbors: &[DropNeighbor],
) -> Option<WidgetPlacement> {
    check_move_placement(monitors, dropped, keep, neighbors).ok()
}

/// [`legal_move_placement`] 的診斷版（fix monitor-id）：同一套對齊與判斷，不合法時回傳
/// [`PlacementRejection`]（全部不成立的條件與判斷對象），供拖曳結束寫記錄。
pub fn check_move_placement(
    monitors: &[MonitorInfo],
    dropped: PhysicalRect,
    keep: (i32, i32),
    neighbors: &[DropNeighbor],
) -> Result<WidgetPlacement, PlacementRejection> {
    let center_x = saturate_i32(i64::from(dropped.x) + i64::from(dropped.width) / 2);
    let center_y = saturate_i32(i64::from(dropped.y) + i64::from(dropped.height) / 2);
    let target = monitor_index_for_point(monitors, center_x, center_y)
        .ok_or_else(PlacementRejection::no_monitor)?;
    check_move_placement_on(monitors, target, dropped, keep, neighbors)
}

/// [`check_move_placement`] 指定所屬顯示器的版本（fix drag-dpi）：編輯版面拖曳中由
/// [`resolve_drag_rect`] 決定目標顯示器（多數情況＝矩形中心所在者；只有兩台都不自洽的窄帶
/// 維持前一個目標），預告與放開都以同一個 `target` 判斷，與視窗目前的實際大小一致。
/// `target` 超出範圍回傳 `NoMonitor`。
pub fn check_move_placement_on(
    monitors: &[MonitorInfo],
    target: usize,
    dropped: PhysicalRect,
    keep: (i32, i32),
    neighbors: &[DropNeighbor],
) -> Result<WidgetPlacement, PlacementRejection> {
    let monitor = monitors
        .get(target)
        .ok_or_else(PlacementRejection::no_monitor)?;
    let rect = align_move_unclamped(monitor, dropped.x, dropped.y, keep.0, keep.1);
    check_placement_on(monitors, target, rect, None, neighbors)
}

// ── fix drag-dpi：跨縮放比例拖曳時視窗即時變成目標顯示器上的實際大小（design.md D7）──

/// `keep` 格數在 `monitor` 上的實體大小：`(floor(w × 工作區寬 ÷ 48), floor(h × 工作區高 ÷ 48))`
/// （＝從格線 0 起算的 [`edge`] 差）。實際放下後的大小依起始格線可能差 1 px，拖曳中的預覽
/// 不在意這 1 px。
pub fn grid_span_size(monitor: &MonitorInfo, keep: (i32, i32)) -> (i32, i32) {
    let wa = monitor.work_area;
    (
        edge(0, wa.width, keep.0).max(0),
        edge(0, wa.height, keep.1).max(0),
    )
}

/// 游標在 `rect` 內的相對位置（0＝左／上緣，1＝右／下緣），夾在 `[0, 1]`；寬或高 ≤ 0 時該軸
/// 取 0.5。拖曳開始時記下，之後換尺寸也讓游標維持在視窗的同一個比例位置上。
pub fn grab_fraction(rect: PhysicalRect, cursor: (i32, i32)) -> (f64, f64) {
    let axis = |origin: i32, extent: i32, at: i32| {
        if extent <= 0 {
            0.5
        } else {
            ((f64::from(at) - f64::from(origin)) / f64::from(extent)).clamp(0.0, 1.0)
        }
    };
    (
        axis(rect.x, rect.width, cursor.0),
        axis(rect.y, rect.height, cursor.1),
    )
}

/// 以游標為錨擺放 `size` 大小的矩形：游標落在新矩形的 `grab` 比例位置
/// （`x = cursor.x − round(grab.x × 寬)`，y 同理）。
pub fn rect_at_grab(cursor: (i32, i32), grab: (f64, f64), size: (i32, i32)) -> PhysicalRect {
    let offset = |fraction: f64, extent: i32| {
        let v = (fraction * f64::from(extent)).round();
        if v.is_finite() {
            saturate_i32(v as i64)
        } else {
            0
        }
    };
    PhysicalRect {
        x: cursor.0.saturating_sub(offset(grab.0, size.0)),
        y: cursor.1.saturating_sub(offset(grab.1, size.1)),
        width: size.0,
        height: size.1,
    }
}

/// 一次編輯版面移動的尺寸換算上下文（fix drag-dpi），拖曳開始時建立、整個迴圈不變。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DragResize {
    /// 拖曳開始時視窗中心所在的顯示器索引（同一份顯示器快照）。
    pub start_monitor: usize,
    /// 拖曳開始時的視窗實際大小；回到起始顯示器時沿用，不以 [`grid_span_size`] 重算。
    pub start_size: (i32, i32),
    /// 保留的格數（小工具目前實際的 `{w, h}`）。
    pub keep: (i32, i32),
    /// 拖曳開始時游標在視窗內的比例位置（[`grab_fraction`]）。
    pub grab: (f64, f64),
}

impl DragResize {
    fn size_on(&self, monitors: &[MonitorInfo], index: usize) -> (i32, i32) {
        if index == self.start_monitor {
            self.start_size
        } else {
            grid_span_size(&monitors[index], self.keep)
        }
    }
}

/// 拖曳中「視窗此刻應該在哪台顯示器、多大、放在哪」（fix drag-dpi；design.md D7）。
///
/// 每台顯示器 `m` 的候選矩形＝以游標為錨（[`rect_at_grab`]）、大小為 `keep` 在 `m` 上的實際
/// 大小。候選矩形的中心落在 `m` 上稱為「自洽」——此時視窗的實際大小與放開判定（中心所在
/// 顯示器、保留格數）一致。選擇順序：
/// 1. `current`（上一次的目標）自洽就沿用（兩台都自洽時不切換，避免來回跳）；
/// 2. 否則取第一台自洽的顯示器；
/// 3. 都不自洽（抓點偏離中心時，換尺寸會讓中心跨回原處的窄帶）就維持 `current`。
///
/// 第 3 種情況下中心可能不在目標上，所以預告與放開都以回傳的目標判斷
/// （[`check_move_placement_on`]），不再另看中心。`monitors` 為空回傳 `None`；`current`
/// 超出範圍時視同起始顯示器（再超出則取 0）。
pub fn resolve_drag_rect(
    monitors: &[MonitorInfo],
    sizing: &DragResize,
    current: usize,
    cursor: (i32, i32),
) -> Option<(usize, PhysicalRect)> {
    if monitors.is_empty() {
        return None;
    }
    let current = if current < monitors.len() {
        current
    } else if sizing.start_monitor < monitors.len() {
        sizing.start_monitor
    } else {
        0
    };
    let candidate = |m: usize| rect_at_grab(cursor, sizing.grab, sizing.size_on(monitors, m));
    let consistent = |m: usize| {
        let r = candidate(m);
        let cx = saturate_i32(i64::from(r.x) + i64::from(r.width) / 2);
        let cy = saturate_i32(i64::from(r.y) + i64::from(r.height) / 2);
        monitor_index_for_point(monitors, cx, cy) == Some(m)
    };
    let target = if consistent(current) {
        current
    } else {
        (0..monitors.len())
            .find(|&m| consistent(m))
            .unwrap_or(current)
    };
    Some((target, candidate(target)))
}

/// 調整大小對齊（design.md D7）：只對齊 `edges` 指出的被拖邊，其餘邊維持 `original` 的格線
/// 不動。`proposed` 是「此刻放開」的提議實體矩形（`WM_SIZING` 當下的矩形）；未被拖曳的邊直接
/// 沿用 `original` 對應的格線編號，不會被 `proposed` 影響。左／右邊都不可越過對方（`clamp` 保
/// `w、h >= 1`），上／下同理。
///
/// `original` 不合法（不在 [`in_grid_bounds`] 內，例如 w ≤ 0、col ≥ 48）時原樣回傳、不 panic
/// （fix F1，review 7.1 L1）：否則 `clamp` 的下限會大於上限，而本函式可能在 `WM_SIZING` 的
/// 子類別化回呼（FFI 邊界）內被呼叫，panic 會 abort 行程。
pub fn align_resize(
    monitor: &MonitorInfo,
    original: GridRect,
    edges: ResizeEdges,
    proposed: PhysicalRect,
) -> GridRect {
    if !in_grid_bounds(original) {
        return original;
    }
    let wa = monitor.work_area;
    let mut col = original.col;
    let mut row = original.row;
    let mut right = original.right();
    let mut bottom = original.bottom();

    if edges.left {
        col = nearest_grid_index(wa.x, wa.width, proposed.x).clamp(0, right - 1);
    }
    if edges.right {
        right =
            nearest_grid_index(wa.x, wa.width, proposed.x + proposed.width).clamp(col + 1, GRID);
    }
    if edges.top {
        row = nearest_grid_index(wa.y, wa.height, proposed.y).clamp(0, bottom - 1);
    }
    if edges.bottom {
        bottom =
            nearest_grid_index(wa.y, wa.height, proposed.y + proposed.height).clamp(row + 1, GRID);
    }

    GridRect {
        col,
        row,
        w: right - col,
        h: bottom - row,
    }
}

/// 調整大小對齊的「不夾回」版本（task 7.6；與 [`align_move_unclamped`] 同一個理由：spec「編輯
/// 版面」放開會超出工作區或小於最小格數 SHALL 彈回，所以合法判斷要看未夾回的結果）。只有
/// `edges` 指出的被拖邊用提議矩形 round 到最近格線（可為負或大於 48），其餘邊沿用 `original`
/// 的格線；被拖邊越過對邊時寬／高 ≤ 0，照實回傳，由 [`in_grid_bounds`] 判為不合法。
pub fn align_resize_unclamped(
    monitor: &MonitorInfo,
    original: GridRect,
    edges: ResizeEdges,
    proposed: PhysicalRect,
) -> GridRect {
    let wa = monitor.work_area;
    let col = if edges.left {
        grid_index_unclamped(wa.x, wa.width, proposed.x)
    } else {
        original.col
    };
    let right = if edges.right {
        grid_index_unclamped(wa.x, wa.width, proposed.x.saturating_add(proposed.width))
    } else {
        original.right()
    };
    let row = if edges.top {
        grid_index_unclamped(wa.y, wa.height, proposed.y)
    } else {
        original.row
    };
    let bottom = if edges.bottom {
        grid_index_unclamped(wa.y, wa.height, proposed.y.saturating_add(proposed.height))
    } else {
        original.bottom()
    };
    GridRect {
        col,
        row,
        w: right.saturating_sub(col),
        h: bottom.saturating_sub(row),
    }
}

/// 編輯版面調整大小的合法判斷（task 7.6；design.md D7「編輯版面」；specs/widget-host-windows
/// 「調整大小」「縮到比最小格數還小」）：「若在 `proposed` 放開」會得到的新記錄位置，不合法
/// 回傳 `None`。拖曳中的紅框預告（`WM_SIZING`）與放開（`WM_EXITSIZEMOVE`）用同一個判斷。
///
/// - 所屬顯示器：`monitor_index`＝小工具目前實際所在的顯示器（調整大小不換顯示器，不看提議
///   矩形中心）；索引超出範圍視為不合法。
/// - 對齊：`original`（目前實際格子）只有 `edges` 指出的被拖邊 round 到最近格線，不夾回
///   （[`align_resize_unclamped`]）。
/// - 合法條件＝移動的 1–4 條（[`legal_move_placement`]：顯示器有穩定識別、不與同台鄰居的實際
///   位置相交、不與記錄就在這台的鄰居記錄位置相交、在範圍內），**另加**不小於最小格數（fix F6
///   起移動不檢查最小格數，只有調整大小檢查）。
///
/// `zoom_box`＝被調整小工具的倍率設計框（最小格數看它的最小框，widget-adaptive-zoom-and-grid
/// design.md D3）。
pub fn legal_resize_placement(
    monitors: &[MonitorInfo],
    monitor_index: usize,
    original: GridRect,
    edges: ResizeEdges,
    proposed: PhysicalRect,
    zoom_box: &ZoomBox,
    neighbors: &[DropNeighbor],
) -> Option<WidgetPlacement> {
    check_resize_placement(
        monitors,
        monitor_index,
        original,
        edges,
        proposed,
        zoom_box,
        neighbors,
    )
    .ok()
}

/// [`legal_resize_placement`] 的診斷版（fix monitor-id），回傳值同 [`check_move_placement`]。
pub fn check_resize_placement(
    monitors: &[MonitorInfo],
    monitor_index: usize,
    original: GridRect,
    edges: ResizeEdges,
    proposed: PhysicalRect,
    zoom_box: &ZoomBox,
    neighbors: &[DropNeighbor],
) -> Result<WidgetPlacement, PlacementRejection> {
    let monitor = monitors
        .get(monitor_index)
        .ok_or_else(PlacementRejection::no_monitor)?;
    let rect = align_resize_unclamped(monitor, original, edges, proposed);
    check_placement_on(monitors, monitor_index, rect, Some(zoom_box), neighbors)
}

/// 放開不合法的單一原因（fix monitor-id：記錄要寫出具體原因，不必再猜）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlacementRejectReason {
    /// 沒有顯示器，或調整大小時所在顯示器的索引已不在清單內。
    NoMonitor,
    /// 所屬顯示器查不到穩定識別（[`MonitorInfo::id`] 為 `None`），沒有可保存的身分。
    UnresolvedMonitor,
    /// 格子超出 `0..=GRID`（含寬／高 < 1）。
    OutOfBounds,
    /// 與同台鄰居的實際位置，或記錄就在這台的鄰居記錄位置相交。
    Overlap,
    /// 小於該顯示器當下的最小格數。
    BelowMinSize,
}

impl PlacementRejectReason {
    fn label(self) -> &'static str {
        match self {
            Self::NoMonitor => "沒有可用的顯示器",
            Self::UnresolvedMonitor => "顯示器無穩定識別",
            Self::OutOfBounds => "超出格線範圍",
            Self::Overlap => "與其他小工具重疊",
            Self::BelowMinSize => "小於最小格數",
        }
    }
}

/// 放開不合法時的診斷資料：所有不成立的條件（[`PlacementRejectReason`]，依判斷順序、可多個）
/// 與判斷當下的對象（顯示器索引、工作區、對齊後的格子、相交的已佔格子），供呼叫端寫記錄。
#[derive(Debug, Clone, PartialEq)]
pub struct PlacementRejection {
    pub reasons: Vec<PlacementRejectReason>,
    /// 判斷所在的顯示器索引；`NoMonitor` 時為 `None`。
    pub monitor_index: Option<usize>,
    pub work_area: Option<PhysicalRect>,
    /// 對齊後（未夾回）的格子；`NoMonitor` 時沒有可對齊的工作區，為 `None`。
    pub rect: Option<GridRect>,
    /// 與 `rect` 相交的已佔格子（只在含 `Overlap` 時非空）。
    pub overlapping: Vec<GridRect>,
}

impl PlacementRejection {
    fn no_monitor() -> Self {
        PlacementRejection {
            reasons: vec![PlacementRejectReason::NoMonitor],
            monitor_index: None,
            work_area: None,
            rect: None,
            overlapping: Vec::new(),
        }
    }
}

fn fmt_grid(r: GridRect) -> String {
    format!("col={} row={} w={} h={}", r.col, r.row, r.w, r.h)
}

impl std::fmt::Display for PlacementRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let labels: Vec<&str> = self.reasons.iter().map(|r| r.label()).collect();
        write!(f, "原因：{}", labels.join("、"))?;
        if let Some(i) = self.monitor_index {
            write!(f, "；顯示器 #{i}")?;
        }
        if let Some(wa) = self.work_area {
            write!(
                f,
                "（工作區 x={} y={} {}×{}）",
                wa.x, wa.y, wa.width, wa.height
            )?;
        }
        if let Some(rect) = self.rect {
            write!(f, "；對齊後格子 {}", fmt_grid(rect))?;
        }
        if !self.overlapping.is_empty() {
            let list: Vec<String> = self.overlapping.iter().map(|&r| fmt_grid(r)).collect();
            write!(f, "；相交的格子 [{}]", list.join("；"))?;
        }
        Ok(())
    }
}

/// [`check_move_placement`] 與 [`check_resize_placement`]（及其 `legal_*` 版本）共用的合法
/// 判斷：`rect` 放在 `monitors[target]` 上是否合法（條件見 [`legal_move_placement`] 文件的
/// 1–4 條），合法回傳可保存的記錄位置；不合法時回傳**全部**不成立的條件，不在第一個失敗處
/// 停下（fix monitor-id）。`min_size`＝被調整小工具的倍率設計框：`Some` 時另檢查最小格數
/// （調整大小，[`meets_min_grid_size`]），`None` 不檢查（移動，fix F6）。
fn check_placement_on(
    monitors: &[MonitorInfo],
    target: usize,
    rect: GridRect,
    min_size: Option<&ZoomBox>,
    neighbors: &[DropNeighbor],
) -> Result<WidgetPlacement, PlacementRejection> {
    let Some(monitor) = monitors.get(target) else {
        return Err(PlacementRejection::no_monitor());
    };
    let occupied: Vec<GridRect> = neighbors
        .iter()
        .flat_map(|n| {
            let (actual_index, actual_rect) = n.actual?;
            let actual = (actual_index == target).then_some(actual_rect);
            let record = (assigned_monitor(monitors, &n.record_monitor) == Some((target, true)))
                .then_some(n.record_rect);
            Some([actual, record])
        })
        .flatten()
        .flatten()
        .collect();

    // 與 is_legal_grid_rect 同三個條件，逐一判斷以便列出原因。
    // 鄰居的實際位置與記錄位置常是同一格，記錄裡只列一次。
    let mut overlapping: Vec<GridRect> = Vec::new();
    for &other in &occupied {
        if rects_overlap(rect, other) && !overlapping.contains(&other) {
            overlapping.push(other);
        }
    }
    let mut reasons = Vec::new();
    if monitor.id.is_none() {
        reasons.push(PlacementRejectReason::UnresolvedMonitor);
    }
    if !in_grid_bounds(rect) {
        reasons.push(PlacementRejectReason::OutOfBounds);
    }
    if !overlapping.is_empty() {
        reasons.push(PlacementRejectReason::Overlap);
    }
    // fix F6：只有調整大小（`min_size` 為 `Some`）檢查最小格數；移動保留格數、不改大小，
    // 不檢查（既有記錄小於新最小格數者照樣能移到合法空位）。
    if let Some(zoom_box) = min_size {
        if !meets_min_grid_size(monitor.work_area, rect, monitor.scale_factor, zoom_box) {
            reasons.push(PlacementRejectReason::BelowMinSize);
        }
    }

    match (&monitor.id, reasons.is_empty()) {
        (Some(id), true) => Ok(WidgetPlacement {
            monitor: id.clone(),
            col: rect.col,
            row: rect.row,
            w: rect.w,
            h: rect.h,
        }),
        _ => Err(PlacementRejection {
            reasons,
            monitor_index: Some(target),
            work_area: Some(monitor.work_area),
            rect: Some(rect),
            overlapping,
        }),
    }
}

/// 找空位（design.md D7）：候選左上角的列 `row` 由 0 往下、同一列的欄 `col` 由 `GRID - w`
/// 往 0（右上優先），第一個與 `occupied` 都不相交的候選即回傳。`w`／`h` 不在 `1..=GRID`
/// 範圍內視為不可能有解，直接回傳 `None`。
pub fn find_empty_slot(w: i32, h: i32, occupied: &[GridRect]) -> Option<GridRect> {
    if w < 1 || h < 1 || w > GRID || h > GRID {
        return None;
    }
    for row in 0..=(GRID - h) {
        let mut col = GRID - w;
        loop {
            let candidate = GridRect { col, row, w, h };
            if occupied.iter().all(|&o| !rects_overlap(candidate, o)) {
                return Some(candidate);
            }
            if col == 0 {
                break;
            }
            col -= 1;
        }
    }
    None
}

/// 找空位的「記錄格數→最小格數」順序（design.md D7、D9）：先試 `record`（從未開啟過者即預設
/// 格數）大小，找不到再試 `min` 大小。
pub fn find_slot_preferring_record_size(
    record: (i32, i32),
    min: (i32, i32),
    occupied: &[GridRect],
) -> Option<GridRect> {
    find_empty_slot(record.0, record.1, occupied)
        .or_else(|| find_empty_slot(min.0, min.1, occupied))
}

/// D9「實際位置推導」步驟 1：每個小工具對應的實體顯示器索引與「是否記錄就在這台」。
/// `Primary` 找目前主螢幕；`Device` 先找相符的識別，找到即為「記錄在這台」，找不到（螢幕已
/// 拔除）才退回主螢幕、標記「從別處換來」。
///
/// 「主螢幕」的退回鏈（fix F1，review 7.1 M1）：清單中標 `is_primary` 者 → 沒有任何一台
/// 標主螢幕時取清單第一台（`desktop::monitor_infos_from_tauri_monitors` 在 `primary_monitor()`
/// 回 `None` 或主螢幕名稱對不到時刻意不標，見該處文件；比照已移除的舊模型
/// `resolve_placement_rect`）。`Primary` 落在第一台仍算「記錄在這台」。只有 `monitors` 為空時
/// 才回傳 `None`。
///
/// 這個「native」旗標同時就是 D7「開啟小工具找空位」（[`placement_for_opening_widget`]）判斷
/// 「所屬顯示器是否存在」的答案：`Device` 找不到相符識別時退回主螢幕、`native=false`，正是
/// 「所屬顯示器不存在」的情況；`Primary` 只要能對到目前主螢幕就一定是 `native=true`。兩處共用
/// 同一份邏輯，不重複定義「存在」的意思。
fn assigned_monitor(monitors: &[MonitorInfo], id: &MonitorId) -> Option<(usize, bool)> {
    match id {
        MonitorId::Primary => primary_index(monitors).map(|i| (i, true)),
        MonitorId::Device(_) => {
            if let Some(i) = monitors.iter().position(|m| m.id.as_ref() == Some(id)) {
                Some((i, true))
            } else {
                primary_index(monitors).map(|i| (i, false))
            }
        }
    }
}

/// 主螢幕的索引：標 `is_primary` 者，沒有就退回清單第一台；`monitors` 為空才回傳 `None`
/// （退回鏈見 [`assigned_monitor`] 文件）。
fn primary_index(monitors: &[MonitorInfo]) -> Option<usize> {
    monitors
        .iter()
        .position(|m| m.is_primary)
        .or_else(|| (!monitors.is_empty()).then_some(0))
}

/// D9「實際位置推導」：記錄位置＋目前顯示器清單 → 每個小工具的實際位置。純函式、無狀態——
/// 同樣輸入永遠得到同樣輸出，因此「顯示器拔除後重新連接」「編輯版面放開後重新推導」都只是
/// 「用不同的 `monitors` 或更新後的 `widgets[i].record_rect` 再呼叫一次」，見
/// `grid_tests::resolve_reconnecting_monitor_restores_original_position` 與
/// `grid_tests::resolve_after_successful_drop_rederiving_keeps_same_position`。
///
/// 演算法（design.md D9 點 2）：
/// 1. 依 [`assigned_monitor`] 把每個小工具分派到一台顯示器，並標記是否「記錄在這台」（native）
///    或「從別處換來」（moved，即 `Device` 找不到、退回主螢幕）。
/// 2. 每台顯示器獨立處理，優先序＝native 優先於 moved、同級依輸入順序（即註冊表順序）：
///    - 第一階段：依優先序單次掃描，只把記錄格子與已放置者不相交的小工具直接放在記錄格子；
///      相交者跳過、留到第二階段（不在同一次掃描內邊放邊找空位，否則會形成連鎖換位）。
///    - 第二階段：依同一優先序，替被跳過者用 [`find_slot_preferring_record_size`] 找空位
///      （先試記錄格數、再試 [`min_grid_size`]）；仍找不到則 `HiddenNoSpace`，不佔格。
/// 3. 輸出順序與輸入 `widgets` 一致（不因任何一筆而跳過或重排其他筆）。
///
/// 第一階段只檢查範圍與碰撞、**不**檢查最小格數：記錄格子小於最小格數（例如
/// widget-adaptive-zoom-and-grid design.md D3 記載的時鐘窄帶）但在界內且不與他人相交者照原位
/// 放置，不移動也不隱藏，倍率由 [`content_zoom`] 夾在 0.5（`grid_tests::
/// resolve_keeps_record_below_min_size_in_place_with_zoom_half`）。
///
/// 每個放置者的倍率＝[`content_zoom`]（實體矩形、該顯示器縮放、該小工具的 [`ZoomBox`]、
/// `font_scale`）。`font_scale` 由呼叫端傳入當下設定（`Settings::font_scale`；拖曳中為拖曳開始
/// 時的快照，widget-adaptive-zoom-and-grid design.md D2）；放置與最小格數都與它無關。
pub fn resolve_grid_placements(
    monitors: &[MonitorInfo],
    widgets: &[GridWidgetInput],
    font_scale: f64,
) -> Vec<ResolvedWidgetPlacement> {
    let assigned: Vec<Option<(usize, bool)>> = widgets
        .iter()
        .map(|w| assigned_monitor(monitors, &w.monitor))
        .collect();

    let mut result: Vec<Option<ResolvedWidgetPlacement>> = vec![None; widgets.len()];

    for (monitor_index, monitor) in monitors.iter().enumerate() {
        let mut on_this_monitor: Vec<usize> = (0..widgets.len())
            .filter(|&i| assigned[i].map(|(m, _)| m) == Some(monitor_index))
            .collect();
        // 穩定排序：native（true）排在 moved（false）之前，同組內保留原輸入（註冊表）順序。
        on_this_monitor.sort_by_key(|&i| !assigned[i].expect("已由上方 filter 篩過，必為 Some").1);

        // 第一階段：單次掃描，只放記錄格子與已放置者不相交者，相交者延後。
        let mut placed: Vec<(usize, GridRect)> = Vec::new();
        let mut pending: Vec<usize> = Vec::new();
        for &i in &on_this_monitor {
            let rect = widgets[i].record_rect;
            let fits = in_grid_bounds(rect) && placed.iter().all(|&(_, r)| !rects_overlap(rect, r));
            if fits {
                placed.push((i, rect));
            } else {
                pending.push(i);
            }
        }

        // 第二階段：替被跳過者找空位（記錄格數 → 最小格數），仍找不到則暫時隱藏。
        for i in pending {
            let widget = &widgets[i];
            let occupied: Vec<GridRect> = placed.iter().map(|&(_, r)| r).collect();
            let min = min_grid_size(monitor.work_area, monitor.scale_factor, &widget.zoom_box);
            let record_size = (widget.record_rect.w.max(1), widget.record_rect.h.max(1));
            match find_slot_preferring_record_size(record_size, min, &occupied) {
                Some(rect) => placed.push((i, rect)),
                None => result[i] = Some(ResolvedWidgetPlacement::HiddenNoSpace),
            }
        }

        for (i, rect) in placed {
            let physical_rect = grid_rect_to_physical(monitor.work_area, rect);
            let zoom = content_zoom(
                physical_rect.width,
                physical_rect.height,
                monitor.scale_factor,
                &widgets[i].zoom_box,
                font_scale,
            );
            let moved_from_elsewhere = !assigned[i].expect("此索引必已成功分派到本顯示器").1;
            result[i] = Some(ResolvedWidgetPlacement::Placed {
                monitor_index,
                rect,
                physical_rect,
                zoom,
                moved_from_elsewhere,
            });
        }
    }

    result
        .into_iter()
        .map(|r| r.unwrap_or(ResolvedWidgetPlacement::HiddenNoSpace))
        .collect()
}

/// task 7.4（design.md D7「記錄位置的寫回時機」第 2 點；specs/widget-host-windows「小工具互不
/// 重疊」「開啟小工具但原位置已被佔用」「空間不足」Scenario）：開啟一個小工具時，判斷它的記錄
/// 位置是否需要換位。
///
/// - `target_monitor`／`target_record`／`target_zoom_box`：即將開啟的小工具本身的記錄位置與
///   倍率設計框（找空位退回最小格數時用，widget-adaptive-zoom-and-grid design.md D3）。
/// - `others`：目前**已開啟**（不含這次要開啟的這個）小工具的 `(記錄顯示器, 記錄格子)` 清單，
///   依呼叫端決定的順序（`crate::widgets` 依註冊表順序逐一處理多個同時開啟時，這裡看到的是
///   「先前已處理者的新位置＋尚未處理者的原記錄」，見該處呼叫端文件）。
///
/// 回傳：
/// - `Ok(None)`：所屬顯示器不存在（`Device` 找不到相符識別，退回主螢幕但不是「記錄在這台」），
///   或所屬顯示器存在但記錄格子與「記錄就在該顯示器上」的其他小工具都不相交（含 `others` 為空）
///   ——兩種情況呼叫端都不需要改記錄，沿用原記錄（design.md D7 點 2／點 3：「不找空位、不改
///   記錄」與「不相交、照常開啟」在寫回這件事上是同一種結果）。
/// - `Ok(Some(rect))`：找到空位，呼叫端應把記錄位置改成這個新格子（`monitor` 不變）。
/// - `Err(())`：所屬顯示器存在、確實相交、但連最小格數都找不到空位——呼叫端應拒絕整個操作
///   （`update_settings` 回傳「空間不足，請先調整版面」，設定不變）。
///
/// 「記錄就在該顯示器上」只計入 `others` 中 `assigned_monitor` 判定為 native 且落在同一台
/// 顯示器者（與 D9 推導、D7「編輯版面」合法性判斷用的是同一個「native」定義，見
/// [`assigned_monitor`] 文件）——從別處換來者與暫時隱藏者的記錄不算，否則看似空白處也會被擋。
pub fn placement_for_opening_widget(
    monitors: &[MonitorInfo],
    target_monitor: &MonitorId,
    target_record: GridRect,
    target_zoom_box: &ZoomBox,
    others: &[(MonitorId, GridRect)],
) -> Result<Option<GridRect>, ()> {
    let Some((target_index, true)) = assigned_monitor(monitors, target_monitor) else {
        // 所屬顯示器不存在（Device 找不到相符識別），或 `monitors` 為空（顯示器列舉暫時失敗）。
        return Ok(None);
    };

    let occupied: Vec<GridRect> = others
        .iter()
        .filter_map(|(monitor, rect)| {
            let (index, native) = assigned_monitor(monitors, monitor)?;
            (native && index == target_index).then_some(*rect)
        })
        .collect();

    if !occupied
        .iter()
        .any(|&other| rects_overlap(target_record, other))
    {
        return Ok(None);
    }

    let monitor = &monitors[target_index];
    let min = min_grid_size(monitor.work_area, monitor.scale_factor, target_zoom_box);
    let record_size = (target_record.w.max(1), target_record.h.max(1));
    find_slot_preferring_record_size(record_size, min, &occupied)
        .map(Some)
        .ok_or(())
}

#[cfg(test)]
mod grid_tests {
    use super::*;

    // ── fixtures ──────────────────────────────────────────────────────────────────

    fn monitor_at(
        id: &str,
        work_area: PhysicalRect,
        scale_factor: f64,
        is_primary: bool,
    ) -> MonitorInfo {
        MonitorInfo {
            id: Some(MonitorId::Device(id.to_string())),
            work_area,
            scale_factor,
            is_primary,
        }
    }

    fn wa(x: i32, y: i32, width: i32, height: i32) -> PhysicalRect {
        PhysicalRect {
            x,
            y,
            width,
            height,
        }
    }

    /// 十個小工具的 id 與倍率設計框（task 7.2 起引用正式常數 `crate::widgets::WIDGET_SPECS`，
    /// 不再在測試內自備數字）。順序與 [`crate::settings::WIDGET_IDS`] 相同。
    fn preset_design(i: usize) -> (&'static str, ZoomBox) {
        let spec = &crate::widgets::WIDGET_SPECS[i];
        (spec.id, spec.zoom_box)
    }

    /// 十個預設格座標（task 7.2 起引用正式常數 `crate::settings::DEFAULT_GRID_RECTS`）。
    const PRESET_RECTS: [GridRect; 10] = crate::settings::DEFAULT_GRID_RECTS;

    /// 扣掉工作列後的工作區（工作列估計 40 邏輯 px，依縮放換算成實體 px）。
    fn workspace(physical_width: i32, physical_height: i32, scale: f64) -> PhysicalRect {
        let taskbar_px = (40.0 * scale).round() as i32;
        wa(0, 0, physical_width, (physical_height - taskbar_px).max(1))
    }

    // ── D7：edge() 與格線→實體矩形 ───────────────────────────────────────────────

    #[test]
    fn edge_divides_extent_into_48_equal_floor_steps() {
        // 工作區實體長度 960（960/48=20，整除，無取整誤差），edge(24) 應正好是中點。
        assert_eq!(edge(100, 960, 0), 100);
        assert_eq!(edge(100, 960, 48), 100 + 960);
        assert_eq!(edge(100, 960, 24), 100 + 480);
    }

    #[test]
    fn edge_floors_when_extent_not_divisible_by_grid() {
        // 1000/48=20.8333…，floor(24*1000/48)=floor(500)=500（整除剛好）；
        // floor(1*1000/48)=floor(20.833)=20（有取整誤差時應無條件捨去，不四捨五入）。
        assert_eq!(edge(0, 1000, 1), 20);
        assert_eq!(edge(0, 1000, 24), 500);
    }

    // ── widget-adaptive-zoom-and-grid task 1.3：格線偏移（編輯版面畫格線用）───────────

    #[test]
    fn grid_line_offsets_equal_edge_with_zero_origin_for_every_line() {
        // 含整除（960）、不整除（1000）、奇數（1037、47）、極小（0、1、2、47、48、49）與負值。
        for extent in [-5, 0, 1, 2, 47, 48, 49, 960, 1000, 1037, 1920, 2561] {
            let offsets = grid_line_offsets(extent);
            assert_eq!(offsets.len(), 47);
            for (k, &off) in offsets.iter().enumerate() {
                let i = k as i32 + 1;
                assert_eq!(off, edge(0, extent, i), "extent={extent} i={i}");
            }
        }
    }

    #[test]
    fn grid_line_offsets_match_grid_rect_to_physical_edges() {
        // 與吸附同源：對任意工作區原點，原點＋偏移＝單格矩形的左／上緣，
        // 且相鄰兩格的共用邊（前一格右緣＝後一格左緣）就是該偏移。
        for (x, y, w, h) in [
            (0, 0, 1920, 1040),
            (-1920, -40, 1037, 701),
            (100, 50, 47, 49),
        ] {
            let work_area = wa(x, y, w, h);
            let xs = grid_line_offsets(w);
            let ys = grid_line_offsets(h);
            for i in 1..GRID {
                let col_rect = grid_rect_to_physical(
                    work_area,
                    GridRect {
                        col: i,
                        row: 0,
                        w: 1,
                        h: 1,
                    },
                );
                let prev_col_rect = grid_rect_to_physical(
                    work_area,
                    GridRect {
                        col: i - 1,
                        row: 0,
                        w: 1,
                        h: 1,
                    },
                );
                assert_eq!(col_rect.x, x + xs[(i - 1) as usize], "x i={i} wa={w}");
                assert_eq!(prev_col_rect.x + prev_col_rect.width, col_rect.x);
                let row_rect = grid_rect_to_physical(
                    work_area,
                    GridRect {
                        col: 0,
                        row: i,
                        w: 1,
                        h: 1,
                    },
                );
                assert_eq!(row_rect.y, y + ys[(i - 1) as usize], "y i={i} wa={h}");
            }
        }
    }

    #[test]
    fn grid_line_offsets_are_non_decreasing_and_inside_extent() {
        for extent in [0, 1, 47, 49, 1037, 2561] {
            let offsets = grid_line_offsets(extent);
            assert!(offsets.windows(2).all(|p| p[0] <= p[1]), "extent={extent}");
            assert!(offsets.iter().all(|&o| (0..=extent.max(0)).contains(&o)));
        }
    }

    #[test]
    fn adjacent_grid_rects_share_exact_pixel_boundary_no_gap_no_overlap() {
        // specs/widget-host-windows「版面格線」Scenario：A 右緣與 B 左緣是同一條格線。
        let work_area = wa(0, 0, 1000, 700);
        let a = GridRect {
            col: 0,
            row: 0,
            w: 20,
            h: 20,
        };
        let b = GridRect {
            col: 20,
            row: 0,
            w: 10,
            h: 20,
        };
        let ra = grid_rect_to_physical(work_area, a);
        let rb = grid_rect_to_physical(work_area, b);
        assert_eq!(
            ra.x + ra.width,
            rb.x,
            "共用同一條格線的兩個小工具，該邊應落在同一個像素，不重疊也不留縫"
        );
    }

    #[test]
    fn grid_rect_to_physical_uses_work_area_origin_not_zero() {
        // 外接螢幕工作區原點非 (0,0)（虛擬桌面座標可為負）。
        let work_area = wa(-1920, -40, 1920, 1040);
        let rect = GridRect {
            col: 0,
            row: 0,
            w: 48,
            h: 48,
        };
        let phys = grid_rect_to_physical(work_area, rect);
        assert_eq!(
            phys,
            PhysicalRect {
                x: -1920,
                y: -40,
                width: 1920,
                height: 1040
            }
        );
    }

    // ── widget-adaptive-zoom-and-grid design.md D1：倍率模型（最小框＋舒適框）──────────

    fn zb(
        min_width: f64,
        min_height: f64,
        comfort_width: Option<f64>,
        comfort_height: f64,
    ) -> ZoomBox {
        ZoomBox {
            min_width,
            min_height,
            comfort_width,
            comfort_height,
        }
    }

    /// 舒適框＝最小框的倍率設計框（只關心最小格數與放置、不關心自適應的測試用）。
    fn fixed_box(width: f64, height: f64) -> ZoomBox {
        zb(width, height, Some(width), height)
    }

    /// 清單類（總經日曆）的框：min 375×216、comfort 500×324（D1 表格）。
    fn list_box() -> ZoomBox {
        zb(375.0, 216.0, Some(500.0), 324.0)
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn content_zoom_wide_but_short_list_is_limited_by_height() {
        // 規格 Scenario「清單又寬又矮」：邏輯寬＝設計寬 2 倍、邏輯高＝舒適框高 → 倍率 1。
        assert!(approx(content_zoom(1000, 324, 1.0, &list_box(), 1.0), 1.0));
    }

    #[test]
    fn content_zoom_font_scale_multiplies_auto_until_cap() {
        // 規格 Scenario「字級設定放大」：600×400 → auto＝min(600/500, 400/324)＝1.2、
        // cap＝min(600/375, 400/216)＝1.6；字級 1.2 → 1.44，字級 1.5 → 1.8 被 cap 截到 1.6。
        let b = list_box();
        assert!(approx(content_zoom(600, 400, 1.0, &b, 1.0), 1.2));
        assert!(approx(content_zoom(600, 400, 1.0, &b, 1.2), 1.44));
        assert!(approx(content_zoom(600, 400, 1.0, &b, 1.5), 1.6));
    }

    #[test]
    fn content_zoom_clock_fills_the_more_constrained_axis() {
        // 規格 Scenario「時鐘框放大」：寬高都 ≥ 最小框兩倍 → 至少 2 倍，取較受限的一軸。
        let clock = zb(212.0, 160.0, Some(212.0), 160.0);
        assert!(approx(content_zoom(424, 320, 1.0, &clock, 1.0), 2.0));
        assert!(approx(
            content_zoom(530, 330, 1.0, &clock, 1.0),
            330.0 / 160.0
        ));
    }

    #[test]
    fn content_zoom_without_comfort_width_only_looks_at_height() {
        // 行情條（comfort_width＝None）：寬度不限制倍率，上限也不看寬（規格 Scenario「行情條加高」）。
        let quotes = zb(992.0, 60.0, None, 60.0);
        assert!(approx(content_zoom(5000, 60, 1.0, &quotes, 1.0), 1.0));
        assert!(approx(content_zoom(5000, 120, 1.0, &quotes, 1.0), 2.0));
        assert!(
            approx(content_zoom(300, 120, 1.0, &quotes, 1.0), 2.0),
            "寬度遠小於 min 寬仍只看高"
        );
    }

    #[test]
    fn content_zoom_divides_physical_size_by_monitor_scale_first() {
        // 150%：實體 750×486 → 邏輯 500×324 → 清單框 auto＝1、cap＝min(1.33, 1.5) → 1。
        assert!(approx(content_zoom(750, 486, 1.5, &list_box(), 1.0), 1.0));
    }

    #[test]
    fn content_zoom_clamps_to_half_and_three() {
        assert_eq!(content_zoom(10, 10, 1.0, &list_box(), 1.0), 0.5);
        let clock = zb(212.0, 160.0, Some(212.0), 160.0);
        assert_eq!(content_zoom(5000, 5000, 1.0, &clock, 1.0), 3.0);
    }

    #[test]
    fn content_zoom_handles_invalid_inputs_without_panicking() {
        let bad = [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY];
        let in_range = |z: f64| z.is_finite() && (0.5..=3.0).contains(&z);
        for v in bad {
            assert!(
                in_range(content_zoom(600, 400, v, &list_box(), 1.0)),
                "scale={v}"
            );
            assert!(in_range(content_zoom(
                600,
                400,
                1.0,
                &zb(v, 216.0, Some(500.0), 324.0),
                1.0
            )));
            assert!(in_range(content_zoom(
                600,
                400,
                1.0,
                &zb(375.0, v, Some(500.0), 324.0),
                1.0
            )));
            assert!(in_range(content_zoom(
                600,
                400,
                1.0,
                &zb(375.0, 216.0, Some(v), 324.0),
                1.0
            )));
            assert!(in_range(content_zoom(
                600,
                400,
                1.0,
                &zb(375.0, 216.0, Some(500.0), v),
                1.0
            )));
            // font_scale 非法時視為 1.0（D1）。
            assert_eq!(
                content_zoom(600, 400, 1.0, &list_box(), v),
                content_zoom(600, 400, 1.0, &list_box(), 1.0),
                "font_scale={v} 應視為 1.0"
            );
        }
        for (w, h) in [(0, 0), (-100, 400), (600, -5), (i32::MIN, i32::MAX)] {
            assert!(
                in_range(content_zoom(w, h, 1.0, &list_box(), 1.0)),
                "{w}x{h}"
            );
        }
    }

    /// 屬性測試（D1）：多種實體尺寸 × 縮放 × 框 × 字級，倍率恆在 0.5–3；矩形不小於最小格數
    /// （邏輯寬 ≥ min 寬／2〔有 comfort 寬才看〕、邏輯高 ≥ min 高／2）時倍率不超過上限
    /// ＝內容不會因倍率而超出矩形（上限本身 > 3 時以 3 為界，夾到 3 也不超過上限）。
    #[test]
    fn content_zoom_stays_in_range_and_never_exceeds_cap_when_rect_meets_min_size() {
        let boxes = [
            zb(212.0, 160.0, Some(212.0), 160.0),
            list_box(),
            zb(352.5, 176.0, Some(470.0), 264.0),
            zb(352.5, 136.0, Some(470.0), 204.0),
            zb(992.0, 60.0, None, 60.0),
        ];
        let mut checked_cap = 0usize;
        for b in &boxes {
            for scale in [1.0, 1.25, 1.5, 1.75, 2.0, 2.5] {
                for font in [0.7, 0.85, 1.0, 1.2, 1.5] {
                    for w in (20..=4000).step_by(97) {
                        for h in (10..=2400).step_by(53) {
                            let z = content_zoom(w, h, scale, b, font);
                            assert!(
                                (0.5..=3.0).contains(&z),
                                "{w}x{h}＠{scale} font {font} → {z}"
                            );
                            let lw = f64::from(w) / scale;
                            let lh = f64::from(h) / scale;
                            let width_ok = b.comfort_width.is_none() || lw >= b.min_width * 0.5;
                            if width_ok && lh >= b.min_height * 0.5 {
                                let cap_w = if b.comfort_width.is_some() {
                                    lw / b.min_width
                                } else {
                                    f64::INFINITY
                                };
                                let cap = cap_w.min(lh / b.min_height);
                                assert!(
                                    z <= cap + 1e-9,
                                    "{b:?} {w}x{h}＠{scale} font {font}：{z} 超過上限 {cap}"
                                );
                                checked_cap += 1;
                            }
                        }
                    }
                }
            }
        }
        assert!(
            checked_cap > 10_000,
            "前提：上限斷言實際跑過（{checked_cap}）"
        );
    }

    // ── D7：碰撞判斷與範圍 ────────────────────────────────────────────────────────

    #[test]
    fn rects_overlap_detects_true_overlap() {
        let a = GridRect {
            col: 0,
            row: 0,
            w: 10,
            h: 10,
        };
        let b = GridRect {
            col: 5,
            row: 5,
            w: 10,
            h: 10,
        };
        assert!(rects_overlap(a, b));
    }

    #[test]
    fn rects_overlap_is_false_for_shared_edge() {
        // 共用邊界不算重疊（版面格線 Scenario 的另一面：不重疊）。
        let a = GridRect {
            col: 0,
            row: 0,
            w: 10,
            h: 10,
        };
        let b = GridRect {
            col: 10,
            row: 0,
            w: 10,
            h: 10,
        };
        assert!(!rects_overlap(a, b));
    }

    #[test]
    fn rects_overlap_is_false_for_fully_separate_rects() {
        let a = GridRect {
            col: 0,
            row: 0,
            w: 5,
            h: 5,
        };
        let b = GridRect {
            col: 20,
            row: 20,
            w: 5,
            h: 5,
        };
        assert!(!rects_overlap(a, b));
    }

    /// fix F1（review 7.2 M1）：有加法的地方不得溢位——`in_grid_bounds` 對極值回傳 `false`、
    /// `right()`／`bottom()` 飽和、碰撞判斷與實體換算不 panic。
    #[test]
    fn grid_arithmetic_does_not_overflow_on_extreme_values() {
        let extremes = [
            g(i32::MAX - 600, 0, 1000, 5),
            g(1, 1, i32::MAX, 5),
            g(0, i32::MAX, 5, i32::MAX),
            g(i32::MIN, 0, i32::MAX, 5),
            g(i32::MAX, i32::MAX, i32::MAX, i32::MAX),
        ];
        for rect in extremes {
            assert!(!in_grid_bounds(rect), "{rect:?} 不應通過範圍驗證");
            let _ = rects_overlap(rect, g(0, 0, 48, 48));
            let phys = grid_rect_to_physical(wa(-3840, -2160, 3840, 2160), rect);
            assert!(phys.width >= 0 && phys.height >= 0);
        }
        assert_eq!(
            g(i32::MAX - 600, 0, 1000, 5).right(),
            i32::MAX,
            "right() 應飽和"
        );
        assert_eq!(
            g(0, i32::MAX, 5, i32::MAX).bottom(),
            i32::MAX,
            "bottom() 應飽和"
        );
    }

    #[test]
    fn in_grid_bounds_accepts_full_grid_and_rejects_out_of_range() {
        assert!(in_grid_bounds(GridRect {
            col: 0,
            row: 0,
            w: 48,
            h: 48
        }));
        assert!(
            !in_grid_bounds(GridRect {
                col: 40,
                row: 0,
                w: 10,
                h: 1
            }),
            "col+w 超過 48"
        );
        assert!(
            !in_grid_bounds(GridRect {
                col: 0,
                row: 40,
                w: 1,
                h: 10
            }),
            "row+h 超過 48"
        );
        assert!(
            !in_grid_bounds(GridRect {
                col: -1,
                row: 0,
                w: 5,
                h: 5
            }),
            "col 為負"
        );
        assert!(
            !in_grid_bounds(GridRect {
                col: 0,
                row: 0,
                w: 0,
                h: 5
            }),
            "w 必須 >= 1"
        );
        assert!(
            !in_grid_bounds(GridRect {
                col: 0,
                row: 0,
                w: 5,
                h: 0
            }),
            "h 必須 >= 1"
        );
    }

    // ── D7：最小格數與放開合法判斷 ──────────────────────────────────────────────────

    #[test]
    fn meets_min_grid_size_true_for_generous_rect() {
        let work_area = wa(0, 0, 1920, 1040);
        // 台股事件框（min 352.5×176）、col15,row1,w15,h17：寬 600、高 368，遠超最小框一半。
        let rect = GridRect {
            col: 15,
            row: 1,
            w: 15,
            h: 17,
        };
        assert!(meets_min_grid_size(
            work_area,
            rect,
            1.0,
            &zb(352.5, 176.0, Some(470.0), 264.0)
        ));
    }

    #[test]
    fn meets_min_grid_size_false_when_width_below_half_min_width() {
        // 工作區寬 960，48 格 = 20px/格。min 寬 2000：需邏輯寬 ≥ 1000＝50 格——超過 GRID，
        // 任何寬度都判不合法。
        let work_area = wa(0, 0, 960, 960);
        let rect = GridRect {
            col: 0,
            row: 0,
            w: 48,
            h: 48,
        };
        assert!(!meets_min_grid_size(
            work_area,
            rect,
            1.0,
            &zb(2000.0, 10.0, Some(2000.0), 10.0)
        ));
    }

    /// widget-adaptive-zoom-and-grid design.md D3：門檻正好是最小框的一半（倍率 0.5 時塞得下），
    /// 高度條件不再乘依寬度算出的 zoom——寬矩形不會因此要求更高。工作區 960×960＝每格 20 px。
    #[test]
    fn meets_min_grid_size_threshold_is_half_of_min_box_on_each_axis() {
        let work_area = wa(0, 0, 960, 960);
        let b = zb(400.0, 200.0, Some(500.0), 300.0);
        // 需邏輯寬 ≥ 200（10 格）、邏輯高 ≥ 100（5 格）。
        assert!(meets_min_grid_size(work_area, g(0, 0, 10, 5), 1.0, &b));
        assert!(!meets_min_grid_size(work_area, g(0, 0, 9, 5), 1.0, &b));
        assert!(!meets_min_grid_size(work_area, g(0, 0, 10, 4), 1.0, &b));
        // 舊規則下 48 格寬（960 → zoom 1.92）要求高 ≥ 200 × 1.92；新規則仍只要 5 格。
        assert!(meets_min_grid_size(work_area, g(0, 0, 48, 5), 1.0, &b));
        // 縮放比例 2：邏輯長度減半，要加倍格數。
        assert!(meets_min_grid_size(work_area, g(0, 0, 20, 10), 2.0, &b));
        assert!(!meets_min_grid_size(work_area, g(0, 0, 19, 10), 2.0, &b));
        assert!(!meets_min_grid_size(work_area, g(0, 0, 20, 9), 2.0, &b));
    }

    /// 行情條（comfort_width＝None）的寬度最小格數仍看 min 寬（D1 表格：min 寬只用於最小格數）。
    #[test]
    fn meets_min_grid_size_uses_min_width_even_without_comfort_width() {
        let work_area = wa(0, 0, 960, 960);
        let quotes = zb(992.0, 60.0, None, 60.0);
        // 需邏輯寬 ≥ 496（25 格＝500）、邏輯高 ≥ 30（2 格＝40）。
        assert!(meets_min_grid_size(work_area, g(0, 0, 25, 2), 1.0, &quotes));
        assert!(!meets_min_grid_size(
            work_area,
            g(0, 0, 24, 2),
            1.0,
            &quotes
        ));
        assert!(!meets_min_grid_size(
            work_area,
            g(0, 0, 25, 1),
            1.0,
            &quotes
        ));
    }

    #[test]
    fn meets_min_grid_size_false_when_height_below_half_min_height() {
        let work_area = wa(0, 0, 1920, 1040);
        // 寬度充足，但高度只有 1 格（21 px），低於 min 高 176 的一半。
        let rect = GridRect {
            col: 0,
            row: 0,
            w: 20,
            h: 1,
        };
        assert!(!meets_min_grid_size(
            work_area,
            rect,
            1.0,
            &zb(352.5, 176.0, Some(470.0), 264.0)
        ));
    }

    #[test]
    fn min_grid_size_result_itself_meets_min_grid_size() {
        // 找到的最小格數，套用同一判斷函式應該剛好合法——兩者必須自洽；且少一格（寬或高）就
        // 不合法（確實是「最小」）。
        for (work_area, scale) in [
            (wa(0, 0, 1920, 1040), 1.0),
            (wa(0, 0, 2560, 1516), 1.75),
            (wa(0, 0, 3840, 2088), 1.5),
            (wa(0, 0, 2256, 1432), 1.5),
            (wa(0, 0, 1366, 728), 1.0),
        ] {
            for (id, zoom_box) in (0..PRESET_RECTS.len()).map(preset_design) {
                let (min_w, min_h) = min_grid_size(work_area, scale, &zoom_box);
                assert!((1..=GRID).contains(&min_w));
                assert!((1..=GRID).contains(&min_h));
                assert!(
                    meets_min_grid_size(work_area, g(0, 0, min_w, min_h), scale, &zoom_box),
                    "{id}：min_grid_size 算出的 {min_w}x{min_h} 應該自己也判定合法"
                );
                if min_w > 1 {
                    assert!(
                        !meets_min_grid_size(
                            work_area,
                            g(0, 0, min_w - 1, min_h),
                            scale,
                            &zoom_box
                        ),
                        "{id}：寬少一格應不合法"
                    );
                }
                if min_h > 1 {
                    assert!(
                        !meets_min_grid_size(
                            work_area,
                            g(0, 0, min_w, min_h - 1),
                            scale,
                            &zoom_box
                        ),
                        "{id}：高少一格應不合法"
                    );
                }
            }
        }
    }

    #[test]
    fn min_grid_size_and_meets_handle_invalid_inputs_without_panicking() {
        let work_area = wa(0, 0, 1920, 1040);
        for v in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            for b in [
                zb(v, 100.0, Some(200.0), 150.0),
                zb(100.0, v, Some(200.0), 150.0),
            ] {
                let (w, h) = min_grid_size(work_area, 1.0, &b);
                assert!((1..=GRID).contains(&w) && (1..=GRID).contains(&h), "{b:?}");
                let _ = meets_min_grid_size(work_area, g(0, 0, 5, 5), 1.0, &b);
            }
            let (w, h) = min_grid_size(work_area, v, &list_box());
            assert!(
                (1..=GRID).contains(&w) && (1..=GRID).contains(&h),
                "scale={v}"
            );
        }
        let (w, h) = min_grid_size(wa(0, 0, 0, -5), 1.0, &list_box());
        assert_eq!((w, h), (GRID, GRID), "工作區退化時退回 GRID（不代表合法）");
    }

    // ── widget-adaptive-zoom-and-grid design.md D3：新最小格數與舊條件的關係 ─────────

    /// 舊條件（升級前的 `meets_min_grid_size`，以參考實作保留在測試內）：邏輯寬 ≥ 設計寬／2，
    /// 且邏輯高 ≥ 設計最小高 × clamp(邏輯寬 ÷ 設計寬, 0.5, 3)。
    fn old_meets_min_grid_size(
        work_area: PhysicalRect,
        rect: GridRect,
        scale: f64,
        design_width: f64,
        design_min_height: f64,
    ) -> bool {
        let phys = grid_rect_to_physical(work_area, rect);
        let lw = f64::from(phys.width) / scale;
        let lh = f64::from(phys.height) / scale;
        let zoom = (lw / design_width).clamp(0.5, 3.0);
        const EPS: f64 = 1e-9;
        lw + EPS >= design_width * 0.5 && lh + EPS >= design_min_height * zoom
    }

    /// 升級前的 `WIDGET_SPECS`（設計寬、設計最小高；高度含上下 gap 8 px），順序同
    /// [`crate::settings::WIDGET_IDS`]。
    const OLD_DESIGN: [(&str, f64, f64); 10] = [
        ("clock", 500.0, 156.0),
        ("macro", 500.0, 216.0),
        ("fixed", 470.0, 176.0),
        ("dynamic", 470.0, 176.0),
        ("quotes", 992.0, 60.0),
        ("custom1", 470.0, 136.0),
        ("custom2", 470.0, 136.0),
        ("custom3", 470.0, 136.0),
        ("custom4", 470.0, 136.0),
        ("custom5", 470.0, 136.0),
    ];

    /// 掃描用的工作區 × 縮放：常見機種解析度 × 常見縮放 × 工作列高度（0／32／40／48 邏輯 px），
    /// 另加寬鬆網格（寬 800–4000 每 97 px × 高 600–2400 每 61 px，縮放 1／1.5／2.25；步長取
    /// 質數，避免全落在 48 的倍數上）。
    fn old_vs_new_sweep() -> Vec<(PhysicalRect, f64)> {
        let mut cases = Vec::new();
        let screens = [
            (1024, 768),
            (1280, 720),
            (1280, 800),
            (1366, 768),
            (1440, 900),
            (1536, 864),
            (1600, 900),
            (1680, 1050),
            (1920, 1080),
            (1920, 1200),
            (2160, 1440),
            (2256, 1504),
            (2560, 1080),
            (2560, 1440),
            (2560, 1600),
            (2736, 1824),
            (2880, 1800),
            (2880, 1920),
            (3000, 2000),
            (3440, 1440),
            (3840, 2160),
            (3840, 2400),
            (5120, 2880),
        ];
        let scales = [1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 3.0];
        for (w, h) in screens {
            for scale in scales {
                for taskbar in [0.0_f64, 32.0, 40.0, 48.0] {
                    let tb = (taskbar * scale).round() as i32;
                    cases.push((wa(0, 0, w, (h - tb).max(1)), scale));
                }
            }
        }
        for w in (800..=4000).step_by(97) {
            for h in (600..=2400).step_by(61) {
                for scale in [1.0, 1.5, 2.25] {
                    cases.push((wa(0, 0, w, h), scale));
                }
            }
        }
        cases
    }

    /// 屬性測試（D3）：`min_height ≤ 舊設計最小高` 的小工具（時鐘以外九個），舊條件合法的矩形
    /// 在新條件下一定合法——既有記錄不會因升級變成「小於最小格數」。掃描 [`old_vs_new_sweep`]
    /// 的每組工作區 × 縮放、每種格數 1–48 × 1–48，起點取 (0,0) 與 (5,3)（格線 floor 取整使同
    /// 格數在不同起點的實體長度可差 1 px）。時鐘的已知窄帶見
    /// [`clock_old_legal_new_illegal_narrow_band_is_the_documented_d3_exception`]。
    #[test]
    fn new_min_size_is_never_stricter_than_old_for_widgets_whose_min_height_did_not_grow() {
        let specs = &crate::widgets::WIDGET_SPECS;
        let covered: Vec<&str> = OLD_DESIGN
            .iter()
            .zip(specs.iter())
            .filter(|((_, _, old_h), spec)| spec.zoom_box.min_height <= *old_h)
            .map(|((id, _, _), _)| *id)
            .collect();
        assert_eq!(
            covered,
            vec![
                "macro", "fixed", "dynamic", "quotes", "custom1", "custom2", "custom3", "custom4",
                "custom5"
            ],
            "前提：只有時鐘的 min_height 比舊設計最小高大（D3）"
        );
        // 數值相同的小工具（台股固定／動態、五個擴充插槽）只掃一次，控制測試時間。
        let mut unique: Vec<(&str, f64, f64, ZoomBox)> = Vec::new();
        for ((id, old_w, old_h), spec) in OLD_DESIGN.iter().zip(specs.iter()) {
            assert_eq!(*id, spec.id, "OLD_DESIGN 順序應與 WIDGET_SPECS 相同");
            if spec.zoom_box.min_height > *old_h {
                continue;
            }
            if !unique
                .iter()
                .any(|&(_, w, h, b)| w == *old_w && h == *old_h && b == spec.zoom_box)
            {
                unique.push((id, *old_w, *old_h, spec.zoom_box));
            }
        }
        let mut old_legal_count = 0usize;
        for (work_area, scale) in old_vs_new_sweep() {
            for &(id, old_w, old_h, zoom_box) in &unique {
                for (col, row) in [(0, 0), (5, 3)] {
                    for w in 1..=(GRID - col) {
                        for h in 1..=(GRID - row) {
                            let rect = g(col, row, w, h);
                            if !old_meets_min_grid_size(work_area, rect, scale, old_w, old_h) {
                                continue;
                            }
                            old_legal_count += 1;
                            assert!(
                                meets_min_grid_size(work_area, rect, scale, &zoom_box),
                                "{id}：{rect:?} 在 {work_area:?}＠{scale} 舊合法、新不合法"
                            );
                        }
                    }
                }
            }
        }
        assert!(
            old_legal_count > 1_000_000,
            "前提：掃描量足夠（{old_legal_count}）"
        );
    }

    /// D3 記載的已知窄帶（task A 反例，commit ceb55a3 的 design.md 裁決）：時鐘 min_height 由
    /// 156 提高到 160（task 2.1 字型餘裕），舊倍率約 0.5–0.513 時舊條件只要求高 ≥ 156 × zoom
    /// （78–80），新條件要求 ≥ 80。例：2256×1504＠150%、工作列 48（工作區 2256×1432）上的 8×4，
    /// 也就是舊 `min_grid_size` 在這台算出的時鐘最小格數。這類既有記錄由推導第一階段照原位
    /// 放置（只檢查範圍與碰撞），見 `resolve_keeps_record_below_min_size_in_place_with_zoom_half`。
    #[test]
    fn clock_old_legal_new_illegal_narrow_band_is_the_documented_d3_exception() {
        let work_area = wa(0, 0, 2256, 1432);
        let rect = g(0, 0, 8, 4);
        let clock = crate::widgets::widget_spec("clock")
            .expect("時鐘規格")
            .zoom_box;
        let phys = grid_rect_to_physical(work_area, rect);
        assert_eq!((phys.width, phys.height), (376, 119), "邏輯 250.67 × 79.33");
        assert!(old_meets_min_grid_size(work_area, rect, 1.5, 500.0, 156.0));
        assert!(!meets_min_grid_size(work_area, rect, 1.5, &clock));
        // 舊 min_grid_size 在這台算出的時鐘最小格數就是 8×4（舊寬門檻 250、zoom≈0.5013）。
        assert!(!old_meets_min_grid_size(
            work_area,
            g(0, 0, 7, 4),
            1.5,
            500.0,
            156.0
        ));
        assert!(!old_meets_min_grid_size(
            work_area,
            g(0, 0, 8, 3),
            1.5,
            500.0,
            156.0
        ));
        // 新規則下時鐘最小格數：寬 106 邏輯 px（4 格＝188 px／1.5＝125.3）、高 80（5 格）。
        assert_eq!(min_grid_size(work_area, 1.5, &clock), (4, 5));

        // 窄帶之外沒有其他反例：掃描中時鐘所有「舊合法、新不合法」的矩形，舊倍率都 < 80 ÷ 156、
        // 邏輯高都落在 [78, 80)。
        let mut band_hits = 0usize;
        for (wa_case, scale) in old_vs_new_sweep() {
            for (col, row) in [(0, 0), (5, 3)] {
                for w in 1..=(GRID - col) {
                    for h in 1..=(GRID - row) {
                        let r = g(col, row, w, h);
                        if !old_meets_min_grid_size(wa_case, r, scale, 500.0, 156.0)
                            || meets_min_grid_size(wa_case, r, scale, &clock)
                        {
                            continue;
                        }
                        band_hits += 1;
                        let p = grid_rect_to_physical(wa_case, r);
                        let lw = f64::from(p.width) / scale;
                        let lh = f64::from(p.height) / scale;
                        assert!(
                            lw / 500.0 < 80.0 / 156.0 && (78.0 - 1e-9..80.0).contains(&lh),
                            "{r:?} 在 {wa_case:?}＠{scale}（邏輯 {lw}×{lh}）不在 D3 記載的窄帶內"
                        );
                    }
                }
            }
        }
        assert!(band_hits > 0, "前提：掃描確實遇到窄帶");
    }

    #[test]
    fn is_legal_grid_rect_rejects_out_of_range() {
        let work_area = wa(0, 0, 1920, 1040);
        let rect = GridRect {
            col: 40,
            row: 0,
            w: 10,
            h: 10,
        };
        assert!(!is_legal_grid_rect(
            work_area,
            rect,
            &[],
            1.0,
            &fixed_box(100.0, 50.0)
        ));
    }

    #[test]
    fn is_legal_grid_rect_rejects_overlap_with_others() {
        let work_area = wa(0, 0, 1920, 1040);
        let rect = GridRect {
            col: 0,
            row: 0,
            w: 10,
            h: 10,
        };
        let others = [GridRect {
            col: 5,
            row: 5,
            w: 10,
            h: 10,
        }];
        assert!(!is_legal_grid_rect(
            work_area,
            rect,
            &others,
            1.0,
            &fixed_box(100.0, 50.0)
        ));
    }

    #[test]
    fn is_legal_grid_rect_rejects_below_min_size() {
        let work_area = wa(0, 0, 1920, 1040);
        let rect = GridRect {
            col: 0,
            row: 0,
            w: 1,
            h: 1,
        };
        assert!(!is_legal_grid_rect(
            work_area,
            rect,
            &[],
            1.0,
            &fixed_box(470.0, 160.0)
        ));
    }

    #[test]
    fn is_legal_grid_rect_accepts_valid_non_overlapping_sized_rect() {
        let work_area = wa(0, 0, 1920, 1040);
        let rect = GridRect {
            col: 15,
            row: 1,
            w: 16,
            h: 9,
        };
        let others = [GridRect {
            col: 32,
            row: 1,
            w: 15,
            h: 17,
        }];
        assert!(is_legal_grid_rect(
            work_area,
            rect,
            &others,
            1.0,
            &fixed_box(500.0, 140.0)
        ));
    }

    // ── D7：移動對齊（四捨五入、保留寬高）────────────────────────────────────────────

    #[test]
    fn nearest_grid_index_rounds_down_below_half() {
        // 100 px 分 48 格：physical=1 → 0.48 格 → 捨去為 0。
        assert_eq!(nearest_grid_index(0, 100, 1), 0);
    }

    #[test]
    fn nearest_grid_index_rounds_up_above_half() {
        // physical=4 → 1.92 格 → 進位為 2。
        assert_eq!(nearest_grid_index(0, 100, 4), 2);
    }

    #[test]
    fn nearest_grid_index_rounds_exact_half_away_from_zero() {
        // 96 px 分 48 格（2px/格），physical=21 → 10.5 格，四捨五入進位為 11。
        assert_eq!(nearest_grid_index(0, 96, 21), 11);
    }

    #[test]
    fn nearest_grid_index_offsets_by_work_area_origin() {
        assert_eq!(nearest_grid_index(-1920, 960, -1920 + 480), 24);
    }

    #[test]
    fn align_move_preserves_width_and_height_grid_count() {
        // 960×960 工作區：一格 20 px。x=101 → 5.05 → col 5（ceil 會得 6）；y=199 → 9.95 → row 10
        // （floor 會得 9）——精確值同時擋住 floor 與 ceil 兩種取整方向錯誤。
        let monitor = monitor_at("m", wa(0, 0, 960, 960), 1.0, true);
        let rect = align_move(&monitor, 101, 199, 16, 9);
        assert_eq!(
            rect,
            GridRect {
                col: 5,
                row: 10,
                w: 16,
                h: 9
            },
            "左上角四捨五入到最近格線，格數不變"
        );
    }

    #[test]
    fn align_move_clamps_top_left_so_rect_stays_in_bounds() {
        let monitor = monitor_at("m", wa(0, 0, 960, 960), 1.0, true);
        // 左上角對齊後若會讓右／下緣超出 48，應整個夾回界內、格數仍不變。
        let rect = align_move(&monitor, 10_000, 10_000, 10, 10);
        assert_eq!(
            rect,
            GridRect {
                col: 38,
                row: 38,
                w: 10,
                h: 10
            }
        );
    }

    #[test]
    fn monitor_index_for_point_picks_monitor_containing_point() {
        let primary = monitor_at("p", wa(0, 0, 1920, 1040), 1.0, true);
        let secondary = monitor_at("s", wa(-1920, 0, 1920, 1040), 1.0, false);
        let monitors = [primary, secondary];
        assert_eq!(monitor_index_for_point(&monitors, 100, 100), Some(0));
        assert_eq!(monitor_index_for_point(&monitors, -100, 100), Some(1));
    }

    #[test]
    fn monitor_index_for_point_falls_back_to_nearest_when_point_outside_all_work_areas() {
        let primary = monitor_at("p", wa(0, 0, 1920, 1040), 1.0, true);
        let secondary = monitor_at("s", wa(3000, 3000, 500, 500), 1.0, false);
        let monitors = [primary, secondary];
        // 點在兩台工作區之外，但明顯離 secondary 近得多。
        assert_eq!(monitor_index_for_point(&monitors, 2990, 2990), Some(1));
        // 明顯離 primary 近。
        assert_eq!(monitor_index_for_point(&monitors, 1900, 1000), Some(0));
    }

    // ── D7：調整大小對齊（只對齊被拖邊）──────────────────────────────────────────────

    /// 960 工作區、一格 20 px 的提議矩形：左上角 (x, y)，右下角 (right, bottom)。
    fn proposed_px(x: i32, y: i32, right: i32, bottom: i32) -> PhysicalRect {
        PhysicalRect {
            x,
            y,
            width: right - x,
            height: bottom - y,
        }
    }

    #[test]
    fn align_resize_right_edge_only_moves_right_boundary() {
        let monitor = monitor_at("m", wa(0, 0, 960, 960), 1.0, true);
        let original = GridRect {
            col: 10,
            row: 10,
            w: 10,
            h: 10,
        };
        // 右緣 610 px → 30.5 → 31（floor 得 30；誤用 x+width−1＝609 → 30.45 → 30）。
        let result = align_resize(
            &monitor,
            original,
            ResizeEdges::RIGHT,
            proposed_px(200, 200, 610, 400),
        );
        assert_eq!(
            result,
            GridRect {
                col: 10,
                row: 10,
                w: 21,
                h: 10
            }
        );
        // 右緣 605 px → 30.25 → 30（ceil 得 31）。
        let result = align_resize(
            &monitor,
            original,
            ResizeEdges::RIGHT,
            proposed_px(200, 200, 605, 400),
        );
        assert_eq!(
            result,
            GridRect {
                col: 10,
                row: 10,
                w: 20,
                h: 10
            }
        );
    }

    /// fix F1（review 7.1 L1）：不合法的 `original`（w ≤ 0、col ≥ 48 等）會讓 `clamp` 的下限
    /// 大於上限而 panic；`align_resize` 在 `WM_SIZING` 的 FFI 路徑上，panic 會 abort 行程。
    /// 改為原樣回傳 `original`，不 panic。
    #[test]
    fn align_resize_returns_original_unchanged_for_invalid_original() {
        let monitor = monitor_at("m", wa(0, 0, 960, 960), 1.0, true);
        let proposed = proposed_px(100, 100, 500, 500);
        let invalid = [
            GridRect {
                col: 0,
                row: 0,
                w: 0,
                h: 5,
            },
            GridRect {
                col: 0,
                row: 0,
                w: 5,
                h: -3,
            },
            GridRect {
                col: 48,
                row: 0,
                w: 1,
                h: 5,
            },
            GridRect {
                col: 0,
                row: 50,
                w: 5,
                h: 5,
            },
            GridRect {
                col: -5,
                row: 0,
                w: 2,
                h: 5,
            },
            GridRect {
                col: i32::MAX,
                row: i32::MAX,
                w: i32::MAX,
                h: i32::MAX,
            },
        ];
        let all_edges = [
            ResizeEdges::LEFT,
            ResizeEdges::RIGHT,
            ResizeEdges::TOP_LEFT,
            ResizeEdges::TOP_RIGHT,
            ResizeEdges::BOTTOM_LEFT,
            ResizeEdges::BOTTOM_RIGHT,
            ResizeEdges::TOP,
            ResizeEdges::BOTTOM,
        ];
        for original in invalid {
            for edges in all_edges {
                assert_eq!(
                    align_resize(&monitor, original, edges, proposed),
                    original,
                    "{original:?} × {edges:?}"
                );
            }
        }
    }

    #[test]
    fn align_resize_left_edge_keeps_right_edge_fixed() {
        let monitor = monitor_at("m", wa(0, 0, 960, 960), 1.0, true);
        let original = GridRect {
            col: 20,
            row: 10,
            w: 10,
            h: 10,
        };
        // 左緣 209 px → 10.45 → 10（ceil 得 11）；190 px → 9.5 → 10（floor 得 9）。右緣 30 不動。
        for x in [209, 190] {
            let result = align_resize(
                &monitor,
                original,
                ResizeEdges::LEFT,
                proposed_px(x, 200, 600, 400),
            );
            assert_eq!(
                result,
                GridRect {
                    col: 10,
                    row: 10,
                    w: 20,
                    h: 10
                },
                "左緣 {x} px"
            );
        }
    }

    #[test]
    fn align_resize_corner_moves_both_dragged_edges_only() {
        let monitor = monitor_at("m", wa(0, 0, 960, 960), 1.0, true);
        let original = GridRect {
            col: 10,
            row: 10,
            w: 10,
            h: 10,
        };
        // 拖右上角：上緣 29 px → 1.45 → 1、右緣 610 px → 31；左緣 10 與下緣 20 不動（提議矩形的
        // 左／下緣故意給別的值，未拖的邊不應受影響）。
        let result = align_resize(
            &monitor,
            original,
            ResizeEdges::TOP_RIGHT,
            proposed_px(333, 29, 610, 777),
        );
        assert_eq!(
            result,
            GridRect {
                col: 10,
                row: 1,
                w: 21,
                h: 19
            }
        );
        // 上緣 30 px → 1.5 → 2（floor 得 1）。
        let result = align_resize(
            &monitor,
            original,
            ResizeEdges::TOP_RIGHT,
            proposed_px(333, 30, 610, 777),
        );
        assert_eq!(
            result,
            GridRect {
                col: 10,
                row: 2,
                w: 21,
                h: 18
            }
        );
    }

    // ── D7：找空位（右上優先，記錄格數→最小格數）────────────────────────────────────

    #[test]
    fn find_empty_slot_prefers_top_right_most_candidate() {
        // 空 48x48 網格找 10x10：候選 row 由 0 往下、同列 col 由 48-10=38 往 0，
        // 第一個候選（row0, col38）即應該被採用。
        let slot = find_empty_slot(10, 10, &[]).expect("空網格必有空位");
        assert_eq!(
            slot,
            GridRect {
                col: 38,
                row: 0,
                w: 10,
                h: 10
            }
        );
    }

    #[test]
    fn find_empty_slot_scans_columns_right_to_left_before_moving_down() {
        // 整條 row0 都被佔用（不論欄位），row0 找不到任何候選，下一個候選才會落到 row1
        // 的最右欄——藉此驗證是先把 row0 整列的欄位都試過（col 由 48-10=38 往 0）才換列，
        // 不是找到第一個 row 有任何空隙就提早跳列。
        let occupied = [GridRect {
            col: 0,
            row: 0,
            w: GRID,
            h: 1,
        }];
        let slot = find_empty_slot(10, 10, &occupied).expect("row0 被佔滿後應找到 row1 的空位");
        assert_eq!(
            slot,
            GridRect {
                col: 38,
                row: 1,
                w: 10,
                h: 10
            }
        );
    }

    #[test]
    fn find_empty_slot_returns_none_when_grid_fully_occupied() {
        let occupied = [GridRect {
            col: 0,
            row: 0,
            w: 48,
            h: 48,
        }];
        assert_eq!(find_empty_slot(10, 10, &occupied), None);
    }

    #[test]
    fn find_empty_slot_rejects_invalid_size() {
        assert_eq!(find_empty_slot(0, 5, &[]), None, "w 必須 >= 1");
        assert_eq!(find_empty_slot(5, 49, &[]), None, "h 不可超過 48");
    }

    #[test]
    fn find_slot_preferring_record_size_uses_record_size_first() {
        let slot = find_slot_preferring_record_size((10, 10), (5, 5), &[]).expect("應找到空位");
        assert_eq!((slot.w, slot.h), (10, 10), "有空間時優先用記錄格數");
    }

    #[test]
    fn find_slot_preferring_record_size_falls_back_to_min_size() {
        // 佔用讓 10x10 放不下、但 5x5 放得下：唯一剩餘空間是右下角 5x5。
        let occupied = [
            GridRect {
                col: 0,
                row: 0,
                w: 48,
                h: 43,
            },
            GridRect {
                col: 5,
                row: 43,
                w: 43,
                h: 5,
            },
        ];
        let slot = find_slot_preferring_record_size((10, 10), (5, 5), &occupied)
            .expect("記錄格數放不下時應退回最小格數再找");
        assert_eq!((slot.w, slot.h), (5, 5));
    }

    #[test]
    fn find_slot_preferring_record_size_none_when_even_min_size_does_not_fit() {
        let occupied = [GridRect {
            col: 0,
            row: 0,
            w: 48,
            h: 48,
        }];
        assert_eq!(
            find_slot_preferring_record_size((10, 10), (5, 5), &occupied),
            None
        );
    }

    // ── D7／D9：開啟小工具找空位（task 7.4）─────────────────────────────────────────

    #[test]
    fn placement_for_opening_widget_relocates_when_record_overlaps_native_widget() {
        // clash 相交、所屬顯示器存在、有空位可換——回傳新格子，且新格子與 clash 不相交。
        let monitor = monitor_at("m", wa(0, 0, 1920, 1040), 1.0, true);
        let clash = GridRect {
            col: 0,
            row: 0,
            w: 10,
            h: 10,
        };
        let result = placement_for_opening_widget(
            &[monitor],
            &MonitorId::Primary,
            clash,
            &fixed_box(100.0, 50.0),
            &[(MonitorId::Primary, clash)],
        );
        let rect = result.expect("應找到空位").expect("應回傳新格子");
        assert_ne!(rect, clash, "應換到別處，而非留在相交的記錄格子");
        assert!(!rects_overlap(rect, clash));
        assert_eq!((rect.w, rect.h), (10, 10), "有空間時優先用記錄格數");
    }

    #[test]
    fn placement_for_opening_widget_rejects_when_even_min_size_does_not_fit() {
        // 整個 48x48 都被佔滿，連最小格數都放不下。
        let monitor = monitor_at("m", wa(0, 0, 1920, 1040), 1.0, true);
        let full = GridRect {
            col: 0,
            row: 0,
            w: GRID,
            h: GRID,
        };
        let target = GridRect {
            col: 0,
            row: 0,
            w: 10,
            h: 10,
        };
        let result = placement_for_opening_widget(
            &[monitor],
            &MonitorId::Primary,
            target,
            &fixed_box(100.0, 50.0),
            &[(MonitorId::Primary, full)],
        );
        assert_eq!(result, Err(()), "連最小格數都找不到空位應回傳 Err");
    }

    #[test]
    fn placement_for_opening_widget_keeps_record_when_target_monitor_missing() {
        // 記錄顯示器已拔除（Device 找不到相符識別）：不找空位、不改記錄，即使有其他小工具
        // 記錄在（退回後的）主螢幕上與它相交。
        let primary = monitor_at("primary", wa(0, 0, 1920, 1040), 1.0, true);
        let clash = GridRect {
            col: 0,
            row: 0,
            w: 10,
            h: 10,
        };
        let result = placement_for_opening_widget(
            &[primary],
            &MonitorId::Device("unplugged".to_string()),
            clash,
            &fixed_box(100.0, 50.0),
            &[(MonitorId::Primary, clash)],
        );
        assert_eq!(
            result,
            Ok(None),
            "所屬顯示器不存在時不應找空位、不應回傳需要改記錄的結果"
        );
    }

    #[test]
    fn placement_for_opening_widget_keeps_record_when_no_overlap() {
        let monitor = monitor_at("m", wa(0, 0, 1920, 1040), 1.0, true);
        let target = GridRect {
            col: 0,
            row: 0,
            w: 10,
            h: 10,
        };
        let elsewhere = GridRect {
            col: 20,
            row: 20,
            w: 10,
            h: 10,
        };
        let result = placement_for_opening_widget(
            &[monitor],
            &MonitorId::Primary,
            target,
            &fixed_box(100.0, 50.0),
            &[(MonitorId::Primary, elsewhere)],
        );
        assert_eq!(result, Ok(None), "不相交時應照常開啟，不改記錄");
    }

    #[test]
    fn placement_for_opening_widget_ignores_others_moved_from_elsewhere() {
        // `others` 裡一筆記錄在已拔除的顯示器上（退回主螢幕、native=false）：不算「記錄就在
        // 該顯示器上」，即使物理上與 target 相交，也不應觸發找空位。
        let primary = monitor_at("primary", wa(0, 0, 1920, 1040), 1.0, true);
        let clash = GridRect {
            col: 0,
            row: 0,
            w: 10,
            h: 10,
        };
        let result = placement_for_opening_widget(
            &[primary],
            &MonitorId::Primary,
            clash,
            &fixed_box(100.0, 50.0),
            &[(MonitorId::Device("unplugged".to_string()), clash)],
        );
        assert_eq!(
            result,
            Ok(None),
            "從別處換來者的記錄不算「記錄就在該顯示器上」，不應被拿來比對"
        );
    }

    // ── D9：實際位置推導 ─────────────────────────────────────────────────────────

    /// 推導輸入：倍率框取「舒適框＝最小框」（推導測試只關心格子與最小格數，不關心自適應）。
    fn widget(
        id: &'static str,
        monitor: MonitorId,
        rect: GridRect,
        min_width: f64,
        min_height: f64,
    ) -> GridWidgetInput {
        GridWidgetInput {
            id,
            monitor,
            record_rect: rect,
            zoom_box: fixed_box(min_width, min_height),
        }
    }

    /// widget-adaptive-zoom-and-grid design.md D3 窄帶的後果：記錄格子小於最小格數、但在界內
    /// 且不與他人相交時，第一階段只檢查範圍與碰撞，照原位放置——不移動、不隱藏；此時倍率被
    /// 夾在 0.5（上限 < 0.5），字級設定也拉不高。
    #[test]
    fn resolve_keeps_record_below_min_size_in_place_with_zoom_half() {
        let monitor = monitor_at("m", wa(0, 0, 2256, 1432), 1.5, true);
        let clock_box = crate::widgets::widget_spec("clock")
            .expect("時鐘規格")
            .zoom_box;
        let clock_record = g(0, 0, 8, 4);
        let tiny_record = g(30, 30, 1, 1);
        assert!(
            !meets_min_grid_size(monitor.work_area, clock_record, 1.5, &clock_box),
            "前提：時鐘記錄小於新最小格數"
        );
        assert!(!meets_min_grid_size(
            monitor.work_area,
            tiny_record,
            1.5,
            &list_box()
        ));
        let widgets = [
            GridWidgetInput {
                id: "clock",
                monitor: MonitorId::Primary,
                record_rect: clock_record,
                zoom_box: clock_box,
            },
            GridWidgetInput {
                id: "macro",
                monitor: MonitorId::Primary,
                record_rect: tiny_record,
                zoom_box: list_box(),
            },
            widget("other", MonitorId::Primary, g(10, 0, 16, 16), 375.0, 216.0),
        ];
        for font_scale in [1.0, 1.5] {
            let result =
                resolve_grid_placements(std::slice::from_ref(&monitor), &widgets, font_scale);
            for (i, record) in [(0, clock_record), (1, tiny_record)] {
                match result[i] {
                    ResolvedWidgetPlacement::Placed {
                        rect,
                        zoom,
                        moved_from_elsewhere,
                        ..
                    } => {
                        assert_eq!(rect, record, "字級 {font_scale}：應照原位放置");
                        assert_eq!(zoom, 0.5, "字級 {font_scale}：倍率應夾在 0.5");
                        assert!(!moved_from_elsewhere);
                    }
                    other => panic!("字級 {font_scale}：應放置而非隱藏：{other:?}"),
                }
            }
        }
    }

    /// 推導出的倍率＝[`content_zoom`]（實體矩形、該顯示器縮放、該小工具的框、傳入的字級）。
    #[test]
    fn resolve_zoom_uses_content_zoom_with_given_font_scale() {
        let monitor = monitor_at("m", wa(0, 0, 1920, 1040), 1.0, true);
        let input = GridWidgetInput {
            id: "macro",
            monitor: MonitorId::Primary,
            record_rect: g(0, 0, 16, 30),
            zoom_box: list_box(),
        };
        for font_scale in [0.7, 1.0, 1.2, 1.5] {
            let result = resolve_grid_placements(
                std::slice::from_ref(&monitor),
                std::slice::from_ref(&input),
                font_scale,
            );
            let ResolvedWidgetPlacement::Placed {
                physical_rect,
                zoom,
                ..
            } = result[0]
            else {
                panic!("應放置：{result:?}");
            };
            assert_eq!(
                zoom,
                content_zoom(
                    physical_rect.width,
                    physical_rect.height,
                    1.0,
                    &list_box(),
                    font_scale
                )
            );
        }
        // 有鑑別力：640×650 的清單框 auto＝1.28、cap＝1.71，字級 1.2 → 1.536 ≠ 字級 1.0。
        let z = |f| match resolve_grid_placements(
            std::slice::from_ref(&monitor),
            std::slice::from_ref(&input),
            f,
        )[0]
        {
            ResolvedWidgetPlacement::Placed { zoom, .. } => zoom,
            ResolvedWidgetPlacement::HiddenNoSpace => panic!("應放置"),
        };
        assert!(approx(z(1.0), 1.28));
        assert!(approx(z(1.2), 1.536));
    }

    #[test]
    fn resolve_assigns_each_widget_to_its_recorded_monitor() {
        let primary = monitor_at("primary", wa(0, 0, 1920, 1040), 1.0, true);
        let secondary = monitor_at("secondary", wa(-1920, 0, 1920, 1040), 1.0, false);
        let widgets = vec![
            widget(
                "a",
                MonitorId::Primary,
                GridRect {
                    col: 0,
                    row: 0,
                    w: 10,
                    h: 10,
                },
                100.0,
                50.0,
            ),
            widget(
                "b",
                MonitorId::Device("secondary".to_string()),
                GridRect {
                    col: 0,
                    row: 0,
                    w: 10,
                    h: 10,
                },
                100.0,
                50.0,
            ),
        ];
        let result = resolve_grid_placements(&[primary, secondary], &widgets, 1.0);
        assert!(matches!(
            result[0],
            ResolvedWidgetPlacement::Placed {
                monitor_index: 0,
                moved_from_elsewhere: false,
                ..
            }
        ));
        assert!(matches!(
            result[1],
            ResolvedWidgetPlacement::Placed {
                monitor_index: 1,
                moved_from_elsewhere: false,
                ..
            }
        ));
    }

    #[test]
    fn resolve_keeps_non_conflicting_records_at_their_recorded_rect() {
        let monitor = monitor_at("m", wa(0, 0, 1920, 1040), 1.0, true);
        let widgets = vec![
            widget("a", MonitorId::Primary, PRESET_RECTS[0], 500.0, 140.0),
            widget("b", MonitorId::Primary, PRESET_RECTS[1], 500.0, 200.0),
        ];
        let result = resolve_grid_placements(&[monitor], &widgets, 1.0);
        assert!(matches!(
            &result[0],
            ResolvedWidgetPlacement::Placed { rect, .. } if *rect == PRESET_RECTS[0]
        ));
        assert!(matches!(
            &result[1],
            ResolvedWidgetPlacement::Placed { rect, .. } if *rect == PRESET_RECTS[1]
        ));
    }

    #[test]
    fn resolve_conflicting_widget_gets_relocated_without_evicting_the_legitimate_holder() {
        // 兩個小工具的記錄格子完全重疊；先到（註冊表順序在前）的合法佔用者應保持原位，
        // 後到者被找空位換走，而不是反過來擠走前者。
        let monitor = monitor_at("m", wa(0, 0, 1920, 1040), 1.0, true);
        let clash = GridRect {
            col: 0,
            row: 0,
            w: 10,
            h: 10,
        };
        let widgets = vec![
            widget("a", MonitorId::Primary, clash, 100.0, 50.0),
            widget("b", MonitorId::Primary, clash, 100.0, 50.0),
        ];
        let result = resolve_grid_placements(&[monitor], &widgets, 1.0);
        let ResolvedWidgetPlacement::Placed { rect: rect_a, .. } = result[0] else {
            panic!("a 應該被放置：{:?}", result[0]);
        };
        let ResolvedWidgetPlacement::Placed { rect: rect_b, .. } = result[1] else {
            panic!("b 應該被找到空位放置：{:?}", result[1]);
        };
        assert_eq!(rect_a, clash, "先到的合法佔用者應保持記錄位置");
        assert_ne!(rect_b, clash, "後到者應被換到別處，而不是擠走前者");
        assert!(!rects_overlap(rect_a, rect_b));
        // 具體驗證找空位順序：右上優先，第一個候選 (col=38,row=0) 未與 a 重疊即採用。
        assert_eq!(
            rect_b,
            GridRect {
                col: 38,
                row: 0,
                w: 10,
                h: 10
            }
        );
    }

    /// fix F1（review 7.1 M3）：D9 分兩階段的理由——單次掃描邊放邊找空位，會讓衝突者找到的
    /// 空位佔走排序較後、記錄本身合法的小工具。c 的記錄就在 b「第一個會找到的空位」(38,0)；
    /// 兩階段實作下 c 留在記錄位置、b 落在別處。
    #[test]
    fn resolve_relocated_widget_does_not_take_a_later_widgets_legal_record() {
        let monitor = monitor_at("m", wa(0, 0, 1920, 1040), 1.0, true);
        let clash = GridRect {
            col: 0,
            row: 0,
            w: 10,
            h: 10,
        };
        let c_record = GridRect {
            col: 38,
            row: 0,
            w: 10,
            h: 10,
        };
        let widgets = vec![
            widget("a", MonitorId::Primary, clash, 100.0, 50.0),
            widget("b", MonitorId::Primary, clash, 100.0, 50.0),
            widget("c", MonitorId::Primary, c_record, 100.0, 50.0),
        ];
        let result = resolve_grid_placements(&[monitor], &widgets, 1.0);
        let rects: Vec<GridRect> = result
            .iter()
            .map(|r| match r {
                ResolvedWidgetPlacement::Placed { rect, .. } => *rect,
                other => panic!("三者都應被放置：{other:?}"),
            })
            .collect();
        assert_eq!(rects[0], clash, "a 保持記錄位置");
        assert_eq!(rects[2], c_record, "c 的記錄合法，不應被 b 找到的空位搶走");
        assert_eq!(
            rects[1],
            GridRect {
                col: 28,
                row: 0,
                w: 10,
                h: 10
            },
            "b 落在 a、c 之外的第一個空位"
        );
        assert_eq!(find_overlapping_pair(&rects), None);
    }

    #[test]
    fn resolve_primary_and_device_landing_on_same_monitor_both_treated_as_native() {
        // 使用者更換過主螢幕：一個小工具記錄 Primary、另一個記錄 Device(目前這台主螢幕的
        // 身分)，兩者現在其實是同一台——都算「記錄在這台」，衝突時純靠註冊表順序排開。
        let monitor = monitor_at("dev-1", wa(0, 0, 1920, 1040), 1.0, true);
        let clash = GridRect {
            col: 0,
            row: 0,
            w: 10,
            h: 10,
        };
        let widgets = vec![
            widget("p", MonitorId::Primary, clash, 100.0, 50.0),
            widget(
                "d",
                MonitorId::Device("dev-1".to_string()),
                clash,
                100.0,
                50.0,
            ),
        ];
        let result = resolve_grid_placements(&[monitor], &widgets, 1.0);
        match (&result[0], &result[1]) {
            (
                ResolvedWidgetPlacement::Placed {
                    rect: ra,
                    moved_from_elsewhere: ma,
                    ..
                },
                ResolvedWidgetPlacement::Placed {
                    rect: rb,
                    moved_from_elsewhere: mb,
                    ..
                },
            ) => {
                assert_eq!(*ra, clash, "先到的 Primary 記錄應保持原位");
                assert_ne!(*rb, clash, "後到的 Device 記錄應被換位");
                assert!(!ma, "Primary 命中目前主螢幕，屬於『記錄在這台』");
                assert!(
                    !mb,
                    "Device 命中同一台，屬於『記錄在這台』，不是『從別處換來』"
                );
            }
            other => panic!("兩者都應該被放置：{other:?}"),
        }
    }

    #[test]
    fn resolve_missing_device_falls_back_to_primary_and_marks_moved_from_elsewhere() {
        let primary = monitor_at("primary", wa(0, 0, 1920, 1040), 1.0, true);
        let widgets = vec![widget(
            "w",
            MonitorId::Device("unplugged".to_string()),
            GridRect {
                col: 0,
                row: 0,
                w: 10,
                h: 10,
            },
            100.0,
            50.0,
        )];
        let result = resolve_grid_placements(&[primary], &widgets, 1.0);
        assert!(matches!(
            result[0],
            ResolvedWidgetPlacement::Placed {
                monitor_index: 0,
                moved_from_elsewhere: true,
                ..
            }
        ));
    }

    /// fix F1（review 7.1 M1）：`desktop::monitors_from_tauri` 在 `primary_monitor()` 回 `None`
    /// 或主螢幕名稱對不到任何一台時，刻意不標任何主螢幕。此時 `Primary` 記錄應退回清單第一台
    /// （視為記錄在這台），找不到 Device 者也退回第一台（標記從別處換來），不可全被判成
    /// 空間不足。
    #[test]
    fn resolve_without_any_primary_flag_falls_back_to_first_monitor() {
        let first = monitor_at("first", wa(0, 0, 1920, 1040), 1.0, false);
        let second = monitor_at("second", wa(1920, 0, 1920, 1040), 1.0, false);
        let rect = |col| GridRect {
            col,
            row: 0,
            w: 10,
            h: 10,
        };
        let widgets = vec![
            widget("p", MonitorId::Primary, rect(0), 100.0, 50.0),
            widget(
                "gone",
                MonitorId::Device("unplugged".to_string()),
                rect(20),
                100.0,
                50.0,
            ),
            widget(
                "s",
                MonitorId::Device("second".to_string()),
                rect(0),
                100.0,
                50.0,
            ),
        ];
        let result = resolve_grid_placements(&[first, second], &widgets, 1.0);
        assert!(
            matches!(
                result[0],
                ResolvedWidgetPlacement::Placed {
                    monitor_index: 0,
                    moved_from_elsewhere: false,
                    ..
                }
            ),
            "Primary 應退回第一台並視為記錄在這台：{:?}",
            result[0]
        );
        assert!(
            matches!(
                result[1],
                ResolvedWidgetPlacement::Placed {
                    monitor_index: 0,
                    moved_from_elsewhere: true,
                    ..
                }
            ),
            "找不到 Device 應退回第一台並標記從別處換來：{:?}",
            result[1]
        );
        assert!(
            matches!(
                result[2],
                ResolvedWidgetPlacement::Placed {
                    monitor_index: 1,
                    moved_from_elsewhere: false,
                    ..
                }
            ),
            "找得到的 Device 照常對應：{:?}",
            result[2]
        );
    }

    /// fix F1（review 7.1 M1）：開啟小工具找空位用的是同一條退回鏈；沒有主螢幕旗標時，
    /// Primary 記錄仍視為「所屬顯示器存在」，相交時要找空位。
    #[test]
    fn placement_for_opening_widget_without_primary_flag_uses_first_monitor() {
        let first = monitor_at("first", wa(0, 0, 1920, 1040), 1.0, false);
        let clash = GridRect {
            col: 0,
            row: 0,
            w: 10,
            h: 10,
        };
        let result = placement_for_opening_widget(
            &[first],
            &MonitorId::Primary,
            clash,
            &fixed_box(100.0, 50.0),
            &[(MonitorId::Primary, clash)],
        );
        assert_eq!(
            result,
            Ok(Some(GridRect {
                col: 38,
                row: 0,
                w: 10,
                h: 10
            }))
        );
    }

    #[test]
    fn resolve_reconnecting_monitor_restores_original_position() {
        let secondary = monitor_at("secondary", wa(2000, 0, 1000, 800), 1.0, false);
        let primary = monitor_at("primary", wa(0, 0, 1920, 1040), 1.0, true);
        let widgets = vec![widget(
            "w",
            MonitorId::Device("secondary".to_string()),
            GridRect {
                col: 5,
                row: 5,
                w: 10,
                h: 10,
            },
            100.0,
            50.0,
        )];

        let with_both =
            resolve_grid_placements(&[primary.clone(), secondary.clone()], &widgets, 1.0);
        let unplugged = resolve_grid_placements(std::slice::from_ref(&primary), &widgets, 1.0);
        let reconnected = resolve_grid_placements(&[primary, secondary], &widgets, 1.0);

        assert!(matches!(
            with_both[0],
            ResolvedWidgetPlacement::Placed {
                monitor_index: 1,
                moved_from_elsewhere: false,
                ..
            }
        ));
        assert!(matches!(
            unplugged[0],
            ResolvedWidgetPlacement::Placed {
                monitor_index: 0,
                moved_from_elsewhere: true,
                ..
            }
        ));
        assert_eq!(
            reconnected, with_both,
            "接回後應完全復位（純函式、記錄位置未被覆寫）"
        );
    }

    /// fix F1（review 7.1 L3）：空間不足的 victim 隱藏且**不佔格**。victim 的記錄有一部分落在
    /// 唯一剩下的空白帶上；排序在它之後、同樣要找空位的 filler 需要整條空白帶（記錄格數＝
    /// GRID × 空白列數）。若隱藏者仍佔著記錄格子，filler 只能退到最小格數，就不會等於整條帶。
    #[test]
    fn resolve_insufficient_space_hides_widget_without_occupying_a_slot() {
        let monitor = monitor_at("m", wa(0, 0, 1920, 1040), 1.0, true);
        // 先用 min_grid_size 算出 victim 實際需要的最小格數，再故意只留一條「差一格」的
        // 空間，確保無論這個最小格數實際是多少，都必然放不下——不靠對特定數字的猜測。
        let (min_w, min_h) = min_grid_size(
            monitor.work_area,
            monitor.scale_factor,
            &fixed_box(470.0, 160.0),
        );
        let free_rows = min_h - 1;
        assert!(free_rows >= 1, "前提：至少留一列空白帶（min_h={min_h}）");
        let blocker_h = GRID - free_rows;
        let blocker = GridRect {
            col: 0,
            row: 0,
            w: GRID,
            h: blocker_h,
        };
        // 與 blocker 重疊一列（第一階段放不下），其餘伸進空白帶。
        let victim_record = GridRect {
            col: 0,
            row: blocker_h - 1,
            w: min_w,
            h: min_h,
        };
        let filler_record = GridRect {
            col: 0,
            row: 0,
            w: GRID,
            h: free_rows,
        };
        let free_band = GridRect {
            col: 0,
            row: blocker_h,
            w: GRID,
            h: free_rows,
        };

        let widgets = vec![
            widget("blocker", MonitorId::Primary, blocker, 100.0, 10.0),
            widget("victim", MonitorId::Primary, victim_record, 470.0, 160.0),
            widget("filler", MonitorId::Primary, filler_record, 10.0, 1.0),
        ];
        let result = resolve_grid_placements(&[monitor], &widgets, 1.0);

        assert!(matches!(
            &result[0],
            ResolvedWidgetPlacement::Placed { rect, .. } if *rect == blocker
        ));
        assert_eq!(
            result[1],
            ResolvedWidgetPlacement::HiddenNoSpace,
            "min_w={min_w} min_h={min_h} free_rows={free_rows}：僅剩空間應不足以安置 victim"
        );
        assert!(
            matches!(
                &result[2],
                ResolvedWidgetPlacement::Placed { rect, .. } if *rect == free_band
            ),
            "隱藏的 victim 不佔格，filler 應拿到整條空白帶：{:?}",
            result[2]
        );
    }

    /// fix F1（review 7.1 M4）：「放開成功後重新推導位置不變」的不變式（D7）：放開合法 ⇒ 新記錄
    /// 在 D9 第一階段一定放得下。情境含一個被換位的 native（b：記錄與 a 相交、實際被換到
    /// (38,0)），被拖的 c 放到 b 的**記錄**上（避開所有實際位置）必須判不合法——只比實際位置
    /// 的話會放行，重新推導後 c 才是合法佔用者、b 的記錄永遠回不去。放到合法位置後寫回記錄、
    /// 重新推導，c 必落在新記錄上，a、b 不受影響。比對集合由 [`check_placement_on`] 組
    /// （實際位置＋記錄就在該顯示器上的 native 記錄；隱藏者與從別處換來者的記錄不算），
    /// 鄰居清單在呼叫端 `widgets::drop_neighbors` 排除自己。
    #[test]
    fn resolve_after_successful_drop_rederiving_keeps_same_position() {
        let monitors = [monitor_at("m", wa(0, 0, 1920, 1040), 1.0, true)];
        let mut widgets = vec![
            widget("a", MonitorId::Primary, g(0, 0, 10, 10), 100.0, 50.0),
            widget("b", MonitorId::Primary, g(5, 0, 10, 10), 100.0, 50.0),
            widget("c", MonitorId::Primary, g(5, 20, 3, 5), 100.0, 50.0),
        ];
        let before = resolve_grid_placements(&monitors, &widgets, 1.0);
        let actual_of = |r: &ResolvedWidgetPlacement| match r {
            ResolvedWidgetPlacement::Placed {
                monitor_index,
                rect,
                ..
            } => Some((*monitor_index, *rect)),
            ResolvedWidgetPlacement::HiddenNoSpace => None,
        };
        assert_eq!(
            actual_of(&before[1]),
            Some((0, g(38, 0, 10, 10))),
            "前提：b 被換位"
        );
        // c 的鄰居＝a、b（排除自己）。
        let neighbors: Vec<DropNeighbor> = [0, 1]
            .iter()
            .map(|&i| {
                neighbor(
                    widgets[i].monitor.clone(),
                    widgets[i].record_rect,
                    actual_of(&before[i]),
                )
            })
            .collect();
        let drop =
            |col, row| legal_move_placement(&monitors, drop_at(col, row), (3, 5), &neighbors);

        assert_eq!(
            drop(11, 2),
            None,
            "落在被換位 native（b）的記錄上，應不合法"
        );
        assert_eq!(drop(40, 2), None, "落在 b 的實際位置上，應不合法");
        let placement = drop(20, 20).expect("空白處應合法");
        assert_eq!(placement.grid_rect(), g(20, 20, 3, 5));

        widgets[2].record_rect = placement.grid_rect();
        let rederived = resolve_grid_placements(&monitors, &widgets, 1.0);
        assert_eq!(
            actual_of(&rederived[2]),
            Some((0, g(20, 20, 3, 5))),
            "c 落在新記錄上"
        );
        assert_eq!(rederived[0], before[0], "a 不受影響");
        assert_eq!(rederived[1], before[1], "b 不受影響");
    }

    // ── 屬性測試：十個預設格座標套用在多種工作區尺寸 × 縮放比例 ──────────────────────

    /// 兩兩比對 `rects` 是否有任何一對相交，回傳第一對相交者的索引（供呼叫端組出好讀的錯誤
    /// 訊息）；全部不相交回傳 `None`。用 `enumerate` + 切片而非索引迴圈，避免對固定陣列的
    /// `needless_range_loop`。
    fn find_overlapping_pair(rects: &[GridRect]) -> Option<(usize, usize)> {
        for (i, a) in rects.iter().enumerate() {
            for (offset, b) in rects[i + 1..].iter().enumerate() {
                if rects_overlap(*a, *b) {
                    return Some((i, i + 1 + offset));
                }
            }
        }
        None
    }

    #[test]
    fn preset_rects_never_overlap_at_grid_level() {
        if let Some((i, j)) = find_overlapping_pair(&PRESET_RECTS) {
            panic!(
                "{} 與 {} 重疊：{:?} / {:?}",
                preset_design(i).0,
                preset_design(j).0,
                PRESET_RECTS[i],
                PRESET_RECTS[j]
            );
        }
    }

    #[test]
    fn preset_rects_are_all_in_grid_bounds() {
        for (i, rect) in PRESET_RECTS.iter().enumerate() {
            assert!(in_grid_bounds(*rect), "{}: {rect:?}", preset_design(i).0);
        }
    }

    #[test]
    fn preset_rects_never_overlap_in_physical_pixels_and_gaps_stay_open() {
        // task 7.2：正式預設格座標彼此都空一格（不共用格線），換算成實體像素後兩兩不相交，
        // 且空出的那一格在任何工作區尺寸下都留下正的像素間距（edge() 的取整不會把縫隙吃掉）。
        // 共用格線的兩矩形「同一像素、不重疊不留縫」由
        // `adjacent_grid_rects_share_exact_pixel_boundary_no_gap_no_overlap` 涵蓋。
        for work_area in [
            wa(0, 0, 1920, 1040),
            wa(0, 0, 2560, 1516),
            wa(0, 0, 3840, 2088),
            wa(-1680, 0, 1680, 1010),
        ] {
            let phys: Vec<PhysicalRect> = PRESET_RECTS
                .iter()
                .map(|r| grid_rect_to_physical(work_area, *r))
                .collect();
            for (i, a) in phys.iter().enumerate() {
                for (j, b) in phys.iter().enumerate().skip(i + 1) {
                    let disjoint = a.x + a.width <= b.x
                        || b.x + b.width <= a.x
                        || a.y + a.height <= b.y
                        || b.y + b.height <= a.y;
                    assert!(
                        disjoint,
                        "{} 與 {} 在 {work_area:?} 的實體矩形相交：{a:?} / {b:?}",
                        preset_design(i).0,
                        preset_design(j).0
                    );
                }
            }
            // custom1 與 custom2 之間空一列：實體間距 > 0。
            let (c1, c2) = (phys[5], phys[6]);
            assert!(c2.y - (c1.y + c1.height) > 0, "custom1／custom2 應留縫");
        }
    }

    /// 屬性矩陣（fix F1，review 7.1 M2／L4）：每一組都是明確的 `(工作區, 縮放比例)`，工作區依
    /// 該縮放扣掉工作列（[`workspace`]），另外每種縮放都附一個高寬比恰 0.53 的邊界工作區
    /// （2000×1060，不扣工作列）。不再用「同一實體尺寸換縮放、再以高寬比篩掉」的寫法——
    /// 那會讓 1920×1080 的 175%／200% 實際上沒被測到。
    fn property_matrix() -> Vec<(PhysicalRect, f64)> {
        let mut cases = Vec::new();
        for scale in [1.0, 1.25, 1.5, 1.75, 2.0] {
            for (w, h) in [
                (1920, 1080),
                (2560, 1440),
                (2560, 1600),
                (3840, 2160),
                (1680, 1050),
            ] {
                cases.push((workspace(w, h, scale), scale));
            }
            // 高寬比恰 0.53 的邊界（1060/2000 = 0.53），每種縮放都跑。
            cases.push((wa(0, 0, 2000, 1060), scale));
            // 格線不整除的工作區（2560×1530、3840×2100 都不是 48 的倍數）與非零原點。
            cases.push((wa(0, 0, 2560, 1530), scale));
            cases.push((wa(-3840, -60, 3840, 2100), scale));
        }
        cases
    }

    /// 實體矩形是否不相交（共邊不算相交）。
    fn physical_disjoint(a: PhysicalRect, b: PhysicalRect) -> bool {
        a.x + a.width <= b.x
            || b.x + b.width <= a.x
            || a.y + a.height <= b.y
            || b.y + b.height <= a.y
    }

    /// 屬性測試（task 7.1；fix F1 review 7.1 M2）：矩陣中每一組工作區 × 縮放，把十個預設格座標
    /// 實際換算成實體矩形後兩兩不相交，且每個實體矩形的四邊都恰好落在 D7 公式
    /// `起點 + floor(i × 長度 ÷ 48)` 的格線上（另以整數運算獨立計算，不呼叫 [`edge`]）。
    #[test]
    fn preset_rects_physical_rects_disjoint_and_on_grid_lines_for_every_workspace_and_scale() {
        let floor_line = |origin: i32, extent: i32, i: i32| {
            origin + (i64::from(i) * i64::from(extent) / 48) as i32
        };
        for (work_area, scale) in property_matrix() {
            let phys: Vec<PhysicalRect> = PRESET_RECTS
                .iter()
                .map(|r| grid_rect_to_physical(work_area, *r))
                .collect();
            for (i, (rect, p)) in PRESET_RECTS.iter().zip(&phys).enumerate() {
                let expected = (
                    floor_line(work_area.x, work_area.width, rect.col),
                    floor_line(work_area.y, work_area.height, rect.row),
                    floor_line(work_area.x, work_area.width, rect.right()),
                    floor_line(work_area.y, work_area.height, rect.bottom()),
                );
                assert_eq!(
                    (p.x, p.y, p.x + p.width, p.y + p.height),
                    expected,
                    "{} 在 {work_area:?}＠{scale} 的實體邊應落在格線上",
                    preset_design(i).0
                );
            }
            for (i, a) in phys.iter().enumerate() {
                for (j, b) in phys.iter().enumerate().skip(i + 1) {
                    assert!(
                        physical_disjoint(*a, *b),
                        "{} 與 {} 在 {work_area:?}＠{scale} 的實體矩形相交：{a:?} / {b:?}",
                        preset_design(i).0,
                        preset_design(j).0
                    );
                }
            }
        }
    }

    /// 屬性測試（fix F1 review 7.1 M2）：矩陣中每一組工作區 × 縮放，任何一條格線 `i`（1–47）
    /// 兩側的相鄰小工具（水平：`[0,i)` 與 `[i,48)`；垂直同理）換算成實體矩形後，共用的那條邊
    /// 落在同一個像素——不重疊也不留縫。
    #[test]
    fn adjacent_rects_share_exact_pixel_boundary_for_every_workspace_and_scale() {
        for (work_area, scale) in property_matrix() {
            for i in 1..GRID {
                let left = grid_rect_to_physical(
                    work_area,
                    GridRect {
                        col: 0,
                        row: 0,
                        w: i,
                        h: 5,
                    },
                );
                let right = grid_rect_to_physical(
                    work_area,
                    GridRect {
                        col: i,
                        row: 0,
                        w: GRID - i,
                        h: 5,
                    },
                );
                assert_eq!(
                    left.x + left.width,
                    right.x,
                    "格線 {i} 在 {work_area:?}＠{scale}：左右相鄰應共用同一像素邊"
                );
                assert!(physical_disjoint(left, right));
                let top = grid_rect_to_physical(
                    work_area,
                    GridRect {
                        col: 0,
                        row: 0,
                        w: 5,
                        h: i,
                    },
                );
                let bottom = grid_rect_to_physical(
                    work_area,
                    GridRect {
                        col: 0,
                        row: i,
                        w: 5,
                        h: GRID - i,
                    },
                );
                assert_eq!(
                    top.y + top.height,
                    bottom.y,
                    "格線 {i} 在 {work_area:?}＠{scale}：上下相鄰應共用同一像素邊"
                );
                assert!(physical_disjoint(top, bottom));
            }
        }
    }

    /// 屬性測試主體：矩陣中高寬比 >= 0.53 的每一組工作區 × 縮放（含本機筆電 2560×1600@175%、
    /// 4K@150%、1920×1080@100%，以及每種縮放下高寬比恰 0.53 的邊界工作區）套用十個預設格座標
    /// 時，每個小工具都不小於其最小格數（design.md D7：寬螢幕（高寬比小）可用設計高度較少，只
    /// 保證 >=0.53 的工作區）。
    #[test]
    fn preset_rects_fit_min_size_on_workspaces_with_aspect_ratio_at_least_0_53() {
        let mut boundary_scales = Vec::new();
        for (work_area, scale) in property_matrix() {
            let ratio = f64::from(work_area.height) / f64::from(work_area.width);
            if ratio < 0.53 {
                continue;
            }
            if work_area.width == 2000 && work_area.height == 1060 {
                boundary_scales.push(scale);
            }
            for (idx, rect) in PRESET_RECTS.iter().enumerate() {
                let (_, zoom_box) = preset_design(idx);
                assert!(
                    meets_min_grid_size(work_area, *rect, scale, &zoom_box),
                    "{}：{rect:?} 在工作區 {work_area:?}＠{scale} 應不小於最小格數（高寬比 {ratio:.3}）",
                    preset_design(idx).0
                );
            }
        }
        assert_eq!(
            boundary_scales,
            vec![1.0, 1.25, 1.5, 1.75, 2.0],
            "高寬比恰 0.53 的邊界工作區應在每種縮放都受測"
        );
    }

    // ── task 7.2：拖曳結束 → 新記錄位置（移動對齊、保留格數）──────────────────────────

    #[test]
    fn placement_after_move_keeps_w_h_and_aligns_top_left_to_nearest_grid_line() {
        // 1920×1040 工作區：一格 40×(1040/48≈21.67) 實體像素。
        let m = monitor_at("dev-a", wa(0, 0, 1920, 1040), 1.0, true);
        // 左上角 (x=417, y=110)：417/40=10.425 → col 10；110×48/1040=5.08 → row 5。
        let dropped = PhysicalRect {
            x: 417,
            y: 110,
            width: 333, // 放開時的矩形尺寸不影響格數（保留 keep_w／keep_h）
            height: 77,
        };
        let placement = placement_after_move(std::slice::from_ref(&m), dropped, 16, 9)
            .expect("顯示器識別已解析");
        assert_eq!(placement.monitor, MonitorId::Device("dev-a".to_string()));
        assert_eq!(
            placement.grid_rect(),
            GridRect {
                col: 10,
                row: 5,
                w: 16,
                h: 9
            }
        );
    }

    #[test]
    fn placement_after_move_uses_monitor_under_rect_center() {
        // 矩形左上角在左側顯示器、中心在右側顯示器 → 歸屬右側（D7：取矩形中心所在者）。
        let left = monitor_at("left", wa(-1920, 0, 1920, 1040), 1.0, false);
        let right = monitor_at("right", wa(0, 0, 1920, 1040), 1.0, true);
        let dropped = PhysicalRect {
            x: -100,
            y: 0,
            width: 640,
            height: 200,
        };
        let placement = placement_after_move(&[left, right], dropped, 16, 9).expect("識別已解析");
        assert_eq!(placement.monitor, MonitorId::Device("right".to_string()));
        assert_eq!(placement.col, 0, "左上角超出工作區左緣，夾回 col 0");
        assert_eq!((placement.w, placement.h), (16, 9));
    }

    #[test]
    fn placement_after_move_clamps_so_rect_stays_in_grid() {
        let m = monitor_at("dev-a", wa(0, 0, 1920, 1040), 1.0, true);
        // 拖到右下角外側：保留 16×9，左上角夾到 (48-16, 48-9)。
        let dropped = PhysicalRect {
            x: 1900,
            y: 1030,
            width: 640,
            height: 195,
        };
        let placement =
            placement_after_move(std::slice::from_ref(&m), dropped, 16, 9).expect("識別已解析");
        assert_eq!(
            placement.grid_rect(),
            GridRect {
                col: 32,
                row: 39,
                w: 16,
                h: 9
            }
        );
    }

    #[test]
    fn placement_after_move_on_unresolved_monitor_returns_none() {
        // 查不到穩定識別的顯示器沒有可保存的身分（不得以 DISPLAYn 暫代），呼叫端保留原記錄。
        let unresolved = MonitorInfo {
            id: None,
            work_area: wa(0, 0, 1920, 1040),
            scale_factor: 1.0,
            is_primary: true,
        };
        let dropped = PhysicalRect {
            x: 100,
            y: 100,
            width: 640,
            height: 195,
        };
        assert_eq!(placement_after_move(&[unresolved], dropped, 16, 9), None);
    }

    #[test]
    fn placement_after_move_with_no_monitors_returns_none() {
        let dropped = PhysicalRect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        assert_eq!(placement_after_move(&[], dropped, 16, 9), None);
    }

    #[test]
    fn placement_after_move_then_resolve_puts_widget_exactly_on_aligned_cells() {
        // 放開 → 寫回 → 重新推導：推導結果的格子＝寫回的格子，實體矩形落在格線上。
        let m = monitor_at("dev-a", wa(0, 0, 2560, 1516), 1.75, true);
        let dropped = PhysicalRect {
            x: 1234,
            y: 321,
            width: 800,
            height: 300,
        };
        let placement =
            placement_after_move(std::slice::from_ref(&m), dropped, 16, 9).expect("識別已解析");
        let resolved = resolve_grid_placements(
            std::slice::from_ref(&m),
            &[widget(
                "clock",
                placement.monitor.clone(),
                placement.grid_rect(),
                500.0,
                140.0,
            )],
            1.0,
        );
        let ResolvedWidgetPlacement::Placed {
            rect,
            physical_rect,
            ..
        } = resolved[0]
        else {
            panic!("應放得下：{resolved:?}");
        };
        assert_eq!(rect, placement.grid_rect());
        assert_eq!(physical_rect, grid_rect_to_physical(m.work_area, rect));
    }

    // ── task 7.5：編輯版面移動的合法判斷（legal_move_placement）──────────────────────

    fn neighbor(
        record_monitor: MonitorId,
        record_rect: GridRect,
        actual: Option<(usize, GridRect)>,
    ) -> DropNeighbor {
        DropNeighbor {
            record_monitor,
            record_rect,
            actual,
        }
    }

    fn g(col: i32, row: i32, w: i32, h: i32) -> GridRect {
        GridRect { col, row, w, h }
    }

    /// 1920×1040＠100%：一格 40×21.67 實體像素；(col, row) 的左上角實體座標。
    fn cell_origin(col: i32, row: i32) -> (i32, i32) {
        (edge(0, 1920, col), edge(0, 1040, row))
    }

    fn drop_at(col: i32, row: i32) -> PhysicalRect {
        let (x, y) = cell_origin(col, row);
        PhysicalRect {
            x,
            y,
            width: 640,
            height: 195,
        }
    }

    #[test]
    fn legal_move_to_empty_area_keeps_w_h_and_aligns() {
        let m = monitor_at("a", wa(0, 0, 1920, 1040), 1.0, true);
        let others = [neighbor(
            MonitorId::Device("a".into()),
            g(30, 0, 10, 10),
            Some((0, g(30, 0, 10, 10))),
        )];
        // 左上角落在 (5, 20) 附近再偏 3 像素，應 round 回 (5, 20)。
        let mut dropped = drop_at(5, 20);
        dropped.x += 3;
        dropped.y -= 3;
        let p = legal_move_placement(std::slice::from_ref(&m), dropped, (16, 9), &others)
            .expect("空白處應合法");
        assert_eq!(p.monitor, MonitorId::Device("a".into()));
        assert_eq!(p.grid_rect(), g(5, 20, 16, 9), "保留格數、左上角對齊");
    }

    #[test]
    fn legal_move_rejects_overlap_with_other_actual_position() {
        let m = monitor_at("a", wa(0, 0, 1920, 1040), 1.0, true);
        let others = [neighbor(
            MonitorId::Device("a".into()),
            g(10, 10, 10, 10),
            Some((0, g(10, 10, 10, 10))),
        )];
        assert_eq!(
            legal_move_placement(std::slice::from_ref(&m), drop_at(15, 15), (16, 9), &others),
            None
        );
        // 共用格線（右緣＝鄰居左緣）不算重疊。
        assert!(
            legal_move_placement(std::slice::from_ref(&m), drop_at(0, 10), (10, 9), &others)
                .is_some()
        );
    }

    #[test]
    fn legal_move_rejects_overlap_with_native_record_even_if_actual_elsewhere() {
        // 鄰居記錄就在這台（native），但目前實際位置被換到別格（第二階段找空位的結果）：
        // 它的記錄格子仍然不可佔（D7 合法條件第二點）。
        let m = monitor_at("a", wa(0, 0, 1920, 1040), 1.0, true);
        let others = [neighbor(
            MonitorId::Device("a".into()),
            g(0, 30, 16, 9),
            Some((0, g(32, 0, 16, 9))),
        )];
        assert_eq!(
            legal_move_placement(std::slice::from_ref(&m), drop_at(4, 32), (16, 9), &others),
            None
        );
    }

    #[test]
    fn legal_move_ignores_records_of_displaced_and_no_space_hidden_widgets() {
        let m = monitor_at("a", wa(0, 0, 1920, 1040), 1.0, true);
        let others = [
            // 從別處換來：記錄在已拔除的顯示器 "gone"，記錄格子不算；實際位置在右上角。
            neighbor(
                MonitorId::Device("gone".into()),
                g(0, 30, 16, 9),
                Some((0, g(32, 0, 16, 9))),
            ),
            // 記錄就在這台、但空間不足暫時隱藏（沒有格子）：記錄格子也不算。
            neighbor(MonitorId::Device("a".into()), g(0, 30, 16, 9), None),
        ];
        let p = legal_move_placement(std::slice::from_ref(&m), drop_at(0, 30), (16, 9), &others)
            .expect("只有實際位置與 native 記錄會擋");
        assert_eq!(p.grid_rect(), g(0, 30, 16, 9));
        // 但換來者的實際位置照樣擋。
        assert_eq!(
            legal_move_placement(std::slice::from_ref(&m), drop_at(30, 2), (16, 9), &others),
            None
        );
    }

    #[test]
    fn legal_move_to_other_monitor_uses_center_and_only_checks_that_monitor() {
        let left = monitor_at("left", wa(-1920, 0, 1920, 1040), 1.0, false);
        let right = monitor_at("right", wa(0, 0, 1920, 1040), 1.0, true);
        // 鄰居在左螢幕 (0,0) 一帶（實際＋記錄）；拖到右螢幕同一組格座標不應被擋。
        let others = [neighbor(
            MonitorId::Device("left".into()),
            g(0, 0, 20, 20),
            Some((0, g(0, 0, 20, 20))),
        )];
        // 整個放在右螢幕左上角：與左螢幕鄰居同一組格座標，但不同台，不擋。
        let dropped = PhysicalRect {
            x: 0,
            y: 0,
            width: 640,
            height: 195,
        };
        let p = legal_move_placement(&[left.clone(), right.clone()], dropped, (16, 9), &others)
            .expect("右螢幕空白處應合法");
        assert_eq!(p.monitor, MonitorId::Device("right".into()));
        assert_eq!(p.grid_rect(), g(0, 0, 16, 9), "跨顯示器保留格數");

        // 左上角仍在左螢幕（x=-100）、中心在右螢幕：歸屬右螢幕，但相對右螢幕 col < 0＝超出
        // 工作區，不合法（fix round 1，不夾回）。
        let straddling = PhysicalRect {
            x: -100,
            y: 0,
            width: 640,
            height: 195,
        };
        assert_eq!(
            legal_move_placement(&[left.clone(), right.clone()], straddling, (16, 9), &others),
            None
        );

        // 反過來：中心落在左螢幕 → 與左螢幕鄰居重疊 → 不合法。
        let dropped_left = PhysicalRect {
            x: -1900,
            y: 0,
            width: 640,
            height: 195,
        };
        assert_eq!(
            legal_move_placement(&[left, right], dropped_left, (16, 9), &others),
            None
        );
    }

    #[test]
    fn legal_move_keeps_cells_even_below_min_on_denser_monitor() {
        // 保留 16×9 格移到 1280×800＠200% 的顯示器：16 格＝426 實體像素＝213 邏輯像素
        // < 500 × 0.5，小於最小格數。fix F6（controller 決定）：移動不檢查最小格數，照樣
        // 合法、保留 16×9（跨顯示器也保留格數，D7）；要變大由使用者調整大小。
        let big = monitor_at("big", wa(0, 0, 1920, 1040), 1.0, true);
        let small = monitor_at("small", wa(1920, 0, 1280, 800), 2.0, false);
        let dropped = PhysicalRect {
            x: 2000,
            y: 100,
            width: 640,
            height: 195,
        };
        let p =
            legal_move_placement(&[big, small], dropped, (16, 9), &[]).expect("移動不檢查最小格數");
        assert_eq!((p.w, p.h), (16, 9));
    }

    #[test]
    fn legal_move_partially_past_work_area_edge_is_illegal() {
        // fix round 1（spec「編輯版面」：放開會超出工作區 SHALL 紅框並彈回）：合法判斷用
        // 「未夾回」的對齊結果。16×9 拖到左上角 col 40 → 40+16=56 > 48，超出右緣。
        let m = monitor_at("a", wa(0, 0, 1920, 1040), 1.0, true);
        let legal = |dropped: PhysicalRect| {
            legal_move_placement(std::slice::from_ref(&m), dropped, (16, 9), &[])
        };
        assert_eq!(legal(drop_at(40, 10)), None, "超出右緣");
        assert_eq!(legal(drop_at(10, 42)), None, "超出下緣（42+9=51）");
        // 超出左緣／上緣：左上角在工作區外（中心仍在這台）→ round 後為負。
        let mut left = drop_at(0, 10);
        left.x -= 60;
        assert_eq!(legal(left), None, "超出左緣");
        let mut top = drop_at(10, 0);
        top.y -= 60;
        assert_eq!(legal(top), None, "超出上緣");
    }

    #[test]
    fn legal_move_flush_with_work_area_edge_is_legal() {
        // 負向：剛好貼齊右緣／下緣（col+w＝48、row+h＝48）仍合法。
        let m = monitor_at("a", wa(0, 0, 1920, 1040), 1.0, true);
        let p = legal_move_placement(std::slice::from_ref(&m), drop_at(32, 39), (16, 9), &[])
            .expect("貼齊邊緣合法");
        assert_eq!(p.grid_rect(), g(32, 39, 16, 9));
        // 左上角貼齊 (0,0) 也合法。
        assert!(
            legal_move_placement(std::slice::from_ref(&m), drop_at(0, 0), (16, 9), &[]).is_some()
        );
    }

    #[test]
    fn legal_move_on_unresolved_monitor_or_no_monitor_is_illegal() {
        let unresolved = MonitorInfo {
            id: None,
            work_area: wa(0, 0, 1920, 1040),
            scale_factor: 1.0,
            is_primary: true,
        };
        assert_eq!(
            legal_move_placement(&[unresolved], drop_at(5, 5), (16, 9), &[]),
            None
        );
        assert_eq!(legal_move_placement(&[], drop_at(5, 5), (16, 9), &[]), None);
    }

    #[test]
    fn legal_move_back_to_own_position_is_legal() {
        // 拖一下又放回原處：自己不在 others 裡（呼叫端排除），應合法、結果不變。
        let m = monitor_at("a", wa(0, 0, 1920, 1040), 1.0, true);
        let others = [neighbor(
            MonitorId::Device("a".into()),
            g(16, 0, 16, 9),
            Some((0, g(16, 0, 16, 9))),
        )];
        let p = legal_move_placement(std::slice::from_ref(&m), drop_at(0, 0), (16, 9), &others)
            .expect("原位合法");
        assert_eq!(p.grid_rect(), g(0, 0, 16, 9));
    }

    // ── fix monitor-id：不合法放開要說得出具體原因（check_move_placement／check_resize_placement）──

    #[test]
    fn check_move_lists_every_failed_condition() {
        // 顯示器無穩定識別＋與鄰居重疊同時成立：兩個原因都要列出，不在第一個失敗就停。
        let unresolved = MonitorInfo {
            id: None,
            work_area: wa(0, 0, 1920, 1040),
            scale_factor: 1.0,
            is_primary: true,
        };
        let others = [neighbor(
            MonitorId::Primary,
            g(10, 10, 10, 10),
            Some((0, g(10, 10, 10, 10))),
        )];
        let err = check_move_placement(
            std::slice::from_ref(&unresolved),
            drop_at(5, 5),
            (16, 9),
            &others,
        )
        .expect_err("應不合法");
        assert_eq!(
            err.reasons,
            vec![
                PlacementRejectReason::UnresolvedMonitor,
                PlacementRejectReason::Overlap
            ]
        );
        assert_eq!(err.monitor_index, Some(0));
        assert_eq!(err.rect, Some(g(5, 5, 16, 9)));
        assert_eq!(err.overlapping, vec![g(10, 10, 10, 10)]);
    }

    #[test]
    fn check_move_reports_out_of_bounds_alone() {
        let m = monitor_at("a", wa(0, 0, 1920, 1040), 1.0, true);
        let err = check_move_placement(std::slice::from_ref(&m), drop_at(40, 5), (16, 9), &[])
            .expect_err("col 40 + 16 > 48 應超界");
        assert_eq!(err.reasons, vec![PlacementRejectReason::OutOfBounds]);
    }

    #[test]
    fn move_below_min_size_to_free_spot_is_accepted() {
        // fix F6（review dragdpi medium，controller 決定）：移動保留格數、不改大小，「小於最小
        // 格數」只在調整大小時檢查。既有設定的時鐘 16×9 在 4K＠150% 低於新最小高度，仍要能
        // 移到合法空位；2×2 格（80×43 實體像素）遠小於設計寬度一半也一樣。
        let m = monitor_at("a", wa(0, 0, 1920, 1040), 1.0, true);
        let p = check_move_placement(std::slice::from_ref(&m), drop_at(5, 5), (2, 2), &[])
            .expect("移動不檢查最小格數，空位應合法");
        assert_eq!(p.grid_rect(), g(5, 5, 2, 2));

        // 4K＠150%（工作區 3840×2088）上的 16×9 時鐘：可用設計高度約 153 < 156。
        let k4 = monitor_at("4k", wa(0, 0, 3840, 2088), 1.5, true);
        let dropped = grid_rect_to_physical(k4.work_area, g(20, 20, 16, 9));
        let p = check_move_placement(std::slice::from_ref(&k4), dropped, (16, 9), &[])
            .expect("既有 16×9 時鐘移到空位應合法");
        assert_eq!(p.grid_rect(), g(20, 20, 16, 9));
    }

    #[test]
    fn move_below_min_size_still_rejects_overlap_and_out_of_bounds() {
        // 不檢查最小格數，但重疊與超界照舊擋。
        let m = monitor_at("a", wa(0, 0, 1920, 1040), 1.0, true);
        let others = [neighbor(
            MonitorId::Primary,
            g(10, 10, 10, 10),
            Some((0, g(10, 10, 10, 10))),
        )];
        let err = check_move_placement(std::slice::from_ref(&m), drop_at(11, 11), (2, 2), &others)
            .expect_err("與鄰居重疊");
        assert_eq!(err.reasons, vec![PlacementRejectReason::Overlap]);
        let err = check_move_placement(std::slice::from_ref(&m), drop_at(47, 5), (2, 2), &[])
            .expect_err("47+2 > 48 超界");
        assert_eq!(err.reasons, vec![PlacementRejectReason::OutOfBounds]);
    }

    #[test]
    fn resize_below_min_size_is_still_rejected() {
        // 調整大小仍檢查最小格數：時鐘（min 高 160，門檻 80 邏輯 px）16×9 從下緣縮到 16×3
        // （65 px）→ 彈回。widget-adaptive-zoom-and-grid D3 起高度門檻不再乘寬度倍率，原本的
        // 16×4（86 px）在新規則下合法，故改縮到 16×3；16×4 合法另行斷言。
        let m = monitor_at("a", wa(0, 0, 1920, 1040), 1.0, true);
        let clock = crate::widgets::widget_spec("clock")
            .expect("時鐘規格")
            .zoom_box;
        let shrink_to = |h: i32| {
            check_resize_placement(
                std::slice::from_ref(&m),
                0,
                g(5, 5, 16, 9),
                ResizeEdges {
                    left: false,
                    top: false,
                    right: false,
                    bottom: true,
                },
                phys(g(5, 5, 16, h)),
                &clock,
                &[],
            )
        };
        let err = shrink_to(3).expect_err("縮到小於最小格數應不合法");
        assert_eq!(err.reasons, vec![PlacementRejectReason::BelowMinSize]);
        assert!(shrink_to(4).is_ok(), "16×4（86 px ≥ 80）不小於最小格數");
    }

    #[test]
    fn check_move_without_monitors_reports_no_monitor() {
        let err = check_move_placement(&[], drop_at(5, 5), (16, 9), &[]).expect_err("沒有顯示器");
        assert_eq!(err.reasons, vec![PlacementRejectReason::NoMonitor]);
        assert_eq!(err.monitor_index, None);
        assert_eq!(err.rect, None);
    }

    #[test]
    fn check_move_ok_matches_legal_move_placement() {
        let m = monitor_at("a", wa(0, 0, 1920, 1040), 1.0, true);
        let ok = check_move_placement(std::slice::from_ref(&m), drop_at(5, 20), (16, 9), &[])
            .expect("空白處應合法");
        assert_eq!(
            Some(ok),
            legal_move_placement(std::slice::from_ref(&m), drop_at(5, 20), (16, 9), &[])
        );
    }

    #[test]
    fn check_resize_with_bad_monitor_index_reports_no_monitor() {
        let m = monitor_at("a", wa(0, 0, 1920, 1040), 1.0, true);
        let err = check_resize_placement(
            std::slice::from_ref(&m),
            3,
            g(0, 0, 16, 9),
            ResizeEdges {
                left: false,
                top: false,
                right: true,
                bottom: false,
            },
            phys(g(0, 0, 18, 9)),
            &fixed_box(500.0, 140.0),
            &[],
        )
        .expect_err("索引超出範圍");
        assert_eq!(err.reasons, vec![PlacementRejectReason::NoMonitor]);
    }

    #[test]
    fn placement_rejection_display_names_each_reason_in_chinese() {
        let all = PlacementRejection {
            reasons: vec![
                PlacementRejectReason::NoMonitor,
                PlacementRejectReason::UnresolvedMonitor,
                PlacementRejectReason::OutOfBounds,
                PlacementRejectReason::Overlap,
                PlacementRejectReason::BelowMinSize,
            ],
            monitor_index: Some(1),
            work_area: Some(wa(3840, 4, 2560, 1516)),
            rect: Some(g(15, 3, 16, 9)),
            overlapping: vec![g(15, 11, 16, 31)],
        };
        let text = all.to_string();
        for needle in [
            "沒有可用的顯示器",
            "顯示器無穩定識別",
            "超出格線範圍",
            "重疊",
            "小於最小格數",
            "顯示器 #1",
            "col=15 row=3 w=16 h=9",
            "col=15 row=11 w=16 h=31",
        ] {
            assert!(text.contains(needle), "缺「{needle}」：{text}");
        }
    }

    // ── task 7.6：編輯版面調整大小的合法判斷（legal_resize_placement）──────────────────

    /// (col,row,w,h) 在 1920×1040 工作區的實體矩形（調整大小的提議矩形基底）。
    fn phys(r: GridRect) -> PhysicalRect {
        grid_rect_to_physical(wa(0, 0, 1920, 1040), r)
    }

    #[test]
    fn legal_resize_widen_right_edge_two_cells_keeps_other_edges() {
        let m = monitor_at("a", wa(0, 0, 1920, 1040), 1.0, true);
        let original = g(10, 5, 16, 9);
        // 右緣往右拖約兩格再偏 7 像素；左／上／下緣也被系統迴圈帶了幾個像素的雜訊，
        // 只拖右緣時它們一律維持原格線。
        let mut proposed = phys(g(10, 5, 18, 9));
        proposed.width += 7;
        proposed.x += 3;
        proposed.width -= 3;
        proposed.y -= 4;
        let p = legal_resize_placement(
            std::slice::from_ref(&m),
            0,
            original,
            ResizeEdges::RIGHT,
            proposed,
            &fixed_box(500.0, 100.0),
            &[],
        )
        .expect("加寬兩格到空白處應合法");
        assert_eq!(p.monitor, MonitorId::Device("a".into()));
        assert_eq!(p.grid_rect(), g(10, 5, 18, 9));
    }

    #[test]
    fn legal_resize_corner_aligns_both_dragged_edges() {
        let m = monitor_at("a", wa(0, 0, 1920, 1040), 1.0, true);
        let original = g(10, 10, 16, 9);
        // 拖左下角：左緣到 col 8、下緣到 row 21；右緣與上緣不動。
        let proposed = phys(g(8, 10, 18, 11));
        let p = legal_resize_placement(
            std::slice::from_ref(&m),
            0,
            original,
            ResizeEdges::BOTTOM_LEFT,
            proposed,
            &fixed_box(500.0, 100.0),
            &[],
        )
        .expect("合法");
        assert_eq!(p.grid_rect(), g(8, 10, 18, 11));
    }

    #[test]
    fn legal_resize_below_min_size_is_illegal() {
        // 1920 寬＠100%、min 寬 500：最小寬格數是讓邏輯寬 ≥ 250 的格數（7 格＝280px）。
        let m = monitor_at("a", wa(0, 0, 1920, 1040), 1.0, true);
        let (min_w, _) = min_grid_size(m.work_area, 1.0, &fixed_box(500.0, 100.0));
        assert_eq!(min_w, 7);
        let original = g(10, 5, 16, 9);
        let resize_to = |w: i32| {
            legal_resize_placement(
                std::slice::from_ref(&m),
                0,
                original,
                ResizeEdges::RIGHT,
                phys(g(10, 5, w, 9)),
                &fixed_box(500.0, 100.0),
                &[],
            )
        };
        assert_eq!(resize_to(min_w - 1), None, "小於最小格數不合法");
        assert!(resize_to(min_w).is_some(), "剛好最小格數合法");
        // 右緣拖過左緣（寬度 ≤ 0）同樣不合法，不得被夾成 1 格。
        let mut crossed = phys(g(10, 5, 1, 9));
        crossed.width = -80;
        assert_eq!(
            legal_resize_placement(
                std::slice::from_ref(&m),
                0,
                original,
                ResizeEdges::RIGHT,
                crossed,
                &fixed_box(500.0, 100.0),
                &[]
            ),
            None
        );
    }

    #[test]
    fn legal_resize_overlapping_neighbor_is_illegal() {
        let m = monitor_at("a", wa(0, 0, 1920, 1040), 1.0, true);
        let others = [neighbor(
            MonitorId::Device("a".into()),
            g(28, 5, 10, 9),
            Some((0, g(28, 5, 10, 9))),
        )];
        let resize_to = |w: i32| {
            legal_resize_placement(
                std::slice::from_ref(&m),
                0,
                g(10, 5, 16, 9),
                ResizeEdges::RIGHT,
                phys(g(10, 5, w, 9)),
                &fixed_box(500.0, 100.0),
                &others,
            )
        };
        assert!(resize_to(18).is_some(), "右緣到 col 28（共邊）合法");
        assert_eq!(resize_to(19), None, "右緣到 col 29 與鄰居相交");
    }

    #[test]
    fn legal_resize_past_work_area_edge_is_illegal() {
        // 與移動一致（fix round 1）：被拖邊 round 後超出 0..=48 不合法，不夾回。
        let m = monitor_at("a", wa(0, 0, 1920, 1040), 1.0, true);
        let right = legal_resize_placement(
            std::slice::from_ref(&m),
            0,
            g(32, 5, 16, 9),
            ResizeEdges::RIGHT,
            phys(g(32, 5, 18, 9)),
            &fixed_box(500.0, 100.0),
            &[],
        );
        assert_eq!(right, None, "右緣超出 48");
        let mut past_left = phys(g(0, 5, 16, 9));
        past_left.x -= 80;
        past_left.width += 80;
        let left = legal_resize_placement(
            std::slice::from_ref(&m),
            0,
            g(0, 5, 16, 9),
            ResizeEdges::LEFT,
            past_left,
            &fixed_box(500.0, 100.0),
            &[],
        );
        assert_eq!(left, None, "左緣超出 0");
        // 負向：右緣剛好貼齊 48 合法。
        assert!(legal_resize_placement(
            std::slice::from_ref(&m),
            0,
            g(30, 5, 16, 9),
            ResizeEdges::RIGHT,
            phys(g(30, 5, 18, 9)),
            &fixed_box(500.0, 100.0),
            &[]
        )
        .is_some());
    }

    #[test]
    fn legal_resize_uses_given_monitor_and_rejects_unresolved_or_missing() {
        // 調整大小的所屬顯示器是小工具目前實際所在者（呼叫端傳索引），不看提議矩形中心。
        let left = monitor_at("L", wa(0, 0, 1920, 1040), 1.0, true);
        let right = monitor_at("R", wa(1920, 0, 1920, 1040), 1.0, false);
        let mut proposed = grid_rect_to_physical(right.work_area, g(0, 5, 18, 9));
        proposed.x = right.work_area.x;
        let p = legal_resize_placement(
            &[left.clone(), right.clone()],
            1,
            g(0, 5, 16, 9),
            ResizeEdges::RIGHT,
            proposed,
            &fixed_box(500.0, 100.0),
            &[],
        )
        .expect("右螢幕加寬合法");
        assert_eq!(p.monitor, MonitorId::Device("R".into()));
        assert_eq!(p.grid_rect(), g(0, 5, 18, 9));
        // 索引超出範圍、或該顯示器沒有穩定識別：不合法。
        assert_eq!(
            legal_resize_placement(
                std::slice::from_ref(&left),
                3,
                g(0, 5, 16, 9),
                ResizeEdges::RIGHT,
                proposed,
                &fixed_box(500.0, 100.0),
                &[]
            ),
            None
        );
        let unresolved = MonitorInfo { id: None, ..left };
        assert_eq!(
            legal_resize_placement(
                &[unresolved],
                0,
                g(0, 5, 16, 9),
                ResizeEdges::RIGHT,
                phys(g(0, 5, 18, 9)),
                &fixed_box(500.0, 100.0),
                &[]
            ),
            None
        );
    }

    // ── fix drag-dpi：跨縮放比例拖曳時，視窗即時變成目前格數在目標顯示器上的實際大小 ──

    /// 使用者本機實況（monitor-id 報告的探針）：左＝4K 150%、右＝筆電 175%。
    fn uhd_and_laptop() -> [MonitorInfo; 2] {
        [
            monitor_at("4k", wa(0, 0, 3840, 2088), 1.5, true),
            monitor_at("laptop", wa(3840, 4, 2560, 1516), 1.75, false),
        ]
    }

    #[test]
    fn grid_span_size_is_keep_cells_on_that_monitor() {
        let [uhd, laptop] = uhd_and_laptop();
        // 時鐘 16×9：4K 上 floor(16×3840/48)=1280、floor(9×2088/48)=391（monitor-id 報告實測
        // 「在 4K 上實際佔約 1280×391」）；筆電 floor(16×2560/48)=853、floor(9×1516/48)=284。
        assert_eq!(grid_span_size(&uhd, (16, 9)), (1280, 391));
        assert_eq!(grid_span_size(&laptop, (16, 9)), (853, 284));
    }

    #[test]
    fn grab_fraction_is_cursor_position_relative_to_rect() {
        let r = wa(100, 200, 400, 100);
        assert_eq!(grab_fraction(r, (200, 250)), (0.25, 0.5));
        // 游標在矩形外（理論上不會，防禦）：夾在 [0, 1]。
        assert_eq!(grab_fraction(r, (0, 900)), (0.0, 1.0));
        // 退化矩形：取中心。
        assert_eq!(grab_fraction(wa(0, 0, 0, 0), (5, 5)), (0.5, 0.5));
    }

    #[test]
    fn rect_at_grab_keeps_cursor_at_same_fraction_of_new_size() {
        let r = rect_at_grab((1000, 500), (0.25, 0.5), (1280, 391));
        // x = 1000 − 320；y = 500 − round(195.5)=500 − 196。
        assert_eq!(r, wa(680, 304, 1280, 391));
        // 抓點比例由舊矩形算出、換成同尺寸時位置與系統提議完全一致（同顯示器拖曳不改寫）。
        let start = wa(4000, 300, 853, 284);
        let cursor = (4000 + 213, 300 + 71);
        let grab = grab_fraction(start, cursor);
        assert_eq!(rect_at_grab(cursor, grab, (853, 284)), start);
    }

    fn clock_drag_from_laptop(grab: (f64, f64)) -> DragResize {
        DragResize {
            start_monitor: 1,
            start_size: (853, 284),
            keep: (16, 9),
            grab,
        }
    }

    #[test]
    fn resolve_drag_rect_keeps_start_size_while_still_on_start_monitor() {
        let mons = uhd_and_laptop();
        // 起始顯示器沿用拖曳開始時的實際大小（不以 floor 公式重算，避免 1px 抖動觸發多餘的
        // WebView 尺寸變更）。
        let sizing = DragResize {
            start_size: (856, 287),
            ..clock_drag_from_laptop((0.5, 0.5))
        };
        let (target, rect) = resolve_drag_rect(&mons, &sizing, 1, (5000, 700)).expect("有顯示器");
        assert_eq!(target, 1);
        assert_eq!((rect.width, rect.height), (856, 287));
    }

    #[test]
    fn resolve_drag_rect_switches_to_target_monitor_size_when_center_crosses() {
        let mons = uhd_and_laptop();
        let sizing = clock_drag_from_laptop((0.5, 0.5));
        // 游標（＝抓在正中央時的中心）已在 4K 深處：變成 4K 上 16×9 的實際大小。
        let (target, rect) = resolve_drag_rect(&mons, &sizing, 1, (2000, 800)).expect("有顯示器");
        assert_eq!(target, 0);
        assert_eq!(rect, wa(2000 - 640, 800 - 196, 1280, 391));
        // 拖回筆電：變回筆電大小（起始顯示器＝起始大小）。
        let (target, rect) = resolve_drag_rect(&mons, &sizing, 0, (5000, 800)).expect("有顯示器");
        assert_eq!(target, 1);
        assert_eq!((rect.width, rect.height), (853, 284));
    }

    #[test]
    fn resolve_drag_rect_target_is_center_monitor_of_returned_rect_when_consistent() {
        // 「所見即所得」的核心不變式：回傳矩形的中心落在回傳的目標顯示器上（放開判定看中心）。
        let mons = uhd_and_laptop();
        for &fx in &[0.0, 0.1, 0.3, 0.5, 0.7, 0.9, 1.0] {
            let sizing = clock_drag_from_laptop((fx, 0.5));
            for cx in (3000..4800).step_by(7) {
                for current in [0usize, 1] {
                    let (target, rect) =
                        resolve_drag_rect(&mons, &sizing, current, (cx, 800)).expect("有顯示器");
                    let center = (rect.x + rect.width / 2, rect.y + rect.height / 2);
                    let center_mon = monitor_index_for_point(&mons, center.0, center.1);
                    if center_mon != Some(target) {
                        // 唯一允許的例外：兩台都不自洽的窄帶，維持目前目標（遲滯、不來回跳）。
                        assert_eq!(target, current, "fx={fx} cx={cx}");
                        for m in 0..2 {
                            let size = if m == 1 { (853, 284) } else { (1280, 391) };
                            let r = rect_at_grab((cx, 800), (fx, 0.5), size);
                            let c = monitor_index_for_point(
                                &mons,
                                r.x + r.width / 2,
                                r.y + r.height / 2,
                            );
                            assert_ne!(c, Some(m), "fx={fx} cx={cx} m={m} 應不自洽");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn resolve_drag_rect_holds_current_target_in_band_where_neither_is_consistent() {
        // 抓在左側 10%、往左拖進 4K：游標 3400 時，筆電大小的中心＝3400+0.4×853≈3741（在 4K），
        // 4K 大小的中心＝3400+0.4×1280＝3912（在筆電）——兩台都不自洽，維持目前目標不來回跳。
        let mons = uhd_and_laptop();
        let sizing = clock_drag_from_laptop((0.1, 0.5));
        let (t, r) = resolve_drag_rect(&mons, &sizing, 1, (3400, 800)).expect("有顯示器");
        assert_eq!((t, r.width), (1, 853));
        let (t, r) = resolve_drag_rect(&mons, &sizing, 0, (3400, 800)).expect("有顯示器");
        assert_eq!((t, r.width), (0, 1280));
    }

    #[test]
    fn resolve_drag_rect_prefers_current_target_when_both_are_consistent() {
        // 抓在右側 90%：游標 4200 時筆電大小中心 4200−0.4×853≈3859（筆電）、4K 大小中心
        // 4200−0.4×1280＝3688（4K）——兩台都自洽，維持目前目標。
        let mons = uhd_and_laptop();
        let sizing = clock_drag_from_laptop((0.9, 0.5));
        let (t, _) = resolve_drag_rect(&mons, &sizing, 1, (4200, 800)).expect("有顯示器");
        assert_eq!(t, 1);
        let (t, _) = resolve_drag_rect(&mons, &sizing, 0, (4200, 800)).expect("有顯示器");
        assert_eq!(t, 0);
    }

    #[test]
    fn resolve_drag_rect_without_monitors_is_none() {
        let sizing = clock_drag_from_laptop((0.5, 0.5));
        assert_eq!(resolve_drag_rect(&[], &sizing, 0, (0, 0)), None);
    }

    #[test]
    fn check_move_placement_on_judges_on_given_monitor_not_center() {
        // 窄帶內視窗是 4K 大小、中心卻在筆電：預告與放開都以拖曳決定的目標（4K）判斷，
        // 在 4K 的格座標上超出右界 → 不合法；若照中心判斷會落在筆電、換算出另一個結果。
        let mons = uhd_and_laptop();
        let rect = rect_at_grab((3400, 800), (0.1, 0.5), (1280, 391));
        let err = check_move_placement_on(&mons, 0, rect, (16, 9), &[])
            .expect_err("在 4K 上 col+w 超過 48");
        assert_eq!(err.monitor_index, Some(0));
        assert_eq!(err.reasons, vec![PlacementRejectReason::OutOfBounds]);
        // 中心所在顯示器與目標一致時，與 check_move_placement 完全相同。
        let inside = rect_at_grab((1000, 800), (0.5, 0.5), (1280, 391));
        assert_eq!(
            check_move_placement_on(&mons, 0, inside, (16, 9), &[]),
            check_move_placement(&mons, inside, (16, 9), &[])
        );
        // 目標索引超出範圍：沒有顯示器。
        let err =
            check_move_placement_on(&mons, 5, inside, (16, 9), &[]).expect_err("索引超出範圍");
        assert_eq!(err.reasons, vec![PlacementRejectReason::NoMonitor]);
    }
}
