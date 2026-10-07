<#
.SYNOPSIS
    Task 3.5 驗收驅動腳本（多螢幕重新定位：WM_DISPLAYCHANGE／WM_SETTINGCHANGE(SPI_SETWORKAREA)
    觸發全部小工具重新排版，design.md D9）。

.DESCRIPTION
    本機無法自動化「真的改縮放比例／改解析度／拔除外接螢幕／接回外接螢幕」這四種情況
    （global-constraints 第 18 條與 task-3.5-brief：需要人）——這四項已列入
    .superpowers/sdd/tasks/human-checklist.md「Task 3.5」節，待人工驗收。

    本腳本非侵入地（PostMessage，不是輸入注入；行前與行間都會記錄鎖定狀態，鎖定時
    PostMessage 本身不是危險操作，不會打進登入畫面）驗證「訊息 → 重新列舉顯示器 → 重算全部
    小工具矩形 → 套用」這條路徑本身：

      1. 送 WM_DISPLAYCHANGE／WM_SETTINGCHANGE(SPI_SETWORKAREA) 給守門視窗（隱藏頂層視窗，
         desktop.rs `spawn_gatekeeper`），確認 gatekeeper.log 出現對應的 EVENT／RELAYOUT 記錄
         （重排路徑被觸發，見 desktop.rs `gatekeeper_subclass_proc` 「task 3.5」分支）。
      2. 確認觸發後每個**可見**小工具的視窗矩形，與觸發前完全相同——本次並未真的改變顯示器
         組態，`layout::resolve_grid_placements`（task 7.1／7.2 起的格線推導純函式）用「不變的
         顯示器清單＋不變的格座標記錄」算出的矩形理應與觸發前一致；驗證的是
         `widgets::relayout_all_widgets` 這條路徑本身不會在「什麼都沒變」時意外把小工具搬走、
         改錯尺寸或算出無效矩形，不是「顯示器真的變了會怎樣」（那四種情況要看
         human-checklist，layout.rs 的 `grid_tests` 單元測試已涵蓋純函式那一半）。
      3. 確認隱藏中的小工具（fixture 下 quotes／dynamic 預期因無資料而隱藏）觸發前後仍隱藏
         （tao VISIBLE 旗標不變量：`relayout_all_widgets` 不得呼叫 `show_at_bottom`）。

    fix F2（review 3.5）：
      - 重排沒生效時不得 PASS：每次送訊息**之前**先用 SetWindowPos 把一扇可見小工具挪離格子
        （位移＋縮小，非輸入注入），確認真的挪動了，再送訊息，斷言它被拉回觸發前的格線矩形；
        RELAYOUT 記錄行須帶 `applied=<N>` 且 N ≥ 1。
      - 延後重排：WM_SETTINGCHANGE 是同步送達的，RELAYOUT 記錄的時間必須晚於
        SendMessageTimeout 返回的時間（重排不在同步 WndProc 內執行）。
      - 合併：連續送 WM_DISPLAYCHANGE＋WM_SETTINGCHANGE，只能多出**一行** RELAYOUT，且原因含兩者。
      - WM_DPICHANGED（改縮放）無法以訊息模擬（lParam 是跨行程指標），仍屬 human-checklist B6。

    步驟：
      1. 確認沒有 fc-host 在跑；記錄鎖定狀態（不中止——PostMessage 不是輸入注入）。
      2. 暫存 %APPDATA%／%LOCALAPPDATA% ＋ fixture（同 verify-3.3：macro 一筆、其餘空）。
      3. 啟動宿主，等待小工具視窗出現；用 EnumWindows 找出守門視窗（本行程擁有、標題不是
         `fc-host <id>` 的那扇頂層視窗——小工具視窗一律以 `.title("fc-host {id}")` 建立，見
         `widgets::create_widget_window`，守門視窗沒有呼叫 `.title(...)`，兩者可用標題區分，
         不受可見性影響）。
      4. 記錄觸發前每個可見小工具的矩形、每個隱藏小工具的隱藏狀態。
      5. PostMessage(gatekeeper, WM_DISPLAYCHANGE) → 等 gatekeeper.log 出現
         `RELAYOUT reason=display-change` → 比對矩形／隱藏狀態。
      6. SendMessageTimeout(gatekeeper, WM_SETTINGCHANGE, wParam=SPI_SETWORKAREA) → 等
         `RELAYOUT reason=workarea-change` → 比對矩形／隱藏狀態（WM_SETTINGCHANGE 不能用
         PostMessage：Win32 回傳 ERROR_MESSAGE_SYNC_ONLY，實測與原因見腳本內該步驟的註解）。
      7. 結束宿主，寫摘要。不截圖。

.PARAMETER Exe
    fc-host.exe 路徑，預設 host/target/release/fc-host.exe。

.PARAMETER OutDir
    證據輸出目錄，預設 host/tools/evidence。
#>
[CmdletBinding()]
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\fc-host.exe'),
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence')
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\AutostartRegistry.psm1') -Force

Add-Type -Namespace V35 -Name Native -MemberDefinition @'
[DllImport("user32.dll")] public static extern System.IntPtr GetTopWindow(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern System.IntPtr GetWindow(System.IntPtr hWnd, uint cmd);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr hWnd, out uint pid);
[DllImport("user32.dll", CharSet = System.Runtime.InteropServices.CharSet.Unicode)]
public static extern int GetWindowText(System.IntPtr hWnd, System.Text.StringBuilder s, int n);
[DllImport("user32.dll", CharSet = System.Runtime.InteropServices.CharSet.Unicode)]
public static extern int GetClassName(System.IntPtr hWnd, System.Text.StringBuilder s, int n);
[DllImport("user32.dll")] public static extern bool GetWindowRect(System.IntPtr hWnd, out RECT r);
[DllImport("user32.dll", SetLastError = true)] public static extern bool SetWindowPos(System.IntPtr hWnd, System.IntPtr after, int x, int y, int cx, int cy, uint flags);
[DllImport("user32.dll", SetLastError = true)] public static extern bool PostMessage(System.IntPtr hWnd, uint msg, System.UIntPtr wParam, System.IntPtr lParam);
[DllImport("user32.dll", SetLastError = true)] public static extern System.IntPtr SendMessageTimeout(System.IntPtr hWnd, uint msg, System.UIntPtr wParam, System.IntPtr lParam, uint fuFlags, uint uTimeout, out System.UIntPtr lpdwResult);
public delegate bool EnumProc(System.IntPtr h, System.IntPtr l);
[DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, System.IntPtr l);
[System.Runtime.InteropServices.StructLayout(System.Runtime.InteropServices.LayoutKind.Sequential)]
public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
'@

$WM_DISPLAYCHANGE = 0x007E
$WM_SETTINGCHANGE = 0x001A
$SPI_SETWORKAREA = [UIntPtr]47
$SMTO_ABORTIFHUNG = 0x0002

function Get-Ts { Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fffK' }
function Test-Locked { [bool](Get-Process -Name LogonUI, LockApp -ErrorAction SilentlyContinue) }
function Get-Title([IntPtr]$h) { $sb = New-Object Text.StringBuilder 256; [void][V35.Native]::GetWindowText($h, $sb, 256); $sb.ToString() }
function Get-Cls([IntPtr]$h) { $sb = New-Object Text.StringBuilder 256; [void][V35.Native]::GetClassName($h, $sb, 256); $sb.ToString() }
function Get-Rect([IntPtr]$h) {
    $r = New-Object V35.Native+RECT
    [void][V35.Native]::GetWindowRect($h, [ref]$r)
    [PSCustomObject]@{ X = $r.Left; Y = $r.Top; W = ($r.Right - $r.Left); H = ($r.Bottom - $r.Top) }
}

# 本行程（$ProcId）擁有的所有頂層視窗，不論可見與否（EnumWindows 列舉全部，不像
# GetTopWindow/GetWindow 鏈那樣只在乎 z-order；守門視窗 visible=false，必須用這個才找得到）。
function Get-AllOwnedWindows([int]$ProcId) {
    $list = New-Object System.Collections.Generic.List[IntPtr]
    $cb = [V35.Native+EnumProc] {
        param($h, $l)
        $p = 0
        [void][V35.Native]::GetWindowThreadProcessId($h, [ref]$p)
        if ($p -eq $ProcId) { $list.Add($h) }
        $true
    }
    [void][V35.Native]::EnumWindows($cb, [IntPtr]::Zero)
    return $list
}

function Get-HostWidgetWindows([int]$ProcId) {
    $result = @()
    foreach ($h in (Get-AllOwnedWindows $ProcId)) {
        if (-not [V35.Native]::IsWindowVisible($h)) { continue }
        $title = Get-Title $h
        if ($title -like 'fc-host *') { $result += [PSCustomObject]@{ Hwnd = $h; Title = $title } }
    }
    return $result
}

# 守門視窗：本行程擁有、類別是 Tauri 視窗（`class == 'Tauri Window'`，排除單一執行個體的
# mutex 訊息視窗、系統匣圖示、tao 的 Thread Event Target、輸入法等非 Tauri 視窗）、標題**不是**
# `fc-host <id>` 樣式（排除全部小工具視窗）、且目前不可見（守門視窗建立時帶
# `.visible(false)`，見腳本文件「步驟 3」）。實測（debug session）確認守門視窗的標題是 tao/wry
# 沒特別設定時的預設值 `"Tauri App"`——不直接比對這個字串（版本可能改變預設值），改用
# class＋可見性＋標題排除法，較不脆弱。
function Get-GatekeeperWindow([int]$ProcId) {
    foreach ($h in (Get-AllOwnedWindows $ProcId)) {
        if ((Get-Cls $h) -ne 'Tauri Window') { continue }
        if ([V35.Native]::IsWindowVisible($h)) { continue }
        $title = Get-Title $h
        if ($title -notlike 'fc-host *') { return [PSCustomObject]@{ Hwnd = $h; Title = $title } }
    }
    return $null
}

function Wait-Cond([scriptblock]$Cond, [int]$TimeoutSec, [int]$PollMs = 200) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt $TimeoutSec) {
        if (& $Cond) { return $true }
        Start-Sleep -Milliseconds $PollMs
    }
    return $false
}

function Get-RelayoutLines([string]$Path) {
    if (-not (Test-Path $Path)) { return @() }
    return @(Get-Content $Path | Where-Object { $_ -match 'RELAYOUT reason=' })
}

# fix F2（review 3.5）：RELAYOUT 行的 applied=N（缺欄位回 -1）與時間戳。
function Get-RelayoutApplied([string]$Line) {
    if ($Line -match 'applied=(\d+)') { return [int]$Matches[1] }
    return -1
}
function Get-LineTime([string]$Line) {
    [datetime]::ParseExact($Line.Substring(0, 23), 'yyyy-MM-ddTHH:mm:ss.fff', [Globalization.CultureInfo]::InvariantCulture)
}

# fix F2（review 3.5）：把一扇可見小工具挪離格子（位移＋縮小；SWP_NOZORDER|SWP_NOACTIVATE，非輸入
# 注入），回傳是否真的挪動了。重排若沒生效，它會停在挪動後的位置，矩形比對就會 FAIL。
$SWP_NOZORDER = 0x0004
$SWP_NOACTIVATE = 0x0010
function Move-OffGrid([IntPtr]$Hwnd, [string]$Label) {
    $r = Get-Rect $Hwnd
    $ok = [V35.Native]::SetWindowPos($Hwnd, [IntPtr]::Zero, $r.X + 37, $r.Y + 23, [Math]::Max(40, $r.W - 13), [Math]::Max(40, $r.H - 11), ($SWP_NOZORDER -bor $SWP_NOACTIVATE))
    Start-Sleep -Milliseconds 300
    $r2 = Get-Rect $Hwnd
    $moved = $ok -and ($r2.X -ne $r.X -or $r2.Y -ne $r.Y -or $r2.W -ne $r.W -or $r2.H -ne $r.H)
    $wlog.WriteLine("   [$Label] 人為挪離格子：ok=$ok before=$r after=$r2 moved=$moved")
    return $moved
}

# fixture：同 verify-3.3——clock 不靠資料一定出現；macro 一筆一定出現；events／punish／quotes
# 皆空，quotes（無資料自動隱藏）與 dynamic（無 events／punish）預期隱藏，正好用來驗證「已隱藏
# 的小工具重排後仍隱藏」這個不變量。
function New-Fixture {
    $now = Get-Date -Format 'yyyy-MM-dd HH:mm'
    $obj = [ordered]@{
        updated  = $now
        fetched  = $now
        errors   = @()
        macro    = @(@{ title = '測試指標'; date = $now })
        events   = @()
        punish   = @()
        quotes   = @()
        holidays = @()
    }
    return ($obj | ConvertTo-Json -Depth 6)
}

# ── 1. 前置檢查（鎖定與否都可繼續：PostMessage 不是輸入注入，不會打進登入畫面）───────────
if (Get-Process -Name fc-host -ErrorAction SilentlyContinue) {
    throw '已有 fc-host 在執行，請先結束。'
}
$Exe = (Resolve-Path $Exe).Path
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$wlogPath = Join-Path $OutDir '3.5-windows.log'
$slogPath = Join-Path $OutDir '3.5-summary.log'
$wlog = New-EvidenceWriter $wlogPath
$wlog.AutoFlush = $true
$wlog.WriteLine("# verify-3.5.ps1 start=$(Get-Ts) exe=$Exe locked=$([int](Test-Locked))")

# ── 2. 暫存 %APPDATA%／%LOCALAPPDATA%＋資料目錄 fixture ───────────────────────────────
$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ("fc-host-3.5-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
$tempAppData = Join-Path $tempRoot 'Roaming'
$tempLocalAppData = Join-Path $tempRoot 'Local'
New-Item -ItemType Directory -Force -Path $tempAppData, $tempLocalAppData | Out-Null
$dataDir = Join-Path $tempLocalAppData 'tw.fintools.fc-host\data'
New-Item -ItemType Directory -Force -Path $dataDir | Out-Null
Set-Content -Path (Join-Path $dataDir 'tw_events.json') -Value (New-Fixture) -Encoding UTF8
$gatekeeperLog = Join-Path $tempAppData 'tw.fintools.fc-host\gatekeeper.log'
$wlog.WriteLine("# APPDATA=$tempAppData（首次啟動；gatekeeper.log=$gatekeeperLog）LOCALAPPDATA=$tempLocalAppData")

$regSnap = @(Save-FcHostAutostartRegistry)
$wlog.WriteLine("# $(Get-Ts) 開機自啟登錄快照：$(($regSnap | ForEach-Object { "$($_.Name)@$(Split-Path $_.Key -Leaf) Exists=$($_.Exists)" }) -join '; ')")
# ── 3. 啟動宿主 ───────────────────────────────────────────────────────────────────
$oldAppData = $env:APPDATA
$oldLocalAppData = $env:LOCALAPPDATA
$hostProc = $null
try {
    $env:APPDATA = $tempAppData
    $env:LOCALAPPDATA = $tempLocalAppData
    $hostProc = Start-Process -FilePath $Exe -PassThru
} finally {
    $env:APPDATA = $oldAppData
    $env:LOCALAPPDATA = $oldLocalAppData
}
$hostPid = $hostProc.Id
$wlog.WriteLine("# $(Get-Ts) 宿主 pid=$hostPid")

$results = [ordered]@{}
try {
    # ── 4. 至少一個小工具出現；找出守門視窗 ───────────────────────────────────────────
    $ok = Wait-Cond { (Get-HostWidgetWindows $hostPid).Count -gt 0 } 20
    Start-Sleep -Seconds 3
    $wins = Get-HostWidgetWindows $hostPid
    $wlog.WriteLine("## $(Get-Ts) 啟動後：可見小工具 $($wins.Count) 個：$(($wins | ForEach-Object { $_.Title }) -join ', ')")
    $results['至少一個小工具出現'] = $ok -and ($wins.Count -gt 0)
    if (-not $ok -or $wins.Count -eq 0) { throw '沒有任何小工具視窗出現，後續步驟無法進行' }

    $gk = Get-GatekeeperWindow $hostPid
    $wlog.WriteLine("## $(Get-Ts) 守門視窗：$(if ($gk) { "hwnd=0x$($gk.Hwnd.ToInt64().ToString('X')) title='$($gk.Title)'" } else { '找不到' })")
    $results['找到守門視窗'] = [bool]$gk
    if (-not $gk) { throw '找不到守門視窗，無法送 PostMessage' }

    $allOwned = Get-AllOwnedWindows $hostPid
    $hiddenTitles = @()
    foreach ($h in $allOwned) {
        $title = Get-Title $h
        if ($title -like 'fc-host *' -and -not [V35.Native]::IsWindowVisible($h)) { $hiddenTitles += $title }
    }
    $wlog.WriteLine("## $(Get-Ts) 啟動後隱藏中的小工具：$(if ($hiddenTitles) { $hiddenTitles -join ', ' } else { '（無）' })")

    # ── 5. 觸發前基準：矩形＋隱藏狀態＋gatekeeper.log 行數 ─────────────────────────────
    $beforeRects = @{}
    foreach ($w in $wins) { $beforeRects[$w.Title] = Get-Rect $w.Hwnd }
    $wlog.WriteLine('## 觸發前矩形：')
    foreach ($t in $beforeRects.Keys) { $r = $beforeRects[$t]; $wlog.WriteLine("   $t x=$($r.X) y=$($r.Y) w=$($r.W) h=$($r.H)") }

    function Assert-SameRectsAndHidden([string]$Label, [hashtable]$Before) {
        $wins2 = Get-HostWidgetWindows $hostPid
        $sameCount = ($wins2.Count -eq $wins.Count)
        $rectsMatch = $true
        foreach ($w in $wins2) {
            $r = Get-Rect $w.Hwnd
            $b = $Before[$w.Title]
            if (-not $b -or $r.X -ne $b.X -or $r.Y -ne $b.Y -or $r.W -ne $b.W -or $r.H -ne $b.H) {
                $rectsMatch = $false
                $wlog.WriteLine("   [$Label] 矩形不同：$($w.Title) before=$b after=$r")
            }
        }
        $results["[$Label] 可見小工具數量不變"] = $sameCount
        $results["[$Label] 可見小工具矩形與觸發前一致"] = $rectsMatch

        $stillHidden = $true
        foreach ($h in (Get-AllOwnedWindows $hostPid)) {
            $title = Get-Title $h
            if ($title -in $hiddenTitles) {
                if ([V35.Native]::IsWindowVisible($h)) {
                    $stillHidden = $false
                    $wlog.WriteLine("   [$Label] 原隱藏的 $title 變成可見（違反 VISIBLE 旗標不變量）")
                }
            }
        }
        $results["[$Label] 原隱藏的小工具重排後仍隱藏"] = $(if ($hiddenTitles.Count -gt 0) { $stillHidden } else { 'SKIPPED（本次 fixture 下無隱藏小工具）' })
    }

    # ── 6. WM_DISPLAYCHANGE ────────────────────────────────────────────────────────
    $victim = $wins[0]
    $results['[WM_DISPLAYCHANGE] 觸發前已把小工具挪離格子'] = Move-OffGrid $victim.Hwnd 'WM_DISPLAYCHANGE'
    $baseline1 = Get-RelayoutLines $gatekeeperLog
    $at1 = Get-Ts
    $wlog.WriteLine("## $at1 STEP：PostMessage(gatekeeper, WM_DISPLAYCHANGE)（非輸入注入；locked=$([int](Test-Locked))）")
    $post1 = [V35.Native]::PostMessage($gk.Hwnd, $WM_DISPLAYCHANGE, [UIntPtr]::Zero, [IntPtr]::Zero)
    $results['PostMessage(WM_DISPLAYCHANGE) 呼叫成功'] = $post1

    $seen1 = Wait-Cond { (Get-RelayoutLines $gatekeeperLog).Count -gt $baseline1.Count } 15
    $new1 = (Get-RelayoutLines $gatekeeperLog) | Select-Object -Skip $baseline1.Count
    $wlog.WriteLine("## $(Get-Ts) 15 秒內出現 RELAYOUT（display-change）=$seen1；新增行：$($new1 -join ' | ')")
    $results['WM_DISPLAYCHANGE 後 gatekeeper.log 出現 RELAYOUT reason=display-change'] =
    $seen1 -and ($new1 | Where-Object { $_ -match 'reason=display-change' })
    $results['[WM_DISPLAYCHANGE] RELAYOUT 記錄 applied ≥ 1'] = [bool]($new1 | Where-Object { (Get-RelayoutApplied $_) -ge 1 })

    Start-Sleep -Milliseconds 500
    Assert-SameRectsAndHidden 'WM_DISPLAYCHANGE' $beforeRects

    # ── 7. WM_SETTINGCHANGE(SPI_SETWORKAREA) ───────────────────────────────────────
    # 用 SendMessageTimeout 而非 PostMessage：實測 PostMessage 對 WM_SETTINGCHANGE 回傳
    # false、GetLastError=1159（ERROR_MESSAGE_SYNC_ONLY，「此訊息只能用於同步作業」）——
    # 系統本身廣播 WM_SETTINGCHANGE 用的也是 SendMessageTimeout(HWND_BROADCAST, ...)（Microsoft
    # Learn `WM_SETTINGCHANGE`），不是 PostMessage；WM_DISPLAYCHANGE 沒有這個限制，維持
    # PostMessage（見上一步）。SendMessageTimeout 會同步等到守門視窗的訊息迴圈處理完才返回
    # （或逾時），非輸入注入——只是換一個 Win32 訊息傳遞 API，仍是本腳本文件開頭說明的
    # 「PostMessage／送訊息給守門視窗」範疇，不涉及任何鍵盤／滑鼠輸入。
    $results['[WM_SETTINGCHANGE] 觸發前已把小工具挪離格子'] = Move-OffGrid $victim.Hwnd 'WM_SETTINGCHANGE'
    $baseline2 = Get-RelayoutLines $gatekeeperLog
    $at2 = Get-Ts
    $wlog.WriteLine("## $at2 STEP：SendMessageTimeout(gatekeeper, WM_SETTINGCHANGE, wParam=SPI_SETWORKAREA)（非輸入注入；locked=$([int](Test-Locked))）")
    $smtoResult = [UIntPtr]::Zero
    $post2raw = [V35.Native]::SendMessageTimeout($gk.Hwnd, $WM_SETTINGCHANGE, $SPI_SETWORKAREA, [IntPtr]::Zero, $SMTO_ABORTIFHUNG, 5000, [ref]$smtoResult)
    $returnedAt2 = Get-Date
    $post2 = ($post2raw -ne [IntPtr]::Zero)
    $wlog.WriteLine("## $(Get-Ts) SendMessageTimeout 已返回（$($returnedAt2.ToString('HH:mm:ss.fff'))）")
    $results['SendMessageTimeout(WM_SETTINGCHANGE, SPI_SETWORKAREA) 呼叫成功'] = $post2

    $seen2 = Wait-Cond { (Get-RelayoutLines $gatekeeperLog).Count -gt $baseline2.Count } 15
    $new2 = (Get-RelayoutLines $gatekeeperLog) | Select-Object -Skip $baseline2.Count
    $wlog.WriteLine("## $(Get-Ts) 15 秒內出現 RELAYOUT（workarea-change）=$seen2；新增行：$($new2 -join ' | ')")
    $results['WM_SETTINGCHANGE(SPI_SETWORKAREA) 後 gatekeeper.log 出現 RELAYOUT reason=workarea-change'] =
    $seen2 -and ($new2 | Where-Object { $_ -match 'reason=workarea-change' })
    $results['[WM_SETTINGCHANGE] RELAYOUT 記錄 applied ≥ 1'] = [bool]($new2 | Where-Object { (Get-RelayoutApplied $_) -ge 1 })
    # 延後：同步送達的 WM_SETTINGCHANGE 返回之後才重排（舊版在 WndProc 內重排，RELAYOUT 會早於返回）。
    $results['[WM_SETTINGCHANGE] 重排在同步訊息返回之後才執行（延後）'] = [bool]($new2 | Where-Object { (Get-LineTime $_) -ge $returnedAt2.AddMilliseconds(-1) })

    Start-Sleep -Milliseconds 500
    Assert-SameRectsAndHidden 'WM_SETTINGCHANGE' $beforeRects

    # ── 8. 合併：同一波兩種訊息只重排一次 ─────────────────────────────────────────────
    $results['[合併] 觸發前已把小工具挪離格子'] = Move-OffGrid $victim.Hwnd '合併'
    $baseline3 = Get-RelayoutLines $gatekeeperLog
    $wlog.WriteLine("## $(Get-Ts) STEP：連續送 WM_DISPLAYCHANGE（Post）＋WM_SETTINGCHANGE（SendMessageTimeout）")
    $p3 = [V35.Native]::PostMessage($gk.Hwnd, $WM_DISPLAYCHANGE, [UIntPtr]::Zero, [IntPtr]::Zero)
    $s3raw = [V35.Native]::SendMessageTimeout($gk.Hwnd, $WM_SETTINGCHANGE, $SPI_SETWORKAREA, [IntPtr]::Zero, $SMTO_ABORTIFHUNG, 5000, [ref]$smtoResult)
    $seen3 = Wait-Cond { (Get-RelayoutLines $gatekeeperLog).Count -gt $baseline3.Count } 15
    Start-Sleep -Seconds 2   # 讓可能的第二次重排也有機會寫出來
    $new3 = @((Get-RelayoutLines $gatekeeperLog) | Select-Object -Skip $baseline3.Count)
    $wlog.WriteLine("## $(Get-Ts) 合併：post=$p3 smto=$($s3raw -ne [IntPtr]::Zero) 新增 RELAYOUT $($new3.Count) 行：$($new3 -join ' | ')")
    $results['[合併] 兩種訊息只產生一行 RELAYOUT，原因含兩者'] = $seen3 -and ($new3.Count -eq 1) -and
    ($new3[0] -match 'display-change') -and ($new3[0] -match 'workarea-change')
    Start-Sleep -Milliseconds 500
    Assert-SameRectsAndHidden '合併' $beforeRects

    $wlog.WriteLine("## 截圖：略過（鎖定狀態=$([int](Test-Locked))）")
}
finally {
    if ($hostProc -and -not $hostProc.HasExited) { Stop-Process -Id $hostPid -Force -ErrorAction SilentlyContinue }
    $regLeft = @(Restore-FcHostAutostartRegistry $regSnap)
    if ($regLeft.Count -gt 0) { Write-Warning "開機自啟登錄未還原：$($regLeft -join '; ')" }
    $wlog.WriteLine("# $(Get-Ts) 開機自啟登錄還原：未還原 $($regLeft.Count) 項$(if ($regLeft.Count) { '：' + ($regLeft -join '; ') })")
    $results['開機自啟登錄已還原（Run／StartupApproved 的 fc-host）'] = ($regLeft.Count -eq 0)
    $wlog.WriteLine("# $(Get-Ts) 宿主已結束")
    $wlog.Close()
}

$sum = New-EvidenceWriter $slogPath
$sum.WriteLine("# verify-3.5.ps1 summary $(Get-Ts)")
foreach ($k in $results.Keys) {
    $v = $results[$k]
    $tag = if ($v -is [string] -and $v -like 'SKIPPED*') { 'SKIP' } elseif ($v) { 'PASS' } else { 'FAIL' }
    $sum.WriteLine("$tag  $k")
}
$sum.WriteLine('# gatekeeper.log 內容：')
if (Test-Path $gatekeeperLog) { Get-Content $gatekeeperLog | ForEach-Object { $sum.WriteLine("  $_") } }
$sum.Close()
Get-Content $slogPath
try { Copy-EvidenceFile -Source $gatekeeperLog -Destination (Join-Path $OutDir '3.5-gatekeeper.log') } catch { Write-Warning "複製 gatekeeper.log 失敗：$_" }
Remove-Item -Recurse -Force $tempRoot -ErrorAction SilentlyContinue
