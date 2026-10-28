@echo off
rem A stand-in OpenCode executable for daemon tests: it answers --version and the ACP
rem initialize handshake, which is all the readiness probe exercises. It never runs a model.
if "%~1"=="--version" (
  echo 0.0.0-smoke-test
  exit /b 0
)
powershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "%~dp0opencode-probe.ps1" %*
