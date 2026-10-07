<#
.SYNOPSIS
    host/tools/verify-5.3.ps1 的拖曳目標推導（Find-FreeDragOffset）mock 測試與結構檢查
    （純 PowerShell 斷言，不需 Pester）。

.DESCRIPTION
    **不執行 verify-5.3.ps1**（它會啟動宿主、注入滑鼠）：以 PowerShell Parser 只取出純函式，以兩種實機
    組態的工作區與預設格座標（host/src/settings.rs 的 DEFAULT_GRID_RECTS；腳本另開 custom1）推導，
    驗對照組的拖曳目標合法：在工作區內、不與任何已開小工具相交、確實離開原位。

    fix F7（批次 B）：舊版固定「往左 700 px」，會撞到腳本自己開的 custom1（0,1,14,8），放開後彈回，
    對照組看起來像「沒移動」，連帶步驟 2、5 判 FAIL。

.EXAMPLE
    pwsh -NoProfile -File host/tools/tests/Verify53.Tests.ps1
    結束碼 0＝全部通過、1＝有失敗。
#>
param([string]$Target)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$toolsDir = Split-Path $PSScriptRoot -Parent
if (-not $Target) { $Target = Join-Path $toolsDir 'verify-5.3.ps1' }

$script:Pass = 0
$script:Fail = 0
function Check([string]$name, [bool]$cond, [string]$detail = '') {
    if ($cond) { $script:Pass++; Write-Host "PASS  $name" }
    else { $script:Fail++; Write-Host "FAIL  $name $detail" -ForegroundColor Red }
}

$tokens = $null
$errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile($Target, [ref]$tokens, [ref]$errors)
Check 'verify-5.3.ps1 解析 0 錯誤' (@($errors).Count -eq 0) (($errors | ForEach-Object { $_.Message }) -join ' | ')
$GRID = 48
foreach ($name in 'Edge', 'Grid-Rect', 'Test-RectsOverlap', 'Find-FreeDragOffset') {
    $fn = $ast.Find({
            param($n)
            $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $name
        }, $true)
    Check "找得到函式 $name" ($null -ne $fn)
    if ($fn) { . ([scriptblock]::Create($fn.Extent.Text)) }
}

function Shift($r, [int]$dx, [int]$dy) { [PSCustomObject]@{ X = $r.X + $dx; Y = $r.Y + $dy; W = $r.W; H = $r.H } }
function Inside($r, $wa) { $r.X -ge $wa.X -and $r.Y -ge $wa.Y -and ($r.X + $r.W) -le ($wa.X + $wa.W) -and ($r.Y + $r.H) -le ($wa.Y + $wa.H) }

$configs = @(
    [PSCustomObject]@{ Name = '4K＠150%'; Wa = [PSCustomObject]@{ X = 0; Y = 0; W = 3840; H = 2088 } },
    [PSCustomObject]@{ Name = '筆電 2560×1600＠175%'; Wa = [PSCustomObject]@{ X = 3840; Y = 0; W = 2560; H = 1516 } }
)

if ((Get-Command Find-FreeDragOffset -ErrorAction SilentlyContinue) -and (Get-Command Test-RectsOverlap -ErrorAction SilentlyContinue)) {
    Check '相鄰（共用邊）不算相交' (-not (Test-RectsOverlap ([PSCustomObject]@{ X = 0; Y = 0; W = 10; H = 10 }) ([PSCustomObject]@{ X = 10; Y = 0; W = 10; H = 10 })))
    Check '重疊 1 px 算相交' (Test-RectsOverlap ([PSCustomObject]@{ X = 0; Y = 0; W = 10; H = 10 }) ([PSCustomObject]@{ X = 9; Y = 9; W = 10; H = 10 }))

    foreach ($c in $configs) {
        $wa = $c.Wa
        $clock = Grid-Rect $wa 15 1 16 10
        $others = @(
            (Grid-Rect $wa 15 12 16 30), (Grid-Rect $wa 32 1 15 17), (Grid-Rect $wa 32 19 15 23),
            (Grid-Rect $wa 15 43 32 4), (Grid-Rect $wa 0 1 14 8))
        $custom1 = $others[4]
        Check "$($c.Name)：舊手勢（往左 700 px）會撞 custom1（回歸對照）" (Test-RectsOverlap (Shift $clock -700 0) $custom1)

        $o = Find-FreeDragOffset $clock $others $wa
        Check "$($c.Name)：找得到合法目標" ($null -ne $o)
        if ($o) {
            $t = Shift $clock $o.Dx $o.Dy
            Check "$($c.Name)：目標＝起點平移 (Dx,Dy)" ($t.X -eq $o.Target.X -and $t.Y -eq $o.Target.Y) "dx=$($o.Dx) dy=$($o.Dy)"
            Check "$($c.Name)：目標在工作區內" (Inside $t $wa) "t=($($t.X),$($t.Y),$($t.W)x$($t.H))"
            $hit = @($others | Where-Object { Test-RectsOverlap $t $_ })
            Check "$($c.Name)：目標不與任何已開小工具相交（含 custom1、隱藏中的 quotes）" ($hit.Count -eq 0) "hit=$($hit.Count)"
            Check "$($c.Name)：確實移動（位移 ≥ 40 px）" ([math]::Sqrt($o.Dx * $o.Dx + $o.Dy * $o.Dy) -ge 40) "dx=$($o.Dx) dy=$($o.Dy)"
            $onGrid = @(0..48 | Where-Object { (Edge $wa.X $wa.W $_) -eq $t.X }).Count -gt 0 -and @(0..48 | Where-Object { (Edge $wa.Y $wa.H $_) -eq $t.Y }).Count -gt 0
            Check "$($c.Name)：目標左上角正好在格線上（放開時不會被對齊挪動）" $onGrid
            $back = Shift $t (-$o.Dx) (-$o.Dy)
            Check "$($c.Name)：反向手勢回到起點（步驟 5 用）" ($back.X -eq $clock.X -and $back.Y -eq $clock.Y)
        }
    }

    $full = [PSCustomObject]@{ X = 0; Y = 0; W = 480; H = 480 }
    $mov = [PSCustomObject]@{ X = 0; Y = 0; W = 480; H = 480 }
    Check '沒有空位（小工具佔滿工作區）→ $null' ($null -eq (Find-FreeDragOffset $mov @() $full))
}

# 結構：不得再寫死 700 px 的手勢。
$lit700 = @($ast.FindAll({
            param($n)
            $n -is [System.Management.Automation.Language.ConstantExpressionAst] -and "$($n.Value)" -eq '700'
        }, $true))
Check '腳本不再寫死 700 px 手勢' ($lit700.Count -eq 0) "got=$($lit700.Count)"

Write-Host "`n$($script:Pass) passed, $($script:Fail) failed"
if ($script:Fail -gt 0) { exit 1 }
exit 0
