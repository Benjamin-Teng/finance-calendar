<#
.SYNOPSIS
    拖曳延後判定（fix F6b，commit 7b85561）與跨縮放比例拖曳（drag-dpi）的實機驗證：經
    `host/tools/lib/SafeInput.psm1` 注入滑鼠拖曳與 Esc，判讀宿主記錄檔的三行矩形。

.DESCRIPTION
    隔離：全新暫存 %APPDATA%／%LOCALAPPDATA%（首次啟動＝預設版面），資料放 `-DataFile` 複本；
    真正的使用者設定檔只在開始與結束各算一次雜湊；開機自啟登錄以 AutostartRegistry 快照／還原；
    宿主只經 ProcessTree 停自己啟動的那一棵。

    顯示器以 EnumDisplayMonitors 當下讀取：「起始台」＝clock 預設所在的那台，「另一台」＝其餘
    第一台。只有一台時 b／c 記 PENDING。

    取消（fix F7，使用者 2026-10-03 決定方案 A）：**不支援以 Esc 取消拖曳**——小工具不可聚焦，
    Esc 送進使用者的前景程式，系統移動迴圈收不到（批次 B 實測；design.md D7「取消」）。反悔＝
    拖回起點放開（不寫回）或放到不合法位置（彈回）。本腳本以「拖回起點放開」驗不寫回；
    Esc 只保留一項觀察記錄（INFO，不計 PASS／FAIL）：按住時送 Esc，看視窗是否仍停在按住位置。

    Esc 安全：只有在前景是本腳本的基準表單或宿主自己的視窗時才送 Esc，否則不送（避免 Esc
    打進使用者的應用程式或系統對話框），觀察記為未送。

    遮擋（fix F8）：一律經 `lib/Occluders.psm1`，只最小化白名單內的一般應用程式主視窗、finally 以
    `Restore-Occluders` 還原（含吸附與最大化）；遮擋者是工作列或不在白名單（系統 UI、對話框等）＝
    ENV-BLOCKED（結束碼 3）；被本腳本的前景基準表單蓋住＝腳本配置錯誤（FAIL）。按下點命中桌面
    （Progman／WorkerW）、宿主的另一扇視窗或沒命中任何視窗＝小工具不在預期位置或沉到桌面之下＝FAIL
    （fix F8b，判讀經 lib 的 Assert-OccluderResult）。

    步驟（clock，進入編輯版面後；執行順序 a → d → b → c，d 在起始台上做）：
      a. 起始台內往左拖約 400 px，按住（觀察：送 Esc），再拖回起點放開。判讀宿主記錄
         「拖曳開始（WM_ENTERSIZEMOVE）／拖曳結束（WM_EXITSIZEMOVE）／拖曳延後判定（posted）」
         三行矩形與「回到拖曳開始的位置」行；設定檔 clock placement 不變、視窗回原位。
      b. 拖到另一台中央附近，以 50 ms 取樣 GetWindowRect（記中心所在顯示器、尺寸），按住
         （觀察：送 Esc）後沿原路拖回起點放開，判讀同 a，另比對寬高是否回到原尺寸。
      d. 起始台內左右來回拖（50 ms 取樣），最後回到起點放開：尺寸全程不變、不寫回。
      c. 拖到另一台的空位放開：寫回（monitor 改為另一台）、`verify-grid-layout.ps1 -HostPid`
         驗對齊格線與不相交。
#>
[CmdletBinding()]
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\fc-host.exe'),
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [string]$DataFile = 'D:\finance-calendar\tw_events.json',
    [int]$CdpPort = 9391
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\AutostartRegistry.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\ProcessTree.psm1') -Force

Import-Module (Join-Path $PSScriptRoot 'lib\SafeInput.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\Occluders.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\VerifyVerdict.psm1') -Force

Add-Type -Namespace VDE -Name Native -MemberDefinition @'
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
[DllImport("user32.dll")] public static extern bool EnumDisplayMonitors(System.IntPtr hdc, System.IntPtr clip, MonEnum cb, System.IntPtr d);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern bool GetMonitorInfoW(System.IntPtr hMon, ref MONITORINFOEX mi);
[DllImport("shcore.dll")] public static extern int GetDpiForMonitor(System.IntPtr hMon, int type, out uint dpiX, out uint dpiY);
public delegate bool MonEnum(System.IntPtr h, System.IntPtr dc, System.IntPtr r, System.IntPtr d);
[StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
[StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
[StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)] public struct MONITORINFOEX {
  public int cbSize; public RECT rcMonitor; public RECT rcWork; public uint dwFlags;
  [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string szDevice; }
[StructLayout(LayoutKind.Sequential)] public struct GUITHREADINFO {
  public int cbSize; public uint flags; public System.IntPtr hwndActive; public System.IntPtr hwndFocus;
  public System.IntPtr hwndCapture; public System.IntPtr hwndMenuOwner; public System.IntPtr hwndMoveSize;
  public System.IntPtr hwndCaret; public RECT rcCaret; }
[StructLayout(LayoutKind.Sequential)] public struct WINDOWPLACEMENT {
  public int length; public int flags; public int showCmd; public POINT ptMinPosition; public POINT ptMaxPosition;
  public RECT rcNormalPosition; }
'@

# Per-Monitor-V2（-4）：GetWindowRect／GetMonitorInfo／SetCursorPos 都是實體像素。
[void][VDE.Native]::SetThreadDpiAwarenessContext([IntPtr](-4))

# fix F9：前置探查改在設定 DPI 感知之後（它以 GetWindowRect 與螢幕矩形判斷全螢幕覆蓋層，要與後續座標同一個
# awareness；tests/DpiAwareness.Tests.ps1 靜態檢查）。
$pf = Invoke-SafeInputPreflight
if ($pf.ExitCode -ne 0) { Write-Host $pf.Message; exit $pf.ExitCode }
Write-Host $pf.Message

$GRID = 48
function Get-Ts { Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fffK' }
function Hex([IntPtr]$h) { '0x{0:X}' -f $h.ToInt64() }
function Get-Cls([IntPtr]$h) { $sb = New-Object System.Text.StringBuilder 256; [void][VDE.Native]::GetClassName($h, $sb, 256); $sb.ToString() }
function Get-WinPid([IntPtr]$h) { $p = 0; [void][VDE.Native]::GetWindowThreadProcessId($h, [ref]$p); [int]$p }
function Get-WinTid([IntPtr]$h) { $p = 0; [int][VDE.Native]::GetWindowThreadProcessId($h, [ref]$p) }
function Get-Rect([IntPtr]$h) {
    $r = New-Object VDE.Native+RECT
    [void][VDE.Native]::GetWindowRect($h, [ref]$r)
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
function In-Rect($R, [int]$X, [int]$Y) { $X -ge $R.X -and $X -lt ($R.X + $R.W) -and $Y -ge $R.Y -and $Y -lt ($R.Y + $R.H) }

function Get-Monitors {
    $list = New-Object System.Collections.Generic.List[object]
    $cb = [VDE.Native+MonEnum] {
        param($h, $dc, $r, $d)
        $mi = New-Object VDE.Native+MONITORINFOEX
        $mi.cbSize = [Runtime.InteropServices.Marshal]::SizeOf($mi)
        [void][VDE.Native]::GetMonitorInfoW($h, [ref]$mi)
        $dx = [uint32]0; $dy = [uint32]0
        [void][VDE.Native]::GetDpiForMonitor($h, 0, [ref]$dx, [ref]$dy)
        $list.Add([PSCustomObject]@{
                Device  = $mi.szDevice
                Primary = [bool]($mi.dwFlags -band 1)
                Dpi     = [int]$dx
                Mon     = [PSCustomObject]@{ X = $mi.rcMonitor.Left; Y = $mi.rcMonitor.Top; W = $mi.rcMonitor.Right - $mi.rcMonitor.Left; H = $mi.rcMonitor.Bottom - $mi.rcMonitor.Top }
                Work    = [PSCustomObject]@{ X = $mi.rcWork.Left; Y = $mi.rcWork.Top; W = $mi.rcWork.Right - $mi.rcWork.Left; H = $mi.rcWork.Bottom - $mi.rcWork.Top }
            })
        $true
    }
    [void][VDE.Native]::EnumDisplayMonitors([IntPtr]::Zero, [IntPtr]::Zero, $cb, [IntPtr]::Zero)
    return $list.ToArray()
}
function Get-MonitorAt([int]$X, [int]$Y) {
    foreach ($m in $script:mons) { if (In-Rect $m.Mon $X $Y) { return $m } }
    return $null
}

function Find-Window([int]$ProcId, [string]$Title) {
    $h = [VDE.Native]::GetTopWindow([IntPtr]::Zero)
    while ($h -ne [IntPtr]::Zero) {
        if ((Get-WinPid $h) -eq $ProcId -and [VDE.Native]::IsWindowVisible($h)) {
            $sb = New-Object System.Text.StringBuilder 256
            [void][VDE.Native]::GetWindowText($h, $sb, 256)
            if ($sb.ToString() -eq $Title) { return $h }
        }
        $h = [VDE.Native]::GetWindow($h, 2)
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
        Start-Sleep -Milliseconds 250
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

# ── 前置 ─────────────────────────────────────────────────────────────────────────
if (Get-Process -Name fc-host -ErrorAction SilentlyContinue) {
    Write-Host 'BLOCKED: 已有 fc-host 在執行，本腳本不結束它；請先自行關閉。'
    exit 2
}
$Exe = (Resolve-Path $Exe).Path
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$logPath = Join-Path $OutDir 'dragdpi-esc.log'
$sumPath = Join-Path $OutDir 'dragdpi-esc-summary.log'
$samplePath = Join-Path $OutDir 'dragdpi-esc-samples.csv'
$hostLogOut = Join-Path $OutDir 'dragdpi-esc-hostlog.log'
$zorderPath = Join-Path $OutDir 'dragdpi-esc-zorder.log'
$zorderErr = Join-Path ([IO.Path]::GetTempPath()) ('fc-host-dde-zorder-' + [guid]::NewGuid().ToString('N').Substring(0, 8) + '.err')
$log = New-EvidenceWriter $logPath
$log.AutoFlush = $true
function Log([string]$m) { $line = "$(Get-Ts) $m"; $log.WriteLine($line); Write-Host (ConvertTo-EvidenceText $line) }
$csv = New-EvidenceWriter $samplePath
$csv.AutoFlush = $true
$csv.WriteLine('step,t_ms,cursor_x,cursor_y,x,y,w,h,center_monitor,fg_is_safe')
Log "# verify-dragdpi-esc.ps1 exe=$Exe preflight=$($pf.Message)"

$realSettings = Join-Path $env:APPDATA 'tw.fintools.fc-host\settings.json'
$realHashBefore = if (Test-Path $realSettings) { (Get-FileHash $realSettings).Hash } else { '<不存在>' }
$regSnap = @(Save-FcHostAutostartRegistry)
Log "# 真正的設定檔雜湊（開始）=$realHashBefore；開機自啟登錄快照：$(($regSnap | ForEach-Object { "$($_.Name)@$($_.Key) Exists=$($_.Exists)" }) -join '; ')"

$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ('fc-host-dde-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
$tempAppData = Join-Path $tempRoot 'Roaming'
$tempLocal = Join-Path $tempRoot 'Local'
$dataDir = Join-Path $tempLocal 'tw.fintools.fc-host\data'
New-Item -ItemType Directory -Force -Path $tempAppData, $dataDir | Out-Null
if (Test-Path $DataFile) { Copy-Item $DataFile (Join-Path $dataDir 'tw_events.json') }
$tempSettings = Join-Path $tempAppData 'tw.fintools.fc-host\settings.json'
$hostLogDir = Join-Path $tempLocal 'tw.fintools.fc-host\logs'
Log "# 暫存 APPDATA=$tempAppData LOCALAPPDATA=$tempLocal"

$results = [ordered]@{}
$pending = [ordered]@{}
$notRun = [ordered]@{}
$info = [ordered]@{}   # 觀察記錄（INFO）：不計 PASS／FAIL、不影響結束碼
$script:mouseDown = $false
$script:escDown = $false
$hostProc = $null
$fgProc = $null
$zorderProc = $null
$minimized = New-Object System.Collections.Generic.List[object]
# hung-target（review 642050f）：沒還原成功的使用者視窗（無回應、最小化未確認、讀回逾時／不符），寫進摘要。
$occNotRestored = New-Object System.Collections.Generic.List[string]
$script:blocked = $null

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

# fix F8：遮擋處理一律經 lib/Occluders.psm1 的 Clear-Occluders（白名單；殼層、系統 UI、對話框等不動）。
# fix F8b：判讀經 lib 的 Assert-OccluderResult——工作列與系統 UI＝環境 → ENV-BLOCKED（結束碼 3）；命中桌面、
# 宿主的另一扇視窗或沒命中任何視窗＝小工具不在預期位置或沉到桌面之下，被本腳本的前景基準表單蓋住＝腳本
# 配置錯誤 → 一般例外（FAIL）。
$script:envBlocked = $null
$occLog = { param($m) Log "## $m" }
function Get-OwnPids { if ($fgProc) { @($fgProc.Id) } else { @() } }

# Esc 只送給本腳本的基準表單或宿主自己的視窗。
function Test-SafeForeground {
    $fg = [VDE.Native]::GetForegroundWindow()
    if ($fg -eq $script:fgHwnd) { return $true }
    if ($hostProc -and (Get-WinPid $fg) -eq $hostProc.Id) { return $true }
    return $false
}

# 讓基準表單成為前景（Alt 單擊＋SetForegroundWindow，最多 3 次）；回傳是否成功。
function Set-BaseForeground([string]$Tag) {
    for ($i = 1; $i -le 3; $i++) {
        Send-GuardedAltTap
        [void][VDE.Native]::SetForegroundWindow($script:fgHwnd)
        Start-Sleep -Milliseconds 400
        $fg = [VDE.Native]::GetForegroundWindow()
        Log "## 前景（$Tag，第 $i 次）：fg=$(Hex $fg)($(Get-Cls $fg)，pid $(Get-WinPid $fg)) 期望=$(Hex $script:fgHwnd)"
        if ($fg -eq $script:fgHwnd) { return $true }
    }
    return $false
}

function Get-HostLogLines {
    $f = Get-ChildItem -Path $hostLogDir -Filter 'fc-host.*.log' -ErrorAction SilentlyContinue | Sort-Object LastWriteTime | Select-Object -Last 1
    if (-not $f) { return @() }
    return @(Get-Content -LiteralPath $f.FullName -Encoding utf8)
}

# 拖曳：按下 → 沿路徑點逐步移動（每步 50 ms 取樣矩形）→ 走完路徑後放開。
# -HoldAt：走到 $Path[$HoldAt] 時停住、記下按住時矩形（Held）；-EscAtHold 時另做 Esc 觀察
# （只在安全前景送），然後繼續走完剩下的路徑。-HoldAt 省略＝在終點記 Held。
# 回傳 @{ Samples; Held; EscSent; EscSkipped; AfterEsc（Esc 後 400 ms 的矩形取樣） }
function Invoke-PathDrag {
    param([string]$Tag, [object[]]$Path, [int]$StepPx = 40, [int]$HoldAt = -1, [switch]$EscAtHold)
    if ($HoldAt -lt 0) { $HoldAt = $Path.Count - 1 }
    $samples = New-Object System.Collections.Generic.List[object]
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $take = {
        param($cx, $cy)
        $r = Get-Rect $script:clock
        $m = Get-MonitorAt ([int]($r.X + $r.W / 2)) ([int]($r.Y + $r.H / 2))
        $o = [PSCustomObject]@{ T = $sw.ElapsedMilliseconds; CX = $cx; CY = $cy; R = $r; Mon = $(if ($m) { $m.Device } else { '?' }); Safe = (Test-SafeForeground) }
        $samples.Add($o)
        $csv.WriteLine("$Tag,$($o.T),$cx,$cy,$($r.X),$($r.Y),$($r.W),$($r.H),$($o.Mon),$($o.Safe)")
    }
    $from = $Path[0]
    Set-GuardedCursorPos $from[0] $from[1] -What "$Tag 起點"
    Invoke-SafeInputSleep 150
    & $take $from[0] $from[1]
    $script:mouseDown = $true
    Invoke-GuardedMouse 2 -What "$Tag 按下（LEFTDOWN）"
    Invoke-SafeInputSleep 350
    $cur = $from
    $held = $null
    $escSent = $false; $escSkipped = $false
    $after = New-Object System.Collections.Generic.List[object]
    $heldT = 0
    if ($HoldAt -eq 0) { Invoke-SafeInputSleep 300; & $take $cur[0] $cur[1]; $held = Get-Rect $script:clock; $heldT = $samples[$samples.Count - 1].T }
    for ($k = 1; $k -lt $Path.Count; $k++) {
        $to = $Path[$k]
        $dist = [math]::Sqrt([math]::Pow($to[0] - $cur[0], 2) + [math]::Pow($to[1] - $cur[1], 2))
        $n = [math]::Max(1, [int][math]::Ceiling($dist / $StepPx))
        for ($i = 1; $i -le $n; $i++) {
            $x = $cur[0] + [int][math]::Round(($to[0] - $cur[0]) * $i / $n)
            $y = $cur[1] + [int][math]::Round(($to[1] - $cur[1]) * $i / $n)
            Set-GuardedCursorPos $x $y -What "$Tag 拖曳中"
            Invoke-SafeInputSleep 50
            & $take $x $y
        }
        $cur = $to
        if ($k -eq $HoldAt) {
            Invoke-SafeInputSleep 300
            & $take $cur[0] $cur[1]
            $held = Get-Rect $script:clock
            $heldT = $samples[$samples.Count - 1].T
            if ($EscAtHold) {
                # 觀察（INFO）：Esc 不取消拖曳（小工具不可聚焦）。只在安全前景送，否則一個鍵都不送。
                if (Test-SafeForeground) {
                    $script:escDown = $true
                    Invoke-GuardedKey 0x1B -What "$Tag Esc（觀察）"
                    Invoke-SafeInputSleep 60
                    Invoke-GuardedKey 0x1B -Up -What "$Tag Esc（觀察）"
                    $script:escDown = $false
                    $escSent = $true
                    for ($i = 0; $i -lt 8; $i++) {
                        Invoke-SafeInputSleep 50
                        & $take $cur[0] $cur[1]
                        $after.Add($samples[$samples.Count - 1])
                    }
                } else {
                    $fg = [VDE.Native]::GetForegroundWindow()
                    Log "## $Tag：前景 $(Hex $fg)（$(Get-Cls $fg)，pid $(Get-WinPid $fg)）不是基準表單或宿主，不送 Esc（觀察略過）"
                    $escSkipped = $true
                }
            }
        }
    }
    Invoke-SafeInputSleep 300
    & $take $cur[0] $cur[1]
    Invoke-GuardedMouse 4 -What "$Tag 放開（LEFTUP）"
    $script:mouseDown = $false
    Invoke-SafeInputSleep 150
    & $take $cur[0] $cur[1]
    return [PSCustomObject]@{ Samples = $samples; Held = $held; HeldT = $heldT; EscSent = $escSent; EscSkipped = $escSkipped; AfterEsc = $after }
}

# 解析宿主記錄中自 $From 行起的拖曳三行（以最後一組為準）。
function Get-DragTrio([int]$From) {
    $lines = @(Get-HostLogLines)
    $new = if ($lines.Count -gt $From) { $lines[$From..($lines.Count - 1)] } else { @() }
    $rx = 'x=(-?\d+) y=(-?\d+) (\d+)×(\d+)'
    $pick = {
        param($pat)
        $l = @($new | Where-Object { $_ -match $pat }) | Select-Object -Last 1
        if (-not $l) { return $null }
        if ($l -match $rx) {
            return [PSCustomObject]@{ Line = $l; R = [PSCustomObject]@{ X = [int]$Matches[1]; Y = [int]$Matches[2]; W = [int]$Matches[3]; H = [int]$Matches[4] } }
        }
        return [PSCustomObject]@{ Line = $l; R = $null }
    }
    [PSCustomObject]@{
        Enter    = & $pick '拖曳開始（WM_ENTERSIZEMOVE）'
        Exit     = & $pick '拖曳結束（WM_EXITSIZEMOVE）'
        Posted   = & $pick '拖曳延後判定（posted）'
        Cancel   = @($new | Where-Object { $_ -match '回到拖曳開始的位置' }).Count -gt 0
        Others   = @($new | Where-Object { $_ -match '拖曳結束：' -and $_ -notmatch 'WM_EXITSIZEMOVE' })
        AllNew   = $new
    }
}
function Log-Trio([string]$Tag, $T) {
    foreach ($k in 'Enter', 'Exit', 'Posted') {
        $v = $T.$k
        Log "## $Tag 記錄 ${k}：$(if ($v) { "$(Fmt $v.R) ← $($v.Line.Trim())" } else { '<無>' })"
    }
    foreach ($o in $T.Others) { Log "## $Tag 記錄 判定：$($o.Trim())" }
}
function Get-SettingsClock {
    if (-not (Test-Path $tempSettings)) { return '<無設定檔>' }
    $j = Get-Content -LiteralPath $tempSettings -Raw | ConvertFrom-Json
    return ($j.widgets.clock.placement | ConvertTo-Json -Compress -Depth 5)
}
# 觀察（INFO，不計 PASS／FAIL）：Esc 送出後的取樣是否都停在按住位置。
# 沒有取樣（未送 Esc）時 Stayed＝$null。
function Get-EscObservation($Held, [object[]]$AfterRects) {
    $rects = @($AfterRects | Where-Object { $_ })
    if ($rects.Count -eq 0) {
        return [PSCustomObject]@{ Stayed = $null; Text = '未送 Esc（前景不是基準表單或宿主），無觀察' }
    }
    $moved = @($rects | Where-Object { -not (Same $_ $Held) })
    $stayed = ($moved.Count -eq 0)
    $text = if ($stayed) { "Esc 送出後 $($rects.Count) 次取樣都停在按住位置（Esc 不取消拖曳，符合方案 A 預期）" }
    else { "Esc 送出後有 $($moved.Count)/$($rects.Count) 次取樣離開按住位置（與批次 B 實測不同，請人工檢視）" }
    return [PSCustomObject]@{ Stayed = $stayed; Text = $text }
}

# 「拖回起點放開」的判定：回傳 [ordered] 判定名稱（不含步驟前綴）→ bool。
# $Trio＝Get-DragTrio 的結果；$StartRect＝拖曳前的視窗矩形；$EndRect＝放開並穩定後的矩形；
# $SettingsBefore／$SettingsAfter＝設定檔 clock placement 的 JSON 字串。
function Get-ReturnVerdict($Trio, $StartRect, $EndRect, [string]$SettingsBefore, [string]$SettingsAfter) {
    $v = [ordered]@{}
    $complete = [bool]($Trio.Enter -and $Trio.Exit -and $Trio.Posted -and $Trio.Enter.R -and $Trio.Exit.R -and $Trio.Posted.R)
    $v['記錄有開始／結束／延後判定三行且矩形可解析'] = $complete
    $v['開始那行＝起點矩形'] = [bool]($Trio.Enter -and $Trio.Enter.R -and (Same $Trio.Enter.R $StartRect))
    $v['延後判定那行＝開始那行（拖回起點）'] = [bool]($Trio.Enter -and $Trio.Posted -and $Trio.Enter.R -and $Trio.Posted.R -and (Same $Trio.Posted.R $Trio.Enter.R))
    $v['記錄有「回到拖曳開始的位置」（不寫回）'] = [bool]$Trio.Cancel
    $v['設定檔 clock placement 不變'] = ($SettingsAfter -ceq $SettingsBefore)
    $v['視窗回原位、寬高回原尺寸'] = [bool](Same $EndRect $StartRect)
    return $v
}

try {
    Assert-SessionUnlocked '腳本啟動前置檢查'
    $script:mons = @(Get-Monitors)
    foreach ($m in $script:mons) { Log "# 顯示器 $($m.Device) primary=$($m.Primary) dpi=$($m.Dpi)（$([int]($m.Dpi * 100 / 96))%） 螢幕=$(Fmt $m.Mon) 工作區=$(Fmt $m.Work)" }

    $zorderProc = Start-Process pwsh -PassThru -WindowStyle Hidden -RedirectStandardError $zorderErr -ArgumentList @(
        '-NoProfile', '-File', (Join-Path $PSScriptRoot 'watch-zorder.ps1'), '-ProcessName', 'fc-host',
        '-OutFile', $zorderPath, '-DurationSec', '300', '-IntervalMs', '200', '-Quiet')
    Start-Sleep -Seconds 2

    $hostProc = Start-Host
    Log "# 宿主 pid=$($hostProc.Id)"
    $script:clock = Wait-Window $hostProc.Id 'fc-host clock'
    if ($script:clock -eq [IntPtr]::Zero) { throw 'clock 視窗未出現' }
    [void](Wait-Window $hostProc.Id 'fc-host macro')
    Start-Sleep -Seconds 3
    $rect0 = Get-Rect $script:clock
    $p0 = @([int]($rect0.X + $rect0.W / 2), [int]($rect0.Y + $rect0.H / 2))
    $startMon = Get-MonitorAt $p0[0] $p0[1]
    $otherMon = @($script:mons | Where-Object { $_.Device -ne $startMon.Device }) | Select-Object -First 1
    Log "## clock 初始=$(Fmt $rect0) 起始台=$($startMon.Device)（dpi $($startMon.Dpi)） 另一台=$(if ($otherMon) { "$($otherMon.Device)（dpi $($otherMon.Dpi)）" } else { '<無>' })"

    # 前景基準表單（起始台左下角，不在拖曳路徑上）。
    $fgTitle = 'fc-host-dde-foreground-' + [guid]::NewGuid().ToString('N').Substring(0, 6)
    $formCmd = "Add-Type -AssemblyName System.Windows.Forms; `$f = New-Object Windows.Forms.Form; `$f.Text = '$fgTitle'; `$t = New-Object Windows.Forms.TextBox; `$t.Multiline = `$true; `$t.Dock = 'Fill'; `$f.Controls.Add(`$t); `$f.Show(); `$f.Hide(); [Windows.Forms.Application]::Run(`$f)"
    $fgProc = Start-Process pwsh -PassThru -WindowStyle Hidden -ArgumentList @('-NoProfile', '-Command', $formCmd)
    $script:fgHwnd = Wait-Window $fgProc.Id $fgTitle
    if ($script:fgHwnd -eq [IntPtr]::Zero) { throw '前景基準視窗未出現' }
    $wa = $startMon.Work
    [void][VDE.Native]::SetWindowPos($script:fgHwnd, [IntPtr]::Zero, $wa.X + 20, $wa.Y + $wa.H - 360, 420, 220, 0x0014)

    Assert-OccluderResult (Clear-Occluders -HostPid $hostProc.Id -Points @(, $p0) -Minimized $minimized -OwnPids (Get-OwnPids) -Log $occLog)

    Invoke-Eval 'w=clock' "window.__TAURI__.core.invoke('set_edit_mode', { enabled: true })" | Out-Null
    $locked = Wait-Eval 'w=clock' "window.__TAURI__.core.invoke('get_settings').then(s => s.layout_locked)" 'false'
    $results['前置：進入編輯版面（layout_locked=false）'] = ($locked -match 'false')

    $info['前置：基準表單取得前景（觀察用的 Esc 只送到這裡或宿主）'] = (Set-BaseForeground 'a 之前')
    $clock0 = Get-SettingsClock
    Log "## 設定檔 clock placement（開始）=$clock0"

    # ── a. 同一台：往左約 400 px 按住（觀察 Esc），再拖回起點放開 ─────────────────────
    $n0 = (Get-HostLogLines).Count
    $pA = @(($p0[0] - 400), $p0[1])
    $da = Invoke-PathDrag -Tag 'a' -Path @($p0, $pA, $p0) -StepPx 40 -HoldAt 1 -EscAtHold
    $ra = Wait-StableRect $script:clock
    Start-Sleep -Milliseconds 800
    $ta = Get-DragTrio $n0
    $obsA = Get-EscObservation $da.Held @($da.AfterEsc | ForEach-Object { $_.R })
    Log "## a 按住時=$(Fmt $da.Held) Esc 送出=$($da.EscSent) Esc 後取樣=$(($da.AfterEsc | ForEach-Object { Fmt $_.R }) -join ' ') 拖回起點放開後=$(Fmt $ra)"
    Log "## a 觀察（INFO）：$($obsA.Text)"
    Log-Trio 'a' $ta
    $clockA = Get-SettingsClock
    Log "## a 設定檔 clock placement=$clockA"
    $info['a：按住時送 Esc 後視窗仍停在按住位置'] = $obsA.Text
    $results['a：拖曳中視窗確實離開原位（按住時矩形≠起點）'] = [bool]($da.Held -and -not (Same $da.Held $rect0))
    $va = Get-ReturnVerdict $ta $rect0 $ra $clock0 $clockA
    foreach ($k in $va.Keys) { $results["a：拖回起點放開 → $k"] = $va[$k] }

    # ── d. 同一台（起始台）來回拖：尺寸全程不變；最後回到起點放開 ──────────────────────
    $rd0 = Get-Rect $script:clock
    $pd = @([int]($rd0.X + $rd0.W / 2), [int]($rd0.Y + $rd0.H / 2))
    Assert-OccluderResult (Clear-Occluders -HostPid $hostProc.Id -Points @(, $pd) -Minimized $minimized -OwnPids (Get-OwnPids) -Log $occLog)
    [void](Set-BaseForeground 'd 之前')
    $clockD0 = Get-SettingsClock
    $n2 = (Get-HostLogLines).Count
    $pathD = @(, $pd)   # 單一元素也要保持「點的陣列」，不能被攤平成 @(x, y)
    for ($c = 0; $c -lt 4; $c++) { $pathD += , @(($pd[0] - 400), ($pd[1] + 30)); $pathD += , @(($pd[0] - 60), ($pd[1] - 20)) }
    $pathD += , $pd
    $dd = Invoke-PathDrag -Tag 'd' -Path $pathD -StepPx 25
    $rdEnd = Wait-StableRect $script:clock
    Start-Sleep -Milliseconds 800
    $td = Get-DragTrio $n2
    $badD = @($dd.Samples | Where-Object { $_.R.W -ne $rd0.W -or $_.R.H -ne $rd0.H })
    $movedD = @($dd.Samples | Where-Object { $_.R.X -ne $rd0.X }).Count
    $monD = Get-MonitorAt $pd[0] $pd[1]
    Log "## d 顯示器=$($monD.Device)（dpi $($monD.Dpi)） 取樣 $($dd.Samples.Count) 筆（間隔約 50 ms），離開起點 x 的取樣 $movedD 筆，尺寸≠$($rd0.W)x$($rd0.H) 的取樣 $($badD.Count) 筆"
    foreach ($b in $badD | Select-Object -First 10) { Log "##   尺寸不同：t=$($b.T)ms $(Fmt $b.R)" }
    Log-Trio 'd' $td
    Log "## d 放開後=$(Fmt $rdEnd)"
    $results['d：同一台來回拖曳確實移動（取樣中有離開起點）'] = ($movedD -gt 0)
    $results['d：同一台來回拖曳尺寸全程不變（50 ms 取樣）'] = ($badD.Count -eq 0)
    $results['d：回到起點結束後視窗回原位、設定檔不變'] = ((Same $rdEnd $rd0) -and ((Get-SettingsClock) -ceq $clockD0))

    # ── b. 跨螢幕：拖進另一台，取樣尺寸，按住（觀察 Esc），沿原路拖回起點放開 ─────────
    $rb0 = Get-Rect $script:clock
    $p0 = @([int]($rb0.X + $rb0.W / 2), [int]($rb0.Y + $rb0.H / 2))
    Assert-OccluderResult (Clear-Occluders -HostPid $hostProc.Id -Points @(, $p0) -Minimized $minimized -OwnPids (Get-OwnPids) -Log $occLog)
    if ($otherMon) { [void](Set-BaseForeground 'b 之前') }
    $clockB0 = Get-SettingsClock
    if (-not $otherMon) {
        $pending['b：跨螢幕拖曳後拖回起點'] = '只有一台顯示器'
        $pending['c：跨螢幕放開'] = '只有一台顯示器'
    } else {
        $ow = $otherMon.Work
        $pB = @([int]($ow.X + $ow.W / 2), [int]($ow.Y + $ow.H / 3))
        $expOther = Grid-Rect $ow 0 0 16 10
        Log "## b 目標點=($($pB[0]),$($pB[1])) 另一台 16×10 格預期尺寸=$($expOther.W)x$($expOther.H)"
        $n1 = (Get-HostLogLines).Count
        $pB1 = @($p0[0], $pB[1])
        $db = Invoke-PathDrag -Tag 'b' -Path @($p0, $pB1, $pB, $pB1, $p0) -StepPx 40 -HoldAt 2 -EscAtHold
        $rb = Wait-StableRect $script:clock
        Start-Sleep -Milliseconds 800
        $tb = Get-DragTrio $n1
        $sizes = @($db.Samples | ForEach-Object { "$($_.R.W)x$($_.R.H)@$($_.Mon)" } | Select-Object -Unique)
        Log "## b 取樣 $($db.Samples.Count) 筆，出現過的尺寸@中心所在顯示器：$($sizes -join ' → ')"
        # 去程（到按住為止）的取樣；回程另以 Get-ReturnVerdict 判定。
        $fwd = @($db.Samples | Where-Object { $_.T -le $db.HeldT })
        $firstSwitch = $fwd | Where-Object { $_.R.W -ne $rb0.W -or $_.R.H -ne $rb0.H } | Select-Object -First 1
        if ($firstSwitch) {
            $cx = [int]($firstSwitch.R.X + $firstSwitch.R.W / 2)
            Log "## b 第一次換尺寸：t=$($firstSwitch.T)ms 游標=($($firstSwitch.CX),$($firstSwitch.CY)) 矩形=$(Fmt $firstSwitch.R) 中心 x=$cx 所在=$($firstSwitch.Mon)"
        }
        $preCross = $fwd | Where-Object { $_.Mon -eq $startMon.Device } | Select-Object -Last 1
        Log "## b 中心仍在起始台的最後一筆：$(if ($preCross) { "t=$($preCross.T)ms 矩形=$(Fmt $preCross.R)" } else { '<無>' })"
        $obsB = Get-EscObservation $db.Held @($db.AfterEsc | ForEach-Object { $_.R })
        Log "## b 按住時=$(Fmt $db.Held) Esc 送出=$($db.EscSent) Esc 後取樣=$(($db.AfterEsc | ForEach-Object { Fmt $_.R }) -join ' ') 拖回起點放開後=$(Fmt $rb)"
        Log "## b 觀察（INFO）：$($obsB.Text)"
        Log-Trio 'b' $tb
        $clockB = Get-SettingsClock
        Log "## b 設定檔 clock placement=$clockB"
        $info['b：跨螢幕按住時送 Esc 後視窗仍停在按住位置'] = $obsB.Text
        $heldOnOther = ($db.Held -and (Get-MonitorAt ([int]($db.Held.X + $db.Held.W / 2)) ([int]($db.Held.Y + $db.Held.H / 2))).Device -eq $otherMon.Device)
        $results['b：按住時中心在另一台、尺寸＝另一台 16×10 格大小'] = [bool]($heldOnOther -and $db.Held.W -eq $expOther.W -and $db.Held.H -eq $expOther.H)
        $results['b：去程中心仍在起始台的取樣尺寸都＝原尺寸'] = (@($fwd | Where-Object { $_.Mon -eq $startMon.Device -and ($_.R.W -ne $rb0.W -or $_.R.H -ne $rb0.H) }).Count -eq 0)
        $vb = Get-ReturnVerdict $tb $rb0 $rb $clockB0 $clockB
        foreach ($k in $vb.Keys) { $results["b：跨螢幕後拖回起點放開 → $k"] = $vb[$k] }
    }

    # ── c. 跨螢幕拖到合法空位放開 ─────────────────────────────────────────────────
    if ($otherMon) {
        $rc0 = Get-Rect $script:clock
        $pc = @([int]($rc0.X + $rc0.W / 2), [int]($rc0.Y + $rc0.H / 2))
        Assert-OccluderResult (Clear-Occluders -HostPid $hostProc.Id -Points @(, $pc) -Minimized $minimized -OwnPids (Get-OwnPids) -Log $occLog)
        $clockC0 = Get-SettingsClock
        # （PowerShell 變數不分大小寫，放開點不可命名成 $pC——會蓋掉起點 $pc。）
        # 一定要跨螢幕：clock 目前在另一台（b 拖回起點失敗、已在另一台寫回）時改拖回起始台左上的
        # 空位（左緣起 16×10 格的中心，上方不碰總經日曆），否則拖到另一台中央。
        $curMon = Get-MonitorAt $pc[0] $pc[1]
        $sw0 = $startMon.Work
        $pDrop = if ($curMon -and $curMon.Device -eq $otherMon.Device) {
            @([int]($sw0.X + $sw0.W * 8 / 48), [int]($sw0.Y + $sw0.H * 6 / 48))
        } else { $pB }
        $cTarget = Get-MonitorAt $pDrop[0] $pDrop[1]
        Log "## c 起點顯示器=$($curMon.Device) 放開點=($($pDrop[0]),$($pDrop[1])) 目標顯示器=$($cTarget.Device)"
        $n3 = (Get-HostLogLines).Count
        $dc = Invoke-PathDrag -Tag 'c' -Path @($pc, @($pc[0], $pDrop[1]), $pDrop) -StepPx 40
        $rcEnd = Wait-StableRect $script:clock
        Start-Sleep -Milliseconds 1200
        $tc = Get-DragTrio $n3
        Log-Trio 'c' $tc
        $clockC = Get-SettingsClock
        $recC = Invoke-Eval 'w=clock' "window.__TAURI__.core.invoke('get_settings').then(s => JSON.stringify(s.widgets.clock.placement))"
        Log "## c 放開後=$(Fmt $rcEnd) 設定檔 clock placement=$clockC get_settings=$recC"
        $endMon = Get-MonitorAt ([int]($rcEnd.X + $rcEnd.W / 2)) ([int]($rcEnd.Y + $rcEnd.H / 2))
        $results['c：放開後中心在目標顯示器（跨螢幕）'] = ($endMon -and $endMon.Device -eq $cTarget.Device -and $cTarget.Device -ne $curMon.Device)
        $results['c：設定檔已寫回（placement 改變）'] = ($clockC -cne $clockC0)
        $results['c：延後判定那行＝結束那行（正常放開，判定讀到放開位置）'] = ($tc.Posted -and $tc.Exit -and (Same $tc.Posted.R $tc.Exit.R))
        $gridOut = & pwsh -NoProfile -File (Join-Path $PSScriptRoot 'verify-grid-layout.ps1') -HostPid $hostProc.Id -Tag 'dragdpi-esc-c' -OutDir $OutDir -NoScreenshot 2>&1
        $gridExit = $LASTEXITCODE
        foreach ($l in @($gridOut | Where-Object { "$_" -match 'PASS|FAIL|clock' })) { Log "## c grid-layout：$(("$l").Trim())" }
        $results["c：verify-grid-layout -HostPid 結束碼 0（實得 $gridExit）"] = ($gridExit -eq 0)
    }
}
catch {
    $msg = $_.Exception.Message
    if ($msg -like 'BLOCKED:*') {
        $script:blocked = $msg
        Log "## BLOCKED（執行途中鎖定，結果不完整）：$msg"
    } elseif ($msg -like 'ENV-BLOCKED*') {
        $script:envBlocked = $msg
        Log "## $msg（環境問題，結果不完整）"
    } else {
        Log "## 例外中止：$msg（第 $($_.InvocationInfo.ScriptLineNumber) 行）"
        $results['腳本跑完（無例外）'] = $false
    }
}
finally {
    if ($script:escDown) { try { Invoke-GuardedKey 0x1B -Up -What '收尾：補放開 Esc' } catch { Log "## 收尾補放開 Esc 失敗：$_" } }
    if ($script:mouseDown) { try { Invoke-GuardedMouse 4 -What '收尾：補放開（LEFTUP）' } catch { Log "## 收尾補放開失敗：$_" } }
    try {
        $lines = @(Get-HostLogLines)
        $w = New-EvidenceWriter $hostLogOut
        foreach ($l in $lines) { $w.WriteLine($l) }
        $w.Close()
        Log "# 宿主記錄已存（$($lines.Count) 行）"
    } catch { Log "## 收尾存宿主記錄失敗：$_" }
    try { if ($hostProc) { $ids = Stop-ProcessTree -Process $hostProc; Log "# 停止宿主行程樹：$($ids -join ',')" } } catch { Log "## 收尾結束宿主失敗：$_" }
    try { if ($fgProc) { [void](Stop-ProcessTree -Process $fgProc) } } catch { Log "## 收尾結束前景表單失敗：$_" }
    Start-Sleep -Seconds 2
    try { Restore-Occluders -Minimized $minimized -NotRestored $occNotRestored -Log $occLog } catch { Log "## 還原被最小化的視窗失敗：$_" }
    # $minimized 非空＝Restore-Occluders 本身中途丟例外、沒跑完；一併列為未還原（環境，不進逐項結果）。
    if ($minimized.Count -gt 0) { $occNotRestored.Add("Restore-Occluders 未跑完：尚有 $($minimized.Count) 扇未處理（見記錄）") }
    try {
        if ($zorderProc) { [void](Stop-ProcessTree -Process $zorderProc) }
        Start-Sleep -Milliseconds 300
        if ((Test-Path $zorderErr) -and (Get-Item $zorderErr).Length -gt 0) { Log "## watch-zorder stderr：$((Get-Content $zorderErr -Raw).Trim())" }
        Remove-Item $zorderErr -ErrorAction SilentlyContinue
    } catch { Log "## 收尾 watch-zorder 失敗：$_" }
    $regLeft = @(Restore-FcHostAutostartRegistry $regSnap)
    Log "# 開機自啟登錄還原：未還原 $($regLeft.Count) 項$(if ($regLeft.Count) { '：' + ($regLeft -join '; ') })"
    $results['開機自啟登錄已還原（Run／StartupApproved 的 fc-host）'] = ($regLeft.Count -eq 0)
    $realHashAfter = if (Test-Path $realSettings) { (Get-FileHash $realSettings).Hash } else { '<不存在>' }
    Log "# 真正的設定檔雜湊（結束）=$realHashAfter；與開始相同=$($realHashAfter -eq $realHashBefore)"
    $results['隔離：真正的設定檔雜湊前後相同'] = ($realHashAfter -eq $realHashBefore)
    $left = @(Get-Process -Name fc-host -ErrorAction SilentlyContinue)
    Log "# 殘留 fc-host：$($left.Count)"
    $results['收尾：無殘留 fc-host'] = ($left.Count -eq 0)
    Log '# 結束'
    $log.Close()
    $csv.Close()
    Remove-Item -Recurse -Force $tempRoot -ErrorAction SilentlyContinue
}

$occWarn = Format-UnrestoredWarning -NotRestored $occNotRestored
$sum = New-EvidenceWriter $sumPath
$sum.WriteLine("# verify-dragdpi-esc.ps1 summary $(Get-Ts)")
foreach ($k in $results.Keys) { $sum.WriteLine("$(if ($results[$k]) { 'PASS' } else { 'FAIL' })  $k") }
foreach ($k in $pending.Keys) { $sum.WriteLine("PENDING  $k：$($pending[$k])") }
foreach ($k in $notRun.Keys) { $sum.WriteLine("NOT-RUN  $k：$($notRun[$k])") }
foreach ($k in $info.Keys) { $sum.WriteLine("INFO  $k：$($info[$k])（觀察，不計 PASS／FAIL）") }
if ($script:blocked) { $sum.WriteLine("BLOCKED  執行途中鎖定，以上結果不完整：$($script:blocked)") }
if ($script:envBlocked) { $sum.WriteLine("ENV-BLOCKED  $($script:envBlocked)") }
if ($occWarn) { $sum.WriteLine($occWarn) }
$sum.Close()
Get-Content $sumPath
if ($occWarn) { Write-Host $occWarn -ForegroundColor Yellow }
# 結束碼優先序：產品 FAIL（1）＞ 鎖定（2）＞ 環境（3，含使用者視窗未還原）。
exit (Get-VerdictExitCode -Results $results -Locked:([bool]$script:blocked) -EnvBlocked:([bool]$script:envBlocked) -NotRestored $occNotRestored)
