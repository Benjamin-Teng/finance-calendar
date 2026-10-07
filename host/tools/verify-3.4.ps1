<#
.SYNOPSIS
    Task 3.4 驗收驅動腳本（Win+D 顯示桌面，design.md D8 Win+D 條／探針 1.1 定案）：對正式
    宿主 fc-host 跑 Win+D 進出兩輪（另加一輪「切換到應用程式離開」）、點桌面三次，逐項判讀
    task 1.1 的四項條件。

.DESCRIPTION
    驅動方式沿用 host/tools/probe-1.1.ps1（keybd_event 送 Win+D、mouse_event 點擊、
    SetForegroundWindow＋SW_RESTORE 切換視窗），宿主環境沿用 verify-3.3.ps1（暫存
    %APPDATA%／%LOCALAPPDATA%＋最小 fixture，不動使用者真正的 settings.json 與
    D:\finance-calendar）。

    **每一次底層注入呼叫（每個按鍵按下／放開、滑鼠按下／放開、游標移動）前都檢查
    LogonUI.exe**——由 lib/SafeInput.psm1 負責（task 1.1 fix round 1）：工作階段鎖定時按鍵會
    打進登入畫面的密碼框（global-constraints 第 18 條），一旦偵測到鎖定就丟 BLOCKED、停止後續
    所有注入、清理後以結束碼 2 停止，不重試。

    步驟：
      1. 暫存目錄＋fixture → 啟動宿主 → 等小工具出現。
      2. 開自己的 WinForms 表單（lib/ScratchWindow.psm1）蓋在第一個小工具上（一般視窗），設為前景；啟動 watch-zorder
         （-ProcessName fc-host 自動涵蓋所有小工具，另加表單與殼層所有頂層
         Progman／WorkerW 為明確目標）。
      3. 第 1、2 輪：Win+D 進入 → 停 -HoldSeconds 秒 → Win+D 離開。
      4. 第 3 輪：Win+D 進入 → 停 -HoldSeconds 秒 → 切換到表單（SW_RESTORE＋前景）離開。
      5. 表單在前景時點擊桌面空白處三次（間隔 2 秒）。
      6. 結束宿主與表單，依 z-order 記錄與宿主的 gatekeeper.log 判讀：
         (1) Win+D 時小工具可見：進入後 1.5 秒內有 `SHOWDESKTOP ENTER`；之後到離開前，表單
             minimized=1，**啟動時取得的每個小工具**都在記錄中且 visible=1、minimized=0、
             cloaked=0、desktopAbove=0（沒有桌面 host 視窗在它之上）；任何一個缺席即 FAIL
             （fix round 1：小工具 HWND 列為 watch-zorder 明確目標，隱藏時仍會列出）。(3)(4)
             同樣要求每個小工具存在且可見。
         (2) 進入後 5 秒內無來回切換：ENTER＋1 秒之後到 ENTER＋5 秒（或離開，取早者）之間
             gatekeeper.log 沒有任何 SHOWDESKTOP ENTER／REASSERT／EXIT，z-order 記錄也沒有
             任何一行出現小工具 desktopAbove=1。（ENTER 後 1 秒內的 REASSERT 屬進入過程——
             1.1 實測 explorer 在 20～60 ms 內會再抬一次 Progman——只計數不判失敗。）
         (3) 離開後回到一般視窗之下：離開動作後 1.5 秒內有 `SHOWDESKTOP EXIT`；離開後
             2.5 秒的狀態中，每個小工具 below=(none)，且表單 minimized=0、above 小於
             小工具（表單在小工具之上）。
         (4) 點桌面不會浮上來：三次點擊期間到最後一次點擊後 3 秒，gatekeeper.log 沒有
             SHOWDESKTOP ENTER；z-order 記錄每一行小工具 below=(none)、表單在小工具之上。
         另檢查全程前景從未是宿主，並記錄宿主 CPU 時間（事件掛鉤若失控重算會反映在這裡）。

    不截圖。輸出（-Tag 非空時檔名加 -<Tag>）：evidence/3.4-summary.log、3.4-steps.log、
    3.4-zorder.log、3.4-gatekeeper.log。結束碼：0＝全部 PASS、1＝有 FAIL、2＝BLOCKED（鎖定）、3＝ENV-BLOCKED（合成輸入不生效，
    見 lib/SafeInput.psm1 的 Invoke-SafeInputPreflight）。

.EXAMPLE
    cd host; cargo build --release
    pwsh -File tools/verify-3.4.ps1
#>
[CmdletBinding()]
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\fc-host.exe'),
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [int]$HoldSeconds = 7,
    [string]$Tag = ''
)

$ErrorActionPreference = 'Stop'

Add-Type -Namespace V34 -Name Native -MemberDefinition @'
[DllImport("user32.dll")] public static extern System.IntPtr SetThreadDpiAwarenessContext(System.IntPtr ctx);
[DllImport("user32.dll")] public static extern System.IntPtr GetForegroundWindow();
[DllImport("user32.dll")] public static extern bool SetForegroundWindow(System.IntPtr h);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr h, out uint p);
[DllImport("user32.dll")] public static extern System.IntPtr GetShellWindow();
[DllImport("user32.dll")] public static extern System.IntPtr GetTopWindow(System.IntPtr h);
[DllImport("user32.dll")] public static extern System.IntPtr GetWindow(System.IntPtr h, uint cmd);
[DllImport("user32.dll")] public static extern bool GetWindowRect(System.IntPtr h, out RECT r);
[DllImport("user32.dll")] public static extern bool MoveWindow(System.IntPtr h, int x, int y, int w, int hgt, bool repaint);
[DllImport("user32.dll")] public static extern System.IntPtr WindowFromPoint(POINT p);
[DllImport("user32.dll")] public static extern System.IntPtr GetAncestor(System.IntPtr h, uint flags);
[DllImport("user32.dll", CharSet = System.Runtime.InteropServices.CharSet.Unicode)]
public static extern int GetClassName(System.IntPtr h, System.Text.StringBuilder b, int n);
[DllImport("user32.dll", CharSet = System.Runtime.InteropServices.CharSet.Unicode)]
public static extern int GetWindowText(System.IntPtr h, System.Text.StringBuilder b, int n);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(System.IntPtr h);
[DllImport("user32.dll")] public static extern bool IsIconic(System.IntPtr h);
[DllImport("user32.dll")] public static extern bool ShowWindow(System.IntPtr h, int cmd);
[System.Runtime.InteropServices.StructLayout(System.Runtime.InteropServices.LayoutKind.Sequential)]
public struct POINT { public int X; public int Y; }
[System.Runtime.InteropServices.StructLayout(System.Runtime.InteropServices.LayoutKind.Sequential)]
public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
'@

# fix F9：Per-Monitor-V2（-4），在任何座標取得之前。GetWindowRect、WindowFromPoint、Screen.AllScreens 的
# 工作區、表單擺位與 SafeInput 的點擊座標都依本執行緒的 DPI 感知解讀；pwsh 預設 unaware，混合 DPI 時會錯位。
[void][V34.Native]::SetThreadDpiAwarenessContext([IntPtr](-4))

function Get-Cls([IntPtr]$h) { $sb = New-Object Text.StringBuilder 256; [void][V34.Native]::GetClassName($h, $sb, 256); $sb.ToString() }
function Get-Title([IntPtr]$h) { $sb = New-Object Text.StringBuilder 256; [void][V34.Native]::GetWindowText($h, $sb, 256); $sb.ToString() }
function Get-WinPid([IntPtr]$h) { $p = 0; [void][V34.Native]::GetWindowThreadProcessId($h, [ref]$p); [int]$p }
function Hex([IntPtr]$h) { '0x{0:X}' -f $h.ToInt64() }
function Stamp { Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fffK' }

$script:Steps = New-Object System.Collections.Generic.List[string]
$script:Marks = [ordered]@{}
function Step([string]$msg) { $line = "$(Stamp) STEP $msg"; $script:Steps.Add($line); Write-Host $line }
function Mark([string]$name) { $script:Marks[$name] = Get-Date; Step "MARK $name" }

# 所有鍵盤／滑鼠注入一律經 lib/SafeInput.psm1（task 1.1 fix round 1）：每一次底層注入呼叫
# 前重新檢查 LogonUI.exe（LockApp 可能殘留，不用），鎖定就丟 BLOCKED 並停止後續所有注入。
# 本腳本不得自行宣告或呼叫注入 API。
Import-Module (Join-Path $PSScriptRoot 'lib\SafeInput.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\AutostartRegistry.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\VerifyVerdict.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\ScratchWindow.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force

function Get-TopLevel {
    $list = New-Object System.Collections.Generic.List[IntPtr]
    $h = [V34.Native]::GetTopWindow([IntPtr]::Zero)
    while ($h -ne [IntPtr]::Zero) { $list.Add($h); $h = [V34.Native]::GetWindow($h, 2) }
    return $list
}

function Get-HostWidgetWindows([int]$ProcId) {
    @(Get-TopLevel | Where-Object {
            (Get-WinPid $_) -eq $ProcId -and [V34.Native]::IsWindowVisible($_) -and (Get-Title $_) -like 'fc-host *'
        })
}

# 前景鎖繞法：先送一次 Alt；最小化的視窗先 SW_RESTORE（模擬工作列點選／Alt+Tab，
# 只 SetForegroundWindow 不會還原——1.1 run1 的教訓）。
function Set-Foreground([IntPtr]$h) {
    Send-GuardedAltTap
    if ([V34.Native]::IsIconic($h)) { [void][V34.Native]::ShowWindow($h, 9) }
    return [V34.Native]::SetForegroundWindow($h)
}

# 「最上層視窗屬於殼層 Progman／WorkerW」的點（桌面空白處；小工具與表單蓋住的點自然被
# 排除），由右下往左上掃、避開左側桌面圖示欄。**掃所有螢幕**（不只主螢幕）：解鎖後實測發現
# 主螢幕（4K 外接）可能整台被使用者自己的一般視窗（例如全螢幕/最大化的編輯器）蓋滿，這是
# 正常的使用者狀態、不是異常，找不到桌面空白處時退到次螢幕找（只要是殼層桌面即可，測的是
# 「點擊桌面不會讓小工具浮起」這個系統層級行為，不限定哪一台螢幕）。
function Find-DesktopPoint {
    Add-Type -AssemblyName System.Windows.Forms
    $shellPid = Get-WinPid ([V34.Native]::GetShellWindow())
    foreach ($scr in [System.Windows.Forms.Screen]::AllScreens) {
        $wa = $scr.WorkingArea
        for ($y = $wa.Bottom - 60; $y -gt $wa.Top + 60; $y -= 40) {
            for ($x = $wa.Right - 60; $x -gt $wa.Left + 60; $x -= 40) {
                $pt = New-Object V34.Native+POINT; $pt.X = $x; $pt.Y = $y
                $h = [V34.Native]::WindowFromPoint($pt)
                if ($h -eq [IntPtr]::Zero) { continue }
                $root = [V34.Native]::GetAncestor($h, 2)  # GA_ROOT
                $cls = Get-Cls $root
                if (($cls -eq 'Progman' -or $cls -eq 'WorkerW') -and (Get-WinPid $root) -eq $shellPid) {
                    return @{ X = $x; Y = $y; Hit = (Get-Cls $h); Root = "$cls $(Hex $root)"; Screen = $scr.DeviceName }
                }
            }
        }
    }
    return $null
}

function New-Fixture {
    $now = Get-Date -Format 'yyyy-MM-dd HH:mm'
    $obj = [ordered]@{
        updated = $now; fetched = $now; errors = @()
        macro = @(@{ title = '測試指標'; date = $now })
        events = @(); punish = @(); quotes = @(); holidays = @()
    }
    return ($obj | ConvertTo-Json -Depth 6)
}

# ---------------------------------------------------------------- 前置
# 開始前的前置探查（SafeInput fix round 3）：鎖定 → 結束碼 2（BLOCKED）；未鎖定但合成輸入沒被系統
# 計入 → 結束碼 3（ENV-BLOCKED，memory logonui-unlocked-but-synthetic-input-inert），不產生無資訊的 PASS/FAIL。
$pf = Invoke-SafeInputPreflight
if ($pf.ExitCode -ne 0) { Write-Host $pf.Message; exit $pf.ExitCode }
Write-Host $pf.Message
if (Get-Process -Name fc-host -ErrorAction SilentlyContinue) { throw '已有 fc-host 在執行，請先結束。' }
if (-not (Test-Path $Exe)) { throw "找不到 $Exe，先在 host/ 執行 cargo build --release" }
$Exe = (Resolve-Path $Exe).Path
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$sfx = if ($Tag) { "-$Tag" } else { '' }
$zlog = Join-Path $OutDir "3.4-zorder$sfx.log"
$stepsPath = Join-Path $OutDir "3.4-steps$sfx.log"
$sumPath = Join-Path $OutDir "3.4-summary$sfx.log"
$gkCopy = Join-Path $OutDir "3.4-gatekeeper$sfx.log"

$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ('fc-host-3.4-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
$tempAppData = Join-Path $tempRoot 'Roaming'
$tempLocalAppData = Join-Path $tempRoot 'Local'
$dataDir = Join-Path $tempLocalAppData 'tw.fintools.fc-host\data'
New-Item -ItemType Directory -Force -Path $tempAppData, $dataDir | Out-Null
Set-Content -Path (Join-Path $dataDir 'tw_events.json') -Value (New-Fixture) -Encoding UTF8
$gatekeeperLog = Join-Path $tempAppData 'tw.fintools.fc-host\gatekeeper.log'

$regSnap = @(Save-FcHostAutostartRegistry)
Step "開機自啟登錄快照：$(($regSnap | ForEach-Object { "$($_.Name)@$(Split-Path $_.Key -Leaf) Exists=$($_.Exists)" }) -join '; ')"
$hostProc = $null; $form = $null; $normalWin = [IntPtr]::Zero; $watcher = $null; $exitCode = 0
$cpuStart = $null; $cpuEnd = $null
try {
    # ---- 啟動宿主
    $oldA = $env:APPDATA; $oldL = $env:LOCALAPPDATA
    try {
        $env:APPDATA = $tempAppData; $env:LOCALAPPDATA = $tempLocalAppData
        $hostProc = Start-Process -FilePath $Exe -PassThru
    } finally { $env:APPDATA = $oldA; $env:LOCALAPPDATA = $oldL }
    $hostPid = $hostProc.Id
    for ($i = 0; $i -lt 80 -and (Get-HostWidgetWindows $hostPid).Count -eq 0; $i++) { Start-Sleep -Milliseconds 250 }
    Start-Sleep -Seconds 2
    $widgets = Get-HostWidgetWindows $hostPid
    if ($widgets.Count -eq 0) { throw '宿主 20 秒內沒有任何小工具出現' }
    Step "host pid=$hostPid widgets=$(($widgets | ForEach-Object { "$(Hex $_)[$(Get-Title $_)]" }) -join ',')"

    # ---- 自己的 WinForms 表單（一般視窗）蓋住第一個小工具
    # fix F7：不借用記事本（Win11 記事本是單一行程多視窗，以 PID 收尾會連使用者原本開著的記事本
    # 一起關掉；memory：win11-notepad-single-process-multi-window）。收尾只關這扇表單。
    $r = New-Object V34.Native+RECT
    [void][V34.Native]::GetWindowRect($widgets[0], [ref]$r)
    $form = Start-ScratchForm -Title ('fc-host-3.4-normal-' + [guid]::NewGuid().ToString('N').Substring(0, 6)) `
        -X ($r.Left - 200) -Y ($r.Top + 40) -Width 700 -Height ([Math]::Max(300, $r.Bottom - $r.Top))
    $normalWin = $form.Hwnd
    [void](Set-Foreground $normalWin)
    Start-Sleep -Seconds 1
    Step "form=$(Hex $normalWin) pid=$($form.Process.Id) fg=$(Get-Cls ([V34.Native]::GetForegroundWindow()))"

    # ---- 監控：小工具（-ProcessName）＋表單＋殼層 Progman／WorkerW
    $shellPid = Get-WinPid ([V34.Native]::GetShellWindow())
    $desk = @(Get-TopLevel | Where-Object { $c = Get-Cls $_; ($c -eq 'Progman' -or $c -eq 'WorkerW') -and (Get-WinPid $_) -eq $shellPid } | ForEach-Object { Hex $_ })
    Step "shellPid=$shellPid desktopWindows=$($desk -join ',')"
    # 啟動時取得的每個小工具 HWND 也列為明確目標（fix round 1，Codex task-3.4 [medium]）：
    # -ProcessName 只列「目前可見」的視窗，小工具被隱藏時會從記錄消失；明確目標則會照常列出
    # visible=0 或 status=not_found，判讀才看得到。
    $targets = @((Hex $normalWin)) + $desk + @($widgets | ForEach-Object { Hex $_ })
    $watchSec = 3 * ($HoldSeconds + 8) + 20 + 15
    $watchScript = Join-Path $PSScriptRoot 'watch-zorder.ps1'
    # -Command 而非 -File：-File 模式下 "a,b,c" 會被當成單一字串（1.1 教訓）
    $cmd = "& '$watchScript' -ProcessName fc-host -TargetHwnd $($targets -join ',') -OutFile '$zlog' -DurationSec $watchSec -IntervalMs 200 -BelowCount 8 -Quiet"
    $watcher = Start-Process pwsh -ArgumentList @('-NoProfile', '-Command', $cmd) -WindowStyle Hidden -PassThru
    for ($i = 0; $i -lt 100 -and -not ((Test-Path $zlog) -and (Get-Content $zlog -TotalCount 1)); $i++) { Start-Sleep -Milliseconds 100 }
    Start-Sleep -Seconds 2
    $cpuStart = (Get-Process -Id $hostPid).TotalProcessorTime

    # ---- 第 1、2 輪：Win+D 進入 → 停 → Win+D 離開
    foreach ($round in 1, 2) {
        [void](Set-Foreground $normalWin); Start-Sleep -Milliseconds 1500
        Assert-SessionUnlocked "第 $round 輪開始"
        Mark "in$round"; Send-GuardedWinD
        Start-Sleep -Seconds $HoldSeconds
        Mark "out$round"; Send-GuardedWinD
        Start-Sleep -Seconds 3
        Step "round $round done fg=$(Get-Cls ([V34.Native]::GetForegroundWindow())) formIconic=$([int][V34.Native]::IsIconic($normalWin))"
    }

    # ---- 第 3 輪：Win+D 進入 → 停 → 切換到表單離開
    [void](Set-Foreground $normalWin); Start-Sleep -Milliseconds 1500
    Assert-SessionUnlocked '第 3 輪開始'
    Mark 'in3'; Send-GuardedWinD
    Start-Sleep -Seconds $HoldSeconds
    Mark 'out3'; $ok = Set-Foreground $normalWin
    Start-Sleep -Seconds 3
    Step "round 3 done SetForegroundWindow=$ok fg=$(Get-Cls ([V34.Native]::GetForegroundWindow())) formIconic=$([int][V34.Native]::IsIconic($normalWin))"

    # ---- 點桌面三次（表單在前景、未最小化）
    [void](Set-Foreground $normalWin); Start-Sleep -Milliseconds 1500
    $pt = Find-DesktopPoint
    if (-not $pt) { throw '找不到可點擊的桌面空白處' }
    Step "click 前置 formIconic=$([int][V34.Native]::IsIconic($normalWin)) fg=$(Get-Cls ([V34.Native]::GetForegroundWindow())) point=($($pt.X),$($pt.Y)) hit=$($pt.Hit) root=$($pt.Root) screen=$($pt.Screen)"
    foreach ($c in 1, 2, 3) {
        Mark "click$c"; Send-GuardedClick $pt.X $pt.Y
        Start-Sleep -Milliseconds 500
        Step "click $c 後 fg=$(Get-Cls ([V34.Native]::GetForegroundWindow()))"
        Start-Sleep -Milliseconds 1500
    }
    Start-Sleep -Seconds 3
    Mark 'end'
    $cpuEnd = (Get-Process -Id $hostPid).TotalProcessorTime
}
catch {
    if ("$_" -like 'BLOCKED*') { Step "$_"; $exitCode = 2 } else { Step "ERROR $_"; throw }
}
finally {
    try { Stop-ScratchForm $form } catch { Step "關閉表單失敗：$_" }
    if ($hostProc -and -not $hostProc.HasExited) { Stop-Process -Id $hostProc.Id -Force -ErrorAction SilentlyContinue }
    if ($watcher -and -not $watcher.HasExited) { Stop-Process -Id $watcher.Id -Force -ErrorAction SilentlyContinue }
    $regLeft = @(Restore-FcHostAutostartRegistry $regSnap)
    if ($regLeft.Count -gt 0) { Write-Warning "開機自啟登錄未還原：$($regLeft -join '; ')" }
    Step "開機自啟登錄還原：未還原 $($regLeft.Count) 項$(if ($regLeft.Count) { '：' + ($regLeft -join '; ') })"
    $script:Steps | ConvertTo-EvidenceText | Set-Content -Path $stepsPath -Encoding utf8
    if (Test-Path $gatekeeperLog) { Copy-Item $gatekeeperLog $gkCopy -Force }
}
if ($exitCode -eq 2) {
    Remove-Item -Recurse -Force $tempRoot -ErrorAction SilentlyContinue
    Write-Host 'BLOCKED：工作階段鎖定，已停止（未重試）。'
    exit 2
}

# ---------------------------------------------------------------- 判讀
$inv = [Globalization.CultureInfo]::InvariantCulture
# z-order 記錄：每行 → @{ Ts; Fg; FgPid; Win = @{ hwnd = @{ field = value } } }
$zlines = foreach ($ln in (Get-Content $zlog | Where-Object { $_ -notmatch '^#' -and $_.Trim() })) {
    $ts = [DateTimeOffset]::Parse($ln.Substring(0, $ln.IndexOf(' ')), $inv).LocalDateTime
    $win = @{}
    foreach ($m in [regex]::Matches($ln, 'win\[(0x[0-9A-F]+)\]\.(\w+)=(\S+)')) {
        $h = $m.Groups[1].Value
        if (-not $win.ContainsKey($h)) { $win[$h] = @{} }
        $win[$h][$m.Groups[2].Value] = $m.Groups[3].Value
    }
    [PSCustomObject]@{
        Ts = $ts; Win = $win
        FgPid = [regex]::Match($ln, 'fgPid=(\d+)').Groups[1].Value
    }
}
$npHex = Hex $normalWin
# 判讀以「啟動時取得的小工具」為準逐一核對（Get-ExpectedWidgetProblems）：缺席、隱藏、最小化、
# cloaked 都算問題，不能因為記錄裡列不到就當成通過（Codex task-3.4 [medium]）。
$widgetHexes = @($widgets | ForEach-Object { Hex $_ })
function Get-StateAt([datetime]$t) { $zlines | Where-Object { $_.Ts -le $t } | Select-Object -Last 1 }
function Get-LinesIn([datetime]$a, [datetime]$b) { @($zlines | Where-Object { $_.Ts -ge $a -and $_.Ts -lt $b }) }
# 小工具＝class=Tauri Window 的目標；visible=0 且 dupInSnapshot 的項目是列舉途中剛好重排的
# 重複，略過（同 verify-3.2／3.3）。
function Get-WidgetEntries($line) {
    foreach ($h in $line.Win.Keys) {
        $w = $line.Win[$h]
        if ($w['class'] -ne 'Tauri') { continue }  # 值以空白切開，"Tauri Window" 只留 "Tauri"
        if ($w['visible'] -ne '1' -and $w['dupInSnapshot']) { continue }
        [PSCustomObject]@{ Hwnd = $h; F = $w }
    }
}

# gatekeeper.log：SHOWDESKTOP 行 → @{ Ts; Kind; Text }
$gk = @()
if (Test-Path $gkCopy) {
    $gk = @(Get-Content $gkCopy | ForEach-Object {
            if ($_ -match '^(\S+) SHOWDESKTOP (ENTER|REASSERT|EXIT)\b') {
                [PSCustomObject]@{ Ts = [datetime]::ParseExact($Matches[1], 'yyyy-MM-ddTHH:mm:ss.fff', $inv); Kind = $Matches[2]; Text = $_ }
            }
        })
}
function Get-GkIn([datetime]$a, [datetime]$b, [string[]]$kinds) { @($gk | Where-Object { $_.Ts -ge $a -and $_.Ts -le $b -and $kinds -contains $_.Kind }) }

$results = [ordered]@{}
$results['開機自啟登錄已還原（Run／StartupApproved 的 fc-host）'] = ($regLeft.Count -eq 0)
$notes = New-Object System.Collections.Generic.List[string]
$sec = { param($s) [TimeSpan]::FromSeconds($s) }

foreach ($round in 1, 2, 3) {
    $tIn = $script:Marks["in$round"]; $tOut = $script:Marks["out$round"]
    if (-not $tIn -or -not $tOut) { $results["第 $round 輪：未執行"] = $false; continue }
    $how = if ($round -eq 3) { '切換到表單' } else { 'Win+D' }

    # (1) 可見
    $enter = Get-GkIn $tIn ($tIn + (& $sec 1.5)) @('ENTER') | Select-Object -First 1
    $settle = $tIn + (& $sec 1.5)
    $lines1 = @(Get-StateAt $settle) + (Get-LinesIn $settle $tOut) | Where-Object { $_ }
    $bad1 = @(); $npMinimized = $true
    foreach ($ln in $lines1) {
        if ($ln.Win[$npHex] -and $ln.Win[$npHex]['minimized'] -ne '1') { $npMinimized = $false }
        foreach ($x in @(Get-ExpectedWidgetProblems -Win $ln.Win -Expected $widgetHexes -RequireDesktopNotAbove)) {
            $bad1 += "$($ln.Ts.ToString('HH:mm:ss.fff')) $x"
        }
    }
    $results["第 $round 輪 (1) Win+D 時小工具可見且位於桌面之上"] = [bool]$enter -and $npMinimized -and $lines1.Count -gt 0 -and $bad1.Count -eq 0
    $notes.Add("第 $round 輪 ENTER：$(if ($enter) { $enter.Text } else { '(1.5 秒內無)' })；表單全程 minimized=1：$npMinimized")
    $bad1 | ForEach-Object { $notes.Add("  (1) violation: $_") }

    # (2) 5 秒內無來回切換
    if ($enter) {
        $early = Get-GkIn $enter.Ts ($enter.Ts + (& $sec 1)) @('REASSERT')
        $stableTo = if ($tOut -lt ($enter.Ts + (& $sec 5))) { $tOut } else { $enter.Ts + (& $sec 5) }
        $flips = Get-GkIn ($enter.Ts + (& $sec 1)) $stableTo @('ENTER', 'REASSERT', 'EXIT')
        $zflips = @(Get-LinesIn ($enter.Ts + (& $sec 1)) $tOut | Where-Object { (Get-WidgetEntries $_ | Where-Object { $_.F['desktopAbove'] -eq '1' }) })
        $results["第 $round 輪 (2) 進入後 5 秒內無來回切換"] = ($flips.Count -eq 0) -and ($zflips.Count -eq 0) -and (($stableTo - $enter.Ts).TotalSeconds -ge 4.9)
        $notes.Add("第 $round 輪 ENTER 後 1 秒內 REASSERT $($early.Count) 次（進入過程）；1～5 秒 SHOWDESKTOP 事件 $($flips.Count) 筆、z-order desktopAbove=1 $($zflips.Count) 行；穩定觀察 $([Math]::Round(($stableTo - $enter.Ts).TotalSeconds, 1)) 秒")
        $flips | ForEach-Object { $notes.Add("  (2) flip: $($_.Text)") }
    } else {
        $results["第 $round 輪 (2) 進入後 5 秒內無來回切換"] = $false
    }

    # (3) 離開
    $exit = Get-GkIn $tOut ($tOut + (& $sec 1.5)) @('EXIT') | Select-Object -First 1
    $after = Get-StateAt ($tOut + (& $sec 2.5))
    $bad3 = @()
    if (-not $after) { $bad3 += '離開後沒有 z-order 狀態' } else {
        $np = $after.Win[$npHex]
        if (-not $np -or $np['minimized'] -ne '0') { $bad3 += "表單 minimized=$($np['minimized'])" }
        foreach ($x in @(Get-ExpectedWidgetProblems -Win $after.Win -Expected $widgetHexes)) { $bad3 += $x }
        foreach ($w in (Get-WidgetEntries $after)) {
            if ($w.F['below'] -ne '(none)') { $bad3 += "$($w.Hwnd) below=$($w.F['below'])" }
            if ($np -and [int]$np['above'] -ge [int]$w.F['above']) { $bad3 += "$($w.Hwnd) above=$($w.F['above']) 不在表單(above=$($np['above']))之下" }
        }
    }
    $results["第 $round 輪 (3) 離開（$how）後回到一般視窗之下"] = [bool]$exit -and $bad3.Count -eq 0
    $notes.Add("第 $round 輪 EXIT：$(if ($exit) { $exit.Text } else { '(1.5 秒內無)' })")
    $bad3 | ForEach-Object { $notes.Add("  (3) violation: $_") }
}

# (4) 點桌面三次
$tc1 = $script:Marks['click1']; $tc3 = $script:Marks['click3']
if ($tc1 -and $tc3) {
    $winEnd = $tc3 + (& $sec 3)
    $enters = Get-GkIn $tc1 $winEnd @('ENTER')
    $lines4 = @(Get-StateAt $tc1) + (Get-LinesIn $tc1 $winEnd) | Where-Object { $_ }
    $bad4 = @()
    foreach ($ln in $lines4) {
        $np = $ln.Win[$npHex]
        if (-not $np -or $np['minimized'] -ne '0') { $bad4 += "$($ln.Ts.ToString('HH:mm:ss.fff')) 表單 minimized=$($np['minimized'])" }
        foreach ($x in @(Get-ExpectedWidgetProblems -Win $ln.Win -Expected $widgetHexes)) { $bad4 += "$($ln.Ts.ToString('HH:mm:ss.fff')) $x" }
        foreach ($w in (Get-WidgetEntries $ln)) {
            if ($w.F['below'] -ne '(none)') { $bad4 += "$($ln.Ts.ToString('HH:mm:ss.fff')) $($w.Hwnd) below=$($w.F['below'])" }
            if ($np -and [int]$np['above'] -ge [int]$w.F['above']) { $bad4 += "$($ln.Ts.ToString('HH:mm:ss.fff')) $($w.Hwnd) 浮到表單之上" }
        }
    }
    $results['(4) 點桌面三次，小工具不會浮上來'] = ($enters.Count -eq 0) -and $lines4.Count -gt 0 -and ($bad4.Count -eq 0)
    $notes.Add("點擊期間 ENTER $($enters.Count) 筆、z-order 狀態 $($lines4.Count) 行")
    $bad4 | ForEach-Object { $notes.Add("  (4) violation: $_") }
} else {
    $results['(4) 點桌面三次，小工具不會浮上來'] = $false
}

$fgHost = @($zlines | Where-Object { $_.FgPid -eq "$hostPid" })
$results['全程前景從未是宿主'] = ($fgHost.Count -eq 0)

$sum = New-Object System.Collections.Generic.List[string]
$sum.Add("# verify-3.4.ps1 summary $(Stamp) exe=$Exe holdSeconds=$HoldSeconds")
if ($cpuStart -and $cpuEnd) { $sum.Add("# 宿主 CPU 時間（驅動期間）：$([Math]::Round(($cpuEnd - $cpuStart).TotalSeconds, 2)) 秒") }
foreach ($k in $results.Keys) { $sum.Add("$(if ($results[$k]) { 'PASS' } else { 'FAIL' })  $k") }
$notes | ForEach-Object { $sum.Add($_) }
$sum.Add('# gatekeeper.log（SHOWDESKTOP／REBOTTOM）：')
if (Test-Path $gkCopy) { Get-Content $gkCopy | ForEach-Object { $sum.Add("  $_") } }
$sum | ConvertTo-EvidenceText | Set-Content -Path $sumPath -Encoding utf8
$sum | ForEach-Object { Write-Host $_ }
Remove-Item -Recurse -Force $tempRoot -ErrorAction SilentlyContinue
if (@($results.Values | Where-Object { -not $_ }).Count -gt 0) { exit 1 }
exit 0
