# host/tools/ — 桌面行為驗收用監控腳本

## watch-zorder.ps1

z-order／視窗狀態監控腳本。後續桌面行為驗收（Win+D、explorer 重啟、保險重排、
30 秒內恢復等）都拿這支腳本的記錄檔當證據。純 PowerShell 7（pwsh）＋
`Add-Type` P/Invoke，零安裝、不綁 x64（`GetWindowLongPtr`／`GetWindowLong`
依 `IntPtr.Size` 自動切換 64/32 位元路徑）。

### 用法

```powershell
# 監控單一或多個 PID（每次取樣重新掃描該 PID 目前所有可見頂層視窗，
# 故行程啟動後才建立的視窗——例如宿主稍後才開出的小工具視窗——會自動加入）
pwsh -File host/tools/watch-zorder.ps1 -ProcessId 12345 -OutFile host/tools/run.log -DurationSec 60

# 監控明確 HWND（可多個，十進位或 0x 十六進位字串），用於同時盯多個小工具視窗
pwsh -File host/tools/watch-zorder.ps1 -TargetHwnd 0x000A21FE,0x000B3140 -OutFile host/tools/run.log -DurationSec 120

# 監控行程名稱（task 3.1 新增；每次取樣以 Get-Process 重新解析 PID）：可在目標行程
# **啟動之前**就開始監控，等 header 行寫出後再啟動目標，記錄即涵蓋它第一個視窗可見的瞬間
pwsh -File host/tools/watch-zorder.ps1 -ProcessName fc-host -OutFile host/tools/run.log -DurationSec 60 -IntervalMs 100

# 可併用；-IntervalMs 預設 500（對應「每 0.5 秒」取樣）、-BelowCount 預設 5
# （「下方可見視窗類別」最多列出幾個）、-Quiet 不同步印到主控台
```

必填參數：`-ProcessId`、`-ProcessName`、`-TargetHwnd` 至少給一個；`-OutFile` 必填。
時間到（`-DurationSec`）自動結束，適合驗收腳本無人值守地跑。輸出檔每次執行
會覆寫（不是續寫）。

### 輸出格式

每行：`<時間戳，含毫秒與 UTC 偏移> <狀態欄位...>`，只在狀態有變化時才寫一行
（第一次取樣一定算變化，會當成 baseline 寫出）。以 `#` 開頭的行是 session
起訖說明行（開始時間／監控目標／結束時間與寫入行數），不是狀態行，grep 狀態
時記得排除（`grep -v '^#'` 或 `Select-String -NotMatch '^#'`）。

範例（節錄自 `watch-zorder.sample.log`）：

```text
2026-09-27T23:19:56.619+08:00 explorerPid=53908 fgClass=Windows.UI.Core.CoreWindow fgPid=59720 win[0x20B42].class=Shell_TrayWnd win[0x20B42].visible=1 win[0x20B42].minimized=0 win[0x20B42].cloaked=0 win[0x20B42].above=1 win[0x20B42].desktopAbove=0 win[0x20B42].below=Chrome_WidgetWin_1 win[0x20B42].hasProgman=1 win[0x20B42].hasWorkerW=0
```

欄位定義：

| 欄位 | 說明 |
|---|---|
| `explorerPid` | 目前擁有桌面殼層（`GetShellWindow`，即 Progman）的行程 PID。explorer 重啟後這個值會變，可用來偵測重啟事件。 |
| `fgClass` / `fgPid` | 目前前景視窗（`GetForegroundWindow`）的類別名與所屬 PID。 |
| `win[<hwnd>].status=not_found` | 該 HWND 在本次取樣的 z-order 清單中找不到（可能已關閉／隱藏）；此時沒有其餘 `win[...]` 欄位。 |
| `win[<hwnd>].class` | 監控目標視窗自己的類別名。 |
| `win[<hwnd>].visible` | `IsWindowVisible`。**注意**：視窗被最小化或被 DWM cloak 時這個值通常仍是 1（`WS_VISIBLE` 不會因這兩者被清除），單看 `visible` 判斷不出使用者真的看不看得到畫面。 |
| `win[<hwnd>].minimized` | `IsIconic`（是否最小化）。 |
| `win[<hwnd>].cloaked` | `DwmGetWindowAttribute(DWMWA_CLOAKED)`（是否被 DWM cloak）。常見於虛擬桌面切走的視窗、UWP 暫止的視窗。**可見性驗收（例如「Win+D 時小工具可見」）要同時看 `visible`、`minimized`、`cloaked` 三個欄位，缺一都可能誤判**：`visible=1` 不代表真的畫得出來，還要 `minimized=0` 且 `cloaked=0`。這三個欄位都納入「只在狀態變化時寫一行」的比較，只有 `cloaked` 單獨改變也會產生新的一行。 |
| `win[<hwnd>].above` | z-order 中位於該視窗**之上**、可見（`visible`）、非最小化、非 cloaked 的視窗數——**不套用**「一般視窗」判準，不排除 tool window（見下）。 |
| `win[<hwnd>].desktopAbove` | Win+D 驗收的核心欄位：目標**之上**是否存在可見、非 cloaked 的「桌面 host」視窗——Progman 本身，或帶 `SHELLDLL_DefView` 子視窗的 WorkerW（explorer 用來承載桌面圖示的那個 WorkerW，區分於其他跟桌面圖示無關的 WorkerW）。`1` 代表桌面視窗真的被推到目標之上（例如 Win+D 生效）。 |
| `win[<hwnd>].below` | z-order 中位於該視窗**之下**、符合「一般視窗」判準（見下）的視窗類別名，由近到遠列出前 `-BelowCount` 個（預設 5），超過的部分以 `(+N more)` 註記數量不全列。 |
| `win[<hwnd>].dupInSnapshot` | 只在發生時輸出（task 3.1 新增）：同一 HWND 在這一次 z-order 列舉中出現的次數（≥2）。`GetWindow(GW_HWNDNEXT)` 走訪不是原子快照，列舉途中目標的 z-order 被改（例如小工具以一個 `SetWindowPos(HWND_BOTTOM, SWP_SHOWWINDOW)` 從建立時的上層移到底部）時會出現兩次；此時同一行其餘欄位取自**第一個**出現處（較上層的舊位置，常見 `visible=0`），不代表列舉結束時的狀態。 |
| `win[<hwnd>].hasProgman` / `hasWorkerW` | 完整下方 z-order 清單（**不經**「一般視窗」判準）中，是否存在可見、非 cloaked、類別為 `Progman` / `WorkerW` 的視窗。驗收 Win+D、explorer 重啟等情境時建議直接 grep 這兩個旗標；`below` 文字清單本身仍用「一般視窗」判準，不會列出 `Progman`（見下）。 |

### 「上方可見視窗數」「下方可見視窗類別」「桌面視窗」用不同判準

三者**刻意**不共用同一套規則：

- **above**（數量）＝可見（`IsWindowVisible`）、非最小化（`!IsIconic`）、非 cloaked
  （`DwmGetWindowAttribute(DWMWA_CLOAKED)==0`）。**不**額外排除 tool window 或零尺寸
  視窗——一個可見的 tool window（例如浮動面板）壓在目標上方時，也要算進 above。
- **below**（文字清單）＝符合「一般視窗」判準（見下）。這裡刻意排除 tool window／
  零尺寸，因為「下方視窗類別」是用來判斷「目標是否真的被一般應用程式視窗蓋住」，
  工具面板／隱藏代理視窗不是這裡要盯的東西。
- **桌面視窗判斷**（`desktopAbove`、`hasProgman`、`hasWorkerW`）＝**不套用**
  「一般視窗」判準——`Progman` 本身帶 `WS_EX_TOOLWINDOW`，若套用一般視窗判準會被
  tool window 條件排除、永遠偵測不到。這三個欄位只看可見、非 cloaked：
  - `hasProgman` / `hasWorkerW`：完整下方 z-order 清單中，是否存在可見、非 cloaked、
    類別為 `Progman` / `WorkerW` 的視窗（只比對類別名，不要求 WorkerW 帶
    `SHELLDLL_DefView`）。
  - `desktopAbove`：完整上方 z-order 清單中，是否存在可見、非 cloaked 的「桌面 host」
    視窗——`Progman`，或帶 `SHELLDLL_DefView` 子視窗的 `WorkerW`。這是 Win+D 判準
    （design.md D8：「桌面視窗實際位於 z-order 頂端」）的直接對應欄位，不需要另外
    拼湊 `above` 數字或 `below` 清單去猜。

### 「一般視窗」判準（只用於 below 文字清單）

一個頂層視窗要同時滿足以下五條，才會被算進「下方可見視窗類別」：

1. `IsWindowVisible` = true
2. `IsIconic` = false（未最小化——上一條的 `WS_VISIBLE` 不會因最小化被清除，所以要
   另外排除）
3. 未被 DWM cloak（`DwmGetWindowAttribute(DWMWA_CLOAKED) == 0`；cloaked 視窗
   `IsWindowVisible` 可能仍回傳 true，但實際不會畫出來，常見於虛擬桌面切走或
   UWP 暫止的視窗）
4. 不是 tool window（`GWL_EXSTYLE` 未含 `WS_EX_TOOLWINDOW`）
5. 視窗矩形非零尺寸（`GetWindowRect` 寬高皆 > 0）

`Progman` 帶 `WS_EX_TOOLWINDOW`，不會出現在 `below` 文字清單裡；要判斷「下方是否有
桌面視窗」請看 `hasProgman` / `hasWorkerW`，不要看 `below` 清單本身有沒有列出
`Progman` / `WorkerW`（見上一節）。

### 監控目標如何決定

- `-TargetHwnd` 給的 HWND：每次取樣直接查該 HWND 在 z-order 清單中的位置；找不到
  就標 `status=not_found`。
- `-ProcessId` 給的 PID：每次取樣重新掃描該 PID 目前所有 `IsWindowVisible=true` 的
  頂層視窗，全部納入監控——這代表行程稍後才建立的新視窗（例如宿主啟動後才逐一開出
  的多個小工具視窗）會在出現的那次取樣自動被加入，不用事先知道 HWND。
- `-ProcessName` 給的名稱：每次取樣以 `Get-Process -Name` 解析成 PID，之後同 `-ProcessId`。
- 所有目標會合併去重（同一個 HWND 只輸出一次）。

### 範例輸出

`watch-zorder.sample.log` 為實跑範例，監控本機工作列（`Shell_TrayWnd`，explorer 的
一個可見頂層視窗）：直接示範 `hasProgman=1`（真正的 Progman 在它下方，正確偵測到，
不受 Progman 帶 `WS_EX_TOOLWINDOW` 影響）與 `desktopAbove` 欄位的輸出格式；工作列
此刻沒有被推到 Progman 之上，`desktopAbove=0`。

重現指令（用任一 explorer 擁有的可見視窗即可，工作列最穩定好找）：

```powershell
# Shell_TrayWnd 是工作列的視窗類別，用 -ProcessId <explorer PID> 監控可看到 explorer
# 底下所有可見頂層視窗（含 Progman、WorkerW、工作列等），也可以先找出工作列自己的
# HWND 再用 -TargetHwnd 單獨盯它：
pwsh -File host/tools/watch-zorder.ps1 -ProcessId (Get-Process explorer).Id -OutFile host/tools/watch-zorder.sample.log -DurationSec 10 -IntervalMs 500
```

含 above／below 動態變化（一次視窗蓋到目標上層、一次目標自己最小化再還原）的示範，
可監控任一般應用程式視窗（例如記事本）重現：開一個視窗當監控目標、過程中開另一個
視窗蓋上去、再把兩者分別最小化／還原。

## lib/SafeInput.psm1（所有輸入注入的唯一入口）

工作階段鎖定時注入的按鍵會打進登入畫面的密碼框、累積錯誤登入並凍結帳戶。因此
**host/tools/ 下只有這個模組可以宣告／呼叫底層鍵盤、滑鼠注入 API 與游標移動**，
會注入輸入的腳本（probe-1.1、probe-1.3、verify-3.4、verify-4.7-wheel、verify-5.3、
verify-7.5-edit-move、probe-7.6-resize、verify-7.6-edit-resize、verify-7.7-aero-snap）一律
`Import-Module (Join-Path $PSScriptRoot 'lib\SafeInput.psm1') -Force` 後改用：

- `Invoke-SafeInputPreflight`（**開始時必呼叫**）：先查鎖定，再查前景，最後送一次無害的 F15
  並查 `GetLastInputInfo` 有沒有前進。回傳 `ExitCode`：0＝可繼續、2＝BLOCKED（鎖定）、
  3＝ENV-BLOCKED（前景是系統 UI 或無法判斷，或未鎖定但合成輸入沒被系統計入，例如提權視窗
  卡在前景）；腳本照此結束，不產生無資訊的 PASS/FAIL。
- **前景是系統 UI 時不送任何鍵**（fix F8）：送 F15 之前先查前景根視窗的類別、行程、cloaked、
  topmost，以 `lib/SystemUi.psm1` 的 `Get-ForegroundBlockReason` 判斷（系統類別與行程清單與
  `lib/Occluders.psm1` 共用）。`Shell_SystemDialog`、`Shell_SystemDim`、`#32770`、
  `Windows.UI.Core.CoreWindow` 等類別，`PickerHost`、`consent`、`LockApp` 等行程，以及 cloaked
  或 topmost 的前景一律 ENV-BLOCKED（結束碼 3），訊息記前景類別與行程名。任何欄位查不到也一樣
  （fail closed）。桌面本身（`Progman`／`WorkerW`，例如 Win+D 之後）不算系統 UI，照常探查，但
  有兩個前提（fix F8b）：該桌面視窗屬殼層 explorer（`GetShellWindow` 的行程），且畫面上沒有可見、
  未 cloaked、覆蓋整個螢幕的系統 UI（例如「Windows 安全性」的 `Shell_SystemDim`）；任一不符或查不到
  也回 3、不送鍵。注意：preflight 回 0 只代表「前景與覆蓋層不是系統 UI」，不代表畫面上任何角落都
  沒有系統 UI；點擊座標本身的遮擋仍要經 `lib/Occluders.psm1` 檢查。
- `Test-SessionLocked`／`Assert-SessionUnlocked <描述>`：鎖定判準只看 LogonUI.exe
  （LockApp 解鎖後可能殘留，不用）；檢查本身失敗視為鎖定。
- 原生後端在匯入模組時就編譯；每次注入固定「初始化（no-op）→ 查鎖定 → 立即注入」，檢查
  與注入之間沒有任何耗時步驟。
- `Send-GuardedWinD`、`Send-GuardedAltTap`、`Send-GuardedClick x y`、
  `Send-GuardedWheel x y 格數`、`Set-GuardedCursorPos x y`，以及單一呼叫
  `Invoke-GuardedKey`／`Invoke-GuardedMouse`。
- **每一次**底層呼叫前都重新檢查鎖定（等待之後也是），偵測到就丟 `BLOCKED:` 開頭的
  終止例外，並鎖存為「已停止」：之後即使解鎖，同一行程內的所有注入也一律拒絕。
  執行途中鎖定時的結束碼依腳本而異（以程式為準）：verify-7.5-edit-move、verify-dragdpi-esc 的 catch
  認得 `BLOCKED:`，記 BLOCKED、結束碼 2（優先於 1 與 3）；verify-5.3、verify-7.6-edit-resize、
  verify-7.7-aero-snap 沒有這個分支，記為例外中止（結束碼 1），probe-7.6-resize 記「探針跑完（無例外）」
  NO。注入一樣已停止，只是結束碼不區分「途中鎖定」與功能失敗，判讀時要看記錄檔的例外訊息。
- 唯一例外：進入停止狀態的當下，先把本行程已按下、尚未放開的鍵／滑鼠鍵依相反順序送出
  放開（不重查、只送 key-up／mouse-up），避免 Win 或左鍵卡住；不允許任何新的按下、滾輪
  或游標移動。
- **DPI 感知（fix F9）**：載入本模組的腳本都必須在頂層、取得任何座標之前（含
  `Invoke-SafeInputPreflight`），呼叫 `SetThreadDpiAwarenessContext([IntPtr](-4))`，也就是
  Per-Monitor-V2。`GetWindowRect`、`WindowFromPoint`、`Screen.AllScreens`、游標與點擊座標都依
  呼叫端執行緒的 DPI 感知解讀；pwsh 預設是 unaware。在混合 DPI 下，例如探針放在 175% 的筆電螢幕，
  實體像素座標會被當成虛擬化座標，取樣點全部落空。`tests/DpiAwareness.Tests.ps1` 做靜態檢查。

mock 測試（不呼叫任何真實注入 API，鎖定時也可跑）：

```powershell
pwsh -NoProfile -File host/tools/tests/SafeInput.Tests.ps1   # 結束碼 0＝全部通過
```

## lib/VerifyVerdict.psm1（驗收判讀純函式）

驗收腳本的判讀（z-order 記錄分階段、逐項結果 → 結束碼等）放在這裡，以合成記錄做 mock
測試，不啟動宿主、不注入輸入：

```powershell
pwsh -NoProfile -File host/tools/tests/VerifyVerdict.Tests.ps1   # 結束碼 0＝全部通過
```

## verify-3.1.ps1、host-cdp-eval.mjs（task 3.1 視窗工廠驗收）

```powershell
cd host; cargo build --release
pwsh -File host/tools/verify-3.1.ps1   # 證據寫到 host/tools/evidence/3.1-*.log
```

- 以暫存目錄當 `%APPDATA%`（首次啟動、不碰使用者設定），先開 `watch-zorder.ps1
  -ProcessName fc-host` 再啟動宿主；列舉小工具視窗矩形與延伸樣式，與預設格座標換算的格線
  矩形逐像素比對（task 7.7 起；原本比對 Lively 版錨點排列）；再以 CDP 在小工具頁面呼叫 `update_settings` 驗證即時建立／關閉。
- CDP：宿主以環境變數 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=<port>`
  啟動（只在驗收腳本裡設定），`host-cdp-eval.mjs <port> <url 子字串> <JS>` 對符合的頁面
  執行運算式並印出 JSON 結果（CDP 客戶端沿用 `host/tests/compare/cdp.mjs`）。`/json/list`
  的 url 是可見 URL，導覽一開始就換成目標網址、文件卻還是 about:blank；所以腳本會先確認
  `location.href` 已提交才執行（逾時 10 秒），運算式只跑一次。頁面 `main()` 的非同步回填
  （例如設定頁等 `get_settings()`）不在這個保證內，呼叫端要自己等條件成立。
- 不注入輸入、不截圖（小工具在最底層，截到的是蓋在上面的一般視窗，可能含使用者內容）。
- 判讀：只看類別 `Tauri Window` 且 `visible=1` 的項目；宿主的 `Tao Thread Event Target`
  是 tao 內部的透明訊息視窗（帶 `WS_VISIBLE` 但 layered、永不繪製），不列入；
  `visible=0` 的項目必須伴隨 `dupInSnapshot`（見上方欄位說明），否則判為異常。

## verify-3.2.ps1（task 3.2 小工具顯示／隱藏驗收，task 7.7 改為格線版面）

```powershell
cd host; cargo build --release
pwsh -File host/tools/verify-3.2.ps1   # 證據寫到 host/tools/evidence/3.2-*.log
```

- 暫存目錄同時當 `%APPDATA%`（首次啟動）與 `%LOCALAPPDATA%`（資料目錄預設值的來源，見
  `settings::default_data_dir`）——完全不碰使用者真正的設定檔或 `D:\finance-calendar`。
  在暫存資料目錄寫入 fixture：`events` 40 筆（遠超過 dynamic 格子放得下的量）、`quotes` 空
  陣列（quotes 是唯一沒資料就整個隱藏的財經小工具）。
- 驗證重點（task 7.7 起）：①quotes 無資料→視窗隱藏、改寫 fixture 加入一筆行情後（等下一輪
  ≤30 秒排程輪詢）重新出現；②clock／macro／fixed／dynamic 的視窗矩形逐像素＝預設格子，不隨
  內容伸縮（原本「視窗高度＝內容、夾限最大高度」的判準已隨 task 7.3 刪除）；③dynamic 撐爆時
  `#dynList` 的 `scrollHeight > clientHeight`（在視窗內捲動）；④quotes 重新顯示後矩形＝格子、
  仍是 `TOOLWINDOW`／無 `APPWINDOW`／`NOACTIVATE`；⑤全程以 `watch-zorder.ps1` 監控，任何時刻
  都不得有一般視窗位於小工具之下。
- CDP expression **不要**自己再呼叫 `JSON.stringify`——`host-cdp-eval.mjs` 的 `evaluate()`
  已用 CDP `returnByValue` 取回反序列化後的值，外層 `console.log` 只做一次 JSON 編碼；
  expression 裡再包一層會變成「JSON 字串的 JSON 字串」，`ConvertFrom-Json` 解出來是純字串
  而非物件（腳本開發過程中踩過一次，已修正並在腳本內加註解）。
- 不注入輸入、不截圖，理由同 verify-3.1.ps1。
- 實機驗收發現的一個真實前端 bug（已修正，見 `host/ui/widget.html`）：ResizeObserver 原本
  用 `entry.contentRect`（恆為 content-box，排除 padding／border），但小工具視窗要容納
  整張 `.panel` 卡片（含 1px border；`clock.js` 的 `.clockcard` 另加 padding），用
  content-box 高度設視窗尺寸會把卡片底部裁掉。改讀 `entry.borderBoxSize`（防禦性 fallback
  到 `getBoundingClientRect()`），詳見 widget.html 該處註解與 task-3.2-report.md。

## verify-7.3-placeholder.ps1（task 7.3／3.2 佔位外框、Reload、編輯中新建與故障重建）

```powershell
cd host; cargo clean --release -p fc-host; cargo build --release   # 不帶 self-test-ipc
pwsh -NoProfile -File host/tools/verify-7.3-placeholder.ps1                     # Reload＝CDP Page.crash（只打 quotes 那頁）
pwsh -NoProfile -File host/tools/verify-7.3-placeholder.ps1 -ReloadMode Page    # Reload＝CDP location.reload()
pwsh -NoProfile -File host/tools/tests/Verify73Placeholder.Tests.ps1            # 純函式與結構（不啟動宿主）
```

- 前提：沒有其他 fc-host 在跑（否則結束碼 4，不結束它）；工作階段未鎖定且合成輸入有效
  （`Invoke-SafeInputPreflight`：鎖定 2、無效 3；執行途中鎖定 2）；主螢幕可見 clock 與下緣行情條那幾列。
- 隔離同 verify-7.5：暫存 %APPDATA%／%LOCALAPPDATA%、fixture 寫在暫存資料目錄（先 `quotes: []`）、
  真正設定檔只比雜湊、開機自啟登錄快照還原、收尾以 `Stop-ProcessTree` 只停自己啟動的宿主。
- 涵蓋（與腳本檔頭一致）：
  - 3.2：無內容隱藏、編輯版面期間佔位外框、離開後再度隱藏、有資料後在原格子出現（改寫 fixture、
    等輪詢 ≤45 秒，矩形＝記錄的格子）。
  - F1 項目 3：佔位外框可拖並寫回；鎖定時拖不動（非編輯版面時 quotes 隱藏，改以 clock 真實拖曳，
    矩形與所有記錄不變）。
  - F1 項目 4：Reload 後外框仍在、同一 HWND 仍帶 WS_THICKFRAME、再拖一次。
  - F1 項目 5：編輯版面中開啟關閉中的 fixed → 新 HWND 帶 WS_THICKFRAME、八個把手；故障重建
    （CDP `Browser.crash` 讓小工具那組的 browser 當機 → 全部重建）後每扇非 quotes 小工具都是新 HWND
    且帶 WS_THICKFRAME。
- 未涵蓋：
  - F1 項目 5「建立過程中離開編輯版面不殘留 WS_THICKFRAME」：競態窗口在宿主內部、毫秒級、外部觀察
    不到起點，腳本每次經 CDP 下指令要啟動一次 node（約百毫秒），無法可靠落在窗口內。替代：Rust 單元
    測試 `resizable_now_reflects_state_at_call_time`；實機佐證建議在
    `desktop::set_widget_resizable_on_main_thread` 執行時記一行，再對記錄檔斷言。
  - renderer 當機是同一 HWND 的 `Reload()`（recovery.rs），不經視窗工廠，所以不以它驗「重建」；
    RENDER_PROCESS_UNRESPONSIVE 與看門狗觸發的單窗重建需要頁面卡死 15 秒以上，外部無法可靠製造。
  - 「Reload 後立刻畫出外框」沒有時間門檻：最多等 15 秒，只記錄實際毫秒數。
- 當機一律經 `Invoke-CdpCrash`：先確認 CDP 埠的 Listen 行程都在本宿主行程樹內，再呼叫
  `cdp-crash.mjs`（`page <id>` 只對 `widget.html?w=<id>` 恰好一頁送 `Page.crash`，`browser` 送
  `Browser.crash`），不以 PID 猜 renderer。
- 選點一律由 CDP 讀元素矩形 × `devicePixelRatio` ＋ `ClientToScreen`（`lib/ScrollAreaTarget.psm1`），
  送輸入前斷言元素可見、與 viewport 有交集、`elementFromPoint` 命中，不成立就 `PRECONDITION FAIL`
  （結束碼 1）、不送輸入。遮擋經 `lib/Occluders.psm1`，前景基準用 `lib/ScratchWindow.psm1`。輸出
  `evidence/7.3-placeholder.log`、`7.3-placeholder-summary.log`。

## verify-7.5-edit-move.ps1（task 7.5 編輯版面移動：合法判斷、紅框預告、彈回）

```powershell
cd host; cargo clean --release -p fc-host; cargo build --release   # 不帶 self-test-ipc
pwsh -File host/tools/verify-7.5-edit-move.ps1   # 證據寫到 host/tools/evidence/7.5-*
```

- 經 `SafeInput.psm1` 真實拖曳 clock：鎖定時拖不動、拖到空白處對齊格線且格數不變、拖到
  與總經日曆重疊處時頁面出現 `body.edit-invalid`（CDP 讀 DOM＋頁面 MutationObserver 記錄＋
  `PrintWindow` 只截 clock 視窗本身）、放開彈回、離開編輯版面後再拖不動、強制結束重啟後
  位置保留。前景基準是腳本另開的 WinForms 小視窗，拖曳每一步取樣 `GetForegroundWindow`
  與該執行緒 `GetGUIThreadInfo().hwndFocus`。
- 按下點被一般應用程式主視窗（例如最大化的編輯器）蓋住時，經 `lib/Occluders.psm1` 暫時最小化，
  結束時以 `Restore-Occluders` 還原（含吸附與最大化，記錄檔有列出）。判讀一律經
  `Assert-OccluderResult`（見下方 lib/Occluders.psm1 一節）：工作列與系統 UI＝ENV-BLOCKED（結束碼 3）；
  命中桌面或沒命中任何視窗＝小工具不在預期位置或沉到桌面之下（FAIL）；被腳本自己的前景基準表單
  蓋住＝腳本配置錯誤（FAIL）。
- 只有一台顯示器時跨顯示器項目記為 PENDING（待補，見 human-checklist）。

## probe-7.6-resize.ps1（task 7.6 探針：`startResizeDragging` 能否進入尺寸迴圈）

```powershell
pwsh -File host/tools/probe-7.6-resize.ps1 -Suffix '-sizebox'   # 證據 evidence/7.6-probe-*<Suffix>.log
```

- 以 CDP 在 clock 頁面掛 `mousedown` 捕獲監聽器，改呼叫 `startDragging()`（對照組 Move）或
  `startResizeDragging('East'／'SouthWest')`，再經 `SafeInput.psm1` 按住拖曳；每一步以宿主 UI
  執行緒的 `GetGUIThreadInfo`（`GUI_INMOVESIZE`、`hwndMoveSize`）、視窗矩形、前景與焦點判定。
- `-no-sizebox` 證據是 task 7.6 改動前（HEAD 0d6cc78）的建置，`-sizebox` 是編輯版面切
  `WS_SIZEBOX` 之後；結論寫在 design.md D7「調整大小的機制」。

## verify-7.6-edit-resize.ps1（task 7.6 編輯版面調整大小）

```powershell
cd host; cargo clean --release -p fc-host; cargo build --release   # 不帶 self-test-ipc
pwsh -File host/tools/verify-7.6-edit-resize.ps1   # 證據寫到 host/tools/evidence/7.6-*
```

- 經 `SafeInput.psm1` 按住頁面上的調整大小把手拖曳 clock：鎖定時沒有把手、沒有
  `WS_THICKFRAME`、邊緣拉不動，連強制呼叫 `startResizeDragging` 也進不了尺寸迴圈；
  `WS_SIZEBOX` 不改變外觀（腳本暫時加上再拿掉，比對用戶區、DWM 外框範圍與外圈像素，只比對
  不存檔）；關掉 fixed 後右緣加寬兩格 → 對齊格線、`devicePixelRatio` 變大；縮到小於最小格數
  → 紅框（`PrintWindow` 只截 clock）、放開彈回；拖左緣只對齊左緣；離開編輯版面後拉不動；
  強制結束重啟後保留。每一步取樣前景／焦點（同 verify-7.5）。
- 遮擋處理與收尾同 verify-7.5（經 `lib/Occluders.psm1`，結束時以 `Restore-Occluders` 還原）。

## lib/EvidenceLog.psm1（證據去識別）

repo 公開，證據記錄檔不得留下使用者設定檔路徑。驗收腳本一律以 `New-EvidenceWriter <路徑>`
取代 `New-Object System.IO.StreamWriter(<路徑>, $false)`：寫出前把 `%TEMP%`、`%LOCALAPPDATA%`、
`%APPDATA%`、`%USERPROFILE%` 四個目錄改寫成對應字樣（`\`、JSON 跳脫的 `\\`、`/` 都認得）。
`ConvertTo-EvidenceText` 給 `Set-Content`／`Add-Content` 用；`Protect-EvidenceFile` 就地改寫既有
文字檔（保留 BOM 與行尾，task 7.7 用它清過一次 `evidence/` 的歷史記錄）。`host/tests/compare/`
的 Node 腳本改印 `server.tmpRootDisplay`（系統暫存目錄那一段換成 `%TEMP%`）。
子行程寫的記錄檔（宿主統一記錄檔、gatekeeper.log）放進證據目錄一律經 `Copy-EvidenceFile`
（複製後立即 `Protect-EvidenceFile`），不直接 `Copy-Item`。`compare.mjs --evidence` 以同一套規則
寫 `evidence/compare-<名稱>.log`。
路徑改寫之後，不在路徑裡的裸使用者名稱與電腦名稱（宿主記錄檔的「使用者 <名稱>」等）也改寫成
`%USERNAME%`、`%COMPUTERNAME%`。`EvidenceLog.Tests.ps1` 另以 AST 掃描 `host/tools/*.ps1`：寫入
`$OutDir`（及由它衍生的路徑變數）的 `Set-Content`／`Add-Content`／`Out-File` 必須經
`ConvertTo-EvidenceText`（管線前段或 `-Value` 引數內），重新導向到證據路徑一律不允許。
掃描範圍有限（不是保證）：只看 `host/tools/` 頂層 `.ps1`、只認由 `$OutDir` 經變數賦值衍生的路徑；
經函式參數／雜湊表傳遞的路徑、`[IO.File]::WriteAllText`、`Tee-Object`、`Export-Csv`、子行程自己寫的檔案都
不在偵測範圍（子行程的記錄改經 `Copy-EvidenceFile`）。裸名稱規則只排除前後緊接英數或底線，
所以 `<名稱>.`、`<名稱>-PC` 也會改寫；四個目錄的 8.3 短路徑（GetShortPathNameW）也列入。

```powershell
pwsh -NoProfile -File host/tools/tests/EvidenceLog.Tests.ps1   # 結束碼 0＝全部通過
```

## lib/ScratchWindow.psm1（腳本自己的一般視窗）

需要「一般應用程式視窗」（佔前景／鍵盤焦點、蓋住小工具）時一律用 `Start-ScratchForm` 開自己的
WinForms 表單、`Stop-ScratchForm` 收尾（WM_CLOSE 自己的 HWND，逾時才結束自己啟動的 pwsh 行程）。
**不得借用記事本**：Win11 記事本是單一行程多視窗，以 PID 收尾會連使用者原本開著的記事本一起關掉。

## lib/Occluders.psm1（遮擋前提）

要點的座標被使用者的一般視窗蓋住時，`Clear-Occluders` 暫時最小化那扇視窗，`Restore-Occluders`
在 `finally` 反向還原：最小化前記下 `GetWindowPlacement`、`GetWindowRect` 與 `IsWindowArranged`
（吸附狀態）。還原只用非同步呼叫：先 `ShowWindowAsync(SW_SHOWNOACTIVATE)`（不啟用；原本最大化者回到最大化，
沒回到就補 `ShowWindowAsync(SW_SHOWMAXIMIZED)`），原本吸附或還原後矩形不等於原矩形時再以帶
`SWP_ASYNCWINDOWPOS` 的 `SetWindowPos` 回原矩形（不啟用、不改 z-order；吸附狀態本身不恢復），每一步都輪詢讀回
（`IsIconic`／`IsZoomed`／`GetWindowRect`）、有逾時（預設 3 秒）。最小化前本來就是最小化的不動。
**對他人視窗不得用同步的 `ShowWindow`／`SetWindowPlacement`**：目標 UI 執行緒卡住（例如被系統對話框擋住）時
呼叫端會無限等待（2026-10-06 verify-7.3 收尾卡 30 分鐘）。目標無回應（`IsHungAppWindow`）時不送任何還原呼叫，
記錄「未還原（目標無回應）」並列進 `-NotRestored`；讀回逾時、讀回不符（矩形有變但不是原矩形）同樣列入。
`ShowWindowAsync` 只把最小化排進目標佇列，所以最小化後會輪詢 `IsIconic`（逾時 1.5 秒）記下 `MinimizeConfirmed`；
沒確認的視窗在還原時仍照順序補送一次非同步還原，但判「未還原（最小化未確認）」。視窗已銷毀（`IsWindow`=False）
則只記「已不存在」、不等待、不列入。最小化確認逾時、但還原時已是最小化的視窗（最小化晚到但已生效）走一般還原、
可正常確認。無回應的視窗也不最小化（判 ENV-BLOCKED）。

未還原屬於環境問題、不是產品回歸：每支呼叫 `Restore-Occluders` 的腳本（`tests/Occluders.Tests.ps1` 以 AST
掃描全部 `host/tools/*.ps1`，不寫死清單）都傳 `-NotRestored`，但**不放進逐項結果**；摘要另起一行
`Format-UnrestoredWarning` 產生的警示「⚠ 使用者視窗未還原 N 扇：…，需手動還原」，結束碼一律經
`lib/VerifyVerdict.psm1` 的 `Get-VerdictExitCode`，優先序為產品 FAIL（1）＞ 工作階段鎖定（2）＞ 環境（3，含未還原）。
只有未還原、沒有其他 FAIL 時以 3 結束；同時有產品 FAIL 時以 1 結束，警示行照樣列出。鎖定中途停止、不寫摘要的
腳本（4.7 三支、6.1-icon-click）只把警示印在主控台。
可最小化的只有白名單內的「一般應用程式主視窗」：
可見、有回應、未 cloaked、無 owner、非 `WS_EX_TOOLWINDOW`、非 topmost、非 `WS_DISABLED`、具
`WS_MINIMIZEBOX`、不是系統類別（`Shell_*`、`Windows.UI.Core.CoreWindow`、`#32770` 等）、不屬殼層
explorer 或系統 UI 行程（`PickerHost` 等）。其餘遮擋者（含宿主自己的其他視窗與腳本自己的視窗）
一律不動。系統類別與行程清單在 `lib/SystemUi.psm1`，與 SafeInput 前置探查的前景判斷共用。
清不掉回傳 `Ok=False`，並帶 `Action`（命中類別）與 `Class`。

**判讀（fix F8b）一律經 `Assert-OccluderResult`**（純函式 `Get-OccluderVerdict` 決定），腳本不得自己把
`Ok=False` 一律當成環境問題。按下點是由小工具「目前的實際矩形」推出來的，所以：

| 命中 | 判讀 | 理由 |
|---|---|---|
| 桌面 `Progman`／`WorkerW`、沒命中任何視窗 | FAIL | 小工具不在預期位置、沒有顯示或沉到桌面之下（產品回歸） |
| 宿主自己的另一扇視窗 | FAIL | 小工具互相重疊或 z-order 錯 |
| 腳本自己的視窗 | FAIL | 腳本配置錯誤 |
| 工作列 `Shell_TrayWnd`、系統 UI 與其他不在白名單的視窗、最小化後仍蓋著 | ENV-BLOCKED（結束碼 3） | 環境 |
| 結果缺 `Action` 或無法辨識 | FAIL | 證明不了是環境，就不判成環境 |

**要點的本來就是桌面時（fix F9）**，判讀加 `-TargetIsDesktop`。適用的是
`verify-6.1-icon-click` 的圖示中心，以及 `verify-6.1-appearance-hittest` 的小工具邊外點。
呼叫端以 `Clear-Occluders -HostPid <宿主> -TargetHwnd <桌面視窗>` 清遮擋。此時命中任何
`Progman`／`WorkerW` 都算到達目標，其餘規則不變：宿主視窗、沒命中、腳本自己的視窗判 FAIL；
工作列、系統 UI、最小化後仍蓋著判 ENV-BLOCKED。

`Invoke-MinimizeAppWindows` 把一組視窗（例如全螢幕截圖前的所有一般視窗）照同一份白名單暫時
最小化，不在白名單的跳過並記原因，還原同樣用 `Restore-Occluders`。

**規則（fix F8）：`host/tools/*.ps1` 不得自行處理遮擋或自行最小化視窗**——不得自行定義
`Clear-Occluders`／`Restore-Occluders`／`Get-OccluderAction`／`Get-OccluderVerdict`／
`Assert-OccluderResult`。`ShowWindow`／`ShowWindowAsync` 只准直接呼叫，nCmdShow 必須是顯示／還原類的
允許清單（字面值 1／4／5／8／9，或本檔賦值正確的 `$SW_RESTORE`、`$SW_SHOWNOACTIVATE` 等具名常數），
其餘（變數、運算式、0／2／6／7／11）一律違規；也不得用 `CloseWindow`、`SC_MINIMIZE`（含
`0xF020`／`61472`）、`MinimizeAll`、`ToggleDesktop`、`SetWindowVisualState`、`SetWindowPlacement`。
本模組只匯出經白名單的入口與純函式（fix F8b：最小化原語 `Invoke-MinimizeWindow` 不匯出），腳本不得
進入模組範圍（`& (Get-Module Occluders) { … }` 之類）、不得對入口傳測試用注入參數（`-HitTest`、
`-Minimize`、`-Describe`、`-ShellPid`、`-Restore`）。一律呼叫本模組，並在 `finally` 呼叫
`Restore-Occluders`。使用者：verify-4.7-wheel、verify-4.7-wheelrouting、verify-5.3、
verify-7.5-edit-move、verify-7.6-edit-resize、verify-7.7-aero-snap、verify-dragdpi-esc、
probe-7.6-resize、verify-b-manual-screenshot。`tests/NoInlineMinimize.Tests.ps1` 以語法樹靜態檢查
（`lib/` 不在範圍）。

```powershell
pwsh -NoProfile -File host/tools/tests/ScratchWindow.Tests.ps1
pwsh -NoProfile -File host/tools/tests/Occluders.Tests.ps1
pwsh -NoProfile -File host/tools/tests/NoInlineMinimize.Tests.ps1
```

## lib/ScrollAreaTarget.psm1（滾輪選點與前提，fix F10）

verify-4.7-wheel 與 verify-4.7-wheelrouting 的滾輪點由頁面 `.scroll-area` 的實際位置決定，不用
「視窗矩形＋固定偏移」：PMv2 下 `GetWindowRect` 是實體像素，固定偏移的單位會隨 DPI 感知悄悄改變。
腳本經 CDP 讀清單的 `getBoundingClientRect` 與 `devicePixelRatio`，以 `ClientToScreen` 的客戶區原點
換成實體座標。選點與遮擋時的替代點搜尋都只用清單與 viewport（`innerWidth`／`innerHeight`）的交集，
清單超出視窗也不會選到視窗外。送滾輪前有兩項獨立前提：清單可捲動且尺寸有效（`scrollHeight >
clientHeight > 0`、寬高與 `devicePixelRatio` 皆 > 0、與 viewport 有交集），以及最終點的
`elementFromPoint` 命中 `.scroll-area` 或其子孫。任一不成立就記 FAIL、不送滾輪；選點階段才丟的
PRECONDITION 也會在摘要補一項 FAIL，結束碼非零。wheelrouting 的「scrollTop 不變」判定必須靠這兩項前提才有鑑別力。

```powershell
pwsh -NoProfile -File host/tools/tests/Verify47WheelTarget.Tests.ps1
```

## verify-grid-layout.ps1（task 7.7 格線版面）

```powershell
cd host; cargo build --release
pwsh -File host/tools/verify-grid-layout.ps1 -Tag laptop          # 首次啟動（暫存設定）＋截圖
pwsh -File host/tools/verify-grid-layout.ps1 -HostPid <pid> -Tag x # 只讀一個執行中的宿主
```

- 列舉宿主所有可見的小工具頂層視窗（標題 `fc-host <id>`、類別 `Tauri Window`、未最小化、未
  cloak），依矩形中心歸屬顯示器、取該顯示器 `rcWork`，斷言同一台顯示器上兩兩不相交、每條邊都
  等於 `rcWork 起點 + floor(i × 長度 / 48)` 的某個 i，並輸出反推的格座標。不注入任何輸入。
- 首次啟動模式另外比對預設格座標、以 CDP 讀每個頁面 `.panel` 的 scrollHeight／clientHeight
  （時鐘不得被裁切）、`.panel` 內層 overflow-y 為 hidden／clip 的容器 scrollHeight 不得大於
  clientHeight（捲動區不算）、台股動態事件下緣不低於行情條上緣，並以 `PrintWindow` 逐窗擷取後依相對
  位置合成截圖（只含小工具，背景深灰）：`evidence/7.7-default-<Tag>.png`。
- 內層裁切檢查目前是預防性的：依 `host/ui` 的樣式靜態盤點，五個財經小工具沒有 overflow-y 為 hidden／clip 的內層
  容器（記錄檔會印「檢查 0 個」）。scrollHeight／clientHeight 都是整數捨入，可能有 1px 的誤判。

## verify-5.3.ps1（task 5.3 編輯版面切換，task 7.7 改寫）

```powershell
cd host; cargo clean --release -p fc-host; cargo build --release   # 不帶 self-test-ipc
pwsh -File host/tools/verify-5.3.ps1   # 證據寫到 host/tools/evidence/5.3-*
```

- 以 IPC（CDP 在頁面呼叫 `set_edit_mode`）進出編輯版面：`layout_locked` 同步、所有開啟中小工具
  出現 `body.edit-mode`、拖曳區與 `body::before` 虛線外框（`PrintWindow` 外圈 4px 強調色比例
  驗證外框真的畫在視窗內）、無內容的小工具（行情資料改成空陣列的 quotes）顯示佔位外框；離開後
  全部消失、quotes 回到隱藏。鎖定時與離開後經 `SafeInput.psm1` 真實拖曳 clock，矩形不變。
- 合成輸入無效（preflight 結束碼 3）時兩個拖曳項標為 NOT-RUN、其餘照做，結束碼 3。
- 舊版的「以 `update_settings` 送錨點 placement 模擬拖曳結果」已隨格線版面失效（`update_settings`
  拒收 placement）；移動、調整大小、對齊、彈回與保存改由 verify-7.5／7.6 驗收。
- 已刪除的 `verify-5.3-drag.ps1`（錨點模型的真實拖曳）由 `verify-7.5-edit-move.ps1` 取代。

## verify-7.7-aero-snap.ps1（task 7.7 系統吸附）

```powershell
cd host; cargo clean --release -p fc-host; cargo build --release   # 不帶 self-test-ipc
pwsh -File host/tools/verify-7.7-aero-snap.ps1   # 證據 evidence/7.7-aero-snap<Suffix>*.log
```

- 編輯版面中經 `SafeInput.psm1` 把 clock 上緣拖到螢幕頂端、quotes 下緣拖到螢幕底端、雙擊 clock
  上緣、把 clock 移動到螢幕頂端與左緣；每項放開後斷言矩形＝由記錄推導的格線矩形、
  `GetWindowPlacement` showCmd＝一般、`IsZoomed`／`IsWindowArranged` 皆否、rcNormalPosition＝
  目前矩形，並再進出一次編輯版面複查。
- 拖曳確實發生也列入判定（fix F1）：S1／S2／S4／S5 各要求按住期間尺寸／移動迴圈取樣數 > 0、
  按住時矩形離開拖曳前位置，S2／S4／S5 另要求按住時出現紅框；按下沒打到把手時這幾項 FAIL，
  不會被「完全沒動」空真滿足。雙擊（S3）只能做負向判定。
- `-ProbeNoMaximizeBox`：進入編輯版面後由腳本拿掉 `WS_MAXIMIZEBOX`（task 7.7 修正前用來確認機制
  的探針，證據 `7.7-aero-snap-probe-nomaxbox*.log`）；修正前的失敗證據為
  `7.7-aero-snap-before-fix*.log`。

## verify-zoom-reload.ps1（WebView2 故障復原後的內容倍率）

- task 7.7 起倍率由視窗寬度決定（zoom＝矩形邏輯寬 ÷ 設計寬度，夾 0.5–3），不再改設定：斷言
  baseline、renderer 終止後 `Reload()`、browser 終止後全部重建三個時點，頁面
  `devicePixelRatio` 都等於由視窗實體寬與 DPI 推算的值（±0.01）。

## verify-grid-overlay.ps1（widget-adaptive-zoom-and-grid task 6.2：編輯版面格線）

```powershell
cd host; cargo clean --release -p fc-host; cargo build --release   # 不帶 self-test-ipc
pwsh -File host/tools/verify-grid-overlay.ps1   # 證據 evidence/grid-overlay-*
```

- 以 CDP 呼叫 `set_edit_mode` 進出編輯版面兩輪，列舉宿主行程類別 `fc-host-grid-overlay` 的頂層視窗：數量＝
  `Screen.AllScreens` 台數、矩形＝各台 `WorkingArea`（一對一）、延伸樣式含 LAYERED／TRANSPARENT／NOACTIVATE／
  TOOLWINDOW、`WM_NCHITTEST` 回 HTTRANSPARENT；前景視窗前後不變；離開後（含隱藏）歸零，第二輪沒有重複。
- 穿透：每台挑「除了格線與桌面之外沒有任何會接受命中測試的視窗」的格子中心與加強線上各一點，
  `WindowFromPoint` 的根視窗必須是桌面，並記錄格線在 z-order 上是否位於該桌面之上（有鑑別力）。某台整片
  被使用者視窗蓋住時記 NOTE（不最小化使用者視窗），全部都沒測到才 NOT-RUN。判定前先輪詢（每 50 ms、上限 2
  秒）等 z-order 穩定，直到每個格線都在與它重疊的桌面視窗（Progman／WorkerW）之上：格線剛建立時
  `SetWindowPos(HWND_BOTTOM)` 會暫時落到 Progman 之下，explorer 約百毫秒內再把 Progman 壓回最底，取樣過早
  會誤報「有鑑別力」項目；等待時間記在 driver 記錄。逾時（任何格線仍在桌面視窗之下）另有一個結果項判 FAIL，
  寫出逾時的格線與螢幕；「整台工作區被視窗蓋住」的 NOTE 只豁免穿透取樣，不豁免逾時。
- 前景：判準對準規格「前景視窗與焦點不因格線而改變」，兩輪各比一次——相同 → PASS；進入後前景屬於宿主行程
  （pid 比對，任何小工具或格線）→ FAIL；換成別的行程的視窗 → NOT-RUN（不是宿主造成，但無法證明格線沒動到焦點，
  不得記為成功；摘要列出前後類別與 pid，結束碼 3，前景穩定時重跑，例如使用者點擊或終端機取回前景）。
- 判定函式（`Get-ForegroundResult`、`Get-OverlayDesktopProblems`）在 `lib/VerifyVerdict.psm1`，不需宿主即可測：
  `pwsh -NoProfile -File host/tools/tests/VerifyVerdict.Tests.ps1`（涵蓋非宿主前景切換 → NOT-RUN、宿主前景切換 →
  FAIL、格線在桌面之下 → 逾時 FAIL 並指出螢幕）。
- z-order：`lib/ScratchWindow.psm1` 開自己的表單，走訪 z-order 確認它在格線之上、格線之下沒有一般視窗；
  同時背景跑 `watch-zorder.ps1 -ProcessId <宿主>`（在建立自己的表單之前啟動，免得啟動過程干擾進入前的前景），
  事後判讀每筆含格線的記錄 below=(none)。
- 線條：`PrintWindow(PW_RENDERFULLCONTENT)` 只擷取格線視窗本身（分層視窗內容疊在黑底，不含使用者視窗），
  在格子中心那一列／欄逐像素比對 47 條垂直＋47 條水平線＝`起點 + floor(i × 長度 ÷ 48)`，抽查第 6、12、24 條
  加強線。截圖 `grid-overlay-lines-m<N>.png`（只有線）與 `grid-overlay-composite-m<N>.png`（該台小工具以
  `PrintWindow` 貼上、線疊在最上層）。開跑前先刪上次的同名截圖與記錄；截圖失敗、存檔失敗或不是本次新檔都算 FAIL。
- 已有 fc-host 在跑（含使用者裝的正式版：單一執行個體會把隔離的宿主交接給它）或工作階段鎖定 → BLOCKED
  （結束碼 2），不結束任何既有行程。

## verify-adaptive-zoom.ps1（widget-adaptive-zoom-and-grid task 6.3：自適應倍率與字級）

```powershell
cd host; cargo clean --release -p fc-host; cargo build --release   # 不帶 self-test-ipc
pwsh -File host/tools/verify-adaptive-zoom.ps1                    # 全部情境，證據 evidence/adaptive-zoom-*
pwsh -File host/tools/verify-adaptive-zoom.ps1 -Scenarios A,D150   # 只跑部分情境
pwsh -File host/tools/verify-adaptive-zoom.ps1 -DryRun             # 只印本機換算的格座標與預期倍率，不啟動宿主
```

- 情境 A（時鐘約 2.5 倍最小框）、B（行情條寬不變、加高）、C（總經日曆邏輯約 1000×324）、D070／D150（預設版面、
  `font_scale` 0.7／1.5）。每個情境在全新隔離資料夾、啟動前寫 settings.json（`version` 讀自 settings.rs 的
  `SETTINGS_VERSION`），格數依主螢幕工作區與縮放換算。
- 預期倍率＝`verify-visual-edges.mjs` 匯出的 `contentZoom()`／`widgetZoomBoxes()`（同 Rust `content_zoom`、
  從 `WIDGET_SPECS` 讀框），輸入為實際 `GetWindowRect` 與 `GetDpiForWindow`；實際倍率＝頁面
  `devicePixelRatio ÷ 縮放`，容差 0.01。各情境另以獨立簡式交叉核對意圖（A：min(寬/212, 高/160, 3)；B：
  min(高/60, 3)；C：0.9–1.1 且由高度決定；D：清單＝min(字級 1 時 × 字級, 上限)、D150 時鐘與行情條＝上限）。
- 無裁切：時鐘 `.panel` 不溢出、三列文字字寬不超出列寬且溢出部分仍在面板與視窗內；行情條面板高度不溢出；
  清單面板寬度不溢出。B 另驗跑馬燈 `.tlist` 的 scrollLeft 1.5 秒內有變化。截圖 `adaptive-zoom-<情境>-<id>.png`
  只截小工具視窗本身（`PrintWindow`），每張都是具名結果：情境開始前先刪該情境的舊檔，PrintWindow 或存檔失敗、
  存完不是本次新檔都算 FAIL。開機自啟登錄快照之後以單一頂層 try／finally 包住所有情境（Ctrl+C 也會停宿主、還原登錄）。
- 已有 fc-host 在跑或工作階段鎖定 → BLOCKED（結束碼 2）。

## 其他為格線版面更新的腳本（task 7.7）

- `verify-6.1-appearance-hittest.ps1` 的尺寸迴歸改為「矩形＝預設格子」；`verify-5.2.ps1` 的縮放
  提示檢查改為「設定頁不再出現縮放」；`verify-3.5.ps1`／`verify-5.1.ps1` 只改說明文字。
- `verify-b6-resolution.ps1` 改為判定「變更解析度前後 fixed 都＝預設格座標在當時工作區的格線
  矩形」。這支會實際切換顯示解析度，task 7.7 只改判準、未重跑。fix F4 起改為逐扇比對所有可見
  小工具，另加 placement 不變、兩兩不相交、`applied=N` 等於可見數，以及還原前查鎖定並讀回。

## 收尾只停自己啟動的宿主（fix F4）

- 驗收腳本不得以行程名稱停止 fc-host；一律經 `lib/ProcessTree.psm1` 的 `Stop-ProcessTree`，只停
  `Start-Process -PassThru` 取得的宿主與其子孫（含 PID 重用防護）。
- 「已有 fc-host 在執行」的防呆放在任何 try／finally 之前，直接結束、不進入清理。
- `tests/ProcessTree.Tests.ps1` 靜態掃描 `host/tools/*.ps1` 守住這兩條。
- `verify-b10-monitorpower.ps1` 會關閉顯示器，必須有人在場並加 `-UserPresent`，否則直接
  BLOCKED（結束碼 4）；建議排在整批最後。

## probe_wallpaper（dynamic-wallpaper task 1.1／1.2：explorer 負擔與設桌布廣播探針）

拋棄式探針 `examples/probe_wallpaper.rs`：以 `IDesktopWallpaper` 對所有在線螢幕每 10 秒交替設兩張 4K
PNG，同時量 explorer 的資源負擔（1.1）與隱藏頂層視窗收到的廣播訊息（1.2）。會改使用者的桌布，
但備份先於一切、三種結束（正常、Ctrl+C、錯誤／panic）都會還原並讀回比對。

```powershell
cd host; cargo build --release --example probe_wallpaper
bash host/tools/make-dw-probe-pngs.sh                       # 產生測試圖到 %LOCALAPPDATA%\fc-probe-wallpaper\{a,b}.png
pwsh -File host/tools/probe-dw-1.1.ps1 -Smoke               # 冒煙：基線 30 秒＋設定 120 秒＋冷卻 30 秒，證據檔 -smoke；會等到結束
pwsh -File host/tools/probe-dw-1.1.ps1 -Smoke -Tag smoke2 -BaselineSecs 20 -SetSecs 60 -CooldownSecs 20   # 縮短的冒煙
pwsh -File host/tools/probe-dw-1.1.ps1                      # 完整連跑：10 分鐘＋2 小時＋10 分鐘，證據檔 -full（約 2 小時 20 分）；啟動後立即返回
# 直接呼叫（每段時長都能調；只適合短時間測試，完整連跑不要這樣跑，見下方「啟動方式」）：
host/target/release/examples/probe_wallpaper.exe --baseline-secs 600 --set-secs 7200 --cooldown-secs 600 --tag full --out-dir host/tools/evidence
# 單獨還原（任何時候；備份在 %LOCALAPPDATA%\fc-probe-wallpaper\backup-<時間>.json）：
host/target/release/examples/probe_wallpaper.exe --restore <備份.json>
```

- **啟動方式（完整連跑必讀）**：驅動稿用 `Start-Process` 在**獨立的新主控台視窗**（最小化）啟動探針，
  stdout／stderr 導向 `%LOCALAPPDATA%\fc-probe-wallpaper\console\probe-<tag>.{stdout,stderr}.txt`，
  證據記錄由探針自己寫進 `--out-dir`。**不可由工具（Bash／PowerShell 工具的前景或背景）直接執行完整
  連跑**：完整連跑約 2 小時 20 分，超過工具的背景逾時上限（預設 30 分鐘、最長 2 小時），逾時時工具會
  強殺行程樹、探針來不及還原；父行程死亡後 stdout 管線也會斷。工具環境的子行程還常繼承「忽略 Ctrl+C」
  旗標，探針啟動時會自己 `SetConsoleCtrlHandler(NULL, FALSE)` 清除；在探針的主控台視窗內按 Ctrl+C
  即可中止並還原。啟動後以輪詢 `dw-1.1-<tag>.log`（`DONE`、`RESTORE 讀回結論` 行）與 CSV 追蹤，
  行程結束碼見驅動稿說明。若探針被強制結束而沒還原，用上面的 `--restore`。

- **測試圖**：`make-dw-probe-pngs.sh` 以 headless Edge 載入 `assets/design-explore/05-astrolabe/bg-sessions.html`
  （`layout=full&w=3840&h=2160`，`t` 取 `2026-10-02T09:00` 與 `T21:00` 兩個不同時刻），在 iframe 內
  直接 `canvas.toDataURL` 匯出像素（不截圖：headless viewport 比視窗小約 30px，截圖邊緣有接縫），
  各約 7 MB。PNG 不進 git；`assets/` 不進 git，只有本機才有。
- **證據檔**（`--out-dir`，預設 `%LOCALAPPDATA%\fc-probe-wallpaper\out`，正式交付用
  `host/tools/evidence/`）：
  - `dw-1.1-<tag>.csv`：每 30 秒一列，explorer 與探針自身各一組 CPU／工作集／私有位元組／控制代碼／
    GDI／USER 含 peak，另有鎖定旗標、PID 改變旗標、取樣間隔 `gap_s`。
  - `dw-1.1-<tag>.log`：備份內容、每次 `SetWallpaper` 的精確時間／耗時／HRESULT、還原與讀回比對、
    趨勢摘要、WARN。
  - `dw-1.2-<tag>.log`：每個設桌布週期後 3 秒內收到的訊息依類型計數＋逐筆明細、基線／冷卻對照、
    總摘要。
  - 寫出前把 `%LOCALAPPDATA%`／`%APPDATA%`／`%USERPROFILE%`／`%TEMP%` 改寫成字樣。
- **訊息視窗**：一般頂層視窗（非 message-only，後者收不到廣播）、無擁有者、不顯示，在專屬執行緒跑
  訊息迴圈。基線 2 秒後會**廣播一則對照用的 `WM_SETTINGCHANGE`（lParam `fc-probe-control`）**，
  證明視窗收得到廣播；log 裡沒看到這筆就代表「收到 0 則」不可信（`--no-control` 可關閉）。
  `CYCLE`／`OUTSIDE` 行是視窗期結束後才寫檔，行首時間戳是寫檔時間，事件時間看 `start=`／`t+` 欄位。
- **防待機**：從基線開始到還原完成，主執行緒持有 `SetThreadExecutionState(ES_CONTINUOUS |
  ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED)`（本機 Modern Standby，顯示器一關就鎖定並待機）；
  結束時清除。不改電源計畫或登錄。兩次取樣間隔超過 90 秒會在 `dw-1.1-<tag>.log` 記 `WARN`
  （行程曾被凍結）。
- **拒絕執行**：工作階段鎖定（`LogonUI.exe` 在跑，`--allow-locked` 可略過）、桌布為投影片模式
  （探針無法還原投影片）時結束碼 3；測試圖不存在、或與使用者目前桌布路徑相同時結束碼 2。以上都
  發生在改任何桌布之前。探針只對自己的兩張測試圖呼叫 `SetWallpaper`，不刪不覆寫任何使用者圖檔。
- **還原**：只在目前值與備份不同時才寫入；寫完讀回逐項比對（各螢幕 `GetWallpaper`、全域
  `GetPosition`／`GetBackgroundColor`、`HKCU\Control Panel\Desktop` 的 `Wallpaper`／`WallpaperStyle`／
  `TileWallpaper`）。`SetWallpaper` 會把登錄的 `Wallpaper` 改成
  `%APPDATA%\Microsoft\Windows\Themes\TranscodedWallpaper`，COM 還原不會帶回，探針會把這三個登錄值
  原樣寫回並記錄。結束碼：0 成功、2 參數／環境錯誤、3 BLOCKED、4 讀回不一致、5 執行中出錯
  （已還原且一致）。
- **還原不依賴主控台與舊的 COM 介面**：`IDesktopWallpaper` 由 explorer 承載，explorer 重啟後手上的介面
  會斷線，所以還原一律重新 `CoCreateInstance`（最多 15 次、每次間隔 2 秒，等新殼層就緒）；COM 始終
  不可用時仍把三個登錄值寫回、結束碼 4，螢幕桌布留給事後 `--restore`。主迴圈遇到列舉失敗或
  `SetWallpaper` 失敗、或觀察到 explorer PID 改變，下一個週期前重建介面並記 `COM-RECREATE`；
  列舉失敗、沒有在線螢幕、全部設定失敗都算「失敗週期」，連續 20 個就中止並還原。console 輸出一律
  忽略寫入錯誤（stdout 管線斷掉不會 panic），且還原先於任何記錄輸出。
- **測試用開關**：`--test-stop-secs N`（N 秒後模擬 Ctrl+C）、`--test-panic-secs N`（N 秒後主迴圈
  panic），驗證中止路徑仍還原；`--test-drop-dw-secs N`＋`--test-fail-creates M`（N 秒後丟掉 COM 介面、
  之後 M 次建立失敗），驗證主迴圈重建與失敗計數；`--test-restore-fail-creates M`（還原前讓 M 次建立
  失敗），驗證還原重試與登錄後備寫回。`--backup-only` 只寫備份 JSON 並印出目前狀態。
- **task 1.7（GDI 累積受控重測）的選用參數**（不給時與 1.1 完全相同，CSV 仍是 27 欄）：
  `--monitor <idx>` 只設一台（序號＝`GetMonitorDevicePathAt`，其餘螢幕不呼叫 `SetWallpaper`）；
  `--sample-after-set-secs <n>` 設桌布階段改成「每次設定後 n 秒一列」、不再週期取樣（基線／冷卻仍照
  `--sample-secs`）；`--extra-cols` 在 CSV 末尾追加 `sample_kind`（periodic／post_set／final）、
  `set_cycle`、`idle_s`（`GetLastInputInfo`，只讀）、`exp_top_windows`／`exp_top_visible`（`EnumWindows`
  篩 explorer PID 的頂層視窗數與其中可見者）。驅動稿 `probe-dw-1.7.ps1`（`-Smoke` 縮短冒煙並等待；預設
  基線 10 分鐘＋設定 6 小時＋冷卻 10 分鐘、啟動後立即返回，證據 `dw-1.1-1.7-full.*`）**不做輸出導向**，
  避開 `Start-Process` 導向時繼承呼叫端管線的問題。

## probe-dw-1.4.ps1（dynamic-wallpaper task 1.4：宿主內渲染成本探針）

`fc-host --probe-render`（`probe-render` cargo feature，`src/probe_render.rs`）在 release 宿主內以隱藏
WebView2 視窗（label `wallpaper-renderer`）畫星盤正式頁 3840×2160，收 PNG 位元組，量耗時與記憶體。
這個模式在 `main()` 第一行就分支，不經過記錄檔初始化、重新啟動註冊、單一執行個體仲裁、開機自啟、設定
載入，也不建立小工具與系統匣、不碰桌布。

```powershell
cd host; cargo build --release --features probe-render
pwsh -File host/tools/probe-dw-1.4.ps1                                  # 兩種模式各 10 次，證據 -run
pwsh -File host/tools/probe-dw-1.4.ps1 -Modes open-close,persistent -Tag main
pwsh -File host/tools/probe-dw-1.4.ps1 -Modes persistent-redraw -Tag main
pwsh -File host/tools/probe-dw-1.4.ps1 -Modes open-close -Anchor -Tag anchor
pwsh -File host/tools/probe-dw-1.4.ps1 -Modes persistent -ControllerHidden -Tag ctlhidden
pwsh -File host/tools/probe-dw-1.4.ps1 -Modes persistent -Runs 40 -Tag long40
# 量完務必不帶 feature 重建，正式 exe 才不含探針：
cd host; cargo build --release
```

模式：

- `open-close`：每次建立隱藏視窗 → 載入 → 收 PNG → 關閉。
- `persistent`：視窗只建立一次，之後以新的 `t` 重新導覽（`navigate`）。
- `persistent-redraw`：視窗只建立一次，之後以 `eval` 在同一份文件內重新呼叫 `runWallpaper`，不重新導覽。
- `-Anchor`：先建立一個隱藏的錨點 webview 並保持到結束，模擬正式宿主中小工具讓 browser 行程常駐。
- `-ControllerHidden`：建立後再把 WebView2 controller 設成不可見，頁面會變成 `document.hidden`。

`t` 從 `2026-10-05T00:30:00Z` 起每次加 15 分鐘；資料走頁面 fixture 模式（`fixture=../fixtures/tw-events.json`）。
`persistent*` 模式在 10 次之後另做額外量測：toBlob、toDataURL、OffscreenCanvas 三種匯出的比較；同一份
位元組以原始本體與 JSON 數字陣列各送一次（JSON 陣列與 postMessage 退回時的本體形狀相同）；
`x-wallpaper-meta` 從 1 KiB 測到 4 MiB 的長度上限。

驅動稿的規則：

- 前置檢查：工作階段未鎖定、沒有任何 fc-host 在跑（有就停下回報，不結束它）、exe 內含探針建置標記
  `FC_HOST_PROBE_RENDER_BUILD`。
- 隔離：以 `%TEMP%\fc-probe-render-<時間>` 的子目錄覆寫 `APPDATA`、`LOCALAPPDATA`、
  `WEBVIEW2_USER_DATA_FOLDER`，並以 `--webview-data` 明確傳給探針。探針沒拿到這兩者其中之一就拒絕執行。
  執行中讀自己行程樹內 msedgewebview2 的 `--user-data-dir`，確認它落在隔離根目錄內。執行前後也比對真實
  `%LOCALAPPDATA%\tw.fintools.fc-host` 底下 `EBWebView` 與 `logs` 的最後寫入時間。
- 收尾：只用 `lib\ProcessTree.psm1` 停止自己 `Start-Process` 出來的行程樹，不以名稱停止任何行程。
  單一模式逾時（`-TimeoutSecs`，預設 900）時也一樣處理。

產出都在 `host/tools/evidence/`：

| 檔案 | 內容 |
| --- | --- |
| `dw-1.4-<tag>-<mode>.csv` | 每次一列，欄位見 `probe_render.rs` 的 `CSV_HEADER`：三段耗時、PNG 大小與 IHDR 寬高、本體形式、meta 長度、收到 PNG 當下與閒置 2 秒後的宿主／WebView2 工作集與私有位元組、取樣峰值、可見性、前景、延伸樣式 |
| `dw-1.4-<tag>-<mode>.log` | 探針逐步事件、`EXTRAS REPORT`、`SUMMARY`（中位數、最大值、峰值、閒置趨勢斜率） |
| `dw-1.4-<tag>-<modes>-driver.log` | 前置檢查、`UDF`（WebView2 使用者資料夾核對）、真實資料夾前後對照、殘留行程、`VERDICT` |
| `dw-1.4-<tag>-<mode>-runNN.png` | 第一次與最後一次的 PNG（提交時只留整組的第一張與最後一張） |

結束碼：0 全部成功且隔離核對通過；1 有失敗；2 參數或環境錯誤；3 BLOCKED（鎖定中）。

## probe-dw-1.5.ps1／probe-dw-1.5-summarize.ps1（dynamic-wallpaper task 1.5：螢幕識別探針）

**使用者在場時**，插拔外接螢幕、切換縮放、切換解析度的**每個動作前後各取一次樣**；取樣腳本每執行一次就在
jsonl 尾端附加一筆，彙整腳本把相鄰兩筆做差異比對、輸出 markdown 表格，結論寫回 design.md D4。

```powershell
$o = "$env:TEMP\fc-dw15.jsonl"
pwsh -NoProfile -File host/tools/probe-dw-1.5.ps1 -Label before-unplug-4k -Out $o
# ……使用者拔掉外接 4K ……
pwsh -NoProfile -File host/tools/probe-dw-1.5.ps1 -Label after-unplug-4k -Out $o
pwsh -NoProfile -File host/tools/probe-dw-1.5-summarize.ps1 -Path $o                      # 輸出 markdown
pwsh -NoProfile -File host/tools/probe-dw-1.5-summarize.ps1 -Path $o -OutFile $env:TEMP\fc-dw15.md
```

- **只呼叫查詢型 API**：不設桌布、不改顯示設定、不啟動也不碰 fc-host、不注入輸入；沒有前置檢查，工作階段
  鎖定時也能跑（但鎖定中的取樣沒有意義，動作要在使用者在場時做）。
- **每筆紀錄**：`IDesktopWallpaper` 的 `GetMonitorDevicePathCount`、每個索引的 `GetMonitorDevicePathAt`
  與 `GetMonitorRECT`（**每個呼叫都記 HRESULT**，離線螢幕會回錯或 `S_FALSE`，照實記錄；RECT 在執行緒預設
  DPI 感知與 per-monitor v2 下各讀一次，回答 D4「RECT 是否為實體像素」）；`probe-monitor-ids.ps1` 那套資料
  （`QueryDisplayConfig` 的 GDI 名稱對應 device path、`EnumDisplayMonitors` 的 `szDevice`、邊界、工作區、
  主螢幕、有效 DPI 與縮放，以及模擬 `monitors_from_tauri` 得到的穩定 id）；`correlation`＝每台顯示器的穩定 id
  對到 `IDesktopWallpaper` 的哪個索引（依裝置路徑字串、依 RECT 各算一次，`agree` 表示兩種對應一致）。
- **共用型別**：`lib/MonitorIdsType.ps1` 是從 `probe-monitor-ids.ps1` 抽出的 Win32 型別（dot-source 載入，
  `probe-monitor-ids.ps1` 輸出不變；僅 `MonitorRow` 多了 `DpiX`／`DpiY`）。`IDesktopWallpaper` 的 helper 宣告出自
  `lib/InstallerSnapshot.psm1`，差別是這裡用 `PreserveSig` 取得 HRESULT。
- **去識別**：整筆 JSON 寫檔前經 `lib/EvidenceLog.psm1` 的 `ConvertTo-EvidenceText`；彙整輸出也經過一次。
  device path 本身不含使用者名稱，但電腦名稱或使用者路徑一旦出現就會被替換。
- **彙整表**：每對相鄰取樣兩張表。IDesktopWallpaper 表以 `GetMonitorDevicePathAt` 的路徑字串為鍵，列出索引、
  RECT、HRESULT 的前後變化；穩定識別表以穩定 id 為鍵，列出 GDI 名稱、邊界、縮放、對到的 DW 索引（依路徑／
  依 RECT）的前後變化，判定「id 不變而其他欄位改變」＝對應是靠穩定 id 而非索引或 GDI 名稱才站得住。
- 輸出放暫存區，不要 commit；要引用結論時摘要寫進 design.md D4。

## measure-dw-4.5.ps1（dynamic-wallpaper task 4.5：渲染管線實測）

`fc-host --self-test-render`（`self-test-ipc` cargo feature，`src/self_test_render.rs`）**走正常啟動**：
小工具、系統匣、置底守門照常建立，`setup` 完成後另開執行緒，以正式渲染管線（`src/wallpaper_render.rs`：
每次開關的隱藏視窗、獨立 WebView2 環境）依固定間隔出圖，寫進輸出資料夾並讀回驗證尺寸，最後
`app.exit(結束碼)` 正常結束。驅動稿負責隔離、記憶體取樣與摘要。

```powershell
cd host; cargo clean --release -p fc-host; cargo build --release --features self-test-ipc
# 4K 單張（驗尺寸）
pwsh -File host/tools/measure-dw-4.5.ps1 -Tag 4k -Count 1 -WarmupSec 30 -TailSec 15
# 加速曲線：每 1 分鐘、30 分鐘
pwsh -File host/tools/measure-dw-4.5.ps1 -Tag accel -IntervalSec 60 -DurationSec 1800 -WarmupSec 180 -TailSec 180
# 量完務必不帶 feature 重建，正式 exe 才不含 self-test：
cd host; cargo build --release
```

正式曲線（每 15 分鐘、2 小時，各約 2 小時 10 分；兩組**分開、先後**跑，不可同時——前置檢查要求沒有其他 fc-host）：

```powershell
cd host; cargo build --release --features self-test-ipc; cd ..
# 渲染組：第 0、15、…、120 分鐘共 9 次，第 5 次（第 60 分鐘）改畫一定不回報的頁面，走一次真實逾時
pwsh -File host/tools/measure-dw-4.5.ps1 -Tag formal -Count 9 -IntervalSec 900 -WarmupSec 300 -TailSec 300 -SampleMs 2000 -SlopeFromSec 600 -ForceTimeoutAt 5
# 同長度的不渲染對照組：同一個時刻表，宿主 --skip-render
pwsh -File host/tools/measure-dw-4.5.ps1 -Tag formal-control -NoRender -Count 9 -IntervalSec 900 -WarmupSec 300 -TailSec 300 -SampleMs 2000 -SlopeFromSec 600
cd host; cargo build --release
```

判讀：以 summary 的 `SLOPE 暖機後`（第 `-SlopeFromSec` 秒起、閒置樣本對時間的迴歸）與 `M1 暖機後` 比較兩組，
不要用 `SLOPE 完整`（含暖機段，前約 10 分鐘的上升會被線性外推）；`M1 結尾穩態` 是尾段後半、宿主結束前 2 秒以前的
閒置樣本中位數（不取最後一筆——那是結束過程中的暫態）。

`--self-test-render` 的參數（也可以不經驅動稿直接下，但要自己隔離）：`--render-log <path>`（必填）、
`--count N` 或 `--duration-secs D`（換算成 `max(1, D / 間隔)` 次）、`--interval-secs S`（預設 60，相鄰兩次
**開始**的間隔）、`--warmup-secs W`（第一次之前先等，當基準）、`--tail-secs T`（最後一次之後再等，看殘留）、
`--width`／`--height`（預設 3840×2160）、`--theme`（預設 astrolabe）、`--no-fixture`（改走 `wallpaper`
通道；4.6 之前通道不存在，頁面畫缺資料畫面）。`t` 從 `2026-10-05T00:30:00Z` 起每次加 15 分鐘。結束碼：
0 全部成功且讀回尺寸正確；1 有失敗；2 參數錯誤。驅動稿另有 `-NoFixture` 對應 `--no-fixture`。

- `--timeout-at N`（驅動稿 `-ForceTimeoutAt N`）：第 N 次改畫 self-test 專用頁 `wallpapers/__self-test-never-reports__.html`
  （`host/ui/` 內確實存在的檔，正式頁面不會連到它；只有一段停在永不完成的 Promise 上的模組腳本，不載入共用模組、
  不碰 IPC，所以永遠不回報。檔案必須存在：資產找不到時 Tauri 會退回載入根目錄的 `index.html`），在真實宿主上走
  一次 30 秒逾時。宿主記錄 `RENDER-FORCED-TIMEOUT … window_closed=Some(true) PASS`
  （回 `Timeout` 且視窗已關閉並等到 `Destroyed`）；驅動稿另核對渲染那組 browser 行程在逾時後結束（`FORCED-TIMEOUT ok`）。
- `--skip-render`（驅動稿 `-NoRender`）：對照組，同一個時刻表照記 `RENDER-START`，但不建立視窗（`RENDER-SKIPPED`）；
  驅動稿改核對「沒有出現任何渲染視窗」（`CONTROL ok`）。不能與 `--timeout-at` 同時使用。
- 驅動稿其他參數：`-SlopeFromSec`（預設 600，暖機後斜率的起點）、`-UdfSizeEverySec`（預設 30，量渲染視窗資料夾
  `wallpaper-renderer` 大小的間隔，寫進 CSV 的 `renderer_udf_mb`／`renderer_udf_files`，結束後另記 `RENDERER-UDF-FINAL`）。
- 渲染視窗的 label 是 `wallpaper-renderer-<render id>`（每次一個，前綴判斷見 `wallpaper_render::is_renderer_label`）。

驅動稿的規則：

- 前置檢查：工作階段未鎖定、沒有任何 fc-host 在跑（有就停下回報，不結束它）、exe 內含
  `FC_HOST_SELF_TEST_RENDER_BUILD`。
- 隔離：`APPDATA`、`LOCALAPPDATA`、`WEBVIEW2_USER_DATA_FOLDER` 只對啟動的子行程指向
  `%TEMP%\fc-measure-4.5-<時間>`；資料目錄放一份 `host/ui/fixtures/tw-events.json` 讓小工具有資料。宿主
  啟動時接手 `WEBVIEW2_USER_DATA_FOLDER`（`src/webview_env.rs`；WebView2 的這個環境變數會取代呼叫端指定的
  資料夾，不接手的話渲染視窗會併進小工具那組）：小工具那組＝`<它>\EBWebView`，渲染視窗＝
  `<它>\wallpaper-renderer\EBWebView`。另一條執行緒每 50 ms 讀宿主新子行程（各環境的 browser 行程）的
  `--user-data-dir`，核對兩組都在隔離根目錄內且是不同的資料夾（`INDEPENDENT ok`）。執行前後比對真實
  `%LOCALAPPDATA%\tw.fintools.fc-host` 的 `EBWebView`、`wallpaper-renderer`、`wallpaper`、`logs`。
- 取樣（每 `-SampleMs`，預設 500）：與 `soak-6.2.ps1` 同一基準——宿主＋其全部 msedgewebview2 子孫的
  `PrivateMemorySize64`（`total_private_mb`），另依所屬 browser 行程分成 `widget_*`／`renderer_*`。只用
  `Get-Process` 快照（父 PID 取 `Process.Parent`），不做 CIM 全表查詢（約 0.7 秒，渲染那組整個生命期才約 1 秒）。
- 收尾：宿主自己正常結束；逾時才以 `lib\ProcessTree.psm1` 停自己啟動的行程樹。結束 5 秒後再查一次
  fc-host（`RELAUNCH-CHECK`），確認沒有被 Restart Manager 重新啟動（self-test 建置仍會 `RegisterApplicationRestart`）。

產出都在 `host/tools/evidence/`：

| 檔案 | 內容 |
| --- | --- |
| `dw-4.5-<tag>.csv` | 記憶體曲線：`ts_ms`（UNIX 毫秒）、宿主、WebView2 合計、`total_private_mb`、兩組各自的行程數與私有記憶體、渲染視窗資料夾大小與檔案數 |
| `dw-4.5-<tag>-render.log` | 宿主的 self-test 記錄：每次 `RENDER-START`／`RENDER-OK`（render id、a/b、各段耗時）／`RENDER-FORCED-TIMEOUT`／`RENDER-SKIPPED`、`VERIFY`、`SUMMARY` |
| `dw-4.5-<tag>-summary.log` | M1 每次渲染前的閒置水位、結尾穩態與斜率（含暖機後）、`SLOPE` 完整／暖機後的時間迴歸、M2 冷啟動耗時、關窗後渲染那組歸零時間與強制逾時結果、`UDF` 資料夾大小、M3 峰值 |
| `dw-4.5-<tag>-driver.log` | 前置檢查、`BROWSER`／`UDF`（WebView2 使用者資料夾核對）、`INDEPENDENT`、`RELAUNCH-CHECK`、真實資料夾前後對照、`VERDICT` |
| `dw-4.5-<tag>-host-*.log` | 隔離環境裡宿主自己的記錄檔（渲染開始／完成／失敗都有一行） |
| `dw-4.5-<tag>-last.png` | 最後一張輸出圖 |

結束碼：0 宿主結束碼 0 且隔離與獨立環境核對通過；1 有失敗；2 參數或環境錯誤；3 BLOCKED（鎖定中）。

## repro-dw-leaked-renderer.ps1（bug-leaked-renderer：渲染 browser 行程殘留的重現與故障注入）

同 `measure-dw-4.5.ps1` 的隔離與前置檢查（未鎖定、沒有其他 fc-host、exe 含 self-test 標記），以
`--self-test-render` 連續渲染，記每次渲染那組 browser 行程何時結束、下一次開始時前一個是否仍存活、新渲染是否
併進前一個 browser 行程、以及殘留（尾段後仍存活、宿主結束後仍存活）。宿主預設主題「不接管」，只渲染、不設桌布。

```powershell
cd host; cargo build --release --features self-test-ipc; cd ..
# 正式排程的節奏：同一輪兩台（2560×1600、3840×2160）間隔約 1 秒，兩輪之間閒置
pwsh -File host/tools/repro-dw-leaked-renderer.ps1 -Tag prodlike -Count 1000 -GapMs 1000 -AltSize 3840x2160 -PairIdleMs 3000
# 掃關窗後的間隔（100 ms 內的下一次會併進前一個 browser 行程）
pwsh -File host/tools/repro-dw-leaked-renderer.ps1 -Tag gap100 -Count 200 -GapMs 100
# 故障注入：第 5、10、15 次返回後立刻暫停它的 browser 行程（模擬卡在結束流程）
pwsh -File host/tools/repro-dw-leaked-renderer.ps1 -Tag inject -Count 20 -GapMs 2000 -SuspendAt 5,10,15
cd host; cargo build --release
```

- 宿主參數（`src/self_test_render.rs`）：`--gap-ms G`（每次返回後等 G 毫秒開始下一次，取代 `--interval-secs`）、
  `--alt-size WxH`（偶數次改用這個尺寸）、`--pair-idle-ms P`（與 `--gap-ms` 並用：每兩次之後改等 P 毫秒）。
- 宿主記錄：`PREV-BROWSER … alive=`（下一次開始時前一個是否仍存活）、`BROWSER-EXIT …
  after_return_ms=`／`BROWSER-NOT-EXITED`（60 秒內沒結束）、`BROWSER-REUSED`（新的渲染拿到同一個 browser
  行程）、`BROWSERS total= leaked=`（尾段後仍存活）、`SHUTDOWN-SETTLE`（宿主結束前的處置）。
- `-SuspendAt`：讀宿主記錄的 `RENDER-OK n=N … browser_pid=Some(P)`，核對 P 是本稿啟動的宿主的
  msedgewebview2 子行程後以 `NtSuspendProcess` 暫停它（`INJECT … suspended`；返回後約 0.18 秒就結束，來不及
  時記 `fail`）。修正後的宿主在下一次渲染前等 5 秒、核對後結束它（宿主記錄「已由宿主結束」）。
- 宿主結束後，命令列含隔離根目錄的 msedgewebview2 一律記 `SURVIVOR`（含各執行緒狀態）後由本稿結束
  （只限本稿建立的隔離資料夾）。
- 產出 `evidence/dw-bug-renderer-<Tag>-{driver,render,summary}.log`；summary 的 `SETTLE` 一行統計宿主記錄裡
  「下一次前等到結束」「宿主結束卡住者」「沒結束它」的次數。結束碼：0 無殘留；1 有殘留、60 秒內沒結束或宿主失敗；
  2 參數／環境錯誤；3 BLOCKED。

## explorer GDI 注入（dynamic-wallpaper task 4.9：安全閥的 self-test 注入）

`self-test-ipc` 建置的宿主（正式建置不含這條路徑，見 `src/wallpaper_coordinator/explorer_gdi_injection.rs`）
在環境變數 `FC_HOST_SELF_TEST_EXPLORER_GDI_FILE` 指向一個文字檔時，協調迴圈每次要讀 explorer GDI（即將
設定桌布之前）都重讀這個檔，以檔案內容取代或修改真正的讀值。宿主執行中改寫檔案即可改變下一次的讀值。

| 檔案內容（前後空白忽略） | 讀值 |
| --- | --- |
| 檔案不存在、空白、`real` | 真正的讀值（不注入） |
| `<gdi>`，例如 `1500` | 真正的 explorer PID＋指定的 GDI 數 |
| `<pid>:<gdi>`，例如 `999:300` | 指定的 PID 與 GDI 數（模擬 explorer 重新啟動） |
| `+<n>`／`-<n>`，例如 `+2001` | 真正的讀值加減 n（下限 0） |
| `fail` 或 `fail:<說明>` | 讀取失敗 |

內容不認得＝讀取失敗（說明含原文）。每次注入都在協調迴圈的記錄 target（`fc_host::wallpaper_coordinator`）
記一行「self-test 注入 explorer GDI」，接著是正式路徑的「explorer GDI：PID … 讀值 …；基準 …；門檻 …」，
觸發時是 warn 層級的「安全閥觸發：…→ 決策：停止接管…」。

```powershell
cd host; cargo build --release --features self-test-ipc; cd ..
$inject = Join-Path $env:TEMP 'fc-explorer-gdi.txt'
Set-Content -LiteralPath $inject -Value 'real' -NoNewline
$env:FC_HOST_SELF_TEST_EXPLORER_GDI_FILE = $inject
# 以隔離的 APPDATA／LOCALAPPDATA 啟動宿主並選一個主題（第一次設定時讀值成為基準），之後：
Set-Content -LiteralPath $inject -Value '+2001' -NoNewline   # 下一次設定前觸發「增量超過門檻」
Set-Content -LiteralPath $inject -Value '8001' -NoNewline    # 觸發「絕對值超過門檻」
Set-Content -LiteralPath $inject -Value '999:300' -NoNewline # PID 改變：以新讀值為基準、不觸發
Set-Content -LiteralPath $inject -Value 'fail' -NoNewline    # 讀取失敗：照常設定、只記一次 warn
# 驗完務必不帶 feature 重建，正式 exe 才不含注入路徑：
cd host; cargo build --release
```

基準記在狀態檔 `wallpaper-state.json` 的 `explorer_baseline`（`pid`、`gdi`、`recorded_at`）；接管開始、
PID 改變、使用者把主題從「不接管」改回某個主題時重設，回到未接管時清除。

## wallpaper-shots.mjs（dynamic-wallpaper task 3.1：桌布繪圖頁截圖與離線驗證）

以 headless Edge 載入 `host/ui/wallpapers/<頁面>`，等頁面的「畫完」訊號
（`window.__wallpaper.phase` 變成 `done`／`error`），**從頁面 canvas 匯出 PNG**（不截整個視窗：
headless viewport 比視窗小約 30px）。零安裝，只用 Node 22 內建模組；Edge 與 CDP 沿用
`host/tests/compare/cdp.mjs`，每次擷取都是新的 `--user-data-dir`。本機 HTTP 伺服器提供頁面
（`file://` 下 ES module 會被擋）：`/` 對應 `host/ui/`，`/test-fixtures/` 對應 repo 的 `tests/fixtures/` 與 `host/tests/wallpapers/fixtures/`。

```powershell
# 預設：_demo.html 五種尺寸（3840x2160、2560x1600、1920x1080、2560x1080、1080x1920）
node host/tools/wallpaper-shots.mjs

# 加做離線比對（每個尺寸再以「攔截所有非本機請求」畫一次，逐像素比對）
node host/tools/wallpaper-shots.mjs --offline-check

# 加做頁面契約驗證（預設 fixture、{data,config} 信封、未知 query 參數警告、fixture 讀不到的明確失敗、
# 以假的 window.__TAURI__ 驗宿主回報 wallpaper_render_done／wallpaper_render_failed）
node host/tools/wallpaper-shots.mjs --contract-checks

# 可調參數：--page 頁面檔名（預設 _demo.html）、--prefix 輸出檔名前綴（預設 dw-3.1-demo）、
# --out 輸出資料夾（預設 host/tools/evidence）、--sizes WxH 逗號分隔、--t ISO 時間
# （無時區＝--tz 當地時間）、--tz IANA 時區、--fixture 資料 JSON 的網址（none＝不帶，
# 頁面改讀 host/ui/fixtures/tw-events.json；預設 /test-fixtures/tw_events_sample.json）
node host/tools/wallpaper-shots.mjs --page _demo.html --sizes 1920x1080,1080x1920 --out host/tools/evidence

# 縮放比例（task 3.3）：--dsf n 以 CDP Emulation.setDeviceMetricsOverride 把 devicePixelRatio 設為 n
# （viewport＝w/n × h/n，模擬同一台螢幕在 n 倍縮放）；--dsf-compare n 每個尺寸再以 dsf n 畫一次，
# 斷言頁面真的看到該 devicePixelRatio 且兩張逐像素相同（spec「縮放比例不影響構圖」）
node host/tools/wallpaper-shots.mjs --page astrolabe.html --prefix dw-3.3-dsf --sizes 3840x2160 --t 2026-10-05T21:30 --dsf 1 --dsf-compare 1.5

# 反例（task 3.3）：測試頁故意畫一段與「接下來」標題重疊、一段跨出右緣的文字；
# --expect-layout-fail 要求版面檢查同時抓到「相交」與「超出畫面」、且沒有其他問題才算通過。
# 不帶 --expect-layout-fail 跑同一頁，結束碼必須是 1。
node host/tools/wallpaper-shots.mjs --page ../test-fixtures/astrolabe-layout-negative.html --prefix dw-3.3-negative --sizes 1920x1080,1080x1920 --t 2026-10-06T21:30 --expect-layout-fail
```

輸出 `<前綴>-<寬>x<高>.png` 與 `<前綴>-shots.log`（完整輸出，含各張 SHA-256；頁面設了
`window.__wallpaperSummary` 時一併記錄，例：星盤各市場狀態與「接下來」）；`--dsf-compare` 另存
`<前綴>-<寬>x<高>-dsf<n>.png`；`--offline-check` 另寫 `<前綴>-offline-check.log`，`--contract-checks` 另寫
`<前綴>-contract-check.log`。`--page` 可用 `../test-fixtures/<檔>` 載入 `host/tests/wallpapers/fixtures/` 裡的測試頁。
稿內斷言（任一失敗結束碼為 1，並列出原因）：

- PNG 的 IHDR 寬高精確等於要求尺寸；頁面回報尺寸與 query 一致、query 無警告。
- 畫完狀態為 `done`、頁面 `warnings` 為空（未知 query 參數、主題自己回報的設定問題都算警告）；頁面**真的收到** fixture 資料：
  `dataStatus` 為 `ok`，且頁面回報的資料鍵與 fixture 檔實際的鍵完全一致（不只是「有畫出東西」）。
- 頁面要求的每個字型（`runWallpaper({ fonts })`，可以是 `FONT_REQUIREMENTS` 的子集，但不能有清單外的 id）
  `document.fonts.check()` 為真且頁面回報 loaded；字型資源（Resource Timing）全部與頁面同源。
- 頁面回報的文字邊界框（`env.boxes`）全在畫面內且兩兩不相交（`checkLayout`）。
- 整段過程沒有任何對外請求。
- 離線：對外 `fetch` 必須失敗（證明攔截生效）、頁面被攔截的請求數為 0、離線與連線的像素
  SHA-256 與 PNG 位元組完全相同。
- `--dsf`／`--dsf-compare`：頁面的 `devicePixelRatio` 等於要求值，兩次不同，且像素 SHA-256 相同。

純邏輯單元測試：`node --test "host/tests/wallpapers/*.test.mjs"`（含星盤的時段展開
`astrolabe-sessions.test.mjs`）。

### 星盤（task 3.3）的截圖情境

`astrolabe.html` 的時段表與休市表來自 `{data, config}` 信封的 `config`，缺鍵由內建預設檔補齊；
裸 tw_events fixture（預設）＝全用內建預設。證據檔 `host/tools/evidence/dw-3.3-*`：

```powershell
node host/tools/wallpaper-shots.mjs --page astrolabe.html --prefix dw-3.3-trading      --t 2026-10-06T10:00 --offline-check
node host/tools/wallpaper-shots.mjs --page astrolabe.html --prefix dw-3.3-us-open      --t 2026-10-05T21:30
node host/tools/wallpaper-shots.mjs --page astrolabe.html --prefix dw-3.3-thanksgiving --t 2026-11-26T21:30
node host/tools/wallpaper-shots.mjs --page astrolabe.html --prefix dw-3.3-halfday      --t 2026-12-24T19:00
node host/tools/wallpaper-shots.mjs --page astrolabe.html --prefix dw-3.3-overnight    --t 2026-12-07T10:00
node host/tools/wallpaper-shots.mjs --page astrolabe.html --prefix dw-3.3-long-holiday --t 2026-10-06T21:30 --fixture /test-fixtures/astrolabe-long-holiday.json
```

### 撕日曆（task 3.4）的截圖情境

`tearoff.html` 的「宜」「忌」規則在 `lib/tearoff-model.mjs`（單元測試 `host/tests/wallpapers/tearoff-model.test.mjs`），
詞庫、門檻、旺季表、休市表來自 `{data, config}` 信封的 `config`，缺鍵由內建預設檔補齊。情境 fixture 在
`host/tests/wallpapers/fixtures/tearoff-*.json`（詞庫一律從預設檔複製）。證據檔 `host/tools/evidence/dw-3.4-*`：

```powershell
node host/tools/wallpaper-shots.mjs --page tearoff.html --prefix dw-3.4-trading     --t 2026-11-03T08:00 --offline-check
node host/tools/wallpaper-shots.mjs --page tearoff.html --prefix dw-3.4-saturday    --t 2026-10-03T08:00
node host/tools/wallpaper-shots.mjs --page tearoff.html --prefix dw-3.4-tsmc-july   --t 2026-07-16T08:00 --fixture /test-fixtures/tearoff-tsmc-july.json
node host/tools/wallpaper-shots.mjs --page tearoff.html --prefix dw-3.4-short-ratio --t 2026-11-03T08:00 --fixture /test-fixtures/tearoff-short-ratio.json
node host/tools/wallpaper-shots.mjs --page tearoff.html --prefix dw-3.4-long-phrase --t 2029-03-13T08:00 --fixture /test-fixtures/tearoff-long-phrase.json
node host/tools/wallpaper-shots.mjs --page tearoff.html --prefix dw-3.4-dsf --sizes 3840x2160 --t 2026-11-03T08:00 --dsf 1 --dsf-compare 1.5
node host/tools/wallpaper-shots.mjs --page ../test-fixtures/tearoff-layout-negative.html --prefix dw-3.4-negative --sizes 1920x1080,1080x1920 --t 2026-11-03T08:00 --expect-layout-fail
# 可讀下限（41 字的詞庫句）：該行照縮到放進紙頁（沒有硬下限），並記「低於可讀下限」警告
# → 結束碼 1，失敗原因只有那則警告（版面檢查通過；摘要不會出現「超出紙頁」）
node host/tools/wallpaper-shots.mjs --page tearoff.html --prefix dw-3.4-below-readable --sizes 1920x1080,1080x1920 --t 2026-11-03T08:00 --fixture /test-fixtures/tearoff-below-readable.json
# headless Edge 的干支年（不符就 phase error、結束碼 1；Node 端見單元測試）
node host/tools/wallpaper-shots.mjs --page ../test-fixtures/tearoff-ganzhi-edge.html --prefix dw-3.4-ganzhi-edge --sizes 1920x1080 --t 2026-02-17T08:00
```

`--page` 會先正規化（反斜線改斜線、去掉 `./`、摺疊 `a/../`）再查 `PAGE_REQUIRED_FONTS` 與組網址，
`./tearoff.html` 與 `tearoff.html` 檢查同一份必要字型；log 第三行印出實際套用的必要字型清單（未列出的頁面只檢查頁面自己要求的字型）。
`--contract-checks` 的「字型載入失敗」項擋的是該頁第一個必要字型的檔案（撕日曆＝`NotoSerifTC-VF.ttf`）。

### 脊線與等高線（task 3.5）的截圖情境

`ridgeline.html`、`contour.html` 以 `data.twii_intraday`（最後一個完整交易日的盤中走勢）為主形狀。
盤中走勢的純函式在 `lib/intraday.mjs`，交易日與「資料落後幾個交易日」在共用模組 `lib/tw-trading-days.mjs`（3.6 天際線也用）；
單元測試是 `host/tests/wallpapers/{intraday,tw-trading-days}.test.mjs`。情境 fixture 在 `host/tests/wallpapers/fixtures/intraday-*.json`。
證據檔 `host/tools/evidence/dw-3.5-*`（`<頁>` 換成 `ridgeline` 或 `contour`）：

```powershell
node host/tools/wallpaper-shots.mjs --page <頁>.html --prefix dw-3.5-<頁>-fresh  --t 2026-10-02T12:00 --offline-check   # 落後 0
node host/tools/wallpaper-shots.mjs --page <頁>.html --prefix dw-3.5-<頁>-stale3 --t 2026-10-06T13:30                   # 落後 3：右下角「資料停在 10/1」
node host/tools/wallpaper-shots.mjs --page <頁>.html --prefix dw-3.5-<頁>-stale2 --t 2026-10-06T13:29                   # 落後 2：不顯示（與 fresh 逐位元組相同）
node host/tools/wallpaper-shots.mjs --page <頁>.html --prefix dw-3.5-<頁>-flat   --t 2026-10-02T12:00 --fixture /test-fixtures/intraday-flat.json
node host/tools/wallpaper-shots.mjs --page <頁>.html --prefix dw-3.5-<頁>-monday --t 2026-09-28T06:00 --fixture /test-fixtures/intraday-monday-0924.json
node host/tools/wallpaper-shots.mjs --page <頁>.html --prefix dw-3.5-<頁>-dsf --sizes 3840x2160 --t 2026-10-02T12:00 --dsf 1 --dsf-compare 1.5
node host/tools/wallpaper-shots.mjs --page ../test-fixtures/<頁>-layout-negative.html --prefix dw-3.5-<頁>-negative --sizes 1920x1080,1080x1920 --t 2026-10-06T13:30 --expect-layout-fail
# 缺資料：phase error（不畫假資料）→ 結束碼 1
node host/tools/wallpaper-shots.mjs --page <頁>.html --prefix dw-3.5-<頁>-missing --sizes 1920x1080 --t 2026-10-02T12:00 --fixture /test-fixtures/intraday-missing.json
# 契約檢查：預設 fixture（host/ui/fixtures/tw-events.json）沒有 twii_intraday，A、B 兩項改用頁面適用的 fixture
node host/tools/wallpaper-shots.mjs --page <頁>.html --prefix dw-3.5-<頁>-contract --sizes 1920x1080 --t 2026-10-02T12:00 --contract-checks --contract-fixture /test-fixtures/tw_events_sample.json --contract-envelope /test-fixtures/intraday-envelope.json
```

在 Git Bash 執行時請加 `MSYS_NO_PATHCONV=1`，否則 `/test-fixtures/...` 會被改寫成 Windows 路徑。

### 天際線（task 3.6）的截圖情境

`skyline.html` 以 `data.twii_daily`（日 K，由舊到新）畫最後 20 根 K 線建築與 20MA 電線。
純函式在 `lib/skyline-model.mjs`（讀日 K、20MA、取窗、正規化、K 線幾何、seed、標示文字），過期判定沿用 `lib/tw-trading-days.mjs`；
單元測試是 `host/tests/wallpapers/skyline-{model,draw}.test.mjs`。情境 fixture 在 `host/tests/wallpapers/fixtures/skyline-*.json`。
頁面另把「資料帶」（K 線、影線、20MA 電線與節點的範圍）以 label `data-band` 登記進邊界框，版面檢查因此也斷言任何文字都不壓在資料上。
證據檔 `host/tools/evidence/dw-3.6-*`：

```powershell
node host/tools/wallpaper-shots.mjs --page skyline.html --prefix dw-3.6-fresh  --t 2026-10-02T12:00 --offline-check   # 落後 0
node host/tools/wallpaper-shots.mjs --page skyline.html --prefix dw-3.6-stale3 --t 2026-10-06T13:30                   # 落後 3：右下角「資料停在 10/1」
node host/tools/wallpaper-shots.mjs --page skyline.html --prefix dw-3.6-stale2 --t 2026-10-06T13:29                   # 落後 2：不顯示（與 fresh 逐位元組相同）
node host/tools/wallpaper-shots.mjs --page skyline.html --prefix dw-3.6-rally  --t 2026-10-02T12:00 --fixture /test-fixtures/skyline-rally.json  # 大漲：MA 在 K 下方
node host/tools/wallpaper-shots.mjs --page skyline.html --prefix dw-3.6-crash  --t 2026-10-02T12:00 --fixture /test-fixtures/skyline-crash.json  # 大跌：MA 在 K 上方
node host/tools/wallpaper-shots.mjs --page skyline.html --prefix dw-3.6-rows39 --t 2026-10-02T12:00 --fixture /test-fixtures/skyline-39.json     # 剛好 39 筆（與 fresh 逐位元組相同）
node host/tools/wallpaper-shots.mjs --page skyline.html --prefix dw-3.6-dsf --sizes 3840x2160 --t 2026-10-02T12:00 --dsf 1 --dsf-compare 1.5
node host/tools/wallpaper-shots.mjs --page skyline.html --prefix dw-3.6-rally-stale3 --t 2026-10-06T13:30 --fixture /test-fixtures/skyline-rally.json  # 價格極端＋過期標示
node host/tools/wallpaper-shots.mjs --page skyline.html --prefix dw-3.6-crash-stale3 --t 2026-10-06T13:30 --fixture /test-fixtures/skyline-crash.json
node host/tools/wallpaper-shots.mjs --page ../test-fixtures/skyline-layout-negative.html --prefix dw-3.6-negative --sizes 1920x1080,1080x1920 --t 2026-10-06T13:30 --expect-layout-fail
# 反例：過期標示放回修正前的 0.925H，直式必須回報與 data-band 相交
node host/tools/wallpaper-shots.mjs --page ../test-fixtures/skyline-stale-oldpos-negative.html --prefix dw-3.6-stale-oldpos-negative --sizes 1080x1920 --t 2026-10-06T13:30 --fixture /test-fixtures/skyline-rally.json --expect-layout-fail
# 不到 39 筆：K 照畫、缺完整 20MA 的根不畫 MA，頁面警告 → 結束碼 1
node host/tools/wallpaper-shots.mjs --page skyline.html --prefix dw-3.6-rows30 --sizes 1920x1080 --t 2026-10-02T12:00 --fixture /test-fixtures/skyline-30.json
# 缺資料（缺鍵、空陣列、不到 20 筆）：phase error（不畫假資料）→ 結束碼 1
node host/tools/wallpaper-shots.mjs --page skyline.html --prefix dw-3.6-missing --sizes 1920x1080 --t 2026-10-02T12:00 --fixture /test-fixtures/skyline-missing.json
# 契約檢查：預設 fixture 沒有 twii_daily，A、B 兩項改用頁面適用的 fixture
node host/tools/wallpaper-shots.mjs --page skyline.html --prefix dw-3.6-contract --sizes 1920x1080 --t 2026-10-02T12:00 --contract-checks --contract-fixture /test-fixtures/tw_events_sample.json --contract-envelope /test-fixtures/skyline-envelope.json
```

## make-icons.mjs（dynamic-wallpaper task 5.1：由 SVG 產生 .ico）

由 `host/icons/src/*.svg` 產生 `host/icons/icon.ico`（統一圖示，`build.rs` 的 `window_icon_path`
指向它；`tauri.conf.json` 的 `trayIcon.iconPath` 自 task 5.2 修正輪 1 起指向衍生的 `icons/png/icon-16.png`）與 `host/icons/theme-<id>.ico`（astrolabe、tearoff、
ridgeline、contour、skyline 五套主題）。**產物進 git，cargo 建置不依賴本指令稿**；只有改了 SVG
或尺寸表才需要重跑。

```powershell
node host/tools/make-icons.mjs            # 產生 6 個 .ico ＋ 放大對照圖，最後自動驗證（任何缺漏→非零結束碼）
node host/tools/make-icons.mjs --verify   # 只解析既有 .ico 檔頭並驗證（不需要 Edge）
node host/tools/make-icons.mjs --derive   # 只由既有 .ico 重寫衍生檔（不需要 Edge），再驗證
node --test host/tests/ico.test.mjs       # 封裝／解析／驗證的單元測試（含檢查進 git 的 6 個 .ico 與衍生檔）
```

- **衍生檔**（task 5.2，規則在 `lib/icon-sources.mjs` 的 `planDerived`）：`host/icons/png/<name>-<size>.png`
  （6 組 × 16–64 七個尺寸，與 `.ico` 的 entry 逐位元組相同，`host/src/app_icon.rs` 以
  `tauri::include_image!` 內嵌，供系統匣與設定視窗隨主題換圖示）、`docs/favicon.svg`（`c-sunrise-page.svg`
  注入 64×64）、`docs/favicon.png`（`icon.ico` 的 64px）。產生模式寫完 `.ico` 會一併重寫；`--verify`
  逐位元組核對，`.ico` 改了卻沒重跑 `--derive` 會失敗。增減內嵌尺寸要同時改 `RUNTIME_PNG_SIZES` 與
  `app_icon.rs` 的 `EMBEDDED_SIZES` 和逐張清單（Rust 測試核對兩邊一致、`icons/png` 不多不少）。

- **需求**：Node 22（零安裝、不用 npm）與 Edge。Edge 路徑沿用 `host/tests/compare/cdp.mjs` 的偵測
  （`Program Files (x86)`、`Program Files`），裝在別處用環境變數 `FC_COMPARE_EDGE_PATH` 指定。每次執行
  開一個獨立 `--user-data-dir` 的 headless Edge，結束時只關自己啟動的那個行程並刪暫存資料夾。
- **做法**：不用 `--screenshot`（headless Edge 的 viewport 比視窗小約 30px、寬度下限約 492px，量不準
  精確像素），而是在頁面內以 `<canvas>` 用精確尺寸繪製 SVG 後匯出 PNG（SVG 根元素先注入
  width／height＝目標尺寸，向量直接點陣化、保留 alpha）。每個尺寸（16、20、24、32、40、48、64、256）
  各點陣化一次，封成 PNG 內嵌式 ICO（256 的寬高欄位寫 0）。
- **來源 SVG 選擇**（規則在 `lib/icon-sources.mjs`、有單元測試）：統一圖示 `>=32` 用 `c-sunrise-page.svg`、16／20 用 `c-16.svg`、24 用 `c-24.svg`。
  主題圖示 `>=32` 用 `theme-<id>.svg`；`<=24` 優先用簡化版 `theme-<id>-16.svg`（16／20／24 共用，
  24 若另有 `theme-<id>-24.svg` 則優先），沒有簡化版就用原圖。目前 ridgeline、astrolabe、contour
  有簡化版（原圖的細線／細環在 16px 糊掉），tearoff、skyline 原圖在 16px 已可辨識。
- **驗證**：每個 `.ico` 必須含 16、20、24、32、40、48、64、256 全部尺寸、每個 entry 的內嵌 PNG
  是 RGBA 8-bit 且 IHDR 寬高與 entry 一致，否則非零結束碼。
- **16px 辨識檢查**：產生時另輸出 `host/tools/evidence/icons-5.1-zoom-16-20-24.png`（六個圖示 16／20／24px
  最近鄰放大 8 倍，深色底與淺色底並排）與各圖示單張 `icons-5.1-<name>.png`；改 SVG 後要看一眼。
- **何時重跑**：改 `host/icons/src/` 任何 SVG、增減尺寸（`lib/ico.mjs` 的 `REQUIRED_SIZES`）、新增主題
  （同步加進 `lib/icon-sources.mjs` 的 `THEMES`）之後；重跑後連同 `.ico` 與衍生檔一起 commit。

## verify-dw-5.2-exe-icon.ps1／verify-dw-5.2-favicon.mjs（dynamic-wallpaper task 5.2）

```powershell
pwsh -NoProfile -File host/tools/verify-dw-5.2-exe-icon.ps1   # 預設讀 host/target/release/fc-host.exe
node host/tools/verify-dw-5.2-favicon.mjs
```

- `verify-dw-5.2-exe-icon.ps1`：唯讀載入 exe（`LoadLibraryEx` 資料檔模式，不執行），取出 `RT_GROUP_ICON`
  與各 `RT_ICON`，逐尺寸與 `host/icons/icon.ico` 的 entry 逐位元組比對，並以 `LookupIconIdFromDirectoryEx`
  確認系統要畫 16–256 各尺寸時挑到的是同尺寸那張。記錄檔 `evidence/dw-5.2-exe-icon.log`（去識別）。
  先 `cargo clean --release -p fc-host && cargo build --release` 再跑。
- `verify-dw-5.2-favicon.mjs`：headless Edge（獨立 `--user-data-dir`）開 `docs/index.html`，每條 favicon
  連結都要載入成功且為 64×64。記錄檔 `evidence/dw-5.2-favicon.log`。

## installer-snapshot.ps1（installer-auto-update task 4.1：實機安裝測試前後的快照、還原、比對）

```powershell
pwsh -NoProfile -File host/tools/installer-snapshot.ps1 Save    -Dir <快照目錄>            # 測試前
pwsh -NoProfile -File host/tools/installer-snapshot.ps1 Restore -Dir <快照目錄> -WhatIf    # 只列動作
pwsh -NoProfile -File host/tools/installer-snapshot.ps1 Restore -Dir <快照目錄>            # 測試後；結束自動 Compare
pwsh -NoProfile -File host/tools/installer-snapshot.ps1 Compare -Dir <快照目錄>            # 只比對
pwsh -NoProfile -File host/tools/tests/InstallerSnapshot.Tests.ps1                         # 自測（只碰暫存目錄與假登錄樹）
```

- **範圍**（路徑與登錄鍵全部寫死在 `lib/InstallerSnapshot.psm1` 的 `New-FcRealTargets`，命令列只收快照目錄）：
  目錄 `%LOCALAPPDATA%\fc-host`、`%APPDATA%\tw.fintools.fc-host`、`%LOCALAPPDATA%\tw.fintools.fc-host`
  （整個複製；不存在就記「不存在」，還原時確保不存在）；捷徑 `fc-host.lnk`／`財經日曆.lnk`（開始功能表）與
  桌面的 `fc-host.lnk`／`財經日曆.lnk`；HKCU 的 `Run`、`StartupApproved\Run`（值 `fc-host`）、`Uninstall\fc-host` 與
  `Software\fintools\fc-host` 整棵子樹、`Control Panel\Desktop` 的 `Wallpaper`／`WallpaperStyle`／`TileWallpaper`；
  各螢幕桌布讀回（`IDesktopWallpaper`，只讀）。各鍵名在 `installer.nsi` 的出處見腳本檔頭說明。
- **路徑取自 known folder**（LocalApplicationData、ApplicationData、Programs、Desktop），不看環境變數；
  `%LOCALAPPDATA%`／`%APPDATA%` 與 known folder 不一致（例如在覆寫過環境變數的殼裡跑）就拒絕，因為安裝檔與宿主
  用的是 known folder，快照會落在錯的位置。
- **僅回報項**：`%APPDATA%\Microsoft\Windows\Themes\` 下的 `TranscodedWallpaper`、`Transcoded_*`、
  `TranscodedWallpaperCache\*` 只做 Save（記清單）與 Compare，**絕不還原**（那是 explorer 的工作檔）；差異以
  `~` 列在「僅回報」，不計入差異、不影響結束碼。
- **登錄值保真**：單值「不存在」與「空字串」分開記；型別與原始資料逐值記錄（不展開環境變數，DWord／QWord／
  Binary／MultiString 都還原到位元組）；比對區分大小寫（Binary 以 base64 存）；JSON 讀回不把 ISO 日期樣式的
  字串轉成 DateTime。
- **Save 自我驗證**：Save 結束時自動對現況 Compare，不是零差異就把快照標成不完整（刪掉 `snapshot.json`、結束碼 1，
  不可用於還原）。
- **桌布只比對、不設定**：Restore 後桌布若與快照不同，只在差異清單回報；要設回桌布請用宿主的
  `--restore-wallpaper`。桌布讀回失敗時摘要會寫「桌布未驗證」。
- **安全**：Save／Restore 前有 `fc-host`、宿主的 `msedgewebview2`、安裝檔、安裝目錄內的 `uninstall.exe` 在跑就停下
  （結束碼 3），不會結束任何行程；Save 不覆寫既有快照、受管目錄內有 junction／symlink 就拒絕；快照目錄不得放在會被
  還原刪除的目錄內；快照記錄的路徑必須與腳本寫死的一致；Restore 在任何刪除之前先確認快照內容完整（每個「存在」的
  項目都有對應內容，缺一項整個拒絕、結束碼 3）；`-WhatIf` 不改任何東西。
- **結束碼**：0＝成功且零差異；1＝有差異、還原動作失敗或 Save 自我驗證失敗；3＝拒絕（未動任何東西）；4＝非預期錯誤。
