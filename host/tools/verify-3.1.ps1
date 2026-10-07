<#
.SYNOPSIS
    Task 3.1 驗收驅動腳本（視窗工廠）：首次啟動預設版面、置底從第一筆起成立、樣式與
    即時建立／關閉。只讀視窗狀態與透過 CDP 呼叫宿主自己的 IPC 指令，**不注入任何輸入**
    （不送按鍵、不點擊、不截圖），鎖定時也可跑。

.DESCRIPTION
    步驟：
      1. 確認沒有 fc-host 在跑（WebView2 共用 user data folder，已有實例時 CDP 參數不生效）。
      2. 以暫存目錄同時當 %APPDATA%（=> 沒有 settings.json＝首次啟動）與 %LOCALAPPDATA%
         （資料目錄預設值的來源，見 settings::default_data_dir）——完全不碰使用者真正的設定檔
         或 D:\finance-calendar。資料目錄先寫入固定 fixture
         `host/ui/fixtures/tw-events.json`（含 macro／events／quotes 等真實資料，quotes 非空），
         讓五個財經小工具從啟動起就有非空內容（quotes 不會因無資料被隱藏）。
      3. 先啟動 watch-zorder.ps1（-ProcessName fc-host、每 100 ms），等它寫出 header 行
         （Add-Type 完成、開始取樣）後才啟動宿主，故記錄涵蓋小工具第一次可見的瞬間。
      4. 等五個小工具視窗出現，列舉視窗矩形／延伸樣式，與預設格座標（host/src/settings.rs
         `DEFAULT_GRID_RECTS`）在主螢幕工作區換算出的格線矩形比對（task 7.7 起；原本是
         Lively 版錨點公式＋內容高度，已隨格線版面失效），矩形必須逐像素相等（3.1-windows.log）。
         格線本身與兩兩不相交另由 verify-grid-layout.ps1 驗收。
      5. 透過 CDP（WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS 開 remote debugging）在 clock 頁面呼叫
         update_settings：開 custom1 → 關 custom1＋quotes → 開回 quotes，每步後列舉視窗。
      6. 不截圖（小工具在最底層，截到的會是蓋在上面的一般視窗；視覺比對列入人工清單）。
      7. 結束宿主、等監控結束，分析 z-order 記錄（3.1-summary.log）。

    需要：pwsh 7、node（host/tools/host-cdp-eval.mjs）、已建置的 release exe。

.PARAMETER Exe
    fc-host.exe 路徑，預設 host/target/release/fc-host.exe。

.PARAMETER OutDir
    證據輸出目錄，預設 host/tools/evidence。

.PARAMETER CdpPort
    WebView2 remote debugging 埠，預設 9333。
#>
[CmdletBinding()]
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\fc-host.exe'),
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [int]$CdpPort = 9333
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\AutostartRegistry.psm1') -Force

Add-Type -Namespace V31 -Name Native -MemberDefinition @'
[DllImport("user32.dll")] public static extern System.IntPtr SetThreadDpiAwarenessContext(System.IntPtr ctx);
[DllImport("user32.dll")] public static extern System.IntPtr GetTopWindow(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern System.IntPtr GetWindow(System.IntPtr hWnd, uint cmd);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr hWnd, out uint pid);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(System.IntPtr hWnd, System.Text.StringBuilder s, int n);
[DllImport("user32.dll")] public static extern bool GetWindowRect(System.IntPtr hWnd, out RECT r);
[DllImport("user32.dll")] public static extern bool GetClientRect(System.IntPtr hWnd, out RECT r);
[DllImport("user32.dll")] public static extern long GetWindowLongPtrW(System.IntPtr hWnd, int idx);
[DllImport("user32.dll")] public static extern uint GetDpiForWindow(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern System.IntPtr MonitorFromPoint(POINT pt, uint flags);
[DllImport("user32.dll")] public static extern bool GetMonitorInfoW(System.IntPtr hMon, ref MONITORINFO mi);
[DllImport("user32.dll")] public static extern System.IntPtr GetForegroundWindow();
[StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
[StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
[StructLayout(LayoutKind.Sequential)] public struct MONITORINFO { public int cbSize; public RECT rcMonitor; public RECT rcWork; public uint dwFlags; }
'@

# 本執行緒改為 Per-Monitor-V2（-4），GetWindowRect／GetMonitorInfo 才會回實體像素。
[void][V31.Native]::SetThreadDpiAwarenessContext([IntPtr](-4))

$WS_EX_TOOLWINDOW = 0x80
$WS_EX_APPWINDOW = 0x40000
$WS_EX_NOACTIVATE = 0x08000000

# 預設格座標（host/src/settings.rs `DEFAULT_GRID_RECTS`；col,row,w,h，48×48 格線）。task 7.7
# 起取代原本的 Lively 版錨點表（錨點、位移、固定寬、最大高度，已隨格線版面刪除）。
$Layout = [ordered]@{
    clock   = @(15, 1, 16, 10)
    macro   = @(15, 12, 16, 30)
    fixed   = @(32, 1, 15, 17)
    dynamic = @(32, 19, 15, 23)
    quotes  = @(15, 43, 32, 4)
}
function Edge([int]$Origin, [int]$Extent, [int]$I) { $Origin + [int][math]::Floor([int64]$I * $Extent / 48) }

function Get-Ts { Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fffK' }
function Test-Locked { [bool](Get-Process -Name LogonUI, LockApp -ErrorAction SilentlyContinue) }

function Get-HostWindows([int]$ProcId) {
    $list = @()
    $h = [V31.Native]::GetTopWindow([IntPtr]::Zero)
    while ($h -ne [IntPtr]::Zero) {
        $p = 0
        [void][V31.Native]::GetWindowThreadProcessId($h, [ref]$p)
        if ($p -eq $ProcId -and [V31.Native]::IsWindowVisible($h)) {
            $sb = New-Object System.Text.StringBuilder 256
            [void][V31.Native]::GetWindowText($h, $sb, 256)
            $r = New-Object V31.Native+RECT
            [void][V31.Native]::GetWindowRect($h, [ref]$r)
            $c = New-Object V31.Native+RECT
            [void][V31.Native]::GetClientRect($h, [ref]$c)
            $ex = [V31.Native]::GetWindowLongPtrW($h, -20)
            $list += [PSCustomObject]@{
                Hwnd = ('0x{0:X}' -f $h.ToInt64()); Title = $sb.ToString()
                X = $r.Left; Y = $r.Top; W = $r.Right - $r.Left; H = $r.Bottom - $r.Top
                ClientW = $c.Right; ClientH = $c.Bottom
                Tool = [int](($ex -band $WS_EX_TOOLWINDOW) -ne 0)
                App = [int](($ex -band $WS_EX_APPWINDOW) -ne 0)
                NoAct = [int](($ex -band $WS_EX_NOACTIVATE) -ne 0)
                Dpi = [V31.Native]::GetDpiForWindow($h)
            }
        }
        $h = [V31.Native]::GetWindow($h, 2)
    }
    return $list
}

function Write-Windows([string]$Tag, [int]$ProcId, [System.IO.StreamWriter]$W) {
    $wins = Get-HostWindows $ProcId
    $W.WriteLine("## $(Get-Ts) $Tag：pid=$ProcId 可見視窗 $($wins.Count) 個（由上到下）")
    foreach ($x in $wins) {
        $W.WriteLine("  $($x.Hwnd) title='$($x.Title)' rect=($($x.X),$($x.Y),$($x.W)x$($x.H)) client=$($x.ClientW)x$($x.ClientH) dpi=$($x.Dpi) toolwindow=$($x.Tool) appwindow=$($x.App) noactivate=$($x.NoAct)")
    }
    return $wins
}

function Invoke-HostEval([string]$Expr) {
    $out = & node (Join-Path $PSScriptRoot 'host-cdp-eval.mjs') $CdpPort 'w=clock' $Expr 2>&1
    return ($out -join "`n")
}

function Wait-Windows([int]$ProcId, [scriptblock]$Cond, [int]$TimeoutSec = 20) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt $TimeoutSec) {
        $wins = Get-HostWindows $ProcId
        if (& $Cond $wins) { return $true }
        Start-Sleep -Milliseconds 200
    }
    return $false
}

# ── 1. 前置檢查 ─────────────────────────────────────────────────────────────────
if (Get-Process -Name fc-host -ErrorAction SilentlyContinue) {
    throw '已有 fc-host 在執行，請先結束（WebView2 共用 user data folder，CDP 參數不會生效）。'
}
$Exe = (Resolve-Path $Exe).Path
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$zlog = Join-Path $OutDir '3.1-zorder.log'
$wlogPath = Join-Path $OutDir '3.1-windows.log'
$slogPath = Join-Path $OutDir '3.1-summary.log'
$wlog = New-EvidenceWriter $wlogPath
$wlog.AutoFlush = $true
$wlog.WriteLine("# verify-3.1.ps1 start=$(Get-Ts) exe=$Exe locked=$([int](Test-Locked))")

$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ("fc-host-3.1-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
$tempAppData = Join-Path $tempRoot 'Roaming'
$tempLocalAppData = Join-Path $tempRoot 'Local'
New-Item -ItemType Directory -Force -Path $tempAppData, $tempLocalAppData | Out-Null

# 資料目錄＝固定 fixture（非空 quotes，讓五個財經小工具啟動起就有內容；同一份 fixture 也給
# verify-5.6.ps1 當備援來源），不使用真正使用者的 %LOCALAPPDATA%、也不碰 D:\finance-calendar。
$dataDir = Join-Path $tempLocalAppData 'tw.fintools.fc-host\data'
New-Item -ItemType Directory -Force -Path $dataDir | Out-Null
# fix F3（review task-3.1-v31-opus.md [low]）：fixture 不見時立刻中止，不讓「quotes 非空」前提靜默失效。
Copy-Item (Join-Path $PSScriptRoot '..\ui\fixtures\tw-events.json') (Join-Path $dataDir 'tw_events.json') -ErrorAction Stop
$wlog.WriteLine("# APPDATA=$tempAppData（無 settings.json＝首次啟動）LOCALAPPDATA=$tempLocalAppData（資料目錄=$dataDir，fixture=tw-events.json，quotes 非空）")

# ── 2. 先開監控，等 header ──────────────────────────────────────────────────────
$regSnap = @(Save-FcHostAutostartRegistry)
$wlog.WriteLine("# $(Get-Ts) 開機自啟登錄快照：$(($regSnap | ForEach-Object { "$($_.Name)@$(Split-Path $_.Key -Leaf) Exists=$($_.Exists)" }) -join '; ')")
$watcher = Start-Process pwsh -PassThru -WindowStyle Hidden -ArgumentList @(
    '-NoProfile', '-File', (Join-Path $PSScriptRoot 'watch-zorder.ps1'),
    '-ProcessName', 'fc-host', '-OutFile', $zlog, '-DurationSec', '75', '-IntervalMs', '100', '-Quiet')
$sw = [Diagnostics.Stopwatch]::StartNew()
while (-not ((Test-Path $zlog) -and (Get-Content $zlog -TotalCount 1))) {
    if ($sw.Elapsed.TotalSeconds -gt 30) { throw 'watch-zorder 30 秒內沒有寫出 header' }
    Start-Sleep -Milliseconds 100
}
$wlog.WriteLine("# $(Get-Ts) 監控已就緒（header 已寫出），啟動宿主")

# ── 3. 啟動宿主（首次啟動） ───────────────────────────────────────────────────────
$oldAppData = $env:APPDATA
$oldLocalAppData = $env:LOCALAPPDATA
$oldWv2 = $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS
$hostProc = $null
try {
    $env:APPDATA = $tempAppData
    $env:LOCALAPPDATA = $tempLocalAppData
    $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$CdpPort"
    $hostProc = Start-Process -FilePath $Exe -PassThru
} finally {
    $env:APPDATA = $oldAppData
    $env:LOCALAPPDATA = $oldLocalAppData
    $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $oldWv2
}
$hostPid = $hostProc.Id
$wlog.WriteLine("# $(Get-Ts) 宿主 pid=$hostPid")

$results = [ordered]@{}
try {
    $ok = Wait-Windows $hostPid { param($w) ($w | Where-Object { $_.Title -like 'fc-host *' }).Count -ge 5 }
    Start-Sleep -Seconds 3
    $wins = Write-Windows '首次啟動' $hostPid $wlog
    $results['五個財經小工具出現'] = $ok -and (@($wins | Where-Object { $_.Title -like 'fc-host *' }).Count -eq 5)

    # ── 4. 期望矩形比對（主螢幕工作區＋DPI） ──────────────────────────────────────
    $pt = New-Object V31.Native+POINT
    $mon = [V31.Native]::MonitorFromPoint($pt, 1)  # MONITOR_DEFAULTTOPRIMARY
    $mi = New-Object V31.Native+MONITORINFO
    $mi.cbSize = [Runtime.InteropServices.Marshal]::SizeOf($mi)
    [void][V31.Native]::GetMonitorInfoW($mon, [ref]$mi)
    $wa = $mi.rcWork
    $wlog.WriteLine("## 主螢幕工作區（實體像素）=($($wa.Left),$($wa.Top))-($($wa.Right),$($wa.Bottom))")
    # 期望矩形＝預設格座標換算的格線矩形：edge(i) = 起點 + floor(i × 長度 / 48)，逐像素比對。
    $allMatch = $true
    foreach ($id in $Layout.Keys) {
        $L = $Layout[$id]
        $win = $wins | Where-Object { $_.Title -eq "fc-host $id" } | Select-Object -First 1
        if (-not $win) { $wlog.WriteLine("  $id：找不到視窗 FAIL"); $allMatch = $false; continue }
        $waW = $wa.Right - $wa.Left; $waH = $wa.Bottom - $wa.Top
        $x = Edge $wa.Left $waW $L[0]; $w = (Edge $wa.Left $waW ($L[0] + $L[2])) - $x
        $y = Edge $wa.Top $waH $L[1]; $h = (Edge $wa.Top $waH ($L[1] + $L[3])) - $y

        $diffX = [math]::Abs($win.X - $x)
        $diffY = [math]::Abs($win.Y - $y)
        $diffW = [math]::Abs($win.W - $w)
        $diffH = [math]::Abs($win.H - $h)
        $match = ($diffX -eq 0) -and ($diffY -eq 0) -and ($diffW -eq 0) -and ($diffH -eq 0) -and
                 ($win.ClientW -eq $win.W) -and ($win.ClientH -eq $win.H) -and
                 ($win.Tool -eq 1) -and ($win.App -eq 0) -and ($win.NoAct -eq 1)
        if (-not $match) { $allMatch = $false }
        $wlog.WriteLine("  $id 預設格座標=$($L -join ',') 期望=($x,$y,${w}x$h) 實際=($($win.X),$($win.Y),$($win.W)x$($win.H)) client=$($win.ClientW)x$($win.ClientH) 樣式 tool/app/noact=$($win.Tool)/$($win.App)/$($win.NoAct) → $(if ($match) { 'PASS' } else { 'FAIL' })")
    }
    $results['排列＝預設格座標的格線矩形（逐像素）、視窗矩形＝client 無外框、TOOLWINDOW/無 APPWINDOW/NOACTIVATE'] = $allMatch

    $fg = [V31.Native]::GetForegroundWindow()
    $fgPid = 0
    [void][V31.Native]::GetWindowThreadProcessId($fg, [ref]$fgPid)
    $wlog.WriteLine("## 前景視窗 pid=$fgPid（宿主 pid=$hostPid）")
    $results['啟動後前景不是宿主'] = ($fgPid -ne $hostPid)

    # ── 4b. 各頁面以 widget.html?w=<id> 載入並掛載（#widget-root 有子節點、不是佔位訊息；
    #     行情條在無資料時內容文字為 0 是 4.5 的既定行為，故不以文字長度判定）──────────────────
    $allMounted = $true
    foreach ($id in $Layout.Keys) {
        $expr = "(() => { const r = document.getElementById('widget-root'); return { search: location.search, placeholder: !!document.querySelector('.widget-placeholder'), textLen: r ? r.innerText.length : -1, children: r ? r.children.length : -1, title: document.title }; })()"
        $out = & node (Join-Path $PSScriptRoot 'host-cdp-eval.mjs') $CdpPort "w=$id" $expr 2>&1
        $wlog.WriteLine("## 頁面 $id：$($out -join ' ')")
        try {
            $o = ($out -join '') | ConvertFrom-Json
            if ($o.search -ne "?w=$id" -or $o.placeholder -or $o.children -le 0) { $allMounted = $false }
        } catch { $allMounted = $false }
    }
    $results['五個頁面以 widget.html?w=<id> 載入並掛載（非佔位訊息）'] = $allMounted

    # ── 5. 截圖：刻意不做 ─────────────────────────────────────────────────────────
    # 小工具在最底層，一般視窗開著時畫面截到的是那些視窗（task 3.1 第一次實跑即如此，截到
    # 使用者自己的應用程式內容——不可進證據檔）；要看到小工具得先把視窗全部最小化，那屬於
    # 輸入注入／操作使用者視窗，驅動腳本不做。視覺比對列入 human-checklist「Task 3.1」。
    $wlog.WriteLine("## 截圖：略過（見腳本註解；鎖定狀態=$([int](Test-Locked))），視覺比對列入 human-checklist")

    # ── 6. 即時建立／關閉（update_settings via CDP） ──────────────────────────────
    $r = Invoke-HostEval "window.__TAURI__.core.invoke('update_settings', { patch: { widgets: { custom1: { enabled: true } } } }).then(s => s.widgets.custom1.enabled)"
    $wlog.WriteLine("## $(Get-Ts) update_settings custom1=on → $r")
    $ok = Wait-Windows $hostPid { param($w) [bool]($w | Where-Object { $_.Title -eq 'fc-host custom1' }) }
    Start-Sleep -Seconds 2
    $wins = Write-Windows '開啟 custom1 後' $hostPid $wlog
    $c1 = $wins | Where-Object { $_.Title -eq 'fc-host custom1' }
    $results['開啟 custom1 → 即時建立（TOOLWINDOW、無 APPWINDOW）'] = $ok -and $c1 -and ($c1.Tool -eq 1) -and ($c1.App -eq 0)

    $r = Invoke-HostEval "window.__TAURI__.core.invoke('update_settings', { patch: { widgets: { custom1: { enabled: false }, quotes: { enabled: false } } } }).then(s => [s.widgets.custom1.enabled, s.widgets.quotes.enabled])"
    $wlog.WriteLine("## $(Get-Ts) update_settings custom1=off quotes=off → $r")
    $ok = Wait-Windows $hostPid { param($w) -not ($w | Where-Object { $_.Title -in 'fc-host custom1', 'fc-host quotes' }) }
    Start-Sleep -Seconds 2
    $wins = Write-Windows '關閉 custom1＋quotes 後' $hostPid $wlog
    $results['關閉 custom1＋quotes → 即時關閉、其餘四個不受影響'] = $ok -and (@($wins | Where-Object { $_.Title -like 'fc-host *' }).Count -eq 4)

    $r = Invoke-HostEval "window.__TAURI__.core.invoke('update_settings', { patch: { widgets: { quotes: { enabled: true } } } }).then(s => s.widgets.quotes.enabled)"
    $wlog.WriteLine("## $(Get-Ts) update_settings quotes=on → $r")
    $ok = Wait-Windows $hostPid { param($w) [bool]($w | Where-Object { $_.Title -eq 'fc-host quotes' }) }
    Start-Sleep -Seconds 2
    $wins = Write-Windows '開回 quotes 後' $hostPid $wlog
    $results['開回 quotes → 重新建立'] = $ok -and (@($wins | Where-Object { $_.Title -like 'fc-host *' }).Count -eq 5)

    $saved = Get-Content (Join-Path $tempAppData 'tw.fintools.fc-host\settings.json') -Raw | ConvertFrom-Json
    $wlog.WriteLine("## settings.json：custom1.enabled=$($saved.widgets.custom1.enabled) quotes.enabled=$($saved.widgets.quotes.enabled)")
    $results['設定已落地（custom1=false、quotes=true）'] = (-not $saved.widgets.custom1.enabled) -and $saved.widgets.quotes.enabled
    Start-Sleep -Seconds 3
}
finally {
    if ($hostProc -and -not $hostProc.HasExited) { Stop-Process -Id $hostPid -Force }
    $regLeft = @(Restore-FcHostAutostartRegistry $regSnap)
    if ($regLeft.Count -gt 0) { Write-Warning "開機自啟登錄未還原：$($regLeft -join '; ')" }
    $wlog.WriteLine("# $(Get-Ts) 開機自啟登錄還原：未還原 $($regLeft.Count) 項$(if ($regLeft.Count) { '：' + ($regLeft -join '; ') })")
    $results['開機自啟登錄已還原（Run／StartupApproved 的 fc-host）'] = ($regLeft.Count -eq 0)
    $wlog.WriteLine("# $(Get-Ts) 宿主已結束；等待監控結束")
    $wlog.Close()
}

Stop-Process -Id $watcher.Id -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 500

# ── 7. 分析 z-order 記錄 ─────────────────────────────────────────────────────────
# 判讀規則：
# - 只看類別為 `Tauri Window` 的視窗（小工具）。宿主行程另有 tao 內部的
#   `Tao Thread Event Target`（tao event_loop.rs：WS_EX_LAYERED|TRANSPARENT|TOOLWINDOW|NOACTIVATE、
#   0×0 建立、永不繪製；因需要 WM_PAINT 而帶 WS_VISIBLE），不是小工具，不列入判定。
# - `visible=0` 的項目不列入：小工具在「隱藏、仍位於建立時的最上層」與「顯示並置底」之間只有
#   一個 SetWindowPos；監控列舉 z-order 途中剛好發生這個呼叫時，同一 HWND 會在清單中出現兩次
#   （上方隱藏的舊位置、底部可見的新位置），watch-zorder 取第一個出現處，所以印出 visible=0 與
#   一長串 below。隱藏視窗本來就不在規格「可見的一般應用程式視窗位於小工具之下」的範圍內。
$lines = Get-Content $zlog | Where-Object { $_ -notmatch '^#' }
$widgetLines = 0; $violations = @(); $firstSeen = $null; $firstFg = @(); $skippedHidden = @()
foreach ($ln in $lines) {
    $ts = $ln.Substring(0, 29)
    $hwnds = [regex]::Matches($ln, 'win\[(0x[0-9A-F]+)\]\.class=Tauri Window') | ForEach-Object { $_.Groups[1].Value }
    if (-not $hwnds) { continue }
    $counted = $false
    foreach ($hw in $hwnds) {
        $vis = [regex]::Match($ln, "win\[$hw\]\.visible=(\d)").Groups[1].Value
        $below = [regex]::Match($ln, "win\[$hw\]\.below=(\S+)").Groups[1].Value
        if ($vis -ne '1') {
            $dup = [regex]::Match($ln, "win\[$hw\]\.dupInSnapshot=(\d+)").Groups[1].Value
            $skippedHidden += "$ts $hw dupInSnapshot=$(if ($dup) { $dup } else { '無（異常，需人工檢查）' })"
            if (-not $dup) { $violations += "$ts $hw visible=0 但未重複出現" }
            continue
        }
        $counted = $true
        if ($below -ne '(none)') { $violations += "$ts $hw below=$below" }
    }
    if ($counted) {
        $widgetLines++
        if (-not $firstSeen) { $firstSeen = $ts }
    }
    if ($ln -match "fgPid=$hostPid\b") { $firstFg += $ts }
}
$results["z-order 記錄：含小工具的 $widgetLines 行中，小工具之下的一般視窗＝0"] = ($widgetLines -gt 0) -and ($violations.Count -eq 0)
$results['z-order 記錄：前景從未是宿主'] = ($firstFg.Count -eq 0)

$sum = New-EvidenceWriter $slogPath
$sum.WriteLine("# verify-3.1.ps1 summary $(Get-Ts)")
$sum.WriteLine("# 監控第一筆含小工具的記錄時間：$firstSeen")
foreach ($k in $results.Keys) { $sum.WriteLine("$(if ($results[$k]) { 'PASS' } else { 'FAIL' })  $k") }
foreach ($v in $violations) { $sum.WriteLine("  violation: $v") }
foreach ($f in $firstFg) { $sum.WriteLine("  fg=host: $f") }
foreach ($s in $skippedHidden) { $sum.WriteLine("  skipped(visible=0，列舉途中剛好顯示並置底): $s") }
$sum.Close()
Get-Content $slogPath
Remove-Item -Recurse -Force $tempRoot -ErrorAction SilentlyContinue
