<#
.SYNOPSIS
    驗證 WebView2 故障復原（task 5.6，design.md D12）之後內容倍率是否仍然保留——renderer 終止
    觸發的 `Reload()`（同一視窗、同一 WebView2 controller）與 browser process 終止觸發的「先建後拆
    全部重建」（新視窗、新 controller，走 `create_widget_window`）兩條路徑各驗一次。task 7.7 起
    改為格線版面：倍率不再是設定欄位（已刪除的 `placement.scale`），而是**由視窗寬度決定**
    （design.md D7：zoom＝矩形邏輯寬 ÷ 設計寬度，夾 0.5–3；`WebviewWindow::set_zoom` →
    WebView2 `SetZoomFactor`）。

.DESCRIPTION
    判準：WebView2 `SetZoomFactor` 會等比例調高頁面回報的 `window.devicePixelRatio`（Chromium
    行為，`devicePixelRatio = 螢幕 dpr × 頁面 zoom`）。期望值由視窗矩形推算：
    `螢幕 dpr = GetDpiForWindow / 96`、`zoom = clamp((實體寬 ÷ 螢幕 dpr) ÷ 設計寬度 500, 0.5, 3)`、
    `期望 dpr = zoom × 螢幕 dpr`（clock 設計寬度見 host/src/widgets.rs `WIDGET_SPECS`），容許 ±0.01。
    Reload／全部重建之後 dpr 若掉回螢幕 dpr（zoom 1）或偏離期望值，就代表倍率沒有保留。
    `#widget-root` 的 CSS 高度 × dpr 與視窗 ClientH 只記錄參考、不列入判定（task 3.2 時代「視窗
    高度＝內容」的判準；格線版面下 WebView2 viewport 換算有 1–2 實體像素的捨入差）。
    期望倍率恰好等於 1 時這個判準分不出來，記錄檔會註明。

    步驟：
      1. 暫存 %APPDATA%／%LOCALAPPDATA%＋固定 fixture（同 verify-3.1.ps1），啟動宿主（預設版面）。
      2. 等 clock 小工具就緒，量測 baseline：dpr＝由矩形推算的期望值。
      3. R：終止本宿主一個 renderer（`--type=renderer`），直到 clock 的 `performance.timeOrigin`
         改變（＝被 `Reload()` 的那個）——同一 HWND、同一 WebView2 controller。等頁面就緒後
         再量一次：dpr 仍＝矩形推算值。
      4. B：終止本宿主的 browser process（父行程＝宿主、命令列無 `--type=`）→ 全部重建（新
         HWND、新 controller，走 `create_widget_window`）。clock 視窗重新出現後再量一次。
      5. 結束宿主，只清本次行程樹內的 WebView2（**絕不 `taskkill /IM`**，同 verify-5.6.ps1）。

    不注入任何輸入（不送按鍵、不點擊、不截圖），鎖定時也可跑。

.PARAMETER Exe
    fc-host.exe 路徑，預設 host/target/release/fc-host.exe。

.PARAMETER OutDir
    證據輸出目錄，預設 host/tools/evidence。

.PARAMETER CdpPort
    本次專屬的 WebView2 remote debugging 埠（預設 9358，未被其他驗收腳本使用）。
#>
[CmdletBinding()]
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\fc-host.exe'),
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [int]$CdpPort = 9358
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\AutostartRegistry.psm1') -Force

Add-Type -Namespace VZoom -Name Native -MemberDefinition @'
[DllImport("user32.dll")] public static extern System.IntPtr SetThreadDpiAwarenessContext(System.IntPtr ctx);
[DllImport("user32.dll")] public static extern System.IntPtr GetTopWindow(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern System.IntPtr GetWindow(System.IntPtr hWnd, uint cmd);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr hWnd, out uint pid);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(System.IntPtr hWnd, System.Text.StringBuilder s, int n);
[DllImport("user32.dll")] public static extern bool GetClientRect(System.IntPtr hWnd, out RECT r);
[DllImport("user32.dll")] public static extern bool GetWindowRect(System.IntPtr hWnd, out RECT r);
[DllImport("user32.dll")] public static extern uint GetDpiForWindow(System.IntPtr hWnd);
[StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
'@

[void][VZoom.Native]::SetThreadDpiAwarenessContext([IntPtr](-4))

function Get-Ts { Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fffK' }
function Test-Locked { [bool](Get-Process -Name LogonUI, LockApp -ErrorAction SilentlyContinue) }

$Node = (Get-Command node).Source
$CdpScript = Join-Path $PSScriptRoot 'cdp-widgets.mjs'
$CdpEval = Join-Path $PSScriptRoot 'host-cdp-eval.mjs'
$PortMarker = "--remote-debugging-port=$CdpPort"
$ExeMarker = 'webview-exe-name=fc-host.exe'

# ── 視窗（供比對 HWND 是否變化＝重建 vs 同一視窗 Reload） ─────────────────────────────
function Get-ClockWindow([int]$ProcId) {
    $h = [VZoom.Native]::GetTopWindow([IntPtr]::Zero)
    while ($h -ne [IntPtr]::Zero) {
        $p = 0
        [void][VZoom.Native]::GetWindowThreadProcessId($h, [ref]$p)
        if ($p -eq $ProcId -and [VZoom.Native]::IsWindowVisible($h)) {
            $sb = New-Object System.Text.StringBuilder 256
            [void][VZoom.Native]::GetWindowText($h, $sb, 256)
            if ($sb.ToString() -eq 'fc-host clock') {
                $c = New-Object VZoom.Native+RECT
                [void][VZoom.Native]::GetClientRect($h, [ref]$c)
                $r = New-Object VZoom.Native+RECT
                [void][VZoom.Native]::GetWindowRect($h, [ref]$r)
                return [pscustomobject]@{
                    Hwnd    = ('0x{0:X}' -f $h.ToInt64())
                    ClientH = $c.Bottom
                    ClientW = $c.Right
                    Width   = $r.Right - $r.Left
                    Dpi     = [VZoom.Native]::GetDpiForWindow($h)
                }
            }
        }
        $h = [VZoom.Native]::GetWindow($h, 2)
    }
    return $null
}

# ── 行程識別（本宿主行程樹＋命令列標記，同 verify-5.6.ps1） ────────────────────────────
function Get-HostTree([int]$RootPid) {
    $all = @(Get-CimInstance Win32_Process)
    $children = @{}
    foreach ($p in $all) {
        $key = [int]$p.ParentProcessId
        if (-not $children.ContainsKey($key)) { $children[$key] = New-Object System.Collections.ArrayList }
        [void]$children[$key].Add($p)
    }
    $result = New-Object System.Collections.ArrayList
    $queue = New-Object System.Collections.Queue
    $queue.Enqueue($RootPid)
    $seen = @{}
    while ($queue.Count -gt 0) {
        $cur = [int]$queue.Dequeue()
        if ($seen.ContainsKey($cur)) { continue }
        $seen[$cur] = $true
        if ($children.ContainsKey($cur)) {
            foreach ($c in $children[$cur]) {
                [void]$result.Add($c)
                $queue.Enqueue([int]$c.ProcessId)
            }
        }
    }
    return $result
}
function Get-OurWv2([int]$HostPid) {
    @(Get-HostTree $HostPid | Where-Object {
        $_.Name -eq 'msedgewebview2.exe' -and $_.CommandLine -and
        $_.CommandLine.Contains($ExeMarker) -and $_.CommandLine.Contains($PortMarker)
    })
}
function Get-Wv2Type($p) {
    if ($p.CommandLine -match '--type=([a-z-]+)') { return $Matches[1] }
    return 'browser'
}
function Stop-Ours([int]$HostPid, [int]$TargetPid, [string]$why) {
    $procs = Get-OurWv2 $HostPid
    $target = $procs | Where-Object { [int]$_.ProcessId -eq $TargetPid }
    if (-not $target) {
        W "REFUSE 終止 pid=$TargetPid（$why）：不在本宿主行程樹內或命令列標記不符"
        return $false
    }
    W "KILL pid=$TargetPid type=$(Get-Wv2Type $target) ppid=$($target.ParentProcessId)（$why）"
    Stop-Process -Id $TargetPid -Force
    return $true
}

# ── CDP ────────────────────────────────────────────────────────────────────────
function Get-ClockState {
    $raw = & $Node $CdpScript "$CdpPort" state 2>$null
    if (-not $raw) { return $null }
    $rows = $raw | ConvertFrom-Json
    return ($rows | Where-Object { $_.kind -eq 'widget' -and $_.id -eq 'clock' } | Select-Object -First 1)
}
function Wait-ClockReady([int]$TimeoutSec = 30) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt $TimeoutSec) {
        $s = Get-ClockState
        if ($s -and $s.ok) { return $s }
        Start-Sleep -Milliseconds 300
    }
    return $null
}
function Wait-ClockReloaded([double]$PrevT0, [int]$TimeoutSec = 15) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt $TimeoutSec) {
        $s = Get-ClockState
        if ($s -and $s.ok -and $s.timeOrigin -ne $PrevT0) { return $s }
        Start-Sleep -Milliseconds 300
    }
    return $null
}
function Invoke-Clock([string]$Expr) {
    $out = & $Node $CdpEval "$CdpPort" 'w=clock' $Expr 2>&1
    if ($LASTEXITCODE -ne 0) { return $null }
    try { return ($out | ConvertFrom-Json) } catch { return $null }
}

# clock 的設計寬度（host/src/widgets.rs `WIDGET_SPECS`）；倍率夾限同 design.md D7。
$DesignWidth = 500.0
$ZoomMin = 0.5; $ZoomMax = 3.0

# 量測：頁面 dpr 與 `#widget-root` CSS 高度；期望 dpr 由視窗實體寬與螢幕 DPI 推算。
function Measure-Zoom([string]$Tag) {
    $c = Invoke-Clock "(() => { const r = document.getElementById('widget-root').getBoundingClientRect(); return { h: r.height, dpr: window.devicePixelRatio }; })()"
    $win = Get-ClockWindow $hostPid
    if (-not $c -or -not $win) {
        W "MEASURE[$Tag] 失敗：page=$(if ($c) { 'ok' } else { 'null' }) win=$(if ($win) { 'ok' } else { 'null' })"
        return [pscustomobject]@{ tag = $Tag; ok = $false }
    }
    $screenDpr = $win.Dpi / 96.0
    $zoom = [math]::Min($ZoomMax, [math]::Max($ZoomMin, ($win.Width / $screenDpr) / $DesignWidth))
    $expectedDpr = $zoom * $screenDpr
    $dprOk = ([math]::Abs($c.dpr - $expectedDpr) -le 0.01)
    $physical = $c.h * $c.dpr
    $diff = [math]::Abs($win.ClientH - $physical)
    $fits = ($diff -le 1)
    W "MEASURE[$Tag] 視窗hwnd=$($win.Hwnd) 實體寬=$($win.Width) 螢幕dpr=$screenDpr 推算zoom=$([math]::Round($zoom, 4)) 期望dpr=$([math]::Round($expectedDpr, 4)) 頁面dpr=$($c.dpr) 相符=$dprOk；#widget-root CSS 高=$($c.h) 實體高=$physical ClientH=$($win.ClientH) diff=$diff 填滿=$fits"
    [pscustomobject]@{ tag = $Tag; ok = $true; dpr = $c.dpr; expectedDpr = $expectedDpr; screenDpr = $screenDpr; dprOk = $dprOk; fits = $fits; hwnd = $win.Hwnd }
}

# ── 0. 前置 ─────────────────────────────────────────────────────────────────────
if (Get-Process -Name fc-host -ErrorAction SilentlyContinue) {
    throw '已有 fc-host 在執行，請先結束（避免行程樹與 CDP 埠互相干擾）。'
}
$Exe = (Resolve-Path $Exe).Path
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$logPath = Join-Path $OutDir 'zoom-reload-driver.log'
$sumPath = Join-Path $OutDir 'zoom-reload-summary.log'
$log = New-EvidenceWriter $logPath
$log.AutoFlush = $true
function W([string]$msg) { $log.WriteLine("$(Get-Ts) $msg") }
W "# verify-zoom-reload.ps1 exe=$Exe cdpPort=$CdpPort locked=$([int](Test-Locked))"

$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ("fc-host-zoom-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
$tempAppData = Join-Path $tempRoot 'Roaming'
$tempLocalAppData = Join-Path $tempRoot 'Local'
$dataDir = Join-Path $tempLocalAppData 'tw.fintools.fc-host\data'
New-Item -ItemType Directory -Force -Path $tempAppData, $dataDir | Out-Null
Copy-Item (Join-Path $PSScriptRoot '..\ui\fixtures\tw-events.json') (Join-Path $dataDir 'tw_events.json')
W "# APPDATA=$tempAppData LOCALAPPDATA=$tempLocalAppData dataDir=$dataDir（fixture=tw-events.json）"

$regSnap = @(Save-FcHostAutostartRegistry)
W "開機自啟登錄快照：$(($regSnap | ForEach-Object { "$($_.Name)@$(Split-Path $_.Key -Leaf) Exists=$($_.Exists)" }) -join '; ')"
$results = [ordered]@{}
$hostProc = $null
$hostPid = 0
$knownWv2 = @{}
$oldAppData = $env:APPDATA; $oldLocal = $env:LOCALAPPDATA; $oldWv2 = $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS
function Remember-Wv2 { foreach ($p in (Get-OurWv2 $hostPid)) { $knownWv2[[int]$p.ProcessId] = $p.CreationDate } }

try {
    try {
        $env:APPDATA = $tempAppData
        $env:LOCALAPPDATA = $tempLocalAppData
        $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $PortMarker
        $hostProc = Start-Process -FilePath $Exe -PassThru
    } finally {
        $env:APPDATA = $oldAppData; $env:LOCALAPPDATA = $oldLocal; $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $oldWv2
    }
    $hostPid = $hostProc.Id
    W "# 宿主 pid=$hostPid"

    # ── 1. clock 就緒＋baseline：dpr＝由矩形推算的期望值 ──────────────────────────────────
    $s0 = Wait-ClockReady 60
    $results['1 clock 初始就緒'] = [bool]$s0
    Start-Sleep -Seconds 1
    $base = Measure-Zoom 'baseline'
    $results['1 baseline：頁面 dpr＝由視窗寬推算的倍率 × 螢幕 dpr'] = $base.ok -and $base.dprOk
    if ($base.ok -and [math]::Abs($base.expectedDpr - $base.screenDpr) -le 0.01) {
        W 'NOTE 期望倍率恰好為 1：Reload 後倍率掉回 1 在本機分不出來（判準失去鑑別力）'
    }
    Remember-Wv2

    # ── 3. R：終止本宿主一個 renderer，直到 clock 被 Reload（同 HWND、同 controller） ────────
    $rPassed = $false
    $tried = @()
    $hwndBeforeR = (Get-ClockWindow $hostPid).Hwnd
    for ($attempt = 1; $attempt -le 5 -and -not $rPassed; $attempt++) {
        $renderers = @(Get-OurWv2 $hostPid | Where-Object { (Get-Wv2Type $_) -eq 'renderer' -and $tried -notcontains [int]$_.ProcessId })
        if ($renderers.Count -eq 0) { W 'R 找不到可終止的 renderer'; break }
        $victim = [int]$renderers[0].ProcessId
        $tried += $victim
        $prevT0 = (Get-ClockState).timeOrigin
        if (-not (Stop-Ours $hostPid $victim "R attempt=$attempt")) { break }
        $reloaded = Wait-ClockReloaded $prevT0 12
        if ($reloaded) {
            W "R attempt=$attempt clock 已 Reload（timeOrigin 改變）"
            $rPassed = $true
        } else {
            W "R attempt=$attempt clock 未受影響（可能是其他小工具的 renderer），換下一個"
            Start-Sleep -Milliseconds 500
        }
        Remember-Wv2
    }
    $results['3 R：終止 renderer 後 clock 被 Reload'] = $rPassed
    if ($rPassed) {
        Start-Sleep -Seconds 1
        $hwndAfterR = (Get-ClockWindow $hostPid).Hwnd
        $results['3 R：Reload 後仍是同一個 HWND（同一 WebView2 controller）'] = ($hwndAfterR -eq $hwndBeforeR)
        $afterR = Measure-Zoom 'after-renderer-reload'
        $results['3 R：Reload 後倍率仍＝由視窗寬推算的值（頁面 dpr 相符）'] = $afterR.ok -and $afterR.dprOk
    } else {
        $results['3 R：Reload 後仍是同一個 HWND（同一 WebView2 controller）'] = $false
        $results['3 R：Reload 後倍率仍＝由視窗寬推算的值（頁面 dpr 相符）'] = $false
    }

    # ── 4. B：終止 browser process → 全部重建（新 HWND、新 controller） ───────────────────
    $prevT0B = (Get-ClockState).timeOrigin
    $hwndBeforeB = (Get-ClockWindow $hostPid).Hwnd
    $browser = @(Get-OurWv2 $hostPid | Where-Object { (Get-Wv2Type $_) -eq 'browser' -and [int]$_.ParentProcessId -eq $hostPid })
    $bPassed = $false
    if ($browser.Count -eq 1 -and (Stop-Ours $hostPid ([int]$browser[0].ProcessId) 'B')) {
        $reloadedB = Wait-ClockReloaded $prevT0B 15
        $bPassed = [bool]$reloadedB
    }
    $results['4 B：終止 browser 後 clock 全部重建（新頁面）'] = $bPassed
    Remember-Wv2
    if ($bPassed) {
        Start-Sleep -Seconds 1
        $hwndAfterB = (Get-ClockWindow $hostPid).Hwnd
        $results['4 B：重建後是新的 HWND（非沿用舊視窗）'] = ($hwndAfterB -ne $hwndBeforeB) -and $hwndAfterB
        $afterB = Measure-Zoom 'after-browser-rebuild'
        $results['4 B：全部重建後倍率仍＝由視窗寬推算的值（頁面 dpr 相符）'] = $afterB.ok -and $afterB.dprOk
    } else {
        $results['4 B：重建後是新的 HWND（非沿用舊視窗）'] = $false
        $results['4 B：全部重建後倍率仍＝由視窗寬推算的值（頁面 dpr 相符）'] = $false
    }
}
finally {
    $locked2 = Test-Locked
    if ($hostProc -and -not $hostProc.HasExited) {
        Remember-Wv2
        Stop-Process -Id $hostProc.Id -Force -ErrorAction SilentlyContinue
        [void]$hostProc.WaitForExit(10000)
    }
    Start-Sleep -Seconds 2
    $left = @()
    foreach ($procId in @($knownWv2.Keys)) {
        $p = Get-CimInstance Win32_Process -Filter "ProcessId=$procId" -ErrorAction SilentlyContinue
        if ($p -and $p.CreationDate -eq $knownWv2[$procId] -and $p.CommandLine -and
            $p.CommandLine.Contains($ExeMarker) -and $p.CommandLine.Contains($PortMarker)) {
            $left += $procId
            Stop-Process -Id $procId -Force -ErrorAction SilentlyContinue
        }
    }
    Start-Sleep -Seconds 1
    $stillHost = @(Get-Process -Name fc-host -ErrorAction SilentlyContinue).Count
    $stillWv2 = @(Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" | Where-Object {
        $_.CommandLine -and $_.CommandLine.Contains($ExeMarker) -and $_.CommandLine.Contains($PortMarker) }).Count
    $regLeft = @(Restore-FcHostAutostartRegistry $regSnap)
    if ($regLeft.Count -gt 0) { Write-Warning "開機自啟登錄未還原：$($regLeft -join '; ')" }
    W "開機自啟登錄還原：未還原 $($regLeft.Count) 項$(if ($regLeft.Count) { '：' + ($regLeft -join '; ') })"
    $results['開機自啟登錄已還原（Run／StartupApproved 的 fc-host）'] = ($regLeft.Count -eq 0)
    W "# END locked=$([int]$locked2) leftoverKilled=[$($left -join ',')] fcHostRemaining=$stillHost ourWebView2Remaining=$stillWv2"
    $results['E 結束後無殘留 fc-host 與本次 WebView2 行程'] = ($stillHost -eq 0) -and ($stillWv2 -eq 0)
    $log.Close()
}

$sum = New-EvidenceWriter $sumPath
$sum.WriteLine("# verify-zoom-reload.ps1 summary $(Get-Ts)")
foreach ($k in $results.Keys) { $sum.WriteLine("$(if ($results[$k]) { 'PASS' } else { 'FAIL' })  $k") }
$sum.Close()
Get-Content $sumPath

Remove-Item -Recurse -Force $tempRoot -ErrorAction SilentlyContinue

if (@($results.Values | Where-Object { -not $_ }).Count -gt 0) { exit 1 }
exit 0
