<#
.SYNOPSIS
    代驗 human-checklist.md B4（task 3.1 首次啟動預設版面）、B5（task 3.2 小工具卡片完整度）、
    B8（task 5.2 設定視窗，僅靜態外觀）：暫存 %APPDATA% 啟動 fc-host、把主螢幕一般視窗最小化、
    截圖存證、結束後把宿主停掉並把所有被最小化的視窗還原。全程不做任何鍵盤／滑鼠／觸控注入
    （最小化／還原一律經 lib/Occluders.psm1、Start-Process 啟動宿主——皆非輸入注入 API）。

.DESCRIPTION
    fix F4（review B-batch2）：啟動到截圖整段在同一個 try 內，任何中斷都會停掉本腳本啟動的宿主
    （不以行程名稱停止）、還原開機自啟登錄、刪暫存目錄；截圖前確認 settings.json 啟用的每一扇
    小工具都已出現（Settings 模式另確認「財經桌布設定」視窗出現），不成立就不截圖、結束碼 1。

    -Mode Layout（對應 B4＋B5）：
      1. 確認沒有 fc-host.exe 在跑、工作階段未鎖定（LogonUI.exe）。
      2. 暫存 %APPDATA%（首次啟動、不碰真正設定）啟動 fc-host.exe。
      3. 枚舉目前主螢幕上所有「一般視窗」（可見、非最小化、非 cloaked、非 tool window、
         非零尺寸——與 watch-zorder.ps1 的「below」判準一致，並排除 fc-host 自己的小工具
         視窗），經 lib/Occluders.psm1 的 Invoke-MinimizeAppWindows 暫時最小化——fix F8：只動白名單
         內的一般應用程式主視窗，系統 UI（如 Shell_SystemDim）、對話框、殼層等跳過並記原因，不中止。
      4. 等 DWM 動畫穩定，用 Graphics.CopyFromScreen 截主螢幕全螢幕一張圖
         （evidence/B-layout-fullscreen.png）。
      5. 以 Restore-Occluders 全部還原（含吸附與最大化），停 fc-host，刪暫存目錄。
      B4／B5 都用同一張截圖判讀（B4 看整體版面排列、B5 放大看時鐘卡邊界），因為兩項 checklist
      本身都指示用「同 B4 的暫存啟動方式」、預設首次啟動版面一致，不需要跑兩次。

    -Mode Settings（對應 B8 靜態部分）：
      1. 同上前置檢查。
      2. 暫存 %APPDATA% 啟動 fc-host.exe 第一次（建立小工具與系統匣）。
      3. 等幾秒後，用同一組暫存 %APPDATA% 環境變數再 Start-Process fc-host.exe 第二次——
         這是啟動一個新行程，不是任何形式的鍵盤/滑鼠/觸控注入；宿主的 single-instance
         偵測會讓第一個行程收到訊號並開出「財經桌布設定」視窗（見 AGENTS.md host 設計）。
      4. 最小化主螢幕一般視窗、截圖（evidence/B-settings-fullscreen.png）、全部還原。
      5. 停掉所有 fc-host 行程、刪暫存目錄。
      只截圖，不做任何拖曳／點擊（checklist 第 4 步「需要互動」的部分本腳本不執行，留待有
      合成輸入可用時再補）。

.PARAMETER Mode
    Layout 或 Settings。

.PARAMETER Exe
    fc-host.exe 路徑，預設 host/target/release/fc-host.exe。

.PARAMETER OutDir
    證據輸出目錄，預設 host/tools/evidence。

.EXAMPLE
    cd host; cargo build --release
    pwsh -File tools/verify-b-manual-screenshot.ps1 -Mode Layout
    pwsh -File tools/verify-b-manual-screenshot.ps1 -Mode Settings
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][ValidateSet('Layout', 'Settings')][string]$Mode,
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\fc-host.exe'),
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [int]$WaitSec = 3
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\AutostartRegistry.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\ProcessTree.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\Occluders.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\VerifyVerdict.psm1') -Force

Add-Type -Namespace VB -Name Native -MemberDefinition @'
public delegate bool EnumProc(System.IntPtr h, System.IntPtr l);
[DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, System.IntPtr l);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(System.IntPtr h);
[DllImport("user32.dll")] public static extern bool IsIconic(System.IntPtr h);
[DllImport("user32.dll", CharSet = System.Runtime.InteropServices.CharSet.Unicode)]
public static extern int GetClassName(System.IntPtr h, System.Text.StringBuilder sb, int n);
[DllImport("user32.dll", CharSet = System.Runtime.InteropServices.CharSet.Unicode)]
public static extern int GetWindowText(System.IntPtr h, System.Text.StringBuilder sb, int n);
[DllImport("user32.dll")] public static extern int GetWindowTextLength(System.IntPtr h);
[StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
[DllImport("user32.dll")] public static extern bool GetWindowRect(System.IntPtr h, out RECT r);
[DllImport("user32.dll", EntryPoint = "GetWindowLong")] public static extern int GetWindowLong32(System.IntPtr h, int idx);
[DllImport("user32.dll", EntryPoint = "GetWindowLongPtr")] public static extern System.IntPtr GetWindowLongPtr64(System.IntPtr h, int idx);
public static long GetWindowLongAny(System.IntPtr h, int idx) {
    if (System.IntPtr.Size == 8) { return GetWindowLongPtr64(h, idx).ToInt64(); }
    return (long)GetWindowLong32(h, idx);
}
[DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(System.IntPtr h, int attr, out int val, int size);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr h, out uint pid);
'@

$GWL_EXSTYLE = -20
$WS_EX_TOOLWINDOW = 0x00000080
$DWMWA_CLOAKED = 14

function Get-Ts { Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fffK' }
function Get-Cls([IntPtr]$h) { $sb = New-Object Text.StringBuilder 256; [void][VB.Native]::GetClassName($h, $sb, 256); $sb.ToString() }
function Get-Title([IntPtr]$h) { $n = [VB.Native]::GetWindowTextLength($h); if ($n -le 0) { return '' }; $sb = New-Object Text.StringBuilder ($n + 1); [void][VB.Native]::GetWindowText($h, $sb, $sb.Capacity); $sb.ToString() }
function Get-WinPid([IntPtr]$h) { $p = 0; [void][VB.Native]::GetWindowThreadProcessId($h, [ref]$p); [int]$p }

function Test-Locked { return [bool](Get-Process -Name LogonUI -ErrorAction SilentlyContinue) }

# 「一般視窗」判準：與 watch-zorder.ps1 的 below 清單一致（可見、非最小化、非 cloaked、
# 非 tool window、非零尺寸），另外排除 fc-host 自己的小工具視窗（不該被誤最小化）。
function Get-GeneralTopWindows {
    $list = New-Object System.Collections.Generic.List[IntPtr]
    $cb = [VB.Native+EnumProc] {
        param($h, $l)
        try {
            if (-not [VB.Native]::IsWindowVisible($h)) { return $true }
            if ([VB.Native]::IsIconic($h)) { return $true }
            $cloakedVal = 0
            [void][VB.Native]::DwmGetWindowAttribute($h, $DWMWA_CLOAKED, [ref]$cloakedVal, 4)
            if ($cloakedVal -ne 0) { return $true }
            $exStyle = [VB.Native]::GetWindowLongAny($h, $GWL_EXSTYLE)
            if (($exStyle -band $WS_EX_TOOLWINDOW) -ne 0) { return $true }
            $rect = New-Object VB.Native+RECT
            if (-not [VB.Native]::GetWindowRect($h, [ref]$rect)) { return $true }
            $w = $rect.Right - $rect.Left; $ht = $rect.Bottom - $rect.Top
            if ($w -le 0 -or $ht -le 0) { return $true }
            $procId = Get-WinPid $h
            $procName = (Get-Process -Id $procId -ErrorAction SilentlyContinue).ProcessName
            if ($procName -eq 'fc-host') { return $true }
            $script:list.Add($h)
        } catch {}
        return $true
    }
    $script:list = $list
    [void][VB.Native]::EnumWindows($cb, [IntPtr]::Zero)
    return $list
}

# 指定行程擁有的可見頂層視窗標題。
function Get-VisibleTitlesOf([int[]]$Pids) {
    # 同 Get-GeneralTopWindows：回呼裡經 $script: 變數存取清單（不用 GetNewClosure，才看得到本腳本的函式）。
    $script:titlesTmp = New-Object System.Collections.Generic.List[string]
    $script:pidsTmp = $Pids
    $cb = [VB.Native+EnumProc] {
        param($h, $l)
        try {
            if ([VB.Native]::IsWindowVisible($h) -and ($script:pidsTmp -contains (Get-WinPid $h))) { $script:titlesTmp.Add((Get-Title $h)) }
        } catch {}
        return $true
    }
    [void][VB.Native]::EnumWindows($cb, [IntPtr]::Zero)
    return $script:titlesTmp.ToArray()
}

function Get-ScreenshotReadiness {
    <#
    純函式（tests/VerifyBManualWheelRouting.Tests.ps1 以 Parser 取出測試）。Layout：啟用的每一扇
    小工具都有可見視窗（標題 `fc-host <id>`）；Settings：另外要有「財經桌布設定」視窗。
    回傳 @{ Ready; Reason }。（review B-batch2 low：原本只要截圖成功就結束碼 0。）
    #>
    param([string]$Mode, [string[]]$Titles = @(), [string[]]$EnabledIds = @())
    if ($EnabledIds.Count -eq 0) { return [PSCustomObject]@{ Ready = $false; Reason = 'settings.json 沒有任何啟用的小工具' } }
    $missing = @($EnabledIds | Where-Object { $Titles -notcontains "fc-host $_" })
    $settingsShown = $Titles -contains '財經桌布設定'
    $parts = @("小工具 $($EnabledIds.Count - $missing.Count)/$($EnabledIds.Count) 扇可見$(if ($missing.Count) { '（缺 ' + ($missing -join ',') + '）' })")
    if ($Mode -eq 'Settings') { $parts += "設定視窗出現=$settingsShown" }
    $ready = ($missing.Count -eq 0) -and ($Mode -ne 'Settings' -or $settingsShown)
    return [PSCustomObject]@{ Ready = $ready; Reason = ($parts -join '；') }
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$log = New-EvidenceWriter (Join-Path $OutDir "B-$($Mode.ToLower())-log.log")
$log.AutoFlush = $true
function Wl([string]$s) { $line = "$(Get-Ts) $s"; $log.WriteLine($line); Write-Host $line }

if (Test-Locked) { Wl 'BLOCKED：LogonUI.exe 在跑（工作階段鎖定），已停止。'; $log.Close(); exit 2 }
if (Get-Process -Name fc-host -ErrorAction SilentlyContinue) {
    Wl '已有 fc-host 在執行，請先手動結束再重跑本腳本。'
    $log.Close()
    exit 1
}
if (-not (Test-Path $Exe)) { Wl "找不到 $Exe，先在 host/ 執行 cargo build --release"; $log.Close(); exit 1 }
$Exe = (Resolve-Path $Exe).Path

$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ("fc-host-B$($Mode)-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
$tempAppData = Join-Path $tempRoot 'Roaming'
$tempLocalAppData = Join-Path $tempRoot 'Local'
$oldAppData = $env:APPDATA
$oldLocalAppData = $env:LOCALAPPDATA
$minimized = New-Object System.Collections.Generic.List[object]
# hung-target（review 642050f）：沒還原成功的使用者視窗（無回應、最小化未確認、讀回逾時／不符）。屬環境問題，
# 不進逐項結果；摘要另起警示行（Format-UnrestoredWarning），結束碼經 Get-VerdictExitCode（環境＝3）。
$occNotRestored = New-Object System.Collections.Generic.List[string]
$procs = New-Object System.Collections.Generic.List[System.Diagnostics.Process]
$regSnap = @(Save-FcHostAutostartRegistry)
Wl "開機自啟登錄快照：$(($regSnap | ForEach-Object { "$($_.Name)@$(Split-Path $_.Key -Leaf) Exists=$($_.Exists)" }) -join '; ')"
$exitCode = 0

# fix F4（review B-batch2 medium）：啟動、等待、截圖整段在同一個 try 內，任何中斷（Ctrl+C、第二次
# Start-Process 失敗、等待逾時）都會經過下面的 finally：停自己啟動的宿主、還原開機自啟登錄、刪暫存。
try {
    New-Item -ItemType Directory -Force -Path $tempAppData, $tempLocalAppData | Out-Null
    Wl "暫存 APPDATA=$tempAppData LOCALAPPDATA=$tempLocalAppData"
    try {
        $env:APPDATA = $tempAppData
        $env:LOCALAPPDATA = $tempLocalAppData
        $p1 = Start-Process -FilePath $Exe -PassThru
        $procs.Add($p1)
        Wl "第一次啟動 fc-host pid=$($p1.Id)"
    } finally {
        $env:APPDATA = $oldAppData
        $env:LOCALAPPDATA = $oldLocalAppData
    }
    Start-Sleep -Seconds $WaitSec

    # 預設啟用的小工具（首次啟動寫出的 settings.json）全部出現才截圖（review B-batch2 low）。
    $settingsPath = Join-Path (Join-Path $tempAppData 'tw.fintools.fc-host') 'settings.json'
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while (-not (Test-Path -LiteralPath $settingsPath) -and $sw.Elapsed.TotalSeconds -lt 20) { Start-Sleep -Milliseconds 300 }
    if (-not (Test-Path -LiteralPath $settingsPath)) { throw "等不到首次啟動的 settings.json：$settingsPath" }
    $cfg = Get-Content -LiteralPath $settingsPath -Raw | ConvertFrom-Json
    $enabledIds = @($cfg.widgets.PSObject.Properties | Where-Object { $_.Value.enabled } | ForEach-Object { $_.Name })
    Wl "settings.json 啟用的小工具：$($enabledIds -join ',')"

    if ($Mode -eq 'Settings') {
        # 第二次啟動：同一組暫存 APPDATA/LOCALAPPDATA，觸發 single-instance 訊號讓第一個
        # 行程開出設定視窗。這是啟動一個新的行程（Start-Process），不是鍵盤/滑鼠/觸控注入。
        try {
            $env:APPDATA = $tempAppData
            $env:LOCALAPPDATA = $tempLocalAppData
            $p2 = Start-Process -FilePath $Exe -PassThru
            $procs.Add($p2)
            Wl "第二次啟動 fc-host pid=$($p2.Id)（觸發第一個行程開設定視窗，非注入）"
        } finally {
            $env:APPDATA = $oldAppData
            $env:LOCALAPPDATA = $oldLocalAppData
        }
        Start-Sleep -Seconds 3
        # 第二個行程通常會偵測到既有實例後自行結束；記下結束與否供記錄。
        Start-Sleep -Milliseconds 500
        Wl "第二個行程狀態：hasExited=$($p2.HasExited) exitCode=$(if ($p2.HasExited) { $p2.ExitCode } else { 'n/a' })"
    }

    $ready = $null
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt 15) {
        $titles = @(Get-VisibleTitlesOf @($procs | ForEach-Object { $_.Id }))
        $ready = Get-ScreenshotReadiness -Mode $Mode -Titles $titles -EnabledIds $enabledIds
        if ($ready.Ready) { break }
        Start-Sleep -Milliseconds 500
    }
    Wl "截圖前提：$($ready.Reason)"
    if (-not $ready.Ready) { throw "PRECONDITION：$($ready.Reason)；不截圖（截了也只會是缺件的畫面）" }

    # 最小化主螢幕上目前所有一般視窗（含這個 pwsh/終端機視窗本身），記下清單供結束時還原。
    # fix F8：經 lib/Occluders.psm1（白名單）；系統 UI、對話框、殼層等跳過並記原因（截圖照樣進行）。
    $targets = @(Get-GeneralTopWindows)
    foreach ($h in $targets) { Wl "候選 hwnd=0x$($h.ToString('X')) class=$(Get-Cls $h) title=$(Get-Title $h) pid=$(Get-WinPid $h)" }
    $minRes = Invoke-MinimizeAppWindows -Hwnds $targets -Minimized $minimized -Log { param($m) Wl $m }
    Wl "共最小化 $($minRes.Count) 個一般視窗；跳過 $(@($minRes.Skipped).Count) 個不在白名單的視窗"
    Start-Sleep -Milliseconds 1200

    Add-Type -AssemblyName System.Drawing
    Add-Type -AssemblyName System.Windows.Forms
    $screen = [System.Windows.Forms.Screen]::PrimaryScreen
    $bounds = $screen.Bounds
    $bmp = New-Object System.Drawing.Bitmap ([int]$bounds.Width), ([int]$bounds.Height)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen([int]$bounds.Left, [int]$bounds.Top, 0, 0, $bmp.Size)
    $g.Dispose()
    $shotPath = Join-Path $OutDir "B-$($Mode.ToLower())-fullscreen.png"
    $bmp.Save($shotPath, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    Wl "已存截圖 $shotPath ($($bounds.Width)x$($bounds.Height) device=$($screen.DeviceName))"
}
catch {
    Wl "例外：$_"
    $exitCode = 1
}
finally {
    $restoreCount = $minimized.Count
    try { Restore-Occluders -Minimized $minimized -NotRestored $occNotRestored -Log { param($m) Wl $m } } catch { Wl "還原被最小化的視窗失敗：$_" }
    # $minimized 非空＝Restore-Occluders 本身中途丟例外、沒跑完；一併列為未還原（環境，不進逐項結果）。
    if ($minimized.Count -gt 0) { $occNotRestored.Add("Restore-Occluders 未跑完：尚有 $($minimized.Count) 扇未處理（見記錄）") }
    $occWarn = Format-UnrestoredWarning -NotRestored $occNotRestored
    if ($occWarn) { Wl $occWarn }
    Wl "已還原 $restoreCount 個視窗"
    Start-Sleep -Milliseconds 500

    # 只停本腳本啟動的宿主與其子孫，不以行程名稱停止（fix F4 共通規則）。
    foreach ($proc in $procs) {
        try { [void](Stop-ProcessTree -Process $proc) } catch {}
    }
    Wl "已停止本腳本啟動的 fc-host（pid=$(($procs | ForEach-Object { $_.Id }) -join ',')）及其子孫"

    $regLeft = @(Restore-FcHostAutostartRegistry $regSnap)
    if ($regLeft.Count -gt 0) { Write-Warning "開機自啟登錄未還原：$($regLeft -join '; ')" }
    Wl "開機自啟登錄還原：未還原 $($regLeft.Count) 項$(if ($regLeft.Count) { '：' + ($regLeft -join '; ') })"
    if ($regLeft.Count -gt 0) { $exitCode = 1 }
    Remove-Item -Recurse -Force $tempRoot -ErrorAction SilentlyContinue
    Wl "已刪暫存目錄 $tempRoot"
    $log.Close()
}
if ($occWarn) { Write-Host $occWarn -ForegroundColor Yellow }
# 結束碼優先序：產品 FAIL（1）＞ 鎖定（2）＞ 環境（3，含使用者視窗未還原）。
exit (Get-VerdictExitCode -Failed:($exitCode -eq 1) -Locked:($exitCode -eq 2) -EnvBlocked:($exitCode -eq 3) -NotRestored $occNotRestored)
