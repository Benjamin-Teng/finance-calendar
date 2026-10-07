<#
.SYNOPSIS
    fc-host 安裝檔打包（installer-auto-update task 2.2、design.md D6、D7）：建置 NSIS 安裝檔、改成發佈檔名、
    以獨立步驟產生更新簽章 .sig 並驗證可信註解帶版本、（-Release）搜尋 e2e 後門字串。

.DESCRIPTION
    步驟（任一步失敗即停止，結束碼非 0）：

      1. 讀 host/Cargo.toml 的 version（版本只有這一個來源；tauri.conf.json 刻意不寫 version）。
      2. 檢查 host/installer/hooks.nsh 以 UTF-8 BOM（EF BB BF）開頭，否則失敗（防禦性要求：目前 bundler 以
         -INPUTCHARSET UTF8 呼叫 makensis，無 BOM 也能讀對；BOM 讓檔案不依賴 bundler 的呼叫方式）。
      3. -Release：tauri.conf.json 的 plugins.updater.pubkey 若不是真正的 minisign 公鑰（占位字串）就失敗；
         不得帶 -Config、不得帶 update-e2e feature。
      4. 檢查 tauri-cli 快取（%LOCALAPPDATA%\fc-host-tools\tauri-cli-<版本>[-arm64]），缺就下載主機架構的
         預編譯 zip、核對釘選的 SHA256 後解壓；解壓出的 cargo-tauri.exe 也核對釘選的 SHA256（快取命中時同樣核對，
         exe 不符就從驗過的 zip 重新解壓，仍不符就失敗）。
      5. 在 host/ 執行 `cargo tauri build --bundles nsis`（依參數帶 --target／--features／--config）。
         不需要 Node：tauri.conf.json 的 frontendDist 是靜態資料夾、沒有 beforeBuildCommand。
      6. 取 target\[<target>\]release\bundle\nsis\fc-host_<版本>_<arch>-setup.exe，複製成
         finance-calendar-setup.exe（x64）或 finance-calendar-setup-arm64.exe（aarch64）到 -OutDir。
      7. 以獨立步驟 `cargo-tauri signer sign --app-version <版本>` 對**最終安裝檔**產生 .sig（設計 D6：
         日後插入 Authenticode 簽章時 .sig 仍在最後一步）。把 .sig（base64）解碼，確認可信註解
         （trusted comment）的 version 欄位等於版本，且 file 欄位等於發佈檔名；不符就失敗——否則所有使用者
         （requireSignedVersion＝true）的更新都會靜默驗證失敗。
      8. -Release：在打包前的 fc-host.exe 與安裝檔中搜尋 e2e 設定檔的端點網址與公鑰字串，出現就失敗。
         注意：NSIS 安裝檔是 LZMA 整包壓縮，字串搜尋對安裝檔只能抓到未壓縮部分；真正擋得住的是對
         fc-host.exe 的搜尋（安裝檔裝入的就是這個檔，bundler 只在 bundle 前就地修補、不再變動它）。
      9. 輸出摘要：產物路徑、大小、SHA256、.sig 的可信註解。

    CI 分段模式（installer-auto-update task 5.2、design.md D6）：release workflow 把「建置」與「簽章」拆成兩個 job，
    讓建置 job（會執行依賴庫的 build.rs）完全讀不到私鑰：
      -SkipSign             只建置：做步驟 1–6 與 8（-Release 的前置檢查與後門搜尋照做），不簽章、不需要金鑰；
                            產物是尚未簽章的 finance-calendar-setup[-arm64].exe。
      -SignOnly -InstallerPath <已建好的安裝檔>
                            只簽章：對 CI 傳來的安裝檔就地產生 .sig，做步驟 7（含 -Release 的 key ID 核對與可信註解檢查）；
                            不建置、不動 Cargo.toml 以外的任何檔。檔名必須是發佈檔名之一（finance-calendar-setup.exe／
                            finance-calendar-setup-arm64.exe），可信註解的 file 欄位以它為準。

    機密：-SigningKeyPath 只吃私鑰**路徑**；-SigningKeyEnv 只吃存放私鑰**內容**的環境變數名稱（CI 用，私鑰不落地）；
    -SigningKeyPasswordEnv 只吃**環境變數名稱**（密碼本身不上命令列、不印出）。腳本只在簽章子行程的環境中設定
    TAURI_SIGNING_PRIVATE_KEY／TAURI_SIGNING_PRIVATE_KEY_PASSWORD，結束後還原。私鑰內容與密碼不出現在任何輸出或檔案。

    正式金鑰（D5）尚未建立前，本機打包用 e2e 設定與測試私鑰（無密碼）：
        $conf = '<暫存複本，pubkey 填測試公鑰>'
        pwsh -File host/tools/package.ps1 -Config $conf -SigningKeyPath $env:TEMP\fc-host-e2e-key\e2e.key
    -Release 在公鑰仍是占位字串時會失敗（這是刻意的）。

.PARAMETER Target
    Rust target triple。省略＝不傳 --target（以主機架構建置、輸出在 target\release）。
    x86_64-pc-windows-msvc → finance-calendar-setup.exe；aarch64-pc-windows-msvc → finance-calendar-setup-arm64.exe。

.PARAMETER Features
    傳給 cargo tauri build 的 Cargo features（可多個）。-Release 不得含 update-e2e。

.PARAMETER Config
    傳給 cargo tauri build --config 的額外設定檔（可多個，例如 host/tauri.e2e.conf.json 的暫存複本）。
    -Release 一律不接受。

.PARAMETER SigningKeyPath
    更新簽章私鑰檔路徑（tauri signer generate 產生的 minisign 私鑰）。簽章時 -SigningKeyPath 與 -SigningKeyEnv
    二選一必填；-SkipSign 時不得給。

.PARAMETER SigningKeyEnv
    存放私鑰**內容**的環境變數名稱（CI 把 secret 放進環境變數，私鑰不寫到磁碟）。與 -SigningKeyPath 互斥。

.PARAMETER SigningKeyPasswordEnv
    存放私鑰密碼的環境變數**名稱**。私鑰無密碼時省略。

.PARAMETER SkipSign
    只建置、不簽章（CI 建置 job；見上方「CI 分段模式」）。

.PARAMETER SignOnly
    只簽章、不建置（CI 發佈 job）；需 -InstallerPath。

.PARAMETER InstallerPath
    -SignOnly 要簽的安裝檔路徑（檔名須為發佈檔名）；.sig 寫在同資料夾。

.PARAMETER Release
    正式打包：啟用占位公鑰、-Config、update-e2e、後門字串四道檢查。

.PARAMETER OutDir
    發佈檔輸出資料夾，預設 <repo>\dist（.gitignore 已排除）。

.EXAMPLE
    pwsh -NoProfile -File host/tools/package.ps1 -SigningKeyPath "$env:USERPROFILE\.fc-host-signing\release.key" `
        -SigningKeyPasswordEnv FC_SIGNING_PASSWORD -Release
#>
[CmdletBinding()]
param(
    [string]$Target,
    [string[]]$Features = @(),
    [string[]]$Config = @(),
    [string]$SigningKeyPath,
    [string]$SigningKeyEnv,
    [string]$SigningKeyPasswordEnv,
    [switch]$SkipSign,
    [switch]$SignOnly,
    [string]$InstallerPath,
    [switch]$Release,
    [string]$OutDir
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# ================================================================================================
# 純函式（package.Tests.ps1 以語法樹取出本檔所有函式定義後測試；函式不得依賴腳本層級變數）
# ================================================================================================

# 釘選的 tauri-cli 版本與各架構預編譯檔的 SHA256。
# 來源：`gh release view tauri-cli-v2.12.1 -R tauri-apps/tauri --json assets` 的 digest（GitHub 依上傳檔計算），
# 兩個架構的 zip 雜湊都已對實際下載檔以 Get-FileHash 實算核對相符。
# ExeSha256＝zip 解壓出的 cargo-tauri.exe 的 SHA256（對已核對的 zip 解壓後實算）：快取命中時除 zip 外也要比對它，
# 否則解壓後的 exe 被改掉、zip 仍完好時會用到被竄改的執行檔（而它接著會讀到簽章私鑰）。
function Get-PackageConstants {
    @{
        TauriCliVersion  = '2.12.1'
        TauriCliTag      = 'tauri-cli-v2.12.1'
        TauriCliAssets   = @{
            'x86_64-pc-windows-msvc'  = @{ Zip = 'cargo-tauri-x86_64-pc-windows-msvc.zip';  Sha256 = 'd3919ebe0bf7013dd98293862784dd37b8f76db2d650e13a3ec11c5dd799c519'; ExeSha256 = 'c5f10492b295d8a6d7aac4dc8d735b28e25cc8b19d576bf649d5367408de01be'; CacheSuffix = '' }
            'aarch64-pc-windows-msvc' = @{ Zip = 'cargo-tauri-aarch64-pc-windows-msvc.zip'; Sha256 = '74b33a3841fecef620c89a1e3dd7aedf727ad0f811a44d3e5784739c1df607b1'; ExeSha256 = 'f7e10fe54b367a0fdc2a2873dce595aa71e8f2f22ace5a1da752647fd7bbda3a'; CacheSuffix = '-arm64' }
        }
        # tauri.conf.json 與 e2e 設定檔使用的占位字串（真正的 minisign 公鑰不會長這樣）
        PubkeyPlaceholders = @('PRODUCTION_PUBKEY_PENDING_TASK_1_1', 'E2E_TEST_PUBKEY_PLACEHOLDER')
        E2eFeature       = 'update-e2e'
        PublishNames     = @{
            'x86_64-pc-windows-msvc'  = @{ File = 'finance-calendar-setup.exe';       Arch = 'x64' }
            'aarch64-pc-windows-msvc' = @{ File = 'finance-calendar-setup-arm64.exe'; Arch = 'arm64' }
        }
    }
}

# 主機（作業系統）架構對應的 target triple。用 OSArchitecture 而非 ProcessArchitecture：
# ARM64 機器上若跑的是 x64 模擬的 pwsh，仍應取原生 ARM64 的 tauri-cli。
function Get-HostTriple {
    switch ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()) {
        'X64' { 'x86_64-pc-windows-msvc' }
        'Arm64' { 'aarch64-pc-windows-msvc' }
        default { throw "不支援的主機架構：$_（只支援 X64、Arm64）" }
    }
}

# 讀 Cargo.toml `[package]` 區段的 version。
function Get-CargoPackageVersion([string]$CargoToml) {
    $inPackage = $false
    foreach ($line in (Get-Content -LiteralPath $CargoToml -Encoding UTF8)) {
        if ($line -match '^\s*\[(.+?)\]\s*$') { $inPackage = ($Matches[1] -eq 'package'); continue }
        if ($inPackage -and $line -match '^\s*version\s*=\s*"([^"]+)"') { return $Matches[1] }
    }
    throw "在 $CargoToml 的 [package] 找不到 version"
}

# 檔案是否以 UTF-8 BOM（EF BB BF）開頭。
function Test-Utf8Bom([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { return $false }
    $fs = [System.IO.File]::OpenRead($Path)
    try {
        $buf = New-Object byte[] 3
        $n = $fs.Read($buf, 0, 3)
        return ($n -eq 3 -and $buf[0] -eq 0xEF -and $buf[1] -eq 0xBB -and $buf[2] -eq 0xBF)
    }
    finally { $fs.Dispose() }
}

# 公鑰字串是否是真正的 minisign 公鑰：base64 解碼後是
# 「untrusted comment: …」＋換行＋56 字元 base64（2 位元組演算法＋8 位元組金鑰 ID＋32 位元組公鑰＝42 位元組）。
# 占位字串（PRODUCTION_PUBKEY_PENDING_…、E2E_TEST_PUBKEY_PLACEHOLDER）解不出這個格式。
function Test-MinisignPubkey([string]$Pubkey) {
    if ([string]::IsNullOrWhiteSpace($Pubkey)) { return $false }
    try { $text = [System.Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($Pubkey.Trim())) }
    catch { return $false }
    return [bool]($text -match '^untrusted comment: [^\r\n]*\r?\n[A-Za-z0-9+/]{56}\s*$')
}

# 正式打包前置檢查：回傳錯誤訊息陣列（空＝通過）。不做 I/O 以外的事，方便測試。
#   - 公鑰仍是占位字串 → 失敗
#   - 帶了 -Config（e2e 設定等）→ 失敗
#   - Features 含 update-e2e、self-test-ipc、probe-render（逗號或空白串接、`<套件>/<feature>` 寫法也算，
#     cargo 接受 `a,b` 與 `fc-host/a`）→ 失敗
#     （後兩者是驗收／量測用 feature，會多出 self_test_* 指令或接管整個行程的探針，不得進正式版）
#   - 公鑰與 $RejectPubkeys（e2e 測試公鑰）相同、或 key ID 相同 → 失敗（格式合法的測試公鑰會通過格式檢查）
#   - 基礎設定的 plugins.updater.endpoints 必須存在且全為 https://；任何 dangerous* 旗標為真 → 失敗
#     （e2e 值直接改進 tauri.conf.json 時，不靠 e2e 檔的網址比對就能擋下）
function Get-ReleaseViolations([string]$ConfJsonPath, [string[]]$Config, [string[]]$Features, [string]$E2eFeature, [string[]]$RejectPubkeys = @()) {
    $errs = [System.Collections.Generic.List[string]]::new()
    $pubkey = $null
    $upd = $null
    $readOk = $true
    try {
        $json = Get-Content -LiteralPath $ConfJsonPath -Raw -Encoding UTF8 | ConvertFrom-Json
        $upd = $json.plugins.updater
        $pubkey = $upd.pubkey
    }
    catch { $readOk = $false; $errs.Add("讀不到 $ConfJsonPath 的 plugins.updater.pubkey：$($_.Exception.Message)") }
    if ($readOk) {
        if (-not (Test-MinisignPubkey $pubkey)) {
            $shown = if ($pubkey) { "「$pubkey」" } else { '（空）' }
            $errs.Add("plugins.updater.pubkey 不是有效的 minisign 公鑰（目前是 $shown；占位字串代表正式金鑰尚未產生，D5）")
        }
        else {
            $myId = Get-MinisignKeyId $pubkey
            foreach ($bad in @($RejectPubkeys)) {
                if (-not $bad) { continue }
                $badId = $null
                if (Test-MinisignPubkey $bad) { $badId = Get-MinisignKeyId $bad }
                if ($pubkey.Trim() -eq $bad.Trim() -or ($badId -and $badId -eq $myId)) {
                    $errs.Add("plugins.updater.pubkey 是 e2e 測試公鑰（key ID $(ConvertTo-KeyIdDisplay $myId)）；正式打包必須用正式金鑰的公鑰")
                    break
                }
            }
        }
        $props = @($upd.PSObject.Properties)
        $endpoints = @($props | Where-Object { $_.Name -eq 'endpoints' } | ForEach-Object { $_.Value })
        if ($endpoints.Count -eq 0) { $errs.Add('plugins.updater.endpoints 不存在或是空的') }
        foreach ($ep in $endpoints) {
            if (-not ([string]$ep).StartsWith('https://')) { $errs.Add("plugins.updater.endpoints 含非 https 端點：$ep") }
        }
        foreach ($p in ($props | Where-Object { $_.Name -like 'dangerous*' })) {
            if ($p.Value) { $errs.Add("plugins.updater.$($p.Name) 不得為真（正式建置不允許放寬更新通道的安全檢查）") }
        }
    }
    if (@($Config).Count -gt 0) { $errs.Add("-Release 不接受 -Config（收到：$(@($Config) -join ', ')）；e2e 設定只能用於非正式打包") }
    $allFeatures = @(@($Features) | ForEach-Object { $_ -split '[,\s]+' } | Where-Object { $_ })
    # cargo 也接受 `<套件>/<feature>`（例如 `fc-host/self-test-ipc`）：取最後一個 `/` 之後的部分再比對
    $allFeatures = @($allFeatures | ForEach-Object { ($_ -split '/')[-1] })
    foreach ($forbidden in @($E2eFeature, 'self-test-ipc', 'probe-render')) {
        if ($allFeatures -contains $forbidden) { $errs.Add("-Release 不得帶 $forbidden feature") }
    }
    $errs.ToArray()
}

# minisign 公鑰／簽章的 key ID（8 位元組，十六進位大寫）。兩者都是「base64(文字檔)」，文字檔第二行再 base64：
# 公鑰＝2 位元組演算法＋8 位元組 key ID＋32 位元組公鑰；簽章＝2 位元組演算法＋8 位元組 key ID＋64 位元組簽章。
# 所以兩者的位元組 [2..9] 是同一個 key ID；簽章用的私鑰與內建公鑰是否同一把，看這個即可。
function Get-MinisignKeyId([string]$Base64Text) {
    try { $text = [System.Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($Base64Text.Trim())) }
    catch { throw "不是有效的 base64（minisign 公鑰或簽章）：$($_.Exception.Message)" }
    $lines = @($text -split "\r?\n")
    if ($lines.Count -lt 2) { throw 'minisign 內容少於兩行' }
    try { $bytes = [Convert]::FromBase64String($lines[1].Trim()) }
    catch { throw "minisign 第二行不是有效的 base64：$($_.Exception.Message)" }
    if ($bytes.Length -lt 10) { throw "minisign 第二行解碼後只有 $($bytes.Length) 位元組（至少 10）" }
    ($bytes[2..9] | ForEach-Object { $_.ToString('X2') }) -join ''
}

# key ID 的顯示序：Get-MinisignKeyId 回傳的是檔內位元組序；`.pub` 的 untrusted comment 與 minisign 工具顯示的
# 是同 8 位元組當**小端 u64** 的十六進位，兩者互為位元組反序。錯誤訊息一律用顯示序，才能拿去對 `.pub` 的
# comment；比較仍用檔內序（兩邊同序，相等判斷不受影響）。
function ConvertTo-KeyIdDisplay([string]$KeyIdHex) {
    $pairs = @(for ($i = 0; $i + 1 -lt $KeyIdHex.Length; $i += 2) { $KeyIdHex.Substring($i, 2) })
    [array]::Reverse($pairs)
    $pairs -join ''
}

# .sig 檔（單行 base64）的 key ID。
function Get-SigKeyId([string]$SigPath) {
    Get-MinisignKeyId (Get-Content -LiteralPath $SigPath -Raw -Encoding ASCII)
}

# 解碼 .sig（tauri 把 minisign 簽章檔文字再 base64 一次寫入），回傳可信註解（trusted comment: 之後的整行）。
function Get-SigTrustedComment([string]$SigPath) {
    $raw = (Get-Content -LiteralPath $SigPath -Raw -Encoding ASCII).Trim()
    try { $text = [System.Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($raw)) }
    catch { throw "$SigPath 不是有效的 base64：$($_.Exception.Message)" }
    foreach ($line in ($text -split "\r?\n")) {
        if ($line.StartsWith('trusted comment: ')) { return $line.Substring('trusted comment: '.Length) }
    }
    throw "$SigPath 解碼後找不到 'trusted comment:' 行（不是 minisign 簽章？）"
}

# 可信註解是一行以 tab 分隔的欄位（tauri-cli signer sign：timestamp:<秒>\tfile:<檔名>[\tversion:<版本>]）。
# 回傳欄位名稱→值的雜湊表；沒有 `:` 的欄位略過。
function ConvertFrom-TrustedComment([string]$Comment) {
    $fields = @{}
    foreach ($part in ($Comment -split "`t")) {
        $i = $part.IndexOf(':')
        if ($i -gt 0) { $fields[$part.Substring(0, $i)] = $part.Substring($i + 1) }
    }
    $fields
}

# 驗證 .sig 的可信註解：version 欄位必須**完全等於**版本（不是「包含」），file 欄位必須等於發佈檔名。
# 回傳錯誤訊息陣列（空＝通過）。
function Test-SigTrustedComment([string]$Comment, [string]$Version, [string]$FileName) {
    $errs = [System.Collections.Generic.List[string]]::new()
    $f = ConvertFrom-TrustedComment $Comment
    if (-not $f.ContainsKey('version')) { $errs.Add("可信註解沒有 version 欄位（requireSignedVersion 會讓所有使用者的更新驗證失敗）：$Comment") }
    elseif ($f['version'] -ne $Version) { $errs.Add("可信註解的 version 是「$($f['version'])」，應為「$Version」") }
    if (-not $f.ContainsKey('file')) { $errs.Add("可信註解沒有 file 欄位：$Comment") }
    elseif ($f['file'] -ne $FileName) { $errs.Add("可信註解的 file 是「$($f['file'])」，應為「$FileName」") }
    $errs.ToArray()
}

# 讀 e2e 測試公鑰：目錄下所有 *.pub 的內容（Trim）。目錄不存在、檔案讀不到或內容為空時略過該檔，
# 不丟例外（空的 .pub 不得中止非 -Release 的打包；task 2 複審 N2）。
function Get-E2ePubkeys([string]$Dir) {
    if (-not (Test-Path -LiteralPath $Dir -PathType Container)) { return @() }
    $list = [System.Collections.Generic.List[string]]::new()
    foreach ($f in @(Get-ChildItem -LiteralPath $Dir -Filter '*.pub' -File -ErrorAction SilentlyContinue)) {
        $text = ''
        try { $text = "$(Get-Content -LiteralPath $f.FullName -Raw -ErrorAction Stop)".Trim() } catch { $text = '' }
        if ($text) { $list.Add($text) }
    }
    $list.ToArray()
}

# 參數組合檢查（純函式）：回傳錯誤訊息陣列（空＝通過）。$P 是含 Target／Features／Config／SigningKeyPath／
# SigningKeyEnv／SigningKeyPasswordEnv／SkipSign／SignOnly／InstallerPath 鍵的雜湊表。
#   - -SkipSign 與 -SignOnly 互斥；-SkipSign 不得給任何金鑰參數（建置 job 不該碰得到私鑰）
#   - -SignOnly 必須有 -InstallerPath、不得帶建置參數（-Target／-Features／-Config）；-InstallerPath 只能搭配 -SignOnly
#   - 要簽章（非 -SkipSign）時，-SigningKeyPath 與 -SigningKeyEnv 恰好給一個
function Get-ParameterViolations([hashtable]$P) {
    $errs = [System.Collections.Generic.List[string]]::new()
    $hasKey = [bool]$P.SigningKeyPath -or [bool]$P.SigningKeyEnv
    if ($P.SkipSign -and $P.SignOnly) { $errs.Add('-SkipSign 與 -SignOnly 不能同時使用') }
    if ($P.SkipSign) {
        if ($hasKey -or $P.SigningKeyPasswordEnv) { $errs.Add('-SkipSign 不得帶 -SigningKeyPath／-SigningKeyEnv／-SigningKeyPasswordEnv（建置階段不得接觸簽章金鑰）') }
    }
    else {
        if ($P.SigningKeyPath -and $P.SigningKeyEnv) { $errs.Add('-SigningKeyPath 與 -SigningKeyEnv 只能擇一') }
        elseif (-not $hasKey) { $errs.Add('需要簽章金鑰：請給 -SigningKeyPath 或 -SigningKeyEnv（只建置請用 -SkipSign）') }
    }
    if ($P.SignOnly) {
        if (-not $P.InstallerPath) { $errs.Add('-SignOnly 需要 -InstallerPath') }
        if ($P.Target -or @($P.Features).Count -gt 0 -or @($P.Config).Count -gt 0) { $errs.Add('-SignOnly 不建置，不接受 -Target／-Features／-Config') }
    }
    elseif ($P.InstallerPath) { $errs.Add('-InstallerPath 只能搭配 -SignOnly') }
    $errs.ToArray()
}

# 對安裝檔就地產生 .sig 並驗證（步驟 7；建置後簽章與 -SignOnly 共用）。回傳含 SigPath、Comment 的雜湊表。
# 私鑰來源二選一：$KeyPath（檔案路徑，以 -f 傳給 signer）或 $KeyEnv（環境變數名稱，內容在簽章子行程以
# TAURI_SIGNING_PRIVATE_KEY 傳入、結束後還原，不落地）；密碼同樣只在子行程環境中出現。私鑰與密碼的值不印出。
# $IsRelease：簽章後核對 .sig 的 key ID 與 tauri.conf.json 內建公鑰相同，不符就刪 .sig
# （$RemoveInstallerOnMismatch 時連安裝檔一併刪）並失敗。
function Invoke-InstallerSigning([string]$Cli, [string]$Dest, [string]$Version, [string]$FileName, [string]$ConfPath,
    [string]$KeyPath, [string]$KeyEnv, [string]$PasswordEnv, [bool]$IsRelease, [bool]$RemoveInstallerOnMismatch) {
    $sigPath = "$Dest.sig"
    if (Test-Path -LiteralPath $sigPath) { Remove-Item -LiteralPath $sigPath -Force }
    $passwordValue = $null
    if ($PasswordEnv) {
        $passwordValue = [Environment]::GetEnvironmentVariable($PasswordEnv)
        if ([string]::IsNullOrEmpty($passwordValue)) { throw "環境變數 $PasswordEnv 不存在或是空的" }
    }
    $keyValue = $null
    if ($KeyEnv) {
        $keyValue = [Environment]::GetEnvironmentVariable($KeyEnv)
        if ([string]::IsNullOrWhiteSpace($keyValue)) { throw "環境變數 $KeyEnv（私鑰內容）不存在或是空的" }
    }
    elseif (-not (Test-Path -LiteralPath $KeyPath -PathType Leaf)) { throw "-SigningKeyPath 檔案不存在：$KeyPath" }

    # 密碼與私鑰內容只放進簽章子行程的環境變數，結束後還原
    $pwdEnvName = 'TAURI_SIGNING_PRIVATE_KEY_PASSWORD'
    $keyEnvName = 'TAURI_SIGNING_PRIVATE_KEY'
    $savedPassword = [Environment]::GetEnvironmentVariable($pwdEnvName)
    $savedKey = [Environment]::GetEnvironmentVariable($keyEnvName)
    try {
        if ($null -ne $passwordValue) { [Environment]::SetEnvironmentVariable($pwdEnvName, $passwordValue) }
        if ($null -ne $keyValue) { [Environment]::SetEnvironmentVariable($keyEnvName, $keyValue) }
        Write-Host "產生更新簽章：signer sign --app-version $Version（私鑰與密碼不印出）"
        if ($null -eq $passwordValue) {
            Write-Host '（未指定 -SigningKeyPasswordEnv：假設私鑰沒有密碼；私鑰有密碼時 signer 會在互動終端提示輸入，請改用 -SigningKeyPasswordEnv）'
        }
        $signArgs = @('signer', 'sign', '--app-version', $Version)
        if ($KeyPath) { $signArgs += @('-f', $KeyPath) }
        $signArgs += $Dest
        $signOut = & $Cli @signArgs 2>&1
        if ($LASTEXITCODE -ne 0) {
            # signer 的輸出只有路徑與錯誤原因（不含私鑰或密碼），失敗時印出方便判斷
            $signOut | ForEach-Object { Write-Host "  signer: $_" }
            throw "signer sign 失敗，結束碼 $LASTEXITCODE"
        }
    }
    finally {
        [Environment]::SetEnvironmentVariable($pwdEnvName, $savedPassword)
        [Environment]::SetEnvironmentVariable($keyEnvName, $savedKey)
    }
    if (-not (Test-Path -LiteralPath $sigPath)) { throw "簽章後找不到 $sigPath" }
    if ($IsRelease) {
        # 簽章用的私鑰必須與 tauri.conf.json 內建的公鑰是同一把：否則 v0.1.0 出貨後所有更新在使用者端驗簽失敗，
        # 而公鑰寫死在使用者的程式裡、無法靠更新修復
        $pubJson = Get-Content -LiteralPath $ConfPath -Raw -Encoding UTF8 | ConvertFrom-Json
        $pubId = Get-MinisignKeyId $pubJson.plugins.updater.pubkey
        $sigId = Get-SigKeyId $sigPath
        if ($pubId -ne $sigId) {
            # 不留下不可發佈的產物，避免被誤上傳
            $bads = if ($RemoveInstallerOnMismatch) { @($Dest, $sigPath) } else { @($sigPath) }
            foreach ($bad in $bads) { Remove-Item -LiteralPath $bad -Force -ErrorAction SilentlyContinue }
            throw "簽章的 key ID（$(ConvertTo-KeyIdDisplay $sigId)）與 tauri.conf.json 內建公鑰的 key ID（$(ConvertTo-KeyIdDisplay $pubId)）不同（顯示序同 .pub 的 comment）：簽章金鑰不是這把公鑰對應的私鑰。已刪除剛簽出的產物"
        }
        Write-Host "key ID 核對：簽章與內建公鑰皆為 $(ConvertTo-KeyIdDisplay $sigId)"
    }
    $comment = Get-SigTrustedComment $sigPath
    $sigErrs = @(Test-SigTrustedComment $comment $Version $FileName)
    if ($sigErrs.Count -gt 0) { throw ("簽章可信註解檢查失敗：`n  - " + ($sigErrs -join "`n  - ")) }
    Write-Host "可信註解（已解碼）：$($comment -replace "`t", ' | ')"
    @{ SigPath = $sigPath; Comment = $comment }
}

# 在檔案位元組中搜尋字串（同時找 UTF-8／ASCII 與 UTF-16LE 兩種編碼），回傳「找到的字串」陣列。
# 以 Latin-1 把位元組一對一轉成字元後用 ordinal IndexOf，30 MB 級的檔案不需逐位元組迴圈。
function Find-ForbiddenStrings([string]$Path, [string[]]$Needles) {
    $bytes = [System.IO.File]::ReadAllBytes($Path)
    $hay = [System.Text.Encoding]::Latin1.GetString($bytes)
    $found = [System.Collections.Generic.List[string]]::new()
    foreach ($needle in $Needles) {
        if ([string]::IsNullOrEmpty($needle)) { continue }
        $utf8 = [System.Text.Encoding]::Latin1.GetString([System.Text.Encoding]::UTF8.GetBytes($needle))
        $utf16 = [System.Text.Encoding]::Latin1.GetString([System.Text.Encoding]::Unicode.GetBytes($needle))
        if ($hay.IndexOf($utf8, [StringComparison]::Ordinal) -ge 0 -or $hay.IndexOf($utf16, [StringComparison]::Ordinal) -ge 0) {
            $found.Add($needle)
        }
    }
    $found.ToArray()
}

# e2e 設定檔中不得出現在正式建置裡的字串：所有 http:// 端點的完整網址，以及公鑰字串（占位字串或測試公鑰）。
function Get-BackdoorNeedles([string]$E2eConfPath, [string[]]$ExtraPubkeys) {
    $needles = [System.Collections.Generic.List[string]]::new()
    $json = Get-Content -LiteralPath $E2eConfPath -Raw -Encoding UTF8 | ConvertFrom-Json
    $upd = $json.plugins.updater
    foreach ($ep in @($upd.endpoints)) { if ($ep -and $ep.StartsWith('http://')) { $needles.Add($ep) } }
    if ($upd.pubkey) { $needles.Add([string]$upd.pubkey) }
    foreach ($k in @($ExtraPubkeys)) { if ($k) { $needles.Add($k.Trim()) } }
    $needles | Select-Object -Unique
}

# 執行外部程式；結束碼非 0 就丟例外。不印出參數（參數可能含路徑，但絕不含機密——機密只走環境變數）。
function Invoke-Native([string]$Exe, [string[]]$Arguments, [string]$WorkingDirectory) {
    Push-Location $WorkingDirectory
    try {
        & $Exe @Arguments
        if ($LASTEXITCODE -ne 0) { throw "$([System.IO.Path]::GetFileName($Exe)) $($Arguments[0..1] -join ' ') 失敗，結束碼 $LASTEXITCODE" }
    }
    finally { Pop-Location }
}

# 確保 tauri-cli 在快取目錄；回傳 cargo-tauri.exe 路徑。
# 快取：%LOCALAPPDATA%\fc-host-tools\tauri-cli-<版本>（x64）／…-arm64（ARM64）。
# 下載的 zip 一律核對釘選的 SHA256，不符就刪掉並失敗；解壓出的 cargo-tauri.exe 另核對 ExeSha256
# （快取命中時也核對，不符就從驗過的 zip 重新解壓；仍不符就失敗），不使用未核對的執行檔。
function Initialize-TauriCli([string]$HostTriple, [hashtable]$Const) {
    $asset = $Const.TauriCliAssets[$HostTriple]
    $dir = Join-Path $env:LOCALAPPDATA "fc-host-tools\tauri-cli-$($Const.TauriCliVersion)$($asset.CacheSuffix)"
    $exe = Join-Path $dir 'cargo-tauri.exe'
    $zip = Join-Path $dir $asset.Zip
    New-Item -ItemType Directory -Force -Path $dir | Out-Null

    # 已解壓、zip 核對相符、且解壓出的 cargo-tauri.exe 也與釘選的 ExeSha256 相符 → 直接用。
    # 只驗 zip 不夠：exe 在解壓後被改掉時 zip 仍完好；exe 不符就落到下面，從驗過的 zip 重新解壓。
    if ((Test-Path -LiteralPath $exe) -and (Test-Path -LiteralPath $zip) -and
        ((Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash -ieq $asset.Sha256) -and
        ((Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash -ieq $asset.ExeSha256)) {
        return $exe
    }
    if (-not ((Test-Path -LiteralPath $zip) -and ((Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash -ieq $asset.Sha256))) {
        $url = "https://github.com/tauri-apps/tauri/releases/download/$($Const.TauriCliTag)/$($asset.Zip)"
        Write-Host "下載 tauri-cli $($Const.TauriCliVersion)：$url"
        Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing
        $actual = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash
        if ($actual -ine $asset.Sha256) {
            Remove-Item -LiteralPath $zip -Force
            throw "tauri-cli zip 的 SHA256 不符：實際 $actual、釘選 $($asset.Sha256)；已刪除下載檔，未使用"
        }
    }
    Expand-Archive -LiteralPath $zip -DestinationPath $dir -Force
    if (-not (Test-Path -LiteralPath $exe)) { throw "解壓後找不到 $exe" }
    $exeActual = (Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash
    if ($exeActual -ine $asset.ExeSha256) {
        Remove-Item -LiteralPath $exe -Force
        throw "cargo-tauri.exe 的 SHA256 不符：實際 $exeActual、釘選 $($asset.ExeSha256)（zip 已核對相符，卻解出不同的執行檔）；已刪除，未使用"
    }
    $exe
}

# ================================================================================================
# 主流程
# ================================================================================================
function Invoke-Package {
    $const = Get-PackageConstants
    $toolsDir = $PSScriptRoot
    $hostDir = Split-Path $toolsDir -Parent
    $repoRoot = Split-Path $hostDir -Parent
    $confPath = Join-Path $hostDir 'tauri.conf.json'
    $hooksPath = Join-Path $hostDir 'installer\hooks.nsh'
    $e2eConfPath = Join-Path $hostDir 'tauri.e2e.conf.json'
    if (-not $OutDir) { $OutDir = Join-Path $repoRoot 'dist' }

    # 0. 參數組合（-SkipSign／-SignOnly／金鑰來源）
    $paramErrs = @(Get-ParameterViolations @{
            Target = $Target; Features = $Features; Config = $Config
            SigningKeyPath = $SigningKeyPath; SigningKeyEnv = $SigningKeyEnv; SigningKeyPasswordEnv = $SigningKeyPasswordEnv
            SkipSign = [bool]$SkipSign; SignOnly = [bool]$SignOnly; InstallerPath = $InstallerPath
        })
    if ($paramErrs.Count -gt 0) { throw ("參數錯誤：`n  - " + ($paramErrs -join "`n  - ")) }

    # 1. 版本
    $version = Get-CargoPackageVersion (Join-Path $hostDir 'Cargo.toml')
    Write-Host "版本：$version（host/Cargo.toml）"

    # 2. hooks.nsh BOM（-SignOnly 不建置，已在建置階段檢查過）
    if (-not $SignOnly) {
        if (-not (Test-Utf8Bom $hooksPath)) { throw "$hooksPath 不是以 UTF-8 BOM（EF BB BF）開頭；這是防禦性要求（讓 hooks.nsh 的中文不依賴 bundler 的 -INPUTCHARSET 呼叫方式）" }
        Write-Host 'hooks.nsh：UTF-8 BOM 正確'
    }

    # 3. 目標與正式打包前置檢查
    $hostTriple = Get-HostTriple
    $effectiveTriple = if ($Target) { $Target } else { $hostTriple }
    if (-not $const.PublishNames.ContainsKey($effectiveTriple)) {
        throw "不支援的 target：$effectiveTriple（支援：$($const.PublishNames.Keys -join ', ')）"
    }
    $publish = $const.PublishNames[$effectiveTriple]
    if ($SignOnly) {
        # 發佈檔名由安裝檔檔名決定（CI 的 artifact 就是發佈檔名），不是由主機架構決定
        if (-not (Test-Path -LiteralPath $InstallerPath -PathType Leaf)) { throw "-InstallerPath 檔案不存在：$InstallerPath" }
        $leaf = Split-Path $InstallerPath -Leaf
        $match = @($const.PublishNames.Values | Where-Object { $_.File -eq $leaf })
        if ($match.Count -ne 1) { throw "-InstallerPath 的檔名必須是發佈檔名（$(@($const.PublishNames.Values | ForEach-Object { $_.File }) -join '、')）：$leaf" }
        $publish = $match[0]
    }
    # 本機慣例的 e2e 測試公鑰（D7：拋棄式金鑰放 %TEMP%\fc-host-e2e-key）：-Release 拒絕用它當內建公鑰，
    # 並把它列入後門字串搜尋。目錄或 .pub 不存在、內容為空時略過該檔，不中止打包（task 2 複審 N2）
    $e2ePubs = @(Get-E2ePubkeys (Join-Path $env:TEMP 'fc-host-e2e-key'))
    if ($Release) {
        $violations = @(Get-ReleaseViolations $confPath $Config $Features $const.E2eFeature $e2ePubs)
        if ($violations.Count -gt 0) {
            throw ("-Release 前置檢查失敗：`n  - " + ($violations -join "`n  - "))
        }
        Write-Host '-Release 前置檢查：公鑰有效且不是 e2e 測試公鑰、端點全為 https、無 dangerous 旗標、無 -Config、無 update-e2e'
    }
    foreach ($c in $Config) { if (-not (Test-Path -LiteralPath $c -PathType Leaf)) { throw "-Config 檔案不存在：$c" } }
    if ($SigningKeyPath -and -not (Test-Path -LiteralPath $SigningKeyPath -PathType Leaf)) { throw "-SigningKeyPath 檔案不存在：$SigningKeyPath" }
    # 密碼／私鑰內容環境變數：這裡先檢查「存在且非空」（建置要好幾分鐘，別等建完才發現），值只在 Invoke-InstallerSigning 內讀取
    foreach ($envName in @($SigningKeyPasswordEnv, $SigningKeyEnv) | Where-Object { $_ }) {
        if ([string]::IsNullOrWhiteSpace([Environment]::GetEnvironmentVariable($envName))) { throw "環境變數 $envName 不存在或是空的" }
    }

    # 4. tauri-cli
    $cli = Initialize-TauriCli $hostTriple $const
    Write-Host "tauri-cli：$cli（$(& $cli --version)）"

    if ($SignOnly) {
        # -SignOnly：只簽章（CI 發佈 job）。就地簽 -InstallerPath，不建置
        $dest = (Resolve-Path -LiteralPath $InstallerPath).Path
        $signed = Invoke-InstallerSigning $cli $dest $version $publish.File $confPath $SigningKeyPath $SigningKeyEnv $SigningKeyPasswordEnv ([bool]$Release) $false
        $item = Get-Item -LiteralPath $dest
        $hash = (Get-FileHash -LiteralPath $dest -Algorithm SHA256).Hash.ToLowerInvariant()
        Write-Host ''
        Write-Host '=== 簽章完成（-SignOnly）==='
        Write-Host "安裝檔 ：$($item.FullName)"
        Write-Host ("大小   ：{0:N0} bytes（{1:N2} MiB）" -f $item.Length, ($item.Length / 1MB))
        Write-Host "SHA256 ：$hash"
        Write-Host "簽章   ：$($signed.SigPath)"
        return
    }

    # 5. cargo tauri build
    $buildArgs = @('tauri', 'build', '--bundles', 'nsis')
    if ($Target) { $buildArgs += @('--target', $Target) }
    foreach ($f in $Features) { $buildArgs += @('--features', $f) }
    foreach ($c in $Config) { $buildArgs += @('--config', (Resolve-Path -LiteralPath $c).Path) }
    Write-Host "cargo tauri build（在 $hostDir）：$($buildArgs -join ' ')"
    $cargoTomlPath = Join-Path $hostDir 'Cargo.toml'
    $cargoHashBefore = (Get-FileHash -LiteralPath $cargoTomlPath -Algorithm SHA256).Hash
    Invoke-Native $cli $buildArgs $hostDir
    # tauri-cli 會依設定就地同步依賴 features（實測：tauri-build 補上 features = []）。有變就警告；正式打包直接失敗，
    # 否則產物與 `cargo build`／測試用的依賴 features 會悄悄分岔
    if ((Get-FileHash -LiteralPath $cargoTomlPath -Algorithm SHA256).Hash -ne $cargoHashBefore) {
        $msg = 'cargo tauri build 改寫了 host/Cargo.toml（tauri-cli 同步依賴 features）；請檢視 git diff 並提交後重跑'
        if ($Release) { throw $msg }
        Write-Warning $msg
    }

    # 6. 找安裝檔、複製成發佈檔名
    $releaseDir = if ($Target) { Join-Path $hostDir "target\$Target\release" } else { Join-Path $hostDir 'target\release' }
    $bundleName = "fc-host_${version}_$($publish.Arch)-setup.exe"
    $bundled = Join-Path $releaseDir "bundle\nsis\$bundleName"
    if (-not (Test-Path -LiteralPath $bundled)) { throw "找不到預期的安裝檔：$bundled" }
    New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
    $dest = Join-Path $OutDir $publish.File
    $sigPath = "$dest.sig"
    foreach ($stale in @($dest, $sigPath)) { if (Test-Path -LiteralPath $stale) { Remove-Item -LiteralPath $stale -Force } }
    Copy-Item -LiteralPath $bundled -Destination $dest

    # 7. 獨立步驟簽章（-SkipSign 時略過：由 CI 發佈 job 以 -SignOnly 簽）
    if (-not $SkipSign) {
        $null = Invoke-InstallerSigning $cli $dest $version $publish.File $confPath $SigningKeyPath $SigningKeyEnv $SigningKeyPasswordEnv ([bool]$Release) $true
    }
    else { Write-Host '-SkipSign：不簽章（簽章由發佈 job 以 -SignOnly 另行處理）' }

    # 8. 後門字串搜尋（-Release）
    if ($Release) {
        # e2e 測試公鑰存在時一併搜尋；實際用 e2e 設定建置時，公鑰會被嵌進執行檔
        $needles = @(Get-BackdoorNeedles $e2eConfPath $e2ePubs)
        $exeBeforeBundle = Join-Path $releaseDir 'fc-host.exe'
        foreach ($p in @($exeBeforeBundle, $dest)) {
            $hits = @(Find-ForbiddenStrings $p $needles)
            if ($hits.Count -gt 0) { throw "後門字串出現在 ${p}：$($hits -join ' ; ')" }
            Write-Host "後門字串搜尋：$(Split-Path $p -Leaf) 乾淨（$($needles.Count) 個字串）"
        }
    }

    # 9. 摘要
    $item = Get-Item -LiteralPath $dest
    $hash = (Get-FileHash -LiteralPath $dest -Algorithm SHA256).Hash.ToLowerInvariant()
    Write-Host ''
    Write-Host '=== 打包完成 ==='
    Write-Host "安裝檔 ：$($item.FullName)"
    Write-Host ("大小   ：{0:N0} bytes（{1:N2} MiB）" -f $item.Length, ($item.Length / 1MB))
    Write-Host "SHA256 ：$hash"
    if ($SkipSign) { Write-Host '簽章   ：（未簽章，-SkipSign）' } else { Write-Host "簽章   ：$sigPath" }
}

# 以 `. .\package.ps1` 載入函式（測試）時不執行主流程。
if ($MyInvocation.InvocationName -ne '.') { Invoke-Package }
