<#
.SYNOPSIS
    dynamic-wallpaper task 5.2：從 fc-host.exe 取出圖示資源，逐尺寸比對 host/icons/icon.ico。

.DESCRIPTION
    唯讀：以 LoadLibraryEx(LOAD_LIBRARY_AS_DATAFILE | LOAD_LIBRARY_AS_IMAGE_RESOURCE) 載入 exe（不執行），
    列舉 RT_GROUP_ICON，解析每個群組的 GRPICONDIRENTRY 與對應 RT_ICON 的原始位元組，與 icon.ico 同尺寸
    entry 的 PNG 位元組逐位元組比對（位元組相同＝像素相同）。另以 LookupIconIdFromDirectoryEx 問系統
    「要畫 N px 時會挑哪一張」，確認 16／20／24／32／40／48／64／256 都挑到同尺寸那張，而不是由別的尺寸縮放。

    用法：pwsh -NoProfile -File host/tools/verify-dw-5.2-exe-icon.ps1 [-Exe <路徑>] [-Ico <路徑>] [-Log <路徑>]
    結束碼：0＝PASS、1＝FAIL。記錄檔經 EvidenceLog 去識別。
#>
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\fc-host.exe'),
    [string]$Ico = (Join-Path $PSScriptRoot '..\icons\icon.ico'),
    [string]$Log = (Join-Path $PSScriptRoot 'evidence\dw-5.2-exe-icon.log')
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force

if (-not ('DwIcon52.ResReader' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;

namespace DwIcon52 {
    public static class ResReader {
        const uint LOAD_LIBRARY_AS_DATAFILE = 0x2;
        const uint LOAD_LIBRARY_AS_IMAGE_RESOURCE = 0x20;
        static readonly IntPtr RT_ICON = (IntPtr)3;
        static readonly IntPtr RT_GROUP_ICON = (IntPtr)14;

        [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
        static extern IntPtr LoadLibraryExW(string path, IntPtr file, uint flags);
        [DllImport("kernel32.dll", SetLastError = true)]
        static extern bool FreeLibrary(IntPtr module);
        delegate bool EnumResNameProc(IntPtr module, IntPtr type, IntPtr name, IntPtr param);
        [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
        static extern bool EnumResourceNamesW(IntPtr module, IntPtr type, EnumResNameProc proc, IntPtr param);
        [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
        static extern IntPtr FindResourceW(IntPtr module, IntPtr name, IntPtr type);
        [DllImport("kernel32.dll", SetLastError = true)]
        static extern IntPtr LoadResource(IntPtr module, IntPtr res);
        [DllImport("kernel32.dll", SetLastError = true)]
        static extern IntPtr LockResource(IntPtr data);
        [DllImport("kernel32.dll", SetLastError = true)]
        static extern uint SizeofResource(IntPtr module, IntPtr res);
        [DllImport("user32.dll", SetLastError = true)]
        static extern int LookupIconIdFromDirectoryEx(byte[] dir, bool icon, int cx, int cy, uint flags);

        public class Group {
            public string Name;
            public byte[] Dir;
            public List<Entry> Entries = new List<Entry>();
        }
        public class Entry {
            public int Width; public int Height; public int BitCount; public int Id; public byte[] Data;
        }

        static byte[] Read(IntPtr module, IntPtr name, IntPtr type) {
            IntPtr res = FindResourceW(module, name, type);
            if (res == IntPtr.Zero) throw new Exception("FindResource 失敗：" + Marshal.GetLastWin32Error());
            uint size = SizeofResource(module, res);
            IntPtr ptr = LockResource(LoadResource(module, res));
            var buf = new byte[size];
            Marshal.Copy(ptr, buf, 0, (int)size);
            return buf;
        }

        public static List<Group> ReadGroups(string path) {
            IntPtr module = LoadLibraryExW(path, IntPtr.Zero, LOAD_LIBRARY_AS_DATAFILE | LOAD_LIBRARY_AS_IMAGE_RESOURCE);
            if (module == IntPtr.Zero) throw new Exception("LoadLibraryEx 失敗：" + Marshal.GetLastWin32Error());
            try {
                var names = new List<IntPtr>();
                var labels = new List<string>();
                EnumResourceNamesW(module, RT_GROUP_ICON, (m, t, n, p) => {
                    if (((long)n >> 16) == 0) { names.Add(n); labels.Add("#" + (long)n); }
                    else { string s = Marshal.PtrToStringUni(n); names.Add(Marshal.StringToHGlobalUni(s)); labels.Add(s); }
                    return true;
                }, IntPtr.Zero);
                var groups = new List<Group>();
                for (int g = 0; g < names.Count; g++) {
                    var grp = new Group { Name = labels[g], Dir = Read(module, names[g], RT_GROUP_ICON) };
                    int count = BitConverter.ToUInt16(grp.Dir, 4);
                    for (int i = 0; i < count; i++) {
                        int o = 6 + 14 * i;
                        var e = new Entry {
                            Width = grp.Dir[o] == 0 ? 256 : grp.Dir[o],
                            Height = grp.Dir[o + 1] == 0 ? 256 : grp.Dir[o + 1],
                            BitCount = BitConverter.ToUInt16(grp.Dir, o + 6),
                            Id = BitConverter.ToUInt16(grp.Dir, o + 12),
                        };
                        e.Data = Read(module, (IntPtr)e.Id, RT_ICON);
                        grp.Entries.Add(e);
                    }
                    groups.Add(grp);
                }
                return groups;
            } finally {
                FreeLibrary(module);
            }
        }

        // 系統要畫 size×size 時會從這個群組挑哪個 RT_ICON id（0＝失敗）。
        public static int Pick(byte[] dir, int size) {
            return LookupIconIdFromDirectoryEx(dir, true, size, size, 0);
        }
    }
}
'@
}

function Read-IcoEntries([string]$path) {
    $buf = [System.IO.File]::ReadAllBytes($path)
    $count = [BitConverter]::ToUInt16($buf, 4)
    $list = @()
    for ($i = 0; $i -lt $count; $i++) {
        $o = 6 + 16 * $i
        $w = if ($buf[$o] -eq 0) { 256 } else { [int]$buf[$o] }
        $bytes = [BitConverter]::ToUInt32($buf, $o + 8)
        $offset = [BitConverter]::ToUInt32($buf, $o + 12)
        $data = New-Object byte[] $bytes
        [Array]::Copy($buf, [int]$offset, $data, 0, [int]$bytes)
        $list += [pscustomobject]@{ Size = $w; Data = $data }
    }
    return $list
}

function Get-Sha256([byte[]]$data) {
    return [Convert]::ToHexString([System.Security.Cryptography.SHA256]::HashData($data)).Substring(0, 16)
}

$lines = New-Object System.Collections.Generic.List[string]
$fail = 0
$exeFull = (Resolve-Path $Exe).Path
$icoFull = (Resolve-Path $Ico).Path
$exeItem = Get-Item $exeFull
$lines.Add("dw-5.2 exe 圖示資源比對  $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')")
$lines.Add("exe：$exeFull（$($exeItem.Length) bytes，修改時間 $($exeItem.LastWriteTime.ToString('yyyy-MM-dd HH:mm:ss'))）")
$lines.Add("ico：$icoFull")

$icoEntries = Read-IcoEntries $icoFull
$groups = [DwIcon52.ResReader]::ReadGroups($exeFull)
$lines.Add("RT_GROUP_ICON 群組數：$($groups.Count)（$((($groups | ForEach-Object { $_.Name }) -join ', '))）")
if ($groups.Count -lt 1) {
    $lines.Add('FAIL  exe 沒有任何 RT_GROUP_ICON')
    $fail++
} else {
    # Windows 以第一個（名稱／id 最小者）群組作為檔案總管顯示的 exe 圖示；tauri-winres 只放一個群組。
    $grp = $groups[0]
    $lines.Add("比對群組 $($grp.Name)：$($grp.Entries.Count) 個尺寸 [$((($grp.Entries | ForEach-Object { $_.Width }) -join ','))]")
    foreach ($ie in $icoEntries) {
        $match = @($grp.Entries | Where-Object { $_.Width -eq $ie.Size -and $_.Height -eq $ie.Size })
        if ($match.Count -ne 1) {
            $lines.Add("FAIL  $($ie.Size)px：exe 群組內有 $($match.Count) 個同尺寸 entry")
            $fail++
            continue
        }
        $e = $match[0]
        $same = [System.Linq.Enumerable]::SequenceEqual([byte[]]$e.Data, [byte[]]$ie.Data)
        $picked = [DwIcon52.ResReader]::Pick($grp.Dir, $ie.Size)
        $pickOk = ($picked -eq $e.Id)
        $verdict = if ($same -and $pickOk) { 'PASS' } else { 'FAIL' }
        if ($verdict -eq 'FAIL') { $fail++ }
        $lines.Add(("{0}  {1,3}px  RT_ICON #{2,-3} {3,6} bytes  sha256[0..16]={4}  與 icon.ico 位元組相同={5}  系統挑選 id={6}（{7}）" -f `
                    $verdict, $ie.Size, $e.Id, $e.Data.Length, (Get-Sha256 $e.Data), $same, $picked, $(if ($pickOk) { '同尺寸' } else { '不是同尺寸' })))
    }
    $extra = @($grp.Entries | Where-Object { $w = $_.Width; -not ($icoEntries | Where-Object { $_.Size -eq $w }) })
    if ($extra.Count -gt 0) {
        $lines.Add("FAIL  exe 群組有 icon.ico 沒有的尺寸：$((($extra | ForEach-Object { $_.Width }) -join ','))")
        $fail++
    }
}
$lines.Add($(if ($fail -eq 0) { 'VERDICT PASS' } else { "VERDICT FAIL（$fail 項）" }))

$dir = Split-Path -Parent $Log
if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Force $dir | Out-Null }
$text = ConvertTo-EvidenceText (($lines -join "`r`n") + "`r`n")
[System.IO.File]::WriteAllText($Log, $text, (New-Object System.Text.UTF8Encoding($false)))
$text
exit $(if ($fail -eq 0) { 0 } else { 1 })
