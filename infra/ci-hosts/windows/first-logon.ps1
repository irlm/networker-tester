# infra/ci-hosts/windows/first-logon.ps1 — runs ONCE at the first Administrator
# logon of a freshly installed Windows Server CI host (FirstLogonCommands in
# autounattend.xml.tmpl; this file sits on the answer ISO next to it).
#
# Makes the machine reachable and manageable, nothing more:
#   1. OpenSSH Server, PowerShell as the ssh shell, firewall rule on 22
#   2. the operator's public key (authorized_keys on the answer ISO) for the
#      Administrators group (administrators_authorized_keys, SYSTEM+Admins ACL)
#   3. QEMU guest agent from the virtio-win ISO (Proxmox learns the IP)
#   4. auto-logon off, completion marker
# The GitHub runner and the toolchains are installed afterwards over ssh by
# install-ci-host.ps1 — the PAT is never on this ISO.
Start-Transcript -Path 'C:\first-logon.log' -Append

# Where am I being run from? (the answer ISO's drive letter)
$isoRoot = Split-Path -Parent $MyInvocation.MyCommand.Path

Write-Host '=== OpenSSH Server ===' -ForegroundColor Cyan
Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0 | Out-Null
Set-Service -Name sshd -StartupType Automatic
Start-Service sshd
New-ItemProperty -Path 'HKLM:\SOFTWARE\OpenSSH' -Name DefaultShell `
    -Value 'C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe' -PropertyType String -Force | Out-Null
if (-not (Get-NetFirewallRule -Name 'OpenSSH-Server' -ErrorAction SilentlyContinue)) {
    New-NetFirewallRule -Name 'OpenSSH-Server' -DisplayName 'OpenSSH Server (sshd)' `
        -Enabled True -Direction Inbound -Protocol TCP -Action Allow -LocalPort 22 | Out-Null
}

Write-Host '=== operator ssh key for Administrators ===' -ForegroundColor Cyan
$keys = Join-Path $isoRoot 'authorized_keys'
if (Test-Path $keys) {
    $dst = 'C:\ProgramData\ssh\administrators_authorized_keys'
    Copy-Item $keys $dst -Force
    # sshd refuses the file unless only SYSTEM + Administrators can touch it.
    icacls $dst /inheritance:r /grant 'SYSTEM:F' /grant 'BUILTIN\Administrators:F' | Out-Null
    Restart-Service sshd
    Write-Host "  installed $(Get-Content $keys | Measure-Object -Line | Select-Object -ExpandProperty Lines) key(s)"
} else {
    Write-Host '  WARNING: no authorized_keys on the answer ISO — password login only' -ForegroundColor Yellow
}

Write-Host '=== QEMU guest agent (virtio-win ISO) ===' -ForegroundColor Cyan
$msi = Get-ChildItem -Path (Get-PSDrive -PSProvider FileSystem | ForEach-Object { "$($_.Root)guest-agent" }) `
    -Filter 'qemu-ga-x86_64.msi' -ErrorAction SilentlyContinue | Select-Object -First 1
if ($msi) {
    Start-Process msiexec.exe -ArgumentList "/i `"$($msi.FullName)`" /qn /norestart" -Wait
    Write-Host '  installed'
} else {
    Write-Host '  WARNING: qemu-ga-x86_64.msi not found on any CD — attach virtio-win.iso' -ForegroundColor Yellow
}

Write-Host '=== auto-logon off ===' -ForegroundColor Cyan
Set-ItemProperty -Path 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon' -Name AutoAdminLogon -Value '0'
Remove-ItemProperty -Path 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon' -Name DefaultPassword -ErrorAction SilentlyContinue

New-Item -ItemType File -Path 'C:\ci-host-first-logon-complete' -Force | Out-Null
Write-Host '=== first logon complete — install-ci-host.ps1 runs over ssh next ===' -ForegroundColor Green
Stop-Transcript
