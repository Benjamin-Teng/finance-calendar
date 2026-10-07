<#
.SYNOPSIS
    host/tools/package.ps1 的純函式測試與 host/installer/hooks.nsh 的解除安裝 hook 行為測試
    （純 PowerShell 斷言，不需 Pester）。

.DESCRIPTION
    **不啟動 fc-host、不執行真正的安裝檔、不碰桌面、不寫真實使用者資料**：
      A. package.ps1 的函式（以語法樹取出）：版本解析、BOM 檢查、公鑰格式、-Release 前置檢查、
         .sig 可信註解解析與驗證（用 tauri-cli 產生的真實 .sig 當樣本）、後門字串搜尋、
         以及腳本結構（參數、機密不外洩）的靜態檢查。
         另以暫存的假 host/ 目錄實際執行 package.ps1，驗證「占位公鑰」「缺 BOM」兩個失敗路徑
         在任何建置之前就停下。
      B. hooks.nsh（需要 makensis，找不到就略過並標 SKIP）：在暫存資料夾以 NSIS 編譯一個測試用
         uninstaller（只含 PREUNINSTALL 巨集）與假的 `fc-host-stub.exe`（依序回傳指定結束碼），
         逐一驗證結束碼處置（0／6 繼續；3、4、1 等待後重跑一次，7 與其他碼直接重跑一次；仍失敗只記錄、不重試第三次；
         找不到主程式；更新模式不動作；起不來時的 "error"；等待宿主跑完而不強制終止；記錄附加）。
         測試用 uninstaller 的記錄目錄與等待時間經 hooks.nsh 的 FC_TEST_LOG_DIR／FC_WAIT_CODE*_MS
         覆寫，全部在暫存資料夾內。**未覆蓋**：訊息框（需要互動視窗）與 POSTINSTALL／POSTUNINSTALL
         （會寫真實的登錄與開始功能表，留給 tasks.md 4.x 的實機安裝測試）。

.EXAMPLE
    pwsh -NoProfile -File host/tools/tests/package.Tests.ps1
    結束碼 0＝全部通過、1＝有失敗。
#>
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$toolsDir = Split-Path $PSScriptRoot -Parent
$hostDir = Split-Path $toolsDir -Parent
$target = Join-Path $toolsDir 'package.ps1'
$hooksPath = Join-Path $hostDir 'installer\hooks.nsh'

$script:Pass = 0
$script:Fail = 0
$script:Skip = 0
function Check([string]$name, [bool]$cond, [string]$detail = '') {
    if ($cond) { $script:Pass++; Write-Host "PASS  $name" }
    else { $script:Fail++; Write-Host "FAIL  $name $detail" -ForegroundColor Red }
}
# 測試用的暫存登錄區（hooks.nsh 的 FC_* 覆寫常數導向這裡）；每個情境前後清掉
function Reset-TestHiveSafe { Remove-Item -LiteralPath 'HKCU:\Software\fc-host-nsis-test' -Recurse -Force -ErrorAction SilentlyContinue }
function Skip([string]$name, [string]$why) { $script:Skip++; Write-Host "SKIP  $name（$why）" -ForegroundColor Yellow }

$tmpRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("fc-package-tests-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force -Path $tmpRoot | Out-Null
function New-TempDir([string]$name) { $d = Join-Path $tmpRoot $name; New-Item -ItemType Directory -Force -Path $d | Out-Null; $d }
function Write-Utf8NoBom([string]$path, [string]$text) { [System.IO.File]::WriteAllText($path, $text, (New-Object System.Text.UTF8Encoding($false))) }

try {
    # ============================================================================================
    # A. package.ps1 函式
    # ============================================================================================
    $tokens = $null
    $errors = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseFile($target, [ref]$tokens, [ref]$errors)
    Check 'package.ps1 解析 0 錯誤' (@($errors).Count -eq 0) (($errors | ForEach-Object { $_.Message }) -join ' | ')
    $fns = @($ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] }, $false))
    foreach ($f in $fns) { . ([scriptblock]::Create($f.Extent.Text)) }
    foreach ($name in 'Get-PackageConstants', 'Get-HostTriple', 'Get-CargoPackageVersion', 'Test-Utf8Bom', 'Test-MinisignPubkey',
        'Get-ReleaseViolations', 'Get-SigTrustedComment', 'Test-SigTrustedComment', 'Find-ForbiddenStrings', 'Get-BackdoorNeedles',
        'ConvertTo-KeyIdDisplay', 'Get-ParameterViolations', 'Invoke-InstallerSigning', 'Get-E2ePubkeys',
        'Initialize-TauriCli', 'Invoke-Package') {
        Check "找得到函式 $name" (@($fns | Where-Object { $_.Name -eq $name }).Count -eq 1)
    }

    # ---- 釘選常數
    $const = Get-PackageConstants
    Check '常數：tauri-cli 版本為 2.12.1 且 tag 與之對應' ($const.TauriCliVersion -eq '2.12.1' -and $const.TauriCliTag -eq 'tauri-cli-v2.12.1')
    foreach ($t in 'x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc') {
        $a = $const.TauriCliAssets[$t]
        Check "常數：$t 的 SHA256 是 64 位十六進位、zip 檔名含 triple" ($a.Sha256 -match '^[0-9a-f]{64}$' -and $a.Zip -eq "cargo-tauri-$t.zip")
        Check "常數：$t 有發佈檔名" ($const.PublishNames[$t].File -like 'finance-calendar-setup*.exe')
        Check "常數：$t 的 ExeSha256（解壓後 cargo-tauri.exe）是 64 位小寫十六進位" ($a.ExeSha256 -cmatch '^[0-9a-f]{64}$')
    }
    Check '常數：x64 發佈檔名 finance-calendar-setup.exe、ARM64 為 -arm64' (
        $const.PublishNames['x86_64-pc-windows-msvc'].File -eq 'finance-calendar-setup.exe' -and
        $const.PublishNames['aarch64-pc-windows-msvc'].File -eq 'finance-calendar-setup-arm64.exe')
    Check 'Get-HostTriple 回傳支援的 triple' ($const.PublishNames.ContainsKey((Get-HostTriple)))

    # ---- Cargo 版本解析
    $d = New-TempDir 'cargo'
    Write-Utf8NoBom (Join-Path $d 'Cargo.toml') "[package]`nname = `"x`"`nversion = `"1.2.3-rc.1`"`n`n[dependencies]`nfoo = { version = `"9.9.9`" }`nversion = `"8.8.8`"`n"
    Check 'Cargo 版本：只取 [package] 的 version（含預發佈字尾，不被後面的表蓋掉）' ((Get-CargoPackageVersion (Join-Path $d 'Cargo.toml')) -eq '1.2.3-rc.1')
    Write-Utf8NoBom (Join-Path $d 'Cargo2.toml') "[dependencies]`nversion = `"8.8.8`"`n"
    $threw = $false; try { Get-CargoPackageVersion (Join-Path $d 'Cargo2.toml') | Out-Null } catch { $threw = $true }
    Check 'Cargo 版本：[package] 沒有 version → 丟例外' $threw
    $realVer = Get-CargoPackageVersion (Join-Path $hostDir 'Cargo.toml')
    Check 'Cargo 版本：真實 host/Cargo.toml 解得出語意化版本' ($realVer -match '^\d+\.\d+\.\d+')

    # ---- BOM 檢查
    $bomOk = Join-Path $d 'bom.nsh'
    [System.IO.File]::WriteAllBytes($bomOk, [byte[]](0xEF, 0xBB, 0xBF, 0x3B, 0x0A))
    $bomNo = Join-Path $d 'nobom.nsh'
    [System.IO.File]::WriteAllBytes($bomNo, [byte[]](0x3B, 0x20, 0xE8, 0xB2, 0xA1))
    $bomShort = Join-Path $d 'short.nsh'
    [System.IO.File]::WriteAllBytes($bomShort, [byte[]](0xEF, 0xBB))
    $bomEmpty = Join-Path $d 'empty.nsh'
    [System.IO.File]::WriteAllBytes($bomEmpty, [byte[]]@())
    Check 'BOM：有 EF BB BF → true' (Test-Utf8Bom $bomOk)
    Check 'BOM：沒有 BOM → false' (-not (Test-Utf8Bom $bomNo))
    Check 'BOM：只有前兩個位元組 → false' (-not (Test-Utf8Bom $bomShort))
    Check 'BOM：空檔 → false' (-not (Test-Utf8Bom $bomEmpty))
    Check 'BOM：檔案不存在 → false' (-not (Test-Utf8Bom (Join-Path $d 'nope.nsh')))
    Check 'BOM：真實 hooks.nsh 以 EF BB BF 開頭' (Test-Utf8Bom $hooksPath)

    # ---- minisign 公鑰格式
    $pubText = "untrusted comment: minisign public key: 9652A98D711CF3D5`nRWTV8xxxjalSlnT/eO5XhFFZPDwpwoFPSNuWvmVqbPMGkoysN9S32glE`n"
    $goodPub = [Convert]::ToBase64String([System.Text.Encoding]::UTF8.GetBytes($pubText))
    Check '公鑰：真正的 minisign 公鑰格式 → 有效' (Test-MinisignPubkey $goodPub)
    Check '公鑰：前後空白容許' (Test-MinisignPubkey "  $goodPub`n")
    foreach ($bad in 'PRODUCTION_PUBKEY_PENDING_TASK_1_1', 'E2E_TEST_PUBKEY_PLACEHOLDER', '', '   ', 'not base64 !!!',
        [Convert]::ToBase64String([System.Text.Encoding]::UTF8.GetBytes("untrusted comment: x`nshort`n")),
        [Convert]::ToBase64String([System.Text.Encoding]::UTF8.GetBytes('hello world'))) {
        Check "公鑰：占位／壞值「$bad」→ 無效" (-not (Test-MinisignPubkey $bad))
    }
    Check '公鑰：$null → 無效' (-not (Test-MinisignPubkey $null))
    foreach ($ph in $const.PubkeyPlaceholders) { Check "公鑰：占位常數「$ph」被判無效" (-not (Test-MinisignPubkey $ph)) }
    $prodConf = Get-Content (Join-Path $hostDir 'tauri.conf.json') -Raw | ConvertFrom-Json
    $e2eConf = Get-Content (Join-Path $hostDir 'tauri.e2e.conf.json') -Raw | ConvertFrom-Json
    Check '公鑰：repo 內 e2e 設定檔的公鑰是占位字串（不得提交測試公鑰）' (-not (Test-MinisignPubkey $e2eConf.plugins.updater.pubkey))
    Check '設定：tauri.conf.json 不寫 version（版本只有 Cargo.toml 一個來源）' (-not ($prodConf.PSObject.Properties.Name -contains 'version'))
    Check '設定：e2e 設定檔端點為 http://127.0.0.1:8737/latest.json' (@($e2eConf.plugins.updater.endpoints) -contains 'http://127.0.0.1:8737/latest.json')

    # ---- -Release 前置檢查
    $confDir = New-TempDir 'conf'
    $confPlaceholder = Join-Path $confDir 'placeholder.json'
    $httpsEp = '"endpoints":["https://example.com/latest.json"]'
    Write-Utf8NoBom $confPlaceholder ('{"plugins":{"updater":{"pubkey":"PRODUCTION_PUBKEY_PENDING_TASK_1_1",' + $httpsEp + '}}}')
    $confGood = Join-Path $confDir 'good.json'
    Write-Utf8NoBom $confGood ('{"plugins":{"updater":{"pubkey":"' + $goodPub + '",' + $httpsEp + '}}}')
    $confNoKey = Join-Path $confDir 'nokey.json'
    Write-Utf8NoBom $confNoKey '{"plugins":{}}'
    $v = @(Get-ReleaseViolations $confPlaceholder @() @() 'update-e2e')
    Check '-Release：占位公鑰 → 違規（訊息提到 pubkey 與占位字串）' ($v.Count -eq 1 -and $v[0] -match 'pubkey' -and $v[0] -match 'PRODUCTION_PUBKEY_PENDING_TASK_1_1') ($v -join '|')
    Check '-Release：沒有 plugins.updater.pubkey → 違規' (@(Get-ReleaseViolations $confNoKey @() @() 'update-e2e').Count -eq 1)
    Check '-Release：設定檔不存在 → 違規' (@(Get-ReleaseViolations (Join-Path $confDir 'nope.json') @() @() 'update-e2e').Count -ge 1)
    Check '-Release：有效公鑰、無 -Config、無 e2e feature → 通過' (@(Get-ReleaseViolations $confGood @() @() 'update-e2e').Count -eq 0)
    Check '-Release：有效公鑰但帶 -Config → 違規' (@(Get-ReleaseViolations $confGood @('x.json') @() 'update-e2e').Count -eq 1)
    Check '-Release：有效公鑰但帶 update-e2e feature → 違規' (@(Get-ReleaseViolations $confGood @() @('other', 'update-e2e') 'update-e2e').Count -eq 1)
    Check '-Release：其他 feature 不違規' (@(Get-ReleaseViolations $confGood @() @('some-feature') 'update-e2e').Count -eq 0)
    # 最終審查 M3：驗收／量測用的 self-test-ipc、probe-render 也不得進正式版（含逗號串接）
    $vSelfTest = @(Get-ReleaseViolations $confGood @() @('self-test-ipc') 'update-e2e')
    Check '-Release：帶 self-test-ipc feature → 違規' ($vSelfTest.Count -eq 1 -and $vSelfTest[0] -match 'self-test-ipc') ($vSelfTest -join ' | ')
    $vProbe = @(Get-ReleaseViolations $confGood @() @('a,probe-render') 'update-e2e')
    Check '-Release：帶 probe-render feature（逗號串接）→ 違規' ($vProbe.Count -eq 1 -and $vProbe[0] -match 'probe-render') ($vProbe -join ' | ')
    # cargo 的 `<套件>/<feature>` 寫法也要擋（三個禁用 feature 都適用）
    $vPkgSelf = @(Get-ReleaseViolations $confGood @() @('fc-host/self-test-ipc') 'update-e2e')
    Check '-Release：-Features "fc-host/self-test-ipc" → 違規' ($vPkgSelf.Count -eq 1 -and $vPkgSelf[0] -match 'self-test-ipc') ($vPkgSelf -join ' | ')
    $vPkgE2e = @(Get-ReleaseViolations $confGood @() @('a,fc-host/update-e2e') 'update-e2e')
    Check '-Release：-Features "a,fc-host/update-e2e" → 違規' ($vPkgE2e.Count -eq 1 -and $vPkgE2e[0] -match 'update-e2e') ($vPkgE2e -join ' | ')
    Check '-Release：-Features "fc-host/probe-render" → 違規' (@(Get-ReleaseViolations $confGood @() @('fc-host/probe-render') 'update-e2e').Count -eq 1)
    Check '-Release：-Features "fc-host/some-feature" 不誤擋' (@(Get-ReleaseViolations $confGood @() @('fc-host/some-feature') 'update-e2e').Count -eq 0)
    Check '-Release：多項違規一次列出' (@(Get-ReleaseViolations $confPlaceholder @('e2e.json') @('update-e2e') 'update-e2e').Count -eq 3)
    # update-e2e 以逗號／空白串接（cargo 接受 `--features a,b`）也要擋
    Check '-Release：-Features "a,update-e2e"（逗號串接）→ 違規' (@(Get-ReleaseViolations $confGood @() @('a,update-e2e') 'update-e2e').Count -eq 1)
    Check '-Release：-Features "update-e2e,a"、"a update-e2e b" → 違規' (
        @(Get-ReleaseViolations $confGood @() @('update-e2e,a') 'update-e2e').Count -eq 1 -and @(Get-ReleaseViolations $confGood @() @('a update-e2e b') 'update-e2e').Count -eq 1)
    Check '-Release：名稱只是包含 update-e2e 的別的 feature（update-e2e-extra）不誤擋' (@(Get-ReleaseViolations $confGood @() @('update-e2e-extra,x') 'update-e2e').Count -eq 0)
    # 基礎設定本身的端點與 dangerous* 旗標
    $pubJ = '"pubkey":"' + $goodPub + '"'
    $cNoEp = Join-Path $confDir 'noep.json'; Write-Utf8NoBom $cNoEp ('{"plugins":{"updater":{' + $pubJ + '}}}')
    $cHttp = Join-Path $confDir 'http.json'; Write-Utf8NoBom $cHttp ('{"plugins":{"updater":{' + $pubJ + ',"endpoints":["http://127.0.0.1:8737/latest.json"]}}}')
    $cMixed = Join-Path $confDir 'mixed.json'; Write-Utf8NoBom $cMixed ('{"plugins":{"updater":{' + $pubJ + ',"endpoints":["https://a.example/x.json","http://b.example/y.json"]}}}')
    $cDanger = Join-Path $confDir 'danger.json'; Write-Utf8NoBom $cDanger ('{"plugins":{"updater":{' + $pubJ + ',' + $httpsEp + ',"dangerousInsecureTransportProtocol":true}}}')
    $cDangerFalse = Join-Path $confDir 'dangerfalse.json'; Write-Utf8NoBom $cDangerFalse ('{"plugins":{"updater":{' + $pubJ + ',' + $httpsEp + ',"dangerousInsecureTransportProtocol":false}}}')
    Check '-Release：基礎設定沒有 endpoints → 違規' (@(Get-ReleaseViolations $cNoEp @() @() 'update-e2e').Count -eq 1)
    Check '-Release：基礎設定端點是 http:// → 違規（即使不是 e2e 檔的網址）' (@(Get-ReleaseViolations $cHttp @() @() 'update-e2e').Count -eq 1)
    Check '-Release：多個端點只要有一個非 https → 違規' (@(Get-ReleaseViolations $cMixed @() @() 'update-e2e').Count -eq 1)
    Check '-Release：基礎設定 dangerousInsecureTransportProtocol=true → 違規' (@(Get-ReleaseViolations $cDanger @() @() 'update-e2e').Count -eq 1)
    Check '-Release：dangerousInsecureTransportProtocol=false 不違規' (@(Get-ReleaseViolations $cDangerFalse @() @() 'update-e2e').Count -eq 0)
    $realV = @(Get-ReleaseViolations (Join-Path $hostDir 'tauri.conf.json') @() @() 'update-e2e')
    Check '-Release：真實 tauri.conf.json 的端點與 dangerous 旗標沒有違規（只可能剩公鑰占位字串這一項）' (
        @($realV | Where-Object { $_ -match 'endpoints|dangerous' }).Count -eq 0) ($realV -join '|')

    # e2e 測試公鑰：格式合法所以會通過格式檢查，必須另外擋
    Check '-Release：公鑰＝e2e 測試公鑰（字串相同）→ 違規' (@(Get-ReleaseViolations $confGood @() @() 'update-e2e' @($goodPub)).Count -eq 1)
    $goodPubCrlf = [Convert]::ToBase64String([System.Text.Encoding]::UTF8.GetBytes($pubText.Replace("`n", "`r`n")))
    Check '-Release：公鑰與 e2e 測試公鑰只差換行格式（key ID 相同）→ 違規' (
        @(Get-ReleaseViolations $confGood @() @() 'update-e2e' @($goodPubCrlf)).Count -eq 1)
    # 另一把金鑰（key ID 不同）的公鑰：構造 42 位元組＝'Ed'＋8 位元組 key ID＋32 位元組公鑰
    function New-FakePub([byte[]]$keyId) {
        $raw = [byte[]](0x45, 0x64) + $keyId + [byte[]](1..32)
        $txt = "untrusted comment: minisign public key: TEST`n" + [Convert]::ToBase64String($raw) + "`n"
        [Convert]::ToBase64String([System.Text.Encoding]::UTF8.GetBytes($txt))
    }
    $otherPub = New-FakePub ([byte[]](1, 2, 3, 4, 5, 6, 7, 8))
    $confOther = Join-Path $confDir 'other.json'
    Write-Utf8NoBom $confOther ('{"plugins":{"updater":{"pubkey":"' + $otherPub + '",' + $httpsEp + '}}}')
    Check '-Release：公鑰是另一把金鑰（key ID 不同於 e2e 測試公鑰）→ 通過' (@(Get-ReleaseViolations $confOther @() @() 'update-e2e' @($goodPub)).Count -eq 0)
    Check '-Release：e2e 公鑰清單含壞值／空值不影響判斷' (@(Get-ReleaseViolations $confOther @() @() 'update-e2e' @('', 'not base64 !!!')).Count -eq 0)

    # key ID（minisign 公鑰與簽章位元組 [2..9]）
    # untrusted comment 顯示的 9652A98D711CF3D5 是位元組序反過來（小端）；函式回傳的是檔內位元組序 D5F31C718DA95296
    Check 'key ID：e2e 公鑰解出 D5F31C718DA95296（＝comment 的 9652A98D711CF3D5 位元組反序）' ((Get-MinisignKeyId $goodPub) -eq 'D5F31C718DA95296') (Get-MinisignKeyId $goodPub)
    Check 'key ID：假公鑰解出構造的 ID' ((Get-MinisignKeyId $otherPub) -eq '0102030405060708')
    foreach ($bad in 'not base64 !!!', [Convert]::ToBase64String([System.Text.Encoding]::UTF8.GetBytes("one line only`n")),
        [Convert]::ToBase64String([System.Text.Encoding]::UTF8.GetBytes("c`nQUJD`n"))) {
        $threw = $false; try { Get-MinisignKeyId $bad | Out-Null } catch { $threw = $true }
        Check "key ID：壞輸入「$($bad.Substring(0, [Math]::Min(12, $bad.Length)))」丟例外" $threw
    }

    # ---- .sig 可信註解（樣本＝tauri-cli 2.12.1 `signer sign` 產生的真實 .sig；測試用拋棄式金鑰，內容為公開資料）
    $sigWithVersion = 'dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVUVjh4eHhqYWxTbGdYY3h6T2cwY1R0Y2xvb0UvcUpSYk5JTDZVK2M1RWhPbnErRDB4a3Q3aytDdlZnRXVyUmVYMGJPWHdteTFSdEhBS0ZOYmIvdzhLNFMvUGRVM2ptemdVPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzkxMTU4MzU5CWZpbGU6ZmluYW5jZS1jYWxlbmRhci1zZXR1cC5leGUJdmVyc2lvbjowLjEuMApFaFR4Y2VsNkIyVkxjb0ZadFV5NzVyTnZrb2NFNkhyRXdjOUVHcE9uMVJLV2E1b2g5dGZNS2hIN2tLWnUvbnFSTU9xTXpjbzZyaWV6OHk5R1ROZ2VBQT09Cg=='
    $sigNoVersion = 'dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVUVjh4eHhqYWxTbHAzVWl1UTZSZHYwNlpjMnpiY25tTzJqZ1FvRmFma2o5MWtWeXp2WitWdnMzaTBObWpoTVBjblRHZjYyZ1UxSHFPWmJpVm1FV21BUjBsRGZzazA3YWdBPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzkxMTU4MzU5CWZpbGU6ZmluYW5jZS1jYWxlbmRhci1zZXR1cC5leGUKcVRYTkFBd2Z4ZEp2b1RiSENwSVpUUkhKUWhlMmthVDVrYnV2VUM2L2pOTllYWU1nQk41OXBHYjFPL1pBNmVvV0YzL1R3VFJkSEt2YmFuZ2ZQb3RRQ2c9PQo='
    $sigDir = New-TempDir 'sig'
    Write-Utf8NoBom (Join-Path $sigDir 'with.sig') $sigWithVersion
    Write-Utf8NoBom (Join-Path $sigDir 'without.sig') $sigNoVersion
    Write-Utf8NoBom (Join-Path $sigDir 'garbage.sig') '%%% not base64 %%%'
    Write-Utf8NoBom (Join-Path $sigDir 'notminisign.sig') ([Convert]::ToBase64String([System.Text.Encoding]::UTF8.GetBytes("hello`nworld`n")))
    $c1 = Get-SigTrustedComment (Join-Path $sigDir 'with.sig')
    Check '.sig：解碼取得可信註解（含 version 欄位）' ($c1 -eq "timestamp:1791158359`tfile:finance-calendar-setup.exe`tversion:0.1.0") $c1
    $c2 = Get-SigTrustedComment (Join-Path $sigDir 'without.sig')
    Check '.sig：未帶 --app-version 的簽章，可信註解沒有 version' ($c2 -eq "timestamp:1791158359`tfile:finance-calendar-setup.exe") $c2
    foreach ($bad in 'garbage.sig', 'notminisign.sig') {
        $threw = $false; try { Get-SigTrustedComment (Join-Path $sigDir $bad) | Out-Null } catch { $threw = $true }
        Check ".sig：$bad 解析失敗時丟例外" $threw
    }
    Check '.sig 驗證：version 與 file 都吻合 → 通過' (@(Test-SigTrustedComment $c1 '0.1.0' 'finance-calendar-setup.exe').Count -eq 0)
    $e = @(Test-SigTrustedComment $c2 '0.1.0' 'finance-calendar-setup.exe')
    Check '.sig 驗證：沒有 version 欄位 → 失敗，訊息提到 requireSignedVersion' ($e.Count -eq 1 -and $e[0] -match 'requireSignedVersion') ($e -join '|')
    Check '.sig 驗證：version 不同 → 失敗' (@(Test-SigTrustedComment $c1 '0.1.1' 'finance-calendar-setup.exe').Count -eq 1)
    Check '.sig 驗證：version 只是子字串（0.1.0 vs 預期 0.1）→ 失敗（必須完全相等）' (@(Test-SigTrustedComment $c1 '0.1' 'finance-calendar-setup.exe').Count -eq 1)
    Check '.sig 驗證：version 欄位更長（預期 0.1.0 實際 10.1.0）→ 失敗' (@(Test-SigTrustedComment "timestamp:1`tfile:a.exe`tversion:10.1.0" '0.1.0' 'a.exe').Count -eq 1)
    Check '.sig 驗證：file 不同 → 失敗' (@(Test-SigTrustedComment $c1 '0.1.0' 'finance-calendar-setup-arm64.exe').Count -eq 1)
    Check '.sig 驗證：把「version:0.1.0」塞在別的欄位值裡（file:evil version:0.1.0）不算' (
        @(Test-SigTrustedComment "timestamp:1`tfile:evil version:0.1.0" '0.1.0' 'evil version:0.1.0').Count -eq 1)
    Check 'key ID：樣本 .sig（e2e 金鑰簽）的 key ID 等於 e2e 公鑰的 key ID（簽章與公鑰同一把）' ((Get-SigKeyId (Join-Path $sigDir 'with.sig')) -eq (Get-MinisignKeyId $goodPub)) (Get-SigKeyId (Join-Path $sigDir 'with.sig'))
    Check 'key ID：樣本 .sig 與另一把金鑰的公鑰 key ID 不同（-Release 的核對會失敗）' ((Get-SigKeyId (Join-Path $sigDir 'with.sig')) -ne (Get-MinisignKeyId $otherPub))
    $threw = $false; try { Get-SigKeyId (Join-Path $sigDir 'garbage.sig') | Out-Null } catch { $threw = $true }
    Check 'key ID：壞 .sig 丟例外' $threw
    # 活的樣本：tauri-cli 在本機（快取目錄已有）就實際簽一次，確認真實輸出格式與假設一致
    $cliDir = Join-Path $env:LOCALAPPDATA "fc-host-tools\tauri-cli-$($const.TauriCliVersion)"
    $cli = Join-Path $cliDir 'cargo-tauri.exe'
    if (Test-Path -LiteralPath $cli) {
        $kd = New-TempDir 'livekey'
        $null = & $cli signer generate --ci -w (Join-Path $kd 'k.key') 2>&1
        $payload = Join-Path $kd 'finance-calendar-setup.exe'
        Write-Utf8NoBom $payload 'payload'
        $null = & $cli signer sign --app-version 7.8.9 -f (Join-Path $kd 'k.key') $payload 2>&1
        $live = Get-SigTrustedComment "$payload.sig"
        Check '.sig（現場產生）：可信註解 version 等於 --app-version、file 等於檔名' (@(Test-SigTrustedComment $live '7.8.9' 'finance-calendar-setup.exe').Count -eq 0) $live
        # 同一把金鑰簽的 .sig 與它的公鑰 key ID 相同；另一把金鑰的公鑰不同（簽章私鑰與內建公鑰不是同一把時 -Release 要失敗）
        $kd2 = New-TempDir 'livekey2'
        $null = & $cli signer generate --ci -w (Join-Path $kd2 'k2.key') 2>&1
        $pubA = (Get-Content (Join-Path $kd 'k.key.pub') -Raw).Trim()
        $pubB = (Get-Content (Join-Path $kd2 'k2.key.pub') -Raw).Trim()
        $liveSigId = Get-SigKeyId "$payload.sig"
        Check 'key ID（現場產生）：簽章的 key ID 等於同一把金鑰公鑰的 key ID' ($liveSigId -eq (Get-MinisignKeyId $pubA)) "$liveSigId vs $(Get-MinisignKeyId $pubA)"
        Check 'key ID（現場產生）：另一把金鑰的公鑰 key ID 不同' ($liveSigId -ne (Get-MinisignKeyId $pubB))
    }
    else { Skip '.sig（現場產生）' "tauri-cli 快取不存在：$cli" }

    # ---- tauri-cli 快取：exe 也要核對雜湊（task 5.3，收尾線複審 low）
    # 以暫存的 LOCALAPPDATA＋從真實快取複製的 x64 zip 驅動 Initialize-TauriCli（zip 在位，不會下載）；
    # 目標 triple 固定 x64，與測試執行的主機架構無關。
    $realZip = Join-Path $cliDir 'cargo-tauri-x86_64-pc-windows-msvc.zip'
    $xAsset = $const.TauriCliAssets['x86_64-pc-windows-msvc']
    if ((Test-Path -LiteralPath $realZip) -and ((Get-FileHash -LiteralPath $realZip -Algorithm SHA256).Hash -ieq $xAsset.Sha256)) {
        $savedLad = $env:LOCALAPPDATA
        try {
            $fakeLad = New-TempDir 'tcli-lad'
            $env:LOCALAPPDATA = $fakeLad
            $fakeDir = Join-Path $fakeLad "fc-host-tools\tauri-cli-$($const.TauriCliVersion)"
            New-Item -ItemType Directory -Force -Path $fakeDir | Out-Null
            Copy-Item -LiteralPath $realZip -Destination $fakeDir
            $fakeExe = Join-Path $fakeDir 'cargo-tauri.exe'
            $x64 = 'x86_64-pc-windows-msvc'

            $r1 = Initialize-TauriCli $x64 $const
            Check 'tauri-cli 快取：只有 zip（無 exe）→ 解壓，exe 雜湊等於 ExeSha256' ($r1 -eq $fakeExe -and (Get-FileHash -LiteralPath $fakeExe -Algorithm SHA256).Hash -ieq $xAsset.ExeSha256)
            $stamp = (Get-Item -LiteralPath $fakeExe).LastWriteTimeUtc
            $r2 = Initialize-TauriCli $x64 $const
            Check 'tauri-cli 快取：命中（zip 與 exe 雜湊皆符）→ 直接用、不重新解壓' ($r2 -eq $fakeExe -and (Get-Item -LiteralPath $fakeExe).LastWriteTimeUtc -eq $stamp)

            # exe 被竄改、zip 完好 → 必須從驗過的 zip 重新解壓回正確的 exe（舊行為：只驗 zip，會直接回傳被竄改的 exe）
            [System.IO.File]::AppendAllText($fakeExe, 'TAMPERED')
            Check '（前置）竄改後 exe 雜湊確實不同' ((Get-FileHash -LiteralPath $fakeExe -Algorithm SHA256).Hash -ine $xAsset.ExeSha256)
            $r3 = Initialize-TauriCli $x64 $const
            Check 'tauri-cli 快取：exe 被竄改（zip 完好）→ 從驗過的 zip 重新解壓，回傳的 exe 雜湊正確' ($r3 -eq $fakeExe -and (Get-FileHash -LiteralPath $fakeExe -Algorithm SHA256).Hash -ieq $xAsset.ExeSha256)

            # zip 解出的 exe 與釘選的 ExeSha256 不符（常數被改成錯值）→ 失敗並刪除 exe、不使用
            $badConst = Get-PackageConstants
            $badConst.TauriCliAssets[$x64].ExeSha256 = ('0' * 64)
            $threw = $false; $msg = ''
            try { Initialize-TauriCli $x64 $badConst | Out-Null } catch { $threw = $true; $msg = $_.Exception.Message }
            Check 'tauri-cli 快取：解壓出的 exe 與釘選 ExeSha256 不符 → 丟例外、訊息說明、不留下該 exe' ($threw -and $msg -match 'cargo-tauri\.exe 的 SHA256 不符' -and -not (Test-Path -LiteralPath $fakeExe)) $msg
        }
        finally { $env:LOCALAPPDATA = $savedLad }
    }
    else { Skip 'tauri-cli 快取 exe 雜湊' "找不到已核對的 x64 zip：$realZip" }

    # ---- 後門字串搜尋
    $bd = New-TempDir 'backdoor'
    $url = 'http://127.0.0.1:8737/latest.json'
    $rnd = New-Object byte[] 4096; (New-Object System.Random 1).NextBytes($rnd)
    $withUtf8 = Join-Path $bd 'utf8.bin'
    [System.IO.File]::WriteAllBytes($withUtf8, $rnd + [System.Text.Encoding]::UTF8.GetBytes("xx${url}yy") + $rnd)
    $withUtf16 = Join-Path $bd 'utf16.bin'
    [System.IO.File]::WriteAllBytes($withUtf16, $rnd + [System.Text.Encoding]::Unicode.GetBytes($url) + $rnd)
    $clean = Join-Path $bd 'clean.bin'
    [System.IO.File]::WriteAllBytes($clean, $rnd + [System.Text.Encoding]::UTF8.GetBytes('http://127.0.0.1:9999/other') + $rnd)
    $partial = Join-Path $bd 'partial.bin'
    [System.IO.File]::WriteAllBytes($partial, $rnd + [System.Text.Encoding]::UTF8.GetBytes('http://127.0.0.1:8737/latest.jso') + $rnd)
    Check '搜尋：UTF-8 內含 → 找到' (@(Find-ForbiddenStrings $withUtf8 @($url)) -contains $url)
    Check '搜尋：UTF-16LE 內含 → 找到' (@(Find-ForbiddenStrings $withUtf16 @($url)) -contains $url)
    Check '搜尋：不含 → 空' (@(Find-ForbiddenStrings $clean @($url)).Count -eq 0)
    Check '搜尋：只差一個字元（截斷）→ 不誤報' (@(Find-ForbiddenStrings $partial @($url)).Count -eq 0)
    $multi = @(Find-ForbiddenStrings $withUtf8 @($url, 'NOT-PRESENT', '', 'dW50cnVz'))
    Check '搜尋：多個字串只回傳找到的、空字串略過' ($multi.Count -eq 1 -and $multi[0] -eq $url) ($multi -join '|')
    Check '搜尋：非 ASCII 字串（中文）也能找到' (@(Find-ForbiddenStrings (New-Item -ItemType File -Path (Join-Path $bd 'zh.bin') -Value 'abc財經日曆def' -Force).FullName @('財經日曆')).Count -eq 1)

    $needles = @(Get-BackdoorNeedles (Join-Path $hostDir 'tauri.e2e.conf.json') @())
    Check '後門字串：含 e2e 設定檔的完整端點網址' ($needles -contains $url) ($needles -join '|')
    Check '後門字串：含 e2e 設定檔的公鑰字串（占位）' ($needles -contains $e2eConf.plugins.updater.pubkey)
    $httpsConf = Join-Path $confDir 'https.json'
    Write-Utf8NoBom $httpsConf '{"plugins":{"updater":{"endpoints":["https://example.com/a.json","http://127.0.0.1:1/x.json"],"pubkey":"PK"}}}'
    $n2 = @(Get-BackdoorNeedles $httpsConf @(' EXTRA-KEY '))
    Check '後門字串：只收 http:// 端點（https 不算）、公鑰、額外公鑰（去空白）' (
        $n2.Count -eq 3 -and ($n2 -contains 'http://127.0.0.1:1/x.json') -and ($n2 -contains 'PK') -and ($n2 -contains 'EXTRA-KEY') -and -not ($n2 -contains 'https://example.com/a.json')) ($n2 -join '|')

    # ---- 腳本結構（靜態）
    $paramNames = @($ast.ParamBlock.Parameters | ForEach-Object { $_.Name.VariablePath.UserPath })
    foreach ($p in 'Target', 'Features', 'Config', 'SigningKeyPath', 'SigningKeyEnv', 'SigningKeyPasswordEnv', 'SkipSign', 'SignOnly', 'InstallerPath', 'Release', 'OutDir') {
        Check "參數：有 -$p" ($paramNames -contains $p)
    }
    Check '參數：沒有直接吃密碼的參數（只有 SigningKeyPasswordEnv 吃環境變數名稱）' (@($paramNames | Where-Object { $_ -match 'Password|Secret|Token' -and $_ -ne 'SigningKeyPasswordEnv' }).Count -eq 0)
    $src = (Get-Content -LiteralPath $target -Raw) -replace "`r`n", "`n"
    $noComments = ($src -split "`n" | Where-Object { $_ -notmatch '^\s*#' }) -join "`n"
    Check '機密：密碼值變數只用於讀取與設定子行程環境變數，不出現在 Write-Host／throw／字串內插' (
        @([regex]::Matches($noComments, '\$passwordValue')).Count -ge 2 -and
        $noComments -notmatch '(Write-Host|Write-Output|throw)[^\n]*\$passwordValue' -and
        $noComments -notmatch '"[^"\n]*\$passwordValue[^"\n]*"')
    Check '機密：私鑰內容變數只用於讀取與設定子行程環境變數，不出現在 Write-Host／throw／字串內插' (
        @([regex]::Matches($noComments, '\$keyValue')).Count -ge 2 -and
        $noComments -notmatch '(Write-Host|Write-Output|throw)[^\n]*\$keyValue' -and
        $noComments -notmatch '"[^"\n]*\$keyValue[^"\n]*"')
    Check '機密：私鑰檔只以 -f 路徑傳給 signer、腳本不讀取其內容' ($noComments -notmatch 'Get-Content[^\n]*\$SigningKeyPath')
    Check '簽章：以獨立步驟 signer sign --app-version（不用 createUpdaterArtifacts）' ($noComments -match 'signer sign --app-version' -or $noComments -match "signer sign --app-version")
    Check '建置：不呼叫 node／npm（不需要 Node）' ($noComments -notmatch '\b(node|npm|npx)\b\s')
    Check '建置：只打包 nsis' ($noComments -match "'--bundles', 'nsis'")
    Check '行程：不以名稱停止行程' ($noComments -notmatch 'Stop-Process|taskkill')

    # 位元組掃描：本 change 在 host/ 下寫的文字檔不得含 TAB／CR／LF 以外的控制字元（NUL、換頁等）。
    # 緣由：寫檔時路徑裡的 `\f` 被吃成換頁字元 0x0C（README 的 `%LOCALAPPDATA%\fc-host-tools`），編譯、markdownlint 都抓不到。
    foreach ($rel in 'README.md', 'installer\hooks.nsh', 'tools\package.ps1', 'tools\tests\package.Tests.ps1', 'Cargo.toml', 'tauri.conf.json', 'tauri.e2e.conf.json') {
        $bytes = [System.IO.File]::ReadAllBytes((Join-Path $hostDir $rel))
        $bad = @(for ($i = 0; $i -lt $bytes.Length; $i++) { $b = $bytes[$i]; if (($b -lt 0x20 -and $b -ne 9 -and $b -ne 10 -and $b -ne 13) -or $b -eq 0x7F) { '0x{0:X2}@{1}' -f $b, $i } })
        Check "位元組掃描：$rel 沒有控制字元" ($bad.Count -eq 0) ($bad -join ',')
    }
    Check '位元組掃描：README 的快取路徑寫對（%LOCALAPPDATA%\fc-host-tools）' ([System.IO.File]::ReadAllText((Join-Path $hostDir 'README.md')).Contains('%LOCALAPPDATA%\fc-host-tools'))

    # ---- 實際執行 package.ps1 的失敗路徑（假 host/ 目錄；任何建置之前就停下）
    function New-FakeHostTree([string]$name, [bool]$withBom, [string]$pubkey) {
        $root = New-TempDir $name
        $h = Join-Path $root 'host'
        New-Item -ItemType Directory -Force -Path (Join-Path $h 'tools'), (Join-Path $h 'installer') | Out-Null
        Copy-Item -LiteralPath $target -Destination (Join-Path $h 'tools\package.ps1')
        Write-Utf8NoBom (Join-Path $h 'Cargo.toml') "[package]`nname = `"fc-host`"`nversion = `"0.0.1`"`n"
        Write-Utf8NoBom (Join-Path $h 'tauri.conf.json') ('{"plugins":{"updater":{"pubkey":"' + $pubkey + '","endpoints":["https://example.com/latest.json"]}}}')
        Write-Utf8NoBom (Join-Path $h 'tauri.e2e.conf.json') '{"plugins":{"updater":{"endpoints":["http://127.0.0.1:8737/latest.json"],"pubkey":"E2E_TEST_PUBKEY_PLACEHOLDER"}}}'
        $bytes = [System.Text.Encoding]::UTF8.GetBytes("; hook`n")
        if ($withBom) { $bytes = [byte[]](0xEF, 0xBB, 0xBF) + $bytes }
        [System.IO.File]::WriteAllBytes((Join-Path $h 'installer\hooks.nsh'), $bytes)
        $root
    }
    $fakeKey = Join-Path $tmpRoot 'fake.key'
    Write-Utf8NoBom $fakeKey 'not a real key'
    $pwsh = (Get-Process -Id $PID).Path

    $root1 = New-FakeHostTree 'fake-placeholder' $true 'PRODUCTION_PUBKEY_PENDING_TASK_1_1'
    $out1 = Join-Path $root1 'out'
    $r1 = & $pwsh -NoProfile -File (Join-Path $root1 'host\tools\package.ps1') -Release -SigningKeyPath $fakeKey -OutDir $out1 2>&1 | Out-String
    $code1 = $LASTEXITCODE
    Check '執行：-Release＋占位公鑰 → 結束碼非 0、訊息指出 pubkey' ($code1 -ne 0 -and $r1 -match 'pubkey' -and $r1 -match 'PRODUCTION_PUBKEY_PENDING_TASK_1_1') "code=$code1 $r1"
    Check '執行：-Release 失敗時沒有開始建置（沒有 tauri-cli 輸出、沒有輸出資料夾）' ($r1 -notmatch 'cargo tauri build' -and -not (Test-Path -LiteralPath $out1))

    $root2 = New-FakeHostTree 'fake-nobom' $false $goodPub
    $r2 = & $pwsh -NoProfile -File (Join-Path $root2 'host\tools\package.ps1') -SigningKeyPath $fakeKey -OutDir (Join-Path $root2 'out') 2>&1 | Out-String
    $code2 = $LASTEXITCODE
    Check '執行：hooks.nsh 缺 BOM → 結束碼非 0、訊息指出 BOM' ($code2 -ne 0 -and $r2 -match 'BOM' -and $r2 -notmatch 'cargo tauri build') "code=$code2 $r2"

    $root3 = New-FakeHostTree 'fake-config-release' $true $goodPub
    $r3 = & $pwsh -NoProfile -File (Join-Path $root3 'host\tools\package.ps1') -Release -Config (Join-Path $root3 'host\tauri.e2e.conf.json') -Features update-e2e -SigningKeyPath $fakeKey -OutDir (Join-Path $root3 'out') 2>&1 | Out-String
    $code3 = $LASTEXITCODE
    Check '執行：-Release 帶 e2e 設定與 update-e2e feature → 失敗並兩項都列出' ($code3 -ne 0 -and $r3 -match '-Config' -and $r3 -match 'update-e2e' -and $r3 -notmatch 'cargo tauri build') "code=$code3 $r3"

    $root4 = New-FakeHostTree 'fake-nokey' $true $goodPub
    $r4 = & $pwsh -NoProfile -File (Join-Path $root4 'host\tools\package.ps1') -SigningKeyPath (Join-Path $root4 'missing.key') -OutDir (Join-Path $root4 'out') 2>&1 | Out-String
    Check '執行：私鑰檔不存在 → 失敗（建置之前）' ($LASTEXITCODE -ne 0 -and $r4 -match 'SigningKeyPath' -and $r4 -notmatch 'cargo tauri build') "code=$LASTEXITCODE $r4"

    $env:FC_PKG_TEST_EMPTY_PW = ''
    $root5 = New-FakeHostTree 'fake-emptypw' $true $goodPub
    $r5 = & $pwsh -NoProfile -File (Join-Path $root5 'host\tools\package.ps1') -SigningKeyPath $fakeKey -SigningKeyPasswordEnv FC_PKG_TEST_EMPTY_PW -OutDir (Join-Path $root5 'out') 2>&1 | Out-String
    Check '執行：-SigningKeyPasswordEnv 指到空的環境變數 → 失敗（建置之前）' ($LASTEXITCODE -ne 0 -and $r5 -match 'FC_PKG_TEST_EMPTY_PW' -and $r5 -notmatch 'cargo tauri build') "code=$LASTEXITCODE $r5"
    Remove-Item Env:FC_PKG_TEST_EMPTY_PW -ErrorAction SilentlyContinue

    # ---- task 5.2：key ID 顯示序（task 2 複審 N1）、e2e 公鑰讀取（N2）、CI 分段模式（-SkipSign／-SignOnly／-SigningKeyEnv）
    Check 'key ID 顯示序：檔內序 D5F31C718DA95296 → .pub comment 的 9652A98D711CF3D5（小端 u64）' ((ConvertTo-KeyIdDisplay 'D5F31C718DA95296') -eq '9652A98D711CF3D5')
    Check 'key ID 顯示序：樣本公鑰的顯示值＝其 untrusted comment 內的 key ID' ((ConvertTo-KeyIdDisplay (Get-MinisignKeyId $goodPub)) -eq '9652A98D711CF3D5')
    $vE2e = @(Get-ReleaseViolations $confGood @() @() 'update-e2e' @($goodPub))
    Check '-Release 的 e2e 公鑰違規訊息：key ID 用顯示序（能對上 .pub 的 comment），不是檔內序' ($vE2e.Count -eq 1 -and $vE2e[0].Contains('9652A98D711CF3D5') -and -not $vE2e[0].Contains('D5F31C718DA95296')) ($vE2e -join '|')

    $pd = New-TempDir 'e2epubs'
    Write-Utf8NoBom (Join-Path $pd 'empty.pub') ''
    Write-Utf8NoBom (Join-Path $pd 'blank.pub') "  `n"
    Write-Utf8NoBom (Join-Path $pd 'good.pub') "  KEY-A`n"
    Write-Utf8NoBom (Join-Path $pd 'other.txt') 'KEY-B'
    $pubs = @(Get-E2ePubkeys $pd)
    Check 'e2e 公鑰讀取：空的／全空白的 .pub 略過、其餘 Trim、非 .pub 不讀（不丟例外）' ($pubs.Count -eq 1 -and $pubs[0] -eq 'KEY-A') ($pubs -join '|')
    Check 'e2e 公鑰讀取：目錄不存在 → 空陣列、不丟例外' (@(Get-E2ePubkeys (Join-Path $pd 'nope')).Count -eq 0)

    $okSign = @{ Target = ''; Features = @(); Config = @(); SigningKeyPath = 'k'; SigningKeyEnv = ''; SigningKeyPasswordEnv = ''; SkipSign = $false; SignOnly = $false; InstallerPath = '' }
    function Get-Pv([hashtable]$over) { $h = $okSign.Clone(); foreach ($k in $over.Keys) { $h[$k] = $over[$k] }; , @(Get-ParameterViolations $h) }
    Check '參數組合：一般簽章打包（-SigningKeyPath）→ 通過' ((Get-Pv @{}).Count -eq 0)
    Check '參數組合：-SigningKeyEnv 取代路徑 → 通過' ((Get-Pv @{ SigningKeyPath = ''; SigningKeyEnv = 'X' }).Count -eq 0)
    Check '參數組合：路徑與環境變數同時給 → 違規' ((Get-Pv @{ SigningKeyEnv = 'X' }).Count -eq 1)
    Check '參數組合：簽章但沒給金鑰 → 違規' ((Get-Pv @{ SigningKeyPath = '' }).Count -eq 1)
    Check '參數組合：-SkipSign（無金鑰）→ 通過' ((Get-Pv @{ SigningKeyPath = ''; SkipSign = $true }).Count -eq 0)
    Check '參數組合：-SkipSign 帶金鑰路徑／金鑰環境變數／密碼環境變數 → 各自違規（建置 job 不得碰簽章金鑰）' (
        (Get-Pv @{ SkipSign = $true }).Count -eq 1 -and (Get-Pv @{ SigningKeyPath = ''; SigningKeyEnv = 'X'; SkipSign = $true }).Count -eq 1 -and
        (Get-Pv @{ SigningKeyPath = ''; SigningKeyPasswordEnv = 'P'; SkipSign = $true }).Count -eq 1)
    Check '參數組合：-SkipSign 與 -SignOnly 互斥' ((Get-Pv @{ SigningKeyPath = ''; SkipSign = $true; SignOnly = $true; InstallerPath = 'a' }).Count -ge 1)
    Check '參數組合：-SignOnly＋-InstallerPath＋金鑰 → 通過' ((Get-Pv @{ SignOnly = $true; InstallerPath = 'a.exe' }).Count -eq 0)
    Check '參數組合：-SignOnly 缺 -InstallerPath → 違規' ((Get-Pv @{ SignOnly = $true }).Count -eq 1)
    Check '參數組合：-SignOnly 帶 -Target／-Features／-Config → 違規' (
        (Get-Pv @{ SignOnly = $true; InstallerPath = 'a'; Target = 'x' }).Count -eq 1 -and (Get-Pv @{ SignOnly = $true; InstallerPath = 'a'; Features = @('f') }).Count -eq 1 -and
        (Get-Pv @{ SignOnly = $true; InstallerPath = 'a'; Config = @('c') }).Count -eq 1)
    Check '參數組合：-InstallerPath 沒有 -SignOnly → 違規' ((Get-Pv @{ InstallerPath = 'a' }).Count -eq 1)

    $rootP = New-FakeHostTree 'fake-params' $true $goodPub
    $pkg = Join-Path $rootP 'host\tools\package.ps1'
    $rp1 = & $pwsh -NoProfile -File $pkg -SkipSign -SigningKeyPath $fakeKey 2>&1 | Out-String
    Check '執行：-SkipSign 帶金鑰 → 失敗（建置之前）' ($LASTEXITCODE -ne 0 -and $rp1 -match 'SkipSign' -and $rp1 -notmatch 'cargo tauri build') "code=$LASTEXITCODE $rp1"
    $rp2 = & $pwsh -NoProfile -File $pkg -SignOnly -SigningKeyPath $fakeKey 2>&1 | Out-String
    Check '執行：-SignOnly 缺 -InstallerPath → 失敗' ($LASTEXITCODE -ne 0 -and $rp2 -match 'InstallerPath') "code=$LASTEXITCODE $rp2"
    $rp4 = & $pwsh -NoProfile -File $pkg 2>&1 | Out-String
    Check '執行：完全沒給金鑰也沒給 -SkipSign → 失敗並提示（不因 Mandatory 在非互動環境卡住）' ($LASTEXITCODE -ne 0 -and $rp4 -match 'SkipSign') "code=$LASTEXITCODE $rp4"
    $env:FC_PKG_TEST_BLANKKEY = ''
    $rp5 = & $pwsh -NoProfile -File $pkg -SigningKeyEnv FC_PKG_TEST_BLANKKEY 2>&1 | Out-String
    Check '執行：-SigningKeyEnv 指到空的環境變數 → 失敗（建置之前）' ($LASTEXITCODE -ne 0 -and $rp5 -match 'FC_PKG_TEST_BLANKKEY' -and $rp5 -notmatch 'cargo tauri build') "code=$LASTEXITCODE $rp5"
    Remove-Item Env:FC_PKG_TEST_BLANKKEY -ErrorAction SilentlyContinue

    # -SignOnly 活的端到端（需要本機已快取 tauri-cli；不建置、不啟動任何安裝檔）
    if (Test-Path -LiteralPath $cli) {
        $ka = New-TempDir 'so-keyA'; $kb = New-TempDir 'so-keyB'
        $null = & $cli signer generate --ci -w (Join-Path $ka 'a.key') 2>&1
        $null = & $cli signer generate --ci -w (Join-Path $kb 'b.key') 2>&1
        $pubA = (Get-Content (Join-Path $ka 'a.key.pub') -Raw).Trim()
        $rootS = New-FakeHostTree 'fake-signonly' $true $pubA
        $pkgS = Join-Path $rootS 'host\tools\package.ps1'
        $distS = New-TempDir 'so-dist'
        foreach ($fn in 'finance-calendar-setup.exe', 'finance-calendar-setup-arm64.exe') { Write-Utf8NoBom (Join-Path $distS $fn) "payload $fn" }

        $rs1 = & $pwsh -NoProfile -File $pkgS -SignOnly -Release -InstallerPath (Join-Path $distS 'finance-calendar-setup.exe') -SigningKeyPath (Join-Path $ka 'a.key') 2>&1 | Out-String
        $code = $LASTEXITCODE
        $c1s = if (Test-Path "$distS\finance-calendar-setup.exe.sig") { Get-SigTrustedComment "$distS\finance-calendar-setup.exe.sig" } else { '' }
        Check '執行 -SignOnly -Release（路徑私鑰）：成功、.sig 可信註解 version＝Cargo 版本 0.0.1、file＝發佈檔名' (
            $code -eq 0 -and @(Test-SigTrustedComment $c1s '0.0.1' 'finance-calendar-setup.exe').Count -eq 0 -and $rs1 -match 'key ID 核對') "code=$code $rs1"

        # 私鑰內容經環境變數（CI 的作法）＋ ARM64 檔名；輸出不得含私鑰內容
        $keyText = (Get-Content (Join-Path $ka 'a.key') -Raw).Trim()
        $env:FC_PKG_TEST_KEY = $keyText
        $rs2 = & $pwsh -NoProfile -File $pkgS -SignOnly -Release -InstallerPath (Join-Path $distS 'finance-calendar-setup-arm64.exe') -SigningKeyEnv FC_PKG_TEST_KEY 2>&1 | Out-String
        $code = $LASTEXITCODE
        Remove-Item Env:FC_PKG_TEST_KEY -ErrorAction SilentlyContinue
        $c2s = if (Test-Path "$distS\finance-calendar-setup-arm64.exe.sig") { Get-SigTrustedComment "$distS\finance-calendar-setup-arm64.exe.sig" } else { '' }
        Check '執行 -SignOnly -SigningKeyEnv：成功、file＝finance-calendar-setup-arm64.exe' (
            $code -eq 0 -and @(Test-SigTrustedComment $c2s '0.0.1' 'finance-calendar-setup-arm64.exe').Count -eq 0) "code=$code $rs2"
        Check '機密：-SigningKeyEnv 的私鑰內容不出現在輸出' (-not $rs2.Contains($keyText) -and -not $rs2.Contains(($keyText -split '\s+')[-1]))

        # 簽章金鑰與內建公鑰不是同一把（B 簽、conf 是 A 的公鑰）：失敗、刪 .sig、保留安裝檔
        $rs3 = & $pwsh -NoProfile -File $pkgS -SignOnly -Release -InstallerPath (Join-Path $distS 'finance-calendar-setup.exe') -SigningKeyPath (Join-Path $kb 'b.key') 2>&1 | Out-String
        $code = $LASTEXITCODE
        $disp = ConvertTo-KeyIdDisplay (Get-MinisignKeyId $pubA)
        Check '執行 -SignOnly -Release：金鑰與內建公鑰不符 → 失敗、訊息帶內建公鑰的顯示序 key ID、刪 .sig、保留安裝檔' (
            $code -ne 0 -and $rs3 -match 'key ID' -and $rs3.Contains($disp) -and -not (Test-Path "$distS\finance-calendar-setup.exe.sig") -and (Test-Path "$distS\finance-calendar-setup.exe")) "code=$code $rs3"

        Write-Utf8NoBom (Join-Path $distS 'other.exe') 'x'
        $rs4 = & $pwsh -NoProfile -File $pkgS -SignOnly -InstallerPath (Join-Path $distS 'other.exe') -SigningKeyPath (Join-Path $ka 'a.key') 2>&1 | Out-String
        Check '執行 -SignOnly：檔名不是發佈檔名 → 失敗' ($LASTEXITCODE -ne 0 -and $rs4 -match '發佈檔名') "code=$LASTEXITCODE $rs4"
        # -SignOnly 前的前置檢查：占位公鑰時 -Release 失敗（不簽）
        $rootS2 = New-FakeHostTree 'fake-signonly-ph' $true 'PRODUCTION_PUBKEY_PENDING_TASK_1_1'
        $rs5 = & $pwsh -NoProfile -File (Join-Path $rootS2 'host\tools\package.ps1') -SignOnly -Release -InstallerPath (Join-Path $distS 'finance-calendar-setup-arm64.exe') -SigningKeyPath (Join-Path $ka 'a.key') 2>&1 | Out-String
        Check '執行 -SignOnly -Release：占位公鑰 → 失敗' ($LASTEXITCODE -ne 0 -and $rs5 -match 'pubkey') "code=$LASTEXITCODE $rs5"
    }
    else { Skip '-SignOnly 端到端' "tauri-cli 快取不存在：$cli" }

    # ============================================================================================
    # B. hooks.nsh
    # ============================================================================================
    $hookSrc = [System.IO.File]::ReadAllText($hooksPath, [System.Text.Encoding]::UTF8)
    foreach ($m in 'NSIS_HOOK_PREINSTALL', 'NSIS_HOOK_POSTINSTALL', 'NSIS_HOOK_PREUNINSTALL', 'NSIS_HOOK_POSTUNINSTALL') {
        Check "hooks.nsh：定義巨集 $m（名稱須與範本 !ifmacrodef 完全一致）" ($hookSrc -match "(?m)^!macro $m\s*$")
    }
    $hookCode = ($hookSrc -split "`n" | ForEach-Object { ($_ -replace '^\s*;.*$', '') -replace '\s;\s.*$', '' }) -join "`n"
    Check 'hooks.nsh：不強制終止行程（無 TerminateProcess／taskkill／KillProcess／nsProcess::_KillProcess）' ($hookCode -notmatch '(?i)TerminateProcess|taskkill|KillProcess|RmShutdown')
    Check 'hooks.nsh：註解引用 installer.nsi 行號（至少 20 處）' (@([regex]::Matches($hookSrc, 'installer\.nsi:\d+')).Count -ge 20)
    Check 'hooks.nsh：DisplayName 以 SHCTX 覆寫為「財經日曆」' ($hookCode -match 'WriteRegStr SHCTX "\$\{UNINSTKEY\}" "DisplayName" "財經日曆"')
    # 測試框架會覆寫下面三個常數把 hook 導到暫存位置；這裡確認「不覆寫時」的預設值就是真實位置
    Check 'hooks.nsh：預設開始功能表資料夾是 $SMPROGRAMS' ($hookSrc.Contains('!define FC_SM_DIR "$SMPROGRAMS"'))
    Check 'hooks.nsh：預設 Run 鍵是 HKCU\...\CurrentVersion\Run' ($hookSrc.Contains('!define FC_RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"'))
    Check 'hooks.nsh：預設 StartupApproved 鍵是 ...\Explorer\StartupApproved\Run' ($hookSrc.Contains('!define FC_STARTUP_APPROVED_KEY "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run"'))
    Check 'hooks.nsh：預設等待為 15000／5000／5000 ms（結束碼 3／4／1）' ($hookSrc.Contains('!define FC_WAIT_CODE3_MS 15000') -and $hookSrc.Contains('!define FC_WAIT_CODE4_MS 5000') -and $hookSrc.Contains('!define FC_WAIT_CODE1_MS 5000'))
    Check 'hooks.nsh：Run 值內容為 "<exe>" --autostart、值名 ${PRODUCTNAME}' ($hookCode.Contains('''"$INSTDIR\${MAINBINARYNAME}.exe" --autostart''') -and $hookCode.Contains('WriteRegStr HKCU "${FC_RUN_KEY}" "${PRODUCTNAME}"'))
    Check 'hooks.nsh：PREUNINSTALL 包在非更新模式內' ($hookCode -match '(?s)!macro NSIS_HOOK_PREUNINSTALL\s*\$\{If\} \$UpdateMode <> 1')
    # 桌面捷徑：預設位置是真正的桌面；互動安裝的捷徑在完成頁才建立（晚於所有 hook），靠 .onGUIEnd 改名
    Check 'hooks.nsh：預設桌面資料夾是 $DESKTOP' ($hookSrc.Contains('!define FC_DESKTOP_DIR "$DESKTOP"'))
    Check 'hooks.nsh：定義 .onGUIEnd，且只在 POSTINSTALL 跑過（$FcInstallDone=1）才改名桌面捷徑' (
        $hookCode -match '(?s)\nFunction \.onGUIEnd\s*\$\{If\} \$FcInstallDone = 1\s*Call FcRenameDesktopShortcut\s*\$\{EndIf\}\s*FunctionEnd')
    Check 'hooks.nsh：POSTINSTALL 先改名桌面捷徑、最後才設 $FcInstallDone=1' (
        $hookCode -match '(?s)!macro NSIS_HOOK_POSTINSTALL.*StrCpy \$FcDeskSrc "\$\{FC_DESKTOP_DIR\}\\\$\{PRODUCTNAME\}\.lnk".*StrCpy \$FcDeskDst "\$\{FC_DESKTOP_DIR\}\\財經日曆\.lnk".*Call FcRenameDesktopShortcut\s*StrCpy \$FcInstallDone 1\s*!macroend')
    Check 'hooks.nsh：改名桌面捷徑前比對來源目標（IsShortcutTarget）' (
        $hookCode -match '(?s)Function FcRenameDesktopShortcut.*IsShortcutTarget "\$FcDeskSrc" "\$FcDeskExe".*Rename "\$FcDeskSrc" "\$FcDeskDst".*FunctionEnd')
    Check 'hooks.nsh：POSTUNINSTALL 刪桌面「財經日曆.lnk」前要求目標是本主程式' (
        $hookCode -match '(?s)!macro NSIS_HOOK_POSTUNINSTALL.*IsShortcutTarget "\$\{FC_DESKTOP_DIR\}\\財經日曆\.lnk" "\$INSTDIR\\\$\{MAINBINARYNAME\}\.exe".*Delete "\$\{FC_DESKTOP_DIR\}\\財經日曆\.lnk"')

    $makensis = @(
        (Join-Path $env:LOCALAPPDATA 'tauri\NSIS\makensis.exe'),
        (Join-Path ${env:ProgramFiles(x86)} 'NSIS\makensis.exe'),
        (Join-Path $env:ProgramFiles 'NSIS\makensis.exe')
    ) | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
    if (-not $makensis) {
        Skip 'hooks.nsh 的 NSIS 行為測試' '找不到 makensis（tauri bundler 第一次打包時會下載到 %LOCALAPPDATA%\tauri\NSIS）'
    }
    else {
        $nsisRoot = New-TempDir 'nsis'
        function Write-NsisSource([string]$path, [string]$text) { [System.IO.File]::WriteAllText($path, $text, (New-Object System.Text.UTF8Encoding($true))) }

        # 測試用安裝／解除安裝程式：把四個 hook 巨集接在與範本相同的位置；$PassiveMode／$UpdateMode 由命令列決定
        # （同範本 .onInit／un.onInit）。記錄、捷徑、登錄全部導到暫存位置（hooks.nsh 的 FC_* 覆寫常數），
        # 不碰真實的開始功能表與 HKCU\...\Run；UnpinShortcut 以空巨集代替（真正的實作是範本 utils.nsh 的 COM 呼叫，
        # 已由 tauri build 實際編譯過）。
        $harnessNsi = Join-Path $nsisRoot 'harness.nsi'
        $harnessExe = Join-Path $nsisRoot 'harness-setup.exe'
        Write-NsisSource $harnessNsi @"
Unicode true
RequestExecutionLevel user
SilentInstall silent
SilentUnInstall silent
!include "LogicLib.nsh"
!include "FileFunc.nsh"
!define FC_TEST_LOG_DIR "`$EXEDIR\logs"
!define FC_SM_DIR "`$EXEDIR\sm"
!define FC_DESKTOP_DIR "`$EXEDIR\desk"
!define FC_RUN_KEY "Software\fc-host-nsis-test\Run"
!define FC_STARTUP_APPROVED_KEY "Software\fc-host-nsis-test\StartupApproved"
!define FC_WAIT_CODE3_MS 300
!define FC_WAIT_CODE4_MS 100
!define FC_WAIT_CODE1_MS 1500
; 範本的 IsShortcutTarget（utils.nsh:164-188，COM 讀 .lnk 目標）在 hooks 之前 include（installer.nsi:26 對 :36），
; hooks 的 Function 本體會用到，所以替身也要定義在 include 之前。替身把「捷徑」當純文字檔：第一行＝目標路徑。
!macro IsShortcutTarget shortcut target
  StrCpy `$3 0
  ClearErrors
  FileOpen `$0 "`${shortcut}" r
  `${IfNot} `${Errors}
    FileRead `$0 `$2
    FileClose `$0
    StrCpy `$1 `$2 1 -1
    `${If} `$1 == "`$\n"
      StrCpy `$2 `$2 -1
    `${EndIf}
    `${If} `$2 == "`${target}"
      StrCpy `$3 1
    `${EndIf}
  `${EndIf}
  ClearErrors
  Push `$3
!macroend
; 與範本同序：hooks 在 !define／Var 之前 include（installer.nsi:36 對 :41-78）。Function 本體若誤用範本的
; define 或變數，這裡會編不過（或出警告，被下面的「無警告」斷言抓到），不會測得過、打包卻行為錯誤。
!include "$hooksPath"
!define PRODUCTNAME "fc-host-nsis-test"
!define MAINBINARYNAME "fc-host-stub"
!define BUNDLEID "tw.fintools.fc-host-nsis-test"
!define UNINSTKEY "Software\fc-host-nsis-test\Uninstall\`${PRODUCTNAME}"
!define STARTMENUFOLDER ""
!macro UnpinShortcut shortcut
!macroend
Var PassiveMode
Var UpdateMode
OutFile "$harnessExe"
InstallDir "`$EXEDIR"
Function .onInit
  `${GetOptions} `$CMDLINE "/UPDATE" `$UpdateMode
  `${IfNot} `${Errors}
    StrCpy `$UpdateMode 1
  `${EndIf}
FunctionEnd
Section
  !insertmacro NSIS_HOOK_PREINSTALL
  FileOpen `$0 "`$EXEDIR\preinstall.txt" w
  FileWrite `$0 "FcHadInstall=`$FcHadInstall"
  FileClose `$0
  ; 範本在 installer.nsi:702 每次安裝都把 DisplayName 寫回 PRODUCTNAME，POSTINSTALL 之後才覆寫
  WriteRegStr SHCTX "`${UNINSTKEY}" "DisplayName" "`${PRODUCTNAME}"
  ; 測試用旗標（範本沒有）：/CANCEL＝模擬使用者在 POSTINSTALL 之前取消、視窗關閉（只呼叫 .onGUIEnd）；
  ; /FINISHDESK＝模擬互動安裝完成頁勾了「建立桌面捷徑」（範本 installer.nsi:411-413、:976 在 POSTINSTALL
  ; 之後建立 `$DESKTOP\`${PRODUCTNAME}.lnk）；/GUI＝有安裝視窗（互動、被動），結束時呼叫 .onGUIEnd
  ; （NSIS Ui.c：靜默安裝不呼叫）。測試框架本身是 SilentInstall，.onGUIEnd 不會被 NSIS 自動呼叫，所以手動呼叫。
  ClearErrors
  `${GetOptions} `$CMDLINE "/CANCEL" `$R0
  `${IfNot} `${Errors}
    Call .onGUIEnd
  `${Else}
    !insertmacro NSIS_HOOK_POSTINSTALL
    ClearErrors
    `${GetOptions} `$CMDLINE "/FINISHDESK" `$R0
    `${IfNot} `${Errors}
      CreateDirectory "`$EXEDIR\desk"
      FileOpen `$0 "`$EXEDIR\desk\`${PRODUCTNAME}.lnk" w
      FileWrite `$0 "`$INSTDIR\`${MAINBINARYNAME}.exe`$\nFINISH"
      FileClose `$0
    `${EndIf}
    ClearErrors
    `${GetOptions} `$CMDLINE "/GUI" `$R0
    `${IfNot} `${Errors}
      Call .onGUIEnd
    `${EndIf}
  `${EndIf}
  WriteUninstaller "`$EXEDIR\harness-uninst.exe"
SectionEnd
Function un.onInit
  `${GetOptions} `$CMDLINE "/P" `$PassiveMode
  `${IfNot} `${Errors}
    StrCpy `$PassiveMode 1
  `${EndIf}
  `${GetOptions} `$CMDLINE "/UPDATE" `$UpdateMode
  `${IfNot} `${Errors}
    StrCpy `$UpdateMode 1
  `${EndIf}
FunctionEnd
Section Uninstall
  !insertmacro NSIS_HOOK_PREUNINSTALL
  !insertmacro NSIS_HOOK_POSTUNINSTALL
SectionEnd
"@
        $mk = & $makensis /V2 /INPUTCHARSET UTF8 $harnessNsi 2>&1 | Out-String
        Check 'NSIS：測試用 uninstaller 編譯成功（含 hooks.nsh，無警告）' ((Test-Path -LiteralPath $harnessExe) -and $LASTEXITCODE -eq 0 -and $mk -notmatch 'warning') $mk

        # 假的 fc-host-stub.exe：依呼叫次序回傳 $codes[i]（超過就回 99）；每次呼叫把命令列附加到 calls.log、可選睡眠
        function New-StubExe([string]$dir, [int[]]$codes, [int]$sleepMs = 0) {
            $branches = for ($i = 0; $i -lt $codes.Count; $i++) { "  `${If} `$2 = $i`n    SetErrorLevel $($codes[$i])`n  `${EndIf}" }
            $nsi = Join-Path $dir 'stub.nsi'
            Write-NsisSource $nsi @"
Unicode true
RequestExecutionLevel user
SilentInstall silent
!include "LogicLib.nsh"
OutFile "$dir\fc-host-stub.exe"
Section
  FileOpen `$0 "`$EXEDIR\calls.log" a
  FileSeek `$0 0 END
  FileWrite `$0 "`$CMDLINE`$\r`$\n"
  FileClose `$0
  StrCpy `$2 0
  ClearErrors
  FileOpen `$1 "`$EXEDIR\count.txt" r
  `${IfNot} `${Errors}
    FileRead `$1 `$2
    FileClose `$1
  `${EndIf}
  IntOp `$3 `$2 + 1
  FileOpen `$1 "`$EXEDIR\count.txt" w
  FileWrite `$1 "`$3"
  FileClose `$1
  Sleep $sleepMs
  SetErrorLevel 99
$($branches -join "`n")
SectionEnd
"@
            $o = & $makensis /V1 /INPUTCHARSET UTF8 $nsi 2>&1 | Out-String
            if (-not (Test-Path -LiteralPath (Join-Path $dir 'fc-host-stub.exe'))) { throw "stub 編譯失敗：$o" }
        }

        # 跑一個情境：建資料夾、放 harness 與 stub、產生 uninstaller、以指定參數執行（可重複 -Runs 次）。
        function Invoke-HookScenario([string]$name, [int[]]$codes, [string]$uninstArgs = '/S', [int]$sleepMs = 0, [int]$runs = 1,
            [switch]$NoStub, [switch]$BrokenStub) {
            $dir = New-TempDir "scn-$name"
            Copy-Item -LiteralPath $harnessExe -Destination (Join-Path $dir 'harness-setup.exe')
            if ($BrokenStub) { Write-Utf8NoBom (Join-Path $dir 'fc-host-stub.exe') 'this is not an executable' }
            elseif (-not $NoStub) { New-StubExe $dir $codes $sleepMs }
            $p = Start-Process -FilePath (Join-Path $dir 'harness-setup.exe') -ArgumentList '/S' -Wait -PassThru -WindowStyle Hidden
            if ($p.ExitCode -ne 0 -or -not (Test-Path (Join-Path $dir 'harness-uninst.exe'))) { throw "harness-setup 失敗（$($p.ExitCode)）" }
            $sw = [System.Diagnostics.Stopwatch]::StartNew()
            $exit = $null
            for ($i = 0; $i -lt $runs; $i++) {
                $u = Start-Process -FilePath (Join-Path $dir 'harness-uninst.exe') -ArgumentList "$uninstArgs _?=$dir" -Wait -PassThru -WindowStyle Hidden
                $exit = $u.ExitCode
            }
            $sw.Stop()
            $logPath = Join-Path $dir 'logs\uninstall.log'
            $callsPath = Join-Path $dir 'calls.log'
            $preinstallPath = Join-Path $dir 'preinstall.txt'
            $log = $null; $logBytes = [byte[]]@(); $calls = [string[]]@(); $pre = $null
            if (Test-Path -LiteralPath $logPath) {
                $log = Get-Content -LiteralPath $logPath -Raw -Encoding ASCII
                $logBytes = [System.IO.File]::ReadAllBytes($logPath)
            }
            if (Test-Path -LiteralPath $callsPath) { $calls = [string[]]@(Get-Content -LiteralPath $callsPath | Where-Object { $_ }) }
            if (Test-Path -LiteralPath $preinstallPath) { $pre = (Get-Content -LiteralPath $preinstallPath -Raw -Encoding ASCII).Trim() }
            [PSCustomObject]@{
                Dir = $dir; ExitCode = $exit; ElapsedMs = $sw.ElapsedMilliseconds
                Log = $log; LogBytes = $logBytes; Calls = $calls; PreInstall = $pre
            }
        }

        $ts = '\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2} '
        $r = Invoke-HookScenario 'code0' @(0)
        Check 'NSIS 情境 結束碼 0：呼叫一次、帶 --restore-wallpaper、繼續（uninstaller 結束碼 0）' ($r.Calls.Count -eq 1 -and $r.Calls[0] -match '--restore-wallpaper' -and $r.ExitCode -eq 0) ($r | Out-String)
        Check 'NSIS 情境 結束碼 0：記錄有時間戳、exit code 0、restored，且記錄目錄被自動建立' (
            $r.Log -match "(?m)^${ts}attempt 1: exit code 0\r?$" -and $r.Log -match 'result: wallpaper restored' -and $r.Log -notmatch 'retry') $r.Log
        Check 'NSIS PREINSTALL：安裝前主程式已存在 → FcHadInstall=1' ($r.PreInstall -eq 'FcHadInstall=1') "$($r.PreInstall)"
        Check 'NSIS 記錄：純 ASCII、無 NUL（不是 UTF-16）、以 CRLF 結尾' (
            (@($r.LogBytes | Where-Object { $_ -eq 0 -or $_ -gt 127 }).Count -eq 0) -and $r.Log.EndsWith("`r`n"))

        $r = Invoke-HookScenario 'code6' @(6)
        Check 'NSIS 情境 結束碼 6：視為完成、不重跑、不當失敗' ($r.Calls.Count -eq 1 -and $r.Log -match 'attempt 1: exit code 6' -and $r.Log -match 'treated as done' -and $r.Log -notmatch 'retry|giving up') $r.Log

        $r = Invoke-HookScenario 'code3-then0' @(3, 0)
        Check 'NSIS 情境 3→0：等 FC_WAIT_CODE3（300 ms）後重跑一次、成功' (
            $r.Calls.Count -eq 2 -and $r.Log -match 'exit code 3' -and $r.Log -match 'retry once after 300 ms' -and $r.Log -match 'attempt 2: exit code 0' -and $r.Log -match 'result: wallpaper restored' -and $r.Log -notmatch 'giving up') $r.Log
        Check 'NSIS 情境 3→0：實際有等待（總時間 ≥ 250 ms）' ($r.ElapsedMs -ge 250) "elapsed=$($r.ElapsedMs)"

        $r = Invoke-HookScenario 'code4-then6' @(4, 6)
        Check 'NSIS 情境 4→6：等 FC_WAIT_CODE4（100 ms）後重跑一次、6 視為完成' (
            $r.Calls.Count -eq 2 -and $r.Log -match 'exit code 4' -and $r.Log -match 'retry once after 100 ms' -and $r.Log -match 'attempt 2: exit code 6' -and $r.Log -notmatch 'giving up') $r.Log

        $r = Invoke-HookScenario 'code7-then0' @(7, 0)
        $r7Elapsed = $r.ElapsedMs  # 同樣兩次呼叫、不等待：給 1→0 的「實際有等待」當基準
        Check 'NSIS 情境 7→0：不等待、直接重跑一次' ($r.Calls.Count -eq 2 -and $r.Log -match 'retry once after 0 ms' -and $r.Log -match 'attempt 2: exit code 0') $r.Log

        $r = Invoke-HookScenario 'code3-then3' @(3, 3, 0)
        Check 'NSIS 情境 3→3：只重跑一次（共 2 次呼叫、不會第三次），記錄放棄、uninstaller 仍正常結束' (
            $r.Calls.Count -eq 2 -and $r.Log -match 'giving up' -and $r.Log -match 'exit code 3\)' -and $r.ExitCode -eq 0) "$($r.Calls.Count) $($r.Log)"

        $r = Invoke-HookScenario 'code1-then0' @(1, 0)
        # 結束碼 1 常是暫時性（拓樸切換途中、explorer 重啟中讀不到桌布），立即重跑會撞同一個錯誤：要先等 FC_WAIT_CODE1
        Check 'NSIS 情境 1→0：等 FC_WAIT_CODE1（1500 ms）後重跑一次、成功' (
            $r.Calls.Count -eq 2 -and $r.Log -match 'exit code 1' -and $r.Log -match 'retry once after 1500 ms' -and $r.Log -match 'attempt 2: exit code 0' -and $r.Log -notmatch 'giving up') $r.Log
        Check 'NSIS 情境 1→0：實際有等待（比 7→0 的不等待基準多 ≥ 1200 ms）' ($r.ElapsedMs -ge ($r7Elapsed + 1200)) "elapsed=$($r.ElapsedMs) base=$r7Elapsed"

        $r = Invoke-HookScenario 'code101-then101' @(101, 101, 0)
        Check 'NSIS 情境 101（panic 碼）→101：重跑一次後放棄' ($r.Calls.Count -eq 2 -and $r.Log -match 'giving up' -and $r.Log -match 'exit code 101\)') $r.Log

        $r = Invoke-HookScenario 'passive-fail' @(7, 7) '/S /P'
        Check 'NSIS 情境 passive（/P）仍失敗：只寫記錄、不卡住（有訊息框會卡到逾時）' ($r.Calls.Count -eq 2 -and $r.Log -match 'giving up' -and $r.ExitCode -eq 0) $r.Log

        $r = Invoke-HookScenario 'slow' @(0) '/S' 1500
        Check 'NSIS 情境 宿主跑得慢（1.5 秒）：等它結束、不強制終止，結束碼照實記錄' (
            $r.Calls.Count -eq 1 -and $r.Log -match 'attempt 1: exit code 0' -and $r.ElapsedMs -ge 1400) "elapsed=$($r.ElapsedMs) $($r.Log)"

        $r = Invoke-HookScenario 'nostub' @() -NoStub
        Check 'NSIS 情境 主程式不存在：不呼叫、記錄一行、不當失敗' ($r.Calls.Count -eq 0 -and $r.Log -match 'main executable not found' -and $r.Log -notmatch 'giving up' -and $r.ExitCode -eq 0) $r.Log
        Check 'NSIS PREINSTALL：安裝前主程式不存在（首次安裝）→ FcHadInstall=0' ($r.PreInstall -eq 'FcHadInstall=0') "$($r.PreInstall)"

        $r = Invoke-HookScenario 'broken' @() -BrokenStub
        Check 'NSIS 情境 主程式起不來：ExecWait 回字串 error，視為失敗、重跑一次後放棄（不當成 0）' (
            $r.Log -match 'attempt 1: exit code error' -and $r.Log -match 'retry once after 0 ms' -and $r.Log -match 'attempt 2: exit code error' -and $r.Log -match 'giving up' -and $r.ExitCode -eq 0) $r.Log

        $r = Invoke-HookScenario 'update' @(0) '/S /UPDATE'
        Check 'NSIS 情境 更新模式（/UPDATE）：不呼叫還原、不寫記錄' ($r.Calls.Count -eq 0 -and $null -eq $r.Log -and $r.ExitCode -eq 0) ($r | Out-String)

        $r = Invoke-HookScenario 'append' @(0, 0) '/S' 0 2
        Check 'NSIS 情境 重複執行：記錄是附加（兩次 hook start、兩次呼叫）而非覆寫' (
            @([regex]::Matches($r.Log, 'uninstall hook start')).Count -eq 2 -and $r.Calls.Count -eq 2) $r.Log

        # ---- 安裝／解除安裝的登錄與捷徑 hook（全部在暫存位置：HKCU\Software\fc-host-nsis-test、<情境資料夾>\sm）
        $testHive = 'HKCU:\Software\fc-host-nsis-test'
        $zhName = '財經日曆'
        $zhLnk = "$zhName.lnk"
        function Get-TestReg([string]$sub, [string]$name) {
            $k = Join-Path $testHive $sub
            if (-not (Test-Path -LiteralPath $k)) { return $null }
            $item = Get-ItemProperty -LiteralPath $k
            if ($item.PSObject.Properties.Name -contains $name) { $item.$name } else { $null }
        }
        # 建情境資料夾；-WithStub＝安裝前主程式已存在；-Source／-Target＝捷徑檔內容（$null＝不建立）
        function New-InstallScenario([string]$name, [bool]$WithStub, $Source, $Target) {
            Reset-TestHiveSafe
            $dir = New-TempDir "inst-$name"
            Copy-Item -LiteralPath $harnessExe -Destination (Join-Path $dir 'harness-setup.exe')
            if ($WithStub) { New-StubExe $dir @(0) }
            $sm = New-TempDir "inst-$name\sm"
            if ($null -ne $Source) { Write-Utf8NoBom (Join-Path $sm 'fc-host-nsis-test.lnk') $Source }
            if ($null -ne $Target) { Write-Utf8NoBom (Join-Path $sm $zhLnk) $Target }
            $dir
        }
        function Invoke-Setup([string]$dir, [string]$setupArgs = '/S') {
            $p = Start-Process -FilePath (Join-Path $dir 'harness-setup.exe') -ArgumentList $setupArgs -Wait -PassThru -WindowStyle Hidden
            if ($p.ExitCode -ne 0) { throw "harness-setup 結束碼 $($p.ExitCode)" }
        }
        function Invoke-Uninst([string]$dir, [string]$uArgs = '/S') {
            $u = Start-Process -FilePath (Join-Path $dir 'harness-uninst.exe') -ArgumentList "$uArgs _?=$dir" -Wait -PassThru -WindowStyle Hidden
            if ($u.ExitCode -ne 0) { throw "harness-uninst 結束碼 $($u.ExitCode)" }
        }
        function Read-Lnk([string]$dir, [string]$file) {
            $f = Join-Path $dir "sm\$file"
            if (Test-Path -LiteralPath $f) { $c = Get-Content -LiteralPath $f -Raw; if ($null -eq $c) { "<empty>" } else { $c.Trim() } } else { $null }
        }
        function Set-TestReg([string]$sub, [string]$name, $value, [string]$type = 'String') {
            $k = Join-Path $testHive $sub
            if (-not (Test-Path -LiteralPath $k)) { New-Item -Path $k -Force | Out-Null }
            New-ItemProperty -Path $k -Name $name -PropertyType $type -Value $value -Force | Out-Null
        }
        $approvedOn = [byte[]](2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0)
        $approvedOff = [byte[]](3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0)

        # 首次安裝：安裝前沒有主程式、非更新模式
        $dir = New-InstallScenario 'first' $false 'NEW' $null
        Invoke-Setup $dir
        Check 'POSTINSTALL 首次安裝：PREINSTALL 記下「安裝前無主程式」' ((Get-Content (Join-Path $dir 'preinstall.txt') -Raw).Trim() -eq 'FcHadInstall=0')
        Check 'POSTINSTALL 首次安裝：DisplayName 被覆寫成「財經日曆」（中文沒有亂碼）' ((Get-TestReg 'Uninstall\fc-host-nsis-test' 'DisplayName') -eq $zhName) "$(Get-TestReg 'Uninstall\fc-host-nsis-test' 'DisplayName')"
        Check 'POSTINSTALL 首次安裝：寫入 Run 值 "<exe>" --autostart（值名＝PRODUCTNAME）' ((Get-TestReg 'Run' 'fc-host-nsis-test') -eq "`"$dir\fc-host-stub.exe`" --autostart") "$(Get-TestReg 'Run' 'fc-host-nsis-test')"
        Check 'POSTINSTALL 首次安裝：來源捷徑被改名為「財經日曆.lnk」、來源消失' ((Read-Lnk $dir $zhLnk) -eq 'NEW' -and -not (Test-Path (Join-Path $dir 'sm\fc-host-nsis-test.lnk')))

        # 更新模式（/UPDATE）：範本不建來源捷徑；既有「財經日曆.lnk」必須原封不動；Run 不碰
        $dir = New-InstallScenario 'update-absent' $true $null 'KEEP'
        Invoke-Setup $dir '/S /UPDATE'
        Check 'POSTINSTALL 更新模式：DisplayName 仍每次覆寫（被範本改回後再蓋成中文）' ((Get-TestReg 'Uninstall\fc-host-nsis-test' 'DisplayName') -eq $zhName)
        Check 'POSTINSTALL 更新模式：沒有來源捷徑時不動既有「財經日曆.lnk」（不先刪目標）' ((Read-Lnk $dir $zhLnk) -eq 'KEEP')
        Check 'POSTINSTALL 更新模式：Run 值本來沒有（使用者關掉自啟）→ 仍然沒有' ($null -eq (Get-TestReg 'Run' 'fc-host-nsis-test'))
        $dir = New-InstallScenario 'update-run' $true $null 'KEEP'
        Set-TestReg 'Run' 'fc-host-nsis-test' 'USER-VALUE'
        Invoke-Setup $dir '/S /UPDATE'
        Check 'POSTINSTALL 更新模式：既有 Run 值原封不動' ((Get-TestReg 'Run' 'fc-host-nsis-test') -eq 'USER-VALUE')

        # 同版本直接覆蓋（非更新模式、安裝前已有主程式）：Run 不寫；來源捷徑取代既有目標
        $dir = New-InstallScenario 'overwrite' $true 'NEW' 'OLD'
        Invoke-Setup $dir
        Check 'POSTINSTALL 覆蓋安裝：PREINSTALL 記下「安裝前已有主程式」' ((Get-Content (Join-Path $dir 'preinstall.txt') -Raw).Trim() -eq 'FcHadInstall=1')
        Check 'POSTINSTALL 覆蓋安裝：不寫 Run 值（保留使用者關掉自啟的選擇）' ($null -eq (Get-TestReg 'Run' 'fc-host-nsis-test'))
        Check 'POSTINSTALL 覆蓋安裝：先刪既有目標再改名 → 「財經日曆.lnk」是新捷徑、來源消失（Rename 不會因目標已存在而失敗）' (
            (Read-Lnk $dir $zhLnk) -eq 'NEW' -and -not (Test-Path (Join-Path $dir 'sm\fc-host-nsis-test.lnk')))
        Check 'POSTINSTALL 覆蓋安裝：DisplayName 為「財經日曆」' ((Get-TestReg 'Uninstall\fc-host-nsis-test' 'DisplayName') -eq $zhName)

        # 首次安裝但範本沒建捷徑（/NS 等）：什麼都不建、不出錯
        $dir = New-InstallScenario 'first-nolnk' $false $null $null
        Invoke-Setup $dir
        Check 'POSTINSTALL 首次安裝但範本沒建捷徑：不產生任何捷徑' (@(Get-ChildItem (Join-Path $dir 'sm') -ErrorAction SilentlyContinue).Count -eq 0)

        # 解除安裝（非更新）：刪「財經日曆.lnk」與 StartupApproved 的值；Run 值由範本負責、hook 不碰
        $dir = New-InstallScenario 'uninst' $true $null $null
        Invoke-Setup $dir '/S /UPDATE'
        Write-Utf8NoBom (Join-Path $dir "sm\$zhLnk") 'LNK'
        Set-TestReg 'StartupApproved' 'fc-host-nsis-test' $approvedOff 'Binary'
        Set-TestReg 'StartupApproved' 'other-app' $approvedOn 'Binary'
        Set-TestReg 'Run' 'fc-host-nsis-test' 'RUNVAL'
        Invoke-Uninst $dir '/S'
        Check 'POSTUNINSTALL：刪除「財經日曆.lnk」' ($null -eq (Read-Lnk $dir $zhLnk))
        Check 'POSTUNINSTALL：刪除 StartupApproved\Run 的 fc-host 值' ($null -eq (Get-TestReg 'StartupApproved' 'fc-host-nsis-test'))
        Check 'POSTUNINSTALL：不動別的程式的 StartupApproved 值' ($null -ne (Get-TestReg 'StartupApproved' 'other-app'))
        Check 'POSTUNINSTALL：不重複刪 Run 值（那是範本的工作）' ((Get-TestReg 'Run' 'fc-host-nsis-test') -eq 'RUNVAL')

        # 解除安裝（/UPDATE）：捷徑與 StartupApproved 都不動
        $dir = New-InstallScenario 'uninst-update' $true $null $null
        Invoke-Setup $dir '/S /UPDATE'
        Write-Utf8NoBom (Join-Path $dir "sm\$zhLnk") 'LNK'
        Set-TestReg 'StartupApproved' 'fc-host-nsis-test' $approvedOff 'Binary'
        Invoke-Uninst $dir '/S /UPDATE'
        Check 'POSTUNINSTALL 更新模式：「財經日曆.lnk」與 StartupApproved 值都保留' ((Read-Lnk $dir $zhLnk) -eq 'LNK' -and $null -ne (Get-TestReg 'StartupApproved' 'fc-host-nsis-test'))

        # ---- 桌面捷徑（暫存位置 <情境資料夾>\desk；捷徑替身＝純文字「目標路徑<LF>標記」，見 harness 的 IsShortcutTarget）
        $srcLnk = 'fc-host-nsis-test.lnk'
        function Set-DeskLnk([string]$dir, [string]$file, [string]$target, [string]$tag) {
            $d = Join-Path $dir 'desk'
            if (-not (Test-Path -LiteralPath $d)) { New-Item -ItemType Directory -Force -Path $d | Out-Null }
            Write-Utf8NoBom (Join-Path $d $file) "$target`n$tag"
        }
        # 回傳標記（第二行）；檔案不存在回 $null
        function Read-DeskTag([string]$dir, [string]$file) {
            $f = Join-Path $dir "desk\$file"
            if (-not (Test-Path -LiteralPath $f)) { return $null }
            $lines = @((Get-Content -LiteralPath $f -Raw) -split "`n")
            if ($lines.Count -ge 2) { $lines[1].Trim() } else { '<no-tag>' }
        }
        function Get-DeskFiles([string]$dir) { @(Get-ChildItem -LiteralPath (Join-Path $dir 'desk') -ErrorAction SilentlyContinue | ForEach-Object Name | Sort-Object) }
        $foreign = 'C:\Somewhere\Else\other.exe'

        # 被動／靜默：範本在 POSTINSTALL 之前（installer.nsi:729-731）已建好來源捷徑 → POSTINSTALL 直接改名
        $dir = New-InstallScenario 'desk-silent' $false $null $null
        Set-DeskLnk $dir $srcLnk "$dir\fc-host-stub.exe" 'TPL'
        Invoke-Setup $dir '/S'
        Check '桌面 靜默安裝：範本建的 fc-host.lnk 在 POSTINSTALL 改名為「財經日曆.lnk」、只剩一個' (
            (Read-DeskTag $dir $zhLnk) -eq 'TPL' -and ((Get-DeskFiles $dir) -join '|') -eq $zhLnk) ((Get-DeskFiles $dir) -join '|')

        # 被動（有視窗）：POSTINSTALL 已改名，之後 .onGUIEnd 沒有來源可改、不出錯、不重複
        $dir = New-InstallScenario 'desk-passive' $false $null $null
        Set-DeskLnk $dir $srcLnk "$dir\fc-host-stub.exe" 'TPL'
        Invoke-Setup $dir '/S /GUI'
        Check '桌面 被動安裝：POSTINSTALL 改名後 .onGUIEnd 不再動作，只剩「財經日曆.lnk」' (
            (Read-DeskTag $dir $zhLnk) -eq 'TPL' -and ((Get-DeskFiles $dir) -join '|') -eq $zhLnk) ((Get-DeskFiles $dir) -join '|')

        # 互動、完成頁勾選：捷徑在 POSTINSTALL 之後才建立 → .onGUIEnd 改名
        $dir = New-InstallScenario 'desk-finish' $false $null $null
        Invoke-Setup $dir '/S /FINISHDESK /GUI'
        Check '桌面 互動安裝勾選：完成頁建的 fc-host.lnk 在 .onGUIEnd 改名為「財經日曆.lnk」、只剩一個' (
            (Read-DeskTag $dir $zhLnk) -eq 'FINISH' -and ((Get-DeskFiles $dir) -join '|') -eq $zhLnk) ((Get-DeskFiles $dir) -join '|')

        # 互動、完成頁沒勾：不產生任何桌面捷徑
        $dir = New-InstallScenario 'desk-unchecked' $false $null $null
        Invoke-Setup $dir '/S /GUI'
        Check '桌面 互動安裝沒勾：不產生桌面捷徑' (@(Get-DeskFiles $dir).Count -eq 0) ((Get-DeskFiles $dir) -join '|')

        # 同版本重裝（互動、勾選）：既有「財經日曆.lnk」指向本主程式 → 先刪再改名，不留兩個
        $dir = New-InstallScenario 'desk-reinstall' $true $null $null
        Set-DeskLnk $dir $zhLnk "$dir\fc-host-stub.exe" 'OLD'
        Invoke-Setup $dir '/S /FINISHDESK /GUI'
        Check '桌面 同版本重裝勾選：「財經日曆.lnk」換成新捷徑、只剩一個' (
            (Read-DeskTag $dir $zhLnk) -eq 'FINISH' -and ((Get-DeskFiles $dir) -join '|') -eq $zhLnk) ((Get-DeskFiles $dir) -join '|')

        # 同版本重裝（互動、沒勾）：既有「財經日曆.lnk」原封不動
        $dir = New-InstallScenario 'desk-reinstall-unchecked' $true $null $null
        Set-DeskLnk $dir $zhLnk "$dir\fc-host-stub.exe" 'OLD'
        Invoke-Setup $dir '/S /GUI'
        Check '桌面 同版本重裝沒勾：既有「財經日曆.lnk」保留' (
            (Read-DeskTag $dir $zhLnk) -eq 'OLD' -and ((Get-DeskFiles $dir) -join '|') -eq $zhLnk) ((Get-DeskFiles $dir) -join '|')

        # 使用者自己的「財經日曆.lnk」指向別處：不刪、不覆蓋（fc-host.lnk 留著）
        $dir = New-InstallScenario 'desk-user-own' $false $null $null
        Set-DeskLnk $dir $zhLnk $foreign 'USER'
        Invoke-Setup $dir '/S /FINISHDESK /GUI'
        Check '桌面 使用者自己的同名捷徑（目標不同）：保留它、不刪除' (
            (Read-DeskTag $dir $zhLnk) -eq 'USER' -and (Read-DeskTag $dir $srcLnk) -eq 'FINISH') ((Get-DeskFiles $dir) -join '|')

        # 來源 fc-host.lnk 目標不是本主程式：不改名
        $dir = New-InstallScenario 'desk-foreign-src' $false $null $null
        Set-DeskLnk $dir $srcLnk $foreign 'FOREIGN'
        Invoke-Setup $dir '/S /GUI'
        Check '桌面 fc-host.lnk 目標不是本主程式：不改名' (
            (Read-DeskTag $dir $srcLnk) -eq 'FOREIGN' -and $null -eq (Read-DeskTag $dir $zhLnk)) ((Get-DeskFiles $dir) -join '|')

        # 更新模式：範本不建桌面捷徑（installer.nsi:970-972）；既有「財經日曆.lnk」必須原封不動
        $dir = New-InstallScenario 'desk-update' $true $null $null
        Set-DeskLnk $dir $zhLnk "$dir\fc-host-stub.exe" 'KEEP'
        Invoke-Setup $dir '/S /UPDATE /GUI'
        Check '桌面 更新模式：沒有來源捷徑時不動既有「財經日曆.lnk」' (
            (Read-DeskTag $dir $zhLnk) -eq 'KEEP' -and ((Get-DeskFiles $dir) -join '|') -eq $zhLnk) ((Get-DeskFiles $dir) -join '|')

        # 取消安裝（POSTINSTALL 沒跑）：.onGUIEnd 不動桌面
        $dir = New-InstallScenario 'desk-cancel' $false $null $null
        Set-DeskLnk $dir $srcLnk "$dir\fc-host-stub.exe" 'LEFT'
        Invoke-Setup $dir '/S /CANCEL'
        Check '桌面 取消安裝：.onGUIEnd 不改名' ((Read-DeskTag $dir $srcLnk) -eq 'LEFT' -and $null -eq (Read-DeskTag $dir $zhLnk)) ((Get-DeskFiles $dir) -join '|')

        # 解除安裝（非更新）：桌面「財經日曆.lnk」目標是本主程式才刪
        $dir = New-InstallScenario 'desk-uninst' $true $null $null
        Invoke-Setup $dir '/S /UPDATE'
        Set-DeskLnk $dir $zhLnk "$dir\fc-host-stub.exe" 'OURS'
        Invoke-Uninst $dir '/S'
        Check 'POSTUNINSTALL：刪除桌面「財經日曆.lnk」（目標是本主程式）' ($null -eq (Read-DeskTag $dir $zhLnk)) ((Get-DeskFiles $dir) -join '|')

        $dir = New-InstallScenario 'desk-uninst-user' $true $null $null
        Invoke-Setup $dir '/S /UPDATE'
        Set-DeskLnk $dir $zhLnk $foreign 'USER'
        Invoke-Uninst $dir '/S'
        Check 'POSTUNINSTALL：桌面「財經日曆.lnk」目標不是本主程式 → 保留' ((Read-DeskTag $dir $zhLnk) -eq 'USER')

        $dir = New-InstallScenario 'desk-uninst-update' $true $null $null
        Invoke-Setup $dir '/S /UPDATE'
        Set-DeskLnk $dir $zhLnk "$dir\fc-host-stub.exe" 'OURS'
        Invoke-Uninst $dir '/S /UPDATE'
        Check 'POSTUNINSTALL 更新模式：桌面「財經日曆.lnk」保留' ((Read-DeskTag $dir $zhLnk) -eq 'OURS')
    }
}
finally {
    Reset-TestHiveSafe
    Remove-Item -LiteralPath $tmpRoot -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host "`n$($script:Pass) passed, $($script:Fail) failed, $($script:Skip) skipped"
if ($script:Fail -gt 0) { exit 1 }
exit 0
