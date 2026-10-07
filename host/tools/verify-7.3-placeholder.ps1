<#
.SYNOPSIS
    Task 7.3／3.2 驗收驅動腳本（真實滑鼠拖曳；design.md D7「無內容」「編輯版面」、D4 `get_edit_mode`、
    D12「WebView2 故障」；fix F1 報告「需要實機才能驗的項目」3–5）。

.DESCRIPTION
    涵蓋（與 host/tools/README.md 該節一致）：
      - tasks 3.2：無內容（fixture `quotes: []`）時 quotes 隱藏；編輯版面期間以佔位外框顯示；離開編輯版面
        後再度隱藏；有資料後在原格子出現（中途改寫隔離資料目錄的 fixture，等宿主輪詢，矩形＝記錄的格子）。
      - F1 項目 3：佔位外框可拖、放開對齊並寫回；鎖定時拖不動（非編輯版面時 quotes 隱藏、沒有可按之處，
        改以 clock 驗：真實拖曳後矩形不變、所有記錄不變）。
      - F1 項目 4：Reload 後外框仍在、仍可拖（Reload＝以 CDP `Page.crash` 精準讓 quotes 那一頁的
        renderer 當機，宿主的 RENDER_PROCESS_EXITED 處理是同一 HWND 的 `Reload()`；或 `-ReloadMode Page`）。
      - F1 項目 5：編輯版面中開啟關閉中的小工具（fixed），新視窗帶 WS_THICKFRAME 與八個把手；故障重建
        （CDP `Browser.crash` 讓小工具那組的 browser 行程當機 → 宿主先建新一代全部小工具視窗、再拆舊的）
        後，每扇非 quotes 小工具都是新 HWND、帶 WS_THICKFRAME，fixed 頁面有八個把手。
    未涵蓋：
      - F1 項目 5「建立過程中離開編輯版面不殘留 WS_THICKFRAME」：競態窗口在宿主內部（工作執行緒建好視窗
        → 排進主執行緒判斷 `resizable_now`），只有毫秒級、外部觀察不到何時開始；腳本每次經 CDP 下指令要
        啟動一次 node（約百毫秒），無法可靠落在窗口內，做出 PASS 也證明不了打到競態。替代驗證：Rust 單元
        測試 `resizable_now_reflects_state_at_call_time`（已有，驗判斷時機）；如需實機佐證，建議在
        `desktop::set_widget_resizable_on_main_thread` 實際執行時記一行（label、當下 edit_mode、是否加上
        WS_SIZEBOX），再對宿主記錄檔斷言「判斷時 edit_mode=false 就沒有加上」。
      - renderer 當機不經視窗工廠（recovery.rs：RENDER_PROCESS_EXITED＝同一 HWND `Reload()`），所以
        「renderer 當機 → 重建新 HWND」不成立；故障重建只驗 browser 行程當機那條（全部重建）。
        RENDER_PROCESS_UNRESPONSIVE 與看門狗觸發的單窗重建需要讓頁面卡死 15 秒以上，外部無法可靠製造，
        未涵蓋。
      - 「Reload 後立刻畫出外框」沒有時間門檻：最多等 15 秒，記錄實際毫秒數，不據以判 FAIL。

    安全：開頭 `Invoke-SafeInputPreflight`（鎖定 → 結束碼 2、合成輸入沒被系統計入 → 3），每次注入前由
    `lib/SafeInput.psm1` 重查鎖定，執行途中鎖定＝BLOCKED（結束碼 2）。已有 fc-host 在跑＝結束碼 4
    （不結束它）。前提不成立（PRECONDITION FAIL，結束碼 1）就不送該步驟的輸入：目標元素不可見、尺寸無效、
    與 viewport 沒有交集、按下點的 `elementFromPoint` 沒命中目標、找不到合法放開位置、Reload／重建沒有發生。
    當機指令送出前確認 CDP 埠的 Listen 行程都在本宿主行程樹內（`Test-CdpOwnedByTree`），cdp-crash.mjs
    自己再確認目標全是宿主頁面。

    選點：CDP 讀目標元素的 `getBoundingClientRect()` 與 `devicePixelRatio`（已含頁面縮放），以
    `ClientToScreen`（本執行緒 Per-Monitor-V2＝實體像素）換成螢幕座標（`lib/ScrollAreaTarget.psm1`），
    取元素與 viewport 交集的中心；不用「視窗矩形＋固定像素」。放開位置＝格線矩形差＋三成格寬／格高。

    遮擋經 `lib/Occluders.psm1`（白名單；`Assert-OccluderResult`，finally `Restore-Occluders`）；工作列／
    系統 UI＝ENV-BLOCKED（結束碼 3）。前景基準＝`lib/ScratchWindow.psm1` 的自己的表單。隔離：暫存
    %APPDATA%／%LOCALAPPDATA%、fixture 寫在暫存資料目錄、真正設定檔只比雜湊、開機自啟登錄以
    `lib/AutostartRegistry.psm1` 快照還原、收尾以 `lib/ProcessTree.psm1` 只停自己啟動的宿主行程樹與自己的
    表單；證據經 `lib/EvidenceLog.psm1` 去識別。

    步驟：
      0. 首次啟動：clock／macro／fixed／dynamic 出現，quotes 隱藏。
      L. 鎖定時拖不動：真實拖曳 clock 三格 → 矩形不變、所有記錄不變、前景／焦點不變。
      進入編輯版面（CDP `set_edit_mode(true)`）→ quotes 出現、佔位外框可見且帶拖曳屬性。
      1. 拖佔位外框到合法空位 → 對齊、寫回。
      2. Reload quotes → 同一 HWND、外框仍在、WS_THICKFRAME 仍在；再拖一次 → 對齊、寫回。
      3. 關閉再開啟 fixed → 新 HWND、WS_THICKFRAME、八個把手。
      R. 故障重建：Browser.crash → 每扇非 quotes 小工具是新 HWND 且帶 WS_THICKFRAME、fixed 有八個把手。
      4. 離開編輯版面 → quotes 隱藏、WS_THICKFRAME 拿掉。
      D. 有資料後在原格子出現：改寫 fixture（quotes 一筆）→ 等輪詢（≤45 秒）→ quotes 出現、矩形＝記錄的
         格子、記錄未變。

.PARAMETER Exe
    fc-host.exe 路徑，預設 host/target/release/fc-host.exe（應為**不含** self-test-ipc 的建置）。

.PARAMETER ReloadMode
    Crash（預設，CDP `Page.crash` 只讓 quotes 那一頁的 renderer 當機＝宿主的 Reload 路徑）或 Page
    （CDP `location.reload()`，只驗頁面啟動流程）。

.EXAMPLE
    cd host; cargo clean --release -p fc-host; cargo build --release
    pwsh -NoProfile -File host/tools/verify-7.3-placeholder.ps1
#>
[CmdletBinding()]
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\fc-host.exe'),
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [ValidateSet('Crash', 'Page')][string]$ReloadMode = 'Crash',
    [int]$CdpPort = 9385
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\AutostartRegistry.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\SafeInput.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\Occluders.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\VerifyVerdict.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\ScratchWindow.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\ProcessTree.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\ScrollAreaTarget.psm1') -Force

Add-Type -Namespace V73P -Name Native -MemberDefinition @'
[DllImport("user32.dll")] public static extern System.IntPtr SetThreadDpiAwarenessContext(System.IntPtr ctx);
[DllImport("user32.dll")] public static extern System.IntPtr GetTopWindow(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern System.IntPtr GetWindow(System.IntPtr hWnd, uint cmd);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr hWnd, out uint pid);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(System.IntPtr hWnd, System.Text.StringBuilder s, int n);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassName(System.IntPtr hWnd, System.Text.StringBuilder s, int n);
[DllImport("user32.dll")] public static extern bool GetWindowRect(System.IntPtr hWnd, out RECT r);
[DllImport("user32.dll")] public static extern bool ClientToScreen(System.IntPtr hWnd, ref POINT p);
[DllImport("user32.dll")] public static extern long GetWindowLongPtrW(System.IntPtr hWnd, int idx);
[DllImport("user32.dll")] public static extern System.IntPtr GetForegroundWindow();
[DllImport("user32.dll")] public static extern bool SetForegroundWindow(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern bool GetGUIThreadInfo(uint idThread, ref GUITHREADINFO info);
[DllImport("user32.dll")] public static extern System.IntPtr MonitorFromWindow(System.IntPtr hWnd, uint flags);
[DllImport("user32.dll")] public static extern bool GetMonitorInfoW(System.IntPtr hMon, ref MONITORINFO mi);
[StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
[StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
[StructLayout(LayoutKind.Sequential)] public struct MONITORINFO { public int cbSize; public RECT rcMonitor; public RECT rcWork; public uint dwFlags; }
[StructLayout(LayoutKind.Sequential)] public struct GUITHREADINFO {
  public int cbSize; public uint flags; public System.IntPtr hwndActive; public System.IntPtr hwndFocus;
  public System.IntPtr hwndCapture; public System.IntPtr hwndMenuOwner; public System.IntPtr hwndMoveSize;
  public System.IntPtr hwndCaret; public RECT rcCaret; }
'@

# Per-Monitor-V2（-4）：GetWindowRect／ClientToScreen／GetMonitorInfo／游標座標都是實體像素。
[void][V73P.Native]::SetThreadDpiAwarenessContext([IntPtr](-4))

$pf = Invoke-SafeInputPreflight
if ($pf.ExitCode -ne 0) { Write-Host $pf.Message; exit $pf.ExitCode }
Write-Host $pf.Message

$GRID = 48

# ── 純函式（tests/Verify73Placeholder.Tests.ps1 以 AST 取出測試）──────────────────────────
# 元素探查：可見性、拖曳屬性、矩形、dpr、viewport。運算式不含雙引號（經命令列傳給 node）。
function Get-ElementProbeExpression {
    param([Parameter(Mandatory)][string]$Selector)
    "(() => { const el = document.querySelector('$Selector'); if (!el) return { exists: false }; " +
    "const r = el.getBoundingClientRect(); const cs = getComputedStyle(el); " +
    "return { exists: true, display: cs.display, visibility: cs.visibility, drag: el.getAttribute('data-tauri-drag-region'), " +
    "editMode: document.body.classList.contains('edit-mode'), dpr: window.devicePixelRatio, left: r.left, top: r.top, " +
    "width: r.width, height: r.height, vw: window.innerWidth, vh: window.innerHeight }; })()"
}

function Get-ElementHitExpression {
    param([Parameter(Mandatory)][string]$Selector, [Parameter(Mandatory)][double]$CssX, [Parameter(Mandatory)][double]$CssY)
    $inv = [System.Globalization.CultureInfo]::InvariantCulture
    $x = $CssX.ToString('R', $inv); $y = $CssY.ToString('R', $inv)
    "(() => { const el = document.querySelector('$Selector'); const h = document.elementFromPoint($x, $y); " +
    "return { inside: !!(el && h && el.contains(h)), hit: h ? h.tagName + '.' + String(h.className) : null }; })()"
}

# 探查結果 → 能不能按：元素存在、display 不是 none、visibility 可見、寬高與 dpr > 0、與 viewport 有交集。
# 回傳 { Ok; Reason; CssX; CssY; Dpr }，CssX／CssY＝元素與 viewport 交集的中心。
function Test-ElementProbe {
    param([AllowNull()]$Probe)
    $no = { param($why) [PSCustomObject]@{ Ok = $false; Reason = $why; CssX = [double]::NaN; CssY = [double]::NaN; Dpr = [double]::NaN } }
    if ($null -eq $Probe) { return (& $no '探查沒有結果（CDP 失敗或頁面未就緒）') }
    $get = { param($n) $p = $Probe.PSObject.Properties[$n]; if ($null -eq $p) { $null } else { $p.Value } }
    if ((& $get 'exists') -ne $true) { return (& $no '頁面沒有目標元素') }
    if ([string](& $get 'display') -eq 'none') { return (& $no '目標元素 display=none（未顯示）') }
    if ([string](& $get 'visibility') -eq 'hidden') { return (& $no '目標元素 visibility=hidden') }
    $num = @{}
    foreach ($n in 'dpr', 'left', 'top', 'width', 'height', 'vw', 'vh') {
        $d = 0.0
        if (-not [double]::TryParse([string](& $get $n), [System.Globalization.NumberStyles]::Float, [System.Globalization.CultureInfo]::InvariantCulture, [ref]$d)) {
            return (& $no "探查欄位 $n 缺失或不是數字")
        }
        $num[$n] = $d
    }
    if ($num.dpr -le 0 -or $num.width -le 0 -or $num.height -le 0 -or $num.vw -le 0 -or $num.vh -le 0) {
        return (& $no "尺寸無效（width=$($num.width) height=$($num.height) dpr=$($num.dpr) viewport=$($num.vw)x$($num.vh)）")
    }
    $l = [math]::Max(0.0, $num.left); $t = [math]::Max(0.0, $num.top)
    $r = [math]::Min($num.vw, $num.left + $num.width); $b = [math]::Min($num.vh, $num.top + $num.height)
    if ($r - $l -lt 1.0 -or $b - $t -lt 1.0) { return (& $no '目標元素與 viewport 沒有交集') }
    return [PSCustomObject]@{ Ok = $true; Reason = ''; CssX = ($l + $r) / 2.0; CssY = ($t + $b) / 2.0; Dpr = $num.dpr }
}

function Test-ElementHit {
    param([AllowNull()]$Hit)
    if ($null -eq $Hit) { return [PSCustomObject]@{ Ok = $false; Reason = '命中測試沒有結果（CDP 失敗）' } }
    $p = $Hit.PSObject.Properties['inside']
    if ($null -ne $p -and $p.Value -eq $true) { return [PSCustomObject]@{ Ok = $true; Reason = '' } }
    $h = $Hit.PSObject.Properties['hit']
    return [PSCustomObject]@{ Ok = $false; Reason = "按下點命中 $(if ($h) { $h.Value } else { '(無)' })，不是目標元素" }
}

function Test-GridOverlap($A, $B) {
    ($A.col -lt $B.col + $B.w) -and ($B.col -lt $A.col + $A.w) -and ($A.row -lt $B.row + $B.h) -and ($B.row -lt $A.row + $A.h)
}

# 放開位置：保留格數，依序試同列左移 10..1 格、右移 10..1 格（先試大位移）、上下移 1..2 列；
# 必須在 48×48 內、不等於目前位置、與其他開啟中小工具的記錄都不相交。找不到回傳 $null。
function Select-PlaceholderDropTarget {
    param([Parameter(Mandatory)]$Current, [AllowEmptyCollection()][object[]]$Others = @())
    $moves = @()
    foreach ($d in 10..1) { $moves += , @(-$d, 0) }
    foreach ($d in 10..1) { $moves += , @($d, 0) }
    foreach ($d in -1, 1, -2, 2) { $moves += , @(0, $d) }
    foreach ($m in $moves) {
        $c = [PSCustomObject]@{ col = $Current.col + $m[0]; row = $Current.row + $m[1]; w = $Current.w; h = $Current.h }
        if ($c.col -lt 0 -or $c.row -lt 0 -or $c.col + $c.w -gt $GRID -or $c.row + $c.h -gt $GRID) { continue }
        if (@($Others | Where-Object { Test-GridOverlap $c $_ }).Count -gt 0) { continue }
        return $c
    }
    return $null
}

function Test-HasThickFrame([long]$Style) { [bool]($Style -band 0x40000) }  # WS_THICKFRAME

# CDP 埠的 Listen 行程 vs. 本宿主行程樹 → 'OK'｜'NO-LISTENER'｜'FOREIGN'（只有 OK 才送當機指令）。
function Test-CdpOwnedByTree {
    param([AllowEmptyCollection()][int[]]$ListenerPids = @(), [AllowEmptyCollection()][int[]]$TreeIds = @())
    if (@($ListenerPids).Count -eq 0) { return 'NO-LISTENER' }
    foreach ($l in $ListenerPids) { if (-not ($TreeIds -contains $l)) { return 'FOREIGN' } }
    return 'OK'
}

# 故障重建判讀：-Before＝id → 重建前 HWND；-After＝id → @{ Hwnd; Style }。回傳問題清單（空＝通過）。
function Test-RebuiltWindows {
    param([Parameter(Mandatory)][hashtable]$Before, [Parameter(Mandatory)][hashtable]$After)
    $problems = @()
    foreach ($id in @($Before.Keys | Sort-Object)) {
        if (-not $After.ContainsKey($id) -or -not $After[$id] -or [int64]$After[$id].Hwnd -eq 0) { $problems += "$id：重建後沒有可見視窗"; continue }
        if ([int64]$After[$id].Hwnd -eq [int64]$Before[$id]) { $problems += "$id：HWND 沒變（沒有重建）" }
        if (-not (Test-HasThickFrame ([long]$After[$id].Style))) { $problems += "$id：重建後的視窗沒有 WS_THICKFRAME" }
    }
    return $problems
}

# ── 視窗與 CDP 工具 ──────────────────────────────────────────────────────────────
function Get-Ts { Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fffK' }
function Hex([IntPtr]$h) { '0x{0:X}' -f $h.ToInt64() }
function Get-Cls([IntPtr]$h) { $sb = New-Object System.Text.StringBuilder 256; [void][V73P.Native]::GetClassName($h, $sb, 256); $sb.ToString() }
function Get-WinPid([IntPtr]$h) { $p = 0; [void][V73P.Native]::GetWindowThreadProcessId($h, [ref]$p); [int]$p }
function Get-Rect([IntPtr]$h) {
    $r = New-Object V73P.Native+RECT
    [void][V73P.Native]::GetWindowRect($h, [ref]$r)
    [PSCustomObject]@{ X = $r.Left; Y = $r.Top; W = $r.Right - $r.Left; H = $r.Bottom - $r.Top }
}
function Get-Style([IntPtr]$h) { [V73P.Native]::GetWindowLongPtrW($h, -16) }
function Fmt($r) { if ($r) { "($($r.X),$($r.Y),$($r.W)x$($r.H))" } else { '<null>' } }
function Same($a, $b) { $a -and $b -and $a.X -eq $b.X -and $a.Y -eq $b.Y -and $a.W -eq $b.W -and $a.H -eq $b.H }
function Edge([int]$Origin, [int]$Extent, [int]$I) { $Origin + [int][math]::Floor([int64]$I * $Extent / $GRID) }
function Grid-Rect($Wa, $P) {
    $x0 = Edge $Wa.X $Wa.W $P.col; $x1 = Edge $Wa.X $Wa.W ($P.col + $P.w)
    $y0 = Edge $Wa.Y $Wa.H $P.row; $y1 = Edge $Wa.Y $Wa.H ($P.row + $P.h)
    [PSCustomObject]@{ X = $x0; Y = $y0; W = $x1 - $x0; H = $y1 - $y0 }
}
function Get-WorkArea([IntPtr]$H) {
    $mi = New-Object V73P.Native+MONITORINFO
    $mi.cbSize = [Runtime.InteropServices.Marshal]::SizeOf($mi)
    [void][V73P.Native]::GetMonitorInfoW([V73P.Native]::MonitorFromWindow($H, 2), [ref]$mi)
    [PSCustomObject]@{ X = $mi.rcWork.Left; Y = $mi.rcWork.Top; W = $mi.rcWork.Right - $mi.rcWork.Left; H = $mi.rcWork.Bottom - $mi.rcWork.Top }
}
function Find-Window([int]$ProcId, [string]$Title) {
    $h = [V73P.Native]::GetTopWindow([IntPtr]::Zero)
    while ($h -ne [IntPtr]::Zero) {
        if ((Get-WinPid $h) -eq $ProcId -and [V73P.Native]::IsWindowVisible($h)) {
            $sb = New-Object System.Text.StringBuilder 256
            [void][V73P.Native]::GetWindowText($h, $sb, 256)
            if ($sb.ToString() -eq $Title) { return $h }
        }
        $h = [V73P.Native]::GetWindow($h, 2)
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
function Wait-WindowGone([int]$ProcId, [string]$Title, [int]$TimeoutSec = 15) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt $TimeoutSec) {
        if ((Find-Window $ProcId $Title) -eq [IntPtr]::Zero) { return $true }
        Start-Sleep -Milliseconds 200
    }
    return $false
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
function Wait-ThickFrame([IntPtr]$H, [bool]$Want, [int]$TimeoutSec = 5) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    do {
        $has = ($H -ne [IntPtr]::Zero) -and (Test-HasThickFrame (Get-Style $H))
        if ($has -eq $Want) { return $has }
        Start-Sleep -Milliseconds 200
    } while ($sw.Elapsed.TotalSeconds -lt $TimeoutSec)
    return $has
}
function Invoke-Eval([string]$Page, [string]$Expr) {
    $out = & node (Join-Path $PSScriptRoot 'host-cdp-eval.mjs') $CdpPort $Page $Expr 2>&1
    return ($out -join "`n").Trim()
}
function Invoke-EvalJson([string]$Page, [string]$Expr) {
    $raw = Invoke-Eval $Page $Expr
    try { return ($raw | ConvertFrom-Json -ErrorAction Stop) } catch { return $null }
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
function Wait-TimeOriginChanged([string]$Page, [string]$Before, [int]$TimeoutSec = 15) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt $TimeoutSec) {
        Start-Sleep -Milliseconds 400
        $now = Invoke-Eval $Page 'performance.timeOrigin'
        if ($now -match '^\d' -and $now -ne $Before) { return $true }
    }
    return $false
}
function Get-FocusState([uint32]$Tid) {
    $info = New-Object V73P.Native+GUITHREADINFO
    $info.cbSize = [Runtime.InteropServices.Marshal]::SizeOf($info)
    if ([V73P.Native]::GetGUIThreadInfo($Tid, [ref]$info)) { return $info.hwndFocus }
    return [IntPtr]::Zero
}

# fixture：預設行情條無內容（quotes 空陣列）；-WithQuotes 填一筆（大小改變，JsonFileSource 才會重讀）。
function New-Fixture([switch]$WithQuotes) {
    $today = Get-Date -Format 'yyyy-MM-dd'
    $now = Get-Date -Format 'yyyy-MM-dd HH:mm'
    $events = for ($i = 1; $i -le 5; $i++) { [ordered]@{ date = $today; type = 'earnings'; code = ('T{0:D3}' -f $i); name = "測試公司$i"; note = '第1季 財報' } }
    $quotes = @()
    if ($WithQuotes) { $quotes = @([ordered]@{ name = 'USD/TWD'; kind = 'fx'; price = 31.5; chg_pct = 0.12; chg_abs = 0.04 }) }
    ([ordered]@{ updated = $now; fetched = $now; errors = @(); macro = @(); events = @($events); punish = @(); quotes = $quotes; holidays = @() }) | ConvertTo-Json -Depth 6
}

$settingsExpr = "window.__TAURI__.core.invoke('get_settings').then(s => { const out = { locked: s.layout_locked, widgets: {} }; for (const [k, v] of Object.entries(s.widgets)) { out.widgets[k] = { enabled: v.enabled, col: v.placement.col, row: v.placement.row, w: v.placement.w, h: v.placement.h }; } return out; })"
function Get-SettingsView { Invoke-EvalJson 'w=clock' $settingsExpr }
function Get-Placement($View, [string]$Id) {
    $w = $View.widgets.PSObject.Properties[$Id].Value
    [PSCustomObject]@{ col = [int]$w.col; row = [int]$w.row; w = [int]$w.w; h = [int]$w.h }
}
function Get-OtherRecords($View, [string]$Id) {
    @($View.widgets.PSObject.Properties | Where-Object { $_.Name -ne $Id -and $_.Value.enabled -eq $true } |
            ForEach-Object { [PSCustomObject]@{ col = [int]$_.Value.col; row = [int]$_.Value.row; w = [int]$_.Value.w; h = [int]$_.Value.h } })
}
function Fmt-P($P) { if ($P) { "$($P.col),$($P.row),$($P.w),$($P.h)" } else { '<null>' } }
function Fmt-AllRecords($View) {
    if (-not $View) { return '<null>' }
    (@($View.widgets.PSObject.Properties | Sort-Object Name | ForEach-Object { "$($_.Name)=$($_.Value.enabled):$($_.Value.col),$($_.Value.row),$($_.Value.w),$($_.Value.h)" })) -join ';'
}

# ── 前置 ─────────────────────────────────────────────────────────────────────────
if (Get-Process -Name fc-host -ErrorAction SilentlyContinue) {
    Write-Host 'HOST-RUNNING: 已有 fc-host 在執行，本腳本不結束它；請先自行關閉（結束碼 4）。'
    exit 4
}
$Exe = (Resolve-Path $Exe).Path
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$logPath = Join-Path $OutDir '7.3-placeholder.log'
$sumPath = Join-Path $OutDir '7.3-placeholder-summary.log'
$log = New-EvidenceWriter $logPath
$log.AutoFlush = $true
function Log([string]$m) { $line = "$(Get-Ts) $m"; $log.WriteLine($line); Write-Host $line }
Log "# verify-7.3-placeholder.ps1 exe=$Exe reloadMode=$ReloadMode preflight=$($pf.Message)"

$realSettings = Join-Path $env:APPDATA 'tw.fintools.fc-host\settings.json'
$realHashBefore = if (Test-Path $realSettings) { (Get-FileHash $realSettings).Hash } else { '<不存在>' }
$regSnap = @(Save-FcHostAutostartRegistry)
Log "# 真正的設定檔雜湊（開始）=$realHashBefore；開機自啟登錄快照：$(($regSnap | ForEach-Object { "$($_.Name)@$(Split-Path $_.Key -Leaf) Exists=$($_.Exists)" }) -join '; ')"

$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ('fc-host-7.3p-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
$tempAppData = Join-Path $tempRoot 'Roaming'
$tempLocal = Join-Path $tempRoot 'Local'
$dataDir = Join-Path $tempLocal 'tw.fintools.fc-host\data'
$fixturePath = Join-Path $dataDir 'tw_events.json'
New-Item -ItemType Directory -Force -Path $tempAppData, $dataDir | Out-Null
Set-Content -Path $fixturePath -Value (New-Fixture) -Encoding UTF8
Log "# 暫存 APPDATA=$tempAppData LOCALAPPDATA=$tempLocal（fixture：quotes 空陣列＝行情條無內容）"

$results = [ordered]@{}
$script:mouseDown = $false
$script:blocked = $null
$script:envBlocked = $null
$script:precondition = $null
$hostProc = $null
$hostStart = $null
$form = $null
$minimized = New-Object System.Collections.Generic.List[object]
# hung-target（review 642050f）：沒還原成功的使用者視窗（無回應、最小化未確認、讀回逾時／不符），寫進摘要。
$occNotRestored = New-Object System.Collections.Generic.List[string]
$occLog = { param($m) Log "## $m" }

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

function Get-OwnPids { if ($form) { @($form.Process.Id) } else { @() } }

# 當機指令唯一入口：CDP 埠的 Listen 行程必須都在本宿主行程樹內才呼叫 cdp-crash.mjs（它自己再確認目標
# 全是宿主頁面，page 模式只打 `w=<id>` 恰好一頁）。不符丟 PRECONDITION、一個指令都不送。
function Invoke-CdpCrash([string]$Mode, [string]$Id = '') {
    $listeners = @(Get-NetTCPConnection -LocalPort $CdpPort -State Listen -ErrorAction SilentlyContinue | ForEach-Object { [int]$_.OwningProcess } | Sort-Object -Unique)
    $procs = @(Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, CreationDate)
    $tree = @(Get-ProcessTreeIds -RootId $hostProc.Id -RootStartTime $hostStart -Processes $procs)
    $owner = Test-CdpOwnedByTree -ListenerPids $listeners -TreeIds $tree
    Log "## CDP 當機 $Mode $Id：埠 $CdpPort Listen=$($listeners -join ',') 判定=$owner"
    if ($owner -ne 'OK') { throw "PRECONDITION: CDP 埠 $CdpPort 不屬於本宿主（$owner），不送當機指令" }
    $crashArgs = @($CdpPort, $Mode)
    if ($Id) { $crashArgs += $Id }
    $out = & node (Join-Path $PSScriptRoot 'cdp-crash.mjs') @crashArgs 2>&1
    $code = $LASTEXITCODE
    foreach ($l in $out) { Log "##   cdp> $l" }
    if ($code -ne 0) { throw "PRECONDITION: cdp-crash.mjs $Mode $Id 結束碼 $code（3＝目標檢查不通過、未送指令）" }
}

# 拖曳：按下 → 等系統移動迴圈開始 → 分段移動（每步取樣前景／焦點）→ 放開。
function Invoke-Drag {
    param([int]$FromX, [int]$FromY, [int]$ToX, [int]$ToY, [int]$Steps = 16)
    $samples = New-Object System.Collections.Generic.List[object]
    $sample = { $samples.Add([PSCustomObject]@{ Fg = [V73P.Native]::GetForegroundWindow(); Focus = (Get-FocusState $form.ThreadId) }) }
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
function Reset-Foreground {
    [void][V73P.Native]::SetForegroundWindow($form.Hwnd)
    Start-Sleep -Milliseconds 300
    $script:baseFg = [V73P.Native]::GetForegroundWindow()
    $script:baseFocus = Get-FocusState $form.ThreadId
}

# 按下點：探查（前提）→ CSS 中心 × dpr ＋ ClientToScreen → 在視窗矩形內 → 遮擋判讀 → 命中（前提）。
# 任何前提不成立都丟 PRECONDITION（呼叫端尚未送任何輸入）。回傳 @{ X; Y }。
function Get-PressPoint([string]$Tag, [string]$Page, [string]$Selector, [IntPtr]$Hwnd) {
    $probe = Invoke-EvalJson $Page (Get-ElementProbeExpression -Selector $Selector)
    $pre = Test-ElementProbe $probe
    Log "## $Tag 前提：$Selector 探查=$(if ($probe) { $probe | ConvertTo-Json -Compress } else { 'null' }) ok=$($pre.Ok) $($pre.Reason)"
    if (-not $pre.Ok) { throw "PRECONDITION: $Tag $Selector 不可按：$($pre.Reason)" }
    $rect0 = Get-Rect $Hwnd
    $origin = New-Object V73P.Native+POINT
    [void][V73P.Native]::ClientToScreen($Hwnd, [ref]$origin)
    $pt = ConvertTo-ScreenPoint -OriginX $origin.X -OriginY $origin.Y -Dpr $pre.Dpr -CssX $pre.CssX -CssY $pre.CssY
    Log "## $Tag 選點 css=($($pre.CssX),$($pre.CssY)) dpr=$($pre.Dpr) origin=($($origin.X),$($origin.Y)) → 螢幕 ($($pt.X),$($pt.Y))；視窗=$(Fmt $rect0)"
    if ($pt.X -lt $rect0.X -or $pt.X -ge $rect0.X + $rect0.W -or $pt.Y -lt $rect0.Y -or $pt.Y -ge $rect0.Y + $rect0.H) {
        throw "PRECONDITION: $Tag 按下點 ($($pt.X),$($pt.Y)) 不在視窗矩形 $(Fmt $rect0) 內"
    }
    Assert-OccluderResult (Clear-Occluders -HostPid $hostProc.Id -Points @(, @($pt.X, $pt.Y)) -Minimized $minimized -OwnPids (Get-OwnPids) -Log $occLog) -What "$Tag 按下點"
    $css = ConvertTo-CssPoint -OriginX $origin.X -OriginY $origin.Y -Dpr $pre.Dpr -X $pt.X -Y $pt.Y
    $hit = Test-ElementHit (Invoke-EvalJson $Page (Get-ElementHitExpression -Selector $Selector -CssX $css.X -CssY $css.Y))
    Log "## $Tag 前提：按下點命中 css=($($css.X),$($css.Y)) ok=$($hit.Ok) $($hit.Reason)"
    if (-not $hit.Ok) { throw "PRECONDITION: $Tag $($hit.Reason)" }
    return $pt
}

# L：鎖定（非編輯版面）時真實拖曳 clock 三格 → 矩形、所有記錄都不變。
function Invoke-LockedDrag([IntPtr]$Clock) {
    $before = Get-SettingsView
    if (-not $before -or $before.locked -ne $true) { throw "PRECONDITION: L 不在鎖定狀態（layout_locked=$(if ($before) { $before.locked } else { '<讀不到>' })）" }
    $rect0 = Get-Rect $Clock
    $pt = Get-PressPoint 'L' 'w=clock' '#widget-root' $Clock
    Reset-Foreground
    $wa = Get-WorkArea $Clock
    $dx = [int][math]::Floor($wa.W / $GRID * 3); $dy = [int][math]::Floor($wa.H / $GRID * 2)
    $s = Invoke-Drag -FromX $pt.X -FromY $pt.Y -ToX ($pt.X - $dx) -ToY ($pt.Y + $dy)
    Start-Sleep -Milliseconds 800
    $r = Get-Rect $Clock
    $after = Get-SettingsView
    Invoke-Eval 'w=clock' "(() => { window.getSelection().removeAllRanges(); return 'ok'; })()" | Out-Null
    Log "## L 鎖定時拖曳：前=$(Fmt $rect0) 後=$(Fmt $r)；記錄前=$(Fmt-AllRecords $before) 後=$(Fmt-AllRecords $after)"
    $results['L：鎖定時拖不動（真實拖曳 clock 後矩形不變）'] = Same $r $rect0
    $results['L：鎖定時拖曳後所有記錄不變（沒有寫回）'] = ($after -and (Fmt-AllRecords $after) -eq (Fmt-AllRecords $before))
    $results['L：拖曳期間前景與焦點不變'] = Test-FgUnchanged $s 'L'
}

# 佔位外框拖曳一次：按下點（前提）→ 拖到合法空位 → 對齊與記錄。
function Invoke-PlaceholderDrag([string]$Tag, [IntPtr]$Quotes) {
    $view = Get-SettingsView
    if (-not $view) { throw "PRECONDITION: $Tag 讀不到 get_settings" }
    $cur = Get-Placement $view 'quotes'
    $target = Select-PlaceholderDropTarget -Current $cur -Others (Get-OtherRecords $view 'quotes')
    if (-not $target) { throw "PRECONDITION: $Tag 找不到 quotes $(Fmt-P $cur) 的合法放開位置" }
    $wa = Get-WorkArea $Quotes
    $rect0 = Get-Rect $Quotes
    $expect = Grid-Rect $wa $target
    Log "## $Tag 記錄=$(Fmt-P $cur) 視窗=$(Fmt $rect0) 目標=$(Fmt-P $target) 期望矩形=$(Fmt $expect) 工作區=$(Fmt $wa)"
    $pt = Get-PressPoint $Tag 'w=quotes' '#widget-root > .edit-placeholder' $Quotes
    Reset-Foreground
    # 放開位置＝格線矩形差＋三成格寬／格高（不用固定像素；驗 round 對齊）。
    $nudgeX = [int][math]::Floor($wa.W / $GRID * 0.3); $nudgeY = [int][math]::Floor($wa.H / $GRID * 0.3)
    $dx = $expect.X - $rect0.X + $nudgeX; $dy = $expect.Y - $rect0.Y + $nudgeY
    $s = Invoke-Drag -FromX $pt.X -FromY $pt.Y -ToX ($pt.X + $dx) -ToY ($pt.Y + $dy)
    $r = Wait-StableRect $Quotes
    $after = Get-SettingsView
    $rec = if ($after) { Get-Placement $after 'quotes' } else { $null }
    Log "## $Tag 放開後=$(Fmt $r) 期望=$(Fmt $expect) 記錄=$(Fmt-P $rec)（期望 $(Fmt-P $target)）"
    $results["$Tag：拖曳佔位外框後視窗四邊落在目標格線 $(Fmt-P $target)"] = Same $r $expect
    $results["$Tag：記錄寫回 $(Fmt-P $target)"] = ((Fmt-P $rec) -eq (Fmt-P $target))
    $results["$Tag：拖曳期間前景與焦點不變"] = Test-FgUnchanged $s $Tag
}

$placeholderOk = { param($p) $p -and $p.exists -eq $true -and $p.display -ne 'none' -and $p.drag -eq 'deep' -and $p.editMode -eq $true }
$placeholderExpr = Get-ElementProbeExpression -Selector '#widget-root > .edit-placeholder'
$nonQuotes = 'clock', 'macro', 'fixed', 'dynamic'

try {
    Assert-SessionUnlocked '腳本啟動前置檢查'

    # ── 0. 首次啟動 ─────────────────────────────────────────────────────────────
    $hostProc = Start-Host
    $hostStart = Get-ProcessStartTimeOrNull $hostProc
    Log "# 宿主 pid=$($hostProc.Id)"
    foreach ($id in $nonQuotes) {
        if ((Wait-Window $hostProc.Id "fc-host $id") -eq [IntPtr]::Zero) { throw "PRECONDITION: $id 視窗未出現" }
    }
    Start-Sleep -Seconds 3
    $quotesHiddenAtStart = ((Find-Window $hostProc.Id 'fc-host quotes') -eq [IntPtr]::Zero)
    Log "## 0 首次啟動：quotes 隱藏=$quotesHiddenAtStart"
    $results['0：quotes 無內容時隱藏（3.2）'] = $quotesHiddenAtStart
    if (-not $quotesHiddenAtStart) { throw 'PRECONDITION: quotes 在 fixture quotes:[] 下仍可見（fixture 未生效），不驗佔位外框' }

    $clock = Find-Window $hostProc.Id 'fc-host clock'
    $waPrimary = Get-WorkArea $clock
    # 表單放左上（cols 0–14 平時空著；拖曳路徑在 clock 左下與下緣行情條那幾列）；寬高取工作區比例。
    $form = Start-ScratchForm -Title ('fc-host-7.3p-fg-' + [guid]::NewGuid().ToString('N').Substring(0, 6)) `
        -X ($waPrimary.X + [int]($waPrimary.W * 0.02)) -Y ($waPrimary.Y + [int]($waPrimary.H * 0.05)) `
        -Width ([int]($waPrimary.W * 0.2)) -Height ([int]($waPrimary.H * 0.15))
    Send-GuardedAltTap
    Reset-Foreground
    Log "## 前景基準：fg=$(Hex $script:baseFg)($(Get-Cls $script:baseFg)) 期望=$(Hex $form.Hwnd) 焦點=$(Hex $script:baseFocus)"
    $results['前置：前景基準表單取得前景與鍵盤焦點'] = ($script:baseFg -eq $form.Hwnd -and $script:baseFocus -ne [IntPtr]::Zero)

    # ── L. 鎖定時拖不動 ─────────────────────────────────────────────────────────────
    Invoke-LockedDrag $clock

    # ── 進入編輯版面 ─────────────────────────────────────────────────────────────
    Invoke-Eval 'w=clock' "window.__TAURI__.core.invoke('set_edit_mode', { enabled: true })" | Out-Null
    $locked = Wait-Eval 'w=clock' "window.__TAURI__.core.invoke('get_settings').then(s => s.layout_locked)" 'false'
    $results['0：進入編輯版面（layout_locked=false）'] = ($locked -match 'false')
    $quotes = Wait-Window $hostProc.Id 'fc-host quotes' 15
    $results['0：編輯版面期間無內容的 quotes 視窗出現（3.2）'] = ($quotes -ne [IntPtr]::Zero)
    if ($quotes -eq [IntPtr]::Zero) { throw 'PRECONDITION: 進入編輯版面後 quotes 視窗沒有出現，無法驗佔位外框' }
    $p0 = Invoke-EvalJson 'w=quotes' $placeholderExpr
    Log "## 0 佔位外框=$(if ($p0) { $p0 | ConvertTo-Json -Compress } else { 'null' })"
    $results['0：佔位外框可見、帶 data-tauri-drag-region=deep、body.edit-mode（3.2）'] = [bool](& $placeholderOk $p0)

    # ── 1. 佔位外框拖曳 ───────────────────────────────────────────────────────────
    Invoke-PlaceholderDrag '1' $quotes

    # ── 2. Reload 後外框仍在、仍可拖 ─────────────────────────────────────────────────
    $hwndBefore = $quotes
    $t0 = Invoke-Eval 'w=quotes' 'performance.timeOrigin'
    $otherT0 = @{}; foreach ($id in $nonQuotes) { $otherT0[$id] = Invoke-Eval "w=$id" 'performance.timeOrigin' }
    if ($ReloadMode -eq 'Page') {
        Invoke-Eval 'w=quotes' "(() => { setTimeout(() => location.reload(), 50); return 'ok'; })()" | Out-Null
    } else {
        Invoke-CdpCrash 'page' 'quotes'
    }
    $reloaded = Wait-TimeOriginChanged 'w=quotes' $t0 20
    $alsoReloaded = @($nonQuotes | Where-Object { $now = Invoke-Eval "w=$_" 'performance.timeOrigin'; $now -match '^\d' -and $now -ne $otherT0[$_] })
    Log "## 2 quotes Reload（timeOrigin 改變）=$reloaded；其他一併重載的頁面=$(if ($alsoReloaded) { $alsoReloaded -join ',' } else { '無' })（renderer 共用時會一起重載，僅記錄）"
    $results["2：quotes 頁面已 Reload（$ReloadMode）"] = $reloaded
    if (-not $reloaded) { throw 'PRECONDITION: quotes 頁面沒有被 Reload，無法驗 Reload 後的外框' }
    $sw = [Diagnostics.Stopwatch]::StartNew(); $p2 = $null
    while ($sw.Elapsed.TotalSeconds -lt 15) {
        $p2 = Invoke-EvalJson 'w=quotes' $placeholderExpr
        if (& $placeholderOk $p2) { break }
        Start-Sleep -Milliseconds 300
    }
    Log "## 2 Reload 後佔位外框=$(if ($p2) { $p2 | ConvertTo-Json -Compress } else { 'null' })（等了 $([int]$sw.Elapsed.TotalMilliseconds) ms；沒有時間門檻）"
    $quotes = Find-Window $hostProc.Id 'fc-host quotes'
    $results['2：Reload 後 quotes 視窗仍可見'] = ($quotes -ne [IntPtr]::Zero)
    $results['2：Reload 後仍是同一個 HWND'] = ($quotes -eq $hwndBefore)
    $results['2：Reload 後同一視窗仍帶 WS_THICKFRAME'] = (Wait-ThickFrame $quotes $true)
    $results['2：Reload 後佔位外框仍在（可見、data-tauri-drag-region=deep、body.edit-mode）'] = [bool](& $placeholderOk $p2)
    if ($quotes -eq [IntPtr]::Zero) { throw 'PRECONDITION: Reload 後 quotes 視窗不可見，無法再拖' }
    Invoke-PlaceholderDrag '2' $quotes

    # ── 3. 編輯版面中開啟關閉中的小工具 → 新視窗帶 WS_THICKFRAME ───────────────────────────
    $fixedOld = Find-Window $hostProc.Id 'fc-host fixed'
    $off = Invoke-Eval 'w=clock' "window.__TAURI__.core.invoke('update_settings', { patch: { widgets: { fixed: { enabled: false } } } }).then(() => 'ok')"
    $gone = Wait-WindowGone $hostProc.Id 'fc-host fixed'
    Log "## 3 關閉 fixed：回應=$off 舊 HWND=$(Hex $fixedOld) 視窗消失=$gone"
    if (-not $gone) { throw 'PRECONDITION: 關閉 fixed 後視窗沒有消失，無法驗「新建」的視窗' }
    $stillEdit = Invoke-Eval 'w=clock' "window.__TAURI__.core.invoke('get_settings').then(s => s.layout_locked)"
    if ($stillEdit -notmatch 'false') { throw "PRECONDITION: 開啟 fixed 前已不在編輯版面（layout_locked=$stillEdit）" }
    $on = Invoke-Eval 'w=clock' "window.__TAURI__.core.invoke('update_settings', { patch: { widgets: { fixed: { enabled: true } } } }).then(() => 'ok')"
    $fixedNew = Wait-Window $hostProc.Id 'fc-host fixed' 20
    $tfNew = Wait-ThickFrame $fixedNew $true
    $handles = Wait-Eval 'w=fixed' "document.querySelectorAll('[data-resize-dir]').length" '^8$' 10
    Log "## 3 開啟 fixed：回應=$on 新 HWND=$(Hex $fixedNew) WS_THICKFRAME=$tfNew 把手數=$handles"
    $results['3：編輯版面中開啟 fixed → 新視窗出現（新 HWND）'] = ($fixedNew -ne [IntPtr]::Zero -and $fixedNew -ne $fixedOld)
    $results['3：編輯版面中新建的視窗帶 WS_THICKFRAME'] = $tfNew
    $results['3：新建視窗的頁面有八個調整大小把手'] = ($handles -eq '8')

    # ── R. 故障重建（browser 行程當機 → 全部重建）後新 HWND 帶 WS_THICKFRAME ──────────────────
    $beforeR = @{}
    foreach ($id in $nonQuotes) { $beforeR[$id] = (Find-Window $hostProc.Id "fc-host $id").ToInt64() }
    Log "## R 重建前 HWND：$(($nonQuotes | ForEach-Object { "$_=0x{0:X}" -f $beforeR[$_] }) -join ' ')"
    # 埠上必須是小工具那組的 browser（看得到 w=clock 頁面），不是桌布渲染那組。
    $widgetBrowser = Invoke-Eval 'w=clock' "'widget-browser'"
    if ($widgetBrowser -ne '"widget-browser"') { throw "PRECONDITION: R CDP 埠上看不到 w=clock 頁面（$widgetBrowser），不送 Browser.crash" }
    Invoke-CdpCrash 'browser'
    $afterR = @{}
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt 40) {
        foreach ($id in $nonQuotes) { $h = Find-Window $hostProc.Id "fc-host $id"; $afterR[$id] = @{ Hwnd = $h.ToInt64(); Style = $(if ($h -ne [IntPtr]::Zero) { Get-Style $h } else { 0 }) } }
        if (@($nonQuotes | Where-Object { $afterR[$_].Hwnd -eq 0 -or $afterR[$_].Hwnd -eq $beforeR[$_] }).Count -eq 0) { break }
        Start-Sleep -Milliseconds 400
    }
    # 新視窗的 WS_SIZEBOX 是排進主執行緒套用的，再給 5 秒。
    foreach ($id in $nonQuotes) {
        $h = [IntPtr]$afterR[$id].Hwnd
        if ($h -ne [IntPtr]::Zero) { [void](Wait-ThickFrame $h $true); $afterR[$id].Style = Get-Style $h }
    }
    $problems = @(Test-RebuiltWindows -Before $beforeR -After $afterR)
    $handlesR = Wait-Eval 'w=fixed' "document.querySelectorAll('[data-resize-dir]').length" '^8$' 15
    Log "## R 重建後：$(($nonQuotes | ForEach-Object { "$_=0x{0:X} style=0x{1:X8}" -f $afterR[$_].Hwnd, $afterR[$_].Style }) -join ' ')；問題=$(if ($problems) { $problems -join '；' } else { '無' })；fixed 把手數=$handlesR"
    $results['R：故障重建後每扇非 quotes 小工具都是新 HWND 且帶 WS_THICKFRAME'] = ($problems.Count -eq 0)
    $results['R：故障重建後 fixed 頁面有八個調整大小把手'] = ($handlesR -eq '8')
    $quotes = Wait-Window $hostProc.Id 'fc-host quotes' 15

    # ── 4. 離開編輯版面 ───────────────────────────────────────────────────────────
    Invoke-Eval 'w=clock' "window.__TAURI__.core.invoke('set_edit_mode', { enabled: false })" | Out-Null
    $locked2 = Wait-Eval 'w=clock' "window.__TAURI__.core.invoke('get_settings').then(s => s.layout_locked)" 'true'
    $quotesGone = Wait-WindowGone $hostProc.Id 'fc-host quotes'
    $fixedNow = Find-Window $hostProc.Id 'fc-host fixed'
    $tfAfter = ($fixedNow -ne [IntPtr]::Zero) -and (Wait-ThickFrame $fixedNow $false)
    Log "## 4 離開編輯版面：layout_locked=$locked2 quotes 隱藏=$quotesGone fixed WS_THICKFRAME=$tfAfter"
    $results['4：離開編輯版面後無內容的 quotes 再度隱藏（3.2）'] = ($locked2 -match 'true' -and $quotesGone)
    $results['4：離開編輯版面後 fixed 的 WS_THICKFRAME 拿掉'] = ($fixedNow -ne [IntPtr]::Zero -and -not $tfAfter)

    # ── D. 有資料後在原格子出現（3.2）────────────────────────────────────────────────
    $viewD = Get-SettingsView
    if (-not $viewD) { throw 'PRECONDITION: D 讀不到 get_settings' }
    $recD = Get-Placement $viewD 'quotes'
    Set-Content -Path $fixturePath -Value (New-Fixture -WithQuotes) -Encoding UTF8
    Log "## D 改寫 fixture（quotes 一筆 USD/TWD），等宿主輪詢（JsonFileSource 每 30 秒比對修改時間＋大小）；quotes 記錄=$(Fmt-P $recD)"
    $quotesD = Wait-Window $hostProc.Id 'fc-host quotes' 45
    $results['D：有資料後 quotes 在非編輯版面出現（≤45 秒）'] = ($quotesD -ne [IntPtr]::Zero)
    if ($quotesD -ne [IntPtr]::Zero) {
        $rD = Wait-StableRect $quotesD
        $expD = Grid-Rect (Get-WorkArea $quotesD) $recD
        $viewD2 = Get-SettingsView
        $lockedD = if ($viewD2) { $viewD2.locked } else { $null }
        $pD = Invoke-EvalJson 'w=quotes' $placeholderExpr
        Log "## D 出現：矩形=$(Fmt $rD) 期望（記錄的格子）=$(Fmt $expD) layout_locked=$lockedD 記錄後=$(if ($viewD2) { Fmt-P (Get-Placement $viewD2 'quotes') } else { '<null>' }) 外框=$(if ($pD) { $pD.display } else { 'null' })"
        $results['D：有資料後矩形＝記錄的格子（原格子）'] = Same $rD $expD
        $results['D：仍在鎖定狀態、記錄未變、佔位外框不顯示'] = ($lockedD -eq $true -and $viewD2 -and (Fmt-P (Get-Placement $viewD2 'quotes')) -eq (Fmt-P $recD) -and $pD -and $pD.display -eq 'none')
    }
}
catch {
    $msg = $_.Exception.Message
    if ($msg -like 'BLOCKED:*') {
        $script:blocked = $msg
        Log "## BLOCKED（執行途中鎖定，結果不完整）：$msg"
    } elseif ($msg -like 'ENV-BLOCKED*') {
        $script:envBlocked = $msg
        Log "## $msg（環境問題，結果不完整）"
    } elseif ($msg -like 'PRECONDITION*') {
        $script:precondition = $msg
        Log "## PRECONDITION FAIL（未送該步驟的輸入）：$msg"
        $results['前提成立（未在 PRECONDITION 中止）'] = $false
    } else {
        Log "## 例外中止：$msg（第 $($_.InvocationInfo.ScriptLineNumber) 行）"
        $results['腳本跑完（無例外）'] = $false
    }
}
finally {
    if ($script:mouseDown) {
        try { Invoke-GuardedMouse 4 -What '收尾：補放開（LEFTUP）' } catch { Log "## 收尾補放開失敗：$_" }
    }
    try { if ($hostProc) { $stopped = @(Stop-ProcessTree -Process $hostProc); Log "## 收尾：結束自己啟動的宿主行程樹 $($stopped -join ',')"; Start-Sleep -Seconds 2 } } catch { Log "## 收尾結束宿主失敗：$_" }
    try { Stop-ScratchForm $form } catch { Log "## 收尾結束前景表單失敗：$_" }
    try { Restore-Occluders -Minimized $minimized -NotRestored $occNotRestored -Log $occLog } catch { Log "## 還原被最小化的視窗失敗：$_" }
    # $minimized 非空＝Restore-Occluders 本身中途丟例外、沒跑完；一併列為未還原（環境，不進逐項結果）。
    if ($minimized.Count -gt 0) { $occNotRestored.Add("Restore-Occluders 未跑完：尚有 $($minimized.Count) 扇未處理（見記錄）") }
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
$sum.WriteLine("# verify-7.3-placeholder.ps1 summary $(Get-Ts) reloadMode=$ReloadMode")
foreach ($k in $results.Keys) { $sum.WriteLine("$(if ($results[$k]) { 'PASS' } else { 'FAIL' })  $k") }
if ($script:precondition) { $sum.WriteLine("PRECONDITION FAIL  $($script:precondition)") }
if ($script:blocked) { $sum.WriteLine("BLOCKED  執行途中鎖定，以上結果不完整：$($script:blocked)") }
if ($script:envBlocked) { $sum.WriteLine("ENV-BLOCKED  $($script:envBlocked)") }
if ($occWarn) { $sum.WriteLine($occWarn) }
$sum.Close()
Get-Content $sumPath
if ($occWarn) { Write-Host $occWarn -ForegroundColor Yellow }
# 結束碼優先序：產品 FAIL（1）＞ 鎖定（2）＞ 環境（3，含使用者視窗未還原）。
exit (Get-VerdictExitCode -Results $results -Locked:([bool]$script:blocked) -EnvBlocked:([bool]$script:envBlocked) -NotRestored $occNotRestored)
