@echo off
rem === Finance Calendar Wallpaper - uninstall ===
rem Elevates to admin (UAC), then runs scripts/uninstall.ps1.
net session >nul 2>&1 && goto :run
echo Requesting administrator privileges (UAC)...
powershell -NoProfile -Command "Start-Process -FilePath '%~f0' -Verb RunAs"
exit /b
:run
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\uninstall.ps1"
