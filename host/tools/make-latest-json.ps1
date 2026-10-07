<#
.SYNOPSIS
    組裝 Tauri updater 的 latest.json（installer-auto-update task 5.1、design.md D6）：讀兩個架構的 .sig，
    產出 `releases/download/<tag>/` 固定版本網址的清單，並在寫出前後各檢查一次。

.DESCRIPTION
    輸出格式（tauri-plugin-updater 的靜態 JSON；平台鍵＝「作業系統-架構」，與宿主的 cfg!(target_arch) 對應，D8）：

        {
          "version": "<版本>",
          "notes": "<-Notes>",
          "pub_date": "<RFC 3339 UTC>",
          "platforms": {
            "windows-x86_64":  { "signature": "<x64 .sig 內容>",   "url": "https://github.com/<repo>/releases/download/<tag>/finance-calendar-setup.exe" },
            "windows-aarch64": { "signature": "<arm64 .sig 內容>", "url": "https://github.com/<repo>/releases/download/<tag>/finance-calendar-setup-arm64.exe" }
          }
        }

    網址一律用 `releases/download/<tag>/`（固定版本），不用 `latest`：`latest.json` 自己是從 `releases/latest/download/`
    被取走的，但它指向的安裝檔必須與它同屬一個 release，否則版本與檔案會錯位。

    檢查（任一項不符就失敗、不留下輸出檔；所有錯誤一次列出）：
      1. -Version 是語意化版本，且 -Tag 恰為 `v<版本>`（版本＝tag 去掉 `v`）。
      2. 兩個平台都在（-X64Sig、-Arm64Sig 都要給）、.sig 檔存在且內容非空。
      3. 每個 .sig 能解碼，可信註解（trusted comment）的 `version` 欄位**完全等於**版本（requireSignedVersion 的前提），
         且 `file` 欄位等於該平台的發佈檔名（防兩個 .sig 放反）；兩個 .sig 的 key ID 相同（同一把私鑰簽的）。
      4. 輸出的 URL 都是 https://github.com/<repo>/releases/download/<tag>/<發佈檔名>，tag 與 -Tag 一致。
      5. 寫出後重新讀回並以同一組檢查再驗一次（`Test-LatestJson`）。

    不驗簽（不碰公鑰）：驗簽由發佈後驗證 job（verify-published.ps1）與 package.ps1 -Release 的 key ID 核對負責。

.PARAMETER Version
    版本（例如 0.1.0），須等於 -Tag 去掉開頭的 `v`。

.PARAMETER Tag
    發佈 tag（例如 v0.1.0）。

.PARAMETER X64Sig
    x64 安裝檔（finance-calendar-setup.exe）的 .sig 檔路徑。

.PARAMETER Arm64Sig
    ARM64 安裝檔（finance-calendar-setup-arm64.exe）的 .sig 檔路徑。

.PARAMETER Repo
    GitHub 倉庫（owner/name），預設 Benjamin-Teng/finance-calendar。

.PARAMETER Notes
    release notes 文字（放進 latest.json 的 notes 欄位）。

.PARAMETER Out
    輸出路徑，預設 latest.json（目前目錄）。

.PARAMETER PubDate
    發佈時間（RFC 3339 UTC）；省略＝現在。測試用。

.EXAMPLE
    pwsh -NoProfile -File host/tools/make-latest-json.ps1 -Version 0.1.0 -Tag v0.1.0 `
        -X64Sig dist/finance-calendar-setup.exe.sig -Arm64Sig dist/finance-calendar-setup-arm64.exe.sig -Out dist/latest.json
#>
[CmdletBinding()]
param(
    [string]$Version,
    [string]$Tag,
    [string]$X64Sig,
    [string]$Arm64Sig,
    [string]$Repo = 'Benjamin-Teng/finance-calendar',
    [string]$Notes = '',
    [string]$Out = 'latest.json',
    [string]$PubDate
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# ================================================================================================
# 共用檢查函式（latest.json 內容、簽章可信註解）在 lib\LatestJson.ps1：發佈後驗證（verify-published.ps1）也用同一份，
# 組裝與驗證不會各自維護一套規則。只含函式、沒有參數，dot-source 不會污染本腳本的變數。
# ================================================================================================
. (Join-Path (Join-Path $PSScriptRoot 'lib') 'LatestJson.ps1')

# ================================================================================================
# 純函式（make-latest-json.Tests.ps1 以語法樹取出本檔與 lib\LatestJson.ps1 的函式定義後測試；函式不得依賴腳本層級變數）
# ================================================================================================

# 檢查輸入參數（版本、tag、repo）。回傳錯誤訊息陣列。
function Test-Inputs([string]$Version, [string]$Tag, [string]$Repo) {
    $errs = [System.Collections.Generic.List[string]]::new()
    if ($Version -notmatch '^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$') { $errs.Add("-Version 不是語意化版本：「$Version」") }
    if ($Tag -ne "v$Version") { $errs.Add("-Tag（「$Tag」）必須恰為 v<版本>（「v$Version」）") }
    if ($Repo -notmatch '^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$') { $errs.Add("-Repo 必須是 owner/name：「$Repo」") }
    $errs.ToArray()
}

# 讀 .sig 檔文字；缺檔或空檔回傳 $null／空字串由 Test-SignatureText 報錯。
function Read-SigFile([string]$Path) {
    if (-not $Path) { return $null }
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { return $null }
    "$(Get-Content -LiteralPath $Path -Raw -Encoding ASCII)".Trim()
}

# 組出 latest.json 物件（不檢查；檢查由 Test-LatestJson 對這個物件與讀回的檔案做）。
function New-LatestJsonObject([string]$Version, [string]$Tag, [string]$Repo, [string]$Notes, [string]$PubDate, [string]$X64SigText, [string]$Arm64SigText) {
    $table = Get-PlatformTable
    $sigs = @{ 'windows-x86_64' = $X64SigText; 'windows-aarch64' = $Arm64SigText }
    $platforms = [ordered]@{}
    foreach ($key in $table.Keys) {
        $platforms[$key] = [ordered]@{
            signature = $sigs[$key]
            url       = "https://github.com/$Repo/releases/download/$Tag/$($table[$key])"
        }
    }
    [ordered]@{ version = $Version; notes = $Notes; pub_date = $PubDate; platforms = $platforms }
}

# ================================================================================================
# 主流程
# ================================================================================================
function Invoke-MakeLatestJson {
    $errs = [System.Collections.Generic.List[string]]::new()
    foreach ($e in (Test-Inputs $Version $Tag $Repo)) { $errs.Add($e) }

    # 缺平台：兩個 .sig 路徑都要給，且檔案存在
    $sigPaths = [ordered]@{ 'windows-x86_64' = $X64Sig; 'windows-aarch64' = $Arm64Sig }
    $texts = @{}
    foreach ($key in $sigPaths.Keys) {
        $path = $sigPaths[$key]
        if (-not $path) { $errs.Add("缺少平台 ${key}：沒有給 .sig 路徑（-X64Sig／-Arm64Sig 兩個都要給）"); continue }
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { $errs.Add("缺少平台 ${key}：.sig 檔不存在：$path"); continue }
        $texts[$key] = Read-SigFile $path
    }
    if ($errs.Count -gt 0) { throw ("latest.json 輸入檢查失敗：`n  - " + ($errs -join "`n  - ")) }

    $pub = if ($PubDate) { $PubDate } else { [DateTime]::UtcNow.ToString("yyyy-MM-dd'T'HH:mm:ss'Z'") }
    $obj = New-LatestJsonObject $Version $Tag $Repo $Notes $pub $texts['windows-x86_64'] $texts['windows-aarch64']
    $json = $obj | ConvertTo-Json -Depth 6

    # 先對組好的內容檢查（還沒寫出）；通過才寫，寫完再從檔案讀回檢查一次
    foreach ($e in (Test-LatestJson (ConvertFrom-JsonKeepDates $json) $Version $Tag $Repo)) { $errs.Add($e) }
    if ($errs.Count -gt 0) { throw ("latest.json 檢查失敗：`n  - " + ($errs -join "`n  - ")) }

    # 相對路徑以 PowerShell 的目前位置解析（.NET 的目前目錄可能不同）
    $outFull = [System.IO.Path]::GetFullPath($(if ([System.IO.Path]::IsPathRooted($Out)) { $Out } else { Join-Path $PWD.ProviderPath $Out }))
    $outDir = Split-Path -Parent $outFull
    if ($outDir -and -not (Test-Path -LiteralPath $outDir)) { New-Item -ItemType Directory -Force -Path $outDir | Out-Null }
    $tmp = "$outFull.tmp"
    [System.IO.File]::WriteAllText($tmp, $json + "`n", (New-Object System.Text.UTF8Encoding($false)))
    try {
        $back = ConvertFrom-JsonKeepDates (Get-Content -LiteralPath $tmp -Raw -Encoding UTF8)
        foreach ($e in (Test-LatestJson $back $Version $Tag $Repo)) { $errs.Add($e) }
        if ($errs.Count -gt 0) { throw ("latest.json 讀回檢查失敗：`n  - " + ($errs -join "`n  - ")) }
        Move-Item -LiteralPath $tmp -Destination $outFull -Force
    }
    finally { if (Test-Path -LiteralPath $tmp) { Remove-Item -LiteralPath $tmp -Force } }

    Write-Host "latest.json 已輸出：$outFull"
    Write-Host "  version  ：$Version（tag $Tag）"
    foreach ($key in $back.platforms.PSObject.Properties.Name) { Write-Host "  $key ：$($back.platforms.$key.url)" }
}

# 以 `. .\make-latest-json.ps1` 載入函式（測試）時不執行主流程。
if ($MyInvocation.InvocationName -ne '.') { Invoke-MakeLatestJson }
