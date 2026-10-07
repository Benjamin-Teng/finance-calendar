<#
.SYNOPSIS
    發佈後驗證（installer-auto-update task 5.2、design.md D6「發佈後驗證」）：從使用者的更新端點取 latest.json，
    下載兩個平台的安裝檔，以 tauri.conf.json 內建的公鑰用 minisign 驗簽，並比對版本與可信註解。

.DESCRIPTION
    release workflow 建的是 draft；收尾線人工檢查、發佈並設為 Latest 之後，由 `release: published` 觸發的
    release-verify.yml 呼叫本腳本。驗的是**使用者真正會走的路徑**：

      1. 更新端點＝tauri.conf.json 的 plugins.updater.endpoints[0]（必須是
         https://github.com/<owner>/<repo>/releases/latest/download/latest.json 形式）。
      2. 取 latest.json；有 -ExpectedTag（release 事件的 tag）時，等到 latest.json 的 version 等於該 tag 的版本
         （「發佈」到「設為 Latest」之間可能有數秒延遲；重試 -MaxAttempts 次、間隔 -RetrySeconds 秒）。
         一直等不到＝這個 release 沒有成為 Latest，或 Latest 的 release 不含 latest.json（design.md 風險 D8），失敗。
      3. latest.json 內容檢查（lib\LatestJson.ps1 的 Test-LatestJson，與組裝時同一份）：兩平台都在、網址是該 tag 的
         releases/download/ 固定路徑、簽章非空、可信註解 version 與 file 正確、兩個簽章同一把金鑰。
      4. 兩個安裝檔逐一下載，檢查非空且以 MZ 開頭，並用**預編譯的 minisign**（釘版本與 SHA256，快取在
         %LOCALAPPDATA%\fc-host-tools\minisign-<版本>）以公鑰驗簽：`minisign -V -m <安裝檔> -x <簽章> -p <公鑰>`。
         minisign 同時驗簽章本體與「可信註解的簽章」，所以可信註解（version／file）是經過驗證的。
    任一項失敗＝結束碼非 0，錯誤一次列出（workflow 據此開 issue）。

    為什麼用 minisign：tauri 的簽章是標準 minisign 格式；.NET 沒有內建 Ed25519，自己實作密碼學不值得。
    minisign 是單一靜態執行檔、runner 不用裝套件；釘死版本與雜湊（上游另有 minisign 簽章，釘選時已核對）。

.PARAMETER ExpectedTag
    release 事件的 tag（例如 v0.1.0）。省略＝不比對 tag，以 latest.json 自己宣告的版本為準（手動重跑用）。

.PARAMETER Repo
    預設由端點網址解析；可覆寫（測試用）。

.PARAMETER ConfPath
    tauri.conf.json 路徑，預設 host/tauri.conf.json。

.PARAMETER WorkDir
    下載與暫存資料夾，預設暫存資料夾下的新資料夾。

.PARAMETER MaxAttempts
    等 latest.json 版本到位的最多次數（預設 10）。

.PARAMETER RetrySeconds
    重試間隔秒數（預設 30）。

.EXAMPLE
    pwsh -NoProfile -File host/tools/verify-published.ps1 -ExpectedTag v0.1.0
#>
[CmdletBinding()]
param(
    [string]$ExpectedTag = '',
    [string]$Repo = '',
    [string]$ConfPath,
    [string]$WorkDir,
    [int]$MaxAttempts = 10,
    [int]$RetrySeconds = 30
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

. (Join-Path (Join-Path $PSScriptRoot 'lib') 'LatestJson.ps1')

# ================================================================================================
# 純函式（verify-published.Tests.ps1 以語法樹取出本檔函式後測試；函式不得依賴腳本層級變數）
# ================================================================================================

# 釘選的 minisign 預編譯檔。來源：https://github.com/jedisct1/minisign/releases/tag/0.12
# SHA256 由 task 5.2 對下載檔實算（37b600…），並以 minisign 自己的簽章核對過：
#   minisign -Vm minisign-0.12-win64.zip -P RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3
#   （上游 README 公布的公鑰；輸出 "Signature and comment signature verified"）。
function Get-MinisignConstants {
    @{
        Version = '0.12'
        Zip     = 'minisign-0.12-win64.zip'
        Url     = 'https://github.com/jedisct1/minisign/releases/download/0.12/minisign-0.12-win64.zip'
        Sha256  = '37b600344e20c19314b2e82813db2bfdcc408b77b876f7727889dbd46d539479'
    }
}

# 確保 minisign 在快取目錄，回傳 minisign.exe 路徑。zip 一律核對釘選的 SHA256，不符就刪除並失敗。
# zip 內含 x86_64 與 aarch64 兩個執行檔，依作業系統架構選。
function Initialize-Minisign {
    $c = Get-MinisignConstants
    $dir = Join-Path $env:LOCALAPPDATA "fc-host-tools\minisign-$($c.Version)"
    $zip = Join-Path $dir $c.Zip
    $sub = if ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString() -eq 'Arm64') { 'aarch64' } else { 'x86_64' }
    $exe = Join-Path $dir "minisign-win64\$sub\minisign.exe"
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    if ((Test-Path -LiteralPath $exe) -and (Test-Path -LiteralPath $zip) -and ((Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash -ieq $c.Sha256)) { return $exe }
    if (-not ((Test-Path -LiteralPath $zip) -and ((Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash -ieq $c.Sha256))) {
        Write-Host "下載 minisign $($c.Version)：$($c.Url)"
        Invoke-WebRequest -Uri $c.Url -OutFile $zip -UseBasicParsing
        $actual = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash
        if ($actual -ine $c.Sha256) {
            Remove-Item -LiteralPath $zip -Force
            throw "minisign zip 的 SHA256 不符：實際 $actual、釘選 $($c.Sha256)；已刪除下載檔，未使用"
        }
    }
    Expand-Archive -LiteralPath $zip -DestinationPath $dir -Force
    if (-not (Test-Path -LiteralPath $exe)) { throw "解壓後找不到 $exe" }
    $exe
}

# 從更新端點網址解析 owner/repo；不是 https://github.com/<o>/<r>/releases/latest/download/latest.json 形式就丟例外。
function Get-RepoFromEndpoint([string]$Endpoint) {
    if ($Endpoint -match '^https://github\.com/([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+)/releases/latest/download/latest\.json$') { return $Matches[1] }
    throw "更新端點不是 https://github.com/<owner>/<repo>/releases/latest/download/latest.json 形式：$Endpoint"
}

# 等 latest.json 的 version 等於期望版本。$Fetch：scriptblock，回傳 latest.json 文字（失敗丟例外）；
# $Sleep：scriptblock param($seconds)（測試注入）。$ExpectedVersion 空＝取到第一份可解析的就回傳。
# 回傳 latest.json 文字；用盡次數就丟例外，訊息帶最後一次看到的版本或錯誤。
function Wait-ForLatestJson([scriptblock]$Fetch, [string]$ExpectedVersion, [int]$MaxAttempts, [int]$RetrySeconds, [scriptblock]$Sleep) {
    $last = ''
    for ($i = 1; $i -le $MaxAttempts; $i++) {
        try {
            $text = & $Fetch
            $json = ConvertFrom-JsonKeepDates $text
            $seen = if ($json.PSObject.Properties.Name -contains 'version') { [string]$json.version } else { '' }
            if (-not $ExpectedVersion -or $seen -eq $ExpectedVersion) { return $text }
            $last = "latest.json 的 version 是「$seen」，不是期望的「$ExpectedVersion」"
        }
        catch { $last = "取得或解析 latest.json 失敗：$($_.Exception.Message)" }
        Write-Host "第 $i／$MaxAttempts 次：$last"
        if ($i -lt $MaxAttempts) { & $Sleep $RetrySeconds }
    }
    throw "等了 $MaxAttempts 次仍未取得期望的 latest.json：$last（這個 release 可能沒有成為 Latest，或 Latest 的 release 不含 latest.json）"
}

# 檔案是否以 MZ（PE）開頭。
function Test-PeHeader([string]$Path) {
    $fs = [System.IO.File]::OpenRead($Path)
    try {
        $buf = New-Object byte[] 2
        $n = $fs.Read($buf, 0, 2)
        return ($n -eq 2 -and $buf[0] -eq 0x4D -and $buf[1] -eq 0x5A)
    }
    finally { $fs.Dispose() }
}

# 核心驗證：回傳錯誤訊息陣列（空＝通過）。
#   $LatestText    latest.json 文字
#   $Repo          owner/repo（網址檢查用）
#   $ExpectedTag   release 事件的 tag；空＝以 latest.json 的 version 推 v<version>
#   $PubkeyBase64  tauri.conf.json 的 plugins.updater.pubkey（base64 的 minisign 公鑰檔）
#   $MinisignExe   minisign.exe 路徑
#   $WorkDir       下載與暫存資料夾
#   $Download      scriptblock param($url, $outFile)：把網址內容存成檔案（失敗丟例外）
function Test-PublishedRelease([string]$LatestText, [string]$Repo, [string]$ExpectedTag, [string]$PubkeyBase64, [string]$MinisignExe, [string]$WorkDir, [scriptblock]$Download) {
    $errs = [System.Collections.Generic.List[string]]::new()
    try { $json = ConvertFrom-JsonKeepDates $LatestText }
    catch { return @("latest.json 不是有效的 JSON：$($_.Exception.Message)") }
    if ($json.PSObject.Properties.Name -notcontains 'version' -or -not $json.version) { return @('latest.json 沒有 version 欄位') }
    $version = [string]$json.version
    $tag = if ($ExpectedTag) { $ExpectedTag } else { "v$version" }
    if ($tag -ne "v$version") { $errs.Add("latest.json 的 version（$version）與 release tag（$tag）不一致") }
    foreach ($e in (Test-LatestJson $json $version $tag $Repo)) { $errs.Add($e) }
    if (-not (Test-MinisignPubkey $PubkeyBase64)) {
        $errs.Add('tauri.conf.json 的 plugins.updater.pubkey 不是有效的 minisign 公鑰')
        return $errs.ToArray()
    }
    if ($errs.Count -gt 0) { return $errs.ToArray() }   # 內容有問題就不必下載

    New-Item -ItemType Directory -Force -Path $WorkDir | Out-Null
    $pubFile = Join-Path $WorkDir 'updater.pub'
    [System.IO.File]::WriteAllBytes($pubFile, [Convert]::FromBase64String($PubkeyBase64.Trim()))
    $table = Get-PlatformTable
    foreach ($key in $table.Keys) {
        $p = $json.platforms.$key
        $installer = Join-Path $WorkDir $table[$key]
        $sigFile = "$installer.minisig"
        try { & $Download ([string]$p.url) $installer }
        catch { $errs.Add("${key}：下載失敗（$($p.url)）：$($_.Exception.Message)"); continue }
        if (-not (Test-Path -LiteralPath $installer -PathType Leaf) -or (Get-Item -LiteralPath $installer).Length -eq 0) { $errs.Add("${key}：下載的安裝檔不存在或是空的"); continue }
        if (-not (Test-PeHeader $installer)) { $errs.Add("${key}：下載的檔案不是 Windows 執行檔（沒有 MZ 標頭）；可能下載到錯誤頁面") ; continue }
        $hash = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash.ToLowerInvariant()
        Write-Host ("{0}：{1}（{2:N0} bytes，SHA256 {3}）" -f $key, $table[$key], (Get-Item -LiteralPath $installer).Length, $hash)
        # .sig（latest.json 的 signature）＝ base64(minisign 簽章檔文字)；還原成 minisign 認得的簽章檔
        [System.IO.File]::WriteAllBytes($sigFile, [Convert]::FromBase64String(([string]$p.signature).Trim()))
        $out = & $MinisignExe -V -m $installer -x $sigFile -p $pubFile 2>&1 | Out-String
        $code = $LASTEXITCODE
        if ($code -ne 0 -or $out -notmatch 'Signature and comment signature verified') {
            $errs.Add("${key}：minisign 驗簽失敗（結束碼 $code）：$($out.Trim())")
        }
        else { Write-Host "${key}：minisign 驗簽通過 — $(($out -split "`r?`n" | Where-Object { $_ -like 'Trusted comment:*' }) -join '')" }
    }
    $errs.ToArray()
}

# ================================================================================================
# 主流程
# ================================================================================================
function Invoke-VerifyPublished {
    $hostDir = Split-Path $PSScriptRoot -Parent
    if (-not $ConfPath) { $ConfPath = Join-Path $hostDir 'tauri.conf.json' }
    $conf = Get-Content -LiteralPath $ConfPath -Raw -Encoding UTF8 | ConvertFrom-Json
    $upd = $conf.plugins.updater
    $endpoint = @($upd.endpoints)[0]
    $pubkey = [string]$upd.pubkey
    Write-Host "更新端點：$endpoint"
    $repoName = if ($Repo) { $Repo } else { Get-RepoFromEndpoint $endpoint }
    $expectedVersion = ''
    if ($ExpectedTag) {
        if ($ExpectedTag -notmatch '^v(\d+\.\d+\.\d+.*)$') { throw "-ExpectedTag 必須是 v<版本>：$ExpectedTag" }
        $expectedVersion = $Matches[1]
    }
    if (-not $WorkDir) { $WorkDir = Join-Path ([System.IO.Path]::GetTempPath()) ("fc-verify-" + [guid]::NewGuid().ToString('N').Substring(0, 8)) }
    New-Item -ItemType Directory -Force -Path $WorkDir | Out-Null

    $download = {
        param($url, $outFile)
        if (-not $url.StartsWith('https://')) { throw "只接受 https 網址：$url" }
        Invoke-WebRequest -Uri $url -OutFile $outFile -UseBasicParsing -MaximumRetryCount 3 -RetryIntervalSec 5
    }
    $fetch = {
        $tmp = Join-Path $WorkDir 'latest.json'
        if (Test-Path -LiteralPath $tmp) { Remove-Item -LiteralPath $tmp -Force }
        & $download $endpoint $tmp
        [System.IO.File]::ReadAllText($tmp, [System.Text.Encoding]::UTF8)
    }
    $minisign = Initialize-Minisign
    $msVersion = (& $minisign -v 2>&1 | Out-String).Trim()
    Write-Host "minisign：$minisign（$msVersion）"
    $text = Wait-ForLatestJson $fetch $expectedVersion $MaxAttempts $RetrySeconds { param($s) Start-Sleep -Seconds $s }
    $errs = @(Test-PublishedRelease $text $repoName $ExpectedTag $pubkey $minisign $WorkDir $download)
    if ($errs.Count -gt 0) { throw ("發佈後驗證失敗：`n  - " + ($errs -join "`n  - ")) }
    $v = (ConvertFrom-JsonKeepDates $text).version
    Write-Host ''
    Write-Host "=== 發佈後驗證通過：版本 $v，兩個平台的安裝檔皆以內建公鑰驗簽通過 ==="
}

# 以 `. .\verify-published.ps1` 載入函式（測試）時不執行主流程。
if ($MyInvocation.InvocationName -ne '.') { Invoke-VerifyPublished }
