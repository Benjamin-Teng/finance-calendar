<#
.SYNOPSIS
    dynamic-wallpaper task 1.7（GDI 累積受控重測）驅動稿：沿用 1.1 探針 probe_wallpaper，只對一台螢幕
    每 10 秒設定、每次設定後 5 秒取樣一列，CSV 追加閒置秒數與 explorer 頂層視窗數。

.DESCRIPTION
    預設（完整連跑）：基線 10 分鐘、設定 6 小時、冷卻 10 分鐘（約 6 小時 20 分），證據檔
    dw-1.1-1.7-full.{csv,log}、dw-1.2-1.7-full.log；啟動後立即返回。
    -Smoke：縮短的冒煙測試（預設基線 20 秒、設定 60 秒、冷卻 20 秒），tag 1.7-smoke，會等探針結束。
    -Wait  ：不論模式都等探針結束並回傳其結束碼。

    探針參數：--monitor <-Monitor>（預設 1＝本機主螢幕 3840×2160，序號＝GetMonitorDevicePathAt；
    log 的 MONITOR 行記下實際 rect，跑前以 --backup-only 確認）、--sample-after-set-secs 5、
    --extra-cols、--sample-secs 10（基線／冷卻的週期取樣，與設定階段每 10 秒一列的密度一致）。

    啟動方式與 probe-dw-1.1.ps1 的差別：**不做 stdout／stderr 導向**。Start-Process 一帶
    -RedirectStandardOutput／Error 就以 bInheritHandles=TRUE 建立行程，子行程會繼承呼叫端（工具）的
    管線，工具一直等到子行程結束、逾時強殺（README「啟動方式」與專案 memory
    start-process-redirect-inherits-caller-pipe）。探針自己把證據寫進 -OutDir，dw-1.1 log 也逐行寫檔，
    主控台輸出只是附帶，留在獨立的最小化主控台視窗裡（在該視窗按 Ctrl+C 可中止並還原）。

    前置檢查與結束碼同 probe-dw-1.1.ps1：鎖定（LogonUI.exe）→ 3；已有 probe_wallpaper 在跑、exe 或
    測試 PNG 不存在 → 2。探針結束碼：0 成功；2 參數／環境錯誤；3 BLOCKED；4 還原讀回不一致；
    5 執行中出錯（已還原）。異常結束沒還原時：probe_wallpaper.exe --restore <備份 JSON>（見 log 的
    BACKUP 行）。

.EXAMPLE
    cd host; cargo build --release --example probe_wallpaper
    pwsh -File host/tools/probe-dw-1.7.ps1 -Smoke
    pwsh -File host/tools/probe-dw-1.7.ps1             # 完整連跑，啟動後立即返回
#>
[CmdletBinding()]
param(
    [switch]$Smoke,
    [switch]$Wait,
    [string]$Tag = '',
    [int]$Monitor = 1,
    [int]$BaselineSecs = -1,
    [int]$SetSecs = -1,
    [int]$CooldownSecs = -1,
    [int]$IntervalSecs = 10,
    [int]$SampleAfterSetSecs = 5,
    [int]$SampleSecs = 10,
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [string]$ExePath = (Join-Path $PSScriptRoot '..\target\release\examples\probe_wallpaper.exe'),
    [string]$ImageDir = (Join-Path $env:LOCALAPPDATA 'fc-probe-wallpaper')
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
    if (-not $Tag) { $Tag = '1.7-smoke' }
    $defaults = @(20, 60, 20)
    $Wait = $true
} else {
    if (-not $Tag) { $Tag = '1.7-full' }
    $defaults = @(600, 21600, 600)
}
if ($BaselineSecs -lt 0) { $BaselineSecs = $defaults[0] }
if ($SetSecs -lt 0) { $SetSecs = $defaults[1] }
if ($CooldownSecs -lt 0) { $CooldownSecs = $defaults[2] }

$argv = @(
    '--baseline-secs', $BaselineSecs, '--set-secs', $SetSecs, '--cooldown-secs', $CooldownSecs,
    '--interval-secs', $IntervalSecs, '--sample-secs', $SampleSecs,
    '--monitor', $Monitor, '--sample-after-set-secs', $SampleAfterSetSecs, '--extra-cols',
    '--a', (Join-Path $ImageDir 'a.png'), '--b', (Join-Path $ImageDir 'b.png'),
    '--tag', $Tag, '--out-dir', $OutDir
) | ForEach-Object { $s = [string]$_; if ($s -match '\s') { '"' + $s + '"' } else { $s } }

Write-Host "$ExePath $($argv -join ' ')"
# 不加 -NoNewWindow、不做導向：新的獨立主控台視窗（最小化），不繼承呼叫端管線。
$proc = Start-Process -FilePath $ExePath -ArgumentList $argv -PassThru -WindowStyle Minimized
$total = $BaselineSecs + $SetSecs + $CooldownSecs
Write-Host "已啟動 probe_wallpaper PID=$($proc.Id)（獨立主控台視窗，已最小化；在該視窗按 Ctrl+C 可中止並還原）"
Write-Host "預計結束：$((Get-Date).AddSeconds($total + 10).ToString('yyyy-MM-dd HH:mm:ss'))（三段共 $total 秒，另加還原數秒）"
Write-Host "證據：$OutDir\dw-1.1-$Tag.csv／.log、dw-1.2-$Tag.log"

if (-not $Wait) {
    Write-Host '不等待，立即返回。結束後查 dw-1.1-<tag>.log 的「RESTORE 讀回結論」與 DONE 行。'
    exit 0
}

$null = $proc.Handle   # 先取 handle，之後才讀得到 ExitCode
$proc.WaitForExit()
Write-Host "probe_wallpaper 結束，結束碼 $($proc.ExitCode)"
exit $proc.ExitCode
