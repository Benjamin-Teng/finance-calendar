# ==================== 財經日曆桌布 一鍵設定 (setup.ps1) ====================
# 由 setup.bat 以系統管理員身分呼叫。建立/更新資料更新排程，並實跑一次驗證鏡像到 Lively。
try { [Console]::OutputEncoding = [Text.Encoding]::UTF8 } catch {}
$ErrorActionPreference = 'Stop'
$TaskName = 'TW財經桌布資料更新'
$ProjDir  = Split-Path -Parent $PSScriptRoot
$Script   = Join-Path $ProjDir 'update_tw_events.py'

function Step($m){ Write-Host "`n>> $m" -ForegroundColor Cyan }
function Ok($m){   Write-Host "   [OK]   $m" -ForegroundColor Green }
function Info($m){ Write-Host "   [..]   $m" -ForegroundColor Gray }
function Warn($m){ Write-Host "   [WARN] $m" -ForegroundColor Yellow }
function Die($m){
  Write-Host "   [FAIL] $m" -ForegroundColor Red
  Write-Host "`n設定中止——請處理上面的問題後，再執行一次 setup.bat。" -ForegroundColor Red
  Read-Host "`n按 Enter 關閉"; exit 1
}
function Find-Python {
  $c = New-Object System.Collections.Generic.List[string]
  try { $p = & py -3 -c "import sys;print(sys.executable)" 2>$null; if ($p) { $c.Add(($p -replace 'python\.exe$','pythonw.exe')) } } catch {}
  foreach ($n in 'pythonw.exe','python.exe') {
    $g = Get-Command $n -EA SilentlyContinue
    if ($g -and $g.Source) { $c.Add(($g.Source -replace 'python\.exe$','pythonw.exe')) }
  }
  foreach ($gp in @("$env:LOCALAPPDATA\Programs\Python\*\pythonw.exe",
                    "$env:ProgramFiles\Python*\pythonw.exe",
                    "${env:ProgramFiles(x86)}\Python*\pythonw.exe")) {
    foreach ($g in (Get-Item $gp -EA SilentlyContinue)) { $c.Add($g.FullName) }
  }
  foreach ($x in $c) { if ($x -and (Test-Path $x)) { return (Get-Item $x).FullName } }
  return $null
}

Write-Host "========================================" -ForegroundColor Cyan
Write-Host "   財經日曆桌布 - 一鍵設定" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Info "專案資料夾：$ProjDir"

# --- 防呆 0：系統管理員 ---
Step "檢查系統管理員權限"
$admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $admin) { Die "未以系統管理員身分執行。請對 setup.bat 按右鍵 -> 以系統管理員身分執行。" }
Ok "系統管理員權限（使用者：$env:USERNAME）"

# --- 防呆 1：更新腳本存在 ---
Step "檢查更新腳本"
if (-not (Test-Path $Script)) { Die "找不到 update_tw_events.py（預期在 $Script）。請把 setup.bat / setup.ps1 放在專案資料夾內。" }
Ok "找到 update_tw_events.py"

# --- 防呆 2：偵測 Python（沒有就用 winget 自動安裝，winget 自動選架構）---
Step "偵測 Python（機器架構 $env:PROCESSOR_ARCHITECTURE）"
$pyw = Find-Python
if (-not $pyw) {
  Warn "未偵測到 Python，改用 winget 自動安裝（winget 會自動挑對 arm64/amd64）..."
  if (-not (Get-Command winget -EA SilentlyContinue)) {
    Die "找不到 Python，且系統無 winget 可自動安裝。請手動安裝 Python 3（python.org 或 Microsoft Store），再執行 setup.bat。"
  }
  try { & winget install -e --id Python.Python.3.12 --scope machine --silent --accept-package-agreements --accept-source-agreements | Out-Host }
  catch { Warn "winget 過程訊息：$($_.Exception.Message)" }
  Start-Sleep -Seconds 2
  $pyw = Find-Python
  if (-not $pyw) { Die "winget 安裝後仍偵測不到 Python。請重開 setup.bat 再試一次，或手動安裝 Python。" }
  Ok "已透過 winget 安裝 Python"
}
$pyexe = $pyw -replace 'pythonw\.exe$','python.exe'   # 驗證時用有主控台的 python.exe 才收得到輸出
Ok "Python：$pyw"

# --- 防呆 3：Store 版 Lively ---
Step "偵測 Lively Wallpaper（Store 版）"
$lively = Get-AppxPackage -Name '*LivelyWallpaper*' -EA SilentlyContinue
if (-not $lively) {
  Warn "未偵測到 Lively，改用 winget 從 Microsoft Store 自動安裝（rocksdanister 官方版）..."
  if (-not (Get-Command winget -EA SilentlyContinue)) {
    Die "未安裝 Lively，且系統無 winget 可自動安裝。請到 Microsoft Store 搜尋『Lively Wallpaper』(rocksdanister) 安裝，再執行 setup.bat。"
  }
  try { & winget install -e --id 9NTM2QC6QWS7 --source msstore --accept-package-agreements --accept-source-agreements | Out-Host }
  catch { Warn "winget 過程訊息：$($_.Exception.Message)" }
  Start-Sleep -Seconds 3
  $lively = Get-AppxPackage -Name '*LivelyWallpaper*' -EA SilentlyContinue
  if (-not $lively) { Die "Lively 自動安裝後仍偵測不到。請手動到 Microsoft Store 安裝『Lively Wallpaper』(rocksdanister)，再執行 setup.bat。" }
  Ok "已透過 winget（msstore）安裝 Lively"
}
Ok "Lively：$($lively.Name) $($lively.Version)"

# --- 防呆 4：桌布已在 Lively 設好（＝鏡像有目標）---
Step "確認桌布已在 Lively 設好（鏡像目標）"
$libGlob = Join-Path $env:LOCALAPPDATA 'Packages\12030rocksdanister.LivelyWallpaper_*\LocalCache\Local\Lively Wallpaper\Library'
$copies = @(Get-ChildItem -Path $libGlob -Recurse -Filter 'finance-calendar.html' -EA SilentlyContinue)
if ($copies.Count -eq 0) {
  Warn "Lively 裡找不到本桌布複製——代表還沒把它設成桌布。排程仍會建立；請之後在 Lively 設好桌布，資料才會自動更新到桌面。"
} else {
  Ok "Lively 已有本桌布複製 $($copies.Count) 份，鏡像目標就緒"
  if ($copies.Count -gt 1) { Warn "偵測到多份複製（多半是重匯殘留），資料會全部更新、無害。" }
}

# --- 建立/更新排程（Register-ScheduledTask，避開 XML UTF-16/BOM 坑）---
Step "建立/更新排程『$TaskName』"
$action = New-ScheduledTaskAction -Execute $pyw -Argument ('"{0}"' -f $Script)
$tDaily = New-ScheduledTaskTrigger -Daily -At '6:00AM'
$tDaily.Repetition = (New-ScheduledTaskTrigger -Once -At '6:00AM' -RepetitionInterval (New-TimeSpan -Hours 6) -RepetitionDuration (New-TimeSpan -Days 1)).Repetition
$tLogon = New-ScheduledTaskTrigger -AtLogOn
$tLogon.Delay = 'PT2M'
# 錯過的 6 小時排程恢復補跑；睡眠／休眠喚醒另由 Power-Troubleshooter Event ID 1 立即觸發。
# ScheduledTasks 的 Register-ScheduledTask 不接受 EventTrigger CIM 物件，故先建一般任務，
# 再以記憶體 XML 加上原生事件觸發器（不寫入 XML 檔，避開編碼問題）。
$settings = New-ScheduledTaskSettingsSet -StartWhenAvailable -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit (New-TimeSpan -Minutes 30)
$principal = New-ScheduledTaskPrincipal -UserId "$env:USERDOMAIN\$env:USERNAME" -LogonType Interactive -RunLevel Limited
try {
  Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger @($tDaily,$tLogon) -Settings $settings -Principal $principal -Force -EA Stop | Out-Null
  [xml]$taskXml = Export-ScheduledTask -TaskName $TaskName
  $ns = 'http://schemas.microsoft.com/windows/2004/02/mit/task'
  $nsmgr = New-Object System.Xml.XmlNamespaceManager($taskXml.NameTable)
  $nsmgr.AddNamespace('t', $ns)
  $triggers = $taskXml.SelectSingleNode('/t:Task/t:Triggers', $nsmgr)
  $wake = $taskXml.CreateElement('EventTrigger', $ns)
  $enabled = $taskXml.CreateElement('Enabled', $ns); $enabled.InnerText = 'true'
  $subscription = $taskXml.CreateElement('Subscription', $ns)
  $subscription.InnerText = '<QueryList><Query Id="0" Path="System"><Select Path="System">*[System[Provider[@Name="Microsoft-Windows-Power-Troubleshooter"] and EventID=1]]</Select></Query></QueryList>'
  [void]$wake.AppendChild($enabled)
  [void]$wake.AppendChild($subscription)
  [void]$triggers.AppendChild($wake)
  Register-ScheduledTask -TaskName $TaskName -Xml $taskXml.OuterXml -Force -EA Stop | Out-Null
} catch { Die "建立排程失敗：$($_.Exception.Message)" }
Ok "排程已建立（身分 $env:USERNAME／每 6 小時、登入後 2 分鐘、睡眠／休眠喚醒後立即執行；錯過的定時更新不補跑）"

# --- 實跑一次驗證（抓資料＋鏡像）---
Step "實跑一次資料更新（約 30-60 秒，抓資料並鏡像到 Lively）"
$out = & $pyexe $Script 2>&1
$exit = $LASTEXITCODE
# 逐行回放：真正「失敗」的行標黃，其餘灰（「下週檔尚未發布 404」屬正常、維持灰）
$out | ForEach-Object { if ("$_" -match '失敗') { Warn "$_" } else { Info "$_" } }
$done = ($out | Where-Object { $_ -match '完成：' } | Select-Object -Last 1)
$hit  = ($out | Where-Object { $_ -match 'Lively Wallpaper' })
if ($exit -ne 0) { Warn "更新腳本回傳碼 $exit（可能暫時網路問題；排程之後每 6 小時會自動重試）。" }
if ($done) {
  # 摘要行：完成訊息內含「警告／失敗」才算部分失敗（標黃），全乾淨才綠
  $summary = "資料更新完成 -> " + ($done -replace '^\s*完成：','')
  if ($done -match '警告|失敗') { Warn $summary } else { Ok $summary }
} else { Warn "未見『完成』訊息，請看上面的輸出。" }
if ($copies.Count -gt 0) {
  if ($hit) { Ok "已鏡像到 Lively 桌布資料夾（動態尋找成功，桌面約 1 分鐘內翻新）" }
  else      { Warn "這次沒看到鏡像到 Lively 的輸出——請確認桌布已在 Lively 設好。" }
  $fresh = $copies | ForEach-Object { $d = Join-Path $_.DirectoryName 'tw_events.js'; if (Test-Path $d) { (Get-Item $d).LastWriteTime } } |
           Sort-Object -Descending | Select-Object -First 1
  if ($fresh) { Ok "Lively 複製夾資料時間戳：$fresh" }
}

# --- 完成 ---
Write-Host "`n========================================" -ForegroundColor Cyan
Write-Host "   [完成] 設定成功" -ForegroundColor Green
Write-Host "   每 6 小時＋每次登入後，會自動更新資料並推送到 Lively 桌布。" -ForegroundColor Green
Write-Host "   （若上面有黃色 [WARN]，請照提示處理。）" -ForegroundColor Gray
Write-Host "========================================" -ForegroundColor Cyan
Read-Host "`n可以關閉了。按 Enter 關閉此視窗"
