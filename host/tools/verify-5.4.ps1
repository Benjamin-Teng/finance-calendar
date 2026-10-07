<#
.SYNOPSIS
    Task 5.4 驗收驅動腳本（開機自動啟動；design.md D12；specs/widget-host-lifecycle
    「開機自動啟動」「單一執行個體」）。2026-10-01 起初次登錄與移除歸安裝檔，本腳本驗證：
    首次啟動（設定檔首次建立）不寫 HKCU Run；設定視窗的「登入時自動啟動」開關顯示登錄實況
    （設定檔預設 true、登錄沒值時顯示關閉）；只有切換開關才寫入／刪除 Run 值；其他設定的
    修改與平常啟動都不碰登錄；全程不改寫 StartupApproved；帶 `--autostart` 啟動時不開設定
    視窗。只讀視窗狀態、透過 CDP 對 settings 視窗操作表單
    DOM、直接讀登錄機碼，**不注入任何輸入**（不送按鍵、不點擊、不截圖），鎖定時也可跑。

.DESCRIPTION
    會寫入使用者真正的 HKCU\Software\Microsoft\Windows\CurrentVersion\Run\fc-host 與
    Explorer\StartupApproved\Run\fc-host（不受暫存 %APPDATA%／%LOCALAPPDATA% 影響）；腳本一開始
    以 lib\AutostartRegistry.psm1 快照兩個值，finally 區塊無論成功與否都還原（含原本不存在），
    未還原項目列入 FAIL。

    步驟：
      1. 確認沒有 fc-host 在跑、記錄登錄原值。
      2. 以暫存目錄同時當 %APPDATA%（=> 首次啟動、預設 autostart=true）與 %LOCALAPPDATA%。
      3. 先刪掉 Run 值（模擬未經安裝檔安裝的開發建置），在 StartupApproved 寫入「使用者已
         停用」旗標（模擬工作管理員停用）。啟動第一個執行個體（CDP 除錯埠），等 clock 就緒 →
         驗證首次啟動**沒有**寫 Run 值，且 StartupApproved 的停用旗標原封不動。
      4. 手動重複啟動開設定視窗：暫存 settings.json 是預設的 autostart:true，但 checkbox 與
         get_settings().autostart 都應為 false（顯示登錄實況）。
      5. 勾選 checkbox（dispatch change 事件）→ 驗證 get_settings().autostart=true 且登錄值
         ＝`"<exe>" --autostart`（路徑加引號）。
      6. 勾掉 checkbox → 驗證 get_settings().autostart=false 且登錄機碼被刪除。
      6b. 從外部把 Run 值設成哨兵字串（模擬安裝檔登錄）→ get_settings().autostart=true；再以
         update_settings 改另一個欄位（show_dividend）→ 哨兵不變（其他設定的修改不碰登錄）。
         整段 3–6b 後 StartupApproved 停用旗標仍原封不動。
      7. 結束宿主，手動刪除登錄機碼；以 `--autostart` 參數重新啟動（模擬登入時的真實呼叫
         方式）→ 驗證：(a) 平常啟動**不**寫登錄機碼；(b) 全程未出現「財經桌布設定」視窗（即使
         clock 視窗已就緒超過幾秒）。
      8. 結束宿主，把暫存 settings.json 的 autostart 改成 false、Run 值設成哨兵字串，重新啟動
         → 驗證平常啟動既不刪除也不覆寫（哨兵不變）。
      9. 結束宿主、以模組還原兩個登錄值、刪除暫存目錄。

.PARAMETER Exe
    fc-host.exe 路徑，預設 host/target/release/fc-host.exe（**不含** self-test-ipc feature，
    見 task-5.4-report.md 的建置說明）。

.PARAMETER OutDir
    證據輸出目錄，預設 host/tools/evidence。

.PARAMETER CdpPort
    WebView2 remote debugging 埠，預設 9340（與既有 verify-3.1/3.2/4.6/4.7/5.2/5.8 的
    9333/9334/9336/9337/9339/9338 分開）。
#>
[CmdletBinding()]
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\fc-host.exe'),
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [int]$CdpPort = 9340
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\AutostartRegistry.psm1') -Force

Add-Type -Namespace V54 -Name Native -MemberDefinition @'
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
    $h = [V54.Native]::GetTopWindow([IntPtr]::Zero)
    while ($h -ne [IntPtr]::Zero) {
        $p = 0
        [void][V54.Native]::GetWindowThreadProcessId($h, [ref]$p)
        if ($p -eq $ProcId -and [V54.Native]::IsWindowVisible($h)) {
            $sb = New-Object System.Text.StringBuilder 256
            [void][V54.Native]::GetWindowText($h, $sb, 256)
            if ($sb.Length -gt 0) { $titles += $sb.ToString() }
        }
        $h = [V54.Native]::GetWindow($h, 2)
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

# ── 登錄機碼讀寫（HKCU Run 的 fc-host；宿主自 fix F2 起只寫這個值）＋ StartupApproved ───────
$RunKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$ApprovedKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run'
$ValueName = 'fc-host'
# 工作管理員「停用」的旗標：首位 03、其後 8 位元組非零（auto-launch 判斷「最後 8 位元組全為 0」
# 才算啟用）。
$UserDisabledFlag = [byte[]](3, 0, 0, 0, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x08)
$Sentinel = 'fc-host-5.4-sentinel'

function Get-ApprovedRegHex {
    $v = Get-ItemProperty -Path $ApprovedKey -Name $ValueName -ErrorAction SilentlyContinue
    if ($null -eq $v) { return '<不存在>' }
    return (($v.$ValueName | ForEach-Object { $_.ToString('X2') }) -join ' ')
}
$UserDisabledHex = ($UserDisabledFlag | ForEach-Object { $_.ToString('X2') }) -join ' '

function Get-AutostartRegValue {
    $v = Get-ItemProperty -Path $RunKey -Name $ValueName -ErrorAction SilentlyContinue
    if ($null -eq $v) { return $null }
    return $v.$ValueName
}

# ── 1. 前置檢查：沒有 fc-host 在跑、記錄登錄原值 ────────────────────────────────────
if (Get-Process -Name fc-host -ErrorAction SilentlyContinue) {
    throw '已有 fc-host 在執行，請先結束（WebView2 共用 user data folder，CDP 參數不會生效）。'
}
$Exe = (Resolve-Path $Exe).Path
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$logPath = Join-Path $OutDir '5.4-log.log'
$sumPath = Join-Path $OutDir '5.4-summary.log'
$log = New-EvidenceWriter $logPath
$log.AutoFlush = $true
$log.WriteLine("# verify-5.4.ps1 start=$(Get-Ts) exe=$Exe locked=$([int](Test-Locked))")

$regSnap = @(Save-FcHostAutostartRegistry)
$log.WriteLine("# 登錄快照（還原用）：$(($regSnap | ForEach-Object { "$($_.Key)\$($_.Name) exists=$($_.Exists)" }) -join '；')")

$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ("fc-host-5.4-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
$tempAppData = Join-Path $tempRoot 'Roaming'
$tempLocalAppData = Join-Path $tempRoot 'Local'
New-Item -ItemType Directory -Force -Path $tempAppData, $tempLocalAppData | Out-Null
$settingsJsonPath = Join-Path $tempAppData 'tw.fintools.fc-host\settings.json'
$log.WriteLine("# APPDATA=$tempAppData（無 settings.json＝首次啟動）LOCALAPPDATA=$tempLocalAppData")

$results = [ordered]@{}
$hostProc = $null
$oldAppData = $env:APPDATA
$oldLocalAppData = $env:LOCALAPPDATA
$oldWv2 = $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS

function Start-Host([string[]]$ExtraArgs = @()) {
    $p = $null
    try {
        $env:APPDATA = $script:tempAppData
        $env:LOCALAPPDATA = $script:tempLocalAppData
        $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$script:CdpPort"
        if ($ExtraArgs.Count -gt 0) {
            $p = Start-Process -FilePath $script:Exe -ArgumentList $ExtraArgs -PassThru
        } else {
            $p = Start-Process -FilePath $script:Exe -PassThru
        }
    } finally {
        $env:APPDATA = $script:oldAppData
        $env:LOCALAPPDATA = $script:oldLocalAppData
        $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $script:oldWv2
    }
    return $p
}

function Open-SettingsWindowViaManualRelaunch([int]$ProcId) {
    # 手動重複啟動（無參數）：既有執行個體收到 single-instance 回呼會開啟設定視窗（不是輸入
    # 注入，是另一個行程正常結束，比照 verify-5.1.ps1／verify-5.2.ps1）。
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

function Wait-CheckboxState([string]$Id, [string]$Expected, [int]$TimeoutSec = 8) {
    # settings.html 的 main() 非同步：頁面剛在 CDP 除錯埠註冊時，checkbox 可能還是 el() 建立時
    # 的預設值（未勾選），要等 get_settings() 回來、renderForm() 跑過才是真值——輪詢到符合或
    # 逾時，不能只讀一次。
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $last = $null
    while ($sw.Elapsed.TotalSeconds -lt $TimeoutSec) {
        $last = Invoke-SettingsEval "document.getElementById('$Id').checked"
        if ($last -match $Expected) { return $last }
        Start-Sleep -Milliseconds 300
    }
    return $last
}

function Stop-HostAndOrphans([int]$ProcId) {
    Stop-Process -Id $ProcId -Force -ErrorAction SilentlyContinue
    $tempLeaf = Split-Path $script:tempRoot -Leaf
    $orphans = Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" -ErrorAction SilentlyContinue |
        Where-Object { $_.CommandLine -and $_.CommandLine -match [regex]::Escape($tempLeaf) }
    foreach ($o in $orphans) { Stop-Process -Id $o.ProcessId -Force -ErrorAction SilentlyContinue }
    Start-Sleep -Seconds 2
}

try {
    # ── 2/3. 模擬未經安裝檔安裝（Run 值不存在）、使用者在工作管理員停用；啟動第一個執行個體
    #        （首次啟動，設定檔預設 autostart=true）
    Remove-ItemProperty -Path $RunKey -Name $ValueName -ErrorAction SilentlyContinue
    if (-not (Test-Path $ApprovedKey)) { New-Item -Path $ApprovedKey -Force | Out-Null }
    New-ItemProperty -Path $ApprovedKey -Name $ValueName -Value $UserDisabledFlag -PropertyType Binary -Force | Out-Null
    $log.WriteLine("# $(Get-Ts) 啟動前 Run 值＝$(Get-AutostartRegValue)（應不存在）；StartupApproved 寫入使用者停用旗標：$(Get-ApprovedRegHex)")
    $hostProc = Start-Host
    $hostPid = $hostProc.Id
    $log.WriteLine("# $(Get-Ts) 宿主 pid=$hostPid")

    $clockReady = Wait-WindowTitles $hostPid { param($t) ($t | Where-Object { $_ -eq 'fc-host clock' }) }
    $log.WriteLine("## $(Get-Ts) 首次啟動：clock 就緒＝$clockReady")
    $results['首次啟動成功（clock 視窗出現）'] = $clockReady

    Start-Sleep -Milliseconds 500
    $regAfterFirstStart = Get-AutostartRegValue
    $log.WriteLine("## $(Get-Ts) 首次啟動後登錄值＝$(if ($null -eq $regAfterFirstStart) { '<不存在>' } else { $regAfterFirstStart })")
    $results['首次啟動（設定檔首次建立）→ 不寫 Run 值'] = ($null -eq $regAfterFirstStart)
    $results['首次啟動已建立設定檔且 autostart=true（預設值）'] = (Test-Path $settingsJsonPath) -and ((Get-Content $settingsJsonPath -Raw | ConvertFrom-Json).autostart -eq $true)
    $approvedAfterFirst = Get-ApprovedRegHex
    $log.WriteLine("## $(Get-Ts) 首次啟動後 StartupApproved＝$approvedAfterFirst")
    $results['首次啟動不改寫 StartupApproved（使用者停用旗標仍在）'] = ($approvedAfterFirst -eq $UserDisabledHex)

    # ── 4. 手動重複啟動開設定視窗：開關顯示登錄實況（設定檔 true、登錄沒值 → 關閉）──────
    $settingsOpened = Open-SettingsWindowViaManualRelaunch $hostPid
    $log.WriteLine("## $(Get-Ts) 設定視窗已開啟＝$settingsOpened")
    $results['手動重複啟動開啟設定視窗'] = $settingsOpened

    # 預期值是 false，而 checkbox 在 get_settings() 回來前本來就是未勾選——等到 renderForm()
    # 跑過（資料目錄欄位填入非空值）再讀，才不會把「還沒渲染」誤判成 PASS。
    $rendered = Invoke-SettingsEval "new Promise(r => { const t0 = Date.now(); (function w() { const d = document.getElementById('data-dir'); const ok = !!(d && d.value); if (ok || Date.now() - t0 > 8000) r(String(ok)); else setTimeout(w, 200); })(); })"
    $initChecked = Invoke-SettingsEval "document.getElementById('autostart').checked"
    $backendInit = Invoke-SettingsEval "window.__TAURI__.core.invoke('get_settings').then(s => s.autostart)"
    $log.WriteLine("## $(Get-Ts) 初始（表單已渲染＝$rendered）：checkbox＝$initChecked get_settings().autostart=$backendInit")
    $results['初始 checkbox 未勾選（登錄沒有值，不照設定檔的 true 顯示）'] = ($rendered -match 'true') -and ($initChecked -match 'false')
    $results['初始 get_settings().autostart=false（登錄實況）'] = ($backendInit -match 'false')

    # ── 5. 勾選 → 寫入 Run 值 ───────────────────────────────────────────────────────────
    $recheck = "(() => { const c = document.getElementById('autostart'); c.checked = true; c.dispatchEvent(new Event('change', { bubbles: true })); return c.checked; })()"
    Invoke-SettingsEval $recheck | Out-Null
    Start-Sleep -Milliseconds 800
    $backendAfterOn = Invoke-SettingsEval "window.__TAURI__.core.invoke('get_settings').then(s => s.autostart)"
    $regAfterOn = Get-AutostartRegValue
    $log.WriteLine("## $(Get-Ts) 勾選後：get_settings().autostart=$backendAfterOn 登錄值＝$regAfterOn")
    $results['勾選 checkbox → get_settings().autostart=true'] = ($backendAfterOn -match 'true')
    $results['勾選 checkbox → 登錄值＝"<exe>" --autostart（路徑加引號）'] = ($regAfterOn -eq ('"' + $Exe + '" --autostart'))

    # ── 6. 勾掉 → 刪除 Run 值 ───────────────────────────────────────────────────────────
    $uncheck = "(() => { const c = document.getElementById('autostart'); c.checked = false; c.dispatchEvent(new Event('change', { bubbles: true })); return c.checked; })()"
    Invoke-SettingsEval $uncheck | Out-Null
    Start-Sleep -Milliseconds 800
    $backendAfterOff = Invoke-SettingsEval "window.__TAURI__.core.invoke('get_settings').then(s => s.autostart)"
    $regAfterOff = Get-AutostartRegValue
    $log.WriteLine("## $(Get-Ts) 勾掉後：get_settings().autostart=$backendAfterOff 登錄值＝$(if ($null -eq $regAfterOff) { '<不存在>' } else { $regAfterOff })")
    $results['勾掉 checkbox → get_settings().autostart=false'] = ($backendAfterOff -match 'false')
    $results['勾掉 checkbox → 登錄機碼被刪除'] = ($null -eq $regAfterOff)

    # ── 6b. 外部登錄（模擬安裝檔）反映到 get_settings；其他欄位的修改不碰登錄；StartupApproved 不變
    Set-ItemProperty -Path $RunKey -Name $ValueName -Value $Sentinel
    $backendAfterExternal = Invoke-SettingsEval "window.__TAURI__.core.invoke('get_settings').then(s => s.autostart)"
    $log.WriteLine("## $(Get-Ts) 外部寫入哨兵 Run 值後 get_settings().autostart=$backendAfterExternal")
    $results['外部登錄 Run 值（模擬安裝檔）→ get_settings().autostart=true'] = ($backendAfterExternal -match 'true')
    $otherPatch = Invoke-SettingsEval "window.__TAURI__.core.invoke('get_settings').then(s => window.__TAURI__.core.invoke('update_settings', { patch: { show_dividend: !s.show_dividend } })).then(s => 'ok ' + s.show_dividend)"
    Start-Sleep -Milliseconds 800
    $regAfterOther = Get-AutostartRegValue
    $log.WriteLine("## $(Get-Ts) 改 show_dividend（$otherPatch）後登錄值＝$regAfterOther（應仍是哨兵）")
    $results['改其他設定（show_dividend）→ 不碰登錄（哨兵不變）'] = ($otherPatch -match 'ok') -and ($regAfterOther -eq $Sentinel)
    $approvedAfterToggle = Get-ApprovedRegHex
    $log.WriteLine("## $(Get-Ts) 勾選／勾掉／改其他設定後 StartupApproved＝$approvedAfterToggle")
    $results['設定切換全程不改寫 StartupApproved'] = ($approvedAfterToggle -eq $UserDisabledHex)

    # ── 7. 結束、模擬外部把登錄機碼清掉、以 --autostart 重啟 → 對帳＋不開設定視窗 ───────────
    Stop-HostAndOrphans $hostPid
    Remove-ItemProperty -Path $RunKey -Name $ValueName -ErrorAction SilentlyContinue
    $log.WriteLine("# $(Get-Ts) 第一個宿主已結束，手動刪除登錄機碼模擬外部漂移；登錄值現在＝$(Get-AutostartRegValue)")

    $savedBeforeRestart = Get-Content $settingsJsonPath -Raw | ConvertFrom-Json
    # 6b 合併時以登錄實況（哨兵存在＝true）為基準存檔，故此時設定檔是 true、登錄沒值——下面驗證
    # 啟動不會因此把登錄「補」回去。
    $log.WriteLine("## $(Get-Ts) 暫存 settings.json 的 autostart=$($savedBeforeRestart.autostart)（應為 true）")
    $results['暫存 settings.json 為 autostart=true（與登錄不一致的前提）'] = ($savedBeforeRestart.autostart -eq $true)

    $hostProc = Start-Host -ExtraArgs @('--autostart')
    $hostPid = $hostProc.Id
    $log.WriteLine("# $(Get-Ts) 以 --autostart 重啟宿主 pid=$hostPid")
    $clockReady2 = Wait-WindowTitles $hostPid { param($t) ($t | Where-Object { $_ -eq 'fc-host clock' }) }
    Start-Sleep -Seconds 3
    $titlesAfterAutostart = Get-HostWindowTitles $hostPid
    $settingsAppearedOnAutostart = [bool]($titlesAfterAutostart | Where-Object { $_ -eq '財經桌布設定' })
    $log.WriteLine("## $(Get-Ts) --autostart 啟動：clock 就緒＝$clockReady2，3 秒後視窗清單＝$($titlesAfterAutostart -join ', ')")
    $results['--autostart 啟動：clock 出現'] = $clockReady2
    $results['--autostart 啟動：未跳出設定視窗'] = (-not $settingsAppearedOnAutostart)

    $regAfterAutostartLaunch = Get-AutostartRegValue
    $log.WriteLine("## $(Get-Ts) --autostart 啟動後（登錄機碼先前已被清掉）登錄值＝$regAfterAutostartLaunch")
    $results['--autostart 平常啟動不寫回登錄機碼（不碰登錄）'] = ($null -eq $regAfterAutostartLaunch)

    # ── 8. autostart=false 時，平常啟動既不刪除也不覆寫登錄 ──────────────────────────────
    Stop-HostAndOrphans $hostPid
    Set-ItemProperty -Path $RunKey -Name $ValueName -Value $Sentinel
    $settingsJson = Get-Content $settingsJsonPath -Raw | ConvertFrom-Json
    $settingsJson.autostart = $false
    ($settingsJson | ConvertTo-Json -Depth 10) | Set-Content -Path $settingsJsonPath -Encoding UTF8
    $log.WriteLine("# $(Get-Ts) 暫存 settings.json 改成 autostart=false，登錄值設為哨兵，重啟驗證平常啟動不碰登錄")

    $hostProc = Start-Host
    $hostPid = $hostProc.Id
    $clockReady3 = Wait-WindowTitles $hostPid { param($t) ($t | Where-Object { $_ -eq 'fc-host clock' }) }
    Start-Sleep -Milliseconds 800
    $regWithAutostartFalse = Get-AutostartRegValue
    $log.WriteLine("## $(Get-Ts) autostart=false 重啟：clock 就緒＝$clockReady3，登錄值＝$regWithAutostartFalse（應仍是哨兵）")
    $results['autostart=false 時啟動：clock 出現'] = $clockReady3
    $results['autostart=false 平常啟動：不刪除也不覆寫登錄（哨兵不變）'] = ($regWithAutostartFalse -eq $Sentinel)

    $log.WriteLine("## 截圖：略過（鎖定狀態=$([int](Test-Locked))），視覺比對不適用本 task")
}
catch {
    $log.WriteLine("## $(Get-Ts) 例外中止：$($_.Exception.Message)")
    $results['腳本未跑完（見 5.4-log.log 例外訊息）'] = $false
}
finally {
    if ($hostProc -and -not $hostProc.HasExited) { Stop-Process -Id $hostProc.Id -Force -ErrorAction SilentlyContinue }
    Start-Sleep -Milliseconds 500

    # ── 9. 還原登錄（Run 與 StartupApproved 的 fc-host，含原本不存在）──────────────────
    $regLeft = @(Restore-FcHostAutostartRegistry $regSnap)
    $log.WriteLine("# $(Get-Ts) 還原登錄：未還原 $($regLeft.Count) 項 $($regLeft -join '；')；還原後 Run＝$(Get-AutostartRegValue)／StartupApproved＝$(Get-ApprovedRegHex)")
    if ($regLeft.Count -gt 0) { Write-Warning "開機自啟登錄未還原：$($regLeft -join '; ')" }
    $results['開機自啟登錄已還原（Run／StartupApproved 的 fc-host）'] = ($regLeft.Count -eq 0)

    $log.WriteLine("# $(Get-Ts) 宿主已結束")
    $log.Close()
}

$sum = New-EvidenceWriter $sumPath
$sum.WriteLine("# verify-5.4.ps1 summary $(Get-Ts)")
$sum.WriteLine("# 登錄還原：快照 $(($regSnap | ForEach-Object { "$($_.Name)@$(Split-Path $_.Key -Leaf) exists=$($_.Exists)" }) -join '，') / 還原後 Run=$(Get-AutostartRegValue) StartupApproved=$(Get-ApprovedRegHex)")
foreach ($k in $results.Keys) { $sum.WriteLine("$(if ($results[$k]) { 'PASS' } else { 'FAIL' })  $k") }
$sum.Close()
Get-Content $sumPath
Remove-Item -Recurse -Force $tempRoot -ErrorAction SilentlyContinue

$failed = @($results.Values | Where-Object { -not $_ })
if ($failed.Count -gt 0) { exit 1 }
exit 0
