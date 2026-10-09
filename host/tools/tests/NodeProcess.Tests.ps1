<#
.SYNOPSIS
    host/tools/lib/NodeProcess.psm1（Invoke-NodeUtf8）與兩支驗收腳本 CDP 呼叫的編碼測試（純 PowerShell 斷言，不需 Pester）。

.DESCRIPTION
    task 5.2 實跑：從背景 shell（主控台碼頁 950）執行 verify-adaptive-zoom.ps1 時，`& node` 的 UTF-8 輸出被解成
    亂碼，宿主的中文拒絕訊息連引號一起被吃掉，JSON 讀不到 `ok`。本測試把本行程的 `[Console]::OutputEncoding`
    設成 950 重現該環境（對照組：`& node` 確實讀出亂碼），再驗：
      - Invoke-NodeUtf8 讀回正確中文、參數（含空白、引號、中文）原樣傳給 node、結束碼與 stderr、逾時。
      - verify-adaptive-zoom.ps1 的 `Invoke-Json` 與 verify-grid-overlay.ps1 的 `Invoke-Eval`（以 AST 取出，把
        `$PSScriptRoot` 換成放了假 host-cdp-eval.mjs 的暫存資料夾）在 950 碼頁下仍讀得到 `ok:false` 與中文訊息。
    不啟動宿主、不呼叫 Win32 API、不注入輸入。

.EXAMPLE
    pwsh -NoProfile -File host/tools/tests/NodeProcess.Tests.ps1
    結束碼 0＝全部通過、1＝有失敗。
#>
$ErrorActionPreference = 'Stop'

$toolsDir = Split-Path $PSScriptRoot -Parent
Import-Module (Join-Path $toolsDir 'lib\NodeProcess.psm1') -Force

$script:Pass = 0
$script:Fail = 0
function Check([string]$name, [bool]$cond, [string]$detail = '') {
    if ($cond) { $script:Pass++; Write-Host "PASS  $name" }
    else { $script:Fail++; Write-Host "FAIL  $name $detail" -ForegroundColor Red }
}

$msg = '不在編輯版面或版面已鎖定，無法調整字級'
$work = Join-Path ([IO.Path]::GetTempPath()) ('nodeproc-tests-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force -Path $work | Out-Null
$enc = New-Object System.Text.UTF8Encoding($false)
$savedOut = [Console]::OutputEncoding
try {
    # 假的 host-cdp-eval.mjs：印出含中文的拒絕結果（同宿主 adjust_widget_font_scale 被拒時的形狀），並回顯參數。
    $fake = @"
import { spawn } from 'node:child_process';
const args = process.argv.slice(2);
if (args[0] === 'argv') { console.log(JSON.stringify(args.slice(1))); process.exit(0); }
if (args[0] === 'fail') { console.error('錯誤：' + args[1]); process.exit(3); }
if (args[0] === 'sleep') { setTimeout(() => {}, 60000); }
else if (args[0] === 'sleep-child') {
  const c = spawn(process.execPath, ['-e', 'setTimeout(() => {}, 60000)'], { stdio: 'ignore' });
  console.log(JSON.stringify({ child: c.pid }));
  setTimeout(() => {}, 60000);
}
else if (args[1] === 'w=fail-true') {
  console.log(JSON.stringify({ ok: false, err: 'x' }));
  console.error('true false 找不到分頁');
  process.exit(2);
}
else if (args[1] === 'w=ok-true') { console.log('true'); }
else if (args[1] === 'w=ok-untrue') { console.log(JSON.stringify('untrue')); }
else { console.log(JSON.stringify({ ok: false, err: '$msg', page: args[1] })); }
"@
    $fakePath = Join-Path $work 'host-cdp-eval.mjs'
    [IO.File]::WriteAllText($fakePath, $fake, $enc)

    [Console]::OutputEncoding = [Text.Encoding]::GetEncoding(950)
    Check '重現環境：主控台輸出碼頁設成 950' ([Console]::OutputEncoding.CodePage -eq 950)

    # 對照組：舊寫法 `& node ... 2>&1` 在 950 下讀出亂碼（若這項失敗，代表本機重現不了，下面的正向檢查就沒有鑑別力）。
    $old = (& node $fakePath 9999 'w=macro' 'x' 2>&1) -join "`n"
    $oldJ = $null; try { $oldJ = $old | ConvertFrom-Json } catch { }
    Check '對照組：`& node` 在 950 碼頁下讀不到正確的中文訊息（重現實跑的亂碼）' (-not ($oldJ -and $oldJ.err -eq $msg)) $old

    $r = Invoke-NodeUtf8 -ArgumentList @($fakePath, '9999', 'w=macro', 'x')
    $j = $null; try { $j = $r.StdOut | ConvertFrom-Json } catch { }
    Check 'Invoke-NodeUtf8：950 碼頁下仍以 UTF-8 讀回、JSON 可解析' ($r.ExitCode -eq 0 -and $null -ne $j) "$($r.ExitCode) $($r.StdOut)"
    Check 'Invoke-NodeUtf8：ok=false、中文訊息完全相同' ($j -and $j.ok -eq $false -and $j.err -eq $msg) $r.StdOut

    $weird = @('有 空白', 'a"b', "c'd", 'e\f\"g', '', '中文；全形')
    $r = Invoke-NodeUtf8 -ArgumentList (@($fakePath, 'argv') + $weird)
    $back = @(); try { $back = @($r.StdOut | ConvertFrom-Json) } catch { }
    Check 'Invoke-NodeUtf8：參數（空白、引號、反斜線、空字串、中文）原樣傳給 node' ($back.Count -eq $weird.Count -and (($back -join '|') -eq ($weird -join '|'))) $r.StdOut

    $r = Invoke-NodeUtf8 -ArgumentList @($fakePath, 'fail', '找不到分頁')
    Check 'Invoke-NodeUtf8：非 0 結束碼與 UTF-8 stderr' ($r.ExitCode -eq 3 -and $r.StdErr -eq '錯誤：找不到分頁' -and $r.StdOut -eq '') "$($r.ExitCode) [$($r.StdErr)]"

    $sw = [Diagnostics.Stopwatch]::StartNew()
    $r = Invoke-NodeUtf8 -ArgumentList @($fakePath, 'sleep') -TimeoutSec 2
    Check 'Invoke-NodeUtf8：逾時回 -1、TimedOut 且不卡住' ($r.ExitCode -eq -1 -and $r.TimedOut -eq $true -and $sw.Elapsed.TotalSeconds -lt 15) "$($r.ExitCode) $($sw.Elapsed.TotalSeconds)"
    Check 'Invoke-NodeUtf8：逾時回傳時 node 已結束' ($r.Pid -gt 0 -and -not (Get-Process -Id $r.Pid -ErrorAction SilentlyContinue)) "pid=$($r.Pid)"

    # 逾時＋子行程：node 先生一個子 node（印出它的 PID）再睡；回傳時兩者都必須已消失（Kill(true)＋等待確認）。
    $r = Invoke-NodeUtf8 -ArgumentList @($fakePath, 'sleep-child') -TimeoutSec 3
    $childPid = 0; try { $childPid = [int]($r.StdOut | ConvertFrom-Json).child } catch { }
    Check '逾時＋子行程：逾時前寫出的 stdout 仍回傳（讀得到子行程 PID）' ($r.TimedOut -eq $true -and $childPid -gt 0) "out=[$($r.StdOut)] err=[$($r.StdErr)]"
    $alive = @(@($r.Pid, $childPid) | Where-Object { $_ -gt 0 -and (Get-Process -Id $_ -ErrorAction SilentlyContinue) })
    Check '逾時＋子行程：回傳時 node 與其子行程都已消失' ($childPid -gt 0 -and $alive.Count -eq 0) "仍存活：$($alive -join ',')"
    foreach ($id in $alive) { Stop-Process -Id $id -Force -ErrorAction SilentlyContinue }   # 測試失敗時不留行程

    $r = Invoke-NodeUtf8 -ArgumentList @($fakePath, '9999', 'w=fail-true', 'x')
    Check '非 0 結束：ExitCode 保留、stdout 與 stderr 分開' ($r.ExitCode -eq 2 -and $r.StdOut -eq '{"ok":false,"err":"x"}' -and $r.StdErr -match '^true false') "$($r.ExitCode) [$($r.StdOut)] [$($r.StdErr)]"

    # 兩支驗收腳本的 CDP 呼叫函式：以 AST 取出，把 $PSScriptRoot 換成假 host-cdp-eval.mjs 所在資料夾。
    $CdpPort = 9999
    foreach ($case in @(
            @{ File = 'verify-adaptive-zoom.ps1'; Fn = 'Invoke-Json' },
            @{ File = 'verify-grid-overlay.ps1'; Fn = 'Invoke-EvalResult' })) {
        $ast = [System.Management.Automation.Language.Parser]::ParseFile((Join-Path $toolsDir $case.File), [ref]$null, [ref]$null)
        $fn = $ast.Find({ param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $case.Fn }, $true)
        Check "$($case.File)：找得到 $($case.Fn)" ($null -ne $fn)
        if (-not $fn) { continue }
        Check "$($case.File)：$($case.Fn) 不再用 `& node`（改經 Invoke-NodeUtf8）" (-not ($fn.Extent.Text -match '&\s*node\b') -and $fn.Extent.Text.Contains('Invoke-NodeUtf8'))
        . ([scriptblock]::Create($fn.Extent.Text.Replace('$PSScriptRoot', "'$work'")))
        $out = & $case.Fn 'w=macro' 'x'
        $parsed = if ($out -is [string]) { try { $out | ConvertFrom-Json } catch { $null } }
        elseif ($out.PSObject.Properties['StdOut']) { if ($out.ExitCode -eq 0) { try { $out.StdOut | ConvertFrom-Json } catch { $null } } else { $null } }
        else { $out }
        Check "$($case.File)：$($case.Fn) 在 950 碼頁下讀到 ok=false 與正確中文訊息" ($parsed -and $parsed.ok -eq $false -and $parsed.err -eq $msg -and $parsed.page -eq 'w=macro') ($out | Out-String)
    }
    # Codex review：node 非 0 結束（stdout 是合法 JSON、stderr 含 true／false）不得被當成頁面結果。
    $jErr = Invoke-Json 'w=fail-true' 'x'
    Check 'verify-adaptive-zoom.ps1 Invoke-Json：非 0 結束 → __error（不解析 stdout 的 ok:false 當成被拒）' ($jErr.PSObject.Properties['__error'] -and -not $jErr.PSObject.Properties['ok']) ($jErr | Out-String)
    $ast = [System.Management.Automation.Language.Parser]::ParseFile((Join-Path $toolsDir 'verify-adaptive-zoom.ps1'), [ref]$null, [ref]$null)
    foreach ($n in 'Test-Rejected') { $f = $ast.Find({ param($x) $x -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $x.Name -eq $n }, $true); . ([scriptblock]::Create($f.Extent.Text)) }
    Check 'verify-adaptive-zoom.ps1：非 0 結束的結果不算「被拒」' (-not (Test-Rejected $jErr))
    $fz = $ast.Find({ param($x) $x -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $x.Name -eq 'Get-ExpectedZooms' }, $true)
    Check 'verify-adaptive-zoom.ps1 Get-ExpectedZooms：node 非 0 結束就丟例外、只解析 StdOut' ($fz.Extent.Text -match 'if \(\$r\.ExitCode -ne 0\) \{ throw' -and $fz.Extent.Text -match '\$r\.StdOut \| ConvertFrom-Json')

    $gAst = [System.Management.Automation.Language.Parser]::ParseFile((Join-Path $toolsDir 'verify-grid-overlay.ps1'), [ref]$null, [ref]$null)
    foreach ($n in 'Invoke-EvalResult', 'Test-EvalMatch', 'Wait-Eval') {
        $f = $gAst.Find({ param($x) $x -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $x.Name -eq $n }, $true)
        Check "verify-grid-overlay.ps1：找得到 $n" ($null -ne $f)
        if ($f) { . ([scriptblock]::Create($f.Extent.Text.Replace('$PSScriptRoot', "'$work'"))) }
    }
    if (Get-Command Test-EvalMatch, Wait-Eval -ErrorAction SilentlyContinue | Measure-Object | Where-Object { $_.Count -eq 2 }) {
    $ok0 = [PSCustomObject]@{ ExitCode = 0; StdOut = 'true'; StdErr = '' }
    Check 'Test-EvalMatch：exit 0、stdout=true 符合 true' (Test-EvalMatch $ok0 'true')
    Check 'Test-EvalMatch：exit 0、stdout=true 符合 true|false' (Test-EvalMatch $ok0 'true|false')
    Check 'Test-EvalMatch：exit 0、stdout=true 不符合 false' (-not (Test-EvalMatch $ok0 'false'))
    Check 'Test-EvalMatch：錨定——"untrue" 不符合 true' (-not (Test-EvalMatch ([PSCustomObject]@{ ExitCode = 0; StdOut = '"untrue"'; StdErr = '' }) 'true'))
    Check 'Test-EvalMatch：錨定——"true false" 不符合 true|false' (-not (Test-EvalMatch ([PSCustomObject]@{ ExitCode = 0; StdOut = 'true false'; StdErr = '' }) 'true|false'))
    Check 'Test-EvalMatch：非 0 結束、stderr=true → 不符' (-not (Test-EvalMatch ([PSCustomObject]@{ ExitCode = 1; StdOut = ''; StdErr = 'true' }) 'true'))
    Check 'Test-EvalMatch：非 0 結束、stdout=true → 不符' (-not (Test-EvalMatch ([PSCustomObject]@{ ExitCode = 1; StdOut = 'true'; StdErr = '' }) 'true'))
    Check 'Test-EvalMatch：$null → 不符' (-not (Test-EvalMatch $null 'true'))
    $w = Wait-Eval 'w=fail-true' 'x' 'true' -TimeoutSec 1 -PollMs 50
    Check 'Wait-Eval：node 非 0 結束、stderr 含 true → Ok=false（不假通過）' ($w.Ok -eq $false -and $w.Text -match '錯誤 exit=2') ($w | Out-String)
    $w = Wait-Eval 'w=fail-true' 'x' 'true|false' -TimeoutSec 1 -PollMs 50
    Check 'Wait-Eval：同上、pattern true|false → Ok=false' ($w.Ok -eq $false) ($w | Out-String)
    $w = Wait-Eval 'w=ok-true' 'x' 'true' -TimeoutSec 5 -PollMs 50
    Check 'Wait-Eval：exit 0、stdout=true → Ok=true、Text=true' ($w.Ok -eq $true -and $w.Text -eq 'true') ($w | Out-String)
    $w = Wait-Eval 'w=ok-untrue' 'x' 'true' -TimeoutSec 1 -PollMs 50
    Check 'Wait-Eval：stdout="untrue" → Ok=false（錨定）' ($w.Ok -eq $false) ($w | Out-String)
    }
    $gsrc = [IO.File]::ReadAllText((Join-Path $toolsDir 'verify-grid-overlay.ps1'))
    Check 'verify-grid-overlay.ps1：layout_locked 判定改用 Wait-Eval 的 .Ok（不再 -match 文字）' ($gsrc.Contains('([bool]$locked1.Ok)') -and $gsrc.Contains('[bool]$locked2.Ok') -and -not ($gsrc -match '\$locked[12] -match'))

    $src = [IO.File]::ReadAllText((Join-Path $toolsDir 'verify-adaptive-zoom.ps1'))
    Check 'verify-adaptive-zoom.ps1 全檔沒有 `& node`（預期倍率 helper 也走 UTF-8）' (-not ($src -match '&\s*node\b'))
    Check 'verify-adaptive-zoom.ps1 匯入 NodeProcess.psm1' ($src.Contains("lib\NodeProcess.psm1"))
}
finally {
    [Console]::OutputEncoding = $savedOut
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}

Write-Host "NodeProcess.Tests：$script:Pass passed, $script:Fail failed"
if ($script:Fail -gt 0) { exit 1 }
exit 0
