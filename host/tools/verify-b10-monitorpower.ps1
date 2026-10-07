<#
.SYNOPSIS
    代驗 human-checklist.md B10「人工項目 2：顯示器關閉」（task 5.5 自動暫停）。以文件化的
    SendMessage(HWND_BROADCAST, WM_SYSCOMMAND, SC_MONITORPOWER, 2/-1) 關閉／嘗試喚醒顯示器，
    查 gatekeeper.log 有沒有 EVENT power-setting-change signal=DisplayOff(true)/(false)。
    全程不做任何鍵盤／滑鼠／觸控注入（WM_SYSCOMMAND 是系統訊息，不是輸入注入 API）。

.DESCRIPTION
    **必須有人在場**（memory monitor-off-locks-and-sleeps-unattended）：本機顯示器一關，工作階段
    幾乎同時被鎖定並進入 Modern Standby，程式叫不醒，腳本會凍結到使用者回來（2026-09-29 實例：
    凍結約 10.5 小時、沒有收尾）。所以不帶 -UserPresent 時直接回報 BLOCKED（結束碼 4），不做任何事；
    這項驗收也應排在整批的最後。

    1. 確認帶了 -UserPresent、沒有 LogonUI.exe（鎖定）、沒有 fc-host.exe 在跑（都在 try 之前，
       不符就直接結束，不進入任何清理）。
    2. 暫存 %APPDATA%／%LOCALAPPDATA% 首次啟動 fc-host。
    3. 記下 gatekeeper.log 目前的行數（宿主啟動時就會收到一次目前狀態 DisplayOff(false)，
       之後的判定只看這一行之後的記錄——review B-batch3 high）。
    4. SetThreadExecutionState(ES_CONTINUOUS|ES_SYSTEM_REQUIRED) 防止系統待機（不含
       ES_DISPLAY_REQUIRED，否則顯示器不會關）；結束時以 ES_CONTINUOUS 還原。
    5. SendMessageTimeout(HWND_BROADCAST, WM_SYSCOMMAND, SC_MONITORPOWER, 2) 關閉顯示器
       （文件：Microsoft Learn WM_SYSCOMMAND，SC_MONITORPOWER 的 lParam：-1=開／1=低耗電／
       2=關閉）；用 SendMessageTimeout（非純 SendMessage）避免任何未回應視窗造成腳本卡死。
    6. 等 WaitOffSec 秒（量實際經過時間：遠大於預期＝系統曾待機／凍結，記錄下來），查基準行之後
       是否出現 DisplayOff(true)。
    7. SendMessageTimeout(..., SC_MONITORPOWER, -1) 嘗試喚醒；等 WaitOnSec 秒，查 DisplayOff(true)
       那一行**之後**是否出現 DisplayOff(false)。
    8. 判定：
       - 關閉與之後的開啟都觀察到 → PASS（結束碼 0），停宿主、刪暫存目錄。
       - 只觀察到關閉（-1 喚不醒、喚醒後已鎖定、或期間系統曾待機）→ BLOCKED-PARTIAL（結束碼 3）：
         **不停宿主、不刪暫存目錄**，把 PID／暫存路徑／gatekeeper.log 路徑印在記錄檔供使用者回座
         後補查（不得用滑鼠移動等輸入注入嘗試喚醒）。
       - 沒觀察到關閉 → FAIL（結束碼 1）。

.PARAMETER UserPresent
    明確表示使用者在電腦前、顯示器關閉後會手動喚醒並解鎖。不給就 BLOCKED（結束碼 4）。

.PARAMETER Exe
    fc-host.exe 路徑，預設 host/target/release/fc-host.exe。

.PARAMETER OutDir
    證據輸出目錄，預設 host/tools/evidence。
#>
[CmdletBinding()]
param(
    [switch]$UserPresent,
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\fc-host.exe'),
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [int]$WaitOffSec = 15,
    [int]$WaitOnSec = 5
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\AutostartRegistry.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\ProcessTree.psm1') -Force

Add-Type -Namespace B10 -Name Native -MemberDefinition @'
[DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
public static extern System.IntPtr SendMessageTimeoutW(
    System.IntPtr hWnd, uint Msg, System.IntPtr wParam, System.IntPtr lParam,
    uint fuFlags, uint uTimeout, out System.IntPtr lpdwResult);
[DllImport("kernel32.dll")] public static extern uint SetThreadExecutionState(uint esFlags);
'@

$HWND_BROADCAST = [IntPtr]0xffff
$WM_SYSCOMMAND = 0x0112
$SC_MONITORPOWER = [IntPtr]0xF170
$SMTO_ABORTIFHUNG = 0x0002
$SMTO_NORMAL = 0x0000
# Microsoft Learn SetThreadExecutionState：ES_CONTINUOUS=0x80000000、ES_SYSTEM_REQUIRED=0x00000001。
$ES_CONTINUOUS = [uint32]2147483648
$ES_SYSTEM_REQUIRED = [uint32]1
# 實際等待超過預期這麼多秒，就視為期間系統曾待機／腳本曾凍結。
$StallToleranceSec = 60

function Get-Ts { Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fffK' }
function Test-Locked { [bool](Get-Process -Name LogonUI -ErrorAction SilentlyContinue) }

function Send-MonitorPower([int]$lParamValue, [System.IO.StreamWriter]$Log) {
    # lParamValue: -1=開 / 1=低耗電 / 2=關閉（Microsoft Learn WM_SYSCOMMAND / SC_MONITORPOWER）。
    $lp = [IntPtr]$lParamValue
    $result = [IntPtr]::Zero
    $ret = [B10.Native]::SendMessageTimeoutW($HWND_BROADCAST, $WM_SYSCOMMAND, $SC_MONITORPOWER, $lp,
        ($SMTO_ABORTIFHUNG -bor $SMTO_NORMAL), 5000, [ref]$result)
    $err = [System.Runtime.InteropServices.Marshal]::GetLastWin32Error()
    $Log.WriteLine("$(Get-Ts) SendMessageTimeoutW(HWND_BROADCAST, WM_SYSCOMMAND, SC_MONITORPOWER, $lParamValue) 回傳=$ret lastError=$err")
    Write-Host "SendMessageTimeoutW SC_MONITORPOWER=$lParamValue 回傳=$ret lastError=$err"
}

function Get-DisplayPowerObservation {
    <#
    純函式（tests/VerifyB10B6.Tests.ps1 以 Parser 取出測試）。只看第 StartIndex 行（0 起算）之後的
    記錄：OffIndex＝第一個 DisplayOff(true)；OnIndex＝OffIndex 之後第一個 DisplayOff(false)。
    宿主啟動時的初始通知 DisplayOff(false) 在基準行之前，不會被算成「喚醒」（review B-batch3 high）。
    #>
    param([string[]]$Lines = @(), [int]$StartIndex = 0)
    $offIdx = -1
    $onIdx = -1
    for ($i = [Math]::Max(0, $StartIndex); $i -lt $Lines.Count; $i++) {
        if ($offIdx -lt 0) {
            if ($Lines[$i] -match 'EVENT power-setting-change signal=DisplayOff\(true\)') { $offIdx = $i }
        } elseif ($Lines[$i] -match 'EVENT power-setting-change signal=DisplayOff\(false\)') {
            $onIdx = $i
            break
        }
    }
    [PSCustomObject]@{ HasOff = ($offIdx -ge 0); HasOn = ($onIdx -ge 0); OffIndex = $offIdx; OnIndex = $onIdx }
}

function Get-B10Verdict {
    <#
    純函式。回傳 @{ Verdict = 'PASS'|'BLOCKED-PARTIAL'|'FAIL'; ExitCode; Reason }。
    「之後的開啟」有觀察到才 PASS；只有關閉時一律 BLOCKED-PARTIAL（喚醒後鎖定、系統曾待機都在這裡
    說明原因），不繼續判定成 PASS／FAIL。
    #>
    param([bool]$HasOff, [bool]$HasOn, [bool]$LockedAfterWake = $false, [bool]$Stalled = $false)
    if ($HasOff -and $HasOn) { return [PSCustomObject]@{ Verdict = 'PASS'; ExitCode = 0; Reason = 'DisplayOff(true) 與其後的 DisplayOff(false) 皆已觀察到' } }
    if ($HasOff) {
        $why = @()
        if ($Stalled) { $why += '等待期間系統曾待機／腳本曾凍結' }
        if ($LockedAfterWake) { $why += '喚醒後工作階段已鎖定' }
        if ($why.Count -eq 0) { $why += '-1 未能喚醒或事件未寫入' }
        return [PSCustomObject]@{ Verdict = 'BLOCKED-PARTIAL'; ExitCode = 3; Reason = "DisplayOff(true) 已確認；$($why -join '、')" }
    }
    return [PSCustomObject]@{ Verdict = 'FAIL'; ExitCode = 1; Reason = '基準行之後未觀察到 DisplayOff(true)，SC_MONITORPOWER 訊息可能未被系統接受' }
}

function Read-LinesOrEmpty([string]$Path) {
    if (Test-Path -LiteralPath $Path) { return @(Get-Content -LiteralPath $Path) }
    return @()
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$log = New-EvidenceWriter (Join-Path $OutDir 'B10-monitorpower-log.log')
$log.AutoFlush = $true
function Wl([string]$s) { $line = "$(Get-Ts) $s"; $log.WriteLine($line); Write-Host $line }

# 前置防呆一律在任何 try／finally 之前檢查並直接結束（fix F4 共通規則）：原本「已有 fc-host」
# 在 try 內 throw，finally 會以行程名稱把既有的宿主全部砍掉（review B-batch3 high）。
if (-not $UserPresent) {
    Wl 'BLOCKED-UNATTENDED：本項會關閉顯示器，本機隨即鎖定並待機、程式叫不醒；必須有人在場時以 -UserPresent 執行（建議排在整批最後）。未做任何動作。'
    $log.Close()
    exit 4
}
if (Test-Locked) { Wl 'BLOCKED：LogonUI.exe 在跑（工作階段鎖定），已停止。'; $log.Close(); exit 2 }
if (Get-Process -Name fc-host -ErrorAction SilentlyContinue) { Wl '已有 fc-host 在執行，請先手動結束再重跑本腳本（本腳本不會停止它）。'; $log.Close(); exit 1 }
if (-not (Test-Path $Exe)) { Wl "找不到 $Exe，先在 host/ 執行 cargo build --release"; $log.Close(); exit 1 }
$Exe = (Resolve-Path $Exe).Path

$regSnap = @(Save-FcHostAutostartRegistry)
try { Wl "開機自啟登錄快照：$(($regSnap | ForEach-Object { "$($_.Name)@$(Split-Path $_.Key -Leaf) Exists=$($_.Exists)" }) -join '; ')" } catch { }
$exitCode = 0
$proc = $null
$tempRoot = $null
$keepRunning = $false
$execStateSet = $false

try {
    $tempRoot = Join-Path ([IO.Path]::GetTempPath()) ("fc-host-B10-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
    $tempAppData = Join-Path $tempRoot 'Roaming'
    $tempLocalAppData = Join-Path $tempRoot 'Local'
    New-Item -ItemType Directory -Force -Path $tempAppData, $tempLocalAppData | Out-Null
    Wl "暫存 APPDATA=$tempAppData LOCALAPPDATA=$tempLocalAppData"

    $oldAppData = $env:APPDATA
    $oldLocalAppData = $env:LOCALAPPDATA
    try {
        $env:APPDATA = $tempAppData
        $env:LOCALAPPDATA = $tempLocalAppData
        $proc = Start-Process -FilePath $Exe -PassThru
        Wl "啟動 fc-host pid=$($proc.Id)"
    } finally {
        $env:APPDATA = $oldAppData
        $env:LOCALAPPDATA = $oldLocalAppData
    }
    Start-Sleep -Seconds 3

    $gkLog = Join-Path (Join-Path $tempAppData 'tw.fintools.fc-host') 'gatekeeper.log'
    Wl "gatekeeper.log 路徑=$gkLog"

    if (Test-Locked) { Wl 'BLOCKED：關閉顯示器前偵測到鎖定，停止（本步驟非輸入注入，但仍照全域規則先查一次）'; throw '偵測到鎖定，停止' }

    # 判定基準：只看「關閉顯示器之前」這一刻以後的記錄（宿主啟動時的初始 DisplayOff(false) 不算）。
    $baseline = @(Read-LinesOrEmpty $gkLog).Count
    Wl "gatekeeper.log 基準行數=$baseline（之後的記錄才納入判定）"

    # 防待機：旗標先設、再呼叫（fix F4 共通規則：需要還原的旗標在改之前設好）。
    $execStateSet = $true
    $prevState = [B10.Native]::SetThreadExecutionState($ES_CONTINUOUS -bor $ES_SYSTEM_REQUIRED)
    Wl ('SetThreadExecutionState(ES_CONTINUOUS|ES_SYSTEM_REQUIRED) 前一狀態=0x{0:X}（0＝呼叫失敗）' -f $prevState)
    if ($prevState -eq 0) { throw 'SetThreadExecutionState 失敗，無法防止系統待機，中止（不關閉顯示器）' }

    Wl '--- 關閉顯示器 ---'
    $sw = [Diagnostics.Stopwatch]::StartNew()
    Send-MonitorPower 2 $log
    Start-Sleep -Seconds $WaitOffSec
    $offElapsed = $sw.Elapsed.TotalSeconds
    $stalled = $offElapsed -gt ($WaitOffSec + $StallToleranceSec)
    Wl ("關閉後等待實際經過 {0:N1} 秒（預期約 {1} 秒）；系統曾待機／凍結={2}" -f $offElapsed, $WaitOffSec, $stalled)

    $obs1 = Get-DisplayPowerObservation -Lines (Read-LinesOrEmpty $gkLog) -StartIndex $baseline
    Wl "基準行之後是否出現 DisplayOff(true)=$($obs1.HasOff)"

    Wl '--- 嘗試喚醒顯示器 ---'
    Send-MonitorPower (-1) $log
    Start-Sleep -Seconds $WaitOnSec

    $lines2 = Read-LinesOrEmpty $gkLog
    $obs = Get-DisplayPowerObservation -Lines $lines2 -StartIndex $baseline
    $lockedAfterWake = Test-Locked
    Wl "DisplayOff(true) 之後是否出現 DisplayOff(false)=$($obs.HasOn)；喚醒後鎖定=$lockedAfterWake"
    $related = @($lines2 | Select-Object -Skip $baseline | Where-Object { $_ -match 'DisplayOff|Locked' })
    if ($related.Count -gt 0) { Wl "基準行之後的相關行：$($related -join '; ')" }

    $v = Get-B10Verdict -HasOff $obs.HasOff -HasOn $obs.HasOn -LockedAfterWake $lockedAfterWake -Stalled $stalled
    Wl "=== 結論：$($v.Verdict)（$($v.Reason)） ==="
    $exitCode = $v.ExitCode
    if ($v.Verdict -eq 'BLOCKED-PARTIAL') {
        Wl '依規則不得用輸入注入嘗試叫醒；保留宿主待使用者回座後補查。'
        Wl "!!! 保留執行中的宿主：pid=$($proc.Id) 暫存目錄=$tempRoot gatekeeper.log=$gkLog"
        $keepRunning = $true
    }
} catch {
    Wl "例外：$_"
    $exitCode = 1
} finally {
    if ($execStateSet) {
        $restored = [B10.Native]::SetThreadExecutionState($ES_CONTINUOUS)
        try { Wl ('SetThreadExecutionState(ES_CONTINUOUS) 還原：回傳前一狀態=0x{0:X}（0＝失敗）' -f $restored) } catch { }
        if ($restored -eq 0) { $exitCode = 1 }
    }
    if (-not $keepRunning) {
        try {
            # 只停本腳本啟動的宿主與其子孫，不以行程名稱停止（fix F4 共通規則）。
            if ($proc) { [void](Stop-ProcessTree -Process $proc); Wl "已停止本腳本啟動的 fc-host pid=$($proc.Id) 及其子孫" }
        } catch {}
        try {
            if ($tempRoot -and (Test-Path $tempRoot)) { Remove-Item -Recurse -Force $tempRoot -ErrorAction SilentlyContinue; Wl "已刪暫存目錄 $tempRoot" }
        } catch {}
    } else {
        Wl '依 BLOCKED-PARTIAL 規則：保留 fc-host 行程與暫存目錄，不清理。'
    }
    $regLeft = @(Restore-FcHostAutostartRegistry $regSnap)
    if ($regLeft.Count -gt 0) { Write-Warning "開機自啟登錄未還原：$($regLeft -join '; ')" }
    try { Wl "開機自啟登錄還原：未還原 $($regLeft.Count) 項$(if ($regLeft.Count) { '：' + ($regLeft -join '; ') })" } catch { }
    if ($regLeft.Count -gt 0) { $exitCode = 1 }
    $log.Close()
}
exit $exitCode
