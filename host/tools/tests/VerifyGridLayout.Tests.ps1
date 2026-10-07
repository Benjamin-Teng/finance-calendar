<#
.SYNOPSIS
    host/tools/verify-grid-layout.ps1 的內層裁切檢查（Get-InnerClipExpr、Test-InnerClipResult）測試
    與結構檢查（純 PowerShell 斷言，不需 Pester）。

.DESCRIPTION
    **不執行 verify-grid-layout.ps1**（它會啟動宿主）：以 PowerShell Parser 只取出純函式。
    7.7-low (a)：舊版只比較 `.panel` 自己的 scrollHeight／clientHeight，內層 overflow:hidden 容器
    把內容裁掉時 `.panel` 量起來仍然剛好，偵測不到。內層檢查的 JS 另以 Node 對假 DOM 執行，驗它
    只挑 overflow-y 為 hidden／clip 的子孫元素、比較 scrollHeight 與 clientHeight。

.EXAMPLE
    pwsh -NoProfile -File host/tools/tests/VerifyGridLayout.Tests.ps1
    結束碼 0＝全部通過、1＝有失敗。
#>
param([string]$Target)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$toolsDir = Split-Path $PSScriptRoot -Parent
if (-not $Target) { $Target = Join-Path $toolsDir 'verify-grid-layout.ps1' }

$script:Pass = 0
$script:Fail = 0
function Check([string]$name, [bool]$cond, [string]$detail = '') {
    if ($cond) { $script:Pass++; Write-Host "PASS  $name" }
    else { $script:Fail++; Write-Host "FAIL  $name $detail" -ForegroundColor Red }
}

$tokens = $null
$errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile($Target, [ref]$tokens, [ref]$errors)
Check 'verify-grid-layout.ps1 解析 0 錯誤' (@($errors).Count -eq 0) (($errors | ForEach-Object { $_.Message }) -join ' | ')
$found = @{}
foreach ($name in 'Get-InnerClipExpr', 'Test-InnerClipResult') {
    $fn = $ast.Find({
            param($n)
            $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $name
        }, $true)
    $found[$name] = ($null -ne $fn)
    Check "找得到函式 $name" $found[$name]
    if ($fn) { . ([scriptblock]::Create($fn.Extent.Text)) }
}

if ($found['Test-InnerClipResult']) {
    $r = Test-InnerClipResult '{"checked":3,"clipped":[]}'
    Check '沒有被裁切的內層容器 → Ok' ($r.Ok -and @($r.Bad).Count -eq 0) ($r | Out-String)
    $r = Test-InnerClipResult '{"checked":0,"clipped":[]}'
    Check '沒有任何 overflow:hidden 內層容器 → Ok（沒東西可裁）' ($r.Ok) ($r | Out-String)
    $r = Test-InnerClipResult '{"checked":4,"clipped":[{"el":"div#clockDate.date","sh":38,"ch":20}]}'
    Check '有內層容器 scrollHeight > clientHeight → 不 Ok、列出元素' (-not $r.Ok -and @($r.Bad).Count -eq 1 -and $r.Bad[0] -match 'div#clockDate\.date' -and $r.Bad[0] -match '38' -and $r.Bad[0] -match '20') ($r | Out-String)
    $r = Test-InnerClipResult '{"checked":-1,"clipped":[]}'
    Check '找不到 .panel（checked=-1）→ 不 Ok' (-not $r.Ok) ($r | Out-String)
    $r = Test-InnerClipResult '找不到 url 含「w=clock」的分頁'
    Check 'CDP 回傳不是 JSON → 不 Ok（不丟例外）' (-not $r.Ok -and @($r.Bad).Count -ge 1) ($r | Out-String)
    $r = Test-InnerClipResult ''
    Check '空字串 → 不 Ok' (-not $r.Ok) ($r | Out-String)
}

if ($found['Get-InnerClipExpr']) {
    $expr = Get-InnerClipExpr
    Check '內層檢查 JS 從 #widget-root > .panel 往下找' ($expr.Contains("#widget-root > .panel"))
    Check '內層檢查 JS 比較 scrollHeight 與 clientHeight' ($expr.Contains('scrollHeight') -and $expr.Contains('clientHeight'))
    Check '內層檢查 JS 讀 computed overflow-y' ($expr.Contains('getComputedStyle') -and $expr.Contains('overflowY'))

    $node = Get-Command node -ErrorAction SilentlyContinue
    Check '找得到 node（以假 DOM 執行 JS）' ($null -ne $node)
    if ($node) {
        # 假 DOM：.panel 底下四個元素——hidden 且溢出、hidden 未溢出、auto 溢出（捲動區，不算）、clip 溢出。
        $harness = @'
const mk = (tag, id, cls, oy, sh, ch) => ({ tagName: tag, id, className: cls, scrollHeight: sh, clientHeight: ch, _oy: oy });
const kids = [
  mk('DIV', 'clockDate', 'date', 'hidden', 38, 20),
  mk('DIV', '', 'row ok', 'hidden', 20, 20),
  mk('DIV', 'dynList', 'evlist', 'auto', 900, 300),
  mk('SPAN', '', '', 'clip', 12, 10),
];
const panel = { querySelectorAll: (s) => (s === '*' ? kids : []) };
globalThis.document = { querySelector: (s) => (s === '#widget-root > .panel' ? (globalThis.__noPanel ? null : panel) : null) };
globalThis.getComputedStyle = (el) => ({ overflowY: el._oy });
const expr = process.env.FC_EXPR;
const a = eval(expr);
globalThis.__noPanel = true;
const b = eval(expr);
console.log(JSON.stringify({ a, b }));
'@
        $tmpJs = Join-Path ([IO.Path]::GetTempPath()) ('vgrid-harness-' + [guid]::NewGuid().ToString('N') + '.cjs')
        try {
            [IO.File]::WriteAllText($tmpJs, $harness)
            $env:FC_EXPR = $expr
            $out = (& node $tmpJs 2>&1) -join "`n"
            $j = $null
            try { $j = $out | ConvertFrom-Json } catch { }
            Check '假 DOM：JS 執行成功且回傳 JSON' ($null -ne $j) $out
            if ($j) {
                Check '假 DOM：只計 overflow-y 為 hidden／clip 的元素（3 個）' ($j.a.checked -eq 3) $out
                $els = @($j.a.clipped | ForEach-Object { $_.el })
                Check '假 DOM：抓到 hidden 溢出的 div#clockDate.date 與 clip 溢出的 span' ($els.Count -eq 2 -and $els -contains 'div#clockDate.date' -and $els -contains 'span') $out
                Check '假 DOM：捲動區（overflow auto）不算裁切' (-not ($els -match 'dynList')) $out
                Check '假 DOM：沒有 .panel 時 checked=-1' ($j.b.checked -eq -1) $out
                $r = Test-InnerClipResult (($j.a | ConvertTo-Json -Compress -Depth 5))
                Check '假 DOM 結果餵進 Test-InnerClipResult → 不 Ok、2 筆' (-not $r.Ok -and @($r.Bad).Count -eq 2) ($r | Out-String)
            }
        } finally {
            Remove-Item $tmpJs -ErrorAction SilentlyContinue
            Remove-Item Env:FC_EXPR -ErrorAction SilentlyContinue
        }
    }
}

# 結構：主流程對每個財經小工具都跑內層檢查，並有獨立的判定項目。
$src = [IO.File]::ReadAllText($Target)
Check '主流程以 Invoke-Eval 執行 Get-InnerClipExpr' ($src -match 'Invoke-Eval\s+"w=\$id"\s+\(Get-InnerClipExpr\)' -or $src -match 'Invoke-Eval\s+"w=\$id"\s+\$innerClipExpr')
Check '主流程以 Test-InnerClipResult 判定' ($src -match 'Test-InnerClipResult\s')
Check '有「內層 overflow:hidden 容器沒有裁切」判定項目' ($src -match "\`$results\['[^']*內層[^']*'\]")

Write-Host "VerifyGridLayout.Tests：$script:Pass passed, $script:Fail failed"
if ($script:Fail -gt 0) { exit 1 }
exit 0
