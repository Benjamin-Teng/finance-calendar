<#
.SYNOPSIS
    host/tools/lib/ProcessTree.psm1 的 mock 測試，加上「腳本不得以行程名稱停止 fc-host」的靜態檢查
    （純 PowerShell 斷言，不需 Pester）。

.DESCRIPTION
    **不停止任何真實行程**：只測純函式 Get-ProcessTreeIds（餵合成的行程清單）。靜態檢查掃描
    host/tools/ 下所有 .ps1（去掉註解後）：不得出現 `Stop-Process -Name fc-host`、
    `Get-Process … fc-host … | Stop-Process`，也不得對以名稱列舉出的 fc-host 清單逐一 Stop-Process
    （fix F4 共通規則；review B-batch3 high、B-batch2 low）。

.EXAMPLE
    pwsh -NoProfile -File host/tools/tests/ProcessTree.Tests.ps1
    結束碼 0＝全部通過、1＝有失敗。
#>
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$toolsDir = Split-Path $PSScriptRoot -Parent
Import-Module (Join-Path $toolsDir 'lib\ProcessTree.psm1') -Force

$script:Pass = 0
$script:Fail = 0
function Check([string]$name, [bool]$cond, [string]$detail = '') {
    if ($cond) { $script:Pass++; Write-Host "PASS  $name" }
    else { $script:Fail++; Write-Host "FAIL  $name $detail" -ForegroundColor Red }
}

$t0 = [datetime]'2026-10-01T10:00:00'
function P([int]$id, [int]$parent, [int]$sec) { [PSCustomObject]@{ ProcessId = $id; ParentProcessId = $parent; CreationDate = $t0.AddSeconds($sec) } }

# ---------------------------------------------------------------- 1. 樹狀收集
$procs = @(
    (P 100 4 0),     # 自己啟動的宿主
    (P 101 100 1),   # msedgewebview2 browser
    (P 102 101 2),   # renderer
    (P 103 101 2),   # gpu
    (P 200 4 5),     # 使用者自己的另一個 fc-host（不相干）
    (P 201 200 6),
    (P 300 1 0)
)
$ids = @(Get-ProcessTreeIds -RootId 100 -Processes $procs)
Check '樹：根與所有子孫都收進來（100,101,102,103）' ((($ids | Sort-Object) -join ',') -eq '100,101,102,103') ($ids -join ',')
Check '樹：根排在最前（先停根）' ($ids.Count -gt 0 -and $ids[0] -eq 100) ($ids -join ',')
Check '樹：不相干的同名行程與其子孫不收（200,201）' (-not ($ids -contains 200) -and -not ($ids -contains 201)) ($ids -join ',')

# ---------------------------------------------------------------- 2. PID 重用防護
# 子行程比父行程早建立＝父 PID 是重用的。
$procs2 = @((P 100 4 10), (P 150 100 3), (P 151 100 11))
$ids = @(Get-ProcessTreeIds -RootId 100 -Processes $procs2)
Check '重用：比根更早建立的「子行程」不收' (-not ($ids -contains 150) -and ($ids -contains 151)) ($ids -join ',')

# 根已結束、PID 被別的行程重用：根本身不收，只收原本的根留下的孤兒。
$procs3 = @((P 100 4 60), (P 110 100 5), (P 111 100 70))
$ids = @(Get-ProcessTreeIds -RootId 100 -RootStartTime $t0 -Processes $procs3)
Check '重用：根 PID 被重用時不停重用者本身' (-not ($ids -contains 100)) ($ids -join ',')
Check '重用：仍收原本的根留下的孤兒（建立於原根啟動後、重用者啟動前）' ($ids -contains 110) ($ids -join ',')
Check '重用：不收重用者的子行程' (-not ($ids -contains 111)) ($ids -join ',')

# 呼叫端確知根已結束（Process.HasExited）：同 PID 的現存行程一定是重用者。
$ids = @(Get-ProcessTreeIds -RootId 100 -RootStartTime $t0 -Processes $procs3 -RootExited)
Check '根已結束（-RootExited）：不停同 PID 的重用者、不收其子行程，只收原本的孤兒' (
    (-not ($ids -contains 100)) -and (-not ($ids -contains 111)) -and ($ids -contains 110)) ($ids -join ',')
$ids = @(Get-ProcessTreeIds -RootId 100 -Processes @((P 110 100 5)) -RootExited)
Check '根已結束且不知道啟動時間：無法確認的子行程一律不收' ($ids.Count -eq 0) ($ids -join ',')

# 根已不在清單（已結束）：仍清它留下的子孫。
$procs4 = @((P 101 100 1), (P 102 101 2))
$ids = @(Get-ProcessTreeIds -RootId 100 -RootStartTime $t0 -Processes $procs4)
Check '根已結束：仍收子孫 101、102' (($ids -contains 101) -and ($ids -contains 102)) ($ids -join ',')

# 沒有建立時間（例如測試清單或查詢失敗）：退回純父子關係。
$procs5 = @([PSCustomObject]@{ ProcessId = 100; ParentProcessId = 4 }, [PSCustomObject]@{ ProcessId = 101; ParentProcessId = 100 })
$ids = @(Get-ProcessTreeIds -RootId 100 -Processes $procs5)
Check '無建立時間：退回純父子關係' ((($ids | Sort-Object) -join ',') -eq '100,101') ($ids -join ',')

# 自我迴圈與環狀不得無窮迴圈。
$procs6 = @((P 100 100 0), (P 101 100 1), (P 102 101 2), [PSCustomObject]@{ ProcessId = 100; ParentProcessId = 102; CreationDate = $t0 })
$ids = @(Get-ProcessTreeIds -RootId 100 -Processes $procs6)
Check '環狀：不重複、不卡住' ($ids.Count -eq 3) ($ids -join ',')

# ---------------------------------------------------------------- 3. 靜態：不得以行程名稱停止 fc-host
function Get-CodeText([string]$path) {
    $t = Get-Content $path -Raw
    $t = [regex]::Replace($t, '(?s)<#.*?#>', '')
    ($t -split "`r?`n" | Where-Object { $_ -notmatch '^\s*#' }) -join "`n"
}
$nameStopPatterns = @(
    'Stop-Process[^\n]*-Name\s+[''"]?fc-host',
    'Get-Process[^\n|]*fc-host[^\n]*\|\s*Stop-Process',
    'Get-FcHostProcesses\)\)\s*\{\s*Stop-Process',
    'taskkill[^\n]*fc-host'
)
foreach ($f in Get-ChildItem $toolsDir -Filter *.ps1 -File) {
    $code = Get-CodeText $f.FullName
    $hit = @($nameStopPatterns | Where-Object { $code -match $_ } | ForEach-Object { $Matches[0] })
    Check "靜態：$($f.Name) 沒有以行程名稱停止 fc-host" ($hit.Count -eq 0) ($hit -join ' | ')
}

# ---------------------------------------------------------------- 4. 靜態：「已有 fc-host」防呆不得在 try 內
# 防呆在 try 內 throw／exit 會進入 finally 的清理（review B-batch3 high：B6／B10 因此砍掉既有宿主）。
# 判準：條件含 `Get-Process … fc-host`、本體含 throw 或 exit 的 if 敘述，不得位於任何 try 之內。
foreach ($f in Get-ChildItem $toolsDir -Filter *.ps1 -File) {
    $tk = $null; $pe = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseFile($f.FullName, [ref]$tk, [ref]$pe)
    $guards = @($ast.FindAll({
                param($n)
                $n -is [System.Management.Automation.Language.IfStatementAst] -and
                $n.Clauses.Count -ge 1 -and
                $n.Clauses[0].Item1.Extent.Text -match 'Get-Process[^\n]*fc-host' -and
                $n.Clauses[0].Item2.Extent.Text -match '\b(throw|exit)\b'
            }, $true))
    $inTry = @($guards | Where-Object {
            $p = $_.Parent
            $found = $false
            while ($p) { if ($p -is [System.Management.Automation.Language.TryStatementAst]) { $found = $true }; $p = $p.Parent }
            $found
        })
    if ($guards.Count -gt 0) {
        Check "靜態：$($f.Name) 的「已有 fc-host」防呆不在 try 之內" ($inTry.Count -eq 0) (($inTry | ForEach-Object { "line $($_.Extent.StartLineNumber)" }) -join ', ')
    }
}

Write-Host ''
Write-Host "合計：$($script:Pass) PASS、$($script:Fail) FAIL"
if ($script:Fail -gt 0) { exit 1 }
exit 0
