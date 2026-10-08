<#
.SYNOPSIS
    Task 6.2 驗收驅動腳本（widget-adaptive-zoom-and-grid：編輯版面的格線疊加視窗；design.md D4，
    specs/widget-host-windows「編輯版面顯示格線」）。

.DESCRIPTION
    以 IPC（CDP 在小工具頁面呼叫宿主自己的 `set_edit_mode`，與系統匣「編輯版面」走同一個指令）進出
    編輯版面，列舉宿主行程的格線疊加視窗（類別與標題 `fc-host-grid-overlay`），驗證：
      1. 數量＝`[System.Windows.Forms.Screen]::AllScreens` 的台數，每台一個、矩形（GetWindowRect，
         實體像素）＝該台 `WorkingArea`（一對一）。
      2. 延伸樣式含 WS_EX_LAYERED(0x80000)／TRANSPARENT(0x20)／NOACTIVATE(0x8000000)／TOOLWINDOW(0x80)；
         另以 `SendMessageTimeout(WM_NCHITTEST)` 確認視窗程序回 HTTRANSPARENT（design.md D4 第二道保險）。
      3. 穿透：每台工作區挑「格子中心」與「加強線上」各一點，條件是該點除了格線與桌面（Progman／
         WorkerW）之外沒有任何可見、未 cloak、未最小化、會接受命中測試（不是 LAYERED＋TRANSPARENT）
         的頂層視窗——即使用者視窗、小工具、本腳本的表單都不在那裡。`WindowFromPoint` 的根視窗不得是
         格線，且必須是桌面；並記錄格線在 z-order 上是否位於該桌面視窗之上（＝判準有鑑別力：格線
         若沒有穿透，命中的就會是它）。整台工作區都被視窗蓋住時該台記 NOTE（不最小化使用者視窗），每一台都沒測到才記 NOT-RUN。
         穿透與鑑別力判定前先輪詢（每 50 ms、上限 2 秒）等 z-order 穩定＝每個格線都在與它重疊的桌面視窗之上：
         格線剛建立時 `SetWindowPos(HWND_BOTTOM)` 會暫時落到 Progman 之下，explorer 約百毫秒內再把 Progman
         壓回最底，取樣過早會誤報；等待時間記在 driver 記錄。逾時（任何格線仍在桌面視窗之下）另有一個結果項
         判 FAIL 並寫出逾時的格線與螢幕，不受上述「整台被蓋住」NOTE 的豁免影響。
      4. 前景：進入編輯版面前後 `GetForegroundWindow`（兩輪各比一次），對準規格「前景視窗與焦點不因格線而
         改變」：相同 → PASS；換成宿主行程（pid 比對）的視窗 → FAIL；換成別的行程的視窗 → NOT-RUN（不是
         宿主造成，但無法證明格線沒動到焦點，不得記為成功；結束碼 3，前後類別與 pid 列在摘要，前景穩定時重跑）。
         判定函式 `Get-ForegroundResult`／`Get-OverlayDesktopProblems` 在 lib/VerifyVerdict.psm1，
         tests/VerifyVerdict.Tests.ps1 以合成資料覆蓋（不需宿主）。watch-zorder 在建立自己的表單之前
         啟動，避免它的啟動過程干擾「進入前前景」。
      5. z-order：以 lib/ScratchWindow.psm1 開一扇自己的一般視窗（放在主螢幕工作區內，與格線重疊），
         以 `GetTopWindow`／`GetWindow(GW_HWNDNEXT)` 走訪：該表單在每一個格線之上，且格線之下沒有任何
         「一般視窗」（可見、未最小化、未 cloak、非 TOOLWINDOW、非零尺寸，不含宿主自己的視窗）。
         同時以 `watch-zorder.ps1 -ProcessId <宿主>` 背景取樣，事後判讀：每筆含格線的記錄 below=(none)。
         小工具與格線的相對位置只記錄、不判定（design.md D4「z-order 不硬搶」）。
      6. 線條位置：`PrintWindow(PW_RENDERFULLCONTENT)` 只擷取格線視窗本身（分層視窗的內容疊在黑底上，
         不含任何使用者視窗），在格子中心那一列／欄掃描：47 條垂直／水平線的位置＝
         `工作區起點 + floor(i × 長度 ÷ 48)`（design.md D7，與 layout.rs `grid_line_offsets` 同源），
         抽查第 6、12、24 條（加強線）在預期像素上有線、左右（上下）相鄰像素是黑底，且比第 1 條（一般線）
         亮。存 `grid-overlay-lines-m<N>.png`（只有線）與 `grid-overlay-composite-m<N>.png`（該台的小工具
         `PrintWindow` 依位置貼上、線疊在最上層，供目視線條與小工具邊緣對齊）。
      7. `set_edit_mode(false)` 後宿主行程的格線視窗數（含隱藏）＝0。
      8. 再進入→離開一次：數量與矩形同第一輪、沒有重複（過程中任何時點都不超過台數）、離開後歸零。
    結果彙整到 `evidence/grid-overlay-summary.log`（PASS／FAIL／NOT-RUN），過程記錄
    `grid-overlay-driver.log`、z-order 記錄 `grid-overlay-zorder.log`、宿主記錄 `grid-overlay-hostlog.log`。

    不注入任何鍵盤／滑鼠輸入；不最小化、不移動任何使用者視窗。工作階段鎖定時（LogonUI.exe 在跑）
    開始前就 BLOCKED，途中偵測到鎖定也停止並回報 BLOCKED（鎖定時截圖全黑，判讀無效）。

    隔離：全新暫存 %APPDATA%／%LOCALAPPDATA%（資料＝host/ui/fixtures/tw-events.json 複本）；真正的設定檔
    不讀內容，只比對開始與結束時的大小與修改時間；開機自啟登錄以 lib\AutostartRegistry.psm1 快照、還原。
    收尾只停自己啟動的宿主（lib\ProcessTree.psm1）。證據一律經 lib\EvidenceLog.psm1 去識別。

    結束碼：0＝全部通過；1＝有 FAIL；2＝BLOCKED（已有 fc-host 在跑或工作階段鎖定）；3＝有 NOT-RUN。

.PARAMETER Exe
    fc-host.exe 路徑，預設 host/target/release/fc-host.exe（**不含** self-test-ipc）。

.PARAMETER CdpPort
    本次專屬的 WebView2 remote debugging 埠（預設 9363）。
#>
[CmdletBinding()]
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\fc-host.exe'),
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [int]$CdpPort = 9363
)

$ErrorActionPreference = 'Stop'

Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\AutostartRegistry.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\ProcessTree.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\ScratchWindow.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\VerifyVerdict.psm1') -Force

Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
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
[DllImport("user32.dll", EntryPoint = "GetWindowLongW")] public static extern int GetWindowLong(System.IntPtr h, int index);
[DllImport("user32.dll")] public static extern System.IntPtr WindowFromPoint(POINT p);
[DllImport("user32.dll")] public static extern System.IntPtr GetAncestor(System.IntPtr h, uint flags);
[DllImport("user32.dll")] public static extern System.IntPtr GetForegroundWindow();
[DllImport("user32.dll")] public static extern bool PrintWindow(System.IntPtr h, System.IntPtr hdc, uint flags);
[DllImport("user32.dll")] public static extern System.IntPtr SendMessageTimeoutW(System.IntPtr h, uint msg, System.IntPtr w, System.IntPtr l, uint flags, uint timeout, out System.IntPtr result);
[DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(System.IntPtr h, int attr, out int value, int size);
[StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
[StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
'@

# Per-Monitor-V2（-4）：GetWindowRect／WindowFromPoint／Screen.AllScreens 都以實體像素解讀（tests/DpiAwareness.Tests.ps1）。
[void][VGrid.Native]::SetThreadDpiAwarenessContext([IntPtr](-4))

$GRID = 48
$OverlayClass = 'fc-host-grid-overlay'
$WS_EX = [ordered]@{ LAYERED = 0x80000; TRANSPARENT = 0x20; NOACTIVATE = 0x8000000; TOOLWINDOW = 0x80 }
$DesktopClasses = @('Progman', 'WorkerW')
$MajorLines = @(6, 12, 24)

function Get-Ts { Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fffK' }
function Test-Locked { [bool](Get-Process -Name LogonUI -ErrorAction SilentlyContinue) }
function Hex([IntPtr]$h) { '0x{0:X}' -f $h.ToInt64() }
function Get-Cls([IntPtr]$h) { $sb = New-Object System.Text.StringBuilder 256; [void][VGrid.Native]::GetClassName($h, $sb, 256); $sb.ToString() }
function Get-Title([IntPtr]$h) { $sb = New-Object System.Text.StringBuilder 256; [void][VGrid.Native]::GetWindowText($h, $sb, 256); $sb.ToString() }
function Get-WinPid([IntPtr]$h) { $p = 0; [void][VGrid.Native]::GetWindowThreadProcessId($h, [ref]$p); [int]$p }
function Get-Rect([IntPtr]$h) {
    $r = New-Object VGrid.Native+RECT
    [void][VGrid.Native]::GetWindowRect($h, [ref]$r)
    [PSCustomObject]@{ X = $r.Left; Y = $r.Top; W = $r.Right - $r.Left; H = $r.Bottom - $r.Top }
}
function Fmt($r) { if ($r) { "($($r.X),$($r.Y),$($r.W)x$($r.H))" } else { '<null>' } }
function Same($a, $b) { $a -and $b -and $a.X -eq $b.X -and $a.Y -eq $b.Y -and $a.W -eq $b.W -and $a.H -eq $b.H }
function Get-ExStyle([IntPtr]$h) { [int64][VGrid.Native]::GetWindowLong($h, -20) -band 0xFFFFFFFFL }
function Test-Cloaked([IntPtr]$h) { $v = 0; [void][VGrid.Native]::DwmGetWindowAttribute($h, 14, [ref]$v, 4); $v -ne 0 }
# design.md D7：edge(i) = 起點 + floor(i × 長度 / 48)（整數運算，同 layout.rs `edge`）。
function Edge([int]$Origin, [int]$Extent, [int]$I) { $Origin + [int][math]::Floor([int64]$I * $Extent / $GRID) }
function Test-InRect($R, [int]$X, [int]$Y) { $X -ge $R.X -and $X -lt ($R.X + $R.W) -and $Y -ge $R.Y -and $Y -lt ($R.Y + $R.H) }

# z-order 快照（由上到下）：GetTopWindow(NULL) 起沿 GW_HWNDNEXT 走訪，含隱藏視窗（另有上限防走訪途中重排成環）。
function Get-ZList {
    $list = New-Object System.Collections.Generic.List[object]
    $h = [VGrid.Native]::GetTopWindow([IntPtr]::Zero)
    $n = 0
    while ($h -ne [IntPtr]::Zero -and $n -lt 20000) {
        $vis = [VGrid.Native]::IsWindowVisible($h)
        $o = [PSCustomObject]@{
            Hwnd = $h; Index = $n; Pid = (Get-WinPid $h); Class = (Get-Cls $h); Visible = $vis
            Iconic = $false; Cloaked = $false; ExStyle = 0L; Rect = $null
        }
        if ($vis) {
            $o.Iconic = [VGrid.Native]::IsIconic($h)
            $o.Cloaked = Test-Cloaked $h
            $o.ExStyle = Get-ExStyle $h
            $o.Rect = Get-Rect $h
        }
        $list.Add($o)
        $h = [VGrid.Native]::GetWindow($h, 2)
        $n++
    }
    return , $list
}
# 宿主行程的格線視窗（含隱藏，供「離開後消失」判定）。
function Get-Overlays([int]$ProcId) {
    $zl = Get-ZList
    @($zl | Where-Object { $_.Pid -eq $ProcId -and $_.Class -eq $OverlayClass } | ForEach-Object {
            [PSCustomObject]@{
                Hwnd = $_.Hwnd; Index = $_.Index; Visible = $_.Visible; Rect = (Get-Rect $_.Hwnd)
                ExStyle = (Get-ExStyle $_.Hwnd); Title = (Get-Title $_.Hwnd)
            }
        })
}
function Wait-OverlayCount([int]$ProcId, [int]$Want, [int]$TimeoutSec = 12) {
    $sw = [Diagnostics.Stopwatch]::StartNew(); $max = 0; $last = @()
    while ($sw.Elapsed.TotalSeconds -lt $TimeoutSec) {
        $last = @(Get-Overlays $ProcId)
        if ($last.Count -gt $max) { $max = $last.Count }
        if (@($last | Where-Object { $_.Visible }).Count -eq $Want -and $last.Count -eq $Want) { break }
        Start-Sleep -Milliseconds 200
    }
    # 再穩定 1 秒、取最後一次，並把這段期間觀察到的最大數量一併回報（判「沒有重複」）。
    $sw2 = [Diagnostics.Stopwatch]::StartNew()
    while ($sw2.Elapsed.TotalSeconds -lt 1) {
        $last = @(Get-Overlays $ProcId)
        if ($last.Count -gt $max) { $max = $last.Count }
        Start-Sleep -Milliseconds 200
    }
    [PSCustomObject]@{ Overlays = $last; MaxSeen = $max; Elapsed = [math]::Round($sw.Elapsed.TotalSeconds, 1) }
}
function Find-Window([int]$ProcId, [string]$Title) {
    $h = [VGrid.Native]::GetTopWindow([IntPtr]::Zero)
    while ($h -ne [IntPtr]::Zero) {
        if ((Get-WinPid $h) -eq $ProcId -and [VGrid.Native]::IsWindowVisible($h) -and (Get-Title $h) -eq $Title) { return $h }
        $h = [VGrid.Native]::GetWindow($h, 2)
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
function Invoke-Eval([string]$Page, [string]$Expr) {
    $out = & node (Join-Path $PSScriptRoot 'host-cdp-eval.mjs') $CdpPort $Page $Expr 2>&1
    return ($out -join "`n").Trim()
}
function Wait-Eval([string]$Page, [string]$Expr, [string]$Pattern, [int]$TimeoutSec = 15) {
    $sw = [Diagnostics.Stopwatch]::StartNew(); $last = $null
    while ($sw.Elapsed.TotalSeconds -lt $TimeoutSec) {
        $last = Invoke-Eval $Page $Expr
        if ($last -match $Pattern) { return $last }
        Start-Sleep -Milliseconds 300
    }
    return $last
}
# PrintWindow（PW_RENDERFULLCONTENT＝2）只擷取單一視窗；分層視窗的內容會疊在黑底上（實測）。
function Get-WindowBitmap([IntPtr]$H) {
    $r = Get-Rect $H
    if ($r.W -le 0 -or $r.H -le 0) { return $null }   # 視窗已不存在或零尺寸（GetWindowRect 失敗時是 0×0）
    $bmp = New-Object System.Drawing.Bitmap $r.W, $r.H
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $hdc = $g.GetHdc()
    $ok = [VGrid.Native]::PrintWindow($H, $hdc, 2)
    $g.ReleaseHdc($hdc); $g.Dispose()
    if (-not $ok) { $bmp.Dispose(); return $null }
    return $bmp
}
function Get-Lum($c) { [int]$c.R + [int]$c.G + [int]$c.B }
# 證據檔是否為本次寫出：存在、非空、修改時間不早於 $NotBeforeUtc。
function Test-FreshFile([string]$Path, [datetime]$NotBeforeUtc) {
    $i = Get-Item -LiteralPath $Path -ErrorAction SilentlyContinue
    return [bool]($i -and $i.Length -gt 0 -and $i.LastWriteTimeUtc -ge $NotBeforeUtc)
}
# 存 PNG 並確認是本次的新檔（Codex review 6.x：失敗不得只記訊息、舊檔不得冒充）。回傳 $true／$false，失敗原因寫記錄。
function Save-FreshPng($Bmp, [string]$Path) {
    $t0 = [datetime]::UtcNow.AddSeconds(-1)
    try {
        if (Test-Path -LiteralPath $Path) { Remove-Item -LiteralPath $Path -Force -ErrorAction Stop }
        $Bmp.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png)
    } catch { Log "  存 $(Split-Path $Path -Leaf) 失敗：$($_.Exception.Message)"; return $false }
    if (-not (Test-FreshFile $Path $t0)) { Log "  存 $(Split-Path $Path -Leaf) 後找不到本次寫出的新檔"; return $false }
    return $true
}

# 一點的「桌面空白」判定：由上往下找第一個包含該點、會接受命中測試的頂層視窗（略過格線本身、隱藏、
# 最小化、cloaked、零尺寸、LAYERED＋TRANSPARENT）。回傳 @{ Blank; Hit（第一個命中者）}。
function Get-PointOccupant($ZList, [int]$X, [int]$Y, [Int64[]]$Skip) {
    foreach ($w in $ZList) {
        if ($Skip -contains $w.Hwnd.ToInt64()) { continue }
        if (-not $w.Visible -or $w.Iconic -or $w.Cloaked -or -not $w.Rect) { continue }
        if ($w.Rect.W -le 0 -or $w.Rect.H -le 0) { continue }
        if (($w.ExStyle -band $WS_EX.LAYERED) -and ($w.ExStyle -band $WS_EX.TRANSPARENT)) { continue }
        if (Test-InRect $w.Rect $X $Y) { return [PSCustomObject]@{ Blank = ($DesktopClasses -contains $w.Class); Hit = $w } }
    }
    return [PSCustomObject]@{ Blank = $false; Hit = $null }
}

# 等 z-order 穩定：輪詢（預設每 50 ms、上限 2 秒）直到每個格線都位於「與它重疊的可見桌面視窗（Progman／WorkerW）」
# 之上。回傳 @{ Settled; WaitedMs; Pending }：逾時（Settled=$false）時呼叫端必須判 FAIL（結果項寫出逾時的格線與螢幕），
# 不受「整台被視窗蓋住、沒有空白取樣點」那個 NOTE 豁免影響；等待時間與卡住的組合都記進記錄。
function Wait-OverlayAboveDesktop($Ov, [int]$TimeoutMs = 2000, [int]$IntervalMs = 50) {
    $ov = @($Ov)
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($true) {
        $zl = Get-ZList
        # 判定本體是 lib/VerifyVerdict.psm1 的純函式（tests/VerifyVerdict.Tests.ps1 以合成 z-order 覆蓋）。
        $pending = @(Get-OverlayDesktopProblems -ZList $zl -Overlays $ov -DesktopClasses $DesktopClasses -Screens $screens)
        if ($pending.Count -eq 0) {
            Log "  等待 z-order 穩定：$($sw.ElapsedMilliseconds) ms 後每個格線都在重疊的桌面視窗之上"
            return [PSCustomObject]@{ Settled = $true; WaitedMs = $sw.ElapsedMilliseconds; Pending = @() }
        }
        if ($sw.ElapsedMilliseconds -ge $TimeoutMs) {
            Log "  等待 z-order 穩定：逾時 $($sw.ElapsedMilliseconds) ms 仍未穩定（$($pending -join '；')）"
            return [PSCustomObject]@{ Settled = $false; WaitedMs = $sw.ElapsedMilliseconds; Pending = $pending }
        }
        Start-Sleep -Milliseconds $IntervalMs
    }
}

# 前景判準（規格：前景視窗與焦點不因格線而改變），結果值由 Get-ForegroundResult 決定：進入後前景與進入前相同 → PASS；
# 換成宿主行程的視窗（任何小工具或格線）→ FAIL；換成別的行程的視窗 → NOT-RUN（不是宿主造成，但不能當成功；
# 例如使用者點擊或終端機取回前景，重跑即可）。
function Get-ForegroundVerdictLogged([IntPtr]$Before, [IntPtr]$After, [int]$HostPid, [string]$Round) {
    $bPid = Get-WinPid $Before; $aPid = Get-WinPid $After
    $desc = "進入前 $(Hex $Before) class=$(Get-Cls $Before) pid=$bPid；進入後 $(Hex $After) class=$(Get-Cls $After) pid=$aPid"
    $v = Get-ForegroundResult -Before $Before -After $After -AfterPid $aPid -HostPid $HostPid
    if ($v -is [string]) {
        $notRun.Add("前景（$Round）：進入編輯版面前後前景視窗不同，新前景不屬於宿主（$desc），無法證明格線沒動到焦點，請在前景穩定時重跑")
        Log "  前景（$Round）：換成非宿主行程的視窗 → NOT-RUN（$desc）"
    } elseif ($v) { Log "  前景（$Round）：不變（$desc）" }
    else { Log "  前景（$Round）：換成宿主行程的視窗 → NG（$desc）" }
    return $v
}

# 穿透檢查（步驟 2）：每台挑桌面空白的格子中心與加強線上各一點，WindowFromPoint 的根視窗不得是格線、必須是桌面。
# 某一台整台都找不到空白點時記 NOTE（不最小化使用者視窗）。回傳 @{ Tested; AllOk; Discriminating }。
function Test-PassThrough($Ov) {
    $ov = @($Ov)
    $zl = Get-ZList
    $skip = [Int64[]]@($ov | ForEach-Object { $_.Hwnd.ToInt64() })
    $wfpAll = $true; $wfpTested = 0; $wfpDiscriminating = $true
    foreach ($o in $ov) {
        $r = $o.Rect
        $cands = New-Object System.Collections.Generic.List[object]
        # 格子中心（每 3 格取一個）與加強線（第 6／12／24 條垂直線）上的點。
        for ($row = 1; $row -lt $GRID; $row += 3) {
            $cy = [int]((Edge $r.Y $r.H $row) + (Edge $r.Y $r.H ($row + 1))) / 2
            for ($col = 1; $col -lt $GRID; $col += 3) {
                $cx = [int]((Edge $r.X $r.W $col) + (Edge $r.X $r.W ($col + 1))) / 2
                $cands.Add([PSCustomObject]@{ Kind = '格子中心'; X = [int]$cx; Y = [int]$cy })
            }
            foreach ($i in $MajorLines) { $cands.Add([PSCustomObject]@{ Kind = "第 $i 條垂直線上"; X = (Edge $r.X $r.W $i); Y = [int]$cy }) }
        }
        $picked = @{}
        $occupants = @{}
        foreach ($c in $cands) {
            $kind = if ($c.Kind -eq '格子中心') { 'center' } else { 'line' }
            if ($picked.ContainsKey($kind)) { continue }
            $occ = Get-PointOccupant $zl $c.X $c.Y $skip
            if ($occ.Blank) { $picked[$kind] = [PSCustomObject]@{ Point = $c; Desktop = $occ.Hit } }
            elseif ($occ.Hit) { $occupants[$occ.Hit.Class] = 1 + [int]$occupants[$occ.Hit.Class] }
        }
        if ($picked.Count -eq 0) {
            $notes.Add("2：$(Fmt $r) 整台工作區的取樣點都被視窗蓋住（$(($occupants.Keys | ForEach-Object { "$_×$($occupants[$_])" }) -join ', ')），未測穿透")
            Log "  $(Fmt $r)：找不到桌面空白點，略過（佔用者：$(($occupants.Keys | ForEach-Object { "$_×$($occupants[$_])" }) -join ', ')）"
            continue
        }
        foreach ($k in $picked.Keys) {
            $p = $picked[$k]
            $pt = New-Object VGrid.Native+POINT
            $pt.X = $p.Point.X; $pt.Y = $p.Point.Y
            $hit = [VGrid.Native]::WindowFromPoint($pt)
            $root = if ($hit -ne [IntPtr]::Zero) { [VGrid.Native]::GetAncestor($hit, 2) } else { [IntPtr]::Zero }
            $rootCls = if ($root -ne [IntPtr]::Zero) { Get-Cls $root } else { '<無>' }
            $isOverlay = ($skip -contains $hit.ToInt64()) -or ($skip -contains $root.ToInt64())
            $rootZ = @($zl | Where-Object { $_.Hwnd -eq $root } | Select-Object -First 1)
            $disc = ($rootZ.Count -eq 1 -and $o.Index -lt $rootZ[0].Index)
            $ok = (-not $isOverlay) -and ($DesktopClasses -contains $rootCls)
            $wfpTested++
            if (-not $ok) { $wfpAll = $false }
            if (-not $disc) { $wfpDiscriminating = $false }
            Log "  穿透 $($p.Point.Kind) ($($p.Point.X),$($p.Point.Y))：WindowFromPoint=$(Hex $hit) 根=$(Hex $root) class=$rootCls 是格線=$isOverlay；格線 z=$($o.Index) 桌面 z=$(if ($rootZ.Count) { $rootZ[0].Index } else { '?' }) 格線在其上（有鑑別力）=$disc → $(if ($ok) { 'OK' } else { 'NG' })"
        }
    }
    return [PSCustomObject]@{ Tested = $wfpTested; AllOk = $wfpAll; Discriminating = $wfpDiscriminating }
}

# 線條位置（步驟 4）：PrintWindow 只擷取格線視窗本身，逐像素掃描並存線條圖與合成圖。回傳 @{ LinesAll; MajorAll; ShotsOk }；
# ShotsOk＝每台的兩張圖都是本次寫出的新檔、合成圖沒有缺小工具。
function Test-GridLines($Ov, [int]$HostPid) {
    $ov = @($Ov)
    $hostPid = $HostPid
    $linesAll = $true; $majorAll = $true; $shotsOk = $true; $mIdx = 0
    foreach ($o in ($ov | Sort-Object { $_.Rect.X })) {
        $mIdx++
        $r = $o.Rect
        $bmp = Get-WindowBitmap $o.Hwnd
        if (-not $bmp) { $linesAll = $false; $majorAll = $false; $shotsOk = $false; Log "  m$mIdx PrintWindow 失敗"; continue }
        try {
            if (-not (Save-FreshPng $bmp (Join-Path $OutDir "grid-overlay-lines-m$mIdx.png"))) { $shotsOk = $false }
            # 掃描列／欄取第 0 格中心（不在任何水平／垂直線上）。
            $yScan = [int](((Edge 0 $r.H 0) + (Edge 0 $r.H 1)) / 2)
            $xScan = [int](((Edge 0 $r.W 0) + (Edge 0 $r.W 1)) / 2)
            $expX = @(1..47 | ForEach-Object { Edge 0 $r.W $_ })
            $expY = @(1..47 | ForEach-Object { Edge 0 $r.H $_ })
            $gotX = @(0..($r.W - 1) | Where-Object { (Get-Lum ($bmp.GetPixel($_, $yScan))) -gt 20 })
            $gotY = @(0..($r.H - 1) | Where-Object { (Get-Lum ($bmp.GetPixel($xScan, $_))) -gt 20 })
            $xEq = (($gotX -join ',') -eq ($expX -join ','))
            $yEq = (($gotY -join ',') -eq ($expY -join ','))
            Log "  m$mIdx 格線 $(Fmt $r)：第 $yScan 列掃到垂直線 $($gotX.Count) 條、與預期 47 條逐一相同=$xEq；第 $xScan 欄掃到水平線 $($gotY.Count) 條、相同=$yEq"
            if (-not $xEq) { Log "    垂直 預期=$($expX -join ',')"; Log "    垂直 實得=$($gotX -join ',')" }
            if (-not $yEq) { Log "    水平 預期=$($expY -join ',')"; Log "    水平 實得=$($gotY -join ',')" }
            if (-not ($xEq -and $yEq)) { $linesAll = $false }
            $minor = Get-Lum ($bmp.GetPixel((Edge 0 $r.W 1), $yScan))
            foreach ($i in $MajorLines) {
                $x = Edge 0 $r.W $i
                $c = $bmp.GetPixel($x, $yScan); $l = $bmp.GetPixel($x - 1, $yScan); $rt = $bmp.GetPixel($x + 1, $yScan)
                $okV = (Get-Lum $c) -gt $minor -and (Get-Lum $l) -le 20 -and (Get-Lum $rt) -le 20
                $y = Edge 0 $r.H $i
                $cy2 = $bmp.GetPixel($xScan, $y); $up = $bmp.GetPixel($xScan, $y - 1); $dn = $bmp.GetPixel($xScan, $y + 1)
                $okH = (Get-Lum $cy2) -gt $minor -and (Get-Lum $up) -le 20 -and (Get-Lum $dn) -le 20
                Log "    第 $i 條：垂直 x=工作區左+$x（螢幕 x=$($r.X + $x)）RGB=($($c.R),$($c.G),$($c.B)) 左右=($(Get-Lum $l),$(Get-Lum $rt)) → $okV；水平 y=工作區上+$y RGB=($($cy2.R),$($cy2.G),$($cy2.B)) 上下=($(Get-Lum $up),$(Get-Lum $dn)) → $okH（一般線亮度 $minor）"
                if (-not ($okV -and $okH)) { $majorAll = $false }
            }
            # 合成圖：該台上的小工具以 PrintWindow 貼在對應位置，格線（黑色當透明）疊在最上層。
            $comp = New-Object System.Drawing.Bitmap $r.W, $r.H
            $g = [System.Drawing.Graphics]::FromImage($comp)
            $g.Clear([System.Drawing.Color]::FromArgb(255, 32, 32, 32))
            $pasted = @()
            $zlNow = Get-ZList   # 先存變數再管線（函式以 `, $list` 回傳整個 List，直接接管線不會逐項展開）
            foreach ($w in ($zlNow | Where-Object { $_.Pid -eq $hostPid -and $_.Visible -and $_.Class -ne $OverlayClass })) {
                $title = Get-Title $w.Hwnd
                if ($title -notlike 'fc-host *') { continue }
                $wr = Get-Rect $w.Hwnd
                if (-not (Test-InRect $r ($wr.X + [int]($wr.W / 2)) ($wr.Y + [int]($wr.H / 2)))) { continue }
                $wb = Get-WindowBitmap $w.Hwnd
                if ($wb) { $g.DrawImage($wb, $wr.X - $r.X, $wr.Y - $r.Y, $wr.W, $wr.H); $wb.Dispose(); $pasted += $title }
                else { $shotsOk = $false; Log "  m$mIdx 合成圖：$title PrintWindow 失敗（合成圖會缺這個小工具）" }
            }
            $ia = New-Object System.Drawing.Imaging.ImageAttributes
            $ia.SetColorKey([System.Drawing.Color]::FromArgb(255, 0, 0, 0), [System.Drawing.Color]::FromArgb(255, 0, 0, 0))
            $g.DrawImage($bmp, (New-Object System.Drawing.Rectangle 0, 0, $r.W, $r.H), 0, 0, $r.W, $r.H, [System.Drawing.GraphicsUnit]::Pixel, $ia)
            $g.Dispose()
            try { if (-not (Save-FreshPng $comp (Join-Path $OutDir "grid-overlay-composite-m$mIdx.png"))) { $shotsOk = $false } }
            finally { $comp.Dispose() }
            Log "  m$mIdx 截圖：grid-overlay-lines-m$mIdx.png（只有格線）、grid-overlay-composite-m$mIdx.png（貼上 $($pasted.Count) 個小工具：$($pasted -join ', ')）"
        } finally { $bmp.Dispose() }
    }
    return [PSCustomObject]@{ LinesAll = $linesAll; MajorAll = $majorAll; ShotsOk = ($shotsOk -and $mIdx -gt 0) }
}

# z-order（步驟 3）：自己的表單在每一個格線之上；格線之下沒有一般視窗（可見、未最小化、未 cloak、非 TOOLWINDOW、
# 非零尺寸；宿主自己的視窗與桌面不算）。小工具與格線的相對位置只記錄（design.md D4）。回傳 @{ ScratchAbove; BelowNormal }。
function Test-OverlayZOrder($Ov, [IntPtr]$ScratchHwnd, [int]$HostPid) {
    $zl = Get-ZList
    $scratchZ = @($zl | Where-Object { $_.Hwnd -eq $ScratchHwnd })
    $zOk = ($scratchZ.Count -eq 1)
    $belowNormal = New-Object System.Collections.Generic.List[string]
    foreach ($o in @($Ov)) {
        $oz = @($zl | Where-Object { $_.Hwnd -eq $o.Hwnd })
        if ($oz.Count -ne 1 -or $scratchZ.Count -ne 1 -or $scratchZ[0].Index -ge $oz[0].Index) { $zOk = $false }
        foreach ($w in $zl) {
            if ($oz.Count -ne 1 -or $w.Index -le $oz[0].Index) { continue }
            if (-not $w.Visible -or $w.Iconic -or $w.Cloaked -or -not $w.Rect -or $w.Rect.W -le 0 -or $w.Rect.H -le 0) { continue }
            if ($w.ExStyle -band $WS_EX.TOOLWINDOW) { continue }
            if ($w.Pid -eq $HostPid -or $DesktopClasses -contains $w.Class) { continue }
            $belowNormal.Add("$(Hex $o.Hwnd) 之下：$(Hex $w.Hwnd) class=$($w.Class) z=$($w.Index)")
        }
        $widgetsBelow = @($zl | Where-Object { $_.Pid -eq $HostPid -and $_.Visible -and $_.Class -ne $OverlayClass -and $oz.Count -eq 1 -and $_.Index -gt $oz[0].Index } |
                ForEach-Object { Get-Title $_.Hwnd })
        Log "  z-order：格線 $(Hex $o.Hwnd) z=$(if ($oz.Count) { $oz[0].Index } else { '?' })；自己的表單 z=$(if ($scratchZ.Count) { $scratchZ[0].Index } else { '?' })；（只記錄）位於格線之下的宿主視窗：$(if ($widgetsBelow.Count) { $widgetsBelow -join ', ' } else { '無' })"
    }
    foreach ($b in $belowNormal) { Log "  格線之下的一般視窗：$b" }
    return [PSCustomObject]@{ ScratchAbove = $zOk; BelowNormal = $belowNormal }
}

# watch-zorder 記錄判讀：每筆含格線的記錄，格線 visible=1、未最小化、未 cloak、below=(none)（之下沒有一般視窗）。
# 回傳 @{ Lines（狀態記錄筆數）; WithOverlay（格線出現次數）; Bad（異常描述）}。
function Test-ZOrderLog([string]$Path) {
    $lines = @(Get-Content $Path | Where-Object { $_ -and $_ -notmatch '^#' })
    $withOverlay = 0; $bad = New-Object System.Collections.Generic.List[string]
    foreach ($ln in $lines) {
        $p = ConvertFrom-ZOrderLine $ln
        foreach ($h in $p.Win.Keys) {
            $w = $p.Win[$h]
            if (-not $w.ContainsKey('class') -or $w['class'] -ne $OverlayClass) { continue }
            $withOverlay++
            if ($w['visible'] -ne '1' -or $w['minimized'] -ne '0' -or $w['cloaked'] -ne '0' -or $w['below'] -ne '(none)') {
                $bad.Add("$($p.Ts.ToString('HH:mm:ss.fff')) $h visible=$($w['visible']) minimized=$($w['minimized']) cloaked=$($w['cloaked']) below=$($w['below'])")
            }
        }
    }
    return [PSCustomObject]@{ Lines = $lines.Count; WithOverlay = $withOverlay; Bad = $bad }
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
# 先清掉上次留下的截圖與複製進來的記錄（driver／summary 由 writer 覆寫），沒產生新檔時舊檔不得冒充本次結果。
Get-ChildItem -LiteralPath $OutDir -File -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -like 'grid-overlay-lines-m*.png' -or $_.Name -like 'grid-overlay-composite-m*.png' -or $_.Name -in @('grid-overlay-zorder.log', 'grid-overlay-hostlog.log') } |
    Remove-Item -Force -ErrorAction Stop
$logPath = Join-Path $OutDir 'grid-overlay-driver.log'
$sumPath = Join-Path $OutDir 'grid-overlay-summary.log'
$log = New-EvidenceWriter $logPath
$log.AutoFlush = $true
function Log([string]$m) { $line = "$(Get-Ts) $m"; $log.WriteLine($line); Write-Host (ConvertTo-EvidenceText $line) }
Log "# verify-grid-overlay.ps1 exe=$Exe cdpPort=$CdpPort"

$realSettings = Join-Path $env:APPDATA 'tw.fintools.fc-host\settings.json'
function Get-RealSettingsStamp { $i = Get-Item -LiteralPath $realSettings -ErrorAction SilentlyContinue; if ($i) { "$($i.Length)@$($i.LastWriteTimeUtc.Ticks)" } else { '<不存在>' } }
$realStampBefore = Get-RealSettingsStamp
$regSnap = @(Save-FcHostAutostartRegistry)
Log "# 真正的設定檔（只看大小與修改時間，不讀內容）開始=$realStampBefore；開機自啟登錄快照：$(($regSnap | ForEach-Object { "$($_.Name)@$(Split-Path $_.Key -Leaf) Exists=$($_.Exists)" }) -join '; ')"

$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ('fc-host-grid-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
$tempAppData = Join-Path $tempRoot 'Roaming'
$tempLocal = Join-Path $tempRoot 'Local'
$dataDir = Join-Path $tempLocal 'tw.fintools.fc-host\data'
New-Item -ItemType Directory -Force -Path $tempAppData, $dataDir | Out-Null
Copy-Item (Join-Path $PSScriptRoot '..\ui\fixtures\tw-events.json') (Join-Path $dataDir 'tw_events.json')
Log "# 暫存 APPDATA=$tempAppData LOCALAPPDATA=$tempLocal（資料：host/ui/fixtures/tw-events.json 複本）"

$screens = @([System.Windows.Forms.Screen]::AllScreens)
foreach ($s in $screens) { Log "# 螢幕 $($s.DeviceName) 主螢幕=$($s.Primary) bounds=$($s.Bounds) 工作區=$($s.WorkingArea)" }

$results = [ordered]@{}
$notRun = New-Object System.Collections.Generic.List[string]
# 不影響結束碼的備註（例如某一台的穿透沒測到；只要至少一台測到，該項仍照判定）。
$notes = New-Object System.Collections.Generic.List[string]
$hostProc = $null
$scratch = $null
$watcher = $null
$zlogRaw = Join-Path $tempRoot 'zorder.log'
$script:blocked = $null
function Assert-Unlocked([string]$Where) { if (Test-Locked) { throw "LOCKED：$Where 時偵測到工作階段鎖定" } }
function Set-Result([string]$Key, $Value) { $results[$Key] = $Value; Log "  => $(if ($Value -is [string]) { $Value } elseif ($Value) { 'PASS' } else { 'FAIL' })  $Key" }

# 一輪進入編輯版面後的格線檢查（第 1 輪做完整檢查，第 2 輪只驗數量、矩形與沒有重複）。
function Test-OverlaySet($Overlays, [string]$Round) {
    $vis = @($Overlays | Where-Object { $_.Visible })
    Set-Result "$Round：格線視窗數（可見）＝螢幕數 $($screens.Count)，且沒有隱藏的多餘視窗" ($vis.Count -eq $screens.Count -and $Overlays.Count -eq $screens.Count)
    $matched = @{}
    $allMatch = $true
    foreach ($o in $vis) {
        $hit = @($screens | Where-Object {
                $wa = $_.WorkingArea
                $o.Rect.X -eq $wa.X -and $o.Rect.Y -eq $wa.Y -and $o.Rect.W -eq $wa.Width -and $o.Rect.H -eq $wa.Height
            })
        if ($hit.Count -eq 1 -and -not $matched.ContainsKey($hit[0].DeviceName)) { $matched[$hit[0].DeviceName] = $true }
        else { $allMatch = $false }
        Log "  格線 $(Hex $o.Hwnd) 標題=$($o.Title) rect=$(Fmt $o.Rect) z=$($o.Index) exstyle=0x$('{0:X8}' -f $o.ExStyle) 對到螢幕=$(if ($hit.Count -eq 1) { $hit[0].DeviceName } else { '<無>' })"
    }
    Set-Result "$Round：每個格線矩形＝某台螢幕的 WorkingArea（一對一）" ($allMatch -and $matched.Count -eq $screens.Count)
}

try {
    # ── 0. 啟動宿主 ───────────────────────────────────────────────────────────────
    $old = @($env:APPDATA, $env:LOCALAPPDATA, $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS)
    try {
        $env:APPDATA = $tempAppData
        $env:LOCALAPPDATA = $tempLocal
        $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$CdpPort"
        $hostProc = Start-Process -FilePath $Exe -PassThru
    } finally {
        $env:APPDATA, $env:LOCALAPPDATA, $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $old
    }
    $hostPid = $hostProc.Id
    Log "# 宿主 pid=$hostPid"
    $clock = Wait-Window $hostPid 'fc-host clock'
    if ($clock -eq [IntPtr]::Zero) { throw 'clock 視窗未出現' }
    foreach ($id in 'macro', 'fixed', 'dynamic') { [void](Wait-Window $hostPid "fc-host $id" 30) }
    [void](Wait-Eval 'w=clock' "window.__TAURI__.core.invoke('get_settings').then(s => s.layout_locked)" 'true|false')
    Start-Sleep -Seconds 2
    $pre = @(Get-Overlays $hostPid)
    Log "## 鎖定中（尚未進入編輯版面）的格線視窗數＝$($pre.Count)"
    Set-Result '0：版面鎖定時沒有格線視窗' ($pre.Count -eq 0)

    # ── 自己的一般視窗（z-order 對照）＋ watch-zorder ──────────────────────────────────
    # watch-zorder 要在建立自己的表單**之前**啟動：它是新行程，啟動過程可能把前景移走（實測使用者從終端機
    # 執行時，進入前前景＝剛建立的表單、進入後卻是終端機）；先啟動並等它穩定，之後取「進入前前景」才不會被它干擾。
    # 只給 -ProcessId（不給 -TargetHwnd）：判讀只看含格線的記錄，不需要表單的 HWND。
    $watcher = Start-Process pwsh -PassThru -WindowStyle Hidden -ArgumentList @(
        '-NoProfile', '-File', (Join-Path $PSScriptRoot 'watch-zorder.ps1'), '-ProcessId', "$hostPid",
        '-OutFile', $zlogRaw, '-DurationSec', '150', '-IntervalMs', '200', '-BelowCount', '8', '-Quiet')
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt 20 -and -not (Test-Path $zlogRaw)) { Start-Sleep -Milliseconds 200 }
    Start-Sleep -Seconds 2
    Log "## watch-zorder pid=$($watcher.Id) 記錄已建立=$(Test-Path $zlogRaw)"
    $primary = @($screens | Where-Object { $_.Primary })[0]
    $pwa = $primary.WorkingArea
    $scratch = Start-ScratchForm -X ($pwa.X + 80) -Y ($pwa.Y + 80) -Width 520 -Height 360
    Log "## 自己的表單 $(Hex $scratch.Hwnd) 標題=$($scratch.Title) rect=$(Fmt (Get-Rect $scratch.Hwnd))"
    Start-Sleep -Seconds 1   # 讓表單搶前景的動作落定，再取進入前的前景

    # ── 1. 第一輪進入編輯版面 ─────────────────────────────────────────────────────────
    Assert-Unlocked '第一輪進入編輯版面前'
    $fg0 = [VGrid.Native]::GetForegroundWindow()
    Log "## 進入前前景 $(Hex $fg0) class=$(Get-Cls $fg0) pid 為宿主=$((Get-WinPid $fg0) -eq $hostPid) 為自己的表單=$($fg0 -eq $scratch.Hwnd)"
    [void](Invoke-Eval 'w=clock' "window.__TAURI__.core.invoke('set_edit_mode', { enabled: true })")
    $locked1 = Wait-Eval 'w=clock' "window.__TAURI__.core.invoke('get_settings').then(s => s.layout_locked)" 'false'
    $w1 = Wait-OverlayCount $hostPid $screens.Count
    $fg1 = [VGrid.Native]::GetForegroundWindow()
    Log "## 第一輪進入：layout_locked=$locked1 格線 $($w1.Overlays.Count) 個（$($w1.Elapsed)s；期間最多 $($w1.MaxSeen)）；前景 $(Hex $fg1) class=$(Get-Cls $fg1)"
    Set-Result '1：set_edit_mode(true) → layout_locked=false' ($locked1 -match 'false')
    Test-OverlaySet $w1.Overlays '1'
    $ov = @($w1.Overlays | Where-Object { $_.Visible })
    $exOk = $true
    foreach ($o in $ov) {
        $miss = @($WS_EX.Keys | Where-Object { -not ($o.ExStyle -band $WS_EX[$_]) })
        if ($miss.Count -gt 0) { $exOk = $false; Log "  $(Hex $o.Hwnd) 缺少延伸樣式：$($miss -join ',')" }
    }
    Set-Result '1：延伸樣式含 LAYERED、TRANSPARENT、NOACTIVATE、TOOLWINDOW' ($exOk -and $ov.Count -gt 0)
    $htOk = $true
    foreach ($o in $ov) {
        $cx = $o.Rect.X + [int]($o.Rect.W / 2); $cy = $o.Rect.Y + [int]($o.Rect.H / 2)
        $lp = [IntPtr](([int64]($cy -band 0xFFFF) -shl 16) -bor [int64]($cx -band 0xFFFF))
        $res = [IntPtr]::Zero
        $sent = [VGrid.Native]::SendMessageTimeoutW($o.Hwnd, 0x84, [IntPtr]::Zero, $lp, 0x2, 2000, [ref]$res)
        Log "  WM_NCHITTEST $(Hex $o.Hwnd) @($cx,$cy) 送達=$($sent -ne [IntPtr]::Zero) 回傳=$($res.ToInt64())（HTTRANSPARENT＝-1）"
        if ($sent -eq [IntPtr]::Zero -or $res.ToInt64() -ne -1) { $htOk = $false }
    }
    Set-Result '1：視窗程序對 WM_NCHITTEST 回 HTTRANSPARENT' ($htOk -and $ov.Count -gt 0)
    Set-Result '1：進入編輯版面前後前景視窗不變' (Get-ForegroundVerdictLogged $fg0 $fg1 $hostPid '第一輪')

    # ── 2. 穿透（WindowFromPoint）─────────────────────────────────────────────────────
    Assert-Unlocked '穿透檢查前'
    # 格線剛建立時 SetWindowPos(HWND_BOTTOM) 會暫時落到 Progman 之下，explorer 隨後（約百毫秒內）把 Progman 再壓回最底；
    # 先等 z-order 穩定再判穿透與鑑別力，否則取樣過早會誤報「格線不在桌面之上」。
    $settle = Wait-OverlayAboveDesktop $ov
    Set-Result "2：進入編輯版面後 2 秒內每個格線都位於重疊的桌面視窗之上（z-order 穩定）$(if (-not $settle.Settled) { "；逾時：$($settle.Pending -join '；')" })" $settle.Settled
    $pass = Test-PassThrough $ov
    if ($pass.Tested -gt 0) {
        Set-Result '2：桌面空白處（格子中心／加強線上）WindowFromPoint 不回傳格線、回傳桌面' $pass.AllOk
        Set-Result '2：上項有鑑別力（格線在 z-order 上位於被命中的桌面視窗之上）' $pass.Discriminating
    } else {
        Set-Result '2：桌面空白處（格子中心／加強線上）WindowFromPoint 不回傳格線、回傳桌面' 'SKIPPED'
    }

    # ── 3. z-order：自己的表單在格線之上、格線之下沒有一般視窗 ─────────────────────────────
    $zr3 = Test-OverlayZOrder $ov $scratch.Hwnd $hostPid
    Set-Result '3：自己的一般視窗（表單）在每一個格線之上' $zr3.ScratchAbove
    Set-Result '3：格線之下沒有任何一般視窗（不含宿主自己的視窗與桌面）' ($zr3.BelowNormal.Count -eq 0 -and $ov.Count -gt 0)

    # ── 4. 線條位置（PrintWindow 只擷取格線視窗本身）─────────────────────────────────────
    Assert-Unlocked '擷取格線前'
    $lines = Test-GridLines $ov $hostPid
    Set-Result '4：47 條垂直＋47 條水平線的位置＝工作區起點 + floor(i × 長度 ÷ 48)（PrintWindow 逐像素掃描）' ($lines.LinesAll -and $ov.Count -gt 0)
    Set-Result '4：第 6、12、24 條（加強線）在預期像素上、相鄰像素為黑底、比一般線亮' ($lines.MajorAll -and $ov.Count -gt 0)
    Set-Result '4：截圖 grid-overlay-lines／composite-m<N>.png 都是本次 PrintWindow 新檔（合成圖沒有缺小工具）' ($lines.ShotsOk -and $ov.Count -gt 0)

    # ── 5. 離開編輯版面 → 格線消失 ────────────────────────────────────────────────────
    Assert-Unlocked '第一輪離開前'
    [void](Invoke-Eval 'w=clock' "window.__TAURI__.core.invoke('set_edit_mode', { enabled: false })")
    $locked2 = Wait-Eval 'w=clock' "window.__TAURI__.core.invoke('get_settings').then(s => s.layout_locked)" 'true'
    $l1 = Wait-OverlayCount $hostPid 0
    Log "## 第一輪離開：layout_locked=$locked2 格線剩 $($l1.Overlays.Count) 個（含隱藏；$($l1.Elapsed)s）"
    Set-Result '5：set_edit_mode(false) 後格線視窗（含隱藏）數量＝0' ($locked2 -match 'true' -and $l1.Overlays.Count -eq 0)

    # ── 6. 第二輪：進入→離開，沒有殘留或重複 ─────────────────────────────────────────────
    Assert-Unlocked '第二輪進入前'
    $fg2 = [VGrid.Native]::GetForegroundWindow()
    [void](Invoke-Eval 'w=clock' "window.__TAURI__.core.invoke('set_edit_mode', { enabled: true })")
    $w2 = Wait-OverlayCount $hostPid $screens.Count
    $fg3 = [VGrid.Native]::GetForegroundWindow()
    $newHwnds = @($w2.Overlays | Where-Object { $ov.Hwnd -notcontains $_.Hwnd }).Count
    Log "## 第二輪進入：格線 $($w2.Overlays.Count) 個（期間最多 $($w2.MaxSeen)；新 HWND $newHwnds 個）；前景前後 $(Hex $fg2)→$(Hex $fg3)"
    Test-OverlaySet $w2.Overlays '6'
    Set-Result '6：第二輪進入過程中格線數從未超過螢幕數（沒有重複）' ($w2.MaxSeen -le $screens.Count)
    Set-Result '6：第二輪進入前後前景視窗不變' (Get-ForegroundVerdictLogged $fg2 $fg3 $hostPid '第二輪')
    [void](Invoke-Eval 'w=clock' "window.__TAURI__.core.invoke('set_edit_mode', { enabled: false })")
    $l2 = Wait-OverlayCount $hostPid 0
    Log "## 第二輪離開：格線剩 $($l2.Overlays.Count) 個（含隱藏）"
    Set-Result '6：第二輪離開後格線視窗（含隱藏）數量＝0' ($l2.Overlays.Count -eq 0)
    Start-Sleep -Seconds 2
}
catch {
    if ("$($_.Exception.Message)" -like 'LOCKED*') {
        $script:blocked = $_.Exception.Message
        Log "## BLOCKED：$($script:blocked)（結果不完整）"
    } else {
        Log "## 例外中止：$($_.Exception.Message)（第 $($_.InvocationInfo.ScriptLineNumber) 行：$($_.InvocationInfo.Line.Trim())）"
        $results['腳本跑完（無例外）'] = $false
    }
}
finally {
    # 每一項收尾各自 try，任何一項失敗都不影響其餘。
    try { if ($watcher) { [void](Stop-ProcessTree -Process $watcher) } } catch { Log "## 收尾停止 watch-zorder 失敗：$_" }
    try { Stop-ScratchForm $scratch } catch { Log "## 收尾關閉自己的表單失敗：$_" }
    try { if ($hostProc) { [void](Stop-ProcessTree -Process $hostProc) } } catch { Log "## 收尾結束宿主失敗：$_" }
    Start-Sleep -Seconds 2
    try {
        $regLeft = @(Restore-FcHostAutostartRegistry $regSnap)
        if ($regLeft.Count -gt 0) { Write-Warning "開機自啟登錄未還原：$($regLeft -join '; ')" }
        Log "# 開機自啟登錄還原：未還原 $($regLeft.Count) 項 $($regLeft -join '; ')"
        $results['開機自啟登錄已還原（Run／StartupApproved 的 fc-host）'] = ($regLeft.Count -eq 0)
    } catch {
        Log "## 收尾開機自啟登錄還原失敗：$_"
        $results['開機自啟登錄已還原（Run／StartupApproved 的 fc-host）'] = $false
    }
    $realStampAfter = Get-RealSettingsStamp
    Log "# 真正的設定檔（大小@修改時間）結束=$realStampAfter；與開始相同=$($realStampAfter -eq $realStampBefore)"
    $results['隔離：真正的設定檔大小與修改時間前後相同'] = ($realStampAfter -eq $realStampBefore)
    # z-order 記錄判讀：每筆含格線的記錄，格線 visible=1、未最小化、未 cloak、below=(none)。
    try {
        if (Test-Path $zlogRaw) {
            Copy-EvidenceFile -Source $zlogRaw -Destination (Join-Path $OutDir 'grid-overlay-zorder.log')
            $zr = Test-ZOrderLog $zlogRaw
            foreach ($b in $zr.Bad) { Log "  z-order 記錄異常：$b" }
            Log "# watch-zorder：$($zr.Lines) 筆狀態記錄，含格線者 $($zr.WithOverlay) 筆，異常 $($zr.Bad.Count) 筆"
            $results['3：watch-zorder 記錄中格線一律可見且之下沒有一般視窗（below=(none)）'] = ($zr.WithOverlay -gt 0 -and $zr.Bad.Count -eq 0)
        } else {
            $results['3：watch-zorder 記錄中格線一律可見且之下沒有一般視窗（below=(none)）'] = $false
            Log '## watch-zorder 記錄不存在'
        }
    } catch { Log "## z-order 記錄判讀失敗：$_"; $results['3：watch-zorder 記錄中格線一律可見且之下沒有一般視窗（below=(none)）'] = $false }
    try {
        $hostLogs = @(Get-ChildItem (Join-Path $tempLocal 'tw.fintools.fc-host\logs') -Filter *.log -ErrorAction SilentlyContinue | Sort-Object LastWriteTime)
        if ($hostLogs.Count -gt 0) { Copy-EvidenceFile -Source $hostLogs[-1].FullName -Destination (Join-Path $OutDir 'grid-overlay-hostlog.log') }
    } catch { Log "## 複製宿主記錄失敗：$_" }
    Log '# 結束'
    $log.Close()
    Remove-Item -Recurse -Force $tempRoot -ErrorAction SilentlyContinue
}

$sum = New-EvidenceWriter $sumPath
$sum.WriteLine("# verify-grid-overlay.ps1 summary $(Get-Ts)")
foreach ($k in $results.Keys) {
    $v = $results[$k]
    $tag = if ($v -is [string]) { 'NOT-RUN' } elseif ($v) { 'PASS' } else { 'FAIL' }
    $sum.WriteLine("$tag  $k")
}
foreach ($n in $notRun) { $sum.WriteLine("NOT-RUN  $n") }
foreach ($n in $notes) { $sum.WriteLine("NOTE  $n") }
if ($script:blocked) { $sum.WriteLine("BLOCKED  $($script:blocked)") }
$sum.Close()
Get-Content $sumPath
if ($script:blocked) { exit (Get-VerdictExitCode -Results $results -Locked) }
exit (Get-VerdictExitCode -Results $results -EnvBlocked:($notRun.Count -gt 0 -or @($results.Values | Where-Object { $_ -is [string] }).Count -gt 0) -RequireResults)
