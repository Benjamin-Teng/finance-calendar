<#
.SYNOPSIS
    靜態測試（fix F8）：host/tools/*.ps1 不得自行處理遮擋或自行最小化視窗——一律經 lib/Occluders.psm1
    （白名單：只動一般應用程式主視窗；系統 UI、對話框、殼層等不動；還原含吸附與最大化）。

.DESCRIPTION
    批次 B 補跑的失效鏈：dragdpi-esc、7.6、5.3、7.7 各自內建一份沒有白名單的 Clear-Occluders，
    會把「Windows 安全性」對話框的全螢幕 Shell_SystemDim 最小化。本測試只解析語法樹、不執行任何腳本：
      1. 不得定義 Clear-Occluders／Restore-Occluders／Get-OccluderAction／Get-OccluderVerdict／
         Assert-OccluderResult（lib/ 的 .psm1 不在掃描範圍）。
      2. ShowWindow／ShowWindowAsync 只准直接呼叫、兩個參數，且 nCmdShow 是允許清單內的整數字面值
         （1／4／5／8／9）或具名常數（SW_SHOWNORMAL、SW_NORMAL、SW_SHOWNOACTIVATE、SW_SHOW、SW_SHOWNA、
         SW_RESTORE，且本檔每次賦值都是正確值、不是參數）；變數、運算式、字串、間接引用（.Invoke）、
         動態成員名稱一律違規（fix F8b：原本只擋 0／2／6／7／11 與名稱含 MIN 的變數，換個變數名就繞過）。
      3. 程式碼（不含註解；含字串）不得出現 CloseWindow（Win32 的 CloseWindow 是最小化）、SC_MINIMIZE、
         0xF020、61472、MinimizeAll、ToggleDesktop、SetWindowVisualState、SetWindowPlacement、
         以 EntryPoint 把 ShowWindow 改名宣告。
      4. 不得進入 Occluders 的模組範圍（& 或 . 加 scriptblock、Get-Module Occluders、Import-Module -PassThru、
         dot-source 模組檔、NewBoundScriptBlock／SessionState、InModuleScope）、不得呼叫未匯出的函式，
         也不得對 Clear-Occluders／Invoke-MinimizeAppWindows／Restore-Occluders 傳測試用注入參數或 splat。
      5. Occluders 的匯出清單只含經白名單的入口與純函式（Invoke-MinimizeWindow 不匯出）。
      6. 呼叫 Clear-Occluders 的腳本必須匯入 lib/Occluders.psm1，且在 finally 呼叫 Restore-Occluders；
         本次改寫的腳本以 lib 的 Assert-OccluderResult 判讀（ENV-BLOCKED 以結束碼 3 結束）。
    自我檢查對上述每一種違規各放一個樣本，確認偵測器抓得到；正常寫法放一個樣本，確認不誤報。

.EXAMPLE
    pwsh -NoProfile -File host/tools/tests/NoInlineMinimize.Tests.ps1
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

# fix F8b：ShowWindow／ShowWindowAsync 的 nCmdShow 改為「允許清單」——只准顯示／還原類的具名常數或整數字面值。
# 具名常數（變數）必須在本檔被賦值、且每一次賦值都是該名稱的正確值；變數、運算式、字串、參數一律違規。
$script:AllowedShowCmds = @{ SW_SHOWNORMAL = 1; SW_NORMAL = 1; SW_SHOWNOACTIVATE = 4; SW_SHOW = 5; SW_SHOWNA = 8; SW_RESTORE = 9 }
$script:AllowedShowValues = @(1, 4, 5, 8, 9)
$script:Lang = 'System.Management.Automation.Language'
function Get-PlainVarName($varAst) { return ($varAst.VariablePath.UserPath -replace '^(script|global|local|private):', '') }
function Get-IntLiteral([string]$text) {
    $t = $text.Trim()
    if ($t -match '^0x[0-9a-fA-F]+$') { return [Convert]::ToInt64($t.Substring(2), 16) }
    if ($t -match '^[0-9]+$') { return [int64]$t }
    return $null
}
# 回傳空字串＝允許；否則回傳違規原因。
function Get-ShowCmdViolation($arg, $ast) {
    if ($arg -is [System.Management.Automation.Language.ConstantExpressionAst] -and
        $arg -isnot [System.Management.Automation.Language.StringConstantExpressionAst]) {
        $n = Get-IntLiteral $arg.Extent.Text
        if ($null -ne $n -and $n -in $script:AllowedShowValues) { return '' }
        return "字面值 $($arg.Extent.Text) 不在允許清單（1／4／5／8／9）"
    }
    if ($arg -is [System.Management.Automation.Language.VariableExpressionAst]) {
        $name = Get-PlainVarName $arg
        if (-not $script:AllowedShowCmds.ContainsKey($name)) { return "變數 `$$name 不是允許清單內的具名常數" }
        $want = $script:AllowedShowCmds[$name]
        $params = @($ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.ParameterAst] -and (Get-PlainVarName $n.Name) -eq $name }, $true))
        if ($params.Count -gt 0) { return "具名常數 `$$name 是參數（值由呼叫端決定）" }
        $asg = @($ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.AssignmentStatementAst] -and
                    $n.Left -is [System.Management.Automation.Language.VariableExpressionAst] -and (Get-PlainVarName $n.Left) -eq $name }, $true))
        if ($asg.Count -eq 0) { return "具名常數 `$$name 沒有在本檔賦值" }
        foreach ($a in $asg) {
            $val = if ($a.Operator -eq [System.Management.Automation.Language.TokenKind]::Equals) { Get-IntLiteral $a.Right.Extent.Text } else { $null }
            if ($null -eq $val -or $val -ne $want) { return "具名常數 `$$name 的賦值不是 $want（第 $($a.Extent.StartLineNumber) 行：$($a.Extent.Text)）" }
        }
        return ''
    }
    return "第二參數不是允許清單內的具名常數或字面值（$($arg.Extent.Text)）"
}

# 匯入後在模組範圍執行程式碼、或繞過匯出清單的寫法（fix F8b）。
$script:NonExported = 'Invoke-MinimizeWindow', 'Invoke-RestoreWindow', 'Get-WindowInfo', 'Get-RootAtPoint', 'Get-ShellPid'
$script:OccluderEntry = 'Clear-Occluders', 'Invoke-MinimizeAppWindows', 'Restore-Occluders'
$script:InjectionParams = 'HitTest', 'Minimize', 'Describe', 'ShellPid', 'Backend', 'Restore'

function Get-OccluderViolations([string]$Path) {
    $tokens = $null; $errs = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseFile($Path, [ref]$tokens, [ref]$errs)
    $v = New-Object System.Collections.Generic.List[string]
    if (@($errs).Count -gt 0) { $v.Add("解析錯誤：$((@($errs) | ForEach-Object { $_.Message }) -join ' | ')") }
    foreach ($fn in $ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] }, $true)) {
        if ($fn.Name -in 'Clear-Occluders', 'Restore-Occluders', 'Get-OccluderAction', 'Get-OccluderVerdict', 'Assert-OccluderResult') { $v.Add("第 $($fn.Extent.StartLineNumber) 行自行定義 $($fn.Name)") }
    }
    # 1. ShowWindow／ShowWindowAsync：只准直接呼叫、兩個參數、第二參數在允許清單內。
    foreach ($m in $ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.MemberExpressionAst] }, $true)) {
        $line = $m.Extent.StartLineNumber
        if ($m.Member -isnot [System.Management.Automation.Language.StringConstantExpressionAst]) {
            if ($m -is [System.Management.Automation.Language.InvokeMemberExpressionAst]) { $v.Add("第 $line 行以動態成員名稱呼叫方法（無法驗證）：$($m.Extent.Text)") }
            continue
        }
        $member = $m.Member.Value
        if ($member -notin 'ShowWindow', 'ShowWindowAsync') { continue }
        if ($m -isnot [System.Management.Automation.Language.InvokeMemberExpressionAst]) { $v.Add("第 $line 行間接引用 $member（只准直接呼叫）：$($m.Parent.Extent.Text)"); continue }
        $args_ = @($m.Arguments)
        if ($args_.Count -ne 2) { $v.Add("第 $line 行呼叫 $member 的參數不是兩個（無法驗證 nCmdShow）：$($m.Extent.Text)"); continue }
        $why = Get-ShowCmdViolation $args_[1] $ast
        if ($why) { $v.Add("第 $line 行呼叫 $member 的 nCmdShow 不允許（$why）：$($m.Extent.Text)") }
    }
    # 2. 程式碼（不含註解；含字串，例如 Add-Type 的 C# 宣告）不得出現的最小化 API／訊息／別名。
    $code = (@($tokens) | Where-Object { $_.Kind -ne [System.Management.Automation.Language.TokenKind]::Comment } | ForEach-Object { $_.Text }) -join ' '
    $forbidden = [ordered]@{
        'CloseWindow'          = '\bCloseWindow\b'
        'SC_MINIMIZE'          = 'SC_MINIMIZE'
        '0xF020'               = '\b0x0*F020\b'
        '61472'                = '\b61472\b'
        'MinimizeAll'          = '\bMinimizeAll\b'
        'ToggleDesktop'        = '\bToggleDesktop\b'
        'SetWindowVisualState' = '\bSetWindowVisualState\b'
        'SetWindowPlacement'   = '\bSetWindowPlacement\b'
        'EntryPoint'           = 'EntryPoint\s*=\s*"*ShowWindow'
    }
    foreach ($k in $forbidden.Keys) {
        if ($code -match $forbidden[$k]) { $v.Add("程式碼含最小化 API／訊息／別名 $k：$($Matches[0])") }
    }
    # 3. 不得在 Occluders 的模組範圍執行程式碼、不得呼叫未匯出函式、不得對入口注入測試用參數。
    foreach ($c in $ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.CommandAst] }, $true)) {
        $line = $c.Extent.StartLineNumber
        $els = @($c.CommandElements)
        $name = $c.GetCommandName()
        $op = $c.InvocationOperator
        if ($op -in [System.Management.Automation.Language.TokenKind]::Ampersand, [System.Management.Automation.Language.TokenKind]::Dot) {
            if ($els.Count -ge 2 -and $els[0] -isnot [System.Management.Automation.Language.ScriptBlockExpressionAst] -and
                $els[1] -is [System.Management.Automation.Language.ScriptBlockExpressionAst]) {
                $v.Add("第 $line 行以 & 或 . 在模組範圍執行程式碼：$($c.Extent.Text)")
            }
            if ($els[0].Extent.Text -match 'Occluders\.psm1') { $v.Add("第 $line 行 dot-source／直接執行 Occluders 模組檔：$($c.Extent.Text)") }
        }
        if ($name -eq 'Get-Module' -and $c.Extent.Text -match 'Occluders') { $v.Add("第 $line 行取得 Occluders 模組物件（可進入模組範圍）：$($c.Extent.Text)") }
        if ($name -eq 'Import-Module' -and $c.Extent.Text -match 'Occluders' -and $c.Extent.Text -match '-PassThru') { $v.Add("第 $line 行以 -PassThru 取得 Occluders 模組物件：$($c.Extent.Text)") }
        if ($name -eq 'InModuleScope') { $v.Add("第 $line 行 InModuleScope（進入模組範圍）：$($c.Extent.Text)") }
        if ($name -in $script:NonExported) { $v.Add("第 $line 行呼叫 Occluders 未匯出的函式 $name：$($c.Extent.Text)") }
        if ($name -in $script:OccluderEntry) {
            foreach ($e in $els) {
                if ($e -is [System.Management.Automation.Language.CommandParameterAst] -and $e.ParameterName -in $script:InjectionParams) {
                    $v.Add("第 $line 行對 $name 注入測試用參數 -$($e.ParameterName)（只准 tests/ 使用）")
                }
                if ($e -is [System.Management.Automation.Language.VariableExpressionAst] -and $e.Splatted) {
                    $v.Add("第 $line 行對 $name 以 splat 傳參數（無法驗證有沒有注入）：$($c.Extent.Text)")
                }
            }
        }
    }
    foreach ($m in $ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.MemberExpressionAst] -and
                $n.Member -is [System.Management.Automation.Language.StringConstantExpressionAst] -and $n.Member.Value -in 'NewBoundScriptBlock', 'SessionState' }, $true)) {
        $v.Add("第 $($m.Extent.StartLineNumber) 行經 PSModuleInfo 進入模組範圍（$($m.Member.Value)）：$($m.Extent.Text)")
    }
    return [PSCustomObject]@{ Ast = $ast; Violations = $v.ToArray() }
}

# ---------------------------------------------------------------- 1–3：全部 host/tools/*.ps1（lib／tests 不在範圍）
$scripts = @(Get-ChildItem $toolsDir -Filter *.ps1 -File)
Check '掃描到 host/tools/*.ps1' ($scripts.Count -ge 20) "got=$($scripts.Count)"
$parsed = @{}
foreach ($f in $scripts) {
    $r = Get-OccluderViolations $f.FullName
    $parsed[$f.Name] = $r.Ast
    Check "$($f.Name)：不自行處理遮擋、不自行最小化視窗" ($r.Violations.Count -eq 0) ($r.Violations -join ' | ')
}

# ---------------------------------------------------------------- fix F8c：AST 判讀（取代字串比對）
# ① 腳本不得自己丟「ENV-BLOCKED: …遮擋…」：任何引號形式（單引號、雙引號、here-string，皆為 StringConstant／
#    ExpandableString 節點）都算；唯一例外是 Resolve-WheelPoint 內 Assert-OccluderResult 之後的保險 throw。
function Get-EnvBlockedOccluderThrows($Ast) {
    $isStr = { param($n) $n -is [System.Management.Automation.Language.StringConstantExpressionAst] -or $n -is [System.Management.Automation.Language.ExpandableStringExpressionAst] }
    $out = New-Object System.Collections.Generic.List[string]
    foreach ($t in $Ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.ThrowStatementAst] }, $true)) {
        $p = $t.Parent; $inSafe = $false
        while ($p) { if ($p -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $p.Name -eq 'Resolve-WheelPoint') { $inSafe = $true }; $p = $p.Parent }
        if ($inSafe) { continue }
        foreach ($s in $t.FindAll({ param($n) (& $isStr $n) }, $true)) {
            if ($s.Value -match 'ENV-BLOCKED:[\s\S]*遮擋') { $out.Add("第 $($t.Extent.StartLineNumber) 行自己丟 ENV-BLOCKED 遮擋：$($t.Extent.Text)"); break }
        }
    }
    return $out.ToArray()
}
# ② 先清遮擋、後取前景基準：Clear-Occluders 與 Resolve-WheelPoint 的呼叫位置都要在第一次對 $baseFg 賦值之前
#    （最小化遮擋者可能正是前景視窗，順序反了會讓「未搶走前景」誤判 FAIL）。
function Get-ClearBeforeFgViolations($Ast) {
    $out = New-Object System.Collections.Generic.List[string]
    $cmd = { param($name) @($Ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq $name }, $true) | Sort-Object { $_.Extent.StartOffset }) }
    $fg = @($Ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.AssignmentStatementAst] -and
                $n.Left -is [System.Management.Automation.Language.VariableExpressionAst] -and $n.Left.VariablePath.UserPath -eq 'baseFg' }, $true) |
            Sort-Object { $_.Extent.StartOffset })
    if ($fg.Count -eq 0) { $out.Add('找不到 $baseFg 賦值'); return $out.ToArray() }
    foreach ($name in 'Clear-Occluders', 'Resolve-WheelPoint') {
        $c = @(& $cmd $name)
        if ($c.Count -eq 0) { $out.Add("找不到 $name 呼叫"); continue }
        if ($c[0].Extent.StartOffset -gt $fg[0].Extent.StartOffset) { $out.Add("$name（第 $($c[0].Extent.StartLineNumber) 行）在 `$baseFg 賦值（第 $($fg[0].Extent.StartLineNumber) 行）之後") }
    }
    return $out.ToArray()
}
foreach ($name in 'verify-4.7-wheel.ps1', 'verify-4.7-wheelrouting.ps1', 'verify-4.7-touch.ps1') {
    $v = @(Get-ClearBeforeFgViolations $parsed[$name])
    Check "$name：先 Clear-Occluders／Resolve-WheelPoint、後取前景基準 `$baseFg" ($v.Count -eq 0) ($v -join ' | ')
}
$pi = { param($code) [System.Management.Automation.Language.Parser]::ParseInput($code, [ref]$null, [ref]$null) }
Check '自我檢查：順序正確樣本 → 0 項' (@(Get-ClearBeforeFgViolations (& $pi '$o = Clear-Occluders -Minimized $m; $c = Resolve-WheelPoint -Occ $o; $baseFg = 1')).Count -eq 0)
Check '自我檢查：Clear-Occluders 在 $baseFg 之後 → 抓到' (@(Get-ClearBeforeFgViolations (& $pi '$baseFg = 1; $o = Clear-Occluders -Minimized $m; $c = Resolve-WheelPoint -Occ $o')).Count -ge 1)
Check '自我檢查：Resolve-WheelPoint 在 $baseFg 之後 → 抓到' (@(Get-ClearBeforeFgViolations (& $pi '$o = Clear-Occluders -Minimized $m; $baseFg = 1; $c = Resolve-WheelPoint -Occ $o')).Count -ge 1)
Check '自我檢查：缺 Clear-Occluders → 抓到' (@(Get-ClearBeforeFgViolations (& $pi '$baseFg = 1')).Count -ge 1)
$sq = "throw 'ENV-BLOCKED: 整個矩形遮擋'"
$dq = 'throw "ENV-BLOCKED: 整個矩形遮擋 $x"'
$hs = "throw @'`nENV-BLOCKED: 整個矩形遮擋`n'@"
Check '自我檢查：單引號 ENV-BLOCKED 遮擋 throw → 抓到' (@(Get-EnvBlockedOccluderThrows (& $pi $sq)).Count -eq 1)
Check '自我檢查：雙引號 ENV-BLOCKED 遮擋 throw → 抓到' (@(Get-EnvBlockedOccluderThrows (& $pi $dq)).Count -eq 1)
Check '自我檢查：here-string ENV-BLOCKED 遮擋 throw → 抓到' (@(Get-EnvBlockedOccluderThrows (& $pi $hs)).Count -eq 1)
Check '自我檢查：Resolve-WheelPoint 內的保險 throw 不算' (@(Get-EnvBlockedOccluderThrows (& $pi "function Resolve-WheelPoint { $sq }")).Count -eq 0)
Check '自我檢查：與遮擋無關的 ENV-BLOCKED throw 不算' (@(Get-EnvBlockedOccluderThrows (& $pi "throw 'ENV-BLOCKED: 合成輸入不生效'")).Count -eq 0)

# ---------------------------------------------------------------- 4：呼叫端必須用共用模組並在 finally 還原
function Test-InFinally($node) {
    $p = $node.Parent
    while ($p) {
        if ($p -is [System.Management.Automation.Language.TryStatementAst] -and $p.Finally -and
            $node.Extent.StartOffset -ge $p.Finally.Extent.StartOffset -and $node.Extent.EndOffset -le $p.Finally.Extent.EndOffset) { return $true }
        $p = $p.Parent
    }
    return $false
}
foreach ($name in $parsed.Keys) {
    $ast = $parsed[$name]
    $uses = $ast.Find({ param($n) $n -is [System.Management.Automation.Language.CommandAst] -and $n.GetCommandName() -in 'Clear-Occluders', 'Invoke-MinimizeAppWindows' }, $true)
    if (-not $uses) { continue }
    Check "$name：匯入 lib/Occluders.psm1" ($ast.Extent.Text -match 'lib\\Occluders\.psm1')
    $restore = @($ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq 'Restore-Occluders' }, $true))
    Check "$name：在 finally 呼叫 Restore-Occluders" (@($restore | Where-Object { Test-InFinally $_ }).Count -ge 1)
}

# review 31c7c53 low：Get-VerdictExitCode 的 -EnvBlocked 綁定必須是變數或運算式，且其中至少一個變數
# 在「body 含 ENV-BLOCKED* 的 catch」內被賦值（＝catch 接到 ENV-BLOCKED 時會走到帶 -EnvBlocked 的那一行）。
# -EnvBlocked:$false／$true 等常數綁定、裸 -EnvBlocked 開關、綁到 catch 沒碰過的變數，一律不算。
function Test-EnvBlockedBoundToCatch($Ast) {
    $catchVars = New-Object System.Collections.Generic.HashSet[string]
    foreach ($c in $Ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.CatchClauseAst] -and $n.Body.Extent.Text -match 'ENV-BLOCKED\*' }, $true)) {
        foreach ($a in $c.Body.FindAll({ param($n) $n -is [System.Management.Automation.Language.AssignmentStatementAst] -and $n.Left -is [System.Management.Automation.Language.VariableExpressionAst] }, $true)) {
            [void]$catchVars.Add((Get-PlainVarName $a.Left))
        }
    }
    foreach ($cmd in $Ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq 'Get-VerdictExitCode' }, $true)) {
        foreach ($e in $cmd.CommandElements) {
            if ($e -isnot [System.Management.Automation.Language.CommandParameterAst] -or $e.ParameterName -ne 'EnvBlocked' -or $null -eq $e.Argument) { continue }
            $vars = @($e.Argument.FindAll({ param($n) $n -is [System.Management.Automation.Language.VariableExpressionAst] }, $true) |
                    ForEach-Object { Get-PlainVarName $_ } | Where-Object { $_ -notin 'true', 'false', 'null' })
            if (@($vars | Where-Object { $catchVars.Contains($_) }).Count -gt 0) { return $true }
            # catch 不賦值任何變數（例如 hittest：catch 回傳 BLOCKED 判定，結果由別處彙整）時退而求其次：只排除常數。
            if ($catchVars.Count -eq 0 -and $vars.Count -gt 0) { return $true }
        }
    }
    return $false
}
$tb = { param($code) [System.Management.Automation.Language.Parser]::ParseInput($code, [ref]$null, [ref]$null) }
$cc = 'try { x } catch { if ("$_" -like ''ENV-BLOCKED*'') { $eb = "$_" } }' + "`n"
Check '自我檢查：-EnvBlocked:([bool]$eb) 且 catch 賦值 $eb → 通過' (Test-EnvBlockedBoundToCatch (& $tb ($cc + 'exit (Get-VerdictExitCode -EnvBlocked:([bool]$eb))')))
Check '自我檢查：-EnvBlocked:($stop -eq 3) 且 catch 賦值 $stop → 通過' (Test-EnvBlockedBoundToCatch (& $tb ('try { x } catch { if ("$_" -like ''ENV-BLOCKED*'') { $stop = 3 } }' + "`n" + 'exit (Get-VerdictExitCode -EnvBlocked:($stop -eq 3))')))
Check '自我檢查：-EnvBlocked:$false 常數 → 抓到' (-not (Test-EnvBlockedBoundToCatch (& $tb ($cc + 'exit (Get-VerdictExitCode -EnvBlocked:$false)'))))
Check '自我檢查：-EnvBlocked:$true 常數 → 抓到' (-not (Test-EnvBlockedBoundToCatch (& $tb ($cc + 'exit (Get-VerdictExitCode -EnvBlocked:$true)'))))
Check '自我檢查：裸 -EnvBlocked 開關 → 抓到' (-not (Test-EnvBlockedBoundToCatch (& $tb ($cc + 'exit (Get-VerdictExitCode -EnvBlocked)'))))
Check '自我檢查：綁到 catch 沒賦值的變數 → 抓到' (-not (Test-EnvBlockedBoundToCatch (& $tb ($cc + 'exit (Get-VerdictExitCode -EnvBlocked:([bool]$other))'))))
$converted = 'verify-5.3.ps1', 'verify-7.5-edit-move.ps1', 'verify-7.6-edit-resize.ps1', 'verify-7.7-aero-snap.ps1', 'verify-dragdpi-esc.ps1', 'probe-7.6-resize.ps1',
'verify-4.7-wheel.ps1', 'verify-4.7-wheelrouting.ps1', 'verify-4.7-touch.ps1',
# fix F9：目標本來就是桌面的兩支（圖示點、點穿取樣點），判讀帶 -TargetIsDesktop。
'verify-6.1-icon-click.ps1', 'verify-6.1-appearance-hittest.ps1'
foreach ($name in $converted) {
    $ast = $parsed[$name]
    Check "$name：存在且已解析" ($null -ne $ast)
    if ($null -eq $ast) { continue }
    $text = $ast.Extent.Text
    $clear = @($ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq 'Clear-Occluders' }, $true))
    Check "$name：改呼叫共用 Clear-Occluders（具名參數 -Minimized）" ($clear.Count -ge 1 -and @($clear | Where-Object { $_.Extent.Text -notmatch '-Minimized\s+\$minimized' }).Count -eq 0) "calls=$($clear.Count)"
    # fix F8b：判讀一律經 lib 的 Assert-OccluderResult（命中桌面／沒命中＝FAIL，工作列與系統 UI＝ENV-BLOCKED），
    # 腳本不得自己把 Ok=False 一律丟成 ENV-BLOCKED。
    $asserts = @($ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq 'Assert-OccluderResult' }, $true))
    Check "$name：每個 Clear-Occluders 結果都經 lib 的 Assert-OccluderResult 判讀" ($asserts.Count -ge $clear.Count -and $asserts.Count -ge 1) "clear=$($clear.Count) assert=$($asserts.Count)"
    $envThrows = @(Get-EnvBlockedOccluderThrows $ast)
    Check "$name：不自己把遮擋結果丟成 ENV-BLOCKED（AST：單引號、雙引號、here-string 皆算）" ($envThrows.Count -eq 0) ($envThrows -join ' | ')
    # review 31c7c53：結束碼可改經 lib/VerifyVerdict 的 Get-VerdictExitCode（-EnvBlocked → 3）。
    # review 31c7c53 low：-EnvBlocked 的值必須是由「接到 ENV-BLOCKED 的 catch」賦值的變數／運算式，常數（-EnvBlocked:$false）不算。
    Check "$name：catch 認得 ENV-BLOCKED 並以結束碼 3 結束" (($text -match "-like 'ENV-BLOCKED\*'") -and (($text -match 'exit 3') -or (Test-EnvBlockedBoundToCatch $ast)))
}
# fix F9：目標是桌面的腳本，每個 Assert-OccluderResult／Get-OccluderVerdict 都要帶 -TargetIsDesktop
# （不帶就套用「小工具目標」規則：命中桌面＝FAIL，圖示點與點穿取樣點會全部誤判）。
foreach ($name in 'verify-6.1-icon-click.ps1', 'verify-6.1-appearance-hittest.ps1') {
    $ast = $parsed[$name]
    if ($null -eq $ast) { continue }
    $judge = @($ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.CommandAst] -and $n.GetCommandName() -in 'Assert-OccluderResult', 'Get-OccluderVerdict' }, $true))
    $noSwitch = @($judge | Where-Object { -not ($_.CommandElements | Where-Object { $_ -is [System.Management.Automation.Language.CommandParameterAst] -and $_.ParameterName -eq 'TargetIsDesktop' }) })
    Check "$name：遮擋判讀都帶 -TargetIsDesktop（目標是桌面）" ($judge.Count -ge 1 -and $noSwitch.Count -eq 0) (($noSwitch | ForEach-Object { "第 $($_.Extent.StartLineNumber) 行" }) -join '、')
}
$bm = $parsed['verify-b-manual-screenshot.ps1']
Check 'verify-b-manual-screenshot.ps1：全螢幕最小化改用 Invoke-MinimizeAppWindows' ($null -ne $bm -and $null -ne $bm.Find({ param($n) $n -is [System.Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq 'Invoke-MinimizeAppWindows' }, $true))

# ---------------------------------------------------------------- 5（fix F8b）：Occluders 只匯出經白名單的入口
# 不經白名單的最小化原語（Invoke-MinimizeWindow）與還原原語（Invoke-RestoreWindow，可注入 -Backend）都不匯出；
# 匯出清單必須是下列子集，新增匯出要在這裡逐一審過。
$occPath = Join-Path $toolsDir 'lib/Occluders.psm1'
$occMod = Import-Module $occPath -Force -PassThru
$allowedExports = 'Get-OccluderAction', 'Get-NotMinimizableReason', 'Clear-Occluders', 'Restore-Occluders', 'Invoke-MinimizeAppWindows',
'Get-OccluderVerdict', 'Assert-OccluderResult'
$exported = @($occMod.ExportedFunctions.Keys)
Check 'Occluders 不匯出 Invoke-MinimizeWindow（不經白名單的最小化原語）' ($exported -notcontains 'Invoke-MinimizeWindow') ($exported -join ',')
Check 'Occluders 匯出清單只含經白名單的入口與純函式' (@($exported | Where-Object { $_ -notin $allowedExports }).Count -eq 0) ($exported -join ',')
Remove-Module Occluders -ErrorAction SilentlyContinue

# ---------------------------------------------------------------- 自我檢查：偵測器本身抓得到每一種違規（避免永遠綠）
# 每個違規樣本各寫成一個暫存檔，斷言偵測器至少回報一項、且回報內容含預期關鍵字；正常寫法放同一檔，斷言 0 項。
# 路徑一律用正斜線（避免寫檔時反斜線被吃掉，見專案 memory subagent-edits-drop-single-backslashes）。
$badSamples = [ordered]@{
    '自行定義 Clear-Occluders'                  = @{ Code = 'function Clear-Occluders([int]$HostPid) { }'; Want = '自行定義' }
    '自行定義 Assert-OccluderResult'            = @{ Code = 'function Assert-OccluderResult($Occ) { }'; Want = '自行定義' }
    'ShowWindowAsync(h, 6) SW_MINIMIZE'         = @{ Code = '[void][X.Native]::ShowWindowAsync($h, 6)'; Want = 'nCmdShow' }
    'ShowWindow(h, 0) SW_HIDE'                  = @{ Code = '[void][X.Native]::ShowWindow($h, 0)'; Want = 'nCmdShow' }
    'ShowWindow(h, 2) SW_SHOWMINIMIZED'         = @{ Code = '[void][X.Native]::ShowWindow($h, 2)'; Want = 'nCmdShow' }
    'ShowWindow(h, 7) SW_SHOWMINNOACTIVE'       = @{ Code = '[void][X.Native]::ShowWindow($h, 7)'; Want = 'nCmdShow' }
    'ShowWindow(h, 11) SW_FORCEMINIMIZE'        = @{ Code = '[void][X.Native]::ShowWindow($h, 11)'; Want = 'nCmdShow' }
    '名稱不含 MIN 的變數 $cmd = 6'              = @{ Code = "`$cmd = 6`n[void][X.Native]::ShowWindowAsync(`$h, `$cmd)"; Want = 'nCmdShow' }
    '允許清單名稱但值不對 $SW_RESTORE = 6'      = @{ Code = "`$SW_RESTORE = 6`n[void][X.Native]::ShowWindow(`$h, `$SW_RESTORE)"; Want = 'nCmdShow' }
    '允許清單名稱但沒有賦值'                    = @{ Code = '[void][X.Native]::ShowWindow($h, $SW_RESTORE)'; Want = 'nCmdShow' }
    '允許清單名稱來自參數'                      = @{ Code = "function F(`$SW_RESTORE = 9) { [void][X.Native]::ShowWindow(`$h, `$SW_RESTORE) }"; Want = 'nCmdShow' }
    '運算式 (3+3)'                              = @{ Code = '[void][X.Native]::ShowWindow($h, (3 + 3))'; Want = 'nCmdShow' }
    '字串常數 "6"'                              = @{ Code = '[void][X.Native]::ShowWindow($h, "6")'; Want = 'nCmdShow' }
    '參數個數不是 2（splat 陣列）'              = @{ Code = '[void][X.Native]::ShowWindow($argsArray)'; Want = 'nCmdShow' }
    '間接引用 ShowWindow.Invoke'                = @{ Code = '[void][X.Native]::ShowWindow.Invoke($h, 9)'; Want = '間接引用' }
    '動態成員名稱'                              = @{ Code = "`$fn = `"ShowWindow`"`n[void][X.Native]::`$fn(`$h, 6)"; Want = '動態成員' }
    'EntryPoint 別名'                           = @{ Code = '$def = "[DllImport(""user32.dll"", EntryPoint = ""ShowWindowAsync"")] public static extern bool Hide(System.IntPtr h, int c);"'; Want = 'EntryPoint' }
    'WM_SYSCOMMAND 十進位 61472'                = @{ Code = '[void][X.Native]::PostMessage($h, 0x112, 61472, 0)'; Want = '61472' }
    'SC_MINIMIZE 十六進位 0xF020'               = @{ Code = '[void][X.Native]::SendMessage($h, 0x112, 0xF020, 0)'; Want = '0xF020' }
    'SC_MINIMIZE 常數名'                        = @{ Code = '$SC_MINIMIZE = 1'; Want = 'SC_MINIMIZE' }
    'Shell.Application MinimizeAll'             = @{ Code = '(New-Object -ComObject Shell.Application).MinimizeAll()'; Want = 'MinimizeAll' }
    'Shell.Application ToggleDesktop'           = @{ Code = '(New-Object -ComObject Shell.Application).ToggleDesktop()'; Want = 'ToggleDesktop' }
    'UIA SetWindowVisualState'                  = @{ Code = '$pattern.SetWindowVisualState(2)'; Want = 'SetWindowVisualState' }
    'CloseWindow（Win32 的最小化）'             = @{ Code = '[X.Native]::CloseWindow($h)'; Want = 'CloseWindow' }
    'SetWindowPlacement（showCmd 可最小化）'    = @{ Code = '[void][X.Native]::SetWindowPlacement($h, [ref]$wp)'; Want = 'SetWindowPlacement' }
    '& (Get-Module Occluders) { … }'            = @{ Code = '& (Get-Module Occluders) { Invoke-MinimizeWindow -Hwnd $h }'; Want = '模組' }
    'Import-Module -PassThru 後 & $m { … }'     = @{ Code = "`$m = Import-Module (Join-Path `$PSScriptRoot `"lib/Occluders.psm1`") -PassThru`n& `$m { Get-ShellPid }"; Want = '模組' }
    '. $m { … }（dot 呼叫模組範圍）'            = @{ Code = '. $occ { Get-WindowInfo $h }'; Want = '模組' }
    'dot-source Occluders.psm1'                 = @{ Code = '. (Join-Path $PSScriptRoot "lib/Occluders.psm1")'; Want = '模組' }
    'PSModuleInfo.NewBoundScriptBlock'          = @{ Code = '$sb = $mod.NewBoundScriptBlock({ Invoke-MinimizeWindow -Hwnd $h })'; Want = '模組' }
    'PSModuleInfo.SessionState'                 = @{ Code = '$fn = $mod.SessionState.InvokeCommand'; Want = '模組' }
    '直接呼叫未匯出的 Invoke-MinimizeWindow'    = @{ Code = 'Invoke-MinimizeWindow -Hwnd $h'; Want = '未匯出' }
    'Clear-Occluders 注入 -Minimize'            = @{ Code = 'Clear-Occluders -HostPid 1 -Points @() -Minimized $m -Minimize { param($h) }'; Want = '注入' }
    'Clear-Occluders 注入 -HitTest'             = @{ Code = 'Clear-Occluders -HostPid 1 -Points @() -Minimized $m -HitTest { param($x, $y) }'; Want = '注入' }
    'Invoke-MinimizeAppWindows 注入 -Describe'  = @{ Code = 'Invoke-MinimizeAppWindows -Hwnds @() -Minimized $m -Describe { param($h) }'; Want = '注入' }
    'Invoke-MinimizeAppWindows 注入 -ShellPid'  = @{ Code = 'Invoke-MinimizeAppWindows -Hwnds @() -Minimized $m -ShellPid 0'; Want = '注入' }
    'Restore-Occluders 注入 -Restore'           = @{ Code = 'Restore-Occluders -Minimized $m -Restore { param($h, $s) }'; Want = '注入' }
    'Clear-Occluders 以 splat 傳參數'           = @{ Code = 'Clear-Occluders @occArgs'; Want = '注入' }
}
$okSample = @(
    '# 註解提到 ShowWindowAsync($h, 6)、MinimizeAll、0xF020 不算'
    '[void][X.Native]::ShowWindow($h, 9)'
    '[void][X.Native]::ShowWindowAsync($h, 4)'
    '[void][X.Native]::ShowWindow($h, 8)'
    '$SW_SHOWNOACTIVATE = 4'
    '[void][X.Native]::ShowWindow($h, $SW_SHOWNOACTIVATE)'
    'Import-Module (Join-Path $PSScriptRoot "lib/Occluders.psm1") -Force'
    'Clear-Occluders -HostPid 1 -Points @() -Minimized $m -OwnPids @(1) -Log { param($x) }'
    'Restore-Occluders -Minimized $m -Log { param($x) }'
    '& $occLog "訊息"'
    '& { param($a) $a } 1'
) -join "`n"
$tmpDir = Join-Path ([IO.Path]::GetTempPath()) ("fc-noinline-{0}" -f [guid]::NewGuid().ToString('N'))
[void](New-Item -ItemType Directory -Path $tmpDir)
try {
    $enc = New-Object System.Text.UTF8Encoding($false)
    $i = 0
    foreach ($k in $badSamples.Keys) {
        $i++
        $f = Join-Path $tmpDir "bad$i.ps1"
        [IO.File]::WriteAllText($f, $badSamples[$k].Code, $enc)
        $r = Get-OccluderViolations $f
        $hit = @($r.Violations | Where-Object { $_ -match [regex]::Escape($badSamples[$k].Want) })
        Check "自我檢查（違規）：$k → 抓到「$($badSamples[$k].Want)」" ($hit.Count -ge 1) ($r.Violations -join ' | ')
    }
    $f = Join-Path $tmpDir 'ok.ps1'
    [IO.File]::WriteAllText($f, $okSample, $enc)
    $r = Get-OccluderViolations $f
    Check '自我檢查（正常）：註解、還原類 nCmdShow（9／4／8 與賦值正確的 $SW_SHOWNOACTIVATE）、正常匯入與呼叫不算違規' ($r.Violations.Count -eq 0) ($r.Violations -join ' | ')
} finally { Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue }

Write-Host "`n$($script:Pass) passed, $($script:Fail) failed"
if ($script:Fail -gt 0) { exit 1 }
exit 0
