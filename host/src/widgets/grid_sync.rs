//! 編輯版面格線疊加視窗的生命週期（widget-adaptive-zoom-and-grid task 5.2，widget-adaptive-zoom-and-grid
//! design.md D4「生命週期」「執行緒」）。
//!
//! - [`plan_overlays`]：純函式，「目標狀態」判定——在編輯版面中（且不是正在因更新結束）時每台顯示器
//!   各一個、矩形＝該台工作區；否則全部銷毀。輸入目前已有的格線（鍵、矩形、顏色），輸出要銷毀／
//!   更新／建立的清單。
//! - [`apply_on_this_thread`]：在**主執行緒**依計畫建立、更新、銷毀 [`GridOverlay`]。格線集合放在
//!   本模組的 `thread_local!`：[`GridOverlay`] 是 `!Send`（視窗屬於建立它的執行緒），不能放進
//!   `AppState`（Tauri managed state 必須 `Send + Sync`）。呼叫端是 `widgets::sync_grid_overlay`，
//!   它經 `AppHandle::run_on_main_thread` 保證在主執行緒執行。

use std::cell::RefCell;

use crate::desktop::grid_overlay::{GridOverlay, Rgb};
use crate::layout::{MonitorInfo, PhysicalRect};
use crate::settings::MonitorId;

/// 一個格線視窗屬於哪台顯示器。
///
/// - `Monitor`：顯示器的穩定識別（[`MonitorInfo::id`]，＝版面記錄位置用的同一個 `monitorDevicePath`）。
/// - `Unidentified(索引)`：本次列舉查不到穩定識別（`MonitorInfo::id` 為 `None`），或同一個識別在清單裡
///   重複出現（不應發生，防禦用）時，退回以本次列舉的索引當鍵——這台顯示器照樣要有格線，只是鍵不保證
///   跨列舉穩定；索引對到別台時矩形不同，照「矩形不同就更新」處理，結果仍正確。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayKey {
    Monitor(MonitorId),
    Unidentified(usize),
}

/// 目前已存在的一個格線視窗（[`plan_overlays`] 的輸入）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExistingOverlay {
    pub key: OverlayKey,
    pub work_area: PhysicalRect,
    pub color: Rgb,
    /// 建立時取不到殼層桌面視窗、以 `HWND_BOTTOM` 退路顯示，等殼層恢復後重新定位
    /// （`GridOverlay::needs_restack`）。為真時即使矩形與顏色沒變也列入 `update`。
    pub needs_restack: bool,
}

/// [`plan_overlays`] 的輸入（目前狀態，不含已存在的格線）。
#[derive(Debug, Clone, Copy)]
pub struct OverlayInputs<'a> {
    /// 是否在編輯版面中（`AppState::edit_mode`）。
    pub edit_mode: bool,
    /// `updater::is_exiting_for_update()`：為真時一律不建立（AGENTS.md「自動更新」建立入口不變式）。
    pub exiting_for_update: bool,
    /// 目前的顯示器清單（與 `relayout_all_widgets` 同一來源）；不在編輯版面時呼叫端可傳空清單。
    pub monitors: &'a [MonitorInfo],
    /// 主題色（`grid_overlay::accent_or_default(&settings.accent_color)`）。
    pub color: Rgb,
}

/// [`plan_overlays`] 的結果。執行順序：先銷毀、再更新、最後建立。`update`／`create` 一律用
/// [`OverlayInputs::color`]。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OverlayPlan {
    pub destroy: Vec<OverlayKey>,
    /// 矩形或顏色有變的既有格線：就地重畫（`GridOverlay::update` 會同時搬移／改尺寸，不動 z-order）。
    pub update: Vec<(OverlayKey, PhysicalRect)>,
    pub create: Vec<(OverlayKey, PhysicalRect)>,
}

impl OverlayPlan {
    pub fn is_empty(&self) -> bool {
        self.destroy.is_empty() && self.update.is_empty() && self.create.is_empty()
    }
}

/// 「目標狀態」判定（design.md D4「生命週期」）。純函式、冪等：同樣的輸入對已達目標的狀態回傳空計畫。
///
/// 規則（依序）：
/// 1. 不在編輯版面，或正在因更新結束（`exiting_for_update`）→ 目標是「沒有格線」：既有的全部銷毀，
///    不看顯示器清單。因更新結束時**絕不建立**（建立入口不變式）；銷毀無害（行程本來就要結束）。
/// 2. 顯示器清單為空（`current_monitors` 列舉失敗）→ 維持現狀（比照 `relayout_all_widgets`／
///    `resolve_for_relayout` 的語意），不因一次暫時失敗把格線拆掉再建回來。
/// 3. 否則目標＝每台工作區寬高皆 > 0 的顯示器各一個、矩形＝工作區，鍵見 [`OverlayKey`]：
///    - 已有、但鍵不在目標裡（顯示器拔掉、工作區退化）→ 銷毀；
///    - 已有、矩形或顏色不同（顯示器、解析度、縮放比例、工作區、主題色變了），或建立時以退路定位、
///      等待殼層恢復（`needs_restack`）→ 更新；
///    - 目標裡沒有對應的既有格線 → 建立。
///
/// 輸出順序：`destroy` 依 `existing` 的順序，`update`／`create` 依 `monitors` 的順序（記錄與測試穩定）。
pub fn plan_overlays(inputs: OverlayInputs<'_>, existing: &[ExistingOverlay]) -> OverlayPlan {
    if !inputs.edit_mode || inputs.exiting_for_update {
        return OverlayPlan {
            destroy: existing.iter().map(|e| e.key.clone()).collect(),
            ..OverlayPlan::default()
        };
    }
    if inputs.monitors.is_empty() {
        return OverlayPlan::default();
    }

    let targets = overlay_targets(inputs.monitors);
    let mut plan = OverlayPlan {
        destroy: existing
            .iter()
            .filter(|e| !targets.iter().any(|(key, _)| *key == e.key))
            .map(|e| e.key.clone())
            .collect(),
        ..OverlayPlan::default()
    };
    for (key, work_area) in targets {
        match existing.iter().find(|e| e.key == key) {
            Some(e) if e.work_area == work_area && e.color == inputs.color && !e.needs_restack => {}
            Some(_) => plan.update.push((key, work_area)),
            None => plan.create.push((key, work_area)),
        }
    }
    plan
}

/// 目標格線：每台工作區寬高皆 > 0 的顯示器一個 `(鍵, 工作區)`，依 `monitors` 順序；鍵的規則見
/// [`OverlayKey`]（沒有穩定識別或識別重複 → 以索引當鍵）。
fn overlay_targets(monitors: &[MonitorInfo]) -> Vec<(OverlayKey, PhysicalRect)> {
    let mut targets: Vec<(OverlayKey, PhysicalRect)> = Vec::with_capacity(monitors.len());
    for (index, m) in monitors.iter().enumerate() {
        if m.work_area.width <= 0 || m.work_area.height <= 0 {
            continue;
        }
        let key = match &m.id {
            Some(id)
                if !targets
                    .iter()
                    .any(|(k, _)| *k == OverlayKey::Monitor(id.clone())) =>
            {
                OverlayKey::Monitor(id.clone())
            }
            _ => OverlayKey::Unidentified(index),
        };
        targets.push((key, m.work_area));
    }
    targets
}

thread_local! {
    /// 目前存在的格線視窗。只在主執行緒用到（[`apply_on_this_thread`] 的呼叫端前提），其他執行緒的這份
    /// 永遠是空的。`Drop` 時 `DestroyWindow`（`GridOverlay` 的 `Drop`）。
    ///
    /// 行程結束時不保證執行主執行緒的 TLS 解構——不影響：視窗隨行程結束一併被系統銷毀。
    static OVERLAYS: RefCell<Vec<(OverlayKey, GridOverlay)>> = const { RefCell::new(Vec::new()) };
}

/// [`apply_plan`] 需要的格線視窗介面：正式碼是 [`GridOverlay`]，單元測試以假物件替代（不建真視窗）。
pub trait OverlayWindow {
    fn work_area(&self) -> PhysicalRect;
    fn color(&self) -> Rgb;
    fn update(&mut self, work_area: PhysicalRect, color: Rgb) -> Result<bool, String>;
    /// 是否以退路定位、等待殼層恢復後重新定位（見 [`ExistingOverlay::needs_restack`]）。
    fn needs_restack(&self) -> bool {
        false
    }
}

impl OverlayWindow for GridOverlay {
    fn work_area(&self) -> PhysicalRect {
        GridOverlay::work_area(self)
    }
    fn color(&self) -> Rgb {
        GridOverlay::color(self)
    }
    fn needs_restack(&self) -> bool {
        GridOverlay::needs_restack(self)
    }
    fn update(&mut self, work_area: PhysicalRect, color: Rgb) -> Result<bool, String> {
        GridOverlay::update(self, work_area, color)
    }
}

/// 依 [`plan_overlays`] 在本執行緒建立、更新、銷毀格線（呼叫端保證是主執行緒，見模組文件）。
/// `is_exiting`＝`updater::is_exiting_for_update`（建立前後重新讀取，見 [`apply_plan`]）。
///
/// 重入：建立／銷毀視窗時系統只對格線自己的視窗程序送訊息，不會回到 tao 事件迴圈，理論上不會重入；
/// 萬一重入（`RefCell` 已被借用）就略過這一次並記 warn，外層那次同步會以它開始時讀到的狀態完成。
pub fn apply_on_this_thread(inputs: OverlayInputs<'_>, is_exiting: &dyn Fn() -> bool) {
    OVERLAYS.with(|cell| {
        let Ok(mut overlays) = cell.try_borrow_mut() else {
            log::warn!("格線同步：重入（上一輪尚未完成），略過這一次");
            return;
        };
        apply_plan(&mut overlays, inputs, is_exiting, &mut GridOverlay::create);
    });
}

/// 銷毀本執行緒上所有格線（task 5.2 修正第 1 輪：更新收尾 `close_ui` 在主執行緒呼叫，收掉收尾開始前
/// 已存在的格線——`close_ui` 只銷毀 `webview_windows()`，看不到這些原生視窗）。回傳銷毀的數量。
/// 重入（`RefCell` 已被借用）時記 warn、回 0（理論上不會發生，理由同 [`apply_on_this_thread`]）。
pub fn clear_on_this_thread() -> usize {
    OVERLAYS.with(|cell| {
        let Ok(mut overlays) = cell.try_borrow_mut() else {
            log::warn!("格線清除：重入（同步進行中），略過");
            return 0;
        };
        let taken = std::mem::take(&mut *overlays);
        let count = taken.len();
        drop(taken);
        if count > 0 {
            log::info!("格線清除：已銷毀 {count} 個格線視窗");
        }
        count
    })
}

/// [`apply_on_this_thread`] 的本體（對格線型別泛型，供測試注入假物件、假的建立函式與旗標讀取函式）。
///
/// 個別格線失敗只記錄、不中斷其餘顯示器（同 `sync_widget_windows` 的錯誤處理原則）；失敗者留到下一次
/// 同步（下一次重排、主題色變更或進出編輯版面）再試：建立失敗＝集合裡沒有它，下次照樣列入 `create`；
/// 更新失敗＝`update` 保留舊紀錄，下次照樣列入 `update`。
pub fn apply_plan<O: OverlayWindow>(
    overlays: &mut Vec<(OverlayKey, O)>,
    inputs: OverlayInputs<'_>,
    is_exiting: &dyn Fn() -> bool,
    create: &mut dyn FnMut(PhysicalRect, Rgb) -> Result<O, String>,
) {
    let existing: Vec<ExistingOverlay> = overlays
        .iter()
        .map(|(key, overlay)| ExistingOverlay {
            key: key.clone(),
            work_area: overlay.work_area(),
            color: overlay.color(),
            needs_restack: overlay.needs_restack(),
        })
        .collect();
    if inputs.exiting_for_update && inputs.edit_mode {
        log::info!("格線同步：正在因更新結束，不建立格線");
    }
    let plan = plan_overlays(inputs, &existing);
    if plan.is_empty() {
        return;
    }

    // 1. 銷毀：移出集合即 drop（`DestroyWindow`）。
    overlays.retain(|(key, _)| {
        let keep = !plan.destroy.contains(key);
        if !keep {
            log::info!("格線同步：移除 {key:?}");
        }
        keep
    });

    // 2. 更新：矩形或顏色變了就地重畫；以退路定位的格線在殼層恢復後重新插到桌面正上方。
    for (key, work_area) in &plan.update {
        let Some((_, overlay)) = overlays.iter_mut().find(|(k, _)| k == key) else {
            continue;
        };
        match overlay.update(*work_area, inputs.color) {
            Ok(_) => log::info!("格線同步：重畫 {key:?} → {work_area:?}"),
            Err(err) => log::warn!("格線同步：重畫失敗（{key:?}，下次再試）：{err}"),
        }
    }

    // 3. 建立。`inputs.exiting_for_update` 只是規劃前的一次取樣；更新器在背景執行緒設旗標，可能在規劃之後、
    //    甚至某次建立期間才翻轉，所以每次建立前後都重新讀（比照小工具視窗工廠在 `build()` 前後各查一次）：
    //    建立前為真＝不建、後面的也不建；建立後為真＝剛建好的立即銷毀（drop＝`DestroyWindow`）、不放進集合。
    //    收尾開始前已存在的格線由 `close_ui` 經 [`clear_on_this_thread`] 收掉。
    for (key, work_area) in plan.create {
        if is_exiting() {
            log::info!("格線同步：正在因更新結束，停止建立格線（{key:?} 起未建立）");
            break;
        }
        match create(work_area, inputs.color) {
            Ok(overlay) => {
                if is_exiting() {
                    drop(overlay);
                    log::info!("格線同步：建立期間開始因更新結束，已銷毀剛建立的 {key:?}");
                    break;
                }
                log::info!("格線同步：建立 {key:?} → {work_area:?}");
                overlays.push((key, overlay));
            }
            Err(err) => log::warn!("格線同步：建立失敗（{key:?}，下次再試）：{err}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOLD: Rgb = Rgb {
        r: 0xe0,
        g: 0xaa,
        b: 0x54,
    };
    const BLUE: Rgb = Rgb {
        r: 0x40,
        g: 0x80,
        b: 0xff,
    };

    fn rect(x: i32, y: i32, width: i32, height: i32) -> PhysicalRect {
        PhysicalRect {
            x,
            y,
            width,
            height,
        }
    }

    fn device(name: &str) -> MonitorId {
        MonitorId::Device(name.to_string())
    }

    fn monitor(id: Option<MonitorId>, work_area: PhysicalRect, is_primary: bool) -> MonitorInfo {
        MonitorInfo {
            id,
            work_area,
            scale_factor: 1.0,
            is_primary,
        }
    }

    /// 雙螢幕：主螢幕 A（3840×2088 工作區）＋左側 B（1920×1040，座標為負）。
    fn two_monitors() -> Vec<MonitorInfo> {
        vec![
            monitor(Some(device("A")), rect(0, 0, 3840, 2088), true),
            monitor(Some(device("B")), rect(-1920, 0, 1920, 1040), false),
        ]
    }

    fn existing(key: OverlayKey, work_area: PhysicalRect, color: Rgb) -> ExistingOverlay {
        ExistingOverlay {
            key,
            work_area,
            color,
            needs_restack: false,
        }
    }

    fn inputs(edit_mode: bool, exiting: bool, monitors: &[MonitorInfo]) -> OverlayInputs<'_> {
        OverlayInputs {
            edit_mode,
            exiting_for_update: exiting,
            monitors,
            color: GOLD,
        }
    }

    #[test]
    fn entering_edit_mode_creates_one_overlay_per_monitor_covering_its_work_area() {
        let monitors = two_monitors();
        let plan = plan_overlays(inputs(true, false, &monitors), &[]);
        assert_eq!(
            plan,
            OverlayPlan {
                destroy: vec![],
                update: vec![],
                create: vec![
                    (OverlayKey::Monitor(device("A")), rect(0, 0, 3840, 2088)),
                    (OverlayKey::Monitor(device("B")), rect(-1920, 0, 1920, 1040)),
                ],
            }
        );
    }

    #[test]
    fn already_at_target_state_plans_nothing() {
        let monitors = two_monitors();
        let have = [
            existing(
                OverlayKey::Monitor(device("A")),
                rect(0, 0, 3840, 2088),
                GOLD,
            ),
            existing(
                OverlayKey::Monitor(device("B")),
                rect(-1920, 0, 1920, 1040),
                GOLD,
            ),
        ];
        let plan = plan_overlays(inputs(true, false, &monitors), &have);
        assert!(plan.is_empty(), "{plan:?}");
    }

    /// Codex 審 5c46c74 medium：建立時取不到殼層桌面視窗（explorer 重啟中）而以 `HWND_BOTTOM` 退路顯示的格線，
    /// 矩形與顏色都沒變也要列入更新，讓 `GridOverlay::update` 在殼層恢復後重新插到桌面正上方。
    #[test]
    fn overlay_placed_by_fallback_is_updated_even_when_unchanged() {
        let monitors = two_monitors();
        let mut a = existing(
            OverlayKey::Monitor(device("A")),
            rect(0, 0, 3840, 2088),
            GOLD,
        );
        a.needs_restack = true;
        let have = [
            a,
            existing(
                OverlayKey::Monitor(device("B")),
                rect(-1920, 0, 1920, 1040),
                GOLD,
            ),
        ];
        let plan = plan_overlays(inputs(true, false, &monitors), &have);
        assert_eq!(
            plan.update,
            vec![(OverlayKey::Monitor(device("A")), rect(0, 0, 3840, 2088))]
        );
        assert!(
            plan.create.is_empty() && plan.destroy.is_empty(),
            "{plan:?}"
        );
    }

    #[test]
    fn leaving_edit_mode_destroys_every_overlay_and_ignores_monitors() {
        let monitors = two_monitors();
        let have = [
            existing(
                OverlayKey::Monitor(device("A")),
                rect(0, 0, 3840, 2088),
                GOLD,
            ),
            existing(
                OverlayKey::Unidentified(1),
                rect(-1920, 0, 1920, 1040),
                GOLD,
            ),
        ];
        let plan = plan_overlays(inputs(false, false, &monitors), &have);
        assert_eq!(
            plan,
            OverlayPlan {
                destroy: vec![
                    OverlayKey::Monitor(device("A")),
                    OverlayKey::Unidentified(1)
                ],
                update: vec![],
                create: vec![],
            }
        );
    }

    #[test]
    fn not_editing_without_overlays_plans_nothing() {
        let plan = plan_overlays(inputs(false, false, &[]), &[]);
        assert!(plan.is_empty(), "{plan:?}");
    }

    #[test]
    fn exiting_for_update_never_creates_and_tears_down_existing() {
        let monitors = two_monitors();
        let plan = plan_overlays(inputs(true, true, &monitors), &[]);
        assert!(plan.is_empty(), "因更新結束中不得建立：{plan:?}");

        let have = [existing(
            OverlayKey::Monitor(device("A")),
            rect(0, 0, 3840, 2088),
            GOLD,
        )];
        let plan = plan_overlays(inputs(true, true, &monitors), &have);
        assert_eq!(plan.destroy, vec![OverlayKey::Monitor(device("A"))]);
        assert!(plan.update.is_empty() && plan.create.is_empty(), "{plan:?}");
    }

    #[test]
    fn work_area_change_updates_only_that_monitor() {
        // B 的縮放比例／解析度／工作列改了：工作區變成 2560×1392。
        let monitors = vec![
            monitor(Some(device("A")), rect(0, 0, 3840, 2088), true),
            monitor(Some(device("B")), rect(-2560, 0, 2560, 1392), false),
        ];
        let have = [
            existing(
                OverlayKey::Monitor(device("A")),
                rect(0, 0, 3840, 2088),
                GOLD,
            ),
            existing(
                OverlayKey::Monitor(device("B")),
                rect(-1920, 0, 1920, 1040),
                GOLD,
            ),
        ];
        let plan = plan_overlays(inputs(true, false, &monitors), &have);
        assert_eq!(
            plan,
            OverlayPlan {
                destroy: vec![],
                update: vec![(OverlayKey::Monitor(device("B")), rect(-2560, 0, 2560, 1392))],
                create: vec![],
            }
        );
    }

    #[test]
    fn accent_color_change_redraws_every_overlay() {
        let monitors = two_monitors();
        let have = [
            existing(
                OverlayKey::Monitor(device("A")),
                rect(0, 0, 3840, 2088),
                GOLD,
            ),
            existing(
                OverlayKey::Monitor(device("B")),
                rect(-1920, 0, 1920, 1040),
                GOLD,
            ),
        ];
        let mut changed = inputs(true, false, &monitors);
        changed.color = BLUE;
        let plan = plan_overlays(changed, &have);
        assert_eq!(
            plan,
            OverlayPlan {
                destroy: vec![],
                update: vec![
                    (OverlayKey::Monitor(device("A")), rect(0, 0, 3840, 2088)),
                    (OverlayKey::Monitor(device("B")), rect(-1920, 0, 1920, 1040)),
                ],
                create: vec![],
            }
        );
    }

    #[test]
    fn unplugged_monitor_is_destroyed_and_new_monitor_is_created() {
        let monitors = vec![
            monitor(Some(device("A")), rect(0, 0, 3840, 2088), true),
            monitor(Some(device("C")), rect(3840, 0, 1920, 1040), false),
        ];
        let have = [
            existing(
                OverlayKey::Monitor(device("A")),
                rect(0, 0, 3840, 2088),
                GOLD,
            ),
            existing(
                OverlayKey::Monitor(device("B")),
                rect(-1920, 0, 1920, 1040),
                GOLD,
            ),
        ];
        let plan = plan_overlays(inputs(true, false, &monitors), &have);
        assert_eq!(
            plan,
            OverlayPlan {
                destroy: vec![OverlayKey::Monitor(device("B"))],
                update: vec![],
                create: vec![(OverlayKey::Monitor(device("C")), rect(3840, 0, 1920, 1040))],
            }
        );
    }

    /// 顯示器列舉失敗（`current_monitors` 回空清單）時比照 `relayout_all_widgets` 維持現狀：不因一次
    /// 暫時失敗把格線全拆掉、下一輪又建回來（閃爍）。
    #[test]
    fn empty_monitor_enumeration_while_editing_keeps_existing_overlays() {
        let have = [existing(
            OverlayKey::Monitor(device("A")),
            rect(0, 0, 3840, 2088),
            GOLD,
        )];
        let plan = plan_overlays(inputs(true, false, &[]), &have);
        assert!(plan.is_empty(), "{plan:?}");
    }

    #[test]
    fn monitors_without_stable_id_still_get_an_overlay_keyed_by_index() {
        let monitors = vec![
            monitor(None, rect(0, 0, 1920, 1040), true),
            monitor(Some(device("B")), rect(1920, 0, 1920, 1040), false),
            monitor(None, rect(3840, 0, 1920, 1040), false),
        ];
        let plan = plan_overlays(inputs(true, false, &monitors), &[]);
        assert_eq!(
            plan.create,
            vec![
                (OverlayKey::Unidentified(0), rect(0, 0, 1920, 1040)),
                (OverlayKey::Monitor(device("B")), rect(1920, 0, 1920, 1040)),
                (OverlayKey::Unidentified(2), rect(3840, 0, 1920, 1040)),
            ]
        );
    }

    /// 同一個識別重複出現（不應發生）時，第二台退回以索引當鍵，兩台都有格線、鍵不相撞。
    #[test]
    fn duplicate_monitor_ids_fall_back_to_index_keys() {
        let monitors = vec![
            monitor(Some(device("A")), rect(0, 0, 1920, 1040), true),
            monitor(Some(device("A")), rect(1920, 0, 1920, 1040), false),
        ];
        let plan = plan_overlays(inputs(true, false, &monitors), &[]);
        assert_eq!(
            plan.create,
            vec![
                (OverlayKey::Monitor(device("A")), rect(0, 0, 1920, 1040)),
                (OverlayKey::Unidentified(1), rect(1920, 0, 1920, 1040)),
            ]
        );
    }

    // ── 修正第 1 輪：更新收尾旗標在規劃後／建立期間翻轉（競態） ──────────────────────────────

    use std::cell::Cell;
    use std::rc::Rc;

    /// 假格線：記錄被 drop 的次數（＝真格線的 `DestroyWindow`）。
    struct FakeOverlay {
        work_area: PhysicalRect,
        color: Rgb,
        dropped: Rc<Cell<usize>>,
    }

    impl Drop for FakeOverlay {
        fn drop(&mut self) {
            self.dropped.set(self.dropped.get() + 1);
        }
    }

    impl OverlayWindow for FakeOverlay {
        fn work_area(&self) -> PhysicalRect {
            self.work_area
        }
        fn color(&self) -> Rgb {
            self.color
        }
        fn update(&mut self, work_area: PhysicalRect, color: Rgb) -> Result<bool, String> {
            self.work_area = work_area;
            self.color = color;
            Ok(true)
        }
    }

    /// 進入編輯版面、兩台顯示器、規劃當下旗標為假；`flip_after` 次讀取之後旗標變真（`None`＝永不翻轉）。
    /// 回傳（建立函式被呼叫的次數、留在集合裡的鍵、`apply_plan` 期間被 drop 的數量）。
    fn run_with_flag_flip(flip_after: Option<usize>) -> (usize, Vec<OverlayKey>, usize) {
        let monitors = two_monitors();
        let reads = Cell::new(0usize);
        let is_exiting = || {
            let n = reads.get() + 1;
            reads.set(n);
            flip_after.is_some_and(|limit| n > limit)
        };
        let dropped = Rc::new(Cell::new(0usize));
        let created = Cell::new(0usize);
        let mut create = |work_area: PhysicalRect, color: Rgb| {
            created.set(created.get() + 1);
            Ok(FakeOverlay {
                work_area,
                color,
                dropped: Rc::clone(&dropped),
            })
        };
        let mut overlays: Vec<(OverlayKey, FakeOverlay)> = Vec::new();
        apply_plan(
            &mut overlays,
            inputs(true, false, &monitors),
            &is_exiting,
            &mut create,
        );
        // 在集合（及其中的假格線）drop 之前取值，只算 `apply_plan` 期間的銷毀。
        let dropped_during_apply = dropped.get();
        let keys = overlays.iter().map(|(k, _)| k.clone()).collect();
        (created.get(), keys, dropped_during_apply)
    }

    #[test]
    fn flag_never_flipping_creates_every_overlay() {
        let (created, keys, dropped) = run_with_flag_flip(None);
        assert_eq!(created, 2);
        assert_eq!(
            keys,
            vec![
                OverlayKey::Monitor(device("A")),
                OverlayKey::Monitor(device("B"))
            ]
        );
        assert_eq!(dropped, 0);
    }

    /// 規劃後、第一次建立前旗標翻轉：一個都不建。
    #[test]
    fn flag_flipping_before_the_first_create_creates_nothing() {
        let (created, keys, _) = run_with_flag_flip(Some(0));
        assert_eq!(created, 0, "旗標為真後不得呼叫建立");
        assert!(keys.is_empty(), "{keys:?}");
    }

    /// 第一個建立期間旗標翻轉：建好的那個立即銷毀、不放進集合，也不再建第二個。
    #[test]
    fn flag_flipping_during_a_create_destroys_the_new_overlay_and_stops() {
        let (created, keys, dropped) = run_with_flag_flip(Some(1));
        assert_eq!(created, 1, "翻轉後不得再建第二個");
        assert!(keys.is_empty(), "建立期間翻轉的格線不得留下：{keys:?}");
        assert_eq!(dropped, 1, "建好的那個要立即銷毀");
    }

    /// 兩次建立之間旗標翻轉：第一個保留（由 `close_ui` 的清除收掉），第二個不建。
    #[test]
    fn flag_flipping_between_creates_stops_before_the_next_create() {
        let (created, keys, _) = run_with_flag_flip(Some(2));
        assert_eq!(created, 1);
        assert_eq!(keys, vec![OverlayKey::Monitor(device("A"))]);
    }

    /// 測試執行緒從未建立格線：清除回 0、不 panic。
    #[test]
    fn clearing_on_a_thread_without_overlays_returns_zero() {
        assert_eq!(clear_on_this_thread(), 0);
    }

    /// `widgets.rs` 正式碼（切在測試模組之前、統一換行）中，`signature` 起到頂層函式結尾（下一個
    /// `\n}\n`）的本體。
    fn production_fn_body(signature: &str) -> String {
        let src = include_str!("../widgets.rs").replace("\r\n", "\n");
        let production = &src[..src
            .find("#[cfg(test)]\nmod tests")
            .expect("widgets.rs 測試模組起點")];
        let start = production
            .find(signature)
            .unwrap_or_else(|| panic!("找不到 {signature}"));
        let body = &production[start..];
        let end = body.find("\n}\n").expect("找得到函式結尾");
        body[..end].to_string()
    }

    /// 掛接點（需要 `AppHandle`、無法在單元測試實際呼叫，改以原始碼斷言；同 `updater::exit` 測試的做法）：
    /// - `set_edit_mode` 切換成功後呼叫收斂點；
    /// - `relayout_all_widgets`（顯示器、DPI、工作區變更，及 `update_settings` 主題色變更都會走到）**每條
    ///   路徑**都呼叫收斂點——包含「沒有小工具視窗」「顯示器列舉為空」這兩個提早返回，故外層包裝本身不得
    ///   `return`；
    /// - 收斂點一律經 `run_on_main_thread`（主執行緒上 tauri 同步執行、其他執行緒只投遞不等待）。
    #[test]
    fn sync_grid_overlay_is_wired_into_every_trigger() {
        let edit = production_fn_body("pub fn set_edit_mode(");
        let sync_at = edit
            .find("sync_grid_overlay(&app)")
            .expect("set_edit_mode 要呼叫 sync_grid_overlay");
        let switched_at = edit
            .find("switch_edit_mode(")
            .expect("set_edit_mode 切換旗標");
        assert!(switched_at < sync_at, "要在切換旗標之後同步格線");

        let relayout = production_fn_body("pub fn relayout_all_widgets(");
        assert!(
            relayout.contains("relayout_widgets_now(app)"),
            "relayout_all_widgets 是包裝：\n{relayout}"
        );
        assert!(relayout.contains("sync_grid_overlay(app)"), "{relayout}");
        assert!(
            !relayout.contains("return"),
            "包裝不得提早返回（否則略過格線同步）：\n{relayout}"
        );

        let update = production_fn_body("pub fn update_settings(");
        assert!(
            update.contains("relayout_all_widgets(&app)"),
            "update_settings 經 relayout_all_widgets 同步格線（主題色）"
        );

        let sync = production_fn_body("pub fn sync_grid_overlay(");
        assert!(sync.contains(".run_on_main_thread("), "{sync}");
    }

    /// 工作區寬或高 ≤ 0（列舉異常）的顯示器不建格線（`GridOverlay::create` 本來就會拒絕，這裡先排除，
    /// 免得每次重排都記一次建立失敗）；這台若原本有格線就銷毀。
    #[test]
    fn degenerate_work_area_is_not_a_target() {
        let monitors = vec![
            monitor(Some(device("A")), rect(0, 0, 3840, 2088), true),
            monitor(Some(device("B")), rect(-1920, 0, 0, 1040), false),
        ];
        let have = [existing(
            OverlayKey::Monitor(device("B")),
            rect(-1920, 0, 1920, 1040),
            GOLD,
        )];
        let plan = plan_overlays(inputs(true, false, &monitors), &have);
        assert_eq!(
            plan,
            OverlayPlan {
                destroy: vec![OverlayKey::Monitor(device("B"))],
                update: vec![],
                create: vec![(OverlayKey::Monitor(device("A")), rect(0, 0, 3840, 2088))],
            }
        );
    }
}
