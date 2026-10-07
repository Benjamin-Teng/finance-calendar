<#
.SYNOPSIS
    Task 1.4 WebView2 故障探針的無人值守驅動腳本：啟動 probe_webview2（N 個同源小工具視窗），
    依序終止本探針底下的 renderer／GPU／browser 行程、令頁面 JS 卡死，並錄下兩份記錄檔。

.DESCRIPTION
    記錄檔（-OutDir，預設 host/tools/evidence）：
      1.4-probe-<Tag>.log   探針自己的事件記錄（ProcessFailed 種類、心跳中斷／恢復、行程對照、重建）
      1.4-driver-<Tag>.log  本腳本的步驟與每個階段前後的行程樹快照

    階段（-Phases，逗號分隔，依序執行）：
      R   終止第一個視窗所用的 renderer（依探針回報的「行程→frame」對照挑選）
      G   終止 GPU process（--type=gpu-process）
      U   令第二個視窗的頁面 JS 無窮迴圈，觀察是否收到 RENDER_PROCESS_UNRESPONSIVE；
          逾時仍卡住則終止該 renderer 收尾
      H   令第三個視窗的頁面 JS 無窮迴圈，8 秒後以 recreate 單窗重建，觀察心跳恢復與舊 renderer
          是否隨之結束
      B   終止 browser process（msedgewebview2 中父行程＝探針者）
      R2  B 之後對新一代第一個視窗重做 R（確認重建後的事件訂閱仍有效）

    -HoldSec <n>：所有階段跑完後維持 n 秒再結束（隔離驗證用，見 probe-1.4-isolation.ps1）。

    安全邊界：每次執行以 GUID 建獨立工作目錄與 user data folder。只終止「行程樹根＝本探針 PID」、
    且命令列 --user-data-dir 與本次 UDF 完整路徑相符的 msedgewebview2.exe；收尾只清本次執行中
    已登錄（PID＋CreationDate）且 UDF 相符的殘留行程，不做全域字串搜尋。絕不碰其他應用程式或
    其他探針執行個體的 WebView2。

.EXAMPLE
    cd host; cargo build --release --example probe_webview2
    pwsh -File host/tools/probe-1.4.ps1
    pwsh -File host/tools/probe-1.4.ps1 -Phases B -OnBrowserExit none -Tag none
#>
[CmdletBinding()]
param(
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [string]$ExePath = (Join-Path $PSScriptRoot '..\target\release\examples\probe_webview2.exe'),
    [int]$Count = 3,
    [string]$Phases = 'R,G,U,H,B,R2',
    [ValidateSet('rebuild', 'none')][string]$OnBrowserExit = 'rebuild',
    [int]$HangWaitSec = 40,
    [int]$HoldSec = 0,
    [string]$Tag = 'main'
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force
New-Item -ItemType Directory -Force $OutDir | Out-Null
# 每次執行一個 GUID：工作目錄、UDF、控制檔、info 全在其下，平行或同一秒啟動的執行個體互不共用。
$runId = [guid]::NewGuid().ToString('N')
$work = [IO.Path]::GetFullPath((Join-Path ([IO.Path]::GetTempPath()) "fc-probe14-$runId"))
New-Item -ItemType Directory -Force $work | Out-Null
$udf = Join-Path $work 'udf'
# WebView2 在 UDF 之下建 EBWebView，並以 --user-data-dir=<udf>\EBWebView 傳給每個子行程（2026-09-27 實測）。
$udfArgExpected = @($udf, (Join-Path $udf 'EBWebView'))
$probeLog = Join-Path $OutDir "1.4-probe-$Tag.log"
$driverLog = Join-Path $OutDir "1.4-driver-$Tag.log"
$info = Join-Path $work 'info.json'
$ctl = Join-Path $work 'ctl.txt'
Set-Content -Path $driverLog -Value (ConvertTo-EvidenceText "# probe-1.4 driver start $(Get-Date -Format o) runId=$runId phases=$Phases count=$Count onBrowserExit=$OnBrowserExit holdSec=$HoldSec udf=$udf") -Encoding utf8

function Stamp { Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fffK' }
function W([string]$m) { $l = "$(Stamp) $m"; Add-Content -Path $driverLog -Value (ConvertTo-EvidenceText $l) -Encoding utf8; Write-Host $l }

# 取命令列中 --user-data-dir 的完整值（引號或不帶引號兩種寫法），正規化成完整路徑；沒有則 $null。
function Get-UdfArg([string]$cmd) {
    if ($cmd -match '--user-data-dir=(?:"([^"]+)"|(\S+))') {
        $v = if ($Matches[1]) { $Matches[1] } else { $Matches[2] }
        try { return [IO.Path]::GetFullPath($v).TrimEnd('\') } catch { return $null }
    }
    return $null
}

# 命令列的 --user-data-dir 與本次 UDF「完整路徑」相等（不分大小寫），非子字串比對。
function Test-OurUdf($p) {
    $v = Get-UdfArg $p.CommandLine
    if (-not $v) { return $false }
    foreach ($e in $udfArgExpected) { if ($v -ieq $e.TrimEnd('\')) { return $true } }
    return $false
}

# 本次執行確認屬於本探針的行程身分：PID → CreationDate（防 PID 重用）。只在行程樹內且 UDF 完全相符時登錄。
$script:Known = @{}
function Register-Known($p) {
    if (Test-OurUdf $p) { $script:Known[[int]$p.ProcessId] = [string]$p.CreationDate }
}

function Get-ProbeTree([int]$probePid) {
    $all = @(Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'")
    $ids = New-Object 'System.Collections.Generic.HashSet[int]'
    [void]$ids.Add($probePid)
    $tree = New-Object System.Collections.Generic.List[object]
    do {
        $added = $false
        foreach ($p in $all) {
            if (-not $ids.Contains([int]$p.ProcessId) -and $ids.Contains([int]$p.ParentProcessId)) {
                [void]$ids.Add([int]$p.ProcessId); $tree.Add($p); $added = $true
            }
        }
    } while ($added)
    foreach ($p in $tree) {
        $type = if ($p.CommandLine -match '--type=([\w-]+)') { $Matches[1] } else { 'browser' }
        $p | Add-Member -NotePropertyName PType -NotePropertyValue $type -Force
        Register-Known $p
    }
    return $tree
}

function Write-Tree([int]$probePid, [string]$label) {
    $tree = Get-ProbeTree $probePid
    W "TREE[$label] probePid=$probePid alive=$([bool](Get-Process -Id $probePid -ErrorAction SilentlyContinue)) count=$($tree.Count)"
    foreach ($p in $tree) {
        W "  pid=$($p.ProcessId) ppid=$($p.ParentProcessId) type=$($p.PType) udfExact=$(Test-OurUdf $p)"
    }
    return $tree
}

function Stop-Ours([int]$probePid, [int]$target, [string]$why) {
    $tree = Get-ProbeTree $probePid
    $hit = $tree | Where-Object { [int]$_.ProcessId -eq $target }
    if (-not $hit) { W "REFUSE kill pid=$target ($why)：不在本探針行程樹內"; return $false }
    if (-not (Test-OurUdf $hit)) {
        W "REFUSE kill pid=$target ($why)：命令列 --user-data-dir 與本次 UDF 不完全相符"; return $false
    }
    W "KILL pid=$target type=$($hit.PType) ($why)"
    Stop-Process -Id $target -Force
    return $true
}

function Read-Info {
    for ($i = 0; $i -lt 20; $i++) {
        try { return Get-Content $info -Raw -ErrorAction Stop | ConvertFrom-Json } catch { Start-Sleep -Milliseconds 250 }
    }
    throw '讀不到探針 info JSON'
}

function Get-RendererFor($inf, [string]$label) {
    ($inf.processes | Where-Object { $_.kind -eq 'renderer' -and $_.frames -contains $label } | Select-Object -First 1).pid
}

function Send-Ctl([string]$cmd) { Set-Content -Path $ctl -Value $cmd -Encoding utf8; W "CTL $cmd" }

if (-not (Test-Path $ExePath)) { throw "找不到 $ExePath，請先 cargo build --release --example probe_webview2" }
$argList = @('--log', $probeLog, '--info', $info, '--ctl', $ctl, '--udf', $udf, '--count', $Count, '--on-browser-exit', $OnBrowserExit)
$proc = Start-Process -FilePath $ExePath -ArgumentList $argList -PassThru
$probePid = $proc.Id
W "START probe pid=$probePid"

# 等所有視窗的 renderer 對照出現
$deadline = (Get-Date).AddSeconds(30)
do {
    Start-Sleep -Milliseconds 500
    $inf = if (Test-Path $info) { Read-Info } else { $null }
    $ready = $inf -and (@($inf.labels | Where-Object { Get-RendererFor $inf $_ }).Count -eq $Count)
} while (-not $ready -and (Get-Date) -lt $deadline)
if (-not $ready) { W 'FAIL 30 秒內未取得完整行程對照'; }
Start-Sleep -Seconds 3
[void](Write-Tree $probePid 'baseline')

try {
    foreach ($ph in ($Phases -split ',' | ForEach-Object { $_.Trim() })) {
        W "PHASE $ph begin"
        switch ($ph) {
            'R' {
                $inf = Read-Info; $l = $inf.labels[0]; $r = Get-RendererFor $inf $l
                W "R target label=$l renderer=$r"
                if ($r) { [void](Stop-Ours $probePid $r "renderer of $l") }
                Start-Sleep -Seconds 12
            }
            'G' {
                $g = (Get-ProbeTree $probePid | Where-Object { $_.PType -eq 'gpu-process' } | Select-Object -First 1).ProcessId
                if ($g) { [void](Stop-Ours $probePid $g 'gpu-process') } else { W 'G 找不到 gpu-process' }
                Start-Sleep -Seconds 10
            }
            'U' {
                $inf = Read-Info; $l = $inf.labels[[Math]::Min(1, $inf.labels.Count - 1)]; $r = Get-RendererFor $inf $l
                W "U target label=$l renderer=$r waitSec=$HangWaitSec"
                Send-Ctl "hang $l"
                Start-Sleep -Seconds $HangWaitSec
                $got = Select-String -Path $probeLog -Pattern "PF label=$l kind=RENDER_PROCESS_UNRESPONSIVE" -Quiet
                W "U unresponsiveEventReceived=$got"
                $inf2 = Read-Info; $r2 = Get-RendererFor $inf2 $l
                if (-not $got -and $r2) {
                    W "U cleanup：終止仍卡住的 renderer $r2"
                    [void](Stop-Ours $probePid $r2 "hung renderer of $l")
                }
                Start-Sleep -Seconds 10
            }
            'H' {
                $inf = Read-Info; $l = $inf.labels[[Math]::Min(2, $inf.labels.Count - 1)]; $r = Get-RendererFor $inf $l
                W "H target label=$l renderer=$r（JS 卡死 → 等心跳中斷 → 單窗重建）"
                Send-Ctl "hang $l"
                Start-Sleep -Seconds 8
                Send-Ctl "recreate $l"
                Start-Sleep -Seconds 10
                $still = [bool](Get-Process -Id $r -ErrorAction SilentlyContinue)
                W "H oldRendererAlive=$still (pid=$r)"
                if ($still) {
                    Start-Sleep -Seconds 20
                    $still = [bool](Get-Process -Id $r -ErrorAction SilentlyContinue)
                    W "H oldRendererAlive(+20s)=$still (pid=$r)"
                }
            }
            'B' {
                $b = (Get-ProbeTree $probePid | Where-Object { $_.PType -eq 'browser' -and [int]$_.ParentProcessId -eq $probePid } | Select-Object -First 1).ProcessId
                if ($b) { [void](Stop-Ours $probePid $b 'browser process') } else { W 'B 找不到 browser process' }
                Start-Sleep -Seconds 20
            }
            'R2' {
                $deadline = (Get-Date).AddSeconds(15)
                do { $inf = Read-Info; Start-Sleep -Milliseconds 500 } while ($inf.gen -lt 1 -and (Get-Date) -lt $deadline)
                $l = $inf.labels[0]; $r = Get-RendererFor $inf $l
                W "R2 gen=$($inf.gen) target label=$l renderer=$r"
                if ($r) { [void](Stop-Ours $probePid $r "renderer of $l (after rebuild)") }
                Start-Sleep -Seconds 12
            }
            default { W "未知階段 $ph" }
        }
        Send-Ctl 'dump'
        Start-Sleep -Seconds 1
        [void](Write-Tree $probePid "after-$ph")
    }
    if ($HoldSec -gt 0) {
        # 隔離驗證用：階段跑完後維持運作一段時間，前後各拍一次行程樹，並確認工作目錄仍在。
        [void](Write-Tree $probePid 'hold-begin')
        W "HOLD begin sec=$HoldSec workExists=$(Test-Path $work)"
        Start-Sleep -Seconds $HoldSec
        W "HOLD end workExists=$(Test-Path $work) infoExists=$(Test-Path $info)"
        [void](Write-Tree $probePid 'hold-end')
    }
}
finally {
    [void](Get-ProbeTree $probePid)   # 結束前最後登錄一次本探針的行程身分
    Send-Ctl 'quit'
    if (-not $proc.WaitForExit(10000)) { W 'probe 未在 10 秒內結束，Stop-Process'; Stop-Process -Id $probePid -Force }
    Start-Sleep -Seconds 2
    # 收尾：探針已結束、父子關係斷掉的殘留 msedgewebview2。只處理「本次執行中已登錄的 PID、
    # CreationDate 相同（非 PID 重用）、且 --user-data-dir 與本次 UDF 完整路徑相符」者；不做全域字串搜尋。
    $left = @(Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" | Where-Object {
            $script:Known.ContainsKey([int]$_.ProcessId) -and
            $script:Known[[int]$_.ProcessId] -eq [string]$_.CreationDate -and
            (Test-OurUdf $_)
        })
    W "END probeExited=$($proc.HasExited) exitCode=$($proc.ExitCode) knownPids=$($script:Known.Count) leftoverWebView2=$($left.Count)"
    foreach ($p in $left) { W "  leftover pid=$($p.ProcessId) → Stop-Process"; Stop-Process -Id $p.ProcessId -Force -ErrorAction SilentlyContinue }
    Start-Sleep -Seconds 1
    # $work 由本次 GUID 產生，只屬於本執行個體。
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}
