<#
.SYNOPSIS
    repro-soak-exit.ps1 的純函式 mock 測試與結構檢查（純 PowerShell 斷言，不需 Pester）。

.DESCRIPTION
    **不執行腳本**（它會啟動宿主、終止／當掉 WebView2 子行程）：以 PowerShell Parser 只取出純函式，
    餵合成的行程清單；其餘以語法樹做結構檢查。

    fix F5（review fix-soak）：
      - low：行程樹改用 lib/ProcessTree.psm1（有 PID 重用防護），不再自行沿 ParentProcessId 往下走；
        -OpenSettings 啟動的第二個行程收尾時一律以 Stop-ProcessTree 停掉。

.EXAMPLE
    pwsh -NoProfile -File host/tools/tests/ReproSoakExit.Tests.ps1
    結束碼 0＝全部通過、1＝有失敗。
#>
param([string]$Target)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$toolsDir = Split-Path $PSScriptRoot -Parent
if (-not $Target) { $Target = Join-Path $toolsDir 'repro-soak-exit.ps1' }
Import-Module (Join-Path $toolsDir 'lib\ProcessTree.psm1') -Force

$script:Pass = 0
$script:Fail = 0
function Check([string]$name, [bool]$cond, [string]$detail = '') {
    if ($cond) { $script:Pass++; Write-Host "PASS  $name" }
    else { $script:Fail++; Write-Host "FAIL  $name $detail" -ForegroundColor Red }
}

$tokens = $null; $errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile($Target, [ref]$tokens, [ref]$errors)
Check 'repro-soak-exit.ps1 解析 0 錯誤' (@($errors).Count -eq 0) (($errors | ForEach-Object { $_.Message }) -join ' | ')
$missing = @()
foreach ($name in @('Select-TreeProcesses', 'Select-CdpPort', 'Get-CdpOwnerVerdict')) {
    $fn = $ast.Find({ param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $name }, $true)
    if ($fn) { . ([scriptblock]::Create($fn.Extent.Text)) } else { $missing += $name }
}
Check '找得到純函式' ($missing.Count -eq 0) "缺：$($missing -join ',')"
# 去掉註解後的腳本文字（結構檢查用）。
$code = (($ast.Extent.Text -split "`r?`n") | Where-Object { $_ -notmatch '^\s*#' }) -join "`n"

# ---------------------------------------------------------------- 行程樹（low）
$t0 = [datetime]'2026-10-01T10:00:00'
function P([int]$id, [int]$parent, [int]$sec, [string]$name = 'msedgewebview2.exe') {
    [PSCustomObject]@{ ProcessId = $id; ParentProcessId = $parent; CreationDate = $t0.AddSeconds($sec); Name = $name; CommandLine = "x --type=renderer" }
}
if (Get-Command Select-TreeProcesses -ErrorAction SilentlyContinue) {
    $procs = @(
        (P 100 4 0 'fc-host.exe'),
        (P 101 100 1),
        (P 102 101 2),
        (P 103 4 0 'other.exe'),
        # PID 重用：父 PID 101 是後來才出現的行程，102 建立時間早於它的「父」→ 不列入。
        (P 104 103 1),
        # 子行程建立時間早於宿主＝宿主 PID 被重用前留下的不相干行程，不列入。
        (P 105 100 -30)
    )
    $sel = @(Select-TreeProcesses -All $procs -RootId 100 -RootStartTime $t0)
    $ids = @($sel | ForEach-Object { [int]$_.ProcessId } | Sort-Object)
    Check '行程樹：只收宿主的子孫、不含宿主本身' (($ids -join ',') -eq '101,102') ($ids -join ',')
    Check '行程樹：回傳的是原物件（保留 CommandLine）' (@($sel | Where-Object { $_.CommandLine }).Count -eq $sel.Count)
    Check '行程樹：建立時間早於宿主的子行程不列入' (-not ($ids -contains 105))
}
Check '結構：不再自行實作 Get-Descendants' (-not ($code -match 'function\s+Get-Descendants'))
Check '結構：匯入 lib\ProcessTree.psm1' ($code -match "Import-Module\s+\(Join-Path\s+\`$PSScriptRoot\s+'lib\\ProcessTree\.psm1'\)")
Check '結構：Select-TreeProcesses 以 Get-ProcessTreeIds 計算' (
    $null -ne ($ast.Find({ param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'Select-TreeProcesses' }, $true)) -and
    ($ast.Find({ param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'Select-TreeProcesses' }, $true).Extent.Text -match 'Get-ProcessTreeIds'))

# -OpenSettings 的第二個行程：finally 一律 Stop-ProcessTree（只停自己啟動的）。
$tries = @($ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.TryStatementAst] -and $n.Finally }, $true))
$mainFinally = @($tries | Where-Object { $_.Finally.Extent.Text -match 'Restore-FcHostAutostartRegistry' })
Check '結構：主 finally 存在' ($mainFinally.Count -eq 1)
if ($mainFinally.Count -eq 1) {
    $fin = (($mainFinally[0].Finally.Extent.Text -split "`r?`n") | Where-Object { $_ -notmatch '^\s*#' }) -join "`n"
    Check '結構：finally 以 Stop-ProcessTree 停第二個行程' ($fin -match 'Stop-ProcessTree\s+-Process\s+\$second')
    Check '結構：finally 以 Stop-ProcessTree 停測試宿主' ($fin -match 'Stop-ProcessTree\s+-Process\s+\$hostProc')
    Check '結構：finally 不以名稱停 fc-host' (-not ($fin -match 'Stop-Process\s+-Name'))
}
Check '結構：$second 在 try 之前初始化為 $null（finally 可安全判斷）' ($code -match '(?m)^\$second\s*=\s*\$null')

# ---------------------------------------------------------------- -RealCrash 的 CDP 埠（high）
if (Get-Command Select-CdpPort -ErrorAction SilentlyContinue) {
    Check 'CDP 埠：偏好埠空著就用它' ((Select-CdpPort -Preferred 9351 -BusyPorts @()) -eq 9351)
    Check 'CDP 埠：偏好埠被佔用就換下一個空的' ((Select-CdpPort -Preferred 9351 -BusyPorts @(9351, 9352)) -eq 9353)
    Check 'CDP 埠：範圍內全被佔用 → $null（呼叫端中止）' ($null -eq (Select-CdpPort -Preferred 9351 -BusyPorts @(9351..9370)))
}
if (Get-Command Get-CdpOwnerVerdict -ErrorAction SilentlyContinue) {
    $tree = @(100, 101, 102)
    Check 'CDP 擁有者：Listen 行程是宿主子孫 → OK' ((Get-CdpOwnerVerdict -ListenerPids @(101) -TreeIds $tree) -eq 'OK')
    Check 'CDP 擁有者：沒有 Listen → NO-LISTENER' ((Get-CdpOwnerVerdict -ListenerPids @() -TreeIds $tree) -eq 'NO-LISTENER')
    Check 'CDP 擁有者：Listen 行程不在宿主樹 → FOREIGN' ((Get-CdpOwnerVerdict -ListenerPids @(555) -TreeIds $tree) -eq 'FOREIGN')
    Check 'CDP 擁有者：有一個不在宿主樹也是 FOREIGN' ((Get-CdpOwnerVerdict -ListenerPids @(101, 555) -TreeIds $tree) -eq 'FOREIGN')
    Check 'CDP 擁有者：宿主樹為空 → FOREIGN' ((Get-CdpOwnerVerdict -ListenerPids @(101) -TreeIds @()) -eq 'FOREIGN')
}
function Get-FunctionText([string]$name) {
    $fn = $ast.Find({ param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $name }, $true)
    if (-not $fn) { return '' }
    return (($fn.Extent.Text -split "`r?`n") | Where-Object { $_ -notmatch '^\s*#' }) -join "`n"
}
$listenText = Get-FunctionText 'Get-ListenPids'
Check '結構：以 Get-NetTCPConnection -State Listen 取埠的 OwningProcess' ($listenText -match 'Get-NetTCPConnection\s+-LocalPort\s+\$Port\s+-State\s+Listen' -and $listenText -match 'OwningProcess')
$assertText = Get-FunctionText 'Assert-CdpOwnedByHost'
Check '結構：擁有者檢查以宿主 PID＋啟動時間算行程樹' ($assertText -match 'Get-ProcessTreeIds\s+-RootId\s+\$script:hostPid\s+-RootStartTime\s+\$script:hostStart')
Check '結構：擁有者檢查不是 OK 就 throw' ($assertText -match "-ne\s+'OK'" -and $assertText -match '\bthrow\b')
$crashText = Get-FunctionText 'Crash-Cdp'
$assertAt = $crashText.IndexOf('Assert-CdpOwnedByHost')
$nodeAt = $crashText.IndexOf('& node')
Check '結構：Crash-Cdp 先檢查擁有者才呼叫 node' ($assertAt -ge 0 -and $nodeAt -gt $assertAt) "assert=$assertAt node=$nodeAt"
Check '結構：Crash-Cdp 檢查 node 結束碼' ($crashText -match '\$code\s*=\s*\$LASTEXITCODE' -and $crashText -match 'if\s+\(\$code\s+-ne\s+0\)\s+\{\s*throw')
# 啟動前：-RealCrash 時先選空的埠，選不到就 throw；選埠必須在啟動宿主之前。
$selectAt = $code.IndexOf('Select-CdpPort -Preferred')
$startAt = $code.IndexOf('$hostProc = Start-WithTempEnv')
Check '結構：啟動宿主前先選空的 CDP 埠' ($selectAt -ge 0 -and $startAt -gt $selectAt) "select=$selectAt start=$startAt"
Check '結構：選不到空埠就中止' ($code -match '(?s)Select-CdpPort -Preferred.{0,400}\$null -eq \$port.{0,200}throw')

Write-Host ''
Write-Host "通過 $script:Pass、失敗 $script:Fail"
if ($script:Fail -gt 0) { exit 1 }
exit 0
