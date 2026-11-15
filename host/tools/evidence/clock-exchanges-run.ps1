# clock-six-exchanges 實機驗收：隔離宿主＋空資料目錄＋實網首抓。
# 驗兩件事：① 時鐘六列的文字與開收盤狀態，與 exchange-clock.mjs 在同一時刻算出來的一致；
#           ② 總經日曆從「資料抓取中」拿到第一份資料後，清單已捲到尚未發生的事件（scrollTop > 0）。
# 只結束本腳本啟動的 PID；不注入輸入（截圖用 PrintWindow、頁面讀值用 CDP）、不碰真實設定，HKCU Run 快照後還原。
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\..\target\release\fc-host.exe'),
    [int]$CdpPort = 9352,
    [int]$RoundTimeoutSec = 480,
    [string]$RootBase = [IO.Path]::GetTempPath()
)
$ErrorActionPreference = 'Stop'
$tools = Split-Path $PSScriptRoot -Parent
$repo = (Resolve-Path (Join-Path $tools '..\..')).Path
Import-Module (Join-Path $tools 'lib\EvidenceLog.psm1') -Force
Import-Module (Join-Path $tools 'lib\AutostartRegistry.psm1') -Force

Add-Type -AssemblyName System.Drawing
Add-Type @'
using System; using System.Collections.Generic; using System.Runtime.InteropServices; using System.Text;
public static class CxWin {
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
[CxWin]::SetProcessDpiAwarenessContext([IntPtr](-4)) | Out-Null

if (Get-Process LogonUI, LockApp -ErrorAction SilentlyContinue) { Write-Output 'BLOCKED: 工作階段鎖定中'; exit 2 }
if (Get-Process fc-host -ErrorAction SilentlyContinue) { Write-Output 'BLOCKED: 已有 fc-host 在跑'; exit 2 }
$Exe = (Resolve-Path $Exe).Path
$log = New-EvidenceWriter (Join-Path $PSScriptRoot 'clock-exchanges-run.log')
$log.AutoFlush = $true
$fail = 0
function Say([string]$m) { $line = "$(Get-Date -Format 'HH:mm:ss.fff') $m"; $log.WriteLine($line); Write-Output $line }
function Check([string]$name, [bool]$ok) { if ($ok) { Say "PASS $name" } else { Say "FAIL $name"; $script:fail++ } }

function Invoke-Eval([string]$UrlPart, [string]$Expr, [int]$TimeoutSec = 60) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($true) {
        $out = (& node (Join-Path $tools 'host-cdp-eval.mjs') $CdpPort $UrlPart $Expr 2>&1) -join ''
        if ($out -notmatch '找不到 url 含|fetch failed|尚未提交') { return $out }
        if ($sw.Elapsed.TotalSeconds -ge $TimeoutSec) { return $out }
        Start-Sleep -Milliseconds 400
    }
}
function Save-Shots([int]$HostPid, [string]$Label) {
    foreach ($w in [CxWin]::List([uint32]$HostPid)) {
        $parts = $w.Split('|')
        if ($parts[1] -notlike 'fc-host *' -or $parts[1] -like '*設定*') { continue }
        $c = $parts[2].Split(',') | ForEach-Object { [int]$_ }; $wd = $c[2] - $c[0]; $ht = $c[3] - $c[1]
        if ($wd -le 0 -or $ht -le 0) { continue }
        $name = ($parts[1] -replace '^fc-host\s+', '') -replace '[^\w-]', ''
        $file = Join-Path $PSScriptRoot "clock-exchanges-$Label-$name.png"
        $bmp = New-Object System.Drawing.Bitmap $wd, $ht; $g = [System.Drawing.Graphics]::FromImage($bmp)
        $hdc = $g.GetHdc(); [CxWin]::PrintWindow([IntPtr][long]$parts[0], $hdc, 2) | Out-Null; $g.ReleaseHdc($hdc)
        $bmp.Save($file); $g.Dispose(); $bmp.Dispose()
        Say "[$Label] 截圖 $name ${wd}x${ht}"
    }
}

$root = Join-Path $RootBase ("fc-cx-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
$local = Join-Path $root 'Local'; $roaming = Join-Path $root 'Roaming'
$appRoam = Join-Path $roaming 'tw.fintools.fc-host'; $appLocal = Join-Path $local 'tw.fintools.fc-host'
New-Item -ItemType Directory -Force $appRoam, $appLocal | Out-Null
Set-Content -Encoding utf8 (Join-Path $appRoam 'settings.json') '{"version":2,"data_fetch":"on"}'
$regSnap = @(Save-FcHostAutostartRegistry)

$clockProbe = "(() => { const rows = [...document.querySelectorAll('.clockcard .xrow')].map(r => ({ code: r.querySelector('.xcode').textContent, time: r.querySelector('.xtime').textContent, date: r.querySelector('.xdate').textContent, open: r.classList.contains('open'), color: getComputedStyle(r.querySelector('.xtime')).color })); const p = document.querySelector('.clockcard'); return { now: Date.now(), rows, overflow: p ? [p.scrollWidth - p.clientWidth, p.scrollHeight - p.clientHeight] : null, zoom: window.devicePixelRatio }; })()"
$macroProbe = "(() => { const l = document.querySelector('#widget-root .evlist') || document.querySelector('#widget-root [class*=list]'); const loading = document.querySelector('.fetch-loading'); return { loadingShown: !!loading && getComputedStyle(loading).display !== 'none', scrollTop: l ? l.scrollTop : null, scrollHeight: l ? l.scrollHeight : null, clientHeight: l ? l.clientHeight : null, rows: l ? l.children.length : 0 }; })()"
$activity = "window.__TAURI__.core.invoke('get_fetch_activity')"

$p = $null
try {
    Say "# clock-exchanges-run.ps1 exe_mtime=$((Get-Item $Exe).LastWriteTime.ToString('o'))"
    $psi = New-Object System.Diagnostics.ProcessStartInfo $Exe; $psi.UseShellExecute = $false
    $psi.EnvironmentVariables['LOCALAPPDATA'] = $local; $psi.EnvironmentVariables['APPDATA'] = $roaming
    $psi.EnvironmentVariables['WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS'] = "--remote-debugging-port=$CdpPort"
    $p = [System.Diagnostics.Process]::Start($psi)
    Say "宿主 pid=$($p.Id)"

    # ① 時鐘：等內建表載入（任何一列有機會變白之前先等 3 秒），讀 DOM，與同一時刻的純函式結果比。
    Start-Sleep -Seconds 6
    $clockJson = Invoke-Eval 'w=clock' $clockProbe 40
    Say "[時鐘 DOM] $clockJson"
    $clock = $clockJson | ConvertFrom-Json
    $expectJson = (& node --input-type=module -e "import { exchangeRows } from 'file:///$(($repo -replace '\\','/'))/host/ui/exchange-clock.mjs'; import fs from 'node:fs'; const cfg = JSON.parse(fs.readFileSync('$(($repo -replace '\\','/'))/host/ui/wallpapers/config/wallpaper-config.default.json','utf8')); console.log(JSON.stringify(exchangeRows(cfg, $($clock.now))));") -join ''
    Say "[時鐘 預期] $expectJson"
    $expect = $expectJson | ConvertFrom-Json
    Check '時鐘有六列' ($clock.rows.Count -eq 6)
    $same = $true
    for ($i = 0; $i -lt 6; $i++) {
        $a = $clock.rows[$i]; $b = $expect[$i]
        if ($a.code -ne $b.code -or $a.time -ne $b.time -or $a.date -ne $b.date -or [bool]$a.open -ne [bool]$b.open) { $same = $false; Say "  不一致 #$i DOM=$($a | ConvertTo-Json -Compress) 預期=$($b | ConvertTo-Json -Compress)" }
    }
    Check '六列的簡寫、時間、日期、開收盤與純函式一致' $same
    $colorOk = $true
    foreach ($r in $clock.rows) { $white = ($r.color -eq 'rgb(255, 255, 255)'); if ($white -ne [bool]$r.open) { $colorOk = $false; Say "  顏色不符 $($r.code) open=$($r.open) color=$($r.color)" } }
    Check '開盤列時間為白色、其餘不是白色' $colorOk
    $forced = Invoke-Eval 'w=clock' "(() => { const r = document.querySelector('.clockcard .xrow'); const had = r.classList.contains('open'); r.classList.add('open'); const c = getComputedStyle(r.querySelector('.xtime')).color; if (!had) r.classList.remove('open'); return c; })()" 20
    Say "[時鐘 強制加上 open 後的時間顏色] $forced"
    Check '加上 open 的列時間為白色（樣式有進執行檔）' ($forced -match 'rgb\(255, 255, 255\)')
    Check '時鐘內容沒有溢出面板' ($clock.overflow -and $clock.overflow[0] -le 0 -and $clock.overflow[1] -le 0)
    Save-Shots $p.Id 'start'

    # ② 總經日曆：首抓中應顯示載入；抓完後清單已捲動。
    $before = Invoke-Eval 'w=macro' $macroProbe 30
    Say "[總經 首抓中] $before  活動=$(Invoke-Eval 'w=clock' $activity 20)"
    $sw = [Diagnostics.Stopwatch]::StartNew(); $done = $null
    while ($sw.Elapsed.TotalSeconds -lt $RoundTimeoutSec) {
        $a = Invoke-Eval 'w=clock' $activity 20
        if ($a -match '"running":false' -and $a -match '"last":"(ok|failed)"') { $done = $a; break }
        Start-Sleep -Milliseconds 700
    }
    Say "首抓結束：$done"
    Check '首抓成功' ($done -match '"last":"ok"')
    Start-Sleep -Seconds 3
    $afterJson = Invoke-Eval 'w=macro' $macroProbe 30
    Say "[總經 抓完] $afterJson"
    $after = $afterJson | ConvertFrom-Json
    Check '總經日曆已離開載入畫面且有事件列' ((-not $after.loadingShown) -and $after.rows -gt 0)
    if ($after.scrollHeight -gt $after.clientHeight) {
        Check '總經日曆首抓後已捲到未發生事件（scrollTop > 0）' ($after.scrollTop -gt 0)
    } else {
        Say 'NOTE 總經清單沒有超出可視高度，無從捲動；本項不判定'
    }
    foreach ($w in 'fixed', 'dynamic', 'quotes') {
        Say "[$w] $(Invoke-Eval "w=$w" "(() => { const r = document.getElementById('widget-root'); return r ? r.innerText.replace(/\s+/g, ' ').trim().slice(0, 90) : null; })()" 20)"
    }
    Save-Shots $p.Id 'done'
    $errs = @(Get-ChildItem (Join-Path $appLocal 'logs') -ErrorAction SilentlyContinue | ForEach-Object {
        $fs = [IO.File]::Open($_.FullName, 'Open', 'Read', 'ReadWrite,Delete'); $sr = New-Object IO.StreamReader $fs, ([Text.Encoding]::UTF8)
        try { $sr.ReadToEnd() -split "`r?`n" } finally { $sr.Dispose() }
    } | Where-Object { $_ -match '\b(ERROR|panic)\b' })
    Say "記錄檔 ERROR／panic 行數：$($errs.Count)"
    $errs | Select-Object -First 5 | ForEach-Object { Say "  $_" }
    Check '宿主仍在執行' (-not $p.HasExited)
}
catch { Say "FAIL 腳本例外：$($_.Exception.Message)"; $fail++ }
finally {
    if ($p -and -not $p.HasExited) { $p.Kill(); $p.WaitForExit(10000) | Out-Null }
    $null = @(Restore-FcHostAutostartRegistry $regSnap)
    Start-Sleep -Seconds 2
    Remove-Item -Recurse -Force $root -ErrorAction SilentlyContinue
    Say "總結：$(if ($fail -eq 0) { 'PASS' } else { "FAIL（$fail 項）" })"
    $log.Dispose()
}
exit $(if ($fail -eq 0) { 0 } else { 1 })
