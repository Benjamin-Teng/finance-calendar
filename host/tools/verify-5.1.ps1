<#
.SYNOPSIS
    Task 5.1 驗收驅動腳本（系統匣選單與單一執行個體；design.md D12、
    specs/widget-host-lifecycle「單一執行個體」兩個 Scenario）：驗證兩種重複啟動——
    手動重複啟動（開設定視窗）與自動化重複啟動（`--autostart`／`--restarted`，靜默結束）。
    只讀行程／視窗列舉，**不注入任何輸入**（不送按鍵、不點擊、不截圖），鎖定時也可跑。

.DESCRIPTION
    步驟：
      1. 確認沒有 fc-host 在跑。
      2. 以暫存目錄同時當 %APPDATA%（=> 首次啟動）與 %LOCALAPPDATA%（比照
         verify-5.8.ps1 的隔離作法），完全不碰使用者真正的設定檔或
         `D:\finance-calendar`。
      3. 啟動第一個執行個體（手動、無參數），等 `clock` 小工具視窗出現＝視為就緒。
      4. 自動化重複啟動 ×2（`--autostart`、`--restarted` 各跑一次）：啟動第二個行程，
         等它結束並記錄結束碼；結束後數秒內輪詢第一個執行個體的視窗清單，驗證**沒有**新出現
         標題為「財經桌布設定」的視窗，且小工具視窗數量不變（沒有第二組小工具）。
      5. 手動重複啟動：啟動第二個行程（無參數），等它結束並記錄結束碼；驗證第一個執行個體
         的視窗清單**出現**標題為「財經桌布設定」的視窗（single-instance 回呼開啟／聚焦
         `open_settings_window`），且小工具視窗數量仍不變。
      6. 結束第一個執行個體、刪除暫存目錄。

.PARAMETER Exe
    fc-host.exe 路徑，預設 host/target/release/fc-host.exe。

.PARAMETER OutDir
    證據輸出目錄，預設 host/tools/evidence。
#>
[CmdletBinding()]
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\fc-host.exe'),
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence')
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\AutostartRegistry.psm1') -Force

Add-Type -Namespace V51 -Name Native -MemberDefinition @'
[DllImport("user32.dll")] public static extern System.IntPtr GetTopWindow(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern System.IntPtr GetWindow(System.IntPtr hWnd, uint cmd);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr hWnd, out uint pid);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(System.IntPtr hWnd, System.Text.StringBuilder s, int n);
'@

function Get-Ts { Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fffK' }
function Test-Locked { [bool](Get-Process -Name LogonUI, LockApp -ErrorAction SilentlyContinue) }

function Get-HostWindowTitles([int]$ProcId) {
    $titles = @()
    $h = [V51.Native]::GetTopWindow([IntPtr]::Zero)
    while ($h -ne [IntPtr]::Zero) {
        $p = 0
        [void][V51.Native]::GetWindowThreadProcessId($h, [ref]$p)
        if ($p -eq $ProcId -and [V51.Native]::IsWindowVisible($h)) {
            $sb = New-Object System.Text.StringBuilder 256
            [void][V51.Native]::GetWindowText($h, $sb, 256)
            if ($sb.Length -gt 0) { $titles += $sb.ToString() }
        }
        $h = [V51.Native]::GetWindow($h, 2)
    }
    return $titles
}

function Wait-WindowTitles([int]$ProcId, [scriptblock]$Cond, [int]$TimeoutSec = 20) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt $TimeoutSec) {
        $titles = Get-HostWindowTitles $ProcId
        if (& $Cond $titles) { return $true }
        Start-Sleep -Milliseconds 300
    }
    return $false
}

# ── 1. 前置檢查 ─────────────────────────────────────────────────────────────────
if (Get-Process -Name fc-host -ErrorAction SilentlyContinue) {
    throw '已有 fc-host 在執行，請先結束（避免與本次暫存實例的行程／視窗列舉互相干擾）。'
}
$Exe = (Resolve-Path $Exe).Path
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$logPath = Join-Path $OutDir '5.1-driver.log'
$sumPath = Join-Path $OutDir '5.1-summary.log'
$log = New-EvidenceWriter $logPath
$log.AutoFlush = $true
$log.WriteLine("# verify-5.1.ps1 start=$(Get-Ts) exe=$Exe locked=$([int](Test-Locked))")

# ── 2. 暫存 %APPDATA%／%LOCALAPPDATA%（不碰使用者真正的設定檔／資料目錄／記錄檔） ─────────
$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ("fc-host-5.1-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
$tempAppData = Join-Path $tempRoot 'Roaming'
$tempLocalAppData = Join-Path $tempRoot 'Local'
New-Item -ItemType Directory -Force -Path $tempAppData, $tempLocalAppData | Out-Null
$log.WriteLine("# APPDATA=$tempAppData（首次啟動）LOCALAPPDATA=$tempLocalAppData")

$regSnap = @(Save-FcHostAutostartRegistry)
$log.WriteLine("# $(Get-Ts) 開機自啟登錄快照：$(($regSnap | ForEach-Object { "$($_.Name)@$(Split-Path $_.Key -Leaf) Exists=$($_.Exists)" }) -join '; ')")
$results = [ordered]@{}
$hostProc = $null
$oldAppData = $env:APPDATA
$oldLocalAppData = $env:LOCALAPPDATA
try {
    # ── 3. 啟動第一個執行個體（手動、無參數） ────────────────────────────────────────
    try {
        $env:APPDATA = $tempAppData
        $env:LOCALAPPDATA = $tempLocalAppData
        $hostProc = Start-Process -FilePath $Exe -PassThru
    } finally {
        $env:APPDATA = $oldAppData
        $env:LOCALAPPDATA = $oldLocalAppData
    }
    $hostPid = $hostProc.Id
    $log.WriteLine("# $(Get-Ts) 第一個執行個體 pid=$hostPid")

    # 等到 clock／macro／fixed／dynamic 四個財經小工具視窗都出現才算「就緒」──剛啟動時視窗是
    # 陸續建立的，只等 clock 一個會在其餘視窗建立完成前就把基準視窗數量抓少了（本腳本初版的
    # race condition，實測抓到 2 個就誤判成基準值，見 task-5.1-report.md）。`quotes`（行情條）
    # 不列入等待條件：暫存資料目錄從未寫入任何資料，AGENTS.md 既有行為「無資料自動隱藏」
    # （widget-data-feed／widgets.rs `apply_report_content`：頁面回報無內容即隱藏）使它在無資料時
    # 這個測試環境下持續保持隱藏，不會出現在可見視窗清單——這是既有、正確的行為，不是本
    # task 的迴歸。
    $readyIds = @('fc-host clock', 'fc-host macro', 'fc-host fixed', 'fc-host dynamic')
    $ok = Wait-WindowTitles $hostPid { param($t) ($readyIds | Where-Object { $_ -notin $t }).Count -eq 0 } 20

    # `quotes`（行情條）建立當下先以格子矩形顯示，等頁面第一次 report_content 回報無內容後才
    # 被隱藏（task 7.3；本測試從未寫入資料，預期收斂為隱藏）——用「連續兩次快照相同」近似
    # 判斷視窗清單已穩定，避免在這個轉場中間點抓到不具代表性的基準視窗數量。
    $baselineTitles = $null
    $stableSw = [Diagnostics.Stopwatch]::StartNew()
    while ($stableSw.Elapsed.TotalSeconds -lt 10) {
        $t1 = (Get-HostWindowTitles $hostPid) | Sort-Object
        Start-Sleep -Milliseconds 500
        $t2 = (Get-HostWindowTitles $hostPid) | Sort-Object
        if (($t1 -join '|') -eq ($t2 -join '|')) { $baselineTitles = $t2; break }
    }
    if (-not $baselineTitles) { $baselineTitles = Get-HostWindowTitles $hostPid }
    $log.WriteLine("## $(Get-Ts) 首次啟動：四個財經小工具就緒＝$ok，視窗清單已穩定（$([math]::Round($stableSw.Elapsed.TotalSeconds, 1))s，清單：$($baselineTitles -join ', '))")
    $results['首次啟動成功（clock／macro／fixed／dynamic 視窗出現）'] = $ok
    $baselineCount = $baselineTitles.Count

    function Invoke-SecondInstance([string[]]$Argv, [string]$Label) {
        # 第二個行程沿用同一份暫存 %APPDATA%／%LOCALAPPDATA%（single-instance 用的具名
        # mutex／事件視窗以 `tauri.conf.json` 的 `identifier` 命名，不吃這兩個 env，但仍照
        # 慣例把它們指到暫存路徑，避免任何非預期的行為需要讀取這兩個目錄）。
        $before = Get-Date
        $env:APPDATA = $script:tempAppData
        $env:LOCALAPPDATA = $script:tempLocalAppData
        $p = $null
        try {
            if ($Argv.Count -gt 0) {
                $p = Start-Process -FilePath $script:Exe -ArgumentList $Argv -PassThru
            } else {
                $p = Start-Process -FilePath $script:Exe -PassThru
            }
        } finally {
            $env:APPDATA = $script:oldAppData
            $env:LOCALAPPDATA = $script:oldLocalAppData
        }
        $exited = $p.WaitForExit(15000)
        $elapsed = [math]::Round(((Get-Date) - $before).TotalSeconds, 2)
        $exitCode = if ($exited) { $p.ExitCode } else { $null }
        $script:log.WriteLine("## $(Get-Ts) [$Label] 第二個行程 pid=$($p.Id) argv=[$($Argv -join ' ')] 結束＝$exited（${elapsed}s）exitCode=$exitCode")
        if (-not $exited) {
            Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
        }
        return [pscustomobject]@{ Exited = $exited; ExitCode = $exitCode }
    }

    # ── 4. 自動化重複啟動：--autostart／--restarted 各跑一次，第一個執行個體不應開設定視窗 ──
    foreach ($flag in @('--autostart', '--restarted')) {
        $r = Invoke-SecondInstance -Argv @($flag) -Label "自動化($flag)"
        $results["[$flag] 第二個行程結束（結束碼 0）"] = ($r.Exited -and $r.ExitCode -eq 0)

        # 給既有執行個體幾秒鐘的反應時間；預期它「什麼都不做」，故等滿再判斷一次。
        Start-Sleep -Seconds 3
        $titles = Get-HostWindowTitles $hostPid
        $noSettings = -not ($titles | Where-Object { $_ -eq '財經桌布設定' })
        $sameWidgetCount = ($titles.Count -eq $baselineCount)
        $log.WriteLine("## $(Get-Ts) [$flag] 既有執行個體視窗清單：$($titles -join ', ')")
        $results["[$flag] 既有執行個體未開設定視窗"] = $noSettings
        $results["[$flag] 小工具視窗數量不變（無第二組小工具，$baselineCount 個）"] = $sameWidgetCount
    }

    # ── 5. 手動重複啟動：第一個執行個體應開啟設定視窗 ──────────────────────────────────
    $r = Invoke-SecondInstance -Argv @() -Label '手動'
    $results['[手動] 第二個行程結束（結束碼 0）'] = ($r.Exited -and $r.ExitCode -eq 0)

    $settingsSeen = Wait-WindowTitles $hostPid { param($t) ($t | Where-Object { $_ -eq '財經桌布設定' }) } 15
    $titles = Get-HostWindowTitles $hostPid
    $sameWidgetCount = (($titles | Where-Object { $_ -ne '財經桌布設定' }).Count -eq $baselineCount)
    $log.WriteLine("## $(Get-Ts) [手動] 既有執行個體視窗清單：$($titles -join ', ')")
    $results['[手動] 既有執行個體開啟設定視窗（標題「財經桌布設定」）'] = $settingsSeen
    $results['[手動] 小工具視窗數量不變（無第二組小工具）'] = $sameWidgetCount

    $log.WriteLine("## 截圖：略過（不涉及視覺驗收；鎖定狀態=$([int](Test-Locked))）")
}
finally {
    if ($hostProc -and -not $hostProc.HasExited) { Stop-Process -Id $hostProc.Id -Force -ErrorAction SilentlyContinue }
    $regLeft = @(Restore-FcHostAutostartRegistry $regSnap)
    if ($regLeft.Count -gt 0) { Write-Warning "開機自啟登錄未還原：$($regLeft -join '; ')" }
    $log.WriteLine("# $(Get-Ts) 開機自啟登錄還原：未還原 $($regLeft.Count) 項$(if ($regLeft.Count) { '：' + ($regLeft -join '; ') })")
    $results['開機自啟登錄已還原（Run／StartupApproved 的 fc-host）'] = ($regLeft.Count -eq 0)
    $log.WriteLine("# $(Get-Ts) 第一個執行個體已結束")
    $log.Close()
}

$sum = New-EvidenceWriter $sumPath
$sum.WriteLine("# verify-5.1.ps1 summary $(Get-Ts)")
foreach ($k in $results.Keys) { $sum.WriteLine("$(if ($results[$k]) { 'PASS' } else { 'FAIL' })  $k") }
$sum.Close()
Get-Content $sumPath

Remove-Item -Recurse -Force $tempRoot -ErrorAction SilentlyContinue

$failed = @($results.Values | Where-Object { -not $_ })
if ($failed.Count -gt 0) { exit 1 }
exit 0
