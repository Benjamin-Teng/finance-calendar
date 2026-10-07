<#
.SYNOPSIS
    verify-b10-monitorpower.ps1 與 verify-b6-resolution.ps1 的判定函式 mock 測試（純 PowerShell 斷言，
    不需 Pester）。

.DESCRIPTION
    **不執行兩支腳本**（它們會關閉顯示器／改解析度、啟動宿主）：以 PowerShell Parser 只取出純函式
    定義，餵合成的記錄行、settings.json 與視窗矩形。另以語法樹做幾項靜態檢查（防呆位置、旗標先設、
    無人值守防護）。

    fix F4（review B-batch3）：
      - B10 [high]：DisplayOff(false) 只能算 DisplayOff(true) 之後、且在腳本動作基準行之後的那一行；
        宿主啟動時的初始通知不得讓「喚醒」恆真。
      - B10 [medium]：沒有 -UserPresent 就 BLOCKED；防待機旗標先設再呼叫、finally 還原。
      - B6 [high]：逐扇比對所有可見小工具的格線矩形、placement 不變、兩兩不相交、applied=可見數。

    -B10Target／-B6Target 可指向其他版本的腳本（例如 `git show <rev>:<path>` 存出的檔案）。

.EXAMPLE
    pwsh -NoProfile -File host/tools/tests/VerifyB10B6.Tests.ps1
    結束碼 0＝全部通過、1＝有失敗。
#>
param([string]$B10Target, [string]$B6Target)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$toolsDir = Split-Path $PSScriptRoot -Parent
if (-not $B10Target) { $B10Target = Join-Path $toolsDir 'verify-b10-monitorpower.ps1' }
if (-not $B6Target) { $B6Target = Join-Path $toolsDir 'verify-b6-resolution.ps1' }

$script:Pass = 0
$script:Fail = 0
function Check([string]$name, [bool]$cond, [string]$detail = '') {
    if ($cond) { $script:Pass++; Write-Host "PASS  $name" }
    else { $script:Fail++; Write-Host "FAIL  $name $detail" -ForegroundColor Red }
}

function Import-Functions([string]$Path, [string[]]$Names) {
    $tokens = $null; $errors = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseFile($Path, [ref]$tokens, [ref]$errors)
    Check "$(Split-Path $Path -Leaf) 解析 0 錯誤" (@($errors).Count -eq 0) (($errors | ForEach-Object { $_.Message }) -join ' | ')
    $missing = @()
    foreach ($name in $Names) {
        $fn = $ast.Find({ param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $name }, $true)
        if ($fn) { . ([scriptblock]::Create($fn.Extent.Text)); Set-Item -Path "function:script:$name" -Value (Get-Item "function:$name").ScriptBlock }
        else { $missing += $name }
    }
    Check "$(Split-Path $Path -Leaf) 找得到函式 $($Names -join '、')" ($missing.Count -eq 0) "缺：$($missing -join ',')"
    return $ast
}

function Test-InsideTry($node) {
    $p = $node.Parent
    while ($p) { if ($p -is [System.Management.Automation.Language.TryStatementAst]) { return $true }; $p = $p.Parent }
    return $false
}

# ================================================================ B10
$b10Ast = Import-Functions $B10Target @('Get-DisplayPowerObservation', 'Get-B10Verdict')
$start = '2026-09-29T10:24:19.473 EVENT power-setting-change signal=DisplayOff(false)'
$off = '2026-09-29T10:24:21.857 EVENT power-setting-change signal=DisplayOff(true)'
$on = '2026-09-29T10:24:40.000 EVENT power-setting-change signal=DisplayOff(false)'
$noise = '2026-09-29T10:24:21.900 EVENT session Locked=true'

if (Get-Command Get-DisplayPowerObservation -ErrorAction SilentlyContinue) {
    # 2026-09-29 證據的型態：啟動時的初始 DisplayOff(false) 在前、之後只有 DisplayOff(true)。
    $o = Get-DisplayPowerObservation -Lines @($start, $off, $noise) -StartIndex 1
    Check 'B10：啟動時的初始 DisplayOff(false) 不算喚醒（只有關閉）' ($o.HasOff -and -not $o.HasOn) "off=$($o.HasOff) on=$($o.HasOn)"
    $o = Get-DisplayPowerObservation -Lines @($start, $off, $noise) -StartIndex 0
    Check 'B10：即使基準設在 0，DisplayOff(true) 之前的 false 也不算' ($o.HasOff -and -not $o.HasOn) "off=$($o.HasOff) on=$($o.HasOn)"
    $o = Get-DisplayPowerObservation -Lines @($start, $off, $noise, $on) -StartIndex 1
    Check 'B10：DisplayOff(true) 之後的 DisplayOff(false)＝喚醒' ($o.HasOff -and $o.HasOn -and $o.OnIndex -eq 3) "off=$($o.HasOff) on=$($o.HasOn) idx=$($o.OnIndex)"
    $o = Get-DisplayPowerObservation -Lines @($off, $on, $start) -StartIndex 2
    Check 'B10：基準行之前的關閉／開啟都不算' (-not $o.HasOff -and -not $o.HasOn) "off=$($o.HasOff) on=$($o.HasOn)"
    $o = Get-DisplayPowerObservation -Lines @() -StartIndex 0
    Check 'B10：空記錄 → 兩者皆無' (-not $o.HasOff -and -not $o.HasOn)
}
if (Get-Command Get-B10Verdict -ErrorAction SilentlyContinue) {
    $v = Get-B10Verdict -HasOff $true -HasOn $true
    Check 'B10 判定：關閉＋其後開啟 → PASS／0' ($v.Verdict -eq 'PASS' -and $v.ExitCode -eq 0) "$($v.Verdict)/$($v.ExitCode)"
    $v = Get-B10Verdict -HasOff $true -HasOn $false
    Check 'B10 判定：只有關閉 → BLOCKED-PARTIAL／3' ($v.Verdict -eq 'BLOCKED-PARTIAL' -and $v.ExitCode -eq 3) "$($v.Verdict)/$($v.ExitCode)"
    $v = Get-B10Verdict -HasOff $true -HasOn $false -LockedAfterWake $true -Stalled $true
    Check 'B10 判定：喚醒後鎖定＋曾待機 → BLOCKED-PARTIAL 並寫明原因' ($v.Verdict -eq 'BLOCKED-PARTIAL' -and $v.Reason -match '鎖定' -and $v.Reason -match '待機') $v.Reason
    $v = Get-B10Verdict -HasOff $false -HasOn $false
    Check 'B10 判定：沒有關閉 → FAIL／1' ($v.Verdict -eq 'FAIL' -and $v.ExitCode -eq 1) "$($v.Verdict)/$($v.ExitCode)"
}

# 靜態：-UserPresent 防護在 try 之前、以 exit 結束；防待機旗標先設再呼叫、finally 還原。
$b10Text = Get-Content $B10Target -Raw
$upGuard = @($b10Ast.FindAll({
            param($n)
            $n -is [System.Management.Automation.Language.IfStatementAst] -and
            $n.Clauses[0].Item1.Extent.Text -match '-not\s+\$UserPresent' -and
            $n.Clauses[0].Item2.Extent.Text -match '\bexit\s+4\b'
        }, $true))
Check 'B10 靜態：沒有 -UserPresent 就 exit 4（BLOCKED），且防護不在 try 內' ($upGuard.Count -eq 1 -and -not (Test-InsideTry $upGuard[0])) "found=$($upGuard.Count)"
$flagIdx = $b10Text.IndexOf('$execStateSet = $true')
$setIdx = $b10Text.IndexOf('SetThreadExecutionState($ES_CONTINUOUS -bor $ES_SYSTEM_REQUIRED)')
Check 'B10 靜態：防待機的還原旗標在 SetThreadExecutionState 之前設好' ($flagIdx -ge 0 -and $setIdx -gt $flagIdx) "flag=$flagIdx set=$setIdx"
$finallyHasRestore = @($b10Ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.TryStatementAst] -and $n.Finally -and
            $n.Finally.Extent.Text -match 'SetThreadExecutionState\(\$ES_CONTINUOUS\)' }, $true)).Count -gt 0
Check 'B10 靜態：finally 以 ES_CONTINUOUS 還原執行狀態' $finallyHasRestore

# ================================================================ B6
$b6Ast = Import-Functions $B6Target @('Get-GridRectPx', 'Test-SameRect', 'Test-RectsIntersect', 'Get-PlacementMap',
    'Compare-PlacementMap', 'Get-RelayoutApplied', 'Get-DisplayChangeLog', 'Test-WidgetGridLayout', 'Test-SameMode')

# host/src/settings.rs DEFAULT_GRID_RECTS 的財經五扇（預設啟用），custom1 預設關閉。
$grid = [ordered]@{
    clock = @(15, 1, 16, 10); macro = @(15, 12, 16, 30); fixed = @(32, 1, 15, 17); dynamic = @(32, 19, 15, 23); quotes = @(15, 43, 32, 4)
    custom1 = @(0, 1, 14, 8)
}
function New-SettingsJson([hashtable]$Override = @{}) {
    $w = [ordered]@{}
    foreach ($id in $grid.Keys) {
        $g = if ($Override.ContainsKey($id)) { $Override[$id] } else { $grid[$id] }
        $w[$id] = [ordered]@{ enabled = ($id -ne 'custom1'); placement = [ordered]@{ monitor = 'primary'; col = $g[0]; row = $g[1]; w = $g[2]; h = $g[3] } }
    }
    ([ordered]@{ version = 2; widgets = $w } | ConvertTo-Json -Depth 6)
}
$work = [PSCustomObject]@{ Left = 0; Top = 0; W = 1920; H = 1152 }

$haveB6 = [bool](Get-Command Test-WidgetGridLayout -ErrorAction SilentlyContinue) -and [bool](Get-Command Get-PlacementMap -ErrorAction SilentlyContinue)
if ($haveB6) {
    $pm = Get-PlacementMap (New-SettingsJson)
    Check 'B6 placement：讀出 6 扇、啟用 5 扇' ($pm.Count -eq 6 -and @($pm.Keys | Where-Object { $pm[$_].Enabled }).Count -eq 5) "count=$($pm.Count)"
    $g = Get-GridRectPx $work $pm['fixed']
    # 1920/48=40、1152/48=24：fixed (32,1,15,17) → (1280,24,600x408)
    Check 'B6 格線換算：fixed＝(1280,24,600x408)' ($g.X -eq 1280 -and $g.Y -eq 24 -and $g.W -eq 600 -and $g.H -eq 408) "($($g.X),$($g.Y),$($g.W)x$($g.H))"
    $gOdd = Get-GridRectPx ([PSCustomObject]@{ Left = 10; Top = 0; W = 1000; H = 500 }) ([PSCustomObject]@{ col = 1; row = 1; w = 2; h = 1 })
    # floor(1×1000/48)=20、floor(3×1000/48)=62 → X=30、W=42；floor(500/48)=10、floor(1000/48)=20 → Y=10、H=10
    Check 'B6 格線換算：不整除時用 floor（同 layout.rs）' ($gOdd.X -eq 30 -and $gOdd.W -eq 42 -and $gOdd.Y -eq 10 -and $gOdd.H -eq 10) "($($gOdd.X),$($gOdd.Y),$($gOdd.W)x$($gOdd.H))"

    function New-Windows($Placements, $Work, [hashtable]$Shift = @{}) {
        @($Placements.Keys | Where-Object { $Placements[$_].Enabled } | ForEach-Object {
                $r = Get-GridRectPx $Work $Placements[$_]
                $d = if ($Shift.ContainsKey($_)) { $Shift[$_] } else { @(0, 0, 0, 0) }
                [PSCustomObject]@{ Id = $_; X = $r.X + $d[0]; Y = $r.Y + $d[1]; W = $r.W + $d[2]; H = $r.H + $d[3] }
            })
    }
    $ok = Test-WidgetGridLayout (New-Windows $pm $work) $pm $work
    Check 'B6 逐扇：五扇都在格線位置 → Ok' $ok.Ok ($ok.Lines -join ' | ')

    # review 3.5 high 的失效型態：fixed 正確、但最後一扇（quotes）停在 Windows 建議矩形。
    $bad = Test-WidgetGridLayout (New-Windows $pm $work @{ quotes = @(37, -15, 0, 0) }) $pm $work
    Check 'B6 逐扇：fixed 正確但 quotes 偏離 → 不 Ok，且點名 quotes' ((-not $bad.Ok) -and ($bad.Mismatched -contains 'quotes')) ($bad.Lines -join ' | ')
    $bad = Test-WidgetGridLayout (New-Windows $pm $work @{ dynamic = @(0, 0, 1, 0) }) $pm $work
    Check 'B6 逐扇：差 1px 也不 Ok' ((-not $bad.Ok) -and ($bad.Mismatched -contains 'dynamic')) ($bad.Lines -join ' | ')
    $pmOverlap = Get-PlacementMap (New-SettingsJson @{ dynamic = @(32, 10, 15, 23) })
    $bad = Test-WidgetGridLayout (New-Windows $pmOverlap $work) $pmOverlap $work
    Check 'B6 逐扇：兩扇格座標相交 → 不 Ok，回報相交' ((-not $bad.Ok) -and $bad.Overlaps.Count -ge 1) ($bad.Lines -join ' | ')
    $wins4 = @(New-Windows $pm $work | Where-Object { $_.Id -ne 'macro' })
    $bad = Test-WidgetGridLayout $wins4 $pm $work
    Check 'B6 逐扇：啟用的小工具少一扇 → 不 Ok，回報缺少' ((-not $bad.Ok) -and ($bad.Missing -contains 'macro')) ($bad.Lines -join ' | ')
    $edge = @([PSCustomObject]@{ X = 0; Y = 0; W = 10; H = 10 }, [PSCustomObject]@{ X = 10; Y = 0; W = 10; H = 10 })
    Check 'B6 相交：邊緣相接不算相交' (-not (Test-RectsIntersect $edge[0] $edge[1]))

    $pmMoved = Get-PlacementMap (New-SettingsJson @{ dynamic = @(32, 20, 15, 23) })
    $diff = @(Compare-PlacementMap $pm $pmMoved)
    Check 'B6 placement 不變：任何一扇格座標變了就回報該扇' ($diff.Count -eq 1 -and $diff[0] -eq 'dynamic') ($diff -join ',')
    Check 'B6 placement 不變：完全相同 → 空' (@(Compare-PlacementMap $pm (Get-PlacementMap (New-SettingsJson))).Count -eq 0)
}

if (Get-Command Get-DisplayChangeLog -ErrorAction SilentlyContinue) {
    Check 'B6 RELAYOUT：applied=5 → 5' ((Get-RelayoutApplied '2026-10-01T10:00:00.000 RELAYOUT reason=display-change applied=5') -eq 5)
    Check 'B6 RELAYOUT：多位數 applied=12 → 12' ((Get-RelayoutApplied '2026-10-01T10:00:00.000 RELAYOUT reason=dpi-change,display-change applied=12') -eq 12)
    Check 'B6 RELAYOUT：沒有 applied → -1' ((Get-RelayoutApplied 'RELAYOUT reason=display-change') -eq -1)
    $lines = @(
        '2026-10-01T10:00:00.000 RELAYOUT reason=display-change applied=5',
        '2026-10-01T10:00:05.000 EVENT WM_DISPLAYCHANGE',
        '2026-10-01T10:00:05.100 RELAYOUT reason=display-change applied=5',
        '2026-10-01T10:00:05.300 RELAYOUT reason=dpi-change,display-change applied=4'
    )
    $dc = Get-DisplayChangeLog -Lines $lines -StartIndex 1
    Check 'B6 RELAYOUT：取基準行之後「最後一行」display-change 的 applied（4）' ($dc.HasEvent -and $dc.Applied -eq 4) "event=$($dc.HasEvent) applied=$($dc.Applied)"
    $dc = Get-DisplayChangeLog -Lines $lines -StartIndex 4
    Check 'B6 RELAYOUT：基準行之後沒有 → 無事件、applied=-1' ((-not $dc.HasEvent) -and $dc.Applied -eq -1) "event=$($dc.HasEvent) applied=$($dc.Applied)"
}

if (Get-Command Test-SameMode -ErrorAction SilentlyContinue) {
    $m1 = [PSCustomObject]@{ dmPelsWidth = 2560; dmPelsHeight = 1600; dmDisplayFrequency = 240; dmDisplayOrientation = 0 }
    $m2 = [PSCustomObject]@{ dmPelsWidth = 2560; dmPelsHeight = 1600; dmDisplayFrequency = 60; dmDisplayOrientation = 0 }
    Check 'B6 模式比對：相同 → true' (Test-SameMode $m1 $m1)
    Check 'B6 模式比對：頻率不同 → false' (-not (Test-SameMode $m1 $m2))
    Check 'B6 模式比對：讀不到（$null）→ false' (-not (Test-SameMode $null $m1))
}

# 靜態：「需要還原」旗標在 ChangeDisplaySettingsExW（目標模式）之前設好；保險還原以旗標與原始模式為前提。
$b6Text = Get-Content $B6Target -Raw
$flagIdx = $b6Text.IndexOf('$needRevert = $true')
$changeIdx = $b6Text.IndexOf('ChangeDisplaySettingsExW($deviceName, [ref]$t,')
Check 'B6 靜態：還原旗標在套用目標模式之前設好' ($flagIdx -ge 0 -and $changeIdx -gt $flagIdx) "flag=$flagIdx change=$changeIdx"
Check 'B6 靜態：finally 的保險還原以「已改過且有原始模式」為前提' ($b6Text -match 'if \(\$needRevert -and -not \$revertedOk -and \$orig\)')
Check 'B6 靜態：還原前查鎖定（Restore-OriginalMode 內）' ($b6Text -match '(?s)function Restore-OriginalMode.*?Test-Locked.*?ChangeDisplaySettingsExW')

Write-Host ''
Write-Host "合計：$($script:Pass) PASS、$($script:Fail) FAIL"
if ($script:Fail -gt 0) { exit 1 }
exit 0
