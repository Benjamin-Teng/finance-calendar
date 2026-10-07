<#
.SYNOPSIS
    host/tools/make-latest-json.ps1 的測試（純 PowerShell 斷言，不需 Pester）。

.DESCRIPTION
    **不啟動 fc-host、不執行任何安裝檔、不碰真實金鑰、不連網**：
    用本機已快取的 tauri-cli（package.ps1 下載的那份）現場產生拋棄式金鑰，對兩個假的「安裝檔」
    （內容只是幾個位元組）簽章，再餵給 make-latest-json.ps1。

    情境：
      - 正常一種（輸出內容、UTF-8 無 BOM、網址固定版本、讀回檢查、不留 .tmp）
      - 四種必須失敗：缺平台、空簽章、網址 tag 不符（-Tag 與版本不符、-Repo 格式、對現成 JSON 檢查 URL）、
        可信註解缺版本（外加版本不符、兩個 .sig 放反、不同金鑰、壞 base64）
      - 與 package.ps1 同名函式的行為對照（兩份各自維護，這裡確保不分岔）
      - 失敗時不留下輸出檔

    tauri-cli 快取不存在時，需要現場簽章的情境標 SKIP。

.EXAMPLE
    pwsh -NoProfile -File host/tools/tests/make-latest-json.Tests.ps1
    結束碼 0＝全部通過、1＝有失敗。
#>
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$toolsDir = Split-Path $PSScriptRoot -Parent
$target = Join-Path $toolsDir 'make-latest-json.ps1'
$packagePs1 = Join-Path $toolsDir 'package.ps1'
$libPs1 = Join-Path $toolsDir 'lib\LatestJson.ps1'

$script:Pass = 0
$script:Fail = 0
$script:Skip = 0
function Check([string]$name, [bool]$cond, [string]$detail = '') {
    if ($cond) { $script:Pass++; Write-Host "PASS  $name" }
    else { $script:Fail++; Write-Host "FAIL  $name $detail" -ForegroundColor Red }
}
function Skip([string]$name, [string]$why) { $script:Skip++; Write-Host "SKIP  $name（$why）" -ForegroundColor Yellow }

$tmpRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("fc-latestjson-tests-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force -Path $tmpRoot | Out-Null
function New-TempDir([string]$name) { $d = Join-Path $tmpRoot $name; New-Item -ItemType Directory -Force -Path $d | Out-Null; $d }
function Write-Utf8NoBom([string]$path, [string]$text) { [System.IO.File]::WriteAllText($path, $text, (New-Object System.Text.UTF8Encoding($false))) }
$pwsh = (Get-Process -Id $PID).Path

# 以語法樹取出某個腳本的函式定義，放進獨立模組（兩個腳本有同名函式，不能載入到同一個範圍）
function Import-ScriptFunctions([string[]]$paths, [string]$moduleName) {
    $texts = @(); $names = @()
    foreach ($path in $paths) {
        $tokens = $null; $errors = $null
        $ast = [System.Management.Automation.Language.Parser]::ParseFile($path, [ref]$tokens, [ref]$errors)
        if (@($errors).Count -gt 0) { throw "解析失敗：$path" }
        $fns = @($ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] }, $false))
        $texts += @($fns | ForEach-Object { $_.Extent.Text })
        $names += @($fns | ForEach-Object { $_.Name })
    }
    @{ Module = (New-Module -Name $moduleName -ScriptBlock ([scriptblock]::Create($texts -join "`n"))); Names = $names }
}

try {
    # ---- 語法與結構
    $tokens = $null; $errors = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseFile($target, [ref]$tokens, [ref]$errors)
    Check 'make-latest-json.ps1 解析 0 錯誤' (@($errors).Count -eq 0) (($errors | ForEach-Object { $_.Message }) -join ' | ')
    $paramNames = @($ast.ParamBlock.Parameters | ForEach-Object { $_.Name.VariablePath.UserPath })
    foreach ($p in 'Version', 'Tag', 'X64Sig', 'Arm64Sig', 'Repo', 'Notes', 'Out') { Check "參數：有 -$p" ($paramNames -contains $p) }
    $repoDefault = @($ast.ParamBlock.Parameters | Where-Object { $_.Name.VariablePath.UserPath -eq 'Repo' })[0].DefaultValue.Value
    Check '參數：-Repo 預設 Benjamin-Teng/finance-calendar' ($repoDefault -eq 'Benjamin-Teng/finance-calendar')
    $mk = Import-ScriptFunctions @($target, $libPs1) 'MakeLatest'
    foreach ($n in 'Get-PlatformTable', 'Test-LatestJson', 'ConvertFrom-JsonKeepDates', 'Test-SignatureText', 'Test-MinisignPubkey', 'Test-Inputs', 'New-LatestJsonObject', 'Invoke-MakeLatestJson') {
        Check "找得到函式 $n" ($mk.Names -contains $n)
    }
    $src = [System.IO.File]::ReadAllText($target)
    $noComments = (($src -replace "`r`n", "`n") -split "`n" | Where-Object { $_ -notmatch '^\s*#' }) -join "`n"
    Check '機密：不讀私鑰、不呼叫 tauri-cli／簽章（只讀 .sig）' ($noComments -notmatch 'signer\s+sign|TAURI_SIGNING|Invoke-WebRequest|Invoke-RestMethod')

    # ---- 平台表（鍵名與宿主的平台鍵一致，D8）
    $table = & $mk.Module { Get-PlatformTable }
    Check '平台表：windows-x86_64 → finance-calendar-setup.exe、windows-aarch64 → finance-calendar-setup-arm64.exe' (
        @($table.Keys).Count -eq 2 -and $table['windows-x86_64'] -eq 'finance-calendar-setup.exe' -and $table['windows-aarch64'] -eq 'finance-calendar-setup-arm64.exe')

    # ---- 輸入檢查（純函式）
    function Inputs([string]$v, [string]$t, [string]$r = 'Benjamin-Teng/finance-calendar') { , @(& $mk.Module { param($a, $b, $c) Test-Inputs $a $b $c } $v $t $r) }
    Check '輸入：版本與 tag 一致 → 通過' ((Inputs '0.1.0' 'v0.1.0').Count -eq 0)
    Check '輸入：預發佈版本 0.2.0-rc.1／v0.2.0-rc.1 → 通過' ((Inputs '0.2.0-rc.1' 'v0.2.0-rc.1').Count -eq 0)
    Check '輸入：tag 缺 v 前綴 → 違規' ((Inputs '0.1.0' '0.1.0').Count -eq 1)
    Check '輸入：tag 版本與 -Version 不同 → 違規' ((Inputs '0.1.0' 'v0.1.1').Count -eq 1)
    Check '輸入：版本不是語意化版本 → 違規' ((Inputs 'abc' 'vabc').Count -ge 1 -and (Inputs '' 'v').Count -ge 1 -and (Inputs '1.2' 'v1.2').Count -ge 1)
    Check '輸入：-Repo 不是 owner/name → 違規' ((Inputs '0.1.0' 'v0.1.0' 'no-slash').Count -eq 1 -and (Inputs '0.1.0' 'v0.1.0' 'a/b/c').Count -eq 1)

    # ---- 與 package.ps1 同名函式的行為對照
    $pk = Import-ScriptFunctions @($packagePs1) 'PackageFns'
    $cliDir = Join-Path $env:LOCALAPPDATA 'fc-host-tools\tauri-cli-2.12.1'
    $cli = Join-Path $cliDir 'cargo-tauri.exe'
    if (-not (Test-Path -LiteralPath $cli)) {
        Skip '需要現場簽章的所有情境' "tauri-cli 快取不存在：$cli（先跑一次 package.ps1 或 package.Tests.ps1 讓它下載）"
    }
    else {
        # ---- 現場產生拋棄式金鑰與簽章
        $kd = New-TempDir 'keys'
        $null = & $cli signer generate --ci -w (Join-Path $kd 'a.key') 2>&1
        $null = & $cli signer generate --ci -w (Join-Path $kd 'b.key') 2>&1
        $files = New-TempDir 'files'
        function New-Sig([string]$name, [string]$key, [string]$ver) {
            # 在獨立資料夾放一個假安裝檔再簽，回傳 .sig 路徑；$ver 為空＝不帶 --app-version
            $d = New-TempDir ("sig-" + [guid]::NewGuid().ToString('N').Substring(0, 6))
            $f = Join-Path $d $name
            Write-Utf8NoBom $f "payload $name $ver"
            $args2 = @('signer', 'sign', '-f', (Join-Path $kd $key))
            if ($ver) { $args2 += @('--app-version', $ver) }
            $null = & $cli @args2 $f 2>&1
            "$f.sig"
        }
        $x64 = 'finance-calendar-setup.exe'; $arm = 'finance-calendar-setup-arm64.exe'
        $sigX = New-Sig $x64 'a.key' '0.1.0'
        $sigA = New-Sig $arm 'a.key' '0.1.0'
        $sigXnoVer = New-Sig $x64 'a.key' ''
        $sigXv11 = New-Sig $x64 'a.key' '0.1.1'
        $sigXkeyB = New-Sig $x64 'b.key' '0.1.0'
        $sigXtext = (Get-Content -LiteralPath $sigX -Raw).Trim()

        # 對照：兩份 helper 對同一個 .sig 的輸出必須一致
        $idMk = & $mk.Module { param($t) Get-MinisignKeyId $t } $sigXtext
        $idPk = & $pk.Module { param($t) Get-MinisignKeyId $t } $sigXtext
        Check '對照：Get-MinisignKeyId 與 package.ps1 的結果相同' ($idMk -eq $idPk -and $idMk -match '^[0-9A-F]{16}$') "$idMk vs $idPk"
        $cMk = & $mk.Module { param($t) Get-SigTrustedCommentFromText $t } $sigXtext
        $cPk = & $pk.Module { param($p) Get-SigTrustedComment $p } $sigX
        Check '對照：可信註解解碼與 package.ps1 的結果相同（含 version:0.1.0）' ($cMk -eq $cPk -and $cMk -match "version:0\.1\.0") "$cMk vs $cPk"
        $fMk = & $mk.Module { param($c) (ConvertFrom-TrustedComment $c).GetEnumerator() | Sort-Object Name | ForEach-Object { "$($_.Name)=$($_.Value)" } } $cMk
        $fPk = & $pk.Module { param($c) (ConvertFrom-TrustedComment $c).GetEnumerator() | Sort-Object Name | ForEach-Object { "$($_.Name)=$($_.Value)" } } $cMk
        Check '對照：ConvertFrom-TrustedComment 與 package.ps1 的結果相同' (($fMk -join '|') -eq ($fPk -join '|') -and @($fMk).Count -ge 3) "$($fMk -join '|') vs $($fPk -join '|')"

        $pubA = (Get-Content (Join-Path $kd 'a.key.pub') -Raw).Trim()
        $pubChecks = foreach ($v in $pubA, 'PRODUCTION_PUBKEY_PENDING_TASK_1_1', '', 'not base64 !!!') {
            $m1 = & $mk.Module { param($t) Test-MinisignPubkey $t } $v
            $m2 = & $pk.Module { param($t) Test-MinisignPubkey $t } $v
            $m1 -eq $m2
        }
        Check '對照：Test-MinisignPubkey 與 package.ps1 的結果相同（真公鑰／占位字串／空字串／壞 base64）' (@($pubChecks | Where-Object { -not $_ }).Count -eq 0)
        Check '對照：Test-MinisignPubkey 接受 tauri 產生的真公鑰' (& $mk.Module { param($t) Test-MinisignPubkey $t } $pubA)

        function Run-Make([string[]]$MakeArgs) {
            $out = & $pwsh -NoProfile -File $target @MakeArgs 2>&1 | Out-String
            @{ Code = $LASTEXITCODE; Out = $out }
        }

        # ---- 正常
        $outDir = New-TempDir 'out-ok'
        $outFile = Join-Path $outDir 'latest.json'
        $notes = "財經日曆 0.1.0：第一版 `"引號`" <b>&</b> 'x'"
        $r = Run-Make @('-Version', '0.1.0', '-Tag', 'v0.1.0', '-X64Sig', $sigX, '-Arm64Sig', $sigA, '-Notes', $notes, '-Out', $outFile)
        Check '正常：結束碼 0、輸出檔存在、沒有殘留 .tmp' ($r.Code -eq 0 -and (Test-Path -LiteralPath $outFile) -and -not (Test-Path -LiteralPath "$outFile.tmp")) "code=$($r.Code) $($r.Out)"
        if (Test-Path -LiteralPath $outFile) {
            $bytes = [System.IO.File]::ReadAllBytes($outFile)
            Check '正常：UTF-8 無 BOM' (-not ($bytes.Length -ge 3 -and $bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF))
            $j = & $mk.Module { param($t) ConvertFrom-JsonKeepDates $t } ([System.IO.File]::ReadAllText($outFile, [System.Text.Encoding]::UTF8))
            Check '正常：version、notes（含中文與特殊字元）原樣保留' ($j.version -eq '0.1.0' -and $j.notes -eq $notes) $j.notes
            Check '正常：pub_date 是 RFC 3339 UTC 且在最近 5 分鐘內' (
                $j.pub_date -match '^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$' -and
                ([DateTime]::UtcNow - [DateTime]::Parse($j.pub_date, [Globalization.CultureInfo]::InvariantCulture, [Globalization.DateTimeStyles]::AdjustToUniversal)).TotalMinutes -lt 5) $j.pub_date
            Check '正常：只有兩個平台 windows-x86_64／windows-aarch64' ((@($j.platforms.PSObject.Properties.Name) -join ',') -eq 'windows-x86_64,windows-aarch64')
            Check '正常：網址為 releases/download/v0.1.0/ 固定版本路徑（不是 latest）' (
                $j.platforms.'windows-x86_64'.url -eq 'https://github.com/Benjamin-Teng/finance-calendar/releases/download/v0.1.0/finance-calendar-setup.exe' -and
                $j.platforms.'windows-aarch64'.url -eq 'https://github.com/Benjamin-Teng/finance-calendar/releases/download/v0.1.0/finance-calendar-setup-arm64.exe' -and
                $r.Out -notmatch 'releases/latest')
            Check '正常：signature 等於 .sig 檔內容（逐字，Trim 後）' ($j.platforms.'windows-x86_64'.signature -eq $sigXtext -and $j.platforms.'windows-aarch64'.signature -eq (Get-Content -LiteralPath $sigA -Raw).Trim())
            $errsOk = @(& $mk.Module { param($o, $v, $t, $rp) Test-LatestJson $o $v $t $rp } $j '0.1.0' 'v0.1.0' 'Benjamin-Teng/finance-calendar')
            Check '正常：產出的檔案通過 Test-LatestJson' ($errsOk.Count -eq 0) ($errsOk -join '|')
            Check '正常：每行以 LF 結尾、檔案以換行結束' ([System.IO.File]::ReadAllText($outFile).EndsWith("`n"))

            # ---- 對現成 JSON 的 URL 檢查（網址 tag 不符）
            function Test-Mutated([scriptblock]$mutate) {
                $copy = & $mk.Module { param($t) ConvertFrom-JsonKeepDates $t } ([System.IO.File]::ReadAllText($outFile, [System.Text.Encoding]::UTF8))
                & $mutate $copy
                , @(& $mk.Module { param($o, $v, $t, $rp) Test-LatestJson $o $v $t $rp } $copy '0.1.0' 'v0.1.0' 'Benjamin-Teng/finance-calendar')
            }
            $e = Test-Mutated { param($o) $o.platforms.'windows-aarch64'.url = $o.platforms.'windows-aarch64'.url -replace 'v0\.1\.0', 'v0.0.9' }
            Check 'URL 檢查：ARM64 網址指到別的 tag → 違規並指出平台' ($e.Count -eq 1 -and $e[0] -match 'windows-aarch64' -and $e[0] -match 'v0\.0\.9') ($e -join '|')
            $e = Test-Mutated { param($o) $o.platforms.'windows-x86_64'.url = $o.platforms.'windows-x86_64'.url -replace 'download/v0\.1\.0', 'latest/download' }
            Check 'URL 檢查：網址改用 latest 路徑 → 違規' ($e.Count -eq 1 -and $e[0] -match 'windows-x86_64') ($e -join '|')
            $e = Test-Mutated { param($o) $o.platforms.'windows-x86_64'.url = $o.platforms.'windows-x86_64'.url -replace 'Benjamin-Teng', 'someone-else' }
            Check 'URL 檢查：倉庫不同 → 違規' ($e.Count -eq 1)
            $e = Test-Mutated { param($o) $o.platforms.'windows-x86_64'.url = $o.platforms.'windows-x86_64'.url -replace '^https', 'http' }
            Check 'URL 檢查：http（非 https）→ 違規' ($e.Count -eq 1)
            $e = Test-Mutated { param($o) $o.platforms.'windows-aarch64'.url = $o.platforms.'windows-x86_64'.url }
            Check 'URL 檢查：兩平台網址相同（ARM64 指到 x64 安裝檔）→ 違規' ($e.Count -eq 1)
            $e = Test-Mutated { param($o) $o.version = '0.1.1' }
            Check '內容檢查：version 與期望不同 → 違規' ($e.Count -ge 1)
            $e = Test-Mutated { param($o) $o.platforms.'windows-x86_64'.signature = '' }
            Check '內容檢查：簽章被清空 → 違規（空簽章）' ($e.Count -eq 1 -and $e[0] -match '空')
            $e = Test-Mutated { param($o) $o.platforms.PSObject.Properties.Remove('windows-aarch64') }
            Check '內容檢查：少一個平台 → 違規' ($e.Count -eq 1 -and $e[0] -match 'windows-aarch64')
            $e = Test-Mutated { param($o) $o.platforms | Add-Member -NotePropertyName 'linux-x86_64' -NotePropertyValue ([pscustomobject]@{ signature = 'x'; url = 'y' }) }
            Check '內容檢查：多出未預期的平台 → 違規' ($e.Count -eq 1 -and $e[0] -match 'linux-x86_64')
            $e = Test-Mutated { param($o) $o.pub_date = 'yesterday' }
            Check '內容檢查：pub_date 格式不對 → 違規' ($e.Count -eq 1)
        }

        # ---- 失敗情境（每個都要：結束碼非 0、指出原因、不留下輸出檔）
        function Expect-Fail([string]$name, [string[]]$MakeArgs, [string]$pattern) {
            $d = New-TempDir ("fail-" + [guid]::NewGuid().ToString('N').Substring(0, 6))
            $o = Join-Path $d 'latest.json'
            $r = Run-Make ($MakeArgs + @('-Out', $o))
            Check "失敗：$name" ($r.Code -ne 0 -and $r.Out -match $pattern -and -not (Test-Path -LiteralPath $o) -and -not (Test-Path -LiteralPath "$o.tmp")) "code=$($r.Code) out=$($r.Out)"
        }
        $base = @('-Version', '0.1.0', '-Tag', 'v0.1.0')
        # (1) 缺平台
        Expect-Fail '缺平台：沒給 -Arm64Sig' ($base + @('-X64Sig', $sigX)) 'windows-aarch64'
        Expect-Fail '缺平台：沒給 -X64Sig' ($base + @('-Arm64Sig', $sigA)) 'windows-x86_64'
        Expect-Fail '缺平台：.sig 檔不存在' ($base + @('-X64Sig', $sigX, '-Arm64Sig', (Join-Path $files 'nope.sig'))) '不存在'
        # (2) 空簽章
        $emptySig = Join-Path $files 'empty.sig'; Write-Utf8NoBom $emptySig ''
        $blankSig = Join-Path $files 'blank.sig'; Write-Utf8NoBom $blankSig "   `r`n"
        Expect-Fail '空簽章：0 位元組的 .sig' ($base + @('-X64Sig', $sigX, '-Arm64Sig', $emptySig)) '空'
        Expect-Fail '空簽章：只有空白的 .sig' ($base + @('-X64Sig', $blankSig, '-Arm64Sig', $sigA)) '空'
        # (3) 網址 tag 不符
        Expect-Fail '網址 tag 不符：-Tag 與 -Version 不同（v0.1.1 vs 0.1.0）' @('-Version', '0.1.0', '-Tag', 'v0.1.1', '-X64Sig', $sigX, '-Arm64Sig', $sigA) 'Tag'
        Expect-Fail '網址 tag 不符：-Tag 缺 v 前綴' @('-Version', '0.1.0', '-Tag', '0.1.0', '-X64Sig', $sigX, '-Arm64Sig', $sigA) 'Tag'
        Expect-Fail '網址不符：-Repo 格式錯誤' ($base + @('-X64Sig', $sigX, '-Arm64Sig', $sigA, '-Repo', 'not a repo')) 'Repo'
        # (4) 可信註解缺版本（外加：版本不符、放反、不同金鑰、壞資料）
        Expect-Fail '可信註解缺版本：x64 的 .sig 沒帶 --app-version' ($base + @('-X64Sig', $sigXnoVer, '-Arm64Sig', $sigA)) 'version 欄位'
        Expect-Fail '可信註解版本不符：.sig 簽的是 0.1.1' ($base + @('-X64Sig', $sigXv11, '-Arm64Sig', $sigA)) '0\.1\.1'
        Expect-Fail '可信註解 file 不符：兩個 .sig 放反' ($base + @('-X64Sig', $sigA, '-Arm64Sig', $sigX)) 'file'
        Expect-Fail '兩個 .sig 由不同金鑰簽 → key ID 不同' ($base + @('-X64Sig', $sigXkeyB, '-Arm64Sig', $sigA)) 'key ID'
        $junk = Join-Path $files 'junk.sig'; Write-Utf8NoBom $junk '%%% not base64 %%%'
        Expect-Fail '壞資料：.sig 不是 base64' ($base + @('-X64Sig', $junk, '-Arm64Sig', $sigA)) 'base64'
        $notMini = Join-Path $files 'notmini.sig'; Write-Utf8NoBom $notMini ([Convert]::ToBase64String([System.Text.Encoding]::UTF8.GetBytes("hello`nworld`n")))
        Expect-Fail '壞資料：base64 但不是 minisign 簽章' ($base + @('-X64Sig', $notMini, '-Arm64Sig', $sigA)) 'trusted comment'
        # 多個錯誤一次列出
        $dd = New-TempDir 'multi'
        $rm = Run-Make ($base + @('-X64Sig', $sigXnoVer, '-Arm64Sig', $emptySig, '-Out', (Join-Path $dd 'latest.json')))
        # （輸入階段只擋缺檔；簽章內容的多項錯誤在組好後檢查時一起列出）
        Check '多項錯誤：x64 缺 version 與 ARM64 空簽章同時列出' ($rm.Code -ne 0 -and $rm.Out -match 'windows-x86_64' -and $rm.Out -match 'windows-aarch64') $rm.Out

        # ---- 相對 -Out 以 PowerShell 的目前位置解析
        $relDir = New-TempDir 'relative'
        $rr = & $pwsh -NoProfile -Command "Set-Location -LiteralPath '$relDir'; & '$target' -Version 0.1.0 -Tag v0.1.0 -X64Sig '$sigX' -Arm64Sig '$sigA' -Out 'sub\rel.json'; exit `$LASTEXITCODE" 2>&1 | Out-String
        Check '相對 -Out：寫到目前位置下（含自動建立子資料夾）' ($LASTEXITCODE -eq 0 -and (Test-Path -LiteralPath (Join-Path $relDir 'sub\rel.json'))) "code=$LASTEXITCODE $rr"
        # 覆寫既有檔：失敗時不得破壞原檔
        $keep = Join-Path $relDir 'keep.json'; Write-Utf8NoBom $keep '{"keep":true}'
        $rk = Run-Make ($base + @('-X64Sig', $sigXnoVer, '-Arm64Sig', $sigA, '-Out', $keep))
        Check '失敗時不破壞既有輸出檔' ($rk.Code -ne 0 -and [System.IO.File]::ReadAllText($keep) -eq '{"keep":true}')
    }
}
finally {
    Remove-Item -LiteralPath $tmpRoot -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host ''
Write-Host "$($script:Pass) passed, $($script:Fail) failed, $($script:Skip) skipped"
if ($script:Fail -gt 0) { exit 1 }
exit 0
