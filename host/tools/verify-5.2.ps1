<#
.SYNOPSIS
    Task 5.2 驗收驅動腳本（設定視窗 settings.html；design.md D4／D10／D12；
    specs/widget-host-lifecycle「設定持久化」；specs/widget-host-windows「小工具外觀模式」；
    specs/widget-data-feed「資料目錄」；specs/finance-widgets「顯示除權息」）：驗證改值後
    所有小工具即時套用（透明度、主題色、顯示除權息、小工具開關）、資料目錄改變後 60 秒內生效、
    重啟後設定保留。只讀視窗狀態、透過 CDP 對 settings 視窗操作表單 DOM／讀各小工具頁面 DOM，
    **不注入任何輸入**（不送按鍵、不點擊、不截圖），鎖定時也可跑。

.DESCRIPTION
    步驟：
      1. 確認沒有 fc-host 在跑。
      2. 以暫存目錄同時當 %APPDATA%（=> 首次啟動）與 %LOCALAPPDATA%（=> 預設資料目錄），
         另備一個「新資料目錄」放入 tw_events.json 樣本，供步驟 6 的資料目錄切換測試使用——
         完全不碰使用者真正的設定檔或 D:\finance-calendar。
      3. 啟動第一個執行個體（CDP 除錯埠），等五個財經小工具就緒。
      4. 以「手動重複啟動」（不帶參數、等它結束）觸發既有執行個體開啟設定視窗（比照
         verify-5.1.ps1 的作法，不是輸入注入，是另一個行程的正常結束）。
      5. 在 settings 頁面讀初始表單值，確認與 `get_settings()` 一致（settings.html 有正確
         回填表單）。
      6. 逐項操作表單 DOM（設值＋dispatch input/change 事件，即真正的使用者互動路徑，不是繞過
         表單直接呼叫 invoke）：
           - 透明度滑桿 → 各小工具頁面 `--panel-o` 立即改變
           - 主題色 → 各小工具頁面 `--accent` 立即改變
           - 顯示除權息 → dynamic 頁面套用（該小工具既有邏輯，非本 task 新增，僅觀察）
           - 開啟 custom1 → 視窗立即建立
           - 資料目錄改成「新資料目錄」→ 60 秒內 clock 頁面 `get_snapshot('tw-events')`
             讀到新目錄內容
      7. 結束宿主，直接讀暫存 settings.json 確認上述改動都已落地。
      8. 重新啟動宿主（同一份暫存 %APPDATA%），再次手動重複啟動開設定視窗，確認表單顯示的是
         重啟前的值（不是預設值）——「重啟後保留」不只驗證 settings.json 內容，也驗證
         settings.html 讀得回來。
      9. 結束宿主、刪除暫存目錄。

.PARAMETER Exe
    fc-host.exe 路徑，預設 host/target/release/fc-host.exe。

.PARAMETER OutDir
    證據輸出目錄，預設 host/tools/evidence。

.PARAMETER CdpPort
    WebView2 remote debugging 埠，預設 9339（與既有 verify-3.1/3.2/4.6/4.7/5.8 的
    9333/9334/9336/9337/9338 分開）。
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

Add-Type -Namespace V52 -Name Native -MemberDefinition @'
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
    $h = [V52.Native]::GetTopWindow([IntPtr]::Zero)
    while ($h -ne [IntPtr]::Zero) {
        $p = 0
        [void][V52.Native]::GetWindowThreadProcessId($h, [ref]$p)
        if ($p -eq $ProcId -and [V52.Native]::IsWindowVisible($h)) {
            $sb = New-Object System.Text.StringBuilder 256
            [void][V52.Native]::GetWindowText($h, $sb, 256)
            if ($sb.Length -gt 0) { $titles += $sb.ToString() }
        }
        $h = [V52.Native]::GetWindow($h, 2)
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

# 原生 HWND 標題出現（Wait-WindowTitles）不代表該視窗的 WebView2 已在 CDP 除錯埠註冊完成——
# 兩者是非同步的兩件事，且實測這個註冊延遲不穩定（同一支腳本連續跑，有時 <1 秒、有時
# 20 秒以上才出現在 `/json/list`，settings 視窗是手動重複啟動後才動態建立，比啟動時就存在的
# 小工具視窗更容易撞到這個窗口）。`Invoke-PageEval` 因此對 host-cdp-eval.mjs 的「找不到 url
# 含…的分頁」這個特定失敗訊息內建重試（輪詢到逾時），其餘錯誤（頁面本身的 JS 例外等）不重試、
# 原樣回傳讓呼叫端判斷。
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

function Invoke-SettingsEval([string]$Expr) {
    return Invoke-PageEval 'settings.html' $Expr
}

# 設定頁 main() 是非同步的：頁面提交後先用 HTML 預設值建好表單（透明度 1、#000000、資料目錄空白），
# get_settings() 回來、renderForm() 跑過才是真值（2026-10-06 實測兩者相隔約 50 ms）。讀表單前先等
# 資料目錄欄位非空——設定的 data_dir 恆非空（預設是 %LOCALAPPDATA% 下的路徑），HTML 預設值是空白。
function Wait-SettingsFormLoaded {
    $r = Invoke-SettingsEval "new Promise(r => { const t0 = Date.now(); (function w() { const d = document.getElementById('data-dir'); const ok = !!(d && d.value); if (ok || Date.now() - t0 > 8000) r(String(ok)); else setTimeout(w, 200); })(); })"
    return ($r -match 'true')
}

function Invoke-WidgetEval([string]$WidgetId, [string]$Expr) {
    return Invoke-PageEval "w=$WidgetId" $Expr
}

# ── 1. 前置檢查 ─────────────────────────────────────────────────────────────────
if (Get-Process -Name fc-host -ErrorAction SilentlyContinue) {
    throw '已有 fc-host 在執行，請先結束（WebView2 共用 user data folder，CDP 參數不會生效）。'
}
$Exe = (Resolve-Path $Exe).Path
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$logPath = Join-Path $OutDir '5.2-log.log'
$sumPath = Join-Path $OutDir '5.2-summary.log'
$log = New-EvidenceWriter $logPath
$log.AutoFlush = $true
$log.WriteLine("# verify-5.2.ps1 start=$(Get-Ts) exe=$Exe locked=$([int](Test-Locked))")

# ── 2. 暫存 %APPDATA%／%LOCALAPPDATA%＋新資料目錄樣本 ───────────────────────────────
$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ("fc-host-5.2-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
$tempAppData = Join-Path $tempRoot 'Roaming'
$tempLocalAppData = Join-Path $tempRoot 'Local'
$newDataDir = Join-Path $tempRoot 'new-data-dir'
New-Item -ItemType Directory -Force -Path $tempAppData, $tempLocalAppData, $newDataDir | Out-Null
# validate_tw_events（host/src/data.rs，移植自 Lively 版 isEmptyPayload）要求
# macro/events/punish/quotes 四個頂層陣列合計長度 > 0，這裡用 quotes 帶一筆；fetched／
# updated 依真實資料層格式是字串（非 epoch 數字，host/src/data.rs extract_top_level_string
# 只認 Value::as_str），塞一個好比對的哨兵字串，供下面輪詢比對「是不是這份新資料」。
$sentinel = '5-2-VERIFY-SENTINEL'
$sampleEvents = "{`"fetched`": `"$sentinel`", `"updated`": `"$sentinel`", `"macro`": [], `"events`": [], `"punish`": [], `"quotes`": [{`"name`": `"test`", `"value`": 1}]}"
Set-Content -Path (Join-Path $newDataDir 'tw_events.json') -Value $sampleEvents -Encoding UTF8 -NoNewline
$settingsJsonPath = Join-Path $tempAppData 'tw.fintools.fc-host\settings.json'
$log.WriteLine("# APPDATA=$tempAppData（無 settings.json＝首次啟動）LOCALAPPDATA=$tempLocalAppData 新資料目錄=$newDataDir（已放入 tw_events.json 樣本）")

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

function Open-SettingsWindowViaManualRelaunch([int]$ProcId) {
    # 手動重複啟動（無參數）：既有執行個體收到 single-instance 回呼會開啟設定視窗
    # （specs/widget-host-lifecycle「單一執行個體」；不是輸入注入，是另一個行程正常結束，
    # 比照 verify-5.1.ps1）。第二個行程理論上很快就會自行結束（single-instance 只是轉發後
    # exit(0)）；仿照 verify-5.1.ps1 `Invoke-SecondInstance`，逾時未結束就強制關閉，避免留下
    # 一個懸置行程干擾後續判斷（例如佔用 CDP 除錯埠、或被誤判成另一個獨立執行個體）。
    $env:APPDATA = $script:tempAppData
    $env:LOCALAPPDATA = $script:tempLocalAppData
    $p2 = $null
    try {
        $p2 = Start-Process -FilePath $script:Exe -PassThru
    } finally {
        $env:APPDATA = $script:oldAppData
        $env:LOCALAPPDATA = $script:oldLocalAppData
    }
    $exited = $p2.WaitForExit(15000)
    if (-not $exited) {
        $script:log.WriteLine("## $(Get-Ts) 警告：第二個行程（pid=$($p2.Id)）15 秒內未自行結束，強制關閉")
        Stop-Process -Id $p2.Id -Force -ErrorAction SilentlyContinue
    }
    $opened = Wait-WindowTitles $ProcId { param($t) ($t | Where-Object { $_ -eq '財經桌布設定' }) } 15
    $fcHostCount = @(Get-Process -Name fc-host -ErrorAction SilentlyContinue).Count
    $script:log.WriteLine("## $(Get-Ts) 手動重複啟動：第二個行程自行結束＝$exited，目前 fc-host 行程數＝$fcHostCount（應恰為 1）")
    return $opened
}

try {
    # ── 3. 啟動第一個執行個體 ─────────────────────────────────────────────────────
    $hostProc = Start-Host
    $hostPid = $hostProc.Id
    $log.WriteLine("# $(Get-Ts) 宿主 pid=$hostPid")

    $ok = Wait-WindowTitles $hostPid { param($t) ($t | Where-Object { $_ -eq 'fc-host clock' }) }
    $log.WriteLine("## $(Get-Ts) 首次啟動：clock 就緒＝$ok（視窗清單：$((Get-HostWindowTitles $hostPid) -join ', '))")
    $results['首次啟動成功（clock 視窗出現）'] = $ok

    # ── 4. 手動重複啟動開設定視窗 ──────────────────────────────────────────────────
    $settingsOpened = Open-SettingsWindowViaManualRelaunch $hostPid
    $log.WriteLine("## $(Get-Ts) 設定視窗已開啟＝$settingsOpened（視窗清單：$((Get-HostWindowTitles $hostPid) -join ', '))")
    $results['手動重複啟動開啟設定視窗'] = $settingsOpened

    # ── 5. 初始表單值與 get_settings() 一致（Invoke-SettingsEval 內建重試，見該函式註解：
    #     CDP page target 註冊時機不穩定，此處不用另外等待；表單回填要另外等，見 Wait-SettingsFormLoaded）
    $formLoaded = Wait-SettingsFormLoaded
    $log.WriteLine("## $(Get-Ts) 設定表單已載入設定＝$formLoaded")
    $initExpr = "window.__TAURI__.core.invoke('get_settings').then(s => ({ opacity: s.opacity, accent: s.accent_color, dividend: s.show_dividend, dataDir: s.data_dir, clockEnabled: s.widgets.clock.enabled, custom1Enabled: s.widgets.custom1.enabled, autostart: s.autostart }))"
    $initSettings = (Invoke-SettingsEval $initExpr) | ConvertFrom-Json
    $formExpr = "(() => ({ opacity: document.getElementById('opacity-range').value, accent: document.getElementById('accent-color').value, dividend: document.getElementById('show-dividend').checked, dataDir: document.getElementById('data-dir').value, clockEnabled: document.getElementById('widget-clock-enabled').checked, custom1Enabled: document.getElementById('widget-custom1-enabled').checked, autostart: document.getElementById('autostart').checked }))()"
    $initForm = (Invoke-SettingsEval $formExpr) | ConvertFrom-Json
    $log.WriteLine("## $(Get-Ts) get_settings=$($initSettings | ConvertTo-Json -Compress) 表單=$($initForm | ConvertTo-Json -Compress)")
    $formMatchesBackend = ([double]$initForm.opacity -eq [double]$initSettings.opacity) -and
                           ($initForm.accent -eq $initSettings.accent) -and
                           ($initForm.dividend -eq $initSettings.dividend) -and
                           ($initForm.clockEnabled -eq $initSettings.clockEnabled) -and
                           ($initForm.custom1Enabled -eq $initSettings.custom1Enabled) -and
                           ($initForm.autostart -eq $initSettings.autostart)
    $results['初始表單值與 get_settings() 一致'] = $formMatchesBackend

    # task 7.2 起個別「縮放」欄位已移除（倍率改由格子寬度決定，design.md D7），task 7.7 把原本
    # 「縮放提示文字＝立即套用」的檢查改成：設定頁看得到的文字不再提到縮放。
    $scaleText = Invoke-SettingsEval "document.body.innerText.includes('縮放')"
    $log.WriteLine("## $(Get-Ts) 設定頁可見文字含「縮放」：$scaleText")
    $results['設定頁已沒有縮放欄位與縮放說明（task 7.2 移除）'] = ($scaleText -match 'false')

    # 設定改動到小工具頁面套用要走「JS debounce → invoke → Rust 合併存檔 → emit → 各視窗
    # onSettings handler」一整輪 IPC，單次固定 sleep 撞過幾次時機不夠（尤其是這一輪剛開完
    # 設定視窗、系統忙著處理其他 IPC 的時候）；改成輪詢到符合或逾時，比固定 sleep 穩定。
    function Wait-CssVarEquals([string]$WidgetId, [string]$VarName, [string]$Expected, [int]$TimeoutSec = 8) {
        $sw = [Diagnostics.Stopwatch]::StartNew()
        $last = $null
        while ($sw.Elapsed.TotalSeconds -lt $TimeoutSec) {
            $last = Invoke-WidgetEval $WidgetId "getComputedStyle(document.documentElement).getPropertyValue('$VarName').trim()"
            if ($last -match [regex]::Escape("`"$Expected`"")) { return $last }
            Start-Sleep -Milliseconds 300
        }
        return $last
    }

    # ── 6a. 透明度滑桿 → 各小工具 --panel-o 立即改變 ──────────────────────────────
    # dynamic／quotes 納入斷言（task 5.2 後續：這兩個小工具的 applyAppearance 掛鉤原本由
    # 另一個持有 dynamic.js／quotes.js 的 subagent 進行中的 4.4/4.5 fix 佔用，該 subagent
    # 收工後才補上，見 task-5.2-report.md）。quotes 預設開啟（Settings::default()），此時
    # 應該已經跟 clock/macro/fixed/dynamic 一起在首次啟動時建立好，可直接查。
    $setOpacity = "(() => { const r = document.getElementById('opacity-range'); r.value = '0.8'; r.dispatchEvent(new Event('input', { bubbles: true })); return r.value; })()"
    Invoke-SettingsEval $setOpacity | Out-Null
    $clockPanelO = Wait-CssVarEquals 'clock' '--panel-o' '0.8'
    $macroPanelO = Wait-CssVarEquals 'macro' '--panel-o' '0.8'
    $dynamicPanelO = Wait-CssVarEquals 'dynamic' '--panel-o' '0.8'
    $quotesPanelO = Wait-CssVarEquals 'quotes' '--panel-o' '0.8'
    $log.WriteLine("## $(Get-Ts) 透明度→0.8 後：clock --panel-o=$clockPanelO macro --panel-o=$macroPanelO dynamic --panel-o=$dynamicPanelO quotes --panel-o=$quotesPanelO")
    $results['透明度改為 0.8 → clock/macro 的 --panel-o 立即變成 0.8'] = ($clockPanelO -match '"0\.8"') -and ($macroPanelO -match '"0\.8"')
    $results['透明度改為 0.8 → dynamic/quotes 的 --panel-o 立即變成 0.8'] = ($dynamicPanelO -match '"0\.8"') -and ($quotesPanelO -match '"0\.8"')

    # ── 6b. 主題色 → 各小工具 --accent 立即改變 ────────────────────────────────────
    $setAccent = "(() => { const i = document.getElementById('accent-color'); i.value = '#123456'; i.dispatchEvent(new Event('change', { bubbles: true })); return i.value; })()"
    Invoke-SettingsEval $setAccent | Out-Null
    $clockAccent = Wait-CssVarEquals 'clock' '--accent' '#123456'
    $fixedAccent = Wait-CssVarEquals 'fixed' '--accent' '#123456'
    $dynamicAccent = Wait-CssVarEquals 'dynamic' '--accent' '#123456'
    $quotesAccent = Wait-CssVarEquals 'quotes' '--accent' '#123456'
    $log.WriteLine("## $(Get-Ts) 主題色→#123456 後：clock --accent=$clockAccent fixed --accent=$fixedAccent dynamic --accent=$dynamicAccent quotes --accent=$quotesAccent")
    $results['主題色改為 #123456 → clock/fixed 的 --accent 立即變成 #123456'] = ($clockAccent -match '(?i)"#123456"') -and ($fixedAccent -match '(?i)"#123456"')
    $results['主題色改為 #123456 → dynamic/quotes 的 --accent 立即變成 #123456'] = ($dynamicAccent -match '(?i)"#123456"') -and ($quotesAccent -match '(?i)"#123456"')

    # ── 6c. 顯示除權息 → dynamic 頁面套用（既有邏輯，本 task 只驗證設定視窗確實觸發） ───────
    $setDividend = "(() => { const c = document.getElementById('show-dividend'); c.checked = true; c.dispatchEvent(new Event('change', { bubbles: true })); return c.checked; })()"
    Invoke-SettingsEval $setDividend | Out-Null
    Start-Sleep -Milliseconds 600
    $backendDividend = Invoke-SettingsEval "window.__TAURI__.core.invoke('get_settings').then(s => s.show_dividend)"
    $log.WriteLine("## $(Get-Ts) 顯示除權息勾選後，後端 show_dividend=$backendDividend")
    $results['勾選顯示除權息 → 後端設定立即更新為 true'] = ($backendDividend -match 'true')

    # ── 6d. 開啟 custom1 → 視窗立即建立 ───────────────────────────────────────────
    $enableCustom1 = "(() => { const c = document.getElementById('widget-custom1-enabled'); c.checked = true; c.dispatchEvent(new Event('change', { bubbles: true })); return c.checked; })()"
    Invoke-SettingsEval $enableCustom1 | Out-Null
    $ok = Wait-WindowTitles $hostPid { param($t) ($t | Where-Object { $_ -eq 'fc-host custom1' }) }
    Start-Sleep -Seconds 1
    $log.WriteLine("## $(Get-Ts) 開啟 custom1 checkbox 後，視窗出現＝$ok（視窗清單：$((Get-HostWindowTitles $hostPid) -join ', '))")
    $results['設定視窗勾選開啟 custom1 → 視窗立即建立'] = $ok

    # ── 6e. 資料目錄改成新目錄 → 60 秒內 clock 頁面讀到新目錄的 tw_events.json ────────────
    $escapedDir = $newDataDir.Replace('\', '\\')
    $setDataDir = "(() => { const i = document.getElementById('data-dir'); i.value = '$escapedDir'; i.dispatchEvent(new Event('change', { bubbles: true })); return i.value; })()"
    # task 4.1 fix round 3：記下切換前的註冊表世代，切換後 get_snapshot 的世代必須遞增
    # （empty 與 ok 回覆都帶 generation，design.md D4）。
    $genBefore = Invoke-WidgetEval 'clock' "window.__TAURI__.core.invoke('get_snapshot', { channel: 'tw-events' }).then(s => ({ status: s.status, generation: s.generation }))"
    $log.WriteLine("## $(Get-Ts) 切換資料目錄前 get_snapshot：$genBefore")
    $setResult = Invoke-SettingsEval $setDataDir
    $changeTs = Get-Date
    $log.WriteLine("## $(Get-Ts) 資料目錄改為 $newDataDir → 表單回報 $setResult")

    $picked = $false
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $lastSnap = $null
    while ($sw.Elapsed.TotalSeconds -lt 60) {
        $lastSnap = Invoke-WidgetEval 'clock' "window.__TAURI__.core.invoke('get_snapshot', { channel: 'tw-events' }).then(s => ({ status: s.status, fetched: s.meta ? s.meta.fetched : null, generation: s.generation }))"
        if ($lastSnap -match '"status":"ok"' -and $lastSnap -match $sentinel) { $picked = $true; break }
        Start-Sleep -Seconds 3
    }
    $elapsed = [math]::Round(((Get-Date) - $changeTs).TotalSeconds, 1)
    $log.WriteLine("## $(Get-Ts) 改資料目錄後 ${elapsed}s，clock 讀到的 tw-events 快照：$lastSnap")
    $results['改資料目錄後 60 秒內 tw-events 通道載入新目錄的 tw_events.json'] = $picked
    $gb = ($genBefore | ConvertFrom-Json).generation
    $ga = ($lastSnap | ConvertFrom-Json).generation
    $results["get_snapshot 帶註冊表世代，切換資料目錄後遞增（$gb → $ga）"] = ($null -ne $gb) -and ($null -ne $ga) -and ([int64]$ga -gt [int64]$gb)

    # ── 7. 讀暫存 settings.json 確認全部改動已落地 ─────────────────────────────────
    Start-Sleep -Seconds 1
    $saved = Get-Content $settingsJsonPath -Raw | ConvertFrom-Json
    $log.WriteLine("## $(Get-Ts) settings.json：opacity=$($saved.opacity) accent=$($saved.accent_color) dividend=$($saved.show_dividend) dataDir=$($saved.data_dir) custom1=$($saved.widgets.custom1.enabled)")
    $savedOk = ($saved.opacity -eq 0.8) -and
               ($saved.accent_color -eq '#123456') -and
               ($saved.show_dividend -eq $true) -and
               ($saved.widgets.custom1.enabled -eq $true) -and
               ($saved.data_dir -replace '\\', '/') -eq ($newDataDir -replace '\\', '/')
    $results['settings.json 已落地全部改動（透明度／主題色／除權息／custom1／資料目錄）'] = $savedOk

    # ── 8. 結束並重啟，確認表單讀回的是重啟前的值 ──────────────────────────────────
    # `Stop-Process -Force` 只殺 fc-host.exe 這個父行程，WebView2 的實際瀏覽器/GPU 子行程
    # （msedgewebview2.exe）不一定馬上跟著結束（實測：父行程結束後子行程可能還存活數秒）；
    # 若這些子行程仍鎖著同一份 user-data-dir（`%LOCALAPPDATA%\...\EBWebView`，本次暫存路徑
    # 底下），緊接著重啟的新行程要建立新的 WebView2 環境／視窗時可能卡住，導致之後動態建立
    # 的設定視窗遲遲不出現在 CDP 除錯埠（實測現象：手動重複啟動的第二個行程 15 秒內不結束、
    # settings.html 的 CDP page target 遲遲不出現）。用 `CommandLine` 比對本次暫存目錄的
    # GUID 資料夾名稱（唯一、不會誤殺系統其他 WebView2 應用，例如 Windows 搜尋的
    # msedgewebview2.exe 用的是完全不同的 user-data-dir）逐一清掉殘留子行程。
    Stop-Process -Id $hostPid -Force
    $tempLeaf = Split-Path $tempRoot -Leaf
    $orphans = Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" -ErrorAction SilentlyContinue |
        Where-Object { $_.CommandLine -and $_.CommandLine -match [regex]::Escape($tempLeaf) }
    foreach ($o in $orphans) {
        Stop-Process -Id $o.ProcessId -Force -ErrorAction SilentlyContinue
    }
    $log.WriteLine("# $(Get-Ts) 第一個宿主已結束，準備重啟驗證持久化（清掉 $($orphans.Count) 個殘留 msedgewebview2 子行程）")
    Start-Sleep -Seconds 2

    $hostProc = Start-Host
    $hostPid = $hostProc.Id
    $log.WriteLine("# $(Get-Ts) 重啟後宿主 pid=$hostPid")
    $ok = Wait-WindowTitles $hostPid { param($t) ($t | Where-Object { $_ -eq 'fc-host clock' }) }
    $log.WriteLine("## $(Get-Ts) 重啟後 clock 視窗就緒＝$ok（視窗清單：$((Get-HostWindowTitles $hostPid) -join ', '))")
    $results['重啟後首次啟動成功（clock 視窗出現）'] = $ok

    $settingsOpened2 = Open-SettingsWindowViaManualRelaunch $hostPid
    $log.WriteLine("## $(Get-Ts) 重啟後手動重複啟動→設定視窗已開啟＝$settingsOpened2（視窗清單：$((Get-HostWindowTitles $hostPid) -join ', '))")
    $formLoaded2 = Wait-SettingsFormLoaded
    $log.WriteLine("## $(Get-Ts) 重啟後設定表單已載入設定＝$formLoaded2")
    $reopenExpr ="(() => ({ opacity: document.getElementById('opacity-range').value, accent: document.getElementById('accent-color').value, dividend: document.getElementById('show-dividend').checked, dataDir: document.getElementById('data-dir').value, custom1Enabled: document.getElementById('widget-custom1-enabled').checked }))()"
    $reopenForm = (Invoke-SettingsEval $reopenExpr) | ConvertFrom-Json
    $log.WriteLine("## $(Get-Ts) 重啟後設定視窗開啟＝$settingsOpened2，表單=$($reopenForm | ConvertTo-Json -Compress)")
    $persisted = $settingsOpened2 -and
                 ([double]$reopenForm.opacity -eq 0.8) -and
                 ($reopenForm.accent -eq '#123456') -and
                 ($reopenForm.dividend -eq $true) -and
                 ($reopenForm.custom1Enabled -eq $true) -and
                 (($reopenForm.dataDir -replace '\\', '/') -eq ($newDataDir -replace '\\', '/'))
    $results['重啟後設定視窗表單顯示的是重啟前的值（非預設值）'] = $persisted

    $log.WriteLine("## 截圖：略過（鎖定狀態=$([int](Test-Locked))），視覺比對列入 human-checklist")
}
catch {
    # 任何一步意外失敗（例如 CDP 逾時後 ConvertFrom-Json 丟例外）不讓整支腳本直接中止、
    # 損失前面已經跑完並記錄的證據——記下例外訊息，之後仍照常寫出目前累積到的 $results，
    # 未跑到的檢查項目在摘要裡就是缺席（不是誤判為 PASS，讀摘要時要留意「缺席＝未完成」）。
    $log.WriteLine("## $(Get-Ts) 例外中止：$($_.Exception.Message)")
    $results['腳本未跑完（見 5.2-log.log 例外訊息）'] = $false
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
$sum.WriteLine("# verify-5.2.ps1 summary $(Get-Ts)")
foreach ($k in $results.Keys) { $sum.WriteLine("$(if ($results[$k]) { 'PASS' } else { 'FAIL' })  $k") }
$sum.Close()
Get-Content $sumPath
Remove-Item -Recurse -Force $tempRoot -ErrorAction SilentlyContinue

$failed = @($results.Values | Where-Object { -not $_ })
if ($failed.Count -gt 0) { exit 1 }
exit 0
