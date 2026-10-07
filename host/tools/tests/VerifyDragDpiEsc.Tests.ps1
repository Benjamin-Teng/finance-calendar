<#
.SYNOPSIS
    host/tools/verify-dragdpi-esc.ps1 的判定函式（Get-EscObservation、Get-ReturnVerdict）mock 測試與
    Esc 安全／不計分的結構檢查（純 PowerShell 斷言，不需 Pester）。

.DESCRIPTION
    **不執行 verify-dragdpi-esc.ps1**（它會啟動宿主、注入滑鼠與 Esc）：以 PowerShell Parser 只取出純
    函式，餵合成的矩形與宿主記錄解析結果；其餘以語法樹做結構檢查。

    fix F7（使用者 2026-10-03 決定方案 A：不支援 Esc 取消拖曳）：
      - 反悔＝拖回起點放開：延後判定那行＝開始那行、記錄有「回到拖曳開始的位置」、設定不變、視窗回原位；
      - Esc 只做觀察（INFO），不得出現在 PASS／FAIL 的判定鍵；
      - 送 Esc（按下）只能在 `Test-SafeForeground` 為真的分支內。

.EXAMPLE
    pwsh -NoProfile -File host/tools/tests/VerifyDragDpiEsc.Tests.ps1
    結束碼 0＝全部通過、1＝有失敗。
#>
param([string]$Target)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$toolsDir = Split-Path $PSScriptRoot -Parent
if (-not $Target) { $Target = Join-Path $toolsDir 'verify-dragdpi-esc.ps1' }

$script:Pass = 0
$script:Fail = 0
function Check([string]$name, [bool]$cond, [string]$detail = '') {
    if ($cond) { $script:Pass++; Write-Host "PASS  $name" }
    else { $script:Fail++; Write-Host "FAIL  $name $detail" -ForegroundColor Red }
}

$tokens = $null
$errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile($Target, [ref]$tokens, [ref]$errors)
Check 'verify-dragdpi-esc.ps1 解析 0 錯誤' (@($errors).Count -eq 0) (($errors | ForEach-Object { $_.Message }) -join ' | ')
foreach ($name in 'Same', 'Get-EscObservation', 'Get-ReturnVerdict') {
    $fn = $ast.Find({
            param($n)
            $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $name
        }, $true)
    Check "找得到函式 $name" ($null -ne $fn)
    if ($fn) { . ([scriptblock]::Create($fn.Extent.Text)) }
}

function Rect4([int]$x, [int]$y, [int]$w, [int]$h) { [PSCustomObject]@{ X = $x; Y = $y; W = $w; H = $h } }
function TrioLine($r) { [PSCustomObject]@{ Line = 'x'; R = $r } }

# ── Get-EscObservation ────────────────────────────────────────────────────────
if (Get-Command Get-EscObservation -ErrorAction SilentlyContinue) {
    $held = Rect4 800 43 1280 435
    $o = Get-EscObservation $held @((Rect4 800 43 1280 435), (Rect4 800 43 1280 435))
    Check 'Esc 後取樣都等於按住位置 → Stayed=True' ($o.Stayed -eq $true) "got=$($o.Stayed)"
    $o = Get-EscObservation $held @((Rect4 800 43 1280 435), (Rect4 1200 43 1280 435))
    Check 'Esc 後有取樣回到別處 → Stayed=False' ($o.Stayed -eq $false) "got=$($o.Stayed)"
    $o = Get-EscObservation $held @()
    Check '沒有 Esc 後取樣（未送 Esc）→ Stayed=$null' ($null -eq $o.Stayed) "got=$($o.Stayed)"
    Check '觀察文字含 INFO 性質描述' ([bool]($o.Text)) "got=$($o.Text)"
}

# ── Get-ReturnVerdict ─────────────────────────────────────────────────────────
if (Get-Command Get-ReturnVerdict -ErrorAction SilentlyContinue) {
    $start = Rect4 1200 43 1280 435
    $mid = Rect4 800 43 1280 435
    $okTrio = [PSCustomObject]@{ Enter = (TrioLine $start); Exit = (TrioLine $start); Posted = (TrioLine $start); Cancel = $true }
    $v = Get-ReturnVerdict $okTrio $start $start '{"col":15}' '{"col":15}'
    Check '拖回起點放開：全部判定 True' (@($v.Values | Where-Object { -not $_ }).Count -eq 0) (($v.GetEnumerator() | ForEach-Object { "$($_.Key)=$($_.Value)" }) -join '; ')
    Check '判定至少 6 項' ($v.Count -ge 6) "got=$($v.Count)"
    Check '判定鍵不提 Esc' (@($v.Keys | Where-Object { $_ -match 'Esc' }).Count -eq 0)

    $badPosted = [PSCustomObject]@{ Enter = (TrioLine $start); Exit = (TrioLine $mid); Posted = (TrioLine $mid); Cancel = $false }
    $v = Get-ReturnVerdict $badPosted $start $mid '{"col":15}' '{"col":10}'
    Check '放在中途：延後判定≠開始、設定變、視窗不在原位 → 至少 4 項 False' (@($v.Values | Where-Object { -not $_ }).Count -ge 4) (($v.GetEnumerator() | ForEach-Object { "$($_.Key)=$($_.Value)" }) -join '; ')

    $v = Get-ReturnVerdict $okTrio $start $start '{"col":15}' '{"col":16}'
    Check '只有設定改變 → 恰好 1 項 False' (@($v.Values | Where-Object { -not $_ }).Count -eq 1)
    $v = Get-ReturnVerdict $okTrio $start (Rect4 1201 43 1280 435) '{"col":15}' '{"col":15}'
    Check '視窗差 1 px 不算回原位 → 恰好 1 項 False' (@($v.Values | Where-Object { -not $_ }).Count -eq 1)

    $missing = [PSCustomObject]@{ Enter = (TrioLine $start); Exit = $null; Posted = (TrioLine $start); Cancel = $true }
    $v = Get-ReturnVerdict $missing $start $start '{"col":15}' '{"col":15}'
    Check '缺「結束」那行 → 記錄不完整項 False' (@($v.Values | Where-Object { -not $_ }).Count -ge 1)
    $noCancel = [PSCustomObject]@{ Enter = (TrioLine $start); Exit = (TrioLine $start); Posted = (TrioLine $start); Cancel = $false }
    $v = Get-ReturnVerdict $noCancel $start $start '{"col":15}' '{"col":15}'
    Check '缺「回到拖曳開始的位置」行 → 恰好 1 項 False' (@($v.Values | Where-Object { -not $_ }).Count -eq 1)
}

# ── 結構：Esc 不計分、只在安全前景送 ─────────────────────────────────────────
$resultKeys = @($ast.FindAll({
            param($n)
            $n -is [System.Management.Automation.Language.IndexExpressionAst] -and
            $n.Target.Extent.Text -eq '$results'
        }, $true) | ForEach-Object { $_.Index.Extent.Text })
Check '有 $results 判定鍵' ($resultKeys.Count -gt 0)
$escKeys = @($resultKeys | Where-Object { $_ -match 'Esc' })
Check '$results 判定鍵不含 Esc（Esc 只記 INFO）' ($escKeys.Count -eq 0) ($escKeys -join ' | ')

$infoKeys = @($ast.FindAll({
            param($n)
            $n -is [System.Management.Automation.Language.IndexExpressionAst] -and
            $n.Target.Extent.Text -eq '$info'
        }, $true))
Check '有 $info 觀察記錄' ($infoKeys.Count -gt 0)
Check '摘要輸出 INFO 行' ($ast.Extent.Text -match '"INFO\s')

$escPress = @($ast.FindAll({
            param($n)
            $n -is [System.Management.Automation.Language.CommandAst] -and
            $n.GetCommandName() -eq 'Invoke-GuardedKey' -and
            $n.Extent.Text -match '0x1B' -and $n.Extent.Text -notmatch '-Up\b'
        }, $true))
Check '有送 Esc 按下的呼叫' ($escPress.Count -gt 0)
foreach ($c in $escPress) {
    $p = $c.Parent; $guarded = $false
    while ($p) {
        if ($p -is [System.Management.Automation.Language.IfStatementAst]) {
            foreach ($clause in $p.Clauses) {
                if ($clause.Item1.Extent.Text -match 'Test-SafeForeground' -and
                    $c.Extent.StartOffset -ge $clause.Item2.Extent.StartOffset -and
                    $c.Extent.EndOffset -le $clause.Item2.Extent.EndOffset) { $guarded = $true }
            }
        }
        $p = $p.Parent
    }
    Check "Esc 按下（第 $($c.Extent.StartLineNumber) 行）在 Test-SafeForeground 分支內" $guarded
}

$n3Assign = $ast.Find({
        param($n)
        $n -is [System.Management.Automation.Language.AssignmentStatementAst] -and $n.Left.Extent.Text -eq '$n3'
    }, $true)
Check 'c 的記錄起點 $n3 有指派（不得未定義）' ($null -ne $n3Assign)

Write-Host "`n$($script:Pass) passed, $($script:Fail) failed"
if ($script:Fail -gt 0) { exit 1 }
exit 0
