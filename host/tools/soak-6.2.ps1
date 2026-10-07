<#
.SYNOPSIS
    Task 6.2 連續執行驗收：定時取樣宿主與其 WebView2 行程樹的記憶體／CPU、可見小工具數，
    判定「記憶體增幅低於 20%，期間小工具未消失」。不注入任何輸入、不啟動也不結束宿主。

.DESCRIPTION
    每 `-IntervalSec` 秒寫一列 CSV（`evidence/6.2-soak-<Tag>.csv`）：
    - host_pid、host_private_mb、host_ws_mb：fc-host 本身
    - wv_count、wv_private_mb、wv_ws_mb：fc-host 所有子孫行程（msedgewebview2）合計
    - total_private_mb：兩者私有記憶體合計（增幅判準用這欄）
    - cpu_pct：取樣間隔內整棵行程樹的 CPU 使用率（除以邏輯處理器數，0–100）
    - widgets：可見、未最小化、未 cloaked 的小工具視窗數（判定同 verify-grid-layout.ps1）
    - widget_ids：可見小工具 id，以 `|` 分隔
    - locked：LogonUI.exe 是否在執行
    - gap_s：與上一列的實際間隔秒數；大於 3 倍取樣間隔視為睡眠／待機（事件記錄檔另記一行）

    事件記錄（`evidence/6.2-soak-<Tag>.log`）只記狀態變化：宿主 PID 改變或消失、可見小工具
    數改變、鎖定／解鎖、睡眠間隔。

    判定（`-SummarizeOnly` 或跑滿時長後自動輸出；fix F4 起排除 host_pid=0 的列）：
    - 基準＝啟動後第 10–20 分鐘、宿主在執行的 total_private_mb 平均（避開 WebView2 暖機）；
      結尾＝「最後一個宿主仍在時」的 10 筆平均；增幅 < 20% 為 PASS。
    - 宿主在執行、未鎖定、且不在睡眠間隔後第一筆的樣本中，widgets 低於基準期眾數即記為「消失」；
      出現任何一筆為 FAIL。
    - 宿主不在（host_pid=0）的期間單獨列出起訖與時長，只要出現任何一段就判 FAIL。
      宿主 PID 改變（故障重啟）另列出。

.EXAMPLE
    pwsh -File host/tools/soak-6.2.ps1 -Tag run1                 # 跑 24 小時
    pwsh -File host/tools/soak-6.2.ps1 -Tag run1 -SummarizeOnly  # 隨時看目前結果
#>
[CmdletBinding()]
param(
    [string]$Tag = 'run1',
    [double]$DurationHours = 24,
    [int]$IntervalSec = 60,
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [switch]$SummarizeOnly
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force

$csvPath = Join-Path $OutDir "6.2-soak-$Tag.csv"
$logPath = Join-Path $OutDir "6.2-soak-$Tag.log"
$sumPath = Join-Path $OutDir "6.2-soak-$Tag-summary.log"

function Get-Ts { Get-Date -Format 'yyyy-MM-ddTHH:mm:ssK' }

function Get-HostAbsentPeriods {
    <#
    純函式（tests/Soak62.Tests.ps1 以 Parser 取出測試）。連續的 host_pid=0 列合成一段「宿主不在」：
    Start＝該段第一列時間、End＝宿主重新出現的那一列時間（持續到最後一列則為最後一列時間，Ongoing=$true）、
    Minutes＝End−Start。
    #>
    param([object[]]$Rows = @())
    $periods = New-Object System.Collections.Generic.List[object]
    $start = $null
    $lastAbsent = $null
    foreach ($r in $Rows) {
        if ([int]$r.host_pid -eq 0) {
            if ($null -eq $start) { $start = [datetime]$r.ts }
            $lastAbsent = [datetime]$r.ts
        } elseif ($null -ne $start) {
            $end = [datetime]$r.ts
            $periods.Add([PSCustomObject]@{ Start = $start; End = $end; Minutes = ($end - $start).TotalMinutes; Ongoing = $false })
            $start = $null
        }
    }
    if ($null -ne $start) {
        $periods.Add([PSCustomObject]@{ Start = $start; End = $lastAbsent; Minutes = ($lastAbsent - $start).TotalMinutes; Ongoing = $true })
    }
    return $periods.ToArray()
}

function Get-SoakSummary {
    <#
    純函式（tests/Soak62.Tests.ps1 以 Parser 取出測試）。回傳 @{ Lines; Verdict（PASS／FAIL／INCOMPLETE）;
    MemoryOk; WidgetsOk; HostAbsent; BaseMb; EndMb; Growth }。
    fix F4（soak 腳本）：
      - 記憶體／CPU／小工具計算一律排除 host_pid=0 的列（宿主不在時這些欄位是空的，原本被當成 0，
        結尾平均變 0 MB、增幅 −100% 而判 PASS）；
      - 結尾＝「最後一個宿主仍在時」的 10 筆平均；
      - 宿主不在的期間單獨列出起訖與時長，只要出現任何一段就判 FAIL。
    #>
    param([object[]]$Rows = @(), [int]$IntervalSec = 60)
    $out = New-Object System.Collections.Generic.List[string]
    $rows = @($Rows)
    if ($rows.Count -eq 0) {
        $out.Add('沒有任何樣本')
        return [PSCustomObject]@{ Lines = $out.ToArray(); Verdict = 'INCOMPLETE'; MemoryOk = $false; WidgetsOk = $false; HostAbsent = @(); BaseMb = $null; EndMb = $null; Growth = $null }
    }
    $out.Add("$($rows.Count) 筆，$($rows[0].ts) → $($rows[-1].ts)")
    $t0 = [datetime]$rows[0].ts
    $out.Add(('經過 {0:N2} 小時' -f ([datetime]$rows[-1].ts - $t0).TotalHours))
    $present = @($rows | Where-Object { [int]$_.host_pid -ne 0 })
    $absent = @(Get-HostAbsentPeriods -Rows $rows)

    $memOk = $false; $widgetsOk = $false; $baseMb = $null; $endMb = $null; $growth = $null; $complete = $true
    $base = @($present | Where-Object { $m = ([datetime]$_.ts - $t0).TotalMinutes; $m -ge 10 -and $m -lt 20 })
    if ($base.Count -eq 0) {
        $out.Add('基準期（第 10–20 分鐘、宿主在執行）尚無樣本，無法判定')
        $complete = $false
    } else {
        $baseMb = ($base | ForEach-Object { [double]$_.total_private_mb } | Measure-Object -Average).Average
        $tail = @($present | Select-Object -Last 10)
        $endMb = ($tail | ForEach-Object { [double]$_.total_private_mb } | Measure-Object -Average).Average
        $peak = ($present | ForEach-Object { [double]$_.total_private_mb } | Measure-Object -Maximum).Maximum
        $growth = ($endMb - $baseMb) / $baseMb * 100
        $memOk = $growth -lt 20
        $out.Add(('記憶體（私有，宿主＋WebView2；排除宿主不在的列）：基準 {0:N1} MB、結尾（最後一個宿主仍在時的 {5} 筆）{1:N1} MB（{6} → {7}）、峰值 {2:N1} MB、增幅 {3:N1}%  → {4}' -f `
                    $baseMb, $endMb, $peak, $growth, $(if ($memOk) { 'PASS' } else { 'FAIL' }), $tail.Count, $tail[0].ts, $tail[-1].ts))
        $cpuRows = @($present | Where-Object { $_.cpu_pct -ne '' })
        if ($cpuRows.Count -gt 0) {
            $cpu = $cpuRows | ForEach-Object { [double]$_.cpu_pct } | Measure-Object -Average -Maximum
            $out.Add(('CPU（整棵行程樹）：平均 {0:N2}%、最高 {1:N2}%' -f $cpu.Average, $cpu.Maximum))
        }
        $expect = [int](($base | Group-Object widgets | Sort-Object Count -Descending | Select-Object -First 1).Name)
        $prevGap = $false
        $missing = New-Object System.Collections.Generic.List[string]
        foreach ($r in $rows) {
            $isGap = [double]$r.gap_s -gt 3 * $IntervalSec
            # 宿主不在的列另列於下方「宿主不在的期間」，這裡只看宿主在執行時小工具有沒有消失。
            if ([int]$r.host_pid -ne 0 -and $r.locked -eq 'False' -and -not $isGap -and -not $prevGap -and [int]$r.widgets -lt $expect) {
                $missing.Add("$($r.ts) widgets=$($r.widgets) ids=$($r.widget_ids)")
            }
            $prevGap = $isGap
        }
        $widgetsOk = ($missing.Count -eq 0)
        $out.Add("可見小工具（宿主在執行時）：基準 $expect 個；未鎖定時少於基準 $($missing.Count) 筆 → $(if ($widgetsOk) { 'PASS' } else { 'FAIL' })")
        $missing | Select-Object -First 20 | ForEach-Object { $out.Add("  $_") }
    }

    if ($absent.Count -eq 0) {
        $out.Add('宿主不在的期間：無 → PASS')
    } else {
        $out.Add("宿主不在的期間：$($absent.Count) 段 → FAIL")
        foreach ($p in $absent) {
            $out.Add(('  {0:yyyy-MM-ddTHH:mm:ssK} → {1:yyyy-MM-ddTHH:mm:ssK}，{2:N1} 分鐘{3}' -f $p.Start, $p.End, $p.Minutes, $(if ($p.Ongoing) { '（持續到最後一筆，宿主未恢復）' } else { '' })))
        }
    }
    $pids = @($rows | Select-Object -ExpandProperty host_pid -Unique)
    $out.Add("宿主 PID：$($pids -join ' → ')")
    $gaps = @($rows | Where-Object { [double]$_.gap_s -gt 3 * $IntervalSec })
    $out.Add("睡眠／待機間隔：$($gaps.Count) 次；鎖定樣本 $(@($rows | Where-Object locked -eq 'True').Count) 筆")

    $verdict = if ($absent.Count -gt 0) { 'FAIL' }
               elseif (-not $complete) { 'INCOMPLETE' }
               elseif ($memOk -and $widgetsOk) { 'PASS' }
               else { 'FAIL' }
    $out.Add("=== 結論：$verdict ===")
    return [PSCustomObject]@{ Lines = $out.ToArray(); Verdict = $verdict; MemoryOk = $memOk; WidgetsOk = $widgetsOk; HostAbsent = $absent; BaseMb = $baseMb; EndMb = $endMb; Growth = $growth }
}

function Write-Summary {
    $s = Get-SoakSummary -Rows @(Import-Csv $csvPath) -IntervalSec $IntervalSec
    $lines = @("$(Get-Ts) 摘要：") + $s.Lines
    $lines | ConvertTo-EvidenceText | Set-Content -Encoding utf8 $sumPath
    $lines
}

if ($SummarizeOnly) { Write-Summary; return }

Add-Type -Namespace Soak -Name Native -MemberDefinition @'
[DllImport("user32.dll")] public static extern System.IntPtr GetTopWindow(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern System.IntPtr GetWindow(System.IntPtr hWnd, uint cmd);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern bool IsIconic(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr hWnd, out uint pid);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(System.IntPtr hWnd, System.Text.StringBuilder s, int n);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassName(System.IntPtr hWnd, System.Text.StringBuilder s, int n);
[DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(System.IntPtr h, int attr, out int v, int size);
'@

function Get-WidgetIds([int]$ProcId) {
    $ids = New-Object System.Collections.Generic.List[string]
    $h = [Soak.Native]::GetTopWindow([IntPtr]::Zero)
    while ($h -ne [IntPtr]::Zero) {
        $p = 0
        [void][Soak.Native]::GetWindowThreadProcessId($h, [ref]$p)
        if ($p -eq $ProcId -and [Soak.Native]::IsWindowVisible($h) -and -not [Soak.Native]::IsIconic($h)) {
            $t = New-Object System.Text.StringBuilder 256; [void][Soak.Native]::GetWindowText($h, $t, 256)
            $c = New-Object System.Text.StringBuilder 256; [void][Soak.Native]::GetClassName($h, $c, 256)
            $cloaked = 0; [void][Soak.Native]::DwmGetWindowAttribute($h, 14, [ref]$cloaked, 4)  # DWMWA_CLOAKED
            if ($t.ToString().StartsWith('fc-host ') -and $c.ToString() -eq 'Tauri Window' -and $cloaked -eq 0) {
                $ids.Add($t.ToString().Substring(8))
            }
        }
        $h = [Soak.Native]::GetWindow($h, 2)
    }
    return @($ids | Sort-Object)
}

function Get-Descendants([int]$Root, $All) {
    $result = New-Object System.Collections.Generic.List[int]
    $queue = New-Object System.Collections.Generic.Queue[int]
    $queue.Enqueue($Root)
    while ($queue.Count -gt 0) {
        $cur = $queue.Dequeue()
        foreach ($p in $All) {
            if ($p.ParentProcessId -eq $cur -and $p.ProcessId -ne $Root -and -not $result.Contains([int]$p.ProcessId)) {
                $result.Add([int]$p.ProcessId); $queue.Enqueue([int]$p.ProcessId)
            }
        }
    }
    return $result.ToArray()
}

$newFile = -not (Test-Path $csvPath)
$csv = [System.IO.StreamWriter]::new($csvPath, $true, [System.Text.UTF8Encoding]::new($false))
$csv.AutoFlush = $true
if ($newFile) {
    $csv.WriteLine('ts,host_pid,host_private_mb,host_ws_mb,wv_count,wv_private_mb,wv_ws_mb,total_private_mb,cpu_pct,widgets,widget_ids,locked,gap_s')
}
$log = New-EvidenceWriter $logPath
$log.AutoFlush = $true
function Log([string]$m) { $line = "$(Get-Ts) $m"; $log.WriteLine($line); Write-Host (ConvertTo-EvidenceText $line) }

$cores = [Environment]::ProcessorCount
Log "開始：時長 $DurationHours 小時、間隔 $IntervalSec 秒、邏輯處理器 $cores"
$deadline = (Get-Date).AddHours($DurationHours)
$prev = @{ Time = $null; Cpu = $null; Pid = $null; Widgets = $null; Locked = $null }

while ((Get-Date) -lt $deadline) {
    $now = Get-Date
    $gap = if ($prev.Time) { ($now - $prev.Time).TotalSeconds } else { 0 }
    if ($prev.Time -and $gap -gt 3 * $IntervalSec) { Log ('睡眠／待機間隔 {0:N0} 秒' -f $gap) }

    $locked = [bool](Get-Process LogonUI -ErrorAction SilentlyContinue)
    if ($locked -ne $prev.Locked) { Log "鎖定＝$locked" }

    $hostProc = Get-Process fc-host -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $hostProc) {
        if ($prev.Pid -ne 0) { Log '宿主不在執行' }
        $csv.WriteLine(('{0},0,,,,,,,,0,,{1},{2:F0}' -f (Get-Ts), $locked, $gap))
        $prev.Pid = 0; $prev.Cpu = $null; $prev.Widgets = 0
    } else {
        if ($prev.Pid -and $prev.Pid -ne $hostProc.Id) { Log "宿主 PID 改變：$($prev.Pid) → $($hostProc.Id)" }
        $all = Get-CimInstance Win32_Process -Property ProcessId, ParentProcessId
        $kids = Get-Descendants $hostProc.Id $all
        $kidProcs = @($kids | ForEach-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue })
        $cpuNow = $hostProc.TotalProcessorTime.TotalSeconds + ($kidProcs | Measure-Object { $_.TotalProcessorTime.TotalSeconds } -Sum).Sum
        $cpuPct = ''
        if ($prev.Cpu -ne $null -and $prev.Pid -eq $hostProc.Id -and $gap -gt 0) {
            $cpuPct = '{0:N3}' -f ([math]::Max(0.0, $cpuNow - $prev.Cpu) / $gap / $cores * 100)
        }
        $hp = $hostProc.PrivateMemorySize64 / 1MB; $hw = $hostProc.WorkingSet64 / 1MB
        $kp = ($kidProcs | Measure-Object PrivateMemorySize64 -Sum).Sum / 1MB
        $kw = ($kidProcs | Measure-Object WorkingSet64 -Sum).Sum / 1MB
        $ids = Get-WidgetIds $hostProc.Id
        if ($ids.Count -ne $prev.Widgets) { Log "可見小工具 $($ids.Count) 個：$($ids -join ',')" }
        $csv.WriteLine(('{0},{1},{2:F1},{3:F1},{4},{5:F1},{6:F1},{7:F1},{8},{9},{10},{11},{12:F0}' -f `
                    (Get-Ts), $hostProc.Id, $hp, $hw, $kidProcs.Count, $kp, $kw, ($hp + $kp), $cpuPct, $ids.Count, ($ids -join '|'), $locked, $gap))
        $prev.Pid = $hostProc.Id; $prev.Cpu = $cpuNow; $prev.Widgets = $ids.Count
    }
    $prev.Time = $now; $prev.Locked = $locked
    $sleepMs = [math]::Max(1000, $IntervalSec * 1000 - ((Get-Date) - $now).TotalMilliseconds)
    Start-Sleep -Milliseconds $sleepMs
}

Log '時長到，結束取樣'
$csv.Close(); $log.Close()
Write-Summary
