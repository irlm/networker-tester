@echo off
rem ── networker lab: Windows target first-boot hook ────────────────────────────
rem dockur/windows copies the /oem bind mount to C:\OEM and runs this file once,
rem at the end of the unattended Windows Server setup, as the auto-logged-in
rem local admin ("Docker", UAC disabled). Everything happens in lab-setup.ps1;
rem this wrapper only makes sure its output survives even if PowerShell dies
rem before it can log (C:\lab\ + the host share \\host.lan\Data = the
rem lab/.generated/windows/shared/ folder on the host).
mkdir C:\lab 2>nul
powershell.exe -ExecutionPolicy Bypass -NoProfile -NonInteractive -File C:\OEM\lab-setup.ps1 > C:\lab\lab-setup.out 2>&1
copy /Y C:\lab\lab-setup.out \\host.lan\Data\lab-setup.out >nul 2>&1
exit /b 0
