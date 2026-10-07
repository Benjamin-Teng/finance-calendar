<#
.SYNOPSIS
    Task 1.2 Acrylic（毛玻璃）探針的驅動腳本：啟動 probe_acrylic（A／B 雙視窗）、把前景切到
    另一個自己開的視窗（自己的 WinForms 表單，不用模擬點擊／按鍵）使小工具失焦，以 per-monitor
    DPI aware 方式截圖存證，驗完把前景還給原本的視窗、關掉自己的表單與探針行程。

.DESCRIPTION
    步驟：
      1. 檢查工作階段未鎖定（LogonUI.exe）。
      2. 啟動 probe_acrylic.exe，等它寫出 info json（含 A／B 兩視窗的 hwnd／物理像素矩形）。
      3. 記下目前前景視窗（結束時要還原）。
      4. 開一個自己的 WinForms 表單（lib/ScratchWindow.psm1）、`SetForegroundWindow` 切過去（純 API 呼叫，不送任何合成輸入事件）；
         記錄切換前後的前景視窗類別，確認切換成功且不是探針視窗
         （探針視窗 `.focusable(false)` 本來就拿不到前景）。實測直接呼叫
         `SetForegroundWindow` 會被 Windows 的前景鎖擋下（呼叫端不是目前前景行程）；
         改用 `AttachThreadInput` 把本執行緒暫時併入目前前景視窗的輸入佇列再呼叫——這是
         Win32 說明文件記載的正規繞法（`AttachThreadInput` 本身不送任何合成輸入事件，
         純粹是訊息佇列關聯，符合「不要用模擬點擊／按鍵」的要求；與 probe-1.1.ps1 用
         `keybd_event(VK_MENU)` 的繞法不同，本腳本沒有送任何鍵盤／滑鼠事件）。
      5. 先呼叫 `SetThreadDpiAwarenessContext`（per-monitor v2）避免 DPI 虛擬化把座標／尺寸
         縮放成邏輯像素，再用 `Graphics.CopyFromScreen` 截兩張圖：
         - `1.2-laptop-fullscreen.png`：全螢幕，脈絡用（可看到表單在前景、小工具在桌面之上）。
         - `1.2-laptop-closeup.png`：只裁 A／B 兩視窗＋四周留白的近拍，方便判讀模糊／黑底／殘影。
      6. 還原前景視窗、關自己的表單、停探針行程。

    工作階段鎖定時（LogonUI.exe 在跑）直接以結束碼 2 中止，不做任何事。

.EXAMPLE
    cd host; cargo build --release --example probe_acrylic
    pwsh -File host/tools/probe-1.2.ps1
#>
[CmdletBinding()]
param(
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [string]$ExePath = (Join-Path $PSScriptRoot '..\target\release\examples\probe_acrylic.exe'),
    [int]$Monitor = 0,
    [ValidateSet('default', 'donotround', 'round', 'roundsmall')][string]$Corner = 'round',
    [ValidateSet('none', 'mainwindow', 'transientwindow', 'tabbedwindow')][string]$BackdropA = 'transientwindow',
    [ValidateSet('none', 'mainwindow', 'transientwindow', 'tabbedwindow')][string]$BackdropB = 'none',
    [Nullable[int]]$X = $null,
    [Nullable[int]]$Y = $null,
    [switch]$ForceActiveA,
    [string]$Tag = ''
)

$ErrorActionPreference = 'Stop'

Add-Type -Namespace Probe12 -Name Native -MemberDefinition @'
[DllImport("user32.dll")] public static extern System.IntPtr GetForegroundWindow();
[DllImport("user32.dll")] public static extern bool SetForegroundWindow(System.IntPtr h);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr h, out uint p);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(System.IntPtr h);
[DllImport("user32.dll")] public static extern bool IsIconic(System.IntPtr h);
[DllImport("user32.dll")] public static extern bool ShowWindow(System.IntPtr h, int cmd);
[DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
// IsHungAppWindow：只讀狀態、不送訊息。AttachThreadInput 到無回應的執行緒會讓本腳本的輸入狀態跟著卡住。
[DllImport("user32.dll")] public static extern bool IsHungAppWindow(System.IntPtr h);
// AttachThreadInput：把兩個執行緒的輸入佇列暫時併在一起，讓非前景行程也能呼叫
// SetForegroundWindow（Windows 的前景鎖只看「呼叫端是否與目前前景視窗共用輸入佇列」）。
// 不送任何合成鍵盤／滑鼠事件，只是訊息佇列關聯，做完立刻 detach。
[DllImport("user32.dll")] public static extern bool AttachThreadInput(uint idAttach, uint idAttachTo, bool fAttach);
[DllImport("user32.dll", CharSet = System.Runtime.InteropServices.CharSet.Unicode)]
public static extern int GetClassName(System.IntPtr h, System.Text.StringBuilder b, int n);
public delegate bool EnumProc(System.IntPtr h, System.IntPtr l);
[DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, System.IntPtr l);
[System.Runtime.InteropServices.StructLayout(System.Runtime.InteropServices.LayoutKind.Sequential)]
public struct POINT { public int X; public int Y; }
// 截圖前先查目標點目前是哪扇視窗在畫（z-order 頂端）：小工具 SetWindowPos(HWND_BOTTOM)
// 之後，若使用者現有的一般視窗（例如瀏覽器）剛好蓋在同一塊螢幕區域，小工具會被完全遮住，
// 截圖只會拍到蓋在上面的那扇視窗——這與「失焦」無關，是另一種必須先排除的干擾。
[DllImport("user32.dll")] public static extern System.IntPtr WindowFromPoint(POINT p);
[DllImport("user32.dll")] public static extern System.IntPtr GetAncestor(System.IntPtr h, uint flags);
// per-monitor DPI aware v2，只影響呼叫端這個執行緒（不像 SetProcessDpiAwarenessContext
// 只能設一次、pwsh 本身 manifest 已設過 System Aware 導致再設一次必失敗——實測
// SetProcessDpiAwarenessContext 在 pwsh 裡回傳 false；改用這個 per-thread 版本，
// 之後同一執行緒內 Screen.AllScreens／CopyFromScreen 才會拿到真正的實體像素）。
// Microsoft Learn: SetThreadDpiAwarenessContext，
// DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 = -4。
[DllImport("user32.dll")] public static extern System.IntPtr SetThreadDpiAwarenessContext(System.IntPtr value);
'@

$DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 = [IntPtr](-4)

function Get-Cls([IntPtr]$h) { $sb = New-Object Text.StringBuilder 256; [void][Probe12.Native]::GetClassName($h, $sb, 256); $sb.ToString() }
function Get-WinPid([IntPtr]$h) { $p = 0; [void][Probe12.Native]::GetWindowThreadProcessId($h, [ref]$p); [int]$p }
function Get-WinThreadId([IntPtr]$h) { $p = 0; [Probe12.Native]::GetWindowThreadProcessId($h, [ref]$p) }
function Hex([IntPtr]$h) { '0x{0:X}' -f $h.ToInt64() }
function Stamp { Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fffK' }

# 不送任何合成輸入事件的前景切換繞法：AttachThreadInput 到目前前景視窗的執行緒，
# 呼叫 SetForegroundWindow，再 detach。回傳 SetForegroundWindow 的結果。
# hung-target（review 642050f）：目前前景視窗（常是使用者的視窗）或目標無回應時不 attach、不切換，丟 ENV-BLOCKED（環境，結束碼 3；2 保留給工作階段鎖定）。
function Set-ForegroundNoInject([IntPtr]$target) {
    $fgNow = [Probe12.Native]::GetForegroundWindow()
    if ($fgNow -ne [IntPtr]::Zero -and [Probe12.Native]::IsHungAppWindow($fgNow)) {
        throw "ENV-BLOCKED: 目前前景視窗 $(Hex $fgNow) 無回應（IsHungAppWindow），不 AttachThreadInput"
    }
    if ($target -ne [IntPtr]::Zero -and [Probe12.Native]::IsHungAppWindow($target)) {
        throw "ENV-BLOCKED: 目標視窗 $(Hex $target) 無回應（IsHungAppWindow），不切換前景"
    }
    $fgTid = Get-WinThreadId $fgNow
    $myTid = [Probe12.Native]::GetCurrentThreadId()
    $attached = $false
    if ($fgTid -ne 0 -and $fgTid -ne $myTid) {
        $attached = [Probe12.Native]::AttachThreadInput($myTid, $fgTid, $true)
    }
    try {
        return [Probe12.Native]::SetForegroundWindow($target)
    }
    finally {
        if ($attached) { [void][Probe12.Native]::AttachThreadInput($myTid, $fgTid, $false) }
    }
}

$script:Steps = New-Object System.Collections.Generic.List[string]
function Step([string]$msg) { $line = "$(Stamp) STEP $msg"; $script:Steps.Add($line); Write-Host $line }

# 鎖定判斷一律看 LogonUI.exe；本腳本只呼叫 SetForegroundWindow（不送合成輸入事件），
# 但鎖定時截圖必為黑底＋鎖定畫面（見 memory: screen-capture-black-when-locked），做了也是白做，
# 一律先檢查、鎖定就整支腳本中止。
function Test-Locked {
    return [bool](Get-Process -Name LogonUI -ErrorAction SilentlyContinue)
}

if (Test-Locked) { Write-Host 'BLOCKED：LogonUI.exe 在跑（工作階段鎖定），無法做視覺驗收。'; exit 2 }
if (-not (Test-Path $ExePath)) { throw "找不到 $ExePath，先在 host/ 執行 cargo build --release --example probe_acrylic" }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

# 必須在任何螢幕座標查詢／截圖之前呼叫，否則本執行緒看到的座標／尺寸會被 DPI 虛擬化
# （非零回傳值＝前一個 context，代表設定成功；IntPtr::Zero＝失敗）。
$prevCtx = [Probe12.Native]::SetThreadDpiAwarenessContext($DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)
Step "SetThreadDpiAwarenessContext(PER_MONITOR_AWARE_V2) prevContext=$prevCtx ok=$($prevCtx -ne [IntPtr]::Zero)"

$sfx = if ($Tag) { "-$Tag" } else { '' }
$probeLog = Join-Path $OutDir "1.2-probe$sfx.log"
$probeInfo = Join-Path $OutDir "1.2-probe$sfx.json"
Remove-Item $probeInfo -ErrorAction SilentlyContinue

$probeArgs = @(
    '--log', $probeLog, '--info', $probeInfo, '--monitor', $Monitor,
    '--corner', $Corner, '--backdrop-a', $BackdropA, '--backdrop-b', $BackdropB
)
if ($null -ne $X) { $probeArgs += @('--x', $X) }
if ($null -ne $Y) { $probeArgs += @('--y', $Y) }
if ($ForceActiveA) { $probeArgs += @('--force-active-a', 'true') }
$probe = Start-Process $ExePath -ArgumentList $probeArgs -PassThru
for ($i = 0; $i -lt 160 -and -not (Test-Path $probeInfo); $i++) {
    Start-Sleep -Milliseconds 250
    if ($probe.HasExited) { throw "probe_acrylic 提早結束（exitCode=$($probe.ExitCode)），未寫出 info 檔；見 $probeLog" }
}
if (-not (Test-Path $probeInfo)) { throw 'probe_acrylic 40 秒內未寫出 info 檔' }
Start-Sleep -Seconds 1
$info = Get-Content $probeInfo -Raw | ConvertFrom-Json
Step "probe pid=$($probe.Id) buildNumber=$($info.buildNumber) buildSupportsBackdrop=$($info.buildSupportsBackdrop) corner=$($info.corner)"
Step "A hwnd=$($info.a.hwnd) backdrop=$($info.a.backdrop) rect=$($info.a.rect -join ',')"
Step "B hwnd=$($info.b.hwnd) backdrop=$($info.b.backdrop) rect=$($info.b.rect -join ',')"

$origForeground = [Probe12.Native]::GetForegroundWindow()
Step "原前景視窗 fg=$(Hex $origForeground) class=$(Get-Cls $origForeground) pid=$(Get-WinPid $origForeground)"

$ax, $ay, $aw, $ah = $info.a.rect | ForEach-Object { [int]$_ }
$bx, $by, $bw, $bh = $info.b.rect | ForEach-Object { [int]$_ }
$hwndA = [IntPtr]([Convert]::ToInt64($info.a.hwnd, 16))
$hwndB = [IntPtr]([Convert]::ToInt64($info.b.hwnd, 16))

# 遮擋檢查：小工具是 SetWindowPos(HWND_BOTTOM)，若使用者現有的一般視窗（例如瀏覽器）
# 剛好蓋在同一塊螢幕區域會把小工具完全蓋住——這與「失焦」無關，截圖前先查清楚，
# 避免拍到「其實是別的視窗、不是小工具」卻誤判成「毛玻璃呈現黑底／沒有材質」。
function Test-Occluded([int]$cx, [int]$cy, [IntPtr]$expected, [string]$label) {
    $pt = New-Object Probe12.Native+POINT
    $pt.X = $cx; $pt.Y = $cy
    $hit = [Probe12.Native]::WindowFromPoint($pt)
    $root = [Probe12.Native]::GetAncestor($hit, 2)  # GA_ROOT
    $isExpected = ($root -eq $expected)
    Step "遮擋檢查 $label point=($cx,$cy) hit=$(Hex $hit) root=$(Hex $root) class=$(Get-Cls $root) pid=$(Get-WinPid $root) 命中小工具本身=$isExpected"
    return $isExpected
}
$occlA = Test-Occluded ($ax + [int]($aw / 2)) ($ay + [int]($ah / 2)) $hwndA 'A'
$occlB = Test-Occluded ($bx + [int]($bw / 2)) ($by + [int]($bh / 2)) $hwndB 'B'
if (-not $occlA -or -not $occlB) {
    Step 'WARN 至少一扇小工具被其他一般視窗蓋住（見上一行 root class/pid），截圖會拍到蓋住它的視窗、不是小工具本身；建議換 --monitor 或 --x/--y 到目前沒有視窗佔用的區域再重跑'
}

Import-Module (Join-Path $PSScriptRoot 'lib\ScratchWindow.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force
$form = $null
$normalWin = [IntPtr]::Zero
$exitCode = 0
try {
    # 一般視窗＝本腳本自己的 WinForms 表單（lib/ScratchWindow.psm1）。fix F7：不借用記事本——Win11
    # 記事本是單一行程多視窗，以 PID 收尾會連使用者原本開著的記事本一起關掉。
    $form = Start-ScratchForm -Title ('fc-host-1.2-normal-' + [guid]::NewGuid().ToString('N').Substring(0, 6))
    $normalWin = $form.Hwnd
    Step "form=$(Hex $normalWin) pid=$($form.Process.Id)"

    # 無合成輸入事件的前景切換（見 Set-ForegroundNoInject），使小工具（本來就
    # focusable(false)、拿不到前景）明確處於「另一個應用程式在前景」狀態。
    $ok = Set-ForegroundNoInject $normalWin
    Start-Sleep -Milliseconds 400
    $fg = [Probe12.Native]::GetForegroundWindow()
    $fgClass = Get-Cls $fg
    Step "SetForegroundWindow(form)=$ok 切換後 fg=$(Hex $fg) class=$fgClass"
    if ($fg -ne $normalWin) {
        Step "WARN 前景不是本腳本的表單（class=$fgClass），仍繼續截圖，但失焦狀態可能未成立"
    }
    if ($fg -eq $hwndA -or $fg -eq $hwndB) {
        throw '探針視窗竟然拿到前景（不應發生，focusable(false) 失效？）'
    }

    Start-Sleep -Milliseconds 600  # 讓 DWM 動畫／材質有時間穩定

    Add-Type -AssemblyName System.Drawing
    Add-Type -AssemblyName System.Windows.Forms

    # ---- 脈絡截圖：只拍小工具所在那台螢幕，不拍整個虛擬桌面 ----
    # 刻意不用 SystemInformation.VirtualScreen（會把另一台螢幕也拍進去）：本機另一台
    # 螢幕當下可能顯示使用者實際在用的視窗內容（例如瀏覽器），與本次驗收無關卻可能是
    # 使用者的私人畫面，不應該被存進 repo 的證據檔。改用「小工具所在那台螢幕的完整
    # Bounds」，只在真的需要脈絡（例如判斷小工具是否被其他視窗蓋住）時才擴大到整台螢幕。
    $widgetScreen = [System.Windows.Forms.Screen]::AllScreens |
        Where-Object { $_.Bounds.Contains([System.Drawing.Point]::new($ax, $ay)) } |
        Select-Object -First 1
    if (-not $widgetScreen) { $widgetScreen = [System.Windows.Forms.Screen]::PrimaryScreen }
    $bounds = $widgetScreen.Bounds
    # ConvertFrom-Json 給的數字是 Int64；Bitmap(width,height) 只接受 Int32 多載，
    # 混用會讓 New-Object 找不到可綁定的建構子（"Parameter is not valid."），故全部顯式轉型。
    $fullW = [int]$bounds.Width
    $fullH = [int]$bounds.Height
    $full = New-Object System.Drawing.Bitmap $fullW, $fullH
    $g = [System.Drawing.Graphics]::FromImage($full)
    $g.CopyFromScreen([int]$bounds.Left, [int]$bounds.Top, 0, 0, $full.Size)
    $g.Dispose()
    $fullPath = Join-Path $OutDir "1.2-laptop-fullscreen$sfx.png"
    $full.Save($fullPath, [System.Drawing.Imaging.ImageFormat]::Png)
    $full.Dispose()
    Step "已存 $fullPath ($($fullW)x$($fullH) device=$($widgetScreen.DeviceName))"

    # ---- A／B 近拍（含四周留白，方便對照模糊／黑底/殘影）----
    $pad = 60
    $left = [int]([Math]::Min($ax, $bx) - $pad)
    $top = [int]([Math]::Min($ay, $by) - $pad)
    $right = [int]([Math]::Max($ax + $aw, $bx + $bw) + $pad)
    $bottom = [int]([Math]::Max($ay + $ah, $by + $bh) + $pad)
    $w = $right - $left
    $h = $bottom - $top
    $close = New-Object System.Drawing.Bitmap $w, $h
    $g2 = [System.Drawing.Graphics]::FromImage($close)
    $g2.CopyFromScreen($left, $top, 0, 0, $close.Size)
    $g2.Dispose()
    $closePath = Join-Path $OutDir "1.2-laptop-closeup$sfx.png"
    $close.Save($closePath, [System.Drawing.Imaging.ImageFormat]::Png)
    $close.Dispose()
    Step "已存 $closePath (裁切區域 x=$left y=$top w=$w h=$h)"
}
catch {
    if ("$_" -like 'BLOCKED*') { Step "$_"; Write-Host "$_"; $exitCode = 2 }
    elseif ("$_" -like 'ENV-BLOCKED*') { Step "$_"; Write-Host "$_"; $exitCode = 3 }
    else { throw }
}
finally {
    if ($origForeground -ne [IntPtr]::Zero -and [Probe12.Native]::IsWindowVisible($origForeground)) {
        try {
            $restored = Set-ForegroundNoInject $origForeground
            Step "還原前景視窗 fg=$(Hex $origForeground) SetForegroundWindow=$restored"
        }
        catch { Step "略過還原前景視窗：$_" }
    }
    else {
        Step "原前景視窗已不存在／不可見，略過還原"
    }
    # 只關自己的表單（WM_CLOSE 它的 HWND，必要時才結束自己啟動的那個 pwsh 行程）。
    try { Stop-ScratchForm $form } catch { Step "關閉表單失敗：$_" }
    Stop-Process -Id $probe.Id -ErrorAction SilentlyContinue
    $script:Steps | ConvertTo-EvidenceText | Set-Content -Path (Join-Path $OutDir "1.2-steps$sfx.log") -Encoding utf8
}
exit $exitCode
