<#
.SYNOPSIS
    Task 7.6 探針：在 `resizable(false)`（無 `WS_THICKFRAME`）的正式小工具視窗上，
    `getCurrentWindow().startResizeDragging(方向)` 能否進入系統尺寸迴圈、前景與焦點是否不變
    （design.md D7「調整大小的機制」、Risks 中 `startResizeDragging` 那條）。

.DESCRIPTION
    做法（正式組態的宿主，release 建置，capability 已含 `core:window:allow-start-resize-dragging`）：
      1. 暫存 %APPDATA%／%LOCALAPPDATA% 首次啟動宿主（CDP 埠），CDP `set_edit_mode(true)`。
      2. CDP 在 clock 頁面掛一個 window 捕獲階段的 `mousedown` 監聽器：擋掉 Tauri 內建
         `data-tauri-drag-region` 的 `startDragging`（stopImmediatePropagation），改呼叫
         `window.__TAURI__.window.getCurrentWindow().startResizeDragging(<方向>)`，結果／例外記在
         `window.__probe76`。等同正式把手的呼叫路徑（按下把手 → JS → `plugin:window|start_resize_dragging`
         → tao `PostMessage(WM_NCLBUTTONDOWN, HT*)`）。
      3. 經 SafeInput 在 clock 右緣內側按下、向右移動約兩格寬、放開。每一步取樣：視窗矩形、
         宿主 UI 執行緒 `GetGUIThreadInfo` 的 `flags & GUI_INMOVESIZE(0x2)` 與 `hwndMoveSize`
         （＝系統的 size/move 迴圈是否進行中、對象是哪扇視窗）、前景視窗、基準執行緒焦點。
      4. 方向依序測 East 與 SouthWest（左、下兩邊與角落把手的代表）。
    判定：按住期間 `GUI_INMOVESIZE` 且 `hwndMoveSize`＝clock、視窗大小跟著滑鼠改變＝進入尺寸迴圈
    （尺寸迴圈中系統逐步送 `WM_SIZING`；Microsoft Learn「WM_SIZING」「WM_ENTERSIZEMOVE」）。
    放開後宿主（7.5 程式）會把視窗依移動對齊彈回，本探針不判讀放開後的矩形。

    安全：開頭 `Invoke-SafeInputPreflight`（鎖定 → 2、合成輸入無效 → 3，直接結束）；所有注入經
    SafeInput（每次注入前查 LogonUI）。收尾各自 try：補放開、結束宿主、結束前景表單、還原被最小化
    的視窗、比對真正設定檔雜湊與 HKCU Run。遮擋（fix F8）：一律經 `lib/Occluders.psm1`（白名單，
    `Restore-Occluders` 還原含吸附與最大化）；遮擋者是工作列或不在白名單＝ENV-BLOCKED（結束碼 3）；
    按下點命中桌面、宿主的另一扇視窗或沒命中任何視窗＝小工具不在預期位置（判讀經 lib 的
    Assert-OccluderResult，fix F8b），記為「探針跑完（無例外）」NO。
#>
[CmdletBinding()]
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\fc-host.exe'),
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [string]$DataFile = 'D:\finance-calendar\tw_events.json',
    [int]$CdpPort = 9376,
    # 證據檔名後綴：第一輪（HEAD 0d6cc78 建置，編輯版面不切 WS_SIZEBOX）用 -no-sizebox，
    # 第二輪（task 7.6 退路：編輯版面由 desktop.rs 加上 WS_SIZEBOX）用 -sizebox。
    [string]$Suffix = ''
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force

Import-Module (Join-Path $PSScriptRoot 'lib\SafeInput.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\Occluders.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\VerifyVerdict.psm1') -Force

Add-Type -Namespace P76 -Name Native -MemberDefinition @'
[DllImport("user32.dll")] public static extern System.IntPtr SetThreadDpiAwarenessContext(System.IntPtr ctx);
[DllImport("user32.dll")] public static extern System.IntPtr GetTopWindow(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern System.IntPtr GetWindow(System.IntPtr hWnd, uint cmd);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr hWnd, out uint pid);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(System.IntPtr hWnd, System.Text.StringBuilder s, int n);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassName(System.IntPtr hWnd, System.Text.StringBuilder s, int n);
[DllImport("user32.dll")] public static extern bool GetWindowRect(System.IntPtr hWnd, out RECT r);
[DllImport("user32.dll")] public static extern System.IntPtr GetForegroundWindow();
[DllImport("user32.dll")] public static extern bool SetForegroundWindow(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern bool GetGUIThreadInfo(uint idThread, ref GUITHREADINFO info);
[DllImport("user32.dll")] public static extern System.IntPtr WindowFromPoint(POINT p);
[DllImport("user32.dll")] public static extern System.IntPtr GetAncestor(System.IntPtr h, uint flags);
[DllImport("user32.dll")] public static extern bool SetWindowPos(System.IntPtr h, System.IntPtr after, int x, int y, int cx, int cy, uint flags);
[DllImport("user32.dll", EntryPoint = "GetWindowLongPtrW")] public static extern System.IntPtr GetWindowLongPtr(System.IntPtr h, int idx);
[StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
[StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
[StructLayout(LayoutKind.Sequential)] public struct GUITHREADINFO {
  public int cbSize; public uint flags; public System.IntPtr hwndActive; public System.IntPtr hwndFocus;
  public System.IntPtr hwndCapture; public System.IntPtr hwndMenuOwner; public System.IntPtr hwndMoveSize;
  public System.IntPtr hwndCaret; public RECT rcCaret; }
[StructLayout(LayoutKind.Sequential)] public struct WINDOWPLACEMENT {
  public int length; public int flags; public int showCmd; public POINT ptMinPosition; public POINT ptMaxPosition;
  public RECT rcNormalPosition; }
'@
[void][P76.Native]::SetThreadDpiAwarenessContext([IntPtr](-4))

# fix F9：前置探查改在設定 DPI 感知之後（它以 GetWindowRect 與螢幕矩形判斷全螢幕覆蓋層，要與後續座標同一個
# awareness；tests/DpiAwareness.Tests.ps1 靜態檢查）。
$pf = Invoke-SafeInputPreflight
if ($pf.ExitCode -ne 0) { Write-Host $pf.Message; exit $pf.ExitCode }
Write-Host $pf.Message

function Get-Ts { Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fffK' }
function Hex([IntPtr]$h) { '0x{0:X}' -f $h.ToInt64() }
function Get-Cls([IntPtr]$h) { $sb = New-Object System.Text.StringBuilder 256; [void][P76.Native]::GetClassName($h, $sb, 256); $sb.ToString() }
function Get-WinPid([IntPtr]$h) { $p = 0; [void][P76.Native]::GetWindowThreadProcessId($h, [ref]$p); [int]$p }
function Get-WinTid([IntPtr]$h) { $p = 0; [int][P76.Native]::GetWindowThreadProcessId($h, [ref]$p) }
function Get-Rect([IntPtr]$h) {
    $r = New-Object P76.Native+RECT
    [void][P76.Native]::GetWindowRect($h, [ref]$r)
    [PSCustomObject]@{ X = $r.Left; Y = $r.Top; W = $r.Right - $r.Left; H = $r.Bottom - $r.Top }
}
function Fmt($r) { if ($r) { "($($r.X),$($r.Y),$($r.W)x$($r.H))" } else { '<null>' } }
function Find-Window([int]$ProcId, [string]$Title) {
    $h = [P76.Native]::GetTopWindow([IntPtr]::Zero)
    while ($h -ne [IntPtr]::Zero) {
        if ((Get-WinPid $h) -eq $ProcId -and [P76.Native]::IsWindowVisible($h)) {
            $sb = New-Object System.Text.StringBuilder 256
            [void][P76.Native]::GetWindowText($h, $sb, 256)
            if ($sb.ToString() -eq $Title) { return $h }
        }
        $h = [P76.Native]::GetWindow($h, 2)
    }
    return [IntPtr]::Zero
}
function Wait-Window([int]$ProcId, [string]$Title, [int]$TimeoutSec = 25) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt $TimeoutSec) {
        $h = Find-Window $ProcId $Title
        if ($h -ne [IntPtr]::Zero) { return $h }
        Start-Sleep -Milliseconds 200
    }
    return [IntPtr]::Zero
}
function Invoke-Eval([string]$Page, [string]$Expr) {
    $out = & node (Join-Path $PSScriptRoot 'host-cdp-eval.mjs') $CdpPort $Page $Expr 2>&1
    return ($out -join "`n").Trim()
}
function Get-ThreadInfo([uint32]$Tid) {
    $info = New-Object P76.Native+GUITHREADINFO
    $info.cbSize = [Runtime.InteropServices.Marshal]::SizeOf($info)
    [void][P76.Native]::GetGUIThreadInfo($Tid, [ref]$info)
    return $info
}

if (Get-Process -Name fc-host -ErrorAction SilentlyContinue) {
    Write-Host 'BLOCKED: 已有 fc-host 在執行，本腳本不結束它；請先自行關閉。'
    exit 2
}
$Exe = (Resolve-Path $Exe).Path
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$logPath = Join-Path $OutDir "7.6-probe-resize$Suffix.log"
$zorderPath = Join-Path $OutDir "7.6-probe-zorder$Suffix.log"
$log = New-EvidenceWriter $logPath
$log.AutoFlush = $true
function Log([string]$m) { $line = "$(Get-Ts) $m"; $log.WriteLine($line); Write-Host $line }
Log "# probe-7.6-resize.ps1 exe=$Exe preflight=$($pf.Message)"

$realSettings = Join-Path $env:APPDATA 'tw.fintools.fc-host\settings.json'
$realHashBefore = if (Test-Path $realSettings) { (Get-FileHash $realSettings).Hash } else { '<不存在>' }
Import-Module (Join-Path $PSScriptRoot 'lib\AutostartRegistry.psm1') -Force
$regSnap = @(Save-FcHostAutostartRegistry)
Log "# 真正的設定檔雜湊（開始）=$realHashBefore；開機自啟登錄快照：$(($regSnap | ForEach-Object { "$($_.Name)@$(Split-Path $_.Key -Leaf) Exists=$($_.Exists)" }) -join '; ')"

$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ('fc-host-7.6p-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
$tempAppData = Join-Path $tempRoot 'Roaming'
$tempLocal = Join-Path $tempRoot 'Local'
$dataDir = Join-Path $tempLocal 'tw.fintools.fc-host\data'
New-Item -ItemType Directory -Force -Path $tempAppData, $dataDir | Out-Null
if (Test-Path $DataFile) { Copy-Item $DataFile (Join-Path $dataDir 'tw_events.json') }

$script:mouseDown = $false
$hostProc = $null; $fgProc = $null; $zorderProc = $null
$minimized = New-Object System.Collections.Generic.List[object]
# hung-target（review 642050f）：沒還原成功的使用者視窗（無回應、最小化未確認、讀回逾時／不符）。屬環境問題，
# 不進逐項結果；摘要另起警示行（Format-UnrestoredWarning），結束碼經 Get-VerdictExitCode（環境＝3）。
$occNotRestored = New-Object System.Collections.Generic.List[string]
$verdicts = [ordered]@{}
$script:envBlocked = $null

try {
    Assert-SessionUnlocked '探針前置檢查'
    $zorderProc = Start-Process pwsh -PassThru -WindowStyle Hidden -ArgumentList @(
        '-NoProfile', '-File', (Join-Path $PSScriptRoot 'watch-zorder.ps1'), '-ProcessName', 'fc-host',
        '-OutFile', $zorderPath, '-DurationSec', '120', '-IntervalMs', '200', '-Quiet')
    Start-Sleep -Seconds 2

    $old = @($env:APPDATA, $env:LOCALAPPDATA, $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS)
    try {
        $env:APPDATA = $tempAppData; $env:LOCALAPPDATA = $tempLocal
        $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$CdpPort"
        $hostProc = Start-Process -FilePath $Exe -PassThru
    } finally { $env:APPDATA, $env:LOCALAPPDATA, $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $old }
    Log "# 宿主 pid=$($hostProc.Id)"
    $clock = Wait-Window $hostProc.Id 'fc-host clock'
    if ($clock -eq [IntPtr]::Zero) { throw 'clock 視窗未出現' }
    Start-Sleep -Seconds 3
    $hostTid = [uint32](Get-WinTid $clock)
    $style = [P76.Native]::GetWindowLongPtr($clock, -16).ToInt64()
    Log ("## clock hwnd=$(Hex $clock) UI 執行緒=$hostTid GWL_STYLE=0x{0:X8} WS_THICKFRAME(0x40000)={1}" -f $style, [bool]($style -band 0x40000))
    $verdicts['前置：正式組態的 clock 沒有 WS_THICKFRAME'] = -not [bool]($style -band 0x40000)

    # 前景基準視窗（左下角）。
    $fgTitle = 'fc-host-7.6p-foreground-' + [guid]::NewGuid().ToString('N').Substring(0, 6)
    $formCmd = "Add-Type -AssemblyName System.Windows.Forms; `$f = New-Object Windows.Forms.Form; `$f.Text = '$fgTitle'; `$t = New-Object Windows.Forms.TextBox; `$t.Multiline = `$true; `$t.Dock = 'Fill'; `$f.Controls.Add(`$t); `$f.Show(); `$f.Hide(); [Windows.Forms.Application]::Run(`$f)"
    $fgProc = Start-Process pwsh -PassThru -WindowStyle Hidden -ArgumentList @('-NoProfile', '-Command', $formCmd)
    $fgHwnd = Wait-Window $fgProc.Id $fgTitle
    if ($fgHwnd -eq [IntPtr]::Zero) { throw '前景基準視窗未出現' }
    [void][P76.Native]::SetWindowPos($fgHwnd, [IntPtr]::Zero, 20, 1100, 420, 220, 0x0014)

    Invoke-Eval 'w=clock' "window.__TAURI__.core.invoke('set_edit_mode', { enabled: true })" | Out-Null
    Start-Sleep -Seconds 1
    Log "## 進入編輯版面：layout_locked=$(Invoke-Eval 'w=clock' "window.__TAURI__.core.invoke('get_settings').then(s => s.layout_locked)")"
    $styleEdit = [P76.Native]::GetWindowLongPtr($clock, -16).ToInt64()
    Log ("## 編輯版面中 clock GWL_STYLE=0x{0:X8} WS_THICKFRAME={1}" -f $styleEdit, [bool]($styleEdit -band 0x40000))

    # 對照組 Move：同一個掛鉤改呼叫 startDragging()（7.5 已驗證可進入移動迴圈），確認本探針的
    # 量測方法本身看得到 size/move 迴圈；East／SouthWest 才是待驗的 startResizeDragging。
    foreach ($case in @(
            @{ Dir = 'Move'; Dx = 1; Dy = 0 },
            @{ Dir = 'East'; Dx = 1; Dy = 0 },
            @{ Dir = 'SouthWest'; Dx = -1; Dy = 1 })) {
        $dir = $case.Dir
        $r0 = Get-Rect $clock
        # 按下點：該方向對應的邊緣內側 6px（East＝右緣中段；SouthWest＝左下角）。
        $px = if ($case.Dx -gt 0) { $r0.X + $r0.W - 6 } elseif ($case.Dx -lt 0) { $r0.X + 6 } else { $r0.X + [int]($r0.W / 2) }
        $py = if ($case.Dy -gt 0) { $r0.Y + $r0.H - 6 } elseif ($case.Dy -lt 0) { $r0.Y + 6 } else { $r0.Y + [int]($r0.H / 2) }
        $dist = [int]($r0.W / 8)   # 約兩格（clock 寬 16 格）
        # 遮擋（fix F8）：一律經 lib/Occluders.psm1（白名單；系統 UI、對話框、工作列等不動 → ENV-BLOCKED）。
        # fix F8b：判讀經 lib 的 Assert-OccluderResult——命中桌面或沒命中任何視窗＝小工具不在預期位置（FAIL）。
        $ownPids = if ($fgProc) { @($fgProc.Id) } else { @() }
        $occ = Clear-Occluders -HostPid $hostProc.Id -Points @(, @($px, $py)) -Minimized $minimized -OwnPids $ownPids -Log { param($m) Log "## $m" }
        Assert-OccluderResult -Result $occ -What "[$dir] 按下點"

        $call = if ($dir -eq 'Move') { "window.__TAURI__.window.getCurrentWindow().startDragging()" } else { "window.__TAURI__.window.getCurrentWindow().startResizeDragging('$dir')" }
        $hook = "(() => { window.__probe76 = []; if (window.__probe76h) window.removeEventListener('mousedown', window.__probe76h, true); window.__probe76h = (e) => { e.stopImmediatePropagation(); e.preventDefault(); $call.then(() => window.__probe76.push('ok'), (err) => window.__probe76.push('err:' + err)); }; window.addEventListener('mousedown', window.__probe76h, true); return 'hooked'; })()"
        Log "## [$dir] 掛 mousedown→startResizeDragging：$(Invoke-Eval 'w=clock' $hook)"

        Send-GuardedAltTap
        [void][P76.Native]::SetForegroundWindow($fgHwnd)
        Start-Sleep -Milliseconds 500
        $baseFg = [P76.Native]::GetForegroundWindow()
        $fgTid = [uint32](Get-WinTid $fgHwnd)
        $baseFocus = (Get-ThreadInfo $fgTid).hwndFocus
        Log "## [$dir] 基準 fg=$(Hex $baseFg)（期望 $(Hex $fgHwnd)）focus=$(Hex $baseFocus) 起始矩形=$(Fmt $r0) 按下點=($px,$py)"

        $samples = New-Object System.Collections.Generic.List[object]
        Set-GuardedCursorPos $px $py -What "[$dir] 起點"
        Invoke-SafeInputSleep 150
        $script:mouseDown = $true
        Invoke-GuardedMouse 2 -What "[$dir] 按下（LEFTDOWN）"
        Invoke-SafeInputSleep 400
        $steps = 12
        for ($s = 0; $s -le $steps; $s++) {
            if ($s -gt 0) {
                $x = $px + [int][math]::Round($case.Dx * $dist * $s / $steps)
                $y = $py + [int][math]::Round($case.Dy * $dist * $s / $steps)
                Set-GuardedCursorPos $x $y -What "[$dir] 拖曳中（$s/$steps）"
                Invoke-SafeInputSleep 60
            }
            $ti = Get-ThreadInfo $hostTid
            $fti = Get-ThreadInfo $fgTid
            $rr = Get-Rect $clock
            $samples.Add([PSCustomObject]@{ Step = $s; InMoveSize = [bool]($ti.flags -band 2); MoveSize = $ti.hwndMoveSize; Rect = $rr; Fg = [P76.Native]::GetForegroundWindow(); Focus = $fti.hwndFocus })
            Log "##   [$dir] 步 $s：rect=$(Fmt $rr) GUI_INMOVESIZE=$([bool]($ti.flags -band 2)) hwndMoveSize=$(Hex $ti.hwndMoveSize) fg=$(Hex ([P76.Native]::GetForegroundWindow())) focus=$(Hex $fti.hwndFocus)"
        }
        $held = Get-Rect $clock
        Invoke-GuardedMouse 4 -What "[$dir] 放開（LEFTUP）"
        $script:mouseDown = $false
        Start-Sleep -Milliseconds 1200
        $after = Get-Rect $clock
        $jsLog = Invoke-Eval 'w=clock' "window.__probe76.join(' | ')"
        Log "## [$dir] 按住最後矩形=$(Fmt $held) 放開後=$(Fmt $after) JS 呼叫結果=$jsLog"

        $inLoop = @($samples | Where-Object { $_.Step -gt 0 -and $_.InMoveSize -and $_.MoveSize -eq $clock }).Count
        $sizeChanged = if ($dir -eq 'Move') { ($held.X -ne $r0.X -or $held.Y -ne $r0.Y) } else { ($held.W -ne $r0.W -or $held.H -ne $r0.H) }
        $fgBad = @($samples | Where-Object { $_.Fg -ne $baseFg -or $_.Focus -ne $baseFocus }).Count
        $verdicts["[$dir] JS 呼叫成功（capability 生效）"] = ($jsLog -match 'ok')
        $verdicts["[$dir] 按住期間進入 size/move 迴圈（GUI_INMOVESIZE 且 hwndMoveSize＝clock，$inLoop/$steps 次）"] = ($inLoop -gt 0)
        $verdicts["[$dir] 視窗（Move＝位置，其餘＝大小）跟著滑鼠改變（$(Fmt $r0) → $(Fmt $held)）"] = $sizeChanged
        $verdicts["[$dir] 前景與焦點全程不變（取樣 $($samples.Count) 次，不同 $fgBad 次）"] = ($fgBad -eq 0)
        Invoke-Eval 'w=clock' "(() => { window.removeEventListener('mousedown', window.__probe76h, true); return 'unhooked'; })()" | Out-Null
        Start-Sleep -Milliseconds 800
    }
}
catch {
    if ("$($_.Exception.Message)" -like 'ENV-BLOCKED*') {
        $script:envBlocked = $_.Exception.Message
        Log "## $($script:envBlocked)（環境問題，結果不完整）"
    } else {
        Log "## 例外中止：$($_.Exception.Message)"
        $verdicts['探針跑完（無例外）'] = $false
    }
}
finally {
    if ($script:mouseDown) { try { Invoke-GuardedMouse 4 -What '收尾：補放開（LEFTUP）' } catch { Log "## 收尾補放開失敗：$_" } }
    try {
        if ($hostProc -and -not $hostProc.HasExited) { Stop-Process -Id $hostProc.Id -Force; $hostProc.WaitForExit(10000) | Out-Null }
        $leaf = Split-Path $tempRoot -Leaf
        Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" -ErrorAction SilentlyContinue |
            Where-Object { $_.CommandLine -and $_.CommandLine -match [regex]::Escape($leaf) } |
            ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
        Start-Sleep -Seconds 2
    } catch { Log "## 收尾結束宿主失敗：$_" }
    try { if ($fgProc -and -not $fgProc.HasExited) { Stop-Process -Id $fgProc.Id -Force } } catch { Log "## 收尾結束前景表單失敗：$_" }
    try { Restore-Occluders -Minimized $minimized -NotRestored $occNotRestored -Log { param($m) Log "## $m" } } catch { Log "## 還原被最小化的視窗失敗：$_" }
    # $minimized 非空＝Restore-Occluders 本身中途丟例外、沒跑完；一併列為未還原（環境，不進逐項結果）。
    if ($minimized.Count -gt 0) { $occNotRestored.Add("Restore-Occluders 未跑完：尚有 $($minimized.Count) 扇未處理（見記錄）") }
    try { if ($zorderProc -and -not $zorderProc.HasExited) { Stop-Process -Id $zorderProc.Id -Force } } catch { Log "## 收尾 watch-zorder 失敗：$_" }
    $regLeft = @(Restore-FcHostAutostartRegistry $regSnap)
    if ($regLeft.Count -gt 0) { Write-Warning "開機自啟登錄未還原：$($regLeft -join '; ')" }
    Log "# 開機自啟登錄還原：未還原 $($regLeft.Count) 項$(if ($regLeft.Count) { '：' + ($regLeft -join '; ') })"
    $verdicts['開機自啟登錄已還原（Run／StartupApproved 的 fc-host）'] = ($regLeft.Count -eq 0)
    $realHashAfter = if (Test-Path $realSettings) { (Get-FileHash $realSettings).Hash } else { '<不存在>' }
    Log "# 真正的設定檔雜湊（結束）=$realHashAfter；與開始相同=$($realHashAfter -eq $realHashBefore)"
    $verdicts['隔離：真正的設定檔雜湊前後相同'] = ($realHashAfter -eq $realHashBefore)
    foreach ($k in $verdicts.Keys) { Log "$(if ($verdicts[$k]) { 'YES' } else { 'NO ' })  $k" }
    $occWarn = Format-UnrestoredWarning -NotRestored $occNotRestored
    if ($occWarn) { Log $occWarn }
    Log '# 結束'
    $log.Close()
    Remove-Item -Recurse -Force $tempRoot -ErrorAction SilentlyContinue
}
if ($occWarn) { Write-Host $occWarn -ForegroundColor Yellow }
if ($script:envBlocked) { Write-Host $script:envBlocked }
exit (Get-VerdictExitCode -EnvBlocked:([bool]$script:envBlocked) -NotRestored $occNotRestored)
