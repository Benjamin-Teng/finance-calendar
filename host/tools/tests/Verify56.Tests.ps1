<#
.SYNOPSIS
    host/tools/verify-5.6.ps1 的手動暫停驗收指令判定（Get-ManualPauseOutcome）mock 測試與結構檢查
    （純 PowerShell 斷言，不需 Pester）。

.DESCRIPTION
    **不執行 verify-5.6.ps1**（它會啟動宿主、結束 WebView2 行程）：以 PowerShell Parser 只取出純函式，
    餵合成的 host-cdp-eval 輸出。

    fix F7（批次 A）：正式版（不帶 self-test-ipc）沒有 `self_test_set_manual_pause`，P 項原本判 FAIL；
    改比照 verify-5.5 列 PENDING（指令存在但呼叫失敗才是 FAIL）。

.EXAMPLE
    pwsh -NoProfile -File host/tools/tests/Verify56.Tests.ps1
    結束碼 0＝全部通過、1＝有失敗。
#>
param([string]$Target)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$toolsDir = Split-Path $PSScriptRoot -Parent
if (-not $Target) { $Target = Join-Path $toolsDir 'verify-5.6.ps1' }

$script:Pass = 0
$script:Fail = 0
function Check([string]$name, [bool]$cond, [string]$detail = '') {
    if ($cond) { $script:Pass++; Write-Host "PASS  $name" }
    else { $script:Fail++; Write-Host "FAIL  $name $detail" -ForegroundColor Red }
}

$tokens = $null
$errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile($Target, [ref]$tokens, [ref]$errors)
Check 'verify-5.6.ps1 解析 0 錯誤' (@($errors).Count -eq 0) (($errors | ForEach-Object { $_.Message }) -join ' | ')
$fn = $ast.Find({
        param($n)
        $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'Get-ManualPauseOutcome'
    }, $true)
Check '找得到函式 Get-ManualPauseOutcome' ($null -ne $fn)
if ($fn) {
    . ([scriptblock]::Create($fn.Extent.Text))
    Check '結束碼 0、輸出 true → ok' ((Get-ManualPauseOutcome 0 'true') -eq 'ok')
    Check '結束碼 0、輸出 true（前後空白）→ ok' ((Get-ManualPauseOutcome 0 " true`n") -eq 'ok')
    Check 'Tauri：Command self_test_set_manual_pause not found → missing' ((Get-ManualPauseOutcome 1 'Error: Command self_test_set_manual_pause not found') -eq 'missing')
    # fix F7b：只認 Tauri 2.12 的精確形式（tauri-2.12.0/src/webview/mod.rs：
    # `resolver.reject(format!("Command {command} not found"))`），其餘「找不到／not found」一律 FAIL。
    Check 'Tauri 原樣字串（Uncaught 包裝）→ missing' ((Get-ManualPauseOutcome 1 "Uncaught Command self_test_set_manual_pause not found") -eq 'missing')
    Check 'host-cdp-eval：分頁不存在 → failed（不得當 PENDING 掩蓋）' ((Get-ManualPauseOutcome 1 '找不到 url 含「w=quotes」的分頁；現有：tauri://localhost/?w=clock') -eq 'failed')
    Check '中文「找不到」（非 Tauri 形式）→ failed' ((Get-ManualPauseOutcome 1 '找不到指令 self_test_set_manual_pause') -eq 'failed')
    Check 'Tauri：別的指令 not found → failed' ((Get-ManualPauseOutcome 1 'Error: Command get_pause not found') -eq 'failed')
    Check 'window／webview not found → failed' ((Get-ManualPauseOutcome 1 'Error: webview not found') -eq 'failed')
    Check 'window not found → failed' ((Get-ManualPauseOutcome 1 'window not found') -eq 'failed')
    Check '指令存在但回 false → failed（照 FAIL）' ((Get-ManualPauseOutcome 0 'false') -eq 'failed')
    Check 'CDP 連線失敗等其他錯誤 → failed（不得當 PENDING 掩蓋）' ((Get-ManualPauseOutcome 1 'Error: connect ECONNREFUSED 127.0.0.1:9351') -eq 'failed')
    Check '沒有輸出 → failed' ((Get-ManualPauseOutcome 1 '') -eq 'failed')
}

# 結構：missing 時寫 $pending，不寫 $results。
$text = $ast.Extent.Text -replace "`r`n", "`n"
Check 'missing 分支寫入 $pending（P 項）' ($text -match "(?s)'missing'.{0,400}\`$pending\['P ")
Check '不再以「回傳不是 true」一律記 FAIL（舊判斷式已移除）' ($text -notmatch 'if \(\$pauseResult -ne \$true\)')
Check '指令存在卻失敗（failed）仍記 FAIL' ($text -match "(?s)elseif \(\`$pauseOutcome -ne 'ok'\).{0,200}\`$results\['P 手動暫停（驗收用指令）生效'\] = \`$false")

Write-Host "`n$($script:Pass) passed, $($script:Fail) failed"
if ($script:Fail -gt 0) { exit 1 }
exit 0
