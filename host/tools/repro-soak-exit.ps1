<#
.SYNOPSIS
    soak 事件重現：WebView2 子行程連串結束後，宿主（fc-host.exe）是否還活著。

.DESCRIPTION
    2026-10-01 soak 中宿主在 renderer×5、Storage／Network Service、GPU 行程依序當機
    （皆 0xC0000005）後無聲消失。本腳本以暫存設定啟動一個測試宿主，依情境終止它的 WebView2
    子行程，觀察宿主是否存活、結束碼、stderr／stdout 與宿主記錄檔。

    隔離：
      - `%APPDATA%`／`%LOCALAPPDATA%` 指向暫存目錄（設定檔、記錄檔、資料目錄都落在暫存樹下）。
      - WebView2 的 user data folder 另以 `WEBVIEW2_USER_DATA_FOLDER` 指向暫存目錄——Tauri 的
        預設 UDF 用 known folder API 解析，不吃 `%LOCALAPPDATA%` 覆寫（verify-5.8.ps1 的診斷
        dump 已證實）；本腳本啟動後會檢查 browser 行程命令列的 `--user-data-dir` 確實落在暫存
        目錄，否則立即中止並清理（不讓測試碰到使用者真正的 EBWebView）。
      - 開機自啟登錄以 `AutostartRegistry.psm1` 快照、結束時還原。
      - 只終止「測試宿主的子孫行程」（以 ParentProcessId 從宿主 PID 往下走），絕不依名稱終止
        其他程式的 msedgewebview2。

    不注入任何鍵盤／滑鼠輸入。`-OpenSettings` 以同一組暫存環境做一次「手動重複啟動」，讓既有
    宿主開出設定視窗（事件當時設定視窗開著）。

.PARAMETER Scenario
    gpu｜gpuloop｜renderers｜sequence｜browser｜hostkill｜none：
      gpu       只終止 GPU 行程
      gpuloop   GPU 行程連續 6 次（間隔 3 秒）；實測第 6 次後 browser 自行結束（0x80000003）
      renderers 終止所有 renderer
      sequence  依事件順序：renderer 逐一（間隔 0.45 秒）→ Storage Service → Network Service → GPU
      browser   終止 browser 行程
      hostkill  校準用：終止宿主本身，量 WebView2 子行程多久跟著結束、終止後寫了哪些 UDF 檔案
      none      不終止任何行程（對照組）
    加 -RealCrash 時 renderer／GPU／browser 改以 CDP 讓它們真的當機（cdp-crash.mjs）。-CdpPort
    已有行程在 Listen 時往上改用第一個空的埠（20 個內都被佔用就中止）；每次送當機指令前確認該埠
    的 Listen 行程全是測試宿主的子孫，不符就中止、不送指令（不讓當機指令打到其他程式）。

.PARAMETER ObserveSec
    終止後觀察秒數（預設 60）。
#>
[CmdletBinding()]
param(
    [ValidateSet('gpu', 'gpuloop', 'renderers', 'sequence', 'browser', 'hostkill', 'none')]
    [string]$Scenario = 'sequence',
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\fc-host.exe'),
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [int]$ObserveSec = 60,
    [switch]$OpenSettings,
    # 以 CDP（cdp-crash.mjs）讓 renderer／GPU／browser 真的當機，而不是 TerminateProcess。
    # utility 行程沒有對應的 CDP 當機指令，sequence 情境裡仍以終止代替。
    [switch]$RealCrash,
    [int]$CdpPort = 9351,
    [string]$SeedDataFile = ''
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'lib\AutostartRegistry.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\ProcessTree.psm1') -Force

function Get-Ts { (Get-Date).ToString('HH:mm:ss.fff') }

# 純函式：從行程快照 $All（需有 ProcessId、ParentProcessId、CreationDate）挑出根的子孫（不含根），
# 回傳原物件（保留 CommandLine、Name）。樹以 lib/ProcessTree.psm1 的 Get-ProcessTreeIds 計算，
# 有 PID 重用防護（fix F5，review fix-soak low）。
function Select-TreeProcesses {
    param([object[]]$All = @(), [Parameter(Mandatory)][int]$RootId, $RootStartTime = $null)
    $ids = @(Get-ProcessTreeIds -RootId $RootId -RootStartTime $RootStartTime -Processes $All)
    $want = New-Object System.Collections.Generic.HashSet[int]
    foreach ($i in $ids) { if ($i -ne $RootId) { [void]$want.Add([int]$i) } }
    return @($All | Where-Object { $want.Contains([int]$_.ProcessId) })
}

# ---- -RealCrash 的 CDP 埠安全（fix F5，review fix-soak high）--------------------------------
# 埠被別的 Chromium 系程式（其他程式的 WebView2、Edge）佔用時，測試宿主的 browser 綁不到埠、
# 只記一行錯誤就繼續跑；cdp-crash.mjs 連過去會把當機指令送到「佔用者」身上。兩道防護：
#   1. 啟動前挑一個沒人 Listen 的埠（Select-CdpPort），挑不到就中止；
#   2. 每次送當機指令前，確認在該埠 Listen 的行程全是本次測試宿主的子孫（Assert-CdpOwnedByHost），
#      不符就 throw、一個指令都不送（啟動後才被搶走的競態也擋得住）。

# 純函式：從 Preferred 起往上 Span 個埠，回傳第一個不在 BusyPorts 裡的；都被佔用回傳 $null。
function Select-CdpPort {
    param([Parameter(Mandatory)][int]$Preferred, [int[]]$BusyPorts = @(), [int]$Span = 20)
    for ($p = $Preferred; $p -lt $Preferred + $Span; $p++) {
        if (-not ($BusyPorts -contains $p)) { return $p }
    }
    return $null
}

# 純函式：Listen 行程 PID 清單 vs. 測試宿主的行程樹 → 'OK'｜'NO-LISTENER'｜'FOREIGN'。
function Get-CdpOwnerVerdict {
    param([int[]]$ListenerPids = @(), [int[]]$TreeIds = @())
    if (@($ListenerPids).Count -eq 0) { return 'NO-LISTENER' }
    foreach ($l in $ListenerPids) {
        if (-not ($TreeIds -contains $l)) { return 'FOREIGN' }
    }
    return 'OK'
}

# 在 $Port 上 Listen 的行程 PID（去重）。
function Get-ListenPids([int]$Port) {
    return @(Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue |
            ForEach-Object { [int]$_.OwningProcess } | Sort-Object -Unique)
}

# 送當機指令前呼叫：該埠的 Listen 行程不全是測試宿主的子孫就 throw。
function Assert-CdpOwnedByHost([int]$Port) {
    $listeners = @(Get-ListenPids $Port)
    $all = @(Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, CreationDate)
    $tree = @(Get-ProcessTreeIds -RootId $script:hostPid -RootStartTime $script:hostStart -Processes $all)
    $verdict = Get-CdpOwnerVerdict -ListenerPids $listeners -TreeIds $tree
    if ($verdict -ne 'OK') {
        throw "CDP 埠 $Port 擁有者檢查 $verdict（Listen pid=$($listeners -join ',')；宿主 pid=$script:hostPid），中止、不送任何當機指令"
    }
}

function Get-HostDescendants {
    $all = @(Get-CimInstance Win32_Process)
    return , @(Select-TreeProcesses -All $all -RootId $script:hostPid -RootStartTime $script:hostStart)
}

function Get-ProcType($Proc) {
    $cl = [string]$Proc.CommandLine
    $t = [regex]::Match($cl, '--type=([\w-]+)').Groups[1].Value
    if (-not $t) { $t = 'browser' }
    $sub = [regex]::Match($cl, '--utility-sub-type=([\w.]+)').Groups[1].Value
    if ($sub) { $t = "$t($sub)" }
    return $t
}

if (Get-Process fc-host -ErrorAction SilentlyContinue) {
    throw '已有 fc-host 在跑，為免誤動，本腳本不執行。'
}
$Exe = (Resolve-Path $Exe).Path
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ("fc-host-soakexit-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
$tempAppData = Join-Path $tempRoot 'Roaming'
$tempLocal = Join-Path $tempRoot 'Local'
$tempUdf = Join-Path $tempRoot 'WV2'
$dataDir = Join-Path $tempLocal 'tw.fintools.fc-host\data'
New-Item -ItemType Directory -Force -Path $tempAppData, $tempLocal, $tempUdf, $dataDir | Out-Null
if ($SeedDataFile -and (Test-Path $SeedDataFile)) { Copy-Item $SeedDataFile (Join-Path $dataDir 'tw_events.json') }
$stderrPath = Join-Path $tempRoot 'stderr.txt'
$stdoutPath = Join-Path $tempRoot 'stdout.txt'
$report = New-Object System.Collections.Generic.List[string]
function Note([string]$s) { $line = "$(Get-Ts) $s"; $report.Add($line); Write-Host $line }

$regSnap = @(Save-FcHostAutostartRegistry)
$saved = @{
    APPDATA                               = $env:APPDATA
    LOCALAPPDATA                          = $env:LOCALAPPDATA
    WEBVIEW2_USER_DATA_FOLDER             = $env:WEBVIEW2_USER_DATA_FOLDER
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS
}
$hostProc = $null
$hostPid = 0
$hostStart = $null
# -OpenSettings 啟動的第二個行程：finally 一律停掉（只停自己啟動的，Stop-ProcessTree）。
$second = $null
$knownDesc = @()
try {
    function Start-WithTempEnv([string[]]$ArgList, [switch]$Redirect) {
        try {
            $env:APPDATA = $tempAppData
            $env:LOCALAPPDATA = $tempLocal
            $env:WEBVIEW2_USER_DATA_FOLDER = $tempUdf
            if ($RealCrash) { $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$CdpPort" }
            $p = @{ FilePath = $Exe; PassThru = $true }
            if ($ArgList) { $p.ArgumentList = $ArgList }
            if ($Redirect) { $p.RedirectStandardError = $stderrPath; $p.RedirectStandardOutput = $stdoutPath }
            return Start-Process @p
        } finally {
            $env:APPDATA = $saved.APPDATA
            $env:LOCALAPPDATA = $saved.LOCALAPPDATA
            $env:WEBVIEW2_USER_DATA_FOLDER = $saved.WEBVIEW2_USER_DATA_FOLDER
            $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $saved.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS
        }
    }

    if ($RealCrash) {
        $busy = @(foreach ($p in $CdpPort..($CdpPort + 19)) { if (@(Get-ListenPids $p).Count -gt 0) { $p } })
        $port = Select-CdpPort -Preferred $CdpPort -BusyPorts $busy
        if ($null -eq $port) {
            throw "CDP 埠 $CdpPort 起 20 個都有行程在 Listen，中止（避免把當機指令送到其他程式）"
        }
        if ($port -ne $CdpPort) { Note "CDP 埠 $CdpPort 已被佔用（Listen pid=$((Get-ListenPids $CdpPort) -join ','))，改用 $port" }
        $CdpPort = $port
    }
    $hostProc = Start-WithTempEnv -Redirect
    $hostPid = $hostProc.Id
    $hostStart = Get-ProcessStartTimeOrNull $hostProc
    Note "scenario=$Scenario 宿主 pid=$hostPid temp=$tempRoot"

    # 等 browser 與 5 個 renderer 出現
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $desc = @()
    while ($sw.Elapsed.TotalSeconds -lt 40) {
        $desc = Get-HostDescendants
        $rcount = @($desc | Where-Object { (Get-ProcType $_) -eq 'renderer' }).Count
        if ($rcount -ge 5) { break }
        Start-Sleep -Milliseconds 500
    }
    $browser = @($desc | Where-Object { (Get-ProcType $_) -eq 'browser' -and $_.Name -eq 'msedgewebview2.exe' })
    if ($browser.Count -lt 1) { throw '找不到測試宿主的 browser 行程' }
    $udfArg = [regex]::Match([string]$browser[0].CommandLine, '--user-data-dir="?([^"]+?)"?(\s--|$)').Groups[1].Value
    Note "browser pid=$($browser[0].ProcessId) user-data-dir=$udfArg"
    if (-not $udfArg.StartsWith($tempUdf, [StringComparison]::OrdinalIgnoreCase)) {
        throw "UDF 不在暫存目錄（$udfArg），中止以免碰到使用者真正的 EBWebView"
    }

    if ($OpenSettings) {
        $second = Start-WithTempEnv
        $second.WaitForExit(15000) | Out-Null
        Note "手動重複啟動（開設定視窗）pid=$($second.Id) exited=$($second.HasExited)"
        Start-Sleep -Seconds 4
    }
    Start-Sleep -Seconds 3
    $desc = Get-HostDescendants
    $knownDesc = @($desc | ForEach-Object { [int]$_.ProcessId })
    foreach ($d in $desc) { Note "  child pid=$($d.ProcessId) ppid=$($d.ParentProcessId) type=$(Get-ProcType $d)" }

    function Kill-Type([string]$Pattern, [int]$GapMs = 0) {
        # 先指派再篩選：Get-HostDescendants 以 `, 陣列` 輸出，直接接管線會把整個陣列當成單一物件。
        $all = Get-HostDescendants
        $targets = @($all | Where-Object { (Get-ProcType $_) -match $Pattern -and $_.Name -eq 'msedgewebview2.exe' })
        foreach ($t in $targets) {
            Note "終止 pid=$($t.ProcessId) type=$(Get-ProcType $t)"
            Stop-Process -Id $t.ProcessId -Force -ErrorAction SilentlyContinue
            if ($GapMs -gt 0) { Start-Sleep -Milliseconds $GapMs }
        }
        if ($targets.Count -eq 0) { Note "沒有符合 $Pattern 的子行程" }
    }

    $cdpCrash = Join-Path $PSScriptRoot 'cdp-crash.mjs'
    function Crash-Cdp([string]$Mode) {
        Assert-CdpOwnedByHost $CdpPort
        Note "CDP 當機：$Mode（埠 $CdpPort 的 Listen 行程已確認屬於測試宿主）"
        $out = & node $cdpCrash $CdpPort $Mode 2>&1
        $code = $LASTEXITCODE
        foreach ($l in $out) { Note "  cdp> $l" }
        # 結束碼 3＝cdp-crash.mjs 自己的目標檢查不通過（未送指令），其他非 0＝執行失敗：都中止。
        if ($code -ne 0) { throw "cdp-crash.mjs 結束碼 $code，中止" }
    }

    if ($RealCrash) {
        switch ($Scenario) {
            'gpu' { Crash-Cdp 'gpu' }
            'gpuloop' { for ($i = 0; $i -lt 6; $i++) { Crash-Cdp 'gpu'; Start-Sleep -Seconds 3 } }
            'renderers' { Crash-Cdp 'renderers' }
            'sequence' {
                Crash-Cdp 'renderers'
                Kill-Type 'StorageService' 600
                Kill-Type 'NetworkService' 800
                Crash-Cdp 'gpu'
            }
            'browser' { Crash-Cdp 'browser' }
            'none' { }
        }
    } else {
        switch ($Scenario) {
            'gpu' { Kill-Type '^gpu-process$' }
            'gpuloop' { for ($i = 0; $i -lt 6; $i++) { Kill-Type '^gpu-process$'; Start-Sleep -Seconds 3 } }
            'renderers' { Kill-Type '^renderer$' }
            'sequence' {
                Kill-Type '^renderer$' 450
                Kill-Type 'StorageService' 600
                Kill-Type 'NetworkService' 800
                Kill-Type '^gpu-process$'
            }
            'browser' { Kill-Type '^browser$' }
            'none' { }
        }
    }

    if ($Scenario -eq 'hostkill') {
        # 校準用：宿主本身被終止後，WebView2 子行程多久跟著結束（對照事件中 EBWebView 的最後
        # 寫入時間，推估宿主實際消失的時刻）。
        Note "終止宿主本身 pid=$hostPid"
        Stop-Process -Id $hostPid -Force
        $sw = [Diagnostics.Stopwatch]::StartNew()
        while ($sw.Elapsed.TotalSeconds -lt $ObserveSec) {
            $alive = @($knownDesc | Where-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue })
            if ($alive.Count -eq 0) { break }
            Start-Sleep -Milliseconds 250
        }
        Note ("宿主終止後 {0:N1}s，本次 WebView2 子行程剩 {1} 個" -f $sw.Elapsed.TotalSeconds, $alive.Count)
        $killTime = (Get-Date).AddSeconds(-$sw.Elapsed.TotalSeconds)
        Start-Sleep -Seconds 3
        $lock = Join-Path $tempUdf 'EBWebView\lockfile'
        Note "EBWebView\lockfile 仍存在＝$(Test-Path $lock)"
        Get-ChildItem $tempUdf -Recurse -Force -ErrorAction SilentlyContinue |
            Where-Object { $_.LastWriteTime -ge $killTime } |
            ForEach-Object { Note ("  終止後寫入 {0} {1}" -f $_.LastWriteTime.ToString('HH:mm:ss.fff'), $_.FullName.Substring($tempUdf.Length)) }
    }

    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt $ObserveSec -and -not $hostProc.HasExited) { Start-Sleep -Milliseconds 500 }
    if ($hostProc.HasExited) {
        Note ("宿主已結束 exitCode=$($hostProc.ExitCode) (0x{0:X8}) 於終止後 {1:N1}s" -f $hostProc.ExitCode, $sw.Elapsed.TotalSeconds)
    } else {
        Note "宿主存活（觀察 ${ObserveSec}s）"
        $after = Get-HostDescendants
        foreach ($d in $after) { Note "  child(after) pid=$($d.ProcessId) type=$(Get-ProcType $d)" }
    }
} catch {
    Note "腳本錯誤：$($_.Exception.Message)"
} finally {
    # 只停自己啟動的行程與其子孫（Stop-ProcessTree）；-OpenSettings 的第二個行程一律停掉——
    # 逾時沒結束（例如首個宿主恰好結束、它成了 Primary）時會殘留一個帶暫存環境的 fc-host。
    if ($second) { [void](Stop-ProcessTree -Process $second) }
    if ($hostProc) { [void](Stop-ProcessTree -Process $hostProc) }
    Start-Sleep -Seconds 2
    # 清掉本次宿主殘留的 WebView2 子孫行程：只看本次記下的 PID 與命令列含本次暫存目錄者
    $leaf = Split-Path $tempRoot -Leaf
    Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" | Where-Object {
        ($knownDesc -contains [int]$_.ProcessId -and $_.CommandLine -and $_.CommandLine.Contains($leaf)) -or
        ($_.CommandLine -and $_.CommandLine.Contains($leaf))
    } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
    $left = @(Restore-FcHostAutostartRegistry $regSnap)
    Note "登錄還原：$(if ($left.Count) { $left -join '; ' } else { '全部還原' })"
    $logDir = Join-Path $tempLocal 'tw.fintools.fc-host\logs'
    $out = Join-Path $OutDir "soak-exit-$Scenario-$stamp.log"
    $sb = New-Object System.Text.StringBuilder
    foreach ($l in $report) { [void]$sb.AppendLine($l) }
    [void]$sb.AppendLine('---- stderr ----')
    if (Test-Path $stderrPath) { [void]$sb.AppendLine((Get-Content $stderrPath -Raw)) }
    [void]$sb.AppendLine('---- stdout ----')
    if (Test-Path $stdoutPath) { [void]$sb.AppendLine((Get-Content $stdoutPath -Raw)) }
    [void]$sb.AppendLine('---- host log ----')
    Get-ChildItem $logDir -Filter 'fc-host.*.log' -ErrorAction SilentlyContinue | ForEach-Object { [void]$sb.AppendLine((Get-Content $_.FullName -Raw)) }
    # 證據去識別：使用者設定檔路徑改寫成 %TEMP% 等字樣（repo 公開，見 EvidenceLog.psm1）。
    [IO.File]::WriteAllText($out, (ConvertTo-EvidenceText $sb.ToString()), [Text.UTF8Encoding]::new($false))
    Write-Host "證據：$out"
    Start-Sleep -Seconds 1
    Remove-Item -Recurse -Force $tempRoot -ErrorAction SilentlyContinue
    if (Test-Path $tempRoot) { Write-Host "暫存目錄未能刪除：$tempRoot" }
}
