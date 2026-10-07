<#
.SYNOPSIS
    靜態測試（fix F9）：會注入輸入的腳本（載入 lib/SafeInput.psm1 的 host/tools/*.ps1）必須在頂層、
    取得任何座標之前，把本執行緒設成 Per-Monitor-V2 DPI 感知。

.DESCRIPTION
    失效鏈（批次 B 第二次補跑，probe-1.3）：pwsh 執行緒預設 DPI unaware（awareness 0）。探針視窗放在
    175% 的筆電螢幕上，腳本拿探針回報的實體像素座標呼叫 WindowFromPoint，系統卻以虛擬化（邏輯）座標解讀，
    所有取樣點都沒打到小工具，誤報「整個矩形被覆蓋」。GetWindowRect／WindowFromPoint／游標與點擊座標／
    Screen.AllScreens 都依「呼叫端執行緒」的 DPI 感知解讀，必須全部在同一個 awareness 下。

    規則（只解析語法樹，不執行任何腳本）：
      1. 頂層（腳本主體，不在函式、if、try、迴圈或 scriptblock 內）有一句
         `SetThreadDpiAwarenessContext([IntPtr](-4))`（DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2）。
      2. 這一句在第一個「會用到座標或注入」的頂層敘述之前。用到座標或注入＝呼叫 SafeInput 的注入入口
         與前置探查、Clear-Occluders、Start-ScratchForm，或 GetWindowRect／WindowFromPoint／GetCursorPos
         等座標 API；呼叫本檔自己定義、內部（遞移地）用到上述任一項的函式也算。
      3. 本檔有宣告 SetThreadDpiAwarenessContext（Add-Type 的 C# 宣告）。
    自我檢查對每種違規各放一個樣本，確認偵測器抓得到；正常寫法放一個樣本，確認不誤報。

.EXAMPLE
    pwsh -NoProfile -File host/tools/tests/DpiAwareness.Tests.ps1
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

$L = 'System.Management.Automation.Language'
# 會用到座標或注入的指令（SafeInput 的入口與前置探查、遮擋前提、腳本自己的表單定位）。
$script:UseCommands = @(
    'Invoke-SafeInputPreflight', 'Send-GuardedWinD', 'Send-GuardedAltTap', 'Send-GuardedClick', 'Send-GuardedWheel',
    'Set-GuardedCursorPos', 'Invoke-GuardedKey', 'Invoke-GuardedMouse', 'Invoke-GuardedTouch', 'Send-GuardedTouchDrag',
    'New-GuardedTouchDevice', 'Clear-Occluders', 'Invoke-MinimizeAppWindows', 'Start-ScratchForm'
)
# 依呼叫端執行緒 DPI 感知解讀座標的 Win32 API。
$script:UseMembers = @(
    'GetWindowRect', 'GetClientRect', 'WindowFromPoint', 'GetCursorPos', 'ClientToScreen', 'ScreenToClient',
    'MonitorFromPoint', 'GetMonitorInfoW', 'GetMonitorInfo', 'SetWindowPos', 'MoveWindow', 'AllScreens', 'PrimaryScreen'
)

function Test-IsPmV2Call($n) {
    if ($n -isnot [System.Management.Automation.Language.InvokeMemberExpressionAst]) { return $false }
    if ($n.Member -isnot [System.Management.Automation.Language.StringConstantExpressionAst]) { return $false }
    if ($n.Member.Value -ne 'SetThreadDpiAwarenessContext') { return $false }
    $a = @($n.Arguments)
    if ($a.Count -ne 1) { return $false }
    return (($a[0].Extent.Text -replace '\s', '').ToLowerInvariant() -eq '[intptr](-4)')
}

# 回傳節點所屬頂層敘述在 EndBlock.Statements 的索引；不在頂層（函式、if／try／迴圈、scriptblock 內）回傳 -1。
function Get-TopIndex($node, $root) {
    $p = $node
    while ($p.Parent) {
        $parent = $p.Parent
        if ($parent -is [System.Management.Automation.Language.NamedBlockAst]) {
            if ($parent.Parent -ne $root -or $parent -ne $root.EndBlock) { return -1 }
            return [array]::IndexOf(@($root.EndBlock.Statements), $p)
        }
        if ($parent -is [System.Management.Automation.Language.StatementBlockAst] -or
            $parent -is [System.Management.Automation.Language.FunctionDefinitionAst] -or
            $parent -is [System.Management.Automation.Language.ScriptBlockAst] -or
            $parent -is [System.Management.Automation.Language.ScriptBlockExpressionAst]) { return -1 }
        $p = $parent
    }
    return -1
}

function Test-ContainsUse($subtree, [string[]]$tainted) {
    $hit = $subtree.Find({
            param($n)
            ($n -is [System.Management.Automation.Language.CommandAst] -and ($n.GetCommandName() -in ($script:UseCommands + $tainted))) -or
            ($n -is [System.Management.Automation.Language.MemberExpressionAst] -and
            $n.Member -is [System.Management.Automation.Language.StringConstantExpressionAst] -and $n.Member.Value -in $script:UseMembers)
        }, $true)
    return ($null -ne $hit)
}

# 回傳 $null＝不適用（沒有載入 SafeInput）；否則回傳違規清單（空＝通過）。
function Get-DpiAwarenessViolations([string]$Path) {
    $tokens = $null; $errs = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseFile($Path, [ref]$tokens, [ref]$errs)
    $v = New-Object System.Collections.Generic.List[string]
    if (@($errs).Count -gt 0) { $v.Add("解析錯誤：$((@($errs) | ForEach-Object { $_.Message }) -join ' | ')"); return , $v.ToArray() }
    $loadsSafeInput = $ast.Find({ param($n) $n -is [System.Management.Automation.Language.CommandAst] -and
            $n.GetCommandName() -eq 'Import-Module' -and $n.Extent.Text -like '*SafeInput.psm1*' }, $true)
    if (-not $loadsSafeInput) { return $null }

    $declared = $ast.Find({ param($n) ($n -is [System.Management.Automation.Language.StringConstantExpressionAst] -or
                $n -is [System.Management.Automation.Language.ExpandableStringExpressionAst]) -and
            $n.Value -match 'extern\s+System\.IntPtr\s+SetThreadDpiAwarenessContext\(' }, $true)
    if (-not $declared) { $v.Add('沒有宣告 SetThreadDpiAwarenessContext（Add-Type）') }

    $calls = @($ast.FindAll({ param($n) Test-IsPmV2Call $n }, $true))
    $topIdx = @($calls | ForEach-Object { Get-TopIndex $_ $ast } | Where-Object { $_ -ge 0 } | Sort-Object)
    if ($topIdx.Count -eq 0) {
        $where = if ($calls.Count -gt 0) { "（有 $($calls.Count) 處，但都不在頂層）" } else { '' }
        $v.Add("沒有在頂層呼叫 SetThreadDpiAwarenessContext([IntPtr](-4))$where")
        return , $v.ToArray()
    }

    # 本檔自己定義、內部（遞移地）用到座標或注入的函式。
    $fns = @($ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] }, $true))
    $tainted = New-Object System.Collections.Generic.List[string]
    do {
        $added = $false
        foreach ($f in $fns) {
            if ($tainted.Contains($f.Name)) { continue }
            if (Test-ContainsUse $f.Body $tainted.ToArray()) { $tainted.Add($f.Name); $added = $true }
        }
    } while ($added)

    $stmts = @($ast.EndBlock.Statements)
    $firstUse = -1
    for ($i = 0; $i -lt $stmts.Count; $i++) {
        if ($stmts[$i] -is [System.Management.Automation.Language.FunctionDefinitionAst]) { continue }
        if (Test-ContainsUse $stmts[$i] $tainted.ToArray()) { $firstUse = $i; break }
    }
    if ($firstUse -ge 0 -and $topIdx[0] -gt $firstUse) {
        $v.Add("SetThreadDpiAwarenessContext（第 $($stmts[$topIdx[0]].Extent.StartLineNumber) 行）在第一個用到座標或注入的敘述（第 $($stmts[$firstUse].Extent.StartLineNumber) 行）之後")
    }
    return , $v.ToArray()
}

# ---------------------------------------------------------------- 全部 host/tools/*.ps1（lib／tests 不在範圍）
$scripts = @(Get-ChildItem $toolsDir -Filter *.ps1 -File)
$applicable = New-Object System.Collections.Generic.List[string]
foreach ($f in $scripts) {
    $r = Get-DpiAwarenessViolations $f.FullName
    if ($null -eq $r) { continue }
    $applicable.Add($f.Name)
    Check "$($f.Name)：頂層、取得座標之前設定 Per-Monitor-V2 DPI 感知" (@($r).Count -eq 0) (@($r) -join ' | ')
}
Check '掃描到載入 SafeInput 的注入類腳本（至少 13 支，含 probe-1.3）' ($applicable.Count -ge 13 -and $applicable.Contains('probe-1.3.ps1')) ($applicable -join ',')

# ---------------------------------------------------------------- 自我檢查：偵測器本身抓得到每一種違規（避免永遠綠）
# 路徑一律用正斜線（避免寫檔時反斜線被吃掉，見專案 memory subagent-edits-drop-single-backslashes）。
$decl = '$def = "[DllImport(""user32.dll"")] public static extern System.IntPtr SetThreadDpiAwarenessContext(System.IntPtr ctx);"'
$imp = 'Import-Module (Join-Path $PSScriptRoot "lib/SafeInput.psm1") -Force'
$set = '[void][X.Native]::SetThreadDpiAwarenessContext([IntPtr](-4))'
$pf = '$pf = Invoke-SafeInputPreflight'
$samples = [ordered]@{
    '正常：頂層、在前置探查與座標之前'     = @{ Code = @($decl, $imp, $set, $pf, '[void][X.Native]::GetWindowRect($h, [ref]$r)'); Want = 0 }
    '正常：函式定義在設定之前不算使用'     = @{ Code = @($decl, $imp, 'function F { [X.Native]::WindowFromPoint($p) }', $set, 'F'); Want = 0 }
    '缺：完全沒有設定'                     = @{ Code = @($decl, $imp, $pf); Want = 1 }
    '缺：只在函式內設定'                   = @{ Code = @($decl, $imp, "function Init { $set }", 'Init', $pf); Want = 1 }
    '缺：只在 if 區塊內設定'               = @{ Code = @($decl, $imp, "if (`$true) { $set }", $pf); Want = 1 }
    '缺：只在 try 區塊內設定'              = @{ Code = @($decl, $imp, "try { $set } finally { }", $pf); Want = 1 }
    '錯值：-3（PER_MONITOR_AWARE v1）'     = @{ Code = @($decl, $imp, '[void][X.Native]::SetThreadDpiAwarenessContext([IntPtr](-3))', $pf); Want = 1 }
    '順序：前置探查在設定之前'             = @{ Code = @($decl, $imp, $pf, $set); Want = 1 }
    '順序：GetWindowRect 在設定之前'       = @{ Code = @($decl, $imp, '[void][X.Native]::GetWindowRect($h, [ref]$r)', $set); Want = 1 }
    '順序：遞移呼叫用到座標的函式'         = @{ Code = @($decl, $imp, 'function A { [X.Native]::WindowFromPoint($p) }', 'function B { A }', '$x = B', $set); Want = 1 }
    '順序：try 內的點擊在設定之前'         = @{ Code = @($decl, $imp, 'try { Send-GuardedClick 1 2 } finally { }', $set); Want = 1 }
    '缺宣告'                               = @{ Code = @($imp, $set, $pf); Want = 1 }
}
$tmpDir = Join-Path ([IO.Path]::GetTempPath()) ("fc-dpi-{0}" -f [guid]::NewGuid().ToString('N'))
[void](New-Item -ItemType Directory -Path $tmpDir)
try {
    $enc = New-Object System.Text.UTF8Encoding($false)
    $i = 0
    foreach ($k in $samples.Keys) {
        $i++
        $f = Join-Path $tmpDir "s$i.ps1"
        [IO.File]::WriteAllText($f, ($samples[$k].Code -join "`n"), $enc)
        $r = Get-DpiAwarenessViolations $f
        $r = @($r)
        if ($samples[$k].Want -eq 0) { Check "自我檢查：$k → 0 項" ($r.Count -eq 0) ($r -join ' | ') }
        else { Check "自我檢查：$k → 抓到" ($r.Count -ge 1) '偵測器沒有回報' }
    }
    $f = Join-Path $tmpDir 'noinject.ps1'
    [IO.File]::WriteAllText($f, '[void][X.Native]::GetWindowRect($h, [ref]$r)', $enc)
    Check '自我檢查：沒有載入 SafeInput → 不適用' ($null -eq (Get-DpiAwarenessViolations $f))
} finally { Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue }

Write-Host "`n$($script:Pass) passed, $($script:Fail) failed"
if ($script:Fail -gt 0) { exit 1 }
exit 0
