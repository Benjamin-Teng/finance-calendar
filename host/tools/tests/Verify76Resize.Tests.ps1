<#
.SYNOPSIS
    host/tools/verify-7.6-edit-resize.ps1 的把手按下點推導（Get-HandlePoint、Get-InitialOccluderPoints）
    mock 測試與結構檢查（純 PowerShell 斷言，不需 Pester）。

.DESCRIPTION
    **不執行 verify-7.6-edit-resize.ps1**（它會啟動宿主、注入滑鼠）：以 PowerShell Parser 只取出純函式，
    以兩種實機組態的工作區（4K＠150% 3840×2088、筆電 2560×1600＠175% 工作區 2560×1516）推導格線矩形，
    驗按下點都落在當下小工具矩形內側。

    fix F7（批次 B）：舊版在加寬之前就對「加寬後右緣 −4」（17 格右緣）做遮擋檢查，那個點落在 clock
    右緣與 fixed 之間的桌面空隙，`(2556,260) 被 Progman 蓋住` 而例外中止。

.EXAMPLE
    pwsh -NoProfile -File host/tools/tests/Verify76Resize.Tests.ps1
    結束碼 0＝全部通過、1＝有失敗。
#>
param([string]$Target)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$toolsDir = Split-Path $PSScriptRoot -Parent
if (-not $Target) { $Target = Join-Path $toolsDir 'verify-7.6-edit-resize.ps1' }

$script:Pass = 0
$script:Fail = 0
function Check([string]$name, [bool]$cond, [string]$detail = '') {
    if ($cond) { $script:Pass++; Write-Host "PASS  $name" }
    else { $script:Fail++; Write-Host "FAIL  $name $detail" -ForegroundColor Red }
}

$tokens = $null
$errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile($Target, [ref]$tokens, [ref]$errors)
Check 'verify-7.6-edit-resize.ps1 解析 0 錯誤' (@($errors).Count -eq 0) (($errors | ForEach-Object { $_.Message }) -join ' | ')
$GRID = 48
foreach ($name in 'Edge', 'Grid-Rect', 'Test-PointInRect', 'Get-HandlePoint', 'Get-InitialOccluderPoints') {
    $fn = $ast.Find({
            param($n)
            $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $name
        }, $true)
    Check "找得到函式 $name" ($null -ne $fn)
    if ($fn) { . ([scriptblock]::Create($fn.Extent.Text)) }
}

$configs = @(
    [PSCustomObject]@{ Name = '4K＠150%'; Wa = [PSCustomObject]@{ X = 0; Y = 0; W = 3840; H = 2088 } },
    [PSCustomObject]@{ Name = '筆電 2560×1600＠175%'; Wa = [PSCustomObject]@{ X = 3840; Y = 0; W = 2560; H = 1516 } }
)

if ((Get-Command Get-HandlePoint -ErrorAction SilentlyContinue) -and (Get-Command Get-InitialOccluderPoints -ErrorAction SilentlyContinue)) {
    foreach ($c in $configs) {
        $rect0 = Grid-Rect $c.Wa 15 1 16 10
        $wide = Grid-Rect $c.Wa 15 1 17 10
        $fixed = Grid-Rect $c.Wa 32 1 16 10   # 預設 fixed 從 col 32 起（與 clock 之間隔一格空隙）
        $pts = @(Get-InitialOccluderPoints $rect0)
        Check "$($c.Name)：初始遮擋檢查點至少 6 個" ($pts.Count -ge 6) "got=$($pts.Count)"
        $out = @($pts | Where-Object { -not (Test-PointInRect $rect0 $_) })
        Check "$($c.Name)：初始遮擋檢查點全在 clock 16 格矩形內" ($out.Count -eq 0) (($out | ForEach-Object { "($($_[0]),$($_[1]))" }) -join ' ')
        $outFixed = @($pts | Where-Object { Test-PointInRect $fixed $_ })
        Check "$($c.Name)：初始遮擋檢查點都不在 fixed 上" ($outFixed.Count -eq 0)

        $e0 = Get-HandlePoint $rect0 'E'
        $w0 = Get-HandlePoint $rect0 'W'
        $eW = Get-HandlePoint $wide 'E'
        Check "$($c.Name)：右緣把手點在 clock 內" (Test-PointInRect $rect0 $e0) "pt=($($e0[0]),$($e0[1])) rect=$($rect0.X),$($rect0.W)"
        Check "$($c.Name)：左緣把手點在 clock 內" (Test-PointInRect $rect0 $w0) "pt=($($w0[0]),$($w0[1]))"
        Check "$($c.Name)：加寬後右緣把手點在 17 格矩形內" (Test-PointInRect $wide $eW) "pt=($($eW[0]),$($eW[1]))"
        Check "$($c.Name)：加寬後右緣把手點不在 16 格矩形內（只能加寬後再做遮擋檢查）" (-not (Test-PointInRect $rect0 $eW))
        Check "$($c.Name)：右緣把手點離右緣 ≤ 8 px（在 8 CSS px 感應帶內）" (($rect0.X + $rect0.W - $e0[0]) -le 8 -and ($rect0.X + $rect0.W - $e0[0]) -ge 1)
        Check "$($c.Name)：把手點縱向在中央（避開四角 14 px 方塊）" ($e0[1] -gt $rect0.Y + 40 -and $e0[1] -lt $rect0.Y + $rect0.H - 40)
    }
    Check '4K：舊版的 (2556,260) 確實不在 16 格 clock 內（回歸對照）' (-not (Test-PointInRect (Grid-Rect $configs[0].Wa 15 1 16 10) @(2556, 260)))
}

# 結構：初始的 Clear-Occluders 不得使用加寬後的矩形 $wide。
$calls = @($ast.FindAll({
            param($n)
            $n -is [System.Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq 'Clear-Occluders'
        }, $true))
Check '有 Clear-Occluders 呼叫' ($calls.Count -ge 2) "got=$($calls.Count)"
$wideBeforeStretch = @($calls | Where-Object { $_.Extent.Text -match '\$wide\b' })
Check 'Clear-Occluders 不直接拿 $wide 推點（改用當下矩形）' ($wideBeforeStretch.Count -eq 0) (($wideBeforeStretch | ForEach-Object { "第 $($_.Extent.StartLineNumber) 行" }) -join ', ')
$hardMinus4 = @($ast.FindAll({
            param($n)
            $n -is [System.Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq 'Invoke-Drag' -and
            $n.Extent.Text -match '-FromX\s+\(\$\w+\.X\s*\+\s*\$\w+\.W\s*-\s*4\)'
        }, $true))
Check 'Invoke-Drag 的按下點不寫死「右緣 −4」（改用 Get-HandlePoint）' ($hardMinus4.Count -eq 0) "got=$($hardMinus4.Count)"

Write-Host "`n$($script:Pass) passed, $($script:Fail) failed"
if ($script:Fail -gt 0) { exit 1 }
exit 0
