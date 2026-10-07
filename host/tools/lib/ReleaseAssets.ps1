# release 資產核對函式（installer-auto-update task 5.2 第 2 輪修正）。由 release.yml 的 publish job dot-source；
# 只含純函式、沒有參數與主流程。測試：host/tools/tests/release-workflow.Tests.ps1。
#
# GitHub REST「Release assets」物件有 `size`（整數）、`state`（uploaded／open）與 `digest`（字串或 null，格式 `sha256:<64 位十六進位>`）：
# https://docs.github.com/en/rest/releases/assets

# 核對 draft release 的資產：每個本機檔案都要有同名資產，且 state＝uploaded、size 相同；
# API 有 digest 時比對本機 SHA256（`sha256:<hex>`，不分大小寫）；digest 為 null 時只能比 size，記一則警告。
# $ApiAssets：API 回傳的資產物件陣列；$LocalPaths：本機檔案路徑。回傳 @{ Errors = <字串陣列>; Warnings = <字串陣列> }。
function Compare-ReleaseAssets($ApiAssets, [string[]]$LocalPaths) {
    $errs = [System.Collections.Generic.List[string]]::new()
    $warns = [System.Collections.Generic.List[string]]::new()
    $assets = @($ApiAssets)
    $names = @($assets | ForEach-Object { $_.name })
    foreach ($path in $LocalPaths) {
        $leaf = Split-Path $path -Leaf
        $matches2 = @($assets | Where-Object { $_.name -eq $leaf })
        if ($matches2.Count -eq 0) { $errs.Add("draft release 缺少資產：$leaf（現有：$($names -join ', ')）"); continue }
        if ($matches2.Count -gt 1) { $errs.Add("draft release 有 $($matches2.Count) 個同名資產：$leaf"); continue }
        $a = $matches2[0]
        $item = Get-Item -LiteralPath $path
        if ($a.state -ne 'uploaded') { $errs.Add("${leaf}：資產狀態是「$($a.state)」，不是 uploaded（上傳未完成）") }
        if ([int64]$a.size -ne $item.Length) { $errs.Add("${leaf}：資產大小 $($a.size) 與本機 $($item.Length) 不同") }
        $props = @($a.PSObject.Properties | ForEach-Object { $_.Name })
        $digest = if ($props -contains 'digest') { $a.digest } else { $null }
        if ([string]::IsNullOrEmpty($digest)) { $warns.Add("${leaf}：API 沒有提供 digest，只比對了大小") }
        else {
            $local = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
            if ($digest -ne "sha256:$local" -and $digest.ToLowerInvariant() -ne "sha256:$local") { $errs.Add("${leaf}：資產 digest $digest 與本機 SHA256 sha256:$local 不同") }
        }
    }
    @{ Errors = $errs.ToArray(); Warnings = $warns.ToArray() }
}
