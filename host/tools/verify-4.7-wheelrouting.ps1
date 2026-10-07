<#
.SYNOPSIS
    Task B7 人工項目 3 代驗腳本（關閉「捲動非使用中的視窗」系統設定後的滾輪行為）：讀出目前的
    滑鼠滾輪路由設定、暫時關閉、對真正在跑的 `fc-host` macro 小工具送真實滾輪、記錄結果，
    最後**務必**還原原本的設定值並讀回確認。

.DESCRIPTION
    對應 `.superpowers/sdd/tasks/human-checklist.md` B7 人工項目 3：
    「設定 → 藍牙與裝置 → 滑鼠 → 『捲動非使用中的視窗』關閉；游標移到小工具清單上用滾輪捲動。
    記錄結果（預期：清單不會捲動，因為小工具永遠拿不到鍵盤焦點）……測完務必把這項系統設定改
    回開啟」。

    這項系統設定對應的 Win32 API 是 `SystemParametersInfoW` 的
    `SPI_GETMOUSEWHEELROUTING`（0x201C）／`SPI_SETMOUSEWHEELROUTING`（0x201D）——兩者皆逐字
    出現在 Microsoft Learn 官方 `SystemParametersInfoW` 一手文件的完整參數表中（非僅見於社群
    文章／機碼觀察）。**呼叫慣例（2026-09-29 實測，memory spi-set-pvparam-is-value-not-pointer）**：
    GET 的 `pvParam` 指向一個 DWORD 接收讀出值；SET 的 `pvParam` **直接傳值本身**（`(IntPtr)值`），
    不是指向 DWORD 的指標——文件對 SET 的措辭抄自 GET，照文件傳指標會回 false／GetLastError=87，
    改用其他慣例亂試則會把即時值寫壞。值：0（`MOUSEWHEEL_ROUTING_FOCUS`）＝只送給有鍵盤焦點的
    視窗（對應「關閉」）、1（`MOUSEWHEEL_ROUTING_HYBRID`，文件標示為預設值）＝依應用程式類型送給
    焦點或游標下的視窗；本機原值為 2（文件未列，是「設定」App 開關「開啟」時用的值）。本腳本只
    需要「讀出目前值→設成 0→立即讀回確認（不符就中止）→做完測試→設回原本讀到的值→讀回確認」，
    不需要完整解讀所有可能的數值語意。

    步驟：
      1. `Invoke-SafeInputPreflight`（鎖定 → 結束碼 2；合成輸入不生效 → 結束碼 3）。
      2. 確認沒有既有 `fc-host`；暫存 `%APPDATA%`／`%LOCALAPPDATA%`（同 verify-4.7-wheel.ps1）。
      3. `SPI_GETMOUSEWHEELROUTING` 讀原值。
      4. `SPI_SETMOUSEWHEELROUTING` 設為 0（`MOUSEWHEEL_ROUTING_FOCUS`，對應系統設定「關閉」）。
         **設定呼叫不帶 `SPIF_UPDATEINIFILE`**（只改當次工作階段的即時值，不寫回登入設定檔／
         機碼）：萬一腳本中途被強制中斷來不及還原，使用者下次登入仍是原本的設定，不會留下
         永久性的系統改動痕跡；`finally` 一定跑還原（同一顆 API、同一組不持久化旗標）。
         「需要還原」旗標在 SET 之前就設好；SET 後立即 GET 讀回，不等於 0 就中止後續步驟。
      5. 啟動宿主、找到 macro 小工具、確認非使用中視窗＋座標未被遮擋兩個前提（同
         verify-4.7-wheel.ps1 fix round 1／2 的做法）。fix F7：座標被使用者的一般視窗蓋住時先暫時
         最小化（lib/Occluders.psm1，finally 照原位置還原）。fix F8c：按下點命中桌面、沒命中任何視窗、
         宿主另一扇視窗＝小工具不在預期位置，判 FAIL（不找替代點）；工作列／系統 UI／最小化後仍蓋住
         才在清單矩形內找替代點，整個清單矩形都找不到才判 ENV-BLOCKED（結束碼 3）。fix F10：滾輪點＝
         `.scroll-area` 中心（CDP 讀實際位置與 devicePixelRatio，`lib/ScrollAreaTarget.psm1`），另有兩項
         前提——清單可捲動、最終點 elementFromPoint 命中清單；任一不成立＝FAIL、不送滾輪（本腳本判定是
         「scrollTop 不變」，點落在清單外照樣不變，沒有這兩項前提就沒有鑑別力）。
      6. 送 6 格滾輪、比對 `scrollTop`：**預期不變**（因為小工具 `focusable(false)`、永遠拿不到
         鍵盤焦點，`MOUSEWHEEL_ROUTING_FOCUS` 下滾輪訊息只送給焦點視窗，小工具收不到）。
      7. **`finally`**：無論成功或例外，一律呼叫 `SPI_SETMOUSEWHEELROUTING` 還原成步驟 3
         讀到的原值，再 `SPI_GETMOUSEWHEELROUTING` 讀回一次確認等於原值——兩者皆寫進記錄檔
         與摘要，且都計入 PASS/FAIL（不是「盡力還原」，是「還原且驗證還原成功」才算過）。

    結果印在終端機、寫進 `host/tools/evidence/4.7-wheelrouting-summary.log`（細節見
    `4.7-wheelrouting-log.log`）。結束碼：0＝全部 PASS、1＝有 FAIL、2＝BLOCKED、
    3＝ENV-BLOCKED。

.PARAMETER Exe
    fc-host.exe 路徑，預設 host/target/release/fc-host.exe。

.PARAMETER OutDir
    證據輸出目錄，預設 host/tools/evidence。

.PARAMETER CdpPort
    WebView2 remote debugging 埠，預設 9340。

.PARAMETER SettingOnly
    只跑「讀原值 → SET → 立即讀回確認 → 還原 → 再讀回確認」這段，不啟動 fc-host、不做滾輪
    注入。用於單獨證明 SPI_SETMOUSEWHEELROUTING 的呼叫慣例正確且還原成功，不需要真的驗證
    小工具的滾輪捲動行為（那部分見 verify-4.7-wheel.ps1）。

.EXAMPLE
    工作階段確認未鎖定後：
    cd host; cargo build --release
    pwsh -File tools/verify-4.7-wheelrouting.ps1

.EXAMPLE
    只驗證系統設定呼叫慣例（不啟動宿主、不注入滾輪）：
    pwsh -File tools/verify-4.7-wheelrouting.ps1 -SettingOnly
#>
[CmdletBinding()]
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\fc-host.exe'),
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [int]$CdpPort = 9340,
    [switch]$SettingOnly
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\AutostartRegistry.psm1') -Force

# SystemParametersInfoW 不是鍵盤／滑鼠／觸控注入 API（它讀寫的是一個系統設定值，不送出任何
# 合成輸入事件），故不受「host/tools 下只有 SafeInput.psm1 可宣告注入 API」規則約束，直接在本
# 腳本宣告；游標移動／滾輪注入仍一律經 lib/SafeInput.psm1。
Add-Type -Namespace V47R -Name Native -MemberDefinition @'
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
// SPI_GETMOUSEWHEELROUTING=0x201C／SPI_SETMOUSEWHEELROUTING=0x201D。
// GET：pvParam 指向 DWORD 接收讀出值（Microsoft Learn 文件記載的慣例，本機實測有效）。
[DllImport("user32.dll", SetLastError = true)] public static extern bool SystemParametersInfoW(uint uiAction, uint uiParam, ref uint pvParam, uint fWinIni);
// SET：pvParam **直接是值本身**、不是指向 DWORD 的指標——文件對 SPI_SETMOUSEWHEELROUTING
// 的措辭抄自 GET，但 user32 實作把 pvParam 當成值傳遞（controller 2026-09-29 實測：
// SystemParametersInfoW(0x201D,0,(IntPtr)2,SPIF_SENDCHANGE) 回 true、GET 讀回 2；用
// ref uint 傳指標會回 false／GetLastError=87）。memory：spi-set-pvparam-is-value-not-pointer。
[DllImport("user32.dll", SetLastError = true, EntryPoint = "SystemParametersInfoW")] public static extern bool SystemParametersInfoSet(uint uiAction, uint uiParam, System.IntPtr pvParam, uint fWinIni);
[StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
[StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
'@

# fix F9：Per-Monitor-V2（-4），在任何座標取得之前。GetWindowRect、WindowFromPoint、GetCursorPos、
# lib/Occluders 的命中測試與 SafeInput 的游標／滾輪座標都依本執行緒的 DPI 感知解讀；pwsh 預設 unaware。
[void][V47R.Native]::SetThreadDpiAwarenessContext([IntPtr](-4))

$SPI_GETMOUSEWHEELROUTING = [uint32]0x201C
$SPI_SETMOUSEWHEELROUTING = [uint32]0x201D
$MOUSEWHEEL_ROUTING_FOCUS = [uint32]0
# SPIF_SENDCHANGE（0x02）：只廣播 WM_SETTINGCHANGE、不寫回登入設定檔／機碼。
# **絕不**加 SPIF_UPDATEINIFILE（0x01）——那會把即時值持久化到 HKCU\Control
# Panel\Desktop\MouseWheelRouting，本腳本只碰當次工作階段的即時值。
$SPIF_SENDCHANGE = [uint32]0x02

function Get-Ts { Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fffK' }
Import-Module (Join-Path $PSScriptRoot 'lib\SafeInput.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\VerifyVerdict.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\Occluders.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\ScrollAreaTarget.psm1') -Force
function Hex([IntPtr]$h) { '0x{0:X}' -f $h.ToInt64() }

function Get-MouseWheelRouting {
    $v = [uint32]0
    $ok = [V47R.Native]::SystemParametersInfoW($SPI_GETMOUSEWHEELROUTING, 0, [ref]$v, 0)
    if (-not $ok) { throw "SPI_GETMOUSEWHEELROUTING 失敗，GetLastError=$([Runtime.InteropServices.Marshal]::GetLastWin32Error())" }
    return $v
}

function Set-MouseWheelRouting([uint32]$Value) {
    # 呼叫慣例（2026-09-29 修正，見 memory spi-set-pvparam-is-value-not-pointer）：
    # SPI_SETMOUSEWHEELROUTING 的 pvParam **是值本身**，不是指向 DWORD 的指標。上一版用
    # ref uint（等同傳指標）呼叫，穩定回傳 false（GetLastError=87 ERROR_INVALID_PARAMETER），
    # 且中途改用其他未文件化的替代呼叫慣例亂試，把即時值改壞到 0、事後改不回來、還一度誤寫入
    # 機碼（已用 Set-ItemProperty 修正，機碼現況＝原值）。本函式現在只用 controller 實測有效
    # 的呼叫慣例：pvParam=(IntPtr)Value，fWinIni=SPIF_SENDCHANGE（只廣播 WM_SETTINGCHANGE、
    # **絕不**帶 SPIF_UPDATEINIFILE，不持久化到機碼）。
    $ok = [V47R.Native]::SystemParametersInfoSet($SPI_SETMOUSEWHEELROUTING, 0, [IntPtr]$Value, $SPIF_SENDCHANGE)
    if (-not $ok) { throw "SPI_SETMOUSEWHEELROUTING($Value) 失敗，GetLastError=$([Runtime.InteropServices.Marshal]::GetLastWin32Error())" }
}

function Get-TopWindows {
    $list = New-Object System.Collections.Generic.List[IntPtr]
    $h = [V47R.Native]::GetTopWindow([IntPtr]::Zero)
    while ($h -ne [IntPtr]::Zero) { $list.Add($h); $h = [V47R.Native]::GetWindow($h, 2) }
    return $list
}

function Get-WindowTitle([IntPtr]$h) {
    $sb = New-Object System.Text.StringBuilder 256
    [void][V47R.Native]::GetWindowText($h, $sb, 256)
    return $sb.ToString()
}

function Find-HostWindow([int]$ProcId, [string]$TitleExact, [int]$TimeoutSec = 20) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt $TimeoutSec) {
        foreach ($h in (Get-TopWindows)) {
            $p = 0
            [void][V47R.Native]::GetWindowThreadProcessId($h, [ref]$p)
            if ($p -eq $ProcId -and [V47R.Native]::IsWindowVisible($h) -and (Get-WindowTitle $h) -eq $TitleExact) {
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

function Find-ClearPointInRect([int]$rx, [int]$ry, [int]$rw, [int]$rh, [IntPtr]$targetHwnd) {
    for ($dy = 20; $dy -lt $rh; $dy += 20) {
        for ($dx = 20; $dx -lt $rw; $dx += 20) {
            $pt = New-Object V47R.Native+POINT; $pt.X = $rx + $dx; $pt.Y = $ry + $dy
            $hAt = [V47R.Native]::WindowFromPoint($pt)
            if ($hAt -eq [IntPtr]::Zero) { continue }
            $root = [V47R.Native]::GetAncestor($hAt, 2)  # GA_ROOT
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
# SettingOnly 模式全程不碰任何輸入注入 API（不送滾輪、不動游標），故不呼叫
# Invoke-SafeInputPreflight（它會送一次真實的 F15 按鍵探測——本模式不需要、也不該送）；
# 只用 Test-SessionLocked 做非注入式的鎖定檢查。SystemParametersInfoW 本身不是輸入注入
# API，不受鎖定狀態影響，但仍先確認未鎖定以符合全域驗收前置條件。
if ($SettingOnly) {
    if (Test-SessionLocked) {
        Write-Host 'BLOCKED：工作階段鎖定（LogonUI.exe 存在），已停止。'
        exit 2
    }
    Write-Host 'SettingOnly：工作階段未鎖定，略過 Invoke-SafeInputPreflight（本模式不注入任何輸入）'
} else {
    $pf = Invoke-SafeInputPreflight
    if ($pf.ExitCode -ne 0) { Write-Host $pf.Message; exit $pf.ExitCode }
    Write-Host $pf.Message
}
if (Get-Process -Name fc-host -ErrorAction SilentlyContinue) {
    throw '已有 fc-host 在執行，請先結束（WebView2 共用 user data folder，CDP 參數不會生效）。'
}
if (-not $SettingOnly) { $Exe = (Resolve-Path $Exe).Path }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$logPath = Join-Path $OutDir '4.7-wheelrouting-log.log'
$sumPath = Join-Path $OutDir '4.7-wheelrouting-summary.log'
$log = New-EvidenceWriter $logPath
$log.AutoFlush = $true
$log.WriteLine("# verify-4.7-wheelrouting.ps1 start=$(Get-Ts) exe=$Exe")

if ($SettingOnly) {
    $log.WriteLine("# SettingOnly 模式：只跑系統設定 SET/GET/還原，不啟動 fc-host、不做滾輪注入")
} else {
    $fixtureSrc = Join-Path $PSScriptRoot '..\ui\fixtures\tw-events.json'
    if (-not (Test-Path $fixtureSrc)) { throw "找不到 fixture：$fixtureSrc" }

    $tempRoot = Join-Path ([IO.Path]::GetTempPath()) ("fc-host-4.7r-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
    $tempAppData = Join-Path $tempRoot 'Roaming'
    $tempLocalAppData = Join-Path $tempRoot 'Local'
    New-Item -ItemType Directory -Force -Path $tempAppData, $tempLocalAppData | Out-Null
    $dataDir = Join-Path $tempLocalAppData 'tw.fintools.fc-host\data'
    New-Item -ItemType Directory -Force -Path $dataDir | Out-Null
    Copy-Item $fixtureSrc (Join-Path $dataDir 'tw_events.json') -Force
    $log.WriteLine("# APPDATA=$tempAppData（首次啟動＝macro 預設開啟）LOCALAPPDATA=$tempLocalAppData（已放入 fixture）")
}

$regSnap = @(Save-FcHostAutostartRegistry)
$log.WriteLine("# $(Get-Ts) 開機自啟登錄快照：$(($regSnap | ForEach-Object { "$($_.Name)@$(Split-Path $_.Key -Leaf) Exists=$($_.Exists)" }) -join '; ')")
$results = [ordered]@{}
$blocked = $null
$envBlocked = $null
$minimized = New-Object System.Collections.Generic.List[object]
# hung-target（review 642050f）：沒還原成功的使用者視窗（無回應、最小化未確認、讀回逾時／不符），寫進摘要。
$occNotRestored = New-Object System.Collections.Generic.List[string]
$hostProc = $null
$origCursor = New-Object V47R.Native+POINT
[void][V47R.Native]::GetCursorPos([ref]$origCursor)
$oldAppData = $env:APPDATA
$oldLocalAppData = $env:LOCALAPPDATA
$oldWv2 = $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS

# 讀原值在最外層 try 之前：即使後面任何一步失敗，只要這裡讀成功，finally 都拿得到原值可還原。
$originalRouting = Get-MouseWheelRouting
$log.WriteLine("# $(Get-Ts) 原始 SPI_GETMOUSEWHEELROUTING=$originalRouting")
$results['讀到原始滾輪路由設定值'] = $true
$routingChanged = $false
$restoredOk = $false
$restoredReadback = $null

try {
    # fix F4（review B-batch2 low；共通規則）：「需要還原」旗標在 SET 之前設好——SET 生效後、下一行
    # 之前被中斷，或 SET 回 false 但即時值其實已被改掉，finally 都要還原（重寫一次原值無害）。
    $routingChanged = $true
    Set-MouseWheelRouting $MOUSEWHEEL_ROUTING_FOCUS
    $confirmSet = Get-MouseWheelRouting
    $log.WriteLine("# $(Get-Ts) 已設定 SPI_SETMOUSEWHEELROUTING=$MOUSEWHEEL_ROUTING_FOCUS（FOCUS，對應系統設定「關閉」），讀回=$confirmSet")
    $results['成功關閉「捲動非使用中的視窗」（設為 MOUSEWHEEL_ROUTING_FOCUS）'] = ($confirmSet -eq $MOUSEWHEEL_ROUTING_FOCUS)
    # SET 讀回不符：系統處於預期之外的狀態，立即中止後續步驟（不啟動宿主、不注入），交給 finally 還原。
    if ($confirmSet -ne $MOUSEWHEEL_ROUTING_FOCUS) { throw "SET 讀回不符（預期 $MOUSEWHEEL_ROUTING_FOCUS、讀回 $confirmSet），中止後續步驟" }

    if ($SettingOnly) {
        $log.WriteLine("# $(Get-Ts) SettingOnly：跳過宿主啟動與滾輪注入")
        Write-Host 'SettingOnly：呼叫慣例＋還原驗證見下方摘要，未跑滾輪注入' -ForegroundColor Yellow
    } else {
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
    $rect = New-Object V47R.Native+RECT
    [void][V47R.Native]::GetWindowRect($macroHwnd, [ref]$rect)
    $log.WriteLine("## $(Get-Ts) macro hwnd=$(Hex $macroHwnd) rect=($($rect.Left),$($rect.Top))-($($rect.Right),$($rect.Bottom))")

    Start-Sleep -Seconds 2

    # fix F10（與 verify-4.7-wheel 一致）：滾輪點由頁面 .scroll-area 的實際位置決定
    # （lib/ScrollAreaTarget.psm1），不再用「視窗矩形 Top＋固定偏移」。本腳本的判定是「scrollTop 不變」，
    # 點落在清單外照樣會「不變」——所以前提一（清單可捲動）與前提二（最終點命中清單）是這項判定
    # 有鑑別力的必要條件，任一不成立記 FAIL、不送滾輪。
    $origin = New-Object V47R.Native+POINT
    [void][V47R.Native]::ClientToScreen($macroHwnd, [ref]$origin)
    $probe = Get-ScrollAreaProbe
    $scr = Test-ScrollAreaScrollable -Probe $probe
    $log.WriteLine("## $(Get-Ts) 前提：清單探查 origin=($($origin.X),$($origin.Y)) probe=$(if ($probe) { $probe | ConvertTo-Json -Compress } else { 'null' }) ok=$($scr.Ok) $($scr.Reason)")
    $results['前提：.scroll-area 可捲動且尺寸有效（scrollHeight > clientHeight > 0、寬高與 dpr > 0、與 viewport 有交集）'] = $scr.Ok
    if (-not $scr.Ok) { throw "PRECONDITION: $($scr.Reason)" }
    $target = Get-ScrollAreaWheelTarget -Probe $probe -OriginX $origin.X -OriginY $origin.Y
    $log.WriteLine("## $(Get-Ts) 選點：清單中心 css=($($target.CssX),$($target.CssY)) dpr=$($target.Dpr) → 螢幕 ($($target.X),$($target.Y))；清單實體矩形=($($target.Left),$($target.Top)) $($target.Width)x$($target.Height)")

    # 遮擋前提（fix F7，與 verify-4.7-wheel／7.5／7.6／5.3 一致）：滾輪座標被使用者的一般視窗蓋住時
    # 暫時最小化它（lib/Occluders.psm1，finally 照原位置還原）；仍蓋住才在清單矩形內網格搜尋，再找不到
    # ＝ENV-BLOCKED（結束碼 3），不送滾輪、不判 FAIL。必須在取前景基準之前做。
    $preferX = $target.X
    $preferY = $target.Y
    $occ = Clear-Occluders -HostPid $hostPid -TargetHwnd $macroHwnd -Points @(, @($preferX, $preferY)) -Minimized $minimized `
        -Log { param($m) $log.WriteLine("## $(Get-Ts) $m") }
    # fix F8c：判讀一律經 lib 的 Get-OccluderVerdict／Assert-OccluderResult（見 Resolve-WheelPoint）：
    # 偏好點命中桌面／沒命中／宿主另一扇＝FAIL（不找替代點）；其餘環境類才找替代點，找不到＝ENV-BLOCKED。
    $log.WriteLine("## $(Get-Ts) 前提：遮擋清除結果 prefer=($preferX,$preferY) ok=$($occ.Ok) action=$($occ.Action) $($occ.Reason) macroHwnd=$(Hex $macroHwnd)")
    $clear = Resolve-WheelPoint -Occ $occ -PreferX $preferX -PreferY $preferY -FindAlternative {
        Find-ClearPointInRect $target.Left $target.Top $target.Width $target.Height $macroHwnd
    }
    $log.WriteLine("## $(Get-Ts) 前提：滾輪座標 clear=($($clear.X),$($clear.Y))")
    $results['前提：滾輪座標落在小工具本身（WindowFromPoint 未被其他視窗遮擋）'] = $true

    # 前提二（fix F10）：最終點（含遮擋時的替代點）換回 CSS 座標，elementFromPoint 必須命中
    # .scroll-area 或其子孫，否則「scrollTop 不變」證明不了任何事，不送滾輪。
    $css = ConvertTo-CssPoint -OriginX $origin.X -OriginY $origin.Y -Dpr $target.Dpr -X $clear.X -Y $clear.Y
    $hit = Test-ScrollAreaHit -Hit (Get-ScrollAreaHitAt $css.X $css.Y)
    $log.WriteLine("## $(Get-Ts) 前提：滾輪點命中 css=($($css.X),$($css.Y)) ok=$($hit.Ok) $($hit.Reason)")
    $results['前提：滾輪點命中 .scroll-area（elementFromPoint）'] = $hit.Ok
    if (-not $hit.Ok) { throw "PRECONDITION: $($hit.Reason)" }

    $baseFg = [V47R.Native]::GetForegroundWindow()
    $baseFgPid = 0
    if ($baseFg -ne [IntPtr]::Zero) { [void][V47R.Native]::GetWindowThreadProcessId($baseFg, [ref]$baseFgPid) }
    $before = Get-ScrollTop
    $log.WriteLine("## $(Get-Ts) 送滾輪前 scrollTop=$before fg=$(Hex $baseFg) fgPid=$baseFgPid hostPid=$hostPid")
    $results['讀到初始 scrollTop（CDP 連得上）'] = ($null -ne $before)
    if ($null -eq $before) { throw '讀不到初始 scrollTop（CDP 連線或頁面選取器有問題）' }

    $pre = Test-InactiveWidgetPrecondition -FgHwnd $baseFg -FgPid ([int]$baseFgPid) -HostPid $hostPid
    $log.WriteLine("## $(Get-Ts) 非使用中前提：ok=$($pre.Ok) $($pre.Reason)")
    $results['前提：送滾輪前前景不是小工具（小工具為非使用中視窗）'] = $pre.Ok
    if (-not $pre.Ok) { throw "PRECONDITION: $($pre.Reason)" }

    $cx = $clear.X
    $cy = $clear.Y
    Send-GuardedWheel $cx $cy 6
    Start-Sleep -Milliseconds 500

    $after = Get-ScrollTop
    $log.WriteLine("## $(Get-Ts) 送滾輪後 scrollTop=$after")
    $unchanged = ($null -ne $after) -and ($null -ne $before) -and ([Math]::Abs($after - $before) -lt 0.01)
    # 符合 human-checklist.md B7 的「預期」：小工具永遠拿不到鍵盤焦點，FOCUS 路由下收不到滾輪。
    $results['符合預期：MOUSEWHEEL_ROUTING_FOCUS 下小工具收不到滾輪（scrollTop 不變）'] = $unchanged
    if ($unchanged) {
        Write-Host 'PASS：關閉「捲動非使用中的視窗」後，滾輪確實無法捲動小工具（符合預期）' -ForegroundColor Green
    } else {
        Write-Host "FAIL：預期 scrollTop 不變，實際 $before -> $after" -ForegroundColor Red
    }
    }
}
catch {
    # Codex task-4.7 [medium] 教訓同款 bug（見 verify-4.7-wheel.ps1 fix round 1）：generic 例外
    # 若不記進 $results，Get-ResultsExitCode 只看得到前面已成功的項目，會誤判整體 PASS。
    # 2026-09-29 實測踩過一次：SPI_SETMOUSEWHEELROUTING 在本機用文件記載的 pvParam byref
    # 呼叫法直接回傳 false（GetLastError=87），必須讓這裡確實記一筆 FAIL、非零結束，而不是
    # 靜默略過只印訊息。
    if ("$_" -like 'BLOCKED*') { $blocked = "$_"; $log.WriteLine("# $(Get-Ts) $_") }
    elseif ("$_" -like 'PRECONDITION*') { $log.WriteLine("# $(Get-Ts) $_"); Write-Host "FAIL：$_" -ForegroundColor Red; $results['未在例外前完成全部驗收步驟'] = $false }
    # 小工具不在預期位置（fix F8c，Assert-OccluderResult 丟「FAIL: 」）：記 FAIL、不送滾輪，結束碼 1；系統設定照常還原。
    elseif ("$_" -like 'FAIL:*') { $log.WriteLine("# $(Get-Ts) $_"); Write-Host "$_" -ForegroundColor Red; $results['未在例外前完成全部驗收步驟'] = $false }
    # 遮擋清不掉（fix F7）：環境問題，不送滾輪、不判 FAIL，以結束碼 3（ENV-BLOCKED）結束；系統設定照常還原。
    elseif ("$_" -like 'ENV-BLOCKED*') { $envBlocked = "$_"; $log.WriteLine("# $(Get-Ts) $_"); Write-Host "$_" -ForegroundColor Yellow }
    else { $log.WriteLine("# $(Get-Ts) 例外：$_"); Write-Host "FAIL（例外）：$_" -ForegroundColor Red; $results['未在例外前完成全部驗收步驟'] = $false }
}
finally {
    if ($hostProc -and -not $hostProc.HasExited) { Stop-Process -Id $hostProc.Id -Force -ErrorAction SilentlyContinue }
    # 被暫時最小化的使用者視窗照原 WINDOWPLACEMENT 還原（fix F7）。
    try { Restore-Occluders -Minimized $minimized -NotRestored $occNotRestored -Log { param($m) $log.WriteLine("# $(Get-Ts) $m") } } catch { $log.WriteLine("# 還原被最小化的視窗失敗：$_") }
    # $minimized 非空＝Restore-Occluders 本身中途丟例外、沒跑完；一併列為未還原（環境，不進逐項結果）。
    if ($minimized.Count -gt 0) { $occNotRestored.Add("Restore-Occluders 未跑完：尚有 $($minimized.Count) 扇未處理（見記錄）") }
    if (-not $SettingOnly) {
        # SettingOnly 模式全程沒有移動過游標（沒送滾輪），不需要也不該呼叫這個注入函式還原。
        try { Set-GuardedCursorPos $origCursor.X $origCursor.Y -What '還原游標' } catch { $log.WriteLine("# 還原游標略過：$_") }
    }

    # 還原系統設定：不論前面成功或失敗都要做，且要讀回確認，不是「盡力而為」。
    if ($routingChanged) {
        try {
            Set-MouseWheelRouting $originalRouting
            $restoredReadback = Get-MouseWheelRouting
            $restoredOk = ($restoredReadback -eq $originalRouting)
            $log.WriteLine("# $(Get-Ts) 還原 SPI_SETMOUSEWHEELROUTING=$originalRouting，讀回=$restoredReadback restoredOk=$restoredOk")
        } catch {
            $log.WriteLine("# $(Get-Ts) 還原系統設定失敗：$_")
        }
    } else {
        # 根本沒改過（例如一開始讀原值就失敗），視同不需還原也不算失敗。
        $restoredOk = $true
        $restoredReadback = $originalRouting
    }
    $regLeft = @(Restore-FcHostAutostartRegistry $regSnap)
    if ($regLeft.Count -gt 0) { Write-Warning "開機自啟登錄未還原：$($regLeft -join '; ')" }
    $log.WriteLine("# $(Get-Ts) 開機自啟登錄還原：未還原 $($regLeft.Count) 項$(if ($regLeft.Count) { '：' + ($regLeft -join '; ') })")
    $results['開機自啟登錄已還原（Run／StartupApproved 的 fc-host）'] = ($regLeft.Count -eq 0)
    $results['還原滾輪路由設定並讀回確認等於原值'] = $restoredOk

    $log.WriteLine("# $(Get-Ts) $(if ($SettingOnly) { 'SettingOnly 模式結束（未啟動宿主）' } else { '宿主已結束' })")
    $log.Close()
    if ($tempRoot) { Remove-Item -Recurse -Force $tempRoot -ErrorAction SilentlyContinue }
}
$occWarn = Format-UnrestoredWarning -NotRestored $occNotRestored
if ($blocked) {
    Write-Host "BLOCKED：工作階段鎖定，已停止（未重試）。$blocked"
    if ($occWarn) {
        Write-Host $occWarn -ForegroundColor Yellow
        # 中途停止沒有逐項摘要：把警示行（連同停止原因）落檔，否則只剩終端機輸出會隨 session 消失。
        @("# verify-4.7-wheelrouting.ps1 summary $(Get-Ts)", "BLOCKED  $blocked", $occWarn) | ConvertTo-EvidenceText | Set-Content -Path $sumPath -Encoding utf8
    }
    Write-Host "系統設定還原狀態：original=$originalRouting restored=$restoredReadback ok=$restoredOk"
    # 鎖定中止：不做驗收判讀（逐項結果不完整），結束碼經共用決策（鎖定＝2）。
    exit (Get-VerdictExitCode -Locked -NotRestored $occNotRestored)
}

$sum = New-EvidenceWriter $sumPath
$sum.WriteLine("# verify-4.7-wheelrouting.ps1 summary $(Get-Ts)")
$sum.WriteLine("# original=$originalRouting restored_readback=$restoredReadback restored_ok=$restoredOk")
foreach ($k in $results.Keys) { $sum.WriteLine("$(if ($results[$k]) { 'PASS' } else { 'FAIL' })  $k") }
if ($envBlocked) { $sum.WriteLine("BLOCKED  $envBlocked") }
if ($occWarn) { $sum.WriteLine($occWarn) }
$sum.Close()
Get-Content $sumPath
if ($occWarn) { Write-Host $occWarn -ForegroundColor Yellow }
exit (Get-VerdictExitCode -Results $results -EnvBlocked:([bool]$envBlocked) -NotRestored $occNotRestored -RequireResults)
