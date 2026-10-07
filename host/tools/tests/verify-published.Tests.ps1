<#
.SYNOPSIS
    host/tools/verify-published.ps1 的測試（純 PowerShell 斷言，不需 Pester）。

.DESCRIPTION
    **不連到 GitHub、不啟動任何安裝檔、不碰真實金鑰**：下載一律以注入的 scriptblock 對應到本機檔案；
    金鑰是用快取的 tauri-cli 現場產生的拋棄式金鑰，簽的「安裝檔」只是以 MZ 開頭的幾個位元組。
    minisign 走 Initialize-Minisign（釘版本與 SHA256；快取沒有時下載一次，這是本檔唯一的網路存取）。

    涵蓋：正常通過；安裝檔被竄改、用別把公鑰驗、下載失敗、下載到非執行檔、latest.json 內容錯（tag／網址／簽章）時各自失敗；
    等 latest.json 到位的重試邏輯（不實際 sleep）；端點網址解析；minisign 釘選常數。
    tauri-cli 快取或 minisign 取得不到時，需要它們的情境標 SKIP。

.EXAMPLE
    pwsh -NoProfile -File host/tools/tests/verify-published.Tests.ps1
    結束碼 0＝全部通過、1＝有失敗。
#>
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$toolsDir = Split-Path $PSScriptRoot -Parent
$target = Join-Path $toolsDir 'verify-published.ps1'
$libPs1 = Join-Path $toolsDir 'lib\LatestJson.ps1'
$makeJson = Join-Path $toolsDir 'make-latest-json.ps1'

$script:Pass = 0
$script:Fail = 0
$script:Skip = 0
function Check([string]$name, [bool]$cond, [string]$detail = '') {
    if ($cond) { $script:Pass++; Write-Host "PASS  $name" }
    else { $script:Fail++; Write-Host "FAIL  $name $detail" -ForegroundColor Red }
}
function Skip([string]$name, [string]$why) { $script:Skip++; Write-Host "SKIP  $name（$why）" -ForegroundColor Yellow }

$tmpRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("fc-verifypub-tests-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force -Path $tmpRoot | Out-Null
function New-TempDir([string]$name) { $d = Join-Path $tmpRoot $name; New-Item -ItemType Directory -Force -Path $d | Out-Null; $d }
$pwsh = (Get-Process -Id $PID).Path

try {
    # ---- 語法與函式載入
    $tokens = $null; $errors = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseFile($target, [ref]$tokens, [ref]$errors)
    Check 'verify-published.ps1 解析 0 錯誤' (@($errors).Count -eq 0) (($errors | ForEach-Object { $_.Message }) -join ' | ')
    $fns = @($ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] }, $false))
    $texts = @($fns | ForEach-Object { $_.Extent.Text })
    $libAst = [System.Management.Automation.Language.Parser]::ParseFile($libPs1, [ref]$tokens, [ref]$errors)
    $texts += @($libAst.FindAll({ param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] }, $false) | ForEach-Object { $_.Extent.Text })
    $vp = New-Module -Name VerifyPub -ScriptBlock ([scriptblock]::Create($texts -join "`n"))
    foreach ($n in 'Get-MinisignConstants', 'Initialize-Minisign', 'Get-RepoFromEndpoint', 'Wait-ForLatestJson', 'Test-PeHeader', 'Test-PublishedRelease', 'Invoke-VerifyPublished') {
        Check "找得到函式 $n" (@($fns | Where-Object { $_.Name -eq $n }).Count -eq 1)
    }
    $paramNames = @($ast.ParamBlock.Parameters | ForEach-Object { $_.Name.VariablePath.UserPath })
    Check '參數：-ExpectedTag／-Repo／-ConfPath／-WorkDir／-MaxAttempts／-RetrySeconds' (@('ExpectedTag', 'Repo', 'ConfPath', 'WorkDir', 'MaxAttempts', 'RetrySeconds' | Where-Object { $paramNames -notcontains $_ }).Count -eq 0)
    $src = [System.IO.File]::ReadAllText($target)
    $noComments = (($src -replace "`r`n", "`n") -split "`n" | Where-Object { $_ -notmatch '^\s*#' }) -join "`n"
    Check '機密：不碰私鑰（沒有 signer sign、TAURI_SIGNING、私鑰路徑）' ($noComments -notmatch 'signer\s+sign|TAURI_SIGNING|\.fc-host-signing')
    Check '下載：只接受 https 網址' ($noComments -match "StartsWith\('https://'\)")

    # ---- minisign 釘選常數
    $c = & $vp { Get-MinisignConstants }
    Check 'minisign 常數：版本 0.12、SHA256 為 64 位十六進位、網址含版本與檔名' (
        $c.Version -eq '0.12' -and $c.Sha256 -match '^[0-9a-f]{64}$' -and $c.Url -eq "https://github.com/jedisct1/minisign/releases/download/$($c.Version)/$($c.Zip)" -and $c.Zip -eq 'minisign-0.12-win64.zip')

    # ---- 端點解析
    $ep = 'https://github.com/Benjamin-Teng/finance-calendar/releases/latest/download/latest.json'
    Check '端點解析：標準端點 → owner/repo' ((& $vp { param($e) Get-RepoFromEndpoint $e } $ep) -eq 'Benjamin-Teng/finance-calendar')
    foreach ($bad in 'http://github.com/a/b/releases/latest/download/latest.json', 'https://example.com/a/b/releases/latest/download/latest.json',
        'https://github.com/a/b/releases/download/v1/latest.json', 'https://github.com/a/b/releases/latest/download/other.json', '') {
        $threw = $false; try { & $vp { param($e) Get-RepoFromEndpoint $e } $bad | Out-Null } catch { $threw = $true }
        Check "端點解析：不合形式 → 丟例外（$bad）" $threw
    }

    # ---- 等 latest.json 到位（不實際 sleep）
    $sleeps = [System.Collections.Generic.List[int]]::new()
    $sleeper = { param($s) $sleeps.Add($s) }
    $mkText = { param($v) "{`"version`":`"$v`"}" }
    $script:calls = 0
    $seq = @('0.0.9', '0.0.9', '0.1.0')
    $fetchSeq = { $i = $script:calls; $script:calls++; & $mkText $seq[[Math]::Min($i, 2)] }
    $got = & $vp { param($f, $a, $b, $c2, $d) Wait-ForLatestJson $f $a $b $c2 $d } $fetchSeq '0.1.0' 5 30 $sleeper
    Check '等待：前兩次是舊版、第三次到位 → 回傳新內容、睡兩次、每次 30 秒' ($got -match '0\.1\.0' -and $script:calls -eq 3 -and $sleeps.Count -eq 2 -and $sleeps[0] -eq 30) "calls=$script:calls sleeps=$($sleeps -join ',')"
    $sleeps.Clear(); $script:calls = 0
    $threw = ''; try { & $vp { param($f, $a, $b, $c2, $d) Wait-ForLatestJson $f $a $b $c2 $d } { $script:calls++; & $mkText '0.0.9' } '0.1.0' 3 7 $sleeper | Out-Null } catch { $threw = $_.Exception.Message }
    Check '等待：一直是舊版 → 用盡次數後丟例外（最後一次不睡）、訊息帶看到的版本' ($threw -match '0\.0\.9' -and $threw -match '0\.1\.0' -and $script:calls -eq 3 -and $sleeps.Count -eq 2) "$threw calls=$script:calls sleeps=$($sleeps.Count)"
    $sleeps.Clear(); $script:calls = 0
    $got = & $vp { param($f, $a, $b, $c2, $d) Wait-ForLatestJson $f $a $b $c2 $d } { $script:calls++; if ($script:calls -lt 3) { throw '404' }; & $mkText '0.1.0' } '0.1.0' 5 1 $sleeper
    Check '等待：前兩次取得失敗（404）也重試，第三次成功' ($got -match '0\.1\.0' -and $script:calls -eq 3)
    $sleeps.Clear(); $script:calls = 0
    $got = & $vp { param($f, $a, $b, $c2, $d) Wait-ForLatestJson $f $a $b $c2 $d } { $script:calls++; & $mkText '0.0.1' } '' 5 1 $sleeper
    Check '等待：沒有期望版本 → 第一份可解析的就回傳、不睡' ($script:calls -eq 1 -and $sleeps.Count -eq 0 -and $got -match '0\.0\.1')
    $threw = ''; try { & $vp { param($f, $a, $b, $c2, $d) Wait-ForLatestJson $f $a $b $c2 $d } { 'not json {' } '0.1.0' 2 1 $sleeper | Out-Null } catch { $threw = $_.Exception.Message }
    Check '等待：回傳壞 JSON → 視為失敗並重試到用盡' ($threw -match '解析失敗|JSON' -or $threw -match '等了 2 次') $threw

    # ---- PE 標頭
    $pe = Join-Path $tmpRoot 'pe.bin'; [System.IO.File]::WriteAllBytes($pe, [byte[]](0x4D, 0x5A, 0x90))
    $html = Join-Path $tmpRoot 'html.bin'; [System.IO.File]::WriteAllText($html, '<html>Not Found</html>')
    Check 'PE 標頭：MZ 開頭 → true；HTML 錯誤頁 → false' ((& $vp { param($p) Test-PeHeader $p } $pe) -and -not (& $vp { param($p) Test-PeHeader $p } $html))

    # ---- 活的驗證（需要 tauri-cli 快取與 minisign）
    $cli = Join-Path $env:LOCALAPPDATA 'fc-host-tools\tauri-cli-2.12.1\cargo-tauri.exe'
    $minisign = $null
    if (Test-Path -LiteralPath $cli) { try { $minisign = & $vp { Initialize-Minisign } } catch { Write-Host "取得 minisign 失敗：$($_.Exception.Message)" } }
    if (-not (Test-Path -LiteralPath $cli)) { Skip '活的驗簽情境' "tauri-cli 快取不存在：$cli" }
    elseif (-not $minisign) { Skip '活的驗簽情境' '取得不到 minisign（需連網下載一次釘選版本）' }
    else {
        Check 'Initialize-Minisign：回傳的 minisign.exe 可執行且版本為 0.12' ((& $minisign -v 2>&1 | Out-String) -match '0\.12')

        $kd = New-TempDir 'keys'
        $null = & $cli signer generate --ci -w (Join-Path $kd 'a.key') 2>&1
        $null = & $cli signer generate --ci -w (Join-Path $kd 'b.key') 2>&1
        $pubA = (Get-Content (Join-Path $kd 'a.key.pub') -Raw).Trim()
        $pubB = (Get-Content (Join-Path $kd 'b.key.pub') -Raw).Trim()

        function New-Release([string]$name, [string]$version, [string]$key = 'a.key') {
            # 產生 <name> 資料夾：兩個以 MZ 開頭的假安裝檔、對應 .sig、組好的 latest.json；回傳資料夾路徑
            $d = New-TempDir $name
            foreach ($fn in 'finance-calendar-setup.exe', 'finance-calendar-setup-arm64.exe') {
                $bytes = [byte[]](0x4D, 0x5A) + [System.Text.Encoding]::UTF8.GetBytes("fake installer $fn $version") + (New-Object byte[] 2048)
                [System.IO.File]::WriteAllBytes((Join-Path $d $fn), $bytes)
                $null = & $cli signer sign --app-version $version -f (Join-Path $kd $key) (Join-Path $d $fn) 2>&1
            }
            $null = & $pwsh -NoProfile -File $makeJson -Version $version -Tag "v$version" -X64Sig "$d\finance-calendar-setup.exe.sig" -Arm64Sig "$d\finance-calendar-setup-arm64.exe.sig" -Out "$d\latest.json" 2>&1
            if ($LASTEXITCODE -ne 0) { throw "組 latest.json 失敗：$name" }
            $d
        }
        # 下載器：把 https://github.com/<repo>/releases/download/<tag>/<file> 對應到資料夾裡的同名檔
        function New-Downloader([string]$dir, [hashtable]$override = @{}) {
            { param($url, $out) $f = Split-Path $url -Leaf; if ($override.ContainsKey($f)) { & $override[$f] $out; return }; Copy-Item -LiteralPath (Join-Path $dir $f) -Destination $out -Force }.GetNewClosure()
        }
        function Verify([string]$dir, [string]$tag, [string]$pub, [scriptblock]$dl, [string]$latestText = '') {
            $text = if ($latestText) { $latestText } else { [System.IO.File]::ReadAllText("$dir\latest.json") }
            $wd = New-TempDir ("work-" + [guid]::NewGuid().ToString('N').Substring(0, 6))
            $pubB64 = $pub   # tauri 的 .pub 檔本身就是 base64（與 tauri.conf.json 的 pubkey 同格式）
            , @(& $vp { param($a, $b, $c2, $d, $e, $f, $g) Test-PublishedRelease $a $b $c2 $d $e $f $g } $text 'Benjamin-Teng/finance-calendar' $tag $pubB64 $minisign $wd $dl)
        }

        $rel = New-Release 'rel-ok' '0.1.0'
        $e = Verify $rel 'v0.1.0' $pubA (New-Downloader $rel)
        Check '驗證：正常的 release（兩個安裝檔以公鑰驗簽、可信註解正確）→ 通過' ($e.Count -eq 0) ($e -join '|')
        $e = Verify $rel '' $pubA (New-Downloader $rel)
        Check '驗證：未指定 tag（手動重跑）→ 以 latest.json 的版本為準、通過' ($e.Count -eq 0) ($e -join '|')

        $e = Verify $rel 'v0.1.1' $pubA (New-Downloader $rel)
        Check '驗證：release 事件的 tag 與 latest.json 版本不同 → 失敗' ($e.Count -ge 1 -and ($e -join '|') -match 'v0\.1\.1')
        $e = Verify $rel 'v0.1.0' $pubB (New-Downloader $rel)
        Check '驗證：用另一把公鑰驗（內建公鑰與簽章金鑰不是同一把）→ 失敗、兩個平台都報' ($e.Count -eq 2 -and ($e -join '|') -match 'windows-x86_64' -and ($e -join '|') -match 'windows-aarch64' -and ($e -join '|') -match 'minisign') ($e -join '|')
        $e = Verify $rel 'v0.1.0' 'PRODUCTION_PUBKEY_PENDING_TASK_1_1' (New-Downloader $rel)
        Check '驗證：公鑰是占位字串 → 失敗並指出公鑰無效（不下載）' ($e.Count -eq 1 -and $e[0] -match '公鑰')

        # 安裝檔被竄改（簽章不變）
        $tamper = {
            param($out)
            $b = [System.IO.File]::ReadAllBytes((Join-Path $rel 'finance-calendar-setup-arm64.exe')); $b[100] = $b[100] -bxor 0xFF
            [System.IO.File]::WriteAllBytes($out, $b)
        }.GetNewClosure()
        $e = Verify $rel 'v0.1.0' $pubA (New-Downloader $rel @{ 'finance-calendar-setup-arm64.exe' = $tamper })
        Check '驗證：ARM64 安裝檔被竄改一個位元組 → 只有 ARM64 失敗（minisign 驗簽失敗）' ($e.Count -eq 1 -and $e[0] -match 'windows-aarch64' -and $e[0] -match 'minisign') ($e -join '|')
        # 下載失敗
        $boom = { param($out) throw 'HTTP 404' }
        $e = Verify $rel 'v0.1.0' $pubA (New-Downloader $rel @{ 'finance-calendar-setup.exe' = $boom })
        Check '驗證：x64 下載失敗（404）→ 失敗並指出平台與網址' ($e.Count -eq 1 -and $e[0] -match 'windows-x86_64' -and $e[0] -match '下載失敗' -and $e[0] -match 'releases/download/v0\.1\.0') ($e -join '|')
        # 下載到 HTML 錯誤頁
        $htmlPage = { param($out) [System.IO.File]::WriteAllText($out, '<html>Not Found</html>') }
        $e = Verify $rel 'v0.1.0' $pubA (New-Downloader $rel @{ 'finance-calendar-setup.exe' = $htmlPage })
        Check '驗證：下載到 HTML 頁面（非 PE）→ 失敗' ($e.Count -eq 1 -and $e[0] -match 'MZ')
        $emptyFile = { param($out) [System.IO.File]::WriteAllBytes($out, [byte[]]@()) }
        $e = Verify $rel 'v0.1.0' $pubA (New-Downloader $rel @{ 'finance-calendar-setup.exe' = $emptyFile })
        Check '驗證：下載到空檔 → 失敗' ($e.Count -eq 1 -and $e[0] -match '空')

        # latest.json 內容錯誤：不下載就失敗
        $latest = [System.IO.File]::ReadAllText("$rel\latest.json")
        $neverDownload = { param($url, $out) throw '不應下載' }
        $e = Verify $rel 'v0.1.0' $pubA $neverDownload ($latest -replace 'download/v0\.1\.0/finance-calendar-setup-arm64', 'download/v0.0.9/finance-calendar-setup-arm64')
        Check '驗證：latest.json 的 ARM64 網址指到別的 tag → 失敗、不下載' ($e.Count -eq 1 -and $e[0] -match 'windows-aarch64' -and $e[0] -notmatch '不應下載') ($e -join '|')
        $rel2 = New-Release 'rel-0.1.1' '0.1.1'
        $mixed = ($latest | ConvertFrom-Json -DateKind String)
        $mixed.platforms.'windows-aarch64'.signature = ([System.IO.File]::ReadAllText("$rel2\latest.json") | ConvertFrom-Json -DateKind String).platforms.'windows-aarch64'.signature
        $e = Verify $rel 'v0.1.0' $pubA $neverDownload ($mixed | ConvertTo-Json -Depth 6)
        Check '驗證：ARM64 簽章是別的版本（0.1.1）的 → 失敗（可信註解版本不符）、不下載' ($e.Count -ge 1 -and ($e -join '|') -match '0\.1\.1' -and ($e -join '|') -notmatch '不應下載') ($e -join '|')
        $e = Verify $rel 'v0.1.0' $pubA $neverDownload '{"version":"0.1.0"}'
        Check '驗證：latest.json 缺欄位 → 失敗、不下載' ($e.Count -ge 1)
        $e = Verify $rel 'v0.1.0' $pubA $neverDownload 'garbage {'
        Check '驗證：latest.json 不是 JSON → 失敗' ($e.Count -eq 1 -and $e[0] -match 'JSON')

        # 簽章金鑰不同：兩個平台由不同金鑰簽（即使內建公鑰是其中一把，另一個平台驗簽失敗）
        $relB = New-Release 'rel-keyB' '0.1.0' 'b.key'
        $mixedKeys = ([System.IO.File]::ReadAllText("$rel\latest.json") | ConvertFrom-Json -DateKind String)
        $mixedKeys.platforms.'windows-aarch64'.signature = ([System.IO.File]::ReadAllText("$relB\latest.json") | ConvertFrom-Json -DateKind String).platforms.'windows-aarch64'.signature
        $e = Verify $rel 'v0.1.0' $pubA $neverDownload ($mixedKeys | ConvertTo-Json -Depth 6)
        Check '驗證：兩個平台的簽章不是同一把金鑰 → 失敗（key ID 不同）、不下載' ($e.Count -ge 1 -and ($e -join '|') -match 'key ID') ($e -join '|')
    }

    # ---- 主流程對照 tauri.conf.json（真實檔）：端點可解析、占位公鑰會被擋
    $realConf = Get-Content -LiteralPath (Join-Path (Split-Path $toolsDir -Parent) 'tauri.conf.json') -Raw | ConvertFrom-Json
    $threw = $false; $repoName = ''
    try { $repoName = & $vp { param($e) Get-RepoFromEndpoint $e } @($realConf.plugins.updater.endpoints)[0] } catch { $threw = $true }
    Check '真實 tauri.conf.json：更新端點符合驗證腳本要求的形式' (-not $threw -and $repoName -eq 'Benjamin-Teng/finance-calendar') "$threw $repoName"
}
finally {
    Remove-Item -LiteralPath $tmpRoot -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host ''
Write-Host "$($script:Pass) passed, $($script:Fail) failed, $($script:Skip) skipped"
if ($script:Fail -gt 0) { exit 1 }
exit 0
