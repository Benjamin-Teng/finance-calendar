<#
.SYNOPSIS
    host/tools/verify-7.3-placeholder.ps1 的純函式與結構測試（純 PowerShell 斷言，不需 Pester）。

.DESCRIPTION
    **不執行 verify-7.3-placeholder.ps1**（它會啟動宿主、注入滑鼠）：以 PowerShell Parser 只取出純函式，
    餵合成的探查結果、格座標、埠與行程清單；探查運算式另以 Node 對假 DOM 執行。結構檢查確認前提不成立時
    在送輸入之前就中止、收尾只動自己的東西、當機只經 cdp-crash.mjs 精準送出，以及檔頭與 README 的
    涵蓋聲明一致。

.EXAMPLE
    pwsh -NoProfile -File host/tools/tests/Verify73Placeholder.Tests.ps1
    結束碼 0＝全部通過、1＝有失敗。
#>
param([string]$Target, [string]$Readme)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$toolsDir = Split-Path $PSScriptRoot -Parent
if (-not $Target) { $Target = Join-Path $toolsDir 'verify-7.3-placeholder.ps1' }
if (-not $Readme) { $Readme = Join-Path $toolsDir 'README.md' }

$script:Pass = 0
$script:Fail = 0
function Check([string]$name, [bool]$cond, [string]$detail = '') {
    if ($cond) { $script:Pass++; Write-Host "PASS  $name" }
    else { $script:Fail++; Write-Host "FAIL  $name $detail" -ForegroundColor Red }
}

$tokens = $null
$errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile($Target, [ref]$tokens, [ref]$errors)
Check 'verify-7.3-placeholder.ps1 解析 0 錯誤' (@($errors).Count -eq 0) (($errors | ForEach-Object { $_.Message }) -join ' | ')
$GRID = 48
$pure = 'Get-ElementProbeExpression', 'Get-ElementHitExpression', 'Test-ElementProbe', 'Test-ElementHit', 'Test-GridOverlap',
'Select-PlaceholderDropTarget', 'Test-HasThickFrame', 'Test-CdpOwnedByTree', 'Test-RebuiltWindows'
$structural = 'Get-PressPoint', 'Invoke-PlaceholderDrag', 'Invoke-LockedDrag', 'Invoke-CdpCrash'
$fnAst = @{}
foreach ($name in $pure + $structural) {
    $fn = $ast.Find({
            param($n)
            $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $name
        }, $true)
    Check "找得到函式 $name" ($null -ne $fn)
    $fnAst[$name] = $fn
    if ($fn -and $pure -contains $name) { . ([scriptblock]::Create($fn.Extent.Text)) }
}
if (@($fnAst.Values | Where-Object { $null -eq $_ }).Count -gt 0) {
    Write-Host "Verify73Placeholder.Tests：$script:Pass passed, $script:Fail failed"
    exit 1
}

# ── Test-ElementProbe ───────────────────────────────────────────────────────────
function P([hashtable]$h) {
    $base = [ordered]@{ exists = $true; display = 'flex'; visibility = 'visible'; drag = 'deep'; editMode = $true; dpr = 2.5; left = 0; top = 0; width = 400; height = 40; vw = 400; vh = 40 }
    foreach ($k in $h.Keys) { $base[$k] = $h[$k] }
    [PSCustomObject]$base
}
$r = Test-ElementProbe (P @{})
Check '元素填滿 viewport → Ok、中心 (200,20)、dpr 2.5' ($r.Ok -and $r.CssX -eq 200 -and $r.CssY -eq 20 -and $r.Dpr -eq 2.5) ($r | Out-String)
$r = Test-ElementProbe (P @{ left = -100; width = 300 })
Check '元素左側超出 viewport → 取交集中心 (100,20)' ($r.Ok -and $r.CssX -eq 100 -and $r.CssY -eq 20) ($r | Out-String)
foreach ($c in @(
        @{ n = '探查為 null'; p = $null },
        @{ n = '沒有該元素'; p = [PSCustomObject]@{ exists = $false } },
        @{ n = 'display=none'; p = (P @{ display = 'none' }) },
        @{ n = 'visibility=hidden'; p = (P @{ visibility = 'hidden' }) },
        @{ n = '寬度 0'; p = (P @{ width = 0 }) },
        @{ n = 'dpr 0'; p = (P @{ dpr = 0 }) },
        @{ n = '欄位不是數字'; p = (P @{ top = 'abc' }) },
        @{ n = '完全在 viewport 外'; p = (P @{ left = 500 }) },
        @{ n = '只剩不到 1 CSS px 的交集'; p = (P @{ top = 39.5 }) }
    )) {
    $r = Test-ElementProbe $c.p
    Check "前提不成立：$($c.n) → 不 Ok（不送輸入）" (-not $r.Ok -and $r.Reason) ($r | Out-String)
}

# ── Test-ElementHit ─────────────────────────────────────────────────────────────
Check '命中目標（inside=true）→ Ok' ((Test-ElementHit ([PSCustomObject]@{ inside = $true; hit = 'DIV.edit-placeholder' })).Ok)
$r = Test-ElementHit ([PSCustomObject]@{ inside = $false; hit = 'DIV.resize-handle resize-s' })
Check '命中別的元素 → 不 Ok、說明命中誰' (-not $r.Ok -and $r.Reason -match 'resize-handle') ($r | Out-String)
Check '命中測試沒有結果 → 不 Ok' (-not (Test-ElementHit $null).Ok)

# ── 運算式 ────────────────────────────────────────────────────────────────────
$sel = '#widget-root > .edit-placeholder'
$probeExpr = Get-ElementProbeExpression -Selector $sel
Check '探查運算式不含雙引號（經命令列傳給 node）' (-not $probeExpr.Contains('"'))
Check '探查運算式以指定選擇器查詢' ($probeExpr.Contains("document.querySelector('$sel')"))
Check '探查運算式讀 getBoundingClientRect 與 devicePixelRatio（不用固定像素）' ($probeExpr.Contains('getBoundingClientRect') -and $probeExpr.Contains('devicePixelRatio'))
Check '#widget-root 也可當選擇器（鎖定時拖 clock）' ((Get-ElementProbeExpression -Selector '#widget-root').Contains("document.querySelector('#widget-root')"))
$oldCulture = [System.Threading.Thread]::CurrentThread.CurrentCulture
try {
    [System.Threading.Thread]::CurrentThread.CurrentCulture = [System.Globalization.CultureInfo]::GetCultureInfo('de-DE')
    $hitExpr = Get-ElementHitExpression -Selector $sel -CssX 12.5 -CssY 7.25
} finally { [System.Threading.Thread]::CurrentThread.CurrentCulture = $oldCulture }
Check '命中運算式不含雙引號' (-not $hitExpr.Contains('"'))
Check '命中運算式在 de-DE 文化下仍用小數點（elementFromPoint(12.5, 7.25)）' ($hitExpr.Contains('elementFromPoint(12.5, 7.25)')) $hitExpr

$node = Get-Command node -ErrorAction SilentlyContinue
Check '找得到 node（以假 DOM 執行探查運算式）' ($null -ne $node)
if ($node) {
    $harness = @'
const ph = { getBoundingClientRect: () => ({ left: 0, top: 0, width: 640, height: 64 }), getAttribute: (n) => (n === 'data-tauri-drag-region' ? 'deep' : null), _d: 'flex' };
globalThis.window = { devicePixelRatio: 2.8, innerWidth: 640, innerHeight: 64 };
globalThis.document = { querySelector: (s) => (s === '#widget-root > .edit-placeholder' ? (globalThis.__none ? null : ph) : null), body: { classList: { contains: (c) => c === 'edit-mode' } } };
globalThis.getComputedStyle = (el) => ({ display: el._d, visibility: 'visible' });
const a = eval(process.env.FC_EXPR);
globalThis.__none = true;
const b = eval(process.env.FC_EXPR);
console.log(JSON.stringify({ a, b }));
'@
    $tmpJs = Join-Path ([IO.Path]::GetTempPath()) ('v73p-harness-' + [guid]::NewGuid().ToString('N') + '.cjs')
    try {
        [IO.File]::WriteAllText($tmpJs, $harness)
        $env:FC_EXPR = $probeExpr
        $out = (& node $tmpJs 2>&1) -join "`n"
        $j = $null
        try { $j = $out | ConvertFrom-Json } catch { }
        Check '假 DOM：探查運算式執行成功' ($null -ne $j) $out
        if ($j) {
            Check '假 DOM：回傳 drag=deep、editMode=true、dpr 2.8' ($j.a.exists -eq $true -and $j.a.drag -eq 'deep' -and $j.a.editMode -eq $true -and $j.a.dpr -eq 2.8) $out
            $r = Test-ElementProbe $j.a
            Check '假 DOM 結果 → Test-ElementProbe Ok、中心 (320,32)' ($r.Ok -and $r.CssX -eq 320 -and $r.CssY -eq 32) ($r | Out-String)
            Check '假 DOM：沒有元素 → exists=false → 不 Ok' ($j.b.exists -eq $false -and -not (Test-ElementProbe $j.b).Ok) $out
        }
    } finally {
        Remove-Item $tmpJs -ErrorAction SilentlyContinue
        Remove-Item Env:FC_EXPR -ErrorAction SilentlyContinue
    }
}

# ── Select-PlaceholderDropTarget（預設格座標：host/src/settings.rs DEFAULT_GRID_RECTS）───────────
function G([int]$c, [int]$r, [int]$w, [int]$h) { [PSCustomObject]@{ col = $c; row = $r; w = $w; h = $h } }
$others = @((G 15 1 16 10), (G 15 12 16 30), (G 32 1 15 17), (G 32 19 15 23))
$t = Select-PlaceholderDropTarget -Current (G 15 43 32 4) -Others $others
Check '預設版面：quotes (15,43,32,4) → 左移 10 格 (5,43,32,4)' ($t -and $t.col -eq 5 -and $t.row -eq 43 -and $t.w -eq 32 -and $t.h -eq 4) ($t | Out-String)
$t2 = Select-PlaceholderDropTarget -Current (G 5 43 32 4) -Others $others
Check '第二次（Reload 後）：從 (5,43) → (0,43)，格數不變' ($t2 -and $t2.col -eq 0 -and $t2.row -eq 43 -and $t2.w -eq 32) ($t2 | Out-String)
$t3 = Select-PlaceholderDropTarget -Current (G 0 43 32 4) -Others @()
Check '已在左緣：不越界，改往右 (10,43)' ($t3 -and $t3.col -eq 10) ($t3 | Out-String)
$t4 = Select-PlaceholderDropTarget -Current (G 15 43 32 4) -Others @((G 0 39 15 9), (G 47 39 1 9), (G 0 41 48 2))
Check '左右都擋住、上方擋住 → 往下一列 (15,44)' ($t4 -and $t4.col -eq 15 -and $t4.row -eq 44) ($t4 | Out-String)
$t5 = Select-PlaceholderDropTarget -Current (G 0 0 48 48) -Others @()
Check '佔滿整個格線 → 沒有合法位置（$null，呼叫端 PRECONDITION FAIL）' ($null -eq $t5)
$t6 = Select-PlaceholderDropTarget -Current (G 15 43 32 4) -Others @((G 0 0 48 43), (G 0 47 48 1))
Check '只剩自己那一列且左右無空間時不回傳原位' ($null -eq $t6 -or -not ($t6.col -eq 15 -and $t6.row -eq 43)) ($t6 | Out-String)
Check 'Test-GridOverlap：共用邊不算相交' (-not (Test-GridOverlap (G 0 0 10 10) (G 10 0 5 5)))
Check 'Test-GridOverlap：重疊一格算相交' (Test-GridOverlap (G 0 0 10 10) (G 9 9 5 5))

# ── Test-HasThickFrame ─────────────────────────────────────────────────────────
Check 'WS_POPUP|WS_VISIBLE（0x94000000）→ 沒有 WS_THICKFRAME' (-not (Test-HasThickFrame 0x94000000))
Check '加上 WS_THICKFRAME（0x94040000）→ 有' (Test-HasThickFrame 0x94040000)

# ── Test-CdpOwnedByTree：送當機指令前，CDP 埠的 Listen 行程必須在本宿主行程樹內 ───────────────
Check 'Listen 行程在樹內 → OK' ((Test-CdpOwnedByTree -ListenerPids @(201) -TreeIds @(100, 200, 201)) -eq 'OK')
Check '沒有 Listen 行程 → NO-LISTENER' ((Test-CdpOwnedByTree -ListenerPids @() -TreeIds @(100, 200)) -eq 'NO-LISTENER')
Check '有任何 Listen 行程不在樹內 → FOREIGN' ((Test-CdpOwnedByTree -ListenerPids @(201, 999) -TreeIds @(100, 201)) -eq 'FOREIGN')

# ── Test-RebuiltWindows：故障重建後每扇都是新 HWND 且帶 WS_THICKFRAME ─────────────────────────
$before = @{ clock = 0x100; fixed = 0x200 }
$ok = @(Test-RebuiltWindows -Before $before -After @{ clock = @{ Hwnd = 0x101; Style = 0x94040000 }; fixed = @{ Hwnd = 0x201; Style = 0x94040000 } })
Check '全部是新 HWND、都帶 WS_THICKFRAME → 沒有問題' ($ok.Count -eq 0) ($ok -join '; ')
$bad = @(Test-RebuiltWindows -Before $before -After @{ clock = @{ Hwnd = 0x100; Style = 0x94040000 }; fixed = @{ Hwnd = 0x201; Style = 0x94000000 } })
Check 'HWND 沒變（沒重建）與缺 WS_THICKFRAME 各算一項' ($bad.Count -eq 2 -and ($bad -join ' ') -match 'clock' -and ($bad -join ' ') -match 'fixed') ($bad -join '; ')
$miss = @(Test-RebuiltWindows -Before $before -After @{ clock = @{ Hwnd = 0x101; Style = 0x94040000 } })
Check '重建後少了一扇 → 算問題' ($miss.Count -eq 1 -and $miss[0] -match 'fixed') ($miss -join '; ')

# ── 結構 ──────────────────────────────────────────────────────────────────────
$src = [IO.File]::ReadAllText($Target)
foreach ($lib in 'EvidenceLog', 'AutostartRegistry', 'SafeInput', 'Occluders', 'ScratchWindow', 'ProcessTree', 'ScrollAreaTarget') {
    Check "匯入 lib\$lib.psm1" ($src.Contains("Import-Module (Join-Path `$PSScriptRoot 'lib\$lib.psm1') -Force"))
}
Check '開頭跑 Invoke-SafeInputPreflight，非 0 就照結束碼離開' ($src -match '\$pf = Invoke-SafeInputPreflight\s+if \(\$pf\.ExitCode -ne 0\) \{ Write-Host \$pf\.Message; exit \$pf\.ExitCode \}')
Check '已有 fc-host 在跑 → 獨立結束碼 4（不與執行途中鎖定的 2 共用）' ($src -match "Get-Process -Name fc-host[^\n]*\)\s*\{\s*\n[^\n]*\n\s*exit 4\s*\n")
Check '宿主以暫存 APPDATA／LOCALAPPDATA 啟動' ($src.Contains('$env:APPDATA = $tempAppData') -and $src.Contains('$env:LOCALAPPDATA = $tempLocal'))
Check '前景基準用 Start-ScratchForm（不自己開表單）' ($src.Contains('Start-ScratchForm') -and -not $src.Contains('Windows.Forms.Form'))

$tries = @($ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.TryStatementAst] -and $null -ne $n.Finally }, $true))
$finallyText = ($tries | ForEach-Object { $_.Finally.Extent.Text }) -join "`n"
foreach ($cmd in 'Stop-ProcessTree -Process $hostProc', 'Stop-ScratchForm $form', 'Restore-Occluders', 'Restore-FcHostAutostartRegistry $regSnap', "Invoke-GuardedMouse 4") {
    Check "finally 收尾：$cmd" ($finallyText.Contains($cmd))
}
$stops = @($ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.CommandAst] -and $n.GetCommandName() -in 'Stop-Process', 'taskkill', 'taskkill.exe' }, $true))
Check '沒有任何 Stop-Process／taskkill（當機只經 cdp-crash.mjs，收尾只經 Stop-ProcessTree）' ($stops.Count -eq 0) (($stops | ForEach-Object { $_.Extent.Text }) -join ' | ')

# 當機一律經 Invoke-CdpCrash：先確認 CDP 埠屬於本宿主（Test-CdpOwnedByTree），再呼叫 cdp-crash.mjs。
$crashText = $fnAst['Invoke-CdpCrash'].Extent.Text
$iOwn = $crashText.IndexOf('Test-CdpOwnedByTree'); $iNode = $crashText.IndexOf('cdp-crash.mjs')
Check 'Invoke-CdpCrash：先 Test-CdpOwnedByTree 再呼叫 cdp-crash.mjs' ($iOwn -ge 0 -and $iNode -gt $iOwn) "own=$iOwn node=$iNode"
$crashCalls = @($ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq 'Invoke-CdpCrash' }, $true) | ForEach-Object { $_.Extent.Text })
Check 'Reload 以 Page.crash 精準打 quotes（Invoke-CdpCrash page quotes）' ($crashCalls -contains "Invoke-CdpCrash 'page' 'quotes'") ($crashCalls -join ' | ')
Check '故障重建以 Browser.crash 觸發（Invoke-CdpCrash browser）' ($crashCalls -contains "Invoke-CdpCrash 'browser'") ($crashCalls -join ' | ')
Check '全檔只有 Invoke-CdpCrash 呼叫 cdp-crash.mjs' (([regex]::Matches($src, 'cdp-crash\.mjs')).Count -eq ([regex]::Matches($crashText, 'cdp-crash\.mjs')).Count + ([regex]::Matches(($tokens | Where-Object { $_.Kind -eq 'Comment' } | ForEach-Object Text) -join "`n", 'cdp-crash\.mjs')).Count)

# Get-PressPoint：探查前提、選點、遮擋判讀、命中前提的順序；兩種拖曳都先取按下點才送輸入。
$pressText = $fnAst['Get-PressPoint'].Extent.Text
$order = 'Test-ElementProbe', 'if (-not $pre.Ok) { throw "PRECONDITION', 'ConvertTo-ScreenPoint', 'Assert-OccluderResult (Clear-Occluders', 'Test-ElementHit', 'if (-not $hit.Ok) { throw "PRECONDITION'
$prev = -1
foreach ($k in $order) {
    $i = $pressText.IndexOf($k)
    Check "Get-PressPoint：「$k」依序出現" ($i -gt $prev) "index=$i prev=$prev"
    $prev = [math]::Max($prev, $i)
}
foreach ($f in 'Invoke-PlaceholderDrag', 'Invoke-LockedDrag') {
    $t = $fnAst[$f].Extent.Text
    $iP = $t.IndexOf('Get-PressPoint'); $iD = $t.IndexOf('Invoke-Drag ')
    Check "$f：先 Get-PressPoint 再 Invoke-Drag" ($iP -ge 0 -and $iD -gt $iP) "press=$iP drag=$iD"
    Check "$f：按下點來自 Get-PressPoint（-FromX `$pt.X -FromY `$pt.Y）" ($t -match 'Invoke-Drag -FromX \$pt\.X -FromY \$pt\.Y')
}
$cursor = @($ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq 'Set-GuardedCursorPos' }, $true))
Check 'Set-GuardedCursorPos 只出現在 Invoke-Drag 內（游標座標只來自拖曳參數）' ($cursor.Count -ge 1 -and @($cursor | Where-Object {
                $p = $_.Parent; while ($p -and -not ($p -is [System.Management.Automation.Language.FunctionDefinitionAst])) { $p = $p.Parent }
                -not $p -or $p.Name -ne 'Invoke-Drag'
            }).Count -eq 0)

# 主流程順序：鎖定時拖曳在進入編輯版面之前；有資料的 fixture 在離開編輯版面之後才寫。
$main = ($ast.EndBlock.Statements | Where-Object { $_ -is [System.Management.Automation.Language.TryStatementAst] } | Select-Object -Last 1).Body.Extent.Text
$iLocked = $main.IndexOf('Invoke-LockedDrag')
$iEditOn = $main.IndexOf("set_edit_mode', { enabled: true })")
$iEditOff = $main.IndexOf("set_edit_mode', { enabled: false })")
$iData = $main.IndexOf('New-Fixture -WithQuotes')
$iBrowser = $main.IndexOf("Invoke-CdpCrash 'browser'")
Check '主流程：鎖定時拖曳（Invoke-LockedDrag）在進入編輯版面之前' ($iLocked -ge 0 -and $iEditOn -gt $iLocked) "locked=$iLocked editOn=$iEditOn"
Check '主流程：故障重建在編輯版面期間（進入之後、離開之前）' ($iBrowser -gt $iEditOn -and $iBrowser -lt $iEditOff) "browser=$iBrowser on=$iEditOn off=$iEditOff"
Check '主流程：有資料的 fixture 在離開編輯版面之後寫入' ($iData -gt $iEditOff) "data=$iData off=$iEditOff"
Check 'PRECONDITION 例外單獨記錄、摘要寫 PRECONDITION FAIL' ($src.Contains("-like 'PRECONDITION*'") -and $src.Contains('PRECONDITION FAIL'))
# review 31c7c53：結束碼經 lib/VerifyVerdict 的 Get-VerdictExitCode，優先序 失敗 1 ＞ BLOCKED 2 ＞ ENV-BLOCKED 3
# （行為本身由 tests/VerifyVerdict.Tests.ps1 驗證）。
Check '結束碼：經 Get-VerdictExitCode（失敗 1、BLOCKED 2、ENV-BLOCKED 3）' ($src -match 'exit \(Get-VerdictExitCode -Results \$results -Locked:\(\[bool\]\$script:blocked\) -EnvBlocked:\(\[bool\]\$script:envBlocked\) -NotRestored \$occNotRestored\)')

# 涵蓋聲明：檔頭（comment-based help）與 README 該節都要列出同一組涵蓋／未涵蓋項目。
$help = ($tokens | Where-Object { $_.Kind -eq 'Comment' } | Select-Object -First 1).Text
$readmeText = [IO.File]::ReadAllText($Readme)
$m = [regex]::Match($readmeText, '(?s)## verify-7\.3-placeholder\.ps1.*?(?=\r?\n## )')
Check 'README 有 verify-7.3-placeholder.ps1 一節' $m.Success
$coverage = '涵蓋', '鎖定時拖不動', '有資料後在原格子出現', 'Reload 後外框仍在', '編輯版面中開啟', '故障重建', '未涵蓋', '建立過程中離開編輯版面',
'看門狗', '沒有時間門檻', '結束碼 4'
foreach ($k in $coverage) {
    Check "涵蓋聲明「$k」：檔頭與 README 都有" ($help.Contains($k) -and $m.Success -and $m.Value.Contains($k)) "help=$($help.Contains($k)) readme=$($m.Success -and $m.Value.Contains($k))"
}

Write-Host "Verify73Placeholder.Tests：$script:Pass passed, $script:Fail failed"
if ($script:Fail -gt 0) { exit 1 }
exit 0
