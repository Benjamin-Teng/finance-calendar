<#
.SYNOPSIS
    Task 4.5 實測：在隔離環境啟動「完整宿主＋--self-test-render」，邊渲染邊取樣宿主與全部 WebView2
    子孫的私有記憶體，並依 WebView2 使用者資料夾把子孫分成「小工具那組」與「渲染視窗那組」。

.DESCRIPTION
    宿主走正常啟動（小工具照常建立），setup 後由 self_test_render 依固定間隔以正式渲染管線出圖
    （每次開關隱藏視窗、獨立 WebView2 環境）。本稿：

    1. 前置檢查：工作階段未鎖定（LogonUI 不在）、沒有任何 fc-host 在跑（有就停下、不結束它）、exe 含
       FC_HOST_SELF_TEST_RENDER_BUILD 標記。
    2. 隔離：%APPDATA%、%LOCALAPPDATA%、WEBVIEW2_USER_DATA_FOLDER 只對本稿啟動的子行程指向
       %TEMP%\fc-measure-4.5-<時間>；資料目錄放一份 host/ui/fixtures/tw-events.json 讓小工具有資料。
       宿主啟動時接手 WEBVIEW2_USER_DATA_FOLDER（webview_env）：小工具那組＝<它>\EBWebView、
       渲染視窗那組＝<它>\wallpaper-renderer\EBWebView。執行中讀 msedgewebview2 命令列的
       --user-data-dir 核對兩組都在隔離根目錄內、且確實是兩個不同的資料夾。
    3. 取樣（每 -SampleMs）：與 soak-6.2.ps1 同一基準——宿主本身＋其所有 msedgewebview2 子孫的
       PrivateMemorySize64（total_private_mb）；另依 --user-data-dir 分組（widget_*／renderer_*）。
       寫 evidence/dw-4.5-<Tag>.csv。
    4. 結束：宿主跑完 self-test 會自己 app.exit（正常結束）；逾時才以 lib\ProcessTree.psm1 只停自己啟動
       的行程樹。結束後等 5 秒再查一次 fc-host，確認沒有被 Restart Manager 重新啟動。
    5. 摘要（evidence/dw-4.5-<Tag>-summary.log）：
       - 量測 1：每次渲染「開始前」的小工具那組／整棵樹私有記憶體序列、第一次增量與之後的斜率；
       - 量測 2：每次關窗後渲染那組行程全部結束所需時間（取樣解析度）、是否歸零；冷啟動耗時
         （宿主記錄的 to_png＝建立視窗＋載入＋繪製＋收到 PNG）中位數／最大；
       - 量測 3：整棵樹的曲線（CSV）與「每次渲染前」序列的斜率（MB／次、MB／小時）。

    證據經 lib\EvidenceLog.psm1 去識別。真實 %LOCALAPPDATA%\tw.fintools.fc-host 的
    EBWebView／wallpaper-renderer／wallpaper／logs 前後比對最後寫入時間。

    結束碼：0 宿主結束碼 0 且隔離核對通過；1 有失敗；2 參數／環境錯誤；3 BLOCKED。

.EXAMPLE
    cd host; cargo clean --release -p fc-host; cargo build --release --features self-test-ipc
    # 4K 單張（驗尺寸）
    pwsh -File host/tools/measure-dw-4.5.ps1 -Tag 4k -Count 1 -WarmupSec 20 -TailSec 15
    # 加速曲線：每 1 分鐘、30 分鐘
    pwsh -File host/tools/measure-dw-4.5.ps1 -Tag accel -IntervalSec 60 -DurationSec 1800 -WarmupSec 120 -TailSec 120
    # 正式曲線：每 15 分鐘、2 小時（各約 2 小時 10 分；兩組先後跑）
    pwsh -File host/tools/measure-dw-4.5.ps1 -Tag formal -Count 9 -IntervalSec 900 -WarmupSec 300 -TailSec 300 -SampleMs 2000 -SlopeFromSec 600 -ForceTimeoutAt 5
    # 同長度的不渲染對照組
    pwsh -File host/tools/measure-dw-4.5.ps1 -Tag formal-control -NoRender -Count 9 -IntervalSec 900 -WarmupSec 300 -TailSec 300 -SampleMs 2000 -SlopeFromSec 600
#>
[CmdletBinding()]
param(
    [string]$Tag = 'run',
    [int]$Count = 0,
    [int]$IntervalSec = 60,
    [int]$DurationSec = 0,
    [int]$WarmupSec = 60,
    [int]$TailSec = 60,
    [int]$Width = 3840,
    [int]$Height = 2160,
    [int]$SampleMs = 500,
    [switch]$NoFixture,
    # 對照組：同樣的時刻表但不渲染（宿主 --skip-render）。
    [switch]$NoRender,
    # 第 N 次渲染改畫一定不回報的頁面，在真實宿主上走一次 30 秒逾時（宿主 --timeout-at N；0＝不做）。
    [int]$ForceTimeoutAt = 0,
    # 暖機後斜率的起點（相對宿主啟動，秒）。
    [int]$SlopeFromSec = 600,
    # 渲染視窗資料夾（wallpaper-renderer）大小的量測間隔（秒）。
    [int]$UdfSizeEverySec = 30,
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [string]$ExePath = (Join-Path $PSScriptRoot '..\target\release\fc-host.exe')
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'lib\ProcessTree.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force

$ExePath = (Resolve-Path -LiteralPath $ExePath).Path
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$OutDir = (Resolve-Path -LiteralPath $OutDir).Path
$prefix = Join-Path $OutDir "dw-4.5-$Tag"
$driverLog = "$prefix-driver.log"
$csvPath = "$prefix.csv"
$renderLog = "$prefix-render.log"
$summaryPath = "$prefix-summary.log"
foreach ($p in @($driverLog, $csvPath, $renderLog, $summaryPath)) { if (Test-Path -LiteralPath $p) { Remove-Item -LiteralPath $p -Force } }
Set-Content -LiteralPath $driverLog -Value @() -Encoding utf8

function Write-Log([string]$Message) {
    $line = '{0} {1}' -f (Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fff'), $Message
    Add-Content -LiteralPath $driverLog -Value (ConvertTo-EvidenceText $line) -Encoding utf8
    Write-Host $line
}

function Get-EpochMs { [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() }

function Get-DirSize {
    <# 資料夾（含子資料夾）檔案大小合計（MB）與檔案數；不存在＝0。讀不到的檔略過。 #>
    param([string]$Path)
    if (-not (Test-Path -LiteralPath $Path)) { return [PSCustomObject]@{ Mb = 0.0; Files = 0 } }
    $files = @(Get-ChildItem -LiteralPath $Path -Recurse -File -Force -ErrorAction SilentlyContinue)
    $bytes = ($files | Measure-Object -Property Length -Sum).Sum
    return [PSCustomObject]@{ Mb = [double]$bytes / 1MB; Files = $files.Count }
}

# ── 純函式（摘要用）────────────────────────────────────────────────────────────────────────

function Get-LinearSlope {
    <# 最小平方法斜率（y 對 x）；少於 2 點回傳 $null。 #>
    param([double[]]$X, [double[]]$Y)
    $n = $X.Count
    if ($n -lt 2) { return $null }
    $mx = ($X | Measure-Object -Average).Average
    $my = ($Y | Measure-Object -Average).Average
    $num = 0.0; $den = 0.0
    for ($i = 0; $i -lt $n; $i++) { $num += ($X[$i] - $mx) * ($Y[$i] - $my); $den += ($X[$i] - $mx) * ($X[$i] - $mx) }
    if ($den -eq 0.0) { return $null }
    return $num / $den
}

function Get-Median([double[]]$Values) {
    if ($Values.Count -eq 0) { return $null }
    $s = @($Values | Sort-Object)
    $n = $s.Count
    if ($n % 2 -eq 1) { return [double]$s[[int](($n - 1) / 2)] }
    return ([double]$s[$n / 2 - 1] + [double]$s[$n / 2]) / 2.0
}

function Read-RenderEvents {
    <#
    解析宿主的 self-test 記錄：回傳每次渲染
    {N, StartMs, EndMs, Ok, Skipped, Forced, WindowClosed, ToPngMs, OpenMs, PageMs, CloseMs}。
    Skipped＝對照組（--skip-render）；Forced＝強制逾時那一次（--timeout-at），Ok＝它的 PASS 判定。
    #>
    param([string]$Path)
    $byN = @{}
    foreach ($line in (Get-Content -LiteralPath $Path -Encoding utf8)) {
        if ($line -match '^(\d+) RENDER-START n=(\d+)') {
            $byN[[int]$Matches[2]] = [ordered]@{ N = [int]$Matches[2]; StartMs = [int64]$Matches[1]; EndMs = $null; Ok = $false
                Skipped = $false; Forced = $false; WindowClosed = $null; ToPngMs = $null; OpenMs = $null; PageMs = $null; CloseMs = $null }
        } elseif ($line -match '^(\d+) RENDER-(OK|FAIL|SKIPPED|FORCED-TIMEOUT) n=(\d+)') {
            $e = $byN[[int]$Matches[3]]
            if ($null -eq $e) { continue }
            $e.EndMs = [int64]$Matches[1]
            switch ($Matches[2]) {
                'OK' { $e.Ok = $true }
                'SKIPPED' { $e.Ok = $true; $e.Skipped = $true }
                'FORCED-TIMEOUT' {
                    $e.Forced = $true
                    $e.Ok = $line -match ' PASS '
                    if ($line -match 'window_closed=Some\((true|false)\)') { $e.WindowClosed = $Matches[1] -eq 'true' }
                }
            }
            foreach ($k in @('to_png_ms', 'open_ms', 'page_ms', 'close_ms')) {
                if ($line -match " $k=([\d.]+)") {
                    $name = @{ to_png_ms = 'ToPngMs'; open_ms = 'OpenMs'; page_ms = 'PageMs'; close_ms = 'CloseMs' }[$k]
                    $e[$name] = [double]$Matches[1]
                }
            }
        }
    }
    return @($byN.Keys | Sort-Object | ForEach-Object { [PSCustomObject]$byN[$_] })
}

function Read-HostMarkers {
    <# 宿主記錄的時刻（UNIX 毫秒）：LoopEndMs（RENDER-LOOP-END）、TailSec（TAIL 秒數）、SummaryMs（SUMMARY）。 #>
    param([string]$Path)
    $m = [ordered]@{ LoopEndMs = $null; TailSec = 0; SummaryMs = $null }
    foreach ($line in (Get-Content -LiteralPath $Path -Encoding utf8)) {
        if ($line -match '^(\d+) RENDER-LOOP-END') { $m.LoopEndMs = [int64]$Matches[1] }
        elseif ($line -match '^(\d+) TAIL (\d+) s') { $m.TailSec = [int]$Matches[2] }
        elseif ($line -match '^(\d+) SUMMARY ') { $m.SummaryMs = [int64]$Matches[1] }
    }
    return [PSCustomObject]$m
}

function Get-TimeSlopes {
    <# 閒置樣本（渲染那組 0 個行程）在 [FromMs, ToMs) 內對時間（小時）的迴歸斜率：total／widget／host，MB／時。 #>
    param([object[]]$Rows, [int64]$T0, [int64]$FromMs, [int64]$ToMs)
    $win = @($Rows | Where-Object { [int]$_.renderer_count -eq 0 -and [int64]$_.ts_ms -ge $FromMs -and [int64]$_.ts_ms -lt $ToMs })
    if ($win.Count -lt 2) { return $null }
    $x = [double[]]($win | ForEach-Object { ([int64]$_.ts_ms - $T0) / 3600000.0 })
    return [PSCustomObject]@{
        N = $win.Count
        Total = Get-LinearSlope -X $x -Y ([double[]]($win | ForEach-Object { [double]$_.total_private_mb }))
        Widget = Get-LinearSlope -X $x -Y ([double[]]($win | ForEach-Object { [double]$_.widget_private_mb }))
        Host = Get-LinearSlope -X $x -Y ([double[]]($win | ForEach-Object { [double]$_.host_private_mb }))
        TotalP50 = Get-Median ([double[]]($win | ForEach-Object { [double]$_.total_private_mb }))
    }
}

function Get-Measure45Summary {
    <#
    由取樣列（CSV 物件）、渲染事件與宿主時刻算出三項量測。回傳文字行陣列。
    -WarmupSec：完整斜率的起點（相對第一列）；-SlopeFromSec：暖機後斜率的起點（相對第一列）。
    #>
    param([object[]]$Rows, [object[]]$Renders, [object]$Markers, [int]$IntervalSec, [int]$WarmupSec, [int]$SlopeFromSec)
    $lines = New-Object System.Collections.Generic.List[string]
    $rows = @($Rows | Where-Object { [int]$_.host_pid -ne 0 })
    if ($rows.Count -eq 0 -or $Renders.Count -eq 0) { $lines.Add('SUMMARY 無資料'); return $lines.ToArray() }
    $t0 = [int64]$rows[0].ts_ms
    $control = @($Renders | Where-Object Skipped).Count -eq $Renders.Count

    # 每次渲染開始前最後一筆（＝前一次關窗後的閒置水位；第一次＝暖機後基準）
    $pre = foreach ($r in $Renders) {
        $row = $rows | Where-Object { [int64]$_.ts_ms -lt $r.StartMs } | Select-Object -Last 1
        if ($row) { [PSCustomObject]@{ N = $r.N; StartMs = $r.StartMs; Total = [double]$row.total_private_mb; Widget = [double]$row.widget_private_mb
                Host = [double]$row.host_private_mb; Renderer = [double]$row.renderer_private_mb; RendererCount = [int]$row.renderer_count } }
    }
    $pre = @($pre)
    $lines.Add(('BASIS 宿主＋全部 msedgewebview2 子孫的 PrivateMemorySize64（同 soak-6.2 total_private_mb）；取樣 {0} 列；{1}' -f `
            $rows.Count, $(if ($control) { '對照組（--skip-render，不渲染）' } else { '渲染組' })))

    $lines.Add('M1 每次渲染開始前（前一次關窗後的閒置水位）：n total_mb widget_mb host_mb renderer_mb renderer_procs')
    foreach ($p in $pre) { $lines.Add(('M1   {0,3} {1,8:N1} {2,8:N1} {3,7:N1} {4,8:N1} {5}' -f $p.N, $p.Total, $p.Widget, $p.Host, $p.Renderer, $p.RendererCount)) }

    # 結尾穩態：TAIL 後半段、SUMMARY 前 2 秒以前的閒置樣本中位數（不取最後一筆：那是宿主結束過程中的暫態）。
    if ($Markers.LoopEndMs -and $Markers.SummaryMs) {
        $from = $Markers.LoopEndMs + [int64]($Markers.TailSec * 500)
        $to = $Markers.SummaryMs - 2000
        $tail = @($rows | Where-Object { [int]$_.renderer_count -eq 0 -and [int64]$_.ts_ms -ge $from -and [int64]$_.ts_ms -lt $to })
        if ($tail.Count -gt 0) {
            $lines.Add(('M1   結尾穩態（尾段後半、宿主結束前 2 秒以前的閒置樣本中位數，{0} 筆）total={1:N1} widget={2:N1} host={3:N1} renderer_procs=0' -f `
                    $tail.Count, (Get-Median ([double[]]($tail | ForEach-Object { [double]$_.total_private_mb }))),
                    (Get-Median ([double[]]($tail | ForEach-Object { [double]$_.widget_private_mb }))),
                    (Get-Median ([double[]]($tail | ForEach-Object { [double]$_.host_private_mb })))))
        } else {
            $lines.Add('M1   結尾穩態：尾段太短，沒有可用樣本')
        }
    }
    if ($pre.Count -ge 3) {
        $first = $pre[1].Widget - $pre[0].Widget
        $rest = @($pre | Select-Object -Skip 1)
        $sw = Get-LinearSlope -X ([double[]]($rest | ForEach-Object N)) -Y ([double[]]($rest | ForEach-Object Widget))
        $st = Get-LinearSlope -X ([double[]]($rest | ForEach-Object N)) -Y ([double[]]($rest | ForEach-Object Total))
        $sh = Get-LinearSlope -X ([double[]]($rest | ForEach-Object N)) -Y ([double[]]($rest | ForEach-Object Host))
        $lines.Add(('M1 小工具那組：第 1→2 次增量 {0:N2} MB；第 2 次起斜率 {1:N3} MB／次（{2} 點）；首→末 {3:N1}→{4:N1}' -f `
                $first, $sw, $rest.Count, $pre[0].Widget, $pre[-1].Widget))
        $lines.Add(('M1 宿主本身：第 2 次起斜率 {0:N3} MB／次；整棵樹：第 2 次起斜率 {1:N3} MB／次（×{2:N1} 次／時＝{3:N1} MB／時）' -f `
                $sh, $st, (3600.0 / $IntervalSec), ($st * 3600.0 / $IntervalSec)))
        $lines.Add(('M1 整棵樹首→末 {0:N1}→{1:N1} MB（{2:+0.0;-0.0}%）' -f $pre[0].Total, $pre[-1].Total, (($pre[-1].Total - $pre[0].Total) / $pre[0].Total * 100)))
        $post = @($pre | Where-Object { $_.StartMs -ge ($t0 + [int64]$SlopeFromSec * 1000) })
        if ($post.Count -ge 2) {
            $pw = Get-LinearSlope -X ([double[]]($post | ForEach-Object N)) -Y ([double[]]($post | ForEach-Object Widget))
            $pt = Get-LinearSlope -X ([double[]]($post | ForEach-Object N)) -Y ([double[]]($post | ForEach-Object Total))
            $lines.Add(('M1 暖機後（第 {0} 秒起，{1} 點）：小工具那組斜率 {2:N3} MB／次、整棵樹 {3:N3} MB／次；整棵樹 {4:N1}→{5:N1}' -f `
                    $SlopeFromSec, $post.Count, $pw, $pt, $post[0].Total, $post[-1].Total))
        }
    }
    # 時間迴歸（閒置樣本）：完整（暖機結束到渲染迴圈結束）與暖機後（第 SlopeFromSec 秒起）
    $endMs = if ($Markers.LoopEndMs) { $Markers.LoopEndMs } else { [int64]$rows[-1].ts_ms }
    foreach ($w in @(@{ Name = '完整'; From = $t0 + [int64]$WarmupSec * 1000 }, @{ Name = '暖機後'; From = $t0 + [int64]$SlopeFromSec * 1000 })) {
        $s = Get-TimeSlopes -Rows $rows -T0 $t0 -FromMs $w.From -ToMs $endMs
        if ($s) {
            $lines.Add(('SLOPE {0}（第 {1:N0}–{2:N0} 秒，閒置樣本 {3} 筆）：整棵樹 {4:+0.00;-0.00} MB／時、小工具那組 {5:+0.00;-0.00}、宿主 {6:+0.00;-0.00}；整棵樹中位數 {7:N1} MB' -f `
                    $w.Name, (($w.From - $t0) / 1000), (($endMs - $t0) / 1000), $s.N, $s.Total, $s.Widget, $s.Host, $s.TotalP50))
        }
    }

    $real = @($Renders | Where-Object { -not $_.Skipped })
    if ($real.Count -gt 0) {
        $lines.Add('M2 每次：n ok to_png_ms open_ms page_ms close_ms 渲染那組峰值MB 關窗後歸零ms（取樣解析度）')
        $exitMs = New-Object System.Collections.Generic.List[double]
        $neverZero = 0
        foreach ($r in $real) {
            $during = @($rows | Where-Object { [int64]$_.ts_ms -ge $r.StartMs -and ($null -eq $r.EndMs -or [int64]$_.ts_ms -le ($r.EndMs + 1000)) })
            $peak = ($during | ForEach-Object { [double]$_.renderer_private_mb } | Measure-Object -Maximum).Maximum
            $zero = $null
            if ($r.EndMs) {
                $z = $rows | Where-Object { [int64]$_.ts_ms -ge $r.EndMs -and [int]$_.renderer_count -eq 0 } | Select-Object -First 1
                if ($z) { $zero = [int64]$z.ts_ms - $r.EndMs; $exitMs.Add([double]$zero) } else { $neverZero++ }
            }
            $tag = if ($r.Forced) { ' 強制逾時 window_closed={0}' -f $r.WindowClosed } else { '' }
            $lines.Add(('M2   {0,3} {1} {2,7} {3,7} {4,7} {5,6} {6,7:N1} {7}{8}' -f $r.N, $r.Ok, $r.ToPngMs, $r.OpenMs, $r.PageMs, $r.CloseMs, $peak,
                    $(if ($null -eq $zero) { '未歸零' } else { $zero }), $tag))
            if ($r.Forced) {
                $lines.Add(('M2 強制逾時（第 {0} 次）：{1}；視窗已關閉並等到 Destroyed＝{2}；渲染那組 browser 行程{3}' -f `
                        $r.N, $(if ($r.Ok) { 'PASS' } else { 'FAIL' }), $r.WindowClosed,
                        $(if ($null -eq $zero) { '未結束' } else { "在逾時後 $zero ms 內全部結束" })))
            }
        }
        $ok = @($real | Where-Object { $_.Ok -and -not $_.Forced })
        $toPng = [double[]]@($ok | ForEach-Object ToPngMs)
        $open = [double[]]@($ok | ForEach-Object OpenMs)
        $lines.Add(('M2 成功 {0}/{1}（不含強制逾時）；冷啟動（to_png＝建立視窗＋載入＋繪製＋收到 PNG）中位數 {2:N1} ms、最大 {3:N1} ms；建立視窗（含新 browser 行程）中位數 {4:N1} ms、最大 {5:N1} ms' -f `
                $ok.Count, @($real | Where-Object { -not $_.Forced }).Count, (Get-Median $toPng), (($toPng | Measure-Object -Maximum).Maximum), (Get-Median $open), (($open | Measure-Object -Maximum).Maximum)))
        $lines.Add(('M2 關窗後渲染那組行程全部結束：{0}/{1} 次歸零；歸零時間中位數 {2} ms、最大 {3} ms；未歸零 {4} 次' -f `
                $exitMs.Count, $real.Count, (Get-Median ([double[]]$exitMs)), (($exitMs | Measure-Object -Maximum).Maximum), $neverZero))
    }
    $udf = @($rows | Where-Object { $_.renderer_udf_mb -ne '' -and $null -ne $_.renderer_udf_mb })
    if ($udf.Count -gt 0) {
        $firstAfter = $udf | Where-Object { [double]$_.renderer_udf_mb -gt 0 } | Select-Object -First 1
        $lines.Add(('UDF 渲染視窗資料夾（wallpaper-renderer）大小：第一次有內容 {0} MB／{1} 檔 → 最後 {2} MB／{3} 檔；最大 {4} MB' -f `
                $(if ($firstAfter) { $firstAfter.renderer_udf_mb } else { 0 }), $(if ($firstAfter) { $firstAfter.renderer_udf_files } else { 0 }),
                $udf[-1].renderer_udf_mb, $udf[-1].renderer_udf_files, (($udf | ForEach-Object { [double]$_.renderer_udf_mb } | Measure-Object -Maximum).Maximum)))
    }
    $peakTotal = ($rows | ForEach-Object { [double]$_.total_private_mb } | Measure-Object -Maximum).Maximum
    $lines.Add(('M3 整棵樹峰值 {0:N1} MB；曲線見 CSV（total_private_mb）' -f $peakTotal))
    return $lines.ToArray()
}

# ── 前置檢查 ───────────────────────────────────────────────────────────────────────────
if (Get-Process -Name LogonUI -ErrorAction SilentlyContinue) {
    Write-Log 'BLOCKED 工作階段鎖定中（LogonUI.exe 在跑），不啟動。'
    exit 3
}
$running = @(Get-Process -Name fc-host -ErrorAction SilentlyContinue)
if ($running.Count -gt 0) {
    Write-Log ("ABORT 已有 fc-host 在跑（PID {0}），不啟動、也不結束它。" -f (($running | ForEach-Object Id) -join ','))
    exit 2
}
$exeText = [System.Text.Encoding]::Latin1.GetString([System.IO.File]::ReadAllBytes($ExePath))
if (-not $exeText.Contains('FC_HOST_SELF_TEST_RENDER_BUILD')) {
    Write-Log "ABORT $ExePath 不含 self-test-render 建置標記（請以 cargo build --release --features self-test-ipc 建置）。"
    exit 2
}
$exeText = $null
Write-Log "PRECHECK ok exe=$ExePath unlocked=true fcHostRunning=0 marker=present"

$realBase = Join-Path $env:LOCALAPPDATA 'tw.fintools.fc-host'
function Get-RealState {
    $o = [ordered]@{}
    foreach ($sub in @('EBWebView', 'wallpaper-renderer', 'wallpaper', 'logs')) {
        $p = Join-Path $realBase $sub
        if (Test-Path -LiteralPath $p) {
            $latest = Get-ChildItem -LiteralPath $p -Recurse -File -Force -ErrorAction SilentlyContinue |
                Sort-Object LastWriteTimeUtc -Descending | Select-Object -First 1
            $o[$sub] = if ($latest) { '{0:o}' -f $latest.LastWriteTimeUtc } else { 'empty' }
        } else {
            $o[$sub] = 'absent'
        }
    }
    return $o
}
$realBefore = Get-RealState
Write-Log ("REAL-BEFORE {0}" -f (($realBefore.GetEnumerator() | ForEach-Object { "$($_.Key)=$($_.Value)" }) -join ' '))

# ── 隔離 ───────────────────────────────────────────────────────────────────────────────
$isoRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("fc-measure-4.5-{0}" -f (Get-Date -Format 'yyyyMMdd-HHmmss'))
$isoAppData = Join-Path $isoRoot 'appdata'
$isoLocal = Join-Path $isoRoot 'localappdata'
$isoUdf = Join-Path $isoRoot 'webview2'
$isoData = Join-Path $isoLocal 'tw.fintools.fc-host\data'
foreach ($d in @($isoAppData, $isoLocal, $isoUdf, $isoData)) { New-Item -ItemType Directory -Force -Path $d | Out-Null }
$isoRoot = (Resolve-Path -LiteralPath $isoRoot).Path
Copy-Item -LiteralPath (Join-Path $PSScriptRoot '..\ui\fixtures\tw-events.json') -Destination (Join-Path $isoData 'tw_events.json')
Write-Log "ISOLATION root=$isoRoot"

$hostArgs = @('--self-test-render', '--render-log', $renderLog, '--interval-secs', $IntervalSec,
    '--warmup-secs', $WarmupSec, '--tail-secs', $TailSec, '--width', $Width, '--height', $Height)
if ($Count -gt 0) { $hostArgs += @('--count', $Count) } elseif ($DurationSec -gt 0) { $hostArgs += @('--duration-secs', $DurationSec) }
if ($NoFixture) { $hostArgs += @('--no-fixture') }
if ($NoRender) { $hostArgs += @('--skip-render') }
if ($ForceTimeoutAt -gt 0) { $hostArgs += @('--timeout-at', $ForceTimeoutAt) }
$hostArgs = $hostArgs | ForEach-Object { $s = [string]$_; if ($s -match '\s') { '"' + $s + '"' } else { $s } }
$renders = if ($Count -gt 0) { $Count } elseif ($DurationSec -gt 0) { [Math]::Max(1, [Math]::Floor($DurationSec / $IntervalSec)) } else { 1 }
$timeoutSec = $WarmupSec + $TailSec + $renders * $IntervalSec + 300

$saved = @{ APPDATA = $env:APPDATA; LOCALAPPDATA = $env:LOCALAPPDATA; WEBVIEW2_USER_DATA_FOLDER = $env:WEBVIEW2_USER_DATA_FOLDER }
try {
    $env:APPDATA = $isoAppData
    $env:LOCALAPPDATA = $isoLocal
    $env:WEBVIEW2_USER_DATA_FOLDER = $isoUdf
    $proc = Start-Process -FilePath $ExePath -ArgumentList $hostArgs -PassThru
} finally {
    $env:APPDATA = $saved.APPDATA
    $env:LOCALAPPDATA = $saved.LOCALAPPDATA
    $env:WEBVIEW2_USER_DATA_FOLDER = $saved.WEBVIEW2_USER_DATA_FOLDER
}
$rootStart = Get-ProcessStartTimeOrNull $proc
Write-Log "START pid=$($proc.Id) renders=$renders timeoutSec=$timeoutSec args=$($hostArgs -join ' ')"

# ── 取樣 ───────────────────────────────────────────────────────────────────────────────
$csv = New-EvidenceWriter -Path $csvPath
$csv.WriteLine('ts_ms,host_pid,host_private_mb,wv_count,wv_private_mb,total_private_mb,widget_count,widget_private_mb,renderer_count,renderer_private_mb,renderer_udf_mb,renderer_udf_files')
# 每次取樣只用一份 Get-Process 快照（記憶體與父 PID 都出自它；CIM 全表查詢約 0.7 秒，太慢，渲染那組
# 整個生命期才約 1 秒）。父 PID 以 Process.Parent（NtQueryInformationProcess）讀、首次看到時記下。
# 分組：沿父鏈找到「宿主的直接子行程」（各 WebView2 環境的 browser 行程）；每個 browser 行程首次出現時
# 以 CIM 讀一次命令列的 --user-data-dir 判定屬哪一組（讀不到＝已結束：小工具那組已知時歸渲染那組）。
$procCache = @{}   # "pid|ticks" → @{ Parent; Created }
$browserGroup = @{}   # 宿主直接子行程的 "pid|ticks" → 'widget'／'renderer'
$udfSeen = @{}
$overall = 0
$deadline = (Get-Date).AddSeconds($timeoutSec)
$rendererUdfDir = Join-Path $isoUdf 'wallpaper-renderer'
$udfLastMs = 0
$udfStat = [PSCustomObject]@{ Mb = 0.0; Files = 0 }
$keyOf = { param($p) try { '{0}|{1}' -f $p.Id, $p.StartTime.Ticks } catch { '{0}|?' -f $p.Id } }
# 渲染那組的 browser 行程只活約 1 秒，主迴圈（每 -SampleMs）來不及讀它的命令列；另開一條執行緒每 50 ms
# 找宿主的新直接子行程，立刻讀命令列寫進隔離目錄下的暫存檔（每行：鍵<TAB>命令列）。
$udfFile = Join-Path $isoRoot 'browser-cmdlines.tsv'
$udfJob = Start-ThreadJob -ArgumentList $proc.Id, $udfFile -ScriptBlock {
    param($hostId, $outFile)
    $seen = @{}
    while (Get-Process -Id $hostId -ErrorAction SilentlyContinue) {
        foreach ($p in @(Get-Process -Name msedgewebview2 -ErrorAction SilentlyContinue)) {
            $k = try { '{0}|{1}' -f $p.Id, $p.StartTime.Ticks } catch { '{0}|?' -f $p.Id }
            if ($seen.ContainsKey($k)) { continue }
            $seen[$k] = 1
            $parent = try { $p.Parent.Id } catch { $null }
            if ($parent -ne $hostId) { continue }
            $c = Get-CimInstance Win32_Process -Filter "ProcessId=$($p.Id)" -ErrorAction SilentlyContinue | Select-Object -First 1
            Add-Content -LiteralPath $outFile -Value ("{0}`t{1}" -f $k, [string]$c.CommandLine) -Encoding utf8
        }
        Start-Sleep -Milliseconds 50
    }
}
function Get-BrowserCmdlines {
    $map = @{}
    if (Test-Path -LiteralPath $udfFile) {
        foreach ($line in (Get-Content -LiteralPath $udfFile -Encoding utf8)) {
            $parts = $line -split "`t", 2
            if ($parts.Count -eq 2) { $map[$parts[0]] = $parts[1] }
        }
    }
    return $map
}
while (-not $proc.HasExited) {
    $loopStart = Get-EpochMs
    $wvProcs = @(Get-Process -Name msedgewebview2 -ErrorAction SilentlyContinue)
    $hostP = Get-Process -Id $proc.Id -ErrorAction SilentlyContinue
    $t = Get-EpochMs
    $treeInput = New-Object System.Collections.Generic.List[object]
    $treeInput.Add([PSCustomObject]@{ ProcessId = $proc.Id; ParentProcessId = 0; CreationDate = $rootStart })
    $byId = @{}
    foreach ($p in $wvProcs) {
        $key = & $keyOf $p
        if (-not $procCache.ContainsKey($key)) {
            $parent = try { $p.Parent.Id } catch { $null }
            $created = try { $p.StartTime } catch { $null }
            $procCache[$key] = @{ Parent = $(if ($null -eq $parent) { -1 } else { [int]$parent }); Created = $created }
        }
        $info = $procCache[$key]
        $byId[$p.Id] = @{ Proc = $p; Key = $key; Parent = $info.Parent }
        $treeInput.Add([PSCustomObject]@{ ProcessId = $p.Id; ParentProcessId = $info.Parent; CreationDate = $info.Created })
    }
    $ids = @(Get-ProcessTreeIds -RootId $proc.Id -RootStartTime $rootStart -Processes $treeInput.ToArray())
    $hostMb = if ($hostP) { $hostP.PrivateMemorySize64 / 1MB } else { 0 }
    $wv = 0.0; $kidN = 0; $wN = 0; $wMb = 0.0; $rN = 0; $rMb = 0.0
    foreach ($id in $ids) {
        if ($id -eq $proc.Id -or -not $byId.ContainsKey($id)) { continue }
        # 沿父鏈找宿主的直接子行程（browser）
        $top = $id
        $guard = 0
        while ($byId.ContainsKey($top) -and $byId[$top].Parent -ne $proc.Id -and $guard -lt 8) { $top = $byId[$top].Parent; $guard++ }
        $group = 'widget'
        if ($byId.ContainsKey($top)) {
            $bkey = $byId[$top].Key
            if (-not $browserGroup.ContainsKey($bkey)) {
                $cmdline = (Get-BrowserCmdlines)[$bkey]
                if (-not $cmdline) {
                    $cmdline = [string](Get-CimInstance Win32_Process -Filter "ProcessId=$top" -ErrorAction SilentlyContinue | Select-Object -First 1).CommandLine
                }
                $m = [regex]::Match([string]$cmdline, '--user-data-dir=(?:"([^"]+)"|(\S+))')
                $udf = if ($m.Groups[1].Success) { $m.Groups[1].Value } elseif ($m.Groups[2].Success) { $m.Groups[2].Value } else { '' }
                $g = if ($udf) {
                    if ($udf -match '[\\/]wallpaper-renderer[\\/]') { 'renderer' } else { 'widget' }
                } elseif (@($browserGroup.Values) -contains 'widget') { 'renderer' } else { 'widget' }
                $browserGroup[$bkey] = $g
                Write-Log ("BROWSER pid={0} group={1} --user-data-dir={2}" -f $top, $g, $(if ($udf) { $udf } else { '（讀不到，已結束）' }))
                if ($udf -and -not $udfSeen.ContainsKey($udf)) { $udfSeen[$udf] = $g }
            }
            $group = $browserGroup[$bkey]
        }
        $mb = $byId[$id].Proc.PrivateMemorySize64 / 1MB
        $kidN++; $wv += $mb
        if ($group -eq 'renderer') { $rN++; $rMb += $mb } else { $wN++; $wMb += $mb }
    }
    if (($t - $udfLastMs) -ge ($UdfSizeEverySec * 1000)) {
        $udfLastMs = $t
        $udfStat = Get-DirSize -Path $rendererUdfDir
    }
    $csv.WriteLine(('{0},{1},{2:F1},{3},{4:F1},{5:F1},{6},{7:F1},{8},{9:F1},{10:F2},{11}' -f $t, $proc.Id, $hostMb, $kidN, $wv, ($hostMb + $wv), $wN, $wMb, $rN, $rMb, $udfStat.Mb, $udfStat.Files))
    $csv.Flush()
    if ((Get-Date) -gt $deadline) {
        Write-Log "TIMEOUT 超過 $timeoutSec 秒，停止自己啟動的行程樹"
        $stopped = Stop-ProcessTree -Process $proc
        Write-Log ("STOPPED {0}" -f ($stopped -join ','))
        $overall = 1
        break
    }
    $sleep = [Math]::Max(0, $SampleMs - ((Get-EpochMs) - $loopStart))
    if ($sleep -gt 0) { Start-Sleep -Milliseconds $sleep }
}
$csv.Dispose()
$udfJob | Wait-Job -Timeout 10 | Out-Null
$udfJob | Remove-Job -Force
$cmdlines = Get-BrowserCmdlines
foreach ($k in $cmdlines.Keys) {
    $m = [regex]::Match([string]$cmdlines[$k], '--user-data-dir=(?:"([^"]+)"|(\S+))')
    $udf = if ($m.Groups[1].Success) { $m.Groups[1].Value } elseif ($m.Groups[2].Success) { $m.Groups[2].Value } else { '' }
    if (-not $udf) { continue }
    $g = if ($udf -match '[\\/]wallpaper-renderer[\\/]') { 'renderer' } else { 'widget' }
    if (-not $udfSeen.ContainsKey($udf)) { $udfSeen[$udf] = $g; Write-Log "UDF group=$g browser=$k --user-data-dir=$udf" }
}
Write-Log ("BROWSERS 宿主直接子行程（各 WebView2 環境的 browser）共 {0} 個：widget={1} renderer={2}" -f $cmdlines.Count,
    @($cmdlines.Values | Where-Object { $_ -notmatch '[\\/]wallpaper-renderer[\\/]' }).Count,
    @($cmdlines.Values | Where-Object { $_ -match '[\\/]wallpaper-renderer[\\/]' }).Count)
$proc.WaitForExit()
$code = $proc.ExitCode
Write-Log "EXIT code=$code"
if ($code -ne 0) { $overall = 1 }
$left = @(Stop-ProcessTree -Process $proc)
Write-Log ("LEFTOVER stopped=[{0}]" -f ($left -join ','))
Start-Sleep -Seconds 5
$relaunched = @(Get-Process -Name fc-host -ErrorAction SilentlyContinue)
Write-Log ("RELAUNCH-CHECK fc-host={0}{1}" -f $relaunched.Count, $(if ($relaunched.Count) { ' PID ' + (($relaunched | ForEach-Object Id) -join ',') + '（未結束，需人工判讀）' } else { '' }))
if ($relaunched.Count -gt 0) { $overall = 1 }

# ── 隔離核對 ───────────────────────────────────────────────────────────────────────────
$groups = @($udfSeen.Values | Sort-Object -Unique)
if ($udfSeen.Count -eq 0) { Write-Log 'ISOLATION-FAIL 沒讀到任何 msedgewebview2 的 --user-data-dir'; $overall = 1 }
foreach ($u in $udfSeen.Keys) {
    if (-not $u.StartsWith($isoRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
        Write-Log "ISOLATION-FAIL --user-data-dir 不在隔離根目錄內：$u"; $overall = 1
    }
}
$widgetUdfs = @($udfSeen.Keys | Where-Object { $udfSeen[$_] -eq 'widget' })
$rendererUdfs = @($udfSeen.Keys | Where-Object { $udfSeen[$_] -eq 'renderer' })
Write-Log ("UDF-GROUPS widget=[{0}] renderer=[{1}]" -f ($widgetUdfs -join ';'), ($rendererUdfs -join ';'))
if ($NoRender) {
    if ($rendererUdfs.Count -eq 0) { Write-Log 'CONTROL ok 對照組沒有建立任何渲染視窗（不核對獨立環境）' }
    else { Write-Log 'CONTROL-FAIL 對照組卻出現渲染視窗的 browser 行程'; $overall = 1 }
}
elseif ($rendererUdfs.Count -eq 0) { Write-Log 'INDEPENDENT-FAIL 沒看到渲染視窗自己的 --user-data-dir（可能併進小工具那組）'; $overall = 1 }
elseif (@($rendererUdfs | Where-Object { $widgetUdfs -contains $_ }).Count -gt 0) { Write-Log 'INDEPENDENT-FAIL 兩組共用同一個 --user-data-dir'; $overall = 1 }
else { Write-Log 'INDEPENDENT ok 渲染視窗與小工具使用不同的 WebView2 使用者資料夾' }

$realAfter = Get-RealState
Write-Log ("REAL-AFTER {0}" -f (($realAfter.GetEnumerator() | ForEach-Object { "$($_.Key)=$($_.Value)" }) -join ' '))
foreach ($k in $realBefore.Keys) {
    if ($realBefore[$k] -ne $realAfter[$k]) { Write-Log "REAL-CHANGED $k（可能是其他程式寫入，需人工判讀）" }
}
$udfFinal = Get-DirSize -Path $rendererUdfDir
Write-Log ("RENDERER-UDF-FINAL wallpaper-renderer {0:N2} MB／{1} 檔（宿主結束後）" -f $udfFinal.Mb, $udfFinal.Files)
$outPngs = @(Get-ChildItem -LiteralPath (Join-Path $isoLocal 'tw.fintools.fc-host\wallpaper') -Filter '*.png' -File -ErrorAction SilentlyContinue)
Write-Log ("OUTPUT-FILES {0}" -f (($outPngs | ForEach-Object { '{0}({1} bytes)' -f $_.Name, $_.Length }) -join ' '))
if ($outPngs.Count -gt 0) {
    $latest = $outPngs | Sort-Object LastWriteTimeUtc -Descending | Select-Object -First 1
    $dest = "$prefix-last.png"
    Copy-Item -LiteralPath $latest.FullName -Destination $dest -Force
    Write-Log "COPIED $($latest.Name) -> $dest"
}
$hostLogs = @(Get-ChildItem -LiteralPath (Join-Path $isoLocal 'tw.fintools.fc-host\logs') -File -ErrorAction SilentlyContinue)
foreach ($l in $hostLogs) {
    $dest = "$prefix-host-$($l.Name)"
    Copy-Item -LiteralPath $l.FullName -Destination $dest -Force
    Protect-EvidenceFile -Path $dest | Out-Null
    Write-Log "COPIED host log $($l.Name) -> $dest"
}

# ── 摘要 ───────────────────────────────────────────────────────────────────────────────
if (Test-Path -LiteralPath $renderLog) {
    Protect-EvidenceFile -Path $renderLog | Out-Null
    $events = @(Read-RenderEvents -Path $renderLog)
    $rows = @(Import-Csv -LiteralPath $csvPath)
    $summary = @(Get-Measure45Summary -Rows $rows -Renders $events -Markers (Read-HostMarkers -Path $renderLog) `
            -IntervalSec $IntervalSec -WarmupSec $WarmupSec -SlopeFromSec $SlopeFromSec)
    $hostSummary = @(Get-Content -LiteralPath $renderLog -Encoding utf8 | Where-Object { $_ -match ' (SUMMARY|RENDERER-UDF|VERIFY .* FAIL|RENDER-FAIL|RENDER-FORCED-TIMEOUT)' })
    $all = @($summary) + @($hostSummary | ForEach-Object { "HOST $_" })
    Set-Content -LiteralPath $summaryPath -Value ($all | ForEach-Object { ConvertTo-EvidenceText $_ }) -Encoding utf8
    $all | ForEach-Object { Write-Host $_ }
    if (@($events | Where-Object { -not $_.Ok }).Count -gt 0) { $overall = 1 }
    # 強制逾時那一次：渲染那組的 browser 行程必須在逾時後結束（取樣看到 0 個行程）。
    foreach ($f in @($events | Where-Object Forced)) {
        $z = $rows | Where-Object { $f.EndMs -and [int64]$_.ts_ms -ge $f.EndMs -and [int]$_.renderer_count -eq 0 } | Select-Object -First 1
        if (-not $z) { Write-Log "FORCED-TIMEOUT-FAIL 第 $($f.N) 次逾時後渲染那組行程沒有結束"; $overall = 1 }
        else { Write-Log ("FORCED-TIMEOUT ok 第 {0} 次：視窗已關閉＝{1}，渲染那組行程在 {2} ms 內結束" -f $f.N, $f.WindowClosed, ([int64]$z.ts_ms - $f.EndMs)) }
    }
    if ($ForceTimeoutAt -gt 0 -and @($events | Where-Object Forced).Count -eq 0) { Write-Log 'FORCED-TIMEOUT-FAIL 沒有看到強制逾時那一次'; $overall = 1 }
} else {
    Write-Log 'NO-RENDER-LOG 宿主沒有寫出 self-test 記錄'
    $overall = 1
}

try {
    Remove-Item -LiteralPath $isoRoot -Recurse -Force -ErrorAction Stop
    Write-Log "CLEANUP removed $isoRoot"
} catch {
    Write-Log "CLEANUP 未能刪除 $isoRoot：$($_.Exception.Message)"
}
Write-Log ("VERDICT {0}" -f $(if ($overall -eq 0) { 'PASS' } else { 'FAIL' }))
exit $overall
