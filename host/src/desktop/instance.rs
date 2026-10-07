//! 啟動仲裁：補 tauri-plugin-single-instance 同時啟動的競態（task 5.1 fix round 1，
//! Codex `.superpowers/sdd/tasks/reviews/task-5.1-codex.md` [high]）。
//!
//! ## 缺陷
//!
//! plugin 2.5.0 的 Windows 實作（`platform_impl/windows.rs`）在 `Builder::build` 內先
//! `CreateMutexW("{identifier}-sim")`，**之後**才 `CreateWindowExW` 建交接視窗。第二個行程若
//! 剛好在兩者之間跑到同一段：`CreateMutexW` 回報 `ERROR_ALREADY_EXISTS`，`FindWindowW` 卻
//! 找不到視窗 → 該分支直接落到 `Ok(())`，**照常啟動**、建立第二組小工具。更糟的是這個行程
//! 手上握著那個 mutex 的 handle 卻沒建交接視窗，之後的重複啟動只要撞見同一狀態也會照常啟動。
//! 決定性重現：`host/tools/verify-5.1-concurrent.ps1 -Simulate plugin-mutex-only`（腳本自己
//! 持有 plugin 的 mutex、不建視窗＝模擬首個執行個體卡在兩步之間，修正前的宿主照常啟動）。
//!
//! ## 修法：自己的兩把具名 mutex，在 `tauri::Builder` 之前仲裁
//!
//! - **啟動鎖**（[`STARTUP_LOCK_NAME`]）：`main()` 一開始就等它（最多 [`STARTUP_WAIT`]）。
//!   勝出者（Primary）一路持有到 `.setup()` 閉包開頭才放（[`release_startup_lock`]）——
//!   Tauri 2.12 在 `Builder::build` 裡初始化 plugin（single-instance 在那裡建 mutex＋交接
//!   視窗），`.setup()` 閉包則在事件迴圈 `Ready` 才跑（`tauri-2.12.0/src/app.rs`：
//!   `initialize_plugins` 在 `build()` 尾端、使用者 `setup` 在 `RuntimeRunEvent::Ready`），
//!   所以啟動鎖涵蓋了 plugin 那段「mutex 已建、視窗未建」的整個臨界區。
//! - **執行個體鎖**（[`INSTANCE_LOCK_NAME`]）：取得啟動鎖後以逾時 0 試取。取得＝Primary，
//!   **終生持有**（行程結束由核心釋放＝abandoned，下一個行程照樣取得）；取不到＝已有執行個體
//!   （Secondary），立刻放開啟動鎖（讓其他 Secondary 不必排隊），照常進入 `Builder`，由 plugin
//!   找到交接視窗、轉交參數後 `process::exit(0)`——手動重複啟動開設定視窗、`--autostart`／
//!   `--restarted` 靜默退出的既有行為不變。
//! - Secondary 若**仍**走到 `.setup()`（plugin 找不到交接視窗＝首個執行個體正在結束、或其
//!   交接視窗建立失敗），`main.rs` 在建立任何視窗之前以 `cleanup_before_exit()`＋
//!   `process::exit(0)` 安全退出，不會變成第二組小工具。
//!
//! 為何不改成「等交接視窗出現」：那要依賴 plugin 的私有命名（`{id}-sic`／`{id}-siw`），升版
//! 就可能悄悄失效；兩把自有 mutex 只依賴「plugin 在 `build()` 內完成初始化」這個 Tauri 公開
//! 流程，以及 Win32 mutex 的所有權語意。
//!
//! ## 失敗模式
//!
//! | 情境 | 結果 |
//! |---|---|
//! | 首個執行個體在臨界區內崩潰 | 核心釋放它的兩把鎖（`WAIT_ABANDONED`＝取得），下一個行程成為 Primary，正常啟動 |
//! | 首個執行個體卡在啟動超過 [`STARTUP_WAIT`] | 新行程記錄後**退出**（不正常啟動）：卡住的那個仍持有執行個體鎖，再開一組只會變兩組 |
//! | 首個執行個體正在結束（交接視窗已毀） | Secondary 走到 `.setup()` 後安全退出；這次啟動的意圖（例如手動開設定）遺失，使用者再開一次即可 |
//! | `CreateMutexW`／`WaitForSingleObject` 失敗（例如同名物件被其他型別佔用） | [`StartupRole::Unarbitrated`]：記錄錯誤、退回只靠 plugin 的舊行為，不因仲裁機制本身故障而無法啟動 |
//!
//! mutex 所有權屬於**執行緒**：兩把鎖都在主執行緒取得，[`release_startup_lock`] 也必須在主
//! 執行緒呼叫（`.setup()` 閉包就在主執行緒跑）。啟動鎖放在 `thread_local!` 裡，在別的執行緒
//! 呼叫只會找不到而什麼都不做，不會誤放別人的鎖。
//!
//! 名稱用 `Local\` 前綴＝工作階段範圍，與 plugin 的 mutex（無前綴，行程在使用者工作階段時
//! 即落在同一個 session 命名空間）一致：單一執行個體是「每個登入工作階段一個」。

use std::cell::RefCell;
use std::time::Duration;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, HANDLE, WAIT_ABANDONED, WAIT_EVENT, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};

/// 啟動鎖名稱（見模組文件）。`host/tools/verify-5.1-concurrent.ps1` 的模擬模式用同一字串。
pub const STARTUP_LOCK_NAME: PCWSTR = w!("Local\\tw.fintools.fc-host.startup");
/// 執行個體鎖名稱（見模組文件）。
pub const INSTANCE_LOCK_NAME: PCWSTR = w!("Local\\tw.fintools.fc-host.instance");
/// 等待啟動鎖的上限。首個執行個體從 `main()` 到 `.setup()` 實測不到 1 秒；開機自啟時磁碟
/// 與 CPU 滿載可能拉長數倍，30 秒留足餘裕。
pub const STARTUP_WAIT: Duration = Duration::from_secs(30);

/// 本行程在仲裁後的角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupRole {
    /// 唯一執行個體：持有執行個體鎖；啟動鎖持有到 [`release_startup_lock`]。
    Primary,
    /// 已有執行個體：交給 plugin 交接；若仍走到 `.setup()` 必須安全退出。
    Secondary,
    /// 仲裁機制本身故障（Win32 呼叫失敗）：退回只靠 plugin 的舊行為。
    Unarbitrated,
}

impl StartupRole {
    /// fix F3（review task-5.1-r-opus.md [low]）：本行程可不可以寫啟動狀態（設定檔首次落地、
    /// 損壞檔備份與存回、開機自啟登錄同步）。落敗的 Secondary 只會交接後結束，不得寫；
    /// `Unarbitrated`（仲裁機制故障）可能就是唯一執行個體，照舊可寫（見
    /// `settings::load_for_arbitrated_startup`）。
    pub fn may_write_startup_state(self) -> bool {
        self != Self::Secondary
    }
}

/// 單次等待的結果分類（純資料，供 [`decide_role`] 單元測試）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockOutcome {
    /// `WAIT_OBJECT_0`。
    Acquired,
    /// `WAIT_ABANDONED`：前一個持有者未釋放就結束了；所有權照樣轉給本執行緒。
    Abandoned,
    /// `WAIT_TIMEOUT`。
    TimedOut,
    /// `CreateMutexW` 失敗或 `WAIT_FAILED`／其他非預期值。
    Failed,
}

impl LockOutcome {
    fn from_wait(event: WAIT_EVENT) -> Self {
        if event == WAIT_OBJECT_0 {
            Self::Acquired
        } else if event == WAIT_ABANDONED {
            Self::Abandoned
        } else if event == WAIT_TIMEOUT {
            Self::TimedOut
        } else {
            Self::Failed
        }
    }

    fn owned(self) -> bool {
        matches!(self, Self::Acquired | Self::Abandoned)
    }
}

/// 仲裁決策（純函式）。`None`＝等啟動鎖逾時，呼叫端必須退出、不得正常啟動。
///
/// `instance` 只在取得啟動鎖（或啟動鎖本身故障）後才會被評估——對應實作中「先取啟動鎖、
/// 再試執行個體鎖」的順序。
pub fn decide_role(
    startup: LockOutcome,
    instance: impl FnOnce() -> LockOutcome,
) -> Option<StartupRole> {
    match startup {
        LockOutcome::TimedOut => None,
        LockOutcome::Acquired | LockOutcome::Abandoned | LockOutcome::Failed => {
            let role = match instance() {
                LockOutcome::Acquired | LockOutcome::Abandoned => StartupRole::Primary,
                LockOutcome::TimedOut => StartupRole::Secondary,
                LockOutcome::Failed => StartupRole::Unarbitrated,
            };
            // 啟動鎖故障但執行個體鎖正常時，執行個體鎖仍能區分 Primary／Secondary，只是少了
            // 「涵蓋 plugin 臨界區」的序列化——降級為 Unarbitrated 以便記錄，行為上 Primary
            // 與 Unarbitrated 相同（都照常啟動）。
            if startup == LockOutcome::Failed && role == StartupRole::Primary {
                Some(StartupRole::Unarbitrated)
            } else {
                Some(role)
            }
        }
    }
}

/// 具名 mutex 的 handle；`owned` 時 drop 會先 `ReleaseMutex` 再 `CloseHandle`。
struct NamedMutex {
    handle: HANDLE,
    owned: bool,
}

impl NamedMutex {
    fn open(name: PCWSTR) -> windows::core::Result<Self> {
        // SAFETY: `name` 是 `w!` 產生的 'static、NUL 結尾 UTF-16 字串；安全屬性傳 None（預設
        // DACL）；初始不擁有（所有權一律經由 WaitForSingleObject 取得，才分得出 abandoned）。
        let handle = unsafe { CreateMutexW(None, false, name) }?;
        Ok(Self {
            handle,
            owned: false,
        })
    }

    fn wait(&mut self, timeout: Duration) -> LockOutcome {
        let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX - 1);
        // SAFETY: `self.handle` 是本結構持有、尚未關閉的有效 mutex handle。
        let outcome = LockOutcome::from_wait(unsafe { WaitForSingleObject(self.handle, ms) });
        self.owned = outcome.owned();
        outcome
    }
}

impl Drop for NamedMutex {
    fn drop(&mut self) {
        if self.owned {
            // SAFETY: handle 有效；只在本執行緒確實擁有時才釋放。若在非擁有者執行緒 drop，
            // ReleaseMutex 回傳 ERROR_NOT_OWNER（不會誤放別人的鎖），記錄即可。
            if let Err(err) = unsafe { ReleaseMutex(self.handle) } {
                log::warn!("啟動仲裁：ReleaseMutex 失敗：{err}");
            }
        }
        // SAFETY: handle 由 CreateMutexW 取得、只在這裡關閉一次。
        let _ = unsafe { CloseHandle(self.handle) };
    }
}

thread_local! {
    /// Primary（與 Unarbitrated 中取得啟動鎖者）持有到 `.setup()` 的啟動鎖（見模組文件）。
    static STARTUP_LOCK: RefCell<Option<NamedMutex>> = const { RefCell::new(None) };
}

/// 仲裁結果：角色＋終生持有的執行個體鎖。呼叫端（`main`）必須讓它活到行程結束——
/// drop 會釋放執行個體鎖。
pub struct StartupArbitration {
    role: StartupRole,
    _instance: Option<NamedMutex>,
}

impl StartupArbitration {
    pub fn role(&self) -> StartupRole {
        self.role
    }
}

/// 在 `tauri::Builder` 之前呼叫（主執行緒）。回傳 `None`＝等啟動鎖逾時，呼叫端應直接結束
/// 行程（見模組文件「失敗模式」）。
pub fn arbitrate_startup() -> Option<StartupArbitration> {
    arbitrate_startup_within(STARTUP_WAIT)
}

/// [`arbitrate_startup`]，等啟動鎖最多 `startup_wait`（dynamic-wallpaper 4.7b 複審 N3：`--restore-wallpaper`
/// 的總時限要涵蓋這一段）。
pub fn arbitrate_startup_within(startup_wait: Duration) -> Option<StartupArbitration> {
    let mut startup = match NamedMutex::open(STARTUP_LOCK_NAME) {
        Ok(m) => Some(m),
        Err(err) => {
            log::error!("啟動仲裁：建立啟動鎖失敗：{err}");
            None
        }
    };
    let startup_outcome = match startup.as_mut() {
        Some(m) => m.wait(startup_wait),
        None => LockOutcome::Failed,
    };

    let mut instance: Option<NamedMutex> = None;
    let role = decide_role(startup_outcome, || {
        match NamedMutex::open(INSTANCE_LOCK_NAME) {
            Ok(mut m) => {
                let outcome = m.wait(Duration::ZERO);
                instance = Some(m);
                outcome
            }
            Err(err) => {
                log::error!("啟動仲裁：建立執行個體鎖失敗：{err}");
                LockOutcome::Failed
            }
        }
    });

    let Some(role) = role else {
        log::warn!(
            "啟動仲裁：{} 秒內等不到啟動鎖（已有執行個體卡在啟動中），本行程退出",
            startup_wait.as_secs()
        );
        return None;
    };
    log::info!("啟動仲裁：啟動鎖={startup_outcome:?} 角色={role:?}");

    match role {
        StartupRole::Secondary => {
            // 不必排隊：立刻放開啟動鎖（drop），讓 plugin 交接；執行個體鎖本來就沒拿到。
            drop(startup);
            drop(instance.take());
        }
        StartupRole::Primary | StartupRole::Unarbitrated => {
            STARTUP_LOCK.with(|slot| *slot.borrow_mut() = startup);
        }
    }

    Some(StartupArbitration {
        role,
        _instance: instance.filter(|m| m.owned),
    })
}

/// 持有中的執行個體鎖（[`try_acquire_instance_lock`]）；drop 時釋放。
pub struct InstanceLock {
    _mutex: NamedMutex,
}

/// dynamic-wallpaper task 4.7b（審查 F5）：**不等待**地試取執行個體鎖（不碰啟動鎖）。`--restore-wallpaper`
/// 等宿主結束時輪詢用：每次探測立即返回，等待的總時間由呼叫端的上限決定。取得＝沒有宿主在執行；
/// 持有期間啟動的宿主是 Secondary、安全退出。必須在同一條執行緒持有到不再需要（mutex 屬於執行緒）。
pub fn try_acquire_instance_lock() -> Option<InstanceLock> {
    let mut m = NamedMutex::open(INSTANCE_LOCK_NAME).ok()?;
    m.wait(Duration::ZERO)
        .owned()
        .then_some(InstanceLock { _mutex: m })
}

/// 放開啟動鎖（`.setup()` 閉包開頭呼叫，主執行緒）。冪等；在非取得鎖的執行緒呼叫不做任何事。
pub fn release_startup_lock() {
    STARTUP_LOCK.with(|slot| drop(slot.borrow_mut().take()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_on_startup_lock_means_exit_without_touching_instance_lock() {
        let mut touched = false;
        let role = decide_role(LockOutcome::TimedOut, || {
            touched = true;
            LockOutcome::Acquired
        });
        assert_eq!(role, None, "等啟動鎖逾時必須退出，不得正常啟動");
        assert!(!touched, "逾時時不得再去搶執行個體鎖");
    }

    #[test]
    fn owning_instance_lock_is_primary_even_if_previous_holder_died() {
        for startup in [LockOutcome::Acquired, LockOutcome::Abandoned] {
            for instance in [LockOutcome::Acquired, LockOutcome::Abandoned] {
                assert_eq!(
                    decide_role(startup, || instance),
                    Some(StartupRole::Primary),
                    "startup={startup:?} instance={instance:?}"
                );
            }
        }
    }

    /// fix F3（review task-5.1-r-opus.md [low]）：只有落敗的 Secondary 不得寫設定檔／登錄；
    /// 仲裁故障（Unarbitrated）可能就是唯一執行個體，照舊可寫。
    #[test]
    fn only_secondary_must_not_write_startup_state() {
        assert!(StartupRole::Primary.may_write_startup_state());
        assert!(StartupRole::Unarbitrated.may_write_startup_state());
        assert!(!StartupRole::Secondary.may_write_startup_state());
    }

    #[test]
    fn instance_lock_held_elsewhere_is_secondary() {
        for startup in [
            LockOutcome::Acquired,
            LockOutcome::Abandoned,
            LockOutcome::Failed,
        ] {
            assert_eq!(
                decide_role(startup, || LockOutcome::TimedOut),
                Some(StartupRole::Secondary),
                "startup={startup:?}：已有執行個體就必須是 Secondary"
            );
        }
    }

    #[test]
    fn broken_arbitration_falls_back_to_unarbitrated() {
        assert_eq!(
            decide_role(LockOutcome::Acquired, || LockOutcome::Failed),
            Some(StartupRole::Unarbitrated)
        );
        assert_eq!(
            decide_role(LockOutcome::Failed, || LockOutcome::Acquired),
            Some(StartupRole::Unarbitrated),
            "啟動鎖故障時失去臨界區序列化，標為 Unarbitrated 以便記錄"
        );
    }

    #[test]
    fn wait_event_classification() {
        assert_eq!(LockOutcome::from_wait(WAIT_OBJECT_0), LockOutcome::Acquired);
        assert_eq!(
            LockOutcome::from_wait(WAIT_ABANDONED),
            LockOutcome::Abandoned
        );
        assert_eq!(LockOutcome::from_wait(WAIT_TIMEOUT), LockOutcome::TimedOut);
        assert_eq!(
            LockOutcome::from_wait(WAIT_EVENT(u32::MAX)),
            LockOutcome::Failed
        );
    }

    /// 真的 Win32 mutex：同一行程內另開執行緒持有執行個體鎖，本執行緒試取＝逾時（Secondary
    /// 的判定依據）；持有者執行緒結束而未釋放＝abandoned，本執行緒取得（Primary）。
    /// 用測試專屬名稱，不干擾實際執行中的宿主。
    #[test]
    fn real_mutex_timeout_and_abandoned_semantics() {
        let name: Vec<u16> = format!("Local\\fc-host-test-instance-{}\0", std::process::id())
            .encode_utf16()
            .collect();
        let name_ptr = name.as_ptr() as usize;

        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let (quit_tx, quit_rx) = std::sync::mpsc::channel::<()>();
        let holder = std::thread::spawn(move || {
            let mut m = NamedMutex::open(PCWSTR(name_ptr as *const u16)).expect("CreateMutexW");
            assert_eq!(m.wait(Duration::ZERO), LockOutcome::Acquired);
            held_tx.send(()).unwrap();
            quit_rx.recv().unwrap();
            // 模擬行程崩潰：不 ReleaseMutex 就結束執行緒（只關 handle）。
            m.owned = false;
        });
        held_rx.recv().unwrap();

        let mut mine = NamedMutex::open(PCWSTR(name.as_ptr())).expect("CreateMutexW");
        assert_eq!(mine.wait(Duration::ZERO), LockOutcome::TimedOut);

        quit_tx.send(()).unwrap();
        holder.join().unwrap();
        assert_eq!(mine.wait(Duration::from_secs(5)), LockOutcome::Abandoned);
        assert!(mine.owned);
    }
}
