<#
.SYNOPSIS
    host/tools/lib/VerifyVerdict.psm1 的 mock 測試（純 PowerShell 斷言，不需 Pester）。

.DESCRIPTION
    以合成的 watch-zorder 記錄與逐項結果驗證驗收腳本的判讀：不啟動宿主、不呼叫任何 Win32
    API、不注入任何輸入，工作階段鎖定時照樣可以跑。

.EXAMPLE
    pwsh -NoProfile -File host/tools/tests/VerifyVerdict.Tests.ps1
    結束碼 0＝全部通過、1＝有失敗。
#>
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$toolsDir = Split-Path $PSScriptRoot -Parent
Import-Module (Join-Path $toolsDir 'lib\VerifyVerdict.psm1') -Force

$script:Pass = 0
$script:Fail = 0
function Check([string]$name, [bool]$cond, [string]$detail = '') {
    if ($cond) { $script:Pass++; Write-Host "PASS  $name" }
    else { $script:Fail++; Write-Host "FAIL  $name $detail" -ForegroundColor Red }
}
function T([string]$hms) { [DateTimeOffset]::Parse("2026-09-28T$hms+08:00", [Globalization.CultureInfo]::InvariantCulture) }

# ---------------------------------------------------------------- task 3.3：分階段判讀
# 一行 z-order 記錄：小工具 0x100，below 為其下方一般視窗類別（(none)＝正常）。
function L33([string]$hms, [string]$below = '(none)', [string]$visible = '1') {
    "2026-09-28T$hms+08:00 explorerPid=1 fgClass=Notepad fgPid=5 win[0x100].class=Tauri Window win[0x100].visible=$visible win[0x100].minimized=0 win[0x100].cloaked=0 win[0x100].above=3 win[0x100].desktopAbove=0 win[0x100].below=$below win[0x100].hasProgman=1 win[0x100].hasWorkerW=0"
}
$faultWin = @{ Name = 'fault'; Start = (T '10:00:10.000'); End = (T '10:00:42.000') }
$bad = 'WindowsForms10.Window.8.app.0.1'

# Codex 合成反例：注入後 10 秒內恢復 → 修正前判 FAIL（見 task-3.3-report fix round 1 RED），修正後應 PASS。
$v = Get-SafetyRecoveryVerdict -Lines @((L33 '10:00:00.000'), (L33 '10:00:10.200' $bad), (L33 '10:00:20.000')) -Windows @($faultWin) -FaultName 'fault' -FaultConfirmedAt (T '10:00:10.500')
Check '3.3：故障 10 秒內恢復 → 窗口外違規＝0' ($v.OutsideViolations.Count -eq 0) ($v.OutsideViolations -join ' | ')
Check '3.3：故障 10 秒內恢復 → 觀察到故障' $v.FaultObserved
Check '3.3：故障 10 秒內恢復 → 判定已恢復，耗時約 9.5 秒' ($v.FaultRecovered -and $v.RecoverySec -eq 9.5) "recovered=$($v.FaultRecovered) sec=$($v.RecoverySec)"

# 超過期限才恢復 → 窗口結束時仍異常 → 違規、未恢復。
$v = Get-SafetyRecoveryVerdict -Lines @((L33 '10:00:00.000'), (L33 '10:00:10.200' $bad), (L33 '10:00:50.000')) -Windows @($faultWin) -FaultName 'fault' -FaultConfirmedAt (T '10:00:10.500')
Check '3.3：40 秒才恢復 → 判未恢復' (-not $v.FaultRecovered)
Check '3.3：40 秒才恢復 → 窗口結束時仍異常算違規' (@($v.OutsideViolations | Where-Object { $_ -like '*仍異常*' }).Count -eq 1) ($v.OutsideViolations -join ' | ')

# 從未恢復（記錄結束仍異常）。
$v = Get-SafetyRecoveryVerdict -Lines @((L33 '10:00:00.000'), (L33 '10:00:10.200' $bad)) -Windows @($faultWin) -FaultName 'fault' -FaultConfirmedAt (T '10:00:10.500')
Check '3.3：從未恢復 → 判未恢復且有違規' ((-not $v.FaultRecovered) -and $v.OutsideViolations.Count -ge 1) ($v.OutsideViolations -join ' | ')

# 故障根本沒出現在記錄（例如被 tao 攔回）→ 不得當成通過。
$v = Get-SafetyRecoveryVerdict -Lines @((L33 '10:00:00.000'), (L33 '10:00:30.000')) -Windows @($faultWin) -FaultName 'fault' -FaultConfirmedAt (T '10:00:10.500')
Check '3.3：故障未被觀察到 → FaultObserved=false' (-not $v.FaultObserved)
Check '3.3：故障未被觀察到 → FaultRecovered=false' (-not $v.FaultRecovered)

# 窗口以外（基準期／平靜期）的異常一律違規。
$v = Get-SafetyRecoveryVerdict -Lines @((L33 '10:00:00.000' $bad), (L33 '10:00:01.000'), (L33 '10:00:10.200' $bad), (L33 '10:00:20.000'), (L33 '10:05:00.000' $bad), (L33 '10:05:01.000')) -Windows @($faultWin) -FaultName 'fault' -FaultConfirmedAt (T '10:00:10.500')
Check '3.3：基準期與平靜期各一次異常 → 窗口外違規 2 筆' ($v.OutsideViolations.Count -eq 2) ($v.OutsideViolations -join ' | ')
Check '3.3：同時故障窗口內仍判已恢復' $v.FaultRecovered

# fix F3（review task-3.3-fixB-opus.md [medium]）：rebottom 未生效，故障一直持續到腳本關閉故障視窗；
# 關閉後記錄自然變乾淨。故障窗口 End 若仍是「確認＋32 秒」，關閉後的乾淨行落在窗口內，會被誤判為
# 已恢復＝假 PASS。End 必須截在關閉故障視窗那一刻（Get-FaultWindowEnd），恢復只能發生在故障視窗仍存在時。
$confirmed = T '10:00:10.500'
$closedAt = T '10:00:15.000'
$noFixLines = @((L33 '10:00:00.000'), (L33 '10:00:10.200' $bad), (L33 '10:00:12.000' $bad), (L33 '10:00:14.800' $bad), (L33 '10:00:15.200'), (L33 '10:00:20.000'))
$end = Get-FaultWindowEnd -ConfirmedAt $confirmed -RecoverSec 30 -TolSec 2 -FaultClosedAt $closedAt
Check '3.3：故障視窗提早關閉 → 窗口 End 截在關閉時刻' ($end -eq $closedAt) "end=$end"
$v = Get-SafetyRecoveryVerdict -Lines $noFixLines -Windows @(@{ Name = 'fault'; Start = (T '10:00:10.000'); End = $end }) -FaultName 'fault' -FaultConfirmedAt $confirmed
Check '3.3：rebottom 未生效、關閉故障視窗後才乾淨 → 判未恢復' (-not $v.FaultRecovered) "recovered=$($v.FaultRecovered)"
Check '3.3：rebottom 未生效 → 窗口結束時仍異常算違規（FAIL）' (@($v.OutsideViolations | Where-Object { $_ -like '*仍異常*' }).Count -eq 1) ($v.OutsideViolations -join ' | ')
$end = Get-FaultWindowEnd -ConfirmedAt $confirmed -RecoverSec 30 -TolSec 2 -FaultClosedAt (T '10:01:00.000')
Check '3.3：故障視窗在期限後才關閉 → End 仍是確認＋32 秒' ($end -eq (T '10:00:42.500')) "end=$end"
$end = Get-FaultWindowEnd -ConfirmedAt $confirmed -RecoverSec 30 -TolSec 2
Check '3.3：沒有關閉時刻 → End 是確認＋32 秒' ($end -eq (T '10:00:42.500')) "end=$end"

# explorer 重啟窗口同樣允許短暫異常。
$expWin = @{ Name = 'explorer'; Start = (T '09:59:00.000'); End = (T '09:59:45.000') }
$v = Get-SafetyRecoveryVerdict -Lines @((L33 '09:59:01.000' $bad), (L33 '09:59:05.000'), (L33 '10:00:10.200' $bad), (L33 '10:00:20.000')) -Windows @($expWin, $faultWin) -FaultName 'fault' -FaultConfirmedAt (T '10:00:10.500')
Check '3.3：explorer 重啟窗口內的短暫異常不算違規' ($v.OutsideViolations.Count -eq 0) ($v.OutsideViolations -join ' | ')

# visible=0 且非 dupInSnapshot 在窗口外 → 違規（沿用原規則）。
$v = Get-SafetyRecoveryVerdict -Lines @((L33 '10:03:00.000' '(none)' '0')) -Windows @($faultWin) -FaultName 'fault'
Check '3.3：窗口外小工具 visible=0（非列舉重複）→ 違規' ($v.OutsideViolations.Count -eq 1) ($v.OutsideViolations -join ' | ')

# ---------------------------------------------------------------- task 4.7：結束碼與非使用中前提
$r = [ordered]@{ '滾輪捲動小工具內容（scrollTop 增加）' = $false; '滾輪捲動未搶走前景視窗' = $true }
Check '4.7：任一項 FAIL → 結束碼 1' ((Get-ResultsExitCode $r) -eq 1)
$r = [ordered]@{ a = $true; b = $true }
Check '4.7：全部 PASS → 結束碼 0' ((Get-ResultsExitCode $r) -eq 0)
$r = [ordered]@{ a = $true; b = 'SKIPPED' }
Check '結束碼：SKIPPED 不計' ((Get-ResultsExitCode $r) -eq 0)
$r = [ordered]@{ a = 'SKIPPED' }
Check '結束碼：沒有任何計分項 → 1（沒有證據不算通過）' ((Get-ResultsExitCode $r) -eq 1)
$r = [ordered]@{ a = $null }
Check '結束碼：值為 $null（例如讀值失敗）→ 1' ((Get-ResultsExitCode $r) -eq 1)

$p = Test-InactiveWidgetPrecondition -FgHwnd ([IntPtr]0x500) -FgPid 999 -HostPid 999
Check '4.7：前景屬宿主 → 前提不成立' (-not $p.Ok) $p.Reason
$p = Test-InactiveWidgetPrecondition -FgHwnd ([IntPtr]::Zero) -FgPid 0 -HostPid 999
Check '4.7：前景為空 → 前提不成立' (-not $p.Ok) $p.Reason
$p = Test-InactiveWidgetPrecondition -FgHwnd ([IntPtr]0x600) -FgPid 0 -HostPid 999
Check '4.7：查不到前景 PID → 前提不成立' (-not $p.Ok) $p.Reason
$p = Test-InactiveWidgetPrecondition -FgHwnd ([IntPtr]0x600) -FgPid 1234 -HostPid 999
Check '4.7：前景屬其他行程 → 前提成立' $p.Ok $p.Reason

# ---------------------------------------------------------------- task 3.4：預期小工具逐一存在且可見
function W34([string]$fields) { (ConvertFrom-ZOrderLine "2026-09-28T10:00:00.000+08:00 fgPid=5 $fields").Win }
$np = 'win[0xAAA].class=Notepad win[0xAAA].visible=1 win[0xAAA].minimized=1 win[0xAAA].cloaked=0'
function Wd([string]$h, [string]$vis = '1', [string]$min = '0', [string]$clk = '0', [string]$da = '0') {
    "win[$h].class=Tauri Window win[$h].visible=$vis win[$h].minimized=$min win[$h].cloaked=$clk win[$h].desktopAbove=$da"
}
$exp = @('0x100', '0x200')

# Codex 反例：Win+D 後小工具全部被隱藏（-ProcessName 不再列出）→ 修正前判 PASS，修正後必須有問題。
$p = @(Get-ExpectedWidgetProblems -Win (W34 $np) -Expected $exp -RequireDesktopNotAbove)
Check '3.4：全部小工具消失 → 兩筆問題' ($p.Count -eq 2) ($p -join ' | ')
$p = @(Get-ExpectedWidgetProblems -Win (W34 "$np $(Wd '0x100')") -Expected $exp -RequireDesktopNotAbove)
Check '3.4：消失其中一個 → 一筆問題且指名 0x200' (($p.Count -eq 1) -and ($p[0] -like '0x200*')) ($p -join ' | ')
$p = @(Get-ExpectedWidgetProblems -Win (W34 "$np $(Wd '0x100') $(Wd '0x200')") -Expected $exp -RequireDesktopNotAbove)
Check '3.4：兩個都在且可見 → 無問題' ($p.Count -eq 0) ($p -join ' | ')
$p = @(Get-ExpectedWidgetProblems -Win (W34 "$np $(Wd '0x100') $(Wd '0x200' '0')") -Expected $exp)
Check '3.4：visible=0（明確目標仍列出）→ 問題' (($p.Count -eq 1) -and ($p[0] -like '*visible=0*')) ($p -join ' | ')
$p = @(Get-ExpectedWidgetProblems -Win (W34 "$np $(Wd '0x100' '1' '1') $(Wd '0x200')") -Expected $exp)
Check '3.4：minimized=1 → 問題' (($p.Count -eq 1) -and ($p[0] -like '*minimized=1*')) ($p -join ' | ')
$p = @(Get-ExpectedWidgetProblems -Win (W34 "$np $(Wd '0x100' '1' '0' '1') $(Wd '0x200')") -Expected $exp)
Check '3.4：cloaked=1 → 問題' (($p.Count -eq 1) -and ($p[0] -like '*cloaked=1*')) ($p -join ' | ')
$p = @(Get-ExpectedWidgetProblems -Win (W34 "$np win[0x100].status=not_found $(Wd '0x200')") -Expected $exp)
Check '3.4：status=not_found → 問題' (($p.Count -eq 1) -and ($p[0] -like '*not_found*')) ($p -join ' | ')
$p = @(Get-ExpectedWidgetProblems -Win (W34 "$np $(Wd '0x100' '1' '0' '0' '1') $(Wd '0x200')") -Expected $exp -RequireDesktopNotAbove)
Check '3.4：desktopAbove=1 且要求 → 問題' ($p.Count -eq 1) ($p -join ' | ')
$p = @(Get-ExpectedWidgetProblems -Win (W34 "$np $(Wd '0x100' '1' '0' '0' '1') $(Wd '0x200')") -Expected $exp)
Check '3.4：desktopAbove=1 但未要求 → 無問題' ($p.Count -eq 0) ($p -join ' | ')
$p = @(Get-ExpectedWidgetProblems -Win (W34 "$np $(Wd '0x100' '0') win[0x100].dupInSnapshot=2 $(Wd '0x200')") -Expected $exp)
Check '3.4：visible=0 但 dupInSnapshot → 略過（沿用慣例）' ($p.Count -eq 0) ($p -join ' | ')
$p = @(Get-ExpectedWidgetProblems -Win (W34 $np) -Expected @())
Check '3.4：沒有任何預期小工具 → 問題（不得空集合通過）' ($p.Count -eq 1) ($p -join ' | ')

# ── review 31c7c53：結束碼優先序 產品 FAIL（1）＞ 鎖定（2）＞ 環境（3）；使用者視窗未還原＝環境，不進 $results ──
$hasVec = [bool](Get-Command Get-VerdictExitCode -ErrorAction SilentlyContinue) -and [bool](Get-Command Format-UnrestoredWarning -ErrorAction SilentlyContinue)
Check '匯出 Get-VerdictExitCode 與 Format-UnrestoredWarning' $hasVec
if ($hasVec) {
    $okRes = [ordered]@{ a = $true; b = $true }
    $badRes = [ordered]@{ a = $true; b = $false }
    $nr = @('0xB02（Chrome_WidgetWin_1） 未還原（目標無回應）：mock')
    Check '全過 → 0' ((Get-VerdictExitCode -Results $okRes) -eq 0)
    Check '未還原、其他全過 → 3（環境，不是產品 FAIL）' ((Get-VerdictExitCode -Results $okRes -NotRestored $nr) -eq 3)
    Check '未還原＋產品 FAIL → 1' ((Get-VerdictExitCode -Results $badRes -NotRestored $nr) -eq 1)
    Check '產品 FAIL＋鎖定 → 1（FAIL 優先）' ((Get-VerdictExitCode -Results $badRes -Locked) -eq 1)
    Check '鎖定＋未還原 → 2（鎖定優先於環境）' ((Get-VerdictExitCode -Results $okRes -Locked -NotRestored $nr) -eq 2)
    Check 'ENV-BLOCKED、其他全過 → 3' ((Get-VerdictExitCode -Results $okRes -EnvBlocked) -eq 3)
    Check '-Failed（無逐項結果的腳本）→ 1' ((Get-VerdictExitCode -Failed -NotRestored $nr) -eq 1)
    Check 'SKIPPED 不計為 FAIL' ((Get-VerdictExitCode -Results ([ordered]@{ a = $true; s = 'SKIPPED' })) -eq 0)
    Check '-RequireResults：沒有計分項 → 1（沿用 Get-ResultsExitCode 的「沒有證據不算通過」）' ((Get-VerdictExitCode -Results ([ordered]@{}) -RequireResults) -eq 1)
    Check '-RequireResults：沒有計分項但 ENV-BLOCKED → 3' ((Get-VerdictExitCode -Results ([ordered]@{}) -RequireResults -EnvBlocked) -eq 3)
    Check 'NotRestored 為 $null／空 → 不影響' ((Get-VerdictExitCode -Results $okRes -NotRestored $null) -eq 0 -and (Get-VerdictExitCode -Results $okRes -NotRestored @()) -eq 0)
    $w = Format-UnrestoredWarning -NotRestored $nr
    Check '警示行：醒目字樣＋扇數＋hwnd／原因＋需手動還原' ($w -like '⚠ 使用者視窗未還原 1 扇：*' -and $w -match '0xB02' -and $w -match '目標無回應' -and $w -match '需手動還原') $w
    Check '警示行：沒有未還原 → 空字串' ((Format-UnrestoredWarning -NotRestored @()) -eq '' -and (Format-UnrestoredWarning -NotRestored $null) -eq '')
    # 摘要仍列出未還原：警示行與結束碼無關（有產品 FAIL 時照樣產生）。
    Check '未還原＋產品 FAIL：結束碼 1 且警示行仍產生' ((Get-VerdictExitCode -Results $badRes -NotRestored $nr) -eq 1 -and (Format-UnrestoredWarning -NotRestored $nr) -ne '')
}

# ── verify-grid-overlay：前景判準與 z-order 穩定判定（合成資料，不需宿主）──
$hasGo = [bool](Get-Command Get-ForegroundResult -ErrorAction SilentlyContinue) -and [bool](Get-Command Get-OverlayDesktopProblems -ErrorAction SilentlyContinue)
Check '匯出 Get-ForegroundResult 與 Get-OverlayDesktopProblems' $hasGo
if ($hasGo) {
    $hostPid = 4242
    $fgA = [IntPtr]0x3E1336; $fgB = [IntPtr]0x2A0F00
    $same = Get-ForegroundResult -Before $fgA -After $fgA -AfterPid 100 -HostPid $hostPid
    Check '前景：前後相同 → $true（PASS）' ($same -is [bool] -and $same)
    $hostSw = Get-ForegroundResult -Before $fgA -After $fgB -AfterPid $hostPid -HostPid $hostPid
    Check '前景：切到宿主行程視窗 → $false（FAIL）' ($hostSw -is [bool] -and -not $hostSw)
    $otherSw = Get-ForegroundResult -Before $fgA -After $fgB -AfterPid 100 -HostPid $hostPid
    Check '前景：切到其他行程視窗 → NOT-RUN（不得記為成功）' ($otherSw -is [string] -and $otherSw -eq 'NOT-RUN')
    $rNr = [ordered]@{ fg = $otherSw; ok = $true }
    $nrEnv = (@($rNr.Values | Where-Object { $_ -is [string] }).Count -gt 0)
    Check '前景 NOT-RUN → 腳本結束碼 3（非 0）' ((Get-VerdictExitCode -Results $rNr -EnvBlocked:$nrEnv -RequireResults) -eq 3)
    Check '前景 FAIL → 腳本結束碼 1' ((Get-VerdictExitCode -Results ([ordered]@{ fg = $hostSw }) -RequireResults) -eq 1)

    # 合成 z-order（由上到下，Index 小者在上）：兩個格線與 Progman，副螢幕格線 0xE61454 在 Progman 之下（2026-10 實測的瞬間）。
    $rectMain = [PSCustomObject]@{ X = 0; Y = 0; W = 3840; H = 2088 }
    $rectSub = [PSCustomObject]@{ X = 3840; Y = 4; W = 2560; H = 1528 }
    $rectDesk = [PSCustomObject]@{ X = 0; Y = 0; W = 6400; H = 2160 }
    function Z([int64]$h, [int]$i, [string]$c, $r) { [PSCustomObject]@{ Hwnd = [IntPtr]$h; Index = $i; Class = $c; Visible = $true; Rect = $r } }
    $ovMain = [PSCustomObject]@{ Hwnd = [IntPtr]0x1121604; Rect = $rectMain }
    $ovSub = [PSCustomObject]@{ Hwnd = [IntPtr]0xE61454; Rect = $rectSub }
    $screensFake = @(
        [PSCustomObject]@{ DeviceName = '\\.\DISPLAY5'; WorkingArea = [PSCustomObject]@{ X = 0; Y = 0; Width = 3840; Height = 2088 } },
        [PSCustomObject]@{ DeviceName = '\\.\DISPLAY1'; WorkingArea = [PSCustomObject]@{ X = 3840; Y = 4; Width = 2560; Height = 1528 } })
    $classes = @('Progman', 'WorkerW')
    $zGood = @((Z 0x1121604 607 'fc-host-grid-overlay' $rectMain), (Z 0xE61454 608 'fc-host-grid-overlay' $rectSub), (Z 0x10064 609 'Progman' $rectDesk))
    $zBad = @((Z 0x1121604 607 'fc-host-grid-overlay' $rectMain), (Z 0x10064 609 'Progman' $rectDesk), (Z 0xE61454 610 'fc-host-grid-overlay' $rectSub))
    $pGood = @(Get-OverlayDesktopProblems -ZList $zGood -Overlays @($ovMain, $ovSub) -DesktopClasses $classes -Screens $screensFake)
    Check 'z-order：格線都在桌面之上 → 無問題（Settled）' ($pGood.Count -eq 0)
    $pBad = @(Get-OverlayDesktopProblems -ZList $zBad -Overlays @($ovMain, $ovSub) -DesktopClasses $classes -Screens $screensFake)
    Check 'z-order：副螢幕格線在 Progman 之下 → 恰 1 筆問題，指出該格線與螢幕（逾時 → FAIL）' ($pBad.Count -eq 1 -and $pBad[0] -match '0xE61454' -and $pBad[0] -match 'DISPLAY1') ($pBad -join '|')
    Check 'z-order：Settled=false → 結果項為 $false → 腳本結束碼 1（不被「整台被蓋住」NOTE 豁免）' ((Get-VerdictExitCode -Results ([ordered]@{ z = ($pBad.Count -eq 0) }) -RequireResults) -eq 1)
    # 回歸：真正的 System.Windows.Forms.Screen[] 不得讓函式丟例外（@($Screens) 轉換曾丟 Argument types do not match）。
    Add-Type -AssemblyName System.Windows.Forms
    $realScreens = [System.Windows.Forms.Screen]::AllScreens
    $realOk = $true; try { [void](Get-OverlayDesktopProblems -ZList $zBad -Overlays @($ovMain, $ovSub) -DesktopClasses $classes -Screens $realScreens) } catch { $realOk = $false }
    Check 'z-order：傳入真正的 Screen[] 不丟例外' $realOk
    # 回歸：腳本的 Get-ZList 回傳 List[object]（@($List) 在模組內轉換同樣曾丟 Argument types do not match）。
    $zList = New-Object System.Collections.Generic.List[object]; foreach ($zz in $zBad) { $zList.Add($zz) }
    $listOk = $true; $pList = @(); try { $pList = @(Get-OverlayDesktopProblems -ZList $zList -Overlays @($ovMain, $ovSub) -DesktopClasses $classes -Screens $screensFake) } catch { $listOk = $false }
    Check 'z-order：ZList 傳 List[object]（Get-ZList 的型別）不丟例外且結果同陣列' ($listOk -and $pList.Count -eq 1)
    $zMiss = @((Z 0x10064 609 'Progman' $rectDesk))
    $pMiss = @(Get-OverlayDesktopProblems -ZList $zMiss -Overlays @($ovMain) -DesktopClasses $classes -Screens $screensFake)
    Check 'z-order：格線不在列舉中 → 問題（不當成穩定）' ($pMiss.Count -eq 1 -and $pMiss[0] -match '不在 z-order')
    $zNoOverlap = @((Z 0x1121604 607 'fc-host-grid-overlay' $rectMain), (Z 0x10099 100 'Progman' ([PSCustomObject]@{ X = 9000; Y = 0; W = 100; H = 100 })), (Z 0x10064 609 'Progman' $rectDesk))
    $zNoOverlap = @($zNoOverlap | Sort-Object Index)
    $pNo = @(Get-OverlayDesktopProblems -ZList $zNoOverlap -Overlays @($ovMain) -DesktopClasses $classes -Screens $screensFake)
    Check 'z-order：與格線不重疊的桌面視窗不影響判定' ($pNo.Count -eq 0)
}

# ── verify-grid-overlay：watch-zorder 記錄判讀（建立中／銷毀中的 visible=0 是過渡狀態）──
# 一行只含格線欄位的 z-order 記錄；$wins＝@( @{ H; V; Min; Clk; Below } … )。
function LG([string]$hms, $wins) {
    $f = foreach ($w in $wins) {
        $h = $w.H
        $min = if ($w.ContainsKey('Min')) { $w.Min } else { '0' }
        $clk = if ($w.ContainsKey('Clk')) { $w.Clk } else { '0' }
        $below = if ($w.ContainsKey('Below')) { $w.Below } else { '(none)' }
        "win[$h].class=fc-host-grid-overlay win[$h].visible=$($w.V) win[$h].minimized=$min win[$h].cloaked=$clk win[$h].above=11 win[$h].desktopAbove=0 win[$h].below=$below win[$h].hasProgman=1 win[$h].hasWorkerW=0"
    }
    "2026-10-09T$hms+08:00 explorerPid=1 fgClass=Chrome_WidgetWin_1 fgPid=5 win[0x5C16B0].class=Tauri Window win[0x5C16B0].visible=1 win[0x5C16B0].below=(none) $($f -join ' ')"
}
# 2026-10-09 實跑 grid-overlay-zorder.log 第 3、4、5 筆（只留格線與一個小工具的欄位，值照抄）：0x2507A0 在
# 10:00:02.290 剛建立（visible=0、位於最上層所以 below=Chrome_WidgetWin_1），10:00:02.628 已顯示並置底，10:00:04.896 已關閉。
$realGridLog = @(
    '2026-10-09T10:00:02.290+08:00 explorerPid=32372 fgClass=Chrome_WidgetWin_1 fgPid=65864 win[0x5C16B0].class=Tauri Window win[0x5C16B0].visible=1 win[0x5C16B0].minimized=0 win[0x5C16B0].cloaked=0 win[0x5C16B0].above=19 win[0x5C16B0].desktopAbove=0 win[0x5C16B0].below=(none) win[0x5C16B0].hasProgman=1 win[0x5C16B0].hasWorkerW=0 win[0x2507A0].class=fc-host-grid-overlay win[0x2507A0].visible=0 win[0x2507A0].minimized=0 win[0x2507A0].cloaked=0 win[0x2507A0].above=11 win[0x2507A0].desktopAbove=0 win[0x2507A0].below=Chrome_WidgetWin_1 win[0x2507A0].hasProgman=1 win[0x2507A0].hasWorkerW=0 win[0x2507A0].dupInSnapshot=2 win[0x351194].class=fc-host-grid-overlay win[0x351194].visible=1 win[0x351194].minimized=0 win[0x351194].cloaked=0 win[0x351194].above=24 win[0x351194].desktopAbove=0 win[0x351194].below=(none) win[0x351194].hasProgman=1 win[0x351194].hasWorkerW=0',
    '2026-10-09T10:00:02.628+08:00 explorerPid=32372 fgClass=Chrome_WidgetWin_1 fgPid=65864 win[0x5C16B0].class=Tauri Window win[0x5C16B0].visible=1 win[0x5C16B0].minimized=0 win[0x5C16B0].cloaked=0 win[0x5C16B0].above=19 win[0x5C16B0].desktopAbove=0 win[0x5C16B0].below=(none) win[0x5C16B0].hasProgman=1 win[0x5C16B0].hasWorkerW=0 win[0x2507A0].class=fc-host-grid-overlay win[0x2507A0].visible=1 win[0x2507A0].minimized=0 win[0x2507A0].cloaked=0 win[0x2507A0].above=23 win[0x2507A0].desktopAbove=0 win[0x2507A0].below=(none) win[0x2507A0].hasProgman=1 win[0x2507A0].hasWorkerW=0 win[0x351194].class=fc-host-grid-overlay win[0x351194].visible=1 win[0x351194].minimized=0 win[0x351194].cloaked=0 win[0x351194].above=24 win[0x351194].desktopAbove=0 win[0x351194].below=(none) win[0x351194].hasProgman=1 win[0x351194].hasWorkerW=0',
    '2026-10-09T10:00:04.896+08:00 explorerPid=32372 fgClass=Chrome_WidgetWin_1 fgPid=65864 win[0x5C16B0].class=Tauri Window win[0x5C16B0].visible=1 win[0x5C16B0].minimized=0 win[0x5C16B0].cloaked=0 win[0x5C16B0].above=19 win[0x5C16B0].desktopAbove=0 win[0x5C16B0].below=(none) win[0x5C16B0].hasProgman=1 win[0x5C16B0].hasWorkerW=0'
)
$hasGz = [bool](Get-Command Get-OverlayZOrderLogVerdict -ErrorAction SilentlyContinue)
Check '匯出 Get-OverlayZOrderLogVerdict' $hasGz
if ($hasGz) {
    $gc = 'fc-host-grid-overlay'
    $vReal = Get-OverlayZOrderLogVerdict -Lines $realGridLog -OverlayClass $gc
    Check 'z-order 記錄：實跑片段（建立中 visible=0、之後可見）→ 不異常' ($vReal.Bad.Count -eq 0) ($vReal.Bad -join ' | ')
    Check 'z-order 記錄：實跑片段 → 過渡狀態 1 筆，含時間與 HWND' ($vReal.Transitional.Count -eq 1 -and $vReal.Transitional[0] -match '10:00:02\.290' -and $vReal.Transitional[0] -match '0x2507A0' -and $vReal.Transitional[0] -match '建立中') ($vReal.Transitional -join ' | ')
    Check 'z-order 記錄：實跑片段 → 3 筆狀態記錄、含格線 4 筆' ($vReal.Lines -eq 3 -and $vReal.WithOverlay -eq 4) "lines=$($vReal.Lines) with=$($vReal.WithOverlay)"

    $vCreate = Get-OverlayZOrderLogVerdict -OverlayClass $gc -Lines @(
        (LG '10:00:00.000' @(@{ H = '0xA1'; V = '0'; Below = 'Notepad' })),
        (LG '10:00:00.200' @(@{ H = '0xA1'; V = '0'; Below = 'Notepad' })),
        (LG '10:00:00.400' @(@{ H = '0xA1'; V = '1' })))
    Check 'z-order 記錄：建立中（連續兩筆 visible=0、之後可見）→ 不異常、過渡 2 筆' ($vCreate.Bad.Count -eq 0 -and $vCreate.Transitional.Count -eq 2) "bad=$($vCreate.Bad -join ' | ') tr=$($vCreate.Transitional -join ' | ')"

    $vDestroy = Get-OverlayZOrderLogVerdict -OverlayClass $gc -Lines @(
        (LG '10:00:00.000' @(@{ H = '0xA1'; V = '1' })),
        (LG '10:00:00.200' @(@{ H = '0xA1'; V = '0'; Below = 'Notepad' })),
        (LG '10:00:00.400' @()))
    Check 'z-order 記錄：銷毀中（之前可見、visible=0 後不再出現）→ 不異常、過渡 1 筆（銷毀中）' ($vDestroy.Bad.Count -eq 0 -and $vDestroy.Transitional.Count -eq 1 -and $vDestroy.Transitional[0] -match '銷毀中' -and $vDestroy.Transitional[0] -match '0xA1') "bad=$($vDestroy.Bad -join ' | ') tr=$($vDestroy.Transitional -join ' | ')"

    $vNever = Get-OverlayZOrderLogVerdict -OverlayClass $gc -Lines @(
        (LG '10:00:00.000' @(@{ H = '0xA1'; V = '0' })),
        (LG '10:00:00.200' @(@{ H = '0xA1'; V = '0' })),
        (LG '10:00:00.400' @(@{ H = '0xB2'; V = '1' })))
    Check 'z-order 記錄：某格線從未可見 → 異常（指出該 HWND）' ($vNever.Bad.Count -ge 1 -and ($vNever.Bad -join ' ') -match '0xA1' -and -not (($vNever.Bad -join ' ') -match '0xB2')) ($vNever.Bad -join ' | ')

    $vBelow = Get-OverlayZOrderLogVerdict -OverlayClass $gc -Lines @(
        (LG '10:00:00.000' @(@{ H = '0xA1'; V = '1' })),
        (LG '10:00:00.200' @(@{ H = '0xA1'; V = '1'; Below = 'Chrome_WidgetWin_1' })),
        (LG '10:00:00.400' @(@{ H = '0xA1'; V = '1' })))
    Check 'z-order 記錄：可見但 below 有一般視窗 → 異常' ($vBelow.Bad.Count -eq 1 -and $vBelow.Bad[0] -match '10:00:00\.200' -and $vBelow.Bad[0] -match 'below=Chrome_WidgetWin_1') ($vBelow.Bad -join ' | ')

    $vMin = Get-OverlayZOrderLogVerdict -OverlayClass $gc -Lines @(
        (LG '10:00:00.000' @(@{ H = '0xA1'; V = '1'; Min = '1' })),
        (LG '10:00:00.200' @(@{ H = '0xA1'; V = '1'; Clk = '1' })))
    Check 'z-order 記錄：可見但 minimized=1 → 異常' (@($vMin.Bad | Where-Object { $_ -match 'minimized=1' }).Count -eq 1) ($vMin.Bad -join ' | ')
    Check 'z-order 記錄：可見但 cloaked=1 → 異常' (@($vMin.Bad | Where-Object { $_ -match 'cloaked=1' }).Count -eq 1) ($vMin.Bad -join ' | ')

    # 建立豁免只適用於該 HWND 第一次 visible=1 之前：曾可見後再 visible=0、之後又可見（1→0→1）→ 異常。
    $vBlink = Get-OverlayZOrderLogVerdict -OverlayClass $gc -Lines @(
        (LG '10:00:00.000' @(@{ H = '0xA1'; V = '1' })),
        (LG '10:00:00.200' @(@{ H = '0xA1'; V = '0' })),
        (LG '10:00:00.400' @(@{ H = '0xA1'; V = '1' })))
    Check 'z-order 記錄：1→0→1（曾可見後又 visible=0）→ 異常、不算建立中' ($vBlink.Bad.Count -eq 1 -and $vBlink.Bad[0] -match '10:00:00\.200' -and $vBlink.Transitional.Count -eq 0) "bad=$($vBlink.Bad -join ' | ') tr=$($vBlink.Transitional -join ' | ')"

    # 銷毀豁免要求之後至少一筆完整狀態記錄缺少該 HWND：1→0→記錄結束（沒有之後的記錄）→ 異常。
    $vEnd = Get-OverlayZOrderLogVerdict -OverlayClass $gc -Lines @(
        (LG '10:00:00.000' @(@{ H = '0xA1'; V = '1' })),
        (LG '10:00:00.200' @(@{ H = '0xA1'; V = '0' })),
        '# watch-zorder.ps1 session end=2026-10-09T10:00:00.400+08:00 stateLines=2')
    Check 'z-order 記錄：1→0→記錄結束（沒有之後缺少該 HWND 的記錄）→ 異常、不算銷毀中' ($vEnd.Bad.Count -eq 1 -and $vEnd.Bad[0] -match '10:00:00\.200' -and $vEnd.Transitional.Count -eq 0) "bad=$($vEnd.Bad -join ' | ') tr=$($vEnd.Transitional -join ' | ')"

    # 之前可見、之後仍出現但再也不可見（1→0→0，記錄就此結束）→ 兩筆都異常。
    $vHidden = Get-OverlayZOrderLogVerdict -OverlayClass $gc -Lines @(
        (LG '10:00:00.000' @(@{ H = '0xA1'; V = '1' })),
        (LG '10:00:00.200' @(@{ H = '0xA1'; V = '0' })),
        (LG '10:00:00.400' @(@{ H = '0xA1'; V = '0' })))
    Check 'z-order 記錄：1→0→0（記錄結束）→ 兩筆都異常' ($vHidden.Bad.Count -eq 2 -and $vHidden.Transitional.Count -eq 0) "bad=$($vHidden.Bad -join ' | ') tr=$($vHidden.Transitional -join ' | ')"

    # 之後的記錄以 status=not_found 明確標出該 HWND（-TargetHwnd 監控時的格式）＝也算缺少 → 銷毀中。
    $vNf = Get-OverlayZOrderLogVerdict -OverlayClass $gc -Lines @(
        (LG '10:00:00.000' @(@{ H = '0xA1'; V = '1' })),
        (LG '10:00:00.200' @(@{ H = '0xA1'; V = '0' })),
        '2026-10-09T10:00:00.400+08:00 explorerPid=1 fgClass=Notepad fgPid=5 win[0xA1].status=not_found')
    Check 'z-order 記錄：之後記錄 status=not_found → 銷毀中、不異常' ($vNf.Bad.Count -eq 0 -and $vNf.Transitional.Count -eq 1 -and $vNf.Transitional[0] -match '銷毀中') "bad=$($vNf.Bad -join ' | ') tr=$($vNf.Transitional -join ' | ')"

    $vNone = Get-OverlayZOrderLogVerdict -OverlayClass $gc -Lines @((LG '10:00:00.000' @()))
    Check 'z-order 記錄：沒有任何格線 → WithOverlay=0（呼叫端判 FAIL）' ($vNone.WithOverlay -eq 0 -and $vNone.Bad.Count -eq 0)
}

# verify-grid-overlay.ps1 的 Test-ZOrderLog（以 AST 取出）：實跑片段寫成檔案後判讀，異常 0 筆、過渡 1 筆。
$gAst = [System.Management.Automation.Language.Parser]::ParseFile((Join-Path $toolsDir 'verify-grid-overlay.ps1'), [ref]$null, [ref]$null)
$fTz = $gAst.Find({ param($x) $x -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $x.Name -eq 'Test-ZOrderLog' }, $true)
Check 'verify-grid-overlay.ps1：找得到 Test-ZOrderLog' ($null -ne $fTz)
if ($fTz) {
    $OverlayClass = 'fc-host-grid-overlay'
    . ([scriptblock]::Create($fTz.Extent.Text))
    $tmpLog = Join-Path ([IO.Path]::GetTempPath()) ("grid-zorder-test-{0}.log" -f [guid]::NewGuid().ToString('N'))
    try {
        [IO.File]::WriteAllLines($tmpLog, [string[]](@('# watch-zorder.ps1 session start=2026-10-09T09:59:58.388+08:00') + $realGridLog))
        $zr = Test-ZOrderLog $tmpLog
        Check 'verify-grid-overlay.ps1 Test-ZOrderLog：實跑片段 → 異常 0 筆' ($zr.Bad.Count -eq 0) ($zr.Bad -join ' | ')
        $trProp = $zr.PSObject.Properties['Transitional']
        Check 'verify-grid-overlay.ps1 Test-ZOrderLog：實跑片段 → 回傳過渡狀態 1 筆' ($null -ne $trProp -and @($trProp.Value).Count -eq 1)
        Check 'verify-grid-overlay.ps1 Test-ZOrderLog：3 筆狀態記錄、含格線 4 筆' ($zr.Lines -eq 3 -and $zr.WithOverlay -eq 4) "lines=$($zr.Lines) with=$($zr.WithOverlay)"
    } finally { Remove-Item $tmpLog -ErrorAction SilentlyContinue }
}
$gsrcZ = [IO.File]::ReadAllText((Join-Path $toolsDir 'verify-grid-overlay.ps1'))
Check 'verify-grid-overlay.ps1：過渡狀態寫進 driver log 為 NOTE' ($gsrcZ -match 'foreach \(\$t in \$zr\.Transitional\) \{ Log "  NOTE ')

Write-Host ''
Write-Host "合計：$($script:Pass) PASS、$($script:Fail) FAIL"
if ($script:Fail -gt 0) { exit 1 }
exit 0
