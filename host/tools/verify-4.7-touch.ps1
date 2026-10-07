<#
.SYNOPSIS
    Task B2／B7 代驗腳本（合成觸控拖曳）：本機無觸控螢幕硬體，改用 Microsoft 文件化的
    `CreateSyntheticPointerDevice`＋`InjectSyntheticPointerInput`（PT_TOUCH）對真正在跑的
    `fc-host` macro 小工具視窗做「按住→上下拖曳→放開」，驗證清單捲動、且前景視窗／鍵盤焦點
    全程不變。

.DESCRIPTION
    對應 `.superpowers/sdd/tasks/human-checklist.md`：
      - B2（Task 1.3 焦點與觸控探針——觸控螢幕）：清單跟著手指捲動、前景／鍵盤焦點不搶走。
      - B7 人工項目 2（Task 4.7 原生捲動——觸控螢幕拖曳）：同上，對象換成真正的 macro 小工具
        （非 probe_focus_touch 專用探針視窗）。

    `CreateSyntheticPointerDevice`／`InjectSyntheticPointerInput`／`DestroySyntheticPointerDevice`
    是 Microsoft Learn 一手文件的合成指標注入 API（winuser.h，Windows 10 1809+），不需要實體
    觸控數位板即可運作；POINTER_FLAG_*／POINTER_INPUT_TYPE 等常數皆逐一對照 Microsoft Learn
    確認（見 `lib/SafeInput.psm1` 內對應區塊的註解與各常數來源頁）。所有觸控注入呼叫（含裝置
    建立／銷毀）一律經 `lib/SafeInput.psm1` 的 `Send-GuardedTouchDrag`，本腳本不得自行宣告或
    呼叫這些 API（同 `keybd_event`／`mouse_event`／`SendInput` 的既有規則）。

    步驟：
      1. `Invoke-SafeInputPreflight`（鎖定 → 結束碼 2；合成輸入不生效 → 結束碼 3，不重試）。
      2. 確認沒有既有 `fc-host` 在跑；暫存 `%APPDATA%`／`%LOCALAPPDATA%`（macro 預設開啟，
         資料目錄放入 fixture 確保清單夠長可捲動——同 `verify-4.7-wheel.ps1`）。
      3. 啟動宿主、找到 `fc-host macro` 視窗矩形，透過 CDP 讀取 `.scroll-area` 的初始 `scrollTop`。
      4. 前提：前景不是宿主（非使用中視窗）；觸控按下的座標必須真的落在小工具本身
         （`WindowFromPoint` 的 `GetAncestor(GA_ROOT)`＝macro hwnd，同 `verify-4.7-wheel.ps1`
         fix round 2 的做法）——觸控有隱式擷取（一旦在某視窗上按下，後續移動／放開都送給
         同一扇視窗，即使座標移出其矩形），故只需檢查按下點。
      5. 記錄目前前景視窗與其執行緒的鍵盤焦點（`GetGUIThreadInfo`，同 `probe-1.3.ps1`）。
      6. `Send-GuardedTouchDrag`：在小工具矩形內選兩個內容區域的點（避開表頭）拖曳。
      7. 再次讀 `scrollTop`、前景、鍵盤焦點，比對變化。

    結果與逐項 PASS/FAIL 印在終端機、寫進 `host/tools/evidence/4.7-touch-summary.log`
    （細節見 `4.7-touch-log.log`）。結束碼：0＝全部 PASS、1＝有 FAIL（含前提不成立）、
    2＝BLOCKED（鎖定）、3＝ENV-BLOCKED（合成輸入不生效；或遮擋判讀為環境類——工作列／系統 UI／最小化後
    仍蓋住，且整個矩形找不到乾淨點）。fix F8c：被一般視窗蓋住時暫時最小化（lib/Occluders.psm1）；按下點命中
    桌面、沒命中任何視窗、宿主另一扇視窗＝小工具不在預期位置，判 FAIL（結束碼 1），不找替代點。

.PARAMETER Exe
    fc-host.exe 路徑，預設 host/target/release/fc-host.exe。

.PARAMETER OutDir
    證據輸出目錄，預設 host/tools/evidence。

.PARAMETER CdpPort
    WebView2 remote debugging 埠，預設 9339（與既有 verify-*/9333-9337 分開避免衝突）。

.EXAMPLE
    工作階段確認未鎖定後：
    cd host; cargo build --release
    pwsh -File tools/verify-4.7-touch.ps1
#>
[CmdletBinding()]
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\fc-host.exe'),
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [int]$CdpPort = 9339
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\AutostartRegistry.psm1') -Force

Add-Type -Namespace V47T -Name Native -MemberDefinition @'
[DllImport("user32.dll")] public static extern System.IntPtr GetTopWindow(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern System.IntPtr GetWindow(System.IntPtr hWnd, uint cmd);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr hWnd, out uint pid);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(System.IntPtr hWnd, System.Text.StringBuilder s, int n);
[DllImport("user32.dll")] public static extern bool GetWindowRect(System.IntPtr hWnd, out RECT r);
[DllImport("user32.dll")] public static extern System.IntPtr GetForegroundWindow();
[DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
[DllImport("user32.dll")] public static extern System.IntPtr WindowFromPoint(POINT p);
[DllImport("user32.dll")] public static extern System.IntPtr GetAncestor(System.IntPtr hWnd, uint flags);
[DllImport("user32.dll")] public static extern bool GetGUIThreadInfo(uint idThread, ref GUITHREADINFO info);
// per-monitor DPI aware v2（同 probe-1.2.ps1）：本機混合 DPI 雙螢幕，pwsh 本身宣告的 DPI 感知層級
// 可能與小工具所在螢幕不同，若不校正，GetWindowRect／WindowFromPoint 用的座標空間與
// InjectSyntheticPointerInput 期待的實體像素座標可能不一致（前者有相容性虛擬化、後者未必有）。
[DllImport("user32.dll")] public static extern System.IntPtr SetThreadDpiAwarenessContext(System.IntPtr value);
[StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
[StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
[StructLayout(LayoutKind.Sequential)]
public struct GUITHREADINFO {
    public int cbSize;
    public uint flags;
    public System.IntPtr hwndActive;
    public System.IntPtr hwndFocus;
    public System.IntPtr hwndCapture;
    public System.IntPtr hwndMenuOwner;
    public System.IntPtr hwndMoveSize;
    public System.IntPtr hwndCaret;
    public RECT rcCaret;
}
'@

function Get-Ts { Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fffK' }
# 所有觸控注入（含裝置建立／銷毀、游標無關）一律經 lib/SafeInput.psm1（task B2/B7 代驗，
# 2026-09-29 新增 Send-GuardedTouchDrag）：每一次底層注入呼叫前重新檢查 LogonUI.exe，鎖定就丟
# BLOCKED 並停止後續所有注入。本腳本不得自行宣告或呼叫 CreateSyntheticPointerDevice／
# InjectSyntheticPointerInput。
Import-Module (Join-Path $PSScriptRoot 'lib\SafeInput.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\VerifyVerdict.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\Occluders.psm1') -Force
function Hex([IntPtr]$h) { '0x{0:X}' -f $h.ToInt64() }

# DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 = -4（Microsoft Learn）。只影響本執行緒；非零回傳
# 值＝前一個 context（成功），IntPtr.Zero＝失敗。
$prevDpiCtx = [V47T.Native]::SetThreadDpiAwarenessContext([IntPtr](-4))
Write-Host "SetThreadDpiAwarenessContext(PER_MONITOR_AWARE_V2) 前一個 context=$(Hex $prevDpiCtx) ok=$($prevDpiCtx -ne [IntPtr]::Zero)"

function Get-TopWindows {
    $list = New-Object System.Collections.Generic.List[IntPtr]
    $h = [V47T.Native]::GetTopWindow([IntPtr]::Zero)
    while ($h -ne [IntPtr]::Zero) { $list.Add($h); $h = [V47T.Native]::GetWindow($h, 2) }
    return $list
}

function Get-WindowTitle([IntPtr]$h) {
    $sb = New-Object System.Text.StringBuilder 256
    [void][V47T.Native]::GetWindowText($h, $sb, 256)
    return $sb.ToString()
}

function Find-HostWindow([int]$ProcId, [string]$TitleExact, [int]$TimeoutSec = 20) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt $TimeoutSec) {
        foreach ($h in (Get-TopWindows)) {
            $p = 0
            [void][V47T.Native]::GetWindowThreadProcessId($h, [ref]$p)
            if ($p -eq $ProcId -and [V47T.Native]::IsWindowVisible($h) -and (Get-WindowTitle $h) -eq $TitleExact) {
                return $h
            }
        }
        Start-Sleep -Milliseconds 300
    }
    return [IntPtr]::Zero
}

function Invoke-PageEval([string]$WidgetId, [string]$Expr) {
    $out = & node (Join-Path $PSScriptRoot 'host-cdp-eval.mjs') $CdpPort "w=$WidgetId" $Expr 2>&1
    return ($out -join '')
}

function Get-ScrollTop {
    $raw = Invoke-PageEval 'macro' "(() => { const el = document.querySelector('.scroll-area'); return el ? el.scrollTop : null; })()"
    try { return [double]($raw | ConvertFrom-Json) } catch { return $null }
}

# 遮擋前提（同 verify-4.7-wheel.ps1 fix round 2）：觸控按下的座標必須真的落在小工具本身。
# 觸控有隱式擷取（POINTER_INFO.hwndTarget 說明：一旦在某視窗上方按下即擷取該視窗），後續
# 移動／放開送給同一扇視窗，故只需確認按下點未被遮擋。
function Find-ClearPointInRect([int]$rx, [int]$ry, [int]$rw, [int]$rh, [IntPtr]$targetHwnd) {
    for ($dy = 20; $dy -lt $rh; $dy += 20) {
        for ($dx = 20; $dx -lt $rw; $dx += 20) {
            $pt = New-Object V47T.Native+POINT; $pt.X = $rx + $dx; $pt.Y = $ry + $dy
            $hAt = [V47T.Native]::WindowFromPoint($pt)
            if ($hAt -eq [IntPtr]::Zero) { continue }
            $root = [V47T.Native]::GetAncestor($hAt, 2)  # GA_ROOT
            if ($root -eq $targetHwnd) { return @{ X = $pt.X; Y = $pt.Y } }
        }
    }
    return $null
}

# fix F8c：由 Clear-Occluders 的結果決定按下座標（與 verify-4.7-wheel／wheelrouting 同一份判讀）。
# 判讀只用 lib 的 Get-OccluderVerdict／Assert-OccluderResult：ok → 偏好點；fail（桌面／沒命中／宿主另一扇／
# 腳本自己的視窗）→ 丟「FAIL: 」，不找替代點；env-blocked → 才在矩形內找替代點，找不到 → 丟「ENV-BLOCKED: 」。
function Resolve-WheelPoint($Occ, [int]$PreferX, [int]$PreferY, [scriptblock]$FindAlternative) {
    $v = Get-OccluderVerdict -Result $Occ
    if ($v.Verdict -eq 'ok') { return @{ X = $PreferX; Y = $PreferY } }
    if ($v.Verdict -ne 'env-blocked') { Assert-OccluderResult -Result $Occ -What '按下座標' }
    $alt = & $FindAlternative
    if ($null -ne $alt) { return $alt }
    Assert-OccluderResult -Result $Occ -What '整個 macro 小工具矩形'
    throw 'ENV-BLOCKED: 整個 macro 小工具矩形仍被其他視窗遮擋，不送觸控、不判定'
}

function Get-ThreadFocus([uint32]$threadId) {
    $info = New-Object V47T.Native+GUITHREADINFO
    $info.cbSize = [System.Runtime.InteropServices.Marshal]::SizeOf([type][V47T.Native+GUITHREADINFO])
    if ([V47T.Native]::GetGUIThreadInfo($threadId, [ref]$info)) { return $info.hwndFocus }
    return [IntPtr]::Zero
}

# ---------------------------------------------------------------- main
$pf = Invoke-SafeInputPreflight
if ($pf.ExitCode -ne 0) { Write-Host $pf.Message; exit $pf.ExitCode }
Write-Host $pf.Message
if (Get-Process -Name fc-host -ErrorAction SilentlyContinue) {
    throw '已有 fc-host 在執行，請先結束（WebView2 共用 user data folder，CDP 參數不會生效）。'
}
$Exe = (Resolve-Path $Exe).Path
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$logPath = Join-Path $OutDir '4.7-touch-log.log'
$sumPath = Join-Path $OutDir '4.7-touch-summary.log'
$log = New-EvidenceWriter $logPath
$log.AutoFlush = $true
$log.WriteLine("# verify-4.7-touch.ps1 start=$(Get-Ts) exe=$Exe")
$log.WriteLine("# SetThreadDpiAwarenessContext(PER_MONITOR_AWARE_V2) 前一個 context=$(Hex $prevDpiCtx) ok=$($prevDpiCtx -ne [IntPtr]::Zero)")

$fixtureSrc = Join-Path $PSScriptRoot '..\ui\fixtures\tw-events.json'
if (-not (Test-Path $fixtureSrc)) { throw "找不到 fixture：$fixtureSrc" }

$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ("fc-host-4.7t-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
$tempAppData = Join-Path $tempRoot 'Roaming'
$tempLocalAppData = Join-Path $tempRoot 'Local'
New-Item -ItemType Directory -Force -Path $tempAppData, $tempLocalAppData | Out-Null
$dataDir = Join-Path $tempLocalAppData 'tw.fintools.fc-host\data'
New-Item -ItemType Directory -Force -Path $dataDir | Out-Null
Copy-Item $fixtureSrc (Join-Path $dataDir 'tw_events.json') -Force
$log.WriteLine("# APPDATA=$tempAppData（首次啟動＝macro 預設開啟）LOCALAPPDATA=$tempLocalAppData（已放入 fixture）")

$regSnap = @(Save-FcHostAutostartRegistry)
$log.WriteLine("# $(Get-Ts) 開機自啟登錄快照：$(($regSnap | ForEach-Object { "$($_.Name)@$(Split-Path $_.Key -Leaf) Exists=$($_.Exists)" }) -join '; ')")
$results = [ordered]@{}
$blocked = $null
$envBlocked = $null
$minimized = New-Object System.Collections.Generic.List[object]
# hung-target（review 642050f）：沒還原成功的使用者視窗（無回應、最小化未確認、讀回逾時／不符）。屬環境問題，
# 不進逐項結果；摘要另起警示行（Format-UnrestoredWarning），結束碼經 Get-VerdictExitCode（環境＝3）。
$occNotRestored = New-Object System.Collections.Generic.List[string]
$hostProc = $null
$origCursor = New-Object V47T.Native+POINT
[void][V47T.Native]::GetCursorPos([ref]$origCursor)
$oldAppData = $env:APPDATA
$oldLocalAppData = $env:LOCALAPPDATA
$oldWv2 = $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS
try {
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

    $macroHwnd = Find-HostWindow $hostPid 'fc-host macro'
    $ok = ($macroHwnd -ne [IntPtr]::Zero)
    $results['macro 小工具視窗出現'] = $ok
    if (-not $ok) { throw '逾時等不到 fc-host macro 視窗' }
    $rect = New-Object V47T.Native+RECT
    [void][V47T.Native]::GetWindowRect($macroHwnd, [ref]$rect)
    $log.WriteLine("## $(Get-Ts) macro hwnd=$(Hex $macroHwnd) rect=($($rect.Left),$($rect.Top))-($($rect.Right),$($rect.Bottom))")

    Start-Sleep -Seconds 2  # 等頁面資料載入、內容渲染完成

    # fix F8c：遮擋清除必須在取前景基準之前——被最小化的可能正是前景視窗（操作者的終端機），
    # 順序反了會讓後面「未搶走前景／焦點」誤判 FAIL。
    # 遮擋前提：觸控按下點必須真的落在小工具本身。內容區在表頭（約 53px）之下，取矩形下半部
    # 較靠底、上半部較靠表頭下緣的兩個點做拖曳（往上拖＝手指上滑，多數觸控裝置對應內容往上
    # 捲動，但本腳本用 |Δ scrollTop|>0 判斷，不預設方向）。
    $rw = $rect.Right - $rect.Left
    $rh = $rect.Bottom - $rect.Top
    $headerPx = 60
    $usable = $rh - $headerPx
    if ($usable -lt 80) { throw "PRECONDITION: 小工具矩形太小（高度 $rh px），內容區不足 80px 可拖曳" }
    $cx = [int](($rect.Left + $rect.Right) / 2)
    $startY = $rect.Top + $headerPx + [int]($usable * 0.75)
    $endY = $rect.Top + $headerPx + [int]($usable * 0.15)
    # fix F8c：與 verify-4.7-wheel 一致——被使用者的一般視窗蓋住時暫時最小化（lib/Occluders.psm1，finally 還原）；
    # 判讀一律經 lib 的 Get-OccluderVerdict／Assert-OccluderResult（見 Resolve-WheelPoint）：桌面／沒命中／
    # 宿主另一扇＝FAIL（不找替代點）；環境類才找替代點，整個矩形找不到＝ENV-BLOCKED（結束碼 3）。
    $occ = Clear-Occluders -HostPid $hostPid -TargetHwnd $macroHwnd -Points @(, @($cx, $startY)) -Minimized $minimized `
        -Log { param($m) $log.WriteLine("## $(Get-Ts) $m") }
    $log.WriteLine("## $(Get-Ts) 前提：遮擋清除結果 prefer=($cx,$startY) ok=$($occ.Ok) action=$($occ.Action) $($occ.Reason) macroHwnd=$(Hex $macroHwnd)")
    $clear = Resolve-WheelPoint -Occ $occ -PreferX $cx -PreferY $startY -FindAlternative {
        Find-ClearPointInRect $rect.Left $rect.Top $rw $rh $macroHwnd
    }
    $log.WriteLine("## $(Get-Ts) 前提：按下座標 clear=($($clear.X),$($clear.Y))")
    $results['前提：觸控按下座標落在小工具本身（WindowFromPoint 未被其他視窗遮擋）'] = $true
    $startX = $clear.X
    $startYActual = $clear.Y
    # 找到的乾淨點若非優先點（被遮擋退到網格搜尋），終點沿用同一 X、往 endY 方向拖曳。

    $baseFg = [V47T.Native]::GetForegroundWindow()
    $baseFgPid = 0
    $baseFgTid = 0
    if ($baseFg -ne [IntPtr]::Zero) { $baseFgTid = [V47T.Native]::GetWindowThreadProcessId($baseFg, [ref]$baseFgPid) }
    $baseFocus = Get-ThreadFocus $baseFgTid
    $before = Get-ScrollTop
    $log.WriteLine("## $(Get-Ts) 拖曳前 scrollTop=$before fg=$(Hex $baseFg) fgPid=$baseFgPid focus=$(Hex $baseFocus) hostPid=$hostPid")
    $results['讀到初始 scrollTop（CDP 連得上）'] = ($null -ne $before)
    if ($null -eq $before) { throw '讀不到初始 scrollTop（CDP 連線或頁面選取器有問題）' }

    # 非使用中視窗前提（同 verify-4.7-wheel.ps1）：前景有效且不屬於宿主。
    $pre = Test-InactiveWidgetPrecondition -FgHwnd $baseFg -FgPid ([int]$baseFgPid) -HostPid $hostPid
    $log.WriteLine("## $(Get-Ts) 非使用中前提：ok=$($pre.Ok) $($pre.Reason)")
    $results['前提：拖曳前前景不是小工具（小工具為非使用中視窗）'] = $pre.Ok
    if (-not $pre.Ok) { throw "PRECONDITION: $($pre.Reason)" }


    $log.WriteLine("## $(Get-Ts) Send-GuardedTouchDrag ($startX,$startYActual) -> ($startX,$endY)")
    Send-GuardedTouchDrag -StartX $startX -StartY $startYActual -EndX $startX -EndY $endY -Steps 12 -StepDelayMs 40
    Start-Sleep -Milliseconds 500

    $afterFg = [V47T.Native]::GetForegroundWindow()
    $afterFgPid = 0
    $afterFgTid = 0
    if ($afterFg -ne [IntPtr]::Zero) { $afterFgTid = [V47T.Native]::GetWindowThreadProcessId($afterFg, [ref]$afterFgPid) }
    $afterFocus = Get-ThreadFocus $afterFgTid
    $after = Get-ScrollTop
    $log.WriteLine("## $(Get-Ts) 拖曳後 scrollTop=$after fg=$(Hex $afterFg) fgPid=$afterFgPid focus=$(Hex $afterFocus)")

    $scrolled = ($null -ne $after) -and ($null -ne $before) -and ([Math]::Abs($after - $before) -gt 0.01)
    $results['觸控拖曳捲動小工具內容（scrollTop 改變）'] = $scrolled
    # fix F4（review 4.7-minor [medium]）：觸控沒有作用到小工具時，前景與焦點本來就不會變，這兩項
    # PASS 不帶任何資訊（曾被拿去當「注入無副作用」的旁證）。以「確實捲動了」為前提，未捲動一律 FAIL。
    $fgKept = ($afterFg -eq $baseFg) -and ([int]$afterFgPid -ne $hostPid)
    $focusKept = ($afterFocus -eq $baseFocus)
    if (-not $scrolled) { $log.WriteLine("## $(Get-Ts) 未捲動：前景／焦點兩項不具資訊，判 FAIL（原始值 fgKept=$fgKept focusKept=$focusKept）") }
    $results['觸控拖曳未搶走前景視窗（前提：確實捲動）'] = $scrolled -and $fgKept
    $results['觸控拖曳未搶走鍵盤焦點（前提：確實捲動）'] = $scrolled -and $focusKept

    if ($scrolled -and $fgKept -and $focusKept) {
        Write-Host 'PASS：合成觸控拖曳成功捲動 macro 小工具，且未搶前景／鍵盤焦點' -ForegroundColor Green
    } else {
        Write-Host 'FAIL：見逐項結果' -ForegroundColor Red
    }
}
catch {
    if ("$_" -like 'BLOCKED*') { $blocked = "$_"; $log.WriteLine("# $(Get-Ts) $_") }
    elseif ("$_" -like 'PRECONDITION*') { $log.WriteLine("# $(Get-Ts) $_"); Write-Host "FAIL：$_" -ForegroundColor Red }
    # 小工具不在預期位置（fix F8c，Assert-OccluderResult 丟「FAIL: 」）：記 FAIL、不送觸控，結束碼 1。
    elseif ("$_" -like 'FAIL:*') {
        $results['前提：觸控按下座標落在小工具本身（WindowFromPoint 未被其他視窗遮擋）'] = $false
        $log.WriteLine("# $(Get-Ts) $_"); Write-Host "$_" -ForegroundColor Red
    }
    # 遮擋清不掉（fix F8c）：環境問題，不送觸控、不判 FAIL，以結束碼 3（ENV-BLOCKED）結束。
    elseif ("$_" -like 'ENV-BLOCKED*') { $envBlocked = "$_"; $log.WriteLine("# $(Get-Ts) $_"); Write-Host "$_" -ForegroundColor Yellow }
    else { throw }
}
finally {
    if ($hostProc -and -not $hostProc.HasExited) { Stop-Process -Id $hostProc.Id -Force -ErrorAction SilentlyContinue }
    # 被暫時最小化的使用者視窗照原 WINDOWPLACEMENT 還原（fix F8c）。
    try { Restore-Occluders -Minimized $minimized -NotRestored $occNotRestored -Log { param($m) $log.WriteLine("# $(Get-Ts) $m") } } catch { $log.WriteLine("# 還原被最小化的視窗失敗：$_") }
    # $minimized 非空＝Restore-Occluders 本身中途丟例外、沒跑完；一併列為未還原（環境，不進逐項結果）。
    if ($minimized.Count -gt 0) { $occNotRestored.Add("Restore-Occluders 未跑完：尚有 $($minimized.Count) 扇未處理（見記錄）") }
    try { Set-GuardedCursorPos $origCursor.X $origCursor.Y -What '還原游標' } catch { $log.WriteLine("# 還原游標略過：$_") }
    $regLeft = @(Restore-FcHostAutostartRegistry $regSnap)
    if ($regLeft.Count -gt 0) { Write-Warning "開機自啟登錄未還原：$($regLeft -join '; ')" }
    $log.WriteLine("# $(Get-Ts) 開機自啟登錄還原：未還原 $($regLeft.Count) 項$(if ($regLeft.Count) { '：' + ($regLeft -join '; ') })")
    $results['開機自啟登錄已還原（Run／StartupApproved 的 fc-host）'] = ($regLeft.Count -eq 0)
    $log.WriteLine("# $(Get-Ts) 宿主已結束")
    $log.Close()
    Remove-Item -Recurse -Force $tempRoot -ErrorAction SilentlyContinue
}
$occWarn = Format-UnrestoredWarning -NotRestored $occNotRestored
if ($blocked) {
    Write-Host "BLOCKED：工作階段鎖定，已停止（未重試）。$blocked"
    if ($occWarn) {
        Write-Host $occWarn -ForegroundColor Yellow
        # 中途停止沒有逐項摘要：把警示行（連同停止原因）落檔，否則只剩終端機輸出會隨 session 消失。
        @("# verify-4.7-touch.ps1 summary $(Get-Ts)", "BLOCKED  $blocked", $occWarn) | ConvertTo-EvidenceText | Set-Content -Path $sumPath -Encoding utf8
    }
    # 鎖定中止：不做驗收判讀（逐項結果不完整），結束碼經共用決策（鎖定＝2）。
    exit (Get-VerdictExitCode -Locked -NotRestored $occNotRestored)
}

$sum = New-EvidenceWriter $sumPath
$sum.WriteLine("# verify-4.7-touch.ps1 summary $(Get-Ts)")
foreach ($k in $results.Keys) { $sum.WriteLine("$(if ($results[$k]) { 'PASS' } else { 'FAIL' })  $k") }
if ($envBlocked) { $sum.WriteLine("BLOCKED  $envBlocked") }
if ($occWarn) { $sum.WriteLine($occWarn) }
$sum.Close()
Get-Content $sumPath
if ($occWarn) { Write-Host $occWarn -ForegroundColor Yellow }
exit (Get-VerdictExitCode -Results $results -EnvBlocked:([bool]$envBlocked) -NotRestored $occNotRestored -RequireResults)
