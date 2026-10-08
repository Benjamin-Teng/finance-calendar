<#
.SYNOPSIS
    Task 6.3 驗收驅動腳本（widget-adaptive-zoom-and-grid：自適應倍率與字級；design.md D1／D2，
    specs/widget-host-windows「小工具尺寸由版面格決定」、specs/widget-host-lifecycle「設定持久化」）。

.DESCRIPTION
    每個情境在全新的隔離資料夾、**啟動前**寫入 settings.json（`version`＝settings.rs 的
    `SETTINGS_VERSION`，只寫五個財經小工具的開關與格座標、`font_scale`、`data_fetch`=off，其餘由宿主補預設），
    各啟動一次宿主：

    | 情境 | 設定 | 預期 |
    |---|---|---|
    | A 時鐘大框 | 時鐘移到左側空白欄，寬、高都 ≥ 最小框（212×160 邏輯）兩倍（目標約 2.5 倍） | 倍率＝min(邏輯寬/212, 邏輯高/160) 夾 3；文字不裁切 |
    | B 行情條加高 | 行情條寬維持預設 32 格、高改成約 2.75 倍 min 高的格數（總經、動態事件高度讓位） | 倍率＝min(邏輯高/60, 3)；跑馬燈仍在動 |
    | C 清單又寬又矮 | 總經日曆邏輯寬約 2×500、邏輯高約 324（移到左側） | 倍率約 1（由高度決定），不是 2 |
    | D070／D150 字級 | 預設版面，`font_scale` 0.7 與 1.5 | 清單倍率＝min(自適應×字級, 上限)；時鐘、行情條在 150% 時不超過上限 |

    格數依主螢幕當下的工作區與縮放比例換算（目標邏輯尺寸 × 縮放 × 48 ÷ 工作區實體長度），所以換一台
    機器也照同樣的意圖擺放；擺放結果若相交或超出 48 格就該情境 NOT-RUN。

    每個情境、每個財經小工具：
      - 預期倍率＝與 Rust `layout::content_zoom` 相同的公式，輸入是**實際**視窗矩形（GetWindowRect）、該視窗的
        縮放比例（GetDpiForWindow ÷ 96）與 `font_scale`；倍率框（`ZoomBox`）與公式沿用
        host/tests/compare/verify-visual-edges.mjs 匯出的 `widgetZoomBoxes()`／`contentZoom()`（從
        host/src/widgets.rs 的 `WIDGET_SPECS` 讀出，不寫死）。情境本身的意圖另以獨立的簡式交叉核對
        （A：min(寬/212, 高/160, 3)；B：min(高/60, 3)；C：0.9–1.1 且高度比寬度先到；D150：時鐘、行情條＝上限）。
      - 實際倍率＝頁面 `window.devicePixelRatio ÷ 縮放比例`（WebView2 ZoomFactor 等比反映在 devicePixelRatio，
        verify-zoom-reload.ps1 已實測），容差 0.01。
      - 無裁切：時鐘 `.panel` 的 scrollWidth ≤ clientWidth、scrollHeight ≤ clientHeight 且外框在 viewport 內；
        `#clockTime`／`#clockDate`／`#weekRange` 的 scrollWidth ≤ clientWidth、外框在 `.panel` 內，縱向溢出的部分
        （scrollHeight − clientHeight，#clockTime 的字型內容區本來就比行高多約 9 CSS px）仍在 `.panel` 與
        viewport 內（見 `$ClipSpec` 註解）；行情條 `.panel` 高度未溢出（scrollHeight ≤
        clientHeight、下緣在 viewport 內；`.tlist` 是橫向跑馬燈，寬度不算）；清單 `.panel` 寬度未溢出（縱向是
        原生捲動，不算裁切）。
      - 視窗矩形＝設定格座標在主螢幕工作區的格線矩形（design.md D7 `edge`），確認宿主照設定擺放。
      - 設定讀回：`get_settings` 的 `font_scale` 與寫入值相同（設定檔沒有被重設）。
      - 截圖：情境主角（A 時鐘、B 行情條、C 總經日曆、D 全部五個）以 `PrintWindow` 只截小工具視窗本身，
        存 `evidence/adaptive-zoom-<情境>-<id>.png`。
    結果彙整到 `evidence/adaptive-zoom-summary.log`，過程記錄 `adaptive-zoom-driver.log`。

    不注入任何鍵盤／滑鼠輸入。工作階段鎖定時（LogonUI.exe 在跑）BLOCKED，途中鎖定也停止。隔離、開機自啟
    登錄快照還原、收尾只停自己啟動的宿主、證據去識別同 verify-grid-overlay.ps1。

    結束碼：0＝全部通過；1＝有 FAIL；2＝BLOCKED；3＝有 NOT-RUN。

.PARAMETER Scenarios
    要跑的情境（預設 A、B、C、D070、D150 全部）。

.PARAMETER DryRun
    只印出依本機主螢幕換算的各情境格座標與預期倍率（以格線矩形代替實際視窗矩形），不啟動宿主。
#>
[CmdletBinding()]
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\fc-host.exe'),
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [int]$CdpPort = 9364,
    [string[]]$Scenarios = @('A', 'B', 'C', 'D070', 'D150'),
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'

Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\AutostartRegistry.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\ProcessTree.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\VerifyVerdict.psm1') -Force

Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
Add-Type -Namespace VZoomA -Name Native -MemberDefinition @'
[DllImport("user32.dll")] public static extern System.IntPtr SetThreadDpiAwarenessContext(System.IntPtr ctx);
[DllImport("user32.dll")] public static extern System.IntPtr GetTopWindow(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern System.IntPtr GetWindow(System.IntPtr hWnd, uint cmd);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr hWnd, out uint pid);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(System.IntPtr hWnd, System.Text.StringBuilder s, int n);
[DllImport("user32.dll")] public static extern bool GetWindowRect(System.IntPtr hWnd, out RECT r);
[DllImport("user32.dll")] public static extern uint GetDpiForWindow(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern bool PrintWindow(System.IntPtr h, System.IntPtr hdc, uint flags);
[DllImport("user32.dll")] public static extern System.IntPtr MonitorFromPoint(POINT p, uint flags);
[DllImport("shcore.dll")] public static extern int GetDpiForMonitor(System.IntPtr hMon, int type, out uint dpiX, out uint dpiY);
[StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
[StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
'@

# Per-Monitor-V2（-4）：GetWindowRect 與 Screen.WorkingArea 都是實體像素。
[void][VZoomA.Native]::SetThreadDpiAwarenessContext([IntPtr](-4))

$GRID = 48
$FinanceIds = @('clock', 'macro', 'fixed', 'dynamic', 'quotes')
# 預設格座標（host/src/settings.rs `DEFAULT_GRID_RECTS`，順序同上）；改了那邊這裡要跟著改（下方會與 Rust 原始碼核對）。
$DefaultRects = [ordered]@{
    clock = @(15, 1, 16, 10); macro = @(15, 12, 16, 30); fixed = @(32, 1, 15, 17); dynamic = @(32, 19, 15, 23); quotes = @(15, 43, 32, 4)
}
# 情境主角（截圖對象）。
$Shots = @{ A = @('clock'); B = @('quotes'); C = @('macro'); D070 = $FinanceIds; D150 = $FinanceIds }
# 頁面上要檢查裁切的元素（`whole`＝寬高都查；`height`＝只查高；`width`＝只查寬；`text`＝時鐘文字列，見下）。
# `text`：scrollWidth ≤ clientWidth（字寬不超出列寬），外框在 `.panel` 內，且「外框下緣＋(scrollHeight − clientHeight)」
# 不超過 `.panel` 與 viewport 下緣。不直接要求 scrollHeight ≤ clientHeight：#clockTime（52px、line-height 1.1）
# 的字型內容區本來就比行高多約 9 CSS px（headless 實測 66 vs 57，與倍率無關、倍率 1.8／2.5 都一樣），那是
# 行框的版面溢出、不是被裁切；判準改成「溢出的部分仍在面板與視窗內」＝看得到、沒被裁掉。
$ClipSpec = @{
    clock   = @(@('#clockTime', 'text'), @('#clockDate', 'text'), @('#weekRange', 'text'), @('.panel', 'whole'))
    quotes  = @(, @('.panel', 'height'))
    macro   = @(, @('.panel', 'width'))
    fixed   = @(, @('.panel', 'width'))
    dynamic = @(, @('.panel', 'width'))
}

function Get-Ts { Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fffK' }
function Test-Locked { [bool](Get-Process -Name LogonUI -ErrorAction SilentlyContinue) }
function Get-WinPid([IntPtr]$h) { $p = 0; [void][VZoomA.Native]::GetWindowThreadProcessId($h, [ref]$p); [int]$p }
function Get-Title([IntPtr]$h) { $sb = New-Object System.Text.StringBuilder 256; [void][VZoomA.Native]::GetWindowText($h, $sb, 256); $sb.ToString() }
function Get-Rect([IntPtr]$h) {
    $r = New-Object VZoomA.Native+RECT
    [void][VZoomA.Native]::GetWindowRect($h, [ref]$r)
    [PSCustomObject]@{ X = $r.Left; Y = $r.Top; W = $r.Right - $r.Left; H = $r.Bottom - $r.Top }
}
function Fmt($r) { if ($r) { "($($r.X),$($r.Y),$($r.W)x$($r.H))" } else { '<null>' } }
function Same($a, $b) { $a -and $b -and $a.X -eq $b.X -and $a.Y -eq $b.Y -and $a.W -eq $b.W -and $a.H -eq $b.H }
function Edge([int]$Origin, [int]$Extent, [int]$I) { $Origin + [int][math]::Floor([int64]$I * $Extent / $GRID) }
function Grid-Rect($Wa, [int[]]$G) {
    $x0 = Edge $Wa.X $Wa.W $G[0]; $x1 = Edge $Wa.X $Wa.W ($G[0] + $G[2])
    $y0 = Edge $Wa.Y $Wa.H $G[1]; $y1 = Edge $Wa.Y $Wa.H ($G[1] + $G[3])
    [PSCustomObject]@{ X = $x0; Y = $y0; W = $x1 - $x0; H = $y1 - $y0 }
}
function Test-GridOverlap([int[]]$A, [int[]]$B) {
    $A[0] -lt ($B[0] + $B[2]) -and $B[0] -lt ($A[0] + $A[2]) -and $A[1] -lt ($B[1] + $B[3]) -and $B[1] -lt ($A[1] + $A[3])
}
# 目標邏輯長度 → 格數（ceil／round；結果至少 1）。全部用 double 運算（避免 [math] 多載選到整數版本）。
function Get-Cells([double]$Logical, [double]$Scale, [int]$Extent, [switch]$Round) {
    $v = $Logical * $Scale * 48.0 / [double]$Extent
    $n = if ($Round) { [math]::Round($v, [MidpointRounding]::AwayFromZero) } else { [math]::Ceiling($v - 1e-9) }
    [int][math]::Max(1.0, [double]$n)
}

function Find-Window([int]$ProcId, [string]$Title) {
    $h = [VZoomA.Native]::GetTopWindow([IntPtr]::Zero)
    while ($h -ne [IntPtr]::Zero) {
        if ((Get-WinPid $h) -eq $ProcId -and [VZoomA.Native]::IsWindowVisible($h) -and (Get-Title $h) -eq $Title) { return $h }
        $h = [VZoomA.Native]::GetWindow($h, 2)
    }
    return [IntPtr]::Zero
}
function Wait-Window([int]$ProcId, [string]$Title, [int]$TimeoutSec = 60) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt $TimeoutSec) {
        $h = Find-Window $ProcId $Title
        if ($h -ne [IntPtr]::Zero) { return $h }
        Start-Sleep -Milliseconds 250
    }
    return [IntPtr]::Zero
}
function Invoke-Json([string]$Page, [string]$Expr) {
    $out = & node (Join-Path $PSScriptRoot 'host-cdp-eval.mjs') $CdpPort $Page $Expr 2>&1
    $text = ($out -join "`n").Trim()
    if ($LASTEXITCODE -ne 0) { return [PSCustomObject]@{ __error = $text } }
    try { return ($text | ConvertFrom-Json) } catch { return [PSCustomObject]@{ __error = $text } }
}
function Get-WindowBitmap([IntPtr]$H) {
    $r = Get-Rect $H
    if ($r.W -le 0 -or $r.H -le 0) { return $null }   # 視窗已不存在或零尺寸（GetWindowRect 失敗時是 0×0）
    $bmp = New-Object System.Drawing.Bitmap $r.W, $r.H
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $hdc = $g.GetHdc()
    $ok = [VZoomA.Native]::PrintWindow($H, $hdc, 2)
    $g.ReleaseHdc($hdc); $g.Dispose()
    if (-not $ok) { $bmp.Dispose(); return $null }
    return $bmp
}

# 證據檔是否為本次寫出：存在、非空、修改時間不早於 $NotBeforeUtc。
function Test-FreshFile([string]$Path, [datetime]$NotBeforeUtc) {
    $i = Get-Item -LiteralPath $Path -ErrorAction SilentlyContinue
    return [bool]($i -and $i.Length -gt 0 -and $i.LastWriteTimeUtc -ge $NotBeforeUtc)
}
# 只截一扇視窗並存成 PNG（Codex review 6.x：失敗不得只記訊息、舊檔不得冒充）。先刪同名舊檔；刪不掉、PrintWindow
# 失敗、存檔失敗、或存完不是本次的新檔，一律回 Ok=$false。回傳 @{ Ok; Reason }。
function Save-WindowShot([IntPtr]$H, [string]$Path) {
    $t0 = [datetime]::UtcNow.AddSeconds(-1)
    try { if (Test-Path -LiteralPath $Path) { Remove-Item -LiteralPath $Path -Force -ErrorAction Stop } }
    catch { return [PSCustomObject]@{ Ok = $false; Reason = "刪不掉舊檔：$($_.Exception.Message)" } }
    $bmp = Get-WindowBitmap $H
    if (-not $bmp) { return [PSCustomObject]@{ Ok = $false; Reason = 'PrintWindow 失敗（或視窗已不存在）' } }
    try { $bmp.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png) }
    catch { return [PSCustomObject]@{ Ok = $false; Reason = "存檔失敗：$($_.Exception.Message)" } }
    finally { $bmp.Dispose() }
    if (-not (Test-FreshFile $Path $t0)) { return [PSCustomObject]@{ Ok = $false; Reason = '存檔後找不到本次寫出的新檔' } }
    return [PSCustomObject]@{ Ok = $true; Reason = '' }
}

# 預期倍率：一次呼叫 node，沿用 verify-visual-edges.mjs 的 widgetZoomBoxes()／contentZoom()（同 Rust content_zoom）。
# 輸入 [{ id, physW, physH, scale, fontScale }]，回傳 [{ id, box, zoom, zoomAt1, cap }]；cap＝font_scale 極大時的值（上限，已夾 0.5–3）。
$ZoomHelper = @'
import { readFileSync } from 'node:fs';
const [modUrl, inFile] = process.argv.slice(2);
const m = await import(modUrl);
const boxes = m.widgetZoomBoxes();
const input = JSON.parse(readFileSync(inFile, 'utf8'));
const out = input.map((w) => {
  const box = boxes[w.id];
  return {
    id: w.id,
    box,
    zoom: m.contentZoom(w.physW, w.physH, w.scale, box, w.fontScale),
    zoomAt1: m.contentZoom(w.physW, w.physH, w.scale, box, 1),
    cap: m.contentZoom(w.physW, w.physH, w.scale, box, 1e9),
  };
});
console.log(JSON.stringify(out));
'@
function Get-ExpectedZooms([object[]]$Items, [string]$WorkDir) {
    $helper = Join-Path $WorkDir 'zoom-helper.mjs'
    $inFile = Join-Path $WorkDir 'zoom-input.json'
    $enc = New-Object System.Text.UTF8Encoding($false)
    [IO.File]::WriteAllText($helper, $ZoomHelper, $enc)
    [IO.File]::WriteAllText($inFile, (ConvertTo-Json @($Items) -Depth 4), $enc)
    $modUrl = ([Uri](Resolve-Path (Join-Path $PSScriptRoot '..\tests\compare\verify-visual-edges.mjs')).Path).AbsoluteUri
    $out = & node $helper $modUrl $inFile 2>&1
    if ($LASTEXITCODE -ne 0) { throw "預期倍率計算失敗：$($out -join ' ')" }
    $map = @{}
    foreach ($e in (($out -join "`n") | ConvertFrom-Json)) { $map[$e.id] = $e }
    return $map
}

# ── 情境的格座標（依主螢幕工作區與縮放比例換算）────────────────────────────────────────
$primary = @([System.Windows.Forms.Screen]::AllScreens | Where-Object { $_.Primary })[0]
$pwa = [PSCustomObject]@{ X = $primary.WorkingArea.X; Y = $primary.WorkingArea.Y; W = $primary.WorkingArea.Width; H = $primary.WorkingArea.Height }
$pt = New-Object VZoomA.Native+POINT
$pt.X = $primary.Bounds.X + 1; $pt.Y = $primary.Bounds.Y + 1
$dpiX = [uint32]0; $dpiY = [uint32]0
[void][VZoomA.Native]::GetDpiForMonitor([VZoomA.Native]::MonitorFromPoint($pt, 1), 0, [ref]$dpiX, [ref]$dpiY)
$pScale = [double]$dpiX / 96.0

function New-ScenarioPlan([string]$Name) {
    $rects = [ordered]@{}
    foreach ($id in $FinanceIds) { $rects[$id] = [int[]]$DefaultRects[$id].Clone() }
    $font = 1.0
    $note = ''
    switch ($Name) {
        'A' {
            # 時鐘目標 2.5 倍最小框（530×400 邏輯），放在左側 col 0 起的空白欄。
            $w = Get-Cells 530 $pScale $pwa.W; $h = Get-Cells 400 $pScale $pwa.H
            $rects['clock'] = [int[]]@(0, 1, $w, $h)
            $note = '時鐘移到左側空白欄、寬高約 2.5 倍最小框'
        }
        'B' {
            # 行情條寬不變（預設 32 格），高度目標 2.75 倍 min 高（165 邏輯）；總經、動態事件下緣讓位。
            $h = Get-Cells 165 $pScale $pwa.H
            $row = 47 - $h
            $rects['quotes'] = [int[]]@(15, $row, 32, $h)
            $rects['macro'][3] = $row - 1 - $rects['macro'][1]
            $rects['dynamic'][3] = $row - 1 - $rects['dynamic'][1]
            $note = '行情條寬不變、加高'
        }
        'C' {
            # 總經日曆邏輯寬約 2×500、高約 324（四捨五入到格），移到左側 col 0、時鐘之下。
            $w = Get-Cells 1000 $pScale $pwa.W -Round; $h = Get-Cells 324 $pScale $pwa.H -Round
            $rects['macro'] = [int[]]@(0, 12, $w, $h)
            $note = '總經日曆又寬又矮'
        }
        'D070' { $font = 0.7; $note = '預設版面、字級 70%' }
        'D150' { $font = 1.5; $note = '預設版面、字級 150%' }
        default { throw "未知情境 $Name" }
    }
    $problems = @()
    foreach ($id in $FinanceIds) {
        $g = $rects[$id]
        if ($g[2] -lt 1 -or $g[3] -lt 1 -or $g[0] -lt 0 -or $g[1] -lt 0 -or ($g[0] + $g[2]) -gt $GRID -or ($g[1] + $g[3]) -gt $GRID) { $problems += "$id 超出格線 $($g -join ',')" }
    }
    for ($i = 0; $i -lt $FinanceIds.Count; $i++) {
        for ($j = $i + 1; $j -lt $FinanceIds.Count; $j++) {
            if (Test-GridOverlap $rects[$FinanceIds[$i]] $rects[$FinanceIds[$j]]) { $problems += "$($FinanceIds[$i]) 與 $($FinanceIds[$j]) 相交" }
        }
    }
    [PSCustomObject]@{ Name = $Name; Rects = $rects; FontScale = $font; Note = $note; Problems = $problems }
}
function ConvertTo-SettingsJson($Plan, [int]$Version) {
    $widgets = [ordered]@{}
    foreach ($id in $FinanceIds) {
        $g = $Plan.Rects[$id]
        $widgets[$id] = [ordered]@{ enabled = $true; placement = [ordered]@{ monitor = 'primary'; col = $g[0]; row = $g[1]; w = $g[2]; h = $g[3] } }
    }
    $s = [ordered]@{ version = $Version; data_fetch = 'off'; font_scale = $Plan.FontScale; widgets = $widgets }
    return (ConvertTo-Json $s -Depth 6)
}

# 從 Rust 原始碼讀 SETTINGS_VERSION 與 DEFAULT_GRID_RECTS 核對（不一致就不跑，避免寫出被宿主重設的設定檔）。
$settingsRs = Get-Content (Join-Path $PSScriptRoot '..\src\settings.rs') -Raw
$verMatch = [regex]::Match($settingsRs, 'pub const SETTINGS_VERSION: u32 = (\d+);')
if (-not $verMatch.Success) { throw 'settings.rs 找不到 SETTINGS_VERSION' }
$SettingsVersion = [int]$verMatch.Groups[1].Value
$rectsBlock = [regex]::Match($settingsRs, 'pub const DEFAULT_GRID_RECTS: \[GridRect; \d+\] = \[([\s\S]*?)\n\];').Groups[1].Value
$rustRects = @([regex]::Matches($rectsBlock, 'grid\((\d+),\s*(\d+),\s*(\d+),\s*(\d+)\)') | ForEach-Object { ($_.Groups[1..4] | ForEach-Object { $_.Value }) -join ',' })
$mine = @($FinanceIds | ForEach-Object { $DefaultRects[$_] -join ',' })
if ((($rustRects[0..4]) -join ';') -ne ($mine -join ';')) { throw "本腳本的預設格座標與 settings.rs 不一致：Rust=$($rustRects[0..4] -join ';') 本腳本=$($mine -join ';')" }

# pwsh -File 模式下 `-Scenarios A,B` 會變成單一字串 "A,B"：一律再以逗號切開。
$Scenarios = @($Scenarios | ForEach-Object { $_ -split ',' } | ForEach-Object { $_.Trim() } | Where-Object { $_ })
$plans = @($Scenarios | ForEach-Object { New-ScenarioPlan $_ })

if ($DryRun) {
    Write-Host "主螢幕工作區 $(Fmt $pwa) 縮放 $pScale；SETTINGS_VERSION=$SettingsVersion"
    $tmp = Join-Path ([IO.Path]::GetTempPath()) ('fc-host-zoomdry-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
    New-Item -ItemType Directory -Force -Path $tmp | Out-Null
    try {
        foreach ($p in $plans) {
            $items = @(foreach ($id in $FinanceIds) { $gr = Grid-Rect $pwa $p.Rects[$id]; [PSCustomObject]@{ id = $id; physW = $gr.W; physH = $gr.H; scale = $pScale; fontScale = $p.FontScale } })
            $exp = Get-ExpectedZooms $items $tmp
            Write-Host "── 情境 $($p.Name)（$($p.Note)；font_scale=$($p.FontScale)）問題：$(if ($p.Problems.Count) { $p.Problems -join '; ' } else { '無' })"
            foreach ($it in $items) {
                $e = $exp[$it.id]
                Write-Host ("  {0,-8} 格={1,-12} 實體={2}x{3} 邏輯={4:N1}x{5:N1} 倍率={6:N4}（字級 1 時 {7:N4}、上限 {8:N4}）" -f $it.id, ($p.Rects[$it.id] -join ','), $it.physW, $it.physH, ($it.physW / $pScale), ($it.physH / $pScale), $e.zoom, $e.zoomAt1, $e.cap)
            }
            Write-Host (ConvertTo-SettingsJson $p $SettingsVersion)
        }
    } finally { Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue }
    exit 0
}

# ── 前置（任何 try／finally 之前；不結束任何既有行程）───────────────────────────────
if (Get-Process -Name fc-host -ErrorAction SilentlyContinue) {
    Write-Host 'BLOCKED: 已有 fc-host 在執行（單一執行個體，隔離的宿主會被交接給它），本腳本不結束它；請先自行從系統匣結束再重跑。'
    exit 2
}
if (Test-Locked) {
    Write-Host 'BLOCKED: 工作階段已鎖定（LogonUI.exe 在執行），截圖會全黑；解鎖後重跑。'
    exit 2
}
$Exe = (Resolve-Path $Exe).Path
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$logPath = Join-Path $OutDir 'adaptive-zoom-driver.log'
$sumPath = Join-Path $OutDir 'adaptive-zoom-summary.log'
$log = New-EvidenceWriter $logPath
$log.AutoFlush = $true
function Log([string]$m) { $line = "$(Get-Ts) $m"; $log.WriteLine($line); Write-Host (ConvertTo-EvidenceText $line) }
Log "# verify-adaptive-zoom.ps1 exe=$Exe cdpPort=$CdpPort 情境=$($Scenarios -join ',')"
Log "# 主螢幕工作區 $(Fmt $pwa) 縮放 $pScale；SETTINGS_VERSION=$SettingsVersion"

$realSettings = Join-Path $env:APPDATA 'tw.fintools.fc-host\settings.json'
function Get-RealSettingsStamp { $i = Get-Item -LiteralPath $realSettings -ErrorAction SilentlyContinue; if ($i) { "$($i.Length)@$($i.LastWriteTimeUtc.Ticks)" } else { '<不存在>' } }
$realStampBefore = Get-RealSettingsStamp

$results = [ordered]@{}
$notRun = New-Object System.Collections.Generic.List[string]
$zoomTable = New-Object System.Collections.Generic.List[object]
$script:blocked = $null
# 目前情境由本腳本啟動、尚未收尾的宿主（頂層 finally 用：Ctrl+C 或情境外的例外時也要停掉它）。
$script:currentHost = $null
function Set-Result([string]$Key, $Value) { $results[$Key] = $Value; Log "  => $(if ($Value -is [string]) { $Value } elseif ($Value) { 'PASS' } else { 'FAIL' })  $Key" }
function Assert-Unlocked([string]$Where) { if (Test-Locked) { throw "LOCKED：$Where 時偵測到工作階段鎖定" } }

# 頁面量測運算式（單引號 JS、無 $）：dpr、viewport、各元素的捲動／用戶區尺寸與外框。
function Get-MeasureExpr([string]$Id) {
    $sels = @($ClipSpec[$Id] | ForEach-Object { "'" + $_[0] + "'" }) -join ','
    return "(() => { const vw = window.innerWidth, vh = window.innerHeight; const r2 = (v) => Math.round(v * 100) / 100; const els = [$sels].map((s) => { const el = document.querySelector(s); if (!el) return { sel: s, missing: true }; const b = el.getBoundingClientRect(); return { sel: s, sw: el.scrollWidth, cw: el.clientWidth, sh: el.scrollHeight, ch: el.clientHeight, l: r2(b.left), t: r2(b.top), r: r2(b.right), b: r2(b.bottom) }; }); return { dpr: window.devicePixelRatio, vw: vw, vh: vh, els: els }; })()"
}
$TickerExpr = "(() => { const u = document.querySelector('.tlist'); return u ? { sl: u.scrollLeft, sw: u.scrollWidth, cw: u.clientWidth } : null; })()"

# 頂層 try／finally（Codex review 6.x）：從開機自啟登錄快照之後起包住所有情境；Ctrl+C、情境外的例外也會
# 先停掉本腳本啟動的宿主、再無條件還原登錄並關閉記錄檔。
$regSnap = @(Save-FcHostAutostartRegistry)
Log "# 真正的設定檔（只看大小與修改時間，不讀內容）開始=$realStampBefore；開機自啟登錄快照：$(($regSnap | ForEach-Object { "$($_.Name)@$(Split-Path $_.Key -Leaf) Exists=$($_.Exists)" }) -join '; ')"
try {
    foreach ($plan in $plans) {
        $S = $plan.Name
        Log "## ── 情境 $S：$($plan.Note)；font_scale=$($plan.FontScale)；格座標 $(($FinanceIds | ForEach-Object { "$_=($($plan.Rects[$_] -join ','))" }) -join ' ')"
        if ($plan.Problems.Count -gt 0) {
            $notRun.Add("情境 $S：本機換算出的格座標不合法（$($plan.Problems -join '; ')）")
            Log "  格座標不合法，略過：$($plan.Problems -join '; ')"
            continue
        }
        # 先清掉本情境上次留下的同名證據（截圖、宿主記錄），避免沒產生新檔時舊檔冒充本次結果。
        Get-ChildItem -LiteralPath $OutDir -File -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -like "adaptive-zoom-$S-*.png" -or $_.Name -eq "adaptive-zoom-$S-hostlog.log" } |
            Remove-Item -Force -ErrorAction Stop
        $tempRoot = Join-Path ([IO.Path]::GetTempPath()) ("fc-host-zoom63-$S-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
        $tempAppData = Join-Path $tempRoot 'Roaming'
        $tempLocal = Join-Path $tempRoot 'Local'
        $dataDir = Join-Path $tempLocal 'tw.fintools.fc-host\data'
        $cfgDir = Join-Path $tempAppData 'tw.fintools.fc-host'
        New-Item -ItemType Directory -Force -Path $cfgDir, $dataDir | Out-Null
        Copy-Item (Join-Path $PSScriptRoot '..\ui\fixtures\tw-events.json') (Join-Path $dataDir 'tw_events.json')
        $json = ConvertTo-SettingsJson $plan $SettingsVersion
        [IO.File]::WriteAllText((Join-Path $cfgDir 'settings.json'), $json, (New-Object System.Text.UTF8Encoding($false)))
        Log "  settings.json：$(($json -replace '\s+', ' '))"
        $hostProc = $null
        try {
            Assert-Unlocked "情境 $S 啟動前"
            $old = @($env:APPDATA, $env:LOCALAPPDATA, $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS)
            try {
                $env:APPDATA = $tempAppData
                $env:LOCALAPPDATA = $tempLocal
                $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$CdpPort"
                $hostProc = Start-Process -FilePath $Exe -PassThru
                $script:currentHost = $hostProc
            } finally {
                $env:APPDATA, $env:LOCALAPPDATA, $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $old
            }
            $hostPid = $hostProc.Id
            Log "  宿主 pid=$hostPid"
            $hwnds = @{}
            foreach ($id in $FinanceIds) { $hwnds[$id] = Wait-Window $hostPid "fc-host $id" 60 }
            $missing = @($FinanceIds | Where-Object { $hwnds[$_] -eq [IntPtr]::Zero })
            Set-Result "$S：五個財經小工具視窗都出現" ($missing.Count -eq 0)
            if ($missing.Count -gt 0) { Log "  未出現：$($missing -join ', ')" }
            Start-Sleep -Seconds 4   # 倍率套用、頁面版面穩定

            $st = Invoke-Json 'w=clock' "window.__TAURI__.core.invoke('get_settings').then(s => ({ font_scale: s.font_scale, widgets: Object.fromEntries(['clock','macro','fixed','dynamic','quotes'].map(k => [k, s.widgets[k].placement])) }))"
            $fsOk = ($null -ne $st.font_scale -and [math]::Abs([double]$st.font_scale - $plan.FontScale) -lt 1e-9)
            $plOk = $true
            foreach ($id in $FinanceIds) {
                $pl = $st.widgets.$id
                $g = $plan.Rects[$id]
                if (-not $pl -or $pl.col -ne $g[0] -or $pl.row -ne $g[1] -or $pl.w -ne $g[2] -or $pl.h -ne $g[3]) { $plOk = $false }
            }
            Log "  get_settings：font_scale=$($st.font_scale) placement=$(($FinanceIds | ForEach-Object { $p = $st.widgets.$_; "$_=($($p.col),$($p.row),$($p.w),$($p.h))" }) -join ' ')$(if ($st.__error) { " 錯誤=$($st.__error)" })"
            Set-Result "$S：設定讀回 font_scale＝$($plan.FontScale)、格座標＝寫入值（設定檔沒有被重設）" ($fsOk -and $plOk)

            # 量測視窗矩形與縮放比例，計算預期倍率。
            $meas = [ordered]@{}
            foreach ($id in $FinanceIds) {
                if ($hwnds[$id] -eq [IntPtr]::Zero) { continue }
                $r = Get-Rect $hwnds[$id]
                $scale = [double][VZoomA.Native]::GetDpiForWindow($hwnds[$id]) / 96.0
                $meas[$id] = [PSCustomObject]@{ Rect = $r; Scale = $scale }
            }
            $exp = Get-ExpectedZooms @($meas.Keys | ForEach-Object { [PSCustomObject]@{ id = $_; physW = $meas[$_].Rect.W; physH = $meas[$_].Rect.H; scale = $meas[$_].Scale; fontScale = $plan.FontScale } }) $tempRoot

            $rectAll = $true; $zoomAll = $true; $clipAll = $true
            foreach ($id in $meas.Keys) {
                $m = $meas[$id]; $e = $exp[$id]
                $want = Grid-Rect $pwa $plan.Rects[$id]
                $rectOk = Same $m.Rect $want
                if (-not $rectOk) { $rectAll = $false }
                $pg = Invoke-Json "w=$id" (Get-MeasureExpr $id)
                $actual = if ($pg.dpr) { [double]$pg.dpr / $m.Scale } else { [double]::NaN }
                $zoomOk = [math]::Abs($actual - [double]$e.zoom) -le 0.01
                if (-not $zoomOk) { $zoomAll = $false }
                $lw = $m.Rect.W / $m.Scale; $lh = $m.Rect.H / $m.Scale
                $clipNotes = @()
                $panelEl = @(@($pg.els) | Where-Object { $_.sel -eq '.panel' -and -not $_.missing })
                foreach ($el in @($pg.els)) {
                    $kind = (@($ClipSpec[$id] | Where-Object { $_[0] -eq $el.sel }))[0][1]
                    if ($el.missing) { $clipNotes += "$($el.sel) 不存在"; continue }
                    if ($kind -eq 'text') {
                        if ($panelEl.Count -ne 1) { $clipNotes += "$($el.sel) 找不到 .panel 可比對"; continue }
                        $pn = $panelEl[0]
                        $inkBottom = [double]$el.b + [math]::Max(0.0, [double]($el.sh - $el.ch))
                        $wOk = ($el.sw -le $el.cw) -and ($el.l -ge [double]$pn.l - 0.5) -and ($el.r -le [double]$pn.r + 0.5)
                        $hOk = ($el.t -ge [double]$pn.t - 0.5) -and ($inkBottom -le [math]::Min([double]$pn.b, [double]$pg.vh) + 0.5)
                    } else {
                        $wOk = ($el.sw -le $el.cw) -and ($el.l -ge -0.5) -and ($el.r -le [double]$pg.vw + 0.5)
                        $hOk = ($el.sh -le $el.ch) -and ($el.t -ge -0.5) -and ($el.b -le [double]$pg.vh + 0.5)
                    }
                    $bad = switch ($kind) { 'whole' { -not ($wOk -and $hOk) } 'text' { -not ($wOk -and $hOk) } 'height' { -not $hOk } 'width' { -not $wOk } }
                    if ($bad) { $clipNotes += "$($el.sel) 溢出（scroll $($el.sw)x$($el.sh) > client $($el.cw)x$($el.ch) 或外框 [$($el.l),$($el.t),$($el.r),$($el.b)] 超出 viewport $($pg.vw)x$($pg.vh)）" }
                }
                if ($pg.__error) { $clipNotes += "量測失敗：$($pg.__error)" }
                if ($clipNotes.Count -gt 0) { $clipAll = $false }
                Log ("  {0,-8} rect={1} 期望格線矩形={2} 相符={3} 縮放={4} 邏輯={5:N1}x{6:N1} 預期倍率={7:N4}（字級 1 時 {8:N4}、上限 {9:N4}） 頁面 dpr={10} → 實際倍率={11:N4} 相符={12} viewport={13}x{14} 裁切={15}" -f $id, (Fmt $m.Rect), (Fmt $want), $rectOk, $m.Scale, $lw, $lh, [double]$e.zoom, [double]$e.zoomAt1, [double]$e.cap, $pg.dpr, $actual, $zoomOk, $pg.vw, $pg.vh, $(if ($clipNotes.Count) { $clipNotes -join '; ' } else { '無' }))
                foreach ($el in @($pg.els)) { if (-not $el.missing) { Log "      $($el.sel)：scroll $($el.sw)x$($el.sh) client $($el.cw)x$($el.ch) 外框 [$($el.l),$($el.t),$($el.r),$($el.b)]" } }
                $zoomTable.Add([PSCustomObject]@{ Scenario = $S; Id = $id; Rect = (Fmt $m.Rect); Logical = ('{0:N1}x{1:N1}' -f $lw, $lh); Expected = [double]$e.zoom; Actual = $actual; Cap = [double]$e.cap; ZoomAt1 = [double]$e.zoomAt1; Ok = $zoomOk })
                $meas[$id] | Add-Member -NotePropertyName Expected -NotePropertyValue ([double]$e.zoom)
                $meas[$id] | Add-Member -NotePropertyName Actual -NotePropertyValue $actual
                $meas[$id] | Add-Member -NotePropertyName Cap -NotePropertyValue ([double]$e.cap)
                $meas[$id] | Add-Member -NotePropertyName ZoomAt1 -NotePropertyValue ([double]$e.zoomAt1)
                $meas[$id] | Add-Member -NotePropertyName Lw -NotePropertyValue $lw
                $meas[$id] | Add-Member -NotePropertyName Lh -NotePropertyValue $lh
            }
            Set-Result "$S：五個小工具的視窗矩形＝設定格座標的格線矩形" ($rectAll -and $meas.Count -eq 5)
            Set-Result "$S：五個小工具的實際倍率（dpr ÷ 縮放）＝content_zoom 公式（實際矩形、縮放、font_scale），容差 0.01" ($zoomAll -and $meas.Count -eq 5)
            Set-Result "$S：無裁切（時鐘文字與面板、行情條面板高度、清單面板寬度）" ($clipAll -and $meas.Count -eq 5)

            # 情境意圖的獨立交叉核對。
            switch ($S) {
                'A' {
                    $c = $meas['clock']
                    $pre = $c -and $c.Lw -ge 424 -and $c.Lh -ge 320
                    $simple = if ($c) { [math]::Min(3.0, [math]::Min($c.Lw / 212.0, $c.Lh / 160.0)) } else { [double]::NaN }
                    Log ("  A 交叉核對：時鐘邏輯 {0:N1}x{1:N1}（≥ 424x320＝{2}）；min(寬/212, 高/160, 3)={3:N4} 實際 {4:N4}" -f $c.Lw, $c.Lh, $pre, $simple, $c.Actual)
                    Set-Result 'A：時鐘框寬高都 ≥ 最小框兩倍（前提）' ([bool]$pre)
                    Set-Result 'A：時鐘倍率＝min(邏輯寬/212, 邏輯高/160) 夾 3（獨立簡式，容差 0.01）' ($c -and [math]::Abs($c.Actual - $simple) -le 0.01)
                }
                'B' {
                    $q = $meas['quotes']
                    $defW = (Grid-Rect $pwa $DefaultRects['quotes']).W
                    $simple = if ($q) { [math]::Min(3.0, $q.Lh / 60.0) } else { [double]::NaN }
                    Log ("  B 交叉核對：行情條實體寬 {0}（預設 {1}）邏輯高 {2:N1}；min(高/60, 3)={3:N4} 實際 {4:N4}" -f $q.Rect.W, $defW, $q.Lh, $simple, $q.Actual)
                    Set-Result 'B：行情條寬度與預設相同、高度比預設高（前提）' ($q -and $q.Rect.W -eq $defW -and $q.Rect.H -gt (Grid-Rect $pwa $DefaultRects['quotes']).H)
                    Set-Result 'B：行情條倍率＝min(邏輯高/60, 3)（寬度不限制；獨立簡式，容差 0.01）' ($q -and [math]::Abs($q.Actual - $simple) -le 0.01)
                    if ($hwnds['quotes'] -ne [IntPtr]::Zero) {
                        $t0 = Invoke-Json 'w=quotes' $TickerExpr
                        Start-Sleep -Milliseconds 1500
                        $t1 = Invoke-Json 'w=quotes' $TickerExpr
                        $overflow = $t0 -and $t0.sw -gt $t0.cw
                        $moving = $t0 -and $t1 -and $t0.sl -ne $t1.sl
                        Log "  B 跑馬燈：.tlist scrollWidth=$($t0.sw) clientWidth=$($t0.cw)（需要捲動=$overflow）scrollLeft $($t0.sl) → 1.5 秒後 $($t1.sl)（在動=$moving）"
                        if ($overflow) { Set-Result 'B：加高後跑馬燈仍在動（.tlist scrollLeft 1.5 秒內有變化）' ([bool]$moving) }
                        else { Set-Result 'B：加高後跑馬燈仍在動（.tlist scrollLeft 1.5 秒內有變化）' 'SKIPPED'; $notRun.Add('B：行情內容沒有超出寬度，不需要跑馬燈，無從判定是否在動') }
                    }
                }
                'C' {
                    $mc = $meas['macro']
                    $wAuto = $mc.Lw / 500.0; $hAuto = $mc.Lh / 324.0
                    Log ("  C 交叉核對：總經日曆邏輯 {0:N1}x{1:N1}；寬/500={2:N4} 高/324={3:N4}；實際 {4:N4}" -f $mc.Lw, $mc.Lh, $wAuto, $hAuto, $mc.Actual)
                    Set-Result 'C：總經日曆邏輯寬約 2×500（≥ 900）、高約 324（±30）（前提）' ($mc -and $mc.Lw -ge 900 -and [math]::Abs($mc.Lh - 324.0) -le 30)
                    Set-Result 'C：倍率約 1（0.9–1.1）且由高度決定（高/324 < 寬/500），不是 2' ($mc -and $mc.Actual -ge 0.9 -and $mc.Actual -le 1.1 -and $hAuto -lt $wAuto)
                }
                'D150' {
                    $okCap = $true
                    foreach ($id in 'clock', 'quotes') {
                        $m = $meas[$id]
                        Log ("  D150 {0}：實際 {1:N4} 上限 {2:N4}（字級 1 時 {3:N4}）" -f $id, $m.Actual, $m.Cap, $m.ZoomAt1)
                        if (-not $m -or $m.Actual -gt $m.Cap + 0.01) { $okCap = $false }
                    }
                    Set-Result 'D150：時鐘、行情條在字級 150% 時不超過上限' $okCap
                }
            }
            if ($S -like 'D*') {
                $okList = $true
                foreach ($id in 'macro', 'fixed', 'dynamic') {
                    $m = $meas[$id]
                    $simple = [math]::Min($m.ZoomAt1 * $plan.FontScale, $m.Cap)
                    Log ("  $S {0}：字級 1 時 {1:N4} × {2} = {3:N4}，上限 {4:N4} → min={5:N4}；實際 {6:N4}" -f $id, $m.ZoomAt1, $plan.FontScale, ($m.ZoomAt1 * $plan.FontScale), $m.Cap, $simple, $m.Actual)
                    if (-not $m -or [math]::Abs($m.Actual - [math]::Max(0.5, $simple)) -gt 0.01) { $okList = $false }
                }
                Set-Result "$($S)：清單倍率＝min(自適應 × 字級, 上限)（交叉核對）" $okList
            }

            # 截圖：只截小工具自己的視窗；每張都是具名結果（失敗或不是本次新檔＝FAIL）。
            foreach ($id in $Shots[$S]) {
                $name = "adaptive-zoom-$S-$id.png"
                Assert-Unlocked "情境 $S 截圖前"
                $shot = if ($hwnds[$id] -eq [IntPtr]::Zero) { [PSCustomObject]@{ Ok = $false; Reason = '視窗未出現' } }
                else { Save-WindowShot $hwnds[$id] (Join-Path $OutDir $name) }
                Log "  截圖 $name：$(if ($shot.Ok) { 'OK' } else { "失敗（$($shot.Reason)）" })"
                Set-Result "$($S)：截圖 $name 為本次 PrintWindow 新檔" $shot.Ok
            }
        }
        catch {
            if ("$($_.Exception.Message)" -like 'LOCKED*') {
                $script:blocked = $_.Exception.Message
                Log "## BLOCKED：$($script:blocked)（結果不完整）"
            } else {
                Log "## 情境 $S 例外中止：$($_.Exception.Message)（第 $($_.InvocationInfo.ScriptLineNumber) 行：$($_.InvocationInfo.Line.Trim())）"
                $results["$($S)：腳本跑完（無例外）"] = $false
            }
        }
        finally {
            try { if ($hostProc) { [void](Stop-ProcessTree -Process $hostProc) }; $script:currentHost = $null } catch { Log "## 收尾結束宿主失敗：$_" }
            Start-Sleep -Seconds 2
            try {
                $hostLogs = @(Get-ChildItem (Join-Path $tempLocal 'tw.fintools.fc-host\logs') -Filter *.log -ErrorAction SilentlyContinue | Sort-Object LastWriteTime)
                if ($hostLogs.Count -gt 0) { Copy-EvidenceFile -Source $hostLogs[-1].FullName -Destination (Join-Path $OutDir "adaptive-zoom-$S-hostlog.log") }
            } catch { Log "## 複製宿主記錄失敗：$_" }
            Remove-Item -Recurse -Force $tempRoot -ErrorAction SilentlyContinue
        }
        if ($script:blocked) { break }
    }
}
catch {
    Log "## 情境迴圈外的例外中止：$($_.Exception.Message)（第 $($_.InvocationInfo.ScriptLineNumber) 行：$($_.InvocationInfo.Line.Trim())）"
    $results['腳本跑完（無例外）'] = $false
}
finally {
    # 每一項收尾各自 try：先停仍存活、由本腳本啟動的宿主，再還原登錄，最後關記錄檔。
    try { if ($script:currentHost) { [void](Stop-ProcessTree -Process $script:currentHost); $script:currentHost = $null; Start-Sleep -Seconds 2 } } catch { Log "## 收尾結束宿主失敗：$_" }
    try {
        $regLeft = @(Restore-FcHostAutostartRegistry $regSnap)
        if ($regLeft.Count -gt 0) { Write-Warning "開機自啟登錄未還原：$($regLeft -join '; ')" }
        Log "# 開機自啟登錄還原：未還原 $($regLeft.Count) 項 $($regLeft -join '; ')"
        $results['開機自啟登錄已還原（Run／StartupApproved 的 fc-host）'] = ($regLeft.Count -eq 0)
    } catch {
        Log "## 開機自啟登錄還原失敗：$_"
        $results['開機自啟登錄已還原（Run／StartupApproved 的 fc-host）'] = $false
    }
    try {
        $realStampAfter = Get-RealSettingsStamp
        Log "# 真正的設定檔（大小@修改時間）結束=$realStampAfter；與開始相同=$($realStampAfter -eq $realStampBefore)"
        $results['隔離：真正的設定檔大小與修改時間前後相同'] = ($realStampAfter -eq $realStampBefore)
        Log '# 倍率表（情境｜小工具｜實際矩形｜邏輯尺寸｜預期｜實際｜上限｜字級 1 時｜相符）'
        foreach ($z in $zoomTable) { Log ("#   {0}|{1}|{2}|{3}|{4:N4}|{5:N4}|{6:N4}|{7:N4}|{8}" -f $z.Scenario, $z.Id, $z.Rect, $z.Logical, $z.Expected, $z.Actual, $z.Cap, $z.ZoomAt1, $z.Ok) }
        Log '# 結束'
    } catch { Write-Warning "收尾記錄失敗：$_" }
    $log.Close()
}

$sum = New-EvidenceWriter $sumPath
$sum.WriteLine("# verify-adaptive-zoom.ps1 summary $(Get-Ts)")
foreach ($k in $results.Keys) {
    $v = $results[$k]
    $tag = if ($v -is [string]) { 'NOT-RUN' } elseif ($v) { 'PASS' } else { 'FAIL' }
    $sum.WriteLine("$tag  $k")
}
foreach ($n in $notRun) { $sum.WriteLine("NOT-RUN  $n") }
if ($script:blocked) { $sum.WriteLine("BLOCKED  $($script:blocked)") }
$sum.Close()
Get-Content $sumPath
if ($script:blocked) { exit (Get-VerdictExitCode -Results $results -Locked) }
exit (Get-VerdictExitCode -Results $results -EnvBlocked:($notRun.Count -gt 0) -RequireResults)
