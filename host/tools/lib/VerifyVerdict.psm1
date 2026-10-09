<#
.SYNOPSIS
    host/tools 驗收腳本共用的「判讀」純函式（不呼叫任何 Win32 API、不啟動宿主）。

.DESCRIPTION
    驗收腳本把實機記錄（watch-zorder 的 z-order 記錄、逐項結果）交給這裡判讀，判讀邏輯因此
    可以用 host/tools/tests/VerifyVerdict.Tests.ps1 以合成記錄做 mock 測試（Codex review 指出的
    「假通過／假失敗」都能在不碰桌面的情況下重現與防回歸）。

    z-order 記錄格式見 watch-zorder.ps1：每行 `<時間戳> key=value ...`，時間戳到第一個空白為止；
    小工具＝`win[0x..].class=Tauri Window` 的目標（值以空白切開後只剩 `Tauri`）。
#>
Set-StrictMode -Version Latest

$script:Inv = [Globalization.CultureInfo]::InvariantCulture

function ConvertFrom-ZOrderLine {
    <# 一行 z-order 記錄 → @{ Ts [DateTimeOffset]; Raw; Win = @{ hex = @{ 欄位 = 值 } } }。 #>
    param([Parameter(Mandatory)][string]$Line)
    $ts = [DateTimeOffset]::Parse($Line.Substring(0, $Line.IndexOf(' ')), $script:Inv)
    $win = @{}
    foreach ($m in [regex]::Matches($Line, 'win\[(0x[0-9A-Fa-f]+)\]\.(\w+)=(\S+)')) {
        $h = $m.Groups[1].Value
        if (-not $win.ContainsKey($h)) { $win[$h] = @{} }
        $win[$h][$m.Groups[2].Value] = $m.Groups[3].Value
    }
    [PSCustomObject]@{ Ts = $ts; Raw = $Line; Win = $win }
}

function Get-ZOrderLineViolations {
    <#
    task 3.3 的單行判準（沿用修正前 verify-3.3.ps1 的規則）：每個可見小工具 below 必須是
    (none)；visible=0 只有在 dupInSnapshot（列舉途中剛好重排）時可略過，否則也算異常。
    回傳 @{ HasWidget; Violations = string[] }。
    #>
    param([Parameter(Mandatory)]$Parsed)
    $v = New-Object System.Collections.Generic.List[string]
    $hasWidget = $false
    foreach ($h in $Parsed.Win.Keys) {
        $w = $Parsed.Win[$h]
        if (-not $w.ContainsKey('class') -or $w['class'] -ne 'Tauri') { continue }
        if ($w['visible'] -ne '1') {
            if ($w.ContainsKey('dupInSnapshot')) { continue }
            $v.Add("$h visible=0 但未重複出現"); continue
        }
        $hasWidget = $true
        if ($w['below'] -ne '(none)') { $v.Add("$h below=$($w['below'])") }
    }
    [PSCustomObject]@{ HasWidget = $hasWidget; Violations = @($v) }
}

function Get-SafetyRecoveryVerdict {
    <#
    task 3.3 分階段判讀（Codex task-3.3 [medium] 第二點：原本整段記錄只要出現過 below 非空就
    判失敗，把「故意製造、預期會被修復」的故障窗口也算成永久失敗）。

    -Windows：預期會短暫異常的窗口，每個 @{ Name; Start; End }（DateTimeOffset）。窗口內的
      異常行不算違規，但**窗口結束當下（End 之前最後一行）的狀態必須已恢復**，否則算違規。
      窗口以外（基準期、平靜期）的任何異常行都算違規。
    -FaultName：Windows 中代表「人為故障注入」的那一個；另外要求它的窗口內**確實觀察到異常**
      （否則故障根本沒成立，無從證明保險檢查會修復），並回報恢復耗時。
    -FaultConfirmedAt：腳本以自己的 z-order 快照確認故障成立的時間（恢復期限從這裡起算）。

    回傳 @{ WidgetLines; OutsideViolations; FaultObserved; FaultRecovered; RecoverySec; Notes }。
    #>
    param(
        [AllowEmptyCollection()][string[]]$Lines = @(),
        [object[]]$Windows = @(),
        [string]$FaultName = '',
        $FaultConfirmedAt = $null
    )
    $parsed = @($Lines | Where-Object { $_ -and $_ -notmatch '^#' -and $_.Trim() } | ForEach-Object {
            $p = ConvertFrom-ZOrderLine $_
            $r = Get-ZOrderLineViolations $p
            [PSCustomObject]@{ Ts = $p.Ts; HasWidget = $r.HasWidget; Violations = $r.Violations }
        })
    $outside = New-Object System.Collections.Generic.List[string]
    $notes = New-Object System.Collections.Generic.List[string]
    $widgetLines = @($parsed | Where-Object { $_.HasWidget }).Count
    foreach ($ln in $parsed) {
        if ($ln.Violations.Count -eq 0) { continue }
        $inWin = @($Windows | Where-Object { $ln.Ts -ge $_.Start -and $ln.Ts -le $_.End })
        if ($inWin.Count -gt 0) { continue }
        foreach ($x in $ln.Violations) { $outside.Add("$($ln.Ts.ToString('HH:mm:ss.fff')) $x") }
    }
    foreach ($w in $Windows) {
        $last = @($parsed | Where-Object { $_.Ts -le $w.End }) | Select-Object -Last 1
        if ($last -and $last.Violations.Count -gt 0) {
            $outside.Add("窗口「$($w.Name)」結束時（$($w.End.ToString('HH:mm:ss.fff'))）仍異常：$($last.Violations -join '; ')")
        }
    }

    $faultObserved = $false; $faultRecovered = $false; $recoverySec = $null
    $fw = @($Windows | Where-Object { $_.Name -eq $FaultName }) | Select-Object -First 1
    if ($fw) {
        $inFault = @($parsed | Where-Object { $_.Ts -ge $fw.Start -and $_.Ts -le $fw.End })
        $bad = @($inFault | Where-Object { $_.Violations.Count -gt 0 })
        $faultObserved = $bad.Count -gt 0
        if ($faultObserved) {
            $lastBad = $bad[-1]
            $fix = @($parsed | Where-Object { $_.Ts -gt $lastBad.Ts -and $_.Violations.Count -eq 0 }) | Select-Object -First 1
            if ($fix -and $fix.Ts -le $fw.End) {
                $faultRecovered = $true
                $from = if ($FaultConfirmedAt) { $FaultConfirmedAt } else { $fw.Start }
                $recoverySec = [Math]::Round(($fix.Ts - $from).TotalSeconds, 1)
            }
            $notes.Add("故障窗口內異常 $($bad.Count) 行；恢復＝$faultRecovered$(if ($null -ne $recoverySec) { "（確認後 $recoverySec 秒）" })")
        } else {
            $notes.Add('故障窗口內 z-order 記錄沒有任何異常行（故障未被觀察到）')
        }
    }
    [PSCustomObject]@{
        WidgetLines       = $widgetLines
        OutsideViolations = @($outside)
        FaultObserved     = $faultObserved
        FaultRecovered    = $faultRecovered
        RecoverySec       = $recoverySec
        Notes             = @($notes)
    }
}

function Get-FaultWindowEnd {
    <#
    task 3.3 故障窗口的結束時間（fix F3，review task-3.3-fixB-opus.md [medium]）：預設是「確認故障
    成立＋RecoverSec＋TolSec」，但若腳本在那之前就關掉故障視窗，End 要截在關閉那一刻——關閉後
    記錄自然變乾淨，那些乾淨行若仍落在窗口內，會被 Get-SafetyRecoveryVerdict 當成「已恢復」，
    置底其實沒生效也判 PASS。截斷後，恢復必須在故障視窗仍存在時就出現在 z-order 記錄裡。
    -FaultClosedAt 為 $null（尚未關閉或不知道）時回傳預設值。
    #>
    param(
        [Parameter(Mandatory)][DateTimeOffset]$ConfirmedAt,
        [Parameter(Mandatory)][int]$RecoverSec,
        [Parameter(Mandatory)][int]$TolSec,
        $FaultClosedAt = $null
    )
    $deadline = $ConfirmedAt.AddSeconds($RecoverSec + $TolSec)
    if ($null -ne $FaultClosedAt -and ([DateTimeOffset]$FaultClosedAt) -lt $deadline) { return [DateTimeOffset]$FaultClosedAt }
    return $deadline
}

function Get-ResultsExitCode {
    <#
    逐項結果（[ordered] 名稱 → bool 或 'SKIPPED'）→ 結束碼：任一項為假 → 1，否則 0；'SKIPPED'
    不計。沒有任何計分項（全部 SKIPPED 或空）也回 1——沒有證據不能算通過。
    （Codex task-4.7 [medium]：原本失敗只印 FAIL、仍以 0 結束。）
    #>
    param([Parameter(Mandatory)][System.Collections.IDictionary]$Results)
    $scored = @($Results.Keys | Where-Object { -not ($Results[$_] -is [string] -and $Results[$_] -eq 'SKIPPED') })
    if ($scored.Count -eq 0) { return 1 }
    $failed = @($scored | Where-Object { -not $Results[$_] })
    if ($failed.Count -gt 0) { return 1 }
    return 0
}

function Get-VerdictExitCode {
    <#
    驗收腳本統一的結束碼決策（review 31c7c53）。優先序：產品 FAIL（1）＞ 工作階段鎖定（2）＞ 環境（3）＞ 通過（0）。
    - 產品 FAIL：-Results 任一非 'SKIPPED' 項為假，或 -Failed（沒有逐項結果的腳本自己判定）。
    - 環境：-EnvBlocked，或 -NotRestored 非空（被暫時最小化的使用者視窗沒還原成功：目標無回應、最小化未確認、
      讀回逾時／不符——2026-10-06 的事故就是這種，本質是環境，不得被當成產品回歸）。
    - -RequireResults：沒有任何計分項且沒有鎖定／環境 → 1（沿用 Get-ResultsExitCode「沒有證據不能算通過」）。
    未還原的警示行由 Format-UnrestoredWarning 產生、與結束碼無關（有產品 FAIL 時摘要照樣列出）。
    #>
    param(
        [System.Collections.IDictionary]$Results = $null,
        [switch]$Failed, [switch]$Locked, [switch]$EnvBlocked,
        [AllowNull()][AllowEmptyCollection()][string[]]$NotRestored = @(),
        [switch]$RequireResults
    )
    $scored = @()
    if ($null -ne $Results) { $scored = @($Results.Keys | Where-Object { -not ($Results[$_] -is [string] -and $Results[$_] -eq 'SKIPPED') }) }
    if ($Failed -or @($scored | Where-Object { -not $Results[$_] }).Count -gt 0) { return 1 }
    if ($Locked) { return 2 }
    if ($EnvBlocked -or @($NotRestored).Where({ $_ }).Count -gt 0) { return 3 }
    if ($RequireResults -and $scored.Count -eq 0) { return 1 }
    return 0
}

function Format-UnrestoredWarning {
    <# 未還原的使用者視窗 → 摘要用的醒目警示行；沒有未還原 → 空字串。 #>
    param([AllowNull()][AllowEmptyCollection()][string[]]$NotRestored = @())
    $items = @(@($NotRestored).Where({ $_ }))
    if ($items.Count -eq 0) { return '' }
    return "⚠ 使用者視窗未還原 $($items.Count) 扇：$($items -join '；')，需手動還原"
}

function Test-InactiveWidgetPrecondition {
    <#
    「非使用中視窗」前提（Codex task-4.7 [medium]）：送滾輪之前，前景必須是有效視窗、且不屬於
    宿主行程——否則小工具本身就是使用中視窗，滾輪能捲也證明不了「非使用中視窗滾輪路由」。
    回傳 @{ Ok; Reason }。
    #>
    param([Parameter(Mandatory)][IntPtr]$FgHwnd, [Parameter(Mandatory)][int]$FgPid, [Parameter(Mandatory)][int]$HostPid)
    if ($FgHwnd -eq [IntPtr]::Zero) { return [PSCustomObject]@{ Ok = $false; Reason = '前景視窗為空（GetForegroundWindow 回傳 0）' } }
    if ($FgPid -le 0) { return [PSCustomObject]@{ Ok = $false; Reason = '查不到前景視窗的擁有者 PID' } }
    if ($FgPid -eq $HostPid) { return [PSCustomObject]@{ Ok = $false; Reason = "前景視窗屬於宿主（pid=$HostPid）：小工具是使用中視窗，無法驗證非使用中視窗的滾輪路由" } }
    return [PSCustomObject]@{ Ok = $true; Reason = "前景屬其他行程（pid=$FgPid）" }
}

function Get-ExpectedWidgetProblems {
    <#
    task 3.4 可見性判準（Codex task-3.4 [medium]：原本只檢查「記錄裡還列得到」的小工具，小工具
    被隱藏或消失時它根本不會出現在 -ProcessName 的清單裡，缺席等於通過）。對「啟動時取得的每個
    小工具 HWND」逐一要求：記錄中存在（非 status=not_found）、visible=1、minimized=0、cloaked=0；
    -RequireDesktopNotAbove 時另要求 desktopAbove=0。缺一即回報問題。
    visible≠1 但帶 dupInSnapshot（列舉途中剛好重排、欄位取自舊位置）沿用既有慣例略過該行該項。
    -Win：一行 z-order 記錄解析後的 @{ hex = @{ 欄位 = 值 } }。回傳問題字串陣列（空＝全部符合；
    呼叫端請以 @(...) 包住，單一元素時 PowerShell 會攤平成字串）。
    #>
    param(
        [Parameter(Mandatory)][hashtable]$Win,
        [Parameter(Mandatory)][AllowEmptyCollection()][string[]]$Expected,
        [switch]$RequireDesktopNotAbove
    )
    $problems = New-Object System.Collections.Generic.List[string]
    if ($Expected.Count -eq 0) { $problems.Add('沒有任何預期的小工具 HWND（啟動時就沒有小工具）'); return @($problems) }
    foreach ($hex in $Expected) {
        $key = @($Win.Keys | Where-Object { $_ -ieq $hex }) | Select-Object -First 1
        if (-not $key) { $problems.Add("$hex 不在記錄中（消失）"); continue }
        $f = $Win[$key]
        if ($f.ContainsKey('status') -and $f['status'] -eq 'not_found') { $problems.Add("$hex status=not_found（視窗已不存在）"); continue }
        if ($f['visible'] -ne '1' -and $f.ContainsKey('dupInSnapshot')) { continue }
        $bad = @()
        if ($f['visible'] -ne '1') { $bad += "visible=$($f['visible'])" }
        if ($f['minimized'] -ne '0') { $bad += "minimized=$($f['minimized'])" }
        if ($f['cloaked'] -ne '0') { $bad += "cloaked=$($f['cloaked'])" }
        if ($RequireDesktopNotAbove -and $f['desktopAbove'] -ne '0') { $bad += "desktopAbove=$($f['desktopAbove'])" }
        if ($bad.Count -gt 0) { $problems.Add("$hex $($bad -join ' ')") }
    }
    return @($problems)
}

function Get-ForegroundResult {
    <#
    verify-grid-overlay 的前景判準（規格：前景視窗與焦點不因格線而改變）。回傳結果項的值：
    - $true（PASS）：進入後前景與進入前相同（-Before／-After 為 HWND）。
    - $false（FAIL）：前景換成宿主行程的視窗（-AfterPid＝-HostPid；任何小工具或格線）。
    - 'NOT-RUN'：前景換成別的行程的視窗——不是宿主造成，但也無法證明格線沒動到焦點，故不得記為成功
      （結果項標 NOT-RUN，結束碼走 Get-VerdictExitCode 的環境碼 3）。
    #>
    param([Parameter(Mandatory)][IntPtr]$Before, [Parameter(Mandatory)][IntPtr]$After,
        [Parameter(Mandatory)][int]$AfterPid, [Parameter(Mandatory)][int]$HostPid)
    if ($Before -eq $After) { return $true }
    if ($AfterPid -eq $HostPid) { return $false }
    return 'NOT-RUN'
}

function Get-OverlayDesktopProblems {
    <#
    z-order 穩定判定（verify-grid-overlay）：每個格線都必須位於「與它重疊的可見桌面視窗（-DesktopClasses）」之上。
    -ZList：由上到下的快照，每項 @{ Hwnd; Index; Class; Visible; Rect（X,Y,W,H 或 $null） }；
    -Overlays：@{ Hwnd; Rect }；-Screens：@{ DeviceName; WorkingArea（X,Y,Width,Height） }，只用來在訊息中指出是哪一台。
    回傳問題描述 string[]（空＝穩定）；非空且等待逾時＝該格線與螢幕就是逾時項（呼叫端必須判 FAIL）。
    #>
    param([Parameter(Mandatory)]$ZList, [Parameter(Mandatory)]$Overlays, [Parameter(Mandatory)][string[]]$DesktopClasses, $Screens = @())
    $problems = New-Object System.Collections.Generic.List[string]
    foreach ($o in @($Overlays)) {
        $r = $o.Rect
        # 逐項走訪（不用 @($Screens) 包裝：PowerShell 對 Screen[] 做陣列轉換會丟 Argument types do not match）。
        $scrName = '<未對應螢幕>'
        foreach ($s in $Screens) {
            $wa = $s.WorkingArea
            if ($wa.X -eq $r.X -and $wa.Y -eq $r.Y -and $wa.Width -eq $r.W -and $wa.Height -eq $r.H) { $scrName = $s.DeviceName; break }
        }
        $oh = '0x{0:X}' -f $o.Hwnd.ToInt64()
        $oz = $null
        foreach ($z in $ZList) { if ($z.Hwnd -eq $o.Hwnd) { $oz = $z; break } }
        if ($null -eq $oz) { $problems.Add("格線 $oh（螢幕 $scrName）不在 z-order 列舉中"); continue }
        foreach ($w in $ZList) {
            if (-not ($DesktopClasses -contains $w.Class) -or -not $w.Visible -or -not $w.Rect) { continue }
            if ($w.Rect.W -le 0 -or $w.Rect.H -le 0) { continue }
            if ($w.Rect.X -ge ($r.X + $r.W) -or ($w.Rect.X + $w.Rect.W) -le $r.X -or $w.Rect.Y -ge ($r.Y + $r.H) -or ($w.Rect.Y + $w.Rect.H) -le $r.Y) { continue }
            if ($w.Index -lt $oz.Index) {
                $problems.Add("格線 $oh（螢幕 $scrName）z=$($oz.Index) 在桌面 0x$('{0:X}' -f $w.Hwnd.ToInt64()) class=$($w.Class) z=$($w.Index) 之下")
            }
        }
    }
    return @($problems)
}

function Get-OverlayZOrderLogVerdict {
    <#
    watch-zorder 記錄判讀（verify-grid-overlay 步驟 3）：逐個格線 HWND（class＝-OverlayClass）看它在記錄中的所有取樣。
    - visible=1 的取樣必須 minimized=0、cloaked=0、below=(none)，否則異常。
    - visible=0 的取樣是過渡狀態、不算異常，只在下列兩種情形之一：
      建立中＝在該 HWND 第一次 visible=1 之前、且之後確實有 visible=1 的取樣（CreateWindowEx 後、ShowWindow＋置底前，
      新視窗位在 z-order 最上層，所以 below 會是一般視窗）；銷毀中＝之前有 visible=1、這是該 HWND 的最後一筆取樣，
      且之後至少還有一筆完整狀態記錄（該記錄缺少這個 HWND，或以 status=not_found 標出）——只因記錄恰好結束而
      沒有後續取樣，分不出銷毀與藏起來，不算。曾可見之後的其他 visible=0（1→0→1、1→0→記錄結束）仍是異常。
      不可見的視窗不繪製、不攔截滑鼠，不違反規格。
      記錄格式（watch-zorder.ps1 -ProcessId）：每筆列出該行程當下所有可見頂層視窗，visible=0 只會在列舉途中
      剛好重排、同一 HWND 出現兩次（dupInSnapshot）時出現；所以之後的記錄缺少某 HWND＝它已不可見或已關閉。
    - 某 HWND 從未出現 visible=1 → 異常（一筆，指出該 HWND）。
    -Lines：記錄行（註解行與空行會略過）。回傳 @{ Lines（狀態記錄筆數）; WithOverlay（格線取樣數）;
    Bad（異常描述 string[]）; Transitional（過渡狀態描述 string[]，呼叫端記為 NOTE）}。
    #>
    param([Parameter(Mandatory)][AllowEmptyCollection()][string[]]$Lines, [Parameter(Mandatory)][string]$OverlayClass)
    $states = @($Lines | Where-Object { $_ -and $_ -notmatch '^#' })
    # HWND → 依記錄順序的取樣清單（保留第一次出現的順序，讓輸出依時間排列）。
    $byHwnd = [ordered]@{}
    $withOverlay = 0
    for ($li = 0; $li -lt $states.Count; $li++) {
        $p = ConvertFrom-ZOrderLine $states[$li]
        foreach ($h in $p.Win.Keys) {
            $w = $p.Win[$h]
            if (-not $w.ContainsKey('class') -or $w['class'] -ne $OverlayClass) { continue }
            $withOverlay++
            if (-not $byHwnd.Contains($h)) { $byHwnd[$h] = New-Object System.Collections.Generic.List[object] }
            $byHwnd[$h].Add([PSCustomObject]@{ Ts = $p.Ts; W = $w; LineIdx = $li })
        }
    }
    $bad = New-Object System.Collections.Generic.List[string]
    $transitional = New-Object System.Collections.Generic.List[string]
    foreach ($h in $byHwnd.Keys) {
        $samples = $byHwnd[$h]
        $visIdx = @(for ($i = 0; $i -lt $samples.Count; $i++) { if ($samples[$i].W['visible'] -eq '1') { $i } })
        if ($visIdx.Count -eq 0) {
            $bad.Add("$h 從未可見（$($samples.Count) 筆皆 visible=0，首筆 $($samples[0].Ts.ToString('HH:mm:ss.fff'))）")
            continue
        }
        for ($i = 0; $i -lt $samples.Count; $i++) {
            $s = $samples[$i]; $w = $s.W
            $desc = "$($s.Ts.ToString('HH:mm:ss.fff')) $h visible=$($w['visible']) minimized=$($w['minimized']) cloaked=$($w['cloaked']) below=$($w['below'])"
            if ($w['visible'] -eq '1') {
                if ($w['minimized'] -ne '0' -or $w['cloaked'] -ne '0' -or $w['below'] -ne '(none)') { $bad.Add($desc) }
                continue
            }
            # 建立中：第一次可見之前（$visIdx 非空，所以之後一定有 visible=1）。
            # 銷毀中：曾可見、這是最後一筆取樣，且之後還有完整狀態記錄（那些記錄都不含這個格線取樣）。
            if ($i -lt $visIdx[0]) {
                $transitional.Add("$desc（建立中：$($samples[$visIdx[0]].Ts.ToString('HH:mm:ss.fff')) 首次可見）")
            } elseif ($i -eq $samples.Count - 1 -and $s.LineIdx -lt $states.Count - 1) {
                $transitional.Add("$desc（銷毀中：之前可見、之後的記錄不再有它）")
            } else {
                $bad.Add("$desc（不是建立中或銷毀中的過渡狀態）")
            }
        }
    }
    [PSCustomObject]@{ Lines = $states.Count; WithOverlay = $withOverlay; Bad = @($bad); Transitional = @($transitional) }
}

Export-ModuleMember -Function ConvertFrom-ZOrderLine, Get-ZOrderLineViolations, Get-SafetyRecoveryVerdict,
Get-FaultWindowEnd, Get-ResultsExitCode, Test-InactiveWidgetPrecondition, Get-ExpectedWidgetProblems,
Get-VerdictExitCode, Format-UnrestoredWarning, Get-ForegroundResult, Get-OverlayDesktopProblems, Get-OverlayZOrderLogVerdict
