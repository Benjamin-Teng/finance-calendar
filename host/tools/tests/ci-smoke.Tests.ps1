<#
.SYNOPSIS
    host/tools/ci-smoke.ps1 的純函式與防呆測試（純 PowerShell 斷言，不需 Pester）。

.DESCRIPTION
    **不安裝、不啟動、不解除安裝任何東西**——ci-smoke.ps1 的主流程只能在 GitHub Actions 上跑，這裡只測：
      - 非 CI 環境拒絕執行（以子行程實跑，確認在任何動作之前就停、沒有碰安裝目錄）
      - 純函式：PE 機器型別、記錄檔啟動行比對、panic／ERROR 行篩選、視窗計數（只呼叫 EnumWindows）
      - 腳本結構：防呆是主流程第一個動作、安裝／解除安裝／啟動都只出現在主流程函式內
    主流程本身（安裝→啟動→存活→解除安裝）要等 workflow 第一次實跑才有證據。

.EXAMPLE
    pwsh -NoProfile -File host/tools/tests/ci-smoke.Tests.ps1
    結束碼 0＝全部通過、1＝有失敗。
#>
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$toolsDir = Split-Path $PSScriptRoot -Parent
$target = Join-Path $toolsDir 'ci-smoke.ps1'

$script:Pass = 0
$script:Fail = 0
function Check([string]$name, [bool]$cond, [string]$detail = '') {
    if ($cond) { $script:Pass++; Write-Host "PASS  $name" }
    else { $script:Fail++; Write-Host "FAIL  $name $detail" -ForegroundColor Red }
}

$tmpRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("fc-smoke-tests-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force -Path $tmpRoot | Out-Null
$pwsh = (Get-Process -Id $PID).Path

try {
    $tokens = $null; $errors = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseFile($target, [ref]$tokens, [ref]$errors)
    Check 'ci-smoke.ps1 解析 0 錯誤' (@($errors).Count -eq 0) (($errors | ForEach-Object { $_.Message }) -join ' | ')
    $fns = @($ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] }, $false))
    $cs = New-Module -Name CiSmoke -ScriptBlock ([scriptblock]::Create((($fns | ForEach-Object { $_.Extent.Text }) -join "`n")))
    foreach ($n in 'Get-CiGuardError', 'Get-PeMachine', 'Get-ExpectedPeMachine', 'Find-StartLogLines', 'Get-LogProblems', 'Get-SettingsVersion', 'New-SmokeSettingsJson', 'Get-FetchGateLines', 'Get-VisibleWindowCount', 'Get-InstallDir', 'Wait-Until', 'Read-SharedText', 'Invoke-Smoke') {
        Check "找得到函式 $n" (@($fns | Where-Object { $_.Name -eq $n }).Count -eq 1)
    }

    # ---- 防呆
    Check '防呆：GITHUB_ACTIONS=true → 允許（空字串）' ((& $cs { Get-CiGuardError 'true' }) -eq '')
    foreach ($v in '', 'false', 'True1', $null) {
        $msg = & $cs { param($x) Get-CiGuardError $x } $v
        Check "防呆：GITHUB_ACTIONS='$v' → 拒絕並說明原因" ($msg -match '拒絕執行' -and $msg -match 'GITHUB_ACTIONS')
    }
    # 實跑：非 CI 時在任何動作之前就停；沒有碰安裝目錄、沒有建立診斷資料夾
    $instDir = Join-Path $env:LOCALAPPDATA 'fc-host'
    $before = Test-Path -LiteralPath $instDir
    $logOut = Join-Path $tmpRoot 'logs-should-not-exist'
    $fakeInstaller = Join-Path $tmpRoot 'finance-calendar-setup.exe'
    [System.IO.File]::WriteAllBytes($fakeInstaller, [byte[]](0x4D, 0x5A))
    $env:GITHUB_ACTIONS = 'false'
    $out = & $pwsh -NoProfile -File $target -Installer $fakeInstaller -Arch x64 -Version 0.1.0 -LogOut $logOut 2>&1 | Out-String
    $code = $LASTEXITCODE
    Remove-Item Env:GITHUB_ACTIONS -ErrorAction SilentlyContinue
    Check '實跑（非 CI）：結束碼非 0、訊息說明只能在 GitHub Actions 跑' ($code -ne 0 -and $out -match 'GITHUB_ACTIONS') "code=$code $out"
    Check '實跑（非 CI）：沒有碰安裝目錄、沒有建立診斷資料夾（防呆早於任何動作）' ((Test-Path -LiteralPath $instDir) -eq $before -and -not (Test-Path -LiteralPath $logOut))

    # ---- 結構（靜態）
    $invoke = @($fns | Where-Object { $_.Name -eq 'Invoke-Smoke' })[0]
    $firstStmt = $invoke.Body.EndBlock.Statements[0].Extent.Text
    Check '結構：主流程第一個陳述式是防呆檢查' ($firstStmt -match 'Get-CiGuardError')
    $src = (([System.IO.File]::ReadAllText($target)) -replace "`r`n", "`n")
    $noComments = ($src -split "`n" | Where-Object { $_ -notmatch '^\s*#' }) -join "`n"
    $invokeNoComments = (($invoke.Extent.Text -replace "`r`n", "`n") -split "`n" | Where-Object { $_ -notmatch '^\s*#' }) -join "`n"
    $outside = $noComments.Replace($invokeNoComments, '')
    Check '結構（自檢）：主流程函式文字確實從全文中切除' ($outside.Length -lt $noComments.Length)
    Check '結構：Start-Process／Kill 只出現在主流程函式內（純函式不會動到系統）' ($outside -notmatch 'Start-Process|\.Kill\(')
    Check '結構：腳本不寫死設定版本（沒有 "version":<數字> 字面值），改由 settings.rs 讀出' ($noComments -notmatch '"version"\s*:\s*\d' -and $noComments -notmatch "'\{`"version`":\d")
    Check '結構：讀 SETTINGS_VERSION 發生在安裝之前（讀不到就不安裝）' ($invoke.Extent.Text.IndexOf('Get-SettingsVersion') -ge 0 -and $invoke.Extent.Text.IndexOf('Get-SettingsVersion') -lt $invoke.Extent.Text.IndexOf("-ArgumentList '/S'"))
    Check '結構：每個 Start-Process -PassThru 之後立刻快取 .Handle（避免 ExitCode 為 $null）' (
        @([regex]::Matches($invoke.Extent.Text, 'Start-Process[^\n]*-PassThru')).Count -eq 3 -and @([regex]::Matches($invoke.Extent.Text, '\$null = \$\w+\.Handle')).Count -eq 3)
    $throwLines = @(($invoke.Extent.Text -split "`n") | Where-Object { $_ -match '\bthrow\b' })
    Check '結構：成敗判準不依賴視窗（任何 throw 都不引用視窗數 $windows）；視窗數只用於資訊與 step summary' (
        @($throwLines | Where-Object { $_ -match '\$windows' }).Count -eq 0 -and $invoke.Extent.Text -match '視窗未驗')
    Check 'UninstallTimeoutSeconds 預設 300 秒、InstallTimeoutSeconds 預設 300 秒' (
        @($ast.ParamBlock.Parameters | Where-Object { $_.Name.VariablePath.UserPath -eq 'UninstallTimeoutSeconds' })[0].DefaultValue.Value -eq 300 -and
        @($ast.ParamBlock.Parameters | Where-Object { $_.Name.VariablePath.UserPath -eq 'InstallTimeoutSeconds' })[0].DefaultValue.Value -eq 300)
    Check '結構：確認抓取停用的檢查（缺行或啟用都失敗）在主流程內' ($invoke.Extent.Text -match 'Get-FetchGateLines' -and $invoke.Extent.Text -match "gate\.Off\.Count -eq 0" -and $invoke.Extent.Text -match "gate\.Enabled\.Count -gt 0")

    # ---- SETTINGS_VERSION 讀取（只讀 host/src/settings.rs，不改它）
    $realSettings = Join-Path (Split-Path $toolsDir -Parent) 'src\settings.rs'
    $realVer = & $cs { param($p) Get-SettingsVersion $p } $realSettings
    $rawRs = [System.IO.File]::ReadAllText($realSettings)
    $expectVer = [int]([regex]::Match($rawRs, 'pub const SETTINGS_VERSION: u32 = (\d+);').Groups[1].Value)
    Check "SETTINGS_VERSION：從真實 settings.rs 讀出（$realVer）且與原始碼一致" ($realVer -eq $expectVer -and $realVer -ge 1)
    $rs1 = Join-Path $tmpRoot 'rs1.rs'; [System.IO.File]::WriteAllText($rs1, "// pub const SETTINGS_VERSION: u32 = 99;`n    pub const SETTINGS_VERSION: u32 = 7;`n")
    Check 'SETTINGS_VERSION：只認真正的宣告行（註解裡的不算）、可有縮排' ((& $cs { param($p) Get-SettingsVersion $p } $rs1) -eq 7)
    $rs2 = Join-Path $tmpRoot 'rs2.rs'; [System.IO.File]::WriteAllText($rs2, "const OTHER: u32 = 3;`n")
    $threw = $false; try { & $cs { param($p) Get-SettingsVersion $p } $rs2 | Out-Null } catch { $threw = $true }
    Check 'SETTINGS_VERSION：樣式不存在 → 丟例外（不退回預設值）' $threw
    $threw = $false; try { & $cs { param($p) Get-SettingsVersion $p } (Join-Path $tmpRoot 'nope.rs') | Out-Null } catch { $threw = $true }
    Check 'SETTINGS_VERSION：檔案不存在 → 丟例外' $threw
    $json = & $cs { param($v) New-SmokeSettingsJson $v } $realVer
    $obj = $json | ConvertFrom-Json
    Check '設定檔 JSON：合法、version 等於 SETTINGS_VERSION、data_fetch 為 off' ($obj.version -eq $realVer -and $obj.data_fetch -eq 'off')

    # ---- 抓取閘門記錄行（以 scheduler.rs 的真實字串為準）
    $sched = [System.IO.File]::ReadAllText((Join-Path (Split-Path $toolsDir -Parent) 'src\fetch\scheduler.rs'))
    Check '閘門字串：與 scheduler.rs 的 log_gate 實際字串一致（off／啟用／隔離）' (
        $sched.Contains('抓取排程：data_fetch=off，不抓取') -and $sched.Contains('抓取排程：啟用（data_fetch=') -and $sched.Contains('隔離環境，不抓取（LOCALAPPDATA='))
    $offLine = '2026-10-05 10:00:02 [ INFO] fc_host::fetch: 抓取排程：data_fetch=off，不抓取（只讀取資料目錄中已有的檔案）'
    $onLine = '2026-10-05 10:00:02 [ INFO] fc_host::fetch: 抓取排程：啟用（data_fetch=auto，資料目錄=C:\x）'
    $isoLine = '2026-10-05 10:00:02 [ INFO] fc_host::fetch: 隔離環境，不抓取（LOCALAPPDATA=a，系統登記=b）；要在此環境抓取…'
    $g = & $cs { param($t) Get-FetchGateLines $t } ("$offLine`r`n$onLine`r`n$isoLine")
    Check '閘門行：off、啟用、隔離各自歸類' ($g.Off.Count -eq 1 -and $g.Enabled.Count -eq 1 -and $g.Isolated.Count -eq 1)
    $g = & $cs { param($t) Get-FetchGateLines $t } '2026-10-05 10:00:00 [ INFO] fc_host: fc-host 啟動 version=0.1.0 pid=1'
    Check '閘門行：沒有任何閘門行 → 三類皆空（主流程據此失敗）' ($g.Off.Count -eq 0 -and $g.Enabled.Count -eq 0 -and $g.Isolated.Count -eq 0)
    $g = & $cs { param($t) Get-FetchGateLines $t } '2026-10-05 10:00:02 [ INFO] other::target: 抓取排程：data_fetch=off，不抓取'
    Check '閘門行：target 不是 fc_host::fetch 的同字串不算（只認宿主實際寫的那行）' ($g.Off.Count -eq 0)
    Check '結構：以 /S 安裝與解除安裝、以 --autostart 啟動（不帶 /R）' (
        $noComments -match "-ArgumentList '/S'" -and $noComments -match "-ArgumentList '--autostart'" -and $noComments -notmatch "'/R'")
    Check '結構：finally 會收掉本腳本啟動的行程' ($noComments -match '(?s)finally \{.*Kill\(\$true\)')

    # ---- PE 機器型別
    $pwshMachine = & $cs { param($p) Get-PeMachine $p } $pwsh
    $hostArch = [System.Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture.ToString()
    $expected = switch ($hostArch) { 'X64' { 0x8664 } 'Arm64' { 0xAA64 } default { -1 } }
    Check "PE 機器型別：目前的 pwsh.exe（$hostArch）→ 0x$('{0:X4}' -f $expected)" ($pwshMachine -eq $expected) ('實際 0x{0:X4}' -f $pwshMachine)
    $notPe = Join-Path $tmpRoot 'text.bin'; [System.IO.File]::WriteAllText($notPe, 'hello world, not a PE file at all, padding padding padding padding padding')
    Check 'PE 機器型別：非 PE 檔 → 0' ((& $cs { param($p) Get-PeMachine $p } $notPe) -eq 0)
    $tiny = Join-Path $tmpRoot 'tiny.bin'; [System.IO.File]::WriteAllBytes($tiny, [byte[]](0x4D, 0x5A))
    Check 'PE 機器型別：只有 MZ 兩位元組 → 0（不丟例外）' ((& $cs { param($p) Get-PeMachine $p } $tiny) -eq 0)
    Check '預期機器型別：x64＝0x8664、arm64＝0xAA64、其他丟例外' (
        (& $cs { Get-ExpectedPeMachine 'x64' }) -eq 0x8664 -and (& $cs { Get-ExpectedPeMachine 'arm64' }) -eq 0xAA64 -and
        $(try { & $cs { Get-ExpectedPeMachine 'x86' }; $false } catch { $true }))
    # 構造一個 arm64 的最小 PE 標頭，確認讀得出 0xAA64
    $arm = Join-Path $tmpRoot 'arm.bin'
    $buf = New-Object byte[] 256; $buf[0] = 0x4D; $buf[1] = 0x5A; $buf[0x3C] = 0x80
    $buf[0x80] = 0x50; $buf[0x81] = 0x45; $buf[0x82] = 0; $buf[0x83] = 0; $buf[0x84] = 0x64; $buf[0x85] = 0xAA
    [System.IO.File]::WriteAllBytes($arm, $buf)
    Check 'PE 機器型別：構造的 ARM64 標頭 → 0xAA64' ((& $cs { param($p) Get-PeMachine $p } $arm) -eq 0xAA64)

    # ---- 記錄檔啟動行
    $log = @(
        '2026-10-05 10:00:00 [ INFO] fc_host: fc-host 啟動 version=0.1.0 pid=1234',
        '2026-10-05 10:00:01 [ INFO] fc_host: 啟動仲裁：啟動鎖=Acquired 角色=Primary',
        '2026-10-05 10:05:00 [ INFO] fc_host: fc-host 啟動 version=0.1.0 pid=4321',
        '2026-10-05 10:06:00 [ INFO] fc_host: fc-host 啟動 version=0.1.01 pid=77'
    ) -join "`r`n"
    function Find-Lines([string]$t, [string]$v, [int]$p) { , @(& $cs { param($a, $b, $c) Find-StartLogLines $a $b $c } $t $v $p) }
    Check '啟動行：版本與 PID 都相符 → 找到一行' ((Find-Lines $log '0.1.0' 1234).Count -eq 1)
    Check '啟動行：PID 只是前綴（123 vs 1234）→ 不算' ((Find-Lines $log '0.1.0' 123).Count -eq 0)
    Check '啟動行：版本不同 → 不算' ((Find-Lines $log '0.1.1' 1234).Count -eq 0)
    Check '啟動行：版本只是前綴（0.1.0 vs 記錄的 0.1.01）→ 不誤判' ((Find-Lines $log '0.1.0' 77).Count -eq 0)
    Check '啟動行：先前行程的啟動行（另一個 PID）不算本次' ((Find-Lines $log '0.1.0' 9999).Count -eq 0 -and (Find-Lines $log '0.1.0' 4321).Count -eq 1)
    Check '啟動行：空記錄 → 沒有' ((Find-Lines '' '0.1.0' 1).Count -eq 0)
    $prob = & $cs { param($t) Get-LogProblems $t } ("2026-10-05 10:00:00 [ INFO] a: ok`n2026-10-05 10:00:01 [ERROR] fc_host::panic: thread main panicked`n2026-10-05 10:00:02 [ERROR] data: 讀檔失敗`n2026-10-05 10:00:03 [ WARN] x: y")
    Check '記錄問題：panic 與一般 ERROR 分開計（panic 不重複算進 ERROR）' ($prob.Panics.Count -eq 1 -and $prob.Errors.Count -eq 1 -and $prob.Errors[0] -match '讀檔失敗')

    # ---- 讀取宿主仍開著寫入的記錄檔（v0.1.0 首發 CI：兩架構冒煙都在這步失敗）
    # 宿主（Rust std 預設共用模式 READ|WRITE|DELETE）在冒煙期間一直開著當日記錄檔寫入；
    # [IO.File]::ReadAllText 以 FileShare.Read 開檔，與既有的寫入者衝突而丟「被另一個行程使用」。
    $heldLog = Join-Path $tmpRoot 'held.log'
    $writer = [System.IO.FileStream]::new($heldLog, [System.IO.FileMode]::Create, [System.IO.FileAccess]::Write,
        ([System.IO.FileShare]::ReadWrite -bor [System.IO.FileShare]::Delete))
    try {
        $bytes = [System.Text.Encoding]::UTF8.GetBytes("2026-10-08 02:00:00 [ INFO] fc_host: fc-host 啟動 version=0.1.1 pid=42`n")
        $writer.Write($bytes, 0, $bytes.Length); $writer.Flush()
        $oldThrows = $false
        try { [void][System.IO.File]::ReadAllText($heldLog, [System.Text.Encoding]::UTF8) } catch { $oldThrows = $true }
        Check '讀記錄檔：前提——舊寫法 ReadAllText 讀寫入中的檔案會失敗（重現 CI 失敗）' $oldThrows
        $read = $null; $readErr = ''
        try { $read = & $cs { param($p) Read-SharedText $p } $heldLog } catch { $readErr = $_.Exception.Message }
        Check '讀記錄檔：Read-SharedText 讀得到寫入中的檔案內容' ($null -ne $read -and $read -match 'version=0\.1\.1 pid=42') $readErr
    }
    finally { $writer.Dispose() }

    # ---- 視窗計數（只呼叫 EnumWindows，不動任何視窗）
    $n0 = & $cs { Get-VisibleWindowCount @(0x7FFFFFF0) }
    Check '視窗計數：不存在的 PID → 0' ($n0 -eq 0)
    $nSelf = & $cs { param($p) Get-VisibleWindowCount @($p) } $PID
    Check '視窗計數：自己的 PID → 非負整數（不丟例外）' ($nSelf -ge 0) "$nSelf"

    # ---- Wait-Until
    Check 'Wait-Until：條件立刻成立 → true' (& $cs { Wait-Until { $true } 5 50 })
    Check 'Wait-Until：條件從不成立 → 逾時 false' (-not (& $cs { Wait-Until { $false } 1 100 }))
}
finally {
    Remove-Item -LiteralPath $tmpRoot -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host ''
Write-Host "$($script:Pass) passed, $($script:Fail) failed"
if ($script:Fail -gt 0) { exit 1 }
exit 0
