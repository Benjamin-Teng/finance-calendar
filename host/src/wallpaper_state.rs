//! 桌布接管狀態檔 `wallpaper-state.json` 與接管狀態機（dynamic-wallpaper task 4.4；design.md D5；
//! specs/desktop-wallpaper「接管前記錄原桌布」「原桌布為 Windows 焦點時先提醒」「使用者自行更換
//! 桌布時讓位」「還原原桌布」「逐螢幕依原生解析度出圖」的新螢幕一段、「輸出檔數量有上限」的備份
//! 一段）。
//!
//! 這是最直接保護使用者桌面的部分：任何會改桌布的動作之前，原桌布都必須已經安全地寫在磁碟上；
//! 任何不確定的情況（狀態檔讀不懂、讀回失敗）都選擇「不動使用者的桌布」。
//!
//! ## 分工
//!
//! - 純邏輯（可單元測試）：[`judge_readback`]（讓位判定）、[`classify_for_restore`]（還原前的
//!   讓位判定）、[`restore_target`]／[`unknown_original_fallback`]（還原來源）、[`same_file`]／
//!   [`is_host_output_file`]（路徑比對加檔案識別）、[`StatePaths::is_themes_cache`]（Windows 的
//!   桌布快取資料夾）、[`monitor_key`]（螢幕鍵）、狀態檔的序列化與驗證。
//! - I/O 經注入：桌布 API 是 [`WallpaperService`]（測試以 `spawn_with` 注入假後端）、登錄是
//!   [`RegistryStore`]（測試以記憶體內的假登錄）、檔案位置是 [`StatePaths`]（測試用暫存資料夾）、
//!   檔案識別是 `FileIdentity`（正式為 `Win32FileIdentity`）、時間由呼叫端以 [`TakeoverIo::now`]
//!   帶入；等待時間都在 [`TakeoverConfig`]，測試用短值。
//! - **不接排程與事件（4.7）、不渲染（4.5）**：本模組只在被呼叫時動作；宿主啟動時不建立它。
//!
//! ## 狀態檔（版本 [`STATE_VERSION`]＝1）
//!
//! 位置：宿主設定資料夾（`settings.json` 同一個資料夾）的 [`STATE_FILE_NAME`]
//! （[`default_paths`]）。寫入一律經本模組自己的 [`write_durable`]（暫存檔＋`sync_all`＋`rename`；
//! `crate::settings::write_atomic` 不做 `sync_all`，維持 desktop-widget-host 原行為）。每次寫入成功後，
//! 刪掉備份資料夾裡沒有人用的 `m-*` 備份（[`remove_orphan_backups`]：任何紀錄的 `backup` 或
//! `original_wallpaper` 指向它、它是某台螢幕目前的桌布——離線螢幕讀得到時也算——或它列在
//! `superseded_backups` 時都保留）。備份檔名是
//! `<螢幕鍵>.<副檔名>`；這個檔正被其他紀錄引用或正在顯示時，改用 `<螢幕鍵>-<n>.<副檔名>`，
//! 絕不覆寫（修正輪 5，[`backup_original`]）。每筆紀錄的 `backup` 仍只有一份。
//!
//! ```text
//! {
//!   "version": 1,
//!   "status": "not_taken_over" | "taken_over",
//!   "original": null | {                       // 有值 若且唯若 status == taken_over
//!     "recorded_at": <Unix 秒>,
//!     "position": <DESKTOP_WALLPAPER_POSITION 原值>,
//!     "background_color": <COLORREF>,
//!     "slideshow_status": <GetStatus 旗標>,
//!     "slideshow": null | { "items": [路徑...], "options": <旗標>, "tick_ms": <毫秒> },
//!                                              // 有值 若且唯若 slideshow_status 含 SLIDESHOW
//!     "registry": {                            // HKCU\Control Panel\Desktop 三個值的忠實快照
//!       "wallpaper":       { "state": "missing" } | { "state": "present", "kind": <型別>, "data_hex": "<原始位元組>" },
//!       "wallpaper_style": { ... }, "tile_wallpaper": { ... }
//!     },
//!     "spotlight": { "enabled_state": null | <u32>, "background_type": null | <u32> },  // 只供診斷
//!     "solid_color": true,                     // task 6.1；原桌布是全域純色時才寫出，缺少＝否
//!     "all_monitors": true                     // installer-auto-update 4.x；原桌布是「全部螢幕」設定時才寫出，缺少＝否
//!   },
//!   "monitors": {                              // 鍵＝螢幕鍵（monitor_key）
//!     "m-<16 位十六進位>": {
//!       "device_path": "<GetMonitorDevicePathAt>",
//!       "original_wallpaper": "<GetWallpaper 讀回的原路徑；空字串＝純色>",
//!       "backup": null | "<wallpaper\original\ 下的檔名>",
//!       "recorded_at": <Unix 秒>,
//!       "last_set": null | "<宿主上次（準備）設定的輸出圖路徑>",
//!       "host_applied": <是否曾讀回確認螢幕上是宿主的圖>,
//!       "pending_restore": <待還原：未接管但可能仍顯示宿主的圖>,  // 缺少時為 false
//!       "original_unknown": <原桌布未知：記錄當下就是宿主的圖>,    // 缺少時為 false
//!       "stale": <沿用自前一次接管、這次開始時離線的紀錄>,         // 缺少時為 false
//!       "original_file_id": { "volume_serial": <u32>, "file_index": <u64> },
//!                                              // task 6.4；記錄當下原圖的檔案識別，查不到時不寫出
//!       "solid_transition_readback": "<路徑>"  // task 6.1 修正輪 1；全域純色原桌布設定前的過渡讀回，沒有時不寫出
//!     }
//!   },
//!   "last_event": null | { "kind": "restored", "at", "reason", "offline_unverified", "warnings" }
//!                      | { "kind": "yielded", "at", "monitors" },              // 只供診斷
//!   "restore_in_progress": {                   // 4.7b；只在接管中且還原開始後存在，沒有時不寫出
//!     "reason": "not_takeover" | "tray_exit" | "command_line" | "safety_valve",
//!     "detail": "<安全閥的說明>",               // 只有 safety_valve 有
//!     "started_at": <Unix 秒>
//!   },
//!   "explorer_baseline": {                     // 4.9；沒有基準時不寫出
//!     "pid": <explorer PID>, "gdi": <GDI 物件數>, "recorded_at": <Unix 秒>
//!   },
//!   "superseded_backups": ["<備份檔名>", ...]  // task 6.4；讓位／還原時被取代的舊備份，空的時候不寫出
//! }
//! ```
//!
//! `superseded_backups`、`original_file_id`（task 6.4，審查 R1-M3b／M4）：讓位或還原時發現使用者自換、
//! 紀錄改採納新桌布時，舊備份不在同一次寫入刪掉，而是記在這裡、**保留到下一次接管開始**（判定萬一
//! 是誤判，原圖仍在）；接管開始時清空，之後的寫入才把沒人用的刪掉。`original_file_id` 給還原時判斷
//! 「原路徑上的檔是不是已被換掉」（[`restore_target`]）。版本仍是 1：舊檔沒有這兩個欄位＝空。
//!
//! `restore_in_progress`（task 4.7b）：[`WallpaperTakeover::restore`] 在接管中開始還原前寫入（只寫
//! 狀態檔、不清孤兒備份），還原成功時與「未接管」同一次寫入清除；還原失敗、逾時、當機時留著，下次
//! 啟動由 4.7 續做。版本仍是 1：舊檔沒有這個欄位＝沒有標記；原因不認得＝讀不懂（封鎖）。
//!
//! `explorer_baseline`（task 4.9；design.md D11）：explorer 資源安全閥的基準。4.7 依排程器的
//! `Decision::rebaseline` 以 [`WallpaperTakeover::set_explorer_baseline`] 寫入（只寫狀態檔）；未接管時
//! 寫入的值是「接管開始時的讀值」，接管開始時帶進新的接管狀態；回到未接管（還原成功、讓位）時清除；
//! 使用者重新開啟接管時 4.7 以 [`WallpaperTakeover::clear_explorer_baseline`] 清除。判定只用
//! [`WallpaperTakeover::valve_baseline`]（接管中才有值）。版本仍是 1：舊檔沒有這個欄位＝沒有基準；
//! 型別不符＝讀不懂（封鎖）。
//!
//! 讀檔規則：`version` 不是 1、JSON 壞掉、欄位缺漏或型別不符、登錄值編碼無效、`status` 與
//! `original` 不一致——一律**不覆寫原檔、不接管、也不還原**（[`BlockReason`]），另寫一份內容相同的
//! `<檔名>.corrupt-<Unix 秒>` 供除錯（已有內容相同的副本時不重複寫）。原檔不改名：改名後下次啟動
//! 會當成「從未接管」，把螢幕上宿主的圖記成原桌布，等於遺失使用者的原桌布記錄。登錄值絕不從缺漏
//! 或解析失敗預設成「不存在」（還原時會誤刪值）。
//!
//! 讀不到檔（共用違規、權限……；不存在不算）是**暫時性**的，不封鎖：這段期間不接管、不還原
//! （[`RefuseReason::StateUnavailable`]／[`RestoreResult::StateUnavailable`]），之後每個操作開始時
//! 重讀；系統匣結束與 `--restore-wallpaper` 會隔 [`TakeoverConfig::transient_retry_delay`] 再重讀
//! 一次才放棄。
//!
//! ## 狀態圖
//!
//! ```text
//!                         ┌──────────────────────── 讀檔失敗／損壞／未知版本 ─────────┐
//!                         │                                                         ▼
//!   (啟動) ──讀檔──► 未接管 NotTakenOver ◄──────────────┐                   封鎖 Blocked
//!                     │   ▲   ▲                       │                 （不寫、不設、不還原；
//!   第一次設定前：讀 EnabledState                       │                   already_taken_over＝真）
//!                     │   │   └─ 還原成功（不接管／系統匣結束／安全閥／--restore-wallpaper；
//!                     │   │        當下離線的螢幕標為待還原 pending_restore）
//!            ＝1 且未確認 │                             │
//!                     ▼   │ 取消（主題改回 none）       │
//!          等使用者確認 ────┘                           │
//!       AwaitingSpotlightConfirmation                  │
//!                     │ 確認                            │
//!                     ▼                                │
//!   記錄原桌布＋備份＋登錄快照 → 原子寫入 → 接管中 TakenOver ──┤
//!                                                   │  ▲   │
//!                     每次設定前讀回 → 宿主的圖／尚未套用 ─┘  │   │ 讓位（使用者自換）：主題改 none、
//!                     讀回失敗 → 本次跳過（狀態不變）────────┘   │ 新桌布記為原桌布；其他仍是宿主圖的螢幕
//!                                                                    │ 逐螢幕還原（失敗或離線→待還原）；不寫登錄
//!                     驗證失敗的還原 → 狀態不變（下次重試）       └──► 未接管＋停止 Stopped
//!                     工作階段結束（登出／關機）→ 不還原、狀態不變        （選了某個主題前不再接管；
//!                                                                          選「不接管」不解除）
//! ```
//!
//! ## 還原（所有還原路徑共用：不接管、系統匣結束、安全閥、`--restore-wallpaper`、待還原）
//!
//! 1. 先讀回；讀不到就什麼都不做（不設定、不寫登錄），狀態保留。
//! 2. 逐台在線螢幕分類（[`classify_for_restore`]）：
//!    - 仍是宿主的圖 → 還原；
//!    - 已是原桌布 → 不動；
//!    - 讀回是 explorer 的桌布快取工作檔（[`StatePaths::is_themes_cache`]；task 6.4）→ 內容可能已是
//!      宿主的圖，照原紀錄還原（不算使用者自選）；
//!    - **續做**還原（狀態檔原本就有「還原進行中」標記）、原桌布是投影片且目前正在播放，讀回落在
//!      投影片來源資料夾內 → 上一次還原已恢復投影片，算原桌布（task 6.4，審查 R1-M1；否則每台都被
//!      判成使用者自選，全域填滿方式、背景色、登錄永遠不還原）；
//!    - 其他 → 使用者自己選的：不動，記為新的原桌布（舊備份記進 `superseded_backups`）。
//!
//!    另外列舉**沒有紀錄**、卻正顯示宿主輸出圖的在線螢幕（接管期間接上、宿主還沒設定過它；task 6.4，
//!    審查 R1-M2）：補一筆紀錄，還原成「全域原桌布」——狀態檔記錄的登錄 `Wallpaper` 原值，不可用時
//!    用任一已記錄螢幕的原桌布，都沒有就純色——並在結果的 `warnings` 記一行（讓位時同樣處理）。
//! 3. 只有在沒有任何「使用者自己選的」螢幕時，才還原全域設定（背景色、投影片、填滿方式）與三個
//!    登錄值（4.2 契約：使用者已自行選擇時不得寫回登錄）。
//!
//!    **原桌布是全域純色**（[`OriginalDesktop::solid_color`]；task 6.1 修正 Bug 1／B9）：純色是全域狀態，
//!    逐螢幕 `SetWallpaper(<螢幕>, "")` 只會換回 explorer 記憶中的圖片。沒有使用者自選的螢幕時，所有仍是
//!    宿主圖的螢幕一律以一次 `SetWallpaper(NULL, "")`（`WallpaperService::set_solid_color`）還原，再寫回
//!    背景色、填滿方式與登錄，讀回驗證要求每台都是空字串。有使用者自選的螢幕時不能用全域純色（會蓋掉使用者
//!    的選擇），其餘螢幕逐螢幕設空字串，讀回已不是宿主的圖（explorer 記憶中的圖片）就算還原（讓位與待還原
//!    同樣如此，[`restored_matches`]）。原桌布為純色的待還原螢幕重新接上時（修正輪 1，審查 L4）：其餘在線
//!    螢幕仍是純色才再呼叫一次全域純色，否則不動它、清掉待還原（[`WallpaperTakeover::restore_pending_monitors`]；
//!    修正輪 2：另列在 [`PendingReport::left_showing_host`]，記錄寫明仍顯示宿主的圖、需使用者自行更換）。
//!    **原桌布是「全部螢幕」設定**（[`OriginalDesktop::all_monitors`]；installer-auto-update 4.x，實機 4.4）：接管開始時
//!    登錄 `Wallpaper`＝各在線螢幕讀回的共同路徑（[`setting_method`] 為 `AllMonitors`；逐螢幕設定過的桌布，登錄是
//!    Themes 的 `TranscodedWallpaper`），而且不是純色、投影片、Windows 焦點。此時 `GetWallpaper(NULL)` 有值，宿主逐螢幕
//!    設定之後它變空字串，逐螢幕還原也不會讓它回來（畫面相同、不是原狀態）。所以沒有使用者自選的螢幕、且每台在線螢幕
//!    都是「要還原」或「已是這張圖」、要還原成的路徑彼此相同時（[`restore_all_monitors_path`]），以一次
//!    `SetWallpaper(NULL, 原圖)`（`WallpaperService::set_wallpaper_all`）還原；條件不全成立就維持逐螢幕還原（讓位與待還原
//!    也一律逐螢幕）。備份沒有記錄 `GetWallpaper(NULL)` 的讀回，這個旗標是以接管前登錄快照推斷的——與備份轉存檔選擇
//!    （[`backup_transcoded`]）用同一個判準（memory `setwallpaper-side-effects`：全部螢幕設定把登錄寫成原圖路徑、逐螢幕設定
//!    把它改成 `TranscodedWallpaper`）。
//! 4. 寫回登錄前，先等 explorer 處理完非同步的 `SetWallpaper`（輪詢登錄 `Wallpaper` 直到改變，
//!    上限 [`TakeoverConfig::registry_settle_timeout`]）——explorer 處理時會把它改成
//!    `TranscodedWallpaper`。
//! 5. 逐項讀回驗證，失敗時隔 [`TakeoverConfig::verify_interval`] 再驗一次；仍失敗就保留狀態檔。
//!
//! 還原來源（[`restore_target`]）：原路徑存在就用原路徑、不存在就用備份；原路徑在桌布快取資料夾內、
//! 或它的檔案識別與記錄時不同（task 6.4，審查 R1-M3b）而有備份時，優先用備份。
//!
//! 原桌布未知的螢幕（記錄當下就是宿主的圖）依 [`unknown_original_fallback`] 還原；選中的是備份資料夾
//! 內的檔（別台的備份）時，先複製成這台自己的備份再以它還原，並採納為這台的原桌布（修正輪 4：每個
//! 備份檔只屬於一筆紀錄，螢幕絕不顯示別台的備份）。逐螢幕還原成功的圖也列入「目前的桌布」保護，
//! 接下來的清理不會刪它。螢幕目前顯示的就是這筆紀錄自己的備份檔（沒有被其他紀錄引用；原桌布已知
//! 的紀錄只認它的 `backup`，未知的紀錄認以本鍵命名的檔、含序號檔名——例如上一次還原中斷前寫下的
//! 複本）時，視為已是原桌布並重用該檔，不另寫（修正輪 R）。在線但這次讀不到
//! 桌布的螢幕視同「使用者自己選的」來擋登錄與全域設定，本身不設定、標為待還原。沿用自前一次接管
//! 的紀錄（`stale`）只在顯示宿主的圖時還原，其餘不動、也不擋登錄。
//!
//! ## 紀錄的保存
//!
//! 記錄原桌布時一定要備份到原圖（純色、原桌布未知者除外；task 6.4，審查 R1-M3a）：原圖複製不了（接管
//! 前就被刪除或搬走——explorer 仍以快取顯示它——或權限、磁碟問題）時，接管開始改備份 explorer 的轉存檔
//! （有任何在線螢幕正顯示宿主的圖時不可信、不用），依**設定方式**選來源（task 6.1 修正 Bug 2／B3、修正輪 3／4），
//! 設定方式以第一次 `SetWallpaper` **之前**的登錄 `Wallpaper` 快照判定（[`setting_method`]）：登錄值＝各在線螢幕讀回的
//! 共同路徑（＝「全部螢幕」設定）→ 全域 [`StatePaths::transcoded_wallpaper`]（多螢幕也可以；殘留的逐螢幕檔是舊圖、
//! 不用）；登錄值是這個 Themes 的 `TranscodedWallpaper`（＝逐螢幕設定）→ 這台的 [`StatePaths::monitor_transcoded`]
//! `Transcoded_<索引>`（索引＝第一次設定前列舉的 `GetMonitorDevicePathAt` 索引），不存在就不能備份；判斷不出就不能
//! 備份。選中的檔要存在、大小 > 0、依檔頭認得出是圖片；**不**核對格式或尺寸（轉存檔沿用原圖的格式與尺寸）。仍備份不了、或是
//! 接管中新接上的螢幕（轉存檔此時已是宿主的圖）——**不接管**：什麼都不設定、不寫狀態檔，回
//! [`SetOutcome::BackupFailed`]（主題改「不接管」＋通知；已接管時呼叫端另外還原）。
//!
//! 接管前所有在線螢幕都讀回空字串、而 Windows 焦點沒有啟用＝原桌布是**全域純色**，記在
//! [`OriginalDesktop::solid_color`]（色碼是 `background_color`；task 6.1 修正 Bug 1、修正輪 1 審查 L5）。只看 `GetWallpaper`：`GetStatus` 在純色時回 0 是本機實測，只記錄。接管中新螢幕的讀回本身
//! 就是轉存檔或快取工作檔時（內容已是宿主的圖；修正第 2 輪複審 N2），比照宿主輸出圖記為原桌布未知、不備份。
//! 備份的副檔名：來源有圖片副檔名就沿用，否則依內容判斷（認 PNG／JPEG／BMP／GIF／WebP 簽章，判斷不出來用 `jpg`）。
//!
//! 接管開始時，舊狀態中離線螢幕的紀錄一律保留（標為 `stale`）：它的備份可能正是那台螢幕目前的
//! 桌布來源（原圖刪除後從備份還原）。它重新接上時，讀回是宿主的圖就照常判定；不是就不做讓位判定，
//! 等要設定它時以當時的桌布重新記錄。配鍵時先讓沿用舊紀錄的螢幕取回舊鍵，新紀錄再避開所有
//! 已用的鍵與舊狀態中屬於其他螢幕的鍵——備份檔名就是鍵，絕不覆寫另一台螢幕的備份。
//!
//! 未接管時可以有「待還原」的螢幕（`pending_restore`）：還原或讓位當下離線、或逐螢幕還原的讀回
//! 驗證失敗者。螢幕接上時 [`WallpaperTakeover::restore_pending_monitors`]：讀回仍是宿主的圖就
//! 逐螢幕還原並清除標記，是其他圖（使用者自己換了）就只清除標記；`restore`（含
//! `--restore-wallpaper`）在未接管時也處理它們。再次接管時沿用待還原螢幕的紀錄，不把它目前的
//! 宿主圖記成原桌布。逐螢幕還原不動全域設定、不寫登錄。
//!
//! 持久化的只有 `status`（未接管／接管中）與各螢幕的待還原標記；「等使用者確認」「停止」「還原排隊中」是這次執行的
//! 記憶體狀態（[`Phase`]）。等待確認時宿主重啟，設定中的主題仍在，下次第一次設定前會再問一次。
//!
//! ## 給排程器（4.3）的 `already_taken_over`
//!
//! [`WallpaperTakeover::already_taken_over`]＝狀態檔 `status == taken_over`（封鎖時保守回真）。
//! 第一次寫入接管記錄（緊接著就是第一個 `SetWallpaper`）時變真；還原成功或讓位後變假。
//! 排程器的 `HoldWithoutData`（接管中資料消失）**不是**還原條件，本模組不看排程決策。
//!
//! ## 呼叫端契約（4.7）
//!
//! 1. 啟動時 [`WallpaperTakeover::load`]（主執行緒以外；讀寫的檔案很小）。[`WallpaperTakeover::blocked`]
//!    為 `Some` 時記錯誤、不排程渲染（任何設定都會被拒絕），`--restore-wallpaper` 以非零碼結束。
//!    [`WallpaperTakeover::unavailable`] 為 `Some`（暫時讀不到）時照常排程，下一次評估自動重讀。
//! 2. 每台螢幕畫好之後呼叫 [`WallpaperTakeover::set_monitor_wallpaper`]，**不得**直接呼叫
//!    `WallpaperService::set_wallpaper`：它依序做「（首次）焦點檢查與記錄」「讀回與讓位判定」
//!    「新螢幕記錄」「原子寫入」，最後才設定。依 [`SetOutcome`] 對排程器記錄結果：
//!    `Applied`→`record_drawn`；`SetFailed`／`ReadbackFailed`→依錯誤種類：逾時＝`FailureKind::Apply`、
//!    `Busy` 錯誤＝`FailureKind::Busy`、explorer 回錯＝`FailureKind::ApplyError`（退避短間隔重試）；`Refused`→記錯誤（`StateWriteFailed` 等同 `Apply`）；`Yielded`→套用
//!    事件（主題改 none、系統匣通知），排程器已在本模組內 `reset`；`BackupFailed`（task 6.4）→套用
//!    事件，**已接管時再以 [`RestoreReason::NotTakeover`] 呼叫 `restore`**（套用 `ThemeSetToNone` 不會
//!    觸發還原）；`AwaitingSpotlightConfirmation`→開確認提示（4.8），確認呼叫
//!    [`WallpaperTakeover::confirm_spotlight`]、取消呼叫 [`WallpaperTakeover::cancel_spotlight`] 並套用
//!    回傳的事件。讀回落在桌布快取資料夾的螢幕不讓位、不改紀錄，只記一行（[`Readback::Uncertain`]）；
//!    原桌布是全域純色時，還沒設定過的螢幕被 explorer 換回記憶中的圖片也一樣（[`Readback::SolidTransition`]）。
//!
//!    **`Applied` 之後的義務（修正輪 2 裁決 2）**：等約 1 秒再讀回該螢幕，以讀回值呼叫
//!    [`WallpaperTakeover::confirm_applied`]。這會立即把 `host_applied` 設為真；不呼叫的話，
//!    「尚未套用」的窗口會長達一整個重畫間隔，期間使用者換回原圖會被覆蓋。
//! 3. 執行 `Redraw` 前後呼叫 [`WallpaperTakeover::redraw_started`]／
//!    [`WallpaperTakeover::redraw_finished`]。`Redraw` 進行中要求還原時 [`WallpaperTakeover::restore`]
//!    回 [`RestoreResult::Queued`]（之後該次 `Redraw` 尚未送出的螢幕一律 `Refused(RestorePending)`），
//!    `redraw_finished` 回傳排隊的原因，呼叫端立即以它再呼叫一次 `restore`。
//! 4. 選「不接管」、系統匣「結束」、安全閥（4.9）、`--restore-wallpaper`：呼叫
//!    [`WallpaperTakeover::restore`]，排程器的 `reset` 在裡面呼叫。工作階段結束（登出、關機）
//!    **不**呼叫：保留最後一張圖、狀態維持接管中，下次登入照常接管（協調迴圈的
//!    `Wake::SessionEnding` 只停止渲染）。當機同樣不經過還原。
//!
//!    **`StateUnavailable` 的重試（修正輪 3 裁決 6；4.7 的義務）**：`restore` 回
//!    [`RestoreResult::StateUnavailable`]（狀態檔暫時讀不到）時，宿主的圖還留在桌面上，而主題已是
//!    「不接管」、排程不會再評估——4.7 必須自行排定重試：第 n 次失敗後隔
//!    [`crate::wallpaper::retry_delay`]`(n)` 秒以同一個原因再呼叫 `restore`，直到結果不是
//!    `StateUnavailable`（成功、`NothingToRestore`、`Failed` 照各自規則處理；`Blocked` 就停止重試、
//!    記錯誤）。系統匣結束與 `--restore-wallpaper` 已在本模組內重讀一次，之後宿主就要結束，不重試。
//! 5. 使用者在設定中改主題時呼叫 [`WallpaperTakeover::theme_selected`]：選了某個主題才解除讓位／
//!    安全閥後的停止；選「不接管」（含套用 `ThemeSetToNone` 事件）不解除。
//! 6. 螢幕接上、顯示組態變更時（未接管）呼叫 [`WallpaperTakeover::restore_pending_monitors`]。
//! 7. explorer 資源安全閥（4.9）：判定前以 [`WallpaperTakeover::valve_baseline`] 取基準；排程器帶出
//!    新基準時呼叫 [`WallpaperTakeover::set_explorer_baseline`]；主題從「不接管」改回某個主題時呼叫
//!    [`WallpaperTakeover::clear_explorer_baseline`]。觸發時以 [`RestoreReason::SafetyValve`] 呼叫
//!    [`WallpaperTakeover::restore`]（與第 4 點同一條還原路徑）。

mod monitor_key;

pub use monitor_key::{monitor_key, StableDisplay};

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::desktop::wallpaper::file_identity::{same_local_drive, FileId, FileIdentity};
use crate::desktop::wallpaper::pathcmp::{path_eq, path_is_within};
use crate::desktop::wallpaper::registry::{
    self, DesktopRegistrySnapshot, RegValue, RegistryStore, SpotlightState, DESKTOP_KEY,
    DESKTOP_VALUE_NAMES,
};
use crate::desktop::wallpaper::{
    slideshow_applicable, MonitorWallpaper, SlideshowInfo, WallpaperError, WallpaperPosition,
    WallpaperService, WallpaperSnapshot,
};
use crate::settings::{self, WallpaperTheme};
use crate::wallpaper::{ExplorerSample, SchedulerState};

/// 狀態檔檔名（宿主設定資料夾）。
pub const STATE_FILE_NAME: &str = "wallpaper-state.json";
/// 狀態檔格式版本。讀到其他版本一律封鎖（[`BlockReason::UnknownVersion`]）。
pub const STATE_VERSION: u32 = 1;
/// 宿主輸出資料夾名稱（`%LOCALAPPDATA%\tw.fintools.fc-host\` 底下；design.md D4）。
pub const OUTPUT_DIR_NAME: &str = "wallpaper";
/// 原圖備份子資料夾名稱（輸出資料夾底下）。**不算**宿主輸出：讓位判定時，讀回落在這裡＝不是
/// 宿主設的圖（宿主只在還原時才設它）。
pub const BACKUP_DIR_NAME: &str = "original";

// ---------------------------------------------------------------------------------------------
// 檔案位置
// ---------------------------------------------------------------------------------------------

/// 狀態檔與輸出資料夾的位置（測試注入暫存資料夾）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatePaths {
    /// `wallpaper-state.json` 的完整路徑。
    pub state_file: PathBuf,
    /// 宿主輸出資料夾（`<螢幕鍵>-a.png`／`-b.png`；4.5 寫入）。
    pub output_dir: PathBuf,
    /// Windows 的桌布快取資料夾 `%APPDATA%\Microsoft\Windows\Themes`（task 6.4）：explorer 在
    /// 每次 `SetWallpaper` 後改寫其中的 [`TRANSCODED_WALLPAPER_NAME`]，所以它的工作檔**內容會變**——
    /// 讀回是工作檔時不判讓位、還原時不信任它的內容（[`Self::is_themes_cache`] 只認工作檔，不是整個
    /// 資料夾）；原圖複製失敗時改備份這裡的轉存檔（接管開始時，內容仍是使用者的桌布）。
    pub themes_dir: PathBuf,
}

impl StatePaths {
    /// 原圖備份資料夾 `<輸出資料夾>\original`。
    pub fn backup_dir(&self) -> PathBuf {
        self.output_dir.join(BACKUP_DIR_NAME)
    }

    /// `path` 是否為宿主的輸出圖（在輸出資料夾內、但不在備份資料夾內）。
    pub fn is_host_output(&self, path: &str) -> bool {
        path_is_within(path, &self.output_dir) && !path_is_within(path, &self.backup_dir())
    }

    /// 系統快取的轉存檔 `<Themes>\TranscodedWallpaper`。只有「全部螢幕」設定（`SetWallpaper(NULL, 圖)`，不產生
    /// 逐螢幕轉存檔）時才代表所有螢幕；逐螢幕設定過之後它不代表任何一台（task 6.1 B3／B4）。
    pub fn transcoded_wallpaper(&self) -> PathBuf {
        self.themes_dir.join(TRANSCODED_WALLPAPER_NAME)
    }

    /// 某台螢幕自己的轉存檔 `<Themes>\Transcoded_<索引三位數>`（task 6.1 B4 實機：編號＝
    /// `GetMonitorDevicePathAt` 的索引，格式與尺寸沿用原圖；逐螢幕 `SetWallpaper` 只更新對應那個，全部螢幕設定
    /// 不產生它，全域純色會刪掉它）。
    pub fn monitor_transcoded(&self, index: usize) -> PathBuf {
        self.themes_dir
            .join(format!("{TRANSCODED_PREFIX}{index:03}"))
    }

    /// `path` 是否為 explorer 的桌布快取工作檔（task 6.4，審查 R1-M3b／M4；修正第 2 輪複審 N1 收窄）。
    /// 只認：
    ///
    /// - 任何位置、檔名就是 `TranscodedWallpaper` 的檔；
    /// - [`Self::themes_dir`] 這一層裡 `Transcoded_` 開頭的逐螢幕轉存檔；
    /// - [`Self::themes_dir`] 的 `CachedFiles\` 子資料夾之內的檔；
    /// - [`Self::themes_dir`] 的 `TranscodedWallpaperCache\` 子資料夾之內的檔（task 6.1 B6：實機 explorer 用的是
    ///   這個，`TranscodedWallpaper_<雜湊>`＋同名 `.meta`，LRU 約 4 筆；`CachedFiles\` 沒出現過，保留不刪）。
    ///
    /// 6.1 B2 實機：`GetWallpaper` 讀回從未落在這些檔、以 `\\?\` 設定的讀回會去掉前綴、8.3 短檔名原樣回傳——
    /// 短檔名只在別的程式以短檔名設定時出現（那本來就是使用者自換），所以路徑比對不另外展開短檔名。
    ///
    /// 這些檔由 explorer 改寫，內容可能已是宿主的圖：讀回落在這裡**不**判為使用者自換
    /// （[`Readback::Uncertain`]），還原時有備份就優先用備份（[`restore_target`]）。使用者套用的佈景主題
    /// （Store／`.themepack` 解壓到 `%LOCALAPPDATA%\Microsoft\Windows\Themes\<名稱>\DesktopBackground\`）與
    /// `themes_dir` 裡其他的檔**不算**——那是使用者的選擇，照常判讓位。路徑比對沿用 `pathcmp`（Windows
    /// 序數、不分大小寫）。
    pub fn is_themes_cache(&self, path: &str) -> bool {
        let p = Path::new(path);
        let Some(name) = p.file_name().map(|n| n.to_string_lossy()) else {
            return false;
        };
        if name.eq_ignore_ascii_case(TRANSCODED_WALLPAPER_NAME) {
            return true;
        }
        let in_themes_dir = p
            .parent()
            .is_some_and(|d| path_eq(&d.to_string_lossy(), &self.themes_dir.to_string_lossy()));
        if in_themes_dir
            && name
                .get(..TRANSCODED_PREFIX.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(TRANSCODED_PREFIX))
        {
            return true;
        }
        path_is_within(path, &self.themes_dir.join(CACHED_FILES_DIR))
            || path_is_within(path, &self.themes_dir.join(TRANSCODED_CACHE_DIR))
    }
}

/// explorer 的桌布轉存檔檔名（`%APPDATA%\Microsoft\Windows\Themes\` 底下）。
pub const TRANSCODED_WALLPAPER_NAME: &str = "TranscodedWallpaper";
/// 逐螢幕轉存檔的檔名開頭（`Transcoded_000` 等，與 `TranscodedWallpaper` 同一層；6.1 B4 實機：編號＝
/// `GetMonitorDevicePathAt` 索引，見 [`StatePaths::monitor_transcoded`]）。
const TRANSCODED_PREFIX: &str = "Transcoded_";
/// explorer 依尺寸與填滿方式快取的桌布子資料夾（`Themes\CachedFiles\`；6.1 實機沒出現過，保留）。
const CACHED_FILES_DIR: &str = "CachedFiles";
/// explorer 的桌布轉存快取子資料夾（`Themes\TranscodedWallpaperCache\`；6.1 B6 實機）。
const TRANSCODED_CACHE_DIR: &str = "TranscodedWallpaperCache";

/// 正式位置：狀態檔與 `settings.json` 同一個資料夾（`%APPDATA%\tw.fintools.fc-host\`）；輸出
/// 資料夾 `%LOCALAPPDATA%\tw.fintools.fc-host\wallpaper`；桌布快取資料夾
/// `%APPDATA%\Microsoft\Windows\Themes`（環境變數不存在時退回目前工作目錄，比照 `crate::settings`）。
pub fn default_paths() -> StatePaths {
    let local = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let roaming = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    StatePaths {
        state_file: settings::default_settings_path().with_file_name(STATE_FILE_NAME),
        output_dir: local.join("tw.fintools.fc-host").join(OUTPUT_DIR_NAME),
        themes_dir: roaming.join("Microsoft").join("Windows").join("Themes"),
    }
}

// ---------------------------------------------------------------------------------------------
// 狀態檔格式
// ---------------------------------------------------------------------------------------------

/// 是否接管中（持久化的唯一狀態）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TakeoverStatus {
    NotTakenOver,
    TakenOver,
}

/// `wallpaper-state.json` 的內容（格式見模組文件）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateFile {
    pub version: u32,
    pub status: TakeoverStatus,
    /// 接管開始時的全域狀態；有值若且唯若 `status == TakenOver`。
    pub original: Option<OriginalDesktop>,
    /// 逐螢幕紀錄，鍵＝螢幕鍵。離線螢幕的紀錄保留。
    pub monitors: BTreeMap<String, MonitorRecord>,
    /// 最近一次還原或讓位（只供診斷）。
    pub last_event: Option<LastEvent>,
    /// 還原進行中（task 4.7b）：接管中開始還原前寫入、還原成功時與「未接管」同一次寫入清除。
    /// 下次啟動看到它＝上次的還原沒做完（逾時、當機、工作階段結束），要先續做。舊檔沒有此欄位＝
    /// 沒有標記；沒有標記時不寫出這個欄位（與 4.4 的檔逐鍵相同）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore_in_progress: Option<RestoreMarker>,
    /// explorer 資源安全閥的基準（task 4.9；design.md D11）。舊檔沒有此欄位＝沒有基準（下一次
    /// 讀到時設基準）；沒有基準時不寫出這個欄位。回到未接管時清除。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explorer_baseline: Option<ExplorerBaseline>,
    /// 被取代的原圖備份檔名（task 6.4，審查 R1-M4）：讓位或還原時發現使用者自換，紀錄改採納新桌布、
    /// 放掉舊備份；舊備份**保留到下一次接管開始**才清（判定萬一是誤判，原圖仍在）。舊檔沒有此欄位
    /// ＝空；空的時候不寫出。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub superseded_backups: Vec<String>,
}

/// explorer 資源安全閥的基準（task 4.9；[`StateFile::explorer_baseline`]）：某一次讀到的 explorer
/// PID 與 GDI 物件數。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExplorerBaseline {
    pub pid: u32,
    pub gdi: u32,
    /// 取得這個基準的時刻（Unix 秒；只供診斷）。
    pub recorded_at: i64,
}

impl ExplorerBaseline {
    pub fn sample(&self) -> ExplorerSample {
        ExplorerSample {
            pid: self.pid,
            gdi: self.gdi,
        }
    }
}

/// 「還原進行中」標記的原因（[`RestoreReason`] 去掉安全閥的說明）。讀到不認得的值＝讀不懂，
/// 與其他欄位一樣封鎖整個狀態檔。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkerReason {
    NotTakeover,
    TrayExit,
    CommandLine,
    SafetyValve,
}

/// 還原進行中標記（task 4.7b；[`StateFile::restore_in_progress`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreMarker {
    pub reason: MarkerReason,
    /// 安全閥的說明（其他原因沒有）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// 這次還原開始的時刻（Unix 秒）。續做時不更新。
    pub started_at: i64,
}

impl RestoreMarker {
    pub fn new(reason: &RestoreReason, started_at: i64) -> Self {
        let (reason, detail) = match reason {
            RestoreReason::NotTakeover => (MarkerReason::NotTakeover, None),
            RestoreReason::TrayExit => (MarkerReason::TrayExit, None),
            RestoreReason::CommandLine => (MarkerReason::CommandLine, None),
            RestoreReason::SafetyValve { detail } => {
                (MarkerReason::SafetyValve, Some(detail.clone()))
            }
        };
        Self {
            reason,
            detail,
            started_at,
        }
    }

    /// 續做時用的還原原因。
    pub fn restore_reason(&self) -> RestoreReason {
        match self.reason {
            MarkerReason::NotTakeover => RestoreReason::NotTakeover,
            MarkerReason::TrayExit => RestoreReason::TrayExit,
            MarkerReason::CommandLine => RestoreReason::CommandLine,
            MarkerReason::SafetyValve => RestoreReason::SafetyValve {
                detail: self.detail.clone().unwrap_or_default(),
            },
        }
    }

    /// 與 `reason` 是否同一個原因（含說明）。
    fn same_reason(&self, reason: &RestoreReason) -> bool {
        self.restore_reason() == *reason
    }
}

/// 外部要求的「還原進行中」標記（task 4.7b 修正輪 1）：系統匣結束、命令列交接在**等協調迴圈之前**
/// 就把標記寫進狀態檔——進行中的渲染可能拖過結束的等待上限，宿主沒還原就結束時，下次啟動才會續做
/// 還原而不是照常接管。
///
/// 狀態檔平常只有協調執行緒在寫；這裡是唯一的第二個寫入者，兩者以同一把鎖序列化：
/// [`RestoreRequestMarker::request`] 持鎖讀磁碟上的檔、加上標記、只寫狀態檔（不清孤兒備份）；
/// [`WallpaperTakeover`] 的每次寫檔也持同一把鎖，要求還在且狀態為接管中時把標記一起寫進去——記憶體中
/// 的狀態不知道外部寫了標記，不這樣做會在下一次寫檔時把它蓋掉。協調迴圈處理完要求後 [`Self::clear`]。
#[derive(Debug, Default)]
pub struct RestoreRequestMarker {
    inner: Mutex<RequestInner>,
}

#[derive(Debug, Default)]
pub struct RequestInner {
    requested: Option<RestoreMarker>,
    paths: Option<StatePaths>,
}

/// [`RestoreRequestMarker::request`] 的結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkOutcome {
    /// 已寫入狀態檔。
    Written,
    /// 不需要寫：未接管、已有標記、狀態檔不存在或讀不懂（不動它）。
    NotNeeded,
    /// 還沒接上狀態檔位置（協調迴圈還沒建立或啟動失敗）。
    NotAttached,
    /// 等共用鎖超過上限（協調迴圈卡在寫檔）：沒寫、也沒記下要求（審查 F6）。
    LockTimeout,
    /// 讀寫失敗。
    Failed(String),
}

impl RestoreRequestMarker {
    fn lock(&self) -> MutexGuard<'_, RequestInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 協調迴圈建立時設定狀態檔位置。
    pub fn set_paths(&self, paths: StatePaths) {
        self.lock().paths = Some(paths);
    }

    /// 取鎖，最多等 `wait`（審查 F6：系統匣結束的執行緒不得無上限地等協調迴圈的寫檔）。
    fn lock_within(&self, wait: Duration) -> Option<MutexGuard<'_, RequestInner>> {
        let deadline = Instant::now() + wait;
        loop {
            match self.inner.try_lock() {
                Ok(g) => return Some(g),
                Err(std::sync::TryLockError::Poisoned(e)) => return Some(e.into_inner()),
                Err(std::sync::TryLockError::WouldBlock) => {}
            }
            if Instant::now() >= deadline {
                return None;
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// 測試用：持住共用鎖（模擬協調迴圈卡在寫檔）。
    #[cfg(test)]
    pub fn hold_lock_for_test(&self) -> MutexGuard<'_, RequestInner> {
        self.lock()
    }

    /// 記下要求並立即把標記寫進狀態檔（任何執行緒可呼叫；只做一次小檔讀寫）。取共用鎖最多等
    /// `wait`，等不到回 [`MarkOutcome::LockTimeout`]。
    pub fn request(&self, reason: &RestoreReason, now: i64, wait: Duration) -> MarkOutcome {
        let Some(mut inner) = self.lock_within(wait) else {
            return MarkOutcome::LockTimeout;
        };
        let marker = RestoreMarker::new(reason, now);
        inner.requested = Some(marker.clone());
        let Some(paths) = inner.paths.clone() else {
            return MarkOutcome::NotAttached;
        };
        let bytes = match fs::read(&paths.state_file) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return MarkOutcome::NotNeeded,
            Err(e) => return MarkOutcome::Failed(e.to_string()),
        };
        let Ok(mut state) = parse_state(&bytes) else {
            return MarkOutcome::NotNeeded;
        };
        if state.status != TakeoverStatus::TakenOver || state.restore_in_progress.is_some() {
            return MarkOutcome::NotNeeded;
        }
        state.restore_in_progress = Some(marker);
        match write_state_json(&paths.state_file, &state) {
            Ok(()) => MarkOutcome::Written,
            Err(e) => MarkOutcome::Failed(e.to_string()),
        }
    }

    /// 要求處理完（協調迴圈已以自己的寫入接手標記）。
    pub fn clear(&self) {
        self.lock().requested = None;
    }
}

impl StateFile {
    /// 從未接管（新安裝、或狀態檔不存在）。
    pub fn empty() -> Self {
        Self {
            version: STATE_VERSION,
            status: TakeoverStatus::NotTakenOver,
            original: None,
            monitors: BTreeMap::new(),
            last_event: None,
            restore_in_progress: None,
            explorer_baseline: None,
            superseded_backups: Vec::new(),
        }
    }

    /// 跨欄位的一致性（讀檔時檢查；不一致＝損壞）。
    fn validate(&self) -> Result<(), String> {
        match (self.status, &self.original) {
            (TakeoverStatus::TakenOver, None) => {
                return Err("status 為 taken_over 卻沒有 original".to_owned())
            }
            (TakeoverStatus::NotTakenOver, Some(_)) => {
                return Err("status 為 not_taken_over 卻有 original".to_owned())
            }
            _ => {}
        }
        if let Some(o) = &self.original {
            if o.slideshow.is_some() != slideshow_applicable(o.slideshow_status) {
                return Err("original.slideshow 與 slideshow_status 不一致".to_owned());
            }
        }
        Ok(())
    }
}

/// 接管開始時記錄的全域原狀態。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OriginalDesktop {
    pub recorded_at: i64,
    /// `GetPosition` 原值（還原時寫回）。
    pub position: i32,
    /// `GetBackgroundColor` 原值（純色背景的色碼）。
    pub background_color: u32,
    /// `GetStatus` 原始旗標。
    pub slideshow_status: i32,
    /// 投影片設定；有值若且唯若 `slideshow_status` 含 `SLIDESHOW_STATE_SLIDESHOW`。
    pub slideshow: Option<PersistedSlideshow>,
    /// `HKCU\Control Panel\Desktop` 三個值（第一次 `SetWallpaper` 之前取的唯一一次快照）。
    pub registry: PersistedRegistry,
    /// Windows 焦點相關登錄值（只供診斷）。
    pub spotlight: PersistedSpotlight,
    /// 原桌布是**全域純色**（task 6.1 修正，Bug 1／B9）：接管開始時所有在線螢幕的 `GetWallpaper` 都讀回
    /// 空字串（Windows 焦點啟用中時不算，修正輪 1 審查 L5）。純色是全域狀態（逐螢幕設空字串不會變純色），所以還原時以 `SetWallpaper(NULL, "")` 一次
    /// 還原所有螢幕，色碼是 [`Self::background_color`]；接管途中 explorer 把尚未設定的螢幕換回它記憶中的
    /// 圖片，也不算使用者自換（[`Readback::SolidTransition`]）。舊檔沒有此欄位＝否；否的時候不寫出。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub solid_color: bool,
    /// 原桌布是以「**全部螢幕**」設定的（installer-auto-update 4.x，實機 4.4）：接管開始時登錄 `Wallpaper` 等於各在線
    /// 螢幕讀回的共同路徑（[`setting_method`] 為 [`SettingMethod::AllMonitors`]；逐螢幕設定過的桌布，登錄會是 Themes
    /// 的 `TranscodedWallpaper`）。此時 `GetWallpaper(NULL)` 有值；宿主接管期間逐螢幕 `SetWallpaper` 之後它變空字串，
    /// 逐螢幕還原也不會讓它回來——畫面相同、但不是原狀態。所以還原時（[`restore_all_monitors_path`] 的條件都成立）改以
    /// 一次 `SetWallpaper(NULL, 原圖)` 還原。舊檔沒有此欄位＝否（維持逐螢幕還原）；否的時候不寫出。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub all_monitors: bool,
}

/// 投影片設定（`SlideshowInfo` 的持久化形式）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedSlideshow {
    pub items: Vec<String>,
    pub options: i32,
    pub tick_ms: u32,
}

impl PersistedSlideshow {
    fn from_info(info: &SlideshowInfo) -> Self {
        Self {
            items: info.items.clone(),
            options: info.options,
            tick_ms: info.tick_ms,
        }
    }

    fn to_info(&self) -> SlideshowInfo {
        SlideshowInfo {
            items: self.items.clone(),
            options: self.options,
            tick_ms: self.tick_ms,
        }
    }
}

/// 三個登錄值的忠實快照（型別＋原始位元組，或「原本不存在」）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedRegistry {
    #[serde(with = "reg_value_serde")]
    pub wallpaper: RegValue,
    #[serde(with = "reg_value_serde")]
    pub wallpaper_style: RegValue,
    #[serde(with = "reg_value_serde")]
    pub tile_wallpaper: RegValue,
}

impl PersistedRegistry {
    fn from_snapshot(s: DesktopRegistrySnapshot) -> Self {
        Self {
            wallpaper: s.wallpaper,
            wallpaper_style: s.wallpaper_style,
            tile_wallpaper: s.tile_wallpaper,
        }
    }

    fn to_snapshot(&self) -> DesktopRegistrySnapshot {
        DesktopRegistrySnapshot {
            wallpaper: self.wallpaper.clone(),
            wallpaper_style: self.wallpaper_style.clone(),
            tile_wallpaper: self.tile_wallpaper.clone(),
        }
    }
}

/// 登錄值的序列化：`{"state":"missing"}` 或
/// `{"state":"present","kind":<型別>,"data_hex":"<小寫十六進位>"}`。沒有預設值：欄位缺漏、
/// 標籤不認得、十六進位無效一律是錯誤（整份狀態檔視為損壞），絕不默默變成 `Missing`。
mod reg_value_serde {
    use super::RegValue;
    use serde::de::Error as _;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    #[serde(tag = "state", rename_all = "snake_case")]
    enum Repr {
        Missing,
        Present { kind: u32, data_hex: String },
    }

    pub fn serialize<S: Serializer>(v: &RegValue, s: S) -> Result<S::Ok, S::Error> {
        match v {
            RegValue::Missing => Repr::Missing,
            RegValue::Present { kind, data } => Repr::Present {
                kind: *kind,
                data_hex: data.iter().map(|b| format!("{b:02x}")).collect(),
            },
        }
        .serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<RegValue, D::Error> {
        match Repr::deserialize(d)? {
            Repr::Missing => Ok(RegValue::Missing),
            Repr::Present { kind, data_hex } => decode_hex(&data_hex)
                .map(|data| RegValue::Present { kind, data })
                .ok_or_else(|| D::Error::custom("登錄值的 data_hex 不是有效的十六進位字串")),
        }
    }

    fn decode_hex(s: &str) -> Option<Vec<u8>> {
        if !s.len().is_multiple_of(2) || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
            .collect()
    }
}

/// Windows 焦點相關登錄值（讀不到為 `null`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedSpotlight {
    pub enabled_state: Option<u32>,
    pub background_type: Option<u32>,
}

impl From<SpotlightState> for PersistedSpotlight {
    fn from(s: SpotlightState) -> Self {
        Self {
            enabled_state: s.enabled_state,
            background_type: s.background_type,
        }
    }
}

/// 一台螢幕的紀錄。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorRecord {
    /// `GetMonitorDevicePathAt`（`SetWallpaper` 的 monitorID；螢幕鍵對應不到時的第二層比對）。
    pub device_path: String,
    /// 記錄當下 `GetWallpaper` 的讀回值（原路徑；空字串＝純色）。`original_unknown` 為真時無意義
    /// （空字串）。
    pub original_wallpaper: String,
    /// `wallpaper\original\` 下的備份檔名；純色、原檔不存在或複製失敗時為 `null`。
    pub backup: Option<String>,
    pub recorded_at: i64,
    /// 宿主上次準備設定的輸出圖（在 `SetWallpaper` **之前**寫入）。
    pub last_set: Option<String>,
    /// 是否曾讀回確認這台螢幕上是宿主的圖。之前（剛記錄、或設定途中當機）讀回仍是原圖不算自換。
    pub host_applied: bool,
    /// 待還原：未接管，但這台螢幕可能仍顯示宿主的圖（還原或讓位當下離線、或還原讀回失敗）。
    /// 4.7 在螢幕接上時呼叫 [`WallpaperTakeover::restore_pending_monitors`]。舊檔沒有此欄位時為
    /// `false`（本欄位在 4.4 修正輪 1 加入，此前沒有發布過的狀態檔）。
    #[serde(default)]
    pub pending_restore: bool,
    /// 原桌布未知：記錄當下螢幕上就是宿主的輸出圖（新接上的螢幕被 Windows 套用了宿主的圖、狀態檔
    /// 遺失後重新接管……）。不記錄也不備份那張圖；還原時見 [`unknown_original_fallback`]。缺少時
    /// 為 `false`（修正輪 2 加入，此前沒有發布過的狀態檔）。
    #[serde(default)]
    pub original_unknown: bool,
    /// 沿用自前一次接管、這次接管開始時離線的紀錄（修正輪 3）。這次接管從未碰過這台螢幕：它重新
    /// 接上時，讀回是宿主的圖就照常判定；不是宿主的圖就**不**做讓位判定，等要設定它時以當時的
    /// 桌布重新記錄（與新螢幕相同）。還原時也只還原顯示宿主圖者，其餘不動、不擋登錄。缺少時為
    /// `false`。
    #[serde(default)]
    pub stale: bool,
    /// 記錄當下原圖的檔案識別（task 6.4，審查 R1-M3b；查不到為 `null`，沒有時不寫出）。還原時原路徑
    /// 的識別與它不同（檔案被換掉）而有備份，就優先用備份（[`restore_target`]）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_file_id: Option<PersistedFileId>,
    /// 全域純色原桌布在接管途中的過渡讀回（task 6.1 修正輪 1，審查 L2）：這台**設定之前**讀到的 explorer 記憶
    /// 圖片。設定之後（`last_set` 有值）還沒確認套用時，只有讀回仍是這張才算過渡讀回（設定沒生效）；換成別的
    /// 圖＝使用者自換、照常讓位。沒有時不寫出、舊檔讀成沒有。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solid_transition_readback: Option<String>,
}

/// 檔案識別（磁碟區序號＋檔案索引）的持久化形式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedFileId {
    pub volume_serial: u32,
    pub file_index: u64,
}

impl From<FileId> for PersistedFileId {
    fn from(id: FileId) -> Self {
        Self {
            volume_serial: id.volume_serial,
            file_index: id.file_index,
        }
    }
}

impl PersistedFileId {
    fn matches(self, id: FileId) -> bool {
        self.volume_serial == id.volume_serial && self.file_index == id.file_index
    }
}

/// 最近一次還原或讓位（只供診斷）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LastEvent {
    Restored {
        at: i64,
        reason: String,
        /// 還原當下離線、無法還原與驗證的螢幕（裝置路徑）；它們的紀錄保留 `last_set`。
        offline_unverified: Vec<String>,
        warnings: Vec<String>,
    },
    Yielded {
        at: i64,
        monitors: Vec<String>,
    },
}

// ---------------------------------------------------------------------------------------------
// 純邏輯
// ---------------------------------------------------------------------------------------------

/// 兩個路徑是否指向同一個檔：先比路徑（Windows 序數、不分大小寫），不相等時再比檔案識別
/// （磁碟區序號＋檔案索引；8.3 短檔名、同一磁碟機上的 junction、`\\?\` 前綴）。空字串（純色）只與
/// 空字串相同。只有兩邊在同一個磁碟機代號上時才查識別（修正輪 4：不對無關的磁碟機開檔）。
pub fn same_file(a: &str, b: &str, files: &dyn FileIdentity) -> bool {
    if path_eq(a, b) {
        return true;
    }
    if a.is_empty() || b.is_empty() || !same_local_drive(a, b) {
        return false;
    }
    match (files.file_id(Path::new(a)), files.file_id(Path::new(b))) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// `path` 是否為宿主的輸出圖：先比路徑（輸出資料夾內、但不在備份資料夾內），不成立時再以檔案
/// 識別比對它所在的資料夾是不是輸出資料夾本身（輸出圖都直接放在輸出資料夾裡；備份資料夾是另一個
/// 資料夾，識別不同，自然排除）。
pub fn is_host_output_file(path: &str, paths: &StatePaths, files: &dyn FileIdentity) -> bool {
    if paths.is_host_output(path) {
        return true;
    }
    if path.is_empty() || path_is_within(path, &paths.backup_dir()) {
        return false;
    }
    let Some(parent) = Path::new(path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
    else {
        return false;
    };
    if !same_local_drive(path, &paths.output_dir.to_string_lossy()) {
        return false;
    }
    match (files.file_id(parent), files.file_id(&paths.output_dir)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// 讀回值是不是宿主的圖：等於上次所設的檔，或是輸出資料夾內的檔（兩者都先比路徑、再比識別）。
fn shows_host_image(
    record: &MonitorRecord,
    current: &str,
    paths: &StatePaths,
    files: &dyn FileIdentity,
) -> bool {
    record
        .last_set
        .as_deref()
        .is_some_and(|last| same_file(current, last, files))
        || is_host_output_file(current, paths, files)
}

/// 讀回判定的結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readback {
    /// 螢幕上是宿主的圖（上次所設的檔，或輸出資料夾內的另一個檔）。
    Host,
    /// 還沒確認套用過宿主的圖，讀回仍是原桌布（剛記錄、設定途中當機、`SetWallpaper` 尚未生效）。
    NotYetApplied,
    /// 使用者自行換了桌布：讓位。
    UserChanged,
    /// 無法判定（task 6.4，審查 R1-M4）：讀回落在 Windows 的桌布快取資料夾（例如 explorer 重新啟動
    /// 後讀回 `TranscodedWallpaper`），內容可能就是宿主的圖。不讓位、不改原桌布紀錄，只記一行，下次
    /// 再判。
    Uncertain,
    /// 接管途中的過渡讀回（task 6.1 修正 Bug 1／B9）：原桌布是全域純色，這台**還沒設定過**（或設定過但沒
    /// 生效、讀回仍是設定前記下的同一張），讀回卻是某張圖片——explorer 在宿主設定**別台**之後離開純色模式，
    /// 把其餘螢幕換回它記憶中的逐螢幕圖片。不是使用者自換：不讓位、不改原桌布紀錄（仍是純色），只記一行；
    /// 條件見 `solid_transition`。
    SolidTransition,
}

/// 讓位判定（design.md D5；brief 裁決 4）：讀回值既不是上次所設的、也不是宿主輸出資料夾內的檔，
/// 就是使用者自換。**只用 `GetWallpaper` 讀回值**（登錄 `Wallpaper` 會被改成
/// `TranscodedWallpaper`，不能用）。比對見 [`same_file`]／[`is_host_output_file`]。
///
/// 補充（`host_applied == false`，還沒確認套用過宿主的圖時）：讀回仍是原桌布不算自換——否則
/// 「記錄後、設定前當機」的下次啟動會被誤判成讓位。原桌布為投影片時，讀回落在投影片來源資料夾
/// 內；原桌布為 Windows 焦點時，讀回與原圖同一個資料夾——兩者都會自行輪替，同樣視為原桌布。
/// 原桌布未知（[`MonitorRecord::original_unknown`]）時不做這項補充。4.7 以
/// [`WallpaperTakeover::confirm_applied`] 在設定後立即確認，縮短這個窗口。
pub fn judge_readback(
    record: &MonitorRecord,
    original: &OriginalDesktop,
    readback: &str,
    paths: &StatePaths,
    files: &dyn FileIdentity,
) -> Readback {
    if shows_host_image(record, readback, paths, files) {
        return Readback::Host;
    }
    if !record.host_applied && looks_like_original(record, Some(original), readback, files) {
        return Readback::NotYetApplied;
    }
    if solid_transition(record, Some(original), readback) {
        return Readback::SolidTransition;
    }
    if paths.is_themes_cache(readback) {
        return Readback::Uncertain;
    }
    Readback::UserChanged
}

/// 讀回是不是全域純色原桌布在接管途中的過渡狀態（[`Readback::SolidTransition`]；task 6.1 修正 Bug 1）：
/// 原桌布記為全域純色、這筆紀錄的原桌布就是純色（不是之後才接上、另有原桌布的螢幕），這台還沒確認套用過
/// 宿主的圖，讀回卻不是空字串，而且：
///
/// - 這台**還沒設定過**（`last_set` 為 `None`；設定確定失敗時會退回設定前的值，見
///   `WallpaperTakeover::roll_back_last_set`）——explorer 離開純色時換上的記憶圖片；或
/// - 設定過但沒生效（SetFailed、explorer 還沒處理），讀回仍是設定前記下的那張記憶圖片
///   （[`MonitorRecord::solid_transition_readback`]）。
///
/// 已設定的螢幕讀回換成別的圖＝使用者自換，照常讓位（修正輪 1，審查 L2）；已確認套用的螢幕被換掉也照常
/// 讓位（explorer 離開純色時只改尚未設定的螢幕）。比裁定的「接管完成之前一律不讓位」更窄、更保護使用者的選擇。
fn solid_transition(
    record: &MonitorRecord,
    original: Option<&OriginalDesktop>,
    readback: &str,
) -> bool {
    original.is_some_and(|o| o.solid_color)
        && !record.original_unknown
        && record.original_wallpaper.is_empty()
        && !record.host_applied
        && !readback.is_empty()
        && (record.last_set.is_none()
            || record
                .solid_transition_readback
                .as_deref()
                .is_some_and(|seen| path_eq(seen, readback)))
}

/// 讀回值是否為這台螢幕記錄的原桌布（含投影片／焦點自行輪替的情況；原桌布未知時一律否）。
fn looks_like_original(
    record: &MonitorRecord,
    original: Option<&OriginalDesktop>,
    readback: &str,
    files: &dyn FileIdentity,
) -> bool {
    if record.original_unknown {
        return false;
    }
    let original_path = record.original_wallpaper.as_str();
    if same_file(readback, original_path, files) {
        return true;
    }
    let Some(original) = original else {
        return false;
    };
    if in_slideshow_source(original, readback) {
        return true;
    }
    original.spotlight.enabled_state == Some(1) && same_parent(readback, original_path)
}

/// 讀回是否落在原桌布投影片的來源資料夾內（原桌布不是投影片時一律否）。
fn in_slideshow_source(original: &OriginalDesktop, readback: &str) -> bool {
    original.slideshow.as_ref().is_some_and(|s| {
        s.items
            .iter()
            .any(|item| path_is_within(readback, Path::new(item)))
    })
}

fn same_parent(a: &str, b: &str) -> bool {
    match (Path::new(a).parent(), Path::new(b).parent()) {
        (Some(pa), Some(pb)) if !pa.as_os_str().is_empty() => {
            path_eq(&pa.to_string_lossy(), &pb.to_string_lossy())
        }
        _ => false,
    }
}

/// 還原前，一台在線螢幕目前的狀況（審查 finding 1／修正輪 2 裁決 1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreClass {
    /// 仍是宿主的圖：還原。
    Host,
    /// 已經是原桌布（或要還原成的那張）：不動。
    Original,
    /// 其他圖＝使用者自己選的：不動，記為新的原桌布；整次還原不寫登錄、不動全域設定。
    UserChoice,
}

/// 還原前的分類。`want` 是要還原成的值（[`restore_target`] 等的結果；純色＝空字串）。
///
/// `slideshow_resume`（task 6.4，審查 R1-M1）：這次是**續做**還原（狀態檔原本就有「還原進行中」標記）、
/// 原桌布是投影片、而目前投影片正在播放——上一次還原已呼叫過 `restore_slideshow`，explorer 正顯示投影片
/// 的某一張（不是記錄當下那張）。讀回落在投影片來源資料夾內就是「投影片還原途中」的狀態，判為原桌布，
/// 不因 `host_applied` 判成使用者自選（否則全域填滿方式、背景色、登錄永遠不還原）。
///
/// 讀回落在 Windows 的桌布快取資料夾（[`StatePaths::is_themes_cache`]；審查 R1-M4）不算使用者自選：
/// 內容可能已是宿主的圖，照原紀錄還原。
pub fn classify_for_restore(
    record: &MonitorRecord,
    original: Option<&OriginalDesktop>,
    current: &str,
    want: &str,
    paths: &StatePaths,
    files: &dyn FileIdentity,
    slideshow_resume: bool,
) -> RestoreClass {
    if shows_host_image(record, current, paths, files) {
        return RestoreClass::Host;
    }
    if same_file(current, want, files) {
        return RestoreClass::Original;
    }
    if slideshow_resume && original.is_some_and(|o| in_slideshow_source(o, current)) {
        return RestoreClass::Original;
    }
    if paths.is_themes_cache(current) {
        return RestoreClass::Host;
    }
    // task 6.1 修正（Bug 1）：原桌布是全域純色、這台還沒設定過宿主的圖，讀回卻是 explorer 換回的記憶圖片
    // ——接管途中的過渡狀態，不是使用者自選：照原紀錄還原（全域純色）。
    if solid_transition(record, original, current) {
        return RestoreClass::Host;
    }
    let rotating_ok = !record.host_applied;
    let is_original = if rotating_ok {
        looks_like_original(record, original, current, files)
    } else {
        !record.original_unknown && same_file(current, &record.original_wallpaper, files)
    };
    if is_original {
        RestoreClass::Original
    } else {
        RestoreClass::UserChoice
    }
}

/// 一台螢幕要還原成什麼。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreTarget {
    /// 純色（`SetWallpaper` 空字串，顏色由全域背景色還原）。
    Solid,
    /// 這個圖片路徑（原路徑，或原路徑不存在時的備份）。
    Image(String),
    /// 原圖與備份都不存在：呼叫端改還原為純色並記警告。
    Missing,
    /// 原桌布未知（記錄當下就是宿主的圖）：呼叫端改用 [`unknown_original_fallback`]。
    Unknown,
}

/// 還原來源（brief 裁決 2）：原路徑存在就用原路徑；不存在就用備份（spec「原圖之後被刪除仍可
/// 還原」）。原路徑是宿主輸出圖時一律用備份（輸出圖會被 a/b 輪替覆寫；修正輪 2 起不會再這樣
/// 記錄，留著防舊資料）。`exists` 注入檔案是否存在的判定。
///
/// task 6.4（審查 R1-M3b）：原路徑存在、但位於 Windows 的桌布快取資料夾（[`StatePaths::is_themes_cache`]：
/// explorer 會改寫它，內容可能已是宿主的圖），或它的檔案識別已與記錄時不同（檔案被換掉）——有備份就
/// 優先用備份；沒有備份才用原路徑。
pub fn restore_target(
    record: &MonitorRecord,
    paths: &StatePaths,
    exists: &dyn Fn(&Path) -> bool,
    files: &dyn FileIdentity,
) -> RestoreTarget {
    if record.original_unknown {
        return RestoreTarget::Unknown;
    }
    let original = record.original_wallpaper.as_str();
    if original.is_empty() {
        return RestoreTarget::Solid;
    }
    let backup = record
        .backup
        .as_ref()
        .map(|b| paths.backup_dir().join(b))
        .filter(|b| exists(b));
    if !paths.is_host_output(original) && exists(Path::new(original)) {
        let untrusted = paths.is_themes_cache(original)
            || record.original_file_id.is_some_and(|recorded| {
                files
                    .file_id(Path::new(original))
                    .is_some_and(|now| !recorded.matches(now))
            });
        return match backup {
            Some(b) if untrusted => {
                log::info!(
                    "原路徑 {original} 在桌布快取資料夾或已被換掉：改用備份 {}",
                    b.display()
                );
                RestoreTarget::Image(b.to_string_lossy().into_owned())
            }
            _ => RestoreTarget::Image(original.to_owned()),
        };
    }
    match backup {
        Some(b) => RestoreTarget::Image(b.to_string_lossy().into_owned()),
        None => RestoreTarget::Missing,
    }
}

/// 原桌布未知的螢幕要還原成什麼（修正輪 2 裁決 4）：
///
/// 1. 已知的原桌布（`known`，不含未知者與 `stale` 者——修正輪 4：離線很久的紀錄不參與）全部是
///    同一張（或全部是純色）→ 用它；
/// 2. 否則用接管開始時登錄 `Wallpaper` 記錄的原值——必須是真實存在的檔、不是
///    `TranscodedWallpaper`、不是宿主輸出圖（`REG_EXPAND_SZ` 先展開環境變數）；
/// 3. 否則 [`RestoreTarget::Missing`]（呼叫端改純色並記警告）。
pub fn unknown_original_fallback(
    known: &[&MonitorRecord],
    registry_wallpaper: Option<&RegValue>,
    paths: &StatePaths,
    exists: &dyn Fn(&Path) -> bool,
    files: &dyn FileIdentity,
) -> RestoreTarget {
    let known: Vec<&&MonitorRecord> = known
        .iter()
        .filter(|r| !r.original_unknown && !r.stale)
        .collect();
    if let Some(first) = known.first() {
        if known
            .iter()
            .all(|r| path_eq(&r.original_wallpaper, &first.original_wallpaper))
        {
            match restore_target(first, paths, exists, files) {
                RestoreTarget::Missing | RestoreTarget::Unknown => {}
                found => return found,
            }
        }
    }
    match registry_original(registry_wallpaper, paths, exists) {
        Some(path) => RestoreTarget::Image(path),
        None => RestoreTarget::Missing,
    }
}

/// 接管開始時登錄 `Wallpaper` 記錄的原值，可用時（真實存在、不在桌布快取資料夾——`TranscodedWallpaper`
/// 等，task 6.4——、不是宿主輸出圖；`REG_EXPAND_SZ` 先展開環境變數）。
fn registry_original(
    registry_wallpaper: Option<&RegValue>,
    paths: &StatePaths,
    exists: &dyn Fn(&Path) -> bool,
) -> Option<String> {
    let path = expand_env_vars(&registry_wallpaper.and_then(RegValue::as_string)?);
    (!path.is_empty()
        && !paths.is_themes_cache(&path)
        && !paths.is_host_output(&path)
        && exists(Path::new(&path)))
    .then_some(path)
}

/// 展開 `%名稱%`（環境變數不存在時原樣保留），供 `REG_EXPAND_SZ` 使用。
fn expand_env_vars(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('%') else {
            break;
        };
        let name = &after[..end];
        out.push_str(&rest[..start]);
        match std::env::var(name) {
            Ok(v) if !name.is_empty() => out.push_str(&v),
            _ => {
                out.push('%');
                out.push_str(name);
                out.push('%');
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

// ---------------------------------------------------------------------------------------------
// 狀態機的輸入輸出
// ---------------------------------------------------------------------------------------------

/// 為什麼還原。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreReason {
    /// 使用者在設定中選「不接管」。
    NotTakeover,
    /// 系統匣「結束」。
    TrayExit,
    /// `fc-host.exe --restore-wallpaper`。
    CommandLine,
    /// explorer 資源安全閥（4.9）：與「不接管」同一條還原路徑，另外主題改 none 並通知。
    SafetyValve { detail: String },
}

impl RestoreReason {
    fn label(&self) -> String {
        match self {
            Self::NotTakeover => "not_takeover".to_owned(),
            Self::TrayExit => "tray_exit".to_owned(),
            Self::CommandLine => "command_line".to_owned(),
            Self::SafetyValve { detail } => format!("safety_valve: {detail}"),
        }
    }

    /// 這次還原之後宿主就要結束：狀態檔暫時讀不到時要先重讀一次再放棄。
    fn is_final(&self) -> bool {
        matches!(self, Self::TrayExit | Self::CommandLine)
    }
}

/// 主題為何被改成「不接管」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeNoneCause {
    UserChangedWallpaper,
    SpotlightCancelled,
    SafetyValve,
    /// 上次的還原沒做完、這次啟動續做完成（4.7b）：維持不接管直到使用者重新選主題。
    RestoreResumed,
    /// `--restore-wallpaper`（4.7b）：解除安裝情境下次啟動不再接管。
    CommandLine,
    /// 原桌布無法備份（task 6.4，審查 R1-M3a）：原圖與系統快取的轉存檔都複製不了，不接管。
    BackupFailed,
    /// 系統匣「結束」（task 6.4，審查 R1 low：不論還原是否在時限內完成，一律存成「不接管」）。
    TrayExit,
}

/// 要以系統匣通知告訴使用者的事（文字與顯示屬 4.7／4.8）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TakeoverNotice {
    /// 使用者自行換了桌布，宿主已讓位（可在設定中重新開啟）。
    Yielded { device_paths: Vec<String> },
    /// 安全閥觸發，已停止接管（`restore`＝這次還原的結果，通知文字依它決定：未接管時沒有東西可
    /// 還原，不可寫「已還原」；4.8 ledger）。`detail`（讀值、基準、門檻）只供記錄；task 6.1 起通知內文
    /// 不帶它（橫幅只顯示前幾行）。
    SafetyValve {
        detail: String,
        restore: ValveRestore,
    },
    /// 原桌布無法備份（task 6.4，審查 R1-M3a）：不接管（已接管時停止接管並還原），主題改「不接管」。
    /// `detail` 只寫進記錄（通知文字不帶路徑，避免超過系統匣的長度上限）。
    BackupFailed { detail: String },
}

/// 安全閥觸發時那次還原的結果（只供通知文字；4.8）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValveRestore {
    /// 已還原原桌布（每台都還原了，或本來就是原桌布）。
    Restored,
    /// 已還原，但有螢幕上是使用者自己換的桌布、維持不動（task 6.4：通知文字不得寫成「已還原原本的
    /// 桌布」）。
    RestoredKeepingUserChoice,
    /// 未接管、沒有東西要還原（桌布沒被宿主改過）。
    NothingToRestore,
    /// 這次沒還原成（讀取／驗證失敗、狀態檔暫時讀不到）：協調迴圈稍後重試。
    RetryLater,
    /// 狀態檔讀不懂，無法自動還原。
    Unable,
}

impl ValveRestore {
    /// 由還原結果歸類（`Queued` 不會帶事件，歸為稍後重試）。
    pub fn from_result(result: &RestoreResult) -> Self {
        match result {
            RestoreResult::Restored {
                user_choice_kept, ..
            } if !user_choice_kept.is_empty() => Self::RestoredKeepingUserChoice,
            RestoreResult::Restored { .. } => Self::Restored,
            RestoreResult::NothingToRestore => Self::NothingToRestore,
            RestoreResult::Blocked => Self::Unable,
            RestoreResult::Failed { .. }
            | RestoreResult::StateUnavailable
            | RestoreResult::Queued => Self::RetryLater,
        }
    }
}

/// 渲染前就判定「這次接管要先問 Windows 焦點」（4.8；design.md D5）：未接管、這次執行還沒確認、
/// `EnabledState`＝1。`EnabledState` 讀不到、型別不符、不是 1 一律視同一般圖片（不問）。
/// [`WallpaperTakeover::set_monitor_wallpaper`] 第一次設定前與設定視窗的選主題流程共用這條規則。
pub fn needs_spotlight_confirmation(
    taken_over: bool,
    phase: Phase,
    spotlight: &SpotlightState,
) -> bool {
    !taken_over && phase != Phase::SpotlightConfirmed && spotlight.is_spotlight()
}

/// 狀態機要求呼叫端執行的副作用（4.7 套用到設定與系統匣）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TakeoverEvent {
    /// 把設定的桌布主題改成「不接管」並存檔。
    ThemeSetToNone { cause: ThemeNoneCause },
    /// 顯示一則系統匣通知。
    Notify(TakeoverNotice),
}

/// 拒絕設定的原因（都沒有呼叫 `SetWallpaper`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefuseReason {
    /// 狀態檔讀不懂（[`BlockReason`]）。
    Blocked,
    /// 狀態檔暫時讀不到（共用違規、權限……）：下一次評估重讀。
    StateUnavailable,
    /// 讓位或安全閥之後、主題重新開啟之前。
    Stopped,
    /// 有還原在排隊（等進行中的 `Redraw` 結束）。
    RestorePending,
    /// 要設的圖不在宿主輸出資料夾內（或在原圖備份資料夾內）。
    NotHostOutput,
    /// 這台螢幕目前離線或不存在。
    MonitorNotOnline,
    /// 狀態檔寫入失敗：先寫後設，寫不進去就不設。
    StateWriteFailed(String),
    /// 記錄原桌布失敗（登錄快照失敗等），不接管。
    RecordFailed(String),
}

/// [`WallpaperTakeover::set_monitor_wallpaper`] 的結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetOutcome {
    /// 狀態已寫入、`SetWallpaper` 已被接受（是否生效以讀回為準；4.7 稍後讀回並呼叫
    /// [`WallpaperTakeover::confirm_applied`]）。
    Applied,
    /// 狀態已寫入，但 `SetWallpaper` 失敗。確定沒有生效的失敗（後端錯誤、請求沒送出）時，本模組已把這台的
    /// `last_set` 退回設定前的值並存檔（task 6.1 修正輪 2）；逾時不退（可能之後才生效）。呼叫端不需另外處理。
    SetFailed(WallpaperError),
    /// 設定前的讀取失敗（逾時、忙碌、COM 錯誤）：**不算**自換，本次跳過、下次再判定。
    ReadbackFailed(WallpaperError),
    /// 使用者自行換了桌布：已讓位（狀態檔改未接管、排程器已 `reset`），呼叫端套用事件。
    Yielded { events: Vec<TakeoverEvent> },
    /// 原桌布為 Windows 焦點，等使用者確認；什麼都沒設定、沒寫狀態檔。
    AwaitingSpotlightConfirmation,
    /// 原桌布無法備份（task 6.4，審查 R1-M3a）：什麼都沒設定、沒寫狀態檔，狀態機進入 Stopped；呼叫端
    /// 套用事件（主題改「不接管」＋通知），**已接管時另外以 `RestoreReason::NotTakeover` 還原**（套用
    /// `ThemeSetToNone` 本身不會觸發還原）。
    BackupFailed { events: Vec<TakeoverEvent> },
    /// 拒絕設定。
    Refused(RefuseReason),
}

/// 還原的結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreResult {
    /// 未接管、也沒有在線的待還原螢幕：什麼都不做，**不寫登錄**。
    NothingToRestore,
    /// `Redraw` 進行中，已排隊（見呼叫端契約第 3 點）。
    Queued,
    /// 狀態檔讀不懂，無法還原（不動任何東西）。
    Blocked,
    /// 狀態檔暫時讀不到（系統匣結束與 `--restore-wallpaper` 已重讀一次）：不動任何東西。
    StateUnavailable,
    /// 已還原並逐項讀回驗證通過；狀態改為未接管。
    Restored {
        /// 離線、無法還原與驗證的螢幕（裝置路徑）；已標為待還原。
        offline_unverified: Vec<String>,
        /// 螢幕上是使用者自己選的桌布、因此沒有還原的螢幕（裝置路徑）；有任何一台時，整次不寫
        /// 登錄、不動全域設定。
        user_choice_kept: Vec<String>,
        /// 驗證通過但過程中有的狀況（某個 API 呼叫失敗後讀回仍正確、原圖與備份都不見而改純色……）。
        warnings: Vec<String>,
        /// 結果是否已寫進狀態檔（失敗時下次啟動會再處理一次，結果相同）。
        state_saved: bool,
    },
    /// 讀取或讀回驗證失敗：狀態檔保持原樣，下次可重試或以 `--restore-wallpaper` 處理。
    Failed { problems: Vec<String> },
}

/// [`WallpaperTakeover::restore_pending_monitors`] 的結果（裝置路徑）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingReport {
    /// 讀回仍是宿主的圖，已還原並驗證。
    pub restored: Vec<String>,
    /// 讀回是其他圖：使用者這段期間自己換了（記為新的原桌布）、或已經是原桌布；只清掉待還原
    /// 標記，不覆蓋。
    pub cleared: Vec<String>,
    /// 仍離線：維持待還原。
    pub still_offline: Vec<String>,
    /// 還原或讀回驗證失敗：維持待還原（附原因）。
    pub failed: Vec<(String, String)>,
    /// 原桌布是純色、重新接上時仍顯示宿主的圖，但其他螢幕已不是純色（使用者改選了圖片）：純色是全域狀態、
    /// 無法只還原這台，不動它、清掉待還原——**之後不會再自動還原**，要使用者自行更換（task 6.1 修正輪 2，
    /// 複審 L4）。與 `cleared`（已不是宿主的圖）意義相反，分開列出。
    pub left_showing_host: Vec<String>,
    /// 變更是否已寫進狀態檔（沒有變更時為真）。
    pub state_saved: bool,
}

impl Default for PendingReport {
    fn default() -> Self {
        Self {
            restored: Vec::new(),
            cleared: Vec::new(),
            still_offline: Vec::new(),
            failed: Vec::new(),
            left_showing_host: Vec::new(),
            state_saved: true,
        }
    }
}

/// [`WallpaperTakeover::restore`] 的回傳值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreOutcome {
    pub result: RestoreResult,
    /// 要套用的事件（安全閥：主題改 none＋通知；排隊時為空，實際執行時才給）。
    pub events: Vec<TakeoverEvent>,
}

/// 狀態檔為何封鎖（整次執行；暫時性的讀檔失敗不在此列，見
/// [`WallpaperTakeover::unavailable`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockReason {
    /// 內容損壞（JSON、欄位、登錄值編碼、一致性）。
    Corrupt(String),
    /// `version` 不是 [`STATE_VERSION`]（`None`＝沒有或不是整數）。
    UnknownVersion(Option<u64>),
}

/// 這次執行的記憶體狀態（不持久化）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// 一般狀態。
    Idle,
    /// 原桌布為 Windows 焦點，等使用者確認；期間不設定任何桌布。
    AwaitingSpotlightConfirmation,
    /// 使用者已確認，下一次設定會開始接管。
    SpotlightConfirmed,
    /// 讓位或安全閥之後：選了某個主題（[`WallpaperTakeover::theme_selected`]）之前不再接管。
    Stopped,
}

/// 狀態機設定（等待時間都可注入，測試用短值）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TakeoverConfig {
    /// 還原後讀回驗證的次數（至少 1；預設 2＝失敗時隔一段時間再驗一次）。
    pub verify_attempts: u32,
    /// 兩次驗證之間的間隔。
    pub verify_interval: Duration,
    /// 還原時，逐螢幕 `SetWallpaper` 之後、寫回登錄之前，等 explorer 處理（登錄 `Wallpaper`
    /// 變化）的上限（探針 1.1 的還原等 500 ms）。
    pub registry_settle_timeout: Duration,
    /// 等待期間輪詢登錄的間隔。
    pub registry_poll_interval: Duration,
    /// 系統匣結束／`--restore-wallpaper` 時，狀態檔暫時讀不到，隔多久重讀一次。
    pub transient_retry_delay: Duration,
}

impl Default for TakeoverConfig {
    fn default() -> Self {
        Self {
            verify_attempts: 2,
            verify_interval: Duration::from_millis(500),
            registry_settle_timeout: Duration::from_secs(2),
            registry_poll_interval: Duration::from_millis(50),
            transient_retry_delay: Duration::from_millis(500),
        }
    }
}

/// 一次操作要用的 I/O（呼叫端每次組一份）。
pub struct TakeoverIo<'a> {
    /// 桌布 API（不得在主執行緒上呼叫，見 `crate::desktop::wallpaper`）。
    pub wallpaper: &'a WallpaperService,
    /// `HKCU` 登錄（正式為 `registry::HkcuRegistry`）。
    pub registry: &'a mut dyn RegistryStore,
    /// 穩定顯示器識別與矩形（[`monitor_key`]；查不到時傳空切片）。
    pub stable_displays: &'a [StableDisplay],
    /// 檔案識別（正式為 `file_identity::Win32FileIdentity`）。
    pub files: &'a dyn FileIdentity,
    /// 現在（Unix 秒）。
    pub now: i64,
}

#[derive(Debug)]
enum Loaded {
    /// 裝箱：狀態檔內容遠大於其他變體。
    Ready(Box<StateFile>),
    /// 整次執行封鎖。
    Blocked(BlockReason),
    /// 暫時讀不到；每個操作開始時重讀一次。
    Unavailable(String),
}

/// 讓位時，使用者自換者以外的一台螢幕要怎麼處理（[`WallpaperTakeover::yield_to_user`] 的第一趟）。
enum YieldAction {
    /// 顯示宿主的圖：還原成這個。
    Restore(Want),
    /// 仍是原桌布：只清掉宿主圖的標記。
    ClearMarks,
    /// 離線：設定過宿主的圖就標為待還原。
    MarkPendingIfSet,
}

/// 還原計畫中的一台在線螢幕。
struct PlanItem {
    key: String,
    device: String,
    current: String,
    /// 要還原成什麼（含 [`Want::own_copy`]：不是使用者的選擇時採納為這台的原桌布）。
    want: Want,
    class: RestoreClass,
}

// ---------------------------------------------------------------------------------------------
// 狀態機
// ---------------------------------------------------------------------------------------------

/// 接管狀態機（規則見模組文件）。
#[derive(Debug)]
pub struct WallpaperTakeover {
    paths: StatePaths,
    config: TakeoverConfig,
    state: Loaded,
    phase: Phase,
    redraw_in_progress: bool,
    pending_restore: Option<RestoreReason>,
    /// 最近一次讀到的各螢幕桌布路徑（含讀得到的離線螢幕）：清理孤兒備份時，正在當桌布的檔絕不刪。
    seen_wallpapers: Vec<String>,
    /// 外部要求的標記（4.7b 修正輪 1）：寫檔時持它的鎖，必要時把標記一起寫進去。
    restore_request: Option<Arc<RestoreRequestMarker>>,
}

impl WallpaperTakeover {
    /// 讀狀態檔（不存在＝從未接管，**不**建立檔案）。損壞、未知版本時封鎖；暫時讀不到時之後的
    /// 每個操作會重讀。`now` 用於 `.corrupt-<時間>` 副本的檔名。
    pub fn load(paths: StatePaths, config: TakeoverConfig, now: i64) -> Self {
        let state = load_state(&paths.state_file, now);
        Self {
            paths,
            config,
            state,
            phase: Phase::Idle,
            redraw_in_progress: false,
            pending_restore: None,
            seen_wallpapers: Vec::new(),
            restore_request: None,
        }
    }

    /// 接上外部要求的標記（4.7b 修正輪 1；協調迴圈建立時呼叫）。
    pub fn attach_restore_request(&mut self, request: Arc<RestoreRequestMarker>) {
        self.restore_request = Some(request);
    }

    /// 記下這次讀到的螢幕桌布（見 `seen_wallpapers`）。離線螢幕讀得到時（Windows 為它記住的桌布）
    /// 也列入（修正輪 R：使用者可能在宿主停止期間替它選了宿主的備份檔）；讀不到的不臆測。
    fn remember_seen(&mut self, snap: &WallpaperSnapshot) {
        self.seen_wallpapers = snap
            .monitors
            .iter()
            .filter_map(|m| m.wallpaper.clone())
            .filter(|w| !w.is_empty())
            .collect();
    }

    /// 排程器的 `already_taken_over`：狀態檔為接管中（封鎖或暫時讀不到時保守回真）。
    pub fn already_taken_over(&self) -> bool {
        match &self.state {
            Loaded::Blocked(_) | Loaded::Unavailable(_) => true,
            Loaded::Ready(s) => s.status == TakeoverStatus::TakenOver,
        }
    }

    /// 封鎖原因（`None`＝沒有封鎖）。
    pub fn blocked(&self) -> Option<&BlockReason> {
        match &self.state {
            Loaded::Blocked(r) => Some(r),
            _ => None,
        }
    }

    /// 狀態檔暫時讀不到的原因（`None`＝讀得到）。
    pub fn unavailable(&self) -> Option<&str> {
        match &self.state {
            Loaded::Unavailable(e) => Some(e),
            _ => None,
        }
    }

    /// 目前的狀態檔內容（封鎖或暫時讀不到時 `None`）。
    pub fn state(&self) -> Option<&StateFile> {
        match &self.state {
            Loaded::Ready(s) => Some(s.as_ref()),
            _ => None,
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// 狀態檔暫時讀不到時重讀一次（4.7b 審查 F1：協調迴圈的延後啟動判定）；回傳現在是否已不是
    /// 「暫時讀不到」（讀到了或封鎖）。
    pub fn reload_if_unavailable(&mut self, now: i64) -> bool {
        self.refresh(now);
        self.unavailable().is_none()
    }

    /// 暫時讀不到時重讀一次。
    fn refresh(&mut self, now: i64) {
        if matches!(self.state, Loaded::Unavailable(_)) {
            self.state = load_state(&self.paths.state_file, now);
        }
    }

    /// 使用者在焦點提示中按「確認」：下一次設定開始接管。回傳階段是否因此改為已確認。
    ///
    /// 4.8：設定視窗在使用者選主題時就先問（渲染前），確認可能早於第一次設定、甚至早於主題變更被
    /// 協調迴圈看到——**未接管**時「一般」與「停止」（讓位／安全閥之後）階段也接受這個事先確認；
    /// 之後選「不接管」（[`Self::theme_selected`]）即撤回。已接管時不會再問，維持原階段（停止接管
    /// 的還原還沒完成時不得解除停止）。
    pub fn confirm_spotlight(&mut self) -> bool {
        let accept = match self.phase {
            Phase::AwaitingSpotlightConfirmation => true,
            Phase::Idle | Phase::Stopped => !self.already_taken_over(),
            Phase::SpotlightConfirmed => false,
        };
        if accept {
            self.phase = Phase::SpotlightConfirmed;
        }
        accept
    }

    /// 使用者在焦點提示中按「取消」：不接管，回傳「主題改回 none」事件。
    pub fn cancel_spotlight(&mut self) -> Vec<TakeoverEvent> {
        if !matches!(
            self.phase,
            Phase::AwaitingSpotlightConfirmation | Phase::SpotlightConfirmed
        ) {
            return Vec::new();
        }
        self.phase = Phase::Idle;
        log::info!("原桌布為 Windows 焦點，使用者取消接管；主題維持「不接管」");
        vec![TakeoverEvent::ThemeSetToNone {
            cause: ThemeNoneCause::SpotlightCancelled,
        }]
    }

    /// 設定中的主題改變（4.7 在套用設定變更時呼叫）。選了某個主題＝解除讓位／安全閥後的停止；
    /// 選「不接管」＝放棄等待中的焦點確認，**不**解除停止（4.7 套用 `ThemeSetToNone` 事件時
    /// 也會走到這裡，此時同一次重畫的其餘螢幕仍必須被擋下）。
    pub fn theme_selected(&mut self, theme: WallpaperTheme) {
        match (theme, self.phase) {
            (WallpaperTheme::None, Phase::Stopped) => {}
            (_, Phase::Stopped) => self.phase = Phase::Idle,
            (
                WallpaperTheme::None,
                Phase::AwaitingSpotlightConfirmation | Phase::SpotlightConfirmed,
            ) => self.phase = Phase::Idle,
            _ => {}
        }
    }

    /// 開始執行一份 `RedrawPlan`。
    pub fn redraw_started(&mut self) {
        self.redraw_in_progress = true;
    }

    /// `RedrawPlan` 執行完（或中止）。回傳排隊中的還原原因：呼叫端立即以它呼叫 [`Self::restore`]。
    pub fn redraw_finished(&mut self) -> Option<RestoreReason> {
        self.redraw_in_progress = false;
        self.pending_restore.take()
    }

    /// 設定後確認已套用（修正輪 2 裁決 2；**4.7 的義務**）：`set_monitor_wallpaper` 回
    /// [`SetOutcome::Applied`] 後，等一小段時間（約 1 秒）再讀回該螢幕，以讀回值呼叫本函式。
    /// 讀回是宿主的圖就立即把 `host_applied` 設為真並存檔——之後使用者換回原圖會被判為讓位，
    /// 「尚未套用」的窗口不再長達一整個重畫間隔。回傳是否已確認（已確認過也回真）。
    pub fn confirm_applied(
        &mut self,
        io: &TakeoverIo<'_>,
        device_path: &str,
        readback: &str,
    ) -> bool {
        self.refresh(io.now);
        let Loaded::Ready(state) = &self.state else {
            return false;
        };
        if state.status != TakeoverStatus::TakenOver {
            return false;
        }
        let mut next = state.as_ref().clone();
        let Some(rec) = next
            .monitors
            .values_mut()
            .find(|r| path_eq(&r.device_path, device_path))
        else {
            return false;
        };
        if rec.host_applied {
            return true;
        }
        if !shows_host_image(rec, readback, &self.paths, io.files) {
            return false;
        }
        rec.host_applied = true;
        rec.stale = false;
        rec.solid_transition_readback = None;
        if let Err(e) = self.persist(&next, io.files) {
            log::warn!("確認已套用後寫入狀態檔失敗（記憶體中已確認）：{e}");
        }
        self.state = Loaded::Ready(Box::new(next));
        true
    }

    /// 設定一台螢幕的桌布（唯一允許的入口；流程見模組文件「呼叫端契約」第 2 點）。`image` 必須是
    /// 宿主輸出資料夾內的檔。
    pub fn set_monitor_wallpaper(
        &mut self,
        io: &mut TakeoverIo<'_>,
        scheduler: &mut SchedulerState,
        device_path: &str,
        image: &Path,
    ) -> SetOutcome {
        self.refresh(io.now);
        let current = match &self.state {
            Loaded::Blocked(reason) => {
                log::error!("狀態檔無法使用（{reason:?}），不設定桌布：{device_path}");
                return SetOutcome::Refused(RefuseReason::Blocked);
            }
            Loaded::Unavailable(e) => {
                log::warn!("狀態檔暫時讀不到（{e}），本次不設定桌布：{device_path}");
                return SetOutcome::Refused(RefuseReason::StateUnavailable);
            }
            Loaded::Ready(s) => s.as_ref().clone(),
        };
        if self.pending_restore.is_some() {
            return SetOutcome::Refused(RefuseReason::RestorePending);
        }
        if current.status == TakeoverStatus::TakenOver && current.restore_in_progress.is_some() {
            // 審查 F1（防禦）：上次的還原沒做完——先續做，不得照常接管，也不做讓位判定（會丟掉 original）。
            log::warn!("狀態檔有「還原進行中」標記：拒絕設定桌布 {device_path}（要先續做還原）");
            return SetOutcome::Refused(RefuseReason::RestorePending);
        }
        match self.phase {
            Phase::Stopped => return SetOutcome::Refused(RefuseReason::Stopped),
            Phase::AwaitingSpotlightConfirmation => {
                return SetOutcome::AwaitingSpotlightConfirmation
            }
            Phase::Idle | Phase::SpotlightConfirmed => {}
        }
        let image_str = match image.to_str() {
            Some(s) if self.paths.is_host_output(s) => s.to_owned(),
            _ => {
                log::error!(
                    "拒絕設定桌布：{} 不在宿主輸出資料夾 {} 內",
                    image.display(),
                    self.paths.output_dir.display()
                );
                return SetOutcome::Refused(RefuseReason::NotHostOutput);
            }
        };

        let first = current.status == TakeoverStatus::NotTakenOver;
        let spotlight = if first {
            let s = registry::read_spotlight_in(&*io.registry);
            if needs_spotlight_confirmation(false, self.phase, &s) {
                self.phase = Phase::AwaitingSpotlightConfirmation;
                log::info!("原桌布為 Windows 焦點（EnabledState=1），等使用者確認後才接管");
                return SetOutcome::AwaitingSpotlightConfirmation;
            }
            Some(s)
        } else {
            None
        };

        let snapshot = match io.wallpaper.read() {
            Ok(s) => s,
            Err(e) => {
                log::warn!("設定桌布前讀回失敗（不算使用者自換，本次跳過）：{e}");
                return SetOutcome::ReadbackFailed(e);
            }
        };
        self.remember_seen(&snapshot);
        let Some(target) = snapshot
            .monitors
            .iter()
            .find(|m| m.monitor.is_online() && path_eq(&m.monitor.device_path, device_path))
        else {
            log::warn!("螢幕 {device_path} 目前離線或不存在，不設定");
            return SetOutcome::Refused(RefuseReason::MonitorNotOnline);
        };
        let Some(target_wallpaper) = target.wallpaper.clone() else {
            return SetOutcome::Refused(RefuseReason::RecordFailed(format!(
                "在線螢幕 {device_path} 讀不到目前的桌布"
            )));
        };
        let target_key = monitor_key(
            &target.monitor.device_path,
            target.monitor.rect,
            io.stable_displays,
        );

        let mut next = if let Some(spotlight) = spotlight {
            let reg = match registry::snapshot_desktop_registry_in(&*io.registry) {
                Ok(r) => r,
                Err(e) => {
                    log::error!("記錄原桌布失敗（登錄快照），不接管：{e}");
                    return SetOutcome::Refused(RefuseReason::RecordFailed(e.to_string()));
                }
            };
            match begin_state(
                &self.paths,
                &current,
                &snapshot,
                reg,
                spotlight,
                io,
                &self.seen_wallpapers,
            ) {
                Ok(s) => s,
                Err(BeginError::Record(e)) => {
                    log::error!("記錄原桌布失敗，不接管：{e}");
                    return SetOutcome::Refused(RefuseReason::RecordFailed(e));
                }
                Err(BeginError::Backup(e)) => return self.backup_failed(e),
            }
        } else {
            let Some(original) = current.original.clone() else {
                return SetOutcome::Refused(RefuseReason::RecordFailed(
                    "狀態檔為接管中卻沒有原桌布記錄".to_owned(),
                ));
            };
            let mut next = current.clone();
            let mut changed = Vec::new();
            for mw in snapshot.monitors.iter().filter(|m| m.monitor.is_online()) {
                let Some(readback) = mw.wallpaper.as_deref() else {
                    continue;
                };
                let key = monitor_key(&mw.monitor.device_path, mw.monitor.rect, io.stable_displays);
                let Some(k) = find_record(&next, &key, &mw.monitor.device_path) else {
                    continue;
                };
                let Some(rec) = next.monitors.get_mut(&k) else {
                    continue;
                };
                if rec.stale {
                    // 這次接管從未碰過它：是宿主的圖就收編，否則不判定（要設定它時重新記錄）。
                    if shows_host_image(rec, readback, &self.paths, io.files) {
                        rec.stale = false;
                        rec.host_applied = true;
                    }
                    continue;
                }
                match judge_readback(rec, &original, readback, &self.paths, io.files) {
                    Readback::Host => rec.host_applied = true,
                    Readback::NotYetApplied => {}
                    // task 6.4（審查 R1-M4）：讀回落在桌布快取資料夾，可能就是宿主的圖——不讓位、不改
                    // 原桌布紀錄、不動備份，下一次設定前再判。
                    Readback::Uncertain => log::warn!(
                        "螢幕 {} 讀回的是 Windows 的桌布快取 {readback}：無法判定是不是使用者自換，這次不讓位、不改原桌布紀錄，下次再判",
                        mw.monitor.device_path
                    ),
                    // task 6.1 修正（Bug 1／B9）：原桌布是全域純色，宿主設定別台後 explorer 把這台換回它記憶
                    // 中的圖片——接管途中的過渡讀回，不讓位；這台設定並確認之後照常判定。
                    Readback::SolidTransition => {
                        log::info!(
                            "螢幕 {} 讀回 {readback}：原桌布是全域純色，這是設定別台後 explorer 換回的記憶圖片（接管途中的過渡讀回），不算使用者自換",
                            mw.monitor.device_path
                        );
                        if rec.last_set.is_none() {
                            rec.solid_transition_readback = Some(readback.to_owned());
                        }
                    }
                    Readback::UserChanged => changed.push((k, readback.to_owned())),
                }
            }
            if !changed.is_empty() {
                let events = self.yield_to_user(io, scheduler, next, changed, &snapshot);
                return SetOutcome::Yielded { events };
            }
            match find_record(&next, &target_key, device_path) {
                None => {
                    log::info!("接管期間出現新螢幕 {device_path}：先記錄原桌布再設定");
                    let key =
                        free_key(&|k| next.monitors.contains_key(k), &target_key, device_path);
                    let guard = BackupGuard {
                        paths: &self.paths,
                        states: vec![&next.monitors],
                        in_use: &self.seen_wallpapers,
                        files: io.files,
                    };
                    let rec = match record_monitor(
                        &guard,
                        &key,
                        &target.monitor.device_path,
                        &target_wallpaper,
                        io.now,
                        None,
                    ) {
                        Ok(rec) => rec,
                        Err(e) => return self.backup_failed(e),
                    };
                    next.monitors.insert(key, rec);
                }
                Some(k) if next.monitors.get(&k).is_some_and(|r| r.stale) => {
                    log::info!(
                        "螢幕 {device_path} 接上時已不是宿主的圖：以目前的桌布重新記錄原桌布再設定"
                    );
                    let guard = BackupGuard {
                        paths: &self.paths,
                        states: vec![&next.monitors],
                        in_use: &self.seen_wallpapers,
                        files: io.files,
                    };
                    let rec = match record_monitor(
                        &guard,
                        &k,
                        &target.monitor.device_path,
                        &target_wallpaper,
                        io.now,
                        None,
                    ) {
                        Ok(rec) => rec,
                        Err(e) => return self.backup_failed(e),
                    };
                    next.monitors.insert(k, rec);
                }
                Some(_) => {}
            }
            next
        };

        let Some(rec) = next
            .monitors
            .values_mut()
            .find(|r| path_eq(&r.device_path, device_path))
        else {
            return SetOutcome::Refused(RefuseReason::RecordFailed(format!(
                "找不到 {device_path} 的紀錄"
            )));
        };
        let previous_last_set = rec.last_set.replace(image_str.clone());
        // 首次接管（或接管後還沒有任何一台確認套用過——例如記錄後、設定填滿方式前當機）時把
        // 填滿方式設為 FILL；之後不再動它。
        let set_fill = snapshot.position != WallpaperPosition::FILL
            && next.monitors.values().all(|r| !r.host_applied);

        if let Err(e) = self.persist(&next, io.files) {
            log::error!(
                "寫入狀態檔 {} 失敗，不設定桌布（先寫後設）：{e}",
                self.paths.state_file.display()
            );
            return SetOutcome::Refused(RefuseReason::StateWriteFailed(e.to_string()));
        }
        self.state = Loaded::Ready(Box::new(next));
        if first {
            self.phase = Phase::Idle;
            log::info!("開始接管桌布：已記錄原桌布並寫入狀態檔");
        }
        if set_fill {
            if let Err(e) = io.wallpaper.set_position(WallpaperPosition::FILL) {
                log::warn!("設定填滿方式為 FILL 失敗：{e}");
            }
        }
        match io.wallpaper.set_wallpaper(device_path, image) {
            Ok(()) => SetOutcome::Applied,
            Err(e) => {
                log::warn!("設定桌布失敗（{device_path}）：{e}");
                if set_definitely_not_applied(&e) {
                    self.roll_back_last_set(device_path, &image_str, previous_last_set);
                }
                SetOutcome::SetFailed(e)
            }
        }
    }

    /// 設定**確定失敗**之後把這台的 `last_set` 退回設定前的值並存檔（task 6.1 修正輪 2，複審 L2 殘餘）：
    /// 螢幕上仍是設定前的桌布，等於沒設定過——全域純色原桌布時，之後 explorer 換上的記憶圖片才會照「還沒設定過」
    /// 判成過渡讀回，不會誤讓位。
    ///
    /// 不違反「先寫後設」：設定呼叫的當下狀態檔已是新值；這裡只在設定已確定沒有發生之後才改回。萬一判斷錯、圖
    /// 其實套上了，讀回也是輸出資料夾內的檔，仍判為宿主的圖（`is_host_output_file`）。a／b 選格以讀回為準，
    /// 讀不到才退回 `last_set`——退回後的值正是螢幕上還在的那一格。只寫狀態檔、不清孤兒備份（備份引用不變）。
    fn roll_back_last_set(&mut self, device_path: &str, attempted: &str, previous: Option<String>) {
        let Loaded::Ready(state) = &self.state else {
            return;
        };
        let mut next = state.as_ref().clone();
        let Some(rec) = next.monitors.values_mut().find(|r| {
            path_eq(&r.device_path, device_path) && r.last_set.as_deref() == Some(attempted)
        }) else {
            return;
        };
        rec.last_set = previous.clone();
        match self.persist_marker_only(&next) {
            Ok(()) => log::info!(
                "設定 {device_path} 確定失敗：last_set 退回設定前的值 {previous:?}（螢幕上仍是原本那張）"
            ),
            Err(e) => log::warn!(
                "設定 {device_path} 確定失敗，last_set 退回 {previous:?}，但寫入狀態檔失敗（記憶體中已退回）：{e}"
            ),
        }
        self.state = Loaded::Ready(Box::new(next));
    }

    /// 原桌布無法備份（task 6.4，審查 R1-M3a）：什麼都不設定、不寫狀態檔，進入 Stopped（同一次重畫的
    /// 其餘螢幕被擋下；使用者重新選主題才再試），回傳「主題改不接管＋通知」事件。已接管時的還原由
    /// 呼叫端負責（[`SetOutcome::BackupFailed`]）。
    fn backup_failed(&mut self, detail: String) -> SetOutcome {
        log::error!("{detail}：為避免遺失原桌布，不接管這次的設定，主題改為「不接管」");
        self.phase = Phase::Stopped;
        SetOutcome::BackupFailed {
            events: vec![
                TakeoverEvent::ThemeSetToNone {
                    cause: ThemeNoneCause::BackupFailed,
                },
                TakeoverEvent::Notify(TakeoverNotice::BackupFailed { detail }),
            ],
        }
    }

    /// 讓位（brief 裁決 4＋修正輪 1 裁決 3）：
    ///
    /// - 使用者換掉的螢幕不動，新桌布記為它的原桌布；
    /// - 其他讀回仍是宿主圖的在線螢幕，逐螢幕還原為各自的原桌布（不動全域填滿方式、背景色，
    ///   **不寫登錄**——使用者已自行選擇）；設定或讀回驗證失敗者標為待還原；
    /// - 離線而 `last_set` 有值（可能仍是宿主的圖）者標為待還原；
    /// - 狀態改未接管、進入 Stopped、排程器 `reset` 一次。
    fn yield_to_user(
        &mut self,
        io: &mut TakeoverIo<'_>,
        scheduler: &mut SchedulerState,
        mut next: StateFile,
        changed: Vec<(String, String)>,
        snapshot: &WallpaperSnapshot,
    ) -> Vec<TakeoverEvent> {
        let now = io.now;
        let changed_keys: Vec<String> = changed.iter().map(|(k, _)| k.clone()).collect();
        let mut warnings = Vec::new();
        let registry_wallpaper = next.original.as_ref().map(|o| o.registry.wallpaper.clone());

        // 第一趟（修正輪 5 裁決 2）：以尚未採納任何變更的紀錄，算出每台要怎麼處理——與 `restore_now`
        // 一致，先處理的螢幕不會改變後面螢幕「共同原圖」的判定。
        let mut actions: Vec<(String, YieldAction)> = Vec::new();
        {
            let guard = BackupGuard {
                paths: &self.paths,
                states: vec![&next.monitors],
                in_use: &self.seen_wallpapers,
                files: io.files,
            };
            for (key, rec) in next
                .monitors
                .iter()
                .filter(|(k, _)| !changed_keys.contains(k))
            {
                let action = match online_wallpaper(snapshot, &rec.device_path) {
                    // 桌布快取資料夾的讀回（task 6.4）不是使用者的選擇，可能就是宿主的圖：照原紀錄還原。
                    Some(cur)
                        if shows_host_image(rec, cur, &self.paths, io.files)
                            || (!rec.stale && self.paths.is_themes_cache(cur)) =>
                    {
                        YieldAction::Restore(restore_want(
                            key,
                            rec,
                            cur,
                            &next.monitors,
                            registry_wallpaper.as_ref(),
                            &guard,
                            &mut warnings,
                        ))
                    }
                    Some(_) if rec.stale => continue,
                    // 仍是原桌布（還沒套用過宿主的圖）：不必還原。
                    Some(_) => YieldAction::ClearMarks,
                    None => YieldAction::MarkPendingIfSet,
                };
                actions.push((key.clone(), action));
            }
        }

        // 沒有紀錄、卻正顯示宿主圖的在線螢幕（task 6.4，審查 R1-M2）：補紀錄，還原成全域原桌布。
        let unrecorded = self.adopt_unrecorded_host_monitors(
            &mut next,
            registry_wallpaper.as_ref(),
            snapshot,
            io,
            &mut warnings,
        );

        // 第二趟：採納（使用者自換者的新選擇、fallback 的複本），再逐螢幕還原。被取代的舊備份留到
        // 下一次接管開始（審查 R1-M4）。
        let mut device_paths = Vec::new();
        for (key, readback) in changed {
            if let Some(rec) = next.monitors.get_mut(&key) {
                device_paths.push(rec.device_path.clone());
                supersede_backup(&mut next.superseded_backups, rec);
                adopt_user_choice(rec, readback, now);
            }
        }
        let mut targets: Vec<(String, String, String)> = unrecorded
            .into_iter()
            .map(|u| (u.key, u.device, u.want_path))
            .collect();
        for (key, action) in actions {
            let Some(rec) = next.monitors.get_mut(&key) else {
                continue;
            };
            match action {
                YieldAction::Restore(want) => {
                    adopt_own_copy(rec, &want, now);
                    targets.push((key, rec.device_path.clone(), want.path));
                }
                YieldAction::ClearMarks => clear_host_marks(rec),
                YieldAction::MarkPendingIfSet => {
                    if rec.last_set.is_some() {
                        rec.pending_restore = true;
                    }
                }
            }
        }
        let wanted: Vec<(String, String)> = targets
            .iter()
            .map(|(k, _, want)| (k.clone(), want.clone()))
            .collect();
        for (key, result) in self.restore_monitors(io, &targets) {
            if let Some(rec) = next.monitors.get_mut(&key) {
                let want_path = wanted
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, w)| w.as_str());
                let want_solid = want_path.is_some_and(str::is_empty);
                let want = match want_path {
                    Some("") => "純色",
                    Some(w) => w,
                    None => "?",
                };
                match result {
                    // task 6.1 修正（low）：成功也記一行（與失敗同格式），每個桌布 API 呼叫的結果都查得到。
                    // 修正輪 4（複審 N1）：要還原成純色時，逐螢幕設空字串**一定**回不到純色（純色是全域狀態，memory
                    // setwallpaper-side-effects）——不看讀回，一律如實記錄、不寫「成功：純色」。實機（6.1 重跑 2b）設定後
                    // 第一次讀回仍是空字串、數秒後才換成記憶中的圖片，所以讀回值只附上當參考。
                    Ok(readback) if want_solid => {
                        let now_reads = if readback.is_empty() {
                            "目前讀回空字串，Windows 稍後會換成記憶中的圖片".to_owned()
                        } else {
                            format!("目前讀回 {readback}")
                        };
                        log::warn!(
                            "讓位時還原 {}：原為純色，無法單獨還原（純色是全域狀態，逐螢幕設空字串只會換回 explorer 記憶中的圖片）——該螢幕會顯示 explorer 記憶中的圖片（{now_reads}）",
                            rec.device_path
                        );
                        clear_host_marks(rec);
                    }
                    Ok(readback) => {
                        log::info!(
                            "讓位時還原 {} 成功：{want}（讀回 {readback}）",
                            rec.device_path
                        );
                        clear_host_marks(rec);
                    }
                    Err(e) => {
                        log::warn!(
                            "讓位時還原 {} 失敗，標為待還原：{want}（{e}）",
                            rec.device_path
                        );
                        rec.pending_restore = true;
                    }
                }
            }
        }
        for w in &warnings {
            log::warn!("讓位時還原其他螢幕：{w}");
        }

        next.status = TakeoverStatus::NotTakenOver;
        next.original = None;
        next.explorer_baseline = None;
        next.last_event = Some(LastEvent::Yielded {
            at: now,
            monitors: device_paths.clone(),
        });
        if let Err(e) = self.persist(&next, io.files) {
            // 仍然讓位（不得覆蓋使用者的桌布）。磁碟上仍是接管中；之後的還原會先做讓位判定，
            // 不會把使用者的桌布蓋回舊圖（修正輪 2 裁決 1）。
            log::error!("讓位後寫入狀態檔失敗：{e}");
        }
        self.state = Loaded::Ready(Box::new(next));
        self.phase = Phase::Stopped;
        scheduler.reset();
        log::warn!("使用者自行更換了桌布（{device_paths:?}）：讓位，主題改為「不接管」");
        vec![
            TakeoverEvent::ThemeSetToNone {
                cause: ThemeNoneCause::UserChangedWallpaper,
            },
            TakeoverEvent::Notify(TakeoverNotice::Yielded { device_paths }),
        ]
    }

    /// 還原原桌布（不接管、系統匣結束、安全閥、`--restore-wallpaper`）。規則見模組文件「還原」；
    /// 之後一律呼叫 `scheduler.reset()`（排隊時除外）。
    pub fn restore(
        &mut self,
        io: &mut TakeoverIo<'_>,
        scheduler: &mut SchedulerState,
        reason: RestoreReason,
    ) -> RestoreOutcome {
        if self.redraw_in_progress {
            // 安全閥的原因不被一般原因取代（它帶著要通知使用者的事件）。
            let keep_old = matches!(
                self.pending_restore,
                Some(RestoreReason::SafetyValve { .. })
            ) && !matches!(reason, RestoreReason::SafetyValve { .. });
            if !keep_old {
                self.pending_restore = Some(reason);
            }
            log::info!("Redraw 進行中，還原排隊到它結束之後");
            return RestoreOutcome {
                result: RestoreResult::Queued,
                events: Vec::new(),
            };
        }
        self.pending_restore = None;
        let events = match &reason {
            RestoreReason::SafetyValve { detail } => {
                self.phase = Phase::Stopped;
                vec![
                    TakeoverEvent::ThemeSetToNone {
                        cause: ThemeNoneCause::SafetyValve,
                    },
                    TakeoverEvent::Notify(TakeoverNotice::SafetyValve {
                        detail: detail.clone(),
                        restore: ValveRestore::Restored,
                    }),
                ]
            }
            _ => Vec::new(),
        };
        self.refresh(io.now);
        if matches!(self.state, Loaded::Unavailable(_)) && reason.is_final() {
            thread::sleep(self.config.transient_retry_delay);
            self.refresh(io.now);
        }
        // 續做（task 6.4，審查 R1-M1）：狀態檔原本就有標記（上一次還原中斷、或驗證失敗後重試）。外部
        // 要求只寫磁碟、不進記憶體，所以系統匣結束與命令列交接的第一次還原不算續做。
        let resuming = self.restore_marker().is_some();
        self.mark_restore_started(io, &reason);
        let result = self.restore_now(io, &reason, resuming);
        scheduler.reset();
        // 通知文字依這次還原的結果（4.8 ledger：未接管時是 NothingToRestore，不可寫「已還原」）。
        let valve = ValveRestore::from_result(&result);
        let events = events
            .into_iter()
            .map(|e| match e {
                TakeoverEvent::Notify(TakeoverNotice::SafetyValve { detail, .. }) => {
                    TakeoverEvent::Notify(TakeoverNotice::SafetyValve {
                        detail,
                        restore: valve,
                    })
                }
                other => other,
            })
            .collect();
        RestoreOutcome { result, events }
    }

    /// 還原進行中標記（task 4.7b）：`None`＝沒有。
    pub fn restore_marker(&self) -> Option<&RestoreMarker> {
        self.state()?.restore_in_progress.as_ref()
    }

    /// 接管中開始還原前寫入「還原進行中」標記（task 4.7b）：已有同一個原因的標記（續做）時不重寫、
    /// 保留原本的開始時刻；原因不同（例如「不接管」的還原失敗後又從系統匣結束）時改成新的原因。
    /// 寫入失敗只記錯誤、照常還原——還原成功時本來就會寫「未接管」，標記只是給中斷的情況用。
    fn mark_restore_started(&mut self, io: &TakeoverIo<'_>, reason: &RestoreReason) {
        let Loaded::Ready(state) = &self.state else {
            return;
        };
        if state.status != TakeoverStatus::TakenOver
            || state
                .restore_in_progress
                .as_ref()
                .is_some_and(|m| m.same_reason(reason))
        {
            return;
        }
        let mut next = state.as_ref().clone();
        next.restore_in_progress = Some(RestoreMarker::new(reason, io.now));
        match self.persist_marker_only(&next) {
            Ok(()) => log::info!("已寫入「還原進行中」標記（{}）", reason.label()),
            Err(e) => log::error!("寫入「還原進行中」標記失敗（照常還原）：{e}"),
        }
        self.state = Loaded::Ready(Box::new(next));
    }

    /// 使用者放棄進行中的還原（task 4.7b：「不接管」的還原失敗後又選回某個主題）：清除標記並
    /// 存檔，其餘不動（仍是接管中、原桌布記錄保留）。回傳是否真的清除了。
    pub fn abandon_restore_marker(&mut self) -> io::Result<bool> {
        let Loaded::Ready(state) = &self.state else {
            return Ok(false);
        };
        if state.restore_in_progress.is_none() {
            return Ok(false);
        }
        let mut next = state.as_ref().clone();
        next.restore_in_progress = None;
        let saved = self.persist_marker_only(&next);
        self.state = Loaded::Ready(Box::new(next));
        saved.map(|()| true)
    }

    /// 安全閥判定用的基準（task 4.9）：**接管中**才回傳狀態檔的基準；未接管（接管開始前）一律
    /// `None`——接管開始時以當下讀值重設，不沿用上一次接管的值。封鎖或暫時讀不到時 `None`。
    pub fn valve_baseline(&self) -> Option<ExplorerSample> {
        self.state()
            .filter(|s| s.status == TakeoverStatus::TakenOver)
            .and_then(|s| s.explorer_baseline)
            .map(|b| b.sample())
    }

    /// 寫入新基準（task 4.9；排程器的 `Decision::rebaseline`）。只寫狀態檔、不清孤兒備份；寫入失敗
    /// 時記憶體中仍更新（回 `Err`，呼叫端記錄）。回傳是否更新（狀態檔封鎖或暫時讀不到時 `false`）。
    pub fn set_explorer_baseline(&mut self, sample: ExplorerSample, now: i64) -> io::Result<bool> {
        let Loaded::Ready(state) = &self.state else {
            return Ok(false);
        };
        let mut next = state.as_ref().clone();
        next.explorer_baseline = Some(ExplorerBaseline {
            pid: sample.pid,
            gdi: sample.gdi,
            recorded_at: now,
        });
        let saved = self.persist_marker_only(&next);
        self.state = Loaded::Ready(Box::new(next));
        saved.map(|()| true)
    }

    /// 清掉基準並存檔（task 4.9：使用者重新開啟接管）。回傳是否真的清除了。
    pub fn clear_explorer_baseline(&mut self) -> io::Result<bool> {
        let Loaded::Ready(state) = &self.state else {
            return Ok(false);
        };
        if state.explorer_baseline.is_none() {
            return Ok(false);
        }
        let mut next = state.as_ref().clone();
        next.explorer_baseline = None;
        let saved = self.persist_marker_only(&next);
        self.state = Loaded::Ready(Box::new(next));
        saved.map(|()| true)
    }

    fn restore_now(
        &mut self,
        io: &mut TakeoverIo<'_>,
        reason: &RestoreReason,
        resuming: bool,
    ) -> RestoreResult {
        let state = match &self.state {
            Loaded::Blocked(r) => {
                log::error!("狀態檔無法使用（{r:?}），無法還原原桌布");
                return RestoreResult::Blocked;
            }
            Loaded::Unavailable(e) => {
                log::error!("狀態檔暫時讀不到（{e}），本次無法還原原桌布");
                return RestoreResult::StateUnavailable;
            }
            Loaded::Ready(s) => s.as_ref().clone(),
        };
        if state.status == TakeoverStatus::NotTakenOver {
            return self.restore_pending_only(io, &state);
        }
        let mut state = state;
        let Some(original) = state.original.clone() else {
            return RestoreResult::Failed {
                problems: vec!["狀態檔為接管中卻沒有原桌布記錄".to_owned()],
            };
        };

        // 1. 先讀：不知道螢幕上是什麼就什麼都不做（不設定、不寫登錄），狀態保留供下次重試。
        let snap = match io.wallpaper.read() {
            Ok(s) => s,
            Err(e) => {
                log::error!("還原前讀取目前桌布失敗，本次不還原：{e}");
                return RestoreResult::Failed {
                    problems: vec![format!("還原前讀取目前桌布失敗：{e}")],
                };
            }
        };
        self.remember_seen(&snap);
        let slideshow_resume =
            resuming && original.slideshow.is_some() && slideshow_applicable(snap.slideshow_status);
        if slideshow_resume {
            log::info!(
                "續做還原：原桌布是投影片且目前正在播放，投影片來源資料夾內的讀回視為原桌布"
            );
        }

        // 2. 逐台分類：宿主的圖→還原；已是原桌布→不動；其他→使用者的選擇，不動。沒有紀錄卻正顯示
        //    宿主圖的在線螢幕（task 6.4，審查 R1-M2）在各紀錄算完之後才補紀錄（與讓位一致：補上的紀錄
        //    不參與原桌布未知者的後備判定），還原成全域原桌布。
        let mut warnings = Vec::new();
        let mut plan: Vec<PlanItem> = Vec::new();
        let mut offline = Vec::new();
        let mut unknown_online = Vec::new();
        for (key, rec) in &state.monitors {
            let current = match monitor_readback(&snap, &rec.device_path) {
                MonitorRead::Offline => {
                    offline.push(key.clone());
                    continue;
                }
                MonitorRead::Unknown => {
                    // 在線但讀不到：不知道上面是什麼，視同使用者的選擇來擋登錄與全域設定，
                    // 本身不設定、標為待還原（修正輪 3 裁決 2）。
                    unknown_online.push(key.clone());
                    continue;
                }
                MonitorRead::Wallpaper(w) => w,
            };
            if rec.stale && !shows_host_image(rec, current, &self.paths, io.files) {
                // 沿用自前一次接管、這次從未碰過：不是宿主的圖就不動，也不擋登錄。
                continue;
            }
            let want = restore_want(
                key,
                rec,
                current,
                &state.monitors,
                Some(&original.registry.wallpaper),
                &BackupGuard {
                    paths: &self.paths,
                    states: vec![&state.monitors],
                    in_use: &self.seen_wallpapers,
                    files: io.files,
                },
                &mut warnings,
            );
            let class = classify_for_restore(
                rec,
                Some(&original),
                current,
                &want.path,
                &self.paths,
                io.files,
                slideshow_resume,
            );
            plan.push(PlanItem {
                key: key.clone(),
                device: rec.device_path.clone(),
                current: current.to_owned(),
                want,
                class,
            });
        }
        let unrecorded = self.adopt_unrecorded_host_monitors(
            &mut state,
            Some(&original.registry.wallpaper),
            &snap,
            io,
            &mut warnings,
        );
        plan.extend(unrecorded.into_iter().map(|u| PlanItem {
            key: u.key,
            device: u.device,
            current: u.current,
            want: Want {
                path: u.want_path,
                own_copy: None,
            },
            class: RestoreClass::Host,
        }));
        let user_chose =
            plan.iter().any(|p| p.class == RestoreClass::UserChoice) || !unknown_online.is_empty();
        let slideshow = original.slideshow.as_ref().map(PersistedSlideshow::to_info);
        // 投影片是全域設定：只有沒有任何使用者選擇時才整體還原投影片。
        let slideshow_global = slideshow.is_some() && !user_chose;
        // task 6.1 修正（Bug 1／B9）：純色也是全域設定——逐螢幕設空字串只會換回 explorer 記憶中的圖片，只有
        // `SetWallpaper(NULL, "")` 才回到純色。沒有任何使用者選擇時，所有螢幕一律還原成全域純色（含接管期間
        // 才接上、另記了原桌布的螢幕：接管前它在純色模式下同樣是純色）。
        let solid_global = original.solid_color && !slideshow_global && !user_chose;
        if solid_global {
            for item in plan.iter_mut().filter(|p| p.class == RestoreClass::Host) {
                item.want = Want {
                    path: String::new(),
                    own_copy: None,
                };
            }
        }
        // 4.x 實機 4.4：原桌布是「全部螢幕」設定時，一次 `SetWallpaper(NULL, 原圖)` 還原（逐螢幕設定會讓
        // `GetWallpaper(NULL)` 變成空字串、不是原狀態）。條件不全成立就維持逐螢幕還原。
        let all_global_path = (original.all_monitors
            && !solid_global
            && !slideshow_global
            && !user_chose
            && unknown_online.is_empty())
        .then(|| restore_all_monitors_path(&plan, &snap))
        .flatten();
        let registry_snapshot = original.registry.to_snapshot();
        let mut problems = Vec::new();

        // 3. 逐螢幕設定（記下設定前的登錄 Wallpaper，用來等 explorer 處理）。
        let registry_before = io.registry.get(DESKTOP_KEY, DESKTOP_VALUE_NAMES[0]).ok();
        let mut any_set = false;
        if solid_global {
            let showing_image = snap
                .monitors
                .iter()
                .filter(|m| m.monitor.is_online())
                .any(|m| m.wallpaper.as_deref() != Some(""));
            if showing_image {
                any_set = true;
                match io.wallpaper.set_solid_color() {
                    Ok(()) => log::info!(
                        "還原全域純色：SetWallpaper(NULL, \"\") 已被接受（背景色 {:#08x} 接著寫回）",
                        original.background_color
                    ),
                    Err(e) => problems.push(format!("還原全域純色失敗：{e}")),
                }
            }
        } else if let Some(path) = all_global_path.as_deref() {
            any_set = true;
            match io.wallpaper.set_wallpaper_all(Path::new(path)) {
                Ok(()) => log::info!(
                    "還原「全部螢幕」設定：SetWallpaper(NULL, {path}) 已被接受（登錄接著寫回）"
                ),
                Err(e) => problems.push(format!("還原「全部螢幕」設定的桌布失敗：{e}")),
            }
        } else if !slideshow_global {
            if original.solid_color && plan.iter().any(|p| p.class == RestoreClass::Host) {
                log::warn!(
                    "原桌布是全域純色，但有螢幕是使用者自選的桌布：不能用全域純色（會蓋掉使用者的選擇），其餘螢幕逐螢幕設空字串——Windows 會顯示它記憶中的圖片，無法只讓那幾台回到純色"
                );
            }
            for item in plan.iter().filter(|p| p.class == RestoreClass::Host) {
                if path_eq(&item.current, &item.want.path) {
                    continue;
                }
                any_set = true;
                if let Err(e) = io
                    .wallpaper
                    .set_wallpaper(&item.device, Path::new(&item.want.path))
                {
                    problems.push(format!("還原 {} 的桌布失敗：{e}", item.device));
                }
            }
        }
        // 4. 全域設定與登錄：只有每台在線螢幕都是宿主的圖或已是原桌布時才動（4.2 契約：使用者
        //    已自行選擇時不得寫回登錄）。
        if !user_chose {
            if snap.background_color != original.background_color {
                if let Err(e) = io.wallpaper.set_background_color(original.background_color) {
                    problems.push(format!("還原背景色失敗：{e}"));
                }
            }
            if let Some(info) = slideshow.as_ref().filter(|_| slideshow_global) {
                if !slideshow_matches(&snap, info) {
                    any_set = true;
                    if let Err(e) = io.wallpaper.restore_slideshow(info) {
                        problems.push(format!("還原投影片失敗：{e}"));
                    }
                }
            }
            if snap.position.0 != original.position {
                if let Err(e) = io
                    .wallpaper
                    .set_position(WallpaperPosition(original.position))
                {
                    problems.push(format!("還原填滿方式失敗：{e}"));
                }
            }
            // explorer 非同步處理 SetWallpaper，處理時會改寫登錄：先等它處理完再寫回。
            if any_set {
                self.wait_for_registry_settle(io, registry_before.as_ref());
            }
            if let Err(e) = registry::restore_desktop_registry_in(io.registry, &registry_snapshot) {
                problems.push(e.to_string());
            }
        } else {
            log::warn!("還原時發現使用者已自行換了桌布：那些螢幕不動，整次不寫登錄、不動全域設定");
        }

        // 5. 讀回驗證（失敗時隔一段時間再驗一次）。
        let to_verify: Vec<(String, String)> = plan
            .iter()
            .filter(|p| p.class == RestoreClass::Host)
            .map(|p| (p.device.clone(), p.want.path.clone()))
            .collect();
        let attempts = self.config.verify_attempts.max(1);
        let mut verified = None;
        let mut mismatches = Vec::new();
        for attempt in 0..attempts {
            if attempt > 0 {
                thread::sleep(self.config.verify_interval);
            }
            match verify_restored(
                io,
                &to_verify,
                (!user_chose).then_some(&original),
                slideshow.as_ref().filter(|_| slideshow_global),
                (!user_chose).then_some(&registry_snapshot),
                // 全域純色還原要求每台都讀回空字串；逐螢幕設空字串時（使用者自選了別台）讀回是 explorer
                // 記憶中的圖片，不再是宿主的圖就算還原了。
                (!solid_global).then_some(&self.paths),
            ) {
                Ok(gone) => {
                    verified = Some(gone);
                    break;
                }
                Err(m) => mismatches = m,
            }
        }
        let Some(went_offline) = verified else {
            problems.extend(mismatches);
            log::error!("還原原桌布後讀回驗證失敗，保留狀態檔供下次重試：{problems:?}");
            return RestoreResult::Failed { problems };
        };
        warnings.extend(problems);

        // 6. 寫回狀態：未接管；離線者待還原；使用者選擇者記為新的原桌布。
        self.seen_wallpapers.extend(
            plan.iter()
                .filter(|p| p.class == RestoreClass::Host && !p.want.path.is_empty())
                .map(|p| p.want.path.clone()),
        );
        let mut next = state;
        let mut offline_unverified = Vec::new();
        let mut user_choice_kept = Vec::new();
        for item in &plan {
            let Some(rec) = next.monitors.get_mut(&item.key) else {
                continue;
            };
            if item.class != RestoreClass::UserChoice {
                adopt_own_copy(rec, &item.want, io.now);
            }
            if item.class == RestoreClass::UserChoice {
                user_choice_kept.push(rec.device_path.clone());
                supersede_backup(&mut next.superseded_backups, rec);
                adopt_user_choice(rec, item.current.clone(), io.now);
            } else if went_offline.iter().any(|d| path_eq(d, &item.device)) {
                rec.pending_restore = rec.last_set.is_some();
                offline_unverified.push(rec.device_path.clone());
            } else {
                if item.class == RestoreClass::Host {
                    let want = if item.want.path.is_empty() {
                        "純色"
                    } else {
                        item.want.path.as_str()
                    };
                    log::info!("還原 {} 成功：{want}（讀回驗證通過）", item.device);
                }
                clear_host_marks(rec);
            }
        }
        for key in &offline {
            if let Some(rec) = next.monitors.get_mut(key) {
                // 離線而未還原：重新接上時螢幕上仍可能是宿主的圖。保留原桌布紀錄與 last_set、
                // 標為待還原（修正輪 1 裁決 4）；從未設定過的螢幕本來就是原桌布，不必標。
                rec.pending_restore = rec.last_set.is_some();
                if rec.pending_restore {
                    offline_unverified.push(rec.device_path.clone());
                }
            }
        }
        for key in &unknown_online {
            if let Some(rec) = next.monitors.get_mut(key) {
                rec.pending_restore = true;
                offline_unverified.push(rec.device_path.clone());
            }
        }
        if !user_choice_kept.is_empty() {
            warnings.push(format!(
                "使用者已自行換了桌布，不覆蓋、不寫登錄：{user_choice_kept:?}"
            ));
        }
        next.status = TakeoverStatus::NotTakenOver;
        next.original = None;
        next.restore_in_progress = None;
        next.explorer_baseline = None;
        next.last_event = Some(LastEvent::Restored {
            at: io.now,
            reason: reason.label(),
            offline_unverified: offline_unverified.clone(),
            warnings: warnings.clone(),
        });
        let state_saved = match self.persist(&next, io.files) {
            Ok(()) => true,
            Err(e) => {
                log::error!("還原成功，但寫入狀態檔失敗（下次啟動會再處理一次）：{e}");
                false
            }
        };
        self.state = Loaded::Ready(Box::new(next));
        log::info!(
            "已還原原桌布（{}）；離線待還原 {offline_unverified:?}",
            reason.label()
        );
        for w in &warnings {
            log::warn!("還原原桌布：{w}");
        }
        RestoreResult::Restored {
            offline_unverified,
            user_choice_kept,
            warnings,
            state_saved,
        }
    }

    /// 未接管時的 `restore`：只處理在線的待還原螢幕（不寫登錄）。
    fn restore_pending_only(
        &mut self,
        io: &mut TakeoverIo<'_>,
        state: &StateFile,
    ) -> RestoreResult {
        if state.restore_in_progress.is_some() {
            // 未接管卻有標記（不該發生：清除標記與改未接管是同一次寫入）：不是還原進行中，清掉。
            match self.abandon_restore_marker() {
                Ok(_) => log::warn!("狀態檔為未接管卻有「還原進行中」標記：已清除"),
                Err(e) => log::error!("清除多餘的「還原進行中」標記失敗：{e}"),
            }
        }
        if !state.monitors.values().any(|r| r.pending_restore) {
            return RestoreResult::NothingToRestore;
        }
        let report = self.restore_pending_monitors(io);
        if !report.failed.is_empty() {
            return RestoreResult::Failed {
                problems: report
                    .failed
                    .into_iter()
                    .map(|(d, e)| format!("{d}：{e}"))
                    .collect(),
            };
        }
        RestoreResult::Restored {
            offline_unverified: report.still_offline,
            user_choice_kept: Vec::new(),
            warnings: report
                .left_showing_host
                .iter()
                .map(|d| {
                    format!("{d} 仍顯示宿主的圖：原桌布是純色、其他螢幕已不是純色，無法只還原這台，請自行更換")
                })
                .collect(),
            state_saved: report.state_saved,
        }
    }

    /// 處理待還原的螢幕（修正輪 1 裁決 4；4.7 在螢幕接上、顯示組態變更時呼叫，`restore` 在未接管
    /// 時也會呼叫）：在線且讀回仍是宿主的圖 → 逐螢幕還原並驗證後清除標記；讀回是其他圖 → 只清除
    /// 標記、不覆蓋（是使用者的選擇時記為新的原桌布）；仍離線 → 維持；讀取失敗 → 維持。不寫登錄、
    /// 不動全域設定。接管中、封鎖或暫時讀不到時什麼都不做。
    ///
    /// 原桌布是純色的螢幕（task 6.1 修正輪 1，審查 L4 裁定）：純色是全域狀態，**不得**對它單獨設空字串（只會
    /// 換回 explorer 記憶中的圖片，還可能把其他螢幕帶出純色）。其餘在線螢幕都仍讀回空字串（桌面仍是純色）時，
    /// 再呼叫一次 `SetWallpaper(NULL, "")` 並驗證這些螢幕都讀回空字串；否則（使用者已改選圖片）不動它，記一行並
    /// 清掉待還原。
    pub fn restore_pending_monitors(&mut self, io: &mut TakeoverIo<'_>) -> PendingReport {
        self.refresh(io.now);
        let mut report = PendingReport::default();
        let mut next = match &self.state {
            Loaded::Ready(s) if s.status == TakeoverStatus::NotTakenOver => s.as_ref().clone(),
            _ => return report,
        };
        if !next.monitors.values().any(|r| r.pending_restore) {
            return report;
        }
        let snapshot = match io.wallpaper.read() {
            Ok(s) => {
                self.remember_seen(&s);
                s
            }
            Err(e) => {
                let reason = format!("讀取目前桌布失敗：{e}");
                report.failed = next
                    .monitors
                    .values()
                    .filter(|r| r.pending_restore)
                    .map(|r| (r.device_path.clone(), reason.clone()))
                    .collect();
                log::warn!("處理待還原螢幕：{reason}");
                return report;
            }
        };
        let mut warnings = Vec::new();
        // 第一趟（修正輪 5 裁決 2）：以尚未採納任何變更的紀錄，算出每台待還原螢幕要還原成什麼並
        // 分類——與 `restore_now` 一致，先處理的螢幕不會改變後面螢幕「共同原圖」的判定。
        let mut planned: Vec<(String, String, String, Want, RestoreClass)> = Vec::new();
        {
            let guard = BackupGuard {
                paths: &self.paths,
                states: vec![&next.monitors],
                in_use: &self.seen_wallpapers,
                files: io.files,
            };
            for (key, rec) in next.monitors.iter().filter(|(_, r)| r.pending_restore) {
                let device = rec.device_path.clone();
                let Some(cur) = online_wallpaper(&snapshot, &device) else {
                    report.still_offline.push(device);
                    continue;
                };
                let want = restore_want(key, rec, cur, &next.monitors, None, &guard, &mut warnings);
                let class =
                    classify_for_restore(rec, None, cur, &want.path, &self.paths, io.files, false);
                planned.push((key.clone(), device, cur.to_owned(), want, class));
            }
        }
        // 第二趟：採納，並在還原與 `persist` 之前完成（複本從此有紀錄引用；修正輪 4）。
        let mut targets = Vec::new();
        // （紀錄鍵、裝置路徑、目前讀回）
        let mut solid_targets: Vec<(String, String, String)> = Vec::new();
        for (key, device, cur, want, class) in planned {
            let Some(rec) = next.monitors.get_mut(&key) else {
                continue;
            };
            if class != RestoreClass::UserChoice {
                adopt_own_copy(rec, &want, io.now);
            }
            let solid_original = !rec.original_unknown && rec.original_wallpaper.is_empty();
            match class {
                RestoreClass::Host if solid_original && want.path.is_empty() => {
                    solid_targets.push((key, device, cur));
                }
                RestoreClass::Host => targets.push((key, device, want.path)),
                RestoreClass::UserChoice => {
                    log::info!(
                        "待還原螢幕 {device} 已是其他桌布（{cur}），使用者已自行更換：不覆蓋"
                    );
                    supersede_backup(&mut next.superseded_backups, rec);
                    adopt_user_choice(rec, cur, io.now);
                    report.cleared.push(device);
                }
                RestoreClass::Original => {
                    clear_host_marks(rec);
                    report.cleared.push(device);
                }
            }
        }
        let mut results = self.restore_monitors(io, &targets);
        if !solid_targets.is_empty() {
            let others_solid = snapshot
                .monitors
                .iter()
                .filter(|m| m.monitor.is_online())
                .filter(|m| {
                    !solid_targets
                        .iter()
                        .any(|(_, d, _)| path_eq(d, &m.monitor.device_path))
                })
                .all(|m| m.wallpaper.as_deref() == Some(""));
            if others_solid {
                let group: Vec<(String, String)> = solid_targets
                    .iter()
                    .map(|(k, d, _)| (k.clone(), d.clone()))
                    .collect();
                results.extend(self.restore_solid_group(io, &group));
            } else {
                for (key, device, cur) in &solid_targets {
                    log::warn!(
                        "待還原螢幕 {device} 仍顯示宿主的圖 {cur}：它的原桌布是純色，但其他螢幕已不是純色（使用者改選了圖片），純色是全域狀態、無法只還原這台——不動它、清掉待還原，之後不會再自動還原，請到 Windows 設定自行更換這台的桌布"
                    );
                    if let Some(rec) = next.monitors.get_mut(key) {
                        clear_host_marks(rec);
                    }
                    report.left_showing_host.push(device.clone());
                }
            }
        }
        for (key, result) in results {
            if let Some(rec) = next.monitors.get_mut(&key) {
                match result {
                    Ok(readback) => {
                        log::info!("待還原螢幕 {} 已還原（讀回 {readback}）", rec.device_path);
                        clear_host_marks(rec);
                        report.restored.push(rec.device_path.clone());
                    }
                    Err(e) => report.failed.push((rec.device_path.clone(), e)),
                }
            }
        }
        for w in &warnings {
            log::warn!("還原待還原螢幕：{w}");
        }
        if !report.restored.is_empty()
            || !report.cleared.is_empty()
            || !report.left_showing_host.is_empty()
        {
            if let Err(e) = self.persist(&next, io.files) {
                log::error!("處理待還原螢幕後寫入狀態檔失敗：{e}");
                report.state_saved = false;
            }
            self.state = Loaded::Ready(Box::new(next));
        }
        report
    }

    /// 沒有紀錄、卻正顯示宿主輸出圖的在線螢幕（task 6.4，審查 R1-M2：接管期間接上、宿主還沒設定過它
    /// ——暫停、COM 忙碌、缺資料——Windows 就套上了宿主的圖）。還原與讓位時把它們一起還原，否則回報
    /// 成功、狀態改未接管後再也沒有紀錄，這台會永久停在宿主的圖。
    ///
    /// 每台補一筆紀錄（`last_set`＝目前顯示的宿主圖，之後離線或驗證失敗照常標為待還原），原桌布＝
    /// 「全域原桌布」（[`global_original_want`]），並記一行說明（寫進 `warnings`，結果不是乾淨的成功）。
    fn adopt_unrecorded_host_monitors(
        &self,
        state: &mut StateFile,
        registry_wallpaper: Option<&RegValue>,
        snap: &WallpaperSnapshot,
        io: &TakeoverIo<'_>,
        warnings: &mut Vec<String>,
    ) -> Vec<Unrecorded> {
        let mut found: Vec<Unrecorded> = Vec::new();
        for mw in snap.monitors.iter().filter(|m| m.monitor.is_online()) {
            let device = mw.monitor.device_path.as_str();
            let Some(current) = mw.wallpaper.as_deref() else {
                continue;
            };
            if state
                .monitors
                .values()
                .any(|r| path_eq(&r.device_path, device))
                || found.iter().any(|u| path_eq(&u.device, device))
                || !is_host_output_file(current, &self.paths, io.files)
            {
                continue;
            }
            let preferred = monitor_key(device, mw.monitor.rect, io.stable_displays);
            let key = free_key(
                &|k| state.monitors.contains_key(k) || found.iter().any(|u| u.key == k),
                &preferred,
                device,
            );
            let want = {
                let guard = BackupGuard {
                    paths: &self.paths,
                    states: vec![&state.monitors],
                    in_use: &self.seen_wallpapers,
                    files: io.files,
                };
                global_original_want(
                    &key,
                    device,
                    &state.monitors,
                    registry_wallpaper,
                    &guard,
                    warnings,
                )
            };
            let shown = if want.path.is_empty() {
                "純色".to_owned()
            } else {
                want.path.clone()
            };
            let line = format!(
                "螢幕 {device} 沒有紀錄、卻正顯示宿主的圖 {current}（接管期間接上、宿主還沒設定過它）：還原為全域原桌布 {shown}"
            );
            log::warn!("{line}");
            warnings.push(line);
            found.push(Unrecorded {
                key,
                device: device.to_owned(),
                current: current.to_owned(),
                want_path: want.path,
                own_copy: want.own_copy,
            });
        }
        for u in &found {
            state.monitors.insert(
                u.key.clone(),
                MonitorRecord {
                    device_path: u.device.clone(),
                    original_wallpaper: u.want_path.clone(),
                    backup: u.own_copy.clone(),
                    recorded_at: io.now,
                    last_set: Some(u.current.clone()),
                    host_applied: true,
                    pending_restore: false,
                    original_unknown: false,
                    stale: false,
                    original_file_id: None,
                    solid_transition_readback: None,
                },
            );
        }
        found
    }

    /// 逐螢幕還原（不動全域設定、不寫登錄）：逐台 `SetWallpaper` 後讀回驗證（失敗時隔
    /// `verify_interval` 再驗，共 `verify_attempts` 次）。回傳每台（紀錄鍵）的成敗，成功時帶回讀回值
    /// （要還原成純色時讀回可能是 explorer 記憶中的圖片，見 [`restored_matches`]）。驗證成功的圖
    /// 列入 `seen_wallpapers`（修正輪 4：剛還原上去的檔在接下來的 `persist` 不會被當成孤兒）。
    fn restore_monitors(
        &mut self,
        io: &mut TakeoverIo<'_>,
        targets: &[(String, String, String)],
    ) -> Vec<(String, Result<String, String>)> {
        let mut results: Vec<(String, Result<String, String>)> = Vec::new();
        let mut waiting: Vec<&(String, String, String)> = Vec::new();
        for target in targets {
            let (key, device, want) = target;
            match io.wallpaper.set_wallpaper(device, Path::new(want)) {
                Ok(()) => waiting.push(target),
                Err(e) => results.push((key.clone(), Err(format!("設定失敗：{e}")))),
            }
        }
        let attempts = self.config.verify_attempts.max(1);
        let mut last_error = String::from("讀回不相符");
        for attempt in 0..attempts {
            if waiting.is_empty() {
                break;
            }
            if attempt > 0 {
                thread::sleep(self.config.verify_interval);
            }
            match io.wallpaper.read() {
                Ok(snap) => {
                    let paths = &self.paths;
                    let seen = &mut self.seen_wallpapers;
                    waiting.retain(|(key, device, want)| {
                        let current = online_wallpaper(&snap, device)
                            .filter(|c| restored_matches(c, want, Some(paths), io.files));
                        let Some(current) = current else {
                            return true;
                        };
                        results.push((key.clone(), Ok(current.to_owned())));
                        if !want.is_empty() {
                            seen.push(want.clone());
                        }
                        false
                    });
                    last_error = String::from("讀回不相符");
                }
                Err(e) => last_error = format!("讀回失敗：{e}"),
            }
        }
        for (key, _, _) in waiting {
            results.push((key.clone(), Err(last_error.clone())));
        }
        results
    }

    /// 以一次 `SetWallpaper(NULL, "")` 讓 `targets`（紀錄鍵、裝置路徑）回到純色，再讀回驗證它們都是空字串
    /// （失敗時隔 `verify_interval` 再驗，共 `verify_attempts` 次；task 6.1 修正輪 1，審查 L4）。只在其餘在線
    /// 螢幕都已是純色時呼叫，所以不會改變使用者的選擇。不動背景色、填滿方式與登錄（待還原只還原螢幕本身）。
    fn restore_solid_group(
        &mut self,
        io: &mut TakeoverIo<'_>,
        targets: &[(String, String)],
    ) -> Vec<(String, Result<String, String>)> {
        if let Err(e) = io.wallpaper.set_solid_color() {
            return targets
                .iter()
                .map(|(k, _)| (k.clone(), Err(format!("全域純色設定失敗：{e}"))))
                .collect();
        }
        log::info!(
            "待還原螢幕 {:?} 的原桌布是純色、其餘螢幕仍是純色：已呼叫 SetWallpaper(NULL, \"\")",
            targets.iter().map(|(_, d)| d.as_str()).collect::<Vec<_>>()
        );
        let mut results = Vec::new();
        let mut waiting: Vec<&(String, String)> = targets.iter().collect();
        let mut last_error = String::from("讀回不是純色");
        for attempt in 0..self.config.verify_attempts.max(1) {
            if waiting.is_empty() {
                break;
            }
            if attempt > 0 {
                thread::sleep(self.config.verify_interval);
            }
            match io.wallpaper.read() {
                Ok(snap) => {
                    waiting.retain(|(key, device)| {
                        if online_wallpaper(&snap, device) == Some("") {
                            results.push((key.clone(), Ok(String::new())));
                            false
                        } else {
                            true
                        }
                    });
                    last_error = String::from("讀回不是純色");
                }
                Err(e) => last_error = format!("讀回失敗：{e}"),
            }
        }
        for (key, _) in waiting {
            results.push((key.clone(), Err(last_error.clone())));
        }
        results
    }

    /// 等 explorer 處理完 `SetWallpaper`：輪詢登錄 `Wallpaper`，直到它與設定前的值不同，或滿
    /// `registry_settle_timeout`（原值與 explorer 會寫的值相同時就是等滿上限）。
    fn wait_for_registry_settle(&self, io: &TakeoverIo<'_>, before: Option<&RegValue>) {
        let deadline = Instant::now() + self.config.registry_settle_timeout;
        loop {
            let now_value = io.registry.get(DESKTOP_KEY, DESKTOP_VALUE_NAMES[0]).ok();
            if now_value.as_ref() != before {
                return;
            }
            if Instant::now() >= deadline {
                log::info!("等待 explorer 改寫登錄逾時，照常寫回登錄");
                return;
            }
            thread::sleep(self.config.registry_poll_interval);
        }
    }

    /// 只改「還原進行中」標記時的寫入（task 4.7b）：**不**清孤兒備份。標記在讀回之前寫，此時
    /// `seen_wallpapers` 可能是空的（剛啟動的行程），清理會把螢幕正在顯示、沒有紀錄引用的備份
    /// （例如上一次還原中斷前寫下的複本，修正輪 R）當成孤兒刪掉；標記也不改變任何備份的引用。
    ///
    /// 審查 F6：清除自己的標記（放棄）時，外部要求的標記（系統匣結束、命令列交接）不得一起被清掉
    /// ——與 [`Self::persist`] 相同，要求還在且寫的是接管中、沒有標記的狀態時合併寫入。
    fn persist_marker_only(&self, state: &StateFile) -> io::Result<()> {
        let guard = self.restore_request.as_ref().map(|r| r.lock());
        let requested = guard.as_ref().and_then(|g| g.requested.clone());
        match requested {
            Some(marker)
                if state.status == TakeoverStatus::TakenOver
                    && state.restore_in_progress.is_none() =>
            {
                let mut with_marker = state.clone();
                with_marker.restore_in_progress = Some(marker);
                write_state_json(&self.paths.state_file, &with_marker)
            }
            _ => write_state_json(&self.paths.state_file, state),
        }
    }

    /// 持久寫入狀態檔（[`write_durable`]）；成功後清掉沒有人用的原圖備份（[`remove_orphan_backups`]）。
    ///
    /// 外部要求的標記還在、而這次寫的是接管中且沒有標記的狀態時，把標記一起寫進去（4.7b 修正輪 1）。
    fn persist(&self, state: &StateFile, files: &dyn FileIdentity) -> io::Result<()> {
        {
            let guard = self.restore_request.as_ref().map(|r| r.lock());
            let requested = guard.as_ref().and_then(|g| g.requested.clone());
            match requested {
                Some(marker)
                    if state.status == TakeoverStatus::TakenOver
                        && state.restore_in_progress.is_none() =>
                {
                    let mut with_marker = state.clone();
                    with_marker.restore_in_progress = Some(marker);
                    write_state_json(&self.paths.state_file, &with_marker)?;
                }
                _ => write_state_json(&self.paths.state_file, state)?,
            }
        }
        remove_orphan_backups(&self.paths, state, &self.seen_wallpapers, files);
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// 內部輔助
// ---------------------------------------------------------------------------------------------

/// 讀狀態檔（規則見模組文件「讀檔規則」）。
fn load_state(path: &Path, now: i64) -> Loaded {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Loaded::Ready(Box::new(StateFile::empty()))
        }
        Err(e) => {
            log::warn!(
                "桌布狀態檔 {} 暫時讀不到（之後重讀；不接管、不還原）：{e}",
                path.display()
            );
            return Loaded::Unavailable(e.to_string());
        }
    };
    match parse_state(&bytes) {
        Ok(s) => Loaded::Ready(Box::new(s)),
        Err(reason) => {
            log::error!(
                "桌布狀態檔 {} 無法使用（{reason:?}）：不覆寫原檔、不接管、不還原",
                path.display()
            );
            preserve_corrupt_copy(path, &bytes, now);
            Loaded::Blocked(reason)
        }
    }
}

fn parse_state(bytes: &[u8]) -> Result<StateFile, BlockReason> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|e| BlockReason::Corrupt(e.to_string()))?;
    let version = value.get("version").and_then(serde_json::Value::as_u64);
    if version != Some(u64::from(STATE_VERSION)) {
        return Err(BlockReason::UnknownVersion(version));
    }
    let state: StateFile =
        serde_json::from_value(value).map_err(|e| BlockReason::Corrupt(e.to_string()))?;
    state.validate().map_err(BlockReason::Corrupt)?;
    Ok(state)
}

/// 寫一份內容相同的 `<檔名>.corrupt-<now>`（已有內容相同的副本時略過；不碰原檔）。
fn preserve_corrupt_copy(path: &Path, bytes: &[u8], now: i64) {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| STATE_FILE_NAME.to_owned());
    let prefix = format!("{name}.corrupt-");
    if let Some(dir) = path.parent() {
        if let Ok(entries) = fs::read_dir(dir) {
            let duplicate = entries.flatten().any(|e| {
                e.file_name().to_string_lossy().starts_with(&prefix)
                    && fs::read(e.path()).is_ok_and(|b| b == bytes)
            });
            if duplicate {
                return;
            }
        }
    }
    let copy = path.with_file_name(format!("{prefix}{now}"));
    let written = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&copy)
        .and_then(|mut f| io::Write::write_all(&mut f, bytes));
    match written {
        Ok(()) => log::warn!("已保留損壞的狀態檔副本：{}", copy.display()),
        Err(e) => log::error!("保留損壞的狀態檔副本 {} 失敗：{e}", copy.display()),
    }
}

/// 找既有紀錄（回傳紀錄的鍵）：鍵命中時必須核對裝置路徑；鍵不符（對應表過期、鍵相撞）時改以
/// 裝置路徑找；都找不到＝新螢幕。
fn find_record(state: &StateFile, key: &str, device_path: &str) -> Option<String> {
    if let Some(rec) = state.monitors.get(key) {
        if path_eq(&rec.device_path, device_path) {
            return Some(key.to_owned());
        }
        log::warn!(
            "螢幕鍵 {key} 屬於 {}，與 {device_path} 不符：不沿用該紀錄",
            rec.device_path
        );
    }
    state
        .monitors
        .iter()
        .find(|(_, r)| path_eq(&r.device_path, device_path))
        .map(|(k, _)| k.clone())
}

/// 給新紀錄一個空著的鍵：先用 `preferred`；被佔用（`taken`）時改用裝置路徑算出的鍵；仍被佔用就
/// 加序號。絕不覆寫既有紀錄，也就絕不覆寫另一台螢幕的備份檔（備份檔名＝鍵）。
fn free_key(taken: &dyn Fn(&str) -> bool, preferred: &str, device_path: &str) -> String {
    if !taken(preferred) {
        return preferred.to_owned();
    }
    log::warn!("螢幕鍵 {preferred} 已被其他螢幕使用，{device_path} 改用其他鍵記錄");
    let fallback = monitor_key(device_path, None, &[]);
    if !taken(&fallback) {
        return fallback;
    }
    let mut n = 2u32;
    loop {
        let candidate = format!("{preferred}-{n}");
        if !taken(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// 首次接管的新狀態：記錄全域原狀態與每台在線螢幕。
///
/// 1. 舊狀態中目前**不在線**的紀錄一律保留（修正輪 3 裁決 1：不因離線而丟棄紀錄——它的備份可能
///    正是那台螢幕目前的桌布來源），鍵不變、標為 `stale`。
/// 2. 第一輪：在線螢幕目前是宿主的圖、且舊狀態有它的紀錄（裝置路徑核對過）→ 沿用舊紀錄（真正的
///    原桌布與備份），取回它的舊鍵（修正輪 3 裁決 3：先於任何新紀錄配鍵，舊鍵不會被別台搶走）。
/// 3. 第二輪：其餘在線螢幕以當下的桌布為新的原桌布重新記錄（brief 裁決 5）；當下就是宿主的圖而
///    沒有舊紀錄者，原桌布記為未知（修正輪 2 裁決 4）。配鍵時避開新狀態已用的鍵，以及舊狀態中
///    屬於其他螢幕的鍵——絕不覆寫另一台螢幕的備份檔。同一裝置路徑重複列舉時只記第一次。
fn begin_state(
    paths: &StatePaths,
    previous: &StateFile,
    snapshot: &WallpaperSnapshot,
    desktop_registry: DesktopRegistrySnapshot,
    spotlight: SpotlightState,
    io: &TakeoverIo<'_>,
    in_use: &[String],
) -> Result<StateFile, BeginError> {
    // `snapshot.monitors` 依 `GetMonitorDevicePathAt` 索引排列（第 i 個＝索引 i，含離線者）：逐螢幕轉存檔
    // `Transcoded_<索引>` 以它編號（task 6.1 修正 Bug 2）。
    let indexed: Vec<(usize, &MonitorWallpaper)> = snapshot
        .monitors
        .iter()
        .enumerate()
        .filter(|(_, m)| m.monitor.is_online())
        .collect();
    let online: Vec<&MonitorWallpaper> = indexed.iter().map(|(_, m)| *m).collect();
    // task 6.4（審查 R1-M3a）：原圖複製不了時改備份 explorer 的轉存檔。接管開始時它是使用者目前桌布
    // 的轉存——但有任何在線螢幕正顯示宿主的圖（上一次的待還原螢幕）時，最後一次 `SetWallpaper` 可能
    // 是宿主做的，轉存檔不可信。
    let cache_ok = !online.iter().any(|m| {
        m.wallpaper
            .as_deref()
            .is_some_and(|w| is_host_output_file(w, paths, io.files))
    });
    // 修正輪 4（複審 N2）：轉存檔代表哪張圖取決於接管前的設定方式，以登錄 `Wallpaper` 判定。`desktop_registry`
    // 是呼叫端在這次接管的第一次 `SetWallpaper` 之前取的快照（4.2），之後 explorer 會把它改寫成
    // `TranscodedWallpaper`，所以只能用這一份。
    let method = setting_method(&desktop_registry.wallpaper, &online, paths, io.files);
    let fallback_for = |device: &str| -> Option<TranscodedFallback> {
        if !cache_ok {
            return None;
        }
        indexed
            .iter()
            .find(|(_, m)| path_eq(&m.monitor.device_path, device))
            .map(|(index, _)| TranscodedFallback {
                index: *index,
                method: method.clone(),
            })
    };
    // task 6.1 修正（Bug 1／B9）：純色是全域狀態——所有在線螢幕都讀回空字串＝原桌布是全域純色（色碼是
    // 背景色）。只用 `GetWallpaper`：`GetStatus` 在純色時回 0 只是本機實測，不拿來判定、只記錄。
    let all_empty = !online.is_empty() && online.iter().all(|m| m.wallpaper.as_deref() == Some(""));
    // 修正輪 1（審查 L5）：Windows 焦點啟用中（`EnabledState`＝1）時讀回空字串不算純色，照焦點既有流程
    // （先提醒、還原時無法自動切回焦點）。
    let solid_color = snapshot.slideshow.is_none() && all_empty && !spotlight.is_spotlight();
    if all_empty && spotlight.is_spotlight() {
        log::warn!(
            "所有螢幕讀回空字串，但 Windows 焦點啟用中（EnabledState=1）：不記為全域純色，照焦點流程處理"
        );
    }
    // 4.x 實機 4.4：原桌布是「全部螢幕」設定（登錄＝共同路徑）時記下來，還原才能用 `SetWallpaper(NULL, 圖)` 回到原狀態
    // （`GetWallpaper(NULL)` 才讀得回原圖）。保守條件：純色、投影片、Windows 焦點、有螢幕正顯示宿主的圖（登錄快照
    // 不可信，`cache_ok` 為假）時都不記。
    let all_monitors = method == SettingMethod::AllMonitors
        && cache_ok
        && !solid_color
        && snapshot.slideshow.is_none()
        && !spotlight.is_spotlight();
    if all_monitors {
        log::info!("原桌布是以「全部螢幕」設定的（登錄 Wallpaper＝各螢幕讀回的共同路徑）：還原時以 SetWallpaper(NULL, 原圖) 還原");
    }
    if solid_color {
        log::info!(
            "原桌布是全域純色（背景色 {:#08x}、GetStatus {}）：還原時以 SetWallpaper(NULL, \"\") 還原；接管途中 explorer 換回的記憶圖片不算使用者自換",
            snapshot.background_color,
            snapshot.slideshow_status
        );
    } else if online.iter().any(|m| m.wallpaper.as_deref() == Some("")) {
        log::warn!(
            "部分螢幕讀回空字串（純色）、部分不是：不記為全域純色，那些螢幕還原時只能換回 explorer 記憶中的圖片"
        );
    }
    let mut monitors: BTreeMap<String, MonitorRecord> = BTreeMap::new();
    for (key, rec) in &previous.monitors {
        let is_online = online
            .iter()
            .any(|m| path_eq(&m.monitor.device_path, &rec.device_path));
        if !is_online {
            let mut rec = rec.clone();
            rec.pending_restore = false;
            rec.host_applied = false;
            rec.stale = true;
            monitors.insert(key.clone(), rec);
        }
    }

    // 第一輪：沿用舊紀錄者先取回舊鍵。
    let mut fresh: Vec<(&str, String, &str)> = Vec::new();
    let mut handled: Vec<&str> = Vec::new();
    for mw in online {
        let device = mw.monitor.device_path.as_str();
        if handled.iter().any(|d| path_eq(d, device)) {
            log::warn!("螢幕 {device} 重複列舉，只記錄第一次");
            continue;
        }
        handled.push(device);
        let Some(current) = mw.wallpaper.as_deref() else {
            return Err(BeginError::Record(format!(
                "在線螢幕 {device} 讀不到目前的桌布"
            )));
        };
        let key = monitor_key(device, mw.monitor.rect, io.stable_displays);
        let carried = is_host_output_file(current, paths, io.files)
            .then(|| find_record(previous, &key, device))
            .flatten()
            .and_then(|k| previous.monitors.get(&k).cloned().map(|r| (k, r)));
        match carried {
            Some((old_key, mut rec)) if !monitors.contains_key(&old_key) => {
                log::warn!(
                    "螢幕 {device} 目前是宿主的圖，沿用先前記錄的原桌布 {}",
                    rec.original_wallpaper
                );
                rec.host_applied = false;
                rec.pending_restore = false;
                rec.stale = false;
                rec.solid_transition_readback = None;
                monitors.insert(old_key, rec);
            }
            _ => fresh.push((device, key, current)),
        }
    }

    // 第二輪：新紀錄。
    for (device, key, current) in fresh {
        let key = {
            let taken = |k: &str| {
                monitors.contains_key(k)
                    || previous
                        .monitors
                        .get(k)
                        .is_some_and(|r| !path_eq(&r.device_path, device))
            };
            free_key(&taken, &key, device)
        };
        // 備份不得覆寫其他紀錄引用或正在顯示的檔（修正輪 5）。舊狀態不必另外列入：離線者已以
        // `stale` 保留在新狀態裡，在線者目前顯示的檔都在 `in_use`。
        let guard = BackupGuard {
            paths,
            states: vec![&monitors],
            in_use,
            files: io.files,
        };
        let fallback = fallback_for(device);
        let rec = record_monitor(&guard, &key, device, current, io.now, fallback.as_ref())
            .map_err(BeginError::Backup)?;
        monitors.insert(key, rec);
    }

    Ok(StateFile {
        version: STATE_VERSION,
        status: TakeoverStatus::TakenOver,
        original: Some(OriginalDesktop {
            recorded_at: io.now,
            position: snapshot.position.0,
            background_color: snapshot.background_color,
            slideshow_status: snapshot.slideshow_status,
            slideshow: snapshot
                .slideshow
                .as_ref()
                .map(PersistedSlideshow::from_info),
            registry: PersistedRegistry::from_snapshot(desktop_registry),
            spotlight: spotlight.into(),
            solid_color,
            all_monitors,
        }),
        monitors,
        last_event: previous.last_event.clone(),
        restore_in_progress: None,
        // 接管開始前（未接管時）協調迴圈以這次設定前的讀值寫入的基準＝接管開始時的基準。
        explorer_baseline: previous.explorer_baseline,
        // 接管開始：讓位時留下的舊備份不再保留（下一次寫入清掉沒人用的）。
        superseded_backups: Vec::new(),
    })
}

/// 接管開始（[`begin_state`]）失敗的原因。
#[derive(Debug)]
enum BeginError {
    /// 記錄失敗（在線螢幕讀不到目前的桌布）：`RefuseReason::RecordFailed`。
    Record(String),
    /// 原桌布無法備份（task 6.4，審查 R1-M3a）：不接管、主題改「不接管」並通知。
    Backup(String),
}

/// 寫備份時的保護範圍（修正輪 5）：備份不得覆寫**其他**紀錄引用的檔（`backup` 或
/// `original_wallpaper`，路徑加檔案識別），也不得覆寫某台螢幕目前正顯示的檔（`in_use`）。
struct BackupGuard<'a> {
    paths: &'a StatePaths,
    /// 要納入的紀錄集合（例如接管開始時的新狀態與舊狀態）；與寫入者同鍵的紀錄不算「其他」。
    states: Vec<&'a BTreeMap<String, MonitorRecord>>,
    in_use: &'a [String],
    files: &'a dyn FileIdentity,
}

impl BackupGuard<'_> {
    /// `dest` 是否被 `key` 以外的紀錄引用（`backup` 或 `original_wallpaper`）。
    fn referenced_by_other(&self, key: &str, dest: &str) -> bool {
        let dir = self.paths.backup_dir();
        self.states
            .iter()
            .flat_map(|m| m.iter())
            .filter(|(k, _)| k.as_str() != key)
            .any(|(_, r)| {
                r.backup
                    .as_ref()
                    .is_some_and(|b| same_file(&dir.join(b).to_string_lossy(), dest, self.files))
                    || (!r.original_wallpaper.is_empty()
                        && same_file(&r.original_wallpaper, dest, self.files))
            })
    }

    /// `dest`（備份資料夾內的完整路徑）是否受保護、`key` 這筆紀錄不得覆寫它。
    fn protects(&self, key: &str, dest: &str) -> bool {
        self.referenced_by_other(key, dest)
            || self.in_use.iter().any(|w| same_file(w, dest, self.files))
    }
}

/// 記錄一台螢幕的原桌布並備份原圖（純色不備份）。當下就是宿主的輸出圖時，原桌布記為未知、
/// 不備份（修正輪 2 裁決 4：不得把宿主的圖記成原桌布）。
///
/// 原圖複製不了（接管前就被刪除或搬走——explorer 仍以快取顯示它——或權限、磁碟問題；task 6.4，審查
/// R1-M3a）：有 `fallback`（接管開始時）就改備份 explorer 的轉存檔——依接管前的設定方式選來源（[`SettingMethod`]：
/// 「全部螢幕」設定用全域 `TranscodedWallpaper`、逐螢幕設定用這台的 `Transcoded_<索引>`、判斷不出就不用），見
/// [`backup_transcoded`]（task 6.1 修正 Bug 2、修正輪 4）；仍不行、
/// 或沒有 `fallback`（接管中新接上的螢幕：轉存檔此時已是宿主的圖）就回 `Err`——呼叫端不接管這台。原路徑
/// 照樣記錄（原圖之後回來時用它）。
fn record_monitor(
    guard: &BackupGuard<'_>,
    key: &str,
    device_path: &str,
    wallpaper: &str,
    now: i64,
    fallback: Option<&TranscodedFallback>,
) -> Result<MonitorRecord, String> {
    let cache_ok = fallback.is_some();
    // 修正第 2 輪（複審 N2）：接管中（`cache_ok` 為假）讀回本身就是 explorer 的轉存檔／快取檔時，它的內容
    // 已是宿主的圖——比照宿主輸出圖，原桌布記為未知、不備份，還原時走原桌布未知的後備。
    let cached_host = !cache_ok && guard.paths.is_themes_cache(wallpaper);
    let unknown = cached_host || is_host_output_file(wallpaper, guard.paths, guard.files);
    if cached_host {
        log::warn!(
            "螢幕 {device_path} 記錄當下讀回的是 Windows 的桌布轉存檔 {wallpaper}（接管中內容已是宿主的圖）：原桌布記為未知、不備份，還原時改用其他螢幕的原桌布或登錄記錄的原值"
        );
    } else if unknown {
        log::warn!(
            "螢幕 {device_path} 記錄當下的桌布是宿主的輸出圖：原桌布記為未知，還原時改用其他螢幕的原桌布或登錄記錄的原值"
        );
    }
    let backup = if unknown || wallpaper.is_empty() {
        None
    } else {
        match (backup_original(guard, key, wallpaper), fallback) {
            (Some(name), _) => Some(name),
            (None, Some(fallback)) => match backup_transcoded(guard, key, fallback) {
                Ok((cache, name)) => {
                    let method = match fallback.method {
                        SettingMethod::AllMonitors => "接管前是「全部螢幕」設定",
                        _ => "接管前是逐螢幕設定",
                    };
                    log::warn!(
                        "螢幕 {device_path} 的原圖 {wallpaper} 複製不了：改備份 explorer 的轉存檔 {}（{name}；{method}）",
                        cache.display()
                    );
                    Some(name)
                }
                Err(why) => {
                    return Err(format!(
                        "螢幕 {device_path} 的原桌布 {wallpaper} 無法備份：原圖複製不了，轉存檔也不能代替（{why}）"
                    ))
                }
            },
            (None, None) => {
                return Err(format!(
                    "螢幕 {device_path} 的原桌布 {wallpaper} 無法備份：原圖複製不了，而系統快取的轉存檔可能已是宿主的圖、不能代替"
                ))
            }
        }
    };
    let original_file_id = if unknown || wallpaper.is_empty() {
        None
    } else {
        guard
            .files
            .file_id(Path::new(wallpaper))
            .map(PersistedFileId::from)
    };
    Ok(MonitorRecord {
        device_path: device_path.to_owned(),
        original_wallpaper: if unknown {
            String::new()
        } else {
            wallpaper.to_owned()
        },
        backup,
        recorded_at: now,
        last_set: None,
        host_applied: false,
        pending_restore: false,
        original_unknown: unknown,
        stale: false,
        original_file_id,
        solid_transition_readback: None,
    })
}

/// 備份時沿用來源副檔名的圖片格式（小寫）；其他一律依內容判斷（[`sniff_image_ext`]）。
const IMAGE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "jpe", "jfif", "png", "bmp", "dib", "gif", "tif", "tiff", "webp", "heic",
    "heif", "avif", "jxr", "wdp",
];

/// 依檔頭判定圖片格式（PNG／JPEG／BMP／GIF／WebP），回傳副檔名；認不出來或讀不到時 `None`。
fn sniff_image_format(path: &Path) -> Option<&'static str> {
    let mut head = [0u8; 12];
    let n = fs::File::open(path)
        .and_then(|mut f| io::Read::read(&mut f, &mut head))
        .ok()?;
    let head = &head[..n];
    if head.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("png")
    } else if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if head.starts_with(b"BM") {
        Some("bmp")
    } else if head.starts_with(b"GIF8") {
        Some("gif")
    } else if head.len() >= 12 && head.starts_with(b"RIFF") && &head[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

/// 依檔頭判定圖片的副檔名（轉存檔沒有副檔名；task 6.4）。認不出來時用 `jpg`（explorer 的轉存檔
/// 是 JPEG）。
fn sniff_image_ext(path: &Path) -> &'static str {
    sniff_image_format(path).unwrap_or("jpg")
}

/// 原圖複製不了時可改備份的 explorer 轉存檔（task 6.1 修正 Bug 2／B3、修正輪 3；只在接管開始、而且沒有任何
/// 在線螢幕正顯示宿主的圖時才有——否則轉存檔可能已是宿主的圖）。
///
/// 6.1 實機（重跑確認）：逐螢幕設定時每台有自己的 `Transcoded_<索引三位數>`（索引＝第一次設定前列舉時的
/// `GetMonitorDevicePathAt` 索引），**格式與尺寸沿用原圖**（JPEG 原圖就是 JPEG、3840×2400 就是 3840×2400）；
/// 以「全部螢幕」設定（`SetWallpaper(NULL, 圖)`）時不產生任何逐螢幕轉存檔，這時全域 `TranscodedWallpaper` 才代表
/// 所有螢幕。逐螢幕設定過之後全域檔停在最後一次「全部螢幕」設定的圖、**不代表任何一台**——拿它當逐螢幕備份會
/// 還原成錯的圖（B3）。所以來源依「設定方式」（[`SettingMethod`]）選，不能靠格式或尺寸判斷。
#[derive(Debug, Clone)]
struct TranscodedFallback {
    /// 這台螢幕在列舉中的索引。
    index: usize,
    /// 接管開始時判定的設定方式（[`setting_method`]）。
    method: SettingMethod,
}

/// 接管前桌布是怎麼設定的（修正輪 4，複審 N2；依第一次 `SetWallpaper` **之前**的登錄 `Wallpaper` 判定，見
/// [`setting_method`]）。決定原圖遺失時哪個轉存檔代表這台螢幕目前的桌布。
///
/// 6.1 實機：逐螢幕 `SetWallpaper(<螢幕>, 圖)` 之後登錄 `Wallpaper` 被改成 `...\Themes\TranscodedWallpaper`，各台的
/// `Transcoded_<索引>` 是各自的圖；「全部螢幕」`SetWallpaper(NULL, 圖)` 把登錄寫成原圖路徑、不產生逐螢幕檔，全域
/// `TranscodedWallpaper` 代表所有螢幕——之前逐螢幕設定留下的 `Transcoded_<索引>` 這時可能還在，但那是舊圖。
#[derive(Debug, Clone, PartialEq, Eq)]
enum SettingMethod {
    /// 「全部螢幕」設定：登錄＝各在線螢幕讀回的共同路徑 → 用全域 `TranscodedWallpaper`（即使殘留逐螢幕檔也不用）。
    AllMonitors,
    /// 逐螢幕設定：登錄＝這個 Themes 資料夾的 `TranscodedWallpaper` → 用該台的 `Transcoded_<索引>`，不存在就不能備份。
    PerMonitor,
    /// 判斷不出（帶原因）：任何轉存檔都不用，不接管＋通知。
    Unknown(String),
}

/// 以接管前（第一次 `SetWallpaper` 之前）的登錄 `Wallpaper` 判定設定方式（修正輪 4，複審 N2）：
///
/// 1. 登錄值（`REG_EXPAND_SZ` 先展開環境變數）＝各在線螢幕讀回的那個共同路徑（`path_eq`：`pathcmp` 正規化、不分
///    大小寫；所有在線螢幕都要讀得到、非空且彼此相同）→ [`SettingMethod::AllMonitors`]；
/// 2. 否則登錄值是這個 Themes 資料夾的 `TranscodedWallpaper`（[`same_file`]：先比路徑，不同時再比檔案識別——
///    隔離環境以 junction 指到同一個 Themes 時路徑字串不同）→ [`SettingMethod::PerMonitor`]；
/// 3. 其他（登錄讀不到、不是字串、空字串、與讀回不符又不是轉存檔路徑）→ [`SettingMethod::Unknown`]（保守）。
fn setting_method(
    registry_wallpaper: &RegValue,
    online: &[&MonitorWallpaper],
    paths: &StatePaths,
    files: &dyn FileIdentity,
) -> SettingMethod {
    let Some(raw) = registry_wallpaper.as_string() else {
        return SettingMethod::Unknown("登錄 Wallpaper 不存在或不是字串".to_owned());
    };
    let reg = expand_env_vars(&raw);
    if reg.is_empty() {
        return SettingMethod::Unknown("登錄 Wallpaper 是空字串".to_owned());
    }
    let common = online.first().and_then(|first| {
        let first = first.wallpaper.as_deref().filter(|w| !w.is_empty())?;
        online
            .iter()
            .all(|m| m.wallpaper.as_deref().is_some_and(|w| path_eq(w, first)))
            .then_some(first)
    });
    if common.is_some_and(|c| path_eq(&reg, c)) {
        return SettingMethod::AllMonitors;
    }
    let transcoded = paths.transcoded_wallpaper();
    if same_file(&reg, &transcoded.to_string_lossy(), files) {
        return SettingMethod::PerMonitor;
    }
    SettingMethod::Unknown(match common {
        Some(c) => format!(
            "登錄 Wallpaper（{reg}）既不是各螢幕讀回的共同路徑 {c}，也不是 {}",
            transcoded.display()
        ),
        None => format!(
            "各螢幕讀回不是同一個路徑，而登錄 Wallpaper（{reg}）也不是 {}",
            transcoded.display()
        ),
    })
}

/// 還原時能不能改用「全部螢幕」的一次 `SetWallpaper(NULL, 圖)`（[`OriginalDesktop::all_monitors`] 為真時才問）：回傳
/// 要設定的路徑，不能就回 `None`（維持逐螢幕還原）。全部成立才行：
///
/// 1. 至少有一台要還原的螢幕（[`RestoreClass::Host`]），而且它們要還原成的路徑**非空且彼此相同**（用備份副本還原
///    的螢幕備份檔名各不相同，不可能是同一張）；
/// 2. 沒有離線螢幕（審查 M1）：`SetWallpaper(NULL, …)` 對離線螢幕的語意未經實機驗證，可能連 explorer 為它記住的
///    （例如使用者在接管期間逐螢幕換的）圖一起覆寫；離線螢幕重新接上時也會依待還原紀錄逐螢幕還原，
///    `GetWallpaper(NULL)` 又變回空字串。有離線螢幕就維持逐螢幕還原；
/// 3. 每一台在線螢幕都在計畫裡，而且不是「要還原」就是「已經顯示這張圖」——`SetWallpaper(NULL, …)` 會動到所有螢幕，
///    計畫外的在線螢幕（沿用自前一次接管、不是宿主圖而不動它的 `stale` 紀錄、沒有紀錄的螢幕）不能被它蓋掉。
///
/// 使用者自選的螢幕、讀不到的螢幕、投影片、純色都在呼叫端先排除。
fn restore_all_monitors_path(plan: &[PlanItem], snap: &WallpaperSnapshot) -> Option<String> {
    let mut hosts = plan.iter().filter(|p| p.class == RestoreClass::Host);
    let first = hosts.next()?.want.path.clone();
    if first.is_empty() {
        return None;
    }
    if !plan
        .iter()
        .filter(|p| p.class == RestoreClass::Host)
        .all(|p| path_eq(&p.want.path, &first))
    {
        return None;
    }
    if snap.monitors.iter().any(|m| !m.monitor.is_online()) {
        return None;
    }
    let covered = snap
        .monitors
        .iter()
        .filter(|m| m.monitor.is_online())
        .all(|m| {
            plan.iter()
                .find(|p| path_eq(&p.device, &m.monitor.device_path))
                .is_some_and(|p| match p.class {
                    RestoreClass::Host => true,
                    RestoreClass::Original => path_eq(&p.current, &first),
                    RestoreClass::UserChoice => false,
                })
        });
    covered.then_some(first)
}

/// 依 [`TranscodedFallback`] 的設定方式備份轉存檔（修正輪 4）：
///
/// - [`SettingMethod::AllMonitors`] → 全域 `TranscodedWallpaper`（殘留的逐螢幕檔是舊圖，不用）；
/// - [`SettingMethod::PerMonitor`] → 這台的 `Transcoded_<索引>`，不存在就不能備份（不退回全域檔）；
/// - [`SettingMethod::Unknown`] → 不能備份。
///
/// 選中的檔必須存在、可讀、大小 > 0，而且依檔頭判斷得出是圖片（[`sniff_image_format`]：PNG／JPEG／BMP／GIF／
/// WebP），副檔名照檔頭；否則不能備份——寧可不接管，也不備到別台或別張圖。成功回 (來源, 備份檔名)；失敗回原因。
fn backup_transcoded(
    guard: &BackupGuard<'_>,
    key: &str,
    fallback: &TranscodedFallback,
) -> Result<(PathBuf, String), String> {
    let cache = match &fallback.method {
        SettingMethod::AllMonitors => guard.paths.transcoded_wallpaper(),
        SettingMethod::PerMonitor => {
            let own = guard.paths.monitor_transcoded(fallback.index);
            if !own.exists() {
                return Err(format!(
                    "接管前是逐螢幕設定（登錄 Wallpaper 是 TranscodedWallpaper），但這台的逐螢幕轉存檔 {} 不存在；全域 TranscodedWallpaper 不代表這台",
                    own.display()
                ));
            }
            own
        }
        SettingMethod::Unknown(why) => {
            return Err(format!(
                "判斷不出接管前是「全部螢幕」還是逐螢幕設定（{why}），不知道哪個轉存檔代表這台"
            ))
        }
    };
    let size = fs::metadata(&cache)
        .map_err(|e| format!("{} 讀不到：{e}", cache.display()))?
        .len();
    if size == 0 {
        return Err(format!("{} 是空檔", cache.display()));
    }
    let Some(ext) = sniff_image_format(&cache) else {
        return Err(format!(
            "{} 依檔頭判斷不出是圖片（PNG／JPEG／BMP／GIF／WebP）",
            cache.display()
        ));
    };
    match backup_original_as(guard, key, &cache.to_string_lossy(), Some(ext)) {
        Some(name) => Ok((cache, name)),
        None => Err(format!("{} 複製不了", cache.display())),
    }
}

/// 沒有紀錄、卻正顯示宿主圖的在線螢幕（[`WallpaperTakeover::adopt_unrecorded_host_monitors`]）。
struct Unrecorded {
    key: String,
    device: String,
    /// 目前顯示的宿主輸出圖。
    current: String,
    /// 要還原成什麼（純色＝空字串）。
    want_path: String,
    /// 從別台的備份複製來的這台自己的備份檔名（每個備份檔只屬於一筆紀錄）。
    own_copy: Option<String>,
}

/// 沒有紀錄的螢幕要還原成的「全域原桌布」（task 6.4，審查 R1-M2 裁定）：
///
/// 1. 狀態檔記錄的登錄 `Wallpaper` 原值（[`registry_original`]：存在、不在桌布快取資料夾、不是宿主輸出圖）；
/// 2. 否則任一已記錄螢幕（原桌布已知、不是 `stale`）的原桌布（[`restore_target`]；落在備份資料夾內時先
///    複製成這台自己的備份——絕不讓螢幕顯示別台的備份）；
/// 3. 都沒有就純色並記警告。
fn global_original_want(
    key: &str,
    device: &str,
    all: &BTreeMap<String, MonitorRecord>,
    registry_wallpaper: Option<&RegValue>,
    guard: &BackupGuard<'_>,
    warnings: &mut Vec<String>,
) -> Want {
    let paths = guard.paths;
    let exists = |p: &Path| p.exists();
    if let Some(path) = registry_original(registry_wallpaper, paths, &exists)
        .filter(|p| !path_is_within(p, &paths.backup_dir()))
    {
        return Want {
            path,
            own_copy: None,
        };
    }
    for rec in all.values().filter(|r| !r.original_unknown && !r.stale) {
        match restore_target(rec, paths, &exists, guard.files) {
            RestoreTarget::Solid => {
                return Want {
                    path: String::new(),
                    own_copy: None,
                }
            }
            RestoreTarget::Image(src) if path_is_within(&src, &paths.backup_dir()) => {
                if let Some(name) = backup_original(guard, key, &src) {
                    return Want {
                        path: paths
                            .backup_dir()
                            .join(&name)
                            .to_string_lossy()
                            .into_owned(),
                        own_copy: Some(name),
                    };
                }
            }
            RestoreTarget::Image(src) => {
                return Want {
                    path: src,
                    own_copy: None,
                }
            }
            RestoreTarget::Missing | RestoreTarget::Unknown => {}
        }
    }
    warnings.push(format!(
        "{device} 沒有紀錄、正顯示宿主的圖，但找不到全域原桌布（登錄原值與其他螢幕的原桌布都不可用），改還原為純色"
    ));
    Want {
        path: String::new(),
        own_copy: None,
    }
}

/// 紀錄要改採納使用者的新桌布前，把它的舊備份記進 [`StateFile::superseded_backups`]（task 6.4，審查
/// R1-M4）：保留到下一次接管開始，不在同一次寫入當孤兒刪掉。
fn supersede_backup(superseded: &mut Vec<String>, rec: &MonitorRecord) {
    if let Some(b) = &rec.backup {
        if !superseded.contains(b) {
            superseded.push(b.clone());
        }
    }
}

/// 使用者自己選的桌布記為新的原桌布（讓位、還原時發現使用者已換）。
fn adopt_user_choice(rec: &mut MonitorRecord, current: String, now: i64) {
    rec.original_wallpaper = current;
    rec.original_unknown = false;
    rec.backup = None;
    rec.recorded_at = now;
    clear_host_marks(rec);
}

/// 螢幕上已不是宿主的圖：清掉與宿主圖相關的標記。
fn clear_host_marks(rec: &mut MonitorRecord) {
    rec.last_set = None;
    rec.host_applied = false;
    rec.pending_restore = false;
    rec.solid_transition_readback = None;
}

/// 一台螢幕要還原成什麼（[`restore_want`] 的結果）。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Want {
    /// 要設定的 `GetWallpaper` 值（純色＝空字串）。
    path: String,
    /// 原桌布未知、而 fallback 選中備份資料夾內的檔時：已複製成這台自己的備份，這是它的檔名
    /// （`path` 就是它的完整路徑）。呼叫端以 [`adopt_own_copy`] 把它採納為這台的原桌布
    /// （修正輪 4：每個備份檔只屬於一筆紀錄，絕不讓螢幕顯示別台的備份）。
    own_copy: Option<String>,
}

/// 這台螢幕（紀錄鍵 `key`）要還原成什麼；原桌布未知時依 [`unknown_original_fallback`]；都找不到
/// 時改純色並記警告。
///
/// fallback 選中備份資料夾內的檔（別台的備份）時，先把它複製成這台自己的備份
/// `<備份資料夾>\<key>.<副檔名>`，改以這份複本還原（修正輪 4）：否則別台的紀錄一改（以新圖重新
/// 記錄覆寫同名備份、或讓位放掉備份後被當成孤兒刪掉），這台螢幕的桌布來源就跟著消失。複製失敗時
/// 不沿用別台的備份，改純色並記警告。
fn restore_want(
    key: &str,
    record: &MonitorRecord,
    current: &str,
    all: &BTreeMap<String, MonitorRecord>,
    registry_wallpaper: Option<&RegValue>,
    guard: &BackupGuard<'_>,
    warnings: &mut Vec<String>,
) -> Want {
    if let Some(name) = own_backup_shown(key, record, current, guard) {
        // 修正輪 R：這台正顯示自己寫下的備份（例如上一次還原中斷前寫下的 fallback 複本）＝已是
        // 原桌布。重用它、不另寫序號檔；它本來就是這筆紀錄的 `backup` 時不必再採納。
        return Want {
            path: current.to_owned(),
            own_copy: (record.backup.as_deref() != Some(name.as_str())).then_some(name),
        };
    }
    let paths = guard.paths;
    let exists = |p: &Path| p.exists();
    let solid = Want {
        path: String::new(),
        own_copy: None,
    };
    let target = match restore_target(record, paths, &exists, guard.files) {
        RestoreTarget::Unknown => {
            let known: Vec<&MonitorRecord> = all.values().collect();
            match unknown_original_fallback(&known, registry_wallpaper, paths, &exists, guard.files)
            {
                RestoreTarget::Image(src) if path_is_within(&src, &paths.backup_dir()) => {
                    return match backup_original(guard, key, &src) {
                        Some(name) => Want {
                            path: paths
                                .backup_dir()
                                .join(&name)
                                .to_string_lossy()
                                .into_owned(),
                            own_copy: Some(name),
                        },
                        None => {
                            // 修正輪 5 裁決 3：依 fallback 的順序改試登錄記錄的原值，再不行才純色。
                            match registry_original(registry_wallpaper, paths, &exists) {
                                Some(path) if !path_is_within(&path, &paths.backup_dir()) => {
                                    warnings.push(format!(
                                        "{} 的原桌布未知，要沿用的備份 {src} 無法複製成這台自己的備份，改用登錄記錄的原值 {path}",
                                        record.device_path
                                    ));
                                    Want {
                                        path,
                                        own_copy: None,
                                    }
                                }
                                _ => {
                                    warnings.push(format!(
                                        "{} 的原桌布未知，要沿用的備份 {src} 無法複製成這台自己的備份，改還原為純色",
                                        record.device_path
                                    ));
                                    solid
                                }
                            }
                        }
                    };
                }
                other => other,
            }
        }
        other => other,
    };
    match target {
        RestoreTarget::Solid => solid,
        RestoreTarget::Image(path) => Want {
            path,
            own_copy: None,
        },
        RestoreTarget::Missing | RestoreTarget::Unknown => {
            warnings.push(format!(
                "{} 的原桌布找不到（原圖與備份都不存在，或原桌布未知），改還原為純色",
                record.device_path
            ));
            solid
        }
    }
}

/// `name` 是否為鍵 `key` 命名的備份檔：`<鍵>.<副檔名>` 或 `<鍵>-<n>.<副檔名>`（n ≥ 2，
/// [`backup_original`] 的序號檔名）。
fn is_own_backup_name(key: &str, name: &str) -> bool {
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return false;
    };
    if ext.is_empty() || stem.len() < key.len() || !stem.is_char_boundary(key.len()) {
        return false;
    }
    let (head, rest) = stem.split_at(key.len());
    if !head.eq_ignore_ascii_case(key) {
        return false;
    }
    rest.is_empty()
        || rest.strip_prefix('-').is_some_and(|n| {
            !n.is_empty()
                && n.bytes().all(|b| b.is_ascii_digit())
                && n.parse::<u32>().is_ok_and(|n| n >= 2)
        })
}

/// 這台螢幕（紀錄鍵 `key`）目前顯示的 `current` 是不是它自己的備份檔（修正輪 R）：備份資料夾內、
/// 路徑或檔案識別與 `current` 相同，而且**沒有**被其他紀錄引用（`<鍵>-<n>` 也可能是 `free_key`
/// 序號鍵那筆紀錄的備份）。回傳檔名。「自己的」：
///
/// - 原桌布已知的紀錄：只有這筆紀錄的 `backup`（同鍵但沒被它引用的殘留檔不算）；
/// - 原桌布未知的紀錄（沒有 `backup` 可認）：以本鍵命名的檔，含序號檔名——上一次還原中斷前寫下、
///   還沒採納的 fallback 複本。
fn own_backup_shown(
    key: &str,
    record: &MonitorRecord,
    current: &str,
    guard: &BackupGuard<'_>,
) -> Option<String> {
    if current.is_empty() {
        return None;
    }
    let dir = guard.paths.backup_dir();
    fs::read_dir(&dir)
        .ok()?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| {
            record.backup.as_deref() == Some(n.as_str())
                || (record.original_unknown && is_own_backup_name(key, n))
        })
        .find(|n| {
            let full = dir.join(n).to_string_lossy().into_owned();
            same_file(&full, current, guard.files) && !guard.referenced_by_other(key, &full)
        })
}

/// 把 [`Want::own_copy`] 採納為這台的原桌布：它現在擁有這份備份，螢幕上（還原後）顯示的就是它。
fn adopt_own_copy(rec: &mut MonitorRecord, want: &Want, now: i64) {
    if let Some(name) = &want.own_copy {
        rec.original_wallpaper = want.path.clone();
        rec.backup = Some(name.clone());
        rec.original_unknown = false;
        rec.recorded_at = now;
    }
}

/// 把原圖複製成 `<備份資料夾>\<鍵>.<副檔名>`（先寫暫存檔再改名：來源就是備份檔本身時也不會
/// 截斷它）。失敗只記警告、回 `None`。同一個鍵的其他副檔名或序號備份由 [`remove_orphan_backups`]
/// 清掉。
///
/// 絕不覆寫受保護的檔（修正輪 5，[`BackupGuard::protects`]）：`<鍵>.<副檔名>` 正被其他紀錄引用
/// （例如使用者從「最近使用的背景」替另一台選了它）或正在顯示時，改寫到 `<鍵>-<n>.<副檔名>`
/// （n 從 2 起，取第一個不受保護的）。目的檔就是來源本身時照常沿用（內容不變）。
fn backup_original(guard: &BackupGuard<'_>, key: &str, source: &str) -> Option<String> {
    backup_original_as(guard, key, source, None)
}

/// [`backup_original`]，可指定副檔名（`None`＝取來源的副檔名；轉存檔沒有副檔名，task 6.4）。
fn backup_original_as(
    guard: &BackupGuard<'_>,
    key: &str,
    source: &str,
    ext: Option<&str>,
) -> Option<String> {
    // 修正第 2 輪（複審 N2 附帶）：來源沒有已知的圖片副檔名（例如轉存檔）時依內容判斷，判斷不出來用 jpg——
    // 不產生 `.bin`（`SetWallpaper` 對非圖片副檔名的接受度沒有查證）。
    let ext = ext.map(str::to_owned).unwrap_or_else(|| {
        Path::new(source)
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .filter(|e| IMAGE_EXTENSIONS.contains(&e.as_str()))
            .unwrap_or_else(|| sniff_image_ext(Path::new(source)).to_owned())
    });
    let dir = guard.paths.backup_dir();
    let mut n = 1u32;
    let name = loop {
        let candidate = if n == 1 {
            format!("{key}.{ext}")
        } else {
            format!("{key}-{n}.{ext}")
        };
        let full = dir.join(&candidate).to_string_lossy().into_owned();
        if same_file(&full, source, guard.files) || !guard.protects(key, &full) {
            break candidate;
        }
        log::warn!("備份檔 {candidate} 正被其他紀錄引用或正在顯示：不覆寫，改用其他檔名");
        n += 1;
    };
    let dest = dir.join(&name);
    let tmp = dir.join(format!("{name}.tmp"));
    let copied = fs::create_dir_all(&dir)
        .and_then(|()| fs::copy(source, &tmp))
        .and_then(|_| fs::rename(&tmp, &dest));
    match copied {
        Ok(()) => Some(name),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            log::warn!(
                "原圖備份失敗（{source} → {}）：{e}；仍記錄原路徑",
                dest.display()
            );
            None
        }
    }
}

/// 刪掉備份資料夾中沒有人用的備份（只看宿主自己命名的 `m-*`；修正輪 2 裁決 9、修正輪 3 裁決 1）。
/// 下列任一成立就**保留**（比對都是路徑加檔案識別）：
///
/// - 某筆紀錄的 `backup` 是它；
/// - 某筆紀錄的 `original_wallpaper` 就是它（從備份還原後，它成了那台螢幕的桌布來源）；
/// - 它是某台螢幕目前的桌布（`in_use`，最近一次讀回；含讀得到的離線螢幕）；
/// - 它是讓位或還原時被取代的舊備份（[`StateFile::superseded_backups`]；task 6.4，保留到下一次接管開始）。
fn remove_orphan_backups(
    paths: &StatePaths,
    state: &StateFile,
    in_use: &[String],
    files: &dyn FileIdentity,
) {
    let dir = paths.backup_dir();
    let Ok(entries) = fs::read_dir(&dir) else {
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.starts_with("m-") {
            continue;
        }
        let full = dir.join(&name).to_string_lossy().into_owned();
        let referenced = state.monitors.values().any(|r| {
            r.backup.as_deref() == Some(name.as_str())
                || (!r.original_wallpaper.is_empty()
                    && same_file(&r.original_wallpaper, &full, files))
        }) || state.superseded_backups.contains(&name)
            || in_use.iter().any(|w| same_file(w, &full, files));
        if !referenced {
            if let Err(err) = fs::remove_file(e.path()) {
                log::warn!("刪除沒有人用的原圖備份 {name} 失敗：{err}");
            }
        }
    }
}

/// 狀態檔的持久寫入（修正輪 3 裁決 4：只給本模組用，`settings::write_atomic` 維持原行為）：
/// 目錄不存在時先建立，寫到同目錄的 `<檔名>.tmp`、`sync_all` 讓內容落地後才 `rename` 換上——
/// 斷電時不會留下「已改名、內容卻是空的」狀態檔（那會遺失使用者的原桌布紀錄）。
/// 序列化並持久寫入狀態檔。
fn write_state_json(path: &Path, state: &StateFile) -> io::Result<()> {
    let text = serde_json::to_string_pretty(state)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    write_durable(path, text.as_bytes())
}

fn write_durable(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| STATE_FILE_NAME.to_owned());
    let tmp = path.with_file_name(format!("{file_name}.tmp"));
    {
        let mut file = fs::File::create(&tmp)?;
        io::Write::write_all(&mut file, bytes)?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)
}

/// `SetWallpaper` 的錯誤是否代表**確定沒有生效**（task 6.1 修正輪 2）：後端回報失敗（HRESULT 錯誤、沒有介面、
/// 資料無法使用）或請求根本沒送出（忙碌、主執行緒、工作執行緒已結束）。逾時與處理中途中止**不算**——請求可能
/// 已送到 explorer、之後才生效。
fn set_definitely_not_applied(e: &WallpaperError) -> bool {
    matches!(
        e,
        WallpaperError::Backend { .. }
            | WallpaperError::Busy { .. }
            | WallpaperError::WrongThread { .. }
            | WallpaperError::WorkerGone { .. }
    )
}

/// 一台螢幕在一次讀取中的狀況。
enum MonitorRead<'s> {
    /// 離線或不在列舉中。
    Offline,
    /// 在線，但這次讀不到它的桌布。
    Unknown,
    /// 在線，目前的桌布（空字串＝純色）。
    Wallpaper(&'s str),
}

fn monitor_readback<'s>(snap: &'s WallpaperSnapshot, device_path: &str) -> MonitorRead<'s> {
    match snap
        .monitors
        .iter()
        .find(|m| m.monitor.is_online() && path_eq(&m.monitor.device_path, device_path))
    {
        None => MonitorRead::Offline,
        Some(m) => match m.wallpaper.as_deref() {
            None => MonitorRead::Unknown,
            Some(w) => MonitorRead::Wallpaper(w),
        },
    }
}

/// 在線螢幕目前的桌布（離線、不存在或讀不到為 `None`）。
fn online_wallpaper<'s>(snap: &'s WallpaperSnapshot, device_path: &str) -> Option<&'s str> {
    snap.monitors
        .iter()
        .find(|m| m.monitor.is_online() && path_eq(&m.monitor.device_path, device_path))
        .and_then(|m| m.wallpaper.as_deref())
}

fn slideshow_matches(snap: &WallpaperSnapshot, want: &SlideshowInfo) -> bool {
    slideshow_applicable(snap.slideshow_status)
        && snap.slideshow.as_ref().is_some_and(|s| {
            s.options == want.options
                && s.tick_ms == want.tick_ms
                && s.items.len() == want.items.len()
                && s.items.iter().zip(&want.items).all(|(a, b)| path_eq(a, b))
        })
}

/// 逐螢幕還原後的讀回是否算還原成功：與要還原的值相同；或（`relax_solid` 有值時）要還原的是純色（空字串）、
/// 讀回已不是宿主的圖也不是桌布快取工作檔——逐螢幕設空字串時 explorer 換回它記憶中的圖片（task 6.1 B9：
/// 純色是全域狀態，逐螢幕做不到），這已是能做到的還原，不算失敗（否則永遠標為待還原、反覆重試）。
fn restored_matches(
    current: &str,
    want: &str,
    relax_solid: Option<&StatePaths>,
    files: &dyn FileIdentity,
) -> bool {
    if path_eq(current, want) {
        return true;
    }
    let Some(paths) = relax_solid.filter(|_| want.is_empty()) else {
        return false;
    };
    let ok = !is_host_output_file(current, paths, files) && !paths.is_themes_cache(current);
    if ok {
        log::info!(
            "逐螢幕還原為純色：讀回 {current}（純色是全域狀態，逐螢幕設空字串時 Windows 顯示它記憶中的圖片）"
        );
    }
    ok
}

/// 還原後讀回驗證；`Ok` 帶回驗證時已離線的螢幕，`Err` 帶回不符的項目。`globals`／`slideshow`／
/// `registry` 為 `None` 時不驗證該項（使用者已自行選擇、這次沒有還原它們）。`relax_solid` 見
/// [`restored_matches`]（全域純色還原時為 `None`：每台都要讀回空字串）。
fn verify_restored(
    io: &mut TakeoverIo<'_>,
    expected: &[(String, String)],
    globals: Option<&OriginalDesktop>,
    slideshow: Option<&SlideshowInfo>,
    registry_snapshot: Option<&DesktopRegistrySnapshot>,
    relax_solid: Option<&StatePaths>,
) -> Result<Vec<String>, Vec<String>> {
    let snap = io
        .wallpaper
        .read()
        .map_err(|e| vec![format!("還原後讀回失敗：{e}")])?;
    let mut mismatches = Vec::new();
    let mut offline = Vec::new();
    for (device, want) in expected {
        match online_wallpaper(&snap, device) {
            None => offline.push(device.clone()),
            // 投影片由 explorer 自行挑圖，逐螢幕路徑不比對，改比對投影片設定。
            Some(_) if slideshow.is_some() => {}
            Some(cur) if restored_matches(cur, want, relax_solid, io.files) => {}
            Some(cur) => mismatches.push(format!("{device} 的桌布是 {cur}，應為 {want}")),
        }
    }
    if let Some(info) = slideshow {
        if !slideshow_matches(&snap, info) {
            mismatches.push("投影片設定與原本不同".to_owned());
        }
    }
    if let Some(original) = globals {
        if snap.position.0 != original.position {
            mismatches.push(format!(
                "填滿方式是 {}，應為 {}",
                snap.position.0, original.position
            ));
        }
        if snap.background_color != original.background_color {
            mismatches.push(format!(
                "背景色是 {:#08x}，應為 {:#08x}",
                snap.background_color, original.background_color
            ));
        }
    }
    if let Some(want) = registry_snapshot {
        match registry::snapshot_desktop_registry_in(&*io.registry) {
            Ok(cur) if cur == *want => {}
            Ok(_) => {
                // explorer 可能在寫回之後才處理完 SetWallpaper：重寫一次，下一次驗證再看。
                let _ = registry::restore_desktop_registry_in(io.registry, want);
                mismatches.push("桌布登錄值與原值不同".to_owned());
            }
            Err(e) => mismatches.push(e.to_string()),
        }
    }
    if mismatches.is_empty() {
        Ok(offline)
    } else {
        Err(mismatches)
    }
}

#[cfg(test)]
mod tests;
