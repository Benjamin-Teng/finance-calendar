<#
.SYNOPSIS
    host/tools/lib/AutostartRegistry.psm1 的 mock 測試（純 PowerShell 斷言，不需 Pester）。

.DESCRIPTION
    讀寫後端以 Set-FcHostRegistryBackend 換成記憶體內的假登錄：**本測試不讀寫任何真正的登錄**
    （fix F2 硬限制：不得碰使用者的 HKCU Run／StartupApproved）。

    另做靜態檢查：host/tools/ 下每支會啟動 fc-host 的腳本（以 `Start-Process -FilePath $Exe`
    `$script:Exe`、rm-restart-test 的 `$TestExe`，或 verify-5.1-concurrent 的
    `CreateProcessW($script:Exe` 啟動宿主者）都匯入本模組、呼叫 Save-／Restore-。

.EXAMPLE
    pwsh -NoProfile -File host/tools/tests/AutostartRegistry.Tests.ps1
    結束碼 0＝全部通過、1＝有失敗。
#>
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$toolsDir = Split-Path $PSScriptRoot -Parent
Import-Module (Join-Path $toolsDir 'lib\AutostartRegistry.psm1') -Force

$script:Pass = 0
$script:Fail = 0
function Check([string]$name, [bool]$cond, [string]$detail = '') {
    if ($cond) { $script:Pass++; Write-Host "PASS  $name" }
    else { $script:Fail++; Write-Host "FAIL  $name $detail" -ForegroundColor Red }
}

$RunKey = 'Software\Microsoft\Windows\CurrentVersion\Run'
$ApprovedKey = 'Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run'

# 假登錄：鍵 "<Key>|<Name>" → @{ Kind; Value }。$script:FailSetFor 指定的鍵寫入時丟例外。
$script:Fake = @{}
$script:FailSetFor = @()
function New-FakeBackend {
    @{
        Get    = { param($Key, $Name) $script:Fake["$Key|$Name"] }
        Set    = {
            param($Key, $Name, $Kind, $Value)
            if ($script:FailSetFor -contains "$Key|$Name") { throw '模擬寫入失敗' }
            $script:Fake["$Key|$Name"] = @{ Kind = $Kind; Value = $Value }
        }
        Remove = { param($Key, $Name) $script:Fake.Remove("$Key|$Name") }
    }
}
Set-FcHostRegistryBackend (New-FakeBackend)

# ── 1. 原本兩個值都不存在：宿主寫入後，還原＝刪除 ─────────────────────────────────────
$script:Fake = @{}
$snap = @(Save-FcHostAutostartRegistry)
Check '快照有兩項（Run、StartupApproved）' ($snap.Count -eq 2)
Check '原本不存在 → Exists=false' (-not $snap[0].Exists -and -not $snap[1].Exists)
$script:Fake["$RunKey|fc-host"] = @{ Kind = 'String'; Value = '"D:\dev\fc-host.exe" --autostart' }
$script:Fake["$ApprovedKey|fc-host"] = @{ Kind = 'Binary'; Value = [byte[]](2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0) }
$left = Restore-FcHostAutostartRegistry $snap
Check '還原後兩個值都被刪除' ($script:Fake.Count -eq 0) ($script:Fake.Keys -join ',')
Check '全部還原時回傳空陣列' (@($left).Count -eq 0)
# 呼叫端（30 支腳本）實際寫法是直接 @(Restore-…)；回傳若帶前置逗號，空清單會被包成
# 「含一個空陣列的陣列」而 Count＝1（批次 A 每支都誤判「未還原：System.String[]」）。
$script:Fake["$RunKey|fc-host"] = @{ Kind = 'String'; Value = 'x' }
Check '呼叫端寫法 @(Restore-…) 全部還原時 Count＝0' (@(Restore-FcHostAutostartRegistry $snap).Count -eq 0)
$callers = @(Get-ChildItem -Path (Join-Path $PSScriptRoot '..') -Filter '*.ps1' |
        Where-Object { (Get-Content $_.FullName -Raw) -match '\[string\[\]\]\s*\(\s*Restore-FcHostAutostartRegistry' })
Check '呼叫端一律以 @(Restore-…) 收集（[string[]] 在空回傳時為 $null，嚴格模式取 Count 會丟例外）' ($callers.Count -eq 0) (($callers | ForEach-Object Name) -join ',')

# ── 2. 原本存在（含使用者在工作管理員停用）：被改寫後還原成原型別原值 ──────────────────
$userRun = '"C:\Program Files\fc-host\fc-host.exe" --autostart'
$userApproved = [byte[]](3, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8)   # 停用旗標
$script:Fake = @{
    "$RunKey|fc-host"      = @{ Kind = 'String'; Value = $userRun }
    "$ApprovedKey|fc-host" = @{ Kind = 'Binary'; Value = $userApproved }
}
$snap = @(Save-FcHostAutostartRegistry)
$script:Fake["$RunKey|fc-host"] = @{ Kind = 'String'; Value = 'D:\dev\fc-host.exe --autostart' }
$script:Fake.Remove("$ApprovedKey|fc-host")
$left = Restore-FcHostAutostartRegistry $snap
Check 'Run 還原成原值' ($script:Fake["$RunKey|fc-host"].Value -eq $userRun)
Check 'StartupApproved 停用旗標原樣寫回（型別 Binary、位元組相同）' (
    $script:Fake["$ApprovedKey|fc-host"].Kind -eq 'Binary' -and
    (@(Compare-Object $script:Fake["$ApprovedKey|fc-host"].Value $userApproved -SyncWindow 0).Count -eq 0))
Check '全部還原時回傳空陣列（2）' (@($left).Count -eq 0)
# 真正的 RegistryKey.SetValue(name, value, Binary) 只吃 byte[]；快照若把位元組攤平成 object[]，
# 寫回會丟「type of the value object did not match」（2026-10-01 soak 重現腳本實測）。
Check '快照保留 Binary 值的 byte[] 型別' ($snap[1].Value -is [byte[]]) "type=$($snap[1].Value.GetType().FullName)"
Check '寫回 StartupApproved 的值是 byte[]' ($script:Fake["$ApprovedKey|fc-host"].Value -is [byte[]])

# ── 3. 某一項還原失敗不中斷其餘項目，且列出未還原項目 ─────────────────────────────────
$script:Fake = @{ "$ApprovedKey|fc-host" = @{ Kind = 'Binary'; Value = $userApproved }; "$RunKey|fc-host" = @{ Kind = 'String'; Value = $userRun } }
$snap = @(Save-FcHostAutostartRegistry)
$script:Fake = @{}
$script:FailSetFor = @("$RunKey|fc-host")
$left = @(Restore-FcHostAutostartRegistry $snap)
$script:FailSetFor = @()
Check 'Run 寫回失敗時 StartupApproved 仍被還原' ($script:Fake.ContainsKey("$ApprovedKey|fc-host"))
Check '未還原清單恰一項且指出 Run 值' ($left.Count -eq 1 -and $left[0] -match 'CurrentVersion\\Run\\fc-host') ($left -join ' | ')

# ── 4. 讀取失敗：Save 丟例外（不可在沒有快照時啟動宿主）───────────────────────────────
Set-FcHostRegistryBackend @{ Get = { throw '模擬讀取失敗' }; Set = {}; Remove = {} }
$threw = $false
try { $null = Save-FcHostAutostartRegistry } catch { $threw = $true }
Check '讀取失敗時 Save 丟例外' $threw
Set-FcHostRegistryBackend (New-FakeBackend)

# ── 5. 靜態檢查：每支會啟動宿主的腳本都有快照與還原 ───────────────────────────────────
$launchers = Get-ChildItem (Join-Path $toolsDir '*.ps1') | Where-Object {
    $text = Get-Content $_.FullName -Raw
    ($text -match 'Start-Process\s+(-FilePath\s+)?\$(script:)?(Exe|TestExe)\b') -or
    ($text -match 'CreateProcessW\(\$script:Exe')
}
Check '找得到會啟動宿主的腳本' (@($launchers).Count -gt 20) "count=$(@($launchers).Count)"
foreach ($f in $launchers) {
    $text = Get-Content $f.FullName -Raw
    $ok = ($text -match 'lib\\AutostartRegistry\.psm1') -and
          ($text -match 'Save-FcHostAutostartRegistry') -and
          ($text -match 'Restore-FcHostAutostartRegistry')
    Check "$($f.Name) 匯入模組並快照／還原開機自啟登錄" $ok
}

Write-Host "`n$($script:Pass) passed, $($script:Fail) failed"
if ($script:Fail -gt 0) { exit 1 }
exit 0
