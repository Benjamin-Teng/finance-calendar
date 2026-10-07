<#
.SYNOPSIS
    Task 6.1 驗收驅動腳本（外觀模式與點穿）：補 scenario-matrix.md 兩條「無證據」——
    specs/widget-host-windows「小工具外觀模式／切換為純色模式」與
    「面板以外的區域不攔截滑鼠／點擊小工具旁的桌面圖示」（本腳本只驗其中「視窗層不攔截
    點擊」這一半，真正的「桌面圖示被選取」需要合成輸入，見 human-checklist.md A 節新增項）。
    只讀視窗狀態、透過 CDP 呼叫宿主自己的 IPC 指令與讀取頁面 DOM／computed style，**不注入
    任何輸入**（不送按鍵、不點擊、不截圖），鎖定時也可跑。唯一會動到的使用者視窗：點穿取樣點被
    一般應用程式主視窗蓋住時，經 lib/Occluders.psm1 白名單暫時最小化，取樣完與 finally 照原位置還原（fix F9）。

.DESCRIPTION
    步驟：
      1. 確認沒有 fc-host 在跑。
      2. 暫存 %APPDATA%（首次啟動）／%LOCALAPPDATA%（資料目錄），fixture 用既有
         `host/ui/fixtures/tw-events.json`（quotes 非空，五個財經小工具啟動起就有內容），
         比照 verify-3.1.ps1／verify-3.2.ps1。
      3. 啟動宿主（CDP 除錯埠），等五個財經小工具視窗出現。
      4. 對每個小工具：
         a. 外觀（design.md D8、spec「小工具外觀模式」）：`DwmGetWindowAttribute(hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE=38)` 讀基準值——`apply_appearance`（host/src/desktop.rs）
            對 Acrylic／Solid 兩個分支都不呼叫任何 DWM API，故基準值必為 Windows 對「從未設定
            過這個屬性」的預設值：`DWMSBT_AUTO`(0) 或 `DWMSBT_NONE`(1)（Microsoft Learn
            `DWM_SYSTEMBACKDROP_TYPE` 列舉：AUTO 為 the default；不會是 MAINWINDOW(2)／
            TRANSIENTWINDOW(3，即 Acrylic）／TABBEDWINDOW(4)，那些只有明確呼叫
            `DwmSetWindowAttribute` 設定過才會出現）。
         b. 尺寸（design D7 格線版面的迴歸確認，task 7.7 起）：`GetWindowRect` 逐像素＝預設格座標
            （host/src/settings.rs `DEFAULT_GRID_RECTS`）在主螢幕工作區換算的格線矩形，client＝
            整個視窗（原本「高度＝內容夾限最大高度」的判準已隨 task 7.3 失效）。
         c. 點穿（spec「面板以外的區域不攔截滑鼠」）：`GetWindowRect` 四邊中點各向外 1
            實體像素呼叫 `WindowFromPoint`：命中桌面（本身或 GA_ROOT 祖先的類別為 Progman／
            WorkerW／SHELLDLL_DefView／SysListView32）才算 PASS、命中宿主＝FAIL（fix F4，review
            6.1-verify medium——原本「不是宿主」就 PASS，整片被瀏覽器蓋住時 20/20 空洞通過）。
            fix F9：取樣前先經 `Clear-Occluders`（目標＝殼層桌面視窗）清遮擋——被一般應用程式主視窗
            蓋住就暫時最小化；判讀經 `Resolve-OutsideSample`（lib `Assert-OccluderResult
            -TargetIsDesktop`）：系統 UI／工作列／最小化後仍蓋著＝BLOCKED（該點不提供點穿資訊），
            宿主視窗蓋住、沒命中任何視窗＝FAIL。有 BLOCKED 而無 FAIL 時結束碼 3。
            面板內部中心點作對照組，應命中宿主行程本身。
      5. 外觀模式切換：以 CDP 對 clock 頁面呼叫 `update_settings({ appearance_mode: 'acrylic'
         })`，等 `refresh_all_widget_appearance`（`update_settings` 內無條件呼叫
         `reapply_power_pause_rules` → `refresh_all_widget_appearance`）跑過一輪後，重讀每個
         小工具的 DWM 屬性（應不變，仍是 AUTO/NONE）與面板 computed background（spec「純色＝
         半透明深色」：alpha 嚴格介於 0 與 1 且等於設定的 opacity（首次啟動預設 0.55，±0.01）、
         R／G／B 各 ≤ 64（--panel-rgb 13,19,32））；再切回 `solid`，重複同一組斷言。`update_settings` 回傳的 appearance_mode
         必須等於要求的值，「切換後」斷言以切換成功為前提（fix F4，review 6.1-verify low×2）。
      6. settings.html DOM（design.md D8「毛玻璃選項在設定介面標示為不可用」；task 5.2 既有
         實作）：以手動重複啟動觸發既有執行個體開設定視窗（比照 verify-5.2.ps1），CDP 讀
         `#appearance-acrylic` 的 `disabled === true`，以及 `.hint` 段落文字含「毛玻璃」與
         「無法呈現模糊效果」。
      7. 不截圖（理由同 verify-3.1.ps1：小工具在最底層，截到的是蓋在上面的一般視窗）。
      8. 結束宿主、刪除暫存目錄。

.PARAMETER Exe
    fc-host.exe 路徑，預設 host/target/release/fc-host.exe。

.PARAMETER OutDir
    證據輸出目錄，預設 host/tools/evidence。

.PARAMETER CdpPort
    WebView2 remote debugging 埠，預設 9340（與既有 verify-3.1/3.2/4.6/4.7/5.2/5.6/5.8 的
    9333/9334/9336/9337/9339/…分開）。
#>
[CmdletBinding()]
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\fc-host.exe'),
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [int]$CdpPort = 9340
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\AutostartRegistry.psm1') -Force
# fix F9：點穿取樣點被使用者的一般視窗蓋住時，經 lib 白名單暫時最小化、finally 還原（不注入任何輸入）。
Import-Module (Join-Path $PSScriptRoot 'lib\Occluders.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\VerifyVerdict.psm1') -Force

Add-Type -Namespace V61 -Name Native -MemberDefinition @'
[DllImport("user32.dll")] public static extern System.IntPtr SetThreadDpiAwarenessContext(System.IntPtr ctx);
[DllImport("user32.dll")] public static extern System.IntPtr GetShellWindow();
[DllImport("user32.dll")] public static extern System.IntPtr GetTopWindow(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern System.IntPtr GetWindow(System.IntPtr hWnd, uint cmd);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr hWnd, out uint pid);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(System.IntPtr hWnd, System.Text.StringBuilder s, int n);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassName(System.IntPtr hWnd, System.Text.StringBuilder s, int n);
[DllImport("user32.dll")] public static extern bool GetWindowRect(System.IntPtr hWnd, out RECT r);
[DllImport("user32.dll")] public static extern bool GetClientRect(System.IntPtr hWnd, out RECT r);
[DllImport("user32.dll")] public static extern uint GetDpiForWindow(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern System.IntPtr WindowFromPoint(POINT pt);
[DllImport("user32.dll")] public static extern System.IntPtr GetAncestor(System.IntPtr hWnd, uint flags);
[DllImport("user32.dll")] public static extern long GetWindowLongPtrW(System.IntPtr hWnd, int idx);
[DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(System.IntPtr hwnd, int dwAttribute, out int pvAttribute, int cbAttribute);
[StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
[StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
[StructLayout(LayoutKind.Sequential)] public struct MONITORINFO { public int cbSize; public RECT rcMonitor; public RECT rcWork; public uint dwFlags; }
[DllImport("user32.dll")] public static extern System.IntPtr MonitorFromPoint(POINT pt, uint flags);
[DllImport("user32.dll")] public static extern bool GetMonitorInfoW(System.IntPtr hMon, ref MONITORINFO mi);
'@

# 本執行緒改為 Per-Monitor-V2（-4），GetWindowRect／WindowFromPoint 才會回實體像素、
# 與 verify-3.1.ps1/verify-3.2.ps1 一致的座標系。
[void][V61.Native]::SetThreadDpiAwarenessContext([IntPtr](-4))

# DWMWA_SYSTEMBACKDROP_TYPE：Microsoft Learn `DWMWINDOWATTRIBUTE` 列舉頁只文字列出順序、
# 未逐一標數字，數值 38 取自 windows crate 常數定義（一手依據：
# ~/.cargo/registry/src/*/windows-0.62.2/src/Windows/Win32/Graphics/Dwm/mod.rs:237
# `pub const DWMWA_SYSTEMBACKDROP_TYPE: DWMWINDOWATTRIBUTE = DWMWINDOWATTRIBUTE(38i32);`，
# windows-sys 0.48/0.52/0.59/0.60/0.61 五個版本一致）。
# https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/ne-dwmapi-dwmwindowattribute
$DWMWA_SYSTEMBACKDROP_TYPE = 38
# DWM_SYSTEMBACKDROP_TYPE 列舉值（Microsoft Learn）：
# https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/ne-dwmapi-dwm_systembackdrop_type
# AUTO=0（預設，「Let DWM automatically decide…might also decide to draw no backdrop material
# at all」）、NONE=1（不畫）、MAINWINDOW=2（Mica）、TRANSIENTWINDOW=3（Acrylic）、
# TABBEDWINDOW=4（Mica Alt）。本專案的 apply_appearance 從未呼叫 DwmSetWindowAttribute，
# 故只有 AUTO／NONE 兩者才算「無背景材質」的基準；出現 2/3/4 代表有東西幫我們設定了材質，
# 應判 FAIL。
$NO_BACKDROP_VALUES = @(0, 1)

# WS_EX_TRANSPARENT（GWL_EXSTYLE 的其中一個旗標位元）：若小工具視窗本身被設了這個旗標，
# 整扇視窗（含面板內部）都會對滑鼠點穿到 z-order 中它下面的視窗——這才是「面板以外的區域
# 不攔截滑鼠」這條 spec 真正要避免的相反面（面板*應該*攔截、只有面板以外才不該攔截）。
# design.md D8／widgets.rs create_widget_window 從未設定這個旗標（只設
# focusable(false)/decorations(false)/transparent(true) 這些會轉譯成 WS_EX_NOACTIVATE 等，
# 不含 WS_EX_TRANSPARENT），故靜態檢查其「不存在」比空間座標點穿測試更可靠——後者會被使用者
# 桌面上剛好蓋在該座標的一般視窗（例如目前 4K 外接螢幕整片被瀏覽器視窗佔滿，design.md D8
# 「Acrylic」探針已記錄過同一台機器同一個現象）混淆，無法單靠命中結果分辨「有一般視窗合理
# 蓋住」與「小工具本身點穿」兩種情況。
$WS_EX_TRANSPARENT = 0x20
$GWL_EXSTYLE = -20

# 五個財經小工具＝首次啟動即有內容的預設集合（host/src/settings.rs default_widgets：
# clock/macro/fixed/dynamic/quotes enabled=true，custom1-5 enabled=false）；
# 預設格座標取自 host/src/settings.rs `DEFAULT_GRID_RECTS`（col,row,w,h，48×48 格線）。
$WidgetIds = @('clock', 'macro', 'fixed', 'dynamic', 'quotes')
$Grid = @{ clock = @(15, 1, 16, 10); macro = @(15, 12, 16, 30); fixed = @(32, 1, 15, 17); dynamic = @(32, 19, 15, 23); quotes = @(15, 43, 32, 4) }
function Edge([int]$Origin, [int]$Extent, [int]$I) { $Origin + [int][math]::Floor([int64]$I * $Extent / 48) }

function Get-Ts { Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fffK' }
function Test-Locked { [bool](Get-Process -Name LogonUI, LockApp -ErrorAction SilentlyContinue) }

# ---------------------------------------------------------------- 純函式（tests/Verify61.Tests.ps1 以 Parser 取出測試）

function Get-OutsideHitVerdict {
    <#
    fix F4（review 6.1-verify medium）：小工具邊外 1px 的 WindowFromPoint 結果判定。
      - 命中宿主行程 → FAIL（面板以外的區域攔截了點擊）；
      - 命中桌面（本身或頂層祖先的類別屬於 Progman／WorkerW／SHELLDLL_DefView／SysListView32）→ PASS；
      - 其他（被一般視窗覆蓋、或沒命中任何視窗）→ BLOCKED：這一點不提供點穿資訊，不得記成 PASS。
    #>
    param([int]$HitPid, [int]$HostPid, [string]$HitClass, [string]$RootClass, [bool]$HitIsNull = $false)
    $DesktopClasses = @('Progman', 'WorkerW', 'SHELLDLL_DefView', 'SysListView32')
    if ($HitIsNull) { return 'BLOCKED' }
    if ($HitPid -eq $HostPid) { return 'FAIL' }
    if (($DesktopClasses -contains $HitClass) -or ($DesktopClasses -contains $RootClass)) { return 'PASS' }
    return 'BLOCKED'
}

function Get-AggregateVerdict([string[]]$Verdicts) {
    <# 任一 FAIL → FAIL；否則任一 BLOCKED（或沒有任何取樣）→ BLOCKED；全部 PASS → PASS。 #>
    $v = @($Verdicts)
    if ($v.Count -eq 0) { return 'BLOCKED' }
    if ($v -contains 'FAIL') { return 'FAIL' }
    if ($v -contains 'BLOCKED') { return 'BLOCKED' }
    return 'PASS'
}

function Resolve-OutsideSample {
    <#
    fix F9：一個點穿取樣點的判讀。Occ＝lib/Occluders 的 Clear-Occluders 結果（呼叫端以 -HostPid＝宿主、
    -TargetHwnd＝殼層桌面視窗清遮擋：被一般應用程式主視窗蓋住已經白名單暫時最小化），判讀一律經 lib 的
    Assert-OccluderResult -TargetIsDesktop（目標本來就是桌面）：
      - 環境（工作列、系統 UI、最小化後仍蓋著）→ BLOCKED（該點不提供點穿資訊）；
      - 宿主視窗蓋住邊外點、沒命中任何視窗 → FAIL；
      - 到達桌面 → 以 Observe 重新取實際命中，交給 Get-OutsideHitVerdict（命中宿主＝FAIL、桌面＝PASS）。
    回傳 @{ Verdict = 'PASS'|'FAIL'|'BLOCKED'; Note; Hit }。
    #>
    param($Occ, [Parameter(Mandatory)][int]$HostPid, [Parameter(Mandatory)][scriptblock]$Observe, [string]$What = '點穿取樣點')
    try { Assert-OccluderResult -Result $Occ -What $What -TargetIsDesktop }
    catch {
        if ("$_" -like 'ENV-BLOCKED*') { return [PSCustomObject]@{ Verdict = 'BLOCKED'; Note = "$_"; Hit = $null } }
        if ("$_" -like 'FAIL:*') { return [PSCustomObject]@{ Verdict = 'FAIL'; Note = "$_"; Hit = $null } }
        throw
    }
    $o = & $Observe
    $v = Get-OutsideHitVerdict -HitPid ([int]$o.HitPid) -HostPid $HostPid -HitClass ([string]$o.HitClass) -RootClass ([string]$o.RootClass) -HitIsNull ([bool]$o.HitIsNull)
    return [PSCustomObject]@{ Verdict = $v; Note = ''; Hit = $o }
}

function Test-UpdateSettingsResult([string]$Raw, [string]$Mode) {
    <# update_settings 回傳（CDP eval 的 JSON 文字）解析後必須等於要求的 appearance_mode（review 6.1-verify low）。 #>
    try { $v = $Raw | ConvertFrom-Json } catch { return $false }
    return ($v -is [string]) -and ($v -eq $Mode)
}

function Test-TranslucentDarkBackground {
    <#
    fix F4（review 6.1-verify low）：spec「純色＝半透明深色、無模糊」。判準：
      - 半透明：alpha 嚴格介於 0 與 1 之間（0＝全透明＝沒有面板；1 或沒有 alpha 的 rgb()＝不透明），
        且等於設定的 opacity（ExpectedAlpha，±0.01）——本腳本以首次啟動預設值跑，opacity＝0.55
        （widget.css --panel-o、settings.rs Settings::default），幾乎全透明的 0.01 也會被擋下。
      - 深色：R、G、B 皆 ≤ 64（--panel-rgb＝13, 19, 32）。
    回傳 @{ Ok; Alpha; Rgb; Reason }。
    #>
    param([string]$Css, [double]$ExpectedAlpha = 0.55)
    $s = "$Css".Trim().Trim('"')
    if ($s -notmatch '^rgba?\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*(?:,\s*([0-9.]+)\s*)?\)$') {
        return [PSCustomObject]@{ Ok = $false; Alpha = $null; Rgb = $null; Reason = "無法解析背景色：$s" }
    }
    $r = [int]$Matches[1]; $g = [int]$Matches[2]; $b = [int]$Matches[3]
    $a = if ($Matches[4]) { [double]::Parse($Matches[4], [Globalization.CultureInfo]::InvariantCulture) } else { 1.0 }
    $translucent = ($a -gt 0) -and ($a -lt 1) -and ([Math]::Abs($a - $ExpectedAlpha) -le 0.01)
    $dark = ($r -le 64) -and ($g -le 64) -and ($b -le 64)
    $why = @()
    if (-not $translucent) { $why += "alpha=$a 不在 (0,1) 或不等於設定的 opacity $ExpectedAlpha（±0.01）" }
    if (-not $dark) { $why += "RGB=($r,$g,$b) 不是深色（各通道須 ≤ 64）" }
    return [PSCustomObject]@{ Ok = ($translucent -and $dark); Alpha = $a; Rgb = "$r,$g,$b"; Reason = $(if ($why) { $why -join '；' } else { "半透明深色 alpha=$a RGB=($r,$g,$b)" }) }
}

function Get-ClassNameOf([IntPtr]$Hwnd) {
    if ($Hwnd -eq [IntPtr]::Zero) { return '' }
    $sb = New-Object System.Text.StringBuilder 256
    [void][V61.Native]::GetClassName($Hwnd, $sb, 256)
    return $sb.ToString()
}

function Get-HostWindows([int]$ProcId) {
    $list = @()
    $h = [V61.Native]::GetTopWindow([IntPtr]::Zero)
    while ($h -ne [IntPtr]::Zero) {
        $p = 0
        [void][V61.Native]::GetWindowThreadProcessId($h, [ref]$p)
        if ($p -eq $ProcId -and [V61.Native]::IsWindowVisible($h)) {
            $sb = New-Object System.Text.StringBuilder 256
            [void][V61.Native]::GetWindowText($h, $sb, 256)
            $r = New-Object V61.Native+RECT
            [void][V61.Native]::GetWindowRect($h, [ref]$r)
            $c = New-Object V61.Native+RECT
            [void][V61.Native]::GetClientRect($h, [ref]$c)
            $list += [PSCustomObject]@{
                Hwnd = $h; Title = $sb.ToString()
                Left = $r.Left; Top = $r.Top; Right = $r.Right; Bottom = $r.Bottom
                ClientW = $c.Right; ClientH = $c.Bottom
                Dpi = [V61.Native]::GetDpiForWindow($h)
            }
        }
        $h = [V61.Native]::GetWindow($h, 2)
    }
    return $list
}

function Get-WindowDescription([IntPtr]$Hwnd) {
    if ($Hwnd -eq [IntPtr]::Zero) { return '(none)' }
    $sb = New-Object System.Text.StringBuilder 256
    [void][V61.Native]::GetClassName($Hwnd, $sb, 256)
    $cls = $sb.ToString()
    $tb = New-Object System.Text.StringBuilder 256
    [void][V61.Native]::GetWindowText($Hwnd, $tb, 256)
    $p = 0
    [void][V61.Native]::GetWindowThreadProcessId($Hwnd, [ref]$p)
    return "hwnd=0x$($Hwnd.ToInt64().ToString('X')) class=$cls title='$($tb.ToString())' pid=$p"
}

function Get-BackdropType([IntPtr]$Hwnd) {
    $val = 0
    $hr = [V61.Native]::DwmGetWindowAttribute($Hwnd, $DWMWA_SYSTEMBACKDROP_TYPE, [ref]$val, 4)
    return [PSCustomObject]@{ Hr = $hr; Value = $val }
}

function Invoke-PageEval([string]$UrlPart, [string]$Expr, [int]$TimeoutSec = 60) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $out = $null
    while ($true) {
        $out = (& node (Join-Path $PSScriptRoot 'host-cdp-eval.mjs') $CdpPort $UrlPart $Expr 2>&1) -join ''
        if ($out -notmatch '找不到 url 含') { return $out }
        if ($sw.Elapsed.TotalSeconds -ge $TimeoutSec) { return $out }
        Start-Sleep -Milliseconds 400
    }
}

function Invoke-WidgetEval([string]$WidgetId, [string]$Expr) { Invoke-PageEval "w=$WidgetId" $Expr }
function Invoke-SettingsEval([string]$Expr) { Invoke-PageEval 'settings.html' $Expr }

function Wait-WindowTitles([int]$ProcId, [scriptblock]$Cond, [int]$TimeoutSec = 20) {
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
$logPath = Join-Path $OutDir '6.1-appearance-hittest.log'
$log = New-EvidenceWriter $logPath
$log.AutoFlush = $true
$log.WriteLine("# verify-6.1-appearance-hittest.ps1 start=$(Get-Ts) exe=$Exe locked=$([int](Test-Locked))")
$log.WriteLine('# DWMWA_SYSTEMBACKDROP_TYPE=38（windows crate 0.62.2/mod.rs:237 一手依據），')
$log.WriteLine('# DWM_SYSTEMBACKDROP_TYPE：AUTO=0(預設) NONE=1 MAINWINDOW=2(Mica) TRANSIENTWINDOW=3(Acrylic) TABBEDWINDOW=4(MicaAlt)')
$log.WriteLine('# https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/ne-dwmapi-dwmwindowattribute')
$log.WriteLine('# https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/ne-dwmapi-dwm_systembackdrop_type')

$regSnap = @(Save-FcHostAutostartRegistry)
$log.WriteLine("# $(Get-Ts) 開機自啟登錄快照：$(($regSnap | ForEach-Object { "$($_.Name)@$(Split-Path $_.Key -Leaf) Exists=$($_.Exists)" }) -join '; ')")
$results = [ordered]@{}
# 取樣不提供資訊的項目（例如點穿取樣點被一般視窗覆蓋）：不記 PASS，結束碼 3（ENV-BLOCKED）。
$blockedItems = New-Object System.Collections.Generic.List[string]
# fix F9：點穿取樣點清遮擋時被暫時最小化的使用者視窗（lib/Occluders），取樣完與 finally 都照原位置還原。
$minimized = New-Object System.Collections.Generic.List[object]
# hung-target（review 642050f）：沒還原成功的使用者視窗（無回應、最小化未確認、讀回逾時／不符），寫進摘要。
$occNotRestored = New-Object System.Collections.Generic.List[string]
# 點穿取樣點的目標＝殼層桌面視窗（GetShellWindow，通常是 Progman）；命中其他 Progman／WorkerW 也算桌面
# （見 lib Get-OccluderVerdict -TargetIsDesktop）。
$desktopTop = [V61.Native]::GetShellWindow()

# ── 2. 暫存 %APPDATA%／%LOCALAPPDATA%＋資料目錄 fixture ─────────────────────────────
$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ("fc-host-6.1-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
$tempAppData = Join-Path $tempRoot 'Roaming'
$tempLocalAppData = Join-Path $tempRoot 'Local'
New-Item -ItemType Directory -Force -Path $tempAppData, $tempLocalAppData | Out-Null
$dataDir = Join-Path $tempLocalAppData 'tw.fintools.fc-host\data'
New-Item -ItemType Directory -Force -Path $dataDir | Out-Null
Copy-Item (Join-Path $PSScriptRoot '..\ui\fixtures\tw-events.json') (Join-Path $dataDir 'tw_events.json')
$log.WriteLine("# APPDATA=$tempAppData（無 settings.json＝首次啟動）LOCALAPPDATA=$tempLocalAppData（資料目錄=$dataDir，fixture=tw-events.json，quotes 非空）")

$oldAppData = $env:APPDATA
$oldLocalAppData = $env:LOCALAPPDATA
$oldWv2 = $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS
$hostProc = $null

function Open-SettingsWindowViaManualRelaunch([int]$ProcId) {
    # 手動重複啟動（無參數）：既有執行個體收到 single-instance 回呼會開啟設定視窗，比照
    # verify-5.2.ps1 `Open-SettingsWindowViaManualRelaunch`——不是輸入注入，是另一個行程正常
    # 結束。
    $env:APPDATA = $script:tempAppData
    $env:LOCALAPPDATA = $script:tempLocalAppData
    $p2 = $null
    try {
        $p2 = Start-Process -FilePath $script:Exe -PassThru
    } finally {
        $env:APPDATA = $script:oldAppData
        $env:LOCALAPPDATA = $script:oldLocalAppData
    }
    $exited = $p2.WaitForExit(15000)
    if (-not $exited) {
        $script:log.WriteLine("## $(Get-Ts) 警告：第二個行程（pid=$($p2.Id)）15 秒內未自行結束，強制關閉")
        Stop-Process -Id $p2.Id -Force -ErrorAction SilentlyContinue
    }
    return Wait-WindowTitles $ProcId { param($w) [bool]($w | Where-Object { $_.Title -eq '財經桌布設定' }) } 15
}

try {
    # ── 3. 啟動宿主 ────────────────────────────────────────────────────────────────
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
    $log.WriteLine("# $(Get-Ts) 宿主 pid=$hostPid")

    $ok = Wait-WindowTitles $hostPid { param($w) ($w | Where-Object { $_.Title -like 'fc-host *' }).Count -ge 5 }
    Start-Sleep -Seconds 3
    $wins = Get-HostWindows $hostPid
    $results['五個財經小工具出現'] = $ok -and (@($wins | Where-Object { $_.Title -like 'fc-host *' }).Count -eq 5)
    $log.WriteLine("## $(Get-Ts) 首次啟動：視窗清單＝$(($wins | ForEach-Object { $_.Title }) -join ', ')")

    # ── 4. 逐一小工具：外觀基準／尺寸／點穿 ───────────────────────────────────────────
    $widgetHwnd = @{}
    $allBackdropOk = $true
    $allHeightOk = $true
    $outsideVerdicts = New-Object System.Collections.Generic.List[string]
    $allInsideOk = $true
    foreach ($id in $WidgetIds) {
        $w = $wins | Where-Object { $_.Title -eq "fc-host $id" } | Select-Object -First 1
        if (-not $w) {
            $log.WriteLine("  $id：找不到視窗，全部斷言 FAIL")
            $allBackdropOk = $false; $allHeightOk = $false; $outsideVerdicts.Add('FAIL'); $allInsideOk = $false
            continue
        }
        $widgetHwnd[$id] = $w.Hwnd

        # 4a. 外觀基準
        $bd = Get-BackdropType $w.Hwnd
        $bdOk = ($bd.Hr -eq 0) -and ($NO_BACKDROP_VALUES -contains $bd.Value)
        if (-not $bdOk) { $allBackdropOk = $false }
        $log.WriteLine("  $id backdrop 基準：hr=$($bd.Hr) value=$($bd.Value) → $(if ($bdOk) { 'PASS' } else { 'FAIL' })")

        # 4b. 尺寸＝格子（格線版面迴歸確認）
        $pt0 = New-Object V61.Native+POINT
        $mi = New-Object V61.Native+MONITORINFO
        $mi.cbSize = [Runtime.InteropServices.Marshal]::SizeOf($mi)
        [void][V61.Native]::GetMonitorInfoW([V61.Native]::MonitorFromPoint($pt0, 1), [ref]$mi)
        $wa = $mi.rcWork; $g = $Grid[$id]
        $waW = $wa.Right - $wa.Left; $waH = $wa.Bottom - $wa.Top
        $ex0 = Edge $wa.Left $waW $g[0]; $ex1 = Edge $wa.Left $waW ($g[0] + $g[2])
        $ey0 = Edge $wa.Top $waH $g[1]; $ey1 = Edge $wa.Top $waH ($g[1] + $g[3])
        $hOk = ($w.Left -eq $ex0 -and $w.Right -eq $ex1 -and $w.Top -eq $ey0 -and $w.Bottom -eq $ey1 -and $w.ClientH -eq ($w.Bottom - $w.Top))
        if (-not $hOk) { $allHeightOk = $false }
        $log.WriteLine("  $id 尺寸：預設格座標=$($g -join ',') 期望=($ex0,$ey0)-($ex1,$ey1) 實際=($($w.Left),$($w.Top))-($($w.Right),$($w.Bottom)) ClientH=$($w.ClientH) → $(if ($hOk) { 'PASS' } else { 'FAIL' })")

        # 4c. 點穿：四邊外 1 實體像素 ── WindowFromPoint 不應命中宿主
        $midX = [int](($w.Left + $w.Right) / 2)
        $midY = [int](($w.Top + $w.Bottom) / 2)
        $outsidePoints = [ordered]@{
            top    = @{ X = $midX; Y = $w.Top - 1 }
            bottom = @{ X = $midX; Y = $w.Bottom + 1 }
            left   = @{ X = $w.Left - 1; Y = $midY }
            right  = @{ X = $w.Right + 1; Y = $midY }
        }
        foreach ($edge in $outsidePoints.Keys) {
            $px = [int]$outsidePoints[$edge].X
            $py = [int]$outsidePoints[$edge].Y
            # fix F9：先經 lib/Occluders 清遮擋（目標＝殼層桌面視窗；被一般應用程式主視窗蓋住就經白名單暫時
            # 最小化，finally 還原），判讀經 Resolve-OutsideSample（Assert-OccluderResult -TargetIsDesktop）。
            $occ = Clear-Occluders -HostPid $hostPid -TargetHwnd $desktopTop -Points @(, @($px, $py)) -Minimized $minimized `
                -Log { param($m) $log.WriteLine("  $m") }
            $sample = Resolve-OutsideSample -Occ $occ -HostPid $hostPid -What "$id 邊外 1px（$edge，點=($px,$py)）" -Observe {
                $pt = New-Object V61.Native+POINT
                $pt.X = $px; $pt.Y = $py
                $hit = [V61.Native]::WindowFromPoint($pt)
                $hitPid = 0
                if ($hit -ne [IntPtr]::Zero) { [void][V61.Native]::GetWindowThreadProcessId($hit, [ref]$hitPid) }
                [PSCustomObject]@{
                    HitPid = [int]$hitPid; HitClass = (Get-ClassNameOf $hit); RootClass = (Get-ClassNameOf ([V61.Native]::GetAncestor($hit, 2)))  # GA_ROOT=2
                    HitIsNull = ($hit -eq [IntPtr]::Zero); Desc = (Get-WindowDescription $hit)
                }
            }
            $ev = $sample.Verdict
            $outsideVerdicts.Add($ev)
            $note = switch ($ev) { 'PASS' { 'PASS（命中桌面）' } 'FAIL' { 'FAIL（命中宿主或沒命中，未點穿）' } default { 'BLOCKED（該點被系統 UI／工作列擋住或清不掉，不提供點穿資訊）' } }
            $hitTxt = if ($sample.Hit) { "命中 $($sample.Hit.Desc) root=$($sample.Hit.RootClass)" } else { "遮擋判讀：$($sample.Note)" }
            $log.WriteLine("  $id 邊外 1px（$edge，點=($px,$py)）：$hitTxt → $note")
        }

        # 4d. 對照組：面板內部中心點「應該」命中宿主本身——但這只有在該座標當下沒有任何
        # 一般視窗蓋在小工具之上時才能直接由 WindowFromPoint 觀察到（小工具設計上永遠在
        # z-order 底層，正上方若剛好有一般視窗蓋住該像素，WindowFromPoint 回傳的會是那個
        # 一般視窗，這是預期行為、不是點穿；且此結果與「小工具本身被設成點穿」在座標測試下
        # 無法區分，因為兩種情況下 WindowFromPoint 一樣不會回傳宿主）。故主要判準改用靜態
        # 旗標檢查：GWL_EXSTYLE 不含 WS_EX_TRANSPARENT（不存在才不會整扇視窗對滑鼠點穿），
        # WindowFromPoint 的座標命中結果只當附加診斷資訊記錄，命中一般視窗時不算違規。
        $exStyle = [V61.Native]::GetWindowLongPtrW($w.Hwnd, $GWL_EXSTYLE)
        $notTransparent = (([int64]$exStyle) -band $WS_EX_TRANSPARENT) -eq 0
        if (-not $notTransparent) { $allInsideOk = $false }

        $cpt = New-Object V61.Native+POINT
        $cpt.X = $midX
        $cpt.Y = $midY
        $insideHit = [V61.Native]::WindowFromPoint($cpt)
        $insidePid = 0
        if ($insideHit -ne [IntPtr]::Zero) { [void][V61.Native]::GetWindowThreadProcessId($insideHit, [ref]$insidePid) }
        $insideDesc = Get-WindowDescription $insideHit
        $insideNote = if ($insidePid -eq $hostPid) { '座標命中＝宿主（此刻無一般視窗遮蔽，直接印證）' }
                      elseif ($insideHit -eq [IntPtr]::Zero) { '座標命中＝無視窗（異常，記錄供人工檢查）' }
                      else { '座標命中＝其他一般視窗（該像素目前被使用者視窗合理遮蔽，非點穿，見上方旗標判準）' }
        $log.WriteLine("  $id 內部中心點（($($cpt.X),$($cpt.Y))）：GWL_EXSTYLE=0x$($exStyle.ToString('X')) WS_EX_TRANSPARENT未設定=$notTransparent → $(if ($notTransparent) { 'PASS' } else { 'FAIL（視窗本身點穿）' })；座標診斷：命中 $insideDesc（$insideNote）")
    }
    # fix F9：取樣完立刻還原被暫時最小化的使用者視窗（finally 再保險一次；清單已清空時是 no-op）。
    Restore-Occluders -Minimized $minimized -NotRestored $occNotRestored -Log { param($m) $log.WriteLine("  $m") }
    $results['外觀基準：每個可見小工具 DWMWA_SYSTEMBACKDROP_TYPE＝AUTO(0)/NONE(1)，無背景材質'] = $allBackdropOk
    $results['尺寸迴歸：視窗矩形＝預設格子（逐像素）、client＝整個視窗'] = $allHeightOk
    $outsideAgg = Get-AggregateVerdict $outsideVerdicts.ToArray()
    $outsideKey = '點穿：四邊外 1px 皆命中桌面（Progman／WorkerW／SHELLDLL_DefView／SysListView32）'
    $log.WriteLine("  點穿彙總：$outsideAgg（PASS=$(@($outsideVerdicts | Where-Object { $_ -eq 'PASS' }).Count) FAIL=$(@($outsideVerdicts | Where-Object { $_ -eq 'FAIL' }).Count) BLOCKED=$(@($outsideVerdicts | Where-Object { $_ -eq 'BLOCKED' }).Count)）")
    if ($outsideAgg -eq 'BLOCKED') { $blockedItems.Add("$outsideKey（有取樣點被系統 UI／工作列擋住或清不掉，未取得點穿資訊）") }
    else { $results[$outsideKey] = ($outsideAgg -eq 'PASS') }
    $results['對照組：面板本身未設定 WS_EX_TRANSPARENT（內部不點穿；座標命中結果見 log 附註）'] = $allInsideOk

    # ── 5. 外觀模式切換（acrylic → solid，各驗一次）──────────────────────────────────
    function Test-AppearanceMode([string]$Mode) {
        $r = Invoke-WidgetEval 'clock' "window.__TAURI__.core.invoke('update_settings', { patch: { appearance_mode: '$Mode' } }).then(s => s.appearance_mode)"
        # fix F4（review 6.1-verify low）：切換本身要成功，「切換後」的斷言才有意義。
        $switched = Test-UpdateSettingsResult $r $Mode
        $log.WriteLine("## $(Get-Ts) update_settings appearance_mode=$Mode → $r（切換成功=$switched）")
        Start-Sleep -Milliseconds 800
        $bdOk = $true
        $bgOk = $true
        foreach ($id in $WidgetIds) {
            if (-not $widgetHwnd.ContainsKey($id)) { continue }
            $bd = Get-BackdropType $widgetHwnd[$id]
            $thisBdOk = ($bd.Hr -eq 0) -and ($NO_BACKDROP_VALUES -contains $bd.Value)
            if (-not $thisBdOk) { $bdOk = $false }
            $bg = Invoke-WidgetEval $id "(() => { const p = document.querySelector('.panel'); return p ? getComputedStyle(p).backgroundColor : null; })()"
            $bgCheck = Test-TranslucentDarkBackground $bg
            if (-not $bgCheck.Ok) { $bgOk = $false }
            $log.WriteLine("  mode=$Mode $id：backdrop value=$($bd.Value) 面板 background=$bg → backdrop $(if ($thisBdOk) { 'PASS' } else { 'FAIL' }) / 背景 $(if ($bgCheck.Ok) { 'PASS' } else { 'FAIL' })（$($bgCheck.Reason)）")
        }
        return [PSCustomObject]@{ Switched = $switched; BackdropOk = $bdOk; BackgroundOk = $bgOk }
    }

    # 「切換後」兩項以切換成功為前提（否則首次啟動的預設值就會讓它們空洞通過）。
    $acrylicResult = Test-AppearanceMode 'acrylic'
    $results['update_settings 切換為 acrylic 成功（回傳的 appearance_mode＝acrylic）'] = $acrylicResult.Switched
    $results['appearance_mode=acrylic 後：所有小工具 backdrop 仍無材質'] = $acrylicResult.Switched -and $acrylicResult.BackdropOk
    $results['appearance_mode=acrylic 後：所有小工具面板背景為半透明深色（alpha＝opacity 0.55±0.01、RGB 各 ≤ 64）'] = $acrylicResult.Switched -and $acrylicResult.BackgroundOk

    $solidResult = Test-AppearanceMode 'solid'
    $results['update_settings 切換為 solid 成功（回傳的 appearance_mode＝solid）'] = $solidResult.Switched
    $results['appearance_mode=solid 後：所有小工具 backdrop 仍無材質'] = $solidResult.Switched -and $solidResult.BackdropOk
    $results['appearance_mode=solid 後：所有小工具面板背景為半透明深色（alpha＝opacity 0.55±0.01、RGB 各 ≤ 64）'] = $solidResult.Switched -and $solidResult.BackgroundOk

    # ── 6. settings.html：毛玻璃選項 disabled＋說明文字 ──────────────────────────────
    $settingsOpened = Open-SettingsWindowViaManualRelaunch $hostPid
    $log.WriteLine("## $(Get-Ts) 設定視窗已開啟＝$settingsOpened")
    $results['手動重複啟動開啟設定視窗'] = $settingsOpened

    # settings.html 用 ES modules（`import`，隱含 defer）建構表單：CDP page target 註冊的
    # 時機（WebView2 導覽完成）不保證模組已跑完、DOM 已由 `buildAppearanceSection()` 掛上，
    # 故用輪詢直到讀到非 null 的 `disabled` 欄位（同 verify-5.2.ps1 `Wait-CssVarEquals` 的
    # 輪詢精神），不靠單次固定延遲賭時機。
    $domExpr = "(() => { const r = document.getElementById('appearance-acrylic'); const h = document.querySelector('.hint'); return { disabled: r ? r.disabled : null, hintText: h ? h.textContent : null }; })()"
    $dom = $null
    $domSw = [Diagnostics.Stopwatch]::StartNew()
    while ($domSw.Elapsed.TotalSeconds -lt 15) {
        $raw = Invoke-SettingsEval $domExpr
        try {
            $parsed = $raw | ConvertFrom-Json
            if ($null -ne $parsed.disabled) { $dom = $parsed; break }
        } catch {}
        Start-Sleep -Milliseconds 300
    }
    if (-not $dom) { $dom = [PSCustomObject]@{ disabled = $null; hintText = $null } }
    $log.WriteLine("## $(Get-Ts) settings.html DOM（輪詢 $([math]::Round($domSw.Elapsed.TotalSeconds,1))s）：disabled=$($dom.disabled) hintText='$($dom.hintText)'")
    $results['settings.html 毛玻璃選項 disabled=true'] = ($dom.disabled -eq $true)
    $results['settings.html 說明文字含「毛玻璃」與「無法呈現模糊效果」'] = ($dom.hintText -match '毛玻璃') -and ($dom.hintText -match '無法呈現模糊效果')

    $log.WriteLine("## 截圖：略過（理由同 verify-3.1.ps1；鎖定狀態=$([int](Test-Locked))）")
}
catch {
    $log.WriteLine("## $(Get-Ts) 例外中止：$($_.Exception.Message)")
    $results['腳本未跑完（見例外訊息）'] = $false
}
finally {
    if ($hostProc -and -not $hostProc.HasExited) { Stop-Process -Id $hostProc.Id -Force -ErrorAction SilentlyContinue }
    try { Restore-Occluders -Minimized $minimized -NotRestored $occNotRestored -Log { param($m) $log.WriteLine("# $(Get-Ts) $m") } } catch { $log.WriteLine("# 還原被最小化的視窗失敗：$_") }
    # $minimized 非空＝Restore-Occluders 本身中途丟例外、沒跑完；一併列為未還原（環境，不進逐項結果）。
    if ($minimized.Count -gt 0) { $occNotRestored.Add("Restore-Occluders 未跑完：尚有 $($minimized.Count) 扇未處理（見記錄）") }
    $tempLeaf = Split-Path $tempRoot -Leaf
    Start-Sleep -Milliseconds 500
    $orphans = Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" -ErrorAction SilentlyContinue |
        Where-Object { $_.CommandLine -and $_.CommandLine -match [regex]::Escape($tempLeaf) }
    foreach ($o in $orphans) { Stop-Process -Id $o.ProcessId -Force -ErrorAction SilentlyContinue }
    $regLeft = @(Restore-FcHostAutostartRegistry $regSnap)
    if ($regLeft.Count -gt 0) { Write-Warning "開機自啟登錄未還原：$($regLeft -join '; ')" }
    $log.WriteLine("# $(Get-Ts) 開機自啟登錄還原：未還原 $($regLeft.Count) 項$(if ($regLeft.Count) { '：' + ($regLeft -join '; ') })")
    $results['開機自啟登錄已還原（Run／StartupApproved 的 fc-host）'] = ($regLeft.Count -eq 0)
    $log.WriteLine("# $(Get-Ts) 宿主已結束（清掉 $($orphans.Count) 個殘留 msedgewebview2 子行程）")
}

$log.WriteLine('# ── summary ──')
foreach ($k in $results.Keys) { $log.WriteLine("$(if ($results[$k]) { 'PASS' } else { 'FAIL' })  $k") }
$occWarn = Format-UnrestoredWarning -NotRestored $occNotRestored
foreach ($b in $blockedItems) { $log.WriteLine("BLOCKED  $b") }
if ($occWarn) { $log.WriteLine($occWarn) }
$log.Close()
Get-Content $logPath | Select-Object -Last ($results.Count + $blockedItems.Count + 1)
Remove-Item -Recurse -Force $tempRoot -ErrorAction SilentlyContinue

# 結束碼：有 FAIL＝1；沒有 FAIL 但有 BLOCKED＝3（ENV-BLOCKED，不得當成 PASS）；全部 PASS＝0。
if ($occWarn) { Write-Host $occWarn -ForegroundColor Yellow }
exit (Get-VerdictExitCode -Results $results -EnvBlocked:($blockedItems.Count -gt 0) -NotRestored $occNotRestored)
