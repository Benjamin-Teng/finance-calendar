<#
.SYNOPSIS
    fix F10：verify-4.7-wheel.ps1 與 verify-4.7-wheelrouting.ps1 的滾輪點改由 `.scroll-area` 實際位置決定
    （lib/ScrollAreaTarget.psm1），並在送滾輪之前有「可捲動」「命中清單」兩項前提（純 PowerShell 斷言，不需 Pester）。

.DESCRIPTION
    失效鏈：兩支腳本原本以 `$rect.Top + 150` 當滾輪點。fix F9 讓腳本改成 Per-Monitor-V2 之後，GetWindowRect
    回傳實體像素，150 的語意從「150 邏輯 px」變成「150 實體 px」（150% 縮放下＝100 CSS px），座標換了位置卻
    沒有任何前提擋住；wheelrouting 的判定是「scrollTop 不變」，點落在清單外照樣 PASS，失去鑑別力。

    **不執行兩支腳本**（會啟動宿主、送滾輪）：
      1. 純函式：載入 lib/ScrollAreaTarget.psm1，驗 CSS↔實體座標換算（dpr＝1.5）、選點落在清單內、兩項前提判讀。
      2. 語法樹：兩支腳本不得再有 `$rect.Top + <常數>`／`$rect.Left + <常數>` 形式的滾輪點；必須呼叫
         Get-ScrollAreaWheelTarget；Test-ScrollAreaScrollable 與 Test-ScrollAreaHit 都在 Send-GuardedWheel
         之前，且各自與 Send-GuardedWheel 之間有 `throw 'PRECONDITION…'`；命中測試在 Resolve-WheelPoint 之後
         （驗的是最終點，含替代點）；替代點搜尋不再用整個視窗矩形。

.EXAMPLE
    pwsh -NoProfile -File host/tools/tests/Verify47WheelTarget.Tests.ps1
    結束碼 0＝全部通過、1＝有失敗。
#>
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$toolsDir = Split-Path $PSScriptRoot -Parent
$script:Pass = 0
$script:Fail = 0
function Check([string]$name, [bool]$cond, [string]$detail = '') {
    if ($cond) { $script:Pass++; Write-Host "PASS  $name" }
    else { $script:Fail++; Write-Host "FAIL  $name $detail" -ForegroundColor Red }
}

# ---------------------------------------------------------------- 1. 純函式
$modPath = Join-Path $toolsDir 'lib/ScrollAreaTarget.psm1'
$modOk = Test-Path $modPath
Check 'lib/ScrollAreaTarget.psm1 存在' $modOk
if ($modOk) {
    Import-Module $modPath -Force

    # 10/05 實機：macro 視窗客戶區原點 (1200,522) 實體 px、150% 縮放。
    $p = ConvertTo-ScreenPoint -OriginX 1200 -OriginY 522 -Dpr 1.5 -CssX 100 -CssY 200
    Check 'dpr=1.5：CSS (100,200) → 實體 (1350,822)' ($p.X -eq 1350 -and $p.Y -eq 822) "got=($($p.X),$($p.Y))"
    $p = ConvertTo-ScreenPoint -OriginX 0 -OriginY 0 -Dpr 1.0 -CssX 10.9 -CssY 20.2
    Check 'dpr=1：小數 CSS 座標往下取整（不跨出元素）' ($p.X -eq 10 -and $p.Y -eq 20) "got=($($p.X),$($p.Y))"
    $c = ConvertTo-CssPoint -OriginX 1200 -OriginY 522 -Dpr 1.5 -X 1350 -Y 822
    Check 'dpr=1.5：實體 (1350,822) → CSS (100,200)' ([Math]::Abs($c.X - 100) -lt 1e-9 -and [Math]::Abs($c.Y - 200) -lt 1e-9) "got=($($c.X),$($c.Y))"

    $probe = [PSCustomObject]@{ dpr = 1.5; left = 9; top = 63; width = 835; height = 760; scrollHeight = 1400; clientHeight = 760; vw = 853; vh = 830 }
    $t = Get-ScrollAreaWheelTarget -Probe $probe -OriginX 1200 -OriginY 522
    Check '選點：CSS 點在 .scroll-area 矩形內' ($t.CssX -gt 9 -and $t.CssX -lt 844 -and $t.CssY -gt 63 -and $t.CssY -lt 823) "css=($($t.CssX),$($t.CssY))"
    Check '選點：CSS 點是清單中心' ([Math]::Abs($t.CssX - 426.5) -lt 1e-9 -and [Math]::Abs($t.CssY - 443) -lt 1e-9) "css=($($t.CssX),$($t.CssY))"
    Check '選點：實體點＝原點＋CSS×dpr' ($t.X -eq 1839 -and $t.Y -eq 1186) "got=($($t.X),$($t.Y))"
    Check '選點：回傳清單的實體矩形（供替代點搜尋）' ($t.Left -eq 1213 -and $t.Top -eq 616 -and $t.Width -eq 1252 -and $t.Height -eq 1140) "rect=($($t.Left),$($t.Top),$($t.Width),$($t.Height))"
    Check '選點：舊算法 Top+150 實體 px 在 dpr=1.5 只有 100 CSS px，新點遠離清單上緣' ($t.CssY - $probe.top -gt 100)

    # fix F10b：清單超出 viewport 時，選點與替代點矩形只取「清單 ∩ viewport」，不落在視窗外。
    $tall = [PSCustomObject]@{ dpr = 1.5; left = -20; top = 63; width = 900; height = 2000; scrollHeight = 3000; clientHeight = 2000; vw = 853; vh = 830 }
    $t = Get-ScrollAreaWheelTarget -Probe $tall -OriginX 1200 -OriginY 522
    Check '交集：清單超出 viewport，中心點取交集矩形中心' ([Math]::Abs($t.CssX - 426.5) -lt 1e-9 -and [Math]::Abs($t.CssY - 446.5) -lt 1e-9) "css=($($t.CssX),$($t.CssY))"
    Check '交集：中心點在 viewport 內' ($t.CssX -ge 0 -and $t.CssX -lt 853 -and $t.CssY -ge 0 -and $t.CssY -lt 830) "css=($($t.CssX),$($t.CssY))"
    Check '交集：替代點搜尋矩形＝交集的實體矩形（不超出視窗）' ($t.Left -eq 1200 -and $t.Top -eq 616 -and $t.Width -eq 1279 -and $t.Height -eq 1150) "rect=($($t.Left),$($t.Top),$($t.Width),$($t.Height))"
    $v = Get-ScrollAreaVisibleRect -Probe $tall
    Check '交集：Get-ScrollAreaVisibleRect 回 (0,63)-(853,830)' ($null -ne $v -and $v.Left -eq 0 -and $v.Top -eq 63 -and $v.Right -eq 853 -and $v.Bottom -eq 830) "v=$(if ($v) { "($($v.Left),$($v.Top))-($($v.Right),$($v.Bottom))" } else { 'null' })"
    $off = [PSCustomObject]@{ dpr = 1.5; left = 9; top = 900; width = 835; height = 760; scrollHeight = 1400; clientHeight = 760; vw = 853; vh = 830 }
    Check '交集：清單完全在 viewport 外 → Get-ScrollAreaVisibleRect 回 null' ($null -eq (Get-ScrollAreaVisibleRect -Probe $off))
    $bad = $null
    try { [void](Get-ScrollAreaWheelTarget -Probe $off -OriginX 0 -OriginY 0) } catch { $bad = "$_" }
    Check '交集為空 → 選點丟 PRECONDITION:' ($null -ne $bad -and $bad -like 'PRECONDITION:*') "err=$bad"
    $s = Test-ScrollAreaScrollable -Probe $off
    Check '交集為空 → 可捲動前提不 Ok' (-not $s.Ok) $s.Reason
    $s = Test-ScrollAreaScrollable -Probe ([PSCustomObject]@{ dpr = 1.5; left = 9; top = 63; width = 835; height = 760; scrollHeight = 1400; clientHeight = 760 })
    Check '探查缺 innerWidth／innerHeight → 可捲動前提不 Ok' (-not $s.Ok) $s.Reason

    # fix F10b：尺寸無效併進可捲動前提（在前提項就 FAIL，不靠選點階段的 throw）。
    foreach ($case in @(
            @('clientHeight=0', [PSCustomObject]@{ dpr = 1.5; left = 9; top = 63; width = 835; height = 760; scrollHeight = 10; clientHeight = 0; vw = 853; vh = 830 }),
            @('width=0', [PSCustomObject]@{ dpr = 1.5; left = 9; top = 63; width = 0; height = 760; scrollHeight = 1400; clientHeight = 760; vw = 853; vh = 830 }),
            @('height=0', [PSCustomObject]@{ dpr = 1.5; left = 9; top = 63; width = 835; height = 0; scrollHeight = 1400; clientHeight = 760; vw = 853; vh = 830 }),
            @('dpr=0', [PSCustomObject]@{ dpr = 0; left = 9; top = 63; width = 835; height = 760; scrollHeight = 1400; clientHeight = 760; vw = 853; vh = 830 }))) {
        $s = Test-ScrollAreaScrollable -Probe $case[1]
        Check "可捲動前提：$($case[0]) → 不 Ok" (-not $s.Ok) $s.Reason
    }

    $bad = $null
    try { [void](Get-ScrollAreaWheelTarget -Probe ([PSCustomObject]@{ dpr = 0; left = 0; top = 0; width = 10; height = 10; vw = 100; vh = 100 }) -OriginX 0 -OriginY 0) } catch { $bad = "$_" }
    Check '選點：dpr 無效 → 丟 PRECONDITION:' ($null -ne $bad -and $bad -like 'PRECONDITION:*') "err=$bad"
    $bad = $null
    try { [void](Get-ScrollAreaWheelTarget -Probe $null -OriginX 0 -OriginY 0) } catch { $bad = "$_" }
    Check '選點：探查結果為 null（找不到 .scroll-area）→ 丟 PRECONDITION:' ($null -ne $bad -and $bad -like 'PRECONDITION:*') "err=$bad"

    $s = Test-ScrollAreaScrollable -Probe $probe
    Check '可捲動：scrollHeight 1400 > clientHeight 760 → Ok' ($s.Ok) $s.Reason
    $s = Test-ScrollAreaScrollable -Probe ([PSCustomObject]@{ dpr = 1.5; left = 9; top = 63; width = 835; height = 760; scrollHeight = 760; clientHeight = 760; vw = 853; vh = 830 })
    Check '可捲動：scrollHeight＝clientHeight → 不 Ok' (-not $s.Ok) $s.Reason
    $s = Test-ScrollAreaScrollable -Probe $null
    Check '可捲動：探查結果為 null → 不 Ok' (-not $s.Ok) $s.Reason

    $h = Test-ScrollAreaHit -Hit ([PSCustomObject]@{ inside = $true; hit = 'LI.ev macro high' })
    Check '命中：elementFromPoint 在 .scroll-area 內 → Ok' ($h.Ok) $h.Reason
    $h = Test-ScrollAreaHit -Hit ([PSCustomObject]@{ inside = $false; hit = 'HEADER.' })
    Check '命中：落在 header → 不 Ok，原因含命中元素' (-not $h.Ok -and $h.Reason -like '*HEADER*') $h.Reason
    $h = Test-ScrollAreaHit -Hit $null
    Check '命中：CDP 無結果 → 不 Ok' (-not $h.Ok) $h.Reason

    $expr = Get-ScrollAreaHitExpression -CssX 426.5 -CssY 443
    Check '命中運算式：座標以不變文化格式化（小數點）且呼叫 elementFromPoint' ($expr -like '*elementFromPoint(426.5, 443)*') $expr
    Check '命中運算式：不含雙引號（經 node 命令列傳遞）' (-not $expr.Contains('"'))
    Check '探查運算式：不含雙引號，讀 devicePixelRatio 與 getBoundingClientRect' (-not (Get-ScrollAreaProbeExpression).Contains('"') -and (Get-ScrollAreaProbeExpression) -like '*devicePixelRatio*' -and (Get-ScrollAreaProbeExpression) -like '*getBoundingClientRect*')
    Check '探查運算式：一併讀 innerWidth／innerHeight（viewport 交集用）' ((Get-ScrollAreaProbeExpression) -like '*innerWidth*' -and (Get-ScrollAreaProbeExpression) -like '*innerHeight*')
}

# ---------------------------------------------------------------- 2. 語法樹
$L = 'System.Management.Automation.Language'
function Find-Commands($ast, [string]$name) {
    @($ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq $name }, $true))
}

function Find-PreconditionGuard($ast, [string]$fn) {
    <#
    fix F10b：把 throw 綁到對應前提。找 `$var = <fn> ...` 的指派，再找位於該指派之後、唯一一處
    Send-GuardedWheel 之前的 IfStatementAst：某個子句的條件引用 `$var.Ok`，且該子句本體有
    throw 'PRECONDITION…'。只看「之間有 throw」會被別的前提的 throw 冒充。
    #>
    $cmd = @(Find-Commands $ast $fn)
    $send = @(Find-Commands $ast 'Send-GuardedWheel')
    if ($cmd.Count -lt 1 -or $send.Count -ne 1) { return @{ Ok = $false; Var = '?'; Detail = "fn=$($cmd.Count) send=$($send.Count)" } }
    $assign = $cmd[0].Parent
    while ($null -ne $assign -and $assign -isnot [System.Management.Automation.Language.AssignmentStatementAst]) { $assign = $assign.Parent }
    if ($null -eq $assign) { return @{ Ok = $false; Var = '?'; Detail = "$fn 的結果沒有指派給變數" } }
    $var = $assign.Left.Extent.Text
    $from = $assign.Extent.EndOffset
    $to = $send[0].Extent.StartOffset
    $pattern = '(^|[^\w])' + [regex]::Escape($var) + '\.Ok\b'
    $ifs = @($ast.FindAll({
                param($n)
                if ($n -isnot [System.Management.Automation.Language.IfStatementAst]) { return $false }
                if ($n.Extent.StartOffset -lt $from -or $n.Extent.StartOffset -gt $to) { return $false }
                foreach ($c in $n.Clauses) {
                    $throwsPre = @($c.Item2.FindAll({ param($t) $t -is [System.Management.Automation.Language.ThrowStatementAst] -and $t.Extent.Text -like '*PRECONDITION*' }, $true))
                    if ($c.Item1.Extent.Text -match $pattern -and $throwsPre.Count -ge 1) { return $true }
                }
                $false
            }, $true))
    if ($ifs.Count -lt 1) { return @{ Ok = $false; Var = $var; Detail = "找不到條件引用 $var.Ok 且 throw PRECONDITION 的 if（$from..$to）" } }
    @{ Ok = $true; Var = $var; If = $ifs[0]; Detail = $ifs[0].Extent.Text }
}

function Test-PreconditionCatchMarksFail($ast) {
    <# catch 區塊內，條件含 PRECONDITION 的 if／elseif 子句本體有 `$results[...] = $false`。 #>
    $catches = @($ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.CatchClauseAst] }, $true))
    foreach ($cc in $catches) {
        $ifs = @($cc.Body.FindAll({ param($n) $n -is [System.Management.Automation.Language.IfStatementAst] }, $true))
        foreach ($i in $ifs) {
            foreach ($c in $i.Clauses) {
                if ($c.Item1.Extent.Text -notlike '*PRECONDITION*') { continue }
                $marks = @($c.Item2.FindAll({
                            param($a)
                            $a -is [System.Management.Automation.Language.AssignmentStatementAst] -and
                            $a.Left.Extent.Text -like '$results`[*' -and $a.Right.Extent.Text.Trim() -eq '$false'
                        }, $true))
                if ($marks.Count -ge 1) { return $true }
            }
        }
    }
    $false
}

foreach ($name in 'verify-4.7-wheel.ps1', 'verify-4.7-wheelrouting.ps1') {
    $tokens = $null
    $errors = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseFile((Join-Path $toolsDir $name), [ref]$tokens, [ref]$errors)
    Check "$name 解析 0 錯誤" (@($errors).Count -eq 0) (($errors | ForEach-Object { $_.Message }) -join ' | ')

    $fixedOffsets = @($ast.FindAll({
                param($n)
                if ($n -isnot [System.Management.Automation.Language.BinaryExpressionAst]) { return $false }
                if ($n.Operator -notin 'Plus', 'Minus') { return $false }
                $sides = @($n.Left, $n.Right)
                $hasRect = @($sides | Where-Object {
                        $_ -is [System.Management.Automation.Language.MemberExpressionAst] -and
                        $_.Expression.Extent.Text -eq '$rect' -and $_.Member.Extent.Text -in 'Top', 'Left', 'Bottom', 'Right'
                    }).Count -gt 0
                $hasConst = @($sides | Where-Object { $_ -is [System.Management.Automation.Language.ConstantExpressionAst] }).Count -gt 0
                $hasRect -and $hasConst
            }, $true))
    Check "${name}：沒有 `$rect.<邊> ± <常數> 形式的座標" ($fixedOffsets.Count -eq 0) (($fixedOffsets | ForEach-Object { $_.Extent.Text }) -join ' | ')

    $imports = @($ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq 'Import-Module' -and $n.Extent.Text -like '*ScrollAreaTarget.psm1*' }, $true))
    Check "${name}：載入 lib/ScrollAreaTarget.psm1" ($imports.Count -ge 1)

    $target = @(Find-Commands $ast 'Get-ScrollAreaWheelTarget')
    $scrollable = @(Find-Commands $ast 'Test-ScrollAreaScrollable')
    $hit = @(Find-Commands $ast 'Test-ScrollAreaHit')
    $send = @(Find-Commands $ast 'Send-GuardedWheel')
    $resolve = @(Find-Commands $ast 'Resolve-WheelPoint')
    $occ = @(Find-Commands $ast 'Clear-Occluders')
    Check "${name}：呼叫 Get-ScrollAreaWheelTarget" ($target.Count -ge 1)
    Check "${name}：恰好一處 Send-GuardedWheel" ($send.Count -eq 1) "count=$($send.Count)"
    if ($target.Count -lt 1 -or $send.Count -ne 1 -or $scrollable.Count -lt 1 -or $hit.Count -lt 1 -or $resolve.Count -lt 1 -or $occ.Count -lt 1) {
        Check "${name}：前提函式都存在（Test-ScrollAreaScrollable／Test-ScrollAreaHit／Resolve-WheelPoint／Clear-Occluders）" $false "scrollable=$($scrollable.Count) hit=$($hit.Count) resolve=$($resolve.Count) occ=$($occ.Count)"
        continue
    }
    $sendAt = $send[0].Extent.StartOffset
    Check "${name}：選點在 Clear-Occluders 之前（遮擋檢查用新的點）" ($target[0].Extent.StartOffset -lt $occ[0].Extent.StartOffset)
    $occText = $occ[0].Extent.Text
    Check "${name}：Clear-Occluders 的 -Points 不再由 `$rect 推算" ($occText -notlike '*$rect.*') $occText
    $resolveText = $resolve[0].Extent.Text
    Check "${name}：替代點搜尋限在清單矩形（不再用整個視窗 `$rect）" ($resolveText -notlike '*$rect.*') $resolveText
    Check "${name}：命中測試在 Resolve-WheelPoint 之後（驗最終點）" ($hit[0].Extent.StartOffset -gt $resolve[0].Extent.StartOffset)

    foreach ($fn in 'Test-ScrollAreaScrollable', 'Test-ScrollAreaHit') {
        Check "${name}：$fn 在 Send-GuardedWheel 之前" ((@(Find-Commands $ast $fn)[0].Extent.StartOffset) -lt $sendAt)
        $g = Find-PreconditionGuard $ast $fn
        Check "${name}：$fn 的結果變數（$($g.Var)）有 if (-not $($g.Var).Ok) { throw 'PRECONDITION…' } 擋在 Send-GuardedWheel 之前" ($g.Ok) $g.Detail
        if ($g.Ok) {
            # RED 證據（變異測試）：刪掉這個 if 的副本，同一檢查必須判不通過——證明本檢查真的綁在該前提上。
            $src = $ast.Extent.Text
            $mut = $src.Remove($g.If.Extent.StartOffset, $g.If.Extent.EndOffset - $g.If.Extent.StartOffset)
            $mt = $null
            $me = $null
            $mast = [System.Management.Automation.Language.Parser]::ParseInput($mut, [ref]$mt, [ref]$me)
            $mg = Find-PreconditionGuard $mast $fn
            Check "${name}：變異測試——刪掉 $fn 的 throw 守門 if 後，檢查判不通過" (-not $mg.Ok) $mg.Detail
        }
    }

    Check "${name}：catch 的 PRECONDITION 分支把某個 `$results 設為 `$false（摘要與結束碼不會全 PASS）" (Test-PreconditionCatchMarksFail $ast)

    $text = $ast.Extent.Text
    Check "${name}：記成獨立前提項（清單可捲動）" ($text -match "\`$results\['前提：\.scroll-area 可捲動")
    Check "${name}：記成獨立前提項（滾輪點命中清單）" ($text -match "\`$results\['前提：滾輪點命中 \.scroll-area")
}

Write-Host "`n$($script:Pass) passed, $($script:Fail) failed"
if ($script:Fail -gt 0) { exit 1 }
exit 0
