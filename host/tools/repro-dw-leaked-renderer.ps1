<#
.SYNOPSIS
    bug-leaked-renderer 重現：以 --self-test-render 連續渲染，量每次渲染那組 WebView2 browser 行程的
    結束時間、下一次開始時前一個是否仍存活（重疊）、以及殘留（最後仍存活、宿主結束後仍存活）。

.DESCRIPTION
    隔離同 measure-dw-4.5.ps1（%APPDATA%、%LOCALAPPDATA%、WEBVIEW2_USER_DATA_FOLDER 只對啟動的宿主指向
    %TEMP%\fc-repro-leak-<時間>）。宿主預設主題為「不接管」，協調迴圈不設桌布；self-test 只渲染、
    寫進隔離資料夾的輸出圖，不碰使用者桌布。

    宿主參數：--gap-ms <GapMs>（每次渲染返回後等多久開始下一次；正式排程同一輪兩台螢幕約 1000 ms），
    --count <Count>、--tail-secs <TailSec>（最後一次之後再等，讓正常的 browser 行程都結束）。

    宿主記錄（self_test_render.rs）：PREV-BROWSER（下一次開始時前一個是否仍存活）、BROWSER-EXIT／
    BROWSER-NOT-EXITED（返回後多久結束，上限 60 秒）、BROWSER-REUSED（新的渲染拿到同一個 browser
    行程）、BROWSERS（尾段後仍存活者＝殘留）。

    宿主結束後，本稿再列出命令列含隔離根目錄的 msedgewebview2（＝宿主結束後仍存活的殘留），記下
    執行緒數、各執行緒狀態，然後只結束這些行程（它們的 --user-data-dir 在本稿建立的隔離資料夾內，
    父行程是本稿啟動的宿主）。

    產出 evidence/dw-bug-renderer-<Tag>-{driver,render,summary}.log。結束碼：0 無殘留；1 有殘留或
    宿主失敗；2 參數／環境錯誤；3 BLOCKED（鎖定中）。

.EXAMPLE
    cd host; cargo build --release --features self-test-ipc; cd ..
    pwsh -File host/tools/repro-dw-leaked-renderer.ps1 -Tag gap1000 -Count 300 -GapMs 1000
    pwsh -File host/tools/repro-dw-leaked-renderer.ps1 -Tag gap3000 -Count 300 -GapMs 3000
#>
[CmdletBinding()]
param(
    [string]$Tag = 'run',
    [int]$Count = 100,
    [int]$GapMs = 1000,
    [int]$WarmupSec = 10,
    [int]$TailSec = 20,
    [int]$Width = 2560,
    [int]$Height = 1600,
    # 偶數次改用的尺寸（例如 '3840x2160'；空＝不換）與每兩次之間的閒置（毫秒；0＝不用）。
    [string]$AltSize = '',
    [int]$PairIdleMs = 0,
    # 故障注入：第幾次渲染返回後立刻暫停它的 browser 行程（模擬卡在結束流程），逗號分隔（例如 5,10,15）。
    # 用字串而非 [int[]]：pwsh -File 會把 "5,10,15" 當成千分位、變成 51015。空＝不注入。
    [string]$SuspendAt = '',
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [string]$ExePath = (Join-Path $PSScriptRoot '..\target\release\fc-host.exe')
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'lib\ProcessTree.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force

$ExePath = (Resolve-Path -LiteralPath $ExePath).Path
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$OutDir = (Resolve-Path -LiteralPath $OutDir).Path
$prefix = Join-Path $OutDir "dw-bug-renderer-$Tag"
$driverLog = "$prefix-driver.log"
$renderLog = "$prefix-render.log"
$summaryPath = "$prefix-summary.log"
foreach ($p in @($driverLog, $renderLog, $summaryPath)) { if (Test-Path -LiteralPath $p) { Remove-Item -LiteralPath $p -Force } }
Set-Content -LiteralPath $driverLog -Value @() -Encoding utf8

function Write-Log([string]$Message) {
    $line = '{0} {1}' -f (Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fff'), $Message
    Add-Content -LiteralPath $driverLog -Value (ConvertTo-EvidenceText $line) -Encoding utf8
    Write-Host $line
}

# ── 前置檢查 ───────────────────────────────────────────────────────────────────────────
if (Get-Process -Name LogonUI -ErrorAction SilentlyContinue) {
    Write-Log 'BLOCKED 工作階段鎖定中（LogonUI.exe 在跑），不啟動。'
    exit 3
}
$running = @(Get-Process -Name fc-host -ErrorAction SilentlyContinue)
if ($running.Count -gt 0) {
    Write-Log ("ABORT 已有 fc-host 在跑（PID {0}），不啟動、也不結束它。" -f (($running | ForEach-Object Id) -join ','))
    exit 2
}
$exeText = [System.Text.Encoding]::Latin1.GetString([System.IO.File]::ReadAllBytes($ExePath))
if (-not $exeText.Contains('FC_HOST_SELF_TEST_RENDER_BUILD')) {
    Write-Log "ABORT $ExePath 不含 self-test-render 建置標記（請以 cargo build --release --features self-test-ipc 建置）。"
    exit 2
}
$exeText = $null
Write-Log "PRECHECK ok exe=$ExePath"

# ── 隔離 ───────────────────────────────────────────────────────────────────────────────
$isoRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("fc-repro-leak-{0}" -f (Get-Date -Format 'yyyyMMdd-HHmmss'))
$isoAppData = Join-Path $isoRoot 'appdata'
$isoLocal = Join-Path $isoRoot 'localappdata'
$isoUdf = Join-Path $isoRoot 'webview2'
$isoData = Join-Path $isoLocal 'tw.fintools.fc-host\data'
foreach ($d in @($isoAppData, $isoLocal, $isoUdf, $isoData)) { New-Item -ItemType Directory -Force -Path $d | Out-Null }
$isoRoot = (Resolve-Path -LiteralPath $isoRoot).Path
Copy-Item -LiteralPath (Join-Path $PSScriptRoot '..\ui\fixtures\tw-events.json') -Destination (Join-Path $isoData 'tw_events.json')
Write-Log "ISOLATION root=$isoRoot"

$hostArgs = @('--self-test-render', '--render-log', $renderLog, '--count', $Count, '--gap-ms', $GapMs,
    '--warmup-secs', $WarmupSec, '--tail-secs', $TailSec, '--width', $Width, '--height', $Height)
if ($AltSize) { $hostArgs += @('--alt-size', $AltSize) }
if ($PairIdleMs -gt 0) { $hostArgs += @('--pair-idle-ms', $PairIdleMs) }
$hostArgs = $hostArgs | ForEach-Object { $s = [string]$_; if ($s -match '\s') { '"' + $s + '"' } else { $s } }
# 每次約 1 秒＋間隔；上限另加 5 分鐘。
$timeoutSec = $WarmupSec + $TailSec + [int][Math]::Ceiling($Count * (2.5 + $GapMs / 1000.0 + $PairIdleMs / 2000.0)) + 300

$saved = @{ APPDATA = $env:APPDATA; LOCALAPPDATA = $env:LOCALAPPDATA; WEBVIEW2_USER_DATA_FOLDER = $env:WEBVIEW2_USER_DATA_FOLDER }
try {
    $env:APPDATA = $isoAppData
    $env:LOCALAPPDATA = $isoLocal
    $env:WEBVIEW2_USER_DATA_FOLDER = $isoUdf
    $proc = Start-Process -FilePath $ExePath -ArgumentList $hostArgs -PassThru
} finally {
    $env:APPDATA = $saved.APPDATA
    $env:LOCALAPPDATA = $saved.LOCALAPPDATA
    $env:WEBVIEW2_USER_DATA_FOLDER = $saved.WEBVIEW2_USER_DATA_FOLDER
}
$hostPid = $proc.Id
Write-Log "START pid=$hostPid count=$Count gapMs=$GapMs timeoutSec=$timeoutSec args=$($hostArgs -join ' ')"

$deadline = (Get-Date).AddSeconds($timeoutSec)
$suspendList = @($SuspendAt -split '[,\s]+' | Where-Object { $_ } | ForEach-Object { [int]$_ })
if ($suspendList.Count -gt 0) {
    # 故障注入：第 N 次渲染返回（RENDER-OK n=N … browser_pid=Some(P)）後立刻暫停那個 browser 行程，模擬
    # 「卡在結束流程、永不退出」。只動本稿啟動的宿主自己回報、且父行程確實是它的 msedgewebview2。
    if (-not ('FcLeakInject' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class FcLeakInject {
    [DllImport("ntdll.dll")] public static extern int NtSuspendProcess(IntPtr h);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern IntPtr OpenProcess(uint access, bool inherit, uint pid);
    [DllImport("kernel32.dll")] public static extern bool CloseHandle(IntPtr h);
}
'@
    }
    $pending = New-Object System.Collections.Generic.HashSet[int]
    foreach ($n in $suspendList) { [void]$pending.Add($n) }
    Write-Log "INJECT-PLAN n=$($suspendList -join ',')"

    $pos = 0L
    $carry = ''
    while (-not $proc.HasExited -and (Get-Date) -lt $deadline) {
        if ($pending.Count -gt 0 -and (Test-Path -LiteralPath $renderLog)) {
            $fs = [System.IO.File]::Open($renderLog, 'Open', 'Read', 'ReadWrite')
            try {
                if ($fs.Length -gt $pos) {
                    [void]$fs.Seek($pos, 'Begin')
                    $buf = New-Object byte[] ($fs.Length - $pos)
                    $read = $fs.Read($buf, 0, $buf.Length)
                    $pos += $read
                    $carry += [System.Text.Encoding]::UTF8.GetString($buf, 0, $read)
                }
            } finally { $fs.Dispose() }
            while ($carry.Contains("`n")) {
                $i = $carry.IndexOf("`n")
                $line = $carry.Substring(0, $i)
                $carry = $carry.Substring($i + 1)
                if ($line -match ' RENDER-OK n=(\d+) id=\d+ browser_pid=Some\((\d+)\)' -and $pending.Contains([int]$Matches[1])) {
                    $n = [int]$Matches[1]; $bpid = [int]$Matches[2]
                    [void]$pending.Remove($n)
                    $result = 'fail'
                    try {
                        $bp = Get-Process -Id $bpid -ErrorAction Stop
                        if ($bp.ProcessName -ne 'msedgewebview2' -or $bp.Parent.Id -ne $hostPid) {
                            $result = "skip（不是本宿主的 msedgewebview2：{0} parent={1}）" -f $bp.ProcessName, $bp.Parent.Id
                        } else {
                            $h = [FcLeakInject]::OpenProcess(0x0800, $false, [uint32]$bpid)
                            if ($h -eq [IntPtr]::Zero) { $result = 'fail（OpenProcess）' }
                            else {
                                $st = [FcLeakInject]::NtSuspendProcess($h)
                                [void][FcLeakInject]::CloseHandle($h)
                                $result = if ($st -eq 0) { 'suspended' } else { 'fail（NTSTATUS {0:X8}）' -f $st }
                            }
                        }
                    } catch { $result = 'fail（行程已結束：來不及）' }
                    Write-Log "INJECT n=$n pid=$bpid $result"
                }
            }
        }
        Start-Sleep -Milliseconds 5
    }
}

$exited = $proc.WaitForExit([int][Math]::Max(1000, ($deadline - (Get-Date)).TotalMilliseconds))
if (-not $exited) {
    Write-Log "TIMEOUT 宿主 $timeoutSec 秒內沒有結束，停止自己啟動的行程樹"
    $stopped = Stop-ProcessTree -Process $proc
    Write-Log "STOPPED $($stopped -join ',')"
    $hostCode = -1
} else {
    $hostCode = $proc.ExitCode
    Write-Log "HOST-EXIT code=$hostCode"
}

# ── 宿主結束後仍存活的渲染 browser 行程（命令列在隔離根目錄內）────────────────────────────────
Start-Sleep -Seconds 3
$rendererUdf = Join-Path $isoUdf 'wallpaper-renderer'
$injected = @(Get-Content -LiteralPath $driverLog -Encoding utf8 | Where-Object { $_ -match " INJECT .* suspended" }).Count
$survivors = @(Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" |
        Where-Object { $_.CommandLine -and $_.CommandLine.IndexOf($isoRoot, [StringComparison]::OrdinalIgnoreCase) -ge 0 })
foreach ($s in $survivors) {
    $isBrowser = $s.CommandLine -notmatch '--type='
    $isRenderer = $s.CommandLine.IndexOf($rendererUdf, [StringComparison]::OrdinalIgnoreCase) -ge 0
    $threads = ''
    try {
        $gp = Get-Process -Id $s.ProcessId -ErrorAction Stop
        $threads = ($gp.Threads | ForEach-Object { '{0}:{1}/{2}' -f $_.Id, $_.ThreadState, $(try { $_.WaitReason } catch { '-' }) }) -join ' '
    } catch { $threads = '（讀不到）' }
    Write-Log ("SURVIVOR pid={0} parent={1} created={2:o} browser={3} renderer={4} threads=[{5}]" -f $s.ProcessId, $s.ParentProcessId, $s.CreationDate, $isBrowser, $isRenderer, $threads)
}
foreach ($s in $survivors) {
    # 只結束本稿隔離資料夾的行程（父＝本稿啟動的宿主，或其子孫）。
    Stop-Process -Id $s.ProcessId -Force -ErrorAction SilentlyContinue
    Write-Log "KILLED-SURVIVOR pid=$($s.ProcessId)"
}
Write-Log "SURVIVORS after host exit: $($survivors.Count)"

# ── 摘要 ───────────────────────────────────────────────────────────────────────────────
$overall = if ($hostCode -eq 0) { 0 } else { 1 }
if (Test-Path -LiteralPath $renderLog) {
    Protect-EvidenceFile -Path $renderLog | Out-Null
    $lines = Get-Content -LiteralPath $renderLog -Encoding utf8
    $prev = @($lines | Where-Object { $_ -match ' PREV-BROWSER ' })
    $prevAlive = @($prev | Where-Object { $_ -match 'alive=true' })
    $exits = @($lines | Where-Object { $_ -match ' BROWSER-EXIT ' } | ForEach-Object { if ($_ -match 'after_return_ms=([\d.]+)') { [double]$Matches[1] } })
    $notExited = @($lines | Where-Object { $_ -match ' BROWSER-NOT-EXITED ' })
    $reused = @($lines | Where-Object { $_ -match ' BROWSER-REUSED ' })
    $okCount = @($lines | Where-Object { $_ -match ' RENDER-OK ' }).Count
    $failCount = @($lines | Where-Object { $_ -match ' RENDER-FAIL ' }).Count
    $leakLine = @($lines | Where-Object { $_ -match ' BROWSERS total=' }) | Select-Object -Last 1
    $sorted = @($exits | Sort-Object)
    $p50 = if ($sorted.Count) { $sorted[[int][Math]::Floor(($sorted.Count - 1) / 2)] } else { $null }
    $p95 = if ($sorted.Count) { $sorted[[int][Math]::Floor(($sorted.Count - 1) * 0.95)] } else { $null }
    $pmax = if ($sorted.Count) { $sorted[-1] } else { $null }
    # 宿主記錄裡「前一個渲染 browser 行程」的處置（修正後才有）：等了多久才結束、被宿主結束、沒結束它。
    $hostLogLines = @(Get-ChildItem -LiteralPath (Join-Path $isoLocal 'tw.fintools.fc-host\logs') -Filter '*.log' -File -ErrorAction SilentlyContinue |
            ForEach-Object { Get-Content -LiteralPath $_.FullName -Encoding utf8 } | Where-Object { $_ -match '前一個渲染 browser 行程|最後一個桌布渲染 browser' })
    $settleWaited = @($hostLogLines | Where-Object { $_ -match '已結束（等了' })
    $settleKilled = @($hostLogLines | Where-Object { $_ -match '已由宿主結束' })
    $settleLeft = @($hostLogLines | Where-Object { $_ -match '不結束它|結束它失敗' })
    $shutdownLine = @($lines | Where-Object { $_ -match ' SHUTDOWN-SETTLE ' }) | Select-Object -Last 1
    $summary = @(
        "TAG $Tag count=$Count gapMs=$GapMs size=${Width}x$Height altSize=$AltSize pairIdleMs=$PairIdleMs",
        "RENDER ok=$okCount fail=$failCount hostExit=$hostCode",
        "OVERLAP 下一次開始時前一個 browser 仍存活：$($prevAlive.Count)/$($prev.Count)",
        ("EXIT 返回後結束耗時 ms：n={0} p50={1} p95={2} max={3}" -f $sorted.Count, $p50, $p95, $pmax),
        "NOT-EXITED 60 秒內沒結束：$($notExited.Count)",
        "REUSED 新渲染拿到前一個 browser 行程：$($reused.Count)",
        "HOST $leakLine",
        "SURVIVORS 宿主結束後仍存活（隔離資料夾內）：$($survivors.Count)",
        "SETTLE 下一次前等到結束（≥10 ms）：$($settleWaited.Count)；宿主結束卡住者：$($settleKilled.Count)；沒結束它：$($settleLeft.Count)",
        "HOST $shutdownLine",
        "INJECT 成功暫停的 browser 行程（故障注入）：$injected"
    ) + @($notExited | ForEach-Object { "DETAIL $_" }) + @($reused | ForEach-Object { "DETAIL $_" }) +
    @($settleKilled + $settleLeft | ForEach-Object { "DETAIL-HOST $_" })
    Set-Content -LiteralPath $summaryPath -Value ($summary | ForEach-Object { ConvertTo-EvidenceText $_ }) -Encoding utf8
    $summary | ForEach-Object { Write-Host $_ }
    if ($notExited.Count -gt 0 -or $survivors.Count -gt 0) { $overall = 1 }
} else {
    Write-Log 'NO-RENDER-LOG 宿主沒有寫出 self-test 記錄'
    $overall = 1
}

try {
    Remove-Item -LiteralPath $isoRoot -Recurse -Force -ErrorAction Stop
    Write-Log "CLEANUP removed $isoRoot"
} catch {
    Write-Log "CLEANUP 未能刪除 $isoRoot：$($_.Exception.Message)"
}
Write-Log ("VERDICT {0}" -f $(if ($overall -eq 0) { 'NO-LEAK' } else { 'LEAK-OR-FAIL' }))
exit $overall
