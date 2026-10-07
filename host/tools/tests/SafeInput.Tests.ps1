<#
.SYNOPSIS
    host/tools/lib/SafeInput.psm1 的 mock 測試（純 PowerShell 斷言，不需 Pester）。

.DESCRIPTION
    所有後端（初始化、鎖定檢查、最後輸入時間、前景描述、按鍵、滑鼠、游標、等待）都以
    Set-SafeInputBackend 換成記錄用的 scriptblock：**本測試不呼叫任何真實注入 API，也不讀
    真實的 LogonUI 狀態**，工作階段鎖定時照樣可以跑（匯入模組時會編譯原生型別，但編譯不注入）。
    重點情境是 Codex task 1.1 r1 [high] 指出的「入口檢查時未鎖定、等待期間才鎖定」：以 mock 的
    Sleep 在等待當下把鎖定旗標翻成 true，驗證之後不再有任何注入；以及 1.1fix r1 [high] 的
    「初始化（編譯）期間鎖定」與合成輸入是否生效的前置探查（fix round 3）。

    另做靜態檢查：host/tools/*.ps1 除本模組外不得再直接宣告／呼叫底層注入 API，且有用到
    注入的腳本都匯入本模組。（API 名稱以字串拼接表示，避免本檔被全域鎖定 hook 誤判為注入腳本。）

.EXAMPLE
    pwsh -NoProfile -File host/tools/tests/SafeInput.Tests.ps1
    結束碼 0＝全部通過、1＝有失敗。
#>
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$toolsDir = Split-Path $PSScriptRoot -Parent
$modulePath = Join-Path $toolsDir 'lib\SafeInput.psm1'
Import-Module $modulePath -Force

$script:Pass = 0
$script:Fail = 0
function Check([string]$name, [bool]$cond, [string]$detail = '') {
    if ($cond) { $script:Pass++; Write-Host "PASS  $name" }
    else { $script:Fail++; Write-Host "FAIL  $name $detail" -ForegroundColor Red }
}

# 真實後端的原生型別必須在「匯入模組」時就編譯好（fix round 3，Codex 1.1fix r1 [high]）：
# 在任何 mock 替換、任何鎖定檢查之前先記下來。
$script:NativeLoadedAtImport = [bool]('SafeInputNative.User32' -as [type])

# 共用 mock 狀態：Log 依序記錄每次「prepare」「check」與「inject」，Locked 為目前的鎖定旗標。
# Tick＝模擬 GetLastInputInfo 的 dwTime；InputEffective＝合成輸入是否被系統計入（按鍵時推進 Tick）。
$state = @{ Locked = $false; Log = New-Object System.Collections.Generic.List[string]; SleepCount = 0; LockAfterSleep = -1; ProbeThrows = $false; CheckCount = 0; LockAtCheck = -1
    LockDuringPrepare = $false; Tick = [uint32]1000; InputEffective = $true; TickThrows = $false; Foreground = $null; ForegroundThrows = $false }

# fix F8：前置探查送 F15 之前先查前景。預設前景＝一扇一般的 Windows Terminal 主視窗。
function New-Fg {
    param([IntPtr]$Hwnd = [IntPtr]0x500, [string]$Class = 'CASCADIA_HOSTING_WINDOW_CLASS', [int]$Pid_ = 99,
        [string]$ProcessName = 'WindowsTerminal', $Cloaked = $false, $ExStyle = [long]0x00000100, $ShellPid = $null, $Overlays = $null)
    [PSCustomObject]@{ Hwnd = $Hwnd; Class = $Class; Pid = $Pid_; ProcessName = $ProcessName; Cloaked = $Cloaked; ExStyle = $ExStyle
        ShellPid = $ShellPid; Overlays = $Overlays }
}
# fix F8b：桌面前景時另列「可見且覆蓋整個螢幕」的頂層視窗（原生後端只列這些候選，判斷交給 SystemUi）。
function New-Overlay {
    param([string]$Class = 'Shell_SystemDim', [string]$ProcessName = 'explorer', $Visible = $true, $Cloaked = $false, $CoversMonitor = $true)
    [PSCustomObject]@{ Hwnd = [IntPtr]0x900; Class = $Class; ProcessName = $ProcessName; Visible = $Visible; Cloaked = $Cloaked; CoversMonitor = $CoversMonitor }
}

function Use-Mock {
    param([int]$LockAfterSleep = -1, [bool]$Locked = $false, [bool]$ProbeThrows = $false, [int]$LockAtCheck = -1,
        [bool]$LockDuringPrepare = $false, [uint32]$Tick = 1000, [bool]$InputEffective = $true, [bool]$TickThrows = $false,
        $Foreground = (New-Fg), [bool]$ForegroundThrows = $false)
    $state.Foreground = $Foreground
    $state.ForegroundThrows = $ForegroundThrows
    $state.Locked = $Locked
    $state.Log.Clear()
    $state.SleepCount = 0
    $state.LockAfterSleep = $LockAfterSleep
    $state.ProbeThrows = $ProbeThrows
    $state.CheckCount = 0
    $state.LockAtCheck = $LockAtCheck
    $state.LockDuringPrepare = $LockDuringPrepare
    $state.Tick = $Tick
    $state.InputEffective = $InputEffective
    $state.TickThrows = $TickThrows
    $s = $state
    Set-SafeInputBackend `
        -Prepare ({
            $s.Log.Add('prepare')
            # 模擬「初始化（編譯原生後端）期間工作階段被鎖定」
            if ($s.LockDuringPrepare) { $s.Locked = $true }
        }.GetNewClosure()) `
        -IsLocked ({
            $s.Log.Add('check')
            $s.CheckCount++
            # 第 N 次鎖定檢查當下工作階段剛好被鎖定
            if ($s.LockAtCheck -ge 1 -and $s.CheckCount -ge $s.LockAtCheck) { $s.Locked = $true }
            if ($s.ProbeThrows) { throw '模擬：查詢行程失敗' }
            $s.Locked
        }.GetNewClosure()) `
        -LastInputTick ({
            if ($s.TickThrows) { throw '模擬：GetLastInputInfo 失敗' }
            $s.Tick
        }.GetNewClosure()) `
        -DescribeForeground ({ 'mock-foreground' }) `
        -ForegroundInfo ({
            $s.Log.Add('fgquery')
            if ($s.ForegroundThrows) { throw '模擬：GetForegroundWindow 查詢失敗' }
            $s.Foreground
        }.GetNewClosure()) `
        -Key ({
            param($vk, $flags)
            $s.Log.Add(('inject key {0:X2}/{1}' -f $vk, $flags))
            if ($s.InputEffective) { $s.Tick = [uint32](([int64]$s.Tick + 16) % 4294967296) }
        }.GetNewClosure()) `
        -Mouse ({ param($flags, $data) $s.Log.Add(('inject mouse {0:X4}/{1}' -f $flags, $data)) }.GetNewClosure()) `
        -Cursor ({ param($x, $y) $s.Log.Add("inject cursor $x,$y") }.GetNewClosure()) `
        -Sleep ({
            param($ms)
            $s.SleepCount++
            $s.Log.Add("sleep $ms")
            # 第 N 次等待期間工作階段被鎖定
            if ($s.LockAfterSleep -ge 1 -and $s.SleepCount -ge $s.LockAfterSleep) { $s.Locked = $true }
        }.GetNewClosure())
}

function Get-Injects { @($state.Log | Where-Object { $_ -like 'inject *' }) }

# 每個 inject 的前一筆紀錄必須是 check（中間不得隔著 sleep 或其他注入）。
function Test-EveryInjectPrecededByCheck {
    $log = @($state.Log)
    for ($i = 0; $i -lt $log.Count; $i++) {
        if ($log[$i] -like 'inject *' -and ($i -eq 0 -or $log[$i - 1] -ne 'check')) { return $false }
    }
    return $true
}

function Invoke-Blocked([scriptblock]$action) {
    try { & $action; return $null } catch { return "$_" }
}

# ---------------------------------------------------------------- 1. 未鎖定：正常送出，且逐次檢查
Use-Mock
Send-GuardedClick 100 200
$inj = @(Get-Injects)
Check '未鎖定：點擊送出游標＋按下＋放開三個呼叫' ($inj.Count -eq 3) ($inj -join ' | ')
Check '未鎖定：點擊的每個注入之前都緊接一次鎖定檢查' (Test-EveryInjectPrecededByCheck) ($state.Log -join ' | ')

Use-Mock
Send-GuardedWinD
$inj = @(Get-Injects)
Check '未鎖定：Win+D 送出四個按鍵呼叫' ($inj.Count -eq 4) ($inj -join ' | ')
Check '未鎖定：Win+D 的每個注入之前都緊接一次鎖定檢查' (Test-EveryInjectPrecededByCheck) ($state.Log -join ' | ')

Use-Mock
Send-GuardedAltTap
Check '未鎖定：Alt 送出按下＋放開' (@(Get-Injects).Count -eq 2)
Check '未鎖定：Alt 的每個注入之前都緊接一次鎖定檢查' (Test-EveryInjectPrecededByCheck)

Use-Mock
Send-GuardedWheel 10 20 6
$inj = @(Get-Injects)
Check '未鎖定：滾輪送出游標＋6 格' ($inj.Count -eq 7) ($inj -join ' | ')
Check '未鎖定：滾輪每格 data＝-120 的 DWORD 2 補數' (@($inj | Where-Object { $_ -eq 'inject mouse 0800/4294967176' }).Count -eq 6) ($inj -join ' | ')
Check '未鎖定：滾輪的每個注入之前都緊接一次鎖定檢查' (Test-EveryInjectPrecededByCheck)
Check '未鎖定：模組未進入停止狀態' ($null -eq (Get-SafeInputTripped))

# ---------------------------------------------------------------- 2. Codex r1 [high]：入口解鎖、等待後鎖定
# 點擊：移游標後的 80 ms 等待期間鎖定 → 不得送出按下／放開。
Use-Mock -LockAfterSleep 1
$err = Invoke-Blocked { Send-GuardedClick 100 200 }
$inj = @(Get-Injects)
Check '點擊：入口未鎖定（游標已移動）' ($inj.Count -ge 1 -and $inj[0] -eq 'inject cursor 100,200') ($inj -join ' | ')
Check '點擊：等待後鎖定 → 丟 BLOCKED' ($err -like 'BLOCKED*') "err=$err"
Check '點擊：等待後鎖定 → 沒有任何滑鼠按下／放開' (@($inj | Where-Object { $_ -like 'inject mouse*' }).Count -eq 0) ($inj -join ' | ')
Check '點擊：鎖定之後的紀錄沒有任何注入' (@($state.Log | Select-Object -Skip ($state.Log.IndexOf('sleep 80') + 1) | Where-Object { $_ -like 'inject *' }).Count -eq 0) ($state.Log -join ' | ')
Check '點擊：模組進入停止狀態' ($null -ne (Get-SafeInputTripped))

# 點擊：按下後的 40 ms 等待期間鎖定 → 只允許放開本行程按住的左鍵（fix round 2 裁決），
# 不得有其他注入。
Use-Mock -LockAfterSleep 2
$err = Invoke-Blocked { Send-GuardedClick 100 200 }
$inj = @(Get-Injects)
Check '點擊（按下後鎖定）：丟 BLOCKED' ($err -like 'BLOCKED*') "err=$err"
Check '點擊（按下後鎖定）：游標、按下之後只多一個左鍵放開' (($inj.Count -eq 3) -and ($inj[1] -eq 'inject mouse 0002/0') -and ($inj[2] -eq 'inject mouse 0004/0')) ($inj -join ' | ')

# Win+D：按下 Win、D 之後的 40 ms 等待期間鎖定 → 只放開 D、Win（相反順序），沒有新的按下。
Use-Mock -LockAfterSleep 1
$err = Invoke-Blocked { Send-GuardedWinD }
$inj = @(Get-Injects)
Check 'Win+D：等待後鎖定 → 丟 BLOCKED' ($err -like 'BLOCKED*') "err=$err"
Check 'Win+D：等待後鎖定 → 等待之後只有放開 D、Win（相反順序）' (($inj.Count -eq 4) -and ($inj[2] -eq 'inject key 44/2') -and ($inj[3] -eq 'inject key 5B/2')) ($inj -join ' | ')

# ---------------------------------------------------------------- 2b. fix round 2：鎖定時放開本行程按住的鍵
# ① Win 按下後鎖定（第 3 次檢查＝D 按下前）→ 只送出 Win 放開、沒有 D 的任何事件。
Use-Mock -LockAtCheck 3
$err = Invoke-Blocked { Send-GuardedWinD }
$inj = @(Get-Injects)
Check '放開①：Win 按下後鎖定 → 丟 BLOCKED' ($err -like 'BLOCKED*') "err=$err"
Check '放開①：Win 按下後鎖定 → 只有 Win 按下＋Win 放開' (($inj.Count -eq 2) -and ($inj[0] -eq 'inject key 5B/0') -and ($inj[1] -eq 'inject key 5B/2')) ($inj -join ' | ')
Check '放開①：沒有 D 的任何事件' (@($inj | Where-Object { $_ -like 'inject key 44/*' }).Count -eq 0) ($inj -join ' | ')
Check '放開①：放開發生在偵測到鎖定之後、不再重查' (($state.Log[$state.Log.Count - 1] -eq 'inject key 5B/2') -and ($state.Log[$state.Log.Count - 2] -eq 'check')) ($state.Log -join ' | ')

# ② 滑鼠按下後鎖定 → 只送左鍵放開，沒有游標移動或其他滑鼠事件。
Use-Mock -LockAfterSleep 2
$err = Invoke-Blocked { Send-GuardedClick 7 8 }
$after = @($state.Log | Select-Object -Skip ($state.Log.IndexOf('sleep 40') + 1) | Where-Object { $_ -like 'inject *' })
Check '放開②：滑鼠按下後鎖定 → 丟 BLOCKED' ($err -like 'BLOCKED*') "err=$err"
Check '放開②：鎖定後只送出一個左鍵放開' (($after.Count -eq 1) -and ($after[0] -eq 'inject mouse 0004/0')) ($after -join ' | ')

# ③ 集合為空時鎖定 → 零注入（一次完整點擊之後才鎖定）。
Use-Mock
Send-GuardedClick 1 2
$state.Locked = $true
$state.Log.Clear()
$err = Invoke-Blocked { Send-GuardedAltTap }
Check '放開③：集合為空時鎖定 → 丟 BLOCKED' ($err -like 'BLOCKED*') "err=$err"
Check '放開③：集合為空時鎖定 → 零注入' (@(Get-Injects).Count -eq 0) ($state.Log -join ' | ')

# 放開只做一次：停止後再呼叫不會重送放開。
Use-Mock -LockAtCheck 3
[void](Invoke-Blocked { Send-GuardedWinD })
$state.Log.Clear()
$err = Invoke-Blocked { Invoke-GuardedKey 0x5B -Up }
Check '放開：停止後再呼叫（含放開）一律拒絕且零注入' (($err -like 'BLOCKED*') -and @(Get-Injects).Count -eq 0) ($state.Log -join ' | ')

# 未鎖定時正常放開會從集合移除：之後鎖定不會重複放開。
Use-Mock
Invoke-GuardedMouse 2
Invoke-GuardedMouse 4
Invoke-GuardedKey 0x12
Invoke-GuardedKey 0x12 -Up
$state.Locked = $true
$state.Log.Clear()
$err = Invoke-Blocked { Assert-SessionUnlocked '情境開始' }
Check '放開：已正常放開的鍵不會在鎖定時重送' (($err -like 'BLOCKED*') -and @(Get-Injects).Count -eq 0) ($state.Log -join ' | ')

# 滾輪：第 3 次等待（第 2 格之後）鎖定 → 只送出 2 格。
Use-Mock -LockAfterSleep 3
$err = Invoke-Blocked { Send-GuardedWheel 10 20 6 }
$inj = @(Get-Injects)
Check '滾輪：中途鎖定 → 丟 BLOCKED' ($err -like 'BLOCKED*') "err=$err"
Check '滾輪：中途鎖定 → 只送出鎖定前的 2 格' (@($inj | Where-Object { $_ -like 'inject mouse*' }).Count -eq 2) ($inj -join ' | ')

# ---------------------------------------------------------------- 3. 停止狀態是鎖存的：解鎖後也不再注入
Use-Mock -LockAfterSleep 1
[void](Invoke-Blocked { Send-GuardedClick 1 1 })
$state.Locked = $false
$state.LockAfterSleep = -1
$state.Log.Clear()
$err = Invoke-Blocked { Send-GuardedAltTap }
Check '鎖存：之後即使解鎖，Alt 仍丟 BLOCKED' ($err -like 'BLOCKED*') "err=$err"
Check '鎖存：之後即使解鎖，也沒有任何注入' (@(Get-Injects).Count -eq 0) ($state.Log -join ' | ')
$err = Invoke-Blocked { Invoke-GuardedKey 0x41 }
Check '鎖存：單一按鍵呼叫也被拒' (($err -like 'BLOCKED*') -and @(Get-Injects).Count -eq 0) "err=$err"
$err = Invoke-Blocked { Assert-SessionUnlocked '情境開始' }
Check '鎖存：Assert-SessionUnlocked 也丟 BLOCKED' ($err -like 'BLOCKED*') "err=$err"

# ---------------------------------------------------------------- 4. 入口就鎖定、檢查失敗（fail-closed）
Use-Mock -Locked $true
$err = Invoke-Blocked { Send-GuardedWinD }
Check '入口已鎖定：丟 BLOCKED 且零注入' (($err -like 'BLOCKED*') -and @(Get-Injects).Count -eq 0) "err=$err"
Check '入口已鎖定：Test-SessionLocked 回 true' (Test-SessionLocked)

Use-Mock -ProbeThrows $true
$err = Invoke-Blocked { Send-GuardedClick 5 5 }
Check '鎖定檢查本身失敗：視為鎖定（fail-closed）、零注入' (($err -like 'BLOCKED*') -and @(Get-Injects).Count -eq 0) "err=$err"
Check '鎖定檢查本身失敗：Test-SessionLocked 回 true' (Test-SessionLocked)

# ---------------------------------------------------------------- 4b. fix round 3：初始化不得夾在「檢查→注入」之間
# Codex 1.1fix r1 [high]：原本第一次注入時才 Add-Type 編譯原生後端（數秒），發生在鎖定檢查之後。
Check '初始化：匯入模組時原生後端就已編譯載入（不等第一次注入）' $script:NativeLoadedAtImport

Use-Mock
Send-GuardedClick 3 4
$log = @($state.Log)
$firstInj = [array]::IndexOf($log, (@($log | Where-Object { $_ -like 'inject *' })[0]))
Check '初始化：首次注入的前一筆必為鎖定檢查' ($firstInj -ge 1 -and $log[$firstInj - 1] -eq 'check') ($log -join ' | ')
Check '初始化：prepare 一律在該次檢查之前（檢查與注入之間沒有初始化）' ($firstInj -ge 2 -and $log[$firstInj - 2] -eq 'prepare') ($log -join ' | ')
Check '初始化：任何 inject 的前一筆都不是 prepare' (Test-EveryInjectPrecededByCheck) ($log -join ' | ')

Use-Mock -LockDuringPrepare $true
$err = Invoke-Blocked { Send-GuardedWinD }
Check '初始化期間鎖定：丟 BLOCKED' ($err -like 'BLOCKED*') "err=$err"
Check '初始化期間鎖定：首次注入為零' (@(Get-Injects).Count -eq 0) ($state.Log -join ' | ')
Check '初始化期間鎖定：停止狀態已鎖存' ($null -ne (Get-SafeInputTripped))
$state.Locked = $false; $state.LockDuringPrepare = $false; $state.Log.Clear()
$err = Invoke-Blocked { Send-GuardedClick 1 1 }
Check '初始化期間鎖定：之後解鎖也不再注入' (($err -like 'BLOCKED*') -and @(Get-Injects).Count -eq 0) ($state.Log -join ' | ')

# 真實後端：注入用的 scriptblock 內不得再做任何初始化／編譯（檢查之後直接呼叫底層 API）。
$modSrc = Get-Content $modulePath -Raw
$backendSrc = [regex]::Match($modSrc, '(?s)function New-DefaultSafeInputBackend \{(.*?)\n\}').Groups[1].Value
$injectLines = @($backendSrc -split "`n" | Where-Object { $_ -match '^\s*(Key|Mouse|Cursor)\s*=' })
Check '初始化：真實後端的 Key／Mouse／Cursor 三項都找得到' ($injectLines.Count -eq 3) ($injectLines -join ' | ')
Check '初始化：真實後端的注入 scriptblock 不含 Initialize／Add-Type' (@($injectLines | Where-Object { $_ -match 'Initialize-SafeInputNative|Add-Type' }).Count -eq 0) ($injectLines -join ' | ')

# ---------------------------------------------------------------- 4c. 合成輸入是否真的生效（前置探查）
# memory logonui-unlocked-but-synthetic-input-inert：LogonUI 不在不代表合成輸入會生效。
Use-Mock
$pf = Invoke-SafeInputPreflight
Check '前置探查：輸入生效 → ExitCode 0' ($pf.ExitCode -eq 0) "$($pf.ExitCode) $($pf.Message)"
$inj = @(Get-Injects)
Check '前置探查：只送 F15 按下＋放開（無害鍵）' (($inj.Count -eq 2) -and $inj[0] -eq 'inject key 7E/0' -and $inj[1] -eq 'inject key 7E/2') ($inj -join ' | ')
Check '前置探查：每個注入之前都緊接一次鎖定檢查' (Test-EveryInjectPrecededByCheck) ($state.Log -join ' | ')

Use-Mock -InputEffective $false
$pf = Invoke-SafeInputPreflight
Check '前置探查：GetLastInputInfo 沒前進 → ExitCode 3' ($pf.ExitCode -eq 3) "$($pf.ExitCode) $($pf.Message)"
Check '前置探查：訊息以 ENV-BLOCKED 開頭' ($pf.Message -like 'ENV-BLOCKED*') $pf.Message
Check '前置探查：ENV-BLOCKED 訊息含前景視窗描述' ($pf.Message -like '*mock-foreground*') $pf.Message
Check '前置探查：ENV-BLOCKED 時 F15 已放開（沒有卡住的鍵）' (@(Get-Injects)[-1] -eq 'inject key 7E/2') ($state.Log -join ' | ')

Use-Mock -TickThrows $true
$pf = Invoke-SafeInputPreflight
Check '前置探查：GetLastInputInfo 失敗 → 視為不生效（ExitCode 3）' ($pf.ExitCode -eq 3) "$($pf.ExitCode) $($pf.Message)"

Use-Mock -Tick ([uint32]4294967290)
$pf = Invoke-SafeInputPreflight
Check '前置探查：tick 繞回 0 仍判為前進' ($pf.ExitCode -eq 0) "$($pf.ExitCode) $($pf.Message)"

Use-Mock -Locked $true
$pf = Invoke-SafeInputPreflight
Check '前置探查：鎖定 → ExitCode 2、零注入' (($pf.ExitCode -eq 2) -and @(Get-Injects).Count -eq 0) "$($pf.ExitCode) $($pf.Message)"
Check '前置探查：鎖定訊息以 BLOCKED 開頭' ($pf.Message -like 'BLOCKED*') $pf.Message

# ---------------------------------------------------------------- 4d. fix F8：前景是系統 UI 時不送任何鍵
# 批次 B 補跑：「Windows 安全性」對話框（PickerHost，Shell_SystemDialog＋全螢幕 Shell_SystemDim）佔住前景，
# 原本的前置探查一開頭就把 F15 送進它。送任何鍵之前先查前景：屬於系統 UI 或無法判斷 → 結束碼 3、零注入。
Use-Mock
$pf = Invoke-SafeInputPreflight
$fgAt = $state.Log.IndexOf('fgquery')
$firstInj = [array]::IndexOf(@($state.Log), (@(Get-Injects)[0]))
Check '前景（一般應用程式）：ExitCode 0、照常送 F15' (($pf.ExitCode -eq 0) -and @(Get-Injects).Count -eq 2) "$($pf.ExitCode) $($pf.Message)"
Check '前景（一般應用程式）：前景查詢發生在第一個注入之前' (($fgAt -ge 0) -and ($firstInj -gt $fgAt)) ($state.Log -join ' | ')

$sysFg = [ordered]@{
    '類別 Shell_SystemDialog（Windows 安全性對話框）'  = New-Fg -Class 'Shell_SystemDialog' -ProcessName 'PickerHost'
    '類別 Shell_SystemDim（全螢幕變暗層）'             = New-Fg -Class 'Shell_SystemDim' -ProcessName 'explorer'
    '類別 #32770（對話框）'                            = New-Fg -Class '#32770' -ProcessName 'notepad'
    '類別 Windows.UI.Core.CoreWindow'                  = New-Fg -Class 'Windows.UI.Core.CoreWindow' -ProcessName 'SearchHost'
    '行程 LockApp'                                     = New-Fg -Class 'ApplicationFrameWindow' -ProcessName ('Lock' + 'App')
    '行程 PickerHost'                                  = New-Fg -Class 'ApplicationFrameWindow' -ProcessName 'PickerHost'
    '行程 consent（UAC）'                              = New-Fg -Class 'Credential Dialog Xaml Host' -ProcessName 'consent'
    'DWM cloaked'                                      = New-Fg -Cloaked $true
    'WS_EX_TOPMOST'                                    = New-Fg -ExStyle ([long]0x00000108)
}
foreach ($k in $sysFg.Keys) {
    $w = $sysFg[$k]
    Use-Mock -Foreground $w
    $pf = Invoke-SafeInputPreflight
    Check "前景系統 UI（$k）：ExitCode 3" ($pf.ExitCode -eq 3) "$($pf.ExitCode) $($pf.Message)"
    Check "前景系統 UI（$k）：零注入（不送 F15）" (@(Get-Injects).Count -eq 0) ($state.Log -join ' | ')
    Check "前景系統 UI（$k）：訊息以 ENV-BLOCKED 開頭並記前景類別與行程名" (($pf.Message -like 'ENV-BLOCKED*') -and $pf.Message.Contains("class=$($w.Class)") -and $pf.Message.Contains("process=$($w.ProcessName)")) $pf.Message
    Check "前景系統 UI（$k）：模組未進入鎖定停止狀態（不是 BLOCKED）" ($null -eq (Get-SafeInputTripped))
}

$unknownFg = [ordered]@{
    '查詢丟例外'                    = @{ Throws = $true; Fg = (New-Fg) }
    'GetForegroundWindow 回傳 NULL' = @{ Throws = $false; Fg = (New-Fg -Hwnd ([IntPtr]::Zero) -Class '' -ProcessName '') }
    '查詢結果為 $null'              = @{ Throws = $false; Fg = $null }
    '類別查不到'                    = @{ Throws = $false; Fg = (New-Fg -Class '') }
    '行程名查不到'                  = @{ Throws = $false; Fg = (New-Fg -ProcessName '') }
    'cloaked 讀不到'                = @{ Throws = $false; Fg = (New-Fg -Cloaked $null) }
    '延伸樣式讀不到'                = @{ Throws = $false; Fg = (New-Fg -ExStyle $null) }
}
foreach ($k in $unknownFg.Keys) {
    Use-Mock -Foreground $unknownFg[$k].Fg -ForegroundThrows $unknownFg[$k].Throws
    $pf = Invoke-SafeInputPreflight
    Check "前景無法判斷（$k）：ExitCode 3、零注入（fail closed）" (($pf.ExitCode -eq 3) -and @(Get-Injects).Count -eq 0) "$($pf.ExitCode) $($pf.Message) | $($state.Log -join ' | ')"
    Check "前景無法判斷（$k）：訊息以 ENV-BLOCKED 開頭" ($pf.Message -like 'ENV-BLOCKED*') $pf.Message
}

# 桌面（Win+D 之後的 WorkerW／Progman）不是系統 UI 對話框：F15 對它無害，照常探查——前提是它屬於殼層
# explorer（GetShellWindow 的行程），且沒有可見的系統 UI 覆蓋整個螢幕（fix F8b：「Windows 安全性」的
# Shell_SystemDim 仍在時前景可能落在桌面，F15 雖無害，但 preflight 回 0 會讓後續點擊落在系統 UI 上）。
$deskPid = 4000
$trayNotCover = New-Overlay -Class 'Shell_TrayWnd' -CoversMonitor $false
$maxApp = New-Overlay -Class 'Chrome_WidgetWin_1' -ProcessName 'chrome'
foreach ($cls in 'WorkerW', 'Progman') {
    Use-Mock -Foreground (New-Fg -Class $cls -ProcessName 'explorer' -Pid_ $deskPid -ShellPid $deskPid -Overlays @())
    $pf = Invoke-SafeInputPreflight
    Check "前景是殼層的桌面 $cls、沒有系統覆蓋層：照常探查（ExitCode 0）" (($pf.ExitCode -eq 0) -and @(Get-Injects).Count -eq 2) "$($pf.ExitCode) $($pf.Message)"
}
$deskOk = [ordered]@{
    '工作列（沒有覆蓋整個螢幕）'           = @($trayNotCover)
    '最大化的一般應用程式'                 = @($maxApp)
    'Shell_SystemDim 已 cloaked'           = @((New-Overlay -Cloaked $true))
    'Shell_SystemDim 不可見'               = @((New-Overlay -Visible $false))
    'Shell_SystemDim 沒有覆蓋整個螢幕'     = @((New-Overlay -CoversMonitor $false))
}
foreach ($k in $deskOk.Keys) {
    Use-Mock -Foreground (New-Fg -Class 'WorkerW' -ProcessName 'explorer' -Pid_ $deskPid -ShellPid $deskPid -Overlays $deskOk[$k])
    $pf = Invoke-SafeInputPreflight
    Check "前景是桌面＋$k：不算系統覆蓋層，照常探查（ExitCode 0）" (($pf.ExitCode -eq 0) -and @(Get-Injects).Count -eq 2) "$($pf.ExitCode) $($pf.Message)"
}
$deskBlocked = [ordered]@{
    '可見的 Shell_SystemDim 覆蓋整個螢幕' = @{ Fg = (New-Fg -Class 'Progman' -ProcessName 'explorer' -Pid_ $deskPid -ShellPid $deskPid -Overlays @($trayNotCover, (New-Overlay))); Want = 'Shell_SystemDim' }
    '系統 UI 行程 PickerHost 覆蓋整個螢幕' = @{ Fg = (New-Fg -Class 'WorkerW' -ProcessName 'explorer' -Pid_ $deskPid -ShellPid $deskPid -Overlays @((New-Overlay -Class 'ApplicationFrameWindow' -ProcessName 'PickerHost'))); Want = 'PickerHost' }
    '覆蓋層 cloaked 讀不到（當作可見）'    = @{ Fg = (New-Fg -Class 'WorkerW' -ProcessName 'explorer' -Pid_ $deskPid -ShellPid $deskPid -Overlays @((New-Overlay -Cloaked $null))); Want = 'Shell_SystemDim' }
    '覆蓋範圍讀不到（當作覆蓋）'          = @{ Fg = (New-Fg -Class 'WorkerW' -ProcessName 'explorer' -Pid_ $deskPid -ShellPid $deskPid -Overlays @((New-Overlay -CoversMonitor $null))); Want = 'Shell_SystemDim' }
    '桌面視窗不屬殼層 explorer'           = @{ Fg = (New-Fg -Class 'WorkerW' -ProcessName 'explorer' -Pid_ 5000 -ShellPid $deskPid -Overlays @()); Want = '殼層' }
    '殼層行程查不到（ShellPid=0）'        = @{ Fg = (New-Fg -Class 'Progman' -ProcessName 'explorer' -Pid_ $deskPid -ShellPid 0 -Overlays @()); Want = '殼層' }
    '殼層行程欄位缺漏'                    = @{ Fg = (New-Fg -Class 'Progman' -ProcessName 'explorer' -Pid_ $deskPid -Overlays @()); Want = '殼層' }
    '覆蓋層列舉失敗（Overlays=$null）'    = @{ Fg = (New-Fg -Class 'WorkerW' -ProcessName 'explorer' -Pid_ $deskPid -ShellPid $deskPid); Want = '覆蓋' }
}
foreach ($k in $deskBlocked.Keys) {
    Use-Mock -Foreground $deskBlocked[$k].Fg
    $pf = Invoke-SafeInputPreflight
    Check "前景是桌面但$k：ExitCode 3、零注入" (($pf.ExitCode -eq 3) -and @(Get-Injects).Count -eq 0) "$($pf.ExitCode) $($pf.Message) | $($state.Log -join ' | ')"
    Check "前景是桌面但$k：訊息以 ENV-BLOCKED 開頭並寫明原因" (($pf.Message -like 'ENV-BLOCKED*') -and $pf.Message.Contains($deskBlocked[$k].Want)) $pf.Message
    Check "前景是桌面但$k：模組未進入鎖定停止狀態" ($null -eq (Get-SafeInputTripped))
}

# 鎖定優先：已鎖定時回 2，連前景都不查。
Use-Mock -Locked $true -Foreground (New-Fg -Class 'Shell_SystemDialog' -ProcessName 'PickerHost')
$pf = Invoke-SafeInputPreflight
Check '前景系統 UI 但已鎖定：仍回 BLOCKED（ExitCode 2）、零注入' (($pf.ExitCode -eq 2) -and @(Get-Injects).Count -eq 0) "$($pf.ExitCode) $($pf.Message)"

# ---------------------------------------------------------------- 5. 靜態檢查：腳本不得繞過模組
# API 名稱用拼接，避免本檔文字本身命中全域 hook 的注入關鍵字。
$apiNames = @(('keybd' + '_event'), ('mouse' + '_event'), ('Send' + 'Input'), ('SetCursor' + 'Pos'))
$apiRe = ($apiNames | ForEach-Object { [regex]::Escape($_) }) -join '|'
function Get-CodeText([string]$path) {
    # 去掉區塊註解與整行註解（說明文字提到 API 名稱不算呼叫）
    $t = Get-Content $path -Raw
    $t = [regex]::Replace($t, '(?s)<#.*?#>', '')
    ($t -split "`r?`n" | Where-Object { $_ -notmatch '^\s*#' }) -join "`n"
}
$scripts = Get-ChildItem $toolsDir -Filter *.ps1 -File
foreach ($f in $scripts) {
    $code = Get-CodeText $f.FullName
    Check "靜態：$($f.Name) 沒有直接宣告／呼叫底層注入 API" (-not ($code -match $apiRe)) $(if ($code -match $apiRe) { "命中 $($Matches[0])" })
}
foreach ($name in 'probe-1.1.ps1', 'probe-1.3.ps1', 'verify-3.4.ps1', 'verify-4.7-wheel.ps1', 'verify-7.5-edit-move.ps1', 'verify-7.6-edit-resize.ps1', 'verify-7.7-aero-snap.ps1') {
    $code = Get-CodeText (Join-Path $toolsDir $name)
    Check "靜態：$name 匯入 SafeInput 模組" ($code -match 'lib\\SafeInput\.psm1')
    Check "靜態：$name 鎖定判準不含 LockApp" (-not ($code -match 'LockApp'))
    Check "靜態：$name 開始時呼叫 Invoke-SafeInputPreflight 並以其結束碼停止" (($code -match 'Invoke-SafeInputPreflight') -and ($code -match 'exit \$pf\.ExitCode'))
}
$modCode = Get-CodeText $modulePath
Check '靜態：模組的鎖定判準只用 LogonUI' (($modCode -match "Get-Process -Name LogonUI -ErrorAction") -and -not ($modCode -match 'LockApp'))

Reset-SafeInputBackend
Write-Host ''
Write-Host "合計：$($script:Pass) PASS、$($script:Fail) FAIL"
if ($script:Fail -gt 0) { exit 1 }
exit 0
