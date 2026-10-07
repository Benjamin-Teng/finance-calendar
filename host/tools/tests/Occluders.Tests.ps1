<#
.SYNOPSIS
    host/tools/lib/Occluders.psm1（暫時最小化遮擋小工具的使用者視窗、結束時照原位置還原）的 mock 測試，
    以及 verify-4.7-wheel／verify-4.7-wheelrouting 改用它的結構檢查（純 PowerShell 斷言，不需 Pester）。

.DESCRIPTION
    **不動任何真實視窗**：Clear-Occluders 的命中測試與最小化以 -HitTest／-Minimize 注入假的實作；
    Restore-Occluders 以 -Restore 注入。兩支滾輪腳本不執行，只解析語法樹。

    fix F7（批次 B）：4.7 兩支滾輪腳本遇到使用者視窗遮擋就判 FAIL；改為與 7.5／7.6／5.3 一致——
    開跑前暫時最小化遮擋的使用者視窗、結束時還原，仍被遮擋才判 BLOCKED（結束碼 3）。

.EXAMPLE
    pwsh -NoProfile -File host/tools/tests/Occluders.Tests.ps1
    結束碼 0＝全部通過、1＝有失敗。
#>
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$toolsDir = Split-Path $PSScriptRoot -Parent
$modPath = Join-Path $toolsDir 'lib\Occluders.psm1'

$script:Pass = 0
$script:Fail = 0
function Check([string]$name, [bool]$cond, [string]$detail = '') {
    if ($cond) { $script:Pass++; Write-Host "PASS  $name" }
    else { $script:Fail++; Write-Host "FAIL  $name $detail" -ForegroundColor Red }
}

Check 'lib/Occluders.psm1 存在' (Test-Path $modPath)
if (Test-Path $modPath) {
    $occMod = Import-Module $modPath -Force -PassThru
    # fix F8b：Invoke-MinimizeWindow／Invoke-RestoreWindow 不再匯出（腳本不得繞過白名單），測試經模組範圍呼叫。
    function Invoke-ModuleMinimize { param($Hwnd, $Backend) & $occMod { param($h, $b) Invoke-MinimizeWindow -Hwnd $h -Backend $b } $Hwnd $Backend }
    function Invoke-ModuleRestore { param($Hwnd, $Snapshot, $Backend) & $occMod { param($h, $s, $b) Invoke-RestoreWindow -Hwnd $h -Snapshot $s -Backend $b } $Hwnd $Snapshot $Backend }
    $target = [IntPtr]0x100
    $hostPid = 50

    # ── Get-OccluderAction ───────────────────────────────────────────────────────
    # fix F7b：白名單——只有「一般應用程式主視窗」才可最小化（可見、未 cloaked、無 owner、非 TOOLWINDOW、
    # 非 TOPMOST、非 DISABLED、具 MINIMIZEBOX、不是系統類別、不屬殼層 explorer 或系統 UI 行程）。
    # New-Win 的預設值＝一扇一般的 Chrome 主視窗；各案例只改一個屬性。
    $shellPid = 4000
    function New-Win {
        param([IntPtr]$Hwnd = [IntPtr]0x500, [int]$Pid_ = 99, [string]$Class = 'Chrome_WidgetWin_1',
            [bool]$Visible = $true, [bool]$Cloaked = $false, [IntPtr]$Owner = [IntPtr]::Zero,
            [long]$Style = 0x16CF0000, [long]$ExStyle = 0x00000100, [string]$ProcessName = 'chrome', [bool]$Hung = $false)
        [PSCustomObject]@{ Hwnd = $Hwnd; Pid = $Pid_; Class = $Class; Visible = $Visible; Cloaked = $Cloaked
            Owner = $Owner; Style = $Style; ExStyle = $ExStyle; ProcessName = $ProcessName; Hung = $Hung }
    }
    function Act($w, [IntPtr]$tgt = $target, [int[]]$own = @()) {
        Get-OccluderAction -Window $w -HostPid $hostPid -TargetHwnd $tgt -OwnPids $own -ShellPid $shellPid
    }
    Check '命中目標小工具 → clear' ((Act (New-Win -Hwnd $target -Pid_ $hostPid -Class 'Tauri Window')) -eq 'clear')
    Check '命中宿主的另一扇小工具 → host-other（不得最小化宿主自己的視窗）' ((Act (New-Win -Hwnd ([IntPtr]0x200) -Pid_ $hostPid -Class 'Tauri Window')) -eq 'host-other')
    Check '沒給 TargetHwnd 時命中宿主任一視窗 → clear' ((Act (New-Win -Hwnd ([IntPtr]0x200) -Pid_ $hostPid -Class 'Tauri Window') ([IntPtr]::Zero)) -eq 'clear')
    foreach ($cls in 'Progman', 'WorkerW', 'Shell_TrayWnd') {
        Check "殼層 $cls → shell（不最小化）" ((Act (New-Win -Hwnd ([IntPtr]0x300) -Pid_ 7 -Class $cls)) -eq 'shell')
    }
    Check '腳本自己的視窗 → own（配置錯誤，不最小化）' ((Act (New-Win -Hwnd ([IntPtr]0x400) -Pid_ 77 -Class 'WindowsForms10') $target @(77)) -eq 'own')
    Check '一般使用者視窗 → minimize' ((Act (New-Win)) -eq 'minimize')
    Check '一般 Terminal 主視窗 → minimize' ((Act (New-Win -Class 'CASCADIA_HOSTING_WINDOW_CLASS' -ProcessName 'WindowsTerminal')) -eq 'minimize')
    Check '沒命中任何視窗（0）→ none' ((Act (New-Win -Hwnd ([IntPtr]::Zero) -Pid_ 0 -Class '')) -eq 'none')

    # 被排除的每一類 → system（不動、由腳本判 BLOCKED）。
    $excluded = [ordered]@{
        '不可見'                             = New-Win -Visible $false
        'DWM cloaked'                        = New-Win -Cloaked $true
        '有 owner（對話框／彈出視窗）'       = New-Win -Owner ([IntPtr]0x900)
        'WS_EX_TOOLWINDOW'                   = New-Win -ExStyle 0x00000180
        'WS_EX_TOPMOST'                      = New-Win -ExStyle 0x00000108
        '沒有 WS_MINIMIZEBOX'                = New-Win -Style 0x16CD0000
        'WS_DISABLED（有強制回應對話框）'    = New-Win -Style 0x1ECF0000
        '屬殼層 explorer 行程'               = New-Win -Pid_ $shellPid -Class 'CabinetWClass' -ProcessName 'explorer'
        'PickerHost 行程（檔案選擇器）'      = New-Win -Class 'ApplicationFrameWindow' -ProcessName 'PickerHost'
        '目標無回應（IsHungAppWindow）'      = New-Win -Hung $true
    }
    foreach ($cls in 'Shell_SecondaryTrayWnd', 'Shell_SystemDim', 'Shell_SystemDialog', 'Windows.UI.Core.CoreWindow', '#32770',
        'NotifyIconOverflowWindow', 'TopLevelWindowForOverflowXamlIsland', 'tooltips_class32', 'Xaml_WindowedPopupClass') {
        $excluded["系統類別 $cls"] = New-Win -Class $cls
    }
    foreach ($k in $excluded.Keys) {
        $a = Act $excluded[$k]
        Check "排除：$k → system（不最小化）" ($a -eq 'system') "got=$a"
    }
    Check '缺屬性的命中結果（舊格式）→ system（不在白名單即不動）' ((Get-OccluderAction -Window ([PSCustomObject]@{ Hwnd = [IntPtr]0x500; Pid = 99; Class = 'Chrome_WidgetWin_1' }) -HostPid $hostPid -TargetHwnd $target -ShellPid $shellPid) -eq 'system')
    Check 'Get-NotMinimizableReason：一般視窗回空字串' ((Get-NotMinimizableReason -Window (New-Win) -ShellPid $shellPid) -eq '')
    Check 'Get-NotMinimizableReason：有 owner 寫明原因' ((Get-NotMinimizableReason -Window (New-Win -Owner ([IntPtr]0x900)) -ShellPid $shellPid) -match 'owner')
    # 跨行程對無回應的視窗做同步視窗呼叫會讓腳本無限等待（2026-10-06 verify-7.3 收尾卡 30 分鐘）：
    # 無回應的視窗一律不碰（不最小化），由 Get-OccluderVerdict 判 ENV-BLOCKED。
    Check 'Get-NotMinimizableReason：無回應寫明原因' ((Get-NotMinimizableReason -Window (New-Win -Hung $true) -ShellPid $shellPid) -match '無回應')
    $noHung = New-Win; $noHung.PSObject.Properties.Remove('Hung')
    Check '缺 Hung 屬性 → system（無法確認有回應即不動）' ((Act $noHung) -eq 'system')
    Check 'Get-WindowInfo 以 IsHungAppWindow 填 Hung' ((& $occMod { ${function:Get-WindowInfo}.ToString() }) -match 'IsHungAppWindow')

    # ── Clear-Occluders（注入假的命中測試與最小化）─────────────────────────────────
    # 情境：(10,10) 依序被 Chrome、Terminal 蓋住，最小化兩扇後露出目標。
    $stack = New-Object System.Collections.Generic.List[object]
    $stack.Add((New-Win -Hwnd ([IntPtr]0x500) -Pid_ 99))
    $stack.Add((New-Win -Hwnd ([IntPtr]0x600) -Pid_ 98 -Class 'CASCADIA_HOSTING_WINDOW_CLASS' -ProcessName 'WindowsTerminal'))
    $stack.Add((New-Win -Hwnd $target -Pid_ $hostPid -Class 'Tauri Window' -ProcessName 'fc-host'))
    $hit = { param($x, $y) $stack[0] }
    $minCalls = New-Object System.Collections.Generic.List[IntPtr]
    $min = { param($h) $minCalls.Add($h); $stack.RemoveAt(0); [PSCustomObject]@{ showCmd = 1 } }
    $list = New-Object System.Collections.Generic.List[object]
    $r = Clear-Occluders -HostPid $hostPid -TargetHwnd $target -Points @(, @(10, 10)) -Minimized $list -HitTest $hit -Minimize $min -SettleMs 0 -ShellPid $shellPid
    Check '兩層遮擋都最小化後 Ok' ($r.Ok -eq $true) "reason=$($r.Reason)"
    Check '依序最小化 Chrome、Terminal' ($minCalls.Count -eq 2 -and $minCalls[0] -eq [IntPtr]0x500 -and $minCalls[1] -eq [IntPtr]0x600)
    Check '記下兩筆原 placement 供還原' ($list.Count -eq 2 -and $list[0].Hwnd -eq [IntPtr]0x500)

    # 情境：被殼層蓋住（點落在小工具間隙）→ 不最小化、BLOCKED。
    $shellHit = { param($x, $y) [PSCustomObject]@{ Hwnd = [IntPtr]0x700; Pid = 7; Class = 'Progman' } }
    $minCalls.Clear(); $list2 = New-Object System.Collections.Generic.List[object]
    $r = Clear-Occluders -HostPid $hostPid -TargetHwnd $target -Points @(, @(2556, 260)) -Minimized $list2 -HitTest $shellHit -Minimize $min -SettleMs 0 -ShellPid $shellPid
    Check '殼層 → Ok=False、不最小化任何視窗' ($r.Ok -eq $false -and $minCalls.Count -eq 0 -and $list2.Count -eq 0) "reason=$($r.Reason)"
    Check '殼層 → 原因寫明 (x,y) 與類別' ($r.Reason -match '2556,260' -and $r.Reason -match 'Progman')

    # 情境（fix F7b）：被不在白名單的視窗（有 owner 的對話框、系統類別）蓋住 → 不最小化、BLOCKED。
    foreach ($w in @((New-Win -Hwnd ([IntPtr]0x710) -Class '#32770' -Owner ([IntPtr]0x500)), (New-Win -Hwnd ([IntPtr]0x720) -Class 'Shell_SystemDim' -ProcessName 'explorer'))) {
        $sysHit = { param($x, $y) $w }.GetNewClosure()
        $minCalls.Clear(); $listS = New-Object System.Collections.Generic.List[object]
        $r = Clear-Occluders -HostPid $hostPid -TargetHwnd $target -Points @(, @(30, 40)) -Minimized $listS -HitTest $sysHit -Minimize $min -SettleMs 0 -ShellPid $shellPid
        Check "非白名單 $($w.Class) → Ok=False、不最小化任何視窗" ($r.Ok -eq $false -and $minCalls.Count -eq 0 -and $listS.Count -eq 0) "reason=$($r.Reason)"
        Check "非白名單 $($w.Class) → 原因寫明 (x,y)、類別與不最小化理由" ($r.Reason -match '30,40' -and $r.Reason -match [regex]::Escape($w.Class) -and $r.Reason -match '不最小化')
    }

    # 情境：最小化後仍蓋著（視窗不肯最小化）→ 最多試 MaxPerPoint 次後 BLOCKED。
    $stuckHit = { param($x, $y) New-Win -Hwnd ([IntPtr]0x800) -Pid_ 97 -Class 'Stubborn' }
    $stuckMin = { param($h) [PSCustomObject]@{ showCmd = 1 } }
    $list3 = New-Object System.Collections.Generic.List[object]
    $r = Clear-Occluders -HostPid $hostPid -TargetHwnd $target -Points @(, @(5, 5)) -Minimized $list3 -HitTest $stuckHit -Minimize $stuckMin -SettleMs 0 -MaxPerPoint 3 -ShellPid $shellPid
    Check '最小化無效 → Ok=False' ($r.Ok -eq $false) "reason=$($r.Reason)"
    Check '同一扇視窗只記一次（還原不重複）' ($list3.Count -eq 1)

    # ── fix F8b：遮擋結果 → 驗收判讀（Get-OccluderVerdict／Assert-OccluderResult）──────────────────
    # 按下點由小工具自己的實際矩形推出：命中桌面或沒命中任何視窗＝小工具不在預期位置或沉到桌面之下（產品
    # 回歸）→ FAIL；工作列與系統 UI → ENV-BLOCKED；被腳本自己的視窗蓋住＝配置錯誤 → FAIL。
    function Get-ClearResult($w, [int[]]$own = @()) {
        $hitW = { param($x, $y) $w }.GetNewClosure()
        $lst = New-Object System.Collections.Generic.List[object]
        Clear-Occluders -HostPid $hostPid -TargetHwnd $target -Points @(, @(30, 40)) -Minimized $lst -HitTest $hitW `
            -Minimize { param($h) throw '不該最小化' } -SettleMs 0 -ShellPid $shellPid -OwnPids $own
    }
    function Get-AssertOutcome($res) {
        try { Assert-OccluderResult -Result $res -What '按下點'; return 'ok' } catch { return "$($_.Exception.Message)" }
    }
    $verdictCases = [ordered]@{
        '命中桌面 Progman（小工具沉到桌面之下）'   = @{ W = (New-Win -Hwnd ([IntPtr]0x300) -Pid_ $shellPid -Class 'Progman' -ProcessName 'explorer'); Own = @(); Want = 'fail' }
        '命中桌面 WorkerW（小工具沉到桌面之下）'   = @{ W = (New-Win -Hwnd ([IntPtr]0x301) -Pid_ $shellPid -Class 'WorkerW' -ProcessName 'explorer'); Own = @(); Want = 'fail' }
        '沒命中任何視窗（小工具不在預期位置）'     = @{ W = (New-Win -Hwnd ([IntPtr]::Zero) -Pid_ 0 -Class ''); Own = @(); Want = 'fail' }
        '工作列 Shell_TrayWnd'                     = @{ W = (New-Win -Hwnd ([IntPtr]0x302) -Pid_ $shellPid -Class 'Shell_TrayWnd' -ProcessName 'explorer'); Own = @(); Want = 'env-blocked' }
        '系統 UI Shell_SystemDim'                  = @{ W = (New-Win -Hwnd ([IntPtr]0x303) -Class 'Shell_SystemDim' -ProcessName 'explorer'); Own = @(); Want = 'env-blocked' }
        '系統 UI 行程 PickerHost'                  = @{ W = (New-Win -Hwnd ([IntPtr]0x304) -Class 'Shell_SystemDialog' -ProcessName 'PickerHost'); Own = @(); Want = 'env-blocked' }
        '使用者視窗無回應（不碰它）'               = @{ W = (New-Win -Hwnd ([IntPtr]0x307) -Hung $true); Own = @(); Want = 'env-blocked' }
        '腳本自己的表單'                           = @{ W = (New-Win -Hwnd ([IntPtr]0x305) -Pid_ 77 -Class 'WindowsForms10'); Own = @(77); Want = 'fail' }
        '宿主自己的另一扇視窗（小工具重疊或 z-order 錯）' = @{ W = (New-Win -Hwnd ([IntPtr]0x306) -Pid_ $hostPid -Class 'Tauri Window'); Own = @(); Want = 'fail' }
    }
    foreach ($k in $verdictCases.Keys) {
        $c = $verdictCases[$k]
        $res = Get-ClearResult $c.W $c.Own
        $v = Get-OccluderVerdict -Result $res
        Check "判讀：$k → $($c.Want)" ($res.Ok -eq $false -and $v.Verdict -eq $c.Want) "verdict=$($v.Verdict) action=$($res.PSObject.Properties['Action'] | ForEach-Object Value) reason=$($res.Reason)"
        $msg = Get-AssertOutcome $res
        if ($c.Want -eq 'fail') {
            Check "Assert：$k → 一般例外（FAIL，不是 ENV-BLOCKED／BLOCKED）" ($msg -ne 'ok' -and $msg -notlike 'ENV-BLOCKED*' -and $msg -notlike 'BLOCKED*') $msg
        } else {
            Check "Assert：$k → ENV-BLOCKED 例外" ($msg -like 'ENV-BLOCKED: *') $msg
        }
        Check "Assert：$k → 訊息含命中點與原因" ($msg -match '30,40') $msg
    }
    $deskMsg = Get-AssertOutcome (Get-ClearResult $verdictCases['命中桌面 Progman（小工具沉到桌面之下）'].W)
    Check 'Assert：命中桌面 → 訊息寫明小工具不在預期位置或沉到桌面之下' ($deskMsg -match '沉到桌面之下') $deskMsg
    Check 'Assert：Ok=True → 不丟例外' ((Get-AssertOutcome ([PSCustomObject]@{ Ok = $true; Reason = '' })) -eq 'ok')
    Check '判讀：最小化後仍蓋著（使用者視窗不肯最小化）→ env-blocked' ((Get-OccluderVerdict -Result $r).Verdict -eq 'env-blocked') "reason=$($r.Reason)"
    $legacy = [PSCustomObject]@{ Ok = $false; Reason = '(1,2) 被某視窗蓋住' }
    Check '判讀：結果缺 Action（無法證明是環境）→ fail，不遮成環境' ((Get-OccluderVerdict -Result $legacy).Verdict -eq 'fail')
    Check '判讀：結果為 $null → fail' ((Get-OccluderVerdict -Result $null).Verdict -eq 'fail')

    # ── fix F9：目標本來就是桌面（-TargetIsDesktop，verify-6.1-icon-click 的圖示點、hittest 的點穿取樣點）──
    # 命中桌面（Progman／WorkerW，不論是不是 -TargetHwnd 那一扇）＝到達目標；其餘規則不變：宿主視窗、沒命中、
    # 腳本自己的視窗＝FAIL，工作列、系統 UI、最小化後仍蓋著＝ENV-BLOCKED。
    function Get-DesktopAssertOutcome($res) {
        try { Assert-OccluderResult -Result $res -What '桌面點' -TargetIsDesktop; return 'ok' } catch { return "$($_.Exception.Message)" }
    }
    $deskCases = [ordered]@{
        '命中桌面 Progman'          = @{ Key = '命中桌面 Progman（小工具沉到桌面之下）'; Want = 'ok' }
        '命中桌面 WorkerW'          = @{ Key = '命中桌面 WorkerW（小工具沉到桌面之下）'; Want = 'ok' }
        '沒命中任何視窗'            = @{ Key = '沒命中任何視窗（小工具不在預期位置）'; Want = 'fail' }
        '工作列 Shell_TrayWnd'      = @{ Key = '工作列 Shell_TrayWnd'; Want = 'env-blocked' }
        '系統 UI Shell_SystemDim'   = @{ Key = '系統 UI Shell_SystemDim'; Want = 'env-blocked' }
        '系統 UI 行程 PickerHost'   = @{ Key = '系統 UI 行程 PickerHost'; Want = 'env-blocked' }
        '腳本自己的表單'            = @{ Key = '腳本自己的表單'; Want = 'fail' }
        '宿主視窗蓋住應落在桌面的點' = @{ Key = '宿主自己的另一扇視窗（小工具重疊或 z-order 錯）'; Want = 'fail' }
    }
    foreach ($k in $deskCases.Keys) {
        $c = $verdictCases[$deskCases[$k].Key]
        $want = $deskCases[$k].Want
        $res = Get-ClearResult $c.W $c.Own
        $v = Get-OccluderVerdict -Result $res -TargetIsDesktop
        Check "桌面目標判讀：$k → $want" ($v.Verdict -eq $want) "verdict=$($v.Verdict) msg=$($v.Message)"
        $msg = Get-DesktopAssertOutcome $res
        switch ($want) {
            'ok' { Check "桌面目標 Assert：$k → 不丟例外" ($msg -eq 'ok') $msg }
            'fail' { Check "桌面目標 Assert：$k → FAIL: 例外（寫明沒有落在桌面上）" ($msg -like 'FAIL: *' -and $msg -match '桌面' -and $msg -match '30,40') $msg }
            default { Check "桌面目標 Assert：$k → ENV-BLOCKED: 例外" ($msg -like 'ENV-BLOCKED: *' -and $msg -match '30,40') $msg }
        }
    }
    Check '桌面目標判讀：最小化後仍蓋著 → env-blocked' ((Get-OccluderVerdict -Result $r -TargetIsDesktop).Verdict -eq 'env-blocked')
    Check '桌面目標判讀：結果缺 Action → fail' ((Get-OccluderVerdict -Result $legacy -TargetIsDesktop).Verdict -eq 'fail')
    Check '桌面目標判讀：結果為 $null → fail' ((Get-OccluderVerdict -Result $null -TargetIsDesktop).Verdict -eq 'fail')
    Check '桌面目標判讀：Ok=True → ok' ((Get-OccluderVerdict -Result ([PSCustomObject]@{ Ok = $true; Reason = '' }) -TargetIsDesktop).Verdict -eq 'ok')
    Check '不帶 -TargetIsDesktop 時命中桌面仍是 fail（小工具目標的規則不變）' ((Get-OccluderVerdict -Result (Get-ClearResult $verdictCases['命中桌面 Progman（小工具沉到桌面之下）'].W)).Verdict -eq 'fail')

    # ── Restore-Occluders：反向還原 ────────────────────────────────────────────
    $restored = New-Object System.Collections.Generic.List[IntPtr]
    $rest = { param($h, $p) $restored.Add($h); $true }
    $log = New-Object System.Collections.Generic.List[string]
    Restore-Occluders -Minimized $list -Restore $rest -Log { param($m) $log.Add($m) }
    Check '反向還原（後最小化者先還原）' ($restored.Count -eq 2 -and $restored[0] -eq [IntPtr]0x600 -and $restored[1] -eq [IntPtr]0x500)
    Check '還原後清單清空（重複呼叫不重複還原）' ($list.Count -eq 0)
    Check '每筆還原都有記錄' ($log.Count -eq 2)

    # ── fix F7b／hung-target：最小化快照（placement＋實際矩形＋吸附狀態）與還原（可注入的 Win32 後端 mock）──
    # 還原只用非同步呼叫（ShowWindowAsync、SetWindowPos＋SWP_ASYNCWINDOWPOS），再以「輪詢讀回、有逾時」確認；
    # 不用 SetWindowPlacement（跨行程同步呼叫：目標 UI 執行緒卡住時呼叫端無限等待）。
    # mock 世界：每扇視窗有 Placement（showCmd、Normal）、Rect（GetWindowRect）、Arranged、Zoomed、Minimized；
    # 可選 RestoreRect（SW_SHOWNOACTIVATE 還原後的矩形）、Hung（IsHungAppWindow）、Unresponsive（非同步請求不生效）。
    # mock 的 IsZoomed 照系統行為：最小化中回 False。SW_SHOWNOACTIVATE 還原「最小化前的狀態」（最大化者回最大化），
    # 除非 RestoreMaxLost（模擬沒有回到最大化）。
    # 時序（review 642050f）：Busy＝目標忙碌但還沒被判 hung——Minimize／ShowAsync 只排進 Queue，等測試呼叫
    # $be.Drain 才依序生效；Gone＝視窗已銷毀（IsWindow=False）；ClampRect＝SetWindowPos 後系統給的矩形不是要求的。
    function New-Rect($l, $t, $r, $b) { [PSCustomObject]@{ Left = $l; Top = $t; Right = $r; Bottom = $b } }
    function New-Backend {
        param([hashtable]$World, [System.Collections.Generic.List[string]]$Calls, [switch]$NoArrangedApi)
        $maxRect = New-Rect 0 0 2560 1400
        $be = @{
            GetPlacement = { param($h) $Calls.Add("GetPlacement"); $World[$h].Placement }.GetNewClosure()
            GetRect      = { param($h) $Calls.Add("GetRect"); $World[$h].Rect }.GetNewClosure()
            IsZoomed     = { param($h) (-not $World[$h].Minimized) -and [bool]$World[$h].Zoomed }.GetNewClosure()
            IsIconic     = { param($h) [bool]$World[$h].Minimized }.GetNewClosure()
            IsHung       = { param($h) $w = $World[$h]; [bool]($w.PSObject.Properties['Hung'] -and $w.Hung) }.GetNewClosure()
            IsWindow     = { param($h) $w = $World[$h]; -not [bool]($w.PSObject.Properties['Gone'] -and $w.Gone) }.GetNewClosure()
            Sleep        = { param($ms) $Calls.Add("Sleep") }.GetNewClosure()
        }
        # 目標執行緒處理一個顯示狀態請求（min／show:N）。
        $apply = {
            param($w, $op)
            if ($op -eq 'min') { $w.Minimized = $true; return }
            $w.Minimized = $false
            if ($op -eq 'show:3') { $w.Rect = $maxRect; $w.Zoomed = $true; return }
            if ($w.PSObject.Properties['RestoreMaxLost'] -and $w.RestoreMaxLost) { $w.Zoomed = $false }
            if ($w.PSObject.Properties['RestoreRect'] -and $null -ne $w.RestoreRect) { $w.Rect = $w.RestoreRect }
        }.GetNewClosure()
        $enqueue = {
            param($h, $op)
            $w = $World[$h]
            if ($w.PSObject.Properties['Unresponsive'] -and $w.Unresponsive) { return }
            if ($w.PSObject.Properties['Busy'] -and $w.Busy) {
                if (-not $w.PSObject.Properties['Queue']) { $w | Add-Member Queue (New-Object System.Collections.Generic.List[string]) }
                $w.Queue.Add($op); return
            }
            & $apply $w $op
        }.GetNewClosure()
        $be.Minimize = { param($h) $Calls.Add("Minimize"); & $enqueue $h 'min' }.GetNewClosure()
        $be.ShowAsync = { param($h, $cmd) $Calls.Add("ShowAsync:$cmd"); & $enqueue $h "show:$cmd" }.GetNewClosure()
        $be.Drain = {
            param($h)
            $w = $World[$h]; $w.Busy = $false
            if ($w.PSObject.Properties['Queue']) { foreach ($op in @($w.Queue)) { & $apply $w $op }; $w.Queue.Clear() }
        }.GetNewClosure()
        $be.SetWindowPos = {
            param($h, $x, $y, $cx, $cy, $flags)
            $Calls.Add("SetWindowPos:$x,$y,$cx,$cy,flags=0x$('{0:X}' -f $flags)")
            $w = $World[$h]
            if ($w.PSObject.Properties['Unresponsive'] -and $w.Unresponsive) { return $true }
            if ($w.PSObject.Properties['ClampRect'] -and $null -ne $w.ClampRect) { $w.Rect = $w.ClampRect; return $true }
            # GetNewClosure 的動態模組看不到本腳本的函式，故不呼叫 New-Rect。
            $w.Rect = [PSCustomObject]@{ Left = $x; Top = $y; Right = $x + $cx; Bottom = $y + $cy }
            $true
        }.GetNewClosure()
        if ($NoArrangedApi) { $be.IsArranged = { param($h) throw [System.EntryPointNotFoundException]::new('IsWindowArranged') } }
        else { $be.IsArranged = { param($h) $Calls.Add("IsArranged"); $World[$h].Arranged }.GetNewClosure() }
        $be
    }
    function New-WorldWin {
        param($Placement, $Rect, [bool]$Arranged = $false, [bool]$Zoomed = $false, $RestoreRect = $null)
        [PSCustomObject]@{ Placement = $Placement; Rect = $Rect; Arranged = $Arranged; Zoomed = $Zoomed; Minimized = $false; RestoreRect = $RestoreRect }
    }
    $asyncFlags = 'flags=0x4214'   # SWP_ASYNCWINDOWPOS|SWP_NOOWNERZORDER|SWP_NOACTIVATE|SWP_NOZORDER
    $hw = [IntPtr]0xA00
    $snapRect = New-Rect 1280 0 2560 1400     # 吸附在右半邊的實際矩形
    $preSnap = New-Rect 300 200 1100 900      # 吸附前的還原矩形（rcNormalPosition）

    # 0. 靜態：Win32 後端不得有同步的跨行程視窗呼叫（ShowWindow、SetWindowPlacement、不帶 SWP_ASYNCWINDOWPOS 的 SetWindowPos）。
    $modTokens = $null; $modErr = $null
    [void][System.Management.Automation.Language.Parser]::ParseFile($modPath, [ref]$modTokens, [ref]$modErr)
    $modCode = (@($modTokens | Where-Object { $_.Kind -ne 'Comment' } | ForEach-Object { $_.Text }) -join ' ')
    Check 'Occluders.psm1 程式碼不含 SetWindowPlacement（同步、目標卡住會無限等待）' ($modCode -notmatch 'SetWindowPlacement')
    Check 'Occluders.psm1 程式碼不含同步 ShowWindow（只准 ShowWindowAsync）' ($modCode -notmatch '\bShowWindow\s*\(' -and $modCode -notmatch '::ShowWindow\b(?!Async)')
    Check 'Occluders.psm1 宣告並使用 IsHungAppWindow' ($modCode -match 'IsHungAppWindow')
    $swpFlags = & $occMod { $script:SwpRestoreFlags }
    Check '還原用的 SetWindowPos 旗標帶 SWP_ASYNCWINDOWPOS（0x4000）' (($swpFlags -band 0x4000) -ne 0) ('0x{0:X}' -f $swpFlags)
    $win32Be = & $occMod { $script:Win32Backend }
    Check 'Win32 後端沒有 SetPlacement 鍵' (-not $win32Be.ContainsKey('SetPlacement'))
    Check 'Win32 後端有 IsHung／ShowAsync／Sleep 鍵' ($win32Be.ContainsKey('IsHung') -and $win32Be.ContainsKey('ShowAsync') -and $win32Be.ContainsKey('Sleep'))
    foreach ($k in 'ShowAsync', 'SetWindowPos', 'Minimize') {
        $body = if ($win32Be.ContainsKey($k)) { $win32Be[$k].ToString() } else { '' }
        Check "Win32 後端 $k 不呼叫同步 ShowWindow／SetWindowPlacement" ($body -ne '' -and $body -notmatch '::ShowWindow\(' -and $body -notmatch 'SetWindowPlacement') $body
    }
    # lib 下其他模組：不得 P/Invoke 同步的 ShowWindow／SetWindowPlacement；SetWindowPos 只准 ScratchWindow（自己建的表單）
    # 與 Occluders（已斷言帶 SWP_ASYNCWINDOWPOS）。
    foreach ($lib in Get-ChildItem (Join-Path $toolsDir 'lib') -Filter *.psm1) {
        $tk = $null; $er = $null
        [void][System.Management.Automation.Language.Parser]::ParseFile($lib.FullName, [ref]$tk, [ref]$er)
        $code = (@($tk | Where-Object { $_.Kind -ne 'Comment' } | ForEach-Object { $_.Text }) -join ' ')
        Check "lib/$($lib.Name)：不宣告同步 ShowWindow／SetWindowPlacement" ($code -notmatch 'extern\s+bool\s+(ShowWindow|SetWindowPlacement)\s*\(')
        if ($lib.Name -notin 'Occluders.psm1', 'ScratchWindow.psm1') {
            Check "lib/$($lib.Name)：不呼叫 SetWindowPos（他人視窗一律經 Occluders）" ($code -notmatch '\bSetWindowPos\b')
        }
    }

    # 1. 吸附中的視窗：快照記下實際矩形與 Arranged；還原時 ShowWindowAsync(SW_SHOWNOACTIVATE) → 非同步 SetWindowPos 回吸附矩形。
    $calls = New-Object System.Collections.Generic.List[string]
    $world = @{ $hw = New-WorldWin -Placement ([PSCustomObject]@{ showCmd = 1; Normal = $preSnap }) -Rect $snapRect -Arranged $true -RestoreRect $preSnap }
    $be = New-Backend -World $world -Calls $calls
    $snap = Invoke-ModuleMinimize -Hwnd $hw -Backend $be
    Check '快照：記下 GetWindowPlacement' ($snap.Placement.showCmd -eq 1)
    Check '快照：記下 GetWindowRect（吸附後的實際矩形）' ($snap.Rect.Left -eq 1280 -and $snap.Rect.Right -eq 2560)
    Check '快照：記下 IsWindowArranged=True' ($snap.Arranged -eq $true)
    Check '快照：先讀狀態、最後才最小化' ($calls.IndexOf('Minimize') -eq ($calls.Count - 1) -and $calls.Contains('GetRect') -and $calls.Contains('IsArranged'))
    $calls.Clear()
    $res = Invoke-ModuleRestore -Hwnd $hw -Snapshot $snap -Backend $be
    Check '吸附：先 ShowWindowAsync(SW_SHOWNOACTIVATE)' ($calls -contains 'ShowAsync:4') ($calls -join ' | ')
    Check '吸附：再非同步 SetWindowPos 回吸附矩形（1280,0,1280x1400）' ($calls -contains "SetWindowPos:1280,0,1280,1400,$asyncFlags") ($calls -join ' | ')
    Check '吸附：還原後矩形＝最小化前矩形' ($world[$hw].Rect.Left -eq 1280 -and $world[$hw].Rect.Bottom -eq 1400)
    Check '吸附：回報 Ok 且矩形相符' ($res.Ok -eq $true -and $res.RectMatches -eq $true -and $res.Repositioned -eq $true) "$($res.Detail)"

    # 2. 一般視窗、還原後矩形已相符 → 不呼叫 SetWindowPos。
    $world[$hw] = New-WorldWin -Placement ([PSCustomObject]@{ showCmd = 1; Normal = $preSnap }) -Rect $preSnap
    $snap = Invoke-ModuleMinimize -Hwnd $hw -Backend $be
    $calls.Clear()
    $res = Invoke-ModuleRestore -Hwnd $hw -Snapshot $snap -Backend $be
    Check '一般視窗：只 ShowWindowAsync、不 SetWindowPos' ($calls -contains 'ShowAsync:4' -and -not ($calls -match '^SetWindowPos')) ($calls -join ' | ')
    Check '一般視窗：Ok、Repositioned=False' ($res.Ok -eq $true -and $res.Repositioned -eq $false -and $res.RectMatches -eq $true) "$($res.Detail)"

    # 3. 未吸附但還原後矩形不同（rcNormalPosition 殘留）→ 非同步 SetWindowPos 回原矩形。
    $world[$hw] = New-WorldWin -Placement ([PSCustomObject]@{ showCmd = 1; Normal = $preSnap }) -Rect (New-Rect 10 20 810 620) -RestoreRect $preSnap
    $snap = Invoke-ModuleMinimize -Hwnd $hw -Backend $be
    $calls.Clear()
    $res = Invoke-ModuleRestore -Hwnd $hw -Snapshot $snap -Backend $be
    Check '矩形不符：非同步 SetWindowPos 回原矩形（不啟用、不改 z-order）' ($calls -contains "SetWindowPos:10,20,800,600,$asyncFlags" -and $res.Ok -eq $true) ($calls -join ' | ')

    # 3b. 原本吸附、但還原後矩形剛好相符 → 仍 SetWindowPos（吸附狀態一律補正）。
    $world[$hw] = New-WorldWin -Placement ([PSCustomObject]@{ showCmd = 1; Normal = $snapRect }) -Rect $snapRect -Arranged $true
    $snap = Invoke-ModuleMinimize -Hwnd $hw -Backend $be
    $calls.Clear()
    $res = Invoke-ModuleRestore -Hwnd $hw -Snapshot $snap -Backend $be
    Check '吸附且矩形已相符：仍 SetWindowPos' ($calls -contains "SetWindowPos:1280,0,1280,1400,$asyncFlags" -and $res.Repositioned -eq $true) ($calls -join ' | ')

    # 4. 原本最大化 → 還原後必須是最大化，不 SetWindowPos（最大化視窗的 GetWindowRect 含超出螢幕的邊框，
    #    與還原後讀到的矩形不必相等，也不得因此被 SetWindowPos 拉成一般視窗）。
    #    待實測：「最小化前是最大化的視窗，SW_SHOWNOACTIVATE 會直接回到最大化」只是依文件推論、未在實機驗證；
    #    故兩種行為都接受——直接回到最大化（不補送），或沒回到時補 ShowWindowAsync(SW_SHOWMAXIMIZED)（見 4b）。
    $world[$hw] = New-WorldWin -Placement ([PSCustomObject]@{ showCmd = 3; Normal = $preSnap }) -Rect (New-Rect -8 -8 2568 1408) -Zoomed $true
    $snap = Invoke-ModuleMinimize -Hwnd $hw -Backend $be
    Check '最大化：快照 Maximized=True' ($snap.Maximized -eq $true)
    $calls.Clear()
    $res = Invoke-ModuleRestore -Hwnd $hw -Snapshot $snap -Backend $be
    Check '最大化：先 ShowWindowAsync(SW_SHOWNOACTIVATE)、不 SetWindowPos（是否補 SW_SHOWMAXIMIZED 兩種都接受）' ($calls -contains 'ShowAsync:4' -and -not ($calls -match '^SetWindowPos')) ($calls -join ' | ')
    Check '最大化：還原後仍最大化、Ok' ($world[$hw].Zoomed -eq $true -and -not $world[$hw].Minimized -and $res.Ok -eq $true) "$($res.Detail)"
    # 4b. 還原後沒有回到最大化 → 補 ShowWindowAsync(SW_SHOWMAXIMIZED)，輪詢讀回 IsZoomed。
    $world[$hw] = New-WorldWin -Placement ([PSCustomObject]@{ showCmd = 3; Normal = $preSnap }) -Rect (New-Rect -8 -8 2568 1408) -Zoomed $true -RestoreRect $preSnap
    $world[$hw] | Add-Member RestoreMaxLost $true
    $snap = Invoke-ModuleMinimize -Hwnd $hw -Backend $be
    $calls.Clear()
    $res = Invoke-ModuleRestore -Hwnd $hw -Snapshot $snap -Backend $be
    Check '最大化未恢復：補 ShowWindowAsync(SW_SHOWMAXIMIZED)、不 SetWindowPos' ($calls -contains 'ShowAsync:3' -and -not ($calls -match '^SetWindowPos')) ($calls -join ' | ')
    Check '最大化未恢復：補完後最大化、Ok' ($world[$hw].Zoomed -eq $true -and $res.Ok -eq $true) "$($res.Detail)"

    # 5. IsWindowArranged 不可用（舊版 Windows）→ Arranged=$null，仍以矩形比對補正。
    $calls2 = New-Object System.Collections.Generic.List[string]
    $world2 = @{ $hw = New-WorldWin -Placement ([PSCustomObject]@{ showCmd = 1; Normal = $preSnap }) -Rect $snapRect -Arranged $true -RestoreRect $preSnap }
    $be2 = New-Backend -World $world2 -Calls $calls2 -NoArrangedApi
    $snap = Invoke-ModuleMinimize -Hwnd $hw -Backend $be2
    Check 'IsWindowArranged 不可用：Arranged=$null、仍最小化' ($null -eq $snap.Arranged -and $calls2.Contains('Minimize'))
    $res = Invoke-ModuleRestore -Hwnd $hw -Snapshot $snap -Backend $be2
    Check 'IsWindowArranged 不可用：矩形不符仍 SetWindowPos 補正' ($calls2 -contains "SetWindowPos:1280,0,1280,1400,$asyncFlags" -and $res.RectMatches -eq $true) ($calls2 -join ' | ')

    # 5b. 快照的 GetWindowPlacement 讀取失敗（Placement=$null）→ 仍以 ShowWindowAsync 還原、照矩形補正。
    $calls3 = New-Object System.Collections.Generic.List[string]
    $world3 = @{ $hw = New-WorldWin -Placement $null -Rect (New-Rect 100 100 900 700) }
    $be3 = New-Backend -World $world3 -Calls $calls3
    $snapB = Invoke-ModuleMinimize -Hwnd $hw -Backend $be3
    Check 'placement 讀不到：快照 Placement=$null、Iconic=False、仍最小化' ($null -eq $snapB.Placement -and $snapB.Iconic -eq $false -and $world3[$hw].Minimized)
    $calls3.Clear()
    $res = Invoke-ModuleRestore -Hwnd $hw -Snapshot $snapB -Backend $be3
    Check 'placement 讀不到：ShowWindowAsync 救回、回報 Ok' ($calls3 -contains 'ShowAsync:4' -and -not $world3[$hw].Minimized -and $res.Ok -eq $true) "$($calls3 -join ' | ') $($res.Detail)"
    # 快照時本來就是最小化（Iconic=True）→ 不還原（維持使用者原狀）、不送任何顯示呼叫。
    $calls3.Clear()
    $world3[$hw].Minimized = $true
    $res = Invoke-ModuleRestore -Hwnd $hw -Snapshot ([PSCustomObject]@{ Placement = $null; Rect = $preSnap; Arranged = $false; Maximized = $false; Iconic = $true }) -Backend $be3
    Check '快照時本來就最小化：不送 ShowWindowAsync／SetWindowPos、維持最小化' (-not ($calls3 -match '^(ShowAsync|SetWindowPos)') -and $world3[$hw].Minimized -and $res.Ok -eq $true) ($calls3 -join ' | ')

    # 5c. hung-target：還原時目標無回應（IsHungAppWindow）→ 不送任何還原呼叫、Ok=False、Hung=True、寫明未還原。
    $calls4 = New-Object System.Collections.Generic.List[string]
    $world4 = @{ $hw = New-WorldWin -Placement ([PSCustomObject]@{ showCmd = 1; Normal = $preSnap }) -Rect $preSnap }
    $be4 = New-Backend -World $world4 -Calls $calls4
    $snapH = Invoke-ModuleMinimize -Hwnd $hw -Backend $be4
    $world4[$hw] | Add-Member Hung $true
    $calls4.Clear()
    $res = Invoke-ModuleRestore -Hwnd $hw -Snapshot $snapH -Backend $be4
    Check '無回應：不送 ShowWindowAsync／SetWindowPos' (-not ($calls4 -match '^(ShowAsync|SetWindowPos)')) ($calls4 -join ' | ')
    Check '無回應：Ok=False、Hung=True、細節寫明未還原（目標無回應）' ($res.Ok -eq $false -and $res.Hung -eq $true -and $res.Detail -match '未還原（目標無回應）') "$($res.Detail)"
    # 5d. 非同步請求遲遲不生效（目標沒處理訊息、但還沒被判為 hung）→ 輪詢有上限、回報逾時，不無限等待。
    $world4[$hw] = New-WorldWin -Placement ([PSCustomObject]@{ showCmd = 1; Normal = $preSnap }) -Rect $preSnap
    $snapU = Invoke-ModuleMinimize -Hwnd $hw -Backend $be4
    $world4[$hw] | Add-Member Unresponsive $true
    $calls4.Clear()
    $res = & $occMod { param($h, $s, $b) Invoke-RestoreWindow -Hwnd $h -Snapshot $s -Backend $b -TimeoutMs 1000 -PollMs 100 } $hw $snapU $be4
    $sleeps = @($calls4 | Where-Object { $_ -eq 'Sleep' }).Count
    Check '請求不生效：Ok=False、TimedOut=True' ($res.Ok -eq $false -and $res.TimedOut -eq $true) "$($res.Detail)"
    Check '請求不生效：輪詢次數有上限（1000/100＝10 次以內）' ($sleeps -ge 1 -and $sleeps -le 10) "sleeps=$sleeps"

    # 5f. review 642050f：最小化只排進佇列、還沒生效就被還原（目標忙碌但未滿 5 秒、IsHung=False）。
    #     舊行為：IsIconic=False → 回 Ok、什麼都不送；目標恢復後排隊的最小化才執行，視窗停在最小化、記錄卻寫成功。
    #     現行：最小化後輪詢 IsIconic（有逾時）記 MinimizeConfirmed；未確認者還原時照順序補送一次非同步還原，
    #     並判「未確認」列進 NotRestored。
    $calls5 = New-Object System.Collections.Generic.List[string]
    $world5 = @{ $hw = New-WorldWin -Placement ([PSCustomObject]@{ showCmd = 1; Normal = $preSnap }) -Rect $preSnap }
    $world5[$hw] | Add-Member Busy $true
    $be5 = New-Backend -World $world5 -Calls $calls5
    $snapQ = & $occMod { param($h, $b) Invoke-MinimizeWindow -Hwnd $h -Backend $b -TimeoutMs 500 -PollMs 100 } $hw $be5
    $minSleeps = @($calls5 | Where-Object { $_ -eq 'Sleep' }).Count
    Check '最小化未生效：快照 MinimizeConfirmed=False' ([bool]($snapQ.PSObject.Properties['MinimizeConfirmed'] -and $snapQ.MinimizeConfirmed -eq $false))
    Check '最小化未生效：輪詢有上限（500/100＝5 次以內）' ($minSleeps -ge 1 -and $minSleeps -le 5) "sleeps=$minSleeps"
    $calls5.Clear()
    $res = Invoke-ModuleRestore -Hwnd $hw -Snapshot $snapQ -Backend $be5
    Check '最小化未生效：還原時仍補送非同步還原（排在最小化之後）' ($calls5 -contains 'ShowAsync:4') ($calls5 -join ' | ')
    Check '最小化未生效：Ok=False、Unconfirmed=True（不得記成功）' ([bool]($res.Ok -eq $false -and $res.PSObject.Properties['Unconfirmed'] -and $res.Unconfirmed -eq $true)) "$($res.Detail)"
    & $be5.Drain $hw
    Check '最小化未生效：目標恢復、佇列依序處理後視窗不停在最小化' (-not $world5[$hw].Minimized) "minimized=$($world5[$hw].Minimized)"
    $snapQ2 = [PSCustomObject]@{ Placement = $snapQ.Placement; Rect = $snapQ.Rect; Arranged = $false; Maximized = $false; Iconic = $false; MinimizeConfirmed = $false }
    $listQ = New-Object System.Collections.Generic.List[object]
    $listQ.Add([PSCustomObject]@{ Hwnd = $hw; Snapshot = $snapQ2; Cls = 'Chrome_WidgetWin_1' })
    $nrQ = New-Object System.Collections.Generic.List[string]
    $logQ = New-Object System.Collections.Generic.List[string]
    $world5[$hw] = New-WorldWin -Placement ([PSCustomObject]@{ showCmd = 1; Normal = $preSnap }) -Rect $preSnap
    Restore-Occluders -Minimized $listQ -NotRestored $nrQ -Log { param($m) $logQ.Add($m) } -Restore { param($h, $s) Invoke-ModuleRestore -Hwnd $h -Snapshot $s -Backend $be5 }
    Check '最小化未生效：列進 NotRestored，原因寫明最小化未確認' ($nrQ.Count -eq 1 -and $nrQ[0] -match '最小化未確認') ($nrQ -join ' | ')
    # 最小化正常生效 → MinimizeConfirmed=True。
    $world5[$hw] = New-WorldWin -Placement ([PSCustomObject]@{ showCmd = 1; Normal = $preSnap }) -Rect $preSnap
    $snapOk = Invoke-ModuleMinimize -Hwnd $hw -Backend $be5
    Check '最小化生效：MinimizeConfirmed=True' ([bool]($snapOk.PSObject.Properties['MinimizeConfirmed'] -and $snapOk.MinimizeConfirmed -eq $true))

    # 5f-2. review 31c7c53：最小化在確認逾時之後、還原之前才生效（還原時 IsIconic 已是 True）→ 走一般還原路徑，
    #       吸附的視窗以 SetWindowPos 回原矩形，可正常確認，不再判 Unconfirmed。
    $calls8 = New-Object System.Collections.Generic.List[string]
    $world8 = @{ $hw = New-WorldWin -Placement ([PSCustomObject]@{ showCmd = 1; Normal = $preSnap }) -Rect $snapRect -Arranged $true -RestoreRect $preSnap }
    $world8[$hw] | Add-Member Busy $true
    $be8 = New-Backend -World $world8 -Calls $calls8
    $snapL = & $occMod { param($h, $b) Invoke-MinimizeWindow -Hwnd $h -Backend $b -TimeoutMs 300 -PollMs 100 } $hw $be8
    Check '最小化晚生效：快照 MinimizeConfirmed=False' ([bool]($snapL.PSObject.Properties['MinimizeConfirmed'] -and $snapL.MinimizeConfirmed -eq $false))
    & $be8.Drain $hw
    Check '最小化晚生效：還原前已是最小化（前提）' ([bool]$world8[$hw].Minimized)
    $calls8.Clear()
    $res = Invoke-ModuleRestore -Hwnd $hw -Snapshot $snapL -Backend $be8
    Check '最小化晚生效：走一般還原（ShowWindowAsync＋非同步 SetWindowPos 回吸附矩形）' ($calls8 -contains 'ShowAsync:4' -and $calls8 -contains "SetWindowPos:1280,0,1280,1400,$asyncFlags") ($calls8 -join ' | ')
    Check '最小化晚生效：Ok=True、Unconfirmed=False、矩形相符' ($res.Ok -eq $true -and $res.Unconfirmed -eq $false -and $res.RectMatches -eq $true -and -not $world8[$hw].Minimized) "$($res.Detail)"

    # 5g. 視窗已銷毀（IsWindow=False）→ 不送任何呼叫、不輪詢（不白等 3 秒）、Gone=True；Restore-Occluders 記「已不存在」。
    $calls6 = New-Object System.Collections.Generic.List[string]
    $world6 = @{ $hw = New-WorldWin -Placement ([PSCustomObject]@{ showCmd = 1; Normal = $preSnap }) -Rect $preSnap }
    $be6 = New-Backend -World $world6 -Calls $calls6
    $snapG = Invoke-ModuleMinimize -Hwnd $hw -Backend $be6
    $world6[$hw] | Add-Member Gone $true
    $calls6.Clear()
    $res = Invoke-ModuleRestore -Hwnd $hw -Snapshot $snapG -Backend $be6
    Check '已銷毀：不送 ShowWindowAsync／SetWindowPos、不輪詢' (-not ($calls6 -match '^(ShowAsync|SetWindowPos|Sleep)')) ($calls6 -join ' | ')
    Check '已銷毀：Gone=True、細節寫明已不存在' ([bool]($res.PSObject.Properties['Gone'] -and $res.Gone -eq $true -and $res.Detail -match '已不存在')) "$($res.Detail)"
    $listG = New-Object System.Collections.Generic.List[object]
    $listG.Add([PSCustomObject]@{ Hwnd = $hw; Snapshot = $snapG; Cls = 'Chrome_WidgetWin_1' })
    $nrG = New-Object System.Collections.Generic.List[string]
    $logG = New-Object System.Collections.Generic.List[string]
    Restore-Occluders -Minimized $listG -NotRestored $nrG -Log { param($m) $logG.Add($m) } -Restore { param($h, $s) Invoke-ModuleRestore -Hwnd $h -Snapshot $s -Backend $be6 }
    Check '已銷毀：記錄寫「已不存在」、不列進 NotRestored（沒有留在最小化的視窗）' ($nrG.Count -eq 0 -and @($logG | Where-Object { $_ -match '已不存在' }).Count -ge 1) "nr=$($nrG -join ' | ') log=$($logG -join ' | ')"

    # 5h. 矩形不符：SetWindowPos 後矩形有變但不等於原矩形（系統夾限）→ 記「讀回不符」，不是「逾時」。
    $calls7 = New-Object System.Collections.Generic.List[string]
    $world7 = @{ $hw = New-WorldWin -Placement ([PSCustomObject]@{ showCmd = 1; Normal = $preSnap }) -Rect (New-Rect 10 20 810 620) -RestoreRect $preSnap }
    $be7 = New-Backend -World $world7 -Calls $calls7
    $snapM = Invoke-ModuleMinimize -Hwnd $hw -Backend $be7
    $world7[$hw] | Add-Member ClampRect (New-Rect 10 20 700 500)
    $res = & $occMod { param($h, $s, $b) Invoke-RestoreWindow -Hwnd $h -Snapshot $s -Backend $b -TimeoutMs 300 -PollMs 100 } $hw $snapM $be7
    Check '矩形不符：Ok=False、Mismatch=True、TimedOut=False' ([bool]($res.Ok -eq $false -and $res.PSObject.Properties['Mismatch'] -and $res.Mismatch -eq $true -and $res.TimedOut -eq $false)) "$($res.Detail)"
    $listM = New-Object System.Collections.Generic.List[object]
    $listM.Add([PSCustomObject]@{ Hwnd = $hw; Snapshot = $snapM; Cls = 'Chrome_WidgetWin_1' })
    $nrM = New-Object System.Collections.Generic.List[string]
    Restore-Occluders -Minimized $listM -NotRestored $nrM -Restore { param($h, $s) [PSCustomObject]@{ Ok = $false; Hung = $false; TimedOut = $false; Mismatch = $true; Detail = 'mock' } }
    Check '矩形不符：NotRestored 寫「讀回不符」' ($nrM.Count -eq 1 -and $nrM[0] -match '讀回不符' -and $nrM[0] -notmatch '逾時') ($nrM -join ' | ')
    # 真的逾時（矩形完全沒動）仍記「逾時」。
    $nrT = New-Object System.Collections.Generic.List[string]
    $listT = New-Object System.Collections.Generic.List[object]
    $listT.Add([PSCustomObject]@{ Hwnd = $hw; Snapshot = $snapM; Cls = 'Chrome_WidgetWin_1' })
    Restore-Occluders -Minimized $listT -NotRestored $nrT -Restore { param($h, $s) [PSCustomObject]@{ Ok = $false; Hung = $false; TimedOut = $true; Mismatch = $false; Detail = 'mock' } }
    Check '真的逾時：NotRestored 寫「逾時」' ($nrT.Count -eq 1 -and $nrT[0] -match '逾時') ($nrT -join ' | ')

    # 5e. Restore-Occluders：還原失敗／無回應的視窗列進 -NotRestored，記錄寫明「未還原」；成功的不列。
    $listN = New-Object System.Collections.Generic.List[object]
    $listN.Add([PSCustomObject]@{ Hwnd = [IntPtr]0xB01; Snapshot = $snapH; Cls = 'Chrome_WidgetWin_1' })
    $listN.Add([PSCustomObject]@{ Hwnd = [IntPtr]0xB02; Snapshot = $snapH; Cls = 'CASCADIA_HOSTING_WINDOW_CLASS' })
    $notRestored = New-Object System.Collections.Generic.List[string]
    $logN = New-Object System.Collections.Generic.List[string]
    Restore-Occluders -Minimized $listN -NotRestored $notRestored -Log { param($m) $logN.Add($m) } -Restore {
        param($h, $s)
        if ($h -eq [IntPtr]0xB02) { return [PSCustomObject]@{ Ok = $false; Hung = $true; TimedOut = $false; Detail = '未還原（目標無回應）' } }
        [PSCustomObject]@{ Ok = $true; Hung = $false; TimedOut = $false; Detail = 'mock' }
    }
    Check 'NotRestored：只列無回應那扇' ($notRestored.Count -eq 1 -and $notRestored[0] -match '0xB02' -and $notRestored[0] -match '目標無回應') ($notRestored -join ' | ')
    Check 'NotRestored：記錄寫明未還原' (@($logN | Where-Object { $_ -match '未還原' }).Count -ge 1) ($logN -join ' | ')
    Check 'NotRestored：清單照樣清空' ($listN.Count -eq 0)

    # 6. Clear-Occluders 記下的是完整快照、Restore-Occluders 原樣交給還原。
    $snapSeen = New-Object System.Collections.Generic.List[object]
    $listR = New-Object System.Collections.Generic.List[object]
    $listR.Add([PSCustomObject]@{ Hwnd = $hw; Snapshot = $snap; Cls = 'Chrome_WidgetWin_1' })
    $logR = New-Object System.Collections.Generic.List[string]
    Restore-Occluders -Minimized $listR -Restore { param($h, $s) $snapSeen.Add($s); [PSCustomObject]@{ Ok = $true; RectMatches = $true; Repositioned = $true; Detail = 'mock' } } -Log { param($m) $logR.Add($m) }
    Check 'Restore-Occluders 把快照交給還原' ($snapSeen.Count -eq 1 -and $snapSeen[0].Rect.Left -eq 1280)
    Check 'Restore-Occluders 記錄含 ok 與細節' ($logR.Count -eq 1 -and $logR[0] -match 'ok=True' -and $logR[0] -match 'mock') ($logR -join ' | ')

    # 7. fix F8：Invoke-MinimizeAppWindows（全螢幕截圖用）——只最小化白名單內的視窗，其餘跳過並記原因。
    $wins = @{
        [long]0xA01 = New-Win -Hwnd ([IntPtr]0xA01)
        [long]0xA02 = New-Win -Hwnd ([IntPtr]0xA02) -Class 'Shell_SystemDim' -ProcessName 'explorer'
        [long]0xA03 = New-Win -Hwnd ([IntPtr]0xA03) -Class 'Shell_SystemDialog' -ProcessName 'PickerHost' -ExStyle 0x00000108
        [long]0xA04 = New-Win -Hwnd ([IntPtr]0xA04) -Class 'CASCADIA_HOSTING_WINDOW_CLASS' -ProcessName 'WindowsTerminal'
        [long]0xA05 = New-Win -Hwnd ([IntPtr]0xA05) -Owner ([IntPtr]0xA01)
    }
    $minA = New-Object System.Collections.Generic.List[IntPtr]
    $listA = New-Object System.Collections.Generic.List[object]
    $logA = New-Object System.Collections.Generic.List[string]
    $resA = Invoke-MinimizeAppWindows -Hwnds @([IntPtr]0xA01, [IntPtr]0xA02, [IntPtr]0xA03, [IntPtr]0xA04, [IntPtr]0xA05, [IntPtr]0xA01) -Minimized $listA `
        -Describe { param($h) $wins[([IntPtr]$h).ToInt64()] } -Minimize { param($h) $minA.Add($h); [PSCustomObject]@{ showCmd = 1 } } `
        -Log { param($m) $logA.Add($m) } -ShellPid $shellPid
    Check '全螢幕最小化：只動兩扇一般主視窗（Chrome、Terminal），重複的只動一次' ($minA.Count -eq 2 -and $minA[0] -eq [IntPtr]0xA01 -and $minA[1] -eq [IntPtr]0xA04) ($minA -join ',')
    Check '全螢幕最小化：Count=2、記入 -Minimized 供 Restore-Occluders 還原' ($resA.Count -eq 2 -and $listA.Count -eq 2 -and $null -ne $listA[0].Snapshot)
    Check '全螢幕最小化：跳過 Shell_SystemDim、Shell_SystemDialog、有 owner 的對話框，並寫明原因' (@($resA.Skipped).Count -eq 3 -and
        @($resA.Skipped | Where-Object { $_ -match 'Shell_SystemDim' }).Count -eq 1 -and @($resA.Skipped | Where-Object { $_ -match 'owner' }).Count -eq 1) ($resA.Skipped -join ' | ')
    Check '全螢幕最小化：跳過的視窗有記錄' (@($logA | Where-Object { $_ -like '不最小化*' }).Count -eq 3) ($logA -join ' | ')
}

# ── 結構：兩支滾輪腳本 ─────────────────────────────────────────────────────────
foreach ($name in 'verify-4.7-wheel.ps1', 'verify-4.7-wheelrouting.ps1') {
    $path = Join-Path $toolsDir $name
    $t = $null; $e = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseFile($path, [ref]$t, [ref]$e)
    Check "$name 解析 0 錯誤" (@($e).Count -eq 0) (($e | ForEach-Object { $_.Message }) -join ' | ')
    $text = $ast.Extent.Text
    Check "$name 匯入 lib/Occluders.psm1" ($text -match "lib\\Occluders\.psm1")
    $clear = $ast.Find({ param($n) $n -is [System.Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq 'Clear-Occluders' }, $true)
    $fg = $ast.Find({ param($n) $n -is [System.Management.Automation.Language.AssignmentStatementAst] -and $n.Left.Extent.Text -eq '$baseFg' }, $true)
    Check "$name 有 Clear-Occluders 呼叫" ($null -ne $clear)
    Check "$name 先清遮擋、後取前景基準（最小化可能改變前景）" ($clear -and $fg -and $clear.Extent.StartOffset -lt $fg.Extent.StartOffset)
    $restore = $ast.Find({ param($n) $n -is [System.Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq 'Restore-Occluders' }, $true)
    $inFinally = $false
    if ($restore) {
        $p = $restore.Parent
        while ($p) { if ($p -is [System.Management.Automation.Language.TryStatementAst] -and $p.Finally -and $restore.Extent.StartOffset -ge $p.Finally.Extent.StartOffset -and $restore.Extent.EndOffset -le $p.Finally.Extent.EndOffset) { $inFinally = $true }; $p = $p.Parent }
    }
    Check "$name 在 finally 還原被最小化的視窗" $inFinally
    # review 31c7c53：結束碼改經 lib 的 Get-VerdictExitCode（-EnvBlocked → 3）。
    Check "$name 仍被遮擋 → ENV-BLOCKED 結束碼 3" ($text -match "'ENV-BLOCKED\*'" -and ($text -match 'exit 3' -or $text -match 'Get-VerdictExitCode[^\r\n]*-EnvBlocked'))
    Check "$name 不再把遮擋當 PRECONDITION FAIL" ($text -notmatch "PRECONDITION: 整個 macro 小工具矩形被其他一般視窗遮擋")
}

# ── review 642050f／31c7c53：呼叫端把未還原的視窗寫進摘要，但不升級成產品 FAIL ─────────────────────────
# 用 AST 掃 host/tools/*.ps1 中所有呼叫 Restore-Occluders 的腳本（不寫死清單）：
#   - 每個 Restore-Occluders 呼叫都傳 -NotRestored <變數>；
#   - 未還原不得放進 $results／$verdicts（環境問題，不是產品 FAIL）——不得有鍵含「還原」且右邊引用該變數的索引賦值；
#   - 摘要以 Format-UnrestoredWarning <變數> 另起醒目的警示行；
#   - 結束碼一律經 lib/VerifyVerdict 的 Get-VerdictExitCode，並傳 -NotRestored <同一個變數>
#     （優先序 產品 FAIL 1 ＞ 鎖定 2 ＞ 環境 3，見 tests/VerifyVerdict.Tests.ps1）。
$restoreScripts = @(Get-ChildItem $toolsDir -Filter *.ps1 | Where-Object {
        $tk = $null; $er = $null
        $a = [System.Management.Automation.Language.Parser]::ParseFile($_.FullName, [ref]$tk, [ref]$er)
        $null -ne $a.Find({ param($n) $n -is [System.Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq 'Restore-Occluders' }, $true)
    })
Check '掃到呼叫 Restore-Occluders 的腳本（至少 13 支）' ($restoreScripts.Count -ge 13) "count=$($restoreScripts.Count)"
function Get-ParamVarName($cmd, [string]$param) {
    $els = @($cmd.CommandElements)
    for ($i = 0; $i -lt $els.Count; $i++) {
        if ($els[$i] -is [System.Management.Automation.Language.CommandParameterAst] -and $els[$i].ParameterName -eq $param) {
            $arg = if ($els[$i].Argument) { $els[$i].Argument } elseif ($i + 1 -lt $els.Count) { $els[$i + 1] } else { $null }
            if ($arg -is [System.Management.Automation.Language.VariableExpressionAst]) { return $arg.VariablePath.UserPath }
            return '?'
        }
    }
    return $null
}
foreach ($f in $restoreScripts) {
    $name = $f.Name
    $tk = $null; $er = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseFile($f.FullName, [ref]$tk, [ref]$er)
    $cmds = { param($cn) @($ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq $cn }.GetNewClosure(), $true)) }
    $calls = & $cmds 'Restore-Occluders'
    $vars = @($calls | ForEach-Object { Get-ParamVarName $_ 'NotRestored' } | Select-Object -Unique)
    $okVar = $vars.Count -eq 1 -and $null -ne $vars[0] -and $vars[0] -ne '?'
    Check "$name：每個 Restore-Occluders 都傳 -NotRestored（同一個變數）" ($okVar -and @($calls | Where-Object { $null -eq (Get-ParamVarName $_ 'NotRestored') }).Count -eq 0) "vars=$($vars -join ',')"
    $v = if ($okVar) { $vars[0] } else { '__none__' }
    $row = $ast.Find({ param($n) $n -is [System.Management.Automation.Language.AssignmentStatementAst] -and
            $n.Left -is [System.Management.Automation.Language.IndexExpressionAst] -and $n.Left.Index.Extent.Text -match '還原' -and
            $n.Right.Extent.Text -match [regex]::Escape("`$$v") }, $true)
    Check "$name：未還原不放進逐項結果（不升級成產品 FAIL）" ($null -eq $row) $(if ($row) { $row.Extent.Text })
    $warn = @(& $cmds 'Format-UnrestoredWarning' | Where-Object { $_.Extent.Text -match [regex]::Escape("`$$v") })
    Check "$name：摘要以 Format-UnrestoredWarning `$$v 另起警示行" ($warn.Count -ge 1)
    $exits = @(& $cmds 'Get-VerdictExitCode')
    $exitsNr = @($exits | Where-Object { (Get-ParamVarName $_ 'NotRestored') -eq $v })
    Check "$name：結束碼經 Get-VerdictExitCode -NotRestored `$$v" ($exits.Count -ge 1 -and $exitsNr.Count -eq $exits.Count) "exits=$($exits.Count) withNr=$($exitsNr.Count)"
    Check "$name：匯入 lib/VerifyVerdict.psm1" ($ast.Extent.Text -match 'lib\\VerifyVerdict\.psm1')
}

# ── 中途停止路徑（if 內提前 exit，帶 -Locked／-EnvBlocked）：警示行不能只 Write-Host，要寫進 summary 檔 ──
# 該路徑的區塊內必須有一條管線：引用警示變數、經 ConvertTo-EvidenceText、再 Set-Content／Add-Content 落檔。
function Get-EarlyStopPersistViolations($Ast) {
    $out = New-Object System.Collections.Generic.List[string]
    $warnVar = $null
    $wa = $Ast.Find({ param($n) $n -is [System.Management.Automation.Language.AssignmentStatementAst] -and $n.Right.Extent.Text -match 'Format-UnrestoredWarning' }, $true)
    if ($wa) { $warnVar = $wa.Left.VariablePath.UserPath }
    foreach ($ex in $Ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.ExitStatementAst] }, $true)) {
        $vc = $ex.Find({ param($n) $n -is [System.Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq 'Get-VerdictExitCode' }, $true)
        if (-not $vc) { continue }
        if (-not ($vc.CommandElements | Where-Object { $_ -is [System.Management.Automation.Language.CommandParameterAst] -and $_.ParameterName -in 'Locked', 'EnvBlocked' })) { continue }
        $blk = $ex.Parent
        while ($blk -and $blk -isnot [System.Management.Automation.Language.IfStatementAst]) { $blk = $blk.Parent }
        if (-not $blk) { continue }   # 最後一個 exit（頂層）：摘要已寫過
        $ok = $false
        if ($warnVar) {
            foreach ($pl in $blk.FindAll({ param($n) $n -is [System.Management.Automation.Language.PipelineAst] }, $true)) {
                $names = @($pl.PipelineElements | Where-Object { $_ -is [System.Management.Automation.Language.CommandAst] } | ForEach-Object { $_.GetCommandName() })
                if ('ConvertTo-EvidenceText' -in $names -and ('Set-Content' -in $names -or 'Add-Content' -in $names) -and $pl.Extent.Text -match [regex]::Escape("`$$warnVar")) { $ok = $true }
            }
        }
        if (-not $ok) { $out.Add("第 $($ex.Extent.StartLineNumber) 行提前 exit 的區塊沒有把 `$$warnVar 經 ConvertTo-EvidenceText 寫進檔案") }
    }
    return $out.ToArray()
}
$esParse = { param($code) [System.Management.Automation.Language.Parser]::ParseInput($code, [ref]$null, [ref]$null) }
$esBad = '$w = Format-UnrestoredWarning -NotRestored $nr' + "`n" + 'if ($b) { Write-Host $w; exit (Get-VerdictExitCode -Locked -NotRestored $nr) }'
$esOk = '$w = Format-UnrestoredWarning -NotRestored $nr' + "`n" + 'if ($b) { @("x", $w) | ConvertTo-EvidenceText | Set-Content -Path $p; exit (Get-VerdictExitCode -Locked -NotRestored $nr) }'
$esNoScrub = '$w = Format-UnrestoredWarning -NotRestored $nr' + "`n" + 'if ($b) { @("x", $w) | Set-Content -Path $p; exit (Get-VerdictExitCode -Locked -NotRestored $nr) }'
Check '自我檢查：提前 exit 只 Write-Host 警示 → 抓到' (@(Get-EarlyStopPersistViolations (& $esParse $esBad)).Count -eq 1)
Check '自我檢查：提前 exit 區塊內經 ConvertTo-EvidenceText 落檔 → 不誤報' (@(Get-EarlyStopPersistViolations (& $esParse $esOk)).Count -eq 0)
Check '自我檢查：落檔但沒經 ConvertTo-EvidenceText → 抓到' (@(Get-EarlyStopPersistViolations (& $esParse $esNoScrub)).Count -eq 1)
foreach ($f in $restoreScripts) {
    $ast = [System.Management.Automation.Language.Parser]::ParseFile($f.FullName, [ref]$null, [ref]$null)
    $v = @(Get-EarlyStopPersistViolations $ast)
    Check "$($f.Name)：中途停止路徑的未還原警示寫進 summary／log（經 ConvertTo-EvidenceText）" ($v.Count -eq 0) ($v -join ' | ')
}

# ── probe-1.2：Set-ForegroundNoInject attach 到使用者前景執行緒之前先查 IsHungAppWindow；無回應＝環境（結束碼 3）──
$p12 = Join-Path $toolsDir 'probe-1.2.ps1'
$t = $null; $e = $null
$ast12 = [System.Management.Automation.Language.Parser]::ParseFile($p12, [ref]$t, [ref]$e)
$fn12 = $ast12.Find({ param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'Set-ForegroundNoInject' }, $true)
$body12 = if ($fn12) { $fn12.Body.Extent.Text } else { '' }
$iHung = $body12.IndexOf('IsHungAppWindow'); $iAttach = $body12.IndexOf('AttachThreadInput(')
Check 'probe-1.2：Set-ForegroundNoInject 在 AttachThreadInput 之前查 IsHungAppWindow' ($iHung -ge 0 -and $iAttach -gt $iHung) "hung=$iHung attach=$iAttach"
Check 'probe-1.2：無回應時丟 ENV-BLOCKED（不 attach；2 留給工作階段鎖定）' ($body12 -match "throw\s+[`"']ENV-BLOCKED" -and $body12 -notmatch "throw\s+[`"']BLOCKED")
$catch12 = @($ast12.FindAll({ param($n) $n -is [System.Management.Automation.Language.CatchClauseAst] -and $n.Body.Extent.Text -match "ENV-BLOCKED\*" }, $true))
Check 'probe-1.2：ENV-BLOCKED 以結束碼 3 結束' (@($catch12 | Where-Object { $_.Body.Extent.Text -match '\$exitCode\s*=\s*3' }).Count -ge 1)
Check 'probe-1.2：宣告 IsHungAppWindow' ($ast12.Extent.Text -match 'extern\s+bool\s+IsHungAppWindow')
$fin12 = @($ast12.FindAll({ param($n) $n -is [System.Management.Automation.Language.TryStatementAst] -and $n.Finally }, $true))
$finCalls = @($fin12 | ForEach-Object { $_.Finally.FindAll({ param($n) $n -is [System.Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq 'Set-ForegroundNoInject' }, $true) })
$finWrapped = @($finCalls | Where-Object {
        $p = $_.Parent; $inTry = $false
        while ($p) { if ($p -is [System.Management.Automation.Language.TryStatementAst] -and $p.CatchClauses.Count -gt 0 -and $_.Extent.StartOffset -ge $p.Body.Extent.StartOffset -and $_.Extent.EndOffset -le $p.Body.Extent.EndOffset) { $inTry = $true }; $p = $p.Parent }
        $inTry })
Check 'probe-1.2：finally 還原前景的呼叫包在 try/catch（無回應時略過、不中斷收尾）' ($finCalls.Count -ge 1 -and $finWrapped.Count -eq $finCalls.Count) "calls=$($finCalls.Count) wrapped=$($finWrapped.Count)"

Write-Host "`n$($script:Pass) passed, $($script:Fail) failed"
if ($script:Fail -gt 0) { exit 1 }
exit 0
