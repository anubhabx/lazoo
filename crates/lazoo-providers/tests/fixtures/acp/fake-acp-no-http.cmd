@echo off
if "%~1"=="--version" (
  echo 1.0.0-test
  exit /b 0
)
set "FAKE_ACP_NO_HTTP_MCP=1"
powershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "%~dp0fake-acp.ps1" %*
