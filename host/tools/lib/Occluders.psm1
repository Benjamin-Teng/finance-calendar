<#
.SYNOPSIS
    驗收腳本的「遮擋前提」：要點的座標若被使用者的一般視窗蓋住，暫時最小化那扇視窗，結束時照原
    WINDOWPLACEMENT 還原（fix F7，從 verify-7.5／7.6／5.3／dragdpi-esc 的同名函式抽出）。

.DESCRIPTION
    - `Get-OccluderAction`：純函式，依命中點的最上層視窗決定動作——
        clear（命中目標小工具；沒給 -TargetHwnd 時命中宿主任一視窗）、
        host-other（宿主自己的另一扇視窗：不得最小化）、
        shell（Progman／WorkerW／Shell_TrayWnd：桌面或工作列，最小化不了）、
        own（腳本自己的視窗，例如基準表單：配置錯誤）、
        none（沒命中任何視窗）、
        system（fix F7b：不在白名單——不可見、cloaked、有 owner、TOOLWINDOW、TOPMOST、DISABLED、沒有
          MINIMIZEBOX、系統類別、殼層 explorer 或系統 UI 行程；一律不動，呼叫端判 BLOCKED）、
        minimize（只有通過白名單的一般應用程式主視窗，見 `Get-NotMinimizableReason`）。
    - `Clear-Occluders -HostPid -Points [-TargetHwnd] -Minimized <List> [-OwnPids] [-Log]`：逐點檢查，
      必要時最小化（每點最多 -MaxPerPoint 次）。回傳 `@{ Ok; Reason }`，不丟例外；Ok=False 時呼叫端
      判 BLOCKED（環境），不是 FAIL。被最小化的視窗與原 placement 加進 -Minimized（同一扇只記一次）。
    - `Restore-Occluders -Minimized <List> [-Log] [-NotRestored <List[string]>]`：反向還原（後最小化者先還原），
      還原後清空清單。只用非同步呼叫（ShowWindowAsync、SWP_ASYNCWINDOWPOS）＋輪詢讀回、有逾時；目標無回應
      （IsHungAppWindow）就不碰。沒還原成功的視窗列進 -NotRestored 並記錄「未還原」。
    - 對他人視窗不得用同步的 ShowWindow／SetWindowPlacement／不帶 SWP_ASYNCWINDOWPOS 的 SetWindowPos：目標 UI
      執行緒卡住時呼叫端會無限等待（2026-10-06 verify-7.3 收尾卡 30 分鐘；tests/Occluders.Tests.ps1 靜態檢查）。
      無回應的視窗也不最小化（Get-NotMinimizableReason → system → ENV-BLOCKED）。
    - `Invoke-MinimizeAppWindows -Hwnds -Minimized [-Log]`（fix F8）：把一組視窗（例如全螢幕截圖前的所有
      一般視窗）照同一份白名單暫時最小化；不在白名單的跳過並記原因，不中止。還原同樣用 Restore-Occluders。
    - 系統 UI 類別／行程清單在 lib/SystemUi.psm1（fix F8），與 SafeInput 前置探查的前景判斷共用。
    - host/tools/*.ps1 不得自行處理遮擋或自行最小化視窗（tests/NoInlineMinimize.Tests.ps1 靜態檢查）。

    座標與 `WindowFromPoint` 以呼叫端執行緒的 DPI 感知解讀，與呼叫端 `GetWindowRect` 一致即可。
    -HitTest／-Minimize／-Describe／-ShellPid／-Restore 可注入假的實作（只供 tests/Occluders.Tests.ps1；
    host/tools/*.ps1 傳這些參數＝繞過白名單，tests/NoInlineMinimize.Tests.ps1 靜態禁止）。
    匯出清單只含經白名單的入口與純函式（fix F8b），最小化原語 Invoke-MinimizeWindow 不匯出。
#>
Set-StrictMode -Version Latest

if (-not ('FcOcc.Native' -as [type])) {
    Add-Type -Namespace FcOcc -Name Native -MemberDefinition @'
[DllImport("user32.dll")] public static extern System.IntPtr WindowFromPoint(POINT p);
[DllImport("user32.dll")] public static extern System.IntPtr GetAncestor(System.IntPtr h, uint flags);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr h, out uint pid);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassName(System.IntPtr h, System.Text.StringBuilder s, int n);
[DllImport("user32.dll")] public static extern bool ShowWindowAsync(System.IntPtr h, int cmd);
[DllImport("user32.dll")] public static extern bool GetWindowRect(System.IntPtr h, out RECT r);
[DllImport("user32.dll")] public static extern bool IsWindowArranged(System.IntPtr h);
[DllImport("user32.dll")] public static extern bool IsZoomed(System.IntPtr h);
[DllImport("user32.dll")] public static extern bool IsIconic(System.IntPtr h);
[DllImport("user32.dll")] public static extern bool SetWindowPos(System.IntPtr h, System.IntPtr after, int x, int y, int cx, int cy, uint flags);
[DllImport("user32.dll")] public static extern System.IntPtr GetShellWindow();
[DllImport("user32.dll")] public static extern bool IsWindowVisible(System.IntPtr h);
[DllImport("user32.dll")] public static extern System.IntPtr GetWindow(System.IntPtr h, uint cmd);
[DllImport("user32.dll", EntryPoint = "GetWindowLongW")] public static extern int GetWindowLong(System.IntPtr h, int index);
[DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(System.IntPtr h, int attr, out int value, int size);
[DllImport("user32.dll")] public static extern bool GetWindowPlacement(System.IntPtr h, ref WINDOWPLACEMENT wp);
[DllImport("user32.dll")] public static extern bool IsHungAppWindow(System.IntPtr h);
[DllImport("user32.dll")] public static extern bool IsWindow(System.IntPtr h);
[StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
[StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
[StructLayout(LayoutKind.Sequential)] public struct WINDOWPLACEMENT {
  public int length; public int flags; public int showCmd; public POINT ptMinPosition; public POINT ptMaxPosition;
  public RECT rcNormalPosition; }
'@
}

$script:ShellClasses = @('Progman', 'WorkerW', 'Shell_TrayWnd')

# fix F7b：可最小化的只有「一般應用程式主視窗」（白名單，見 Get-NotMinimizableReason）。系統類別與
# 系統 UI 行程清單只是佐證——就算不在清單上，只要不符合白名單條件也一律不動。
# fix F8：清單移到 lib/SystemUi.psm1（Get-SystemUiReason），與 SafeInput 前置探查的前景判斷共用同一套規則。
Import-Module (Join-Path $PSScriptRoot 'SystemUi.psm1')
$script:WS_DISABLED = 0x08000000L
$script:WS_MINIMIZEBOX = 0x00020000L
$script:WS_EX_TOPMOST = 0x00000008L
$script:WS_EX_TOOLWINDOW = 0x00000080L

function Get-WinProp($Window, [string]$Name, $Default) {
    if ($null -ne $Window -and $Window.PSObject.Properties[$Name]) { return $Window.$Name }
    return $Default
}

# 回傳空字串＝可最小化（一般應用程式主視窗）；否則回傳「為何不動它」。缺任何屬性都視為不在白名單。
function Get-NotMinimizableReason {
    param([Parameter(Mandatory)]$Window, [int]$ShellPid = 0)
    $cls = [string](Get-WinProp $Window 'Class' '')
    $proc = [string](Get-WinProp $Window 'ProcessName' '')
    $visible = Get-WinProp $Window 'Visible' $null
    $cloaked = Get-WinProp $Window 'Cloaked' $null
    $owner = Get-WinProp $Window 'Owner' $null
    $style = Get-WinProp $Window 'Style' $null
    $ex = Get-WinProp $Window 'ExStyle' $null
    $hung = Get-WinProp $Window 'Hung' $null
    if ($null -eq $visible -or $null -eq $cloaked -or $null -eq $owner -or $null -eq $style -or $null -eq $ex -or $null -eq $hung) {
        return '缺視窗屬性（無法確認是一般應用程式主視窗）'
    }
    $sys = Get-SystemUiReason -Class $cls -ProcessName $proc
    if ($sys) { return $sys }
    # 無回應的視窗不碰：之後的還原對它無法確認結果（memory cross-process-showwindow-blocks-on-hung-target）。
    if ([bool]$hung) { return '目標無回應（IsHungAppWindow）' }
    if ($ShellPid -ne 0 -and [int](Get-WinProp $Window 'Pid' 0) -eq $ShellPid) { return '屬殼層 explorer 行程' }
    if (-not [bool]$visible) { return '不可見' }
    if ([bool]$cloaked) { return 'DWM cloaked' }
    if ([IntPtr]$owner -ne [IntPtr]::Zero) { return '有 owner（對話框／彈出視窗）' }
    $style = [long]$style; $ex = [long]$ex
    if ($ex -band $script:WS_EX_TOOLWINDOW) { return 'WS_EX_TOOLWINDOW' }
    if ($ex -band $script:WS_EX_TOPMOST) { return 'WS_EX_TOPMOST' }
    if ($style -band $script:WS_DISABLED) { return 'WS_DISABLED（有強制回應對話框）' }
    if (-not ($style -band $script:WS_MINIMIZEBOX)) { return '沒有 WS_MINIMIZEBOX' }
    return ''
}

function Get-OccluderAction {
    param(
        [Parameter(Mandatory)]$Window, [Parameter(Mandatory)][int]$HostPid,
        [IntPtr]$TargetHwnd = [IntPtr]::Zero, [int[]]$OwnPids = @(), [int]$ShellPid = 0
    )
    $rootHwnd = [IntPtr](Get-WinProp $Window 'Hwnd' ([IntPtr]::Zero))
    $rootPid = [int](Get-WinProp $Window 'Pid' 0)
    $rootClass = [string](Get-WinProp $Window 'Class' '')
    if ($rootHwnd -eq [IntPtr]::Zero) { return 'none' }
    if ($TargetHwnd -ne [IntPtr]::Zero) {
        if ($rootHwnd -eq $TargetHwnd) { return 'clear' }
        if ($rootPid -eq $HostPid) { return 'host-other' }
    } elseif ($rootPid -eq $HostPid) { return 'clear' }
    if ($rootClass -in $script:ShellClasses) { return 'shell' }
    if ($OwnPids -contains $rootPid) { return 'own' }
    if (Get-NotMinimizableReason -Window $Window -ShellPid $ShellPid) { return 'system' }
    return 'minimize'
}

function Get-ShellPid {
    $sh = [FcOcc.Native]::GetShellWindow()
    $procId = [uint32]0
    if ($sh -ne [IntPtr]::Zero) { [void][FcOcc.Native]::GetWindowThreadProcessId($sh, [ref]$procId) }
    return [int]$procId
}

function Get-RootAtPoint([int]$X, [int]$Y) {
    $p = New-Object FcOcc.Native+POINT; $p.X = $X; $p.Y = $Y
    $root = [FcOcc.Native]::GetAncestor([FcOcc.Native]::WindowFromPoint($p), 2)  # GA_ROOT
    Get-WindowInfo $root
}

# 視窗屬性（Get-NotMinimizableReason 需要的全部欄位）。Hwnd 為 0 時只回 Hwnd／Pid／Class。
function Get-WindowInfo([IntPtr]$root) {
    $procId = [uint32]0
    $sb = New-Object System.Text.StringBuilder 256
    $o = [ordered]@{ Hwnd = $root; Pid = 0; Class = '' }
    if ($root -ne [IntPtr]::Zero) {
        [void][FcOcc.Native]::GetWindowThreadProcessId($root, [ref]$procId)
        [void][FcOcc.Native]::GetClassName($root, $sb, 256)
        $cloaked = 0
        $hr = [FcOcc.Native]::DwmGetWindowAttribute($root, 14, [ref]$cloaked, 4)  # DWMWA_CLOAKED
        $o.Pid = [int]$procId
        $o.Class = $sb.ToString()
        $o.Visible = [FcOcc.Native]::IsWindowVisible($root)
        # 讀不到 cloaked 狀態（hr≠S_OK）時當作 cloaked：不在白名單即不動。
        $o.Cloaked = ($hr -ne 0) -or ($cloaked -ne 0)
        $o.Owner = [FcOcc.Native]::GetWindow($root, 4)  # GW_OWNER
        # GetWindowLongW 回傳有號 int（WS_POPUP 等高位元會變負數），以位元遮罩轉成無號值。
        $o.Style = [long][FcOcc.Native]::GetWindowLong($root, -16) -band 0xFFFFFFFFL  # GWL_STYLE
        $o.ExStyle = [long][FcOcc.Native]::GetWindowLong($root, -20) -band 0xFFFFFFFFL  # GWL_EXSTYLE
        $o.ProcessName = try { (Get-Process -Id $procId -ErrorAction Stop).ProcessName } catch { '' }
        # IsHungAppWindow 只讀狀態、不送訊息給目標，不會被卡住的目標拖住。
        $o.Hung = [FcOcc.Native]::IsHungAppWindow($root)
    }
    [PSCustomObject]$o
}

# fix F7b：Win32 後端（可注入，測試以 mock 取代）。IsWindowArranged：user32.dll，Windows 10 1903 起
# （Microsoft Learn nf-winuser-iswindowarranged；文件註明吸附視窗的 GetWindowPlacement showCmd 可能仍是
# SW_SHOWNORMAL、rcNormalPosition 是吸附前的矩形）。舊系統沒有這個匯出時丟 EntryPointNotFoundException，
# 快照記 $null，還原改靠矩形比對補正。
$script:Win32Backend = @{
    GetPlacement = {
        param($h)
        $wp = New-Object FcOcc.Native+WINDOWPLACEMENT
        $wp.length = [Runtime.InteropServices.Marshal]::SizeOf($wp)
        if (-not [FcOcc.Native]::GetWindowPlacement($h, [ref]$wp)) { return $null }
        $wp
    }
    GetRect      = {
        param($h)
        $r = New-Object FcOcc.Native+RECT
        if (-not [FcOcc.Native]::GetWindowRect($h, [ref]$r)) { return $null }
        [PSCustomObject]@{ Left = $r.Left; Top = $r.Top; Right = $r.Right; Bottom = $r.Bottom }
    }
    IsArranged   = { param($h) [FcOcc.Native]::IsWindowArranged($h) }
    IsZoomed     = { param($h) [FcOcc.Native]::IsZoomed($h) }
    IsIconic     = { param($h) [FcOcc.Native]::IsIconic($h) }
    IsHung       = { param($h) [FcOcc.Native]::IsHungAppWindow($h) }
    IsWindow     = { param($h) [FcOcc.Native]::IsWindow($h) }
    Sleep        = { param($ms) Start-Sleep -Milliseconds $ms }
    Minimize     = { param($h) [void][FcOcc.Native]::ShowWindowAsync($h, 6) }  # SW_MINIMIZE
    # hung-target：對他人視窗的顯示狀態變更一律 ShowWindowAsync（排進目標佇列、不等它處理）。
    # 同步的 ShowWindow／SetWindowPlacement 在目標 UI 執行緒卡住時會讓呼叫端無限等待（2026-10-06 verify-7.3 收尾
    # 卡 30 分鐘被強殺，該視窗留在最小化）。結果一律由 Invoke-RestoreWindow 輪詢讀回、有逾時。
    ShowAsync    = { param($h, $cmd) [void][FcOcc.Native]::ShowWindowAsync($h, [int]$cmd) }
    # 旗標由呼叫端給（$script:SwpRestoreFlags，含 SWP_ASYNCWINDOWPOS：跨執行緒時改為投遞、不等目標處理）。
    SetWindowPos = { param($h, $x, $y, $cx, $cy, $flags) [FcOcc.Native]::SetWindowPos($h, [IntPtr]::Zero, $x, $y, $cx, $cy, [uint32]$flags) }
}
# SWP_ASYNCWINDOWPOS | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER：只搬位置大小，不啟用、不改 z-order、不等目標。
$script:SwpRestoreFlags = 0x4214
$script:SW_SHOWMAXIMIZED = 3
$script:SW_SHOWNOACTIVATE = 4

# 輪詢 $Condition 直到為真或逾時（最多 TimeoutMs/PollMs 次 Sleep）；回傳是否達成。
function Wait-WindowState {
    param([Parameter(Mandatory)][scriptblock]$Condition, [Parameter(Mandatory)][hashtable]$Backend, [int]$TimeoutMs = 3000, [int]$PollMs = 100)
    $polls = [int][Math]::Ceiling([double]$TimeoutMs / [double]([Math]::Max(1, $PollMs)))
    for ($i = 0; ; $i++) {
        if (& $Condition) { return $true }
        if ($i -ge $polls) { return $false }
        & $Backend.Sleep $PollMs
    }
}

function Test-RectEqual($a, $b) {
    if ($null -eq $a -or $null -eq $b) { return $false }
    return ($a.Left -eq $b.Left -and $a.Top -eq $b.Top -and $a.Right -eq $b.Right -and $a.Bottom -eq $b.Bottom)
}

# 最小化前的快照：Placement（GetWindowPlacement）、Rect（GetWindowRect，吸附中＝吸附後的實際矩形）、
# Arranged（IsWindowArranged；API 不可用＝$null）、Maximized（IsZoomed 或 showCmd=SW_SHOWMAXIMIZED）。
# MinimizeConfirmed（review 642050f）：ShowWindowAsync 只把 SW_MINIMIZE 排進目標佇列；目標忙碌（還沒滿 5 秒、
# IsHung=False）時可能晚於之後的還原才生效。送出後輪詢 IsIconic（有逾時）確認，沒確認的由還原端補送並判「未確認」。
function Invoke-MinimizeWindow {
    param([Parameter(Mandatory)][IntPtr]$Hwnd, [hashtable]$Backend = $script:Win32Backend, [int]$TimeoutMs = 1500, [int]$PollMs = 100)
    $wp = & $Backend.GetPlacement $Hwnd
    $rect = & $Backend.GetRect $Hwnd
    $arranged = $null
    try { $arranged = [bool](& $Backend.IsArranged $Hwnd) } catch { $arranged = $null }
    $maximized = [bool](& $Backend.IsZoomed $Hwnd)
    if ($null -ne $wp -and $wp.showCmd -eq 3) { $maximized = $true }
    $iconic = [bool](& $Backend.IsIconic $Hwnd)
    & $Backend.Minimize $Hwnd
    $confirmed = $iconic -or (Wait-WindowState -Backend $Backend -TimeoutMs $TimeoutMs -PollMs $PollMs -Condition { [bool](& $Backend.IsIconic $Hwnd) })
    return [PSCustomObject]@{ Placement = $wp; Rect = $rect; Arranged = $arranged; Maximized = $maximized; Iconic = $iconic; MinimizeConfirmed = [bool]$confirmed }
}

# 還原（hung-target：只用非同步呼叫＋輪詢讀回、有逾時，絕不無限等待）：
#   0. 視窗已銷毀（IsWindow=False）→ Gone=True，不送任何呼叫、不輪詢。
#   1. 目標無回應（IsHungAppWindow）→ 不送任何呼叫，回報「未還原（目標無回應）」，由 Restore-Occluders 列進 NotRestored。
#   2. 最小化前本來就是最小化 → 維持原狀。
#   3. 最小化沒確認（快照 MinimizeConfirmed=False）且此刻仍不是最小化（SW_MINIMIZE 可能還排在目標佇列裡）→
#      （此刻已是最小化＝最小化晚到但已生效，改走下面的一般路徑）照順序補送一次非同步還原
#      （排在那個最小化之後；最大化者送 SW_SHOWMAXIMIZED，其餘 SW_SHOWNOACTIVATE），但此刻讀回無法區分「最小化還沒
#      執行」與「已還原」，一律判 Unconfirmed、Ok=False，由 Restore-Occluders 列進 NotRestored。
#   4. 仍最小化 → ShowWindowAsync(SW_SHOWNOACTIVATE)（不啟用），輪詢 IsIconic。
#   5. 原本最大化但沒回到最大化 → ShowWindowAsync(SW_SHOWMAXIMIZED)，輪詢 IsZoomed。（SW_SHOWNOACTIVATE 是否直接
#      回到最大化未實測，兩種情況都處理。）
#   6. 非最大化者，若原本吸附（Arranged=True）或矩形不等於最小化前矩形 → SetWindowPos＋SWP_ASYNCWINDOWPOS 回原矩形
#      （不啟用、不改 z-order），輪詢 GetWindowRect。吸附狀態本身不會恢復（只還原位置大小），這是已知取捨。
#      逾時後再讀一次：矩形有變動但不等於原矩形 → Mismatch（讀回不符）；完全沒動 → TimedOut（讀回逾時）。
# 任一步逾時 → TimedOut=True、Ok=False（不重試、不改用同步呼叫）。
function Invoke-RestoreWindow {
    param([Parameter(Mandatory)][IntPtr]$Hwnd, [Parameter(Mandatory)]$Snapshot, [hashtable]$Backend = $script:Win32Backend,
        [int]$TimeoutMs = 3000, [int]$PollMs = 100)
    $fmt = { param($r) if ($null -eq $r) { '(none)' } else { "($($r.Left),$($r.Top),$($r.Right),$($r.Bottom))" } }
    $orig = $Snapshot.Rect
    $maximized = [bool]$Snapshot.Maximized
    $result = {
        param([hashtable]$f, $detail)
        $o = [ordered]@{ Ok = $false; Gone = $false; Hung = $false; Unconfirmed = $false; TimedOut = $false; Mismatch = $false
            Shown = $false; Repositioned = $false; RectMatches = $null; Detail = $detail }
        foreach ($k in $f.Keys) { $o[$k] = $f[$k] }
        [PSCustomObject]$o
    }
    if (-not [bool](& $Backend.IsWindow $Hwnd)) {
        return (& $result @{ Gone = $true } '已不存在（視窗已銷毀），略過還原')
    }
    if ([bool](& $Backend.IsHung $Hwnd)) {
        return (& $result @{ Hung = $true } '未還原（目標無回應）：IsHungAppWindow=True，未送任何還原呼叫')
    }
    $wasIconic = $Snapshot.PSObject.Properties['Iconic'] -and $Snapshot.Iconic -eq $true
    if ($wasIconic) { return (& $result @{ Ok = $true } '最小化前本來就是最小化，維持原狀') }
    # 最小化確認逾時、但還原前已經生效（此刻 IsIconic=True）→ 佇列裡已沒有待處理的最小化，走一般還原路徑即可確認。
    if ($Snapshot.PSObject.Properties['MinimizeConfirmed'] -and $Snapshot.MinimizeConfirmed -eq $false -and -not [bool](& $Backend.IsIconic $Hwnd)) {
        $cmd = if ($maximized) { $script:SW_SHOWMAXIMIZED } else { $script:SW_SHOWNOACTIVATE }
        & $Backend.ShowAsync $Hwnd $cmd
        return (& $result @{ Unconfirmed = $true } "最小化未確認（送出後 IsIconic 一直未成立，SW_MINIMIZE 可能仍在目標佇列）；已補送非同步還原 nCmdShow=$cmd，排在最小化之後，結果無法確認")
    }

    $timedOut = $false
    $mismatch = $false
    $shown = -not [bool](& $Backend.IsIconic $Hwnd)
    if (-not $shown) {
        & $Backend.ShowAsync $Hwnd $script:SW_SHOWNOACTIVATE
        $shown = Wait-WindowState -Backend $Backend -TimeoutMs $TimeoutMs -PollMs $PollMs -Condition { -not [bool](& $Backend.IsIconic $Hwnd) }
        if (-not $shown) { $timedOut = $true }
    }
    $zoomed = $null
    if ($shown -and $maximized) {
        $zoomed = [bool](& $Backend.IsZoomed $Hwnd)
        if (-not $zoomed) {
            & $Backend.ShowAsync $Hwnd $script:SW_SHOWMAXIMIZED
            $zoomed = Wait-WindowState -Backend $Backend -TimeoutMs $TimeoutMs -PollMs $PollMs -Condition { [bool](& $Backend.IsZoomed $Hwnd) }
            if (-not $zoomed) { $timedOut = $true }
        }
    }
    $repositioned = $false
    $after = & $Backend.GetRect $Hwnd
    if ($shown -and -not $maximized -and $null -ne $orig -and (($Snapshot.Arranged -eq $true) -or -not (Test-RectEqual $after $orig))) {
        $beforeMove = $after
        $repositioned = [bool](& $Backend.SetWindowPos $Hwnd $orig.Left $orig.Top ($orig.Right - $orig.Left) ($orig.Bottom - $orig.Top) $script:SwpRestoreFlags)
        $placed = Wait-WindowState -Backend $Backend -TimeoutMs $TimeoutMs -PollMs $PollMs -Condition { Test-RectEqual (& $Backend.GetRect $Hwnd) $orig }
        $after = & $Backend.GetRect $Hwnd
        if (-not $placed) {
            # 逾時後再讀一次：有動但不是原矩形＝目標處理了請求、系統給了別的矩形（讀回不符）；完全沒動＝讀回逾時。
            if (-not (Test-RectEqual $after $beforeMove) -and -not (Test-RectEqual $after $orig)) { $mismatch = $true } else { $timedOut = $true }
        }
    }
    $matches_ = if ($maximized) { $null } else { Test-RectEqual $after $orig }
    $ok = $shown -and $(if ($maximized) { $zoomed -eq $true } else { $matches_ -eq $true })
    $detail = "shown=$shown arranged=$($Snapshot.Arranged) maximized=$maximized zoomed=$zoomed repositioned=$repositioned timedOut=$timedOut mismatch=$mismatch rect 原=$(& $fmt $orig) 後=$(& $fmt $after) 相符=$matches_"
    return (& $result @{ Ok = [bool]$ok; TimedOut = $timedOut; Mismatch = $mismatch; Shown = [bool]$shown; Repositioned = $repositioned; RectMatches = $matches_ } $detail)
}

function Clear-Occluders {
    param(
        [Parameter(Mandatory)][int]$HostPid,
        [Parameter(Mandatory)][object[]]$Points,
        [Parameter(Mandatory)][AllowEmptyCollection()][System.Collections.Generic.List[object]]$Minimized,
        [IntPtr]$TargetHwnd = [IntPtr]::Zero,
        [int[]]$OwnPids = @(),
        [scriptblock]$Log = $null,
        [int]$MaxPerPoint = 8,
        [int]$SettleMs = 600,
        [scriptblock]$HitTest = $null,
        [scriptblock]$Minimize = $null,
        [int]$ShellPid = -1
    )
    if (-not $HitTest) { $HitTest = { param($x, $y) Get-RootAtPoint $x $y } }
    if (-not $Minimize) { $Minimize = { param($h) Invoke-MinimizeWindow -Hwnd $h } }
    if ($ShellPid -lt 0) { $ShellPid = Get-ShellPid }
    foreach ($pt in $Points) {
        $x = [int]$pt[0]; $y = [int]$pt[1]
        $ok = $false
        for ($i = 0; $i -lt $MaxPerPoint; $i++) {
            $r = & $HitTest $x $y
            $action = Get-OccluderAction -Window $r -HostPid $HostPid -TargetHwnd $TargetHwnd -OwnPids $OwnPids -ShellPid $ShellPid
            $desc = "0x$('{0:X}' -f ([IntPtr]$r.Hwnd).ToInt64())（$($r.Class)，pid $($r.Pid)）"
            # fix F8b：失敗結果帶 Action 與 Class，供 Get-OccluderVerdict 區分「產品回歸」與「環境」。
            $fail = { param($a, $why) [PSCustomObject]@{ Ok = $false; Action = $a; Class = [string]$r.Class; Reason = $why } }
            switch ($action) {
                'clear' { $ok = $true }
                'none' { return (& $fail 'none' "($x,$y) 沒有命中任何視窗") }
                'shell' { return (& $fail 'shell' "($x,$y) 被 $desc 蓋住（桌面／工作列，無法點到小工具）") }
                'system' {
                    $why = Get-NotMinimizableReason -Window $r -ShellPid $ShellPid
                    return (& $fail 'system' "($x,$y) 被 $desc 蓋住（$why：不是一般應用程式主視窗，不最小化）")
                }
                'host-other' { return (& $fail 'host-other' "($x,$y) 被宿主自己的另一扇視窗 $desc 蓋住") }
                'own' { return (& $fail 'own' "($x,$y) 被腳本自己的視窗 $desc 蓋住（腳本配置錯誤）") }
                default {
                    $already = @($Minimized | Where-Object { $_.Hwnd -eq $r.Hwnd }).Count -gt 0
                    if (-not $already) {
                        $snapshot = & $Minimize $r.Hwnd
                        $Minimized.Add([PSCustomObject]@{ Hwnd = [IntPtr]$r.Hwnd; Snapshot = $snapshot; Cls = $r.Class })
                        if ($Log) { & $Log "遮擋：($x,$y) 被 $desc 蓋住 → 暫時最小化" }
                    }
                    if ($SettleMs -gt 0) { Start-Sleep -Milliseconds $SettleMs }
                }
            }
            if ($ok) { break }
        }
        if (-not $ok) { return [PSCustomObject]@{ Ok = $false; Action = 'stuck'; Class = [string]$r.Class; Reason = "($x,$y) 最小化 $MaxPerPoint 次後仍被其他視窗蓋住" } }
    }
    return [PSCustomObject]@{ Ok = $true; Action = 'clear'; Class = ''; Reason = '' }
}

# fix F8b：Clear-Occluders 的結果 → 驗收判讀。呼叫端的按下點是由小工具「目前的實際矩形」推出來的，所以：
#   - 命中桌面（Progman／WorkerW）或沒命中任何視窗＝小工具不在預期位置、沒有顯示或沉到桌面之下——
#     這是被驗證的物件本身出錯（z-order／置底正是本 change 的核心不變式），判 FAIL，不得遮成環境問題；
#   - 命中宿主自己的另一扇視窗＝小工具互相重疊或 z-order 錯，同樣是產品問題 → FAIL；
#   - 被腳本自己的視窗蓋住＝腳本配置錯誤 → FAIL；
#   - 工作列（Shell_TrayWnd）、系統 UI 與其他不在白名單的視窗、最小化後仍蓋著 → ENV-BLOCKED（環境）。
#   - 結果缺 Action（舊格式）或無法辨識 → FAIL：證明不了是環境，就不准判成環境。
# fix F9：-TargetIsDesktop＝要點的本來就是桌面（verify-6.1-icon-click 的圖示中心、hittest 的小工具邊外點；
# 呼叫端以 Clear-Occluders -HostPid <宿主> -TargetHwnd <桌面視窗> 清遮擋）。此時命中桌面（Progman／WorkerW，
# 不論是不是 -TargetHwnd 那一扇）＝到達目標 → ok；宿主視窗蓋住應落在桌面的點、沒命中任何視窗、腳本自己的視窗
# 仍是 FAIL；工作列、系統 UI、最小化後仍蓋著仍是 ENV-BLOCKED。
# 回傳 @{ Verdict = 'ok'|'fail'|'env-blocked'; Message }。
$script:DesktopClasses = @('Progman', 'WorkerW')
function Get-OccluderVerdict {
    param([Parameter(Mandatory)][AllowNull()]$Result, [switch]$TargetIsDesktop)
    $ok = Get-WinProp $Result 'Ok' $false
    if ($ok -eq $true) { return [PSCustomObject]@{ Verdict = 'ok'; Message = '' } }
    $action = [string](Get-WinProp $Result 'Action' '')
    $cls = [string](Get-WinProp $Result 'Class' '')
    $reason = [string](Get-WinProp $Result 'Reason' '(沒有遮擋結果)')
    if ($TargetIsDesktop -and $action -eq 'shell' -and $cls -in $script:DesktopClasses) {
        return [PSCustomObject]@{ Verdict = 'ok'; Message = "$reason（目標本來就是桌面：已到達桌面）" }
    }
    $v = 'fail'
    $msg = switch ($action) {
        'none' {
            if ($TargetIsDesktop) { "$reason——應落在桌面的點沒有命中任何視窗：座標不在任何螢幕上" }
            else { "$reason——按下點由小工具的實際矩形推出，卻沒有命中任何視窗：小工具不在預期位置或沒有顯示" }
        }
        'shell' {
            if ($cls -in $script:DesktopClasses) { "$reason——按下點由小工具的實際矩形推出，卻命中桌面：小工具不在預期位置或沉到桌面之下" }
            else { $v = 'env-blocked'; "$reason（工作列蓋住小工具所在的螢幕邊緣）" }
        }
        'host-other' {
            if ($TargetIsDesktop) { "$reason——宿主視窗蓋住應落在桌面的點：小工具超出自己的矩形或攔截了點擊" }
            else { "$reason——小工具互相重疊或 z-order 錯" }
        }
        'own' { "$reason" }
        'system' { $v = 'env-blocked'; "$reason" }
        'stuck' { $v = 'env-blocked'; "$reason" }
        default { "$reason（遮擋結果缺少或無法辨識 Action=$action，無法證明是環境問題）" }
    }
    return [PSCustomObject]@{ Verdict = $v; Message = [string]$msg }
}

# fix F8b：驗收腳本唯一的遮擋判讀入口（取代各腳本自己的同名函式）。Ok → 不做事；環境 → 丟
# 「ENV-BLOCKED: 」開頭的例外（呼叫端 catch 記錄、結束碼 3）；其餘 → 丟「FAIL: 」開頭的一般例外（呼叫端判 FAIL）。
function Assert-OccluderResult {
    param([Parameter(Mandatory)][AllowNull()]$Result, [string]$What = '按下點', [switch]$TargetIsDesktop)
    $v = Get-OccluderVerdict -Result $Result -TargetIsDesktop:$TargetIsDesktop
    if ($v.Verdict -eq 'ok') { return }
    if ($v.Verdict -eq 'env-blocked') { throw "ENV-BLOCKED: $What 的遮擋無法排除：$($v.Message)" }
    $where = if ($TargetIsDesktop) { '桌面' } else { '小工具' }
    throw "FAIL: $What 沒有落在${where}上：$($v.Message)"
}

# fix F8：把一組視窗（例如整個螢幕的一般視窗，供全螢幕截圖）暫時最小化——同樣只動白名單內的一般應用程式
# 主視窗；不在白名單的跳過並記原因（不中止）。被最小化者加進 -Minimized，以 Restore-Occluders 還原。
# 回傳 @{ Count＝本次最小化數; Skipped＝跳過的描述清單 }。-Describe／-Minimize 可注入（測試用）。
function Invoke-MinimizeAppWindows {
    param(
        [Parameter(Mandatory)][AllowEmptyCollection()][IntPtr[]]$Hwnds,
        [Parameter(Mandatory)][AllowEmptyCollection()][System.Collections.Generic.List[object]]$Minimized,
        [scriptblock]$Log = $null,
        [scriptblock]$Describe = $null,
        [scriptblock]$Minimize = $null,
        [int]$ShellPid = -1
    )
    if (-not $Describe) { $Describe = { param($h) Get-WindowInfo $h } }
    if (-not $Minimize) { $Minimize = { param($h) Invoke-MinimizeWindow -Hwnd $h } }
    if ($ShellPid -lt 0) { $ShellPid = Get-ShellPid }
    $count = 0
    $skipped = New-Object System.Collections.Generic.List[string]
    foreach ($h in $Hwnds) {
        $w = & $Describe $h
        $desc = "0x$('{0:X}' -f ([IntPtr]$h).ToInt64())（$(Get-WinProp $w 'Class' '?')，pid $(Get-WinProp $w 'Pid' '?')）"
        $why = Get-NotMinimizableReason -Window $w -ShellPid $ShellPid
        if ($why) {
            $skipped.Add("$desc：$why")
            if ($Log) { & $Log "不最小化 $desc：$why" }
            continue
        }
        if (@($Minimized | Where-Object { $_.Hwnd -eq [IntPtr]$h }).Count -gt 0) { continue }
        $snapshot = & $Minimize ([IntPtr]$h)
        $Minimized.Add([PSCustomObject]@{ Hwnd = [IntPtr]$h; Snapshot = $snapshot; Cls = (Get-WinProp $w 'Class' '') })
        $count++
        if ($Log) { & $Log "暫時最小化 $desc" }
    }
    return [PSCustomObject]@{ Count = $count; Skipped = $skipped.ToArray() }
}

function Restore-Occluders {
    param(
        [Parameter(Mandatory)][AllowEmptyCollection()][System.Collections.Generic.List[object]]$Minimized,
        [scriptblock]$Log = $null,
        [scriptblock]$Restore = $null,
        # hung-target：沒還原成功（目標無回應、讀回逾時、例外）的視窗描述加進這裡（呼叫端可據此判讀／回報）；
        # 不給也照樣記錄並在結尾寫一行「未還原 N 扇」。
        [AllowNull()][System.Collections.Generic.List[string]]$NotRestored = $null
    )
    if (-not $Restore) { $Restore = { param($h, $s) Invoke-RestoreWindow -Hwnd $h -Snapshot $s } }
    $failed = New-Object System.Collections.Generic.List[string]
    for ($i = $Minimized.Count - 1; $i -ge 0; $i--) {
        $m = $Minimized[$i]
        $desc = "0x$('{0:X}' -f ([IntPtr]$m.Hwnd).ToInt64())（$($m.Cls)）"
        try {
            $res = & $Restore $m.Hwnd $m.Snapshot
            if ($res -is [bool]) {
                $ok = $res; $txt = "ok=$res"
            } else {
                $ok = $res.Ok -eq $true; $txt = "ok=$($res.Ok) $($res.Detail)"
            }
            $flag = { param($n) $res -isnot [bool] -and $res.PSObject.Properties[$n] -and $res.$n -eq $true }
            if ($ok) {
                if ($Log) { & $Log "還原 $desc $txt" }
            } elseif (& $flag 'Gone') {
                # 視窗已銷毀：沒有留在最小化的視窗，不列進 NotRestored，只記錄。
                if ($Log) { & $Log "略過 $desc：已不存在（視窗已銷毀）" }
            } else {
                $why = if (& $flag 'Hung') { '未還原（目標無回應）' }
                elseif (& $flag 'Unconfirmed') { '未還原（最小化未確認：已補送非同步還原，結果無法確認）' }
                elseif (& $flag 'Mismatch') { '未還原（讀回不符）' }
                elseif (& $flag 'TimedOut') { '未還原（讀回逾時，未確認）' }
                else { '未還原（結果不符）' }
                $failed.Add("$desc $why：$txt")
                if ($Log) { & $Log "$why $desc $txt" }
            }
        } catch {
            $failed.Add("$desc 未還原（例外）：$_")
            if ($Log) { & $Log "還原 $desc 失敗（未還原）：$_" }
        }
    }
    $Minimized.Clear()
    if ($null -ne $NotRestored) { foreach ($f in $failed) { $NotRestored.Add($f) } }
    if ($failed.Count -gt 0 -and $Log) { & $Log "未還原 $($failed.Count) 扇，需手動還原：$($failed -join '；')" }
}

# fix F8b：只匯出經白名單的入口（Clear-Occluders、Invoke-MinimizeAppWindows、Restore-Occluders）與純函式。
# Invoke-MinimizeWindow（不經白名單的最小化原語）與 Invoke-RestoreWindow（可注入 -Backend）不匯出；
# tests/Occluders.Tests.ps1 經模組範圍呼叫它們，tests/NoInlineMinimize.Tests.ps1 禁止腳本這樣做。
Export-ModuleMember -Function Get-OccluderAction, Get-NotMinimizableReason, Clear-Occluders, Restore-Occluders,
Invoke-MinimizeAppWindows, Get-OccluderVerdict, Assert-OccluderResult
