<#
.SYNOPSIS
    dynamic-wallpaper task 1.5「螢幕識別探針」取樣腳本（唯讀）：每執行一次，在 jsonl 尾端附加一筆螢幕識別快照。

.DESCRIPTION
    使用者插拔外接螢幕、切換縮放或解析度的**每個動作前後各取一次樣**（-Label 區分，例如
    before-unplug-4k／after-unplug-4k／before-scale-150／after-scale-150）。每筆紀錄含：
    1. IDesktopWallpaper：GetMonitorDevicePathCount，以及每個索引 i 的 GetMonitorDevicePathAt(i) 與
       GetMonitorRECT(path)，**每個呼叫都記 HRESULT**（離線螢幕會回錯或 S_FALSE，照實記錄）。
       GetMonitorRECT 在兩種執行緒 DPI 感知下各讀一次（行程預設、per-monitor v2），回答 D4 的
       「RECT 是否為實體像素、是否受行程 DPI 感知影響」。
    2. probe-monitor-ids.ps1 那套資料（共用 lib/MonitorIdsType.ps1）：QueryDisplayConfig（QDC_ONLY_ACTIVE_PATHS，
       同宿主 source_name_to_device_path）的 GDI 名稱→monitorDevicePath、EnumDisplayMonitors 的 szDevice、
       邊界、工作區、主螢幕、有效 DPI，以及模擬宿主 monitors_from_tauri（szDevice 精確比對、區分大小寫）所得
       的穩定識別 id。
    3. correlation：每台顯示器的穩定 id 對應到 IDesktopWallpaper 的哪個索引（依裝置路徑字串、依 RECT 各算一次）。

    **只呼叫查詢型 API**：不設桌布、不改顯示設定、不啟動也不碰 fc-host、不注入輸入。IDesktopWallpaper 的
    SetWallpaper 等方法只為保住 vtable 順序而宣告，絕不呼叫。

    **去識別**：整筆 JSON 文字寫檔前一律經 lib/EvidenceLog.psm1 的 ConvertTo-EvidenceText。

    彙整用 host/tools/probe-dw-1.5-summarize.ps1（相鄰兩筆做差異比對、輸出 markdown 表格）。

    IDesktopWallpaper 的 C# helper 介面宣告（GUID、vtable 順序）出自 lib/InstallerSnapshot.psm1 的
    FcSnapshot.WallpaperReader；差別是這裡用 PreserveSig 取得 HRESULT（含 S_FALSE），那邊吞掉回傳碼。

.PARAMETER Label
    這次取樣的標籤。

.PARAMETER Out
    jsonl 路徑；每次附加一行，父目錄不存在會建立。

.EXAMPLE
    pwsh -NoProfile -File host/tools/probe-dw-1.5.ps1 -Label before-unplug-4k -Out $env:TEMP\fc-dw15.jsonl
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Label,
    [Parameter(Mandatory)][string]$Out
)

$ErrorActionPreference = 'Stop'

Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force
. (Join-Path $PSScriptRoot 'lib\MonitorIdsType.ps1')

if (-not ('FcProbe15.DwProbe' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Threading;

namespace FcProbe15 {
    [StructLayout(LayoutKind.Sequential)]
    public struct NativeRect { public int Left, Top, Right, Bottom; }

    // 只呼叫 GetMonitorDevicePathCount／GetMonitorDevicePathAt／GetMonitorRECT；其餘方法只為保住 vtable 順序而
    // 宣告，絕不呼叫。GUID 與順序出自 lib/InstallerSnapshot.psm1（對照 windows crate 的 IDesktopWallpaper_Vtbl）。
    [ComImport, Guid("B92B56A9-8B55-4E14-9A89-0199BBB6F93B"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    internal interface IDesktopWallpaper {
        void SetWallpaper([MarshalAs(UnmanagedType.LPWStr)] string monitorID, [MarshalAs(UnmanagedType.LPWStr)] string wallpaper);
        void GetWallpaper([MarshalAs(UnmanagedType.LPWStr)] string monitorID, [MarshalAs(UnmanagedType.LPWStr)] out string wallpaper);
        [PreserveSig] int GetMonitorDevicePathAt(uint monitorIndex, [MarshalAs(UnmanagedType.LPWStr)] out string monitorID);
        [PreserveSig] int GetMonitorDevicePathCount(out uint count);
        [PreserveSig] int GetMonitorRECT([MarshalAs(UnmanagedType.LPWStr)] string monitorID, out NativeRect displayRect);
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

    public class DwRect { public int Hr; public int Left, Top, Right, Bottom; }

    public class DwEntry {
        public int Index; public string Path; public int PathHr;
        public DwRect RectDefault; public DwRect RectPmv2;
    }

    public class DwSnapshot {
        public bool Ok; public string Error;
        public int CountHr; public uint Count;
        public int DefaultAwareness; public bool DefaultIsPmv2;
        public List<DwEntry> Entries = new List<DwEntry>();
    }

    public static class DwProbe {
        [DllImport("user32.dll")] static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
        [DllImport("user32.dll")] static extern IntPtr GetThreadDpiAwarenessContext();
        [DllImport("user32.dll")] static extern int GetAwarenessFromDpiAwarenessContext(IntPtr context);
        [DllImport("user32.dll")] static extern bool AreDpiAwarenessContextsEqual(IntPtr a, IntPtr b);

        // 在獨立 STA 執行緒讀取（只讀）；COM 失敗回報在 Error、不丟例外。
        public static DwSnapshot Read() {
            var snap = new DwSnapshot();
            var t = new Thread(delegate () { Core(snap); });
            t.SetApartmentState(ApartmentState.STA);
            t.Start();
            t.Join();
            return snap;
        }

        static DwRect Rect(IDesktopWallpaper dw, string path) {
            var r = new DwRect();
            if (path == null) { r.Hr = unchecked((int)0x80004003); return r; } // 沒有路徑可查：記 E_POINTER 樣式的占位
            NativeRect nr;
            r.Hr = dw.GetMonitorRECT(path, out nr);
            r.Left = nr.Left; r.Top = nr.Top; r.Right = nr.Right; r.Bottom = nr.Bottom;
            return r;
        }

        static void Core(DwSnapshot snap) {
            object com = null;
            try {
                IntPtr ctx = GetThreadDpiAwarenessContext();
                snap.DefaultAwareness = GetAwarenessFromDpiAwarenessContext(ctx);
                snap.DefaultIsPmv2 = AreDpiAwarenessContextsEqual(ctx, new IntPtr(-4));

                Type type = Type.GetTypeFromCLSID(new Guid("C2CF3110-460E-4FC1-B9D0-8A1C0C9CC4BD"), true);
                com = Activator.CreateInstance(type);
                var dw = (IDesktopWallpaper)com;
                uint count;
                snap.CountHr = dw.GetMonitorDevicePathCount(out count);
                snap.Count = count;
                for (uint i = 0; i < count; i++) {
                    var e = new DwEntry { Index = (int)i };
                    string id;
                    e.PathHr = dw.GetMonitorDevicePathAt(i, out id);
                    e.Path = id;
                    snap.Entries.Add(e);
                }
                foreach (var e in snap.Entries) e.RectDefault = Rect(dw, e.Path);
                IntPtr old = SetThreadDpiAwarenessContext(new IntPtr(-4)); // per-monitor v2；只影響本執行緒的座標換算
                foreach (var e in snap.Entries) e.RectPmv2 = Rect(dw, e.Path);
                SetThreadDpiAwarenessContext(old);
                snap.Ok = true;
            } catch (Exception ex) {
                snap.Ok = false;
                snap.Error = ex.GetType().Name + ": " + ex.Message;
            } finally {
                if (com != null) { Marshal.ReleaseComObject(com); }
            }
        }
    }
}
'@
}

function ConvertTo-RectObject($r) {
    [ordered]@{
        hr = '0x{0:X8}' -f [int]$r.Hr
        left = $r.Left; top = $r.Top; right = $r.Right; bottom = $r.Bottom
    }
}

function Format-RectString($r) { '({0},{1})-({2},{3})' -f $r.Left, $r.Top, $r.Right, $r.Bottom }

# ── 取樣（全部只讀）──────────────────────────────────────────────────────────────
$dw = [FcProbe15.DwProbe]::Read()
$qdcPaths = [FcProbe.MonitorIds]::Paths($false)
$qdcStatus = [FcProbe.MonitorIds]::LastQueryStatus
$monitors = [FcProbe.MonitorIds]::Monitors()

# 宿主對照表（desktop.rs source_name_to_device_path）：GDI 名稱 → monitorDevicePath，後者覆蓋前者。
$gdiToPath = @{}
$dupes = @()
foreach ($p in $qdcPaths) {
    if ($null -ne $p.SourceGdiName -and $null -ne $p.MonitorDevicePath) {
        if ($gdiToPath.ContainsKey($p.SourceGdiName)) { $dupes += $p.SourceGdiName }
        $gdiToPath[$p.SourceGdiName] = $p.MonitorDevicePath
    }
}

$dwEntries = @(foreach ($e in $dw.Entries) {
        [ordered]@{
            index = $e.Index
            path = $e.Path
            pathHr = '0x{0:X8}' -f [int]$e.PathHr
            rectDefaultDpi = ConvertTo-RectObject $e.RectDefault
            rectPmv2 = ConvertTo-RectObject $e.RectPmv2
        }
    })

$monitorRows = @(foreach ($m in $monitors) {
        # monitors_from_tauri 以 szDevice 精確（區分大小寫）比對；hashtable 鍵不分大小寫，故用 -ceq 逐一比。
        $hit = $gdiToPath.GetEnumerator() | Where-Object { $_.Key -ceq $m.SzDevice } | Select-Object -First 1
        $scale = if ($m.DpiX -gt 0) { [math]::Round($m.DpiX / 96.0, 4) } else { $null }
        [ordered]@{
            szDevice = $m.SzDevice
            primary = [bool]$m.Primary
            bounds = $m.Bounds
            work = $m.Work
            dpiX = [int]$m.DpiX
            dpiY = [int]$m.DpiY
            scale = $scale
            stableId = if ($hit) { [string]$hit.Value } else { $null }
        }
    })

$correlation = @(foreach ($m in $monitorRows) {
        $byPath = $null
        $byRect = $null
        foreach ($e in $dw.Entries) {
            if ($m.stableId -and $e.Path -and ($e.Path -ieq $m.stableId)) { $byPath = $e.Index }
            if ($e.RectPmv2.Hr -eq 0 -and (Format-RectString $e.RectPmv2) -ceq $m.bounds) { $byRect = $e.Index }
        }
        [ordered]@{
            szDevice = $m.szDevice
            stableId = $m.stableId
            dwIndexByPath = $byPath
            dwIndexByRect = $byRect
            agree = ($byPath -eq $byRect)
        }
    })

$qdcRows = @(foreach ($p in $qdcPaths) {
        [ordered]@{
            index = $p.Index
            sourceAdapter = $p.SourceAdapter; sourceId = [int64]$p.SourceId
            sourceStatus = $p.SourceStatus; sourceGdiName = $p.SourceGdiName
            targetAdapter = $p.TargetAdapter; targetId = [int64]$p.TargetId
            targetAvailable = $p.TargetAvailable; pathFlags = '0x{0:X}' -f $p.PathFlags
            targetStatus = $p.TargetStatus
            monitorDevicePath = $p.MonitorDevicePath; friendlyName = $p.FriendlyName
        }
    })

$record = [ordered]@{
    schema = 1
    timestamp = (Get-Date).ToString('o')
    label = $Label
    desktopWallpaper = [ordered]@{
        ok = $dw.Ok
        error = $dw.Error
        countHr = '0x{0:X8}' -f [int]$dw.CountHr
        count = [int]$dw.Count
        threadDefaultDpiAwareness = $dw.DefaultAwareness   # 0 unaware／1 system／2 per-monitor
        threadDefaultIsPerMonitorV2 = $dw.DefaultIsPmv2
        entries = $dwEntries
    }
    queryDisplayConfig = [ordered]@{
        flag = 'QDC_ONLY_ACTIVE_PATHS'
        status = $qdcStatus
        duplicateGdiNames = $dupes
        paths = $qdcRows
    }
    monitors = $monitorRows
    correlation = $correlation
}

$json = $record | ConvertTo-Json -Depth 10 -Compress
$line = ConvertTo-EvidenceText $json

$dir = Split-Path -Parent ([IO.Path]::GetFullPath($Out))
if ($dir -and -not (Test-Path -LiteralPath $dir)) { [void](New-Item -ItemType Directory -Path $dir) }
[IO.File]::AppendAllText([IO.Path]::GetFullPath($Out), $line + "`n", (New-Object System.Text.UTF8Encoding($false)))

Write-Host ("已附加 label='{0}'：IDesktopWallpaper {1} 台（count hr={2}）、QueryDisplayConfig {3} 條路徑、EnumDisplayMonitors {4} 台 → {5}" -f `
        $Label, $dwEntries.Count, $record.desktopWallpaper.countHr, $qdcRows.Count, $monitorRows.Count, (ConvertTo-EvidenceText $Out))
