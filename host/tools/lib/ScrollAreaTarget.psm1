<#
.SYNOPSIS
    滾輪驗收的選點與前提判讀（fix F10）：由頁面 `.scroll-area` 的實際位置決定滾輪點，不靠視窗矩形加固定偏移。

.DESCRIPTION
    失效鏈：verify-4.7-wheel／wheelrouting 原本以 `GetWindowRect` 的 Top 加 150 當滾輪點。fix F9 改成
    Per-Monitor-V2 之後 GetWindowRect 回傳實體像素，同一個 150 在 150% 縮放下只剩 100 CSS px，偏移的單位
    悄悄換掉卻沒有任何前提擋住。本模組只放純函式（不呼叫 Win32、不連 CDP）：
      - 頁面端運算式（呼叫端經 host-cdp-eval.mjs 執行）：探查 `.scroll-area` 的
        getBoundingClientRect、scrollHeight／clientHeight 與 devicePixelRatio；在指定 CSS 點做
        elementFromPoint 命中測試。
      - 座標換算：實體螢幕座標＝客戶區原點（ClientToScreen，呼叫端須為 PMv2）＋CSS 座標×dpr。
        devicePixelRatio 已含頁面縮放，所以這個換算在頁面縮放下也成立。
      - 選點與替代點搜尋只用清單與 viewport（innerWidth／innerHeight）的交集，不落在視窗外。
      - 兩項前提：清單確實可捲動且尺寸有效、與 viewport 有交集；最終滾輪點命中 `.scroll-area` 或其子孫。
    運算式刻意不含雙引號：它經命令列傳給 node。
#>

$script:Inv = [System.Globalization.CultureInfo]::InvariantCulture

function Get-ScrollAreaProbeExpression {
    "(() => { const el = document.querySelector('.scroll-area'); if (!el) return null; const r = el.getBoundingClientRect(); " +
    "return { dpr: window.devicePixelRatio, left: r.left, top: r.top, width: r.width, height: r.height, " +
    "scrollHeight: el.scrollHeight, clientHeight: el.clientHeight, vw: window.innerWidth, vh: window.innerHeight }; })()"
}

function Get-ScrollAreaHitExpression {
    param([Parameter(Mandatory)][double]$CssX, [Parameter(Mandatory)][double]$CssY)
    $x = $CssX.ToString('R', $script:Inv)
    $y = $CssY.ToString('R', $script:Inv)
    "(() => { const el = document.querySelector('.scroll-area'); const h = document.elementFromPoint($x, $y); " +
    "return { inside: !!(el && h && el.contains(h)), hit: h ? h.tagName + '.' + String(h.className) : null }; })()"
}

function ConvertTo-ScreenPoint {
    <# CSS 座標（相對 viewport）→ 實體螢幕座標；往下取整，確保不跨出元素右／下緣。 #>
    param([Parameter(Mandatory)][int]$OriginX, [Parameter(Mandatory)][int]$OriginY, [Parameter(Mandatory)][double]$Dpr,
        [Parameter(Mandatory)][double]$CssX, [Parameter(Mandatory)][double]$CssY)
    @{ X = [int][Math]::Floor($OriginX + $CssX * $Dpr); Y = [int][Math]::Floor($OriginY + $CssY * $Dpr) }
}

function ConvertTo-CssPoint {
    <# 實體螢幕座標 → CSS 座標（相對 viewport）；供替代點也能做 elementFromPoint 命中測試。 #>
    param([Parameter(Mandatory)][int]$OriginX, [Parameter(Mandatory)][int]$OriginY, [Parameter(Mandatory)][double]$Dpr,
        [Parameter(Mandatory)][int]$X, [Parameter(Mandatory)][int]$Y)
    @{ X = ($X - $OriginX) / $Dpr; Y = ($Y - $OriginY) / $Dpr }
}

function Get-ProbeNumber {
    <# 讀探查結果的數值欄位；欄位缺失或不是數字＝NaN（後續比較一律不成立＝前提 FAIL）。 #>
    param([AllowNull()]$Probe, [Parameter(Mandatory)][string]$Name)
    if ($null -eq $Probe) { return [double]::NaN }
    $p = $Probe.PSObject.Properties[$Name]
    if ($null -eq $p -or $null -eq $p.Value) { return [double]::NaN }
    $d = 0.0
    if ([double]::TryParse([string]$p.Value, [System.Globalization.NumberStyles]::Float, $script:Inv, [ref]$d)) { return $d }
    [double]::NaN
}

function Get-ScrollAreaVisibleRect {
    <#
    fix F10b：`.scroll-area` 的 getBoundingClientRect 與 viewport (0,0,innerWidth,innerHeight) 的交集（CSS px）。
    清單超出視窗時，選點與替代點搜尋都只用這個交集，不會落在視窗外。交集為空（或欄位無效）→ $null。
    #>
    param([AllowNull()]$Probe)
    $l = Get-ProbeNumber $Probe 'left'
    $t = Get-ProbeNumber $Probe 'top'
    $w = Get-ProbeNumber $Probe 'width'
    $h = Get-ProbeNumber $Probe 'height'
    $vw = Get-ProbeNumber $Probe 'vw'
    $vh = Get-ProbeNumber $Probe 'vh'
    if (-not ($w -gt 0 -and $h -gt 0 -and $vw -gt 0 -and $vh -gt 0 -and $l -eq $l -and $t -eq $t)) { return $null }
    $left = [Math]::Max($l, 0.0)
    $top = [Math]::Max($t, 0.0)
    $right = [Math]::Min($l + $w, $vw)
    $bottom = [Math]::Min($t + $h, $vh)
    if (-not ($right -gt $left -and $bottom -gt $top)) { return $null }
    @{ Left = $left; Top = $top; Right = $right; Bottom = $bottom }
}

function Get-ScrollAreaWheelTarget {
    <#
    由探查結果選滾輪點：`.scroll-area` 與 viewport 交集的中心（CSS），換成實體螢幕座標；另回傳交集的實體
    矩形，供遮擋時的替代點搜尋限在清單可見部分內。探查結果缺失、尺寸無效或交集為空 → 丟「PRECONDITION: 」
    （不送滾輪）。正常流程這些情況已先由 Test-ScrollAreaScrollable 判 FAIL，這裡是第二道防線。
    #>
    param([AllowNull()]$Probe, [Parameter(Mandatory)][int]$OriginX, [Parameter(Mandatory)][int]$OriginY)
    if ($null -eq $Probe) { throw 'PRECONDITION: 找不到 .scroll-area（CDP 探查結果為 null）' }
    $dpr = Get-ProbeNumber $Probe 'dpr'
    $w = Get-ProbeNumber $Probe 'width'
    $h = Get-ProbeNumber $Probe 'height'
    if (-not ($dpr -gt 0)) { throw "PRECONDITION: devicePixelRatio 無效（$dpr）" }
    if (-not ($w -gt 0 -and $h -gt 0)) { throw "PRECONDITION: .scroll-area 尺寸無效（${w}x$h CSS px）" }
    $v = Get-ScrollAreaVisibleRect -Probe $Probe
    if ($null -eq $v) { throw "PRECONDITION: .scroll-area 與 viewport 沒有交集（innerWidth=$(Get-ProbeNumber $Probe 'vw') innerHeight=$(Get-ProbeNumber $Probe 'vh')）" }
    $cssX = ($v.Left + $v.Right) / 2
    $cssY = ($v.Top + $v.Bottom) / 2
    $pt = ConvertTo-ScreenPoint -OriginX $OriginX -OriginY $OriginY -Dpr $dpr -CssX $cssX -CssY $cssY
    $tl = ConvertTo-ScreenPoint -OriginX $OriginX -OriginY $OriginY -Dpr $dpr -CssX $v.Left -CssY $v.Top
    @{
        X = $pt.X; Y = $pt.Y; CssX = $cssX; CssY = $cssY; Dpr = $dpr
        Left = $tl.X; Top = $tl.Y
        Width = [int][Math]::Floor(($v.Right - $v.Left) * $dpr); Height = [int][Math]::Floor(($v.Bottom - $v.Top) * $dpr)
    }
}

function Test-ScrollAreaScrollable {
    <#
    前提：清單確實可捲動（scrollHeight > clientHeight），否則「scrollTop 不變」沒有鑑別力。fix F10b：尺寸有效
    （clientHeight、width、height、devicePixelRatio 皆 > 0）與「清單和 viewport 有交集」也併在這一項——不成立時
    在前提項就記 FAIL，不留到選點階段才丟例外。
    #>
    param([AllowNull()]$Probe)
    if ($null -eq $Probe) { return @{ Ok = $false; Reason = '找不到 .scroll-area（CDP 探查結果為 null）' } }
    $sh = Get-ProbeNumber $Probe 'scrollHeight'
    $ch = Get-ProbeNumber $Probe 'clientHeight'
    $w = Get-ProbeNumber $Probe 'width'
    $h = Get-ProbeNumber $Probe 'height'
    $dpr = Get-ProbeNumber $Probe 'dpr'
    if (-not ($ch -gt 0 -and $w -gt 0 -and $h -gt 0 -and $dpr -gt 0)) {
        return @{ Ok = $false; Reason = "清單尺寸無效：clientHeight=$ch width=$w height=$h dpr=$dpr（皆須 > 0）" }
    }
    if ($null -eq (Get-ScrollAreaVisibleRect -Probe $Probe)) {
        return @{ Ok = $false; Reason = "清單與 viewport 沒有交集（innerWidth=$(Get-ProbeNumber $Probe 'vw') innerHeight=$(Get-ProbeNumber $Probe 'vh')）" }
    }
    if ($sh -gt $ch) { return @{ Ok = $true; Reason = "scrollHeight=$sh > clientHeight=$ch" } }
    @{ Ok = $false; Reason = "清單不可捲動：scrollHeight=$sh ≤ clientHeight=$ch（資料未載入或內容不夠長）" }
}

function Test-ScrollAreaHit {
    <# 前提：最終滾輪點的 elementFromPoint 命中 `.scroll-area` 或其子孫。 #>
    param([AllowNull()]$Hit)
    if ($null -eq $Hit) { return @{ Ok = $false; Reason = 'elementFromPoint 沒有結果（CDP 失敗或點在 viewport 外）' } }
    if ([bool]$Hit.inside) { return @{ Ok = $true; Reason = "命中 $($Hit.hit)" } }
    @{ Ok = $false; Reason = "滾輪點落在 .scroll-area 之外，命中 $($Hit.hit)" }
}

Export-ModuleMember -Function Get-ScrollAreaProbeExpression, Get-ScrollAreaHitExpression, ConvertTo-ScreenPoint,
ConvertTo-CssPoint, Get-ScrollAreaVisibleRect, Get-ScrollAreaWheelTarget, Test-ScrollAreaScrollable, Test-ScrollAreaHit
