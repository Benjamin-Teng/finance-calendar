<#
.SYNOPSIS
    host/tools/lib/EvidenceLog.psm1 的測試（純 PowerShell 斷言，不需 Pester）。

.EXAMPLE
    pwsh -NoProfile -File host/tools/tests/EvidenceLog.Tests.ps1
    結束碼 0＝全部通過、1＝有失敗。
#>
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$toolsDir = Split-Path $PSScriptRoot -Parent
Import-Module (Join-Path $toolsDir 'lib\EvidenceLog.psm1') -Force

$script:Pass = 0
$script:Fail = 0
function Check([string]$name, [bool]$cond, [string]$detail = '') {
    if ($cond) { $script:Pass++; Write-Host "PASS  $name" }
    else { $script:Fail++; Write-Host "FAIL  $name $detail" -ForegroundColor Red }
}

$temp = [IO.Path]::GetTempPath().TrimEnd('\')
$profileDir = $env:USERPROFILE.TrimEnd('\')
$user = Split-Path $profileDir -Leaf

$a = ConvertTo-EvidenceText "APPDATA=$temp\fc-host-7.7-abc\Roaming"
Check '單一反斜線的 TEMP 路徑 → %TEMP%' ($a -eq 'APPDATA=%TEMP%\fc-host-7.7-abc\Roaming') $a

$json = ($temp -replace '\\', '\\\\') + '\\\\fc-host'
$b = ConvertTo-EvidenceText "`"data_dir`":`"$json`""
Check 'JSON 跳脫（\\\\）的 TEMP 路徑 → %TEMP%' ($b -eq '"data_dir":"%TEMP%\\\\fc-host"') $b

$c = ConvertTo-EvidenceText ($temp.ToUpperInvariant() + '\x')
Check '大小寫不同也認得' ($c -eq '%TEMP%\x') $c

$d = ConvertTo-EvidenceText ($temp -replace '\\', '/')
Check '斜線分隔也認得' ($d -eq '%TEMP%') $d

$e = ConvertTo-EvidenceText "$env:APPDATA\tw.fintools.fc-host\settings.json"
Check 'APPDATA → %APPDATA%' ($e -eq '%APPDATA%\tw.fintools.fc-host\settings.json') $e

$f = ConvertTo-EvidenceText "$profileDir\Desktop"
Check 'USERPROFILE → %USERPROFILE%' ($f -eq '%USERPROFILE%\Desktop') $f

$g = ConvertTo-EvidenceText "$($profileDir)x\y"
Check '名稱只是前綴相同時不改' ($g -eq "$($profileDir)x\y") $g

Check '改寫後不再含使用者名稱' (-not ("$a$b$c$d$e$f" -match [regex]::Escape($user)))

# 宿主記錄檔會寫出裸使用者名稱（例：wallpaper_cli「使用者 <名稱>、設定檔 …」），不在路徑裡也要改寫。
$u1 = ConvertTo-EvidenceText "沒有執行中的宿主；使用者 $env:USERNAME、設定檔 $env:APPDATA\x"
Check '裸使用者名稱 → %USERNAME%' ($u1 -eq '沒有執行中的宿主；使用者 %USERNAME%、設定檔 %APPDATA%\x') $u1
$u2 = ConvertTo-EvidenceText "（使用者 $($env:USERNAME.ToUpperInvariant())）"
Check '裸使用者名稱大小寫不同也認得' ($u2 -eq '（使用者 %USERNAME%）') $u2
$u3 = ConvertTo-EvidenceText "x$($env:USERNAME) $($env:USERNAME)y"
Check '使用者名稱只是較長名稱的一部分時不改' ($u3 -eq "x$($env:USERNAME) $($env:USERNAME)y") $u3
$u4 = ConvertTo-EvidenceText "host=$env:COMPUTERNAME;"
Check '裸電腦名稱 → %COMPUTERNAME%' ($u4 -eq 'host=%COMPUTERNAME%;') $u4
# 審查 low：名稱後接句點（句尾）、連字號（主機名稱延伸）也要改寫。
$u5 = ConvertTo-EvidenceText "使用者 $env:USERNAME."
Check '裸使用者名稱在句尾（後接句點）→ %USERNAME%.' ($u5 -eq '使用者 %USERNAME%.') $u5
$u6 = ConvertTo-EvidenceText "$env:COMPUTERNAME-PC $env:COMPUTERNAME.lan"
Check '電腦名稱後接 -PC／.lan → 仍改寫' ($u6 -eq '%COMPUTERNAME%-PC %COMPUTERNAME%.lan') $u6

# 審查 low：8.3 短路徑（cmd 的 %~sI）也要改寫。短名停用（短＝長）時這幾項照樣成立。
function Get-CmdShortPath([string]$p) { (& cmd /c "for %I in (`"$p`") do @echo %~sI" | Select-Object -First 1).Trim() }
foreach ($pair in @(@($env:USERPROFILE, '%USERPROFILE%'), @($env:LOCALAPPDATA, '%LOCALAPPDATA%'), @($env:APPDATA, '%APPDATA%'))) {
    $short = Get-CmdShortPath $pair[0]
    $s1 = ConvertTo-EvidenceText "$short\zz"
    Check "8.3 短路徑 $($pair[1]) → $($pair[1])\zz（短路徑 $(if ($short -ne $pair[0]) { '有' } else { '無' })）" ($s1 -eq "$($pair[1])\zz") $s1
}
# 本機設定檔路徑的各段本來就合 8.3（短＝長）時上面驗不到機制：另起子行程，把 LOCALAPPDATA 指向一個
# 長檔名目錄（TEMP／TMP 指到不存在處，免得 TEMP 規則先吃掉），只用短路徑寫，必須改寫成 %LOCALAPPDATA%。
$longDir = Join-Path ([IO.Path]::GetTempPath()) ('evidencelog-longdirname-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Path $longDir | Out-Null
try {
    $shortDir = Get-CmdShortPath $longDir
    if ($shortDir -eq $longDir) {
        Write-Host "SKIP  8.3 短路徑機制：此磁碟未產生短檔名（$longDir）"
    } else {
        $mod = Join-Path $toolsDir 'lib\EvidenceLog.psm1'
        $child = "`$env:TMP = 'Z:\no-such-tmp-evlog'; `$env:TEMP = `$env:TMP; `$env:LOCALAPPDATA = '$longDir'; Import-Module '$mod'; ConvertTo-EvidenceText '$shortDir\zz'"
        $s2 = (& pwsh -NoProfile -Command $child | Select-Object -Last 1)
        Check "8.3 短路徑機制：LOCALAPPDATA 的短路徑 $(Split-Path $shortDir -Leaf) → %LOCALAPPDATA%\zz" ($s2 -eq '%LOCALAPPDATA%\zz') $s2
    }
} finally {
    Remove-Item -LiteralPath $longDir -ErrorAction SilentlyContinue
}

$tmp = Join-Path ([IO.Path]::GetTempPath()) ('evidencelog-test-' + [guid]::NewGuid().ToString('N') + '.log')
try {
    $w = New-EvidenceWriter $tmp
    $w.WriteLine("dir=$temp\z")
    $w.Close()
    $raw = [IO.File]::ReadAllBytes($tmp)
    $txt = [Text.Encoding]::UTF8.GetString($raw)
    Check 'New-EvidenceWriter 寫出時改寫、無 BOM' ($txt -eq "dir=%TEMP%\z`r`n" -and $raw[0] -ne 0xEF) $txt

    $bom = [byte[]](0xEF, 0xBB, 0xBF) + [Text.Encoding]::UTF8.GetBytes("a=$temp\q`nb=1`n")
    [IO.File]::WriteAllBytes($tmp, $bom)
    $n = Protect-EvidenceFile $tmp
    $raw2 = [IO.File]::ReadAllBytes($tmp)
    $txt2 = [Text.Encoding]::UTF8.GetString($raw2, 3, $raw2.Length - 3)
    Check 'Protect-EvidenceFile 保留 BOM 與 LF 行尾' ($n -eq 1 -and $raw2[0] -eq 0xEF -and $txt2 -eq "a=%TEMP%\q`nb=1`n") $txt2
    Check 'Protect-EvidenceFile 沒有可改的內容時不動檔案' ((Protect-EvidenceFile $tmp) -eq 0)
} finally {
    Remove-Item $tmp -ErrorAction SilentlyContinue
}

# fix F7（批次 A）：宿主記錄檔直接 Copy-Item 進證據目錄會留下使用者路徑；改經 Copy-EvidenceFile。
$cmdCopy = Get-Command Copy-EvidenceFile -ErrorAction SilentlyContinue
Check '匯出 Copy-EvidenceFile' ($null -ne $cmdCopy)
if ($cmdCopy) {
    $src = Join-Path ([IO.Path]::GetTempPath()) ('evlog-src-' + [guid]::NewGuid().ToString('N') + '.log')
    $dst = Join-Path ([IO.Path]::GetTempPath()) ('evlog-dst-' + [guid]::NewGuid().ToString('N') + '.log')
    try {
        [IO.File]::WriteAllText($src, "WARN 解析失敗 path=$temp\x\tw_events.json`n")
        Copy-EvidenceFile -Source $src -Destination $dst
        $d = [IO.File]::ReadAllText($dst)
        Check 'Copy-EvidenceFile：目的檔已去識別' ($d -eq "WARN 解析失敗 path=%TEMP%\x\tw_events.json`n") $d
        Check 'Copy-EvidenceFile：來源檔不變' ([IO.File]::ReadAllText($src) -match [regex]::Escape($temp))
        Copy-EvidenceFile -Source (Join-Path ([IO.Path]::GetTempPath()) 'evlog-no-such-file.log') -Destination $dst
        Check 'Copy-EvidenceFile：來源不存在時不丟例外、不動目的檔' ([IO.File]::ReadAllText($dst) -eq $d)
    } finally {
        Remove-Item $src, $dst -ErrorAction SilentlyContinue
    }
}

# 結構：host/tools 頂層 .ps1 不得以 Copy-Item 把檔案直接複製進證據目錄（$OutDir）。
$toolsDir = Split-Path $PSScriptRoot -Parent
foreach ($f in Get-ChildItem -Path $toolsDir -Filter '*.ps1' -File) {
    $ast = [System.Management.Automation.Language.Parser]::ParseFile($f.FullName, [ref]$null, [ref]$null)
    $raw = @($ast.FindAll({
                param($n)
                $n -is [System.Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq 'Copy-Item' -and
                $n.Extent.Text -match '\$OutDir'
            }, $true))
    Check "$($f.Name)：沒有直接 Copy-Item 進證據目錄（改用 Copy-EvidenceFile）" ($raw.Count -eq 0) (($raw | ForEach-Object { "第 $($_.Extent.StartLineNumber) 行" }) -join ', ')
}

# 7.7-low (b)：寫入證據目錄（$OutDir 及由它衍生的路徑變數）的 Set-Content／Add-Content／Out-File
# 必須經過 ConvertTo-EvidenceText（同一管線的前段，或 -Value 引數內）；重新導向（> ／ >>）到證據
# 路徑一律不允許。`-Value @()`（只建立空檔）與 -AsByteStream 不算。New-EvidenceWriter／
# Copy-EvidenceFile／Protect-EvidenceFile 是另外的合法路徑，不在此掃描範圍。
function Get-EvidenceWriteViolations([string]$Path) {
    $ast = [System.Management.Automation.Language.Parser]::ParseFile($Path, [ref]$null, [ref]$null)
    # 證據路徑變數：由 $OutDir 一路衍生（遞移閉包）。
    $vars = @{ 'OutDir' = $true }
    $assigns = @($ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.AssignmentStatementAst] -and $n.Left -is [System.Management.Automation.Language.VariableExpressionAst] }, $true))
    do {
        $grew = $false
        foreach ($a in $assigns) {
            $name = $a.Left.VariablePath.UserPath
            if ($vars.ContainsKey($name)) { continue }
            $refs = @($a.Right.FindAll({ param($n) $n -is [System.Management.Automation.Language.VariableExpressionAst] }, $true) | ForEach-Object { $_.VariablePath.UserPath })
            if (@($refs | Where-Object { $vars.ContainsKey($_) }).Count -gt 0) { $vars[$name] = $true; $grew = $true }
        }
    } while ($grew)
    $touches = { param($node) @($node.FindAll({ param($n) $n -is [System.Management.Automation.Language.VariableExpressionAst] -and $vars.ContainsKey($n.VariablePath.UserPath) }, $true)).Count -gt 0 }
    $bad = @()
    $cmds = @($ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.CommandAst] -and $n.GetCommandName() -in 'Set-Content', 'Add-Content', 'Out-File' }, $true))
    foreach ($c in $cmds) {
        if ($c.Extent.Text -match '-AsByteStream|-Encoding\s+Byte') { continue }
        $target = $null; $value = $null; $positional = @()
        $els = $c.CommandElements
        for ($i = 1; $i -lt $els.Count; $i++) {
            $e = $els[$i]
            if ($e -is [System.Management.Automation.Language.CommandParameterAst]) {
                $arg = if ($e.Argument) { $e.Argument } elseif ($i + 1 -lt $els.Count) { $i++; $els[$i] } else { $null }
                if ($e.ParameterName -match '^(Path|LiteralPath|FilePath|PSPath|LP)$') { $target = $arg }
                elseif ($e.ParameterName -match '^(Value|InputObject)$') { $value = $arg }
                elseif ($e.ParameterName -match '^(Encoding|Width)$') { }
                # 其他參數是 switch
            } else { $positional += $e }
        }
        if (-not $target -and $positional.Count -ge 1) { $target = $positional[0] }
        if (-not $value -and $positional.Count -ge 2) { $value = $positional[1] }
        if (-not $target -or -not (& $touches $target)) { continue }
        # `-Value @()`：只建立／清空檔案，沒有內容可去識別。
        if ($value -is [System.Management.Automation.Language.ArrayExpressionAst] -and $value.SubExpression.Statements.Count -eq 0) { continue }
        $ok = $false
        $pipe = $c.Parent
        if ($pipe -is [System.Management.Automation.Language.PipelineAst]) {
            $idx = [array]::IndexOf(@($pipe.PipelineElements), $c)
            for ($j = 0; $j -lt $idx; $j++) {
                $p = $pipe.PipelineElements[$j]
                if ($p -is [System.Management.Automation.Language.CommandAst] -and $p.GetCommandName() -eq 'ConvertTo-EvidenceText') { $ok = $true }
            }
        }
        if (-not $ok -and $value) {
            if (@($value.FindAll({ param($n) $n -is [System.Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq 'ConvertTo-EvidenceText' }, $true)).Count -gt 0) { $ok = $true }
        }
        if (-not $ok) { $bad += "第 $($c.Extent.StartLineNumber) 行 $($c.GetCommandName())" }
    }
    # 重新導向到證據路徑（> / >>）一律不允許：無法在重新導向上去識別。
    foreach ($r in @($ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.FileRedirectionAst] }, $true))) {
        if (& $touches $r.Location) { $bad += "第 $($r.Extent.StartLineNumber) 行 重新導向" }
    }
    return $bad
}

$fixtureDir = Join-Path ([IO.Path]::GetTempPath()) ('evlog-scan-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $fixtureDir | Out-Null
try {
    $fx = @{
        'bad-pipe.ps1'     = @('$p = Join-Path $OutDir ''a.log''', '$lines | Set-Content -Path $p -Encoding utf8')
        'bad-value.ps1'    = @('$q = Join-Path $OutDir ''a.log''', '$r = $q', 'Set-Content -Path $r -Value $lines -Encoding utf8')
        'bad-outfile.ps1'  = @('$lines | Out-File (Join-Path $OutDir ''a.log'')')
        'bad-add.ps1'      = @('Add-Content -Encoding utf8 (Join-Path $OutDir ''a.log'') ''x''')
        'bad-redirect.ps1' = @('$p = Join-Path $OutDir ''a.log''', 'Get-Date > $p')
        'ok-pipe.ps1'      = @('$p = Join-Path $OutDir ''a.log''', '$lines | ConvertTo-EvidenceText | Set-Content -Path $p -Encoding utf8')
        'ok-value.ps1'     = @('$p = Join-Path $OutDir ''a.log''', 'Set-Content -Path $p -Value ($lines | ConvertTo-EvidenceText) -Encoding utf8')
        'ok-empty.ps1'     = @('$p = Join-Path $OutDir ''a.log''', 'Set-Content -LiteralPath $p -Value @() -Encoding utf8')
        'ok-other.ps1'     = @('$d = Join-Path $tempRoot ''data''', '$fixture | Set-Content -Path (Join-Path $d ''tw_events.json'')')
    }
    foreach ($k in $fx.Keys) {
        $fp = Join-Path $fixtureDir $k
        Set-Content -LiteralPath $fp -Value $fx[$k] -Encoding utf8
        $v = @(Get-EvidenceWriteViolations $fp)
        if ($k -like 'bad-*') { Check "證據寫入掃描：$k 被抓到" ($v.Count -eq 1) ($v -join '; ') }
        else { Check "證據寫入掃描：$k 不誤報" ($v.Count -eq 0) ($v -join '; ') }
    }
} finally {
    Remove-Item -Recurse -Force $fixtureDir -ErrorAction SilentlyContinue
}

foreach ($f in Get-ChildItem -Path $toolsDir -Filter '*.ps1' -File) {
    $v = @(Get-EvidenceWriteViolations $f.FullName)
    Check "$($f.Name)：寫入證據目錄的 Set-Content／Add-Content／Out-File 都經 ConvertTo-EvidenceText" ($v.Count -eq 0) ($v -join ', ')
    $src = [IO.File]::ReadAllText($f.FullName)
    if ($src -match 'ConvertTo-EvidenceText') {
        Check "$($f.Name)：用到 ConvertTo-EvidenceText 就有匯入 lib\EvidenceLog.psm1" ($src.Contains("Import-Module (Join-Path `$PSScriptRoot 'lib\EvidenceLog.psm1')"))
    }
}

Write-Host "EvidenceLog.Tests：$script:Pass 通過、$script:Fail 失敗"
if ($script:Fail -gt 0) { exit 1 }
exit 0
