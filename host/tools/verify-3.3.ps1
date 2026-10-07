<#
.SYNOPSIS
    Task 3.3 驗收驅動腳本（置底守門：explorer 重啟＋30 秒保險檢查，design.md D8）。

.DESCRIPTION
    步驟：
      1. 確認沒有 fc-host 在跑、確認工作階段未鎖定。
      2. 暫存目錄當 %APPDATA%（首次啟動、無 settings.json；也是
         `settings::default_gatekeeper_log_path()` 的落點，本腳本直接讀這個記錄檔當證據）
         與 %LOCALAPPDATA%（資料目錄，寫入最小 fixture 讓 clock／macro／fixed 三個小工具
         出現）。
      3. 開一個自己的 WinForms 表單（lib/ScratchWindow.psm1）當「一般應用程式視窗」；先啟動 watch-zorder（-ProcessName fc-host）
         再啟動宿主。
      4. 基準線：確認 gatekeeper.log 尚無任何 REBOTTOM 記錄。
      5. **explorer 重啟（全程只做一次）**：記錄時間 → `Stop-Process -Name explorer` →
         等待自動重啟（10 秒內未回來就 `Start-Process explorer.exe`）→ 確認 explorer／工作列
         回來 → 30 秒內 gatekeeper.log 出現 `REBOTTOM reason=explorer-restart`、z-order
         記錄顯示小工具恢復到一般視窗之下。
      6. **故障注入：把一般視窗塞到小工具之下**（fix round 1；不是輸入注入）。修正前是
         `SetWindowPos(小工具, HWND_TOP)` 把小工具抬上去——但小工具帶 always_on_bottom，
         tao 0.37.1 的 `WM_WINDOWPOSCHANGING`（`platform_impl/windows/event_loop.rs`）會把
         `hwndInsertAfter` 無條件改寫成 `HWND_BOTTOM`，呼叫「成功」卻不會真的抬高，保險檢查
         根本沒有異常可修（Codex task-3.3 [medium]）。改為另開一個自己的一般視窗（WinForms
         表單，獨立 pwsh 行程），以 `SetWindowPos(該視窗, hwndInsertAfter=最上層小工具)` 把它
         插到小工具正下方：動的是**不受 tao 攔截的那一方**，結果同樣符合保險檢查判準
         （`needs_rebottom`：最上層小工具之後有一般視窗）。先用 z-order 快照確認故障確實成立，
         才從確認時刻起算 30 秒（＋2 秒容差）：期限內 gatekeeper.log 須出現
         `REBOTTOM reason=safety-check`。fix F3：看到 REBOTTOM 後不立刻關故障視窗，而是在它仍
         存活時輪詢 z-order 快照到故障解除、再多等 600 ms（超過 watch-zorder 取樣間隔）才關閉；
         故障窗口 End 截在關閉那一刻，關閉後自然變乾淨的記錄不算恢復。
      7. **10 分鐘平靜期**：不做任何外部介入，等待 600 秒，確認 gatekeeper.log 的 REBOTTOM
         行數在這段期間完全不變（驗收要求「平常 10 分鐘內無任何重排」）。
      8. 不截圖；結束宿主、關閉自己的表單，**分階段**分析 z-order 記錄（lib/VerifyVerdict.psm1 的
         `Get-SafetyRecoveryVerdict`）：explorer 重啟與故障注入兩個預期窗口內的短暫異常不算
         違規，但窗口結束時必須已恢復；故障窗口內必須確實觀察到異常；窗口以外（基準期、
         平靜期）任何異常都算違規。
      結束碼：0＝全部 PASS（SKIP 不計）、1＝有 FAIL。

.PARAMETER Exe
    fc-host.exe 路徑，預設 host/target/release/fc-host.exe。

.PARAMETER OutDir
    證據輸出目錄，預設 host/tools/evidence。

.PARAMETER QuietSeconds
    10 分鐘平靜期的秒數，預設 600；可在開發迭代時調小，正式證據需維持 600。

.PARAMETER SkipExplorerRestart
    開發迭代用：跳過「Stop-Process -Name explorer」這一步（該動作全程只能做一次，見
    task-3.3-brief／global-constraints；先用這個旗標把腳本其餘部分跑過一輪確認沒有 bug，
    再不帶這個旗標正式跑一次才是真正的驗收證據）。跳過時該項結果標為 SKIPPED，不計入
    PASS/FAIL。
#>
[CmdletBinding()]
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\fc-host.exe'),
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [int]$QuietSeconds = 600,
    [switch]$SkipExplorerRestart
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\AutostartRegistry.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\ScratchWindow.psm1') -Force

Add-Type -Namespace V33 -Name Native -MemberDefinition @'
[DllImport("user32.dll")] public static extern System.IntPtr GetTopWindow(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern System.IntPtr GetWindow(System.IntPtr hWnd, uint cmd);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr hWnd, out uint pid);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(System.IntPtr hWnd, System.Text.StringBuilder s, int n);
[DllImport("user32.dll")] public static extern bool SetWindowPos(System.IntPtr hWnd, System.IntPtr hWndInsertAfter, int x, int y, int cx, int cy, uint flags);
[DllImport("user32.dll")] public static extern bool ShowWindow(System.IntPtr hWnd, int cmd);
[DllImport("user32.dll")] public static extern bool IsIconic(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern bool GetWindowRect(System.IntPtr hWnd, out RECT r);
[StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
'@

$SWP_NOSIZE = 0x0001
$SWP_NOMOVE = 0x0002
$SWP_NOACTIVATE = 0x0010
$SW_SHOWNOACTIVATE = 4
# 保險檢查週期 30 秒（design.md D8）；容差 2 秒涵蓋 watch-zorder 取樣間隔（200 ms）與 WM_TIMER 延遲。
$RecoverSec = 30
$TolSec = 2

Import-Module (Join-Path $PSScriptRoot 'lib\VerifyVerdict.psm1') -Force

function Get-Ts { Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fffK' }
# 鎖定判準只看 LogonUI.exe（LockApp 解鎖後可能殘留，會誤記 locked=1）。
function Test-Locked { [bool](Get-Process -Name LogonUI -ErrorAction SilentlyContinue) }
# 注意：Stop-Process -Name explorer／Start-Process explorer.exe／SetWindowPos 都不是輸入注入
# （global-constraints 第 18 條禁止的是 SendInput／keybd_event／mouse_event／Win+D／自動點擊
# 拖曳等），鎖定時也可以做——controller 已明確裁決（見 task-3.3 交辦紀錄）。本函式只負責把
# 鎖定狀態寫進記錄檔，不中止腳本；若鎖定期間的證據被判定不具代表性，由讀報告的人另外決定要不
# 要在解鎖後補跑。
function Log-LockStatus([string]$context, [System.IO.StreamWriter]$w) {
    $locked = Test-Locked
    $w.WriteLine("## $(Get-Ts) 鎖定狀態檢查（$context）：locked=$([int]$locked)")
    return $locked
}

function Get-HostWidgetWindows([int]$ProcId) {
    $list = @()
    $h = [V33.Native]::GetTopWindow([IntPtr]::Zero)
    while ($h -ne [IntPtr]::Zero) {
        $p = 0
        [void][V33.Native]::GetWindowThreadProcessId($h, [ref]$p)
        if ($p -eq $ProcId -and [V33.Native]::IsWindowVisible($h)) {
            $sb = New-Object System.Text.StringBuilder 256
            [void][V33.Native]::GetWindowText($h, $sb, 256)
            $title = $sb.ToString()
            if ($title -like 'fc-host *') {
                $list += [PSCustomObject]@{ Hwnd = $h; Title = $title }
            }
        }
        $h = [V33.Native]::GetWindow($h, 2)
    }
    return $list
}

# 由上到下列舉所有頂層視窗（含不可見），回傳 IntPtr 清單。
function Get-AllTopWindows {
    $list = New-Object System.Collections.Generic.List[IntPtr]
    $h = [V33.Native]::GetTopWindow([IntPtr]::Zero)
    while ($h -ne [IntPtr]::Zero) { $list.Add($h); $h = [V33.Native]::GetWindow($h, 2) }
    return $list
}

function Get-WindowTitle([IntPtr]$h) {
    $sb = New-Object System.Text.StringBuilder 256
    [void][V33.Native]::GetWindowText($h, $sb, 256)
    return $sb.ToString()
}

# 故障窗口用的「一般視窗」：獨立 pwsh 行程開一個 WinForms 表單（非 tool window、有尺寸）。
# 以 -EncodedCommand 傳碼避免引號問題；-WindowStyle Hidden 只為藏主控台，但它會讓子行程
# 「第一次 ShowWindow」也變成隱藏（STARTUPINFO 規則），所以找到視窗後再以 SW_SHOWNOACTIVATE
# 顯示一次。回傳 @{ Proc; Hwnd }，Hwnd 為 Zero 表示 15 秒內沒找到。
function New-FaultWindow([string]$Title, [int]$X, [int]$Y) {
    $code = @"
Add-Type -AssemblyName System.Windows.Forms
`$f = New-Object System.Windows.Forms.Form
`$f.Text = '$Title'
`$f.StartPosition = 'Manual'
`$f.Location = New-Object System.Drawing.Point($X, $Y)
`$f.Size = New-Object System.Drawing.Size(420, 300)
[System.Windows.Forms.Application]::Run(`$f)
"@
    $enc = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($code))
    $proc = Start-Process pwsh -PassThru -WindowStyle Hidden -ArgumentList @('-NoProfile', '-EncodedCommand', $enc)
    $hwnd = [IntPtr]::Zero
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($hwnd -eq [IntPtr]::Zero -and $sw.Elapsed.TotalSeconds -lt 15) {
        foreach ($h in (Get-AllTopWindows)) { if ((Get-WindowTitle $h) -eq $Title) { $hwnd = $h; break } }
        if ($hwnd -eq [IntPtr]::Zero) { Start-Sleep -Milliseconds 200 }
    }
    if ($hwnd -ne [IntPtr]::Zero) { [void][V33.Native]::ShowWindow($hwnd, $SW_SHOWNOACTIVATE); Start-Sleep -Milliseconds 300 }
    return [PSCustomObject]@{ Proc = $proc; Hwnd = $hwnd }
}

# 故障是否成立：故障視窗可見、未最小化，且在 z-order 中排在「最上層可見小工具」之後。
function Test-FaultEstablished([int]$ProcId, [IntPtr]$FaultHwnd) {
    if (-not [V33.Native]::IsWindowVisible($FaultHwnd) -or [V33.Native]::IsIconic($FaultHwnd)) { return $false }
    $all = Get-AllTopWindows
    $widgetSet = @((Get-HostWidgetWindows $ProcId) | ForEach-Object { $_.Hwnd.ToInt64() })
    $topWidget = -1; $fault = -1
    for ($i = 0; $i -lt $all.Count; $i++) {
        $v = $all[$i].ToInt64()
        if ($topWidget -lt 0 -and $widgetSet -contains $v) { $topWidget = $i }
        if ($fault -lt 0 -and $v -eq $FaultHwnd.ToInt64()) { $fault = $i }
    }
    return ($topWidget -ge 0 -and $fault -gt $topWidget)
}

function Wait-Cond([scriptblock]$Cond, [int]$TimeoutSec, [int]$PollMs = 300) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt $TimeoutSec) {
        if (& $Cond) { return $true }
        Start-Sleep -Milliseconds $PollMs
    }
    return $false
}

function Get-RebottomLines([string]$Path) {
    if (-not (Test-Path $Path)) { return @() }
    return @(Get-Content $Path | Where-Object { $_ -match 'REBOTTOM' })
}

# fixture：clock 不靠資料（系統時間）一定會出現；macro 一筆、events 空、quotes 空
# （quotes 因此隱藏，dynamic 因無 events／punish 也可能隱藏——不影響本次驗收，只需要
# 「至少有小工具可見」）。
function New-Fixture {
    $now = Get-Date -Format 'yyyy-MM-dd HH:mm'
    $obj = [ordered]@{
        updated  = $now
        fetched  = $now
        errors   = @()
        macro    = @(@{ title = '測試指標'; date = $now })
        events   = @()
        punish   = @()
        quotes   = @()
        holidays = @()
    }
    return ($obj | ConvertTo-Json -Depth 6)
}

# ── 1. 前置檢查（鎖定與否都可繼續，見 Log-LockStatus 說明；狀態記錄在 header 的 locked=）───
if (Get-Process -Name fc-host -ErrorAction SilentlyContinue) {
    throw '已有 fc-host 在執行，請先結束。'
}
$Exe = (Resolve-Path $Exe).Path
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$zlog = Join-Path $OutDir '3.3-zorder.log'
$wlogPath = Join-Path $OutDir '3.3-windows.log'
$slogPath = Join-Path $OutDir '3.3-summary.log'
$wlog = New-EvidenceWriter $wlogPath
$wlog.AutoFlush = $true
$wlog.WriteLine("# verify-3.3.ps1 start=$(Get-Ts) exe=$Exe locked=$([int](Test-Locked)) quietSeconds=$QuietSeconds")

# ── 2. 暫存 %APPDATA%／%LOCALAPPDATA%＋資料目錄 fixture ───────────────────────────────
$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ("fc-host-3.3-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
$tempAppData = Join-Path $tempRoot 'Roaming'
$tempLocalAppData = Join-Path $tempRoot 'Local'
New-Item -ItemType Directory -Force -Path $tempAppData, $tempLocalAppData | Out-Null
$dataDir = Join-Path $tempLocalAppData 'tw.fintools.fc-host\data'
New-Item -ItemType Directory -Force -Path $dataDir | Out-Null
Set-Content -Path (Join-Path $dataDir 'tw_events.json') -Value (New-Fixture) -Encoding UTF8
$gatekeeperLog = Join-Path $tempAppData 'tw.fintools.fc-host\gatekeeper.log'
$wlog.WriteLine("# APPDATA=$tempAppData（首次啟動；gatekeeper.log=$gatekeeperLog）LOCALAPPDATA=$tempLocalAppData")

# ── 3. 開一個自己的 WinForms 表單當「一般應用程式視窗」；先開監控、等 header ──────────────
# fix F7：不借用記事本（Win11 記事本是單一行程多視窗，啟動殼 PID ≠ 視窗所屬 PID，收尾會漏關或誤關
# 使用者的記事本；memory：win11-notepad-single-process-multi-window）。收尾只關這扇表單。
$normalForm = Start-ScratchForm -Title ('fc-host-3.3-normal-' + [guid]::NewGuid().ToString('N').Substring(0, 6))
$wlog.WriteLine("# $(Get-Ts) 一般視窗（自己的表單）hwnd=0x$('{0:X}' -f $normalForm.Hwnd.ToInt64()) pid=$($normalForm.Process.Id)")

$watchDuration = 120 + 40 + $QuietSeconds + 60
$regSnap = @(Save-FcHostAutostartRegistry)
$wlog.WriteLine("# $(Get-Ts) 開機自啟登錄快照：$(($regSnap | ForEach-Object { "$($_.Name)@$(Split-Path $_.Key -Leaf) Exists=$($_.Exists)" }) -join '; ')")
$watcher = Start-Process pwsh -PassThru -WindowStyle Hidden -ArgumentList @(
    '-NoProfile', '-File', (Join-Path $PSScriptRoot 'watch-zorder.ps1'),
    '-ProcessName', 'fc-host', '-OutFile', $zlog, '-DurationSec', $watchDuration, '-IntervalMs', '200', '-Quiet')
$sw = [Diagnostics.Stopwatch]::StartNew()
while (-not ((Test-Path $zlog) -and (Get-Content $zlog -TotalCount 1))) {
    if ($sw.Elapsed.TotalSeconds -gt 30) { throw 'watch-zorder 30 秒內沒有寫出 header' }
    Start-Sleep -Milliseconds 100
}
$wlog.WriteLine("# $(Get-Ts) 監控已就緒（header 已寫出，預計監控 $watchDuration 秒），啟動宿主")

# ── 4. 啟動宿主（首次啟動） ───────────────────────────────────────────────────────
$oldAppData = $env:APPDATA
$oldLocalAppData = $env:LOCALAPPDATA
$hostProc = $null
try {
    $env:APPDATA = $tempAppData
    $env:LOCALAPPDATA = $tempLocalAppData
    $hostProc = Start-Process -FilePath $Exe -PassThru
} finally {
    $env:APPDATA = $oldAppData
    $env:LOCALAPPDATA = $oldLocalAppData
}
$hostPid = $hostProc.Id
$wlog.WriteLine("# $(Get-Ts) 宿主 pid=$hostPid")

$results = [ordered]@{}
$fault = $null
try {
    # ── 5. 至少一個小工具出現（clock 不依賴 fixture） ─────────────────────────────────
    $ok = Wait-Cond { (Get-HostWidgetWindows $hostPid).Count -gt 0 } 20
    Start-Sleep -Seconds 2
    $wins = Get-HostWidgetWindows $hostPid
    $wlog.WriteLine("## $(Get-Ts) 首次啟動：可見小工具 $($wins.Count) 個：$(($wins | ForEach-Object { $_.Title }) -join ', ')")
    $results['至少一個小工具出現'] = $ok -and ($wins.Count -gt 0)
    if (-not $ok -or $wins.Count -eq 0) { throw '沒有任何小工具視窗出現，後續步驟無法進行' }

    # ── 6. 基準線：gatekeeper.log 尚無 REBOTTOM 記錄 ─────────────────────────────────
    Start-Sleep -Seconds 3
    $baseline = Get-RebottomLines $gatekeeperLog
    $wlog.WriteLine("## $(Get-Ts) 基準線：gatekeeper.log REBOTTOM 行數=$($baseline.Count)（期望 0）")
    $results['基準線：啟動後平常狀態沒有任何重排記錄'] = ($baseline.Count -eq 0)

    # ── 7. explorer 重啟（全程只做一次） ───────────────────────────────────────────
    $explorerRestartAt = '(SkipExplorerRestart)'
    $explorerStartDto = $null; $explorerBackDto = $null
    if ($SkipExplorerRestart) {
        $wlog.WriteLine("## $(Get-Ts) SkipExplorerRestart：略過真正的 explorer 重啟（開發迭代模式）")
        $results['explorer 重啟後 explorer 行程回來'] = 'SKIPPED'
        $results['explorer 重啟 30 秒內 gatekeeper.log 出現 REBOTTOM reason=explorer-restart'] = 'SKIPPED'
        $results['宿主行程在 explorer 重啟後持續存活'] = 'SKIPPED'
        $results['小工具視窗在 explorer 重啟後保持存在且可見'] = 'SKIPPED'
    } else {
        Log-LockStatus '重啟 explorer 之前' $wlog | Out-Null
        $explorerRestartAt = Get-Ts
        $explorerStartDto = [DateTimeOffset]::Now
        $wlog.WriteLine("## $explorerRestartAt STEP：Stop-Process -Name explorer（只做這一次）")
        Stop-Process -Name explorer -Force -ErrorAction SilentlyContinue
        $backByItself = Wait-Cond { [bool](Get-Process -Name explorer -ErrorAction SilentlyContinue) } 10
        if (-not $backByItself) {
            $wlog.WriteLine("## $(Get-Ts) explorer 10 秒內未自動重啟，手動 Start-Process explorer.exe")
            Start-Process explorer.exe
        }
        $explorerBack = Wait-Cond { [bool](Get-Process -Name explorer -ErrorAction SilentlyContinue) } 20
        $explorerBackDto = [DateTimeOffset]::Now
        $wlog.WriteLine("## $(Get-Ts) explorer 行程回來=$explorerBack（自行重啟=$backByItself）")
        $results['explorer 重啟後 explorer 行程回來'] = $explorerBack

        $restartRebottom = Wait-Cond { (Get-RebottomLines $gatekeeperLog).Count -gt $baseline.Count } 30
        $afterRestartLines = Get-RebottomLines $gatekeeperLog
        $wlog.WriteLine("## $(Get-Ts) 30 秒內出現 REBOTTOM（explorer 重啟）=$restartRebottom；目前總行數=$($afterRestartLines.Count)")
        $newRestartLines = $afterRestartLines | Select-Object -Skip $baseline.Count
        $results['explorer 重啟 30 秒內 gatekeeper.log 出現 REBOTTOM reason=explorer-restart'] =
        $restartRebottom -and ($newRestartLines | Where-Object { $_ -match 'reason=explorer-restart' })

        $hostAliveAfterRestart = -not (Get-Process -Id $hostPid -ErrorAction SilentlyContinue).HasExited
        $results['宿主行程在 explorer 重啟後持續存活'] = $hostAliveAfterRestart
        $winsAfterRestart = Get-HostWidgetWindows $hostPid
        $results['小工具視窗在 explorer 重啟後保持存在且可見'] = ($winsAfterRestart.Count -eq $wins.Count)
    }

    # ── 8. 故障注入：把自己開的一般視窗塞到最上層小工具正下方（不動小工具，避開 tao 攔截） ──
    Log-LockStatus '故障注入之前' $wlog | Out-Null
    $target = (Get-HostWidgetWindows $hostPid) | Select-Object -First 1   # 列舉由上到下＝最上層小工具
    $tr = New-Object V33.Native+RECT
    [void][V33.Native]::GetWindowRect($target.Hwnd, [ref]$tr)
    $faultTitle = 'verify-3.3 fault window ' + [guid]::NewGuid().ToString('N').Substring(0, 8)
    $fault = New-FaultWindow $faultTitle ($tr.Left + 20) ($tr.Top + 20)
    $wlog.WriteLine("## $(Get-Ts) 故障視窗 hwnd=0x$('{0:X}' -f $fault.Hwnd.ToInt64()) title=$faultTitle pid=$($fault.Proc.Id)")
    $results['故障視窗建立（自己開的一般視窗）'] = ($fault.Hwnd -ne [IntPtr]::Zero)
    $faultInjectedDto = $null; $faultConfirmedDto = $null; $faultClosedDto = $null
    if ($fault.Hwnd -ne [IntPtr]::Zero) {
        $baselineForFault = Get-RebottomLines $gatekeeperLog
        $faultInjectedDto = [DateTimeOffset]::Now
        $wlog.WriteLine("## $(Get-Ts) STEP：SetWindowPos(故障視窗, hwndInsertAfter=$($target.Title))（人為介入，非輸入注入）")
        $posOk = [V33.Native]::SetWindowPos($fault.Hwnd, $target.Hwnd, 0, 0, 0, 0, ($SWP_NOMOVE -bor $SWP_NOSIZE -bor $SWP_NOACTIVATE))
        $established = Wait-Cond { Test-FaultEstablished $hostPid $fault.Hwnd } 3 100
        if ($established) { $faultConfirmedDto = [DateTimeOffset]::Now }
        $wlog.WriteLine("## $(Get-Ts) SetWindowPos=$posOk；z-order 快照確認故障成立（一般視窗在最上層小工具之下）=$established")
        $results['故障注入成立：z-order 快照確認一般視窗位於小工具之下'] = $posOk -and $established

        if ($established) {
            # 期限從「確認故障成立」起算（Codex task-3.3 [medium] 建議）。
            $safetyRebottom = Wait-Cond { (Get-RebottomLines $gatekeeperLog).Count -gt $baselineForFault.Count } ($RecoverSec + $TolSec)
            $afterFaultLines = Get-RebottomLines $gatekeeperLog
            $newFaultLines = $afterFaultLines | Select-Object -Skip $baselineForFault.Count
            $wlog.WriteLine("## $(Get-Ts) 確認後 $($RecoverSec + $TolSec) 秒內出現 REBOTTOM（保險檢查）=$safetyRebottom；目前總行數=$($afterFaultLines.Count)")
            $results["故障成立後 $RecoverSec 秒（+$TolSec 秒容差）內 gatekeeper.log 出現 REBOTTOM reason=safety-check"] =
            $safetyRebottom -and [bool]($newFaultLines | Where-Object { $_ -match 'reason=safety-check' })

            # fix F3（review task-3.3-fixB-opus.md [medium]）：看到 REBOTTOM 不代表置底真的生效。故障視窗
            # 仍存活時，用自己的 z-order 快照輪詢到故障解除（期限同樣是確認後 RecoverSec＋TolSec），
            # 再多等超過一個 watch-zorder 取樣間隔（200 ms），確保 z-order 記錄在故障視窗還在時就
            # 寫下恢復後的狀態；之後才關閉故障視窗。故障窗口 End 另外截在關閉那一刻
            # （Get-FaultWindowEnd），關閉後自然變乾淨的記錄不會被當成恢復。
            $releaseDeadline = $faultConfirmedDto.AddSeconds($RecoverSec + $TolSec)
            $faultReleased = $false
            while ([DateTimeOffset]::Now -lt $releaseDeadline) {
                if ($fault.Proc.HasExited) { break }
                if (-not (Test-FaultEstablished $hostPid $fault.Hwnd)) { $faultReleased = $true; break }
                Start-Sleep -Milliseconds 100
            }
            $faultAlive = -not $fault.Proc.HasExited
            if ($faultReleased) { Start-Sleep -Milliseconds 600 }
            # Opus 重審 F3 low：Test-FaultEstablished 在找不到任何小工具時也回 false——宿主若在
            # REBOTTOM 後崩潰、小工具全部消失，會被誤當成「故障已解除」。解除必須同時成立：宿主
            # 仍存活、且仍找得到可見小工具視窗。
            $hostAlive = [bool](Get-Process -Id $hostPid -ErrorAction SilentlyContinue)
            $widgetsLeft = @(Get-HostWidgetWindows $hostPid).Count
            $wlog.WriteLine("## $(Get-Ts) 故障視窗存活時 z-order 快照確認故障已解除=$faultReleased（故障視窗仍存活=$faultAlive，宿主存活=$hostAlive，可見小工具=$widgetsLeft）")
            $results['故障視窗仍存活時，z-order 快照確認小工具已回到一般視窗之下'] = $faultReleased -and $faultAlive -and $hostAlive -and ($widgetsLeft -gt 0)
        } else {
            $results["故障成立後 $RecoverSec 秒（+$TolSec 秒容差）內 gatekeeper.log 出現 REBOTTOM reason=safety-check"] = $false
        }
    }
    $faultClosedDto = [DateTimeOffset]::Now
    if ($fault.Proc -and -not $fault.Proc.HasExited) { Stop-Process -Id $fault.Proc.Id -Force -ErrorAction SilentlyContinue }
    $wlog.WriteLine("## $(Get-Ts) 故障視窗已關閉")

    # ── 9. 10 分鐘平靜期：REBOTTOM 行數完全不變 ────────────────────────────────────────
    Start-Sleep -Seconds 2
    $quietBaseline = Get-RebottomLines $gatekeeperLog
    $quietStart = Get-Ts
    $wlog.WriteLine("## $quietStart STEP：開始 $QuietSeconds 秒平靜期，目前 REBOTTOM 行數=$($quietBaseline.Count)")
    Start-Sleep -Seconds $QuietSeconds
    $quietEnd = Get-RebottomLines $gatekeeperLog
    $wlog.WriteLine("## $(Get-Ts) 平靜期結束，REBOTTOM 行數=$($quietEnd.Count)（期望與 $($quietBaseline.Count) 相同）")
    $results["平常 $QuietSeconds 秒內無任何重排"] = ($quietEnd.Count -eq $quietBaseline.Count)

    $wlog.WriteLine("## 截圖：略過（鎖定狀態=$([int](Test-Locked))）")
}
finally {
    if ($fault -and $fault.Proc -and -not $fault.Proc.HasExited) { Stop-Process -Id $fault.Proc.Id -Force -ErrorAction SilentlyContinue }
    if ($hostProc -and -not $hostProc.HasExited) { Stop-Process -Id $hostPid -Force -ErrorAction SilentlyContinue }
    try { Stop-ScratchForm $normalForm } catch { $wlog.WriteLine("## $(Get-Ts) 關閉一般視窗表單失敗：$_") }
    $regLeft = @(Restore-FcHostAutostartRegistry $regSnap)
    if ($regLeft.Count -gt 0) { Write-Warning "開機自啟登錄未還原：$($regLeft -join '; ')" }
    $wlog.WriteLine("# $(Get-Ts) 開機自啟登錄還原：未還原 $($regLeft.Count) 項$(if ($regLeft.Count) { '：' + ($regLeft -join '; ') })")
    $results['開機自啟登錄已還原（Run／StartupApproved 的 fc-host）'] = ($regLeft.Count -eq 0)
    $wlog.WriteLine("# $(Get-Ts) 宿主與一般視窗表單已結束；等待監控結束")
    $wlog.Close()
}

Stop-Process -Id $watcher.Id -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 500

# ── 10. 分階段分析 z-order 記錄（fix round 1，Codex task-3.3 [medium]）：explorer 重啟與故障
#        注入兩個預期窗口內的短暫異常不算違規，但窗口結束時必須已恢復；故障窗口內必須確實
#        觀察到異常；窗口以外（基準期、平靜期）任何異常都算違規。單行判準（只看 Tauri Window，
#        visible=0 必須伴隨 dupInSnapshot）同 verify-3.2.ps1，見 lib/VerifyVerdict.psm1。
# ────────────────────────────────────────────────────────────────────────────────
$lines = @(Get-Content $zlog | Where-Object { $_ -notmatch '^#' -and $_.Trim() })
$windows = @()
if ($explorerStartDto) {
    $expEnd = $(if ($explorerBackDto) { $explorerBackDto } else { $explorerStartDto }).AddSeconds($RecoverSec + $TolSec)
    $windows += @{ Name = 'explorer 重啟'; Start = $explorerStartDto; End = $expEnd }
}
if ($faultConfirmedDto) {
    # fix F3：End 截在關閉故障視窗那一刻（若早於確認＋RecoverSec＋TolSec），見 Get-FaultWindowEnd。
    $faultEnd = Get-FaultWindowEnd -ConfirmedAt $faultConfirmedDto -RecoverSec $RecoverSec -TolSec $TolSec -FaultClosedAt $faultClosedDto
    $windows += @{ Name = '故障注入'; Start = $faultInjectedDto; End = $faultEnd }
}
$verdict = Get-SafetyRecoveryVerdict -Lines $lines -Windows $windows -FaultName '故障注入' -FaultConfirmedAt $faultConfirmedDto
$results['z-order 記錄：故障窗口內確實觀察到「一般視窗在小工具之下」'] = [bool]$verdict.FaultObserved
$results["z-order 記錄：故障在確認後 $RecoverSec 秒（+$TolSec 秒容差）內恢復"] = [bool]$verdict.FaultRecovered
$results["z-order 記錄：含小工具的 $($verdict.WidgetLines) 行中，預期窗口以外（基準期／平靜期）小工具之下的一般視窗＝0"] =
($verdict.WidgetLines -gt 0) -and ($verdict.OutsideViolations.Count -eq 0)
$firstFg = @($lines | Where-Object { $_ -match "fgPid=$hostPid\b" } | ForEach-Object { $_.Substring(0, $_.IndexOf(' ')) })
$results['z-order 記錄：前景從未是宿主'] = ($firstFg.Count -eq 0)

$sum = New-EvidenceWriter $slogPath
$sum.WriteLine("# verify-3.3.ps1 summary $(Get-Ts)")
$sum.WriteLine("# explorer 重啟時間：$explorerRestartAt")
foreach ($w in $windows) { $sum.WriteLine("# 預期窗口「$($w.Name)」：$($w.Start.ToString('HH:mm:ss.fff'))～$($w.End.ToString('HH:mm:ss.fff'))") }
foreach ($k in $results.Keys) {
    $v = $results[$k]
    $tag = if ($v -is [string] -and $v -eq 'SKIPPED') { 'SKIP' } elseif ($v) { 'PASS' } else { 'FAIL' }
    $sum.WriteLine("$tag  $k")
}
foreach ($n in $verdict.Notes) { $sum.WriteLine("  note: $n") }
foreach ($v in $verdict.OutsideViolations) { $sum.WriteLine("  violation: $v") }
foreach ($f in $firstFg) { $sum.WriteLine("  fg=host: $f") }
$sum.WriteLine('# gatekeeper.log 內容：')
if (Test-Path $gatekeeperLog) { Get-Content $gatekeeperLog | ForEach-Object { $sum.WriteLine("  $_") } }
$sum.Close()
Get-Content $slogPath
try { Copy-EvidenceFile -Source $gatekeeperLog -Destination (Join-Path $OutDir '3.3-gatekeeper.log') } catch { Write-Warning "複製 gatekeeper.log 失敗：$_" }
Remove-Item -Recurse -Force $tempRoot -ErrorAction SilentlyContinue
# 結束碼：任何 FAIL → 1（SKIPPED 不計），全部通過 → 0。
$failed = @($results.Keys | Where-Object { $v = $results[$_]; -not ($v -is [string] -and $v -eq 'SKIPPED') -and -not $v })
if ($failed.Count -gt 0) { exit 1 }
exit 0
