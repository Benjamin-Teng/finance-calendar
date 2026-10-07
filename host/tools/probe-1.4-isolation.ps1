<#
.SYNOPSIS
    Task 1.4 驅動腳本的執行個體隔離驗證：幾乎同時啟動兩個不同 Tag 的 probe-1.4.ps1，
    確認其中一個（A）跑完收尾時，不影響另一個（B）的 WebView2 行程與工作檔案。

.DESCRIPTION
    B：-Phases R -HoldSec 60 -Tag isoB（跑完 R 後維持 60 秒，前後各拍一次行程樹）
    A：-Phases R,G -Tag isoA（較短，會在 B 的維持期間內結束並收尾）
    判定（全部成立才 PASS），結果寫 <OutDir>/1.4-isolation.log：
      1. 兩者 runId 不同（工作目錄／UDF 各自獨立）
      2. A 的 END（收尾）時間落在 B 的 HOLD 期間內
      3. B 在 hold-begin 與 hold-end 的行程樹 PID 集合完全相同，且非空
      4. B 的探針記錄在 HOLD 期間沒有任何 ProcessFailed（PF）或心跳中斷（HB-STOP）
      5. B 在 HOLD 結束時工作目錄與 info 檔仍存在
      6. A 終止過的 PID 都不在 B 任何一次行程樹快照中

.EXAMPLE
    cd host; cargo build --release --example probe_webview2
    pwsh -File host/tools/probe-1.4-isolation.ps1
#>
[CmdletBinding()]
param([string]$OutDir = (Join-Path $PSScriptRoot 'evidence'))

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force
$drv = Join-Path $PSScriptRoot 'probe-1.4.ps1'
$out = Join-Path $OutDir '1.4-isolation.log'
$lines = New-Object System.Collections.Generic.List[string]
function L([string]$m) { $lines.Add($m); Write-Host $m }

$common = @('-NoProfile', '-File', $drv, '-OutDir', $OutDir)
$b = Start-Process pwsh -ArgumentList ($common + @('-Phases', 'R', '-HoldSec', '60', '-Tag', 'isoB')) -PassThru -WindowStyle Hidden
$a = Start-Process pwsh -ArgumentList ($common + @('-Phases', 'R,G', '-Tag', 'isoA')) -PassThru -WindowStyle Hidden
L "# probe-1.4 isolation start $(Get-Date -Format o) driverA.pid=$($a.Id) driverB.pid=$($b.Id)"
if (-not $a.WaitForExit(180000)) { L 'FAIL A 驅動逾時'; exit 1 }
if (-not $b.WaitForExit(240000)) { L 'FAIL B 驅動逾時'; exit 1 }

$aDrv = Get-Content (Join-Path $OutDir '1.4-driver-isoA.log')
$bDrv = Get-Content (Join-Path $OutDir '1.4-driver-isoB.log')
$bProbe = Get-Content (Join-Path $OutDir '1.4-probe-isoB.log')

function T([string]$line) { [datetime]::Parse(($line -split ' ')[0]) }
function RunId($drvLines) { if ($drvLines[0] -match 'runId=(\w+)') { $Matches[1] } }
function TreePids($drvLines, [string]$label) {
    $key = "TREE[$label]"   # 用 Contains，避免 -like 把 [] 當萬用字元類別
    $i = -1
    for ($k = 0; $k -lt $drvLines.Count; $k++) { if ($drvLines[$k].Contains($key)) { $i = $k; break } }
    if ($i -lt 0) { return @() }
    $pids = @()
    for ($j = $i + 1; $j -lt $drvLines.Count -and $drvLines[$j] -match '^\S+\s+pid=(\d+)'; $j++) { $pids += [int]$Matches[1] }
    return ($pids | Sort-Object)
}

$ok = $true
function Check([bool]$cond, [string]$msg) { L ("{0} {1}" -f ($(if ($cond) { 'PASS' } else { 'FAIL' }), $msg)); if (-not $cond) { $script:ok = $false } }

$ra = RunId $aDrv; $rb = RunId $bDrv
L "A header: $($aDrv[0])"
L "B header: $($bDrv[0])"
Check ($ra -and $rb -and $ra -ne $rb) "1 runId 不同 A=$ra B=$rb"

$aEnd = $aDrv | Where-Object { $_ -match ' END ' } | Select-Object -First 1
$hb = $bDrv | Where-Object { $_ -match ' HOLD begin ' } | Select-Object -First 1
$he = $bDrv | Where-Object { $_ -match ' HOLD end ' } | Select-Object -First 1
L "A END : $aEnd"
L "B HOLD: $hb"
L "B HOLD: $he"
$inWindow = $aEnd -and $hb -and $he -and (T $aEnd) -gt (T $hb) -and (T $aEnd) -lt (T $he)
Check $inWindow '2 A 的收尾發生在 B 的 HOLD 期間'

$p1 = @(TreePids $bDrv 'hold-begin'); $p2 = @(TreePids $bDrv 'hold-end')
Check ($p1.Count -gt 0 -and (($p1 -join ',') -eq ($p2 -join ','))) "3 B 行程樹 hold-begin=[$($p1 -join ',')] hold-end=[$($p2 -join ',')]"

$bad = @()
if ($hb -and $he) {
    $t1 = T $hb; $t2 = T $he
    $bad = @($bProbe | Where-Object { $_ -match ' (PF|HB-STOP) ' -and (T $_) -gt $t1 -and (T $_) -lt $t2 })
}
Check ($hb -and $he -and $bad.Count -eq 0) "4 B 在 HOLD 期間的 PF／HB-STOP 筆數=$($bad.Count)"
foreach ($x in $bad) { L "    $x" }

Check ([bool]($he -match 'workExists=True infoExists=True')) '5 B 工作目錄與 info 在 HOLD 結束時仍存在'

$aKilled = @($aDrv | Where-Object { $_ -match ' KILL pid=(\d+)' } | ForEach-Object { [void]($_ -match ' KILL pid=(\d+)'); [int]$Matches[1] })
$bAll = @($bDrv | Where-Object { $_ -match '^\S+\s+pid=(\d+)' } | ForEach-Object { [void]($_ -match 'pid=(\d+)'); [int]$Matches[1] })
$cross = @($aKilled | Where-Object { $bAll -contains $_ })
Check ($aKilled.Count -gt 0 -and $cross.Count -eq 0) "6 A 終止的 PID=[$($aKilled -join ',')] 與 B 行程交集=[$($cross -join ',')]"

L ("# verdict {0}" -f $(if ($ok) { 'PASS' } else { 'FAIL' }))
$lines | ConvertTo-EvidenceText | Set-Content -Path $out -Encoding utf8
if (-not $ok) { exit 1 }
