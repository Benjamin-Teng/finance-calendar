<#
.SYNOPSIS
    host/tools/verify-adaptive-zoom.ps1 的判讀純函式測試（widget-font-scale-per-widget task 5.2；純 PowerShell
    斷言，不需 Pester）。

.DESCRIPTION
    **不執行 verify-adaptive-zoom.ps1**（它會啟動宿主）：以 PowerShell Parser 只取出純函式，在本檔設定它們
    需要的腳本層級變數（假的主螢幕工作區與縮放）後呼叫。涵蓋：
      - 設定檔格式：新格式字級寫在 `widgets.<id>.font_scale`、沒有頂層鍵；F 寫 v0.2.0 格式（只有頂層鍵）。
      - 情境擺放：D 各小工具字級各不相同；E 在多種螢幕上都是「約 0.64 倍設計寬、字級 2.0 → 倍率 ≥ 1.2」。
      - 預期倍率與 at_cap（經 node 呼叫 verify-visual-edges.mjs 的 contentZoom，同 Rust content_zoom_detail）。
      - 讀回、字級狀態、指令被拒、事件、遷移後設定檔、清單列重疊／裁切的判讀（含負向案例）。
      - 清單列量測 JS 以 Node 對假 DOM 執行，結果再餵給判讀函式。
    不呼叫任何 Win32 API、不注入輸入、不啟動宿主，工作階段鎖定時照樣可以跑。
    未開 Set-StrictMode：被測函式與主腳本一樣在非 strict 模式下讀可能不存在的 JSON 屬性（例如 `.r.at_cap`）。

.EXAMPLE
    pwsh -NoProfile -File host/tools/tests/VerifyAdaptiveZoom.Tests.ps1
    結束碼 0＝全部通過、1＝有失敗。
#>
param([string]$Target)
$ErrorActionPreference = 'Stop'

$toolsDir = Split-Path $PSScriptRoot -Parent
if (-not $Target) { $Target = Join-Path $toolsDir 'verify-adaptive-zoom.ps1' }
Import-Module (Join-Path $toolsDir 'lib\NodeProcess.psm1') -Force   # Get-ExpectedZooms 經 Invoke-NodeUtf8 呼叫 node

$script:Pass = 0
$script:Fail = 0
function Check([string]$name, [bool]$cond, [string]$detail = '') {
    if ($cond) { $script:Pass++; Write-Host "PASS  $name" }
    else { $script:Fail++; Write-Host "FAIL  $name $detail" -ForegroundColor Red }
}

$tokens = $null
$errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile($Target, [ref]$tokens, [ref]$errors)
Check 'verify-adaptive-zoom.ps1 解析 0 錯誤' (@($errors).Count -eq 0) (($errors | ForEach-Object { $_.Message }) -join ' | ')
$names = @('Test-SameFont', 'Get-Cells', 'Test-GridOverlap', 'Edge', 'Grid-Rect', 'New-ScenarioPlan', 'ConvertTo-SettingsJson',
    'Get-ExpectedZooms', 'Get-SettingsReadbackProblems', 'Get-FontStateProblems', 'Test-Rejected', 'Get-MapMismatch',
    'Test-HasWidgetFontEvent', 'Get-MigratedFileVerdict', 'Get-RowLayoutExpr', 'Get-RowLayoutProblems', 'Get-InvokeExpr',
    'Set-ProblemResult')
$missingFns = @()
foreach ($name in $names) {
    $fn = $ast.Find({
            param($n)
            $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $name
        }, $true)
    if ($fn) { . ([scriptblock]::Create($fn.Extent.Text)) } else { $missingFns += $name }
}
Check "找得到全部 $($names.Count) 個被測函式" ($missingFns.Count -eq 0) "缺：$($missingFns -join ', ')"
if ($missingFns.Count -gt 0) { Write-Host "VerifyAdaptiveZoom.Tests：$script:Pass passed, $script:Fail failed"; exit 1 }

# 主腳本層級變數（New-ScenarioPlan、Grid-Rect 讀這些）；與主腳本同值，下方另以原始碼核對。
$GRID = 48
$FinanceIds = @('clock', 'macro', 'fixed', 'dynamic', 'quotes')
$DefaultRects = [ordered]@{
    clock = @(15, 1, 16, 10); macro = @(15, 12, 16, 30); fixed = @(32, 1, 15, 17); dynamic = @(32, 19, 15, 23); quotes = @(15, 43, 32, 4)
}
$ScenarioDFonts = [ordered]@{ clock = 1.5; macro = 2.0; fixed = 0.7; dynamic = 1.0; quotes = 1.3 }
$AllIds = @('clock', 'macro', 'fixed', 'dynamic', 'quotes', 'custom1', 'custom2', 'custom3', 'custom4', 'custom5')
$src = [IO.File]::ReadAllText($Target)
Check '測試的 $AllIds 與主腳本 $AllWidgetIds 相同' ($src.Contains("`$AllWidgetIds = @('clock', 'macro', 'fixed', 'dynamic', 'quotes', 'custom1', 'custom2', 'custom3', 'custom4', 'custom5')"))
$rustSettings = [IO.File]::ReadAllText((Join-Path $toolsDir '..\src\settings.rs'))
$rustIdsBlock = [regex]::Match($rustSettings, 'pub const WIDGET_IDS: \[&str; \d+\] = \[([^\]]*)\]')
$rustIds = @([regex]::Matches($rustIdsBlock.Groups[1].Value, '"([^"]+)"') | ForEach-Object { $_.Groups[1].Value })
Check '$AllIds＝settings.rs WIDGET_IDS（十個）' (($rustIds -join ',') -eq ($AllIds -join ',') -and $rustIds.Count -eq 10) "Rust=$($rustIds -join ',')"
Check '主腳本啟動前也以 settings.rs WIDGET_IDS 核對 $AllWidgetIds' ($src -match 'pub const WIDGET_IDS' -and $src -match "\(\`$rustIds -join ','\) -ne \(\`$AllWidgetIds -join ','\)")
Check '測試的 $ScenarioDFonts 與主腳本相同' ($src.Contains('$ScenarioDFonts = [ordered]@{ clock = 1.5; macro = 2.0; fixed = 0.7; dynamic = 1.0; quotes = 1.3 }'))
Check '測試的 $DefaultRects 與主腳本相同' ($src.Contains('clock = @(15, 1, 16, 10); macro = @(15, 12, 16, 30); fixed = @(32, 1, 15, 17); dynamic = @(32, 19, 15, 23); quotes = @(15, 43, 32, 4)'))

$work = Join-Path ([IO.Path]::GetTempPath()) ('vzoom-tests-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force -Path $work | Out-Null
try {
    # ── 1. 設定檔格式 ─────────────────────────────────────────────────────────────
    $pwa = [PSCustomObject]@{ X = 0; Y = 0; W = 2560; H = 1528 }; $pScale = 1.5
    $pD = New-ScenarioPlan 'D'
    $jD = (ConvertTo-SettingsJson $pD 2) | ConvertFrom-Json
    Check '新格式：沒有頂層 font_scale' (-not $jD.PSObject.Properties['font_scale'])
    Check '新格式：version、data_fetch 照寫' ($jD.version -eq 2 -and $jD.data_fetch -eq 'off')
    $dBad = @($FinanceIds | Where-Object { -not (Test-SameFont $jD.widgets.$_.font_scale $ScenarioDFonts[$_]) })
    Check 'D：各小工具字級寫在 widgets.<id>.font_scale 且＝D 字級表' ($dBad.Count -eq 0) "不符：$($dBad -join ',')"
    Check 'D：五個字級至少四種不同值（不同小工具不同字級）' (@($ScenarioDFonts.Values | Sort-Object -Unique).Count -ge 4)
    Check 'D：格座標＝預設' (@($FinanceIds | Where-Object { ($jD.widgets.$_.placement.col, $jD.widgets.$_.placement.row, $jD.widgets.$_.placement.w, $jD.widgets.$_.placement.h) -join ',' -ne ($DefaultRects[$_] -join ',') }).Count -eq 0)
    $pF = New-ScenarioPlan 'F'
    $jF = (ConvertTo-SettingsJson $pF 2) | ConvertFrom-Json
    Check 'F：v0.2.0 格式有頂層 font_scale 1.2' (Test-SameFont $jF.font_scale 1.2)
    Check 'F：v0.2.0 格式各小工具沒有 font_scale' (@($FinanceIds | Where-Object { $jF.widgets.$_.PSObject.Properties['font_scale'] }).Count -eq 0)
    Check 'F：預期字級（遷移後）全部 1.2' (@($FinanceIds | Where-Object { -not (Test-SameFont $pF.Fonts[$_] 1.2) }).Count -eq 0)
    $pG = New-ScenarioPlan 'G'
    $jGText = ConvertTo-SettingsJson $pG 2
    $jG = $jGText | ConvertFrom-Json
    Check 'G：新格式、全部 1.0、無頂層' (-not $jG.PSObject.Properties['font_scale'] -and @($FinanceIds | Where-Object { -not (Test-SameFont $jG.widgets.$_.font_scale 1.0) }).Count -eq 0)
    Check 'G：字級 1.0 序列化為數字（不是字串）' ($jGText -match '"font_scale":\s*1(\.0)?\s*[,}\r\n]')
    $threw = $false; try { [void](New-ScenarioPlan 'D150') } catch { $threw = $true }
    Check '舊情境名 D150 已移除（未知情境丟例外）' $threw

    # ── 2. 情境擺放與預期倍率（多種螢幕）──────────────────────────────────────────
    $screens = @(
        @{ Name = '2560x1528@1.5（本機主螢幕）'; Wa = [PSCustomObject]@{ X = 0; Y = 0; W = 2560; H = 1528 }; Scale = 1.5 },
        @{ Name = '1920x1032@1.0'; Wa = [PSCustomObject]@{ X = 0; Y = 0; W = 1920; H = 1032 }; Scale = 1.0 },
        @{ Name = '3840x2088@1.5'; Wa = [PSCustomObject]@{ X = 0; Y = 0; W = 3840; H = 2088 }; Scale = 1.5 },
        @{ Name = '1366x728@1.0（矮螢幕）'; Wa = [PSCustomObject]@{ X = 0; Y = 0; W = 1366; H = 728 }; Scale = 1.0 },
        @{ Name = '2880x1800@2.0'; Wa = [PSCustomObject]@{ X = -2880; Y = 0; W = 2880; H = 1800 }; Scale = 2.0 }
    )
    $node = Get-Command node -ErrorAction SilentlyContinue
    Check '找得到 node（預期倍率與假 DOM 需要）' ($null -ne $node)
    foreach ($sc in $screens) {
        $pwa = $sc.Wa; $pScale = $sc.Scale
        foreach ($n in 'D', 'E', 'F', 'G') {
            $pl = New-ScenarioPlan $n
            Check "$($sc.Name) 情境 $n：格座標合法（不相交、不超出）" ($pl.Problems.Count -eq 0) ($pl.Problems -join '; ')
        }
        $pE = New-ScenarioPlan 'E'
        $gr = Grid-Rect $pwa $pE.Rects['macro']
        $lw = $gr.W / $pScale
        Check "$($sc.Name) E：總經邏輯寬在 [0.64×500, 0.75×500)（$('{0:N1}' -f $lw)）" ($lw -ge 319.5 -and $lw -lt 375)
        Check "$($sc.Name) E：只有總經字級 2.0" ((Test-SameFont $pE.Fonts['macro'] 2.0) -and @('clock', 'fixed', 'dynamic', 'quotes' | Where-Object { -not (Test-SameFont $pE.Fonts[$_] 1.0) }).Count -eq 0)
        if ($node) {
            $items = @(foreach ($id in $FinanceIds) { $g = Grid-Rect $pwa $pE.Rects[$id]; [PSCustomObject]@{ id = $id; physW = $g.W; physH = $g.H; scale = $pScale; fontScale = [double]$pE.Fonts[$id] } })
            $ex = Get-ExpectedZooms $items $work $toolsDir
            $m = $ex['macro']
            Check "$($sc.Name) E：預期倍率 ≥ 1.2（$('{0:N4}' -f [double]$m.zoom)）" ([double]$m.zoom -ge 1.2)
            Check "$($sc.Name) E：由寬度決定（自適應＝寬/500、框上限 ≥ 2×自適應＝高度不限制）" ([math]::Abs([double]$m.auto - $lw / 500.0) -lt 1e-9 -and [double]$m.capRaw -ge 2.0 * [double]$m.auto) "auto=$($m.auto) capRaw=$($m.capRaw)"
            Check "$($sc.Name) E：頁面 CSS 寬 < 423（收合斷點）且 ≥ min 寬 229" (($lw / [double]$m.zoom) -lt 423 -and ($lw / [double]$m.zoom) -ge 229) "css=$($lw / [double]$m.zoom)"
        }
    }

    if ($node) {
        # ── 3. at_cap 與倍率公式（同 Rust content_zoom_detail）────────────────────────
        # 總經 E 框（2560x1528@1.5：480x923 實體＝320x615 邏輯）。
        $mk = { param($id, $w, $h, $s, $f) [PSCustomObject]@{ id = $id; physW = $w; physH = $h; scale = $s; fontScale = $f } }
        $z = Get-ExpectedZooms @((& $mk 'macro' 480 923 1.5 2.0), (& $mk 'clock' 853 319 1.5 1.0), (& $mk 'quotes' 1706 128 1.5 0.8), (& $mk 'fixed' 800 542 1.5 0.5)) $work $toolsDir
        Check 'E 框字級 2.0：倍率 1.28、at_cap=false' ([math]::Abs([double]$z['macro'].zoom - 1.28) -lt 1e-9 -and $z['macro'].atCap -eq $false) "zoom=$($z['macro'].zoom) atCap=$($z['macro'].atCap)"
        Check '時鐘（min 框＝舒適框）字級 1.0 已 at_cap=true' ($z['clock'].atCap -eq $true)
        Check '行情條字級 0.8：倍率＝0.8×自適應、at_cap=false（再 +0.1 還會變大）' ([math]::Abs([double]$z['quotes'].zoom - 0.8 * [double]$z['quotes'].auto) -lt 1e-9 -and $z['quotes'].atCap -eq $false)
        Check '固定事件字級 0.5：倍率＝0.5×自適應（≥ 0.5 夾下限）' ([math]::Abs([double]$z['fixed'].zoom - [math]::Max(0.5, 0.5 * [double]$z['fixed'].auto)) -lt 1e-9)
        $z = Get-ExpectedZooms @((& $mk 'macro' 480 923 1.5 3.0)) $work $toolsDir
        Check '字級 3.0：at_cap=true（已達字級上限）' ($z['macro'].atCap -eq $true)
        # 字級 2.1 → 自適應 0.64×2.1=1.344 < 框上限 1.3974；+0.1 → 1.408 > 上限 → 會被夾 → 仍變大（1.3974 > 1.344）→ false
        $z = Get-ExpectedZooms @((& $mk 'macro' 480 923 1.5 2.1)) $work $toolsDir
        Check '字級 2.1：+0.1 夾到上限但仍比目前大 → at_cap=false' ($z['macro'].atCap -eq $false -and [math]::Abs([double]$z['macro'].next - 2.2) -lt 1e-9) "atCap=$($z['macro'].atCap) next=$($z['macro'].next)"
        $z = Get-ExpectedZooms @((& $mk 'macro' 480 923 1.5 2.2)) $work $toolsDir
        Check '字級 2.2：自適應×字級 ≥ 框上限 → 倍率＝上限、at_cap=true' ([math]::Abs([double]$z['macro'].zoom - [double]$z['macro'].capRaw) -lt 1e-9 -and $z['macro'].atCap -eq $true)
        $threw = $false; try { [void](Get-ExpectedZooms @((& $mk 'custom1' 100 100 1.0 1.0)) $work $toolsDir) } catch { $threw = $true }
        Check '沒有倍率框的 id → 丟例外（不靜默回 0）' $threw
    }

    # ── 4. 設定讀回判讀 ─────────────────────────────────────────────────────────────
    $rects = [ordered]@{ clock = [int[]]@(15, 1, 16, 10); macro = [int[]]@(15, 12, 16, 30) }
    $fonts = [ordered]@{ clock = 1.5; macro = 2.0 }
    $mkSt = { param($hasTop, $cf, $mf, $mcol = 15) ('{"hasTop":' + $hasTop + ',"widgets":{"clock":{"placement":{"col":15,"row":1,"w":16,"h":10},"font_scale":' + $cf + '},"macro":{"placement":{"col":' + $mcol + ',"row":12,"w":16,"h":30},"font_scale":' + $mf + '},"custom1":{"placement":{"col":0,"row":0,"w":4,"h":4},"font_scale":1.0}}}') | ConvertFrom-Json }
    Check '讀回：全部相符 → 0 問題' (@(Get-SettingsReadbackProblems (& $mkSt 'false' 1.5 2.0) $rects $fonts).Count -eq 0)
    $p = @(Get-SettingsReadbackProblems (& $mkSt 'true' 1.5 2.0) $rects $fonts)
    Check '讀回：頂層仍有 font_scale → 問題' ($p.Count -eq 1 -and $p[0] -match '頂層') ($p -join '; ')
    $p = @(Get-SettingsReadbackProblems (& $mkSt 'false' 1.5 1.0) $rects $fonts)
    Check '讀回：總經字級被重設成 1.0 → 問題' ($p.Count -eq 1 -and $p[0] -match 'macro 字級') ($p -join '; ')
    $p = @(Get-SettingsReadbackProblems (& $mkSt 'false' 1.5 2.0 0) $rects $fonts)
    Check '讀回：格座標不同 → 問題' ($p.Count -eq 1 -and $p[0] -match 'macro 格座標') ($p -join '; ')
    $p = @(Get-SettingsReadbackProblems ('{"widgets":{}}' | ConvertFrom-Json) $rects $fonts)
    Check '讀回：沒有 hasTop、小工具缺 → 3 個問題' ($p.Count -eq 3) ($p -join '; ')
    $p = @(Get-SettingsReadbackProblems ([PSCustomObject]@{ __error = 'boom' }) $rects $fonts)
    Check '讀回：CDP 失敗 → 問題' ($p.Count -eq 1 -and $p[0] -match 'boom')
    Check '讀回：$null → 問題' (@(Get-SettingsReadbackProblems $null $rects $fonts).Count -eq 1)
    $p = @(Get-SettingsReadbackProblems (& $mkSt 'false' 1.2 1.2) $rects ([ordered]@{ clock = 1.2; macro = 1.2 }) 1.2)
    Check 'F 讀回：擴充插槽沒有被遷移成 1.2 → 問題' ($p.Count -eq 1 -and $p[0] -match 'custom1') ($p -join '; ')
    $okF = ('{"hasTop":false,"widgets":{"clock":{"placement":{"col":15,"row":1,"w":16,"h":10},"font_scale":1.2},"macro":{"placement":{"col":15,"row":12,"w":16,"h":30},"font_scale":1.2},"custom1":{"placement":null,"font_scale":1.2}}}') | ConvertFrom-Json
    Check 'F 讀回：全部（含擴充插槽）1.2 → 0 問題' (@(Get-SettingsReadbackProblems $okF $rects ([ordered]@{ clock = 1.2; macro = 1.2 }) 1.2).Count -eq 0)
    # 十個 WIDGET_IDS：F 要求每一個都在 get_settings 裡（擴充插槽完全缺失不得當成遷移成功）。
    $mkAll = { param([string[]]$ids, $f) $w = [ordered]@{}; foreach ($i in $ids) { $w[$i] = [ordered]@{ placement = $(if ($rects.Contains($i)) { [ordered]@{ col = $rects[$i][0]; row = $rects[$i][1]; w = $rects[$i][2]; h = $rects[$i][3] } } else { $null }); font_scale = $f } }; (ConvertTo-Json ([ordered]@{ hasTop = $false; widgets = $w }) -Depth 5) | ConvertFrom-Json }
    $f12 = [ordered]@{ clock = 1.2; macro = 1.2 }
    Check 'F 讀回：十個 id 都在且 1.2 → 0 問題' (@(Get-SettingsReadbackProblems (& $mkAll $AllIds 1.2) $rects $f12 1.2 $AllIds).Count -eq 0)
    $p = @(Get-SettingsReadbackProblems (& $mkAll @('clock', 'macro') 1.2) $rects $f12 1.2 $AllIds)
    Check 'F 讀回：custom1–custom5 等 8 個 id 完全缺失 → 8 個「不在 widgets」' ($p.Count -eq 8 -and @($p | Where-Object { $_ -match '^custom[1-5] 不在 widgets' }).Count -eq 5) ($p -join '; ')
    $p = @(Get-SettingsReadbackProblems (& $mkAll @($AllIds | Where-Object { $_ -ne 'custom3' }) 1.2) $rects $f12 1.2 $AllIds)
    Check 'F 讀回：只缺 custom3 → 1 個問題' ($p.Count -eq 1 -and $p[0] -match '^custom3 不在') ($p -join '; ')
    Check '非 F 讀回：不傳 $AllIds 時不要求擴充插槽（相容舊呼叫）' (@(Get-SettingsReadbackProblems (& $mkAll @('clock', 'macro') 1.2) $rects $f12).Count -eq 0)

    # ── 5. 字級狀態判讀 ─────────────────────────────────────────────────────────────
    $mkFs = { param($ok, $f, $c, $err = '') if ($ok) { ('{"ok":true,"r":{"font_scale":' + $f + ',"at_cap":' + $c + '}}') | ConvertFrom-Json } else { [PSCustomObject]@{ ok = $false; err = $err } } }
    $states = @{ clock = (& $mkFs $true 1.5 'true'); macro = (& $mkFs $true 2.0 'false') }
    $atc = [ordered]@{ clock = $true; macro = $false }
    Check '字級狀態：相符（含 at_cap）→ 0 問題' (@(Get-FontStateProblems $states $fonts $atc).Count -eq 0)
    $states2 = @{ clock = (& $mkFs $true 1.5 'true'); macro = (& $mkFs $true 2.0 'true') }
    Check '字級狀態：不比 at_cap 時 at_cap 不同也 0 問題' (@(Get-FontStateProblems $states2 $fonts).Count -eq 0)
    $p = @(Get-FontStateProblems $states2 $fonts $atc)
    Check '字級狀態：at_cap 與公式不同 → 問題' ($p.Count -eq 1 -and $p[0] -match 'macro at_cap') ($p -join '; ')
    $p = @(Get-FontStateProblems @{ clock = (& $mkFs $true 1.5 '"true"'); macro = (& $mkFs $true 2.0 'false') } $fonts $atc)
    Check '字級狀態：at_cap 不是布林 → 問題' ($p.Count -eq 1 -and $p[0] -match '不是布林') ($p -join '; ')
    $p = @(Get-FontStateProblems @{ clock = (& $mkFs $true 1.6 'true'); macro = (& $mkFs $false 0 0 '版面鎖定') } $fonts)
    Check '字級狀態：字級不同、被拒 → 2 個問題' ($p.Count -eq 2 -and ($p -join ' ') -match '1\.6' -and ($p -join ' ') -match '版面鎖定') ($p -join '; ')
    $p = @(Get-FontStateProblems @{ clock = [PSCustomObject]@{ __error = '找不到分頁' } } $fonts)
    Check '字級狀態：CDP 失敗與缺結果 → 2 個問題' ($p.Count -eq 2) ($p -join '; ')

    # ── 6. 指令被拒、對照、事件 ─────────────────────────────────────────────────────
    Check 'Test-Rejected：{ok:false, err} → true' (Test-Rejected ([PSCustomObject]@{ ok = $false; err = '不在編輯版面' }))
    Check 'Test-Rejected：成功 → false' (-not (Test-Rejected ('{"ok":true,"r":{"font_scale":1.1,"at_cap":false}}' | ConvertFrom-Json)))
    Check 'Test-Rejected：CDP 失敗 → false（不是被宿主拒絕）' (-not (Test-Rejected ([PSCustomObject]@{ __error = 'x' })))
    Check 'Test-Rejected：$null → false' (-not (Test-Rejected $null))
    Check 'Test-Rejected：ok=false 但沒有錯誤訊息 → false' (-not (Test-Rejected ([PSCustomObject]@{ ok = $false; err = '' })))
    $want = [ordered]@{ clock = '100%'; macro = '110%' }
    Check 'Get-MapMismatch：相同 → 0' (@(Get-MapMismatch @{ clock = '100%'; macro = '110%' } $want).Count -eq 0)
    $p = @(Get-MapMismatch @{ clock = '110%'; macro = '110%' } $want)
    Check 'Get-MapMismatch：別的小工具也變 110% → 1 筆' ($p.Count -eq 1 -and $p[0] -match '^clock=110%') ($p -join '; ')
    $p = @(Get-MapMismatch @{ clock = '100%' } $want)
    Check 'Get-MapMismatch：缺鍵 → 1 筆' ($p.Count -eq 1 -and $p[0] -match '缺') ($p -join '; ')
    $evs = '[{"id":"clock","font_scale":1.0,"at_cap":true},{"id":"macro","font_scale":1.1,"at_cap":false}]' | ConvertFrom-Json
    Check '事件：有 id=macro 1.1 → true' (Test-HasWidgetFontEvent $evs 'macro' 1.1)
    Check '事件：沒有 id=fixed → false' (-not (Test-HasWidgetFontEvent $evs 'fixed' 1.1))
    Check '事件：macro 字級不是 1.2 → false' (-not (Test-HasWidgetFontEvent $evs 'macro' 1.2))
    Check '事件：空陣列／$null → false' (-not (Test-HasWidgetFontEvent @() 'macro' 1.1) -and -not (Test-HasWidgetFontEvent $null 'macro' 1.1))
    $ie = Get-InvokeExpr 'adjust_widget_font_scale' '{ step: 1 }'
    Check 'Get-InvokeExpr：呼叫指令並帶參數、成功與拒絕都包成 { ok }' ($ie.Contains("invoke('adjust_widget_font_scale', { step: 1 })") -and $ie.Contains('ok: true') -and $ie.Contains('ok: false, err: String(e)'))
    Check 'Get-InvokeExpr：沒有參數時不多逗號' ((Get-InvokeExpr 'get_widget_font_state').Contains("invoke('get_widget_font_state')"))

    # ── 6b. Set-ProblemResult：主流程「判讀函式直接當參數」的實際呼叫形狀 ────────────────────
    # task 5.2 實跑：判讀函式沒問題時 `return @()` 不輸出任何東西，當參數傳進來變 $null，舊版 `@($Problems)` 算成
    # 1 筆而 FAIL（log「問題：」後面是空的）。這裡照主流程的寫法 `Set-ProblemResult 'k' (Get-...)` 呼叫，不先包 @()。
    $script:captured = @{}
    function Log([string]$m) { }
    function Set-Result([string]$Key, $Value) { $script:captured[$Key] = $Value }
    Set-ProblemResult 'readback-ok' (Get-SettingsReadbackProblems (& $mkSt 'false' 1.5 2.0) $rects $fonts)
    Check 'Set-ProblemResult：讀回沒有問題（函式回空）→ PASS' ($script:captured['readback-ok'] -eq $true) "值=$($script:captured['readback-ok'])"
    Set-ProblemResult 'fontstate-ok' (Get-FontStateProblems $states $fonts $atc)
    Check 'Set-ProblemResult：字級狀態沒有問題 → PASS' ($script:captured['fontstate-ok'] -eq $true) "值=$($script:captured['fontstate-ok'])"
    Set-ProblemResult 'map-ok' (Get-MapMismatch @{ clock = '100%'; macro = '110%' } $want)
    Check 'Set-ProblemResult：百分比對照相同 → PASS' ($script:captured['map-ok'] -eq $true) "值=$($script:captured['map-ok'])"
    Set-ProblemResult 'empty-var' @()
    Check 'Set-ProblemResult：空陣列變數 → PASS' ($script:captured['empty-var'] -eq $true)
    Set-ProblemResult 'one' (Get-FontStateProblems $states2 $fonts $atc)
    Check 'Set-ProblemResult：函式回單一問題字串 → FAIL' ($script:captured['one'] -eq $false)
    Set-ProblemResult 'two' (Get-FontStateProblems @{ clock = (& $mkFs $true 1.6 'true'); macro = (& $mkFs $false 0 0 '版面鎖定') } $fonts)
    Check 'Set-ProblemResult：函式回兩個問題 → FAIL' ($script:captured['two'] -eq $false)
    Set-ProblemResult 'blank' @('', ' ', $null)
    Check 'Set-ProblemResult：只有空白／$null 元素不算問題 → PASS' ($script:captured['blank'] -eq $true)

    # ── 7. F：遷移後設定檔 ───────────────────────────────────────────────────────────
    $written = ConvertTo-SettingsJson $pF 2
    $jw = $written | ConvertFrom-Json
    Check 'F 寫入：只列五個財經小工具（刻意省略 custom1–custom5，實證未列出者也遷移）' ((@($jw.widgets.PSObject.Properties.Name) -join ',') -eq ($FinanceIds -join ','))
    $v = Get-MigratedFileVerdict $written $written $AllIds 1.2 $false
    Check '遷移檔：內容與寫入相同、修改時間沒變 → FAIL（宿主沒存檔）' ($v.Verdict -eq 'FAIL' -and $v.Detail -match '沒有存檔') $v.Detail
    $v = Get-MigratedFileVerdict $written $written $AllIds 1.2 $true
    Check '遷移檔：修改時間變了但內容與寫入相同 → FAIL（沒改寫成新格式）' ($v.Verdict -eq 'FAIL' -and $v.Detail -match '完全相同') $v.Detail
    $saved = '{"version":2,"layout_locked":true,"widgets":{' + (($AllIds | ForEach-Object { '"' + $_ + '":{"font_scale":1.2}' }) -join ',') + '}}'
    $v = Get-MigratedFileVerdict $written $saved $AllIds 1.2 $true
    Check '遷移檔：已存檔、無頂層、十個 id 全部 1.2 → PASS' ($v.Verdict -eq 'PASS') $v.Detail
    $v = Get-MigratedFileVerdict $written $saved $AllIds 1.2 $false
    Check '遷移檔：內容對但修改時間沒變 → FAIL' ($v.Verdict -eq 'FAIL' -and $v.Detail -match '修改時間') $v.Detail
    $v = Get-MigratedFileVerdict $written ($saved -replace '^\{', '{"font_scale":1.2,') $AllIds 1.2 $true
    Check '遷移檔：仍有頂層 font_scale → FAIL' ($v.Verdict -eq 'FAIL' -and $v.Detail -match '頂層') $v.Detail
    $v = Get-MigratedFileVerdict $written ($saved -replace '"custom1":\{"font_scale":1.2\}', '"custom1":{"font_scale":1.0}') $AllIds 1.2 $true
    Check '遷移檔：擴充插槽是 1.0 → FAIL' ($v.Verdict -eq 'FAIL' -and $v.Detail -match 'custom1') $v.Detail
    $noCustom = '{"version":2,"layout_locked":true,"widgets":{' + (($FinanceIds | ForEach-Object { '"' + $_ + '":{"font_scale":1.2}' }) -join ',') + '}}'
    $v = Get-MigratedFileVerdict $written $noCustom $AllIds 1.2 $true
    Check '遷移檔：custom1–custom5 完全缺失 → FAIL（五個都列出）' ($v.Verdict -eq 'FAIL' -and @([regex]::Matches($v.Detail, 'custom[1-5] 不在')).Count -eq 5) $v.Detail
    $v = Get-MigratedFileVerdict $written ($saved -replace '"quotes":\{"font_scale":1.2\},', '') $AllIds 1.2 $true
    Check '遷移檔：少了財經小工具 → FAIL' ($v.Verdict -eq 'FAIL' -and $v.Detail -match 'quotes 不在') $v.Detail
    $v = Get-MigratedFileVerdict $written ($saved -replace '"macro":\{"font_scale":1.2\}', '"macro":{}') $AllIds 1.2 $true
    Check '遷移檔：某小工具沒有字級 → FAIL' ($v.Verdict -eq 'FAIL' -and $v.Detail -match 'macro') $v.Detail
    Check '遷移檔：不是 JSON → FAIL' ((Get-MigratedFileVerdict $written '{not json' $AllIds 1.2 $true).Verdict -eq 'FAIL')
    Check '遷移檔：讀不到（空字串）→ FAIL' ((Get-MigratedFileVerdict $written '' $AllIds 1.2 $true).Verdict -eq 'FAIL')
    Check '遷移檔：判讀結果只有 PASS／FAIL（不再有 NOTE）' (-not $src.Contains("Verdict = 'NOTE'"))

    # ── 8. 清單列判讀 ───────────────────────────────────────────────────────────────
    function K($n, $l, $t, $r, $b, $sw = 10, $cw = 10) { [PSCustomObject]@{ n = $n; sw = $sw; cw = $cw; l = $l; t = $t; r = $r; b = $b } }
    function Rw($i, $kids, $l = 0, $r = 250, $sw = 230, $cw = 230) { [PSCustomObject]@{ i = $i; sw = $sw; cw = $cw; l = $l; r = $r; kids = @($kids) } }
    function Mm($rows, $vw = 260) { [PSCustomObject]@{ vw = $vw; flexWrap = 'wrap'; rows = @($rows) } }
    $clean = Mm @(
        (Rw 0 @((K 'span.d' 10 10 96 30), (K 'span.cchip' 106 10 130 30), (K 'b' 140 10 240 50), (K 'span.n' 10 52 240 70))),
        (Rw 1 @((K 'span.d' 10 80 96 100), (K 'b' 106 80 240 100)))
    )
    $r = Get-RowLayoutProblems $clean -RequireRows
    Check '清單列：收合兩行、互不相交 → 0 問題' (@($r.Problems).Count -eq 0 -and $r.RowCount -eq 2 -and $r.KidCount -eq 6) (@($r.Problems) -join '; ')
    $r = Get-RowLayoutProblems (Mm @(Rw 0 @((K 'span.d' 10 10 96 30), (K 'b' 96 10 200 30))))
    Check '清單列：只碰邊（右緣＝左緣）不算重疊' (@($r.Problems).Count -eq 0) (@($r.Problems) -join '; ')
    $r = Get-RowLayoutProblems (Mm @(Rw 0 @((K 'span.d' 10 10 120 30), (K 'b' 100 10 200 30))))
    Check '清單列：時間與標題重疊 20px → 問題' (@($r.Problems).Count -eq 1 -and $r.Problems[0] -match '重疊' -and $r.Problems[0] -match 'span\.d') (@($r.Problems) -join '; ')
    $r = Get-RowLayoutProblems (Mm @(Rw 0 @((K 'b' 10 10 200 30), (K 'span.n' 150 25 240 45))))
    Check '清單列：第二行數值與標題縱向重疊 5px → 問題' (@($r.Problems).Count -eq 1 -and $r.Problems[0] -match '重疊') (@($r.Problems) -join '; ')
    $r = Get-RowLayoutProblems (Mm @(Rw 0 @((K 'b' 10 10 200 30 180 150))))
    Check '清單列：子元素 scrollWidth > clientWidth → 裁切' (@($r.Problems).Count -eq 1 -and $r.Problems[0] -match '被裁切') (@($r.Problems) -join '; ')
    $r = Get-RowLayoutProblems (Mm @(Rw 0 @((K 'b' 10 10 200 30)) 0 250 260 230))
    Check '清單列：列 scrollWidth > clientWidth → 溢出' (@($r.Problems).Count -eq 1 -and $r.Problems[0] -match '溢出') (@($r.Problems) -join '; ')
    $r = Get-RowLayoutProblems (Mm @(Rw 0 @((K 'span.n' 200 10 270 30))))
    Check '清單列：子元素右緣超出列 → 問題' (@($r.Problems).Count -eq 1 -and $r.Problems[0] -match '超出列') (@($r.Problems) -join '; ')
    $r = Get-RowLayoutProblems (Mm @(Rw 0 @((K 'b' 10 10 200 30)) 0 300) 260)
    Check '清單列：列超出 viewport → 問題' (@($r.Problems).Count -eq 1 -and $r.Problems[0] -match 'viewport') (@($r.Problems) -join '; ')
    $r = Get-RowLayoutProblems (Mm @(Rw 0 @((K 'b' 10 10 200.4 30), (K 'span.n' 200 10 240 30))))
    Check '清單列：0.4px 的次像素交疊在容差內' (@($r.Problems).Count -eq 0) (@($r.Problems) -join '; ')
    $empty = [PSCustomObject]@{ vw = 260; flexWrap = $null; rows = @() }
    Check '清單列：沒有列、不要求 → 0 問題' (@((Get-RowLayoutProblems $empty).Problems).Count -eq 0)
    Check '清單列：沒有列、-RequireRows → 問題' (@((Get-RowLayoutProblems $empty -RequireRows).Problems).Count -eq 1)
    Check '清單列：CDP 失敗 → 問題' (@((Get-RowLayoutProblems ([PSCustomObject]@{ __error = 'x' })).Problems).Count -eq 1)
    Check '清單列：$null → 問題' (@((Get-RowLayoutProblems $null).Problems).Count -eq 1)
    $many = Mm @(0..19 | ForEach-Object { Rw $_ @((K 'span.d' 10 10 120 30), (K 'b' 100 10 200 30)) })
    $r = Get-RowLayoutProblems $many
    Check '清單列：問題超過 12 筆只列 12 筆＋摘要' (@($r.Problems).Count -eq 13 -and $r.Problems[12] -match '另有 8 筆') "$(@($r.Problems).Count)"

    # ── 9. 清單列量測 JS（Node 假 DOM）→ 判讀 ───────────────────────────────────────
    $expr = Get-RowLayoutExpr
    Check '列量測 JS：只用單引號字串（不靠原生參數傳遞跳脫雙引號）' (-not $expr.Contains('"'))
    if ($node) {
        $harness = @'
const rect = (l, t, r, b) => ({ left: l, top: t, right: r, bottom: b, width: r - l, height: b - t });
const el = (tag, cls, rc, sw, cw, kids = []) => ({ tagName: tag, className: cls, scrollWidth: sw, clientWidth: cw, children: kids, getBoundingClientRect: () => rc, _wrap: 'wrap' });
const rows = [
  el('LI', 'ev macro high', rect(0, 0, 250, 80), 230, 230, [
    el('SPAN', 'd', rect(10, 10, 96, 30), 86, 86),
    el('SPAN', 'cchip US', rect(106, 10, 130, 30), 24, 24),
    el('SPAN', 'n', rect(0, 0, 0, 0), 0, 0),
    el('B', '', rect(140, 10, 240, 50), 100, 100),
  ]),
  el('LI', 'ev macro', rect(0, 0, 0, 0), 0, 0, []),
  el('LI', 'ev macro medium', rect(0, 90, 250, 120), 230, 230, [
    el('SPAN', 'd', rect(10, 95, 120, 115), 110, 110),
    el('B', '', rect(100, 95, 240, 115), 140, 140),
  ]),
];
globalThis.window = { innerWidth: 260 };
globalThis.document = { querySelectorAll: (s) => (s === '.ev' ? rows : []) };
globalThis.getComputedStyle = (e) => ({ flexWrap: e._wrap });
console.log(JSON.stringify(eval(process.env.FC_EXPR)));
'@
        $tmpJs = Join-Path $work 'rows-harness.cjs'
        try {
            [IO.File]::WriteAllText($tmpJs, $harness)
            $env:FC_EXPR = $expr
            $out = (& node $tmpJs 2>&1) -join "`n"
            $j = $null
            try { $j = $out | ConvertFrom-Json } catch { }
            Check '假 DOM：JS 執行成功且回傳 JSON' ($null -ne $j) $out
            if ($j) {
                Check '假 DOM：看不見的列被略過（2 列）、flex-wrap 讀第一列' (@($j.rows).Count -eq 2 -and $j.flexWrap -eq 'wrap' -and $j.vw -eq 260) $out
                Check '假 DOM：看不見的子元素（空的 .n）被略過' (@($j.rows[0].kids).Count -eq 3 -and @($j.rows[0].kids | ForEach-Object { $_.n }) -notcontains 'span.n') $out
                Check '假 DOM：子元素名稱含全部 class' (@($j.rows[0].kids | ForEach-Object { $_.n }) -contains 'span.cchip.US') $out
                $r = Get-RowLayoutProblems $j -RequireRows
                Check '假 DOM 結果餵進判讀：第 2 列時間與標題重疊 → 1 筆' (@($r.Problems).Count -eq 1 -and $r.Problems[0] -match '列 1 span\.d') (@($r.Problems) -join '; ')
            }
        } finally {
            Remove-Item Env:FC_EXPR -ErrorAction SilentlyContinue
        }
    }

    # ── 10. 主流程結構 ─────────────────────────────────────────────────────────────
    Check '預設情境為 A–G' ($src -match "\[string\[\]\]\`$Scenarios = @\('A', 'B', 'C', 'D', 'E', 'F', 'G'\)")
    Check '主流程對清單以 Get-RowLayoutExpr 量測、Get-RowLayoutProblems 判定' ($src -match 'Invoke-Json "w=\$id" \(Get-RowLayoutExpr\)' -and $src -match 'Get-RowLayoutProblems \$rm -RequireRows')
    Check '主流程以 Get-SettingsReadbackProblems 判定讀回（傳入各自字級、遷移值與十個 id）' ($src -match 'Get-SettingsReadbackProblems \$st \$plan\.Rects \$plan\.Fonts \$plan\.LegacyTopFont \$AllWidgetIds')
    Check '預期倍率以各小工具自己的字級計算' ($src -match 'fontScale = \[double\]\$plan\.Fonts\[\$_\]')
    Check 'F 以 Get-MigratedFileVerdict 判定（十個 id、修改時間）並進結果' ($src -match 'Get-MigratedFileVerdict \$json \$readBack \$AllWidgetIds \$plan\.LegacyTopFont \(\$nowTicks -gt \$cfgWrittenTicks\)' -and $src -match "Set-Result 'F：宿主已改寫設定檔")
    Check 'F 判定兩次 set_edit_mode 都回 ok' ($src.Contains('($e1.ok -eq $true -and $e2.ok -eq $true)'))
    Check '主腳本不再有 NOTE 豁免（$notes 已移除）' (-not ($src -match '\$notes'))
    Check 'G 鎖定時與離開後都以 Test-Rejected 判定' (([regex]::Matches($src, 'Test-Rejected \$r[13]')).Count -eq 2)
    Check 'G 以 CDP 呼叫 set_edit_mode 進出編輯版面' ($src.Contains("Get-InvokeExpr 'set_edit_mode' '{ enabled: true }'") -and $src.Contains("Get-InvokeExpr 'set_edit_mode' '{ enabled: false }'"))
    Check '設定檔不再寫頂層 font_scale（除 F 的 LegacyTopFont）' (-not ($src -match "font_scale = \`$Plan\.FontScale"))
}
finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}

Write-Host "VerifyAdaptiveZoom.Tests：$script:Pass passed, $script:Fail failed"
if ($script:Fail -gt 0) { exit 1 }
exit 0
