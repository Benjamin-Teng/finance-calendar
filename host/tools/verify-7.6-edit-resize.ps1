<#
.SYNOPSIS
    Task 7.6 驗收驅動腳本（真實滑鼠拖曳調整大小；design.md D7「編輯版面」「調整大小的機制」、
    D4 `edit-preview`；specs/widget-host-windows「編輯版面」的「調整大小」「縮到比最小格數還小」
    「鎖定時拖曳無效」）：經 `host/tools/lib/SafeInput.psm1` 注入滑鼠，按住頁面上的調整大小把手
    拖曳，驗證對齊、合法判斷、紅框預告、彈回、內容倍率與前景／焦點。

.DESCRIPTION
    安全：開頭 `Invoke-SafeInputPreflight`（鎖定 → 結束碼 2、合成輸入無效 → 3，直接結束、不產生
    PASS/FAIL）；每次注入前由 SafeInput 重查 LogonUI。收尾每一項各自 try：補放開、結束宿主、結束
    前景表單、還原被最小化的使用者視窗（`Restore-Occluders`，含吸附與最大化）、結束 watch-zorder、
    比對真正設定檔雜湊與 HKCU Run。

    遮擋（fix F8）：一律經 `lib/Occluders.psm1`，只最小化白名單內的一般應用程式主視窗；遮擋者是工作列
    或不在白名單（系統 UI、對話框等）＝ENV-BLOCKED（結束碼 3）；被本腳本的前景基準表單蓋住＝腳本
    配置錯誤（FAIL）。按下點命中桌面（Progman／WorkerW）、宿主的另一扇視窗或沒命中任何視窗＝小工具
    不在預期位置或沉到桌面之下＝FAIL（fix F8b，判讀經 lib 的 Assert-OccluderResult）。

    隔離：全新暫存 %APPDATA%／%LOCALAPPDATA%（首次啟動＝預設版面、`layout_locked=true`），資料目錄
    放 `-DataFile` 的複本；真正的設定檔只在開始與結束各算一次雜湊比對。

    前景基準：腳本另開的 WinForms 小視窗（左下角），拖曳全程每一步取樣 `GetForegroundWindow` 與
    該執行緒 `GetGUIThreadInfo().hwndFocus`。

    步驟（clock，預設 (15,1,16,10)，主螢幕）：
      A.  鎖定時：頁面沒有把手（CDP 數 `[data-resize-dir]`）、視窗沒有 WS_THICKFRAME；在右緣內側
          真實拖曳 → 矩形不變。
      A2. 鎖定時強制呼叫：CDP 掛 mousedown 捕獲監聽器直接呼叫 `startResizeDragging('East')` 再拖
          → 仍不進尺寸迴圈（GUI_INMOVESIZE 恆假）、矩形不變（核心層也鎖住，不只靠把手不存在）。
      B.  外觀：同一扇視窗（鎖定狀態）由腳本暫時加上 WS_SIZEBOX＋SWP_FRAMECHANGED，比對加前／加後
          的 GetClientRect、DWMWA_EXTENDED_FRAME_BOUNDS 與視窗外圈 4px／四角 24×24 的螢幕像素
          （只比對、不存檔），驗證這個樣式不改變圓角／邊框；比完立即拿掉並讀回。
      進入編輯版面（CDP `set_edit_mode(true)`）：WS_THICKFRAME 出現、八個把手出現。關掉 fixed
      （CDP `update_settings`，只送 enabled），讓時鐘右側空出來。
      1.  拖右緣把手向右約一格（偏幾個像素）放開 → 矩形＝格線 (15,1,17,10)、記錄 15,1,17,10、
          devicePixelRatio 變大且接近 縮放×(17 格邏輯寬÷500)、全程無紅框、前景／焦點不變。
          （fix F6：時鐘設計最小高度 156，可用設計高度＝500×(h÷w)×(工作區高÷寬)；4K＠150%
          （3840×2088）要 h÷w ≥ 0.574、筆電 2560×1516＠175% 要 ≥ 0.527。18×10 在 4K 只有約
          151，會被判小於最小格數；17×10 兩台約 160／174，都合法。）
      2.  拖右緣把手縮到 4 格（寬度最小格數：4K＠150% 為 5、筆電＠175% 為 9，兩台都小於）按住
          → `body.edit-invalid`＋PrintWindow 截圖；放開 → 彈回 (15,1,17,10)、紅框消失、記錄
          未變、前景／焦點不變。
      3.  拖左緣把手向右一格放開 → (16,1,16,10)：右緣不動（只對齊被拖邊）、記錄 16,1,16,10、
          倍率回到 16 格的值。
      離開編輯版面：WS_THICKFRAME 消失、把手消失；A'：右緣真實拖曳 → 矩形不變。
      4.  強制結束後重啟 → clock 仍在 (16,1,16,10)。

.PARAMETER Exe
    fc-host.exe 路徑，預設 host/target/release/fc-host.exe（應為**不含** self-test-ipc 的建置）。
#>
[CmdletBinding()]
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\fc-host.exe'),
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [string]$DataFile = 'D:\finance-calendar\tw_events.json',
    [int]$CdpPort = 9377
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\AutostartRegistry.psm1') -Force

Import-Module (Join-Path $PSScriptRoot 'lib\SafeInput.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\Occluders.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\VerifyVerdict.psm1') -Force

Add-Type -AssemblyName System.Drawing
Add-Type -Namespace V76 -Name Native -MemberDefinition @'
[DllImport("user32.dll")] public static extern System.IntPtr SetThreadDpiAwarenessContext(System.IntPtr ctx);
[DllImport("user32.dll")] public static extern System.IntPtr GetTopWindow(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern System.IntPtr GetWindow(System.IntPtr hWnd, uint cmd);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr hWnd, out uint pid);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(System.IntPtr hWnd, System.Text.StringBuilder s, int n);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassName(System.IntPtr hWnd, System.Text.StringBuilder s, int n);
[DllImport("user32.dll")] public static extern bool GetWindowRect(System.IntPtr hWnd, out RECT r);
[DllImport("user32.dll")] public static extern bool GetClientRect(System.IntPtr hWnd, out RECT r);
[DllImport("user32.dll")] public static extern System.IntPtr GetForegroundWindow();
[DllImport("user32.dll")] public static extern bool SetForegroundWindow(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern bool GetGUIThreadInfo(uint idThread, ref GUITHREADINFO info);
[DllImport("user32.dll")] public static extern System.IntPtr WindowFromPoint(POINT p);
[DllImport("user32.dll")] public static extern System.IntPtr GetAncestor(System.IntPtr h, uint flags);
[DllImport("user32.dll")] public static extern bool SetWindowPos(System.IntPtr h, System.IntPtr after, int x, int y, int cx, int cy, uint flags);
[DllImport("user32.dll")] public static extern bool PrintWindow(System.IntPtr h, System.IntPtr hdc, uint flags);
[DllImport("user32.dll", EntryPoint = "GetWindowLongPtrW")] public static extern System.IntPtr GetWindowLongPtr(System.IntPtr h, int idx);
[DllImport("user32.dll", EntryPoint = "SetWindowLongPtrW")] public static extern System.IntPtr SetWindowLongPtr(System.IntPtr h, int idx, System.IntPtr v);
[DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(System.IntPtr h, int attr, out RECT r, int size);
[DllImport("user32.dll")] public static extern bool EnumDisplayMonitors(System.IntPtr hdc, System.IntPtr clip, MonEnum cb, System.IntPtr d);
[DllImport("user32.dll")] public static extern bool GetMonitorInfoW(System.IntPtr hMon, ref MONITORINFO mi);
public delegate bool MonEnum(System.IntPtr h, System.IntPtr dc, System.IntPtr r, System.IntPtr d);
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

# Per-Monitor-V2（-4）：GetWindowRect／GetMonitorInfo／SetCursorPos／CopyFromScreen 都是實體像素。
[void][V76.Native]::SetThreadDpiAwarenessContext([IntPtr](-4))

# fix F9：前置探查改在設定 DPI 感知之後（它以 GetWindowRect 與螢幕矩形判斷全螢幕覆蓋層，要與後續座標同一個
# awareness；tests/DpiAwareness.Tests.ps1 靜態檢查）。
$pf = Invoke-SafeInputPreflight
if ($pf.ExitCode -ne 0) { Write-Host $pf.Message; exit $pf.ExitCode }
Write-Host $pf.Message

$GRID = 48
$WS_THICKFRAME = 0x40000
function Get-Ts { Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fffK' }
function Hex([IntPtr]$h) { '0x{0:X}' -f $h.ToInt64() }
function Get-Cls([IntPtr]$h) { $sb = New-Object System.Text.StringBuilder 256; [void][V76.Native]::GetClassName($h, $sb, 256); $sb.ToString() }
function Get-WinPid([IntPtr]$h) { $p = 0; [void][V76.Native]::GetWindowThreadProcessId($h, [ref]$p); [int]$p }
function Get-WinTid([IntPtr]$h) { $p = 0; [int][V76.Native]::GetWindowThreadProcessId($h, [ref]$p) }
function Get-Rect([IntPtr]$h) {
    $r = New-Object V76.Native+RECT
    [void][V76.Native]::GetWindowRect($h, [ref]$r)
    [PSCustomObject]@{ X = $r.Left; Y = $r.Top; W = $r.Right - $r.Left; H = $r.Bottom - $r.Top }
}
function Get-Style([IntPtr]$h) { [V76.Native]::GetWindowLongPtr($h, -16).ToInt64() }
function Has-ThickFrame([IntPtr]$h) { [bool]((Get-Style $h) -band $WS_THICKFRAME) }
function Fmt($r) { if ($r) { "($($r.X),$($r.Y),$($r.W)x$($r.H))" } else { '<null>' } }
function Same($a, $b) { $a -and $b -and $a.X -eq $b.X -and $a.Y -eq $b.Y -and $a.W -eq $b.W -and $a.H -eq $b.H }
# design.md D7：edge(i) = 起點 + floor(i × 長度 / 48)（整數運算）。
function Edge([int]$Origin, [int]$Extent, [int]$I) { $Origin + [int][math]::Floor([int64]$I * $Extent / $GRID) }
function Grid-Rect($Wa, [int]$Col, [int]$Row, [int]$W, [int]$H) {
    $x0 = Edge $Wa.X $Wa.W $Col; $x1 = Edge $Wa.X $Wa.W ($Col + $W)
    $y0 = Edge $Wa.Y $Wa.H $Row; $y1 = Edge $Wa.Y $Wa.H ($Row + $H)
    [PSCustomObject]@{ X = $x0; Y = $y0; W = $x1 - $x0; H = $y1 - $y0 }
}
function Test-PointInRect($Rect, $Pt) {
    $Pt[0] -ge $Rect.X -and $Pt[0] -lt ($Rect.X + $Rect.W) -and $Pt[1] -ge $Rect.Y -and $Pt[1] -lt ($Rect.Y + $Rect.H)
}
# 調整大小把手的按下點：一律由「當下」的視窗矩形推導，落在矩形內側 $Inset 實體像素（頁面把手是
# 8 CSS px 感應帶，倍率 ≥ 1 時至少 8 實體像素），縱向取中央（避開四角 14 CSS px 方塊）。
# fix F7：不可拿尚未生效的目標矩形（例如加寬後的 17 格）推點——那個點此刻可能落在小工具間隙。
function Get-HandlePoint($Rect, [ValidateSet('E', 'W')][string]$Edge, [int]$Inset = 4) {
    $y = [int]($Rect.Y + [math]::Floor($Rect.H / 2))
    if ($Edge -eq 'E') { return @(($Rect.X + $Rect.W - $Inset), $y) }
    return @(($Rect.X + $Inset), $y)
}
# 開始前要確認沒被蓋住的點：clock 四角內側（外觀比對要讀螢幕像素）與左右把手，全在 $Rect 內。
function Get-InitialOccluderPoints($Rect) {
    # 每個座標都要加括號：PowerShell 的逗號運算子優先於 +。
    @(
        @(($Rect.X + 12), ($Rect.Y + 12)), @(($Rect.X + $Rect.W - 13), ($Rect.Y + 12)),
        @(($Rect.X + 12), ($Rect.Y + $Rect.H - 13)), @(($Rect.X + $Rect.W - 13), ($Rect.Y + $Rect.H - 13)),
        (Get-HandlePoint $Rect 'E'), (Get-HandlePoint $Rect 'W')
    )
}

function Get-Monitors {
    $list = New-Object System.Collections.Generic.List[object]
    $cb = [V76.Native+MonEnum] {
        param($h, $dc, $r, $d)
        $mi = New-Object V76.Native+MONITORINFO
        $mi.cbSize = [Runtime.InteropServices.Marshal]::SizeOf($mi)
        [void][V76.Native]::GetMonitorInfoW($h, [ref]$mi)
        $list.Add([PSCustomObject]@{
                Primary = [bool]($mi.dwFlags -band 1)
                Work    = [PSCustomObject]@{ X = $mi.rcWork.Left; Y = $mi.rcWork.Top; W = $mi.rcWork.Right - $mi.rcWork.Left; H = $mi.rcWork.Bottom - $mi.rcWork.Top }
            })
        $true
    }
    [void][V76.Native]::EnumDisplayMonitors([IntPtr]::Zero, [IntPtr]::Zero, $cb, [IntPtr]::Zero)
    return $list.ToArray()
}

function Find-Window([int]$ProcId, [string]$Title) {
    $h = [V76.Native]::GetTopWindow([IntPtr]::Zero)
    while ($h -ne [IntPtr]::Zero) {
        if ((Get-WinPid $h) -eq $ProcId -and [V76.Native]::IsWindowVisible($h)) {
            $sb = New-Object System.Text.StringBuilder 256
            [void][V76.Native]::GetWindowText($h, $sb, 256)
            if ($sb.ToString() -eq $Title) { return $h }
        }
        $h = [V76.Native]::GetWindow($h, 2)
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
function Get-ThreadInfo([uint32]$Tid) {
    $info = New-Object V76.Native+GUITHREADINFO
    $info.cbSize = [Runtime.InteropServices.Marshal]::SizeOf($info)
    [void][V76.Native]::GetGUIThreadInfo($Tid, [ref]$info)
    return $info
}

# 逐窗 PrintWindow（PW_RENDERFULLCONTENT＝2）：只擷取該視窗自己的內容，不含桌面與其他視窗。
function Save-WindowPng([IntPtr]$H, [string]$Path) {
    $r = Get-Rect $H
    $bmp = New-Object System.Drawing.Bitmap $r.W, $r.H
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $hdc = $g.GetHdc()
    $ok = [V76.Native]::PrintWindow($H, $hdc, 2)
    $g.ReleaseHdc($hdc); $g.Dispose()
    $bmp.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    return $ok
}
# 螢幕像素（含 DWM 畫的外框／圓角）：只在記憶體裡比對，不存檔（可能含桌布）。
function Get-ScreenBitmap($R) {
    $bmp = New-Object System.Drawing.Bitmap $R.W, $R.H
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($R.X, $R.Y, 0, 0, (New-Object System.Drawing.Size $R.W, $R.H))
    $g.Dispose()
    return $bmp
}
# 外圈 4px＋四角 24×24 內，兩張圖不同的像素數（時鐘秒數在內部，不在比對區）。
function Compare-Ring($A, $B) {
    $diff = 0; $total = 0
    for ($y = 0; $y -lt $A.Height; $y++) {
        for ($x = 0; $x -lt $A.Width; $x++) {
            $ring = ($x -lt 4 -or $y -lt 4 -or $x -ge $A.Width - 4 -or $y -ge $A.Height - 4)
            $corner = (($x -lt 24 -or $x -ge $A.Width - 24) -and ($y -lt 24 -or $y -ge $A.Height - 24))
            if (-not ($ring -or $corner)) { continue }
            $total++
            if ($A.GetPixel($x, $y).ToArgb() -ne $B.GetPixel($x, $y).ToArgb()) { $diff++ }
        }
    }
    return [PSCustomObject]@{ Diff = $diff; Total = $total }
}
function Get-FrameBounds([IntPtr]$H) {
    $r = New-Object V76.Native+RECT
    $hr = [V76.Native]::DwmGetWindowAttribute($H, 9, [ref]$r, 16)  # DWMWA_EXTENDED_FRAME_BOUNDS
    [PSCustomObject]@{ HR = $hr; X = $r.Left; Y = $r.Top; W = $r.Right - $r.Left; H = $r.Bottom - $r.Top }
}
function Get-ClientSize([IntPtr]$H) {
    $r = New-Object V76.Native+RECT
    [void][V76.Native]::GetClientRect($H, [ref]$r)
    "$($r.Right - $r.Left)x$($r.Bottom - $r.Top)"
}

# ── 前置 ─────────────────────────────────────────────────────────────────────────
if (Get-Process -Name fc-host -ErrorAction SilentlyContinue) {
    Write-Host 'BLOCKED: 已有 fc-host 在執行，本腳本不結束它；請先自行關閉。'
    exit 2
}
$Exe = (Resolve-Path $Exe).Path
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$logPath = Join-Path $OutDir '7.6-edit-resize.log'
$sumPath = Join-Path $OutDir '7.6-edit-resize-summary.log'
$zorderPath = Join-Path $OutDir '7.6-zorder.log'
$pngPath = Join-Path $OutDir '7.6-red-frame.png'
$pngWidePath = Join-Path $OutDir '7.6-widened.png'
$zorderErr = Join-Path ([IO.Path]::GetTempPath()) ('fc-host-7.6-zorder-' + [guid]::NewGuid().ToString('N').Substring(0, 8) + '.err')
$log = New-EvidenceWriter $logPath
$log.AutoFlush = $true
function Log([string]$m) { $line = "$(Get-Ts) $m"; $log.WriteLine($line); Write-Host $line }
Log "# verify-7.6-edit-resize.ps1 exe=$Exe preflight=$($pf.Message)"

$realSettings = Join-Path $env:APPDATA 'tw.fintools.fc-host\settings.json'
$realHashBefore = if (Test-Path $realSettings) { (Get-FileHash $realSettings).Hash } else { '<不存在>' }
$regSnap = @(Save-FcHostAutostartRegistry)
Log "# 真正的設定檔雜湊（開始）=$realHashBefore；開機自啟登錄快照：$(($regSnap | ForEach-Object { "$($_.Name)@$(Split-Path $_.Key -Leaf) Exists=$($_.Exists)" }) -join '; ')"

$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ('fc-host-7.6-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
$tempAppData = Join-Path $tempRoot 'Roaming'
$tempLocal = Join-Path $tempRoot 'Local'
$dataDir = Join-Path $tempLocal 'tw.fintools.fc-host\data'
New-Item -ItemType Directory -Force -Path $tempAppData, $dataDir | Out-Null
if (Test-Path $DataFile) { Copy-Item $DataFile (Join-Path $dataDir 'tw_events.json') }
Log "# 暫存 APPDATA=$tempAppData LOCALAPPDATA=$tempLocal（資料：$DataFile 複本）"

$results = [ordered]@{}
$script:mouseDown = $false
$script:styleTampered = $null
$hostProc = $null
$fgProc = $null
$zorderProc = $null
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

# 按下點（與截圖區）必須真的落在小工具上。fix F8：遮擋處理一律經 lib/Occluders.psm1 的 Clear-Occluders
# （白名單；殼層、系統 UI、對話框等不動）。fix F8b：判讀經 lib 的 Assert-OccluderResult——工作列與系統
# UI＝環境 → ENV-BLOCKED（結束碼 3）；命中桌面、宿主的另一扇視窗或沒命中任何視窗＝小工具不在預期位置
# 或沉到桌面之下，被本腳本的前景基準表單蓋住＝腳本配置錯誤 → 一般例外（FAIL）。
$script:envBlocked = $null
$occLog = { param($m) Log "## $m" }
function Get-OwnPids { if ($fgProc) { @($fgProc.Id) } else { @() } }

# 拖曳：按下 → 等系統迴圈開始 → 分段移動（每步取樣前景／焦點／尺寸迴圈旗標）→ 可選按住回呼 → 放開。
function Invoke-Drag {
    param([int]$FromX, [int]$FromY, [int]$ToX, [int]$ToY, [int]$Steps = 14, [scriptblock]$WhileHeld)
    $samples = New-Object System.Collections.Generic.List[object]
    $sample = {
        $ti = Get-ThreadInfo $script:hostTid
        $samples.Add([PSCustomObject]@{
                Fg         = [V76.Native]::GetForegroundWindow()
                Focus      = (Get-ThreadInfo $script:fgTid).hwndFocus
                InMoveSize = ([bool]($ti.flags -band 2) -and $ti.hwndMoveSize -eq $script:clock)
            })
    }
    Set-GuardedCursorPos $FromX $FromY -What '拖曳起點'
    Invoke-SafeInputSleep 150
    & $sample
    $script:mouseDown = $true
    Invoke-GuardedMouse 2 -What '拖曳按下（LEFTDOWN）'
    Invoke-SafeInputSleep 400
    for ($i = 1; $i -le $Steps; $i++) {
        $x = $FromX + [int][math]::Round(($ToX - $FromX) * $i / $Steps)
        $y = $FromY + [int][math]::Round(($ToY - $FromY) * $i / $Steps)
        Set-GuardedCursorPos $x $y -What "拖曳中（第 $i/$Steps 步）"
        Invoke-SafeInputSleep 50
        & $sample
    }
    Invoke-SafeInputSleep 250
    if ($WhileHeld) { & $WhileHeld; & $sample }
    Invoke-GuardedMouse 4 -What '拖曳放開（LEFTUP）'
    $script:mouseDown = $false
    Invoke-SafeInputSleep 150
    & $sample
    return , $samples
}
function Test-FgUnchanged($Samples, [string]$Tag) {
    $bad = @($Samples | Where-Object { $_.Fg -ne $script:baseFg -or $_.Focus -ne $script:baseFocus })
    Log "## $Tag：前景／焦點取樣 $($Samples.Count) 次，與基準不同 $($bad.Count) 次（基準 fg=$(Hex $script:baseFg) focus=$(Hex $script:baseFocus)）"
    foreach ($b in $bad) { Log "##   不同：fg=$(Hex $b.Fg)($(Get-Cls $b.Fg)) focus=$(Hex $b.Focus)" }
    return ($bad.Count -eq 0)
}
function Count-InLoop($Samples) { @($Samples | Where-Object InMoveSize).Count }
function Reset-Foreground {
    [void][V76.Native]::SetForegroundWindow($script:fgHwnd)
    Start-Sleep -Milliseconds 300
    $script:baseFg = [V76.Native]::GetForegroundWindow()
    $script:baseFocus = (Get-ThreadInfo $script:fgTid).hwndFocus
}

$clockExpr = "window.__TAURI__.core.invoke('get_settings').then(s => { const p = s.widgets.clock.placement; return [p.col, p.row, p.w, p.h].join(','); })"
$handlesExpr = "document.querySelectorAll('[data-resize-dir]').length"
$dprExpr = 'window.devicePixelRatio'

try {
    Assert-SessionUnlocked '腳本啟動前置檢查'
    $mons = @(Get-Monitors)
    foreach ($m in $mons) { Log "# 顯示器 primary=$($m.Primary) 工作區=$(Fmt $m.Work)" }
    $wa = ($mons | Where-Object Primary | Select-Object -First 1).Work

    $zorderProc = Start-Process pwsh -PassThru -WindowStyle Hidden -RedirectStandardError $zorderErr -ArgumentList @(
        '-NoProfile', '-File', (Join-Path $PSScriptRoot 'watch-zorder.ps1'), '-ProcessName', 'fc-host',
        '-OutFile', $zorderPath, '-DurationSec', '240', '-IntervalMs', '200', '-Quiet')
    Start-Sleep -Seconds 2

    $hostProc = Start-Host
    Log "# 宿主 pid=$($hostProc.Id)"
    $script:clock = Wait-Window $hostProc.Id 'fc-host clock'
    if ($script:clock -eq [IntPtr]::Zero) { throw 'clock 視窗未出現' }
    $clock = $script:clock
    [void](Wait-Window $hostProc.Id 'fc-host macro')
    Start-Sleep -Seconds 3
    $script:hostTid = [uint32](Get-WinTid $clock)
    $rect0 = Get-Rect $clock
    $results['首次啟動：clock 在預設格 (15,1,16,10)'] = Same $rect0 (Grid-Rect $wa 15 1 16 10)
    $dpr0 = Invoke-Eval 'w=clock' $dprExpr
    Log "## clock 初始=$(Fmt $rect0) devicePixelRatio=$dpr0"

    # 前景基準視窗（左下角）。先 Show／Hide 用掉 STARTUPINFO 的 SW_HIDE（見 verify-7.5 同段註解）。
    $fgTitle = 'fc-host-7.6-foreground-' + [guid]::NewGuid().ToString('N').Substring(0, 6)
    $formCmd = "Add-Type -AssemblyName System.Windows.Forms; `$f = New-Object Windows.Forms.Form; `$f.Text = '$fgTitle'; `$t = New-Object Windows.Forms.TextBox; `$t.Multiline = `$true; `$t.Dock = 'Fill'; `$f.Controls.Add(`$t); `$f.Show(); `$f.Hide(); [Windows.Forms.Application]::Run(`$f)"
    $fgProc = Start-Process pwsh -PassThru -WindowStyle Hidden -ArgumentList @('-NoProfile', '-Command', $formCmd)
    $script:fgHwnd = Wait-Window $fgProc.Id $fgTitle
    if ($script:fgHwnd -eq [IntPtr]::Zero) { throw '前景基準視窗未出現' }
    [void][V76.Native]::SetWindowPos($script:fgHwnd, [IntPtr]::Zero, $wa.X + 20, $wa.Y + $wa.H - 360, 420, 220, 0x0014) # NOZORDER|NOACTIVATE
    $script:fgTid = [uint32](Get-WinTid $script:fgHwnd)

    # 會用到的按下點與 clock 的整個矩形都要沒被蓋住（外觀比對要讀螢幕像素）。只檢查此刻 clock 矩形內的
    # 點；加寬後的右緣把手等加寬生效後再以實際矩形檢查（fix F7：加寬前那個點落在 clock 與 fixed 的間隙）。
    $wide = Grid-Rect $wa 15 1 17 10
    $eh0 = Get-HandlePoint $rect0 'E'
    $cy = $eh0[1]
    Assert-OccluderResult (Clear-Occluders -HostPid $hostProc.Id -Points (Get-InitialOccluderPoints $rect0) -Minimized $minimized -OwnPids (Get-OwnPids) -Log $occLog)

    Send-GuardedAltTap
    Reset-Foreground
    Log "## 前景基準：fg=$(Hex $script:baseFg)($(Get-Cls $script:baseFg)) 期望=$(Hex $script:fgHwnd) 焦點=$(Hex $script:baseFocus)($(Get-Cls $script:baseFocus))"
    $results['前置：前景基準視窗取得前景與鍵盤焦點'] = ($script:baseFg -eq $script:fgHwnd -and $script:baseFocus -ne [IntPtr]::Zero)

    # ── A. 鎖定時：沒有把手、沒有 WS_THICKFRAME、右緣拉不動 ─────────────────────────
    $hA = Invoke-Eval 'w=clock' $handlesExpr
    $tfA = Has-ThickFrame $clock
    $sA = Invoke-Drag -FromX $eh0[0] -FromY $cy -ToX ($rect0.X + $rect0.W + 110) -ToY $cy
    Start-Sleep -Milliseconds 600
    $rA = Get-Rect $clock
    Log "## A 鎖定：把手數=$hA WS_THICKFRAME=$tfA 右緣拖曳後=$(Fmt $rA) 尺寸迴圈取樣=$(Count-InLoop $sA)"
    $results['A：鎖定時頁面沒有調整大小把手'] = ($hA -eq '0')
    $results['A：鎖定時視窗沒有 WS_THICKFRAME'] = (-not $tfA)
    $results['A：鎖定時拖右緣，矩形不變'] = Same $rA $rect0
    Invoke-Eval 'w=clock' "(() => { window.getSelection().removeAllRanges(); return 'ok'; })()" | Out-Null

    # ── A2. 鎖定時強制呼叫 startResizeDragging：仍不進尺寸迴圈 ─────────────────────────
    $hook = "(() => { window.__force76 = []; window.__force76h = (e) => { e.stopImmediatePropagation(); e.preventDefault(); window.__TAURI__.window.getCurrentWindow().startResizeDragging('East').then(() => window.__force76.push('ok'), (err) => window.__force76.push('err:' + err)); }; window.addEventListener('mousedown', window.__force76h, true); return 'hooked'; })()"
    Invoke-Eval 'w=clock' $hook | Out-Null
    $sA2 = Invoke-Drag -FromX $eh0[0] -FromY $cy -ToX ($rect0.X + $rect0.W + 110) -ToY $cy
    Start-Sleep -Milliseconds 600
    $rA2 = Get-Rect $clock
    $forceLog = Invoke-Eval 'w=clock' "(() => { window.removeEventListener('mousedown', window.__force76h, true); return window.__force76.join('|'); })()"
    Log "## A2 鎖定時強制 startResizeDragging：JS=$forceLog 拖曳後=$(Fmt $rA2) 尺寸迴圈取樣=$(Count-InLoop $sA2)/$($sA2.Count)"
    $results['A2：鎖定時即使強制呼叫 startResizeDragging 也不進尺寸迴圈、矩形不變'] = ((Count-InLoop $sA2) -eq 0 -and (Same $rA2 $rect0))
    $results['A/A2：拖曳期間前景與焦點不變'] = ((Test-FgUnchanged $sA 'A') -and (Test-FgUnchanged $sA2 'A2'))

    # ── B. WS_SIZEBOX 不改變外觀（腳本暫時加上再拿掉；只比對、不存螢幕截圖）────────────
    $styleB0 = Get-Style $clock
    $clientB0 = Get-ClientSize $clock; $frameB0 = Get-FrameBounds $clock
    $bmpB0 = Get-ScreenBitmap $rect0
    $script:styleTampered = $styleB0
    [void][V76.Native]::SetWindowLongPtr($clock, -16, [IntPtr]($styleB0 -bor $WS_THICKFRAME))
    [void][V76.Native]::SetWindowPos($clock, [IntPtr]::Zero, 0, 0, 0, 0, 0x0237) # NOSIZE|NOMOVE|NOZORDER|NOACTIVATE|FRAMECHANGED|NOOWNERZORDER
    Start-Sleep -Milliseconds 700
    $clientB1 = Get-ClientSize $clock; $frameB1 = Get-FrameBounds $clock; $rectB1 = Get-Rect $clock
    $bmpB1 = Get-ScreenBitmap $rect0
    [void][V76.Native]::SetWindowLongPtr($clock, -16, [IntPtr]$styleB0)
    [void][V76.Native]::SetWindowPos($clock, [IntPtr]::Zero, 0, 0, 0, 0, 0x0237)
    $script:styleTampered = $null
    $cmp = Compare-Ring $bmpB0 $bmpB1
    $bmpB0.Dispose(); $bmpB1.Dispose()
    Log "## B 加 WS_SIZEBOX 前：client=$clientB0 frameBounds=$(Fmt $frameB0)（hr=$($frameB0.HR)）；加後：client=$clientB1 frameBounds=$(Fmt $frameB1) rect=$(Fmt $rectB1)；外圈／四角像素不同 $($cmp.Diff)/$($cmp.Total)；還原後 style=0x$('{0:X8}' -f (Get-Style $clock))"
    $results['B：WS_SIZEBOX 不改變視窗矩形、用戶區（仍＝整個視窗）與 DWM 外框範圍'] = ((Same $rectB1 $rect0) -and $clientB0 -eq $clientB1 -and $clientB1 -eq "$($rect0.W)x$($rect0.H)" -and (Same $frameB0 $frameB1))
    $results['B：WS_SIZEBOX 不改變外圈 4px 與四角 24×24 的螢幕像素（圓角／邊框外觀不變）'] = ($cmp.Diff -eq 0)
    $results['B：比完已拿掉 WS_SIZEBOX'] = (-not (Has-ThickFrame $clock))

    # ── 進入編輯版面 ───────────────────────────────────────────────────────────────
    Invoke-Eval 'w=clock' "window.__TAURI__.core.invoke('set_edit_mode', { enabled: true })" | Out-Null
    $locked = Wait-Eval 'w=clock' "window.__TAURI__.core.invoke('get_settings').then(s => s.layout_locked)" 'false'
    $hE = Wait-Eval 'w=clock' $handlesExpr '^8$'
    Start-Sleep -Milliseconds 500
    $tfE = Has-ThickFrame $clock
    Log "## 進入編輯版面：layout_locked=$locked 把手數=$hE WS_THICKFRAME=$tfE style=0x$('{0:X8}' -f (Get-Style $clock))"
    $results['進入編輯版面：layout_locked=false、八個把手、WS_THICKFRAME 出現'] = ($locked -match 'false' -and $hE -eq '8' -and $tfE)
    Invoke-Eval 'w=clock' "window.__TAURI__.core.invoke('update_settings', { patch: { widgets: { fixed: { enabled: false } } } })" | Out-Null
    $fixedOff = Wait-Eval 'w=clock' "window.__TAURI__.core.invoke('get_settings').then(s => s.widgets.fixed.enabled)" 'false'
    Log "## 關掉 fixed（時鐘右側空出來）：fixed.enabled=$fixedOff"
    $results['前置：fixed 已關閉'] = ($fixedOff -match 'false')
    Start-Sleep -Seconds 1
    Invoke-Eval 'w=clock' "(() => { window.__rec76 = []; const t0 = performance.now(); new MutationObserver(() => window.__rec76.push(Math.round(performance.now() - t0) + 'ms invalid=' + document.body.classList.contains('edit-invalid'))).observe(document.body, { attributes: true, attributeFilter: ['class'] }); return 'ok'; })()" | Out-Null
    Reset-Foreground

    # ── 1. 右緣向右加寬一格 ─────────────────────────────────────────────────────────
    $from1 = $eh0[0]
    $to1 = (Get-HandlePoint $wide 'E')[0] + 9   # 游標終點（不是按下點）：偏離格線 9 像素，驗證 round
    $s1 = Invoke-Drag -FromX $from1 -FromY $cy -ToX $to1 -ToY $cy
    $r1 = Wait-StableRect $clock
    Start-Sleep -Milliseconds 800
    $rec1 = Invoke-Eval 'w=clock' $clockExpr
    $dpr1 = Invoke-Eval 'w=clock' $dprExpr
    $rec1Page = Invoke-Eval 'w=clock' "window.__rec76.join(' | ')"
    $expectedDpr = [math]::Round(($wide.W / 500.0), 4)  # 縮放×(邏輯寬÷500)＝實體寬÷500
    Log "## 1 加寬放開後=$(Fmt $r1) 期望=$(Fmt $wide) 記錄=$rec1 devicePixelRatio $dpr0 → $dpr1（期望約 $expectedDpr）尺寸迴圈取樣=$(Count-InLoop $s1)/$($s1.Count) 頁面 class 記錄=$rec1Page"
    $results['1：拖右緣時進入尺寸迴圈（GUI_INMOVESIZE、hwndMoveSize＝clock）'] = ((Count-InLoop $s1) -gt 0)
    $results['1：放開後矩形＝格線 (15,1,17,10)（左／上／下緣不動）'] = Same $r1 $wide
    $results['1：記錄寫回 15,1,17,10'] = ($rec1 -match '15,1,17,10')
    $results['1：內容倍率變大（devicePixelRatio 增加且接近 實體寬÷設計寬）'] = ([double]$dpr1 -gt [double]$dpr0 -and [math]::Abs([double]$dpr1 - $expectedDpr) -lt 0.03)
    $results['1：合法調整全程沒有紅框'] = ($rec1Page -notmatch 'invalid=true')
    $results['1：調整期間前景與焦點不變'] = Test-FgUnchanged $s1 '1'
    [void](Save-WindowPng $clock $pngWidePath)

    # ── 2. 縮到小於最小格數（4 格；寬度最小格數 4K＠150% 為 5、筆電＠175% 為 9）→ 紅框 → 放開彈回 ─────────────────────────────
    Invoke-Eval 'w=clock' "(() => { window.__rec76.length = 0; return 'ok'; })()" | Out-Null
    $narrow = Grid-Rect $wa 15 1 4 10
    $script:heldClass = $null; $script:heldRect = $null; $script:heldPng = $false
    $held = {
        $script:heldRect = Get-Rect $clock
        $script:heldClass = Invoke-Eval 'w=clock' "document.body.classList.contains('edit-invalid')"
        $script:heldPng = Save-WindowPng $clock $pngPath
    }
    # 按下點由加寬後的實際矩形推導，此刻才檢查遮擋（點在 clock 內，不會碰到桌面）。
    $eh1 = Get-HandlePoint $r1 'E'
    Assert-OccluderResult (Clear-Occluders -HostPid $hostProc.Id -Points @(, $eh1) -Minimized $minimized -OwnPids (Get-OwnPids) -Log $occLog)
    Reset-Foreground
    $s2 = Invoke-Drag -FromX $eh1[0] -FromY $eh1[1] -ToX (Get-HandlePoint $narrow 'E')[0] -ToY $eh1[1] -WhileHeld $held
    $r2 = Wait-StableRect $clock
    Start-Sleep -Milliseconds 500
    $classAfter = Invoke-Eval 'w=clock' "document.body.classList.contains('edit-invalid')"
    $rec2Page = Invoke-Eval 'w=clock' "window.__rec76.join(' | ')"
    $rec2 = Invoke-Eval 'w=clock' $clockExpr
    Log "## 2 按住時矩形=$(Fmt $script:heldRect)（期望約 $(Fmt $narrow)）edit-invalid=$($script:heldClass) 截圖=$($script:heldPng)"
    Log "## 2 放開後=$(Fmt $r2) edit-invalid=$classAfter 記錄=$rec2 頁面 class 記錄=$rec2Page"
    $results['2：按住時視窗確實被縮窄'] = ($script:heldRect -and $script:heldRect.W -lt $r1.W - 400)
    $results['2：小於最小格數時頁面出現紅框（body.edit-invalid）'] = ($script:heldClass -match 'true')
    $results['2：紅框截圖已存（PrintWindow 只含 clock 視窗）'] = [bool]$script:heldPng
    $results['2：放開後彈回拖曳前的大小 (15,1,17,10)'] = Same $r2 $r1
    $results['2：放開後紅框消失、頁面記錄先出現後消失'] = ($classAfter -match 'false' -and $rec2Page -match 'invalid=true.*invalid=false')
    $results['2：記錄未變（仍為 15,1,17,10）'] = ($rec2 -match '15,1,17,10')
    $results['2：調整期間前景與焦點不變'] = Test-FgUnchanged $s2 '2'

    # ── 3. 左緣向右一格（只對齊被拖邊，右緣不動）──────────────────────────────────────
    $after3 = Grid-Rect $wa 16 1 16 10
    $wh2 = Get-HandlePoint $r2 'W'
    $s3 = Invoke-Drag -FromX $wh2[0] -FromY $wh2[1] -ToX ((Get-HandlePoint $after3 'W')[0] + 5) -ToY $wh2[1]
    $r3 = Wait-StableRect $clock
    Start-Sleep -Milliseconds 800
    $rec3 = Invoke-Eval 'w=clock' $clockExpr
    $dpr3 = Invoke-Eval 'w=clock' $dprExpr
    Log "## 3 左緣放開後=$(Fmt $r3) 期望=$(Fmt $after3) 記錄=$rec3 devicePixelRatio=$dpr3"
    $results['3：拖左緣後右緣不動、左緣對齊 col 16（16,1,16,10）'] = ((Same $r3 $after3) -and ($r3.X + $r3.W) -eq ($r2.X + $r2.W))
    $results['3：記錄寫回 16,1,16,10、倍率回到 16 格的值'] = ($rec3 -match '16,1,16,10' -and [math]::Abs([double]$dpr3 - [double]$dpr0) -lt 0.01)
    $results['3：調整期間前景與焦點不變'] = Test-FgUnchanged $s3 '3'

    # ── 離開編輯版面 → A'. 拉不動 ─────────────────────────────────────────────────
    Invoke-Eval 'w=clock' "window.__TAURI__.core.invoke('set_edit_mode', { enabled: false })" | Out-Null
    $locked2 = Wait-Eval 'w=clock' "window.__TAURI__.core.invoke('get_settings').then(s => s.layout_locked)" 'true'
    $h2 = Wait-Eval 'w=clock' $handlesExpr '^0$'
    Start-Sleep -Milliseconds 500
    $tf2 = Has-ThickFrame $clock
    Log "## 離開編輯版面：layout_locked=$locked2 把手數=$h2 WS_THICKFRAME=$tf2"
    $results['離開編輯版面：layout_locked=true、把手消失、WS_THICKFRAME 拿掉'] = ($locked2 -match 'true' -and $h2 -eq '0' -and -not $tf2)
    Reset-Foreground
    $eh3 = Get-HandlePoint $r3 'E'
    $sA3 = Invoke-Drag -FromX $eh3[0] -FromY $eh3[1] -ToX ($r3.X + $r3.W + 110) -ToY $eh3[1]
    Start-Sleep -Milliseconds 600
    $rA3 = Get-Rect $clock
    Log "## A' 離開編輯版面後拖右緣=$(Fmt $rA3) 尺寸迴圈取樣=$(Count-InLoop $sA3)"
    $results["A'：離開編輯版面後拖右緣，矩形不變"] = ((Same $rA3 $r3) -and (Count-InLoop $sA3) -eq 0)

    # ── 4. 重啟後保留 ──────────────────────────────────────────────────────────────
    Stop-HostTree $hostProc
    Log '## 4 宿主已強制結束，重啟'
    $hostProc = Start-Host
    $script:clock = Wait-Window $hostProc.Id 'fc-host clock'
    $clock = $script:clock
    Start-Sleep -Seconds 3
    $r4 = if ($clock -ne [IntPtr]::Zero) { Get-Rect $clock } else { $null }
    $rec4 = Invoke-Eval 'w=clock' $clockExpr
    Log "## 4 重啟後 clock=$(Fmt $r4) 記錄=$rec4"
    $results['4：重啟後大小與位置保留（16,1,16,10）'] = ((Same $r4 $r3) -and $rec4 -match '16,1,16,10')
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
    # 每一項收尾各自 try，任何一項失敗都不影響其餘。
    if ($script:mouseDown) {
        try { Invoke-GuardedMouse 4 -What '收尾：補放開（LEFTUP）' } catch { Log "## 收尾補放開失敗：$_" }
    }
    if ($null -ne $script:styleTampered) {
        try { [void][V76.Native]::SetWindowLongPtr($script:clock, -16, [IntPtr]$script:styleTampered) } catch { Log "## 收尾還原樣式失敗：$_" }
    }
    try { Stop-HostTree $hostProc } catch { Log "## 收尾結束宿主失敗：$_" }
    try { if ($fgProc -and -not $fgProc.HasExited) { Stop-Process -Id $fgProc.Id -Force } } catch { Log "## 收尾結束前景表單失敗：$_" }
    try { Restore-Occluders -Minimized $minimized -NotRestored $occNotRestored -Log $occLog } catch { Log "## 還原被最小化的視窗失敗：$_" }
    # $minimized 非空＝Restore-Occluders 本身中途丟例外、沒跑完；一併列為未還原（環境，不進逐項結果）。
    if ($minimized.Count -gt 0) { $occNotRestored.Add("Restore-Occluders 未跑完：尚有 $($minimized.Count) 扇未處理（見記錄）") }
    try {
        if ($zorderProc -and -not $zorderProc.HasExited) { Stop-Process -Id $zorderProc.Id -Force }
        Start-Sleep -Milliseconds 300
        if ((Test-Path $zorderErr) -and (Get-Item $zorderErr).Length -gt 0) { Log "## watch-zorder stderr：$((Get-Content $zorderErr -Raw).Trim())" }
        Remove-Item $zorderErr -ErrorAction SilentlyContinue
    } catch { Log "## 收尾 watch-zorder 失敗：$_" }
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
$sum.WriteLine("# verify-7.6-edit-resize.ps1 summary $(Get-Ts)")
foreach ($k in $results.Keys) { $sum.WriteLine("$(if ($results[$k]) { 'PASS' } else { 'FAIL' })  $k") }
if ($script:envBlocked) { $sum.WriteLine("ENV-BLOCKED  $($script:envBlocked)") }
if ($occWarn) { $sum.WriteLine($occWarn) }
$sum.Close()
Get-Content $sumPath
if ($occWarn) { Write-Host $occWarn -ForegroundColor Yellow }
# 結束碼優先序：產品 FAIL（1）＞ 環境（3，含使用者視窗未還原）。
exit (Get-VerdictExitCode -Results $results -EnvBlocked:([bool]$script:envBlocked) -NotRestored $occNotRestored)
