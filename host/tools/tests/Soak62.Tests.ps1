<#
.SYNOPSIS
    host/tools/soak-6.2.ps1 的摘要判定 mock 測試（純 PowerShell 斷言，不需 Pester）。

.DESCRIPTION
    **不執行 soak-6.2.ps1**（它會長時間取樣）：以 PowerShell Parser 只取出 Get-HostAbsentPeriods、
    Get-SoakSummary 兩個純函式，餵合成的 CSV 列。

    fix F4：2026-10-01 的 run1 證據在宿主消失後仍判記憶體 PASS——宿主不在的列 total_private_mb 為空，
    被當成 0 算進「最後 10 筆」，結尾 0.0 MB、增幅 −100%。修正後：摘要排除 host_pid=0 的列；宿主
    不在的期間單獨列出起訖與時長、出現即 FAIL；結尾改為最後一個宿主仍在時的 10 筆。

    -Target 可指向其他版本的腳本（例如 `git show <rev>:host/tools/soak-6.2.ps1` 存出的檔案）。

.EXAMPLE
    pwsh -NoProfile -File host/tools/tests/Soak62.Tests.ps1
    結束碼 0＝全部通過、1＝有失敗。
#>
param([string]$Target)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$toolsDir = Split-Path $PSScriptRoot -Parent
if (-not $Target) { $Target = Join-Path $toolsDir 'soak-6.2.ps1' }

$script:Pass = 0
$script:Fail = 0
function Check([string]$name, [bool]$cond, [string]$detail = '') {
    if ($cond) { $script:Pass++; Write-Host "PASS  $name" }
    else { $script:Fail++; Write-Host "FAIL  $name $detail" -ForegroundColor Red }
}

$tokens = $null; $errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile($Target, [ref]$tokens, [ref]$errors)
Check 'soak-6.2.ps1 解析 0 錯誤' (@($errors).Count -eq 0) (($errors | ForEach-Object { $_.Message }) -join ' | ')
foreach ($name in 'Get-HostAbsentPeriods', 'Get-SoakSummary') {
    $fn = $ast.Find({ param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $name }, $true)
    Check "找得到函式 $name" ($null -ne $fn)
    if ($fn) { . ([scriptblock]::Create($fn.Extent.Text)) }
}
if (-not (Get-Command Get-SoakSummary -ErrorAction SilentlyContinue)) {
    Write-Host ''
    Write-Host "合計：$($script:Pass) PASS、$($script:Fail) FAIL"
    exit 1
}

# 合成 CSV：每分鐘一筆，從 t0 起。$spec＝@(@(分鐘數, host_pid, total_private_mb, widgets), ...)。
$t0 = [datetimeoffset]'2026-09-30T23:54:34+08:00'
$header = 'ts,host_pid,host_private_mb,host_ws_mb,wv_count,wv_private_mb,wv_ws_mb,total_private_mb,cpu_pct,widgets,widget_ids,locked,gap_s'
function New-Rows([int]$Minutes, [scriptblock]$At) {
    $lines = @($header)
    for ($m = 0; $m -lt $Minutes; $m++) {
        $ts = $t0.AddMinutes($m).ToString('yyyy-MM-ddTHH:mm:ssK')
        $v = & $At $m
        if ($v.Pid -eq 0) { $lines += "$ts,0,,,,,,,,0,,False,60" }
        else { $lines += "$ts,$($v.Pid),10.0,30.0,11,$($v.Mb - 10),770.0,$($v.Mb),0.300,$($v.W),clock|dynamic|fixed|macro|quotes,False,60" }
    }
    return @($lines | ConvertFrom-Csv)
}

# ---------------------------------------------------------------- 1. 正常：宿主一直在、記憶體平穩
$rows = New-Rows 120 { param($m) @{ Pid = 46136; Mb = 600.0; W = 5 } }
$s = Get-SoakSummary -Rows $rows -IntervalSec 60
Check '正常：結論 PASS' ($s.Verdict -eq 'PASS') ($s.Lines -join ' | ')
Check '正常：沒有宿主不在的期間' (@($s.HostAbsent).Count -eq 0)

# ---------------------------------------------------------------- 2. run1 型態：宿主在第 100 分鐘後消失到結尾
$rows = New-Rows 135 { param($m) if ($m -ge 100) { @{ Pid = 0 } } else { @{ Pid = 46136; Mb = 620.0; W = 5 } } }
$s = Get-SoakSummary -Rows $rows -IntervalSec 60
Check 'run1 型態：結論 FAIL（宿主不在）' ($s.Verdict -eq 'FAIL') ($s.Lines -join ' | ')
Check 'run1 型態：結尾記憶體＝最後一個宿主仍在時的 10 筆（620 MB，不是 0）' ($null -ne $s.EndMb -and [Math]::Abs($s.EndMb - 620.0) -lt 0.01) "end=$($s.EndMb)"
Check 'run1 型態：記憶體增幅約 0%（不是 −100%）' ($null -ne $s.Growth -and [Math]::Abs($s.Growth) -lt 0.01) "growth=$($s.Growth)"
$abs = @($s.HostAbsent)
Check 'run1 型態：列出一段宿主不在、持續到結尾' ($abs.Count -eq 1 -and $abs[0].Ongoing) "count=$($abs.Count)"
if ($abs.Count -eq 1) {
    Check 'run1 型態：起點＝第 100 分鐘那一列、時長 34 分鐘' (($abs[0].Start -eq $t0.AddMinutes(100).DateTime -or $abs[0].Start.ToUniversalTime() -eq $t0.AddMinutes(100).UtcDateTime) -and [Math]::Abs($abs[0].Minutes - 34) -lt 0.01) "start=$($abs[0].Start) min=$($abs[0].Minutes)"
}
Check 'run1 型態：小工具消失清單不含宿主不在的列（另列）' (@($s.Lines | Where-Object { $_ -match 'widgets=0' }).Count -eq 0) ($s.Lines -join ' | ')
Check 'run1 型態：摘要文字寫出起訖與時長' (@($s.Lines | Where-Object { $_ -match '→' -and $_ -match '分鐘' -and $_ -match '宿主未恢復' }).Count -eq 1) ($s.Lines -join ' | ')

# ---------------------------------------------------------------- 3. 中途短暫消失後恢復
$rows = New-Rows 120 { param($m) if ($m -ge 60 -and $m -lt 63) { @{ Pid = 0 } } elseif ($m -ge 63) { @{ Pid = 50000; Mb = 600.0; W = 5 } } else { @{ Pid = 46136; Mb = 600.0; W = 5 } } }
$s = Get-SoakSummary -Rows $rows -IntervalSec 60
$abs = @($s.HostAbsent)
Check '短暫消失：結論 FAIL' ($s.Verdict -eq 'FAIL') ($s.Lines -join ' | ')
Check '短暫消失：一段、未持續到結尾、時長＝消失起點到恢復那一列（3 分鐘）' ($abs.Count -eq 1 -and -not $abs[0].Ongoing -and [Math]::Abs($abs[0].Minutes - 3) -lt 0.01) "count=$($abs.Count) min=$(if ($abs) { $abs[0].Minutes })"
Check '短暫消失：記憶體本身仍判 PASS（排除空白列）' $s.MemoryOk ($s.Lines -join ' | ')

# ---------------------------------------------------------------- 4. 記憶體真的成長、宿主一直在
$rows = New-Rows 120 { param($m) @{ Pid = 46136; Mb = $(if ($m -ge 100) { 800.0 } else { 600.0 }); W = 5 } }
$s = Get-SoakSummary -Rows $rows -IntervalSec 60
Check '記憶體成長 33%：結論 FAIL、MemoryOk=false' ($s.Verdict -eq 'FAIL' -and -not $s.MemoryOk) ($s.Lines -join ' | ')

# ---------------------------------------------------------------- 5. 宿主在、但小工具少了一扇
$rows = New-Rows 120 { param($m) @{ Pid = 46136; Mb = 600.0; W = $(if ($m -eq 90) { 4 } else { 5 }) } }
$s = Get-SoakSummary -Rows $rows -IntervalSec 60
Check '小工具消失一筆：結論 FAIL、WidgetsOk=false' ($s.Verdict -eq 'FAIL' -and -not $s.WidgetsOk) ($s.Lines -join ' | ')

# ---------------------------------------------------------------- 6. 還沒到基準期
$rows = New-Rows 5 { param($m) @{ Pid = 46136; Mb = 600.0; W = 5 } }
$s = Get-SoakSummary -Rows $rows -IntervalSec 60
Check '不到 10 分鐘：結論 INCOMPLETE（不是 PASS）' ($s.Verdict -eq 'INCOMPLETE') ($s.Lines -join ' | ')

Write-Host ''
Write-Host "合計：$($script:Pass) PASS、$($script:Fail) FAIL"
if ($script:Fail -gt 0) { exit 1 }
exit 0
