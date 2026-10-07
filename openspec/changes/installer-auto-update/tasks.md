# Tasks

> 執行路徑：SDD｜理由：跨建置設定、NSIS hook、宿主結束流程、CI workflow，且安裝／更新會動到真正的使用者設定檔與登錄，
> 需要逐 task 審查與實機驗收。
>
> 審查路由：逐 task 審查與最終全分支審查一律由 Opus subagent 執行，不用 Codex（使用者 2026-10-02 指示）。發版相關
> （D5、D6、D8）另請收尾線以發版角度複審。
>
> 開工前提：data-layer-rust 全部完成（共用 TLS 組態；3.3 需要資料抓取的停止介面）。
> 實機安裝／更新（4.x）前通知收尾線（session `finance-calendar-c9`），不與它的實機驗收同時進行。
> 著陸頁架構偵測（design.md D8）歸收尾線，是 v0.1.0 的發版前置條件，不在本 change 的 tasks 內。

## 1. 金鑰與設定

- [x] 1.1 以 tauri-cli 預編譯執行檔（主機架構、釘版本、核對 SHA256，放使用者快取目錄）執行 `signer generate` 產生正式
  金鑰：私鑰 `%USERPROFILE%\.fc-host-signing\release.key`、隨機密碼只存密碼管理器（design.md D5）；
  公鑰寫入 `host/tauri.conf.json` 的 `plugins.updater.pubkey`；交接文件寫明使用者待辦（Environment `release` 與審核者、
  tag ruleset、密碼管理器、離線備份）與 design.md D5 的換鑰／外洩程序
- [x] 1.2 `tauri.conf.json`：加 `bundle`（design.md D2：currentUser、TradChinese、downloadBootstrapper、
  `createUpdaterArtifacts: false`、hooks）、刪除 `version`（由 `Cargo.toml` 提供，確認 `cargo build` 與 `cargo tauri build`
  都讀得到）、`plugins.updater`（GitHub 端點、`windows.installMode: passive`、`requireSignedVersion`）；
  `host/tauri.e2e.conf.json`（D7：127.0.0.1 端點、允許 http、公鑰占位字串）；`cargo build --release` 仍可純 cargo 建置、
  行為不變

## 2. 安裝檔

- [x] 2.1 `host/installer/hooks.nsh`（D2，存成 UTF-8 含 BOM，加檢查）：PREINSTALL 記下既有安裝；POSTINSTALL 每次覆寫
  `DisplayName` 與改名開始功能表捷徑（只在來源 `${PRODUCTNAME}.lnk` 存在時先刪目標再改名）、首次安裝才寫 Run 值；PREUNINSTALL（非更新）以 `ExecWait` 呼叫
  `--restore-wallpaper` 並依結束碼處置、寫 `uninstall.log`、不強制終止；POSTUNINSTALL（非更新）刪「財經日曆.lnk」與
  `StartupApproved\Run` 的 `fc-host`。每一處引用 tauri-cli-v2.12.1 `installer.nsi` 的行號。手動覆蓋安裝跑的是舊版解除
  安裝程式，v0.1.0 的 hook 寫錯就修不回來——首次安裝、更新、同版本覆蓋、解除安裝四種路徑都要在 4.x 實測
- [x] 2.2 `host/tools/package.ps1`（D6）：下載並快取主機架構的 tauri-cli、`cargo tauri build`（可選 `--target`、
  `--features`、`--config`）、改成發佈檔名、以獨立步驟 `tauri signer sign --app-version` 產 `.sig` 並解碼確認可信註解含
  版本、正式打包時搜尋執行檔中的 e2e 端點網址與測試公鑰；本機產出 x64 安裝檔並記錄大小；附腳本自測

## 3. 宿主端更新

- [x] 3.1 `host/src/updater.rs`：外掛接線（`default-features = false`＋`native-tls`、`system-proxy`）；在 setup 一開始、
  建立小工具與桌布協調之前啟動，自己的背景任務＋`catch_unwind` 隔離（D3）；當機迴圈保護（D3：`startup-marker` 只由取得執行個體鎖的
  主行程讀寫，`--restore-wallpaper`／`--fetch-once`／Secondary 不碰；存活 5 分鐘或正常結束時刪除；標記仍在時 setup 不阻塞、
  延後建立小工具直到更新任務回報，上限 60 秒；標記在仲裁成主行程之後、`tauri::Builder` 之前寫入；過渡期以原子狀態機
  處理（等待→安裝／建立 UI／結束只轉移一次、交接請求排隊、安裝前失敗改走建立 UI）；單元測試涵蓋「Secondary 不讀標記」
  「正常結束刪標記」、三種轉移與「逾時後下載才完成」）；檢查排程（一般啟動 60–180 秒延遲後每 6 小時；
  `--autostart` 在網路就緒後立即檢查）、背景下載（10 分鐘逾時）與驗簽、同一版本只通知一次、失敗只記錄；
  `--fetch-once`／`--restore-wallpaper` 不檢查；設定允許 http 但建置不含 `update-e2e` 時停用更新器；單元測試以注入的
  檢查結果涵蓋 spec 情境；`cargo tree` 確認仍只有一套 TLS
- [x] 3.2 安裝觸發：系統匣選單項「更新到 vX.Y.Z 並重新啟動」與系統匣通知；`--autostart` 啟動後 10 分鐘內下載完成就
  自動安裝，否則只通知；`update-state.json` 防循環（D3：上次嘗試的版本仍高於目前版本且未滿 24 小時→只通知；手動安裝
  也寫入），單元測試涵蓋；`install()` 在背景任務呼叫、不在主執行緒；重新啟動參數改用 `restart_after_install(false)` 加 `installer_args`
  的 `/R /ARGS --autostart`，引用 `installer.nsi` 的 `/ARGS` 解析行號
- [x] 3.3 更新前結束流程（D4）：`on_before_exit` 依 D4 的時間預算表（必做／可跳過、整段 ≤20 秒、尚未建立 UI 時略過
  主執行緒步驟）——協調迴圈「因更新結束」（新增要求：不還原、不動狀態檔
  與主題、處理完迴圈結束並回報）、停止資料抓取（data-layer-rust 5.2 的停止介面）、經 `run_on_main_thread` 關閉 WebView2
  並等渲染 browser 結束、移除系統匣圖示、強制寫出延後存檔的設定、解除 `RegisterApplicationRestart`、記錄並 flush；
  `install()` 回傳錯誤時依 D4 的失敗處理（舊行程以 `std::process::exit` 結束；新增 `--wait-exit <pid>`：開不了視為已結束、驗證映像路徑與建立時間、
  上限 30 秒；單元測試涵蓋 PID 被重用）。單元測試確認「因更新結束」不走系統匣「結束」的還原與存主題路徑
- [x] 3.4 `update-e2e` feature：只負責放行 D7 的 e2e 設定檔（宿主端檢查見 3.1）；正式建置（無 feature）在 e2e 設定檔下
  更新器停用的單元測試

## 4. 實機驗收（會動真正的使用者資料；事前快照、事後還原、通知收尾線）

- [x] 4.1 快照與還原腳本：design.md D7 列出的全部目錄（複製）與登錄值（匯出）、各螢幕桌布讀回；還原後自動比對並輸出
  差異
- [x] 4.2 安裝與解除安裝：無 UAC；首次安裝寫 Run 值並啟動宿主；「應用程式與功能」與開始功能表顯示「財經日曆」；接管桌布
  中解除安裝會還原桌布、移除 Run 值、`StartupApproved\Run` 的 `fc-host`、捷徑與程式資料夾，沒有殘留宿主行程、WebView2
  行程 30 秒內結束；設定預設保留；同版本直接覆蓋不改 Run 值（關閉狀態下覆蓋後仍關閉）、只有一個捷徑；手動覆蓋安裝
  （先解除舊版）的行為符合 spec「手動下載新版覆蓋安裝」——**完成（2026-10-06 在場第二段，證據 `installer-s2-*`）**：
  互動首次安裝（完成頁「執行」）、同版本互動重裝（預設＝直接覆蓋；關閉自啟後仍關閉）、手動覆蓋新版（預設＝安裝前先解除
  安裝）；passive 部分見 4.x 證據。發現兩項另案：桌面捷徑名稱為 fc-host（使用者決定改「財經日曆」，分支
  `fix/installer-desktop-shortcut-name`）；手動覆蓋後 Run 重新登錄但 `settings.json` 的 `autostart` 仍為 false（待裁定）
- [x] 4.3 更新端對端（D7）：0.1.0→0.1.1，選單觸發與登入觸發（以直接執行 `--autostart` 模擬）兩條路徑；每次 e2e
  重新產生測試金鑰；驗證無 UAC、重新啟動帶 `--autostart`、Run 值（開與關兩種狀態）與設定保留、`DisplayName` 維持
  「財經日曆」且開始功能表捷徑仍在、接管中桌布沒被還原且新版繼續接管、因更新結束的實測時長（≤20 秒）、沒有重複的宿主行程、系統匣沒有殘留圖示、舊版的 WebView2 行程
  30 秒內結束；竄改安裝檔一個位元組時不安裝；版本錯置時第二次啟動只通知；手動放入當機標記（startup-marker）再啟動時，過渡期狀態機在建立小工具前完成檢查、無新版時照常建立 UI；更新器下載的安裝檔沒有網路來源標記
  （`Zone.Identifier`）；手放當機標記＋latest 宣告較新版本：不建小工具、收尾記錄尚未建立 UI、新版接手、系統匣只有一個
  圖示——**完成（2026-10-06）**：系統匣選單觸發（收尾 131 ms、使用者目視 1 個圖示、自啟關閉狀態保留）與過渡期同步安裝
  （收尾 4 ms「只移除系統匣圖示」、使用者目視 1 個圖示）於在場第二段；登入觸發含延後路徑見 `installer-4x-deferred-*`；其餘見 4.x 證據
- [x] 4.4 結束後以 4.1 的腳本還原並比對，與執行前一致

## 5. 發佈流程

- [x] 5.1 `host/tools/make-latest-json.ps1`：讀兩個 `.sig`、組出 `latest.json`（`releases/download/vX.Y.Z/` 固定網址）、
  檢查欄位完整、簽章非空、網址指向同一個 tag、簽章可信註解含版本；附自測（缺平台、空簽章、網址 tag 不符、可信註解
  缺版本四種失敗）
- [x] 5.2 `.github/workflows/release.yml`（D6）：tag 觸發（`workflow_dispatch` 手動觸發時須以該 tag 為 ref 執行，
  版本一致檢查才讀得到 tag）、版本一致檢查、`windows-latest` 建 x64 與交叉編譯 ARM64（不帶
  e2e 設定與 feature，執行 2.2 的後門搜尋）、兩個架構各自的冒煙測試 job（x64 在 `windows-latest`、ARM64 在
  `windows-11-arm`；安裝後先寫 `data_fetch: "off"` 再啟動；設 `timeout-minutes`；`workflow_dispatch` 的
  `waive-arm64-smoke` 豁免與 release notes 標記；第一次實跑確認 runner 是否有互動式桌面）、Environment `release` 的發佈 job（產 `.sig`、組 `latest.json`、建立 draft release）；另一個
  `release: published` 觸發的驗證 job（下載 latest 的兩個安裝檔、驗簽、比對版本，失敗開 issue）；以 `actionlint`
  （預編譯執行檔，不安裝到系統）檢查；交叉編譯不可行時改 `windows-11-arm` 原生建置
- [x] 5.3 文件：README 安裝與更新說明（含未簽章與 Smart App Control 提示、手動覆蓋安裝後要重選主題、刪除應用程式資料
  可能刪不乾淨）、AGENTS.md「發布」改為新流程、發佈清單（含「x64 實機安裝並啟動」）與「不得發佈不含 `latest.json` 的
  release」規則、`host/README.md` 打包說明；`openspec validate installer-auto-update --strict` 通過
- [x] 5.4 品質 gate：`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`（另跑 `--features self-test-ipc` 與
  `--features update-e2e`）、`cargo test`、改過的 `.md` 跑 markdownlint
