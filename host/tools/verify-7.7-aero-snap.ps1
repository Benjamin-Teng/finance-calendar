<#
.SYNOPSIS
    Task 7.7 追加驗收：編輯版面調整大小時的 Aero Snap（垂直最大化）殘留（task-7.6-report.md 疑慮 1；
    design.md D7「調整大小的機制」）。經 `lib/SafeInput.psm1` 真實注入。

.DESCRIPTION
    編輯版面期間小工具有 `WS_THICKFRAME`（7.6 退路），把上緣或下緣拖到螢幕頂端／底端、或雙擊上緣，
    系統可能套用「垂直最大化」吸附。每一項放開後檢查：
      - 視窗矩形＝由目前記錄（`get_settings` 的 col,row,w,h）推導的格線矩形；
      - `GetWindowPlacement`：showCmd＝SW_SHOWNORMAL（1）、`IsZoomed`＝否，且 rcNormalPosition＝
        目前矩形（小工具帶 WS_EX_TOOLWINDOW，rcNormalPosition 為螢幕座標；系統吸附後這個「還原
        矩形」會與實際矩形不同，是吸附狀態殘留的特徵）；
      - 離開再進入編輯版面一次後矩形仍＝推導矩形。

    步驟（主螢幕、全新暫存設定＝預設版面）：
      S1. clock（(15,1,16,10)，上方沒有鄰居）上緣把手拖到螢幕最上緣（y＝螢幕頂端）停留後放開。
      S2. quotes（(15,43,32,4)，下緣＝工作區底）下緣把手拖到螢幕最下緣（工作列上）停留後放開
          ——超出工作區，應判不合法並彈回。
      S3. 雙擊 clock 上緣把手。

    安全與收尾同 verify-7.6：開頭 preflight（鎖定→2、合成輸入無效→3，不產生 PASS/FAIL）、
    finally 各自 try（補放開、結束宿主、還原被最小化的使用者視窗、HKCU Run、真正設定檔雜湊）。
    記錄檔路徑以 %TEMP% 等字樣輸出。遮擋（fix F8）：一律經 `lib/Occluders.psm1`，只最小化白名單內
    的一般應用程式主視窗、finally 以 `Restore-Occluders` 還原（含吸附與最大化）；遮擋者是工作列或
    不在白名單（系統 UI、對話框等）＝ENV-BLOCKED（結束碼 3）。按下點命中桌面（Progman／WorkerW）或
    沒命中任何視窗＝小工具不在預期位置或沉到桌面之下＝FAIL（fix F8b，判讀經 lib 的 Assert-OccluderResult）。

.PARAMETER Exe
    fc-host.exe 路徑，預設 host/target/release/fc-host.exe（**不含** self-test-ipc）。
#>
[CmdletBinding()]
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\fc-host.exe'),
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [string]$DataFile = 'D:\finance-calendar\tw_events.json',
    [int]$CdpPort = 9381,
    [string]$Suffix = '',
    # 探針：進入編輯版面後由腳本拿掉 clock／quotes 的 WS_MAXIMIZEBOX，驗證吸附是否因此停用。
    [switch]$ProbeNoMaximizeBox
)

$ErrorActionPreference = 'Stop'

Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\AutostartRegistry.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\SafeInput.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\Occluders.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\VerifyVerdict.psm1') -Force

Add-Type -Namespace V77S -Name Native -MemberDefinition @'
[DllImport("user32.dll")] public static extern System.IntPtr SetThreadDpiAwarenessContext(System.IntPtr ctx);
[DllImport("user32.dll")] public static extern System.IntPtr GetTopWindow(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern System.IntPtr GetWindow(System.IntPtr hWnd, uint cmd);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern bool IsZoomed(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern bool IsWindowArranged(System.IntPtr hWnd);
[DllImport("user32.dll", EntryPoint = "GetWindowLongPtrW")] public static extern System.IntPtr GetWindowLongPtr(System.IntPtr h, int idx);
[DllImport("user32.dll", EntryPoint = "SetWindowLongPtrW")] public static extern System.IntPtr SetWindowLongPtr(System.IntPtr h, int idx, System.IntPtr v);
[DllImport("user32.dll")] public static extern bool SetWindowPos(System.IntPtr h, System.IntPtr after, int x, int y, int cx, int cy, uint flags);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr hWnd, out uint pid);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(System.IntPtr hWnd, System.Text.StringBuilder s, int n);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassName(System.IntPtr hWnd, System.Text.StringBuilder s, int n);
[DllImport("user32.dll")] public static extern bool GetWindowRect(System.IntPtr hWnd, out RECT r);
[DllImport("user32.dll")] public static extern bool GetGUIThreadInfo(uint idThread, ref GUITHREADINFO info);
[DllImport("user32.dll")] public static extern System.IntPtr WindowFromPoint(POINT p);
[DllImport("user32.dll")] public static extern System.IntPtr GetAncestor(System.IntPtr h, uint flags);
[DllImport("user32.dll")] public static extern bool GetWindowPlacement(System.IntPtr h, ref WINDOWPLACEMENT wp);
[DllImport("user32.dll")] public static extern System.IntPtr MonitorFromPoint(POINT pt, uint flags);
[DllImport("user32.dll")] public static extern bool GetMonitorInfoW(System.IntPtr hMon, ref MONITORINFO mi);
[StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
[StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
[StructLayout(LayoutKind.Sequential)] public struct MONITORINFO { public int cbSize; public RECT rcMonitor; public RECT rcWork; public uint dwFlags; }
[StructLayout(LayoutKind.Sequential)] public struct GUITHREADINFO {
  public int cbSize; public uint flags; public System.IntPtr hwndActive; public System.IntPtr hwndFocus;
  public System.IntPtr hwndCapture; public System.IntPtr hwndMenuOwner; public System.IntPtr hwndMoveSize;
  public System.IntPtr hwndCaret; public RECT rcCaret; }
[StructLayout(LayoutKind.Sequential)] public struct WINDOWPLACEMENT {
  public int length; public int flags; public int showCmd; public POINT ptMinPosition; public POINT ptMaxPosition;
  public RECT rcNormalPosition; }
'@

[void][V77S.Native]::SetThreadDpiAwarenessContext([IntPtr](-4))

# fix F9：前置探查改在設定 DPI 感知之後（它以 GetWindowRect 與螢幕矩形判斷全螢幕覆蓋層，要與後續座標同一個
# awareness；tests/DpiAwareness.Tests.ps1 靜態檢查）。
$pf = Invoke-SafeInputPreflight
if ($pf.ExitCode -ne 0) { Write-Host $pf.Message; exit $pf.ExitCode }
Write-Host $pf.Message

$GRID = 48
function Get-Ts { Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fffK' }
function Hex([IntPtr]$h) { '0x{0:X}' -f $h.ToInt64() }
function Get-Cls([IntPtr]$h) { $sb = New-Object System.Text.StringBuilder 256; [void][V77S.Native]::GetClassName($h, $sb, 256); $sb.ToString() }
function Get-WinPid([IntPtr]$h) { $p = 0; [void][V77S.Native]::GetWindowThreadProcessId($h, [ref]$p); [int]$p }
function Get-WinTid([IntPtr]$h) { $p = 0; [int][V77S.Native]::GetWindowThreadProcessId($h, [ref]$p) }
function Get-Rect([IntPtr]$h) {
    $r = New-Object V77S.Native+RECT
    [void][V77S.Native]::GetWindowRect($h, [ref]$r)
    [PSCustomObject]@{ X = $r.Left; Y = $r.Top; W = $r.Right - $r.Left; H = $r.Bottom - $r.Top }
}
function Fmt($r) { if ($r) { "($($r.X),$($r.Y),$($r.W)x$($r.H))" } else { '<null>' } }
function Same($a, $b) { $a -and $b -and $a.X -eq $b.X -and $a.Y -eq $b.Y -and $a.W -eq $b.W -and $a.H -eq $b.H }
function Edge([int]$Origin, [int]$Extent, [int]$I) { $Origin + [int][math]::Floor([int64]$I * $Extent / $GRID) }
function Grid-Rect($Wa, [int]$Col, [int]$Row, [int]$W, [int]$H) {
    $x0 = Edge $Wa.X $Wa.W $Col; $x1 = Edge $Wa.X $Wa.W ($Col + $W)
    $y0 = Edge $Wa.Y $Wa.H $Row; $y1 = Edge $Wa.Y $Wa.H ($Row + $H)
    [PSCustomObject]@{ X = $x0; Y = $y0; W = $x1 - $x0; H = $y1 - $y0 }
}
# IsWindowArranged（winuser.h，文件化）只用來佐證「系統吸附狀態」；舊版 Windows 沒有這個匯出時回傳 $null。
function Get-Arranged([IntPtr]$h) { try { [V77S.Native]::IsWindowArranged($h) } catch { $null } }
function Get-Placement([IntPtr]$h) {
    $wp = New-Object V77S.Native+WINDOWPLACEMENT
    $wp.length = [Runtime.InteropServices.Marshal]::SizeOf($wp)
    [void][V77S.Native]::GetWindowPlacement($h, [ref]$wp)
    $n = $wp.rcNormalPosition
    [PSCustomObject]@{ ShowCmd = $wp.showCmd; Normal = [PSCustomObject]@{ X = $n.Left; Y = $n.Top; W = $n.Right - $n.Left; H = $n.Bottom - $n.Top }; Zoomed = [V77S.Native]::IsZoomed($h); Arranged = (Get-Arranged $h); Style = ('0x{0:X8}' -f [V77S.Native]::GetWindowLongPtr($h, -16).ToInt64()) }
}
function Find-Window([int]$ProcId, [string]$Title) {
    $h = [V77S.Native]::GetTopWindow([IntPtr]::Zero)
    while ($h -ne [IntPtr]::Zero) {
        if ((Get-WinPid $h) -eq $ProcId -and [V77S.Native]::IsWindowVisible($h)) {
            $sb = New-Object System.Text.StringBuilder 256
            [void][V77S.Native]::GetWindowText($h, $sb, 256)
            if ($sb.ToString() -eq $Title) { return $h }
        }
        $h = [V77S.Native]::GetWindow($h, 2)
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
function Wait-StableRect([IntPtr]$H, [int]$TimeoutSec = 8) {
    $sw = [Diagnostics.Stopwatch]::StartNew(); $prev = $null
    while ($sw.Elapsed.TotalSeconds -lt $TimeoutSec) {
        Start-Sleep -Milliseconds 300
        $cur = Get-Rect $H
        if (Same $cur $prev) { return $cur }
        $prev = $cur
    }
    return $prev
}
function Invoke-Eval([string]$Page, [string]$Expr) {
    $out = & node (Join-Path $PSScriptRoot 'host-cdp-eval.mjs') $CdpPort $Page $Expr 2>&1
    return ($out -join "`n").Trim()
}
function Wait-Eval([string]$Page, [string]$Expr, [string]$Pattern, [int]$TimeoutSec = 12) {
    $sw = [Diagnostics.Stopwatch]::StartNew(); $last = $null
    while ($sw.Elapsed.TotalSeconds -lt $TimeoutSec) {
        $last = Invoke-Eval $Page $Expr
        if ($last -match $Pattern) { return $last }
        Start-Sleep -Milliseconds 300
    }
    return $last
}
function Get-ThreadInfo([uint32]$Tid) {
    $info = New-Object V77S.Native+GUITHREADINFO
    $info.cbSize = [Runtime.InteropServices.Marshal]::SizeOf($info)
    [void][V77S.Native]::GetGUIThreadInfo($Tid, [ref]$info)
    return $info
}

# ── 前置 ─────────────────────────────────────────────────────────────────────────
if (Get-Process -Name fc-host -ErrorAction SilentlyContinue) {
    Write-Host 'BLOCKED: 已有 fc-host 在執行，本腳本不結束它；請先自行關閉。'
    exit 2
}
$Exe = (Resolve-Path $Exe).Path
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$logPath = Join-Path $OutDir "7.7-aero-snap$Suffix.log"
$sumPath = Join-Path $OutDir "7.7-aero-snap$Suffix-summary.log"
$log = New-EvidenceWriter $logPath
$log.AutoFlush = $true
function Log([string]$m) { $line = "$(Get-Ts) $m"; $log.WriteLine($line); Write-Host (ConvertTo-EvidenceText $line) }
Log "# verify-7.7-aero-snap.ps1 exe=$Exe preflight=$($pf.Message)"

$realSettings = Join-Path $env:APPDATA 'tw.fintools.fc-host\settings.json'
$realHashBefore = if (Test-Path $realSettings) { (Get-FileHash $realSettings).Hash } else { '<不存在>' }
$regSnap = @(Save-FcHostAutostartRegistry)
Log "# 真正的設定檔雜湊（開始）=$realHashBefore；開機自啟登錄快照：$(($regSnap | ForEach-Object { "$($_.Name)@$(Split-Path $_.Key -Leaf) Exists=$($_.Exists)" }) -join '; ')"

$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ('fc-host-7.7s-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
$tempAppData = Join-Path $tempRoot 'Roaming'
$tempLocal = Join-Path $tempRoot 'Local'
$dataDir = Join-Path $tempLocal 'tw.fintools.fc-host\data'
New-Item -ItemType Directory -Force -Path $tempAppData, $dataDir | Out-Null
if (Test-Path $DataFile) { Copy-Item $DataFile (Join-Path $dataDir 'tw_events.json') }
Log "# 暫存 APPDATA=$tempAppData LOCALAPPDATA=$tempLocal（資料：$DataFile 複本）"

$results = [ordered]@{}
$script:mouseDown = $false
$hostProc = $null
$minimized = New-Object System.Collections.Generic.List[object]
# hung-target（review 642050f）：沒還原成功的使用者視窗（無回應、最小化未確認、讀回逾時／不符），寫進摘要。
$occNotRestored = New-Object System.Collections.Generic.List[string]

function Start-Host {
    $old = @($env:APPDATA, $env:LOCALAPPDATA, $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS)
    try {
        $env:APPDATA = $tempAppData
        $env:LOCALAPPDATA = $tempLocal
        $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$CdpPort"
        return Start-Process -FilePath $Exe -PassThru
    } finally {
        $env:APPDATA, $env:LOCALAPPDATA, $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $old
    }
}
function Stop-HostTree([System.Diagnostics.Process]$P) {
    if ($P -and -not $P.HasExited) { Stop-Process -Id $P.Id -Force -ErrorAction SilentlyContinue; $P.WaitForExit(10000) | Out-Null }
    $leaf = Split-Path $tempRoot -Leaf
    Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" -ErrorAction SilentlyContinue |
        Where-Object { $_.CommandLine -and $_.CommandLine -match [regex]::Escape($leaf) } |
        ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
    Start-Sleep -Seconds 2
}
# fix F8：遮擋處理一律經 lib/Occluders.psm1 的 Clear-Occluders（白名單；殼層、系統 UI、對話框等不動）。
# fix F8b：判讀經 lib 的 Assert-OccluderResult——按下點由小工具的實際矩形推出，命中桌面或沒命中任何視窗
# ＝小工具不在預期位置或沉到桌面之下 → FAIL；工作列與系統 UI → ENV-BLOCKED（catch 記錄、結束碼 3）。
$script:envBlocked = $null
$occLog = { param($m) Log "## $m" }

# 拖曳：按下 → 分段移到終點 → 在終點停留（讓吸附預覽出現）→ 放開；記錄是否進入尺寸迴圈與按住時矩形。
function Invoke-EdgeDrag([IntPtr]$H, [int]$FromX, [int]$FromY, [int]$ToX, [int]$ToY, [int]$HoldMs = 1200, [int]$Steps = 14) {
    $tid = [uint32](Get-WinTid $H)
    $inLoop = 0
    Set-GuardedCursorPos $FromX $FromY -What '拖曳起點'
    Invoke-SafeInputSleep 150
    $script:mouseDown = $true
    Invoke-GuardedMouse 2 -What '拖曳按下（LEFTDOWN）'
    Invoke-SafeInputSleep 400
    for ($i = 1; $i -le $Steps; $i++) {
        $x = $FromX + [int][math]::Round(($ToX - $FromX) * $i / $Steps)
        $y = $FromY + [int][math]::Round(($ToY - $FromY) * $i / $Steps)
        Set-GuardedCursorPos $x $y -What "拖曳中（第 $i/$Steps 步）"
        Invoke-SafeInputSleep 50
        $ti = Get-ThreadInfo $tid
        if (([bool]($ti.flags -band 2)) -and $ti.hwndMoveSize -eq $H) { $inLoop++ }
    }
    Invoke-SafeInputSleep $HoldMs
    $held = Get-Rect $H
    $invalid = Invoke-Eval "w=$($script:curId)" "document.body.classList.contains('edit-invalid')"
    Invoke-GuardedMouse 4 -What '拖曳放開（LEFTUP）'
    $script:mouseDown = $false
    Invoke-SafeInputSleep 200
    return [PSCustomObject]@{ InLoop = $inLoop; Held = $held; HeldInvalid = $invalid }
}
# fix F1（review 7.7 M）：拖曳確實發生的正向證據——按住期間系統的尺寸／移動迴圈有取樣到
# （GUI_INMOVESIZE 且 hwndMoveSize＝本視窗），且按住時矩形已離開拖曳前的位置。按下沒打到
# 把手或內容區（遮擋、落點偏移）時兩者都不成立，S1／S2／S4／S5 的「矩形＝推導、記錄未變」
# 會被「完全沒動」空真滿足，故列入判定。$ExpectInvalid 為真時另要求按住當下頁面出現紅框。
function Test-DragHappened($Drag, $Before, [bool]$ExpectInvalid) {
    $moved = ($null -ne $Drag.Held) -and -not (Same $Drag.Held $Before)
    $ok = ($Drag.InLoop -gt 0) -and $moved
    if ($ExpectInvalid) { $ok = $ok -and ($Drag.HeldInvalid -ceq 'true') }
    return $ok
}
function Get-Record([string]$Id) {
    (Invoke-Eval "w=$Id" "window.__TAURI__.core.invoke('get_settings').then(s => { const p = s.widgets['$Id'].placement; return [p.col, p.row, p.w, p.h].join(','); })").Trim('"')
}
function Test-Settled([string]$Tag, [string]$Id, [IntPtr]$H, $Wa) {
    $r = Wait-StableRect $H
    Start-Sleep -Milliseconds 500
    $r = Get-Rect $H
    $rec = Get-Record $Id
    $n = $rec.Split(',') | ForEach-Object { [int]$_ }
    $expected = Grid-Rect $Wa $n[0] $n[1] $n[2] $n[3]
    $pl = Get-Placement $H
    $ok = (Same $r $expected) -and $pl.ShowCmd -eq 1 -and -not $pl.Zoomed -and $pl.Arranged -ne $true -and (Same $pl.Normal $r)
    Log "## $Tag：rect=$(Fmt $r) 記錄=$rec 推導=$(Fmt $expected) showCmd=$($pl.ShowCmd) IsZoomed=$($pl.Zoomed) IsWindowArranged=$($pl.Arranged) style=$($pl.Style) rcNormalPosition=$(Fmt $pl.Normal) → $(if ($ok) { 'OK' } else { 'NG' })"
    return [PSCustomObject]@{ Ok = $ok; Rect = $r; Record = $rec; RectOk = (Same $r $expected); PlacementOk = ($pl.ShowCmd -eq 1 -and -not $pl.Zoomed); NormalOk = ((Same $pl.Normal $r) -and $pl.Arranged -ne $true) }
}
function Invoke-ReenterEditMode {
    Invoke-Eval 'w=clock' "window.__TAURI__.core.invoke('set_edit_mode', { enabled: false })" | Out-Null
    [void](Wait-Eval 'w=clock' "window.__TAURI__.core.invoke('get_settings').then(s => s.layout_locked)" 'true')
    Start-Sleep -Milliseconds 800
    Invoke-Eval 'w=clock' "window.__TAURI__.core.invoke('set_edit_mode', { enabled: true })" | Out-Null
    [void](Wait-Eval 'w=clock' "window.__TAURI__.core.invoke('get_settings').then(s => s.layout_locked)" 'false')
    Start-Sleep -Milliseconds 800
}

try {
    Assert-SessionUnlocked '腳本啟動前置檢查'
    $pt0 = New-Object V77S.Native+POINT
    $mi = New-Object V77S.Native+MONITORINFO
    $mi.cbSize = [Runtime.InteropServices.Marshal]::SizeOf($mi)
    [void][V77S.Native]::GetMonitorInfoW([V77S.Native]::MonitorFromPoint($pt0, 1), [ref]$mi)
    $wa = [PSCustomObject]@{ X = $mi.rcWork.Left; Y = $mi.rcWork.Top; W = $mi.rcWork.Right - $mi.rcWork.Left; H = $mi.rcWork.Bottom - $mi.rcWork.Top }
    $mon = [PSCustomObject]@{ Top = $mi.rcMonitor.Top; Bottom = $mi.rcMonitor.Bottom }
    Log "# 主螢幕 rcMonitor 上緣=$($mon.Top) 下緣=$($mon.Bottom) 工作區=$(Fmt $wa)"

    $hostProc = Start-Host
    Log "# 宿主 pid=$($hostProc.Id)"
    $clock = Wait-Window $hostProc.Id 'fc-host clock'
    $quotes = Wait-Window $hostProc.Id 'fc-host quotes'
    if ($clock -eq [IntPtr]::Zero -or $quotes -eq [IntPtr]::Zero) { throw 'clock／quotes 視窗未出現' }
    Start-Sleep -Seconds 3
    $c0 = Get-Rect $clock; $q0 = Get-Rect $quotes
    Log "## 初始 clock=$(Fmt $c0) quotes=$(Fmt $q0)"

    Invoke-Eval 'w=clock' "window.__TAURI__.core.invoke('set_edit_mode', { enabled: true })" | Out-Null
    $hE = Wait-Eval 'w=clock' "document.querySelectorAll('[data-resize-dir]').length" '^8$'
    Log "## 進入編輯版面：clock 把手數=$hE"
    if ($ProbeNoMaximizeBox) {
        foreach ($hw in $clock, $quotes) {
            $st = [V77S.Native]::GetWindowLongPtr($hw, -16).ToInt64()
            [void][V77S.Native]::SetWindowLongPtr($hw, -16, [IntPtr]($st -band -bnot 0x10000))
            [void][V77S.Native]::SetWindowPos($hw, [IntPtr]::Zero, 0, 0, 0, 0, 0x0237)
            Log "## 探針：$(Hex $hw) 拿掉 WS_MAXIMIZEBOX：0x$('{0:X8}' -f $st) → 0x$('{0:X8}' -f [V77S.Native]::GetWindowLongPtr($hw, -16).ToInt64())"
        }
    }
    Start-Sleep -Seconds 1

    $cx = [int]($c0.X + $c0.W / 2); $qx = [int]($q0.X + $q0.W / 2)
    Assert-OccluderResult (Clear-Occluders -HostPid $hostProc.Id -Points @(@($cx, ($c0.Y + 3)), @($qx, ($q0.Y + $q0.H - 3))) -Minimized $minimized -Log $occLog)
    $base = Test-Settled '基準（編輯版面，未拖）' 'clock' $clock $wa
    $results['基準：clock 矩形＝推導、一般狀態、rcNormalPosition＝矩形'] = $base.Ok

    # ── S1：clock 上緣拖到螢幕最上緣 ─────────────────────────────────────────────────
    $script:curId = 'clock'
    $d1 = Invoke-EdgeDrag $clock $cx ($c0.Y + 3) $cx $mon.Top
    Log "## S1 上緣拖到 y=$($mon.Top)：尺寸迴圈取樣=$($d1.InLoop) 按住時=$(Fmt $d1.Held) edit-invalid=$($d1.HeldInvalid)"
    $s1 = Test-Settled 'S1 放開後' 'clock' $clock $wa
    $results['S1：拖曳確實發生（尺寸迴圈取樣 > 0、按住時矩形離開原位）'] = Test-DragHappened $d1 $base.Rect $false
    $results['S1：上緣拖到螢幕頂端放開 → 矩形＝推導矩形'] = $s1.RectOk
    $results['S1：放開後 showCmd＝一般、未最大化、rcNormalPosition＝目前矩形（無吸附殘留）'] = ($s1.PlacementOk -and $s1.NormalOk)
    Invoke-ReenterEditMode
    $s1b = Test-Settled 'S1 再進出編輯版面後' 'clock' $clock $wa
    $results['S1：再進出一次編輯版面後矩形仍正確'] = $s1b.Ok

    # ── S2：quotes 下緣拖到螢幕最下緣（工作列上）─────────────────────────────────────
    $script:curId = 'quotes'
    $q1 = Get-Rect $quotes
    $d2 = Invoke-EdgeDrag $quotes $qx ($q1.Y + $q1.H - 3) $qx ($mon.Bottom - 1)
    Log "## S2 下緣拖到 y=$($mon.Bottom - 1)：尺寸迴圈取樣=$($d2.InLoop) 按住時=$(Fmt $d2.Held) edit-invalid=$($d2.HeldInvalid)"
    $s2 = Test-Settled 'S2 放開後' 'quotes' $quotes $wa
    $results['S2：拖曳確實發生（尺寸迴圈取樣 > 0、按住時矩形離開原位、按住時紅框）'] = Test-DragHappened $d2 $q1 $true
    $results['S2：下緣拖到螢幕底端放開 → 彈回、矩形＝推導矩形（記錄未變）'] = ($s2.RectOk -and $s2.Record -eq '15,43,32,4')
    $results['S2：放開後 showCmd＝一般、未最大化、rcNormalPosition＝目前矩形'] = ($s2.PlacementOk -and $s2.NormalOk)
    Invoke-ReenterEditMode
    $s2b = Test-Settled 'S2 再進出編輯版面後' 'quotes' $quotes $wa
    $results['S2：再進出一次編輯版面後矩形仍正確'] = $s2b.Ok

    # ── S3：雙擊 clock 上緣把手 ─────────────────────────────────────────────────────
    $script:curId = 'clock'
    $c3 = Get-Rect $clock
    Assert-OccluderResult (Clear-Occluders -HostPid $hostProc.Id -Points @(, @($cx, ($c3.Y + 3))) -Minimized $minimized -Log $occLog)
    Set-GuardedCursorPos $cx ($c3.Y + 3) -What '雙擊上緣'
    Invoke-SafeInputSleep 150
    for ($k = 1; $k -le 2; $k++) {
        $script:mouseDown = $true
        Invoke-GuardedMouse 2 -What "雙擊第 $k 下按下"
        Invoke-SafeInputSleep 40
        Invoke-GuardedMouse 4 -What "雙擊第 $k 下放開"
        $script:mouseDown = $false
        Invoke-SafeInputSleep 60
    }
    Invoke-SafeInputSleep 1200
    Log "## S3 雙擊上緣後（1.2 秒）rect=$(Fmt (Get-Rect $clock))"
    $s3 = Test-Settled 'S3 雙擊後' 'clock' $clock $wa
    $results['S3：雙擊上緣後矩形＝推導矩形'] = $s3.RectOk
    $results['S3：雙擊後 showCmd＝一般、未最大化、rcNormalPosition＝目前矩形'] = ($s3.PlacementOk -and $s3.NormalOk)
    Invoke-ReenterEditMode
    $s3b = Test-Settled 'S3 再進出編輯版面後' 'clock' $clock $wa
    $results['S3：再進出一次編輯版面後矩形仍正確'] = $s3b.Ok

    # ── S4：移動 clock（按住內容）到螢幕最上緣（吸附「最大化」的觸發位置）──────────────
    $c4 = Get-Rect $clock
    $rec4Before = Get-Record 'clock'
    $cy4 = [int]($c4.Y + $c4.H / 2)
    Assert-OccluderResult (Clear-Occluders -HostPid $hostProc.Id -Points @(, @($cx, $cy4)) -Minimized $minimized -Log $occLog)
    $d4 = Invoke-EdgeDrag $clock $cx $cy4 $cx $mon.Top
    Log "## S4 移動到 y=$($mon.Top)：移動迴圈取樣=$($d4.InLoop) 按住時=$(Fmt $d4.Held) edit-invalid=$($d4.HeldInvalid)"
    $s4 = Test-Settled 'S4 放開後' 'clock' $clock $wa
    $results['S4：拖曳確實發生（移動迴圈取樣 > 0、按住時矩形離開原位、按住時紅框）'] = Test-DragHappened $d4 $c4 $true
    $results['S4：移動到螢幕頂端放開 → 矩形＝推導矩形（超出工作區應彈回、記錄未變）'] = ($s4.RectOk -and $s4.Record -eq $rec4Before)
    $results['S4：放開後 showCmd＝一般、未最大化、rcNormalPosition＝目前矩形'] = ($s4.PlacementOk -and $s4.NormalOk)
    Invoke-ReenterEditMode
    $s4b = Test-Settled 'S4 再進出編輯版面後' 'clock' $clock $wa
    $results['S4：再進出一次編輯版面後矩形仍正確'] = $s4b.Ok

    # ── S5：移動 clock 到螢幕最左緣（吸附「左半邊」的觸發位置）──────────────────────────
    $c5 = Get-Rect $clock
    $rec5Before = Get-Record 'clock'
    $cy5 = [int]($c5.Y + $c5.H / 2)
    $d5 = Invoke-EdgeDrag $clock $cx $cy5 0 $cy5
    Log "## S5 移動到 x=0：移動迴圈取樣=$($d5.InLoop) 按住時=$(Fmt $d5.Held) edit-invalid=$($d5.HeldInvalid)"
    $s5 = Test-Settled 'S5 放開後' 'clock' $clock $wa
    $results['S5：拖曳確實發生（移動迴圈取樣 > 0、按住時矩形離開原位、按住時紅框）'] = Test-DragHappened $d5 $c5 $true
    $results['S5：移動到螢幕左緣放開 → 矩形＝推導矩形（超出工作區應彈回、記錄未變）'] = ($s5.RectOk -and $s5.Record -eq $rec5Before)
    $results['S5：放開後 showCmd＝一般、未最大化、rcNormalPosition＝目前矩形'] = ($s5.PlacementOk -and $s5.NormalOk)

    Invoke-Eval 'w=clock' "window.__TAURI__.core.invoke('set_edit_mode', { enabled: false })" | Out-Null
    [void](Wait-Eval 'w=clock' "window.__TAURI__.core.invoke('get_settings').then(s => s.layout_locked)" 'true')
    Start-Sleep -Seconds 1
    $fin = Test-Settled '離開編輯版面後（clock）' 'clock' $clock $wa
    $finQ = Test-Settled '離開編輯版面後（quotes）' 'quotes' $quotes $wa
    $results['離開編輯版面後 clock／quotes 矩形與 placement 皆正確'] = ($fin.Ok -and $finQ.Ok)
}
catch {
    if ("$($_.Exception.Message)" -like 'ENV-BLOCKED*') {
        $script:envBlocked = $_.Exception.Message
        Log "## $($script:envBlocked)（環境問題，結果不完整）"
    } else {
        Log "## 例外中止：$($_.Exception.Message)（第 $($_.InvocationInfo.ScriptLineNumber) 行：$($_.InvocationInfo.Line.Trim())）"
        $results['腳本跑完（無例外）'] = $false
    }
}
finally {
    if ($script:mouseDown) {
        try { Invoke-GuardedMouse 4 -What '收尾：補放開（LEFTUP）' } catch { Log "## 收尾補放開失敗：$_" }
    }
    try { Stop-HostTree $hostProc } catch { Log "## 收尾結束宿主失敗：$_" }
    try { Restore-Occluders -Minimized $minimized -NotRestored $occNotRestored -Log $occLog } catch { Log "## 還原被最小化的視窗失敗：$_" }
    # $minimized 非空＝Restore-Occluders 本身中途丟例外、沒跑完；一併列為未還原（環境，不進逐項結果）。
    if ($minimized.Count -gt 0) { $occNotRestored.Add("Restore-Occluders 未跑完：尚有 $($minimized.Count) 扇未處理（見記錄）") }
    $regLeft = @(Restore-FcHostAutostartRegistry $regSnap)
    if ($regLeft.Count -gt 0) { Write-Warning "開機自啟登錄未還原：$($regLeft -join '; ')" }
    Log "# 開機自啟登錄還原：未還原 $($regLeft.Count) 項$(if ($regLeft.Count) { '：' + ($regLeft -join '; ') })"
    $results['開機自啟登錄已還原（Run／StartupApproved 的 fc-host）'] = ($regLeft.Count -eq 0)
    $realHashAfter = if (Test-Path $realSettings) { (Get-FileHash $realSettings).Hash } else { '<不存在>' }
    Log "# 真正的設定檔雜湊（結束）=$realHashAfter；與開始相同=$($realHashAfter -eq $realHashBefore)"
    $results['隔離：真正的設定檔雜湊前後相同'] = ($realHashAfter -eq $realHashBefore)
    Log '# 結束'
    $log.Close()
    Remove-Item -Recurse -Force $tempRoot -ErrorAction SilentlyContinue
}

$occWarn = Format-UnrestoredWarning -NotRestored $occNotRestored
$sum = New-EvidenceWriter $sumPath
$sum.WriteLine("# verify-7.7-aero-snap.ps1 summary $(Get-Ts)")
foreach ($k in $results.Keys) { $sum.WriteLine("$(if ($results[$k]) { 'PASS' } else { 'FAIL' })  $k") }
if ($script:envBlocked) { $sum.WriteLine("ENV-BLOCKED  $($script:envBlocked)") }
if ($occWarn) { $sum.WriteLine($occWarn) }
$sum.Close()
Get-Content $sumPath
if ($occWarn) { Write-Host $occWarn -ForegroundColor Yellow }
# 結束碼優先序：產品 FAIL（1）＞ 環境（3，含使用者視窗未還原）。
exit (Get-VerdictExitCode -Results $results -EnvBlocked:([bool]$script:envBlocked) -NotRestored $occNotRestored)
