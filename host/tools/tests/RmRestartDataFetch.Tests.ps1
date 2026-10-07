<#
.SYNOPSIS
    host/tools/rm-restart-test.ps1 放進真正路徑的測試專用設定檔必須寫 `"data_fetch": "off"`
    （純 PowerShell 斷言，不需 Pester）。

.DESCRIPTION
    **不執行 rm-restart-test.ps1**（它會啟動宿主、操作 Restart Manager）：以 PowerShell Parser
    只取出 ConvertTo-RestartTestLayout 純函式呼叫，並檢查腳本寫入真正 settings.json 的每一處都
    經過它。

    背景（data-layer-rust design.md D10、tasks.md 5.3 對照項）：`data_fetch` 預設 `auto`，以
    `LOCALAPPDATA` 與 `SHGetKnownFolderPath` 是否一致判斷隔離環境。RmRestart 重啟出的新行程
    不繼承呼叫端的環境變數、拿到真實環境，會被判成「非隔離」而開始抓取，覆寫本腳本搬進真實
    路徑的樣本，所以腳本放入的設定檔要明寫 `"off"`。

.EXAMPLE
    pwsh -NoProfile -File host/tools/tests/RmRestartDataFetch.Tests.ps1
    結束碼 0＝全部通過、1＝有失敗。
#>
param([string]$Target)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$toolsDir = Split-Path $PSScriptRoot -Parent
if (-not $Target) { $Target = Join-Path $toolsDir 'rm-restart-test.ps1' }

$script:Pass = 0
$script:Fail = 0
function Check([string]$name, [bool]$cond, [string]$detail = '') {
    if ($cond) { $script:Pass++; Write-Host "PASS  $name" }
    else { $script:Fail++; Write-Host "FAIL  $name $detail" -ForegroundColor Red }
}

$tokens = $null
$errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile($Target, [ref]$tokens, [ref]$errors)
Check 'rm-restart-test.ps1 解析 0 錯誤' (@($errors).Count -eq 0) (($errors | ForEach-Object { $_.Message }) -join ' | ')

$fn = $ast.Find({
        param($n)
        $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'ConvertTo-RestartTestLayout'
    }, $true)
Check '找得到函式 ConvertTo-RestartTestLayout' ($null -ne $fn)

if ($fn) {
    . ([scriptblock]::Create($fn.Extent.Text))

    # 首次啟動產生的設定檔（新版宿主會序列化 data_fetch=auto）。
    $withKey = '{"version":2,"data_dir":null,"data_fetch":"auto","widgets":{"clock":{"enabled":true},"fixed":{"enabled":true}}}' | ConvertFrom-Json
    $out = ConvertTo-RestartTestLayout $withKey 'C:\tmp\fixture'
    $json = $out | ConvertTo-Json -Depth 20 | ConvertFrom-Json
    Check '[已有 data_fetch] 寫出的設定 data_fetch＝off' ($json.data_fetch -eq 'off') "實際=$($json.data_fetch)"
    Check '[已有 data_fetch] fixed 關閉' ($json.widgets.fixed.enabled -eq $false)
    Check '[已有 data_fetch] clock 維持開啟' ($json.widgets.clock.enabled -eq $true)
    Check '[已有 data_fetch] data_dir 指向暫存 fixture' ($json.data_dir -eq 'C:\tmp\fixture') "實際=$($json.data_dir)"

    # 舊版設定檔沒有 data_fetch 鍵：也要補上 off，不可丟例外。
    $noKey = '{"version":2,"data_dir":null,"widgets":{"clock":{"enabled":true},"fixed":{"enabled":true}}}' | ConvertFrom-Json
    $out2 = ConvertTo-RestartTestLayout $noKey 'C:\tmp\fixture'
    $json2 = $out2 | ConvertTo-Json -Depth 20 | ConvertFrom-Json
    Check '[沒有 data_fetch 鍵] 補上 data_fetch＝off' ($json2.data_fetch -eq 'off') "實際=$($json2.PSObject.Properties['data_fetch'])"
}

# 寫入真正 settings.json 的每一處（WriteAllText 目標含 $realCfgDir 'settings.json'）都要寫經
# ConvertTo-RestartTestLayout 處理過的物件；情境 A 與 B 各一處。
$writes = @($ast.FindAll({
            param($n)
            $n -is [System.Management.Automation.Language.InvokeMemberExpressionAst] -and
            $n.Member.Extent.Text -eq 'WriteAllText' -and
            $n.Extent.Text -match "realCfgDir\s+'settings\.json'"
        }, $true))
Check '情境 A 與 B 都在真正路徑寫入測試專用設定檔（2 處）' ($writes.Count -eq 2) "實際=$($writes.Count)"
foreach ($w in $writes) {
    $line = $w.Extent.StartLineNumber
    $assigned = $ast.FindAll({
            param($n)
            $n -is [System.Management.Automation.Language.AssignmentStatementAst] -and
            $n.Extent.EndLineNumber -lt $line -and
            $n.Right.Extent.Text -match 'ConvertTo-RestartTestLayout'
        }, $true)
    $argText = $w.Arguments[1].Extent.Text
    $varName = if ($argText -match '^\(\$(\w+)\s*\|\s*ConvertTo-Json') { $Matches[1] } else { '' }
    $ok = $varName -and @($assigned | Where-Object { $_.Left.Extent.Text -eq "`$$varName" }).Count -ge 1
    Check "第 $line 行寫入的 `$$varName 來自 ConvertTo-RestartTestLayout" ([bool]$ok) "引數=$argText"
}

Write-Host "`n$($script:Pass) passed, $($script:Fail) failed"
if ($script:Fail -gt 0) { exit 1 }
exit 0
