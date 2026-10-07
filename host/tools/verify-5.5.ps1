<#
.SYNOPSIS
    Task 5.5 驗收驅動腳本（自動暫停偵測：QUNS 狀態對照、鎖定、顯示器關閉、電池、省電、手動；
    design.md D12、specs/widget-host-lifecycle「自動暫停」）。

.DESCRIPTION
    只讀行程列舉／CIM 查詢／獨立 Win32 唯讀查詢（`GetSystemPowerStatus`／
    `SHQueryUserNotificationState`／`WTSQuerySessionInformationW`，三者皆為純查詢、不改動任何
    系統狀態）＋啟動一個本機 Tauri 探針視窗（`probe_fullscreen_busy`，製造真正的全螢幕狀態讓
    殼層回報 `QUNS_BUSY`，不需要獨占全螢幕 Direct3D）。**不注入任何輸入**（不送按鍵／滑鼠、
    不呼叫 `LockWorkStation`、不強制關閉顯示器）：
      - 「鎖定」只在工作階段本來就已鎖定時觀察，不主動鎖定；未鎖定時該項標 SKIP，不當作失敗，
        也不假裝驗過。
      - 「顯示器關閉」「電池」「省電模式」需要真的拔插電源／切換 Windows 設定，留給人工，見
        `.superpowers/sdd/tasks/human-checklist.md`「Task 5.5」節。

    分兩個階段各自獨立啟動宿主一次（**重要發現**，見下方「已知環境限制」）：

    **階段一（CDP 埠開啟）**：驗初始狀態主動查詢（鎖定／電池／省電／忙碌四項與獨立地面真相
    比對）、驗跑馬燈基準（scrollLeft 有在變化）、若工作階段目前真的鎖定則額外驗「暫停原因
    集合含 Locked、整體 paused=true」。

    fix F2（review 5.5 medium）：階段一另以 CDP invoke 驗收用指令 `self_test_set_manual_pause`
    （僅 `--features self-test-ipc` 建置才有；非輸入注入）驗證整條「set_pause_reason → pause
    事件 → quotes.js 停止」：手動暫停後統一記錄檔有 `Manual=true … paused=true`、跑馬燈
    scrollLeft 2 秒不變；繼續後 scrollLeft 從停止處接續（剛恢復時與停止值相差不到約 1.5 秒的
    位移量，不是歸零重來）且 2 秒內有變化。正式建置沒有這個指令時，該兩項列 PENDING（摘要印
    PENDING、不算 PASS；無 FAIL 但有 PENDING 時 exit 2）。

    開機自啟登錄：每階段啟動宿主前以 lib\AutostartRegistry.psm1 快照，該階段收尾時還原
    （暫存 %APPDATA%＝首次啟動，宿主會寫 HKCU Run\fc-host）。

    **階段二（不開 CDP，全新宿主）**：啟動 `probe_fullscreen_busy.exe`（真全螢幕視窗，覆蓋整個
    主螢幕、取得前景），驗宿主 5 秒輪詢在探針進入／離開全螢幕後正確偵測到
    `SystemBusy=true`／`SystemBusy=false`（讀 gatekeeper 記錄檔的 `EVENT
    quns-busy-changed` 一行，不透過 CDP）。

    ## 已知環境限制（本機實測，2026-09-28）：CDP remote debugging 埠會讓殼層測不到全螢幕

    第一版腳本把兩件事放在同一個宿主行程裡（CDP 開著讀跑馬燈＋同時跑全螢幕探針），結果
    `SystemBusy` 從未被偵測到；拆解後發現：只要 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=
    --remote-debugging-port=<port>` 有設定在宿主的啟動環境（不需要真的有客戶端連進去），
    `SHQueryUserNotificationState` 就測不到另一個真正全螢幕視窗的 `QUNS_BUSY`——反覆清乾淨
    環境（沒有殘留的 `fc-host.exe`／`probe_fullscreen_busy.exe`／其他本次啟動的
    `msedgewebview2.exe`）後仍穩定重現：不開 CDP 時全螢幕探針啟動後 3–5 秒內必定偵測到
    （`host/tools/evidence/5.5-*.log` 留有兩種情況各自的證據），開了 CDP 時 18 秒內從未偵測到。
    尚未查出 Windows 殼層內部確切原因（推測與 Chromium 的 remote-debugging 模式對系統其餘
    行程的排程／逾時行為有旁支影響，但未逐一排除），已知只是**本驗收方法論**的限制、不影響
    產品本身──真實使用者執行 `fc-host.exe` 從不會帶這個環境變數，見 `main.rs`／
    `tauri.conf.json` 全文搜尋確認。故本腳本改為兩階段、分開啟動宿主，不在同一個行程裡同時
    驗證「CDP 讀跑馬燈」與「真全螢幕偵測」。

    `Get-GroundTruthLocked` 的位移量也曾經算錯過一次（`WTSINFOEX_LEVEL1` 因為內含 `i64`
    欄位而整個 union 需要 8 位元組對齊，`Level`（4 bytes）之後有 4 bytes padding，`SessionFlags`
    正確 offset 是 16 不是天真推算的 12），已在函式註解記錄，避免下次重犯。

.PARAMETER Exe
    fc-host.exe 路徑，預設 host/target/release/fc-host.exe。

.PARAMETER ProbeExe
    probe_fullscreen_busy.exe 路徑，預設 host/target/release/examples/probe_fullscreen_busy.exe。

.PARAMETER OutDir
    證據輸出目錄，預設 host/tools/evidence。

.PARAMETER CdpPort
    WebView2 remote debugging 埠（僅階段一使用），預設 9339（與既有 verify-3.x/4.x/5.1/5.8
    的埠分開）。
#>
[CmdletBinding()]
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\fc-host.exe'),
    [string]$ProbeExe = (Join-Path $PSScriptRoot '..\target\release\examples\probe_fullscreen_busy.exe'),
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [int]$CdpPort = 9339
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\AutostartRegistry.psm1') -Force

Add-Type -Namespace V55 -Name Native -MemberDefinition @'
[StructLayout(LayoutKind.Sequential)]
public struct SYSTEM_POWER_STATUS {
    public byte ACLineStatus;
    public byte BatteryFlag;
    public byte BatteryLifePercent;
    public byte SystemStatusFlag;
    public int BatteryLifeTime;
    public int BatteryFullLifeTime;
}
[DllImport("kernel32.dll")] public static extern bool GetSystemPowerStatus(out SYSTEM_POWER_STATUS sps);
[DllImport("shell32.dll")] public static extern int SHQueryUserNotificationState(out int state);
[DllImport("wtsapi32.dll", SetLastError = true)]
public static extern bool WTSQuerySessionInformation(System.IntPtr hServer, int sessionId, int wtsInfoClass, out System.IntPtr ppBuffer, out uint bytesReturned);
[DllImport("wtsapi32.dll")] public static extern void WTSFreeMemory(System.IntPtr pMemory);
'@

function Get-Ts { Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fffK' }

function Get-GroundTruthOnBattery {
    $sps = New-Object V55.Native+SYSTEM_POWER_STATUS
    [void][V55.Native]::GetSystemPowerStatus([ref]$sps)
    $onBattery = if ($sps.ACLineStatus -eq 0) { $true } elseif ($sps.ACLineStatus -eq 1) { $false } else { $null }
    [PSCustomObject]@{ OnBattery = $onBattery; PowerSaver = ($sps.SystemStatusFlag -ne 0); ACLineStatus = $sps.ACLineStatus; SystemStatusFlag = $sps.SystemStatusFlag }
}
function Get-GroundTruthBusy {
    $quns = 0
    [void][V55.Native]::SHQueryUserNotificationState([ref]$quns)
    [PSCustomObject]@{ Busy = ($quns -in 2, 3, 4); Quns = $quns }  # QUNS_BUSY／D3D_FULL_SCREEN／PRESENTATION_MODE
}
# 與 `desktop::query_session_locked` 完全相同的 API（`WTSQuerySessionInformationW`＋
# `WTSSessionInfoEx`＋`SessionFlags`），本腳本獨立呼叫、不依賴宿主程式碼——`WTSSessionInfoEx`
# ＝25、`WTS_CURRENT_SESSION`＝-1（0xFFFFFFFF）。
#
# 版面 offset **曾經算錯過一次**（記錄下來避免重蹈覆轍）：原本以為 `WTSINFOEXW{ Level: u32,
# Data: union }` 的 `Data` 緊接在 `Level`（4 bytes）之後、從 offset 4 開始，`SessionFlags` 落在
# offset 12。用一次性診斷探針（同時以「手動 offset 讀位元組」與「Rust 編譯器產生的
# struct／union 存取」讀同一顆已鎖定的緩衝區比對）才發現：`WTSINFOEX_LEVEL1_W` 內含
# `LogonTime` 等 `i64` 欄位，需要 8 位元組對齊，這個對齊需求會往外傳染到整個 union
# （`WTSINFOEX_LEVEL_W`）、再傳染到外層 `WTSINFOEXW`——C／Rust 的 repr(C) 都會在 `Level`
# （4 bytes）之後插入 4 bytes padding，把 `Data` 挪到 offset 8 才開始（不是 4）。正確版面：
# `Level`（offset 0）／padding（offset 4–8）／`SessionId`（offset 8）／`SessionState`
# （offset 12）／`SessionFlags`（offset 16，`Int32`）／`WinStationName`（offset 20 起，實測值
# 確實是 `L"Console"`，佐證這個 offset 是對的）。`desktop::query_session_locked` 用 Rust
# 編譯器自動算出的 offset，從頭到尾就是對的；錯的是這支腳本手動硬編碼的 offset 12。
function Get-GroundTruthLocked {
    $buffer = [IntPtr]::Zero
    $bytesReturned = 0
    $ok = [V55.Native]::WTSQuerySessionInformation([IntPtr]::Zero, -1, 25, [ref]$buffer, [ref]$bytesReturned)
    if (-not $ok -or $buffer -eq [IntPtr]::Zero) { return $null }
    try {
        $level = [Runtime.InteropServices.Marshal]::ReadInt32($buffer, 0)
        if ($level -ne 1) { return $null }
        $flags = [Runtime.InteropServices.Marshal]::ReadInt32($buffer, 16)
        switch ($flags) {
            0 { return $true }   # WTS_SESSIONSTATE_LOCK
            1 { return $false }  # WTS_SESSIONSTATE_UNLOCK
            default { return $null }  # WTS_SESSIONSTATE_UNKNOWN(-1) 或其他未定義值
        }
    } finally {
        [V55.Native]::WTSFreeMemory($buffer)
    }
}
function Wait-FileMatch([string]$Path, [string]$Pattern, [int]$TimeoutSec) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt $TimeoutSec) {
        $content = Get-Content -Raw $Path -ErrorAction SilentlyContinue
        if ($content -and $content -match $Pattern) { return $true }
        Start-Sleep -Milliseconds 400
    }
    return $false
}
# PowerShell 把「只有一個元素的陣列」轉成布林值時，會直接取該元素本身的布林值（不是看元素
# 個數）——若 `$AcceptableValues` 剛好是單一元素 `@($false)`，`-not $AcceptableValues` 會誤判
# 成「空」。改用 `.Count`（先濾掉 `$null`，即「該項地面真相未知」）避免這個陷阱。
function Test-FieldMatches([string]$Content, [string]$Name, [array]$AcceptableValues) {
    $known = @($AcceptableValues | Where-Object { $null -ne $_ })
    if ($known.Count -eq 0) { return $null }  # 地面真相未知，不比對
    $m = [regex]::Match($Content, "STARTUP $Name=(\w+)")
    if (-not $m.Success) { return $false }
    # Rust `Display` for bool 輸出小寫 true／false；[bool]::Parse 本身不分大小寫。
    $hostValue = [bool]::Parse($m.Groups[1].Value)
    return $hostValue -in $known
}
$NodeExe = (Get-Command node -ErrorAction Stop).Source
$CdpEvalScript = Join-Path $PSScriptRoot 'host-cdp-eval.mjs'
function Read-ScrollLeft([int]$Port) {
    $out = & $NodeExe $CdpEvalScript $Port 'w=quotes' "document.querySelector('ul.tlist').scrollLeft" 2>&1
    if ($LASTEXITCODE -ne 0) { return $null }
    try { return [double]($out | ConvertFrom-Json) } catch { return $null }
}
function Invoke-QuotesEval([int]$Port, [string]$Expr) {
    $out = & $NodeExe $CdpEvalScript $Port 'w=quotes' $Expr 2>&1
    return (($out | Out-String).Trim())
}
# 2 秒內 scrollLeft 是否不變；回傳 @{ a; b; still }。
function Test-QuotesStill([int]$Port) {
    $a = Read-ScrollLeft $Port
    Start-Sleep -Seconds 2
    $b = Read-ScrollLeft $Port
    @{ a = $a; b = $b; still = ($null -ne $a) -and ($null -ne $b) -and ($a -eq $b) }
}
function New-TempFcHostEnv([string]$Prefix) {
    $root = Join-Path ([IO.Path]::GetTempPath()) ("$Prefix-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
    $appData = Join-Path $root 'Roaming'
    $localAppData = Join-Path $root 'Local'
    $dataDir = Join-Path $localAppData 'tw.fintools.fc-host\data'
    New-Item -ItemType Directory -Force -Path $appData, $dataDir | Out-Null
    Copy-Item $script:fixturePath (Join-Path $dataDir 'tw_events.json')
    [PSCustomObject]@{
        Root               = $root
        AppData            = $appData
        LocalAppData       = $localAppData
        UnifiedLogPath     = Join-Path $localAppData "tw.fintools.fc-host\logs\fc-host.$(Get-Date -Format 'yyyy-MM-dd').log"
        GatekeeperLogPath  = Join-Path $appData 'tw.fintools.fc-host\gatekeeper.log'
    }
}
function Stop-OwnedWebView2([string]$Marker) {
    Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" -ErrorAction SilentlyContinue |
        Where-Object { $_.CommandLine -and $_.CommandLine.Contains('webview-exe-name=fc-host.exe') -and $_.CommandLine.Contains($Marker) } |
        ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
}

# ── 前置檢查 ─────────────────────────────────────────────────────────────────────
if (Get-Process -Name fc-host -ErrorAction SilentlyContinue) {
    throw '已有 fc-host 在執行，請先結束（避免與本次暫存實例的行程列舉互相干擾）。'
}
$Exe = (Resolve-Path $Exe).Path
$ProbeExe = (Resolve-Path $ProbeExe).Path
$fixturePath = (Resolve-Path (Join-Path $PSScriptRoot '..\ui\fixtures\tw-events.json')).Path
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$logPath = Join-Path $OutDir '5.5-driver.log'
$sumPath = Join-Path $OutDir '5.5-summary.log'
$log = New-EvidenceWriter $logPath
$log.AutoFlush = $true
$log.WriteLine("# verify-5.5.ps1 start=$(Get-Ts) exe=$Exe")

$results = [ordered]@{}
# fix F2：沒驗到的項目另列 PENDING，不寫進 $results 當 PASS。
$pending = [ordered]@{}
$oldAppData = $env:APPDATA
$regSnap = @(Save-FcHostAutostartRegistry)
$log.WriteLine("# 開機自啟登錄快照：$(($regSnap | ForEach-Object { "$($_.Name)@$(Split-Path $_.Key -Leaf) Exists=$($_.Exists)" }) -join '; ')")
function Restore-AutostartRegistryFor([string]$Phase) {
    $left = @(Restore-FcHostAutostartRegistry $script:regSnap)
    if ($left.Count -gt 0) { Write-Warning "開機自啟登錄未還原（$Phase）：$($left -join '; ')" }
    $script:log.WriteLine("# $(Get-Ts) 開機自啟登錄還原（$Phase）：未還原 $($left.Count) 項 $($left -join '; ')")
    $script:results["開機自啟登錄已還原（$Phase，Run／StartupApproved 的 fc-host）"] = ($left.Count -eq 0)
}
$oldLocalAppData = $env:LOCALAPPDATA
$oldWv2 = $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS

# ═══ 階段一：CDP 埠開啟——初始查詢地面真相比對＋跑馬燈基準＋（若已鎖定）暫停生效 ═══════════
$hostProc = $null
try {
    $env1 = New-TempFcHostEnv 'fc-host-5.5a'
    $log.WriteLine("# 階段一 APPDATA=$($env1.AppData) LOCALAPPDATA=$($env1.LocalAppData) 統一記錄檔=$($env1.UnifiedLogPath) gatekeeper 記錄檔=$($env1.GatekeeperLogPath)")

    $groundPower = Get-GroundTruthOnBattery
    $groundBusy1 = Get-GroundTruthBusy
    $groundLocked = Get-GroundTruthLocked
    $log.WriteLine("# $(Get-Ts) 地面真相（啟動前）：ACLineStatus=$($groundPower.ACLineStatus) SystemStatusFlag=$($groundPower.SystemStatusFlag) on_battery=$($groundPower.OnBattery) power_saver=$($groundPower.PowerSaver) QUNS=$($groundBusy1.Quns) busy=$($groundBusy1.Busy) locked=$groundLocked")

    try {
        $env:APPDATA = $env1.AppData
        $env:LOCALAPPDATA = $env1.LocalAppData
        $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$CdpPort"
        $hostProc = Start-Process -FilePath $Exe -PassThru
    } finally {
        $env:APPDATA = $oldAppData
        $env:LOCALAPPDATA = $oldLocalAppData
        $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $oldWv2
    }
    $log.WriteLine("# $(Get-Ts) 階段一宿主 pid=$($hostProc.Id)")

    $startupSeen = Wait-FileMatch $env1.GatekeeperLogPath 'STARTUP busy=' 20
    $log.WriteLine("## $(Get-Ts) STARTUP 記錄出現＝$startupSeen")
    $results['啟動時已寫入 STARTUP 一行（鎖定／電池／省電／忙碌四項主動查詢）'] = $startupSeen

    if ($startupSeen) {
        $gkContent = Get-Content -Raw $env1.GatekeeperLogPath
        $obMatch = Test-FieldMatches $gkContent 'on_battery' @($groundPower.OnBattery)
        $psMatch = Test-FieldMatches $gkContent 'power_saver' @($groundPower.PowerSaver)
        $busyMatch = Test-FieldMatches $gkContent 'busy' @($groundBusy1.Busy)
        # 工作階段的鎖定狀態本身在驗收過程中實測會在同一秒內翻轉（疑似 Windows Hello 持續嘗試
        # 臉部辨識、鎖定畫面反覆進出），故再查一次「STARTUP 那行寫下之後」的地面真相，只要與
        # 兩次讀值中任一次相符就算一致——比對的是「機制是否忠實反映當下狀態」而不是「兩個獨立
        # 行程能不能讀到同一奈秒」。
        $groundLockedAfter = Get-GroundTruthLocked
        $lockedMatch = Test-FieldMatches $gkContent 'locked' @($groundLocked, $groundLockedAfter)
        $log.WriteLine("## $(Get-Ts) 初始查詢比對：on_battery match=$obMatch power_saver match=$psMatch busy match=$busyMatch locked match=$lockedMatch（地面真相 before=$groundLocked after=$groundLockedAfter）")
        if ($null -ne $obMatch) { $results['初始查詢：on_battery 與獨立地面真相一致'] = $obMatch }
        if ($null -ne $psMatch) { $results['初始查詢：power_saver 與獨立地面真相一致'] = $psMatch }
        if ($null -ne $busyMatch) { $results['初始查詢：busy 與獨立地面真相一致'] = $busyMatch }
        if ($null -ne $lockedMatch) { $results['初始查詢：locked 與獨立地面真相一致（WTSSessionInfoEx，容許啟動前後任一次讀值）'] = $lockedMatch }

        if ($groundLocked -eq $true -and $groundLockedAfter -eq $true) {
            $pauseSeen = Wait-FileMatch $env1.UnifiedLogPath '暫停原因變更：Locked=true.*paused=true' 5
            $log.WriteLine("## $(Get-Ts) 目前確實鎖定中（地面真相前後一致）：統一記錄檔含 Locked、整體 paused=true＝$pauseSeen")
            $results['地面真相確認目前鎖定中：暫停原因集合含 Locked、整體 paused=true'] = $pauseSeen
        } elseif ($groundLocked -eq $false -and $groundLockedAfter -eq $false) {
            $log.WriteLine('## 工作階段目前未鎖定（地面真相前後一致），「鎖定時應暫停」屬於 SKIP，非 FAIL——不主動呼叫 LockWorkStation；見 human-checklist.md「Task 5.5」節，鎖定 10 分鐘後解鎖仍待人工驗收')
        } else {
            $log.WriteLine("## 鎖定狀態在啟動前後兩次地面真相讀值之間不一致（before=$groundLocked after=$groundLockedAfter），或查詢失敗——'鎖定時應暫停'這條斷言本次略過，改看上面『初始查詢：locked…』那條是否吻合任一次讀值")
        }
    }

    $baseline1 = $null
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt 20 -and $null -eq $baseline1) {
        $baseline1 = Read-ScrollLeft $CdpPort
        if ($null -eq $baseline1) { Start-Sleep -Milliseconds 500 }
    }
    Start-Sleep -Seconds 2
    $baseline2 = Read-ScrollLeft $CdpPort
    $movingBeforePause = ($null -ne $baseline1) -and ($null -ne $baseline2) -and ($baseline1 -ne $baseline2)
    $log.WriteLine("## $(Get-Ts) 跑馬燈基準：scrollLeft $baseline1 → $baseline2（2 秒間隔）moving=$movingBeforePause")
    $results['暫停前：跑馬燈 scrollLeft 有在變化（基準）'] = $movingBeforePause

    # ── fix F2（review 5.5 medium）：手動暫停真的讓跑馬燈停下、繼續後從停止處接續 ─────────
    $ratePerSec = if ($movingBeforePause) { [Math]::Abs($baseline2 - $baseline1) / 2 } else { 0 }
    $pauseOut = Invoke-QuotesEval $CdpPort "window.__TAURI__.core.invoke('self_test_set_manual_pause', { active: true })"
    $log.WriteLine("## $(Get-Ts) self_test_set_manual_pause(true) → $pauseOut")
    if ($pauseOut -match 'not found|找不到|Command .* not') {
        $log.WriteLine('## 本建置沒有 self_test_set_manual_pause（非 self-test-ipc 建置），手動暫停兩項列 PENDING')
        $pending['手動暫停：跑馬燈停止（需 --features self-test-ipc 建置重跑）'] = $true
        $pending['手動繼續：跑馬燈從停止處接續（需 --features self-test-ipc 建置重跑）'] = $true
    } else {
        $manualLogged = Wait-FileMatch $env1.UnifiedLogPath '暫停原因變更：Manual=true.*paused=true' 5
        Start-Sleep -Milliseconds 500   # 讓 pause 事件送達頁面、最後一格動畫結束
        $stillP = Test-QuotesStill $CdpPort
        $log.WriteLine("## $(Get-Ts) 手動暫停：記錄檔 Manual=true paused=true＝$manualLogged；scrollLeft $($stillP.a) → $($stillP.b) still=$($stillP.still)")
        $results['手動暫停：記錄檔 Manual=true、paused=true，且跑馬燈 scrollLeft 2 秒不變'] = $movingBeforePause -and $manualLogged -and $stillP.still

        $resumeOut = Invoke-QuotesEval $CdpPort "window.__TAURI__.core.invoke('self_test_set_manual_pause', { active: false })"
        $c = Read-ScrollLeft $CdpPort
        Start-Sleep -Seconds 2
        $d = Read-ScrollLeft $CdpPort
        $tolerance = [Math]::Max(5, $ratePerSec * 1.5)
        $continued = ($null -ne $c) -and ($null -ne $stillP.b) -and ([Math]::Abs($c - $stillP.b) -le $tolerance)
        $movingAgain = ($null -ne $c) -and ($null -ne $d) -and ($c -ne $d)
        $log.WriteLine("## $(Get-Ts) 手動繼續 → $resumeOut；停止值=$($stillP.b) 恢復當下=$c（容許差 $tolerance）2 秒後=$d continued=$continued moving=$movingAgain")
        $results['手動繼續：跑馬燈從停止處接續（未歸零）且恢復移動'] = $continued -and $movingAgain
    }
}
finally {
    if ($hostProc -and -not $hostProc.HasExited) { Stop-Process -Id $hostProc.Id -Force -ErrorAction SilentlyContinue }
    Stop-OwnedWebView2 "--remote-debugging-port=$CdpPort"
    Restore-AutostartRegistryFor '階段一'
    try { Copy-EvidenceFile -Source $env1.UnifiedLogPath -Destination (Join-Path $OutDir '5.5-unified.log') } catch { Write-Warning "複製統一記錄檔失敗：$_" }
    try { Copy-EvidenceFile -Source $env1.GatekeeperLogPath -Destination (Join-Path $OutDir '5.5-gatekeeper-phase1.log') } catch { Write-Warning "複製 gatekeeper.log 失敗：$_" }
    if (Test-Path $env1.Root) { Remove-Item -Recurse -Force $env1.Root -ErrorAction SilentlyContinue }
    $log.WriteLine("# $(Get-Ts) 階段一收尾完成")
}

# ═══ 階段二：不開 CDP，全新宿主——真全螢幕探針觸發 SystemBusy（design.md「每 5 秒輪詢」）═══
$hostProc2 = $null
$probeProc = $null
try {
    $env2 = New-TempFcHostEnv 'fc-host-5.5b'
    $log.WriteLine("# 階段二 APPDATA=$($env2.AppData) LOCALAPPDATA=$($env2.LocalAppData) gatekeeper 記錄檔=$($env2.GatekeeperLogPath)（本階段刻意不開 CDP，見腳本文件「已知環境限制」）")

    try {
        $env:APPDATA = $env2.AppData
        $env:LOCALAPPDATA = $env2.LocalAppData
        $hostProc2 = Start-Process -FilePath $Exe -PassThru
    } finally {
        $env:APPDATA = $oldAppData
        $env:LOCALAPPDATA = $oldLocalAppData
    }
    $log.WriteLine("# $(Get-Ts) 階段二宿主 pid=$($hostProc2.Id)")
    $startupSeen2 = Wait-FileMatch $env2.GatekeeperLogPath 'STARTUP busy=' 20
    $log.WriteLine("## $(Get-Ts) 階段二 STARTUP 記錄出現＝$startupSeen2")

    if ($startupSeen2) {
        $probeProc = Start-Process -FilePath $ProbeExe -ArgumentList '--duration-secs', '20' -PassThru
        $log.WriteLine("## $(Get-Ts) 已啟動全螢幕探針 pid=$($probeProc.Id)")

        $busySeen = Wait-FileMatch $env2.GatekeeperLogPath 'EVENT quns-busy-changed busy=true' 15
        $log.WriteLine("## $(Get-Ts) SystemBusy=true 出現＝$busySeen")
        $results['全螢幕探針啟動後 15 秒內：SystemBusy 變化事件出現（busy=true，design.md「每 5 秒輪詢」）'] = $busySeen

        $probeProc.WaitForExit(25000) | Out-Null
        if (-not $probeProc.HasExited) { Stop-Process -Id $probeProc.Id -Force -ErrorAction SilentlyContinue }
        $log.WriteLine("## $(Get-Ts) 探針視窗已結束")

        $resumedSeen = Wait-FileMatch $env2.GatekeeperLogPath 'EVENT quns-busy-changed busy=false' 15
        $log.WriteLine("## $(Get-Ts) SystemBusy=false 出現＝$resumedSeen")
        $results['探針結束後 15 秒內：SystemBusy 變化事件出現（busy=false）'] = $resumedSeen
    } else {
        $log.WriteLine('## 階段二宿主未在時限內就緒，全螢幕探針步驟略過')
    }
}
finally {
    if ($probeProc -and -not $probeProc.HasExited) { Stop-Process -Id $probeProc.Id -Force -ErrorAction SilentlyContinue }
    if ($hostProc2 -and -not $hostProc2.HasExited) { Stop-Process -Id $hostProc2.Id -Force -ErrorAction SilentlyContinue }
    try { Copy-EvidenceFile -Source $env2.GatekeeperLogPath -Destination (Join-Path $OutDir '5.5-gatekeeper-phase2.log') } catch { Write-Warning "複製 gatekeeper.log 失敗：$_" }
    if (Test-Path $env2.Root) { Remove-Item -Recurse -Force $env2.Root -ErrorAction SilentlyContinue }
    Restore-AutostartRegistryFor '階段二'
    $log.WriteLine("# $(Get-Ts) 階段二收尾完成")
    $log.Close()
}

$sum = New-EvidenceWriter $sumPath
$sum.WriteLine("# verify-5.5.ps1 summary $(Get-Ts)")
foreach ($k in $results.Keys) { $sum.WriteLine("$(if ($results[$k]) { 'PASS' } else { 'FAIL' })  $k") }
foreach ($k in $pending.Keys) { $sum.WriteLine("PENDING  $k") }
$sum.Close()
Get-Content $sumPath

if (@($results.Values | Where-Object { -not $_ }).Count -gt 0) { exit 1 }
if ($pending.Count -gt 0) { exit 2 }
exit 0
