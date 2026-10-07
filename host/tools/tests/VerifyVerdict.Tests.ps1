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

Write-Host ''
Write-Host "合計：$($script:Pass) PASS、$($script:Fail) FAIL"
if ($script:Fail -gt 0) { exit 1 }
exit 0
