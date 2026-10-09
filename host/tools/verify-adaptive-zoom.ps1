<#
.SYNOPSIS
    自適應倍率與各小工具字級的實機驗收驅動腳本（widget-adaptive-zoom-and-grid task 6.3 建立；
    widget-font-scale-per-widget task 5.2 改為各小工具字級。design.md D1–D5，specs/widget-host-windows
    「小工具尺寸由版面格決定」「窄框的清單可以放大」、specs/widget-host-lifecycle「設定持久化」）。

.DESCRIPTION
    每個情境在全新的隔離資料夾、**啟動前**寫入 settings.json（`version`＝settings.rs 的
    `SETTINGS_VERSION`，只寫五個財經小工具的開關、格座標與字級，`data_fetch`=off，其餘由宿主補預設），
    各啟動一次宿主。字級寫在各小工具底下（`widgets.<id>.font_scale`）；只有 F 刻意寫 v0.2.0 格式
    （頂層 `font_scale`、小工具底下沒有字級）驗遷移。

    | 情境 | 設定 | 預期 |
    |---|---|---|
    | A 時鐘大框 | 時鐘移到左側空白欄，寬、高都 ≥ 最小框（212×160 邏輯）兩倍（目標約 2.5 倍） | 倍率＝min(邏輯寬/212, 邏輯高/160) 夾 3；文字不裁切 |
    | B 行情條加高 | 行情條寬維持預設 32 格、高改成約 2.75 倍 min 高的格數（總經、動態事件高度讓位） | 倍率＝min(邏輯高/60, 3)；跑馬燈仍在動 |
    | C 清單又寬又矮 | 總經日曆邏輯寬約 2×500、邏輯高約 324（移到左側） | 倍率約 1（由高度決定），不是 2 |
    | D 各自字級 | 預設版面，時鐘 1.5、總經 2.0、固定 0.7、動態 1.0、行情條 1.3 | 每個小工具倍率＝clamp(min(自適應×自己的字級, 上限), 0.5, 3)；時鐘、行情條＝上限 |
    | E 窄框放大 | 總經日曆邏輯寬約 0.64×500（無條件進位到格）、高約 600（高度不限制）、字級 2.0 | 倍率 ≥ 1.2 且＝公式；收合版面生效、列內無重疊無裁切 |
    | F v0.2.0 遷移 | 頂層 `font_scale` 1.2、各小工具沒有字級 | 各小工具字級 1.2（含未寫出的擴充插槽）、倍率依 1.2；存檔後不再有頂層鍵 |
    | G 字級指令 | 預設版面、全部 1.0 | 不在編輯版面時 `adjust_widget_font_scale` 被拒；編輯版面中只改呼叫端自己；`widget-font` 事件只被自己的頁面採用 |

    格數依主螢幕當下的工作區與縮放比例換算（目標邏輯尺寸 × 縮放 × 48 ÷ 工作區實體長度），所以換一台
    機器也照同樣的意圖擺放；擺放結果若相交或超出 48 格就該情境 NOT-RUN。

    每個情境、每個財經小工具：
      - 預期倍率＝與 Rust `layout::content_zoom_detail` 相同的公式，輸入是**實際**視窗矩形（GetWindowRect）、
        該視窗的縮放比例（GetDpiForWindow ÷ 96）與**該小工具自己的**字級；倍率框（`ZoomBox`）與公式沿用
        host/tests/compare/verify-visual-edges.mjs 匯出的 `widgetZoomBoxes()`／`contentZoom()`（從
        host/src/widgets.rs 的 `WIDGET_SPECS` 讀出，不寫死）。`at_cap` 預期＝字級已達 3.0，或字級 +0.1 後
        倍率不再變大（同 Rust）。情境本身的意圖另以獨立的簡式交叉核對（A：min(寬/212, 高/160, 3)；
        B：min(高/60, 3)；C：0.9–1.1 且高度比寬度先到；D：clamp(min(自適應×字級, 上限))；E：≥ 1.2 且由寬度決定）。
      - 實際倍率＝頁面 `window.devicePixelRatio ÷ 縮放比例`（WebView2 ZoomFactor 等比反映在 devicePixelRatio，
        verify-zoom-reload.ps1 已實測），容差 0.01。
      - 字級讀回：`get_settings` 各小工具 `font_scale`＝預期、沒有頂層 `font_scale`、格座標＝寫入值；
        各小工具頁 `get_widget_font_state` 的 `font_scale`＝預期、`at_cap`＝公式。
      - 無裁切：時鐘 `.panel` 的 scrollWidth ≤ clientWidth、scrollHeight ≤ clientHeight 且外框在 viewport 內；
        `#clockTime`／`#clockDate`／`#weekRange` 的 scrollWidth ≤ clientWidth、外框在 `.panel` 內，縱向溢出的部分
        （scrollHeight − clientHeight，#clockTime 的字型內容區本來就比行高多約 9 CSS px）仍在 `.panel` 與
        viewport 內（見 `$ClipSpec` 註解）；行情條 `.panel` 高度未溢出（scrollHeight ≤
        clientHeight、下緣在 viewport 內；`.tlist` 是橫向跑馬燈，寬度不算）；清單 `.panel` 寬度未溢出（縱向是
        原生捲動，不算裁切）。
      - 清單列（總經、固定、動態的每個 `.ev`）：子元素外框兩兩不相交、子元素與列本身 scrollWidth ≤ clientWidth、
        子元素外框在列內、列在 viewport 內（`Get-RowLayoutProblems`）。E 另要求列數 > 0、`.ev` 為 flex-wrap: wrap
        （收合版面生效）。
      - 視窗矩形＝設定格座標在主螢幕工作區的格線矩形（design.md D7 `edge`），確認宿主照設定擺放。
      - 截圖：情境主角（A 時鐘、B 行情條、C／E 總經日曆、D 全部五個；G 是編輯版面中的全部五個，存
        `adaptive-zoom-G-<id>-edit.png`）以 `PrintWindow` 只截小工具視窗本身，存 `evidence/adaptive-zoom-<情境>-<id>.png`。
    F 另在宿主執行中以 `set_edit_mode(true)` → `(false)` 觸發存檔（兩次都會存 `layout_locked`），讀回隔離設定檔：
    兩次都必須回 ok、檔案必須被改寫（修改時間與內容都要變；沒存檔＝FAIL），且不得再有頂層 `font_scale`、十個
    `WIDGET_IDS`（含 F 刻意沒寫出的 custom1–custom5）都在 `widgets` 裡、字級都是 1.2。F 寫入的設定檔只列五個財經
    小工具，用來實證「未列出的小工具也遷移」；get_settings 讀回同樣要求十個 id 都在且為 1.2。
    G 的順序：不在編輯版面時呼叫 → 被拒、字級不變；時鐘頁掛 `widget-font` 記錄器；`set_edit_mode(true)` → 五頁都有
    字級控制、顯示 100%；總經頁 `adjust_widget_font_scale({step:1})` → 回 1.1；五頁 `get_widget_font_state` 只有總經
    變 1.1；頁面百分比只有總經變 110%（CDP 呼叫的回傳值不經頁面，頁面只能靠 `widget-font` 事件更新）；時鐘頁
    收到 id=macro 的事件但自己仍顯示 100%；`get_settings` 只有總經 1.1；總經倍率依 1.1 重算；截圖；
    `set_edit_mode(false)` → 控制移除、再呼叫又被拒。
    結果彙整到 `evidence/adaptive-zoom-summary.log`，過程記錄 `adaptive-zoom-driver.log`。

    不注入任何鍵盤／滑鼠輸入。工作階段鎖定時（LogonUI.exe 在跑）BLOCKED，途中鎖定也停止。隔離、開機自啟
    登錄快照還原、收尾只停自己啟動的宿主、證據去識別同 verify-grid-overlay.ps1。

    純函式（`New-ScenarioPlan`、`ConvertTo-SettingsJson`、`Get-ExpectedZooms`、`Get-*Problems`、
    `Test-Rejected`、`Get-MigratedFileVerdict` 等）的測試在 host/tools/tests/VerifyAdaptiveZoom.Tests.ps1。

    結束碼：0＝全部通過；1＝有 FAIL；2＝BLOCKED；3＝有 NOT-RUN。

.PARAMETER Scenarios
    要跑的情境（預設 A–G 全部）。

.PARAMETER DryRun
    只印出依本機主螢幕換算的各情境格座標、字級、預期倍率與 at_cap（以格線矩形代替實際視窗矩形），
    不啟動宿主。
#>
[CmdletBinding()]
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\fc-host.exe'),
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [int]$CdpPort = 9364,
    [string[]]$Scenarios = @('A', 'B', 'C', 'D', 'E', 'F', 'G'),
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'

Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\AutostartRegistry.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\ProcessTree.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\VerifyVerdict.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\NodeProcess.psm1') -Force

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
$ListIds = @('macro', 'fixed', 'dynamic')
# 全部小工具 id（host/src/settings.rs `WIDGET_IDS`，下方會與 Rust 原始碼核對）：F 的遷移判讀要求每一個都在、都是遷移值。
$AllWidgetIds = @('clock', 'macro', 'fixed', 'dynamic', 'quotes', 'custom1', 'custom2', 'custom3', 'custom4', 'custom5')
# 預設格座標（host/src/settings.rs `DEFAULT_GRID_RECTS`，順序同上）；改了那邊這裡要跟著改（下方會與 Rust 原始碼核對）。
$DefaultRects = [ordered]@{
    clock = @(15, 1, 16, 10); macro = @(15, 12, 16, 30); fixed = @(32, 1, 15, 17); dynamic = @(32, 19, 15, 23); quotes = @(15, 43, 32, 4)
}
# D 情境各小工具的字級（刻意各不相同；總經 2.0 在預設框會進收合版面，時鐘、行情條 >1 會碰到上限）。
$ScenarioDFonts = [ordered]@{ clock = 1.5; macro = 2.0; fixed = 0.7; dynamic = 1.0; quotes = 1.3 }
# 情境主角（截圖對象）。G 的截圖在編輯版面中另外拍（`adaptive-zoom-G-<id>-edit.png`）。
$Shots = @{ A = @('clock'); B = @('quotes'); C = @('macro'); D = $FinanceIds; E = @('macro'); F = @('macro'); G = @() }
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
# 兩個字級是否相同（設定值都是 0.1 的倍數，序列化後的 double 應完全相同；容差只防序列化差一個 ulp）。
function Test-SameFont($A, $B) {
    if ($null -eq $A -or $null -eq $B) { return $false }
    try { return [math]::Abs([double]$A - [double]$B) -lt 1e-9 } catch { return $false }
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
    # node 的輸出一律以 UTF-8 解碼（lib/NodeProcess.psm1）：以呼叫運算子直接執行 node 會用呼叫端主控台碼頁（背景 shell＝950）解碼，
    # 宿主回的中文錯誤訊息被解成亂碼、連引號一起吃掉，JSON 讀不到 `ok`（task 5.2 實跑）。
    $r = Invoke-NodeUtf8 -ArgumentList @((Join-Path $PSScriptRoot 'host-cdp-eval.mjs'), "$CdpPort", $Page, $Expr)
    if ($r.ExitCode -ne 0) { return [PSCustomObject]@{ __error = (@($r.StdOut, $r.StdErr) | Where-Object { $_ }) -join "`n" } }
    try { return ($r.StdOut | ConvertFrom-Json) } catch { return [PSCustomObject]@{ __error = "$($r.StdOut)`n$($r.StdErr)".Trim() } }
}
# 在頁面呼叫宿主指令，成功與拒絕都包成 { ok, r | err }（拒絕不是 CDP 錯誤，要能判讀）。
function Get-InvokeExpr([string]$Cmd, [string]$ArgsJs = '') {
    $a = if ($ArgsJs) { ", $ArgsJs" } else { '' }
    return "window.__TAURI__.core.invoke('$Cmd'$a).then((r) => ({ ok: true, r: r === undefined ? null : r }), (e) => ({ ok: false, err: String(e) }))"
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
# 輸入 [{ id, physW, physH, scale, fontScale }]（fontScale＝該小工具自己的字級），回傳 id → { id, box, zoom, zoomAt1,
# cap, auto, capRaw, atCap, next }：cap＝字級極大時的值（上限，已夾 0.5–3）；auto／capRaw＝未夾的自適應倍率與
# 框上限（交叉核對用，直接由 box 算）；atCap＝同 Rust `content_zoom_detail`：字級 ≥ 3 − 1e-9，或字級 +0.1
# （正規化到 0.1、夾 0.5–3）後倍率 ≤ 目前 + 1e-9。
function Get-ExpectedZooms([object[]]$Items, [string]$WorkDir, [string]$ToolsDir = $PSScriptRoot) {
    $zoomHelper = @'
import { readFileSync } from 'node:fs';
const [modUrl, inFile] = process.argv.slice(2);
const m = await import(modUrl);
const boxes = m.widgetZoomBoxes();
const input = JSON.parse(readFileSync(inFile, 'utf8'));
const EPS = 1e-9;
const out = input.map((w) => {
  const box = boxes[w.id];
  if (!box) throw new Error(`沒有 ${w.id} 的倍率框`);
  const zoom = m.contentZoom(w.physW, w.physH, w.scale, box, w.fontScale);
  const next = Math.round(Math.min(Math.max(w.fontScale + 0.1, 0.5), 3) * 10) / 10;
  const lw = w.physW / w.scale;
  const lh = w.physH / w.scale;
  const auto = box.comfortWidth !== null ? Math.min(lw / box.comfortWidth, lh / box.comfortHeight) : lh / box.comfortHeight;
  const capRaw = box.comfortWidth !== null ? Math.min(lw / box.minWidth, lh / box.minHeight) : lh / box.minHeight;
  return {
    id: w.id,
    box,
    fontScale: w.fontScale,
    zoom,
    zoomAt1: m.contentZoom(w.physW, w.physH, w.scale, box, 1),
    cap: m.contentZoom(w.physW, w.physH, w.scale, box, 1e9),
    auto,
    capRaw,
    next,
    atCap: w.fontScale >= 3 - EPS || m.contentZoom(w.physW, w.physH, w.scale, box, next) <= zoom + EPS,
  };
});
console.log(JSON.stringify(out));
'@
    $helper = Join-Path $WorkDir 'zoom-helper.mjs'
    $inFile = Join-Path $WorkDir 'zoom-input.json'
    $enc = New-Object System.Text.UTF8Encoding($false)
    [IO.File]::WriteAllText($helper, $zoomHelper, $enc)
    [IO.File]::WriteAllText($inFile, (ConvertTo-Json @($Items) -Depth 4), $enc)
    $modUrl = ([Uri](Resolve-Path (Join-Path $ToolsDir '..\tests\compare\verify-visual-edges.mjs')).Path).AbsoluteUri
    $r = Invoke-NodeUtf8 -ArgumentList @($helper, $modUrl, $inFile)
    if ($r.ExitCode -ne 0) { throw "預期倍率計算失敗：$($r.StdOut) $($r.StdErr)" }
    $map = @{}
    foreach ($e in ($r.StdOut | ConvertFrom-Json)) { $map[$e.id] = $e }
    return $map
}

# ── 判讀純函式（host/tools/tests/VerifyAdaptiveZoom.Tests.ps1 以 AST 取出測試；不得讀腳本層級變數）──────

# 設定讀回（`get_settings` 的 { hasTop, widgets: { id: { placement, font_scale } } }）：沒有頂層字級、財經小工具的
# 格座標與字級＝預期；`$AllIds`（十個 WIDGET_IDS）每一個都要在 widgets 裡（宿主一律補齊）；`$LegacyTop` 不是
# $null 時（F）`$AllIds` 與 widgets 底下每個小工具（含設定檔沒寫出、由宿主補齊的擴充插槽）的字級都要是它。
function Get-SettingsReadbackProblems($St, $Rects, $Fonts, $LegacyTop = $null, [string[]]$AllIds = @()) {
    $p = @()
    if ($null -eq $St) { return @('get_settings 沒有結果') }
    if ($St.PSObject.Properties['__error']) { return @("get_settings 失敗：$($St.__error)") }
    if ($St.hasTop -ne $false) { $p += "頂層仍有 font_scale（hasTop=$($St.hasTop)）" }
    if ($null -eq $St.widgets) { return $p + @('沒有 widgets') }
    foreach ($id in $Rects.Keys) {
        $w = $St.widgets.$id
        if ($null -eq $w) { $p += "$id 不在 widgets"; continue }
        $g = $Rects[$id]; $pl = $w.placement
        if (-not $pl -or $pl.col -ne $g[0] -or $pl.row -ne $g[1] -or $pl.w -ne $g[2] -or $pl.h -ne $g[3]) {
            $p += "$id 格座標 ($($pl.col),$($pl.row),$($pl.w),$($pl.h)) ≠ 寫入 ($($g -join ','))"
        }
        if (-not (Test-SameFont $w.font_scale $Fonts[$id])) { $p += "$id 字級 $($w.font_scale) ≠ 預期 $($Fonts[$id])" }
    }
    foreach ($id in $AllIds) {
        if ($Rects.Contains($id)) { continue }
        if (-not $St.widgets.PSObject.Properties[$id]) { $p += "$id 不在 widgets（宿主應補齊全部 WIDGET_IDS）" }
    }
    if ($null -ne $LegacyTop) {
        foreach ($k in $St.widgets.PSObject.Properties.Name) {
            if (-not (Test-SameFont $St.widgets.$k.font_scale $LegacyTop)) { $p += "$k 字級 $($St.widgets.$k.font_scale) ≠ 遷移值 $LegacyTop" }
        }
    }
    return $p
}

# 各頁 `get_widget_font_state` 的結果（id → { ok, r: { font_scale, at_cap } | err }）與預期字級、預期 at_cap
# （`$AtCap` 為 $null 時不比 at_cap）比對。
function Get-FontStateProblems($States, $Fonts, $AtCap = $null) {
    $p = @()
    foreach ($id in $Fonts.Keys) {
        $s = $States[$id]
        if ($null -eq $s) { $p += "$id 沒有查詢結果"; continue }
        if ($s.PSObject.Properties['__error']) { $p += "$id 查詢失敗：$($s.__error)"; continue }
        if ($s.ok -ne $true) { $p += "$id 被拒：$($s.err)"; continue }
        if (-not (Test-SameFont $s.r.font_scale $Fonts[$id])) { $p += "$id 字級 $($s.r.font_scale) ≠ 預期 $($Fonts[$id])" }
        if ($null -ne $AtCap -and $AtCap.Contains($id)) {
            if ($s.r.at_cap -isnot [bool]) { $p += "$id at_cap 不是布林（$($s.r.at_cap)）" }
            elseif ($s.r.at_cap -ne [bool]$AtCap[$id]) { $p += "$id at_cap=$($s.r.at_cap) ≠ 公式 $($AtCap[$id])" }
        }
    }
    return $p
}

# 指令被拒（{ ok: false, err }）＝$true；成功、CDP 失敗、格式不對都不算被拒。
function Test-Rejected($R) {
    if ($null -eq $R -or $R.PSObject.Properties['__error']) { return $false }
    return ($R.ok -eq $false -and -not [string]::IsNullOrEmpty([string]$R.err))
}

# 兩個 id → 值 的對照：列出實際與預期不同的項目（值以字串比較；預期有、實際缺也列）。
function Get-MapMismatch($Actual, $Expected) {
    $p = @()
    foreach ($k in $Expected.Keys) {
        $a = if ($Actual.Contains($k)) { $Actual[$k] } else { '<缺>' }
        if ("$a" -ne "$($Expected[$k])") { $p += "$k=$a（預期 $($Expected[$k])）" }
    }
    return $p
}

# G：時鐘頁記錄到的 widget-font 事件 payload 中，有沒有 id=$Id、字級＝$Font 的那筆。
function Test-HasWidgetFontEvent($Events, [string]$Id, $Font) {
    foreach ($e in @($Events)) { if ($e -and $e.id -eq $Id -and (Test-SameFont $e.font_scale $Font)) { return $true } }
    return $false
}

# F：宿主存檔後的隔離設定檔判讀。回傳 { Verdict = PASS|FAIL; Detail }。
# 宿主一定要存過檔：`$Rewritten`（修改時間比寫入時新）為假、或內容與寫入的完全相同＝沒存檔＝FAIL（腳本已以
# set_edit_mode 強制觸發存檔，沒存就是問題）。存過檔則不得有頂層 font_scale、`$Ids`（十個 WIDGET_IDS）都要在
# widgets 裡、widgets 底下每個小工具的 font_scale 都＝$Value。
function Get-MigratedFileVerdict([string]$Written, [string]$ReadBack, [string[]]$Ids, $Value, [bool]$Rewritten) {
    if ([string]::IsNullOrEmpty($ReadBack)) { return [PSCustomObject]@{ Verdict = 'FAIL'; Detail = '讀不到設定檔' } }
    if (-not $Rewritten) { return [PSCustomObject]@{ Verdict = 'FAIL'; Detail = '設定檔修改時間沒有變（宿主沒有存檔）' } }
    if ($ReadBack -eq $Written) { return [PSCustomObject]@{ Verdict = 'FAIL'; Detail = '設定檔內容與寫入的完全相同（宿主沒有改寫成新格式）' } }
    try { $j = $ReadBack | ConvertFrom-Json -ErrorAction Stop } catch { return [PSCustomObject]@{ Verdict = 'FAIL'; Detail = "設定檔不是 JSON：$($_.Exception.Message)" } }
    $p = @()
    if ($j.PSObject.Properties['font_scale']) { $p += "頂層仍有 font_scale=$($j.font_scale)" }
    if ($null -eq $j.widgets) { $p += '沒有 widgets' }
    else {
        foreach ($id in $Ids) { if (-not $j.widgets.PSObject.Properties[$id]) { $p += "$id 不在 widgets" } }
        foreach ($k in $j.widgets.PSObject.Properties.Name) {
            if (-not (Test-SameFont $j.widgets.$k.font_scale $Value)) { $p += "$k 字級 $($j.widgets.$k.font_scale) ≠ $Value" }
        }
    }
    if ($p.Count -gt 0) { return [PSCustomObject]@{ Verdict = 'FAIL'; Detail = ($p -join '; ') } }
    return [PSCustomObject]@{ Verdict = 'PASS'; Detail = "已存檔、無頂層 font_scale、$(@($j.widgets.PSObject.Properties).Count) 個小工具都是 $Value" }
}

# 清單列量測（單引號 JS、無 $）：每個看得見的 `.ev` 的 scroll／client 寬與左右緣，以及看得見的直接子元素的外框與
# scroll／client 寬；`flexWrap` 是第一列的 computed flex-wrap（收合版面＝wrap）。
function Get-RowLayoutExpr {
    return '(() => { const r2 = (v) => Math.round(v * 100) / 100; const vis = (el) => { const b = el.getBoundingClientRect(); return b.width > 0 && b.height > 0; }; const name = (el) => el.tagName.toLowerCase() + (el.className ? ''.'' + String(el.className).trim().split(/\s+/).join(''.'') : ''''); const rows = Array.from(document.querySelectorAll(''.ev'')).filter(vis); return { vw: window.innerWidth, flexWrap: rows.length ? getComputedStyle(rows[0]).flexWrap : null, rows: rows.map((el, i) => { const b = el.getBoundingClientRect(); return { i: i, sw: el.scrollWidth, cw: el.clientWidth, l: r2(b.left), r: r2(b.right), kids: Array.from(el.children).filter(vis).map((k) => { const kb = k.getBoundingClientRect(); return { n: name(k), sw: k.scrollWidth, cw: k.clientWidth, l: r2(kb.left), t: r2(kb.top), r: r2(kb.right), b: r2(kb.bottom) }; }) }; }) }; })()'
}

# 清單列判讀：子元素外框兩兩不相交（容差 $Tol CSS px；只碰邊不算）、子元素與列 scrollWidth ≤ clientWidth、
# 子元素左右緣在列內、列在 viewport 內。`-RequireRows` 時沒有列也算問題。回傳 { Problems; RowCount; KidCount }。
function Get-RowLayoutProblems($M, [switch]$RequireRows, [double]$Tol = 0.5) {
    $p = New-Object System.Collections.Generic.List[string]
    if ($null -eq $M) { return [PSCustomObject]@{ Problems = @('沒有量測結果'); RowCount = 0; KidCount = 0 } }
    if ($M.PSObject.Properties['__error']) { return [PSCustomObject]@{ Problems = @("量測失敗：$($M.__error)"); RowCount = 0; KidCount = 0 } }
    $rows = @($M.rows | Where-Object { $null -ne $_ })
    if ($rows.Count -eq 0 -and $RequireRows) { $p.Add('沒有任何看得見的 .ev 列可檢查') }
    $kidCount = 0
    $vw = [double]$M.vw
    foreach ($row in $rows) {
        $kids = @($row.kids | Where-Object { $null -ne $_ })
        $kidCount += $kids.Count
        if ([double]$row.sw -gt [double]$row.cw) { $p.Add("列 $($row.i) 溢出：scrollWidth $($row.sw) > clientWidth $($row.cw)") }
        if ([double]$row.l -lt (-$Tol) -or [double]$row.r -gt $vw + $Tol) { $p.Add("列 $($row.i) 超出 viewport：[$($row.l),$($row.r)] vw=$vw") }
        foreach ($k in $kids) {
            if ([double]$k.sw -gt [double]$k.cw) { $p.Add("列 $($row.i) $($k.n) 被裁切：scrollWidth $($k.sw) > clientWidth $($k.cw)") }
            if ([double]$k.l -lt [double]$row.l - $Tol -or [double]$k.r -gt [double]$row.r + $Tol) { $p.Add("列 $($row.i) $($k.n) 超出列：[$($k.l),$($k.r)] 列 [$($row.l),$($row.r)]") }
        }
        for ($a = 0; $a -lt $kids.Count; $a++) {
            for ($b = $a + 1; $b -lt $kids.Count; $b++) {
                $x = $kids[$a]; $y = $kids[$b]
                $hit = ([double]$x.l -lt [double]$y.r - $Tol) -and ([double]$y.l -lt [double]$x.r - $Tol) -and ([double]$x.t -lt [double]$y.b - $Tol) -and ([double]$y.t -lt [double]$x.b - $Tol)
                if ($hit) { $p.Add("列 $($row.i) $($x.n) [$($x.l),$($x.t),$($x.r),$($x.b)] 與 $($y.n) [$($y.l),$($y.t),$($y.r),$($y.b)] 重疊") }
            }
        }
    }
    $list = @($p)
    if ($list.Count -gt 12) { $list = @($list[0..11]) + @("…另有 $($list.Count - 12) 筆") }
    return [PSCustomObject]@{ Problems = $list; RowCount = $rows.Count; KidCount = $kidCount }
}

# ── 情境的格座標與字級（依主螢幕工作區與縮放比例換算）──────────────────────────────────
# 讀腳本層級的 $pwa／$pScale／$DefaultRects／$FinanceIds／$ScenarioDFonts／$GRID（測試會自己設定這些變數）。
function New-ScenarioPlan([string]$Name) {
    $rects = [ordered]@{}
    $fonts = [ordered]@{}
    foreach ($id in $FinanceIds) { $rects[$id] = [int[]]$DefaultRects[$id].Clone(); $fonts[$id] = 1.0 }
    $legacyTop = $null
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
        'D' {
            foreach ($id in $FinanceIds) { $fonts[$id] = [double]$ScenarioDFonts[$id] }
            $note = '預設版面、各小工具不同字級'
        }
        'E' {
            # 總經日曆邏輯寬約 0.64×500＝320（無條件進位到格，確保 ≥ 320 → 字級 2.0 時自適應×字級 ≥ 1.28），
            # 高約 600（≫ 字級 2.0 不受高度限制所需的 1.28×216≈277；矮螢幕上以第 12 列到底為限，是否仍不限制
            # 倍率由實跑時的前提項目判定），移到左側 col 0、時鐘列之下。
            $w = Get-Cells 320 $pScale $pwa.W; $h = [int][math]::Min([double](Get-Cells 600 $pScale $pwa.H), [double]($GRID - 12))
            $rects['macro'] = [int[]]@(0, 12, $w, $h)
            $fonts['macro'] = 2.0
            $note = '總經日曆窄框（約 0.64 倍設計寬）、高度充足、字級 200%'
        }
        'F' {
            $legacyTop = 1.2
            foreach ($id in $FinanceIds) { $fonts[$id] = 1.2 }
            $note = 'v0.2.0 格式：頂層 font_scale 1.2、各小工具沒有字級'
        }
        'G' { $note = '預設版面、全部 100%；以 CDP 呼叫字級指令' }
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
    [PSCustomObject]@{ Name = $Name; Rects = $rects; Fonts = $fonts; LegacyTopFont = $legacyTop; Note = $note; Problems = $problems }
}
# 新格式：字級寫在 `widgets.<id>.font_scale`；`LegacyTopFont` 不是 $null（F）時改寫 v0.2.0 格式：頂層
# `font_scale`、小工具底下不寫字級。小工具清單取自 `$Plan.Rects`（不讀腳本層級變數）＝只有五個財經小工具：
# F 因此刻意不列 custom1–custom5，用來實證「設定檔沒列出的小工具也遷移成頂層值」（design.md D1 第 3 步）。
function ConvertTo-SettingsJson($Plan, [int]$Version) {
    $widgets = [ordered]@{}
    foreach ($id in $Plan.Rects.Keys) {
        $g = $Plan.Rects[$id]
        $w = [ordered]@{ enabled = $true; placement = [ordered]@{ monitor = 'primary'; col = $g[0]; row = $g[1]; w = $g[2]; h = $g[3] } }
        if ($null -eq $Plan.LegacyTopFont) { $w['font_scale'] = [double]$Plan.Fonts[$id] }
        $widgets[$id] = $w
    }
    $s = [ordered]@{ version = $Version; data_fetch = 'off' }
    if ($null -ne $Plan.LegacyTopFont) { $s['font_scale'] = [double]$Plan.LegacyTopFont }
    $s['widgets'] = $widgets
    return (ConvertTo-Json $s -Depth 6)
}

$primary = @([System.Windows.Forms.Screen]::AllScreens | Where-Object { $_.Primary })[0]
$pwa = [PSCustomObject]@{ X = $primary.WorkingArea.X; Y = $primary.WorkingArea.Y; W = $primary.WorkingArea.Width; H = $primary.WorkingArea.Height }
$pt = New-Object VZoomA.Native+POINT
$pt.X = $primary.Bounds.X + 1; $pt.Y = $primary.Bounds.Y + 1
$dpiX = [uint32]0; $dpiY = [uint32]0
[void][VZoomA.Native]::GetDpiForMonitor([VZoomA.Native]::MonitorFromPoint($pt, 1), 0, [ref]$dpiX, [ref]$dpiY)
$pScale = [double]$dpiX / 96.0

# 從 Rust 原始碼讀 SETTINGS_VERSION 與 DEFAULT_GRID_RECTS 核對（不一致就不跑，避免寫出被宿主重設的設定檔）。
$settingsRs = Get-Content (Join-Path $PSScriptRoot '..\src\settings.rs') -Raw
$verMatch = [regex]::Match($settingsRs, 'pub const SETTINGS_VERSION: u32 = (\d+);')
if (-not $verMatch.Success) { throw 'settings.rs 找不到 SETTINGS_VERSION' }
$SettingsVersion = [int]$verMatch.Groups[1].Value
$rectsBlock = [regex]::Match($settingsRs, 'pub const DEFAULT_GRID_RECTS: \[GridRect; \d+\] = \[([\s\S]*?)\n\];').Groups[1].Value
$rustRects = @([regex]::Matches($rectsBlock, 'grid\((\d+),\s*(\d+),\s*(\d+),\s*(\d+)\)') | ForEach-Object { ($_.Groups[1..4] | ForEach-Object { $_.Value }) -join ',' })
$mine = @($FinanceIds | ForEach-Object { $DefaultRects[$_] -join ',' })
if ((($rustRects[0..4]) -join ';') -ne ($mine -join ';')) { throw "本腳本的預設格座標與 settings.rs 不一致：Rust=$($rustRects[0..4] -join ';') 本腳本=$($mine -join ';')" }
$idsBlock = [regex]::Match($settingsRs, 'pub const WIDGET_IDS: \[&str; \d+\] = \[([^\]]*)\]')
if (-not $idsBlock.Success) { throw 'settings.rs 找不到 WIDGET_IDS' }
$rustIds = @([regex]::Matches($idsBlock.Groups[1].Value, '"([^"]+)"') | ForEach-Object { $_.Groups[1].Value })
if (($rustIds -join ',') -ne ($AllWidgetIds -join ',')) { throw "本腳本的 `$AllWidgetIds 與 settings.rs WIDGET_IDS 不一致：Rust=$($rustIds -join ',') 本腳本=$($AllWidgetIds -join ',')" }

# pwsh -File 模式下 `-Scenarios A,B` 會變成單一字串 "A,B"：一律再以逗號切開。
$Scenarios = @($Scenarios | ForEach-Object { $_ -split ',' } | ForEach-Object { $_.Trim() } | Where-Object { $_ })
$plans = @($Scenarios | ForEach-Object { New-ScenarioPlan $_ })

if ($DryRun) {
    Write-Host "主螢幕工作區 $(Fmt $pwa) 縮放 $pScale；SETTINGS_VERSION=$SettingsVersion"
    $tmp = Join-Path ([IO.Path]::GetTempPath()) ('fc-host-zoomdry-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
    New-Item -ItemType Directory -Force -Path $tmp | Out-Null
    try {
        foreach ($p in $plans) {
            $items = @(foreach ($id in $FinanceIds) { $gr = Grid-Rect $pwa $p.Rects[$id]; [PSCustomObject]@{ id = $id; physW = $gr.W; physH = $gr.H; scale = $pScale; fontScale = [double]$p.Fonts[$id] } })
            $exp = Get-ExpectedZooms $items $tmp
            Write-Host "── 情境 $($p.Name)（$($p.Note)）問題：$(if ($p.Problems.Count) { $p.Problems -join '; ' } else { '無' })"
            foreach ($it in $items) {
                $e = $exp[$it.id]
                $lw = $it.physW / $pScale
                Write-Host ("  {0,-8} 格={1,-12} 實體={2}x{3} 邏輯={4:N1}x{5:N1} 字級={6} 倍率={7:N4} at_cap={8}（字級 1 時 {9:N4}、上限 {10:N4}、自適應 {11:N4}）頁面 CSS 寬={12:N1}" -f $it.id, ($p.Rects[$it.id] -join ','), $it.physW, $it.physH, $lw, ($it.physH / $pScale), $it.fontScale, $e.zoom, $e.atCap, $e.zoomAt1, $e.cap, $e.auto, ($lw / [double]$e.zoom))
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
# 判讀函式「沒有問題」時 `return @()` 在管線上什麼都不輸出，當成參數傳進來就變成 $null，`@($null)` 卻是 1 個元素
# ——task 5.2 實跑時所有讀回項目因此 FAIL、「問題：」後面是空的。只計非空白的問題字串。
function Set-ProblemResult([string]$Key, $Problems) {
    $list = @($Problems | Where-Object { -not [string]::IsNullOrWhiteSpace([string]$_) })
    if ($list.Count -gt 0) { Log "  問題：$($list -join '; ')" }
    Set-Result $Key ($list.Count -eq 0)
}
function Assert-Unlocked([string]$Where) { if (Test-Locked) { throw "LOCKED：$Where 時偵測到工作階段鎖定" } }
function Get-FontStates([string[]]$Ids) {
    $m = @{}
    foreach ($id in $Ids) { $m[$id] = Invoke-Json "w=$id" (Get-InvokeExpr 'get_widget_font_state') }
    return $m
}
function Format-FontStates($States) {
    ($States.Keys | Sort-Object | ForEach-Object { $s = $States[$_]; if ($s.ok) { "$_=$($s.r.font_scale)/at_cap=$($s.r.at_cap)" } elseif ($s.PSObject.Properties['__error']) { "$_=錯誤($($s.__error))" } else { "$_=拒絕($($s.err))" } }) -join ' '
}
# 頁面上字級控制的狀態：{ present, pct, l, t, r, b, vw, vh }（單引號 JS、無 $）。
$FontControlExpr = '(() => { const box = document.querySelector(''.font-controls''); if (!box) return { present: false, vw: window.innerWidth, vh: window.innerHeight }; const b = box.getBoundingClientRect(); const pct = box.querySelector(''.font-pct''); return { present: true, pct: pct ? pct.textContent : null, l: Math.round(b.left * 100) / 100, t: Math.round(b.top * 100) / 100, r: Math.round(b.right * 100) / 100, b: Math.round(b.bottom * 100) / 100, vw: window.innerWidth, vh: window.innerHeight }; })()'
$SettingsExpr = "window.__TAURI__.core.invoke('get_settings').then((s) => ({ hasTop: Object.prototype.hasOwnProperty.call(s, 'font_scale'), widgets: Object.fromEntries(Object.keys(s.widgets).map((k) => [k, { placement: s.widgets[k].placement, font_scale: s.widgets[k].font_scale }])) }))"

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
        Log "## ── 情境 $S：$($plan.Note)；字級 $(($FinanceIds | ForEach-Object { "$_=$($plan.Fonts[$_])" }) -join ' ')$(if ($null -ne $plan.LegacyTopFont) { "（頂層 font_scale=$($plan.LegacyTopFont)）" })；格座標 $(($FinanceIds | ForEach-Object { "$_=($($plan.Rects[$_] -join ','))" }) -join ' ')"
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
        $cfgFile = Join-Path $cfgDir 'settings.json'
        New-Item -ItemType Directory -Force -Path $cfgDir, $dataDir | Out-Null
        Copy-Item (Join-Path $PSScriptRoot '..\ui\fixtures\tw-events.json') (Join-Path $dataDir 'tw_events.json')
        $json = ConvertTo-SettingsJson $plan $SettingsVersion
        [IO.File]::WriteAllText($cfgFile, $json, (New-Object System.Text.UTF8Encoding($false)))
        $cfgWrittenTicks = (Get-Item -LiteralPath $cfgFile).LastWriteTimeUtc.Ticks   # F 用來判斷宿主有沒有改寫
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

            $st = Invoke-Json 'w=clock' $SettingsExpr
            Log "  get_settings：頂層字級=$($st.hasTop) $(($FinanceIds | ForEach-Object { $w = $st.widgets.$_; $p = $w.placement; "$_=($($p.col),$($p.row),$($p.w),$($p.h))/字級 $($w.font_scale)" }) -join ' ')$(if ($st.PSObject.Properties['__error']) { " 錯誤=$($st.__error)" })"
            Set-ProblemResult "$S：設定讀回：無頂層 font_scale、各小工具字級＝$(if ($null -ne $plan.LegacyTopFont) { "遷移值 $($plan.LegacyTopFont)（含擴充插槽）" } else { '寫入值' })、格座標＝寫入值" (Get-SettingsReadbackProblems $st $plan.Rects $plan.Fonts $plan.LegacyTopFont $AllWidgetIds)

            # 量測視窗矩形與縮放比例，計算預期倍率。
            $meas = [ordered]@{}
            foreach ($id in $FinanceIds) {
                if ($hwnds[$id] -eq [IntPtr]::Zero) { continue }
                $r = Get-Rect $hwnds[$id]
                $scale = [double][VZoomA.Native]::GetDpiForWindow($hwnds[$id]) / 96.0
                $meas[$id] = [PSCustomObject]@{ Rect = $r; Scale = $scale }
            }
            $exp = Get-ExpectedZooms @($meas.Keys | ForEach-Object { [PSCustomObject]@{ id = $_; physW = $meas[$_].Rect.W; physH = $meas[$_].Rect.H; scale = $meas[$_].Scale; fontScale = [double]$plan.Fonts[$_] } }) $tempRoot

            $fontStates = Get-FontStates $FinanceIds
            $atCapWant = [ordered]@{}
            foreach ($id in $meas.Keys) { $atCapWant[$id] = [bool]$exp[$id].atCap }
            Log "  get_widget_font_state：$(Format-FontStates $fontStates)；公式 at_cap：$(($atCapWant.Keys | ForEach-Object { "$_=$($atCapWant[$_])" }) -join ' ')"
            Set-ProblemResult "$S：get_widget_font_state 各小工具字級＝預期" (Get-FontStateProblems $fontStates $plan.Fonts)
            Set-ProblemResult "$S：get_widget_font_state 的 at_cap＝公式（字級 3.0 或 +0.1 後倍率不再變大）" (Get-FontStateProblems $fontStates $plan.Fonts $atCapWant)

            $rectAll = $true; $zoomAll = $true; $clipAll = $true
            $rowProblems = @()
            $rowMeas = @{}
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
                if ($pg.PSObject.Properties['__error']) { $clipNotes += "量測失敗：$($pg.__error)" }
                if ($clipNotes.Count -gt 0) { $clipAll = $false }
                Log ("  {0,-8} rect={1} 期望格線矩形={2} 相符={3} 縮放={4} 邏輯={5:N1}x{6:N1} 字級={7} 預期倍率={8:N4}（字級 1 時 {9:N4}、上限 {10:N4}、自適應 {11:N4}） 頁面 dpr={12} → 實際倍率={13:N4} 相符={14} viewport={15}x{16} 裁切={17}" -f $id, (Fmt $m.Rect), (Fmt $want), $rectOk, $m.Scale, $lw, $lh, $plan.Fonts[$id], [double]$e.zoom, [double]$e.zoomAt1, [double]$e.cap, [double]$e.auto, $pg.dpr, $actual, $zoomOk, $pg.vw, $pg.vh, $(if ($clipNotes.Count) { $clipNotes -join '; ' } else { '無' }))
                foreach ($el in @($pg.els)) { if (-not $el.missing) { Log "      $($el.sel)：scroll $($el.sw)x$($el.sh) client $($el.cw)x$($el.ch) 外框 [$($el.l),$($el.t),$($el.r),$($el.b)]" } }
                if ($ListIds -contains $id) {
                    $rm = Invoke-Json "w=$id" (Get-RowLayoutExpr)
                    $rowMeas[$id] = $rm
                    $rv = Get-RowLayoutProblems $rm -RequireRows:($S -eq 'E' -and $id -eq 'macro')
                    Log "      清單列：$($rv.RowCount) 列、$($rv.KidCount) 個子元素、flex-wrap=$($rm.flexWrap)、問題 $(@($rv.Problems).Count) 筆"
                    if ($rv.RowCount -eq 0) { Log "      （$id 沒有看得見的 .ev 列）" }
                    $rowProblems += @($rv.Problems | ForEach-Object { "$id：$_" })
                }
                $zoomTable.Add([PSCustomObject]@{ Scenario = $S; Id = $id; Rect = (Fmt $m.Rect); Logical = ('{0:N1}x{1:N1}' -f $lw, $lh); Font = $plan.Fonts[$id]; Expected = [double]$e.zoom; Actual = $actual; Cap = [double]$e.cap; ZoomAt1 = [double]$e.zoomAt1; Ok = $zoomOk })
                $meas[$id] | Add-Member -NotePropertyName Expected -NotePropertyValue ([double]$e.zoom)
                $meas[$id] | Add-Member -NotePropertyName Actual -NotePropertyValue $actual
                $meas[$id] | Add-Member -NotePropertyName Cap -NotePropertyValue ([double]$e.cap)
                $meas[$id] | Add-Member -NotePropertyName ZoomAt1 -NotePropertyValue ([double]$e.zoomAt1)
                $meas[$id] | Add-Member -NotePropertyName Auto -NotePropertyValue ([double]$e.auto)
                $meas[$id] | Add-Member -NotePropertyName CapRaw -NotePropertyValue ([double]$e.capRaw)
                $meas[$id] | Add-Member -NotePropertyName Box -NotePropertyValue $e.box
                $meas[$id] | Add-Member -NotePropertyName Lw -NotePropertyValue $lw
                $meas[$id] | Add-Member -NotePropertyName Lh -NotePropertyValue $lh
            }
            Set-Result "$S：五個小工具的視窗矩形＝設定格座標的格線矩形" ($rectAll -and $meas.Count -eq 5)
            Set-Result "$S：五個小工具的實際倍率（dpr ÷ 縮放）＝content_zoom 公式（實際矩形、縮放、各自字級），容差 0.01" ($zoomAll -and $meas.Count -eq 5)
            Set-Result "$S：無裁切（時鐘文字與面板、行情條面板高度、清單面板寬度）" ($clipAll -and $meas.Count -eq 5)
            Set-ProblemResult "$S：清單列無重疊無裁切（.ev 子元素兩兩不交、scrollWidth ≤ clientWidth、在列與 viewport 內）" $rowProblems

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
                'D' {
                    $dp = @()
                    foreach ($id in $FinanceIds) {
                        $m = $meas[$id]
                        if (-not $m) { $dp += "$id 沒有量測"; continue }
                        $f = [double]$plan.Fonts[$id]
                        $simple = [math]::Min([math]::Max([math]::Min($m.Auto * $f, $m.CapRaw), 0.5), 3.0)
                        Log ("  D {0}：自適應 {1:N4} × 字級 {2} = {3:N4}，框上限 {4:N4} → clamp(min)={5:N4}；實際 {6:N4}" -f $id, $m.Auto, $f, ($m.Auto * $f), $m.CapRaw, $simple, $m.Actual)
                        if ([math]::Abs($m.Actual - $simple) -gt 0.01) { $dp += "$id 實際 $($m.Actual) ≠ 簡式 $simple" }
                    }
                    Set-ProblemResult 'D：每個小工具倍率＝clamp(min(自適應 × 自己的字級, 框上限), 0.5, 3)（獨立簡式，容差 0.01）' $dp
                    $okCap = $true
                    foreach ($id in 'clock', 'quotes') {
                        $m = $meas[$id]
                        Log ("  D {0}：字級 {1}，實際 {2:N4} 上限 {3:N4}（字級 1 時 {4:N4}）" -f $id, $plan.Fonts[$id], $m.Actual, $m.Cap, $m.ZoomAt1)
                        if (-not $m -or [math]::Abs($m.Actual - $m.Cap) -gt 0.01) { $okCap = $false }
                    }
                    Set-Result 'D：時鐘（150%）、行情條（130%）字級 >1 時倍率停在上限' $okCap
                    $distinct = @($FinanceIds | ForEach-Object { $meas[$_].Actual / [math]::Max($meas[$_].ZoomAt1, 1e-9) } | ForEach-Object { [math]::Round($_, 2) } | Sort-Object -Unique).Count
                    Log "  D：實際倍率 ÷ 字級 1 倍率 的相異值 $distinct 個（各小工具字級確實各自生效）"
                }
                'E' {
                    $mc = $meas['macro']
                    $rm = $rowMeas['macro']
                    $cw = if ($mc) { [double]$mc.Box.comfortWidth } else { [double]::NaN }
                    $minW = if ($mc) { [double]$mc.Box.minWidth } else { [double]::NaN }
                    $pre = $mc -and $mc.Lw -ge 0.64 * $cw - 0.5 -and $mc.Lw -lt 0.75 * $cw -and [math]::Abs($mc.Auto - $mc.Lw / $cw) -lt 1e-9 -and $mc.CapRaw -ge 2.0 * $mc.Auto
                    $simple = if ($mc) { [math]::Min(2.0 * $mc.Lw / $cw, $mc.Lw / $minW) } else { [double]::NaN }
                    $cssW = if ($mc -and $mc.Actual -gt 0) { $mc.Lw / $mc.Actual } else { [double]::NaN }
                    Log ("  E 交叉核對：總經日曆邏輯 {0:N1}x{1:N1}（設計寬 {2}、min 寬 {3}）；自適應 {4:N4}、框上限 {5:N4}；min(2×寬/{2}, 寬/{3})={6:N4} 實際 {7:N4}；頁面 CSS 寬 {8:N1}；flex-wrap={9}" -f $mc.Lw, $mc.Lh, $cw, $minW, $mc.Auto, $mc.CapRaw, $simple, $mc.Actual, $cssW, $rm.flexWrap)
                    Set-Result 'E：總經日曆邏輯寬約 0.64 倍設計寬（≥ 0.64×、< 0.75×）、高度不限制倍率（前提）' ([bool]$pre)
                    Set-Result 'E：字級 200% 時總經日曆倍率 ≥ 1.2' ($mc -and $mc.Actual -ge 1.2)
                    Set-Result 'E：倍率＝min(2 × 寬/設計寬, 寬/min 寬)（由寬度決定；獨立簡式，容差 0.01）' ($mc -and [math]::Abs($mc.Actual - $simple) -le 0.01)
                    Set-Result 'E：收合版面生效（.ev 的 flex-wrap 為 wrap）' ($rm -and $rm.flexWrap -eq 'wrap')
                }
                'F' {
                    # 觸發存檔：set_edit_mode(true) 會存 layout_locked=false、(false) 再存一次 true（都是同步存檔）。
                    $e1 = Invoke-Json 'w=clock' (Get-InvokeExpr 'set_edit_mode' '{ enabled: true }')
                    Start-Sleep -Milliseconds 800
                    $e2 = Invoke-Json 'w=clock' (Get-InvokeExpr 'set_edit_mode' '{ enabled: false }')
                    Start-Sleep -Milliseconds 800
                    Log "  F 觸發存檔：set_edit_mode(true) ok=$($e1.ok)$($e1.err)$($e1.__error)；set_edit_mode(false) ok=$($e2.ok)$($e2.err)$($e2.__error)"
                    Set-Result 'F：觸發存檔的 set_edit_mode(true)、set_edit_mode(false) 都回 ok' ($e1.ok -eq $true -and $e2.ok -eq $true)
                    $readBack = try { [IO.File]::ReadAllText($cfgFile) } catch { '' }
                    $nowTicks = try { (Get-Item -LiteralPath $cfgFile).LastWriteTimeUtc.Ticks } catch { 0 }
                    $fv = Get-MigratedFileVerdict $json $readBack $AllWidgetIds $plan.LegacyTopFont ($nowTicks -gt $cfgWrittenTicks)
                    Log "  F 設定檔讀回：修改時間 $cfgWrittenTicks → $nowTicks；$($fv.Verdict) $($fv.Detail)；內容：$(($readBack -replace '\s+', ' '))"
                    Set-Result 'F：宿主已改寫設定檔：沒有頂層 font_scale、十個 WIDGET_IDS（含未寫出的 custom1–custom5）都在且字級 1.2' ($fv.Verdict -eq 'PASS')
                }
                'G' {
                    $id0 = 'macro'
                    $before = [ordered]@{}
                    foreach ($id in $FinanceIds) { $before[$id] = if ($fontStates[$id].ok) { "$($fontStates[$id].r.font_scale)" } else { '<錯誤>' } }
                    # 1. 不在編輯版面（版面鎖定）時被拒、字級不變。
                    $r1 = Invoke-Json "w=$id0" (Get-InvokeExpr 'adjust_widget_font_scale' '{ step: 1 }')
                    $s1 = Get-FontStates @($id0)
                    Log "  G1 鎖定時 adjust：ok=$($r1.ok) err=$($r1.err)$($r1.__error)；之後 $(Format-FontStates $s1)"
                    Set-Result 'G：不在編輯版面（版面鎖定）時 adjust_widget_font_scale 被拒' (Test-Rejected $r1)
                    Set-ProblemResult 'G：被拒後總經字級仍為 1.0' (Get-FontStateProblems $s1 ([ordered]@{ macro = 1.0 }))
                    # 2. 時鐘頁掛 widget-font 記錄器（同頁面的 listen：getCurrentWebviewWindow().listen）。
                    $lr = Invoke-Json 'w=clock' "window.__TAURI__.webviewWindow.getCurrentWebviewWindow().listen('widget-font', (e) => { (window.__vzWidgetFont = window.__vzWidgetFont || []).push(e.payload); }).then(() => ({ ok: true }), (e) => ({ ok: false, err: String(e) }))"
                    Log "  G2 時鐘頁掛 widget-font 記錄器：ok=$($lr.ok)$($lr.err)$($lr.__error)"
                    # 3. 進入編輯版面：五頁都有字級控制、顯示 100%、控制在 viewport 內。
                    $em = Invoke-Json 'w=clock' (Get-InvokeExpr 'set_edit_mode' '{ enabled: true }')
                    Start-Sleep -Milliseconds 1500
                    $ctl = @{}
                    foreach ($id in $FinanceIds) { $ctl[$id] = Invoke-Json "w=$id" $FontControlExpr }
                    Log "  G3 set_edit_mode(true) ok=$($em.ok)$($em.err)$($em.__error)；字級控制：$(($FinanceIds | ForEach-Object { $c = $ctl[$_]; "$_=present:$($c.present)/pct:$($c.pct)/[$($c.l),$($c.t),$($c.r),$($c.b)] vp $($c.vw)x$($c.vh)" }) -join ' ')"
                    $cp = @()
                    foreach ($id in $FinanceIds) {
                        $c = $ctl[$id]
                        if ($c.present -ne $true) { $cp += "$id 沒有 .font-controls"; continue }
                        if ([double]$c.l -lt -0.5 -or [double]$c.t -lt -0.5 -or [double]$c.r -gt [double]$c.vw + 0.5 -or [double]$c.b -gt [double]$c.vh + 0.5) { $cp += "$id 字級控制 [$($c.l),$($c.t),$($c.r),$($c.b)] 超出 viewport $($c.vw)x$($c.vh)" }
                    }
                    Set-ProblemResult 'G：編輯版面中五個小工具都有字級控制且在視窗內' $cp
                    $pctBefore = @{}; foreach ($id in $FinanceIds) { $pctBefore[$id] = $ctl[$id].pct }
                    Set-ProblemResult 'G：編輯版面中五個小工具的百分比都顯示 100%' (Get-MapMismatch $pctBefore ([ordered]@{ clock = '100%'; macro = '100%'; fixed = '100%'; dynamic = '100%'; quotes = '100%' }))
                    # 4. 總經頁 +1：回 1.1，只有總經變。
                    $r2 = Invoke-Json "w=$id0" (Get-InvokeExpr 'adjust_widget_font_scale' '{ step: 1 }')
                    Start-Sleep -Milliseconds 1500
                    Log "  G4 編輯版面中 adjust(+1)：ok=$($r2.ok) font_scale=$($r2.r.font_scale) at_cap=$($r2.r.at_cap) err=$($r2.err)$($r2.__error)"
                    Set-Result 'G：編輯版面中 adjust_widget_font_scale({step:1}) 成功、回傳 1.1' ($r2.ok -eq $true -and (Test-SameFont $r2.r.font_scale 1.1))
                    $after = Get-FontStates $FinanceIds
                    $wantAfter = [ordered]@{}; foreach ($id in $FinanceIds) { $wantAfter[$id] = 1.0 }; $wantAfter[$id0] = 1.1
                    Log "  G4 之後 get_widget_font_state：$(Format-FontStates $after)"
                    Set-ProblemResult 'G：只有呼叫端（總經）字級 +0.1，其他小工具 get_widget_font_state 不變' (Get-FontStateProblems $after $wantAfter)
                    $st2 = Invoke-Json 'w=clock' $SettingsExpr
                    Set-ProblemResult 'G：get_settings 只有總經 1.1、其他 1.0、無頂層 font_scale' (Get-SettingsReadbackProblems $st2 $plan.Rects $wantAfter $null $AllWidgetIds)
                    # 5. 頁面百分比：CDP 呼叫的回傳值不經頁面，總經頁只能靠 widget-font 事件更新成 110%；其他頁不變。
                    $pctAfter = @{}
                    foreach ($id in $FinanceIds) { $pctAfter[$id] = (Invoke-Json "w=$id" $FontControlExpr).pct }
                    $wantPct = [ordered]@{ clock = '100%'; macro = '110%'; fixed = '100%'; dynamic = '100%'; quotes = '100%' }
                    Log "  G5 頁面百分比：$(($FinanceIds | ForEach-Object { "$_=$($pctAfter[$_])" }) -join ' ')"
                    Set-ProblemResult 'G：widget-font 事件只被總經頁採用（總經 110%、其他仍 100%）' (Get-MapMismatch $pctAfter $wantPct)
                    $ev = Invoke-Json 'w=clock' '(window.__vzWidgetFont || [])'
                    Log "  G5 時鐘頁收到的 widget-font：$((@($ev) | ForEach-Object { "{id=$($_.id),font_scale=$($_.font_scale),at_cap=$($_.at_cap)}" }) -join ' ')"
                    Set-Result 'G：時鐘頁收到 id=macro、字級 1.1 的 widget-font 事件（廣播），但自己的百分比未變' ($lr.ok -eq $true -and (Test-HasWidgetFontEvent $ev 'macro' 1.1) -and $pctAfter['clock'] -eq '100%')
                    # 6. 總經倍率依 1.1 重算。
                    if ($hwnds[$id0] -ne [IntPtr]::Zero) {
                        $r = Get-Rect $hwnds[$id0]; $sc = [double][VZoomA.Native]::GetDpiForWindow($hwnds[$id0]) / 96.0
                        $e11 = (Get-ExpectedZooms @([PSCustomObject]@{ id = $id0; physW = $r.W; physH = $r.H; scale = $sc; fontScale = 1.1 }) $tempRoot)[$id0]
                        $pg = Invoke-Json "w=$id0" (Get-MeasureExpr $id0)
                        $act = if ($pg.dpr) { [double]$pg.dpr / $sc } else { [double]::NaN }
                        Log ("  G6 總經字級 1.1：預期倍率 {0:N4}（at_cap {1}）實際 {2:N4}" -f [double]$e11.zoom, $e11.atCap, $act)
                        $zoomTable.Add([PSCustomObject]@{ Scenario = 'G+1'; Id = $id0; Rect = (Fmt $r); Logical = ('{0:N1}x{1:N1}' -f ($r.W / $sc), ($r.H / $sc)); Font = 1.1; Expected = [double]$e11.zoom; Actual = $act; Cap = [double]$e11.cap; ZoomAt1 = [double]$e11.zoomAt1; Ok = ([math]::Abs($act - [double]$e11.zoom) -le 0.01) })
                        Set-Result 'G：總經倍率改依字級 1.1 計算（容差 0.01）' ([math]::Abs($act - [double]$e11.zoom) -le 0.01)
                        Set-Result 'G：adjust 回傳的 at_cap＝公式' ($r2.ok -eq $true -and $r2.r.at_cap -is [bool] -and $r2.r.at_cap -eq [bool]$e11.atCap)
                    }
                    # 7. 編輯版面中的外觀存證（只截小工具自己的視窗）。
                    foreach ($id in $FinanceIds) {
                        $name = "adaptive-zoom-G-$id-edit.png"
                        Assert-Unlocked '情境 G 截圖前'
                        $shot = if ($hwnds[$id] -eq [IntPtr]::Zero) { [PSCustomObject]@{ Ok = $false; Reason = '視窗未出現' } }
                        else { Save-WindowShot $hwnds[$id] (Join-Path $OutDir $name) }
                        Log "  截圖 $name：$(if ($shot.Ok) { 'OK' } else { "失敗（$($shot.Reason)）" })"
                        Set-Result "G：截圖 $name 為本次 PrintWindow 新檔（編輯版面字級控制外觀，需目視）" $shot.Ok
                    }
                    # 8. 離開編輯版面：控制移除、再呼叫被拒、字級維持 1.1。
                    $ex = Invoke-Json 'w=clock' (Get-InvokeExpr 'set_edit_mode' '{ enabled: false }')
                    Start-Sleep -Milliseconds 1500
                    $c8 = Invoke-Json "w=$id0" $FontControlExpr
                    $r3 = Invoke-Json "w=$id0" (Get-InvokeExpr 'adjust_widget_font_scale' '{ step: 1 }')
                    $s3 = Get-FontStates @($id0)
                    Log "  G8 set_edit_mode(false) ok=$($ex.ok)$($ex.err)$($ex.__error)；總經字級控制 present=$($c8.present)；再 adjust ok=$($r3.ok) err=$($r3.err)$($r3.__error)；$(Format-FontStates $s3)"
                    Set-Result 'G：離開編輯版面後字級控制移除' ($ex.ok -eq $true -and $c8.present -eq $false)
                    Set-Result 'G：離開編輯版面後 adjust_widget_font_scale 又被拒' (Test-Rejected $r3)
                    Set-ProblemResult 'G：離開編輯版面後總經字級維持 1.1' (Get-FontStateProblems $s3 ([ordered]@{ macro = 1.1 }))
                }
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
        Log '# 倍率表（情境｜小工具｜實際矩形｜邏輯尺寸｜字級｜預期｜實際｜上限｜字級 1 時｜相符）'
        foreach ($z in $zoomTable) { Log ("#   {0}|{1}|{2}|{3}|{4}|{5:N4}|{6:N4}|{7:N4}|{8:N4}|{9}" -f $z.Scenario, $z.Id, $z.Rect, $z.Logical, $z.Font, $z.Expected, $z.Actual, $z.Cap, $z.ZoomAt1, $z.Ok) }
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
