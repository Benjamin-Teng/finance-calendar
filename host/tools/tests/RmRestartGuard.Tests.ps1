<#
.SYNOPSIS
    host/tools/rm-restart-test.ps1 的真實資料保護（Enter-／Exit-RealDataGuard）mock 測試
    （純 PowerShell 斷言，不需 Pester）。

.DESCRIPTION
    **不執行 rm-restart-test.ps1**（它會啟動宿主、操作 Restart Manager）：以 PowerShell Parser
    只取出 Enter-RealDataGuard、Exit-RealDataGuard、Remove-WithRetry 三個函式，把 `$RealItems`
    指向 %TEMP% 底下的假目錄後呼叫。**不碰使用者真正的 %APPDATA%／%LOCALAPPDATA%**。

    fix F2b（重審 M2）：中途某一項搬不動（被其他行程持有控制代碼）時，已搬走的項目必須被還原；
    搬不動的那一項不得被當成「測試期間新生成的內容」刪掉；未還原的項目要進 `$NotRestored`。

    -Target 可指向其他版本的腳本（例如 `git show eb637e2:host/tools/rm-restart-test.ps1` 存出的
    檔案），用來確認修正前的版本會失敗。

.EXAMPLE
    pwsh -NoProfile -File host/tools/tests/RmRestartGuard.Tests.ps1
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
foreach ($name in 'Enter-RealDataGuard', 'Exit-RealDataGuard', 'Remove-WithRetry') {
    $fn = $ast.Find({
            param($n)
            $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $name
        }, $true)
    Check "找得到函式 $name" ($null -ne $fn)
    if ($fn) { . ([scriptblock]::Create($fn.Extent.Text)) }
}

$script:WLog = New-Object System.Collections.Generic.List[string]
function W([string]$m) { $script:WLog.Add($m) }

# 假的真實資料：AppData\tw.fintools.fc-host\{settings.json,gatekeeper.log}、
# Local\tw.fintools.fc-host\{data,logs}\…（與腳本的 $RealItems 同結構）。
function New-FakeRealData {
    $root = Join-Path ([IO.Path]::GetTempPath()) "fc-rmguard-$([guid]::NewGuid().ToString('N'))"
    $cfg = Join-Path (Join-Path $root 'Roaming') 'tw.fintools.fc-host'
    $loc = Join-Path (Join-Path $root 'Local') 'tw.fintools.fc-host'
    New-Item -ItemType Directory -Force -Path $cfg, (Join-Path $loc 'data'), (Join-Path $loc 'logs') | Out-Null
    Set-Content -LiteralPath (Join-Path $cfg 'settings.json') -Value '{"user":"original"}' -NoNewline
    Set-Content -LiteralPath (Join-Path $cfg 'gatekeeper.log') -Value 'gk-original' -NoNewline
    Set-Content -LiteralPath (Join-Path (Join-Path $loc 'data') 'tw_events.json') -Value 'data-original' -NoNewline
    Set-Content -LiteralPath (Join-Path (Join-Path $loc 'logs') 'fc-host.log') -Value 'log-original' -NoNewline
    [PSCustomObject]@{
        Root  = $root
        Cfg   = $cfg
        Items = @(
            (Join-Path $cfg 'settings.json'),
            (Join-Path $cfg 'gatekeeper.log'),
            (Join-Path $loc 'data'),
            (Join-Path $loc 'logs')
        )
    }
}
function Get-Backups($Fake) {
    @(Get-ChildItem -LiteralPath $Fake.Root -Recurse -Force | Where-Object { $_.Name -like '*.rmtest-bak-*' })
}

# ── 1. 正常：四項都搬走，收尾全部還原 ──────────────────────────────────────────────────
$fake = New-FakeRealData
$RealItems = $fake.Items
$realCfgDir = $fake.Cfg
$NotRestored = New-Object System.Collections.Generic.List[string]
try {
    $g = Enter-RealDataGuard 'T1'
    Check '[正常] 四項都已搬走' (-not ($fake.Items | Where-Object { Test-Path $_ }))
    Check '[正常] guard 記錄四項備份' (@($g.Items | Where-Object { $_.Backup }).Count -eq 4)
    Exit-RealDataGuard $g
    Check '[正常] 收尾後四項都回到原位' (-not ($fake.Items | Where-Object { -not (Test-Path $_) }))
    Check '[正常] settings.json 內容是原檔' ((Get-Content -LiteralPath $fake.Items[0] -Raw) -eq '{"user":"original"}')
    Check '[正常] 沒有殘留備份' (@(Get-Backups $fake).Count -eq 0)
    Check '[正常] 未還原清單為空' ($NotRestored.Count -eq 0) ($NotRestored -join ' | ')
} finally { Remove-Item -Recurse -Force -LiteralPath $fake.Root -ErrorAction SilentlyContinue }

# ── 2. 第 2 項（gatekeeper.log）被其他行程獨占開啟、搬不動 ─────────────────────────────
$fake = New-FakeRealData
$RealItems = $fake.Items
$realCfgDir = $fake.Cfg
$NotRestored = New-Object System.Collections.Generic.List[string]
$lock = [IO.File]::Open($fake.Items[1], [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::None)
try {
    $g = $null
    $threw = $false
    try { $g = Enter-RealDataGuard 'T2' } catch { $threw = $true }
    Check '[搬不動] Enter-RealDataGuard 丟例外（情境記為未跑完）' $threw
    $lock.Dispose(); $lock = $null
    # 呼叫端 finally 會照常呼叫 Exit-RealDataGuard（$g 為 $null 時直接返回）。
    Exit-RealDataGuard $g
    Check '[搬不動] 已搬走的 settings.json 已還原到原位' (Test-Path -LiteralPath $fake.Items[0])
    Check '[搬不動] settings.json 內容是原檔' ((Test-Path -LiteralPath $fake.Items[0]) -and
        (Get-Content -LiteralPath $fake.Items[0] -Raw) -eq '{"user":"original"}')
    Check '[搬不動] 搬不動的 gatekeeper.log 沒被刪、內容不變' ((Test-Path -LiteralPath $fake.Items[1]) -and
        (Get-Content -LiteralPath $fake.Items[1] -Raw) -eq 'gk-original')
    Check '[搬不動] 後續項目 data／logs 沒被動過' ((Test-Path -LiteralPath (Join-Path $fake.Items[2] 'tw_events.json')) -and
        (Test-Path -LiteralPath (Join-Path $fake.Items[3] 'fc-host.log')))
    Check '[搬不動] 沒有殘留備份' (@(Get-Backups $fake).Count -eq 0) ((Get-Backups $fake | ForEach-Object Name) -join ',')
    Check '[搬不動] 全部還原時未還原清單為空（R 判定與實況一致）' ($NotRestored.Count -eq 0) ($NotRestored -join ' | ')
} finally {
    if ($lock) { $lock.Dispose() }
    Remove-Item -Recurse -Force -LiteralPath $fake.Root -ErrorAction SilentlyContinue
}

# ── 3. 搬不動後回滾時，已搬走的項目也還原不了 → 必須列入未還原清單（R 不得 PASS） ────────
# 模擬：settings.json 搬走後，原位置被別的行程重建並獨占開啟（回滾時刪不掉）。
$fake = New-FakeRealData
$RealItems = $fake.Items
$realCfgDir = $fake.Cfg
$NotRestored = New-Object System.Collections.Generic.List[string]
$lock = [IO.File]::Open($fake.Items[1], [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::None)
$squatter = $null
# 改名一發生就重建 settings.json 並獨占開啟：用 FileSystemWatcher 不穩，改包一層 Rename-Item。
function Rename-Item {
    param([string]$Path, [string]$NewName, $ErrorAction)
    Microsoft.PowerShell.Management\Rename-Item -Path $Path -NewName $NewName -ErrorAction Stop
    if ($Path -eq $fake.Items[0] -and -not $script:squatter) {
        $script:squatter = [IO.File]::Open($Path, [IO.FileMode]::CreateNew, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
    }
}
try {
    $g = $null
    try { $g = Enter-RealDataGuard 'T3' } catch { Write-Verbose "預期的例外：$($_.Exception.Message)" }
    Exit-RealDataGuard $g
    Check '[回滾失敗] 未還原清單列出 settings.json 與其備份路徑' (
        @($NotRestored | Where-Object { $_ -like "*$($fake.Items[0])*" -and $_ -like '*rmtest-bak-T3*' }).Count -ge 1
    ) ($NotRestored -join ' | ')
    Check '[回滾失敗] 備份仍在（可供手動還原）' (@(Get-Backups $fake).Count -ge 1)
} finally {
    Remove-Item Function:\Rename-Item -ErrorAction SilentlyContinue
    if ($squatter) { $squatter.Dispose() }
    if ($lock) { $lock.Dispose() }
    Remove-Item -Recurse -Force -LiteralPath $fake.Root -ErrorAction SilentlyContinue
}

Write-Host "`n$($script:Pass) passed, $($script:Fail) failed"
if ($script:Fail -gt 0) { exit 1 }
exit 0
