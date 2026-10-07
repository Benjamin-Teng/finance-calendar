<#
.SYNOPSIS
    共用：probe-monitor-ids.ps1 與 probe-dw-1.5.ps1 的 Win32 顯示器查詢型別（FcProbe.MonitorIds）。

.DESCRIPTION
    原本寫在 probe-monitor-ids.ps1 內，dynamic-wallpaper task 1.5 抽出共用（dot-source 載入；
    型別已載入就略過）。只含查詢型 API：QueryDisplayConfig／DisplayConfigGetDeviceInfo、
    EnumDisplayMonitors／GetMonitorInfoW／GetDpiForMonitor；不改任何顯示設定。
    task 1.5 相對原版的唯一差異：MonitorRow 多了 DpiX／DpiY（GetDpiForMonitor 的有效 DPI，
    縮放比例＝DpiX／96）；既有欄位與行為不變。
#>
if (-not ('FcProbe.MonitorIds' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;

namespace FcProbe {
    [StructLayout(LayoutKind.Sequential)]
    public struct LUID { public uint LowPart; public int HighPart; }

    [StructLayout(LayoutKind.Sequential)]
    public struct PATH_SOURCE_INFO {
        public LUID adapterId; public uint id; public uint modeInfoIdx; public uint statusFlags;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct PATH_TARGET_INFO {
        public LUID adapterId; public uint id; public uint modeInfoIdx; public uint outputTechnology;
        public uint rotation; public uint scaling; public uint refreshNum; public uint refreshDen;
        public uint scanLineOrdering; public int targetAvailable; public uint statusFlags;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct PATH_INFO {
        public PATH_SOURCE_INFO sourceInfo; public PATH_TARGET_INFO targetInfo; public uint flags;
    }

    [StructLayout(LayoutKind.Sequential, Size = 64)]
    public struct MODE_INFO { public uint infoType; public uint id; public LUID adapterId; }

    [StructLayout(LayoutKind.Sequential)]
    public struct DEVICE_INFO_HEADER { public uint type; public uint size; public LUID adapterId; public uint id; }

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    public struct SOURCE_DEVICE_NAME {
        public DEVICE_INFO_HEADER header;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string viewGdiDeviceName;
    }

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    public struct TARGET_DEVICE_NAME {
        public DEVICE_INFO_HEADER header;
        public uint flags; public uint outputTechnology;
        public ushort edidManufactureId; public ushort edidProductCodeId; public uint connectorInstance;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 64)] public string monitorFriendlyDeviceName;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 128)] public string monitorDevicePath;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left, Top, Right, Bottom; }

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    public struct MONITORINFOEX {
        public int cbSize; public RECT rcMonitor; public RECT rcWork; public uint dwFlags;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string szDevice;
    }

    public class PathRow {
        public int Index; public string SourceAdapter; public uint SourceId; public string TargetAdapter;
        public uint TargetId; public int TargetAvailable; public uint PathFlags;
        public int SourceStatus; public string SourceGdiName;
        public int TargetStatus; public string MonitorDevicePath; public string FriendlyName;
    }

    public class MonitorRow {
        public string SzDevice; public bool Primary; public string Work; public string Bounds;
        public uint DpiX; public uint DpiY; // GetDpiForMonitor(MDT_EFFECTIVE_DPI)；失敗為 0
    }

    public static class MonitorIds {
        [DllImport("user32.dll")] static extern int GetDisplayConfigBufferSizes(uint flags, out uint numPaths, out uint numModes);
        [DllImport("user32.dll")] static extern int QueryDisplayConfig(uint flags, ref uint numPaths, [Out] PATH_INFO[] paths, ref uint numModes, [Out] MODE_INFO[] modes, IntPtr topologyId);
        [DllImport("user32.dll")] static extern int DisplayConfigGetDeviceInfo(ref SOURCE_DEVICE_NAME request);
        [DllImport("user32.dll")] static extern int DisplayConfigGetDeviceInfo(ref TARGET_DEVICE_NAME request);

        delegate bool MonitorEnumProc(IntPtr hMonitor, IntPtr hdc, IntPtr rect, IntPtr data);
        [DllImport("user32.dll")] static extern bool EnumDisplayMonitors(IntPtr hdc, IntPtr clip, MonitorEnumProc proc, IntPtr data);
        [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern bool GetMonitorInfoW(IntPtr hMonitor, ref MONITORINFOEX info);

        static string Luid(LUID l) { return string.Format("{0:X8}:{1:X8}", l.HighPart, l.LowPart); }

        public static int LastQueryStatus;

        public static List<PathRow> Paths(bool allPaths) {
            uint flags = allPaths ? 1u : 2u; // QDC_ALL_PATHS=1, QDC_ONLY_ACTIVE_PATHS=2
            var rows = new List<PathRow>();
            for (int attempt = 0; attempt < 8; attempt++) {
                uint np, nm;
                int s = GetDisplayConfigBufferSizes(flags, out np, out nm);
                if (s != 0) { LastQueryStatus = s; return rows; }
                var paths = new PATH_INFO[np];
                var modes = new MODE_INFO[nm];
                s = QueryDisplayConfig(flags, ref np, paths, ref nm, modes, IntPtr.Zero);
                LastQueryStatus = s;
                if (s == 122) continue; // ERROR_INSUFFICIENT_BUFFER
                if (s != 0) return rows;
                for (int i = 0; i < np; i++) {
                    var p = paths[i];
                    var src = new SOURCE_DEVICE_NAME();
                    src.header.type = 1; // GET_SOURCE_NAME
                    src.header.size = (uint)Marshal.SizeOf(typeof(SOURCE_DEVICE_NAME));
                    src.header.adapterId = p.sourceInfo.adapterId;
                    src.header.id = p.sourceInfo.id;
                    int ss = DisplayConfigGetDeviceInfo(ref src);
                    var tgt = new TARGET_DEVICE_NAME();
                    tgt.header.type = 2; // GET_TARGET_NAME
                    tgt.header.size = (uint)Marshal.SizeOf(typeof(TARGET_DEVICE_NAME));
                    tgt.header.adapterId = p.targetInfo.adapterId;
                    tgt.header.id = p.targetInfo.id;
                    int ts = DisplayConfigGetDeviceInfo(ref tgt);
                    rows.Add(new PathRow {
                        Index = i,
                        SourceAdapter = Luid(p.sourceInfo.adapterId), SourceId = p.sourceInfo.id,
                        TargetAdapter = Luid(p.targetInfo.adapterId), TargetId = p.targetInfo.id,
                        TargetAvailable = p.targetInfo.targetAvailable, PathFlags = p.flags,
                        SourceStatus = ss, SourceGdiName = ss == 0 ? src.viewGdiDeviceName : null,
                        TargetStatus = ts, MonitorDevicePath = ts == 0 ? tgt.monitorDevicePath : null,
                        FriendlyName = ts == 0 ? tgt.monitorFriendlyDeviceName : null,
                    });
                }
                return rows;
            }
            return rows;
        }

        [DllImport("user32.dll")] static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
        [DllImport("shcore.dll")] static extern int GetDpiForMonitor(IntPtr hMonitor, int dpiType, out uint dpiX, out uint dpiY);

        public static List<MonitorRow> Monitors() {
            // 宿主是 per-monitor v2 DPI aware；本執行緒切成同一模式，工作區才是宿主看到的實體像素。
            // 只影響本探針執行緒的座標換算，不改任何系統設定。
            SetThreadDpiAwarenessContext(new IntPtr(-4)); // DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2
            var rows = new List<MonitorRow>();
            EnumDisplayMonitors(IntPtr.Zero, IntPtr.Zero, (h, dc, r, d) => {
                var info = new MONITORINFOEX();
                info.cbSize = Marshal.SizeOf(typeof(MONITORINFOEX));
                if (GetMonitorInfoW(h, ref info)) {
                    uint dx = 0, dy = 0;
                    if (GetDpiForMonitor(h, 0, out dx, out dy) != 0) { dx = 0; dy = 0; } // MDT_EFFECTIVE_DPI=0
                    rows.Add(new MonitorRow {
                        DpiX = dx, DpiY = dy,
                        SzDevice = info.szDevice, Primary = (info.dwFlags & 1) != 0,
                        Work = string.Format("({0},{1})-({2},{3})", info.rcWork.Left, info.rcWork.Top, info.rcWork.Right, info.rcWork.Bottom),
                        Bounds = string.Format("({0},{1})-({2},{3})", info.rcMonitor.Left, info.rcMonitor.Top, info.rcMonitor.Right, info.rcMonitor.Bottom),
                    });
                }
                return true;
            }, IntPtr.Zero);
            return rows;
        }
    }
}
'@
}
