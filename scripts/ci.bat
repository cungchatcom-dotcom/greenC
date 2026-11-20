@echo off
pwsh -NoProfile -ExecutionPolicy Bypass -File "%~dp0ci.ps1"
if %ERRORLEVEL% NEQ 0 (
    echo Quality gate failed with exit code %ERRORLEVEL%
    exit /b %ERRORLEVEL%
)
