<#
.SYNOPSIS
    .github/workflows/release.yml 與 release-verify.yml 的靜態結構測試，外加 version job 腳本的本機演練
    （純 PowerShell 斷言，不需 Pester；語法與型別檢查由 actionlint 負責，這裡守住 actionlint 看不到的專案規則）。

.DESCRIPTION
    **不觸發 workflow、不連網、不碰任何 secret**。以逐行解析 YAML（兩份檔都是固定的縮排風格；不引入 YAML 套件）：
      - 第三方 action 一律釘 40 位十六進位 commit SHA
      - 沒有任何 run 區塊內嵌 `${{ }}`（防腳本注入：外部值一律經 env 傳入）
      - secrets 只出現在 publish job；Environment release 只在 publish；build／smoke 不碰簽章金鑰
      - 權限最小化（預設只讀；publish contents: write；verify issues: write；沒有 write-all）
      - 每個 job 有 timeout-minutes；沒有 tauri-action
      - 觸發條件（v* tag、workflow_dispatch 的 waive-arm64-smoke 布林預設 false）
      - 呼叫的腳本與參數存在於 package.ps1／make-latest-json.ps1／ci-smoke.ps1／verify-published.ps1 的 param 區塊
      - version job 的腳本本機演練：tag 與 Cargo 版本一致→通過並寫 outputs；不一致、非 tag ref、格式錯誤→失敗

.EXAMPLE
    pwsh -NoProfile -File host/tools/tests/release-workflow.Tests.ps1
    結束碼 0＝全部通過、1＝有失敗。
#>
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$toolsDir = Split-Path $PSScriptRoot -Parent
$hostDir = Split-Path $toolsDir -Parent
$repoRoot = Split-Path $hostDir -Parent
$wfDir = Join-Path $repoRoot '.github\workflows'
$pwsh = (Get-Process -Id $PID).Path

$script:Pass = 0
$script:Fail = 0
function Check([string]$name, [bool]$cond, [string]$detail = '') {
    if ($cond) { $script:Pass++; Write-Host "PASS  $name" }
    else { $script:Fail++; Write-Host "FAIL  $name $detail" -ForegroundColor Red }
}

function Read-Lines([string]$path) { @(([System.IO.File]::ReadAllText($path) -replace "`r`n", "`n") -split "`n") }

# 切出 jobs 底下每個 job 的行（以 2 個空白縮排的 `name:` 為界）→ 雜湊表 name → 行陣列
function Split-Jobs([string[]]$Lines) {
    $jobs = [ordered]@{}
    $inJobs = $false; $cur = $null
    foreach ($l in $Lines) {
        if ($l -match '^jobs:\s*$') { $inJobs = $true; continue }
        if (-not $inJobs) { continue }
        if ($l -match '^  ([A-Za-z0-9_-]+):\s*$') { $cur = $Matches[1]; $jobs[$cur] = [System.Collections.Generic.List[string]]::new(); continue }
        if ($cur) { $jobs[$cur].Add($l) }
    }
    $jobs
}

# 取出所有 `run:` 區塊的內容（`run: |` 多行與單行 `run: <cmd>`）；回傳 @{ Start = 行號; Text = 內容 } 陣列
function Get-RunBlocks([string[]]$Lines) {
    $blocks = @()
    for ($i = 0; $i -lt $Lines.Count; $i++) {
        $l = $Lines[$i]
        if ($l -match '^(\s*)(?:- )?run:\s*\|\s*$') {
            $indent = $Matches[1].Length + 2
            $body = [System.Collections.Generic.List[string]]::new()
            for ($j = $i + 1; $j -lt $Lines.Count; $j++) {
                $x = $Lines[$j]
                if ($x.Trim() -eq '') { $body.Add(''); continue }
                $lead = $x.Length - $x.TrimStart().Length
                if ($lead -lt $indent) { break }
                $body.Add($x.Substring($indent))
            }
            $blocks += , @{ Start = $i + 1; Text = ($body -join "`n") }
        }
        elseif ($l -match '^\s*(?:- )?run:\s*(\S.*)$') { $blocks += , @{ Start = $i + 1; Text = $Matches[1] } }
    }
    $blocks
}

$relPath = Join-Path $wfDir 'release.yml'
$verPath = Join-Path $wfDir 'release-verify.yml'
Check 'release.yml 與 release-verify.yml 存在' ((Test-Path $relPath) -and (Test-Path $verPath))
$rel = Read-Lines $relPath
$ver = Read-Lines $verPath
$relJobs = Split-Jobs $rel
$verJobs = Split-Jobs $ver

# ---- job 結構
Check 'release.yml 的 job：version、build、smoke-x64、smoke-arm64、publish' ((@($relJobs.Keys) -join ',') -eq 'version,build,smoke-x64,smoke-arm64,publish') ((@($relJobs.Keys) -join ','))
Check 'release-verify.yml 的 job：verify' ((@($verJobs.Keys) -join ',') -eq 'verify')
foreach ($pair in @(@('release.yml', $relJobs), @('release-verify.yml', $verJobs))) {
    foreach ($name in $pair[1].Keys) {
        Check "$($pair[0])／$name：有 timeout-minutes" (($pair[1][$name] -join "`n") -match '(?m)^    timeout-minutes:\s*\d+')
    }
}
Check 'needs：build 需要 version；smoke 需要 build；publish 需要 version、build、兩個 smoke' (
    (($relJobs['build'] -join "`n") -match 'needs: version') -and (($relJobs['smoke-x64'] -join "`n") -match 'needs: \[version, build\]') -and
    (($relJobs['smoke-arm64'] -join "`n") -match 'needs: \[version, build\]') -and (($relJobs['publish'] -join "`n") -match 'needs: \[version, build, smoke-x64, smoke-arm64\]'))

# ---- action 釘 SHA、不用 tauri-action
$allText = ($rel + $ver) -join "`n"
$uses = @([regex]::Matches($allText, '(?m)^\s*(?:- )?uses:\s*(\S+)') | ForEach-Object { $_.Groups[1].Value })
Check '第三方 action 至少有 checkout／upload／download' ($uses.Count -ge 6) ($uses -join ',')
$unpinned = @($uses | Where-Object { $_ -notmatch '^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+@[0-9a-f]{40}$' })
Check '第三方 action 全部釘到 40 位十六進位 commit SHA' ($unpinned.Count -eq 0) ($unpinned -join ',')
Check '沒有 tauri-action（D6）' ($allText -notmatch 'tauri-apps/tauri-action')

# ---- 腳本注入：run 區塊內不得內嵌 ${{ }}
$bad = [System.Collections.Generic.List[string]]::new()
foreach ($f in @(@('release.yml', $rel), @('release-verify.yml', $ver))) {
    foreach ($b in (Get-RunBlocks $f[1])) { if ($b.Text.Contains('${{')) { $bad.Add("$($f[0]):$($b.Start)") } }
}
Check 'run 區塊內沒有 ${{ }}（外部值一律經 env 傳入，防腳本注入）' ($bad.Count -eq 0) ($bad -join ',')
Check '解析到足夠的 run 區塊（防止解析器壞掉造成假通過）' ((@(Get-RunBlocks $rel)).Count -ge 8 -and (@(Get-RunBlocks $ver)).Count -ge 2)

# ---- secrets 與 Environment
foreach ($name in $relJobs.Keys) {
    $t = $relJobs[$name] -join "`n"
    if ($name -eq 'publish') {
        Check 'publish：讀 secrets（私鑰與密碼）' ($t -match 'secrets\.TAURI_SIGNING_PRIVATE_KEY\b' -and $t -match 'secrets\.TAURI_SIGNING_PRIVATE_KEY_PASSWORD\b')
        Check 'publish：environment: release' ($t -match '(?m)^    environment: release\s*$')
    }
    else {
        Check "$name：不讀 secrets、不用 Environment、不碰簽章金鑰" ($t -notmatch 'secrets\.' -and $t -notmatch '(?m)^    environment:' -and $t -notmatch 'SigningKey|SIGNING_PRIVATE')
    }
}
Check 'release-verify：不讀任何 secrets（只用 github.token）' (($ver -join "`n") -notmatch 'secrets\.')
Check '只有 publish 有 secrets 且 secrets 只在簽章步驟的 env（不在 job 層級 env）' (
    (($relJobs['publish'] -join "`n") -notmatch '(?m)^    env:\s*\n(?:      [^\n]*\n)*?      \w+: \$\{\{ secrets\.'))

# ---- 權限
Check '預設權限：頂層 permissions 只有 contents: read' (($rel -join "`n") -match '(?m)^permissions:\s*\n  contents: read\s*$' -and ($ver -join "`n") -match '(?m)^permissions:\s*\n  contents: read\s*\n  issues: write\s*$')
Check '沒有 write-all／id-token 等寬權限' ($allText -notmatch 'write-all|id-token|pull-requests: write|actions: write|packages: write')
foreach ($name in 'version', 'build', 'smoke-x64', 'smoke-arm64') {
    Check "release.yml／$name：只讀（contents: read）" (($relJobs[$name] -join "`n") -match '(?m)^    permissions:\s*\n      contents: read\s*\n' -and ($relJobs[$name] -join "`n") -notmatch '(?m)^\s+[\w-]+: write\s*$')
}
Check 'release.yml／publish：contents: write 且只有這一項' (($relJobs['publish'] -join "`n") -match '(?m)^    permissions:\s*\n      contents: write\s*\n')
Check 'release-verify／verify：contents: read＋issues: write' (($verJobs['verify'] -join "`n") -match '(?m)^    permissions:\s*\n      contents: read\s*\n      issues: write\s*\n')
Check 'checkout 一律 persist-credentials: false' ((@([regex]::Matches($allText, 'actions/checkout@')).Count) -eq (@([regex]::Matches($allText, 'persist-credentials: false')).Count))

# ---- 觸發
$relText = $rel -join "`n"
Check '觸發：push tags v*' ($relText -match "(?m)^  push:\s*\n    tags: \['v\*'\]")
Check '觸發：workflow_dispatch 輸入 waive-arm64-smoke 為 boolean、預設 false' ($relText -match '(?s)workflow_dispatch:\s*\n    inputs:\s*\n      waive-arm64-smoke:.*?type: boolean\s*\n        default: false')
Check '說明：release.yml 註明 workflow_dispatch 必須以 tag 為 ref' ($relText -match '必須以 tag 為 ref')
Check 'ARM64 冒煙豁免：smoke-arm64 在 waive-arm64-smoke 為真時整個 job 跳過；x64 冒煙沒有豁免條件' (
    ($relJobs['smoke-arm64'] -join "`n") -match "github.event_name == 'workflow_dispatch' && inputs.waive-arm64-smoke" -and ($relJobs['smoke-x64'] -join "`n") -notmatch 'waive')
Check '豁免：publish 條件明寫（success 或 skipped＋豁免）、並把「ARM64 未冒煙」寫進 notes' (
    ($relJobs['publish'] -join "`n") -match "needs.smoke-arm64.result == 'skipped'" -and ($relJobs['publish'] -join "`n") -match 'ARM64 未冒煙')
Check 'runner：x64 冒煙與建置在 windows-latest、ARM64 冒煙在 windows-11-arm' (
    ($relJobs['smoke-x64'] -join "`n") -match 'runs-on: windows-latest' -and ($relJobs['smoke-arm64'] -join "`n") -match 'runs-on: windows-11-arm' -and ($relJobs['build'] -join "`n") -match 'runner: windows-latest')
Check '建置：ARM64 以 aarch64-pc-windows-msvc 交叉編譯並 rustup target add；備案（windows-11-arm 原生）寫在註解' (
    $relText -match 'target: aarch64-pc-windows-msvc' -and $relText -match 'rustup target add' -and $relText -match '備案：windows-11-arm')
Check '建置：-Release -SkipSign（不讀私鑰），發佈：-SignOnly -Release -SigningKeyEnv' (
    ($relJobs['build'] -join "`n") -match 'package\.ps1 -Release -SkipSign -Target' -and ($relJobs['publish'] -join "`n") -match 'package\.ps1 -SignOnly -Release -InstallerPath .* -SigningKeyEnv TAURI_SIGNING_PRIVATE_KEY -SigningKeyPasswordEnv')
Check '發佈：gh release create 帶 --draft 與 --verify-tag（draft 不被更新器看到）' (($relJobs['publish'] -join "`n") -match "'--draft'" -and ($relJobs['publish'] -join "`n") -match "'--verify-tag'")
Check '發佈：上傳 setup、arm64、兩個 .sig、latest.json 共五個檔' (
    @('finance-calendar-setup.exe', 'finance-calendar-setup.exe.sig', 'finance-calendar-setup-arm64.exe', 'finance-calendar-setup-arm64.exe.sig', 'latest.json' | Where-Object { ($relJobs['publish'] -join "`n") -notmatch [regex]::Escape("dist/$_") }).Count -eq 0)
Check '驗證失敗、取消或逾時都開 issue：failure() || cancelled()＋驗證步驟有步驟層級 timeout-minutes（小於 job 的 20 分鐘）' (
    ($verJobs['verify'] -join "`n") -match 'if: \$\{\{ failure\(\) \|\| cancelled\(\) \}\}' -and ($verJobs['verify'] -join "`n") -match 'gh issue create' -and
    ($verJobs['verify'] -join "`n") -match '(?m)^        timeout-minutes: (\d+)\s*$' -and [int]$Matches[1] -lt 20)
Check '發佈：建立 release 前先比對 tag 指向的 commit 與 GITHUB_SHA；建立前檢查同 tag 沒有既有 release（含 draft）' (
    ($relJobs['publish'] -join "`n") -match 'git ls-remote origin' -and ($relJobs['publish'] -join "`n") -match 'GITHUB_SHA' -and ($relJobs['publish'] -join "`n") -match 'releases\?per_page=100')

# ---- 呼叫的腳本與參數存在
function Get-ParamNames([string]$script) {
    $t = $null; $e = $null
    $a = [System.Management.Automation.Language.Parser]::ParseFile((Join-Path $toolsDir $script), [ref]$t, [ref]$e)
    @($a.ParamBlock.Parameters | ForEach-Object { $_.Name.VariablePath.UserPath })
}
$callRx = '(?:\./)?host/tools/([A-Za-z0-9-]+\.ps1)((?:\s+-[A-Za-z0-9]+)(?:\s+(?!-)(?:"[^"]*"|\$\S+|\S+))?)*'
$calls = @()
foreach ($b in ((Get-RunBlocks $rel) + (Get-RunBlocks $ver))) {
    foreach ($m in [regex]::Matches($b.Text, '(?:\./)?host/tools/([A-Za-z0-9-]+\.ps1)([^\n]*)')) {
        $calls += , @{ Script = $m.Groups[1].Value; Args = @([regex]::Matches($m.Groups[2].Value, '(?<![\w-])-([A-Za-z][A-Za-z0-9]*)(?=\s|$)') | ForEach-Object { $_.Groups[1].Value }) }
    }
}
Check '解析到對 host/tools 腳本的呼叫（package／make-latest-json／ci-smoke／verify-published）' (
    @('package.ps1', 'make-latest-json.ps1', 'ci-smoke.ps1', 'verify-published.ps1' | Where-Object { $c = $_; @($calls | Where-Object { $_.Script -eq $c }).Count -eq 0 }).Count -eq 0) (($calls | ForEach-Object { $_.Script }) -join ',')
foreach ($c in $calls) {
    $names = Get-ParamNames $c.Script
    $missing = @($c.Args | Where-Object { $names -notcontains $_ })
    Check "呼叫 $($c.Script) 的參數都存在（$($c.Args -join ' ')）" ($missing.Count -eq 0) ("找不到：" + ($missing -join ','))
}

# ---- version job 腳本的本機演練（不寫 repo、GITHUB_OUTPUT 指到暫存檔）
$verStep = @(Get-RunBlocks ($relJobs['version'])) | Where-Object { $_.Text -match 'REF_TYPE' } | Select-Object -First 1
Check '找得到 version job 的比對腳本' ($null -ne $verStep)
if ($verStep) {
    $cargoVersion = $null
    foreach ($l in (Get-Content -LiteralPath (Join-Path $hostDir 'Cargo.toml') -Encoding UTF8)) { if ($l -match '^version\s*=\s*"([^"]+)"') { $cargoVersion = $Matches[1]; break } }
    $tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("fc-wf-tests-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
    New-Item -ItemType Directory -Force -Path $tmp | Out-Null
    try {
        $scriptFile = Join-Path $tmp 'version-step.ps1'
        [System.IO.File]::WriteAllText($scriptFile, $verStep.Text, (New-Object System.Text.UTF8Encoding($false)))
        function Run-VersionStep([string]$refType, [string]$refName) {
            $outFile = Join-Path $tmp ("out-" + [guid]::NewGuid().ToString('N').Substring(0, 6) + '.txt')
            New-Item -ItemType File -Path $outFile | Out-Null
            $env:REF_TYPE = $refType; $env:REF_NAME = $refName; $env:GITHUB_OUTPUT = $outFile
            Push-Location $repoRoot
            try { $o = & $pwsh -NoProfile -File $scriptFile 2>&1 | Out-String; $code = $LASTEXITCODE }
            finally { Pop-Location; Remove-Item Env:REF_TYPE, Env:REF_NAME, Env:GITHUB_OUTPUT -ErrorAction SilentlyContinue }
            @{ Code = $code; Out = $o; Outputs = [System.IO.File]::ReadAllText($outFile) }
        }
        $ok = Run-VersionStep 'tag' "v$cargoVersion"
        Check "version 演練：tag v$cargoVersion 與 Cargo.toml 一致 → 通過並寫入 version／tag outputs" (
            $ok.Code -eq 0 -and $ok.Outputs -match "version=$([regex]::Escape($cargoVersion))" -and $ok.Outputs -match "tag=v$([regex]::Escape($cargoVersion))") "code=$($ok.Code) $($ok.Out) outputs=$($ok.Outputs)"
        $mis = Run-VersionStep 'tag' 'v99.0.0'
        Check 'version 演練：tag 與 Cargo.toml 版本不一致 → 失敗、訊息指出兩個版本、沒有 outputs' ($mis.Code -ne 0 -and $mis.Out -match '不一致' -and $mis.Out -match $([regex]::Escape($cargoVersion)) -and $mis.Outputs -eq '') "code=$($mis.Code) $($mis.Out)"
        $br = Run-VersionStep 'branch' 'main'
        Check 'version 演練：以分支為 ref（workflow_dispatch 選了分支）→ 失敗並說明要選 tag' ($br.Code -ne 0 -and $br.Out -match 'tag' -and $br.Out -match 'Tags') "code=$($br.Code) $($br.Out)"
        foreach ($badTag in 'v0.1', 'release-1', "$cargoVersion", 'v0.1.0.0') {
            $bt = Run-VersionStep 'tag' $badTag
            Check "version 演練：tag 格式不合（$badTag）→ 失敗" ($bt.Code -ne 0 -and $bt.Outputs -eq '') "code=$($bt.Code) $($bt.Out)"
        }
    }
    finally { Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue }
}

# ---- publish 的「tag 仍指向觸發時 commit」步驟演練（本機暫存 git repo 當 origin；不連網）
$tagStep = @(Get-RunBlocks ($relJobs['publish'])) | Where-Object { $_.Text -match 'git ls-remote' } | Select-Object -First 1
Check '找得到 publish 的 tag 比對腳本' ($null -ne $tagStep)
if ($tagStep -and (Get-Command git -ErrorAction SilentlyContinue)) {
    $g = Join-Path ([System.IO.Path]::GetTempPath()) ("fc-wf-git-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
    New-Item -ItemType Directory -Force -Path $g | Out-Null
    try {
        $origin = Join-Path $g 'origin'; $work = Join-Path $g 'work'
        git init -q $origin 2>&1 | Out-Null
        git -C $origin config user.email t@example.com; git -C $origin config user.name t; git -C $origin config commit.gpgsign false; git -C $origin config tag.gpgsign false
        Set-Content -LiteralPath (Join-Path $origin 'a.txt') -Value 'one'
        git -C $origin add a.txt; git -C $origin commit -q -m one
        $c1 = (git -C $origin rev-parse HEAD).Trim()
        git -C $origin tag v1.0.0
        git -C $origin tag -a v1.1.0 -m annotated
        Set-Content -LiteralPath (Join-Path $origin 'a.txt') -Value 'two'
        git -C $origin commit -q -am two
        $c2 = (git -C $origin rev-parse HEAD).Trim()
        git clone -q $origin $work 2>&1 | Out-Null
        $stepFile = Join-Path $g 'tag-step.ps1'
        [System.IO.File]::WriteAllText($stepFile, $tagStep.Text, (New-Object System.Text.UTF8Encoding($false)))
        function Run-TagStep([string]$tag, [string]$sha) {
            $env:TAG = $tag; $env:GITHUB_SHA = $sha
            Push-Location $work
            try { $o = & $pwsh -NoProfile -File $stepFile 2>&1 | Out-String; $code = $LASTEXITCODE } finally { Pop-Location; Remove-Item Env:TAG, Env:GITHUB_SHA -ErrorAction SilentlyContinue }
            @{ Code = $code; Out = $o }
        }
        $r = Run-TagStep 'v1.0.0' $c1
        Check 'tag 演練：輕量 tag 指向觸發時的 commit → 通過' ($r.Code -eq 0) "$($r.Code) $($r.Out)"
        $r = Run-TagStep 'v1.1.0' $c1
        Check 'tag 演練：註解 tag 以 ^{} 解參考後等於 commit → 通過（tag 物件 SHA 不等於 commit SHA 也不誤判）' ($r.Code -eq 0) "$($r.Code) $($r.Out)"
        $r = Run-TagStep 'v1.0.0' $c2
        Check 'tag 演練：tag 指向的 commit 與 GITHUB_SHA 不同（tag 被移動）→ 失敗並指出兩個 SHA' ($r.Code -ne 0 -and $r.Out -match $c1 -and $r.Out -match $c2 -and $r.Out -match '被移動') "$($r.Code) $($r.Out)"
        $r = Run-TagStep 'v9.9.9' $c1
        Check 'tag 演練：遠端沒有這個 tag（被刪除）→ 失敗' ($r.Code -ne 0 -and $r.Out -match '找不到 tag') "$($r.Code) $($r.Out)"
        # 移動 tag 後（遠端改指新 commit）以舊 SHA 檢查
        git -C $origin tag -f v1.0.0 $c2 2>&1 | Out-Null
        $r = Run-TagStep 'v1.0.0' $c1
        Check 'tag 演練：遠端 tag 真的被 -f 移到新 commit 後，以舊 commit 檢查 → 失敗' ($r.Code -ne 0 -and $r.Out -match '被移動') "$($r.Code) $($r.Out)"
    }
    finally { Remove-Item -LiteralPath $g -Recurse -Force -ErrorAction SilentlyContinue }
}

# ---- 第 2 輪：發佈指令一律 --latest、released 觸發、artifact 保留期與逾時、tauri-cli 預先取得、資產核對、issue 去重
$readmeText = [System.IO.File]::ReadAllText((Join-Path $hostDir 'README.md'))
$everything = $allText + "`n" + $readmeText
$draftFalseLines = @(($everything -split "`n") | Where-Object { $_ -match 'draft=false' })
Check '發佈指令：所有提到 --draft=false 的地方（workflow 註解、step summary、README）都同時寫 --latest' (
    $draftFalseLines.Count -ge 3 -and @($draftFalseLines | Where-Object { $_ -notmatch '--latest' }).Count -eq 0) (($draftFalseLines | Where-Object { $_ -notmatch '--latest' }) -join ' | ')
Check '發佈指令：沒有「發佈並設為 Latest」這種沒給指令的模糊寫法（只在網頁勾選的指示）' ($everything -notmatch '在 GitHub 發佈並設為 Latest')
Check '發佈指令：README 與 step summary 有完整指令 gh release edit <tag> --draft=false --latest' (
    $readmeText.Contains('gh release edit <tag> --draft=false --latest') -and ($relJobs['publish'] -join "`n") -match 'gh release edit \$\(\$env:TAG\) --draft=false --latest')
Check '觸發：release-verify 用 released（不是 published），不再自己過濾 prerelease' (
    ($ver -join "`n") -match '(?m)^    types: \[released\]' -and ($ver -join "`n") -notmatch '(?m)^\s+if: .*prerelease')
Check 'artifact 保留期：installer-* 為 35 天（等於單一 run 含等待核准的上限）、README 有說明' (
    ($relJobs['build'] -join "`n") -match 'retention-days: 35' -and $readmeText -match '35 天' -and $readmeText -match 'Environment 審核')
foreach ($j in 'smoke-x64', 'smoke-arm64') {
    $mm = [regex]::Match(($relJobs[$j] -join "`n"), '(?m)^    timeout-minutes:\s*(\d+)')
    Check "$j：job 逾時涵蓋安裝 300＋存活 60＋解除安裝 300 秒再加開銷（≥ 25 分鐘）" ($mm.Success -and [int]$mm.Groups[1].Value -ge 25)
}
$smokeAst = [System.Management.Automation.Language.Parser]::ParseFile((Join-Path $toolsDir 'ci-smoke.ps1'), [ref]$null, [ref]$null)
$smokeParamDefault = @($smokeAst.ParamBlock.Parameters | Where-Object { $_.Name.VariablePath.UserPath -eq 'UninstallTimeoutSeconds' })[0].DefaultValue.Value
Check 'ci-smoke：UninstallTimeoutSeconds 預設 300（PREUNINSTALL 的 restore 最壞 2×90 秒再加等待）' ($smokeParamDefault -eq 300) "$smokeParamDefault"

$pubText = $relJobs['publish'] -join "`n"
$iPre = $pubText.IndexOf('預先取得 tauri-cli'); $iSign = $pubText.IndexOf('簽章（私鑰只在這個步驟的環境變數）'); $iSecret = $pubText.IndexOf('secrets.TAURI_SIGNING_PRIVATE_KEY')
Check 'publish：tauri-cli 在簽章前一個沒有 secret 的步驟取得（Initialize-TauriCli），且該步驟在 secrets 出現之前' ($iPre -ge 0 -and $iPre -lt $iSign -and $iSign -lt $iSecret -and $pubText -match 'Initialize-TauriCli \(Get-HostTriple\) \(Get-PackageConstants\)')

# Compare-ReleaseAssets
. (Join-Path $toolsDir 'lib\ReleaseAssets.ps1')
$ra = Join-Path ([System.IO.Path]::GetTempPath()) ("fc-wf-assets-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force -Path $ra | Out-Null
try {
    $fa = Join-Path $ra 'a.exe'; [System.IO.File]::WriteAllBytes($fa, [byte[]](1..200))
    $fb = Join-Path $ra 'b.sig'; [System.IO.File]::WriteAllText($fb, 'sigtext')
    $ha = (Get-FileHash $fa -Algorithm SHA256).Hash.ToLowerInvariant(); $hb = (Get-FileHash $fb -Algorithm SHA256).Hash.ToLowerInvariant()
    function Asset($name, $size, $digest, $state = 'uploaded') { [pscustomobject]@{ name = $name; size = $size; digest = $digest; state = $state } }
    $good = @((Asset 'a.exe' 200 "sha256:$ha"), (Asset 'b.sig' 7 "sha256:$hb"))
    $r = Compare-ReleaseAssets $good @($fa, $fb)
    Check '資產核對：名稱、大小、digest 都相符 → 通過、無警告' ($r.Errors.Count -eq 0 -and $r.Warnings.Count -eq 0) ($r.Errors -join '|')
    $r = Compare-ReleaseAssets @((Asset 'a.exe' 200 "sha256:$($ha.ToUpperInvariant())"), (Asset 'b.sig' 7 "sha256:$hb")) @($fa, $fb)
    Check '資產核對：digest 大小寫不同仍視為相符' ($r.Errors.Count -eq 0)
    $r = Compare-ReleaseAssets @((Asset 'a.exe' 200 "sha256:$ha")) @($fa, $fb)
    Check '資產核對：缺資產 → 失敗並指出名稱' ($r.Errors.Count -eq 1 -and $r.Errors[0] -match 'b\.sig')
    $r = Compare-ReleaseAssets @((Asset 'a.exe' 199 "sha256:$ha"), (Asset 'b.sig' 7 "sha256:$hb")) @($fa, $fb)
    Check '資產核對：大小不同 → 失敗' ($r.Errors.Count -eq 1 -and $r.Errors[0] -match 'a\.exe' -and $r.Errors[0] -match '大小')
    $r = Compare-ReleaseAssets @((Asset 'a.exe' 200 ('sha256:' + ('0' * 64))), (Asset 'b.sig' 7 "sha256:$hb")) @($fa, $fb)
    Check '資產核對：大小相同但 digest 不同（內容被換掉）→ 失敗' ($r.Errors.Count -eq 1 -and $r.Errors[0] -match 'digest')
    $r = Compare-ReleaseAssets @((Asset 'a.exe' 200 $null), (Asset 'b.sig' 7 "sha256:$hb")) @($fa, $fb)
    Check '資產核對：digest 為 null → 只比大小並警告（不失敗）' ($r.Errors.Count -eq 0 -and $r.Warnings.Count -eq 1 -and $r.Warnings[0] -match 'a\.exe')
    $noDigest = @([pscustomobject]@{ name = 'a.exe'; size = 200; state = 'uploaded' }, (Asset 'b.sig' 7 "sha256:$hb"))
    $r = Compare-ReleaseAssets $noDigest @($fa, $fb)
    Check '資產核對：物件沒有 digest 欄位（舊 API）→ 同樣只警告' ($r.Errors.Count -eq 0 -and $r.Warnings.Count -eq 1)
    $r = Compare-ReleaseAssets @((Asset 'a.exe' 200 "sha256:$ha" 'open'), (Asset 'b.sig' 7 "sha256:$hb")) @($fa, $fb)
    Check '資產核對：state 不是 uploaded → 失敗' ($r.Errors.Count -eq 1 -and $r.Errors[0] -match 'uploaded')
    $r = Compare-ReleaseAssets @((Asset 'a.exe' 200 "sha256:$ha"), (Asset 'a.exe' 200 "sha256:$ha"), (Asset 'b.sig' 7 "sha256:$hb")) @($fa, $fb)
    Check '資產核對：同名資產出現兩次 → 失敗' ($r.Errors.Count -eq 1 -and $r.Errors[0] -match '同名')
    Check 'publish：建立後以 Compare-ReleaseAssets 核對（dot-source lib\ReleaseAssets.ps1）' ($pubText -match 'lib/ReleaseAssets\.ps1' -and $pubText -match 'Compare-ReleaseAssets \$mine\[0\]\.assets \$files')
}
finally { Remove-Item -LiteralPath $ra -Recurse -Force -ErrorAction SilentlyContinue }

# release-verify 的 issue 去重：以 PowerShell 函式取代 gh 演練（不連網）
$issueStep = @(Get-RunBlocks ($verJobs['verify'])) | Where-Object { $_.Text -match 'gh issue create' } | Select-Object -First 1
Check '找得到 release-verify 的開 issue 腳本，且先 gh issue list 去重、有既有就 gh issue comment' (
    $null -ne $issueStep -and $issueStep.Text -match 'gh issue list' -and $issueStep.Text -match 'gh issue comment' -and $issueStep.Text.IndexOf('gh issue list') -lt $issueStep.Text.IndexOf('gh issue create'))
if ($issueStep) {
    $gd = Join-Path ([System.IO.Path]::GetTempPath()) ("fc-wf-issue-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
    New-Item -ItemType Directory -Force -Path $gd | Out-Null
    try {
        $sf = Join-Path $gd 'issue-step.ps1'
        [System.IO.File]::WriteAllText($sf, $issueStep.Text, (New-Object System.Text.UTF8Encoding($false)))
        $wrapper = Join-Path $gd 'wrap.ps1'
        [System.IO.File]::WriteAllText($wrapper, @'
param($StepFile, $LogFile)
function gh {
    Add-Content -LiteralPath $LogFile -Value ($args[0..1] -join ' ')
    if ($args[0] -eq 'issue' -and $args[1] -eq 'list') { $env:FC_TEST_LIST_JSON }
    if ($args[0] -eq 'issue' -and $args[1] -eq 'comment') { Add-Content -LiteralPath $LogFile -Value ('comment-target=' + $args[2]) }
    $global:LASTEXITCODE = 0
}
& $StepFile
'@, (New-Object System.Text.UTF8Encoding($false)))
        function Run-IssueStep([string]$listJson) {
            $log = Join-Path $gd ("log-" + [guid]::NewGuid().ToString('N').Substring(0, 6) + '.txt')
            New-Item -ItemType File -Path $log | Out-Null
            $env:FC_TEST_LIST_JSON = $listJson; $env:EXPECTED_TAG = 'v0.1.0'; $env:RUN_URL = 'https://example.com/run/1'; $env:GITHUB_REPOSITORY = 'o/r'; $env:GH_TOKEN = 'x'
            try { $o = & $pwsh -NoProfile -File $wrapper -StepFile $sf -LogFile $log 2>&1 | Out-String; $code = $LASTEXITCODE }
            finally { Remove-Item Env:FC_TEST_LIST_JSON, Env:EXPECTED_TAG, Env:RUN_URL, Env:GITHUB_REPOSITORY, Env:GH_TOKEN -ErrorAction SilentlyContinue }
            @{ Code = $code; Out = $o; Log = @(Get-Content -LiteralPath $log) }
        }
        $t = '發佈後驗證未通過：v0.1.0'
        $r = Run-IssueStep ('[{"number":7,"title":"' + $t + '"}]')
        Check 'issue 去重：已有同標題 open issue → 留言到該 issue、不新開' ($r.Code -eq 0 -and ($r.Log -contains 'issue comment') -and ($r.Log -contains 'comment-target=7') -and ($r.Log -notcontains 'issue create')) "$($r.Code) $($r.Out) $($r.Log -join ',')"
        $r = Run-IssueStep ('[{"number":9,"title":"別的標題 ' + $t + ' 後綴"}]')
        Check 'issue 去重：搜尋結果標題只是相近、不完全相同 → 新開 issue' ($r.Code -eq 0 -and ($r.Log -contains 'issue create') -and ($r.Log -notcontains 'issue comment')) "$($r.Out) $($r.Log -join ',')"
        $r = Run-IssueStep '[]'
        Check 'issue 去重：沒有既有 issue → 新開' ($r.Code -eq 0 -and ($r.Log -contains 'issue create') -and ($r.Log -notcontains 'issue comment'))
    }
    finally { Remove-Item -LiteralPath $gd -Recurse -Force -ErrorAction SilentlyContinue }
}

# ---- 位元組掃描（控制字元）與 UTF-8 無 BOM
foreach ($p in $relPath, $verPath) {
    $bytes = [System.IO.File]::ReadAllBytes($p)
    $badBytes = @(for ($i = 0; $i -lt $bytes.Length; $i++) { $b = $bytes[$i]; if (($b -lt 0x20 -and $b -ne 9 -and $b -ne 10) -or $b -eq 0x7F) { '0x{0:X2}@{1}' -f $b, $i } })
    Check "位元組掃描：$(Split-Path $p -Leaf) 沒有控制字元（含 CR）、無 BOM" ($badBytes.Count -eq 0 -and -not ($bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB)) ($badBytes -join ',')
}

Write-Host ''
Write-Host "$($script:Pass) passed, $($script:Fail) failed"
if ($script:Fail -gt 0) { exit 1 }
exit 0
