# ==================== 財經日曆桌布 移除 (uninstall.ps1) ====================
# 由 uninstall.bat 以系統管理員身分呼叫。刪除資料更新排程；Lively／Python 是否移除由使用者決定。
try { [Console]::OutputEncoding = [Text.Encoding]::UTF8 } catch {}
$ErrorActionPreference = 'Stop'
$TaskName = 'TW財經桌布資料更新'

function Step($m){ Write-Host "`n>> $m" -ForegroundColor Cyan }
function Ok($m){   Write-Host "   [OK]   $m" -ForegroundColor Green }
function Info($m){ Write-Host "   [..]   $m" -ForegroundColor Gray }
function Warn($m){ Write-Host "   [WARN] $m" -ForegroundColor Yellow }
function Ask($q){ $a = Read-Host "   $q [y/N]"; return ($a -match '^\s*[Yy]') }

Write-Host "========================================" -ForegroundColor Cyan
Write-Host "   財經日曆桌布 - 移除" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan

# 系統管理員
$admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $admin) {
  Write-Host "   [FAIL] 未以系統管理員身分執行。請對 uninstall.bat 按右鍵 -> 以系統管理員身分執行。" -ForegroundColor Red
  Read-Host "`n按 Enter 關閉"; exit 1
}

# 1. 刪除排程（一定執行）
Step "移除資料更新排程"
$t = Get-ScheduledTask -TaskName $TaskName -EA SilentlyContinue
if ($t) {
  try { Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -EA Stop; Ok "已刪除排程『$TaskName』" }
  catch { Warn "刪除排程失敗：$($_.Exception.Message)" }
} else { Info "排程不存在（已刪除或從未建立）" }

# 2. 詢問是否移除 Lively（預設 N＝保留）
Step "Lively Wallpaper"
$lively = Get-AppxPackage -Name '*LivelyWallpaper*' -EA SilentlyContinue
if ($lively) {
  Warn "移除 Lively 後，桌面桌布也會一起消失。"
  if (Ask "要一併移除 Lively Wallpaper 嗎？（預設不移除）") {
    try { $lively | Remove-AppxPackage -EA Stop; Ok "已移除 Lively Wallpaper" }
    catch { Warn "移除 Lively 失敗：$($_.Exception.Message)（可改用 winget uninstall --id 9NTM2QC6QWS7）" }
  } else { Info "保留 Lively Wallpaper" }
} else { Info "未偵測到 Lively（未安裝或已移除）" }

# 3. 詢問是否移除 Python（預設 N＝保留；其他程式可能也在用）
Step "Python"
Warn "其他程式可能也在用 Python，通常建議保留。"
if (Ask "要一併移除 Python 嗎？（預設不移除；僅移除 winget 裝的 Python.Python.3.12）") {
  if (Get-Command winget -EA SilentlyContinue) {
    try { & winget uninstall -e --id Python.Python.3.12 --silent | Out-Host; Ok "已嘗試移除 Python 3.12（非 winget 安裝者會略過）" }
    catch { Warn "移除 Python 失敗：$($_.Exception.Message)" }
  } else { Warn "系統無 winget，無法自動移除；請自行到「設定 -> 應用程式」移除 Python。" }
} else { Info "保留 Python" }

# 收尾提醒
if (Get-AppxPackage -Name '*LivelyWallpaper*' -EA SilentlyContinue) {
  Info "若保留了 Lively，可自行在 Lively 裡把 finance-calendar 這張桌布移除。"
}

Write-Host "`n========================================" -ForegroundColor Cyan
Write-Host "   [完成] 移除完成 — 資料更新排程已刪除。" -ForegroundColor Green
Write-Host "   本專案資料夾可直接手動刪除。" -ForegroundColor Gray
Write-Host "========================================" -ForegroundColor Cyan
Read-Host "`n可以關閉了。按 Enter 關閉此視窗"
