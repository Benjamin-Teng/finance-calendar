<#
.SYNOPSIS
    Task 5.8 驗收驅動腳本（本機記錄檔：每日輪替、保留 7 天；design.md D12「記錄」條，
    task-5.8-brief「製造一次資料解析錯誤與一次 renderer 終止，確認記錄檔出現」）。

.DESCRIPTION
    只讀行程列舉／訊息列舉與 CIM 查詢，**不注入任何輸入**（不送按鍵、不點擊、不截圖），鎖定
    時也可跑（`Stop-Process` 依 PID 精確終止，不是輸入注入）。

    步驟：
      1. 確認沒有 fc-host 在跑。
      2. 以暫存目錄同時當 %APPDATA%（=> 首次啟動）與 %LOCALAPPDATA%（=> 統一記錄檔目錄
         `<temp>\tw.fintools.fc-host\logs\`、資料目錄 `<temp>\tw.fintools.fc-host\data\`，
         兩者都是本 crate 自己讀 env 算出來的路徑，確實落在暫存樹下）——完全不碰使用者真正的
         設定檔、`D:\finance-calendar`，或使用者真正的記錄檔。**注意**：WebView2 的預設
         user data folder 不吃這個 env 覆寫（仍解析到真正的
         `%LOCALAPPDATA%\tw.fintools.fc-host\EBWebView`，本腳本第一次跑時以診斷 dump
         證實），故步驟 6 改用行程命令列裡的 `webview-exe-name=fc-host.exe` ＋本次專屬的
         `--remote-debugging-port` 精確識別，不依賴 UDF 路徑。
      3. 啟動宿主（首次啟動，五個財經小工具開）。
      4. 驗證啟動當下已寫入「fc-host 啟動」一行到今天的記錄檔（`logging::init` 在
         `main()` 最開頭呼叫，早於任何視窗）。
      5. 在資料目錄寫入一份壞掉的 `tw_events.json`（不是合法 JSON），等一個輪詢週期
         （`POLL_INTERVAL`=30 秒，留 35 秒餘裕），驗證記錄檔出現一行含
         `資料來源解析失敗` 與 `channel=tw-events` 的 `WARN`。
      6. 精確找出「本次啟動」的 `msedgewebview2.exe`：命令列同時含
         `webview-exe-name=fc-host.exe` 與本次啟動專屬的 `--remote-debugging-port=<CdpPort>`
         （步驟 1 已確認啟動前沒有其他 fc-host 在跑，這兩個條件合起來只會命中本次啟動生出的
         行程，不會誤殺其他應用程式或其他宿主實例；絕不用 `taskkill /IM msedgewebview2.exe`
         不帶 PID），從中挑一個 `--type=renderer` 的行程 `Stop-Process -Id`，等 10 秒，驗證
         記錄檔出現一行含 `WebView2 故障` 與 `kind=RENDER_PROCESS_EXITED` 的 `ERROR`（見
         `desktop::install_process_failed_logger`／`log_process_failed`）。
      7. 結束宿主、刪除暫存目錄。

.PARAMETER Exe
    fc-host.exe 路徑，預設 host/target/release/fc-host.exe。

.PARAMETER OutDir
    證據輸出目錄，預設 host/tools/evidence。

.PARAMETER CdpPort
    WebView2 remote debugging 埠（本腳本不使用 CDP，只是避免與其他驗收腳本的預設埠衝突時
    WebView2 抱怨埠被占用；預設 9338，未被 verify-3.x/4.x 使用）。
#>
[CmdletBinding()]
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\fc-host.exe'),
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [int]$CdpPort = 9338
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\AutostartRegistry.psm1') -Force

Add-Type -Namespace V58 -Name Native -MemberDefinition @'
[DllImport("user32.dll")] public static extern System.IntPtr GetTopWindow(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern System.IntPtr GetWindow(System.IntPtr hWnd, uint cmd);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr hWnd, out uint pid);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(System.IntPtr hWnd, System.Text.StringBuilder s, int n);
'@

function Get-Ts { Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fffK' }
function Test-Locked { [bool](Get-Process -Name LogonUI, LockApp -ErrorAction SilentlyContinue) }

function Get-HostWindowTitles([int]$ProcId) {
    $titles = @()
    $h = [V58.Native]::GetTopWindow([IntPtr]::Zero)
    while ($h -ne [IntPtr]::Zero) {
        $p = 0
        [void][V58.Native]::GetWindowThreadProcessId($h, [ref]$p)
        if ($p -eq $ProcId -and [V58.Native]::IsWindowVisible($h)) {
            $sb = New-Object System.Text.StringBuilder 256
            [void][V58.Native]::GetWindowText($h, $sb, 256)
            if ($sb.Length -gt 0) { $titles += $sb.ToString() }
        }
        $h = [V58.Native]::GetWindow($h, 2)
    }
    return $titles
}

function Wait-WindowTitles([int]$ProcId, [scriptblock]$Cond, [int]$TimeoutSec = 20) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt $TimeoutSec) {
        $titles = Get-HostWindowTitles $ProcId
        if (& $Cond $titles) { return $true }
        Start-Sleep -Milliseconds 300
    }
    return $false
}

# ── 1. 前置檢查 ─────────────────────────────────────────────────────────────────
if (Get-Process -Name fc-host -ErrorAction SilentlyContinue) {
    throw '已有 fc-host 在執行，請先結束（避免與本次暫存實例的行程列舉互相干擾）。'
}
$Exe = (Resolve-Path $Exe).Path
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$logPath = Join-Path $OutDir '5.8-driver.log'
$sumPath = Join-Path $OutDir '5.8-summary.log'
$log = New-EvidenceWriter $logPath
$log.AutoFlush = $true
$log.WriteLine("# verify-5.8.ps1 start=$(Get-Ts) exe=$Exe locked=$([int](Test-Locked))")

# ── 2. 暫存 %APPDATA%／%LOCALAPPDATA%（不碰使用者真正的設定檔／資料目錄／記錄檔） ─────────
$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ("fc-host-5.8-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
$tempAppData = Join-Path $tempRoot 'Roaming'
$tempLocalAppData = Join-Path $tempRoot 'Local'
New-Item -ItemType Directory -Force -Path $tempAppData, $tempLocalAppData | Out-Null
$dataDir = Join-Path $tempLocalAppData 'tw.fintools.fc-host\data'
$logDir = Join-Path $tempLocalAppData 'tw.fintools.fc-host\logs'
New-Item -ItemType Directory -Force -Path $dataDir | Out-Null
$today = Get-Date -Format 'yyyy-MM-dd'
$unifiedLogPath = Join-Path $logDir "fc-host.$today.log"
$log.WriteLine("# APPDATA=$tempAppData（首次啟動）LOCALAPPDATA=$tempLocalAppData（資料目錄=$dataDir，統一記錄檔=$unifiedLogPath）")

$regSnap = @(Save-FcHostAutostartRegistry)
$log.WriteLine("# $(Get-Ts) 開機自啟登錄快照：$(($regSnap | ForEach-Object { "$($_.Name)@$(Split-Path $_.Key -Leaf) Exists=$($_.Exists)" }) -join '; ')")
$results = [ordered]@{}
$hostProc = $null
$oldAppData = $env:APPDATA
$oldLocalAppData = $env:LOCALAPPDATA
$oldWv2 = $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS
try {
    # ── 3. 啟動宿主 ───────────────────────────────────────────────────────────────
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

    $ok = Wait-WindowTitles $hostPid { param($t) ($t | Where-Object { $_ -eq 'fc-host clock' }) }
    $log.WriteLine("## $(Get-Ts) 首次啟動：五個財經小工具就緒＝$ok（視窗清單：$((Get-HostWindowTitles $hostPid) -join ', '))")
    $results['首次啟動成功（clock 視窗出現）'] = $ok

    # ── 4. 啟動事件：main() 最開頭 logging::init() 之後立刻 log::info!，早於任何視窗 ─────────
    $startupSeen = $false
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt 10) {
        if (Test-Path $unifiedLogPath) {
            $content = Get-Content -Raw $unifiedLogPath -ErrorAction SilentlyContinue
            if ($content -match 'fc-host 啟動 version=') { $startupSeen = $true; break }
        }
        Start-Sleep -Milliseconds 300
    }
    $log.WriteLine("## $(Get-Ts) 啟動記錄行出現＝$startupSeen（記錄檔=$unifiedLogPath）")
    $results['啟動時已寫入「fc-host 啟動」一行（早於任何視窗）'] = $startupSeen

    # ── 5. 壞 JSON 資料錯誤 ──────────────────────────────────────────────────────
    $badJsonPath = Join-Path $dataDir 'tw_events.json'
    $badJson = '{ 這不是合法的 JSON ,,,'
    Set-Content -Path $badJsonPath -Value $badJson -Encoding UTF8 -NoNewline
    $writeTs = Get-Date
    $log.WriteLine("## $(Get-Ts) 已寫入壞 JSON：$badJsonPath")

    $parseErrorSeen = $false
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt 35) {
        $content = Get-Content -Raw $unifiedLogPath -ErrorAction SilentlyContinue
        if ($content -match '資料來源解析失敗' -and $content -match 'channel=tw-events') {
            $parseErrorSeen = $true; break
        }
        Start-Sleep -Seconds 2
    }
    $elapsed = [math]::Round(((Get-Date) - $writeTs).TotalSeconds, 1)
    $log.WriteLine("## $(Get-Ts) 壞 JSON 寫入後 ${elapsed}s，解析失敗記錄出現＝$parseErrorSeen")
    $results['壞 JSON 資料錯誤：一個輪詢週期內記錄檔出現「資料來源解析失敗 channel=tw-events」'] = $parseErrorSeen

    # ── 6. renderer 終止（只殺本次啟動的行程，精確以 PID 終止） ─────────────────────────────
    # 發現（值得記錄）：把 $env:LOCALAPPDATA 導到暫存目錄只影響本 crate 自己讀 env 的程式碼
    # （settings.rs／logging.rs 的 default_*_path，見上面資料目錄／記錄檔都確實落在暫存路徑）；
    # WebView2 的預設 user data folder 不吃這個 env 覆寫，仍解析到「真正」的
    # `%LOCALAPPDATA%\tw.fintools.fc-host\EBWebView`（診斷 dump 已證實）。故改用
    # `webview-exe-name=fc-host.exe` ＋本次啟動特有的 `--remote-debugging-port=$CdpPort`
    # （這兩個字串合起來只會出現在「本次啟動的 fc-host」所生出的 msedgewebview2.exe 命令列
    # 裡；步驟 1 已確認啟動前沒有其他 fc-host 在跑，故不會誤殺其他實例或其他應用程式）取代
    # UDF 路徑比對；`--type=renderer` 篩出真正的 renderer 行程，不含 browser／gpu／utility。
    # WebView2 的 browser／renderer 行程是非同步啟動的，視窗標題出現不代表 renderer 行程已經
    # 在 Win32_Process 看得到；輪詢最多 20 秒等它們出現。
    $rendererMarker = "webview-exe-name=fc-host.exe"
    $portMarker = "--remote-debugging-port=$CdpPort"
    $renderers = @()
    $findSw = [Diagnostics.Stopwatch]::StartNew()
    while ($findSw.Elapsed.TotalSeconds -lt 20) {
        $renderers = @(Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" | Where-Object {
            $_.CommandLine -and
            $_.CommandLine.Contains($rendererMarker) -and
            $_.CommandLine.Contains($portMarker) -and
            $_.CommandLine -match '--type=renderer'
        })
        if ($renderers.Count -ge 1) { break }
        Start-Sleep -Milliseconds 500
    }
    $log.WriteLine("## $(Get-Ts) 本次啟動的 renderer 行程數＝$($renderers.Count)（等待 $([math]::Round($findSw.Elapsed.TotalSeconds, 1))s）")
    if ($renderers.Count -eq 0) {
        # 診斷用：列出全部 msedgewebview2.exe 的命令列，協助判讀是比對條件不符還是根本沒有
        # renderer 行程。
        $allWv2 = @(Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'")
        $log.WriteLine("## 診斷：系統上全部 msedgewebview2.exe（$($allWv2.Count) 個）：")
        foreach ($p in $allWv2) { $log.WriteLine("##   pid=$($p.ProcessId) cmd=$($p.CommandLine)") }
    }
    $processFailedSeen = $false
    if ($renderers.Count -ge 1) {
        $target = $renderers[0]
        $log.WriteLine("## $(Get-Ts) 終止 renderer pid=$($target.ProcessId)")
        Stop-Process -Id $target.ProcessId -Force

        $sw = [Diagnostics.Stopwatch]::StartNew()
        while ($sw.Elapsed.TotalSeconds -lt 10) {
            $content = Get-Content -Raw $unifiedLogPath -ErrorAction SilentlyContinue
            if ($content -match 'WebView2 故障' -and $content -match 'kind=RENDER_PROCESS_EXITED') {
                $processFailedSeen = $true; break
            }
            Start-Sleep -Milliseconds 500
        }
    } else {
        $log.WriteLine('## 找不到本次啟動的 renderer 行程，略過 renderer 終止步驟')
    }
    $log.WriteLine("## $(Get-Ts) renderer 終止記錄出現＝$processFailedSeen")
    $results['找得到本次啟動的至少一個 renderer 行程'] = ($renderers.Count -ge 1)
    $results['renderer 終止後 10 秒內記錄檔出現「WebView2 故障…kind=RENDER_PROCESS_EXITED」'] = $processFailedSeen

    # ── 7. 截圖：刻意不做（不涉及視覺驗收；鎖定狀態僅供記錄） ────────────────────────────
    $log.WriteLine("## 截圖：略過（鎖定狀態=$([int](Test-Locked))）")
}
finally {
    if ($hostProc -and -not $hostProc.HasExited) { Stop-Process -Id $hostProc.Id -Force -ErrorAction SilentlyContinue }
    # 清掉本次啟動可能殘留的 msedgewebview2.exe（同 probe-1.4.ps1 收尾原則：只清精確比對
    # 本次啟動特徵者，不做全域字串搜尋，不影響其他應用程式或其他宿主實例；比對條件同步驟 6，
    # UDF 路徑不吃 env 覆寫的原因見該步驟註解）。
    Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" -ErrorAction SilentlyContinue |
        Where-Object {
            $_.CommandLine -and
            $_.CommandLine.Contains('webview-exe-name=fc-host.exe') -and
            $_.CommandLine.Contains("--remote-debugging-port=$CdpPort")
        } |
        ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
    $regLeft = @(Restore-FcHostAutostartRegistry $regSnap)
    if ($regLeft.Count -gt 0) { Write-Warning "開機自啟登錄未還原：$($regLeft -join '; ')" }
    $log.WriteLine("# $(Get-Ts) 開機自啟登錄還原：未還原 $($regLeft.Count) 項$(if ($regLeft.Count) { '：' + ($regLeft -join '; ') })")
    $results['開機自啟登錄已還原（Run／StartupApproved 的 fc-host）'] = ($regLeft.Count -eq 0)
    $log.WriteLine("# $(Get-Ts) 宿主已結束")
    $log.Close()
}

$sum = New-EvidenceWriter $sumPath
$sum.WriteLine("# verify-5.8.ps1 summary $(Get-Ts)")
foreach ($k in $results.Keys) { $sum.WriteLine("$(if ($results[$k]) { 'PASS' } else { 'FAIL' })  $k") }
$sum.Close()
Get-Content $sumPath

# fix F7（批次 A）：統一記錄檔含使用者路徑（解析失敗 WARN 那行），複製後一律去識別。
try { Copy-EvidenceFile -Source $unifiedLogPath -Destination (Join-Path $OutDir '5.8-unified.log') } catch { Write-Warning "複製統一記錄檔失敗：$_" }
Remove-Item -Recurse -Force $tempRoot -ErrorAction SilentlyContinue

$failed = @($results.Values | Where-Object { -not $_ })
if ($failed.Count -gt 0) { exit 1 }
exit 0
