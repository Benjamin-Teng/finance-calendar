<#
.SYNOPSIS
    dynamic-wallpaper task 1.1／1.2 探針（probe_wallpaper）的驅動稿：前置檢查後，以 Start-Process
    在「獨立的新主控台視窗」啟動探針，脫離呼叫端（工具環境）的行程樹與管線。

.DESCRIPTION
    -Smoke  ：冒煙測試（預設基線 30 秒、設定 120 秒、冷卻 30 秒），證據檔名加 -smoke；預設會等探針結束。
    預設     ：完整連跑（基線 10 分鐘、設定 2 小時、冷卻 10 分鐘），證據檔名 -full（可用 -Tag 改）；
               啟動後立即返回（約 2 小時 20 分，超過工具的背景逾時上限，絕不能由工具前景／背景直接跑）。
    -Wait   ：不論模式都等探針結束並回傳其結束碼。
    -BaselineSecs／-SetSecs／-CooldownSecs：覆寫三段時長（例如縮短的冒煙測試）。

    為什麼要獨立主控台：探針會改使用者桌布，必須讓「還原」有機會執行。(1) 由工具環境直接前景／背景
    執行時，工具逾時會強殺行程樹，探針來不及還原；(2) 父行程死亡後 stdout 管線斷掉；(3) 工具環境的
    子行程常繼承「忽略 Ctrl+C」旗標。獨立的新主控台視窗（預設最小化）不受以上影響；探針啟動時也會
    自己清除繼承的忽略旗標，在該視窗內按 Ctrl+C 可中止並還原。stdout／stderr 另外導向檔案
    （-LogDir，預設 %LOCALAPPDATA%\fc-probe-wallpaper\console），證據記錄由探針自己寫進 -OutDir。

    前置檢查：工作階段未鎖定（LogonUI.exe 不在跑）、沒有另一個探針在跑、探針 exe 與兩張測試 PNG 存在。
    鎖定時以結束碼 3 中止（BLOCKED），不啟動探針。探針結束碼（-Wait 時原樣回傳）：0 成功；
    2 參數／環境錯誤；3 BLOCKED；4 還原讀回不一致；5 執行中出錯（已還原）。
    萬一異常結束後桌布沒還原：probe_wallpaper.exe --restore <備份 JSON>（路徑見 dw-1.1-<tag>.log
    的 BACKUP 行，或 %LOCALAPPDATA%\fc-probe-wallpaper\backup-*.json）。

.EXAMPLE
    cd host; cargo build --release --example probe_wallpaper
    bash host/tools/make-dw-probe-pngs.sh
    pwsh -File host/tools/probe-dw-1.1.ps1 -Smoke
    pwsh -File host/tools/probe-dw-1.1.ps1            # 完整連跑，啟動後立即返回
#>
[CmdletBinding()]
param(
    [switch]$Smoke,
    [switch]$Wait,
    [string]$Tag = '',
    [int]$BaselineSecs = -1,
    [int]$SetSecs = -1,
    [int]$CooldownSecs = -1,
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [string]$ExePath = (Join-Path $PSScriptRoot '..\target\release\examples\probe_wallpaper.exe'),
    [string]$ImageDir = (Join-Path $env:LOCALAPPDATA 'fc-probe-wallpaper'),
    [string]$LogDir = (Join-Path (Join-Path $env:LOCALAPPDATA 'fc-probe-wallpaper') 'console')
)

$ErrorActionPreference = 'Stop'

if (Get-Process -Name LogonUI -ErrorAction SilentlyContinue) {
    Write-Host 'BLOCKED：工作階段鎖定中（LogonUI.exe 在跑），不啟動探針。' -ForegroundColor Yellow
    exit 3
}
if (Get-Process -Name probe_wallpaper -ErrorAction SilentlyContinue) {
    Write-Host '已有另一個 probe_wallpaper 在跑，不重複啟動（兩個探針同時改桌布，備份會互相污染）。' -ForegroundColor Red
    exit 2
}
foreach ($p in @($ExePath, (Join-Path $ImageDir 'a.png'), (Join-Path $ImageDir 'b.png'))) {
    if (-not (Test-Path -LiteralPath $p)) {
        Write-Host "找不到 $p（先建置探針、並執行 bash host/tools/make-dw-probe-pngs.sh）" -ForegroundColor Red
        exit 2
    }
}

if ($Smoke) {
    if (-not $Tag) { $Tag = 'smoke' }
    $defaults = @(30, 120, 30)
    $Wait = $true
} else {
    if (-not $Tag) { $Tag = 'full' }
    $defaults = @(600, 7200, 600)
}
if ($BaselineSecs -lt 0) { $BaselineSecs = $defaults[0] }
if ($SetSecs -lt 0) { $SetSecs = $defaults[1] }
if ($CooldownSecs -lt 0) { $CooldownSecs = $defaults[2] }

$argv = @(
    '--baseline-secs', $BaselineSecs, '--set-secs', $SetSecs, '--cooldown-secs', $CooldownSecs,
    '--a', (Join-Path $ImageDir 'a.png'), '--b', (Join-Path $ImageDir 'b.png'),
    '--tag', $Tag, '--out-dir', $OutDir
) | ForEach-Object { $s = [string]$_; if ($s -match '\s') { '"' + $s + '"' } else { $s } }

New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
$stdout = Join-Path $LogDir "probe-$Tag.stdout.txt"
$stderr = Join-Path $LogDir "probe-$Tag.stderr.txt"

Write-Host "$ExePath $($argv -join ' ')"
# 不加 -NoNewWindow：Start-Process 預設為子行程開新的主控台視窗（最小化，不遮住桌面）。
$proc = Start-Process -FilePath $ExePath -ArgumentList $argv -PassThru -WindowStyle Minimized `
    -RedirectStandardOutput $stdout -RedirectStandardError $stderr
Write-Host "已啟動 probe_wallpaper PID=$($proc.Id)（獨立主控台視窗，已最小化；在該視窗按 Ctrl+C 可中止並還原）"
Write-Host "stdout：$stdout"
Write-Host "stderr：$stderr"
Write-Host "證據：$OutDir\dw-1.1-$Tag.csv／.log、dw-1.2-$Tag.log（追蹤：Get-Content $OutDir\dw-1.1-$Tag.log -Wait）"

if (-not $Wait) {
    Write-Host '不等待，立即返回。完整連跑約 2 小時 20 分；結束後查 dw-1.1-<tag>.log 的「RESTORE 讀回結論」與 DONE 行。'
    exit 0
}

$null = $proc.Handle   # 先取 handle，之後才讀得到 ExitCode
$proc.WaitForExit()
Write-Host "probe_wallpaper 結束，結束碼 $($proc.ExitCode)"
exit $proc.ExitCode
