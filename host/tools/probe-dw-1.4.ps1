<#
.SYNOPSIS
    dynamic-wallpaper task 1.4 渲染成本探針（fc-host --probe-render）的驅動稿：前置檢查、資料隔離、
    依序跑兩種模式、核對 WebView2 使用者資料夾、只收自己啟動的行程樹。

.DESCRIPTION
    前置檢查（任一不過就不啟動）：
      - 工作階段未鎖定（LogonUI.exe 不在跑）→ 否則結束碼 3（BLOCKED）。
      - 沒有任何 fc-host 在跑（宿主是 single-instance）→ 有就停下回報、結束碼 2，絕不結束它。
      - exe 內含探針建置標記 FC_HOST_PROBE_RENDER_BUILD（＝帶 probe-render feature 的建置；
        不帶 feature 的正式版看不到 --probe-render，會照常啟動宿主）→ 否則結束碼 2。

    隔離：每次執行建一個暫存根目錄（%TEMP%\fc-probe-render-<時間>），以它的子目錄覆寫
    APPDATA、LOCALAPPDATA、WEBVIEW2_USER_DATA_FOLDER（只對本稿啟動的子行程生效，結束後還原），
    並以 --webview-data 明確傳給探針（Tauri 預設的 EBWebView 位置不跟 %LOCALAPPDATA% 覆寫走）。
    執行中每秒讀本稿啟動的行程樹中 msedgewebview2.exe 的命令列，記下 --user-data-dir，
    結束後判定是否全部落在暫存根目錄內；並比對真實 %LOCALAPPDATA%\tw.fintools.fc-host 底下
    EBWebView 與 logs 的最後寫入時間前後是否不變。

    收尾：只以 Start-Process 取得的 PID 為根，用 lib\ProcessTree.psm1 停它與子孫；
    絕不以名稱停止 fc-host 或 msedgewebview2。

    -Anchor：探針先建立一個隱藏的錨點 webview（同源小檔案）並保持到結束，模擬正式宿主中小工具
    讓 WebView2 browser 行程常駐。-ControllerHidden：建立後把 WebView2 controller 也設為不可見
    （頁面 document.visibilityState 變成 hidden），量 toBlob 在「頁面自認隱藏」時的行為。

    結束碼：0 兩種模式皆成功且隔離核對通過；1 有失敗；2 參數／環境錯誤；3 BLOCKED。

.EXAMPLE
    cd host; cargo build --release --features probe-render
    pwsh -File host/tools/probe-dw-1.4.ps1
    pwsh -File host/tools/probe-dw-1.4.ps1 -Modes persistent -Runs 3 -Tag smoke
    pwsh -File host/tools/probe-dw-1.4.ps1 -Modes open-close -Anchor -Tag anchor
    pwsh -File host/tools/probe-dw-1.4.ps1 -Modes persistent -ControllerHidden -Tag ctlhidden
    pwsh -File host/tools/probe-dw-1.4.ps1 -Modes persistent-redraw -Tag redraw
#>
[CmdletBinding()]
param(
    [string[]]$Modes = @('open-close', 'persistent'),
    [int]$Runs = 10,
    [string]$Tag = 'run',
    [int]$TimeoutSecs = 900,
    [switch]$Anchor,
    [switch]$ControllerHidden,
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [string]$ExePath = (Join-Path $PSScriptRoot '..\target\release\fc-host.exe')
)

$ErrorActionPreference = 'Stop'
# pwsh -File 會把 "a,b" 當成單一字串傳進來，這裡統一拆開。
$Modes = @($Modes | ForEach-Object { $_ -split ',' } | ForEach-Object { $_.Trim() } | Where-Object { $_ })
foreach ($m in $Modes) {
    if ($m -notin @('open-close', 'persistent', 'persistent-redraw')) { Write-Host "未知模式：$m" -ForegroundColor Red; exit 2 }
}
Import-Module (Join-Path $PSScriptRoot 'lib\ProcessTree.psm1') -Force
# repo 公開：證據不得留下使用者設定檔路徑（README「lib/EvidenceLog.psm1」）。驅動記錄逐行改寫；
# 探針自己寫的 .log／.csv 在每個模式結束後以 Protect-EvidenceFile 就地改寫。
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force

$ExePath = (Resolve-Path -LiteralPath $ExePath).Path
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$OutDir = (Resolve-Path -LiteralPath $OutDir).Path
# 驅動記錄檔名帶模式清單：同一個 -Tag 分次跑不同模式時不會互相覆蓋。
$runLog = Join-Path $OutDir ("dw-1.4-{0}-{1}-driver.log" -f $Tag, ($Modes -join '+'))
Set-Content -LiteralPath $runLog -Value @() -Encoding utf8

function Write-Log([string]$Message) {
    $line = '{0} {1}' -f (Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fff'), $Message
    Add-Content -LiteralPath $runLog -Value (ConvertTo-EvidenceText $line) -Encoding utf8
    Write-Host $line
}

# ── 前置檢查 ───────────────────────────────────────────────────────────────────────────
if (Get-Process -Name LogonUI -ErrorAction SilentlyContinue) {
    Write-Log 'BLOCKED 工作階段鎖定中（LogonUI.exe 在跑），不啟動探針。'
    exit 3
}
$running = @(Get-Process -Name fc-host -ErrorAction SilentlyContinue)
if ($running.Count -gt 0) {
    Write-Log ("ABORT 已有 fc-host 在跑（PID {0}），不啟動探針、也不結束它。" -f (($running | ForEach-Object Id) -join ','))
    exit 2
}
$exeText = [System.Text.Encoding]::Latin1.GetString([System.IO.File]::ReadAllBytes($ExePath))
if (-not $exeText.Contains('FC_HOST_PROBE_RENDER_BUILD')) {
    Write-Log "ABORT $ExePath 不含探針建置標記（請以 cargo build --release --features probe-render 建置）。"
    exit 2
}
$exeText = $null
Write-Log "PRECHECK ok exe=$ExePath unlocked=true fcHostRunning=0 marker=present"

# ── 真實資料夾的前後對照 ─────────────────────────────────────────────────────────────────
$realBase = Join-Path $env:LOCALAPPDATA 'tw.fintools.fc-host'
function Get-RealState {
    $o = [ordered]@{}
    foreach ($sub in @('EBWebView', 'logs')) {
        $p = Join-Path $realBase $sub
        if (Test-Path -LiteralPath $p) {
            $latest = Get-ChildItem -LiteralPath $p -Recurse -File -Force -ErrorAction SilentlyContinue |
                Sort-Object LastWriteTimeUtc -Descending | Select-Object -First 1
            $o[$sub] = if ($latest) { '{0:o}' -f $latest.LastWriteTimeUtc } else { 'empty' }
        } else {
            $o[$sub] = 'absent'
        }
    }
    return $o
}
$realBefore = Get-RealState
Write-Log ("REAL-BEFORE {0}" -f (($realBefore.GetEnumerator() | ForEach-Object { "$($_.Key)=$($_.Value)" }) -join ' '))

# ── 隔離根目錄 ───────────────────────────────────────────────────────────────────────────
$isoRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("fc-probe-render-{0}" -f (Get-Date -Format 'yyyyMMdd-HHmmss'))
$isoAppData = Join-Path $isoRoot 'appdata'
$isoLocal = Join-Path $isoRoot 'localappdata'
$isoWebView = Join-Path $isoRoot 'webview2'
foreach ($d in @($isoAppData, $isoLocal, $isoWebView)) { New-Item -ItemType Directory -Force -Path $d | Out-Null }
$isoRoot = (Resolve-Path -LiteralPath $isoRoot).Path
Write-Log "ISOLATION root=$isoRoot"

$saved = @{
    APPDATA = $env:APPDATA
    LOCALAPPDATA = $env:LOCALAPPDATA
    WEBVIEW2_USER_DATA_FOLDER = $env:WEBVIEW2_USER_DATA_FOLDER
}
$overall = 0
$udfSeen = New-Object System.Collections.Generic.HashSet[string]
try {
    foreach ($mode in $Modes) {
        $udf = Join-Path $isoWebView $mode
        $probeLog = Join-Path $OutDir "dw-1.4-$Tag-$mode.log"
        $env:APPDATA = $isoAppData
        $env:LOCALAPPDATA = $isoLocal
        $env:WEBVIEW2_USER_DATA_FOLDER = $udf
        $argv = @('--probe-render', '--mode', $mode, '--runs', $Runs, '--tag', $Tag,
            '--log', $probeLog, '--out', $OutDir, '--webview-data', $udf)
        if ($Anchor) { $argv += '--anchor' }
        if ($ControllerHidden) { $argv += '--controller-hidden' }
        $argv = $argv |
            ForEach-Object { $s = [string]$_; if ($s -match '\s') { '"' + $s + '"' } else { $s } }
        $proc = Start-Process -FilePath $ExePath -ArgumentList $argv -PassThru
        $env:APPDATA = $saved.APPDATA
        $env:LOCALAPPDATA = $saved.LOCALAPPDATA
        $env:WEBVIEW2_USER_DATA_FOLDER = $saved.WEBVIEW2_USER_DATA_FOLDER
        $rootStart = Get-ProcessStartTimeOrNull $proc
        Write-Log "START mode=$mode pid=$($proc.Id) args=$($argv -join ' ')"

        $deadline = (Get-Date).AddSeconds($TimeoutSecs)
        $modeUdf = New-Object System.Collections.Generic.HashSet[string]
        while (-not $proc.HasExited) {
            $all = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue |
                    Select-Object ProcessId, ParentProcessId, CreationDate, Name, CommandLine)
            $ids = @(Get-ProcessTreeIds -RootId $proc.Id -RootStartTime $rootStart -Processes $all)
            foreach ($p in $all) {
                if ($ids -contains [int]$p.ProcessId -and $p.Name -ieq 'msedgewebview2.exe' -and $p.CommandLine) {
                    $m = [regex]::Match($p.CommandLine, '--user-data-dir=(?:"([^"]+)"|(\S+))')
                    $dir = if ($m.Groups[1].Success) { $m.Groups[1].Value } else { $m.Groups[2].Value }
                    if ($m.Success -and $modeUdf.Add($dir)) {
                        Write-Log "UDF mode=$mode pid=$($p.ProcessId) --user-data-dir=$dir"
                    }
                }
            }
            if ((Get-Date) -gt $deadline) {
                Write-Log "TIMEOUT mode=$mode 超過 $TimeoutSecs 秒，停止自己啟動的行程樹"
                $stopped = Stop-ProcessTree -Process $proc
                Write-Log ("STOPPED {0}" -f ($stopped -join ','))
                $overall = 1
                break
            }
            Start-Sleep -Milliseconds 1000
        }
        $proc.WaitForExit()
        $code = $proc.ExitCode
        $probeCsv = Join-Path $OutDir "dw-1.4-$Tag-$mode.csv"
        Protect-EvidenceFile -Path @(@($probeLog, $probeCsv) | Where-Object { Test-Path -LiteralPath $_ })
        Write-Log "EXIT mode=$mode code=$code"
        if ($code -ne 0) { $overall = 1 }
        if ($modeUdf.Count -eq 0) {
            Write-Log "ISOLATION-FAIL mode=$mode 沒讀到任何 msedgewebview2 的 --user-data-dir"
            $overall = 1
        }
        foreach ($u in $modeUdf) {
            [void]$udfSeen.Add($u)
            if (-not $u.StartsWith($isoRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
                Write-Log "ISOLATION-FAIL mode=$mode --user-data-dir 不在隔離根目錄內：$u"
                $overall = 1
            }
        }
        # 殘留：根已結束，只收它留下的孤兒（建立時間落在根啟動之後）。
        $left = @(Stop-ProcessTree -Process $proc)
        Write-Log ("LEFTOVER mode=$mode stopped=[{0}]" -f ($left -join ','))
        Start-Sleep -Seconds 2
    }
} finally {
    $env:APPDATA = $saved.APPDATA
    $env:LOCALAPPDATA = $saved.LOCALAPPDATA
    $env:WEBVIEW2_USER_DATA_FOLDER = $saved.WEBVIEW2_USER_DATA_FOLDER
}

$realAfter = Get-RealState
Write-Log ("REAL-AFTER {0}" -f (($realAfter.GetEnumerator() | ForEach-Object { "$($_.Key)=$($_.Value)" }) -join ' '))
foreach ($k in $realBefore.Keys) {
    if ($realBefore[$k] -ne $realAfter[$k]) {
        Write-Log "REAL-CHANGED $k（可能是其他程式寫入，需人工判讀）"
    }
}
$isoLogs = @(Get-ChildItem -LiteralPath $isoLocal -Recurse -File -Force -ErrorAction SilentlyContinue)
Write-Log ("ISOLATED-LOCALAPPDATA files={0}" -f $isoLogs.Count)
$udfFiles = @(Get-ChildItem -LiteralPath $isoWebView -Recurse -File -Force -ErrorAction SilentlyContinue)
Write-Log ("ISOLATED-WEBVIEW2 files={0}" -f $udfFiles.Count)
try {
    Remove-Item -LiteralPath $isoRoot -Recurse -Force -ErrorAction Stop
    Write-Log "CLEANUP removed $isoRoot"
} catch {
    Write-Log "CLEANUP 未能刪除 $isoRoot：$($_.Exception.Message)"
}
Write-Log ("VERDICT {0}" -f $(if ($overall -eq 0) { 'PASS' } else { 'FAIL' }))
exit $overall
