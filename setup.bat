@echo off
rem === Finance Calendar Wallpaper - one-click setup ===
rem Elevates to admin (UAC), then runs setup.ps1 (all logic + Chinese output).
net session >nul 2>&1 && goto :run
echo Requesting administrator privileges (UAC)...
powershell -NoProfile -Command "Start-Process -FilePath '%~f0' -Verb RunAs"
exit /b
:run
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\setup.ps1"
