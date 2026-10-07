<#
.SYNOPSIS
    Task 1.3 焦點與觸控探針的驅動腳本：啟動 probe_focus_touch、開自己的 WinForms 表單佔住前景／鍵盤焦點，
    自動模擬「點擊小工具」與「滾輪捲動小工具」，比對前後的前景視窗／鍵盤焦點是否改變、
    scrollTop 是否隨滾輪變化，把結果寫進 host/tools/evidence/1.3-*.log。

.DESCRIPTION
    可自動驗的三項（task-1.3-brief.md）：
      A  點擊小工具內容 → 前景視窗與（表單執行緒的）鍵盤焦點不變
      B  滾輪捲動小工具內容 → 探針記錄檔出現 scrollTop 變化，且前景／焦點仍不變
      C  Alt+Tab 判準、切換置底旗標後兩項樣式仍在 → 由 probe_focus_touch.exe 自己在啟動時
         做完並寫進它自己的記錄檔（本腳本只等待、不重做一次）

    人工項目（觸控板兩指捲動、觸控螢幕拖曳）不在本腳本內，見
    .superpowers/sdd/tasks/human-checklist.md「Task 1.3」節。

    前景／鍵盤焦點判斷：`GetForegroundWindow()` 取前景視窗；鍵盤焦點用
    `GetGUIThreadInfo(表單所在執行緒 id)` 的 `hwndFocus`——`GetFocus()` 只能查呼叫者自己
    執行緒的焦點，要查別的行程／執行緒必須用 `GetGUIThreadInfo`（Microsoft Learn）。

    工作階段鎖定時（LogonUI.exe 在跑）無法做實機驗證，腳本以結束碼 2 中止，且每次注入前都
    重新檢查一次（同 probe-1.1.ps1 的安全模式，不重試、不繞過）。

.EXAMPLE
    cd host; cargo build --release --example probe_focus_touch
    pwsh -File host/tools/probe-1.3.ps1
#>
[CmdletBinding()]
param(
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [string]$ExePath = (Join-Path $PSScriptRoot '..\target\release\examples\probe_focus_touch.exe'),
    [string]$Tag = ''
)

$ErrorActionPreference = 'Stop'

Add-Type -Namespace Probe13 -Name Native -MemberDefinition @'
[DllImport("user32.dll")] public static extern System.IntPtr SetThreadDpiAwarenessContext(System.IntPtr ctx);
[DllImport("user32.dll")] public static extern System.IntPtr GetForegroundWindow();
[DllImport("user32.dll")] public static extern bool SetForegroundWindow(System.IntPtr h);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr h, out uint p);
[DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
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
[DllImport("user32.dll")] public static extern bool GetGUIThreadInfo(uint idThread, ref GUITHREADINFO info);
[System.Runtime.InteropServices.StructLayout(System.Runtime.InteropServices.LayoutKind.Sequential)]
public struct POINT { public int X; public int Y; }
[System.Runtime.InteropServices.StructLayout(System.Runtime.InteropServices.LayoutKind.Sequential)]
public struct RECT2 { public int Left; public int Top; public int Right; public int Bottom; }
[System.Runtime.InteropServices.StructLayout(System.Runtime.InteropServices.LayoutKind.Sequential)]
public struct GUITHREADINFO {
    public int cbSize;
    public uint flags;
    public System.IntPtr hwndActive;
    public System.IntPtr hwndFocus;
    public System.IntPtr hwndCapture;
    public System.IntPtr hwndMenuOwner;
    public System.IntPtr hwndMoveSize;
    public System.IntPtr hwndCaret;
    public RECT2 rcCaret;
}
'@

# fix F9：本執行緒改為 Per-Monitor-V2（DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2＝-4，Microsoft Learn
# SetThreadDpiAwarenessContext；同 verify-4.7-touch.ps1 等腳本的慣例），必須在任何座標取得之前。
# probe_focus_touch 回報的矩形是實體像素；pwsh 執行緒預設 DPI unaware（awareness 0），WindowFromPoint／
# GetCursorPos／SafeInput 的游標與點擊座標都會被系統以虛擬化座標解讀——探針放在 175% 的筆電螢幕上時，
# 所有取樣點都沒打到小工具，誤報整個矩形被覆蓋（批次 B 第二次補跑）。設定後所有座標 API 同在 PMv2 下。
# 非零回傳＝設定成功（回傳的是前一個 context）。
$prevDpiCtx = [Probe13.Native]::SetThreadDpiAwarenessContext([IntPtr](-4))
Write-Host "SetThreadDpiAwarenessContext(PER_MONITOR_AWARE_V2) 前一個 context=0x$('{0:X}' -f $prevDpiCtx.ToInt64()) ok=$($prevDpiCtx -ne [IntPtr]::Zero)"

function Get-Cls([IntPtr]$h) { $sb = New-Object Text.StringBuilder 256; [void][Probe13.Native]::GetClassName($h, $sb, 256); $sb.ToString() }
function Get-WinPid([IntPtr]$h) { $p = 0; [void][Probe13.Native]::GetWindowThreadProcessId($h, [ref]$p); [int]$p }
function Get-WinThreadId([IntPtr]$h) { $p = 0; [Probe13.Native]::GetWindowThreadProcessId($h, [ref]$p) }
function Hex([IntPtr]$h) { '0x{0:X}' -f $h.ToInt64() }
function Stamp { Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fffK' }

# 查「表單執行緒目前的鍵盤焦點視窗」——GetFocus() 只能查呼叫者自己執行緒，跨行程／
# 執行緒必須用 GetGUIThreadInfo（Microsoft Learn：
# https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getguithreadinfo）。
function Get-ThreadFocus([uint32]$threadId) {
    $info = New-Object Probe13.Native+GUITHREADINFO
    $info.cbSize = [System.Runtime.InteropServices.Marshal]::SizeOf([type][Probe13.Native+GUITHREADINFO])
    if ([Probe13.Native]::GetGUIThreadInfo($threadId, [ref]$info)) {
        return @{ Active = $info.hwndActive; Focus = $info.hwndFocus }
    }
    return $null
}

$script:Steps = New-Object System.Collections.Generic.List[string]
function Step([string]$msg) { $line = "$(Stamp) STEP $msg"; $script:Steps.Add($line); Write-Host $line }

# 所有鍵盤／滑鼠注入（含游標移動）一律經 lib/SafeInput.psm1（task 1.1 fix round 1）：
# 每一次底層注入呼叫前重新檢查 LogonUI.exe，鎖定就丟 BLOCKED 並停止後續所有注入
# （不重試、不繞過）。本腳本不得自行宣告或呼叫注入 API。
Import-Module (Join-Path $PSScriptRoot 'lib\SafeInput.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\ScratchWindow.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force

function Get-TopWindows {
    $list = New-Object System.Collections.Generic.List[IntPtr]
    $cb = [Probe13.Native+EnumProc] { param($h, $l) $list.Add($h); $true }
    [void][Probe13.Native]::EnumWindows($cb, [IntPtr]::Zero)
    return $list
}

# 前景鎖（foreground lock）的常見繞法：先送一次 Alt 讓本行程成為「最後輸入來源」
# （同 probe-1.1.ps1 的 Set-Foreground）。
function Set-Foreground([IntPtr]$h) {
    Send-GuardedAltTap
    if ([Probe13.Native]::IsIconic($h)) { [void][Probe13.Native]::ShowWindow($h, 9) }
    return [Probe13.Native]::SetForegroundWindow($h)
}

# 點擊＝Send-GuardedClick；滾輪＝Send-GuardedWheel（每格 -120＝向下捲動，內容往上移、
# scrollTop 增加）。游標先移到目標座標——滑鼠滾輪訊息路由到游標下的視窗（Windows Vista
# 起「mouse wheel routing」：滾輪訊息送給游標下的視窗，不是鍵盤焦點視窗，這正是
# focusable(false)＋不搶焦點的小工具仍能被滾輪捲動的機制，design.md D8 risk 條目的判斷依據）。

# ---------------------------------------------------------------- main
# 開始前的前置探查（SafeInput fix round 3）：鎖定 → 結束碼 2（BLOCKED）；未鎖定但合成輸入沒被系統
# 計入 → 結束碼 3（ENV-BLOCKED，memory logonui-unlocked-but-synthetic-input-inert），不產生無資訊的 PASS/FAIL。
$pf = Invoke-SafeInputPreflight
if ($pf.ExitCode -ne 0) { Write-Host $pf.Message; exit $pf.ExitCode }
Write-Host $pf.Message
if (-not (Test-Path $ExePath)) { throw "找不到 $ExePath，先在 host/ 執行 cargo build --release --example probe_focus_touch" }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$sfx = if ($Tag) { "-$Tag" } else { '' }
$probeLog = Join-Path $OutDir "1.3-probe$sfx.log"
$probeInfo = Join-Path $OutDir "1.3-probe$sfx.json"
Remove-Item $probeInfo -ErrorAction SilentlyContinue

# 記下注入前的游標位置，結束後還原（controller 2026-09-28：不留痕跡在使用者的游標上）。
$origCursor = New-Object Probe13.Native+POINT
[void][Probe13.Native]::GetCursorPos([ref]$origCursor)

$probe = Start-Process $ExePath -ArgumentList @('--log', $probeLog, '--info', $probeInfo) -PassThru
for ($i = 0; $i -lt 60 -and -not (Test-Path $probeInfo); $i++) { Start-Sleep -Milliseconds 250 }
if (-not (Test-Path $probeInfo)) { throw 'probe_focus_touch 未寫出 info 檔' }
Start-Sleep -Seconds 1
$info = Get-Content $probeInfo -Raw | ConvertFrom-Json
$widget = [IntPtr]([Convert]::ToInt64($info.hwnd, 16))
$rect = $info.rect  # [x, y, width, height]，實體像素
Step "probe pid=$($probe.Id) widget=$($info.hwnd) rect=$($rect -join ',') dpiPmV2=$($prevDpiCtx -ne [IntPtr]::Zero)"

$form = $null
$normalWin = [IntPtr]::Zero
$exitCode = 0
try {
    # 一般視窗＝本腳本自己的 WinForms 表單（lib/ScratchWindow.psm1，含 TextBox 可取得鍵盤焦點）。
    # fix F7：不借用記事本——Win11 記事本是單一行程多視窗，以 PID 收尾會連使用者原本開著的記事本
    # 一起關掉（memory：win11-notepad-single-process-multi-window）。
    # 放在小工具正下方的桌面空白處（同一顯示器，不重疊小工具本身、也不動使用者其他視窗）。
    $form = Start-ScratchForm -Title ('fc-host-1.3-foreground-' + [guid]::NewGuid().ToString('N').Substring(0, 6)) `
        -X ([int]$rect[0]) -Y ([int]$rect[1] + [int]$rect[3] + 40) -Width 500 -Height 400
    $normalWin = $form.Hwnd
    [void](Set-Foreground $normalWin)
    Start-Sleep -Milliseconds 500
    $tid = Get-WinThreadId $normalWin
    Step "form=$(Hex $normalWin) pid=$($form.Process.Id) tid=$tid fg=$(Get-Cls ([Probe13.Native]::GetForegroundWindow()))"

    # ---- 基準：自己的表單為前景、鍵盤焦點在表單執行緒的某個視窗（其 TextBox）
    Assert-SessionUnlocked '情境開始'
    $baseFg = [Probe13.Native]::GetForegroundWindow()
    $baseFocus = Get-ThreadFocus $tid
    Step "BASELINE fg=$(Hex $baseFg)($(Get-Cls $baseFg)) form-thread-focus=$(Hex $baseFocus.Focus)($(Get-Cls $baseFocus.Focus))"
    # 前景被別的視窗佔住（例如系統對話框）是環境問題：ENV-BLOCKED（結束碼 3），不點擊、不判定。
    if ($baseFg -ne $normalWin) { throw "ENV-BLOCKED: 基準不成立：前景不是本腳本的表單（$(Hex $baseFg) $(Get-Cls $baseFg)），本次不點擊、不判定" }

    # ---- 前提：點擊/滾輪座標必須真的落在小工具本身（WindowFromPoint 的 root ancestor
    # pid＝小工具 pid），否則測到的是覆蓋在上面的其他視窗、不是小工具的行為（同 verify-6.1
    # 的 hittest 思路；task 1.3 首次實跑發現矩形中心可能被使用者其他視窗〔本機是一個 Edge
    # 分頁〕覆蓋，故補這個檢查，不是產品程式碼問題，是驗收腳本要先確認測到的是誰）。找不到
    # 乾淨點時（occluded）以結束碼 3（ENV-BLOCKED）結束、不點擊（fix F4；原本退回矩形中心照常點擊）。
    $widgetPid = Get-WinPid $widget
    function Find-ClearPointInRect([int]$rx, [int]$ry, [int]$rw, [int]$rh, [int]$targetPid) {
        for ($dy = 20; $dy -lt $rh; $dy += 20) {
            for ($dx = 20; $dx -lt $rw; $dx += 20) {
                $pt = New-Object Probe13.Native+POINT; $pt.X = $rx + $dx; $pt.Y = $ry + $dy
                $hAt = [Probe13.Native]::WindowFromPoint($pt)
                if ($hAt -eq [IntPtr]::Zero) { continue }
                $root = [Probe13.Native]::GetAncestor($hAt, 2)  # GA_ROOT
                if ((Get-WinPid $root) -eq $targetPid) { return @{ X = $pt.X; Y = $pt.Y } }
            }
        }
        return $null
    }
    $clear = Find-ClearPointInRect ([int]$rect[0]) ([int]$rect[1]) ([int]$rect[2]) ([int]$rect[3]) $widgetPid
    $occluded = -not $clear
    $cx = if ($clear) { $clear.X } else { [int]$rect[0] + [int]([int]$rect[2] / 2) }
    $cy = if ($clear) { $clear.Y } else { [int]$rect[1] + [int]([int]$rect[3] / 2) }
    Step "PRECHECK widgetPid=$widgetPid clearPoint=$(if ($clear) { "($($clear.X),$($clear.Y))" } else { '(none，整個矩形被覆蓋)' }) occluded=$occluded"
    # fix F4（review sectionA low）：整個矩形被覆蓋時不點擊、不送滾輪，直接以結束碼 3（ENV-BLOCKED）
    # 結束——原本退回矩形中心照常點擊並可能印出 PASS（無資訊 PASS），還會把點擊打進覆蓋的視窗。
    if ($occluded) { throw "ENV-BLOCKED: 整個小工具矩形被其他視窗覆蓋（occluded=True），本次不點擊、不判定；請清空該區域後重跑" }

    # ---- A：點擊小工具內容（未被覆蓋的座標，找不到才退回矩形中心），確認前景／鍵盤焦點不變
    Step "A: 點擊小工具內容 ($cx,$cy)"
    Send-GuardedClick $cx $cy
    Start-Sleep -Milliseconds 300
    $fgA = [Probe13.Native]::GetForegroundWindow()
    $focusA = Get-ThreadFocus $tid
    $passA = ($fgA -eq $baseFg) -and ($focusA.Focus -eq $baseFocus.Focus)
    Step "A: 結果 fg=$(Hex $fgA)($(Get-Cls $fgA)) form-thread-focus=$(Hex $focusA.Focus) PASS=$passA"

    # ---- B：滾輪捲動小工具內容，確認 scrollTop 有變、且前景／鍵盤焦點仍不變
    $beforeLines = (Get-Content $probeLog -ErrorAction SilentlyContinue | Where-Object { $_ -match 'SCROLL fragment=' }).Count
    Step "B: 滾輪捲動小工具內容 ($cx,$cy) x6，捲動前 SCROLL 行數=$beforeLines"
    Send-GuardedWheel $cx $cy 6
    Start-Sleep -Milliseconds 500
    $fgB = [Probe13.Native]::GetForegroundWindow()
    $focusB = Get-ThreadFocus $tid
    $afterLines = Get-Content $probeLog -ErrorAction SilentlyContinue | Where-Object { $_ -match 'SCROLL fragment=' }
    $lastFragment = if ($afterLines) { (@($afterLines) | Select-Object -Last 1) } else { '(none)' }
    $scrolled = $afterLines.Count -gt $beforeLines
    $passB = ($fgB -eq $baseFg) -and ($focusB.Focus -eq $baseFocus.Focus) -and $scrolled
    Step "B: 結果 fg=$(Hex $fgB)($(Get-Cls $fgB)) form-thread-focus=$(Hex $focusB.Focus) scrolled=$scrolled last=`"$lastFragment`" PASS=$passB"

    if (-not $passA) { Write-Host "FAIL: A（點擊搶走前景／焦點）" -ForegroundColor Red }
    if (-not $passB) { Write-Host "FAIL: B（滾輪未捲動，或搶走前景／焦點）" -ForegroundColor Red }
    if ($passA -and $passB) { Write-Host "PASS: A、B 皆通過 occluded=$occluded" -ForegroundColor Green }
}
catch {
    if ("$_" -like 'BLOCKED*') { Step "$_"; Write-Host "$_"; $exitCode = 2 }
    elseif ("$_" -like 'ENV-BLOCKED*') { Step "$_"; Write-Host "$_" -ForegroundColor Yellow; $exitCode = 3 }
    else { throw }
}
finally {
    # 只關自己的表單（WM_CLOSE 它的 HWND，必要時才結束自己啟動的那個 pwsh 行程）。
    try { Stop-ScratchForm $form } catch { Step "關閉表單失敗：$_" }
    if ($probe -and -not $probe.HasExited) { Stop-Process -Id $probe.Id -ErrorAction SilentlyContinue }
    # 還原游標位置（controller 2026-09-28）。游標移動也經 SafeInput；鎖定（或已停止）時就
    # 放棄還原，不在登入畫面上動游標。
    try { Set-GuardedCursorPos $origCursor.X $origCursor.Y -What '還原游標' } catch { Step "還原游標略過：$_" }
    $script:Steps | ConvertTo-EvidenceText | Set-Content -Path (Join-Path $OutDir "1.3-steps$sfx.log") -Encoding utf8
}
exit $exitCode
