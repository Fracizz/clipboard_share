@echo off
cd /d "%~dp0"
clipboard_share.exe start
if errorlevel 1 (
  echo.
  echo Start failed. Check config.json and data\logs
  pause
  exit /b 1
)
echo.
clipboard_share.exe status
echo.
echo Window will close in 5 seconds. Background process keeps running.
timeout /t 5 /nobreak >nul
