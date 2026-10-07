<#
.SYNOPSIS
    Task 4.6 驗收驅動腳本（擴充插槽 custom1–custom5 空白模組，design.md D6，
    finance-widgets spec「擴充插槽小工具」、widget-data-feed spec「擴充通道讀取任意
    JSON」／「拒收無效資料並保留上一份好資料」／「尚無資料」）：在宿主中開啟 custom1／
    custom2，驗證放入範例 custom1.json 後 60 秒內顯示摘要、刪除後仍保留上一份、custom2
    無檔時顯示「尚未設定」。只讀視窗狀態與透過 CDP 呼叫宿主自己的 IPC 指令／讀頁面 DOM，
    **不注入任何輸入**（不送按鍵、不點擊、不截圖），鎖定時也可跑。

.DESCRIPTION
    步驟：
      1. 確認沒有 fc-host 在跑（WebView2 共用 user data folder，已有實例時 CDP 參數不生效）。
      2. 以暫存目錄同時當 %APPDATA%（=> 沒有 settings.json＝首次啟動，custom1／custom2
         預設關閉）與 %LOCALAPPDATA%（=> 資料目錄預設值 tw.fintools.fc-host\data，見
         settings::default_data_dir）——**完全不碰使用者真正的設定檔或 D:\finance-calendar**，
         比照 verify-3.1.ps1／verify-3.2.ps1 的作法（task-4.6-brief.md「先備份使用者既有
         settings.json…或以暫存路徑執行」二選一，這裡選後者：settings.json 全程只存在於
         暫存 %APPDATA%，不曾碰過真正的檔案，比「備份＋還原」更不會出錯）。
      3. 啟動宿主（首次啟動，五個財經小工具開、customN 全關）。
      4. 透過 CDP 呼叫 `update_settings` 開啟 custom1 與 custom2（brief「開啟 custom1」；
         custom2 也要開才能觀察「無檔時顯示尚未設定」——插槽沒開視窗根本不存在）。
      5. 兩個插槽此時資料目錄都沒有對應的 json，讀 DOM 驗證皆顯示「尚未設定」。
      6. 寫入範例 `custom1.json`，60 秒內輪詢 CDP 讀 custom1 頁面 DOM，驗證出現內容摘要
         （非「尚未設定」）。
      7. 刪除 `custom1.json`，等超過一個輪詢週期（35 秒，資料層至少每 30 秒檢查一次，
         design.md D5／widget-data-feed spec「偵測資料更新」），驗證 custom1 仍顯示
         **同一份**摘要（JsonFileSource 保留上一份好資料，不會因檔案消失而回報空）。
      8. 全程驗證 custom2（從未有對應檔案）持續顯示「尚未設定」。
      9. 結束宿主、刪除暫存目錄。

.PARAMETER Exe
    fc-host.exe 路徑，預設 host/target/release/fc-host.exe。

.PARAMETER OutDir
    證據輸出目錄，預設 host/tools/evidence。

.PARAMETER CdpPort
    WebView2 remote debugging 埠，預設 9336（與 verify-3.1/3.2/3.3 的 9333/9334/9335 分開，
    避免同時跑時衝突）。
#>
[CmdletBinding()]
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\fc-host.exe'),
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [int]$CdpPort = 9336
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\AutostartRegistry.psm1') -Force

Add-Type -Namespace V46 -Name Native -MemberDefinition @'
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
    $h = [V46.Native]::GetTopWindow([IntPtr]::Zero)
    while ($h -ne [IntPtr]::Zero) {
        $p = 0
        [void][V46.Native]::GetWindowThreadProcessId($h, [ref]$p)
        if ($p -eq $ProcId -and [V46.Native]::IsWindowVisible($h)) {
            $sb = New-Object System.Text.StringBuilder 256
            [void][V46.Native]::GetWindowText($h, $sb, 256)
            if ($sb.Length -gt 0) { $titles += $sb.ToString() }
        }
        $h = [V46.Native]::GetWindow($h, 2)
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

function Invoke-PageEval([string]$WidgetId, [string]$Expr) {
    $out = & node (Join-Path $PSScriptRoot 'host-cdp-eval.mjs') $CdpPort "w=$WidgetId" $Expr 2>&1
    return ($out -join '')
}

# 讀 #widget-root 的純文字內容（custom.js 渲染出的標題／更新時間／摘要或「尚未設定」全部
# 在這個節點內，不需要逐一比對 CSS class）。
function Get-WidgetRootText([string]$WidgetId) {
    $raw = Invoke-PageEval $WidgetId "(() => { const r = document.getElementById('widget-root'); return r ? r.innerText : null; })()"
    try { return ($raw | ConvertFrom-Json) } catch { return "<解析失敗：$raw>" }
}

# ── 1. 前置檢查 ─────────────────────────────────────────────────────────────────
if (Get-Process -Name fc-host -ErrorAction SilentlyContinue) {
    throw '已有 fc-host 在執行，請先結束（WebView2 共用 user data folder，CDP 參數不會生效）。'
}
$Exe = (Resolve-Path $Exe).Path
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$logPath = Join-Path $OutDir '4.6-log.log'
$sumPath = Join-Path $OutDir '4.6-summary.log'
$log = New-EvidenceWriter $logPath
$log.AutoFlush = $true
$log.WriteLine("# verify-4.6.ps1 start=$(Get-Ts) exe=$Exe locked=$([int](Test-Locked))")

# ── 2. 暫存 %APPDATA%／%LOCALAPPDATA%（不碰使用者真正的設定檔／資料目錄） ─────────────────
$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ("fc-host-4.6-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
$tempAppData = Join-Path $tempRoot 'Roaming'
$tempLocalAppData = Join-Path $tempRoot 'Local'
New-Item -ItemType Directory -Force -Path $tempAppData, $tempLocalAppData | Out-Null
$dataDir = Join-Path $tempLocalAppData 'tw.fintools.fc-host\data'
New-Item -ItemType Directory -Force -Path $dataDir | Out-Null
$custom1Path = Join-Path $dataDir 'custom1.json'
$log.WriteLine("# APPDATA=$tempAppData（無 settings.json＝首次啟動，customN 預設關閉）LOCALAPPDATA=$tempLocalAppData（資料目錄=$dataDir，目前沒有任何 customN.json）")

$regSnap = @(Save-FcHostAutostartRegistry)
$log.WriteLine("# $(Get-Ts) 開機自啟登錄快照：$(($regSnap | ForEach-Object { "$($_.Name)@$(Split-Path $_.Key -Leaf) Exists=$($_.Exists)" }) -join '; ')")
$results = [ordered]@{}
$hostProc = $null
$oldAppData = $env:APPDATA
$oldLocalAppData = $env:LOCALAPPDATA
$oldWv2 = $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS
try {
    # ── 3. 啟動宿主（首次啟動） ───────────────────────────────────────────────────
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

    # ── 4. 開啟 custom1／custom2（brief：設定視窗尚未完成，經由 update_settings 等效於
    #     「直接編輯 settings.json」——兩者都只是把 widgets.custom1/2.enabled 改成 true，
    #     差別只在落地路徑，經由 IPC 不會碰到使用者真正的檔案，見 verify-3.1.ps1 同一手法）
    $r = Invoke-PageEval 'clock' "window.__TAURI__.core.invoke('update_settings', { patch: { widgets: { custom1: { enabled: true }, custom2: { enabled: true } } } }).then(s => [s.widgets.custom1.enabled, s.widgets.custom2.enabled])"
    $log.WriteLine("## $(Get-Ts) update_settings custom1=on custom2=on → $r")
    $ok = Wait-WindowTitles $hostPid { param($t) (@($t | Where-Object { $_ -in 'fc-host custom1', 'fc-host custom2' })).Count -eq 2 }
    Start-Sleep -Seconds 2
    $log.WriteLine("## $(Get-Ts) custom1／custom2 視窗就緒＝$ok（視窗清單：$((Get-HostWindowTitles $hostPid) -join ', '))")
    $results['開啟 custom1 與 custom2 → 即時建立視窗'] = $ok

    # ── 5. 兩者初始都沒有對應檔案 → 應顯示「尚未設定」 ───────────────────────────────
    $t1 = Get-WidgetRootText 'custom1'
    $t2 = Get-WidgetRootText 'custom2'
    $log.WriteLine("## $(Get-Ts) 初始 custom1 文字：$t1")
    $log.WriteLine("## $(Get-Ts) 初始 custom2 文字：$t2")
    $results['custom1 初始無檔 → 顯示「尚未設定」'] = ($t1 -match '尚未設定')
    $results['custom2 初始無檔 → 顯示「尚未設定」'] = ($t2 -match '尚未設定')

    # ── 6. 寫入範例 custom1.json，60 秒內應顯示內容摘要（頂層鍵與筆數，design.md D6） ───────
    $sample = '{"updated": "2026-09-28 12:00", "items": [{"title": "範例項目 1", "value": 42}, {"title": "範例項目 2", "value": 17}, {"title": "範例項目 3", "value": 5}]}'
    Set-Content -Path $custom1Path -Value $sample -Encoding UTF8 -NoNewline
    $writeTs = Get-Date
    $log.WriteLine("## $(Get-Ts) 已寫入 $custom1Path：$sample")

    $summaryShown = $false
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $lastText = $null
    while ($sw.Elapsed.TotalSeconds -lt 60) {
        $lastText = Get-WidgetRootText 'custom1'
        if ($lastText -match '頂層鍵' -and $lastText -notmatch '尚未設定') { $summaryShown = $true; break }
        Start-Sleep -Seconds 3
    }
    $elapsed = [math]::Round(((Get-Date) - $writeTs).TotalSeconds, 1)
    $log.WriteLine("## $(Get-Ts) 放入 custom1.json 後 ${elapsed}s：$lastText")
    $results['放入 custom1.json 後 60 秒內顯示內容摘要'] = $summaryShown
    # 鍵的順序不保證（fix：serde_json::Value 預設用 BTreeMap，物件鍵在 Rust 端往返一趟會被
    # 排成字母序，「updated」「items」的相對順序不是 custom.js 的邏輯錯誤，是資料管線的既有
    # 行為——JSON 物件鍵順序本來就不受 JSON 規格保障，這裡只驗證兩個鍵與各自筆數都有出現，
    # 不要求特定順序）。
    $hasItems = $lastText -match [regex]::Escape('items（3 筆）')
    $hasUpdated = $lastText -match [regex]::Escape('updated')
    $hasCount = $lastText -match [regex]::Escape('2 個頂層鍵')
    $results['摘要內容正確（2 個頂層鍵，含 updated 與 items（3 筆））'] = $hasItems -and $hasUpdated -and $hasCount
    $firstSummary = $lastText

    # ── 7. 刪除 custom1.json，等超過一個輪詢週期，仍應顯示同一份摘要（保留上一份好資料） ────
    Remove-Item -Path $custom1Path -Force
    $log.WriteLine("## $(Get-Ts) 已刪除 $custom1Path")
    Start-Sleep -Seconds 35
    $afterDelete = Get-WidgetRootText 'custom1'
    $log.WriteLine("## $(Get-Ts) 刪檔後 35 秒：$afterDelete")
    $results['刪除 custom1.json 後仍保留上一份摘要（不變回「尚未設定」）'] = ($afterDelete -notmatch '尚未設定') -and ($afterDelete -match '頂層鍵')
    $results['刪檔後顯示內容與刪檔前逐字相同'] = ($afterDelete -eq $firstSummary)

    # ── 8. 全程 custom2（從未有對應檔案）應持續顯示「尚未設定」 ───────────────────────────
    $t2b = Get-WidgetRootText 'custom2'
    $log.WriteLine("## $(Get-Ts) 全程結束後 custom2 文字：$t2b")
    $results['custom2 全程無檔 → 持續顯示「尚未設定」'] = ($t2b -match '尚未設定')

    # ── 9. 截圖：刻意不做（小工具在最底層，一般視窗開著時截到的是那些視窗；比照 verify-3.1/3.2） ──
    $log.WriteLine("## 截圖：略過（鎖定狀態=$([int](Test-Locked))），視覺比對列入 human-checklist")
}
finally {
    if ($hostProc -and -not $hostProc.HasExited) { Stop-Process -Id $hostProc.Id -Force }
    $regLeft = @(Restore-FcHostAutostartRegistry $regSnap)
    if ($regLeft.Count -gt 0) { Write-Warning "開機自啟登錄未還原：$($regLeft -join '; ')" }
    $log.WriteLine("# $(Get-Ts) 開機自啟登錄還原：未還原 $($regLeft.Count) 項$(if ($regLeft.Count) { '：' + ($regLeft -join '; ') })")
    $results['開機自啟登錄已還原（Run／StartupApproved 的 fc-host）'] = ($regLeft.Count -eq 0)
    $log.WriteLine("# $(Get-Ts) 宿主已結束")
    $log.Close()
}

$sum = New-EvidenceWriter $sumPath
$sum.WriteLine("# verify-4.6.ps1 summary $(Get-Ts)")
foreach ($k in $results.Keys) { $sum.WriteLine("$(if ($results[$k]) { 'PASS' } else { 'FAIL' })  $k") }
$sum.Close()
Get-Content $sumPath
Remove-Item -Recurse -Force $tempRoot -ErrorAction SilentlyContinue
