<#
.SYNOPSIS
    host/tools/lib/SafeInput.psm1 合成觸控部分的 mock 測試（純 PowerShell 斷言，不需 Pester）。

.DESCRIPTION
    **本測試不注入任何輸入、不建立真實的合成觸控裝置**：建立／銷毀裝置與送出 frame 的後端都以
    Set-SafeInputBackend 換成記錄用的 scriptblock；只有「組 frame」走真正的 C# helper（純記憶體
    運算，不呼叫任何 user32 API）。

    fix F4（review 4.7-minor）：
      - [high] PowerShell 對巢狀 struct 的多層賦值（`$info.touchInfo.pointerInfo.X = v`）只改到暫存
        複本，原本送出的 frame 除了最外層 type 以外全是 0（memory
        powershell-nested-struct-assign-hits-copy）。這裡逐欄讀回組好的 frame。
      - [medium] 拖曳途中發生非鎖定例外時，finally 要送 UP、清掉觸控紀錄，且不得對已銷毀的
        裝置再操作或重複銷毀。
      - [low] frame 要在鎖定檢查「之前」組好，檢查之後立即注入。

    -Target 可指向其他版本的模組（例如 `git show <rev>:host/tools/lib/SafeInput.psm1` 存出的檔案），
    用來確認修正前的版本會失敗。

.EXAMPLE
    pwsh -NoProfile -File host/tools/tests/SafeInputTouch.Tests.ps1
    結束碼 0＝全部通過、1＝有失敗。
#>
param([string]$Target)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$toolsDir = Split-Path $PSScriptRoot -Parent
if (-not $Target) { $Target = Join-Path $toolsDir 'lib\SafeInput.psm1' }
Import-Module $Target -Force

$script:Pass = 0
$script:Fail = 0
function Check([string]$name, [bool]$cond, [string]$detail = '') {
    if ($cond) { $script:Pass++; Write-Host "PASS  $name" }
    else { $script:Fail++; Write-Host "FAIL  $name $detail" -ForegroundColor Red }
}

function Get-FrameText($f) {
    $p = $f.touchInfo.pointerInfo
    'type={0} ptype={1} id={2} frame={3} flags=0x{4:X} hist={5} px=({6},{7}) raw=({8},{9})' -f $f.type, $p.pointerType,
    $p.pointerId, $p.frameId, $p.pointerFlags, $p.historyCount, $p.ptPixelLocation.X, $p.ptPixelLocation.Y,
    $p.ptPixelLocationRaw.X, $p.ptPixelLocationRaw.Y
}

function Test-Frame($f, [int]$Flags, [int]$X, [int]$Y) {
    if ($null -eq $f) { return $false }
    $p = $f.touchInfo.pointerInfo
    return ($f.type -eq 2) -and ($p.pointerType -eq 2) -and ($p.pointerId -eq 1) -and ($p.frameId -gt 0) -and
    ($p.pointerFlags -eq $Flags) -and ($p.historyCount -eq 1) -and
    ($p.ptPixelLocation.X -eq $X) -and ($p.ptPixelLocation.Y -eq $Y) -and
    ($p.ptPixelLocationRaw.X -eq $X) -and ($p.ptPixelLocationRaw.Y -eq $Y)
}

# ---------------------------------------------------------------- 1. 組 frame：逐欄讀回（真正的 C# helper）
$hasBuilder = [bool](Get-Command New-SafeInputTouchFrame -ErrorAction SilentlyContinue)
Check '模組匯出 New-SafeInputTouchFrame（組 frame 的唯一入口）' $hasBuilder
if ($hasBuilder) {
    $f1 = New-SafeInputTouchFrame -Flags 0x10017 -X 1234 -Y 567
    Check '組 frame：DOWN 的 type／pointerType／pointerId／pointerFlags／historyCount／兩組座標逐欄等於輸入' (Test-Frame $f1 0x10017 1234 567) (Get-FrameText $f1)
    $f2 = New-SafeInputTouchFrame -Flags 0x20016 -X -5 -Y 2000
    Check '組 frame：UPDATE（含負座標）逐欄等於輸入' (Test-Frame $f2 0x20016 -5 2000) (Get-FrameText $f2)
    Check '組 frame：frameId 逐次遞增' ($f2.touchInfo.pointerInfo.frameId -gt $f1.touchInfo.pointerInfo.frameId) "$(Get-FrameText $f1) / $(Get-FrameText $f2)"
    $f3 = New-SafeInputTouchFrame -Flags 0x40000 -X 9 -Y 8
    Check '組 frame：UP 逐欄等於輸入' (Test-Frame $f3 0x40000 9 8) (Get-FrameText $f3)
    Check '組 frame：touchMask＝0（不使用接觸矩形／壓力）' ($f3.touchInfo.touchMask -eq 0)
}

# ---------------------------------------------------------------- mock 後端
# Log 依序記錄 prepare／check／build／inject touch／destroy；Frames 收到送出的 frame（原值）。
$state = @{ Log = New-Object System.Collections.Generic.List[string]; Frames = New-Object System.Collections.Generic.List[object]
    Locked = $false; SleepCount = 0; LockAfterSleep = -1; TouchCalls = 0; FailAtTouch = -1; Destroyed = 0 }

function Use-TouchMock {
    param([int]$LockAfterSleep = -1, [int]$FailAtTouch = -1)
    $state.Log.Clear(); $state.Frames.Clear()
    $state.Locked = $false; $state.SleepCount = 0; $state.LockAfterSleep = $LockAfterSleep
    $state.TouchCalls = 0; $state.FailAtTouch = $FailAtTouch; $state.Destroyed = 0
    $s = $state
    Set-SafeInputBackend `
        -Prepare ({ $s.Log.Add('prepare') }.GetNewClosure()) `
        -IsLocked ({ $s.Log.Add('check'); $s.Locked }.GetNewClosure()) `
        -LastInputTick ({ [uint32]1 }) `
        -DescribeForeground ({ 'mock-foreground' }) `
        -Key ({ param($vk, $flags) $s.Log.Add('inject key') }.GetNewClosure()) `
        -Mouse ({ param($flags, $data) $s.Log.Add('inject mouse') }.GetNewClosure()) `
        -Cursor ({ param($x, $y) $s.Log.Add('inject cursor') }.GetNewClosure()) `
        -Sleep ({
            param($ms)
            $s.SleepCount++
            if ($s.LockAfterSleep -ge 1 -and $s.SleepCount -ge $s.LockAfterSleep) { $s.Locked = $true }
        }.GetNewClosure()) `
        -TouchCreateDevice ({ $s.Log.Add('create'); [IntPtr]4242 }.GetNewClosure()) `
        -TouchDestroyDevice ({ param($d) $s.Log.Add("destroy $d"); $s.Destroyed++ }.GetNewClosure()) `
        -TouchFrame ({
            param($Flags, $X, $Y, $FrameId)
            $s.Log.Add('build')
            [SafeInputNative.User32]::MakeTouchFrame([uint32]1, [uint32]$FrameId, [int]$Flags, [int]$X, [int]$Y)
        }.GetNewClosure()) `
        -Touch ({
            param($Device, $Frame)
            $s.TouchCalls++
            $s.Log.Add(('inject touch {0} 0x{1:X}' -f $Device, $Frame.touchInfo.pointerInfo.pointerFlags))
            $s.Frames.Add($Frame)
            -not ($s.FailAtTouch -ge 1 -and $s.TouchCalls -eq $s.FailAtTouch)
        }.GetNewClosure())
}

function Invoke-Caught([scriptblock]$action) {
    try { & $action; return $null } catch { return "$_" }
}

# ---------------------------------------------------------------- 2. 正常拖曳：frame 內容與順序
$mockOk = $true
try { Use-TouchMock } catch { $mockOk = $false; Check 'Set-SafeInputBackend 接受 -TouchFrame 與 (Device, Frame) 形狀的 -Touch' $false "$_" }
if ($mockOk) {
    $err = Invoke-Caught { Send-GuardedTouchDrag -StartX 100 -StartY 400 -EndX 100 -EndY 200 -Steps 4 -StepDelayMs 1 }
    Check '正常拖曳：沒有例外' ($null -eq $err) "err=$err"
    $fr = $state.Frames.ToArray()
    Check '正常拖曳：送出 DOWN＋4×UPDATE＋UP 共 6 個 frame' ($fr.Count -eq 6) "count=$($fr.Count)"
    if ($fr.Count -eq 6) {
        Check '正常拖曳：第一個 frame＝DOWN 0x10017 於起點，欄位齊全' (Test-Frame $fr[0] 0x10017 100 400) (Get-FrameText $fr[0])
        Check '正常拖曳：最後一個 UPDATE 於終點' (Test-Frame $fr[4] 0x20016 100 200) (Get-FrameText $fr[4])
        Check '正常拖曳：最後一個 frame＝UP 0x40000 於終點' (Test-Frame $fr[5] 0x40000 100 200) (Get-FrameText $fr[5])
        $ids = @($fr | ForEach-Object { [int64]$_.touchInfo.pointerInfo.frameId })
        $inc = $true
        for ($i = 1; $i -lt $ids.Count; $i++) { if ($ids[$i] -le $ids[$i - 1]) { $inc = $false } }
        Check '正常拖曳：frameId 嚴格遞增' $inc ($ids -join ',')
    }
    $log = @($state.Log)
    $okOrder = $true
    for ($i = 0; $i -lt $log.Count; $i++) {
        if ($log[$i] -like 'inject touch*') {
            if ($i -lt 2 -or $log[$i - 1] -ne 'check' -or $log[$i - 2] -ne 'build') { $okOrder = $false }
        }
    }
    Check '正常拖曳：每次注入都是「組 frame → 鎖定檢查 → 立即注入」（檢查與注入之間沒有組裝）' $okOrder ($log -join ' | ')
    Check '正常拖曳：裝置只銷毀一次' ($state.Destroyed -eq 1) ($log -join ' | ')
}

# ---------------------------------------------------------------- 3. 途中非鎖定例外：finally 送 UP、清紀錄、只銷毀一次
if ($mockOk) {
    # 第 3 個 frame（第 2 個 UPDATE）回傳 false → Invoke-GuardedTouch 丟一般例外。
    Use-TouchMock -FailAtTouch 3
    $err = Invoke-Caught { Send-GuardedTouchDrag -StartX 50 -StartY 300 -EndX 50 -EndY 100 -Steps 4 -StepDelayMs 1 }
    Check '途中失敗：丟出非 BLOCKED 例外' (($null -ne $err) -and ($err -notlike 'BLOCKED*')) "err=$err"
    $fr = $state.Frames.ToArray()
    $last =if ($fr.Count -gt 0) { $fr[$fr.Count - 1] } else { $null }
    Check '途中失敗：finally 補送 UP（最後一個 frame 為 0x40000、座標為失敗前最後成功的位置）' (Test-Frame $last 0x40000 50 250) $(if ($last) { Get-FrameText $last } else { '(無 frame)' })
    $log = @($state.Log)
    $upIdx = [array]::LastIndexOf($log, 'inject touch 4242 0x40000')
    $dIdx = [array]::IndexOf($log, 'destroy 4242')
    Check '途中失敗：先送 UP 再銷毀裝置' (($upIdx -ge 0) -and ($dIdx -gt $upIdx)) ($log -join ' | ')
    Check '途中失敗：裝置只銷毀一次' ($state.Destroyed -eq 1) ($log -join ' | ')
    # 之後偵測到鎖定：觸控紀錄已清掉，不得對已銷毀的裝置再送任何 frame 或再銷毀。
    $state.Locked = $true
    $state.Log.Clear()
    $err = Invoke-Caught { Assert-SessionUnlocked '途中失敗後的下一步' }
    Check '途中失敗後鎖定：丟 BLOCKED' ($err -like 'BLOCKED*') "err=$err"
    Check '途中失敗後鎖定：對已銷毀的裝置零操作（無 frame、無銷毀）' (@($state.Log | Where-Object { $_ -like 'inject touch*' -or $_ -like 'destroy*' }).Count -eq 0) ($state.Log -join ' | ')
}

# ---------------------------------------------------------------- 4. 途中鎖定：ReleaseAll 送 UP＋銷毀，finally 不重複銷毀
if ($mockOk) {
    # 第 2 次等待（第 1 個 UPDATE 之後）期間鎖定。
    Use-TouchMock -LockAfterSleep 2
    $err = Invoke-Caught { Send-GuardedTouchDrag -StartX 70 -StartY 300 -EndX 70 -EndY 100 -Steps 4 -StepDelayMs 1 }
    Check '途中鎖定：丟 BLOCKED' ($err -like 'BLOCKED*') "err=$err"
    $fr = $state.Frames.ToArray()
    $ups =@($fr | Where-Object { $_.touchInfo.pointerInfo.pointerFlags -eq 0x40000 })
    Check '途中鎖定：恰好送出一個 UP' ($ups.Count -eq 1) "ups=$($ups.Count)"
    if ($ups.Count -ge 1) {
        Check '途中鎖定：放開用的 UP frame 欄位齊全（不是全 0）、座標為最後位置' (Test-Frame $ups[0] 0x40000 70 250) (Get-FrameText $ups[0])
    }
    Check '途中鎖定：裝置只銷毀一次（finally 不重複銷毀）' ($state.Destroyed -eq 1) ($state.Log -join ' | ')
}

Reset-SafeInputBackend
Write-Host ''
Write-Host "合計：$($script:Pass) PASS、$($script:Fail) FAIL"
if ($script:Fail -gt 0) { exit 1 }
exit 0
