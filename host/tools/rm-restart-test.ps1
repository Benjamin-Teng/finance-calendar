<#
.SYNOPSIS
    Task 5.7 驗收驅動腳本：`RegisterApplicationRestart`（啟動時註冊、帶 `--restarted`）與系統匣
    「結束」時 `UnregisterApplicationRestart`（design.md D12「重新啟動註冊」；
    specs/widget-host-lifecycle「Restart Manager 關閉後重新啟動」）。自製 Restart Manager 測試
    腳本：`RmStartSession`→`RmRegisterResources`（宿主 exe 檔名）→`RmShutdown`→`RmRestart`→
    `RmEndSession`。

.DESCRIPTION
    只呼叫行程層級的 Win32 API（`rstrtmgr.dll` 五個函式＋視窗列舉），**不注入任何輸入**
    （不送按鍵、不點擊、不截圖），鎖定時也可跑。`RmShutdown` 只對「本腳本自己啟動、以暫存
    `%APPDATA%`／`%LOCALAPPDATA%` 隔離的測試宿主」做，**絕不把 `explorer.exe` 或任何其他程式
    註冊進 RM session**（`RmRegisterResources` 全程只傳一個檔名＝本次測試用的 `fc-host.exe`）。

    fix F2（review 5.7 medium×2＋low）重寫判定：

    **情境 A（有註冊 → RM 關閉後會重新啟動，並依保存的版面顯示）**：
      1. 暫存 `%APPDATA%`／`%LOCALAPPDATA%`＋fixture 資料啟動宿主，等四個財經小工具就緒。
      2. 等行程存活滿 65 秒（Microsoft Learn〈RegisterApplicationRestart〉：系統只重啟至少已
         執行 60 秒的應用程式，防循環重啟）。
      3. 進入「真實使用者資料保護」（見下）：RmRestart 重啟出的新行程**不繼承**本腳本設定的
         環境變數、會讀寫真正的 `%APPDATA%`／`%LOCALAPPDATA%`（專案 memory
         `restart-manager-rmrestart-ignores-caller-env.md`）。把真正的 settings.json／
         gatekeeper.log／data\／logs\ 改名搬開，並在真正路徑放一份**測試專用版面檔**：由
         步驟 1 的暫存 settings.json 複製，關閉 `fixed`、`data_dir` 指向暫存 fixture 目錄、
         `data_fetch` 寫 `"off"`（新行程拿真實環境，`auto` 會判成非隔離而抓取並覆寫樣本）。
      4. `RmShutdown(RmForceShutdown)` → 舊行程結束（A3）→ `RmRestart` → 20 秒內出現命令列
         含 `--restarted` 的新行程（A4）。
      5. A5（Scenario THEN「宿主恢復執行並依保存的版面顯示小工具」）：新行程 30 秒內顯示
         clock 與 macro、**沒有** fixed（測試版面關閉了它；預設版面會顯示 fixed，故有鑑別力），
         且再存活 5 秒仍在（不是啟動後即崩潰）。
      6. 收尾：結束所有 fc-host、逐項還原真實資料（見下）。

    **情境 B（系統匣結束路徑取消註冊 → RM 關閉後不會重新啟動）**：
      1. 另一份暫存環境＋CDP 埠啟動宿主，等就緒、存活滿 65 秒（與 A 同條件）。
      2. 以 CDP invoke 驗收用指令 `self_test_unregister_restart`（僅 `--features self-test-ipc`
         建置才有）：呼叫與系統匣「結束」同一個 `desktop::unregister_restart`，但**不**結束行程
         （B2；統一記錄檔須出現「已取消重新啟動註冊」）。
      3. 同樣進入真實資料保護，並放入 `data_fetch: "off"` 的測試版面（萬一被錯誤重啟，新行程
         也碰不到使用者資料、不會抓取）。
      4. `RmShutdown` → 舊行程**被 RM 關閉**（B3：證明這次 RmShutdown 真的關到它，B4 才有意義）
         → `RmRestart` → 20 秒內**沒有**新的 fc-host 行程（B4）。
      5. B4 只有在對照組 A4 成立時才算 PASS——同一套時序與 RM 呼叫在「有註冊」時會重啟，
         「取消註冊」時不重啟，才證明 `UnregisterApplicationRestart` 確實有作用。舊版 B 在
         RmShutdown 前就以 `self_test_quit` 讓行程自行結束、且只活 3 秒，拿掉取消註冊也照樣
         PASS（review 5.7 medium）。

    **真實使用者資料保護與還原（review 5.7 low）**：只搬四個真的受 `%APPDATA%`／
    `%LOCALAPPDATA%` 影響的葉節點（`EBWebView` 本來就不吃覆寫、且會被瀏覽器持有控制代碼，整個
    父目錄搬移會 Access denied，見 memory）。還原逐項 try/catch：清除風險窗口內新生成的東西時
    重試數次，清不掉就**不**改名（保留備份、記錄路徑），任何一項失敗都不中斷其餘項目；最後在
    摘要列出所有未還原項目（含備份路徑，供手動還原），並判 FAIL。開機自啟登錄（暫存 `%APPDATA%`
    ＝首次啟動，宿主會寫 HKCU Run\fc-host）以 lib\AutostartRegistry.psm1 在腳本開始快照、結束
    還原。

.PARAMETER Exe
    fc-host.exe 路徑，預設 host/target/release/fc-host.exe。情境 B 需要驗收用指令
    `self_test_unregister_restart`，須以 `cargo build --release --features self-test-ipc` 建置
    （改過 host/ui/ 時先 `cargo clean --release -p fc-host`）。

.PARAMETER OutDir
    證據輸出目錄，預設 host/tools/evidence。

.PARAMETER CdpPort
    情境 B 專用的 WebView2 remote debugging 埠，預設 9357（未被其他驗收腳本使用）。
#>
[CmdletBinding()]
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\fc-host.exe'),
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [int]$CdpPort = 9357
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\AutostartRegistry.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\ProcessTree.psm1') -Force

Add-Type -Namespace V57 -Name Native -MemberDefinition @'
[DllImport("user32.dll")] public static extern System.IntPtr GetTopWindow(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern System.IntPtr GetWindow(System.IntPtr hWnd, uint cmd);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr hWnd, out uint pid);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(System.IntPtr hWnd, System.Text.StringBuilder s, int n);

// Restart Manager（rstrtmgr.dll）：只用檔名式註冊（nApplications=nServices=0），不需要
// RM_UNIQUE_PROCESS／FILETIME 結構。查證於 Microsoft Learn：
//   RmStartSession        https://learn.microsoft.com/windows/win32/api/restartmanager/nf-restartmanager-rmstartsession
//   RmRegisterResources   https://learn.microsoft.com/windows/win32/api/restartmanager/nf-restartmanager-rmregisterresources
//   RmShutdown            https://learn.microsoft.com/windows/win32/api/restartmanager/nf-restartmanager-rmshutdown
//   RmRestart             https://learn.microsoft.com/windows/win32/api/restartmanager/nf-restartmanager-rmrestart
// strSessionKey 緩衝區大小 CCH_RM_SESSION_KEY+1＝33（CCH_RM_SESSION_KEY＝sizeof(GUID)*2＝32，
// 查證於 windows-rs 產生的常數 windows::Win32::System::RestartManager::CCH_RM_SESSION_KEY＝32；
// RestartManager.h 本身的頁面未列出數值，故以 windows-rs 的產生結果佐證）。
[DllImport("rstrtmgr.dll", CharSet = CharSet.Unicode)] public static extern int RmStartSession(out uint pSessionHandle, int dwSessionFlags, System.Text.StringBuilder strSessionKey);
[DllImport("rstrtmgr.dll")] public static extern int RmEndSession(uint pSessionHandle);
[DllImport("rstrtmgr.dll", CharSet = CharSet.Unicode)] public static extern int RmRegisterResources(uint pSessionHandle, uint nFiles, string[] rgsFilenames, uint nApplications, System.IntPtr rgApplications, uint nServices, string[] rgsServiceNames);
[DllImport("rstrtmgr.dll")] public static extern int RmShutdown(uint dwSessionHandle, uint lActionFlags, System.IntPtr fnStatus);
[DllImport("rstrtmgr.dll")] public static extern int RmRestart(uint dwSessionHandle, uint dwRestartFlags, System.IntPtr fnStatus);
'@

# RmForceShutdown＝0x1（Microsoft Learn〈RM_SHUTDOWN_TYPE〉）。
$RmForceShutdown = 0x1
# Microsoft Learn：至少執行 60 秒系統才會允許重啟，抓 65 秒留緩衝。A、B 兩情境同一門檻。
$MinUptimeSec = 65

function Get-Ts { Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fffK' }
function Test-Locked { [bool](Get-Process -Name LogonUI, LockApp -ErrorAction SilentlyContinue) }

$Exe = (Resolve-Path $Exe).Path
$Node = (Get-Command node).Source
$CdpEval = Join-Path $PSScriptRoot 'host-cdp-eval.mjs'
$FixtureEvents = (Resolve-Path (Join-Path $PSScriptRoot '..\ui\fixtures\tw-events.json')).Path
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$logPath = Join-Path $OutDir '5.7-driver.log'
$sumPath = Join-Path $OutDir '5.7-summary.log'
$log = New-EvidenceWriter $logPath
$log.AutoFlush = $true
function W([string]$msg) { $log.WriteLine("$(Get-Ts) $msg") }
W "# rm-restart-test.ps1 exe=$Exe cdpPort=$CdpPort locked=$([int](Test-Locked))"

function Get-HostWindowTitles([int]$ProcId) {
    $titles = @()
    $h = [V57.Native]::GetTopWindow([IntPtr]::Zero)
    while ($h -ne [IntPtr]::Zero) {
        $p = 0
        [void][V57.Native]::GetWindowThreadProcessId($h, [ref]$p)
        if ($p -eq $ProcId -and [V57.Native]::IsWindowVisible($h)) {
            $sb = New-Object System.Text.StringBuilder 256
            [void][V57.Native]::GetWindowText($h, $sb, 256)
            if ($sb.Length -gt 0) { $titles += $sb.ToString() }
        }
        $h = [V57.Native]::GetWindow($h, 2)
    }
    return $titles
}

function Wait-WindowTitles([int]$ProcId, [scriptblock]$Cond, [int]$TimeoutSec = 30) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt $TimeoutSec) {
        $titles = Get-HostWindowTitles $ProcId
        if (& $Cond $titles) { return $true }
        Start-Sleep -Milliseconds 300
    }
    return $false
}

function Wait-FinanceWidgetsReady([int]$ProcId, [int]$TimeoutSec = 30) {
    $readyIds = @('fc-host clock', 'fc-host macro', 'fc-host fixed', 'fc-host dynamic')
    Wait-WindowTitles $ProcId { param($t) ($readyIds | Where-Object { $_ -notin $t }).Count -eq 0 } $TimeoutSec
}

function Wait-Uptime([System.Diagnostics.Process]$Proc, [string]$Tag) {
    $uptime = ((Get-Date) - $Proc.StartTime).TotalSeconds
    if ($uptime -lt $MinUptimeSec) {
        $waitSec = [math]::Ceiling($MinUptimeSec - $uptime)
        W "## $Tag 等待行程存活滿 $MinUptimeSec 秒（Microsoft Learn 循環重啟保護），再等 ${waitSec}s"
        Start-Sleep -Seconds $waitSec
    }
    W "## $Tag 行程已存活 $([math]::Round(((Get-Date) - $Proc.StartTime).TotalSeconds, 1))s"
}

# ── Restart Manager 包裝（每次呼叫都記錄 HRESULT/錯誤碼到 driver.log） ──────────────────────
function New-RmSession {
    $handle = 0
    $key = New-Object System.Text.StringBuilder 64
    $hr = [V57.Native]::RmStartSession([ref]$handle, 0, $key)
    W "RM RmStartSession hr=$hr handle=$handle key=$($key.ToString())"
    if ($hr -ne 0) { throw "RmStartSession 失敗，錯誤碼 $hr" }
    return [uint32]$handle
}
function Register-RmExeFile([uint32]$Handle, [string]$ExePath) {
    $hr = [V57.Native]::RmRegisterResources($Handle, 1, [string[]]@($ExePath), 0, [IntPtr]::Zero, 0, $null)
    W "RM RmRegisterResources file=$ExePath hr=$hr"
    return $hr
}
function Invoke-RmShutdown([uint32]$Handle) {
    $hr = [V57.Native]::RmShutdown($Handle, $RmForceShutdown, [IntPtr]::Zero)
    W "RM RmShutdown(RmForceShutdown) hr=$hr"
    return $hr
}
function Invoke-RmRestart([uint32]$Handle) {
    $hr = [V57.Native]::RmRestart($Handle, 0, [IntPtr]::Zero)
    W "RM RmRestart hr=$hr"
    return $hr
}
function Close-RmSession([uint32]$Handle) {
    $hr = [V57.Native]::RmEndSession($Handle)
    W "RM RmEndSession hr=$hr"
    return $hr
}

function Get-FcHostProcesses { @(Get-CimInstance Win32_Process -Filter "Name='fc-host.exe'" -ErrorAction SilentlyContinue) }

function Wait-ProcessGone([int]$ProcId, [int]$TimeoutSec = 40) {
    # RmForceShutdown：GUI 應用程式逾時 30 秒內強制關閉，抓 40 秒。
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt $TimeoutSec) {
        if (-not (Get-Process -Id $ProcId -ErrorAction SilentlyContinue)) { return $true }
        Start-Sleep -Milliseconds 500
    }
    return $false
}

# 本腳本負責收尾的宿主：自己 Start-Process 啟動的，加上 RmRestart 依本次註冊的 exe 重新啟動的
# （它不是本腳本的子行程，但是本腳本的 RM 工作階段造成的）。收尾只停這些與其子孫，
# 不以行程名稱停止（fix F4 共通規則：使用者自己的宿主或連跑中的宿主不得被誤殺）。
$script:OwnedHosts = New-Object System.Collections.Generic.List[object]
$script:ScriptStartedAt = Get-Date
function Add-OwnedHost([int]$Id, $StartTime) {
    $script:OwnedHosts.Add([PSCustomObject]@{ Id = $Id; StartTime = $StartTime })
}

function Wait-NewFcHost([int]$OldPid, [int]$TimeoutSec = 20) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt $TimeoutSec) {
        $cands = @(Get-FcHostProcesses | Where-Object { [int]$_.ProcessId -ne $OldPid })
        if ($cands.Count -ge 1) {
            $c = $cands[0]
            # 偵測不篩路徑（B4 要看到任何重新啟動）；納入收尾只限本次測試的 exe。
            if (-not $c.ExecutablePath -or [string]::Equals($c.ExecutablePath, $Exe, [StringComparison]::OrdinalIgnoreCase)) {
                Add-OwnedHost ([int]$c.ProcessId) $c.CreationDate
            } else {
                W "## 新出現的 fc-host pid=$($c.ProcessId) 路徑=$($c.ExecutablePath) 不是本次測試的 exe，不納入收尾"
            }
            return $c
        }
        Start-Sleep -Milliseconds 300
    }
    return $null
}

function Stop-OwnedHosts {
    # RmRestart 可能在 Wait-NewFcHost 逾時之後才拉起宿主：本次測試 exe、且在本腳本開始之後才
    # 建立的 fc-host 也是本腳本的 RM 工作階段造成的，一併納入（仍不以名稱全停）。
    foreach ($c in (Get-FcHostProcesses)) {
        $mine = [string]::Equals([string]$c.ExecutablePath, $Exe, [StringComparison]::OrdinalIgnoreCase) -and
            $c.CreationDate -and ([datetime]$c.CreationDate -ge $script:ScriptStartedAt)
        $known = @($script:OwnedHosts | Where-Object { $_.Id -eq [int]$c.ProcessId }).Count -gt 0
        if ($mine -and -not $known) {
            W "## 收尾：發現本次測試 exe 於腳本開始後建立的 fc-host pid=$($c.ProcessId)（RM 晚到的重新啟動），納入收尾"
            Add-OwnedHost ([int]$c.ProcessId) $c.CreationDate
        }
    }
    foreach ($h in @($script:OwnedHosts.ToArray())) {
        $ids = @(Stop-ProcessTree -RootId $h.Id -RootStartTime $h.StartTime)
        W "## 收尾：停止本腳本負責的宿主 pid=$($h.Id) 及其子孫（共 $($ids.Count) 個 PID）"
    }
    $script:OwnedHosts.Clear()
    Start-Sleep -Seconds 1
}

function New-TempHostEnv([string]$Tag) {
    $root = Join-Path ([IO.Path]::GetTempPath()) ("fc-host-5.7$Tag-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
    $appData = Join-Path $root 'Roaming'
    $local = Join-Path $root 'Local'
    $dataDir = Join-Path $local 'tw.fintools.fc-host\data'
    New-Item -ItemType Directory -Force -Path $appData, $dataDir | Out-Null
    Copy-Item $FixtureEvents (Join-Path $dataDir 'tw_events.json')
    [PSCustomObject]@{
        Root         = $root
        AppData      = $appData
        Local        = $local
        DataDir      = $dataDir
        SettingsJson = Join-Path $appData 'tw.fintools.fc-host\settings.json'
        UnifiedLog   = Join-Path $local "tw.fintools.fc-host\logs\fc-host.$(Get-Date -Format 'yyyy-MM-dd').log"
    }
}

function Start-IsolatedHost($TempEnv, [int]$Port = 0) {
    $oldWv2 = $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS
    try {
        $env:APPDATA = $TempEnv.AppData
        $env:LOCALAPPDATA = $TempEnv.Local
        if ($Port -gt 0) { $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$Port" }
        $p = Start-Process -FilePath $Exe -PassThru
        Add-OwnedHost $p.Id (Get-ProcessStartTimeOrNull $p)
        return $p
    } finally {
        $env:APPDATA = $script:oldAppData
        $env:LOCALAPPDATA = $script:oldLocal
        $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $oldWv2
    }
}

# ── 真實使用者資料保護（RmRestart 重啟的新行程讀寫真正的 %APPDATA%／%LOCALAPPDATA%） ─────────
$oldAppData = $env:APPDATA
$oldLocal = $env:LOCALAPPDATA
$realCfgDir = Join-Path $oldAppData 'tw.fintools.fc-host'
$RealItems = @(
    (Join-Path $realCfgDir 'settings.json'),
    (Join-Path $realCfgDir 'gatekeeper.log'),
    (Join-Path $oldLocal 'tw.fintools.fc-host\data'),
    (Join-Path $oldLocal 'tw.fintools.fc-host\logs'),
    # installer-auto-update：RmRestart 重啟的新行程不在隔離環境，更新器會啟用並讀寫這兩個檔
    # （當機標記、更新嘗試紀錄）。注意：開發版本號低於 GitHub Latest 時，該行程可能真的安裝正式版——
    # 跑這支前先確認 host/Cargo.toml 的版本不低於 Latest。
    (Join-Path $oldLocal 'tw.fintools.fc-host\startup-marker'),
    (Join-Path $oldLocal 'tw.fintools.fc-host\update-state.json')
)
# 所有未還原項目（描述字串，含備份路徑），最後寫進摘要並判 FAIL。
$NotRestored = New-Object System.Collections.Generic.List[string]

# fix F2b（重審 M2）：逐項 try/catch。任何一項搬不動（例如被編輯器／tail 持有控制代碼）時，
# 已搬走的項目立刻經 Exit-RealDataGuard 逐項還原（還原不了的列入 $NotRestored，R 判 FAIL），
# 再丟例外讓情境記為未跑完。搬不動的那一項本身沒被動過，不列入 $guard.Items——否則還原時的
# Remove-WithRetry 會把使用者原本的資料當成「測試期間新生成的內容」刪掉。
function Enter-RealDataGuard([string]$Tag) {
    $guard = [PSCustomObject]@{ Tag = $Tag; Items = @(); CfgDirCreated = $false }
    $failure = $null
    foreach ($path in $RealItems) {
        $bak = $null
        try {
            if (Test-Path $path) {
                $bak = "$path.rmtest-bak-$Tag-$([DateTimeOffset]::Now.ToUnixTimeSeconds())"
                Rename-Item -Path $path -NewName (Split-Path $bak -Leaf) -ErrorAction Stop
            }
        } catch {
            $failure = "搬移 $path 失敗：$($_.Exception.Message)"
            break
        }
        $guard.Items += [PSCustomObject]@{ Path = $path; Backup = $bak }
        if ($bak) { W "## $Tag 環境保護：搬移 $path → $bak" }
    }
    if (-not $failure -and -not (Test-Path $realCfgDir)) {
        try {
            New-Item -ItemType Directory -Force -Path $realCfgDir -ErrorAction Stop | Out-Null
            $guard.CfgDirCreated = $true
            W "## $Tag 環境保護：真正的設定目錄原本不存在，暫時建立 $realCfgDir（收尾時移除）"
        } catch {
            $failure = "建立 $realCfgDir 失敗：$($_.Exception.Message)"
        }
    }
    if ($failure) {
        W "## $Tag 環境保護中止：$failure；先還原已搬走的 $(@($guard.Items | Where-Object { $_.Backup }).Count) 項"
        Exit-RealDataGuard $guard
        throw "$Tag 環境保護中止：$failure"
    }
    return $guard
}

function Remove-WithRetry([string]$Path) {
    for ($i = 0; $i -lt 5 -and (Test-Path $Path); $i++) {
        try { Remove-Item -Recurse -Force $Path -ErrorAction Stop } catch { Start-Sleep -Milliseconds 500 }
    }
    return -not (Test-Path $Path)
}

# 逐項還原；任何一項失敗都不中斷其餘項目，失敗描述加進 $NotRestored。
function Exit-RealDataGuard($Guard) {
    if (-not $Guard) { return }
    foreach ($item in $Guard.Items) {
        try {
            if (-not (Remove-WithRetry $item.Path)) {
                $script:NotRestored.Add("$($Guard.Tag)：$($item.Path) 風險窗口內新生成的內容刪不掉$(if ($item.Backup) { "；原檔仍在備份 $($item.Backup)，需手動還原" })")
                continue
            }
            if ($item.Backup) { Rename-Item -Path $item.Backup -NewName (Split-Path $item.Path -Leaf) -ErrorAction Stop }
            $ok = if ($item.Backup) { Test-Path $item.Path } else { -not (Test-Path $item.Path) }
            if (-not $ok) { $script:NotRestored.Add("$($Guard.Tag)：$($item.Path) 還原後狀態與測試前不符$(if ($item.Backup) { "（備份 $($item.Backup)）" })") }
        } catch {
            $script:NotRestored.Add("$($Guard.Tag)：$($item.Path) 還原失敗：$($_.Exception.Message)$(if ($item.Backup) { "；備份 $($item.Backup)" })")
        }
    }
    if ($Guard.CfgDirCreated) {
        try {
            if (-not (Remove-WithRetry $realCfgDir)) { $script:NotRestored.Add("$($Guard.Tag)：暫時建立的 $realCfgDir 刪不掉") }
        } catch { $script:NotRestored.Add("$($Guard.Tag)：暫時建立的 $realCfgDir 刪除失敗：$($_.Exception.Message)") }
    }
    W "## $($Guard.Tag) 環境還原完成；目前累計未還原 $($script:NotRestored.Count) 項"
}

# 測試專用版面：沿用首次啟動建立的暫存設定檔，關閉 fixed、資料目錄指向暫存 fixture，並明寫
# `data_fetch: "off"`——RmRestart 重啟出的新行程拿到真實環境變數，`auto` 的隔離偵測
# （LOCALAPPDATA 與 SHGetKnownFolderPath 比較）會判成「非隔離」而開始抓取，覆寫搬進真實路徑的
# 樣本（data-layer-rust design.md D10、tasks.md 5.3 對照項）。舊設定檔沒有這個鍵時以 Add-Member 補上。
function ConvertTo-RestartTestLayout($Layout, [string]$DataDir) {
    $Layout.widgets.fixed.enabled = $false
    $Layout.data_dir = $DataDir
    $Layout | Add-Member -NotePropertyName 'data_fetch' -NotePropertyValue 'off' -Force
    return $Layout
}

$results = [ordered]@{}

# ── 前置：確認沒有 fc-host 在跑；快照開機自啟登錄 ──────────────────────────────────────
if (Get-Process -Name fc-host -ErrorAction SilentlyContinue) {
    throw '已有 fc-host 在執行，請先結束（避免與本次隔離實例互相干擾，也避免誤傷不相干的行程）。'
}
$regSnap = @(Save-FcHostAutostartRegistry)
W "# 開機自啟登錄快照：$(($regSnap | ForEach-Object { "$($_.Name)@$(Split-Path $_.Key -Leaf) Exists=$($_.Exists)" }) -join '; ')"

$a4Restarted = $false
try {
    # ════════════════════════════════════════════════════════════════════════════════════
    # 情境 A：有註冊 → RM 關閉後重新啟動，並依保存的版面顯示
    # ════════════════════════════════════════════════════════════════════════════════════
    $envA = New-TempHostEnv 'A'
    $hostProcA = $null
    $sessionA = $null
    $guardA = $null
    try {
        W "## 情境 A：暫存 APPDATA=$($envA.AppData) LOCALAPPDATA=$($envA.Local)"
        $hostProcA = Start-IsolatedHost $envA
        $hostPidA = $hostProcA.Id
        W "## A 宿主 pid=$hostPidA startTime=$($hostProcA.StartTime)"

        $ready = Wait-FinanceWidgetsReady $hostPidA 30
        W "## A 四個財經小工具就緒=$ready"
        $results['A1 首次啟動就緒（clock／macro／fixed／dynamic 視窗出現）'] = $ready
        Wait-Uptime $hostProcA 'A'

        $layout = ConvertTo-RestartTestLayout (Get-Content $envA.SettingsJson -Raw | ConvertFrom-Json) $envA.DataDir

        $sessionA = New-RmSession
        $regHr = Register-RmExeFile $sessionA $Exe
        $results['A2 RmRegisterResources 成功（只註冊本次測試宿主 exe 這一個檔名）'] = ($regHr -eq 0)

        $guardA = Enter-RealDataGuard 'A'
        # 無 BOM 的 UTF-8（Windows PowerShell 5.1 的 Set-Content -Encoding UTF8 會加 BOM）。
        [IO.File]::WriteAllText((Join-Path $realCfgDir 'settings.json'), ($layout | ConvertTo-Json -Depth 20), (New-Object Text.UTF8Encoding $false))
        W "## A 已在真正路徑放置測試專用版面檔（fixed 關閉、data_fetch=off、data_dir=$($envA.DataDir)）"

        $shutdownHr = Invoke-RmShutdown $sessionA
        $exited = Wait-ProcessGone $hostPidA
        W "## A RmShutdown hr=$shutdownHr 舊行程(pid=$hostPidA)已結束=$exited"
        $results['A3 RmShutdown 後舊行程結束'] = $exited

        $restartHr = Invoke-RmRestart $sessionA
        $newProc = Wait-NewFcHost $hostPidA 20
        $hasRestartedArg = [bool]($newProc -and $newProc.CommandLine -and $newProc.CommandLine -match '--restarted')
        W "## A RmRestart hr=$restartHr 新行程=$(if ($newProc) { "pid=$($newProc.ProcessId) cmdline=[$($newProc.CommandLine)]" } else { '（20 秒內未出現）' }) 含--restarted=$hasRestartedArg"
        $a4Restarted = [bool]($newProc -and $hasRestartedArg)
        $results['A4 RmRestart 後出現新行程且命令列含 --restarted'] = $a4Restarted

        if ($newProc) {
            $newPid = [int]$newProc.ProcessId
            $layoutShown = Wait-WindowTitles $newPid { param($t) ('fc-host clock' -in $t) -and ('fc-host macro' -in $t) } 30
            Start-Sleep -Seconds 5
            $titles = Get-HostWindowTitles $newPid
            $alive = [bool](Get-Process -Id $newPid -ErrorAction SilentlyContinue)
            $fixedShown = 'fc-host fixed' -in $titles
            W "## A 重啟後：clock＋macro 30 秒內出現=$layoutShown；再 5 秒後存活=$alive；視窗=[$($titles -join ', ')]；fixed 出現=$fixedShown（測試版面已關閉 fixed）"
            $results['A5 重啟的宿主恢復執行（存活）並依保存的版面顯示（clock、macro 出現；測試版面關閉的 fixed 不出現）'] =
                $layoutShown -and $alive -and (-not $fixedShown)

            # 診斷：新行程的啟動記錄落在哪裡（預期真正路徑，見 memory）；不是判準。
            $tempHit = (Test-Path $envA.UnifiedLog) -and [bool](Select-String -Path $envA.UnifiedLog -Pattern "pid=$newPid\b" -Quiet)
            $realLogDir = Join-Path $oldLocal 'tw.fintools.fc-host\logs'
            $realHit = (Test-Path $realLogDir) -and [bool](Get-ChildItem $realLogDir -Filter '*.log' -ErrorAction SilentlyContinue |
                    Select-String -Pattern "pid=$newPid\b" -Quiet)
            W "## A 診斷：新行程 pid=$newPid 啟動記錄命中暫存路徑=$tempHit／（搬空後的）真正路徑=$realHit"
        } else {
            $results['A5 重啟的宿主恢復執行（存活）並依保存的版面顯示（clock、macro 出現；測試版面關閉的 fixed 不出現）'] = $false
        }
    }
    catch {
        W "## A 例外中止：$($_.Exception.Message)"
        $results['A 情境未跑完（見 5.7-driver.log 例外訊息）'] = $false
    }
    finally {
        # 先確保沒有任何 fc-host（含 RM 重啟的新行程）還在跑，才能還原真實資料。
        try { if ($sessionA) { Close-RmSession $sessionA | Out-Null } } catch { W "## A RmEndSession 例外：$($_.Exception.Message)" }
        Stop-OwnedHosts
        Exit-RealDataGuard $guardA
        Remove-Item -Recurse -Force $envA.Root -ErrorAction SilentlyContinue
        W '## 情境 A 收尾完成'
    }

    # ════════════════════════════════════════════════════════════════════════════════════
    # 情境 B：取消註冊（系統匣「結束」同一路徑）→ RM 真的關閉它後不會重新啟動
    # ════════════════════════════════════════════════════════════════════════════════════
    if (Get-Process -Name fc-host -ErrorAction SilentlyContinue) {
        W '## 情境 B 前置檢查失敗：情境 A 收尾後仍有 fc-host 在跑，略過情境 B'
        $results['B0 情境 B 前置：情境 A 收尾乾淨（無殘留 fc-host）'] = $false
    } else {
        $envB = New-TempHostEnv 'B'
        $hostProcB = $null
        $sessionB = $null
        $guardB = $null
        try {
            W "## 情境 B：暫存 APPDATA=$($envB.AppData) LOCALAPPDATA=$($envB.Local) cdpPort=$CdpPort"
            $hostProcB = Start-IsolatedHost $envB $CdpPort
            $hostPidB = $hostProcB.Id
            W "## B 宿主 pid=$hostPidB"
            $readyB = Wait-FinanceWidgetsReady $hostPidB 30
            W "## B 四個財經小工具就緒=$readyB"
            $results['B1 第二個隔離宿主啟動就緒'] = $readyB
            Wait-Uptime $hostProcB 'B'

            $unregOut = ((& $Node $CdpEval $CdpPort 'w=clock' "window.__TAURI__.core.invoke('self_test_unregister_restart')" 2>&1) -join ' ').Trim()
            Start-Sleep -Milliseconds 500
            $unregLogged = (Test-Path $envB.UnifiedLog) -and [bool](Select-String -Path $envB.UnifiedLog -Pattern '已取消重新啟動註冊' -Quiet)
            W "## B self_test_unregister_restart（CDP invoke，非輸入注入）output=$unregOut 記錄檔有「已取消重新啟動註冊」=$unregLogged"
            $results['B2 取消重新啟動註冊（與系統匣結束同一個 unregister_restart）已執行且行程仍存活'] =
                ($unregOut -match 'true') -and $unregLogged -and [bool](Get-Process -Id $hostPidB -ErrorAction SilentlyContinue)
            if ($unregOut -notmatch 'true') {
                W '## B 指令不存在或失敗：exe 可能不是以 --features self-test-ipc 建置'
            }

            $sessionB = New-RmSession
            $regHrB = Register-RmExeFile $sessionB $Exe
            W "## B RmRegisterResources hr=$regHrB"
            $guardB = Enter-RealDataGuard 'B'
            # 萬一被錯誤重啟，新行程讀到的也是 data_fetch=off 的測試版面，不會對真實路徑抓取寫檔。
            $layoutB = ConvertTo-RestartTestLayout (Get-Content $envB.SettingsJson -Raw | ConvertFrom-Json) $envB.DataDir
            [IO.File]::WriteAllText((Join-Path $realCfgDir 'settings.json'), ($layoutB | ConvertTo-Json -Depth 20), (New-Object Text.UTF8Encoding $false))
            W "## B 已在真正路徑放置測試專用版面檔（data_fetch=off、data_dir=$($envB.DataDir)）"

            $shutdownHrB = Invoke-RmShutdown $sessionB
            $exitedB = Wait-ProcessGone $hostPidB
            W "## B RmShutdown hr=$shutdownHrB 宿主(pid=$hostPidB)被 RM 關閉=$exitedB"
            $results['B3 RmShutdown 真的關閉了已取消註冊的宿主（B4 才有鑑別力）'] = ($regHrB -eq 0) -and $exitedB

            $restartHrB = Invoke-RmRestart $sessionB
            $spurious = Wait-NewFcHost $hostPidB 20
            W "## B RmRestart hr=$restartHrB 20 秒內新 fc-host 行程=$(if ($spurious) { "pid=$($spurious.ProcessId) cmdline=[$($spurious.CommandLine)]" } else { '無' })；對照組 A4（有註冊會重啟）=$a4Restarted"
            $results['B4 取消註冊後 RmRestart 不重新啟動（且對照組 A4 有註冊時確實會重啟）'] = (-not $spurious) -and $a4Restarted
        }
        catch {
            W "## B 例外中止：$($_.Exception.Message)"
            $results['B 情境未跑完（見 5.7-driver.log 例外訊息）'] = $false
        }
        finally {
            try { if ($sessionB) { Close-RmSession $sessionB | Out-Null } } catch { W "## B RmEndSession 例外：$($_.Exception.Message)" }
            Stop-OwnedHosts
            Exit-RealDataGuard $guardB
            Remove-Item -Recurse -Force $envB.Root -ErrorAction SilentlyContinue
            W '## 情境 B 收尾完成'
        }
    }
}
finally {
    try {
        $regLeft = @(Restore-FcHostAutostartRegistry $regSnap)
        foreach ($r in $regLeft) { $NotRestored.Add("開機自啟登錄：$r") }
    } catch { $NotRestored.Add("開機自啟登錄還原例外：$($_.Exception.Message)") }
    $results['R 真正的 settings.json／gatekeeper.log／data／logs 與開機自啟登錄全部還原為測試前狀態'] = ($NotRestored.Count -eq 0)
    W "## 還原總結：未還原 $($NotRestored.Count) 項"
    foreach ($n in $NotRestored) { W "## 未還原：$n" }
    $log.Close()
}

$sum = New-EvidenceWriter $sumPath
$sum.WriteLine("# rm-restart-test.ps1 summary $(Get-Ts)")
foreach ($k in $results.Keys) { $sum.WriteLine("$(if ($results[$k]) { 'PASS' } else { 'FAIL' })  $k") }
if ($NotRestored.Count -gt 0) {
    $sum.WriteLine('# 未還原項目（需手動處理）：')
    foreach ($n in $NotRestored) { $sum.WriteLine("#   $n") }
}
$sum.Close()
Get-Content $sumPath

$failed = @($results.Values | Where-Object { -not $_ })
if ($failed.Count -gt 0) { exit 1 }
exit 0
