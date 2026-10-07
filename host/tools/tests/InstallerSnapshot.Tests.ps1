<#
.SYNOPSIS
    host/tools/lib/InstallerSnapshot.psm1 與 host/tools/installer-snapshot.ps1 的自測（純 PowerShell 斷言，不需 Pester）。

.DESCRIPTION
    演練 Save → 修改 → Restore → Compare 零差異。所有受管位置都換成暫存目錄與
    HKCU:\Software\fc-host-snapshot-test\<guid>\… 的假登錄樹（模組只吃 $Targets，正式清單寫死在 CLI 腳本），
    所以本測試不會讀寫真實的 %LOCALAPPDATA%\fc-host、%APPDATA%\tw.fintools.fc-host、HKCU Run 等。

    唯一碰到真實系統的兩處（皆唯讀）：
      1. 一次 IDesktopWallpaper 讀回，確認 C# helper 的 COM vtable 宣告正確（只呼叫 Get*）；
      2. 對 CLI 腳本的「參數錯誤、不存在的快照、Save -WhatIf」三種呼叫——這些路徑在任何寫入或讀取
         真實設定檔之前就結束。

.EXAMPLE
    pwsh -NoProfile -File host/tools/tests/InstallerSnapshot.Tests.ps1
    結束碼 0＝全部通過、1＝有失敗。
#>
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$toolsDir = Split-Path $PSScriptRoot -Parent
Import-Module (Join-Path $toolsDir 'lib\InstallerSnapshot.psm1') -Force

$script:Pass = 0
$script:Fail = 0
function Check([string]$name, [bool]$cond, [string]$detail = '') {
    if ($cond) { $script:Pass++; Write-Host "PASS  $name" }
    else { $script:Fail++; Write-Host "FAIL  $name $detail" -ForegroundColor Red }
}
function Test-Throws([scriptblock]$Block, [string]$Pattern) {
    try { & $Block | Out-Null } catch { return ($_.Exception.Message -match $Pattern) }
    return $false
}

$hive = [Microsoft.Win32.Registry]::CurrentUser
$testRoot = 'Software\fc-host-snapshot-test'
$base = "$testRoot\$([guid]::NewGuid().ToString('N'))"
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("fc-snap-test-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmp | Out-Null

function Set-Reg([string]$Key, [string]$Name, $Value, [string]$Kind) {
    $k = $hive.CreateSubKey($Key)
    try { $k.SetValue($Name, $Value, [Microsoft.Win32.RegistryValueKind]::$Kind) } finally { $k.Close() }
}
function Get-Reg([string]$Key, [string]$Name) {
    $k = $hive.OpenSubKey($Key, $false)
    if (-not $k) { return $null }
    try {
        if (-not ($k.GetValueNames() -contains $Name)) { return $null }
        return @{ Kind = [string]$k.GetValueKind($Name); Value = $k.GetValue($Name, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames) }
    } finally { $k.Close() }
}
function Test-Key([string]$Key) { $k = $hive.OpenSubKey($Key, $false); if ($k) { $k.Close(); $true } else { $false } }
function Write-Text([string]$Path, [string]$Text) {
    New-Item -ItemType Directory -Path (Split-Path $Path -Parent) -Force | Out-Null
    [System.IO.File]::WriteAllText($Path, $Text)
}

# 受管路徑（全在 $tmp 之下）與假登錄鍵
$P = @{
    Install = Join-Path $tmp 'local\fc-host'
    AppData = Join-Path $tmp 'roaming\tw.fintools.fc-host'
    Local   = Join-Path $tmp 'local\tw.fintools.fc-host'
    SmDir   = Join-Path $tmp 'sm'
    DeskDir = Join-Path $tmp 'desk'
    Themes  = Join-Path $tmp 'roaming\Microsoft\Windows\Themes'
    Snap    = Join-Path $tmp 'snap'
}
$RK = @{
    Run       = "$base\Run"
    Approved  = "$base\Approved"
    Desktop   = "$base\Desktop"
    Uninstall = "$base\Uninstall\fc-host"
    ManuParent = "$base\Manu"
    ManuProd  = "$base\Manu\fc-host"
}

$script:Wall = $null
function Reset-FakeWall {
    $script:Wall = [ordered]@{
        Ok = $true; Error = $null; Global = 'C:\wall\a.png'; Position = 4; Color = 0; Status = 1
        Monitors = @(
            [ordered]@{ Id = '\\?\DISPLAY#AAA#1'; Path = 'C:\wall\a.png'; Left = 0; Top = 0; Right = 1920; Bottom = 1080 }
            [ordered]@{ Id = '\\?\DISPLAY#BBB#2'; Path = 'C:\wall\b.png'; Left = 1920; Top = 0; Right = 3840; Bottom = 1080 }
        )
    }
}
Reset-FakeWall

function New-Targets {
    @{
        Dirs      = @(
            @{ Id = 'install'; Path = $P.Install }
            @{ Id = 'appdata'; Path = $P.AppData }
            @{ Id = 'localdata'; Path = $P.Local }
        )
        Files     = @(
            @{ Id = 'sm-fchost'; Path = (Join-Path $P.SmDir 'fc-host.lnk') }
            @{ Id = 'sm-cn'; Path = (Join-Path $P.SmDir '財經日曆.lnk') }
            @{ Id = 'desktop-fchost'; Path = (Join-Path $P.DeskDir 'fc-host.lnk') }
        )
        RegValues = @(
            @{ Id = 'run'; Key = $RK.Run; Name = 'fc-host' }
            @{ Id = 'approved'; Key = $RK.Approved; Name = 'fc-host' }
            @{ Id = 'wallpaper'; Key = $RK.Desktop; Name = 'Wallpaper' }
            @{ Id = 'style'; Key = $RK.Desktop; Name = 'WallpaperStyle' }
            @{ Id = 'tile'; Key = $RK.Desktop; Name = 'TileWallpaper' }
        )
        RegTrees  = @(
            @{ Id = 'uninstall'; Key = $RK.Uninstall }
            @{ Id = 'manu'; Key = $RK.ManuProd; PruneParentKey = $RK.ManuParent }
        )
        Watch     = @(
            @{ Id = 'themes'; Path = $P.Themes; Include = @('TranscodedWallpaper', 'Transcoded_*', 'TranscodedWallpaperCache\*') }
        )
        Wallpaper = $true
    }
}
$script:HostRunning = @()
function New-Ctx {
    $c = New-FcSnapshotContext
    $c.IsHostRunning = { $script:HostRunning }
    $c.ReadWallpaper = { $script:Wall }
    $c.HashLimitBytes = 1000   # 讓 big.bin（5000 位元組）走「大小＋修改時間」分支
    $c
}

function Initialize-State {
    Write-Text (Join-Path $P.Install 'fc-host.exe') 'EXE-v1'
    Write-Text (Join-Path $P.Install 'sub\a.txt') 'alpha'
    New-Item -ItemType Directory -Path (Join-Path $P.Install 'empty') -Force | Out-Null
    [System.IO.File]::WriteAllBytes((Join-Path $P.Install 'big.bin'), [byte[]](1..250 * 20))
    (Get-Item (Join-Path $P.Install 'big.bin')).LastWriteTimeUtc = [datetime]'2024-03-05T06:07:08Z'
    Write-Text (Join-Path $P.AppData 'settings.json') '{"a":1}'
    # localdata：刻意不存在
    Write-Text (Join-Path $P.SmDir 'fc-host.lnk') 'LNK-old'
    # 財經日曆.lnk、桌面 fc-host.lnk：刻意不存在
    Write-Text (Join-Path $P.Themes 'TranscodedWallpaper') 'TW-original'
    Write-Text (Join-Path $P.Themes 'Transcoded_000') 'T0-original'
    Write-Text (Join-Path $P.Themes 'TranscodedWallpaperCache\c1') 'cache-1'
    Write-Text (Join-Path $P.Themes 'other.dat') 'not-watched'
    Set-Reg $RK.Run 'fc-host' '' 'String'                                   # 空字串（≠ 不存在）
    Set-Reg $RK.Approved 'fc-host' ([byte[]](2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0)) 'Binary'
    Set-Reg $RK.Desktop 'Wallpaper' '%SystemRoot%\Web\wall.jpg' 'ExpandString'  # 不得被展開
    Set-Reg $RK.Desktop 'TileWallpaper' '0' 'String'
    # WallpaperStyle：刻意不存在
    Set-Reg $RK.Uninstall 'DisplayName' 'fc-host' 'String'
    Set-Reg $RK.Uninstall 'EstimatedSize' ([int]-1) 'DWord'                 # 0xFFFFFFFF
    Set-Reg $RK.Uninstall 'Big' ([long]9007199254740993) 'QWord'            # 超過 double 精度
    Set-Reg $RK.Uninstall 'Multi' ([string[]]@('a', 'b')) 'MultiString'
    Set-Reg $RK.Uninstall 'Bin' ([byte[]](0, 255, 7)) 'Binary'
    Set-Reg $RK.Uninstall '' 'default-value' 'String'                       # 預設值
    Set-Reg "$($RK.Uninstall)\Sub" 'x' 'y' 'String'
    # ManuParent／ManuProd：刻意不存在
}

function Invoke-Mutation {
    Write-Text (Join-Path $P.Install 'sub\a.txt') 'ALPHA'
    Remove-Item (Join-Path $P.Install 'fc-host.exe')
    Write-Text (Join-Path $P.Install 'new.txt') 'extra'
    (Get-Item (Join-Path $P.Install 'big.bin')).LastWriteTimeUtc = [datetime]'2025-01-01T00:00:00Z'
    Remove-Item (Join-Path $P.Install 'empty') -Recurse
    Remove-Item $P.AppData -Recurse -Force
    Write-Text (Join-Path $P.Local 'logs\x.log') 'log'
    Write-Text (Join-Path $P.SmDir 'fc-host.lnk') 'LNK-new'
    Write-Text (Join-Path $P.SmDir '財經日曆.lnk') 'LNK-cn'
    Write-Text (Join-Path $P.DeskDir 'fc-host.lnk') 'LNK-desktop'
    Write-Text (Join-Path $P.Themes 'TranscodedWallpaper') 'TW-changed-by-explorer'
    Remove-Item (Join-Path $P.Themes 'Transcoded_000')
    Write-Text (Join-Path $P.Themes 'TranscodedWallpaperCache\c2') 'cache-2'
    Write-Text (Join-Path $P.Themes 'other.dat') 'changed-but-not-watched'
    $k = $hive.OpenSubKey($RK.Run, $true); $k.DeleteValue('fc-host'); $k.Close()
    Set-Reg $RK.Approved 'fc-host' ([byte[]](3, 0, 0, 0)) 'Binary'
    Set-Reg $RK.Desktop 'WallpaperStyle' '2' 'String'
    $k = $hive.OpenSubKey($RK.Desktop, $true); $k.DeleteValue('TileWallpaper'); $k.Close()
    Set-Reg $RK.Uninstall 'DisplayName' '財經日曆' 'String'
    $k = $hive.OpenSubKey($RK.Uninstall, $true); $k.DeleteValue('Multi'); $k.Close()
    Set-Reg "$($RK.Uninstall)\Sub2" 'z' 'z' 'String'
    Set-Reg $RK.ManuProd '' 'C:\somewhere' 'String'
    Set-Reg $RK.ManuProd 'Installer Language' '1028' 'String'
    $script:Wall.Monitors[0].Path = 'C:\wall\other.png'
    $script:Wall.Global = ''
    $script:Wall.Position = 2
}

try {
    $targets = New-Targets
    $ctx = New-Ctx

    # ── 1. Save → 立刻 Compare：零差異 ───────────────────────────────────────────────────────
    Initialize-State
    $origBigTicks = (Get-Item (Join-Path $P.Install 'big.bin')).LastWriteTimeUtc.Ticks
    Save-FcSnapshot -Dir $P.Snap -Targets $targets -Ctx $ctx *> $null
    Check '快照完整標記 snapshot.json 存在' (Test-Path (Join-Path $P.Snap 'snapshot.json'))
    $cmp0 = Compare-FcSnapshot -Dir $P.Snap -Targets $targets -Ctx $ctx
    Check 'Save 後立刻 Compare＝零差異' ($cmp0.Differences.Count -eq 0) ($cmp0.Differences -join '; ')

    # ── 2. 修改 → Compare 逐類抓到 ───────────────────────────────────────────────────────────
    Invoke-Mutation
    $cmp1 = Compare-FcSnapshot -Dir $P.Snap -Targets $targets -Ctx $ctx
    $d1 = $cmp1.Differences -join "`n"
    foreach ($pair in @(
            @('檔案內容變更', 'sub\\a\.txt.*內容不同'), @('檔案被刪', '缺少 fc-host\.exe'), @('多出檔案', '多出 new\.txt'),
            @('大檔只改修改時間', 'big\.bin.*修改時間'), @('空目錄被刪', '缺少 empty'),
            @('整個目錄被刪', '目錄 appdata：快照時存在、現在不存在'), @('原本不存在的目錄被建立', '目錄 localdata：快照時不存在、現在存在'),
            @('捷徑內容變更', '檔案 sm-fchost：內容不同'), @('原本不存在的捷徑被建立', '檔案 sm-cn：快照時不存在、現在存在'),
            @('空字串值被刪', "\[fc-host\]：快照時存在、現在不存在"), @('二進位值改變', 'Approved'),
            @('原本不存在的值被建立', 'WallpaperStyle\]：快照時不存在、現在存在'), @('值被刪', 'TileWallpaper\]：快照時存在、現在不存在'),
            @('子樹內值改變', 'DisplayName'), @('子樹內值被刪', 'V\|\|Multi'), @('子樹多出子鍵', '多出 K\|Sub2'),
            @('原本不存在的子樹被建立', '登錄子樹 HKCU\\.*Manu\\fc-host：快照時不存在、現在存在'),
            @('桌布單螢幕改變', '螢幕 .*AAA.* 的桌布不同'), @('桌布全域路徑', '全域路徑不同'), @('桌布位置', 'GetPosition'),
            @('桌面捷徑（I5）被建立', '檔案 desktop-fchost：快照時不存在、現在存在')
        )) {
        Check "Compare 抓到：$($pair[0])" ($d1 -match $pair[1]) "（差異清單：$d1）"
    }
    $ro1 = $cmp1.ReportOnly -join "`n"
    Check '僅回報項：內容變更被回報' ($ro1 -match 'TranscodedWallpaper 大小不同')
    Check '僅回報項：被刪的 Transcoded_* 被回報' ($ro1 -match '缺少 Transcoded_000')
    Check '僅回報項：TranscodedWallpaperCache 下新增的檔被回報' ($ro1 -match '多出 TranscodedWallpaperCache\\c2')
    Check '僅回報項：不在清單內的檔（other.dat）不回報' ($ro1 -notmatch 'other\.dat')
    Check '僅回報項不混進 Differences（不影響零差異判定）' ($d1 -notmatch 'Transcoded')

    # ── 3. -WhatIf：不動任何東西；host 在跑：拒絕且不動 ─────────────────────────────────────
    $before = Get-Content (Join-Path $P.Install 'new.txt') -Raw
    $wi = Restore-FcSnapshot -Dir $P.Snap -Targets $targets -Ctx $ctx -WhatIf *> $null
    Check 'Restore -WhatIf 沒有改動（mutation 的 new.txt 仍在、appdata 仍不存在）' ((Test-Path (Join-Path $P.Install 'new.txt')) -and -not (Test-Path $P.AppData))
    $wiObj = Restore-FcSnapshot -Dir $P.Snap -Targets $targets -Ctx $ctx -WhatIf 6>$null
    Check 'Restore -WhatIf 回傳動作清單' (@($wiObj.Actions).Count -ge 10) "（$(@($wiObj.Actions).Count) 項）"
    $script:HostRunning = @('fc-host#4242')
    Check 'host 在跑：Restore 拒絕' (Test-Throws { Restore-FcSnapshot -Dir $P.Snap -Targets $targets -Ctx $ctx } '拒絕還原.*fc-host')
    Check 'host 在跑：拒絕後沒有任何改動' ((Test-Path (Join-Path $P.Install 'new.txt')) -and -not (Test-Path $P.AppData))
    Check 'host 在跑：Save 拒絕' (Test-Throws { Save-FcSnapshot -Dir (Join-Path $tmp 'snap-x') -Targets $targets -Ctx $ctx } '拒絕快照.*fc-host')
    $script:HostRunning = @()

    # ── 4. Restore：自動 Compare；桌布只比對不設定 ──────────────────────────────────────────
    $res = Restore-FcSnapshot -Dir $P.Snap -Targets $targets -Ctx $ctx 6>$null
    Check 'Restore 無動作失敗' ($res.Errors.Count -eq 0) ($res.Errors -join '; ')
    $nonWall = @($res.Differences | Where-Object { $_ -notmatch '^桌布' })
    Check 'Restore 後除桌布外零差異' ($nonWall.Count -eq 0) ($nonWall -join '; ')
    Check 'Restore 不設桌布：桌布差異仍被回報（只回報、不修）' (@($res.Differences | Where-Object { $_ -match '^桌布' }).Count -ge 1)
    Reset-FakeWall
    $cmp2 = Compare-FcSnapshot -Dir $P.Snap -Targets $targets -Ctx $ctx
    Check '桌布還原後再 Compare＝完全零差異' ($cmp2.Differences.Count -eq 0) ($cmp2.Differences -join '; ')
    Check 'Restore 不還原僅回報項：TranscodedWallpaper 維持被「explorer」改過的內容' ((Get-Content (Join-Path $P.Themes 'TranscodedWallpaper') -Raw) -eq 'TW-changed-by-explorer')
    Check 'Restore 不還原僅回報項：被刪的 Transcoded_000 沒有被補回' (-not (Test-Path (Join-Path $P.Themes 'Transcoded_000')))
    Check 'Restore 不還原僅回報項：多出的 cache 檔沒有被清掉' (Test-Path (Join-Path $P.Themes 'TranscodedWallpaperCache\c2'))
    Check 'Restore 結果的 ReportOnly 仍列出這些差異（只回報）' (@($res.ReportOnly).Count -ge 3) "（$(@($res.ReportOnly).Count) 項）"
    Check 'Restore 結果的 Differences 不含僅回報項' (@($res.Differences | Where-Object { $_ -match 'Transcoded' }).Count -eq 0)
    Check 'Restore 後桌面捷徑（原本不存在）被移除' (-not (Test-Path (Join-Path $P.DeskDir 'fc-host.lnk')))
    Check 'Restore 結果標示桌布已驗證（兩次讀回都成功）' ($res.WallpaperVerified -eq $true)
    # 僅回報項已被改過；後面的「零差異」斷言只看 Differences。把它恢復成快照時的樣子，讓後續情境從乾淨狀態開始。
    Write-Text (Join-Path $P.Themes 'TranscodedWallpaper') 'TW-original'
    Write-Text (Join-Path $P.Themes 'Transcoded_000') 'T0-original'
    Remove-Item (Join-Path $P.Themes 'TranscodedWallpaperCache\c2')

    # 還原內容細節
    Check '原本不存在的目錄還原後不存在（localdata）' (-not (Test-Path $P.Local))
    Check '原本不存在的捷徑還原後不存在' (-not (Test-Path (Join-Path $P.SmDir '財經日曆.lnk')))
    Check '被刪的檔案回來了' ((Get-Content (Join-Path $P.Install 'fc-host.exe') -Raw) -eq 'EXE-v1')
    Check '多出的檔案被清掉' (-not (Test-Path (Join-Path $P.Install 'new.txt')))
    Check '空目錄回來了' (Test-Path (Join-Path $P.Install 'empty') -PathType Container)
    Check '大檔修改時間還原（robocopy 保留時間戳）' ((Get-Item (Join-Path $P.Install 'big.bin')).LastWriteTimeUtc.Ticks -eq $origBigTicks)
    $r = Get-Reg $RK.Run 'fc-host'
    Check '空字串值還原為「存在且為空字串」' ($r -and $r.Kind -eq 'String' -and $r.Value -eq '')
    Check '原本不存在的值還原後不存在（WallpaperStyle）' ($null -eq (Get-Reg $RK.Desktop 'WallpaperStyle'))
    $r = Get-Reg $RK.Desktop 'Wallpaper'
    Check 'ExpandString 還原後型別與原文不變（未展開環境變數）' ($r.Kind -eq 'ExpandString' -and $r.Value -eq '%SystemRoot%\Web\wall.jpg')
    $r = Get-Reg $RK.Approved 'fc-host'
    Check 'Binary 還原位元組一致' ($r.Kind -eq 'Binary' -and (($r.Value -join ',') -eq '2,0,0,0,0,0,0,0,0,0,0,0'))
    $r = Get-Reg $RK.Uninstall 'EstimatedSize'
    Check 'DWord 0xFFFFFFFF 還原一致' ($r.Kind -eq 'DWord' -and $r.Value -eq -1)
    $r = Get-Reg $RK.Uninstall 'Big'
    Check 'QWord 超過 double 精度的值還原一致' ($r.Kind -eq 'QWord' -and $r.Value -eq [long]9007199254740993)
    $r = Get-Reg $RK.Uninstall 'Multi'
    Check 'MultiString 還原一致' ($r.Kind -eq 'MultiString' -and (($r.Value -join '|') -eq 'a|b'))
    $r = Get-Reg $RK.Uninstall ''
    Check '子樹預設值還原' ($r -and $r.Value -eq 'default-value')
    Check '子樹多出的子鍵被清掉' (-not (Test-Key "$($RK.Uninstall)\Sub2"))
    Check '原本不存在的子樹與空父鍵還原後都不存在（對應 /ifempty）' (-not (Test-Key $RK.ManuProd) -and -not (Test-Key $RK.ManuParent))

    # ── 5. 父鍵快照時就存在且有別的子鍵：還原後保留 ─────────────────────────────────────────
    Set-Reg "$($RK.ManuParent)\other" 'v' '1' 'String'
    $snap2 = Join-Path $tmp 'snap2'
    Save-FcSnapshot -Dir $snap2 -Targets $targets -Ctx $ctx *> $null
    Set-Reg $RK.ManuProd '' 'C:\x' 'String'
    $res2 = Restore-FcSnapshot -Dir $snap2 -Targets $targets -Ctx $ctx 6>$null
    Check '父鍵快照時存在：還原後父鍵與兄弟鍵保留、子樹移除' ((Test-Key "$($RK.ManuParent)\other") -and -not (Test-Key $RK.ManuProd))
    Check '父鍵情境 Restore 零差異' ($res2.Differences.Count -eq 0 -and $res2.Errors.Count -eq 0) ($res2.Differences -join '; ')
    $hive.DeleteSubKeyTree($RK.ManuParent, $false)

    # ── 6. 安全檢查 ──────────────────────────────────────────────────────────────────────────
    Check 'Save 拒絕寫進非空目錄（不覆寫既有快照）' (Test-Throws { Save-FcSnapshot -Dir $P.Snap -Targets $targets -Ctx $ctx } '已存在且非空')
    Check 'Save 拒絕快照目錄位於受管目錄內' (Test-Throws { Save-FcSnapshot -Dir (Join-Path $P.Install 'snap-inside') -Targets $targets -Ctx $ctx } '位於會被還原刪除的目錄')
    Check 'Restore 拒絕快照目錄位於受管目錄內' (Test-Throws { Restore-FcSnapshot -Dir (Join-Path $P.AppData 'snap') -Targets $targets -Ctx $ctx } '位於會被還原刪除的目錄')
    Check 'Compare 拒絕不存在的快照' (Test-Throws { Compare-FcSnapshot -Dir (Join-Path $tmp 'nope') -Targets $targets -Ctx $ctx } '缺 snapshot\.json')
    $other = New-Targets
    $other.Dirs[0] = @{ Id = 'install'; Path = (Join-Path $tmp 'elsewhere\fc-host') }
    Check '快照路徑與目前清單不同：拒絕還原' (Test-Throws { Restore-FcSnapshot -Dir $P.Snap -Targets $other -Ctx $ctx } '路徑.*不同')
    $incomplete = Join-Path $tmp 'snap-incomplete'
    Copy-Item $P.Snap $incomplete -Recurse
    Remove-Item (Join-Path $incomplete 'snapshot.json')
    Check '缺完整標記的快照：拒絕' (Test-Throws { Restore-FcSnapshot -Dir $incomplete -Targets $targets -Ctx $ctx } '缺 snapshot\.json')
    foreach ($bad in @('', 'relative\dir', 'C:\', 'C:\Users')) {
        Check "Assert-SafeDeleteDir 擋 '$bad'" (Test-Throws { Assert-SafeDeleteDir $bad } '拒絕刪除')
    }
    # 刪除防線失敗時整個 Restore 不得動任何東西
    $shallow = New-Targets
    $shallow.RegTrees[0] = @{ Id = 'uninstall'; Key = $testRoot }   # 深度 2：低於刪除下限 3（內容只有本測試的假樹，Save 不會讀大量資料）
    $shallowSnapDir = Join-Path $tmp 'snap-shallow'
    Set-Reg $RK.Run 'fc-host' 'A' 'String'
    Save-FcSnapshot -Dir $shallowSnapDir -Targets $shallow -Ctx $ctx *> $null
    Set-Reg $RK.Run 'fc-host' 'CHANGED' 'String'
    $refused = Test-Throws { Restore-FcSnapshot -Dir $shallowSnapDir -Targets $shallow -Ctx $ctx } '路徑過淺'
    Check '登錄子樹路徑過淺：Restore 拒絕' $refused
    Check '防線失敗時整個 Restore 不動任何目標（Run 仍是 CHANGED）' ((Get-Reg $RK.Run 'fc-host').Value -eq 'CHANGED')

    # ── 6a. I1／I2：登錄比對區分大小寫、ISO 日期樣式字串不被改寫、Save 自我驗證 ─────────────────
    function New-MiniTargets([string]$Key, [bool]$Wall = $false) {
        @{
            Dirs = @(); Files = @(); Watch = @()
            RegValues = @(
                @{ Id = 'bin'; Key = $Key; Name = 'Bin' }
                @{ Id = 'str'; Key = $Key; Name = 'Str' }
                @{ Id = 'date'; Key = $Key; Name = 'Date' }
            )
            RegTrees = @( @{ Id = 'tree'; Key = "$Key\Tree" } )
            Wallpaper = $Wall
        }
    }
    $mk = "$base\Mini"
    $mini = New-MiniTargets $mk
    Set-Reg $mk 'Bin' ([byte[]](0, 0, 0)) 'Binary'          # base64＝AAAA
    Set-Reg $mk 'Str' 'abc' 'String'
    Set-Reg $mk 'Date' '2024-01-02T03:04:05' 'String'        # ISO 日期樣式：ConvertFrom-Json 預設會轉成 DateTime（I2）
    Set-Reg "$mk\Tree" 'B' ([byte[]](0, 0, 0)) 'Binary'
    Set-Reg "$mk\Tree" 'S' 'abc' 'String'
    Set-Reg "$mk\Tree" 'D' '2025-12-31T23:59:59Z' 'String'
    $snapMini = Join-Path $tmp 'snap-mini'
    $rMini = Save-FcSnapshot -Dir $snapMini -Targets $mini -Ctx $ctx 6>$null
    Check 'I2：Save 自我驗證通過（含 ISO 日期樣式字串）→ Complete' ($rMini.Complete -and (Test-Path (Join-Path $snapMini 'snapshot.json'))) ($rMini.Differences -join '; ')
    Set-Reg $mk 'Bin' ([byte[]](0x69, 0xA6, 0x9A)) 'Binary'  # base64＝aaaa：只差大小寫
    Set-Reg $mk 'Str' 'ABC' 'String'
    Set-Reg "$mk\Tree" 'B' ([byte[]](0x69, 0xA6, 0x9A)) 'Binary'
    Set-Reg "$mk\Tree" 'S' 'ABC' 'String'
    $cMini = Compare-FcSnapshot -Dir $snapMini -Targets $mini -Ctx $ctx
    $dm = $cMini.Differences -join "`n"
    Check 'I1：單值 Binary 只差 base64 大小寫 → 抓得到' ($dm -match '\[Bin\]：不同')
    Check 'I1：單值 String 只差大小寫 → 抓得到' ($dm -match '\[Str\]：不同')
    Check 'I1：子樹內 Binary 只差 base64 大小寫 → 抓得到' ($dm -match 'V\|\|B 不同')
    Check 'I1：子樹內 String 只差大小寫 → 抓得到' ($dm -match 'V\|\|S 不同')
    $rrMini = Restore-FcSnapshot -Dir $snapMini -Targets $mini -Ctx $ctx 6>$null
    Check 'I2：Restore 後零差異' ($rrMini.Differences.Count -eq 0 -and $rrMini.Errors.Count -eq 0) ($rrMini.Differences -join '; ')
    Check 'I2：ISO 日期樣式字串（單值）原樣寫回，不被改寫成地區格式' ((Get-Reg $mk 'Date').Value -ceq '2024-01-02T03:04:05') ((Get-Reg $mk 'Date').Value)
    Check 'I2：ISO 日期樣式字串（子樹內）原樣寫回' ((Get-Reg "$mk\Tree" 'D').Value -ceq '2025-12-31T23:59:59Z') ((Get-Reg "$mk\Tree" 'D').Value)
    Check 'I1：還原後 Binary／String 回到原值（含大小寫）' ((((Get-Reg $mk 'Bin').Value) -join ',') -eq '0,0,0' -and (Get-Reg $mk 'Str').Value -ceq 'abc')

    $script:WallCalls = 0
    $ctxFlip = New-Ctx
    $ctxFlip.ReadWallpaper = { $script:WallCalls++; [ordered]@{ Ok = $true; Error = $null; Global = "C:\w\$($script:WallCalls).png"; Position = 4; Color = 0; Status = 1; Monitors = @() } }
    $snapFlip = Join-Path $tmp 'snap-flip'
    $rFlip = Save-FcSnapshot -Dir $snapFlip -Targets (New-MiniTargets $mk $true) -Ctx $ctxFlip 6>$null
    Check 'I2：Save 後立刻 Compare 有差異 → 快照判為不完整（Complete=false）' (-not $rFlip.Complete -and @($rFlip.Differences | Where-Object { $_ -match '全域路徑不同' }).Count -eq 1)
    Check 'I2：不完整的快照沒有 snapshot.json，Restore 拒絕' (-not (Test-Path (Join-Path $snapFlip 'snapshot.json')) -and (Test-Throws { Restore-FcSnapshot -Dir $snapFlip -Targets (New-MiniTargets $mk $true) -Ctx $ctx } '缺 snapshot\.json'))

    # ── 6b. M5：桌布讀回失敗要明確標示「未驗證」 ───────────────────────────────────────────
    $snapW = Join-Path $tmp 'snap-wall'
    $miniW = New-MiniTargets $mk $true
    $null = Save-FcSnapshot -Dir $snapW -Targets $miniW -Ctx $ctx 6>$null
    Check 'M5：兩次讀回都成功 → WallpaperVerified＝true' ((Compare-FcSnapshot -Dir $snapW -Targets $miniW -Ctx $ctx).WallpaperVerified -eq $true)
    $script:Wall.Ok = $false; $script:Wall.Error = 'locked'
    $cW = Compare-FcSnapshot -Dir $snapW -Targets $miniW -Ctx $ctx
    Check 'M5：現在讀回失敗 → WallpaperVerified＝false 且警告寫「桌布未驗證」' ($cW.WallpaperVerified -eq $false -and ($cW.Warnings -join ' ') -match '桌布未驗證')
    Reset-FakeWall

    # ── 6c. I3：還原前先確認快照內容完整，缺一項整個拒絕且真實資料不被動到 ─────────────────────
    Set-Reg $RK.Run 'fc-host' '' 'String'
    $snapI3 = Join-Path $tmp 'snap-i3'
    $rI3 = Save-FcSnapshot -Dir $snapI3 -Targets $targets -Ctx $ctx 6>$null
    Check 'I3 前置：完整快照 Save 成功' ($rI3.Complete) ($rI3.Differences -join '; ')
    function Add-Markers {
        Write-Text (Join-Path $P.Install 'marker.txt') 'm'
        Write-Text (Join-Path $P.AppData 'marker.txt') 'm'
        Write-Text (Join-Path $P.SmDir 'fc-host.lnk') 'LNK-marker'
        Set-Reg $RK.Run 'fc-host' 'MARK' 'String'
    }
    function Test-MarkersIntact {
        (Test-Path (Join-Path $P.Install 'marker.txt')) -and (Test-Path (Join-Path $P.AppData 'marker.txt')) -and
        ((Get-Content (Join-Path $P.SmDir 'fc-host.lnk') -Raw) -eq 'LNK-marker') -and ((Get-Reg $RK.Run 'fc-host').Value -eq 'MARK') -and
        (Test-Path (Join-Path $P.Install 'fc-host.exe')) -and (Test-Path (Join-Path $P.Install 'sub\a.txt'))
    }
    $variants = @(
        @{ Name = '整個目錄內容 payload\dirs\install 被刪'; Break = { param($d) Remove-Item (Join-Path $d 'payload\dirs\install') -Recurse -Force } }
        @{ Name = 'payload 內少一個檔（appdata\settings.json）'; Break = { param($d) Remove-Item (Join-Path $d 'payload\dirs\appdata\settings.json') } }
        @{ Name = 'payload 內少一個空目錄（install\empty）'; Break = { param($d) Remove-Item (Join-Path $d 'payload\dirs\install\empty') -Recurse -Force } }
        @{ Name = 'payload 內檔案被截短（大小不符）'; Break = { param($d) [System.IO.File]::WriteAllText((Join-Path $d 'payload\dirs\install\sub\a.txt'), 'x') } }
        @{ Name = 'payload 內多出檔案'; Break = { param($d) [System.IO.File]::WriteAllText((Join-Path $d 'payload\dirs\install\extra.bin'), 'x') } }
        @{ Name = '捷徑內容 payload\files\sm-fchost 被刪'; Break = { param($d) Remove-Item (Join-Path $d 'payload\files\sm-fchost') } }
        @{ Name = '捷徑內容被竄改（SHA256 不符）'; Break = { param($d) [System.IO.File]::WriteAllText((Join-Path $d 'payload\files\sm-fchost'), 'tampered-content') } }
        @{ Name = 'manifest-install.json 被刪'; Break = { param($d) Remove-Item (Join-Path $d 'manifest-install.json') } }
        @{ Name = '登錄子樹標為存在但沒有內容'; Break = { param($d) $f = Join-Path $d 'registry.json'; $j = Get-Content $f -Raw | ConvertFrom-Json -DateKind String; ($j.Trees | Where-Object { $_.Id -eq 'uninstall' }).Node = $null; [System.IO.File]::WriteAllText($f, ($j | ConvertTo-Json -Depth 64 -Compress)) } }
        @{ Name = '登錄值標為存在但缺資料'; Break = { param($d) $f = Join-Path $d 'registry.json'; $j = Get-Content $f -Raw | ConvertFrom-Json -DateKind String; ($j.Values | Where-Object { $_.Id -eq 'approved' }).Data = $null; [System.IO.File]::WriteAllText($f, ($j | ConvertTo-Json -Depth 64 -Compress)) } }
    )
    $vi = 0
    foreach ($v in $variants) {
        $vi++
        $copy = Join-Path $tmp "snap-i3-v$vi"
        Copy-Item $snapI3 $copy -Recurse
        & $v.Break $copy
        Add-Markers
        $threw = Test-Throws { Restore-FcSnapshot -Dir $copy -Targets $targets -Ctx $ctx } '快照內容不完整'
        Check "I3：$($v.Name) → 整個拒絕" $threw
        Check "I3：$($v.Name) → 真實目錄／捷徑／登錄完全沒被動到" (Test-MarkersIntact)
    }
    $okI3 = Restore-FcSnapshot -Dir $snapI3 -Targets $targets -Ctx $ctx 6>$null
    Check 'I3：未損壞的快照仍可正常還原（標記被清掉、零差異）' ((-not (Test-Path (Join-Path $P.Install 'marker.txt'))) -and $okI3.Differences.Count -eq 0 -and $okI3.Errors.Count -eq 0) ($okI3.Differences -join '; ')

    # ── 6d. M1：父鍵與鍵名取自目前清單，不信任快照內的 PruneParentKey ────────────────────────
    $otherKey = "$base\OtherEmpty"
    $hive.CreateSubKey($otherKey).Close()
    $snapM1 = Join-Path $tmp 'snap-m1'
    $null = Save-FcSnapshot -Dir $snapM1 -Targets $targets -Ctx $ctx 6>$null
    $f = Join-Path $snapM1 'registry.json'
    $j = Get-Content $f -Raw | ConvertFrom-Json -DateKind String
    ($j.Trees | Where-Object { $_.Id -eq 'manu' }).PruneParentKey = $otherKey
    [System.IO.File]::WriteAllText($f, ($j | ConvertTo-Json -Depth 64 -Compress))
    Set-Reg $RK.ManuProd '' 'x' 'String'
    $null = Restore-FcSnapshot -Dir $snapM1 -Targets $targets -Ctx $ctx 6>$null
    Check 'M1：被竄改的 PruneParentKey 不會讓「空鍵才刪」作用到別的鍵' (Test-Key $otherKey)
    Check 'M1：目前清單的父鍵仍照規則處理（快照時不存在、現在是空鍵 → 刪）' (-not (Test-Key $RK.ManuParent))
    $hive.DeleteSubKey($otherKey, $false)

    # ── 6e. M4：還原後的比對本身失敗，已累積的結果仍要回報 ──────────────────────────────────
    $snapM4 = Join-Path $tmp 'snap-m4'
    $null = Save-FcSnapshot -Dir $snapM4 -Targets $targets -Ctx $ctx 6>$null
    Write-Text (Join-Path $P.Install 'm4.txt') 'x'
    $script:WallThrow = $true
    $ctx4 = New-Ctx
    $ctx4.ReadWallpaper = { if ($script:WallThrow) { throw 'boom' }; $script:Wall }
    try { $r4 = Restore-FcSnapshot -Dir $snapM4 -Targets $targets -Ctx $ctx4 6>$null }
    catch { $r4 = [pscustomobject]@{ Errors = @("Restore 丟出例外：$($_.Exception.Message)"); Actions = @() } }
    $script:WallThrow = $false
    Check 'M4：比對丟例外時 Restore 不再丟、Errors 帶「還原後比對失敗」' (@($r4.Errors | Where-Object { $_ -match '還原後比對失敗.*boom' }).Count -eq 1)
    Check 'M4：還原動作本身已完成（m4.txt 被清掉），Actions 有紀錄' ((-not (Test-Path (Join-Path $P.Install 'm4.txt'))) -and @($r4.Actions).Count -ge 10)

    # ── 6f. M7：受管目錄內有 reparse point → Save 拒絕、不建立任何東西 ───────────────────────
    $jt = Join-Path $tmp 'junction-target'
    Write-Text (Join-Path $jt 'f.txt') 'x'
    $jn = Join-Path $P.Install 'jn'
    New-Item -ItemType Junction -Path $jn -Target $jt | Out-Null
    $snapJ = Join-Path $tmp 'snap-junction'
    Check 'M7：受管目錄含 junction → Save 拒絕' (Test-Throws { Save-FcSnapshot -Dir $snapJ -Targets $targets -Ctx $ctx } 'reparse point')
    Check 'M7：拒絕時沒有建立快照目錄' (-not (Test-Path $snapJ))
    Check 'M7：Assert-SafeDeleteDir 拒絕 reparse point' (Test-Throws { Assert-SafeDeleteDir $jn } 'reparse point')
    [System.IO.Directory]::Delete($jn)   # 只移除連結本身，不動目標
    Check 'M7：移除連結後目標目錄內容仍在' (Test-Path (Join-Path $jt 'f.txt'))

    # ── 6g. I4：known folder 為準，環境變數不一致就拒絕 ─────────────────────────────────────
    $kf = @{ LocalApplicationData = 'C:\Users\u\AppData\Local'; ApplicationData = 'C:\Users\u\AppData\Roaming'; Programs = 'C:\Users\u\AppData\Roaming\Microsoft\Windows\Start Menu\Programs'; Desktop = 'D:\OneDrive\Desktop' }
    $getKf = { param($n) $kf[$n] }
    $okEnv = @{ LOCALAPPDATA = 'c:\users\u\appdata\local\'; APPDATA = 'C:\Users\u\AppData\Roaming' }   # 大小寫、結尾反斜線不同仍視為一致
    $rp = Resolve-FcProfilePaths -GetFolder $getKf -EnvVars $okEnv
    Check 'I4：環境變數與 known folder 一致（忽略大小寫與結尾反斜線）→ 取 known folder' ($rp.LocalAppData -eq 'C:\Users\u\AppData\Local' -and $rp.Desktop -eq 'D:\OneDrive\Desktop' -and $rp.Programs -like '*Start Menu\Programs')
    Check 'I4：環境變數未設定 → 照 known folder' ((Resolve-FcProfilePaths -GetFolder $getKf -EnvVars @{}).AppData -eq 'C:\Users\u\AppData\Roaming')
    Check 'I4：LOCALAPPDATA 被覆寫 → 拒絕' (Test-Throws { Resolve-FcProfilePaths -GetFolder $getKf -EnvVars @{ LOCALAPPDATA = 'C:\Temp\fake-local'; APPDATA = 'C:\Users\u\AppData\Roaming' } } '環境變數 LOCALAPPDATA.*不一致')
    Check 'I4：APPDATA 被覆寫 → 拒絕' (Test-Throws { Resolve-FcProfilePaths -GetFolder $getKf -EnvVars @{ LOCALAPPDATA = 'C:\Users\u\AppData\Local'; APPDATA = 'C:\Temp\fake-roaming' } } '環境變數 APPDATA.*不一致')
    $kf.Desktop = ''
    Check 'I4：known folder 取不到 → 拒絕' (Test-Throws { Resolve-FcProfilePaths -GetFolder $getKf -EnvVars @{} } 'Desktop 取不到有效路徑')
    $kf.Desktop = 'D:\OneDrive\Desktop'
    # 正式清單的內容（以假路徑組出）
    $rt = New-FcRealTargets $rp
    Check 'I4／I5：正式清單目錄路徑' ((@($rt.Dirs | ForEach-Object { $_.Path }) -join '|') -eq 'C:\Users\u\AppData\Local\fc-host|C:\Users\u\AppData\Roaming\tw.fintools.fc-host|C:\Users\u\AppData\Local\tw.fintools.fc-host') (@($rt.Dirs | ForEach-Object { $_.Path }) -join '|')
    Check 'I5：正式清單含兩個桌面捷徑（桌面取自 known folder）與兩個開始功能表捷徑' ((@($rt.Files | ForEach-Object { $_.Path }) -join '|') -eq 'C:\Users\u\AppData\Roaming\Microsoft\Windows\Start Menu\Programs\fc-host.lnk|C:\Users\u\AppData\Roaming\Microsoft\Windows\Start Menu\Programs\財經日曆.lnk|D:\OneDrive\Desktop\fc-host.lnk|D:\OneDrive\Desktop\財經日曆.lnk') (@($rt.Files | ForEach-Object { $_.Path }) -join '|')
    Check '正式清單：僅回報項＝Themes 下三種桌布轉存檔，且不在 Dirs／Files 內（不會被還原）' ($rt.Watch.Count -eq 1 -and $rt.Watch[0].Path -eq 'C:\Users\u\AppData\Roaming\Microsoft\Windows\Themes' -and (@($rt.Watch[0].Include) -join '|') -eq 'TranscodedWallpaper|Transcoded_*|TranscodedWallpaperCache\*' -and @($rt.Dirs | Where-Object { $_.Path -like '*Themes*' }).Count -eq 0)
    Check '正式清單：登錄鍵' ((@($rt.RegValues | ForEach-Object { "$($_.Key)|$($_.Name)" }) -join ';') -eq 'Software\Microsoft\Windows\CurrentVersion\Run|fc-host;Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run|fc-host;Control Panel\Desktop|Wallpaper;Control Panel\Desktop|WallpaperStyle;Control Panel\Desktop|TileWallpaper')
    Check '正式清單：子樹與父鍵' ((@($rt.RegTrees | ForEach-Object { $_.Key }) -join ';') -eq 'Software\Microsoft\Windows\CurrentVersion\Uninstall\fc-host;Software\fintools\fc-host' -and $rt.RegTrees[1].PruneParentKey -eq 'Software\fintools')

    # ── 6h. M3：哪些行程在跑時拒絕（純函式；另對真實行程表做一次唯讀查詢確認不丟例外）──────────
    $fake = @(
        [pscustomobject]@{ ProcessId = 1; Name = 'fc-host.exe'; ExecutablePath = 'C:\x\fc-host.exe'; CommandLine = '' }
        [pscustomobject]@{ ProcessId = 2; Name = 'msedgewebview2.exe'; ExecutablePath = 'C:\w\msedgewebview2.exe'; CommandLine = '--user-data-dir=C:\Users\u\AppData\Local\tw.fintools.fc-host\EBWebView' }
        [pscustomobject]@{ ProcessId = 3; Name = 'msedgewebview2.exe'; ExecutablePath = 'C:\w\msedgewebview2.exe'; CommandLine = '--user-data-dir=C:\Users\u\AppData\Local\OtherApp\EBWebView' }
        [pscustomobject]@{ ProcessId = 4; Name = 'finance-calendar-setup-arm64.exe'; ExecutablePath = 'C:\d\finance-calendar-setup-arm64.exe'; CommandLine = '' }
        [pscustomobject]@{ ProcessId = 5; Name = 'uninstall.exe'; ExecutablePath = 'C:\Users\u\AppData\Local\fc-host\uninstall.exe'; CommandLine = '' }
        [pscustomobject]@{ ProcessId = 6; Name = 'uninstall.exe'; ExecutablePath = 'C:\Program Files\Other\uninstall.exe'; CommandLine = '' }
        [pscustomobject]@{ ProcessId = 7; Name = 'notepad.exe'; ExecutablePath = 'C:\Windows\notepad.exe'; CommandLine = '' }
        [pscustomobject]@{ ProcessId = 8; Name = 'msedgewebview2.exe'; ExecutablePath = $null; CommandLine = $null }
        # N1：更新器啟動的安裝檔（正）與 NSIS 解除安裝複本（正）；名稱相近但不該擋的（反）
        [pscustomobject]@{ ProcessId = 9; Name = 'fc-host-0.1.1-installer.exe'; ExecutablePath = 'C:\Users\u\AppData\Local\Temp\fc-host-0.1.1-updater-AbC123\fc-host-0.1.1-installer.exe'; CommandLine = '/P /R' }
        [pscustomobject]@{ ProcessId = 10; Name = 'Un.exe'; ExecutablePath = 'C:\Users\u\AppData\Local\Temp\~nsu1A2B.tmp\Un.exe'; CommandLine = '_?=C:\Users\u\AppData\Local\fc-host\' }
        [pscustomobject]@{ ProcessId = 11; Name = 'Un.exe'; ExecutablePath = 'C:\Tools\Un.exe'; CommandLine = '' }
        [pscustomobject]@{ ProcessId = 12; Name = 'other-app-0.1.1-installer.exe'; ExecutablePath = 'C:\x\other-app-0.1.1-installer.exe'; CommandLine = '' }
        [pscustomobject]@{ ProcessId = 13; Name = 'fc-host-tool.exe'; ExecutablePath = 'C:\x\fc-host-tool.exe'; CommandLine = '' }
        [pscustomobject]@{ ProcessId = 14; Name = 'Un.exe'; ExecutablePath = $null; CommandLine = $null }
    )
    $hit = @(Select-FcBlockingProcesses $fake)
    Check 'M3／N1：宿主、宿主的 WebView2、安裝檔、更新器安裝檔、安裝目錄內的 uninstall.exe、~nsu*.tmp\Un.exe 會擋；別的 WebView2／別處的 uninstall／別處的 Un.exe／別 app 的 installer／fc-host-tool／notepad／無命令列或無路徑的行程不擋' (($hit -join ',') -eq 'fc-host.exe#1,msedgewebview2.exe#2,finance-calendar-setup-arm64.exe#4,uninstall.exe#5,fc-host-0.1.1-installer.exe#9,Un.exe#10') ($hit -join ',')
    Check 'M3：沒有任何行程 → 空陣列' (@(Select-FcBlockingProcesses @()).Count -eq 0)
    $realProbe = $null
    try { $realProbe = @(& (New-FcSnapshotContext).IsHostRunning) } catch { $realProbe = "ERR:$($_.Exception.Message)" }
    Check 'M3：對真實行程表做唯讀查詢不丟例外' ($realProbe -isnot [string]) ($realProbe -join ',')

    # ── 7. 桌布讀回 helper：真實 COM 唯讀一次（確認 vtable 宣告）──────────────────────────────
    $real = Get-FcWallpaperReadback
    if ($real.Ok) {
        Check 'IDesktopWallpaper 唯讀讀回成功且至少一台螢幕' (@($real.Monitors).Count -ge 1) ($real | ConvertTo-Json -Depth 4 -Compress)
        Check '讀回的螢幕有 Id 與矩形' ((@($real.Monitors | Where-Object { $_.Id -and $_.Right -gt $_.Left }).Count) -eq @($real.Monitors).Count)
    } else {
        Write-Host "SKIP  IDesktopWallpaper 讀回失敗（可能鎖定或無桌面）：$($real.Error)" -ForegroundColor Yellow
    }

    # ── 8. CLI 腳本：靜態與「不碰真實設定檔」的呼叫 ─────────────────────────────────────────
    $cli = Join-Path $toolsDir 'installer-snapshot.ps1'
    $tokens = $null; $errs = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseFile($cli, [ref]$tokens, [ref]$errs)
    Check 'CLI 腳本語法無錯' ($errs.Count -eq 0)
    $paramNames = @($ast.ParamBlock.Parameters | ForEach-Object { $_.Name.VariablePath.UserPath })
    Check 'CLI 只收 Command／Dir／WhatIf（不接受任何路徑覆寫參數）' (($paramNames -join ',') -eq 'Command,Dir,WhatIf') ($paramNames -join ',')
    $cliText = [System.IO.File]::ReadAllText($cli, [System.Text.Encoding]::UTF8)
    $modText = [System.IO.File]::ReadAllText((Join-Path $toolsDir 'lib\InstallerSnapshot.psm1'), [System.Text.Encoding]::UTF8)
    foreach ($lit in @(
            "'Software\Microsoft\Windows\CurrentVersion\Run'", "'Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run'",
            "'Software\Microsoft\Windows\CurrentVersion\Uninstall\fc-host'", "'Software\fintools\fc-host'", "'Control Panel\Desktop'",
            "'Wallpaper'", "'WallpaperStyle'", "'TileWallpaper'", "'fc-host'", "'tw.fintools.fc-host'", "'財經日曆.lnk'", "'fc-host.lnk'",
            "'Microsoft\Windows\Themes'", "'TranscodedWallpaper'", "'Transcoded_*'", "'TranscodedWallpaperCache\*'"
        )) {
        Check "正式清單寫死（New-FcRealTargets）：$lit" ($modText.Contains($lit))
    }
    Check 'CLI 路徑來自 Resolve-FcProfilePaths（known folder），不再直接讀環境變數' ($cliText.Contains('New-FcRealTargets (Resolve-FcProfilePaths)') -and -not ($cliText -match '\$env:(LOCALAPPDATA|APPDATA)'))
    Check 'CLI Save 在快照不完整時以非零結束碼結束' ($cliText -match 'if \(\$r\.Complete\) \{ exit 0 \}\s+exit 1')
    $pw = (Get-Process -Id $PID).Path
    $out = & $pw -NoProfile -NonInteractive -File $cli Restore -Dir (Join-Path $tmp 'no-such-snapshot') 2>&1
    Check 'CLI Restore 不存在的快照：結束碼 3、不動真實設定檔' ($LASTEXITCODE -eq 3) "（碼 $LASTEXITCODE：$out）"
    $out = & $pw -NoProfile -NonInteractive -File $cli Compare -Dir (Join-Path $tmp 'no-such-snapshot') 2>&1
    Check 'CLI Compare 不存在的快照：結束碼 3' ($LASTEXITCODE -eq 3) "（碼 $LASTEXITCODE：$out）"
    $out = & $pw -NoProfile -NonInteractive -File $cli Frobnicate -Dir $tmp 2>&1
    Check 'CLI 未知子命令：非零結束碼' ($LASTEXITCODE -ne 0)
    $out = & $pw -NoProfile -NonInteractive -File $cli Save 2>&1
    Check 'CLI 缺 -Dir：非零結束碼' ($LASTEXITCODE -ne 0)
    $whatIfDir = Join-Path $tmp 'whatif-snap'
    $out = & $pw -NoProfile -NonInteractive -File $cli Save -Dir $whatIfDir -WhatIf 2>&1
    $code = $LASTEXITCODE
    if ($code -eq 3) { Write-Host 'SKIP  CLI Save -WhatIf：本機有真實 fc-host 在跑（拒絕屬正常行為）' -ForegroundColor Yellow }
    else {
        Check 'CLI Save -WhatIf：結束碼 0、有 [WhatIf] 輸出、不建立快照目錄' ($code -eq 0 -and ($out -join "`n") -match '\[WhatIf\]' -and -not (Test-Path $whatIfDir)) "（碼 $code）"
    }
}
finally {
    if ($base.StartsWith("$testRoot\") -and $base.Length -gt $testRoot.Length + 5) { $hive.DeleteSubKeyTree($base, $false) }
    $tr = $hive.OpenSubKey($testRoot, $false)
    if ($tr) { $empty = ($tr.SubKeyCount -eq 0 -and $tr.ValueCount -eq 0); $tr.Close(); if ($empty) { $hive.DeleteSubKey($testRoot, $false) } }
    if ($tmp.StartsWith([System.IO.Path]::GetTempPath(), [System.StringComparison]::OrdinalIgnoreCase) -and (Split-Path $tmp -Leaf).StartsWith('fc-snap-test-')) {
        Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
    }
}

Write-Host ''
Write-Host "通過 $script:Pass、失敗 $script:Fail"
if ($script:Fail -gt 0) { exit 1 }
exit 0
