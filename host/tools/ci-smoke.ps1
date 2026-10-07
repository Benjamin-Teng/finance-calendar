<#
.SYNOPSIS
    CI 冒煙測試（installer-auto-update task 5.2、design.md D6）：在 GitHub 託管 runner 上靜默安裝 → 寫入
    `data_fetch: "off"` 設定 → 啟動宿主 → 存活 N 秒 → 記錄檔有啟動行 → 靜默解除安裝 → 無殘留宿主行程。

.DESCRIPTION
    **只能在 GitHub Actions 的託管 runner 上執行**（`GITHUB_ACTIONS=true`，否則拒絕）：本腳本會真的安裝到目前使用者的
    `%LOCALAPPDATA%\fc-host`、寫 HKCU 的 Run／解除安裝鍵與使用者資料夾、啟動並結束宿主；在開發機上跑會動到真實安裝。
    runner 是一次性的虛擬機，沒有這個顧慮。

    為什麼先寫 `data_fetch: "off"`：runner 的 `%LOCALAPPDATA%` 是真實使用者路徑，宿主的資料抓取隔離偵測
    （data-layer-rust D10）會判為「非隔離」而去抓外網；冒煙測試不需要資料，也不該讓測試結果依賴外部來源是否可達。

    步驟（任一步失敗即丟例外，結束碼非 0）：
      1. 前置：僅限 CI、安裝檔存在、runner 上沒有既有安裝（否則結果不可信）。記錄桌面工作階段資訊（見下）。
      2. `<安裝檔> /S` 靜默安裝（不帶 `/R`：安裝完不自動啟動；由本腳本自己啟動以便掌握 PID）。
         取 HKCU 解除安裝鍵的 InstallLocation（退回 `%LOCALAPPDATA%\fc-host`），確認 fc-host.exe 存在，
         且其 PE 機器型別符合 -Arch（x64＝0x8664、arm64＝0xAA64；防止兩個架構的安裝檔放反）。
      3. 寫 `%APPDATA%\tw.fintools.fc-host\settings.json`＝`{"version":<SETTINGS_VERSION>,"data_fetch":"off"}`；版本從
         `host/src/settings.rs` 的 `pub const SETTINGS_VERSION` 讀出（-SettingsSource），讀不到就失敗——不寫死，
         否則日後升版時宿主會把這份設定判為版本不符、退回預設（`data_fetch: auto`）而去抓外網，冒煙仍會通過。
      4. 啟動 `fc-host.exe --autostart`（與開機自啟的命令列相同）。
      5. 存活 -AliveSeconds 秒（預設 60）：每 2 秒確認行程還在，提早結束就失敗並印出結束碼。
      6. 記錄檔（`%LOCALAPPDATA%\tw.fintools.fc-host\logs\fc-host.<日期>.log`）必須有本次啟動行
         `fc-host 啟動 version=<版本> pid=<本次 PID>`；出現 panic（target fc_host::panic）就失敗；[ERROR] 行只列出、不判失敗。
         另以記錄檔**正向確認抓取確實停用**：必須有宿主實際寫出的那一行
         `抓取排程：data_fetch=off，不抓取（…）`（`host/src/fetch/scheduler.rs` 的 `log_gate`，target fc_host::fetch），
         且不得有 `抓取排程：啟用` 行；缺行或啟用就失敗（宿主若把設定判為不合法，會退回 auto 並啟用）。
      7. 靜默解除安裝（`uninstall.exe /S`；宿主仍在執行，等於使用者在宿主執行中解除安裝，會走 PREUNINSTALL hook 與
         `CheckIfAppIsRunning`）；等到 fc-host.exe 與 uninstall.exe 都消失（上限 -UninstallTimeoutSeconds 秒）。
      8. 無殘留 fc-host 行程（等最多 15 秒讓它收尾）。
    結束（含失敗）時把記錄檔與 uninstall.log 複製到 -LogOut，供 workflow 上傳成 artifact；本腳本啟動的行程一律收掉。

    互動式桌面：**待第一次實跑確認**——託管 runner 是否有讓 WebView2 建立視窗的互動式桌面工作階段，文件沒有明講。
    **成敗判準完全不依賴視窗**：只看行程存活 -AliveSeconds 秒、記錄檔有本次啟動行與「抓取停用」行、無 panic、解除安裝乾淨。
    視窗只作資訊（工作階段編號、UserInteractive、explorer 是否在跑、宿主擁有的可見視窗數〔EnumWindows〕）；沒有視窗時 step summary
    註明「視窗未驗」。若 runner 沒有互動桌面而 WebView2 起不來導致宿主在啟動後結束，那是判準的「行程存活」這一項會失敗，不是視窗檢查：
    第一次實跑時請看 step summary 的視窗數與工作階段資訊、診斷 artifact 裡的記錄檔（宿主是否在 WebView2 建立失敗後仍存活、
    記錄檔有沒有啟動行與抓取停用行），再決定是否需要調整 x64 冒煙的判準（x64 冒煙不可豁免）。

.PARAMETER Installer
    安裝檔路徑（finance-calendar-setup.exe 或 finance-calendar-setup-arm64.exe）。

.PARAMETER Arch
    x64 或 arm64；用來核對安裝後 fc-host.exe 的 PE 機器型別。

.PARAMETER Version
    預期版本（等於 host/Cargo.toml），記錄檔啟動行要帶這個版本。

.PARAMETER AliveSeconds
    啟動後要存活的秒數，預設 60。

.PARAMETER UninstallTimeoutSeconds
    等解除安裝完成的上限秒數，預設 300。解除安裝的 PREUNINSTALL hook 會呼叫 `fc-host.exe --restore-wallpaper`（單次硬上限 90 秒，
    結束碼 3／4／7 等會再重跑一次，最壞 2×90 秒再加 15 秒等待＝195 秒），之後才是 CheckIfAppIsRunning 與刪檔，180 秒不夠。

.PARAMETER InstallTimeoutSeconds
    等安裝完成的上限秒數，預設 300（含必要時下載 WebView2 bootstrapper）。

.PARAMETER SettingsSource
    宿主的 settings.rs 路徑（讀 SETTINGS_VERSION 用），預設 <repo>\host\src\settings.rs。

.PARAMETER LogOut
    診斷檔輸出資料夾，預設 $env:RUNNER_TEMP\smoke-logs-<Arch>。
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Installer,
    [Parameter(Mandatory)][ValidateSet('x64', 'arm64')][string]$Arch,
    [Parameter(Mandatory)][string]$Version,
    [int]$AliveSeconds = 60,
    [int]$UninstallTimeoutSeconds = 300,
    [int]$InstallTimeoutSeconds = 300,
    [string]$SettingsSource,
    [string]$LogOut
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# ================================================================================================
# 純函式（ci-smoke.Tests.ps1 以語法樹取出本檔函式後測試；函式不得依賴腳本層級變數）
# ================================================================================================

# 只允許在 GitHub Actions 上跑。回傳錯誤訊息（空字串＝允許）。
function Get-CiGuardError([string]$GithubActions) {
    if ($GithubActions -ne 'true') { return '拒絕執行：本腳本會真的安裝／啟動／解除安裝宿主並寫入使用者資料與 HKCU，只能在 GitHub Actions 託管 runner（GITHUB_ACTIONS=true）上跑；在開發機上跑會動到真實安裝' }
    ''
}

# PE 檔的機器型別（IMAGE_FILE_HEADER.Machine）：x64＝0x8664、arm64＝0xAA64、x86＝0x014C；讀不出回傳 0。
function Get-PeMachine([string]$Path) {
    $fs = [System.IO.File]::OpenRead($Path)
    try {
        $br = New-Object System.IO.BinaryReader($fs)
        if ($fs.Length -lt 0x40 -or $br.ReadUInt16() -ne 0x5A4D) { return 0 }
        $fs.Position = 0x3C
        $peOffset = $br.ReadInt32()
        if ($peOffset -lt 0 -or $peOffset + 6 -gt $fs.Length) { return 0 }
        $fs.Position = $peOffset
        if ($br.ReadUInt32() -ne 0x00004550) { return 0 }
        return [int]$br.ReadUInt16()
    }
    finally { $fs.Dispose() }
}

function Get-ExpectedPeMachine([string]$Arch) {
    switch ($Arch) { 'x64' { 0x8664 } 'arm64' { 0xAA64 } default { throw "未知架構：$Arch" } }
}

# 記錄檔文字中找本次啟動行：`<時間> [ INFO] <target>: fc-host 啟動 version=<版本> pid=<PID>`。
# 回傳符合的行（可能多行：同一份記錄檔含先前行程的啟動行；只認版本與 PID 都相符的）。
function Find-StartLogLines([string]$LogText, [string]$Version, [int]$ProcessId) {
    $pattern = "fc-host 啟動 version=$([regex]::Escape($Version)) pid=$ProcessId(\s|$)"
    @($LogText -split "`r?`n" | Where-Object { $_ -match $pattern })
}

# 記錄檔中的 panic 行（logging.rs 的 panic hook 以 target fc_host::panic 寫 ERROR）與一般 ERROR 行。
function Get-LogProblems([string]$LogText) {
    $lines = @($LogText -split "`r?`n")
    @{
        Panics = @($lines | Where-Object { $_ -match 'fc_host::panic' })
        Errors = @($lines | Where-Object { $_ -match '\[\s*ERROR\]' -and $_ -notmatch 'fc_host::panic' })
    }
}

# 指定行程擁有的可見頂層視窗數（EnumWindows）。runner 沒有互動桌面時為 0（資訊用途，不判成敗）。
# 列舉寫在 C#（Add-Type）裡：避免 PowerShell scriptblock 轉成委派時的範圍問題。
function Get-VisibleWindowCount([int[]]$ProcessIds) {
    if (-not ('FcSmoke.Win' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
namespace FcSmoke {
    public static class Win {
        delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
        [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
        [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr hWnd);
        [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint pid);
        public static int CountVisible(int[] pids) {
            var set = new HashSet<uint>();
            foreach (var p in pids) set.Add((uint)p);
            int count = 0;
            EnumWindows((h, l) => {
                uint wp; GetWindowThreadProcessId(h, out wp);
                if (set.Contains(wp) && IsWindowVisible(h)) count++;
                return true;
            }, IntPtr.Zero);
            return count;
        }
    }
}
'@
    }
    [FcSmoke.Win]::CountVisible([int[]]$ProcessIds)
}

# 從 settings.rs 讀 `pub const SETTINGS_VERSION: u32 = <n>;`；找不到檔或樣式就丟例外（不退回預設值）。
function Get-SettingsVersion([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "找不到 settings.rs：$Path（讀不到 SETTINGS_VERSION，不寫死設定版本）" }
    $text = [System.IO.File]::ReadAllText($Path, [System.Text.Encoding]::UTF8)
    $m = [regex]::Match($text, '(?m)^\s*pub\s+const\s+SETTINGS_VERSION\s*:\s*u32\s*=\s*(\d+)\s*;')
    if (-not $m.Success) { throw "在 $Path 找不到 'pub const SETTINGS_VERSION: u32 = <n>;'" }
    [int]$m.Groups[1].Value
}

# 冒煙測試用的設定檔內容：data_fetch off，版本取自 SETTINGS_VERSION。
function New-SmokeSettingsJson([int]$SettingsVersion) {
    '{"version":' + $SettingsVersion + ',"data_fetch":"off"}'
}

# 記錄檔中的抓取閘門行（scheduler.rs log_gate，target fc_host::fetch）：
#   Off     ＝ `抓取排程：data_fetch=off，不抓取（只讀取資料目錄中已有的檔案）`
#   Enabled ＝ `抓取排程：啟用（data_fetch=…，資料目錄=…）`
#   Isolated＝ `隔離環境，不抓取（…）`（資料抓取關閉的另一種原因，不算「off」確認）
function Get-FetchGateLines([string]$LogText) {
    $lines = @($LogText -split "`r?`n")
    @{
        Off      = @($lines | Where-Object { $_ -match 'fc_host::fetch' -and $_.Contains('抓取排程：data_fetch=off，不抓取') })
        Enabled  = @($lines | Where-Object { $_ -match 'fc_host::fetch' -and $_.Contains('抓取排程：啟用') })
        Isolated = @($lines | Where-Object { $_ -match 'fc_host::fetch' -and $_.Contains('隔離環境，不抓取') })
    }
}

# 安裝目錄：HKCU 解除安裝鍵的 InstallLocation（NSIS 範本寫成帶引號的路徑），沒有就退回預設 %LOCALAPPDATA%\fc-host。
function Get-InstallDir {
    $key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\fc-host'
    $loc = $null
    if (Test-Path -LiteralPath $key) { $loc = (Get-ItemProperty -LiteralPath $key -ErrorAction SilentlyContinue).InstallLocation }
    if ($loc) { return ([string]$loc).Trim('"') }
    Join-Path $env:LOCALAPPDATA 'fc-host'
}

# 讀取可能仍被宿主開著寫入的文字檔（UTF-8）。宿主在冒煙期間一直開著當日記錄檔（Rust std 預設共用模式
# READ|WRITE|DELETE）；[IO.File]::ReadAllText 以 FileShare.Read 開檔，會與既有的寫入者衝突而失敗，
# 所以這裡明確允許與寫入者／刪除者共用。
function Read-SharedText([string]$Path) {
    $fs = [System.IO.FileStream]::new($Path, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read,
        ([System.IO.FileShare]::ReadWrite -bor [System.IO.FileShare]::Delete))
    try {
        $sr = [System.IO.StreamReader]::new($fs, [System.Text.Encoding]::UTF8)
        try { return $sr.ReadToEnd() } finally { $sr.Dispose() }
    }
    finally { $fs.Dispose() }
}

# 等條件成立（輪詢）；逾時回傳 $false。
function Wait-Until([scriptblock]$Condition, [int]$TimeoutSeconds, [int]$IntervalMs = 500) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt $TimeoutSeconds) {
        if (& $Condition) { return $true }
        Start-Sleep -Milliseconds $IntervalMs
    }
    [bool](& $Condition)
}

function Add-StepSummary([string]$Line) {
    Write-Host $Line
    if ($env:GITHUB_STEP_SUMMARY) { Add-Content -LiteralPath $env:GITHUB_STEP_SUMMARY -Value $Line -Encoding UTF8 }
}

# ================================================================================================
# 主流程
# ================================================================================================
function Invoke-Smoke {
    $guard = Get-CiGuardError $env:GITHUB_ACTIONS
    if ($guard) { throw $guard }
    if (-not (Test-Path -LiteralPath $Installer -PathType Leaf)) { throw "安裝檔不存在：$Installer" }
    $Installer = (Resolve-Path -LiteralPath $Installer).Path
    if (-not $LogOut) { $LogOut = Join-Path ($(if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [System.IO.Path]::GetTempPath() })) "smoke-logs-$Arch" }
    New-Item -ItemType Directory -Force -Path $LogOut | Out-Null

    if (-not $SettingsSource) { $SettingsSource = Join-Path (Join-Path (Split-Path $PSScriptRoot -Parent) 'src') 'settings.rs' }
    $settingsVersion = Get-SettingsVersion $SettingsSource   # 讀不到就在任何安裝動作之前失敗
    $appData = Join-Path $env:APPDATA 'tw.fintools.fc-host'
    $localData = Join-Path $env:LOCALAPPDATA 'tw.fintools.fc-host'
    $logDir = Join-Path $localData 'logs'
    $proc = $null
    $instDir = $null
    # 開機自啟登錄值（HKCU Run／StartupApproved\Run 的 fc-host）：與其他會啟動宿主的腳本一致，開頭快照、finally 還原
    # （AutostartRegistry.Tests.ps1 靜態檢查）。
    Import-Module (Join-Path $PSScriptRoot 'lib\AutostartRegistry.psm1') -Force
    $regSnap = @(Save-FcHostAutostartRegistry)

    # ---- 1. 前置（防呆）
    # 放在 try 之外：防呆 throw 若進入 finally，會把「事先就在跑」的 fc-host 當成本腳本啟動的而砍掉
    # （ProcessTree.Tests.ps1「已有 fc-host 防呆不得在 try 內」；本腳本只允許在 CI 跑，但規則一律適用）。
    Add-StepSummary "## 冒煙測試 $Arch（版本 $Version）"
    $existing = Join-Path $env:LOCALAPPDATA 'fc-host\fc-host.exe'
    $preflight = ''
    if (Test-Path -LiteralPath $existing) { $preflight = "runner 上已有既有安裝（$existing），結果不可信" }
    if (@(Get-Process -Name fc-host -ErrorAction SilentlyContinue).Count -gt 0) { $preflight = 'runner 上已有 fc-host 行程在跑，結果不可信' }
    if ($preflight) {
        Add-StepSummary "- **結果：失敗** — $preflight"
        throw $preflight
    }
    try {
        $explorer = @(Get-Process -Name explorer -ErrorAction SilentlyContinue).Count
        Write-Host "桌面資訊：工作階段 $((Get-Process -Id $PID).SessionId)、UserInteractive=$([Environment]::UserInteractive)、explorer 行程 $explorer 個、使用者 $env:USERNAME、OS $([Environment]::OSVersion.VersionString)、行程架構 $([System.Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture)"

        # ---- 2. 靜默安裝
        Write-Host "安裝：$Installer /S"
        $inst = Start-Process -FilePath $Installer -ArgumentList '/S' -PassThru
        $null = $inst.Handle   # 立刻快取把手：否則 PowerShell 7 的行程結束後 ExitCode 可能是 $null
        if (-not $inst.WaitForExit($InstallTimeoutSeconds * 1000)) { try { $inst.Kill($true) } catch { }; throw "安裝逾時（$InstallTimeoutSeconds 秒）" }
        if ($inst.ExitCode -ne 0) { throw "安裝檔結束碼 $($inst.ExitCode)（預期 0）" }
        $instDir = Get-InstallDir
        $exe = Join-Path $instDir 'fc-host.exe'
        if (-not (Test-Path -LiteralPath $exe)) { throw "安裝完成但找不到 $exe（InstallLocation=$instDir）" }
        $machine = Get-PeMachine $exe
        $want = Get-ExpectedPeMachine $Arch
        if ($machine -ne $want) { throw ("fc-host.exe 的 PE 機器型別是 0x{0:X4}，-Arch $Arch 預期 0x{1:X4}（兩個架構的安裝檔放反了？）" -f $machine, $want) }
        Add-StepSummary ("- 安裝完成：``$instDir``，PE 機器型別 0x{0:X4}（$Arch 相符），ProductVersion {1}" -f $machine, (Get-Item -LiteralPath $exe).VersionInfo.ProductVersion)

        # ---- 3. 設定檔：data_fetch off
        New-Item -ItemType Directory -Force -Path $appData | Out-Null
        $settingsJson = New-SmokeSettingsJson $settingsVersion
        [System.IO.File]::WriteAllText((Join-Path $appData 'settings.json'), $settingsJson, (New-Object System.Text.UTF8Encoding($false)))
        Write-Host "設定檔：$settingsJson（版本取自 $SettingsSource）"

        # ---- 4. 啟動
        $proc = Start-Process -FilePath $exe -ArgumentList '--autostart' -PassThru
        $null = $proc.Handle
        Write-Host "已啟動 fc-host.exe --autostart（PID $($proc.Id)）"

        # ---- 5. 存活
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        $windows = 0
        $reportedAt = 0
        while ($sw.Elapsed.TotalSeconds -lt $AliveSeconds) {
            Start-Sleep -Seconds 2
            if ($proc.HasExited) { throw ("宿主在啟動後 {0:N0} 秒就結束了，結束碼 {1}（預期持續存活 {2} 秒）" -f $sw.Elapsed.TotalSeconds, $proc.ExitCode, $AliveSeconds) }
            if ($sw.Elapsed.TotalSeconds - $reportedAt -ge 15) {
                $reportedAt = [int]$sw.Elapsed.TotalSeconds
                $windows = Get-VisibleWindowCount @($proc.Id)
                $wv = @(Get-Process -Name msedgewebview2 -ErrorAction SilentlyContinue).Count
                Write-Host ("  存活 {0:N0}／{1} 秒：可見視窗 {2}、msedgewebview2 行程 {3}、宿主工作集 {4:N0} MB" -f $sw.Elapsed.TotalSeconds, $AliveSeconds, $windows, $wv, ($proc.WorkingSet64 / 1MB))
            }
        }
        $proc.Refresh()
        if ($proc.HasExited) { throw "宿主在存活檢查結束時已結束，結束碼 $($proc.ExitCode)" }
        $windows = Get-VisibleWindowCount @($proc.Id)
        Add-StepSummary "- 宿主存活 $AliveSeconds 秒（PID $($proc.Id)）；宿主擁有的可見視窗數：$windows"
        if ($windows -eq 0) { Add-StepSummary '- 注意：沒有偵測到可見視窗——runner 可能沒有互動式桌面工作階段（待第一次實跑確認），**視窗未驗**；成敗以行程存活與記錄檔啟動行為準' }

        # ---- 6. 記錄檔
        $logs = @(if (Test-Path -LiteralPath $logDir) { Get-ChildItem -LiteralPath $logDir -Filter 'fc-host.*.log' -File })
        if ($logs.Count -eq 0) { throw "記錄目錄沒有 fc-host.*.log：$logDir" }
        $logText = ($logs | ForEach-Object { Read-SharedText $_.FullName }) -join "`n"   # 宿主仍在執行、開著記錄檔寫入
        $startLines = @(Find-StartLogLines $logText $Version $proc.Id)
        if ($startLines.Count -eq 0) { throw "記錄檔沒有本次啟動行（fc-host 啟動 version=$Version pid=$($proc.Id)）；記錄檔：$($logs.Name -join ', ')" }
        $problems = Get-LogProblems $logText
        foreach ($e in $problems.Errors) { Write-Host "  [記錄檔 ERROR] $e" }
        if ($problems.Panics.Count -gt 0) { throw ("記錄檔含 panic：`n  " + ($problems.Panics -join "`n  ")) }
        $gate = Get-FetchGateLines $logText
        if ($gate.Enabled.Count -gt 0) { throw ("宿主啟用了資料抓取（記錄檔有『抓取排程：啟用』），冒煙測試的設定 data_fetch=off 沒有生效：`n  " + ($gate.Enabled -join "`n  ")) }
        if ($gate.Off.Count -eq 0) {
            throw ("記錄檔沒有『抓取排程：data_fetch=off，不抓取』這一行，無法確認資料抓取已停用（宿主可能把 settings.json 判為不合法而退回預設；請檢查 SETTINGS_VERSION 與設定檔）。" +
                "隔離環境行：$($gate.Isolated.Count) 筆；記錄檔：$($logs.Name -join ', ')")
        }
        Add-StepSummary "- 抓取已確認停用：``$($gate.Off[0].Trim())``"
        Add-StepSummary "- 記錄檔啟動行：``$($startLines[0].Trim())``；ERROR 行 $($problems.Errors.Count) 筆（只列出、不判失敗）"

        # ---- 7. 靜默解除安裝
        $un = Join-Path $instDir 'uninstall.exe'
        if (-not (Test-Path -LiteralPath $un)) { throw "找不到解除安裝程式：$un" }
        Write-Host "解除安裝：$un /S（宿主仍在執行）"
        $u = Start-Process -FilePath $un -ArgumentList '/S' -PassThru
        $null = $u.Handle
        [void]$u.WaitForExit($UninstallTimeoutSeconds * 1000)   # 解除安裝程式會把自己複製到暫存再啟動，這個行程可能早早結束；真正的完成條件看檔案
        $gone = Wait-Until { -not (Test-Path -LiteralPath $exe) -and -not (Test-Path -LiteralPath $un) } $UninstallTimeoutSeconds
        if (-not $gone) { throw "解除安裝逾時（$UninstallTimeoutSeconds 秒）：$instDir 仍有 fc-host.exe 或 uninstall.exe" }
        Add-StepSummary '- 靜默解除安裝完成（fc-host.exe 與 uninstall.exe 已移除）'

        # ---- 8. 無殘留行程
        $clean = Wait-Until { @(Get-Process -Name fc-host -ErrorAction SilentlyContinue).Count -eq 0 } 15
        if (-not $clean) { throw ('解除安裝後仍有殘留 fc-host 行程：' + ((Get-Process -Name fc-host -ErrorAction SilentlyContinue | ForEach-Object { "PID $($_.Id)" }) -join ', ')) }
        $run = (Get-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -ErrorAction SilentlyContinue)
        $runVal = if ($run -and $run.PSObject.Properties.Name -contains 'fc-host') { $run.'fc-host' } else { '（無）' }
        Add-StepSummary "- 無殘留 fc-host 行程；HKCU Run 的 fc-host 值：$runVal"
        Add-StepSummary '- **結果：通過**'
    }
    catch {
        Add-StepSummary "- **結果：失敗** — $($_.Exception.Message)"
        throw
    }
    finally {
        # 診斷檔（記錄檔、uninstall.log）→ -LogOut；本腳本啟動的行程與殘留一律收掉
        try {
            if (Test-Path -LiteralPath $logDir) { Copy-Item -LiteralPath $logDir -Destination (Join-Path $LogOut 'logs') -Recurse -Force -ErrorAction SilentlyContinue }
            $ul = Join-Path $localData 'logs\uninstall.log'
            if (Test-Path -LiteralPath $ul) { Copy-Item -LiteralPath $ul -Destination $LogOut -Force -ErrorAction SilentlyContinue }
            if (Test-Path -LiteralPath (Join-Path $appData 'settings.json')) { Copy-Item -LiteralPath (Join-Path $appData 'settings.json') -Destination $LogOut -Force -ErrorAction SilentlyContinue }
        }
        catch { Write-Host "收集診斷檔失敗：$($_.Exception.Message)" }
        if ($proc) { try { if (-not $proc.HasExited) { $proc.Kill($true) } } catch { } }
        Get-Process -Name fc-host -ErrorAction SilentlyContinue | ForEach-Object { try { $_.Kill($true) } catch { } }
        # 宿主已停止後才還原開機自啟登錄值（先前已確認 runner 上沒有既有宿主，上面的收尾只會殺到本腳本啟動的）
        $notRestored = @(Restore-FcHostAutostartRegistry $regSnap)
        if ($notRestored.Count -gt 0) { Write-Warning "開機自啟登錄值未還原：$($notRestored -join '; ')" }
    }
}

# 以 `. .\ci-smoke.ps1` 載入函式（測試）時不執行主流程。
if ($MyInvocation.InvocationName -ne '.') { Invoke-Smoke }
