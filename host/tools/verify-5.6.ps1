<#
.SYNOPSIS
    Task 5.6 驗收驅動腳本：WebView2 故障復原（design.md D12「WebView2 故障」條；
    specs/widget-host-lifecycle「網頁引擎故障復原」）。

.DESCRIPTION
    **不注入任何輸入**（不送按鍵、不點擊、不截圖），鎖定時也可跑。只終止**本次啟動的宿主**
    所生出的 WebView2 行程：目標必須同時滿足
      (1) 在「根＝本次 fc-host PID」的行程樹內（Win32_Process.ParentProcessId 逐層往上）；
      (2) 命令列含 `webview-exe-name=fc-host.exe`；
      (3) 命令列含本次專屬的 `--remote-debugging-port=<CdpPort>`；
    並以 `Stop-Process -Id <PID>` 精確終止。**絕不使用 `taskkill /IM msedgewebview2.exe`**
    或任何不帶 PID 的全域終止。（WebView2 的 user data folder 不吃 %LOCALAPPDATA% 覆寫，見
    verify-5.8.ps1 步驟 6 的發現，故不以 UDF 路徑比對。）

    步驟：
      0. 前置：確認沒有 fc-host 在跑；暫存 %APPDATA%／%LOCALAPPDATA%（首次啟動＝五個財經
         小工具；資料目錄放一份 D:\finance-calendar\tw_events.json 的複本，只讀來源）。
         以 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=<CdpPort>` 啟動。
      1. 等五個小工具頁面就緒（CDP 讀 DOM：#widget-root 有內容、無佔位訊息）。
      R. 終止一個本宿主的 renderer（`--type=renderer`）→ 10 秒內所有小工具 ok，且至少一個
         小工具的 `performance.timeOrigin` 改變（＝被 Reload 的那個）；記錄檔有
         `kind=RENDER_PROCESS_EXITED` 與「已重新載入」。若終止的是沒有服務任何頁面的備用
         renderer（沒有小工具受影響），換下一個再試（最多 3 個）。
      S. 以第二個執行個體（手動重複啟動＝single-instance 開設定視窗，非輸入注入）開啟設定
         視窗，等它就緒。
      B. 終止本宿主的 browser process（父行程＝fc-host、命令列無 `--type=`）→ 10 秒內五個
         小工具都換成新頁面（timeOrigin 全部不在終止前的集合內）且 ok、小工具頁面數恰為 5；
         設定視窗也重新開啟就緒；記錄檔有 `kind=BROWSER_PROCESS_EXITED`、只觸發一次全部重建；
         新視窗樣式（WS_EX_TOOLWINDOW、WS_EX_NOACTIVATE、無 WS_EX_APPWINDOW／TOPMOST）。
      D. 重建後資料推播：改寫資料目錄的 tw_events.json（updated 改成標記值），40 秒內 macro
         小工具的頁面文字出現標記（新視窗已重新 subscribe_data）。
      H. 卡死：以 CDP 在 fixed 小工具頁面注入 JS 無窮迴圈 → 看門狗（5 秒×連續 3 次）判定
         卡死並重建，60 秒內 fixed 換成新頁面且 ok；記錄檔有 `kind=WATCHDOG_TIMEOUT`。
         宿主自己判定為暫停中（記錄檔最後一行 `paused=true`）時改驗「看門狗不計時」，重建
         本身標 PENDING（摘要印 PENDING、不算 PASS；無 FAIL 但有 PENDING 時 exit 2）。
      P. （task 5.6 fix round 1）暫停中復原仍保持暫停：以驗收用指令
         `self_test_set_manual_pause`（僅 `--features self-test-ipc` 建置才有，CDP invoke，非輸入
         注入）進入手動暫停 → 行情條 scrollLeft 2 秒不變；逐一終止本宿主 renderer 直到 quotes
         被重新載入 → 仍不變、`get_pause` 回報 paused；終止 browser → 重建後的 quotes 仍不變；
         最後解除暫停 → scrollLeft 恢復變化（證明前面的「不變」不是跑馬燈本來就不動）。
         正式版沒有這個驗收用指令時，P 項比照 verify-5.5 列 PENDING（fix F7）；指令存在卻失敗才
         記 FAIL。
      E. 結束宿主，清掉本次行程樹內殘留的 WebView2，確認無殘留。

.PARAMETER Exe
    fc-host.exe 路徑，預設 host/target/release/fc-host.exe。P 步驟需要驗收用指令
    `self_test_set_manual_pause`，須以 `cargo build --release --features self-test-ipc` 建置
    （改過 host/ui/ 時先 `cargo clean --release -p fc-host`）。

.PARAMETER OutDir
    證據輸出目錄，預設 host/tools/evidence。

.PARAMETER CdpPort
    本次專屬的 WebView2 remote debugging 埠（預設 9356，未被其他驗收腳本使用）。
#>
[CmdletBinding()]
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\fc-host.exe'),
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [int]$CdpPort = 9356
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\AutostartRegistry.psm1') -Force

Add-Type -Namespace V56 -Name Native -MemberDefinition @'
[DllImport("user32.dll")] public static extern System.IntPtr GetTopWindow(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern System.IntPtr GetWindow(System.IntPtr hWnd, uint cmd);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr hWnd, out uint pid);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(System.IntPtr hWnd, System.Text.StringBuilder s, int n);
[DllImport("user32.dll", EntryPoint = "GetWindowLongPtrW")] public static extern System.IntPtr GetWindowLongPtr(System.IntPtr hWnd, int idx);
'@

$WidgetIds = @('clock', 'macro', 'fixed', 'dynamic', 'quotes')
$Node = (Get-Command node).Source
$CdpScript = Join-Path $PSScriptRoot 'cdp-widgets.mjs'
$CdpEval = Join-Path $PSScriptRoot 'host-cdp-eval.mjs'
$PortMarker = "--remote-debugging-port=$CdpPort"
$ExeMarker = 'webview-exe-name=fc-host.exe'

function Get-Ts { Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fffK' }
function Test-Locked { [bool](Get-Process -Name LogonUI, LockApp -ErrorAction SilentlyContinue) }

$Exe = (Resolve-Path $Exe).Path
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$logPath = Join-Path $OutDir '5.6-driver.log'
$sumPath = Join-Path $OutDir '5.6-summary.log'
$log = New-EvidenceWriter $logPath
$log.AutoFlush = $true
function W([string]$msg) { $log.WriteLine("$(Get-Ts) $msg") }

# ── 行程識別（本宿主行程樹＋命令列標記，見檔頭） ─────────────────────────────────────
function Get-HostTree([int]$RootPid) {
    $all = @(Get-CimInstance Win32_Process)
    $children = @{}
    foreach ($p in $all) {
        $key = [int]$p.ParentProcessId
        if (-not $children.ContainsKey($key)) { $children[$key] = New-Object System.Collections.ArrayList }
        [void]$children[$key].Add($p)
    }
    $result = New-Object System.Collections.ArrayList
    $queue = New-Object System.Collections.Queue
    $queue.Enqueue($RootPid)
    $seen = @{}
    while ($queue.Count -gt 0) {
        $cur = [int]$queue.Dequeue()
        if ($seen.ContainsKey($cur)) { continue }
        $seen[$cur] = $true
        if ($children.ContainsKey($cur)) {
            foreach ($c in $children[$cur]) {
                [void]$result.Add($c)
                $queue.Enqueue([int]$c.ProcessId)
            }
        }
    }
    return $result
}

function Get-OurWv2([int]$HostPid) {
    @(Get-HostTree $HostPid | Where-Object {
        $_.Name -eq 'msedgewebview2.exe' -and $_.CommandLine -and
        $_.CommandLine.Contains($ExeMarker) -and $_.CommandLine.Contains($PortMarker)
    })
}

function Get-Wv2Type($p) {
    if ($p.CommandLine -match '--type=([a-z-]+)') { return $Matches[1] }
    return 'browser'
}

function Write-Tree([string]$tag, [int]$HostPid) {
    $procs = Get-OurWv2 $HostPid
    $desc = ($procs | ForEach-Object {
        $t = Get-Wv2Type $_
        if ($t -eq 'utility' -and $_.CommandLine -match '--utility-sub-type=([^ ]+)') { $t = "utility($($Matches[1]))" }
        "$($_.ProcessId)/$t/ppid=$($_.ParentProcessId)"
    }) -join ' '
    W "TREE[$tag] count=$($procs.Count) $desc"
}

function Stop-Ours([int]$HostPid, [int]$TargetPid, [string]$why) {
    $procs = Get-OurWv2 $HostPid
    $target = $procs | Where-Object { [int]$_.ProcessId -eq $TargetPid }
    if (-not $target) {
        W "REFUSE 終止 pid=$TargetPid（$why）：不在本宿主行程樹內或命令列標記不符"
        return $false
    }
    W "KILL pid=$TargetPid type=$(Get-Wv2Type $target) ppid=$($target.ParentProcessId)（$why）"
    Stop-Process -Id $TargetPid -Force
    return $true
}

# ── CDP 狀態 ───────────────────────────────────────────────────────────────────
function Get-PageState([string]$Marker = '') {
    $nodeArgs = @($CdpScript, "$CdpPort", 'state')
    if ($Marker) { $nodeArgs += $Marker }
    $raw = & $Node @nodeArgs 2>$null
    if (-not $raw) { return @() }
    $parsed = $raw | ConvertFrom-Json
    if ($parsed -is [array]) { return $parsed }
    if ($parsed.PSObject.Properties.Name -contains 'error') { return @() }
    return @($parsed)
}

function Format-State($rows) {
    ($rows | Sort-Object kind, id | ForEach-Object {
        "$($_.id)[ok=$($_.ok) len=$($_.len) t0=$([math]::Round([double]($_.timeOrigin), 1)) tid=$($_.targetId.Substring(0, 8))$(if ($_.err) { " err=$($_.err)" })]"
    }) -join ' '
}

# 輪詢直到 $Cond（收到 rows）為真，回傳 @{ ok; elapsedMs; rows }。
function Wait-State([scriptblock]$Cond, [int]$TimeoutSec, [string]$Marker = '') {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $rows = @()
    while ($sw.Elapsed.TotalSeconds -lt $TimeoutSec) {
        $rows = @(Get-PageState $Marker)
        if ($rows.Count -gt 0 -and (& $Cond $rows)) {
            return @{ ok = $true; elapsedMs = [int]$sw.Elapsed.TotalMilliseconds; rows = $rows }
        }
        Start-Sleep -Milliseconds 200
    }
    return @{ ok = $false; elapsedMs = [int]$sw.Elapsed.TotalMilliseconds; rows = $rows }
}

function Get-WidgetRows($rows) { @($rows | Where-Object { $_.kind -eq 'widget' }) }

# ── 視窗樣式 ───────────────────────────────────────────────────────────────────
function Get-WidgetWindows([int]$ProcId) {
    $list = @()
    $h = [V56.Native]::GetTopWindow([IntPtr]::Zero)
    while ($h -ne [IntPtr]::Zero) {
        $p = 0
        [void][V56.Native]::GetWindowThreadProcessId($h, [ref]$p)
        if ($p -eq $ProcId) {
            $sb = New-Object System.Text.StringBuilder 256
            [void][V56.Native]::GetWindowText($h, $sb, 256)
            $title = $sb.ToString()
            if ($title -like 'fc-host *') {
                $ex = [V56.Native]::GetWindowLongPtr($h, -20).ToInt64()
                $list += [pscustomobject]@{
                    hwnd = ('0x{0:X}' -f $h.ToInt64()); title = $title
                    visible = [V56.Native]::IsWindowVisible($h)
                    tool = [bool]($ex -band 0x80); noactivate = [bool]($ex -band 0x08000000)
                    appwindow = [bool]($ex -band 0x40000); topmost = [bool]($ex -band 0x8)
                }
            }
        }
        $h = [V56.Native]::GetWindow($h, 2)
    }
    return $list
}

function Format-Windows($wins) {
    ($wins | ForEach-Object {
        "$($_.title)@$($_.hwnd)[vis=$([int]$_.visible) tool=$([int]$_.tool) noact=$([int]$_.noactivate) app=$([int]$_.appwindow) top=$([int]$_.topmost)]"
    }) -join ' '
}

function Test-WidgetStyles($wins) {
    $w = @($wins | Where-Object { $_.visible })
    ($w.Count -ge 1) -and -not ($w | Where-Object { -not $_.tool -or -not $_.noactivate -or $_.appwindow -or $_.topmost })
}

function Invoke-Page([string]$UrlPart, [string]$Expr) {
    $out = & $Node $CdpEval "$CdpPort" $UrlPart $Expr 2>&1
    if ($LASTEXITCODE -ne 0) { return $null }
    try { return ($out | ConvertFrom-Json) } catch { return $null }
}
# 同 Invoke-Page，但保留結束碼與原始輸出（判斷「指令不存在」需要錯誤訊息）。
function Invoke-PageRaw([string]$UrlPart, [string]$Expr) {
    $out = & $Node $CdpEval "$CdpPort" $UrlPart $Expr 2>&1
    [PSCustomObject]@{ ExitCode = $LASTEXITCODE; Text = (($out | Out-String).Trim()) }
}
# fix F7（批次 A）：self_test_set_manual_pause 的結果分三類——ok（回 true）、missing（正式版沒有這個
# 驗收用指令：比照 verify-5.5 列 PENDING）、failed（指令存在卻失敗，或 CDP 等其他錯誤：照 FAIL）。
# fix F7b：missing 只認 Tauri 2.12 的精確形式（tauri-2.12.0/src/webview/mod.rs：
# `resolver.reject(format!("Command {command} not found"))`，且必須是這個指令名）。其他「找不到／not
# found」——例如 host-cdp-eval.mjs 的「找不到 url 含…的分頁」、window／webview not found——都是真失敗。
function Get-ManualPauseOutcome([int]$ExitCode, [string]$Text) {
    $t = "$Text".Trim()
    if ($ExitCode -eq 0 -and $t -eq 'true') { return 'ok' }
    if ($t -cmatch 'Command\s+self_test_set_manual_pause\s+not found') { return 'missing' }
    return 'failed'
}
function Read-QuotesScroll { Invoke-Page 'w=quotes' "document.querySelector('ul.tlist') ? document.querySelector('ul.tlist').scrollLeft : null" }
# 2 秒內 scrollLeft 是否不變（暫停中應不變）；回傳 @{ a; b; still }。
function Test-QuotesStill {
    $a = Read-QuotesScroll
    Start-Sleep -Seconds 2
    $b = Read-QuotesScroll
    @{ a = $a; b = $b; still = ($null -ne $a) -and ($null -ne $b) -and ($a -eq $b) }
}
function Get-QuotesT0 { $q = @(Get-WidgetRows (Get-PageState) | Where-Object { $_.id -eq 'quotes' -and $_.ok }); if ($q.Count -eq 1) { $q[0].timeOrigin } else { $null } }

function Read-Unified { if (Test-Path $unifiedLogPath) { Get-Content -Raw $unifiedLogPath -Encoding UTF8 } else { '' } }
function Count-Lines([string]$pattern) { @((Read-Unified) -split "`n" | Where-Object { $_ -match $pattern }).Count }

# ── 0. 前置 ─────────────────────────────────────────────────────────────────────
if (Get-Process -Name fc-host -ErrorAction SilentlyContinue) {
    throw '已有 fc-host 在執行，請先結束（避免行程樹與 CDP 埠互相干擾）。'
}
$locked = Test-Locked
W "# verify-5.6.ps1 exe=$Exe cdpPort=$CdpPort locked=$([int]$locked)"

$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ("fc-host-5.6-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
$tempAppData = Join-Path $tempRoot 'Roaming'
$tempLocalAppData = Join-Path $tempRoot 'Local'
$dataDir = Join-Path $tempLocalAppData 'tw.fintools.fc-host\data'
$logDir = Join-Path $tempLocalAppData 'tw.fintools.fc-host\logs'
New-Item -ItemType Directory -Force -Path $tempAppData, $dataDir | Out-Null
$srcData = 'D:\finance-calendar\tw_events.json'
if (-not (Test-Path $srcData)) { $srcData = Join-Path $PSScriptRoot '..\ui\fixtures\tw-events.json' }
Copy-Item $srcData (Join-Path $dataDir 'tw_events.json')
$unifiedLogPath = Join-Path $logDir "fc-host.$(Get-Date -Format 'yyyy-MM-dd').log"
W "# APPDATA=$tempAppData LOCALAPPDATA=$tempLocalAppData data=$srcData（複本）log=$unifiedLogPath"

$regSnap = @(Save-FcHostAutostartRegistry)
W "開機自啟登錄快照：$(($regSnap | ForEach-Object { "$($_.Name)@$(Split-Path $_.Key -Leaf) Exists=$($_.Exists)" }) -join '; ')"
$results = [ordered]@{}
# fix F2（review 5.6 low）：沒驗到的項目另列 PENDING，不得寫進 $results 當 PASS；有 PENDING 且無
# FAIL 時 exit 2（不是 0），只看 exit code 的人也不會誤判已通過。
$pending = [ordered]@{}
$hostProc = $null
$hostPid = 0
$knownWv2 = @{}
$oldAppData = $env:APPDATA; $oldLocal = $env:LOCALAPPDATA; $oldWv2 = $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS

function Remember-Wv2 { foreach ($p in (Get-OurWv2 $hostPid)) { $knownWv2[[int]$p.ProcessId] = $p.CreationDate } }

try {
    try {
        $env:APPDATA = $tempAppData
        $env:LOCALAPPDATA = $tempLocalAppData
        $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $PortMarker
        $hostProc = Start-Process -FilePath $Exe -PassThru
    } finally {
        $env:APPDATA = $oldAppData; $env:LOCALAPPDATA = $oldLocal; $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $oldWv2
    }
    $hostPid = $hostProc.Id
    W "# 宿主 pid=$hostPid"

    # ── 1. 初始就緒 ────────────────────────────────────────────────────────────
    $allOk = {
        param($rows)
        $w = Get-WidgetRows $rows
        -not ($WidgetIds | Where-Object { $id = $_; -not ($w | Where-Object { $_.id -eq $id -and $_.ok }) })
    }
    $r = Wait-State $allOk 60
    W "INIT ready=$($r.ok) elapsedMs=$($r.elapsedMs) $(Format-State $r.rows)"
    $results['1 五個小工具初始就緒'] = $r.ok
    Start-Sleep -Seconds 2
    Write-Tree 'init' $hostPid
    Remember-Wv2
    $winsInit = Get-WidgetWindows $hostPid
    W "WINDOWS[init] $(Format-Windows $winsInit)"

    # ── R. renderer 終止 ────────────────────────────────────────────────────────
    $rPassed = $false
    $tried = @()
    for ($attempt = 1; $attempt -le 3 -and -not $rPassed; $attempt++) {
        $before = Get-WidgetRows (Get-PageState)
        $beforeT0 = @{}; foreach ($x in $before) { $beforeT0[$x.id] = $x.timeOrigin }
        $renderers = @(Get-OurWv2 $hostPid | Where-Object { (Get-Wv2Type $_) -eq 'renderer' -and $tried -notcontains [int]$_.ProcessId })
        if ($renderers.Count -eq 0) { W 'R 找不到可終止的 renderer'; break }
        $victim = [int]$renderers[0].ProcessId
        $tried += $victim
        $reloadBefore = Count-Lines '已重新載入'
        if (-not (Stop-Ours $hostPid $victim "R attempt=$attempt")) { break }
        $cond = {
            param($rows)
            $w = Get-WidgetRows $rows
            $allHealthy = -not ($WidgetIds | Where-Object { $id = $_; -not ($w | Where-Object { $_.id -eq $id -and $_.ok }) })
            $changed = @($w | Where-Object { $_.ok -and $beforeT0.ContainsKey($_.id) -and $_.timeOrigin -ne $beforeT0[$_.id] })
            $allHealthy -and $changed.Count -ge 1
        }
        $r = Wait-State $cond 10
        $affected = @(Get-WidgetRows $r.rows | Where-Object { $beforeT0.ContainsKey($_.id) -and $_.timeOrigin -ne $beforeT0[$_.id] } | ForEach-Object { $_.id })
        W "R attempt=$attempt recovered=$($r.ok) elapsedMs=$($r.elapsedMs) affected=[$($affected -join ',')] $(Format-State $r.rows)"
        if ($r.ok) {
            Start-Sleep -Milliseconds 500
            $pfSeen = (Count-Lines 'WebView2 故障 .*kind=RENDER_PROCESS_EXITED') -ge 1
            $reloadSeen = (Count-Lines '已重新載入') -gt $reloadBefore
            W "R log RENDER_PROCESS_EXITED=$pfSeen reloadLine=$reloadSeen"
            $rPassed = $pfSeen -and $reloadSeen
        } else {
            W "R attempt=$attempt 無小工具受影響（可能是備用 renderer），換下一個"
            Start-Sleep -Seconds 1
        }
    }
    $results['R renderer 終止：所有小工具 10 秒內恢復，記錄檔有 RENDER_PROCESS_EXITED 與重新載入'] = $rPassed
    Write-Tree 'after-R' $hostPid
    Remember-Wv2

    # ── S. 開設定視窗（第二個執行個體＝手動重複啟動） ───────────────────────────────
    $env:APPDATA = $tempAppData; $env:LOCALAPPDATA = $tempLocalAppData
    try { $second = Start-Process -FilePath $Exe -PassThru } finally { $env:APPDATA = $oldAppData; $env:LOCALAPPDATA = $oldLocal }
    [void]$second.WaitForExit(15000)
    $r = Wait-State { param($rows) [bool]($rows | Where-Object { $_.kind -eq 'settings' -and $_.ok }) } 30
    W "S settings ready=$($r.ok) elapsedMs=$($r.elapsedMs) secondExited=$($second.HasExited)"
    $results['S 設定視窗開啟（供 B 驗證一併重開）'] = $r.ok
    $settingsBefore = @($r.rows | Where-Object { $_.kind -eq 'settings' } | ForEach-Object { $_.timeOrigin })

    # ── B. browser process 終止 ─────────────────────────────────────────────────
    $before = Get-PageState
    $beforeT0 = @($before | ForEach-Object { $_.timeOrigin })
    $winsBefore = Get-WidgetWindows $hostPid
    W "B before $(Format-State $before)"
    W "WINDOWS[before-B] $(Format-Windows $winsBefore)"
    $browser = @(Get-OurWv2 $hostPid | Where-Object { (Get-Wv2Type $_) -eq 'browser' -and [int]$_.ParentProcessId -eq $hostPid })
    W "B browser candidates=$($browser.Count) pids=$(($browser | ForEach-Object { $_.ProcessId }) -join ',')"
    $bPassed = $false
    if ($browser.Count -eq 1 -and (Stop-Ours $hostPid ([int]$browser[0].ProcessId) 'B')) {
        $cond = {
            param($rows)
            $w = Get-WidgetRows $rows
            $fresh = @($w | Where-Object { $_.ok -and $beforeT0 -notcontains $_.timeOrigin })
            ($w.Count -eq $WidgetIds.Count) -and -not ($WidgetIds | Where-Object { $id = $_; -not ($fresh | Where-Object { $_.id -eq $id }) })
        }
        $r = Wait-State $cond 10
        W "B widgets recovered=$($r.ok) elapsedMs=$($r.elapsedMs) $(Format-State $r.rows)"
        $rs = Wait-State { param($rows) [bool]($rows | Where-Object { $_.kind -eq 'settings' -and $_.ok -and $settingsBefore -notcontains $_.timeOrigin }) } 15
        W "B settings reopened=$($rs.ok) elapsedMs(after widgets)=$($rs.elapsedMs)"
        Start-Sleep -Seconds 2
        $winsAfter = Get-WidgetWindows $hostPid
        W "WINDOWS[after-B] $(Format-Windows $winsAfter)"
        $oldHwnds = @($winsBefore | ForEach-Object { $_.hwnd })
        $newVisible = @($winsAfter | Where-Object { $_.visible })
        $noOld = -not ($winsAfter | Where-Object { $oldHwnds -contains $_.hwnd })
        $styleOk = Test-WidgetStyles $winsAfter
        $pfCount = Count-Lines 'WebView2 故障 .*kind=BROWSER_PROCESS_EXITED'
        $rebuildAll = Count-Lines '重建全部小工具視窗'
        $dupSkipped = Count-Lines '已在處理，略過'
        W "B windows total=$($winsAfter.Count) visible=$($newVisible.Count) noOldHwnd=$noOld styleOk=$styleOk log BROWSER_PROCESS_EXITED=$pfCount rebuildAll=$rebuildAll dupSkipped=$dupSkipped"
        $results['B browser 終止：五個小工具 10 秒內重建並顯示內容'] = $r.ok
        $results['B 設定視窗一併重開'] = $rs.ok
        $results['B 新視窗恰 5 個、舊 HWND 已移除、樣式（TOOLWINDOW／NOACTIVATE／非 APPWINDOW／非 TOPMOST）'] = ($winsAfter.Count -eq 5) -and $noOld -and $styleOk
        $results['B 記錄檔：BROWSER_PROCESS_EXITED 有記錄、全部重建只觸發一次'] = ($pfCount -ge 1) -and ($rebuildAll -eq 1)
        $subclassFail = Count-Lines '子類別化失敗|子類別化無法排進'
        $results['B 重建視窗 Win+D 子類別化成功（記錄檔無子類別化失敗）'] = ($subclassFail -eq 0)
        $bPassed = $r.ok
    } else {
        $results['B browser 終止：五個小工具 10 秒內重建並顯示內容'] = $false
    }
    Write-Tree 'after-B' $hostPid
    Remember-Wv2

    # ── D. 重建後資料推播 ───────────────────────────────────────────────────────
    $marker = '2099-12-31 23:59'
    $dataFile = Join-Path $dataDir 'tw_events.json'
    $text = [IO.File]::ReadAllText($dataFile)
    $text = [regex]::Replace($text, '"updated":\s*"[^"]*"', "`"updated`": `"$marker`"")
    [IO.File]::WriteAllText($dataFile, $text, (New-Object System.Text.UTF8Encoding($false)))
    W "D 已改寫 updated=$marker"
    $r = Wait-State { param($rows) [bool]($rows | Where-Object { $_.kind -eq 'widget' -and $_.id -eq 'macro' -and $_.hasMarker }) } 40 $marker
    W "D macro 收到推播=$($r.ok) elapsedMs=$($r.elapsedMs)"
    $results['D 重建後的 macro 小工具 40 秒內收到資料推播（重新 subscribe_data）'] = $r.ok

    # ── H. 卡死 → 看門狗重建 ────────────────────────────────────────────────────
    # 注入 JS 無窮迴圈是 CDP 對本宿主頁面的腳本評估，不是輸入注入，鎖定時也可做。看門狗依
    # controller 裁決只在「宿主的暫停原因集合為空」時計時，故以宿主自己記錄的最後一次
    # `paused=` 判斷預期：未暫停 → 60 秒內應重建；暫停中 → 40 秒內不應重建（驗證裁決），
    # 重建本身標 PENDING。
    $pausedLines = @((Read-Unified) -split "`n" | Where-Object { $_ -match 'paused=(true|false)' })
    $hostPaused = $pausedLines.Count -gt 0 -and $pausedLines[-1] -match 'paused=true'
    W "H 宿主暫停狀態 paused=$hostPaused（最後一行：$(if ($pausedLines.Count) { $pausedLines[-1].Trim() })）locked(LogonUI/LockApp)=$([int](Test-Locked))"
    $before = Get-WidgetRows (Get-PageState)
    $fixedBefore = @($before | Where-Object { $_.id -eq 'fixed' })
    $t0 = @($fixedBefore | ForEach-Object { $_.timeOrigin })
    $hangAt = Get-Date
    $hangOut = & $Node $CdpScript "$CdpPort" hang fixed 2>&1
    W "H 注入無窮迴圈：$hangOut"
    $fixedRebuilt = {
        param($rows)
        $w = Get-WidgetRows $rows
        ($w.Count -eq $WidgetIds.Count) -and [bool]($w | Where-Object { $_.id -eq 'fixed' -and $_.ok -and $t0 -notcontains $_.timeOrigin })
    }
    if (-not $hostPaused) {
        $r = Wait-State $fixedRebuilt 60
        $wd = Count-Lines 'kind=WATCHDOG_TIMEOUT'
        $others = @(Get-WidgetRows $r.rows | Where-Object { $_.id -ne 'fixed' -and $_.ok })
        W "H fixed rebuilt=$($r.ok) elapsedMs=$($r.elapsedMs)（自注入起 $([int]((Get-Date) - $hangAt).TotalMilliseconds) ms）watchdogLines=$wd $(Format-State $r.rows)"
        $results['H 頁面卡死：看門狗判定並重建 fixed（60 秒內），記錄檔有 WATCHDOG_TIMEOUT'] = $r.ok -and ($wd -ge 1)
        $results['H 看門狗只重建卡死的那一個（其餘四個 ok、WATCHDOG_TIMEOUT 恰 1 行）'] = ($wd -eq 1) -and ($others.Count -eq 4)
    } else {
        $r = Wait-State $fixedRebuilt 40
        $wd = Count-Lines 'kind=WATCHDOG_TIMEOUT'
        W "H 暫停中：40 秒內 fixed rebuilt=$($r.ok) watchdogLines=$wd（預期皆無：暫停時看門狗不計時）"
        $results['H 暫停中看門狗不計時（40 秒內未重建、無 WATCHDOG_TIMEOUT）'] = (-not $r.ok) -and ($wd -eq 0)
        $pending['H 卡死重建：宿主暫停中未驗證，需未暫停時重跑'] = $true
    }
    Write-Tree 'after-H' $hostPid
    Remember-Wv2

    # ── P. 暫停中復原仍保持暫停（fix round 1：get_pause） ─────────────────────────────
    $moving0 = Test-QuotesStill
    W "P 暫停前 scrollLeft $($moving0.a) → $($moving0.b) still=$($moving0.still)"
    $pauseRaw = Invoke-PageRaw 'w=quotes' "window.__TAURI__.core.invoke('self_test_set_manual_pause', { active: true })"
    $pauseOutcome = Get-ManualPauseOutcome $pauseRaw.ExitCode $pauseRaw.Text
    W "P self_test_set_manual_pause(true) → exit=$($pauseRaw.ExitCode) $($pauseRaw.Text)（判讀：$pauseOutcome）"
    if ($pauseOutcome -eq 'missing') {
        W 'P 本建置沒有驗收用指令 self_test_set_manual_pause（非 self-test-ipc 建置）：P 項列 PENDING'
        $pending['P 暫停中復原仍保持暫停（需 --features self-test-ipc 建置重跑）'] = $true
    } elseif ($pauseOutcome -ne 'ok') {
        W 'P 驗收用指令存在但呼叫失敗'
        $results['P 手動暫停（驗收用指令）生效'] = $false
    } else {
        Start-Sleep -Milliseconds 500
        $p1 = Test-QuotesStill
        W "P 暫停後 scrollLeft $($p1.a) → $($p1.b) still=$($p1.still)"
        $results['P 暫停前跑馬燈在動、暫停後不動（基準）'] = (-not $moving0.still) -and $p1.still

        # 逐一終止本宿主 renderer，直到 quotes 被重新載入（每次只影響一個小工具）。
        $qT0 = Get-QuotesT0
        $reloaded = $false
        $triedP = @()
        for ($i = 1; $i -le 8 -and -not $reloaded; $i++) {
            $cands = @(Get-OurWv2 $hostPid | Where-Object { (Get-Wv2Type $_) -eq 'renderer' -and $triedP -notcontains [int]$_.ProcessId })
            if ($cands.Count -eq 0) { break }
            $victim = [int]$cands[0].ProcessId
            $triedP += $victim
            if (-not (Stop-Ours $hostPid $victim "P renderer #$i")) { break }
            $r = Wait-State { param($rows) $w = Get-WidgetRows $rows; -not ($WidgetIds | Where-Object { $id = $_; -not ($w | Where-Object { $_.id -eq $id -and $_.ok }) }) } 10
            $nowT0 = Get-QuotesT0
            $reloaded = ($null -ne $nowT0) -and ($nowT0 -ne $qT0)
            W "P renderer #$i 後全部 ok=$($r.ok) quotesReloaded=$reloaded"
            Remember-Wv2
        }
        $gp = Invoke-Page 'w=quotes' "window.__TAURI__.core.invoke('get_pause')"
        $p2 = Test-QuotesStill
        W "P renderer 重新載入 quotes 後 get_pause=$($gp | ConvertTo-Json -Compress) scrollLeft $($p2.a) → $($p2.b) still=$($p2.still)"
        $results['P 暫停中 renderer 終止、quotes 重新載入後仍保持暫停（get_pause=paused、scrollLeft 不變）'] = $reloaded -and $p2.still -and ($gp.paused -eq $true)

        # browser 終止 → 全部重建。
        $qT0 = Get-QuotesT0
        $browser = @(Get-OurWv2 $hostPid | Where-Object { (Get-Wv2Type $_) -eq 'browser' -and [int]$_.ParentProcessId -eq $hostPid })
        $rebuilt = $false
        if ($browser.Count -eq 1 -and (Stop-Ours $hostPid ([int]$browser[0].ProcessId) 'P browser')) {
            $r = Wait-State { param($rows) [bool](Get-WidgetRows $rows | Where-Object { $_.id -eq 'quotes' -and $_.ok -and $_.timeOrigin -ne $qT0 }) } 10
            $rebuilt = $r.ok
            Start-Sleep -Seconds 1
        }
        Remember-Wv2
        $p3 = Test-QuotesStill
        W "P browser 重建後 quotesRebuilt=$rebuilt scrollLeft $($p3.a) → $($p3.b) still=$($p3.still)"
        $results['P 暫停中 browser 終止、重建後的 quotes 仍保持暫停（scrollLeft 不變）'] = $rebuilt -and $p3.still

        $resume = Invoke-Page 'w=quotes' "window.__TAURI__.core.invoke('self_test_set_manual_pause', { active: false })"
        Start-Sleep -Milliseconds 500
        $p4 = Test-QuotesStill
        W "P 解除暫停 → $resume scrollLeft $($p4.a) → $($p4.b) still=$($p4.still)"
        $results['P 解除暫停後跑馬燈恢復移動'] = ($resume -eq $false) -and (-not $p4.still)
    }
}
finally {
    $locked2 = Test-Locked
    if ($hostProc -and -not $hostProc.HasExited) {
        Remember-Wv2
        Stop-Process -Id $hostProc.Id -Force -ErrorAction SilentlyContinue
        [void]$hostProc.WaitForExit(10000)
    }
    Start-Sleep -Seconds 2
    # 收尾只清「本次記錄過（PID＋CreationDate 相同，排除 PID 重用）且命令列標記相符」的殘留。
    $left = @()
    foreach ($procId in @($knownWv2.Keys)) {
        $p = Get-CimInstance Win32_Process -Filter "ProcessId=$procId" -ErrorAction SilentlyContinue
        if ($p -and $p.CreationDate -eq $knownWv2[$procId] -and $p.CommandLine -and
            $p.CommandLine.Contains($ExeMarker) -and $p.CommandLine.Contains($PortMarker)) {
            $left += $procId
            Stop-Process -Id $procId -Force -ErrorAction SilentlyContinue
        }
    }
    Start-Sleep -Seconds 1
    $stillHost = @(Get-Process -Name fc-host -ErrorAction SilentlyContinue).Count
    $stillWv2 = @(Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" | Where-Object {
        $_.CommandLine -and $_.CommandLine.Contains($ExeMarker) -and $_.CommandLine.Contains($PortMarker) }).Count
    $regLeft = @(Restore-FcHostAutostartRegistry $regSnap)
    if ($regLeft.Count -gt 0) { Write-Warning "開機自啟登錄未還原：$($regLeft -join '; ')" }
    W "開機自啟登錄還原：未還原 $($regLeft.Count) 項$(if ($regLeft.Count) { '：' + ($regLeft -join '; ') })"
    $results['開機自啟登錄已還原（Run／StartupApproved 的 fc-host）'] = ($regLeft.Count -eq 0)
    W "# END locked=$([int]$locked2) leftoverKilled=[$($left -join ',')] fcHostRemaining=$stillHost ourWebView2Remaining=$stillWv2 knownPids=$($knownWv2.Count)"
    $results['E 結束後無殘留 fc-host 與本次 WebView2 行程'] = ($stillHost -eq 0) -and ($stillWv2 -eq 0)
    $log.Close()
}

$sum = New-EvidenceWriter $sumPath
$sum.WriteLine("# verify-5.6.ps1 summary $(Get-Ts)")
foreach ($k in $results.Keys) { $sum.WriteLine("$(if ($results[$k]) { 'PASS' } else { 'FAIL' })  $k") }
foreach ($k in $pending.Keys) { $sum.WriteLine("PENDING  $k") }
$sum.Close()
Get-Content $sumPath

try { Copy-EvidenceFile -Source $unifiedLogPath -Destination (Join-Path $OutDir '5.6-unified.log') } catch { Write-Warning "複製統一記錄檔失敗：$_" }
Remove-Item -Recurse -Force $tempRoot -ErrorAction SilentlyContinue

if (@($results.Values | Where-Object { -not $_ }).Count -gt 0) { exit 1 }
if ($pending.Count -gt 0) { exit 2 }
exit 0
