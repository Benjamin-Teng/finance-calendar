<#
.SYNOPSIS
    Task 3.2 驗收驅動腳本（task 7.7 起改為格線版面，design.md D7「無內容」）：視窗矩形固定＝
    格子、不隨內容伸縮；內容為 0（report_content）隱藏、有內容後（沿用 3.1 顯示路徑）重新出現；
    動態事件撐爆時視窗仍＝格子、清單在視窗內捲動。原本「視窗高度＝min(內容, 最大高度)」的判準
    已隨 task 7.3 失效並刪除。只讀視窗狀態與透過 CDP 呼叫宿主自己的 IPC 指令，**不注入任何輸入**（不送按鍵、
    不點擊、不截圖），鎖定時也可跑。

.DESCRIPTION
    步驟：
      1. 確認沒有 fc-host 在跑。
      2. 以暫存目錄同時當 %APPDATA%（首次啟動、無 settings.json）與 %LOCALAPPDATA%
         （資料目錄預設值的來源，見 settings::default_data_dir）——**完全不碰使用者真正的
         設定檔或 D:\finance-calendar**。
      3. 在暫存資料目錄先寫入 fixture：`events` 40 筆（遠超過 dynamic 格子放得下的量）、
         `quotes` 空陣列（quotes 唯一沒資料就整個隱藏的小工具）。
      4. 先啟動 watch-zorder.ps1（-ProcessName fc-host）再啟動宿主，涵蓋小工具第一次可見的
         瞬間；啟動時 setup() 會立即輪詢一次資料目錄，四個一律有內容的財經小工具
         （clock／macro／fixed／dynamic）應在數秒內出現，quotes 應保持隱藏。
      5. clock／macro／fixed／dynamic 的視窗矩形逐像素＝預設格座標（host/src/settings.rs
         `DEFAULT_GRID_RECTS`）換算的格線矩形（內容多寡不影響）；dynamic 的 `#dynList`
         `scrollHeight > clientHeight`（撐爆的清單在視窗內捲動）。
      6. 改寫 fixture（quotes 填入一筆資料），等待下一輪排程輪詢（≤30 秒，design.md D5）後
         quotes 視窗應出現，矩形＝格子，且延伸樣式（TOOLWINDOW／無
         APPWINDOW／NOACTIVATE）與 3.1 的不變量一致——證明隱藏→重新顯示走的是
         `desktop::show_at_bottom`（3.1 顯示路徑），不是建立新視窗、也沒有洗掉樣式。
      7. 不截圖；結束宿主，分析 watch-zorder 記錄（隱藏／重新顯示／調整尺寸過程中，任何時刻都
         不得有一般視窗位於小工具之下）。

.PARAMETER Exe
    fc-host.exe 路徑，預設 host/target/release/fc-host.exe。

.PARAMETER OutDir
    證據輸出目錄，預設 host/tools/evidence。

.PARAMETER CdpPort
    WebView2 remote debugging 埠，預設 9334（與 verify-3.1.ps1 的 9333 分開，避免同時跑時衝突）。
#>
[CmdletBinding()]
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\fc-host.exe'),
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [int]$CdpPort = 9334
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\AutostartRegistry.psm1') -Force

Add-Type -Namespace V32 -Name Native -MemberDefinition @'
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
[DllImport("user32.dll")] public static extern System.IntPtr GetForegroundWindow();
[DllImport("user32.dll")] public static extern System.IntPtr MonitorFromPoint(POINT pt, uint flags);
[DllImport("user32.dll")] public static extern bool GetMonitorInfoW(System.IntPtr hMon, ref MONITORINFO mi);
[StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
[StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
[StructLayout(LayoutKind.Sequential)] public struct MONITORINFO { public int cbSize; public RECT rcMonitor; public RECT rcWork; public uint dwFlags; }
'@

# 本執行緒改為 Per-Monitor-V2（-4），GetWindowRect／GetClientRect 才會回實體像素。
[void][V32.Native]::SetThreadDpiAwarenessContext([IntPtr](-4))

$WS_EX_TOOLWINDOW = 0x80
$WS_EX_APPWINDOW = 0x40000
$WS_EX_NOACTIVATE = 0x08000000

# 預設格座標（host/src/settings.rs `DEFAULT_GRID_RECTS`；col,row,w,h，48×48 格線）。
$Grid = @{ clock = @(15, 1, 16, 10); macro = @(15, 12, 16, 30); fixed = @(32, 1, 15, 17); dynamic = @(32, 19, 15, 23); quotes = @(15, 43, 32, 4) }
function Edge([int]$Origin, [int]$Extent, [int]$I) { $Origin + [int][math]::Floor([int64]$I * $Extent / 48) }
# 主螢幕工作區上的格線矩形（實體像素）。
function Get-GridRect([string]$Id) {
    $pt = New-Object V32.Native+POINT
    $mi = New-Object V32.Native+MONITORINFO
    $mi.cbSize = [Runtime.InteropServices.Marshal]::SizeOf($mi)
    [void][V32.Native]::GetMonitorInfoW([V32.Native]::MonitorFromPoint($pt, 1), [ref]$mi)
    $wa = $mi.rcWork; $g = $Grid[$Id]
    $waW = $wa.Right - $wa.Left; $waH = $wa.Bottom - $wa.Top
    $x = Edge $wa.Left $waW $g[0]; $y = Edge $wa.Top $waH $g[1]
    [PSCustomObject]@{ X = $x; Y = $y; W = (Edge $wa.Left $waW ($g[0] + $g[2])) - $x; H = (Edge $wa.Top $waH ($g[1] + $g[3])) - $y }
}
function Test-GridRect($Win, [string]$Id) {
    $e = Get-GridRect $Id
    $ok = ($Win.X -eq $e.X -and $Win.Y -eq $e.Y -and $Win.W -eq $e.W -and $Win.H -eq $e.H -and $Win.ClientH -eq $Win.H)
    $wlog.WriteLine("  $Id 期望格線矩形=($($e.X),$($e.Y),$($e.W)x$($e.H)) 實際=($($Win.X),$($Win.Y),$($Win.W)x$($Win.H)) ClientH=$($Win.ClientH) → $(if ($ok) { 'PASS' } else { 'FAIL' })")
    return $ok
}

function Get-Ts { Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fffK' }
function Test-Locked { [bool](Get-Process -Name LogonUI, LockApp -ErrorAction SilentlyContinue) }

function Get-HostWindows([int]$ProcId) {
    $list = @()
    $h = [V32.Native]::GetTopWindow([IntPtr]::Zero)
    while ($h -ne [IntPtr]::Zero) {
        $p = 0
        [void][V32.Native]::GetWindowThreadProcessId($h, [ref]$p)
        if ($p -eq $ProcId -and [V32.Native]::IsWindowVisible($h)) {
            $sb = New-Object System.Text.StringBuilder 256
            [void][V32.Native]::GetWindowText($h, $sb, 256)
            $r = New-Object V32.Native+RECT
            [void][V32.Native]::GetWindowRect($h, [ref]$r)
            $c = New-Object V32.Native+RECT
            [void][V32.Native]::GetClientRect($h, [ref]$c)
            $ex = [V32.Native]::GetWindowLongPtrW($h, -20)
            $list += [PSCustomObject]@{
                Hwnd = ('0x{0:X}' -f $h.ToInt64()); Title = $sb.ToString()
                X = $r.Left; Y = $r.Top; W = $r.Right - $r.Left; H = $r.Bottom - $r.Top
                ClientW = $c.Right; ClientH = $c.Bottom
                Tool = [int](($ex -band $WS_EX_TOOLWINDOW) -ne 0)
                App = [int](($ex -band $WS_EX_APPWINDOW) -ne 0)
                NoAct = [int](($ex -band $WS_EX_NOACTIVATE) -ne 0)
                Dpi = [V32.Native]::GetDpiForWindow($h)
            }
        }
        $h = [V32.Native]::GetWindow($h, 2)
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

function Wait-Windows([int]$ProcId, [scriptblock]$Cond, [int]$TimeoutSec = 20) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt $TimeoutSec) {
        $wins = Get-HostWindows $ProcId
        if (& $Cond $wins) { return $true }
        Start-Sleep -Milliseconds 300
    }
    return $false
}

function Invoke-PageEval([string]$WidgetId, [string]$Expr) {
    $out = & node (Join-Path $PSScriptRoot 'host-cdp-eval.mjs') $CdpPort "w=$WidgetId" $Expr 2>&1
    return ($out -join '')
}

# fixture：`events` 40 筆（足以撐爆 dynamic 的 640px 上限），quotes 依 $WithQuotes 決定。
# events 一律用「今天」日期——dynamic.js 上節只列 `e.date >= todayIso` 的場次。
function New-Fixture([bool]$WithQuotes) {
    $today = Get-Date -Format 'yyyy-MM-dd'
    $now = Get-Date -Format 'yyyy-MM-dd HH:mm'
    $events = for ($i = 1; $i -le 40; $i++) {
        [ordered]@{
            date = $today
            type = 'earnings'
            code = ('T{0:D3}' -f $i)
            name = "測試公司$i"
            note = '第1季 財報'
        }
    }
    $quotes = @()
    if ($WithQuotes) {
        $quotes = @(
            [ordered]@{ name = 'USD/TWD'; kind = 'fx'; price = 31.5; chg_pct = 0.12; chg_abs = 0.04 }
        )
    }
    $obj = [ordered]@{
        updated  = $now
        fetched  = $now
        errors   = @()
        macro    = @()
        events   = @($events)
        punish   = @()
        quotes   = $quotes
        holidays = @()
    }
    return ($obj | ConvertTo-Json -Depth 6)
}

# ── 1. 前置檢查 ─────────────────────────────────────────────────────────────────
if (Get-Process -Name fc-host -ErrorAction SilentlyContinue) {
    throw '已有 fc-host 在執行，請先結束（WebView2 共用 user data folder，CDP 參數不會生效）。'
}
$Exe = (Resolve-Path $Exe).Path
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$zlog = Join-Path $OutDir '3.2-zorder.log'
$wlogPath = Join-Path $OutDir '3.2-windows.log'
$slogPath = Join-Path $OutDir '3.2-summary.log'
$wlog = New-EvidenceWriter $wlogPath
$wlog.AutoFlush = $true
$wlog.WriteLine("# verify-3.2.ps1 start=$(Get-Ts) exe=$Exe locked=$([int](Test-Locked))")

# ── 2. 暫存 %APPDATA%／%LOCALAPPDATA%＋資料目錄 fixture ───────────────────────────────
$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ("fc-host-3.2-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
$tempAppData = Join-Path $tempRoot 'Roaming'
$tempLocalAppData = Join-Path $tempRoot 'Local'
New-Item -ItemType Directory -Force -Path $tempAppData, $tempLocalAppData | Out-Null
$dataDir = Join-Path $tempLocalAppData 'tw.fintools.fc-host\data'
New-Item -ItemType Directory -Force -Path $dataDir | Out-Null
$twEventsPath = Join-Path $dataDir 'tw_events.json'
Set-Content -Path $twEventsPath -Value (New-Fixture $false) -Encoding UTF8
$wlog.WriteLine("# APPDATA=$tempAppData（無 settings.json＝首次啟動）LOCALAPPDATA=$tempLocalAppData（資料目錄=$dataDir，quotes 初始為空陣列）")

# ── 3. 先開監控，等 header ──────────────────────────────────────────────────────
$regSnap = @(Save-FcHostAutostartRegistry)
$wlog.WriteLine("# $(Get-Ts) 開機自啟登錄快照：$(($regSnap | ForEach-Object { "$($_.Name)@$(Split-Path $_.Key -Leaf) Exists=$($_.Exists)" }) -join '; ')")
$watcher = Start-Process pwsh -PassThru -WindowStyle Hidden -ArgumentList @(
    '-NoProfile', '-File', (Join-Path $PSScriptRoot 'watch-zorder.ps1'),
    '-ProcessName', 'fc-host', '-OutFile', $zlog, '-DurationSec', '90', '-IntervalMs', '150', '-Quiet')
$sw = [Diagnostics.Stopwatch]::StartNew()
while (-not ((Test-Path $zlog) -and (Get-Content $zlog -TotalCount 1))) {
    if ($sw.Elapsed.TotalSeconds -gt 30) { throw 'watch-zorder 30 秒內沒有寫出 header' }
    Start-Sleep -Milliseconds 100
}
$wlog.WriteLine("# $(Get-Ts) 監控已就緒（header 已寫出），啟動宿主")

# ── 4. 啟動宿主（首次啟動） ───────────────────────────────────────────────────────
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
    # ── 5. 四個一律有內容的財經小工具出現；quotes（quotes:[] fixture）應保持隱藏 ────────
    $ok = Wait-Windows $hostPid { param($w) (@($w | Where-Object { $_.Title -in 'fc-host clock', 'fc-host macro', 'fc-host fixed', 'fc-host dynamic' })).Count -eq 4 }
    Start-Sleep -Seconds 3  # 讓 report_content 走完（webview 掛載 → ResizeObserver → IPC）沉澱
    $wins = Write-Windows '首次啟動（quotes 空陣列 fixture、dynamic 40 筆事件）' $hostPid $wlog
    $results['四個一律有內容的財經小工具出現'] = $ok

    $quotesWin = $wins | Where-Object { $_.Title -eq 'fc-host quotes' }
    $results['quotes 無資料 → 視窗隱藏（不在可見視窗清單）'] = (-not $quotesWin)

    # ── 6. 視窗矩形＝格子（task 7.3 起不隨內容伸縮；dynamic 塞了 40 筆事件也一樣）──────────
    foreach ($id in 'clock', 'macro', 'fixed', 'dynamic') {
        $win = $wins | Where-Object { $_.Title -eq "fc-host $id" }
        if (-not $win) { $wlog.WriteLine("  $id：找不到視窗") }
        $results["$id 視窗矩形＝預設格子（不隨內容伸縮）"] = [bool]($win -and (Test-GridRect $win $id))
    }

    # ── 7. 動態事件撐爆：清單在視窗內捲動 ─────────────────────────────────────────────
    $dynWin = $wins | Where-Object { $_.Title -eq 'fc-host dynamic' }
    if ($dynWin) {
        # 注意：expression 本身不要再呼叫 JSON.stringify——host-cdp-eval.mjs 的
        # `evaluate()` 已用 CDP `returnByValue` 取回反序列化後的物件，外層 `console.log`
        # 只做一次 JSON 編碼；expression 裡再包一層 JSON.stringify 會變成「JSON 字串的
        # JSON 字串」，`ConvertFrom-Json` 解出來是一個純字串而非物件（曾經在此踩過一次）。
        $raw = Invoke-PageEval 'dynamic' "(() => { const el = document.getElementById('dynList'); return { scrollH: el ? el.scrollHeight : null, clientH: el ? el.clientHeight : null }; })()"
        $wlog.WriteLine("## dynamic #dynList：$raw")
        try {
            $parsed = ($raw | ConvertFrom-Json)
            $scrollOverflow = [double]$parsed.scrollH -gt [double]$parsed.clientH
        } catch {
            $scrollOverflow = $false
        }
        $results['動態事件清單可捲動（scrollHeight>clientHeight）'] = $scrollOverflow
    } else {
        $results['動態事件清單可捲動（scrollHeight>clientHeight）'] = $false
    }

    # ── 8. 改寫 fixture：quotes 現在有資料，等下一輪排程輪詢（≤30 秒）後應出現 ─────────────
    Set-Content -Path $twEventsPath -Value (New-Fixture $true) -Encoding UTF8
    $wlog.WriteLine("## $(Get-Ts) 更新 fixture：quotes 現在有一筆資料（USD/TWD）")

    $ok = Wait-Windows $hostPid { param($w) [bool]($w | Where-Object { $_.Title -eq 'fc-host quotes' }) } 40
    Start-Sleep -Seconds 2
    $wins2 = Write-Windows '更新 fixture 後（quotes 有資料）' $hostPid $wlog
    $results['quotes 有資料後出現（≤30 秒排程輪詢＋顯示）'] = $ok

    $quotesWin2 = $wins2 | Where-Object { $_.Title -eq 'fc-host quotes' }
    if ($quotesWin2) {
        $results['quotes 重新顯示後矩形＝預設格子'] = (Test-GridRect $quotesWin2 'quotes')
        $results['quotes 重新顯示沿用 3.1 顯示路徑（仍是 TOOLWINDOW／無 APPWINDOW／NOACTIVATE）'] =
        ($quotesWin2.Tool -eq 1) -and ($quotesWin2.App -eq 0) -and ($quotesWin2.NoAct -eq 1)
    } else {
        $results['quotes 重新顯示後矩形＝預設格子'] = $false
        $results['quotes 重新顯示沿用 3.1 顯示路徑（仍是 TOOLWINDOW／無 APPWINDOW／NOACTIVATE）'] = $false
    }

    # ── 9. 截圖：刻意不做（見腳本註解與 verify-3.1.ps1 同理） ──────────────────────────
    $wlog.WriteLine("## 截圖：略過（鎖定狀態=$([int](Test-Locked))），視覺比對列入 human-checklist")

    Start-Sleep -Seconds 2
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

# ── 10. 分析 z-order 記錄：整個隱藏／重新顯示／調整尺寸過程中，任何時刻都不得有一般視窗
#        位於小工具之下（判讀規則同 verify-3.1.ps1：只看 `Tauri Window`；`visible=0` 必須
#        伴隨 `dupInSnapshot` 否則算異常，見該腳本註解）。────────────────────────────────
$lines = Get-Content $zlog | Where-Object { $_ -notmatch '^#' }
$widgetLines = 0; $violations = @(); $firstFg = @(); $skippedHidden = @()
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
    if ($counted) { $widgetLines++ }
    if ($ln -match "fgPid=$hostPid\b") { $firstFg += $ts }
}
$results["z-order 記錄：含小工具的 $widgetLines 行中，小工具之下的一般視窗＝0（隱藏／重新顯示期間亦成立）"] = ($widgetLines -gt 0) -and ($violations.Count -eq 0)
$results['z-order 記錄：前景從未是宿主'] = ($firstFg.Count -eq 0)

$sum = New-EvidenceWriter $slogPath
$sum.WriteLine("# verify-3.2.ps1 summary $(Get-Ts)")
foreach ($k in $results.Keys) { $sum.WriteLine("$(if ($results[$k]) { 'PASS' } else { 'FAIL' })  $k") }
foreach ($v in $violations) { $sum.WriteLine("  violation: $v") }
foreach ($f in $firstFg) { $sum.WriteLine("  fg=host: $f") }
foreach ($s in $skippedHidden) { $sum.WriteLine("  skipped(visible=0，列舉途中剛好顯示並置底): $s") }
$sum.Close()
Get-Content $slogPath
Remove-Item -Recurse -Force $tempRoot -ErrorAction SilentlyContinue
