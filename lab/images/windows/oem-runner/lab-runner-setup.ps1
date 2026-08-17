# -- networker lab: Windows RUNNER setup (runs INSIDE the Windows VM) ---------
# Invoked once by C:\OEM\install.bat at the end of the unattended Windows Server
# install (dockur/windows), as the auto-logged-in local admin. Re-runnable over
# SSH (idempotent) - `lab.sh windows-ssh runner-K` then
# `powershell -File C:\OEM\lab-runner-setup.ps1`.
#
#   1. lab plumbing: fixed admin password, OpenSSH server (+ PowerShell as the
#      SSH shell), firewall for SSH.
#   2. THE REAL INSTALLER: C:\OEM\install.ps1 (the checkout's install.ps1,
#      copied in via /oem) `-Yes -Component tester` -> downloads the RELEASED
#      networker-tester.exe (Windows binaries can't be cross-built on the Linux
#      host; the VM has NAT internet) + the VC++ runtime it needs. Copied to
#      C:\networker\networker-tester.exe (what AGENT_TESTER_PATH points at).
#   3. THE CHECKOUT'S AGENT: C:\OEM\agent\ (Networker.Agent published win-x64
#      self-contained by lab.sh from this checkout) -> C:\networker\agent\.
#   4. Run it exactly like a cloud Windows tester VM after its bootstrap: the
#      AGENT_* env contract (AGENT_DASHBOARD_URL, AGENT_API_KEY, AGENT_NAME,
#      AGENT_TESTER_PATH - machine env vars + a wrapper .cmd, because env vars
#      don't ride schtasks /TR), persisted as a SYSTEM ONSTART scheduled task
#      (`schtasks /SC ONSTART`, the project's Windows persistence shape -
#      install.ps1's language servers / the endpoint use the same; the agent is
#      not a Windows service: no UseWindowsService, sc.exe would time out) and
#      started now through that same task, so first boot == every later boot.
#
# Progress: C:\lab\lab-runner-setup.log + C:\lab\status, mirrored to
# \\host.lan\Data (= lab/.generated/windows-runner-K/shared/ on the host ->
# `lab.sh windows-log runner-K`). Status words the host polls:
# installing | tester | agent | ready | failed:<phase>. The agent's own log is
# C:\lab\agent.log (Get-Content over `lab.sh windows-ssh runner-K`).
# ASCII ONLY in this file: Windows PowerShell 5.1 reads a BOM-less .ps1 as ANSI,
# so a non-ASCII char inside a string (an em dash!) breaks the parse of the whole
# script (seen 2026-08-16: no SSH, no agent, nothing). lab.sh checks at stage time.
$ErrorActionPreference = 'Continue'
$ProgressPreference    = 'SilentlyContinue'
try { [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor 3072 } catch { }

# -- settings (lab.sh generates C:\OEM\lab.env.ps1; defaults below match lab.sh) --
$LabUser           = "Docker"
$LabPassword       = "LabWindows-Pass1!"
$AgentDashboardUrl = "ws://172.31.100.10:5030/ws/agent"
$AgentApiKey       = ""
$AgentName         = "runner-windows"
if (Test-Path "C:\OEM\lab.env.ps1") { . "C:\OEM\lab.env.ps1" }

$LabDir   = "C:\lab"
$NwkDir   = "C:\networker"
$AgentDir = "C:\networker\agent"
$LogFile  = "$LabDir\lab-runner-setup.log"
$Share    = "\\host.lan\Data"
New-Item -ItemType Directory -Force $LabDir | Out-Null
New-Item -ItemType Directory -Force $NwkDir | Out-Null

function Log($m) {
    $line = "$(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')  $m"
    Write-Host $line
    Add-Content -Path $LogFile -Value $line
    try { Add-Content -Path "$Share\lab-runner-setup.log" -Value $line -ErrorAction Stop } catch { }
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

Log "=== lab-runner-setup start: user=$env:USERNAME host=$env:COMPUTERNAME os=$([Environment]::OSVersion.VersionString) agent=$AgentName -> $AgentDashboardUrl"
Set-LabStatus "installing"
if (-not $AgentApiKey) { Set-LabStatus "failed:no-api-key"; Log "lab.env.ps1 carries no AgentApiKey"; exit 1 }

# -- 1. lab plumbing ----------------------------------------------------------
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
    if (-not (Get-NetFirewallRule -DisplayName 'Lab-SSH' -ErrorAction SilentlyContinue)) {
        New-NetFirewallRule -DisplayName 'Lab-SSH' -Direction Inbound -Protocol TCP -LocalPort 22 -Action Allow | Out-Null
    }
    "rules present (outbound is open by default - the runner only dials out)"
}

# -- 2. the real installer: tester --------------------------------------------
Set-LabStatus "tester"
$installer = "C:\OEM\install.ps1"
if (-not (Test-Path $installer)) { Set-LabStatus "failed:no-installer"; Log "C:\OEM\install.ps1 missing"; exit 1 }
Run-Logged "install.ps1 -Yes -Component tester" {
    & powershell.exe -ExecutionPolicy Bypass -NoProfile -NonInteractive -File $installer -Yes -Component tester
    "exit=$LASTEXITCODE"
}
$srcTester = Join-Path $env:USERPROFILE ".cargo\bin\networker-tester.exe"
if (-not (Test-Path $srcTester)) { Set-LabStatus "failed:tester-install"; Log "networker-tester.exe not found at $srcTester"; exit 1 }
$tester = Join-Path $NwkDir "networker-tester.exe"
# The agent runs as SYSTEM: keep the tester outside the user profile (and
# next to the agent, like C:\Program Files\Networker on a cloud tester VM).
Copy-Item -Force $srcTester $tester
Log "tester binary: $tester ($((& $tester --version 2>&1) -join ' '))"

# -- 3. the checkout's agent (published by lab.sh into /oem/agent) ------------
Set-LabStatus "agent"
if (-not (Test-Path "C:\OEM\agent\networker-agent.exe")) { Set-LabStatus "failed:no-agent"; Log "C:\OEM\agent\networker-agent.exe missing (lab.sh publishes it)"; exit 1 }
# Stop a previous instance (re-run over SSH) before overwriting the exe.
schtasks /End /TN 'NetworkerAgent' 2>&1 | Out-Null
Stop-Process -Name 'networker-agent' -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1
New-Item -ItemType Directory -Force $AgentDir | Out-Null
Copy-Item -Force -Recurse "C:\OEM\agent\*" $AgentDir
$agentExe = Join-Path $AgentDir "networker-agent.exe"
Log "agent: $agentExe ($((Get-Item $agentExe).Length) bytes)"

# -- 4. run the agent like a cloud Windows tester VM --------------------------
# Machine env vars = the cloud bootstrap's contract (CloudInitScripts
# WindowsTemplate sets exactly these two + we add the name/tester path); the
# wrapper .cmd repeats them because schtasks /TR carries no environment.
[Environment]::SetEnvironmentVariable('AGENT_DASHBOARD_URL', $AgentDashboardUrl, 'Machine')
[Environment]::SetEnvironmentVariable('AGENT_API_KEY', $AgentApiKey, 'Machine')
[Environment]::SetEnvironmentVariable('AGENT_NAME', $AgentName, 'Machine')
[Environment]::SetEnvironmentVariable('AGENT_TESTER_PATH', $tester, 'Machine')
$wrapper = Join-Path $NwkDir "run-agent.cmd"
@(
    "@echo off",
    "set AGENT_DASHBOARD_URL=$AgentDashboardUrl",
    "set AGENT_API_KEY=$AgentApiKey",
    "set AGENT_NAME=$AgentName",
    "set AGENT_TESTER_PATH=$tester",
    "set RUST_LOG=info",
    "set DOTNET_EnableDiagnostics=0",
    "cd /d `"$AgentDir`"",
    "echo [%DATE% %TIME%] run-agent.cmd: starting networker-agent as %USERNAME% >> `"$LabDir\agent.log`"",
    "`"$agentExe`" >> `"$LabDir\agent.log`" 2>&1"
) | Set-Content -Path $wrapper -Encoding ascii
Run-Logged "schtasks NetworkerAgent (ONSTART, SYSTEM) + start now" {
    schtasks /Create /TN 'NetworkerAgent' /TR "cmd.exe /c `"$wrapper`"" /SC ONSTART /RU SYSTEM /RL HIGHEST /F 2>&1
    schtasks /Run /TN 'NetworkerAgent' 2>&1
}
$up = $false
foreach ($i in 1..30) {
    Start-Sleep -Seconds 2
    if (Get-Process -Name 'networker-agent' -ErrorAction SilentlyContinue) { $up = $true; break }
}
if ($up) {
    Log "networker-agent running (pid $((Get-Process -Name 'networker-agent').Id -join ',')) - the control plane decides 'online'"
    Set-LabStatus "ready"
} else {
    Log "networker-agent did not start within 60s; agent.log tail:"
    Get-Content "$LabDir\agent.log" -Tail 20 -ErrorAction SilentlyContinue | ForEach-Object { Log "   $_" }
    Set-LabStatus "failed:agent-start"
    exit 1
}
Log "=== lab-runner-setup done"
