## Context

動機見 proposal.md。研究全文：`.superpowers/research/updater-installer.md`（git 忽略，2026-10-05 查證，附來源）。
發版線（收尾線）以 tauri-cli-v2.12.1 的 `installer.nsi`、plugins-workspace `updater.rs`、tauri-docs v2 做過唯讀審查；
本文的 `installer.nsi:<行>` 指 tauri-cli-v2.12.1 版本。要點：

- 宿主是 Tauri 2.12、純 `cargo build`；`tauri.conf.json` 沒有 `bundle`，`frontendDist` 是靜態資料夾 `ui`，沒有
  `beforeBuildCommand`。
- desktop-widget-host 已規定：開機自啟歸安裝檔，宿主只在使用者切換開關時寫登錄（memory
  `autostart-registration-belongs-to-installer`）。dynamic-wallpaper 提供 `--restore-wallpaper` 給解除安裝呼叫。
- 宿主已註冊 `RegisterApplicationRestart`、使用 single-instance 外掛，且曾踩過 Restart Manager 的坑（memory
  `restart-manager-rmrestart-ignores-caller-env`）。
- repo 是公開的 Apache-2.0 專案；目前 GitHub 的 Latest release 是 Lively 時代的 v6.2，Lively 版已凍結、不再發版。

## Goals / Non-Goals

**Goals:**

- 一般使用者能一鍵安裝，往後自動收到修正，不需要回著陸頁。
- 更新機制本身可以在本機完整驗證（不必真的發 release）。
- 發出壞版本時不會讓所有使用者卡死：發佈前有自動冒煙測試，更新檢查不依賴其他功能正常。

**Non-Goals:**

- 差量更新、更新頻道（beta／stable）、可回退舊版的選單。
- Authenticode 簽章（v0.1.0 後申請 SignPath Foundation，屆時另開 change；打包流程已預留插入點，見 D6）。
- MSI、Microsoft Store、winget 上架。
- 本機 ARM64 實機驗證（本機沒有 ARM64 機器；ARM64 以 CI 建置與冒煙測試為準，實機驗證列為已知缺口）。
- 著陸頁的架構偵測（歸收尾線，見 D8）。

## Decisions

### D1. tauri-plugin-updater＋Tauri NSIS，不用 Velopack

- 與現有技術棧同一家：NSIS 範本已處理 WebView2 安裝、以 Restart Manager 關閉舊版、`/UPDATE` 模式保留開機自啟值、
  解除安裝時刪除 `HKCU\…\Run\fc-host`（值名與 `productName` 相同）。
- 靜態 `latest.json` 放在 `github.com/…/releases/latest/download/`，不走 GitHub API，不受匿名每小時 60 次的限制。
- Velopack 有差量更新，但要 .NET 工具鏈、每個架構一個頻道、走 GitHub API，還要把它的 `current` 目錄模型與我們的
  單一執行個體、開機自啟重新對接；宿主每次更新完整下載數十 MB、每月至多數次，差量的價值不足以抵銷。cargo-packager
  已 10 個月沒有新 release。不採用。
- 外掛以 `default-features = false`、features `native-tls`、`system-proxy` 引入，與 data-layer-rust 共用 SChannel；
  不開 `zip`（只發 NSIS exe，不發 zip 包）。

### D2. 安裝檔設定與 hook

- `bundle.targets = ["nsis"]`、`windows.nsis.installMode = "currentUser"`、`windows.nsis.languages = ["TradChinese"]`、
  `webviewInstallMode = downloadBootstrapper`（Win11 內建 WebView2，缺少時才下載）、圖示用 `host/icons/icon.ico`。
  `createUpdaterArtifacts = false`：更新簽章在打包後獨立產生（D6）。`tauri.conf.json` 的 `version` 省略，由
  `Cargo.toml` 提供，版本只有一個來源。
- `installerHooks` 指向 `host/installer/hooks.nsh`。**該檔必須存成 UTF-8 含 BOM**：bundler 只替它產生的主腳本加 BOM，
  被 include 的檔案沒有 BOM 時中文會變亂碼；task 2.1 加 BOM 檢查。各 hook：
  - `NSIS_HOOK_PREINSTALL`：記下安裝前 `$INSTDIR\fc-host.exe` 是否已存在（同版本「直接覆蓋」不會先解除安裝，
    `installer.nsi` `PageLeaveReinstall` 同版本的預設選項）。
  - `NSIS_HOOK_POSTINSTALL`：
    - **每次都執行**（含更新模式）：以 `WriteRegStr SHCTX "${UNINSTKEY}" "DisplayName" "財經日曆"` 覆寫顯示名稱——範本在
      `installer.nsi:702` 每次安裝都先寫回 `${PRODUCTNAME}`，POSTINSTALL 在 `:734` 之後；開始功能表捷徑改名：**只在來源
      `$SMPROGRAMS\${PRODUCTNAME}.lnk` 存在時**才先刪除既有的「財經日曆.lnk」、再把來源 `Rename` 成「財經日曆.lnk」；來源
      不存在就什麼都不做。理由：首次安裝與同版本重裝時範本會建立 `fc-host.lnk`（`:949`／`:952`），`Rename` 遇到既有目標會
      失敗所以要先刪；但更新模式下範本在 `:943` 直接 `Return`、不建來源，若仍先刪目標，捷徑就會消失。
    - **只在非更新模式、且安裝前沒有既有的 `fc-host.exe` 時**：寫 Run 值 `"$INSTDIR\fc-host.exe" --autostart`，保留使用者
      在設定視窗關掉自啟的選擇。
  - `NSIS_HOOK_PREUNINSTALL`（只在非更新模式）：以 `ExecWait` 執行 `"$INSTDIR\fc-host.exe" --restore-wallpaper`。`ExecWait`
    沒有逾時參數，正好符合 `host/src/wallpaper_cli.rs` 給安裝檔的契約：指令本身有 90 秒硬上限、一定會自己結束，安裝檔
    不得強制終止它（建議等待上限 ≥120 秒）。依結束碼處置（同一份契約）：0 繼續；6（在線螢幕已還原、離線螢幕待補）只記錄、
    繼續；3（宿主沒回報、可能仍在處理）等 15 秒再重跑一次；4（宿主還沒準備好或收不到）等 5 秒再重跑一次；1（還原失敗或
    狀態檔暫時讀不到，常見於顯示器拓樸切換、explorer 重啟中讀不到任何螢幕這類數秒內會平息的暫時錯誤）等 5 秒再重跑一次；
    7 與 2、5、其他值（契約：可重跑一次）直接重跑一次；仍失敗就以訊息框告知桌布可能停在
    宿主的圖、需自行在 Windows 設定更換（passive／silent 模式不跳訊息框），然後繼續。每次結束碼與處置都以 `FileWrite`
    附加到 `%LOCALAPPDATA%\tw.fintools.fc-host\logs\uninstall.log`。宿主在執行中時，指令會把還原交給它
    （dynamic-wallpaper 4.7b）。殘餘風險：結束碼 3 重跑後若宿主仍卡在還原中途，範本的 Restart Manager 會強制結束它，
    狀態檔留下「還原進行中」標記、但解除安裝後不會再有下一次執行——此時桌布停在宿主的圖，已由訊息框告知。
  - `NSIS_HOOK_POSTUNINSTALL`（只在非更新模式）：刪除「財經日曆.lnk」（範本 `:836-840` 只刪 `${PRODUCTNAME}.lnk`，可沿用
    範本的 `UnpinShortcut`）；刪除 `HKCU\…\Explorer\StartupApproved\Run` 的 `fc-host` 值——否則使用者曾在工作管理員停用
    自啟時，停用狀態會殘留到重新安裝：Run 值寫回了、自啟卻仍停用，宿主的開關又顯示「開」。
  - 解除安裝刪除 Run 值由範本內建，hook 不重複做。更新模式下範本與 hook 都不碰 Run 與 StartupApproved。
- 手動覆蓋安裝：範本的「重新安裝」頁預設會先以非更新模式執行舊版解除安裝程式（`installer.nsi` `PageLeaveReinstall`），
  所以會還原桌布、主題改回「不接管」、重新登錄開機自啟。舊版解除安裝程式無法分辨「使用者真的要移除」與「覆蓋安裝」，
  接受這個行為並寫進 README（spec「手動下載新版覆蓋安裝」）；一般使用者走自動更新，不會遇到。
  實機（task 4.2，2026-10-05）發現：以 `/P`（passive）或 `/S` 執行時重新安裝頁不顯示、讀不到選項，範本的分支結果與互動預設
  **相反**——同版本會先解除舊版、升級則直接覆蓋。因此 spec 的「同版本直接覆蓋」「手動覆蓋安裝」兩個情境以**互動模式**為準
  （待使用者在場驗證）；自動更新帶 `/UPDATE`，不經過此頁，不受影響。CI 冒煙測試用 `/S` 的全新安裝，也不受影響。
- `productName` 維持 `fc-host`：範本用 `${PRODUCTNAME}` 當 Run 值名（解除安裝時刪的就是它）、安裝資料夾名與開始功能表
  捷徑名，宿主切換開機自啟時寫的 Run 值名也是 `fc-host`（desktop-widget-host D12）；改 `productName` 要連宿主一起改並處理
  舊值，不值得。已知限制：安裝精靈視窗標題仍是 `fc-host`。桌面捷徑（2026-10-06 使用者決定）同樣改名為「財經日曆」：
  被動／靜默安裝的桌面捷徑在 `:729-731` 建立、早於 POSTINSTALL，由 POSTINSTALL 改名；互動安裝的桌面捷徑由完成頁勾選框
  （`:411-413`）在所有 hook 之後建立，hooks.nsh 另定義範本沒有的 NSIS 回呼 `.onGUIEnd`（安裝視窗關閉後才呼叫）改名。
  只動目標是本主程式的捷徑；解除安裝刪桌面「財經日曆.lnk」也要求目標吻合（比照範本 `:844-849`）。task 2.1 以實際
  安裝確認中文 `DisplayName` 與捷徑名正確、更新後不被改回。

### D3. 更新策略

- 檢查：啟動後 60–180 秒隨機延遲，之後每 6 小時；失敗只寫記錄。發現新版後立即背景下載並驗簽，位元組留在記憶體
  （安裝檔數十 MB，可接受）。檢查間隔以牆上時鐘計（含睡眠）：每 5 分鐘醒來比對 `SystemTime`，時鐘倒退時從現在重新
  計算；啟動首檢延遲、過渡期 60 秒逾時與標記穩定時間（5 分鐘）以清醒時間計（依保守假設：Windows 8 以後相對逾時不計入
  睡眠，`WaitForSingleObject` 文件明寫，標準庫 `Condvar` 所用的 `WaitOnAddress` 文件未寫）。
- **隔離環境停用**：環境變數 `LOCALAPPDATA` 與系統登記的本機應用程式資料夾不同（驗收腳本以暫存資料夾隔離宿主）時，
  更新器不檢查、不下載、不安裝、系統匣不出現更新項目，記錄一行 info 寫明原因與兩個路徑。理由：NSIS 安裝檔與 `HKCU\…\Run`
  都不跟環境變數走，隔離的舊版測試宿主以 `--autostart` 啟動後會把正式版裝進**真正的**使用者資料夾、寫真正的登錄。判斷與
  資料抓取 `auto` 共用同一個函式（`desktop::detect_isolated_local_app_data`）；以 `cfg!(feature = "update-e2e")` 表達例外——
  `update-e2e` 建置不套用，e2e 要在任何環境都能跑更新。走既有的 gate 停用機制（`DisabledReason::Isolated`，非錯誤），啟動
  過渡狀態機照「更新器停用」處理。
- **更新檢查與其他功能隔離**：更新器在 setup 一開始、建立小工具與桌布協調**之前**就啟動，跑在自己的背景任務裡，以
  `catch_unwind` 與獨立的錯誤處理包住，擋得住其他功能回傳的錯誤與非主執行緒的 panic。主執行緒 panic（結束碼 101）、
  abort 與原生當機會連同更新任務一起結束，`catch_unwind` 擋不住——小工具與桌布正好建在主執行緒的 setup 裡。
- **當機迴圈保護**（補上一點擋不住的情況）：
  - 標記 `%LOCALAPPDATA%\tw.fintools.fc-host\startup-marker`（內容為版本與時間）**只由主行程讀寫**：取得執行個體鎖、
    確定是主行程之後才讀；Secondary（使用者再點一次、`--autostart`、`--restarted`、`--wait-exit` 的新行程撞到鎖）、
    `--restore-wallpaper`、`--fetch-once` 完全不碰。否則正常執行的前 5 分鐘標記本來就在，任何重複啟動都會被誤判成當機。
  - 主行程讀完後寫入新標記，存活 5 分鐘後刪除。系統匣「結束」、工作階段結束（登出、關機）、因更新結束這些正常結束
    路徑也都刪除標記——只有當機（panic、abort、原生當機、被強制終止）會留下它。
  - 主行程讀到舊標記仍在（上次啟動不到 5 分鐘就非正常結束）時：setup **不阻塞**，但先不建立小工具與桌布協調，只啟動
    更新任務並立即檢查（等網路與下載合計上限 60 秒）；更新任務回報「有新版、開始安裝」就走 D4（此時沒有 UI，收尾略過
    協調迴圈與渲染 browser，只經主執行緒移除系統匣圖示、有時限）；回報「沒有新版／失敗／逾時」後才建立小工具與桌布
    協調。主執行緒全程不等待，避免 `install()` 的 `on_before_exit` 需要主執行緒時死鎖。
  - **過渡期狀態機**：系統匣圖示在 setup 前後就已建立，60 秒過渡期間使用者已能操作系統匣，也可能再點一次開始功能表。
    以一個原子狀態表示過渡期：初始為「等待」，只能從「等待」轉移**一次**，三選一：
    - **安裝**：更新任務回報「有新版、開始安裝」時轉移。之後不再建立 UI；走 D4 的「未建 UI」收尾。若 `install()` 在
      呼叫 `on_before_exit` 之前就失敗（寫暫存檔失敗），視為「沒有新版」，改轉移到下一項的行為（建立 UI）——因狀態只能
      轉移一次，實作上「安裝」狀態內保留一個「改走建立 UI」的退路，由同一個呼叫端在失敗時執行。
    - **建立 UI**：更新任務回報「沒有新版／失敗」或 60 秒逾時時轉移。之後才建立小工具與桌布協調；晚到的更新結果一律照
      一般規則處理（`--autostart` 啟動的 10 分鐘自動安裝規則，或只通知），收尾走完整的 D4。沒有舊標記的一般啟動也從
      「建立 UI」起始（setup 同步建立，建完才轉到「UI 已建立」）。UI 建好之前符合自動安裝條件的更新**延後、不丟棄**：
      先不安裝也不通知，UI 建好後在背景執行緒重新評估（10 分鐘窗口以下載完成時的啟動時間判斷——「啟動後 10 分鐘內
      找到並下載完」；退避、狀態與安裝互斥重查；不裝就只通知；不在主執行緒同步安裝，否則 `on_before_exit` 等主執行緒
      會死鎖）；建立途中轉到「結束」則丟棄。
    - **結束**：使用者在過渡期按系統匣「結束」時轉移。取消更新任務；不還原桌布、不存主題（協調迴圈尚未建立，語意同
      工作階段結束）；刪除當機標記後結束。
    - 「等待」期間收到的 single-instance 交接（例如開設定視窗）先排隊，轉移到「建立 UI」後再處理；轉移到「安裝」或
      「結束」則丟棄並記錄。
    - 例外：「等待」或「建立 UI」（UI 尚未建好）時收到 `--restore-wallpaper` 的交接（解除安裝），不排隊——此時沒有協調
      迴圈可交，宿主又不結束，命令列等不到宿主結束就會放棄還原。改為回報「交接失敗」並比照「結束」處理（取消更新任務、
      不還原、不存主題、刪標記、結束），命令列等宿主結束後在自己的行程還原，總時長仍在命令列的總時限內。「安裝」與
      「UI 已建立」時照原本的交接處理。
    - 例外（與「安裝」的退路同性質，不是第二次離開「等待」）：「建立 UI」已決定、但 UI 尚未建好時按系統匣「結束」，比照
      「結束」處理（不還原、不存主題、刪標記），已投遞的建 UI 工作發現正在結束就略過；已在「結束」時重複按則忽略。
      沒有舊標記的一般啟動同樣適用（setup 同步建立 UI 期間建立小工具的巢狀訊息泵可能派送選單事件）；這是動態桌布
      「系統匣『結束』一律存成『不接管』」的例外（AGENTS.md 同條已註明），桌布狀態與主題維持原樣、下次啟動照常。
      `install()` 在 `on_before_exit` **之後**才失敗（例如 `ShellExecuteW` 失敗）時收尾已做完，不回頭建 UI，改走 D4 的
      「以 `--autostart --wait-exit` 重新啟動自己」。
  - 標記在 `main()` 仲裁成主行程之後、`tauri::Builder` 之前寫入，Builder 或 WebView2 初始化時當機也能被抓到。
  - 這樣新版在啟動時穩定崩潰時，修正版仍能在下一次啟動送達。代價：當機後的下一次啟動，小工具最多晚 60 秒出現。
  - 已知限制：工作階段結束時暫停標記的處理掛在建立 UI 時才建立的守門視窗上，過渡期內登出或關機不會暫停標記；下次
    登入又被當成當機，小工具最多再延後 60 秒出現。時間窗只有當機後重啟的頭 60 秒，接受現狀（最終審查 M2b）。
- 安裝時機：系統匣通知一次＋選單項「更新到 vX.Y.Z 並重新啟動」。以 `--autostart` 啟動（登入）時，小工具照常建立，
  更新檢查在網路就緒後立即在背景執行（不套用 60–180 秒延遲）；啟動後 10 分鐘內找到並下載完就自動安裝，超過就只通知。
  一般執行期間不自行開始安裝——更新會讓小工具消失數秒，桌布接管也會暫停。
- 選這個折衷的理由：只靠選單，不點的使用者可能永遠停在舊版；隨時在背景安裝，使用者會看到小工具突然消失。登入後
  幾分鐘內使用者通常還沒開始專注工作，此時安裝最不突兀。全程非同步，不卡住 setup 與單一執行個體的交接。
- **防止自動安裝循環**：重新啟動一律帶 `--autostart`，若安裝被中止或版本號錯置（`latest.json` 的版本高於實際裝上的
  版本），每次重啟都會再自動安裝。因此自動安裝前把「嘗試的版本與時間」寫進 `%LOCALAPPDATA%\tw.fintools.fc-host\update-state.json`；
  啟動後若目前版本仍低於上次嘗試的版本，且距該次嘗試未滿 24 小時，只通知、不自動安裝，並記錄一筆錯誤。使用者手動點選
  不受此限，但手動安裝同樣寫入嘗試紀錄，手動安裝失敗後下次登入也會正確退避。當機迴圈保護的同步安裝同樣受此限。
- 逾時：`UpdaterBuilder::timeout` 只管 `latest.json` 的請求（設 60 秒）；下載不繼承它，以 `Update::timeout` 設 10 分鐘，外層再加保險；逾時視同失敗，下個週期再試（task 3.1 依外掛原始碼查證）。
- 重新啟動參數：外掛預設以「目前行程的命令列參數」重新啟動（`updater.rs` 的 `current_exe_args` 是 `pub(crate)`，
  無法覆寫）。改用 `restart_after_install(false)`，自己以 `installer_args` 帶 `/R /ARGS --autostart`（`GetOptions` 讀
  `/ARGS` 取到下一個 `/` 為止，`--autostart` 不含 `/`，可行；task 3.2 引用原始碼行號並以 e2e 驗證）。理由：更新後重新
  啟動的一律視為「登入」啟動，行為可預期（不跑首次啟動流程）。副作用：新版啟動後會立即檢查更新；它已是最新版，
  不會連續更新（版本錯置的情況由上一點擋下）。`RunAsUser` 重新啟動的行程拿到真實環境，資料抓取的隔離偵測判為非隔離，
  符合預期。
- 安裝模式 `passive`（小視窗進度條、不需互動），設定鍵是 `plugins.updater.windows.installMode`（放在
  `plugins.updater` 底下會被靜默忽略，外掛 `config.rs`）。
- `requireSignedVersion = true`：簽章需帶版本號，防止「新版本號配舊版安裝檔」的降版攻擊。簽章由 D6 的獨立步驟以
  `tauri signer sign --app-version` 產生；打包流程解碼 `.sig` 確認可信註解含版本，不含就失敗——否則所有使用者的更新都會
  靜默驗證失敗。

### D4. 更新前的結束流程

外掛 `install()` 的順序是：把安裝檔寫到暫存檔 → 呼叫 `on_before_exit` → `ShellExecuteW` 啟動安裝檔 → `exit(0)`
（`updater.rs:946-979`），不經過 Tauri 的結束事件。

- `install()` 不得在主執行緒呼叫（收尾要經 `run_on_main_thread` 關視窗，在主執行緒上呼叫會死鎖），在背景任務呼叫。
- 收尾放在 `on_before_exit` 裡，依下表順序執行；「可跳過」的步驟逾時就放棄、繼續下一步；「必做」的步驟本身有界，
  一定執行。各步上限加總 18 秒，整段**不超過 20 秒**。e2e 記錄實測時長。

  | 順序 | 步驟 | 上限 | 性質 |
  |---|---|---|---|
  | 1 | 解除 `RegisterApplicationRestart`（避免 Restart Manager 關閉時被系統重新啟動） | 即時 | 必做 |
  | 2 | 寫出尚未存檔的設定（實作前先確認是否存在延後存檔；目前 `settings::save` 都是當場寫，若沒有就記為「無事可做」並刪除此步；若保留，取 `Mutex<Settings>` 改用 `try_lock` 輪詢） | 2 秒 | 必做 |
  | 3 | 桌布協調迴圈「因更新結束」（見下；參考 `ExitLimits` 的 redraw 等待量級，不還原所以不需要 restore 時限） | 5 秒 | 可跳過 |
  | 4 | 停止資料抓取（data-layer-rust 排程的停止介面，在下一個來源邊界停） | 2 秒 | 可跳過 |
  | 5 | 經 `run_on_main_thread` 關閉所有 WebView2 視窗並移除系統匣圖示（`NIM_DELETE`；`exit(0)` 不會替我們做） | 3 秒 | 可跳過 |
  | 6 | 等渲染用的 browser 行程結束（可能卡在核心拆除、永遠等不到，memory `webview2-same-udf-env-binds-to-live-browser`） | 5 秒 | 可跳過 |
  | 7 | 刪除當機標記、記錄一行「因更新結束」並 flush 記錄檔 | 1 秒 | 必做 |

  尚未建立 UI 時（當機迴圈保護的同步安裝），步驟 3、6 直接略過；步驟 5 只移除系統匣圖示（圖示在 setup 之前就已
  建立，不移除會在 `exit(0)` 後留下殘影），經 `run_on_main_thread`、上限同為 3 秒。過渡期主執行緒全程不等待，所以
  不會死鎖；主執行緒忙碌時逾時放棄，結果寫進收尾記錄（最終審查 M1）。之後 NSIS 的 Restart Manager 只是保險。
- 失敗處理：寫暫存檔失敗時 `on_before_exit` 還沒跑，`install()` 回傳錯誤，宿主照常運作、記錄失敗、下個週期再試。
  `ShellExecuteW` 失敗時收尾已經做完，`install()` 回傳錯誤，宿主此時沒有小工具也沒有桌布協調——改為以
  `--autostart --wait-exit <本行程 PID>` 啟動一個新的自己，記錄失敗，然後**立刻以 `std::process::exit` 結束**本行程。
  不用 `app.exit`：它會走 `ExitRequested`，可能誤入系統匣「結束」的還原桌布路徑。
- `--wait-exit <pid>` 在 `main()` 的啟動仲裁**之前**處理（Win32 呼叫依 AGENTS.md 放在 `desktop.rs`）：
  - `OpenProcess(SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION)` 失敗＝視為已結束，直接進入仲裁。
  - 拿到 handle 後先確認它是舊宿主：`QueryFullProcessImageNameW` 的路徑與自己相同，且 `GetProcessTimes` 的建立時間早於
    自己；不符（PID 被重用）就不等，直接進入仲裁。
  - 符合就 `WaitForSingleObject`，上限 30 秒，之後進入仲裁。
  - 必要性：`instance.rs` 取不到執行個體鎖時會直接當 Secondary 轉交參數後退出、不會等待，而舊行程收到 `--autostart`
    會被 `tray.rs` 靜默忽略，不等待的話最後沒有任何宿主在跑。舊行程在啟動新行程後立刻 `std::process::exit`，實務上
    不會逾時；萬一逾時，新行程會成為 Secondary 退出，記錄錯誤，宿主要等下次登入才回來（已知風險）。
- 已知風險：安裝檔已啟動、但在 `CheckIfAppIsRunning` 等步驟中止（使用者取消、檔案被鎖）時沒有回滾，宿主不會被重新
  啟動，要等下次登入（Run 值仍在）才回來。

協調迴圈目前只有兩條結束相關路徑：系統匣「結束」（還原並把主題存成不接管）與工作階段結束（不還原，但迴圈不結束、
等待取消）。本 change 新增要求「因更新結束」：語意同工作階段結束（不還原、不動狀態檔與主題），但處理完就讓迴圈結束並
回報，有時限（沿用 `ExitLimits` 的量級）。資料抓取的停止依賴 data-layer-rust 排程任務的停止介面，所以本 change 在
data-layer-rust 完成後才開工。

### D5. 簽章金鑰

- **正式金鑰**：以 `tauri signer generate` 產生，私鑰放 `%USERPROFILE%\.fc-host-signing\release.key`（repo 外，資料夾與
  私鑰的 ACL 只留使用者本人）；隨機密碼只存在使用者的密碼管理器，不落地成檔案（2026-10-05 產生時決定）。公鑰寫進 `tauri.conf.json` 的
  `plugins.updater.pubkey`。v0.1.0 發出去後公鑰就寫死在使用者的程式裡，私鑰遺失就再也無法推送更新。
- **使用者待辦**：私鑰與密碼放進 GitHub 的 Environment `release`（設審核者，只有該 Environment 的 job 讀得到），
  不放 repo 層級 secrets；以 tag ruleset 限制只有維護者能建立 `v*` tag；私鑰離線備份。
- **換鑰**：用舊私鑰簽一個「內建新公鑰」的過渡版，使用者更新到過渡版後，之後的版本改用新私鑰簽。
- **私鑰外洩**：立刻以同樣方式發過渡版換鑰；在此之前攻擊者可偽造更新，`requireSignedVersion` 只防降版、不防這種情況。
- **測試金鑰**：e2e（D7）另產一組拋棄式金鑰放 `%TEMP%`，只用於 e2e 設定檔；正式私鑰**不**用來簽任何測試建置。

### D6. 打包流程

- 步驟順序（為日後插入 Authenticode 預留）：建置 `fc-host.exe` →（日後：簽 exe）→ NSIS 封裝 →（日後：簽安裝檔）→
  **最後**以 `tauri signer sign --app-version <版本>` 對最終安裝檔產生 `.sig`。v0.1.0 就把產生 `.sig` 做成獨立步驟、
  不用 `createUpdaterArtifacts` 順便產。注意目前的 job 切分**沒有**現成的 Authenticode 插入點：簽 `fc-host.exe` 要在封裝前
  （build job 內，但 build 不碰 secret），簽安裝檔若放在 publish 又在冒煙測試之後。日後導入 SignPath 時，要在 build 與 smoke
  之間新增一個走 Environment 審核的 sign job（先簽 exe 再重新封裝、簽安裝檔，再交給 smoke 與 publish）——屆時另開 change
  （收尾線發版審查指出）。
- 本機：`host/tools/package.ps1` 下載 tauri-cli 預編譯執行檔（**主機架構**的 `cargo-tauri-<主機架構>-pc-windows-msvc.zip`，
  釘版本並核對 SHA256）到使用者快取目錄，執行 `cargo tauri build [--target ...]`、改成發佈檔名、產 `.sig` 並驗證可信註解
  含版本、搜尋後門字串（D7）。不需要 Node，也不改變日常的 `cargo build`。
- CI：`.github/workflows/release.yml` 在推送 `v*` tag 時觸發：
  - 建置：在 `windows-latest` 上建 x64 與 ARM64（`--target aarch64-pc-windows-msvc` 交叉編譯為主路徑——tauri-cli 與 NSIS
    都是 x86 執行檔；`windows-11-arm` 原生建置為備案）。
  - 冒煙測試：x64 在 `windows-latest`、ARM64 在 `windows-11-arm` 上，各自 `/S` 靜默安裝 → 寫入含 `"data_fetch": "off"`
    的設定檔（runner 的 `LOCALAPPDATA` 是真實路徑，否則宿主會判為非隔離而抓外網）→ 啟動 → 存活 60 秒 → 記錄檔有啟動行
    → `/S` 解除安裝 → 無殘留宿主行程。job 設排隊與執行逾時（`timeout-minutes`）。任一失敗就不建立 release。
  - 豁免：`windows-11-arm` 不可用時不能讓修正版被卡住。workflow 另提供 `workflow_dispatch` 輸入 `waive-arm64-smoke`，
    使用時跳過 ARM64 冒煙、在 release notes 與發佈清單標記「ARM64 未冒煙」，由收尾線人工決定是否發佈。x64 冒煙不可豁免。
  - 待查證（5.2 第一次實跑時確認）：託管 runner 是否有可讓 WebView2 建立視窗的互動式桌面工作階段；沒有的話，冒煙測試
    改以「行程存活＋記錄檔啟動行」為準，並註明視窗未驗。
  - 發佈：最後一個 job（Environment `release`，讀簽章私鑰）產 `.sig`、用 `host/tools/make-latest-json.ps1` 組出並檢查
    `latest.json`（平台網址用 `releases/download/vX.Y.Z/` 固定版本網址，不用 `latest`），建立 **draft** release 並上傳全部
    產物。收尾線依發佈清單（含「x64 實機安裝並啟動」）人工檢查後才發佈，並設為 Latest。
  - 發佈後驗證：另一個以 `release: released` 觸發的 job（prerelease 轉正式版不會觸發 `published`，發版線審查改用
    `released`；由 draft 直接發佈是否觸發待第一次發版確認，沒觸發就改回 `published` 加 prerelease 過濾）從
    `releases/latest/download/latest.json` 下載兩個平台的安裝檔、以公鑰驗簽、比對版本，失敗就通知（開 issue，同名去重）。
- 不用 `tauri-action`：兩個架構的 job 會各自更新 `latest.json`，有競態；而且它不能直接產出我們固定的檔名。
- tag 與版本：CI 檢查 tag 版本與 `Cargo.toml` 的版本一致，不一致就失敗。

### D7. 本機端對端驗證

- **e2e 設定檔**：`host/tauri.e2e.conf.json`（以 `cargo tauri build --config` 疊加）把端點改成
  `http://127.0.0.1:<port>/latest.json`、開 `dangerousInsecureTransportProtocol`、換成測試公鑰。端點在建置時就寫死，
  所以 NSIS 以 `RunAsUser` 重新啟動等不繼承環境變數的路徑也走得到。「登入觸發」以直接執行 `fc-host.exe --autostart`
  模擬（不實際登出登入：使用者不在場，且登出會結束收尾線的工作）；`--autostart` 的程式路徑與 Run 值啟動相同。
- **防止後門漏進正式建置**：`dangerousInsecureTransportProtocol` 是設定檔層級，Rust feature 擋不住，所以兩道檢查：
  宿主啟動時若讀到這個旗標為真、但建置不含 `update-e2e` feature，就停用更新器並記錄錯誤；`package.ps1` 與 release
  workflow 在正式打包後搜尋執行檔，出現 e2e 設定檔中的完整端點網址或測試公鑰字串就失敗（不搜裸 `127.0.0.1`，依賴庫
  可能本來就含這個字串）。
- 測試金鑰是拋棄式：每次 e2e 重新產生並改寫 `host/tauri.e2e.conf.json` 的公鑰，該檔提交時公鑰欄位放占位字串。
- 流程：以 e2e 設定檔加 `update-e2e` feature 打包 0.1.0 與 0.1.1（只改版本號），本機 http 伺服器提供 0.1.1 的
  `latest.json` 與安裝檔；安裝 0.1.0 → 選單觸發與登入觸發兩條路徑 → 驗證沒有 UAC、自動重啟帶 `--autostart`、開機自啟
  值與設定保留、桌布沒有被還原、沒有殘留行程；竄改安裝檔時不安裝；`latest.json` 宣告 0.1.2 但提供 0.1.1 的安裝檔時
  （版本錯置），第二次啟動只通知、不再自動安裝。
- 這會改動真正的使用者資料：安裝目錄 `%LOCALAPPDATA%\fc-host`、設定與桌布狀態 `%APPDATA%\tw.fintools.fc-host`、
  資料與記錄 `%LOCALAPPDATA%\tw.fintools.fc-host`、HKCU 的 `Run` 與 `StartupApproved\Run`、解除安裝鍵、
  `HKCU\Software\<manufacturer>\fc-host`（範本寫入）、`Control Panel\Desktop` 的 `Wallpaper`／`WallpaperStyle`／
  `TileWallpaper`、目前的桌布。執行前全部快照（目錄複製、登錄匯出、桌布讀回），執行後解除安裝並還原、比對；事前
  通知收尾線，不與它的實機驗收同時進行。

### D8. ARM64 與檔名

- `finance-calendar-setup.exe` 是 x64、`finance-calendar-setup-arm64.exe` 是 ARM64。更新器以執行檔本身的架構
  （`cfg!(target_arch)`）找 `latest.json` 的平台鍵，裝哪一版就一直更新哪一版——ARM64 使用者若裝了 x64 版，就會永遠以
  模擬執行，而模擬執行正是當初離開 Wallpaper Engine 的原因。
- **v0.1.0 的發版前置條件**（收尾線負責）：著陸頁以 `navigator.userAgentData.getHighEntropyValues(['architecture'])`
  偵測，ARM 就把主按鈕換成 `-arm64.exe`；取不到時退回 x64，並保留兩個架構的文字連結。

## Risks / Trade-offs

- [未簽章：瀏覽器下載會跳 SmartScreen；開了 Smart App Control 的電腦可能直接擋下，且沒有「仍要執行」，全新安裝的
  Win11（含 ARM64 機種）預設常開 SAC] → README 與著陸頁說明；v0.1.0 發佈後立即申請 SignPath Foundation，優先度高。
  自動更新下載的檔案沒有網路來源標記，預期不會跳 SmartScreen（由原始碼推論，D7 實測確認）。
- [私鑰遺失就再也無法推送更新；外洩就能偽造更新] → D5 的 Environment 審核、離線備份、換鑰過渡版。
- [發出壞版本、所有人卡死] → D6 冒煙測試、D3 更新檢查隔離、draft 人工檢查。
- [`releases/latest` 被不含 `latest.json` 的 release 搶走] → Lively 已凍結；AGENTS.md「發布」寫明規則；D6 發佈後驗證
  job 會發現；必要時在 `endpoints` 加第二個備援網址。
- [NSIS 以 Restart Manager 關閉宿主時，與 `RegisterApplicationRestart`、WebView2 行程交互] → D4 先自行結束；D7 實測。
- [安裝在中途被中止時沒有回滾] → 等下次登入；D4 已列。
- [勾選「刪除應用程式資料」時，WebView2 仍握著檔案可能刪不乾淨] → README 說明；D4 的收尾讓這種情況少見。
- [`SHGetKnownFolderPath` 展開使用者資料夾時可能參考呼叫端的 `USERPROFILE`，驗收腳本若同時改了它，資料抓取的隔離偵測
  會失效] → 待查證；收尾線已確認現有腳本不改 `USERPROFILE`（data-layer-rust D10）。
- [安裝檔含字型等資源，每次更新完整下載數十 MB] → 頻率低，可接受；差量更新列 backlog。
- [ARM64 沒有本機實機驗證] → CI 冒煙測試涵蓋安裝與啟動；列已知缺口，有 ARM64 使用者回報再補。

## Migration Plan

1. 開發機上 `HKCU\…\Run\fc-host` 若仍指向開發建置，D7 測試前先快照，測完還原（快照範圍見 D7）。
2. hook 必須一次做對：手動覆蓋安裝跑的是**舊版**的解除安裝程式，v0.1.0 的 hook 一旦寫錯，之後的版本修不回來。
3. 發版（收尾線）：使用者設定 Environment `release` 與 tag ruleset → 著陸頁架構偵測上線 → 推 `v0.1.0` tag → CI 建置、
   冒煙測試、draft → 依發佈清單檢查（含 x64 實機安裝並啟動）→ 發佈並設為 Latest → 發佈後驗證 job 通過。
4. 舊 Lively 使用者沒有自動遷移：著陸頁改為新版下載；Lively 版 release 保留不刪。
5. 退回：發布版本號更高的修正版（更新器只升不降）；最壞情況是使用者從著陸頁重裝。
