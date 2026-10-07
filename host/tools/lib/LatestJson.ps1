# latest.json 與更新簽章（minisign .sig）的共用檢查函式（installer-auto-update task 5.1／5.2）。
# 由 make-latest-json.ps1（組裝）與 verify-published.ps1（發佈後驗證）dot-source；只含純函式、沒有參數與主流程。
# 測試：host/tools/tests/make-latest-json.Tests.ps1、verify-published.Tests.ps1（以語法樹取出本檔函式）。
#
# Get-MinisignKeyId／ConvertFrom-TrustedComment／Test-MinisignPubkey 與 package.ps1 的同名函式邏輯相同
# （package.ps1 是 task 2 審查過的原版；不 dot-source 它，因為它的參數會被帶進呼叫端的變數範圍）。
# 兩邊由 make-latest-json.Tests.ps1 以同一組輸入互相對照，確保不分岔。

# 平台鍵 → 發佈檔名。鍵名與宿主以 cfg!(target_arch) 組出的平台鍵一致（design.md D8）。
function Get-PlatformTable {
    [ordered]@{
        'windows-x86_64'  = 'finance-calendar-setup.exe'
        'windows-aarch64' = 'finance-calendar-setup-arm64.exe'
    }
}

# minisign 簽章／公鑰的 key ID（檔內位元組序，十六進位大寫）；兩者都是「base64(文字檔)」，文字檔第二行再 base64，
# 位元組 [2..9] 是 key ID。
function Get-MinisignKeyId([string]$Base64Text) {
    try { $text = [System.Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($Base64Text.Trim())) }
    catch { throw "不是有效的 base64（minisign 簽章）：$($_.Exception.Message)" }
    $lines = @($text -split "\r?\n")
    if ($lines.Count -lt 2) { throw 'minisign 內容少於兩行' }
    try { $bytes = [Convert]::FromBase64String($lines[1].Trim()) }
    catch { throw "minisign 第二行不是有效的 base64：$($_.Exception.Message)" }
    if ($bytes.Length -lt 10) { throw "minisign 第二行解碼後只有 $($bytes.Length) 位元組（至少 10）" }
    ($bytes[2..9] | ForEach-Object { $_.ToString('X2') }) -join ''
}

# 解碼 .sig 文字（單行 base64），回傳可信註解（trusted comment: 之後的整行）。
function Get-SigTrustedCommentFromText([string]$SigText) {
    $raw = $SigText.Trim()
    try { $text = [System.Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($raw)) }
    catch { throw "簽章不是有效的 base64：$($_.Exception.Message)" }
    foreach ($line in ($text -split "\r?\n")) {
        if ($line.StartsWith('trusted comment: ')) { return $line.Substring('trusted comment: '.Length) }
    }
    throw "簽章解碼後找不到 'trusted comment:' 行（不是 minisign 簽章？）"
}

# 可信註解是一行以 tab 分隔的欄位（timestamp:<秒>\tfile:<檔名>[\tversion:<版本>]）→ 欄位名稱→值的雜湊表。
function ConvertFrom-TrustedComment([string]$Comment) {
    $fields = @{}
    foreach ($part in ($Comment -split "`t")) {
        $i = $part.IndexOf(':')
        if ($i -gt 0) { $fields[$part.Substring(0, $i)] = $part.Substring($i + 1) }
    }
    $fields
}

# 檢查一份簽章文字：非空、可解碼、可信註解的 version 完全等於 $Version、file 等於 $FileName。
# 回傳 @{ Errors = <字串陣列>; KeyId = <字串或 $null> }。$Label 是錯誤訊息前綴（平台鍵）。
function Test-SignatureText([string]$Label, [string]$SigText, [string]$Version, [string]$FileName) {
    $errs = [System.Collections.Generic.List[string]]::new()
    $keyId = $null
    if ([string]::IsNullOrWhiteSpace($SigText)) {
        $errs.Add("${Label}：簽章是空的")
        return @{ Errors = $errs.ToArray(); KeyId = $null }
    }
    try {
        $comment = Get-SigTrustedCommentFromText $SigText
        $f = ConvertFrom-TrustedComment $comment
        if (-not $f.ContainsKey('version')) { $errs.Add("${Label}：簽章的可信註解沒有 version 欄位（requireSignedVersion 會讓所有使用者的更新驗證失敗）：$comment") }
        elseif ($f['version'] -ne $Version) { $errs.Add("${Label}：簽章的可信註解 version 是「$($f['version'])」，應為「$Version」") }
        if (-not $f.ContainsKey('file')) { $errs.Add("${Label}：簽章的可信註解沒有 file 欄位：$comment") }
        elseif ($f['file'] -ne $FileName) { $errs.Add("${Label}：簽章的可信註解 file 是「$($f['file'])」，應為「$FileName」（兩個平台的 .sig 放反了？）") }
        $keyId = Get-MinisignKeyId $SigText
    }
    catch { $errs.Add("${Label}：$($_.Exception.Message)") }
    @{ Errors = $errs.ToArray(); KeyId = $keyId }
}

# JSON 文字 → 物件。PowerShell 7 的 ConvertFrom-Json 預設把 ISO 8601 字串轉成 DateTime，pub_date 就檢查不到原字串；
# 支援 -DateKind（7.5+）時用 String 保留原樣。
function ConvertFrom-JsonKeepDates([string]$Text) {
    if ((Get-Command ConvertFrom-Json).Parameters.ContainsKey('DateKind')) { $Text | ConvertFrom-Json -DateKind String }
    else { $Text | ConvertFrom-Json }
}

# 檢查 latest.json 的內容（已解析的物件）。回傳錯誤訊息陣列（空＝通過）。
# 組裝前（檢查輸入）與組裝後（讀回檔案）共用，所以 URL 與簽章都從物件本身檢查，不信任「組出來時的變數」。
function Test-LatestJson($Json, [string]$Version, [string]$Tag, [string]$Repo) {
    $errs = [System.Collections.Generic.List[string]]::new()
    $names = @($Json.PSObject.Properties | ForEach-Object { $_.Name })
    foreach ($k in 'version', 'notes', 'pub_date', 'platforms') {
        if ($names -notcontains $k) { $errs.Add("latest.json 缺少欄位 $k") }
    }
    if ($errs.Count -gt 0) { return $errs.ToArray() }
    if ($Json.version -ne $Version) { $errs.Add("latest.json 的 version 是「$($Json.version)」，應為「$Version」") }
    if ($Json.pub_date -isnot [string] -or $Json.pub_date -notmatch '^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?Z$') { $errs.Add("latest.json 的 pub_date 不是 RFC 3339 UTC：$($Json.pub_date)") }
    $platformNames = @($Json.platforms.PSObject.Properties | ForEach-Object { $_.Name })
    $table = Get-PlatformTable
    $keyIds = @{}
    foreach ($key in $table.Keys) {
        $file = $table[$key]
        if ($platformNames -notcontains $key) { $errs.Add("latest.json 缺少平台 $key"); continue }
        $p = $Json.platforms.$key
        $pn = @($p.PSObject.Properties | ForEach-Object { $_.Name })
        if ($pn -notcontains 'signature' -or $pn -notcontains 'url') { $errs.Add("${key}：缺少 signature 或 url 欄位"); continue }
        $r = Test-SignatureText $key ([string]$p.signature) $Version $file
        foreach ($e in $r.Errors) { $errs.Add($e) }
        if ($r.KeyId) { $keyIds[$key] = $r.KeyId }
        $expected = "https://github.com/$Repo/releases/download/$Tag/$file"
        if ([string]$p.url -ne $expected) { $errs.Add("${key}：url 是「$($p.url)」，應為「$expected」（網址必須指向同一個 tag 的固定版本路徑）") }
    }
    foreach ($extra in ($platformNames | Where-Object { $table.Keys -notcontains $_ })) { $errs.Add("latest.json 含未預期的平台 $extra") }
    if (@($keyIds.Values | Select-Object -Unique).Count -gt 1) { $errs.Add("兩個平台的簽章 key ID 不同（$((($keyIds.GetEnumerator() | ForEach-Object { "$($_.Key)=$($_.Value)" }) -join '、'))）：必須由同一把私鑰簽") }
    $errs.ToArray()
}

# 公鑰字串是否是真正的 minisign 公鑰：base64 解碼後是「untrusted comment: …」＋換行＋56 字元 base64
# （2 位元組演算法＋8 位元組 key ID＋32 位元組公鑰＝42 位元組）。
function Test-MinisignPubkey([string]$Pubkey) {
    if ([string]::IsNullOrWhiteSpace($Pubkey)) { return $false }
    try { $text = [System.Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($Pubkey.Trim())) }
    catch { return $false }
    return [bool]($text -match '^untrusted comment: [^\r\n]*\r?\n[A-Za-z0-9+/]{56}\s*$')
}
