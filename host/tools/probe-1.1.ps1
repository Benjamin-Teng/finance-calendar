<#
.SYNOPSIS
    Task 1.1 Win+D 探針的無人值守驅動腳本：啟動 probe_wind、開自己的 WinForms 表單（一般視窗）
    佔住小工具附近、以 watch-zorder.ps1 錄三段記錄檔，並自動模擬 Win+D／切換視窗／點擊桌面空白處。

.DESCRIPTION
    三段情境（每段各一個 watch-zorder 記錄檔，另有探針自己的事件記錄檔）：
      A  1.1-A-wind-toggle.log   表單在前景 → Win+D（進入）→ 停 8 秒 → 再按 Win+D（離開）
      B  1.1-B-wind-switch.log   Win+D（進入）→ 停 6 秒 → 切換到表單（離開）
      C  1.1-C-click-desktop.log 表單在前景 → 點擊桌面空白處 → 停 6 秒
    監控目標＝小工具 HWND、表單 HWND、殼層（explorer）擁有的所有頂層 Progman／WorkerW。
    各目標的 above 值出自同一次列舉，可直接比大小判斷相對順序（above 較小者在上）。

    工作階段鎖定時（LogonUI.exe 在跑）無法做實機桌面驗證，腳本以結束碼 2 中止。所有注入經
    lib/SafeInput.psm1，每一次底層注入呼叫前都重新檢查鎖定（task 1.1 fix round 1）。

.EXAMPLE
    cd host; cargo build --release --example probe_wind
    pwsh -File host/tools/probe-1.1.ps1
#>
[CmdletBinding()]
param(
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [string]$ExePath = (Join-Path $PSScriptRoot '..\target\release\examples\probe_wind.exe'),
    [ValidateSet('above-desktop', 'top')][string]$Insert = 'above-desktop',
    [ValidateSet('both', 'event', 'poll')][string]$Detect = 'both',
    [string]$Tag = ''
)

$ErrorActionPreference = 'Stop'

Add-Type -Namespace Probe11 -Name Native -MemberDefinition @'
[DllImport("user32.dll")] public static extern System.IntPtr SetThreadDpiAwarenessContext(System.IntPtr ctx);
[DllImport("user32.dll")] public static extern System.IntPtr GetForegroundWindow();
[DllImport("user32.dll")] public static extern bool SetForegroundWindow(System.IntPtr h);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr h, out uint p);
[DllImport("user32.dll")] public static extern System.IntPtr GetShellWindow();
[DllImport("user32.dll")] public static extern bool SetWindowPos(System.IntPtr h, System.IntPtr after, int x, int y, int cx, int cy, uint f);
[DllImport("user32.dll")] public static extern System.IntPtr WindowFromPoint(POINT p);
[DllImport("user32.dll")] public static extern System.IntPtr GetAncestor(System.IntPtr h, uint flags);
[DllImport("user32.dll", CharSet = System.Runtime.InteropServices.CharSet.Unicode)]
public static extern int GetClassName(System.IntPtr h, System.Text.StringBuilder b, int n);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(System.IntPtr h);
[DllImport("user32.dll")] public static extern bool IsIconic(System.IntPtr h);
[DllImport("user32.dll")] public static extern bool ShowWindow(System.IntPtr h, int cmd);
public delegate bool EnumProc(System.IntPtr h, System.IntPtr l);
[DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, System.IntPtr l);
[System.Runtime.InteropServices.StructLayout(System.Runtime.InteropServices.LayoutKind.Sequential)]
public struct POINT { public int X; public int Y; }
'@

# fix F9：Per-Monitor-V2（-4），在任何座標取得之前。WindowFromPoint、Screen.AllScreens 的工作區與 SafeInput
# 的點擊座標都依本執行緒的 DPI 感知解讀；pwsh 預設 unaware，混合 DPI 時取樣點會落在虛擬化後的錯誤位置。
[void][Probe11.Native]::SetThreadDpiAwarenessContext([IntPtr](-4))

function Get-Cls([IntPtr]$h) { $sb = New-Object Text.StringBuilder 256; [void][Probe11.Native]::GetClassName($h, $sb, 256); $sb.ToString() }
function Get-WinPid([IntPtr]$h) { $p = 0; [void][Probe11.Native]::GetWindowThreadProcessId($h, [ref]$p); [int]$p }
function Hex([IntPtr]$h) { '0x{0:X}' -f $h.ToInt64() }
function Stamp { Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fffK' }

$script:Steps = New-Object System.Collections.Generic.List[string]
function Step([string]$msg) { $line = "$(Stamp) STEP $msg"; $script:Steps.Add($line); Write-Host $line }

# 所有鍵盤／滑鼠注入一律經 lib/SafeInput.psm1（task 1.1 fix round 1）：它在**每一次**底層
# 注入呼叫前重新檢查 LogonUI.exe（LockApp 可能殘留，不用），鎖定就丟 BLOCKED 並停止後續所有
# 注入（不重試）。本腳本不得自行宣告或呼叫注入 API。
Import-Module (Join-Path $PSScriptRoot 'lib\SafeInput.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\ScratchWindow.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force

function Get-TopWindows {
    $list = New-Object System.Collections.Generic.List[IntPtr]
    $cb = [Probe11.Native+EnumProc] { param($h, $l) $list.Add($h); $true }
    [void][Probe11.Native]::EnumWindows($cb, [IntPtr]::Zero)
    return $list
}

# 前景鎖（foreground lock）的常見繞法：先送一次 Alt 讓本行程成為「最後輸入來源」。
function Set-Foreground([IntPtr]$h) {
    Send-GuardedAltTap
    # 顯示桌面會把視窗最小化；只 SetForegroundWindow 不會還原（run1 實測：前景是記事本但
    # minimized=1、仍在顯示桌面）。工作列點選／Alt+Tab 會還原視窗，這裡以 SW_RESTORE 模擬。
    if ([Probe11.Native]::IsIconic($h)) { [void][Probe11.Native]::ShowWindow($h, 9) }
    return [Probe11.Native]::SetForegroundWindow($h)
}

# 在主螢幕工作區找一個「最上層視窗屬於殼層 Progman／WorkerW」的點（桌面空白處），
# 由右下往左上掃，避開左側桌面圖示欄。
# 掃所有螢幕（不只主螢幕）：解鎖後實測發現主螢幕可能整台被使用者自己的一般視窗
# （例如最大化的編輯器）蓋滿，這是正常的使用者狀態，找不到桌面空白處時退到次螢幕找
# （同一份修正見 verify-3.4.ps1 的 Find-DesktopPoint）。
function Find-DesktopPoint([int[]]$exclude) {
    Add-Type -AssemblyName System.Windows.Forms
    $shellPid = Get-WinPid ([Probe11.Native]::GetShellWindow())
    foreach ($scr in [System.Windows.Forms.Screen]::AllScreens) {
        $wa = $scr.WorkingArea
        for ($y = $wa.Bottom - 60; $y -gt $wa.Top + 60; $y -= 40) {
            for ($x = $wa.Right - 60; $x -gt $wa.Left + 60; $x -= 40) {
                $pt = New-Object Probe11.Native+POINT; $pt.X = $x; $pt.Y = $y
                $h = [Probe11.Native]::WindowFromPoint($pt)
                if ($h -eq [IntPtr]::Zero) { continue }
                $root = [Probe11.Native]::GetAncestor($h, 2)  # GA_ROOT
                $cls = Get-Cls $root
                if (($cls -eq 'Progman' -or $cls -eq 'WorkerW') -and (Get-WinPid $root) -eq $shellPid) {
                    return @{ X = $x; Y = $y; Hit = (Get-Cls $h); Root = "$cls $(Hex $root)"; Screen = $scr.DeviceName }
                }
            }
        }
    }
    return $null
}

function Start-Watch([string]$name, [string[]]$hwnds, [int]$sec) {
    $out = Join-Path $OutDir $name
    $watch = Join-Path $PSScriptRoot 'watch-zorder.ps1'
    # 用 -Command 而非 -File：-File 模式下 "a,b,c" 會被當成單一字串傳給 [string[]] 參數
    $cmd = "& '$watch' -TargetHwnd $($hwnds -join ',') -OutFile '$out' -DurationSec $sec -IntervalMs 250 -BelowCount 8 -Quiet"
    $p = Start-Process pwsh -ArgumentList @('-NoProfile', '-Command', $cmd) -WindowStyle Hidden -PassThru
    Start-Sleep -Seconds 2   # 讓 baseline 先寫出
    return $p
}

# ---------------------------------------------------------------- main
# 開始前的前置探查（SafeInput fix round 3）：鎖定 → 結束碼 2（BLOCKED）；未鎖定但合成輸入沒被系統
# 計入 → 結束碼 3（ENV-BLOCKED，memory logonui-unlocked-but-synthetic-input-inert），不產生無資訊的 PASS/FAIL。
$pf = Invoke-SafeInputPreflight
if ($pf.ExitCode -ne 0) { Write-Host $pf.Message; exit $pf.ExitCode }
Write-Host $pf.Message
if (-not (Test-Path $ExePath)) { throw "找不到 $ExePath，先在 host/ 執行 cargo build --release --example probe_wind" }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$sfx = if ($Tag) { "-$Tag" } else { '' }
$probeLog = Join-Path $OutDir "1.1-probe$sfx.log"
$probeInfo = Join-Path $OutDir "1.1-probe$sfx.json"
Remove-Item $probeInfo -ErrorAction SilentlyContinue

$probe = Start-Process $ExePath -ArgumentList @('--log', $probeLog, '--info', $probeInfo, '--insert', $Insert, '--detect', $Detect) -PassThru
for ($i = 0; $i -lt 60 -and -not (Test-Path $probeInfo); $i++) { Start-Sleep -Milliseconds 250 }
if (-not (Test-Path $probeInfo)) { throw 'probe_wind 未寫出 info 檔' }
Start-Sleep -Seconds 2
$info = Get-Content $probeInfo -Raw | ConvertFrom-Json
$widget = [IntPtr]([Convert]::ToInt64($info.hwnd, 16))
Step "probe pid=$($probe.Id) widget=$($info.hwnd) rect=$($info.rect -join ',') insert=$Insert detect=$Detect"

$form = $null
$normalWin = [IntPtr]::Zero
$exitCode = 0
try {
# 一般視窗＝本腳本自己的 WinForms 表單（lib/ScratchWindow.psm1）。fix F7：不借用記事本——Win11
# 記事本是單一行程多視窗，以 PID 收尾會連使用者原本開著的記事本一起關掉。
# 讓表單蓋住小工具左半部（物理像素；Start-ScratchForm 以 SWP_NOZORDER|SWP_NOACTIVATE 擺位）。
$r = $info.rect
$form = Start-ScratchForm -Title ('fc-host-1.1-normal-' + [guid]::NewGuid().ToString('N').Substring(0, 6)) `
    -X ([int]$r[0] - 300) -Y ([int]$r[1] + 60) -Width 700 -Height ([int]$r[3])
$normalWin = $form.Hwnd
[void](Set-Foreground $normalWin)
Start-Sleep -Seconds 1
Step "form=$(Hex $normalWin) pid=$($form.Process.Id) fg=$(Get-Cls ([Probe11.Native]::GetForegroundWindow()))"

$shellPid = Get-WinPid ([Probe11.Native]::GetShellWindow())
$desk = @(Get-TopWindows | Where-Object { $c = Get-Cls $_; ($c -eq 'Progman' -or $c -eq 'WorkerW') -and (Get-WinPid $_) -eq $shellPid } | ForEach-Object { Hex $_ })
Step "shellPid=$shellPid desktopWindows=$($desk -join ',')"
$targets = @((Hex $widget), (Hex $normalWin)) + $desk

    # ---- A：Win+D 進入 → 8 秒 → Win+D 離開
    Assert-SessionUnlocked '情境開始'
    $w = Start-Watch "1.1-A-wind-toggle$sfx.log" $targets 22
    Step 'A: Win+D（進入顯示桌面）'
    Send-GuardedWinD
    Start-Sleep -Seconds 8
    Step 'A: Win+D（離開顯示桌面）'
    Send-GuardedWinD
    Start-Sleep -Seconds 6
    $w.WaitForExit()
    Step 'A: done'

    # ---- B：Win+D 進入 → 6 秒 → 切換到自己的表單
    [void](Set-Foreground $normalWin); Start-Sleep -Seconds 1
    Assert-SessionUnlocked '情境開始'
    $w = Start-Watch "1.1-B-wind-switch$sfx.log" $targets 18
    Step 'B: Win+D（進入顯示桌面）'
    Send-GuardedWinD
    Start-Sleep -Seconds 6
    $ok = Set-Foreground $normalWin
    Step "B: 切換到自己的表單 SetForegroundWindow=$ok fg=$(Get-Cls ([Probe11.Native]::GetForegroundWindow()))"
    Start-Sleep -Seconds 6
    $w.WaitForExit()
    Step 'B: done'

    # ---- C：自己的表單在前景 → 點擊桌面空白處
    [void](Set-Foreground $normalWin); Start-Sleep -Seconds 1
    Assert-SessionUnlocked '情境開始'
    Step "C: 前置 formIconic=$([int][Probe11.Native]::IsIconic($normalWin)) fg=$(Get-Cls ([Probe11.Native]::GetForegroundWindow()))"
    $pt = Find-DesktopPoint
    if (-not $pt) { throw '找不到可點擊的桌面空白處' }
    $w = Start-Watch "1.1-C-click-desktop$sfx.log" $targets 12
    Step "C: 點擊桌面空白處 ($($pt.X),$($pt.Y)) hit=$($pt.Hit) root=$($pt.Root)"
    Send-GuardedClick $pt.X $pt.Y
    Start-Sleep -Milliseconds 500
    Step "C: 點擊後 fg=$(Get-Cls ([Probe11.Native]::GetForegroundWindow()))"
    Start-Sleep -Seconds 6
    $w.WaitForExit()
    Step 'C: done'
}
catch {
    if ("$_" -like 'BLOCKED*') { Step "$_"; Write-Host "$_"; $exitCode = 2 } else { throw }
}
finally {
    # 只關自己的表單（WM_CLOSE 它的 HWND，必要時才結束自己啟動的那個 pwsh 行程）。
    try { Stop-ScratchForm $form } catch { Step "關閉表單失敗：$_" }
    Stop-Process -Id $probe.Id -ErrorAction SilentlyContinue
    $script:Steps | ConvertTo-EvidenceText | Set-Content -Path (Join-Path $OutDir "1.1-steps$sfx.log") -Encoding utf8
}
exit $exitCode
