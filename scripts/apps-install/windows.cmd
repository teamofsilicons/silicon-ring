@echo off
rem Silicon Apps runs this with cmd.exe after installing or updating the ring CLI on Windows x64,
rem from the package directory, with APPS_INSTALL_DIR set. The CLI is for silicons and is already
rem in place; install-desktop.ps1 adds the Ring desktop app for the people who use this account.
rem It never fails the CLI install.
powershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "%~dp0install-desktop.ps1"
exit /b 0
