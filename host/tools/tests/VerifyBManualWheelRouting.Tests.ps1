<#
.SYNOPSIS
    verify-b-manual-screenshot.ps1 與 verify-4.7-wheelrouting.ps1 的 mock／靜態測試（純 PowerShell
    斷言，不需 Pester）。

.DESCRIPTION
    **不執行兩支腳本**（它們會啟動宿主、最小化使用者視窗、改滾輪路由系統設定）：以 PowerShell
    Parser 取出純函式 Get-ScreenshotReadiness 餵合成的視窗標題，其餘以語法樹做結構檢查。

    fix F4（review B-batch2）：
      - [medium] b-manual 的啟動區塊（含兩次 Start-Process）必須在「finally 會停宿主、還原開機自啟
        登錄、刪暫存目錄」的 try 之內；
      - [low] 截圖前確認小工具／設定視窗真的出現；
      - [low] wheelrouting「需要還原」旗標在 SET 之前設好、讀回不符即中止；檔頭寫明 SET 傳值。

    -BManualTarget／-WheelRoutingTarget 可指向其他版本的腳本（例如 `git show <rev>:<path>` 存出的檔案）。

.EXAMPLE
    pwsh -NoProfile -File host/tools/tests/VerifyBManualWheelRouting.Tests.ps1
    結束碼 0＝全部通過、1＝有失敗。
#>
param([string]$BManualTarget, [string]$WheelRoutingTarget)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$toolsDir = Split-Path $PSScriptRoot -Parent
if (-not $BManualTarget) { $BManualTarget = Join-Path $toolsDir 'verify-b-manual-screenshot.ps1' }
if (-not $WheelRoutingTarget) { $WheelRoutingTarget = Join-Path $toolsDir 'verify-4.7-wheelrouting.ps1' }

$script:Pass = 0
$script:Fail = 0
function Check([string]$name, [bool]$cond, [string]$detail = '') {
    if ($cond) { $script:Pass++; Write-Host "PASS  $name" }
    else { $script:Fail++; Write-Host "FAIL  $name $detail" -ForegroundColor Red }
}

function Get-Ast([string]$Path) {
    $tokens = $null; $errors = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseFile($Path, [ref]$tokens, [ref]$errors)
    Check "$(Split-Path $Path -Leaf) 解析 0 錯誤" (@($errors).Count -eq 0) (($errors | ForEach-Object { $_.Message }) -join ' | ')
    return $ast
}

# ================================================================ b-manual
$bm = Get-Ast $BManualTarget
$fn = $bm.Find({ param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'Get-ScreenshotReadiness' }, $true)
Check 'b-manual：找得到 Get-ScreenshotReadiness' ($null -ne $fn)
if ($fn) {
    . ([scriptblock]::Create($fn.Extent.Text))
    $ids = @('clock', 'macro', 'fixed', 'dynamic', 'quotes')
    $all = @($ids | ForEach-Object { "fc-host $_" })
    $r = Get-ScreenshotReadiness -Mode Layout -Titles $all -EnabledIds $ids
    Check 'b-manual 截圖前提：Layout 五扇都在 → Ready' $r.Ready $r.Reason
    $r = Get-ScreenshotReadiness -Mode Layout -Titles @($all | Where-Object { $_ -ne 'fc-host quotes' }) -EnabledIds $ids
    Check 'b-manual 截圖前提：Layout 少一扇 → 不 Ready、點名缺少的' ((-not $r.Ready) -and $r.Reason -match 'quotes') $r.Reason
    $r = Get-ScreenshotReadiness -Mode Settings -Titles $all -EnabledIds $ids
    Check 'b-manual 截圖前提：Settings 沒有設定視窗 → 不 Ready' (-not $r.Ready) $r.Reason
    $r = Get-ScreenshotReadiness -Mode Settings -Titles (@($all) + '財經桌布設定') -EnabledIds $ids
    Check 'b-manual 截圖前提：Settings 五扇＋設定視窗 → Ready' $r.Ready $r.Reason
    $r = Get-ScreenshotReadiness -Mode Layout -Titles $all -EnabledIds @()
    Check 'b-manual 截圖前提：讀不到任何啟用小工具 → 不 Ready（不是空洞通過）' (-not $r.Ready) $r.Reason
}

# 每一個 Start-Process 都必須位於某個 try 之內，且那個 try 的 finally 會停宿主（Stop-ProcessTree）、
# 還原開機自啟登錄、刪暫存目錄。
$starts = @($bm.FindAll({ param($n) $n -is [System.Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq 'Start-Process' }, $true))
Check 'b-manual：找得到 Start-Process' ($starts.Count -ge 2) "count=$($starts.Count)"
$uncovered = @()
foreach ($s in $starts) {
    $covered = $false
    $p = $s.Parent
    while ($p) {
        if ($p -is [System.Management.Automation.Language.TryStatementAst] -and $p.Finally) {
            $ft = $p.Finally.Extent.Text
            if ($ft -match 'Stop-ProcessTree' -and $ft -match 'Restore-FcHostAutostartRegistry' -and $ft -match 'Remove-Item') { $covered = $true }
        }
        $p = $p.Parent
    }
    if (-not $covered) { $uncovered += "line $($s.Extent.StartLineNumber)" }
}
Check 'b-manual：所有 Start-Process 都在「finally 會停宿主＋還原登錄＋刪暫存」的 try 內' ($uncovered.Count -eq 0) ($uncovered -join ', ')
$bmText = Get-Content $BManualTarget -Raw
$readyIdx = $bmText.IndexOf('if (-not $ready.Ready)')
$shotIdx = $bmText.IndexOf('$g.CopyFromScreen(')
Check 'b-manual：截圖之前先確認截圖前提' ($readyIdx -ge 0 -and $shotIdx -gt $readyIdx) "ready=$readyIdx shot=$shotIdx"

# ================================================================ wheelrouting
[void](Get-Ast $WheelRoutingTarget)
$wrText = Get-Content $WheelRoutingTarget -Raw
$flagIdx = $wrText.IndexOf('$routingChanged = $true')
$setIdx = $wrText.IndexOf('Set-MouseWheelRouting $MOUSEWHEEL_ROUTING_FOCUS')
Check 'wheelrouting：還原旗標在 SET 之前設好' ($flagIdx -ge 0 -and $setIdx -gt $flagIdx) "flag=$flagIdx set=$setIdx"
Check 'wheelrouting：SET 讀回不符即中止（throw）' ($wrText -match 'if \(\$confirmSet -ne \$MOUSEWHEEL_ROUTING_FOCUS\) \{ throw')
$header = [regex]::Match($wrText, '(?s)<#.*?#>').Value
Check 'wheelrouting：檔頭寫明 SET 的 pvParam 直接傳值' ($header -match 'pvParam` \*\*直接傳值本身\*\*')
Check 'wheelrouting：檔頭不再說 pvParam 指向 DWORD：0（舊的錯誤說法）' (-not ($header -match 'pvParam` 指向一個 DWORD：0'))
$setCalls = @([regex]::Matches($wrText, 'SystemParametersInfoSet\(\$SPI_SETMOUSEWHEELROUTING[^\n]*') | ForEach-Object { $_.Value })
Check 'wheelrouting：SET 一律只帶 SPIF_SENDCHANGE（不持久化）' ($setCalls.Count -ge 1 -and @($setCalls | Where-Object { $_ -notmatch '\$SPIF_SENDCHANGE\)' }).Count -eq 0) ($setCalls -join ' | ')

Write-Host ''
Write-Host "合計：$($script:Pass) PASS、$($script:Fail) FAIL"
if ($script:Fail -gt 0) { exit 1 }
exit 0
