@echo off
cd /d "%~dp0"
clipboard_share.exe stop
if errorlevel 1 (
  echo.
  echo Stop failed.
  pause
  exit /b 1
)
echo.
clipboard_share.exe status
echo.
echo Window will close in 3 seconds.
timeout /t 3 /nobreak >nul
