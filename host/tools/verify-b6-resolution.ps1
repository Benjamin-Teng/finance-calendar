<#
.SYNOPSIS
    代驗 human-checklist.md B6「人工項目 2：改變解析度」（task 3.5 多螢幕重新定位）。
    以文件化的 EnumDisplaySettingsEx／ChangeDisplaySettingsEx 切到另一個支援的解析度
    （不帶 CDS_UPDATEREGISTRY，只改動態模式、不持久化），**逐扇**比對所有可見小工具是否仍落在
    各自的格座標，再改回原解析度並讀回確認。全程不做任何鍵盤／滑鼠／觸控注入。

.DESCRIPTION
    判準對照 human-checklist B6（fix F4，review B-batch3 high：原本只看 fixed 一扇，放過「最後一扇
    停在 Windows 建議矩形」的失效型態）：
      1. 每一扇可見小工具的視窗矩形＝它在 settings.json 的格座標在「新」工作區換算的格線矩形；
      2. settings.json 每一扇的 placement 在解析度變更前後完全不變（暫時換位不得寫回）；
      3. 所有可見小工具兩兩不相交；
      4. 變更後 gatekeeper.log 有 `EVENT WM_DISPLAYCHANGE`，且最後一行
         `RELAYOUT reason=…display-change… applied=N` 的 N＝可見小工具數。
    變更前也先核對 1 與 3（前提不成立就不改解析度）。

    系統狀態（fix F4 共通規則；review B-batch3 medium／low）：
      - 讀不到目前模式就直接中止，不會拿全零的 DEVMODE 去「還原」；
      - 「需要還原」旗標在 ChangeDisplaySettingsEx 之前設好；套用後立即讀回，與目標不符就中止
        （finally 還原）；
      - 還原前查鎖定（LogonUI.exe），鎖定就等到解鎖（最多 UnlockWaitSec 秒），仍鎖定則不呼叫、
        明確記錄「解析度仍為 X、未還原」並以結束碼 2 結束；
      - 還原一律檢查回傳碼並讀回比對，finally 的保險還原同樣如此，結果計入結束碼。

    流程：
    1. 確認沒有 LogonUI.exe（鎖定）、沒有 fc-host.exe 在跑（try 之前，不符直接結束）。
    2. 記錄目前解析度；列舉同色深、不同尺寸的模式（優先 1920x1200）。
    3. 暫存 %APPDATA%／%LOCALAPPDATA% 首次啟動 fc-host（不碰使用者真正設定與資料）。
    4. 等所有 enabled 小工具出現，記下 settings.json placement 與各扇矩形，核對前提。
    5. 記下 gatekeeper.log 行數，套用新解析度並讀回；等 WaitAfterChangeSec 秒後依上列判準核對。
    6. 還原原始模式並讀回；停自己啟動的 fc-host 與其子孫、刪暫存目錄。

.PARAMETER Exe
    fc-host.exe 路徑，預設 host/target/release/fc-host.exe。

.PARAMETER OutDir
    證據輸出目錄，預設 host/tools/evidence。

.PARAMETER UnlockWaitSec
    還原解析度前若工作階段鎖定，最多等多少秒解鎖（預設 300）。
#>
[CmdletBinding()]
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\fc-host.exe'),
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [int]$WaitAfterChangeSec = 10,
    [int]$UnlockWaitSec = 300
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\AutostartRegistry.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\ProcessTree.psm1') -Force

Add-Type -Namespace B6 -Name Native -MemberDefinition @'
[StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
public struct DEVMODE {
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string dmDeviceName;
    public short dmSpecVersion;
    public short dmDriverVersion;
    public short dmSize;
    public short dmDriverExtra;
    public int dmFields;
    public int dmPositionX;
    public int dmPositionY;
    public int dmDisplayOrientation;
    public int dmDisplayFixedOutput;
    public short dmColor;
    public short dmDuplex;
    public short dmYResolution;
    public short dmTTOption;
    public short dmCollate;
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string dmFormName;
    public short dmLogPixels;
    public int dmBitsPerPel;
    public int dmPelsWidth;
    public int dmPelsHeight;
    public int dmDisplayFlags;
    public int dmDisplayFrequency;
    public int dmICMMethod;
    public int dmICMIntent;
    public int dmMediaType;
    public int dmDitherType;
    public int dmReserved1;
    public int dmReserved2;
    public int dmPanningWidth;
    public int dmPanningHeight;
}
[DllImport("user32.dll", CharSet = CharSet.Unicode)]
public static extern bool EnumDisplaySettingsExW(string lpszDeviceName, int iModeNum, ref DEVMODE lpDevMode, int dwFlags);
[DllImport("user32.dll", CharSet = CharSet.Unicode)]
public static extern int ChangeDisplaySettingsExW(string lpszDeviceName, ref DEVMODE lpDevMode, System.IntPtr hwnd, int dwflags, System.IntPtr lParam);
[DllImport("user32.dll")] public static extern System.IntPtr GetTopWindow(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern System.IntPtr GetWindow(System.IntPtr hWnd, uint cmd);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr hWnd, out uint pid);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(System.IntPtr hWnd, System.Text.StringBuilder s, int n);
[DllImport("user32.dll")] public static extern bool GetWindowRect(System.IntPtr hWnd, out RECT r);
[DllImport("user32.dll")] public static extern System.IntPtr SetThreadDpiAwarenessContext(System.IntPtr ctx);
[DllImport("user32.dll")] public static extern uint GetDpiForWindow(System.IntPtr hWnd);
[StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
[StructLayout(LayoutKind.Sequential)] public struct GPOINT { public int X, Y; }
[StructLayout(LayoutKind.Sequential)] public struct MONITORINFO { public int cbSize; public RECT rcMonitor; public RECT rcWork; public uint dwFlags; }
[DllImport("user32.dll")] public static extern System.IntPtr MonitorFromPoint(GPOINT pt, uint flags);
[DllImport("user32.dll")] public static extern bool GetMonitorInfoW(System.IntPtr hMon, ref MONITORINFO mi);
'@

# ---------------------------------------------------------------- 純函式（tests/VerifyB10B6.Tests.ps1 以 Parser 取出測試）

# 格座標 → 工作區上的格線矩形（同 host/src/layout.rs grid_rect_to_physical：起點＋floor(i×長度/48)）。
function Get-GridRectPx($Work, $Grid) {
    $e = { param($o, $n, $i) $o + [int][math]::Floor([int64]$i * $n / 48) }
    $x = & $e $Work.Left $Work.W $Grid.col
    $y = & $e $Work.Top $Work.H $Grid.row
    [PSCustomObject]@{
        X = $x; Y = $y
        W = (& $e $Work.Left $Work.W ($Grid.col + $Grid.w)) - $x
        H = (& $e $Work.Top $Work.H ($Grid.row + $Grid.h)) - $y
    }
}

function Test-SameRect($A, $B) { [bool]($A -and $B -and $A.X -eq $B.X -and $A.Y -eq $B.Y -and $A.W -eq $B.W -and $A.H -eq $B.H) }

# 兩矩形相交（邊緣相接不算）。
function Test-RectsIntersect($A, $B) {
    ($A.X -lt ($B.X + $B.W)) -and ($B.X -lt ($A.X + $A.W)) -and ($A.Y -lt ($B.Y + $B.H)) -and ($B.Y -lt ($A.Y + $A.H))
}

# settings.json 文字 → id → @{ Enabled; Monitor; col; row; w; h; Key }（Key＝比對「placement 不變」用的字串）。
function Get-PlacementMap([string]$SettingsJson) {
    $obj = $SettingsJson | ConvertFrom-Json
    $map = [ordered]@{}
    foreach ($prop in $obj.widgets.PSObject.Properties) {
        $p = $prop.Value.placement
        $mon = ($p.monitor | ConvertTo-Json -Compress -Depth 5)
        $map[$prop.Name] = [PSCustomObject]@{
            Enabled = [bool]$prop.Value.enabled; Monitor = $mon
            col = [int]$p.col; row = [int]$p.row; w = [int]$p.w; h = [int]$p.h
            Key = "monitor=$mon col=$($p.col) row=$($p.row) w=$($p.w) h=$($p.h)"
        }
    }
    return $map
}

# 兩份 placement 是否完全相同；回傳不同的 id 清單（空＝相同）。
function Compare-PlacementMap($Before, $After) {
    $ids = @(@($Before.Keys) + @($After.Keys) | Sort-Object -Unique)
    @($ids | Where-Object { -not ($Before.Contains($_) -and $After.Contains($_) -and $Before[$_].Key -eq $After[$_].Key) })
}

function Get-RelayoutApplied([string]$Line) {
    if ($Line -match 'applied=(\d+)') { return [int]$Matches[1] }
    return -1
}

# 第 StartIndex 行（0 起算）之後最後一個 display-change 的 RELAYOUT 行與 WM_DISPLAYCHANGE 事件。
function Get-DisplayChangeLog([string[]]$Lines = @(), [int]$StartIndex = 0) {
    $relayout = $null
    $sawEvent = $false
    for ($i = [Math]::Max(0, $StartIndex); $i -lt $Lines.Count; $i++) {
        if ($Lines[$i] -match 'EVENT WM_DISPLAYCHANGE') { $sawEvent = $true }
        if ($Lines[$i] -match 'RELAYOUT reason=[a-z,-]*display-change') { $relayout = $Lines[$i] }
    }
    [PSCustomObject]@{
        HasEvent = $sawEvent; RelayoutLine = $relayout
        Applied = $(if ($relayout) { Get-RelayoutApplied $relayout } else { -1 })
    }
}

# 逐扇核對：Windows＝@(@{ Id; X; Y; W; H })（可見小工具），Placements＝Get-PlacementMap 結果，
# Work＝@{ Left; Top; W; H }。回傳 @{ Ok; Lines; Mismatched; Overlaps; Missing; Unexpected }。
function Test-WidgetGridLayout($Windows, $Placements, $Work) {
    $lines = New-Object System.Collections.Generic.List[string]
    $mismatched = @(); $overlaps = @(); $missing = @(); $unexpected = @()
    $wins = @($Windows)
    $enabledIds = @($Placements.Keys | Where-Object { $Placements[$_].Enabled })
    foreach ($id in $enabledIds) {
        if (-not (@($wins | Where-Object { $_.Id -eq $id }).Count -eq 1)) { $missing += $id; $lines.Add("$id：找不到恰好一個可見視窗") }
    }
    foreach ($w in $wins) {
        if (-not ($Placements.Contains($w.Id) -and $Placements[$w.Id].Enabled)) { $unexpected += $w.Id; $lines.Add("$($w.Id)：可見但 settings.json 未啟用"); continue }
        $pl = $Placements[$w.Id]
        $g = Get-GridRectPx $Work $pl
        $same = Test-SameRect $w $g
        if (-not $same) { $mismatched += $w.Id }
        $lines.Add("$($w.Id)：視窗=($($w.X),$($w.Y),$($w.W)x$($w.H)) 格線($($pl.col),$($pl.row),$($pl.w),$($pl.h))=($($g.X),$($g.Y),$($g.W)x$($g.H)) 相符=$same")
    }
    for ($i = 0; $i -lt $wins.Count; $i++) {
        for ($j = $i + 1; $j -lt $wins.Count; $j++) {
            if (Test-RectsIntersect $wins[$i] $wins[$j]) { $overlaps += "$($wins[$i].Id)×$($wins[$j].Id)"; $lines.Add("相交：$($wins[$i].Id) 與 $($wins[$j].Id)") }
        }
    }
    [PSCustomObject]@{
        Ok = ($wins.Count -gt 0 -and $mismatched.Count -eq 0 -and $overlaps.Count -eq 0 -and $missing.Count -eq 0 -and $unexpected.Count -eq 0)
        Lines = $lines.ToArray(); Mismatched = $mismatched; Overlaps = $overlaps; Missing = $missing; Unexpected = $unexpected
    }
}

function Test-SameMode($A, $B) {
    [bool]($A -and $B -and $A.dmPelsWidth -eq $B.dmPelsWidth -and $A.dmPelsHeight -eq $B.dmPelsHeight -and
        $A.dmDisplayFrequency -eq $B.dmDisplayFrequency -and $A.dmDisplayOrientation -eq $B.dmDisplayOrientation)
}

# ---------------------------------------------------------------- 原生呼叫

[void][B6.Native]::SetThreadDpiAwarenessContext([IntPtr](-4))  # Per-Monitor-V2：實體像素

$ENUM_CURRENT_SETTINGS = -1
$DISP_CHANGE_SUCCESSFUL = 0

function Get-Ts { Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fffK' }
function Test-Locked { [bool](Get-Process -Name LogonUI -ErrorAction SilentlyContinue) }
function New-DevMode {
    $dm = New-Object B6.Native+DEVMODE
    $dm.dmSize = [System.Runtime.InteropServices.Marshal]::SizeOf([type][B6.Native+DEVMODE])
    return $dm
}
function Format-Mode($m) {
    if (-not $m) { return '(讀不到)' }
    "$($m.dmPelsWidth)x$($m.dmPelsHeight)@$($m.dmDisplayFrequency)Hz bpp=$($m.dmBitsPerPel) orient=$($m.dmDisplayOrientation)"
}
# 目前模式；讀不到回傳 $null（呼叫端不得拿它去還原）。
function Get-CurrentMode([string]$Device) {
    $m = New-DevMode
    if (-not [B6.Native]::EnumDisplaySettingsExW($Device, $ENUM_CURRENT_SETTINGS, [ref]$m, 0)) { return $null }
    return $m
}

# 主螢幕工作區（實體像素）。
function Get-PrimaryWork {
    $pt = New-Object B6.Native+GPOINT
    $mi = New-Object B6.Native+MONITORINFO
    $mi.cbSize = [Runtime.InteropServices.Marshal]::SizeOf($mi)
    [void][B6.Native]::GetMonitorInfoW([B6.Native]::MonitorFromPoint($pt, 1), [ref]$mi)
    $wa = $mi.rcWork
    [PSCustomObject]@{ Left = $wa.Left; Top = $wa.Top; W = $wa.Right - $wa.Left; H = $wa.Bottom - $wa.Top }
}

# 宿主的可見小工具視窗（標題 `fc-host <id>`）。
function Get-WidgetWindows([int]$ProcId) {
    $list = @()
    $h = [B6.Native]::GetTopWindow([IntPtr]::Zero)
    while ($h -ne [IntPtr]::Zero) {
        $p = 0
        [void][B6.Native]::GetWindowThreadProcessId($h, [ref]$p)
        if ($p -eq $ProcId -and [B6.Native]::IsWindowVisible($h)) {
            $sb = New-Object System.Text.StringBuilder 256
            [void][B6.Native]::GetWindowText($h, $sb, 256)
            $title = $sb.ToString()
            if ($title -match '^fc-host (\S+)$') {
                $r = New-Object B6.Native+RECT
                [void][B6.Native]::GetWindowRect($h, [ref]$r)
                $list += [PSCustomObject]@{
                    Id = $Matches[1]; Hwnd = $h; Title = $title
                    X = $r.Left; Y = $r.Top; W = $r.Right - $r.Left; H = $r.Bottom - $r.Top
                    Dpi = [B6.Native]::GetDpiForWindow($h)
                }
            }
        }
        $h = [B6.Native]::GetWindow($h, 2)
    }
    return $list
}

function Wait-WidgetWindows([int]$ProcId, [int]$Expected, [int]$TimeoutSec = 20) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt $TimeoutSec) {
        $wins = @(Get-WidgetWindows $ProcId)
        if ($wins.Count -eq $Expected) { return $wins }
        Start-Sleep -Milliseconds 300
    }
    return @(Get-WidgetWindows $ProcId)
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$log = New-EvidenceWriter (Join-Path $OutDir 'B6-resolution-log.log')
$log.AutoFlush = $true
function Wl([string]$s) { $line = "$(Get-Ts) $s"; $log.WriteLine($line); Write-Host $line }

# 還原原始模式：先查鎖定（鎖定就等，最多 $WaitSec 秒；仍鎖定不呼叫），再呼叫並讀回比對。
# 回傳 @{ Ok; Blocked; Rc; Current }。
function Restore-OriginalMode([string]$Device, $Orig, [int]$WaitSec, [string]$Tag) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ((Test-Locked) -and $sw.Elapsed.TotalSeconds -lt $WaitSec) { Start-Sleep -Seconds 2 }
    if (Test-Locked) {
        $cur = Get-CurrentMode $Device
        Wl "BLOCKED（$Tag）：工作階段鎖定 $WaitSec 秒未解除，未呼叫還原；解析度仍為 $(Format-Mode $cur)（原始 $(Format-Mode $Orig)）。解鎖後請重跑本腳本或在「設定 → 顯示器」手動改回。"
        return [PSCustomObject]@{ Ok = $false; Blocked = $true; Rc = $null; Current = $cur }
    }
    $o = $Orig
    $rc = [B6.Native]::ChangeDisplaySettingsExW($Device, [ref]$o, [IntPtr]::Zero, 0, [IntPtr]::Zero)
    Start-Sleep -Seconds 3
    $cur = Get-CurrentMode $Device
    $ok = ($rc -eq $DISP_CHANGE_SUCCESSFUL) -and (Test-SameMode $cur $Orig)
    Wl "還原（$Tag）：ChangeDisplaySettingsExW 回傳碼=$rc 讀回=$(Format-Mode $cur) 原始=$(Format-Mode $Orig) 還原成功=$ok"
    return [PSCustomObject]@{ Ok = $ok; Blocked = $false; Rc = $rc; Current = $cur }
}

# 前置防呆一律在任何 try／finally 之前檢查並直接結束（fix F4 共通規則）：原本「已有 fc-host」
# 在 try 內 throw，finally 會以行程名稱把既有的宿主全部砍掉（review B-batch3 high）。
if (Test-Locked) { Wl 'BLOCKED：LogonUI.exe 在跑（工作階段鎖定），已停止。'; $log.Close(); exit 2 }
if (Get-Process -Name fc-host -ErrorAction SilentlyContinue) { Wl '已有 fc-host 在執行，請先手動結束再重跑本腳本（本腳本不會停止它）。'; $log.Close(); exit 1 }
if (-not (Test-Path $Exe)) { Wl "找不到 $Exe，先在 host/ 執行 cargo build --release"; $log.Close(); exit 1 }
$Exe = (Resolve-Path $Exe).Path

$regSnap = @(Save-FcHostAutostartRegistry)
try { Wl "開機自啟登錄快照：$(($regSnap | ForEach-Object { "$($_.Name)@$(Split-Path $_.Key -Leaf) Exists=$($_.Exists)" }) -join '; ')" } catch { }
$exitCode = 0
$verdict = 'FAIL'
$proc = $null
$tempRoot = $null
$deviceName = $null
$orig = $null
$needRevert = $false
$revertedOk = $false
$restoreBlocked = $false

try {
    Add-Type -AssemblyName System.Windows.Forms
    Add-Type -AssemblyName System.Drawing
    $deviceName = [System.Windows.Forms.Screen]::PrimaryScreen.DeviceName
    Wl "目標裝置：$deviceName（.NET Screen.PrimaryScreen.DeviceName，文件化 API，避免寫死 \\.\DISPLAYn 編號）"

    # 1) 記錄原始模式：讀不到就中止（$orig 維持 $null，finally 不會拿全零模式去還原）。
    $orig = Get-CurrentMode $deviceName
    if (-not $orig) { throw 'EnumDisplaySettingsExW(ENUM_CURRENT_SETTINGS) 失敗，讀不到目前解析度，中止（不做任何變更）' }
    Wl "原始模式：$(Format-Mode $orig)"

    # 2) 列舉支援的模式，找一個同色深、不同尺寸的候選（優先 1920x1200，找不到任選一個不同尺寸的）
    $candidates = @{}
    $i = 0
    while ($true) {
        $m = New-DevMode
        if (-not [B6.Native]::EnumDisplaySettingsExW($deviceName, $i, [ref]$m, 0)) { break }
        if ($m.dmBitsPerPel -eq $orig.dmBitsPerPel -and
            -not ($m.dmPelsWidth -eq $orig.dmPelsWidth -and $m.dmPelsHeight -eq $orig.dmPelsHeight)) {
            $key = "$($m.dmPelsWidth)x$($m.dmPelsHeight)@$($m.dmDisplayFrequency)"
            if (-not $candidates.ContainsKey($key)) { $candidates[$key] = $m }
        }
        $i++
    }
    Wl "列舉 $i 筆模式，候選（同色深、不同尺寸）$($candidates.Count) 筆"
    $preferKey = ($candidates.Keys | Where-Object { $_ -like '1920x1200@*' } | Sort-Object -Descending | Select-Object -First 1)
    if (-not $preferKey) { $preferKey = ($candidates.Keys | Sort-Object -Descending | Select-Object -First 1) }
    if (-not $preferKey) { throw '找不到任何可切換的替代解析度' }
    $target = $candidates[$preferKey]
    Wl "選定候選模式：$(Format-Mode $target)"

    # 3) 啟動宿主（暫存 APPDATA/LOCALAPPDATA，首次啟動）
    $tempRoot = Join-Path ([IO.Path]::GetTempPath()) ("fc-host-B6-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
    $tempAppData = Join-Path $tempRoot 'Roaming'
    $tempLocalAppData = Join-Path $tempRoot 'Local'
    New-Item -ItemType Directory -Force -Path $tempAppData, $tempLocalAppData | Out-Null
    Wl "暫存 APPDATA=$tempAppData LOCALAPPDATA=$tempLocalAppData"
    $oldAppData = $env:APPDATA
    $oldLocalAppData = $env:LOCALAPPDATA
    try {
        $env:APPDATA = $tempAppData
        $env:LOCALAPPDATA = $tempLocalAppData
        $proc = Start-Process -FilePath $Exe -PassThru
        Wl "啟動 fc-host pid=$($proc.Id)"
    } finally {
        $env:APPDATA = $oldAppData
        $env:LOCALAPPDATA = $oldLocalAppData
    }

    $cfgDir = Join-Path $tempAppData 'tw.fintools.fc-host'
    $settingsPath = Join-Path $cfgDir 'settings.json'
    $gkLog = Join-Path $cfgDir 'gatekeeper.log'
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while (-not (Test-Path -LiteralPath $settingsPath) -and $sw.Elapsed.TotalSeconds -lt 20) { Start-Sleep -Milliseconds 300 }
    if (-not (Test-Path -LiteralPath $settingsPath)) { throw "等不到首次啟動的 settings.json：$settingsPath" }

    # 4) 變更前：placement 快照、各扇矩形，核對前提（格線相符、兩兩不相交）。
    $placeBefore = Get-PlacementMap (Get-Content -LiteralPath $settingsPath -Raw)
    $enabledCount = @($placeBefore.Keys | Where-Object { $placeBefore[$_].Enabled }).Count
    Wl "settings.json（變更前）啟用 $enabledCount 扇：$((@($placeBefore.Keys | Where-Object { $placeBefore[$_].Enabled }) | ForEach-Object { "$_[$($placeBefore[$_].Key)]" }) -join '; ')"
    $winsBefore = @(Wait-WidgetWindows $proc.Id $enabledCount 20)
    $workBefore = Get-PrimaryWork
    $chkBefore = Test-WidgetGridLayout $winsBefore $placeBefore $workBefore
    Wl "變更前工作區 ($($workBefore.Left),$($workBefore.Top),$($workBefore.W)x$($workBefore.H))，可見小工具 $($winsBefore.Count) 扇，逐扇核對 Ok=$($chkBefore.Ok)"
    foreach ($l in $chkBefore.Lines) { Wl "  [變更前] $l" }
    if (-not $chkBefore.Ok) { throw 'PRECONDITION：變更前各扇就不在格線位置或彼此相交，不改解析度' }

    # 截圖：只裁切 fixed 小工具視窗區域（含少量邊界，不含桌面圖示）——輔助判讀，非判準。
    function Save-Crop([string]$Path, $Rect) {
        $pad = 8
        $x = [Math]::Max(0, $Rect.X - $pad); $y = [Math]::Max(0, $Rect.Y - $pad)
        $w = $Rect.W + $pad * 2; $h = $Rect.H + $pad * 2
        $bmp = New-Object System.Drawing.Bitmap $w, $h
        $g = [System.Drawing.Graphics]::FromImage($bmp)
        $g.CopyFromScreen($x, $y, 0, 0, $bmp.Size)
        $g.Dispose()
        $bmp.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png)
        $bmp.Dispose()
    }
    $fixedBefore = $winsBefore | Where-Object { $_.Id -eq 'fixed' } | Select-Object -First 1
    if ($fixedBefore) {
        $beforeShot = Join-Path $OutDir 'B6-resolution-before-crop.png'
        Save-Crop $beforeShot $fixedBefore
        Wl "已存變更前裁切截圖 $beforeShot"
    }

    # 5) 套用新解析度（dwFlags=0：不含 CDS_UPDATEREGISTRY，只改動態模式）。
    #    「需要還原」旗標在呼叫之前設好（共通規則）；套用後立即讀回，不符就中止（finally 還原）。
    $gkBaseline = @(if (Test-Path -LiteralPath $gkLog) { Get-Content -LiteralPath $gkLog }).Count
    Wl "gatekeeper.log 基準行數=$gkBaseline（之後的記錄才納入判定）"
    if (Test-Locked) { throw 'BLOCKED：改解析度前偵測到鎖定，停止' }
    $needRevert = $true
    $t = $target
    $rc = [B6.Native]::ChangeDisplaySettingsExW($deviceName, [ref]$t, [IntPtr]::Zero, 0, [IntPtr]::Zero)
    Wl "ChangeDisplaySettingsExW → 目標模式，回傳碼=$rc（0=DISP_CHANGE_SUCCESSFUL）"
    if ($rc -ne $DISP_CHANGE_SUCCESSFUL) { throw "解析度變更失敗，回傳碼=$rc" }
    Start-Sleep -Seconds 2
    $after = Get-CurrentMode $deviceName
    Wl "變更後讀回模式：$(Format-Mode $after)"
    if (-not ($after -and $after.dmPelsWidth -eq $target.dmPelsWidth -and $after.dmPelsHeight -eq $target.dmPelsHeight)) {
        throw "變更後讀回與目標不符（讀回 $(Format-Mode $after)，目標 $(Format-Mode $target)），中止後續步驟"
    }

    Start-Sleep -Seconds $WaitAfterChangeSec

    # 6) 變更後逐扇核對、placement 不變、RELAYOUT applied=可見數。
    $winsAfter = @(Wait-WidgetWindows $proc.Id $enabledCount 10)
    $workAfter = Get-PrimaryWork
    $placeAfter = Get-PlacementMap (Get-Content -LiteralPath $settingsPath -Raw)
    $chkAfter = Test-WidgetGridLayout $winsAfter $placeAfter $workAfter
    Wl "變更後工作區 ($($workAfter.Left),$($workAfter.Top),$($workAfter.W)x$($workAfter.H))，可見小工具 $($winsAfter.Count) 扇，逐扇核對 Ok=$($chkAfter.Ok)"
    foreach ($l in $chkAfter.Lines) { Wl "  [變更後] $l" }
    $placeDiff = @(Compare-PlacementMap $placeBefore $placeAfter)
    $placeSame = ($placeDiff.Count -eq 0)
    Wl "settings.json placement 變更前後相同=$placeSame$(if (-not $placeSame) { '；不同：' + ($placeDiff -join ',') })"
    $fixedAfter = $winsAfter | Where-Object { $_.Id -eq 'fixed' } | Select-Object -First 1
    if ($fixedAfter) {
        $afterShot = Join-Path $OutDir 'B6-resolution-after-crop.png'
        Save-Crop $afterShot $fixedAfter
        Wl "已存變更後裁切截圖 $afterShot"
        # 解析度切換時 Windows 會依該模式各自記憶的建議縮放比例自動改變有效 DPI（本機實測
        # 2560x1600→175%、1920x1200→150%，非本腳本觸發、非 host bug）；格線版面下位置只由格座標
        # 與工作區決定，與 DPI 無關。
        if ($fixedBefore) { Wl "DPI 是否隨解析度改變：$($fixedBefore.Dpi)→$($fixedAfter.Dpi)（Windows 依解析度模式記憶的建議縮放比例，非小工具定位邏輯）" }
    }

    Start-Sleep -Milliseconds 500
    $gkLines = @(if (Test-Path -LiteralPath $gkLog) { Get-Content -LiteralPath $gkLog })
    $dc = Get-DisplayChangeLog -Lines $gkLines -StartIndex $gkBaseline
    $appliedOk = ($dc.Applied -ge 0) -and ($dc.Applied -eq $winsAfter.Count)
    Wl "gatekeeper.log（基準行之後）EVENT WM_DISPLAYCHANGE=$($dc.HasEvent) 最後一行 display-change RELAYOUT=[$($dc.RelayoutLine)] applied=$($dc.Applied) 可見小工具=$($winsAfter.Count) 相符=$appliedOk"

    # 7) 還原原始模式（查鎖定、讀回）。
    $r = Restore-OriginalMode $deviceName $orig $UnlockWaitSec '主流程'
    $revertedOk = $r.Ok
    $restoreBlocked = $r.Blocked

    $verdict = if ($chkAfter.Ok -and $placeSame -and $dc.HasEvent -and $appliedOk -and $revertedOk) { 'PASS' } else { 'FAIL' }
    Wl "判準：逐扇格線且兩兩不相交=$($chkAfter.Ok) placement 不變=$placeSame WM_DISPLAYCHANGE=$($dc.HasEvent) applied=可見數=$appliedOk 還原讀回相符=$revertedOk"
    Wl "=== 結論：$verdict ==="
    if ($verdict -ne 'PASS') { $exitCode = 1 }
} catch {
    Wl "例外：$_"
    $exitCode = 1
} finally {
    try {
        # 只停本腳本啟動的宿主與其子孫，不以行程名稱停止（fix F4 共通規則）。
        if ($proc) { [void](Stop-ProcessTree -Process $proc); Wl "已停止本腳本啟動的 fc-host pid=$($proc.Id) 及其子孫" }
    } catch {}
    try {
        if ($tempRoot -and (Test-Path $tempRoot)) { Remove-Item -Recurse -Force $tempRoot -ErrorAction SilentlyContinue; Wl "已刪暫存目錄 $tempRoot" }
    } catch {}
    # 保險還原：只在「已改過」且主流程沒有還原成功時；同樣查鎖定、檢查回傳碼並讀回。
    if ($needRevert -and -not $revertedOk -and $orig) {
        try {
            $r2 = Restore-OriginalMode $deviceName $orig $(if ($restoreBlocked) { 30 } else { $UnlockWaitSec }) 'finally 保險還原'
            $revertedOk = $r2.Ok
            $restoreBlocked = $r2.Blocked
        } catch { Wl "保險還原例外：$_（解析度可能仍未還原，原始 $(Format-Mode $orig)）" }
        if (-not $revertedOk) {
            Wl "!!! 解析度未還原：原始 $(Format-Mode $orig)；目前 $(Format-Mode (Get-CurrentMode $deviceName))"
            $exitCode = $(if ($restoreBlocked) { 2 } else { 1 })
        }
    }
    $regLeft = @(Restore-FcHostAutostartRegistry $regSnap)
    if ($regLeft.Count -gt 0) { Write-Warning "開機自啟登錄未還原：$($regLeft -join '; ')" }
    try { Wl "開機自啟登錄還原：未還原 $($regLeft.Count) 項$(if ($regLeft.Count) { '：' + ($regLeft -join '; ') })" } catch { }
    if ($regLeft.Count -gt 0 -and $exitCode -eq 0) { $exitCode = 1 }
    $log.Close()
}
exit $exitCode
