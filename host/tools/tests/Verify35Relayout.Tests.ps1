<#
.SYNOPSIS
    host/tools/verify-3.5.ps1 的 RELAYOUT 記錄解析函式（Get-RelayoutApplied、Get-LineTime）測試
    （純 PowerShell 斷言，不需 Pester）。

.DESCRIPTION
    **不執行 verify-3.5.ps1**（它會啟動宿主、送視窗訊息）：以 PowerShell Parser 解析腳本，只把
    兩個函式定義取出來在本測試的範圍內定義，再餵假記錄行。記錄行格式比照
    `host/src/desktop.rs` 的 `gatekeeper_log`（`<時間戳> <訊息>`）與
    `RELAYOUT reason={} applied={count}`。

    fix F2b：F2 寫入時反斜線遺失，regex 變成 `'applied=(d+)'`，只會比對字面上的字母 d，
    `Get-RelayoutApplied` 一律回 -1，兩個「applied ≥ 1」判準永遠 FAIL。

.EXAMPLE
    pwsh -NoProfile -File host/tools/tests/Verify35Relayout.Tests.ps1
    結束碼 0＝全部通過、1＝有失敗。
#>
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$toolsDir = Split-Path $PSScriptRoot -Parent
$target = Join-Path $toolsDir 'verify-3.5.ps1'

$script:Pass = 0
$script:Fail = 0
function Check([string]$name, [bool]$cond, [string]$detail = '') {
    if ($cond) { $script:Pass++; Write-Host "PASS  $name" }
    else { $script:Fail++; Write-Host "FAIL  $name $detail" -ForegroundColor Red }
}

# 只取函式定義，不執行腳本本體。
$tokens = $null
$errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile($target, [ref]$tokens, [ref]$errors)
Check 'verify-3.5.ps1 解析 0 錯誤' (@($errors).Count -eq 0) (($errors | ForEach-Object { $_.Message }) -join ' | ')
foreach ($name in 'Get-RelayoutApplied', 'Get-LineTime') {
    $fn = $ast.Find({
            param($n)
            $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $name
        }, $true)
    Check "找得到函式 $name" ($null -ne $fn)
    if ($fn) { . ([scriptblock]::Create($fn.Extent.Text)) }
}

$line1 = '2026-10-01T09:15:42.123 RELAYOUT reason=display-change applied=3'
$line2 = '2026-10-01T09:15:42.987 RELAYOUT reason=display-change,workarea-change applied=12'
$line0 = '2026-10-01T09:15:43.000 RELAYOUT reason=dpi-changed applied=0'
$lineOld = '2026-10-01T09:15:44.000 RELAYOUT reason=display-change'

Check 'applied=3 → 3' ((Get-RelayoutApplied $line1) -eq 3) "got=$(Get-RelayoutApplied $line1)"
Check 'applied=12（多位數、多原因）→ 12' ((Get-RelayoutApplied $line2) -eq 12) "got=$(Get-RelayoutApplied $line2)"
Check 'applied=0 → 0（不是 -1）' ((Get-RelayoutApplied $line0) -eq 0) "got=$(Get-RelayoutApplied $line0)"
Check '缺 applied 欄位 → -1' ((Get-RelayoutApplied $lineOld) -eq -1) "got=$(Get-RelayoutApplied $lineOld)"
Check '字面 applied=d 不算數字 → -1' ((Get-RelayoutApplied 'RELAYOUT applied=d') -eq -1)

$t = Get-LineTime $line1
Check 'Get-LineTime 解析毫秒時間戳' ($t -eq [datetime]::new(2026, 10, 1, 9, 15, 42, 123)) "got=$t"

Write-Host "`n$($script:Pass) passed, $($script:Fail) failed"
if ($script:Fail -gt 0) { exit 1 }
exit 0
