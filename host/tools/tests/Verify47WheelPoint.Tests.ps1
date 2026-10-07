<#
.SYNOPSIS
    host/tools/verify-4.7-wheel.ps1 與 verify-4.7-wheelrouting.ps1 的 Resolve-WheelPoint（滾輪座標與遮擋判讀）
    測試（純 PowerShell 斷言，不需 Pester）。

.DESCRIPTION
    **不執行兩支腳本**（會啟動宿主、送滾輪）：以 PowerShell Parser 解析腳本，只把 Resolve-WheelPoint
    函式定義取出來，搭配 lib/Occluders.psm1 的真 Get-OccluderVerdict／Assert-OccluderResult，餵 mock 的
    Clear-Occluders 結果。fix F8c：原本偏好點清不掉時一律在矩形內找替代點、找不到一律判 ENV-BLOCKED，
    小工具沉到桌面之下或沒命中任何視窗會被誤判成環境問題。判讀規則須與其他驗收腳本一致：
    桌面／沒命中／宿主另一扇／腳本自己的視窗＝FAIL（不找替代點）；工作列／系統 UI／最小化後仍蓋住＝
    才找替代點，找不到＝ENV-BLOCKED。

.EXAMPLE
    pwsh -NoProfile -File host/tools/tests/Verify47WheelPoint.Tests.ps1
    結束碼 0＝全部通過、1＝有失敗。
#>
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$toolsDir = Split-Path $PSScriptRoot -Parent
Import-Module (Join-Path $toolsDir 'lib/Occluders.psm1') -Force

$script:Pass = 0
$script:Fail = 0
function Check([string]$name, [bool]$cond, [string]$detail = '') {
    if ($cond) { $script:Pass++; Write-Host "PASS  $name" }
    else { $script:Fail++; Write-Host "FAIL  $name $detail" -ForegroundColor Red }
}

# 回傳 @{ Point; Error; AltCalls }：Error 為例外訊息（沒丟例外＝$null）。
function Invoke-Resolve($Occ, $Alt) {
    $counter = @{ N = 0 }
    $finder = { $counter.N++; $Alt }.GetNewClosure()
    $res = @{ Point = $null; Error = $null; AltCalls = 0 }
    try { $res.Point = Resolve-WheelPoint -Occ $Occ -PreferX 100 -PreferY 200 -FindAlternative $finder }
    catch { $res.Error = "$_" }
    $res.AltCalls = $counter.N
    return $res
}

function New-Occ([string]$Action, [string]$Class = '') {
    [PSCustomObject]@{ Ok = $false; Action = $Action; Class = $Class; Reason = "(100,200) mock $Action $Class" }
}

foreach ($name in 'verify-4.7-wheel.ps1', 'verify-4.7-wheelrouting.ps1', 'verify-4.7-touch.ps1') {
    $tokens = $null
    $errors = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseFile((Join-Path $toolsDir $name), [ref]$tokens, [ref]$errors)
    Check "$name 解析 0 錯誤" (@($errors).Count -eq 0) (($errors | ForEach-Object { $_.Message }) -join ' | ')
    $fn = $ast.Find({
            param($n)
            $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'Resolve-WheelPoint'
        }, $true)
    Check "$name 找得到 Resolve-WheelPoint" ($null -ne $fn)
    if (-not $fn) { continue }
    . ([scriptblock]::Create($fn.Extent.Text))

    $alt = @{ X = 7; Y = 8 }

    $r = Invoke-Resolve ([PSCustomObject]@{ Ok = $true; Action = 'clear'; Class = ''; Reason = '' }) $alt
    Check "${name}：Ok → 偏好點、不找替代點" ($null -eq $r.Error -and $r.Point.X -eq 100 -and $r.Point.Y -eq 200 -and $r.AltCalls -eq 0) "err=$($r.Error) alt=$($r.AltCalls)"

    $failCases = [ordered]@{
        '命中 Progman（沉到桌面之下）' = New-Occ 'shell' 'Progman'
        '命中 WorkerW'                 = New-Occ 'shell' 'WorkerW'
        '沒命中任何視窗'               = New-Occ 'none'
        '宿主另一扇視窗'               = New-Occ 'host-other' 'Tauri Window'
        '腳本自己的視窗'               = New-Occ 'own'
        '$null 結果'                   = $null
        '缺 Action 的舊格式結果'       = [PSCustomObject]@{ Ok = $false; Reason = '舊格式' }
    }
    foreach ($k in $failCases.Keys) {
        $r = Invoke-Resolve $failCases[$k] $alt
        Check "${name}：$k → FAIL: 且不找替代點" ($null -ne $r.Error -and $r.Error -like 'FAIL:*' -and $r.AltCalls -eq 0 -and $null -eq $r.Point) "err=$($r.Error) alt=$($r.AltCalls)"
    }

    $envCases = [ordered]@{
        '工作列 Shell_TrayWnd'       = New-Occ 'shell' 'Shell_TrayWnd'
        '系統 UI（Shell_SystemDim）' = New-Occ 'system' 'Shell_SystemDim'
        '最小化後仍蓋住（stuck）'    = New-Occ 'stuck' 'Chrome_WidgetWin_1'
    }
    foreach ($k in $envCases.Keys) {
        $r = Invoke-Resolve $envCases[$k] $alt
        Check "${name}：$k 且矩形內有乾淨點 → 回替代點" ($null -eq $r.Error -and $r.Point.X -eq 7 -and $r.Point.Y -eq 8 -and $r.AltCalls -eq 1) "err=$($r.Error) alt=$($r.AltCalls)"
        $r = Invoke-Resolve $envCases[$k] $null
        Check "${name}：$k 且整個矩形都找不到 → ENV-BLOCKED:" ($null -ne $r.Error -and $r.Error -like 'ENV-BLOCKED:*' -and $r.AltCalls -eq 1 -and $null -eq $r.Point) "err=$($r.Error) alt=$($r.AltCalls)"
    }
}

Write-Host "`n$($script:Pass) passed, $($script:Fail) failed"
if ($script:Fail -gt 0) { exit 1 }
exit 0
