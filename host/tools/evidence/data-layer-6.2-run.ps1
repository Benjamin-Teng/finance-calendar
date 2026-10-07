# data-layer-rust 6.2 實機：隔離宿主＋data_fetch "on" 從空資料目錄抓取；開設定視窗截圖；之後改回 auto 觀察。
# 只結束本腳本啟動的 PID；不注入輸入、不改桌布、不碰真實設定與 HKCU Run。
param([int]$PhaseASeconds = 150, [int]$PhaseBSeconds = 1800)
$ErrorActionPreference = 'Stop'
$repo = 'D:\projects\finance-calendar-data-layer-rust'
$exe = Join-Path $repo 'host\target\release\fc-host.exe'
$evDir = Join-Path $repo 'host\tools\evidence'
$log = Join-Path $evDir 'data-layer-6.2.log'

Add-Type -AssemblyName System.Drawing
Add-Type @'
using System; using System.Collections.Generic; using System.Runtime.InteropServices; using System.Text;
public static class W62 {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc p, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint f);
  public static List<string> List(uint pid) {
    var o = new List<string>();
    EnumWindows((h, l) => { uint p; GetWindowThreadProcessId(h, out p);
      if (p == pid && IsWindowVisible(h)) { var sb = new StringBuilder(256); GetWindowText(h, sb, 256); RECT r; GetWindowRect(h, out r);
        o.Add(h.ToInt64() + "|" + sb + "|" + r.L + "," + r.T + "," + r.R + "," + r.B); } return true; }, IntPtr.Zero);
    return o;
  }
}
'@
[W62]::SetProcessDpiAwarenessContext([IntPtr](-4)) | Out-Null

function Say($m) { $line = "$(Get-Date -Format 'HH:mm:ss') $m"; Add-Content -Encoding utf8 $log $line; Write-Output $line }
function Capture($pid_, $titleLike, $file) {
  foreach ($w in [W62]::List([uint32]$pid_)) {
    $parts = $w.Split('|'); if ($parts[1] -notlike $titleLike) { continue }
    $c = $parts[2].Split(',') | ForEach-Object { [int]$_ }; $wd = $c[2]-$c[0]; $ht = $c[3]-$c[1]
    if ($wd -le 0 -or $ht -le 0) { continue }
    $bmp = New-Object System.Drawing.Bitmap $wd, $ht; $g = [System.Drawing.Graphics]::FromImage($bmp)
    $hdc = $g.GetHdc(); [W62]::PrintWindow([IntPtr][long]$parts[0], $hdc, 2) | Out-Null; $g.ReleaseHdc($hdc)
    $bmp.Save($file); $g.Dispose(); $bmp.Dispose()
    return "$($parts[1]) ${wd}x${ht}"
  }
  return 'not-found'
}

if (Get-Process LogonUI -ErrorAction SilentlyContinue) { Write-Output 'BLOCKED: 工作階段鎖定中'; exit 2 }
if (Get-Process fc-host -ErrorAction SilentlyContinue) { Write-Output 'BLOCKED: 已有 fc-host 在跑'; exit 2 }
Set-Content -Encoding utf8 $log "data-layer-6.2 run exe_mtime=$((Get-Item $exe).LastWriteTime.ToString('o'))"

$root = Join-Path $env:TEMP ("fc-62-" + (Get-Date -Format 'yyyyMMdd-HHmmss'))
$local = Join-Path $root 'Local'; $roaming = Join-Path $root 'Roaming'
$appRoam = Join-Path $roaming 'tw.fintools.fc-host'; $appLocal = Join-Path $local 'tw.fintools.fc-host'
$dataDir = Join-Path $appLocal 'data'
New-Item -ItemType Directory -Force $dataDir, $appRoam | Out-Null
$settings = Join-Path $appRoam 'settings.json'
Set-Content -Encoding utf8 $settings '{"version":2,"data_fetch":"on"}'
$runBefore = (Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -ErrorAction SilentlyContinue).'fc-host'

function Start-Host($args_) {
  $psi = New-Object System.Diagnostics.ProcessStartInfo $exe; $psi.UseShellExecute = $false
  if ($args_) { $psi.Arguments = $args_ }
  $psi.EnvironmentVariables['LOCALAPPDATA'] = $local; $psi.EnvironmentVariables['APPDATA'] = $roaming
  return [System.Diagnostics.Process]::Start($psi)
}

# Phase A：data_fetch on，空資料目錄
$t0 = Get-Date
$p = Start-Host '--autostart'
Say "A: started pid=$($p.Id) root=$root"
Start-Sleep -Seconds $PhaseASeconds
$data = Join-Path $dataDir 'tw_events.json'; $state = Join-Path $appLocal 'fetch-state.json'
Say "A: alive=$(-not $p.HasExited) tw_events=$(Test-Path $data) fetch_state=$(Test-Path $state)"
if (Test-Path $data) {
  $j = Get-Content -Raw -Encoding utf8 $data | ConvertFrom-Json
  Say "A: written=$((Get-Item $data).LastWriteTime.ToString('HH:mm:ss')) counts=$($j.counts | ConvertTo-Json -Compress) errors=$($j.errors.Count) wallpaper_errors=$($j.wallpaper_errors.Count) updated=$($j.updated)"
}
$fl = Get-ChildItem (Join-Path $appLocal 'logs') -ErrorAction SilentlyContinue | ForEach-Object { Select-String -Path $_.FullName -Pattern 'fc_host::fetch' } | ForEach-Object { $_.Line }
$fl | Select-Object -First 30 | ForEach-Object { Say "A-log: $_" }
Say ("A: widget windows: " + (([W62]::List([uint32]$p.Id)) -join ' ; '))
# 小工具截圖（裁切到各自視窗，只含本宿主的視窗）
$i = 0; foreach ($w in [W62]::List([uint32]$p.Id)) { $i++; $t = $w.Split('|')[1]; Say ("A: capture " + (Capture $p.Id $t (Join-Path $evDir "data-layer-6.2-widget-$i.png"))) }
# 第二執行個體（不帶參數）→ 開設定視窗
$q = Start-Host ''
Start-Sleep -Seconds 8
Say "A: second instance exited=$($q.HasExited) code=$(if ($q.HasExited) { $q.ExitCode })"
Say ("A: settings capture " + (Capture $p.Id '*設定*' (Join-Path $evDir 'data-layer-6.2-settings.png')))
Say ("A: windows after: " + (([W62]::List([uint32]$p.Id)) -join ' ; '))
$hashA = if (Test-Path $data) { (Get-FileHash $data).Hash } else { '' }
$mtimeA = if (Test-Path $data) { (Get-Item $data).LastWriteTimeUtc } else { $null }
Stop-Process -Id $p.Id; $p.WaitForExit(15000) | Out-Null; Start-Sleep 3

# Phase B：拿掉 data_fetch（auto）→ 隔離偵測應停用抓取
Set-Content -Encoding utf8 $settings '{"version":2}'
$p2 = Start-Host '--autostart'
Say "B: started pid=$($p2.Id), observing ${PhaseBSeconds}s"
Start-Sleep -Seconds $PhaseBSeconds
$hashB = if (Test-Path $data) { (Get-FileHash $data).Hash } else { '' }
$mtimeB = if (Test-Path $data) { (Get-Item $data).LastWriteTimeUtc } else { $null }
Say "B: alive=$(-not $p2.HasExited) data_unchanged=$($hashA -eq $hashB -and $mtimeA -eq $mtimeB)"
$fl2 = Get-ChildItem (Join-Path $appLocal 'logs') | ForEach-Object { Select-String -Path $_.FullName -Pattern '隔離' } | ForEach-Object { $_.Line }
$fl2 | Select-Object -Last 3 | ForEach-Object { Say "B-log: $_" }
Stop-Process -Id $p2.Id; $p2.WaitForExit(15000) | Out-Null; Start-Sleep 3

$leftover = Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -like "*$root*" } | Select-Object -ExpandProperty ProcessId
$runAfter = (Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -ErrorAction SilentlyContinue).'fc-host'
Say "end: leftover_pids=$($leftover -join ',') hkcu_run_unchanged=$($runBefore -eq $runAfter)"
Copy-Item $data (Join-Path $evDir 'data-layer-6.2-tw_events.json') -ErrorAction SilentlyContinue
Remove-Item -Recurse -Force $root -ErrorAction SilentlyContinue
