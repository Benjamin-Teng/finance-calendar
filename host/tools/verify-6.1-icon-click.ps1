<#
.SYNOPSIS
    Task 6.1 驗收驅動腳本（補「桌面圖示真的被選取」最後一哩，human-checklist.md A6）：對
    真正在跑的 fc-host（五個財經小工具，預設版面）旁的桌面圖示送真實合成滑鼠點擊，比對
    LVM_GETNEXTITEM 前後的選取索引，證明點擊確實命中圖示、沒有被小工具（always-on-bottom）
    攔截。視窗層「四邊外 1px 不命中宿主」與「面板本體未設定 WS_EX_TRANSPARENT」已由
    verify-6.1-appearance-hittest.ps1 驗過，本腳本只補這一項。

.DESCRIPTION
    找圖示位置不用「人工滑鼠移到圖示上讀游標座標」（本腳本無人操作），改用桌面
    SysListView32 的 LVM_GETITEMRECT／LVM_GETITEMCOUNT——ListView 屬於 explorer.exe，
    跨行程送這類訊息時 LPARAM 指標必須指向 explorer.exe 自己的位址空間，故用
    VirtualAllocEx／WriteProcessMemory／ReadProcessMemory／VirtualFreeEx 在遠端配置一小塊
    暫存記憶體傳遞結構（業界慣用手法，非本腳本發明）。

    步驟：
      1. `Invoke-SafeInputPreflight`：鎖定 → 結束碼 2；未鎖定但合成輸入不生效 → 結束碼 3。
      2. 確認沒有既有 fc-host；暫存 %APPDATA%／%LOCALAPPDATA%＋fixture 首次啟動（預設版面：
         五個小工具貼齊主螢幕工作區右上／右下）。
      3. 找桌面圖示清單（Progman 或某個 WorkerW 底下的 SHELLDLL_DefView → SysListView32）、
         列舉全部圖示的螢幕矩形，候選＝矩形中心不落在任何小工具矩形內者，依「距最近小工具邊緣
         的距離」由近到遠逐一經 `lib/Occluders.psm1` 的 `Clear-Occluders` 清遮擋（fix F9）：
         圖示中心被一般應用程式主視窗蓋住（例如最大化的瀏覽器）→ 經白名單暫時最小化，結束時在
         finally 照原 WINDOWPLACEMENT 還原。判讀經 `Assert-OccluderResult -TargetIsDesktop`：
         被系統 UI／工作列擋住或最小化後仍蓋著 → 換下一個候選，全部如此 → 結束碼 3（ENV-BLOCKED），
         **不點擊**；被宿主視窗蓋住、沒命中任何視窗、命中桌面卻不是圖示清單本身 → FAIL、不點擊。
      4. 記下點擊前 `LVM_GETNEXTITEM(-1, LVNI_SELECTED)`；若不是 -1（使用者原本就選了某個
         圖示），先用 `LVM_SETITEMSTATE` 清空選取，取得乾淨基準。
      5. `Send-GuardedClick` 點擊挑中的圖示中心；點擊前後都記 `GetForegroundWindow()` 的
         擁有者 pid（應非宿主）。
      6. 點擊後再讀一次 `LVM_GETNEXTITEM(-1, LVNI_SELECTED)`，應等於挑中圖示的索引。
      7. 結束宿主、刪除暫存目錄；不截圖。

    結果印在終端機並寫 `host/tools/evidence/6.1-icon-click-summary.log`（細節見
    `6.1-icon-click-log.log`）。結束碼：0＝全部 PASS、1＝有 FAIL、2＝BLOCKED（鎖定）、
    3＝ENV-BLOCKED（合成輸入不生效，或所有候選圖示都被系統 UI／工作列擋住）。

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

Import-Module (Join-Path $PSScriptRoot 'lib\SafeInput.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\Occluders.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\VerifyVerdict.psm1') -Force

Add-Type -Namespace V61C -Name Native -MemberDefinition @'
[DllImport("user32.dll")] public static extern System.IntPtr SetThreadDpiAwarenessContext(System.IntPtr ctx);
[DllImport("user32.dll")] public static extern System.IntPtr GetTopWindow(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern System.IntPtr GetWindow(System.IntPtr hWnd, uint cmd);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr hWnd, out uint pid);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(System.IntPtr hWnd, System.Text.StringBuilder s, int n);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassName(System.IntPtr hWnd, System.Text.StringBuilder s, int n);
[DllImport("user32.dll")] public static extern bool GetWindowRect(System.IntPtr hWnd, out RECT r);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern System.IntPtr FindWindowExW(System.IntPtr parent, System.IntPtr after, string cls, string title);
[DllImport("user32.dll")] public static extern System.IntPtr GetForegroundWindow();
[DllImport("user32.dll")] public static extern bool ClientToScreen(System.IntPtr hWnd, ref POINT pt);
[DllImport("user32.dll")] public static extern System.IntPtr SendMessageW(System.IntPtr hWnd, uint msg, System.IntPtr wParam, System.IntPtr lParam);
[DllImport("kernel32.dll")] public static extern System.IntPtr OpenProcess(uint access, bool inherit, uint pid);
[DllImport("kernel32.dll")] public static extern bool CloseHandle(System.IntPtr h);
[DllImport("kernel32.dll")] public static extern System.IntPtr VirtualAllocEx(System.IntPtr hProc, System.IntPtr addr, uint size, uint allocType, uint protect);
[DllImport("kernel32.dll")] public static extern bool VirtualFreeEx(System.IntPtr hProc, System.IntPtr addr, uint size, uint freeType);
[DllImport("kernel32.dll")] public static extern bool WriteProcessMemory(System.IntPtr hProc, System.IntPtr addr, byte[] buf, uint size, out System.IntPtr written);
[DllImport("kernel32.dll")] public static extern bool ReadProcessMemory(System.IntPtr hProc, System.IntPtr addr, byte[] buf, uint size, out System.IntPtr read);
[StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
[StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
[DllImport("user32.dll")] public static extern System.IntPtr WindowFromPoint(POINT pt);
[DllImport("user32.dll")] public static extern System.IntPtr GetAncestor(System.IntPtr hWnd, uint flags);
'@

# Per-Monitor-V2（-4），GetWindowRect／ClientToScreen 才會回實體像素（同
# verify-3.4.ps1／verify-6.1-appearance-hittest.ps1 的既有慣例）；沒設的話 pwsh 預設
# DPI 感知等級下座標會被系統縮放，導致算出來的圖示中心點在真實螢幕上系統性偏移。
[void][V61C.Native]::SetThreadDpiAwarenessContext([IntPtr](-4))

# fix F9：前置探查改在設定 DPI 感知之後（它以 GetWindowRect 與螢幕矩形判斷全螢幕覆蓋層，要與後續座標同一個
# awareness；tests/DpiAwareness.Tests.ps1 靜態檢查）。
$pf = Invoke-SafeInputPreflight
if ($pf.ExitCode -ne 0) { Write-Host $pf.Message; exit $pf.ExitCode }
Write-Host $pf.Message

function Get-Ts { Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fffK' }
function Hex([IntPtr]$h) { '0x{0:X}' -f $h.ToInt64() }

function Get-HostWidgetWindows([int]$ProcId) {
    $list = @()
    $h = [V61C.Native]::GetTopWindow([IntPtr]::Zero)
    while ($h -ne [IntPtr]::Zero) {
        $p = 0
        [void][V61C.Native]::GetWindowThreadProcessId($h, [ref]$p)
        if ($p -eq $ProcId -and [V61C.Native]::IsWindowVisible($h)) {
            $sb = New-Object System.Text.StringBuilder 256
            [void][V61C.Native]::GetWindowText($h, $sb, 256)
            if ($sb.ToString() -like 'fc-host *') {
                $r = New-Object V61C.Native+RECT
                [void][V61C.Native]::GetWindowRect($h, [ref]$r)
                $list += [PSCustomObject]@{ Hwnd = $h; Title = $sb.ToString(); Rect = $r }
            }
        }
        $h = [V61C.Native]::GetWindow($h, 2)
    }
    return $list
}

function Get-Cls([IntPtr]$h) { $sb = New-Object Text.StringBuilder 256; [void][V61C.Native]::GetClassName($h, $sb, 256); $sb.ToString() }

# 桌面圖示清單所在的 SysListView32：一般在 Progman 底下，部分 Windows 版本／設定會落在某個
# WorkerW 底下（多桌布/多顯示器情境）；兩邊都掃，找到第一個含 SHELLDLL_DefView 的就用。
# 踩雷記錄：FindWindowExW 的「title」參數用 PowerShell 的 `$null` 傳會被當成空字串
# （`FindWindowW('Progman', $null)` 找不到、但 `FindWindowW('Progman', 'Program Manager')`
# 找得到——Progman 標題非空，`$null` 被當成「只比對空標題」而非 Win32 語意的「不比對標題」），
# 導致這裡原本一律找不到；改用 `[NullString]::Value` 才會真的送 NULL 指標，Progman／
# SHELLDLL_DefView／SysListView32 皆一次找到。
function Find-DesktopListView {
    $progman = [V61C.Native]::FindWindowExW([IntPtr]::Zero, [IntPtr]::Zero, 'Progman', [NullString]::Value)
    $candidates = @($progman)
    $h = [V61C.Native]::GetTopWindow([IntPtr]::Zero)
    while ($h -ne [IntPtr]::Zero) {
        if ((Get-Cls $h) -eq 'WorkerW') { $candidates += $h }
        $h = [V61C.Native]::GetWindow($h, 2)
    }
    foreach ($top in $candidates) {
        if ($top -eq [IntPtr]::Zero) { continue }
        $defView = [V61C.Native]::FindWindowExW($top, [IntPtr]::Zero, 'SHELLDLL_DefView', [NullString]::Value)
        if ($defView -ne [IntPtr]::Zero) {
            $lv = [V61C.Native]::FindWindowExW($defView, [IntPtr]::Zero, 'SysListView32', [NullString]::Value)
            if ($lv -ne [IntPtr]::Zero) { return @{ Top = $top; DefView = $defView; ListView = $lv } }
        }
    }
    return $null
}

$LVM_GETITEMCOUNT = 0x1004
$LVM_GETITEMRECT = 0x100E
$LVM_GETNEXTITEM = 0x100C
$LVM_SETITEMSTATE = 0x104B
$LVNI_SELECTED = 2
$LVIF_STATE = 4
$LVIS_SELECTED = 2
$PROCESS_VM_OPERATION = 0x8; $PROCESS_VM_READ = 0x10; $PROCESS_VM_WRITE = 0x20; $PROCESS_QUERY_INFORMATION = 0x400

function Open-RemoteHandle([IntPtr]$lvHwnd) {
    $p = 0
    [void][V61C.Native]::GetWindowThreadProcessId($lvHwnd, [ref]$p)
    $access = $PROCESS_VM_OPERATION -bor $PROCESS_VM_READ -bor $PROCESS_VM_WRITE -bor $PROCESS_QUERY_INFORMATION
    $hProc = [V61C.Native]::OpenProcess([uint32]$access, $false, [uint32]$p)
    if ($hProc -eq [IntPtr]::Zero) { throw "OpenProcess(explorer pid=$p) 失敗（權限不足？）" }
    return $hProc
}

# 取單一圖示的螢幕矩形：LVM_GETITEMRECT 需要呼叫端先把 RECT.left 設成 LVIR_BOUNDS(0)
# 當輸入碼，訊息會把結果寫回同一塊記憶體——這塊記憶體必須在 explorer.exe 的位址空間。
function Get-IconScreenRect([IntPtr]$hProc, [IntPtr]$lvHwnd, [int]$Index) {
    $remote = [V61C.Native]::VirtualAllocEx($hProc, [IntPtr]::Zero, 16, 0x1000 -bor 0x2000, 4)
    if ($remote -eq [IntPtr]::Zero) { throw 'VirtualAllocEx 失敗' }
    try {
        $inBuf = New-Object byte[] 16
        [BitConverter]::GetBytes(0).CopyTo($inBuf, 0)  # LVIR_BOUNDS = 0
        $written = [IntPtr]::Zero
        [void][V61C.Native]::WriteProcessMemory($hProc, $remote, $inBuf, 16, [ref]$written)
        [void][V61C.Native]::SendMessageW($lvHwnd, [uint32]$LVM_GETITEMRECT, [IntPtr]$Index, $remote)
        $outBuf = New-Object byte[] 16
        $readN = [IntPtr]::Zero
        [void][V61C.Native]::ReadProcessMemory($hProc, $remote, $outBuf, 16, [ref]$readN)
        $l = [BitConverter]::ToInt32($outBuf, 0); $t = [BitConverter]::ToInt32($outBuf, 4)
        $r = [BitConverter]::ToInt32($outBuf, 8); $b = [BitConverter]::ToInt32($outBuf, 12)
        $tl = New-Object V61C.Native+POINT; $tl.X = $l; $tl.Y = $t
        [void][V61C.Native]::ClientToScreen($lvHwnd, [ref]$tl)
        $br = New-Object V61C.Native+POINT; $br.X = $r; $br.Y = $b
        [void][V61C.Native]::ClientToScreen($lvHwnd, [ref]$br)
        return [PSCustomObject]@{ Left = $tl.X; Top = $tl.Y; Right = $br.X; Bottom = $br.Y }
    } finally {
        [void][V61C.Native]::VirtualFreeEx($hProc, $remote, 0, 0x8000)
    }
}

function Get-SelectedIndex([IntPtr]$lvHwnd) {
    return [int]([V61C.Native]::SendMessageW($lvHwnd, [uint32]$LVM_GETNEXTITEM, [IntPtr](-1), [IntPtr]$LVNI_SELECTED)).ToInt64()
}

# 清空選取（僅在點擊前發現已有選取時使用，取得乾淨基準）：LVITEMW 開頭三個 UINT/int 欄位
# （mask/iItem/iSubItem 略過不理會、直接把整塊清零＋設 mask/state/stateMask）已足夠，
# 分配 96 bytes（64-bit LVITEMW 實際大小 88 bytes，多留餘裕避免寫入越界）。
function Clear-Selection([IntPtr]$hProc, [IntPtr]$lvHwnd) {
    $remote = [V61C.Native]::VirtualAllocEx($hProc, [IntPtr]::Zero, 96, 0x1000 -bor 0x2000, 4)
    if ($remote -eq [IntPtr]::Zero) { throw 'VirtualAllocEx（LVITEM）失敗' }
    try {
        $buf = New-Object byte[] 96
        [BitConverter]::GetBytes([int]$LVIF_STATE).CopyTo($buf, 0)      # mask
        [BitConverter]::GetBytes(0).CopyTo($buf, 12)                    # state = 0
        [BitConverter]::GetBytes([int]$LVIS_SELECTED).CopyTo($buf, 16)  # stateMask
        $written = [IntPtr]::Zero
        [void][V61C.Native]::WriteProcessMemory($hProc, $remote, $buf, 96, [ref]$written)
        [void][V61C.Native]::SendMessageW($lvHwnd, [uint32]$LVM_SETITEMSTATE, [IntPtr](-1), $remote)
    } finally {
        [void][V61C.Native]::VirtualFreeEx($hProc, $remote, 0, 0x8000)
    }
}

function Test-PointInRect([double]$px, [double]$py, $r) {
    return ($px -ge $r.Left -and $px -lt $r.Right -and $py -ge $r.Top -and $py -lt $r.Bottom)
}

function Distance-ToRect([double]$px, [double]$py, $r) {
    $dx = [Math]::Max([Math]::Max($r.Left - $px, 0), $px - $r.Right)
    $dy = [Math]::Max([Math]::Max($r.Top - $py, 0), $py - $r.Bottom)
    return [Math]::Sqrt($dx * $dx + $dy * $dy)
}

function Select-ReachableIcon {
    <#
    tests/Verify61.Tests.ps1 以 Parser 取出、餵 mock 遮擋結果測試。fix F9：候選圖示依「距最近小工具邊緣的距離」
    由近到遠排序，逐一以 ClearPoint（呼叫端＝lib/Occluders 的 Clear-Occluders：被一般應用程式主視窗蓋住就經
    白名單暫時最小化）取得遮擋結果，判讀一律經 lib 的 Get-OccluderVerdict／Assert-OccluderResult
    -TargetIsDesktop（目標本來就是桌面圖示）：
      - 到達桌面 → 再以 HitsListView 確認命中的是桌面圖示清單本身；是 → 選中；不是 → FAIL（點擊選不到圖示）；
      - 宿主視窗蓋住圖示中心、沒命中任何視窗 → FAIL，不找替代圖示（點擊會被截走，正是本項要驗的事）；
      - 工作列、系統 UI、最小化後仍蓋著 → 環境，略過、換下一個候選；全部如此 → ENV-BLOCKED，不點擊。
    Candidates 元素需有 Index、Cx、Cy、Dist。回傳 @{ Chosen; Skipped＝@{ Candidate; Reason } 清單 }。
    #>
    param([object[]]$Candidates = @(), [Parameter(Mandatory)][scriptblock]$ClearPoint, [Parameter(Mandatory)][scriptblock]$HitsListView)
    $skipped = New-Object System.Collections.Generic.List[object]
    $lastEnv = $null
    foreach ($c in @($Candidates | Sort-Object -Property Dist, Index)) {
        $what = "圖示 index=$($c.Index) 中心 ($($c.Cx),$($c.Cy))"
        $occ = & $ClearPoint $c
        $v = Get-OccluderVerdict -Result $occ -TargetIsDesktop
        if ($v.Verdict -eq 'env-blocked') {
            $skipped.Add([PSCustomObject]@{ Candidate = $c; Reason = $v.Message })
            $lastEnv = $occ
            continue
        }
        Assert-OccluderResult -Result $occ -What $what -TargetIsDesktop
        if (-not (& $HitsListView $c)) { throw "FAIL: $what 命中桌面，卻不是桌面圖示清單本身（點擊選不到圖示）" }
        return [PSCustomObject]@{ Chosen = $c; Skipped = $skipped.ToArray() }
    }
    if ($null -ne $lastEnv) { Assert-OccluderResult -Result $lastEnv -What "全部 $($skipped.Count) 個候選圖示（最後一個）" -TargetIsDesktop }
    throw 'FAIL: 沒有任何候選圖示可檢查'
}

# 點 (X,Y) 目前最上層的視窗是不是桌面圖示清單本身（或其子視窗）。
function Test-PointHitsListView([int]$X, [int]$Y, [IntPtr]$ListView) {
    $pt = New-Object V61C.Native+POINT
    $pt.X = $X; $pt.Y = $Y
    $hit = [V61C.Native]::WindowFromPoint($pt)
    if ($hit -eq [IntPtr]::Zero) { return $false }
    if ($hit -eq $ListView) { return $true }
    return ([V61C.Native]::GetAncestor($hit, 1) -eq $ListView)  # GA_PARENT=1
}

# ── 1. 前置檢查 ─────────────────────────────────────────────────────────────────
if (Get-Process -Name fc-host -ErrorAction SilentlyContinue) {
    throw '已有 fc-host 在執行，請先結束（WebView2 共用 user data folder，CDP 參數不會生效）。'
}
$Exe = (Resolve-Path $Exe).Path
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$logPath = Join-Path $OutDir '6.1-icon-click-log.log'
$sumPath = Join-Path $OutDir '6.1-icon-click-summary.log'
$log = New-EvidenceWriter $logPath
$log.AutoFlush = $true
$log.WriteLine("# verify-6.1-icon-click.ps1 start=$(Get-Ts) exe=$Exe")

$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ('fc-host-6.1c-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
$tempAppData = Join-Path $tempRoot 'Roaming'
$tempLocalAppData = Join-Path $tempRoot 'Local'
New-Item -ItemType Directory -Force -Path $tempAppData, $tempLocalAppData | Out-Null
$dataDir = Join-Path $tempLocalAppData 'tw.fintools.fc-host\data'
New-Item -ItemType Directory -Force -Path $dataDir | Out-Null
Copy-Item (Join-Path $PSScriptRoot '..\ui\fixtures\tw-events.json') (Join-Path $dataDir 'tw_events.json')
$log.WriteLine("# APPDATA=$tempAppData（首次啟動＝預設版面）LOCALAPPDATA=$tempLocalAppData")

$regSnap = @(Save-FcHostAutostartRegistry)
try { $log.WriteLine("# $(Get-Ts) 開機自啟登錄快照：$(($regSnap | ForEach-Object { "$($_.Name)@$(Split-Path $_.Key -Leaf) Exists=$($_.Exists)" }) -join '; ')") } catch { }
$results = [ordered]@{}
$hostProc = $null
$hProc = [IntPtr]::Zero
$stopCode = 0
$stopMsg = $null
# fix F9：被暫時最小化的使用者視窗（lib/Occluders），finally 照原 WINDOWPLACEMENT 還原。
$minimized = New-Object System.Collections.Generic.List[object]
# hung-target（review 642050f）：沒還原成功的使用者視窗（無回應、最小化未確認、讀回逾時／不符），寫進摘要。
$occNotRestored = New-Object System.Collections.Generic.List[string]
$oldAppData = $env:APPDATA; $oldLocalAppData = $env:LOCALAPPDATA

try {
    try {
        $env:APPDATA = $tempAppData; $env:LOCALAPPDATA = $tempLocalAppData
        $hostProc = Start-Process -FilePath $Exe -PassThru
    } finally {
        $env:APPDATA = $oldAppData; $env:LOCALAPPDATA = $oldLocalAppData
    }
    $hostPid = $hostProc.Id
    $log.WriteLine("# $(Get-Ts) 宿主 pid=$hostPid")

    $widgets = $null
    for ($i = 0; $i -lt 80; $i++) {
        $widgets = Get-HostWidgetWindows $hostPid
        if ($widgets.Count -ge 5) { break }
        Start-Sleep -Milliseconds 250
    }
    Start-Sleep -Seconds 2
    $results['五個財經小工具出現'] = ($widgets.Count -ge 5)
    foreach ($w in $widgets) { $log.WriteLine("## widget $(Hex $w.Hwnd) [$($w.Title)] rect=($($w.Rect.Left),$($w.Rect.Top))-($($w.Rect.Right),$($w.Rect.Bottom))") }
    if ($widgets.Count -lt 5) { throw '小工具未全部出現，無法繼續。' }

    $lv = Find-DesktopListView
    $results['找到桌面圖示清單（SysListView32）'] = [bool]$lv
    if (-not $lv) { throw '找不到 Progman/WorkerW → SHELLDLL_DefView → SysListView32。' }
    $log.WriteLine("## $(Get-Ts) desktop listview top=$(Hex $lv.Top)($(Get-Cls $lv.Top)) listview=$(Hex $lv.ListView)")

    $hProc = Open-RemoteHandle $lv.ListView
    $count = [int]([V61C.Native]::SendMessageW($lv.ListView, [uint32]$LVM_GETITEMCOUNT, [IntPtr]::Zero, [IntPtr]::Zero)).ToInt64()
    $log.WriteLine("## $(Get-Ts) 桌面圖示數量=$count")
    $results['桌面至少有一個圖示'] = ($count -gt 0)
    if ($count -le 0) { throw '桌面沒有任何圖示，無法測試。' }

    # 候選：中心點不落在任何小工具矩形內的圖示，記下距最近小工具邊緣的距離（human-checklist A6：
    # 「落在小工具邊緣外側附近」）。
    $cands = New-Object System.Collections.Generic.List[object]
    for ($i = 0; $i -lt $count; $i++) {
        $r = Get-IconScreenRect $hProc $lv.ListView $i
        $cx = ($r.Left + $r.Right) / 2.0
        $cy = ($r.Top + $r.Bottom) / 2.0
        $inside = $false
        $minD = [double]::MaxValue
        foreach ($w in $widgets) {
            if (Test-PointInRect $cx $cy $w.Rect) { $inside = $true }
            $d = Distance-ToRect $cx $cy $w.Rect
            if ($d -lt $minD) { $minD = $d }
        }
        if (-not $inside) { $cands.Add([PSCustomObject]@{ Index = $i; Rect = $r; Cx = [int]$cx; Cy = [int]$cy; Dist = $minD }) }
    }
    $results['找到不與任何小工具重疊的圖示'] = ($cands.Count -gt 0)
    if ($cands.Count -eq 0) { throw '所有圖示中心點都落在小工具矩形內（不應發生）。' }
    # fix F9：由近到遠挑第一個可到達的圖示。圖示中心被一般應用程式主視窗蓋住（例如使用者最大化的瀏覽器）
    # → lib/Occluders 經白名單暫時最小化、finally 還原；被系統 UI／工作列擋住 → 換下一個，全部如此才
    # ENV-BLOCKED（不點擊）；被宿主視窗蓋住或沒命中 → FAIL。判讀見 Select-ReachableIcon。
    $lvHwnd = $lv.ListView
    $lvTop = $lv.Top
    $sel = Select-ReachableIcon -Candidates $cands.ToArray() -ClearPoint {
        param($c)
        $occ = Clear-Occluders -HostPid $hostPid -TargetHwnd $lvTop -Points @(, @($c.Cx, $c.Cy)) -Minimized $minimized `
            -Log { param($m) $log.WriteLine("## $(Get-Ts) $m") }
        $log.WriteLine("## $(Get-Ts) 圖示 index=$($c.Index) center=($($c.Cx),$($c.Cy)) 遮擋清除 ok=$($occ.Ok) action=$($occ.Action) $($occ.Reason)")
        $occ
    } -HitsListView { param($c) Test-PointHitsListView $c.Cx $c.Cy $lvHwnd }
    foreach ($s in $sel.Skipped) { $log.WriteLine("## $(Get-Ts) 略過圖示 index=$($s.Candidate.Index) center=($($s.Candidate.Cx),$($s.Candidate.Cy)) 距最近小工具邊緣=$([Math]::Round($s.Candidate.Dist,1))px：$($s.Reason)") }
    $best = $sel.Chosen
    $log.WriteLine("## $(Get-Ts) 選中圖示 index=$($best.Index) center=($($best.Cx),$($best.Cy)) 距最近小工具邊緣=$([Math]::Round($best.Dist,1))px（遮擋已排除、WindowFromPoint 確認命中桌面圖示清單；略過 $($sel.Skipped.Count) 個較近候選）")
    $results['點擊目標：圖示中心落在桌面圖示清單上（未被宿主視窗截走）'] = $true

    # 點擊前基準：若已有選取，先清空。
    $before0 = Get-SelectedIndex $lv.ListView
    $log.WriteLine("## $(Get-Ts) 點擊前（清空前）選取索引=$before0")
    if ($before0 -ne -1) {
        Clear-Selection $hProc $lv.ListView
        Start-Sleep -Milliseconds 200
    }
    $before = Get-SelectedIndex $lv.ListView
    $log.WriteLine("## $(Get-Ts) 點擊前（乾淨基準）選取索引=$before")
    $results['點擊前無選取（基準乾淨，-1）'] = ($before -eq -1)

    $fgBefore = [V61C.Native]::GetForegroundWindow()
    $fgBeforePid = 0
    if ($fgBefore -ne [IntPtr]::Zero) { [void][V61C.Native]::GetWindowThreadProcessId($fgBefore, [ref]$fgBeforePid) }

    Set-GuardedCursorPos $best.Cx $best.Cy -What '桌面圖示'
    Send-GuardedClick $best.Cx $best.Cy
    Start-Sleep -Milliseconds 400

    $after = Get-SelectedIndex $lv.ListView
    $log.WriteLine("## $(Get-Ts) 點擊後選取索引=$after（應為 $($best.Index)）")
    $results['點擊後選中的正是目標圖示'] = ($after -eq $best.Index)

    $fgAfter = [V61C.Native]::GetForegroundWindow()
    $fgAfterPid = 0
    if ($fgAfter -ne [IntPtr]::Zero) { [void][V61C.Native]::GetWindowThreadProcessId($fgAfter, [ref]$fgAfterPid) }
    $log.WriteLine("## $(Get-Ts) fgBeforePid=$fgBeforePid fgAfterPid=$fgAfterPid hostPid=$hostPid")
    $results['點擊前後前景都不是宿主（點擊未被小工具截走）'] = ([int]$fgBeforePid -ne $hostPid) -and ([int]$fgAfterPid -ne $hostPid)
}
catch {
    # fix F4（review sectionA low）：BLOCKED／ENV-BLOCKED 不在 catch 內關 writer 或 exit——原本
    # catch 先 Close 再 exit，finally 又對已關閉的 writer 寫入、丟例外蓋掉結束碼，控制代碼也關兩次。
    # 這裡只記錄並標記，清理一律交給 finally，結束碼在 finally 之後才決定。
    if ("$_" -like 'BLOCKED*') { $stopCode = 2; $stopMsg = "$_"; $log.WriteLine("# $(Get-Ts) $_") }
    elseif ("$_" -like 'ENV-BLOCKED*') { $stopCode = 3; $stopMsg = "$_"; $log.WriteLine("# $(Get-Ts) $_") }
    # fix F9：圖示中心被宿主視窗蓋住、沒命中任何視窗、或命中桌面卻不是圖示清單（Assert-OccluderResult／
    # Select-ReachableIcon 丟「FAIL: 」）：不點擊，記 FAIL，結束碼 1。
    elseif ("$_" -like 'FAIL:*') {
        $results['點擊目標：圖示中心落在桌面圖示清單上（未被宿主視窗截走）'] = $false
        $log.WriteLine("## $(Get-Ts) $_"); Write-Host "$_" -ForegroundColor Red
    }
    else {
        $log.WriteLine("## $(Get-Ts) 例外中止：$($_.Exception.Message)")
        $results['腳本未跑完（見 6.1-icon-click-log.log 例外訊息）'] = $false
    }
}
finally {
    if ($hProc -ne [IntPtr]::Zero) { [void][V61C.Native]::CloseHandle($hProc); $hProc = [IntPtr]::Zero }
    if ($hostProc -and -not $hostProc.HasExited) { Stop-Process -Id $hostProc.Id -Force -ErrorAction SilentlyContinue }
    # fix F9：被暫時最小化的使用者視窗照原 WINDOWPLACEMENT 還原（反向、不啟用）。
    try { Restore-Occluders -Minimized $minimized -NotRestored $occNotRestored -Log { param($m) $log.WriteLine("# $(Get-Ts) $m") } } catch { try { $log.WriteLine("# 還原被最小化的視窗失敗：$_") } catch { } }
    # $minimized 非空＝Restore-Occluders 本身中途丟例外、沒跑完；一併列為未還原（環境，不進逐項結果）。
    if ($minimized.Count -gt 0) { $occNotRestored.Add("Restore-Occluders 未跑完：尚有 $($minimized.Count) 扇未處理（見記錄）") }
    $regLeft = @(Restore-FcHostAutostartRegistry $regSnap)
    if ($regLeft.Count -gt 0) { Write-Warning "開機自啟登錄未還原：$($regLeft -join '; ')" }
    try { $log.WriteLine("# $(Get-Ts) 開機自啟登錄還原：未還原 $($regLeft.Count) 項$(if ($regLeft.Count) { '：' + ($regLeft -join '; ') })") } catch { }
    $results['開機自啟登錄已還原（Run／StartupApproved 的 fc-host）'] = ($regLeft.Count -eq 0)
    try { $log.WriteLine("# $(Get-Ts) 宿主已結束") } catch { }
    try { $log.Close() } catch { }
    if ($stopCode) { Remove-Item -Recurse -Force $tempRoot -ErrorAction SilentlyContinue }
}
$occWarn = Format-UnrestoredWarning -NotRestored $occNotRestored
if ($stopCode -eq 2 -or $stopCode -eq 3) {
    if ($stopCode -eq 2) { Write-Host "BLOCKED（結束碼 2，不做驗收判讀）：$stopMsg" }
    else { Write-Host "ENV-BLOCKED（結束碼 3，不做驗收判讀）：$stopMsg" -ForegroundColor Yellow }
    if ($occWarn) {
        Write-Host $occWarn -ForegroundColor Yellow
        # 中途停止沒有逐項摘要：把警示行（連同停止原因）落檔，否則只剩終端機輸出會隨 session 消失。
        @("# verify-6.1-icon-click.ps1 summary $(Get-Ts)", "$(if ($stopCode -eq 2) { 'BLOCKED' } else { 'ENV-BLOCKED' })  $stopMsg", $occWarn) | ConvertTo-EvidenceText | Set-Content -Path $sumPath -Encoding utf8
    }
    exit (Get-VerdictExitCode -Locked:($stopCode -eq 2) -EnvBlocked:($stopCode -eq 3) -NotRestored $occNotRestored)
}

$sum = New-EvidenceWriter $sumPath
$sum.WriteLine("# verify-6.1-icon-click.ps1 summary $(Get-Ts)")
foreach ($k in $results.Keys) { $sum.WriteLine("$(if ($results[$k]) { 'PASS' } else { 'FAIL' })  $k") }
if ($occWarn) { $sum.WriteLine($occWarn) }
$sum.Close()
Get-Content $sumPath
if ($occWarn) { Write-Host $occWarn -ForegroundColor Yellow }
Remove-Item -Recurse -Force $tempRoot -ErrorAction SilentlyContinue

exit (Get-VerdictExitCode -Results $results -NotRestored $occNotRestored)
