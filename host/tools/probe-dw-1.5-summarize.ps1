<#
.SYNOPSIS
    dynamic-wallpaper task 1.5 彙整：讀 probe-dw-1.5.ps1 產生的 jsonl，對相鄰兩筆取樣做差異比對，輸出 markdown 表格。

.DESCRIPTION
    相鄰兩筆（動作前→動作後）各產生兩張表，供結論寫回 design.md D4：
    1. IDesktopWallpaper 表（以 GetMonitorDevicePathAt 的**裝置路徑字串**為鍵，不分大小寫）：
       該路徑的索引、GetMonitorRECT（per-monitor v2 感知下讀到的值）、HRESULT 在前後的變化，以及兩種執行緒
       DPI 感知下 RECT 是否一致。判定：不變／新增／消失／索引改變／RECT 改變／HRESULT 改變。
       （路徑字串本身變了，會顯示為「消失」加「新增」各一列。）
    2. 穩定識別表（以 QueryDisplayConfig 的 monitorDevicePath＝宿主模擬得到的 id 為鍵）：GDI 名稱 szDevice、
       邊界、縮放、對到的 IDesktopWallpaper 索引（依路徑字串）與（依 RECT）在前後的變化，以及
       「對應有沒有跟著變」：id 不變而 GDI 名稱／索引／邊界改變＝對應靠得住的證據（id 沒跟著動）；
       id 本身消失或新增＝顯示器接上或拔掉；對應不到（查不到穩定識別或 DW 無此路徑）另列。
    另列 per 筆的「依路徑」與「依 RECT」兩種對應是否一致。

    只讀 jsonl、不碰系統任何設定。輸出一律先經 ConvertTo-EvidenceText。

.PARAMETER Path
    probe-dw-1.5.ps1 的 -Out 檔。

.PARAMETER OutFile
    選填：同時把 markdown 寫到這個檔（UTF-8 無 BOM）；不給就只輸出到主控台（輸出串流）。

.EXAMPLE
    pwsh -NoProfile -File host/tools/probe-dw-1.5-summarize.ps1 -Path $env:TEMP\fc-dw15.jsonl
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Path,
    [string]$OutFile
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force

if (-not (Test-Path -LiteralPath $Path)) { throw "找不到 $Path" }

$records = @(
    Get-Content -LiteralPath $Path -Encoding utf8 |
        Where-Object { $_.Trim() } |
        ForEach-Object { $_ | ConvertFrom-Json }
)
if ($records.Count -eq 0) { throw "$Path 沒有任何紀錄" }

# ── 小工具 ─────────────────────────────────────────────────────────────────────
# 注意：不可寫 `$v -eq ''`——數值 0 與 '' 比較會轉型成相等，索引 0 會被顯示成 —。
function Format-Cell($v) {
    if ([string]::IsNullOrEmpty([string]$v)) { return '—' }
    ([string]$v).Replace('|', '\|')
}

function Format-Code($v) {
    if ([string]::IsNullOrEmpty([string]$v)) { return '—' }
    '`' + ([string]$v).Replace('|', '\|') + '`'
}

# 前後值並列：相同顯示「值（不變）」，不同顯示「前 → 後」。$null 以 — 表示。
function Format-Change($a, $b, [switch]$Code) {
    $fa = if ($Code) { Format-Code $a } else { Format-Cell $a }
    $fb = if ($Code) { Format-Code $b } else { Format-Cell $b }
    if ([string]$a -ceq [string]$b) { return "$fa（不變）" }
    "$fa → $fb"
}

function Format-Time($t) {
    if ($t -is [datetime]) { return $t.ToString('HH:mm:ss') }
    $s = [string]$t
    if ($s.Length -ge 19) { return $s.Substring(11, 8) }
    $s
}

function Get-RectText($r) {
    if ($null -eq $r) { return $null }
    $t = '({0},{1})-({2},{3})' -f $r.left, $r.top, $r.right, $r.bottom
    if ($r.hr -ne '0x00000000') { $t += " [hr=$($r.hr)]" }
    $t
}

function Get-DwMap($rec) {
    $map = [ordered]@{}
    foreach ($e in @($rec.desktopWallpaper.entries)) {
        if ($null -eq $e) { continue }
        $key = if ($e.path) { ([string]$e.path).ToLowerInvariant() } else { "(null@$($e.index))" }
        $map[$key] = $e
    }
    $map
}

function Get-MonitorMap($rec) {
    # 以穩定 id 為鍵；查不到穩定識別者用 szDevice 當鍵，並標記。
    $map = [ordered]@{}
    foreach ($m in @($rec.monitors)) {
        if ($null -eq $m) { continue }
        $key = if ($m.stableId) { ([string]$m.stableId).ToLowerInvariant() } else { "(unresolved)$($m.szDevice)" }
        $corr = @($rec.correlation) | Where-Object { $_.szDevice -ceq $m.szDevice } | Select-Object -First 1
        $map[$key] = [pscustomobject]@{ Mon = $m; Corr = $corr; Friendly = $null }
    }
    foreach ($p in @($rec.queryDisplayConfig.paths)) {
        if ($null -eq $p -or -not $p.monitorDevicePath) { continue }
        $key = ([string]$p.monitorDevicePath).ToLowerInvariant()
        if ($map.Contains($key)) { $map[$key].Friendly = $p.friendlyName }
    }
    $map
}

$out = New-Object System.Collections.Generic.List[string]
function Add-Line([string]$s = '') { $out.Add($s) }

# ── 1. 取樣清單 ────────────────────────────────────────────────────────────────
Add-Line '# 螢幕識別探針（task 1.5）彙整'
Add-Line
Add-Line "共 $($records.Count) 筆取樣。"
Add-Line
Add-Line '## 取樣清單'
Add-Line
Add-Line '| # | label | 時間 | DW 台數（count hr） | DW 路徑字串＝穩定 id？ | 依路徑／依 RECT 對應一致 | EnumDisplayMonitors | QDC 路徑 |'
Add-Line '| --- | --- | --- | --- | --- | --- | --- | --- |'
for ($i = 0; $i -lt $records.Count; $i++) {
    $r = $records[$i]
    $dwPaths = @(@($r.desktopWallpaper.entries) | Where-Object { $_ -and $_.path } | ForEach-Object { ([string]$_.path).ToLowerInvariant() })
    $stable = @(@($r.monitors) | Where-Object { $_ -and $_.stableId } | ForEach-Object { ([string]$_.stableId).ToLowerInvariant() })
    $allStable = ($stable.Count -eq @($r.monitors).Count)
    $samePath = if ($dwPaths.Count -eq 0) { '—' }
    elseif ((@($dwPaths | Where-Object { $_ -notin $stable }).Count -eq 0) -and (@($stable | Where-Object { $_ -notin $dwPaths }).Count -eq 0)) { '是（兩邊集合相同）' }
    else { '否（集合不同）' }
    $agree = @(@($r.correlation) | Where-Object { $_ -and $_.agree }).Count
    $agreeText = "$agree／$(@($r.correlation).Count)"
    $resolved = if ($allStable) { "$(@($r.monitors).Count) 台（皆有穩定識別）" } else { "$(@($r.monitors).Count) 台（$($stable.Count) 台有穩定識別）" }
    Add-Line ("| {0} | {1} | {2} | {3}（{4}） | {5} | {6} | {7} | {8} |" -f `
            ($i + 1), (Format-Cell $r.label), (Format-Time $r.timestamp), $r.desktopWallpaper.count, $r.desktopWallpaper.countHr, $samePath, $agreeText, $resolved, @($r.queryDisplayConfig.paths).Count)
}
Add-Line

$dwFailed = @($records | Where-Object { -not $_.desktopWallpaper.ok })
if ($dwFailed.Count -gt 0) {
    Add-Line '> 注意：下列取樣的 IDesktopWallpaper 讀取失敗，其 DW 表無資料：'
    foreach ($r in $dwFailed) { Add-Line "> - $(Format-Cell $r.label)：$(Format-Cell $r.desktopWallpaper.error)" }
    Add-Line
}

if ($records.Count -lt 2) {
    Add-Line '只有一筆取樣，無法做前後差異比對；至少需要兩筆（動作前、動作後）。'
}

# ── 2. 相鄰兩筆差異 ────────────────────────────────────────────────────────────
for ($i = 0; $i -lt $records.Count - 1; $i++) {
    $a = $records[$i]; $b = $records[$i + 1]
    Add-Line "## $($i + 1) → $($i + 2)：$(Format-Cell $a.label) → $(Format-Cell $b.label)"
    Add-Line

    # 表 1：IDesktopWallpaper，以路徑字串為鍵
    $ma = Get-DwMap $a; $mb = Get-DwMap $b
    $keys = @($ma.Keys) + @($mb.Keys | Where-Object { -not $ma.Contains($_) })
    Add-Line '### IDesktopWallpaper（鍵＝GetMonitorDevicePathAt 路徑字串）'
    Add-Line
    Add-Line '| 路徑字串 | 索引 | GetMonitorRECT（per-monitor v2） | RECT 的 HRESULT | 兩種 DPI 感知 RECT 一致 | 判定 |'
    Add-Line '| --- | --- | --- | --- | --- | --- |'
    $dwChanged = 0
    foreach ($k in $keys) {
        $ea = if ($ma.Contains($k)) { $ma[$k] } else { $null }
        $eb = if ($mb.Contains($k)) { $mb[$k] } else { $null }
        $path = if ($ea) { $ea.path } else { $eb.path }
        $verdict = @()
        if (-not $ea) { $verdict += '新增' }
        elseif (-not $eb) { $verdict += '消失' }
        else {
            if ($ea.index -ne $eb.index) { $verdict += '索引改變' }
            if ((Get-RectText $ea.rectPmv2) -cne (Get-RectText $eb.rectPmv2)) { $verdict += 'RECT 改變' }
            if ($ea.rectPmv2.hr -ne $eb.rectPmv2.hr) { $verdict += 'HRESULT 改變' }
        }
        if ($verdict.Count -eq 0) { $verdict = @('不變') } else { $dwChanged++ }
        $ia = if ($ea) { $ea.index } else { $null }; $ib = if ($eb) { $eb.index } else { $null }
        $ra = if ($ea) { Get-RectText $ea.rectPmv2 } else { $null }; $rb = if ($eb) { Get-RectText $eb.rectPmv2 } else { $null }
        $ha = if ($ea) { $ea.rectPmv2.hr } else { $null }; $hb = if ($eb) { $eb.rectPmv2.hr } else { $null }
        $dpiSame = @(@($ea, $eb) | Where-Object { $_ } | ForEach-Object {
                if ((Get-RectText $_.rectDefaultDpi) -ceq (Get-RectText $_.rectPmv2)) { '是' } else { '否' }
            }) | Select-Object -Unique
        $dpiText = if ($dpiSame.Count -eq 0) { '—' } else { $dpiSame -join '／' }
        Add-Line ('| {0} | {1} | {2} | {3} | {4} | {5} |' -f (Format-Code $path), (Format-Change $ia $ib), (Format-Change $ra $rb -Code), (Format-Change $ha $hb), $dpiText, ($verdict -join '＋'))
    }
    if ($keys.Count -eq 0) { Add-Line '| （兩筆皆無 DW 項目） | — | — | — | — | — |' }
    Add-Line

    # 表 2：穩定識別
    $na = Get-MonitorMap $a; $nb = Get-MonitorMap $b
    $nkeys = @($na.Keys) + @($nb.Keys | Where-Object { -not $na.Contains($_) })
    Add-Line '### 穩定識別對應（鍵＝QueryDisplayConfig monitorDevicePath＝宿主模擬得到的 id）'
    Add-Line
    Add-Line '| 穩定 id | 名稱 | GDI szDevice | 邊界（EnumDisplayMonitors） | 縮放 | DW 索引（依路徑） | DW 索引（依 RECT） | 判定 |'
    Add-Line '| --- | --- | --- | --- | --- | --- | --- | --- |'
    $idDrift = 0
    foreach ($k in $nkeys) {
        $xa = if ($na.Contains($k)) { $na[$k] } else { $null }
        $xb = if ($nb.Contains($k)) { $nb[$k] } else { $null }
        $unresolved = $k.StartsWith('(unresolved)')
        $idText = if ($unresolved) { '（無穩定識別）' } elseif ($xa) { $xa.Mon.stableId } else { $xb.Mon.stableId }
        $friendly = if ($xa -and $xa.Friendly) { $xa.Friendly } elseif ($xb -and $xb.Friendly) { $xb.Friendly } else { $null }
        $verdict = @()
        if ($unresolved) { $verdict += '無穩定識別（宿主視為未解析）' }
        if (-not $xa) { $verdict += '新增（接上）' }
        elseif (-not $xb) { $verdict += '消失（拔掉／停用）' }
        else {
            $chg = @()
            if ($xa.Mon.szDevice -cne $xb.Mon.szDevice) { $chg += 'GDI 名稱' }
            if ($xa.Mon.bounds -cne $xb.Mon.bounds) { $chg += '邊界' }
            if ([string]$xa.Mon.scale -ne [string]$xb.Mon.scale) { $chg += '縮放' }
            if ($xa.Corr.dwIndexByPath -ne $xb.Corr.dwIndexByPath) { $chg += 'DW 索引（依路徑）' }
            if ($xa.Corr.dwIndexByRect -ne $xb.Corr.dwIndexByRect) { $chg += 'DW 索引（依 RECT）' }
            if (-not $xa.Corr.agree -or -not $xb.Corr.agree) { $chg += '依路徑與依 RECT 不一致' }
            if ($chg.Count -gt 0 -and -not $unresolved) { $verdict += "id 不變、$($chg -join '／') 改變"; $idDrift++ }
            elseif (-not $unresolved) { $verdict += '全部不變' }
        }
        $sa = if ($xa) { $xa.Mon.szDevice } else { $null }; $sb = if ($xb) { $xb.Mon.szDevice } else { $null }
        $ba = if ($xa) { $xa.Mon.bounds } else { $null }; $bb = if ($xb) { $xb.Mon.bounds } else { $null }
        $ca = if ($xa) { $xa.Mon.scale } else { $null }; $cb = if ($xb) { $xb.Mon.scale } else { $null }
        $pa = if ($xa) { $xa.Corr.dwIndexByPath } else { $null }; $pb = if ($xb) { $xb.Corr.dwIndexByPath } else { $null }
        $qa = if ($xa) { $xa.Corr.dwIndexByRect } else { $null }; $qb = if ($xb) { $xb.Corr.dwIndexByRect } else { $null }
        Add-Line ('| {0} | {1} | {2} | {3} | {4} | {5} | {6} | {7} |' -f (Format-Code $idText), (Format-Cell $friendly), (Format-Change $sa $sb -Code), (Format-Change $ba $bb -Code), (Format-Change $ca $cb), (Format-Change $pa $pb), (Format-Change $qa $qb), ($verdict -join '＋'))
    }
    if ($nkeys.Count -eq 0) { Add-Line '| （兩筆皆無顯示器） | — | — | — | — | — | — | — |' }
    Add-Line
    Add-Line ("要點：DW 項目有變化 {0} 筆（含新增／消失）；穩定 id 不變但其他欄位改變 {1} 台；顯示器數 {2} → {3}、DW 台數 {4} → {5}。" -f `
            $dwChanged, $idDrift, @($a.monitors).Count, @($b.monitors).Count, $a.desktopWallpaper.count, $b.desktopWallpaper.count)
    Add-Line
}

$text = ConvertTo-EvidenceText ($out -join "`n")
if ($OutFile) {
    $full = [IO.Path]::GetFullPath($OutFile)
    [IO.File]::WriteAllText($full, $text + "`n", (New-Object System.Text.UTF8Encoding($false)))
}
$text
