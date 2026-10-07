# fc-host — 桌面小工具宿主

Rust／Tauri 2 常駐程式，取代 Lively Wallpaper：把財經儀表板拆成多個獨立置底的桌面小工具視窗。
架構背景見 `AGENTS.md`「桌面小工具宿主（host/）」一節；行為依據見
`openspec/changes/desktop-widget-host/design.md`、`specs/`。

`README-spike.md` 是 task 1.x spike 階段（`fc://` scheme、命令列 `--dir`/`--monitor` 等）的
歷史記錄，那些旗標已在 task 2.1 重整時移除，僅保留供對照當時驗證過的行為；正式使用方式以本檔
為準。

## 建置

純 cargo，不用 tauri-cli、不用 Node 打包前端（`host/ui/` 是純 ES modules，`frontendDist`
直接指向該目錄）。

```powershell
cd host
cargo build --release      # 產物：host/target/release/fc-host.exe
cargo build                # debug build（Tauri 顯示主控台輸出）
```

## 執行

直接執行 `fc-host.exe` 即可，無強制參數。

- 無參數：正常啟動並建立設定裡開啟的小工具視窗；若已有執行個體在跑（single-instance），改為
  開啟既有執行個體的設定視窗。
- `--autostart`：開機自動啟動時，`autostart` plugin 帶這個參數重新啟動；single-instance 偵測到
  第二個行程帶此參數時靜默忽略（不開設定視窗）。
- `--restarted`：經 Restart Manager 要求重啟時，宿主自己帶這個參數重新啟動；同樣被
  single-instance 靜默忽略。
- `--fetch-once <目錄>`：單次抓取（見下方「資料抓取」），抓完結束，不建立視窗、不經 single-instance。
- `--self-test-ipc --log <path> [--timeout-secs <n>]`：只在以 `--features self-test-ipc` 建置時
  存在，接手整個行程開驗收用測試視窗，不建立正常的小工具視窗與排程。正式 release build（不帶
  該 feature）不含此參數與對應指令。

## 設定檔與記錄檔位置

環境變數不存在時（極端情況）一律退回目前工作目錄，不會讓宿主無法啟動。

| 內容 | 路徑 | 說明 |
|---|---|---|
| 設定檔 | `%APPDATA%\tw.fintools.fc-host\settings.json` | 解析失敗會改名為 `settings.json.bad-<Unix 秒>` 後以預設值啟動 |
| 置底守門記錄 | `%APPDATA%\tw.fintools.fc-host\gatekeeper.log` | 重排次數記錄，驗收「30 秒內恢復」「平常無多餘重排」用 |
| 本機記錄檔 | `%LOCALAPPDATA%\tw.fintools.fc-host\logs\` | 每日輪替、保留 7 天，`log::info!`/`warn!`/`error!` 輸出 |
| 資料目錄（預設） | `%LOCALAPPDATA%\tw.fintools.fc-host\data` | 可在設定改；宿主抓取的輸出 `tw_events.json` 寫在這裡 |
| 抓取排程紀錄 | `%LOCALAPPDATA%\tw.fintools.fc-host\fetch-state.json` | 上一輪開始／完成時間與是否有來源成功；不在資料目錄，壞掉或不存在視同錯過、會補抓 |

## 資料抓取

宿主內建資料抓取（`host/src/fetch/`，Rust 版，行為與 `update_tw_events.py` 等價），不需另外跑 Python
或建立排程。輸出 `<資料目錄>/tw_events.json`（原子寫入，格式與 Python 版相同），小工具由資料目錄的
輪詢讀到。

- **排程**：台北時間每日 00／06／12／15／18 點，到點後 0–5 分鐘隨機開始；資料檔不存在時啟動後
  5–15 秒首抓；宿主啟動、睡眠恢復或錯過時點後 30–120 秒補抓；上一輪沒有成功更新（沒有任何來源
  成功，或寫檔失敗、整輪 panic）時 10–15 分鐘後重試（連續失敗加倍、上限 60 分鐘）。**到點與補抓的
  兩輪開始時間至少相隔 60 分鐘**（延後、不丟棄）；首抓與失敗重試不受此下限。每一輪開始抓取前都會先等
  網路可達（逾時放行）。
- **`data_fetch`（設定檔 `settings.json`）**：`"auto"`（預設）／`"on"`／`"off"`。`off` 不發任何
  抓取請求、不寫 `tw_events.json`；`auto` 在**隔離偵測**成立時等同 `off`——判定為環境變數
  `LOCALAPPDATA` 與系統登記的本機應用程式資料夾（`FOLDERID_LocalAppData`）不同，也就是驗收腳本以
  暫存資料夾隔離宿主的情況（避免測試時誤打實網、誤寫資料）；`on` 一律抓取，含隔離環境。設定視窗的
  資料來源區只顯示抓取狀態與未抓取原因，不顯示也不能切換 `data_fetch`。要在隔離環境實際抓取：
  **先結束宿主 → 改設定檔的 `"data_fetch"` 為 `"on"` → 重新啟動宿主**（宿主只在啟動時讀設定檔，沒有
  檔案監看；宿主執行中在設定視窗存檔會把記憶體中的設定寫回，覆寫手改的值，所以一定要先結束宿主）。
- **`--fetch-once <目錄>`**：抓一輪、把 `<目錄>/tw_events.json`（該目錄已有的同名檔視為上一份輸出，
  來源失敗時沿用）寫入後結束。不看 `data_fetch`、不讀寫排程紀錄、不經 single-instance，所以常駐宿主
  在跑時也能同時執行；用途是實網對照、除錯、排程出問題時手動補救。結束碼：`0`＝寫檔成功（**即使
  所有來源都失敗、寫出的全是沿用的舊資料也是 0**，來源狀況看記錄檔與輸出的 `errors`）；`1`＝寫檔
  失敗、缺目錄參數或目錄建不起來、HTTP client／runtime 建不起來、整輪 panic。
- **記錄**：寫入本機記錄檔（見上表），log target 為 `fc_host::fetch`（開始行、各來源成敗、結束行）；
  排程紀錄位置見上表。

```powershell
fc-host.exe --fetch-once D:\tmp\fetch-out ; $LASTEXITCODE
```

## 打包與安裝檔

`tauri.conf.json` 的 `bundle`（NSIS、currentUser）與 `plugins.updater`（更新端點、公鑰）供
`cargo tauri build` 與日後的更新器使用；日常 `cargo build` 不讀這兩段。`tauri.e2e.conf.json` 只含
本機端對端驗證要覆寫的鍵（`http://127.0.0.1:8737/latest.json` 端點、允許 http、測試公鑰占位字串），
以 `--config` 疊加（提交的檔案公鑰是占位字串，要用的時候複製一份暫存檔、填入測試公鑰），**不得用於正式打包**。

打包一律走 `host/tools/package.ps1`（不需要 Node；第一次會下載並快取釘選版本的 tauri-cli 到
`%LOCALAPPDATA%\fc-host-tools`，核對 SHA256）：

```powershell
pwsh -NoProfile -File host/tools/package.ps1 -SigningKeyPath <私鑰檔> [-SigningKeyPasswordEnv <存密碼的環境變數名>] `
    [-Target aarch64-pc-windows-msvc] [-Features <f>...] [-Config <設定檔>...] [-Release] [-OutDir <資料夾>]
```

CI 把建置與簽章拆開：`-SkipSign`（只建置，不得帶任何金鑰參數）與 `-SignOnly -InstallerPath <安裝檔>`（只簽章，
不建置，檔名必須是發佈檔名）；私鑰也可用 `-SigningKeyEnv <存私鑰內容的環境變數名>` 傳入而不落地（與
`-SigningKeyPath` 擇一）。參數互斥與組合限制由腳本檢查，違反時在建置之前就失敗。

**本機 e2e 打包**（更新流程的端對端驗證用，不是正式打包）：以 `tauri.e2e.conf.json` 的暫存複本加 `update-e2e` feature，
用拋棄式測試金鑰簽章；正式私鑰絕不用來簽測試建置。

```powershell
$k = Join-Path $env:TEMP 'fc-host-e2e-key'; New-Item -ItemType Directory -Force $k | Out-Null
$cli = Join-Path $env:LOCALAPPDATA 'fc-host-tools\tauri-cli-2.12.1\cargo-tauri.exe'   # 先跑過一次 package.ps1 才有快取
& $cli signer generate --ci -w (Join-Path $k 'e2e.key')                               # 產生 e2e.key 與 e2e.key.pub（無密碼）
$j = Get-Content host/tauri.e2e.conf.json -Raw | ConvertFrom-Json
$j.plugins.updater.pubkey = (Get-Content (Join-Path $k 'e2e.key.pub') -Raw).Trim()   # 提交的檔案是占位字串，只改暫存複本
$conf = Join-Path $k 'tauri.e2e.conf.json'
$j | ConvertTo-Json -Depth 5 | Set-Content $conf -Encoding utf8
pwsh -NoProfile -File host/tools/package.ps1 -SigningKeyPath (Join-Path $k 'e2e.key') `
    -Features update-e2e -Config $conf -OutDir (Join-Path $env:TEMP 'fc-host-e2e-out')
```

要建 0.1.0 與 0.1.1 兩個版本做更新測試時，改 `Cargo.toml` 的版本後各打一次（腳本以 `Cargo.toml` 為版本唯一來源）。
產物以 `127.0.0.1:8737` 的本機 http 伺服器提供，流程與會動到的使用者資料見設計 D7。`-Release` 會拒絕這組設定。

更新器在**隔離環境**（環境變數 `LOCALAPPDATA` 與系統登記的不同，例如驗收腳本以暫存資料夾隔離宿主）停用：不檢查、
不下載、不安裝，記錄檔有一行 info 寫明兩個路徑——安裝檔與開機自啟登錄不跟環境變數走，否則隔離的舊版宿主會把正式版裝進
真正的使用者資料夾。`update-e2e` 建置不套用這條，任何環境都能跑更新。

- **tauri-cli 快取的核對**：釘選版本與雜湊寫在 `package.ps1` 的 `Get-PackageConstants`。下載的 zip 核對 SHA256；解壓出的
  `cargo-tauri.exe` 另核對事先記下的 `ExeSha256`——快取命中時也核對，不符就從驗過的 zip 重新解壓、仍不符就失敗（簽章
  時這支程式會讀到私鑰，不能只信解壓後沒被動過的 zip）。升級 tauri-cli 版本時兩個雜湊要一起更新（exe 雜湊＝對驗過的
  zip 解壓後實算）。
- 產物：`finance-calendar-setup.exe`（x64）或 `finance-calendar-setup-arm64.exe`，與同名 `.sig`，預設放
  repo 根目錄的 `dist/`（git 不追蹤）。版本取 `Cargo.toml`，`tauri.conf.json` 刻意不寫 `version`。
- `.sig` 是對最終安裝檔以獨立步驟 `signer sign --app-version` 產生，腳本會解碼確認可信註解的 `version`
  欄位等於版本、`file` 欄位等於發佈檔名，否則失敗（`requireSignedVersion` 開著，缺了它所有使用者的更新
  都會靜默驗證失敗）。私鑰內容與密碼不印出；密碼只經 `-SigningKeyPasswordEnv` 指定的環境變數傳入。
- `-Release` 是正式打包：`plugins.updater.pubkey` 還是占位字串就失敗（正式金鑰尚未產生時這是預期的）、
  不得是 e2e 測試公鑰（與 `%TEMP%` 下 `fc-host-e2e-key` 的 `.pub` 相同或 key ID 相同）、`endpoints` 必須全為
  `https://`、不得有為真的 `dangerous*` 旗標、不接受 `-Config`、不得帶 `update-e2e`、`self-test-ipc`、`probe-render`
  feature（逗號串接與 `fc-host/<feature>` 寫法也擋）；
  簽章後核對 `.sig` 的 key ID 與內建公鑰的 key ID 相同（簽章私鑰與內建公鑰必須是同一把，否則出貨後所有更新
  在使用者端驗簽失敗、且無法靠更新修復）；最後在 `fc-host.exe` 與安裝檔中搜尋 e2e 端點網址與公鑰字串。
  NSIS 安裝檔是整包壓縮，搜尋只對 `fc-host.exe` 有實質效力。
- `installer/hooks.nsh` 要求是 UTF-8 含 BOM（防禦性要求），腳本打包前檢查；hook 的行為與 `installer.nsi` 行號依據寫在檔頭註解。
- 解除安裝的順序提醒：範本的 PREUNINSTALL 插入點早於「確認應用程式是否仍在執行」對話框，所以解除安裝時若在該對話框
  按取消，解除安裝中止，但桌布已經還原、主題已改回「不接管」，要重新選主題。
- `cargo tauri build` 會就地改寫 `host/Cargo.toml`（`tauri-build` 依賴補上 `features = []`），已提交，
  之後的打包不再改動；打包後若 `Cargo.toml` 又有變動，腳本會警告（`-Release` 直接失敗），請檢視 `git diff` 並提交。
- 腳本自測：`pwsh -NoProfile -File host/tools/tests/package.Tests.ps1`（含以 NSIS 編譯小型測試安裝檔、
  用假的 `--restore-wallpaper` 驗證解除安裝 hook 的結束碼處置；找不到 makensis 時略過該部分）。

### CI 發版流程

發版走 `.github/workflows/release.yml`（設計見 `openspec/changes/installer-auto-update/design.md` 的 D5、D6、D8），
發佈後驗證在 `.github/workflows/release-verify.yml`。不用 `tauri-action`（兩個架構會競態更新 `latest.json`，也產不出固定檔名）。

```text
push v* tag（或 workflow_dispatch，須以 tag 為 ref）
  version        tag 必須等於 host/Cargo.toml 的版本
  build（x64、arm64 兩列）  windows-latest：x64 原生、ARM64 以 aarch64-pc-windows-msvc 交叉編譯
                 package.ps1 -Release -SkipSign（不讀私鑰；拒絕 e2e 設定／feature／公鑰，並做後門字串搜尋）
  smoke-x64      windows-latest；smoke-arm64 在 windows-11-arm（ci-smoke.ps1：/S 安裝、data_fetch off、
                 啟動存活 60 秒、記錄檔啟動行、/S 解除安裝、無殘留行程）
  publish        Environment release：package.ps1 -SignOnly -Release 簽章並核對 key ID、make-latest-json.ps1、
                 建立 draft release 並上傳兩個安裝檔、兩個 .sig、latest.json
發佈（人工，gh release edit <tag> --draft=false --latest）→ release-verify.yml（release: released）：從 releases/latest/download/latest.json 取兩個安裝檔，
                 以內建公鑰用 minisign 驗簽、比對版本，失敗開 issue
```

- **機密與權限**：簽章私鑰與密碼放在 Environment `release` 的 secrets（`TAURI_SIGNING_PRIVATE_KEY` 是私鑰檔**內容**、
  `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` 是密碼），只有 publish job 讀得到；build 與 smoke 不接觸任何 secret。
  workflow 預設只讀，publish 才有 `contents: write`；第三方 action 一律釘 commit SHA。
  金鑰的產生位置、私鑰遺失與外洩的處置、換鑰過渡版見設計 D5：**v0.1.0 發出後公鑰寫死在使用者的程式裡，私鑰遺失就再也
  推不出更新**，私鑰要離線備份。
- **首發順序（v0.1.0）**（一次性程序：v0.1.0 發出並合併進 `main` 後刪除本段，只保留 `workflow_dispatch` 須在預設分支
  那一句）：以 `AGENTS.md`「發布」一節的「首發順序」為準，摘要如下。首發時 `main` 仍是 Lively 線、沒有這兩個 workflow
  檔，GitHub Pages 又取自 `main` 的 `/docs`：
  1. task 1.1：正式公鑰寫進 `tauri.conf.json` 的 `plugins.updater.pubkey`，提交到要打 tag 的 commit（漏掉時 build job 在
     打包前就以占位公鑰失敗）。
  2. 完成下方「使用者待辦」的 Environment `release` 與 `v*` tag ruleset。
  3. 著陸頁的 ARM64 偵測與安裝說明改寫——由收尾線在合併進 `main` 前完成。
  4. 推 `v0.1.0` tag，產生 draft release。
  5. 依發佈清單檢查後以 `--latest` 發佈，確認 `release-verify` 通過。這一步之後線上舊著陸頁的下載連結就失效（`main`
     的著陸頁指向 `finance-calendar.zip`，v0.1.0 沒有這個資產），第 6 步要緊接著做；`release-verify` 失敗時要取捨：先合併
     讓連結指向 v0.1.0，或把 Latest 暫時改回 v6.2。舊 Lively release 改成 prerelease 只能在這一步之後做（之前改，舊著陸頁的 zip 下載會 404）。
  6. 合併進 `main`（Pages 重建後新著陸頁才生效；先合併的話 v0.1.0 成為 Latest 之前下載按鈕會 404）。

  `workflow_dispatch` 只在 workflow 檔位於預設分支時可觸發（「This event will only trigger a workflow run if the workflow
  file exists on the default branch.」，
  <https://docs.github.com/en/actions/writing-workflows/choosing-when-your-workflow-runs/events-that-trigger-workflows#workflow_dispatch>）：
  第 6 步之前，下方的 ARM64 冒煙豁免與 `release-verify.yml` 的手動執行都用不了。首發若必須用豁免，先只把
  `.github/workflows/` 合進 `main`，再以 tag 為 ref 手動觸發。
- **使用者待辦（發版前）**：
  - tag ruleset 對 `v*` 要限制只有維護者能建立，並**同時禁止更新與刪除**（只限制建立不夠）：Environment 審核可能隔數小時，tag 在這段期間
    被改指，release 會掛在新 commit、資產卻來自舊 commit；publish 建立 release 前會比對遠端 tag 指向的 commit 與觸發時的
    `GITHUB_SHA`，不同就失敗，這是第二道防線。
  - Environment `release` 的 Deployment branches and tags 只允許 `v*` tag，分支上改版的 workflow 就連請求審核的機會都沒有。
  - 簽章私鑰與密碼**只放 Environment `release`**，不可放 repo 層級 secrets（放錯時 workflow 照樣能跑、繞過審核，workflow 本身無法偵測）。
  - repo 的 artifact 保留設定要 **≥35 天**（Settings → Actions → General 裡的 artifact／log retention 設定）：GitHub 文件
    明載「`retention-days` 值不得超過 repository、organization 或 enterprise 設定的保留上限」
    （<https://docs.github.com/en/actions/tutorials/store-and-share-data>；`actions/upload-artifact` 的 README 也寫
    最長 90 天「除非在 repository 設定頁更改」）。公開 repo 的預設是 90 天、可設 1–90 天
    （<https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/enabling-features-for-your-repository/managing-github-actions-settings-for-a-repository>），
    所以沒動過設定就沒問題；若曾調到低於 35 天，workflow 的 `retention-days: 35` 會被壓低，Environment 審核等得比保留期久時
    publish 的 download-artifact 會失敗（fail-safe）。
  - 重跑 publish 前，先刪除上一次中途失敗留下的同 tag draft release（publish 發現同 tag 已有 release 會失敗，以免出現多個 draft）。
- **發佈指令（收尾線，一律明寫 `--latest`）**：人工檢查 draft 後以

  ```powershell
  gh release edit <tag> --draft=false --latest
  ```

  發佈。**發佈前依 `AGENTS.md`「發布」一節的發佈清單逐項確認**（Environment 審核、**x64 實機安裝並啟動**、ARM64 冒煙或豁免
  標記、`latest.json` 含兩個平台、`--latest`）。**不得發佈不含 `latest.json` 的 release**，Lively 線不再發 release
  （Latest 被不含 `latest.json` 的 release 搶走，所有使用者的更新器就拿不到更新）。必須帶 `--latest`：沒指定時 GitHub 會依 semver 自動指派 Latest，而 repo 還有 `v6.1`、`v5.6` 這些 Lively 線的 release，新版可能
  不會成為 Latest，更新器（`releases/latest/download/latest.json`）就拿不到它；用網頁發佈時也要手動勾 Set as the latest release。
  發佈後 `release-verify.yml` 會驗證。舊 Lively release 改成 prerelease 的時機在首發順序第 5 步之後（使用者 2026-10-05 決定，見 `AGENTS.md`「首發順序」），workflow 不處理。
- **artifact 保留期與 Environment 審核期限**：publish 在 Environment 審核期間等待，build 上傳的 `installer-*` artifact 必須撐過等待。
  單一 workflow run 含等待核准的上限是 35 天（GitHub Actions limits 文件），所以 `installer-*` 保留 35 天（`upload-artifact` 預設 90 天、
  可設 1–90）；診斷用的 `smoke-logs-*` 保留 14 天。若審核等超過保留期，publish 的 download-artifact 會失敗（fail-safe，不會用到殘缺的檔案），
  以同一個 tag 重新跑 workflow 即可。
- **發佈後驗證的通知**：`release-verify.yml` 在驗證失敗、被取消或逾時都會開 issue（步驟層級 12 分鐘逾時，小於 job 的 20 分鐘）；同標題的 open issue 已存在時改成留言，不重複開。
- **ARM64 冒煙豁免**：`windows-11-arm` 不可用時，以 `workflow_dispatch`（選同一個 tag）勾選 `waive-arm64-smoke`；
  smoke-arm64 整個跳過，release notes 與 `latest.json` 的 `notes` 標「ARM64 未冒煙」，由收尾線人工決定是否發佈。
  x64 冒煙不可豁免。託管 runner 沒有可設的排隊逾時，排不到機器時請取消這次執行再用豁免重跑。
- **runner 是否有互動式桌面**待第一次實跑確認；沒有時冒煙測試以「行程存活＋記錄檔啟動行」為準，視窗未驗
  （腳本把宿主可見視窗數與工作階段資訊寫進 step summary）。
- 腳本與自測（不啟動宿主；只有 verify-published 的自測在 minisign 不在快取時下載一次釘版本的執行檔）：
  `host/tools/make-latest-json.ps1`（組 `latest.json`；`tests/make-latest-json.Tests.ps1`）、
  `host/tools/verify-published.ps1`（發佈後驗證；`tests/verify-published.Tests.ps1`，minisign 釘版本與 SHA256，
  第一次會下載到 `%LOCALAPPDATA%\fc-host-tools`）、`host/tools/ci-smoke.ps1`（只能在 GitHub Actions 上執行；
  `tests/ci-smoke.Tests.ps1`）、`tests/release-workflow.Tests.ps1`（workflow 的專案規則靜態檢查）。
  workflow 語法以 `actionlint` 的預編譯執行檔檢查（不安裝到系統）。

## 工具與測試

```powershell
cd host
cargo fmt --check
cargo clippy --all-targets
cargo clippy --all-targets --features self-test-ipc
cargo test
```

改過的 `.md` 在 repo 根目錄跑 `npx markdownlint-cli2 <檔>`。

桌面行為（置底、Win+D、explorer 重啟復原等）驗收腳本在 `host/tools/`（`watch-zorder.ps1` 監控
z-order、`verify-*.ps1` 針對個別 task、`lib/SafeInput.psm1` 是所有輸入注入的唯一入口），用法與
欄位定義見 `host/tools/README.md`。前端行為對照測試（Lively 版 `finance-calendar.html` vs 新版
`host/ui/widget.html?w=<id>`）在 `host/tests/compare/`：

```sh
node host/tests/compare/compare.mjs            # 五個面板自比對
node host/tests/compare/compare.mjs --widget clock   # 跨版本比對
```

用法與其他情境（跨日、休市順延、暫停/恢復）見 `host/tests/compare/README.md`。其餘前端單元測試
在 `host/tests/*.test.mjs`，各檔為獨立腳本（非 `node:test` runner），逐一以
`node host/tests/<檔名>.test.mjs` 執行；PASS／FAIL 與失敗原因印在 stdout/stderr，結束碼
0＝通過。
