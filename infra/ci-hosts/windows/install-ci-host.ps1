<#
.SYNOPSIS
  Make a Windows Server 2025 (Evaluation) or Windows 11 Pro VM a CI host for
  this repo: what GitHub calls a "self-hosted runner", labels
  self-hosted,windows,networker-ci. ("Runner" in this repo means a tester VM.)

.DESCRIPTION
  Idempotent; run from an ELEVATED PowerShell (or over an OpenSSH session as an
  administrator, which is elevated by default). Installs what the routed
  Windows jobs need (ci.yml Test (windows-latest), test-installer.yml
  psscriptanalyzer + windows-exec, release.yml x86_64-pc-windows-msvc):
    Chocolatey; git, 7zip, jq, gh, nodejs (22), dotnet-sdk (10), pwsh,
    Visual Studio 2022 Build Tools + C++ workload (rustup's msvc toolchain
    links against it), rustup with stable-x86_64-pc-windows-msvc + rustfmt +
    clippy, the IIS feature (the installer stack tests), PSScriptAnalyzer,
    and the actions-runner package under C:\actions-runner.

  Registration: by default an EPHEMERAL loop (each job gets a fresh
  registration; a fresh registration token is minted from a PAT kept in a
  file only Administrators/SYSTEM can read) run by a Scheduled Task as SYSTEM
  at startup. -AsService instead registers one persistent runner as a
  Windows service (GitHub's --runasservice), which is simpler but keeps one
  long-lived registration.

.PARAMETER Repo        OWNER/REPO
.PARAMETER TokenFile   Path of the PAT file (default C:\ProgramData\ci-host\token)
.PARAMETER Labels      Runner labels (default self-hosted,windows,networker-ci)
.PARAMETER Name        Runner name (default: computer name)
.PARAMETER RunnerGroup Optional runner group
.PARAMETER RunnerVersion  Pin an actions-runner version (default: latest)
.PARAMETER SkipToolchains Only (re)install the runner + loop
.PARAMETER AsService   Persistent service instead of the ephemeral loop

.EXAMPLE
  .\install-ci-host.ps1 -Repo irlm/networker-tester
#>
# Write-Host is the right tool for an operator-facing bootstrap log (same call
# as install.ps1, which carries the identical suppression).
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSAvoidUsingWriteHost', '')]
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Repo,
    [string]$TokenFile = 'C:\ProgramData\ci-host\token',
    [string]$Labels = 'self-hosted,windows,networker-ci',
    [string]$Name = $env:COMPUTERNAME,
    [string]$RunnerGroup = '',
    [string]$RunnerVersion = '',
    [switch]$SkipToolchains,
    [switch]$AsService
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

function Write-Step([string]$Message) { Write-Host "[install-ci-host] $Message" -ForegroundColor Cyan }

$principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Run from an elevated PowerShell.'
}
if ($Repo -notmatch '^[^/]+/[^/]+$') { throw "-Repo must be OWNER/REPO, got '$Repo'" }
if (-not (Test-Path $TokenFile) -or (Get-Item $TokenFile).Length -eq 0) {
    throw "PAT file $TokenFile is missing or empty (a PAT with repository Administration:write)."
}

# Lock the token file down to Administrators + SYSTEM.
$acl = Get-Acl $TokenFile
$acl.SetAccessRuleProtection($true, $false)
$acl.Access | ForEach-Object { [void]$acl.RemoveAccessRule($_) }
foreach ($id in 'BUILTIN\Administrators', 'NT AUTHORITY\SYSTEM') {
    $acl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($id, 'FullControl', 'Allow')))
}
Set-Acl $TokenFile $acl

$RunnerDir = 'C:\actions-runner'
$StateDir = 'C:\ProgramData\ci-host'
New-Item -ItemType Directory -Force $StateDir | Out-Null

if (-not $SkipToolchains) {
    # -- 1. Chocolatey ---------------------------------------------------------
    if (-not (Get-Command choco -ErrorAction SilentlyContinue)) {
        Write-Step 'installing Chocolatey'
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        # Downloaded to a file and executed, not Invoke-Expression'd: the same
        # official installer, but auditable on disk and PSScriptAnalyzer-clean.
        $chocoInstaller = Join-Path $env:TEMP 'choco-install.ps1'
        Invoke-WebRequest -Uri 'https://community.chocolatey.org/install.ps1' -OutFile $chocoInstaller
        & $chocoInstaller
        Remove-Item $chocoInstaller -Force -ErrorAction SilentlyContinue
        $env:Path = [Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' + [Environment]::GetEnvironmentVariable('Path', 'User')
    }
    choco feature enable -n allowGlobalConfirmation | Out-Null

    # -- 2. packages -----------------------------------------------------------
    Write-Step 'choco: git 7zip jq gh nodejs dotnet-sdk pwsh'
    choco install -y --no-progress git 7zip jq gh pwsh
    choco install -y --no-progress nodejs --version=22.12.0
    choco install -y --no-progress dotnet-sdk --version=10.0.100
    Write-Step 'choco: Visual Studio 2022 Build Tools + C++ workload (rustup msvc)'
    choco install -y --no-progress visualstudio2022buildtools
    choco install -y --no-progress visualstudio2022-workload-vctools --package-parameters '--includeRecommended'
    $env:Path = [Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' + [Environment]::GetEnvironmentVariable('Path', 'User')

    # -- 3. rustup (machine-wide so SYSTEM and users share it) -----------------
    $rustupHome = 'C:\rust\rustup'; $cargoHome = 'C:\rust\cargo'
    [Environment]::SetEnvironmentVariable('RUSTUP_HOME', $rustupHome, 'Machine')
    [Environment]::SetEnvironmentVariable('CARGO_HOME', $cargoHome, 'Machine')
    $env:RUSTUP_HOME = $rustupHome; $env:CARGO_HOME = $cargoHome
    if (-not (Test-Path "$cargoHome\bin\rustup.exe")) {
        Write-Step 'installing rustup (stable-x86_64-pc-windows-msvc)'
        Invoke-WebRequest -Uri https://win.rustup.rs/x86_64 -OutFile "$env:TEMP\rustup-init.exe"
        & "$env:TEMP\rustup-init.exe" -y --profile minimal --default-toolchain stable-x86_64-pc-windows-msvc --no-modify-path
    }
    & "$cargoHome\bin\rustup.exe" toolchain install stable-x86_64-pc-windows-msvc --profile minimal --component rustfmt clippy
    & "$cargoHome\bin\rustup.exe" default stable-x86_64-pc-windows-msvc
    $machinePath = [Environment]::GetEnvironmentVariable('Path', 'Machine')
    if ($machinePath -notlike "*$cargoHome\bin*") {
        [Environment]::SetEnvironmentVariable('Path', "$machinePath;$cargoHome\bin", 'Machine')
    }

    # -- 4. IIS (installer stack tests) ----------------------------------------
    Write-Step 'enabling IIS'
    if (Get-Command Install-WindowsFeature -ErrorAction SilentlyContinue) {
        Install-WindowsFeature Web-Server, Web-Mgmt-Console, Web-WebSockets -IncludeManagementTools | Out-Null
    } else {
        Enable-WindowsOptionalFeature -Online -FeatureName IIS-WebServerRole, IIS-WebServer, IIS-ManagementConsole, IIS-WebSockets -All -NoRestart | Out-Null
    }

    # -- 5. PSScriptAnalyzer ---------------------------------------------------
    if (-not (Get-Module -ListAvailable PSScriptAnalyzer)) {
        Write-Step 'installing PSScriptAnalyzer'
        Install-PackageProvider -Name NuGet -Force | Out-Null
        Install-Module -Name PSScriptAnalyzer -Force -Scope AllUsers
    }

    # Windows Defender real-time scanning of target/ halves cargo's speed.
    try { Add-MpPreference -ExclusionPath $RunnerDir, 'C:\rust' -ErrorAction SilentlyContinue } catch { Write-Verbose 'Defender exclusion skipped' }
}

# -- 6. actions-runner package ---------------------------------------------
if (-not $RunnerVersion) {
    $RunnerVersion = (Invoke-RestMethod -Uri https://api.github.com/repos/actions/runner/releases/latest -Headers @{ 'User-Agent' = 'ci-host' }).tag_name.TrimStart('v')
}
$versionFile = Join-Path $RunnerDir '.runner-version'
$installed = if (Test-Path $versionFile) { (Get-Content $versionFile -Raw).Trim('"', "`r", "`n") } else { '' }
if (-not (Test-Path "$RunnerDir\run.cmd") -or $installed -ne $RunnerVersion) {
    Write-Step "installing actions-runner $RunnerVersion into $RunnerDir"
    Get-ScheduledTask -TaskName 'ci-host-loop' -ErrorAction SilentlyContinue | Stop-ScheduledTask -ErrorAction SilentlyContinue
    Get-Service -Name 'actions.runner.*' -ErrorAction SilentlyContinue | Stop-Service -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force $RunnerDir | Out-Null
    $zip = "$env:TEMP\actions-runner.zip"
    Invoke-WebRequest -Uri "https://github.com/actions/runner/releases/download/v$RunnerVersion/actions-runner-win-x64-$RunnerVersion.zip" -OutFile $zip
    Expand-Archive -Path $zip -DestinationPath $RunnerDir -Force
    Remove-Item $zip -Force
    Set-Content -Path $versionFile -Value "`"$RunnerVersion`""
}
@(
    'DOTNET_CLI_TELEMETRY_OPTOUT=1',
    'RUSTUP_HOME=C:\rust\rustup',
    'CARGO_HOME=C:\rust\cargo'
) | Set-Content -Path (Join-Path $RunnerDir '.env')

# -- 7. registration -------------------------------------------------------
function Get-RegistrationToken {
    $pat = (Get-Content $TokenFile -Raw).Trim()
    $r = Invoke-RestMethod -Method Post -Uri "https://api.github.com/repos/$Repo/actions/runners/registration-token" `
        -Headers @{ Authorization = "Bearer $pat"; Accept = 'application/vnd.github+json'; 'X-GitHub-Api-Version' = '2022-11-28'; 'User-Agent' = 'ci-host' }
    return $r.token
}

$groupArgs = @()
if ($RunnerGroup) { $groupArgs = @('--runnergroup', $RunnerGroup) }

if ($AsService) {
    Write-Step 'registering ONE persistent runner as a Windows service'
    if (Test-Path "$RunnerDir\.runner") { & "$RunnerDir\config.cmd" remove --token (Get-RegistrationToken) | Out-Null }
    & "$RunnerDir\config.cmd" --unattended --replace --url "https://github.com/$Repo" --token (Get-RegistrationToken) `
        --name $Name --labels $Labels --work _work --runasservice @groupArgs
} else {
    Write-Step 'installing the ephemeral loop as a Scheduled Task (SYSTEM, at startup)'
    $loop = @"
`$ErrorActionPreference = 'Continue'
`$env:RUSTUP_HOME = 'C:\rust\rustup'; `$env:CARGO_HOME = 'C:\rust\cargo'
while (`$true) {
  try {
    `$pat = (Get-Content '$TokenFile' -Raw).Trim()
    `$reg = (Invoke-RestMethod -Method Post -Uri 'https://api.github.com/repos/$Repo/actions/runners/registration-token' -Headers @{ Authorization = "Bearer `$pat"; Accept = 'application/vnd.github+json'; 'X-GitHub-Api-Version' = '2022-11-28'; 'User-Agent' = 'ci-host' }).token
    Remove-Variable pat
  } catch { Write-Host "token mint failed: `$(`$_.Exception.Message) - retrying in 60s"; Start-Sleep 60; continue }
  & '$RunnerDir\config.cmd' --unattended --ephemeral --replace --url 'https://github.com/$Repo' --token `$reg --name '$Name' --labels '$Labels' --work _work $($groupArgs -join ' ')
  if (`$LASTEXITCODE -ne 0) { Write-Host 'config.cmd failed - retrying in 30s'; Start-Sleep 30; continue }
  & '$RunnerDir\run.cmd'
  Remove-Item '$RunnerDir\.runner', '$RunnerDir\.credentials', '$RunnerDir\.credentials_rsaparams' -Force -ErrorAction SilentlyContinue
  Start-Sleep 3
}
"@
    $loopPath = Join-Path $StateDir 'ci-host-loop.ps1'
    Set-Content -Path $loopPath -Value $loop -Encoding ASCII
    $action = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument "-NoProfile -ExecutionPolicy Bypass -File `"$loopPath`"" -WorkingDirectory $RunnerDir
    $trigger = New-ScheduledTaskTrigger -AtStartup
    $settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
    Register-ScheduledTask -TaskName 'ci-host-loop' -Action $action -Trigger $trigger -Settings $settings -User 'NT AUTHORITY\SYSTEM' -RunLevel Highest -Force | Out-Null
    Start-ScheduledTask -TaskName 'ci-host-loop'
}

Write-Step "done - CI host '$Name' for $Repo with labels $Labels"
Write-Host '  Server 2025 Evaluation: 180 days; `slmgr /rearm` extends up to 5 times (docs/self-hosted-ci.md section "Windows licensing").'
