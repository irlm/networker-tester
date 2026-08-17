@echo off
rem ── networker lab: Windows RUNNER first-boot hook ────────────────────────────
rem dockur/windows copies the /oem bind mount to C:\OEM and runs this file once,
rem at the end of the unattended Windows Server setup, as the auto-logged-in
rem local admin ("Docker", UAC disabled). Everything happens in
rem lab-runner-setup.ps1; this wrapper only makes sure its output survives even
rem if PowerShell dies before it can log (C:\lab\ + the host share
rem \\host.lan\Data = lab/.generated/windows-runner-K/shared/ on the host).
mkdir C:\lab 2>nul
powershell.exe -ExecutionPolicy Bypass -NoProfile -NonInteractive -File C:\OEM\lab-runner-setup.ps1 > C:\lab\lab-runner-setup.out 2>&1
copy /Y C:\lab\lab-runner-setup.out \\host.lan\Data\lab-runner-setup.out >nul 2>&1
exit /b 0
