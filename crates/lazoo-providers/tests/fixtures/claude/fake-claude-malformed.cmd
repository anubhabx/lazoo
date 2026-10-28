@echo off
set "FAKE_CLAUDE_AUTH_MODE=malformed"
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0fake-claude.ps1" %*
