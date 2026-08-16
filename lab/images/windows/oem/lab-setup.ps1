# ── networker lab: Windows target setup (runs INSIDE the Windows VM) ─────────
# Invoked once by C:\OEM\install.bat at the end of the unattended Windows Server
# install (dockur/windows), as the auto-logged-in local admin. Re-runnable over
# SSH (idempotent) — `lab.sh windows-ssh` then `powershell -File C:\OEM\lab-setup.ps1`.
#
#   1. lab plumbing: fixed admin password, OpenSSH server (+ PowerShell as the
#      SSH shell), firewall for the endpoint/stack ports.
#   2. THE REAL INSTALLER: C:\OEM\install.ps1 (the checkout's install.ps1,
#      copied in via /oem) `-Yes -Component endpoint` → downloads the released
#      networker-endpoint.exe (Windows binaries can't be cross-built on the
#      Linux host; the VM has NAT internet), then starts it the way the cloud
#      Windows bootstraps do (hidden process + schtasks ONSTART as SYSTEM).
#   3. `install.ps1 -Setup iis` → IIS 8082/8445 + HTTP/3 (http.sys) + ARR
#      reverse proxy to the endpoint. (nginx on Windows: install.ps1 says
#      unsupported — see Invoke-HttpStackSetup — so no nginx target here.)
#   4. Reboot when the installer says http.sys needs it (HTTP/3 registry).
#
# Progress: C:\lab\lab-setup.log + C:\lab\status, mirrored to \\host.lan\Data
# (= lab/.generated/windows/shared/ on the host → `lab.sh windows-log`).
# Status words the host polls: installing | endpoint | iis | rebooting | ready | failed:<phase>
$ErrorActionPreference = 'Continue'
$ProgressPreference    = 'SilentlyContinue'
try { [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor 3072 } catch { }

# ── settings (lab.sh generates C:\OEM\lab.env.ps1; defaults below match lab.sh) ──
$LabUser     = "Docker"
$LabPassword = "LabWindows-Pass1!"
$LabStacks   = "iis"
if (Test-Path "C:\OEM\lab.env.ps1") { . "C:\OEM\lab.env.ps1" }

$LabDir   = "C:\lab"
$LogFile  = "$LabDir\lab-setup.log"
$Share    = "\\host.lan\Data"
New-Item -ItemType Directory -Force $LabDir | Out-Null

function Log($m) {
    $line = "$(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')  $m"
    Write-Host $line
    Add-Content -Path $LogFile -Value $line
    try { Add-Content -Path "$Share\lab-setup.log" -Value $line -ErrorAction Stop } catch { }
}
function Set-LabStatus($s) {
    Set-Content -Path "$LabDir\status" -Value $s
    try { Set-Content -Path "$Share\status" -Value $s -ErrorAction Stop } catch { }
    Log "STATUS $s"
}
function Run-Logged($label, [scriptblock]$block) {
    # Runs $block, streaming every output line into the log (child processes included).
    Log ">> $label"
    try { & $block 2>&1 | ForEach-Object { Log ("   " + ($_ | Out-String).TrimEnd()) } }
    catch { Log "   EXCEPTION: $_" }
}
function Wait-Http($url, $seconds) {
    $deadline = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $deadline) {
        try {
            $r = Invoke-WebRequest -Uri $url -UseBasicParsing -TimeoutSec 5
            if ($r.StatusCode -eq 200) { return $true }
        } catch { }
        Start-Sleep -Seconds 2
    }
    return $false
}

Log "=== lab-setup start: user=$env:USERNAME host=$env:COMPUTERNAME os=$([Environment]::OSVersion.VersionString) stacks=$LabStacks"
Set-LabStatus "installing"

# ── 1. lab plumbing ──────────────────────────────────────────────────────────
try {
    $pw = ConvertTo-SecureString $LabPassword -AsPlainText -Force
    Set-LocalUser -Name $LabUser -Password $pw -PasswordNeverExpires $true
    Log "password set for $LabUser"
} catch { Log "WARN set password failed: $_" }

Run-Logged "OpenSSH server" {
    $cap = Get-WindowsCapability -Online -Name 'OpenSSH.Server*' | Select-Object -First 1
    if ($cap -and $cap.State -ne 'Installed') { Add-WindowsCapability -Online -Name $cap.Name | Out-Null }
    Set-Service sshd -StartupType Automatic
    Start-Service sshd
    New-ItemProperty -Path 'HKLM:\SOFTWARE\OpenSSH' -Name DefaultShell `
        -Value 'C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe' -PropertyType String -Force | Out-Null
    "sshd: $((Get-Service sshd).Status)"
}
Run-Logged "firewall" {
    foreach ($r in @(
        @{ n='Lab-SSH';           p='TCP'; ports='22' },
        @{ n='Lab-Networker-TCP'; p='TCP'; ports='8080-8082,8443-8445' },
        @{ n='Lab-Networker-UDP'; p='UDP'; ports='8080-8082,8443-8445,9997-9999' })) {
        if (-not (Get-NetFirewallRule -DisplayName $r.n -ErrorAction SilentlyContinue)) {
            New-NetFirewallRule -DisplayName $r.n -Direction Inbound -Protocol $r.p -LocalPort ($r.ports -split ',') -Action Allow | Out-Null
        }
    }
    "rules present"
}

# ── 2. the real installer: endpoint ──────────────────────────────────────────
Set-LabStatus "endpoint"
$installer = "C:\OEM\install.ps1"
if (-not (Test-Path $installer)) { Set-LabStatus "failed:no-installer"; Log "C:\OEM\install.ps1 missing"; exit 1 }
Run-Logged "install.ps1 -Yes -Component endpoint" {
    & powershell.exe -ExecutionPolicy Bypass -NoProfile -NonInteractive -File $installer -Yes -Component endpoint
    "exit=$LASTEXITCODE"
}
$exe = Join-Path $env:USERPROFILE ".cargo\bin\networker-endpoint.exe"
if (-not (Test-Path $exe)) { Set-LabStatus "failed:endpoint-install"; Log "networker-endpoint.exe not found at $exe"; exit 1 }
Log "endpoint binary: $exe ($((& $exe --version 2>&1) -join ' '))"

# Start it exactly like the cloud Windows bootstraps (install.sh
# _azure_win_create_endpoint_service / AWS user-data): hidden process now +
# a SYSTEM ONSTART scheduled task for reboot persistence.
Run-Logged "start networker-endpoint" {
    Stop-Process -Name 'networker-endpoint' -Force -ErrorAction SilentlyContinue
    schtasks /Create /TN 'NetworkerEndpoint' /TR "`"$exe`"" /SC ONSTART /RU SYSTEM /F 2>&1
    Start-Process -FilePath $exe -WindowStyle Hidden -WorkingDirectory $LabDir
}
if (Wait-Http "http://127.0.0.1:8080/health" 60) { Log "endpoint healthy on :8080" }
else { Set-LabStatus "failed:endpoint-start"; Log "endpoint not answering on :8080"; exit 1 }

# ── 3. the real installer: HTTP stacks (-Setup <stack>) ──────────────────────
$rebootNeeded = $false
foreach ($stack in ($LabStacks -split ',' | ForEach-Object { $_.Trim() } | Where-Object { $_ })) {
    Set-LabStatus $stack
    $out = @()
    Run-Logged "install.ps1 -Setup $stack" {
        $script:out = & powershell.exe -ExecutionPolicy Bypass -NoProfile -NonInteractive -File $installer -Setup $stack 2>&1
        $script:out
        "exit=$LASTEXITCODE"
    }
    if (($script:out | Out-String) -match 'REBOOT_NEEDED') { $rebootNeeded = $true }
    switch ($stack) {
        'iis' {
            if (Wait-Http "http://127.0.0.1:8082/health" 60) { Log "IIS healthy on :8082" } else { Log "WARN IIS :8082 not answering" }
            # 8445 is TLS with a self-signed cert: probe with cert validation off (PS 5.1 style).
            try {
                [Net.ServicePointManager]::ServerCertificateValidationCallback = { $true }
                $r = Invoke-WebRequest -Uri "https://localhost:8445/health" -UseBasicParsing -TimeoutSec 10
                Log "IIS https :8445 -> $($r.StatusCode) alt-svc=$($r.Headers['alt-svc'])"
            } catch { Log "WARN IIS https :8445 probe failed: $_" }
            finally { [Net.ServicePointManager]::ServerCertificateValidationCallback = $null }
        }
    }
}

# ── 4. reboot for http.sys HTTP/3, or done ───────────────────────────────────
Log "=== lab-setup done (rebootNeeded=$rebootNeeded)"
if ($rebootNeeded) {
    Set-LabStatus "rebooting"
    Log "rebooting so http.sys picks up EnableHttp3 (endpoint returns via the ONSTART task, IIS auto-starts)"
    # One-shot post-boot checker flips the status to "ready" once :8080/:8082 answer again.
    schtasks /Create /TN 'NetworkerLabPostBoot' /SC ONSTART /RU SYSTEM /F `
        /TR "powershell.exe -ExecutionPolicy Bypass -NoProfile -NonInteractive -File C:\OEM\lab-postboot.ps1" 2>&1 | Out-Null
    Start-Sleep -Seconds 3
    Restart-Computer -Force
} else {
    Set-LabStatus "ready"
}
