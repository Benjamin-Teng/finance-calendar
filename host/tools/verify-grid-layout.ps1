<#
.SYNOPSIS
    Task 7.7 格線版面驗收（design.md D7「格線座標」；specs/widget-host-windows「版面格線」
    「預設版面」）：列舉宿主所有可見的小工具頂層視窗，斷言同一台顯示器上的矩形兩兩不相交、
    每條邊都落在該顯示器工作區的格線像素上，並輸出每扇視窗反推的格座標。不注入任何輸入。

.DESCRIPTION
    格線像素：`edge(i) = rcWork 起點 + floor(i × 長度 / 48)`（i＝0..48，整數運算；x 用工作區
    左緣與寬、y 用上緣與高）。視窗所屬顯示器＝矩形中心所在者（`MonitorFromPoint`，
    MONITOR_DEFAULTTONEAREST），工作區取 `GetMonitorInfo` 的 rcWork（實體像素，本執行緒設
    Per-Monitor-V2）。

    兩種用法：
    - `-HostPid <pid>`：只讀一個已在執行的宿主（不啟動、不結束它），適合任何時候對帳目前版面。
    - 不給 `-HostPid`：以全新暫存 %APPDATA%／%LOCALAPPDATA% 首次啟動宿主（＝預設版面；資料目錄
      放 `-DataFile` 的複本），另外斷言反推格座標＝預設格座標、以 CDP 讀每個頁面 `.panel` 的
      scrollHeight／clientHeight（時鐘不得被裁切）與其內層 overflow-y 為 hidden／clip 的容器
      （`Get-InnerClipExpr`，scrollHeight 不得大於 clientHeight）、台股動態事件下緣不低於行情條上緣，並以
      `PrintWindow`（PW_RENDERFULLCONTENT）逐窗擷取、依相對位置合成截圖（只含小工具本身，背景
      填深灰，不含桌面圖示、檔名或其他視窗）。結束時只結束自己啟動的宿主與其 WebView2，比對真正
      設定檔雜湊與 HKCU Run。

    記錄檔中的使用者設定檔路徑由 `lib/EvidenceLog.psm1` 改寫成 %TEMP% 等字樣。

.PARAMETER HostPid
    已在執行的 fc-host 行程 id；0（預設）＝自己首次啟動一個。

.PARAMETER Tag
    證據檔名後綴：記錄 `7.7-grid-layout-<Tag>.log`，截圖 `7.7-default-<Tag>.png`。

.EXAMPLE
    pwsh -File host/tools/verify-grid-layout.ps1 -Tag laptop
    pwsh -File host/tools/verify-grid-layout.ps1 -HostPid (Get-Process fc-host).Id -Tag current
#>
[CmdletBinding()]
param(
    [int]$HostPid = 0,
    [string]$Tag = 'laptop',
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\fc-host.exe'),
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [string]$DataFile = 'D:\finance-calendar\tw_events.json',
    [int]$CdpPort = 9380,
    [switch]$NoScreenshot
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\AutostartRegistry.psm1') -Force

Add-Type -AssemblyName System.Drawing
Add-Type -Namespace VGrid -Name Native -MemberDefinition @'
[DllImport("user32.dll")] public static extern System.IntPtr SetThreadDpiAwarenessContext(System.IntPtr ctx);
[DllImport("user32.dll")] public static extern System.IntPtr GetTopWindow(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern System.IntPtr GetWindow(System.IntPtr hWnd, uint cmd);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern bool IsIconic(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr hWnd, out uint pid);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(System.IntPtr hWnd, System.Text.StringBuilder s, int n);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassName(System.IntPtr hWnd, System.Text.StringBuilder s, int n);
[DllImport("user32.dll")] public static extern bool GetWindowRect(System.IntPtr hWnd, out RECT r);
[DllImport("user32.dll")] public static extern uint GetDpiForWindow(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern System.IntPtr MonitorFromPoint(POINT pt, uint flags);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern bool GetMonitorInfoW(System.IntPtr hMon, ref MONITORINFOEX mi);
[DllImport("user32.dll")] public static extern bool PrintWindow(System.IntPtr h, System.IntPtr hdc, uint flags);
[DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(System.IntPtr h, int attr, out int v, int size);
[StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
[StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
[StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)] public struct MONITORINFOEX {
  public int cbSize; public RECT rcMonitor; public RECT rcWork; public uint dwFlags;
  [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string szDevice; }
'@

# Per-Monitor-V2（-4）：GetWindowRect／GetMonitorInfo 回實體像素。
[void][VGrid.Native]::SetThreadDpiAwarenessContext([IntPtr](-4))

$GRID = 48
# 預設格座標（只在首次啟動模式比對）。來源：host/src/settings.rs `DEFAULT_GRID_RECTS`（順序同
# `WIDGET_IDS`）；那邊改了這裡要跟著改——這是驗收用的期望值，不是第四份註冊表。
$Defaults = [ordered]@{
    clock = '15,1,16,10'; macro = '15,12,16,30'; fixed = '32,1,15,17'; dynamic = '32,19,15,23'; quotes = '15,43,32,4'
}

function Get-Ts { Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fffK' }
function Fmt($r) { "($($r.X),$($r.Y),$($r.W)x$($r.H))" }
function Edge([int]$Origin, [int]$Extent, [int]$I) { $Origin + [int][math]::Floor([int64]$I * $Extent / $GRID) }
# 像素值 → 格線索引（不在任何格線上回傳 $null）。
function Get-GridIndex([int]$Value, [int]$Origin, [int]$Extent) {
    for ($i = 0; $i -le $GRID; $i++) { if ((Edge $Origin $Extent $i) -eq $Value) { return $i } }
    return $null
}

function Get-WidgetWindows([int]$ProcId) {
    $list = New-Object System.Collections.Generic.List[object]
    $h = [VGrid.Native]::GetTopWindow([IntPtr]::Zero)
    while ($h -ne [IntPtr]::Zero) {
        $p = 0
        [void][VGrid.Native]::GetWindowThreadProcessId($h, [ref]$p)
        if ($p -eq $ProcId -and [VGrid.Native]::IsWindowVisible($h) -and -not [VGrid.Native]::IsIconic($h)) {
            $t = New-Object System.Text.StringBuilder 256; [void][VGrid.Native]::GetWindowText($h, $t, 256)
            $c = New-Object System.Text.StringBuilder 256; [void][VGrid.Native]::GetClassName($h, $c, 256)
            $cloaked = 0; [void][VGrid.Native]::DwmGetWindowAttribute($h, 14, [ref]$cloaked, 4)  # DWMWA_CLOAKED
            if ($t.ToString().StartsWith('fc-host ') -and $c.ToString() -eq 'Tauri Window' -and $cloaked -eq 0) {
                $r = New-Object VGrid.Native+RECT
                [void][VGrid.Native]::GetWindowRect($h, [ref]$r)
                $list.Add([PSCustomObject]@{
                        Id = $t.ToString().Substring(8); Hwnd = $h
                        X = $r.Left; Y = $r.Top; W = $r.Right - $r.Left; H = $r.Bottom - $r.Top
                        Dpi = [VGrid.Native]::GetDpiForWindow($h)
                    })
            }
        }
        $h = [VGrid.Native]::GetWindow($h, 2)
    }
    return $list.ToArray()
}

function Get-MonitorOf($Win) {
    $pt = New-Object VGrid.Native+POINT
    $pt.X = $Win.X + [int][math]::Floor($Win.W / 2); $pt.Y = $Win.Y + [int][math]::Floor($Win.H / 2)
    $hm = [VGrid.Native]::MonitorFromPoint($pt, 2)  # MONITOR_DEFAULTTONEAREST
    $mi = New-Object VGrid.Native+MONITORINFOEX
    $mi.cbSize = [Runtime.InteropServices.Marshal]::SizeOf($mi)
    [void][VGrid.Native]::GetMonitorInfoW($hm, [ref]$mi)
    [PSCustomObject]@{
        Key = $hm.ToInt64(); Device = $mi.szDevice; Primary = [bool]($mi.dwFlags -band 1)
        Work = [PSCustomObject]@{ X = $mi.rcWork.Left; Y = $mi.rcWork.Top; W = $mi.rcWork.Right - $mi.rcWork.Left; H = $mi.rcWork.Bottom - $mi.rcWork.Top }
    }
}

function Test-Overlap($A, $B) {
    ($A.X -lt $B.X + $B.W) -and ($B.X -lt $A.X + $A.W) -and ($A.Y -lt $B.Y + $B.H) -and ($B.Y -lt $A.Y + $A.H)
}

function Invoke-Eval([string]$Page, [string]$Expr) {
    $out = & node (Join-Path $PSScriptRoot 'host-cdp-eval.mjs') $CdpPort $Page $Expr 2>&1
    return ($out -join "`n").Trim()
}

# 7.7-low (a)：內層容器的裁切。`.panel` 本身量起來剛好時，內層 overflow-y 為 hidden／clip 的容器
# 仍可能把內容裁掉；逐一比較 scrollHeight 與 clientHeight（overflow auto／scroll 是刻意的捲動區，
# 不算）。回傳 { checked: 檢查了幾個（-1＝找不到 .panel）, clipped: [{ el, sh, ch }] }。
function Get-InnerClipExpr {
    return '(() => { const root = document.querySelector(''#widget-root > .panel''); if (!root) return { checked: -1, clipped: [] }; const clipped = []; let checked = 0; for (const el of root.querySelectorAll(''*'')) { const oy = getComputedStyle(el).overflowY; if (oy !== ''hidden'' && oy !== ''clip'') continue; checked++; if (el.scrollHeight > el.clientHeight) { const cls = (typeof el.className === ''string'' && el.className.trim()) ? ''.'' + el.className.trim().split(/\s+/).join(''.'') : ''''; clipped.push({ el: el.tagName.toLowerCase() + (el.id ? ''#'' + el.id : '''') + cls, sh: el.scrollHeight, ch: el.clientHeight }); } } return { checked, clipped }; })()'
}

# 解析 Get-InnerClipExpr 的 CDP 回傳（JSON）；回傳 { Ok, Checked, Bad=[說明字串] }。不是 JSON、
# 找不到 .panel 都算不 Ok（無法證明沒有裁切）。
function Test-InnerClipResult([string]$Raw) {
    $j = $null
    try { if ($Raw) { $j = $Raw | ConvertFrom-Json -ErrorAction Stop } } catch { $j = $null }
    if ($null -eq $j -or $null -eq $j.PSObject.Properties['checked'] -or $null -eq $j.PSObject.Properties['clipped']) {
        return [PSCustomObject]@{ Ok = $false; Checked = -1; Bad = @("無法解析 CDP 回傳：$Raw") }
    }
    if ([int]$j.checked -lt 0) {
        return [PSCustomObject]@{ Ok = $false; Checked = -1; Bad = @('找不到 #widget-root > .panel') }
    }
    $bad = @(@($j.clipped) | Where-Object { $null -ne $_ } | ForEach-Object { "$($_.el) scrollH=$($_.sh) > clientH=$($_.ch)" })
    return [PSCustomObject]@{ Ok = ($bad.Count -eq 0); Checked = [int]$j.checked; Bad = $bad }
}

# 逐窗 PrintWindow 後依相對位置合成：只含小工具本身，背景深灰。
function Save-Composite($Wins, [string]$Path) {
    $minX = ($Wins | Measure-Object X -Minimum).Minimum; $minY = ($Wins | Measure-Object Y -Minimum).Minimum
    $maxX = ($Wins | ForEach-Object { $_.X + $_.W } | Measure-Object -Maximum).Maximum
    $maxY = ($Wins | ForEach-Object { $_.Y + $_.H } | Measure-Object -Maximum).Maximum
    $canvas = New-Object System.Drawing.Bitmap ([int]($maxX - $minX)), ([int]($maxY - $minY))
    $g = [System.Drawing.Graphics]::FromImage($canvas)
    $g.Clear([System.Drawing.Color]::FromArgb(255, 40, 42, 48))
    $allOk = $true
    foreach ($w in $Wins) {
        $bmp = New-Object System.Drawing.Bitmap $w.W, $w.H
        $bg = [System.Drawing.Graphics]::FromImage($bmp)
        $hdc = $bg.GetHdc()
        $ok = [VGrid.Native]::PrintWindow($w.Hwnd, $hdc, 2)
        $bg.ReleaseHdc($hdc); $bg.Dispose()
        if (-not $ok) { $allOk = $false }
        $g.DrawImage($bmp, [int]($w.X - $minX), [int]($w.Y - $minY), $w.W, $w.H)
        $bmp.Dispose()
    }
    $g.Dispose()
    $canvas.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png)
    $canvas.Dispose()
    return $allOk
}

# ── 前置 ─────────────────────────────────────────────────────────────────────────
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$logPath = Join-Path $OutDir "7.7-grid-layout-$Tag.log"
$pngPath = Join-Path $OutDir "7.7-default-$Tag.png"
$log = New-EvidenceWriter $logPath
$log.AutoFlush = $true
function Log([string]$m) { $line = "$(Get-Ts) $m"; $log.WriteLine($line); Write-Host (ConvertTo-EvidenceText $line) }

$launch = ($HostPid -eq 0)
$results = [ordered]@{}
$hostProc = $null
$tempRoot = $null
$realSettings = Join-Path $env:APPDATA 'tw.fintools.fc-host\settings.json'
$realHashBefore = $null; $regSnap = $null

if ($launch -and (Get-Process -Name fc-host -ErrorAction SilentlyContinue)) {
    Log "# BLOCKED：已有 fc-host 在執行，本腳本不結束它；改用 -HostPid 只讀，或先自行關閉。"
    $log.Close(); exit 2
}

try {
    if ($launch) {
        $Exe = (Resolve-Path $Exe).Path
        $realHashBefore = if (Test-Path $realSettings) { (Get-FileHash $realSettings).Hash } else { '<不存在>' }
        $regSnap = @(Save-FcHostAutostartRegistry)
        Log "# verify-grid-layout.ps1 模式=首次啟動 exe=$Exe"
        Log "# 真正的設定檔雜湊（開始）=$realHashBefore；開機自啟登錄快照：$(($regSnap | ForEach-Object { "$($_.Name)@$(Split-Path $_.Key -Leaf) Exists=$($_.Exists)" }) -join '; ')"
        $tempRoot = Join-Path ([IO.Path]::GetTempPath()) ('fc-host-7.7g-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
        $tempAppData = Join-Path $tempRoot 'Roaming'
        $tempLocal = Join-Path $tempRoot 'Local'
        $dataDir = Join-Path $tempLocal 'tw.fintools.fc-host\data'
        New-Item -ItemType Directory -Force -Path $tempAppData, $dataDir | Out-Null
        if (Test-Path $DataFile) { Copy-Item $DataFile (Join-Path $dataDir 'tw_events.json') }
        Log "# 暫存 APPDATA=$tempAppData LOCALAPPDATA=$tempLocal（資料：$DataFile 複本）"
        $old = @($env:APPDATA, $env:LOCALAPPDATA, $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS)
        try {
            $env:APPDATA = $tempAppData; $env:LOCALAPPDATA = $tempLocal
            $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$CdpPort"
            $hostProc = Start-Process -FilePath $Exe -PassThru
        } finally {
            $env:APPDATA, $env:LOCALAPPDATA, $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $old
        }
        $HostPid = $hostProc.Id
        Log "# 宿主 pid=$HostPid"
        $sw = [Diagnostics.Stopwatch]::StartNew()
        while ($sw.Elapsed.TotalSeconds -lt 40 -and @(Get-WidgetWindows $HostPid).Count -lt 5) { Start-Sleep -Milliseconds 300 }
        Start-Sleep -Seconds 5   # 等資料載入、頁面排版與倍率套用完成
    } else {
        Log "# verify-grid-layout.ps1 模式=只讀既有宿主 pid=$HostPid"
    }

    $wins = @(Get-WidgetWindows $HostPid)
    Log "## 可見小工具視窗 $($wins.Count) 扇"
    $rows = @()
    foreach ($w in $wins) {
        $m = Get-MonitorOf $w
        $col = Get-GridIndex $w.X $m.Work.X $m.Work.W
        $right = Get-GridIndex ($w.X + $w.W) $m.Work.X $m.Work.W
        $row = Get-GridIndex $w.Y $m.Work.Y $m.Work.H
        $bottom = Get-GridIndex ($w.Y + $w.H) $m.Work.Y $m.Work.H
        $onGrid = ($null -ne $col -and $null -ne $right -and $null -ne $row -and $null -ne $bottom)
        $gridText = if ($onGrid) { "$col,$row,$($right - $col),$($bottom - $row)" } else { "<不在格線上：左=$col 右=$right 上=$row 下=$bottom>" }
        $rows += [PSCustomObject]@{ Win = $w; Mon = $m; OnGrid = $onGrid; Grid = $gridText }
        Log "  $($w.Id.PadRight(8)) rect=$(Fmt $w) dpi=$($w.Dpi) 顯示器=$($m.Device)（primary=$($m.Primary) 工作區=$(Fmt $m.Work)）反推格座標=$gridText"
    }
    $results['至少一扇可見小工具視窗'] = ($wins.Count -gt 0)
    $results['每扇視窗的四條邊都落在所屬顯示器工作區的格線像素上'] = (@($rows | Where-Object { -not $_.OnGrid }).Count -eq 0)

    $overlaps = @()
    for ($i = 0; $i -lt $rows.Count; $i++) {
        for ($j = $i + 1; $j -lt $rows.Count; $j++) {
            if ($rows[$i].Mon.Key -eq $rows[$j].Mon.Key -and (Test-Overlap $rows[$i].Win $rows[$j].Win)) {
                $overlaps += "$($rows[$i].Win.Id)×$($rows[$j].Win.Id)"
            }
        }
    }
    Log "## 同顯示器相交配對：$(if ($overlaps) { $overlaps -join ', ' } else { '無' })"
    $results['同一台顯示器上的小工具矩形兩兩不相交'] = ($overlaps.Count -eq 0)

    if ($launch) {
        $byId = @{}; foreach ($r in $rows) { $byId[$r.Win.Id] = $r }
        $mismatch = @()
        foreach ($id in $Defaults.Keys) {
            if (-not $byId.ContainsKey($id)) { $mismatch += "$id=<未顯示>" }
            elseif ($byId[$id].Grid -ne $Defaults[$id] -or -not $byId[$id].Mon.Primary) { $mismatch += "$id=$($byId[$id].Grid)（期望 $($Defaults[$id])，primary=$($byId[$id].Mon.Primary)）" }
        }
        $extra = @($rows | Where-Object { -not $Defaults.Contains($_.Win.Id) } | ForEach-Object { $_.Win.Id })
        Log "## 與預設格座標不符：$(if ($mismatch) { $mismatch -join '；' } else { '無' })；預設外的可見小工具：$(if ($extra) { $extra -join ',' } else { '無' })"
        $results['首次啟動：五個財經小工具都在主螢幕、反推格座標＝預設值、擴充插槽不顯示'] = ($mismatch.Count -eq 0 -and $extra.Count -eq 0)

        # 內容是否被裁切：.panel 的 scrollHeight 不得大於 clientHeight（dynamic／macro 清單在
        # 內部捲動區裡捲，不會撐大 .panel 本身），整頁也不得出現捲軸。
        $clipExpr = "(() => { const p = document.querySelector('#widget-root > .panel'); const d = document.documentElement; return p ? [p.scrollHeight, p.clientHeight, p.scrollWidth, p.clientWidth, d.scrollHeight, window.innerHeight, window.devicePixelRatio].join(',') : 'no-panel'; })()"
        $clipBad = @()
        foreach ($id in $Defaults.Keys) {
            $v = Invoke-Eval "w=$id" $clipExpr
            Log "  $id .panel scrollH,clientH,scrollW,clientW,頁面 scrollH,innerH,dpr=$v"
            $n = $v.Trim('"').Split(',')
            if ($n.Count -ne 7 -or [double]$n[0] -gt [double]$n[1] + 0.5 -or [double]$n[2] -gt [double]$n[3] + 0.5 -or [double]$n[4] -gt [double]$n[5] + 0.5) { $clipBad += $id }
        }
        Log "## 內容溢出 .panel 或整頁出現捲軸：$(if ($clipBad) { $clipBad -join ',' } else { '無' })"
        $results['時鐘不被裁切（.panel scrollHeight ≤ clientHeight、頁面無捲軸）'] = ($clipBad -notcontains 'clock')
        $results['五個財經小工具的 .panel 都沒有溢出'] = ($clipBad.Count -eq 0)
        $innerBad = @()
        foreach ($id in $Defaults.Keys) {
            $ic = Test-InnerClipResult (Invoke-Eval "w=$id" (Get-InnerClipExpr))
            Log "  $id 內層 overflow:hidden／clip 容器：檢查 $($ic.Checked) 個；被裁切：$(if ($ic.Bad) { $ic.Bad -join '；' } else { '無' })"
            if (-not $ic.Ok) { $innerBad += $id }
        }
        Log "## 內層容器被裁切：$(if ($innerBad) { $innerBad -join ',' } else { '無' })"
        $results['五個財經小工具的內層 overflow:hidden 容器都沒有裁切（scrollHeight ≤ clientHeight）'] = ($innerBad.Count -eq 0)
        if ($byId.ContainsKey('dynamic') -and $byId.ContainsKey('quotes')) {
            $dynBottom = $byId['dynamic'].Win.Y + $byId['dynamic'].Win.H
            $qTop = $byId['quotes'].Win.Y
            Log "## 台股動態事件下緣 y=$dynBottom、行情條上緣 y=$qTop"
            $results['台股動態事件不蓋行情條（下緣 ≤ 行情條上緣）'] = ($dynBottom -le $qTop)
        } else {
            $results['台股動態事件不蓋行情條（下緣 ≤ 行情條上緣）'] = $false
        }
    }

    if (-not $NoScreenshot -and $wins.Count -gt 0) {
        $shot = Save-Composite $wins $pngPath
        Log "## 截圖（PrintWindow 逐窗合成、只含小工具）=$(Split-Path $pngPath -Leaf) PrintWindow 全部成功=$shot"
        $results['截圖已存（只含小工具區域）'] = $shot
    }
}
catch {
    Log "## 例外中止：$($_.Exception.Message)（第 $($_.InvocationInfo.ScriptLineNumber) 行）"
    $results['腳本跑完（無例外）'] = $false
}
finally {
    if ($launch -and $hostProc) {
        try {
            if (-not $hostProc.HasExited) { Stop-Process -Id $hostProc.Id -Force; $hostProc.WaitForExit(10000) | Out-Null }
            $leaf = Split-Path $tempRoot -Leaf
            Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" -ErrorAction SilentlyContinue |
                Where-Object { $_.CommandLine -and $_.CommandLine -match [regex]::Escape($leaf) } |
                ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
            Start-Sleep -Seconds 2
        } catch { Log "## 收尾結束宿主失敗：$_" }
        $regLeft = @(Restore-FcHostAutostartRegistry $regSnap)
        if ($regLeft.Count -gt 0) { Write-Warning "開機自啟登錄未還原：$($regLeft -join '; ')" }
        Log "# 開機自啟登錄還原：未還原 $($regLeft.Count) 項$(if ($regLeft.Count) { '：' + ($regLeft -join '; ') })"
        $results['開機自啟登錄已還原（Run／StartupApproved 的 fc-host）'] = ($regLeft.Count -eq 0)
        $realHashAfter = if (Test-Path $realSettings) { (Get-FileHash $realSettings).Hash } else { '<不存在>' }
        Log "# 真正的設定檔雜湊（結束）=$realHashAfter；與開始相同=$($realHashAfter -eq $realHashBefore)"
        $results['隔離：真正的設定檔雜湊前後相同'] = ($realHashAfter -eq $realHashBefore)
        if ($tempRoot) { Remove-Item -Recurse -Force $tempRoot -ErrorAction SilentlyContinue }
    }
    foreach ($k in $results.Keys) { Log "$(if ($results[$k]) { 'PASS' } else { 'FAIL' })  $k" }
    Log '# 結束'
    $log.Close()
}

if (@($results.Values | Where-Object { -not $_ }).Count -gt 0) { exit 1 }
exit 0
