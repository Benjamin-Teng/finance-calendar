# data-layer-rust 5.3 實跑：隔離環境（LOCALAPPDATA／APPDATA 指向暫存）下 data_fetch 預設 auto 不抓取。
# 只結束本腳本啟動的 PID；不注入輸入、不改桌布、不碰真實設定與 HKCU Run。
param([int]$WaitSeconds = 200, [string]$Fixture = '', [switch]$Empty, [string]$Tag = 'isolated-smoke')
$ErrorActionPreference = 'Stop'
$repo = 'D:\projects\finance-calendar-data-layer-rust'
$exe = Join-Path $repo 'host\target\release\fc-host.exe'
if (-not $Fixture) { $Fixture = Join-Path $repo 'host\ui\fixtures\tw-events.json' }

if (Get-Process LogonUI -ErrorAction SilentlyContinue) { Write-Output 'BLOCKED: 工作階段鎖定中'; exit 2 }
$running = Get-Process fc-host -ErrorAction SilentlyContinue
if ($running) { Write-Output "BLOCKED: 已有 fc-host 在跑（$($running.Id -join ',')），單一執行個體會讓測試宿主退出"; exit 2 }

$root = Join-Path $env:TEMP ("fc-smoke-" + (Get-Date -Format 'yyyyMMdd-HHmmss'))
$local = Join-Path $root 'Local'; $roaming = Join-Path $root 'Roaming'
$dataDir = Join-Path $local 'tw.fintools.fc-host\data'
New-Item -ItemType Directory -Force $dataDir, $roaming | Out-Null
$target = Join-Path $dataDir 'tw_events.json'
if (-not $Empty) { Copy-Item $Fixture $target }
$before = if ($Empty) { @{ hash = ''; mtime = '' } } else { @{ hash = (Get-FileHash $target).Hash; mtime = (Get-Item $target).LastWriteTimeUtc } }

$runBefore = (Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -ErrorAction SilentlyContinue).'fc-host'

$psi = New-Object System.Diagnostics.ProcessStartInfo $exe
$psi.UseShellExecute = $false
$psi.EnvironmentVariables['LOCALAPPDATA'] = $local
$psi.EnvironmentVariables['APPDATA'] = $roaming
$p = [System.Diagnostics.Process]::Start($psi)
Write-Output "started pid=$($p.Id) root=$root"
Start-Sleep -Seconds $WaitSeconds

$alive = -not $p.HasExited
$after = if (Test-Path $target) { @{ hash = (Get-FileHash $target).Hash; mtime = (Get-Item $target).LastWriteTimeUtc } } else { @{ hash = ''; mtime = '' } }
$state = Test-Path (Join-Path $local 'tw.fintools.fc-host\fetch-state.json')
$logDir = Join-Path $local 'tw.fintools.fc-host\logs'
$logHits = if (Test-Path $logDir) { Select-String -Path (Join-Path $logDir '*') -Pattern '隔離|data_fetch|fc_host::fetch' -SimpleMatch:$false | ForEach-Object { $_.Line } } else { @() }
$others = Get-ChildItem $dataDir | Select-Object -ExpandProperty Name

if ($alive) { Stop-Process -Id $p.Id; $p.WaitForExit(15000) | Out-Null }
Start-Sleep -Seconds 3
$leftover = Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -like "*$root*" } | Select-Object -ExpandProperty ProcessId
$runAfter = (Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -ErrorAction SilentlyContinue).'fc-host'

$evDir = Join-Path $repo 'host\tools\evidence'
$ev = Join-Path $evDir "data-layer-5.3-$Tag.log"
@(
  "time=$(Get-Date -Format o) wait=${WaitSeconds}s exe_mtime=$((Get-Item $exe).LastWriteTime.ToString('o'))"
  "alive_at_check=$alive"
  "fixture_hash_unchanged=$($before.hash -eq $after.hash) mtime_unchanged=$($before.mtime -eq $after.mtime)"
  "fetch_state_created=$state"
  "data_dir_files=$($others -join ',')"
  "hkcu_run_unchanged=$($runBefore -eq $runAfter)"
  "leftover_pids=$($leftover -join ',')"
  'log_lines:'
) + ($logHits | Select-Object -First 40) | Set-Content -Encoding utf8 $ev
Get-Content $ev
if ($logDir -and (Test-Path $logDir)) { Copy-Item (Join-Path $logDir '*') (Join-Path $evDir "data-layer-5.3-$Tag-hostlog.txt") -ErrorAction SilentlyContinue }
Remove-Item -Recurse -Force $root -ErrorAction SilentlyContinue
