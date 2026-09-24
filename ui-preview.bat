@echo off
chcp 65001 >nul
title TerryComfy Launcher - Fast UI Preview
setlocal
cd /d "%~dp0"

echo.
echo   Fast UI preview - browser only, no Rust rebuild.
echo   URL: http://localhost:1420
echo   Edit files under frontend\src and the page reloads by itself.
echo   Press Ctrl+C here to stop.
echo.

powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\ui-preview.ps1"
if errorlevel 1 pause
endlocal
