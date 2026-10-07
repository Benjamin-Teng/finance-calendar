<#
.SYNOPSIS
    host/tools/lib/ScratchWindow.psm1（腳本自己的 WinForms「一般視窗」）的結構與純函式測試，以及
    「驗收腳本不得借用記事本」的全目錄掃描（純 PowerShell 斷言，不需 Pester）。

.DESCRIPTION
    **不開任何視窗**：只測產生表單的指令字串（Get-ScratchFormCommand）能被 PowerShell 解析、標題
    正確跳脫；以語法樹確認 Stop-ScratchForm 只會結束自己啟動的那個行程；再以 token 掃描
    host/tools 頂層所有 .ps1，程式碼（不含註解）不得出現 notepad。

    fix F7（批次 A／B）：Win11 記事本是單一行程多視窗，腳本以 Stop-Process 收尾會連使用者原本開著的
    記事本視窗一起關掉（memory：win11-notepad-single-process-multi-window）。

.EXAMPLE
    pwsh -NoProfile -File host/tools/tests/ScratchWindow.Tests.ps1
    結束碼 0＝全部通過、1＝有失敗。
#>
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$toolsDir = Split-Path $PSScriptRoot -Parent
$modPath = Join-Path $toolsDir 'lib\ScratchWindow.psm1'

$script:Pass = 0
$script:Fail = 0
function Check([string]$name, [bool]$cond, [string]$detail = '') {
    if ($cond) { $script:Pass++; Write-Host "PASS  $name" }
    else { $script:Fail++; Write-Host "FAIL  $name $detail" -ForegroundColor Red }
}

Check 'lib/ScratchWindow.psm1 存在' (Test-Path $modPath)
if (Test-Path $modPath) {
    Import-Module $modPath -Force
    foreach ($f in 'Get-ScratchFormCommand', 'Start-ScratchForm', 'Stop-ScratchForm') {
        Check "匯出 $f" ($null -ne (Get-Command $f -Module ScratchWindow -ErrorAction SilentlyContinue))
    }

    if (Get-Command Get-ScratchFormCommand -ErrorAction SilentlyContinue) {
        foreach ($title in 'fc-host-scratch-abc123', "it's a 'quoted' title") {
            $cmd = Get-ScratchFormCommand -Title $title
            $t = $null; $e = $null
            [void][System.Management.Automation.Language.Parser]::ParseInput($cmd, [ref]$t, [ref]$e)
            Check "表單指令可解析（標題：$title）" (@($e).Count -eq 0) (($e | ForEach-Object { $_.Message }) -join ' | ')
            $titleTok = @($t | Where-Object { $_.Kind -eq 'StringLiteral' -and $_.Value -eq $title })
            Check "表單指令以字串常值帶入完整標題（$title）" ($titleTok.Count -ge 1)
        }
        $cmd = Get-ScratchFormCommand -Title 'x'
        Check '表單含可取得鍵盤焦點的 TextBox' ($cmd -match 'TextBox')
        Check '表單以 Application.Run 執行訊息迴圈' ($cmd -match 'Application\]::Run')
    }

    # Stop-ScratchForm 只能結束 $Form.Process（自己啟動的 pwsh），不得以名稱或其他 PID 結束行程。
    $ast = [System.Management.Automation.Language.Parser]::ParseFile($modPath, [ref]$null, [ref]$null)
    $stopFn = $ast.Find({ param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'Stop-ScratchForm' }, $true)
    Check '找得到 Stop-ScratchForm 定義' ($null -ne $stopFn)
    if ($stopFn) {
        $kills = @($stopFn.FindAll({
                    param($n)
                    $n -is [System.Management.Automation.Language.CommandAst] -and
                    $n.GetCommandName() -in 'Stop-Process', 'Stop-ProcessTree', 'taskkill', 'taskkill.exe'
                }, $true))
        $bad = @($kills | Where-Object { $_.Extent.Text -notmatch '\$Form\.Process\b' -or $_.Extent.Text -match '-Name\b' })
        Check 'Stop-ScratchForm 的結束行程呼叫都只針對 $Form.Process' ($bad.Count -eq 0) (($bad | ForEach-Object { $_.Extent.Text }) -join ' | ')
        Check 'Stop-ScratchForm 先送 WM_CLOSE（0x10）給自己的視窗' ($stopFn.Extent.Text -match '0x10' -and $stopFn.Extent.Text -match 'PostMessage')
    }
}

# 全目錄掃描：host/tools 頂層 .ps1 的程式碼（不含註解）不得出現 notepad。
foreach ($f in Get-ChildItem -Path $toolsDir -Filter '*.ps1' -File) {
    $t = $null; $e = $null
    [void][System.Management.Automation.Language.Parser]::ParseFile($f.FullName, [ref]$t, [ref]$e)
    $hits = @($t | Where-Object { $_.Kind -ne 'Comment' -and $_.Text -match '(?i)notepad' })
    Check "$($f.Name)：程式碼不借用記事本" ($hits.Count -eq 0) (($hits | Select-Object -First 3 | ForEach-Object { "第 $($_.Extent.StartLineNumber) 行 $($_.Text)" }) -join ' | ')
}

Write-Host "`n$($script:Pass) passed, $($script:Fail) failed"
if ($script:Fail -gt 0) { exit 1 }
exit 0
