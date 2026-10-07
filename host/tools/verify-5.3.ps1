<#
.SYNOPSIS
    Task 5.3 驗收驅動腳本（編輯版面的切換、鎖定旗標與外框；design.md D7「編輯版面」、D9；
    specs/widget-host-windows「編輯版面」「編輯版面時看得到無內容的小工具」「鎖定時拖曳無效」）。
    task 7.7 起改寫為格線版面：移動、調整大小、對齊格線、彈回與保存由 verify-7.5-edit-move.ps1
    與 verify-7.6-edit-resize.ps1 驗收，本腳本只驗切換本身。

.DESCRIPTION
    以 IPC（CDP 在小工具頁面呼叫宿主自己的 `set_edit_mode`／`get_settings`／`update_settings`，
    與系統匣「編輯版面」走同一個指令）切換編輯版面，驗證：
      1. 首次啟動預設鎖定：`layout_locked=true`、頁面沒有 `edit-mode` class、沒有拖曳區、沒有外框；
         資料複本的 `quotes` 改成空陣列＝行情條無內容（平時隱藏）；另開啟 custom1（只送 `enabled`，
         顯示「尚未設定」＝有內容）。
      2. 鎖定時真實拖曳 clock（經 `lib/SafeInput.psm1` 注入）→ 矩形不變。
      3. 進入編輯版面：`layout_locked=false`；所有開啟中的小工具（五個財經＋custom1）都有
         `body.edit-mode`、內容容器帶 `data-tauri-drag-region="deep"`、`body::before` 虛線外框；
         以 `PrintWindow` 逐窗擷取，視窗外圈 4px 內的強調色像素比例必須明顯上升（外框真的畫在
         視窗內、看得到，不是被視窗邊界裁掉）；無內容的 quotes 視窗顯示出來且佔位外框
         （`.edit-placeholder`）可見。
      4. 離開編輯版面：`layout_locked=true` 並落地存檔；class、拖曳區與外框消失（外圈強調色像素
         回到基準）；quotes 回到隱藏。
      5. 離開後再真實拖曳 clock → 矩形不變。

    fix F2（review 5.3 low）：「拖不動」需要對照組——步驟 3（編輯版面、解鎖）中以**同一個手勢**
    （自 clock 中心拖，位移相同）真實拖曳 clock，矩形必須改變；步驟 2、5 的「矩形不變」
    只有在對照組確實移動時才判 PASS（否則可能只是合成拖曳沒送進視窗）。步驟 5 的起點改用步驟 3
    移動後的 clock 中心、位移取反向。

    fix F7（批次 B）：位移不再寫死「往左 700 px」（會撞到本腳本開的 custom1 而彈回），改由
    `Find-FreeDragOffset` 依當下格座標推導：工作區內、不與任何已開小工具相交的格線位置。

    安全：開頭 `Invoke-SafeInputPreflight`：鎖定（結束碼 2）直接結束；合成輸入無效（3）時不繞過，
    2、5 兩項標為「未執行」，其餘照做。每次注入前由 SafeInput 重查 LogonUI。拖曳按下點若被一般
    視窗蓋住，經 lib/Occluders.psm1 暫時最小化（只動白名單內的一般應用程式主視窗），finally 以
    Restore-Occluders 還原（含吸附與最大化）；遮擋者是工作列或不在白名單（系統 UI、對話框等）＝
    ENV-BLOCKED（結束碼 3）（fix F8）。按下點命中桌面（Progman／WorkerW）或沒命中任何視窗＝小工具
    不在預期位置或沉到桌面之下＝FAIL（fix F8b，判讀經 lib 的 Assert-OccluderResult）。

    隔離：全新暫存 %APPDATA%／%LOCALAPPDATA%，資料目錄放 `-DataFile` 的複本；真正的設定檔只在開始
    與結束各算一次雜湊比對；開機自啟登錄（Run／StartupApproved 的 fc-host）以
    lib\AutostartRegistry.psm1 開始快照、結束還原（暫存設定＝首次啟動，宿主會寫 Run）。截圖只存小工具視窗本身
    （`PrintWindow`），記錄檔路徑以 %TEMP% 等字樣輸出。

.PARAMETER Exe
    fc-host.exe 路徑，預設 host/target/release/fc-host.exe（**不含** self-test-ipc）。
#>
[CmdletBinding()]
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\fc-host.exe'),
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [string]$DataFile = 'D:\finance-calendar\tw_events.json',
    [int]$CdpPort = 9341
)

$ErrorActionPreference = 'Stop'

Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\SafeInput.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\AutostartRegistry.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\Occluders.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\VerifyVerdict.psm1') -Force

Add-Type -AssemblyName System.Drawing
Add-Type -Namespace V53 -Name Native -MemberDefinition @'
[DllImport("user32.dll")] public static extern System.IntPtr SetThreadDpiAwarenessContext(System.IntPtr ctx);
[DllImport("user32.dll")] public static extern System.IntPtr GetTopWindow(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern System.IntPtr GetWindow(System.IntPtr hWnd, uint cmd);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr hWnd, out uint pid);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(System.IntPtr hWnd, System.Text.StringBuilder s, int n);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassName(System.IntPtr hWnd, System.Text.StringBuilder s, int n);
[DllImport("user32.dll")] public static extern bool GetWindowRect(System.IntPtr hWnd, out RECT r);
[DllImport("user32.dll")] public static extern System.IntPtr WindowFromPoint(POINT p);
[DllImport("user32.dll")] public static extern System.IntPtr GetAncestor(System.IntPtr h, uint flags);
[DllImport("user32.dll")] public static extern bool PrintWindow(System.IntPtr h, System.IntPtr hdc, uint flags);
[DllImport("user32.dll")] public static extern System.IntPtr MonitorFromWindow(System.IntPtr h, uint flags);
[DllImport("user32.dll")] public static extern bool GetMonitorInfoW(System.IntPtr hMon, ref MONITORINFO mi);
[StructLayout(LayoutKind.Sequential)] public struct MONITORINFO { public int cbSize; public RECT rcMonitor; public RECT rcWork; public uint dwFlags; }
[StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
[StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
[StructLayout(LayoutKind.Sequential)] public struct WINDOWPLACEMENT {
  public int length; public int flags; public int showCmd; public POINT ptMinPosition; public POINT ptMaxPosition;
  public RECT rcNormalPosition; }
'@

# Per-Monitor-V2（-4）：GetWindowRect／SetCursorPos 都是實體像素。
[void][V53.Native]::SetThreadDpiAwarenessContext([IntPtr](-4))

# fix F9：前置探查改在設定 DPI 感知之後（它以 GetWindowRect 與螢幕矩形判斷全螢幕覆蓋層，要與後續座標同一個
# awareness；tests/DpiAwareness.Tests.ps1 靜態檢查）。
$pf = Invoke-SafeInputPreflight
if ($pf.ExitCode -eq 2) { Write-Host $pf.Message; exit 2 }
$canInject = ($pf.ExitCode -eq 0)
Write-Host $pf.Message

$Ids = @('clock', 'macro', 'fixed', 'dynamic', 'quotes', 'custom1')
# 平時可見（有內容）者；quotes 的資料是空陣列＝無內容，只在編輯版面中顯示佔位外框。
$VisibleIds = @('clock', 'macro', 'fixed', 'dynamic', 'custom1')
function Get-Ts { Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fffK' }
function Hex([IntPtr]$h) { '0x{0:X}' -f $h.ToInt64() }
function Get-Cls([IntPtr]$h) { $sb = New-Object System.Text.StringBuilder 256; [void][V53.Native]::GetClassName($h, $sb, 256); $sb.ToString() }
function Get-WinPid([IntPtr]$h) { $p = 0; [void][V53.Native]::GetWindowThreadProcessId($h, [ref]$p); [int]$p }
function Get-Rect([IntPtr]$h) {
    $r = New-Object V53.Native+RECT
    [void][V53.Native]::GetWindowRect($h, [ref]$r)
    [PSCustomObject]@{ X = $r.Left; Y = $r.Top; W = $r.Right - $r.Left; H = $r.Bottom - $r.Top }
}
function Fmt($r) { if ($r) { "($($r.X),$($r.Y),$($r.W)x$($r.H))" } else { '<null>' } }
function Same($a, $b) { $a -and $b -and $a.X -eq $b.X -and $a.Y -eq $b.Y -and $a.W -eq $b.W -and $a.H -eq $b.H }
$GRID = 48
# design.md D7：edge(i) = 起點 + floor(i × 長度 / 48)（整數運算）。
function Edge([int]$Origin, [int]$Extent, [int]$I) { $Origin + [int][math]::Floor([int64]$I * $Extent / $GRID) }
function Grid-Rect($Wa, [int]$Col, [int]$Row, [int]$W, [int]$H) {
    $x0 = Edge $Wa.X $Wa.W $Col; $x1 = Edge $Wa.X $Wa.W ($Col + $W)
    $y0 = Edge $Wa.Y $Wa.H $Row; $y1 = Edge $Wa.Y $Wa.H ($Row + $H)
    [PSCustomObject]@{ X = $x0; Y = $y0; W = $x1 - $x0; H = $y1 - $y0 }
}
# 兩矩形是否相交（共用邊不算）。
function Test-RectsOverlap($A, $B) {
    $A.X -lt ($B.X + $B.W) -and $B.X -lt ($A.X + $A.W) -and $A.Y -lt ($B.Y + $B.H) -and $B.Y -lt ($A.Y + $A.H)
}
# fix F7：對照組的拖曳目標由當下格座標推導——舊版固定往左 700 px，會撞到本腳本自己開的 custom1
# （0,1,14,8）而彈回。列舉工作區內每個格線交點當左上角（尺寸沿用 $Moving），挑出：整個落在工作區內、
# 不與 $Others（所有已開小工具，含隱藏中仍佔格的 quotes）相交、位移 ≥ $MinDistance 者中位移最大的。
# 左上角正好在格線上，放開時的對齊不會再挪動它。回傳 @{ Dx; Dy; Target }；沒有合法目標回 $null。
function Find-FreeDragOffset($Moving, [object[]]$Others, $Work, [int]$MinDistance = 40) {
    $best = $null; $bestD = -1
    for ($r = 0; $r -le $GRID; $r++) {
        for ($c = 0; $c -le $GRID; $c++) {
            $t = [PSCustomObject]@{ X = (Edge $Work.X $Work.W $c); Y = (Edge $Work.Y $Work.H $r); W = $Moving.W; H = $Moving.H }
            if (($t.X + $t.W) -gt ($Work.X + $Work.W) -or ($t.Y + $t.H) -gt ($Work.Y + $Work.H)) { continue }
            $dx = $t.X - $Moving.X; $dy = $t.Y - $Moving.Y
            $d = [math]::Sqrt([double]$dx * $dx + [double]$dy * $dy)
            if ($d -lt $MinDistance) { continue }
            if (@($Others | Where-Object { $_ -and (Test-RectsOverlap $t $_) }).Count -gt 0) { continue }
            if ($d -gt $bestD) { $bestD = $d; $best = [PSCustomObject]@{ Dx = $dx; Dy = $dy; Target = $t } }
        }
    }
    return $best
}
function Get-WorkAreaOf([IntPtr]$H) {
    $mi = New-Object V53.Native+MONITORINFO
    $mi.cbSize = [Runtime.InteropServices.Marshal]::SizeOf($mi)
    [void][V53.Native]::GetMonitorInfoW([V53.Native]::MonitorFromWindow($H, 2), [ref]$mi)
    [PSCustomObject]@{ X = $mi.rcWork.Left; Y = $mi.rcWork.Top; W = $mi.rcWork.Right - $mi.rcWork.Left; H = $mi.rcWork.Bottom - $mi.rcWork.Top }
}

# 可見與否都找（quotes 無內容時隱藏）；回傳 HWND 或 Zero。
function Find-Window([int]$ProcId, [string]$Title, [switch]$VisibleOnly) {
    $h = [V53.Native]::GetTopWindow([IntPtr]::Zero)
    while ($h -ne [IntPtr]::Zero) {
        if ((Get-WinPid $h) -eq $ProcId -and (-not $VisibleOnly -or [V53.Native]::IsWindowVisible($h))) {
            $sb = New-Object System.Text.StringBuilder 256
            [void][V53.Native]::GetWindowText($h, $sb, 256)
            if ($sb.ToString() -eq $Title) { return $h }
        }
        $h = [V53.Native]::GetWindow($h, 2)
    }
    return [IntPtr]::Zero
}
function Wait-Window([int]$ProcId, [string]$Title, [int]$TimeoutSec = 25) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt $TimeoutSec) {
        $h = Find-Window $ProcId $Title -VisibleOnly
        if ($h -ne [IntPtr]::Zero) { return $h }
        Start-Sleep -Milliseconds 200
    }
    return [IntPtr]::Zero
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

# PrintWindow（PW_RENDERFULLCONTENT＝2）擷取單一視窗；回傳 Bitmap（呼叫端 Dispose）。
function Get-WindowBitmap([IntPtr]$H) {
    $r = Get-Rect $H
    $bmp = New-Object System.Drawing.Bitmap $r.W, $r.H
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $hdc = $g.GetHdc()
    [void][V53.Native]::PrintWindow($H, $hdc, 2)
    $g.ReleaseHdc($hdc); $g.Dispose()
    return $bmp
}
# 視窗外圈 4px（避開四角 20px 圓角區）內，接近強調色的像素比例。
function Get-RingAccentRatio($Bmp, [int[]]$Accent) {
    $hit = 0; $total = 0
    $w = $Bmp.Width; $h = $Bmp.Height
    for ($y = 0; $y -lt $h; $y++) {
        $edgeY = ($y -lt 4 -or $y -ge $h - 4)
        for ($x = 0; $x -lt $w; $x++) {
            $edgeX = ($x -lt 4 -or $x -ge $w - 4)
            if (-not ($edgeX -or $edgeY)) { if ($x -eq 4) { $x = $w - 5 }; continue }
            if (($x -lt 20 -or $x -ge $w - 20) -and ($y -lt 20 -or $y -ge $h - 20)) { continue }
            $total++
            $c = $Bmp.GetPixel($x, $y)
            if ([math]::Abs($c.R - $Accent[0]) -le 45 -and [math]::Abs($c.G - $Accent[1]) -le 45 -and [math]::Abs($c.B - $Accent[2]) -le 45) { $hit++ }
        }
    }
    if ($total -eq 0) { return 0.0 }
    return [math]::Round($hit / $total, 4)
}

# ── 前置 ─────────────────────────────────────────────────────────────────────────
if (Get-Process -Name fc-host -ErrorAction SilentlyContinue) {
    Write-Host 'BLOCKED: 已有 fc-host 在執行，本腳本不結束它；請先自行關閉。'
    exit 2
}
$Exe = (Resolve-Path $Exe).Path
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$logPath = Join-Path $OutDir '5.3-log.log'
$sumPath = Join-Path $OutDir '5.3-summary.log'
$log = New-EvidenceWriter $logPath
$log.AutoFlush = $true
function Log([string]$m) { $line = "$(Get-Ts) $m"; $log.WriteLine($line); Write-Host (ConvertTo-EvidenceText $line) }
Log "# verify-5.3.ps1 exe=$Exe preflight=$($pf.Message)"

$realSettings = Join-Path $env:APPDATA 'tw.fintools.fc-host\settings.json'
$realHashBefore = if (Test-Path $realSettings) { (Get-FileHash $realSettings).Hash } else { '<不存在>' }
$regSnap = @(Save-FcHostAutostartRegistry)
Log "# 真正的設定檔雜湊（開始）=$realHashBefore；開機自啟登錄快照：$(($regSnap | ForEach-Object { "$($_.Name)@$(Split-Path $_.Key -Leaf) Exists=$($_.Exists)" }) -join '; ')"

$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ('fc-host-5.3-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
$tempAppData = Join-Path $tempRoot 'Roaming'
$tempLocal = Join-Path $tempRoot 'Local'
$dataDir = Join-Path $tempLocal 'tw.fintools.fc-host\data'
New-Item -ItemType Directory -Force -Path $tempAppData, $dataDir | Out-Null
# 行情條改成空陣列（無內容），其餘資料照舊。
# 用 System.Text.Json 的 JsonNode 改寫（ConvertFrom-Json 會把日期字串轉成 DateTime，改變格式）。
$events = [System.Text.Json.Nodes.JsonNode]::Parse([IO.File]::ReadAllText($DataFile))
$events['quotes'] = [System.Text.Json.Nodes.JsonNode]::Parse('[]')
[IO.File]::WriteAllText((Join-Path $dataDir 'tw_events.json'), $events.ToJsonString(), (New-Object System.Text.UTF8Encoding($false)))
$settingsJsonPath = Join-Path $tempAppData 'tw.fintools.fc-host\settings.json'
Log "# 暫存 APPDATA=$tempAppData LOCALAPPDATA=$tempLocal（資料：$DataFile 複本）"

$results = [ordered]@{}
$notRun = New-Object System.Collections.Generic.List[string]
$script:mouseDown = $false
$hostProc = $null
$minimized = New-Object System.Collections.Generic.List[object]
# hung-target（review 642050f）：沒還原成功的使用者視窗（無回應、最小化未確認、讀回逾時／不符）。屬環境問題，
# 不進逐項結果；摘要另起警示行（Format-UnrestoredWarning），結束碼經 Get-VerdictExitCode（環境＝3）。
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
# 真實拖曳（按下 → 分段移動 → 放開），全程經 SafeInput。
function Invoke-Drag([int]$FromX, [int]$FromY, [int]$ToX, [int]$ToY, [int]$Steps = 12) {
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
    }
    Invoke-SafeInputSleep 250
    Invoke-GuardedMouse 4 -What '拖曳放開（LEFTUP）'
    $script:mouseDown = $false
    Invoke-SafeInputSleep 600
}

$lockedExpr = "window.__TAURI__.core.invoke('get_settings').then(s => s.layout_locked)"
# 狀態字串（見下方 $stateExpr；host-cdp-eval 以 JSON 印出，前後有引號、內部引號被跳脫）：
#   鎖定／離開後：沒有 edit-mode、拖曳區為 null、::before 的 content 為 none。
#   編輯版面中：edit-mode、拖曳區 deep、::before 有 content（不是 none）且為 dashed（寬度為 2 CSS px 對齊裝置像素後的值）、fixed。
$script:LockedPattern = '^"?false\|null\|none\|'
$script:EditPattern = '^"?true\|deep\|(?!none\|)[^|]*\|dashed\|[\d.]+px\|fixed\|'
# 每頁狀態：edit-mode class、拖曳區、::before 外框樣式、佔位外框是否可見。
$stateExpr = "(() => { const b = document.body; const c = document.getElementById('widget-root').firstElementChild; const s = getComputedStyle(b, '::before'); const ph = document.querySelector('.edit-placeholder'); const pr = ph ? ph.getBoundingClientRect() : null; return [b.classList.contains('edit-mode'), c ? String(c.getAttribute('data-tauri-drag-region')) : 'no-container', s.content, s.borderTopStyle, s.borderTopWidth, s.position, ph ? getComputedStyle(ph).display : 'none', pr ? Math.round(pr.width) + 'x' + Math.round(pr.height) : '0x0'].join('|'); })()"

try {
    Assert-SessionUnlocked '腳本啟動前置檢查'
    $hostProc = Start-Host
    Log "# 宿主 pid=$($hostProc.Id)"
    $clock = Wait-Window $hostProc.Id 'fc-host clock'
    if ($clock -eq [IntPtr]::Zero) { throw 'clock 視窗未出現' }
    foreach ($id in $Ids[1..3]) { [void](Wait-Window $hostProc.Id "fc-host $id") }
    Start-Sleep -Seconds 3
    $rect0 = Get-Rect $clock
    Log "## clock 初始=$(Fmt $rect0)"
    $accentHex = (Invoke-Eval 'w=clock' "window.__TAURI__.core.invoke('get_settings').then(s => s.accent_color)").Trim('"')
    $accent = @([Convert]::ToInt32($accentHex.Substring(1, 2), 16), [Convert]::ToInt32($accentHex.Substring(3, 2), 16), [Convert]::ToInt32($accentHex.Substring(5, 2), 16))
    Log "## 強調色=$accentHex → RGB($($accent -join ','))"

    # ── 1. 預設鎖定；quotes 無內容＝隱藏；開啟 custom1──────────────────────────────────
    $locked0 = Wait-Eval 'w=clock' $lockedExpr 'true|false'
    $results['1：首次啟動 layout_locked=true（預設鎖定）'] = ($locked0 -match 'true')
    Invoke-Eval 'w=clock' "window.__TAURI__.core.invoke('update_settings', { patch: { widgets: { custom1: { enabled: true } } } }).then(s => s.widgets.custom1.enabled)" | Out-Null
    Start-Sleep -Seconds 3
    $c1Hwnd = Find-Window $hostProc.Id 'fc-host custom1' -VisibleOnly
    $qHwnd = Find-Window $hostProc.Id 'fc-host quotes'
    $qVisible = ($qHwnd -ne [IntPtr]::Zero -and [V53.Native]::IsWindowVisible($qHwnd))
    Log "## 開啟 custom1：可見視窗 $(if ($c1Hwnd -ne [IntPtr]::Zero) { Hex $c1Hwnd } else { '<無>' })；quotes 視窗 $(if ($qHwnd -ne [IntPtr]::Zero) { Hex $qHwnd } else { '<未建立>' }) 可見=$qVisible"
    $results['1：custom1 開啟後顯示（尚未設定＝有內容）'] = ($c1Hwnd -ne [IntPtr]::Zero)
    $results['1：quotes 已建立、行情為空陣列＝無內容所以平時隱藏'] = ($qHwnd -ne [IntPtr]::Zero -and -not $qVisible)

    $ratio0 = @{}
    $allLockedClean = $true
    foreach ($id in $VisibleIds) {
        $st = Invoke-Eval "w=$id" $stateExpr
        $h = Find-Window $hostProc.Id "fc-host $id"
        $bmp = Get-WindowBitmap $h; $ratio0[$id] = Get-RingAccentRatio $bmp $accent
        if ($id -eq 'clock') { $bmp.Save((Join-Path $OutDir '5.3-locked-clock.png'), [System.Drawing.Imaging.ImageFormat]::Png) }
        $bmp.Dispose()
        Log "  鎖定 $id：狀態=$st 外圈強調色比例=$($ratio0[$id])"
        if ($st -notmatch $script:LockedPattern) { $allLockedClean = $false }
    }
    $results['1：鎖定時可見的小工具（四個財經＋custom1）都沒有 edit-mode、拖曳區與外框'] = $allLockedClean

    # ── 2. 鎖定時真實拖曳 clock → 矩形不變 ─────────────────────────────────────────────
    # 三次拖曳（2 鎖定、3b 對照組、5 離開後）用同一個位移 ($dragDx,$dragDy)：由當下格座標推導的合法
    # 目標（工作區內、不與任何已開小工具相交，含本腳本開的 custom1 與隱藏中仍佔格的 quotes）。
    # 步驟 5 用反向位移（回到 3b 之前的位置，那裡此刻是空的）。
    $cx = [int]($rect0.X + $rect0.W / 2); $cy = [int]($rect0.Y + $rect0.H / 2)
    $waClock = Get-WorkAreaOf $clock
    $otherRects = @(foreach ($id in $Ids) {
            if ($id -eq 'clock') { continue }
            $h = Find-Window $hostProc.Id "fc-host $id"
            if ($h -ne [IntPtr]::Zero) { $r = Get-Rect $h; Log "## 已開小工具 $id=$(Fmt $r)"; $r }
        })
    $free = Find-FreeDragOffset $rect0 $otherRects $waClock
    $dragDx = 0; $dragDy = 0
    if ($free) {
        $dragDx = $free.Dx; $dragDy = $free.Dy
        Log "## 拖曳手勢：位移 ($dragDx,$dragDy) px → 對照組目標 $(Fmt $free.Target)（工作區 $(Fmt $waClock)）"
    } else {
        Log "## 工作區 $(Fmt $waClock) 內找不到 clock 的合法空位，2／3b／5 不執行"
    }
    $canDrag = ($canInject -and $null -ne $free)
    $noDragWhy = if (-not $canInject) { '合成輸入無效' } else { '找不到不與已開小工具相交的合法目標' }
    $rA = $null
    if ($canDrag) {
        Assert-OccluderResult (Clear-Occluders -HostPid $hostProc.Id -Points @(, @($cx, $cy)) -Minimized $minimized -Log $occLog)
        Invoke-Drag $cx $cy ($cx + $dragDx) ($cy + $dragDy)
        $rA = Get-Rect $clock
        Invoke-Eval 'w=clock' "(() => { window.getSelection().removeAllRanges(); return 'ok'; })()" | Out-Null
        Log "## 2 鎖定時拖曳 clock 後=$(Fmt $rA)"
    } else { $notRun.Add("2：鎖定時真實拖曳（$noDragWhy，未執行）") }

    # ── 3. 進入編輯版面 ───────────────────────────────────────────────────────────────
    Invoke-Eval 'w=clock' "window.__TAURI__.core.invoke('set_edit_mode', { enabled: true })" | Out-Null
    $locked1 = Wait-Eval 'w=clock' $lockedExpr 'false'
    Log "## 3 進入編輯版面：layout_locked=$locked1"
    $results['3：set_edit_mode(true) → layout_locked=false'] = ($locked1 -match 'false')
    Start-Sleep -Seconds 2
    $allFramed = $true; $allVisibleRing = $true
    foreach ($id in $Ids) {
        $st = Wait-Eval "w=$id" $stateExpr $script:EditPattern
        $h = Find-Window $hostProc.Id "fc-host $id"
        $vis = ($h -ne [IntPtr]::Zero -and [V53.Native]::IsWindowVisible($h))
        $ratio = -1
        if ($vis) {
            $bmp = Get-WindowBitmap $h; $ratio = Get-RingAccentRatio $bmp $accent
            if ($id -in 'clock', 'quotes') { $bmp.Save((Join-Path $OutDir "5.3-edit-$id.png"), [System.Drawing.Imaging.ImageFormat]::Png) }
            $bmp.Dispose()
        }
        $base = if ($ratio0.ContainsKey($id)) { $ratio0[$id] } else { 0 }
        Log "  編輯 $id：可見=$vis 狀態=$st 外圈強調色比例=$ratio（鎖定時 $base） rect=$(if ($vis) { Fmt (Get-Rect $h) } else { '-' })"
        $framed = $vis -and ($st -match $script:EditPattern)
        if (-not $framed) { $allFramed = $false }
        if (-not ($vis -and $ratio -ge 0.10 -and $ratio -ge $base + 0.08)) { $allVisibleRing = $false }
        if ($id -eq 'quotes') {
            $results['3：無內容的 quotes 在編輯版面中顯示出來、佔位外框（.edit-placeholder）可見'] = ($vis -and $st -match '\|flex\|[1-9]\d*x[1-9]\d*"?$')
        }
    }
    $results['3：所有開啟中小工具都有 edit-mode、拖曳區 deep 與 body::before 虛線外框（dashed、fixed）'] = $allFramed
    $results['3：虛線外框實際畫在視窗內（PrintWindow 外圈 4px 強調色比例 ≥10% 且比鎖定時高 8 個百分點以上）'] = $allVisibleRing

    # ── 3b. 對照組：編輯版面（解鎖）中同一個手勢會移動 clock ─────────────────────────────
    $controlMoved = $false
    if ($canDrag) {
        $rC0 = Get-Rect $clock
        $ccx = [int]($rC0.X + $rC0.W / 2); $ccy = [int]($rC0.Y + $rC0.H / 2)
        Assert-OccluderResult (Clear-Occluders -HostPid $hostProc.Id -Points @(, @($ccx, $ccy)) -Minimized $minimized -Log $occLog)
        Invoke-Drag $ccx $ccy ($ccx + $dragDx) ($ccy + $dragDy)
        Start-Sleep -Seconds 1   # 放開後的對齊／存檔／重新推導
        $rC = Get-Rect $clock
        Invoke-Eval 'w=clock' "(() => { window.getSelection().removeAllRanges(); return 'ok'; })()" | Out-Null
        $controlMoved = -not (Same $rC $rC0)
        Log "## 3b 對照組（編輯版面、解鎖）同一手勢拖曳 clock：前=$(Fmt $rC0) 後=$(Fmt $rC) 移動=$controlMoved"
        $results['3b：對照組——編輯版面中同一個拖曳手勢確實移動 clock'] = $controlMoved
        # 尺寸由宿主依格線重算，可能差 1 px；左上角在格線上，必須正好等於推導的目標。
        $results['3b：對照組放開後左上角＝推導的合法目標（沒有彈回）'] = ($rC.X -eq $free.Target.X -and $rC.Y -eq $free.Target.Y)
        if ($rA) {
            $results['2：鎖定時真實拖曳 clock，矩形不變（且對照組 3b 確實移動）'] = (Same $rA $rect0) -and $controlMoved
        }
    } else { $notRun.Add("3b：對照組拖曳（$noDragWhy，未執行）") }

    # ── 4. 離開編輯版面 ───────────────────────────────────────────────────────────────
    Invoke-Eval 'w=clock' "window.__TAURI__.core.invoke('set_edit_mode', { enabled: false })" | Out-Null
    $locked2 = Wait-Eval 'w=clock' $lockedExpr 'true'
    Start-Sleep -Seconds 2
    $onDisk = (Get-Content $settingsJsonPath -Raw | ConvertFrom-Json).layout_locked
    Log "## 4 離開編輯版面：layout_locked=$locked2 settings.json layout_locked=$onDisk"
    $results['4：set_edit_mode(false) → layout_locked=true 且已落地存檔'] = ($locked2 -match 'true' -and $onDisk -eq $true)
    $allCleared = $true
    foreach ($id in $VisibleIds) {
        $st = Wait-Eval "w=$id" $stateExpr $script:LockedPattern
        $h = Find-Window $hostProc.Id "fc-host $id"
        $bmp = Get-WindowBitmap $h; $ratio = Get-RingAccentRatio $bmp $accent
        if ($id -eq 'clock') { $bmp.Save((Join-Path $OutDir '5.3-after-exit-clock.png'), [System.Drawing.Imaging.ImageFormat]::Png) }
        $bmp.Dispose()
        Log "  離開後 $id：狀態=$st 外圈強調色比例=$ratio（鎖定時 $($ratio0[$id])）"
        if (-not ($st -match $script:LockedPattern -and $ratio -le $ratio0[$id] + 0.02)) { $allCleared = $false }
    }
    $results['4：可見的小工具的 edit-mode、拖曳區與外框都消失（外圈強調色回到鎖定時水準）'] = $allCleared
    Start-Sleep -Seconds 1
    $qAfter = ($qHwnd -ne [IntPtr]::Zero -and [V53.Native]::IsWindowVisible($qHwnd))
    Log "## 4 quotes 可見=$qAfter"
    $results['4：無內容的 quotes 回到隱藏'] = (-not $qAfter)

    # ── 5. 離開後再拖 → 不動 ────────────────────────────────────────────────────────
    if ($canDrag) {
        $rB0 = Get-Rect $clock
        # 對照組已把 clock 移走，起點改用目前的中心；位移取反向（回到 3b 之前的位置，此刻是空的，
        # 若沒鎖住就會是合法的移動）。
        $bx = [int]($rB0.X + $rB0.W / 2); $by = [int]($rB0.Y + $rB0.H / 2)
        Assert-OccluderResult (Clear-Occluders -HostPid $hostProc.Id -Points @(, @($bx, $by)) -Minimized $minimized -Log $occLog)
        Invoke-Drag $bx $by ($bx - $dragDx) ($by - $dragDy)
        $rB = Get-Rect $clock
        Invoke-Eval 'w=clock' "(() => { window.getSelection().removeAllRanges(); return 'ok'; })()" | Out-Null
        Log "## 5 離開後拖曳 clock（位移 ($(-$dragDx),$(-$dragDy)) px）：前=$(Fmt $rB0) 後=$(Fmt $rB)"
        $results['5：離開編輯版面後真實拖曳 clock，矩形不變（且對照組 3b 確實移動）'] = (Same $rB $rB0) -and $controlMoved
    } else { $notRun.Add("5：離開後真實拖曳（$noDragWhy，未執行）") }
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
    try { Stop-HostTree $hostProc } catch { Log "## 收尾結束宿主失敗：$_" }
    try { Restore-Occluders -Minimized $minimized -NotRestored $occNotRestored -Log $occLog } catch { Log "## 還原被最小化的視窗失敗：$_" }
    # $minimized 非空＝Restore-Occluders 本身中途丟例外、沒跑完；一併列為未還原（環境，不進逐項結果）。
    if ($minimized.Count -gt 0) { $occNotRestored.Add("Restore-Occluders 未跑完：尚有 $($minimized.Count) 扇未處理（見記錄）") }
    try {
        $regLeft = @(Restore-FcHostAutostartRegistry $regSnap)
        if ($regLeft.Count -gt 0) { Write-Warning "開機自啟登錄未還原：$($regLeft -join '; ')" }
        Log "# 開機自啟登錄還原：未還原 $($regLeft.Count) 項 $($regLeft -join '; ')"
        $results['開機自啟登錄已還原（Run／StartupApproved 的 fc-host）'] = ($regLeft.Count -eq 0)
    } catch {
        Log "## 收尾開機自啟登錄還原失敗：$_"
        $results['開機自啟登錄已還原（Run／StartupApproved 的 fc-host）'] = $false
    }
    $realHashAfter = if (Test-Path $realSettings) { (Get-FileHash $realSettings).Hash } else { '<不存在>' }
    Log "# 真正的設定檔雜湊（結束）=$realHashAfter；與開始相同=$($realHashAfter -eq $realHashBefore)"
    $results['隔離：真正的設定檔雜湊前後相同'] = ($realHashAfter -eq $realHashBefore)
    Log '# 結束'
    $log.Close()
    Remove-Item -Recurse -Force $tempRoot -ErrorAction SilentlyContinue
}

$occWarn = Format-UnrestoredWarning -NotRestored $occNotRestored
$sum = New-EvidenceWriter $sumPath
$sum.WriteLine("# verify-5.3.ps1 summary $(Get-Ts)")
foreach ($k in $results.Keys) { $sum.WriteLine("$(if ($results[$k]) { 'PASS' } else { 'FAIL' })  $k") }
foreach ($n in $notRun) { $sum.WriteLine("NOT-RUN  $n") }
if ($script:envBlocked) { $sum.WriteLine("ENV-BLOCKED  $($script:envBlocked)") }
if ($occWarn) { $sum.WriteLine($occWarn) }
$sum.Close()
Get-Content $sumPath
if ($occWarn) { Write-Host $occWarn -ForegroundColor Yellow }
# 結束碼優先序：產品 FAIL（1）＞ 環境（3，含 NOT-RUN 與使用者視窗未還原）。
exit (Get-VerdictExitCode -Results $results -EnvBlocked:([bool]$script:envBlocked -or $notRun.Count -gt 0) -NotRestored $occNotRestored)
