<#
.SYNOPSIS
    verify-6.1-appearance-hittest.ps1、verify-6.1-icon-click.ps1、probe-1.3.ps1 的判定函式 mock 測試與
    結構檢查（純 PowerShell 斷言，不需 Pester）。

.DESCRIPTION
    **不執行三支腳本**（它們會啟動宿主、注入點擊／滾輪）：以 PowerShell Parser 只取出純函式，餵合成的
    命中結果、CDP 回傳文字與候選圖示；其餘以語法樹做結構檢查。

    fix F4（review 6.1-verify、sectionA）：
      - 點穿只有命中桌面才算 PASS，被一般視窗覆蓋＝BLOCKED（2026 證據 20/20 命中瀏覽器卻判 PASS）；
      - update_settings 回傳要斷言；背景＝半透明深色（alpha＝opacity、RGB 各 ≤ 64）；
      - 挑圖示要做遮蔽檢查、由近到遠；BLOCKED 路徑不得在 catch 內關 writer／exit；
      - probe-1.3 在 occluded 時回 3、不點擊、不印 PASS。

.EXAMPLE
    pwsh -NoProfile -File host/tools/tests/Verify61.Tests.ps1
    結束碼 0＝全部通過、1＝有失敗。
#>
param([string]$HitTestTarget, [string]$IconClickTarget, [string]$Probe13Target)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$toolsDir = Split-Path $PSScriptRoot -Parent
if (-not $HitTestTarget) { $HitTestTarget = Join-Path $toolsDir 'verify-6.1-appearance-hittest.ps1' }
if (-not $IconClickTarget) { $IconClickTarget = Join-Path $toolsDir 'verify-6.1-icon-click.ps1' }
if (-not $Probe13Target) { $Probe13Target = Join-Path $toolsDir 'probe-1.3.ps1' }

$script:Pass = 0
$script:Fail = 0
function Check([string]$name, [bool]$cond, [string]$detail = '') {
    if ($cond) { $script:Pass++; Write-Host "PASS  $name" }
    else { $script:Fail++; Write-Host "FAIL  $name $detail" -ForegroundColor Red }
}

function Import-Functions([string]$Path, [string[]]$Names) {
    $tokens = $null; $errors = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseFile($Path, [ref]$tokens, [ref]$errors)
    Check "$(Split-Path $Path -Leaf) 解析 0 錯誤" (@($errors).Count -eq 0) (($errors | ForEach-Object { $_.Message }) -join ' | ')
    $missing = @()
    foreach ($name in $Names) {
        $fn = $ast.Find({ param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $name }, $true)
        if ($fn) { . ([scriptblock]::Create($fn.Extent.Text)); Set-Item -Path "function:script:$name" -Value (Get-Item "function:$name").ScriptBlock }
        else { $missing += $name }
    }
    Check "$(Split-Path $Path -Leaf) 找得到函式 $($Names -join '、')" ($missing.Count -eq 0) "缺：$($missing -join ',')"
    return $ast
}

# ================================================================ appearance-hittest
$htAst = Import-Functions $HitTestTarget @('Get-OutsideHitVerdict', 'Get-AggregateVerdict', 'Test-UpdateSettingsResult', 'Test-TranslucentDarkBackground', 'Resolve-OutsideSample')
$hostPid = 4242
if (Get-Command Get-OutsideHitVerdict -ErrorAction SilentlyContinue) {
    Check '點穿：命中宿主 → FAIL' ((Get-OutsideHitVerdict -HitPid $hostPid -HostPid $hostPid -HitClass 'Chrome_RenderWidgetHostHWND' -RootClass 'Tauri Window') -eq 'FAIL')
    Check '點穿：命中桌面圖示清單 SysListView32 → PASS' ((Get-OutsideHitVerdict -HitPid 5500 -HostPid $hostPid -HitClass 'SysListView32' -RootClass 'Progman') -eq 'PASS')
    Check '點穿：命中 WorkerW（root）→ PASS' ((Get-OutsideHitVerdict -HitPid 5500 -HostPid $hostPid -HitClass 'SHELLDLL_DefView' -RootClass 'WorkerW') -eq 'PASS')
    # 2026 證據：20 個邊外點全命中瀏覽器（Chrome_RenderWidgetHostHWND pid=66044）。
    Check '點穿：被瀏覽器覆蓋 → BLOCKED（不是 PASS）' ((Get-OutsideHitVerdict -HitPid 66044 -HostPid $hostPid -HitClass 'Chrome_RenderWidgetHostHWND' -RootClass 'Chrome_WidgetWin_1') -eq 'BLOCKED')
    Check '點穿：沒命中任何視窗 → BLOCKED' ((Get-OutsideHitVerdict -HitPid 0 -HostPid $hostPid -HitClass '' -RootClass '' -HitIsNull $true) -eq 'BLOCKED')
}
if (Get-Command Get-AggregateVerdict -ErrorAction SilentlyContinue) {
    Check '彙總：全 PASS → PASS' ((Get-AggregateVerdict @('PASS', 'PASS')) -eq 'PASS')
    Check '彙總：有 BLOCKED 無 FAIL → BLOCKED' ((Get-AggregateVerdict @('PASS', 'BLOCKED')) -eq 'BLOCKED')
    Check '彙總：有 FAIL → FAIL' ((Get-AggregateVerdict @('BLOCKED', 'FAIL', 'PASS')) -eq 'FAIL')
    Check '彙總：沒有任何取樣 → BLOCKED' ((Get-AggregateVerdict @()) -eq 'BLOCKED')
    Check '彙總：2026 證據型態（20 點全被覆蓋）→ BLOCKED' ((Get-AggregateVerdict @(1..20 | ForEach-Object { 'BLOCKED' })) -eq 'BLOCKED')
}
if (Get-Command Test-UpdateSettingsResult -ErrorAction SilentlyContinue) {
    Check 'update_settings：回傳 "acrylic" 且要求 acrylic → 成功' (Test-UpdateSettingsResult '"acrylic"' 'acrylic')
    Check 'update_settings：回傳 "solid" 但要求 acrylic → 失敗' (-not (Test-UpdateSettingsResult '"solid"' 'acrylic'))
    Check 'update_settings：invoke 被拒（錯誤訊息）→ 失敗' (-not (Test-UpdateSettingsResult 'Error: update_settings 不接受 widgets.clock.placement' 'solid'))
    Check 'update_settings：回傳 null → 失敗' (-not (Test-UpdateSettingsResult 'null' 'solid'))
}
if (Get-Command Test-TranslucentDarkBackground -ErrorAction SilentlyContinue) {
    Check '背景：rgba(13, 19, 32, 0.55) → 半透明深色' (Test-TranslucentDarkBackground 'rgba(13, 19, 32, 0.55)').Ok
    Check '背景：CDP 帶引號的 "rgba(13, 19, 32, 0.55)" 也能解析' (Test-TranslucentDarkBackground '"rgba(13, 19, 32, 0.55)"').Ok
    Check '背景：不透明白色 rgb(255, 255, 255) → 不通過' (-not (Test-TranslucentDarkBackground 'rgb(255, 255, 255)').Ok)
    Check '背景：不透明深色 rgb(13, 19, 32)（alpha＝1）→ 不通過' (-not (Test-TranslucentDarkBackground 'rgb(13, 19, 32)').Ok)
    Check '背景：幾乎全透明 rgba(13, 19, 32, 0.01) → 不通過' (-not (Test-TranslucentDarkBackground 'rgba(13, 19, 32, 0.01)').Ok)
    Check '背景：全透明 rgba(0, 0, 0, 0) → 不通過' (-not (Test-TranslucentDarkBackground 'rgba(0, 0, 0, 0)').Ok)
    Check '背景：淺色半透明 rgba(200, 200, 200, 0.55) → 不通過' (-not (Test-TranslucentDarkBackground 'rgba(200, 200, 200, 0.55)').Ok)
    Check '背景：transparent／null → 不通過' ((-not (Test-TranslucentDarkBackground '"transparent"').Ok) -and (-not (Test-TranslucentDarkBackground 'null').Ok))
    Check '背景：使用者把 opacity 設 0.8 時以 ExpectedAlpha 判斷' (Test-TranslucentDarkBackground -Css 'rgba(13, 19, 32, 0.8)' -ExpectedAlpha 0.8).Ok
}
$htText = Get-Content $HitTestTarget -Raw
Check 'hittest 靜態：點穿彙總為 BLOCKED 時記入 BLOCKED 清單、不寫成 PASS' ($htText -match "if \(\`$outsideAgg -eq 'BLOCKED'\) \{ \`$blockedItems\.Add")
# review 31c7c53：結束碼改經 lib/VerifyVerdict 的 Get-VerdictExitCode（FAIL 1 ＞ 環境 3）。
Check 'hittest 靜態：沒有 FAIL 但有 BLOCKED 時結束碼 3' ($htText -match 'Get-VerdictExitCode -Results \$results -EnvBlocked:\(\$blockedItems\.Count -gt 0\)')
Check 'hittest 靜態：「切換後」斷言以切換成功為前提' ($htText -match '\$acrylicResult\.Switched -and \$acrylicResult\.BackdropOk' -and $htText -match '\$solidResult\.Switched -and \$solidResult\.BackgroundOk')

# ================================================================ fix F9：遮擋判讀改走 lib/Occluders（-TargetIsDesktop）
# mock 的 Clear-Occluders 結果（Action／Class 與 lib 相同格式），判讀用 lib 真的 Get-OccluderVerdict／Assert-OccluderResult。
Import-Module (Join-Path $toolsDir 'lib/Occluders.psm1') -Force
function New-Occ([string]$Action, [string]$Class = '') {
    if ($Action -eq 'clear') { return [PSCustomObject]@{ Ok = $true; Action = 'clear'; Class = ''; Reason = '' } }
    [PSCustomObject]@{ Ok = $false; Action = $Action; Class = $Class; Reason = "(10,10) mock $Action $Class" }
}

if (Get-Command Resolve-OutsideSample -ErrorAction SilentlyContinue) {
    $desk = [PSCustomObject]@{ HitPid = 5500; HitClass = 'SysListView32'; RootClass = 'Progman'; HitIsNull = $false }
    $script:observed = 0
    $obs = { $script:observed++; $desk }
    $cases = [ordered]@{
        '清除後命中目標桌面 → 依實際命中判 PASS'           = @{ Occ = (New-Occ 'clear'); Want = 'PASS'; Observe = $true }
        '命中另一扇桌面 WorkerW（也是桌面）→ PASS'          = @{ Occ = (New-Occ 'shell' 'WorkerW'); Want = 'PASS'; Observe = $true }
        '宿主視窗蓋住邊外點 → FAIL、不再觀察'               = @{ Occ = (New-Occ 'host-other' 'Tauri Window'); Want = 'FAIL'; Observe = $false }
        '沒命中任何視窗 → FAIL（lib 規則）'                 = @{ Occ = (New-Occ 'none'); Want = 'FAIL'; Observe = $false }
        '系統 UI 蓋住（不在白名單、不最小化）→ BLOCKED'      = @{ Occ = (New-Occ 'system' 'Shell_SystemDim'); Want = 'BLOCKED'; Observe = $false }
        '最小化後仍蓋著 → BLOCKED'                          = @{ Occ = (New-Occ 'stuck' 'Chrome_WidgetWin_1'); Want = 'BLOCKED'; Observe = $false }
        '工作列 → BLOCKED'                                  = @{ Occ = (New-Occ 'shell' 'Shell_TrayWnd'); Want = 'BLOCKED'; Observe = $false }
        '$null 結果 → FAIL（證明不了是環境）'               = @{ Occ = $null; Want = 'FAIL'; Observe = $false }
    }
    foreach ($k in $cases.Keys) {
        $script:observed = 0
        $r = Resolve-OutsideSample -Occ $cases[$k].Occ -HostPid $hostPid -Observe $obs
        Check "點穿取樣：$k" ($r.Verdict -eq $cases[$k].Want -and (($script:observed -gt 0) -eq $cases[$k].Observe)) "verdict=$($r.Verdict) observed=$($script:observed) note=$($r.Note)"
    }
    $r = Resolve-OutsideSample -Occ (New-Occ 'clear') -HostPid $hostPid -Observe { [PSCustomObject]@{ HitPid = $hostPid; HitClass = 'Chrome_RenderWidgetHostHWND'; RootClass = 'Tauri Window'; HitIsNull = $false } }
    Check '點穿取樣：清除後實際命中宿主 → FAIL' ($r.Verdict -eq 'FAIL') "verdict=$($r.Verdict)"
}
Check 'hittest 靜態：點穿取樣經 Clear-Occluders（目標＝殼層桌面視窗）再交給 Resolve-OutsideSample' ($htText -match 'Clear-Occluders -HostPid \$hostPid -TargetHwnd \$desktopTop' -and $htText -match 'Resolve-OutsideSample -Occ \$occ')

# ================================================================ icon-click
$icAst = Import-Functions $IconClickTarget @('Select-ReachableIcon')
if (Get-Command Select-ReachableIcon -ErrorAction SilentlyContinue) {
    $c = @(
        [PSCustomObject]@{ Index = 1; Cx = 10; Cy = 10; Dist = 5.0 },
        [PSCustomObject]@{ Index = 2; Cx = 20; Cy = 20; Dist = 10.0 },
        [PSCustomObject]@{ Index = 3; Cx = 30; Cy = 30; Dist = 3.0 },
        [PSCustomObject]@{ Index = 4; Cx = 40; Cy = 40; Dist = 50.0 }
    )
    # 回傳 @{ Sel; Error; Asked }：$occByIndex 決定每個候選的遮擋結果，$lvByIndex 決定是否命中圖示清單。
    function Invoke-Select($occByIndex, $lvByIndex = @{}) {
        $asked = New-Object System.Collections.Generic.List[int]
        $res = @{ Sel = $null; Error = $null; Asked = $asked }
        try {
            $res.Sel = Select-ReachableIcon -Candidates $c -ClearPoint { param($x) $asked.Add($x.Index); $occByIndex[$x.Index] }.GetNewClosure() `
                -HitsListView { param($x) if ($lvByIndex.ContainsKey($x.Index)) { $lvByIndex[$x.Index] } else { $true } }.GetNewClosure()
        } catch { $res.Error = "$_" }
        return $res
    }
    $r = Invoke-Select @{ 3 = (New-Occ 'system' 'Shell_SystemDim'); 1 = (New-Occ 'stuck' 'Chrome_WidgetWin_1'); 2 = (New-Occ 'clear'); 4 = (New-Occ 'clear') }
    Check '挑圖示：被系統 UI／清不掉的較近候選略過，選第一個可到達的（index 2）' ($null -eq $r.Error -and $r.Sel.Chosen.Index -eq 2) "err=$($r.Error)"
    Check '挑圖示：由近到遠檢查（3 → 1 → 2），找到後不再檢查更遠的' (($r.Asked -join ',') -eq '3,1,2') ($r.Asked -join ',')
    Check '挑圖示：記錄被略過的候選與原因（2 個）' ($null -eq $r.Error -and @($r.Sel.Skipped).Count -eq 2 -and @($r.Sel.Skipped)[0].Reason -match 'Shell_SystemDim')
    $r = Invoke-Select @{ 3 = (New-Occ 'shell' 'WorkerW') }
    Check '挑圖示：命中另一扇桌面 WorkerW 且命中圖示清單 → 選中（目標本來就是桌面）' ($null -eq $r.Error -and $r.Sel.Chosen.Index -eq 3) "err=$($r.Error)"
    $r = Invoke-Select @{ 3 = (New-Occ 'host-other' 'Tauri Window'); 1 = (New-Occ 'clear') }
    Check '挑圖示：宿主視窗蓋住圖示中心 → FAIL:，不找替代圖示' ($r.Error -like 'FAIL:*' -and ($r.Asked -join ',') -eq '3') "err=$($r.Error) asked=$($r.Asked -join ',')"
    $r = Invoke-Select @{ 3 = (New-Occ 'none') }
    Check '挑圖示：沒命中任何視窗 → FAIL:' ($r.Error -like 'FAIL:*') "err=$($r.Error)"
    $r = Invoke-Select @{ 3 = (New-Occ 'clear') } @{ 3 = $false }
    Check '挑圖示：命中桌面但不是圖示清單 → FAIL:（點擊選不到圖示）' ($r.Error -like 'FAIL:*' -and $r.Error -match '圖示清單') "err=$($r.Error)"
    $r = Invoke-Select @{ 1 = (New-Occ 'system' 'Shell_SystemDim'); 2 = (New-Occ 'shell' 'Shell_TrayWnd'); 3 = (New-Occ 'stuck' 'X'); 4 = (New-Occ 'system' 'Y') }
    Check '挑圖示：全部被系統 UI／工作列擋住或清不掉 → ENV-BLOCKED:、不選' ($r.Error -like 'ENV-BLOCKED:*' -and $null -eq $r.Sel -and $r.Asked.Count -eq 4) "err=$($r.Error)"
}
$icText = Get-Content $IconClickTarget -Raw
Check 'icon-click 靜態：挑圖示經 Clear-Occluders（目標＝圖示清單所在的桌面視窗）與 Test-PointHitsListView' ($icText -match 'Select-ReachableIcon -Candidates' -and $icText -match 'Clear-Occluders -HostPid \$hostPid -TargetHwnd \$lvTop' -and $icText -match 'Test-PointHitsListView')
$catchBodies = @($icAst.FindAll({ param($n) $n -is [System.Management.Automation.Language.CatchClauseAst] }, $true) |
        ForEach-Object { ($_.Body.Extent.Text -split "`r?`n" | Where-Object { $_ -notmatch '^\s*#' }) -join "`n" })
$mainCatch = @($catchBodies | Where-Object { $_ -match 'BLOCKED' })
Check 'icon-click 靜態：BLOCKED 的 catch 內不關 writer、不 exit（交給 finally）' ($mainCatch.Count -ge 1 -and @($mainCatch | Where-Object { $_ -match '\.Close\(\)' -or $_ -match '\bexit\b' }).Count -eq 0) ($mainCatch -join ' ||| ')
$selIdx = $icText.IndexOf('$sel = Select-ReachableIcon')
$clickIdx = $icText.IndexOf('Send-GuardedClick $best.Cx $best.Cy')
Check 'icon-click 靜態：遮蔽檢查在點擊之前' ($selIdx -ge 0 -and $clickIdx -gt $selIdx) "sel=$selIdx click=$clickIdx"

# ================================================================ probe-1.3
$p13Ast = Import-Functions $Probe13Target @()
$p13Text = Get-Content $Probe13Target -Raw
$occIdx = $p13Text.IndexOf('if ($occluded) { throw "ENV-BLOCKED')
$p13Click = $p13Text.IndexOf('Send-GuardedClick $cx $cy')
Check 'probe-1.3 靜態：occluded 時在點擊之前就以 ENV-BLOCKED 中止' ($occIdx -ge 0 -and $p13Click -gt $occIdx) "occ=$occIdx click=$p13Click"
Check 'probe-1.3 靜態：ENV-BLOCKED 對應結束碼 3' ($p13Text -match "elseif \(`"\`$_`" -like 'ENV-BLOCKED\*'\) \{[^\n]*\`$exitCode = 3 \}")

Write-Host ''
Write-Host "合計：$($script:Pass) PASS、$($script:Fail) FAIL"
if ($script:Fail -gt 0) { exit 1 }
exit 0
