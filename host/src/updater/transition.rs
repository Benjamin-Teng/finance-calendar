//! 過渡期原子狀態機（design.md D3「過渡期狀態機」）。
//!
//! 啟動時讀到舊標記（上次非正常結束）時，setup 不阻塞、先不建立小工具與桌布協調，等更新任務回報
//! （上限 60 秒）。這段期間系統匣圖示已在，使用者可能按「結束」或再點一次開始功能表，所以用一個
//! 狀態表示過渡期，**只能從「等待」轉移一次**：
//!
//! ```text
//!                  ┌─ begin_install ───▶ Installing ──install_failed_retreat──┐
//!                  │   （有新版；之後不建 UI，        （安裝在 on_before_exit │
//!                  │    交接請求丟棄）                 之前失敗＝退路）        │
//!   Waiting ───────┤                                                          ▼
//!   （初始；交接排隊）├─ begin_build_ui ──────────────────────────────▶ BuildingUi ──finish_build_ui──▶ UiReady
//!                  │   （沒有新版／失敗／60 秒逾時）                    （交接仍排隊）                  （交接即時處理）
//!                  │
//!                  └─ begin_quit ─────▶ Quit（取消更新任務、不還原桌布、不存主題、刪標記；交接丟棄）
//!
//!   BuildingUi ──begin_quit──▶ Quit（建立 UI 已決定、但投遞到主執行緒的閉包還沒跑：從主執行緒看 UI 尚未存在，
//!                              結束比照過渡期；閉包開頭發現已是 Quit 就略過建立）
//!                              也可能發生在閉包**執行途中**：建立小工具視窗時 WebView2 以巢狀訊息泵等待，
//!                              `--restore-wallpaper` 的交接（跨行程 SendMessage）會在其中被派送而轉到 Quit（最終
//!                              審查 M2a 引入的路徑；系統匣「結束」經事件迴圈 proxy，不會巢狀進入）。此時已建的
//!                              小工具隨行程結束，`build_ui` 在啟動協調迴圈等背景元件之前以 `is_quitting` 再問一次、
//!                              已是 Quit 就返回（複審 M-A）
//! ```
//!
//! 「安裝」內保留的退路是同一個呼叫端在失敗時執行 [`Transition::install_failed_retreat`]，不是第二次
//! 從「等待」轉移：整條路徑上「建立 UI」仍只會發生一次（`BuildingUi` 只能從 `Waiting` 或 `Installing`
//! 進入，只會離開到 `UiReady` 或 `Quit`）。沒有舊標記的一般啟動以 [`Transition::building_ui`] 起始（最終審查 M2）：
//! setup 同步建立 UI、建完轉到 `UiReady`，之後所有交接即時處理。不從 `UiReady` 起始的理由：`build_ui` 在 setup
//! 內同步執行，期間主執行緒不處理 `run_on_main_thread`；若 phase 已是 `UiReady`，`--autostart` 後很快下載完成的
//! 自動安裝會在 UI 建好之前開始收尾、等不到主執行緒而逾時結束（系統匣圖示沒有移除）。
//!
//! 所有狀態與交接佇列由同一個 `Mutex` 保護，轉移與「收到交接」互斥，沒有「剛轉移完又被排進舊佇列」的競態。

use std::sync::Mutex;

use super::cancel::CancelToken;

/// 佇列上限：60 秒過渡期內的正常使用量遠低於此，防止異常情況下無限成長。
const MAX_QUEUED_HANDOFFS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// 過渡期：等更新任務回報。
    Waiting,
    /// 有新版、正在同步安裝（沒有 UI）。
    Installing,
    /// 正在建立小工具與桌布協調（交接仍排隊，建完才處理）。
    BuildingUi,
    /// UI 已建立（過渡期之後，或沒有過渡期時 setup 同步建完）。
    UiReady,
    /// UI 建好之前（「等待」或「建立 UI」，後者含沒有舊標記時 setup 同步建立途中）按了系統匣「結束」或收到
    /// `--restore-wallpaper` 交接。延後的自動安裝在此丟棄；與安裝互斥（安裝只在 `UiReady` 或從「等待」開始）。
    Quit,
}

/// 收到 single-instance 交接請求時的處置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandoffDisposition {
    /// 已排入佇列，轉到「建立 UI」後再處理。
    Queued,
    /// 立即處理。
    ProcessNow,
    /// 丟棄（已轉到「安裝」或「結束」，或佇列已滿）；已記錄。
    Dropped,
}

/// 系統匣「結束」該怎麼處理。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitDisposition {
    /// 走原本的結束流程（還原桌布、存主題）。
    Normal,
    /// UI 尚未存在時結束（過渡期等待中，或建立 UI 已決定但尚未執行）：呼叫端取消更新任務、不還原、不存
    /// 主題、刪標記後結束。**不得**走一般流程——它找不到協調迴圈時會把桌布主題存成「不接管」。
    QuitDuringTransition,
    /// 已經在結束了（上一次「結束」的 `app.exit(0)` 尚未生效）：忽略重複的按下。
    AlreadyQuitting,
    /// 正在同步安裝：安裝完會自行結束，忽略這次按下。
    IgnoreInstalling,
}

struct Inner {
    phase: Phase,
    queue: Vec<Vec<String>>,
}

pub struct Transition {
    inner: Mutex<Inner>,
    /// 離開「等待」時取消：60 秒逾時看門狗隨之醒來退出。
    left_waiting: CancelToken,
}

impl Transition {
    /// 過渡期起始（讀到舊標記）。
    pub fn waiting() -> Self {
        Self::with_phase(Phase::Waiting)
    }

    /// 沒有過渡期（沒有舊標記）的正式起點：呼叫端在 setup 內**同步**建立 UI，建完呼叫
    /// [`Self::finish_build_ui`]（最終審查 M2）。建立期間 UI 尚不存在：更新任務已在背景跑，`--autostart` 開機、
    /// 網路很快時下載可能先完成——自動安裝看到的不是 `UiReady`，延後到建好後在背景重新評估；建立途中（WebView2 巢狀訊息泵）
    /// 派送進來的交接照過渡期規則（一般交接排隊、建完重放；`--restore-wallpaper` 轉到「結束」）。
    pub fn building_ui() -> Self {
        let t = Self::with_phase(Phase::BuildingUi);
        t.left_waiting.cancel();
        t
    }

    /// UI 已建立（測試起點；正式啟動沒有舊標記時用 [`Self::building_ui`]）：交接即時處理。
    #[cfg(test)]
    pub fn ui_ready() -> Self {
        let t = Self::with_phase(Phase::UiReady);
        t.left_waiting.cancel();
        t
    }

    fn with_phase(phase: Phase) -> Self {
        Self {
            inner: Mutex::new(Inner {
                phase,
                queue: Vec::new(),
            }),
            left_waiting: CancelToken::new(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn phase(&self) -> Phase {
        self.lock().phase
    }

    /// 離開「等待」時會被取消的把手（逾時看門狗用）。
    pub fn left_waiting_token(&self) -> CancelToken {
        self.left_waiting.clone()
    }

    /// 「等待」→「安裝」。成功時丟棄佇列中的交接並記錄。
    pub fn begin_install(&self) -> bool {
        let mut inner = self.lock();
        if inner.phase != Phase::Waiting {
            return false;
        }
        inner.phase = Phase::Installing;
        let dropped = std::mem::take(&mut inner.queue);
        drop(inner);
        self.left_waiting.cancel();
        log_dropped("轉移到「安裝」", &dropped);
        true
    }

    /// 「等待」→「建立 UI」（沒有新版、失敗或逾時）。成功時呼叫端必須建立 UI，建完呼叫
    /// [`Self::finish_build_ui`]。
    pub fn begin_build_ui(&self) -> bool {
        let mut inner = self.lock();
        if inner.phase != Phase::Waiting {
            return false;
        }
        inner.phase = Phase::BuildingUi;
        drop(inner);
        self.left_waiting.cancel();
        true
    }

    /// 「安裝」→「建立 UI」：同步安裝在 `on_before_exit` 之前失敗時的退路（design.md D3）。
    pub fn install_failed_retreat(&self) -> bool {
        let mut inner = self.lock();
        if inner.phase != Phase::Installing {
            return false;
        }
        inner.phase = Phase::BuildingUi;
        true
    }

    /// 「建立 UI」→「UI 已建立」，回傳要依序處理的排隊交接。
    pub fn finish_build_ui(&self) -> Vec<Vec<String>> {
        let mut inner = self.lock();
        if inner.phase != Phase::BuildingUi {
            return Vec::new();
        }
        inner.phase = Phase::UiReady;
        std::mem::take(&mut inner.queue)
    }

    /// 「等待」或「建立 UI」→「結束」。成功時丟棄佇列中的交接並記錄。
    pub fn begin_quit(&self) -> bool {
        let mut inner = self.lock();
        if !matches!(inner.phase, Phase::Waiting | Phase::BuildingUi) {
            return false;
        }
        inner.phase = Phase::Quit;
        let dropped = std::mem::take(&mut inner.queue);
        drop(inner);
        self.left_waiting.cancel();
        log_dropped("轉移到「結束」", &dropped);
        true
    }

    /// 系統匣「結束」的處置；在「等待」或「建立 UI」時同時完成轉移（原子）。
    pub fn quit_disposition(&self) -> QuitDisposition {
        let phase = self.lock().phase;
        match phase {
            Phase::Waiting | Phase::BuildingUi => {
                if self.begin_quit() {
                    QuitDisposition::QuitDuringTransition
                } else {
                    // 兩次之間被別條路徑轉移走了：重問一次。
                    self.quit_disposition()
                }
            }
            Phase::Installing => QuitDisposition::IgnoreInstalling,
            Phase::UiReady => QuitDisposition::Normal,
            Phase::Quit => QuitDisposition::AlreadyQuitting,
        }
    }

    /// 收到 single-instance 交接（非 `--restore-wallpaper`、非自動化重複啟動）時的處置。
    pub fn offer_handoff(&self, args: Vec<String>) -> HandoffDisposition {
        let mut inner = self.lock();
        match inner.phase {
            Phase::UiReady => HandoffDisposition::ProcessNow,
            Phase::Waiting | Phase::BuildingUi => {
                if inner.queue.len() >= MAX_QUEUED_HANDOFFS {
                    log::warn!(target: super::LOG_TARGET, "更新過渡期：交接佇列已滿，丟棄 args={args:?}");
                    return HandoffDisposition::Dropped;
                }
                log::info!(target: super::LOG_TARGET, "更新過渡期：交接請求排隊，待 UI 建立後處理（args={args:?}）");
                inner.queue.push(args);
                HandoffDisposition::Queued
            }
            Phase::Installing | Phase::Quit => {
                log::info!(target: super::LOG_TARGET,
                    "更新過渡期：已轉到 {:?}，丟棄交接請求（args={args:?}）",
                    inner.phase
                );
                HandoffDisposition::Dropped
            }
        }
    }
}

fn log_dropped(reason: &str, dropped: &[Vec<String>]) {
    if !dropped.is_empty() {
        log::info!(target: super::LOG_TARGET, "更新過渡期：{reason}，丟棄 {} 個排隊中的交接請求", dropped.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        vec!["fc-host.exe".to_owned(), s.to_owned()]
    }

    #[test]
    fn starts_in_requested_phase() {
        assert_eq!(Transition::waiting().phase(), Phase::Waiting);
        let ready = Transition::ui_ready();
        assert_eq!(ready.phase(), Phase::UiReady);
        assert_eq!(
            ready.offer_handoff(args("--x")),
            HandoffDisposition::ProcessNow,
            "沒有過渡期時交接照舊即時處理"
        );
        assert!(ready.left_waiting_token().is_cancelled());
    }

    #[test]
    fn no_marker_start_builds_ui_then_becomes_ready() {
        // 最終審查 M2：沒有舊標記時從「建立 UI」起始，setup 同步建完才轉到「UI 已建立」。
        let t = Transition::building_ui();
        assert_eq!(t.phase(), Phase::BuildingUi);
        assert!(
            t.left_waiting_token().is_cancelled(),
            "沒有等待階段，看門狗不需要"
        );
        assert!(!t.begin_install(), "不是過渡期：不得轉到同步安裝");
        assert!(!t.begin_build_ui(), "建立 UI 只會發生一次");
        // 建立途中（巢狀訊息泵）派送進來的一般交接排隊，建完依序重放。
        assert_eq!(t.offer_handoff(args("--a")), HandoffDisposition::Queued);
        assert_eq!(t.finish_build_ui(), vec![args("--a")]);
        assert_eq!(t.phase(), Phase::UiReady);
        assert_eq!(t.offer_handoff(args("--b")), HandoffDisposition::ProcessNow);
        assert_eq!(t.quit_disposition(), QuitDisposition::Normal);

        // 建立途中收到 `--restore-wallpaper` 交接（begin_quit）：轉到「結束」，建完的收尾不再吐出交接。
        let t = Transition::building_ui();
        assert!(t.begin_quit());
        assert!(t.finish_build_ui().is_empty());
        assert_eq!(t.phase(), Phase::Quit);
    }

    #[test]
    fn transition_install_happens_once_and_drops_queued_handoffs() {
        let t = Transition::waiting();
        assert_eq!(t.offer_handoff(args("--a")), HandoffDisposition::Queued);
        assert!(t.begin_install());
        assert_eq!(t.phase(), Phase::Installing);
        // 只能轉移一次：其餘兩條路都失敗。
        assert!(!t.begin_build_ui());
        assert!(!t.begin_quit());
        assert!(!t.begin_install());
        // 轉到「安裝」：佇列已丟棄，新的交接也丟棄，之後也不會有人把它們吐出來。
        assert_eq!(t.offer_handoff(args("--b")), HandoffDisposition::Dropped);
        assert!(t.install_failed_retreat() && t.finish_build_ui().is_empty());
        assert!(t.left_waiting_token().is_cancelled());
    }

    #[test]
    fn transition_build_ui_happens_once_and_replays_queue_in_order() {
        let t = Transition::waiting();
        assert_eq!(t.offer_handoff(args("--a")), HandoffDisposition::Queued);
        assert_eq!(t.offer_handoff(args("--b")), HandoffDisposition::Queued);
        assert!(t.begin_build_ui());
        assert!(!t.begin_install(), "已轉到建立 UI，不能再轉到安裝");
        // 建立中收到的也排隊，建完一併處理。
        assert_eq!(t.offer_handoff(args("--c")), HandoffDisposition::Queued);
        let queued = t.finish_build_ui();
        assert_eq!(queued, vec![args("--a"), args("--b"), args("--c")]);
        assert_eq!(t.phase(), Phase::UiReady);
        assert_eq!(t.offer_handoff(args("--d")), HandoffDisposition::ProcessNow);
        assert!(
            t.finish_build_ui().is_empty(),
            "UI 已建立後再呼叫不得重複吐出"
        );
    }

    #[test]
    fn transition_quit_happens_once_and_drops_handoffs() {
        let t = Transition::waiting();
        assert_eq!(t.offer_handoff(args("--a")), HandoffDisposition::Queued);
        assert_eq!(t.quit_disposition(), QuitDisposition::QuitDuringTransition);
        assert_eq!(t.phase(), Phase::Quit);
        assert!(!t.begin_install());
        assert!(
            !t.begin_build_ui(),
            "使用者已結束，晚到的更新結果不得再建立 UI"
        );
        assert_eq!(t.offer_handoff(args("--b")), HandoffDisposition::Dropped);
        assert!(t.finish_build_ui().is_empty());
        assert!(t.left_waiting_token().is_cancelled());
    }

    #[test]
    fn install_failure_before_on_before_exit_retreats_to_build_ui() {
        let t = Transition::waiting();
        assert!(t.begin_install());
        // 安裝中交接被丟棄，退路之後也不會復活它。
        assert_eq!(t.offer_handoff(args("--a")), HandoffDisposition::Dropped);
        assert!(
            t.install_failed_retreat(),
            "安裝在 on_before_exit 前失敗：改走建立 UI"
        );
        assert_eq!(t.phase(), Phase::BuildingUi);
        assert!(!t.install_failed_retreat(), "退路只能走一次");
        assert!(t.finish_build_ui().is_empty());
        assert_eq!(t.phase(), Phase::UiReady);
    }

    #[test]
    fn retreat_requires_installing() {
        let t = Transition::waiting();
        assert!(!t.install_failed_retreat(), "沒有進入安裝就沒有退路可走");
        assert_eq!(t.phase(), Phase::Waiting);
    }

    #[test]
    fn quit_disposition_by_phase() {
        let installing = Transition::waiting();
        installing.begin_install();
        assert_eq!(
            installing.quit_disposition(),
            QuitDisposition::IgnoreInstalling
        );

        // 建立 UI 已決定、閉包還沒跑：UI 尚不存在，結束比照過渡期（不得走會存主題的一般流程）。
        let building = Transition::waiting();
        building.begin_build_ui();
        assert_eq!(
            building.quit_disposition(),
            QuitDisposition::QuitDuringTransition
        );
        assert_eq!(building.phase(), Phase::Quit);
        assert!(
            building.finish_build_ui().is_empty(),
            "結束後建立 UI 的收尾不得再吐出交接"
        );
        assert_eq!(
            building.quit_disposition(),
            QuitDisposition::AlreadyQuitting,
            "重複按「結束」忽略"
        );

        let waiting = Transition::waiting();
        assert_eq!(
            waiting.quit_disposition(),
            QuitDisposition::QuitDuringTransition
        );
        assert_eq!(waiting.quit_disposition(), QuitDisposition::AlreadyQuitting);

        // UI 已建立後才是一般流程。
        let built = Transition::waiting();
        built.begin_build_ui();
        built.finish_build_ui();
        assert_eq!(built.quit_disposition(), QuitDisposition::Normal);

        assert_eq!(
            Transition::ui_ready().quit_disposition(),
            QuitDisposition::Normal,
            "沒有過渡期時系統匣結束走原本流程"
        );
    }

    #[test]
    fn queue_is_bounded() {
        let t = Transition::waiting();
        for i in 0..MAX_QUEUED_HANDOFFS {
            assert_eq!(
                t.offer_handoff(args(&format!("--{i}"))),
                HandoffDisposition::Queued
            );
        }
        assert_eq!(
            t.offer_handoff(args("--overflow")),
            HandoffDisposition::Dropped
        );
    }

    #[test]
    fn concurrent_transitions_pick_exactly_one_winner() {
        use std::sync::Arc;
        for _ in 0..50 {
            let t = Arc::new(Transition::waiting());
            let handles: Vec<_> = (0..3)
                .map(|i| {
                    let t = Arc::clone(&t);
                    std::thread::spawn(move || match i {
                        0 => t.begin_install(),
                        1 => t.begin_build_ui(),
                        _ => t.begin_quit(),
                    })
                })
                .collect();
            let won: Vec<bool> = handles.into_iter().map(|h| h.join().unwrap()).collect();
            let wins = won.iter().filter(|w| **w).count();
            assert!(wins >= 1, "至少有一個轉移成功");
            // 從「等待」離開只有一次：安裝與建立 UI 互斥；安裝成功就沒有其他轉移能成功。
            assert!(!(won[0] && won[1]), "安裝與建立 UI 不得同時成功");
            if won[0] {
                assert_eq!(wins, 1, "已轉到安裝，結束與建立 UI 都不得再成功");
            }
            // 唯一允許的連續轉移：建立 UI 已決定、尚未執行時按「結束」（I2）。
            assert!(wins <= 2);
        }
    }
}
