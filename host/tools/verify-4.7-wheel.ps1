<#
.SYNOPSIS
    Task 4.7 驗收驅動腳本（原生捲動——真正的宿主小工具視窗＋真實滑鼠滾輪）：對實際執行中的
    `fc-host` macro 小工具視窗送 OS 層 `MOUSEEVENTF_WHEEL`，透過 CDP 讀取捲動前後的
    `scrollTop`，確認 Windows 對「非使用中視窗」的滾輪路由（游標下的視窗收到 `WM_MOUSEWHEEL`，
    不看鍵盤焦點）在真正的小工具視窗上也成立——這一段 headless Edge（`verify-4.7-scroll.mjs`）
    測不到，因為 headless 測試從頭到尾都在同一個瀏覽器分頁裡合成事件，不會經過 OS 的滾輪路由。

.DESCRIPTION
    task-4.7-brief.md：「在宿主中滾輪捲動可自動驗（需未鎖定＋SendInput MOUSEEVENTF_WHEEL；用
    CDP 讀 scrollTop 前後值）」；與 task 1.3 補跑的滾輪驗證（`probe-1.3.ps1` 的情境 B，測的是
    focus/foreground 語意用的專用探針視窗）互補但不重複：本腳本測的是**真正的 macro 小工具**、
    讀的是**真正的 `.scroll-area` DOM**，不是探針視窗的內部記錄檔。

    步驟：
      1. 鎖定檢查（`LogonUI.exe` 在跑就中止，結束碼 2，不重試不繞過——同
         `probe-1.3.ps1`／`global-constraints.md` 的安全模式）。
      2. 確認沒有既有 `fc-host` 在跑（WebView2 共用 user data folder，已有實例時 CDP 參數
         不生效——同 verify-3.1/3.2/4.6 的前提）。
      3. 暫存 `%APPDATA%`／`%LOCALAPPDATA%`：macro 小工具預設開啟（`default_widgets()`），
         資料目錄預先放入 `host/ui/fixtures/tw-events.json` 的內容（19 筆總經事件，已知在
         500px 寬、560px 上限下會產生捲軸，見 registry.js 註解與
         `regression-macro-scroll.mjs` 的前提檢查），確保這次驗收不受「使用者真實資料量
         夠不夠捲」影響，且完全不碰使用者真正的設定檔／資料目錄。
      4. 以 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=<CdpPort>` 啟動
         宿主，等 `fc-host macro` 視窗出現，用 `GetWindowRect` 取得它的螢幕座標矩形。
      5. 透過 CDP（`host-cdp-eval.mjs`）讀 `.scroll-area` 的 `scrollTop` 當基準值。
      6. **前提（fix round 1，Codex task-4.7 [medium]）**：送滾輪前確認前景視窗有效、且不屬於
         宿主行程（小工具＝非使用中視窗）；不成立就記 FAIL、不送滾輪——否則小工具本身是使用中
         視窗，滾輪捲得動也證明不了「非使用中視窗」的滾輪路由。
      6b. **前提（fix round 2，補 deferred minor）**：滾輪座標必須真的落在小工具本身
         （`WindowFromPoint` 命中處的 `GetAncestor(GA_ROOT)` 要等於 macro 小工具 hwnd），
         否則測到的是蓋在上面的其他一般視窗（2026-09-28 實機驗收就踩過一次，座標命中使用者
         的 VS Code 視窗）。滾輪點＝`.scroll-area` 的中心（fix F10：CDP 讀 getBoundingClientRect
         與 devicePixelRatio，經 ClientToScreen 換成實體座標，`lib/ScrollAreaTarget.psm1`；先確認
         清單可捲動）；被使用者的一般視窗蓋住時暫時最小化它（`lib/Occluders.psm1`，結束時照原位置
         還原；fix F7，與 7.5／7.6／5.3 一致），仍蓋住才在清單矩形內網格搜尋（同 `probe-1.3.ps1` 的
         `Find-ClearPointInRect`），再找不到＝ENV-BLOCKED（結束碼 3），不送滾輪、不判 FAIL。最終點
         再以 elementFromPoint 確認命中 `.scroll-area`，不命中＝FAIL、不送滾輪。這一步在取前景基準之前做。
         把游標移到找到的乾淨座標，送 6 格 `MOUSEEVENTF_WHEEL`（負值＝向下捲動）。
      7. 再次透過 CDP 讀 `scrollTop`，確認變大；同時記錄捲動前後 `GetForegroundWindow()`
         不變（滾輪捲動不應搶走前景——這是 D8 已經論證過的滾輪路由機制的附帶驗證，不是
         本腳本的主要目的，1.3 的探針才是這項的權威驗證）。
      8. 結束宿主，還原游標位置，刪除暫存目錄。

    結果與逐項 PASS/FAIL 印在終端機、寫進 `host/tools/evidence/4.7-wheel-summary.log`
    （細節見 `4.7-wheel-log.log`）。結束碼：0＝全部 PASS、1＝有 FAIL（含前提不成立）、
    2＝BLOCKED（鎖定）、3＝ENV-BLOCKED（合成輸入不生效，見 lib/SafeInput.psm1；或遮擋判讀為環境類：
    工作列／系統 UI／最小化後仍蓋住，且整個小工具矩形找不到乾淨點）。fix F8c：按下點命中桌面、沒命中任何
    視窗、宿主另一扇視窗＝小工具不在預期位置，判 FAIL（結束碼 1），不找替代點。

.PARAMETER Exe
    fc-host.exe 路徑，預設 host/target/release/fc-host.exe。

.PARAMETER OutDir
    證據輸出目錄，預設 host/tools/evidence。

.PARAMETER CdpPort
    WebView2 remote debugging 埠，預設 9337（與 verify-3.1/3.2/3.3/4.6 的
    9333/9334/9335/9336 分開，避免同時跑時衝突）。

.EXAMPLE
    工作階段確認未鎖定後：
    cd host; cargo build --release
    pwsh -File tools/verify-4.7-wheel.ps1
#>
[CmdletBinding()]
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\fc-host.exe'),
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [int]$CdpPort = 9337
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\AutostartRegistry.psm1') -Force

Add-Type -Namespace V47 -Name Native -MemberDefinition @'
[DllImport("user32.dll")] public static extern System.IntPtr SetThreadDpiAwarenessContext(System.IntPtr ctx);
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
[DllImport("user32.dll")] public static extern bool ClientToScreen(System.IntPtr hWnd, ref POINT p);
[StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
[StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
'@

# fix F9：Per-Monitor-V2（-4），在任何座標取得之前。GetWindowRect、WindowFromPoint、GetCursorPos、
# lib/Occluders 的命中測試與 SafeInput 的游標／滾輪座標都依本執行緒的 DPI 感知解讀；pwsh 預設 unaware。
[void][V47.Native]::SetThreadDpiAwarenessContext([IntPtr](-4))

function Get-Ts { Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fffK' }
# 所有滑鼠注入（含游標移動）一律經 lib/SafeInput.psm1（task 1.1 fix round 1）：每一次底層
# 注入呼叫前重新檢查 LogonUI.exe（LockApp 可能殘留，不用它判斷），鎖定就丟 BLOCKED 並停止
# 後續所有注入。本腳本不得自行宣告或呼叫注入 API。
Import-Module (Join-Path $PSScriptRoot 'lib\SafeInput.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\VerifyVerdict.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\Occluders.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\ScrollAreaTarget.psm1') -Force
function Hex([IntPtr]$h) { '0x{0:X}' -f $h.ToInt64() }

function Get-TopWindows {
    $list = New-Object System.Collections.Generic.List[IntPtr]
    $h = [V47.Native]::GetTopWindow([IntPtr]::Zero)
    while ($h -ne [IntPtr]::Zero) { $list.Add($h); $h = [V47.Native]::GetWindow($h, 2) }
    return $list
}

function Get-WindowTitle([IntPtr]$h) {
    $sb = New-Object System.Text.StringBuilder 256
    [void][V47.Native]::GetWindowText($h, $sb, 256)
    return $sb.ToString()
}

function Find-HostWindow([int]$ProcId, [string]$TitleExact, [int]$TimeoutSec = 20) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt $TimeoutSec) {
        foreach ($h in (Get-TopWindows)) {
            $p = 0
            [void][V47.Native]::GetWindowThreadProcessId($h, [ref]$p)
            if ($p -eq $ProcId -and [V47.Native]::IsWindowVisible($h) -and (Get-WindowTitle $h) -eq $TitleExact) {
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

# fix F10：.scroll-area 的位置、可捲動量與 devicePixelRatio（運算式見 lib/ScrollAreaTarget.psm1）。
function Get-ScrollAreaProbe {
    $raw = Invoke-PageEval 'macro' (Get-ScrollAreaProbeExpression)
    try { return ($raw | ConvertFrom-Json) } catch { return $null }
}

function Get-ScrollAreaHitAt([double]$CssX, [double]$CssY) {
    $raw = Invoke-PageEval 'macro' (Get-ScrollAreaHitExpression -CssX $CssX -CssY $CssY)
    try { return ($raw | ConvertFrom-Json) } catch { return $null }
}

# 遮擋前提（deferred minor，補於 task-4.7-report.md「實機驗收」節記載的缺口）：送滾輪的座標
# 必須真的落在小工具本身，否則測到的是蓋在上面的其他一般視窗（2026-09-28 實機驗收就踩過一次，
# 座標命中使用者的 VS Code 視窗，靠臨時 SW_MINIMIZE 該視窗才補跑成功，但腳本本身當時沒有這道
# 前提檢查，「巧合命中」與「腳本自己驗證過」是兩回事）。做法同 probe-1.3.ps1 的
# Find-ClearPointInRect：以 GA_ROOT=2 取 WindowFromPoint 命中處的最上層祖先視窗，要求等於
# 目標小工具視窗本身（比對 hwnd，而非 pid——宿主行程可能同時擁有多個小工具視窗，只比對 pid
# 不足以確定命中的正是這一個）。
function Find-ClearPointInRect([int]$rx, [int]$ry, [int]$rw, [int]$rh, [IntPtr]$targetHwnd) {
    for ($dy = 20; $dy -lt $rh; $dy += 20) {
        for ($dx = 20; $dx -lt $rw; $dx += 20) {
            $pt = New-Object V47.Native+POINT; $pt.X = $rx + $dx; $pt.Y = $ry + $dy
            $hAt = [V47.Native]::WindowFromPoint($pt)
            if ($hAt -eq [IntPtr]::Zero) { continue }
            $root = [V47.Native]::GetAncestor($hAt, 2)  # GA_ROOT
            if ($root -eq $targetHwnd) { return @{ X = $pt.X; Y = $pt.Y } }
        }
    }
    return $null
}

# fix F8c：由 Clear-Occluders 的結果決定滾輪座標。判讀只用 lib 的 Get-OccluderVerdict／Assert-OccluderResult：
#   ok → 偏好點；fail（桌面／沒命中／宿主另一扇／腳本自己的視窗）→ 丟「FAIL: 」，不找替代點（小工具不在
#   預期位置是產品問題）；env-blocked → 才在矩形內找替代點，整個矩形都找不到 → 丟「ENV-BLOCKED: 」。
function Resolve-WheelPoint($Occ, [int]$PreferX, [int]$PreferY, [scriptblock]$FindAlternative) {
    $v = Get-OccluderVerdict -Result $Occ
    if ($v.Verdict -eq 'ok') { return @{ X = $PreferX; Y = $PreferY } }
    if ($v.Verdict -ne 'env-blocked') { Assert-OccluderResult -Result $Occ -What '滾輪座標' }
    $alt = & $FindAlternative
    if ($null -ne $alt) { return $alt }
    Assert-OccluderResult -Result $Occ -What '整個 macro 小工具矩形'
    throw 'ENV-BLOCKED: 整個 macro 小工具矩形仍被其他視窗遮擋，不送滾輪、不判定'
}

# ---------------------------------------------------------------- main
# 開始前的前置探查（SafeInput fix round 3）：鎖定 → 結束碼 2（BLOCKED）；未鎖定但合成輸入沒被系統
# 計入 → 結束碼 3（ENV-BLOCKED，memory logonui-unlocked-but-synthetic-input-inert），不產生無資訊的 PASS/FAIL。
$pf = Invoke-SafeInputPreflight
if ($pf.ExitCode -ne 0) { Write-Host $pf.Message; exit $pf.ExitCode }
Write-Host $pf.Message
if (Get-Process -Name fc-host -ErrorAction SilentlyContinue) {
    throw '已有 fc-host 在執行，請先結束（WebView2 共用 user data folder，CDP 參數不會生效）。'
}
$Exe = (Resolve-Path $Exe).Path
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$logPath = Join-Path $OutDir '4.7-wheel-log.log'
$sumPath = Join-Path $OutDir '4.7-wheel-summary.log'
$log = New-EvidenceWriter $logPath
$log.AutoFlush = $true
$log.WriteLine("# verify-4.7-wheel.ps1 start=$(Get-Ts) exe=$Exe")

$fixtureSrc = Join-Path $PSScriptRoot '..\ui\fixtures\tw-events.json'
if (-not (Test-Path $fixtureSrc)) { throw "找不到 fixture：$fixtureSrc" }

$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ("fc-host-4.7-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
$tempAppData = Join-Path $tempRoot 'Roaming'
$tempLocalAppData = Join-Path $tempRoot 'Local'
New-Item -ItemType Directory -Force -Path $tempAppData, $tempLocalAppData | Out-Null
$dataDir = Join-Path $tempLocalAppData 'tw.fintools.fc-host\data'
New-Item -ItemType Directory -Force -Path $dataDir | Out-Null
Copy-Item $fixtureSrc (Join-Path $dataDir 'tw_events.json') -Force
$log.WriteLine("# APPDATA=$tempAppData（首次啟動＝macro 預設開啟）LOCALAPPDATA=$tempLocalAppData（已放入 fixture 當 tw_events.json，確保清單夠長可捲動）")

$regSnap = @(Save-FcHostAutostartRegistry)
$log.WriteLine("# $(Get-Ts) 開機自啟登錄快照：$(($regSnap | ForEach-Object { "$($_.Name)@$(Split-Path $_.Key -Leaf) Exists=$($_.Exists)" }) -join '; ')")
$results = [ordered]@{}
$blocked = $null
$envBlocked = $null
$minimized = New-Object System.Collections.Generic.List[object]
# hung-target（review 642050f）：沒還原成功的使用者視窗（無回應、最小化未確認、讀回逾時／不符），寫進摘要。
$occNotRestored = New-Object System.Collections.Generic.List[string]
$hostProc = $null
$origCursor = New-Object V47.Native+POINT
[void][V47.Native]::GetCursorPos([ref]$origCursor)
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
    $rect = New-Object V47.Native+RECT
    [void][V47.Native]::GetWindowRect($macroHwnd, [ref]$rect)
    $log.WriteLine("## $(Get-Ts) macro hwnd=$(Hex $macroHwnd) rect=($($rect.Left),$($rect.Top))-($($rect.Right),$($rect.Bottom))")

    Start-Sleep -Seconds 2  # 等頁面資料載入、內容渲染完成

    # fix F10：滾輪點由頁面 .scroll-area 的實際位置決定（lib/ScrollAreaTarget.psm1），不再用
    # 「視窗矩形 Top＋固定偏移」——fix F9 改 PMv2 後 GetWindowRect 是實體像素，固定偏移的單位悄悄從
    # 邏輯 px 變成實體 px（150% 下 150→100 CSS px）。CDP 讀清單的 getBoundingClientRect 與
    # devicePixelRatio，以 ClientToScreen（PMv2＝實體像素；小工具視窗無邊框，webview 填滿客戶區）
    # 換成螢幕座標，取清單與 viewport 交集的中心。前提一：清單確實可捲動且尺寸有效、與 viewport 有交集，否則「scrollTop 增加」沒有意義。
    $origin = New-Object V47.Native+POINT
    [void][V47.Native]::ClientToScreen($macroHwnd, [ref]$origin)
    $probe = Get-ScrollAreaProbe
    $scr = Test-ScrollAreaScrollable -Probe $probe
    $log.WriteLine("## $(Get-Ts) 前提：清單探查 origin=($($origin.X),$($origin.Y)) probe=$(if ($probe) { $probe | ConvertTo-Json -Compress } else { 'null' }) ok=$($scr.Ok) $($scr.Reason)")
    $results['前提：.scroll-area 可捲動且尺寸有效（scrollHeight > clientHeight > 0、寬高與 dpr > 0、與 viewport 有交集）'] = $scr.Ok
    if (-not $scr.Ok) { throw "PRECONDITION: $($scr.Reason)" }
    $target = Get-ScrollAreaWheelTarget -Probe $probe -OriginX $origin.X -OriginY $origin.Y
    $log.WriteLine("## $(Get-Ts) 選點：清單中心 css=($($target.CssX),$($target.CssY)) dpr=$($target.Dpr) → 螢幕 ($($target.X),$($target.Y))；清單實體矩形=($($target.Left),$($target.Top)) $($target.Width)x$($target.Height)")

    # 前提（fix round 2，補 deferred minor）：滾輪座標必須真的落在小工具本身（WindowFromPoint
    # 的 root ancestor＝小工具 hwnd），否則測到的是蓋在上面的其他一般視窗，不是小工具的行為。
    # fix F7：被使用者的一般視窗蓋住時，與 7.5／7.6／5.3 一致——暫時最小化（lib/Occluders.psm1，
    # finally 照原位置還原）；仍蓋住才退到清單矩形做網格搜尋，再找不到＝ENV-BLOCKED（結束碼 3），
    # 不送滾輪、不判 FAIL。必須在取前景基準之前做：被最小化的可能正是前景視窗。
    $preferX = $target.X
    $preferY = $target.Y
    $occ = Clear-Occluders -HostPid $hostPid -TargetHwnd $macroHwnd -Points @(, @($preferX, $preferY)) -Minimized $minimized `
        -Log { param($m) $log.WriteLine("## $(Get-Ts) $m") }
    # fix F8c：判讀一律經 lib 的 Get-OccluderVerdict／Assert-OccluderResult（見 Resolve-WheelPoint）：
    # 偏好點命中桌面／沒命中／宿主另一扇＝FAIL（不找替代點）；工作列／系統 UI／最小化後仍蓋＝才找替代點，
    # 整個矩形都找不到＝ENV-BLOCKED。
    $log.WriteLine("## $(Get-Ts) 前提：遮擋清除結果 prefer=($preferX,$preferY) ok=$($occ.Ok) action=$($occ.Action) $($occ.Reason) macroHwnd=$(Hex $macroHwnd)")
    $clear = Resolve-WheelPoint -Occ $occ -PreferX $preferX -PreferY $preferY -FindAlternative {
        Find-ClearPointInRect $target.Left $target.Top $target.Width $target.Height $macroHwnd
    }
    $log.WriteLine("## $(Get-Ts) 前提：滾輪座標 clear=($($clear.X),$($clear.Y))")
    $results['前提：滾輪座標落在小工具本身（WindowFromPoint 未被其他視窗遮擋）'] = $true

    # 前提二（fix F10）：最終點（含遮擋時的替代點）換回 CSS 座標，elementFromPoint 必須命中
    # .scroll-area 或其子孫；座標再算錯會在這裡被抓到，不送滾輪。
    $css = ConvertTo-CssPoint -OriginX $origin.X -OriginY $origin.Y -Dpr $target.Dpr -X $clear.X -Y $clear.Y
    $hit = Test-ScrollAreaHit -Hit (Get-ScrollAreaHitAt $css.X $css.Y)
    $log.WriteLine("## $(Get-Ts) 前提：滾輪點命中 css=($($css.X),$($css.Y)) ok=$($hit.Ok) $($hit.Reason)")
    $results['前提：滾輪點命中 .scroll-area（elementFromPoint）'] = $hit.Ok
    if (-not $hit.Ok) { throw "PRECONDITION: $($hit.Reason)" }

    $baseFg = [V47.Native]::GetForegroundWindow()
    $baseFgPid = 0
    if ($baseFg -ne [IntPtr]::Zero) { [void][V47.Native]::GetWindowThreadProcessId($baseFg, [ref]$baseFgPid) }
    $before = Get-ScrollTop
    $log.WriteLine("## $(Get-Ts) 捲動前 scrollTop=$before fg=$(Hex $baseFg) fgPid=$baseFgPid hostPid=$hostPid")
    $results['讀到初始 scrollTop（CDP 連得上）'] = ($null -ne $before)
    if ($null -eq $before) { throw '讀不到初始 scrollTop（CDP 連線或頁面選取器有問題）' }

    # 非使用中視窗前提：前景有效且不屬於宿主（Codex task-4.7 [medium]）。
    $pre = Test-InactiveWidgetPrecondition -FgHwnd $baseFg -FgPid ([int]$baseFgPid) -HostPid $hostPid
    $log.WriteLine("## $(Get-Ts) 非使用中前提：ok=$($pre.Ok) $($pre.Reason)")
    $results['前提：送滾輪前前景不是小工具（小工具為非使用中視窗）'] = $pre.Ok
    if (-not $pre.Ok) { throw "PRECONDITION: $($pre.Reason)" }

    $cx = $clear.X
    $cy = $clear.Y
    Send-GuardedWheel $cx $cy 6
    Start-Sleep -Milliseconds 500

    $afterFg = [V47.Native]::GetForegroundWindow()
    $after = Get-ScrollTop
    $log.WriteLine("## $(Get-Ts) 捲動後 scrollTop=$after fg=$(Hex $afterFg)")
    $scrolled = ($null -ne $after) -and ($after -gt $before)
    $results['滾輪捲動小工具內容（scrollTop 增加）'] = $scrolled
    $afterFgPid = 0
    if ($afterFg -ne [IntPtr]::Zero) { [void][V47.Native]::GetWindowThreadProcessId($afterFg, [ref]$afterFgPid) }
    $results['滾輪捲動未搶走前景視窗'] = ($afterFg -eq $baseFg) -and ([int]$afterFgPid -ne $hostPid)

    if ($scrolled -and ($afterFg -eq $baseFg)) {
        Write-Host 'PASS：真實 OS 滾輪成功捲動 macro 小工具，且未搶前景' -ForegroundColor Green
    } else {
        Write-Host 'FAIL：見逐項結果' -ForegroundColor Red
    }
}
catch {
    # 注入途中偵測到鎖定：SafeInput 已停止後續所有注入；記下後以結束碼 2 結束，不重試。
    if ("$_" -like 'BLOCKED*') { $blocked = "$_"; $log.WriteLine("# $(Get-Ts) $_") }
    # 前提不成立：不送滾輪，照常寫摘要後以結束碼 1 結束。fix F10b：與 wheelrouting 一致補一項 FAIL——
    # 選點階段（Get-ScrollAreaWheelTarget）丟的 PRECONDITION 沒有對應的前提項，不補的話摘要會全 PASS、結束碼 0。
    elseif ("$_" -like 'PRECONDITION*') { $log.WriteLine("# $(Get-Ts) $_"); Write-Host "FAIL：$_" -ForegroundColor Red; $results['未在例外前完成全部驗收步驟'] = $false }
    # 小工具不在預期位置（fix F8c，Assert-OccluderResult 丟「FAIL: 」）：記 FAIL、不送滾輪，結束碼 1。
    elseif ("$_" -like 'FAIL:*') {
        $results['前提：滾輪座標落在小工具本身（WindowFromPoint 未被其他視窗遮擋）'] = $false
        $log.WriteLine("# $(Get-Ts) $_"); Write-Host "$_" -ForegroundColor Red
    }
    # 遮擋清不掉（fix F7）：環境問題，不送滾輪、不判 FAIL，以結束碼 3（ENV-BLOCKED）結束。
    elseif ("$_" -like 'ENV-BLOCKED*') { $envBlocked = "$_"; $log.WriteLine("# $(Get-Ts) $_"); Write-Host "$_" -ForegroundColor Yellow }
    else { throw }
}
finally {
    if ($hostProc -and -not $hostProc.HasExited) { Stop-Process -Id $hostProc.Id -Force -ErrorAction SilentlyContinue }
    # 被暫時最小化的使用者視窗照原 WINDOWPLACEMENT 還原（fix F7）。
    try { Restore-Occluders -Minimized $minimized -NotRestored $occNotRestored -Log { param($m) $log.WriteLine("# $(Get-Ts) $m") } } catch { $log.WriteLine("# 還原被最小化的視窗失敗：$_") }
    # $minimized 非空＝Restore-Occluders 本身中途丟例外、沒跑完；一併列為未還原（環境，不進逐項結果）。
    if ($minimized.Count -gt 0) { $occNotRestored.Add("Restore-Occluders 未跑完：尚有 $($minimized.Count) 扇未處理（見記錄）") }
    # 還原游標也經 SafeInput；鎖定（或已停止）時就放棄還原，不在登入畫面上動游標。
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
        @("# verify-4.7-wheel.ps1 summary $(Get-Ts)", "BLOCKED  $blocked", $occWarn) | ConvertTo-EvidenceText | Set-Content -Path $sumPath -Encoding utf8
    }
    # 鎖定中止：不做驗收判讀（逐項結果不完整），結束碼經共用決策（鎖定＝2）。
    exit (Get-VerdictExitCode -Locked -NotRestored $occNotRestored)
}

$sum =New-EvidenceWriter $sumPath
$sum.WriteLine("# verify-4.7-wheel.ps1 summary $(Get-Ts)")
foreach ($k in $results.Keys) { $sum.WriteLine("$(if ($results[$k]) { 'PASS' } else { 'FAIL' })  $k") }
if ($envBlocked) { $sum.WriteLine("BLOCKED  $envBlocked") }
if ($occWarn) { $sum.WriteLine($occWarn) }
$sum.Close()
Get-Content $sumPath
if ($occWarn) { Write-Host $occWarn -ForegroundColor Yellow }
# Codex task-4.7 [medium]：任一項 FAIL 以非零結束（原本只印 FAIL、仍以 0 結束）。
exit (Get-VerdictExitCode -Results $results -EnvBlocked:([bool]$envBlocked) -NotRestored $occNotRestored -RequireResults)
