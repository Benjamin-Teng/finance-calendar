<#
.SYNOPSIS
    安裝／更新／解除安裝實機測試用的「快照、還原、比對」核心（installer-auto-update task 4.1、design.md D7）。

.DESCRIPTION
    本模組含機制與「正式受管清單」：快照／還原／比對函式吃 $Targets；正式清單寫死在 New-FcRealTargets，路徑來自
    Resolve-FcProfilePaths（known folder，環境變數不一致就拒絕）。CLI host/tools/installer-snapshot.ps1
    不接受任何路徑參數，只收快照目錄；自測
    （host/tools/tests/InstallerSnapshot.Tests.ps1）傳入暫存目錄與 HKCU:\Software\fc-host-snapshot-test\...
    的假登錄樹，所以測試不會碰到真實位置。

    $Targets（Hashtable）：
      Dirs      = @( @{ Id; Path } ... )                  整個目錄（複製＋逐檔清單；不存在就記「不存在」）
      Files     = @( @{ Id; Path } ... )                  單一檔案（開始功能表捷徑）
      RegValues = @( @{ Id; Key; Name } ... )             HKCU 下單一值（不存在與空字串分開記）
      RegTrees  = @( @{ Id; Key; PruneParentKey } ... )   HKCU 下整棵子樹；PruneParentKey 選填，
                                                         還原後若快照時它不存在且現在是空鍵就刪掉
                                                         （對應 NSIS 的 DeleteRegKey /ifempty）
      Watch     = @( @{ Id; Path; Include } ... )         僅回報：只記清單、只比對、絕不還原（explorer 的桌布轉存工作檔）
      Wallpaper = $true／$false                           是否做各螢幕桌布讀回（只讀、只比對）
    $Ctx（New-FcSnapshotContext）：IsHostRunning、ReadWallpaper、HashLimitBytes。

    快照目錄版面：snapshot.json（最後才寫，Save 結束時自我驗證通過才保留）、manifest-<Id>.json、
    watch-<Id>.json、payload\dirs\<Id>\、payload\files\<Id>、registry.json、wallpaper.json。

    比對判準：目錄＝相對路徑集合＋大小＋（<= HashLimitBytes 的檔案）SHA256／（更大的檔案）修改時間；
    登錄＝每個值的型別＋原始資料（不展開環境變數）；桌布＝各螢幕 GetWallpaper、全域 GetPosition／
    GetBackgroundColor／GetStatus。桌布只比對、絕不設定（設桌布歸宿主的 --restore-wallpaper）。
#>
#Requires -Version 7.5
Set-StrictMode -Version Latest

# ── 桌布讀回（C# 靜態 helper：PowerShell 不能直接呼叫 COM 介面方法）──────────────────────────
# 只呼叫 Get*／GetMonitorDevicePath*／GetMonitorRECT；介面其餘方法只為保住 vtable 順序而宣告，絕不呼叫。
# vtable 順序與 GUID 對照 windows-0.62.2 crate 的 IDesktopWallpaper_Vtbl（B92B56A9-…／C2CF3110-…）。
if (-not ('FcSnapshot.WallpaperReader' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Threading;

namespace FcSnapshot {
    [StructLayout(LayoutKind.Sequential)]
    public struct NativeRect { public int Left, Top, Right, Bottom; }

    [ComImport, Guid("B92B56A9-8B55-4E14-9A89-0199BBB6F93B"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    internal interface IDesktopWallpaper {
        void SetWallpaper([MarshalAs(UnmanagedType.LPWStr)] string monitorID, [MarshalAs(UnmanagedType.LPWStr)] string wallpaper);
        void GetWallpaper([MarshalAs(UnmanagedType.LPWStr)] string monitorID, [MarshalAs(UnmanagedType.LPWStr)] out string wallpaper);
        void GetMonitorDevicePathAt(uint monitorIndex, [MarshalAs(UnmanagedType.LPWStr)] out string monitorID);
        void GetMonitorDevicePathCount(out uint count);
        void GetMonitorRECT([MarshalAs(UnmanagedType.LPWStr)] string monitorID, out NativeRect displayRect);
        void SetBackgroundColor(uint color);
        void GetBackgroundColor(out uint color);
        void SetPosition(int position);
        void GetPosition(out int position);
        void SetSlideshow(IntPtr items);
        void GetSlideshow(out IntPtr items);
        void SetSlideshowOptions(uint options, uint slideshowTick);
        void GetSlideshowOptions(out uint options, out uint slideshowTick);
        void AdvanceSlideshow([MarshalAs(UnmanagedType.LPWStr)] string monitorID, int direction);
        void GetStatus(out int state);
        void Enable([MarshalAs(UnmanagedType.Bool)] bool enable);
    }

    public class MonitorWallpaper {
        public string Id;
        public string Path;
        public int Left, Top, Right, Bottom;
    }

    public class WallpaperInfo {
        public bool Ok;
        public string Error;
        public string Global;
        public int Position;
        public uint Color;
        public int Status;
        public List<MonitorWallpaper> Monitors = new List<MonitorWallpaper>();
    }

    public static class WallpaperReader {
        // 在獨立 STA 執行緒讀取（只讀）；任何 COM 失敗都回報在 Error、不丟例外。
        public static WallpaperInfo Read() {
            WallpaperInfo info = new WallpaperInfo();
            Thread t = new Thread(delegate () { ReadCore(info); });
            t.SetApartmentState(ApartmentState.STA);
            t.Start();
            t.Join();
            return info;
        }

        private static void ReadCore(WallpaperInfo info) {
            object com = null;
            try {
                Type type = Type.GetTypeFromCLSID(new Guid("C2CF3110-460E-4FC1-B9D0-8A1C0C9CC4BD"), true);
                com = Activator.CreateInstance(type);
                IDesktopWallpaper dw = (IDesktopWallpaper)com;
                uint count;
                dw.GetMonitorDevicePathCount(out count);
                for (uint i = 0; i < count; i++) {
                    string id;
                    dw.GetMonitorDevicePathAt(i, out id);
                    MonitorWallpaper m = new MonitorWallpaper();
                    m.Id = id;
                    string path;
                    dw.GetWallpaper(id, out path);
                    m.Path = path ?? "";
                    try {
                        NativeRect r;
                        dw.GetMonitorRECT(id, out r);
                        m.Left = r.Left; m.Top = r.Top; m.Right = r.Right; m.Bottom = r.Bottom;
                    } catch (Exception) { }
                    info.Monitors.Add(m);
                }
                string global;
                dw.GetWallpaper(null, out global);
                info.Global = global ?? "";
                dw.GetPosition(out info.Position);
                dw.GetBackgroundColor(out info.Color);
                dw.GetStatus(out info.Status);
                info.Ok = true;
            } catch (Exception e) {
                info.Ok = false;
                info.Error = e.GetType().Name + ": " + e.Message;
            } finally {
                if (com != null) { Marshal.ReleaseComObject(com); }
            }
        }
    }
}
'@
}

function Get-FcWallpaperReadback {
    <# 只讀：回傳 @{ Ok; Error; Global; Position; Color; Status; Monitors=@(@{Id;Path;Left;Top;Right;Bottom}) } #>
    $r = [FcSnapshot.WallpaperReader]::Read()
    $mons = @(foreach ($m in $r.Monitors) {
            [ordered]@{ Id = $m.Id; Path = $m.Path; Left = $m.Left; Top = $m.Top; Right = $m.Right; Bottom = $m.Bottom }
        })
    [ordered]@{
        Ok = $r.Ok; Error = $r.Error; Global = $r.Global; Position = $r.Position
        Color = [int64]$r.Color; Status = $r.Status; Monitors = $mons
    }
}

function New-FcSnapshotContext {
    @{
        # 回傳會讓快照／還原不可信的行程描述（空陣列＝沒有）
        IsHostRunning  = { Get-FcBlockingProcesses }
        ReadWallpaper  = { Get-FcWallpaperReadback }
        HashLimitBytes = 8MB
    }
}

# ── 行程檢查 ─────────────────────────────────────────────────────────────────────────────────
# 純函式（可用假行程清單測試）：哪些行程存在時不得快照／還原。
#   fc-host.exe                                   宿主本體
#   msedgewebview2.exe 且命令列含 tw.fintools.fc-host  宿主的 WebView2 行程（握著 EBWebView 的檔案，刪目錄會刪不乾淨）
#   finance-calendar-setup*.exe                   安裝檔正在跑
#   ...\fc-host\uninstall.exe                     安裝目錄內的解除安裝程式剛啟動
#   fc-host-<版本>-installer.exe                  更新器下載後啟動的安裝檔（tauri-plugin-updater 2.13.1 updater.rs:1111–1118，
#                                                 檔名＝<app_name>-<ver>-installer.exe，app_name＝productName fc-host）
#   %TEMP%\~nsu*.tmp\Un.exe                       NSIS 解除安裝程式把自己複製到暫存再執行的那個行程（真正在刪檔的是它，
#                                                 不是 uninstall.exe；NSIS Source/exehead/Main.c 的 "~nsu%X.tmp"）；以路徑樣式比對
function Select-FcBlockingProcesses($Infos) {
    @(foreach ($p in @($Infos)) {
            $name = [string]$p.Name
            $exe = [string]$p.ExecutablePath
            $cmd = [string]$p.CommandLine
            $hit = ($name -ieq 'fc-host.exe') -or
            ($name -ilike 'finance-calendar-setup*') -or
            ($name -ilike 'fc-host-*-installer.exe') -or
            (($name -ieq 'msedgewebview2.exe') -and ($cmd.IndexOf('tw.fintools.fc-host', [System.StringComparison]::OrdinalIgnoreCase) -ge 0)) -or
            (($name -ieq 'uninstall.exe') -and $exe.EndsWith('\fc-host\uninstall.exe', [System.StringComparison]::OrdinalIgnoreCase)) -or
            (($name -ieq 'Un.exe') -and ($exe -ilike '*\~nsu*.tmp\Un.exe'))
            if ($hit) { "$name#$($p.ProcessId)" }
        })
}

function Get-FcBlockingProcesses {
    $filter = "Name='fc-host.exe' OR Name='msedgewebview2.exe' OR Name='uninstall.exe' OR Name='Un.exe' OR Name LIKE 'finance-calendar-setup%' OR Name LIKE 'fc-host-%-installer.exe'"
    $infos = @(Get-CimInstance -ClassName Win32_Process -Filter $filter -ErrorAction Stop | ForEach-Object {
            [pscustomobject]@{ ProcessId = $_.ProcessId; Name = $_.Name; ExecutablePath = $_.ExecutablePath; CommandLine = $_.CommandLine }
        })
    Select-FcBlockingProcesses $infos
}

# ── 受管路徑：known folder 為準，環境變數不一致就拒絕（I4）─────────────────────────────────────
# NSIS 的 $LOCALAPPDATA／$APPDATA／$SMPROGRAMS／$DESKTOP 與宿主都走 known folder、不看環境變數；若在環境變數被覆寫過的
# 殼裡跑，快照到的會是假位置而真實位置沒被保護，所以不一致時直接拒絕。
function Resolve-FcProfilePaths {
    param(
        [scriptblock]$GetFolder = { param($Name) [System.Environment]::GetFolderPath($Name) },
        [hashtable]$EnvVars = @{ LOCALAPPDATA = $env:LOCALAPPDATA; APPDATA = $env:APPDATA }
    )
    $paths = @{}
    foreach ($pair in @(@('LocalAppData', 'LocalApplicationData'), @('AppData', 'ApplicationData'), @('Programs', 'Programs'), @('Desktop', 'Desktop'))) {
        $v = [string](& $GetFolder $pair[1])
        if ([string]::IsNullOrWhiteSpace($v) -or -not [System.IO.Path]::IsPathRooted($v)) {
            throw [System.InvalidOperationException]::new("拒絕：known folder $($pair[1]) 取不到有效路徑（'$v'）")
        }
        $paths[$pair[0]] = $v
    }
    foreach ($pair in @(@('LOCALAPPDATA', 'LocalAppData'), @('APPDATA', 'AppData'))) {
        $e = [string]$EnvVars[$pair[0]]
        if (-not [string]::IsNullOrWhiteSpace($e) -and ((Get-NormalPath $e) -ine (Get-NormalPath $paths[$pair[1]]))) {
            throw [System.InvalidOperationException]::new("拒絕：環境變數 $($pair[0])（$e）與 known folder（$($paths[$pair[1]])）不一致；安裝檔與宿主用的是 known folder，快照會落在錯的位置。請在未覆寫環境變數的殼裡執行。")
        }
    }
    $paths
}

# 正式的受管清單（路徑與鍵名全部寫死在這裡；命令列不接受任何路徑參數）。
function New-FcRealTargets {
    param([Parameter(Mandatory)][hashtable]$Paths)
    $themes = Join-Path $Paths.AppData 'Microsoft\Windows\Themes'
    @{
        Dirs      = @(
            @{ Id = 'install'; Path = (Join-Path $Paths.LocalAppData 'fc-host') }
            @{ Id = 'appdata'; Path = (Join-Path $Paths.AppData 'tw.fintools.fc-host') }
            @{ Id = 'localdata'; Path = (Join-Path $Paths.LocalAppData 'tw.fintools.fc-host') }
        )
        Files     = @(
            @{ Id = 'startmenu-fchost'; Path = (Join-Path $Paths.Programs 'fc-host.lnk') }
            @{ Id = 'startmenu-cn'; Path = (Join-Path $Paths.Programs '財經日曆.lnk') }
            @{ Id = 'desktop-fchost'; Path = (Join-Path $Paths.Desktop 'fc-host.lnk') }
            @{ Id = 'desktop-cn'; Path = (Join-Path $Paths.Desktop '財經日曆.lnk') }
        )
        RegValues = @(
            @{ Id = 'run'; Key = 'Software\Microsoft\Windows\CurrentVersion\Run'; Name = 'fc-host' }
            @{ Id = 'startupapproved'; Key = 'Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run'; Name = 'fc-host' }
            @{ Id = 'wallpaper'; Key = 'Control Panel\Desktop'; Name = 'Wallpaper' }
            @{ Id = 'wallpaperstyle'; Key = 'Control Panel\Desktop'; Name = 'WallpaperStyle' }
            @{ Id = 'tilewallpaper'; Key = 'Control Panel\Desktop'; Name = 'TileWallpaper' }
        )
        RegTrees  = @(
            @{ Id = 'uninstall'; Key = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\fc-host' }
            @{ Id = 'manuproduct'; Key = 'Software\fintools\fc-host'; PruneParentKey = 'Software\fintools' }
        )
        # 僅回報（只做 Save 與 Compare，絕不還原）：Windows 的桌布轉存檔是 explorer 的工作檔
        Watch     = @(
            @{ Id = 'themes'; Path = $themes; Include = @('TranscodedWallpaper', 'Transcoded_*', 'TranscodedWallpaperCache\*') }
        )
        Wallpaper = $true
    }
}

# ── 小工具 ───────────────────────────────────────────────────────────────────────────────────
function Write-FcJson([string]$Path, $Object) {
    $json = $Object | ConvertTo-Json -Depth 64 -Compress
    [System.IO.File]::WriteAllText($Path, $json, (New-Object System.Text.UTF8Encoding($false)))
}

function Read-FcJson([string]$Path) {
    # -DateKind String：ISO 日期樣式的字串值不得被轉成 DateTime（還原時會被改寫成地區格式，I2）
    [System.IO.File]::ReadAllText($Path, [System.Text.Encoding]::UTF8) | ConvertFrom-Json -DateKind String
}

function Get-NormalPath([string]$Path) {
    [System.IO.Path]::GetFullPath($Path).TrimEnd('\')
}

function Test-PathInside([string]$Child, [string]$Parent) {
    $c = (Get-NormalPath $Child) + '\'
    $p = (Get-NormalPath $Parent) + '\'
    $c.StartsWith($p, [System.StringComparison]::OrdinalIgnoreCase)
}

function Get-WatchTargets($Targets) {
    @(@($Targets.Watch) | Where-Object { $_ })
}

# 快照目錄不得位於任何會被還原刪除的位置之內（否則還原會把快照自己刪掉）
function Assert-SnapshotDirSafe([string]$Dir, $Targets) {
    $full = Get-NormalPath $Dir
    foreach ($d in @($Targets.Dirs)) {
        if (Test-PathInside $full $d.Path) { throw [System.InvalidOperationException]::new("拒絕：快照目錄 $full 位於會被還原刪除的目錄 $($d.Path) 之內") }
    }
    foreach ($f in @($Targets.Files)) {
        if ((Get-NormalPath $f.Path) -ieq $full) { throw [System.InvalidOperationException]::new("拒絕：快照目錄不得等於受管檔案 $($f.Path)") }
    }
}

# 刪除目錄前的最後防線（路徑只來自 $Targets，這裡擋空值、磁碟根目錄、過淺路徑與 reparse point）
function Assert-SafeDeleteDir([string]$Path) {
    if ([string]::IsNullOrWhiteSpace($Path) -or -not [System.IO.Path]::IsPathRooted($Path)) {
        throw [System.InvalidOperationException]::new("拒絕刪除：路徑不是絕對路徑 '$Path'")
    }
    $full = Get-NormalPath $Path
    $root = ([System.IO.Path]::GetPathRoot($full)).TrimEnd('\')
    $rest = $full.Substring($root.Length).Trim('\')
    if (($rest -split '\\').Count -lt 2) { throw [System.InvalidOperationException]::new("拒絕刪除：路徑過淺 '$full'") }
    if (Test-Path -LiteralPath $full) {
        $item = Get-Item -LiteralPath $full -Force
        if ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) {
            throw [System.InvalidOperationException]::new("拒絕刪除：'$full' 是 reparse point")
        }
    }
}

function Assert-SafeRegKey([string]$Key, [int]$MinDepth = 3) {
    if ([string]::IsNullOrWhiteSpace($Key) -or (($Key.Trim('\') -split '\\').Count -lt $MinDepth)) {
        throw [System.InvalidOperationException]::new("拒絕刪除登錄鍵：路徑過淺 '$Key'")
    }
}

function Assert-HostNotRunning($Ctx, [string]$What) {
    $running = @(& $Ctx.IsHostRunning)
    if ($running.Count -gt 0) {
        throw [System.InvalidOperationException]::new("拒絕$What：偵測到宿主或相關行程在跑（$($running -join ', ')）。請先自行結束（本腳本不會結束任何行程）。")
    }
}

# ── 目錄清單與複製 ───────────────────────────────────────────────────────────────────────────
function Get-FileSha256([string]$Path) {
    try {
        $fs = [System.IO.File]::Open($Path, 'Open', 'Read', 'ReadWrite, Delete')
        try {
            $sha = [System.Security.Cryptography.SHA256]::Create()
            try { return [Convert]::ToHexString($sha.ComputeHash($fs)) } finally { $sha.Dispose() }
        } finally { $fs.Dispose() }
    } catch {
        return '?unreadable'
    }
}

# 清單項：P＝相對路徑、T＝d／f、L＝大小、M＝修改時間 ticks、H＝SHA256（超過 HashLimit 的檔案為 $null）
function New-ManifestEntry($Info, [string]$RootFull, [long]$HashLimit) {
    $rel = [System.IO.Path]::GetRelativePath($RootFull, $Info.FullName)
    if ($Info -is [System.IO.DirectoryInfo]) { return [ordered]@{ P = $rel; T = 'd'; L = 0; M = 0; H = $null } }
    $hash = $null
    if ($Info.Length -le $HashLimit) { $hash = Get-FileSha256 $Info.FullName }
    [ordered]@{ P = $rel; T = 'f'; L = $Info.Length; M = $Info.LastWriteTimeUtc.Ticks; H = $hash }
}

# 受管目錄的清單；不存在回 $null。含 reparse point 時拒絕（robocopy 與列舉都會跟進去，還原也無法重建連結，M7）。
function Get-DirManifest([string]$Path, [long]$HashLimit) {
    if (-not (Test-Path -LiteralPath $Path -PathType Container)) { return $null }
    $root = [System.IO.DirectoryInfo]::new($Path)
    if ($root.Attributes -band [System.IO.FileAttributes]::ReparsePoint) {
        throw [System.InvalidOperationException]::new("拒絕：受管目錄 $Path 本身是 reparse point")
    }
    $opts = [System.IO.EnumerationOptions]::new()
    $opts.RecurseSubdirectories = $true
    $opts.AttributesToSkip = [System.IO.FileAttributes]0
    $opts.IgnoreInaccessible = $false
    $items = [System.Collections.Generic.List[object]]::new()
    foreach ($e in $root.EnumerateFileSystemInfos('*', $opts)) {
        if ($e.Attributes -band [System.IO.FileAttributes]::ReparsePoint) {
            throw [System.InvalidOperationException]::new("拒絕：受管目錄內含 reparse point（junction／symlink）$($e.FullName)")
        }
        $items.Add((New-ManifestEntry $e $root.FullName $HashLimit))
    }
    , @($items)
}

# 「僅回報」目錄的清單：Include 的樣式無反斜線＝該目錄第一層符合的檔案；有反斜線（子目錄\*）＝該子目錄整棵。
# 目錄不存在回 $null；reparse point 略過。
function Get-WatchManifest([string]$Path, $Include, [long]$HashLimit) {
    if (-not (Test-Path -LiteralPath $Path -PathType Container)) { return $null }
    $root = [System.IO.DirectoryInfo]::new($Path)
    $seen = @{}
    $items = [System.Collections.Generic.List[object]]::new()
    $add = {
        param($e)
        if ($e.Attributes -band [System.IO.FileAttributes]::ReparsePoint) { return }
        $entry = New-ManifestEntry $e $root.FullName $HashLimit
        if (-not $seen.ContainsKey($entry.P)) { $seen[$entry.P] = $true; $items.Add($entry) }
    }
    foreach ($pattern in @($Include)) {
        if ($pattern.Contains('\')) {
            $sub = $pattern.Substring(0, $pattern.IndexOf('\'))
            $subDir = [System.IO.DirectoryInfo]::new((Join-Path $Path $sub))
            if (-not $subDir.Exists -or ($subDir.Attributes -band [System.IO.FileAttributes]::ReparsePoint)) { continue }
            & $add $subDir
            $opts = [System.IO.EnumerationOptions]::new()
            $opts.RecurseSubdirectories = $true
            $opts.AttributesToSkip = [System.IO.FileAttributes]0
            foreach ($e in $subDir.EnumerateFileSystemInfos('*', $opts)) { & $add $e }
        } else {
            foreach ($e in $root.EnumerateFiles($pattern)) { & $add $e }
        }
    }
    , @($items)
}

function Copy-DirTree([string]$Src, [string]$Dst) {
    New-Item -ItemType Directory -Path $Dst -Force | Out-Null
    # /E 含空目錄；/COPY:DAT /DCOPY:DAT 保留資料、屬性、時間戳；/XJ 不跟 junction；結束碼 >= 8 才是失敗
    & robocopy.exe $Src $Dst /E /COPY:DAT /DCOPY:DAT /XJ /R:2 /W:1 /NFL /NDL /NJH /NJS /NP | Out-Null
    if ($LASTEXITCODE -ge 8) { throw "robocopy 失敗（結束碼 $LASTEXITCODE）：$Src -> $Dst" }
    $global:LASTEXITCODE = 0
}

function Remove-DirWithRetry([string]$Path) {
    Assert-SafeDeleteDir $Path
    for ($i = 1; $i -le 3; $i++) {
        try {
            if (Test-Path -LiteralPath $Path) { Remove-Item -LiteralPath $Path -Recurse -Force -ErrorAction Stop }
            return
        } catch {
            if ($i -eq 3) { throw }
            Start-Sleep -Seconds 1
        }
    }
}

# 兩份清單比對，差異寫進 $Out（List[string]）。$Saved＝快照的 { Exists; Items }，$Cur＝現況清單或 $null。
function Compare-ManifestInto($Saved, $Cur, [string]$Label, $Out) {
    if (-not $Saved.Exists -and $null -eq $Cur) { return }
    if (-not $Saved.Exists) { $Out.Add("${Label}：快照時不存在、現在存在"); return }
    if ($null -eq $Cur) { $Out.Add("${Label}：快照時存在、現在不存在"); return }
    $savedMap = @{}
    foreach ($e in @($Saved.Items)) { $savedMap[$e.P] = $e }
    $curMap = @{}
    foreach ($e in @($Cur)) { $curMap[$e.P] = $e }
    foreach ($p in ($savedMap.Keys | Sort-Object)) {
        if (-not $curMap.ContainsKey($p)) { $Out.Add("${Label}：缺少 $p"); continue }
        $a = $savedMap[$p]; $b = $curMap[$p]
        if ($a.T -ne $b.T) { $Out.Add("${Label}：$p 類型不同（$($a.T) → $($b.T)）"); continue }
        if ($a.T -eq 'd') { continue }
        if ($a.L -ne $b.L) { $Out.Add("${Label}：$p 大小不同（$($a.L) → $($b.L)）"); continue }
        if ($null -ne $a.H -or $null -ne $b.H) {
            if ($a.H -cne $b.H) { $Out.Add("${Label}：$p 內容不同（SHA256）") }
        } elseif ($a.M -ne $b.M) {
            $Out.Add("${Label}：$p 修改時間不同（大檔只比大小與修改時間）")
        }
    }
    foreach ($p in ($curMap.Keys | Sort-Object)) {
        if (-not $savedMap.ContainsKey($p)) { $Out.Add("${Label}：多出 $p") }
    }
}

# ── 登錄（HKCU 下）─────────────────────────────────────────────────────────────────────────
function ConvertTo-RegText($Kind, $Raw) {
    switch ([string]$Kind) {
        'Binary' { return [Convert]::ToBase64String([byte[]]$Raw) }
        'None' { return [Convert]::ToBase64String([byte[]]$Raw) }
        'MultiString' { return (@([string[]]$Raw) -join "`0") }
        default { return [string]$Raw }
    }
}

function ConvertFrom-RegText([string]$Kind, [string]$Text) {
    switch ($Kind) {
        'Binary' { return , ([Convert]::FromBase64String($Text)) }
        'None' { return , ([Convert]::FromBase64String($Text)) }
        'MultiString' { if ($Text -eq '') { return , ([string[]]@()) } else { return , ([string[]]($Text -split "`0")) } }
        'DWord' { return [int]::Parse($Text) }
        'QWord' { return [long]::Parse($Text) }
        default { return $Text }
    }
}

function Read-RegValue([string]$Key, [string]$Name) {
    $k = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($Key, $false)
    if (-not $k) { return $null }
    try {
        if (-not ($k.GetValueNames() -contains $Name)) { return $null }
        $kind = $k.GetValueKind($Name)
        $raw = $k.GetValue($Name, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        return @{ Kind = [string]$kind; Data = (ConvertTo-RegText $kind $raw) }
    } finally { $k.Close() }
}

function Set-RegValueRaw([string]$Key, [string]$Name, [string]$Kind, [string]$Text) {
    if ($Kind -eq 'Unknown') { throw "無法寫回型別 Unknown 的登錄值 $Key\$Name" }
    $k = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($Key)
    try {
        $value = ConvertFrom-RegText $Kind $Text
        $k.SetValue($Name, $value, [Microsoft.Win32.RegistryValueKind]::$Kind)
    } finally { $k.Close() }
}

function Remove-RegValueIfPresent([string]$Key, [string]$Name) {
    $k = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($Key, $true)
    if (-not $k) { return }
    try {
        if ($k.GetValueNames() -contains $Name) { $k.DeleteValue($Name, $false) }
    } finally { $k.Close() }
}

# 子樹節點：@{ Vals = @(@{Name;Kind;Data}); Subs = @(@{Name;Node}) }（刻意不用 Values，Hashtable 有同名內建屬性）
function Read-RegNode([string]$Key) {
    $k = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($Key, $false)
    if (-not $k) { return $null }
    try {
        $vals = @(foreach ($n in $k.GetValueNames()) {
                $kind = $k.GetValueKind($n)
                $raw = $k.GetValue($n, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
                [ordered]@{ Name = $n; Kind = [string]$kind; Data = (ConvertTo-RegText $kind $raw) }
            })
        $subs = @(foreach ($s in $k.GetSubKeyNames()) {
                [ordered]@{ Name = $s; Node = (Read-RegNode "$Key\$s") }
            })
        return [ordered]@{ Vals = $vals; Subs = $subs }
    } finally { $k.Close() }
}

function Write-RegNode([string]$Key, $Node) {
    $k = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($Key)
    $k.Close()
    foreach ($v in @($Node.Vals)) { Set-RegValueRaw $Key $v.Name $v.Kind $v.Data }
    foreach ($s in @($Node.Subs)) { Write-RegNode "$Key\$($s.Name)" $s.Node }
}

function ConvertTo-RegFlat($Node, [string]$Prefix, [hashtable]$Acc) {
    foreach ($v in @($Node.Vals)) { $Acc["V|$Prefix|$($v.Name)"] = "$($v.Kind)|$($v.Data)" }
    foreach ($s in @($Node.Subs)) {
        $p = if ($Prefix) { "$Prefix\$($s.Name)" } else { $s.Name }
        $Acc["K|$p"] = ''
        ConvertTo-RegFlat $s.Node $p $Acc
    }
}

function Format-RegSig([string]$Sig) {
    if ($Sig.Length -gt 80) { $Sig.Substring(0, 80) + '…' } else { $Sig }
}

function Test-RegKeyExists([string]$Key) {
    $k = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($Key, $false)
    if ($k) { $k.Close(); return $true }
    return $false
}

# ── Save ─────────────────────────────────────────────────────────────────────────────────────
# 回傳 { Complete; Differences; ReportOnly; Warnings }。寫完後自動 Compare：不是零差異就把快照標成不完整（刪掉 snapshot.json），
# 任何序列化往返、複製期間檔案變動的問題都在測試前被擋下（I2、M6）。-WhatIf 回傳 $null。
function Save-FcSnapshot {
    param([Parameter(Mandatory)][string]$Dir, [Parameter(Mandatory)]$Targets, [Parameter(Mandatory)]$Ctx, [switch]$WhatIf)
    Assert-SnapshotDirSafe $Dir $Targets
    Assert-HostNotRunning $Ctx '快照'
    if ((Test-Path -LiteralPath $Dir) -and @(Get-ChildItem -LiteralPath $Dir -Force).Count -gt 0) {
        throw [System.InvalidOperationException]::new("拒絕：快照目錄 $Dir 已存在且非空（不覆寫既有快照）")
    }
    if ($WhatIf) {
        foreach ($d in @($Targets.Dirs)) { Write-Host "[WhatIf] 複製目錄（存在才複製）$($d.Path)" }
        foreach ($f in @($Targets.Files)) { Write-Host "[WhatIf] 複製檔案（存在才複製）$($f.Path)" }
        foreach ($v in @($Targets.RegValues)) { Write-Host "[WhatIf] 讀取登錄值 HKCU\$($v.Key) [$($v.Name)]" }
        foreach ($t in @($Targets.RegTrees)) { Write-Host "[WhatIf] 匯出登錄子樹 HKCU\$($t.Key)" }
        foreach ($w in (Get-WatchTargets $Targets)) { Write-Host "[WhatIf] 記錄僅回報項清單（不複製、不還原）$($w.Path)" }
        if ($Targets.Wallpaper) { Write-Host '[WhatIf] 讀回各螢幕桌布（只讀）' }
        return $null
    }
    # 先確認所有目錄都沒有 reparse point，再建立任何東西
    $hashLimit = [long]$Ctx.HashLimitBytes
    foreach ($d in @($Targets.Dirs)) { $null = Get-DirManifest $d.Path 0 }
    New-Item -ItemType Directory -Path $Dir -Force | Out-Null
    $payload = Join-Path $Dir 'payload'
    New-Item -ItemType Directory -Path (Join-Path $payload 'dirs') -Force | Out-Null
    New-Item -ItemType Directory -Path (Join-Path $payload 'files') -Force | Out-Null

    $dirMeta = @()
    foreach ($d in @($Targets.Dirs)) {
        $exists = Test-Path -LiteralPath $d.Path -PathType Container
        $items = @()
        if ($exists) {
            $copy = Join-Path $payload "dirs\$($d.Id)"
            Copy-DirTree $d.Path $copy
            $items = Get-DirManifest $copy $hashLimit   # 清單取自複製出來的 payload：還原寫回的正是這份內容（M6）
        }
        Write-FcJson (Join-Path $Dir "manifest-$($d.Id).json") @{ Exists = $exists; Items = $items }
        Write-Host ("  目錄 {0}：{1}（{2} 項）" -f $d.Id, $(if ($exists) { '已複製' } else { '不存在（記錄）' }), @($items).Count)
        $dirMeta += @{ Id = $d.Id; Path = $d.Path; Exists = $exists }
    }

    $fileMeta = @()
    foreach ($f in @($Targets.Files)) {
        $exists = Test-Path -LiteralPath $f.Path -PathType Leaf
        $hash = $null
        if ($exists) {
            $copy = Join-Path $payload "files\$($f.Id)"
            Copy-Item -LiteralPath $f.Path -Destination $copy -Force
            $hash = Get-FileSha256 $copy
        }
        Write-Host ("  檔案 {0}：{1}" -f $f.Id, $(if ($exists) { '已複製' } else { '不存在（記錄）' }))
        $fileMeta += @{ Id = $f.Id; Path = $f.Path; Exists = $exists; Sha256 = $hash }
    }

    $watchMeta = @()
    foreach ($w in (Get-WatchTargets $Targets)) {
        $items = Get-WatchManifest $w.Path $w.Include $hashLimit
        $exists = $null -ne $items
        Write-FcJson (Join-Path $Dir "watch-$($w.Id).json") @{ Exists = $exists; Items = $(if ($exists) { $items } else { @() }) }
        Write-Host ("  僅回報 {0}：{1}（不複製、不還原）" -f $w.Id, $(if ($exists) { "$(@($items).Count) 項" } else { '目錄不存在（記錄）' }))
        $watchMeta += @{ Id = $w.Id; Path = $w.Path; Exists = $exists }
    }

    $regValues = @(foreach ($v in @($Targets.RegValues)) {
            $cur = Read-RegValue $v.Key $v.Name
            $entry = @{ Id = $v.Id; Key = $v.Key; Name = $v.Name; Exists = ($null -ne $cur); Kind = $null; Data = $null }
            if ($cur) { $entry.Kind = $cur.Kind; $entry.Data = $cur.Data }
            Write-Host ("  登錄值 {0}：{1}" -f $v.Id, $(if ($cur) { "存在（$($cur.Kind)）" } else { '不存在（記錄）' }))
            $entry
        })
    $regTrees = @(foreach ($t in @($Targets.RegTrees)) {
            $node = Read-RegNode $t.Key
            $entry = @{ Id = $t.Id; Key = $t.Key; Exists = ($null -ne $node); Node = $node; PruneParentKey = $null; ParentExisted = $null }
            if ($t.ContainsKey('PruneParentKey') -and $t.PruneParentKey) {
                $entry.PruneParentKey = $t.PruneParentKey
                $entry.ParentExisted = Test-RegKeyExists $t.PruneParentKey
            }
            Write-Host ("  登錄子樹 {0}：{1}" -f $t.Id, $(if ($node) { '已匯出' } else { '不存在（記錄）' }))
            $entry
        })
    Write-FcJson (Join-Path $Dir 'registry.json') @{ Values = $regValues; Trees = $regTrees }

    $wallOk = $null
    if ($Targets.Wallpaper) {
        $wall = & $Ctx.ReadWallpaper
        Write-FcJson (Join-Path $Dir 'wallpaper.json') $wall
        $wallOk = [bool]$wall.Ok
        if ($wallOk) { Write-Host ("  桌布讀回：{0} 台螢幕" -f @($wall.Monitors).Count) }
        else { Write-Warning "桌布讀回失敗（$($wall.Error)）；快照仍建立，但桌布比對會略過（桌布未驗證）" }
    }

    $metaPath = Join-Path $Dir 'snapshot.json'
    Write-FcJson $metaPath @{
        Version = 1; Complete = $true; Created = (Get-Date).ToString('o'); Machine = $env:COMPUTERNAME; User = $env:USERNAME
        Dirs = $dirMeta; Files = $fileMeta; Watch = $watchMeta; WallpaperOk = $wallOk
    }

    # 自我驗證：剛寫好的快照立刻對現況比對，不是零差異就不算完整快照
    try { $cmp = Compare-FcSnapshot -Dir $Dir -Targets $Targets -Ctx $Ctx }
    catch { Remove-Item -LiteralPath $metaPath -Force; throw }
    if ($cmp.Differences.Count -gt 0) {
        Remove-Item -LiteralPath $metaPath -Force
        Write-Host "快照自我驗證失敗：Save 後立刻比對有 $($cmp.Differences.Count) 項差異，快照已標為不完整（不可用於還原）" -ForegroundColor Red
        foreach ($d in $cmp.Differences) { Write-Host "  - $d" }
    } else {
        Write-Host "快照完成（自我驗證零差異）→ $Dir"
    }
    [pscustomobject]@{ Complete = ($cmp.Differences.Count -eq 0); Differences = $cmp.Differences; ReportOnly = $cmp.ReportOnly; Warnings = $cmp.Warnings }
}

# 讀取並驗證快照：目標集合（Id、路徑、鍵名）必須與目前清單一致；回傳以 Id 索引的登錄資料。
function Read-ValidSnapshot([string]$Dir, $Targets) {
    $metaPath = Join-Path $Dir 'snapshot.json'
    if (-not (Test-Path -LiteralPath $metaPath)) { throw [System.InvalidOperationException]::new("拒絕：$Dir 不是完整快照（缺 snapshot.json）") }
    $meta = Read-FcJson $metaPath
    if ($meta.Version -ne 1 -or -not $meta.Complete) { throw [System.InvalidOperationException]::new('拒絕：快照版本或完整性標記不符') }
    $kinds = @(
        @{ Label = '目錄'; Saved = @($meta.Dirs); Want = @($Targets.Dirs) }
        @{ Label = '檔案'; Saved = @($meta.Files); Want = @($Targets.Files) }
        @{ Label = '僅回報項'; Saved = @($meta.Watch | Where-Object { $_ }); Want = (Get-WatchTargets $Targets) }
    )
    foreach ($k in $kinds) {
        $savedIds = (@($k.Saved | ForEach-Object { $_.Id }) -join ',')
        $wantIds = (@($k.Want | ForEach-Object { $_.Id }) -join ',')
        if ($savedIds -cne $wantIds) { throw [System.InvalidOperationException]::new("拒絕：快照的$($k.Label)目標（$savedIds）與目前清單（$wantIds）不一致") }
        foreach ($w in $k.Want) {
            $m = @($k.Saved | Where-Object { $_.Id -ceq $w.Id })[0]
            if ((Get-NormalPath $m.Path) -ine (Get-NormalPath $w.Path)) { throw [System.InvalidOperationException]::new("拒絕：快照中$($k.Label) $($w.Id) 的路徑 $($m.Path) 與目前 $($w.Path) 不同") }
        }
    }
    $reg = Read-FcJson (Join-Path $Dir 'registry.json')
    $values = @{}
    foreach ($v in @($reg.Values)) { $values[$v.Id] = $v }
    $trees = @{}
    foreach ($t in @($reg.Trees)) { $trees[$t.Id] = $t }
    if ((@($values.Keys | Sort-Object) -join ',') -cne (@($Targets.RegValues | ForEach-Object { $_.Id } | Sort-Object) -join ',') -or
        (@($trees.Keys | Sort-Object) -join ',') -cne (@($Targets.RegTrees | ForEach-Object { $_.Id } | Sort-Object) -join ',')) {
        throw [System.InvalidOperationException]::new('拒絕：快照的登錄目標與目前清單不一致')
    }
    foreach ($v in @($Targets.RegValues)) {
        $s = $values[$v.Id]
        if ($s.Key -cne $v.Key -or $s.Name -cne $v.Name) { throw [System.InvalidOperationException]::new("拒絕：快照的登錄值 $($v.Id) 與目前清單不同") }
    }
    foreach ($t in @($Targets.RegTrees)) {
        if ($trees[$t.Id].Key -cne $t.Key) { throw [System.InvalidOperationException]::new("拒絕：快照的登錄子樹 $($t.Id) 與目前清單不同") }
    }
    @{ Meta = $meta; Values = $values; Trees = $trees }
}

# 還原的第一道關：每個「存在」的項目在快照裡都必須有完整內容，缺一項整個拒絕（刪除之前；I3）。
# 回傳 manifest 字典（以目錄 Id 索引），避免在刪除迴圈中才讀檔。
function Assert-PayloadComplete([string]$Dir, $Snap, $Targets) {
    $problems = [System.Collections.Generic.List[string]]::new()
    $manifests = @{}
    $payload = Join-Path $Dir 'payload'
    foreach ($d in @($Targets.Dirs)) {
        $mp = Join-Path $Dir "manifest-$($d.Id).json"
        if (-not (Test-Path -LiteralPath $mp)) { $problems.Add("缺 manifest-$($d.Id).json"); continue }
        $m = Read-FcJson $mp
        $manifests[$d.Id] = $m
        if (-not $m.Exists) { continue }
        $src = Join-Path $payload "dirs\$($d.Id)"
        if (-not (Test-Path -LiteralPath $src -PathType Container)) { $problems.Add("缺目錄內容 payload\dirs\$($d.Id)"); continue }
        $cur = Get-DirManifest $src 0   # 只比路徑／類型／大小（payload 內容的雜湊由還原後的 Compare 驗）
        $have = @{}
        foreach ($e in @($cur)) { $have[$e.P] = $e }
        $want = @{}
        foreach ($e in @($m.Items)) { $want[$e.P] = $e }
        foreach ($p in $want.Keys) {
            if (-not $have.ContainsKey($p)) { $problems.Add("payload\dirs\$($d.Id) 缺 $p") }
            elseif ($have[$p].T -ne $want[$p].T -or ($want[$p].T -eq 'f' -and $have[$p].L -ne $want[$p].L)) { $problems.Add("payload\dirs\$($d.Id) 的 $p 類型或大小與 manifest 不符") }
        }
        foreach ($p in $have.Keys) { if (-not $want.ContainsKey($p)) { $problems.Add("payload\dirs\$($d.Id) 多出 $p") } }
    }
    foreach ($f in @($Targets.Files)) {
        $m = @($Snap.Meta.Files | Where-Object { $_.Id -ceq $f.Id })[0]
        if (-not $m.Exists) { continue }
        $src = Join-Path $payload "files\$($f.Id)"
        if (-not (Test-Path -LiteralPath $src -PathType Leaf)) { $problems.Add("缺檔案內容 payload\files\$($f.Id)"); continue }
        if ((Get-FileSha256 $src) -cne $m.Sha256) { $problems.Add("payload\files\$($f.Id) 的 SHA256 與記錄不符") }
    }
    foreach ($t in @($Targets.RegTrees)) {
        $s = $Snap.Trees[$t.Id]
        if ($s.Exists -and $null -eq $s.Node) { $problems.Add("登錄子樹 $($t.Id) 標為存在但沒有內容") }
    }
    foreach ($v in @($Targets.RegValues)) {
        $s = $Snap.Values[$v.Id]
        if ($s.Exists -and ($null -eq $s.Kind -or $null -eq $s.Data)) { $problems.Add("登錄值 $($v.Id) 標為存在但缺型別或資料") }
    }
    if ($problems.Count -gt 0) {
        $shown = ($problems | Select-Object -First 8) -join '；'
        throw [System.InvalidOperationException]::new("拒絕還原：快照內容不完整（$($problems.Count) 項：$shown）。未動任何東西。")
    }
    $manifests
}

# ── Compare ──────────────────────────────────────────────────────────────────────────────────
# 回傳 { Differences（會被還原的範圍）; ReportOnly（僅回報項，不還原）; Warnings; WallpaperVerified }
function Compare-FcSnapshot {
    param([Parameter(Mandatory)][string]$Dir, [Parameter(Mandatory)]$Targets, [Parameter(Mandatory)]$Ctx)
    Assert-SnapshotDirSafe $Dir $Targets
    $snap = Read-ValidSnapshot $Dir $Targets
    $diffs = [System.Collections.Generic.List[string]]::new()
    $reportOnly = [System.Collections.Generic.List[string]]::new()
    $warns = [System.Collections.Generic.List[string]]::new()
    $hashLimit = [long]$Ctx.HashLimitBytes

    foreach ($d in @($Targets.Dirs)) {
        $saved = Read-FcJson (Join-Path $Dir "manifest-$($d.Id).json")
        Compare-ManifestInto $saved (Get-DirManifest $d.Path $hashLimit) "目錄 $($d.Id)" $diffs
    }

    foreach ($f in @($Targets.Files)) {
        $m = @($snap.Meta.Files | Where-Object { $_.Id -ceq $f.Id })[0]
        $exists = Test-Path -LiteralPath $f.Path -PathType Leaf
        if (-not $m.Exists -and -not $exists) { continue }
        if (-not $m.Exists) { $diffs.Add("檔案 $($f.Id)：快照時不存在、現在存在（$($f.Path)）"); continue }
        if (-not $exists) { $diffs.Add("檔案 $($f.Id)：快照時存在、現在不存在（$($f.Path)）"); continue }
        if ((Get-FileSha256 $f.Path) -cne $m.Sha256) { $diffs.Add("檔案 $($f.Id)：內容不同（SHA256）") }
    }

    foreach ($tv in @($Targets.RegValues)) {
        $v = $snap.Values[$tv.Id]
        $cur = Read-RegValue $v.Key $v.Name
        $label = "登錄值 HKCU\$($v.Key) [$($v.Name)]"
        if (-not $v.Exists -and -not $cur) { continue }
        if (-not $v.Exists) { $diffs.Add("${label}：快照時不存在、現在存在（$($cur.Kind)：$(Format-RegSig $cur.Data)）"); continue }
        if (-not $cur) { $diffs.Add("${label}：快照時存在、現在不存在"); continue }
        # 區分大小寫：Binary 以 base64 存，大小寫不同＝位元組不同（I1）
        if ($cur.Kind -cne $v.Kind -or $cur.Data -cne $v.Data) {
            $diffs.Add("${label}：不同（快照 $($v.Kind)：$(Format-RegSig $v.Data) → 現在 $($cur.Kind)：$(Format-RegSig $cur.Data)）")
        }
    }
    foreach ($tt in @($Targets.RegTrees)) {
        $t = $snap.Trees[$tt.Id]
        $curNode = Read-RegNode $t.Key
        $label = "登錄子樹 HKCU\$($t.Key)"
        if (-not $t.Exists -and $null -eq $curNode) { continue }
        if (-not $t.Exists) { $diffs.Add("${label}：快照時不存在、現在存在"); continue }
        if ($null -eq $curNode) { $diffs.Add("${label}：快照時存在、現在不存在"); continue }
        $a = @{}; ConvertTo-RegFlat $t.Node '' $a
        $b = @{}; ConvertTo-RegFlat $curNode '' $b
        foreach ($k in ($a.Keys | Sort-Object)) {
            if (-not $b.ContainsKey($k)) { $diffs.Add("${label}：缺少 $k"); continue }
            if ($a[$k] -cne $b[$k]) { $diffs.Add("${label}：$k 不同（快照 $(Format-RegSig $a[$k]) → 現在 $(Format-RegSig $b[$k])）") }
        }
        foreach ($k in ($b.Keys | Sort-Object)) {
            if (-not $a.ContainsKey($k)) { $diffs.Add("${label}：多出 $k") }
        }
    }

    foreach ($w in (Get-WatchTargets $Targets)) {
        $saved = Read-FcJson (Join-Path $Dir "watch-$($w.Id).json")
        Compare-ManifestInto $saved (Get-WatchManifest $w.Path $w.Include $hashLimit) "僅回報 $($w.Id)" $reportOnly
    }

    $wallVerified = $false
    if ($Targets.Wallpaper) {
        $savedWall = Read-FcJson (Join-Path $Dir 'wallpaper.json')
        $curWall = & $Ctx.ReadWallpaper
        if (-not $savedWall.Ok) { $warns.Add("桌布未驗證：快照時讀回失敗（$($savedWall.Error)）") }
        elseif (-not $curWall.Ok) { $warns.Add("桌布未驗證：現在讀回失敗（$($curWall.Error)）") }
        else {
            $wallVerified = $true
            if ($savedWall.Global -ine $curWall.Global) { $diffs.Add("桌布：全域路徑不同（'$($savedWall.Global)' → '$($curWall.Global)'）") }
            if ($savedWall.Position -ne $curWall.Position) { $diffs.Add("桌布：位置（GetPosition）不同（$($savedWall.Position) → $($curWall.Position)）") }
            if ([int64]$savedWall.Color -ne [int64]$curWall.Color) { $diffs.Add("桌布：背景色不同（$($savedWall.Color) → $($curWall.Color)）") }
            if ($savedWall.Status -ne $curWall.Status) { $diffs.Add("桌布：GetStatus 不同（$($savedWall.Status) → $($curWall.Status)）") }
            $sm = @{}; foreach ($m in @($savedWall.Monitors)) { $sm[$m.Id] = $m }
            $cm = @{}; foreach ($m in @($curWall.Monitors)) { $cm[$m.Id] = $m }
            foreach ($id in ($sm.Keys | Sort-Object)) {
                if (-not $cm.ContainsKey($id)) { $diffs.Add("桌布：螢幕 $id 快照時在線、現在不在線"); continue }
                if ($sm[$id].Path -ine $cm[$id].Path) { $diffs.Add("桌布：螢幕 $id 的桌布不同（'$($sm[$id].Path)' → '$($cm[$id].Path)'）") }
            }
            foreach ($id in ($cm.Keys | Sort-Object)) {
                if (-not $sm.ContainsKey($id)) { $diffs.Add("桌布：螢幕 $id 快照時不在線、現在在線") }
            }
        }
    }

    [pscustomobject]@{ Differences = @($diffs); ReportOnly = @($reportOnly); Warnings = @($warns); WallpaperVerified = $wallVerified }
}

# ── Restore ──────────────────────────────────────────────────────────────────────────────────
function Restore-FcSnapshot {
    param([Parameter(Mandatory)][string]$Dir, [Parameter(Mandatory)]$Targets, [Parameter(Mandatory)]$Ctx, [switch]$WhatIf)
    Assert-SnapshotDirSafe $Dir $Targets
    $snap = Read-ValidSnapshot $Dir $Targets
    # 任何刪除動作之前：快照內容完整性、刪除目標防線、行程檢查——有一項不合格就整個不動
    $manifests = Assert-PayloadComplete $Dir $snap $Targets
    foreach ($d in @($Targets.Dirs)) { Assert-SafeDeleteDir $d.Path }
    foreach ($t in @($Targets.RegTrees)) {
        Assert-SafeRegKey $t.Key
        # 父鍵只會以「空鍵才刪」的 DeleteSubKey 處理（不遞迴），深度下限放寬到 2（Software\<製造商>）
        if ($t.ContainsKey('PruneParentKey') -and $t.PruneParentKey) { Assert-SafeRegKey $t.PruneParentKey 2 }
    }
    if (-not $WhatIf) { Assert-HostNotRunning $Ctx '還原' }
    else {
        $running = @(& $Ctx.IsHostRunning)
        if ($running.Count -gt 0) { Write-Warning "[WhatIf] 目前有宿主或相關行程在跑（$($running -join ', ')）；實際還原會被拒絕" }
    }
    $actions = [System.Collections.Generic.List[string]]::new()
    $errors = [System.Collections.Generic.List[string]]::new()
    function Step([string]$Desc, [scriptblock]$Act) {
        $actions.Add($Desc)
        if ($WhatIf) { Write-Host "[WhatIf] $Desc"; return }
        Write-Host "  $Desc"
        try { & $Act } catch { $errors.Add("$Desc 失敗：$($_.Exception.Message)"); Write-Host "    失敗：$($_.Exception.Message)" -ForegroundColor Red }
    }

    $payload = Join-Path $Dir 'payload'
    foreach ($d in @($Targets.Dirs)) {
        $path = $d.Path
        Step "刪除目錄 $path" { Remove-DirWithRetry $path }
        if ($manifests[$d.Id].Exists) {
            $src = Join-Path $payload "dirs\$($d.Id)"
            Step "複製回目錄 $path" { Copy-DirTree $src $path }
        }
    }
    foreach ($f in @($Targets.Files)) {
        $m = @($snap.Meta.Files | Where-Object { $_.Id -ceq $f.Id })[0]
        $path = $f.Path
        if ($m.Exists) {
            $src = Join-Path $payload "files\$($f.Id)"
            Step "還原檔案 $path" {
                New-Item -ItemType Directory -Path (Split-Path $path -Parent) -Force | Out-Null
                Copy-Item -LiteralPath $src -Destination $path -Force
            }
        } else {
            Step "確保檔案不存在 $path" { if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path -Force } }
        }
    }
    # 迭代 $Targets（鍵名與父鍵取自目前清單），只從快照取資料（M1）
    foreach ($tt in @($Targets.RegTrees)) {
        $t = $snap.Trees[$tt.Id]
        $key = $tt.Key
        Step "刪除登錄子樹 HKCU\$key" { [Microsoft.Win32.Registry]::CurrentUser.DeleteSubKeyTree($key, $false) }
        if ($t.Exists) {
            $node = $t.Node
            Step "寫回登錄子樹 HKCU\$key" { Write-RegNode $key $node }
        }
        if ($tt.ContainsKey('PruneParentKey') -and $tt.PruneParentKey -and -not $t.ParentExisted) {
            $parent = $tt.PruneParentKey
            Step "父鍵 HKCU\$parent 快照時不存在：若為空鍵則刪除" {
                $pk = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($parent, $false)
                if ($pk) {
                    $empty = ($pk.SubKeyCount -eq 0 -and $pk.ValueCount -eq 0)
                    $pk.Close()
                    if ($empty) { [Microsoft.Win32.Registry]::CurrentUser.DeleteSubKey($parent, $false) }
                }
            }
        }
    }
    foreach ($tv in @($Targets.RegValues)) {
        $v = $snap.Values[$tv.Id]
        $key = $tv.Key; $name = $tv.Name; $kind = $v.Kind; $data = $v.Data
        if ($v.Exists) {
            Step "寫回登錄值 HKCU\$key [$name]（$kind）" { Set-RegValueRaw $key $name $kind $data }
        } else {
            Step "確保登錄值不存在 HKCU\$key [$name]" { Remove-RegValueIfPresent $key $name }
        }
    }

    if ($WhatIf) {
        foreach ($w in (Get-WatchTargets $Targets)) { Write-Host "[WhatIf] 僅回報項 $($w.Path) 不還原（explorer 的工作檔）" }
        Write-Host '[WhatIf] 以上動作皆未執行；桌布僅比對、不設定'
        return [pscustomobject]@{ Actions = @($actions); Errors = @(); Differences = @(); ReportOnly = @(); Warnings = @(); WallpaperVerified = $false }
    }
    Write-Host '還原動作完成，開始比對…'
    try {
        $cmp = Compare-FcSnapshot -Dir $Dir -Targets $Targets -Ctx $Ctx
    } catch {
        # 比對本身失敗：已累積的動作錯誤仍要出現在摘要裡（M4）
        $errors.Add("還原後比對失敗：$($_.Exception.Message)")
        return [pscustomobject]@{ Actions = @($actions); Errors = @($errors); Differences = @(); ReportOnly = @(); Warnings = @(); WallpaperVerified = $false }
    }
    [pscustomobject]@{ Actions = @($actions); Errors = @($errors); Differences = $cmp.Differences; ReportOnly = $cmp.ReportOnly; Warnings = $cmp.Warnings; WallpaperVerified = $cmp.WallpaperVerified }
}

Export-ModuleMember -Function Save-FcSnapshot, Restore-FcSnapshot, Compare-FcSnapshot, New-FcSnapshotContext, Get-FcWallpaperReadback, Assert-SafeDeleteDir, Resolve-FcProfilePaths, New-FcRealTargets, Select-FcBlockingProcesses
