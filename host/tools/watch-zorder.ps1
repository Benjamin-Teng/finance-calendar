<#
.SYNOPSIS
    z-order／視窗狀態監控腳本（task 1.0）。用來當桌面行為驗收（Win+D、explorer 重啟、
    保險重排等）的證據工具：每 0.5 秒（可調）取樣一次指定視窗的 z-order 狀態，
    只在狀態有變化時寫一行記錄，可指定輸出檔與持續秒數，時間到自動結束（供無人值守跑）。

.DESCRIPTION
    監控目標＝ -TargetHwnd 給的明確 HWND，以及／或 -ProcessId 給的行程目前所有「可見頂層視窗」
    （每次取樣都重新掃描，故行程在啟動後才建立的視窗〔例如宿主稍後才開出的小工具視窗〕會自動加入）。

    每次取樣做法：從 GetTopWindow(NULL) 開始、沿 GetWindow(hWnd, GW_HWNDNEXT) 走訪，
    取得由上到下（topmost 在前）的完整頂層視窗清單，一次列舉即可同時算出所有監控目標的
    z-order 關係，避免多次列舉造成的競態。

    「上方可見視窗數」與「下方可見視窗類別」**刻意用不同判準**：

    - 「上方可見視窗數」（above）＝brief 原文定義：目標視窗在 z-order 清單中，索引小於
      它（排在它前面／更靠上）、且同時滿足「可見（IsWindowVisible）、非最小化
      （!IsIconic）、非 cloaked（DwmGetWindowAttribute(DWMWA_CLOAKED)==0）」三條的
      視窗數量。**不**額外排除 tool window 或零尺寸視窗——一個可見的 tool window
      （例如浮動面板）壓在目標上方時，也要算進 above，不能被判準吃掉。
    - 「下方可見視窗類別」（below）＝索引大於目標視窗（排在它後面／更靠下）且滿足
      「一般視窗」判準（見下）的視窗類別名稱，由近到遠列出前 -BelowCount 個（預設 5
      個），超過的部分只計數不全列。這裡刻意排除 tool window／零尺寸，因為「下方視窗
      類別」是用來判斷「目標是否真的被一般應用程式視窗蓋住」，工具面板／隱藏代理視窗
      不是這裡要盯的東西。

    「一般視窗」判準（只用於「下方可見視窗類別」的文字清單）：同時滿足以下條件才算
    「一般視窗」──
      1. IsWindowVisible = true（注意：視窗被 Minimize 時 IsWindowVisible 仍可能是 true，
         WS_VISIBLE 位元不會因為最小化而被清除，所以視覺上「看不看得到」不能只看這個 API）
      2. IsIconic = false（未最小化——上一點的註記，所以另外用這個 API 排除最小化視窗）
      3. 未被 DWM cloak（DwmGetWindowAttribute(DWMWA_CLOAKED) == 0；cloaked 視窗
         IsWindowVisible 可能仍回傳 true，但實際上不會畫出來，常見於虛擬桌面切走的視窗、
         UWP 暫止的視窗）
      4. 不是 tool window（GWL_EXSTYLE 未含 WS_EX_TOOLWINDOW；工具視窗不進工作列、
         語意上不算一般應用程式視窗）
      5. 視窗矩形非零尺寸（GetWindowRect 寬高皆 > 0；排除隱藏用的 0×0 訊息代理視窗）

    桌面視窗（Progman／WorkerW）判斷——**不經上述「一般視窗」判準**：Progman 本身帶
    WS_EX_TOOLWINDOW，套用「一般視窗」判準會被 tool window 條件排除、永遠偵測不到，
    所以桌面視窗的偵測獨立於 below 文字清單之外，只看可見、非 cloaked：
      - hasProgman／hasWorkerW（below 方向）：完整下方 z-order 清單中（不經一般視窗
        篩選），是否存在可見、非 cloaked、類別為 Progman／WorkerW 的視窗。
      - desktopAbove（above 方向）：完整上方 z-order 清單中，是否存在可見、非 cloaked
        的「桌面 host」視窗——Progman 本身，或帶 SHELLDLL_DefView 子視窗的 WorkerW
        （explorer 承載桌面圖示的那個 WorkerW；區分它與其他跟桌面圖示無關的 WorkerW，
        例如桌布 host 自己開的）。這是 Win+D 判準（design.md D8）的核心：桌面視窗
        「實際位於 z-order 頂端／在小工具之上」，直接看 desktopAbove 就夠，不用另外
        拼湊 above 數字或 below 清單去猜。

    目標視窗自己的原始狀態（visible／minimized／cloaked）三個欄位都會輸出、且都納入
    「只在狀態變化時寫一行」的比較。**可見性驗收要同時看 visible、minimized、cloaked
    三個欄位**，不能只看 visible（視窗被最小化或被 DWM cloak 時，IsWindowVisible
    仍可能回傳 true）。

.PARAMETER ProcessId
    要監控的行程 PID（可多個）。腳本會在每次取樣時，把該 PID 目前所有可見頂層視窗
    （IsWindowVisible=true）都納入監控目標。

.PARAMETER ProcessName
    要監控的行程名稱（不含 .exe，可多個；task 3.1 新增）。每次取樣都以 Get-Process 重新解析
    成 PID，再比照 -ProcessId 納入該行程目前所有可見頂層視窗。用途：監控要在**目標行程啟動
    之前**就開始（例如驗「小工具從第一次顯示起就置底」），此時還不知道 PID；等 header 行寫出
    後再啟動目標行程，第一筆含目標視窗的記錄就是它第一次可見時的狀態。

.PARAMETER TargetHwnd
    要監控的明確 HWND（可多個），接受十進位或 0x 開頭十六進位字串。
    視窗若在取樣當下已不存在於 z-order 清單中（可能已關閉），該行會標記 status=not_found。

.PARAMETER OutFile
    輸出記錄檔路徑（必填）。每次執行會覆寫（不是續寫）。

.PARAMETER DurationSec
    監控總秒數，時間到自動結束（預設 60 秒）。

.PARAMETER IntervalMs
    取樣間隔毫秒數（預設 500，對應「每 0.5 秒」）。

.PARAMETER BelowCount
    「下方可見視窗類別」最多列出幾個（預設 5）。

.PARAMETER Quiet
    不在主控台同步印出寫入的行（預設會印出，方便人工跟盤）。

.EXAMPLE
    # 監控單一 PID（例如 notepad）30 秒，每 0.5 秒取樣、只在變化時寫檔
    pwsh -File host/tools/watch-zorder.ps1 -ProcessId 12345 -OutFile host/tools/run.log -DurationSec 30

.EXAMPLE
    # 同時監控多個明確 HWND（多個小工具視窗）
    pwsh -File host/tools/watch-zorder.ps1 -TargetHwnd 0x000A21FE,0x000B3140 -OutFile host/tools/run.log -DurationSec 120

.NOTES
    輸出每行格式（時間戳含毫秒＋UTC 偏移，易 grep）：

    2026-09-27T21:05:03.512+08:00 explorerPid=8420 fgClass=Progman fgPid=8420 win[0x000A21FE].class=Notepad win[0x000A21FE].visible=1 win[0x000A21FE].minimized=0 win[0x000A21FE].cloaked=0 win[0x000A21FE].above=0 win[0x000A21FE].desktopAbove=1 win[0x000A21FE].below=(none) win[0x000A21FE].hasProgman=0 win[0x000A21FE].hasWorkerW=0

    以 `#` 開頭的行是 session 起訖的說明行，不是狀態行，驗收腳本 grep 狀態時記得排除
    （例如 `grep -v '^#'` 或 `Select-String -NotMatch '^#'`）。
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory = $false)]
    [int[]]$ProcessId = @(),

    [Parameter(Mandatory = $false)]
    [string[]]$ProcessName = @(),

    [Parameter(Mandatory = $false)]
    [string[]]$TargetHwnd = @(),

    [Parameter(Mandatory = $true)]
    [string]$OutFile,

    [Parameter(Mandatory = $false)]
    [int]$DurationSec = 60,

    [Parameter(Mandatory = $false)]
    [int]$IntervalMs = 500,

    [Parameter(Mandatory = $false)]
    [int]$BelowCount = 5,

    [switch]$Quiet
)

$ErrorActionPreference = 'Stop'

if ($ProcessId.Count -eq 0 -and $ProcessName.Count -eq 0 -and $TargetHwnd.Count -eq 0) {
    throw "必須至少指定一個 -ProcessId、-ProcessName 或 -TargetHwnd"
}

Add-Type -Namespace FcHost -Name Native -MemberDefinition @'
[DllImport("user32.dll")]
public static extern System.IntPtr GetTopWindow(System.IntPtr hWnd);

[DllImport("user32.dll")]
public static extern System.IntPtr GetWindow(System.IntPtr hWnd, uint uCmd);

[DllImport("user32.dll")]
public static extern bool IsWindowVisible(System.IntPtr hWnd);

[DllImport("user32.dll")]
public static extern bool IsIconic(System.IntPtr hWnd);

[DllImport("user32.dll")]
public static extern bool IsWindow(System.IntPtr hWnd);

[DllImport("user32.dll", CharSet = System.Runtime.InteropServices.CharSet.Auto)]
public static extern int GetClassName(System.IntPtr hWnd, System.Text.StringBuilder lpClassName, int nMaxCount);

[System.Runtime.InteropServices.StructLayout(System.Runtime.InteropServices.LayoutKind.Sequential)]
public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }

[DllImport("user32.dll")]
public static extern bool GetWindowRect(System.IntPtr hWnd, out RECT lpRect);

[DllImport("user32.dll", EntryPoint = "GetWindowLong")]
public static extern int GetWindowLong32(System.IntPtr hWnd, int nIndex);

[DllImport("user32.dll", EntryPoint = "GetWindowLongPtr")]
public static extern System.IntPtr GetWindowLongPtr64(System.IntPtr hWnd, int nIndex);

public static long GetWindowLongAny(System.IntPtr hWnd, int nIndex) {
    if (System.IntPtr.Size == 8) {
        return GetWindowLongPtr64(hWnd, nIndex).ToInt64();
    }
    return (long)GetWindowLong32(hWnd, nIndex);
}

[DllImport("user32.dll")]
public static extern uint GetWindowThreadProcessId(System.IntPtr hWnd, out uint lpdwProcessId);

[DllImport("user32.dll")]
public static extern System.IntPtr GetForegroundWindow();

[DllImport("user32.dll")]
public static extern System.IntPtr GetShellWindow();

[DllImport("dwmapi.dll")]
public static extern int DwmGetWindowAttribute(System.IntPtr hwnd, int dwAttribute, out int pvAttribute, int cbAttribute);

[DllImport("user32.dll", CharSet = System.Runtime.InteropServices.CharSet.Auto)]
public static extern System.IntPtr FindWindowEx(System.IntPtr hwndParent, System.IntPtr hwndChildAfter, string lpszClass, string lpszWindow);
'@

$GW_HWNDNEXT = 2
$GWL_EXSTYLE = -20
$WS_EX_TOOLWINDOW = 0x00000080
$DWMWA_CLOAKED = 14

function Get-WindowClassName {
    param([IntPtr]$Hwnd)
    $sb = New-Object System.Text.StringBuilder 256
    [void][FcHost.Native]::GetClassName($Hwnd, $sb, 256)
    return $sb.ToString()
}

function Get-ZOrderSnapshot {
    # 由上到下（topmost 在前）列出目前所有頂層視窗的狀態
    $list = New-Object System.Collections.Generic.List[object]
    $h = [FcHost.Native]::GetTopWindow([IntPtr]::Zero)
    while ($h -ne [IntPtr]::Zero) {
        $visible = [FcHost.Native]::IsWindowVisible($h)
        $iconic = [FcHost.Native]::IsIconic($h)

        $cloakedVal = 0
        [void][FcHost.Native]::DwmGetWindowAttribute($h, $DWMWA_CLOAKED, [ref]$cloakedVal, 4)
        $cloaked = ($cloakedVal -ne 0)

        $rect = New-Object FcHost.Native+RECT
        $hasRect = [FcHost.Native]::GetWindowRect($h, [ref]$rect)
        $w = $rect.Right - $rect.Left
        $ht = $rect.Bottom - $rect.Top

        $exStyle = [FcHost.Native]::GetWindowLongAny($h, $GWL_EXSTYLE)
        $toolWindow = (($exStyle -band $WS_EX_TOOLWINDOW) -ne 0)

        $procId = 0
        [void][FcHost.Native]::GetWindowThreadProcessId($h, [ref]$procId)

        $normal = $visible -and (-not $iconic) -and (-not $cloaked) -and (-not $toolWindow) -and $hasRect -and ($w -gt 0) -and ($ht -gt 0)

        $class = Get-WindowClassName $h
        # 桌面視窗判別：Progman 本身，或帶 SHELLDLL_DefView 子視窗的 WorkerW（explorer
        # 用來承載桌面圖示的那個 WorkerW；同時可能存在其他跟桌面圖示無關的 WorkerW，
        # 例如桌布 host 自己開的），這是判斷「桌面是否真的在最上層」的標準技巧。
        $isDesktopHost = $false
        if ($class -eq 'Progman') {
            $isDesktopHost = $true
        } elseif ($class -eq 'WorkerW') {
            $defView = [FcHost.Native]::FindWindowEx($h, [IntPtr]::Zero, 'SHELLDLL_DefView', $null)
            $isDesktopHost = ($defView -ne [IntPtr]::Zero)
        }

        $list.Add([PSCustomObject]@{
            Hwnd      = $h
            Class     = $class
            Pid       = $procId
            Visible   = $visible
            Minimized = $iconic
            Cloaked   = $cloaked
            ToolWindow = $toolWindow
            Normal    = $normal
            DesktopHost = $isDesktopHost
        })

        $h = [FcHost.Native]::GetWindow($h, $GW_HWNDNEXT)
    }
    return $list
}

function Convert-ToHwnd {
    param([string]$s)
    if ($s -match '^0x') {
        return [IntPtr]([Convert]::ToInt64($s, 16))
    }
    return [IntPtr]([Convert]::ToInt64($s, 10))
}

$explicitHwnds = @($TargetHwnd | ForEach-Object { Convert-ToHwnd $_ })

function Format-State {
    param(
        [System.Collections.Generic.List[object]]$Snapshot,
        [IntPtr[]]$ExplicitHwnds,
        [int[]]$Pids,
        [int]$BelowCount
    )

    # 目標集合：明確 HWND ∪ 各 PID 目前可見的頂層視窗（每次取樣重新算，含新開出的視窗）
    $targetHwndSet = New-Object 'System.Collections.Generic.Dictionary[Int64,bool]'
    foreach ($hw in $ExplicitHwnds) { $targetHwndSet[$hw.ToInt64()] = $true }
    if ($Pids.Count -gt 0) {
        foreach ($win in $Snapshot) {
            if ($win.Visible -and ($Pids -contains [int]$win.Pid)) {
                $targetHwndSet[$win.Hwnd.ToInt64()] = $true
            }
        }
    }

    $fgHwnd = [FcHost.Native]::GetForegroundWindow()
    $fgClass = if ($fgHwnd -ne [IntPtr]::Zero) { Get-WindowClassName $fgHwnd } else { '(none)' }
    $fgPid = 0
    if ($fgHwnd -ne [IntPtr]::Zero) { [void][FcHost.Native]::GetWindowThreadProcessId($fgHwnd, [ref]$fgPid) }

    $shellHwnd = [FcHost.Native]::GetShellWindow()
    $explorerPid = 0
    if ($shellHwnd -ne [IntPtr]::Zero) { [void][FcHost.Native]::GetWindowThreadProcessId($shellHwnd, [ref]$explorerPid) }

    $parts = New-Object System.Collections.Generic.List[string]
    $parts.Add("explorerPid=$explorerPid")
    $parts.Add("fgClass=$fgClass")
    $parts.Add("fgPid=$fgPid")

    $sortedTargets = $targetHwndSet.Keys | Sort-Object
    foreach ($hv in $sortedTargets) {
        $hex = "0x{0:X}" -f $hv
        $prefix = "win[$hex]"

        $idx = -1
        $occurrences = 0
        for ($i = 0; $i -lt $Snapshot.Count; $i++) {
            if ($Snapshot[$i].Hwnd.ToInt64() -eq $hv) {
                if ($idx -lt 0) { $idx = $i }
                $occurrences++
            }
        }

        if ($idx -lt 0) {
            $parts.Add("$prefix.status=not_found")
            continue
        }

        $target = $Snapshot[$idx]

        # 「上方可見視窗數」＝brief 原文定義：可見、非最小化、非 cloaked，不套用
        # 「一般視窗」判準（不排除 tool window／零尺寸）——可見的 tool window
        # （例如浮動面板）位於目標上方時，也要算進 above，不能被吃掉。
        $above = 0
        $desktopAbove = $false
        for ($i = 0; $i -lt $idx; $i++) {
            $w = $Snapshot[$i]
            if ($w.Visible -and (-not $w.Minimized) -and (-not $w.Cloaked)) { $above++ }
            # desktopAbove：Win+D 的核心判準——桌面視窗（Progman／有 SHELLDLL_DefView
            # 的 WorkerW）是否實際位於目標之上。Progman 本身帶 WS_EX_TOOLWINDOW，
            # 不能套用「一般視窗」判準（會被 tool window 條件排除），這裡只看
            # 可見、非 cloaked，不看是否最小化（桌面視窗不會被最小化，判準用不到）、
            # 不看 tool window／零尺寸。
            if ($w.Visible -and (-not $w.Cloaked) -and $w.DesktopHost) { $desktopAbove = $true }
        }

        # 「下方可見視窗類別」（below 文字清單）維持用「一般視窗」判準（額外排除
        # tool window／零尺寸），理由見腳本頭部 .DESCRIPTION。
        $belowAll = New-Object System.Collections.Generic.List[object]
        $belowNormal = New-Object System.Collections.Generic.List[object]
        for ($i = $idx + 1; $i -lt $Snapshot.Count; $i++) {
            $belowAll.Add($Snapshot[$i])
            if ($Snapshot[$i].Normal) { $belowNormal.Add($Snapshot[$i]) }
        }

        # hasProgman／hasWorkerW：Progman 帶 WS_EX_TOOLWINDOW，若套用「一般視窗」判準
        # （$belowNormal）會被 tool window 條件排除、永遠算不到——改從**不經一般視窗
        # 篩選**的完整下方清單（$belowAll）算，只要求可見、非 cloaked。
        $hasProgman = [bool]($belowAll | Where-Object { $_.Visible -and (-not $_.Cloaked) -and $_.Class -eq 'Progman' })
        $hasWorkerW = [bool]($belowAll | Where-Object { $_.Visible -and (-not $_.Cloaked) -and $_.Class -eq 'WorkerW' })

        $shown = $belowNormal | Select-Object -First $BelowCount
        $belowStr = if ($shown.Count -gt 0) { ($shown | ForEach-Object { $_.Class }) -join ',' } else { '(none)' }
        $extra = $belowNormal.Count - $shown.Count
        if ($extra -gt 0) { $belowStr = "$belowStr(+$extra more)" }

        $parts.Add("$prefix.class=$($target.Class)")
        $parts.Add("$prefix.visible=$([int]$target.Visible)")
        $parts.Add("$prefix.minimized=$([int]$target.Minimized)")
        $parts.Add("$prefix.cloaked=$([int]$target.Cloaked)")
        $parts.Add("$prefix.above=$above")
        $parts.Add("$prefix.desktopAbove=$([int]$desktopAbove)")
        $parts.Add("$prefix.below=$belowStr")
        $parts.Add("$prefix.hasProgman=$([int]$hasProgman)")
        $parts.Add("$prefix.hasWorkerW=$([int]$hasWorkerW)")
        # 同一 HWND 在一次列舉中出現兩次以上＝列舉途中它的 z-order 被改了（GW_HWNDNEXT 走訪
        # 不是原子快照）。此時上面各欄位取自**第一個**出現處（較上層的舊位置），不代表列舉
        # 結束時的狀態；只在發生時才輸出本欄位（task 3.1 新增）。
        if ($occurrences -gt 1) { $parts.Add("$prefix.dupInSnapshot=$occurrences") }
    }

    return ($parts -join ' ')
}

function Get-Timestamp {
    return (Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fffK')
}

$outDir = Split-Path -Parent $OutFile
if ($outDir -and -not (Test-Path $outDir)) { New-Item -ItemType Directory -Path $outDir -Force | Out-Null }

$writer = New-Object System.IO.StreamWriter($OutFile, $false)
$writer.AutoFlush = $true

try {
    $header = "# watch-zorder.ps1 session start=$(Get-Timestamp) processId=[$($ProcessId -join ',')] processName=[$($ProcessName -join ',')] targetHwnd=[$($TargetHwnd -join ',')] intervalMs=$IntervalMs durationSec=$DurationSec belowCount=$BelowCount"
    $writer.WriteLine($header)
    if (-not $Quiet) { Write-Host $header }

    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $lastState = $null
    $lineCount = 0

    while ($sw.Elapsed.TotalSeconds -lt $DurationSec) {
        $snap = Get-ZOrderSnapshot
        # -ProcessName 每次取樣重新解析（行程可能在監控開始後才啟動、或重啟換了 PID）。
        $pids = @($ProcessId)
        if ($ProcessName.Count -gt 0) {
            $pids += @(Get-Process -Name $ProcessName -ErrorAction SilentlyContinue | ForEach-Object { $_.Id })
        }
        $state = Format-State -Snapshot $snap -ExplicitHwnds $explicitHwnds -Pids $pids -BelowCount $BelowCount

        if ($state -ne $lastState) {
            $line = "$(Get-Timestamp) $state"
            $writer.WriteLine($line)
            if (-not $Quiet) { Write-Host $line }
            $lastState = $state
            $lineCount++
        }

        Start-Sleep -Milliseconds $IntervalMs
    }

    $footer = "# watch-zorder.ps1 session end=$(Get-Timestamp) stateLines=$lineCount"
    $writer.WriteLine($footer)
    if (-not $Quiet) { Write-Host $footer }
}
finally {
    $writer.Flush()
    $writer.Close()
}
