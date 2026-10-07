<#
.SYNOPSIS
    「這扇視窗是不是系統 UI」的共用判斷（fix F8，從 lib/Occluders.psm1 抽出）。純函式，不呼叫任何 Win32 API。

.DESCRIPTION
    兩個使用者，規則同源：
      - lib/Occluders.psm1 的 `Get-NotMinimizableReason`：系統 UI 一律不最小化（白名單的一部分）。
      - lib/SafeInput.psm1 的 `Invoke-SafeInputPreflight`：前景是系統 UI 時不送任何鍵（ENV-BLOCKED）。
    批次 B 補跑的失效鏈：「Windows 安全性」對話框（PickerHost，類別 Shell_SystemDialog，另有全螢幕
    Shell_SystemDim）佔住前景，前置探查的 F15 會送進它，各腳本自帶的遮擋處理也會把 Shell_SystemDim 最小化。

    - `Get-SystemUiReason -Class -ProcessName [-AllowDesktop]`：回傳空字串＝不是系統 UI；否則回傳原因。
      -AllowDesktop：桌面本身（Progman／WorkerW，例如 Win+D 之後的前景）不算——對它送 F15 無害；
      遮擋處理不帶這個開關（桌面不能最小化）。
    - `Get-ForegroundBlockReason -Window`：前置探查用。Window 需有 Hwnd、Class、ProcessName、Cloaked、ExStyle
      （前景的根視窗）。空字串＝可以送探查鍵；否則回傳原因。任何欄位缺漏或查不到一律回傳原因（fail closed）。
      前景是桌面（Progman／WorkerW）時另需 Pid、ShellPid、Overlays（fix F8b）：桌面必須屬殼層 explorer
      （GetShellWindow 的行程），且 Overlays（可見、覆蓋整個螢幕的頂層視窗候選）裡沒有系統 UI——例如
      「Windows 安全性」仍在時的全螢幕 Shell_SystemDim。前景落在桌面不代表畫面上沒有系統 UI，preflight 回 0
      之後的點擊可能落在它上面。
#>
Set-StrictMode -Version Latest

$script:DesktopClasses = @('Progman', 'WorkerW')
$script:SystemClasses = @(
    'Shell_TrayWnd', 'Shell_SecondaryTrayWnd', 'Shell_SystemDim', 'Shell_SystemDialog',
    'Windows.UI.Core.CoreWindow', '#32770', 'NotifyIconOverflowWindow', 'TopLevelWindowForOverflowXamlIsland',
    'tooltips_class32', 'Xaml_WindowedPopupClass', 'ForegroundStaging', 'MultitaskingViewFrame', 'XamlExplorerHostIslandWindow'
)
$script:SystemProcesses = @(
    'PickerHost', 'ShellExperienceHost', 'StartMenuExperienceHost', 'SearchHost', 'SearchApp', 'LockApp', 'LogonUI',
    'TextInputHost', 'ShellHost', 'dwm', 'csrss', 'consent', 'CredentialUIBroker'
)
$script:WS_EX_TOPMOST = 0x00000008L

function Get-SystemUiReason {
    param([AllowEmptyString()][string]$Class = '', [AllowEmptyString()][string]$ProcessName = '', [switch]$AllowDesktop)
    if (-not $AllowDesktop -and $Class -in $script:DesktopClasses) { return "系統類別 $Class" }
    if ($Class -in $script:SystemClasses) { return "系統類別 $Class" }
    if ($ProcessName -and ($script:SystemProcesses -contains $ProcessName)) { return "系統 UI 行程 $ProcessName" }
    return ''
}

function Get-FgProp($Window, [string]$Name) {
    if ($null -ne $Window -and $Window.PSObject.Properties[$Name]) { return $Window.$Name }
    return $null
}

# fix F8b：系統覆蓋層＝可見、未 cloaked、覆蓋整個螢幕、且屬系統 UI（類別或行程；桌面本身除外）的頂層視窗，
# 例如「Windows 安全性」的全螢幕 Shell_SystemDim。欄位讀不到一律往「是覆蓋層」那邊算（fail closed）；
# 只有確定不可見、確定 cloaked 或確定沒有覆蓋整個螢幕才排除。回傳空字串＝不是。
function Get-SystemOverlayReason {
    param([AllowNull()]$Window)
    if ($null -eq $Window) { return '覆蓋層資訊為 $null（無法判斷）' }
    if ((Get-FgProp $Window 'Visible') -eq $false) { return '' }
    if ((Get-FgProp $Window 'Cloaked') -eq $true) { return '' }
    if ((Get-FgProp $Window 'CoversMonitor') -eq $false) { return '' }
    $cls = [string](Get-FgProp $Window 'Class')
    $proc = [string](Get-FgProp $Window 'ProcessName')
    if (-not $cls -and -not $proc) { return '覆蓋整個螢幕的視窗類別與行程都查不到（無法判斷）' }
    $sys = Get-SystemUiReason -Class $cls -ProcessName $proc -AllowDesktop
    if ($sys) { return "$sys（class=$cls process=$proc）" }
    return ''
}

# fix F8b：前景是桌面（Progman／WorkerW）時的額外條件——該視窗必須屬殼層 explorer（GetShellWindow 的行程），
# 且沒有可見的系統 UI 覆蓋整個螢幕。Window 需另有 Pid、ShellPid、Overlays（候選頂層視窗清單）。
function Get-DesktopForegroundReason {
    param([AllowNull()]$Window)
    $shellPid = Get-FgProp $Window 'ShellPid'
    $fgPid = Get-FgProp $Window 'Pid'
    if ($null -eq $shellPid -or [int]$shellPid -eq 0) { return '前景是桌面類別，但殼層 explorer 行程查不到（無法確認桌面屬殼層）' }
    if ($null -eq $fgPid -or [int]$fgPid -ne [int]$shellPid) { return "前景是桌面類別，但不屬殼層 explorer（pid $fgPid，殼層 pid $shellPid）" }
    # 不經 Get-FgProp：函式回傳空陣列會被展開成 $null，「列舉成功、沒有候選」會被誤判成「列舉失敗」。
    $hasProp = $null -ne $Window -and $null -ne $Window.PSObject.Properties['Overlays']
    $overlays = if ($hasProp) { , $Window.Overlays } else { $null }
    if (-not $hasProp -or $null -eq $Window.Overlays) { return '前景是桌面，但系統覆蓋層列舉失敗（無法判斷有沒有系統 UI 覆蓋畫面）' }
    foreach ($o in @($overlays)) {
        $why = Get-SystemOverlayReason -Window $o
        if ($why) { return "前景是桌面，但有系統 UI 覆蓋整個螢幕：$why" }
    }
    return ''
}

function Get-ForegroundBlockReason {
    param([AllowNull()]$Window)
    if ($null -eq $Window) { return '前景查詢沒有結果（無法判斷）' }
    $hwnd = Get-FgProp $Window 'Hwnd'
    if ($null -eq $hwnd -or [IntPtr]$hwnd -eq [IntPtr]::Zero) { return 'GetForegroundWindow 回傳 NULL（無法判斷）' }
    $cls = [string](Get-FgProp $Window 'Class')
    $proc = [string](Get-FgProp $Window 'ProcessName')
    if (-not $cls) { return '前景類別查不到（無法判斷）' }
    if (-not $proc) { return '前景行程名查不到（無法判斷）' }
    $sys = Get-SystemUiReason -Class $cls -ProcessName $proc -AllowDesktop
    if ($sys) { return $sys }
    if ($cls -in $script:DesktopClasses) {
        $desk = Get-DesktopForegroundReason -Window $Window
        if ($desk) { return $desk }
    }
    $cloaked = Get-FgProp $Window 'Cloaked'
    $ex = Get-FgProp $Window 'ExStyle'
    if ($null -eq $cloaked) { return '前景 cloaked 狀態讀不到（無法判斷）' }
    if ($null -eq $ex) { return '前景延伸樣式讀不到（無法判斷）' }
    if ([bool]$cloaked) { return '前景是 DWM cloaked 視窗' }
    if (([long]$ex) -band $script:WS_EX_TOPMOST) { return '前景是 WS_EX_TOPMOST 視窗（系統覆蓋層常見樣式）' }
    return ''
}

Export-ModuleMember -Function Get-SystemUiReason, Get-ForegroundBlockReason
