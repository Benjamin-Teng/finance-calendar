<#
.SYNOPSIS
    task 2.7 follow-up 驗收驅動腳本（design.md D4／specs/widget-data-feed「資料目錄」「尚無
    資料」）：驗證 `update_settings` 改 `data_dir` 重建通道註冊表後，會對已訂閱的小工具主動
    推送一筆新世代的 empty 快照，而不是讓畫面停在切換前的資料。只讀視窗狀態、透過 CDP 讀
    `dynamic` 小工具頁面的 DOM 文字，**不注入任何輸入**（不送按鍵、不點擊、不截圖），鎖定時
    也可跑。

.DESCRIPTION
    背景（task-4.1-report.md fix round 3「疑慮」）：`ChannelRegistry::replace_with` 只遞增
    世代，不主動推送；task 2.7 follow-up 在 `update_settings` 重建後另外對每個已訂閱通道推一
    筆 `snapshot: None`（empty）。本腳本驗證這個推送真的會讓「已開啟、原本顯示著資料」的小
    工具立刻改顯示「尚無資料」，而不是要等下一輪 30 秒排程輪詢才發現新目錄是空的。

    步驟：
      1. 確認沒有 fc-host 在跑。
      2. 準備暫存 %APPDATA%（首次啟動、無 settings.json）／%LOCALAPPDATA%，並在**預設資料
         目錄**（`%LOCALAPPDATA%\tw.fintools.fc-host\data`，見 settings.rs
         `default_data_dir`）先放入一份有效的 `tw_events.json`——host 首次啟動即讀到有資料。
         另準備一個「新資料目錄」，一開始刻意留空。
      3. 啟動宿主（帶 CDP 除錯埠），等 `dynamic` 小工具視窗就緒，確認其 `#dynList` 顯示的是
         真資料（不含「尚無資料」）——先確立「切換前有資料」這個前提，否則後面的「變成尚無
         資料」無法區分「本來就是空的」與「切換後才空」。
      4. 直接在 `dynamic` 頁面以 CDP 呼叫 `update_settings({ data_dir: <新資料目錄> })`
         （新目錄此時仍是空的）。
      5. 短時間輪詢（遠短於 30 秒排程間隔）`#dynList`，確認出現「尚無資料」——這是本 finding
         要驗的推送本身，若核心真的不推送，要等到下一輪排程（至多 30 秒）才會因為
         `poll_all_changed` 對「內容未變」的空目錄回報空清單而完全不觸發，畫面因此永遠停在
         舊資料，不會在這個短時間窗口內變化。
      6. 把有效的 `tw_events.json`（帶哨兵字串）寫進新資料目錄，30 秒內輪詢 `#dynList` 確認
         顯示新資料（驗證「empty 推送之後，正常的排程輪詢仍照常運作」，不是推送把後續輪詢
         弄壞了）。
      7. fix F3（review 2.7 medium）：在兩個「已有資料」的目錄之間來回切換三輪，每輪要求 35
         秒內顯示目標目錄的哨兵，之後觀察一段時間（最後一輪 35 秒，超過一個排程間隔）不得被
         「尚無資料」蓋掉——同世代 ok 先到、empty 後到的競態若重現，畫面會停在「尚無資料」。
      8. 結束宿主、刪除暫存目錄。

.PARAMETER Exe
    fc-host.exe 路徑，預設 host/target/release/fc-host.exe。

.PARAMETER OutDir
    證據輸出目錄，預設 host/tools/evidence。

.PARAMETER CdpPort
    WebView2 remote debugging 埠，預設 9343（與既有 verify-3.1/3.2/4.6/4.7/5.2/5.3/5.4/5.6/
    5.8/zoom-reload/6.1 的埠號分開）。
#>
[CmdletBinding()]
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\fc-host.exe'),
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [int]$CdpPort = 9343
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\AutostartRegistry.psm1') -Force

function Get-Ts { Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fffK' }
function Test-Locked { [bool](Get-Process -Name LogonUI, LockApp -ErrorAction SilentlyContinue) }

# 原生 HWND 標題出現不代表該視窗的 WebView2 已在 CDP 除錯埠註冊完成（見 verify-5.2.ps1 對這個
# 時機不穩定現象的說明）；`Invoke-PageEval` 對「找不到 url 含…的分頁」這個特定失敗訊息內建
# 重試，其餘錯誤原樣回傳。
function Invoke-PageEval([string]$UrlPart, [string]$Expr, [int]$TimeoutSec = 90) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $out = $null
    while ($true) {
        $out = (& node (Join-Path $PSScriptRoot 'host-cdp-eval.mjs') $CdpPort $UrlPart $Expr 2>&1) -join ''
        if ($out -notmatch '找不到 url 含') { return $out }
        if ($sw.Elapsed.TotalSeconds -ge $TimeoutSec) { return $out }
        Start-Sleep -Milliseconds 400
    }
}

function Invoke-DynamicEval([string]$Expr) {
    return Invoke-PageEval 'w=dynamic' $Expr
}

function Wait-DynListMatches([scriptblock]$Cond, [int]$TimeoutSec, [ref]$Last) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt $TimeoutSec) {
        $text = Invoke-DynamicEval "document.getElementById('dynList') ? document.getElementById('dynList').textContent : '<no #dynList>'"
        $Last.Value = $text
        if (& $Cond $text) { return $true }
        Start-Sleep -Milliseconds 500
    }
    return $false
}

# ── 1. 前置檢查 ─────────────────────────────────────────────────────────────────
if (Get-Process -Name fc-host -ErrorAction SilentlyContinue) {
    throw '已有 fc-host 在執行，請先結束（WebView2 共用 user data folder，CDP 參數不會生效）。'
}
$Exe = (Resolve-Path $Exe).Path
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$logPath = Join-Path $OutDir 'datadir-empty-log.log'
$sumPath = Join-Path $OutDir 'datadir-empty-summary.log'
$log = New-EvidenceWriter $logPath
$log.AutoFlush = $true
$log.WriteLine("# verify-datadir-empty.ps1 start=$(Get-Ts) exe=$Exe locked=$([int](Test-Locked))")

# ── 2. 暫存環境：預設資料目錄先放好資料，新資料目錄先留空 ───────────────────────────
$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ("fc-host-de-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
$tempAppData = Join-Path $tempRoot 'Roaming'
$tempLocalAppData = Join-Path $tempRoot 'Local'
$defaultDataDir = Join-Path $tempLocalAppData 'tw.fintools.fc-host\data'
$newDataDir = Join-Path $tempRoot 'new-data-dir'
New-Item -ItemType Directory -Force -Path $tempAppData, $tempLocalAppData, $defaultDataDir, $newDataDir | Out-Null

# validate_tw_events（host/src/data.rs，移植自 Lively 版 isEmptyPayload）要求
# macro/events/punish/quotes 四個頂層陣列合計長度 > 0；dynamic 小工具只看 `events`，這裡放一筆
# 帶未來日期的事件，確保切換前 #dynList 確實顯示內容而非「尚無資料」或「讀不到 tw_events」。
$initialSentinel = 'DE-INITIAL-SENTINEL'
$futureDate = (Get-Date).AddDays(3).ToString('yyyy-MM-dd')
$initialEvents = "{`"fetched`": `"$initialSentinel`", `"updated`": `"$initialSentinel`", `"macro`": [], `"punish`": [], `"quotes`": [], `"holidays`": [], `"events`": [{`"date`": `"$futureDate`", `"name`": `"$initialSentinel`", `"type`": `"earnings`"}]}"
Set-Content -Path (Join-Path $defaultDataDir 'tw_events.json') -Value $initialEvents -Encoding UTF8 -NoNewline

$newSentinel = 'DE-NEW-SENTINEL'
$newFutureDate = (Get-Date).AddDays(4).ToString('yyyy-MM-dd')
$newEvents = "{`"fetched`": `"$newSentinel`", `"updated`": `"$newSentinel`", `"macro`": [], `"punish`": [], `"quotes`": [], `"holidays`": [], `"events`": [{`"date`": `"$newFutureDate`", `"name`": `"$newSentinel`", `"type`": `"earnings`"}]}"

$log.WriteLine("# APPDATA=$tempAppData（無 settings.json＝首次啟動）LOCALAPPDATA=$tempLocalAppData")
$log.WriteLine("# 預設資料目錄=$defaultDataDir（已放入含 $initialSentinel 的 tw_events.json）")
$log.WriteLine("# 新資料目錄=$newDataDir（切換當下刻意留空）")

$regSnap = @(Save-FcHostAutostartRegistry)
$log.WriteLine("# $(Get-Ts) 開機自啟登錄快照：$(($regSnap | ForEach-Object { "$($_.Name)@$(Split-Path $_.Key -Leaf) Exists=$($_.Exists)" }) -join '; ')")
$results = [ordered]@{}
$hostProc = $null
$oldAppData = $env:APPDATA
$oldLocalAppData = $env:LOCALAPPDATA
$oldWv2 = $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS

function Start-Host {
    $p = $null
    try {
        $env:APPDATA = $script:tempAppData
        $env:LOCALAPPDATA = $script:tempLocalAppData
        $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$script:CdpPort"
        $p = Start-Process -FilePath $script:Exe -PassThru
    } finally {
        $env:APPDATA = $script:oldAppData
        $env:LOCALAPPDATA = $script:oldLocalAppData
        $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $script:oldWv2
    }
    return $p
}

try {
    # ── 3. 啟動宿主，等 dynamic 視窗就緒且顯示真資料 ─────────────────────────────────
    $hostProc = Start-Host
    $hostPid = $hostProc.Id
    $log.WriteLine("# $(Get-Ts) 宿主 pid=$hostPid")

    $before = $null
    $hasInitial = Wait-DynListMatches { param($t) $t -match [regex]::Escape($initialSentinel) } 30 ([ref]$before)
    $log.WriteLine("## $(Get-Ts) 切換前 #dynList（應含 $initialSentinel）：$before")
    $results['首次啟動：dynamic 小工具顯示預設資料目錄的真實資料'] = $hasInitial
    $results['切換前 #dynList 不含「尚無資料」（確立前提）'] = ($before -notmatch '尚無資料')

    # ── 4. CDP 呼叫 update_settings 切換到空目錄 ───────────────────────────────────
    $escapedDir = $newDataDir.Replace('\', '\\')
    $genBeforeRaw = Invoke-DynamicEval "window.__TAURI__.core.invoke('get_snapshot', { channel: 'tw-events' }).then(s => ({ status: s.status, generation: s.generation }))"
    $log.WriteLine("## $(Get-Ts) 切換前 get_snapshot：$genBeforeRaw")
    $switchExpr = "window.__TAURI__.core.invoke('update_settings', { patch: { data_dir: '$escapedDir' } }).then(s => s.data_dir)"
    $switchTs = Get-Date
    $switchResult = Invoke-DynamicEval $switchExpr
    $log.WriteLine("## $(Get-Ts) update_settings(data_dir=$newDataDir) 回傳：$switchResult")

    # ── 5. 短時間內（遠短於 30 秒排程間隔）應立即變成「尚無資料」──────────────────────
    $after = $null
    $becameEmpty = Wait-DynListMatches { param($t) $t -match '尚無資料' } 8 ([ref]$after)
    $elapsedEmpty = [math]::Round(((Get-Date) - $switchTs).TotalSeconds, 1)
    $log.WriteLine("## $(Get-Ts) 切換後 ${elapsedEmpty}s，#dynList：$after")
    $results["切換到空目錄後 8 秒內 dynamic 顯示「尚無資料」（推送，非等排程）（實際 ${elapsedEmpty}s）"] = $becameEmpty

    $genAfterRaw = Invoke-DynamicEval "window.__TAURI__.core.invoke('get_snapshot', { channel: 'tw-events' }).then(s => ({ status: s.status, generation: s.generation }))"
    $log.WriteLine("## $(Get-Ts) 切換後 get_snapshot：$genAfterRaw")
    $gb = ($genBeforeRaw | ConvertFrom-Json).generation
    $ga = ($genAfterRaw | ConvertFrom-Json).generation
    $results["get_snapshot 世代切換後遞增（$gb → $ga）"] = ($null -ne $gb) -and ($null -ne $ga) -and ([int64]$ga -gt [int64]$gb)

    # ── 6. 新目錄出現有效檔案後，30 秒內應顯示新資料 ─────────────────────────────────
    Set-Content -Path (Join-Path $newDataDir 'tw_events.json') -Value $newEvents -Encoding UTF8 -NoNewline
    $fillTs = Get-Date
    $log.WriteLine("## $(Get-Ts) 已寫入新資料目錄的 tw_events.json（含 $newSentinel）")
    $final = $null
    $pickedUp = Wait-DynListMatches { param($t) $t -match [regex]::Escape($newSentinel) } 30 ([ref]$final)
    $elapsedFill = [math]::Round(((Get-Date) - $fillTs).TotalSeconds, 1)
    $log.WriteLine("## $(Get-Ts) 新目錄出現檔案後 ${elapsedFill}s，#dynList：$final")
    $results["新目錄出現有效檔案後 30 秒內 dynamic 顯示新資料（實際 ${elapsedFill}s）"] = $pickedUp

    # ── 7. fix F3（review 2.7 medium）：切到「已有資料」的目錄 ───────────────────────────
    # 競態：重建後排程執行緒可能先送出新世代的 ok，補推的同世代 empty 若晚到會把 ok 蓋掉，畫面
    # 停在「尚無資料」直到檔案下次改寫（JsonFileSource 未變不再推送）。核心已改在 registry 鎖內
    # 補推、只推給新世代尚無快照的通道，頁面也丟棄同世代晚到的 empty。這個交錯無法由腳本強迫
    # 發生，所以在兩個「都已有資料」的目錄之間來回切換數輪，每輪都要求：出現目標目錄的哨兵後，
    # 再觀察一段時間（最後一輪超過一個 30 秒排程間隔）不得變成「尚無資料」、且仍顯示該哨兵。
    $prefilledSentinel = 'DE-PREFILLED-SENTINEL'
    $prefilledDataDir = Join-Path $tempRoot 'prefilled-data-dir'
    New-Item -ItemType Directory -Force -Path $prefilledDataDir | Out-Null
    $prefilledDate = (Get-Date).AddDays(5).ToString('yyyy-MM-dd')
    $prefilledEvents = "{`"fetched`": `"$prefilledSentinel`", `"updated`": `"$prefilledSentinel`", `"macro`": [], `"punish`": [], `"quotes`": [], `"holidays`": [], `"events`": [{`"date`": `"$prefilledDate`", `"name`": `"$prefilledSentinel`", `"type`": `"earnings`"}]}"
    Set-Content -Path (Join-Path $prefilledDataDir 'tw_events.json') -Value $prefilledEvents -Encoding UTF8 -NoNewline
    $log.WriteLine("## $(Get-Ts) 已準備預先有資料的目錄 $prefilledDataDir（含 $prefilledSentinel）")

    $rounds = @(
        @{ Dir = $prefilledDataDir; Sentinel = $prefilledSentinel; HoldSec = 5 },
        @{ Dir = $newDataDir; Sentinel = $newSentinel; HoldSec = 5 },
        @{ Dir = $prefilledDataDir; Sentinel = $prefilledSentinel; HoldSec = 35 }
    )
    $roundNo = 0
    foreach ($round in $rounds) {
        $roundNo++
        $dirEscaped = $round.Dir.Replace('\', '\\')
        $roundExpr = "window.__TAURI__.core.invoke('update_settings', { patch: { data_dir: '$dirEscaped' } }).then(s => s.data_dir)"
        $roundTs = Get-Date
        $roundResult = Invoke-DynamicEval $roundExpr
        $log.WriteLine("## $(Get-Ts) 第 $roundNo 輪 update_settings(data_dir=$($round.Dir)) 回傳：$roundResult")
        $sentinel = $round.Sentinel
        $seen = $null
        $shown = Wait-DynListMatches { param($t) $t -match [regex]::Escape($sentinel) } 35 ([ref]$seen)
        $elapsedShown = [math]::Round(((Get-Date) - $roundTs).TotalSeconds, 1)
        $log.WriteLine("## $(Get-Ts) 第 $roundNo 輪切換後 ${elapsedShown}s，#dynList：$seen")
        $results["第 $roundNo 輪切到已有資料的目錄後 35 秒內顯示 $sentinel（實際 ${elapsedShown}s）"] = $shown

        $stable = $shown
        $lastHeld = $seen
        if ($shown) {
            $holdSw = [Diagnostics.Stopwatch]::StartNew()
            while ($holdSw.Elapsed.TotalSeconds -lt $round.HoldSec) {
                $lastHeld = Invoke-DynamicEval "document.getElementById('dynList') ? document.getElementById('dynList').textContent : '<no #dynList>'"
                if ($lastHeld -match '尚無資料' -or $lastHeld -notmatch [regex]::Escape($sentinel)) {
                    $stable = $false
                    break
                }
                Start-Sleep -Milliseconds 500
            }
        }
        $log.WriteLine("## $(Get-Ts) 第 $roundNo 輪觀察 $($round.HoldSec)s 結束，#dynList：$lastHeld")
        $results["第 $roundNo 輪顯示 $sentinel 後 $($round.HoldSec) 秒內未被「尚無資料」蓋掉"] = $stable
    }

    $log.WriteLine("## 截圖：略過（鎖定狀態=$([int](Test-Locked))）")
}
catch {
    $log.WriteLine("## $(Get-Ts) 例外中止：$($_.Exception.Message)")
    $results['腳本未跑完（見 datadir-empty-log.log 例外訊息）'] = $false
}
finally {
    if ($hostProc -and -not $hostProc.HasExited) { Stop-Process -Id $hostProc.Id -Force -ErrorAction SilentlyContinue }
    $regLeft = @(Restore-FcHostAutostartRegistry $regSnap)
    if ($regLeft.Count -gt 0) { Write-Warning "開機自啟登錄未還原：$($regLeft -join '; ')" }
    $log.WriteLine("# $(Get-Ts) 開機自啟登錄還原：未還原 $($regLeft.Count) 項$(if ($regLeft.Count) { '：' + ($regLeft -join '; ') })")
    $results['開機自啟登錄已還原（Run／StartupApproved 的 fc-host）'] = ($regLeft.Count -eq 0)
    $log.WriteLine("# $(Get-Ts) 宿主已結束")
    $log.Close()
}

$sum = New-EvidenceWriter $sumPath
$sum.WriteLine("# verify-datadir-empty.ps1 summary $(Get-Ts)")
foreach ($k in $results.Keys) { $sum.WriteLine("$(if ($results[$k]) { 'PASS' } else { 'FAIL' })  $k") }
$sum.Close()
Get-Content $sumPath
Remove-Item -Recurse -Force $tempRoot -ErrorAction SilentlyContinue

$failed = @($results.Values | Where-Object { -not $_ })
if ($failed.Count -gt 0) { exit 1 }
exit 0
