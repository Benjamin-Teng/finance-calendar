# fetch-progress-and-manual-refresh 實機驗收（task 4.2、6.1、6.2）：隔離宿主＋空資料目錄。
# 只結束本腳本啟動的 PID；不注入輸入（視窗截圖用 PrintWindow、頁面讀值用 CDP）、不碰真實設定與 HKCU Run。
#
#   -Phase online   data_fetch=on：首抓中／抓完的小工具狀態與截圖，設定視窗按鈕按下後的時間序列（4.2、6.1）
#   -Phase offline  data_fetch=on、網路不通：載入動畫在該輪結束後換成失敗提示（6.2 前半；執行前先斷網）
#   -Phase off      data_fetch=off：小工具顯示抓取已關閉、設定視窗按鈕停用（6.2 後半、4.2 步驟 5）
param(
    [Parameter(Mandatory)][ValidateSet('online', 'offline', 'off')][string]$Phase,
    [string]$Exe = (Join-Path $PSScriptRoot '..\..\target\release\fc-host.exe'),
    [int]$CdpPort = 9351,
    [int]$RoundTimeoutSec = 480,
    [string]$RootBase = [IO.Path]::GetTempPath()
)
$ErrorActionPreference = 'Stop'
$tools = Split-Path $PSScriptRoot -Parent
Import-Module (Join-Path $tools 'lib\EvidenceLog.psm1') -Force
Import-Module (Join-Path $tools 'lib\AutostartRegistry.psm1') -Force

Add-Type -AssemblyName System.Drawing
Add-Type @'
using System; using System.Collections.Generic; using System.Runtime.InteropServices; using System.Text;
public static class FpWin {
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
[FpWin]::SetProcessDpiAwarenessContext([IntPtr](-4)) | Out-Null

if (Get-Process LogonUI, LockApp -ErrorAction SilentlyContinue) { Write-Output 'BLOCKED: 工作階段鎖定中'; exit 2 }
if (Get-Process fc-host -ErrorAction SilentlyContinue) { Write-Output 'BLOCKED: 已有 fc-host 在跑'; exit 2 }
$Exe = (Resolve-Path $Exe).Path
$logPath = Join-Path $PSScriptRoot "fetch-progress-$Phase.log"
$log = New-EvidenceWriter $logPath
$log.AutoFlush = $true
function Say([string]$m) { $line = "$(Get-Date -Format 'HH:mm:ss.fff') $m"; $log.WriteLine($line); Write-Output $line }

function Invoke-Eval([string]$UrlPart, [string]$Expr, [int]$TimeoutSec = 60) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($true) {
        $out = (& node (Join-Path $tools 'host-cdp-eval.mjs') $CdpPort $UrlPart $Expr 2>&1) -join ''
        if ($out -notmatch '找不到 url 含|fetch failed|尚未提交') { return $out }
        if ($sw.Elapsed.TotalSeconds -ge $TimeoutSec) { return $out }
        Start-Sleep -Milliseconds 400
    }
}

# 小工具頁面的顯示狀態：整塊載入、角落圖示、內容文字開頭、活動狀態。
$widgetProbe = @'
(async () => {
  const loading = document.querySelector('.fetch-loading');
  const corner = document.querySelector('.fetch-corner');
  const root = document.getElementById('widget-root');
  const shown = (e) => !!e && getComputedStyle(e).display !== 'none' && !e.hidden;
  let activity = null;
  try { activity = await window.__TAURI__.core.invoke('get_fetch_activity'); } catch (e) { activity = 'ERR ' + e; }
  return {
    loading: shown(loading), loadingText: loading ? loading.textContent.trim() : null,
    corner: shown(corner), rootShown: shown(root),
    text: root ? root.innerText.replace(/\s+/g, ' ').trim().slice(0, 70) : null,
    activity,
  };
})()
'@ -replace "`r?`n", ' '
$widgets = 'clock', 'macro', 'fixed', 'dynamic', 'quotes'
function Read-Widgets([string]$Label) {
    foreach ($w in $widgets) { Say "[$Label] $w $(Invoke-Eval "w=$w" $widgetProbe 20)" }
}
function Read-Activity { Invoke-Eval 'w=clock' "window.__TAURI__.core.invoke('get_fetch_activity')" 20 }

function Save-Shots([int]$HostPid, [string]$Label) {
    foreach ($w in [FpWin]::List([uint32]$HostPid)) {
        $parts = $w.Split('|')
        # 設定視窗不截：畫面上的資料目錄欄位含本機使用者名稱，按鈕狀態以 CDP 讀值為證。
        if ($parts[1] -notlike 'fc-host *') { continue }
        $c = $parts[2].Split(',') | ForEach-Object { [int]$_ }; $wd = $c[2] - $c[0]; $ht = $c[3] - $c[1]
        if ($wd -le 0 -or $ht -le 0) { continue }
        $name = if ($parts[1] -like '*設定*') { 'settings' } else { ($parts[1] -replace '^fc-host\s+', '') -replace '[^\w-]', '' }
        $file = Join-Path $PSScriptRoot "fetch-progress-$Phase-$Label-$name.png"
        $bmp = New-Object System.Drawing.Bitmap $wd, $ht; $g = [System.Drawing.Graphics]::FromImage($bmp)
        $hdc = $g.GetHdc(); [FpWin]::PrintWindow([IntPtr][long]$parts[0], $hdc, 2) | Out-Null; $g.ReleaseHdc($hdc)
        $bmp.Save($file); $g.Dispose(); $bmp.Dispose()
        Say "[$Label] 截圖 $name ${wd}x${ht}"
    }
}

# 無資料提示會把資料目錄畫在小工具上：-RootBase 指到使用者資料夾以外，截圖才不會帶出本機使用者名稱。
$root = Join-Path $RootBase ("fc-fp-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
$local = Join-Path $root 'Local'; $roaming = Join-Path $root 'Roaming'
$appRoam = Join-Path $roaming 'tw.fintools.fc-host'; $appLocal = Join-Path $local 'tw.fintools.fc-host'
New-Item -ItemType Directory -Force $appRoam, $appLocal | Out-Null
$mode = if ($Phase -eq 'off') { 'off' } else { 'on' }
Set-Content -Encoding utf8 (Join-Path $appRoam 'settings.json') "{`"version`":2,`"data_fetch`":`"$mode`"}"
$regSnap = @(Save-FcHostAutostartRegistry)

function Start-Host([string]$Arguments) {
    $psi = New-Object System.Diagnostics.ProcessStartInfo $Exe; $psi.UseShellExecute = $false
    if ($Arguments) { $psi.Arguments = $Arguments }
    $psi.EnvironmentVariables['LOCALAPPDATA'] = $local; $psi.EnvironmentVariables['APPDATA'] = $roaming
    $psi.EnvironmentVariables['WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS'] = "--remote-debugging-port=$CdpPort"
    return [System.Diagnostics.Process]::Start($psi)
}
function Get-HostLog([string]$Pattern) {
    Get-ChildItem (Join-Path $appLocal 'logs') -ErrorAction SilentlyContinue | ForEach-Object {
        $fs = [IO.File]::Open($_.FullName, 'Open', 'Read', 'ReadWrite,Delete'); $sr = New-Object IO.StreamReader $fs, ([Text.Encoding]::UTF8)
        try { $sr.ReadToEnd() -split "`r?`n" } finally { $sr.Dispose() }
    } | Where-Object { $_ -match $Pattern }
}
function Wait-Running([bool]$Want, [int]$TimeoutSec) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt $TimeoutSec) {
        $a = Read-Activity
        if ($a -match ('"running":' + $Want.ToString().ToLower())) { return $a }
        Start-Sleep -Milliseconds 500
    }
    return $null
}
function Open-Settings {
    $q = Start-Host ''
    $q.WaitForExit(15000) | Out-Null
    $r = Invoke-Eval 'settings.html' "new Promise(r => { const t = setInterval(() => { const b = document.getElementById('fetch-refresh'); const s = document.getElementById('fetch-status-text'); if (b && s && !/查詢/.test(s.textContent)) { clearInterval(t); r('ready'); } }, 100); setTimeout(() => { clearInterval(t); r('timeout'); }, 8000); })" 60
    Say "設定視窗：第二執行個體 exited=$($q.HasExited)，頁面 $r"
}
$settingsProbe = "(() => { const b = document.getElementById('fetch-refresh'); const s = document.getElementById('fetch-status-text'); return { btn: b ? b.textContent.trim() : null, disabled: b ? b.disabled : null, status: s ? s.textContent.trim() : null, notes: [...document.querySelectorAll('.fetch-note')].map(n => n.textContent.trim()).filter(Boolean) }; })()"

$p = $null
try {
    Say "# fetch-progress-run.ps1 phase=$Phase exe_mtime=$((Get-Item $Exe).LastWriteTime.ToString('o')) data_fetch=$mode"
    $p = Start-Host ''
    Say "宿主 pid=$($p.Id)"

    if ($Phase -eq 'online') {
        $a = Wait-Running $true 60
        Say "首抓開始：$a"
        Read-Widgets '抓取中'
        Save-Shots $p.Id 'fetching'
        $a = Wait-Running $false $RoundTimeoutSec
        Say "首抓結束：$a"
        Start-Sleep -Seconds 3
        Read-Widgets '抓完'
        Save-Shots $p.Id 'done'

        Open-Settings
        Say "[按下前] $(Invoke-Eval 'settings.html' $settingsProbe)"
        # 按下後每 200 ms 取樣 7 秒（CDP 單次指令逾時 10 秒），連同按下當下一起記在頁面內，一次取回（避免每次取樣的 CDP 連線成本）。
        $sample = "new Promise(res => { const out = []; const t0 = performance.now(); const q = (id) => document.getElementById(id); const snap = () => out.push([Math.round(performance.now() - t0), q('fetch-refresh').textContent.trim(), q('fetch-refresh').disabled, q('fetch-status-text').textContent.trim().slice(0, 30)]); snap(); q('fetch-refresh').click(); snap(); const t = setInterval(snap, 200); setTimeout(() => { clearInterval(t); res(out.filter((v, i, a) => i === 0 || v.slice(1).join('|') !== a[i - 1].slice(1).join('|'))); }, 7000); })"
        Say "[按下後 7 秒內的狀態變化（毫秒、按鈕文字、disabled、狀態文字）] $(Invoke-Eval 'settings.html' $sample 40)"
        Read-Widgets '手動更新中'
        Save-Shots $p.Id 'manual'
        Say "[進行中再要求一次] $(Invoke-Eval 'settings.html' "window.__TAURI__.core.invoke('request_fetch_now')")"
        $a = Wait-Running $false $RoundTimeoutSec
        Say "手動輪結束：$a"
        Start-Sleep -Seconds 2
        Say "[結束後] $(Invoke-Eval 'settings.html' $settingsProbe)"
        Read-Widgets '手動更新後'
        Save-Shots $p.Id 'after'
        Start-Sleep -Seconds 20
        Say "結束後 20 秒活動狀態（不應有緊接的第二輪）：$(Read-Activity)"
    }
    elseif ($Phase -eq 'offline') {
        $a = Wait-Running $true 60
        Say "首抓開始：$a"
        Read-Widgets '抓取中'
        Save-Shots $p.Id 'fetching'
        $a = Wait-Running $false $RoundTimeoutSec
        Say "首抓結束：$a"
        Start-Sleep -Seconds 3
        Read-Widgets '失敗後'
        Save-Shots $p.Id 'failed'
    }
    else {
        Start-Sleep -Seconds 8
        Say "活動狀態：$(Read-Activity)"
        Read-Widgets '抓取關閉'
        Open-Settings
        Say "[設定視窗] $(Invoke-Eval 'settings.html' $settingsProbe)"
        Say "[抓取關閉時要求更新] $(Invoke-Eval 'settings.html' "window.__TAURI__.core.invoke('request_fetch_now')")"
        Save-Shots $p.Id 'off'
    }
    Get-HostLog '觸發=|立即更新|\[網路\]|panic|fc_host::fetch.*(WARN|ERROR)' | Select-Object -Last 40 | ForEach-Object { Say "LOG $_" }
}
catch { Say "例外中止：$($_.Exception.Message)" }
finally {
    if ($p -and -not $p.HasExited) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue; $p.WaitForExit(15000) | Out-Null }
    $left = @(Restore-FcHostAutostartRegistry $regSnap)
    Say "宿主已結束；開機自啟登錄未還原 $($left.Count) 項"
    $log.Close()
    Start-Sleep -Seconds 2
    Remove-Item -Recurse -Force $root -ErrorAction SilentlyContinue
}
