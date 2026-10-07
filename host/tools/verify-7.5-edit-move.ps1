<#
.SYNOPSIS
    Task 7.5 驗收驅動腳本（真實滑鼠拖曳；design.md D7「編輯版面」、D4 `edit-preview`；
    specs/widget-host-windows「編輯版面」）：經 `host/tools/lib/SafeInput.psm1` 注入滑鼠拖曳，
    驗證移動的合法判斷、紅框預告與彈回。

.DESCRIPTION
    **每一次滑鼠移動／按下／放開之前都由 `SafeInput.psm1` 重新檢查工作階段是否鎖定**
    （`LogonUI.exe`），鎖定即丟 `BLOCKED:` 終止例外、不重試；開頭先跑
    `Invoke-SafeInputPreflight`，合成輸入沒有被系統計入時回報 ENV-BLOCKED（結束碼 3）後離開，
    不產生無資訊的 PASS/FAIL、不繞過。執行途中被鎖定時（SafeInput 丟 `BLOCKED:`），摘要記一行
    BLOCKED、以結束碼 2 離開（結果不完整，不是功能失敗）；其餘失敗結束碼 1。

    隔離：全新暫存 %APPDATA%／%LOCALAPPDATA%（首次啟動，預設版面、`layout_locked=true`），資料
    目錄放 `-DataFile`（預設 `D:\finance-calendar\tw_events.json`）的複本；真正的使用者設定檔
    只在開始與結束各算一次雜湊比對（不讀寫內容）。HKCU `Run\fc-host` 前後比對，不同就還原。

    遮擋：小工具永遠在最底層，其他視窗（例如最大化的編輯器）會蓋住它、讓按下打不到小工具。
    腳本經 `lib/Occluders.psm1` 檢查每個按下點的根視窗是否屬於宿主，被一般應用程式主視窗（白名單）
    蓋住就暫時最小化，結束時以 `Restore-Occluders` 還原（含吸附與最大化）。遮擋者是工作列或不在
    白名單（系統 UI、對話框等）＝ENV-BLOCKED（結束碼 3）；被本腳本的前景基準表單蓋住＝腳本配置錯誤
    （FAIL）（fix F8）。按下點命中桌面（Progman／WorkerW）、宿主的另一扇視窗或沒命中任何視窗＝小工具
    不在預期位置或沉到桌面之下＝FAIL（fix F8b，判讀經 lib 的 Assert-OccluderResult）。

    前景基準：另開一個小視窗（本腳本啟動的 pwsh 子行程裡的 WinForms 表單＋TextBox，放在左下
    不會被拖曳路徑用到的位置）並設為前景，記下 `GetForegroundWindow` 與該執行緒
    `GetGUIThreadInfo().hwndFocus`；拖曳全程每一步都取樣比對（`GetFocus` 只能查呼叫者自己
    的執行緒，跨執行緒要用 `GetGUIThreadInfo`，Microsoft Learn）。

    步驟（clock 小工具，筆電主螢幕）：
      A. 鎖定（未進入編輯版面）時真實拖曳 → 矩形不變。
      進入編輯版面（CDP `invoke('set_edit_mode')`，非輸入注入）。
      1. 拖到空白處（左上角落在格線 (8,2) 偏幾個像素）放開 → 視窗四邊落在格線、w/h 格數不變；
         `get_settings()` 記錄為 (8,2,16,10)；全程前景／焦點不變。
      2. 拖到與總經日曆重疊處按住不放 → 頁面 `body.edit-invalid` 出現（CDP 讀 DOM）＋逐窗
         `PrintWindow` 裁切截圖（只含 clock 視窗本身）；放開 → 矩形回到步驟 1 的結果、紅框
         消失（頁面 MutationObserver 記錄 class 變化序列）、記錄未變；全程前景／焦點不變。
      2b.（fix round 1）關掉 dynamic 後拖到左上角 (38,20)（38+16＞48，超出右緣）按住 → 紅框；
         放開 → 彈回步驟 1 的矩形、紅框消失、記錄未變。
      離開編輯版面。
      A'. 離開編輯版面後真實拖曳 → 矩形不變。
      3. 結束宿主（強制結束；放開即存檔）→ 以同一組暫存目錄重啟 → clock 矩形與步驟 1 相同。
      跨顯示器：偵測到第二台顯示器才跑；只有一台時記「待補（需外接螢幕）」。

.PARAMETER Exe
    fc-host.exe 路徑，預設 host/target/release/fc-host.exe（應為**不含** self-test-ipc 的建置）。
#>
[CmdletBinding()]
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\fc-host.exe'),
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [string]$DataFile = 'D:\finance-calendar\tw_events.json',
    [int]$CdpPort = 9375
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\AutostartRegistry.psm1') -Force

Import-Module (Join-Path $PSScriptRoot 'lib\SafeInput.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\Occluders.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\VerifyVerdict.psm1') -Force

Add-Type -AssemblyName System.Drawing
Add-Type -Namespace V75 -Name Native -MemberDefinition @'
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
[DllImport("user32.dll")] public static extern bool PrintWindow(System.IntPtr h, System.IntPtr hdc, uint flags);
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

# Per-Monitor-V2（-4）：GetWindowRect／GetMonitorInfo／SetCursorPos 都是實體像素。
[void][V75.Native]::SetThreadDpiAwarenessContext([IntPtr](-4))

# fix F9：前置探查改在設定 DPI 感知之後（它以 GetWindowRect 與螢幕矩形判斷全螢幕覆蓋層，要與後續座標同一個
# awareness；tests/DpiAwareness.Tests.ps1 靜態檢查）。
$pf = Invoke-SafeInputPreflight
if ($pf.ExitCode -ne 0) { Write-Host $pf.Message; exit $pf.ExitCode }
Write-Host $pf.Message

$GRID = 48
function Get-Ts { Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fffK' }
function Hex([IntPtr]$h) { '0x{0:X}' -f $h.ToInt64() }
function Get-Cls([IntPtr]$h) { $sb = New-Object System.Text.StringBuilder 256; [void][V75.Native]::GetClassName($h, $sb, 256); $sb.ToString() }
function Get-WinPid([IntPtr]$h) { $p = 0; [void][V75.Native]::GetWindowThreadProcessId($h, [ref]$p); [int]$p }
function Get-WinTid([IntPtr]$h) { $p = 0; [int][V75.Native]::GetWindowThreadProcessId($h, [ref]$p) }
function Get-Rect([IntPtr]$h) {
    $r = New-Object V75.Native+RECT
    [void][V75.Native]::GetWindowRect($h, [ref]$r)
    [PSCustomObject]@{ X = $r.Left; Y = $r.Top; W = $r.Right - $r.Left; H = $r.Bottom - $r.Top }
}
function Fmt($r) { if ($r) { "($($r.X),$($r.Y),$($r.W)x$($r.H))" } else { '<null>' } }
function Same($a, $b) { $a -and $b -and $a.X -eq $b.X -and $a.Y -eq $b.Y -and $a.W -eq $b.W -and $a.H -eq $b.H }
# design.md D7：edge(i) = 起點 + floor(i × 長度 / 48)（整數運算）。
function Edge([int]$Origin, [int]$Extent, [int]$I) { $Origin + [int][math]::Floor([int64]$I * $Extent / $GRID) }
function Grid-Rect($Wa, [int]$Col, [int]$Row, [int]$W, [int]$H) {
    $x0 = Edge $Wa.X $Wa.W $Col; $x1 = Edge $Wa.X $Wa.W ($Col + $W)
    $y0 = Edge $Wa.Y $Wa.H $Row; $y1 = Edge $Wa.Y $Wa.H ($Row + $H)
    [PSCustomObject]@{ X = $x0; Y = $y0; W = $x1 - $x0; H = $y1 - $y0 }
}

function Get-Monitors {
    $list = New-Object System.Collections.Generic.List[object]
    $cb = [V75.Native+MonEnum] {
        param($h, $dc, $r, $d)
        $mi = New-Object V75.Native+MONITORINFO
        $mi.cbSize = [Runtime.InteropServices.Marshal]::SizeOf($mi)
        [void][V75.Native]::GetMonitorInfoW($h, [ref]$mi)
        $list.Add([PSCustomObject]@{
                Primary = [bool]($mi.dwFlags -band 1)
                Work    = [PSCustomObject]@{ X = $mi.rcWork.Left; Y = $mi.rcWork.Top; W = $mi.rcWork.Right - $mi.rcWork.Left; H = $mi.rcWork.Bottom - $mi.rcWork.Top }
            })
        $true
    }
    [void][V75.Native]::EnumDisplayMonitors([IntPtr]::Zero, [IntPtr]::Zero, $cb, [IntPtr]::Zero)
    return $list.ToArray()
}

function Find-Window([int]$ProcId, [string]$Title) {
    $h = [V75.Native]::GetTopWindow([IntPtr]::Zero)
    while ($h -ne [IntPtr]::Zero) {
        if ((Get-WinPid $h) -eq $ProcId -and [V75.Native]::IsWindowVisible($h)) {
            $sb = New-Object System.Text.StringBuilder 256
            [void][V75.Native]::GetWindowText($h, $sb, 256)
            if ($sb.ToString() -eq $Title) { return $h }
        }
        $h = [V75.Native]::GetWindow($h, 2)
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
# 讀到「連續兩次相同」才算穩定（放開後的重新推導在 WM_EXITSIZEMOVE 之後才套用）。
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

function Get-FocusState([uint32]$Tid) {
    $info = New-Object V75.Native+GUITHREADINFO
    $info.cbSize = [Runtime.InteropServices.Marshal]::SizeOf($info)
    if ([V75.Native]::GetGUIThreadInfo($Tid, [ref]$info)) { return $info.hwndFocus }
    return [IntPtr]::Zero
}

# 逐窗 PrintWindow（PW_RENDERFULLCONTENT＝2）：只擷取該視窗自己的內容，不含桌面與其他視窗。
function Save-WindowPng([IntPtr]$H, [string]$Path) {
    $r = Get-Rect $H
    $bmp = New-Object System.Drawing.Bitmap $r.W, $r.H
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $hdc = $g.GetHdc()
    $ok = [V75.Native]::PrintWindow($H, $hdc, 2)
    $g.ReleaseHdc($hdc); $g.Dispose()
    $bmp.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    return $ok
}

# ── 前置 ─────────────────────────────────────────────────────────────────────────
if (Get-Process -Name fc-host -ErrorAction SilentlyContinue) {
    Write-Host 'BLOCKED: 已有 fc-host 在執行，本腳本不結束它；請先自行關閉。'
    exit 2
}
$Exe = (Resolve-Path $Exe).Path
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$logPath = Join-Path $OutDir '7.5-edit-move.log'
$sumPath = Join-Path $OutDir '7.5-edit-move-summary.log'
$zorderPath = Join-Path $OutDir '7.5-zorder.log'
$pngPath = Join-Path $OutDir '7.5-red-frame.png'
$zorderErr = Join-Path ([IO.Path]::GetTempPath()) ('fc-host-7.5-zorder-' + [guid]::NewGuid().ToString('N').Substring(0, 8) + '.err')
$log = New-EvidenceWriter $logPath
$log.AutoFlush = $true
function Log([string]$m) { $line = "$(Get-Ts) $m"; $log.WriteLine($line); Write-Host $line }
Log "# verify-7.5-edit-move.ps1 exe=$Exe preflight=$($pf.Message)"

$realSettings = Join-Path $env:APPDATA 'tw.fintools.fc-host\settings.json'
$realHashBefore = if (Test-Path $realSettings) { (Get-FileHash $realSettings).Hash } else { '<不存在>' }
$regSnap = @(Save-FcHostAutostartRegistry)
Log "# 真正的設定檔雜湊（開始）=$realHashBefore；開機自啟登錄快照：$(($regSnap | ForEach-Object { "$($_.Name)@$(Split-Path $_.Key -Leaf) Exists=$($_.Exists)" }) -join '; ')"

$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ('fc-host-7.5-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
$tempAppData = Join-Path $tempRoot 'Roaming'
$tempLocal = Join-Path $tempRoot 'Local'
$dataDir = Join-Path $tempLocal 'tw.fintools.fc-host\data'
New-Item -ItemType Directory -Force -Path $tempAppData, $dataDir | Out-Null
if (Test-Path $DataFile) { Copy-Item $DataFile (Join-Path $dataDir 'tw_events.json') }
Log "# 暫存 APPDATA=$tempAppData LOCALAPPDATA=$tempLocal（資料：$DataFile 複本）"

$results = [ordered]@{}
$pending = [ordered]@{}
$script:mouseDown = $false
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

# 按下點必須真的落在小工具上。fix F8：遮擋處理一律經 lib/Occluders.psm1 的 Clear-Occluders（白名單；
# 殼層、系統 UI、對話框等不動）。fix F8b：判讀經 lib 的 Assert-OccluderResult——工作列與系統 UI＝環境
# → ENV-BLOCKED（結束碼 3）；命中桌面、宿主的另一扇視窗或沒命中任何視窗＝小工具不在預期位置或沉到
# 桌面之下，被本腳本的前景基準表單蓋住＝腳本配置錯誤 → 一般例外（FAIL）。
$script:envBlocked = $null
$occLog = { param($m) Log "## $m" }
function Get-OwnPids { if ($fgProc) { @($fgProc.Id) } else { @() } }

# 拖曳：按下 → 等系統移動迴圈開始 → 分段移動（每步取樣前景／焦點）→ 可選按住回呼 → 放開。
function Invoke-Drag {
    param([int]$FromX, [int]$FromY, [int]$ToX, [int]$ToY, [int]$Steps = 16, [scriptblock]$WhileHeld)
    $samples = New-Object System.Collections.Generic.List[object]
    $sample = { $samples.Add([PSCustomObject]@{ Fg = [V75.Native]::GetForegroundWindow(); Focus = (Get-FocusState $script:fgTid) }) }
    Set-GuardedCursorPos $FromX $FromY -What '拖曳起點'
    Invoke-SafeInputSleep 150
    & $sample
    $script:mouseDown = $true
    Invoke-GuardedMouse 2 -What '拖曳按下（LEFTDOWN）'
    Invoke-SafeInputSleep 350
    for ($i = 1; $i -le $Steps; $i++) {
        $x = $FromX + [int][math]::Round(($ToX - $FromX) * $i / $Steps)
        $y = $FromY + [int][math]::Round(($ToY - $FromY) * $i / $Steps)
        Set-GuardedCursorPos $x $y -What "拖曳中（第 $i/$Steps 步）"
        Invoke-SafeInputSleep 45
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

$clockExpr = "window.__TAURI__.core.invoke('get_settings').then(s => { const p = s.widgets.clock.placement; return [p.col, p.row, p.w, p.h].join(','); })"
# fix F1（review 7.5 L5）：記錄比對要錨定整個輸出——host-cdp-eval.mjs 以 JSON 印出結果（字串帶
# 雙引號），CDP 失敗時輸出的是錯誤訊息；只用 -match '8,2,16,10' 子字串比對，錯誤訊息或其他數字
# 剛好含這段時也會成立。
function Test-ClockRecord([string]$Out, [string]$Expected) { return ($Out -ceq ('"' + $Expected + '"')) }
# 頁面紅框記錄：observer 有裝上才回 "REC:..."，否則回 "NOREC"；CDP 失敗時是錯誤訊息。只有以
# "REC: 開頭的輸出才算取得記錄，避免 eval 失敗時 -notmatch 'invalid=true' 空真 PASS。
$previewExpr = "Array.isArray(window.__rec75) ? 'REC:' + window.__rec75.join(' | ') : 'NOREC'"
function Test-PreviewRecorded([string]$Out) { return ($Out -match '^"REC:') }
$script:blocked = $null

try {
    Assert-SessionUnlocked '腳本啟動前置檢查'
    $mons = @(Get-Monitors)
    foreach ($m in $mons) { Log "# 顯示器 primary=$($m.Primary) 工作區=$(Fmt $m.Work)" }
    $wa = ($mons | Where-Object Primary | Select-Object -First 1).Work

    # z-order／前景監控（背景，涵蓋整段）。
    $zorderProc = Start-Process pwsh -PassThru -WindowStyle Hidden -RedirectStandardError $zorderErr -ArgumentList @(
        '-NoProfile', '-File', (Join-Path $PSScriptRoot 'watch-zorder.ps1'), '-ProcessName', 'fc-host',
        '-OutFile', $zorderPath, '-DurationSec', '240', '-IntervalMs', '200', '-Quiet')
    Start-Sleep -Seconds 2

    $hostProc = Start-Host
    Log "# 宿主 pid=$($hostProc.Id)"
    $clock = Wait-Window $hostProc.Id 'fc-host clock'
    if ($clock -eq [IntPtr]::Zero) { throw 'clock 視窗未出現' }
    [void](Wait-Window $hostProc.Id 'fc-host macro')
    Start-Sleep -Seconds 3
    $rect0 = Get-Rect $clock
    $macroRect = Get-Rect (Find-Window $hostProc.Id 'fc-host macro')
    Log "## clock 初始=$(Fmt $rect0) macro=$(Fmt $macroRect)"
    $results['首次啟動：clock 在預設格 (15,1,16,10)'] = Same $rect0 (Grid-Rect $wa 15 1 16 10)

    # 前景基準視窗：左下角的小表單（不在任何拖曳路徑／按下點上）。
    $fgTitle = 'fc-host-7.5-foreground-' + [guid]::NewGuid().ToString('N').Substring(0, 6)
    # 以隱藏主控台啟動（不另開終端機視窗）。STARTUPINFO 的 SW_HIDE 會套到行程「第一次」
    # ShowWindow（Microsoft Learn ShowWindow：nCmdShow 第一次呼叫時被忽略），所以先 Show／Hide
    # 一次把它用掉，Application.Run 的第二次顯示才會真的出現。
    $formCmd = "Add-Type -AssemblyName System.Windows.Forms; `$f = New-Object Windows.Forms.Form; `$f.Text = '$fgTitle'; `$t = New-Object Windows.Forms.TextBox; `$t.Multiline = `$true; `$t.Dock = 'Fill'; `$f.Controls.Add(`$t); `$f.Show(); `$f.Hide(); [Windows.Forms.Application]::Run(`$f)"
    $fgProc = Start-Process pwsh -PassThru -WindowStyle Hidden -ArgumentList @('-NoProfile', '-Command', $formCmd)
    $fgHwnd = Wait-Window $fgProc.Id $fgTitle
    if ($fgHwnd -eq [IntPtr]::Zero) { throw '前景基準視窗未出現' }
    [void][V75.Native]::SetWindowPos($fgHwnd, [IntPtr]::Zero, $wa.X + 20, $wa.Y + $wa.H - 360, 420, 220, 0x0014) # NOZORDER|NOACTIVATE

    # 按下點：初始 clock 中心、步驟 1 之後的 clock 中心。
    $target1 = Grid-Rect $wa 8 2 16 10
    $p0 = @([int]($rect0.X + $rect0.W / 2), [int]($rect0.Y + $rect0.H / 2))
    $p1 = @([int]($target1.X + $target1.W / 2), [int]($target1.Y + $target1.H / 2))
    Assert-OccluderResult (Clear-Occluders -HostPid $hostProc.Id -Points @($p0, $p1) -Minimized $minimized -OwnPids (Get-OwnPids) -Log $occLog)

    Send-GuardedAltTap
    [void][V75.Native]::SetForegroundWindow($fgHwnd)
    Start-Sleep -Milliseconds 500
    $script:baseFg = [V75.Native]::GetForegroundWindow()
    $script:fgTid = [uint32](Get-WinTid $fgHwnd)
    $script:baseFocus = Get-FocusState $script:fgTid
    Log "## 前景基準：fg=$(Hex $script:baseFg)($(Get-Cls $script:baseFg)) 期望=$(Hex $fgHwnd) 執行緒 $script:fgTid 焦點=$(Hex $script:baseFocus)($(Get-Cls $script:baseFocus))"
    $results['前置：前景基準視窗取得前景與鍵盤焦點'] = ($script:baseFg -eq $fgHwnd -and $script:baseFocus -ne [IntPtr]::Zero)

    # ── A. 鎖定時拖曳無效 ─────────────────────────────────────────────────────────
    $sA = Invoke-Drag -FromX $p0[0] -FromY $p0[1] -ToX ($p0[0] - 300) -ToY ($p0[1] + 60)
    Start-Sleep -Milliseconds 600
    $rA = Get-Rect $clock
    Log "## A 鎖定時拖曳後=$(Fmt $rA)"
    $results['A：鎖定（未進入編輯版面）時真實拖曳，矩形不變'] = Same $rA $rect0
    $results['A：拖曳期間前景與焦點不變'] = Test-FgUnchanged $sA 'A'

    # ── 進入編輯版面（CDP，非輸入注入）──────────────────────────────────────────────
    Invoke-Eval 'w=clock' "window.__TAURI__.core.invoke('set_edit_mode', { enabled: true })" | Out-Null
    $locked = Wait-Eval 'w=clock' "window.__TAURI__.core.invoke('get_settings').then(s => s.layout_locked)" 'false'
    # 情境 A 在鎖定狀態下按住拖過時鐘文字＝一般的文字選取（小工具頁面允許選取），清掉選取
    # 反白，免得它出現在步驟 2 的紅框截圖裡（與本 task 的行為無關）。
    Invoke-Eval 'w=clock' "(() => { window.getSelection().removeAllRanges(); return 'ok'; })()" | Out-Null
    $results['進入編輯版面：layout_locked=false'] = ($locked -match 'false')
    # 頁面端記錄 body class 的每次變化（紅框出現／消失的頁面記錄）。
    $observerInstalled = Invoke-Eval 'w=clock' "(() => { window.__rec75 = []; const t0 = performance.now(); new MutationObserver(() => window.__rec75.push(Math.round(performance.now() - t0) + 'ms invalid=' + document.body.classList.contains('edit-invalid'))).observe(document.body, { attributes: true, attributeFilter: ['class'] }); return 'ok'; })()"
    Log "## 頁面紅框記錄器安裝結果=$observerInstalled"
    $results['前置：頁面紅框記錄器已安裝'] = ($observerInstalled -ceq '"ok"')
    [void][V75.Native]::SetForegroundWindow($fgHwnd)
    Start-Sleep -Milliseconds 300
    $script:baseFg = [V75.Native]::GetForegroundWindow()
    $script:baseFocus = Get-FocusState $script:fgTid

    # ── 1. 拖到空白處 ─────────────────────────────────────────────────────────────
    $dropX = $target1.X + 13; $dropY = $target1.Y + 9   # 刻意偏離格線幾個像素，驗證 round
    $s1 = Invoke-Drag -FromX $p0[0] -FromY $p0[1] -ToX ($p0[0] + $dropX - $rect0.X) -ToY ($p0[1] + $dropY - $rect0.Y)
    $r1 = Wait-StableRect $clock
    $rec1 = Invoke-Eval 'w=clock' $clockExpr
    Log "## 1 空白處放開後=$(Fmt $r1) 期望格線矩形=$(Fmt $target1) 記錄=$rec1"
    $results['1：放開後視窗四邊落在格線 (8,2)–(24,11)'] = Same $r1 $target1
    $results['1：寬高格數不變（16×10），記錄寫回 (8,2,16,10)'] = (Test-ClockRecord $rec1 '8,2,16,10')
    $results['1：拖曳期間前景與焦點不變'] = Test-FgUnchanged $s1 '1'
    $preview1 = Invoke-Eval 'w=clock' $previewExpr
    Log "## 1 頁面 class 記錄=$preview1"
    # fix F1（review 7.5 L5）：必須確實取得記錄（observer 已裝、eval 成功）才判斷「沒有紅框」。
    $results['1：合法拖曳全程沒有紅框'] = ((Test-PreviewRecorded $preview1) -and ($preview1 -notmatch 'invalid=true'))

    # ── 2. 拖到與總經日曆重疊處 → 紅框 → 放開彈回 ─────────────────────────────────────
    Invoke-Eval 'w=clock' "(() => { window.__rec75.length = 0; return 'ok'; })()" | Out-Null
    $overlap = Grid-Rect $wa 15 20 16 10
    $script:heldClass = $null; $script:heldPng = $false; $script:heldRect = $null
    $held = {
        $script:heldRect = Get-Rect $clock
        $script:heldClass = Invoke-Eval 'w=clock' "document.body.classList.contains('edit-invalid')"
        $script:heldPng = Save-WindowPng $clock $pngPath
    }
    $s2 = Invoke-Drag -FromX $p1[0] -FromY $p1[1] -ToX ($p1[0] + $overlap.X - $target1.X) -ToY ($p1[1] + $overlap.Y - $target1.Y) -WhileHeld $held
    $r2 = Wait-StableRect $clock
    Start-Sleep -Milliseconds 500
    $classAfter = Invoke-Eval 'w=clock' "document.body.classList.contains('edit-invalid')"
    $preview2 = Invoke-Eval 'w=clock' $previewExpr
    $rec2 = Invoke-Eval 'w=clock' $clockExpr
    Log "## 2 按住時矩形=$(Fmt $script:heldRect) edit-invalid=$($script:heldClass) 截圖=$($script:heldPng)"
    Log "## 2 放開後=$(Fmt $r2) edit-invalid=$classAfter 記錄=$rec2 頁面 class 記錄=$preview2"
    $results['2：按住在重疊處時視窗確實跟著移動（離開原位）'] = ($script:heldRect -and -not (Same $script:heldRect $r1))
    $results['2：拖曳中頁面出現紅框（body.edit-invalid）'] = ($script:heldClass -match 'true')
    $results['2：拖曳中紅框截圖已存（PrintWindow 只含 clock 視窗）'] = [bool]$script:heldPng
    $results['2：放開後彈回步驟 1 的矩形'] = Same $r2 $r1
    $results['2：放開後紅框消失'] = ($classAfter -match 'false')
    $results['2：頁面記錄紅框先出現後消失'] = ((Test-PreviewRecorded $preview2) -and ($preview2 -match 'invalid=true.*invalid=false'))
    $results['2：記錄未變（仍為 8,2,16,10）'] = (Test-ClockRecord $rec2 '8,2,16,10')
    $results['2：拖曳期間前景與焦點不變'] = Test-FgUnchanged $s2 '2'

    # ── 2b（fix round 1）. 拖到部分超出右緣 → 紅框 → 放開彈回 ─────────────────────────
    # 右側那一欄被 fixed／dynamic／quotes 佔滿；先關掉 dynamic（只送 enabled，CDP，非輸入注入），
    # 讓 rows 20–29 的右側只剩「超出工作區」一個不合法原因。目標左上角 (38,20)：38+16=54 > 48。
    Invoke-Eval 'w=clock' "window.__TAURI__.core.invoke('update_settings', { patch: { widgets: { dynamic: { enabled: false } } } })" | Out-Null
    $dynOff = Wait-Eval 'w=clock' "window.__TAURI__.core.invoke('get_settings').then(s => s.widgets.dynamic.enabled)" 'false'
    Start-Sleep -Seconds 1
    Invoke-Eval 'w=clock' "(() => { window.__rec75.length = 0; return 'ok'; })()" | Out-Null
    $past = Grid-Rect $wa 38 20 16 10
    $script:heldClass2b = $null; $script:heldRect2b = $null
    $held2b = {
        $script:heldRect2b = Get-Rect $clock
        $script:heldClass2b = Invoke-Eval 'w=clock' "document.body.classList.contains('edit-invalid')"
    }
    $s2b = Invoke-Drag -FromX $p1[0] -FromY $p1[1] -ToX ($p1[0] + $past.X - $target1.X) -ToY ($p1[1] + $past.Y - $target1.Y) -WhileHeld $held2b
    $r2b = Wait-StableRect $clock
    Start-Sleep -Milliseconds 500
    $classAfter2b = Invoke-Eval 'w=clock' "document.body.classList.contains('edit-invalid')"
    $preview2b = Invoke-Eval 'w=clock' $previewExpr
    $rec2b = Invoke-Eval 'w=clock' $clockExpr
    $held2bRight = if ($script:heldRect2b) { $script:heldRect2b.X + $script:heldRect2b.W } else { $null }
    Log "## 2b dynamic.enabled=$dynOff；按住時矩形=$(Fmt $script:heldRect2b)（右緣 x=$held2bRight，工作區右緣 $($wa.X + $wa.W)）edit-invalid=$($script:heldClass2b)"
    Log "## 2b 放開後=$(Fmt $r2b) edit-invalid=$classAfter2b 記錄=$rec2b 頁面 class 記錄=$preview2b"
    $results['2b：前置 dynamic 已關閉（右側只剩超界一個不合法原因）'] = ($dynOff -match 'false')
    $results['2b：按住時視窗右緣超出工作區'] = ($held2bRight -gt ($wa.X + $wa.W))
    $results['2b：超出右緣時頁面出現紅框'] = ($script:heldClass2b -match 'true')
    $results['2b：放開後彈回步驟 1 的矩形'] = Same $r2b $r1
    $results['2b：放開後紅框消失、頁面記錄先出現後消失'] = ($classAfter2b -match 'false' -and (Test-PreviewRecorded $preview2b) -and $preview2b -match 'invalid=true.*invalid=false')
    $results['2b：記錄未變（仍為 8,2,16,10）'] = (Test-ClockRecord $rec2b '8,2,16,10')
    $results['2b：拖曳期間前景與焦點不變'] = Test-FgUnchanged $s2b '2b'

    # ── 離開編輯版面 → A'. 拖曳無效 ────────────────────────────────────────────────
    Invoke-Eval 'w=clock' "window.__TAURI__.core.invoke('set_edit_mode', { enabled: false })" | Out-Null
    $locked2 = Wait-Eval 'w=clock' "window.__TAURI__.core.invoke('get_settings').then(s => s.layout_locked)" 'true'
    $results['離開編輯版面：layout_locked=true'] = ($locked2 -match 'true')
    [void][V75.Native]::SetForegroundWindow($fgHwnd)
    Start-Sleep -Milliseconds 300
    $sA2 = Invoke-Drag -FromX $p1[0] -FromY $p1[1] -ToX ($p1[0] + 250) -ToY ($p1[1] + 250)
    Start-Sleep -Milliseconds 600
    $rA2 = Get-Rect $clock
    Log "## A' 離開編輯版面後拖曳=$(Fmt $rA2)"
    $results["A'：離開編輯版面後真實拖曳，矩形不變"] = Same $rA2 $r1

    # ── 跨顯示器 ───────────────────────────────────────────────────────────────────
    if ($mons.Count -lt 2) {
        $pending['跨顯示器拖放歸屬中心所在螢幕'] = '待補（需外接螢幕）：本次只偵測到一台顯示器'
        Log '## 跨顯示器：只有一台顯示器，待補（需外接螢幕）'
    } else {
        $pending['跨顯示器拖放歸屬中心所在螢幕'] = '偵測到多台顯示器，但本腳本未實作自動跨螢幕拖曳（見 human-checklist.md）'
    }

    # ── 3. 重啟後保留 ──────────────────────────────────────────────────────────────
    Stop-HostTree $hostProc
    Log '## 3 宿主已強制結束，重啟'
    $hostProc = Start-Host
    $clock = Wait-Window $hostProc.Id 'fc-host clock'
    Start-Sleep -Seconds 3
    $r3 = if ($clock -ne [IntPtr]::Zero) { Get-Rect $clock } else { $null }
    $rec3 = Invoke-Eval 'w=clock' $clockExpr
    Log "## 3 重啟後 clock=$(Fmt $r3) 記錄=$rec3"
    $results['3：重啟後位置保留（與步驟 1 相同）'] = Same $r3 $r1
    $results['3：重啟後記錄仍為 (8,2,16,10)'] = (Test-ClockRecord $rec3 '8,2,16,10')
}
catch {
    $msg = $_.Exception.Message
    if ($msg -like 'BLOCKED:*') {
        # fix F1（review 7.5 L4）：執行途中工作階段被鎖定＝環境阻斷，不是功能失敗——SafeInput 已
        # 停止注入；記為 BLOCKED（結束碼 2），不寫成「腳本跑完」FAIL。
        $script:blocked = $msg
        Log "## BLOCKED（執行途中鎖定，結果不完整）：$msg"
    } elseif ($msg -like 'ENV-BLOCKED*') {
        $script:envBlocked = $msg
        Log "## $msg（環境問題，結果不完整）"
    } else {
        Log "## 例外中止：$msg"
        $results['腳本跑完（無例外）'] = $false
    }
}
finally {
    # 每一項收尾各自 try，任何一項失敗都不影響其餘（宿主、表單、被最小化的視窗都要收乾淨）。
    if ($script:mouseDown) {
        # 拖曳途中出例外：補一次放開（仍經 SafeInput，鎖定時它會拒絕、不注入）。
        try { Invoke-GuardedMouse 4 -What '收尾：補放開（LEFTUP）' } catch { Log "## 收尾補放開失敗：$_" }
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
$sum.WriteLine("# verify-7.5-edit-move.ps1 summary $(Get-Ts)")
foreach ($k in $results.Keys) { $sum.WriteLine("$(if ($results[$k]) { 'PASS' } else { 'FAIL' })  $k") }
foreach ($k in $pending.Keys) { $sum.WriteLine("PENDING  $k：$($pending[$k])") }
if ($script:blocked) { $sum.WriteLine("BLOCKED  執行途中鎖定，以上結果不完整：$($script:blocked)") }
if ($script:envBlocked) { $sum.WriteLine("ENV-BLOCKED  $($script:envBlocked)") }
if ($occWarn) { $sum.WriteLine($occWarn) }
$sum.Close()
Get-Content $sumPath
if ($occWarn) { Write-Host $occWarn -ForegroundColor Yellow }
# 結束碼優先序：產品 FAIL（1）＞ 鎖定（2）＞ 環境（3，含使用者視窗未還原）。
exit (Get-VerdictExitCode -Results $results -Locked:([bool]$script:blocked) -EnvBlocked:([bool]$script:envBlocked) -NotRestored $occNotRestored)
