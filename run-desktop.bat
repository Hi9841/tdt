@echo off
set "APP_DIR=%~dp0desktop"
cd /d "%APP_DIR%"
if not exist "target\release\voice-stt-desktop.exe" (
  echo Release binary not found. Run: cargo build --release
  exit /b 1
)
start "TDT" "target\release\voice-stt-desktop.exe"
